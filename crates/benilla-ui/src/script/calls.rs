//! The script calls whose effects touch the selection, the player's cast or the targeting cursor,
//! each applied inside its verb through the lent host ([`super::UiScript::hosted`]). In the
//! reference each such call is done when it returns: the macro runner `0x4f14e0` fires every line
//! in one pass, `TargetUnit` (`0x4899d0`), `TargetByName` (`0x489d60`) and `ClearTarget`
//! (`0x489ff0`) commit through `SetSelection 0x493540` before returning, and `CastSpellByName`
//! (`0x4b4ab0`) casts at the selection as it stands then. So `/target Bob` then `/cast Flash Heal`
//! heals Bob.
//!
//! A verb hands its call to [`submit`], which applies it before the verb returns. A call made
//! while the host is out (applying another call, or in an engine turn,
//! [`super::UiScript::host_out`]) waits for it, and applies as soon as it is back, ahead of any
//! made later. With no host lent the call is refused; a harness with no engine keeps it instead
//! ([`super::UiScript::keep_calls`]).
//!
//! A call that selects a unit may find the spell targeting cursor up when it is applied:
//! `SelectTarget` (`0x493540`) then binds the unit to the pending cast and returns without
//! selecting (`0x4935d5`), behind the gate's tag 2 inside `BindSpellTargetObject` (`0x6e5ba5`). Each
//! such call carries that answer as `may_cast`, asked where the Lua verb ran.

use std::collections::VecDeque;

use mlua::Lua;

use super::{ActionUse, Model, SelectionRequest, UiScript};

