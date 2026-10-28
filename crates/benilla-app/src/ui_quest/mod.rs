//! The questgiver window's app side: the handlers fill [`QuestGiver`] and its cells, which the
//! getters read through the host at the call ([`live`]), and queue each panel's event and line on
//! the questgiver lane at their packets ([`lane`]); the verbs apply in their calls
//! ([`QuestVerbs`]), the teardowns among them.

use std::collections::HashMap;

use benilla_protocol::messages::{
    QuestDetails, QuestGiverList, QuestOfferReward, QuestRequestItems, QuestRewardItem,
    QuestShareMsg,
};
use bevy::prelude::*;

use benilla_ui::script::{QuestItemView, QuestPanel, QuestRewardSpell, QuestState};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_action::{Spells, UiError};
use crate::ui_script::{SweepApp, SweepLeg};
use crate::ui_session::{LatchApp, LatchedWindow, NpcWindow};

/// The wire packet that opened the current panel.
pub(crate) enum QuestView {
    Greeting(QuestGiverList),
    Detail(QuestDetails),
    Progress(QuestRequestItems),
    Reward(QuestOfferReward),
}

/// The open questgiver window; the per-guid dialog statuses survive a close.
#[derive(Resource, Default)]
pub(crate) struct QuestGiver {
    pub(crate) completed_fanfare: bool,
    /// The panel's source: an NPC, a GameObject, a quest-starter item or a sharing player.
    pub(crate) npc: Option<u64>,
    pub(crate) view: Option<QuestView>,
    /// The reference's latch `0xbe0824`, written by DETAILS and OFFER_REWARD (`0x500ef0`) and by
    /// REQUEST_ITEMS' `closeOnCancel` (`0x501070`), and zeroed only on world enter (`0x500af0`).
    /// Non-zero turns a plain unit's decline into the silent teardown ([`decline_leg`]). vmangos
    /// sends 1 on every DETAILS and OFFER_REWARD; REQUEST_ITEMS carries 1 on a single-quest
    /// auto-open and 0 on `COMPLETE_QUEST` (`QuestHandler.cpp:390-395`).
    pub(crate) close_on_cancel: u32,
    /// The reference's already-acted latch `0xbe0844`, set by `AcceptQuest` and by a re-opening
    /// `DeclineQuest`, which keeps the panel up; both refuse while it is set (`0x5013a8`,
    /// `0x5013fe`) until a panel publish clears it ([`Self::open`]).
    pub(crate) acted: bool,
    /// The reference's `0xbe0828`: the chosen reward's item id, latched by `GetQuestReward`
    /// (`0x50161a`) for the turn-in's item line (`0x5dc676`-`0x5dc6c6`), and zeroed by a panel
    /// publish (`0x500d8c`), world enter (`0x500b15`) and the turn-in.
    pub(crate) chosen_reward: u32,
    /// The greeting's two pools, which only the selects read.
    pub(crate) pools: QuestPools,
    /// The reference's `0xbe0840`: the last escort confirm's quest, written by its packet
    /// (`0x5019c7`), never cleared, and sent by `ConfirmAcceptQuest` (`0x5021e0`).
    pub(crate) confirm: u32,
    /// Per-guid dialog status from `SMSG_QUESTGIVER_STATUS`, the `!`/`?` marker's value.
    statuses: HashMap<u64, u32>,
    /// The lines a handler raised, each a GlobalStrings key and its fills, which the handler moves
    /// onto the lane ([`lane::QuestGiverLane::push_lines`]).
    messages: Vec<UiError>,
    reask: u32,
    /// The dialog statuses dropped at the reference's typemask-8 lookup (GameObject answers, which
    /// vmangos sends), per guid, so a live probe can tell a drop from a query never sent.
    refused: HashMap<u64, u32>,
    refused_count: u32,
    /// The cells the getters read ([`live`]).
    pub(crate) cells: QuestCells,
}

/// The panel's cells the getters read, each written by its panel's setters and kept past a close
/// until another writes it: the texts by `0x500bd0` (a greeting clearing all six first,
/// `0x500c88`-`0x500ca6`) and `0x500dc0`, the pools by the greeting's appends (`0x500e40`,
/// `0x500e90`), the rows, title, money and spell by `0x500ef0` (an offer or a reward) and the
/// required rows, title, flag and wanted money by `0x501070` (a progress). The world entry zeroes
/// all but the reward money and the spell (`0x500af0`).
#[derive(Clone, Debug, Default)]
pub(crate) struct QuestCells {
    /// The texts, the pools' titles and levels, the money and the flag; the rows are below.
    pub(crate) state: QuestState,
    pub(crate) choices: Vec<QuestRewardItem>,
    pub(crate) rewards: Vec<QuestRewardItem>,
    pub(crate) required: Vec<QuestRewardItem>,
    /// `0xbe0830`, the reward spell's id.
    pub(crate) reward_spell: u32,
    /// `0xbe081c`, the last panel's quest id (0 for a greeting), stored by every panel
    /// (`0x500d6e`) and zeroed at world enter (`0x500b01`).
    pub(crate) quest: u32,
}

impl QuestCells {
    /// `view`'s setters, each text through `expand` (`0x506f70`).
    fn write(&mut self, view: &QuestView, expand: &dyn Fn(&str) -> String) {
        // `0x500bd0`: the expansion, an empty one stored as `" "` (`0x500cd8`).
        let text = |src: &str| match expand(src) {
            t if t.is_empty() => " ".to_string(),
            t => t,
        };
        // `0x500ef0` and `0x501070` store the title as sent, an empty one as `" "` (`0x501008`).
        let title = |t: &str| if t.is_empty() { " " } else { t }.to_string();
        self.quest = match view {
            QuestView::Greeting(_) => 0,
            QuestView::Detail(d) => d.quest_id,
            QuestView::Progress(p) => p.quest_id,
            QuestView::Reward(o) => o.quest_id,
        };
        let st = &mut self.state;
        match view {
            QuestView::Greeting(l) => {
                *st = QuestState {
                    panel: QuestPanel::Greeting,
                    greeting: text(&l.greeting),
                    ..QuestState {
                        reward_money: st.reward_money,
                        required_money: st.required_money,
                        completable: st.completable,
                        ..QuestState::default()
                    }
                };
                for q in &l.quests {
                    let (titles, levels) = if row_is_active(q.icon) {
                        (&mut st.active_titles, &mut st.active_levels)
                    } else {
                        (&mut st.available_titles, &mut st.available_levels)
                    };
                    titles.push(q.title.clone());
                    levels.push(q.level);
                }
            }
            QuestView::Detail(d) => {
                st.panel = QuestPanel::Detail;
                st.quest_text = text(&d.details);
                // `0x500dc0`: an empty objective stays empty (`0x500e2d`).
                st.objectives = if d.objectives.is_empty() {
                    String::new()
                } else {
                    expand(&d.objectives)
                };
                self.offer(
                    &d.title,
                    &d.choices,
                    &d.rewards,
                    d.money,
                    d.reward_spell,
                    &title,
                );
            }
            QuestView::Progress(p) => {
                st.panel = QuestPanel::Progress;
                st.progress_text = text(&p.request_text);
                // `0x501070`: the required rows alone, the title, the flag and the wanted money.
                st.title = title(&p.title);
                st.completable = p.is_complete;
                st.required_money = p.required_money;
                self.required = p.required_items.clone();
            }
            QuestView::Reward(o) => {
                st.panel = QuestPanel::Reward;
                st.reward_text = text(&o.offer_text);
                self.offer(
                    &o.title,
                    &o.choices,
                    &o.rewards,
                    o.money,
                    o.reward_spell,
                    &title,
                );
            }
        }
    }

