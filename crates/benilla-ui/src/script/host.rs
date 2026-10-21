//! The lent call host: the engine half of the script calls, which every entry that runs Lua lends
//! the VM for the length of it ([`UiScript::hosted`]), so a verb's call applies inside the verb as
//! the reference's does, whatever entry the script came in by. `CastSpellByName` (`0x4b4ab0`)
//! reaches `ArmCastTargets 0x6e5250` through direct calls, so `SpellIsTargeting()` (`0x6e6cd0`) on
//! the script's next line reads the armed word. The reference has no queue between a verb and its
//! effect, so a call made with no host lent is refused with a Lua error.
//!
//! Two engine steps hold the host's state while they run, so neither can lend it: an engine turn
//! ([`UiScript::host_out`]) and a call's apply ([`CallHost::apply`]). The handlers either fires
//! (an event's listeners, a widget's script) are held ([`holding`]) and run as soon as the host
//! is back, in the order fired, so a call a handler makes applies in place, inside its verb, as
//! the reference's does: they read the state the step left, not the state at the signal.

use std::cell::Cell;
use std::ptr::NonNull;

use mlua::Lua;

use super::{ScriptCall, UiScript};

/// The engine half of the script calls: what applies a call to the state outside the VM.
pub trait CallHost {
    /// Apply `call`, with the input events it was made under raised. The host is out of the VM
    /// while this runs, so a call a handler makes meanwhile waits, and applies next, ahead of any
    /// made after this one. Whatever must run Lua with the host back in place (an event this call
    /// signals, a macro's lines) is returned as the tail.
    fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail>;
}

/// What a call leaves for the VM to run once its host is back in the VM.
pub type CallTail = Box<dyn FnOnce(&mut UiScript)>;

/// The host's place in the VM.
///
/// The invariant that makes [`lend`] and [`with_host`] sound: a pointer in `host` is valid and
/// unaliased. [`lend`] puts it there from a `&mut` that it holds borrowed until it returns, and
/// takes it out again before it returns or unwinds; [`with_host`] takes it out of the slot before
/// it makes the one `&mut` it hands on, and puts it back after. So no pointer outlives the borrow
/// it came from, and no two `&mut` to the host exist at once. `open` holds no pointer.
#[derive(Default)]
struct HostSlot {
    /// The lent host, while one is lent and not out applying a call.
    host: Cell<Option<NonNull<dyn CallHost + 'static>>>,
    /// The lends and the engine turns ([`UiScript::host_out`]) running now. While any is, a call
    /// is the host's: it applies at once with the host in the slot, else as soon as it is back.
    open: Cell<u32>,
    /// An engine step that cannot lend the host is running (an engine turn, or a call's apply) and
    /// no Lua it ran since: a handler fired now is held for the host ([`holding`]).
    hold: Cell<bool>,
}

/// Install the empty slot at the VM's birth, a fork's included: every verb asks it
/// (`calls::submit`) and a lend holds its cell across Lua, so it is installed once and
/// never replaced. A later `set_app_data` would collide with no borrow (benilla-mlua keeps each
/// entry in a cell of its own) but would hand later lookups a second slot beside the lent one.
pub(super) fn install(lua: &Lua) {
    lua.set_app_data(HostSlot::default());
}

/// The slot. It is never replaced or removed after its install, and no one borrows it mutably, so
/// a shared borrow of it may be held across Lua.
fn slot(lua: &Lua) -> mlua::AppDataRef<'_, HostSlot> {
    lua.app_data_ref::<HostSlot>()
        .expect("the host slot is installed at the VM's birth")
}

/// Hold the slot open while it lives: a call made meanwhile waits for the host rather than being
/// refused.
struct Open<'a>(mlua::AppDataRef<'a, HostSlot>);

impl<'a> Open<'a> {
    fn new(slot: mlua::AppDataRef<'a, HostSlot>) -> Self {
        slot.open.set(slot.open.get() + 1);
        Open(slot)
    }
}

impl Drop for Open<'_> {
    fn drop(&mut self) {
        self.0.open.set(self.0.open.get() - 1);
    }
}

/// Set the slot's [`HostSlot::hold`] while it lives, and put back what it was.
struct Hold<'a> {
    slot: mlua::AppDataRef<'a, HostSlot>,
    before: bool,
}

impl<'a> Hold<'a> {
    fn new(slot: mlua::AppDataRef<'a, HostSlot>, hold: bool) -> Self {
        let before = slot.hold.replace(hold);
        Hold { slot, before }
    }
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        self.slot.hold.set(self.before);
    }
}

/// Run `f`, Lua the host runs in place (a chunk, a binding's body, a slash command's handler),
/// with no handler held meanwhile: the handlers it fires run inside it, as its own lines read
/// what they did. Only an engine step's own signals are held.
pub(crate) fn unheld<R>(lua: &Lua, f: impl FnOnce() -> R) -> R {
    let _hold = Hold::new(slot(lua), false);
    f()
}