/// One script call a verb hands the host, in the reference's terms.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptCall {
    /// `TargetUnit`, `AssistUnit`, `AssistByName` and `TargetLastEnemy`.
    Select {
        request: SelectionRequest,
        /// The gate's tag 2, which `SelectTarget`'s bind asks while the cursor is up (`0x6e5ba5`).
        may_cast: bool,
    },
    /// `TargetByName(name, exactMatch)` (`0x489d60`), the stock `/target`.
    TargetByName {
        name: String,
        exact: bool,
        /// As [`ScriptCall::Select::may_cast`].
        may_cast: bool,
    },
    /// `TargetNearestEnemy`, `TargetNearestFriend`, `TargetNearestPartyMember` and
    /// `TargetNearestRaidMember` (`0x489a80`/`0x489aa0`/`0x489ac0`/`0x489ae0`): one TAB cycle,
    /// `0x493f60(reverse, mode)`.
    TargetNearest {
        mode: NearestMode,
        reverse: bool,
        /// As [`ScriptCall::Select::may_cast`], for the pick's `SelectTarget` (`0x4941e6`); tag 4
        /// opened the cycle (`0x493f9b`).
        may_cast: bool,
    },
    /// `TargetLastTarget()` (`0x489b00`): re-select the last-target pair, or deselect when it is
    /// empty.
    TargetLastTarget {
        /// As [`ScriptCall::Select::may_cast`]; the empty pair's deselect asks no gate (`0x4935ac`).
        may_cast: bool,
    },
    /// `ClearTarget()` (`0x489ff0` → `0x493540(0,0)`), a no-op when nothing is selected by then.
    ClearTarget,
    /// `AttackTarget()` (`0x489b50` → `0x6131a0(0,0)`): the attack validator's pick from the
    /// selection as it stands then, which may move it to the nearest enemy, then the toggle.
    AttackTarget {
        /// The gate's tag 4 allowed the pick (`0x6130bc` → `0x493f9b`): with none, the validator
        /// takes the selection as it stands.
        may_pick: bool,
        /// As [`ScriptCall::Select::may_cast`], for the `SelectTarget` of a kept unit
        /// (`0x613100`) and of the cycle's pick.
        may_cast: bool,
    },
    /// `SpellTargetUnit(unit)` (`0x6e6d90`), made while the cursor was up.
    SpellTargetUnit(String),
    /// `SpellStopTargeting()` (`0x6e6e30`), made while the cursor was up.
    SpellStopTargeting,
    /// `SpellStopCasting()` (`0x6e6e80`), made with something to stop.
    SpellStopCasting,
    /// `CastSpell(id, "spell", onSelf)` and `CastSpellByName(name, onSelf)` on a player-book name,
    /// through `0x4b3300`'s player leg; `onSelf` swaps the target guid for the active player's
    /// (`0x4b4afa`, `0x4b4345`).
    CastSpell {
        spell_id: u32,
        on_self: bool,
        /// The gate's tag 2 allowed the cast (`0x6e4bc1`). The book leg's cancels (an active aura
        /// `0x4b3466`, a form `0x4b35da`) come before it, so a refusal stops only the cast.
        may_cast: bool,
    },
    /// `CastSpell(id, "pet", onSelf)` and `CastSpellByName(name, onSelf)` on a name only the pet's
    /// book holds (`0x4b3b51`): `0x4b3300`'s `CMSG_PET_ACTION` fork (`0x4b34ce`), with the same
    /// guid swap.
    CastPetSpell { spell_id: u32, on_self: bool },
    /// `CastShapeshiftForm(index)` (`0x4b4810`), as the form's spell.
    CastShapeshiftForm {
        spell_id: u32,
        /// The gate's tag 2 allowed the cast (`0x4b4992` → `0x6e4bc1`); the cancels at `0x4b496b`
        /// and `0x4b497c` come before it.
        may_cast: bool,
    },
    /// `UseAction(slot, …)` (`0x4e7140` → `0x4e5ee0`); a macro slot runs its lines inside it.
    UseAction(ActionUse),
    /// `UseContainerItem(bag, slot, onSelf)` outside repair mode, its arm chosen and locked in the
    /// call.
    UseContainerItem(super::ContainerUse),
    /// `UseInventoryItem(slot, onSelf)` outside repair mode: `on_self` makes the player the
    /// use's target in place of the selection (`0x4c8e37`-`0x4c8e48`).
    UseInventoryItem {
        slot: u32,
        on_self: bool,
        /// The gate's tag 2 allowed the cast ([`super::ContainerUse::may_cast`]).
        may_cast: bool,
    },
    /// `CastPetAction(slot)`, a 1-based pet bar slot.
    PetAction(u32),
    /// `PetAttack` and the other one-shot orders, as their bar slot's packed word.
    PetOrder(u32),
    /// `SendChatMessage(text, type, language, target)` (`0x49f1e0`) past the binding's checks: the
    /// host runs the rest of the verb inside the call, from the group gates to the packet, and
    /// raises its escape error there ([`UiScript::raise_in_call`]).
    SendChat(super::ChatSend),
    /// `RandomRoll(min, max)` (`0x48c7b0`) past its range check (`0x48c807`-`0x48c830`):
    /// `MSG_RANDOM_ROLL` (`0x48c832`).
    RandomRoll { min: u32, max: u32 },
}

/// The mode the four `TargetNearest*` shims hand the TAB cycler `0x493f60`, the only byte that
/// differs between them; it reaches only the per-candidate filter `0x493e40`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NearestMode {
    /// Mode 1, `TargetNearestEnemy`.
    Enemy,
    /// Mode 2, `TargetNearestFriend`.
    Friend,
    /// Mode 3, `TargetNearestPartyMember`: a party member other than us.
    PartyMember,
    /// Mode 4, `TargetNearestRaidMember`: a party or raid member other than us.
    RaidMember,
}