    /// `0x500ef0`: every row cleared (`0x500f04`), the choices and rewards, the title, the money
    /// split by its sign (`0x501022`-`0x50103b`) and the spell (`0x50104a`).
    fn offer(
        &mut self,
        t: &str,
        choices: &[QuestRewardItem],
        rewards: &[QuestRewardItem],
        money: i32,
        spell: u32,
        title: &dyn Fn(&str) -> String,
    ) {
        self.state.title = title(t);
        self.choices = choices.to_vec();
        self.rewards = rewards.to_vec();
        self.required.clear();
        (self.state.reward_money, self.state.required_money) = if money < 0 {
            (0, money.unsigned_abs())
        } else {
            (money as u32, 0)
        };
        self.reward_spell = spell;
    }

    /// `0x500af0`'s zeroes.
    fn world_enter(&mut self) {
        *self = Self {
            state: QuestState {
                panel: QuestPanel::Greeting,
                reward_money: self.state.reward_money,
                ..QuestState::default()
            },
            reward_spell: self.reward_spell,
            ..Self::default()
        };
    }
}

/// The quest-giver refreshes the quest log's expiry latch owes (`0x4df531`), counted apart from
/// the window, inside the call or the handler that latched; the markers fold it into their re-ask
/// epoch.
#[derive(Resource, Default)]
pub(crate) struct LatchRefreshes(pub(crate) u32);

/// The greeting's pools, the available quests (`0xbdfe60`, its count `0xbe0834`) with each row's
/// one-click flag, and the active ones (`0xbde690`, `0xbe0838`): 32 rows each, cleared whole and
/// refilled by a greeting (`0x500c41`, `0x500e40`, `0x500e90`) and zeroed at world enter
/// (`0x500b21`), so they outlive the greeting panel and the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestPools {
    pub(crate) available: Vec<(u32, bool)>,
    pub(crate) active: Vec<u32>,
}

impl QuestPools {
    /// The rows each pool holds.
    pub(crate) const ROWS: usize = 32;

    /// A greeting's rows: active for icon 3 or 4, else available, one-click for icon 0
    /// (`0x5dbbfe`-`0x5dbc13`).
    fn of(list: &QuestGiverList) -> Self {
        let mut pools = Self::default();
        for q in &list.quests {
            if row_is_active(q.icon) {
                pools.active.push(q.quest_id);
            } else {
                pools.available.push((q.quest_id, row_is_one_click(q.icon)));
            }
        }
        pools
    }
}

impl QuestGiver {
    /// Open (or replace) the window with a fresh wire view for `npc`, its texts unexpanded.
    pub(crate) fn open(&mut self, npc: u64, view: QuestView) {
        self.open_expanded(npc, view, &|t| t.to_string());
    }

    /// Open (or replace) the window with a fresh wire view for `npc`, its texts through `expand`.
    /// Answers whether the reward panel owes the quest-list-complete chime: the setter compares
    /// the quest id with the last panel's before storing it (`0x500d36`-`0x500d3c`).
    fn open_expanded(
        &mut self,
        npc: u64,
        view: QuestView,
        expand: &dyn Fn(&str) -> String,
    ) -> bool {
        let chime = matches!(&view, QuestView::Reward(o) if o.quest_id == self.cells.quest);
        if let QuestView::Greeting(list) = &view {
            self.pools = QuestPools::of(list);
        }
        self.cells.write(&view, expand);
        self.npc = Some(npc);
        self.view = Some(view);
        // `0x500bd0` clears the already-acted latch (`0x500d91`) and the chosen reward
        // (`0x500d8c`) on every panel publish.
        self.acted = false;
        self.chosen_reward = 0;
        chime
    }

    /// Whether the cell `[0xbe0810]` holds a source.
    #[allow(dead_code)]
    pub(crate) fn is_open(&self) -> bool {
        self.npc.is_some()
    }

    /// The one-shot turn-in fanfare, which the reference plays from the packet, not from Lua.
    pub(crate) fn take_completed_fanfare(&mut self) -> bool {
        std::mem::take(&mut self.completed_fanfare)
    }

    /// Zero the cell; the panel and its kind `[0xbe0818]` stay, as `0x501130` leaves them, for
    /// the panel verbs, and the feed shows nothing with the cell zero.
    pub(crate) fn clear(&mut self) {
        self.npc = None;
    }

    pub(crate) fn set_status(&mut self, npc: u64, status: u32) {
        self.statuses.insert(npc, status);
    }

    /// Every stored dialog status, which [`crate::quest_markers`] renders.
    pub(crate) fn statuses(&self) -> &HashMap<u64, u32> {
        &self.statuses
    }

    /// Drop the status of every guid `live` rejects: the reference keeps it on the unit
    /// (`unit+0xcb8`), so it dies with the object, and a map needs the prune.
    pub(crate) fn retain_statuses(&mut self, live: impl Fn(u64) -> bool) {
        self.statuses.retain(|&npc, _| live(npc));
        // The refusals follow the same lifetime.
        self.refused.retain(|&npc, _| live(npc));
    }

    /// Drop one guid's status when it stops being a questgiver, as the reference's teardown does.
    pub(crate) fn clear_status(&mut self, npc: u64) {
        self.statuses.remove(&npc);
    }

