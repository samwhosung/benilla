//! The tradeskill bindings behind the stock `Blizzard_TradeSkillUI`: the app pushes a snapshot
//! ([`UiScript::set_trade_skill`]) with recipes resolved from `Spell.dbc`, `SkillLineAbility.dbc`
//! and bag counts, and applies the calls `DoTradeSkill` and `CloseTradeSkill` make. The engine
//! owns the grouped, filtered list (`0x4fca20`); every Lua index, `SetTradeSkillItem`'s included,
//! is 1-based into its whole sorted array, the displayed rows first ([`recipe_at`]). The filter,
//! fold, repeat and close verbs fire their events inside the call, as the reference's do.

use std::collections::HashSet;

mod api;

/// A profession window verb the host applies inside its call ([`super::ScriptCall::TradeSkill`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeSkillCall {
    /// `DoTradeSkill(index, count)` past its gate (`0x500280`): the recipe's cast (`0x500314` →
    /// `TryCastSpell 0x6e5a90`), `count` the clamped repeat it armed.
    Do { spell_id: u32, count: u32 },
    /// `CloseTradeSkill()` (`0x4fd7f0` → `0x4fc870`): the open marker `[0xbde040]` zeroed.
    Close,
}

/// Hand `call` to the host, which applies it before this returns ([`super::calls::submit`]).
fn submit(lua: &mlua::Lua, call: TradeSkillCall) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::TradeSkill(call))
}
mod itemstats;
pub(crate) mod view;

/// A build's lookups (`0x4fcd4d`): the pending count started again (`0x4fc8cb`) and taken up by
/// each product whose record is in flight (`0x4fcd58`), each waited on with the callback
/// `0x4fc8e0`, which no cancel removes (`0x4fc890`).
fn note_build(model: &mut super::Model) {
    let misses: Vec<u32> = model
        .trade_skill
        .as_ref()
        .map(|t| t.recipes.iter().filter_map(|r| r.icon_pending).collect())
        .unwrap_or_default();
    model.trade_skill_pending = misses.len() as u32;
    model.trade_skill_build_waits.extend(misses);
}

impl TradeSkillState {
    /// Each reagent's carried count from `count` (`0x622130`) and each recipe's makeable count
    /// from them (`0x4fcce0`-`0x4fcf19`).
    pub fn recount(&mut self, count: &dyn Fn(u32) -> u32) {
        for r in &mut self.recipes {
            for re in &mut r.reagents {
                re.have = count(re.item);
            }
            r.num_available = makeable(r.reagents.iter().map(|re| (re.have, re.need)));
        }
    }
}

/// The build's makeable count (`0x4fcce0`-`0x4fcf19`): the least of each reagent's carried count
/// over its need, 0 for a recipe with no reagent.
pub(crate) fn makeable(reagents: impl Iterator<Item = (u32, u32)>) -> u32 {
    reagents
        .map(|(have, need)| have / need.max(1))
        .min()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;

pub(super) use api::install;
pub(crate) use view::recipe_at;

/// A recipe's difficulty band, computed app-side: gray at or above trivialHigh, green from the
/// low/high midpoint, yellow from trivialLow, orange below (`0x4fca20`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeSkillDifficulty {
    /// Orange: a near-certain skill-up.
    Optimal,
    /// Yellow: a good skill-up chance.
    Medium,
    /// Green: a fading skill-up chance.
    Easy,
    /// Gray: no skill-up.
    Trivial,
}

