//! The script calls whose effects touch the selection, the player's cast or the targeting cursor,
//! held in one queue in the order the script made them. In the reference each such call is done
//! when it returns: the macro runner `0x4f14e0` fires every line in one pass, `TargetUnit`
//! (`0x4899d0`), `TargetByName` (`0x489d60`) and `ClearTarget` (`0x489ff0`) commit through
//! `SetSelection 0x493540` before returning, and `CastSpellByName` (`0x4b4ab0`) casts at the
//! selection as it stands then. So `/target Bob` then `/cast Flash Heal` heals Bob: the queue
//! applies front to back.
//!
//! A verb hands its call to [`submit`], which queues it and, with a host lent
//! ([`super::UiScript::hosted`]), applies the queue before the verb returns. With none lent it
//! waits for the host's next drain ([`super::UiScript::drain_calls`]).
//!
//! A call that selects a unit may find the spell targeting cursor up when it is applied:
//! `SelectTarget` (`0x493540`) then binds the unit to the pending cast and returns without
//! selecting (`0x4935d5`), behind the gate's tag 2 inside `BindSpellTargetObject` (`0x6e5ba5`). Each
//! such call carries that answer as `may_cast`, asked where the Lua verb ran.

use std::collections::VecDeque;

use mlua::Lua;

use super::{ActionUse, Model, SelectionRequest, UiScript};

/// One queued call, in the reference's terms.
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
    /// `CastSpell(id, "spell", onSelf)` and `CastSpellByName(name, onSelf)`, through `0x4b3300`'s
    /// player leg; `onSelf` swaps the target guid for the active player's (`0x4b4afa`, `0x4b4345`).
    CastSpell {
        spell_id: u32,
        on_self: bool,
        /// The gate's tag 2 allowed the cast (`0x6e4bc1`). The book leg's cancels (an active aura
        /// `0x4b3466`, a form `0x4b35da`) come before it, so a refusal stops only the cast.
        may_cast: bool,
    },
    /// `CastSpell(id, "pet", onSelf)`, `0x4b3300`'s `CMSG_PET_ACTION` fork (`0x4b34ce`), with the
    /// same guid swap.
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

/// A call and the input events running when its verb queued it. The reference does the call inside
/// that verb, so what it signals runs with those events up ([`super::protected`]); a call applied
/// after its verb returned gets them raised again around it.
#[derive(Clone, Debug, PartialEq)]
pub struct QueuedCall {
    pub call: ScriptCall,
    pub input: super::InputEvents,
}

impl super::Model {
    /// Queue `call` with the input events running now.
    fn queue_call(&mut self, call: ScriptCall) {
        let input = self.protected.events();
        self.script_calls.push(QueuedCall { call, input });
    }
}

/// How deep calls made while applying a call may nest: a macro whose line presses a macro, and so
/// on. Past it (a macro that presses itself) what is left waits for the next frame's drain, so a
/// frame always ends.
pub(crate) const MAX_NESTING: u8 = 8;

/// A verb's call: queued with the input events running now, then, with a host lent and free,
/// applied with everything queued before it, so the verb returns with its call done, as the
/// reference's does. The caller holds no borrow of the model.
pub(crate) fn submit(lua: &Lua, call: ScriptCall) {
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .queue_call(call);
    drain(lua);
}

