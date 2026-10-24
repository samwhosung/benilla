//! The craft bindings the stock `Blizzard_CraftUI` window calls, over the recipe snapshot the app
//! pushes ([`UiScript::set_craft`]); `DoCraft` and `CloseCraft` queue intents the app drains. The
//! window has one action button, `DoCraft(GetCraftSelectionIndex())` (`Blizzard_CraftUI.xml:667`),
//! and no Create All, so a craft intent is a bare spell id.
//!
//! The engine owns the row array (`0x4f60c0`): the recipes in the list's learn order, then a
//! header row per group once there are two, sorted by the craft type's comparator; every Lua
//! index is 1-based into that whole array, the displayed rows first ([`recipe`]).

use mlua::{Lua, MultiValue, Value};

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

/// A craft row's difficulty bucket (`[+0xc]`), the word `GetCraftInfo` pushes from the table at
/// `0x807e00`: 0 none, 1 optimal, 2 medium, 3 easy, 4 trivial, 5 used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CraftDifficulty {
    /// No ability row, or one with a trivial high of 0 (`0x4f61ea`): every Beast Training row.
    None,
    Optimal,
    Medium,
    Easy,
    Trivial,
    /// A bucket-0 row whose learn effect names a spell already known (`0x4f6360`, `0x4f63cc`).
    Used,
}

impl CraftDifficulty {
    /// The word at `0x807e00 + 32 * bucket` (`0x4f6fbc`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Optimal => "optimal",
            Self::Medium => "medium",
            Self::Easy => "easy",
            Self::Trivial => "trivial",
            Self::Used => "used",
        }
    }

    /// The bucket the comparators order by, ascending (`0x4f6a34`).
    pub(crate) fn bucket(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Optimal => 1,
            Self::Medium => 2,
            Self::Easy => 3,
            Self::Trivial => 4,
            Self::Used => 5,
        }
    }
}

impl From<super::TradeSkillDifficulty> for CraftDifficulty {
    fn from(d: super::TradeSkillDifficulty) -> Self {
        match d {
            super::TradeSkillDifficulty::Optimal => Self::Optimal,
            super::TradeSkillDifficulty::Medium => Self::Medium,
            super::TradeSkillDifficulty::Easy => Self::Easy,
            super::TradeSkillDifficulty::Trivial => Self::Trivial,
        }
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
    /// `GetCraftInfo`'s `craftType`, the row's bucket.
    pub difficulty: CraftDifficulty,
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
    /// `requiredLevel` return reads the learned spell's instead (`0x4f705f` → `0x60c840`).
    pub spell_level: u32,
    /// The recipe's group key (row `+0x04`): the skill line of the spell's ability (`0x4f61b1`),
    /// 0 with none (`0x4f61b6`).
    pub group: u32,
    /// The name of [`Self::group`], for `GetCraftSkillLine` and the header row; `None` when
    /// `SkillLine.dbc` has no such row.
    pub skill_line: Option<String>,
    /// `GetCraftInfo`'s sixth value, the training-point cost (`0x4f7078`).
    pub training_point_cost: u32,
    /// `GetCraftInfo`'s seventh value, the pet level the learned spell needs (`0x4f7088`).
    pub required_level: i32,
    /// The totems whose templates are in flight: `GetCraftSpellFocus` waits on each with the
    /// callback `0x4f7710` (`0x4f79f2`).
    pub tools_pending: Vec<u32>,
}

/// The open craft window, pushed whole by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct CraftState {
    /// The title `GetCraftName` answers.
    pub name: String,
    /// `GetCraftDisplaySkillLine`'s name, with [`Self::rank`] and [`Self::max_rank`] its rank
    /// and maximum, each with the permanent bonus; `None` answers nil, 0, 0.
    pub display_line: Option<String>,
    pub rank: u32,
    pub max_rank: u32,
    /// The opener spell's `EffectMiscValue[0]`, stored at `ds:0xbdcfb8`: 1 Beast Training, 2
    /// Disguise, 3 Enchanting. It picks the row comparator (`0x4f6765`) and the button token
    /// (`0x4f6c90`).
    pub craft_type: u32,
    /// In any order; [`UiScript::set_craft`] sorts them.
    pub recipes: Vec<CraftRecipe>,
}

/// Beast Training's craft type, the `EffectMiscValue[0]` of spell 5149.
const CRAFT_TYPE_BEAST_TRAINING: u32 = 1;
/// Enchanting's craft type, the one `GetCraftDisplaySkillLine` answers for (`0x4f6ce3`).
const CRAFT_TYPE_ENCHANTING: u32 = 3;

/// One row of the array (`0xbdcf78`): a header (spell id -1, `0x4f65c5`) keyed by its group, or
/// a recipe's position in [`CraftState::recipes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    Header { group: u32 },
    Recipe(usize),
}

/// One header of the array (`0xbdcf88`): its group key, the group's name, its member count
/// (`+4`) and whether it is collapsed (`+8`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Header {
    group: u32,
    name: Option<String>,
    members: u32,
    collapsed: bool,
}

/// `0x4f6ac0`: the same key ties; a key with no `SkillLine.dbc` row ties (`0x4f6b03`); else the
/// names, case-blind (`0x64a4c0`).
fn header_order(a: &Header, b: &Header) -> i32 {
    if a.group == b.group {
        return 0;
    }
    match (&a.name, &b.name) {
        (Some(x), Some(y)) => crate::crt::str_cmp_i(x, y),
        _ => 0,
    }
}