impl ScriptCall {
    /// What the protected-action gate's tag 2 allowed the verb that queued this call, for the casts
    /// applying it starts (`TryCastSpell`, `0x6e4bc1`) or the bind of a selection it makes while
    /// the targeting cursor is up (`0x6e5ba5`); a call that does neither, or that was refused at
    /// the verb and never queued, reads passed.
    pub fn may_cast(&self) -> bool {
        match self {
            ScriptCall::UseAction(press) => press.may_cast,
            ScriptCall::UseContainerItem(item) => item.may_cast,
            ScriptCall::UseInventoryItem { may_cast, .. } => *may_cast,
            ScriptCall::CastSpell { may_cast, .. } => *may_cast,
            ScriptCall::CastShapeshiftForm { may_cast, .. } => *may_cast,
            ScriptCall::Select { may_cast, .. }
            | ScriptCall::TargetByName { may_cast, .. }
            | ScriptCall::TargetNearest { may_cast, .. }
            | ScriptCall::TargetLastTarget { may_cast }
            | ScriptCall::AttackTarget { may_cast, .. } => *may_cast,
            _ => true,
        }
    }
}

/// A call and the input events running when its verb made it. The reference does the call inside
/// that verb, so what it signals runs with those events up ([`super::protected`]); a call that
/// waited for the host gets them raised again around it.
#[derive(Clone, Debug, PartialEq)]
pub struct QueuedCall {
    pub call: ScriptCall,
    pub input: super::InputEvents,
    /// The call's number, under which a raise it makes is filed for its verb.
    seq: u64,
}

/// The raises the host makes in its calls' verbs ([`UiScript::raise_in_call`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct CallRaises {
    /// The number the next queued call takes.
    next: u64,
    /// The calls whose verbs are inside [`submit`], waiting for them to apply.
    awaited: Vec<u64>,
    /// What the call applying now raised.
    raising: Option<String>,
    /// The raises of awaited calls, for their verbs to take.
    raised: Vec<(u64, String)>,
}

impl super::Model {
    /// Hold `call` for the host, with the input events running now; its number.
    fn queue_call(&mut self, call: ScriptCall) -> u64 {
        let input = self.protected.events();
        let seq = self.call_raises.next;
        self.call_raises.next += 1;
        self.script_calls.push(QueuedCall { call, input, seq });
        seq
    }
}

/// How deep calls made while applying a call may nest: a macro whose line presses a macro, and so
/// on. A call made at the ninth level (a macro that presses itself) is not applied: its verb
/// raises [`TOO_DEEP`] in the Lua that made it, which reports it as that script's error, and the
/// eight calls under it stand applied. So such a chain always ends inside its frame. The
/// reference's own bound on it is not read.
pub(crate) const MAX_NESTING: u8 = 8;

/// What a verb raises when no host is lent: an entry ran Lua without lending one.
pub(crate) const NO_HOST: &str = "no call host is lent for this script call";

/// What a verb raises past [`MAX_NESTING`].
pub(crate) const TOO_DEEP: &str = "script calls nested too deep";

/// A verb's call: with a host lent and free, applied before the verb returns, after any call that
/// was waiting for it, under the input events running now; with the host out, held until it is
/// back. What the host raises in the call ([`UiScript::raise_in_call`]) is the verb's error. The
/// caller holds no borrow of the model.
pub(crate) fn submit(lua: &Lua, call: ScriptCall) -> mlua::Result<()> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if !super::host::open(lua) {
        return match model.kept_calls.as_mut() {
            Some(kept) => {
                kept.push(call);
                Ok(())
            }
            None => Err(mlua::Error::runtime(NO_HOST)),
        };
    }
    if model.call_depth >= MAX_NESTING {
        return Err(mlua::Error::runtime(TOO_DEEP));
    }
    let seq = model.queue_call(call);
    model.call_raises.awaited.push(seq);
    drop(model);
    /// The verb stops waiting for its call as it returns or unwinds.
    struct Awaited<'a>(&'a Lua, u64);
    impl Drop for Awaited<'_> {
        fn drop(&mut self) {
            if let Some(mut model) = self.0.app_data_mut::<Model>() {
                model.call_raises.awaited.retain(|&s| s != self.1);
            }
        }
    }
    let awaited = Awaited(lua, seq);
    drain(lua);
    drop(awaited);
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let raised = &mut model.call_raises.raised;
    match raised.iter().position(|&(s, _)| s == seq) {
        Some(at) => Err(mlua::Error::runtime(raised.remove(at).1)),
        None => Ok(()),
    }
}

