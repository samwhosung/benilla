//! The interaction latch `[0xb4e2d0]` and the one close path every NPC window goes through.
//!
//! Each NPC window keeps its NPC's guid in a cell of its own and registers it through
//! `CGGameUI::SetInteractNPC` (`0x4930d0`, [`register`]) as it opens: a latched holder is closed
//! first, through `0x493310` ([`dispatch`]), so one window at most is latched. `0x493310` clears
//! the latch, then calls the close core of the first window, in its test order ([`NpcWindow`]),
//! whose cell holds the old guid; only that one.
//!
//! Every close core calls `0x4931c0` on its own cell ([`release`]), which runs `0x493310` while
//! the latch still names that cell's guid. So a core entered directly (a Lua close verb, a server
//! close, a leave-world leg) while its window is latched runs once more inside itself, and its
//! `*_CLOSED` fires twice; entered through `0x493310`, it finds the latch clear and fires once.
//! Quest's core zeroes its cell before the release (`0x501201`), so it never re-enters.
//!
//! The cores fire their events into the VM as they run, so a close lands ahead of the open that
//! caused it; a window's feed fires its opens and updates and starts over from closed after each
//! core run ([`InteractLatch::closes`]). A handler that calls its window's close verb, as stock
//! `HideUIPanel` → `OnHide` does, re-enters that core in the reference before the fire returns;
//! [`fire`] runs such a call before going on, as a core that fires before it clears its cell
//! (trainer, bank, auction) is re-entered with the cell still set.
//!
//! The NPC's voice rides the same two calls ([`LatchVocal`]): `0x4930d0` greets a different
//! non-player unit it latches, and `0x4931c0` says goodbye to the unit it releases, unless its
//! caller's `dl` is set. A swap releases with `dl` set, and the watchdog and the leave-world sweep
//! call `0x493310` directly, so neither says goodbye.

use bevy::prelude::*;

use benilla_ui::script::UiScript;

/// The windows `0x493310` tests, in its order (`0x493341` to `0x4934de`): the first whose cell
/// holds the latched guid is closed. The tail cell `[0xb71b48]` closes through a bare `ret`
/// (`0x4c3f40`) and has no window here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum NpcWindow {
    /// `[0xb71728]` → `0x4bf4e0(0, 0)`.
    Trade,
    /// `[0xbc3fa0]` → `0x4e32e0(0, 0)`.
    ItemText,
    /// `[0xbc3f58]` → `0x4e1fa0(0)`.
    Gossip,
    /// `[0xbe0810]` → `0x501130(0, 1)`.
    Quest,
    /// `[0xbddfa0]` → `0x4fadf0`.
    Merchant,
    /// `[0xbb4ab8]` → `0x4dc0f0`.
    Taxi,
    /// `[0xb73a00]` → `0x4d7440(0)`.
    Trainer,
    /// `[0xbdd038]` → `0x4f84d0(0)`.
    Bank,
    /// `[0xbdceb0]` → `0x4f5010`.
    Registrar,
    /// `[0xbdcee8]` → `0x4f58a0`.
    Tabard,
    /// `[0xb6ef88]` → `0x4acd50(cl=0)`.
    Mail,
    /// `[0xb725f8]` → `0x4cd5f0`.
    Auction,
    /// `[0xb72240]` → `0x4cae10`.
    Stable,
}

impl NpcWindow {
    /// Every window, in `0x493310`'s order.
    pub(crate) const ALL: [NpcWindow; 13] = [
        NpcWindow::Trade,
        NpcWindow::ItemText,
        NpcWindow::Gossip,
        NpcWindow::Quest,
        NpcWindow::Merchant,
        NpcWindow::Taxi,
        NpcWindow::Trainer,
        NpcWindow::Bank,
        NpcWindow::Registrar,
        NpcWindow::Tabard,
        NpcWindow::Mail,
        NpcWindow::Auction,
        NpcWindow::Stable,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

/// What the latch asks the latched NPC to say. The unit's kind and state gate it where it is
/// played (`0x60c3b0`): a player, a game object or a book speaks nothing.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LatchVocal {
    /// A different unit latched (`0x4930d0` → `0x49318f`, `0x60c3b0` with 1).
    Hello(u64),
    /// The latch released to nothing with `dl` clear (`0x4931c0` → `0x493210`, `0x60c3b0` with 0).
    Goodbye(u64),
}

/// How deep [`fire`]'s answers to close verbs may nest. The reference recurses without bound, so
/// a handler that calls its own close verb on every fire overflows its stack; three levels cover
/// every stock route.
const MAX_REENTRY: u32 = 8;

/// The interaction latch, `[0xb4e2d0]`: the guid the open NPC window registered, until a close
/// core releases it. A UI shutdown leaves it to the leave-world sweep; a disconnect clears it.
#[derive(Resource, Default)]
pub(crate) struct InteractLatch {
    guid: Option<u64>,
    /// Per window, how many times its core cleared its cell.
    closes: [u64; NpcWindow::ALL.len()],
    /// [`fire`]'s nesting, against a handler that re-closes forever.
    depth: u32,
}

impl InteractLatch {
    /// The latched guid.
    pub(crate) fn guid(&self) -> Option<u64> {
        self.guid
    }

