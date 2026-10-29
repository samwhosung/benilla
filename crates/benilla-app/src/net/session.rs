//! The bridge's session handlers: the connection edges, our own teleport and worldport, the
//! server clocks, reputations and the player's login-scoped stores. Registered ahead of every
//! window plugin, so the teardown runs before the windows' own `Disconnected` listeners.

use benilla_protocol::messages::Character;
use benilla_protocol::JumpInfo;
use bevy::prelude::*;

use crate::items::Items;
use crate::names::NameCache;
use crate::ui_quest::QuestGiver;

use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::ReputationListOp;
use bevy::ecs::system::SystemParam;

use super::{clock_tick, ServerTime};
use super::{
    AddonInfoMessage, CharActionResultMessage, CharListMessage, CharacterLoginFailedMessage,
    CinematicTriggeredMessage, ClientControlMessage, DisconnectedMessage, DroppedOpcodes,
    EnteredWorldMessage, GuidIndex, HomeBind, KnockBackMessage, LoggedOutMessage,
    LoginFailedMessage, LoginQueuedMessage, LoginStageMessage, NetCommands, NetHandlerApp,
    NetStatus, ObjectStore, PendingTransfer, Proficiencies, RealmListMessage, Reputations,
    RosterFailedMessage, SelfGuid, ServerWallClock, TeleportMessage, WorldAuthMessage,
    WorldportMessage,
};
use benilla_world::frame_pace::FrameStep;

/// Registers the session handlers.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    // The posted world load's legs run at the frame's start in the executor's order, after a
    // `/reload` the last frame asked for, which the reference ran inside its call.
    app.init_resource::<PendingWorldports>()
        .configure_sets(
            PreUpdate,
            (
                WorldportTimer::Connection.after(crate::ui_script::run_pending_reload),
                WorldportTimer::Sweep,
                WorldportTimer::Objects,
                WorldportTimer::Cooldowns,
                WorldportTimer::Manager,
                WorldportTimer::Rolls,
            )
                .chain(),
        )
        .add_systems(
            PreUpdate,
            world_load_timer
                .in_set(WorldportTimer::Objects)
                .run_if(|pending: Res<PendingWorldports>| pending.is_posted()),
        );
    app.net_handler(K::LoginStage, on_login_edge)
        .net_handler(K::WorldAuthenticated, on_login_edge)
        .net_handler(K::RosterFailed, on_login_edge)
        .net_handler(K::AddonInfo, on_login_edge)
        .net_handler(K::LoginQueued, on_login_edge)
        .net_handler(K::LoginFailed, on_login_edge)
        .net_handler(K::RealmList, on_login_edge)
        .net_handler(K::CharacterList, on_character_list)
        .net_handler(K::CharActionResult, on_login_edge)
        .net_handler(K::CharRenamed, on_login_edge)
        .net_handler(K::CharacterLoginFailed, on_login_edge)
        .net_handler(K::CinematicTriggered, on_login_edge)
        .net_handler(K::Connected, on_connected)
        .net_handler(K::LoggedOut, on_logged_out)
        .net_handler(K::Disconnected, on_disconnected)
        .net_handler(K::Teleport, on_mover_edge)
        .net_handler(K::KnockBack, on_mover_edge)
        .net_handler(K::ClientControl, on_client_control)
        .net_handler(K::Worldport, on_worldport)
        .net_handler(K::TransferPending, on_transfer_pending)
        .net_handler(K::TransferAborted, on_transfer)
        .net_handler(K::TimeSpeed, on_clock)
        .net_handler(K::GameSpeedSet, on_clock)
        .net_handler(K::GameTimeUpdate, on_clock)
        .net_handler(K::GameTimeSet, on_clock)
        .net_handler(K::ServerUnixTime, on_clock)
        .net_handler(K::Reputations, on_reputations)
        .net_handler(K::ReputationDelta, on_reputation_delta)
        .net_handler(K::ForcedReactions, on_reputations)
        .net_handler(K::ReputationVisible, on_reputations)
        .net_handler(K::ReputationAtWar, on_reputations)
        .net_handler(K::BindPoint, on_player_store)
        .net_handler(K::Proficiency, on_player_store)
        .net_handler(K::PacketDropped, on_packet_dropped)
        .net_handler(K::Pong, on_pong);
}

/// The edges the bridge publishes as messages, and the three resources they park in.
#[derive(SystemParam)]
pub(crate) struct Edges<'w> {
    teleports: MessageWriter<'w, TeleportMessage>,
    char_lists: MessageWriter<'w, CharListMessage>,
    realm_lists: MessageWriter<'w, RealmListMessage>,
    char_actions: MessageWriter<'w, CharActionResultMessage>,
    char_renames: MessageWriter<'w, super::CharRenameMessage>,
    char_login_failures: MessageWriter<'w, CharacterLoginFailedMessage>,
    entered_world: MessageWriter<'w, EnteredWorldMessage>,
    addon_reply: ResMut<'w, super::AddonInfoReply>,
    billing: ResMut<'w, super::SessionBilling>,
    logged_out: MessageWriter<'w, LoggedOutMessage>,
    knockbacks: MessageWriter<'w, KnockBackMessage>,
    login_stages: MessageWriter<'w, LoginStageMessage>,
    login_queued: MessageWriter<'w, LoginQueuedMessage>,
    world_auth: MessageWriter<'w, WorldAuthMessage>,
    roster_failed: MessageWriter<'w, RosterFailedMessage>,
    addon_info: MessageWriter<'w, AddonInfoMessage>,
    login_failures: MessageWriter<'w, LoginFailedMessage>,
    disconnects: MessageWriter<'w, DisconnectedMessage>,
    client_control: MessageWriter<'w, ClientControlMessage>,
    cinematics: MessageWriter<'w, CinematicTriggeredMessage>,
    transfer: ResMut<'w, PendingTransfer>,
}

/// The bridge state the session edges write.
#[derive(SystemParam)]
pub(crate) struct Bridge<'w, 's> {
    commands: Commands<'w, 's>,
    index: ResMut<'w, GuidIndex>,
    self_guid: ResMut<'w, SelfGuid>,
    status: ResMut<'w, NetStatus>,
    names: ResMut<'w, NameCache>,
    items: ResMut<'w, Items>,
    cooldowns: ResMut<'w, crate::spell::Cooldowns>,
    reputations: ResMut<'w, Reputations>,
    server_time: ResMut<'w, ServerTime>,
    wall_clock: ResMut<'w, ServerWallClock>,
    home_bind: ResMut<'w, HomeBind>,
    proficiencies: ResMut<'w, Proficiencies>,
    dropped: ResMut<'w, DroppedOpcodes>,
    net: Res<'w, NetCommands>,
    stores: Query<'w, 's, &'static mut ObjectStore>,
    transports: Query<'w, 's, &'static crate::transport::Transport>,
    /// The active mover, which the world entry zeroes and the teardowns free with its unit.
    mover: Option<ResMut<'w, super::MoverClaim>>,
    /// Our player's far-sight latch, which the teardowns free with it.
    far_sight: Option<Res<'w, crate::player::FarSight>>,
}

impl Bridge<'_, '_> {
    /// Every object freed (`0x467700` → `0x467800`), our own player too: the active mover it
    /// names is zeroed unless its unit is still held (`0x5fb650`), and its far-sight latch goes
    /// with it ([`crate::player::FarSight::free`]).
    fn objects_freed(&mut self) {
        if let Some(mover) = self.mover.as_deref_mut() {
            mover.forget_unless_held(&self.index);
        }
        if let Some(far_sight) = self.far_sight.as_deref() {
            far_sight.free();
        }
    }
}

fn on_login_edge(In(ev): In<SessionEvent>, mut e: Edges) {
    match ev {
        SessionEvent::LoginStage { stage } => login_stage(stage, &mut e.login_stages),
        SessionEvent::WorldAuthenticated {
            realm,
            epoch,
            generation,
        } => {
            e.world_auth.write(WorldAuthMessage {
                realm,
                epoch,
                generation,
            });
        }
        SessionEvent::RosterFailed { epoch } => {
            e.roster_failed.write(RosterFailedMessage { epoch });
        }
        SessionEvent::AddonInfo { records } => {
            // Held for the screens built after it, and for the world entry's load.
            e.addon_reply.0 = Some(records.clone());
            e.addon_info.write(AddonInfoMessage { records });
        }
        SessionEvent::LoginQueued { position, realm } => {
            e.login_queued.write(LoginQueuedMessage { position, realm });
        }
        SessionEvent::LoginFailed {
            refusal, reason, ..
        } => login_failed(refusal, reason, &mut e.login_failures),
        SessionEvent::RealmList { realms, failed } => {
            e.realm_lists.write(RealmListMessage { realms, failed });
        }
        SessionEvent::CharActionResult { action, code } => {
            char_action_result(action, code, &mut e.char_actions)
        }
        SessionEvent::CharRenamed { result, renamed } => {
            e.char_renames
                .write(super::CharRenameMessage { result, renamed });
        }
        SessionEvent::CharacterLoginFailed { result } => {
            character_login_failed(result, &mut e.char_login_failures)
        }
        SessionEvent::CinematicTriggered { cinematic_id } => {
            cinematic_triggered(cinematic_id, &mut e.cinematics)
        }
        _ => {}
    }
}

