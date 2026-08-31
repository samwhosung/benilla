//! The app-side **quest-log state + feed** — decision 0088's deferred second slice (the giver-panel
//! decision named the log window, `SMSG_QUEST_QUERY_RESPONSE`, and the `PLAYER_QUEST_LOG` field
//! accessors as a follow-up; the wire is now pinned in `quest-log-wire-pin.md`, and this is the app
//! layer over it). The inward half of the seam around [`benilla_ui::script::quest_log`], the log's
//! twin of [`crate::ui_quest`]'s questgiver-panel feed.
//!
//! Unlike the questgiver panels (a transient NPC-session window), the quest log is **durable
//! player state**: [`feed_quest_log`] reads the self player's `PLAYER_QUEST_LOG` descriptor slots
//! every frame (the wire pin's field 198 + 3·slot) rather than reacting to one wire event, and
//! resolves EVERY occupied slot's title/level/objectives from the `SMSG_QUEST_QUERY_RESPONSE`
//! template cache ([`QuestLog`] — the exact ask-once twin of [`crate::items::Items`]'s template
//! cache): [`build_objectives`] turns the template's fixed objective quads + the slot's counters
//! (creature/GO objectives) or the live bag count (item objectives; the wire pin: item-objective
//! progress is *not* one of the slot's 6-bit counters, the client counts bags itself) into every
//! entry's [`QuestLogEntryView::objectives`] — not just the selection's, since the Era API answers
//! `GetNumQuestLeaderBoards`/`GetQuestLogLeaderBoard` for ANY index (the watch tracker HUD reads
//! non-selected quests' lines). [`build_detail`] resolves only the selection's description/money/
//! rewards/choices. The snapshot pushes via `set_quest_log` + fires `QUEST_LOG_UPDATE` — plus,
//! per quest whose objectives moved ([`quests_with_progressed_objectives`]), the progress-toast
//! `UI_INFO_MESSAGE`s (each moved leaderboard line's fresh text — the yellow top-center popup —
//! and the COMPLETE flip's "%s (Complete)"), the native `QUEST_WATCH_UPDATE(watchIndex)` (the
//! §5-verified byte arg), and `BENILLA_QUEST_PROGRESS(logIndex)` (the auto-watch's feed — the
//! divergence note at the fire site / decision 0340) — diffed against a `Local`, exactly like
//! every other feed in this crate.
//!
//! [`drain_quest_log_abandons`] maps the confirmed abandon's 1-based Lua entry index back to its
//! descriptor slot (this frame's push order, kept on [`QuestLog`]) and sends
//! `CMSG_QUESTLOG_REMOVE_QUEST`.
//!
//! This module used to also answer the questgiver greeting's active/available split, via a
//! `contains` membership test over the live descriptor slots. **It doesn't any more, and must not
//! again** — the reference splits that list on the wire icon alone and never consults its quest log
//! there ([`crate::ui_quest::row_is_active`], decision 0758). Membership was wrong for exactly the
//! quests that are never in the log: auto-complete turn-ins. The state that backed it is gone
//! rather than left dead, so the retired law can't be reached for by accident.

use crate::ui_items::{count_of, InventoryScope};
use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use benilla_protocol::messages::{
    quest_slot_state, QuestLogSlot, QuestObjective, QuestTemplate, PLAYER_QUEST_LOG_SLOTS,
};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{
    QuestItemView, QuestLogDetail, QuestLogEntryView, QuestLogObjectiveView, QuestLogState,
    ScriptValue, UiScript,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{
    ClientCommand, Guid, NetCommands, ObjectStore, Proficiencies, Reputations, SelfPlayer,
};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_items::feed::{item_usable_for_player, player_req_state};
use crate::ui_script::UiInput;
use crate::ui_unit::UnitFeed;

/// The top bit `QuestObjective::creature_or_go` carries for a gameobject objective —
/// `(−id) | 0x8000_0000` (vmangos `Quest.cpp:512-516`; the wire pin's finding, same raw shape
/// `SMSG_QUESTUPDATE_ADD_KILL`'s `entry` carries for a GO kill/use).
const GO_OBJECTIVE_BIT: u32 = 0x8000_0000;

/// One occupied `PLAYER_QUEST_LOG` slot this frame — the feed's working row before it's turned into
/// a pushed [`QuestLogEntryView`].
struct Row {
    slot: u8,
    quest_id: u32,
    log_slot: QuestLogSlot,
}

/// The quest-log state cache + per-frame bookkeeping.
///
/// - `templates`/`pending` — the `SMSG_QUEST_QUERY_RESPONSE` cache, ask-once by quest id (the exact
///   twin of [`Items`]'s item-template cache).
/// - `entry_slots` — this frame's pushed entry order → descriptor slot, so
///   [`drain_quest_log_abandons`] can turn a confirmed abandon's 1-based Lua index back into the
///   `CMSG_QUESTLOG_REMOVE_QUEST` slot it came from (slots aren't contiguous — an abandoned/turned-in
///   quest leaves a gap).
#[derive(Resource, Default)]
pub(crate) struct QuestLog {
    templates: HashMap<u32, QuestTemplate>,
    pending: HashSet<u32>,
    entry_slots: Vec<Option<u8>>,
    /// Collapsed section headers, keyed by header TITLE (two zones sharing a name share a header
    /// row, so the fold state naturally shares too). Owned here — the engine only reports the
    /// flag and drains toggle intents ([`drain_quest_log_collapses`]).
    collapsed: HashSet<String>,
    /// This frame's pushed entry order → the header title for header rows (`None` for quests) —
    /// the collapse drain's index→identity map, the fold twin of `entry_slots`.
    header_keys: Vec<Option<String>>,
}

impl QuestLog {
    /// The template for `quest_id`, if known — ask-once: a miss sends `CMSG_QUEST_QUERY` (deduped
    /// while in flight) and returns `None`; a cached negative (the server doesn't know the id) is
    /// also `None`, without a re-ask. The exact twin of [`Items::template`].
    pub(crate) fn template(
        &mut self,
        quest_id: u32,
        commands: &NetCommands,
    ) -> Option<&QuestTemplate> {
        if !self.templates.contains_key(&quest_id) {
            if self.pending.insert(quest_id) {
                debug!("ui_quest_log: asking quest template (quest {quest_id})");
                let _ = commands
                    .0
                    .send(ClientCommand::QuestQuery { quest: quest_id });
            }
            return None;
        }
        self.templates.get(&quest_id)
    }

    /// Record a template answer (`SMSG_QUEST_QUERY_RESPONSE`).
    pub(crate) fn insert_template(&mut self, template: QuestTemplate) {
        self.pending.remove(&template.quest_id);
        self.templates.insert(template.quest_id, template);
    }

    /// Disconnect: drop everything, including the templates. Unlike item templates (which persist —
    /// decision: static, stable across sessions), a stale log slice carried across a reconnect would
    /// pair fresh descriptor slots (possibly renumbered) with old titles; templates are cheap enough
    /// to re-ask that there's no reason to risk it.
    pub(crate) fn clear_session(&mut self) {
        self.templates.clear();
        self.pending.clear();
        self.entry_slots.clear();
        self.collapsed.clear();
        self.header_keys.clear();
    }
}

pub(crate) struct UiQuestLogPlugin;

impl Plugin for UiQuestLogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<QuestLog>()
            .add_systems(
                Startup,
                load_quest_header_names.after(benilla_assets::AssetSet::Open),
            )
            .add_systems(
                Update,
                (
                    feed_quest_log.in_set(UnitFeed).before(UiInput),
                    // Before the script tick, so a QuestTimerFrame OnUpdate this frame reads this
                    // frame's clock (the `minimap::feed_game_time` shape).
                    feed_server_clock.before(UiInput),
                    drain_quest_log_abandons.after(UiInput),
                    drain_quest_log_collapses.after(UiInput),
                ),
            );
    }
}

