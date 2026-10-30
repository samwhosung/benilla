//! The lanes: the UI events the packet handlers raise, fired in the order the packets came. The
//! reference's poll takes the queued packets in order (`0x5384d0`, `0x538529`) and runs each
//! handler whole (`0x537aa0`, `0x537ae5`), and a handler's events fire inside it, every listener
//! before the signal returns (`0x703e50`, `0x703f50`). Here a handler queues what it raises on a
//! lane, a producer's queue that stamps each item from one clock as it is pushed ([`stamp`]).
//! Once a packet's handlers are done, the dispatch fires the lowest stamp across the lanes until
//! none is left ([`flush`]), so the events come out in the order their handlers raised them,
//! whichever lane holds them, and each listener reads, and sends into, the world its packet left,
//! ahead of the next packet's handler. What is pushed after the dispatch, by the feeds and the
//! frame's systems, fires in the frame's drain ([`fire_in_order`]).
//!
//! An item pushed while another fires is stamped under it ([`firing`]) and fires straight after
//! it, ahead of everything stamped before it, as a signal raised inside a listener runs to its
//! end first. An item that waits (a line for its sender's name) leaves its lane's merge and is
//! stamped again where it is released, as the name's answer shows the lines that waited on it
//! (`0x49ccc0`); one that never resolves holds nothing up.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use bevy::ecs::system::SystemId;
use bevy::prelude::*;

/// How deep a stamp nests: an item pushed by one fired that deep is stamped as a fresh one and
/// waits for the next drain, the next packet's flush or the frame's, so a listener that queues an
/// item each time it hears one cannot hold the dispatch.
///
/// Deviation: the reference has no such cap. A signal raised inside a listener runs nested in
/// its call, bounded by `luaD_call`'s C-stack count of 200 (`0x6f65ae`), whose error ends the
/// innermost listener alone, or by the stack of the thread that runs the loop, which the image's
/// stack reserve of 0x100000 bounds, whichever comes first: a chain through the chat line
/// function `0x49a870` takes an 8 KB frame each (`0x49a873`, `0x1f0c` bytes). The cap exists only because verbs a listener calls still queue
/// onto a lane instead of firing inside the call (#2202), and goes once none does.
const DEPTH: usize = 4;

/// An item's place in the order: the clock's count when it was pushed, under the stamps of the
/// items firing then, outermost first. Ordered as its path: a nested item after its cause and
/// ahead of whatever was stamped after its cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Stamp {
    path: [u64; DEPTH],
    len: u8,
}

impl Stamp {
    fn root(n: u64) -> Self {
        let mut path = [0; DEPTH];
        path[0] = n;
        Self { path, len: 1 }
    }

    fn child(self, n: u64) -> Self {
        let mut path = self.path;
        path[usize::from(self.len)] = n;
        Self {
            path,
            len: self.len + 1,
        }
    }

    fn path(&self) -> &[u64] {
        &self.path[..usize::from(self.len)]
    }

    /// Below every stamp the clock gives: an item there fires first in the drain.
    pub(crate) fn origin() -> Self {
        Self::root(0)
    }

    /// The count of the outermost push: the drain fires only what was stamped before it began.
    fn first(&self) -> u64 {
        self.path[0]
    }

    /// Whether the item was pushed at or after `mark`: its own count, the path's last, is.
    pub(crate) fn since(&self, mark: Mark) -> bool {
        self.path[usize::from(self.len) - 1] >= mark.0
    }
}

/// The clock's count at a point ([`mark`]), which every stamp taken after it reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mark(u64);

/// The clock now: a call marks where it begins, and takes the items stamped since as its own.
pub(crate) fn mark() -> Mark {
    Mark(CLOCK.load(Ordering::Relaxed))
}

impl Ord for Stamp {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path().cmp(other.path())
    }
}

impl PartialOrd for Stamp {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// The one clock every lane stamps from.
static CLOCK: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// The item firing on this thread, which what is pushed meanwhile nests under.
    static FIRING: Cell<Option<Stamp>> = const { Cell::new(None) };
    /// The stamps taken on this thread: what a firing pushed, never another thread's, whose
    /// stamps nest under nothing it fires.
    static TAKEN: Cell<u64> = const { Cell::new(0) };
}

/// The next stamp: a fresh one, or with an item firing, one nested under it.
pub(crate) fn stamp() -> Stamp {
    let n = CLOCK.fetch_add(1, Ordering::Relaxed);
    TAKEN.set(TAKEN.get() + 1);
    match FIRING.get() {
        Some(cause) if usize::from(cause.len) < DEPTH => cause.child(n),
        _ => Stamp::root(n),
    }
}

