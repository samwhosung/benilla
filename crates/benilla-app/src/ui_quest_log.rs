//! The quest log's app side: the quest-template cache and what the engine's quest log
//! ([`benilla_ui::script`]) reads from the app. Each template that lands goes to the engine as a
//! record, with its header and tag names, its text's macros and its reward spell resolved; the
//! player's `PLAYER_QUEST_LOG` slots go at the quest log's field watch
//! ([`crate::field_watch::FieldNotice::QuestLog`]); the records, creatures and held counts the
//! engine's verbs read are answered here. Nothing is rebuilt or compared per frame: an idle frame
//! reads two message queues and the engine's ask lists.
//!
//! The quest-update packets' toasts are shown from here: `SMSG_QUESTUPDATE_ADD_ITEM`,
//! `_ADD_KILL` and `_COMPLETE`, each with its `QUEST_WATCH_UPDATE`. Abandons, shares and the
//! slot watcher's accept line drain from here too.

use std::collections::{HashMap, HashSet};

use crate::ui_items::{count_of, InventoryScope};

use bevy::prelude::*;

use benilla_protocol::messages::{
    quest_flags, quest_slot_state, QuestTemplate, PLAYER_QUEST_LOG_SLOTS,
};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{
    QuestObjectiveView, QuestRecordView, QuestSlotView, ScriptValue, UiScript, QUEST_LOG_SLOTS,
};

use crate::field_watch::FieldNotice;
use crate::item_watch::ItemNotice;
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::{Ask, QueryCache};
use crate::ui_action::Spells;
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_unit::UnitFeed;

/// Marks a GameObject objective's entry on the wire (vmangos `Quest.cpp:512-516`).
const GO_OBJECTIVE_BIT: u32 = 0x8000_0000;

/// The template cache, asked once per quest id, and what the packets leave for the feed.
#[derive(Resource, Default)]
pub(crate) struct QuestLog {
    templates: QueryCache<u32, QuestTemplate>,
    /// The quests whose template landed, and those refused, since the feed last ran.
    fresh: Vec<u32>,
    refused: Vec<u32>,
    /// The fold mask and the selected quest a dying VM left ([`fold_dying_vm_quest_folds`]),
    /// seated in the next.
    folds: (HashSet<String>, Option<u32>),
    /// The quest-update toasts the handlers resolved, shown by the next feed.
    toasts: Vec<Toast>,
    /// `(quest id, item id, count)` pairs of an added item whose quest record was not cached, held
    /// until it lands (`0x5dd180`).
    deferred_item_adds: Vec<(u32, u32, u32)>,
    /// The creature entries the engine's leaderboard asked for and the cache has not answered.
    creature_waits: HashSet<u32>,
    /// The `SMSG_SET_FACTION_STANDING` packets that moved a rank since the feed last ran.
    faction_rank_moves: u32,
    /// `SMSG_QUESTGIVER_QUEST_FAILED`'s reason as last read (`0xc4d794`), a global the record
    /// callback reads when it fires (`0x5dc968`).
    pub(crate) giver_failed_reason: u32,
    /// The quests a `SMSG_QUESTGIVER_QUEST_FAILED` found uncached, whose line waits for the record
    /// (`0x5dc920`).
    pub(crate) giver_failures: Vec<u32>,
}

impl crate::query_cache::AskOnce for QuestLog {
    fn clear_pending(&mut self) {
        self.templates.clear_pending();
    }
}

impl QuestLog {
    /// The template, read as `ask` says: an ask sends `CMSG_QUEST_QUERY`, once while in flight.
    /// vmangos answers nothing for an id it does not know (`QuestHandler.cpp:229-231`); an answer
    /// with an empty title is a refusal ([`Self::insert_template`]).
    pub(crate) fn template(&self, quest_id: u32, ask: Ask<'_>) -> Option<&QuestTemplate> {
        self.templates.read(quest_id, ask, |commands| {
            debug!("ui_quest_log: asking quest template (quest {quest_id})");
            let _ = commands
                .0
                .send(ClientCommand::QuestQuery { quest: quest_id });
        })
    }

    /// Whether the server answered `quest_id` as unknown, not still pending.
    pub(crate) fn template_answered_unknown(&self, quest_id: u32) -> bool {
        self.templates.answered_unknown(quest_id)
    }

    /// Record a template answer (`SMSG_QUEST_QUERY_RESPONSE`). The handler `0x555300` tests the
    /// title's first byte (`0x555410`): an empty title is a refusal, which runs the callbacks with
    /// status 0 and frees the node (`0x55543c` → `0x563150`), and only a titled one is stored
    /// (`0x562c80`).
    pub(crate) fn insert_template(&mut self, template: QuestTemplate) {
        let quest_id = template.quest_id;
        if template.title.is_empty() {
            self.templates.refuse(quest_id);
            self.refused.push(quest_id);
        } else {
            self.templates.insert(quest_id, template);
            self.fresh.push(quest_id);
        }
    }

    /// An `SMSG_SET_FACTION_STANDING` moved a faction's rank (`0x4d58a9`).
    pub(crate) fn note_faction_rank_move(&mut self) {
        self.faction_rank_moves += 1;
    }

    /// The carry a dying VM left, dropped when the session ends.
    pub(crate) fn forget_folds(&mut self) {
        self.folds = Default::default();
    }

    /// Disconnect: drops everything, the templates and the folds included.
    pub(crate) fn clear_session(&mut self) {
        *self = Self::default();
    }
}

/// One quest-update toast and the quest its `QUEST_WATCH_UPDATE` names.
#[derive(Debug, PartialEq, Eq)]
struct Toast {
    quest_id: u32,
    line: ToastLine,
}

#[derive(Debug, PartialEq, Eq)]
enum ToastLine {
    /// `ERR_QUEST_ADD_ITEM_SII`, `_ADD_KILL_SII` or `_ADD_FOUND_SII`: the name, the count shown
    /// and the need.
    Count {
        key: &'static str,
        name: String,
        cur: u32,
        need: u32,
    },
    /// `ERR_QUEST_OBJECTIVE_COMPLETE_S` with the quest's end text, or with none
    /// `ERR_QUEST_UNKNOWN_COMPLETE`.
    Complete(String),
}

/// What `0x5dd060` makes of one log quest for an added item.
#[derive(Debug, PartialEq, Eq)]
enum ItemAdd {
    /// The quest does not need the item: the walk goes on to the next quest.
    NotNeeded,
    /// The quest needs it and shows nothing, the item record uncached or the need already met:
    /// the walk ends.
    Silent,
    /// The walk ends on this toast.
    Toast(Toast),
}

/// `0x5dd060(record, item, count)` for one log quest: the quest's four required item ids
/// (`+0x14bc`) are searched for `item_id`; the item's record (`0x55ba30`) must be cached and the
/// count held (`0x622130`, mask 8) below the need (`+0x14cc + 4·i`); the toast shows the held count
/// plus the added one, capped at the need. `held` and `name` are read in that order only once the
/// quest needs the item.
fn item_added_to_quest(
    template: &QuestTemplate,
    item_id: u32,
    count: u32,
    held: impl FnOnce() -> u32,
    name: impl FnOnce() -> Option<String>,
) -> ItemAdd {
    let Some(need) = template
        .objectives
        .iter()
        .find(|o| o.item_id == item_id)
        .map(|o| o.item_count)
    else {
        return ItemAdd::NotNeeded;
    };
    let Some(name) = name() else {
        return ItemAdd::Silent;
    };
    let held = held();
    if held >= need {
        return ItemAdd::Silent;
    }
    ItemAdd::Toast(Toast {
        quest_id: template.quest_id,
        line: ToastLine::Count {
            key: "ERR_QUEST_ADD_ITEM_SII",
            name,
            cur: held.saturating_add(count).min(need),
            need,
        },
    })
}

/// `SMSG_QUESTUPDATE_ADD_KILL`'s line (`0x5e5d61`): an objective whose id is the packet's and
/// whose override text is set names it under `ERR_QUEST_ADD_FOUND_SII` (`0x5e5db0`-`0x5e5e2a`);
/// else a GameObject's cached name (`jns`, `0x5e5dcf`) takes the same key and a creature's
/// `ERR_QUEST_ADD_KILL_SII` (`0x5e5e5b`). Both caches are read without a callback, so a miss
/// shows nothing; the count and the need are the packet's.
fn kill_line(
    template: &QuestTemplate,
    entry: u32,
    count: u32,
    need: u32,
    go_name: impl FnOnce(u32) -> Option<String>,
    creature_name: impl FnOnce(u32) -> Option<String>,
) -> Option<ToastLine> {
    let (key, name) = match template
        .objectives
        .iter()
        .find(|o| o.creature_or_go == entry && !o.text.is_empty())
    {
        Some(o) => ("ERR_QUEST_ADD_FOUND_SII", o.text.clone()),
        None if entry & GO_OBJECTIVE_BIT != 0 => (
            "ERR_QUEST_ADD_FOUND_SII",
            go_name(entry & !GO_OBJECTIVE_BIT)?,
        ),
        None => ("ERR_QUEST_ADD_KILL_SII", creature_name(entry)?),
    };
    Some(ToastLine::Count {
        key,
        name,
        cur: count,
        need,
    })
}

/// The fold mask (`[0xbb748c]`) and the selection (`[0xbb7480]`) out of a dying VM: C globals
/// the reference writes only at the collapse verbs (`0x4ded88`, `0x4dedab`, `0x4dedc7`), the
/// rebuild (`0x4de608`, `0x4de793`) and the selection verbs, which no UI init resets, so they
/// outlive a `/reload`. Called with the CVar fold, before the VM is replaced.
pub(crate) fn fold_dying_vm_quest_folds(world: &mut World) {
    let Some(folds) = world
        .get_non_send_resource::<UiScript>()
        .map(UiScript::quest_log_carry)
    else {
        return;
    };
    if let Some(mut log) = world.get_resource_mut::<QuestLog>() {
        log.folds = folds;
    }
}