fn on_character_list(In(ev): In<SessionEvent>, mut e: Edges, mut b: Bridge) {
    if let SessionEvent::CharacterList {
        characters,
        realm,
        cause,
        epoch,
        ..
    } = ev
    {
        character_list(
            characters,
            realm,
            cause,
            epoch,
            &mut b.status,
            &mut e.char_lists,
        );
    }
}

fn on_connected(In(ev): In<SessionEvent>, mut e: Edges, mut b: Bridge) {
    if let SessionEvent::Connected {
        self_guid: guid,
        name,
        billing,
        tutorial_flags,
        addon_info,
    } = ev
    {
        e.billing.0 = billing;
        // `CGUnit_C::Initialize` zeroes the active mover (`0x6039a9`) at every world entry
        // (`ClientInitializeGame`, `0x401639`), so the first create of a session claims it.
        if let Some(mover) = b.mover.as_deref_mut() {
            mover.reset();
        }
        connected(
            guid,
            name,
            tutorial_flags,
            addon_info,
            &mut e.addon_reply,
            &mut b.self_guid,
            &mut b.status,
            &mut b.names,
            &mut e.entered_world,
        );
    }
}

fn on_logged_out(
    In(ev): In<SessionEvent>,
    mut e: Edges,
    mut b: Bridge,
    view_log: Option<ResMut<crate::ui_unit::views::ViewLog>>,
) {
    if let SessionEvent::LoggedOut = ev {
        log_teardown(view_log, &b);
        logged_out(
            &mut b.commands,
            &mut b.index,
            &mut b.self_guid,
            &mut e.logged_out,
        );
        // The teardown destroys every object (`0x402011` → `0x467700`), the mover's unit with it
        // (`0x5fb650`).
        b.objects_freed();
        // The reference zeroes the forced table's count and the 64 slots only in teardown
        // `0x4d5360` (`0x4d5419`, `0x4d541f`), whose one caller is `ShutdownGame 0x491180`
        // (`0x4911dc`), never on a worldport. The session's end stands in for that shutdown: the
        // table and the standings are the character's, and ones left here would colour the next
        // character's world. The server resends the table at login for a saved aura (vmangos
        // `Unit.cpp:3511` → `SpellAuras.cpp:2744-2745`).
        b.reputations.tear_down();
    }
}

/// The selection a session's loss ends, and where its signals are owed.
#[derive(bevy::ecs::system::SystemParam)]
struct LostSelection<'w> {
    selection: Option<ResMut<'w, crate::target::Selection>>,
    owed: Option<ResMut<'w, crate::target::OwedSelects>>,
}

/// The bridge's half of the session end; each window's own listener runs after it.
fn on_disconnected(
    In(ev): In<SessionEvent>,
    mut e: Edges,
    mut b: Bridge,
    mut released: ResMut<super::objects::Released>,
    view_log: Option<ResMut<crate::ui_unit::views::ViewLog>>,
    lost: LostSelection,
) {
    if let SessionEvent::Disconnected { reason, end } = ev {
        log_teardown(view_log, &b);
        // Every object goes with no release, kept link or not; the reference has no reconnect,
        // so the selection ends as at the world's teardown, with nothing sent or signalled.
        //
        // Deviation: on benilla's unattended reconnect, which the reference lacks, the world's VM
        // survives the loss, so the clear signals its `PLAYER_TARGET_CHANGED` and usable refresh
        // as a release's teardown does (`0x493a69`, `0x493a73`), still sending nothing, and the
        // surviving target frame lets the lost unit go.
        let LostSelection { selection, owed } = lost;
        if let Some(mut selection) = selection.filter(|s| s.guid.is_some()) {
            let before = selection.guid;
            crate::target::clear_with_no_world(&mut selection);
            let reconnects = end == benilla_protocol::SessionEnd::Lost && !super::session_over(end);
            if let (true, Some(owed)) = (reconnects, owed) {
                owed.into_inner().push(crate::target::OwedSelect::Signals(
                    crate::script_calls::EngineSelect {
                        before,
                        // No teardown lowers the pet's latch: no `PET_BAR_UPDATE`.
                        latched: false,
                        mark: crate::lanes::mark(),
                        owed: crate::target::SelectDebt::default(),
                    },
                ));
            }
        }
        // The same teardown as `on_logged_out`'s.
        b.reputations.tear_down();
        // Leaving the world frees the released objects with the live ones (`0x467700`).
        released.clear();
        disconnected(
            reason,
            end,
            &mut b.commands,
            &mut b.index,
            &mut b.self_guid,
            &mut b.status,
            &mut b.names,
            &mut b.items,
            &mut b.cooldowns,
            &mut e.transfer,
            &mut e.disconnects,
        );
        b.objects_freed();
    }
}

/// The world left at once: the stores the teardown takes, logged for the views a listener reads
/// ([`crate::ui_unit::views::ViewLog::torn_down`]).
fn log_teardown(view_log: Option<ResMut<crate::ui_unit::views::ViewLog>>, b: &Bridge) {
    if let Some(mut log) = view_log {
        log.torn_down(&b.index, |e| b.stores.get(e).ok());
    }
}

/// Teleport and knockback, forwarded unjudged to the controller, which applies and answers them.
/// A near teleport runs the movement guard first inside the packet (`OnForcedTeleport 0x602fb0`
/// → `0x60e990(1, 0)` at `0x60300e`, [`MoveInput`]), before the move is queued (`0x603029`); the
/// echo's own stop (`0x60e3b1`) finds none left.
fn on_mover_edge(
    In(ev): In<SessionEvent>,
    mut e: Edges,
    self_guid: Res<SelfGuid>,
    mut moved: MoveInput,
) {
    match ev {
        SessionEvent::Teleport {
            guid,
            counter,
            position,
            orientation,
        } => {
            moved.stop(guid, self_guid.0);
            teleport(
                guid,
                counter,
                position,
                orientation,
                &self_guid,
                &mut e.teleports,
            );
        }
        // `QueueKnockBack 0x602670` runs the same guard first (`0x60267e`), ahead of the queue
        // (`0x617a30`).
        SessionEvent::KnockBack {
            guid,
            counter,
            launch,
        } => {
            moved.stop(guid, self_guid.0);
            knock_back(guid, counter, launch, &self_guid, &mut e.knockbacks);
        }
        _ => {}
    }
}

/// The movement guard `CGUnit_C::OnMoveInput(1, 0)` (`0x60e990`) where a forced move's handler runs
/// it on its unit: the active mover's auto-move stops (`0x60e99d`-`0x60e9b5`), the follow's
/// `AUTOFOLLOW_END` on the notice lane, then the active player's loot window closes as on a
/// movement start (`0x60e9c1`-`0x60e9dd`).
#[derive(bevy::ecs::system::SystemParam)]
struct MoveInput<'w, 's> {
    auto_move: crate::player::AutoMoveStop<'w>,
    lane: Option<ResMut<'w, crate::field_watch::NoticeLane>>,
    loot: crate::ui_loot::MoveStartClose<'w, 's>,
}

impl MoveInput<'_, '_> {
    fn stop(&mut self, guid: u64, me: Option<u64>) {
        self.auto_move.stop(guid, self.lane.as_deref_mut());
        if me == Some(guid) {
            self.loot.close();
        }
    }
}

/// What [`on_client_control`] holds beside the host: the cast its fall cancels, our guid, the flag,
/// the notice lane its signals go on (absent in a harness without the unit feed), and the
/// outgoing mover's auto-move.
type ControlSet = (
    crate::spell::SelfCancel<'static>,
    Res<'static, SelfGuid>,
    ResMut<'static, crate::player::OwnControlLost>,
    Option<ResMut<'static, crate::field_watch::NoticeLane>>,
    crate::player::AutoMoveStop<'static>,
);