/// The item firing on this thread, if any: a verb its listeners call runs at that item's moment.
pub(crate) fn firing_now() -> Option<Stamp> {
    FIRING.get()
}

/// Whether the item firing on this thread was pushed between `from` and `to`, or nests under one
/// that was: it is part of what ran between the two marks.
pub(crate) fn firing_between(from: Mark, to: Mark) -> bool {
    FIRING
        .get()
        .is_some_and(|s| s.path().iter().any(|n| (from.0..to.0).contains(n)))
}

thread_local! {
    /// A close core is running held behind a lane: what a core it re-enters signals is its.
    static BEHIND: Cell<bool> = const { Cell::new(false) };
    /// The net dispatch is running the frame's packet handlers on this thread ([`at_packets`]).
    static AT_PACKETS: Cell<bool> = const { Cell::new(false) };
}

/// Run `f` as the dispatch of the frame's packets, each handler whole in wire order: a close core
/// a handler runs with no item firing takes its place on its lane there ([`waits_behind`]), so its
/// signals fire after every earlier packet's and ahead of every later one's, as the reference's
/// fire inside the handler that closes (`0x537ae5`, `0x703e50`).
pub(crate) fn at_packets<R>(f: impl FnOnce() -> R) -> R {
    let _restore = AtPackets::set(true);
    f()
}

/// [`AT_PACKETS`] as it stood before [`AtPackets::set`], put back at the drop.
struct AtPackets(bool);

impl AtPackets {
    fn set(now: bool) -> Self {
        Self(AT_PACKETS.replace(now))
    }
}

impl Drop for AtPackets {
    fn drop(&mut self) {
        AT_PACKETS.set(self.0);
    }
}

/// Where [`behind`] ran its core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Behind {
    /// At once, its signals fired in place.
    Ran,
    /// Held, its signals parked on the lane.
    Parked,
    /// Inside another core's hold, which parks its signals with its own.
    Nested,
}

/// Run a close `core` behind a lane whose head is `head`: at once unless it waits
/// ([`waits_behind`]), else with the world's VM held, what it signalled parked under a key `park`
/// queues on the lane, so a close never fires ahead of the open it closes, nor ahead of an earlier
/// packet's signals when a handler runs it. A core it re-enters, as a release re-enters its close
/// (`0x493310`), runs inside the hold.
pub(crate) fn behind(
    world: &mut World,
    head: Option<Stamp>,
    core: impl FnOnce(&mut World),
    park: impl FnOnce(&mut World, u64, u64),
) -> Behind {
    if BEHIND.get() {
        core(world);
        return Behind::Nested;
    }
    let Some((_held, mark)) = waits_behind(head)
        .then(|| crate::script_calls::defer_vm(world))
        .flatten()
    else {
        core(world);
        return Behind::Ran;
    };
    BEHIND.set(true);
    core(world);
    BEHIND.set(false);
    if let Some((session, key)) = crate::script_calls::park_since(world, mark) {
        park(world, session, key);
    }
    Behind::Parked
}

/// Whether a close run now must wait behind a lane whose head is `head`, as no other core's hold
/// already takes what it signals ([`behind`]): with an item firing, the lane holds one stamped
/// before it; with none, a packet handler runs it ([`at_packets`]), whose place is the lane's
/// next stamp, or the lane holds any item.
pub(crate) fn waits_behind(head: Option<Stamp>) -> bool {
    !BEHIND.get()
        && match firing_now() {
            Some(now) => head.is_some_and(|head| head < now),
            None => AT_PACKETS.get() || head.is_some(),
        }
}

/// Run `f` as the firing of the item stamped `at`, so what it pushes nests under the item; with
/// whether it stamped anything, after which the drain takes the lowest stamp again.
pub(crate) fn firing<R>(at: Stamp, f: impl FnOnce() -> R) -> (R, bool) {
    let before = TAKEN.get();
    let outer = FIRING.replace(Some(at));
    let r = benilla_ui::script::origin::within(benilla_ui::script::origin::Origin::Lane, f);
    FIRING.set(outer);
    (r, TAKEN.get() != before)
}

/// What a lane's firing may fire, among the stamps whose outermost push came before the drain
/// began: those below `before`, the lowest head of the lanes registered ahead of it, and up to
/// `through`, the lowest head of the lanes after it. Equal heads fire in lane order, so a tie
/// never stalls the drain.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Until {
    before: Option<Stamp>,
    through: Option<Stamp>,
    fence: u64,
}

