//! The cast, attack and auto-repeat state a press, a script call or a cast result changes on a
//! unit: the cast's [`Casting`], its [`Precast`], the attack target [`Engaged`] and the
//! auto-repeat markers. They are components, which a system adds or
//! removes only through a sync point, so the seams that change them (`SelfCancel`, `CastLadder`,
//! `AttackSeam`) queue the change here and hold no `Commands`: the call host they sit in carries
//! no deferred buffer, and the VM pass's hosts need no sync point between them.
//! An exclusive caller applies the queue where it applies the commands of what it runs
//! ([`apply_unit_edits`]): the net drain after each handler, the world's hosted entries and
//! one-shots as they return ([`crate::script_calls::with_hosted_vm`],
//! [`crate::script_calls::run_cached`]). The pass's end set, and a flush in each other schedule
//! that can queue one, hand it to the sync point ahead of the readers ([`flush_unit_edits`]).
//! The same queue carries the console commands a call runs against the world, which no host
//! lends ([`UnitEdits::push_world`]).

use std::sync::{Mutex, MutexGuard, PoisonError};

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;

use bevy::ecs::entity::EntityHashMap;

use bevy::ecs::message::Messages;

use super::{
    precast, AutoRepeatArmed, CastEvent, CastEventKind, Casting, Engaged, NockLatch, NockedAmmo,
    PlaySeq, Precast,
};

/// One queued change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnitEdit {
    /// The cast's reap (`0x614150(spellId, 0)`, keyed by the spell): its [`Casting`], while that
    /// is still this spell's.
    ReapCasting { entity: Entity, spell_id: u32 },
    /// `CancelCast`'s reap (`0x614150` at `0x6e4ab7`): the record's spell as its tail keys and
    /// gates it ([`crate::spell::CastCore::cancel_reaping`]), and the failed cast event the
    /// server's echo would bring, stamped as it applies.
    AbortCasting { entity: Entity, spell_id: u32 },
    /// `0x614150(spell, 0)` with no cast edge of its own, as the aura cancel stops the cancelled
    /// spell's kits (`0x6e707e`): its [`Casting`] while that is still this spell's, and a
    /// [`CastEventKind::Reap`].
    StopSpellKits { entity: Entity, spell_id: u32 },
    /// The sticky auto-repeat arm (`0x6e593b`, `|= 0x200`).
    ArmAutoRepeat(Entity),
    /// The local auto-repeat stop's unit leg (`0x6ea080`): the arm (`0x6ea113`), the precast of
    /// the repeat spell `spell_id` (`0x6ea12b`), that spell's kits (`0x614150(spell, 0)` at
    /// `0x6ea139`: its [`Casting`] and a [`CastEventKind::Reap`]) and the nock (`0x6ea140`).
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
    /// StartAttack's send stores the victim as the attack target (`0x5ecd51` → `0x47bf70`:
    /// `[+0xc48]` the guid, `[+0xc50]` 1), ahead of the server's echo.
    StartAttack { entity: Entity, target: u64 },
    /// A unit's death zeroes its attack target (`0x605896`-`0x6058a2`).
    EndAttack(Entity),
}