/// `SMSG_CLIENT_CONTROL_UPDATE` (`0x5fa6b1` → `SetPlayerControl 0x4958e0`), its movement judged by
/// the controller later. For our own mover a change of the flag `[0xb4b3e4]` is stored here
/// (`0x4958ea`); its fall drops the cursor's payload and the cast (`0x4958e8`-`0x495901` →
/// `0x495920`), and the change signals on the notice lane at this packet's place, then runs the
/// usable refresh (`0x4958f7`/`0x49590b`, `0x4958fc`/`0x495910`). A store that changes nothing does
/// nothing (`0x4958e6`). Ahead of it the unit's control moves the active mover (`0x5fa600`): a
/// grant claims the unit (`0x5fa696`) and a stop of the mover zeroes it (`0x5fa678`-`0x5fa696`),
/// and the switch ends the outgoing mover's auto-move first (`SetActiveMover`, `0x600731`-
/// `0x600742`), its `AUTOFOLLOW_END` on the notice lane ahead of the flag's signal.
fn on_client_control(
    In(ev): In<SessionEvent>,
    mut e: Edges,
    // The flag and our guid are the host's too, so they sit in the set it shares with the cast.
    mut vm: crate::script_calls::VmWith<ControlSet>,
    // The unit whose `IsClientControlled` bit the update writes.
    mut units: (Option<Res<super::GuidIndex>>, Commands),
) {
    let SessionEvent::ClientControl { mover, allow_move } = ev else {
        return;
    };
    e.client_control
        .write(ClientControlMessage { mover, allow_move });
    // `0x5fa62a`/`0x5fa631`: the unit's own bit first (`0x5fa636`).
    if let Some(&unit) = units.0.as_deref().and_then(|i| i.0.get(&mover)) {
        let mut unit = units.1.entity(unit);
        if allow_move {
            unit.insert(super::ClientControlled);
        } else {
            unit.remove::<super::ClientControlled>();
        }
    }
    {
        let (_, self_guid, mut own, mut lane, mut stop) = vm.own();
        let switches = stop.mover().is_some_and(|m| (m == mover) != allow_move);
        if switches {
            stop.stop_mover(lane.as_deref_mut());
        }
        if self_guid.0 != Some(mover) || own.0 == !allow_move {
            return;
        }
        own.0 = !allow_move;
    }
    if !allow_move {
        crate::spell::drop_hold(&mut vm, |(mut cancel, ..)| cancel.abort_cast());
    }
    if let (_, _, _, Some(mut lane), _) = vm.own() {
        use crate::field_watch::FieldNotice;
        lane.write(FieldNotice::PlayerControl {
            in_control: allow_move,
        });
        lane.write(FieldNotice::UsableRefresh);
    }
}

/// A `SMSG_NEW_WORLD`'s destination, held for its timer.
#[derive(Clone, Copy, Debug)]
struct Destination {
    map_id: u32,
    position: [f32; 3],
    orientation: f32,
}

/// The world loads posted and not yet run: `SMSG_NEW_WORLD` (`0x401b00`) reads its destination
/// into globals and posts the executor `0x401bc0` as a timer at no delay (`0x401ba5`-`0x401bae`),
/// which the next tick runs ahead of its packets (`0x420d0c`, `0x420d55`). Beside them, the maps
/// whose objects' leg has run, each owed its new object manager (`0x401c22`).
#[derive(Resource, Default)]
pub(crate) struct PendingWorldports {
    posted: Vec<Destination>,
    /// How many loads the objects' leg has run, which the legs after it test ([`world_ported`]).
    ported: u32,
    /// The maps of the loads whose objects' leg has run, owed their new manager.
    loaded: Vec<u32>,
}

impl PendingWorldports {
    /// Whether a timer is posted, for the connection's leave to run only then.
    pub(crate) fn is_posted(&self) -> bool {
        !self.posted.is_empty()
    }

    /// Whether a load's objects are gone and its new manager not yet made, for the manager's leg
    /// ([`WorldportTimer::Manager`]) to run only then.
    pub(crate) fn has_loaded(&self) -> bool {
        !self.loaded.is_empty()
    }

    /// The maps of the loads whose objects are gone, in their order, taken by the manager's leg.
    pub(crate) fn take_loaded(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.loaded)
    }

    /// A load posted, as `SMSG_NEW_WORLD` posts it, for a harness with no objects' leg.
    #[cfg(test)]
    pub(crate) fn post_for_test(&mut self) {
        self.posted.push(Destination {
            map_id: 1,
            position: [0.0; 3],
            orientation: 0.0,
        });
    }

    /// The posted loads taken, as the objects' leg takes them.
    #[cfg(test)]
    pub(crate) fn take_for_test(&mut self) {
        self.posted.clear();
    }
}

/// The executor `0x401bc0`'s body, one unit at the next tick's start, in the order of its calls:
/// the connection leaves the world (`0x401bd4` → `0x5aae70(0)`, `LOGOUT_CANCEL` for a pending
/// request), then the movement and missiles go (`0x401bde`, `0x401bf2`), then the object manager
/// is destroyed (`0x401bf7`), whose walk (`0x467800`) frees the objects in creation order and
/// reaches the local player's destructor and its leave-world sweep (`0x5dd543`) at the player's
/// turn; then the cooldowns are freed (`0x401c17`), the new object manager made (`0x401c22`) and
/// the open loot rolls cancelled (`0x401c2c`), ahead of the map load. The sets order those legs in `PreUpdate`.
///
/// The sweep runs ahead of the objects' leg, so what was created after the player (the target, a
/// pet, an NPC) is alive when `PLAYER_LEAVING_WORLD` fires, as in the reference. What remains: the
/// objects created before the player (its items, say) are gone by then in the reference, and
/// benilla keeps no creation order, so they are alive there too.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum WorldportTimer {
    /// `0x401bd4`: `crate::ui_logout::leave_world`.
    Connection,
    /// `0x5dd543`: `crate::ui_script::sweep_on_worldport`, for a load posted.
    Sweep,
    /// `0x401bf7`'s walk of the objects: [`world_load_timer`].
    Objects,
    /// `0x401c17`: `ClearItemAndSpellCooldowns`, for a load the objects' leg ran.
    Cooldowns,
    /// `0x401c22`: the new object manager for the map loaded (`ClntObjMgrCreate` `0x464ff0`), whose
    /// map-change latch (`0x46500e`) is `crate::ui_instance`'s, for each map the objects' leg ran
    /// ([`PendingWorldports::take_loaded`]).
    Manager,
    /// `0x401c2c`: `CancelAllLootRolls`, for a load the objects' leg ran; the map load
    /// (`0x401c5c`) follows.
    Rolls,
}

/// What the world load needs from the bridge.
#[derive(SystemParam)]
pub(crate) struct WorldLoad<'w, 's> {
    transfer: ResMut<'w, PendingTransfer>,
    worldports: MessageWriter<'w, WorldportMessage>,
    b: Bridge<'w, 's>,
    group: ResMut<'w, crate::ui_party::GroupState>,
    script: Option<NonSend<'w, benilla_ui::script::UiScript>>,
    released: ResMut<'w, super::objects::Released>,
    view_log: Option<ResMut<'w, crate::ui_unit::views::ViewLog>>,
    player: Option<Res<'w, crate::player::Player>>,
    selection: Option<ResMut<'w, crate::target::Selection>>,
    place: crate::ui_party::net::SnapshotPlace<'w, 's>,
    transforms: Query<'w, 's, &'static Transform>,
}

impl WorldLoad<'_, '_> {
    /// The objects' destruction (`0x467700` → `0x467800`) and the destination handed to the app.
    fn objects_destroyed(&mut self, dest: Destination, needs_ack: bool) {
        let Destination {
            map_id,
            position,
            orientation,
        } = dest;
        let b = &mut self.b;
        // Every streamed roster member is about to be freed, and each one's destructor runs the
        // roster hook (`0x5dd6a0` → `0x4e85d0` mode 0).
        let before = self.view_log.as_ref().map(|_| self.group.stats.clone());
        crate::ui_party::net::roster_deactivated(
            &mut self.group,
            &b.index,
            &b.stores,
            &self.place,
            &|e| self.transforms.get(e).ok().copied(),
            &b.names,
            self.script.as_deref(),
            &b.net,
        );
        if let (Some(log), Some(before)) = (self.view_log.as_deref_mut(), before) {
            log.records(&before, &self.group.stats);
        }
        if let Some(log) = self.view_log.as_deref_mut() {
            log.torn_down(&b.index, |e| b.stores.get(e).ok());
        }
        // The transfer frees every object with no release (`0x467700` → `0x467800`): the selection
        // keeps its guid, its object gone, until the arrival's `SelectTarget(0)` (`0x490927`)
        // clears it (the unit feed's world-enter turn).
        if let Some(selection) = self.selection.as_deref_mut() {
            selection.target = None;
            selection.lit = None;
        }
        worldport(
            map_id,
            position,
            orientation,
            needs_ack,
            &mut b.commands,
            &mut b.index,
            &mut self.transfer,
            &b.transports,
            self.player.as_ref().and_then(|p| p.ride_entity()),
            &mut self.worldports,
        );
        // The world load destroys every object (`0x401bf7` → `0x467700` → `0x467800`), our own
        // player too, whose destructor zeroes the active mover it names (`0x5fb650`).
        b.objects_freed();
        // Loading a new world frees the released objects with the live ones (`0x467700`).
        self.released.clear();
    }
}

/// `SMSG_NEW_WORLD` (`0x401b00`) only posts the load: its body runs as a timer at the next tick's
/// start ([`WorldportTimer`], [`world_load_timer`]), so what the port's own tick raised still sees
/// the old world and an open logout request. `SMSG_LOGIN_VERIFY_WORLD` for another map
/// (`0x401de0` → `0x401ea4`) calls the executor in place, with no ack owed.
fn on_worldport(
    In(ev): In<SessionEvent>,
    mut pending: ResMut<PendingWorldports>,
    mut load: WorldLoad,
) {
    if let SessionEvent::Worldport {
        map_id,
        position,
        orientation,
        needs_ack,
    } = ev
    {
        let dest = Destination {
            map_id,
            position,
            orientation,
        };
        if needs_ack {
            pending.posted.push(dest);
        } else {
            load.objects_destroyed(dest, false);
        }
    }
}