impl TradeSkillDifficulty {
    /// A recipe row's `type` in `GetTradeSkillInfo`, a `TradeSkillTypeColor` key
    /// (`Blizzard_TradeSkillUI.lua:7-10`).
    pub fn as_str(self) -> &'static str {
        match self {
            TradeSkillDifficulty::Optimal => "optimal",
            TradeSkillDifficulty::Medium => "medium",
            TradeSkillDifficulty::Easy => "easy",
            TradeSkillDifficulty::Trivial => "trivial",
        }
    }

    /// The tier byte recipes sort by, ascending (`0x4fd380`). The Craft window's comparators key on
    /// the same order (`row[+0xc]`, shifted by one for a "none" tier), so it shares this.
    pub(crate) fn tier(self) -> u8 {
        match self {
            TradeSkillDifficulty::Optimal => 0,
            TradeSkillDifficulty::Medium => 1,
            TradeSkillDifficulty::Easy => 2,
            TradeSkillDifficulty::Trivial => 3,
        }
    }
}

/// One reagent of a recipe, as `GetTradeSkillReagentInfo` reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillReagent {
    /// The item entry `SetTradeSkillItem(i, j)` resolves through.
    pub item: u32,
    /// `None` until the item template arrives.
    pub name: Option<String>,
    /// `None` until the item template arrives.
    pub icon: Option<String>,
    /// How many this recipe consumes.
    pub need: u32,
    /// How many the player's bags hold.
    pub have: u32,
}

/// One recipe: a known spell with `SPELL_ATTR_IS_TRADESKILL`, resolved app-side from `Spell.dbc`,
/// `SkillLineAbility.dbc` and bag counts.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillRecipe {
    /// The product's `(ItemClass, ItemSubClass)` and `ItemSubClass.dbc` name, the header key
    /// (`0x55ba30`); `None` until the product's template arrives.
    pub group: Option<(u32, u32, String)>,
    /// What `DoTradeSkill` casts, one `CMSG_CAST_SPELL` per item: the packet has no count.
    pub spell_id: u32,
    pub name: String,
    pub difficulty: TradeSkillDifficulty,
    /// The least `have / need` over the reagents, reported as `numAvailable`.
    pub num_available: u32,
    /// The icon of `EffectItemType[0]`'s item (`0x4fdae0`), `None` on any miss.
    pub icon: Option<String>,
    /// That item, while its template is in flight: `GetTradeSkillIcon` waits on it with the
    /// callback `0x4fdc30` (`0x4fdb90`).
    pub icon_pending: Option<u32>,
    pub min_made: u32,
    pub max_made: u32,
    /// The recipe spell's cooldown as `(start_ms on the GetTime clock, duration_ms, enabled)`,
    /// `None` when ready: the getter derives the seconds left at its call (`0x4fda20`), so a
    /// running cooldown never changes the pushed state.
    pub cooldown: Option<(i64, u32, bool)>,
    /// The created item, 0 for a pure-effect recipe.
    pub product_item: u32,
    /// The product's `InventoryType`, which the inv-slot filter folds to slot bits through
    /// `DAT_00809200` (`0x4fcee6`); 0 takes the catch-all bit.
    pub product_inv_type: u32,
    /// The product's `ItemLevel` (`0x7c9640`), the sort key between tier and name, highest first
    /// (`row+0x14`, `0x4fd4e6`).
    pub product_item_level: u32,
    /// In display order.
    pub reagents: Vec<TradeSkillReagent>,
    /// The Requirements line's `(tool name, have)` pairs.
    pub tools: Vec<(String, bool)>,
    /// The totems whose templates are in flight: `GetTradeSkillTools` waits on each with the
    /// callback `0x4fdc30` (`0x4ffac6`).
    pub tools_pending: Vec<u32>,
}

/// One open tradeskill window, pushed whole by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillState {
    /// The `SkillLine.dbc` id, the persistence key (`0xbde064`).
    pub line: u32,
    pub line_name: String,
    /// `GetTradeSkillLine`'s rank and maximum: the slot's halves with the permanent bonus
    /// (`0x4fdd40`); the recipes are banded on the raw value.
    pub rank: u32,
    pub max_rank: u32,
    /// In the line's learn order (`0x5e9d72`), which the build refills its array in before it
    /// sorts (`0x4fca20`); Lua indexes the sorted array, not this.
    pub recipes: Vec<TradeSkillRecipe>,
}