impl Until {
    /// Everything stamped so far, for a lane fired alone.
    #[cfg(test)]
    pub(crate) fn all() -> Self {
        Self {
            before: None,
            through: None,
            fence: CLOCK.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn admits(&self, stamp: Stamp) -> bool {
        stamp.first() < self.fence
            && self.before.is_none_or(|b| stamp < b)
            && self.through.is_none_or(|b| stamp <= b)
    }
}

/// What an exclusive firing touches beside the call host and its own lane, as the access a system
/// declares: Bevy reports an exclusive system's as empty, so the lanes' measure takes this one
/// instead. Built from the systems the firing runs ([`system_access`]) and what its world-held
/// steps read.
pub(crate) type LaneReads = fn(&mut World) -> bevy::ecs::query::FilteredAccessSet;

/// The access `system` declares, measured on `world` by initializing a copy of it.
pub(crate) fn system_access<I: bevy::ecs::system::SystemInput + 'static, O: 'static, M>(
    world: &mut World,
    system: impl IntoSystem<I, O, M>,
) -> bevy::ecs::query::FilteredAccessSet {
    use bevy::ecs::system::System;
    IntoSystem::into_system(system).initialize(world)
}

/// One lane: its name, its head, its firing (a one-shot taking [`Until`], the `NetHandlers`
/// shape), what it does once ahead of each drain, an exclusive firing's declared reads, and, for
/// a log whose head waits at the origin, whether its log holds anything that raises ([`due`]).
struct Lane {
    name: &'static str,
    peek: fn(&World) -> Option<Stamp>,
    fire: SystemId<In<Until>>,
    begin: Option<fn(&World)>,
    reads: Option<LaneReads>,
    raises: Option<fn(&World) -> bool>,
}

/// Every lane, in registration order.
#[derive(Resource, Default)]
pub(crate) struct Lanes(
    Vec<Lane>,
    /// Each lane's firing's declared access, an exclusive one's from its [`LaneReads`], and whether
    /// it is exclusive, once [`measure`] ran.
    #[cfg(test)]
    Option<Vec<(bevy::ecs::query::FilteredAccessSet, bool)>>,
);

impl Lanes {
    /// The lanes' names, for the tests.
    #[cfg(test)]
    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.0.iter().map(|l| l.name).collect()
    }

    /// Each lane's firing's declared access, in registration order, for the tests that ask what
    /// the drain touches. Panics before [`measure`].
    #[cfg(test)]
    pub(crate) fn accesses(&self) -> Vec<&bevy::ecs::query::FilteredAccessSet> {
        self.1
            .as_deref()
            .expect("the lanes' accesses are read after `lanes::measure`")
            .iter()
            .map(|(a, _)| a)
            .collect()
    }

    /// Each lane's firing, in registration order: whether it is exclusive, and whether it
    /// declares its reads. Panics before [`measure`].
    #[cfg(test)]
    pub(crate) fn exclusive_reads(&self) -> Vec<(bool, bool)> {
        let measured = self
            .1
            .as_deref()
            .expect("the lanes' firings are read after `lanes::measure`");
        measured
            .iter()
            .zip(&self.0)
            .map(|((_, exclusive), lane)| (*exclusive, lane.reads.is_some()))
            .collect()
    }
}

/// Reads every lane firing's declared access, for [`Lanes::accesses`]: each is initialized, its
/// access kept (an exclusive one's from its [`LaneReads`]), and registered again, where its first run initializes it again on the same state.
/// Done on a test's asking, not at registration: a firing that lends the call host is most of a
/// plugin's build to initialize, and most tests fire few lanes or none.
#[cfg(test)]
pub(crate) fn measure(world: &mut World) {
    world.resource_scope(|world, mut lanes: Mut<Lanes>| {
        if lanes.1.is_some() {
            return;
        }
        let mut accesses = Vec::new();
        for lane in &mut lanes.0 {
            let mut system = world
                .unregister_system(lane.fire)
                .expect("a registered lane firing")
                .system();
            let exclusive = system.is_exclusive();
            let mut access = system.initialize(world);
            if let (true, Some(reads)) = (exclusive, lane.reads) {
                access = reads(world);
            }
            accesses.push((access, exclusive));
            lane.fire = world.register_boxed_system(system);
        }
        lanes.1 = Some(accesses);
    });
}

/// What runs once after each drain, however it ended: a lane's firing that stopped early leaves
/// state no read outside the drain may see.
#[derive(Resource, Default)]
struct DrainEnds(Vec<fn(&mut World)>);

/// What runs once ahead of the frame's drain alone ([`fire_in_order`]), never ahead of a
/// [`flush`]: a pass that counts frames.
#[derive(Resource, Default)]
struct FrameBegins(Vec<fn(&mut World)>);

/// What runs where a span of packets begins ([`span_starts`]): a log's starting state, read
/// before the span's first packet moves it.
#[derive(Resource, Default)]
struct SpanStarts(Vec<fn(&mut World)>);

thread_local! {
    /// The flushes that fired ([`flush`]) and the lane firings every drain ran, counted for the
    /// `drn` trace ([`tally`]).
    static FLUSHES: Cell<u64> = const { Cell::new(0) };
    static FIRED: Cell<u64> = const { Cell::new(0) };
}

/// The flushes fired and the lane firings run on this thread so far: a caller takes the
/// difference across what it measures.
pub(crate) fn tally() -> (u64, u64) {
    (FLUSHES.get(), FIRED.get())
}

/// The drain's place: after every feed in [`crate::ui_script::UiFeed`] has pushed, and before
/// the tick ([`crate::ui_script::UiTick`]).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiLanes;

pub(crate) trait LaneApp {
    /// Register a lane: `peek` reads its head, `fire` fires its items while an [`Until`] admits
    /// them, at least the head, and `begin` runs once ahead of each drain. The first lane adds
    /// the drain.
    fn add_lane<M>(
        &mut self,
        name: &'static str,
        peek: fn(&World) -> Option<Stamp>,
        fire: impl IntoSystem<In<Until>, (), M> + 'static,
        begin: Option<fn(&World)>,
    ) -> &mut Self;