/// Run `f` with the slot open and nothing held, with no host to lend: the drop of what an engine
/// turn left ([`super::calls::refuse_waiting`]), whose handlers still run, their calls waiting to
/// be dropped with the rest.
pub(crate) fn open_unheld<R>(lua: &Lua, f: impl FnOnce() -> R) -> R {
    let _open = Open::new(slot(lua));
    let _hold = Hold::new(slot(lua), false);
    f()
}

/// Lend `host` to the VM of `lua` while `f` runs, then take it back, `f` unwinding included. A
/// host lent already is put back after.
pub(crate) fn lend<R>(lua: &Lua, host: &mut dyn CallHost, f: impl FnOnce() -> R) -> R {
    struct Restore<'a> {
        slot: mlua::AppDataRef<'a, HostSlot>,
        before: Option<NonNull<dyn CallHost + 'static>>,
    }
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            self.slot.host.set(self.before);
        }
    }
    let lent: NonNull<dyn CallHost + '_> = NonNull::from(host);
    // SAFETY: only the lifetime bound changes; the slot's invariant (above) keeps the pointer from
    // being used past this call, which holds `host` borrowed throughout.
    let lent: NonNull<dyn CallHost + 'static> = unsafe { std::mem::transmute(lent) };
    let _open = Open::new(slot(lua));
    let slot = slot(lua);
    let before = slot.host.replace(Some(lent));
    let _restore = Restore { slot, before };
    f()
}

/// Run `f` on the lent host, out of the slot meanwhile; `None` with no host lent, or with the one
/// lent out applying a call. The handlers `f` fires are held ([`holding`]) for the caller to run
/// once the host is back ([`super::calls::drain`]).
pub(crate) fn with_host<R>(lua: &Lua, f: impl FnOnce(&mut dyn CallHost) -> R) -> Option<R> {
    struct PutBack<'a> {
        slot: mlua::AppDataRef<'a, HostSlot>,
        host: NonNull<dyn CallHost + 'static>,
    }
    impl Drop for PutBack<'_> {
        fn drop(&mut self) {
            self.slot.host.set(Some(self.host));
        }
    }
    let slot = slot(lua);
    let host = slot.host.take()?;
    let _back = PutBack { slot, host };
    let _hold = Hold::new(self::slot(lua), true);
    // SAFETY: the pointer came out of the slot, so by its invariant it is valid, and this is the
    // only `&mut` made from it until it goes back.
    Some(f(unsafe { &mut *host.as_ptr() }))
}

/// Whether a host is lent and in the slot, so a call submitted now applies at once.
pub(crate) fn free(lua: &Lua) -> bool {
    slot(lua).host.get().is_some()
}

/// Whether a call made now is the host's: a lend or an engine turn is running.
pub(crate) fn open(lua: &Lua) -> bool {
    slot(lua).open.get() > 0
}

/// Whether a handler fired now is held for the host rather than run: an engine step that cannot
/// lend it is running, and no Lua it ran in place since ([`unheld`]). A handler run then would
/// make its calls wait past the verb that made them, where the reference applies them inside it.
pub(crate) fn holding(lua: &Lua) -> bool {
    let slot = slot(lua);
    slot.hold.get() && slot.host.get().is_none()
}

impl UiScript {
    /// Run `f` with `host` lent to the VM: each script call a verb makes meanwhile applies before
    /// the verb returns, in call order, through [`CallHost::apply`], as the reference's verbs do.
    /// What an engine turn of this entry left ([`Self::host_out`]) applies first, each call under
    /// its own events. When the outermost lend ends, nothing waits.
    pub fn hosted<R>(&mut self, host: &mut dyn CallHost, f: impl FnOnce(&mut UiScript) -> R) -> R {
        let lua = self.lua().clone();
        let r = lend(&lua, host, || {
            super::calls::drain(&lua);
            f(self)
        });
        if !open(&lua) {
            super::calls::refuse_waiting(&lua);
        }
        r
    }

    /// Run `f` as an engine turn: the app's half of a verb or an engine step that holds the host's
    /// own state alongside the VM, so it cannot lend the host meanwhile. The handlers its pushes
    /// and signals fire are held, and a call Lua `f` runs in place makes (a chunk) waits; the
    /// entry lends the host when the turn ends ([`Self::hosted`]), which runs and applies them in
    /// the order made, or runs the handlers and drops the calls with [`Self::refuse_waiting`] when
    /// it has none to lend.
    pub fn host_out<R>(&mut self, f: impl FnOnce(&mut UiScript) -> R) -> R {
        let lua = self.lua().clone();
        let _open = Open::new(slot(&lua));
        let _hold = Hold::new(slot(&lua), true);
        f(self)
    }

