//! A unit's precast: the spell at `+0xc8c` and the unit-word bit `[+0xd58] & 0x400`, which
//! `0x6ec220` sets together through `0x60d020` (`0x6ec2cf`) once the spell resolves a visual
//! (`0x60d450`, else `0x6ec2a9` returns), instants included.
//!
//! It is started by the active player's send (`0x6e5898`, [`precast_at_send`]) and by
//! `SMSG_SPELL_START` (`0x6e7908` for another unit, `0x6e79be` for ours unless the pending record
//! holds the spell), through [`super::UnitEdit`]s, which apply here.
//!
//! Ended by `0x60d040` ([`end`]), which clears both when its spell is the unit's `+0xc8c`
//! (`0x60d049`) and then runs its tail, the active player's loot release among it:
//!
//! - keyed by the spell, at its GO (`0x6e7fc5`, the tail as `0x6e4870` answers), its
//!   `SMSG_SPELL_FAILED_OTHER` (`0x6e8e8c`), the player's failed cast result (`0x6e1c8e`) and
//!   the auto-repeat cancel (`0x6ea12b`), as edits;
//! - with the unit's own spell, so always, by a spline move (`0x6020e8`) and by the mover's own
//!   move messages and acks that `0x60e0a0` does not send at once (`0x60e15d`), through
//!   [`end_precast`].
//!
//! The bit alone is cleared by the local StartAttack (`0x5ecd87` → `0x60fc50`); `0x5fbd60`,
//! which clears it when a unit is re-shown, has no counterpart, as a unit that leaves is
//! despawned.

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;

use super::{UnitEdit, UnitEdits};

/// The unit's precast, present while `+0xc8c` holds a spell. The stand and sheathe gates read
/// the spell (`0x5ed4cc`, `0x5eb52b`), the emote gate the bit (`0x5fcd8b`). Its hooks mirror it
/// into the queue, which answers it for the VM pass ([`UnitEdits::precast`]).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
#[component(on_insert = mirror_insert, on_replace = mirror_replace)]
pub(crate) struct Precast {
    /// `+0xc8c`, never 0.
    pub(crate) spell_id: u32,
    /// `[+0xd58] & 0x400`, cleared alone by `0x60fc50`, which leaves the spell.
    pub(crate) busy: bool,
}

/// `SendCast`'s precast for the active player's send (`0x6e5877`-`0x6e5898`): a spell other
/// than the unit's `+0xc8c` (`0x6e588f`) starts it, if it resolves a visual
/// ([`super::PrecastVisuals`]). The flag `0x6e5885` tests is set only by the trade slot's bind
/// (`0x6e694f`), which benilla does not build, so every send reaches it.
pub(crate) fn precast_at_send(
    player: Option<Entity>,
    spell_id: u32,
    edits: &super::UnitEdits,
    visuals: &super::PrecastVisuals,
) {
    if let Some(entity) = player.filter(|&e| visuals.resolves(e, spell_id)) {
        edits.push(UnitEdit::SendPrecast { entity, spell_id });
    }
}

fn mirror_insert(world: DeferredWorld, ctx: HookContext) {
    if let (Some(&precast), Some(edits)) = (
        world.get::<Precast>(ctx.entity),
        world.get_resource::<UnitEdits>(),
    ) {
        edits.mirror(ctx.entity, Some(precast));
    }
}

fn mirror_replace(world: DeferredWorld, ctx: HookContext) {
    if let Some(edits) = world.get_resource::<UnitEdits>() {
        edits.mirror(ctx.entity, None);
    }
}

/// One change to a unit's precast, as the queue applies it and as it answers it.
#[derive(Clone, Copy, Debug)]
pub(super) enum Change {
    /// `0x6ec220`'s state half, `0x60d020`, for a spell its pusher found a visual for.
    Start(u32),
    /// The send's start, which a spell the unit already precasts skips (`0x6e588f`), even once
    /// `0x60fc50` has cleared its bit.
    Send(u32),
    /// `0x60d040`, keyed by the spell, or with the unit's own for `None`, which matches an empty
    /// `+0xc8c` too (`0x60d049`).
    End(Option<u32>),
    /// `0x60fc50`'s clear of the bit alone.
    Busy,
}

