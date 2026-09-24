//! The trainer bindings (decision 0237) — the Era-shaped class/profession trainer surface driving a
//! faithful port of the real 1.12 `ClassTrainerFrame` (extracted from `interface.MPQ`:
//! `Interface\FrameXML\ClassTrainerFrame.{xml,lua}`). Same two-way seam as [`super::merchant`]: the
//! app pushes a **trainer snapshot** ([`UiScript::set_trainer`] — the wire services already resolved
//! to name/icon/cost/state/requirements by the app), and the Lua `BuyTrainerService`/`CloseTrainer`
//! calls queue outbound **intents** the app drains ([`UiScript::take_trainer_buys`] returns the
//! chosen services' **spell ids** / [`UiScript::take_trainer_close`]). The engine holds no trainer
//! knowledge — a service is name/subtext/icon/cost/state/gates/description, all app-resolved.
//!
//! ## The Era API shape (matched to the real `ClassTrainerFrame.lua`)
//!
//! The reference window's `.lua` is transcribed onto these bindings, so the return *shapes* match it
//! verbatim: `GetTrainerServiceInfo → name, subText, serviceType, isExpanded`;
//! `GetTrainerServiceCost → money, talentPointCost, professionPointCost`;
//! `GetTrainerServiceSkillReq → skill, rank, hasReq`; `GetTrainerServiceAbilityReq → name, hasReq`;
//! `IsTrainerServiceLearnSpell → isLearnSpell, isPetLearnSpell`. `serviceType` is either the string
//! `"header"` (a skill-line group header, with `isExpanded` set) or the service's colour state
//! `"available"`/`"unavailable"`/`"used"` (green/red/gray, `isExpanded` nil).
//!
//! ## The tree (decisions 0247 + 1124 — the byte-verified grouping/sort model, **per trainer type**)
//!
//! The 1.12 wire (`SMSG_TRAINER_LIST`) is a **flat** service list; the client builds a **collapsible
//! tree** on top of it, and `index` is **1-based into that visible tree**, not the wire order.
//! [`UiScript::set_trainer`] takes the flat services (each already carrying its app-resolved
//! [`TrainerService::group_key`]/[`TrainerService::group_name`]) and synthesizes the tree: one
//! **header row per distinct group key**, then that group's services in the group's own order. State
//! is a per-row **colour**, never the grouping key.
//!
//! **The `trainerType` selects everything** — the list finalizer `0x4d8410` picks the row comparator
//! by `ds:0xb73a08` (`dec eax; je` chain @ `0x4d8561`) and the builder `0x4d7560` picks both the
//! group key and the header comparator by the same dword (`0x4d7786`, `0x4d79eb`). Neither the group
//! key nor the sort is one law with a "later refinement" — they are four laws, and 1124 is where
//! benilla stopped applying the class one to all of them:
//!
//! | type | rows | headers | group key |
//! |---|---|---|---|
//! | 0 class, 3 pet | `0x4d85c0` — level → skill value → name → rank | `0x4d7b90`, by name | taught spell's `SkillLine` |
//! | 1 mount ("talent") | `0x4d8850` — **state byte → name** | `0x4d7b90`, `-1` first then by name | `-1` when the service is `used`, else the `SkillLine` |
//! | 2 tradeskill | `0x4d8760` — **skill value → name**, no level key, no rank key | `0x4d7c30`, by **raw key asc** | **1 or 2**, see below |
//!
//! At type 2 the group key is **not a skill line and never resolves one**: the builder defaults it
//! to 2 and sets it to 1 iff the **wire** spell's own `Spell.dbc Effect[0..2]` contains `44`
//! `SKILL_STEP` (`0x4d77b6`), which makes exactly two groups — `TRADESKILL_SERVICE_STEP` ("Development
//! Skills", the profession-learn services) and `TRADESKILL_SERVICE_LEARN` ("Recipes") — and means
//! **no row is ever dropped at a tradeskill trainer**. That partition, not a level key, is what puts
//! "Apprentice Blacksmith" at the top of a blacksmithing trainer: `reqLevel` is inert at type 2.
//! At the other types a service whose group key is `0` (unresolved skill line) is still dropped,
//! matching the client.
//!
//! A service hides when its state fails the dropdown filter ([`Model::trainer_filter`]) or its group
//! is collapsed ([`Model::trainer_collapsed`]) — but **hiding is not removing**: the client parks a
//! hidden record in a tail behind the visible ones and keeps its index, so `GetNumTrainerServices`
//! answers with the *visible* count while every getter's index space is the *whole* array ([`rows`]).
//! An index past the count is a legal hidden row, and the stock window is handed exactly one after a
//! purchase ([`selected_row`]). `Collapse/ExpandTrainerSkillLine(id)` take the **display index of a
//! header row** (`id == 0` = all groups — the collapse-all button); they toggle that group's
//! visibility, never reorder.
//!
//! The **selection** is likewise the selected service's **spell id** (`ds:0xb73a0c`), re-resolved to
//! a position on every `GetTrainerSelectionIndex` — never a stored row number ([`selected_row`]).
//!
//! ## Faithful stubs (no wire data yet — kept so the ported XML runs)
//!
//! `GetTrainerServiceStepReq` returns nil (no tradeskill-step data on the wire), and per-requirement
//! `hasReq`/`met` flags are derived from the service's overall `category` (an unavailable service has
//! an unmet gate) rather than a per-gate wire bit, which 5875 does not send.
//!
//! One thing the client does that this engine does not, named rather than left to be discovered: a
//! group whose row counters go to zero **vanishes entirely, header included** (`0x4d8460`,
//! `0x4d8528`–`0x4d8549`) — which is how a type-1 trainer whose every service is `used` shows one
//! "My Talents" header and no skill-line headers at all. benilla always keeps a header. The case
//! that is *reachable* here (does the state-filter dropdown emptying a group also take its header?)
//! is an open question; the build-time case above cannot arise, because benilla builds
//! a group only from services that exist.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// The green/red/gray state of a service (the wire's `TrainerSpellState`, decision 0237), surfaced to
/// Lua as the Era `category` string `GetTrainerServiceInfo` returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrainerServiceCategory {
    /// Green — learnable now.
    #[default]
    Available,
    /// Red — a requirement (level / skill / prerequisite) is unmet.
    Unavailable,
    /// Gray — already known.
    Used,
}