/// File what the call `seq` raised: for its verb, while that waits inside [`submit`]; else, its
/// verb having returned while the call waited for the host, as its script's error, which the
/// error handler hears as it would the verb's raise.
fn file_raise(lua: &Lua, seq: u64, message: String) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if model.call_raises.awaited.contains(&seq) {
        model.call_raises.raised.push((seq, message));
    } else {
        // The reference never defers a call: its verb raises in its own body, so the lines after
        // it never run and a `pcall` around it catches the error. A call that waited for the host
        // had its verb return without an error, the script's later lines ran, and a `pcall` saw
        // nothing; the error reaches the handler only now, after them.
        model.record_lua_error(&mlua::Error::runtime(message));
    }
}

/// Swap the applying call's raise for `raise`, returning what was there.
fn swap_raising(lua: &Lua, raise: Option<String>) -> Option<String> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    std::mem::replace(&mut model.call_raises.raising, raise)
}

/// Apply what waits through the lent host, front to back, each call and its tail under exactly the
/// input events its verb ran under. The calls an applied call makes while its host is out go next,
/// ahead of the rest; those its tail makes apply inside the tail.
pub(crate) fn drain(lua: &Lua) {
    if !super::host::free(lua) {
        return;
    }
    let mut batch: VecDeque<QueuedCall> = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if model.script_calls.is_empty() {
            return;
        }
        model.call_depth += 1;
        std::mem::take(&mut model.script_calls).into()
    };
    struct Depth<'a>(&'a Lua);
    impl Drop for Depth<'_> {
        fn drop(&mut self) {
            if let Some(mut model) = self.0.app_data_mut::<Model>() {
                model.call_depth = model.call_depth.saturating_sub(1);
            }
        }
    }
    let _depth = Depth(lua);
    let mut script = UiScript::reentrant(lua);
    while !batch.is_empty() {
        if !super::host::free(lua) {
            // A tail took the host out: the rest waits for it, ahead of what was made since.
            requeue(lua, batch);
            return;
        }
        let Some(QueuedCall { call, input, seq }) = batch.pop_front() else {
            break;
        };
        // What the host raises while it applies the call and runs its tail is this call's.
        let outer = swap_raising(lua, None);
        // The reference does the call inside the verb, so what it signals runs inside the key,
        // click or Enter that made it, and only inside that: a queued call gets exactly its events.
        let tail = script.with_input_events(input, |script| {
            super::host::with_host(lua, |host| host.apply(script, call)).flatten()
        });
        if let Some(tail) = tail {
            script.with_input_events(input, tail);
        }
        if let Some(message) = swap_raising(lua, outer) {
            file_raise(lua, seq, message);
        }
        drain(lua);
    }
}

/// Put `calls` back at the front of the queue, ahead of any queued since.
fn requeue(lua: &Lua, calls: impl IntoIterator<Item = QueuedCall>) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let later = std::mem::replace(&mut model.script_calls, calls.into_iter().collect());
    model.script_calls.extend(later);
}

impl super::Model {
    /// Queue an engine-side refusal for the app to show, with the input events running now.
    pub(crate) fn push_ui_error(&mut self, key: &'static str) {
        let input = self.protected.events();
        self.ui_errors.push((key, input));
    }
}

/// Drop what waits for a host that will not come back: an engine turn's calls with no host to
/// apply them, or a lend's leftovers once the outermost lend has ended. The count dropped.
pub(crate) fn refuse_waiting(lua: &Lua) -> usize {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let n = model.script_calls.len();
    model.script_calls.clear();
    n
}

