//! The visible rows the tradeskill API indexes: grouping, sorting and filtering the flat recipes
//! (`0x4fca20`, `0x4fd180`).

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

/// The recompute's row test (`0x4fd180`): the slot bits meet the shown mask (`0x84dd64`) and the
/// group is not hidden.
fn passes_filters(model: &Model, r: &TradeSkillRecipe) -> bool {
    let key = r
        .group
        .as_ref()
        .map_or((u32::MAX, u32::MAX), |(c, s, _)| (*c, *s));
    !model.trade_skill_subclass_hidden.contains(&key)
        && inv_slot_mask(r.product_inv_type) & model.trade_skill_invslot_mask != 0
}

/// One recipe group (`0x4fca20`), keyed by the product's `(ItemClass, ItemSubClass)` and rebuilt
/// on every query.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct TradeSkillGroup {
    pub(super) key: (u32, u32),
    pub(super) name: String,
    /// Positions into `TradeSkillState::recipes`, in display order.
    entries: Vec<usize>,
}

/// An approximation of the reference's name compare, the case-insensitive `0x64a4c0`:
/// case-insensitive, then raw bytes.
fn collate(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// The groups in display order (`0x4fca20`): by `ItemClass`, then name, never subclass id; each
/// group's recipes by tier, product ItemLevel (`record+0x14`), then name. A recipe with no group
/// yet goes into an empty-named group keyed `u32::MAX`, which sorts last.
pub(super) fn build_groups(recipes: &[TradeSkillRecipe]) -> Vec<TradeSkillGroup> {
    let mut map: HashMap<(u32, u32), TradeSkillGroup> = HashMap::new();
    for (i, r) in recipes.iter().enumerate() {
        let (key, name) = match &r.group {
            Some((class, subclass, name)) => ((*class, *subclass), name.clone()),
            None => ((u32::MAX, u32::MAX), String::new()),
        };
        map.entry(key)
            .or_insert_with(|| TradeSkillGroup {
                key,
                name,
                entries: Vec::new(),
            })
            .entries
            .push(i);
    }
    let mut groups: Vec<TradeSkillGroup> = map.into_values().collect();
    for g in &mut groups {
        g.entries.sort_by(|&a, &b| {
            recipes[a]
                .difficulty
                .tier()
                .cmp(&recipes[b].difficulty.tier())
                .then_with(|| {
                    recipes[a]
                        .product_item_level
                        .cmp(&recipes[b].product_item_level)
                })
                .then_with(|| collate(&recipes[a].name, &recipes[b].name))
        });
    }
    groups.sort_by(|a, b| {
        a.key
            .0
            .cmp(&b.key.0)
            .then_with(|| collate(&a.name, &b.name))
    });
    groups
}

/// One visible row: a group header, or a recipe's position in `TradeSkillState::recipes`.
#[derive(Clone)]
pub(super) enum Row {
    Header { key: (u32, u32), name: String },
    Entry(usize),
}

/// The visible rows every Lua index is 1-based into: each group's header, then its recipes that
/// pass the filters unless it is folded. A group the filters empty loses its header too; a merely
/// folded one keeps it (`0x4fd180`).
pub(super) fn rows(model: &Model) -> Vec<Row> {
    let Some(t) = model.trade_skill.as_ref() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for g in build_groups(&t.recipes) {
        if model.trade_skill_subclass_hidden.contains(&g.key) {
            continue;
        }
        let entries: Vec<usize> = g
            .entries
            .into_iter()
            .filter(|&ei| passes_filters(model, &t.recipes[ei]))
            .collect();
        if entries.is_empty() {
            continue;
        }
        let collapsed = model.trade_skill_collapsed.contains(&g.key);
        out.push(Row::Header {
            key: g.key,
            name: g.name,
        });
        if !collapsed {
            for ei in entries {
                out.push(Row::Entry(ei));
            }
        }
    }
    out
}

pub(super) fn num_rows(model: &Model) -> usize {
    rows(model).len()
}

/// The recipe at a 1-based visible index; `None` on a header, so the per-recipe verbs and
/// `SetTradeSkillItem` no-op there.
pub(crate) fn recipe_at(model: &Model, index: usize) -> Option<&TradeSkillRecipe> {
    let n = index.checked_sub(1)?;
    match rows(model).get(n)? {
        Row::Entry(ei) => model.trade_skill.as_ref()?.recipes.get(*ei),
        Row::Header { .. } => None,
    }
}

/// `GetFirstTradeSkill`: the first recipe row's visible index, 0 when none.
pub(super) fn first_recipe_index(model: &Model) -> u32 {
    rows(model)
        .iter()
        .position(|r| matches!(r, Row::Entry(_)))
        .map_or(0, |p| (p + 1) as u32)
}

/// `GetTradeSkillSelectionIndex`: the selection's visible index, 0 when none or hidden. The
/// selection is held as a flat recipe position, so a fold never moves it.
pub(super) fn selected_visible_index(model: &Model) -> u32 {
    let Some(pos) = model.trade_skill_selection.checked_sub(1) else {
        return 0;
    };
    rows(model)
        .iter()
        .position(|r| matches!(r, Row::Entry(ei) if *ei == pos as usize))
        .map_or(0, |p| (p + 1) as u32)
}

/// `SelectTradeSkill`: holds the recipe at a visible index. A header index leaves the selection
/// as it was, and an out-of-range one clears it; the stock Lua folds a header instead of selecting
/// it (`Blizzard_TradeSkillUI.lua:184-192`).
pub(super) fn select(model: &mut Model, index: u32) {
    let visible = rows(model);
    match index.checked_sub(1).and_then(|n| visible.get(n as usize)) {
        Some(Row::Entry(ei)) => {
            model.trade_skill_selection = (*ei + 1) as u32;
            // The spell id (`0xbde044`) is what survives a close and remaps across a re-push.
            model.trade_skill_selected_spell = model
                .trade_skill
                .as_ref()
                .and_then(|t| t.recipes.get(*ei))
                .map_or(0, |r| r.spell_id);
        }
        Some(Row::Header { .. }) => {}
        None => {
            model.trade_skill_selection = 0;
            model.trade_skill_selected_spell = 0;
        }
    }
}

/// Folds or unfolds the group whose header is visible row `id`; 0 is every group, as the stock
/// collapse-all button passes, and any other row is a no-op.
pub(super) fn set_collapsed(model: &mut Model, id: usize, collapse: bool) {
    if id == 0 && !collapse {
        model.trade_skill_collapsed.clear();
        return;
    }
    let targets: Vec<(u32, u32)> = if id == 0 {
        let Some(t) = model.trade_skill.as_ref() else {
            return;
        };
        build_groups(&t.recipes)
            .into_iter()
            .map(|g| g.key)
            .collect()
    } else {
        match rows(model).get(id - 1) {
            Some(Row::Header { key, .. }) => vec![*key],
            _ => Vec::new(),
        }
    };
    for k in targets {
        if collapse {
            model.trade_skill_collapsed.insert(k);
        } else {
            model.trade_skill_collapsed.remove(&k);
        }
    }
}
