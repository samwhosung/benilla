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

use bevy::ecs::entity::EntityHashMap;

use bevy::ecs::message::Messages;

use super::{
    precast, AutoRepeatArmed, CastEvent, CastEventKind, Casting, NockLatch, NockedAmmo, PlaySeq,
    Precast,
};

/// One queued change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnitEdit {
    /// The cast's reap (`0x614150(spellId, 0)`, keyed by the spell): its [`Casting`], while that
    /// is still this spell's.
    ReapCasting { entity: Entity, spell_id: u32 },
    /// `CancelCast`'s reap, from a seam with no message writer (StartAttack's, `0x5ecc85`): the
    /// record's spell as its tail keys and gates it ([`crate::spell::CastCore::cancel_reaping`]),
    /// and the failed cast event the server's echo would bring, stamped as it applies.
    AbortCasting { entity: Entity, spell_id: u32 },
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
            Self::ReapCasting { .. } | Self::AbortCasting { .. } => {
                vec![std::any::TypeId::of::<Casting>()]
            }
            Self::ArmAutoRepeat(_) => vec![std::any::TypeId::of::<AutoRepeatArmed>()],
            Self::ClearAutoRepeat { .. } => vec![
                std::any::TypeId::of::<AutoRepeatArmed>(),
                std::any::TypeId::of::<Precast>(),
                std::any::TypeId::of::<NockedAmmo>(),
                std::any::TypeId::of::<NockLatch>(),
            ],
            Self::StartPrecast { .. }
            | Self::SendPrecast { .. }
            | Self::EndPrecast { .. }
            | Self::EndPrecastBusy(_) => vec![std::any::TypeId::of::<Precast>()],
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
            Self::AbortCasting {
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
                | Self::AbortCasting { .. }
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

impl UnitEdit {
    /// The precast change this makes, on which unit.
    fn precast_change(&self) -> Option<(Entity, precast::Change)> {
        use precast::Change;
        match *self {
            Self::StartPrecast { entity, spell_id } => Some((entity, Change::Start(spell_id))),
            Self::SendPrecast { entity, spell_id } => Some((entity, Change::Send(spell_id))),
            Self::EndPrecast {
                entity, spell_id, ..
            } => Some((entity, Change::End(spell_id))),
            Self::ClearAutoRepeat { entity, spell_id } => {
                Some((entity, Change::End(Some(spell_id))))
            }
            Self::EndPrecastBusy(entity) => Some((entity, Change::Busy)),
            Self::ReapCasting { .. } | Self::AbortCasting { .. } | Self::ArmAutoRepeat(_) => None,
        }
    }
}

/// The queue, pushed through `&self` as the chat log is: its users run one at a time, in the VM
/// pass's order ([`crate::vm_pass::shared_sinks`]) or inside the net drain.
#[derive(Resource, Default)]
pub(crate) struct UnitEdits {
    queue: Mutex<Vec<UnitEdit>>,
    /// Every unit's [`Precast`] as applied, kept by the component's hooks.
    precasts: Mutex<EntityHashMap<Precast>>,
}

impl UnitEdits {
    fn queue(&self) -> MutexGuard<'_, Vec<UnitEdit>> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue `edit` behind the ones already queued.
    pub(crate) fn push(&self, edit: UnitEdit) {
        self.queue().push(edit);
    }

    /// The precast `entity` holds once the queue applies: the applied one with the queued changes
    /// replayed as they will apply. A link of the VM pass reads a unit's precast here, never off
    /// the component, which lags the edits queued ahead of it until the pass's end; that end hands
    /// the queue to its sync point with no link between.
    pub(crate) fn precast(&self, entity: Entity) -> Option<Precast> {
        let mut held = self
            .precasts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&entity)
            .copied();
        for (unit, change) in self.queue().iter().filter_map(UnitEdit::precast_change) {
            if unit == entity {
                held = change.after(held).0;
            }
        }
        held
    }

    /// The component's hooks: `entity` now holds `precast`.
    pub(super) fn mirror(&self, entity: Entity, precast: Option<Precast>) {
        let mut precasts = self.precasts.lock().unwrap_or_else(PoisonError::into_inner);
        match precast {
            Some(p) => precasts.insert(entity, p),
            None => precasts.remove(&entity),
        };
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

/// `0x614150(spellId, 0)`: `entity`'s [`Casting`], while it is still `spell_id`'s.
fn reap(world: &mut World, entity: Entity, spell_id: u32) {
    if let Ok(mut unit) = world.get_entity_mut(entity) {
        if unit
            .get::<Casting>()
            .is_some_and(|c| c.spell_id == spell_id)
        {
            unit.remove::<Casting>();
        }
    }
}

fn apply(world: &mut World, edits: Vec<UnitEdit>) {
    for edit in edits {
        match edit {
            UnitEdit::ReapCasting { entity, spell_id } => reap(world, entity, spell_id),
            UnitEdit::AbortCasting { entity, spell_id } => {
                reap(world, entity, spell_id);
                let seq = world.get_resource_mut::<PlaySeq>().map(|mut s| s.next());
                if let (Some(seq), Some(mut events)) =
                    (seq, world.get_resource_mut::<Messages<CastEvent>>())
                {
                    events.write(CastEvent {
                        entity,
                        spell_id,
                        kind: CastEventKind::Fail,
                        cast_flags: 0,
                        item_cast: false,
                        seq,
                    });
                }
            }
            UnitEdit::ArmAutoRepeat(entity) => {
                if let Ok(mut unit) = world.get_entity_mut(entity) {
                    unit.insert(AutoRepeatArmed);
                }
            }
            UnitEdit::ClearAutoRepeat { entity, .. } => {
                if let Ok(mut unit) = world.get_entity_mut(entity) {
                    unit.remove::<(AutoRepeatArmed, NockedAmmo, NockLatch)>();
                }
            }
            UnitEdit::StartPrecast { .. }
            | UnitEdit::SendPrecast { .. }
            | UnitEdit::EndPrecast { .. }
            | UnitEdit::EndPrecastBusy(_) => {}
        }
        if let Some((entity, change)) = edit.precast_change() {
            // `b` is set but at a GO its record refuses (`0x6e7fc5`).
            let tail = match edit {
                UnitEdit::EndPrecast { tail: false, .. } => precast::PrecastTail::Skip,
                _ => precast::PrecastTail::Run,
            };
            precast::apply(world, entity, change, tail);
        }
    }
}