    /// Run `f` with no host to lend, for an entry of a harness without the engine's half that runs
    /// Lua (a load in a bare world): the handlers run in place, and a call its Lua makes waits and
    /// is dropped as `f` returns, as with [`Self::refuse_waiting`].
    pub fn refusing<R>(&mut self, f: impl FnOnce(&mut UiScript) -> R) -> R {
        let lua = self.lua().clone();
        let r = open_unheld(&lua, || f(self));
        self.refuse_waiting();
        r
    }

    /// Run the handlers an engine turn left with no host to lend them, and drop its calls and
    /// theirs, for a harness without the engine's half; the count of calls dropped.
    pub fn refuse_waiting(&mut self) -> usize {
        super::calls::refuse_waiting(self.lua())
    }

    /// Whether a host is lent and free to apply a call now.
    pub fn host_free(&self) -> bool {
        free(self.lua())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{CallHost, CallTail, ScriptCall, UiScript};

    #[derive(Default)]
    pub(crate) struct Recorder(pub(crate) Vec<ScriptCall>);

    impl CallHost for Recorder {
        fn apply(&mut self, _: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            self.0.push(call);
            None
        }
    }

    /// Run `chunk` as a key's body with a recording host lent: the calls it made, each applied
    /// inside its verb, so none is left queued. A verb that holds the model across its submit
    /// fails the chunk here, where the drain borrows it.
    pub(crate) fn applied_in_call(s: &mut UiScript, chunk: &str) -> Vec<ScriptCall> {
        let mut host = Recorder::default();
        s.hosted(&mut host, |s| s.run_binding(chunk))
            .unwrap_or_else(|e| panic!("{chunk}: {e}"));
        assert_eq!(
            s.take_script_calls(),
            vec![],
            "{chunk}: nothing waits for a drain"
        );
        host.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Count(u32);

    impl CallHost for Count {
        fn apply(&mut self, _: &mut UiScript, _: ScriptCall) -> Option<CallTail> {
            self.0 += 1;
            None
        }
    }

    #[test]
    fn a_host_is_lent_only_inside_hosted() {
        let mut s = UiScript::new().unwrap();
        let mut host = Count(0);
        assert!(!s.host_free());
        s.hosted(&mut host, |s| assert!(s.host_free()));
        assert!(!s.host_free());
    }

    /// The slot is there from the VM's birth, so no lend or verb inserts app data.
    #[test]
    fn the_slot_is_installed_at_the_vms_birth() {
        let s = UiScript::new().unwrap();
        assert!(s.lua().app_data_ref::<HostSlot>().is_some());
    }

    /// A fork is a VM of its own with its own slot: a host lent to it applies the fork's verbs,
    /// and the original's slot stays empty.
    #[test]
    fn a_fork_takes_a_lent_host_and_applies_its_verbs() {
        let original = UiScript::new().unwrap();
        let mut fork = original.fork();
        let mut host = Count(0);
        fork.hosted(&mut host, |s| s.run("ClearTarget() ClearTarget()"))
            .unwrap();
        assert_eq!(host.0, 2);
        assert!(!original.host_free());
    }

    #[test]
    fn a_host_is_out_of_the_slot_while_it_applies() {
        let mut s = UiScript::new().unwrap();
        let mut host = Count(0);
        s.hosted(&mut host, |s| {
            let lua = s.lua().clone();
            let inner = with_host(&lua, |_| free(&lua));
            assert_eq!(inner, Some(false), "out while lent out");
            assert!(free(&lua), "and back after");
        });
    }

    #[test]
    fn a_panic_inside_hosted_leaves_the_slot_empty() {
        let mut s = UiScript::new().unwrap();
        let mut host = Count(0);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            s.hosted(&mut host, |_| panic!("inside the lend"));
        }));
        assert!(caught.is_err());
        assert!(!s.host_free(), "the unwind took the host back");
        let lua = s.lua().clone();
        assert!(with_host(&lua, |_| ()).is_none());
    }

    #[test]
    fn a_panic_while_applying_puts_the_host_back_for_the_rest_of_the_lend() {
        let mut s = UiScript::new().unwrap();
        let mut host = Count(0);
        s.hosted(&mut host, |s| {
            let lua = s.lua().clone();
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                with_host(&lua, |_| panic!("inside an apply"));
            }));
            assert!(caught.is_err());
            assert!(free(&lua));
        });
        assert!(!s.host_free());
    }

    #[test]
    fn a_lend_inside_a_lend_restores_the_outer_host() {
        let mut s = UiScript::new().unwrap();
        let (mut outer, mut inner) = (Count(0), Count(0));
        s.hosted(&mut outer, |s| {
            s.hosted(&mut inner, |s| {
                let lua = s.lua().clone();
                with_host(&lua, |h| h.apply(s, ScriptCall::ClearTarget));
            });
            let lua = s.lua().clone();
            with_host(&lua, |h| h.apply(s, ScriptCall::ClearTarget));
        });
        assert_eq!((outer.0, inner.0), (1, 1));
    }
}
