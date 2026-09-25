//! The craft bindings the stock `Blizzard_CraftUI` window calls, over the recipe snapshot the app
//! pushes ([`UiScript::set_craft`]); `DoCraft` and `CloseCraft` queue intents the app drains. The
//! window has one action button, `DoCraft(GetCraftSelectionIndex())` (`Blizzard_CraftUI.xml:667`),
//! and no Create All, so a craft intent is a bare spell id.
//!
//! Not built: the reference's header rows (`0x4f60c0`). The list is flat, `GetCraftInfo` never
//! answers `"header"` and `Expand`/`CollapseCraftSkillLine` do nothing; Enchanting, with a single
//! group, renders flat in the reference too.

use mlua::{Lua, MultiValue, Value, Variadic};

use super::binding_abi::{self, number_arg};
use super::item_stats::item_link;
use super::Model;

/// One reagent a recipe consumes (`GetCraftReagentInfo`); name and icon are `None` until the item
/// template answer lands.
#[derive(Clone, Debug, PartialEq)]
pub struct CraftReagent {
    /// The item entry `GameTooltip:SetCraftItem` resolves through.
    pub item: u32,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub need: u32,
    pub have: u32,
}

/// What `GameTooltip:SetCraftSpell` (`0x533e90`) describes for a row, resolved by the app from the
/// recipe's own `Effect[i]`: 24 `CREATE_ITEM` picks the item, 36 `LEARN_SPELL` the taught spell.
/// Unlike the trainer's, it never tests 57 (`LEARN_PET_SPELL`), sets no altCaster and never checks
/// that the target resolves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CraftTooltip {
    /// The item builder (`0x52b650`) on the matched `CREATE_ITEM` slot's `EffectItemType`.
    Item(u32),
    /// The spell builder (`0x52e610`) on the matched `LEARN_SPELL` slot's `EffectTriggerSpell`, or
    /// on the recipe itself when no slot matches, as for every enchant.
    Spell(u32),
}

impl Default for CraftTooltip {
    fn default() -> Self {
        CraftTooltip::Spell(0)
    }
}

/// One recipe row: a known `SPELL_ATTR_IS_TRADESKILL` spell, resolved by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct CraftRecipe {
    /// What `DoCraft` queues for `CMSG_CAST_SPELL`.
    pub spell_id: u32,
    pub name: String,
    /// The rank subtext (`craftSubSpellName`), usually empty for an enchant; the stock window
    /// paints it in parentheses when set (`Blizzard_CraftUI.lua:217-221`).
    pub sub_name: String,
    /// `GetCraftInfo`'s `craftType`: one of the four keys `CraftTypeColor` colours as
    /// `TradeSkillTypeColor` does (`Blizzard_CraftUI.lua:6-13`). The reference's table
    /// (`0x807e00`) adds `"none"` and `"used"` (a still-optimal Beast Training recipe teaching a
    /// learnable spell); Enchanting never produces either, and neither is built.
    pub difficulty: super::TradeSkillDifficulty,
    /// `numAvailable`: the least `floor(have / need)` over the reagents.
    pub num_available: u32,
    pub icon: Option<String>,
    /// `GetCraftDescription`, `$`-tokens substituted by the app; `None` paints a blank line
    /// (`Blizzard_CraftUI.lua:316`).
    pub description: Option<String>,
    /// An `ENCHANT_ITEM` effect (`TARGET_FLAG_ITEM`, `TARGET_FLAG_TRADE_ITEM`): `DoCraft` then arms
    /// an app-side item pick. No Lua getter reads it.
    pub needs_item_target: bool,
    pub reagents: Vec<CraftReagent>,
    /// The Requirements line's `(tool name, have)` pairs (`GetCraftSpellFocus`).
    pub tools: Vec<(String, bool)>,
    pub tooltip: CraftTooltip,
    /// `Spell.dbc` `spellLevel` (`+0x74`), the Beast Training comparator's rank key. The
    /// reference's `requiredLevel` return comes from it; here that return is 0.
    pub spell_level: u32,
}