    /// Re-ask every visible questgiver's status, as the reference does after
    /// `SMSG_SET_FACTION_STANDING`, `SMSG_GROUP_LIST`, a turn-in and `SMSG_QUESTUPDATE_*`. A
    /// counter, folded into the marker query's generation, so no bump is lost to ordering.
    pub(crate) fn bump_reask(&mut self) {
        self.reask = self.reask.wrapping_add(1);
    }

    pub(crate) fn reask_epoch(&self) -> u32 {
        self.reask
    }

    pub(crate) fn refuse_status(&mut self, npc: u64, status: u32) {
        self.refused.insert(npc, status);
        self.refused_count = self.refused_count.saturating_add(1);
    }

    pub(crate) fn refused_for(&self, npc: u64) -> Option<u32> {
        self.refused.get(&npc).copied()
    }

    pub(crate) fn refused_count(&self) -> u32 {
        self.refused_count
    }

    /// One guid's stored dialog status; the markers read the whole map instead.
    #[allow(dead_code)]
    pub(crate) fn status(&self, npc: u64) -> Option<u32> {
        self.statuses.get(&npc).copied()
    }

    /// Raise a line on the window, which the handler moves onto the lane; its key's catalog row
    /// picks the surface.
    pub(crate) fn push_message(&mut self, msg: UiError) {
        self.messages.push(msg);
    }

    pub(crate) fn take_messages(&mut self) -> Vec<UiError> {
        std::mem::take(&mut self.messages)
    }

    pub(crate) fn clear_session(&mut self) {
        self.clear();
        self.statuses.clear();
        self.messages.clear();
        self.close_on_cancel = 0;
        self.acted = false;
        self.chosen_reward = 0;
        self.view = None;
        self.pools = QuestPools::default();
        self.cells.world_enter();
    }

    /// The world-enter reset `0x500af0`'s cells.
    pub(crate) fn world_enter(&mut self) {
        self.cells.world_enter();
    }

    /// `GetQuestItemInfo`'s lookup of a template in flight, inside its call
    /// ([`benilla_ui::script::ItemCallback::Quest`]): the item cache asks a miss, a refused entry
    /// again, with the panel's source as the owner id (`[0xbe0810]` at `0x5017d3`, which the panel
    /// setter writes, `0x500d86`), and a miss keeps one callback `0x5018c0` per call (`0x5017ce`),
    /// which outlives the panel and signals at the record's answer
    /// ([`crate::items::RecordCallback::Quest`]).
    pub(crate) fn ask_item_record(&self, items: &Items, commands: &NetCommands, entry: u32) {
        if items
            .template(entry, self.npc.unwrap_or(0), Ask::Event(commands))
            .is_none()
        {
            items.wait(entry, crate::items::RecordCallback::Quest);
        }
    }
}

/// `SMSG_QUESTGIVER_QUEST_INVALID`'s `QuestFailedReason` as a GlobalStrings key, per the
/// reference's handler `0x5dbca0` (the `0x18f` arm of the quest demux `0x5e5910`), whose default,
/// `ERR_QUEST_NEED_PREREQS`, takes reason 0 and everything unlisted.
pub(crate) fn questgiver_invalid_key(reason: u32) -> &'static str {
    match reason {
        1 => "ERR_QUEST_FAILED_LOW_LEVEL",         // msgId 142
        6 => "ERR_QUEST_FAILED_WRONG_RACE",        // msgId 144
        12 => "ERR_QUEST_ONLY_ONE_TIMED",          // msgId 146
        13 => "ERR_QUEST_ALREADY_ON",              // msgId 148
        20 => "ERR_QUEST_FAILED_MISSING_ITEMS",    // msgId 143
        22 => "ERR_QUEST_FAILED_NOT_ENOUGH_MONEY", // msgId 145
        _ => "ERR_QUEST_NEED_PREREQS",             // msgId 147, the default
    }
}

/// `SMSG_QUESTGIVER_QUEST_FAILED`'s reason as a GlobalStrings key, per the reference's handler
/// `0x5dc840` (the `0x192` arm); each line's `%s` is the quest's title (`questRecord+0x9c`).
pub(crate) fn questgiver_failed_key(reason: u32) -> &'static str {
    match reason {
        4 | 50 => "ERR_QUEST_FAILED_BAG_FULL_S", // msgId 140
        17 => "ERR_QUEST_FAILED_MAX_COUNT_S",    // msgId 141
        _ => "ERR_QUEST_FAILED_S",               // msgId 139
    }
}

mod lane;
pub(crate) mod lines;
mod live;
mod net;
#[cfg(test)]
pub(crate) mod verb_tests;
mod verbs;

pub(crate) use lane::{QuestGiverEvent, QuestGiverLane};
#[cfg(test)]
pub(crate) use lines::test_template;
pub(crate) use lines::{QuestLine, QuestLines};
pub(crate) use live::QuestGiverLive;
pub(crate) use verbs::QuestVerbs;

pub(crate) struct UiQuestPlugin;

impl Plugin for UiQuestPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        lines::register(app);
        lane::add_quest_giver_lane(app);
        app.init_resource::<QuestGiver>()
            .init_resource::<LatchRefreshes>()
            .add_latched_window::<QuestGiver>()
            .add_sweep_leg(SweepLeg::Quest, close_quest_on_leave_world);
    }
}

/// The world-enter cascade's reset `0x500af0` (`0x49099c`) in its place
/// ([`crate::ui_world_enter`]), which a login, a worldport and a `/reload` all run.
pub(crate) fn world_enter_cascade(giver: Option<ResMut<QuestGiver>>) {
    if let Some(mut giver) = giver {
        net::world_enter_reset(&mut giver);
    }
}

/// The leave-world sweep's quest leg (`0x490ac6` → `0x500bc0` → `0x501130(0, 1)`): the core with
/// its report set, so a shared quest is declined to its sharer.
fn close_quest_on_leave_world(world: &mut World) {
    close_core(world, true);
}

/// The giver's cell `[0xbe0810]`, which an item's or a GameObject's guid fills as well as an
/// NPC's. `0x493310` calls the core with its report set (`0x4933d3`), and so does `CloseQuest`
/// (`0x501a10`, inside its call), so a walk-away from a shared quest still declines it while an
/// NPC's panel closes with no giver re-open.
impl LatchedWindow for QuestGiver {
    const WINDOW: NpcWindow = NpcWindow::Quest;

    fn cell(&self) -> Option<u64> {
        self.npc
    }

    fn dispatched(world: &mut World) {
        close_core(world, true);
    }
}

