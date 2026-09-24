//! The app-side **quest-log state + feed** — decision 0088's deferred second slice (the giver-panel
//! decision named the log window, `SMSG_QUEST_QUERY_RESPONSE`, and the `PLAYER_QUEST_LOG` field
//! accessors as a follow-up; the wire is now pinned, and this is the app
//! layer over it). The inward half of the seam around [`benilla_ui::script::quest_log`], the log's
//! twin of [`crate::ui_quest`]'s questgiver-panel feed.
//!
//! Unlike the questgiver panels (a transient NPC-session window), the quest log is **durable
//! player state**: [`feed_quest_log`] reads the self player's `PLAYER_QUEST_LOG` descriptor slots
//! every frame (the wire pin's field 198 + 3·slot) rather than reacting to one wire event, and
//! resolves EVERY occupied slot's title/level/tag/objectives from the `SMSG_QUEST_QUERY_RESPONSE`
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
//! byte arg, `0x4df880`), and `BENILLA_QUEST_PROGRESS(logIndex)` (the auto-watch's feed — the
//! divergence note at the fire site / decision 0340) — diffed against a `Local`, exactly like
//! every other feed in this crate.
//!
//! [`drain_quest_log_abandons`] maps the confirmed abandon's quest id to its descriptor slot (this
//! frame's rows, kept on [`QuestLog`]) and sends `CMSG_QUESTLOG_REMOVE_QUEST`.
//!
//! This module used to also answer the questgiver greeting's active/available split, via a
//! `contains` membership test over the live descriptor slots. **It doesn't any more, and must not
//! again** — the reference splits that list on the wire icon alone and never consults its quest log
//! there ([`crate::ui_quest::row_is_active`], decision 0758). Membership was wrong for exactly the
//! quests that are never in the log: auto-complete turn-ins. The state that backed it is gone
//! rather than left dead, so the retired law can't be reached for by accident.

use crate::ui_items::{count_of, InventoryScope};
use std::collections::HashSet;

use bevy::prelude::*;

use benilla_protocol::messages::{
    quest_flags, quest_slot_state, QuestLogSlot, QuestObjective, QuestTemplate,
    PLAYER_QUEST_LOG_SLOTS,
};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{
    QuestItemView, QuestLogDetail, QuestLogEntryView, QuestLogObjectiveView, QuestLogState,
    ScriptValue, UiScript,
};
use benilla_ui::strings::{fill, Arg};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::QueryCache;
use crate::ui_action::Spells;
use crate::ui_script::{UiFeed, UiInput};
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
/// - `templates` — the `SMSG_QUEST_QUERY_RESPONSE` cache, ask-once by quest id through
///   [`QueryCache`] (the exact twin of [`Items`]'s item-template cache).
/// - `row_slots` — this frame's quest rows as `(quest id, descriptor slot)`, folded ones included:
///   the reference's row table `0xbb71c0`, which [`drain_quest_log_abandons`] searches by the
///   confirmed abandon's quest id for the `CMSG_QUESTLOG_REMOVE_QUEST` slot (slots aren't
///   contiguous — an abandoned/turned-in quest leaves a gap).
#[derive(Resource, Default)]
pub(crate) struct QuestLog {
    templates: QueryCache<u32, QuestTemplate>,
    row_slots: Vec<(u32, u8)>,
    /// Collapsed section headers, keyed by header TITLE (two zones sharing a name share a header
    /// row, so the fold state naturally shares too). Owned here — the engine only reports the
    /// flag and drains toggle intents ([`drain_quest_log_collapses`]).
    collapsed: HashSet<String>,
    /// This frame's pushed entry order → the header title for header rows (`None` for quests) —
    /// the collapse drain's index→identity map.
    header_keys: Vec<Option<String>>,
}

impl crate::query_cache::AskOnce for QuestLog {
    fn clear_pending(&mut self) {
        self.templates.clear_pending();
    }
}

impl QuestLog {
    /// The template for `quest_id`, if known — ask-once: a miss sends `CMSG_QUEST_QUERY` (deduped
    /// while in flight) and returns `None`; a cached negative (the server doesn't know the id) is
    /// also `None`, without a re-ask. The exact twin of [`Items::template`].
    pub(crate) fn template(&self, quest_id: u32, commands: &NetCommands) -> Option<&QuestTemplate> {
        self.templates.get_or_ask(quest_id, || {
            debug!("ui_quest_log: asking quest template (quest {quest_id})");
            let _ = commands
                .0
                .send(ClientCommand::QuestQuery { quest: quest_id });
        })
    }

    /// Record a template answer (`SMSG_QUEST_QUERY_RESPONSE`).
    pub(crate) fn insert_template(&mut self, template: QuestTemplate) {
        self.templates.insert(template.quest_id, Some(template));
    }

    /// Disconnect: drop everything, including the templates. Unlike item templates (which persist —
    /// decision: static, stable across sessions), a stale log slice carried across a reconnect would
    /// pair fresh descriptor slots (possibly renumbered) with old titles; templates are cheap enough
    /// to re-ask that there's no reason to risk it.
    pub(crate) fn clear_session(&mut self) {
        self.templates.clear();
        self.row_slots.clear();
        self.collapsed.clear();
        self.header_keys.clear();
    }
}

/// The quest log's packet handler (decision 0088's second slice; in the net handler table since
/// 2320, moved out of the drain's quests arm file).
mod net {
    use benilla_protocol::messages::QuestTemplate;
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::QuestLog;
    use crate::net::NetHandlerApp;

    /// Register the handler and the session-end listener — called from
    /// [`super::UiQuestLogPlugin`].
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::QuestTemplate, on_template)
            .net_handler(K::Disconnected, on_session_end);
    }

    fn on_template(In(ev): In<SessionEvent>, mut quest_log: ResMut<QuestLog>) {
        if let SessionEvent::QuestTemplate(t) = ev {
            quest_template(t, &mut quest_log);
        }
    }

    /// The log's session state dies with the socket. A listener on the session end
    /// (a second handler on the kind, after the bridge's own teardown).
    fn on_session_end(In(_): In<SessionEvent>, mut quest_log: ResMut<QuestLog>) {
        quest_log.clear_session();
    }

    /// The full quest template (`SMSG_QUEST_QUERY_RESPONSE`, answering our `CMSG_QUEST_QUERY`) — the
    /// quest log's ask-once detail source, cached by `quest_id`.
    fn quest_template(t: Box<QuestTemplate>, quest_log: &mut QuestLog) {
        debug!("net: quest template {} ({})", t.quest_id, t.title);
        quest_log.insert_template(*t);
    }
}

pub(crate) struct UiQuestLogPlugin;