/// One leaderboard line for a template objective slot — `None` when the slot carries no content (an
/// unused quad; `SMSG_QUEST_QUERY_RESPONSE`'s objective array is a fixed 4 regardless of how many
/// the quest actually uses — the wire pin). Pure: the caller has already resolved the creature/item
/// name (ask-once — `None` while the answer is in flight) and the live progress (`counter` — the
/// descriptor slot's 6-bit counter, meaningful only for a creature/GO objective; `bag_count` — the
/// live backpack+bags count, meaningful only for an item objective — [`crate::ui_items::count_of`]).
///
/// The custom `text` field, when non-empty, REPLACES the auto-generated name in the line but keeps
/// the "cur/req" suffix — CONFIRMED since (wow-re `ui/scratch/quest-leaderboard-law.md`: a
/// creature objective carrying `ObjectiveText[i]` formats with `QUEST_OBJECTS_FOUND "%s: %d/%d"`,
/// not the "slain" key, and that text IS the name — while `type` still answers `"monster"`).
///
/// **Known-wrong (decision 1156, not yet corrected):** `finished` must be `cur >= req` and NOTHING
/// else. The whole-quest COMPLETE bit is read exactly once in the reference binding, by the `event`
/// line, where it is that line's only predicate (`0x4e02a2`); or-ing it in here marks every
/// objective finished the moment the quest completes. The fix must land WITH [`count_of`]'s scope
/// widening (1156) — alone, it would render a banked-item quest's lines unfinished under a
/// COMPLETE quest.
fn objective_line(
    obj: &QuestObjective,
    counter: u8,
    creature_name: Option<&str>,
    item_name: Option<&str>,
    bag_count: u32,
) -> Option<QuestLogObjectiveView> {
    const PLACEHOLDER: &str = "...";
    if obj.creature_or_go != 0 && obj.required_count > 0 {
        let req = obj.required_count;
        let is_go = obj.creature_or_go & GO_OBJECTIVE_BIT != 0;
        let cur = u32::from(counter).min(req);
        let kind = if is_go { "object" } else { "monster" };
        let text = if !obj.text.is_empty() {
            format!("{}: {cur}/{req}", obj.text)
        } else if is_go {
            // No gameobject-name cache yet — a later CMSG_GAMEOBJECT_QUERY slice.
            format!("Objective: {cur}/{req}")
        } else {
            format!(
                "{} slain: {cur}/{req}",
                creature_name.unwrap_or(PLACEHOLDER)
            )
        };
        return Some(QuestLogObjectiveView {
            text,
            kind: kind.into(),
            finished: cur >= req,
            cur,
            req,
        });
    }
    if obj.item_id != 0 && obj.item_count > 0 {
        let req = obj.item_count;
        let cur = bag_count.min(req);
        let text = if !obj.text.is_empty() {
            format!("{}: {cur}/{req}", obj.text)
        } else {
            format!("{}: {cur}/{req}", item_name.unwrap_or(PLACEHOLDER))
        };
        return Some(QuestLogObjectiveView {
            text,
            kind: "item".into(),
            finished: cur >= req,
            cur,
            req,
        });
    }
    None
}

/// Split a `SMSG_QUEST_QUERY_RESPONSE` `money` value into (required, reward) — negative demands
/// money at turn-in, non-negative grants it (`QuestLogDetail::required_money`/`reward_money`).
fn money_split(money: i32) -> (u32, u32) {
    if money < 0 {
        ((-money) as u32, 0)
    } else {
        (0, money as u32)
    }
}

/// Resolve one template reward/choice `(itemId, count)` pair into a Lua-facing [`QuestItemView`] —
/// the quest-log twin of [`crate::ui_quest::resolve_item`]. The icon path differs from the
/// questgiver panels: `SMSG_QUEST_QUERY_RESPONSE`'s reward arrays carry **no display id** (the wire
/// pin), so the icon comes from the item template's `display_info_id` instead — the same
/// `ItemDisplayInfo.dbc` catalog lookup a bag slot uses ([`crate::ui_items::resolve_slot`]).
fn resolve_template_item(
    entry: u32,
    count: u32,
    items: &mut Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    player_req: &benilla_ui::script::PlayerReqState,
    actions: &PlayerActions,
) -> QuestItemView {
    let template = items.template(entry, 0, commands).cloned();
    let name = template.as_ref().map(|t| t.name.clone());
    let quality = template.as_ref().map(|t| t.quality).unwrap_or(1);
    let usable = template
        .as_ref()
        .is_none_or(|t| item_usable_for_player(t, player_req, actions));
    let texture = template.as_ref().and_then(|t| {
        icons
            .and_then(|i| i.catalog.get(t.display_info_id))
            .and_then(|d| d.icon.clone())
    });
    // `GetQuestLogItemLink`'s payload (decisions 1059/1060) — the same shared link builder, and the
    // same in-flight `None`, as the questgiver panels' [`crate::ui_quest::resolve_item`].
    let link = name
        .as_ref()
        .map(|n| crate::ui_items::item_link(entry, n, quality));
    QuestItemView {
        name,
        texture,
        count,
        quality,
        item_id: entry,
        usable,
        link,
    }
}

