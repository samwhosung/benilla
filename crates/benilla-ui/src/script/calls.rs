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
    AttackTarget,
    /// `SpellTargetUnit(unit)` (`0x6e6d90`), made while the cursor was up.
    SpellTargetUnit(String),
    /// `SpellStopTargeting()` (`0x6e6e30`), made while the cursor was up.
    SpellStopTargeting,
    /// `SpellStopCasting()` (`0x6e6e80`), made with something to stop.
    SpellStopCasting,
    /// `CastSpell(id, "spell")` and `CastSpellByName`, through `0x4b3300`'s player leg.
    CastSpell(u32),
    /// `CastSpell(id, "pet")`, `0x4b3300`'s `CMSG_PET_ACTION` fork (`0x4b34ce`).
    CastPetSpell(u32),
    /// `CastShapeshiftForm(index)` (`0x4b4810`), as the form's spell.
    CastShapeshiftForm(u32),
    /// `UseAction(slot, …)` (`0x4e7140` → `0x4e5ee0`); a macro slot runs its lines inside it.
    UseAction(ActionUse),
    /// `UseContainerItem(bag, slot)` outside repair mode.
    UseContainerItem { bag: i64, slot: u32 },
    /// `UseInventoryItem(id)` outside repair mode.
    UseInventoryItem(u32),
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

impl UiScript {
    /// Take every queued call, in call order.
    pub fn take_script_calls(&mut self) -> Vec<ScriptCall> {
        std::mem::take(&mut self.model_mut().script_calls)
    }

    /// Put calls back at the front of the queue, ahead of any queued since.
    pub fn requeue_script_calls(&mut self, calls: Vec<ScriptCall>) {
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
        model.script_calls.retain(|call| match pick(call) {
            Some(t) => {
                taken.push(t);
                false
            }
            None => true,
        });
        taken
    }
}