impl TrainerServiceCategory {
    /// The Era `category` string (`ClassTrainerFrame`'s colour switch reads exactly these).
    fn era_str(self) -> &'static str {
        match self {
            TrainerServiceCategory::Available => "available",
            TrainerServiceCategory::Unavailable => "unavailable",
            TrainerServiceCategory::Used => "used",
        }
    }

    /// The filter-flag slot this category occupies ([`Model::trainer_filter`]).
    fn filter_slot(self) -> usize {
        match self {
            TrainerServiceCategory::Available => 0,
            TrainerServiceCategory::Unavailable => 1,
            TrainerServiceCategory::Used => 2,
        }
    }

    /// The row's **state byte** `[+0x30]` — the value `0x4d8ba0` maps to the colour string (`0` →
    /// available, `2` → used, anything else → unavailable). It is a colour on every trainer type but
    /// one: at type 1 it is also the third sort key ([`talent_order`]), which is why this exists
    /// separately from the filter slot it numerically coincides with.
    fn state_key(self) -> u8 {
        match self {
            TrainerServiceCategory::Available => 0,
            TrainerServiceCategory::Unavailable => 1,
            TrainerServiceCategory::Used => 2,
        }
    }

    /// Parse the Era filter-type string the sort checkboxes pass.
    fn from_filter_str(s: &str) -> Option<Self> {
        match s {
            "available" => Some(TrainerServiceCategory::Available),
            "unavailable" => Some(TrainerServiceCategory::Unavailable),
            "used" => Some(TrainerServiceCategory::Used),
            _ => None,
        }
    }
}

/// A skill-line requirement on a service (`GetTrainerServiceSkillReq`): the skill's display name, the
/// rank required, and whether the player meets it (`hasReq`, derived from the service `category`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrainerSkillReq {
    pub name: String,
    pub rank: u32,
    pub met: bool,
}

/// A prerequisite-ability requirement (`GetTrainerServiceAbilityReq`): the ability's display name and
/// whether the player has it (`hasReq`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrainerAbilityReq {
    pub name: String,
    pub met: bool,
}

/// What `GameTooltip:SetTrainerService` describes for one row — the pre-resolved output of the
/// **app-side** tooltip law (`ui_trainer::service_tooltip`, transcribing `SetTrainerService
/// 0x5338b0`). The engine holds no DBC, so the app decides the subject and the engine only picks a
/// renderer; the binding in the real client does exactly that too — it emits no line of its own and
/// is a three-way selector into the shared spell builder `0x52e610` or item builder `0x52b650`.
///
/// This is deliberately **not** derivable from [`TrainerService::texture`]: the tooltip and the icon
/// disagree, by design and in both directions (`0x4d8f50` gates on trainer-type, `0x5338b0` does
/// not — the icon needs a trainer-type gate the tooltip does not have, and the icon pins the *wire*
/// wrapper where the tooltip hops to the *taught* spell). On ~806 of the shipped corpus's trainer
/// services the reference client visibly shows one spell's icon above another spell's tooltip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrainerTooltip {
    /// The ITEM builder, on this item id — the taught spell carries `SPELL_ATTR_IS_TRADESKILL`.
    /// `0` (or a template still in flight) renders an EMPTY tooltip, which is the client's own
    /// behaviour: the builder early-outs on a cache miss and the query is enqueued.
    Item(u32),
    /// The SPELL builder, on the taught spell — or on the wire spell itself when no learn-wrapper
    /// slot resolved, the only path that describes the wrapper.
    Spell {
        spell_id: u32,
        /// `param5` altCaster — ONE gate suppressing BOTH the totems and the reagents lines
        /// (byte-verified at `0x52ed43` and `0x52f393`). Set exactly when the matched wrapper slot
        /// was `SPELL_EFFECT_LEARN_PET_SPELL`.
        alt_caster: bool,
    },
}

impl Default for TrainerTooltip {
    fn default() -> Self {
        TrainerTooltip::Spell {
            spell_id: 0,
            alt_caster: false,
        }
    }
}

/// One trainer service row, resolved by the app from the wire `TrainerSpell` (decision 0237). Plain
/// data — position in [`TrainerState::services`] is the *unsorted* wire order; the 1-based index the
/// Lua uses is a position in the visible display tree ([`rows`], built from [`TrainerState::groups`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerService {
    /// The service spell id — what the app sends for `CMSG_TRAINER_BUY_SPELL` when this row is bought.
    pub spell_id: u32,
    /// The spell name (`GetTrainerServiceInfo`'s first return); `None` while `Spell.dbc` is still
    /// loading (the API reports `nil`, the XML shows `UNKNOWN`).
    pub name: Option<String>,
    /// The rank/subtext line (`Spell.dbc` NameSubtext — "Rank N"); `None`/empty shows no second line.
    pub subtext: Option<String>,
    /// Icon texture path (`Interface\Icons\…`); `None` while the catalog answer is in flight.
    pub texture: Option<String>,
    /// The spell description shown in the detail pane (`GetTrainerServiceDescription`); `""` when the
    /// app has no description source yet.
    pub description: String,
    /// Money cost in copper (already reputation-discounted server-side).
    pub cost: u32,
    /// A primary-profession first rank — the real window's `cpCost2 > 0` (the profession-point cost
    /// that raises the "you can only have two professions" confirm dialog).
    pub prof_first_rank: bool,
    /// The green/red/gray colour ([`TrainerServiceCategory`]).
    pub category: TrainerServiceCategory,
    /// Character level required (0 = none).
    pub level_req: u32,
    /// The skill-line requirement, or `None` when the service has no skill gate.
    pub skill_req: Option<TrainerSkillReq>,
    /// Prerequisite-ability requirements (resolved from the wire's req-spell ids); empty when none.
    pub ability_reqs: Vec<TrainerAbilityReq>,
    /// A tradeskill step (`IsTrainerServiceTradeSkill`) vs. a plain learn-spell.
    pub is_trade_skill: bool,
    /// The tree's grouping key, resolved app-side by the **trainer type's own** builder law
    /// (module doc, decision 1124): the taught spell's `SkillLine` at types 0/1/3, or the
    /// `SKILL_STEP` partition's `1`/`2` at type 2. `0` = unresolved: the service is **dropped** from
    /// the tree (the client's `skillLine == 0 → drop`) — reachable only at types 0/1/3.
    pub group_key: u32,
    /// The group's localized display name — the header row's text. `SkillLine.dbc`'s name at types
    /// 0/1/3; the `TRADESKILL_SERVICE_STEP`/`_LEARN` global strings at type 2.
    pub group_name: String,
    /// What the detail-icon hover describes ([`TrainerTooltip`]) — resolved app-side, because the
    /// law reads `Spell.dbc` fields the engine cannot see. Independent of [`Self::texture`].
    pub tooltip: TrainerTooltip,
}