/// Build one occupied slot's objective ("leaderboard") lines from its template + this frame's
/// descriptor slot counters/bag counts — every entry gets these (`GetNumQuestLeaderBoards`/
/// `GetQuestLogLeaderBoard` answer ANY index, not just the selection: the watch tracker HUD reads
/// non-selected quests' lines, ref `QuestLogFrame.lua:613-663`). Pulled out of [`build_detail`]
/// (which used to build this only for the selection) so the per-frame loop below can call it once
/// per occupied slot.
#[allow(clippy::too_many_arguments)]
fn build_objectives(
    template: &QuestTemplate,
    log_slot: &QuestLogSlot,
    store: &ObjectFields,
    items: &mut Items,
    names: &mut NameCache,
    commands: &NetCommands,
) -> Vec<QuestLogObjectiveView> {
    let mut objectives = Vec::with_capacity(template.objectives.len());
    for (i, obj) in template.objectives.iter().enumerate() {
        let is_go = obj.creature_or_go & GO_OBJECTIVE_BIT != 0;
        let creature_name = (obj.creature_or_go != 0 && !is_go)
            .then(|| {
                names
                    .resolve_creature(obj.creature_or_go, 0, commands)
                    .map(str::to_string)
            })
            .flatten();
        let item_name = (obj.item_id != 0)
            .then(|| {
                items
                    .template(obj.item_id, 0, commands)
                    .map(|t| t.name.clone())
            })
            .flatten();
        let bag_count = if obj.item_id != 0 {
            count_of(store, items, obj.item_id, InventoryScope::QUEST_ITEMS)
        } else {
            0
        };
        if let Some(line) = objective_line(
            obj,
            log_slot.counters[i],
            creature_name.as_deref(),
            item_name.as_deref(),
            bag_count,
        ) {
            objectives.push(line);
        }
    }
    objectives
}

/// Build the selected quest's detail pane from its template — description/money/rewards/choices
/// only (the objective lines live on every [`QuestLogEntryView::objectives`] now, selection or
/// not — see [`build_objectives`]), so this no longer needs the descriptor slot at all.
fn build_detail(
    template: &QuestTemplate,
    items: &mut Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    macros: &crate::npc_text::MacroContext,
    player_req: &benilla_ui::script::PlayerReqState,
    actions: &PlayerActions,
) -> QuestLogDetail {
    let (required_money, reward_money) = money_split(template.money);
    let rewards = template
        .rewards
        .iter()
        .filter(|&&(id, _)| id != 0)
        .map(|&(id, count)| {
            resolve_template_item(id, count, items, icons, commands, player_req, actions)
        })
        .collect();
    let choices = template
        .choices
        .iter()
        .filter(|&&(id, _)| id != 0)
        .map(|&(id, count)| {
            resolve_template_item(id, count, items, icons, commands, player_req, actions)
        })
        .collect();

    QuestLogDetail {
        // The wire delivers quest text with its chat macros ($N/$B/$G/$<n>w) un-expanded; the
        // client substitutes (crate::npc_text — the 0109 look fix's shared mechanism).
        description: crate::npc_text::substitute(&template.details, macros),
        objectives_text: crate::npc_text::substitute(&template.objectives_text, macros),
        required_money,
        reward_money,
        choices,
        rewards,
    }
}

/// Map a 1-based Lua entry index to its descriptor slot via this frame's push order (`entry_slots`)
/// — the abandon drain's half of the flow; `None` for a header row (no slot to remove). Pure, so
/// it's testable without a live [`QuestLog`]/ECS.
fn abandon_slot(entry: u32, entry_slots: &[Option<u8>]) -> Option<u8> {
    (entry as usize)
        .checked_sub(1)
        .and_then(|i| entry_slots.get(i))
        .copied()
        .flatten()
}

/// Re-point the engine's quest-log selection (a 1-based entry INDEX) across a snapshot rebuild —
/// headers/folds shift indexes between pushes, so the selection follows the SAME quest to its new
/// position. A header is NEVER a valid selection (ref `QuestLog_SetSelection`'s header branch only
/// folds, and `QuestLog_GetFirstSelectableQuest` skips headers), so a selection that no longer
/// lands on a visible quest — folded away, abandoned, or resting on a header — resets to 0, and
/// the Lua update's transcribed `SetFirstValidSelection` picks the first visible quest, exactly
/// the ref's maintenance loop (`QuestLog_Update`, ref l.293-296). 0 in, 0 out.
fn remap_selection(old: &[QuestLogEntryView], new: &[QuestLogEntryView], sel: u32) -> u32 {
    let Some(prev) = (sel as usize)
        .checked_sub(1)
        .and_then(|i| old.get(i))
        .filter(|e| !e.is_header)
    else {
        return 0;
    };
    new.iter()
        .position(|e| !e.is_header && e.quest_id == prev.quest_id)
        .map(|i| i as u32 + 1)
        .unwrap_or(0)
}

/// Push the server's wall clock into the VM every frame (decision 1150) — the number every
/// countdown binding subtracts against.
///
/// Every frame, not on change: it is a *clock*, so "changed" is always, and the whole point is that
/// `GetQuestTimers()` answers a fresh value on each of the reference `QuestTimerFrame`'s OnUpdate
/// calls. Cheap by construction — one scalar write, no event, no diff, nothing downstream unless
/// some frame actually reads it.
///
/// Silent before the first `SMSG_QUERY_TIME_RESPONSE` (the world-enter query answers within a round
/// trip). The bindings then report *no* timer rather than a guessed one — see
/// `benilla_ui::script::quest_log::seconds_left`.
fn feed_server_clock(
    script: Option<NonSendMut<UiScript>>,
    clock: Res<crate::net::ServerWallClock>,
) {
    let (Some(mut script), Some(now)) = (script, clock.now_unix()) else {
        return;
    };
    script.set_server_unix_time(now);
}