/// Seat the folded mask in a new in-game VM before its load runs any file or event.
pub(crate) fn seed_quest_folds(world: &World, script: &mut UiScript) {
    if let Some(log) = world.get_resource::<QuestLog>() {
        script.seed_quest_log_carry(log.folds.clone());
    }
}

/// The quest log's packet handlers.
mod net {
    use benilla_protocol::messages::{quest_slot_state, QuestTemplate, PLAYER_QUEST_LOG_SLOTS};
    use benilla_protocol::{ObjectFields, SessionEvent, SessionEventKind};
    use benilla_ui::script::UiScript;
    use bevy::ecs::system::SystemParam;
    use bevy::prelude::*;

    use super::{item_added_to_quest, kill_line, ItemAdd, QuestLog, Toast, ToastLine};
    use crate::items::Items;
    use crate::names::NameCache;
    use crate::net::{NetCommands, NetHandlerApp, ObjectStore, Objects, SelfPlayer};
    use crate::query_cache::Ask;
    use crate::ui_items::{count_of, InventoryScope};

    /// Registers the template, quest-update and session-end handlers.
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::QuestTemplate, on_template)
            .net_handler(K::QuestTemplate, on_template_deferred_item_adds)
            .net_handler(K::QuestObjectiveItem, on_item_added)
            .net_handler(K::QuestObjectiveKill, on_kill)
            .net_handler(K::QuestObjectivesComplete, on_complete)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// What an added item's toast reads off the player: the bags and the item cache.
    #[derive(SystemParam)]
    struct ItemBook<'w, 's> {
        self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
        objects: Objects<'w, 's>,
        items: Res<'w, Items>,
        commands: Res<'w, NetCommands>,
    }

    impl ItemBook<'_, '_> {
        fn store(&self) -> Option<&ObjectFields> {
            self.self_q.iter().next().map(|s| &s.0)
        }

        /// `0x622130(inventory, item, 8)`: the banked copies count too.
        fn held(&self, item_id: u32) -> u32 {
            self.store().map_or(0, |store| {
                count_of(store, &self.objects, item_id, InventoryScope::QUEST_ITEMS)
            })
        }

        /// The item's record, read without a callback (`0x5dd0bd`).
        fn name(&self, item_id: u32) -> Option<String> {
            self.items
                .template(item_id, 0, Ask::Read)
                .map(|t| t.name.clone())
        }
    }

    /// `SMSG_QUESTUPDATE_ADD_ITEM` (`0x5e5ad0`, case `0x19a`): the walk over the 20 log slots
    /// (`0x5e5b3d`-`0x5e5beb`) that `0x5dd060` ends on the first quest needing the item. A quest
    /// whose record is not cached queues the pair for its arrival and the walk goes on
    /// (`0x5e5b93`-`0x5e5bd0`). The held count is read now, before the item's own create reaches
    /// the bags, as the packet leads it (vmangos `Player.cpp:10249`). A quest whose row holds the
    /// expiry latch is passed over (`0x5e5b6c`).
    fn on_item_added(
        In(ev): In<SessionEvent>,
        book: ItemBook,
        mut quest_log: ResMut<QuestLog>,
        script: Option<NonSend<UiScript>>,
    ) {
        let SessionEvent::QuestObjectiveItem { item_id, count } = ev else {
            return;
        };
        let Some(store) = book.store() else {
            return;
        };
        for slot in 0..PLAYER_QUEST_LOG_SLOTS {
            let Some(quest_id) = store
                .player_quest_log(slot)
                .map(|s| s.quest_id)
                .filter(|&id| (id as i32) > 0)
                .filter(|&id| !script.as_ref().is_some_and(|s| s.quest_log_latched(id)))
            else {
                continue;
            };
            // The packet looks the quest up with a callback on a miss (`0x5e5bcb`).
            let Some(template) = quest_log.template(quest_id, Ask::Event(&book.commands)) else {
                if !quest_log.template_answered_unknown(quest_id) {
                    quest_log
                        .deferred_item_adds
                        .push((quest_id, item_id, count));
                }
                continue;
            };
            match item_added_to_quest(
                template,
                item_id,
                count,
                || book.held(item_id),
                || book.name(item_id),
            ) {
                ItemAdd::NotNeeded => {}
                ItemAdd::Silent => return,
                ItemAdd::Toast(toast) => {
                    quest_log.toasts.push(toast);
                    return;
                }
            }
        }
    }

    /// The callback `0x5dd180`: a record that lands runs the queued pairs of its quest through
    /// `0x5dd060`, whose answer it ignores; a refused one drops them.
    fn on_template_deferred_item_adds(
        In(ev): In<SessionEvent>,
        book: ItemBook,
        mut quest_log: ResMut<QuestLog>,
    ) {
        let SessionEvent::QuestTemplate(t) = ev else {
            return;
        };
        // A refusal frees the pairs with no walk (`0x5dd1a4`).
        let refused = t.title.is_empty();
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut quest_log.deferred_item_adds)
            .into_iter()
            .partition(|&(quest_id, _, _)| quest_id == t.quest_id);
        quest_log.deferred_item_adds = rest;
        for (_, item_id, count) in mine.into_iter().filter(|_| !refused) {
            if let ItemAdd::Toast(toast) = item_added_to_quest(
                &t,
                item_id,
                count,
                || book.held(item_id),
                || book.name(item_id),
            ) {
                quest_log.toasts.push(toast);
            }
        }
    }

    /// Whether the quest is done for the update arms: its row's expiry latch (`0x4df4b0`,
    /// `0x5e5c16`) or its log slot's failed bit (`0x2`, `0x5e5c7b`).
    fn done(store: Option<&ObjectFields>, script: Option<&UiScript>, quest_id: u32) -> bool {
        script.is_some_and(|s| s.quest_log_latched(quest_id))
            || store.is_some_and(|store| {
                (0..PLAYER_QUEST_LOG_SLOTS)
                    .filter_map(|slot| store.player_quest_log(slot))
                    .any(|s| s.quest_id == quest_id && s.state & quest_slot_state::FAIL != 0)
            })
    }

    /// `SMSG_QUESTUPDATE_ADD_KILL` (case `0x199`, `0x5e5d61`): the record read without a callback
    /// (`0x5e5ca0`), then, for a quest not done, [`kill_line`] and `QUEST_WATCH_UPDATE`. A record
    /// not cached shows nothing here, where the reference reads through it and faults
    /// (`0x5e5db0`).
    fn on_kill(
        In(ev): In<SessionEvent>,
        self_q: Query<&ObjectStore, With<SelfPlayer>>,
        names: Res<NameCache>,
        go_templates: Option<Res<crate::go_templates::GameObjectTemplates>>,
        mut quest_log: ResMut<QuestLog>,
        script: Option<NonSend<UiScript>>,
    ) {
        let SessionEvent::QuestObjectiveKill {
            quest_id,
            entry,
            count,
            required,
        } = ev
        else {
            return;
        };
        if done(
            self_q.iter().next().map(|s| &s.0),
            script.as_deref(),
            quest_id,
        ) {
            return;
        }
        let Some(template) = quest_log.template(quest_id, Ask::Read) else {
            return;
        };
        let line = kill_line(
            template,
            entry,
            count,
            required,
            |go| go_templates.as_ref()?.name(go).map(str::to_string),
            |creature| {
                names
                    .resolve_creature(creature, 0, Ask::Read)
                    .map(str::to_string)
            },
        );
        if let Some(line) = line {
            quest_log.toasts.push(Toast { quest_id, line });
        }
    }

    /// `SMSG_QUESTUPDATE_COMPLETE` (case `0x198`, `0x5e5d12`): for a cached record of a quest not
    /// done, `ERR_QUEST_OBJECTIVE_COMPLETE_S` (row `0xf8`) with its end text (`+0x129c`) and then
    /// `QUEST_WATCH_UPDATE`, or with no end text `ERR_QUEST_UNKNOWN_COMPLETE` (row `0xf9`) alone
    /// (`0x5e5d44`-`0x5e5d5e`).
    fn on_complete(
        In(ev): In<SessionEvent>,
        self_q: Query<&ObjectStore, With<SelfPlayer>>,
        mut quest_log: ResMut<QuestLog>,
        script: Option<NonSend<UiScript>>,
    ) {
        let SessionEvent::QuestObjectivesComplete { quest_id } = ev else {
            return;
        };
        if done(
            self_q.iter().next().map(|s| &s.0),
            script.as_deref(),
            quest_id,
        ) {
            return;
        }
        let Some(template) = quest_log.template(quest_id, Ask::Read) else {
            return;
        };
        let line = ToastLine::Complete(template.end_text.clone());
        quest_log.toasts.push(Toast { quest_id, line });
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
            .add_message::<ItemNotice>()
            .add_message::<FieldNotice>()
            .add_systems(
                Startup,
                (load_quest_header_names, load_quest_tag_names)
                    .after(benilla_assets::AssetSet::Open),
            )
            .add_systems(
                Update,
                (
                    // The unit feed's world-enter cascade rebuilds the log off what this seats.
                    feed_quest_log
                        .in_set(UnitFeed)
                        .in_set(crate::ui_unit::WorldEnterSeats),
                    // Before the script tick, so `QuestTimerFrame` reads this frame's clock.
                    feed_server_clock.in_set(UiFeed),
                    drain_quest_log_abandons.after(UiInput),
                    drain_quest_log_pushes.after(UiInput),
                ),
            );
    }
}

/// The template's `money` as (required, reward): a negative value is demanded at turn-in.
fn money_split(money: i32) -> (u32, u32) {
    if money < 0 {
        ((-money) as u32, 0)
    } else {
        (0, money as u32)
    }
}