    /// Register a lane whose firing holds the world, with what it reads beside the call host and
    /// its own lane ([`LaneReads`]).
    fn add_exclusive_lane(
        &mut self,
        name: &'static str,
        peek: fn(&World) -> Option<Stamp>,
        fire: fn(In<Until>, &mut World),
        reads: LaneReads,
    ) -> &mut Self;

    /// Run `end` once after each drain, whether it fired every item or stopped.
    fn add_drain_end(&mut self, end: fn(&mut World)) -> &mut Self;

    /// Run `begin` once ahead of the frame's drain, after the lanes' own begins.
    fn add_frame_begin(&mut self, begin: fn(&mut World)) -> &mut Self;

    /// Run `start` where each span of packets begins ([`span_starts`]).
    fn add_span_start(&mut self, start: fn(&mut World)) -> &mut Self;

    /// The lane `name`, whose head waits at the origin while its log holds entries, raises an
    /// event when `raises` holds, which makes it [`due`]; a log that raises nothing waits for the
    /// next drain that fires.
    fn lane_raises(&mut self, name: &'static str, raises: fn(&World) -> bool) -> &mut Self;
}

impl LaneApp for App {
    fn add_lane<M>(
        &mut self,
        name: &'static str,
        peek: fn(&World) -> Option<Stamp>,
        fire: impl IntoSystem<In<Until>, (), M> + 'static,
        begin: Option<fn(&World)>,
    ) -> &mut Self {
        if self.world().get_resource::<Lanes>().is_none() {
            crate::ui_script::raise_origin::arm_each_frame(self);
            self.init_resource::<Lanes>()
                .configure_sets(
                    Update,
                    UiLanes
                        .after(crate::ui_script::UiFeed)
                        .before(crate::ui_script::UiTick),
                )
                .add_systems(
                    Update,
                    fire_in_order
                        .in_set(UiLanes)
                        .run_if(crate::ui_script::ingame_ui_up),
                );
        }
        if self
            .world()
            .resource::<Lanes>()
            .0
            .iter()
            .any(|l| l.name == name)
        {
            return self;
        }
        let fire = self.world_mut().register_system(fire);
        self.world_mut().resource_mut::<Lanes>().0.push(Lane {
            name,
            peek,
            fire,
            begin,
            reads: None,
            raises: None,
        });
        self
    }

    fn add_exclusive_lane(
        &mut self,
        name: &'static str,
        peek: fn(&World) -> Option<Stamp>,
        fire: fn(In<Until>, &mut World),
        reads: LaneReads,
    ) -> &mut Self {
        self.add_lane(name, peek, fire, None);
        if let Some(lane) = self
            .world_mut()
            .resource_mut::<Lanes>()
            .0
            .iter_mut()
            .find(|l| l.name == name)
        {
            lane.reads = Some(reads);
        }
        self
    }

    fn add_drain_end(&mut self, end: fn(&mut World)) -> &mut Self {
        self.init_resource::<DrainEnds>();
        self.world_mut().resource_mut::<DrainEnds>().0.push(end);
        self
    }