/// The posted world loads' objects leg once, for a test that dispatches a port and reads what the
/// timer does.
#[cfg(test)]
pub(crate) fn fire_worldport_timer(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(world_load_timer)
        .expect("the world load runs");
    world.flush();
}

/// The posted world loads' objects leg, at the frame's start ([`WorldportTimer::Objects`]).
fn world_load_timer(mut pending: ResMut<PendingWorldports>, mut load: WorldLoad) {
    for dest in std::mem::take(&mut pending.posted) {
        pending.loaded.push(dest.map_id);
        load.objects_destroyed(dest, true);
        pending.ported = pending.ported.wrapping_add(1);
    }
}

/// Whether the objects' leg ran a load since this condition last looked, for the legs after it:
/// the leg takes the posted loads, so [`PendingWorldports::is_posted`] is spent by then.
pub(crate) fn world_ported(pending: Res<PendingWorldports>, mut seen: Local<u32>) -> bool {
    std::mem::replace(&mut *seen, pending.ported) != pending.ported
}

/// The handler `0x401900` reaches `0x495920` on both of its arms (`0x40194c`), ahead of the
/// loading screen: the cursor's payload and the cast are dropped.
fn on_transfer_pending(
    In(ev): In<SessionEvent>,
    mut e: Edges,
    mut vm: crate::script_calls::VmWith<crate::spell::SelfCancel<'static>>,
) {
    if let SessionEvent::TransferPending {
        map_id,
        transport_entry,
        from_map,
    } = ev
    {
        crate::spell::drop_hold(&mut vm, |mut cancel| cancel.abort_cast());
        transfer_pending(map_id, transport_entry, from_map, &mut e.transfer);
    }
}

fn on_transfer(In(ev): In<SessionEvent>, mut e: Edges) {
    if let SessionEvent::TransferAborted { reason } = ev {
        transfer_aborted(reason, &mut e.transfer);
    }
}

/// The game clock's four messages (`SMSG_LOGIN_SETTIMESPEED`, `SMSG_GAMESPEED_SET`,
/// `SMSG_GAMETIME_UPDATE`, `SMSG_GAMETIME_SET`) and the server's wall clock.
fn on_clock(
    In(ev): In<SessionEvent>,
    mut b: Bridge,
    step: Res<FrameStep>,
    quest_log: Option<ResMut<crate::ui_quest_log::QuestLog>>,
) {
    match ev {
        SessionEvent::TimeSpeed { .. }
        | SessionEvent::GameSpeedSet { .. }
        | SessionEvent::GameTimeUpdate { .. }
        | SessionEvent::GameTimeSet { .. } => {
            let lines = b.server_time.0.on_message(&ev, clock_tick(&step));
            for line in lines.into_iter().flatten() {
                info!("console: {line}");
            }
        }
        SessionEvent::ServerUnixTime { unix_time } => {
            server_unix_time(unix_time, &mut b.wall_clock);
            // The setter stores the offset and signals the quest log (`0x4deef0`, `0x4def1b`).
            if let Some(mut quest_log) = quest_log {
                quest_log.note_clock_synced();
            }
        }
        _ => {}
    }
}

/// `UPDATE_FACTION` (`0x703e50` with 0x174) as a reputation message signals it, on the notice lane
/// in the packets' order.
const UPDATE_FACTION: crate::field_watch::FieldNotice = crate::field_watch::FieldNotice::Global {
    id: crate::field_watch::id::UPDATE_FACTION,
};

/// `CGUnit_C::RefreshReaction` (`0x606e20`) on the active player from a reputation message, once,
/// when the object manager holds the player (`0x4d5a55`, `0x4d58ff`): `UNIT_FACTION` for it, its
/// summon and its charm, then for every held unit the sweep reaches, on the notice lane.
fn refresh_player_reaction(b: &Bridge, lane: &mut crate::field_watch::NoticeLane) {
    let Some(me) = b.self_guid.0 else {
        return;
    };
    let Some(store) = b.index.0.get(&me).and_then(|&e| b.stores.get(e).ok()) else {
        return;
    };
    let others = || {
        let units = b.index.0.iter().filter_map(|(&g, &e)| {
            let s = b.stores.get(e).ok()?;
            s.is_unit().then_some((g, &s.0))
        });
        crate::field_watch::reaction_sweep(me, units)
    };
    lane.write_batch(crate::field_watch::reaction_signals(
        me,
        &store.0,
        Some(me),
        &others,
    ));
}

fn on_reputations(
    In(ev): In<SessionEvent>,
    mut b: Bridge,
    factions: Option<Res<crate::target::ring::Factions>>,
    chat: Res<crate::ui_chat::ChatLog>,
    // Absent in a harness with no interface.
    mut lane: Option<ResMut<crate::field_watch::NoticeLane>>,
) {
    let signal = |n: usize, lane: &mut Option<ResMut<crate::field_watch::NoticeLane>>| {
        if let Some(lane) = lane.as_deref_mut() {
            lane.write_batch(std::iter::repeat_n(UPDATE_FACTION, n));
        }
    };
    match ev {
        SessionEvent::Reputations { standings } => {
            reputations(standings, &mut b.reputations);
            // With the player held, the rebuild (`0x4d5694`), whose two sorts signal
            // (`0x4d55d9` → `0x4d6457`, and `0x4d55de`), then the message's own (`0x4d5699`).
            let held = self_facts(&b).is_some();
            if held {
                b.reputations.list_op(ReputationListOp::Rebuild);
            }
            signal(1 + 2 * usize::from(held), &mut lane);
        }
        SessionEvent::ForcedReactions { reactions } => {
            forced_reactions(reactions, &mut b.reputations);
            if let Some(lane) = lane.as_deref_mut() {
                refresh_player_reaction(&b, lane);
            }
        }
        SessionEvent::ReputationVisible { list_id } => {
            // The adder lists the slot and prints its rank, the flag byte not yet moved
            // (`0x4d5733`-`0x4d5738`); then the flag is stored (`0x4d5745`) and the list sorts
            // (`0x4d574a` → `0x4d5dab`), which signals.
            let facts = self_facts(&b);
            let listed = reputation_visible(list_id, &mut b.reputations, |store| {
                store.list_op(ReputationListOp::Edit {
                    listed: vec![list_id],
                    sort: false,
                });
                // The chat line names the rank, so it reads the store before the flag lands.
                if let (Some(f), Some(me)) = (factions.as_deref(), facts.as_ref()) {
                    let line = crate::ui_reputation::visible_line(
                        list_id,
                        store,
                        f.catalog(),
                        me.race,
                        me.class,
                    );
                    crate::ui_reputation::queue_standing_lines(line, f.catalog(), me.female, &chat);
                }
            });
            if listed {
                b.reputations.list_op(ReputationListOp::Edit {
                    listed: vec![],
                    sort: true,
                });
            }
            signal(usize::from(listed), &mut lane);
        }
        SessionEvent::ReputationAtWar { list_id, flags } => {
            reputation_at_war(list_id, flags, &mut b.reputations);
            // The bit moved a row's war state and no list: the lane takes the snapshot there.
            b.reputations.list_op(ReputationListOp::Edit {
                listed: vec![],
                sort: false,
            });
            // Whether or not the bit moved (`0x4d56f6`).
            signal(1, &mut lane);
        }
        _ => {}
    }
}

/// What the reputation chat lines read of the local player.
struct SelfFacts {
    race: u8,
    class: u8,
    female: bool,
}

/// The local player's race, class and gender, `None` before the self object streams in; the
/// reference's `0x62c670` prints nothing without it (`0x62c6bf`). The gender is the real one
/// (the Deviation on `unit_gender`).
fn self_facts(b: &Bridge) -> Option<SelfFacts> {
    let entity = *b.index.0.get(&b.self_guid.0?)?;
    let store = b.stores.get(entity).ok()?;
    Some(SelfFacts {
        race: store.0.unit_race()?,
        class: store.0.unit_class()?,
        female: store.0.unit_gender() == Some(1),
    })
}

