//! The tradeskill bindings (decision 0437 phase 2) — the Era-shaped crafting-window surface driving a
//! faithful port of the real 1.12 `TradeSkillFrame` (extracted from `interface.MPQ`:
//! `Interface\FrameXML\Blizzard_TradeSkillUI\TradeSkillFrame.{xml,lua}`). Same two-way seam as
//! [`super::trainer`]: the app pushes a **tradeskill snapshot** ([`UiScript::set_trade_skill`] — the
//! recipes already resolved to name/icon/reagents/tools/color by the app from `Spell.dbc`/
//! `SkillLineAbility.dbc`/bag counts), and the Lua `DoTradeSkill`/`CloseTradeSkill` calls queue
//! outbound **intents** the app drains ([`UiScript::take_trade_skill_dos`] returns
//! `(spell id, count)` pairs / [`UiScript::take_trade_skill_close`]). The engine holds no recipe
//! knowledge — a recipe is name/icon/difficulty/reagents/tools/cooldown, all app-resolved (0437 §"The
//! decision" item 1).
//!
//! ## The Era API shape (matched to the real `TradeSkillFrame.lua`)
//!
//! `GetTradeSkillInfo(index) → name, type, numAvailable, isExpanded`: `type` is either the recipe's
//! difficulty color key (`"optimal"|"medium"|"easy"|"trivial"`, [`TradeSkillDifficulty::as_str`]) or
//! `"header"` for a group row (see below); `isExpanded` is `nil` for a recipe row, `1`/`nil` for a
//! header. `GetTradeSkillReagentInfo`/`GetTradeSkillTools`/`GetTradeSkillCooldown`/
//! `GetTradeSkillNumMade`/`GetTradeskillRepeatCount` (note the lowercase "s" in "Tradeskill" — that
//! IS the real 1.12 spelling) round out the detail pane; `DoTradeSkill` queues the craft,
//! `CloseTradeSkill` is the client-side close (vanilla sends no packet here either, matching
//! [`super::trainer`]'s `CloseTrainer` precedent).
//!
//! ## The grouped list (`0x4fca20`)
//!
//! Unlike v1's flat render, the engine now owns the WHOLE display tree, built fresh from
//! [`TradeSkillState::recipes`] on every query ([`build_groups`]) — [`TradeSkillState::recipes`] is
//! just the app's flat, unordered input; its push order carries no meaning. Every recipe belongs to
//! a group keyed by the CREATED item's `(ItemClass, ItemSubClass)`, named from `ItemSubClass.dbc`
//! (resolved app-side — [`TradeSkillRecipe::group`]); a recipe whose product template is still in
//! flight has no group yet (the client's own one-frame header deferral) and buckets trailing under
//! a synthesized group keyed `(u32::MAX, u32::MAX)` with an empty name — no real `ItemClass` reaches
//! `u32::MAX`, so it sorts last with no special-casing. Groups order by `ItemClass` id ascending,
//! ties broken alphabetically by group name (case-insensitive, never by subclass id); within a
//! group, recipes order by difficulty tier ascending (Optimal < Medium < Easy < Trivial — the byte
//! tiers 0..3, [`TradeSkillDifficulty::tier`]), ties broken alphabetically by recipe name. The
//! client's own middle tiebreak (an item-template field) is SKIPPED — the one INFERRED remainder,
//! named and pinned like [`TradeSkillDifficulty`]'s own INTERIM color law once was.
//!
//! Visible rows are each group's header (always shown) plus — for an uncollapsed group — its
//! recipes; every index the Era API takes/returns is 1-based into THAT list (mirrors
//! [`super::skills`]'s tree exactly: groups + collapse + visible-index remapping). `GetTradeSkillInfo`
//! on a header row returns `(groupName, "header", 0, isExpanded)`; `GetNumTradeSkills()` counts
//! visible rows; `GetFirstTradeSkill()` is the first NON-header visible index (`0` when none).
//! `Expand/CollapseTradeSkillSubClass(i)` fold a group by its header's VISIBLE index (`i == 0` =
//! ALL groups — the CollapseAll button's own call shape); the collapse set is keyed by group key
//! ([`Model::trade_skill_collapsed`]) and survives a `set_trade_skill` content re-push (pruned to
//! still-live groups). `GetTradeSkillSubClasses()` returns the current group names in group
//! order — the VERIFIED filter-dropdown vocabulary IS the header list, and it never shrinks
//! under a filter — and the `SubClassFilter`/`InvSlot` filter family: the subclass filter is
//! hidden group keys (the client's per-header
//! `+0xc` flag, position mask `0x84dd60` derived), the inv-slot filter is a shown-bit mask
//! (`0x84dd64`) over the accumulated slot vocabulary, [`rows`] drops filtered recipes and any
//! group left empty (a merely-collapsed group keeps its header), and every engine-side mutator
//! (a filter set, an expand/collapse) raises the `trade_skill_touched` flag the app answers with
//! `TRADE_SKILL_UPDATE` — the real client's in-call recompute+resort + event 0x13a (the
//! `0x4fd710`/`0x4fd730`/`0x4fd750` writer trio). Persistence is the client's `0xbde064`-keyed
//! story: filters, collapse, and the spell-id selection all survive a same-profession
//! close→reopen and reset on a profession switch ([`UiScript::set_trade_skill`]). The InvSlot
//! vocabulary folds each product's `InventoryType` through the dumped `DAT_00809200` table
//! ([`inv_slot_mask`] — WEAPON contributes both hand bits) and names bits via the `0x84dd70`
//! token table ([`inv_slot_name`]).
//!
//! Every per-recipe getter — `DoTradeSkill`, `GetTradeSkillIcon`/`NumMade`/`Cooldown`/`NumReagents`/
//! `ReagentInfo`/`Tools` — resolves its `index` through this same visible mapping via [`recipe_at`]
//! and no-ops/nil-shapes on a header row. **`SelectTradeSkill(i)` on a header index is IGNORED, not
//! cleared** — the ref's own `SetSelection` toggles the fold instead of selecting (ref l.184-192,
//! transcribed in the XML); the engine simply leaves the prior selection untouched. The selection
//! itself is held internally as a position into the OPEN WINDOW's FLAT `recipes` list — stable
//! across a collapse/expand (which never reorders `recipes`, only what's visible) and remapped by
//! spell id only across a `set_trade_skill` re-push ([`selected_visible_index`] derives the current
//! VISIBLE index from that flat position on every read, [`super::skills`]'s own by-identity
//! persistence pattern adapted to a field this module can't change the type of).
//!
//! ## The tooltip channel
//!
//! `GameTooltip:SetTradeSkillItem(skillIndex [, reagentIndex])` is registered in
//! [`super::tooltip_item`], beside `SetMerchantItem`, through the same id-keyed item renderer: with
//! `reagentIndex` it resolves that recipe's reagent's item, without it the recipe's PRODUCT item. A
//! product id of `0` (a pure-effect recipe) or an in-flight ask-once template answer both fall back to
//! the renderer's own name-only line — the recipe's name for the product channel — rather than a
//! no-op, since that fallback already exists and reads better than a blank hover.
//!
//! `skillIndex` is a **VISIBLE** index and is resolved through [`recipe_at`] (`pub(crate)` for
//! exactly this), never a raw [`TradeSkillState::recipes`] position — headers interleave with rows
//! since the grouped build (`0x4fca20`) landed, so a raw index would show the wrong item the moment
//! any group precedes the selected recipe's own. A header index resolves to `None` and the hover is
//! a no-op.