/// The panel's one teardown `0x501130(0, report)`, as `0x493310` and the sweep enter it: nothing
/// with no giver (`0x501149`); with `report`, a sharing player is sent `MSG_QUEST_PUSH_RESULT`
/// DECLINE (`0x501159`-`0x5011e0`), as the reference's player lookup there resolves only a
/// player; then the cell is zeroed (`0x501201`) before the latch release (`0x50120d`), so the core
/// never re-enters, and `QUEST_FINISHED` fires once (`0x501217`). The panel kind `[0xbe0818]`
/// stays, and so do the cells. The verbs run it inside their calls
/// ([`crate::script_calls::ScriptCallHost`]); outside one it runs behind the lane, so its
/// `QUEST_FINISHED` never fires ahead of a panel's event queued before it.
pub(crate) fn close_core(world: &mut World, report: bool) {
    let Some(giver) = world.resource::<QuestGiver>().npc else {
        return;
    };
    if report && benilla_protocol::guid::is_player(giver) {
        debug!("ui_quest: closing {giver:#x}'s shared quest — declining it");
        if let Some(net) = world.get_resource::<NetCommands>() {
            let _ = net.0.send(ClientCommand::QuestPushResult {
                sharer: giver,
                msg: QuestShareMsg::DECLINE_QUEST,
            });
        }
    }
    lane::behind_lane(world, |world| {
        world.resource_mut::<QuestGiver>().clear();
        crate::ui_session::release(world, giver);
        crate::ui_session::fire(world, |script| {
            script.fire_event("QUEST_FINISHED", vec![]);
        });
    });
}

/// What the panel setters expand a text against (`0x506f70` with the active player, `0x500cc9`):
/// the player's name, race, class and gender, and the world states; and the giver's name the
/// stock frame reads at the panel's event.
#[derive(bevy::ecs::system::SystemParam)]
struct PanelMacros<'w, 's> {
    self_q: Query<'w, 's, (&'static ObjectStore, &'static Guid), With<SelfPlayer>>,
    names: Option<Res<'w, NameCache>>,
    commands: Option<Res<'w, NetCommands>>,
    index: Option<Res<'w, crate::net::GuidIndex>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    states: Option<Res<'w, crate::world_state::WorldStates>>,
    unit_names: crate::ui_unit::NameTables<'w>,
    /// The 2D cues `PlaySoundByName` plays, absent in a harness without sound.
    sounds: Option<Res<'w, crate::sound::MessageSounds>>,
}

/// The reward panel's chime (`0x841530`, played at `0x500d43`).
const QUEST_LIST_COMPLETE_SOUND: &str = "igQuestListComplete";

/// Whether the object store holds `guid` as a unit, a game object or an item (`0x468460`, then
/// the type tests at `0x500c16`-`0x500c35`: `TYPEMASK_UNIT` 0x8, `_GAMEOBJECT` 0x20, `_ITEM` 0x2).
pub(crate) fn giver_held(world: &World, guid: u64) -> bool {
    const GIVER_TYPES: u32 = 0x8 | 0x20 | 0x2;
    world
        .resource::<crate::net::GuidIndex>()
        .0
        .get(&guid)
        .and_then(|&e| world.get::<ObjectStore>(e))
        .and_then(|s| s.0.object_type_mask())
        .is_some_and(|mask| mask & GIVER_TYPES != 0)
}

/// Make `guid` a held unit for a test that opens a panel on it, keeping the object it already
/// has (`TYPEMASK_OBJECT | _UNIT`).
#[cfg(test)]
pub(crate) fn hold_for_test(world: &mut World, guid: u64) {
    let fields = benilla_protocol::ObjectFields::from_pairs(&[(2, 0x9)]);
    world.init_resource::<crate::net::GuidIndex>();
    let held = world
        .resource::<crate::net::GuidIndex>()
        .0
        .get(&guid)
        .copied();
    match held {
        Some(e) => match world.get_mut::<ObjectStore>(e) {
            // A unit, a game object or an item it already is stays as it is.
            Some(mut store) if store.0.object_type_mask().is_none_or(|m| m & 0x2a == 0) => {
                store.0.merge(fields)
            }
            Some(_) => {}
            None => {
                world.entity_mut(e).insert(ObjectStore(fields));
            }
        },
        None => {
            crate::items::test_spawn_object(world, guid, fields);
        }
    }
}

/// A panel's open (`0x500bd0`, every panel packet's): the latch first, with no test of the cell
/// (`0x500d74`), so a panel replacing the giver's own closes it, `QUEST_FINISHED` ahead of the new
/// panel's event; then the cells, each text expanded against the player as the packet finds them,
/// and the cell (`0x500d86`); then the panel's event at the setter's end (`0x500ee5`,
/// `0x50105d`, `0x501120`), on the lane at this packet.
pub(crate) fn open_panel(world: &mut World, npc: u64, view: QuestView) {
    // The setter resolves its giver first and ends when none is held as a unit, a game object or
    // an item (`0x500bf1`-`0x500c35` → `0x500d96`): no panel, no cell store, no cue.
    if !giver_held(world, npc) {
        debug!("ui_quest: no panel — giver {npc:#x} is not held");
        return;
    }
    crate::ui_session::register(world, npc);
    let panel = match &view {
        QuestView::Greeting(_) => QuestPanel::Greeting,
        QuestView::Detail(_) => QuestPanel::Detail,
        QuestView::Progress(_) => QuestPanel::Progress,
        QuestView::Reward(_) => QuestPanel::Reward,
    };
    let mut macros = bevy::ecs::system::SystemState::<PanelMacros>::new(world);
    world.resource_scope(|world, mut giver: Mut<QuestGiver>| {
        let m = macros.get(world);
        let subject = m.names.as_deref().and_then(|names| {
            crate::npc_text::player_identity(&m.self_q, names, m.unit_names.get())
        });
        let none = crate::world_state::WorldStates::default();
        let ctx = crate::npc_text::MacroContext {
            subject: subject.as_ref(),
            states: m.states.as_deref().unwrap_or(&none),
        };
        let chime = giver.open_expanded(npc, view, &|t| crate::npc_text::substitute(t, &ctx));
        // The reward panel after its quest's progress panel plays `igQuestListComplete` at the
        // setter (`0x500d3e`-`0x500d43`, `PlaySoundByName` `0x458030`).
        if chime {
            if let Some(sounds) = m.sounds.as_deref() {
                sounds.push_cue(QUEST_LIST_COMPLETE_SOUND);
            }
        }
        // The frame reads `UnitName("npc")` at the panel's event (`QuestFrame.lua:63`), which
        // looks the giver up with a callback (`0x5171b8`); its landing fires nothing.
        if let (Some(names), Some(commands)) = (m.names.as_deref(), m.commands.as_deref()) {
            let store = m
                .index
                .as_deref()
                .and_then(|i| i.0.get(&npc))
                .and_then(|&e| m.stores.get(e).ok());
            let _ = names.resolve_unit(npc, store, Ask::Event(commands));
        }
    });
    if let Some(mut lane) = world.get_resource_mut::<QuestGiverLane>() {
        lane.push(QuestGiverEvent::Panel(panel, npc));
    }
}

