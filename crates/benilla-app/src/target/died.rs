//! `CGUnit_C::OnDied 0x605860`, which the health watcher runs on every unit's alive-to-dead edge
//! (`0x6046f0` at `0x6047a3`: the new health not above 0, the old above). In its order:
//!
//! - a player's own leg first (`0x5e99b0` at `0x60588d`): a groupmate's death line, here, and
//!   the active player's release latch and timer, [`crate::ui_session`]'s;
//! - the dying unit's attack words zeroed (`0x605896`-`0x6058a8`): its target ([`Engaged`],
//!   `+0xc48`) and its stop in flight (`+0xc54`);
//! - `PresentUnitDeath 0x625190` (`0x6058ae`), the death line, animation and sound, which the chat
//!   log, the animation driver and the creature sounds play off the same edge;
//! - with a spell in flight (`0x6e3d30`) whose record's target (`0xceac60`, read through
//!   `0x6e3d50`) is this unit, `CancelCast(1, 1, 0x65)` (`0x6058e0`);
//! - when the unit is the selection (`0xb4e2d8`), `SetSelection(0, 0)` (`0x605901`) and then
//!   `StopAutoRepeatSpell 0x6ea080` (`0x605906`);
//! - the tail from `0x60590b`: the quest marker, the missiles, the effects, a sound, the auto-move
//!   stop and `0x4931c0` on the unit's guid (`0x605950`).

use bevy::prelude::*;

use benilla_protocol::messages::ObjectType;