impl Change {
    /// The precast `held` becomes, and whether this was an end that matched.
    pub(super) fn after(self, held: Option<Precast>) -> (Option<Precast>, bool) {
        let spell = held.map(|p| p.spell_id);
        let started = |spell_id| Precast {
            spell_id,
            busy: true,
        };
        match self {
            Change::Start(s) => (Some(started(s)), false),
            Change::Send(s) if spell == Some(s) => (held, false),
            Change::Send(s) => (Some(started(s)), false),
            Change::End(Some(s)) if spell != Some(s) => (held, false),
            Change::End(_) => (None, true),
            Change::Busy => (held.map(|p| Precast { busy: false, ..p }), false),
        }
    }
}

/// Applies `change` to `entity`'s precast, and the tail of an end that matched ([`end`]).
pub(super) fn apply(world: &mut World, entity: Entity, change: Change, tail: PrecastTail) {
    let Ok(mut unit) = world.get_entity_mut(entity) else {
        return;
    };
    let held = unit.get::<Precast>().copied();
    let (after, ended) = change.after(held);
    if after != held {
        match after {
            Some(precast) => unit.insert(precast),
            None => unit.remove::<Precast>(),
        };
    }
    if ended {
        run_tail(world, entity, tail);
    }
}

/// `0x60d040`'s `b` argument: whether a matched end runs its tail, whose last step releases the
/// active player's loot window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrecastTail {
    /// `b` clear: the clear alone.
    Skip,
    /// The tail, its release reading the player's movement flags as the tail does (`0x60d0f7`).
    Run,
    /// The tail, its release told whether the unit moves: a spline's end, whose install has just
    /// written the flags.
    Moving(bool),
}

/// An end with the unit's own spell, `0x60d040(+0xc8c, 1, 1)`, from a system that holds
/// `Commands`: a spline move (`0x6020e8`) and the mover's move messages and acks (`0x60e15d`).
pub(crate) fn end_precast(commands: &mut Commands, entity: Entity, tail: PrecastTail) {
    commands.queue(move |world: &mut World| end(world, entity, None, tail));
}

/// `0x60d040`: keyed by `spell`, or with the unit's own spell for `None`, which matches an empty
/// `+0xc8c` too (`0x60d049`). A matched end clears the precast and, unless `tail` skips it, runs
/// the tail, whose last step releases the active player's loot window (`0x60d0ee`-`0x60d10b`):
/// `0x48f200(1, moving, 0)`, `moving` its flags `& 0x200f`, the four directions and FALLING.
/// Moving, that is the move-start close ([`crate::ui_loot::LootMoveStart`]); at rest the full one
/// ([`crate::ui_loot::LootCloseAtRest`]). An end made in the VM pass applies after it, so its
/// release reaches the loot drain the next frame.
pub(super) fn end(world: &mut World, entity: Entity, spell: Option<u32>, tail: PrecastTail) {
    apply(world, entity, Change::End(spell), tail);
}

fn run_tail(world: &mut World, entity: Entity, tail: PrecastTail) {
    let active_player = world
        .get_entity(entity)
        .is_ok_and(|unit| unit.contains::<crate::net::SelfPlayer>());
    let moving = match tail {
        PrecastTail::Skip => return,
        PrecastTail::Moving(moving) => moving,
        PrecastTail::Run => world
            .get_resource::<crate::player::Player>()
            .is_some_and(|p| p.move_flags() & MOVING != 0),
    };
    if !active_player {
        return;
    }
    if moving {
        if let Some(mut close) = world.get_resource_mut::<crate::ui_loot::LootMoveStart>() {
            close.0 = true;
        }
    } else if let Some(mut close) = world.get_resource_mut::<crate::ui_loot::LootCloseAtRest>() {
        close.0 = true;
    }
}

/// The flags the tail's release reads as moving (`0x60d0fd`).
const MOVING: u32 = super::move_flags::FORWARD
    | super::move_flags::BACKWARD
    | super::move_flags::STRAFE_LEFT
    | super::move_flags::STRAFE_RIGHT
    | super::move_flags::FALLING;