use std::collections::HashSet;

mod api;
mod view;

#[cfg(test)]
mod tests;

use view::build_groups;

pub(super) use api::install;
pub(crate) use view::recipe_at;

/// The recipe difficulty band (the color law, computed app-side): the Lua color-table key
/// `GetTradeSkillInfo` returns as its `type`. These four names ARE the real client's own
/// `TradeSkillTypeColor` keys (`TradeSkillFrame.lua`); WHICH recipes land in which band (the
/// trivial-rank cut points) is CONFIRMED at the bytes (decision 0446, `0x4fca20`:
/// gray ≥ trivialHigh, green ≥ the low/high midpoint, yellow ≥ trivialLow, orange below) — this
/// enum is the verified Era vocabulary the app's law picks from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeSkillDifficulty {
    /// Orange — a near-certain skill-up (the hardest recipes relative to current rank).
    Optimal,
    /// Yellow — a good skill-up chance.
    Medium,
    /// Green — a fading skill-up chance.
    Easy,
    /// Gray — no more skill-up; still craftable for its own sake.
    Trivial,
}

impl TradeSkillDifficulty {
    /// The Era `type` string `GetTradeSkillInfo` returns for a RECIPE row (a header row returns
    /// `"header"` instead — the module doc's grouped list).
    pub fn as_str(self) -> &'static str {
        match self {
            TradeSkillDifficulty::Optimal => "optimal",
            TradeSkillDifficulty::Medium => "medium",
            TradeSkillDifficulty::Easy => "easy",
            TradeSkillDifficulty::Trivial => "trivial",
        }
    }

    /// The byte tier a group sorts recipes by, ascending (`0x4fd380`):
    /// Optimal < Medium < Easy < Trivial, 0..3. The **Craft** window's row comparators key on the
    /// same tier byte first (`row[+0xc]`, decision 1124), which is why this is `pub(crate)` and not
    /// private to this module: the client's two code tables are one vocabulary shifted by one
    /// (Craft's `0` is a `"none"` tier benilla cannot produce), so the *relative* order is identical
    /// and the shift is invisible to a comparator.
    pub(crate) fn tier(self) -> u8 {
        match self {
            TradeSkillDifficulty::Optimal => 0,
            TradeSkillDifficulty::Medium => 1,
            TradeSkillDifficulty::Easy => 2,
            TradeSkillDifficulty::Trivial => 3,
        }
    }
}

