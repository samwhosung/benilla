//! The tradeskill list the API indexes (`0x4fca20`, `0x4fd180`): the recipes and one header row
//! per group in one array, which the CRT sort (`0x73f727`) orders with the displayed rows first,
//! so a row a fold or a filter hides still answers at its index past the displayed count.

use std::collections::HashMap;

use crate::script::Model;

use super::TradeSkillRecipe;

/// A product's `InventoryType` as inv-slot filter bits (`record+0x10`): the client's 29-entry
/// table at `0x809200` after its overrides, so finger, trinket and bag take one bit each, a
/// one-hand weapon sets both hand bits, and non-equipment takes the catch-all bit 23.
fn inv_slot_mask(inv_type: u32) -> u32 {
    match inv_type {
        1 => 1 << 0,                  // HEAD
        2 => 1 << 1,                  // NECK
        3 => 1 << 2,                  // SHOULDER
        4 => 1 << 3,                  // BODY (shirt)
        5 | 20 => 1 << 4,             // CHEST / ROBE
        6 => 1 << 5,                  // WAIST
        7 => 1 << 6,                  // LEGS
        8 => 1 << 7,                  // FEET
        9 => 1 << 8,                  // WRIST
        10 => 1 << 9,                 // HANDS
        11 => 1 << 10,                // FINGER (override: single bit)
        12 => 1 << 12,                // TRINKET (override: single bit)
        13 => (1 << 15) | (1 << 16),  // WEAPON: both hands (0x18000)
        14 | 22 | 23 => 1 << 16,      // SHIELD / WEAPONOFFHAND / HOLDABLE
        15 | 25 | 26 | 28 => 1 << 17, // RANGED / THROWN / RANGEDRIGHT / RELIC
        16 => 1 << 14,                // CLOAK (back)
        17 | 21 => 1 << 15,           // 2HWEAPON / WEAPONMAINHAND
        18 => 1 << 19,                // BAG (override: single bit)
        19 => 1 << 18,                // TABARD
        _ => 1 << 23,                 // NON_EQUIP / AMMO / QUIVER / unknown: the catch-all
    }
}

/// The inv-slot dropdown's `GlobalStrings.lua` token per bit, the client's 24-entry table at
/// `0x84dd70`: the paper-doll `*SLOT` names, not the tooltip's `INVTYPE_*` ones. Bits 11, 13 and
/// 20-22 are in the table, but no `InventoryType` reaches them.
pub(super) fn inv_slot_token(bit: u32) -> Option<&'static str> {
    Some(match bit {
        0 => "HEADSLOT",
        1 => "NECKSLOT",
        2 => "SHOULDERSLOT",
        3 => "SHIRTSLOT",
        4 => "CHESTSLOT",
        5 => "WAISTSLOT",
        6 => "LEGSSLOT",
        7 => "FEETSLOT",
        8 => "WRISTSLOT",
        9 => "HANDSSLOT",
        10 => "FINGER0SLOT",
        11 => "FINGER1SLOT",
        12 => "TRINKET0SLOT",
        13 => "TRINKET1SLOT",
        14 => "BACKSLOT",
        15 => "MAINHANDSLOT",
        16 => "SECONDARYHANDSLOT",
        17 => "RANGEDSLOT",
        18 => "TABARDSLOT",
        19..=22 => "BAGSLOT",
        23 => "NONEQUIPSLOT",
        _ => return None,
    })
}

/// The inv-slot vocabulary: the set bits, ascending, of the slot mask over all recipes, unfiltered
/// (`0xbde058`, walked by `GetTradeSkillInvSlots`, `0x4ffc20`).
pub(super) fn present_inv_slots(model: &Model) -> Vec<u32> {
    let Some(t) = model.trade_skill.as_ref() else {
        return Vec::new();
    };
    let accum = t
        .recipes
        .iter()
        .fold(0u32, |m, r| m | inv_slot_mask(r.product_inv_type));
    (0..24).filter(|b| accum & (1 << b) != 0).collect()
}

/// One header of the list (`0x4fca20`), keyed by the product's `(ItemClass, ItemSubClass)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TradeSkillGroup {
    pub(super) key: (u32, u32),
    pub(super) name: String,
}