/// Installs a spell catalog whose `with_visual` spells resolve a `SpellVisual` row and whose
/// `without` spells name none, for the tests that start a precast.
#[cfg(test)]
pub(crate) fn install_visuals_for_tests(world: &mut World, with_visual: &[u32], without: &[u32]) {
    use std::collections::HashMap;

    use benilla_formats::{SpellCatalog, SpellDisplay, SpellVisualCatalog, VisualStages};

    const VISUAL: u32 = 130;
    let display = |visual| SpellDisplay {
        visual,
        ..Default::default()
    };
    let displays = with_visual
        .iter()
        .map(|&id| (id, display(VISUAL)))
        .chain(without.iter().map(|&id| (id, display(0))))
        .collect();
    world.insert_resource(super::SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(VISUAL, VisualStages::default())]),
        HashMap::new(),
    )));
    world.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(displays),
        ..crate::ui_action::Spells::empty_for_tests()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creature_anim::{apply_unit_edits, UnitEdits};

    /// Fireball: an own `SpellVisual` row.
    const VISUAL_SPELL: u32 = 133;
    /// Frostbolt, given a row as well.
    const OTHER_VISUAL_SPELL: u32 = 116;
    /// A spell whose row names no visual.
    const BARE_SPELL: u32 = 7;

    fn world() -> (World, Entity) {
        let mut world = World::new();
        world.init_resource::<UnitEdits>();
        install_visuals_for_tests(
            &mut world,
            &[VISUAL_SPELL, OTHER_VISUAL_SPELL],
            &[BARE_SPELL],
        );
        let unit = world.spawn_empty().id();
        (world, unit)
    }

    fn apply(world: &mut World, edit: UnitEdit) {
        world.resource::<UnitEdits>().push(edit);
        apply_unit_edits(world);
    }

    fn precast(world: &World, unit: Entity) -> Option<Precast> {
        world.get::<Precast>(unit).copied()
    }

    /// `0x6ec220` starts the precast only for a spell that resolves a visual (`0x6ec2a9`, else
    /// `0x60d020` at `0x6ec2cf`), decided at the send, which queues nothing without one.
    #[test]
    fn a_spell_with_a_visual_precasts_and_one_without_does_not() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut world, unit) = world();
        let send = |world: &mut World, spell_id: u32| {
            world
                .run_system_once(
                    move |edits: Res<UnitEdits>, visuals: super::super::PrecastVisuals| {
                        precast_at_send(Some(unit), spell_id, &edits, &visuals);
                    },
                )
                .unwrap();
            apply_unit_edits(world);
        };
        send(&mut world, BARE_SPELL);
        assert_eq!(precast(&world, unit), None, "no visual, no precast");
        send(&mut world, VISUAL_SPELL);
        assert_eq!(
            precast(&world, unit),
            Some(Precast {
                spell_id: VISUAL_SPELL,
                busy: true
            })
        );
        apply(
            &mut world,
            UnitEdit::StartPrecast {
                entity: unit,
                spell_id: OTHER_VISUAL_SPELL,
            },
        );
        assert_eq!(
            precast(&world, unit).map(|p| p.spell_id),
            Some(OTHER_VISUAL_SPELL),
            "a start replaces the spell"
        );
    }

    /// The send starts nothing for the spell the unit already precasts (`0x6e588f`), so a bit
    /// StartAttack cleared (`0x60fc50`, the spell kept) stays clear; another spell sets it.
    #[test]
    fn a_resend_of_the_precast_spell_leaves_its_cleared_bit_clear() {
        let (mut world, unit) = world();
        let send = |spell_id| UnitEdit::SendPrecast {
            entity: unit,
            spell_id,
        };
        apply(&mut world, send(VISUAL_SPELL));
        apply(&mut world, UnitEdit::EndPrecastBusy(unit));
        assert_eq!(
            precast(&world, unit),
            Some(Precast {
                spell_id: VISUAL_SPELL,
                busy: false
            }),
            "the bit alone clears"
        );
        apply(&mut world, send(VISUAL_SPELL));
        assert_eq!(precast(&world, unit).map(|p| p.busy), Some(false));
        apply(&mut world, send(OTHER_VISUAL_SPELL));
        assert_eq!(
            precast(&world, unit),
            Some(Precast {
                spell_id: OTHER_VISUAL_SPELL,
                busy: true
            })
        );
    }

    /// `0x60d040` clears only when its spell is the unit's (`0x60d049`): a keyed end of another
    /// spell spares it, the unit's own spell ends it, and so does the auto-repeat cancel's for the
    /// repeat spell (`0x6ea12b`).
    #[test]
    fn the_keyed_end_spares_another_spells_precast() {
        let (mut world, unit) = world();
        let start = UnitEdit::StartPrecast {
            entity: unit,
            spell_id: VISUAL_SPELL,
        };
        let end = |spell_id| UnitEdit::EndPrecast {
            entity: unit,
            spell_id,
            tail: true,
        };
        apply(&mut world, start);
        apply(&mut world, end(Some(OTHER_VISUAL_SPELL)));
        assert!(precast(&world, unit).is_some(), "another spell's end");
        apply(&mut world, end(Some(VISUAL_SPELL)));
        assert_eq!(precast(&world, unit), None, "its own spell's end");
        apply(&mut world, start);
        apply(&mut world, end(None));
        assert_eq!(
            precast(&world, unit),
            None,
            "an end with the unit's own spell"
        );
        apply(&mut world, start);
        apply(
            &mut world,
            UnitEdit::ClearAutoRepeat {
                entity: unit,
                spell_id: OTHER_VISUAL_SPELL,
            },
        );
        assert!(
            precast(&world, unit).is_some(),
            "another repeat spell's cancel"
        );
        apply(
            &mut world,
            UnitEdit::ClearAutoRepeat {
                entity: unit,
                spell_id: VISUAL_SPELL,
            },
        );
        assert_eq!(precast(&world, unit), None, "the repeat spell's cancel");
    }

    /// What an end owes the active player's loot window: (`LootMoveStart`, `LootCloseAtRest`).
    fn owed(world: &mut World) -> (bool, bool) {
        (
            std::mem::take(&mut world.resource_mut::<crate::ui_loot::LootMoveStart>().0),
            std::mem::take(&mut world.resource_mut::<crate::ui_loot::LootCloseAtRest>().0),
        )
    }

    /// `0x60d040`'s tail ends in the active player's loot release (`0x60d0ee`-`0x60d10b`):
    /// `0x48f200(1, moving, 0)`, `moving` its flags `& 0x200f` (`0x60d0fd`), the four directions
    /// and FALLING, so the move-start close while it moves and the full close at rest. Only a
    /// matched end (`0x60d04f`) with `b` set (`0x60d076`) reaches it, and only for the active
    /// player (`0x5fa6d0`); an end with the unit's own spell matches an empty `+0xc8c` too.
    #[test]
    fn an_end_releases_the_active_players_loot_as_it_moves_or_rests() {
        use crate::creature_anim::move_flags::{FALLING, FORWARD, STRAFE_LEFT, SWIMMING};
        let (mut world, me) = world();
        world.entity_mut(me).insert(crate::net::SelfPlayer);
        let other = world.spawn_empty().id();
        world.init_resource::<crate::ui_loot::LootMoveStart>();
        world.init_resource::<crate::ui_loot::LootCloseAtRest>();
        fn case(
            world: &mut World,
            flags: u32,
            unit: Entity,
            held: Option<u32>,
            spell: Option<u32>,
            tail: PrecastTail,
        ) -> (bool, bool) {
            world.insert_resource(crate::player::Player::with_move_flags(flags));
            world.entity_mut(unit).remove::<Precast>();
            if let Some(spell_id) = held {
                world.entity_mut(unit).insert(Precast {
                    spell_id,
                    busy: true,
                });
            }
            end(world, unit, spell, tail);
            owed(world)
        }
        let keyed = Some(VISUAL_SPELL);
        let held = Some(VISUAL_SPELL);
        let run = PrecastTail::Run;
        assert_eq!(
            case(&mut world, FORWARD, me, held, keyed, run),
            (true, false),
            "running"
        );
        assert_eq!(
            case(&mut world, STRAFE_LEFT, me, held, keyed, run),
            (true, false),
            "strafing"
        );
        assert_eq!(
            case(&mut world, FALLING, me, held, keyed, run),
            (true, false),
            "falling"
        );
        assert_eq!(
            case(&mut world, 0, me, held, keyed, run),
            (false, true),
            "at rest"
        );
        assert_eq!(
            case(&mut world, SWIMMING, me, held, keyed, run),
            (false, true),
            "afloat and still: 0x200f has no swim bit"
        );
        assert_eq!(
            case(&mut world, 0, me, held, Some(OTHER_VISUAL_SPELL), run),
            (false, false),
            "another spell's end"
        );
        assert_eq!(
            case(&mut world, 0, me, None, keyed, run),
            (false, false),
            "a keyed end with nothing held"
        );
        assert_eq!(
            case(&mut world, 0, me, None, None, run),
            (false, true),
            "the unit's own spell matches an empty +0xc8c"
        );
        assert_eq!(
            case(&mut world, 0, me, held, keyed, PrecastTail::Skip),
            (false, false),
            "b clear"
        );
        assert_eq!(precast(&world, me), None, "b clear still ends it");
        assert_eq!(
            case(&mut world, 0, me, held, None, PrecastTail::Moving(true)),
            (true, false),
            "a spline's path"
        );
        assert_eq!(
            case(
                &mut world,
                FORWARD,
                me,
                held,
                None,
                PrecastTail::Moving(false)
            ),
            (false, true),
            "a refused path"
        );
        assert_eq!(
            case(&mut world, 0, other, held, keyed, run),
            (false, false),
            "another unit"
        );

        // The edits carry it: the GO's `b`, a failure's and the auto-repeat cancel's set one.
        world.insert_resource(crate::player::Player::with_move_flags(0));
        world.entity_mut(me).insert(Precast {
            spell_id: VISUAL_SPELL,
            busy: true,
        });
        apply(
            &mut world,
            UnitEdit::EndPrecast {
                entity: me,
                spell_id: keyed,
                tail: false,
            },
        );
        assert_eq!(owed(&mut world), (false, false), "an edit with b clear");
        world.entity_mut(me).insert(Precast {
            spell_id: VISUAL_SPELL,
            busy: true,
        });
        apply(
            &mut world,
            UnitEdit::ClearAutoRepeat {
                entity: me,
                spell_id: VISUAL_SPELL,
            },
        );
        assert_eq!(
            owed(&mut world),
            (false, true),
            "the auto-repeat cancel (0x6ea12b)"
        );
    }

    /// The queue answers a unit's precast as it stands once the queue applies
    /// ([`UnitEdits::precast`]): the applied one, through every insert, removal and despawn of
    /// the component, with the queued changes replayed in order as they will apply, so a link
    /// of the VM pass reads what the edits queued ahead of it make.
    #[test]
    fn the_queue_answers_the_precast_its_edits_will_leave() {
        let (mut world, unit) = world();
        let other = world.spawn_empty().id();
        let view = |world: &World| world.resource::<UnitEdits>().precast(unit);
        let push = |world: &World, edit| world.resource::<UnitEdits>().push(edit);
        let held = |spell_id, busy| Some(Precast { spell_id, busy });
        assert_eq!(view(&world), None);
        world.entity_mut(unit).insert(Precast {
            spell_id: VISUAL_SPELL,
            busy: true,
        });
        assert_eq!(
            view(&world),
            held(VISUAL_SPELL, true),
            "the applied precast"
        );
        let send = |spell_id| UnitEdit::SendPrecast {
            entity: unit,
            spell_id,
        };
        push(&world, UnitEdit::EndPrecastBusy(unit));
        push(&world, send(VISUAL_SPELL));
        assert_eq!(
            view(&world),
            held(VISUAL_SPELL, false),
            "the bit's clear, and a resend of its spell starts nothing (0x6e588f)"
        );
        push(&world, send(OTHER_VISUAL_SPELL));
        assert_eq!(view(&world), held(OTHER_VISUAL_SPELL, true));
        push(
            &world,
            UnitEdit::EndPrecast {
                entity: unit,
                spell_id: Some(VISUAL_SPELL),
                tail: true,
            },
        );
        push(
            &world,
            UnitEdit::EndPrecast {
                entity: other,
                spell_id: None,
                tail: true,
            },
        );
        assert_eq!(
            view(&world),
            held(OTHER_VISUAL_SPELL, true),
            "another spell's end and another unit's"
        );
        push(
            &world,
            UnitEdit::ClearAutoRepeat {
                entity: unit,
                spell_id: OTHER_VISUAL_SPELL,
            },
        );
        assert_eq!(view(&world), None, "the repeat spell's cancel");
        push(
            &world,
            UnitEdit::StartPrecast {
                entity: unit,
                spell_id: VISUAL_SPELL,
            },
        );
        let answered = view(&world);
        assert_eq!(answered, held(VISUAL_SPELL, true));
        apply_unit_edits(&mut world);
        assert_eq!(
            precast(&world, unit),
            answered,
            "the queue applies as it answered"
        );
        assert_eq!(view(&world), answered);

        world.entity_mut(unit).remove::<Precast>();
        assert_eq!(view(&world), None, "a removal");
        world.entity_mut(unit).insert(Precast {
            spell_id: VISUAL_SPELL,
            busy: true,
        });
        world.despawn(unit);
        assert_eq!(view(&world), None, "a despawn");
    }
}
