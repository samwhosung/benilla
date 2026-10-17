//! The leave-world sweep `0x490a80`: past its latch, 36 calls with no branch between them, each
//! closing or freeing one subsystem's world state, then `PLAYER_LEAVING_WORLD` (`0x490b4d`) and
//! the close of whatever interaction is still latched (`0x490b5f` → `0x493310`), all inside a
//! `PlaySound` suppression scope (`0x490a94` to `0x490b64`), and last the drop of a cinematic in
//! flight (`0x490b69`).
//!
//! Three callers reach it, and it is the same sweep from each: a `/reload` and every UI shutdown
//! root (`0x490bd0`, at `0x490c20`), the local player's destructor on a cross-map worldport
//! (`0x5dd543`), and a `SMSG_DESTROY_OBJECT` of the local player (`0x4674cf` → `0x464920`, the
//! player's deactivate `0x5e9b5a`); an out-of-range block skips the active player (`0x465f3e`).
//! Ours is one schedule, [`WorldLeaveSweep`], run in place by [`run_leave_world_sweep`] so every
//! leg tells the VM it was called from, the dying one on a `/reload`, before the caller goes on.
//! Its callers are the UI shutdown's roots (`super::lifecycle`) and [`sweep_on_worldport`]; the
//! local player's destroy has no producer here.
//!
//! Each subsystem with state benilla models adds its leg in its own [`SweepLeg`] set; the sets run
//! in the reference's call order. The calls benilla has nothing for: the field-watch
//! unregistrations (`0x490aad`, `0x490ab7`, `0x490acb`, `0x490ae4`, `0x490af3`, and the tail of
//! `0x490ada`'s leg), frees of caches it does not hold (the minimap's points of interest
//! `0x490a99`, the durability alert `0x490aa8`, the taxi nodes at the head of `0x490ad0`'s leg,
//! the trade skill's item callbacks `0x490ae9`, the battlefield list `0x490b20`, the minigame
//! `0x490b34`), the sound band's reset (`0x490b3e` → `0x72a450`, `[0xcf06e4]`), the pending-chat
//! latch (`0x490a9e`), an empty stub (`0x490b43`), six bare `ret`s, and past the event the
//! camera's detach (`0x490b52`-`0x490b5a`, `0x4841a0` → `0x50d0f0(0)`).

use bevy::ecs::schedule::{ExecutorKind, ScheduleConfigs, ScheduleLabel};
use bevy::ecs::system::ScheduleSystem;
use bevy::prelude::*;

use benilla_ui::script::UiScript;

/// The world latch, the reference's `[0xb4b424]`: set at `0x4908ce` in the world-enter cascade
/// `0x4908c0` and cleared at `0x490a8d` by the sweep, so the first departure in a world sweeps
/// and the rest are no-ops until the next entry. Spent only by [`run_leave_world_sweep`]. A
/// cross-map worldport spends it without leaving `InWorld`, so a quit on its loading screen
/// sweeps nothing; a `/reload` sweeps, and its rebuild re-arms.
#[derive(Resource, Default)]
pub(crate) struct LeavingWorldArmed(bool);

impl LeavingWorldArmed {
    /// A world began. Idempotent, like the reference's `mov byte [0xb4b424],1`.
    pub(crate) fn arm(&mut self) {
        self.0 = true;
    }

    /// Take the one-shot: `true` at most once per world, `0x490a8d`'s clear folded in.
    fn spend(&mut self) -> bool {
        std::mem::take(&mut self.0)
    }

    /// Read without taking: whether a world is entered, as the script memory watch tests the
    /// byte (`0x4955ab`).
    pub(crate) fn is_armed(&self) -> bool {
        self.0
    }
}

/// How many sweeps have run: a window's feed memory is good for one VM and one world, since the
/// sweep closes the window and tells the VM itself ([`super::VmMemo::get_in_world`]).
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct SweptWorlds(pub(crate) u64);

/// The sweep's legs, in one schedule so a caller runs them in place.
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct WorldLeaveSweep;