/// Pushes the server's wall clock every frame, for the quest timers; before the first
/// `SMSG_QUERY_TIME_RESPONSE` the local one, which the reference counts down by while its offset
/// is 0 (`0x4de3d3`). Each response after the VM's first frame signals `QUEST_LOG_UPDATE` once
/// (`0x4def1b`).
fn feed_server_clock(
    script: Option<NonSendMut<UiScript>>,
    clock: Res<crate::net::ServerWallClock>,
    mut seen: Local<crate::ui_script::VmMemo<Option<u64>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let samples = clock.samples();
    let seen = seen.get(&script);
    let synced = seen.map_or(0, |was| samples.saturating_sub(was));
    *seen = Some(samples);
    let Some(now) = clock.now_unix() else {
        let local = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        script.set_local_unix_time(local);
        return;
    };
    script.set_server_unix_time(now);
    script.set_clock_sync_zero(clock.zero_offset());
    for _ in 0..synced {
        script.quest_log_clock_synced();
    }
}

/// The reference's header for `zoneOrSort` 0 (`0x84b4ec`). Only a cached template makes a header
/// (`0x4de6f6`), so it never stands for a template in flight.
const MISSING_HEADER: &str = "Missing header! (quest designers)";

/// What a record resolves through besides its template.
struct RecordTables<'a> {
    headers: Option<&'a QuestHeaderNamesRes>,
    tags: Option<&'a QuestTagNamesRes>,
    macros: &'a crate::npc_text::MacroContext<'a>,
    spells: Option<&'a Spells>,
    factions: Option<&'a benilla_formats::FactionCatalog>,
}

/// A template as the engine's record: the group's name (`AreaTable` above 0, `QuestSort` below),
/// the tag (`QuestInfo.dbc`), the text's macros expanded and the reward spell.
fn record_view(t: &QuestTemplate, tables: &RecordTables) -> QuestRecordView {
    let (required_money, reward_money) = money_split(t.money);
    let header = if t.zone_or_sort == 0 {
        MISSING_HEADER.to_string()
    } else {
        tables
            .headers
            .and_then(|h| h.0.resolve(t.zone_or_sort))
            .unwrap_or_default()
            .to_string()
    };
    let set = |list: &[(u32, u32)]| -> Vec<(u32, u32)> {
        list.iter().copied().filter(|&(id, _)| id != 0).collect()
    };
    QuestRecordView {
        title: t.title.clone(),
        level: t.level,
        zone_or_sort: t.zone_or_sort,
        header,
        // `Type` is a `QuestInfo.dbc` id, 0 naming none; no table gives `None`, never `""`.
        tag: tables
            .tags
            .and_then(|tags| tags.0.resolve(t.quest_type))
            .map(str::to_string),
        // `QUEST_FLAGS_SHARABLE` (vmangos `QuestDef.h:153`) is the client's test alone: vmangos
        // pushes without it (`QuestHandler.cpp:403-459`) and checks it only at the accept
        // (`QuestHandler.cpp:114`).
        pushable: t.flags & quest_flags::SHARABLE != 0,
        objectives: std::array::from_fn(|i| {
            let o = &t.objectives[i];
            QuestObjectiveView {
                creature_or_go: o.creature_or_go,
                required_count: o.required_count,
                item_id: o.item_id,
                item_count: o.item_count,
                text: o.text.clone(),
            }
        }),
        src_item_id: t.src_item_id,
        // `QUEST_FLAGS_PARTY_ACCEPT` (2) or `_EXPLORATION` (4), vmangos `QuestDef.h:151-152`: the
        // server's complete bit decides.
        needs_complete_bit: t.flags & 6 != 0,
        rep_min: (t.rep_objective_faction, t.rep_objective_value as i32),
        rep_faction_name: tables
            .factions
            .and_then(|f| f.faction_name(t.rep_objective_faction))
            .map(str::to_string),
        rep_max: (t.opposite_rep_faction, t.opposite_rep_value as i32),
        // The leaderboard pushes the cached string itself (`0x4e0289`): no macro is expanded.
        end_text: t.end_text.clone(),
        required_money,
        reward_money,
        rewards: set(&t.rewards),
        choices: set(&t.choices),
        // Quest text arrives with its `$N`, `$B`, `$G` and `$<n>w` macros unexpanded.
        description: crate::npc_text::substitute(&t.details, tables.macros),
        objectives_text: crate::npc_text::substitute(&t.objectives_text, tables.macros),
        reward_spell: crate::ui_quest::reward_spell_view(t.reward_spell, tables.spells),
    }
}

/// Whether `text` carries a world-state macro, `$<digits>w` (`crate::npc_text`).
fn has_world_state_token(text: &str) -> bool {
    text.match_indices('$').any(|(at, _)| {
        let after = &text[at + 1..];
        let rest = after.trim_start_matches(|c: char| c.is_ascii_digit());
        rest.len() < after.len() && rest.starts_with(['w', 'W'])
    })
}

/// The descriptor's twenty slots, as the engine reads them.
pub(crate) fn slot_views(store: &ObjectFields) -> [QuestSlotView; QUEST_LOG_SLOTS] {
    std::array::from_fn(|slot| {
        store
            .player_quest_log(slot as u8)
            .map_or_else(QuestSlotView::default, |s| QuestSlotView {
                quest_id: s.quest_id,
                counters: s.counters,
                state: s.state,
                timer: s.timer,
            })
    })
}

/// Every quest-log slot as `(quest id, state)`, the slot watcher's input.
type SlotStates = [(u32, u8); PLAYER_QUEST_LOG_SLOTS as usize];

/// The quests the slot watcher `0x5ddd80` announces, per slot as `(quest id, state)`: a quest in a
/// slot that held none, or the same quest with its fail bit cleared (`0x5dde17`-`0x5dde34`). A slot
/// that swaps one quest for another announces nothing.
fn accepted_quests(prior: &[(u32, u8)], now: &[(u32, u8)]) -> Vec<u32> {
    now.iter()
        .zip(prior)
        .filter(|&(&(id, state), &(was_id, was_state))| {
            id != 0
                && (was_id == 0
                    || (was_id == id
                        && state & quest_slot_state::FAIL == 0
                        && was_state & quest_slot_state::FAIL != 0))
        })
        .map(|(&(id, _), _)| id)
        .collect()
}

/// The macro inputs a record's text expands against: the player's name, race, class and gender.
type SubjectKey = (String, String, String, u8);

/// The feed's state for one VM and one player.
#[derive(Default)]
pub(crate) struct FeedMemo {
    /// The player the VM was seated for.
    player: Option<u64>,
    /// The slot watcher's last view.
    prior: Option<SlotStates>,
    /// The macro inputs the held records' text was expanded with.
    subject: Option<SubjectKey>,
    /// The held counts last pushed.
    held: HashMap<u32, u32>,
    /// The place in the creature cache's landings the engine's names are current to.
    creatures_seen: usize,
}

/// What the feed reads of the player and the caches.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct QuestLogReads<'w, 's> {
    self_q: Query<'w, 's, (&'static ObjectStore, &'static Guid), With<SelfPlayer>>,
    names: Res<'w, NameCache>,
    // The bag walk behind an item objective's count.
    objects: crate::net::Objects<'w, 's>,
    commands: Res<'w, NetCommands>,
    header_names: Option<Res<'w, QuestHeaderNamesRes>>,
    tag_names: Option<Res<'w, QuestTagNamesRes>>,
    states: Res<'w, crate::world_state::WorldStates>,
    spells: Option<Res<'w, Spells>>,
    // `Faction.dbc`'s names, the reputation line's faction (`0x4e0743`).
    factions: Option<Res<'w, crate::target::Factions>>,
    unit_names: crate::ui_unit::NameTables<'w>,
    // A durability change moves no notice, and a broken copy is not counted (`0x6224fc`).
    changed_items: Query<'w, 's, (), (With<crate::items::ItemObject>, Changed<ObjectStore>)>,
}