impl super::UiScript {
    /// Pushes the open window's snapshot, or `None` on close; the app re-pushes on any change.
    ///
    /// Persistence follows the reference (`0x4fc910`, keyed by `0xbde064`): a different skill line
    /// resets filters, folds and selection, and the same line keeps them across a close. The
    /// selection rides its spell id (`0xbde044`); fold and subclass keys are pruned to live groups,
    /// while the inv-slot mask (`0x84dd64`) is never pruned.
    /// `None` marks the window closed and keeps the list and selection, as the close does
    /// (`0x4fc870`). After the world-enter init no push builds rows until a rebuild does.
    pub fn set_trade_skill(&mut self, state: Option<TradeSkillState>) {
        let mut model = self.model_mut();
        model.trade_skill_open = state.is_some();
        match state {
            None => {}
            Some(s) => {
                if model.trade_skill_last_line != s.line {
                    model.trade_skill_collapsed.clear();
                    model.trade_skill_subclass_hidden.clear();
                    model.trade_skill_invslot_mask = u32::MAX;
                    model.trade_skill_selected_spell = 0;
                    model.trade_skill_last_line = s.line;
                }
                let live: HashSet<(u32, u32)> = s
                    .recipes
                    .iter()
                    .map(|r| {
                        r.group
                            .as_ref()
                            .map_or((u32::MAX, u32::MAX), |(c, sc, _)| (*c, *sc))
                    })
                    .collect();
                model.trade_skill_collapsed.retain(|k| live.contains(k));
                model
                    .trade_skill_subclass_hidden
                    .retain(|k| live.contains(k));
                // A changed recipe set remakes the headers, as a learn's or the open's build does
                // (flag 1, `0x5e9d79`, `0x4fc985`).
                let spells = |recipes: &[TradeSkillRecipe]| {
                    recipes.iter().map(|r| r.spell_id).collect::<Vec<_>>()
                };
                let remake = model
                    .trade_skill
                    .as_ref()
                    .is_none_or(|t| spells(&t.recipes) != spells(&s.recipes));
                model.trade_skill = Some(s);
                if !model.trade_skill_unbuilt {
                    view::build(&mut model, remake);
                }
            }
        }
    }

    /// A rebuild with flag 0 (`0x4fca20(0)`), as the bag, item and skill watchers run one on each
    /// of their runs (`0x4f8e65`, `0x4f8f6d`, `0x4f917c`, `0x5d85be`, `0x5d8788`, `0x5d94d1`,
    /// `0x5dee70`, `0x5de434`, `0x5de457`), then `TRADE_SKILL_UPDATE` (`0x4fd164`) when it
    /// signals one ([`Self::rebuild_trade_skill_with`]).
    pub fn rebuild_trade_skill(&mut self, refresh: &mut dyn FnMut(&mut TradeSkillState)) {
        if self.rebuild_trade_skill_with(false, refresh) {
            self.fire_event("TRADE_SKILL_UPDATE", Vec::new());
        }
    }

    /// A rebuild with `remake` as its flag (`0x4fca20`): while the window is open, the state read
    /// again where `refresh` brings the caller's change in, the array refilled in add order and
    /// sorted, its products' records in flight counted pending (`0xbde048`) and waited on with
    /// the callback `0x4fc8e0` (`0x4fcd32`). Whether its `TRADE_SKILL_UPDATE` follows: an open
    /// window with nothing pending, whether or not anything changed (`0x4fcf34`, `0x4fd164`). A
    /// closed window builds nothing (`0x4fcaee`).
    pub fn rebuild_trade_skill_with(
        &mut self,
        remake: bool,
        refresh: &mut dyn FnMut(&mut TradeSkillState),
    ) -> bool {
        let mut model = self.model_mut();
        if !model.trade_skill_open {
            return false;
        }
        if let Some(t) = model.trade_skill.as_mut() {
            refresh(t);
        }
        model.trade_skill_unbuilt = false;
        let complete = view::build(&mut model, remake);
        note_build(&mut model);
        complete
    }