    fn add_frame_begin(&mut self, begin: fn(&mut World)) -> &mut Self {
        self.init_resource::<FrameBegins>();
        self.world_mut().resource_mut::<FrameBegins>().0.push(begin);
        self
    }

    fn add_span_start(&mut self, start: fn(&mut World)) -> &mut Self {
        self.init_resource::<SpanStarts>();
        self.world_mut().resource_mut::<SpanStarts>().0.push(start);
        self
    }

    fn lane_raises(&mut self, name: &'static str, raises: fn(&World) -> bool) -> &mut Self {
        if let Some(lane) = self
            .world_mut()
            .get_resource_mut::<Lanes>()
            .and_then(|lanes| lanes.into_inner().0.iter_mut().find(|l| l.name == name))
        {
            lane.raises = Some(raises);
        }
        self
    }
}

/// The frame's drain: what the feeds and the frame's systems pushed after the dispatch, and what
/// no flush fired, in stamp order, with the frame's begins ahead ([`fire_through`]).
pub(crate) fn fire_in_order(world: &mut World) {
    fire_through(world, true);
}

/// Whether some lane holds what a drain would fire now: a head that is an item, or a log's start
/// at the origin whose log raises something ([`LaneApp::lane_raises`]). A log that raises nothing
/// keeps its entries for the next drain that fires, whose rewind takes them all in stamp order.
pub(crate) fn due(world: &World) -> bool {
    world.get_resource::<Lanes>().is_some_and(|lanes| {
        lanes.0.iter().any(|lane| {
            (lane.peek)(world).is_some_and(|head| {
                head != Stamp::origin() || lane.raises.is_some_and(|raises| raises(world))
            })
        })
    })
}

/// The lanes fired where a handler's signals would have fired inside its packet: every item
/// stamped so far, in stamp order, when the in-game interface is up and some lane is [`due`],
/// then the next span's starts. Whether it fired.
pub(crate) fn flush(world: &mut World) -> bool {
    if !crate::ui_script::ingame_ui_up_in(world) || !due(world) {
        return false;
    }
    FLUSHES.set(FLUSHES.get() + 1);
    fire_through(world, false);
    span_starts(world);
    true
}

/// Where a span of packets begins: each log reads its starting state ([`LaneApp::add_span_start`]).
pub(crate) fn span_starts(world: &mut World) {
    let starts = world
        .get_resource::<SpanStarts>()
        .map(|s| s.0.clone())
        .unwrap_or_default();
    for start in starts {
        start(world);
    }
}

/// Fire every lane's items in stamp order: each lane's `begin`, the frame's begins when `frame`,
/// then, while any lane's head was stamped before the drain began, the lane holding the lowest
/// fires up to the next lane's head, its Lua run with the call host lent; then the drain's ends.
/// A lane that fires nothing (no VM) ends the drain, its items waiting for the next. Its listeners
/// run outside the packets' dispatch ([`at_packets`]), as a signal's listeners run inside their
/// signal's call (`0x703e50`), not as a handler.
fn fire_through(world: &mut World, frame: bool) {
    let Some(lanes) = world.remove_resource::<Lanes>() else {
        return;
    };
    let _restore = AtPackets::set(false);
    for begin in lanes.0.iter().filter_map(|l| l.begin) {
        begin(world);
    }
    if frame {
        let begins = world
            .get_resource::<FrameBegins>()
            .map(|b| b.0.clone())
            .unwrap_or_default();
        for begin in begins {
            begin(world);
        }
    }
    let fence = CLOCK.load(Ordering::Relaxed);
    loop {
        let heads: Vec<Option<Stamp>> = lanes
            .0
            .iter()
            .map(|l| (l.peek)(world).filter(|s| s.first() < fence))
            .collect();
        let Some((i, head)) = heads
            .iter()
            .enumerate()
            .filter_map(|(i, h)| h.map(|h| (i, h)))
            .min_by_key(|(_, h)| *h)
        else {
            break;
        };
        // A head at the origin is a view's frame-start push, which raises nothing and commutes
        // with another view's: it bounds no other lane. The pick is the first lane holding the
        // lowest head, so a later lane's equal head bounds it inclusively.
        let lowest = |lanes: &[Option<Stamp>]| {
            lanes
                .iter()
                .filter_map(|h| *h)
                .filter(|h| *h != Stamp::origin())
                .min()
        };
        let until = Until {
            before: lowest(&heads[..i]),
            through: lowest(&heads[i + 1..]),
            fence,
        };
        let lane = &lanes.0[i];
        FIRED.set(FIRED.get() + 1);
        let fired =
            benilla_ui::script::origin::within(benilla_ui::script::origin::Origin::Lane, || {
                world.run_system_with(lane.fire, until)
            });
        if let Err(e) = fired {
            error!("lanes: `{}` did not fire: {e}", lane.name);
            break;
        }
        crate::creature_anim::apply_unit_edits(world);
        if (lane.peek)(world) == Some(head) {
            break;
        }
    }
    world.insert_resource(lanes);
    let ends = world
        .get_resource::<DrainEnds>()
        .map(|e| e.0.clone())
        .unwrap_or_default();
    for end in ends {
        end(world);
    }
}

#[cfg(test)]
mod account_data_live_tests;
#[cfg(test)]
mod action_live_tests;
#[cfg(test)]
mod alert_live_tests;
#[cfg(test)]
mod arrival_live_tests;
#[cfg(test)]
mod auction_live_tests;
#[cfg(test)]
mod aura_live_tests;
#[cfg(test)]
mod aura_view_tests;
#[cfg(test)]
mod bag_tests;
#[cfg(test)]
mod bank_live_tests;
#[cfg(test)]
mod battlefield_live_tests;
#[cfg(test)]
mod book_live_tests;
#[cfg(test)]
mod cast_live_tests;
#[cfg(test)]
mod chat_clock_live_tests;
#[cfg(test)]
mod close_parked_tests;
#[cfg(test)]
mod cooldown_live_tests;
#[cfg(test)]
mod deactivate_live_tests;
#[cfg(test)]
mod death_live_tests;
#[cfg(test)]
mod dialog_live_tests;
#[cfg(test)]
mod duration_live_tests;
#[cfg(test)]
mod follow_live_tests;
#[cfg(test)]
mod gossip_live_tests;
#[cfg(test)]
mod group_live_tests;
#[cfg(test)]
mod group_view_tests;
#[cfg(test)]
mod guard_tests;
#[cfg(test)]
mod guild_live_tests;
#[cfg(test)]
mod host_read_tests;
#[cfg(test)]
mod inspect_live_tests;
#[cfg(test)]
mod instance_live_tests;
#[cfg(test)]
mod item_live_tests;
#[cfg(test)]
mod item_tests;
#[cfg(test)]
mod item_text_live_tests;
#[cfg(test)]
mod item_view_live_tests;
#[cfg(test)]
mod lock_live_tests;
#[cfg(test)]
mod logout_live_tests;
#[cfg(test)]
mod loot_live_tests;
#[cfg(test)]
mod loot_roll_live_tests;
#[cfg(test)]
mod mail_live_tests;
#[cfg(test)]
mod merchant_live_tests;
#[cfg(test)]
mod name_live_tests;
#[cfg(test)]
mod nameplate_live_tests;
#[cfg(test)]
mod packet_signal_tests;
#[cfg(test)]
mod pet_live_tests;
#[cfg(test)]
mod petition_live_tests;
#[cfg(test)]
mod plate_live_tests;
#[cfg(test)]
mod profession_live_tests;
#[cfg(test)]
mod quest_log_live_tests;
#[cfg(test)]
mod quiet_tests;
#[cfg(test)]
mod record_callback_tests;
#[cfg(test)]
mod reputation_live_tests;
#[cfg(test)]
mod requirement_live_tests;
#[cfg(test)]
mod selection_live_tests;
#[cfg(test)]
mod social_live_tests;
#[cfg(test)]
mod spell_slot_live_tests;
#[cfg(test)]
mod stable_live_tests;
#[cfg(test)]
mod stance_live_tests;
#[cfg(test)]
mod stat_live_tests;
#[cfg(test)]
mod storm_tests;
#[cfg(test)]
mod tabard_live_tests;
#[cfg(test)]
pub(crate) mod tap_tests;
#[cfg(test)]
mod taxi_live_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod token_live_tests;
#[cfg(test)]
mod tooltip_live_tests;
#[cfg(test)]
mod trade_live_tests;
#[cfg(test)]
mod trainer_live_tests;
#[cfg(test)]
mod tutorial_live_tests;
#[cfg(test)]
mod ui_error_live_tests;
#[cfg(test)]
mod unit_feed_live_tests;
#[cfg(test)]
mod unit_live_tests;
#[cfg(test)]
mod unit_view_tests;
#[cfg(test)]
mod watch_order_tests;

#[cfg(test)]
mod world_map_live_tests;
#[cfg(test)]
mod world_map_update_tests;
#[cfg(test)]
mod world_state_live_tests;
#[cfg(test)]
mod worn_live_tests;

#[cfg(test)]
mod unit_tests {
    use super::*;

