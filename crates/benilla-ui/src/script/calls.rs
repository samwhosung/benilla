//! The script calls whose effects touch the selection, the player's cast or the targeting cursor,
//! held in one queue in the order the script made them. In the reference each such call is done
//! when it returns: the macro runner `0x4f14e0` fires every line in one pass, `TargetUnit`
//! (`0x4899d0`), `TargetByName` (`0x489d60`) and `ClearTarget` (`0x489ff0`) commit through
//! `SetSelection 0x493540` before returning, and `CastSpellByName` (`0x4b4ab0`) casts at the
//! selection as it stands then. So `/target Bob` then `/cast Flash Heal` heals Bob, and the app
//! applies this queue front to back at one point in the frame.

use super::{ActionUse, SelectionRequest, UiScript};

/// One queued call, in the reference's terms.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptCall {
    /// `TargetUnit`, `AssistUnit`, `AssistByName` and `TargetLastEnemy`.
    Select(SelectionRequest),
    /// `TargetByName(name, exactMatch)` (`0x489d60`), the stock `/target`.
    TargetByName { name: String, exact: bool },
    /// `TargetNearestEnemy`, `TargetNearestFriend`, `TargetNearestPartyMember` and
    /// `TargetNearestRaidMember` (`0x489a80`/`0x489aa0`/`0x489ac0`/`0x489ae0`): one TAB cycle,
    /// `0x493f60(reverse, mode)`.
    TargetNearest { mode: NearestMode, reverse: bool },
    /// `TargetLastTarget()` (`0x489b00`): re-select the last-target pair, or deselect when it is
    /// empty.
    TargetLastTarget,
    /// `ClearTarget()` (`0x489ff0` → `0x493540(0,0)`), a no-op when nothing is selected by then.
    ClearTarget,
    /// `AttackTarget()` (`0x489b50` → `0x6131a0(0,0)`): the attack validator's pick from the
    /// selection as it stands then, which may move it to the nearest enemy, then the toggle.
    AttackTarget {
        /// The gate's tag 4 allowed the pick (`0x6130bc` → `0x493f9b`): with none, the validator
        /// takes the selection as it stands.
        may_pick: bool,
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
    /// applying it starts (`TryCastSpell`, `0x6e4bc1`); a call that casts nothing, or that was
    /// refused at the verb and never queued, reads passed.
    pub fn may_cast(&self) -> bool {
        match self {
            ScriptCall::UseAction(press) => press.may_cast,
            ScriptCall::UseContainerItem(item) => item.may_cast,
            ScriptCall::UseInventoryItem { may_cast, .. } => *may_cast,
            ScriptCall::CastSpell { may_cast, .. } => *may_cast,
            ScriptCall::CastShapeshiftForm { may_cast, .. } => *may_cast,
            _ => true,
        }
    }
}

/// A call and the input events running when its verb queued it. The reference does the call inside
/// that verb, so what it signals runs with those events up ([`super::protected`]); the app applies
/// the call a frame step later and raises them again around it.
#[derive(Clone, Debug, PartialEq)]
pub struct QueuedCall {
    pub call: ScriptCall,
    pub input: super::InputEvents,
}

impl super::Model {
    /// Queue `call` with the input events running now.
    pub(crate) fn queue_call(&mut self, call: ScriptCall) {
        let input = self.protected.events();
        self.script_calls.push(QueuedCall { call, input });
    }
}

impl super::Model {
    /// Queue an engine-side refusal for the app to show, with the input events running now.
    pub(crate) fn push_ui_error(&mut self, key: &'static str) {
        let input = self.protected.events();
        self.ui_errors.push((key, input));
    }
}

impl UiScript {
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

    /// Put calls back at the front of the queue, ahead of any queued since.
    pub fn requeue_calls(&mut self, calls: Vec<QueuedCall>) {
        let mut model = self.model_mut();
        let later = std::mem::replace(&mut model.script_calls, calls);
        model.script_calls.extend(later);
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