/// Read the self player's `PLAYER_QUEST_LOG` descriptor slots each frame, resolve entries/detail,
/// and push a [`QuestLogState`] snapshot on change (diffed against a `Local`, the crate's standard
/// feed shape). Also refreshes [`QuestLog::active_quest_ids`]/`entry_slots` for the greeting split
/// and the abandon drain.
#[allow(clippy::too_many_arguments)]
fn feed_quest_log(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    mut quest_log: ResMut<QuestLog>,
    mut names: ResMut<NameCache>,
    mut items: ResMut<Items>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    proficiencies: Res<Proficiencies>,
    reputations: Res<Reputations>,
    factions: Option<Res<crate::target::Factions>>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    header_names: Option<Res<QuestHeaderNamesRes>>,
    states: Res<crate::world_state::WorldStates>,
    mut last: Local<crate::ui_script::VmMemo<QuestLogState>>,
    mut prior_quest_ids: Local<crate::ui_script::VmMemo<Option<HashSet<u32>>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let prior_quest_ids = prior_quest_ids.get(&script);
    let Some((store, _)) = self_q.iter().next() else {
        // No self player streamed yet — nothing to show; `last` stays at its (empty) default, so
        // this is a no-op rather than a repeated empty push.
        return;
    };
    let player_req = player_req_state(
        store,
        &proficiencies,
        &reputations,
        factions.as_deref(),
        &actions,
        spells.as_deref(),
    );

    let mut rows = Vec::new();
    for slot in 0..PLAYER_QUEST_LOG_SLOTS {
        let Some(log_slot) = store.0.player_quest_log(slot) else {
            continue;
        };
        if log_slot.quest_id == 0 {
            continue; // an explicitly-cleared slot (abandon/turn-in) — not a row
        }
        rows.push(Row {
            slot,
            quest_id: log_slot.quest_id,
            log_slot,
        });
    }

    // The accept chord (QUESTADDED → iQuestActivate.wav): the client's C++ plays it when a quest
    // ENTERS the log — no Lua handler owns it. Diffed against the previous frame's occupied ids;
    // the FIRST snapshot after login is silent (the initial object-create floods the whole log —
    // `prior` = None until one real snapshot exists).
    {
        let current: HashSet<u32> = rows.iter().map(|r| r.quest_id).collect();
        if let Some(prior) = prior_quest_ids.as_ref() {
            if current.iter().any(|id| !prior.contains(id)) {
                script.queue_sound_kit("QUESTADDED");
            }
        }
        *prior_quest_ids = Some(current);
    }

    // ── Section headers (the ref's zone/sort groups) ────────────────────────────────────────────
    // Group rows by their template's ZoneOrSort NAME (positive → AreaTable zone, negative →
    // QuestSort sort — [`benilla_formats::QuestHeaderNames`]); an unknown/0/in-flight id falls
    // into a shared "Quests" bucket until the template lands. Headers sort alphabetically and
    // quests keep descriptor-slot order within their group — INTERIM: the real client's exact
    // group/quest ordering (CQuestLog sort) is unpinned; a wow-re follow-up owns it.
    const FALLBACK_HEADER: &str = "Quests";
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let zos = quest_log
            .template(r.quest_id, &commands)
            .map(|t| t.zone_or_sort)
            .unwrap_or(0);
        let name = header_names
            .as_ref()
            .and_then(|h| h.0.resolve(zos))
            .unwrap_or(FALLBACK_HEADER)
            .to_string();
        match groups.iter_mut().find(|(n, _)| *n == name) {
            Some((_, idxs)) => idxs.push(i),
            None => groups.push((name, vec![i])),
        }
    }
    groups.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut entries: Vec<QuestLogEntryView> = Vec::new();
    let mut entry_slots: Vec<Option<u8>> = Vec::new();
    let mut header_keys: Vec<Option<String>> = Vec::new();
    for (name, row_idxs) in &groups {
        let collapsed = quest_log.collapsed.contains(name);
        entries.push(QuestLogEntryView {
            quest_id: 0,
            title: name.clone(),
            level: 0,
            tag: None,
            is_header: true,
            collapsed,
            complete: 0,
            timer: 0, // a header row is not a quest and never carries a timer
            objectives: Vec::new(),
        });
        entry_slots.push(None);
        header_keys.push(Some(name.clone()));
        for &ri in row_idxs {
            let r = &rows[ri];
            if collapsed {
                continue; // folded: the quest stays in the log, just not in the visible list
            }
            let (title, level, objectives) = match quest_log.template(r.quest_id, &commands) {
                Some(t) => (
                    t.title.clone(),
                    t.level,
                    build_objectives(t, &r.log_slot, &store.0, &mut items, &mut names, &commands),
                ),
                // Title/level/objectives placeholder while the template is in flight — the
                // QUEST_LOG_UPDATE refresh on landing fills the row in.
                None => ("...".to_string(), 0, Vec::new()),
            };
            let complete = if r.log_slot.state & quest_slot_state::COMPLETE != 0 {
                1
            } else if r.log_slot.state & quest_slot_state::FAIL != 0 {
                -1
            } else {
                0
            };
            entries.push(QuestLogEntryView {
                quest_id: r.quest_id,
                title,
                level,
                tag: None,
                is_header: false,
                collapsed: false,
                complete,
                // The slot's raw deadline (absolute unix seconds; 0 = untimed) — carried, never
                // converted here. The countdown is subtracted per Lua call against the server
                // clock, so this snapshot stays stable while the number on screen ticks
                // (decision 1150).
                timer: r.log_slot.timer,
                objectives,
            });
            entry_slots.push(Some(r.slot));
            header_keys.push(None);
        }
    }

    // ── Selection remap ─────────────────────────────────────────────────────────────────────────
    let sel = script.quest_log_selection();
    let new_sel = remap_selection(&last.entries, &entries, sel);
    if new_sel != sel {
        script.set_quest_log_selection(new_sel);
    }
    quest_log.entry_slots = entry_slots;
    quest_log.header_keys = header_keys;

    let player = crate::npc_text::player_identity(&self_q, &mut names, &commands);
    let macros = crate::npc_text::MacroContext {
        subject: player.as_ref(),
        states: &states,
    };
    let sel = script.quest_log_selection() as usize;
    let detail = sel
        .checked_sub(1)
        .and_then(|i| entries.get(i))
        .filter(|e| !e.is_header)
        .map(|e| e.quest_id)
        .and_then(|quest_id| {
            quest_log.template(quest_id, &commands).map(|t| {
                build_detail(
                    t,
                    &mut items,
                    icons.as_deref(),
                    &commands,
                    &macros,
                    &player_req,
                    &actions,
                )
            })
        });

    let fresh = QuestLogState {
        entries,
        num_quests: rows.len() as u32,
        detail,
    };
    if fresh == *last {
        return;
    }
    // The objective-progress announces (the quest-update handler law — wow-re
    // `object-layer/scratch/quest-update-ui-feedback-law.md`, §5 trio 2026-07-12, handler
    // `0x5e5ad0`), all fired AFTER the log push so a handler reading the log sees the fresh
    // state. Per progressed quest ([`quests_with_progressed_objectives`] — present in BOTH
    // states; a fresh accept or a turn-in is not "achieved a quest objective"):
    //  - UI_INFO_MESSAGE with each moved line's fresh text — the yellow top-center toast. The
    //    verified keys `ERR_QUEST_ADD_KILL_SII "%s slain: %d/%d"` (creature) /
    //    `ERR_QUEST_ADD_FOUND_SII "%s: %d/%d"` (GO) / `ERR_QUEST_ADD_ITEM_SII "%s: %d/%d"`
    //    compose exactly the leaderboard line, which is why the line IS the message. And on the
    //    whole-quest COMPLETE flip: `ERR_QUEST_OBJECTIVE_COMPLETE_S "%s (Complete)"`, falling to
    //    `ERR_QUEST_UNKNOWN_COMPLETE "Objective Complete."` when the name is unresolvable — the
    //    verified 0x198 pair (never `ERR_QUEST_COMPLETE_S`, a different call site). INTERIM
    //    (named divergence): the real client fires from the SMSG handlers and SKIPS a toast
    //    whose name is uncached (peek-only lookups); ours rides the log diff that feeds the
    //    lines, so the toast always agrees with the log (an in-flight name shows the log's own
    //    placeholder). The diff announces an objective only when it ADVANCED — never on a
    //    regression or a re-render ([`advanced`]; decision 1152 / B237).
    //  - QUEST_WATCH_UPDATE with the byte law's arg: the quest's **1-based WATCH-LIST position,
    //    0 when unwatched** (`0x703f50(0x221)` ← `0x4df880` — NOT the quest-log index the ref
    //    FrameXML's `AutoQuestWatch_Update(arg1)` treats it as; the shipped 1.12 auto-watch
    //    chain is broken at exactly this seam).
    //  - BENILLA_QUEST_PROGRESS with the quest's 1-based LOG index — our own event carrying what
    //    the ref Lua *needed*: the auto-watch (`OPTION_TOOLTIP_AUTO_QUEST_WATCH`, "watched for 5
    //    minutes when you achieve a quest objective") runs off this, a deliberate
    //    intent-over-letter divergence from the ref's broken chain (QuestLogFrame.xml's
    //    auto-watch comment; decision record with this fold-back).
    let progressed = quests_with_progressed_objectives(last, &fresh);
    script.set_quest_log(fresh.clone());
    script.fire_event("QUEST_LOG_UPDATE", vec![]);
    if !progressed.is_empty() {
        // Post-push = post-prune: the current watch list, the byte law's index space.
        let watched = script.quest_log_watched();
        for quest in progressed {
            for line in quest.changed_lines {
                script.fire_event("UI_INFO_MESSAGE", vec![ScriptValue::Str(line)]);
            }
            if quest.completed {
                let msg = if quest.title.is_empty() {
                    "Objective Complete.".to_string()
                } else {
                    format!("{} (Complete)", quest.title)
                };
                script.fire_event("UI_INFO_MESSAGE", vec![ScriptValue::Str(msg)]);
            }
            let watch_index = watched
                .iter()
                .position(|&id| id == quest.quest_id)
                .map_or(0, |p| p as i64 + 1);
            script.fire_event("QUEST_WATCH_UPDATE", vec![ScriptValue::Int(watch_index)]);
            script.fire_event(
                "BENILLA_QUEST_PROGRESS",
                vec![ScriptValue::Int(i64::from(quest.index))],
            );
        }
    }
    *last = fresh;
}

