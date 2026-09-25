//! The quest log's app side: reads the player's `PLAYER_QUEST_LOG` slots every frame, resolves
//! each quest through the `SMSG_QUEST_QUERY_RESPONSE` cache and pushes the log, every quest with
//! its objective lines, firing `QUEST_LOG_UPDATE` on a change. An advanced objective also toasts
//! its line and fires `QUEST_WATCH_UPDATE`. Abandons, shares and header folds drain from here.

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

/// Marks a GameObject objective's entry on the wire (vmangos `Quest.cpp:512-516`).
const GO_OBJECTIVE_BIT: u32 = 0x8000_0000;

/// One occupied `PLAYER_QUEST_LOG` slot this frame.
struct Row {
    slot: u8,
    quest_id: u32,
    log_slot: QuestLogSlot,
}

/// The template cache, asked once per quest id, and this frame's quests as `(quest id, slot)`,
/// folded ones included: the reference's row table `0xbb71c0`, which an abandon searches.
#[derive(Resource, Default)]
pub(crate) struct QuestLog {
    templates: QueryCache<u32, QuestTemplate>,
    row_slots: Vec<(u32, u8)>,
    /// The collapsed headers, by title.
    collapsed: HashSet<String>,
    /// Per pushed entry, the header's title, `None` for a quest: the fold drain's index map.
    header_keys: Vec<Option<String>>,
}

impl crate::query_cache::AskOnce for QuestLog {
    fn clear_pending(&mut self) {
        self.templates.clear_pending();
    }
}

impl QuestLog {
    /// The template, asked once: a miss sends `CMSG_QUEST_QUERY` (once while in flight); an id
    /// the server does not know stays `None` without a re-ask.
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

    /// Disconnect: drops everything, the templates included.
    pub(crate) fn clear_session(&mut self) {
        self.templates.clear();
        self.row_slots.clear();
        self.collapsed.clear();
        self.header_keys.clear();
    }
}

/// The quest log's packet handlers.
mod net {
    use benilla_protocol::messages::QuestTemplate;
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::QuestLog;
    use crate::net::NetHandlerApp;

    /// Registers the template handler and the session-end listener.
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

    /// The log's state dies with the socket: a second handler on the kind, after the bridge's own.
    fn on_session_end(In(_): In<SessionEvent>, mut quest_log: ResMut<QuestLog>) {
        quest_log.clear_session();
    }

    /// `SMSG_QUEST_QUERY_RESPONSE`, the answer to our `CMSG_QUEST_QUERY`, cached by quest id.
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
                    // Before the script tick, so `QuestTimerFrame` reads this frame's clock.
                    feed_server_clock.in_set(UiFeed),
                    drain_quest_log_abandons.after(UiInput),
                    drain_quest_log_pushes.after(UiInput),
                    drain_quest_log_collapses.after(UiInput),
                ),
            );
    }
}

/// Deviation: an in-flight name reads `...`, where the reference's reads `" "` (`0x82ee00`),
/// because a blank looks like a nameless objective.
const NAME_PLACEHOLDER: &str = "...";