/// `SMSG_SET_FACTION_STANDING` (`0x4d5760`) entry by entry, each printing where the reference's
/// does: the first entry's "increased by" line with its standing stored and its flags not yet
/// moved (`0x4d6330`, `0x4d57f5`), a listed slot's rank line in the adder (`0x4d5826`), a moved
/// rank's after the flag write (`0x4d5896`). A line reads the entry against the store as it stood,
/// so each is decided ahead of the write it follows and parked in the chat log after it.
fn on_reputation_delta(
    In(ev): In<SessionEvent>,
    mut b: Bridge,
    factions: Option<Res<crate::target::ring::Factions>>,
    chat_log: Res<crate::ui_chat::ChatLog>,
    mut quest: ResMut<QuestGiver>,
    quest_log: Option<ResMut<crate::ui_quest_log::QuestLog>>,
    // Absent in a harness with no interface.
    mut lane: Option<ResMut<crate::field_watch::NoticeLane>>,
    tutorials: Option<Res<crate::tutorial::Tutorials>>,
) {
    if let SessionEvent::ReputationDelta { standings } = ev {
        let facts = self_facts(&b);
        let ranks = factions
            .as_deref()
            .zip(facts.as_ref())
            .map(|(f, me)| (f.catalog(), me.race, me.class));
        let mut said = |said: Said| match said {
            Said::Change {
                list_id,
                flags,
                old,
                standing,
            } => crate::combat_log::chat::faction_standing_change(
                list_id,
                flags,
                old,
                standing,
                factions.as_deref(),
                &chat_log,
            ),
            Said::Line(line) => {
                if let (Some(f), Some(me)) = (factions.as_deref(), facts.as_ref()) {
                    crate::ui_reputation::queue_standing_lines(
                        [line],
                        f.catalog(),
                        me.female,
                        &chat_log,
                    );
                }
            }
        };
        let latches = reputation_delta(standings, &mut b.reputations, ranks, &mut quest, &mut said);
        // A flag byte that moved re-sorts, which signals (`0x4d58c8` → `0x4d5dab`).
        if latches.flags_moved {
            if let Some(lane) = lane.as_deref_mut() {
                lane.write(UPDATE_FACTION);
            }
        }
        // A moved rank rebuilds the quest log and refreshes the active player's reaction once
        // for the message (`0x4d58ea`-`0x4d5905`); `ranks` holds only with the player.
        if latches.rank_moved {
            if let Some(mut quest_log) = quest_log {
                quest_log.note_faction_rank_move();
            }
            if let Some(lane) = lane.as_deref_mut() {
                refresh_player_reaction(&b, lane);
            }
        }
        // Reputation, with the active player held at level 10 or more (`0x4d5963`-`0x4d597a`).
        let level = b
            .self_guid
            .0
            .and_then(|g| b.index.0.get(&g))
            .and_then(|e| b.stores.get(*e).ok())
            .and_then(|s| s.0.unit_level());
        if level.is_some_and(|l| l as i32 >= 10) {
            if let Some(tutorials) = tutorials.as_deref() {
                tutorials.trigger_now(crate::tutorial::id::REPUTATION);
            }
        }
        // The message's own signal, whatever it held, none included (`0x4d597f`-`0x4d5984`).
        if let Some(lane) = lane.as_deref_mut() {
            lane.write(UPDATE_FACTION);
        }
    }
}

/// The player's login-scoped stores the bridge defines.
fn on_player_store(In(ev): In<SessionEvent>, mut b: Bridge) {
    match ev {
        SessionEvent::BindPoint { area } => b.home_bind.0 = Some(area),
        SessionEvent::Proficiency {
            item_class,
            subclass_mask,
        } => {
            b.proficiencies.0.insert(item_class, subclass_mask);
        }
        _ => {}
    }
}

fn on_packet_dropped(In(ev): In<SessionEvent>, mut b: Bridge) {
    if let SessionEvent::PacketDropped {
        opcode,
        unparseable,
    } = ev
    {
        packet_dropped(opcode, unparseable, &mut b.dropped);
    }
}

/// The read thread consumes the pong inline, as the reference's `OnData` (`0x537b10`) hands
/// `SMSG_PONG` to `HandlePong` (`0x537d60`); reaching here means that bypass is gone and every
/// latency reading is a frame late, so it warns.
fn on_pong(In(ev): In<SessionEvent>) {
    if let SessionEvent::Pong { sequence } = ev {
        warn!("net: pong seq={sequence} reached the drain — the read thread's RTT bypass is gone");
    }
}

/// The pre-logon handshake reached a new stage.
fn login_stage(stage: benilla_protocol::LoginStage, out: &mut MessageWriter<LoginStageMessage>) {
    out.write(LoginStageMessage { stage });
}

/// A login failed before the roster; the IO thread is back at its pre-logon park and
/// [`crate::glue_session::login`] decides what follows.
fn login_failed(
    refusal: Option<benilla_protocol::LoginRefusal>,
    reason: String,
    out: &mut MessageWriter<LoginFailedMessage>,
) {
    out.write(LoginFailedMessage { refusal, reason });
}

/// `SMSG_CHAR_CREATE`/`SMSG_CHAR_DELETE`: the glue screen turns the code into its string.
fn char_action_result(
    action: benilla_protocol::CharAction,
    code: u8,
    out: &mut MessageWriter<CharActionResultMessage>,
) {
    out.write(CharActionResultMessage { action, code });
}

/// `SMSG_CHAR_ENUM`: the roster and the connected realm, for character select.
fn character_list(
    characters: Vec<Character>,
    realm: Option<benilla_protocol::RealmInfo>,
    cause: benilla_protocol::RosterCause,
    epoch: u64,
    status: &mut NetStatus,
    char_lists: &mut MessageWriter<CharListMessage>,
) {
    info!(
        "net: character select — {} character(s) on the account",
        characters.len()
    );
    // A roster means a live link: clear the reason a lost link left, or the select screen keeps
    // its "Server down" note until the next world entry.
    status.last_reason = None;
    char_lists.write(CharListMessage {
        characters,
        realm,
        cause,
        epoch,
    });
}

/// `SMSG_CHARACTER_LOGIN_FAILED`: the announced entry is void; the glue manager says why.
fn character_login_failed(result: u8, out: &mut MessageWriter<CharacterLoginFailedMessage>) {
    warn!("net: character login refused (result {result:#04x})");
    out.write(CharacterLoginFailedMessage { result });
}

/// `SMSG_TRIGGER_CINEMATIC`, handed to [`crate::cinematic`], which sends the ack at the end of
/// playback. While a cinematic runs unacked, vmangos anchors visibility to the camera
/// (`Player::UpdateCinematic`), so every path there must still ack.
fn cinematic_triggered(
    cinematic_id: u32,
    triggered: &mut MessageWriter<CinematicTriggeredMessage>,
) {
    info!("net: cinematic {cinematic_id} triggered");
    triggered.write(CinematicTriggeredMessage { cinematic_id });
}

/// The first in-world event: records our guid and seeds the name cache with our own name.
fn connected(
    guid: u64,
    name: String,
    tutorial_flags: Option<Vec<u8>>,
    addon_info: Option<Vec<benilla_protocol::messages::AddonRecord>>,
    addon_reply: &mut crate::net::AddonInfoReply,
    self_guid: &mut SelfGuid,
    status: &mut NetStatus,
    names: &mut NameCache,
    entered_world: &mut MessageWriter<EnteredWorldMessage>,
) {
    self_guid.0 = Some(guid);
    status.connected = true;
    status.last_reason = None;
    info!("net: in world as {name} (guid {guid})");
    // The reference clears the caches built unsaved at every world entry (`DBCacheClearUnsaved`,
    // `0x555740`, ahead of the world's first packet): player and pet names here, since a guid may
    // name a new character, and the guild and petition stores in [`crate::wdb`]. A saved
    // cache, creature templates included, keeps its records and its asks.
    names.clear_world_session();
    // Our own name came with the login, so "player" never queries.
    names.insert_player(guid, name, None);
    // Set before the world-entry UI load reads it, and every login, so a silent server inherits
    // no earlier answer.
    addon_reply.0 = addon_info;
    entered_world.write(EnteredWorldMessage { tutorial_flags });
}

/// `SMSG_LOGOUT_COMPLETE`: back to character select.
fn logged_out(
    commands: &mut Commands,
    index: &mut GuidIndex,
    self_guid: &mut SelfGuid,
    logged_out: &mut MessageWriter<LoggedOutMessage>,
) {
    // A logout ends the character's session, so the avatar goes too, unlike a reconnectable
    // disconnect; clearing `SelfGuid` first makes the following teardown total.
    info!("net: logged out — back to character select");
    if let Some(guid) = self_guid.0.take() {
        if let Some(e) = index.0.remove(&guid) {
            commands.entity(e).despawn();
        }
    }
    logged_out.write(LoggedOutMessage);
}

/// The world session ended, the link kept or not: tears down the streamed world and clears every
/// session-scoped cache.
fn disconnected(
    reason: String,
    end: benilla_protocol::SessionEnd,
    commands: &mut Commands,
    index: &mut GuidIndex,
    self_guid: &mut SelfGuid,
    status: &mut NetStatus,
    names: &mut NameCache,
    items: &mut Items,
    cooldowns: &mut crate::spell::Cooldowns,
    pending_transfer: &mut PendingTransfer,
    disconnects: &mut MessageWriter<DisconnectedMessage>,
) {
    // Whether the session is over is settled once, here, and carried to every reader.
    let msg = DisconnectedMessage::new(reason.clone(), end);
    let over = msg.session_over;
    disconnects.write(msg);
    warn!("net: {reason} — tearing down the streamed world");
    // An unfinished far teleport dies with the socket.
    pending_transfer.0 = None;
    status.connected = false;
    // A logout or a refused login ends the character, the link kept: nothing reconnects it.
    let character_over = over || end == benilla_protocol::SessionEnd::LoggedOut;
    // The RTT ring is not cleared here: the read thread wipes it as it re-enters its cycle loop.
    // Every streamed entity despawns at once, not faded, so none races the reconnect's creates.
    // The self avatar stays as the local puppet for the reconnect to refresh, unless the
    // character is over, and then it goes as on logout.
    let keep = if character_over { None } else { self_guid.0 };
    if character_over {
        self_guid.0 = None;
    }
    index.0.retain(|guid, e| {
        if Some(*guid) == keep {
            return true;
        }
        commands.entity(*e).despawn();
        false
    });
    // A failure to show at character select; a kept link has none.
    if end != benilla_protocol::SessionEnd::LoggedOut {
        status.last_reason = Some(reason);
    }
    // In-flight name queries died with the socket.
    names.clear_pending();
    items.clear_session();
    // `SMSG_INITIAL_SPELLS` resends every running cooldown at world entry and `seed_initial`
    // appends, so the list must not outlive the session.
    cooldowns.clear_session();
}