impl UnitEdit {
    /// The components this change writes.
    #[cfg(test)]
    pub(crate) fn writes(&self) -> Vec<std::any::TypeId> {
        match self {
            Self::ReapCasting { .. } | Self::AbortCasting { .. } | Self::StopSpellKits { .. } => {
                vec![std::any::TypeId::of::<Casting>()]
            }
            Self::ArmAutoRepeat(_) => vec![std::any::TypeId::of::<AutoRepeatArmed>()],
            Self::ClearAutoRepeat { .. } => vec![
                std::any::TypeId::of::<AutoRepeatArmed>(),
                std::any::TypeId::of::<Precast>(),
                std::any::TypeId::of::<Casting>(),
                std::any::TypeId::of::<NockedAmmo>(),
                std::any::TypeId::of::<NockLatch>(),
            ],
            Self::StartPrecast { .. }
            | Self::SendPrecast { .. }
            | Self::EndPrecast { .. }
            | Self::EndPrecastBusy(_) => vec![std::any::TypeId::of::<Precast>()],
            Self::StartAttack { .. } | Self::EndAttack(_) => {
                vec![std::any::TypeId::of::<Engaged>()]
            }
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
            Self::StopSpellKits {
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
            Self::StartAttack { entity, target: 0 },
            Self::EndAttack(entity),
        ];
        // A new kind stops this match compiling until it is listed above as well.
        for edit in &every {
            match edit {
                Self::ReapCasting { .. }
                | Self::AbortCasting { .. }
                | Self::StopSpellKits { .. }
                | Self::ArmAutoRepeat(_)
                | Self::ClearAutoRepeat { .. }
                | Self::StartPrecast { .. }
                | Self::SendPrecast { .. }
                | Self::EndPrecast { .. }
                | Self::EndPrecastBusy(_)
                | Self::StartAttack { .. }
                | Self::EndAttack(_) => {}
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
            Self::ReapCasting { .. }
            | Self::AbortCasting { .. }
            | Self::StopSpellKits { .. }
            | Self::ArmAutoRepeat(_)
            | Self::StartAttack { .. }
            | Self::EndAttack(_) => None,
        }
    }

    /// Whether this is a precast end with its tail, which closes the active player's loot window
    /// when it matches: an [`Self::EndPrecast`] with `b` set and the auto-repeat stop's
    /// (`0x6ea12b`, `b` 1).
    fn runs_a_tail(&self) -> bool {
        matches!(
            self,
            Self::EndPrecast { tail: true, .. } | Self::ClearAutoRepeat { .. }
        )
    }
}

/// One queued change with the place it was made, for a precast end whose tail may close the loot
/// window: the close's signals go there ([`precast::end`]).
type Queued = (UnitEdit, Option<crate::lanes::Stamp>);

/// The queue, pushed through `&self` as the chat log is: its users run one at a time, in the VM
/// pass's order ([`crate::vm_pass::shared_sinks`]) or inside the net drain.
#[derive(Resource, Default)]
pub(crate) struct UnitEdits {
    queue: Mutex<Vec<Queued>>,
    /// Every unit's [`Precast`] as applied, kept by the component's hooks.
    precasts: Mutex<EntityHashMap<Precast>>,
    /// Every unit's [`Engaged`] target as applied, kept by the component's hooks.
    attacks: Mutex<EntityHashMap<u64>>,
    /// What a call runs against the world, after the unit edits, in the order queued.
    world_runs: Mutex<Vec<WorldRun>>,
}

/// A call's run against the world ([`UnitEdits::push_world`]).
pub(crate) type WorldRun = Box<dyn FnOnce(&mut World) + Send>;

impl UnitEdits {
    fn queue(&self) -> MutexGuard<'_, Vec<Queued>> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue `edit` behind the ones already queued; a precast end that runs its tail takes its
    /// place here, where its caller made it.
    pub(crate) fn push(&self, edit: UnitEdit) {
        let at = edit.runs_a_tail().then(crate::lanes::stamp);
        self.queue().push((edit, at));
    }

    /// Queue `run` against the world, after the unit edits: a console command that writes engine
    /// state no host lends ([`crate::console::ConsoleHandler`]).
    pub(crate) fn push_world(&self, run: WorldRun) {
        self.runs().push(run);
    }

    fn runs(&self) -> MutexGuard<'_, Vec<WorldRun>> {
        self.world_runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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
        for (unit, change) in self.queue().iter().filter_map(|(e, _)| e.precast_change()) {
            if unit == entity {
                held = change.after(held).0;
            }
        }
        held
    }

    /// The attack target `entity` holds once the queue applies, `[+0xc48]` (`HasAttackTarget`
    /// `0x60ecb0` tests it): the applied [`Engaged`] with the queued changes replayed. A link of
    /// the VM pass reads it here, never off the component, so a swing started earlier in the pass
    /// reads as started at once, as the reference's local store makes it (`0x5ecd51`).
    pub(crate) fn attack_target(&self, entity: Entity) -> Option<u64> {
        let mut held = self
            .attacks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&entity)
            .copied();
        for (edit, _) in self.queue().iter() {
            match *edit {
                UnitEdit::StartAttack { entity: e, target } if e == entity => held = Some(target),
                UnitEdit::EndAttack(e) if e == entity => held = None,
                _ => {}
            }
        }
        held
    }

    /// Whether `entity` is attacking once the queue applies ([`Self::attack_target`]).
    pub(crate) fn attacking(&self, entity: Entity) -> bool {
        self.attack_target(entity).is_some()
    }