/// One leaderboard line from its `%s`/`%d`/`%d` key. The keys are the leaderboard's own, not the
/// toast's `ERR_QUEST_ADD_*` twins that share their enUS sentences: `GetQuestLogLeaderBoard`
/// (`0x4e0110`) takes `QUEST_MONSTERS_KILLED` `0x84b61c` (creature), `QUEST_OBJECTS_FOUND`
/// `0x84b634` (override text, GameObject) and `QUEST_ITEMS_NEEDED` `0x84b5f8` (item). A key the
/// install lacks gives an empty line, not no line, as `GetNumQuestLeaderBoards` still counts it.
fn leaderboard_text(
    // The VM's own `GlobalStrings.lua`.
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

/// One creature or GameObject line, for `ReqCreatureOrGOId[i] != 0` (`0x4e00c0`, `0x4e02d7`);
/// `counter` is the slot's 6-bit counter for index i. A non-empty `text` is the name and takes
/// `QUEST_OBJECTS_FOUND` (`0x4e03da`), while `type` stays `"monster"` (`0x4e04ca`).
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
    // A GameObject takes `QUEST_OBJECTS_FOUND` on both legs (`0x4e031e`); with no GameObject
    // name cache here, one without an override wears the placeholder.
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

/// One item line, for `ReqItemId[i] > 0 && ReqItemId[i] != SrcItemId` (`0x4e04f4`-`0x4e04fd`).
/// `bag_count` is the live inventory count, as an item has no slot counter; the name comes from
/// the item cache alone, the override text being the creature branch's (`0x4e033f`, `0x4e03c9`).
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

/// The template's `money` as (required, reward): a negative value is demanded at turn-in.
fn money_split(money: i32) -> (u32, u32) {
    if money < 0 {
        ((-money) as u32, 0)
    } else {
        (0, money as u32)
    }
}

/// One reward or choice `(item id, count)` as a [`QuestItemView`]. `SMSG_QUEST_QUERY_RESPONSE`
/// carries no display id, so the icon comes from the item template's `display_info_id`.
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
    // `GetQuestLogItemLink`'s payload; `None` until the template gives the name and quality.
    let link = name
        .as_ref()
        .map(|n| crate::ui_items::item_link(entry, n, quality));
    QuestItemView {
        name,
        texture,
        count,
        quality,
        item_id: entry,
        usable: true, // not computed; the stock frame tints unusable ones red
        link,
    }
}

/// One quest's objective lines, for every quest, as the watch frame reads any quest's
/// (`QuestLogFrame.lua:613-663`). The reference walks the array once per kind, `event`, creature
/// or GameObject, item, reputation (`0x4e0000`, `0x4e0110`), so one quad can give two lines. The
/// `event` and reputation lines are not built.
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
    // Creature or GameObject: the counter index is the array index, empty elements included.
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
    // Item.
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

/// A quest's detail pane from its template: description, money, rewards and choices.
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
        // Quest text arrives with its `$N`, `$B`, `$G` and `$<n>w` macros unexpanded.
        description: crate::npc_text::substitute(&template.details, macros),
        objectives_text: crate::npc_text::substitute(&template.objectives_text, macros),
        required_money,
        reward_money,
        choices,
        rewards,
        reward_spell: crate::ui_quest::reward_spell_view(template.reward_spell, spells),
    }
}

/// `AbandonQuest`'s search (`0x4df070`): the slot (`+0x4`) of the non-header row of the 40 in
/// `0xbb71c0` that carries `quest_id`; with no such row, nothing is sent.
fn abandon_slot(quest_id: u32, row_slots: &[(u32, u8)]) -> Option<u8> {
    row_slots
        .iter()
        .find(|&&(id, _)| id == quest_id)
        .map(|&(_, slot)| slot)
}

/// Moves the 1-based selection to its quest's new index. A header is never a selection
/// (`QuestLog_GetFirstSelectableQuest` skips them), so one with no visible quest becomes 0 and
/// `QuestLog_Update` selects the first quest (`QuestLogFrame.lua:293-296`).
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

/// Pushes the server's wall clock every frame, for `GetQuestTimers()`; before the first
/// `SMSG_QUERY_TIME_RESPONSE` nothing is pushed and the bindings report no timer.
fn feed_server_clock(
    script: Option<NonSendMut<UiScript>>,
    clock: Res<crate::net::ServerWallClock>,
) {
    let (Some(mut script), Some(now)) = (script, clock.now_unix()) else {
        return;
    };
    script.set_server_unix_time(now);
}

/// The reference's header for `zoneOrSort` 0 (`0x84b4ec`). Only a cached template makes a header
/// (`0x4de6f6`), so it never stands for a template in flight.
const MISSING_HEADER: &str = "Missing header! (quest designers)";

/// One row's grouping and ordering inputs.
struct GroupRow {
    zone_or_sort: i32,
    /// [`MISSING_HEADER`] for 0, empty for an id neither `AreaTable` nor `QuestSort` names.
    header: String,
    level: u32,
    title: String,
    slot: u8,
}