impl Plugin for UiQuestLogPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        crate::query_cache::register::<QuestLog>(app);
        app.init_resource::<QuestLog>()
            .add_systems(
                Startup,
                (load_quest_header_names, load_quest_tag_names)
                    .after(benilla_assets::AssetSet::Open),
            )
            .add_systems(
                Update,
                (
                    feed_quest_log.in_set(UnitFeed),
                    // Before the script tick, so a QuestTimerFrame OnUpdate this frame reads this
                    // frame's clock (the `minimap::feed_game_time` shape).
                    feed_server_clock.in_set(UiFeed),
                    drain_quest_log_abandons.after(UiInput),
                    drain_quest_log_pushes.after(UiInput),
                    drain_quest_log_collapses.after(UiInput),
                ),
            );
    }
}

/// The name a still-in-flight cache miss wears. The reference's miss reads `" "` (a single space,
/// `0x82ee00`); ours reads this, a longer-standing
/// divergence that says "still in flight" rather than looking like a nameless objective.
const NAME_PLACEHOLDER: &str = "...";

/// Format one leaderboard line from its `%s`/`%d`/`%d` key — the shared tail of both passes below.
///
/// **The three format keys are the leaderboard's own, and are NOT the progress toast's** (decision
/// 2045's central trap: `QUEST_MONSTERS_KILLED` and `ERR_QUEST_ADD_KILL_SII` are the same enUS
/// sentence, and `QUEST_OBJECTS_FOUND`/`QUEST_ITEMS_NEEDED`/`ERR_QUEST_ADD_FOUND_SII`/
/// `ERR_QUEST_ADD_ITEM_SII` are the same four). `GetQuestLogLeaderBoard 0x4e0000` resolves
/// `QUEST_MONSTERS_KILLED` `0x84b61c` (creature, no override), `QUEST_OBJECTS_FOUND` `0x84b634`
/// (the override leg AND both GameObject legs) and `QUEST_ITEMS_NEEDED` `0x84b5f8` (item). Only
/// the toast handler `0x5e5ad0` names the `ERR_*` twins, and it names them because they are
/// *message-catalog* rows; these three are not.
///
/// A key the install does not carry renders the line with **empty text rather than no line**: the
/// reference's `FrameScript_GetText` hands back its pre-seeded empty string and the leaderboard
/// entry still exists, which is what `GetNumQuestLeaderBoards` counts.
fn leaderboard_text(
    // The VM's own `GlobalStrings.lua` (decision 2045).
    get: &dyn Fn(&str) -> Option<String>,
    key: &str,
    name: &str,
    cur: u32,
    req: u32,
) -> String {
    get(key).map_or_else(String::new, |f| {
        fill(
            &f,
            &[Arg::S(name), Arg::D(i64::from(cur)), Arg::D(i64::from(req))],
        )
    })
}