/// `MSG_MOVE_TELEPORT_ACK`: only our own is forwarded to the controller, whose ack resumes our
/// movement.
fn teleport(
    guid: u64,
    counter: u32,
    position: [f32; 3],
    orientation: f32,
    self_guid: &SelfGuid,
    teleports: &mut MessageWriter<TeleportMessage>,
) {
    if self_guid.0 == Some(guid) {
        teleports.write(TeleportMessage {
            guid,
            counter,
            position,
            orientation,
        });
    }
}

/// `SMSG_MOVE_KNOCK_BACK`: a ballistic launch our controller flies and acks. The reference
/// handles it for the controller only (`0x603f90`); observers get `MSG_MOVE_KNOCK_BACK`
/// (`0x603bb0`), so one naming another unit is dropped.
fn knock_back(
    guid: u64,
    counter: u32,
    launch: JumpInfo,
    self_guid: &SelfGuid,
    knockbacks: &mut MessageWriter<KnockBackMessage>,
) {
    if self_guid.0 == Some(guid) {
        knockbacks.write(KnockBackMessage {
            guid,
            counter,
            launch,
        });
    }
}

/// `SMSG_TRANSFER_PENDING`: latched for the worldport; its transport block makes `NEW_WORLD`'s
/// coordinates boat-local.
fn transfer_pending(
    map_id: u32,
    transport_entry: Option<u32>,
    from_map: Option<u32>,
    pending: &mut PendingTransfer,
) {
    match transport_entry {
        Some(entry) => info!("net: transfer pending → map {map_id} riding transport {entry}"),
        None => info!("net: transfer pending → map {map_id}"),
    }
    pending.0 = Some(crate::net::PendingTransferInfo {
        map_id,
        transport_entry,
        from_map,
    });
}

/// `SMSG_TRANSFER_ABORTED`: clears the latch, which ends the screen the transfer raised as
/// `0x401a11` does (`0x407e80`); the reason's line is `ui_chat`'s.
fn transfer_aborted(reason: u8, pending: &mut PendingTransfer) {
    warn!("net: transfer aborted (reason {reason})");
    pending.0 = None;
}

/// `SMSG_NEW_WORLD`/`SMSG_LOGIN_VERIFY_WORLD`: drops every tracked object, as `0x401bc0` destroys
/// them, then hands the app the destination. The one exception is the ship a transport transfer
/// carries us on, when its timetable touches the new map: it sails through the map change, so our
/// deck-local pose has a deck, and the server's create refreshes it in place.
fn worldport(
    map_id: u32,
    position: [f32; 3],
    orientation: f32,
    needs_ack: bool,
    commands: &mut Commands,
    index: &mut GuidIndex,
    pending: &mut PendingTransfer,
    transports: &Query<&crate::transport::Transport>,
    ridden: Option<Entity>,
    worldports: &mut MessageWriter<WorldportMessage>,
) {
    let carried = pending
        .0
        .as_ref()
        .is_some_and(|p| p.transport_entry.is_some());
    index.0.retain(|guid, e| {
        let ours = carried && ridden == Some(*e);
        if ours && transports.get(*e).is_ok_and(|t| t.touches_map(map_id)) {
            // info, not debug: rare, and the crossing depends on it firing for the ridden boat.
            info!("worldport: sparing transport {guid:#x} (its path touches map {map_id})");
            // It stands for the new object the post-port create builds, so its clock goes until
            // that create sets one (`0x5f7080`); the deck holds its pose meanwhile.
            commands
                .entity(*e)
                .remove::<crate::transport::TransportAnchor>();
            return true;
        }
        commands.entity(*e).try_despawn();
        false
    });
    let announced = pending.0.take();
    if let Some(p) = announced {
        // vmangos pairs every NEW_WORLD with a same-map TRANSFER_PENDING; a mismatch only warns.
        if p.map_id != map_id {
            warn!(
                "worldport: NEW_WORLD map {map_id} ≠ announced transfer map {} — using {map_id}",
                p.map_id
            );
        }
    }
    let transport_entry = announced.and_then(|p| p.transport_entry);
    worldports.write(WorldportMessage {
        map_id,
        position,
        orientation,
        needs_ack,
        transport_entry,
    });
}

/// `SMSG_QUERY_TIME_RESPONSE`: the server wall clock that absolute descriptor stamps (the
/// timed-quest timer) are dated in; its skew against ours is logged once per session.
fn server_unix_time(unix_time: u32, clock: &mut ServerWallClock) {
    // `time(NULL)` at the sync (`0x4deef3`), whose difference is the reference's offset.
    let local = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if clock.0.is_none() {
        info!(
            "net: server wall clock {unix_time} (local clock differs by {}s)",
            local - i64::from(unix_time)
        );
    }
    clock.sample(unix_time, local);
}

/// `SMSG_INITIALIZE_FACTIONS`: the login reputation store.
fn reputations(standings: Vec<(u8, i32)>, reputations: &mut Reputations) {
    info!("net: reputation store ({} slots)", standings.len());
    reputations.set_slots(&standings);
}

/// `SMSG_SET_FORCED_REACTIONS`: the server resends the table whole on every change, so it
/// replaces the last one.
fn forced_reactions(reactions: Vec<(u32, u32)>, reputations: &mut Reputations) {
    info!("net: forced reactions ({} factions)", reactions.len());
    reputations.set_forced(reactions);
}

/// What an entry of a standing message says, where it says it.
enum Said {
    /// The first entry's "increased by" line, for the slot as it was.
    Change {
        list_id: u32,
        flags: u8,
        old: i32,
        standing: i32,
    },
    /// A rank line.
    Line(crate::ui_reputation::StandingLine),
}

/// `SMSG_SET_FACTION_STANDING` (`0x4d5760`), entry by entry: the slot's standing stored, the
/// slot made visible unless it is visible or `HIDDEN` already (`0x4d5815`), and its at-war bit
/// set by the faction's rank after the entry below Unfriendly (`0x4d5833`), else cleared when that
/// rank rose (`0x4d583f`). `ranks` is the catalog and the active player's race and class the rank
/// reads its base by (`0x4d63a0`); without them the rule is skipped, as every rank reads Neutral
/// before the player's rebuild names the slots (`0x4d54ee`). `said` hears each line at its place
/// among the writes, and the pane's list is handed the adder at the listing (`0x4d5826`) and the
/// sort after the entries (`0x4d58c8`); the message always hands it one operation, which the lane
/// takes the snapshot at.
fn reputation_delta(
    standings: Vec<(u32, i32)>,
    reputations: &mut Reputations,
    ranks: Option<(&benilla_formats::FactionCatalog, u8, u8)>,
    quest: &mut QuestGiver,
    said: &mut dyn FnMut(Said),
) -> DeltaLatches {
    use benilla_formats::faction_flags as flag;
    let rank = |reputations: &Reputations, slot: usize| {
        ranks.map(|(catalog, race, class)| {
            crate::ui_reputation::slot_rank(catalog, reputations, race, class, slot)
        })
    };
    let mut latches = DeltaLatches::default();
    for (n, (list_id, standing)) in standings.into_iter().enumerate() {
        let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_STANDING") else {
            continue;
        };
        let (stored, was) = reputations.slot(i);
        let old = rank(reputations, i);
        reputations.set_standing(i, standing);
        // `0x4d6330` prints the first entry's change with the standing stored (`0x4d634a`).
        if n == 0 {
            said(Said::Change {
                list_id,
                flags: stored,
                old: was,
                standing,
            });
        }
        let line = ranks.and_then(|(catalog, race, class)| {
            crate::ui_reputation::delta_line((list_id, standing), stored, was, catalog, race, class)
        });
        let mut flags = stored;
        let listed = flags & (flag::VISIBLE | flag::HIDDEN) == 0;
        if listed {
            flags |= flag::VISIBLE;
            // The adder, which lists the slot (`0x4d5826`) and prints its rank (`0x4d5bfb`), the
            // flag byte not yet written.
            reputations.list_op(ReputationListOp::Edit {
                listed: vec![list_id],
                sort: false,
            });
            if let Some(line) = line {
                said(Said::Line(line));
            }
        }
        if let (Some(old), Some(new)) = (old, rank(reputations, i)) {
            latches.rank_moved |= new != old;
            if new < 2 {
                flags |= flag::AT_WAR;
            } else if new > old {
                flags &= !flag::AT_WAR;
            }
        }
        if flags != stored {
            latches.flags_moved = true;
            reputations.set_flags(i, flags);
        }
        // A moved rank prints after the flag write (`0x4d5896`), unless the adder printed.
        if let Some(line) = line.filter(|_| !listed) {
            said(Said::Line(line));
        }
    }
    // Standing feeds questgiver status (`SatisfyQuestReputation`); the reference re-asks here too.
    quest.bump_reask();
    reputations.list_op(ReputationListOp::Edit {
        listed: vec![],
        sort: latches.flags_moved,
    });
    latches
}

