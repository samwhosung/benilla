//! The display-tree + filter machinery: grouping/sorting the flat `TradeSkillRecipe` list into the
//! visible-row tree the Era API indexes (see the parent module doc — the grouped-list law and the
//! byte-VERIFIED `SubClassFilter`/`InvSlot` filter family, `0x4fca20`/`0x4fd180`).

use std::collections::HashMap;

use crate::script::Model;

use super::TradeSkillRecipe;

/// Fold a product's `InventoryType` to its InvSlot-filter contribution (`record+0x10`) — the
/// real client's 29-entry `DAT_00809200` table with its build-time overrides, dumped in full and
/// byte-VERIFIED (`0x809200`): FINGER/TRINKET/BAG collapse to their first slot's
/// single bit (`0xb → 0x400`, `0xc → 0x1000`, `0x12 → 0x80000` — the raw table's multi-slot
/// entries are dead, overwritten before use), zero contributions (NON_EQUIP/AMMO/QUIVER) take the
/// `0x800000` catch-all (bit 23), and **WEAPON (13) is the one multi-bit survivor** — `0x18000`,
/// a one-hand weapon shows under both hand slots.
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
        11 => 1 << 10,                // FINGER (override — single bit)
        12 => 1 << 12,                // TRINKET (override — single bit)
        13 => (1 << 15) | (1 << 16),  // WEAPON — both hands (0x18000)
        14 | 22 | 23 => 1 << 16,      // SHIELD / WEAPONOFFHAND / HOLDABLE
        15 | 25 | 26 | 28 => 1 << 17, // RANGED / THROWN / RANGEDRIGHT / RELIC
        16 => 1 << 14,                // CLOAK (back)
        17 | 21 => 1 << 15,           // 2HWEAPON / WEAPONMAINHAND
        18 => 1 << 19,                // BAG (override — single bit)
        19 => 1 << 18,                // TABARD
        _ => 1 << 23,                 // NON_EQUIP / AMMO / QUIVER / unknown — the catch-all
    }
}

/// The InvSlot dropdown's per-bit GlobalString **token** — the real client's 24-entry table
/// (`0x84dd70`), dumped entry by entry. The caller
/// resolves it against the player's own `GlobalStrings.lua`; these are the
/// paper-doll `*SLOT` family, **not** the `INVTYPE_*` family the item tooltip's slot line uses,
/// and that distinction is exactly why the word cannot be stored here: `SECONDARYHANDSLOT`,
/// `INVTYPE_SHIELD` and `INVTYPE_WEAPONOFFHAND` are three separately-localizable strings that all
/// read "Off Hand" in enUS.
///
/// Bits 11/13/20-22 (`FINGER1SLOT`/`TRINKET1SLOT`/the extra `BAGSLOT`s) are present in the table
/// but unreachable through [`inv_slot_mask`] — kept for the full 24-entry fidelity.
///
/// **`None` is the out-of-range arm and is unreachable by construction**: the only caller walks
/// [`present_inv_slots`], which enumerates bits 0..23, and the reference's table has an entry for
/// every one of them. It resolves to the empty string at the call site, which is what
/// `FrameScript_GetText` hands back for a token the table has no row for.
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

/// The InvSlot filter vocabulary: the set bits of the accumulated slot mask, ascending — the real
/// client's bits-0..23 walk over `0xbde058` (`GetTradeSkillInvSlots 0x4ffc20`).
/// Accumulated over ALL recipes, unfiltered — the mask ORs up at list build, before any filter
/// applies.
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

/// Whether a recipe passes the current filters (the recompute's own row test, `0x4fd180`): its slot
/// contribution overlaps the shown-mask (`record+0x10 & 0x84dd64 != 0`) and its group isn't
/// subclass-hidden.
fn passes_filters(model: &Model, r: &TradeSkillRecipe) -> bool {
    let key = r
        .group
        .as_ref()
        .map_or((u32::MAX, u32::MAX), |(c, s, _)| (*c, *s));
    !model.trade_skill_subclass_hidden.contains(&key)
        && inv_slot_mask(r.product_inv_type) & model.trade_skill_invslot_mask != 0
}

/// One recipe group in the synthesized display tree (`0x4fca20`): the created item's
/// `(ItemClass, ItemSubClass)` key (the collapse key) + resolved display name, and the positions
/// (into [`TradeSkillState::recipes`]) of the group's recipes, pre-sorted by tier, product
/// ItemLevel, then name.
/// Engine-internal — rebuilt fresh from the flat recipes on every query ([`build_groups`]) rather
/// than cached on [`Model`] (unlike [`super::skills::SkillGroup`]): `TradeSkillState` has no
/// engine-owned tree field of its own, so [`Row::Header`] just carries the group's key/name
/// directly instead of an index into a cached `Vec`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct TradeSkillGroup {
    pub(super) key: (u32, u32),
    pub(super) name: String,
    /// Positions into [`TradeSkillState::recipes`], sorted by tier, product ItemLevel, then
    /// [`collate`]d name.
    entries: Vec<usize>,
}

