//! The front of `SetSelection` (`SelectTarget 0x493540`): the two returns ahead of every selection
//! it makes, shared by the script verbs (`TargetUnit`, `TargetByName`, the `TargetNearest*` cycle,
//! `TargetLastTarget`, `AssistUnit`, `AttackTarget`'s validator) and the other writers that go through
//! [`super::SelectCommit`].
//!
//! 1. A charmed active player returns at once (`0x493589`-`0x49359c`), whatever the guid.
//! 2. A nonzero guid that names an object, with the spell targeting cursor up (`0x6e48a0`,
//!    `0x4935ca`), is bound to the pending cast (`BindSpellTargetObject 0x6e5b40`, `0x4935d5`) and
//!    the call returns without selecting. The bind asks the gate's tag 2 first (`0x6e5ba5`), and a
//!    refusal is silent. Guid 0, a deselect, skips it (`0x4935ac`).
//!
//! The bind itself belongs to the cursor's owner ([`crate::spell::CursorPicks`]): a selection writer
//! holds the attack seam, whose cast state the cast ladder holds too, so it parks the unit and the
//! owner binds it.
//!
//! A world click does not pass here: it holds off while the cursor is up and the cursor's own click
//! binds the picked object ([`crate::spell::targeting::commit_object_cast_on_click`],
//! `0x4925d0` → `0x493540`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::net::{ObjectStore, SelfPlayer};
use crate::spell::{CursorPicks, ScriptCastPermit, SpellTargeting};

/// What the front reads, and where it parks a unit for the bind.
#[derive(SystemParam)]
pub(crate) struct SelectFront<'w, 's> {
    targeting: Res<'w, SpellTargeting>,
    /// The gate's tag 2 for the script call being applied; absent in a world that applies none.
    permit: Option<Res<'w, ScriptCastPermit>>,
    picks: ResMut<'w, CursorPicks>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
}

impl SelectFront<'_, '_> {
    /// Whether `SelectTarget` returns before selecting `guid`, the object `entity`: charmed, or
    /// the cursor is up and the unit goes to the bind instead.
    pub(super) fn takes(&mut self, entity: Entity, guid: u64) -> bool {
        // `0x49359c`: `UNIT_FIELD_CHARMEDBY` of the active player.
        if self
            .me
            .iter()
            .next()
            .is_some_and(|me| me.0.unit_charmed_by().is_some())
        {
            return true;
        }
        if !self.targeting.active() {
            return false;
        }
        // `0x6e5ba5`: the gate refuses the bind, and `SelectTarget` returns all the same.
        if self.permit.as_deref().is_none_or(ScriptCastPermit::allows) {
            self.picks.park(entity, guid);
        }
        true
    }
}