/// The legs benilla models, by their call sites in `0x490a80`, in its order.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SweepLeg {
    /// `0x490a94` → `0x458f50`: the `PlaySound` suppression scope opens.
    Silence,
    /// `0x490ab2` → `0x4c1ca0` → `CloseInteraction 0x48f200(cl=1, dl=0)`.
    Loot,
    /// `0x490abc` → `0x4e3240` → `0x4e32e0(0, 0)`.
    ItemText,
    /// `0x490ac1` → `0x4e1f60` → `0x4e1fa0(0)`.
    Gossip,
    /// `0x490ac6` → `0x500bc0` → `0x501130(0, 1)`.
    Quest,
    /// `0x490ad0` → `0x4db700` → `0x4dc0f0`.
    Taxi,
    /// `0x490ad5` → `0x4d7400` → `0x4d7440(0)`.
    Trainer,
    /// `0x490ada` → `0x4facc0` → `0x4fadf0`.
    Merchant,
    /// `0x490adf` → `0x4bf220` → `0x5d4310`.
    Trade,
    /// `0x490afd` → `0x4d47d0`.
    Duel,
    /// `0x490b02` → `0x4f5830` → `0x4f58a0`.
    Tabard,
    /// `0x490b07` → `0x4f4fa0` → `0x4f5010`.
    Registrar,
    /// `0x490b11` → `0x4f3eb0`.
    Petition,
    /// `0x490b1b` → `0x4acc00` → `0x4acd50(cl=1)`.
    Mail,
    /// `0x490b25` → `0x4cc2f0` → `0x4cd5f0`.
    Auction,
    /// `0x490b2a` → `0x4caae0` → `0x4cae10`.
    Stable,
    /// `0x490b2f` → `0x4c9f80`.
    MeetingStone,
    /// `0x490b48`/`0x490b4d`: `PLAYER_LEAVING_WORLD`, its one fire site.
    LeavingWorld,
    /// `0x490b5f` → `0x493310`: the interaction still latched. Every window's leg above
    /// released its own but the mailbox's, whose `cl` arm leaves the latch (`0x4acd5f`) with its
    /// cell zeroed, so its object meets only the game-object arm (`0x493513` → `0x5f8c40`), which
    /// benilla has nothing for. Beside it, only the bank and a trade can be latched by now.
    Interaction,
    /// `0x490b64` → `0x458f60`: the suppression scope closes.
    Unsilence,
    /// `0x490b69`-`0x490bbb`: a cinematic in flight is dropped.
    Cinematic,
}

/// Spend the latch and, if it was armed, run the sweep: `true` at most once per world. No latch
/// is no world to leave.
pub(crate) fn run_leave_world_sweep(world: &mut World) -> bool {
    let swept = world
        .get_resource_mut::<LeavingWorldArmed>()
        .is_some_and(|mut l| l.spend());
    if swept {
        if let Some(mut worlds) = world.get_resource_mut::<SweptWorlds>() {
            worlds.0 += 1;
        }
        // Absent in a harness that registers no leg.
        let _ = world.try_run_schedule(WorldLeaveSweep);
    }
    swept
}

/// Open the sweep's `PlaySound` suppression scope (`0x490a94`), so the legs' sounds and their
/// handlers' are dropped.
fn silence(script: Option<NonSend<UiScript>>) {
    if let Some(script) = script {
        script.push_sound_suppression();
    }
}

/// Close it (`0x490b64`), ahead of the cinematic's drop.
fn unsilence(script: Option<NonSend<UiScript>>) {
    if let Some(script) = script {
        script.pop_sound_suppression();
    }
}

/// `PLAYER_LEAVING_WORLD`, fired unconditionally past the latch (`0x490b48`, its one site).
///
/// Deviation: on a worldport it fires with our descriptor present, so `UnitExists("player")`
/// answers true where the reference's (`0x515970`) misses, because matching it breaks
/// `UnitExists` for no stock reader.
fn fire_player_leaving_world(script: Option<NonSendMut<UiScript>>) {
    if let Some(mut script) = script {
        script.fire_event("PLAYER_LEAVING_WORLD", Vec::new());
    }
}

/// Registering the sweep's legs.
pub(crate) trait SweepApp {
    /// The schedule, its order and its event, once; every leg's plugin calls it.
    fn init_world_leave_sweep(&mut self) -> &mut Self;
    /// Add `systems` as the leg `leg`.
    fn add_sweep_leg<M>(
        &mut self,
        leg: SweepLeg,
        systems: impl IntoScheduleConfigs<ScheduleSystem, M>,
    ) -> &mut Self;
}

impl SweepApp for App {
    fn init_world_leave_sweep(&mut self) -> &mut Self {
        if self.world().contains_resource::<SweptWorlds>() {
            return self;
        }
        self.init_resource::<SweptWorlds>()
            .init_resource::<LeavingWorldArmed>()
            .add_schedule(sweep_schedule())
    }

    fn add_sweep_leg<M>(
        &mut self,
        leg: SweepLeg,
        systems: impl IntoScheduleConfigs<ScheduleSystem, M>,
    ) -> &mut Self {
        self.init_world_leave_sweep();
        let systems: ScheduleConfigs<ScheduleSystem> = systems.into_configs();
        self.add_systems(WorldLeaveSweep, systems.in_set(leg))
    }
}