/// The row comparators, `0x4f6920` at Beast Training and `0x4f67a0` at every other type
/// (`0x4f6765`). With headers, each recipe takes its group's position in the header array and is
/// shown unless the group is collapsed; a header row skips the scan (`0x4f67fd`, `0x4f6818`;
/// `0x4f697d`, `0x4f6998`), so it keeps position 0 and shows. Shown rows come first, then by
/// position; then a header ahead of a recipe, and two headers answer 1 (`0x4f68e9`); then the
/// difficulty bucket ascending (`0x4f68ac`), the names case-blind (`0x64a4c0`) and, at Beast
/// Training, `spellLevel` ascending (`0x4f6a5b`).
///
/// Deviation: a last `spell_id` key breaks the remaining ties (Charge's six ranks all have
/// `spellLevel` 0), because the reference leaves them to its qsort over an array benilla does not
/// reproduce; ascending id is ascending rank in every 1.12 pet-ability family.
fn row_order(a: &Row, b: &Row, headers: &[Header], c: &CraftState) -> i32 {
    let group = |r: &Row| match *r {
        Row::Header { .. } => None,
        Row::Recipe(i) => Some(c.recipes[i].group),
    };
    let place = |g: Option<u32>| {
        g.and_then(|g| headers.iter().position(|h| h.group == g))
            .map_or((0, true), |p| (p, !headers[p].collapsed))
    };
    if !headers.is_empty() {
        let ((pa, va), (pb, vb)) = (place(group(a)), place(group(b)));
        match (va, vb) {
            (false, false) => return 0,
            (false, true) => return 1,
            (true, false) => return -1,
            _ if pa != pb => return if pb < pa { 1 } else { -1 },
            _ => {}
        }
    }
    let (ra, rb) = match (*a, *b) {
        (Row::Header { .. }, Row::Header { .. }) => return 1,
        (Row::Header { .. }, _) => return -1,
        (_, Row::Header { .. }) => return 1,
        (Row::Recipe(x), Row::Recipe(y)) => (&c.recipes[x], &c.recipes[y]),
    };
    let (ba, bb) = (ra.difficulty.bucket(), rb.difficulty.bucket());
    if ba != bb {
        return if ba > bb { 1 } else { -1 };
    }
    let by_name = crate::crt::str_cmp_i(&ra.name, &rb.name);
    if by_name != 0 {
        return by_name;
    }
    let by_level = if c.craft_type == CRAFT_TYPE_BEAST_TRAINING {
        ra.spell_level.cmp(&rb.spell_level)
    } else {
        std::cmp::Ordering::Equal
    };
    match by_level.then_with(|| ra.spell_id.cmp(&rb.spell_id)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// The build `0x4f60c0` past its open check: the rows refilled with the recipes in the list's
/// order, one header per group in the order the recipes first name it (`0x4f6404`-`0x4f64cb`),
/// a header row each (`0x4f65ba`-`0x4f65f9`), a single header dropped (`0x4f65fb`), then the
/// recount and sorts.
pub(crate) fn build(model: &mut Model) {
    let Some(c) = model.craft.as_ref() else {
        clear(model);
        return;
    };
    let mut headers: Vec<Header> = Vec::new();
    for r in &c.recipes {
        if headers.iter().all(|h| h.group != r.group) {
            headers.push(Header {
                group: r.group,
                name: r.skill_line.clone(),
                members: 0,
                collapsed: false,
            });
        }
    }
    if headers.len() == 1 {
        headers.clear();
    }
    let mut rows: Vec<Row> = (0..c.recipes.len()).map(Row::Recipe).collect();
    rows.extend(headers.iter().map(|h| Row::Header { group: h.group }));
    model.craft_rows = rows;
    model.craft_headers = headers;
    recount(model);
}

/// The counts a closed window's build zeroes ahead of its open check (`0x4f60d1`-`0x4f60e3`).
fn clear(model: &mut Model) {
    model.craft_rows.clear();
    model.craft_headers.clear();
    model.craft_visible = 0;
}

/// `0x4f6630`: each header's collapse word from its bit by its position as the array stands
/// (`0x4f6650`-`0x4f666d`) and its member count; the displayed count less each row whose group
/// is empty and each recipe of a collapsed group (`0x4f66e1`-`0x4f673a`, a group not found
/// reading header 0); then the CRT `qsort` of the headers (`0x4f6756`) and of the rows
/// (`0x4f6785`) as the last sort left them.
fn recount(model: &mut Model) {
    let Some(c) = model.craft.as_ref() else {
        clear(model);
        return;
    };
    let mask = model.craft_fold_mask;
    let mut headers = std::mem::take(&mut model.craft_headers);
    for (j, h) in headers.iter_mut().enumerate() {
        h.collapsed = mask & (1u32 << (j & 31)) == 0;
        h.members = 0;
    }
    let group = |r: &Row| match *r {
        Row::Header { group } => group,
        Row::Recipe(i) => c.recipes[i].group,
    };
    for r in &model.craft_rows {
        if let Row::Recipe(_) = r {
            for h in headers.iter_mut().filter(|h| h.group == group(r)) {
                h.members += 1;
            }
        }
    }
    let mut visible = model.craft_rows.len();
    if !headers.is_empty() {
        for r in &model.craft_rows {
            let h = headers
                .iter()
                .find(|h| h.group == group(r))
                .unwrap_or(&headers[0]);
            if h.members == 0 || (matches!(r, Row::Recipe(_)) && h.collapsed) {
                visible -= 1;
            }
        }
    }
    crate::crt::qsort(&mut headers, header_order);
    let mut rows = std::mem::take(&mut model.craft_rows);
    crate::crt::qsort(&mut rows, |a, b| row_order(a, b, &headers, c));
    model.craft_rows = rows;
    model.craft_headers = headers;
    model.craft_visible = visible;
}

/// The collapse writer `0x4f6be0`: the mask stored, the recount and sorts (`0x4f6630`), then
/// `CRAFT_UPDATE` in place (`0x4f6bf0`).
fn set_fold_mask(model: &mut Model, mask: u32) {
    model.craft_fold_mask = mask;
    recount(model);
    model.raise("CRAFT_UPDATE", Vec::new());
}

/// `0x4f6b30`: a header row's position in the header array, `None` for a recipe row or an index
/// past the rows.
fn header_position(model: &Model, row: usize) -> Option<usize> {
    match model.craft_rows.get(row)? {
        Row::Header { group } => model.craft_headers.iter().position(|h| h.group == *group),
        Row::Recipe(_) => None,
    }
}

impl CraftState {
    /// Each reagent's carried count from `count` (`0x622130`) and each row's available count
    /// (`+8`) from them.
    pub fn recount(&mut self, count: &dyn Fn(u32) -> u32) {
        for r in &mut self.recipes {
            for re in &mut r.reagents {
                re.have = count(re.item);
            }
            r.num_available =
                super::tradeskill::makeable(r.reagents.iter().map(|re| (re.have, re.need)));
        }
    }
}

/// A craft window verb the host applies inside its call ([`super::ScriptCall::Craft`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CraftCall {
    /// `DoCraft(index)` past its row and gate (`0x4f7d10`): the row's cast through
    /// `CastSpellAsActivePlayer` (`0x4f7d64` → `0x6e5a90`), with no count, as the 1.12 CraftFrame
    /// has no Create All.
    Do { spell_id: u32 },
    /// `CloseCraft()` (`0x4f6c00` → `0x4f5fe0`): the open marker `[0xbdcfb8]` zeroed.
    Close,
}

impl super::UiScript {
    /// Push the open craft window's snapshot, or mark it closed with `None`, which keeps the
    /// list and selection as the reference's close does (`0x4f5fe0`). A push builds the rows
    /// (`0x4f60c0`) from the recipes in the list's order; the selection rides its spell id.
    pub fn set_craft(&mut self, state: Option<CraftState>) {
        let mut model = self.model_mut();
        model.craft_open = state.is_some();
        if let Some(s) = state {
            model.craft = Some(s);
            build(&mut model);
        }
    }

    /// The open's resets (`0x4f601c`-`0x4f602c`): the selection cleared and every group expanded,
    /// then the build (`0x4f6036`).
    pub fn craft_opened(&mut self) {
        let mut model = self.model_mut();
        model.craft_selection = 0;
        model.craft_fold_mask = u32::MAX;
        build(&mut model);
    }

    /// A rebuild of the craft list (`0x4f60c0`), as the bag and item watchers run one on each of
    /// their runs: while a craft window is open, the state read again where `refresh` brings the
    /// watcher's change in, and the rows built again. Whether the window is open, for the caller's
    /// `CRAFT_UPDATE` (`0x4f661a`), signalled whether or not anything changed; a closed window
    /// zeroes the counts and builds nothing (`0x4f60d1`-`0x4f60e3`).
    pub fn rebuild_craft(&mut self, refresh: &mut dyn FnMut(&mut CraftState)) -> bool {
        let mut model = self.model_mut();
        if !model.craft_open {
            clear(&mut model);
            return false;
        }
        if let Some(c) = model.craft.as_mut() {
            refresh(c);
        }
        build(&mut model);
        true
    }

    /// The item records the getters' calls missed that a harness's VM kept
    /// ([`Self::keep_calls`]), one per call.
    pub fn take_craft_item_waits(&mut self) -> Vec<u32> {
        self.take_item_asks(super::ItemCallback::Craft)
    }

    /// The spell ids of the `DoCraft` calls a harness's VM kept ([`Self::keep_calls`]), one per
    /// call.
    pub fn take_craft_dos(&mut self) -> Vec<u32> {
        self.take_calls_where(|c| match c {
            super::ScriptCall::Craft(CraftCall::Do { spell_id }) => Some(*spell_id),
            _ => None,
        })
    }

    /// The `CloseCraft` calls a harness's VM kept ([`Self::keep_calls`]).
    pub fn take_craft_closes(&mut self) -> usize {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Craft(CraftCall::Close) => Some(()),
            _ => None,
        })
        .len()
    }

    /// Whether the VM holds the window open: `CloseCraft` closes it in the verb, so a feed that
    /// finds it closed owes no `CRAFT_CLOSE` of its own.
    pub fn craft_open(&self) -> bool {
        self.model_ref().craft_open
    }
}