/// One creature/GameObject leaderboard line — the reference's **pass 2**, whose predicate is
/// `ReqCreatureOrGOId[i] != 0` alone (`0x4e00c0`/`0x4e02d7`). Pure: the caller has already resolved
/// the creature name (ask-once — `None` while the answer is in flight) and `counter`, the
/// descriptor slot's 6-bit counter for this array index.
///
/// The custom `text` field, when non-empty, REPLACES the auto-generated name in the line but keeps
/// the "cur/req" suffix (`0x4e03da`: a creature objective carrying `ObjectiveText[i]` formats with
/// `QUEST_OBJECTS_FOUND`, not the "slain" key, and that text IS the name — while `type` still
/// answers `"monster"`, read from a fresh sign test of the id, `0x4e04ca`).
fn creature_line(
    obj: &QuestObjective,
    counter: u8,
    creature_name: Option<&str>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<QuestLogObjectiveView> {
    if obj.creature_or_go == 0 {
        return None;
    }
    let req = obj.required_count;
    let is_go = obj.creature_or_go & GO_OBJECTIVE_BIT != 0;
    let cur = u32::from(counter).min(req);
    // The reference's fork, in its own order: a non-empty override IS the name and takes
    // `QUEST_OBJECTS_FOUND` on either kind; a GameObject takes that key on both its legs (the
    // binding fetches it once, above its cache test, at `0x4e031e`); only a plain creature reaches
    // `QUEST_MONSTERS_KILLED`. No gameobject-name cache yet — a later CMSG_GAMEOBJECT_QUERY slice
    // — so a GO with no override wears the placeholder.
    let (key, name) = if !obj.text.is_empty() {
        ("QUEST_OBJECTS_FOUND", obj.text.as_str())
    } else if is_go {
        ("QUEST_OBJECTS_FOUND", NAME_PLACEHOLDER)
    } else {
        (
            "QUEST_MONSTERS_KILLED",
            creature_name.unwrap_or(NAME_PLACEHOLDER),
        )
    };
    Some(QuestLogObjectiveView {
        text: leaderboard_text(get, key, name, cur, req),
        kind: if is_go { "object" } else { "monster" }.into(),
        finished: cur >= req,
        cur,
        req,
    })
}

/// One item ("collect") leaderboard line — the reference's **pass 3**, whose predicate is
/// `ReqItemId[i] > 0 && ReqItemId[i] != SrcItemId` (`0x4e04f4`–`0x4e04fd`): the `+0x34` test drops
/// the quest's own provided/starter item from the board. `bag_count` is the live inventory walk
/// ([`crate::ui_items::count_of`]) — item progress is NOT one of the slot's 6-bit counters, the
/// client counts bags itself.
///
/// **The name comes from the item cache alone.** The per-objective override buffer
/// (`template+0x14dc + i*0x100`) is read only by the creature and GameObject branches
/// (`0x4e033f`, `0x4e03c9`), and on a quad that carries both kinds that buffer belongs to the
/// *creature* objective — reading it here would label the item with the creature's text.
fn item_line(
    obj: &QuestObjective,
    src_item_id: u32,
    item_name: Option<&str>,
    bag_count: u32,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<QuestLogObjectiveView> {
    if obj.item_id == 0 || obj.item_id == src_item_id {
        return None;
    }
    let req = obj.item_count;
    let cur = bag_count.min(req);
    Some(QuestLogObjectiveView {
        text: leaderboard_text(
            get,
            "QUEST_ITEMS_NEEDED",
            item_name.unwrap_or(NAME_PLACEHOLDER),
            cur,
            req,
        ),
        kind: "item".into(),
        finished: cur >= req,
        cur,
        req,
    })
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
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> QuestItemView {
    let template = items.template(entry, 0, commands).cloned();
    let name = template.as_ref().map(|t| t.name.clone());
    let quality = template.as_ref().map(|t| t.quality).unwrap_or(1);
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
        usable: true, // v1: soft-gray only, server authoritative (mirrors ui_quest.rs's resolve_item)
        link,
    }
}

/// Build one occupied slot's objective ("leaderboard") lines from its template + this frame's
/// descriptor slot counters/bag counts — every entry gets these (`GetNumQuestLeaderBoards`/
/// `GetQuestLogLeaderBoard` answer ANY index, not just the selection: the watch tracker HUD reads
/// non-selected quests' lines, ref `QuestLogFrame.lua:613-663`).
///
/// **The passes are independent walks of the whole array, not one walk of merged quads** — the
/// reference enumerates `event`, then creature/GO over `ReqCreatureOrGOId[0..3]`, then item over
/// `ReqItemId[0..3]`, then reputation (`0x4e0000`/`0x4e0110`; decision 1156 §5 named the ORDER
/// half of this and its worse half went unnoticed). Our merged
/// walk emitted at most ONE line per index, so an objective quad carrying BOTH kinds lost its item
/// line outright: quest 358 "Graverobbers" (creature 1941 ×8 and item 2834 ×8, both at index 0)
/// showed 2 lines where the reference shows 3, and `GetNumQuestLeaderBoards` under-counted with it.
///
/// Still not emitted, and still gaps rather than errors (1156 §4): the `event` line and the
/// reputation line.
fn build_objectives(
    template: &QuestTemplate,
    log_slot: &QuestLogSlot,
    store: &ObjectFields,
    objects: &crate::net::Objects,
    items: &Items,
    names: &NameCache,
    commands: &NetCommands,
    // The VM's own `GlobalStrings.lua`, for the leaderboard's three format keys.
    get: &dyn Fn(&str) -> Option<String>,
) -> Vec<QuestLogObjectiveView> {
    let mut objectives = Vec::with_capacity(template.objectives.len());
    // Pass 2 — creature/GameObject. The counter index is this array index (1156: the reference
    // increments it per array ELEMENT, including elements that produce no line, which is what makes
    // `log_slot.counters[i]` the right read over the fixed 4).
    for (i, obj) in template.objectives.iter().enumerate() {
        let is_go = obj.creature_or_go & GO_OBJECTIVE_BIT != 0;
        let creature_name = (obj.creature_or_go != 0 && !is_go)
            .then(|| {
                names
                    .resolve_creature(obj.creature_or_go, 0, commands)
                    .map(str::to_string)
            })
            .flatten();
        if let Some(line) = creature_line(obj, log_slot.counters[i], creature_name.as_deref(), get)
        {
            objectives.push(line);
        }
    }
    // Pass 3 — item.
    for obj in template.objectives.iter() {
        if obj.item_id == 0 {
            continue;
        }
        let item_name = items
            .template(obj.item_id, 0, commands)
            .map(|t| t.name.clone());
        let bag_count = count_of(store, objects, obj.item_id, InventoryScope::QUEST_ITEMS);
        if let Some(line) = item_line(
            obj,
            template.src_item_id,
            item_name.as_deref(),
            bag_count,
            get,
        ) {
            objectives.push(line);
        }
    }
    objectives
}

/// Build a row's detail pane from its template — description/money/rewards/choices
/// only (the objective lines live on every [`QuestLogEntryView::objectives`], selection or
/// not — see [`build_objectives`]), so this no longer needs the descriptor slot at all.
fn build_detail(
    template: &QuestTemplate,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    macros: &crate::npc_text::MacroContext,
    spells: Option<&Spells>,
) -> QuestLogDetail {
    let (required_money, reward_money) = money_split(template.money);
    let rewards = template
        .rewards
        .iter()
        .filter(|&&(id, _)| id != 0)
        .map(|&(id, count)| resolve_template_item(id, count, items, icons, commands))
        .collect();
    let choices = template
        .choices
        .iter()
        .filter(|&&(id, _)| id != 0)
        .map(|&(id, count)| resolve_template_item(id, count, items, icons, commands))
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
        reward_spell: crate::ui_quest::reward_spell_view(template.reward_spell, spells),
    }
}

/// The descriptor slot of the row carrying `quest_id` — `AbandonQuest`'s search (`0x4df070`: walk
/// the 40-row table `0xbb71c0`, skip headers, match the row's quest id, send its slot `+0x4`);
/// `None` when no row carries it, which the reference answers by doing nothing. Pure, so it's
/// testable without a live [`QuestLog`]/ECS.
fn abandon_slot(quest_id: u32, row_slots: &[(u32, u8)]) -> Option<u8> {
    row_slots
        .iter()
        .find(|&&(id, _)| id == quest_id)
        .map(|&(_, slot)| slot)
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

/// The header literal the reference paints for a header whose id is exactly `0` (`0x84b4ec`, its
/// only reference image-wide). It is a **designer-data tell, not a transient**: the only value that
/// can become a header id is a CACHED template's `zoneOrSort` (`0x4de6f6`), so it cannot appear
/// while a template is in flight — which is what makes it safe to render at all.
const MISSING_HEADER: &str = "Missing header! (quest designers)";

/// One row's grouping/ordering inputs — everything [`order_groups`] needs, and nothing else, so
/// the display order is testable without a Bevy world.
struct GroupRow {
    zone_or_sort: i32,
    /// The resolved header name for `zone_or_sort` — [`MISSING_HEADER`] for `0`, and the empty
    /// string for an id neither `AreaTable` nor `QuestSort` names (the reference's lookup simply
    /// finds no string there, and collates it as `""`).
    header: String,
    level: u32,
    title: String,
    slot: u8,
}

/// **The quest log's display order** (rebuild `0x4de510`, group sort `0x4de751`/`0x4de8f0`, row
/// comparator `0x4deac0`). Returns the header groups in order, each with its rows' indices in
/// order.
///
/// - **Groups**: one per distinct `ZoneOrSort`, and `id == 0` is forced **FIRST**, before any name
///   lookup (`0x4de913`). An id that names no row collates as `""` and lands next. The rest go by
///   resolved name through the client's collator `0x64a4c0`, which is **case-INSENSITIVE**
///   (`_strnicmp 0x414310`) — a byte-wise `cmp` puts `"Elwynn Forest"` and `"eastern kingdoms"` in
///   the wrong relative place.
/// - **Within a group**: **level ASC** (`0x4decd9`), then **title** through that same collator
///   (`0x4decfb`). Descriptor-slot order — what this used to do, and what benilla claimed — was
///   REFUTED: `0x4deac0` reads `entry+0x4` into a stack slot it never uses.
/// - The reference's `qsort` is **unstable**, so a full (level, title) tie is unspecified there.
///   We break it on slot, which is deterministic and so cannot contradict an unspecified order.
fn order_groups(rows: &[GroupRow]) -> Vec<(i32, String, Vec<usize>)> {
    let mut groups: Vec<(i32, String, Vec<usize>)> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        match groups.iter_mut().find(|(z, _, _)| *z == r.zone_or_sort) {
            Some((_, _, idxs)) => idxs.push(i),
            None => groups.push((r.zone_or_sort, r.header.clone(), vec![i])),
        }
    }
    // `id == 0` first (rank 0), then everything else by case-folded name — `""` for an
    // unresolvable id, so it sorts immediately after.
    groups.sort_by_key(|(zos, name, _)| {
        if *zos == 0 {
            (0u8, String::new())
        } else {
            (1, name.to_lowercase())
        }
    });
    for (_, _, idxs) in &mut groups {
        idxs.sort_by_key(|&i| {
            let r = &rows[i];
            (r.level, r.title.to_lowercase(), r.slot)
        });
    }
    groups
}

/// Read the self player's `PLAYER_QUEST_LOG` descriptor slots each frame, resolve entries/detail,
/// and push a [`QuestLogState`] snapshot on change (diffed against a `Local`, the crate's standard
/// feed shape). Also refreshes [`QuestLog`]'s `row_slots` for the abandon drain.
fn feed_quest_log(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    mut quest_log: ResMut<QuestLog>,
    names: Res<NameCache>,
    // The bag walk behind an item objective's carried count (2334).
    objects: crate::net::Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    header_names: Option<Res<QuestHeaderNamesRes>>,
    tag_names: Option<Res<QuestTagNamesRes>>,
    states: Res<crate::world_state::WorldStates>,
    spells: Option<Res<Spells>>,
    // The completion toasts are message-catalog rows, so they go out through the one sink that
    // reads the row's surface and sound (`crate::ui_action::show_messages`).
    mut sink: crate::ui_action::MessageSink,
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

    // The player's own `GlobalStrings.lua`, for the leaderboard's three format keys (2045). Taken
    // here, above the build, because everything between this and the push below reads the VM only.
    let get = |key: &str| {
        script
            .lua()
            .globals()
            .get::<String>(key)
            .ok()
            .filter(|t| !t.is_empty())
    };

    // ── The IN-FLIGHT SKIP — the cached rows, and only those ────────────────────────────────────
    // The reference's rebuild (`0x4de510`) counts cache misses into `[0xbb7498]` and skips the
    // MISSING QUEST — not the pass. The miss at `0x4de67e` is a `jmp 0x4de729`, which is the slot
    // loop's own increment block, one instruction above the back-edge: every later slot is still
    // visited and every later cache hit still appends its row at `0x4de68e`. The hit path's own
    // "this group already has a header" branch (`0x4de6f1`) jumps to the same address right after
    // appending a row, which a discard could not do. The tail past the loop is unconditional —
    // both sorts, the collapse restore, the watch prune, and `QUEST_LOG_UPDATE` (`0x4de811`).
    // `0x4de545` then makes any FURTHER rebuild a no-op until the last outstanding response
    // decrements the counter to 0 and `0x4de8b0` rebuilds once more.
    //
    // So `GetNumQuestLogEntries()` answers `(visible rows, CACHED quest count)`, and `0, 0` is the
    // all-cold extreme rather than the general law. Skipping the whole log on any miss — what this
    // did, reading the cold case as the rule — blanked the entire window for a round
    // trip every time a quest whose template we had never seen entered the log. That took the
    // engine selection with it (`remap_selection` reads an empty list as "your quest is gone"), so
    // the detail pane jumped to row 1 on every single pickup, and addons reading the log across
    // the blank saw a log that briefly held nothing (decision 2256).
    //
    // `template()` is called for EVERY row before any filtering, because the miss is what SENDS
    // the query (our `pending` set is the reference's once-per-id dedupe) — short-circuiting would
    // leave the rest of a cold log unrequested, and then nothing would arrive to fill it in.
    let cached: Vec<bool> = rows
        .iter()
        .map(|r| quest_log.template(r.quest_id, &commands).is_some())
        .collect();
    let rows: Vec<Row> = rows
        .into_iter()
        .zip(cached)
        .filter_map(|(r, hit)| hit.then_some(r))
        .collect();

    // ── Section headers (the ref's zone/sort groups) ────────────────────────────────────────────
    // One header per distinct `ZoneOrSort` (positive → AreaTable zone, negative → QuestSort sort —
    // [`benilla_formats::QuestHeaderNames`]), in first-appearance order, then sorted. Every row
    // here has a cached template, so `zone_or_sort` is always the real one — the gate above is
    // what makes that true.
    //
    // **The group order** (`0x4de751`, comparator `0x4de8f0`): `id == 0` is forced FIRST, before
    // any name lookup (`0x4de913`); an id that names no row collates as `""` and lands next; the
    // rest are ordered by resolved name through the client's collator `0x64a4c0`, which is
    // **case-INSENSITIVE** (`_strnicmp 0x414310`) — not the byte `cmp` we had.
    //
    // **`id == 0` renders as the literal `"Missing header! (quest designers)"`** (`0x84b4ec`, its
    // only reference image-wide). That string is a **designer-data tell, not a transient**: the
    // only value that can become a header id is a CACHED template's `zoneOrSort`, so it cannot
    // appear while anything is in flight. Which is precisely why it is safe to render — and why it
    // could not have been, before the gate above existed.
    let ordered = {
        let keyed: Vec<GroupRow> = rows
            .iter()
            .map(|r| {
                let t = quest_log.templates.get(r.quest_id);
                let zos = t.map(|t| t.zone_or_sort).unwrap_or(0);
                GroupRow {
                    zone_or_sort: zos,
                    header: if zos == 0 {
                        MISSING_HEADER.to_string()
                    } else {
                        header_names
                            .as_ref()
                            .and_then(|h| h.0.resolve(zos))
                            .unwrap_or_default()
                            .to_string()
                    },
                    level: t.map(|t| t.level).unwrap_or(0),
                    title: t.map(|t| t.title.clone()).unwrap_or_default(),
                    slot: r.slot,
                }
            })
            .collect();
        order_groups(&keyed)
    };
    let groups = ordered;

    // Above the row loop because every row's detail substitutes through it now (the chat macros
    // in a quest's description/objectives paragraph), not just the selection's.
    let player = crate::npc_text::player_identity(&self_q, &names, &commands);
    let macros = crate::npc_text::MacroContext {
        subject: player.as_ref(),
        states: &states,
    };

    let mut entries: Vec<QuestLogEntryView> = Vec::new();
    let mut header_keys: Vec<Option<String>> = Vec::new();
    // The quests folded under a collapsed header: out of the visible list, still in the log. The
    // engine's watch prune counts them, as the reference's does (`0x4de7a7`–`0x4de80f` scans the
    // whole row array, hidden rows included); leaving
    // them out made every collapse drop its quests' watches for good.
    let mut hidden_quest_ids: Vec<u32> = Vec::new();
    for (_, name, row_idxs) in &groups {
        let collapsed = quest_log.collapsed.contains(name);
        entries.push(QuestLogEntryView {
            quest_id: 0,
            title: name.clone(),
            level: 0,
            tag: None, // a header names a zone/sort, never a quest — it can carry no tag
            is_header: true,
            collapsed,
            complete: 0,
            timer: 0,        // a header row is not a quest and never carries a timer
            pushable: false, // nor is it shareable — there is no quest under the row
            objectives: Vec::new(),
            detail: None, // a header names a zone/sort; there is no quest to describe
        });
        header_keys.push(Some(name.clone()));
        for &ri in row_idxs {
            let r = &rows[ri];
            if collapsed {
                // Folded: the quest stays in the log, just not in the visible list.
                hidden_quest_ids.push(r.quest_id);
                continue;
            }
            let (title, level, tag, pushable, objectives, detail) =
                match quest_log.template(r.quest_id, &commands) {
                    Some(t) => (
                        t.title.clone(),
                        t.level,
                        // The `(Elite)`/`(Dungeon)`/`(Raid)`/`(PvP)` suffix: the template's `Type` is
                        // a `QuestInfo.dbc` row id ([`benilla_formats::QuestTagNames`]), and `Type`
                        // 0 — the untagged majority — names no row. Absent table (no client data) =
                        // no tag, never an empty string: the row Lua branches on presence.
                        tag_names
                            .as_ref()
                            .and_then(|tags| tags.0.resolve(t.quest_type))
                            .map(str::to_string),
                        // `QUEST_FLAGS_SHARABLE` (vmangos `QuestDef.h:153`) — the whole of the
                        // shareable test, and it is the CLIENT's alone: vmangos's
                        // `HandlePushQuestToParty` never checks the bit, it only re-tests it on the
                        // receiver's accept (`Player::CanShareQuest`). So an unshareable quest whose
                        // button we wrongly enabled would push, and the party would get a detail panel
                        // for a quest the server then refuses (decision 1733).
                        t.flags & quest_flags::SHARABLE != 0,
                        build_objectives(
                            t,
                            &r.log_slot,
                            &store.0,
                            &objects,
                            &items,
                            &names,
                            &commands,
                            &get,
                        ),
                        // Every row, not just the selection (decision 2247) — the detail bindings
                        // resolve the live selection against these at call time, the way the
                        // reference peeks its quest cache inside the call.
                        Some(build_detail(
                            t,
                            &items,
                            icons.as_deref(),
                            &commands,
                            &macros,
                            spells.as_deref(),
                        )),
                    ),
                    // Unreachable: the in-flight gate above emptied `rows` unless EVERY template is
                    // cached, which is the reference's own all-or-nothing rebuild. This arm used to
                    // paint a `"..."` placeholder row, which the reference never does (`0x4de67e`).
                    None => unreachable!("the in-flight gate leaves only cached rows"),
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
                tag,
                is_header: false,
                collapsed: false,
                complete,
                pushable,
                // The slot's raw deadline (absolute unix seconds; 0 = untimed) — carried, never
                // converted here. The countdown is subtracted per Lua call against the server
                // clock, so this snapshot stays stable while the number on screen ticks
                // (decision 1150).
                timer: r.log_slot.timer,
                objectives,
                detail,
            });
            header_keys.push(None);
        }
    }

    // ── Selection remap ─────────────────────────────────────────────────────────────────────────
    let sel = script.quest_log_selection();
    let new_sel = remap_selection(&last.entries, &entries, sel);
    if new_sel != sel {
        script.set_quest_log_selection(new_sel);
    }
    // Every cached row, folded or not — the table `AbandonQuest` searches by quest id.
    quest_log.row_slots = rows.iter().map(|r| (r.quest_id, r.slot)).collect();
    quest_log.header_keys = header_keys;

    let fresh = QuestLogState {
        entries,
        num_quests: rows.len() as u32,
        hidden_quest_ids,
    };
    if fresh == *last {
        return;
    }
    // The objective-progress announces (the quest-update handler
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
        for quest in progressed {
            for line in quest.changed_lines {
                script.fire_event("UI_INFO_MESSAGE", vec![ScriptValue::Str(line)]);
            }
            if quest.completed {
                // The 0x198 pair — msgId `0xf8` `ERR_QUEST_OBJECTIVE_COMPLETE_S` when
                // the handler has the objective text, `0xf9` `ERR_QUEST_UNKNOWN_COMPLETE` when it
                // does not (`0x5e5d12`; never `ERR_QUEST_COMPLETE_S`, which is the turn-in's own
                // call site).
                // Both are catalog rows, so the surface and the sound come from there rather than
                // from a hand-picked `fire_event` here (decisions 1770/1815).
                let line = if quest.title.is_empty() {
                    crate::ui_action::keyed_line(&script, "ERR_QUEST_UNKNOWN_COMPLETE")
                } else {
                    crate::ui_action::keyed_line_s(
                        &script,
                        "ERR_QUEST_OBJECTIVE_COMPLETE_S",
                        &[&quest.title],
                    )
                };
                crate::ui_action::show_messages(&mut script, &mut sink, "ui_quest_log", line);
            }
            // QUEST_WATCH_UPDATE's arg1 is the quest's LOG index: stock QuestLog_OnEvent hands it
            // to AutoQuestWatch_Update(questIndex), whose end is AddQuestWatch(questIndex)
            // (QuestLogFrame.lua:68, 752-769). Ours used to pass the watch-list position and
            // fire a BENILLA_QUEST_PROGRESS twin carrying the log index; the twin went with the
            // window (1944).
            script.fire_event(
                "QUEST_WATCH_UPDATE",
                vec![ScriptValue::Int(i64::from(quest.index))],
            );
        }
    }
    *last = fresh;
}