/// The sweep's schedule before any leg: one thread, the legs' order, and its own event.
fn sweep_schedule() -> Schedule {
    let mut schedule = Schedule::new(WorldLeaveSweep);
    schedule
        .set_executor_kind(ExecutorKind::SingleThreaded)
        // Two chains joined at the meeting stone: a tuple chains at most twenty.
        .configure_sets(
            (
                SweepLeg::Silence,
                SweepLeg::Loot,
                SweepLeg::ItemText,
                SweepLeg::Gossip,
                SweepLeg::Quest,
                SweepLeg::Taxi,
                SweepLeg::Trainer,
                SweepLeg::Merchant,
                SweepLeg::Trade,
                SweepLeg::Duel,
                SweepLeg::Tabard,
                SweepLeg::Registrar,
                SweepLeg::Petition,
                SweepLeg::Mail,
                SweepLeg::Auction,
                SweepLeg::Stable,
                SweepLeg::MeetingStone,
            )
                .chain(),
        )
        .configure_sets(
            (
                SweepLeg::MeetingStone,
                SweepLeg::LeavingWorld,
                SweepLeg::Interaction,
                SweepLeg::Unsilence,
                SweepLeg::Cinematic,
            )
                .chain(),
        )
        .add_systems((
            silence.in_set(SweepLeg::Silence),
            fire_player_leaving_world.in_set(SweepLeg::LeavingWorld),
            unsilence.in_set(SweepLeg::Unsilence),
        ));
    schedule
}

/// The sweep in a bare `World`, for a test that drives the lifecycle edges without an `App`: the
/// latch, the schedule, and `legs` in their sets.
#[cfg(test)]
pub(crate) fn install_world_leave_sweep(world: &mut World, legs: impl FnOnce(&mut Schedule)) {
    world.init_resource::<SweptWorlds>();
    world.init_resource::<LeavingWorldArmed>();
    let mut schedule = sweep_schedule();
    legs(&mut schedule);
    world.add_schedule(schedule);
}

/// Arm the latch when the local player's entity appears: the reference's `0x5deb60` entry into
/// the world-enter cascade, which a fresh login and a worldport's new-world create both take
/// (`0x5deb49 call 0x468570` / `0x5deb50 jne 0x5deb6a`).
pub(crate) fn arm_leaving_world_on_self_create(
    created: Query<(), Added<crate::net::SelfPlayer>>,
    mut armed: ResMut<LeavingWorldArmed>,
) {
    if !created.is_empty() {
        armed.arm();
    }
}

/// The sweep from the local player's destructor (`0x5dd543`) on a cross-map worldport; a
/// same-map teleport destroys nothing and never sweeps. The sweep runs at this system's sync
/// point, so its legs and their events land before the entering edge `feed_units`, ordered after
/// it, raises for the same port.
pub(crate) fn sweep_on_worldport(
    mut ports: MessageReader<crate::net::WorldportMessage>,
    mut commands: Commands,
) {
    // `needs_ack` false is the initial-login map, an arrival; the whole iterator is read so no
    // message carries over.
    if ports.read().filter(|w| w.needs_ack).count() > 0 {
        commands.queue(|world: &mut World| {
            run_leave_world_sweep(world);
        });
    }
}

/// What a test of a leg needs: a spied VM, and the reload's and the worldport's sweeps.
#[cfg(test)]
pub(crate) mod test_support {
    use bevy::prelude::*;

    use benilla_ui::script::UiScript;

    /// A VM with a frame that records each of `events`, in order, as it fires.
    pub(crate) fn spied_vm(events: &[&str]) -> UiScript {
        let vm = UiScript::new().expect("VM");
        let list = events
            .iter()
            .map(|e| format!("{e:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        vm.run(&format!(
            "SWEEP_SEEN = {{}} local f = CreateFrame('Frame') \
             for _, e in ipairs({{ {list} }}) do f:RegisterEvent(e) end \
             f:SetScript('OnEvent', function() table.insert(SWEEP_SEEN, event) end)"
        ))
        .expect("spy frame");
        vm
    }

    /// What `vm`'s spy has recorded.
    pub(crate) fn seen(vm: &UiScript) -> Vec<String> {
        vm.eval::<Vec<String>>("return SWEEP_SEEN")
            .expect("spy frame")
    }

    /// The spy of the VM in the world.
    pub(crate) fn seen_in(app: &App) -> Vec<String> {
        seen(app.world().non_send_resource::<UiScript>())
    }

    /// A worldport's sweep (`0x5dd543`): the latch armed as our own create arms it, the sweep on
    /// the VM in the world, which survives it.
    pub(crate) fn sweep_in_place(app: &mut App) {
        app.world_mut()
            .resource_mut::<super::LeavingWorldArmed>()
            .arm();
        assert!(super::run_leave_world_sweep(app.world_mut()));
    }

    /// A `/reload`'s two halves without the load (`0x495664`, `0x495669`): the sweep on the VM in
    /// the world, then a fresh VM spying on `events` in its place. Returns the dying VM.
    pub(crate) fn reload_in_place(app: &mut App, events: &[&str]) -> UiScript {
        sweep_in_place(app);
        let dying = app
            .world_mut()
            .remove_non_send_resource::<UiScript>()
            .expect("a VM in the world");
        app.insert_non_send_resource(spied_vm(events));
        dying
    }
}
