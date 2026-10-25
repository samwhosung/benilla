//! Our cast's state as each packet left it. The reference's cast state is its globals: the
//! in-flight slot (`0xceca88`, its item at `0xcecab0`), the targeting word and its record's spell
//! (`0xceac58`), the auto-repeat key (`0xceac30`) and the attack flag (`[player+0xc48]`). Each
//! packet's handler writes them and signals inside itself (our cast's `SMSG_CAST_RESULT` pops the
//! slot, `0x6e741a` → `0x6e4940` → `0x6e4ad0`, before its `SPELLCAST_STOP`), and the getters read
//! them inside the call (`IsCurrentAction` `0x4e53a0`, `IsAutoRepeatAction` `0x4e55b0` →
//! `0x6e9fd0`, `SpellIsTargeting` `0x6e6cd0`, `SpellStopCasting` `0x6e6e80`), so a listener reads
//! its own packet's. Here every handler of the frame runs before any listener, so the view each
//! packet left is logged, stamped ahead of what the packet raised ([`CastViews`]); the views lane
//! pushes the frame's start, then each packet's view at its place among the other lanes' items.
//! A listener's call that moves the state moves it live, after every handler, so what it moved
//! is the live state's from then on: each later view carries the call's fields as they stand
//! live ([`Moved`]), and the rest as its packet left them.

use std::collections::VecDeque;

use bevy::ecs::system::SystemState;
use bevy::prelude::*;

use benilla_ui::script::{CastView, UiScript};

use crate::lanes::{Stamp, Until};
use crate::spell::CastState;

/// The cast views the frame's packets left, and what the VM was last given of the live state.
#[derive(Resource, Default)]
pub(crate) struct CastViews {
    /// The view as the frame's first logged packet found it: what the drain begins from.
    start: Option<CastView>,
    /// Each packet's view that differs from the one before it, stamped ahead of the packet.
    entries: VecDeque<(Stamp, CastView)>,
    /// The view as the last packet left it, or as the net stage began.
    last: Option<CastView>,
    /// The drain has pushed the start.
    started: bool,
    /// The live view the VM last took, from the frame's feed or a call: a call that leaves it as
    /// it stands pushes nothing, so the drain's view stays its listener's packet's.
    live: Option<CastView>,
    /// The view the VM holds.
    shown: Option<CastView>,
    /// The fields the drain's calls moved, which every later view takes from the live state.
    called: Moved,
}

/// The fields of a view that moved between two of them, the in-flight item with its slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Moved {
    slot: bool,
    targeting: bool,
    auto_repeat: bool,
    attacking: bool,
    form: bool,
}

impl Moved {
    /// Every field.
    const ALL: Self = Self {
        slot: true,
        targeting: true,
        auto_repeat: true,
        attacking: true,
        form: true,
    };

    /// What differs between `a` and `b`.
    fn between(a: &CastView, b: &CastView) -> Self {
        Self {
            slot: a.slot != b.slot || a.item != b.item,
            targeting: a.targeting != b.targeting,
            auto_repeat: a.auto_repeat != b.auto_repeat,
            attacking: a.attacking != b.attacking,
            form: a.form != b.form,
        }
    }

    /// The fields either moved.
    fn or(self, o: Self) -> Self {
        Self {
            slot: self.slot || o.slot,
            targeting: self.targeting || o.targeting,
            auto_repeat: self.auto_repeat || o.auto_repeat,
            attacking: self.attacking || o.attacking,
            form: self.form || o.form,
        }
    }

    /// `base` with these fields as `live` holds them.
    fn over(self, base: CastView, live: CastView) -> CastView {
        let mut v = base;
        if self.slot {
            (v.slot, v.item) = (live.slot, live.item);
        }
        if self.targeting {
            v.targeting = live.targeting;
        }
        if self.auto_repeat {
            v.auto_repeat = live.auto_repeat;
        }
        if self.attacking {
            v.attacking = live.attacking;
        }
        if self.form {
            v.form = live.form;
        }
        v
    }
}