/// The recipe at a 1-based index into the whole row array, bounded by the full count
/// (`0xbdcfc0`); `None` on a header, whose spell id -1 finds no record.
pub(crate) fn recipe(model: &Model, index: usize) -> Option<&CraftRecipe> {
    match model.craft_rows.get(index.checked_sub(1)?)? {
        Row::Recipe(i) => model.craft.as_ref()?.recipes.get(*i),
        Row::Header { .. } => None,
    }
}

/// A getter's item lookup that missed: the item cache asked inside the call, with the callback
/// `0x4f7710` (`0x4f75dd`, `0x4f79f2`), one per call, and the row's spell as the owner id.
fn ask_item(lua: &Lua, entry: u32, spell_id: u32) -> mlua::Result<()> {
    super::calls::ask(
        lua,
        super::RecordAsk::Item {
            entry,
            owner: super::calls::spell_owner(spell_id),
            callback: super::ItemCallback::Craft,
        },
    )
}

/// The row's group name (`+4`): a recipe's line, or its header's.
fn row_line(model: &Model, row: usize) -> Option<&String> {
    match model.craft_rows.get(row)? {
        Row::Recipe(i) => model.craft.as_ref()?.recipes.get(*i)?.skill_line.as_ref(),
        Row::Header { group } => model
            .craft_headers
            .iter()
            .find(|h| h.group == *group)?
            .name
            .as_ref(),
    }
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

    // GetCraftSkillLine(index) (`0x4f7210`): the name of the skill line in the row's group field
    // (`+0x04`), a header row's included, nil for a row outside the list (`dec`, then an unsigned
    // compare against the row count, `ds:0xbdcfc0`) or a line `SkillLine.dbc` lacks.
    g.set(
        "GetCraftSkillLine",
        lua.create_function(|lua, index: Value| {
            let row = binding_abi::number_arg(lua, index, "Usage: GetCraftSkillLine(index)")?
                .wrapping_sub(1) as u32;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            opt_str(lua, row_line(&model, row as usize))
        })?,
    )?;

    // GetCraftDisplaySkillLine() (`0x4f6cb0`) → name, rank, maxRank, only while the open craft
    // type is 3, Enchanting (`0x4f6ce3`); any other type, a closed window or a line the first
    // spell lacks answers nil, 0, 0 (`0x4f6e40`), and the nil hides the stock rank bar
    // (`Blizzard_CraftUI.lua:108-122`).
    g.set(
        "GetCraftDisplaySkillLine",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let enchanting = model.open_professions().craft == Some(CRAFT_TYPE_ENCHANTING);
            let (name, rank, max_rank) = match &model.craft {
                Some(c) if enchanting && c.display_line.is_some() => {
                    (c.display_line.clone(), c.rank, c.max_rank)
                }
                _ => (None, 0, 0),
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(lua, name.as_ref())?,
                Value::Integer(i64::from(rank)),
                Value::Integer(i64::from(max_rank)),
            ]))
        })?,
    )?;

    // GetNumCrafts() (`0x4f6e70`): the displayed count (`0xbdcfc8`), headers included.
    g.set(
        "GetNumCrafts",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.craft_visible as i64)
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
    // trainingPointCost, requiredLevel (`Blizzard_CraftUI.lua:168`); a miss pushes the seven too.
    // A header row answers its line's name, nil, `header`, 0, 1 while its mask bit by its place
    // in the sorted header array is set or nil while clear (`0x4f6f4f` → `0x4f6b80`, `0x4f6bc1`),
    // not the collapse word the recount stored before the sort, then 0, 0; a recipe's
    // `isExpanded` is nil. The cost and the level are the app's, read off the pet bar's unit
    // (`0xb714a0`) for the spell a pet-learn effect teaches (`0x4f6fe6`-`0x4f7093`).
    g.set(
        "GetCraftInfo",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetCraftInfo(index)` (`0x84d398`, `0x4f6ea9`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetCraftInfo(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let header = index.checked_sub(1).and_then(|row| {
                header_position(&model, row).and_then(|p| {
                    let collapsed = model.craft_fold_mask & (1u32 << (p & 31)) == 0;
                    model.craft_headers[p].name.clone().map(|n| (n, collapsed))
                })
            });
            if let Some((name, collapsed)) = header {
                return Ok(MultiValue::from_vec(vec![
                    Value::String(lua.create_string(&name)?),
                    Value::Nil,
                    Value::String(lua.create_string("header")?),
                    Value::Integer(0),
                    if collapsed {
                        Value::Nil
                    } else {
                        Value::Integer(1)
                    },
                    Value::Integer(0),
                    Value::Integer(0),
                ]));
            }
            let Some(r) = recipe(&model, index) else {
                // `0x4f70a4`: nil, nil, nil, 0, nil, 0, 0, a header with no `SkillLine.dbc` row
                // included (`0x4f6f1a`).
                return Ok(binding_abi::tuple(&[
                    None,
                    None,
                    None,
                    Some(0),
                    None,
                    Some(0),
                    Some(0),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&r.name)?),
                Value::String(lua.create_string(&r.sub_name)?),
                Value::String(lua.create_string(r.difficulty.as_str())?),
                Value::Integer(i64::from(r.num_available)),
                Value::Nil,
                Value::Integer(i64::from(r.training_point_cost)),
                Value::Integer(i64::from(r.required_level)),
            ]))
        })?,
    )?;

    // GetCraftIcon(index) → the icon path, nil while in flight or past the end.
    g.set(
        "GetCraftIcon",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTradeSkillIcon(index)` (`0x84d3d0`, `0x4f7171`), the
            // client's own text naming `GetTradeSkillIcon`.
            let index = binding_abi::index_arg(lua, index, "Usage: GetTradeSkillIcon(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            opt_str(lua, recipe(&model, index).and_then(|r| r.icon.as_ref()))
        })?,
    )?;

    // GetCraftDescription(index) → the description or nil, which the stock window tests before
    // painting (`Blizzard_CraftUI.lua:312`).
    g.set(
        "GetCraftDescription",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetCraftDescription(index)` (`0x84d500`, `0x4f7aac`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetCraftDescription(index)")?;
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
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetCraftNumReagents(index)` (`0x84d454`, `0x4f7435`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetCraftNumReagents(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(recipe(&model, index).map_or(0, |r| r.reagents.len()) as i64)
        })?,
    )?;

    // GetCraftReagentInfo(index, reagentIndex) → name, icon, need, have; a miss is four nils
    // (`0x4f759b`).
    g.set(
        "GetCraftReagentInfo",
        lua.create_function(|lua, (index, reagent_index): (Value, Value)| {
            // Both `is-number` else `Usage: GetCraftReagentInfo(index, reagentIndex)` (`0x84d478`,
            // `0x4f76ef`).
            let index = binding_abi::index_arg(
                lua,
                index,
                "Usage: GetCraftReagentInfo(index, reagentIndex)",
            )?;
            let reagent_index = binding_abi::index_arg(
                lua,
                reagent_index,
                "Usage: GetCraftReagentInfo(index, reagentIndex)",
            )?;
            let reagent = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                recipe(&model, index).and_then(|r| {
                    let n = reagent_index.checked_sub(1)?;
                    Some((r.reagents.get(n)?.clone(), r.spell_id))
                })
            };
            let Some((reagent, spell_id)) = reagent else {
                return Ok(binding_abi::tuple(&[None; 4]));
            };
            if reagent.name.is_none() {
                // The miss asks with the callback `0x4f7710` inside the call (`0x4f75dd`).
                ask_item(lua, reagent.item, spell_id)?;
            }
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
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetCraftSpellFocus(index)` (`0x84d4dc`, `0x4f78db`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetCraftSpellFocus(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(r) = recipe(&model, index) else {
                return Ok(MultiValue::new());
            };
            let (tools, pending, spell_id) = (r.tools.clone(), r.tools_pending.clone(), r.spell_id);
            drop(model);
            // Each totem in flight is asked with the callback `0x4f7710` inside the call
            // (`0x4f79f2`).
            for entry in pending {
                ask_item(lua, entry, spell_id)?;
            }
            let mut out = Vec::new();
            for (name, have) in &tools {
                out.push(Value::String(lua.create_string(name)?));
                out.push(binding_abi::flag(*have));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetCraftButtonToken() (`0x4f6c90`): the word at the open craft type (`0xbdcfb8`) in the
    // table at `0x84d0f4`, the global the stock window labels its button with
    // (`Blizzard_CraftUI.lua:106`). A close zeroes the type, so a closed window answers `USE`.
    // Type 4 reads the zero word after the table, which pushes nil.
    // Deviation: the reference reads past the table unbounded (`0x4f6c90`), so type 5 would answer
    // "CloseCraft"; no shipped opener carries a type past 3, and benilla answers nil rather than
    // read past it.
    g.set(
        "GetCraftButtonToken",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let token = match model.open_professions().craft.unwrap_or(0) {
                0 => "USE",
                1 => "TRAIN",
                2 => "DISGUISE",
                3 => "ENSCRIBE",
                _ => return Ok(Value::Nil),
            };
            Ok(Value::String(lua.create_string(token)?))
        })?,
    )?;

    // SelectCraft(index) (`0x4f70f0` → `0x4f6050`) stores the row's spell id, 0 for a header or an
    // index past the rows; GetCraftSelectionIndex() (`0x4f7130` → `0x4f6080`) answers the
    // position of that spell's row plus 1, 0 for none or a spell no row holds.
    g.set(
        "SelectCraft",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: SelectCraft(index)` (`0x84d3b4`, `0x4f7101`).
            let index = binding_abi::index_arg32(lua, index, "Usage: SelectCraft(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.craft_selection = recipe(&model, index as usize).map_or(0, |r| r.spell_id);
            Ok(())
        })?,
    )?;
    g.set(
        "GetCraftSelectionIndex",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let sel = model.craft_selection;
            let at = (sel != 0)
                .then(|| {
                    (1..=model.craft_rows.len())
                        .find(|&i| recipe(&model, i).is_some_and(|r| r.spell_id == sel))
                })
                .flatten();
            Ok(at.unwrap_or(0) as i64)
        })?,
    )?;

    // DoCraft(index): queue the row's spell id; out of range is ignored.
    g.set(
        "DoCraft",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: DoCraft(index)` (`0x84d5c4`, `0x4f7d21`).
            let index = binding_abi::index_arg(lua, index, "Usage: DoCraft(index)")?;
            // The recipe's cast is `TryCastSpell` (`0x4f7d64` → `0x6e5a90`), behind the gate's
            // tag 2 (`0x6e4bc1`).
            let may_cast = super::protected::gate(lua, super::protected::Tag::CAST);
            let spell_id = recipe(&lua.app_data_ref::<Model>().expect("model app_data"), index)
                .map(|r| r.spell_id)
                .filter(|_| may_cast);
            match spell_id {
                Some(spell_id) => {
                    super::calls::submit(lua, super::ScriptCall::Craft(CraftCall::Do { spell_id }))
                }
                None => Ok(()),
            }
        })?,
    )?;

    // CloseCraft() (`0x4f6c00` → `0x4f5fe0`): no packet. It zeroes the open marker alone
    // (`[0xbdcfb8]`), the engine's inside the call, so `GetNumCrafts` (`0x4f6e70`) still answers
    // the held list, and signals `CRAFT_CLOSE` unconditionally (`0x4f5fef`).
    g.set(
        "CloseCraft",
        lua.create_function(|lua, ()| {
            super::calls::submit(lua, super::ScriptCall::Craft(CraftCall::Close))?;
            super::tick::verb(lua, |model| {
                model.craft_open = false;
                model.raise("CRAFT_CLOSE", Vec::new());
            });
            Ok(())
        })?,
    )?;

    // Expand/CollapseCraftSkillLine(index) (`0x4f7c90`, `0x4f7c10`): an index of 0 or below
    // expands or collapses every group (`0x4f7cc7`, `0x4f7c47`); else the row must be a header
    // (`0x4f6b30`), whose bit by its array position is set or cleared (`0x4f7cf2`, `0x4f7c71`),
    // and any other row raises. Each write re-sorts and signals `CRAFT_UPDATE` (`0x4f6be0`).
    g.set(
        "ExpandCraftSkillLine",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: ExpandCraftSkillLine(index)` (`0x84d5a0`, `0x4f7ca1`).
            let index = number_arg(lua, index, "Usage: ExpandCraftSkillLine(index)")?;
            fold(lua, index, false, "Bad skill line in ExpandCraftSkillLine")
        })?,
    )?;
    g.set(
        "CollapseCraftSkillLine",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: CollapseCraftSkillLine(index)` (`0x84d550`, `0x4f7c21`).
            let index = number_arg(lua, index, "Usage: CollapseCraftSkillLine(index)")?;
            fold(lua, index, true, "Bad skill line in CollapseCraftSkillLine")
        })?,
    )?;

    Ok(())
}

/// A fold verb past its usage check: every group for an index of 0 or below, else the header
/// row's bit, or the verb's `bad` line with nothing touched (`0x4f7c5b`, `0x4f7cdc`).
fn fold(lua: &Lua, index: i32, collapse: bool, bad: &'static str) -> mlua::Result<()> {
    super::tick::verb(lua, |model| {
        let mask = match index.checked_sub(1).filter(|&row| row >= 0) {
            None if collapse => 0,
            None => u32::MAX,
            Some(row) => {
                let bit = 1u32 << (header_position(model, row as usize)? & 31);
                if collapse {
                    model.craft_fold_mask & !bit
                } else {
                    model.craft_fold_mask | bit
                }
            }
        };
        set_fold_mask(model, mask);
        Some(())
    })
    .ok_or_else(|| mlua::Error::RuntimeError(bad.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn recipe(
        spell_id: u32,
        name: &str,
        difficulty: CraftDifficulty,
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
            group: 0,
            skill_line: None,
            training_point_cost: 0,
            required_level: 0,
            tools_pending: Vec::new(),
        }
    }

    /// Two Enchanting recipes, pushed trivial first; the sort puts Minor Health at index 1.
    fn state() -> CraftState {
        CraftState {
            name: "Enchanting".into(),
            display_line: Some("Enchanting".into()),
            rank: 57,
            max_rank: 75,
            craft_type: CRAFT_TYPE_ENCHANTING,
            recipes: vec![
                recipe(
                    7418,
                    "Enchant Weapon - Minor Beastslaying",
                    CraftDifficulty::Trivial,
                    5,
                ),
                recipe(
                    683,
                    "Enchant Weapon - Minor Health",
                    CraftDifficulty::Optimal,
                    0,
                ),
            ],
        }
    }

    /// `GetCraftSkillLine` (`0x4f7210`): the usage text for a non-number, nil for a row outside
    /// the list or a recipe whose line has no `SkillLine.dbc` row, else the line's name; rows
    /// count in the sorted display order.
    #[test]
    fn craft_skill_line_answers_the_rows_group_name() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        for call in [
            "GetCraftSkillLine()",
            "GetCraftSkillLine(nil)",
            "GetCraftSkillLine('x')",
            "GetCraftSkillLine({})",
        ] {
            let err = s.run(call).expect_err(call).to_string();
            assert!(
                err.contains("Usage: GetCraftSkillLine(index)"),
                "{call}: {err}"
            );
        }
        // No window: nil.
        assert!(s
            .eval::<bool>("return GetCraftSkillLine(1) == nil")
            .unwrap());
        let mut st = state();
        for r in &mut st.recipes {
            // The second recipe's line has no row.
            r.skill_line = (r.spell_id == 683).then(|| "Enchanting".to_string());
        }
        s.set_craft(Some(st));
        let by_row = |s: &UiScript, i: i64| -> (String, Option<String>) {
            s.eval(&format!("return GetCraftInfo({i}), GetCraftSkillLine({i})"))
                .unwrap()
        };
        // Row 1 is the Optimal recipe (683); row 2 the Trivial one, with no line row.
        let (name, line) = by_row(&s, 1);
        assert!(name.contains("Minor Health"), "{name}");
        assert_eq!(line.as_deref(), Some("Enchanting"));
        assert_eq!(by_row(&s, 2).1, None);
        for call in [
            "GetCraftSkillLine(0)",
            "GetCraftSkillLine(-1)",
            "GetCraftSkillLine(3)",
        ] {
            assert!(
                s.eval::<bool>(&format!("return {call} == nil")).unwrap(),
                "{call}"
            );
        }
        assert_eq!(
            s.eval::<String>("return GetCraftSkillLine('1')").unwrap(),
            "Enchanting"
        );
    }

    /// `GetCraftDisplaySkillLine` (`0x4f6cb0`) answers only while the open craft type is 3
    /// (`cmp ds:0xbdcfb8,0x3` at `0x4f6ce3`); any other type, a closed window (`0x4f5fe0` zeroes
    /// the type and keeps the list) or a missing line answers nil, 0, 0 (`0x4f6e40`).
    #[test]
    fn the_display_skill_line_answers_only_for_enchanting() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let triple = |s: &UiScript| {
            s.eval::<(Option<String>, i64, i64)>("return GetCraftDisplaySkillLine()")
                .unwrap()
        };
        s.set_craft(Some(state()));
        assert_eq!(triple(&s), (Some("Enchanting".into()), 57, 75));
        let mut beasts = state();
        beasts.craft_type = CRAFT_TYPE_BEAST_TRAINING;
        s.set_craft(Some(beasts));
        assert_eq!(triple(&s), (None, 0, 0), "Beast Training");
        assert_eq!(s.arity("GetCraftDisplaySkillLine()").unwrap(), 3);
        s.set_craft(Some(state()));
        s.run("CloseCraft()").unwrap();
        assert_eq!(
            triple(&s),
            (None, 0, 0),
            "closed in the verb, the list held"
        );
        let mut no_line = state();
        no_line.display_line = None;
        s.set_craft(Some(no_line));
        assert_eq!(triple(&s), (None, 0, 0), "no line for the first spell");
    }

    /// A recipe row's sixth and seventh values are its training-point cost and the pet level it
    /// needs (`0x4f7078`, `0x4f7088`).
    #[test]
    fn craft_info_answers_the_training_point_cost_and_the_required_level() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let mut st = state();
        for r in &mut st.recipes {
            r.training_point_cost = 5;
            r.required_level = 12;
        }
        s.set_craft(Some(st));
        assert_eq!(
            s.eval::<(i64, i64)>("local _,_,_,_,_,tp,l = GetCraftInfo(1) return tp, l")
                .unwrap(),
            (5, 12)
        );
    }

    #[test]
    fn snapshot_feeds_the_api_tuples() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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

        assert!(s
            .eval::<bool>("local _,_,t = GetCraftInfo(1) return t ~= 'header'")
            .unwrap());
        // One group builds no header (`0x4f65fb`), so no row folds.
        assert!(s.run("CollapseCraftSkillLine(1)").is_err());
        assert_eq!(s.eval::<i64>("return GetNumCrafts()").unwrap(), 2);
    }

    /// Made-up two-group data for the header mechanism, not a real list: no shipped craft list
    /// holds two groups (every learnable Beast Training spell has a line-261 row, so the real one
    /// shows a single group and no header). Three rows in line 261 and one in group 0, whose line
    /// has no `SkillLine.dbc` row; the names and ids are labels only.
    fn two_groups() -> CraftState {
        let row = |spell_id: u32, name: &str, group: u32| {
            let mut r = recipe(spell_id, name, CraftDifficulty::None, 0);
            r.group = group;
            r.skill_line = (group != 0).then(|| "Beast Training".to_string());
            r
        };
        CraftState {
            name: "Beast Training".into(),
            display_line: None,
            rank: 0,
            max_rank: 0,
            craft_type: CRAFT_TYPE_BEAST_TRAINING,
            recipes: vec![
                row(2649, "Growl", 261),
                row(24604, "Furious Howl", 0),
                row(1742, "Cower", 261),
                row(4187, "Great Stamina", 261),
            ],
        }
    }

    /// `isExpanded` reads the mask bit at the header's place in the array after the sort
    /// (`0x4f6f4f` → `0x4f6b80`, `0x4f6bc1`), where the displayed rows follow the collapse words
    /// the recount set before it (`0x4f6650` ahead of `0x4f6756`). A rebuild remakes the headers
    /// in the order the recipes name them (`0x4f6404`), so one whose sort moves a header reads
    /// the folded bit on the other group: the rows hide one group, the flag names the other.
    #[test]
    fn a_headers_expanded_flag_reads_its_bit_after_the_sort() {
        let row = |spell_id: u32, name: &str, group: u32, line: &str| {
            let mut r = recipe(spell_id, name, CraftDifficulty::None, 0);
            r.group = group;
            r.skill_line = Some(line.to_string());
            r
        };
        // Made-up data: Zeta named first, Alpha sorting first.
        let st = CraftState {
            name: "Test".into(),
            display_line: None,
            rank: 0,
            max_rank: 0,
            craft_type: CRAFT_TYPE_BEAST_TRAINING,
            recipes: vec![
                row(1, "Zeta One", 270, "Zeta"),
                row(2, "Alpha One", 261, "Alpha"),
                row(3, "Alpha Two", 261, "Alpha"),
            ],
        };
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_craft(Some(st.clone()));
        let header = |s: &UiScript, line: &str| {
            (1..=2)
                .find(|i| {
                    s.eval::<bool>(&format!("return GetCraftInfo({i}) == '{line}'"))
                        .unwrap()
                })
                .unwrap()
        };
        let expanded = |s: &UiScript, line: &str| {
            let at = header(s, line);
            s.eval::<Option<i64>>(&format!("local _,_,_,_,e = GetCraftInfo({at}) return e"))
                .unwrap()
        };
        s.run(&format!("CollapseCraftSkillLine({})", header(&s, "Alpha")))
            .unwrap();
        assert_eq!(s.eval::<i64>("return GetNumCrafts()").unwrap(), 3);
        assert_eq!(
            (expanded(&s, "Alpha"), expanded(&s, "Zeta")),
            (None, Some(1))
        );
        s.set_craft(Some(st));
        assert_eq!(
            s.eval::<i64>("return GetNumCrafts()").unwrap(),
            4,
            "Zeta's word took bit 0 before the sort"
        );
        assert_eq!(
            (expanded(&s, "Alpha"), expanded(&s, "Zeta")),
            (None, Some(1)),
            "Alpha holds position 0 after it"
        );
    }

    /// The rows the stock window walks, `(name, type, isExpanded)` for each displayed index.
    fn shown(s: &UiScript) -> Vec<(Option<String>, Option<String>, Option<i64>)> {
        let n = s.eval::<i64>("return GetNumCrafts()").unwrap();
        (1..=n)
            .map(|i| {
                s.eval(&format!("local n,_,t,_,e = GetCraftInfo({i}) return n,t,e"))
                    .unwrap()
            })
            .collect()
    }

    /// Two groups build a header row each (`0x4f60c0`, spell id -1). A header skips the
    /// comparator's group scan (`0x4f697d`, `0x4f6998`), so it keeps position 0 and sorts ahead of
    /// every recipe; the recipes follow by their group's place in the name-sorted headers, then
    /// name. A header answers `header` and its expanded flag (`0x4f6f38`, `0x4f6f4f`); one whose
    /// line has no `SkillLine.dbc` row answers the failure tuple (`0x4f6f1a`), and a header holds
    /// no recipe for the row verbs.
    #[test]
    fn two_groups_head_each_group_ahead_of_the_recipes() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_craft(Some(two_groups()));
        let header = |name: Option<&str>| {
            (
                name.map(str::to_string),
                name.map(|_| "header".to_string()),
                name.map(|_| 1),
            )
        };
        let rec = |name: &str| (Some(name.to_string()), Some("none".to_string()), None);
        // The CRT's `shortsort` over the build's order (`0x73f772`): the tied headers swap, so the
        // nameless line 0 heads the array and its recipe leads the shown ones.
        assert_eq!(
            shown(&s),
            [
                header(None),
                header(Some("Beast Training")),
                rec("Furious Howl"),
                rec("Cower"),
                rec("Great Stamina"),
                rec("Growl"),
            ]
        );
        let at = (1..=2)
            .find(|i| {
                s.eval::<bool>(&format!("return GetCraftInfo({i}) == 'Beast Training'"))
                    .unwrap()
            })
            .unwrap();
        assert_eq!(
            s.eval::<String>(&format!("return GetCraftSkillLine({at})"))
                .unwrap(),
            "Beast Training"
        );
        assert!(s
            .eval::<bool>(&format!("return GetCraftIcon({at}) == nil"))
            .unwrap());
        s.run(&format!("SelectCraft({at}) DoCraft({at})")).unwrap();
        assert_eq!(s.eval::<i64>("return GetCraftSelectionIndex()").unwrap(), 0);
        assert!(s.take_craft_dos().is_empty());
        assert_eq!(s.arity(&format!("GetCraftInfo({at})")).unwrap(), 7);
    }

    /// `CollapseCraftSkillLine` clears its header's bit and `ExpandCraftSkillLine` sets it
    /// (`0x4f7c71`, `0x4f7cf2`); each re-sorts and signals `CRAFT_UPDATE` in place (`0x4f6be0`). A
    /// collapsed group's recipes leave the displayed count and sort past it (`0x4f673a`), its
    /// header shows with a nil `isExpanded`; an index of 0 or below folds every group
    /// (`0x4f7c47`, `0x4f7cc7`); a recipe row raises (`0x4f7c5d`). The open expands every group
    /// (`0x4f602c`).
    #[test]
    fn the_fold_verbs_fold_a_header_and_every_group_at_zero() {
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        // Two named lines, so the header sort never ties (`0x4f6b1e`): Beast Training heads row 1.
        let mut st = two_groups();
        st.recipes[1].group = 270;
        st.recipes[1].skill_line = Some("Pet - Wolf".into());
        s.set_craft(Some(st));
        heard::listen(&s, &["CRAFT_UPDATE"], Some("GetNumCrafts()"));
        let at = |s: &UiScript| {
            (1..=2)
                .find(|i| {
                    s.eval::<bool>(&format!("return GetCraftInfo({i}) == 'Beast Training'"))
                        .unwrap()
                })
                .unwrap()
        };
        assert_eq!(at(&s), 1);
        s.run(&format!("CollapseCraftSkillLine({})", at(&s)))
            .unwrap();
        assert_eq!(heard::take(&s), ["CRAFT_UPDATE 3"]);
        let rows = shown(&s);
        assert!(
            rows.contains(&(Some("Beast Training".into()), Some("header".into()), None)),
            "{rows:?}"
        );
        assert_eq!(
            rows[2],
            (Some("Furious Howl".into()), Some("none".into()), None)
        );
        let err = s.run("CollapseCraftSkillLine(3)").unwrap_err().to_string();
        assert!(
            err.contains("Bad skill line in CollapseCraftSkillLine"),
            "{err}"
        );
        assert!(heard::take(&s).is_empty());
        s.run(&format!("ExpandCraftSkillLine({})", at(&s))).unwrap();
        assert_eq!(heard::take(&s), ["CRAFT_UPDATE 6"]);
        s.run("CollapseCraftSkillLine(0)").unwrap();
        assert_eq!(heard::take(&s), ["CRAFT_UPDATE 2"]);
        s.craft_opened();
        assert_eq!(s.eval::<i64>("return GetNumCrafts()").unwrap(), 6);
        s.run("CollapseCraftSkillLine(-1) ExpandCraftSkillLine(0)")
            .unwrap();
        assert_eq!(heard::take(&s), ["CRAFT_UPDATE 2", "CRAFT_UPDATE 6"]);
        let err = s.run("ExpandCraftSkillLine('x')").unwrap_err().to_string();
        assert!(err.contains("Usage: ExpandCraftSkillLine(index)"), "{err}");
    }

    /// `GetCraftButtonToken` (`0x4f6c90`) pushes the word at the open craft type (`0xbdcfb8`) in
    /// the table at `0x84d0f4`: `USE`, `TRAIN`, `DISGUISE`, `ENSCRIBE`, then a zero word, which
    /// pushes nil. A close zeroes the type (`0x4f5fe0`), so a closed window answers `USE`.
    #[test]
    fn the_button_token_is_the_open_craft_types_word() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let token = |s: &UiScript| {
            s.eval::<Option<String>>("return GetCraftButtonToken()")
                .unwrap()
        };
        assert_eq!(token(&s).as_deref(), Some("USE"), "no window");
        for (craft_type, word) in [
            (1, Some("TRAIN")),
            (2, Some("DISGUISE")),
            (3, Some("ENSCRIBE")),
            (4, None),
        ] {
            let mut st = state();
            st.craft_type = craft_type;
            s.set_craft(Some(st));
            assert_eq!(token(&s).as_deref(), word, "type {craft_type}");
            assert_eq!(s.arity("GetCraftButtonToken()").unwrap(), 1);
        }
        s.set_craft(Some(state()));
        s.run("CloseCraft()").unwrap();
        assert_eq!(token(&s).as_deref(), Some("USE"), "closed in the verb");
        s.set_craft(Some(state()));
        s.set_craft(None);
        assert_eq!(token(&s).as_deref(), Some("USE"), "closed by the app");
    }

    #[test]
    fn selection_persists_across_a_repush_by_spell_id() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        // Minor Beastslaying (spell 7418) sorts to index 2 (trivial, behind the optimal row).
        s.set_craft(Some(state()));
        s.run("SelectCraft(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetCraftSelectionIndex()").unwrap(), 2);

        // Turning 7418 optimal moves it to index 1, and the selection follows the spell id.
        let mut promoted = state();
        promoted.recipes[0].difficulty = CraftDifficulty::Optimal;
        promoted.recipes[0].name = "Aaa Beastslaying".into();
        s.set_craft(Some(promoted));
        assert_eq!(s.eval::<i64>("return GetCraftSelectionIndex()").unwrap(), 1);

        let mut without_7418 = state();
        without_7418.recipes.remove(0);
        s.set_craft(Some(without_7418));
        assert_eq!(s.eval::<i64>("return GetCraftSelectionIndex()").unwrap(), 0);
    }

    /// The recipe's cast is `TryCastSpell`'s, behind the gate's tag 2 (`0x4f7d64` → `0x6e4bc1`).
    #[test]
    fn do_craft_casts_only_inside_an_input_event() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_craft(Some(state()));

        s.run(r#"RunScript("DoCraft(1)")"#).unwrap();
        assert!(
            s.take_craft_dos().is_empty(),
            "a macro outside an input event"
        );
        s.run_binding(r#"RunScript("DoCraft(1)")"#).unwrap();
        assert_eq!(s.take_craft_dos(), vec![683], "inside a key's body");
    }

    #[test]
    fn do_craft_drains_bare_spell_ids_no_count() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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
        s.keep_calls();
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
        s.keep_calls();
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

    /// `0x4f5fe0` zeroes only the open marker (`[0xbdcfb8]`) and signals `CRAFT_CLOSE` before
    /// the verb returns; `GetNumCrafts` (`0x4f6e70`, `[0xbdcfc8]`) and the selection still answer
    /// the held list, through the app's close too.
    #[test]
    fn a_close_keeps_the_list() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_craft(Some(state()));
        s.run("SelectCraft(1)").unwrap();
        let held = s.eval::<i64>("return GetNumCrafts()").unwrap();
        assert!(held > 0);

        crate::script::tick::heard::listen(&s, &["CRAFT_CLOSE"], Some("GetNumCrafts()"));
        s.run("CloseCraft()").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            [format!("CRAFT_CLOSE {held}")]
        );
        assert!(!s.craft_open());
        assert_eq!(s.take_craft_closes(), 1);

        s.set_craft(None);
        assert_eq!(s.eval::<i64>("return GetNumCrafts()").unwrap(), held);
        assert_eq!(s.eval::<i64>("return GetCraftSelectionIndex()").unwrap(), 1);
    }

    /// The expected order is an emulated run of `0x4f6920` over the shipped `Spell.dbc` values.
    #[test]
    fn beast_training_orders_ranks_by_spell_level() {
        let rank = |spell_id: u32, name: &str, level: u32| CraftRecipe {
            spell_id,
            tooltip: CraftTooltip::Spell(spell_id),
            name: name.into(),
            sub_name: format!("Rank {}", level / 10 - 1),
            difficulty: CraftDifficulty::Trivial,
            num_available: 0,
            icon: None,
            description: None,
            needs_item_target: false,
            reagents: vec![],
            tools: vec![],
            spell_level: level,
            group: 0,
            skill_line: None,
            training_point_cost: 0,
            required_level: 0,
            tools_pending: Vec::new(),
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
        s.keep_calls();
        s.set_craft(Some(CraftState {
            name: "Beast Training".into(),
            display_line: None,
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
            display_line: None,
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
        s.keep_calls();
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

    /// `GetCraftReagentInfo` waits on a reagent whose record is in flight (`0x4f75dd`) and
    /// `GetCraftSpellFocus` on each such totem (`0x4f79f2`), one wait per call, each with the
    /// callback `0x4f7710` that rebuilds the list when the record lands.
    #[test]
    fn a_getter_that_misses_a_record_waits_on_it_once_per_call() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let mut st = state();
        for r in &mut st.recipes {
            r.reagents[0].name = None;
            r.tools_pending = vec![5060];
        }
        s.set_craft(Some(st));
        s.run("GetCraftReagentInfo(1, 1) GetCraftReagentInfo(1, 1) GetCraftSpellFocus(1)")
            .unwrap();
        assert_eq!(s.take_craft_item_waits(), [10940, 10940, 5060]);
        s.run("GetCraftReagentInfo(1, 2) GetCraftReagentInfo(9, 1)")
            .unwrap();
        assert!(
            s.take_craft_item_waits().is_empty(),
            "a miss of the row reads no record"
        );
    }
}