impl UiScript {
    /// Raise `message` in the verb of the call the host is applying, as that verb returns: the
    /// reference's verb raises in its own body (`lua_error`, through `0x6f4940`), after what it did
    /// before, which stands. A verb that returned while its call waited for the host reports it as
    /// its script's error. For [`super::CallHost::apply`] and the tail it leaves.
    pub fn raise_in_call(&mut self, message: &str) {
        self.model_mut().call_raises.raising = Some(message.to_string());
    }

    /// Make this VM a harness's: a call its verbs make with no host lent is kept, for the
    /// `take_*` calls to read, in place of the refusal. For a VM with no engine behind it.
    pub fn keep_calls(&mut self) {
        let mut model = self.model_mut();
        if model.kept_calls.is_none() {
            model.kept_calls = Some(Vec::new());
        }
    }

    /// Take every call [`Self::keep_calls`] kept, in call order.
    pub fn take_script_calls(&mut self) -> Vec<ScriptCall> {
        self.model_mut()
            .kept_calls
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Take the kept calls `pick` answers for, in call order, leaving the rest kept.
    pub(crate) fn take_calls_where<T>(
        &mut self,
        mut pick: impl FnMut(&ScriptCall) -> Option<T>,
    ) -> Vec<T> {
        let mut model = self.model_mut();
        let mut taken = Vec::new();
        if let Some(kept) = model.kept_calls.as_mut() {
            kept.retain(|call| match pick(call) {
                Some(t) => {
                    taken.push(t);
                    false
                }
                None => true,
            });
        }
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::super::host::{CallHost, CallTail};
    use super::super::InputEvents;
    use super::*;

    /// A host that records each call it applies, and for the first one runs `inner` with the host
    /// out of the VM, as an applier's engine half runs a handler.
    struct Nesting {
        applied: Vec<ScriptCall>,
        inner: Option<&'static str>,
    }

    impl CallHost for Nesting {
        fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            self.applied.push(call);
            if let Some(chunk) = self.inner.take() {
                script.run(chunk).expect("the inner chunk runs");
            }
            None
        }
    }

    fn select(token: &str) -> ScriptCall {
        ScriptCall::Select {
            request: SelectionRequest::Unit(token.into()),
            may_cast: true,
        }
    }

    const LAST_ENEMY: ScriptCall = ScriptCall::Select {
        request: SelectionRequest::LastEnemy,
        may_cast: true,
    };

    /// With the host out applying a call, a call its handler makes waits, and applies before the
    /// next call the script makes.
    #[test]
    fn a_call_made_while_the_host_is_out_applies_next_in_call_order() {
        let mut s = UiScript::new().unwrap();
        let mut host = Nesting {
            applied: Vec::new(),
            inner: Some("TargetLastEnemy()"),
        };
        s.hosted(&mut host, |s| {
            s.run(r#"TargetUnit("player") ClearTarget()"#).unwrap()
        });
        assert_eq!(
            host.applied,
            vec![select("player"), LAST_ENEMY, ScriptCall::ClearTarget]
        );
    }

    /// A host that records each call and leaves `tail` to run with the host back, noting whether
    /// the tail's own calls were applied by the time it ended.
    struct Tailing {
        applied: Vec<ScriptCall>,
        tail: Option<&'static str>,
        forever: bool,
        errors: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    }

    impl CallHost for Tailing {
        fn apply(&mut self, _: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            self.applied.push(call);
            let chunk = if self.forever {
                self.tail
            } else {
                self.tail.take()
            }?;
            let errors = self.errors.clone();
            Some(Box::new(move |script: &mut UiScript| {
                if let Err(e) = script.run(chunk) {
                    errors.borrow_mut().push(e.to_string());
                }
            }))
        }
    }

    fn tailing(tail: &'static str, forever: bool) -> Tailing {
        Tailing {
            applied: Vec::new(),
            tail: Some(tail),
            forever,
            errors: Default::default(),
        }
    }

    /// A tail runs with the host back in the VM, so the calls it makes apply inside it, ahead of
    /// the calls the script makes after the one that left it.
    #[test]
    fn a_tails_calls_apply_inside_it() {
        let mut s = UiScript::new().unwrap();
        let mut host = tailing("TargetLastEnemy()", false);
        s.hosted(&mut host, |s| {
            s.run(r#"TargetUnit("player") ClearTarget()"#).unwrap()
        });
        assert_eq!(
            host.applied,
            vec![select("player"), LAST_ENEMY, ScriptCall::ClearTarget]
        );
        assert!(host.errors.borrow().is_empty());
    }

    /// A call that makes itself again (a macro pressing its own button) nests [`MAX_NESTING`]
    /// deep, and the verb past the bound raises, so a frame always ends and nothing waits after.
    #[test]
    fn a_call_that_makes_itself_nests_to_the_bound_then_raises() {
        let mut s = UiScript::new().unwrap();
        let mut host = tailing("TargetLastEnemy()", true);
        s.hosted(&mut host, |s| s.run("TargetLastEnemy()").unwrap());
        assert_eq!(host.applied.len(), usize::from(MAX_NESTING));
        let errors = host.errors.borrow();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains(TOO_DEEP), "{errors:?}");
        drop(errors);
        assert_eq!(s.refuse_waiting(), 0, "nothing waits");
    }

    /// With no host lent a call is refused with an error: an entry ran Lua without lending one.
    #[test]
    fn with_no_host_a_call_is_refused() {
        let mut s = UiScript::new().unwrap();
        let e = s.run("ClearTarget()").unwrap_err().to_string();
        assert!(e.contains(NO_HOST), "{e}");
        s.keep_calls();
        s.run("ClearTarget()").unwrap();
        assert_eq!(
            s.take_script_calls(),
            vec![ScriptCall::ClearTarget],
            "a harness's VM keeps it"
        );
    }

    /// A host that records each call and raises in the verb of every `ClearTarget` it applies,
    /// after running `inner` with the host out, as an applier's engine half runs a handler.
    #[derive(Default)]
    struct Raising {
        applied: Vec<ScriptCall>,
        inner: Option<&'static str>,
    }

    impl CallHost for Raising {
        fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            let raises = call == ScriptCall::ClearTarget;
            self.applied.push(call);
            if let Some(chunk) = self.inner.take() {
                script.run(chunk).expect("the inner chunk runs");
            }
            if raises {
                script.raise_in_call("refused in the call");
            }
            None
        }
    }

    /// The host's raise reaches the verb of the call it applied as that verb returns, as the
    /// reference's verb raises in its own body (`0x49f769`, `SendChatMessage`'s escape error): the
    /// call stands applied, the script's next line does not run, and `pcall` catches it.
    #[test]
    fn a_raise_in_the_call_reaches_its_verb_after_what_the_call_did() {
        let mut s = UiScript::new().unwrap();
        let mut host = Raising::default();
        let e = s
            .hosted(&mut host, |s| s.run("ClearTarget() TargetLastEnemy()"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("refused in the call"), "{e}");
        assert_eq!(host.applied, vec![ScriptCall::ClearTarget]);
        let (ok, e) = s
            .hosted(&mut host, |s| {
                s.eval::<(bool, String)>("local ok, e = pcall(ClearTarget) return ok, tostring(e)")
            })
            .unwrap();
        assert!(!ok && e.contains("refused in the call"), "{e}");
        s.hosted(&mut host, |s| s.run("TargetLastEnemy()"))
            .expect("a call that raises nothing returns");
        assert!(s.take_errors().is_empty());
    }

    /// A call that waited for the host applies after its verb returned, so its raise is that
    /// script's error, which the error handler hears; the call that was applying raises nothing.
    #[test]
    fn a_raise_in_a_call_that_waited_is_its_scripts_error() {
        let mut s = UiScript::new().unwrap();
        s.run("HEARD = {} seterrorhandler(function(e) table.insert(HEARD, tostring(e)) end)")
            .unwrap();
        let mut host = Raising {
            inner: Some("ClearTarget()"),
            ..Default::default()
        };
        s.hosted(&mut host, |s| s.run("TargetLastEnemy()"))
            .expect("the outer verb's own call raised nothing");
        assert_eq!(host.applied, vec![LAST_ENEMY, ScriptCall::ClearTarget]);
        s.dispatch_script_errors_to_handler();
        let heard: Vec<String> = s.eval("return HEARD").expect("the handler's record");
        assert_eq!(heard.len(), 1, "{heard:?}");
        assert!(heard[0].contains("refused in the call"), "{heard:?}");
    }

    /// A key's input event raised alone.
    const KEY: InputEvents = InputEvents {
        binding: true,
        ..InputEvents::NONE
    };

    /// A host that records the input events each call applies under, and those its tail runs
    /// under.
    #[derive(Default)]
    struct Events {
        applied: Vec<(ScriptCall, InputEvents)>,
        tails: std::rc::Rc<std::cell::RefCell<Vec<InputEvents>>>,
    }

    impl CallHost for Events {
        fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            self.applied.push((call, script.input_events()));
            let tails = self.tails.clone();
            Some(Box::new(move |script: &mut UiScript| {
                tails.borrow_mut().push(script.input_events());
            }))
        }
    }

    /// A call an engine turn left (an event its engine step signalled) waits for the host and
    /// applies under the events its verb ran under, none, though a key's body is the entry that
    /// lends it, and before that body runs, so the body reads the state it made.
    #[test]
    fn an_engine_turns_call_applies_under_its_own_events_when_the_host_is_lent() {
        let mut s = UiScript::new().unwrap();
        s.host_out(|s| s.run(r#"TargetUnit("player")"#)).unwrap();
        let mut host = Events::default();
        s.hosted(&mut host, |s| s.run_binding("ClearTarget()"))
            .unwrap();
        assert_eq!(
            host.applied,
            vec![
                (select("player"), InputEvents::NONE),
                (ScriptCall::ClearTarget, KEY)
            ]
        );
        assert_eq!(*host.tails.borrow(), vec![InputEvents::NONE, KEY]);
    }

    /// An engine turn with no host to lend after drops what it left: nothing waits past the entry
    /// that made it.
    #[test]
    fn an_engine_turn_with_no_host_drops_what_it_left() {
        let mut s = UiScript::new().unwrap();
        s.host_out(|s| s.run(r#"TargetUnit("player") ClearTarget()"#))
            .unwrap();
        assert_eq!(s.refuse_waiting(), 2);
        let mut host = Events::default();
        s.hosted(&mut host, |_| ());
        assert!(host.applied.is_empty());
    }

    /// A call queued with the host out, under no input event, applies under none and its tail
    /// runs under none, though a key's body raised its event by the time it is applied.
    #[test]
    fn a_call_and_its_tail_never_run_under_the_draining_entrys_events() {
        let mut s = UiScript::new().unwrap();
        let mut host = Events::default();
        s.hosted(&mut host, |s| {
            let lua = s.lua().clone();
            super::super::host::with_host(&lua, |_| s.run(r#"TargetUnit("player")"#))
                .expect("lent")
                .unwrap();
            s.run_binding("ClearTarget()").unwrap();
        });
        assert_eq!(
            host.applied,
            vec![
                (select("player"), InputEvents::NONE),
                (ScriptCall::ClearTarget, KEY)
            ]
        );
        assert_eq!(*host.tails.borrow(), vec![InputEvents::NONE, KEY]);
    }
}