/// What a standing message's entries latched for after them.
#[derive(Default)]
struct DeltaLatches {
    /// A slot's flag byte moved (`0x4d585d`-`0x4d5876`): the list re-sorts.
    flags_moved: bool,
    /// A slot's rank moved, `HIDDEN` or not (`0x4d5880`, `0x4d58a9`, `0x4d58ac`): the quest log and
    /// the reaction refresh read it.
    rank_moved: bool,
}

/// `SMSG_SET_FACTION_VISIBLE`: sets `FACTION_FLAG_VISIBLE` on a slot without it, which the pane
/// lists by (`0x4d572e`-`0x4d5745`); it carries no standing. `listing` runs ahead of the write, on
/// a slot newly listed (the adder, `0x4d5738`). Whether the slot was newly listed.
fn reputation_visible(
    list_id: u32,
    reputations: &mut Reputations,
    listing: impl FnOnce(&mut Reputations),
) -> bool {
    use benilla_formats::faction_flags::VISIBLE;
    let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_VISIBLE") else {
        return false;
    };
    let flags = reputations.flags(i);
    let listed = flags & VISIBLE == 0;
    if listed {
        listing(reputations);
    }
    reputations.set_flags(i, flags | VISIBLE);
    listed
}

/// `SMSG_SET_FACTION_ATWAR` (`0x4d56b0`): bit `0x02` of the wire byte becomes the slot's at-war
/// bit and the byte's other bits are read nowhere (`0x4d56d9`-`0x4d56e9`). It sorts nothing and
/// refreshes no reaction; the pane's list signals `UPDATE_FACTION` when the bit moved a row.
fn reputation_at_war(list_id: u32, wire: u8, reputations: &mut Reputations) {
    use benilla_formats::faction_flags::AT_WAR;
    let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_ATWAR") else {
        return;
    };
    let flags = reputations.flags(i);
    reputations.set_flags(
        i,
        if wire & AT_WAR != 0 {
            flags | AT_WAR
        } else {
            flags & !AT_WAR
        },
    );
}

/// A wire `repListId` as a store index, or `None` (logged) past the `FACTION_LIST_LEN` array
/// (vmangos `MAX_FACTION_COUNT` 64); unchecked, `0xFFFF_FFFF` would resize the store to 34 GB.
fn reputation_slot(list_id: u32, opcode: &str) -> Option<usize> {
    let i = usize::try_from(list_id).ok()?;
    if i >= benilla_protocol::messages::FACTION_LIST_LEN {
        warn!(
            "net: {opcode} names reputation slot {list_id}, past the {}-entry faction list — dropped",
            benilla_protocol::messages::FACTION_LIST_LEN
        );
        return None;
    }
    Some(i)
}

/// The dropped-packet tally: counts each drop and logs an opcode's first drop at info, one line
/// per opcode per run.
fn packet_dropped(opcode: u16, unparseable: bool, dropped: &mut DroppedOpcodes) {
    let tally = dropped.0.entry(opcode).or_default();
    if tally.unknown + tally.unparseable == 0 {
        info!(
            "net: dropped packet {opcode:#06x} ({}) — {} (first occurrence; tallying)",
            benilla_protocol::messages::opcode_name(opcode).unwrap_or("?"),
            if unparseable {
                "parser errored"
            } else {
                "no parser"
            },
        );
    }
    if unparseable {
        tally.unparseable += 1;
    } else {
        tally.unknown += 1;
    }
}

#[cfg(test)]
mod clock_tests;
#[cfg(test)]
mod reputation_tests;

#[cfg(test)]
mod teardown_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// One teardown of `end` over a session in the world as guid 7, whose avatar is spawned:
    /// whether the self guid survived and the reason the select screen would show.
    fn teardown(end: benilla_protocol::SessionEnd) -> (Option<u64>, Option<String>, bool) {
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .insert_resource(SelfGuid(Some(7)))
            .init_resource::<NetStatus>()
            .init_resource::<NameCache>()
            .init_resource::<Items>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<PendingTransfer>()
            .add_message::<DisconnectedMessage>();
        let avatar = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(7, avatar);
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      mut index: ResMut<GuidIndex>,
                      mut self_guid: ResMut<SelfGuid>,
                      mut status: ResMut<NetStatus>,
                      mut names: ResMut<NameCache>,
                      mut items: ResMut<Items>,
                      mut cooldowns: ResMut<crate::spell::Cooldowns>,
                      mut transfer: ResMut<PendingTransfer>,
                      mut out: MessageWriter<DisconnectedMessage>| {
                    disconnected(
                        "character login refused".into(),
                        end,
                        &mut commands,
                        &mut index,
                        &mut self_guid,
                        &mut status,
                        &mut names,
                        &mut items,
                        &mut cooldowns,
                        &mut transfer,
                        &mut out,
                    );
                },
            )
            .unwrap();
        (
            app.world().resource::<SelfGuid>().0,
            app.world().resource::<NetStatus>().last_reason.clone(),
            app.world().get_entity(avatar).is_ok(),
        )
    }

    /// A refused login (or a logout) ends the character with the link kept: the self guid goes as
    /// on a logout, and no failure is left for character select to show as "Server down".
    #[test]
    fn a_teardown_on_a_kept_link_ends_the_character_and_reports_no_failure() {
        let (guid, reason, avatar) = teardown(benilla_protocol::SessionEnd::LoggedOut);
        assert_eq!(guid, None, "the refused character's guid outlived it");
        assert_eq!(reason, None, "a kept link is no failure");
        assert!(!avatar);
    }

    /// An unattended run's lost link keeps the avatar for the reconnect, and its reason.
    #[test]
    fn a_reconnectable_loss_keeps_the_avatar_and_its_reason() {
        let _lock = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _decl = crate::local_state::test_env::EnvGuard::set("WOW_UNATTENDED", "1");
        let (guid, reason, avatar) = teardown(benilla_protocol::SessionEnd::Lost);
        assert_eq!(guid, Some(7));
        assert!(reason.is_some());
        assert!(avatar);
    }
}

#[cfg(test)]
mod forced_reaction_tests {
    use super::*;
    use crate::net::handlers::dispatch;
    use benilla_protocol::SessionEnd;

    /// The registered session handlers over the plugin's own resources.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(crate::net::NetPlugin { connect: false })
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::ui_chat::ChatLog>();
        app
    }

    fn forced(app: &App) -> Vec<(u32, u32)> {
        app.world().resource::<Reputations>().forced()
    }

    fn arrive(app: &mut App, table: &[(u32, u32)]) {
        let ev = SessionEvent::ForcedReactions {
            reactions: table.to_vec(),
        };
        dispatch(app.world_mut(), vec![ev]);
    }

    /// The server resends the table whole on every change, and an aura's removal sends it empty
    /// (vmangos `ReputationMgr::SendForceReactions`): each arrival replaces the last, and the
    /// standings beside it stay.
    #[test]
    fn each_arrival_replaces_the_table_and_an_empty_one_clears_it() {
        let mut app = app();
        dispatch(
            app.world_mut(),
            vec![SessionEvent::Reputations {
                standings: vec![(0x20, 5)],
            }],
        );
        arrive(&mut app, &[(28, 4), (72, 6)]);
        assert_eq!(forced(&app), [(28, 4), (72, 6)]);
        arrive(&mut app, &[(28, 2)]);
        assert_eq!(forced(&app), [(28, 2)], "replaced, not merged");
        arrive(&mut app, &[]);
        assert!(forced(&app).is_empty(), "an empty table clears it");
        assert_eq!(
            app.world().resource::<Reputations>().slot(0),
            (0x20, 5),
            "the standings are not the table's"
        );
    }

    /// The table is the character's: the session's end drops it, on a logout and on a disconnect.
    #[test]
    fn the_session_end_clears_the_table() {
        for end in [
            SessionEvent::LoggedOut,
            SessionEvent::Disconnected {
                reason: "gone".into(),
                end: SessionEnd::LoggedOut,
            },
        ] {
            let mut app = app();
            arrive(&mut app, &[(28, 4)]);
            assert_eq!(forced(&app).len(), 1);
            dispatch(app.world_mut(), vec![end.clone()]);
            assert!(forced(&app).is_empty(), "{end:?} left the table");
        }
    }

    /// The session's end is the teardown (`0x4d5360`), which zeroes all 64 slots
    /// (`0x4d541f`-`0x4d542b`), so no standing or at-war bit outlives the character.
    #[test]
    fn the_session_end_zeroes_the_slots() {
        for end in [
            SessionEvent::LoggedOut,
            SessionEvent::Disconnected {
                reason: "gone".into(),
                end: SessionEnd::LoggedOut,
            },
        ] {
            let mut app = app();
            dispatch(
                app.world_mut(),
                vec![SessionEvent::Reputations {
                    standings: vec![(0x03, 5000); 64],
                }],
            );
            dispatch(app.world_mut(), vec![end.clone()]);
            let reps = app.world().resource::<Reputations>();
            assert!(reps.slots().all(|s| s == (0, 0)), "{end:?} left a slot");
        }
    }
}

