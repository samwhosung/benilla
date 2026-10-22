//! The lanes: the UI events the packet handlers raise, fired in the order the packets came. The
//! reference's poll takes the queued packets in order (`0x5384d0`, `0x538529`) and runs each
//! handler whole (`0x537aa0`, `0x537ae5`), and a handler's events fire inside it, every listener
//! before the signal returns (`0x703e50`, `0x703f50`). Here a handler queues what it raises on a
//! lane, a producer's queue that stamps each item from one clock as it is pushed ([`stamp`]).
//! Once every feed has pushed what the events' listeners read, as the reference writes a whole
//! update block before its notify pass (`0x465277`), [`fire_in_order`] fires the lowest stamp
//! across the lanes, so the events come out in the order their handlers raised them, whichever
//! lane holds them.
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
/// waits for the next drain, so a listener that queues an item each time it hears one cannot
/// hold the frame.
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

    /// The count of the outermost push: the drain fires only what was stamped before it began.
    fn first(&self) -> u64 {
        self.path[0]
    }
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
}

/// The next stamp: a fresh one, or with an item firing, one nested under it.
pub(crate) fn stamp() -> Stamp {
    let n = CLOCK.fetch_add(1, Ordering::Relaxed);
    match FIRING.get() {
        Some(cause) if usize::from(cause.len) < DEPTH => cause.child(n),
        _ => Stamp::root(n),
    }
}

/// Run `f` as the firing of the item stamped `at`, so what it pushes nests under the item; with
/// whether anything was stamped meanwhile, after which the drain takes the lowest stamp again.
pub(crate) fn firing<R>(at: Stamp, f: impl FnOnce() -> R) -> (R, bool) {
    let before = CLOCK.load(Ordering::Relaxed);
    let outer = FIRING.replace(Some(at));
    let r = f();
    FIRING.set(outer);
    (r, CLOCK.load(Ordering::Relaxed) != before)
}

/// What a lane's firing may fire: the stamps below `bound`, the lowest of the other lanes' heads,
/// among those whose outermost push came before the drain began.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Until {
    bound: Option<Stamp>,
    fence: u64,
}

impl Until {
    /// Everything stamped so far, for a lane fired alone.
    #[cfg(test)]
    pub(crate) fn all() -> Self {
        Self {
            bound: None,
            fence: CLOCK.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn admits(&self, stamp: Stamp) -> bool {
        stamp.first() < self.fence && self.bound.is_none_or(|b| stamp < b)
    }
}

/// One lane: its name, its head, its firing (a one-shot taking [`Until`], the `NetHandlers`
/// shape), and what it does once ahead of each drain.
struct Lane {
    name: &'static str,
    peek: fn(&World) -> Option<Stamp>,
    fire: SystemId<In<Until>>,
    begin: Option<fn(&World)>,
}

/// Every lane, in registration order.
#[derive(Resource, Default)]
pub(crate) struct Lanes(
    Vec<Lane>,
    /// Each lane's firing's declared access, for the tests that ask what the drain touches.
    #[cfg(test)]
    pub(crate) Vec<bevy::ecs::query::FilteredAccessSet>,
);

impl Lanes {
    /// The lanes' names, for the tests.
    #[cfg(test)]
    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.0.iter().map(|l| l.name).collect()
    }
}

/// The drain's place: after every feed in [`crate::ui_script::UiFeed`] has pushed, and before
/// the tick ([`crate::ui_script::UiInput`]).
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
            self.init_resource::<Lanes>()
                .configure_sets(
                    Update,
                    UiLanes
                        .after(crate::ui_script::UiFeed)
                        .before(crate::ui_script::UiInput),
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
        // Initialized here, its access kept: the run initializes it again, on the same state.
        #[cfg(test)]
        let (fire, access) = {
            let mut system = IntoSystem::into_system(fire);
            let access = bevy::ecs::system::System::initialize(&mut system, self.world_mut());
            (system, access)
        };
        let fire = self.world_mut().register_system(fire);
        #[cfg(test)]
        self.world_mut().resource_mut::<Lanes>().1.push(access);
        self.world_mut().resource_mut::<Lanes>().0.push(Lane {
            name,
            peek,
            fire,
            begin,
        });
        self
    }
}

/// Fire every lane's items in stamp order: each lane's `begin`, then, while any lane's head was
/// stamped before the drain began, the lane holding the lowest fires up to the next lane's head,
/// its Lua run with the call host lent. A lane that fires nothing (no VM) ends the drain, its
/// items waiting for the next.
pub(crate) fn fire_in_order(world: &mut World) {
    let Some(lanes) = world.remove_resource::<Lanes>() else {
        return;
    };
    for begin in lanes.0.iter().filter_map(|l| l.begin) {
        begin(world);
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
        let bound = heads
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .filter_map(|(_, h)| *h)
            .min();
        let lane = &lanes.0[i];
        if let Err(e) = world.run_system_with(lane.fire, Until { bound, fence }) {
            error!("lanes: `{}` did not fire: {e}", lane.name);
            break;
        }
        crate::creature_anim::apply_unit_edits(world);
        if (lane.peek)(world) == Some(head) {
            break;
        }
    }
    world.insert_resource(lanes);
}

#[cfg(test)]
mod guard_tests;
#[cfg(test)]
mod tests;

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