impl CastViews {
    /// The VM takes the live `view` after a call ([`crate::script_calls`]) where it moved since
    /// the VM last took the live state: inside the drain, the fields that moved, over the view the
    /// VM holds, which later views take from the live state too; else the whole. A listener that
    /// cast nothing reads its own packet's view.
    pub(crate) fn commit(&mut self, script: &mut UiScript, view: CastView) {
        if self.live == Some(view) {
            return;
        }
        let moved = self.live.map_or(Moved::ALL, |l| Moved::between(&l, &view));
        self.live = Some(view);
        match self.shown.filter(|_| self.started) {
            Some(shown) => {
                self.called = self.called.or(moved);
                self.show(script, moved.over(shown, view));
            }
            None => self.show(script, view),
        }
    }

    /// `view` into the VM: the cast state the checked tests and `SpellStopCasting` read, and the
    /// targeting word `SpellIsTargeting` tests, up while the view holds the record's spell.
    fn show(&mut self, script: &mut UiScript, view: CastView) {
        script.set_cast_view(view);
        script.set_spell_targeting(view.targeting.is_some());
        self.shown = Some(view);
    }

    /// `view` logged at `at`, when it differs from the one before it.
    fn log(&mut self, at: Stamp, view: CastView) {
        let Some(last) = self.last else {
            return;
        };
        if last == view {
            return;
        }
        if self.entries.is_empty() && !self.started && self.start.is_none() {
            self.start = Some(last);
        }
        let i = self.entries.partition_point(|(s, _)| *s < at);
        self.entries.insert(i, (at, view));
        self.last = Some(view);
    }

    /// A logged `view` into the VM, with the fields the drain's calls moved as they stand live.
    fn replay(&mut self, script: &mut UiScript, view: CastView) {
        let view = self.called.over(view, self.live.unwrap_or(view));
        self.show(script, view);
    }
}

/// `view` into the VM with no log kept, in a harness without one.
fn push(script: &mut UiScript, view: CastView) {
    script.set_cast_view(view);
    script.set_spell_targeting(view.targeting.is_some());
}

/// The live view off the world, through a reader built once.
#[derive(Resource)]
struct ViewReader(SystemState<Option<CastState<'static, 'static>>>);

/// The live view, `None` in a harness without the cast state.
fn live_view(world: &mut World) -> Option<CastView> {
    let mut reader = world.remove_resource::<ViewReader>()?;
    let view = reader.0.get(world).as_ref().map(CastState::view);
    world.insert_resource(reader);
    view
}

thread_local! {
    /// Where the packet being handled moves the in-flight slot, past what it raised before
    /// ([`slot_moves_here`]).
    static SLOT_MOVES: std::cell::Cell<Option<Stamp>> = const { std::cell::Cell::new(None) };
}

/// The packet being handled moves the in-flight slot here, after what its handler raised so far:
/// a failed `SMSG_CAST_RESULT`'s report fires the GCD clear's pair (`0x6e1d89` → `0x6e167d`)
/// before it pops the slot (`0x6e2218`), so that pair's listeners read the cast still in flight.
/// The packet's view then lands in two steps: everything but the slot and the targeting word,
/// which `CancelCast 0x6e4940` drops with it (`0x6e4955`), ahead of the packet, and those here.
pub(crate) fn slot_moves_here() {
    SLOT_MOVES.set(Some(crate::lanes::stamp()));
}

/// The net stage's start: the view as the frame's input left it, which the first packet that
/// moves it is compared with.
fn packets_begin(world: &mut World) {
    SLOT_MOVES.set(None);
    let view = live_view(world);
    if let Some(mut views) = world.get_resource_mut::<CastViews>() {
        views.last = view;
    }
}

