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
//! A core run where its window's lane still holds an earlier item, its open among them, clears
//! its cell at once and queues its signals behind that item, so a close never lands ahead of the
//! open it closes; else it fires them into the VM as it runs. A handler that calls its window's close verb, as stock
//! `HideUIPanel` → `OnHide` does, re-enters that core in the reference before the fire returns;
//! every close verb runs its core inside its call, through the host ([`LatchSteps`]), so a core
//! that fires before it clears its cell (trainer, bank, auction) is re-entered with the cell still
//! set.
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
    pub(crate) const ORDER: [Self; 13] = [
        Self::Trade,
        Self::ItemText,
        Self::Gossip,
        Self::Quest,
        Self::Merchant,
        Self::Taxi,
        Self::Trainer,
        Self::Bank,
        Self::Registrar,
        Self::Tabard,
        Self::Mail,
        Self::Auction,
        Self::Stable,
    ];

    /// The window's bit among [`PARKED_CLOSES`].
    const fn bit(self) -> u16 {
        1 << self as u16
    }
}

thread_local! {
    /// The windows whose parked close is running its interface half on this thread
    /// ([`in_parked_close`]).
    static PARKED_CLOSES: std::cell::Cell<u16> = const { std::cell::Cell::new(0) };
}

/// Run `f`, the interface half of `window`'s close that a handler or a lane's earlier item held
/// back ([`crate::lanes::behind`]): its signals, and the VM's view of the window, at the close's
/// place on its lane. The engine half, the cell, the latch and the sends, ran where the close was
/// entered, as the reference runs them inside the handler, so the live window may by now be a
/// session a later open began. A close verb the half's listeners re-enter ([`parked_close`])
/// therefore acts on the session this close closed, never on that newer one.
pub(crate) fn in_parked_close<R>(window: NpcWindow, f: impl FnOnce() -> R) -> R {
    struct Reset(u16);
    impl Drop for Reset {
        fn drop(&mut self) {
            PARKED_CLOSES.set(self.0);
        }
    }
    let outer = PARKED_CLOSES.get();
    let _reset = Reset(outer);
    PARKED_CLOSES.set(outer | window.bit());
    f()
}

/// Whether `window`'s parked close is running its interface half ([`in_parked_close`]): its close
/// verb, re-entered from the half's listeners, runs only what the reference's re-entry runs at that
/// point of the core against the session the close closed, its signals, and leaves the live
/// session and the wire alone.
pub(crate) fn parked_close(window: NpcWindow) -> bool {
    PARKED_CLOSES.get() & window.bit() != 0
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

/// The interaction latch, `[0xb4e2d0]`: the guid the open NPC window registered, until a close
/// core releases it. A UI shutdown leaves it to the leave-world sweep; a disconnect clears it.
#[derive(Resource, Default)]
pub(crate) struct InteractLatch {
    guid: Option<u64>,
    /// The guids [`LatchSteps::release`] released inside a call, for the world's check after it
    /// ([`check_call_releases`]).
    #[cfg(debug_assertions)]
    released: Vec<u64>,
}

impl InteractLatch {
    /// The latched guid.
    pub(crate) fn guid(&self) -> Option<u64> {
        self.guid
    }

    /// Latch `guid` with no close, for a test that seats a window directly.
    #[cfg(test)]
    pub(crate) fn latch_for_test(&mut self, guid: u64) {
        self.guid = Some(guid);
    }
}

/// The latch inside a script call: the engine half a close verb's core reaches it by
/// ([`crate::script_calls::ScriptCallHost`]), so the release, the latch's clear and the core's
/// signal all happen before the verb returns, as `0x4931c0` and `0x493310` run inside the core
/// that calls them.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct LatchSteps<'w> {
    /// Absent in a harness with no window on the latch.
    latch: Option<ResMut<'w, InteractLatch>>,
    vocal: Option<ResMut<'w, Messages<LatchVocal>>>,
}