/// Apply the queue through the lent host, front to back, each call and its tail under exactly the
/// input events its verb ran under. The calls an applied call makes while its host is out go next,
/// ahead of the rest; those its tail makes apply inside the tail.
pub(crate) fn drain(lua: &Lua) {
    if !super::host::free(lua) {
        return;
    }
    let mut batch: VecDeque<QueuedCall> = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if model.call_depth >= MAX_NESTING {
            let past = std::mem::take(&mut model.script_calls);
            model.deferred_calls.extend(past);
            return;
        }
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
            // A tail ended the lend: the rest waits for the next drain.
            requeue(lua, batch);
            return;
        }
        let Some(QueuedCall { call, input }) = batch.pop_front() else {
            break;
        };
        // The reference does the call inside the verb, so what it signals runs inside the key,
        // click or Enter that made it, and only inside that: a queued call gets exactly its events.
        let tail = script.with_input_events(input, |script| {
            super::host::with_host(lua, |host| host.apply(script, call)).flatten()
        });
        if let Some(tail) = tail {
            script.with_input_events(input, tail);
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

impl UiScript {
    /// Apply every queued call through the lent host ([`Self::hosted`]): the drain for the calls
    /// made where no host was lent, run once a frame. Calls past the nesting bound wait for the
    /// next one.
    pub fn drain_calls(&mut self) {
        drain(self.lua());
        let past = std::mem::take(&mut self.model_mut().deferred_calls);
        if !past.is_empty() {
            requeue(self.lua(), past);
        }
    }

    /// Take every queued call, in call order.
    pub fn take_script_calls(&mut self) -> Vec<ScriptCall> {
        self.take_queued_calls()
            .into_iter()
            .map(|queued| queued.call)
            .collect()
    }

    /// [`Self::take_script_calls`] with the input events each was made under.
    pub fn take_queued_calls(&mut self) -> Vec<QueuedCall> {
        std::mem::take(&mut self.model_mut().script_calls)
    }

    /// Take the queued calls `pick` answers for, in call order, leaving the rest queued.
    pub(crate) fn take_calls_where<T>(
        &mut self,
        mut pick: impl FnMut(&ScriptCall) -> Option<T>,
    ) -> Vec<T> {
        let mut model = self.model_mut();
        let mut taken = Vec::new();
        model
            .script_calls
            .retain(|queued| match pick(&queued.call) {
                Some(t) => {
                    taken.push(t);
                    false
                }
                None => true,
            });
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

    /// With the host out applying a call, a call its handler makes waits in the queue, and the
    /// drain applies it before the next call queued, in the queued path and in the call alike.
    #[test]
    fn a_call_made_while_the_host_is_out_applies_next_in_call_order() {
        let order = vec![select("player"), LAST_ENEMY, ScriptCall::ClearTarget];

        let mut s = UiScript::new().unwrap();
        s.run(r#"TargetUnit("player") ClearTarget()"#).unwrap();
        let mut host = Nesting {
            applied: Vec::new(),
            inner: Some("TargetLastEnemy()"),
        };
        s.hosted(&mut host, UiScript::drain_calls);
        assert_eq!(host.applied, order, "drained from the queue");

        let mut s = UiScript::new().unwrap();
        let mut host = Nesting {
            applied: Vec::new(),
            inner: Some("TargetLastEnemy()"),
        };
        s.hosted(&mut host, |s| {
            s.run(r#"TargetUnit("player") ClearTarget()"#).unwrap()
        });
        assert_eq!(host.applied, order, "applied in the calls");
        assert!(s.take_script_calls().is_empty());
    }

    /// A host that records each call and leaves `tail` to run with the host back, noting whether
    /// the tail's own calls were applied by the time it ended.
    struct Tailing {
        applied: Vec<ScriptCall>,
        tail: Option<&'static str>,
        forever: bool,
        left_queued: std::rc::Rc<std::cell::Cell<bool>>,
    }

    impl CallHost for Tailing {
        fn apply(&mut self, _: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            self.applied.push(call);
            let chunk = if self.forever {
                self.tail
            } else {
                self.tail.take()
            }?;
            let left_queued = self.left_queued.clone();
            Some(Box::new(move |script: &mut UiScript| {
                script.run(chunk).expect("the tail runs");
                if !script.take_script_calls().is_empty() {
                    left_queued.set(true);
                }
            }))
        }
    }

    /// A tail runs with the host back in the VM, so the calls it makes apply inside it, ahead of
    /// the calls queued after the one that left it.
    #[test]
    fn a_tails_calls_apply_inside_it() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"TargetUnit("player") ClearTarget()"#).unwrap();
        let mut host = Tailing {
            applied: Vec::new(),
            tail: Some("TargetLastEnemy()"),
            forever: false,
            left_queued: Default::default(),
        };
        s.hosted(&mut host, UiScript::drain_calls);
        assert_eq!(
            host.applied,
            vec![select("player"), LAST_ENEMY, ScriptCall::ClearTarget]
        );
        assert!(!host.left_queued.get(), "applied before the tail ended");
    }

    /// A call that makes itself again (a macro pressing its own button) nests [`MAX_NESTING`]
    /// deep, and the rest waits for the next frame's drain, so a frame always ends.
    #[test]
    fn a_call_that_makes_itself_nests_to_the_bound_and_waits_a_frame() {
        let mut s = UiScript::new().unwrap();
        let mut host = Tailing {
            applied: Vec::new(),
            tail: Some("TargetLastEnemy()"),
            forever: true,
            left_queued: Default::default(),
        };
        s.hosted(&mut host, |s| s.run("TargetLastEnemy()").unwrap());
        assert_eq!(host.applied.len(), usize::from(MAX_NESTING));
        assert!(
            s.take_script_calls().is_empty(),
            "past the bound, not queued yet"
        );

        host.applied.clear();
        s.hosted(&mut host, UiScript::drain_calls);
        assert!(
            host.applied.is_empty(),
            "this frame's drain had nothing queued"
        );
        assert_eq!(
            s.take_script_calls(),
            vec![LAST_ENEMY],
            "and leaves the deferred call for the next one"
        );
    }

    /// With no host lent, a call waits in the queue.
    #[test]
    fn with_no_host_a_call_waits() {
        let mut s = UiScript::new().unwrap();
        s.run("ClearTarget()").unwrap();
        assert_eq!(s.take_script_calls(), vec![ScriptCall::ClearTarget]);
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

    /// A call an entry with no host left (an `OnUpdate`'s) applies under the events its verb ran
    /// under, none, though a key's body is the entry that applies it.
    #[test]
    fn a_call_left_by_an_unhosted_entry_applies_under_its_own_events() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"TargetUnit("player")"#).unwrap();
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

    /// What an unhosted entry left is applied when the host is lent, before the entry's script
    /// runs, so that script reads the state those calls made.
    #[test]
    fn a_lend_applies_what_was_left_queued_before_its_script_runs() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"TargetUnit("player")"#).unwrap();
        let mut host = Events::default();
        let left = s.hosted(&mut host, |s| s.take_script_calls());
        assert_eq!(left, vec![]);
        assert_eq!(host.applied, vec![(select("player"), InputEvents::NONE)]);
    }

    /// A call queued with the host out, under no input event, applies under none and its tail
    /// runs under none, though a key's body raised its event by the time it is drained.
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