/// Did one objective **advance** between two log states — the announce predicate, and the whole of
/// decision 1152's fix for B237.
///
/// The verified law (wow-re `object-layer/scratch/quest-update-ui-feedback-law.md`, handler
/// `0x5e5ad0`) is that the progress toast fires from the server's *additive* announcements alone —
/// `SMSG_QUESTUPDATE_ADD_KILL` (`ERR_QUEST_ADD_KILL_SII`) and `_ADD_ITEM`
/// (`ERR_QUEST_ADD_ITEM_SII`, via `0x5dd060`). There is no removal opcode and no client-side
/// re-announce, so the reference **cannot** toast on an objective going backwards. Ours rides the
/// log diff (0340's named divergence — the same state feeds the toast and the log, so they always
/// agree), which made the predicate "the rendered line changed" — strictly wider than the law, and
/// wrong in exactly one director-visible way: a **turn-in** destroys the required items via an
/// inline `SMSG_DESTROY_OBJECT` while the quest-log slot clears only in the tick's batched
/// `SMSG_UPDATE_OBJECT` (vmangos `Player::RewardQuest` — `DestroyItemCount` at the top, the
/// `SetQuestSlot(log_slot, 0)` ~40 lines later), so for a frame or two the quest is still in the
/// log with an empty bag and the line dips to `0/req`. The old predicate read that dip as progress
/// and popped "Rumbleshot's Ammo: 0/1" in the middle of the screen (B237).
///
/// Comparing the numbers rather than the string also drops the re-announce 0340 named as INTERIM
/// (an item/creature **name** landing late rewrote `text` with `cur` unmoved, toasting the same
/// line twice) — the reference skips a toast whose name is uncached outright, so suppressing the
/// second is a step toward it, not away.
///
/// The whole-quest COMPLETE flip is deliberately NOT part of this: it is the reference's own
/// separate `SMSG_QUESTUPDATE_COMPLETE` message (`ERR_QUEST_OBJECTIVE_COMPLETE_S`, one line for
/// the quest), carried by [`QuestProgress::completed`].
fn advanced(now: &QuestLogObjectiveView, was: &QuestLogObjectiveView) -> bool {
    now.cur > was.cur
}

/// One quest whose objectives advanced between log states — the announce unit of
/// [`feed_quest_log`].
struct QuestProgress {
    /// The quest id (stable identity — the watch list is id-keyed).
    quest_id: u32,
    /// The quest's 1-based index in the NEW log (the Era API's quest-log index space).
    index: u32,
    /// The quest title (the COMPLETE toast's `%s`; empty falls to the UNKNOWN form).
    title: String,
    /// The fresh text of every objective line that ADVANCED ([`advanced`]), in objective order.
    changed_lines: Vec<String>,
    /// The whole-quest COMPLETE state flipped on this diff (the `0x198` toast).
    completed: bool,
}

/// The quests present in BOTH states with a same-position objective line that **advanced**, or
/// whose whole-quest COMPLETE state flipped on — "achieved a quest objective", the trigger for
/// the progress toasts and the auto-watch. A quest appearing (fresh accept — or its template
/// resolving late and growing lines from zero) or leaving (turn-in/abandon) does NOT count: the
/// pairwise zip skips added/removed lines by construction. Pure, so it's testable without the
/// ECS/ask-once caches — the exact shape [`abandon_slot`] already keeps this file's
/// feed-vs-logic split in.
fn quests_with_progressed_objectives(
    old: &QuestLogState,
    new: &QuestLogState,
) -> Vec<QuestProgress> {
    fn entry_of(s: &QuestLogState, id: u32) -> Option<&QuestLogEntryView> {
        s.entries.iter().find(|e| !e.is_header && e.quest_id == id)
    }
    new.entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.is_header)
        .filter_map(|(i, e)| {
            let prev = entry_of(old, e.quest_id)?;
            let changed_lines: Vec<String> = e
                .objectives
                .iter()
                .zip(prev.objectives.iter())
                .filter(|(now, was)| advanced(now, was))
                .map(|(now, _)| now.text.clone())
                .collect();
            let completed = e.complete == 1 && prev.complete != 1;
            (!changed_lines.is_empty() || completed).then_some(QuestProgress {
                quest_id: e.quest_id,
                index: i as u32 + 1,
                title: e.title.clone(),
                changed_lines,
                completed,
            })
        })
        .collect()
}

/// Drain the confirmed abandons (1-based Lua entry index, pinned at click time — see
/// `benilla_ui::script::quest_log`'s module doc) and map each to `CMSG_QUESTLOG_REMOVE_QUEST` via
/// this frame's `entry_slots`.
/// The `ZoneOrSort → header name` lookup ([`benilla_formats::QuestHeaderNames`]), loaded once at
/// startup. Absent when the client data didn't load — the feed then buckets everything under
/// "Quests".
#[derive(Resource)]
pub(crate) struct QuestHeaderNamesRes(pub benilla_formats::QuestHeaderNames);

/// Startup: read AreaTable/QuestSort names through the patch chain (the zone-audio catalog's
/// exact load shape).
fn load_quest_header_names(
    mut commands: Commands,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_quest_header_names(&mut chain)
    };
    match loaded {
        Ok(names) => commands.insert_resource(QuestHeaderNamesRes(names)),
        Err(e) => warn!("ui_quest_log: quest header names failed to load: {e:#}"),
    }
}