/// One reagent a recipe consumes (`GetTradeSkillReagentInfo`): the bag-counted need/have plus the
/// resolved name/icon — both `None` until the app's ask-once item-template answer lands (the same
/// miss path as [`super::ItemTemplateView`]'s own asks), at which point the ref Lua sees the slot as
/// blank (a `nil` name/texture hides the row's icon+text, not an error state).
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillReagent {
    /// The reagent's item entry — the tooltip channel key (`GameTooltip:SetTradeSkillItem(i, j)`
    /// resolves through this, never through `name`/`icon`).
    pub item: u32,
    /// The resolved display name, or `None` while the ask-once template answer is still in flight.
    pub name: Option<String>,
    /// The resolved icon texture path, or `None` while still in flight.
    pub icon: Option<String>,
    /// How many this recipe consumes.
    pub need: u32,
    /// How many the player's bags currently hold (`count_of`, decision 0269) — the row's own
    /// have-vs-need gate, which the ref Lua grays/counts off directly.
    pub have: u32,
}

/// One recipe row (`GetTradeSkillInfo` + its detail-pane getters): a known spell carrying attr
/// `SPELL_ATTR_IS_TRADESKILL` (0437 §"What scoping verified"), resolved by the app from
/// `Spell.dbc`/`SkillLineAbility.dbc`/bag counts into everything the window shows.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillRecipe {
    /// The header key (`0x55ba30`): the created item's `(ItemClass,
    /// ItemSubClass)` + the resolved `ItemSubClass.dbc` display name; `None` while the product's
    /// template is still in flight (the client's own one-frame header deferral) — [`build_groups`]
    /// buckets those trailing under a synthesized empty-named group.
    pub group: Option<(u32, u32, String)>,
    /// The recipe spell id — what `DoTradeSkill` queues as the `CMSG_CAST_SPELL` target (one send
    /// per item crafted, decision 0437 — there is no wire count field).
    pub spell_id: u32,
    pub name: String,
    /// The color band `GetTradeSkillInfo`'s `type` reports ([`TradeSkillDifficulty`]).
    pub difficulty: TradeSkillDifficulty,
    /// `min` over reagents of `floor(have/need)` — `0` when any reagent is short (uncraftable); the
    /// count `GetTradeSkillInfo` reports as `numAvailable`.
    pub num_available: u32,
    /// The product item's icon if it resolved, else the spell's own icon (an app-resolved fallback);
    /// `None` only while both are still in flight.
    pub icon: Option<String>,
    pub min_made: u32,
    pub max_made: u32,
    /// Remaining tradeskill cooldown in seconds (the Mooncloth-style per-recipe cooldown); `None` =
    /// ready now (`GetTradeSkillCooldown` returns `nil`, which the ref Lua tests for truthiness).
    pub cooldown_secs: Option<u64>,
    /// The item entry this recipe creates — the tooltip channel key for the no-`reagentIndex` call to
    /// `SetTradeSkillItem`; `0` = no product item (a pure-effect recipe has nothing to show there).
    pub product_item: u32,
    /// The product's `InventoryType` (item template) — the InvSlot filter's raw input, folded to
    /// its slot-bit contribution by [`inv_slot_mask`] (the real client's
    /// `DAT_00809200[InventoryType]` OR-accumulate (`0x4fcee6`) + overrides).
    /// `0` (non-equip, or the template still in flight) folds to the `0x800000` catch-all bit.
    pub product_inv_type: u32,
    /// The product's `ItemLevel` (item template `+0x38`) — the sort's SECONDARY key, ascending,
    /// between the difficulty tier and the name (the `record+0x14` field identity: the dbcache
    /// `ItemStats_C::Read 0x7c9640` write order). `0` while the template is in flight — moot,
    /// since a template-less recipe has
    /// no group yet and buckets trailing anyway.
    pub product_item_level: u32,
    /// Consumed reagents, in display order (`GetTradeSkillNumReagents`/`GetTradeSkillReagentInfo`).
    pub reagents: Vec<TradeSkillReagent>,
    /// The Requirements line's tool list — `(tool name, have)` pairs (e.g. `("Anvil", true)`); exposed
    /// to Lua as `GetTradeSkillTools`'s alternating multivalue, which the ref feeds into
    /// `BuildColoredListString`.
    pub tools: Vec<(String, bool)>,
}