    /// [`Engaged`]'s hooks: `entity` now holds `target`.
    pub(super) fn mirror_attack(&self, entity: Entity, target: Option<u64>) {
        let mut attacks = self.attacks.lock().unwrap_or_else(PoisonError::into_inner);
        match target {
            Some(t) => attacks.insert(entity, t),
            None => attacks.remove(&entity),
        };
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

/// [`Engaged`]'s insert hook: the queue's mirror holds the target.
pub(super) fn engaged_inserted(world: DeferredWorld, ctx: HookContext) {
    if let (Some(&Engaged(target)), Some(edits)) = (
        world.get::<Engaged>(ctx.entity),
        world.get_resource::<UnitEdits>(),
    ) {
        edits.mirror_attack(ctx.entity, Some(target));
    }
}

/// [`Engaged`]'s replace hook, ahead of a re-insert or a removal: the mirror drops it.
pub(super) fn engaged_replaced(world: DeferredWorld, ctx: HookContext) {
    if let Some(edits) = world.get_resource::<UnitEdits>() {
        edits.mirror_attack(ctx.entity, None);
    }
}

/// Applies the queue in order, from an exclusive system. A despawned unit's changes are dropped.
pub(crate) fn apply_unit_edits(world: &mut World) {
    let Some(queued) = world.get_resource::<UnitEdits>() else {
        return;
    };
    let edits = std::mem::take(&mut *queued.queue());
    let runs = std::mem::take(&mut *queued.runs());
    apply(world, edits);
    run_all(world, runs);
}

/// The pass's end: the queue as one command, so it applies at the sync point ahead of the systems
/// ordered after the pass, the readers of what it changes.
pub(crate) fn flush_unit_edits(queued: Res<UnitEdits>, mut commands: Commands) {
    let edits = std::mem::take(&mut *queued.queue());
    let runs = std::mem::take(&mut *queued.runs());
    if !edits.is_empty() || !runs.is_empty() {
        commands.queue(move |world: &mut World| {
            apply(world, edits);
            run_all(world, runs);
        });
    }
}

/// The resources the edits write as they apply, beside the units' own components: the loot
/// close a precast end's tail makes for the active player (`0x60d0ee`-`0x60d10b`), the window,
/// its latch and the dead source's deselect ([`crate::ui_loot::close_at_precast_end`]), and the
/// cast edges with their stamps. A handler's edits apply after it in the net drain, by an
/// exclusive system whose writes Bevy's access does not show, so the lanes' guards count these as
/// what a handler writes.
#[cfg(test)]
pub(crate) fn applied_resources() -> [std::any::TypeId; 5] {
    use std::any::TypeId;
    [
        TypeId::of::<crate::ui_loot::LootState>(),
        TypeId::of::<crate::ui_loot::LootLatch>(),
        TypeId::of::<crate::target::OwedSelects>(),
        TypeId::of::<PlaySeq>(),
        TypeId::of::<Messages<CastEvent>>(),
    ]
}

fn run_all(world: &mut World, runs: Vec<WorldRun>) {
    for run in runs {
        run(world);
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

fn apply(world: &mut World, edits: Vec<Queued>) {
    for (edit, at) in edits {
        match edit {
            UnitEdit::ReapCasting { entity, spell_id } => reap(world, entity, spell_id),
            UnitEdit::AbortCasting { entity, spell_id } => {
                reap(world, entity, spell_id);
                cast_event(world, entity, spell_id, CastEventKind::Fail);
            }
            UnitEdit::StopSpellKits { entity, spell_id } => {
                reap(world, entity, spell_id);
                cast_event(world, entity, spell_id, CastEventKind::Reap);
            }
            UnitEdit::ArmAutoRepeat(entity) => {
                if let Ok(mut unit) = world.get_entity_mut(entity) {
                    unit.insert(AutoRepeatArmed);
                }
            }
            UnitEdit::ClearAutoRepeat { entity, .. } => {
                if let Ok(mut unit) = world.get_entity_mut(entity) {
                    unit.remove::<AutoRepeatArmed>();
                }
            }
            UnitEdit::StartAttack { entity, target } => {
                if let Ok(mut unit) = world.get_entity_mut(entity) {
                    unit.insert(Engaged(target));
                }
            }
            UnitEdit::EndAttack(entity) => {
                if let Ok(mut unit) = world.get_entity_mut(entity) {
                    unit.remove::<Engaged>();
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
            precast::apply(world, entity, change, tail, at);
        }
        // Past the stop's precast end: its kit reap (`0x6ea139`) and the un-nock (`0x6ea140`).
        if let UnitEdit::ClearAutoRepeat { entity, spell_id } = edit {
            reap(world, entity, spell_id);
            cast_event(world, entity, spell_id, CastEventKind::Reap);
            if let Ok(mut unit) = world.get_entity_mut(entity) {
                unit.remove::<(NockedAmmo, NockLatch)>();
            }
        }
    }
}

/// A cast edge the local change makes, stamped as it applies.
fn cast_event(world: &mut World, entity: Entity, spell_id: u32, kind: CastEventKind) {
    let seq = world.get_resource_mut::<PlaySeq>().map(|mut s| s.next());
    if let (Some(seq), Some(mut events)) = (seq, world.get_resource_mut::<Messages<CastEvent>>()) {
        events.write(CastEvent {
            entity,
            spell_id,
            kind,
            seq,
            at: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::component::ComponentId;

    use super::*;

    /// Every resource an edit writes as it applies is one [`applied_resources`] names, so the
    /// guards that follow what a handler writes see it: each kind applied to the active player
    /// with a precast, at rest and moving, a dead unit's loot window open, which the tail closes.
    #[test]
    fn the_edits_write_no_resource_but_the_applied_ones() {
        const SPELL: u32 = 133;
        const CORPSE: u64 = 0xF130_0000_0000_0042;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        let world = app.world_mut();
        super::super::precast::install_visuals_for_tests(world, &[SPELL], &[]);
        let me = world.spawn(crate::net::SelfPlayer).id();
        let corpse = world
            .spawn((
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                crate::net::ObjectStore(
                    benilla_protocol::messages::ObjectFields::from_pairs(&[(
                        benilla_protocol::field::FIELD_UNIT_HEALTH,
                        0,
                    )])
                    .into_created(benilla_protocol::messages::ObjectType::Unit),
                ),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(CORPSE, corpse);
        // The close's one-shot registers itself on its first run, which is bookkeeping, not state.
        crate::ui_loot::close_at_precast_end(world, false);
        let declared: Vec<ComponentId> = applied_resources()
            .into_iter()
            .filter_map(|t| world.components().get_resource_id(t))
            .collect();
        assert_eq!(
            declared.len(),
            applied_resources().len(),
            "the client holds every applied resource"
        );
        let edits = [
            UnitEdit::StartPrecast {
                entity: me,
                spell_id: SPELL,
            },
            UnitEdit::EndPrecastBusy(me),
            UnitEdit::EndPrecast {
                entity: me,
                spell_id: Some(SPELL),
                tail: true,
            },
            UnitEdit::SendPrecast {
                entity: me,
                spell_id: SPELL,
            },
            UnitEdit::EndPrecast {
                entity: me,
                spell_id: None,
                tail: true,
            },
            UnitEdit::ArmAutoRepeat(me),
            UnitEdit::StartPrecast {
                entity: me,
                spell_id: SPELL,
            },
            UnitEdit::ClearAutoRepeat {
                entity: me,
                spell_id: SPELL,
            },
            UnitEdit::ReapCasting {
                entity: me,
                spell_id: SPELL,
            },
            UnitEdit::AbortCasting {
                entity: me,
                spell_id: SPELL,
            },
            UnitEdit::StopSpellKits {
                entity: me,
                spell_id: SPELL,
            },
            UnitEdit::StartAttack {
                entity: me,
                target: 0x2,
            },
            UnitEdit::EndAttack(me),
        ];
        let mut written = std::collections::BTreeSet::new();
        let mut owed = std::collections::BTreeSet::new();
        for flags in [0, crate::creature_anim::move_flags::FORWARD] {
            world.insert_resource(crate::player::Player::with_move_flags(flags));
            world.resource_mut::<crate::ui_loot::LootState>().open(
                CORPSE,
                benilla_protocol::messages::loot_type::CORPSE,
                5,
                vec![],
            );
            world.resource_mut::<crate::ui_loot::LootLatch>().0 = Some(CORPSE);
            let last = world.increment_change_tick();
            for edit in edits {
                world.resource::<UnitEdits>().push(edit);
            }
            apply_unit_edits(world);
            let now = world.change_tick();
            let resources: Vec<(ComponentId, String)> = world
                .iter_resources()
                .map(|(info, _)| (info.id(), info.name().to_string()))
                .collect();
            for (id, name) in resources {
                let changed = world
                    .get_resource_change_ticks_by_id(id)
                    .is_some_and(|t| t.is_changed(last, now));
                if changed && !declared.contains(&id) {
                    written.insert(name);
                } else if changed {
                    owed.insert(id);
                }
            }
        }
        assert!(written.is_empty(), "written but not declared: {written:?}");
        // By id: without the `debug` feature every resource shares one placeholder name.
        assert_eq!(owed.len(), declared.len(), "each applied resource written");
    }
}