/// The open craft window, pushed whole by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct CraftState {
    /// The title `GetCraftName` answers, also `GetCraftDisplaySkillLine`'s name.
    pub name: String,
    pub rank: u32,
    pub max_rank: u32,
    /// The opener spell's `EffectMiscValue[0]`, stored at `ds:0xbdcfb8`: 1 Beast Training, 3
    /// Enchanting. It picks the row comparator (`0x4f6765`).
    pub craft_type: u32,
    /// In any order; [`UiScript::set_craft`] sorts them.
    pub recipes: Vec<CraftRecipe>,
}

/// Beast Training's craft type, the `EffectMiscValue[0]` of spell 5149.
const CRAFT_TYPE_BEAST_TRAINING: u32 = 1;

/// The craft row order: difficulty tier (`[+0xc]`) ascending, then name (collator `0x64a4c0`),
/// then, at Beast Training only (`0x4f6920`; every other type uses `0x4f67a0`), `spellLevel`
/// ascending, which orders a pet ability's ranks.
///
/// Deviation: a last `spell_id` key breaks the remaining ties (Charge's six ranks all have
/// `spellLevel` 0), because the reference leaves them to its qsort over an array benilla does not
/// reproduce; ascending id is ascending rank in every 1.12 pet-ability family.
fn recipe_order(a: &CraftRecipe, b: &CraftRecipe, craft_type: u32) -> std::cmp::Ordering {
    a.difficulty
        .tier()
        .cmp(&b.difficulty.tier())
        .then_with(|| collate(&a.name, &b.name))
        .then_with(|| {
            if craft_type == CRAFT_TYPE_BEAST_TRAINING {
                a.spell_level.cmp(&b.spell_level)
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .then_with(|| a.spell_id.cmp(&b.spell_id))
}

/// Approximates the case-insensitive enUS collator the craft comparators call (`0x64a4c0`; the
/// trainer rows use `0x64a480`): case-folded order, raw bytes breaking ties.
fn collate(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

impl super::UiScript {
    /// Push the open craft window's snapshot, or clear it with `None`. A push sorts the rows and
    /// keeps the selection on its spell id, clearing it when that recipe is gone.
    pub fn set_craft(&mut self, state: Option<CraftState>) {
        let mut model = self.model_mut();
        match state {
            None => {
                model.craft_selection = 0;
                model.craft = None;
            }
            Some(mut s) => {
                let craft_type = s.craft_type;
                s.recipes.sort_by(|a, b| recipe_order(a, b, craft_type));
                let prev_spell_id = model
                    .craft_selection
                    .checked_sub(1)
                    .and_then(|i| model.craft.as_ref().and_then(|c| c.recipes.get(i as usize)))
                    .map(|r| r.spell_id);
                model.craft_selection = prev_spell_id
                    .and_then(|sid| s.recipes.iter().position(|r| r.spell_id == sid))
                    .map_or(0, |i| (i + 1) as u32);
                model.craft = Some(s);
            }
        }
    }

    /// Drain the spell ids `DoCraft` queued, one per call, with no count.
    pub fn take_craft_dos(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().craft_dos)
    }

    /// Whether `CloseCraft` was called since the last drain; the close sends no packet.
    pub fn take_craft_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().craft_close)
    }
}

/// The recipe at a 1-based index.
fn recipe(model: &Model, index: usize) -> Option<&CraftRecipe> {
    let n = index.checked_sub(1)?;
    model.craft.as_ref()?.recipes.get(n)
}

fn num_recipes(model: &Model) -> usize {
    model.craft.as_ref().map_or(0, |c| c.recipes.len())
}

fn opt_str(lua: &Lua, s: Option<&String>) -> mlua::Result<Value> {
    Ok(match s {
        Some(s) => Value::String(lua.create_string(s)?),
        None => Value::Nil,
    })
}

/// Register the craft globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetCraftName() → the title, or "UNKNOWN" when closed (the reference's closed answer is
    // untraced); the stock window sets its title unconditionally (`Blizzard_CraftUI.lua:101`).
    g.set(
        "GetCraftName",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let name = model
                .craft
                .as_ref()
                .map_or_else(|| "UNKNOWN".to_string(), |c| c.name.clone());
            Ok(Value::String(lua.create_string(&name)?))
        })?,
    )?;

    // GetCraftDisplaySkillLine() → name, rank, maxRank; the name is nil when closed, which hides
    // the rank bar (`Blizzard_CraftUI.lua:108-109`).
    g.set(
        "GetCraftDisplaySkillLine",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (name, rank, max_rank) = match &model.craft {
                Some(c) => (Some(c.name.clone()), c.rank, c.max_rank),
                None => (None, 0, 0),
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(lua, name.as_ref())?,
                Value::Integer(i64::from(rank)),
                Value::Integer(i64::from(max_rank)),
            ]))
        })?,
    )?;

    // GetNumCrafts() → the recipe count, 0 when closed.
    g.set(
        "GetNumCrafts",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(num_recipes(&model) as i64)
        })?,
    )?;

    // GetCraftItemLink(index) (`0x4f72a0`): a non-number raises its usage. The reference keys on
    // the spell's `castUI`: 3, Enchanting's craft type, for which the type stands in here, answers
    // `|cffffffff|Henchant:<spell id>|h[<spell name>]|h|r` and cannot miss, even for the twenty
    // recipes that create an item; any other type scans for a `CREATE_ITEM` effect no shipped row
    // has, so Beast Training answers zero values.
    g.set(
        "GetCraftItemLink",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetCraftItemLink(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let link = usize::try_from(index)
                .ok()
                .and_then(|i| recipe(&model, i))
                .filter(|_| model.craft.as_ref().is_some_and(|c| c.craft_type == 3))
                .map(|r| format!("|cffffffff|Henchant:{}|h[{}]|h|r", r.spell_id, r.name));
            Ok(match link {
                Some(l) => MultiValue::from_vec(vec![Value::String(lua.create_string(&l)?)]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // GetCraftReagentItemLink(index, reagentIndex) (`0x4f7730`): both arguments number-gated,
    // `reagentIndex` counting the non-empty slots from 1; always one value, the item link or nil.
    g.set(
        "GetCraftReagentItemLink",
        lua.create_function(|lua, (index, reagent): (Value, Value)| {
            let usage = "Usage: GetCraftReagentItemLink(index, reagentIndex)";
            let index = number_arg(lua, index, usage)?;
            let reagent = number_arg(lua, reagent, usage)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let link = usize::try_from(index)
                .ok()
                .and_then(|i| recipe(&model, i))
                .zip(usize::try_from(reagent).ok().and_then(|r| r.checked_sub(1)))
                .and_then(|(r, ri)| r.reagents.get(ri))
                .and_then(|re| {
                    model
                        .item_templates
                        .get(&re.item)
                        .map(|t| item_link(re.item, &t.name, t.quality))
                });
            Ok(match link {
                Some(l) => Value::String(lua.create_string(&l)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // GetCraftInfo(index) → craftName, craftSubSpellName, craftType, numAvailable, isExpanded,
    // trainingPointCost, requiredLevel (`Blizzard_CraftUI.lua:168`); one nil past the end.
    // `isExpanded` is always nil; Beast Training's cost and level answer 0, not built.
    g.set(
        "GetCraftInfo",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(r) = recipe(&model, index) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&r.name)?),
                Value::String(lua.create_string(&r.sub_name)?),
                Value::String(lua.create_string(r.difficulty.as_str())?),
                Value::Integer(i64::from(r.num_available)),
                Value::Nil,
                Value::Integer(0),
                Value::Integer(0),
            ]))
        })?,
    )?;

    // GetCraftIcon(index) → the icon path, nil while in flight or past the end.
    g.set(
        "GetCraftIcon",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            opt_str(lua, recipe(&model, index).and_then(|r| r.icon.as_ref()))
        })?,
    )?;

    // GetCraftDescription(index) → the description or nil, which the stock window tests before
    // painting (`Blizzard_CraftUI.lua:312`).
    g.set(
        "GetCraftDescription",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            opt_str(
                lua,
                recipe(&model, index).and_then(|r| r.description.as_ref()),
            )
        })?,
    )?;

    // GetCraftNumReagents(index) → the reagent count, 0 past the end or when closed.
    g.set(
        "GetCraftNumReagents",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(recipe(&model, index).map_or(0, |r| r.reagents.len()) as i64)
        })?,
    )?;

    // GetCraftReagentInfo(index, reagentIndex) → name, icon, need, have; one nil past the end.
    g.set(
        "GetCraftReagentInfo",
        lua.create_function(|lua, (index, reagent_index): (usize, usize)| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(reagent) = recipe(&model, index)
                .and_then(|r| reagent_index.checked_sub(1).and_then(|n| r.reagents.get(n)))
            else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(lua, reagent.name.as_ref())?,
                opt_str(lua, reagent.icon.as_ref())?,
                Value::Integer(i64::from(reagent.need)),
                Value::Integer(i64::from(reagent.have)),
            ]))
        })?,
    )?;

    // GetCraftSpellFocus(index) → name, has for each required tool, alternating, the shape
    // `BuildColoredListString` takes (`Blizzard_CraftUI.lua:361`); nothing when there are none.
    g.set(
        "GetCraftSpellFocus",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::new();
            if let Some(r) = recipe(&model, index) {
                for (name, have) in &r.tools {
                    out.push(Value::String(lua.create_string(name)?));
                    out.push(binding_abi::flag(*have));
                }
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetCraftButtonToken() → "CREATE", the global naming the button's label
    // (`Blizzard_CraftUI.lua:106`); Beast Training's "TRAIN" is not built.
    g.set(
        "GetCraftButtonToken",
        lua.create_function(|lua, ()| Ok(Value::String(lua.create_string("CREATE")?)))?,
    )?;

    // SelectCraft(index) / GetCraftSelectionIndex(): the selection, 1-based with 0 for none; an
    // out-of-range select clears it, and the getter answers 0 past the recipe count.
    g.set(
        "SelectCraft",
        lua.create_function(|lua, index: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let count = num_recipes(&model) as u32;
            model.craft_selection = if index >= 1 && index <= count {
                index
            } else {
                0
            };
            Ok(())
        })?,
    )?;
    g.set(
        "GetCraftSelectionIndex",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let count = num_recipes(&model) as u32;
            let sel = model.craft_selection;
            Ok(i64::from(if sel >= 1 && sel <= count { sel } else { 0 }))
        })?,
    )?;

    // DoCraft(index): queue the row's spell id; out of range is ignored.
    g.set(
        "DoCraft",
        lua.create_function(|lua, index: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(spell_id) = recipe(&model, index).map(|r| r.spell_id) {
                model.craft_dos.push(spell_id);
            }
            Ok(())
        })?,
    )?;

    // CloseCraft(): no packet; the flag tells the app to clear its state.
    g.set(
        "CloseCraft",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.craft_close = true;
            Ok(())
        })?,
    )?;

    // Expand/CollapseCraftSkillLine(index): no-ops over the flat list (module doc).
    g.set(
        "ExpandCraftSkillLine",
        lua.create_function(|_, _args: Variadic<Value>| Ok(()))?,
    )?;
    g.set(
        "CollapseCraftSkillLine",
        lua.create_function(|_, _args: Variadic<Value>| Ok(()))?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    /// Enchanting's craft type, whose comparator has no `spellLevel` key.
    const CRAFT_TYPE_ENCHANTING: u32 = 3;

    fn recipe(
        spell_id: u32,
        name: &str,
        difficulty: super::super::TradeSkillDifficulty,
        num_available: u32,
    ) -> CraftRecipe {
        CraftRecipe {
            spell_id,
            tooltip: CraftTooltip::Spell(spell_id),
            name: name.into(),
            sub_name: String::new(),
            difficulty,
            num_available,
            icon: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
            description: Some(format!("{name} description.")),
            needs_item_target: true,
            reagents: vec![CraftReagent {
                item: 10940,
                name: Some("Illusion Dust".into()),
                icon: Some("Interface\\Icons\\INV_Enchant_EssenceMagicSmall".into()),
                need: 1,
                have: 3,
            }],
            tools: vec![("Runed Copper Rod".into(), true)],
            spell_level: 0,
        }
    }

    /// Two Enchanting recipes, pushed trivial first; the sort puts Minor Health at index 1.
    fn state() -> CraftState {
        CraftState {
            name: "Enchanting".into(),
            rank: 57,
            max_rank: 75,
            craft_type: CRAFT_TYPE_ENCHANTING,
            recipes: vec![
                recipe(
                    7418,
                    "Enchant Weapon - Minor Beastslaying",
                    super::super::TradeSkillDifficulty::Trivial,
                    5,
                ),
                recipe(
                    683,
                    "Enchant Weapon - Minor Health",
                    super::super::TradeSkillDifficulty::Optimal,
                    0,
                ),
            ],
        }
    }

    #[test]
    fn snapshot_feeds_the_api_tuples() {
        let mut s = UiScript::new().unwrap();
        s.set_craft(Some(state()));

        assert_eq!(
            s.eval::<String>("return GetCraftName()").unwrap(),
            "Enchanting"
        );
        let (name, rank, max_rank) = s
            .eval::<(String, i64, i64)>("return GetCraftDisplaySkillLine()")
            .unwrap();
        assert_eq!((name.as_str(), rank, max_rank), ("Enchanting", 57, 75));
        assert_eq!(s.eval::<i64>("return GetNumCrafts()").unwrap(), 2);

        let (n, sub, kind, avail, expanded, tp, lvl) = s
            .eval::<(String, String, String, i64, Option<i64>, i64, i64)>(
                "local n,s,t,a,e,tp,l = GetCraftInfo(1) return n,s,t,a,e,tp,l",
            )
            .unwrap();
        assert_eq!(
            (
                n.as_str(),
                sub.as_str(),
                kind.as_str(),
                avail,
                expanded,
                tp,
                lvl
            ),
            (
                "Enchant Weapon - Minor Health",
                "",
                "optimal",
                0,
                None,
                0,
                0
            )
        );
        let (_, _, kind2, avail2, _, _, _) = s
            .eval::<(String, String, String, i64, Option<i64>, i64, i64)>(
                "local n,s,t,a,e,tp,l = GetCraftInfo(2) return n,s,t,a,e,tp,l",
            )
            .unwrap();
        assert_eq!((kind2.as_str(), avail2), ("trivial", 5));

        assert_eq!(
            s.eval::<String>("return GetCraftIcon(1)").unwrap(),
            "Interface\\Icons\\Spell_683"
        );
        assert_eq!(
            s.eval::<String>("return GetCraftDescription(1)").unwrap(),
            "Enchant Weapon - Minor Health description."
        );
        assert_eq!(s.eval::<i64>("return GetCraftNumReagents(1)").unwrap(), 1);
        let (rname, ricon, need, have) = s
            .eval::<(String, String, i64, i64)>("return GetCraftReagentInfo(1, 1)")
            .unwrap();
        assert_eq!(
            (rname.as_str(), ricon.as_str(), need, have),
            (
                "Illusion Dust",
                "Interface\\Icons\\INV_Enchant_EssenceMagicSmall",
                1,
                3
            )
        );
        assert_eq!(
            s.eval::<String>("return GetCraftButtonToken()").unwrap(),
            "CREATE"
        );

        assert!(s
            .eval::<bool>("local _,_,t = GetCraftInfo(1) return t ~= 'header'")
            .unwrap());
        // Expand and collapse do nothing.
        s.run("ExpandCraftSkillLine(1) CollapseCraftSkillLine(1)")
            .unwrap();
        assert_eq!(s.eval::<i64>("return GetNumCrafts()").unwrap(), 2);
    }

    #[test]
    fn selection_persists_across_a_repush_by_spell_id() {
        let mut s = UiScript::new().unwrap();
        // Minor Beastslaying (spell 7418) sorts to index 2 (trivial, behind the optimal row).
        s.set_craft(Some(state()));
        s.run("SelectCraft(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetCraftSelectionIndex()").unwrap(), 2);

        // Turning 7418 optimal moves it to index 1, and the selection follows the spell id.
        let mut promoted = state();
        promoted.recipes[0].difficulty = super::super::TradeSkillDifficulty::Optimal;
        promoted.recipes[0].name = "Aaa Beastslaying".into();
        s.set_craft(Some(promoted));
        assert_eq!(s.eval::<i64>("return GetCraftSelectionIndex()").unwrap(), 1);

        let mut without_7418 = state();
        without_7418.recipes.remove(0);
        s.set_craft(Some(without_7418));
        assert_eq!(s.eval::<i64>("return GetCraftSelectionIndex()").unwrap(), 0);
    }

    #[test]
    fn do_craft_drains_bare_spell_ids_no_count() {
        let mut s = UiScript::new().unwrap();
        s.set_craft(Some(state()));

        s.run("DoCraft(1) DoCraft(2)").unwrap();
        assert_eq!(
            s.take_craft_dos(),
            vec![683, 7418],
            "display order, not push order"
        );
        assert!(s.take_craft_dos().is_empty(), "drained");

        s.run("DoCraft(9)").unwrap();
        assert!(s.take_craft_dos().is_empty());
    }

    #[test]
    fn no_snapshot_shapes_unknown_name_and_nil_info() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<String>("return GetCraftName()").unwrap(),
            "UNKNOWN"
        );
        assert!(s
            .eval::<bool>("return GetCraftDisplaySkillLine() == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetNumCrafts()").unwrap(), 0);
        assert!(s.eval::<bool>("return GetCraftInfo(1) == nil").unwrap());
        assert!(s.eval::<bool>("return GetCraftIcon(1) == nil").unwrap());
        assert!(s
            .eval::<bool>("return GetCraftDescription(1) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetCraftNumReagents(1)").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetCraftReagentInfo(1, 1) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetCraftSelectionIndex()").unwrap(), 0);

        s.run("DoCraft(1)").unwrap();
        assert!(s.take_craft_dos().is_empty(), "no window, no intent");
    }

    #[test]
    fn get_craft_spell_focus_multivalue_shape() {
        let mut s = UiScript::new().unwrap();
        let mut c = state();
        // recipes[1] (Minor Health, optimal) is the row the sort puts at index 1.
        c.recipes[1].tools = vec![
            ("Runed Copper Rod".into(), true),
            ("Arcanite Rod".into(), false),
        ];
        s.set_craft(Some(c));

        let (a, b, cc, d) = s
            .eval::<(String, Option<i64>, String, Option<i64>)>(
                "local a,b,c,d = GetCraftSpellFocus(1) return a,b,c,d",
            )
            .unwrap();
        assert_eq!(
            (a.as_str(), b, cc.as_str(), d),
            ("Runed Copper Rod", Some(1), "Arcanite Rod", None)
        );

        // No tools: zero values, not one nil.
        let mut c2 = state();
        c2.recipes[1].tools.clear();
        s.set_craft(Some(c2));
        assert_eq!(s.arity("GetCraftSpellFocus(1)").unwrap(), 0);
    }

    #[test]
    fn close_and_clear_reset_the_window() {
        let mut s = UiScript::new().unwrap();
        s.set_craft(Some(state()));
        s.run("SelectCraft(1)").unwrap();

        assert!(!s.take_craft_close());
        s.run("CloseCraft()").unwrap();
        assert!(s.take_craft_close());
        assert!(!s.take_craft_close(), "drained");

        s.set_craft(None);
        assert_eq!(s.eval::<i64>("return GetNumCrafts()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetCraftSelectionIndex()").unwrap(), 0);
    }

    /// The expected order is an emulated run of `0x4f6920` over the shipped `Spell.dbc` values.
    #[test]
    fn beast_training_orders_ranks_by_spell_level() {
        let rank = |spell_id: u32, name: &str, level: u32| CraftRecipe {
            spell_id,
            tooltip: CraftTooltip::Spell(spell_id),
            name: name.into(),
            sub_name: format!("Rank {}", level / 10 - 1),
            difficulty: super::super::TradeSkillDifficulty::Trivial,
            num_available: 0,
            icon: None,
            description: None,
            needs_item_target: false,
            reagents: vec![],
            tools: vec![],
            spell_level: level,
        };
        // Shuffled on purpose.
        let recipes = vec![
            rank(24508, "Arcane Resistance", 30),
            rank(24441, "Fire Resistance", 30),
            rank(24495, "Arcane Resistance", 20),
            rank(24464, "Fire Resistance", 50),
            rank(24510, "Arcane Resistance", 50),
            rank(24440, "Fire Resistance", 20),
            rank(24509, "Arcane Resistance", 40),
            rank(24463, "Fire Resistance", 40),
        ];
        let mut s = UiScript::new().unwrap();
        s.set_craft(Some(CraftState {
            name: "Beast Training".into(),
            rank: 0,
            max_rank: 0,
            craft_type: CRAFT_TYPE_BEAST_TRAINING,
            recipes: recipes.clone(),
        }));
        let seen: Vec<(String, String)> = (1..=8)
            .map(|i| {
                s.eval::<(String, String)>(&format!("local n,sub = GetCraftInfo({i}) return n,sub"))
                    .unwrap()
            })
            .collect();
        let got: Vec<String> = seen.iter().map(|(n, sub)| format!("{n} ({sub})")).collect();
        assert_eq!(
            got,
            [
                "Arcane Resistance (Rank 1)",
                "Arcane Resistance (Rank 2)",
                "Arcane Resistance (Rank 3)",
                "Arcane Resistance (Rank 4)",
                "Fire Resistance (Rank 1)",
                "Fire Resistance (Rank 2)",
                "Fire Resistance (Rank 3)",
                "Fire Resistance (Rank 4)",
            ]
        );

        // The same rows at the Enchanting type take `0x4f67a0`, with no `spellLevel` key: the ranks
        // fall to the spell-id tie-break, benilla's order rather than the reference's.
        s.set_craft(Some(CraftState {
            name: "Beast Training".into(),
            rank: 0,
            max_rank: 0,
            craft_type: CRAFT_TYPE_ENCHANTING,
            recipes,
        }));
        let ids: Vec<i64> = (1..=8)
            .map(|i| {
                s.eval::<i64>(&format!("DoCraft({i}) return 0")).unwrap();
                0
            })
            .collect();
        assert_eq!(ids.len(), 8);
        assert_eq!(
            s.take_craft_dos(),
            vec![24495, 24508, 24509, 24510, 24440, 24441, 24463, 24464],
            "no spellLevel key at type 3 — the order is name then the deterministic id tie-break"
        );
    }

    #[test]
    fn the_craft_link_verbs_answer_the_clients_shapes() {
        let mut s = UiScript::new().unwrap();
        s.set_craft(Some(state()));
        // Row 1 is whichever recipe sorts first.
        let first = s.eval::<String>("return (GetCraftInfo(1))").unwrap();
        let spell = state()
            .recipes
            .iter()
            .find(|r| r.name == first)
            .map(|r| r.spell_id)
            .expect("row 1 is a pushed recipe");
        assert_eq!(
            s.eval::<String>("return (GetCraftItemLink(1))").unwrap(),
            format!("|cffffffff|Henchant:{spell}|h[{first}]|h|r")
        );
        assert!(s
            .eval::<bool>("return GetCraftReagentItemLink(1, 1) == nil")
            .unwrap());
        s.set_item_template(
            10940,
            crate::script::ItemTemplateView {
                name: "Illusion Dust".into(),
                quality: 2,
                ..Default::default()
            },
        );
        assert_eq!(
            s.eval::<String>("return GetCraftReagentItemLink(1, 1)")
                .unwrap(),
            "|cff1eff00|Hitem:10940:0:0:0|h[Illusion Dust]|h|r"
        );
        assert_eq!(s.arity("GetCraftReagentItemLink(1, 5)").unwrap(), 1);
        assert!(s
            .eval::<bool>("return GetCraftReagentItemLink(1, 5) == nil")
            .unwrap());
        for bad in ["GetCraftItemLink(nil)", "GetCraftReagentItemLink(1)"] {
            let err = s.run(bad).expect_err(bad).to_string();
            assert!(err.contains("Usage: GetCraft"), "{bad}: {err}");
        }
        let mut beasts = state();
        beasts.craft_type = 1;
        s.set_craft(Some(beasts));
        assert_eq!(
            s.arity("GetCraftItemLink(1)").unwrap(),
            0,
            "a non-Enchanting craft answers zero values"
        );
    }
}