/// One group in the display tree (decisions 0247/1124): the header's key + name and the positions
/// (into [`TrainerState::services`]) of the group's services, pre-sorted by the trainer type's
/// within-group comparator. Synthesized by [`UiScript::set_trainer`] — the app pushes only the flat
/// services.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerGroup {
    /// The group key (also the collapse key).
    pub key: u32,
    /// The header's display name.
    pub name: String,
    /// The group's services, as positions into [`TrainerState::services`], in display order.
    pub services: Vec<usize>,
}

/// One open trainer window: the services, the greeting line, and the wire trainer type. Pushed whole
/// by the app; `None` means no trainer is open (the window is closed).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerState {
    pub services: Vec<TrainerService>,
    /// The trainer's greeting/title line (`SMSG_TRAINER_LIST`'s trailing string).
    pub greeting: String,
    /// `SMSG_TRAINER_LIST`'s `trainerType` (`ds:0xb73a08`), verbatim — **the** switch: it selects the
    /// row comparator, the header comparator and the group-key law (module doc), and it is what the
    /// two whole-trainer predicates test (`IsTradeskillTrainer 0x4d8ea0`: `== 2`; `IsTalentTrainer
    /// 0x4d8ed0`: `== 1`). 0 class · 1 mount · 2 tradeskill · 3 pet.
    pub trainer_type: u32,
    /// The sorted skill-line groups over `services` — **built by [`UiScript::set_trainer`]**, not
    /// pushed by the app (default-empty on a bare `TrainerState`). The display tree walks these.
    pub groups: Vec<TrainerGroup>,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open trainer's service snapshot. On a push, the engine
    /// **builds the skill-line tree** ([`build_groups`]) from the flat services and prunes the
    /// collapse set to the lines that still exist (so a collapse survives a content update — a buy
    /// re-lists — but a switch to a different trainer starts expanded). Clearing resets the selection
    /// and the collapse set (a closed window holds nothing).
    pub fn set_trainer(&mut self, state: Option<TrainerState>) {
        let mut model = self.model_mut();
        match state {
            None => {
                model.trainer_selection = None;
                model.trainer_collapsed.clear();
                model.trainer = None;
            }
            Some(mut s) => {
                s.groups = build_groups(&s.services, s.trainer_type);
                let live: std::collections::HashSet<u32> = s.groups.iter().map(|g| g.key).collect();
                model.trainer_collapsed.retain(|sl| live.contains(sl));
                model.trainer = Some(s);
            }
        }
    }

    /// **Reset what one `SMSG_TRAINER_LIST` arriving resets — the state filter, the collapse set,
    /// and the selection.**
    ///
    /// Byte-verified (decision 1128): the list builder writes the filter mask itself on every
    /// packet — `0x4d75d9 mov ds:0xb73a1c,3`
    /// (available|unavailable, "already known" OFF), or `5` (available|used) when `trainerType == 1`
    /// — alongside `ds:0xb73a20 = ds:0xb73a24 = 0xffffffff`, which is "no group collapsed". So the
    /// player's filter choice does NOT live in the engine across trainer visits in the reference: it
    /// lives in the saved variable `TRAINER_FILTER_*`, and the window's show handler pushes it back
    /// over this reset (decision 1128; `TrainerFrame.xml`'s `BenillaTrainerFrame_ApplyFilter`).
    ///
    /// **The selection is the same edge and the same law.** `0x4d7560`'s tail selects record 0 —
    /// `0x4d7b40 xor ecx,ecx` → `0x4d7b42 call 0x4d74f0`, after the sort — and since row 0 is always
    /// a group header, whose record carries the id `0xffffffff` (`0x4d7b05`), the getter's scan
    /// finds the first header and answers **1**. Clearing is the same thing to every caller: the
    /// stock window only ever asks `GetTrainerSelectionIndex() > 1`, which 0 and 1 both fail, so
    /// both send it down `ClassTrainer_SelectFirstLearnableSkill`. Without this a re-opened trainer
    /// could inherit a selection from the last visit — the same spell id, still in the list, still
    /// resolving — which the reference cannot do.
    ///
    /// The repaint path `0x4d7d40` touches **none** of the three (a `ds:0xb73a0c` census over the
    /// whole image returns four references, all in the setter/getter/teardown), which is why a
    /// purchase leaves the selection sitting on the service it just greyed out.
    ///
    /// This is the **packet** edge, not the content edge — [`Self::set_trainer`] runs on every
    /// snapshot change (an item template landing, a name resolving), and the reference's repaint path
    /// `0x4d7d40` does not touch either mask. Reset there and a filter would evaporate mid-window.
    pub fn reset_trainer_list_state(&mut self, trainer_type: u32) {
        let mut model = self.model_mut();
        // Mask 5 at a mount/"talent" trainer: available + already-known, which is what makes a
        // known mount visible under its "My Talents" header at all (decision 1124's group -1).
        model.trainer_filter = if trainer_type == TRAINER_TYPE_MOUNT {
            [true, false, true]
        } else {
            [true, true, false]
        };
        model.trainer_collapsed.clear();
        model.trainer_selection = None;
    }

    /// Drain the **spell ids** `BuyTrainerService` queued since the last call (the engine resolves each
    /// clicked filtered row to its service's spell id, so the app sends `CMSG_TRAINER_BUY_SPELL`
    /// without needing to know the filter/index mapping).
    pub fn take_trainer_buys(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().trainer_buys)
    }

    /// Whether `CloseTrainer` was called since the last drain (and clear the flag). vanilla's
    /// client-side close sends no packet — the app just clears its local trainer state.
    pub fn take_trainer_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trainer_close)
    }
}