    /// How many times `window`'s close core has cleared its cell: the core told the VM, so a feed
    /// that sees this move forgets what it last announced and starts over from closed.
    pub(crate) fn closes(&self, window: NpcWindow) -> u64 {
        self.closes[window.index()]
    }

    /// Latch `guid` with no close, for a test that seats a window directly.
    #[cfg(test)]
    pub(crate) fn latch_for_test(&mut self, guid: u64) {
        self.guid = Some(guid);
    }
}

/// A window feed's view of its core: the feed fires its window's opens and updates, never its
/// close, which the core told the VM as it ran.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CoreCloses<'w, 's> {
    /// Absent in a harness with no window on the latch.
    latch: Option<Res<'w, InteractLatch>>,
    seen: crate::ui_script::WindowMemo<'w, 's, u64>,
}

impl CoreCloses<'_, '_> {
    /// Whether `window`'s core cleared its cell since this feed last asked: what the feed last
    /// announced is gone, and it starts over from closed.
    pub(crate) fn since_last(&mut self, script: &UiScript, window: NpcWindow) -> bool {
        let now = self.latch.as_ref().map_or(0, |l| l.closes(window));
        std::mem::replace(self.seen.get(script), now) != now
    }
}

/// A window on the latch: its cell, its close core as `0x493310` calls it, and its Lua close
/// verb.
pub(crate) trait LatchedWindow: Resource + Sized {
    const WINDOW: NpcWindow;
    /// The guid in the window's cell; `None` when it is closed.
    fn cell(&self) -> Option<u64>;
    /// The close core as `0x493310` enters it.
    fn dispatched(world: &mut World);
    /// Take the window's Lua close verb, if a handler called it.
    fn take_close_verb(script: &mut UiScript) -> bool;
    /// What the close verb does: its binding's call into the core.
    fn close_verb(world: &mut World) {
        Self::dispatched(world);
    }
}

/// One registered window, type-erased for the dispatcher.
#[derive(Clone, Copy)]
struct Entry {
    window: NpcWindow,
    cell: fn(&World) -> Option<u64>,
    dispatched: fn(&mut World),
    take_close_verb: fn(&mut UiScript) -> bool,
    close_verb: fn(&mut World),
}

/// The windows on the latch, in `0x493310`'s order.
#[derive(Resource, Default)]
struct LatchedWindows(Vec<Entry>);

/// Registering a window on the latch.
pub(crate) trait LatchApp {
    fn add_latched_window<T: LatchedWindow>(&mut self) -> &mut Self;
}

impl LatchApp for App {
    fn add_latched_window<T: LatchedWindow>(&mut self) -> &mut Self {
        let world = self.world_mut();
        world.init_resource::<InteractLatch>();
        let mut windows = world.get_resource_or_init::<LatchedWindows>();
        if windows.0.iter().all(|e| e.window != T::WINDOW) {
            windows.0.push(Entry {
                window: T::WINDOW,
                cell: |world| world.get_resource::<T>().and_then(T::cell),
                dispatched: T::dispatched,
                take_close_verb: T::take_close_verb,
                close_verb: T::close_verb,
            });
            windows.0.sort_by_key(|e| e.window);
        }
        self
    }
}

fn entries(world: &World) -> Vec<Entry> {
    world
        .get_resource::<LatchedWindows>()
        .map(|w| w.0.clone())
        .unwrap_or_default()
}

/// `CGGameUI::SetInteractNPC` (`0x4930d0`), at a window's open: with anything latched, `0x4931c0`
/// on the latch itself with `dl` set (`0x49310a`, `dl = guid != 0`), which always matches and
/// runs `0x493310` with no goodbye, so the holder closes first, even when it is this window on this
/// same guid; then `guid` is latched (`0x493114`). A guid that differs from the one latched
/// before is greeted (`0x49318f`, `bl` clear), so a second window on the same unit is silent. The
/// opener writes its cell after this, except the tabard designer's.
pub(crate) fn register(world: &mut World, guid: u64) {
    world.init_resource::<InteractLatch>();
    let before = world.resource::<InteractLatch>().guid;
    if before.is_some() {
        dispatch(world);
    }
    world.resource_mut::<InteractLatch>().guid = Some(guid);
    if guid != 0 && before != Some(guid) {
        say(world, LatchVocal::Hello(guid));
    }
}

/// `0x4931c0` on a core's own cell with `dl` clear: while the latch names `cell`, the goodbye
/// (`0x493210`), then `0x493310`.
pub(crate) fn release(world: &mut World, cell: u64) {
    release_cell(world, cell, true);
}