/// A packet handled: its view logged at `at`, the stamp taken ahead of its handler, when it
/// differs from the one before it; with its slot at the stamp its handler moved it, when it
/// named one ([`slot_moves_here`]).
fn packet_end(world: &mut World, at: Stamp) {
    let moves = SLOT_MOVES.take().filter(|s| *s > at);
    let Some(view) = live_view(world) else {
        return;
    };
    let Some(mut views) = world.get_resource_mut::<CastViews>() else {
        return;
    };
    let Some(last) = views.last else {
        return;
    };
    if let Some(moves) = moves {
        // `CancelCast 0x6e4940` drops the targeting word (`0x6e4955`) and pops the slot
        // (`0x6e49d6`) at the pop's place.
        let ahead = CastView {
            slot: last.slot,
            item: last.item,
            targeting: last.targeting,
            ..view
        };
        views.log(at, ahead);
        views.log(moves, view);
    } else {
        views.log(at, view);
    }
}

/// The lane's head: its start, at the origin, while the frame logged a view; then its next entry.
fn peek(world: &World) -> Option<Stamp> {
    let views = world.get_resource::<CastViews>()?;
    if views.started {
        views.entries.front().map(|(s, _)| *s)
    } else {
        (!views.entries.is_empty()).then(Stamp::origin)
    }
}

/// The lane's firing: the frame's start pushed once a drain, then each view `until` admits, which
/// raise nothing.
fn fire_cast_views(
    In(until): In<Until>,
    mut views: ResMut<CastViews>,
    mut script: Option<NonSendMut<UiScript>>,
) {
    let views = &mut *views;
    if !views.started {
        views.started = true;
        if let (Some(start), Some(script)) = (views.start.take(), script.as_deref_mut()) {
            views.replay(script, start);
        }
    }
    while views.entries.front().is_some_and(|(s, _)| until.admits(*s)) {
        let Some((_, view)) = views.entries.pop_front() else {
            break;
        };
        if let Some(script) = script.as_deref_mut() {
            views.replay(script, view);
        }
    }
}

/// The drain's end, however it ended: the VM given the live view, the log emptied.
fn end_cast_views(world: &mut World) {
    if !world.get_resource::<CastViews>().is_some_and(|v| v.started) {
        return;
    }
    let view = live_view(world);
    world.resource_scope(|world, mut views: Mut<CastViews>| {
        views.entries.clear();
        views.start = None;
        views.started = false;
        views.called = Moved::default();
        let Some(view) = view else {
            return;
        };
        if let Some(mut script) = world.get_non_send_resource_mut::<UiScript>() {
            views.show(&mut script, view);
        }
        views.live = Some(view);
    });
}

/// What the packets moved while no in-game interface is up, left unpushed: no VM reads it.
fn drop_cast_views(mut views: ResMut<CastViews>) {
    let (live, shown) = (views.live, views.shown);
    *views = CastViews {
        live,
        shown,
        ..Default::default()
    };
}

/// The frame's feed of the live view, which the drain's start replaces while the frame logged
/// any ([`super::feed_cast_reports`]).
pub(super) fn feed_live(script: &mut UiScript, views: Option<&mut CastViews>, view: CastView) {
    match views {
        Some(views) => {
            views.show(script, view);
            views.live = Some(view);
        }
        None => push(script, view),
    }
}

/// The log, its packet hooks, the lane, its end and the drop under the exact complement of the
/// drain's gate.
pub(super) fn add_cast_views_lane(app: &mut App) {
    use crate::lanes::LaneApp;
    let reader = ViewReader(SystemState::new(app.world_mut()));
    app.init_resource::<CastViews>()
        .insert_resource(reader)
        .add_lane("cast views", peek, fire_cast_views, None)
        .add_drain_end(end_cast_views)
        .add_systems(
            Update,
            drop_cast_views
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
    crate::net::handlers::add_packet_hook(app, packets_begin, packet_end);
}