/// The key of a recipe's group; a recipe whose product template is in flight files under an
/// empty-named group keyed `u32::MAX`, which sorts last.
fn group_key(r: &TradeSkillRecipe) -> (u32, u32) {
    r.group
        .as_ref()
        .map_or((u32::MAX, u32::MAX), |(c, s, _)| (*c, *s))
}

/// The groups in the order the recipes first name them, the order the build makes the headers in
/// (`0x4fcd70`).
fn groups_in_add_order(recipes: &[TradeSkillRecipe]) -> Vec<TradeSkillGroup> {
    let mut out: Vec<TradeSkillGroup> = Vec::new();
    for r in recipes {
        let key = group_key(r);
        if out.iter().all(|g| g.key != key) {
            out.push(TradeSkillGroup {
                key,
                name: r.group.as_ref().map_or(String::new(), |g| g.2.clone()),
            });
        }
    }
    out
}

/// `0x4fd540`: item class ascending (`setg` at `0x4fd639`), then, within a class, the subclass
/// names case-blind (`0x4fd624`); 0 when either has no `ItemSubClass.dbc` name (`0x4fd630`).
fn header_order(a: &TradeSkillGroup, b: &TradeSkillGroup) -> i32 {
    if a.key.0 != b.key.0 {
        return if a.key.0 > b.key.0 { 1 } else { -1 };
    }
    if a.name.is_empty() || b.name.is_empty() {
        return 0;
    }
    crate::crt::str_cmp_i(&a.name, &b.name)
}

/// One row of the array and its visible flag (`+0x1c`, written at `0x4fd295`).
type Placed = (Row, bool);

/// `0x4fd380` over two rows, `pos` their groups' positions in the sorted header array: the
/// displayed rows first, two hidden ones equal (`0x4fd51f`); then the group's position, its header
/// row first, two headers of one group 1 either way (`0x4fd506`); then the tier ascending, the
/// product's item level descending (`setl` at `0x4fd4e6`) and the name case-blind (`0x4fd4d6`).
fn row_order(
    a: &Placed,
    b: &Placed,
    pos: &HashMap<(u32, u32), usize>,
    recipes: &[TradeSkillRecipe],
) -> i32 {
    let key = |row: &Row| match row {
        Row::Header { key, .. } => *key,
        Row::Entry(i) => group_key(&recipes[*i]),
    };
    let place = |p: &Placed| (pos.get(&key(&p.0)).copied().unwrap_or(0), p.1);
    let ((ai, av), (bi, bv)) = (place(a), place(b));
    match (av, bv) {
        (false, false) => return 0,
        (false, true) => return 1,
        (true, false) => return -1,
        (true, true) => {}
    }
    if ai != bi {
        return if bi < ai { 1 } else { -1 };
    }
    let (ra, rb) = match (&a.0, &b.0) {
        (Row::Header { .. }, Row::Header { .. }) | (Row::Entry(_), Row::Header { .. }) => return 1,
        (Row::Header { .. }, Row::Entry(_)) => return -1,
        (Row::Entry(x), Row::Entry(y)) => (&recipes[*x], &recipes[*y]),
    };
    let (ta, tb) = (ra.difficulty.tier(), rb.difficulty.tier());
    if ta != tb {
        return if ta > tb { 1 } else { -1 };
    }
    if ra.product_item_level != rb.product_item_level {
        return if ra.product_item_level < rb.product_item_level {
            1
        } else {
            -1
        };
    }
    crate::crt::str_cmp_i(&ra.name, &rb.name)
}