/// One visible row of the display tree: a skill-line **header** (carrying its group index) or a
/// **service** (carrying its position into [`TrainerState::services`]).
#[derive(Clone, Copy)]
enum Row {
    Header(usize),
    Service(usize),
}

/// The WoW enUS collator (`0x64a480`), approximated: case-insensitive alphabetical, with the raw
/// bytes as a stable tie-break so equal-when-folded names keep a deterministic order.
fn collate(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// `SMSG_TRAINER_LIST`'s tradeskill `trainerType` (module doc's table).
const TRAINER_TYPE_TRADESKILL: u32 = 2;
/// `SMSG_TRAINER_LIST`'s mount `trainerType` — the one the client's own vocabulary calls "talent"
/// (`IsTalentTrainer 0x4d8ed0` tests it).
const TRAINER_TYPE_MOUNT: u32 = 1;

/// The already-known group a **type 1** trainer buckets its `used` services into — the client's
/// signed `-1` group key (`0x4d77e8`), whose header takes the `KNOWN_TALENTS_HEADER` global string
/// ("My Talents", 1.12.1 enUS). Modelled as `u32::MAX` because the key is otherwise a skill-line id;
/// the header comparator `0x4d7b90` tests for it explicitly and sorts it **first**, ahead of the
/// name ordering that ranks every other header.
pub const TRAINER_GROUP_KNOWN: u32 = u32::MAX;

/// A service's required-skill value (`[+0x1c]`, the wire's `reqSkillValue`) — `0` when the service
/// carries no skill gate.
fn skill_value(s: &TrainerService) -> u32 {
    s.skill_req.as_ref().map_or(0, |r| r.rank)
}

/// The within-group comparator for a **class** (type 0) or **pet** (type 3) trainer — keys ④–⑦ of
/// the verified `0x4d85c0`: required level (`[+0x14]` byte) → required-skill value (`[+0x1c]` u32) →
/// localized name → localized rank/subtext, all ascending. Pet is not special-cased anywhere in the
/// selection chain at `0x4d8561`; it lands here with class.
fn class_order(a: &TrainerService, b: &TrainerService) -> std::cmp::Ordering {
    let name = |s: &TrainerService| s.name.clone().unwrap_or_default();
    let rank = |s: &TrainerService| s.subtext.clone().unwrap_or_default();
    a.level_req
        .cmp(&b.level_req)
        .then_with(|| skill_value(a).cmp(&skill_value(b)))
        .then_with(|| collate(&name(a), &name(b)))
        .then_with(|| collate(&rank(a), &rank(b)))
}

/// The within-group comparator for a **tradeskill** trainer (type 2) — the whole of `0x4d8760`'s
/// row half: required-skill value **ascending** (`0x4d87d9`/`0x4d8801`), then the localized name.
///
/// The two keys it pointedly does *not* have are the interesting ones, and both were in benilla's
/// output until 1124: there is **no `[+0x14]` required-level key** (which is what used to sink the
/// profession-learn row — the only row with a level — to the bottom of every profession trainer),
/// and **no `[+0x204]` rank tie-break**.
fn tradeskill_order(a: &TrainerService, b: &TrainerService) -> std::cmp::Ordering {
    let name = |s: &TrainerService| s.name.clone().unwrap_or_default();
    skill_value(a)
        .cmp(&skill_value(b))
        .then_with(|| collate(&name(a), &name(b)))
}

/// The within-group comparator for a **mount** trainer (type 1, the client's "talent") — the whole
/// of `0x4d8850`'s row half: the **state byte** `[+0x30]` ascending (`0x4d88f1`, so available →
/// unavailable → used), then the localized name. The one comparator on which the green/red/gray is a
/// sort key rather than only a colour; like the tradeskill one it has no level, skill-value or rank
/// key.
fn talent_order(a: &TrainerService, b: &TrainerService) -> std::cmp::Ordering {
    let name = |s: &TrainerService| s.name.clone().unwrap_or_default();
    a.category
        .state_key()
        .cmp(&b.category.state_key())
        .then_with(|| collate(&name(a), &name(b)))
}

/// Build the display tree from the flat services (decisions 0247/1124), by the trainer type's own
/// three laws:
///
/// * **group** on the app-resolved [`TrainerService::group_key`], dropping the unresolved `0` (which
///   a type-2 list never produces — its partition is total);
/// * **order each group's services** with that type's row comparator;
/// * **order the headers** by raw key ascending at type 2 (`0x4d7c30`: one key, no name, no
///   tie-break), else by localized name with the key breaking a name tie (`0x4d7b90`).
///
/// The service positions index back into the unchanged `services` slice, so every getter still
/// resolves a row to its full service data.
fn build_groups(services: &[TrainerService], trainer_type: u32) -> Vec<TrainerGroup> {
    let mut map: std::collections::HashMap<u32, TrainerGroup> = std::collections::HashMap::new();
    for (i, s) in services.iter().enumerate() {
        if s.group_key == 0 {
            continue;
        }
        map.entry(s.group_key)
            .or_insert_with(|| TrainerGroup {
                key: s.group_key,
                name: s.group_name.clone(),
                services: Vec::new(),
            })
            .services
            .push(i);
    }
    let mut groups: Vec<TrainerGroup> = map.into_values().collect();
    let order = match trainer_type {
        TRAINER_TYPE_TRADESKILL => tradeskill_order,
        TRAINER_TYPE_MOUNT => talent_order,
        _ => class_order,
    };
    for g in &mut groups {
        g.services
            .sort_by(|&a, &b| order(&services[a], &services[b]));
    }
    if trainer_type == TRAINER_TYPE_TRADESKILL {
        groups.sort_by_key(|g| g.key);
    } else {
        // `0x4d7b90`: the `-1` (already-known) group first, then localized name. Its second key —
        // `hdr[+0x24] == 0` first, between those two — is uncarved and unreachable here: benilla's
        // headers are one per distinct skill line, so nothing ties on the name that it would break.
        groups.sort_by(|a, b| {
            (a.key != TRAINER_GROUP_KNOWN)
                .cmp(&(b.key != TRAINER_GROUP_KNOWN))
                .then_with(|| collate(&a.name, &b.name))
                .then_with(|| a.key.cmp(&b.key))
        });
    }
    groups
}

/// The client's whole row array, and how much of its front is **on screen** — the visible rows in
/// display order (decisions 0247/1124), then every hidden one in a **tail** behind them.
///
/// **Hiding a row is not removing it**, and that is the shape of the real array rather than a
/// convenience here. The finalizer `0x4d8410` writes only a per-record visible flag — `[+0x34] = 1`
/// at `0x4d84c6`, `= 0` at `0x4d8546` — decrements the *visible* count `ds:0xb73a18` (`0x4d8549`),
/// and then `qsort`s **all `ds:0xb73a10`** records (`0x4d857e`) with that flag as the comparator's
/// PRIMARY key (`0x4d85ce` … `0x4d8744 setne al` / `0x4d874e lea eax,[eax+eax-1]` → ±1). A hidden
/// row therefore keeps a record, a position and an index — it is simply parked behind the visible
/// ones. `GetNumTrainerServices` (`0x4d8d90`) answers with the visible count `ds:0xb73a18`, while
/// the index space every getter is bounds-checked against is the **total** `ds:0xb73a10`: the
/// thirteen service getters all route through one shared accessor gate, `0x4d89b0`'s
/// `cmp ecx, dword ptr [0xb73a10]`, and `GetTrainerServiceInfo` (`0x4d8dc0`) reaches the same gate
/// through its four siblings (`0x4d8aa2`, `0x4d8b52`, `0x4d8ba2`, `0x4d8c32`). The visible count has
/// **five references image-wide** — the finalizer seeding and decrementing it, the buy-ALL loop, and
/// `GetNumTrainerServices` — and **no getter is among them**. So an index past
/// `GetNumTrainerServices()` is a legal, resolvable, *hidden* row — which is exactly the index the
/// stock window is handed after a purchase, and reading it as out-of-range is what used to strand
/// the detail pane ([`selected_row`]). An all-boxes-off window is empty because the Lua stops
/// iterating, not because the rows stopped existing.
///
/// The tail's own ORDER is the comparator's remaining keys in the client and tree order here. It is
/// unobservable through the stock window — nothing iterates past `GetNumTrainerServices()`, and the
/// single index that ever reaches in is the selection's, resolved by identity — so the difference
/// costs nothing until something walks the tail.
///
/// **The filter and the collapse are deliberately asymmetric**, and that asymmetry is structural in
/// the client rather than incidental. The finalizer builds a per-group flag `hdr[+0x1c]` by walking
/// the group's per-state member counts through the **live** state mask `ds:0xb73a1c` (`0x4d8431`,
/// `0x4d8447`), and the hide it drives at `0x4d8535` has **no header-row exemption** — so a group
/// every one of whose rows the dropdown hides disappears entirely, header included, and unchecking
/// every box empties the window (`GetNumTrainerServices() == 0`). The collapse test three
/// instructions later (`0x4d8541`) *does* exempt headers (`0x4d853d`) and reads a different field
/// (`hdr[+0x20]`), so a collapsed group keeps its header — which is what makes it re-expandable.
/// benilla had both cases keeping the header, with a test asserting it; 1124 inverted the filter
/// half.
fn rows(model: &Model) -> (Vec<Row>, usize) {
    let Some(t) = model.trainer.as_ref() else {
        return (Vec::new(), 0);
    };
    let mut shown = Vec::new();
    let mut tail = Vec::new();
    for (gi, g) in t.groups.iter().enumerate() {
        let (passes, filtered): (Vec<usize>, Vec<usize>) = g
            .services
            .iter()
            .copied()
            .partition(|&si| model.trainer_filter[t.services[si].category.filter_slot()]);
        if passes.is_empty() {
            // The whole group is filtered away — header with it (1124's inverted half).
            tail.push(Row::Header(gi));
        } else {
            shown.push(Row::Header(gi));
            // A collapsed group keeps its header on screen and parks its services behind.
            let into = if model.trainer_collapsed.contains(&g.key) {
                &mut tail
            } else {
                &mut shown
            };
            into.extend(passes.into_iter().map(Row::Service));
        }
        tail.extend(filtered.into_iter().map(Row::Service));
    }
    let visible = shown.len();
    shown.extend(tail);
    (shown, visible)
}

/// The service at a 1-based visible index, or `None` when that row is a **header** (or OOB / no
/// trainer) — so the getters/buy that read a service safely no-op on a header row. `pub(super)`:
/// `GameTooltip:SetTrainerService` lives in the tooltip channel and must resolve its index through
/// the same VISIBLE mapping, never a raw `services[]` position.
pub(super) fn service(model: &Model, index: usize) -> Option<&TrainerService> {
    let n = index.checked_sub(1)?;
    match rows(model).0.get(n)? {
        Row::Service(si) => model.trainer.as_ref()?.services.get(*si),
        Row::Header(_) => None,
    }
}

/// `GetNumTrainerServices`' answer — the count of rows **on screen** (headers + unfiltered,
/// uncollapsed services), which is the client's `ds:0xb73a18` and not the length of the row array
/// the indices run over ([`rows`]).
fn num_services(model: &Model) -> usize {
    rows(model).1
}

/// The selected service's 1-based position in the row array ([`rows`]) — which is **past
/// `GetNumTrainerServices()` when that service is hidden**, and `None` only when nothing is selected
/// or the service has left the trainer's list altogether.
///
/// **The selection is the service, not the row it happens to sit on.** `SelectTrainerService`
/// (`0x4d8e60` → `0x4d74f0`) stores the record's **spell id** at `ds:0xb73a0c` — 0 when the index is
/// out of range, never a clamp — and this getter (`0x4d8f10` → `0x4d7520`) finds it again by linear
/// scan over the whole array, `cmp dword ptr [edi], esi` at `0x4d7543`, answering `-1` → Lua `0`
/// only when the scan runs out. Three separate things renumber the rows under a stored number — the
/// state filter, a collapse, a re-list — so an index stored on Monday is a different service on
/// Tuesday, and one merely *clamped* to the row count is the worst answer available: plausible, in
/// range, and wrong. benilla stored the number.
///
/// It is load-bearing far beyond bookkeeping, because the stock `Blizzard_TrainerUI.lua` repaints
/// its detail pane through exactly **one** door. `ClassTrainerTrainButton_OnClick` clears
/// `ClassTrainerFrame.showSkillDetails`, and `ClassTrainer_SetSelection` early-returns while that is
/// nil — so on `TRAINER_UPDATE` only the `GetTrainerSelectionIndex() > 1` **else** branch,
/// `ClassTrainer_SelectFirstLearnableSkill`, puts the flag back and rewrites the name, icon,
/// `Requires:` line and cost. Answering with a *live, visible* row number for a service that has
/// left the screen therefore takes the silent branch and leaves the pane describing the spell the
/// player just learned, under a highlight sitting on the row that slid up into its place — the
/// director's report.
///
/// The reference's own answer there is the hidden row's index, and its consequence is worth stating
/// because it is what a player sees: `> 1` is true, so the window resets the scroll, calls
/// `ClassTrainer_SetSelection` on a row nothing displays, and the update loop — which asks only
/// whether the selection is *on screen* — finds it is not and hides the detail pane. **After you
/// train a spell the reference blanks the pane and highlights nothing**; it does not advance to the
/// next spell. Answering `0` instead would select the first learnable and repaint, which is the
/// fresh-window behaviour, not the post-purchase one.
fn selected_row(model: &Model) -> Option<usize> {
    let want = model.trainer_selection?;
    let t = model.trainer.as_ref()?;
    rows(model)
        .0
        .iter()
        .position(|r| match r {
            Row::Service(si) => t.services.get(*si).is_some_and(|s| s.spell_id == want),
            Row::Header(_) => false,
        })?
        .checked_add(1)
}

/// Collapse (`collapse = true`) or expand a skill line by the **display index of its header row**
/// (decision 0247's `Collapse/ExpandTrainerSkillLine`). `id == 0` targets **all** groups (the
/// collapse-all button); `id > 0` resolves the header at that index to its skill line. A non-header
/// (or out-of-range) index is a no-op.
///
/// Like every other index in this surface it runs over the whole row array, so a header sitting in
/// the hidden tail resolves too ([`rows`]). That is the shape of the accessor gate the service
/// getters were carved to share (`0x4d89b0`, bounded by the total); these two bindings' own gate was
/// **not** carved, and the difference is unobservable through the stock window, which only ever
/// passes a visible header's index or `0`.
/// Queue `TRAINER_UPDATE` — **the repaint the reference fires from the mask-commit thunk itself**,
/// not from Lua (decision 2244).
///
/// `SetTrainerServiceTypeFilter`'s four legs all commit through `0x4d8c90`, whose whole body is
/// `mov ds:0xb73a1c,ecx; call 0x4d8410; mov ecx,0x136; jmp 0x703e50` — write the mask, re-run the
/// finalizer, **fire event `0x136` = `TRAINER_UPDATE`**. Its siblings `0x4d8cb0` (the skill-line
/// mask) and `0x4d8cd0` (the expand mask, which is what Collapse/ExpandTrainerSkillLine commit)
/// follow `0x4d8c90`'s same shape.
///
/// That matters because **nothing in the stock window repaints after a filter click**:
/// `ClassTrainerFrameFilterDropDown_OnClick` sets the saved global, calls the filter verb, and then
/// only does `ClassTrainerListScrollFrameScrollBar:SetValue(0)` — a no-op when the bar is already at
/// zero. The list is repainted by `ClassTrainerFrame_OnEvent`'s `TRAINER_UPDATE` arm, and by nothing
/// else. While our own `TrainerFrame.xml` ran (before 1957) its click handler repainted explicitly,
/// which is why this only became visible when the window went stock: the checkboxes moved and the
/// list underneath did not.
fn queue_trainer_update(model: &mut Model) {
    model
        .pending_events
        .push(("TRAINER_UPDATE".to_string(), Vec::new()));
}

fn set_collapsed(model: &mut Model, id: usize, collapse: bool) {
    if id == 0 && !collapse {
        model.trainer_collapsed.clear();
        return;
    }
    let targets: Vec<u32> = if id == 0 {
        model
            .trainer
            .as_ref()
            .map(|t| t.groups.iter().map(|g| g.key).collect())
            .unwrap_or_default()
    } else {
        match rows(model).0.get(id - 1) {
            Some(Row::Header(gi)) => model
                .trainer
                .as_ref()
                .and_then(|t| t.groups.get(*gi))
                .map(|g| g.key)
                .into_iter()
                .collect(),
            _ => Vec::new(),
        }
    };
    for sl in targets {
        if collapse {
            model.trainer_collapsed.insert(sl);
        } else {
            model.trainer_collapsed.remove(&sl);
        }
    }
}

/// An optional string as a Lua value (`nil` when absent).
fn opt_str(lua: &Lua, s: Option<&String>) -> mlua::Result<Value> {
    Ok(match s {
        Some(s) => Value::String(lua.create_string(s)?),
        None => Value::Nil,
    })
}

/// A `bool` as the Era `1`/`nil` shape.
fn era_bool(b: bool) -> Value {
    if b {
        Value::Integer(1)
    } else {
        Value::Nil
    }
}

/// Register the trainer globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // → the number of services the open trainer offers under the current filter (0 when closed).
    g.set(
        "GetNumTrainerServices",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(num_services(&model) as i64)
        })?,
    )?;

    // GetTrainerServiceInfo(index) → name, subText, serviceType, isExpanded. `index` 1-based into the
    // visible tree; out of range → a single nil. A HEADER row returns (skillLineName, nil, "header",
    // isExpanded); a SERVICE row returns (name, subText, stateString, nil) (decision 0247).
    g.set(
        "GetTrainerServiceInfo",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(n) = index.checked_sub(1) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let Some(row) = rows(&model).0.get(n).copied() else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let t = model
                .trainer
                .as_ref()
                .expect("a resolved row ⇒ an open trainer");
            match row {
                Row::Header(gi) => {
                    let g = &t.groups[gi];
                    let expanded = !model.trainer_collapsed.contains(&g.key);
                    Ok(MultiValue::from_vec(vec![
                        Value::String(lua.create_string(&g.name)?),
                        Value::Nil,
                        Value::String(lua.create_string("header")?),
                        era_bool(expanded),
                    ]))
                }
                Row::Service(si) => {
                    let s = &t.services[si];
                    Ok(MultiValue::from_vec(vec![
                        opt_str(lua, s.name.as_ref())?,
                        opt_str(lua, s.subtext.as_ref())?,
                        Value::String(lua.create_string(s.category.era_str())?),
                        Value::Nil,
                    ]))
                }
            }
        })?,
    )?;

    // GetTrainerServiceSkillLine(index) → the skill line the service teaches into, by name — the
    // group the tree files it under (`TrainerService::group_name`: `SkillLine.dbc`'s name at
    // trainer types 0/1/3, 1124's builder law). The stock window's one caller is the
    // CONFIRM_PROFESSION dialog, which formats "learn <profession>?" with it
    // (Blizzard_TrainerUI.lua l.19-24). A header row or an out-of-range index answers nil. The
    // binding is registered (`0x4d9160`, 399 bytes) but its return law is not confirmed beyond
    // "delegates"; this is the call site's reading (1957).
    g.set(
        "GetTrainerServiceSkillLine",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(n) = index.checked_sub(1) else {
                return Ok(Value::Nil);
            };
            let Some(Row::Service(si)) = rows(&model).0.get(n).copied() else {
                return Ok(Value::Nil);
            };
            let t = model
                .trainer
                .as_ref()
                .expect("a resolved row ⇒ an open trainer");
            Ok(Value::String(
                lua.create_string(&t.services[si].group_name)?,
            ))
        })?,
    )?;

    // GetTrainerServiceIcon(index) → texture path (nil while in flight / OOB).
    g.set(
        "GetTrainerServiceIcon",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            opt_str(lua, service(&model, index).and_then(|s| s.texture.as_ref()))
        })?,
    )?;

    // GetTrainerServiceDescription(index) → the spell description ("" when the app has none / OOB).
    g.set(
        "GetTrainerServiceDescription",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let text = service(&model, index)
                .map(|s| s.description.clone())
                .unwrap_or_default();
            lua.create_string(&text)
        })?,
    )?;

    // GetTrainerServiceCost(index) → money, talentPointCost, professionPointCost. Talent cost is
    // always 0 (no talent trainer); profession cost is 1 for a primary-profession first rank.
    g.set(
        "GetTrainerServiceCost",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (money, prof) = service(&model, index)
                .map(|s| (s.cost, u32::from(s.prof_first_rank)))
                .unwrap_or((0, 0));
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(money)),
                Value::Integer(0),
                Value::Integer(i64::from(prof)),
            ]))
        })?,
    )?;

    // GetTrainerServiceLevelReq(index) → required character level (0 = none).
    g.set(
        "GetTrainerServiceLevelReq",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(service(&model, index).map_or(0, |s| s.level_req) as i64)
        })?,
    )?;

    // GetTrainerServiceSkillReq(index) → skillName, skillRank, hasReq (nil when no skill gate / OOB).
    g.set(
        "GetTrainerServiceSkillReq",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(req) = service(&model, index).and_then(|s| s.skill_req.as_ref()) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&req.name)?),
                Value::Integer(i64::from(req.rank)),
                era_bool(req.met),
            ]))
        })?,
    )?;

    // GetTrainerServiceNumAbilityReq(index) → how many prerequisite abilities the service lists.
    g.set(
        "GetTrainerServiceNumAbilityReq",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(service(&model, index).map_or(0, |s| s.ability_reqs.len()) as i64)
        })?,
    )?;

    // GetTrainerServiceAbilityReq(index, i) → abilityName, hasReq (nil OOB).
    g.set(
        "GetTrainerServiceAbilityReq",
        lua.create_function(|lua, (index, i): (usize, usize)| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(req) = service(&model, index)
                .and_then(|s| i.checked_sub(1).and_then(|n| s.ability_reqs.get(n)))
            else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&req.name)?),
                era_bool(req.met),
            ]))
        })?,
    )?;

    // GetTrainerServiceStepReq(index) → step, met — always nil (no tradeskill-step data on the wire).
    g.set(
        "GetTrainerServiceStepReq",
        lua.create_function(|_, _index: usize| Ok(Value::Nil))?,
    )?;

    // IsTrainerServiceTradeSkill(index) → 1/nil (a tradeskill step vs. a learn-spell).
    g.set(
        "IsTrainerServiceTradeSkill",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(era_bool(
                service(&model, index).is_some_and(|s| s.is_trade_skill),
            ))
        })?,
    )?;

    // IsTrainerServiceLearnSpell(index) → isLearnSpell, isPetLearnSpell. Pet-learn is always nil
    // (benilla drives no pet trainer); a non-tradeskill service is a learn-spell.
    g.set(
        "IsTrainerServiceLearnSpell",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let learn = service(&model, index).is_some_and(|s| !s.is_trade_skill);
            Ok(MultiValue::from_vec(vec![era_bool(learn), Value::Nil]))
        })?,
    )?;

    // IsTradeskillTrainer() → 1/nil for the whole trainer (drives the tradeskill-vs-class layout):
    // `0x4d8ea0` tests trainerType == 2.
    g.set(
        "IsTradeskillTrainer",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(era_bool(model.trainer.as_ref().is_some_and(|t| {
                t.trainer_type == TRAINER_TYPE_TRADESKILL
            })))
        })?,
    )?;

    // IsTalentTrainer() → 1/nil: `0x4d8ed0` tests trainerType == 1 — vmangos's MOUNT trainers, which
    // the client's own vocabulary calls "talent" (decision 1124). It used to return a hardcoded nil
    // on the belief that benilla drives no such trainer; the shipped world has 23 of them.
    g.set(
        "IsTalentTrainer",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(era_bool(
                model.trainer.as_ref().is_some_and(|t| t.trainer_type == 1),
            ))
        })?,
    )?;

    // GetTrainerGreetingText() → the trainer's greeting line ("" when no trainer is open).
    g.set(
        "GetTrainerGreetingText",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let text = model
                .trainer
                .as_ref()
                .map(|t| t.greeting.clone())
                .unwrap_or_default();
            lua.create_string(&text)
        })?,
    )?;

    // GetTrainerServiceTypeFilter(type) → 1/nil (whether that category is shown). SetTrainerService-
    // TypeFilter(type, on) toggles it — the sort checkboxes' real client-side filtering.
    g.set(
        "GetTrainerServiceTypeFilter",
        lua.create_function(|lua, kind: String| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match TrainerServiceCategory::from_filter_str(&kind) {
                Some(c) => era_bool(model.trainer_filter[c.filter_slot()]),
                None => Value::Nil,
            })
        })?,
    )?;
    g.set(
        "SetTrainerServiceTypeFilter",
        lua.create_function(|lua, (kind, on): (String, Value)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(c) = TrainerServiceCategory::from_filter_str(&kind) {
                // Era passes 1 / 0 (or true/nil); anything truthy-but-not-0 enables.
                let enable = !matches!(on, Value::Nil | Value::Integer(0) | Value::Boolean(false));
                model.trainer_filter[c.filter_slot()] = enable;
                // …and the repaint, from here rather than from Lua ([`queue_trainer_update`]).
                // Unconditional, like the thunk: it fires whether or not the bit moved.
                queue_trainer_update(&mut model);
            }
            Ok(())
        })?,
    )?;

    // Collapse/ExpandTrainerSkillLine(id) — fold a skill line by the display index of its header row
    // (id 0 = all groups, the collapse-all button); a non-header index no-ops (decision 0247).
    g.set(
        "CollapseTrainerSkillLine",
        lua.create_function(|lua, id: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            set_collapsed(&mut model, id, true);
            queue_trainer_update(&mut model);
            Ok(())
        })?,
    )?;
    g.set(
        "ExpandTrainerSkillLine",
        lua.create_function(|lua, id: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            set_collapsed(&mut model, id, false);
            queue_trainer_update(&mut model);
            Ok(())
        })?,
    )?;

    // SelectTrainerService(index) — select the SERVICE at that row, by its spell id. An index out
    // of range clears the selection (`0x4d74f0`'s `cmp ecx, ds:0xb73a10` / `mov [0xb73a0c], 0` —
    // cleared, never clamped); [`selected_row`] is why the id and not the row number.
    //
    // A HEADER row clears here too, where the reference would store the header record's own id,
    // `0xffffffff` (`0x4d7b05`), and find the FIRST header again on the way back out — answering 1
    // rather than 0. The two are the same answer to the only question the stock window asks of this
    // number (`GetTrainerSelectionIndex() > 1`), and nothing else reads it.
    g.set(
        "SelectTrainerService",
        lua.create_function(|lua, index: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.trainer_selection = service(&model, index).map(|s| s.spell_id);
            Ok(())
        })?,
    )?;

    // GetTrainerSelectionIndex() → the selected service's current 1-based row — **past
    // `GetNumTrainerServices()` while that service is hidden**, and 0 only when nothing is selected
    // or the service has left the list entirely ([`selected_row`] has the law and what rides on it).
    g.set(
        "GetTrainerSelectionIndex",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(selected_row(&model).map_or(0i64, |r| r as i64))
        })?,
    )?;

    // BuyTrainerService(index) — queue that row's SPELL ID for purchase (the app sends
    // CMSG_TRAINER_BUY_SPELL). A header row, an out-of-range index, or an ALREADY-KNOWN row is
    // silently ignored: the single-row path `0x4d89d0` resolves through the same total-bounded
    // accessor gate as every getter, then refuses on the state byte — `0x4d89e1 cmp esi,ebx; je`
    // (no record) and `0x4d89e5 cmp byte ptr [esi+0x30], bl; jne` (state != 0, i.e. anything but
    // `available`), both jumping to the same do-nothing exit.
    //
    // Deliberately NOT built: the reference's buy-ALL convenience `0x4d8a70`, the one
    // visible-count-bounded loop in the whole surface (`0x4d8a71`/`0x4d8a88` read `ds:0xb73a18`).
    // The split `0x4da244 dec eax; 0x4da245 jns` is SIGNED, so it is every Lua index **<= 0** that
    // lands there — `BuyTrainerService(0)` buys the player's entire visible list in one call. Here
    // index 0 is a no-op instead. Nothing in the stock window passes a non-positive index, and a
    // verb that spends the whole purse on a typo is not one to reproduce on the strength of that.
    g.set(
        "BuyTrainerService",
        lua.create_function(|lua, index: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let buyable = service(&model, index)
                .filter(|s| s.category == TrainerServiceCategory::Available)
                .map(|s| s.spell_id);
            if let Some(spell_id) = buyable {
                model.trainer_buys.push(spell_id);
            }
            Ok(())
        })?,
    )?;

    // CloseTrainer() — client-side close (no packet, vanilla): flag it so the app clears its state.
    g.set(
        "CloseTrainer",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.trainer_close = true;
            Ok(())
        })?,
    )?;

    Ok(())
}
#[cfg(test)]
mod tests;