/// A greeting row is active when its wire icon is 3 or 4 and available otherwise, never by the
/// quest log (`0x5dbbfe`-`0x5dbc08`; gossip's rows too, `0x4e2430`, `0x4e2580`). vmangos's 3 and 4
/// are `DIALOG_STATUS_INCOMPLETE` and `_REWARD_REP` (`QuestDef.h:118-130`), and it marks an
/// auto-complete quest, never in the log, `_REWARD_REP` (`Player.cpp:12577`).
pub(crate) fn row_is_active(icon: u32) -> bool {
    matches!(icon, 3 | 4)
}

/// An available row with icon 0 is one-click: its select sends `COMPLETE_QUEST`, not
/// `QUERY_QUEST` (`0x5dbc13` stores the flag, `0x5012db` reads it). vmangos never sends icon 0 on
/// this packet.
pub(crate) fn row_is_one_click(icon: u32) -> bool {
    icon == 0
}

/// One wire item row for Lua; the name is `None` and the quality the seeded 0 (`0x501728`) until
/// the template lands. Read as `ask` says: `GetQuestItemInfo` looks it up with a callback
/// (`0x5017de`).
fn resolve_item(
    it: &QuestRewardItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    ask: Ask<'_>,
) -> QuestItemView {
    let template = items.template(it.item_id, 0, ask);
    let name = template.map(|t| t.name.clone());
    // The template's quality (`+0x1c`) for an item with an inventory type (`+0x2c`), else -1
    // (`0x5017fa`).
    let quality = template.map_or(0, |t| {
        if t.inventory_type != 0 {
            t.quality as i32
        } else {
            -1
        }
    });
    let texture = icons
        .and_then(|i| i.catalog.get(it.display_id))
        .and_then(|d| d.icon.clone());
    // `GetQuestItemLink`'s payload; `None` until the template gives the name and quality.
    let link = template.map(|t| crate::ui_items::item_link(it.item_id, &t.name, t.quality));
    QuestItemView {
        name,
        texture,
        count: it.count,
        quality,
        item_id: it.item_id,
        link,
    }
}

pub(super) fn resolve_items(
    src: &[QuestRewardItem],
    items: &Items,
    icons: Option<&ItemDisplays>,
    ask: Ask<'_>,
) -> Vec<QuestItemView> {
    src.iter()
        .map(|it| resolve_item(it, items, icons, ask))
        .collect()
}

/// The quest's reward spell from `rewSpell`, named through `Spell.dbc`; 0 is none, and an id the
/// catalog lacks still counts, with nothing to paint.
pub(crate) fn reward_spell_view(
    spell_id: u32,
    spells: Option<&Spells>,
) -> Option<QuestRewardSpell> {
    if spell_id == 0 {
        return None;
    }
    let d = spells.and_then(|s| s.catalog.get(spell_id));
    Some(QuestRewardSpell {
        spell_id,
        name: d.map(|d| d.name.clone()),
        texture: d.and_then(|d| d.icon.clone()),
        // `isTradeskillSpell`: `Spell.dbc` col 6 `Attributes` bit `0x20` (`0x501e59`).
        tradeskill: d.is_some_and(|d| d.attributes & 0x20 != 0),
    })
}

/// A panel's event, which its setter fires at its end (`0x500ee0`, `0x50105d`, `0x501120`).
fn panel_event(panel: QuestPanel) -> &'static str {
    match panel {
        QuestPanel::Greeting => "QUEST_GREETING",
        QuestPanel::Detail => "QUEST_DETAIL",
        QuestPanel::Progress => "QUEST_PROGRESS",
        QuestPanel::Reward => "QUEST_COMPLETE",
    }
}

/// The leg of `DeclineQuest()`'s core `0x5013f0` a source takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeclineLeg {
    /// The already-acted latch is set (`0x5013fe`) or the source does not resolve (`0x50142c`).
    Nothing,
    /// The teardown `0x501130(0,1)` (`0x5014aa`): declines to a sharer, closes the window.
    Teardown,
    /// A gossip-flagged unit (`0x50143b`-`0x501474`): `CMSG_GOSSIP_HELLO`; the window stays.
    GossipHello,
    /// A plain unit (`0x5014e4`-`0x501526`): `CMSG_QUESTGIVER_HELLO`; the window stays.
    QuestgiverHello,
}