/// The build (`0x4fca20`): the array refilled with the recipes in the order the push holds them,
/// the line's learn order (`0x5e9d72`, `0x4fcbb7`), then a header
/// row per header (`0x4fd08c`-`0x4fd0f3`), then [`refilter`]. A recipe set that changed remakes
/// the headers in the order the recipes name them (flag 1); one that did not keeps the header
/// array as the last sort left it (flag 0). With a product's record in flight the pass leaves
/// before the header rows and the sort (`0x4fcd58`, `0x4fcf34`): the array holds the recipes in
/// add order alone, its full count theirs (`0x4fcba4`), the shown count as the last sort left it,
/// and `false` says no update follows. A set flag has reset the subclass and fold masks first
/// (`0x4fcae2`, `0x4fcae8`) and restores the saved flags only past that exit (`0x4fcf3a`), so
/// such a pass loses the folds and the filters.
pub(crate) fn build(model: &mut Model, remake_headers: bool) -> bool {
    let Some(t) = model.trade_skill.as_ref() else {
        model.trade_skill_rows.clear();
        model.trade_skill_headers.clear();
        model.trade_skill_visible = 0;
        return true;
    };
    if t.recipes.iter().any(|r| r.icon_pending.is_some()) {
        model.trade_skill_rows = (0..t.recipes.len()).map(Row::Entry).collect();
        if remake_headers {
            model.trade_skill_collapsed.clear();
            model.trade_skill_subclass_hidden.clear();
        }
        return false;
    }
    let made = groups_in_add_order(&t.recipes);
    let kept = !remake_headers
        && made.len() == model.trade_skill_headers.len()
        && made
            .iter()
            .all(|g| model.trade_skill_headers.iter().any(|h| h.key == g.key));
    if !kept {
        model.trade_skill_headers = made;
    } else {
        // The names come with this push; the order is the kept array's.
        for h in &mut model.trade_skill_headers {
            if let Some(g) = made.iter().find(|g| g.key == h.key) {
                h.name = g.name.clone();
            }
        }
    }
    let mut rows: Vec<Row> = (0..t.recipes.len()).map(Row::Entry).collect();
    rows.extend(model.trade_skill_headers.iter().map(|h| Row::Header {
        key: h.key,
        name: h.name.clone(),
    }));
    model.trade_skill_rows = rows;
    refilter(model);
    true
}

/// `0x4fd180`: each row's visible flag, the displayed count, and the sorts of the header array
/// (`0x4fd540`) and of the rows as the last sort left them (`0x4fd380`), both by the CRT's
/// `qsort` (`0x73f727`). A recipe shows when its slot bit meets the slot mask (`0x84dd64`) and its
/// group is shown, holds a member that passes the slot mask and is not folded; a header row when
/// its group is shown and holds such a member, so a folded group keeps its header (`0x4fd2f0`-
/// `0x4fd31b`).
pub(crate) fn refilter(model: &mut Model) {
    let Some(t) = model.trade_skill.as_ref() else {
        model.trade_skill_rows.clear();
        model.trade_skill_headers.clear();
        model.trade_skill_visible = 0;
        return;
    };
    let mut headers = std::mem::take(&mut model.trade_skill_headers);
    crate::crt::qsort(&mut headers, header_order);
    let pos: HashMap<(u32, u32), usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| (h.key, i))
        .collect();
    let slot_passes = |r: &TradeSkillRecipe| {
        inv_slot_mask(r.product_inv_type) & model.trade_skill_invslot_mask != 0
    };
    let mut members: HashMap<(u32, u32), usize> = HashMap::new();
    for r in t.recipes.iter().filter(|r| slot_passes(r)) {
        *members.entry(group_key(r)).or_default() += 1;
    }
    let shown = |key: &(u32, u32)| {
        !model.trade_skill_subclass_hidden.contains(key) && members.get(key).is_some_and(|&n| n > 0)
    };
    let mut placed: Vec<Placed> = std::mem::take(&mut model.trade_skill_rows)
        .into_iter()
        .map(|row| {
            let visible = match &row {
                Row::Header { key, .. } => shown(key),
                Row::Entry(i) => {
                    let r = &t.recipes[*i];
                    let key = group_key(r);
                    slot_passes(r) && shown(&key) && !model.trade_skill_collapsed.contains(&key)
                }
            };
            (row, visible)
        })
        .collect();
    crate::crt::qsort(&mut placed, |a, b| row_order(a, b, &pos, &t.recipes));
    let visible = placed.iter().filter(|p| p.1).count();
    model.trade_skill_rows = placed.into_iter().map(|p| p.0).collect();
    model.trade_skill_headers = headers;
    model.trade_skill_visible = visible;
}

/// One row of the array: a group header, or a recipe's position in `TradeSkillState::recipes`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Row {
    Header { key: (u32, u32), name: String },
    Entry(usize),
}

/// The whole array every Lua index is 1-based into, the displayed rows first; each verb bounds its
/// index by the full count (`0xbde04c`, `0x4fd861`).
pub(super) fn rows(model: &Model) -> &[Row] {
    &model.trade_skill_rows
}

