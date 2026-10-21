//! The cast and auto-repeat state a press, a script call or a cast result changes on a unit: the
//! cast's [`Casting`], its [`Precast`] and the auto-repeat markers. They are components, which a system adds or
//! removes only through a sync point, so the seams that change them (`SelfCancel`, `CastLadder`,
//! `AttackSeam`) queue the change here and hold no `Commands`: the call host they sit in carries
//! no deferred buffer, and the VM pass's hosts need no sync point between them.
//! An exclusive caller applies the queue where it applies the commands of what it runs
//! ([`apply_unit_edits`]): the net drain after each handler, the world's hosted entries and
//! one-shots as they return ([`crate::script_calls::with_hosted_vm`],
//! [`crate::script_calls::run_cached`]). The pass's end set, and a flush in each other schedule
//! that can queue one, hand it to the sync point ahead of the readers ([`flush_unit_edits`]).

use std::sync::{Mutex, MutexGuard, PoisonError};

use bevy::prelude::*;

use super::{precast, AutoRepeatArmed, Casting, NockLatch, NockedAmmo};

/// One queued change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnitEdit {
    /// The cast's reap (`0x614150(spellId, 0)`, keyed by the spell): its [`Casting`], while that
    /// is still this spell's.
    ReapCasting { entity: Entity, spell_id: u32 },
    /// The sticky auto-repeat arm (`0x6e593b`, `|= 0x200`).
    ArmAutoRepeat(Entity),
    /// The local auto-repeat cancel's clear (`0x6ea080`): the arm (`0x6ea113`), the precast of
    /// the repeat spell `spell_id` (`0x6ea12b`) and the nock (`0x6ea140`).
    ClearAutoRepeat { entity: Entity, spell_id: u32 },
    /// `0x6ec220`'s precast at `SMSG_SPELL_START` (`0x6e7908`, `0x6e79be`), queued only for a
    /// spell that resolves a visual for the unit, as its pusher found ([`super::PrecastTables`]).
    StartPrecast { entity: Entity, spell_id: u32 },
    /// The same at the active player's send ([`super::precast_at_send`]), unless the unit already
    /// precasts the spell (`0x6e588f`).
    SendPrecast { entity: Entity, spell_id: u32 },
    /// `0x60d040`: the precast ends when it is `spell_id`'s, or whatever it is for `None`, and
    /// runs its tail when `tail` is set (its `b`).
    EndPrecast {
        entity: Entity,
        spell_id: Option<u32>,
        tail: bool,
    },
    /// `0x60fc50`: the precast's busy bit alone.
    EndPrecastBusy(Entity),
}

impl UnitEdit {
    /// The components this change writes.
    #[cfg(test)]
    pub(crate) fn writes(&self) -> Vec<std::any::TypeId> {
        match self {
            Self::ReapCasting { .. } => vec![std::any::TypeId::of::<Casting>()],
            Self::ArmAutoRepeat(_) => vec![std::any::TypeId::of::<AutoRepeatArmed>()],
            Self::ClearAutoRepeat { .. } => vec![
                std::any::TypeId::of::<AutoRepeatArmed>(),
                std::any::TypeId::of::<super::Precast>(),
                std::any::TypeId::of::<NockedAmmo>(),
                std::any::TypeId::of::<NockLatch>(),
            ],
            Self::StartPrecast { .. }
            | Self::SendPrecast { .. }
            | Self::EndPrecast { .. }
            | Self::EndPrecastBusy(_) => vec![std::any::TypeId::of::<super::Precast>()],
        }
    }

    /// One change of each kind, for the test that no later host reads what they write.
    #[cfg(test)]
    pub(crate) fn every(entity: Entity) -> Vec<UnitEdit> {
        let every = vec![
            Self::ReapCasting {
                entity,
                spell_id: 0,
            },
            Self::ArmAutoRepeat(entity),
            Self::ClearAutoRepeat {
                entity,
                spell_id: 0,
            },
            Self::StartPrecast {
                entity,
                spell_id: 0,
            },
            Self::SendPrecast {
                entity,
                spell_id: 0,
            },
            Self::EndPrecast {
                entity,
                spell_id: None,
                tail: true,
            },
            Self::EndPrecastBusy(entity),
        ];
        // A new kind stops this match compiling until it is listed above as well.
        for edit in &every {
            match edit {
                Self::ReapCasting { .. }
                | Self::ArmAutoRepeat(_)
                | Self::ClearAutoRepeat { .. }
                | Self::StartPrecast { .. }
                | Self::SendPrecast { .. }
                | Self::EndPrecast { .. }
                | Self::EndPrecastBusy(_) => {}
            }
        }
        every
    }
}

/// The queue, pushed through `&self` as the chat log is: its users run one at a time, in the VM
/// pass's order ([`crate::vm_pass::shared_sinks`]) or inside the net drain.
#[derive(Resource, Default)]
pub(crate) struct UnitEdits(Mutex<Vec<UnitEdit>>);

impl UnitEdits {
    fn queue(&self) -> MutexGuard<'_, Vec<UnitEdit>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue `edit` behind the ones already queued.
    pub(crate) fn push(&self, edit: UnitEdit) {
        self.queue().push(edit);
    }
}

/// Applies the queue in order, from an exclusive system. A despawned unit's changes are dropped.
pub(crate) fn apply_unit_edits(world: &mut World) {
    let Some(edits) = world.get_resource::<UnitEdits>() else {
        return;
    };
    let edits = std::mem::take(&mut *edits.queue());
    apply(world, edits);
}

/// The pass's end: the queue as one command, so it applies at the sync point ahead of the systems
/// ordered after the pass, the readers of what it changes.
pub(crate) fn flush_unit_edits(edits: Res<UnitEdits>, mut commands: Commands) {
    let edits = std::mem::take(&mut *edits.queue());
    if !edits.is_empty() {
        commands.queue(move |world: &mut World| apply(world, edits));
    }
}

fn apply(world: &mut World, edits: Vec<UnitEdit>) {
    for edit in edits {
        match edit {
            UnitEdit::ReapCasting { entity, spell_id } => {
                if let Ok(mut unit) = world.get_entity_mut(entity) {
                    if unit
                        .get::<Casting>()
                        .is_some_and(|c| c.spell_id == spell_id)
                    {
                        unit.remove::<Casting>();
                    }
                }
            }
            UnitEdit::ArmAutoRepeat(entity) => {
                if let Ok(mut unit) = world.get_entity_mut(entity) {
                    unit.insert(AutoRepeatArmed);
                }
            }
            UnitEdit::ClearAutoRepeat { entity, spell_id } => {
                if let Ok(mut unit) = world.get_entity_mut(entity) {
                    unit.remove::<(AutoRepeatArmed, NockedAmmo, NockLatch)>();
                }
                precast::end(world, entity, Some(spell_id), precast::PrecastTail::Run);
            }
            UnitEdit::StartPrecast { entity, spell_id } => precast::start(world, entity, spell_id),
            UnitEdit::SendPrecast { entity, spell_id } => {
                precast::start_at_send(world, entity, spell_id);
            }
            UnitEdit::EndPrecast {
                entity,
                spell_id,
                tail,
            } => {
                let tail = if tail {
                    precast::PrecastTail::Run
                } else {
                    precast::PrecastTail::Skip
                };
                precast::end(world, entity, spell_id, tail);
            }
            UnitEdit::EndPrecastBusy(entity) => precast::end_busy(world, entity),
        }
    }
}