/// Drain the header fold intents (`CollapseQuestHeader`/`ExpandQuestHeader`): map each 1-based
/// entry index through this frame's `header_keys` to the header TITLE and toggle it in the
/// app-owned collapse set; index 0 = every header (the ref's collapse-all button). The next
/// [`feed_quest_log`] pass rebuilds the filtered list.
fn drain_quest_log_collapses(
    script: Option<NonSendMut<UiScript>>,
    mut quest_log: ResMut<QuestLog>,
) {
    let Some(mut script) = script else {
        return;
    };
    for (idx, collapse) in script.take_quest_log_collapses() {
        if idx == 0 {
            if collapse {
                let all: Vec<String> = quest_log.header_keys.iter().flatten().cloned().collect();
                quest_log.collapsed.extend(all);
            } else {
                quest_log.collapsed.clear();
            }
        } else if let Some(Some(name)) = quest_log.header_keys.get(idx as usize - 1).cloned() {
            if collapse {
                quest_log.collapsed.insert(name);
            } else {
                quest_log.collapsed.remove(&name);
            }
        }
    }
}

fn drain_quest_log_abandons(
    script: Option<NonSendMut<UiScript>>,
    quest_log: Res<QuestLog>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for entry in script.take_quest_log_abandons() {
        match abandon_slot(entry, &quest_log.entry_slots) {
            Some(slot) => {
                debug!("ui_quest_log: abandon entry {entry} → slot {slot}");
                let _ = commands.0.send(ClientCommand::QuestlogRemove { slot });
            }
            None => debug!("ui_quest_log: abandon entry {entry} out of range — ignored"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::test_template;
    use crate::ui_action::PlayerActions;
    use benilla_ui::script::PlayerReqState;

    // ── remap_selection ─────────────────────────────────────────────────────────────────────────

    fn header(title: &str) -> QuestLogEntryView {
        QuestLogEntryView {
            quest_id: 0,
            title: title.into(),
            level: 0,
            tag: None,
            is_header: true,
            collapsed: false,
            complete: 0,
            timer: 0,
            objectives: Vec::new(),
        }
    }

    fn quest(id: u32) -> QuestLogEntryView {
        QuestLogEntryView {
            quest_id: id,
            title: format!("Quest {id}"),
            level: 1,
            tag: None,
            is_header: false,
            collapsed: false,
            complete: 0,
            timer: 0,
            objectives: Vec::new(),
        }
    }

    #[test]
    fn selection_follows_its_quest_across_reindexing() {
        let old = vec![header("A"), quest(7), quest(9)];
        // A new header lands above: quest 9 moves from entry 3 to entry 5.
        let new = vec![header("A"), quest(7), header("B"), header("C"), quest(9)];
        assert_eq!(remap_selection(&old, &new, 3), 5);
    }

    #[test]
    fn fold_hiding_the_selection_resets_to_zero_never_the_header() {
        // The director's defect: selecting quest 7, folding "A" used to re-point the selection at
        // the HEADER row — the detail pane painted the category. A header is never a valid
        // selection (ref QuestLog_GetFirstSelectableQuest skips headers): reset to 0 so the Lua
        // update's SetFirstValidSelection picks the first visible quest.
        let old = vec![header("A"), quest(7)];
        let new = vec![header("A")]; // folded: the quest left the visible list
        assert_eq!(remap_selection(&old, &new, 2), 0);
    }

    #[test]
    fn a_header_selection_is_evicted_not_preserved() {
        let old = vec![header("A"), quest(7)];
        let new = vec![header("A"), quest(7)];
        // However a header got selected, the remap never keeps it.
        assert_eq!(remap_selection(&old, &new, 1), 0);
    }

    #[test]
    fn zero_and_stale_selections_stay_zero() {
        let old = vec![header("A"), quest(7)];
        let new = vec![header("A"), quest(7)];
        assert_eq!(remap_selection(&old, &new, 0), 0);
        assert_eq!(remap_selection(&old, &new, 99), 0); // out-of-range: nothing to follow
    }

    fn obj(
        creature_or_go: u32,
        required_count: u32,
        item_id: u32,
        item_count: u32,
        text: &str,
    ) -> QuestObjective {
        QuestObjective {
            creature_or_go,
            required_count,
            item_id,
            item_count,
            text: text.into(),
        }
    }

    // ── objective_line ──────────────────────────────────────────────────────────────────────────

    #[test]
    fn creature_objective_with_resolved_name() {
        let o = obj(100, 10, 0, 0, "");
        let line = objective_line(&o, 3, Some("Kobold Vermin"), None, 0).unwrap();
        assert_eq!(line.text, "Kobold Vermin slain: 3/10");
        assert_eq!(line.kind, "monster");
        assert!(!line.finished);
    }

    #[test]
    fn creature_objective_in_flight_shows_a_placeholder() {
        let o = obj(100, 10, 0, 0, "");
        let line = objective_line(&o, 0, None, None, 0).unwrap();
        assert_eq!(line.text, "... slain: 0/10");
    }

    #[test]
    fn go_objective_falls_back_without_a_name_cache() {
        let go_id = ((-57i32) as u32) | 0x8000_0000;
        let o = obj(go_id, 1, 0, 0, "");
        let line = objective_line(&o, 1, None, None, 0).unwrap();
        assert_eq!(line.text, "Objective: 1/1");
        assert_eq!(line.kind, "object");
        assert!(line.finished); // cur (1) >= req (1)
    }

    #[test]
    fn go_objective_prefers_its_custom_text_over_the_fallback() {
        let go_id = ((-57i32) as u32) | 0x8000_0000;
        let o = obj(go_id, 4, 0, 0, "Destroy the barricade");
        let line = objective_line(&o, 1, None, None, 0).unwrap();
        assert_eq!(line.text, "Destroy the barricade: 1/4");
        assert_eq!(line.kind, "object");
    }

    #[test]
    fn item_objective_counts_bags_and_clamps_to_required() {
        let o = obj(0, 0, 2000, 5, "");
        // 8 in the bags, but required is only 5 — the line clamps, doesn't overshoot.
        let line = objective_line(&o, 0, None, Some("Kobold Ear"), 8).unwrap();
        assert_eq!(line.text, "Kobold Ear: 5/5");
        assert_eq!(line.kind, "item");
        assert!(line.finished);
    }

    #[test]
    fn item_objective_in_flight_shows_a_placeholder() {
        let o = obj(0, 0, 2000, 5, "");
        let line = objective_line(&o, 0, None, None, 2).unwrap();
        assert_eq!(line.text, "...: 2/5");
    }

    #[test]
    fn custom_text_overrides_the_creature_auto_line() {
        let o = obj(100, 10, 0, 0, "Slay the vermin");
        let line = objective_line(&o, 4, Some("Kobold Vermin"), None, 0).unwrap();
        assert_eq!(line.text, "Slay the vermin: 4/10");
        assert_eq!(line.kind, "monster");
    }

    /// Decision 1158, correcting 0109: a line's `finished` is `cur >= req` and **nothing else**.
    /// The reference reads the whole-quest COMPLETE bit exactly once in `GetQuestLogLeaderBoard`
    /// (`0x4e02a2`), in the `event` branch, where it is that line's only predicate — it is never
    /// or-ed into a counted line's verdict (wow-re `ui/scratch/quest-leaderboard-law.md` §7). Only
    /// the whole-quest turn-in predicate `0x4df580` reads the bit and the counts together.
    #[test]
    fn a_complete_quest_does_not_mark_a_short_objective_finished() {
        let o = obj(100, 10, 0, 0, "");
        // The counter (1) is well short of required (10). Whatever the quest's state byte says,
        // THIS line is not finished.
        let line = objective_line(&o, 1, Some("Kobold"), None, 0).unwrap();
        assert_eq!(line.text, "Kobold slain: 1/10");
        assert!(!line.finished);
    }

    #[test]
    fn empty_objective_slot_emits_no_line() {
        let o = obj(0, 0, 0, 0, "");
        assert!(objective_line(&o, 0, None, None, 0).is_none());
    }

    // ── build_objectives: every occupied slot gets its own lines, not just the selection ────────────
    // (the seam this Part-A change is built on: `QuestLogEntryView::objectives` moved off the
    // selection-only `QuestLogDetail` — `benilla-ui/src/script/quest_log.rs`'s module doc.)

    fn quest_template(objectives: [QuestObjective; 4]) -> QuestTemplate {
        QuestTemplate {
            quest_id: 1,
            method: 0,
            level: 1,
            zone_or_sort: 0,
            quest_type: 0,
            rep_objective_faction: 0,
            rep_objective_value: 0,
            next_quest_in_chain: 0,
            money: 0,
            money_max_level: 0,
            reward_spell: 0,
            src_item_id: 0,
            flags: 0,
            rewards: [(0, 0); 4],
            choices: [(0, 0); 6],
            point_map_id: 0,
            point_x: 0.0,
            point_y: 0.0,
            point_opt: 0,
            title: "A Threat Within".into(),
            objectives_text: String::new(),
            details: String::new(),
            end_text: String::new(),
            objectives,
        }
    }

    #[test]
    fn resolve_template_item_uses_the_shared_item_usable_rule() {
        let mut items = Items::default();
        let mut hammer = test_template("Militia Warhammer");
        hammer.class = 2;
        hammer.subclass = 5;
        items.insert_template(5579, Some(hammer));
        let (tx, _rx) = crossbeam_channel::unbounded();
        let row = resolve_template_item(
            5579,
            1,
            &mut items,
            None,
            &NetCommands(tx),
            &PlayerReqState {
                level: 10,
                class_id: 1,
                race_id: 1,
                proficiency: [(2, 1 << 4)].into_iter().collect(),
                ..Default::default()
            },
            &PlayerActions::default(),
        );
        assert!(
            !row.usable,
            "a missing weapon proficiency must mark the quest-log row unusable"
        );
    }

    #[test]
    fn resolve_template_item_keeps_an_unresolved_template_usable() {
        let mut items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let row = resolve_template_item(
            5579,
            1,
            &mut items,
            None,
            &NetCommands(tx),
            &PlayerReqState {
                level: 10,
                class_id: 1,
                race_id: 1,
                ..Default::default()
            },
            &PlayerActions::default(),
        );

        assert!(row.usable);
        assert!(row.name.is_none());
    }

    #[test]
    fn build_objectives_reads_every_occupied_quad_via_the_slots_counters_and_skips_empties() {
        let template = quest_template([
            obj(100, 10, 0, 0, ""), // creature objective, counter[0]
            obj(0, 0, 0, 0, ""),    // unused quad — emits no line
            obj(0, 0, 2000, 5, ""), // item objective (counted from the bags, not a counter)
            obj(0, 0, 0, 0, ""),    // unused quad
        ]);
        let log_slot = QuestLogSlot {
            quest_id: 1,
            counters: [3, 0, 0, 0],
            state: 0,
            timer: 0,
        };
        let store = ObjectFields::default(); // empty bags — the item objective reads 0/5
        let mut items = Items::default();
        let mut names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template, &log_slot, &store, &mut items, &mut names, &commands,
        );

        // The two unused quads emit nothing; the two real ones resolve in order — creature/item
        // names are unresolved (ask-once mid-flight, no answer seeded) so they fall to the same
        // placeholder path `objective_line`'s own tests already cover directly.
        assert_eq!(objectives.len(), 2);
        assert_eq!(objectives[0].text, "... slain: 3/10");
        assert_eq!(objectives[0].kind, "monster");
        assert!(!objectives[0].finished);
        assert_eq!(objectives[1].text, "...: 0/5");
        assert_eq!(objectives[1].kind, "item");
    }

    /// The 0109-era behaviour this inverts: a COMPLETE slot state used to mark every line
    /// finished. Decision 1158 — the reference's per-line verdict never reads that bit
    /// (`0x4e0110` §7); only the whole-quest turn-in predicate (`0x4df580`) combines the two.
    #[test]
    fn a_complete_slot_state_does_not_finish_a_short_line() {
        let template = quest_template([
            obj(100, 10, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
        ]);
        let log_slot = QuestLogSlot {
            quest_id: 1,
            counters: [1, 0, 0, 0], // well short of required (10)...
            state: quest_slot_state::COMPLETE, // ...but the whole quest is complete
            timer: 0,
        };
        let store = ObjectFields::default();
        let mut items = Items::default();
        let mut names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template, &log_slot, &store, &mut items, &mut names, &commands,
        );
        assert_eq!(objectives.len(), 1);
        assert!(
            !objectives[0].finished,
            "the counter is 1/10 — the quest's COMPLETE bit is not this line's business"
        );
        assert_eq!(objectives[0].text, "... slain: 1/10");
    }

    // ── money_split ──────────────────────────────────────────────────────────────────────────────

    #[test]
    fn money_sign_splits_into_required_or_reward() {
        assert_eq!(money_split(-500), (500, 0));
        assert_eq!(money_split(500), (0, 500));
        assert_eq!(money_split(0), (0, 0));
    }

    // ── abandon_slot: entry → descriptor slot across gaps ──────────────────────────────────────────

    #[test]
    fn abandon_maps_entry_to_slot_across_a_gap_and_skips_headers() {
        // A header row (None), then slots 0 and 2 (slot 1 empty in the descriptor array) — the
        // push order skips the gap; entry 1 is the header, which maps to NO slot.
        let entry_slots = vec![None, Some(0u8), Some(2u8)];
        assert_eq!(abandon_slot(1, &entry_slots), None); // header row: nothing to remove
        assert_eq!(abandon_slot(2, &entry_slots), Some(0));
        assert_eq!(abandon_slot(3, &entry_slots), Some(2));
        assert_eq!(abandon_slot(4, &entry_slots), None);
        assert_eq!(abandon_slot(0, &entry_slots), None); // 1-based; 0 is never a valid entry
    }

    // ── quests_with_progressed_objectives: the QUEST_WATCH_UPDATE per-quest trigger ─────────────────

    /// A one-objective entry built the way [`objective_line`] builds one: the `cur`/`req` carried
    /// on the view are READ BACK OUT of the line's own trailing `%d/%d`, so a fixture can never
    /// claim a count its text disagrees with (the production invariant — the text is formatted
    /// FROM the numbers).
    fn entry(quest_id: u32, objective_text: &str) -> QuestLogEntryView {
        let (cur, req) = objective_text
            .rsplit_once(' ')
            .and_then(|(_, tail)| tail.split_once('/'))
            .map(|(c, r)| (c.parse().unwrap(), r.parse().unwrap()))
            .expect("fixture line must end in `cur/req`");
        QuestLogEntryView {
            quest_id,
            objectives: vec![QuestLogObjectiveView {
                text: objective_text.into(),
                kind: "monster".into(),
                finished: cur >= req,
                cur,
                req,
            }],
            ..Default::default()
        }
    }

    fn state(entries: Vec<QuestLogEntryView>) -> QuestLogState {
        QuestLogState {
            num_quests: entries.iter().filter(|e| !e.is_header).count() as u32,
            entries,
            detail: None,
        }
    }

    #[test]
    fn no_progress_fires_nothing() {
        let old = state(vec![entry(1, "Kobold Vermin slain: 3/10")]);
        let same = state(vec![entry(1, "Kobold Vermin slain: 3/10")]);
        assert!(quests_with_progressed_objectives(&old, &same).is_empty());
    }

    #[test]
    fn a_progressed_quest_fires_its_new_log_index_and_the_moved_lines() {
        let old = state(vec![
            entry(1, "Kobold Vermin slain: 3/10"),
            entry(2, "Tough Wolf Meat: 2/8"),
        ]);
        let progressed = state(vec![
            entry(1, "Kobold Vermin slain: 3/10"),
            entry(2, "Tough Wolf Meat: 3/8"),
        ]);
        let fired = quests_with_progressed_objectives(&old, &progressed);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].index, 2);
        assert_eq!(fired[0].changed_lines, ["Tough Wolf Meat: 3/8"]);
        // Both moving fires both, each at its own 1-based index with its own fresh line.
        let both = state(vec![
            entry(1, "Kobold Vermin slain: 4/10"),
            entry(2, "Tough Wolf Meat: 3/8"),
        ]);
        let fired = quests_with_progressed_objectives(&old, &both);
        assert_eq!(
            fired
                .iter()
                .map(|q| (q.index, q.changed_lines.clone()))
                .collect::<Vec<_>>(),
            [
                (1, vec!["Kobold Vermin slain: 4/10".to_string()]),
                (2, vec!["Tough Wolf Meat: 3/8".to_string()]),
            ]
        );
    }

    #[test]
    fn appearing_or_leaving_the_log_is_not_progress() {
        let empty = state(vec![]);
        let with_entry = state(vec![entry(7, "Kobold Worker slain: 0/10")]);
        // A fresh accept must NOT toast or auto-watch (the ref triggers on "achieved an
        // objective") — and neither may a template resolving late and growing lines from zero.
        assert!(quests_with_progressed_objectives(&empty, &with_entry).is_empty());
        let grown = state(vec![QuestLogEntryView {
            quest_id: 7,
            objectives: vec![],
            ..Default::default()
        }]);
        assert!(quests_with_progressed_objectives(&grown, &with_entry).is_empty());
        // A turn-in/abandon that removes the quest fires nothing either.
        assert!(quests_with_progressed_objectives(&with_entry, &empty).is_empty());
    }

    /// B237 / decision 1152 — the turn-in flash. `Player::RewardQuest` destroys the required items
    /// at the top (an inline `SMSG_DESTROY_OBJECT`) and clears the log slot ~40 lines later (the
    /// tick's batched `SMSG_UPDATE_OBJECT`), so the client holds a complete collect quest over an
    /// empty bag for a frame or two and the line dips to `0/req`. That dip must announce NOTHING:
    /// the reference's toast is `SMSG_QUESTUPDATE_ADD_ITEM` only, and no removal opcode exists.
    #[test]
    fn a_regressing_objective_never_announces() {
        let held = state(vec![entry(313, "Rumbleshot's Ammo: 1/1")]);
        let emptied = state(vec![entry(313, "Rumbleshot's Ammo: 0/1")]);
        assert!(
            quests_with_progressed_objectives(&held, &emptied).is_empty(),
            "the bag emptying under a still-logged quest is not progress"
        );
        // The slot clearing on the next frame was already silent (the quest leaves the log).
        assert!(quests_with_progressed_objectives(&emptied, &state(vec![])).is_empty());
        // The control: the same quest actually being collected still announces.
        let looted = state(vec![entry(313, "Rumbleshot's Ammo: 1/1")]);
        assert_eq!(
            quests_with_progressed_objectives(&emptied, &looted)[0].changed_lines,
            ["Rumbleshot's Ammo: 1/1"]
        );
    }

    /// The ask-once name caches answer a frame or more after the line first renders. That rewrite
    /// moves `text` with the count standing still — the reference skips a toast whose name is
    /// uncached outright, so re-announcing the line when the name lands (0340's named INTERIM) is
    /// a toast the reference never fires.
    #[test]
    fn a_name_landing_late_does_not_re_announce() {
        let placeholder = state(vec![entry(5, "...: 3/5")]);
        let named = state(vec![entry(5, "Small Barnacled Clam: 3/5")]);
        assert!(quests_with_progressed_objectives(&placeholder, &named).is_empty());
    }

    /// The whole-quest COMPLETE flip is its own reference message (`SMSG_QUESTUPDATE_COMPLETE` →
    /// `ERR_QUEST_OBJECTIVE_COMPLETE_S`, one line for the quest) — it must not also re-announce
    /// every objective line just because their `finished` flag flipped with it.
    #[test]
    fn the_complete_flip_announces_once_not_per_objective() {
        let mut before = entry(9, "Kobold Vermin slain: 10/10");
        before.complete = 0;
        before.objectives[0].finished = false;
        let mut after = entry(9, "Kobold Vermin slain: 10/10");
        after.complete = 1;
        after.title = "A Threat Within".into();
        let fired = quests_with_progressed_objectives(&state(vec![before]), &state(vec![after]));
        assert_eq!(fired.len(), 1);
        assert!(fired[0].completed);
        assert!(
            fired[0].changed_lines.is_empty(),
            "the objective count did not move — only the quest's COMPLETE state did"
        );
    }
}