/// The quest log's feed. A new VM or player is seated with every cached record and the slots;
/// otherwise the frame lands the templates the packets brought, answers the engine's asks, keeps
/// the held counts current when the bags or the slots moved, runs the item-change trigger
/// (`0x4df7b0`) for each item notice that carries it, fires the callbacks whose records landed,
/// and shows the quest-update toasts. An idle frame does none of it.
pub(crate) fn feed_quest_log(
    script: Option<NonSendMut<UiScript>>,
    reads: QuestLogReads,
    mut quest_log: ResMut<QuestLog>,
    // The toasts are message-catalog rows, whose surface and sound the sink reads.
    mut sink: crate::ui_action::MessageSink,
    // The accept line's queue.
    mut quest_lines: ResMut<crate::ui_quest::QuestLines>,
    // The latch's quest-giver refresh.
    mut quest_giver: ResMut<crate::ui_quest::QuestGiver>,
    mut item_notices: MessageReader<ItemNotice>,
    mut field_notices: MessageReader<FieldNotice>,
    mut memo: Local<crate::ui_script::VmMemo<FeedMemo>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let memo = memo.get(&script);
    let Some((store, guid)) = reads.self_q.iter().next() else {
        if memo.player.take().is_some() {
            script.set_quest_log_player(false);
        }
        return;
    };
    let store = &store.0;
    let slots_moved = field_notices
        .read()
        .any(|n| matches!(n, FieldNotice::QuestLog));
    let mut item_changes: Vec<u32> = Vec::new();
    let mut bags_moved = !reads.changed_items.is_empty();
    for notice in item_notices.read() {
        bags_moved = true;
        // `0x4df7b0`'s three callers: the record set up, the stack count, the deactivation.
        if let ItemNotice::Record { entry, .. }
        | ItemNotice::StackCount { entry, .. }
        | ItemNotice::Destroyed { entry, .. } = *notice
        {
            item_changes.push(entry);
        }
    }

    let seat = memo.player != Some(guid.0);
    // The rebuild's misses, each looked up with a callback (`0x4de65f`) below.
    let asks = script.take_quest_record_asks();
    let landing = !quest_log.fresh.is_empty() || !asks.is_empty();
    // The macro inputs, read when a record is to be built or a name may have landed.
    let subject = (seat || landing || reads.names.is_changed() || reads.states.is_changed())
        .then(|| {
            crate::npc_text::player_identity(&reads.self_q, &reads.names, reads.unit_names.get())
        })
        .flatten();
    let macros = crate::npc_text::MacroContext {
        subject: subject.as_ref(),
        states: &reads.states,
    };
    let tables = RecordTables {
        headers: reads.header_names.as_deref(),
        tags: reads.tag_names.as_deref(),
        macros: &macros,
        spells: reads.spells.as_deref(),
        factions: reads
            .factions
            .as_deref()
            .map(crate::target::Factions::catalog),
    };
    let key: Option<SubjectKey> = subject
        .as_ref()
        .map(|s| (s.name.clone(), s.race.clone(), s.class.clone(), s.gender));

    if seat {
        // The reference's records and slots outlive a `/reload` in its caches and descriptor; a
        // new VM is seated with both before its world-enter cascade rebuilds the list.
        // So are the creature names, which the leaderboard reads off the cache (`0xc0e354`).
        memo.player = Some(guid.0);
        script.set_quest_log_player(true);
        for (&id, t) in quest_log.templates.iter() {
            script.seed_quest_record(id, record_view(t, &tables));
        }
        for (entry, name) in reads.names.creature_names() {
            script.set_quest_creature_name(entry, name.to_string());
        }
        memo.creatures_seen = reads.names.creatures_landed_since(usize::MAX).1;
        script.set_quest_slots(slot_views(store));
        memo.prior = None;
        memo.subject = key;
    } else if (subject.is_some() && memo.subject != key)
        || (reads.states.is_changed()
            && quest_log.templates.iter().any(|(_, t)| {
                has_world_state_token(&t.details) || has_world_state_token(&t.objectives_text)
            }))
    {
        // The text expands against the player: a name that lands, or a world state a quest's text
        // reads, expands the held records again, with nothing waiting on them.
        if subject.is_some() {
            memo.subject = key;
        }
        for (&id, t) in quest_log.templates.iter() {
            script.seed_quest_record(id, record_view(t, &tables));
        }
    }

    // The templates the packets landed: all held, then the rebuild callbacks they carry.
    let landed: Vec<(u32, QuestRecordView)> = std::mem::take(&mut quest_log.fresh)
        .into_iter()
        .filter_map(|id| Some((id, record_view(quest_log.templates.get(id)?, &tables))))
        .collect();
    let records_landed = !landed.is_empty();
    if records_landed {
        script.land_quest_records(landed);
    }
    for id in std::mem::take(&mut quest_log.refused) {
        script.refuse_quest_record(id);
    }
    let held_now: Vec<(u32, QuestRecordView)> = asks
        .into_iter()
        .filter_map(|id| {
            let t = quest_log.template(id, Ask::Event(&reads.commands))?;
            Some((id, record_view(t, &tables)))
        })
        .collect();
    let records_landed = records_landed || !held_now.is_empty();
    if !held_now.is_empty() {
        script.land_quest_records(held_now);
    }

    // Every creature name that lands goes to the engine unprompted, as the reference's leaderboard
    // reads the one cache; a miss is asked, and its refusal only logs (`0x4e07dc`).
    let (landed, seen) = reads.names.creatures_landed_since(memo.creatures_seen);
    for &entry in landed {
        if let Some(name) = reads.names.resolve_creature(entry, 0, Ask::Read) {
            script.set_quest_creature_name(entry, name.to_string());
        }
        quest_log.creature_waits.remove(&entry);
    }
    memo.creatures_seen = seen;
    for entry in script.take_quest_creature_asks() {
        match reads
            .names
            .resolve_creature(entry, 0, Ask::Event(&reads.commands))
        {
            Some(name) => script.set_quest_creature_name(entry, name.to_string()),
            None => {
                quest_log.creature_waits.insert(entry);
            }
        }
    }
    if !quest_log.creature_waits.is_empty() {
        let names = &reads.names;
        let mut refused = Vec::new();
        quest_log.creature_waits.retain(|&entry| {
            let gone = names.creature_answered_unknown(entry);
            if gone {
                refused.push(entry);
            }
            !gone
        });
        for entry in refused {
            script.refuse_quest_creature(entry);
        }
    }

    // The slot watcher's accept line, whose catalog row carries `QUESTADDED`. The seat is silent:
    // the initial create fills the whole log.
    if seat || slots_moved {
        let now: SlotStates = std::array::from_fn(|slot| {
            store
                .player_quest_log(slot as u8)
                .map_or((0, 0), |s| (s.quest_id, s.state))
        });
        if let Some(prior) = memo.prior.as_ref() {
            for quest_id in accepted_quests(prior, &now) {
                quest_lines.push(crate::ui_quest::QuestLine::Accepted(quest_id));
            }
        }
        memo.prior = Some(now);
    }

    // The held counts the item lines read (`0x622130`), current before the field notices fire.
    if seat || slots_moved || bags_moved || records_landed {
        let held: HashMap<u32, u32> = (0..PLAYER_QUEST_LOG_SLOTS)
            .filter_map(|slot| store.player_quest_log(slot))
            .filter_map(|s| quest_log.templates.get(s.quest_id))
            .flat_map(|t| t.objectives.iter().map(|o| o.item_id))
            .filter(|&id| id != 0)
            .collect::<HashSet<u32>>()
            .into_iter()
            .map(|id| {
                let count = count_of(store, &reads.objects, id, InventoryScope::QUEST_ITEMS);
                (id, count)
            })
            .collect();
        if held != memo.held {
            script.set_quest_item_counts(held.clone());
            memo.held = held;
        }
    }

    for _ in 0..std::mem::take(&mut quest_log.faction_rank_moves) {
        script.quest_log_faction_rank_changed();
    }
    // What the expiry latch owes the givers (`0x4df531`).
    if script.take_quest_giver_refreshes() > 0 {
        quest_giver.bump_reask();
    }
    // The update packets lead the object updates they report (vmangos `Player.cpp:10249`, and the
    // kill's counter `:14570` before `:14575`), so the toasts go first.
    if !quest_log.toasts.is_empty() {
        show_toasts(&mut quest_log, &mut script, &mut sink);
    }
    for entry in item_changes {
        script.quest_item_changed(entry);
    }
    script.flush_quest_log_refires();
}