impl LatchSteps<'_> {
    /// `0x4931c0(cell, dl = 0)` from a window's close core: while the latch names `cell`, the
    /// goodbye (`0x493210`), then `0x493310`, which clears the latch (`0x49334b`) and closes the
    /// first window in its order whose cell holds the guid: the caller's own, whose core then
    /// re-enters while its cell is still set, so its signal fires twice. Whether it re-enters. No
    /// other cell holds a latched guid: every opener registers before it writes its cell
    /// ([`register`] closes the holder first), and the two that write first, the designer's and
    /// the item text's, are closed by that register, or hold an item's or an object's guid. A debug
    /// build checks it, as for every walk ([`debug_assert_one_holder`]), once the call is done
    /// ([`check_call_releases`]): this cannot read the other windows' cells beside the host.
    pub(crate) fn release(&mut self, cell: u64) -> bool {
        let Some(latch) = self.latch.as_deref_mut() else {
            return false;
        };
        if latch.guid != Some(cell) {
            return false;
        }
        if let Some(vocal) = self.vocal.as_deref_mut() {
            vocal.write(LatchVocal::Goodbye(cell));
        }
        latch.guid = None;
        #[cfg(debug_assertions)]
        latch.released.push(cell);
        true
    }

    /// `0x4930d0`'s walk inside a call that opens: the latched guid, taken (`0x49334b`), whose
    /// holder the caller closes next ([`register`]'s), and checked once the call is done as a
    /// release is ([`check_call_releases`]).
    pub(crate) fn take(&mut self) -> Option<u64> {
        let latch = self.latch.as_deref_mut()?;
        let old = latch.guid.take()?;
        #[cfg(debug_assertions)]
        latch.released.push(old);
        Some(old)
    }

    /// `0x4930d0`'s tail inside a call that opens: `guid` latched (`0x493114`), and greeted when
    /// it differs from `before`, the guid latched ahead of the walk (`0x49318f`).
    pub(crate) fn latch(&mut self, guid: u64, before: Option<u64>) {
        let Some(latch) = self.latch.as_deref_mut() else {
            return;
        };
        latch.guid = Some(guid);
        if guid != 0 && before != Some(guid) {
            if let Some(vocal) = self.vocal.as_deref_mut() {
                vocal.write(LatchVocal::Hello(guid));
            }
        }
    }
}

/// A window on the latch: its cell, and its close core as `0x493310` calls it. Its Lua close verb
/// runs the core inside its call ([`LatchSteps`]).
pub(crate) trait LatchedWindow: Resource + Sized {
    const WINDOW: NpcWindow;
    /// The guid in the window's cell; `None` when it is closed.
    fn cell(&self) -> Option<u64>;
    /// The close core as `0x493310` enters it.
    fn dispatched(world: &mut World);
}

/// One registered window, type-erased for the dispatcher.
#[derive(Clone, Copy)]
struct Entry {
    window: NpcWindow,
    cell: fn(&World) -> Option<u64>,
    dispatched: fn(&mut World),
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
        if !self.world().contains_resource::<LatchedWindows>() {
            self.add_systems(Last, check_call_releases);
        }
        let world = self.world_mut();
        world.init_resource::<InteractLatch>();
        let mut windows = world.get_resource_or_init::<LatchedWindows>();
        if windows.0.iter().all(|e| e.window != T::WINDOW) {
            windows.0.push(Entry {
                window: T::WINDOW,
                cell: |world| world.get_resource::<T>().and_then(T::cell),
                dispatched: T::dispatched,
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
        // The designer and the book write their cell first, so one may hold the guid it replaces.
        walk(world, 1);
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
    walk(world, 0);
}

/// [`dispatch`], then the check that at most `left` windows still hold the guid it released.
fn walk(world: &mut World, left: usize) {
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
    debug_assert_one_holder(world, guid, left);
}

/// What [`LatchSteps::release`] rests on, checked where every window's cell is readable: one
/// window's cell at most holds a latched guid, so a core that releases the latch without
/// `0x493310`'s walk strands no other window. After a walk closed the first holder, no cell holds
/// the released guid but, inside an open, the opener's own when it wrote its cell first.
fn debug_assert_one_holder(world: &World, guid: u64, left: usize) {
    if cfg!(debug_assertions) {
        let holders: Vec<NpcWindow> = entries(world)
            .into_iter()
            .filter(|e| (e.cell)(world) == Some(guid))
            .map(|e| e.window)
            .collect();
        debug_assert!(
            holders.len() <= left,
            "windows {holders:?} still hold the released guid {guid:#x}"
        );
    }
}

/// [`debug_assert_one_holder`] for each guid a call's core released without the walk
/// ([`LatchSteps::release`]), where every cell is readable: none still holds it, unless the latch
/// took it again since, as an opener's register does.
pub(crate) fn check_call_releases(world: &mut World) {
    #[cfg(debug_assertions)]
    {
        let Some(mut latch) = world.get_resource_mut::<InteractLatch>() else {
            return;
        };
        let released = std::mem::take(&mut latch.released);
        let latched = latch.guid;
        for guid in released {
            if latched != Some(guid) {
                debug_assert_one_holder(world, guid, 0);
            }
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = world;
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

/// A core's event: `f` fires it into the VM with the host lent, so a handler's script calls,
/// close verbs among them, apply inside their verbs. With no VM there is no handler.
pub(crate) fn fire(world: &mut World, f: impl FnOnce(&mut UiScript)) {
    with_script(world, f);
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