#[cfg(test)]
mod worldport_tests {
    use super::*;
    use crate::net::handlers::dispatch;

    const RIDDEN: u64 = 0xF120_0000_0000_0001;
    const IN_VIEW: u64 = 0xF120_0000_0000_0002;

    /// Two ships whose paths touch maps 1 and 0, one of them ridden, and a worldport to map 0,
    /// announced with a transport or not: which ships outlive it.
    fn kept_across(riding_transfer: bool) -> (bool, bool) {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        let ship = |world: &mut World, guid: u64| {
            let e = world
                .spawn((
                    crate::net::Guid(guid),
                    crate::transport::Transport::sailing_for_test((1, 0)),
                ))
                .id();
            world.resource_mut::<GuidIndex>().0.insert(guid, e);
            e
        };
        let ridden = ship(world, RIDDEN);
        let in_view = ship(world, IN_VIEW);
        world.insert_resource(crate::player::Player::riding_for_test(ridden, RIDDEN));
        world.resource_mut::<PendingTransfer>().0 = Some(crate::net::PendingTransferInfo {
            map_id: 0,
            transport_entry: riding_transfer.then_some(20808),
            from_map: Some(1),
        });
        dispatch(
            world,
            vec![SessionEvent::Worldport {
                map_id: 0,
                position: [0.0; 3],
                orientation: 0.0,
                needs_ack: true,
            }],
        );
        fire_worldport_timer(world);
        let alive = |world: &World, guid: u64, e: Entity| {
            world.resource::<GuidIndex>().0.contains_key(&guid) && world.get_entity(e).is_ok()
        };
        (alive(world, RIDDEN, ridden), alive(world, IN_VIEW, in_view))
    }

    /// The ship kept across a worldport stands for the new object the reference builds from the
    /// post-port create (`0x401bc0`), so that create sets its clock (`0x5f7080`) as a new one's.
    #[test]
    fn the_ship_kept_across_a_worldport_takes_its_next_create_as_new() {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        let old = std::time::Instant::now() - std::time::Duration::from_secs(30);
        let ridden = world
            .spawn((
                crate::net::Guid(RIDDEN),
                crate::transport::Transport::sailing_for_test((1, 0)),
                crate::transport::TransportAnchor {
                    progress_ms: 5,
                    at: old,
                },
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(RIDDEN, ridden);
        world.insert_resource(crate::player::Player::riding_for_test(ridden, RIDDEN));
        world.resource_mut::<PendingTransfer>().0 = Some(crate::net::PendingTransferInfo {
            map_id: 0,
            transport_entry: Some(20808),
            from_map: Some(1),
        });
        dispatch(
            world,
            vec![SessionEvent::Worldport {
                map_id: 0,
                position: [0.0; 3],
                orientation: 0.0,
                needs_ack: true,
            }],
        );
        fire_worldport_timer(world);
        let fresh = std::time::Instant::now();
        let create = SessionEvent::ObjectCreate {
            guid: RIDDEN,
            kind: benilla_protocol::EntityKind::GameObject,
            display_id: None,
            position: [0.0; 3],
            orientation: 0.0,
            scale: 1.0,
            speeds: None,
            mover: None,
            transport_progress: Some(benilla_protocol::TransportClock {
                progress_ms: 777,
                arrived: Some(fresh),
            }),
            transport: None,
            spline: None,
            fields: benilla_protocol::ObjectFields::from_pairs(&[(21, 15)])
                .into_created(benilla_protocol::messages::ObjectType::GameObject),
        };
        dispatch(world, crate::net::wire(vec![create]));
        world.flush();
        let anchor = world
            .get::<crate::transport::TransportAnchor>(ridden)
            .expect("anchored");
        assert_eq!((anchor.progress_ms, anchor.at), (777, fresh));
    }

    /// `0x401bc0` destroys every object at `SMSG_NEW_WORLD`; only the ship a transport transfer
    /// carries us on is kept, until its create arrives. A ship merely in view goes with its
    /// colliders, as does the ridden one on a transfer that does not ride it (a hearth).
    #[test]
    fn a_worldport_keeps_only_the_ship_we_ride_through_it() {
        assert_eq!(kept_across(true), (true, false));
        assert_eq!(kept_across(false), (false, false));
    }
}

#[cfg(test)]
mod login_ask_tests {
    use super::*;
    const CHALLENGER: u64 = 0x99;

    /// `SMSG_DUEL_REQUESTED`, whose handler asks the challenger's name (`0x4d4a83`).
    fn requested() -> SessionEvent {
        SessionEvent::DuelRequested {
            arbiter: 0xF110_0000_0000_0001,
            challenger: CHALLENGER,
        }
    }

    /// The world entry clears the caches' in-flight asks in its handler, ahead of the world's
    /// first packet (`0x48f586` → `0x555740`), so a name a later packet of the login frame asks
    /// stays pending through the frame, and the next packet's ask of it sends nothing.
    #[test]
    fn a_name_asked_in_the_login_frame_is_asked_once() {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
            bevy::ecs::error::ignore,
        ));
        let (tx, sent) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let (inbound, rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetEvents::for_test(rx));
        let frame = |app: &mut App, events: Vec<SessionEvent>| {
            for ev in events {
                inbound.send(ev).unwrap();
            }
            app.update();
        };
        let login = SessionEvent::Connected {
            self_guid: 0x2a,
            name: "Probeseven".into(),
            billing: Default::default(),
            tutorial_flags: None,
            addon_info: None,
        };
        frame(&mut app, vec![login, requested()]);
        assert_eq!(
            app.world().resource::<SelfGuid>().0,
            Some(0x2a),
            "the login handler ran"
        );
        frame(&mut app, vec![requested()]);
        // The world session's packets wait for the entry load (`net::apply::InboundHold`).
        for _ in 0..3 {
            app.update();
        }
        assert!(
            !app.world()
                .contains_resource::<crate::ui_script::PendingEntryUiLoad>(),
            "the entry load ran"
        );
        let asked = crate::query_cache::queries(&sent)
            .into_iter()
            .filter(|c| matches!(c, crate::net::ClientCommand::NameQuery { guid: CHALLENGER }))
            .count();
        assert_eq!(asked, 1, "the login frame forgot its packet's pending ask");
    }

    /// `DBCacheClearUnsaved` (`0x555740`) wipes a cache whole, records and asks, only when its
    /// persist byte is 0 (player names `0xc0e228`, guilds, pet names and petitions); a saved
    /// cache keeps both. The item cache's and the page cache's in-flight asks stand, and an
    /// unsaved player name's ask and record are gone.
    #[test]
    fn the_world_entry_wipes_the_unsaved_caches_and_leaves_the_saved_ones() {
        use crate::query_cache::Ask;
        const ITEM: u32 = 5_000;
        const PAGE: u32 = 358;
        const ANSWERED_PAGE: u32 = 359;
        const ASKED: u64 = 0x77;
        const KNOWN: u64 = 0x78;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
            bevy::ecs::error::ignore,
        ));
        let (tx, sent) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let (inbound, rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetEvents::for_test(rx));
        let ask_all = |app: &App| {
            let commands = app.world().resource::<NetCommands>();
            app.world()
                .resource::<Items>()
                .template(ITEM, 0, Ask::Event(commands));
            app.world()
                .resource::<crate::ui_item_text::PageTexts>()
                .page(PAGE, 0, Ask::Event(commands));
            app.world()
                .resource::<NameCache>()
                .resolve_player(ASKED, Ask::Event(commands));
            crate::query_cache::queries(&sent)
        };
        use crate::net::ClientCommand as C;
        assert_eq!(ask_all(&app).len(), 3, "each cache sends its ask");
        app.world_mut()
            .resource_mut::<crate::ui_item_text::PageTexts>()
            .insert(ANSWERED_PAGE, "A saved page.".into(), 0);
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(KNOWN, "Unsaved".into(), None);

        inbound
            .send(SessionEvent::Connected {
                self_guid: 0x2a,
                name: "Probeseven".into(),
                billing: Default::default(),
                tutorial_flags: None,
                addon_info: None,
            })
            .unwrap();
        app.update();

        let again = ask_all(&app);
        assert!(
            !again
                .iter()
                .any(|c| matches!(c, C::ItemQuery { .. } | C::PageTextQuery { .. })),
            "a saved cache's ask stands, so it is not sent again: {again:?}"
        );
        assert!(
            again
                .iter()
                .any(|c| matches!(c, C::NameQuery { guid: ASKED })),
            "an unsaved cache's ask is gone, so it is sent again: {again:?}"
        );
        let world = app.world();
        assert!(
            world
                .resource::<crate::ui_item_text::PageTexts>()
                .page(ANSWERED_PAGE, 0, Ask::Read)
                .is_some(),
            "a saved cache keeps its records"
        );
        assert_eq!(
            world
                .resource::<NameCache>()
                .resolve_player(KNOWN, Ask::Read),
            None,
            "an unsaved cache loses its records"
        );
    }
}