/// One open tradeskill window: the skill line's name/rank and its recipe list. Pushed whole by the
/// app ([`UiScript::set_trade_skill`]); `None` means the window is closed.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillState {
    /// The skill line id (`SkillLine.dbc`) — the persistence cache key (`0xbde064`): a push
    /// for a different line than the last resets the filters, the
    /// collapse set, and the selection; the same line keeps them, across close/reopen too.
    pub line: u32,
    /// The skill line's display name (`SkillLine.dbc`) — e.g. `"Tailoring"`.
    pub line_name: String,
    pub rank: u32,
    pub max_rank: u32,
    /// The window's recipe rows — the app's FLAT, UNORDERED input; the engine owns ALL ordering
    /// (group + tier + name, the module doc's grouped-list law, `0x4fca20`) and
    /// builds the display tree fresh from this on every query ([`build_groups`]), so push order
    /// carries no meaning. The Era API's `index` is 1-based into the synthesized VISIBLE list, never
    /// straight into this slice — see [`recipe_at`].
    pub recipes: Vec<TradeSkillRecipe>,
    /// Remaining Create All repeats (`GetTradeskillRepeatCount`) — the client-side repeat machine's
    /// own counter (`0x500230`, decision 0446), not engine-driven: the app decrements
    /// and re-pushes as each repeat's cast resolves.
    pub repeat_count: u32,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open tradeskill window's recipe snapshot. Snapshots re-push
    /// every time a resolved field changes (reagent counts, ask-once answers landing, …) — no
    /// diffing happens here.
    ///
    /// Persistence is the real client's (`0x4fc910`, keyed by the
    /// `0xbde064` last-built-line cache): a push for a **different skill line** resets the two
    /// filters, the collapse set, and the selection; the **same line** keeps all of them — and
    /// because a close (`None`) touches none of that state either, they survive a close→reopen
    /// round trip exactly like the client's statics do. The selection rides its **spell id**
    /// (`0xbde044`'s own storage) — on every push it remaps to the id's new flat position, or
    /// clears if the recipe vanished. The collapse/subclass-hidden key sets are pruned to the
    /// groups the fresh recipes still produce (the client's save→restore-by-key round trip loses
    /// exactly the keys that stopped existing); the inv-slot mask is deliberately NOT pruned —
    /// the build never touches `0x84dd64`.
    pub fn set_trade_skill(&mut self, state: Option<TradeSkillState>) {
        let mut model = self.model_mut();
        match state {
            None => {
                model.trade_skill_selection = 0;
                model.trade_skill = None;
            }
            Some(s) => {
                if model.trade_skill_last_line != s.line {
                    model.trade_skill_collapsed.clear();
                    model.trade_skill_subclass_hidden.clear();
                    model.trade_skill_invslot_mask = u32::MAX;
                    model.trade_skill_selected_spell = 0;
                    model.trade_skill_last_line = s.line;
                }
                model.trade_skill_selection = (model.trade_skill_selected_spell != 0)
                    .then(|| {
                        s.recipes
                            .iter()
                            .position(|r| r.spell_id == model.trade_skill_selected_spell)
                    })
                    .flatten()
                    .map_or(0, |i| (i + 1) as u32);
                let live: HashSet<(u32, u32)> = build_groups(&s.recipes)
                    .into_iter()
                    .map(|g| g.key)
                    .collect();
                model.trade_skill_collapsed.retain(|k| live.contains(k));
                model
                    .trade_skill_subclass_hidden
                    .retain(|k| live.contains(k));
                model.trade_skill = Some(s);
            }
        }
    }

    /// Drain the **(spell id, count)** intents `DoTradeSkill` queued since the last call — the engine
    /// resolves each clicked recipe's INDEX to its spell id, so the app sends `CMSG_CAST_SPELL`
    /// without needing the index mapping; `count` is the total the app's own client-side repeat
    /// loop (decision 0437 §5, `0x4fd7b0`) turns into that many sequential sends, one per item —
    /// "Create" queues `1`, "Create All" queues [`TradeSkillState::repeat_count`].
    pub fn take_trade_skill_dos(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.model_mut().trade_skill_dos)
    }

    /// Whether `CloseTradeSkill` was called since the last drain (and clear the flag). Like
    /// [`super::trainer`]'s `CloseTrainer`, the client-side close sends no packet — the app just
    /// clears its local tradeskill state.
    pub fn take_trade_skill_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_skill_close)
    }

    /// Whether an engine-side list mutator ran since the last drain (a filter set, an
    /// expand/collapse) — the real client answers those from inside the C call with a
    /// recompute+resort + `TRADE_SKILL_UPDATE` (the `0x4fd710`/`0x4fd730`/`0x4fd750` writer trio
    /// → event 0x13a); the app drains this and fires the event the
    /// same frame (the ref's `CollapseAllButton_OnClick` and the filter-menu clicks repaint ONLY
    /// off that event — no direct `TradeSkillFrame_Update()` call).
    pub fn take_trade_skill_touched(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_skill_touched)
    }
}