/// The WoW enUS collator, approximated ([`super::skills`]/[`super::trainer`]'s own helper,
/// duplicated here rather than shared — each seam module keeps its own copy, the established local
/// convention): case-insensitive alphabetical, raw bytes as a stable tie-break.
fn collate(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// Build the display tree from the flat recipes (the module doc's grouped-list law,
/// `0x4fca20`): group by the created item's `(ItemClass, ItemSubClass)` — a recipe with no
/// group yet (`group: None`, its product template still in flight) buckets into the trailing
/// `(u32::MAX, u32::MAX)`/`""` group; sort each group's recipes by tier, then the product's
/// **ItemLevel ascending** (the `record+0x14` secondary key — field identity pinned 2026-07-17,
/// [`TradeSkillRecipe::product_item_level`]), then [`collate`]d name; sort the groups by
/// `ItemClass` id ascending, ties broken by [`collate`]d name (never by subclass id — the pending
/// group's `u32::MAX` class id sorts it last with no special-casing needed).
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

/// One visible row of the display tree: a group **header** (carrying its key/name — see
/// [`TradeSkillGroup`]'s own doc for why this isn't an index) or a **recipe** (carrying its
/// position into [`TradeSkillState::recipes`]).
#[derive(Clone)]
pub(super) enum Row {
    Header { key: (u32, u32), name: String },
    Entry(usize),
}

/// The visible rows in display order: each group's header, then — when the group isn't
/// collapsed — its recipes that pass the filters ([`passes_filters`]). A subclass-hidden group,
/// or one whose every recipe the InvSlot filter drops, contributes NO rows at all — header
/// included (the real window never shows an empty header under a filter — the visibility law
/// decision 0452 landed byte-exact, `0x4fd180`: filtered recipes and emptied/
/// filtered headers drop out of the numbered list; a merely-collapsed header stays). The Lua's
/// 1-based `index` is a position in *this* list. Empty when no window is open.
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

/// The count of visible rows (headers + the recipes of uncollapsed groups).
pub(super) fn num_rows(model: &Model) -> usize {
    rows(model).len()
}

/// The recipe at a 1-based VISIBLE index, or `None` when that row is a header (or OOB) — so every
/// per-recipe getter/`DoTradeSkill` safely no-ops on a header row (`0x4fca20`).
///
/// `pub(crate)`: the tooltip channel (`super::tooltip_item`'s `SetTradeSkillItem`) SHOULD route
/// through this too — a header index, or the header rows preceding a real recipe, would otherwise
/// misalign a raw `TradeSkillState::recipes` index against the visible list the XML actually clicks
/// through. See the module doc's "KNOWN GAP" note: that follow-up isn't done here (out of this
/// change's file scope), but the seam is exposed for it.
pub(crate) fn recipe_at(model: &Model, index: usize) -> Option<&TradeSkillRecipe> {
    let n = index.checked_sub(1)?;
    match rows(model).get(n)? {
        Row::Entry(ei) => model.trade_skill.as_ref()?.recipes.get(*ei),
        Row::Header { .. } => None,
    }
}

/// The first NON-header visible index (`GetFirstTradeSkill`'s law), `0` when none.
pub(super) fn first_recipe_index(model: &Model) -> u32 {
    rows(model)
        .iter()
        .position(|r| matches!(r, Row::Entry(_)))
        .map_or(0, |p| (p + 1) as u32)
}

/// `GetTradeSkillSelectionIndex()` — the CURRENT visible index of the engine-held selection (`0`
/// when nothing is selected, or the selected recipe isn't visible right now, e.g. its group just got
/// collapsed). [`Model::trade_skill_selection`] holds a stable position into the open window's FLAT
/// `recipes` list (module doc) — collapse/expand never touches it, only what [`rows`] shows, so this
/// derives the visible position fresh on every read rather than needing an eager remap on every
/// collapse toggle ([`super::skills`]'s own by-identity pattern, adapted to a field this module can't
/// change the type of).
pub(super) fn selected_visible_index(model: &Model) -> u32 {
    let Some(pos) = model.trade_skill_selection.checked_sub(1) else {
        return 0;
    };
    rows(model)
        .iter()
        .position(|r| matches!(r, Row::Entry(ei) if *ei == pos as usize))
        .map_or(0, |p| (p + 1) as u32)
}

/// `SelectTradeSkill(index)` — resolve the 1-based VISIBLE index to a position in the open window's
/// flat recipe list and hold THAT ([`selected_visible_index`]'s own doc). A HEADER row is IGNORED —
/// the prior selection is left exactly as it was (the module doc: the ref's own `SetSelection`
/// toggles the fold instead of selecting, so a header index never reaches here through the XML in
/// practice, but the engine contract holds regardless). An out-of-range index clears the selection
/// (the pre-existing convention, unchanged).
pub(super) fn select(model: &mut Model, index: u32) {
    let visible = rows(model);
    match index.checked_sub(1).and_then(|n| visible.get(n as usize)) {
        Some(Row::Entry(ei)) => {
            model.trade_skill_selection = (*ei + 1) as u32;
            // The spell-id shadow (`0xbde044`'s own storage) — what survives a close→reopen and
            // remaps across a re-push ([`super::UiScript::set_trade_skill`]).
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

/// Collapse (`collapse = true`) or expand a group by the **display index of its header row**
/// ([`super::skills`]'s own `Collapse/ExpandSkillHeader` shape). `id == 0` targets ALL groups (the
/// CollapseAll semantics — no such button ships this slice, the XML's own deviation note); `id > 0`
/// resolves the header at that visible index to its group key. A non-header (or OOB) index is a
/// no-op.
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