/// The quest log's display order (rebuild `0x4de510`): the header groups, each with its rows.
/// Groups (`0x4de751`, comparator `0x4de8f0`) put id 0 first (`0x4de913`), then go by name
/// through the case-insensitive collator `0x64a4c0` (`_strnicmp 0x414310`), an unnamed id as `""`.
/// Rows (`0x4deac0`) go by level (`0x4decd9`), then title (`0x4decfb`), never by slot; the
/// reference's `qsort` is unstable, so a full tie has no order there, and ties break on slot here.
fn order_groups(rows: &[GroupRow]) -> Vec<(i32, String, Vec<usize>)> {
    let mut groups: Vec<(i32, String, Vec<usize>)> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        match groups.iter_mut().find(|(z, _, _)| *z == r.zone_or_sort) {
            Some((_, _, idxs)) => idxs.push(i),
            None => groups.push((r.zone_or_sort, r.header.clone(), vec![i])),
        }
    }
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

/// Reads the player's `PLAYER_QUEST_LOG` slots each frame and pushes a [`QuestLogState`] on a
/// change; also refreshes `row_slots` for the abandon drain.
fn feed_quest_log(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    mut quest_log: ResMut<QuestLog>,
    names: Res<NameCache>,
    // The bag walk behind an item objective's count.
    objects: crate::net::Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    header_names: Option<Res<QuestHeaderNamesRes>>,
    tag_names: Option<Res<QuestTagNamesRes>>,
    states: Res<crate::world_state::WorldStates>,
    spells: Option<Res<Spells>>,
    // The completion toast is a message-catalog row, whose surface and sound the sink reads.
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
        return;
    };

    let mut rows = Vec::new();
    for slot in 0..PLAYER_QUEST_LOG_SLOTS {
        let Some(log_slot) = store.0.player_quest_log(slot) else {
            continue;
        };
        if log_slot.quest_id == 0 {
            continue; // a slot an abandon or turn-in cleared
        }
        rows.push(Row {
            slot,
            quest_id: log_slot.quest_id,
            log_slot,
        });
    }

    // A quest entering the log plays `QUESTADDED`, the sound of the chat line the reference's
    // quest-slot watcher raises, `ERR_QUEST_ACCEPTED_S` (`0x5dde61`); the line is not printed
    // here. The first snapshot after login is silent: the initial create fills the whole log.
    {
        let current: HashSet<u32> = rows.iter().map(|r| r.quest_id).collect();
        if let Some(prior) = prior_quest_ids.as_ref() {
            if current.iter().any(|id| !prior.contains(id)) {
                script.queue_sound_kit("QUESTADDED");
            }
        }
        *prior_quest_ids = Some(current);
    }

    // The player's own `GlobalStrings.lua`, for the leaderboard's three format keys.
    let get = |key: &str| {
        script
            .lua()
            .globals()
            .get::<String>(key)
            .ok()
            .filter(|t| !t.is_empty())
    };

    // ── In-flight templates: only cached quests are listed ──────────────────────────────────────
    // The rebuild (`0x4de510`) skips a quest whose template is uncached, counting it in
    // `[0xbb7498]` (`0x4de67e`), and lists every other one, so `GetNumQuestLogEntries()` counts
    // cached quests. The reference then holds that list until the last template lands
    // (`0x4de545`, `0x4de8b0`); here each quest shows as its template lands. `template()` runs for
    // every row before the filter: the miss is what sends the query.
    let cached: Vec<bool> = rows
        .iter()
        .map(|r| quest_log.template(r.quest_id, &commands).is_some())
        .collect();
    let rows: Vec<Row> = rows
        .into_iter()
        .zip(cached)
        .filter_map(|(r, hit)| hit.then_some(r))
        .collect();

    // ── Section headers ─────────────────────────────────────────────────────────────────────────
    // One per `ZoneOrSort`: a positive id names an `AreaTable` zone, a negative a `QuestSort`.
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

    let player = crate::npc_text::player_identity(&self_q, &names, &commands);
    let macros = crate::npc_text::MacroContext {
        subject: player.as_ref(),
        states: &states,
    };

    let mut entries: Vec<QuestLogEntryView> = Vec::new();
    let mut header_keys: Vec<Option<String>> = Vec::new();
    // Quests under a collapsed header: out of the visible list, still in the log, and counted by
    // the watch prune, as the reference's scans the whole row array (`0x4de7a7`-`0x4de80f`).
    let mut hidden_quest_ids: Vec<u32> = Vec::new();
    for (_, name, row_idxs) in &groups {
        let collapsed = quest_log.collapsed.contains(name);
        entries.push(QuestLogEntryView {
            quest_id: 0,
            title: name.clone(),
            level: 0,
            tag: None,
            is_header: true,
            collapsed,
            complete: 0,
            timer: 0,
            pushable: false,
            objectives: Vec::new(),
            detail: None,
        });
        header_keys.push(Some(name.clone()));
        for &ri in row_idxs {
            let r = &rows[ri];
            if collapsed {
                hidden_quest_ids.push(r.quest_id);
                continue;
            }
            let (title, level, tag, pushable, objectives, detail) =
                match quest_log.template(r.quest_id, &commands) {
                    Some(t) => (
                        t.title.clone(),
                        t.level,
                        // The tag: `Type` is a `QuestInfo.dbc` id, 0 naming none. No table gives
                        // `None`, never `""`: the row's Lua tests presence.
                        tag_names
                            .as_ref()
                            .and_then(|tags| tags.0.resolve(t.quest_type))
                            .map(str::to_string),
                        // `QUEST_FLAGS_SHARABLE` (vmangos `QuestDef.h:153`) is the client's test
                        // alone: vmangos pushes without it (`QuestHandler.cpp:403-459`) and checks
                        // it only at the accept (`QuestHandler.cpp:114`).
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
                        // Every row's: the bindings resolve the selection per call.
                        Some(build_detail(
                            t,
                            &items,
                            icons.as_deref(),
                            &commands,
                            &macros,
                            spells.as_deref(),
                        )),
                    ),
                    // The in-flight filter above kept only cached rows.
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
                // Absolute unix seconds, 0 untimed; the bindings subtract the server clock.
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
    // Every cached row, folded or not: the table `AbandonQuest` searches.
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
    // Announced after the push, so a handler reads the new log. The reference announces from the
    // `SMSG_QUESTUPDATE_*` handler (`0x5e5ad0`), dropping a toast whose name is uncached; this
    // rides the log diff, so a quest under a collapsed header announces nothing. A progress toast
    // is the leaderboard line, as the `ERR_QUEST_ADD_*_SII` keys compose the same sentence.
    let progressed = quests_with_progressed_objectives(last, &fresh);
    script.set_quest_log(fresh.clone());
    script.fire_event("QUEST_LOG_UPDATE", vec![]);
    if !progressed.is_empty() {
        for quest in progressed {
            for line in quest.changed_lines {
                script.fire_event("UI_INFO_MESSAGE", vec![ScriptValue::Str(line)]);
            }
            if quest.completed {
                // Catalog row `0xf8`, or `0xf9` with no text, never the turn-in's
                // `ERR_QUEST_COMPLETE_S`. The reference shows it only on
                // `SMSG_QUESTUPDATE_COMPLETE` (`0x5e5d12`), its `%s` the quest's end text
                // (`+0x129c`, `0x5e5d22`); this shows the title, on any completion flip.
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
            // The quest's 1-based log row: the reference's arg is `0x4df880`'s search of the row
            // array `0xbb71c0`, not of the five-slot watch list `0xbb70a0`, and stock
            // `AutoQuestWatch_Update(questIndex)` takes it (`QuestLogFrame.lua:67-69`).
            script.fire_event(
                "QUEST_WATCH_UPDATE",
                vec![ScriptValue::Int(i64::from(quest.index))],
            );
        }
    }
    *last = fresh;
}

/// Whether an objective advanced. The reference toasts only on the server's additive
/// `SMSG_QUESTUPDATE_ADD_KILL` and `_ADD_ITEM` (`0x5e5ad0`, `0x5dd060`), with no removal opcode,
/// so a count going down or a name landing late announces nothing. A turn-in destroys the items
/// (vmangos `Player.cpp:13252`) before the slot clears (`Player.cpp:13294`): a silent dip.
fn advanced(now: &QuestLogObjectiveView, was: &QuestLogObjectiveView) -> bool {
    now.cur > was.cur
}

/// One quest whose objectives advanced between two log states.
struct QuestProgress {
    /// The quest's 1-based row in the new log, `GetQuestLogTitle`'s index.
    index: u32,
    /// The COMPLETE toast's `%s` here; empty takes `ERR_QUEST_UNKNOWN_COMPLETE`.
    title: String,
    changed_lines: Vec<String>,
    /// The quest's COMPLETE state turned on in this diff.
    completed: bool,
}

/// The quests in both states with a same-position line that advanced, or whose COMPLETE state
/// turned on. A quest or a line appearing or leaving is not progress.
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

/// The `ZoneOrSort` header names; without client data only id 0's header has a name.
#[derive(Resource)]
pub(crate) struct QuestHeaderNamesRes(pub benilla_formats::QuestHeaderNames);

/// Startup: reads the `AreaTable` and `QuestSort` names off the patch chain.
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

/// The quest tag names by `Type` (`QuestInfo.dbc`); without client data no quest has a tag.
#[derive(Resource)]
pub(crate) struct QuestTagNamesRes(pub benilla_formats::QuestTagNames);

/// Startup: reads the `QuestInfo.dbc` tag names off the patch chain.
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

/// Drains `CollapseQuestHeader` and `ExpandQuestHeader`: an entry index names a header by its
/// title, and 0 is every header (the collapse-all button).
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

/// `QuestLogPushQuest()`: the quest id taken at the click goes out; the server finds the party.
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

    // ── order_groups ────────────────────────────────────────────────────────────────────────────

    fn grow(zos: i32, header: &str, level: u32, title: &str, slot: u8) -> GroupRow {
        GroupRow {
            zone_or_sort: zos,
            header: header.into(),
            level,
            title: title.into(),
            slot,
        }
    }

    #[test]
    fn groups_put_id_zero_first_then_unnamed_then_case_folded_names() {
        let rows = vec![
            grow(12, "Elwynn Forest", 1, "a", 0),
            grow(-3, "warlock", 1, "b", 1), // a byte compare sorts it after "Westfall"
            grow(0, MISSING_HEADER, 1, "c", 2),
            grow(40, "Westfall", 1, "d", 3),
            grow(999, "", 1, "e", 4), // an id neither table names
        ];
        let order: Vec<i32> = order_groups(&rows).into_iter().map(|(z, _, _)| z).collect();
        assert_eq!(order, vec![0, 999, 12, -3, 40]);
    }

    #[test]
    fn the_zero_header_is_the_references_literal() {
        let rows = vec![grow(0, MISSING_HEADER, 1, "a", 0)];
        let groups = order_groups(&rows);
        assert_eq!(groups[0].1, "Missing header! (quest designers)");
    }

    #[test]
    fn within_a_group_it_is_level_then_title_not_slot_order() {
        // Seeded in the reverse of the expected order by slot.
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
        assert_eq!(titles, vec!["Anything", "Alpha", "beta", "Zzz"]);
    }

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
        // A header is never a selection: 0 lets `QuestLog_SetFirstValidSelection` pick a quest.
        let old = vec![header("A"), quest(7)];
        let new = vec![header("A")]; // folded: the quest left the visible list
        assert_eq!(remap_selection(&old, &new, 2), 0);
    }

    #[test]
    fn a_header_selection_is_evicted_not_preserved() {
        let old = vec![header("A"), quest(7)];
        let new = vec![header("A"), quest(7)];
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

    /// Templates that name their key, as enUS gives each leaderboard key the same sentence as its
    /// `ERR_QUEST_ADD_*` twin.
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

    /// The GameObject key is fetched above the cache test (`0x4e031e`), so a nameless one keeps it.
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
        // 8 carried, 5 required: the line clamps.
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

    /// An override takes `QUEST_OBJECTS_FOUND` (`0x84b634` at `0x4e03e1`); `type` is `"monster"`.
    #[test]
    fn custom_text_overrides_the_creature_auto_line() {
        let o = obj(100, 10, 0, 0, "Slay the vermin");
        let line = creature_line(&o, 4, Some("Kobold Vermin"), &probe_strings).unwrap();
        assert_eq!(line.text, "<FOUND Slay the vermin 4 of 10>");
        assert_eq!(line.kind, "monster");
    }

    /// Against the real `GlobalStrings.lua`; skips without client data.
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

    /// A line's `finished` is `cur >= req` alone: `GetQuestLogLeaderBoard` reads the COMPLETE bit
    /// only for the `event` line (`0x4e02a2`); the turn-in predicate `0x4df580` reads both.
    #[test]
    fn a_complete_quest_does_not_mark_a_short_objective_finished() {
        let o = obj(100, 10, 0, 0, "");
        // 1 of 10: not finished, whatever the quest's state.
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

    // ── build_objectives ────────────────────────────────────────────────────────────────────────

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
            obj(0, 0, 0, 0, ""),    // unused quad: no line
            obj(0, 0, 2000, 5, ""), // item objective (counted from the bags, not a counter)
            obj(0, 0, 0, 0, ""),    // unused quad
        ]);
        let log_slot = QuestLogSlot {
            quest_id: 1,
            counters: [3, 0, 0, 0],
            state: 0,
            timer: 0,
        };
        let store = ObjectFields::default(); // empty bags: the item objective reads 0/5
        let items = Items::default();
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

        // The unused quads emit nothing; no names are seeded, so both lines wear the placeholder.
        assert_eq!(objectives.len(), 2);
        assert_eq!(objectives[0].text, "<KILLED ... 3 of 10>");
        assert_eq!(objectives[0].kind, "monster");
        assert!(!objectives[0].finished);
        assert_eq!(objectives[1].text, "<NEEDED ... 0 of 5>");
        assert_eq!(objectives[1].kind, "item");
    }

    /// Creatures (`0x4e02d7`) and items (`0x4e04f4`) are separate passes, so a quad carrying both
    /// gives two lines. The fixture is quest 358's shape: creature 1941 ×8 and item 2834 ×8 at
    /// index 0, a second creature at 1.
    #[test]
    fn a_quad_carrying_both_a_creature_and_an_item_emits_both_lines() {
        let template = quest_template([
            obj(1941, 8, 2834, 8, ""), // both kinds, one index
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
        // Empty bags: every item objective reads 0.
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

        // Pass order: both creatures, then the item.
        assert_eq!(objectives.len(), 3, "{objectives:#?}");
        assert_eq!(objectives[0].text, "<KILLED ... 8 of 8>");
        assert_eq!(objectives[1].text, "<KILLED ... 1 of 5>");
        assert_eq!(objectives[2].text, "<NEEDED ... 0 of 8>");
        assert_eq!(objectives[2].kind, "item");
    }

    /// The override text is the creature branch's (`0x4e03da`), never the item line's.
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
        // Empty bags: every item objective reads 0.
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
        // The override is the creature's name, under `QUEST_OBJECTS_FOUND` (`0x4e03e1`).
        assert_eq!(objectives[0].text, "<FOUND Graverobbers routed 8 of 8>");
        // The item line wears the in-flight placeholder, never the creature's text.
        assert_eq!(objectives[1].text, "<NEEDED ... 0 of 8>");
    }

    /// `ReqItemId[i] != SrcItemId` (`0x4e00ce`, `0x4e04fa`).
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
        // Empty bags: every item objective reads 0.
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
            counters: [1, 0, 0, 0],            // 1 of the 10 required
            state: quest_slot_state::COMPLETE, // yet the quest is complete
            timer: 0,
        };
        let store = ObjectFields::default();
        let items = Items::default();
        // Empty bags: every item objective reads 0.
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

    // ── abandon_slot ────────────────────────────────────────────────────────────────────────────

    #[test]
    fn abandon_maps_quest_id_to_slot_across_a_gap() {
        // Slots 0 and 2, slot 1 empty: each id finds its own slot.
        let row_slots = vec![(783u32, 0u8), (7, 2)];
        assert_eq!(abandon_slot(783, &row_slots), Some(0));
        assert_eq!(abandon_slot(7, &row_slots), Some(2));
        assert_eq!(abandon_slot(8, &row_slots), None); // not in the log: nothing to remove
        assert_eq!(abandon_slot(0, &row_slots), None); // 0 is never a quest
    }

    // ── The abandon mark is a quest id ──────────────────────────────────────────────────────────

    /// `SetAbandonQuest` (`0x4dfb50`) copies the selection `0xbb7480`, a quest id (`0x4def30`,
    /// `0x4def5d`), into the mark `0xbb7484`, and `AbandonQuest` (`0x4dfe00`, `0x4df070`) finds its
    /// row in `0xbb71c0` again, so a fold while the popup is up cannot retarget the abandon.
    #[test]
    fn a_fold_between_mark_and_confirm_does_not_retarget_the_abandon() {
        use benilla_protocol::messages::field::FIELD_PLAYER_QUEST_LOG_1_1;

        // Z1 = zone 0 ("Missing header!", forced first) holding A; Z2 = zone 7 holding B and C.
        // Entries: [Z1, A, Z2, B, C]; B is row 4.
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
        // With the popup up, fold Z1: B moves to row 3 and C takes row 4.
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

    // ── quests_with_progressed_objectives ───────────────────────────────────────────────────────

    /// A one-objective entry whose `cur`/`req` are parsed from its text's trailing `cur/req`, so
    /// the two cannot disagree.
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
        // Both advancing fires both, each at its own index with its own line.
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
        // A fresh accept is not progress, nor is a late template growing lines from none.
        assert!(quests_with_progressed_objectives(&empty, &with_entry).is_empty());
        let grown = state(vec![QuestLogEntryView {
            quest_id: 7,
            objectives: vec![],
            ..Default::default()
        }]);
        assert!(quests_with_progressed_objectives(&grown, &with_entry).is_empty());
        // Nor a turn-in or abandon removing the quest.
        assert!(quests_with_progressed_objectives(&with_entry, &empty).is_empty());
    }

    /// A turn-in destroys the items (an immediate `SMSG_DESTROY_OBJECT`) before the slot clears in
    /// the batched `SMSG_UPDATE_OBJECT` (`Player::RewardQuest`), so the line dips to `0/req` for a
    /// frame; the reference toasts only on `SMSG_QUESTUPDATE_ADD_ITEM`.
    #[test]
    fn a_regressing_objective_never_announces() {
        let held = state(vec![entry(313, "Rumbleshot's Ammo: 1/1")]);
        let emptied = state(vec![entry(313, "Rumbleshot's Ammo: 0/1")]);
        assert!(
            quests_with_progressed_objectives(&held, &emptied).is_empty(),
            "the bag emptying under a still-logged quest is not progress"
        );
        // The slot clearing next frame is silent too: the quest leaves the log.
        assert!(quests_with_progressed_objectives(&emptied, &state(vec![])).is_empty());
        // The control: the same quest actually being collected still announces.
        let looted = state(vec![entry(313, "Rumbleshot's Ammo: 1/1")]);
        assert_eq!(
            quests_with_progressed_objectives(&emptied, &looted)[0].changed_lines,
            ["Rumbleshot's Ammo: 1/1"]
        );
    }

    /// A name landing late rewrites the text with the count unmoved: no toast.
    #[test]
    fn a_name_landing_late_does_not_re_announce() {
        let placeholder = state(vec![entry(5, "...: 3/5")]);
        let named = state(vec![entry(5, "Small Barnacled Clam: 3/5")]);
        assert!(quests_with_progressed_objectives(&placeholder, &named).is_empty());
    }

    /// The COMPLETE flip announces once for the quest, not again per objective line.
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