    /// The open's build (`0x4fc985` → `0x4fca20(1)`) over the list the push made: its products'
    /// records in flight counted and waited on as any build's. Whether its update follows.
    pub fn trade_skill_open_build(&mut self) -> bool {
        let mut model = self.model_mut();
        if std::mem::take(&mut model.trade_skill_unbuilt) {
            view::build(&mut model, true);
        }
        note_build(&mut model);
        model.trade_skill_pending == 0
    }

    /// The callback `0x4fc8e0` on a record a build waited on, answered: the pending count down by
    /// one when above 0 (`0x4fc8f3`), and whether a rebuild with flag 1 follows, at 0 (`0x4fc8f9`,
    /// `0x4fc900`).
    pub fn trade_skill_record_landed(&mut self) -> bool {
        let mut model = self.model_mut();
        if model.trade_skill_pending > 0 {
            model.trade_skill_pending -= 1;
        }
        model.trade_skill_pending == 0
    }

    /// The cancel `0x4fc890` on leaving the world (`0x490ae9` → `0x4fc780`): the pending count
    /// zeroed (`0x4fc8cb`); the callbacks stay, as the cancel's keys miss them.
    pub fn trade_skill_cancel_waits(&mut self) {
        self.model_mut().trade_skill_pending = 0;
    }