/// `DeclineQuest`'s fork (`0x5013f0`, called by its binding `0x501d30`) on the source's type mask
/// and NPC flags, `None` when it is not in view; the re-open legs leave the window up. In order:
///
/// 1. the already-acted latch set (`0x5013fe`), or the source unresolved (`0x50142c`): nothing;
/// 2. a unit with the gossip NPC flag, and the active player (`0x50143b`-`0x50146b`):
///    `CMSG_GOSSIP_HELLO`, whatever `0xbe0824` says;
/// 3. an item, a player, or a non-zero `0xbe0824` (`0x501496`-`0x5014a2`): the teardown
///    `0x501130(0,1)`;
/// 4. a GameObject (`0x5014bf`): its interact virtual `[vtbl+0x60]` (`0x5014d0`), whose override
///    is untraced, so it takes the teardown here;
/// 5. anything else: `CMSG_QUESTGIVER_HELLO` (`0x501526`).
fn decline_leg(
    acted: bool,
    close_on_cancel: u32,
    source: Option<(u32, u32)>,
    player: bool,
) -> DeclineLeg {
    // `TYPEMASK_ITEM`, `_UNIT`, `_PLAYER` and `_GAMEOBJECT`.
    const ITEM: u32 = 0x2;
    const UNIT: u32 = 0x8;
    const PLAYER: u32 = 0x10;
    const GAMEOBJECT: u32 = 0x20;
    if acted {
        return DeclineLeg::Nothing;
    }
    let Some((mask, flags)) = source else {
        return DeclineLeg::Nothing;
    };
    if mask & UNIT != 0 && flags & crate::target::cursor_mode::npc_flags::GOSSIP != 0 && player {
        return DeclineLeg::GossipHello;
    }
    if mask & (ITEM | PLAYER) != 0 || close_on_cancel != 0 || mask & GAMEOBJECT != 0 {
        return DeclineLeg::Teardown;
    }
    DeclineLeg::QuestgiverHello
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_action::ui_error_text;
    use benilla_protocol::messages::dialog_status;
    use benilla_ui::script::UiScript;

    fn triple(id: u32) -> QuestRewardItem {
        QuestRewardItem {
            item_id: id,
            count: 1,
            display_id: 100 + id,
        }
    }

    #[test]
    fn greeting_split_reads_the_wire_icon() {
        // 3 = DIALOG_STATUS_INCOMPLETE, 4 = DIALOG_STATUS_REWARD_REP.
        assert!(row_is_active(3));
        assert!(row_is_active(4));
        // 5 = DIALOG_STATUS_AVAILABLE, 2 = DIALOG_STATUS_CHAT.
        assert!(!row_is_active(5));
        assert!(!row_is_active(2));
        // Every other value is available, including ones vmangos never sends.
        for icon in [0, 1, 6, 7, 100, u32::MAX] {
            assert!(!row_is_active(icon), "icon {icon} must fall to AVAILABLE");
        }
    }

    /// vmangos marks an auto-complete quest's row `DIALOG_STATUS_REWARD_REP` though the quest is
    /// never in the log (`Player.cpp:12577`).
    #[test]
    fn an_autocomplete_row_is_active_though_it_is_not_in_the_log() {
        const REWARD_REP: u32 = 4;
        assert!(
            row_is_active(REWARD_REP),
            "an auto-complete turn-in must bin ACTIVE without ever entering the quest log"
        );
        assert!(!row_is_one_click(REWARD_REP), "it is active, not one-click");
    }

    #[test]
    fn the_one_click_flag_is_icon_zero_alone() {
        assert!(row_is_one_click(0));
        for icon in [1, 2, 3, 4, 5, u32::MAX] {
            assert!(!row_is_one_click(icon));
        }
    }

    fn detail(quest_id: u32, details: &str, choices: Vec<QuestRewardItem>) -> QuestView {
        QuestView::Detail(QuestDetails {
            npc: 0x42,
            quest_id,
            title: "A Threat Within".into(),
            details: details.into(),
            objectives: "Slay 10.".into(),
            auto_finish: 1,
            choices,
            rewards: vec![triple(3000)],
            money: 1234,
            reward_spell: 0,
        })
    }

    fn progress(text: &str, required: Vec<QuestRewardItem>, complete: bool) -> QuestView {
        QuestView::Progress(QuestRequestItems {
            npc: 0x42,
            quest_id: 100,
            title: "A Threat Within".into(),
            request_text: text.into(),
            emote: 0,
            close_on_cancel: 1,
            required_money: 500,
            required_items: required,
            is_complete: complete,
        })
    }

    /// The cells as the getters read them over `items`, with no player.
    fn cells(giver: &QuestGiver, items: &Items) -> QuestState {
        live::state(&giver.cells, items, None, None, None, None)
    }

    /// `0x500bd0` expands the text (`0x500cc9`) and `0x500ef0` copies the rows, the title and the
    /// money; a row's record is read without a callback, so building the cells asks nothing: a
    /// reward is asked by `GetQuestItemInfo`'s call alone (`0x5017de`).
    #[test]
    fn detail_cells_carry_text_and_rows() {
        let mut giver = QuestGiver::default();
        giver.open_expanded(
            0x42,
            detail(100, "Kill kobolds, $N.", vec![triple(2000)]),
            &|t| t.replace("$N", "Tri"),
        );
        assert!(giver.is_open());
        let snap = cells(&giver, &Items::default());
        assert_eq!(snap.title, "A Threat Within");
        assert_eq!(snap.quest_text, "Kill kobolds, Tri.");
        assert_eq!(snap.objectives, "Slay 10.");
        assert_eq!(snap.choices.len(), 1);
        assert_eq!(snap.rewards.len(), 1);
        assert_eq!(snap.reward_money, 1234);
        // No template yet: no name, and the seeded quality 0 (`0x501728`).
        assert!(snap.rewards[0].name.is_none());
        assert_eq!(snap.rewards[0].quality, 0);
    }

    /// `GetQuestItemInfo`'s quality (`0x501700`): -1 for a template whose inventory type is 0
    /// (`0x5017fa`), its own quality otherwise, and the seeded 0 while the template is in flight
    /// (`0x501728`).
    #[test]
    fn a_giver_rows_quality_reads_minus_one_for_no_slot_and_zero_in_flight() {
        let mut items = Items::default();
        let mut boots = crate::items::test_template("Boots");
        (boots.inventory_type, boots.quality) = (8, 2);
        items.insert_template(3000, Some(crate::items::test_template("Ale")));
        items.insert_template(3001, Some(boots));
        let mut giver = QuestGiver::default();
        let view = QuestView::Detail(QuestDetails {
            npc: 0x42,
            quest_id: 100,
            title: "A Threat Within".into(),
            details: String::new(),
            objectives: String::new(),
            auto_finish: 1,
            choices: Vec::new(),
            rewards: vec![triple(3000), triple(3001), triple(3002)],
            money: 0,
            reward_spell: 0,
        });
        giver.open(0x42, view);
        let qualities: Vec<i64> = cells(&giver, &items)
            .rewards
            .iter()
            .map(|r| r.quality as i64)
            .collect();
        assert_eq!(qualities, [-1, 2, 0]);
    }

    /// The three texts are three buffers, each written by its own panel and kept by the others
    /// (`0x500cf6`'s jump table); only a greeting clears all six (`0x500c3b`, `0x500c88`-`0x500ca6`).
    /// An empty text is stored as `" "` (`0x500cd8`); an empty objective stays empty (`0x500e2d`).
    #[test]
    fn each_panel_writes_its_own_text_and_a_greeting_clears_them() {
        let mut giver = QuestGiver::default();
        giver.open(0x42, detail(100, "Quest text.", Vec::new()));
        giver.open(0x42, progress("", Vec::new(), true));
        let snap = cells(&giver, &Items::default());
        assert_eq!(
            snap.quest_text, "Quest text.",
            "the detail's, kept by the progress"
        );
        assert_eq!(snap.progress_text, " ", "an empty text is a space");
        assert_eq!(snap.reward_text, "");
        assert_eq!(snap.objectives, "Slay 10.");
        giver.open(0x42, greeting(&[]));
        let snap = cells(&giver, &Items::default());
        for (text, what) in [
            (&snap.quest_text, "quest"),
            (&snap.progress_text, "progress"),
            (&snap.objectives, "objectives"),
            (&snap.title, "title"),
        ] {
            assert!(text.is_empty(), "{what}: {text:?}");
        }
        assert_eq!(snap.greeting, "Hi");
    }

    /// `0x501070` writes the required rows alone, so the offer's rewards stay; `0x500ef0` clears
    /// every row (`0x500f04`). The turn-in flag reads the packet's and the player's held counts at
    /// the call (`0x501230`): no player, or a row held short, is nil.
    #[test]
    fn a_progress_keeps_the_offers_rows_and_completes_on_the_held_counts() {
        let mut giver = QuestGiver::default();
        giver.open(0x42, detail(100, "", vec![triple(2000)]));
        giver.open(0x42, progress("Tusks?", vec![triple(2001)], true));
        let items = Items::default();
        let held = |n: u32| move |_: u32| n;
        let state = |held: Option<&dyn Fn(u32) -> u32>| {
            live::state(&giver.cells, &items, None, None, None, held)
        };
        let snap = state(None);
        assert_eq!((snap.choices.len(), snap.rewards.len()), (1, 1));
        assert_eq!(snap.required.len(), 1);
        assert_eq!(snap.required_money, 500);
        assert!(!snap.completable, "no player");
        assert!(!state(Some(&held(0))).completable, "none held");
        assert!(state(Some(&held(1))).completable);
        giver.open(0x42, detail(100, "", Vec::new()));
        assert!(cells(&giver, &Items::default()).required.is_empty());
    }

    fn greeting(rows: &[(u32, u32, u32, &str)]) -> QuestView {
        QuestView::Greeting(QuestGiverList {
            npc: 0x42,
            greeting: "Hi".into(),
            emote_delay: 0,
            emote: 0,
            quests: rows
                .iter()
                .map(
                    |&(quest_id, icon, level, title)| benilla_protocol::messages::QuestListEntry {
                        quest_id,
                        icon,
                        level,
                        title: title.into(),
                    },
                )
                .collect(),
        })
    }

    /// The greeting keeps each quest's level beside its title, split into the two pools by the
    /// icon (`0x5dbb10` stores the wire level at row `+0x04`, the title at `+0x08`), and the pools
    /// outlive the panel and the window until the next greeting (`0x500c41`).
    #[test]
    fn the_greeting_pools_keep_each_quests_title_and_level_past_the_panel() {
        let mut giver = QuestGiver::default();
        giver.open(
            0x42,
            greeting(&[
                (1, 4, 12, "Active A"),
                (2, 2, 3, "Available A"),
                (3, 3, 9, "Active B"),
                (4, 2, 5, "Available B"),
            ]),
        );
        giver.open(0x42, detail(1, "", Vec::new()));
        giver.clear();
        let snap = cells(&giver, &Items::default());
        assert_eq!(snap.active_titles, ["Active A", "Active B"]);
        assert_eq!(snap.active_levels, [12, 9]);
        assert_eq!(snap.available_titles, ["Available A", "Available B"]);
        assert_eq!(snap.available_levels, [3, 5]);
        world_enter_cells(&mut giver);
        assert!(cells(&giver, &Items::default()).active_titles.is_empty());
    }

    fn world_enter_cells(giver: &mut QuestGiver) {
        giver.world_enter();
    }

    #[test]
    fn status_store_survives_close() {
        let mut giver = QuestGiver::default();
        giver.set_status(0x99, dialog_status::AVAILABLE);
        giver.open(
            0x99,
            QuestView::Greeting(QuestGiverList {
                npc: 0x99,
                greeting: "Hi".into(),
                emote_delay: 0,
                emote: 0,
                quests: vec![],
            }),
        );
        giver.clear();
        assert!(!giver.is_open());
        assert_eq!(giver.status(0x99), Some(dialog_status::AVAILABLE));
    }

    /// Reasons 2 to 5 sit inside `0x5dbca0`'s jump table and still take its default.
    #[test]
    fn questgiver_invalid_keys_match_the_reference_table() {
        assert_eq!(questgiver_invalid_key(13), "ERR_QUEST_ALREADY_ON");
        assert_eq!(questgiver_invalid_key(1), "ERR_QUEST_FAILED_LOW_LEVEL");
        assert_eq!(questgiver_invalid_key(6), "ERR_QUEST_FAILED_WRONG_RACE");
        assert_eq!(questgiver_invalid_key(12), "ERR_QUEST_ONLY_ONE_TIMED");
        assert_eq!(questgiver_invalid_key(20), "ERR_QUEST_FAILED_MISSING_ITEMS");
        assert_eq!(
            questgiver_invalid_key(22),
            "ERR_QUEST_FAILED_NOT_ENOUGH_MONEY"
        );
        for reason in [0, 2, 3, 4, 5, 7, 11, 14, 19, 21, 23, 99, u32::MAX] {
            assert_eq!(
                questgiver_invalid_key(reason),
                "ERR_QUEST_NEED_PREREQS",
                "reason {reason} must take the ref's `ja` default"
            );
        }
    }

    #[test]
    fn questgiver_failed_keys_match_the_reference_table() {
        assert_eq!(questgiver_failed_key(4), "ERR_QUEST_FAILED_BAG_FULL_S");
        assert_eq!(questgiver_failed_key(50), "ERR_QUEST_FAILED_BAG_FULL_S");
        assert_eq!(questgiver_failed_key(17), "ERR_QUEST_FAILED_MAX_COUNT_S");
        for reason in [0, 1, 5, 16, 18, 49, 51] {
            assert_eq!(questgiver_failed_key(reason), "ERR_QUEST_FAILED_S");
        }
    }

    /// Reads the install's `GlobalStrings.lua`; skips without it.
    #[test]
    fn every_quest_refusal_key_resolves_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).ok();

        for reason in 0..=60u32 {
            for key in [
                questgiver_invalid_key(reason),
                questgiver_failed_key(reason),
            ] {
                let text = g(key).unwrap_or_default();
                assert!(!text.is_empty(), "{key} (reason {reason}) missing");
            }
            // The FAILED family names the quest; the INVALID family never does.
            assert!(g(questgiver_failed_key(reason))
                .unwrap_or_default()
                .contains("%s"));
            assert!(!g(questgiver_invalid_key(reason))
                .unwrap_or_default()
                .contains("%s"));
        }
        assert_eq!(
            ui_error_text(&UiError::key(questgiver_invalid_key(13)), &g).as_deref(),
            Some("You are already on that quest")
        );
        assert_eq!(
            ui_error_text(&UiError::s(questgiver_failed_key(4), "A Threat Within"), &g).as_deref(),
            Some("A Threat Within failed: Inventory is full.")
        );
        // The log-full line takes no fill.
        assert_eq!(
            ui_error_text(&UiError::key("ERR_QUEST_LOG_FULL"), &g).as_deref(),
            Some("Your quest log is full.")
        );
    }

    /// The questgiver lane's items, taking them.
    fn lane(app: &mut App) -> Vec<QuestGiverEvent> {
        app.world_mut().resource_mut::<QuestGiverLane>().take()
    }

    /// Each panel setter fires its event at its end, every time (`0x500ee5`, `0x50105d`,
    /// `0x501120`), at its packet's place: a second offer queues `QUEST_DETAIL` again, an
    /// identical one too. `QUEST_ITEM_UPDATE` is queued only by the callback `GetQuestItemInfo`'s
    /// lookup keeps (`0x5017ce`), once per call, at the record's answer (`0x5018fe`-`0x501903`); a
    /// record no call asked for queues nothing.
    #[test]
    fn panel_events_queue_per_publish_and_item_updates_per_callback() {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        app.insert_non_send_resource(UiScript::new().unwrap());
        hold_for_test(app.world_mut(), 0x42);
        let detail = |app: &mut App, choice| {
            open_panel(app.world_mut(), 0x42, detail(100, "", vec![triple(choice)]))
        };
        detail(&mut app, 901);
        detail(&mut app, 901);
        assert_eq!(
            lane(&mut app),
            [
                QuestGiverEvent::Panel(QuestPanel::Detail, 0x42),
                QuestGiverEvent::Panel(QuestPanel::Detail, 0x42)
            ]
        );
        let land = |app: &mut App, entry| {
            let info = Some(Box::new(crate::items::test_template("Sword")));
            crate::net::handlers::dispatch(
                app.world_mut(),
                vec![benilla_protocol::SessionEvent::ItemTemplate { entry, info }],
            );
        };
        land(&mut app, 901);
        assert!(lane(&mut app).is_empty(), "no call asked for 901");
        detail(&mut app, 903);
        lane(&mut app);
        crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
            vm.expect("a VM")
                .run("GetQuestItemInfo('choice', 1) GetQuestItemInfo('choice', 1)")
        })
        .unwrap();
        assert!(lane(&mut app).is_empty(), "the record is still out");
        land(&mut app, 903);
        assert_eq!(
            lane(&mut app),
            [QuestGiverEvent::ItemUpdate, QuestGiverEvent::ItemUpdate],
            "one per call"
        );
        land(&mut app, 903);
        assert!(lane(&mut app).is_empty(), "the callbacks are spent");
    }

    /// The setter resolves its giver first (`0x500bf1`-`0x500c35`) and ends when none is held as
    /// a unit, a game object or an item (`0x500d96`): no panel, no cell store, no cue.
    #[test]
    fn a_panel_for_a_giver_not_held_opens_nothing() {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.init_resource::<crate::sound::MessageSounds>();
        hold_for_test(app.world_mut(), 0x42);
        // A corpse (type mask 0x80) is held, but is no giver.
        crate::items::test_spawn_object(
            app.world_mut(),
            0x44,
            benilla_protocol::ObjectFields::from_pairs(&[(2, 0x81)]),
        );
        open_panel(
            app.world_mut(),
            0x42,
            progress("Bring it.", Vec::new(), true),
        );
        lane(&mut app);
        for gone in [0x43, 0x44] {
            // Quest 100 is the progress panel's: held, this reward would play the chime.
            open_panel(app.world_mut(), gone, reward(100));
            open_panel(app.world_mut(), gone, reward(101));
            let giver = app.world().resource::<QuestGiver>();
            assert_eq!(giver.npc, Some(0x42), "{gone:#x}: the open panel stays");
            assert_eq!(giver.cells.quest, 100, "{gone:#x}: no cell store");
            assert!(lane(&mut app).is_empty(), "{gone:#x}: no event");
            assert!(
                app.world()
                    .resource::<crate::sound::MessageSounds>()
                    .take_cues()
                    .is_empty(),
                "{gone:#x}: no cue"
            );
        }
    }

    fn reward(quest_id: u32) -> QuestView {
        QuestView::Reward(QuestOfferReward {
            npc: 0x42,
            quest_id,
            title: "A Threat Within".into(),
            offer_text: "Well done.".into(),
            auto_finish: 1,
            choices: Vec::new(),
            rewards: Vec::new(),
            money: 0,
            quest_flags: 0,
            reward_spell: 0,
        })
    }

    /// The reward panel's setter plays `igQuestListComplete` when its quest id equals the last
    /// panel's (`0x500d36`-`0x500d43`): the progress panel's Continue, not a reward panel on its
    /// own, a different quest's, or one after the world-enter reset (`0x500b01`).
    #[test]
    fn a_reward_after_its_quests_progress_panel_plays_the_completion_chime() {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.init_resource::<crate::sound::MessageSounds>();
        hold_for_test(app.world_mut(), 0x42);
        let cues = |app: &mut App| {
            app.world()
                .resource::<crate::sound::MessageSounds>()
                .take_cues()
        };
        let chime = vec![QUEST_LIST_COMPLETE_SOUND];
        open_panel(app.world_mut(), 0x42, reward(100));
        assert!(cues(&mut app).is_empty(), "no panel before it");
        open_panel(
            app.world_mut(),
            0x42,
            progress("Bring it.", Vec::new(), true),
        );
        open_panel(app.world_mut(), 0x42, reward(100));
        assert_eq!(cues(&mut app), chime, "the reward after its progress panel");
        open_panel(app.world_mut(), 0x42, reward(100));
        assert_eq!(
            cues(&mut app),
            chime,
            "any panel of the same quest before it"
        );
        open_panel(
            app.world_mut(),
            0x42,
            progress("Bring it.", Vec::new(), true),
        );
        open_panel(app.world_mut(), 0x42, reward(101));
        assert!(cues(&mut app).is_empty(), "another quest's reward");
        open_panel(
            app.world_mut(),
            0x42,
            progress("Bring it.", Vec::new(), true),
        );
        app.world_mut().resource_mut::<QuestGiver>().world_enter();
        open_panel(app.world_mut(), 0x42, reward(100));
        assert!(cues(&mut app).is_empty(), "the world entry zeroes the id");
    }
}