/// `0x4931c0(cell, dl = !goodbye)`: while the latch names `cell`, the unit's goodbye unless `dl`
/// is set (`0x493208`), then `0x493310` whether or not the object is still there (`0x49321e`).
pub(crate) fn release_cell(world: &mut World, cell: u64, goodbye: bool) {
    let latched = world
        .get_resource::<InteractLatch>()
        .and_then(|l| l.guid)
        .filter(|g| *g == cell);
    if latched.is_some() {
        if goodbye {
            say(world, LatchVocal::Goodbye(cell));
        }
        dispatch(world);
    }
}

/// `0x4931c0` on the latch itself (`ecx = 0xb4e2d0`): whatever it holds is released, with the
/// goodbye unless `goodbye` is false.
pub(crate) fn release_latch(world: &mut World, goodbye: bool) {
    if let Some(guid) = world.get_resource::<InteractLatch>().and_then(|l| l.guid) {
        release_cell(world, guid, goodbye);
    }
}

/// Queue the NPC's line for whoever plays it; nothing reads it in a build with no sound.
fn say(world: &mut World, vocal: LatchVocal) {
    if let Some(mut messages) = world.get_resource_mut::<Messages<LatchVocal>>() {
        messages.write(vocal);
    }
}

/// `0x493310`: clear the latch (`0x49334b`), then run the close core of the first window whose
/// cell holds the old guid. The watchdog (`0x493305`) and the leave-world sweep (`0x490b5f`)
/// call it, and so does every core that finds its own guid latched.
pub(crate) fn dispatch(world: &mut World) {
    let Some(guid) = world
        .get_resource_mut::<InteractLatch>()
        .and_then(|mut l| l.guid.take())
    else {
        return;
    };
    if let Some(e) = entries(world)
        .into_iter()
        .find(|e| (e.cell)(world) == Some(guid))
    {
        (e.dispatched)(world);
    }
}

/// A core cleared `window`'s cell: its feed starts over from closed.
pub(crate) fn note_closed(world: &mut World, window: NpcWindow) {
    world.init_resource::<InteractLatch>();
    world.resource_mut::<InteractLatch>().closes[window.index()] += 1;
}

/// Run `f` on the VM, if one is mounted, with no event: a core's own pushes. The host is lent all
/// the same, as for every entry that reaches the VM ([`crate::script_calls`]).
pub(crate) fn with_script(world: &mut World, f: impl FnOnce(&mut UiScript)) {
    crate::script_calls::with_hosted_vm(world, |script| {
        if let Some(script) = script {
            f(script);
        }
    });
}

/// A core's event: `f` fires it into the VM with the host lent, so a handler's script calls apply
/// inside their verbs; then every close verb its handlers called runs, in `0x493310`'s order, as
/// the reference's verb would have inside the fire. With no VM there is no handler to call one.
pub(crate) fn fire(world: &mut World, f: impl FnOnce(&mut UiScript)) {
    let fired = crate::script_calls::with_hosted_vm(world, |script| script.map(f).is_some());
    if !fired {
        return;
    }
    let depth = {
        let mut latch = world.get_resource_or_init::<InteractLatch>();
        latch.depth += 1;
        latch.depth
    };
    if depth <= MAX_REENTRY {
        for e in entries(world) {
            let called = world
                .get_non_send_resource_mut::<UiScript>()
                .is_some_and(|mut s| (e.take_close_verb)(&mut s));
            if called {
                (e.close_verb)(world);
            }
        }
    }
    world.resource_mut::<InteractLatch>().depth -= 1;
}

/// Queue `f` to run on the world at the next command flush: how a system reaches a core.
pub(crate) fn queue(commands: &mut Commands, f: fn(&mut World)) {
    commands.queue(move |world: &mut World| f(world));
}

/// The watchdog `0x493230`, every frame: the latched object out of its leash, or gone, closes
/// through `0x493310` (`0x493305`). An item is never leashed (`0x4932b5`), so a quest panel or a
/// reader opened from a bag stays.
pub(crate) fn watch_interaction_range(
    latch: Res<InteractLatch>,
    index: Res<crate::net::GuidIndex>,
    self_q: Query<&Transform, With<crate::net::SelfPlayer>>,
    transforms: Query<&Transform>,
    mut commands: Commands,
) {
    let Some(guid) = latch.guid else { return };
    if benilla_protocol::guid::is_item(guid) {
        return;
    }
    let Some(me) = self_q.iter().next() else {
        return;
    };
    // Centre to centre with no bounding radii, and `dist² <= leash²` keeps: the reference's
    // compare (`0x4932f2`, `0x4932fe`).
    let out = match index.0.get(&guid).and_then(|e| transforms.get(*e).ok()) {
        Some(tf) => tf.translation.distance_squared(me.translation) > super::leash_sq(guid),
        None => true,
    };
    if out {
        debug!("ui_session: latched {guid:#x} out of range or gone — 0x493310");
        queue(&mut commands, dispatch);
    }
}

/// The connection's end takes the latch with it, silently: each window clears its own cell.
pub(crate) fn clear_latch_on_session_end(
    In(_): In<benilla_protocol::SessionEvent>,
    mut latch: ResMut<InteractLatch>,
) {
    latch.guid = None;
}