/// The quest-update toasts the handlers resolved, each its catalog row, then `QUEST_WATCH_UPDATE`
/// with the quest's row (`0x4df880`, 0 when it has none); the complete arm with no end text shows
/// `ERR_QUEST_UNKNOWN_COMPLETE` and returns before the signal (`0x5e5d44`).
fn show_toasts(
    quest_log: &mut QuestLog,
    script: &mut UiScript,
    sink: &mut crate::ui_action::MessageSink,
) {
    for toast in std::mem::take(&mut quest_log.toasts) {
        let line = match toast.line {
            ToastLine::Count {
                key,
                name,
                cur,
                need,
            } => {
                let error = crate::ui_action::UiError::args(
                    key,
                    vec![
                        crate::ui_action::FillArg::S(name),
                        crate::ui_action::FillArg::D(i64::from(cur)),
                        crate::ui_action::FillArg::D(i64::from(need)),
                    ],
                );
                let get = |key: &str| script.lua().globals().get::<String>(key).ok();
                crate::ui_action::ui_error_text(&error, &get)
                    .map(|text| crate::ui_action::Shown::keyed(error.key, text))
            }
            ToastLine::Complete(end_text) if !end_text.is_empty() => {
                crate::ui_action::keyed_line_s(
                    script,
                    "ERR_QUEST_OBJECTIVE_COMPLETE_S",
                    &[&end_text],
                )
            }
            ToastLine::Complete(_) => {
                let line = crate::ui_action::keyed_line(script, "ERR_QUEST_UNKNOWN_COMPLETE");
                crate::ui_action::show_messages(script, sink, "ui_quest_log", line);
                continue;
            }
        };
        crate::ui_action::show_messages(script, sink, "ui_quest_log", line);
        let row = script.quest_log_index(toast.quest_id);
        script.fire_event("QUEST_WATCH_UPDATE", vec![ScriptValue::Int(i64::from(row))]);
    }
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

/// `AbandonQuest()`'s sends: the slot of the row the engine found for the marked quest
/// (`0x4df070`).
fn drain_quest_log_abandons(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for slot in script.take_quest_log_abandons() {
        debug!("ui_quest_log: abandon slot {slot}");
        let _ = commands.0.send(ClientCommand::QuestlogRemove { slot });
    }
}

/// Quest-log fixtures for a bare VM: records and slots seated as the feed seats them.
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use benilla_protocol::messages::field::FIELD_PLAYER_QUEST_LOG_1_1;
    use benilla_protocol::messages::QuestObjective;
    use benilla_protocol::SessionEvent;
    use benilla_ui::script::ItemTemplateView;

    use super::*;
    use crate::items::Items;

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

    fn none() -> [QuestObjective; 4] {
        std::array::from_fn(|_| obj(0, 0, 0, 0, ""))
    }

    /// Quest `quest_id` in group 0, the reference's missing header, so it is row 2.
    fn quest_template(quest_id: u32, objectives: [QuestObjective; 4]) -> QuestTemplate {
        let mut t = crate::ui_quest::test_template(quest_id, "A Threat Within", 0);
        t.zone_or_sort = 0;
        t.objectives = objectives;
        t
    }

    /// `0x555410`: an answer with an empty title is a refusal (`0x55543c` → `0x563150`), not a
    /// stored record (`0x562c80`); a titled one is stored.
    #[test]
    fn an_empty_quest_title_is_a_refusal_not_a_record() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut log = QuestLog::default();
        assert!(log.template(77, Ask::New(&commands)).is_none());
        assert_eq!(rx.try_iter().count(), 1);

        let mut t = quest_template(77, none());
        t.title = String::new();
        log.insert_template(t.clone());
        assert!(log.templates.get(77).is_none(), "nothing is stored");
        assert!(
            log.template_answered_unknown(77),
            "the waiters read unknown"
        );

        t.title = "Kobold Camp Cleanup".into();
        log.insert_template(t);
        assert!(log.templates.get(77).is_some());
        assert!(!log.template_answered_unknown(77));
    }

    #[test]
    fn money_sign_splits_into_required_or_reward() {
        assert_eq!(money_split(-500), (500, 0));
        assert_eq!(money_split(500), (0, 500));
        assert_eq!(money_split(0), (0, 0));
    }

    #[test]
    fn the_slot_watcher_announces_a_filled_slot_and_a_cleared_failure_only() {
        use quest_slot_state::FAIL;
        let prior = [(0, 0), (7, 0), (8, FAIL), (9, FAIL), (10, 0), (11, 0)];
        let now = [(783, 0), (7, 0), (8, 0), (9, FAIL), (12, 0), (0, 0)];
        assert_eq!(
            accepted_quests(&prior, &now),
            [783, 8],
            "an empty slot filled, and the same quest with its fail bit cleared; a kept quest, a \
             kept failure, a slot swapping quests and a cleared slot say nothing"
        );
    }

    const WOLF_PELT: u32 = 2000;
    const ITEM_ENTRY_FIELD: u16 = 3; // OBJECT_FIELD_ENTRY
    const ITEM_STACK_FIELD: u16 = 14; // ITEM_FIELD_STACK_COUNT
    const PACK_SLOT_FIELD: u16 = 532; // PLAYER_FIELD_PACK_SLOT_1, two fields a guid

    /// Quest 7, wanting five pelts.
    fn pelt_quest() -> QuestTemplate {
        let mut objectives = none();
        objectives[0] = obj(0, 0, WOLF_PELT, 5, "");
        quest_template(7, objectives)
    }

    #[test]
    fn an_added_item_reads_the_quests_need_and_the_held_count() {
        let t = pelt_quest();
        let name = || Some("Wolf Pelt".to_string());
        let toast = |held, count| item_added_to_quest(&t, WOLF_PELT, count, || held, name);
        assert_eq!(
            toast(2, 1),
            ItemAdd::Toast(Toast {
                quest_id: 7,
                line: ToastLine::Count {
                    key: "ERR_QUEST_ADD_ITEM_SII",
                    name: "Wolf Pelt".into(),
                    cur: 3,
                    need: 5
                }
            })
        );
        // The shown count is capped at the need.
        assert!(matches!(
            toast(4, 3),
            ItemAdd::Toast(Toast {
                line: ToastLine::Count { cur: 5, .. },
                ..
            })
        ));
        // A need already met shows nothing, and ends the walk.
        assert_eq!(toast(5, 1), ItemAdd::Silent);
        // An item the quest does not need moves the walk on.
        assert_eq!(
            item_added_to_quest(&t, 999, 1, || 0, name),
            ItemAdd::NotNeeded
        );
        // An item record not cached is silent, and ends the walk.
        assert_eq!(
            item_added_to_quest(&t, WOLF_PELT, 1, || 0, || None),
            ItemAdd::Silent
        );
    }

    /// The kill line's three keys (`0x5e5db0`-`0x5e5e5b`), and a name the caches miss shows
    /// nothing.
    #[test]
    fn a_kill_line_takes_the_override_text_then_the_cached_name() {
        let mut objectives = none();
        objectives[0] = obj(55, 10, 0, 0, "");
        objectives[1] = obj(56, 3, 0, 0, "Kobold Candle found");
        objectives[2] = obj(0x8000_0010, 2, 0, 0, "");
        let t = quest_template(7, objectives);
        let count = |key: &'static str, name: &str, cur, need| {
            Some(ToastLine::Count {
                key,
                name: name.into(),
                cur,
                need,
            })
        };
        let go = |entry: u32| (entry == 0x10).then(|| "Mysterious Crate".to_string());
        let creature = |entry: u32| (entry == 55).then(|| "Kobold Vermin".to_string());
        assert_eq!(
            kill_line(&t, 55, 3, 10, go, creature),
            count("ERR_QUEST_ADD_KILL_SII", "Kobold Vermin", 3, 10)
        );
        assert_eq!(
            kill_line(&t, 56, 1, 3, go, creature),
            count("ERR_QUEST_ADD_FOUND_SII", "Kobold Candle found", 1, 3),
            "the override text, no cache read"
        );
        assert_eq!(
            kill_line(&t, 0x8000_0010, 1, 2, go, creature),
            count("ERR_QUEST_ADD_FOUND_SII", "Mysterious Crate", 1, 2)
        );
        assert_eq!(kill_line(&t, 57, 1, 2, go, creature), None);
        assert_eq!(kill_line(&t, 0x8000_0011, 1, 2, go, creature), None);
    }

    // ── The feed over one player ────────────────────────────────────────────────────────────────

    /// The feed, the quest log's field watcher and the abandon drain over one player, the script
    /// logging the quest events as `name:arg1`.
    struct Rig {
        app: App,
        player: Entity,
        rx: crossbeam_channel::Receiver<ClientCommand>,
        /// The player's descriptor, rewritten whole at each change.
        fields: BTreeMap<u16, u32>,
    }

    impl Rig {
        fn new(templates: Vec<QuestTemplate>, log: &[u32]) -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            app.insert_resource(NetCommands(tx))
                .init_resource::<crate::net::GuidIndex>()
                .init_resource::<NameCache>()
                .init_resource::<Items>()
                .init_resource::<crate::world_state::WorldStates>()
                .init_resource::<crate::ui_chat::ChatLog>()
                .init_resource::<crate::sound::MessageSounds>()
                .init_resource::<crate::ui_quest::QuestLines>()
                .init_resource::<crate::ui_quest::QuestGiver>()
                .init_resource::<crate::ui_unit::field_events::FieldNoticeCursor>()
                .init_resource::<QuestLog>()
                .add_message::<ItemNotice>()
                .add_message::<FieldNotice>();
            net::register(&mut app);
            for t in templates {
                app.world_mut()
                    .resource_mut::<QuestLog>()
                    .insert_template(t);
            }
            let player = app
                .world_mut()
                .spawn((ObjectStore(ObjectFields::default()), Guid(0x2A), SelfPlayer))
                .id();
            app.insert_non_send_resource(Self::vm());
            app.add_systems(
                Update,
                (
                    feed_quest_log,
                    crate::ui_unit::field_events::fire_field_notices,
                    drain_quest_log_abandons,
                )
                    .chain(),
            );
            let mut rig = Self {
                app,
                player,
                rx,
                fields: BTreeMap::new(),
            };
            rig.write_log(log);
            rig
        }

        /// A VM logging the quest events, with the format keys the lines fill.
        fn vm() -> UiScript {
            let script = UiScript::new().unwrap();
            script
                .run(
                    r#"
                    SEEN = {}
                    QUEST_ITEMS_NEEDED = "%s: %d/%d"
                    QUEST_MONSTERS_KILLED = "%s slain: %d/%d"
                    ERR_QUEST_ADD_ITEM_SII = "%s: %d/%d"
                    ERR_QUEST_ADD_KILL_SII = "%s slain: %d/%d"
                    ERR_QUEST_OBJECTIVE_COMPLETE_S = "%s (Complete)"
                    ERR_QUEST_UNKNOWN_COMPLETE = "Objective Complete."
                    local f = CreateFrame("Frame")
                    f:RegisterEvent("QUEST_LOG_UPDATE")
                    f:RegisterEvent("QUEST_WATCH_UPDATE")
                    f:RegisterEvent("UI_INFO_MESSAGE")
                    f:RegisterEvent("UI_ERROR_MESSAGE")
                    f:SetScript("OnEvent", function() table.insert(SEEN, event .. ":" .. tostring(arg1)) end)
                "#,
                )
                .unwrap();
            script
        }

        /// A `/reload`: a new VM over the same caches and descriptor.
        fn reload(&mut self) {
            self.app.insert_non_send_resource(Self::vm());
        }

        fn script(&mut self) -> Mut<'_, UiScript> {
            self.app.world_mut().non_send_resource_mut::<UiScript>()
        }

        fn write_store(&mut self) {
            let pairs: Vec<(u16, u32)> = self.fields.iter().map(|(&f, &v)| (f, v)).collect();
            self.app
                .world_mut()
                .get_mut::<ObjectStore>(self.player)
                .unwrap()
                .0 = ObjectFields::from_pairs(&pairs);
        }

        /// The log's slots hold `log`, from slot 0, with no notice.
        fn write_log(&mut self, log: &[u32]) {
            for slot in 0..PLAYER_QUEST_LOG_SLOTS {
                let id = log.get(usize::from(slot)).copied().unwrap_or(0);
                self.fields
                    .insert(FIELD_PLAYER_QUEST_LOG_1_1 + 3 * u16::from(slot), id);
            }
            self.write_store();
        }

        /// A values block that moved the log: the slots and the watcher's notice.
        fn set_log(&mut self, log: &[u32]) {
            self.write_log(log);
            self.app.world_mut().write_message(FieldNotice::QuestLog);
        }

        /// A values block that moved slot 0's timer word alone.
        fn touch_log(&mut self, timer: u32) {
            self.fields.insert(FIELD_PLAYER_QUEST_LOG_1_1 + 2, timer);
            self.write_store();
            self.app.world_mut().write_message(FieldNotice::QuestLog);
        }

        fn notify(&mut self, notice: ItemNotice) {
            self.app.world_mut().write_message(notice);
        }

        /// A backpack stack as the wire leaves it: the item's own descriptor, and the player's slot
        /// field naming its guid. The notices are the caller's.
        fn set_stack(&mut self, slot: u16, guid: u64, count: u32) {
            let world = self.app.world_mut();
            let fields = ObjectFields::from_pairs(&[
                (ITEM_ENTRY_FIELD, WOLF_PELT),
                (ITEM_STACK_FIELD, count),
            ]);
            let known = world
                .resource::<crate::net::GuidIndex>()
                .0
                .get(&guid)
                .copied();
            match known {
                Some(e) => {
                    world.entity_mut(e).insert(ObjectStore(fields));
                }
                None => {
                    crate::items::test_spawn_item(world, guid, fields, false);
                }
            }
            self.fields.insert(PACK_SLOT_FIELD + 2 * slot, guid as u32);
            self.fields
                .insert(PACK_SLOT_FIELD + 2 * slot + 1, (guid >> 32) as u32);
            self.write_store();
        }

        /// The world-enter cascade, as the unit feed runs it.
        fn enter_world(&mut self) -> Vec<String> {
            self.script().fire_world_enter();
            self.seen()
        }

        fn seen(&mut self) -> Vec<String> {
            let s = self.script();
            let out = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            out
        }

        /// One frame, then the events the script saw in it.
        fn frame(&mut self) -> Vec<String> {
            self.app.update();
            self.seen()
        }

        /// `n` frames, then every event the script saw in them.
        fn frames(&mut self, n: usize) -> Vec<String> {
            for _ in 0..n {
                self.app.update();
            }
            self.seen()
        }

        fn sent(&self) -> Vec<ClientCommand> {
            self.rx.try_iter().collect()
        }

        fn packet(&mut self, ev: SessionEvent) {
            crate::net::handlers::dispatch(self.app.world_mut(), vec![ev]);
        }

        fn eval(&mut self, lua: &str) -> String {
            self.script().eval::<String>(lua).unwrap()
        }

        fn creature(&mut self, entry: u32, name: &str) {
            self.app
                .world_mut()
                .resource_mut::<NameCache>()
                .insert_creature(
                    entry,
                    Some(crate::names::CreatureRecord {
                        name: name.into(),
                        subname: None,
                        creature_type: 7,
                        pet_family: 0,
                        rank: 0,
                        type_flags: 0,
                        civilian: false,
                        racial_leader: false,
                        display_id: 0,
                    }),
                );
        }

        /// The pelt's record in the app's item cache and the engine's mirror.
        fn cache_pelt(&mut self) {
            self.app
                .world_mut()
                .resource_mut::<Items>()
                .insert_template(WOLF_PELT, Some(crate::items::test_template("Wolf Pelt")));
            self.script().set_item_template(
                WOLF_PELT,
                ItemTemplateView {
                    name: "Wolf Pelt".into(),
                    quality: 1,
                    ..Default::default()
                },
            );
        }
    }

    const QLU: &str = "QUEST_LOG_UPDATE:nil";

    /// The seat is silent: the world entry rebuilds and signals once (`0x4de430`); each values
    /// block that moves the log runs the watcher once (`0x4de4d0`), and a frame nothing moved in
    /// signals nothing and sends nothing.
    #[test]
    fn the_log_signals_at_the_world_entry_and_once_per_block_that_moves_it() {
        let mut rig = Rig::new(
            vec![quest_template(7, none()), quest_template(783, none())],
            &[7],
        );
        assert_eq!(rig.frame(), Vec::<String>::new());
        assert_eq!(rig.enter_world(), [QLU]);
        assert_eq!(rig.eval("return (GetQuestLogTitle(2))"), "A Threat Within");

        rig.set_log(&[7, 783]);
        assert_eq!(rig.frame(), [QLU]);
        assert_eq!(
            rig.app
                .world()
                .resource::<crate::ui_quest::QuestLines>()
                .queued(),
            [crate::ui_quest::QuestLine::Accepted(783)]
        );
        rig.touch_log(60);
        assert_eq!(rig.frame(), [QLU], "a timer word is a move too");
        assert_eq!(rig.frames(300), Vec::<String>::new());
        assert!(rig.sent().is_empty());
    }

    /// An addon that walks every getter at each `QUEST_LOG_UPDATE`, counting its walks.
    const WALKER: &str = r#"
        WALKS = 0
        local w = CreateFrame("Frame")
        w:RegisterEvent("QUEST_LOG_UPDATE")
        w:SetScript("OnEvent", function()
            WALKS = WALKS + 1
            for i = 1, GetNumQuestLogEntries() do
                GetQuestLogTitle(i)
                SelectQuestLogEntry(i)
                for j = 1, GetNumQuestLeaderBoards(i) do GetQuestLogLeaderBoard(j, i) end
                for k = 1, GetNumQuestLogRewards() do GetQuestLogRewardInfo(k) end
            end
        end)
    "#;

    /// Twenty quests, each with a creature (`100 + i`), an item (`3000 + i`) and a reward
    /// (`4000 + i`).
    fn twenty_quests() -> Vec<QuestTemplate> {
        (1..=20)
            .map(|i| {
                let mut objectives = none();
                objectives[0] = obj(100 + i, 5, 3000 + i, 4, "");
                let mut t = quest_template(i, objectives);
                t.rewards[0] = (4000 + i, 1);
                t
            })
            .collect()
    }

    /// The twenty quests' items in the VM, as the item feed seats a new one.
    fn hold_items(rig: &mut Rig) {
        for i in 1..=20 {
            for item in [3000 + i, 4000 + i] {
                rig.script().set_item_template(
                    item,
                    ItemTemplateView {
                        name: format!("Item {item}"),
                        quality: 1,
                        ..Default::default()
                    },
                );
            }
        }
        rig.script().run(WALKER).unwrap();
    }

    /// With every name the leaderboard reads already cached, as the reference's persisted caches
    /// hold them, the world entry's one update is the walk's only one, after a `/reload` too, and
    /// the idle frames after do nothing.
    #[test]
    fn a_walking_addon_on_twenty_cached_quests_walks_once_at_each_world_entry() {
        let log: Vec<u32> = (1..=20).collect();
        let mut rig = Rig::new(twenty_quests(), &log);
        for i in 1..=20 {
            rig.creature(100 + i, &format!("Beast {i}"));
        }
        hold_items(&mut rig);
        rig.frame();
        assert_eq!(rig.enter_world(), [QLU]);
        assert_eq!(rig.frame(), Vec::<String>::new(), "every lookup hit");
        assert_eq!(
            rig.eval("return (GetQuestLogLeaderBoard(1, 2))"),
            "Beast 1 slain: 0/5"
        );
        assert_eq!(rig.frames(300), Vec::<String>::new());
        assert!(rig.sent().is_empty(), "no frame asks anything");
        assert_eq!(rig.eval("return tostring(WALKS)"), "1");

        rig.reload();
        hold_items(&mut rig);
        rig.frame();
        assert_eq!(rig.enter_world(), [QLU]);
        assert_eq!(
            rig.frames(10),
            Vec::<String>::new(),
            "the new VM was seated"
        );
        assert!(rig.sent().is_empty());
        assert_eq!(rig.eval("return tostring(WALKS)"), "1");
    }

    /// Names in flight: each that lands signals once (`0x4e07d0`), whatever the frames it lands
    /// in, and an idle frame after does nothing.
    #[test]
    fn creature_names_in_flight_signal_once_each_as_they_land() {
        let log: Vec<u32> = (1..=20).collect();
        let mut rig = Rig::new(twenty_quests(), &log);
        hold_items(&mut rig);
        rig.frame();
        assert_eq!(rig.enter_world(), [QLU]);
        rig.frame();
        let asked = crate::query_cache::queries(&rig.rx);
        assert_eq!(asked.len(), 20, "one ask a creature: {asked:?}");
        for i in 1..=10 {
            rig.creature(100 + i, &format!("Beast {i}"));
        }
        assert_eq!(rig.frame(), vec![QLU; 10]);
        for i in 11..=15 {
            rig.creature(100 + i, &format!("Beast {i}"));
        }
        assert_eq!(rig.frame(), vec![QLU; 5]);
        for i in 16..=20 {
            rig.creature(100 + i, &format!("Beast {i}"));
        }
        assert_eq!(rig.frame(), vec![QLU; 5]);
        assert_eq!(rig.frames(300), Vec::<String>::new());
        assert!(crate::query_cache::queries(&rig.rx).is_empty());
        assert_eq!(rig.eval("return tostring(WALKS)"), "21");
    }

    /// The rebuild's misses ask with the callback `0x4de8b0` and hold the list (`[0xbb7498]`):
    /// the last landing rebuilds and signals once.
    #[test]
    fn the_last_awaited_record_rebuilds_the_list_once() {
        let mut rig = Rig::new(vec![], &[7, 783]);
        rig.frame();
        assert_eq!(rig.enter_world(), [QLU], "the list without the two");
        assert_eq!(rig.eval("return tostring(GetNumQuestLogEntries())"), "0");
        rig.frame();
        let asked: Vec<_> = crate::query_cache::queries(&rig.rx);
        assert!(
            matches!(
                asked[..],
                [
                    ClientCommand::QuestQuery { quest: 7 },
                    ClientCommand::QuestQuery { quest: 783 }
                ]
            ),
            "{asked:?}"
        );
        rig.packet(SessionEvent::QuestTemplate(Box::new(quest_template(
            7,
            none(),
        ))));
        assert_eq!(rig.frame(), Vec::<String>::new(), "one still awaited");
        rig.touch_log(5);
        assert_eq!(
            rig.frame(),
            Vec::<String>::new(),
            "the watcher's rebuild returns at once"
        );
        rig.packet(SessionEvent::QuestTemplate(Box::new(quest_template(
            783,
            none(),
        ))));
        assert_eq!(rig.frame(), [QLU]);
        assert_eq!(rig.eval("return tostring(GetNumQuestLogEntries())"), "3");
    }

    /// A refusal runs the rebuild's callback with status 0, which leaves the count (`0x4de8bc`):
    /// the list holds and nothing asks again until the next world entry zeroes it (`0x4de492`).
    #[test]
    fn a_refused_quest_holds_the_list_until_the_next_world_entry() {
        let mut rig = Rig::new(vec![], &[101]);
        rig.frame();
        rig.enter_world();
        rig.frame();
        assert!(matches!(
            crate::query_cache::queries(&rig.rx)[..],
            [ClientCommand::QuestQuery { quest: 101 }]
        ));
        let mut refused = quest_template(101, none());
        refused.title = String::new();
        rig.packet(SessionEvent::QuestTemplate(Box::new(refused)));
        assert_eq!(rig.frames(300), Vec::<String>::new());
        rig.touch_log(5);
        assert_eq!(rig.frames(300), Vec::<String>::new());
        assert!(crate::query_cache::queries(&rig.rx).is_empty());
        assert_eq!(rig.enter_world(), [QLU]);
        rig.frame();
        assert_eq!(crate::query_cache::queries(&rig.rx).len(), 1);
    }

    /// `SMSG_QUESTUPDATE_ADD_KILL` (`0x5e5d61`): the line with the packet's counts, then
    /// `QUEST_WATCH_UPDATE` with the quest's row; an uncached name shows nothing.
    #[test]
    fn a_kill_toasts_its_line_and_watches_the_quests_row() {
        let mut objectives = none();
        objectives[0] = obj(55, 10, 0, 0, "");
        objectives[1] = obj(56, 10, 0, 0, "");
        let mut rig = Rig::new(vec![quest_template(7, objectives)], &[7]);
        rig.creature(55, "Kobold Vermin");
        rig.frame();
        rig.enter_world();
        let kill = |entry| SessionEvent::QuestObjectiveKill {
            quest_id: 7,
            entry,
            count: 3,
            required: 10,
        };
        rig.packet(kill(55));
        assert_eq!(
            rig.frame(),
            [
                "UI_INFO_MESSAGE:Kobold Vermin slain: 3/10",
                "QUEST_WATCH_UPDATE:2"
            ]
        );
        rig.packet(kill(56));
        assert_eq!(rig.frame(), Vec::<String>::new());
    }

    /// `SMSG_QUESTUPDATE_COMPLETE` (`0x5e5d12`): the end text with `QUEST_WATCH_UPDATE`, or with
    /// none `ERR_QUEST_UNKNOWN_COMPLETE` alone; a failed quest shows nothing.
    #[test]
    fn a_completed_quest_toasts_its_end_text() {
        let mut ended = quest_template(7, none());
        ended.end_text = "Slay 10 kobolds".into();
        let mut rig = Rig::new(vec![ended, quest_template(8, none())], &[7, 8]);
        rig.frame();
        rig.enter_world();
        rig.packet(SessionEvent::QuestObjectivesComplete { quest_id: 7 });
        assert_eq!(
            rig.frame(),
            [
                "UI_INFO_MESSAGE:Slay 10 kobolds (Complete)",
                "QUEST_WATCH_UPDATE:2"
            ]
        );
        rig.packet(SessionEvent::QuestObjectivesComplete { quest_id: 8 });
        assert_eq!(rig.frame(), ["UI_INFO_MESSAGE:Objective Complete."]);
        // Slot 0's state byte, the second dword's top byte: failed.
        rig.fields.insert(
            FIELD_PLAYER_QUEST_LOG_1_1 + 1,
            u32::from(quest_slot_state::FAIL) << 24,
        );
        rig.write_store();
        rig.packet(SessionEvent::QuestObjectivesComplete { quest_id: 7 });
        assert_eq!(rig.frame(), Vec::<String>::new());
    }

    /// The template's end text reaches the record as the wire sent it: the event line, first
    /// (`0x4e0289`).
    #[test]
    fn the_end_text_is_the_leaderboards_event_line() {
        let mut t = pelt_quest();
        t.end_text = "Bring the pelts to $N.".into();
        let mut rig = Rig::new(vec![t], &[7]);
        rig.cache_pelt();
        rig.frame();
        rig.enter_world();
        assert_eq!(rig.eval("return tostring(GetNumQuestLeaderBoards(2))"), "2");
        assert_eq!(
            rig.eval("local t, k = GetQuestLogLeaderBoard(1, 2); return t .. '|' .. k"),
            "Bring the pelts to $N.|event"
        );
        assert_eq!(
            rig.eval("local t, k = GetQuestLogLeaderBoard(2, 2); return t .. '|' .. k"),
            "Wolf Pelt: 0/5|item"
        );
    }

    /// Shift-splitting a held stack (`CMSG_SPLIT_ITEM`): vmangos shrinks the source at once and
    /// seats the new stack at the map's batched update (`Player.cpp:11100-11116`, `10324-10327`),
    /// with no `SMSG_QUESTUPDATE_*`. The restack and the new stack's record each run the item
    /// trigger (`0x5d9519`, `0x5d85d9`), one `QUEST_LOG_UPDATE`, and no toast.
    #[test]
    fn splitting_a_held_stack_signals_the_log_and_toasts_nothing() {
        let mut rig = Rig::new(vec![pelt_quest()], &[7]);
        rig.cache_pelt();
        rig.set_stack(0, 0x100, 4);
        rig.frame();
        rig.enter_world();
        let board = "local text = GetQuestLogLeaderBoard(1, 2); return text";
        assert_eq!(rig.eval(board), "Wolf Pelt: 4/5");

        rig.set_stack(0, 0x100, 3);
        rig.notify(ItemNotice::StackCount {
            guid: 0x100,
            entry: WOLF_PELT,
        });
        assert_eq!(rig.frame(), [QLU]);
        assert_eq!(rig.eval(board), "Wolf Pelt: 3/5", "the dip the split makes");

        rig.set_stack(1, 0x101, 1);
        rig.notify(ItemNotice::Record {
            guid: 0x101,
            entry: WOLF_PELT,
        });
        rig.notify(ItemNotice::Seat {
            old: None,
            new: Some(WOLF_PELT),
        });
        assert_eq!(rig.frame(), [QLU], "the seat itself runs no trigger");
        assert_eq!(rig.eval(board), "Wolf Pelt: 4/5");
    }

    /// A pickup is `SMSG_QUESTUPDATE_ADD_ITEM` ahead of the item's update (`Player.cpp:10249`):
    /// the toast counts the held pelts plus the added one (`0x5dd060`), then the restack signals.
    #[test]
    fn a_picked_up_item_toasts_once_from_its_packet() {
        let mut rig = Rig::new(vec![pelt_quest()], &[7]);
        rig.cache_pelt();
        rig.set_stack(0, 0x100, 4);
        rig.frame();
        rig.enter_world();
        rig.packet(SessionEvent::QuestObjectiveItem {
            item_id: WOLF_PELT,
            count: 1,
        });
        rig.set_stack(0, 0x100, 5);
        rig.notify(ItemNotice::StackCount {
            guid: 0x100,
            entry: WOLF_PELT,
        });
        assert_eq!(
            rig.frame(),
            [
                "UI_INFO_MESSAGE:Wolf Pelt: 5/5",
                "QUEST_WATCH_UPDATE:2",
                QLU
            ]
        );
        assert_eq!(rig.frame(), Vec::<String>::new());
    }

    /// An added item for a quest whose record is not cached waits for it (`0x5dd180`), a
    /// callback registered after the rebuild's: the landing rebuilds, then toasts.
    #[test]
    fn an_item_added_before_the_quest_record_lands_toasts_on_its_arrival() {
        let mut rig = Rig::new(vec![], &[7]);
        rig.cache_pelt();
        rig.set_stack(0, 0x100, 4);
        rig.frame();
        rig.enter_world();
        rig.frame();
        assert!(rig
            .sent()
            .iter()
            .any(|c| matches!(c, ClientCommand::QuestQuery { quest: 7 })));
        rig.packet(SessionEvent::QuestObjectiveItem {
            item_id: WOLF_PELT,
            count: 1,
        });
        assert_eq!(
            rig.frame(),
            Vec::<String>::new(),
            "nothing to name the quest by yet"
        );
        rig.packet(SessionEvent::QuestTemplate(Box::new(pelt_quest())));
        assert_eq!(
            rig.frame(),
            [
                QLU,
                "UI_INFO_MESSAGE:Wolf Pelt: 5/5",
                "QUEST_WATCH_UPDATE:2"
            ]
        );
    }

    /// `SetAbandonQuest` (`0x4dfb50`) copies the selection `0xbb7480`, a quest id (`0x4def30`,
    /// `0x4def5d`), into the mark `0xbb7484`, and `AbandonQuest` (`0x4dfe00`, `0x4df070`) finds its
    /// row in `0xbb71c0` again, so a fold while the popup is up cannot retarget the abandon.
    #[test]
    fn a_fold_between_mark_and_confirm_does_not_retarget_the_abandon() {
        // Z1 = zone 0 (forced first) holding A; Z2 = zone 7 holding B and C. Rows: [Z1, A, Z2, B,
        // C]; B is row 4.
        let quest = |id, zone, title: &str| {
            let mut t = quest_template(id, none());
            t.zone_or_sort = zone;
            t.title = title.into();
            t
        };
        let mut rig = Rig::new(
            vec![quest(101, 0, "A"), quest(201, 7, "B"), quest(202, 7, "C")],
            &[101, 201, 202],
        );
        rig.frame();
        rig.enter_world();
        rig.script()
            .run("SelectQuestLogEntry(4); SetAbandonQuest()")
            .unwrap();
        assert_eq!(rig.eval("return GetAbandonQuestName()"), "B");
        // With the popup up, fold Z1: B moves to row 3 and C takes row 4.
        rig.script().run("CollapseQuestHeader(1)").unwrap();
        rig.frame();
        rig.script().run("AbandonQuest()").unwrap();
        rig.frame();
        let sent: Vec<_> = rig
            .sent()
            .into_iter()
            .filter(|c| matches!(c, ClientCommand::QuestlogRemove { .. }))
            .collect();
        assert!(
            matches!(sent.as_slice(), [ClientCommand::QuestlogRemove { slot: 1 }]),
            "the abandon removes B's slot (1), never C's (2): {sent:?}"
        );
    }

    /// Each `SMSG_QUERY_TIME_RESPONSE` signals once (`0x4de400` into `0x4def1b`); a new VM does
    /// not hear a response it was not there for.
    #[test]
    fn each_time_response_signals_the_log_once() {
        let mut app = App::new();
        app.init_resource::<crate::net::ServerWallClock>()
            .insert_non_send_resource(Rig::vm())
            .add_systems(Update, feed_server_clock);
        let seen = |app: &mut App| {
            let s = app.world_mut().non_send_resource_mut::<UiScript>();
            let out = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            out
        };
        app.update();
        assert_eq!(seen(&mut app), Vec::<String>::new());
        let sample = |app: &mut App, t| {
            app.world_mut()
                .resource_mut::<crate::net::ServerWallClock>()
                .sample(t, 0);
        };
        sample(&mut app, 1_000_000);
        app.update();
        assert_eq!(seen(&mut app), [QLU]);
        app.update();
        assert_eq!(seen(&mut app), Vec::<String>::new());
        app.insert_non_send_resource(Rig::vm());
        app.update();
        assert_eq!(seen(&mut app), Vec::<String>::new(), "the old response");
        sample(&mut app, 1_003_600);
        sample(&mut app, 1_003_601);
        app.update();
        assert_eq!(seen(&mut app), [QLU, QLU]);
    }

    /// A sample whose local second is the server's is the reference's zero offset, which it
    /// stores as 1 (`0x4def02`); the feed tells the VM, and a later skewed sample clears it.
    #[test]
    fn a_zero_offset_sync_reaches_the_vm() {
        let mut app = App::new();
        app.init_resource::<crate::net::ServerWallClock>()
            .insert_non_send_resource(Rig::vm())
            .add_systems(Update, feed_server_clock);
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        let mut q = fixture::quest(7, "Deliver the Message", 1);
        q.record.objectives[0] = fixture::kill(40, 3);
        q.slot.timer = 4_000_000_000;
        fixture::seat(&mut script, &[q]);
        let left = |app: &mut App, server: u32, local: i64| {
            app.world_mut()
                .resource_mut::<crate::net::ServerWallClock>()
                .sample(server, local);
            app.update();
            let s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.eval::<i64>("return GetQuestTimers()").unwrap()
        };
        let skewed = left(&mut app, 1_000_000, 1_000_005);
        let zero = left(&mut app, 1_000_000, 1_000_000);
        assert!(app
            .world()
            .resource::<crate::net::ServerWallClock>()
            .zero_offset());
        assert_eq!(zero, skewed + 1, "a second more");
        assert_eq!(left(&mut app, 1_000_000, 999_999), skewed);
    }

    /// A standing packet that moved a rank signals once (`0x4d58f3` into `0x4de52f`).
    #[test]
    fn a_moved_faction_rank_signals_the_log_once() {
        let mut rig = Rig::new(vec![quest_template(7, none())], &[7]);
        rig.frame();
        rig.enter_world();
        rig.app
            .world_mut()
            .resource_mut::<QuestLog>()
            .note_faction_rank_move();
        assert_eq!(rig.frame(), [QLU]);
        assert_eq!(rig.frame(), Vec::<String>::new());
    }

    /// A refused record frees the item adds waiting on it with no walk (`0x5dd1a4`).
    #[test]
    fn an_item_added_before_a_refused_record_toasts_nothing() {
        let mut rig = Rig::new(vec![], &[7]);
        rig.cache_pelt();
        rig.set_stack(0, 0x100, 4);
        rig.frame();
        rig.enter_world();
        rig.frame();
        rig.packet(SessionEvent::QuestObjectiveItem {
            item_id: WOLF_PELT,
            count: 1,
        });
        rig.frame();
        let mut refused = pelt_quest();
        refused.title = String::new();
        rig.packet(SessionEvent::QuestTemplate(Box::new(refused)));
        assert_eq!(rig.frame(), Vec::<String>::new());
        rig.packet(SessionEvent::QuestTemplate(Box::new(pelt_quest())));
        let seen = rig.frame();
        assert!(
            !seen.iter().any(|e| e.starts_with("UI_INFO_MESSAGE")),
            "the pair went with the refusal: {seen:?}"
        );
    }

    /// The count skips a broken copy (`0x6224fc`-`0x62250e`), and a durability change, which runs no
    /// item watcher, is read all the same.
    #[test]
    fn a_broken_copy_leaves_the_held_count() {
        const DURABILITY: u16 = 46; // ITEM_FIELD_DURABILITY
        const MAX_DURABILITY: u16 = 47;
        let mut rig = Rig::new(vec![pelt_quest()], &[7]);
        rig.cache_pelt();
        rig.set_stack(0, 0x100, 4);
        rig.frame();
        rig.enter_world();
        let board = "local text = GetQuestLogLeaderBoard(1, 2); return text";
        assert_eq!(rig.eval(board), "Wolf Pelt: 4/5");
        let item = rig.app.world().resource::<crate::net::GuidIndex>().0[&0x100];
        rig.app
            .world_mut()
            .get_mut::<ObjectStore>(item)
            .unwrap()
            .0
            .merge(ObjectFields::from_pairs(&[
                (MAX_DURABILITY, 20),
                (DURABILITY, 0),
            ]));
        rig.frame();
        assert_eq!(rig.eval(board), "Wolf Pelt: 0/5");
    }

    /// A quest whose row holds the expiry latch is done to the update packets (`0x5e5b6c`,
    /// `0x5e5c16`): no kill line and no item line. The timers' read shows the failure line in chat
    /// before its `QUEST_LOG_UPDATE` (`0x4e1529`, `0x4e1559`), the line's sound and log follow,
    /// and the latch's quest-giver refresh reaches the givers (`0x4df531`).
    #[test]
    fn a_latched_quest_takes_no_update_and_its_failure_reaches_the_givers() {
        let mut objectives = none();
        objectives[0] = obj(55, 10, 0, 0, "");
        objectives[1] = obj(0, 0, WOLF_PELT, 5, "");
        let mut rig = Rig::new(vec![quest_template(7, objectives)], &[7]);
        rig.app.init_resource::<crate::ui_chat::ChatWindows>();
        rig.app.add_systems(Update, crate::ui_chat::tee_shown_lines);
        rig.creature(55, "Kobold Vermin");
        rig.cache_pelt();
        rig.frame();
        rig.enter_world();
        rig.script()
            .run(
                r#"
                ERR_QUEST_FAILED_S = "%s failed."
                local c = CreateFrame("Frame")
                c:RegisterEvent("CHAT_MSG_SYSTEM")
                c:SetScript("OnEvent", function() table.insert(SEEN, event .. ":" .. arg1) end)
            "#,
            )
            .unwrap();
        // The deadline ran out with the row made: the timers' read latches it.
        rig.fields.insert(FIELD_PLAYER_QUEST_LOG_1_1 + 2, 1000);
        rig.write_store();
        let slots = slot_views(&rig.app.world().get::<ObjectStore>(rig.player).unwrap().0);
        rig.script().set_quest_slots(slots);
        rig.script().set_server_unix_time(2000.0);
        rig.script().run("GetQuestTimers()").unwrap();
        assert_eq!(rig.seen(), ["CHAT_MSG_SYSTEM:A Threat Within failed.", QLU]);
        let epoch = rig
            .app
            .world()
            .resource::<crate::ui_quest::QuestGiver>()
            .reask_epoch();
        let failed_sounds = |rig: &Rig| {
            let sounds = rig.app.world().resource::<crate::sound::MessageSounds>();
            sounds
                .queued()
                .iter()
                .filter(|r| r.key == "ERR_QUEST_FAILED_S")
                .count()
        };
        assert_eq!(failed_sounds(&rig), 0);
        rig.frame();
        assert_eq!(failed_sounds(&rig), 1, "the catalog row's sound");
        let mut giver = rig
            .app
            .world_mut()
            .resource_mut::<crate::ui_quest::QuestGiver>();
        assert_eq!(giver.reask_epoch(), epoch.wrapping_add(1));
        assert!(giver.take_messages().is_empty(), "shown once, in the call");
        rig.packet(SessionEvent::QuestObjectiveKill {
            quest_id: 7,
            entry: 55,
            count: 3,
            required: 10,
        });
        rig.packet(SessionEvent::QuestObjectiveItem {
            item_id: WOLF_PELT,
            count: 1,
        });
        assert_eq!(rig.frame(), Vec::<String>::new());
    }

    /// The slot watcher runs on the log's slots, not its rows: an accepted quest is announced before
    /// its record lands, and the seat is silent.
    #[test]
    fn a_quest_entering_the_log_queues_its_accept_line() {
        use crate::ui_quest::{QuestLine, QuestLines};
        let mut rig = Rig::new(vec![], &[7]);
        rig.frame();
        assert!(
            rig.app.world().resource::<QuestLines>().queued().is_empty(),
            "the seat fills the log silently"
        );
        rig.set_log(&[7, 783]);
        rig.frame();
        assert_eq!(
            rig.app.world().resource::<QuestLines>().queued(),
            [QuestLine::Accepted(783)]
        );
        assert!(
            rig.script().take_sounds().is_empty(),
            "the sound is the accept row's, played with its line"
        );
    }
}