/// Did one objective **advance** between two log states — the announce predicate, and the whole of
/// decision 1152's fix for B237.
///
/// The reference's law (handler
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
                index: i as u32 + 1,
                title: e.title.clone(),
                changed_lines,
                completed,
            })
        })
        .collect()
}

/// Drain the confirmed abandons (the quest id marked at click time — see
/// `benilla_ui::script::quest_log`'s module doc) and map each to `CMSG_QUESTLOG_REMOVE_QUEST` via
/// this frame's `row_slots`.
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

/// The quest `Type → tag name` lookup ([`benilla_formats::QuestTagNames`] — `QuestInfo.dbc`),
/// loaded once at startup. Absent when the client data didn't load; the feed then pushes no tag,
/// which is what an untagged quest pushes anyway.
#[derive(Resource)]
pub(crate) struct QuestTagNamesRes(pub benilla_formats::QuestTagNames);

/// Startup: read the `QuestInfo.dbc` tag names through the patch chain (the header-names load's
/// exact shape).
fn load_quest_tag_names(mut commands: Commands, assets: Option<Res<benilla_assets::WorldAssets>>) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_quest_tag_names(&mut chain)
    };
    match loaded {
        Ok(names) => commands.insert_resource(QuestTagNamesRes(names)),
        Err(e) => warn!("ui_quest_log: quest tag names failed to load: {e:#}"),
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

/// `QuestLogPushQuest()` — the *Share Quest* button (decision 1733). The engine already resolved
/// the selection to a quest id at click time, so this is a straight relay; the server addresses the
/// party itself.
fn drain_quest_log_pushes(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for quest in script.take_quest_log_pushes() {
        debug!("ui_quest_log: sharing quest {quest} with the party");
        let _ = commands.0.send(ClientCommand::PushQuestToParty { quest });
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
    for quest in script.take_quest_log_abandons() {
        match abandon_slot(quest, &quest_log.row_slots) {
            Some(slot) => {
                debug!("ui_quest_log: abandon quest {quest} → slot {slot}");
                let _ = commands.0.send(ClientCommand::QuestlogRemove { slot });
            }
            None => debug!("ui_quest_log: abandon quest {quest} is not in the log — ignored"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── order_groups — the reference's display order ────────────────────────────────────────────

    fn grow(zos: i32, header: &str, level: u32, title: &str, slot: u8) -> GroupRow {
        GroupRow {
            zone_or_sort: zos,
            header: header.into(),
            level,
            title: title.into(),
            slot,
        }
    }

    /// Group order: `id == 0` first regardless of its name, then an unnamed id (`""`), then the
    /// rest by CASE-INSENSITIVE name — the client's collator is `_strnicmp`, so a byte-wise
    /// compare (which is what this used to do) puts an all-lowercase name in the wrong place.
    #[test]
    fn groups_put_id_zero_first_then_unnamed_then_case_folded_names() {
        let rows = vec![
            grow(12, "Elwynn Forest", 1, "a", 0),
            grow(-3, "warlock", 1, "b", 1), // lowercase: byte-cmp would sort it after "Westfall"
            grow(0, MISSING_HEADER, 1, "c", 2),
            grow(40, "Westfall", 1, "d", 3),
            grow(999, "", 1, "e", 4), // an id neither table names
        ];
        let order: Vec<i32> = order_groups(&rows).into_iter().map(|(z, _, _)| z).collect();
        // `warlock` before `Westfall` is the discriminating pair: case-folded, `wa` < `we`; by
        // BYTES, `W`(0x57) < `w`(0x77) puts `Westfall` first. Everything else about this list is
        // the same either way, which is why the old byte-wise sort looked fine.
        assert_eq!(order, vec![0, 999, 12, -3, 40]);
    }

    /// The `id == 0` header carries the reference's own literal — a designer-data tell, and
    /// reachable only because a row is never listed before its template is cached.
    #[test]
    fn the_zero_header_is_the_references_literal() {
        let rows = vec![grow(0, MISSING_HEADER, 1, "a", 0)];
        let groups = order_groups(&rows);
        assert_eq!(groups[0].1, "Missing header! (quest designers)");
    }

    /// Within a group: level ASC, then title (case-insensitively) — NOT descriptor-slot order,
    /// which is what benilla claimed and `0x4deac0` refuted.
    #[test]
    fn within_a_group_it_is_level_then_title_not_slot_order() {
        // Deliberately seeded in the reverse of the expected order by slot.
        let rows = vec![
            grow(12, "Elwynn Forest", 9, "Zzz", 0),
            grow(12, "Elwynn Forest", 3, "beta", 1),
            grow(12, "Elwynn Forest", 3, "Alpha", 2),
            grow(12, "Elwynn Forest", 1, "Anything", 3),
        ];
        let groups = order_groups(&rows);
        assert_eq!(groups.len(), 1);
        let titles: Vec<&str> = groups[0]
            .2
            .iter()
            .map(|&i| rows[i].title.as_str())
            .collect();
        // level 1, then the two level-3s by case-folded title ("Alpha" < "beta" — a byte compare
        // would put uppercase "Alpha" before "beta" too, so the discriminating pair is below),
        // then level 9.
        assert_eq!(titles, vec!["Anything", "Alpha", "beta", "Zzz"]);
    }

    /// The case-folding is load-bearing in its own right: `"apple"` must precede `"Banana"`, which
    /// a byte-wise compare gets wrong (uppercase `B` = 0x42 sorts before lowercase `a` = 0x61).
    #[test]
    fn title_order_is_case_insensitive() {
        let rows = vec![
            grow(12, "Zone", 5, "apple", 0),
            grow(12, "Zone", 5, "Banana", 1),
        ];
        let groups = order_groups(&rows);
        let titles: Vec<&str> = groups[0]
            .2
            .iter()
            .map(|&i| rows[i].title.as_str())
            .collect();
        assert_eq!(titles, vec!["apple", "Banana"]);
    }

    // ── remap_selection ─────────────────────────────────────────────────────────────────────────

    fn header(title: &str) -> QuestLogEntryView {
        QuestLogEntryView {
            detail: None,
            quest_id: 0,
            title: title.into(),
            level: 0,
            tag: None,
            is_header: true,
            collapsed: false,
            complete: 0,
            timer: 0,
            pushable: false,
            objectives: Vec::new(),
        }
    }

    fn quest(id: u32) -> QuestLogEntryView {
        QuestLogEntryView {
            detail: None,
            quest_id: id,
            title: format!("Quest {id}"),
            level: 1,
            tag: None,
            is_header: false,
            collapsed: false,
            complete: 0,
            timer: 0,
            pushable: false,
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

    // ── creature_line / item_line ───────────────────────────────────────────────────────────────

    /// A stand-in string table whose three templates are **deliberately not** the shipped wording.
    ///
    /// That is the point, and it is decision 2045's: in enUS `QUEST_MONSTERS_KILLED` and
    /// `ERR_QUEST_ADD_KILL_SII` are the same sentence, and `QUEST_OBJECTS_FOUND`,
    /// `QUEST_ITEMS_NEEDED` and their two `ERR_QUEST_ADD_*` twins are the same four — so an
    /// assertion on English cannot tell a right key from a wrong one. These name the key in the
    /// rendered line, so each case below asserts *which lookup happened*. The real wording is
    /// checked against the player's own table by
    /// [`the_leaderboard_keys_resolve_in_the_real_global_strings`].
    fn probe_strings(key: &str) -> Option<String> {
        match key {
            "QUEST_MONSTERS_KILLED" => Some("<KILLED %s %d of %d>".into()),
            "QUEST_OBJECTS_FOUND" => Some("<FOUND %s %d of %d>".into()),
            "QUEST_ITEMS_NEEDED" => Some("<NEEDED %s %d of %d>".into()),
            _ => None,
        }
    }

    #[test]
    fn creature_objective_with_resolved_name() {
        let o = obj(100, 10, 0, 0, "");
        let line = creature_line(&o, 3, Some("Kobold Vermin"), &probe_strings).unwrap();
        assert_eq!(line.text, "<KILLED Kobold Vermin 3 of 10>");
        assert_eq!(line.kind, "monster");
        assert!(!line.finished);
    }

    #[test]
    fn creature_objective_in_flight_shows_a_placeholder() {
        let o = obj(100, 10, 0, 0, "");
        let line = creature_line(&o, 0, None, &probe_strings).unwrap();
        assert_eq!(line.text, "<KILLED ... 0 of 10>");
    }

    /// A GameObject objective with no name takes `QUEST_OBJECTS_FOUND` on the placeholder — the
    /// binding fetches that key once, ABOVE its cache test (`0x4e031e`), so both its legs share
    /// it. Before decision 2045 this composed an invented `"Objective: %d/%d"` line, which is a
    /// sentence 1.12 never says.
    #[test]
    fn go_objective_falls_back_without_a_name_cache() {
        let go_id = ((-57i32) as u32) | 0x8000_0000;
        let o = obj(go_id, 1, 0, 0, "");
        let line = creature_line(&o, 1, None, &probe_strings).unwrap();
        assert_eq!(line.text, "<FOUND ... 1 of 1>");
        assert_eq!(line.kind, "object");
        assert!(line.finished); // cur (1) >= req (1)
    }

    #[test]
    fn go_objective_prefers_its_custom_text_over_the_fallback() {
        let go_id = ((-57i32) as u32) | 0x8000_0000;
        let o = obj(go_id, 4, 0, 0, "Destroy the barricade");
        let line = creature_line(&o, 1, None, &probe_strings).unwrap();
        assert_eq!(line.text, "<FOUND Destroy the barricade 1 of 4>");
        assert_eq!(line.kind, "object");
    }

    #[test]
    fn item_objective_counts_bags_and_clamps_to_required() {
        let o = obj(0, 0, 2000, 5, "");
        // 8 in the bags, but required is only 5 — the line clamps, doesn't overshoot.
        let line = item_line(&o, 0, Some("Kobold Ear"), 8, &probe_strings).unwrap();
        assert_eq!(line.text, "<NEEDED Kobold Ear 5 of 5>");
        assert_eq!(line.kind, "item");
        assert!(line.finished);
    }

    #[test]
    fn item_objective_in_flight_shows_a_placeholder() {
        let o = obj(0, 0, 2000, 5, "");
        let line = item_line(&o, 0, None, 2, &probe_strings).unwrap();
        assert_eq!(line.text, "<NEEDED ... 2 of 5>");
    }

    /// **The key fork**, and the one an English assertion could never see: a creature
    /// objective carrying `ObjectiveText[i]` stops being "slain" — the override IS the name and
    /// the format becomes `QUEST_OBJECTS_FOUND` (`mov ecx,0x84b634` @`0x4e03e1`) — while `type`
    /// still answers `"monster"`, because that is read from a fresh sign test of
    /// `ReqCreatureOrGOId[i]`.
    #[test]
    fn custom_text_overrides_the_creature_auto_line() {
        let o = obj(100, 10, 0, 0, "Slay the vermin");
        let line = creature_line(&o, 4, Some("Kobold Vermin"), &probe_strings).unwrap();
        assert_eq!(line.text, "<FOUND Slay the vermin 4 of 10>");
        assert_eq!(line.kind, "monster");
    }

    /// The three keys against the player's REAL `GlobalStrings.lua` — the guard a key assertion
    /// needs beside it, since a typo'd key degrades a line to empty rather than to a wrong
    /// sentence. Skips without client data.
    #[test]
    fn the_leaderboard_keys_resolve_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let vm = UiScript::new().expect("VM");
        vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let get = |key: &str| {
            vm.lua()
                .globals()
                .get::<String>(key)
                .ok()
                .filter(|t| !t.is_empty())
        };
        for key in [
            "QUEST_MONSTERS_KILLED",
            "QUEST_OBJECTS_FOUND",
            "QUEST_ITEMS_NEEDED",
        ] {
            assert!(get(key).is_some(), "{key} missing");
        }
        let o = obj(100, 10, 0, 0, "");
        let line = creature_line(&o, 3, Some("Kobold Vermin"), &get).unwrap();
        assert_eq!(line.text, "Kobold Vermin slain: 3/10");
    }

    /// Decision 1158, correcting 0109: a line's `finished` is `cur >= req` and **nothing else**.
    /// The reference reads the whole-quest COMPLETE bit exactly once in `GetQuestLogLeaderBoard`
    /// (`0x4e02a2`), in the `event` branch, where it is that line's only predicate — it is never
    /// or-ed into a counted line's verdict. Only the whole-quest turn-in predicate `0x4df580` reads
    /// the bit and the counts together.
    #[test]
    fn a_complete_quest_does_not_mark_a_short_objective_finished() {
        let o = obj(100, 10, 0, 0, "");
        // The counter (1) is well short of required (10). Whatever the quest's state byte says,
        // THIS line is not finished.
        let line = creature_line(&o, 1, Some("Kobold"), &probe_strings).unwrap();
        assert_eq!(line.text, "<KILLED Kobold 1 of 10>");
        assert!(!line.finished);
    }

    #[test]
    fn empty_objective_slot_emits_no_line() {
        let o = obj(0, 0, 0, 0, "");
        assert!(creature_line(&o, 0, None, &probe_strings).is_none());
        assert!(item_line(&o, 0, None, 0, &probe_strings).is_none());
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
        let items = Items::default();
        // Empty bags — every item objective here reads 0 owned (2334).
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            &store,
            &objects,
            &items,
            &names,
            &commands,
            &probe_strings,
        );

        // The two unused quads emit nothing; the two real ones resolve in order — creature/item
        // names are unresolved (ask-once mid-flight, no answer seeded) so they fall to the same
        // placeholder path the line builders' own tests already cover directly.
        assert_eq!(objectives.len(), 2);
        assert_eq!(objectives[0].text, "<KILLED ... 3 of 10>");
        assert_eq!(objectives[0].kind, "monster");
        assert!(!objectives[0].finished);
        assert_eq!(objectives[1].text, "<NEEDED ... 0 of 5>");
        assert_eq!(objectives[1].kind, "item");
    }

    /// **The bug this split exists for** (director, 2026-09-15): the reference walks
    /// `ReqCreatureOrGOId[0..3]` (`0x4e02d7`) and `ReqItemId[0..3]` (`0x4e04f4`) as two
    /// INDEPENDENT passes, so one objective quad carrying both kinds produces two lines. Merging
    /// them per quad returned on the creature branch and dropped the item outright.
    ///
    /// The fixture is quest 358 "Graverobbers"' own shape — `ReqCreatureOrGOId1` 1941 ×8 and
    /// `ReqItemId1` 2834 ×8 both at index 0, plus a second creature at index 1 — which showed 2
    /// lines where the reference shows 3.
    #[test]
    fn a_quad_carrying_both_a_creature_and_an_item_emits_both_lines() {
        let template = quest_template([
            obj(1941, 8, 2834, 8, ""), // BOTH kinds, same index
            obj(1675, 5, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
        ]);
        let log_slot = QuestLogSlot {
            quest_id: 358,
            counters: [8, 1, 0, 0],
            state: 0,
            timer: 0,
        };
        let store = ObjectFields::default();
        let items = Items::default();
        // Empty bags — every item objective here reads 0 owned (2334).
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            &store,
            &objects,
            &items,
            &names,
            &commands,
            &probe_strings,
        );

        // Three lines, in the reference's pass order: both creatures, THEN the item — not the
        // quad order, which would interleave them.
        assert_eq!(objectives.len(), 3, "{objectives:#?}");
        assert_eq!(objectives[0].text, "<KILLED ... 8 of 8>");
        assert_eq!(objectives[1].text, "<KILLED ... 1 of 5>");
        assert_eq!(objectives[2].text, "<NEEDED ... 0 of 8>");
        assert_eq!(objectives[2].kind, "item");
    }

    /// The override buffer belongs to the CREATURE branch (`0x4e03da`) — on a shared quad it must
    /// not leak into the item line, which takes its name from the item cache alone. The split made
    /// this reachable for the first time, so it is asserted rather than assumed.
    #[test]
    fn a_shared_quads_objective_text_names_the_creature_not_the_item() {
        let template = quest_template([
            obj(1941, 8, 2834, 8, "Graverobbers routed"),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
        ]);
        let log_slot = QuestLogSlot {
            quest_id: 358,
            counters: [8, 0, 0, 0],
            state: 0,
            timer: 0,
        };
        let store = ObjectFields::default();
        let items = Items::default();
        // Empty bags — every item objective here reads 0 owned (2334).
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            &store,
            &objects,
            &items,
            &names,
            &commands,
            &probe_strings,
        );
        assert_eq!(objectives.len(), 2);
        // The override IS the creature's name, and flips its key to QUEST_OBJECTS_FOUND
        // (`0x4e03e1`).
        assert_eq!(objectives[0].text, "<FOUND Graverobbers routed 8 of 8>");
        // The item line wears the in-flight placeholder, never the creature's text.
        assert_eq!(objectives[1].text, "<NEEDED ... 0 of 8>");
    }

    /// `ReqItemId[i] != SrcItemId` (`0x4e00ce`/`0x4e04fa`): the quest's own provided/starter item
    /// is excluded from the board even when it also sits in the required array.
    #[test]
    fn the_quests_own_source_item_is_not_a_leaderboard_line() {
        let mut template = quest_template([
            obj(0, 0, 2834, 8, ""),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
        ]);
        template.src_item_id = 2834;
        let log_slot = QuestLogSlot {
            quest_id: 1,
            counters: [0; 4],
            state: 0,
            timer: 0,
        };
        let store = ObjectFields::default();
        let items = Items::default();
        // Empty bags — every item objective here reads 0 owned (2334).
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            &store,
            &objects,
            &items,
            &names,
            &commands,
            &probe_strings,
        );
        assert!(objectives.is_empty(), "{objectives:#?}");
    }

    /// The 0109-era behaviour this inverts: a COMPLETE slot state used to mark every line
    /// finished. Decision 1158 — the reference's per-line verdict never reads that bit
    /// (`0x4e0110`); only the whole-quest turn-in predicate (`0x4df580`) combines the two.
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
        let items = Items::default();
        // Empty bags — every item objective here reads 0 owned (2334).
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            &store,
            &objects,
            &items,
            &names,
            &commands,
            &probe_strings,
        );
        assert_eq!(objectives.len(), 1);
        assert!(
            !objectives[0].finished,
            "the counter is 1/10 — the quest's COMPLETE bit is not this line's business"
        );
        assert_eq!(objectives[0].text, "<KILLED ... 1 of 10>");
    }

    // ── money_split ──────────────────────────────────────────────────────────────────────────────

    #[test]
    fn money_sign_splits_into_required_or_reward() {
        assert_eq!(money_split(-500), (500, 0));
        assert_eq!(money_split(500), (0, 500));
        assert_eq!(money_split(0), (0, 0));
    }

    // ── abandon_slot: quest id → descriptor slot across gaps ───────────────────────────────────────

    #[test]
    fn abandon_maps_quest_id_to_slot_across_a_gap() {
        // Slots 0 and 2 (slot 1 empty in the descriptor array) — the id finds its own slot.
        let row_slots = vec![(783u32, 0u8), (7, 2)];
        assert_eq!(abandon_slot(783, &row_slots), Some(0));
        assert_eq!(abandon_slot(7, &row_slots), Some(2));
        assert_eq!(abandon_slot(8, &row_slots), None); // not in the log: nothing to remove
        assert_eq!(abandon_slot(0, &row_slots), None); // 0 is never a quest
    }

    // ── The abandon mark is a QUEST ID (0x4dfb50 / 0x4df070) ─────────────────────────────────────

    /// **An abandon names a quest, not a row.** `SetAbandonQuest` (`0x4dfb50`) copies the
    /// selection `0xbb7480` — which holds the selected row's QUEST ID (`0x4def30`, `0x4def5d`) —
    /// into the mark `0xbb7484`; `AbandonQuest` (`0x4dfe00` → `0x4df070`) searches the row table
    /// `0xbb71c0` for that id and removes that row's slot. So a log that re-indexes while the
    /// confirm popup is up — a fold, a quest arriving or leaving — cannot move the abandon onto a
    /// neighbour. Ours pinned the 1-based row index and resolved it against the rows current at
    /// confirm time: fold the group above and the popup's Yes abandoned the quest below.
    #[test]
    fn a_fold_between_mark_and_confirm_does_not_retarget_the_abandon() {
        use benilla_protocol::messages::field::FIELD_PLAYER_QUEST_LOG_1_1;

        // Z1 = zone 0 ("Missing header!", forced first) holding A; Z2 = zone 7 holding B and C.
        // Entries: [Z1, A, Z2, B, C] — B is row 4.
        const A: u32 = 101;
        const B: u32 = 201;
        const C: u32 = 202;
        let quests = [(0u8, A, 0i32, "A"), (1, B, 7, "B"), (2, C, 7, "C")];

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx));
        app.init_resource::<crate::net::GuidIndex>()
            .init_resource::<NameCache>()
            .init_resource::<Items>()
            .init_resource::<crate::world_state::WorldStates>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>();
        let mut log = QuestLog::default();
        let mut pairs = Vec::new();
        for (slot, id, zos, title) in quests {
            let mut t = quest_template(std::array::from_fn(|_| obj(0, 0, 0, 0, "")));
            t.quest_id = id;
            t.zone_or_sort = zos;
            t.title = title.into();
            log.insert_template(t);
            pairs.push((FIELD_PLAYER_QUEST_LOG_1_1 + 3 * u16::from(slot), id));
        }
        app.insert_resource(log);
        app.world_mut().spawn((
            ObjectStore(ObjectFields::from_pairs(&pairs)),
            Guid(0x2A),
            SelfPlayer,
        ));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(
            Update,
            (
                drain_quest_log_collapses,
                feed_quest_log,
                drain_quest_log_abandons,
            )
                .chain(),
        );
        let run = |app: &mut App, lua: &str| {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .run(lua)
                .unwrap();
        };

        app.update();
        run(&mut app, "SelectQuestLogEntry(4); SetAbandonQuest()");
        assert_eq!(
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<String>("return GetAbandonQuestName()")
                .unwrap(),
            "B"
        );
        // The popup is up; the player folds Z1 — B moves from row 4 to row 3, C takes row 4.
        run(&mut app, "CollapseQuestHeader(1)");
        app.update();
        run(&mut app, "AbandonQuest()");
        app.update();

        let sent: Vec<_> = rx
            .try_iter()
            .filter(|c| matches!(c, ClientCommand::QuestlogRemove { .. }))
            .collect();
        assert!(
            matches!(sent.as_slice(), [ClientCommand::QuestlogRemove { slot: 1 }]),
            "the abandon removes B's slot (1), never C's (2): {sent:?}"
        );
    }

    // ── quests_with_progressed_objectives: the QUEST_WATCH_UPDATE per-quest trigger ─────────────────

    /// A one-objective entry built the way [`creature_line`] builds one: the `cur`/`req` carried
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
            hidden_quest_ids: Vec::new(),
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