    /// A lane of named items, kept in stamp order.
    #[derive(Resource, Default)]
    struct Toy<const N: usize>(Vec<(Stamp, &'static str)>);

    /// What fired, in order.
    #[derive(Resource, Default)]
    struct Fired(Vec<&'static str>);

    /// What an item's firing pushes: `(cause, lane, item)`.
    #[derive(Resource, Default)]
    struct Pushes(Vec<(&'static str, usize, &'static str)>);

    fn push<const N: usize>(world: &mut World, item: &'static str) {
        let s = stamp();
        let mut lane = world.resource_mut::<Toy<N>>();
        let at = lane.0.partition_point(|(t, _)| *t < s);
        lane.0.insert(at, (s, item));
    }

    fn peek<const N: usize>(world: &World) -> Option<Stamp> {
        world.resource::<Toy<N>>().0.first().map(|(s, _)| *s)
    }

    fn fire<const N: usize>(In(until): In<Until>, world: &mut World) {
        loop {
            let Some(&(s, item)) = world.resource::<Toy<N>>().0.first() else {
                return;
            };
            if !until.admits(s) {
                return;
            }
            world.resource_mut::<Toy<N>>().0.remove(0);
            let pushes: Vec<(usize, &'static str)> = world
                .resource::<Pushes>()
                .0
                .iter()
                .filter(|(cause, _, _)| *cause == item)
                .map(|(_, lane, what)| (*lane, *what))
                .collect();
            let ((), moved) = firing(s, || {
                world.resource_mut::<Fired>().0.push(item);
                for (lane, what) in pushes {
                    match lane {
                        0 => push::<0>(world, what),
                        _ => push::<1>(world, what),
                    }
                }
            });
            if moved {
                return;
            }
        }
    }

    fn app(pushes: &[(&'static str, usize, &'static str)]) -> App {
        let mut app = App::new();
        app.init_resource::<Toy<0>>()
            .init_resource::<Toy<1>>()
            .init_resource::<Fired>()
            .insert_resource(Pushes(pushes.to_vec()))
            .add_lane("toy0", peek::<0>, fire::<0>, None)
            .add_lane("toy1", peek::<1>, fire::<1>, None);
        app
    }

    fn drain(app: &mut App) -> Vec<&'static str> {
        fire_in_order(app.world_mut());
        std::mem::take(&mut app.world_mut().resource_mut::<Fired>().0)
    }

    /// The handlers' order across two lanes, item by item (`0x5384d0`, `0x537aa0`).
    #[test]
    fn the_drain_fires_the_lowest_stamp_across_the_lanes() {
        let mut app = app(&[]);
        let w = app.world_mut();
        push::<0>(w, "a");
        push::<1>(w, "b");
        push::<1>(w, "c");
        push::<0>(w, "d");
        push::<1>(w, "e");
        assert_eq!(drain(&mut app), ["a", "b", "c", "d", "e"]);
    }

    /// Two lanes holding one stamp fire in lane order, the first lane's item then the second's,
    /// and the drain goes on to the next stamp: a tie never stalls it.
    #[test]
    fn equal_heads_fire_in_lane_order_and_the_drain_goes_on() {
        let mut app = app(&[]);
        let w = app.world_mut();
        let tie = stamp();
        w.resource_mut::<Toy<0>>().0.push((tie, "a"));
        w.resource_mut::<Toy<1>>().0.push((tie, "b"));
        push::<1>(w, "c");
        push::<0>(w, "d");
        assert_eq!(drain(&mut app), ["a", "b", "c", "d"]);
        assert_eq!(drain(&mut app), Vec::<&str>::new(), "nothing left behind");
    }

    /// A signal raised inside a listener runs to its end before the outer signal returns
    /// (`0x703e50`): an item pushed while another fires fires straight after it, ahead of the
    /// older items of either lane, and one pushed by that one straight after it in turn.
    #[test]
    fn an_item_pushed_by_a_firing_fires_straight_after_its_cause() {
        let mut app = app(&[
            ("a", 1, "a.1"),
            ("a", 0, "a.2"),
            ("a.1", 0, "a.1.1"),
            ("b", 0, "b.1"),
        ]);
        let w = app.world_mut();
        push::<0>(w, "a");
        push::<1>(w, "b");
        push::<0>(w, "c");
        assert_eq!(
            drain(&mut app),
            ["a", "a.1", "a.1.1", "a.2", "b", "b.1", "c"]
        );
    }

    /// What ran around the drains, in order.
    #[derive(Resource, Default)]
    struct Around(Vec<&'static str>);

    /// A lane whose log's start waits at the origin, as the views' do; whether its log raises.
    #[derive(Resource, Default)]
    struct OriginLog {
        logged: bool,
        raises: bool,
    }

    fn origin_peek(world: &World) -> Option<Stamp> {
        world.resource::<OriginLog>().logged.then(Stamp::origin)
    }

    fn origin_fire(In(_): In<Until>, mut log: ResMut<OriginLog>, mut fired: ResMut<Fired>) {
        log.logged = false;
        fired.0.push("origin");
    }

    /// The toy lanes with the origin log behind them, the hooks noting each run, and the in-game
    /// interface up.
    fn hooked() -> App {
        let mut app = app(&[]);
        app.init_resource::<OriginLog>()
            .init_resource::<Around>()
            .insert_resource(State::new(crate::client_state::ClientState::InWorld))
            .add_lane("origin", origin_peek, origin_fire, None)
            .lane_raises("origin", |world| world.resource::<OriginLog>().raises)
            .add_frame_begin(|world| world.resource_mut::<Around>().0.push("frame"))
            .add_span_start(|world| {
                let at_packets = AT_PACKETS.get();
                world.resource_mut::<Around>().0.push(if at_packets {
                    "span at packets"
                } else {
                    "span"
                });
            })
            .add_drain_end(|world| {
                let at_packets = AT_PACKETS.get();
                world.resource_mut::<Around>().0.push(if at_packets {
                    "end at packets"
                } else {
                    "end"
                });
            });
        app
    }

    fn around(app: &mut App) -> Vec<&'static str> {
        std::mem::take(&mut app.world_mut().resource_mut::<Around>().0)
    }

    /// A lane is due for a head that is an item, and for a log's start at the origin only when its
    /// log raises: a log that raises nothing waits for the next drain that fires.
    #[test]
    fn a_lane_is_due_for_an_item_or_a_raising_log() {
        let mut app = hooked();
        assert!(!due(app.world()), "nothing held");
        app.world_mut().resource_mut::<OriginLog>().logged = true;
        assert!(!due(app.world()), "a log that raises nothing");
        app.world_mut().resource_mut::<OriginLog>().raises = true;
        assert!(due(app.world()), "a log that raises");
        app.world_mut().resource_mut::<OriginLog>().raises = false;
        push::<1>(app.world_mut(), "a");
        assert!(due(app.world()), "an item");
    }

    /// A flush fires only when due and the interface is up, outside the packets' dispatch even
    /// inside it, then runs the next span's starts back in the dispatch; the frame's begins are
    /// the frame's drain's alone, and each drain runs its ends.
    #[test]
    fn a_flush_fires_when_due_then_starts_the_span() {
        let mut app = hooked();
        app.world_mut().resource_mut::<OriginLog>().logged = true;
        let (flushes, fired) = tally();
        assert!(!at_packets(|| flush(app.world_mut())), "not due");
        assert!(around(&mut app).is_empty());
        push::<0>(app.world_mut(), "a");
        app.world_mut()
            .remove_resource::<State<crate::client_state::ClientState>>();
        assert!(!flush(app.world_mut()), "no interface up");
        app.world_mut()
            .insert_resource(State::new(crate::client_state::ClientState::InWorld));
        assert!(at_packets(|| flush(app.world_mut())));
        assert_eq!(
            std::mem::take(&mut app.world_mut().resource_mut::<Fired>().0),
            ["origin", "a"],
            "the log's start and the item, in stamp order"
        );
        assert_eq!(around(&mut app), ["end", "span at packets"]);
        assert_eq!(tally(), (flushes + 1, fired + 2));
        push::<1>(app.world_mut(), "b");
        assert_eq!(drain(&mut app), ["b"]);
        assert_eq!(around(&mut app), ["frame", "end"], "the frame's drain");
    }

    /// benilla's own choice, not the reference's ([`DEPTH`]'s `Deviation:`): past four levels a
    /// pushed item is stamped fresh and waits for the next drain.
    #[test]
    fn a_chain_of_pushes_past_the_depth_waits_for_the_next_drain() {
        let mut app = app(&[
            ("a", 0, "b"),
            ("b", 0, "c"),
            ("c", 0, "d"),
            ("d", 0, "e"),
            ("e", 0, "f"),
        ]);
        push::<0>(app.world_mut(), "a");
        assert_eq!(drain(&mut app), ["a", "b", "c", "d"]);
        assert_eq!(drain(&mut app), ["e", "f"]);
    }
}