#[cfg(test)]
pub(crate) mod fixture {
    use benilla_ui::script::{
        QuestObjectiveView, QuestRecordView, QuestSlotView, UiScript, QUEST_LOG_SLOTS,
    };

    /// One quest of a fixture log: its id, record and slot.
    #[derive(Clone)]
    pub(crate) struct LogQuest {
        pub(crate) id: u32,
        pub(crate) record: QuestRecordView,
        pub(crate) slot: QuestSlotView,
    }

    /// Quest `id` titled `title`, at `level`, under the Elwynn Forest header, in progress.
    pub(crate) fn quest(id: u32, title: &str, level: u32) -> LogQuest {
        LogQuest {
            id,
            record: QuestRecordView {
                title: title.into(),
                level,
                zone_or_sort: 12,
                header: "Elwynn Forest".into(),
                ..Default::default()
            },
            slot: QuestSlotView {
                quest_id: id,
                ..Default::default()
            },
        }
    }

    /// A creature objective of `entry` needing `count`.
    pub(crate) fn kill(entry: u32, count: u32) -> QuestObjectiveView {
        QuestObjectiveView {
            creature_or_go: entry,
            required_count: count,
            ..Default::default()
        }
    }

    /// `quests` in slots 0 on, each record held, then the log's field change: the rebuild and its
    /// `QUEST_LOG_UPDATE`, and the callbacks it left fired.
    pub(crate) fn seat(s: &mut UiScript, quests: &[LogQuest]) {
        let mut slots = [QuestSlotView::default(); QUEST_LOG_SLOTS];
        for (i, q) in quests.iter().enumerate() {
            s.seed_quest_record(q.id, q.record.clone());
            slots[i] = q.slot;
        }
        s.set_quest_log_player(true);
        s.quest_log_changed(slots);
        s.flush_quest_log_refires();
    }
}