use super::Selection;
use crate::creature_anim::{AttackSeam, Engaged};
use crate::net::{FieldChanged, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_action::{FillArg, UiError, UiErrorKeys};

/// What a groupmate's death shows: message `0x4c` (`0x5e9a0e`).
const PLAYER_DIED: &str = "ERR_PLAYER_DIED_S";

/// Each unit's death, in the order the edges came.
#[allow(clippy::too_many_arguments)] // the death's legs, each its own owner's state
pub(super) fn on_unit_died(
    mut edges: MessageReader<FieldChanged>,
    mut selection: ResMut<Selection>,
    mut seam: AttackSeam,
    mut commands: Commands,
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
    group: Res<crate::ui_party::GroupState>,
    names: Res<crate::names::NameCache>,
    stores: Query<&ObjectStore>,
    errors: Res<UiErrorKeys>,
) {
    // Our attack target as this pass leaves it: our own death zeroes it before a deselect reads it.
    let mut me_engaged = !engaged.is_empty();
    for e in edges.read().filter(|e| e.unit_died()) {
        let me = seam.me.single().ok();
        // `CGPlayer_C::OnDied 0x5e99b0`, for a player (`0x605882`): another player than the
        // active one (`0x5e99d8`-`0x5e99e4`) on one of our four party slots (`IsPartyMember
        // 0x4918d0` → `0x4e7f70` at `0x5e99f7`) shows the line under `GetUnitName`'s name, the
        // cache's with a callback or `UNKNOWNOBJECT` (`0x609210` at `0x5e9a08`, `0x496720` at
        // `0x5e9a10`).
        if e.kind == ObjectType::Player
            && me != Some(e.entity)
            && group.party_slots().any(|m| m.guid == e.guid)
        {
            let store = stores.get(e.entity).ok();
            errors.push(
                match names.resolve_unit(e.guid, store, Ask::Event(&seam.net)) {
                    Some(name) => UiError::s(PLAYER_DIED, name),
                    None => UiError::args(PLAYER_DIED, vec![FillArg::UnknownObject]),
                },
            );
        }
        if let Ok(mut unit) = commands.get_entity(e.entity) {
            unit.try_remove::<Engaged>();
        }
        seam.clear_stop_word(e.entity);
        if me == Some(e.entity) {
            me_engaged = false;
        }
        let spells = seam.spells.as_deref();
        let mut core = seam.slots.core(&seam.net);
        if core.slot().is_some() && core.pending.record_target() == Some(e.guid) {
            core.cancel_reaping(
                true,
                true,
                crate::spell::REASON_TARGETS_DEAD,
                spells,
                me,
                &seam.edits,
            );
        }
        // The dead target becomes the last one; a corpse selected afterwards stays selected, gray.
        if selection.guid == Some(e.guid) {
            super::click::deselect(&mut selection, &mut seam, me_engaged);
            crate::creature_anim::cancel_auto_repeat_local(
                me,
                &mut seam.auto_repeat,
                &seam.edits,
                &mut seam.slots.core(&seam.net),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use benilla_protocol::field::{FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
    use benilla_protocol::messages::ObjectType;
    use benilla_protocol::ObjectFields;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;
    use crossbeam_channel::Receiver;

    use crate::creature_anim::{AttackSeam, AttackStopInFlight, Engaged};
    use crate::net::{
        ClientCommand, FieldChanged, Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid,
        SelfPlayer,
    };
    use crate::spell::AutoRepeatActive;
    use crate::target::Selection;
    use crate::ui_cast::{CastBarEdge, CastReport, CastReports};

    const ME: u64 = 0x10;
    const MOB: u64 = 0xF130_0000_0000_0042;
    const OTHER: u64 = 0xF130_0000_0000_0043;
    /// A spell that binds any unit (`Targets 0x2`).
    const FIREBALL: u32 = 133;
    /// Wand Shoot's shape: an auto-repeat spell (`AttributesEx2 0x20`).
    const SHOOT: u32 = 5019;
    /// Mind Vision's second rank: a unit word, 50000 yards.
    const MIND_VISION: u32 = 10909;
    /// A groupmate the client does not hold.
    const UNHELD: u64 = 0x99;

    struct Client {
        app: App,
        me: Entity,
        mob: Entity,
        other: Entity,
        sent: Receiver<ClientCommand>,
    }

    /// The whole client headless, our player and two live mobs streamed, the write channel
    /// captured.
    fn client() -> Client {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, sent) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        let unit = |kind, extra: &[(u16, u32)]| {
            let mut pairs = vec![(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)];
            pairs.extend_from_slice(extra);
            ObjectStore(ObjectFields::from_pairs(&pairs).into_created(kind))
        };
        let me = world
            .spawn((
                SelfPlayer,
                Guid(ME),
                Transform::default(),
                unit(ObjectType::Player, &[(2, 0x19), (FIELD_UNIT_FLAGS, 0x8)]),
            ))
            .id();
        let mut mob = |guid| {
            world
                .spawn((
                    Guid(guid),
                    Transform::from_xyz(3.0, 0.0, 0.0),
                    unit(ObjectType::Unit, &[(2, 0x9)]),
                ))
                .id()
        };
        let (mob, other) = (mob(MOB), mob(OTHER));
        let mut index = world.resource_mut::<GuidIndex>();
        index.0.insert(ME, me);
        index.0.insert(MOB, mob);
        index.0.insert(OTHER, other);
        world.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    (
                        FIREBALL,
                        benilla_formats::SpellDisplay {
                            targets: 0x2,
                            ..Default::default()
                        },
                    ),
                    (
                        SHOOT,
                        benilla_formats::SpellDisplay {
                            targets: 0x2,
                            attributes_ex2: 0x20,
                            ..Default::default()
                        },
                    ),
                    (
                        MIND_VISION,
                        benilla_formats::SpellDisplay {
                            implicit_target_a1: 25,
                            range_index: 13,
                            ..Default::default()
                        },
                    ),
                ]
                .into(),
            ),
            ranges: benilla_formats::SpellRangeCatalog::from_rows(
                [(
                    13,
                    benilla_formats::SpellRange {
                        min: 0.0,
                        max: 50_000.0,
                        flags: 0,
                    },
                )]
                .into(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        Client {
            app,
            me,
            mob,
            other,
            sent,
        }
    }

    impl Client {
        fn world(&mut self) -> &mut World {
            self.app.world_mut()
        }

        fn select(&mut self, guid: u64, entity: Entity) {
            *self.world().resource_mut::<Selection>() = Selection {
                target: Some(entity),
                guid: Some(guid),
                lit: Some(entity),
                ..Default::default()
            };
        }

        /// A press of `spell_id` at the selection, the ladder's own bind.
        fn cast(&mut self, spell_id: u32) {
            self.world()
                .run_system_once(
                    move |mut ladder: crate::spell::CastLadder,
                          targeting: crate::spell::cast_target::CastTargeting| {
                        let ctx = targeting.context();
                        ladder.send(spell_id, &ctx, crate::spell::CastCommit::Spell);
                    },
                )
                .expect("the ladder runs on the built client");
        }

        /// `unit`'s health falls from 100 to 0, and the handler runs.
        fn dies(&mut self, entity: Entity, guid: u64, kind: ObjectType) {
            let world = self.world();
            world.write_message(FieldChanged {
                entity,
                guid,
                kind,
                index: FIELD_UNIT_HEALTH,
                old: 100,
                new: 0,
            });
            world
                .run_system_once(super::on_unit_died)
                .expect("the death handler runs on the built client");
        }

        fn sent(&self) -> Vec<ClientCommand> {
            self.sent.try_iter().collect()
        }

        fn reports(&mut self) -> Vec<CastReport> {
            std::mem::take(&mut self.world().resource_mut::<CastReports>().0)
        }

        fn in_flight(&mut self) -> Option<u32> {
            self.world()
                .run_system_once(
                    |mut slots: crate::spell::CastSlots, net: Res<NetCommands>| {
                        slots.core(&net).slot().map(|s| s.spell_id())
                    },
                )
                .expect("the slot reads")
        }
    }

    /// `0x6058b3`-`0x6058e0`: a spell in flight (`0x6e3d30`) whose record target (`0x6e3d50`,
    /// `0xceac60`) is the dying unit is cancelled with `0x6e4940(1, 1, 0x65)`: `CMSG_CANCEL_CAST`,
    /// the pop, and `SPELLCAST_FAILED` for the player's record. Another unit's death leaves it,
    /// and the record target is the bound unit, not the selection.
    #[test]
    fn a_unit_dying_cancels_the_cast_whose_record_names_it() {
        let mut c = client();
        let (mob, other) = (c.mob, c.other);
        c.select(MOB, mob);
        c.cast(FIREBALL);
        assert!(
            c.sent().iter().any(|s| matches!(
                s,
                ClientCommand::CastSpell {
                    spell_id: FIREBALL,
                    target: Some(MOB)
                }
            )),
            "the cast goes out at the mob"
        );
        assert_eq!(c.in_flight(), Some(FIREBALL));
        // The selection moves on; the record keeps the unit the cast bound.
        c.select(OTHER, other);
        c.reports();

        c.dies(other, OTHER, ObjectType::Unit);
        assert!(
            !c.sent()
                .iter()
                .any(|s| matches!(s, ClientCommand::CancelCast { .. })),
            "another unit's death"
        );
        assert_eq!(c.in_flight(), Some(FIREBALL));
        c.reports();

        c.dies(mob, MOB, ObjectType::Unit);
        assert!(
            matches!(
                c.sent()[..],
                [ClientCommand::CancelCast { spell_id: FIREBALL }]
            ),
            "the record's unit died"
        );
        assert_eq!(c.in_flight(), None, "popped");
        assert!(
            c.reports()
                .iter()
                .any(|r| matches!(r, CastReport::Bar(CastBarEdge::Failed))),
            "SPELLCAST_FAILED for SPELL_FAILED_TARGETS_DEAD"
        );
    }

    /// The far cast at a unit the client does not hold parks its guid in the record's target
    /// (`0x6e5454`), which the unit's death reads once it is held (`0x6058c7`).
    #[test]
    fn a_far_casts_parked_target_dying_cancels_it() {
        let mut c = client();
        *c.world().resource_mut::<Selection>() = Selection {
            guid: Some(UNHELD),
            ..Default::default()
        };
        c.cast(MIND_VISION);
        assert!(
            c.sent().iter().any(|s| matches!(
                s,
                ClientCommand::CastSpell {
                    spell_id: MIND_VISION,
                    target: None
                }
            )),
            "the far cast goes out with no target"
        );
        let unit = c.world().spawn(Guid(UNHELD)).id();
        c.dies(unit, UNHELD, ObjectType::Player);
        assert!(c.sent().iter().any(|s| matches!(
            s,
            ClientCommand::CancelCast {
                spell_id: MIND_VISION
            }
        )));
        assert_eq!(c.in_flight(), None);
    }

    /// The cursor's click binds the clicked unit into the record's target (`BindTarget`,
    /// `0x6e5cdc`), and that unit's death cancels the cast.
    #[test]
    fn a_clicked_targets_death_cancels_the_cast_it_bound() {
        let mut c = client();
        let mob = c.mob;
        c.cast(FIREBALL);
        assert_eq!(c.in_flight(), Some(FIREBALL), "the cursor holds the slot");
        c.world()
            .run_system_once(|mut ladder: crate::spell::CastLadder| {
                ladder.commit_targeted(
                    FIREBALL,
                    crate::spell::CastCommit::Spell,
                    crate::spell::TargetedBind::Unit(MOB),
                );
            })
            .expect("the commit runs on the built client");
        assert!(c.sent().iter().any(|s| matches!(
            s,
            ClientCommand::CastSpell {
                spell_id: FIREBALL,
                target: Some(MOB)
            }
        )));
        c.dies(mob, MOB, ObjectType::Unit);
        assert!(c
            .sent()
            .iter()
            .any(|s| matches!(s, ClientCommand::CancelCast { spell_id: FIREBALL })));
    }

    /// `0x6058e5`-`0x605906`: the selection's death deselects (`0x493540(0, 0)` at `0x605901`)
    /// and then stops auto-repeat (`0x6ea080` at `0x605906`): `CMSG_SET_SELECTION` 0, then
    /// `CMSG_CANCEL_AUTO_REPEAT_SPELL`, the key cleared and `STOP_AUTOREPEAT_SPELL` queued.
    #[test]
    fn the_selections_death_deselects_and_then_stops_auto_repeat() {
        let mut c = client();
        let mob = c.mob;
        c.select(MOB, mob);
        c.world().resource_mut::<AutoRepeatActive>().0 = Some(SHOOT);
        c.dies(mob, MOB, ObjectType::Unit);
        let sent = c.sent();
        assert!(
            matches!(
                sent[..],
                [
                    ClientCommand::SetSelection { guid: 0 },
                    ClientCommand::CancelAutoRepeat
                ]
            ),
            "{sent:?}"
        );
        assert_eq!(c.world().resource::<Selection>().guid, None);
        assert_eq!(c.world().resource::<Selection>().last, Some(MOB));
        assert_eq!(c.world().resource::<AutoRepeatActive>().0, None);
        assert!(c
            .reports()
            .iter()
            .any(|r| matches!(r, CastReport::AutoRepeat(false))));
    }

    /// `ReleaseTarget 0x493910` stops our attack (`0x493a08`) before it sends
    /// `CMSG_SET_SELECTION` 0 (`0x493a2a`): the selection's death while we swing at it sends the
    /// stop, the deselect and then the repeat's cancel (`0x605906`).
    #[test]
    fn the_selections_death_mid_swing_stops_before_it_deselects() {
        let mut c = client();
        let (me, mob) = (c.me, c.mob);
        c.select(MOB, mob);
        c.world().entity_mut(me).insert(Engaged(MOB));
        c.world().resource_mut::<AutoRepeatActive>().0 = Some(SHOOT);
        c.dies(mob, MOB, ObjectType::Unit);
        let sent = c.sent();
        assert!(
            matches!(
                sent[..],
                [
                    ClientCommand::AttackStop,
                    ClientCommand::SetSelection { guid: 0 },
                    ClientCommand::CancelAutoRepeat
                ]
            ),
            "{sent:?}"
        );
    }

    /// A unit that is not the selection and names no record dies with no deselect and no stop.
    #[test]
    fn another_units_death_touches_neither_the_selection_nor_the_repeat() {
        let mut c = client();
        let (mob, other) = (c.mob, c.other);
        c.select(MOB, mob);
        c.world().resource_mut::<AutoRepeatActive>().0 = Some(SHOOT);
        c.dies(other, OTHER, ObjectType::Unit);
        assert!(c.sent().is_empty());
        assert_eq!(c.world().resource::<Selection>().guid, Some(MOB));
        assert_eq!(c.world().resource::<AutoRepeatActive>().0, Some(SHOOT));
    }

    /// `0x605896`-`0x6058a8`: the dying unit's `+0xc48` to `+0xc54` zeroed, its attack target
    /// ([`Engaged`]) and, for the active player, the stop in flight (`[+0xc54]`), which the
    /// attack stop's echo clears too (`0x624e40` at `0x624e6b`).
    #[test]
    fn a_death_and_the_attack_stop_echo_clear_the_attack_words() {
        let mut c = client();
        let (me, mob) = (c.me, c.mob);
        c.world().entity_mut(mob).insert(Engaged(ME));
        c.dies(mob, MOB, ObjectType::Unit);
        assert!(c.world().get::<Engaged>(mob).is_none(), "the mob's attack");

        let stop = |c: &mut Client| {
            c.world()
                .run_system_once(|mut seam: AttackSeam| seam.stop(true))
                .expect("the stop runs on the built client");
            assert!(
                c.world().resource::<AttackStopInFlight>().of(me),
                "the stop's send sets it (`0x6243b2`)"
            );
        };
        c.world().entity_mut(me).insert(Engaged(MOB));
        stop(&mut c);
        c.dies(me, ME, ObjectType::Player);
        assert!(!c.world().resource::<AttackStopInFlight>().of(me), "death");
        assert!(c.world().get::<Engaged>(me).is_none());

        c.world().entity_mut(me).insert(Engaged(MOB));
        stop(&mut c);
        crate::net::handlers::dispatch(
            c.world(),
            crate::net::wire(vec![benilla_protocol::SessionEvent::AttackStop {
                attacker: ME,
                victim: MOB,
            }]),
        );
        assert!(
            !c.world().resource::<AttackStopInFlight>().of(me),
            "the echo"
        );
    }

    /// `CGPlayer_C::OnDied 0x5e99b0`, run for a player (`0x605882` → `0x60588d`): another player
    /// than the active one (`0x5e99d8`-`0x5e99e4`) who holds one of our four party slots
    /// (`IsPartyMember 0x4918d0` → `0x4e7f70`, the slots at `0xbc6f48`) shows `ERR_PLAYER_DIED_S`,
    /// message `0x4c`, with `GetUnitName`'s name (`0x609210` at `0x5e9a08`, `0x496720` at
    /// `0x5e9a10`): the name cache's, `UNKNOWNOBJECT` on a miss. A raid member in another
    /// subgroup holds no slot; a stranger, a creature and our own death show nothing.
    #[test]
    fn a_party_members_death_names_them_and_no_one_elses_does() {
        use crate::ui_action::{FillArg, UiError, UiErrorKeys};
        use benilla_protocol::messages::GroupMemberEntry;
        const MATE: u64 = 0x21;
        const NAMELESS: u64 = 0x22;
        const FAR: u64 = 0x23;
        const STRANGER: u64 = 0x24;
        let mut c = client();
        let mut players = Vec::new();
        for guid in [MATE, NAMELESS, FAR, STRANGER] {
            let store = ObjectStore(
                ObjectFields::from_pairs(&[
                    (2, 0x19),
                    (FIELD_UNIT_HEALTH, 100),
                    (FIELD_UNIT_MAXHEALTH, 100),
                ])
                .into_created(ObjectType::Player),
            );
            let e = c.world().spawn((Guid(guid), store)).id();
            c.world().resource_mut::<GuidIndex>().0.insert(guid, e);
            players.push(e);
        }
        let entry = |name: &str, guid: u64, subgroup: u8| GroupMemberEntry {
            name: name.into(),
            guid,
            status: 1,
            flags: subgroup,
        };
        c.world()
            .resource_mut::<crate::ui_party::GroupState>()
            .apply_list(
                crate::ui_party::GROUPTYPE_RAID,
                0,
                vec![
                    entry("Mate", MATE, 0),
                    entry("Nameless", NAMELESS, 0),
                    entry("Far", FAR, 1),
                ],
                ME,
                None,
                Some(ME),
            );
        let mut names = c.world().resource_mut::<crate::names::NameCache>();
        for (guid, name) in [(MATE, "Mate"), (FAR, "Far"), (STRANGER, "Stranger")] {
            names.insert_player(guid, name.into(), None);
        }
        let _ = c.world().resource::<UiErrorKeys>().take();
        let lines = |c: &mut Client, entity, guid, kind| {
            // Each one-shot reads every edge still buffered: only this death's.
            c.world().resource_mut::<Messages<FieldChanged>>().clear();
            c.dies(entity, guid, kind);
            c.world().resource::<UiErrorKeys>().take()
        };
        assert_eq!(
            lines(&mut c, players[0], MATE, ObjectType::Player),
            [UiError::s("ERR_PLAYER_DIED_S", "Mate")]
        );
        assert_eq!(
            lines(&mut c, players[1], NAMELESS, ObjectType::Player),
            [UiError::args(
                "ERR_PLAYER_DIED_S",
                vec![FillArg::UnknownObject]
            )],
            "a name the cache misses"
        );
        assert_eq!(
            lines(&mut c, players[2], FAR, ObjectType::Player),
            [],
            "another subgroup"
        );
        assert_eq!(
            lines(&mut c, players[3], STRANGER, ObjectType::Player),
            [],
            "no groupmate"
        );
        let (mob, me) = (c.mob, c.me);
        assert_eq!(lines(&mut c, mob, MOB, ObjectType::Unit), [], "a creature");
        assert_eq!(lines(&mut c, me, ME, ObjectType::Player), [], "our own");
    }
}