/// `GetNumTradeSkills` (`0x4fd800`): the displayed count (`0xbde054`).
pub(super) fn num_rows(model: &Model) -> usize {
    model.trade_skill_visible
}

/// The recipe at a 1-based index into the whole array; `None` on a header, so the per-recipe
/// verbs and `SetTradeSkillItem` no-op there.
pub(crate) fn recipe_at(model: &Model, index: usize) -> Option<&TradeSkillRecipe> {
    let n = index.checked_sub(1)?;
    match rows(model).get(n)? {
        Row::Entry(ei) => model.trade_skill.as_ref()?.recipes.get(*ei),
        Row::Header { .. } => None,
    }
}

/// `GetFirstTradeSkill` (`0x5001c0`): the first displayed row with a spell id above 0, as an
/// index, 0 when none (`0x5001d5`-`0x5001f6`).
pub(super) fn first_recipe_index(model: &Model) -> u32 {
    rows(model)[..num_rows(model)]
        .iter()
        .position(|r| matches!(r, Row::Entry(_)))
        .map_or(0, |p| (p + 1) as u32)
}

/// `GetTradeSkillSelectionIndex` (`0x4fd9f0` → `0x4fc9e0`): the selected spell's position among
/// every row plus 1, 0 when no row holds it.
pub(super) fn selected_index(model: &Model) -> u32 {
    let spell = model.trade_skill_selected_spell;
    if spell == 0 {
        return 0;
    }
    let Some(t) = model.trade_skill.as_ref() else {
        return 0;
    };
    rows(model)
        .iter()
        .position(|r| matches!(r, Row::Entry(ei) if t.recipes[*ei].spell_id == spell))
        .map_or(0, |p| (p + 1) as u32)
}

/// `SelectTradeSkill` (`0x4fc9b0`): holds the recipe at an index by its spell id (`0xbde044`),
/// which survives a close and remaps across a re-push. A header row's id is -1 and stores 0 (`jg`
/// at `0x4fc9c9`), as an index past the full count does, compared unsigned (`jae` at
/// `0x4fc9b6`): both clear the selection. The stock Lua folds a header instead of selecting it
/// (`Blizzard_TradeSkillUI.lua:184-192`).
pub(super) fn select(model: &mut Model, index: u32) {
    let spell = index
        .checked_sub(1)
        .and_then(|n| match rows(model).get(n as usize) {
            Some(Row::Entry(ei)) => model.trade_skill.as_ref()?.recipes.get(*ei),
            Some(Row::Header { .. }) | None => None,
        })
        .map_or(0, |r| r.spell_id);
    model.trade_skill_selected_spell = spell;
}

/// What a fold verb's index names (`0x5000c0`, `0x500140`).
pub(super) enum Fold {
    /// Every group, the stock collapse-all button's 0.
    All,
    /// The group of a header row.
    Group((u32, u32)),
}

/// The fold verbs' target for an `_ftol`'d index: `index - 1` below 0 is every group (`jns` at
/// `0x5000f5` and `0x500175`); otherwise the row there, within the full count, must be a header,
/// whose group `0x4fd650` finds, and a recipe row or one past the rows is `None`, which the verbs
/// raise.
pub(super) fn fold_target(model: &Model, index: i32) -> Option<Fold> {
    let Ok(n) = usize::try_from(index.wrapping_sub(1)) else {
        return Some(Fold::All);
    };
    match rows(model).get(n)? {
        Row::Header { key, .. } => Some(Fold::Group(*key)),
        Row::Entry(_) => None,
    }
}

/// Folds or unfolds `target`: every group takes the collapse mask 0 or all ones (`0x5000f9`,
/// `0x50017a`), one group its bit (`0x500133`, `0x5001b2`).
pub(super) fn set_collapsed(model: &mut Model, target: Fold, collapse: bool) {
    let targets: Vec<(u32, u32)> = match target {
        Fold::All if !collapse => {
            model.trade_skill_collapsed.clear();
            return;
        }
        Fold::All => model.trade_skill_headers.iter().map(|g| g.key).collect(),
        Fold::Group(key) => vec![key],
    };
    for k in targets {
        if collapse {
            model.trade_skill_collapsed.insert(k);
        } else {
            model.trade_skill_collapsed.remove(&k);
        }
    }
}