    /// The records the builds waited on since the last take, one per miss.
    pub fn take_trade_skill_build_waits(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().trade_skill_build_waits)
    }

    /// The item records the getters' calls missed that a harness's VM kept
    /// ([`Self::keep_calls`]), one per call.
    pub fn take_trade_skill_item_waits(&mut self) -> Vec<u32> {
        self.take_item_asks(super::ItemCallback::TradeSkill)
    }

    /// The rows the list holds, headers included: the full count (`0xbde04c`) that
    /// `TRADE_SKILL_SHOW` waits on at the open (`0x4fc98a`).
    pub fn trade_skill_full_count(&self) -> usize {
        self.model_ref().trade_skill_rows.len()
    }

    /// The `(spell id, count)` crafts `DoTradeSkill` handed a harness's VM ([`Self::keep_calls`]).
    pub fn take_trade_skill_dos(&mut self) -> Vec<(u32, u32)> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::TradeSkill(TradeSkillCall::Do { spell_id, count }) => {
                Some((*spell_id, *count))
            }
            _ => None,
        })
    }

    /// The `CloseTradeSkill` calls a harness's VM kept ([`Self::keep_calls`]).
    pub fn take_trade_skill_closes(&mut self) -> usize {
        self.take_calls_where(|call| match call {
            super::ScriptCall::TradeSkill(TradeSkillCall::Close) => Some(()),
            _ => None,
        })
        .len()
    }

    /// Whether the VM holds the window open: `CloseTradeSkill` closes it in the verb, so a feed
    /// that finds it closed owes no `TRADE_SKILL_CLOSE` of its own.
    pub fn trade_skill_open(&self) -> bool {
        self.model_ref().trade_skill_open
    }

    /// The post-cast hook `0x4fd7b0`, run for each of our own `SMSG_SPELL_GO`s whose first guid
    /// is the player and without flag `0x41` (`0x6e81d3`-`0x6e81de`): the armed spell counts down
    /// (`0x4fd7ca`) and, while any are left, recasts (`0x4fd7d7`); the last one ends silently with
    /// the spell still armed (`0x4fd7cf`). Any other cast, or nothing left, disarms (`0x4fd7e6` →
    /// `0x4fd790`). Returns the spell to recast: the app sends it, then signals with
    /// [`Self::trade_skill_recast_sent`], the reference's order (`0x4fd7dc` after the cast).
    pub fn trade_skill_spell_go(&mut self, spell_id: u32) -> Option<u32> {
        super::tick::verb(&self.lua, |model| {
            let armed = model.trade_skill_repeat_spell;
            if armed == 0 || model.trade_skill_repeat_count == 0 || armed != spell_id {
                disarm_repeat(model);
                return None;
            }
            model.trade_skill_repeat_count -= 1;
            (model.trade_skill_repeat_count != 0).then_some(armed)
        })
    }

    /// The hook's tail after its recast: `UPDATE_TRADESKILL_RECAST` (`0x4fd7dc`).
    pub fn trade_skill_recast_sent(&mut self) {
        super::tick::verb(&self.lua, |model| {
            model.raise("UPDATE_TRADESKILL_RECAST", Vec::new());
        });
    }

    /// A refused or failed cast of the armed spell disarms the repeat with its own signal
    /// (`0x6e1a3c`-`0x6e1a55` → `0x4fd790`), so a later close finds nothing to disarm.
    pub fn trade_skill_cast_failed(&mut self, spell_id: u32) {
        super::tick::verb(&self.lua, |model| {
            if spell_id == model.trade_skill_repeat_spell {
                disarm_repeat(model);
            }
        });
    }

    /// The cast abort with its disarm flag set (`0x6e4940`, `cl != 0`, `0x6e4abc`-`0x6e4ac2` →
    /// `0x4fd790`): whatever is armed disarms, with its signal.
    pub fn trade_skill_disarm(&mut self) {
        super::tick::verb(&self.lua, disarm_repeat);
    }

    /// The world-enter inits (`0x4908c0` at `0x4909bf` and `0x4909c4`), which a login, a
    /// cross-map worldport and a `/reload` all run, with no event: the tradeskill init `0x4fc750`
    /// zeroes the selection (`[0xbde044]`), the row, displayed and header counts (`[0xbde04c]`,
    /// `[0xbde054]`, `[0xbde050]`), the line key (`[0xbde064]`), so the next push resets the
    /// filters, and the armed spell (`[0xbde05c]`, `0x4fc76b`), keeping the count (`[0xbde060]`)
    /// and the open line (`[0xbde040]`), whose list waits for the next rebuild; the craft init
    /// `0x4f5ee0` zeroes the craft type (`[0xbdcfb8]`), which closes its window, its selection and
    /// its counts (`[0xbdcfbc]`-`[0xbdcfc8]`).
    pub fn reset_crafts_on_world_enter(&mut self) {
        let mut model = self.model_mut();
        model.trade_skill_rows.clear();
        model.trade_skill_headers.clear();
        model.trade_skill_visible = 0;
        model.trade_skill_unbuilt = true;
        model.trade_skill_selected_spell = 0;
        model.trade_skill_last_line = 0;
        model.trade_skill_repeat_spell = 0;
        model.craft = None;
        model.craft_open = false;
        model.craft_selection = 0;
        super::craft::build(&mut model);
    }

    /// Seat the repeat count a previous VM left (`[0xbde060]`), which the world-enter init keeps
    /// while it disarms the spell (`0x4fc76b`).
    pub fn seed_trade_skill_repeat_count(&mut self, count: u32) {
        self.model_mut().trade_skill_repeat_count = count;
    }

    /// The armed repeat as `(spell, count)`, the reference's `0xbde05c` and `0xbde060`.
    pub fn trade_skill_repeat(&self) -> (u32, u32) {
        let model = self.model_ref();
        (
            model.trade_skill_repeat_spell,
            model.trade_skill_repeat_count,
        )
    }
}

/// `0x4fd790`: an armed repeat disarms and signals `UPDATE_TRADESKILL_RECAST`; the count stays.
pub(crate) fn disarm_repeat(model: &mut super::Model) {
    if model.trade_skill_repeat_spell != 0 {
        model.trade_skill_repeat_spell = 0;
        model.raise("UPDATE_TRADESKILL_RECAST", Vec::new());
    }
}
