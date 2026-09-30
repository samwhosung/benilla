//! The units `GetMinMaxRange` (`0x6e3480`) reads, built from the world: the caster, a target unit,
//! and the caster's auto-attack target, each as its combat reach, player bit and motion
//! ([`benilla_formats::RangeUnit`]).

use benilla_formats::{RangeUnit, UnitMotion};
use benilla_protocol::messages::ObjectType;
use benilla_protocol::MoveSpeeds;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::creature_anim::select::unify;
use crate::creature_anim::{move_flags, Engaged};
use crate::net::{
    current_speed, CreatureSwimming, Embodied, GuidIndex, ObjectStore, RemoteMotion, SelfPlayer,
    Spline, UnitMoveModes, UnitSpeeds,
};
use crate::player::Player;

/// The descriptor's default combat reach, before a unit's store streams.
pub(crate) const DEFAULT_REACH: f32 = 1.5;

/// One unit's motion as `0x7c4c90` and the moving test read it (`0x5fc350`, `0x6e364c`): `flags`
/// is the movement word (`[[unit+0x118]+0x40]`), and the current speed is 0 with no direction bit
/// (`0x7c4c99`), then the live server spline's speed, its length over its duration (`0x7c4caa`),
/// then the swim, walk, run or run-back speed the flags pick ([`current_speed`]). A unit with no
/// speeds streamed has none of the three: the reference's are zero until the movement block
/// applies, and it is never running.
pub(crate) fn unit_motion(
    flags: u32,
    speeds: Option<&MoveSpeeds>,
    spline: Option<&Spline>,
) -> UnitMotion {
    let Some(speeds) = speeds else {
        return UnitMotion {
            flags,
            ..UnitMotion::default()
        };
    };
    let speed = match spline {
        Some(spline) if flags & move_flags::ANY_MOVE != 0 => spline.speed(),
        _ => current_speed(speeds, flags),
    };
    UnitMotion {
        flags,
        speed,
        walk_speed: speeds.walk,
    }
}

/// What a unit's range inputs are read from: its descriptor, and the state its movement flags and
/// speeds come from ([`unify`], [`UnitSpeeds`]).
type UnitParts = (
    &'static ObjectStore,
    Option<&'static RemoteMotion>,
    Option<&'static Spline>,
    Option<&'static UnitMoveModes>,
    Option<&'static UnitSpeeds>,
    Has<CreatureSwimming>,
    Has<Embodied>,
);

/// The reader of the range units: any streamed unit, our own caster, and a unit's auto-attack
/// target.
#[derive(SystemParam)]
pub(crate) struct RangeUnits<'w, 's> {
    units: Query<'w, 's, UnitParts>,
    caster: Query<'w, 's, Entity, With<SelfPlayer>>,
    /// The auto-attack target guid, `[unit+0xc48]`.
    engaged: Query<'w, 's, &'static Engaged>,
    index: Option<Res<'w, GuidIndex>>,
    /// The flags word of the body we drive, [`Player::move_flags`].
    player: Option<Res<'w, Player>>,
}

impl RangeUnits<'_, '_> {
    /// The unit `entity` is, `None` before its descriptor streams. The body we drive reads the
    /// word our controller builds, one answer for the wire and the local gates; any other unit's
    /// is its relayed or spline-implied flags with the modes the server granted.
    pub(crate) fn unit(&self, entity: Entity) -> Option<RangeUnit> {
        let (store, remote, spline, modes, speeds, swimming, driven) =
            self.units.get(entity).ok()?;
        let flags = if driven {
            self.player.as_deref().map_or(0, Player::move_flags)
        } else {
            unify(None, remote, spline, swimming, modes).flags
        };
        Some(RangeUnit {
            reach: store.0.unit_combat_reach(),
            player: store.0.object_type() == Some(ObjectType::Player),
            motion: unit_motion(flags, speeds.map(|s| &s.0), spline),
        })
    }

    /// The caster: our own player, at the default reach and standing until its descriptor streams.
    pub(crate) fn caster(&self) -> RangeUnit {
        self.caster
            .iter()
            .next()
            .and_then(|e| self.unit(e))
            .unwrap_or(RangeUnit::still(DEFAULT_REACH))
    }

    /// The auto-attack target of `entity`, the unit `GetMinMaxRange` looks up itself for its melee
    /// arm (`0x6e3552`-`0x6e3584`). `None` with no target engaged or before it streams, where the
    /// arm reads the caster's own reach (`0x6e3594`).
    pub(crate) fn attack_target(&self, entity: Entity) -> Option<RangeUnit> {
        let guid = self.engaged.get(entity).ok()?.0;
        let target = *self.index.as_ref()?.0.get(&guid)?;
        self.unit(target)
    }

    /// The auto-attack target of our own player.
    pub(crate) fn caster_attack_target(&self) -> Option<RangeUnit> {
        self.attack_target(self.caster.iter().next()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creature_anim::move_flags as f;
    use crate::net::UnitSpeeds;

    fn speeds() -> MoveSpeeds {
        MoveSpeeds {
            walk: 2.5,
            run: 7.0,
            run_back: 4.5,
            swim: 4.722_222,
            swim_back: 2.5,
            turn_rate: std::f32::consts::PI,
        }
    }

    fn spline(length: f32, secs: u64) -> Spline {
        Spline {
            points: vec![[0.0; 3], [length, 0.0, 0.0]],
            start: std::time::Instant::now(),
            duration: std::time::Duration::from_secs(secs),
            id: 1,
            grounded: true,
            run_mode: true,
            deck: None,
        }
    }

    /// `0x7c4c90` for a forward runner answers the run speed, and `0x5fc350` finds it above twice
    /// the walk speed.
    #[test]
    fn a_forward_runner_is_running() {
        let m = unit_motion(f::FORWARD, Some(&speeds()), None);
        assert_eq!((m.flags, m.speed, m.walk_speed), (f::FORWARD, 7.0, 2.5));
        assert!(m.moves_at_run_speed());
    }

    /// Walk mode picks `min(walk, run)`, no direction bit is speed 0, and swimming is the swim
    /// speed, which is below twice the walk speed.
    #[test]
    fn a_walker_a_stander_and_a_swimmer_are_not_running() {
        let s = speeds();
        for (name, flags) in [
            ("walking", f::FORWARD | f::WALK_MODE),
            ("standing", 0),
            ("jumping in place", f::FALLING),
            ("swimming", f::FORWARD | f::SWIMMING),
            ("backward", f::BACKWARD),
        ] {
            let m = unit_motion(flags, Some(&s), None);
            assert!(!m.moves_at_run_speed(), "{name}: {m:?}");
        }
        // A swim speed past twice the walk speed is running, as the compare has it.
        let fast = MoveSpeeds { swim: 6.0, ..s };
        assert!(unit_motion(f::FORWARD | f::SWIMMING, Some(&fast), None).moves_at_run_speed());
    }

    /// A live spline is the current speed (`0x7c4caa`), whatever the flags pick: a mob fleeing at
    /// 8 yd/s runs, and one wandering at 2.5 does not though its flags are a runner's (a creature's
    /// spline sets forward and no walk mode, [`unify`]); with no direction bit it is 0.
    #[test]
    fn a_spline_is_its_own_speed() {
        let s = speeds();
        let fleeing = unit_motion(f::FORWARD, Some(&s), Some(&spline(16.0, 2)));
        assert_eq!(fleeing.speed, 8.0);
        assert!(fleeing.moves_at_run_speed());
        let wandering = unit_motion(f::FORWARD, Some(&s), Some(&spline(5.0, 2)));
        assert_eq!(wandering.speed, 2.5);
        assert!(!wandering.moves_at_run_speed());
        assert_eq!(unit_motion(0, Some(&s), Some(&spline(16.0, 2))).speed, 0.0);
    }

    /// No speeds streamed: never running.
    #[test]
    fn a_unit_without_speeds_is_not_running() {
        assert!(!unit_motion(f::FORWARD, None, Some(&spline(16.0, 2))).moves_at_run_speed());
    }

    fn store(typemask: u32, reach: f32) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (2, typemask),
            (22, 100),
            (130, reach.to_bits()),
        ]))
    }

    fn remote(flags: u32) -> RemoteMotion {
        RemoteMotion {
            wow_pos: [0.0; 3],
            orientation: 0.0,
            flags,
            pitch: 0.0,
            speed: 0.0,
            vertical_velocity: 0.0,
            jump_xy_vel: [0.0; 2],
            fall_start_z: None,
            pending: Default::default(),
            relay: Default::default(),
            last_apply_ms: 0.0,
            last_apply_pos: [0.0; 3],
        }
    }

    const MOB: u64 = 0xF130_0000_0000_0009;

    /// The body we drive reads the word our controller built, a remote player its relayed flags,
    /// a creature its spline; the player bit is `OBJECT_FIELD_TYPE` bit 4, and the caster's
    /// auto-attack target is looked up through its `Engaged` guid.
    #[test]
    fn a_unit_is_read_off_its_own_state() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(Player::with_move_flags(f::FORWARD));
        let run = UnitSpeeds(speeds());
        let me = world
            .spawn((SelfPlayer, Embodied, run, store(0x19, 2.0)))
            .id();
        let friend = world
            .spawn((remote(f::STRAFE_LEFT), run, store(0x19, 1.5)))
            .id();
        let mob = world.spawn((spline(16.0, 2), run, store(0x09, 1.25))).id();
        let wanderer = world.spawn((spline(5.0, 2), run, store(0x09, 1.5))).id();
        let idle = world.spawn((run, store(0x09, 1.5))).id();
        let bare = world.spawn(run).id();
        world.entity_mut(me).insert(Engaged(MOB));
        world.insert_resource(GuidIndex([(MOB, mob)].into_iter().collect()));
        world
            .run_system_once(move |units: RangeUnits| {
                let me = units.caster();
                assert_eq!(me.reach, 2.0);
                assert!(me.player && me.motion.moves_at_run_speed(), "{me:?}");
                let friend = units.unit(friend).unwrap();
                assert!(
                    friend.player && friend.motion.moves_at_run_speed(),
                    "{friend:?}"
                );
                let mob_unit = units.unit(mob).unwrap();
                assert!(!mob_unit.player, "a creature");
                assert_eq!(mob_unit.reach, 1.25);
                assert_eq!(mob_unit.motion.speed, 8.0, "its spline's speed");
                assert!(mob_unit.motion.moves_at_run_speed());
                let wanderer = units.unit(wanderer).unwrap();
                assert!(!wanderer.motion.moves_at_run_speed(), "{wanderer:?}");
                let idle = units.unit(idle).unwrap();
                assert!(!idle.motion.moves_at_run_speed(), "{idle:?}");
                assert!(units.unit(bare).is_none(), "no descriptor streamed");
                assert_eq!(units.caster_attack_target(), Some(mob_unit));
                assert_eq!(units.attack_target(mob), None, "the mob engages nothing");
            })
            .expect("the units read");
    }

    /// Our own body is the word `Player` holds, so the same body standing still, walking or with
    /// no controller in the world is not running.
    #[test]
    fn the_driven_body_follows_the_controllers_word() {
        use bevy::ecs::system::RunSystemOnce;

        let caster_at = |player: Option<Player>| {
            let mut world = World::new();
            if let Some(player) = player {
                world.insert_resource(player);
            }
            world.spawn((SelfPlayer, Embodied, UnitSpeeds(speeds()), store(0x19, 1.5)));
            world
                .run_system_once(|units: RangeUnits| units.caster().motion.moves_at_run_speed())
                .expect("the caster reads")
        };
        assert!(caster_at(Some(Player::with_move_flags(f::FORWARD))));
        assert!(caster_at(Some(Player::with_move_flags(
            f::STRAFE_RIGHT | f::FALLING
        ))));
        assert!(!caster_at(Some(Player::with_move_flags(0))));
        assert!(!caster_at(Some(Player::with_move_flags(
            f::FORWARD | f::WALK_MODE
        ))));
        assert!(!caster_at(Some(Player::with_move_flags(f::BACKWARD))));
        assert!(!caster_at(None));
    }

    /// The send path's range gate ([`super::super::cast_target::CastTargeting`]): a warrior running
    /// after a running mob presses Hamstring (the melee row) at 7 yards and is not refused, and
    /// the same press with either one standing is "Out of range."
    #[test]
    fn the_press_reaches_the_moving_bonus_with_both_units_running() {
        use super::super::cast_target::CastTargeting;
        use benilla_formats::{SpellDisplay, SpellRange};
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 0xF130_0000_0000_0001;
        let refusal = |caster_runs: bool, mob_runs: bool, yards: f32| {
            let mut world = World::new();
            world.insert_resource(Player::with_move_flags(if caster_runs {
                f::FORWARD
            } else {
                0
            }));
            world.init_resource::<crate::spell::AutoSelfCast>();
            world.insert_resource(crate::net::Reputations(Vec::new()));
            world.insert_resource(crate::net::SelfGuid(Some(ME)));
            let run = UnitSpeeds(speeds());
            world.spawn((
                SelfPlayer,
                Embodied,
                run,
                store(0x19, 1.5),
                Transform::default(),
            ));
            let mob = world
                .spawn((run, store(0x09, 1.5), Transform::from_xyz(yards, 0.0, 0.0)))
                .id();
            if mob_runs {
                world.entity_mut(mob).insert(spline(16.0, 2));
            }
            world.insert_resource(crate::target::Selection {
                target: Some(mob),
                guid: Some(MOB),
                ..Default::default()
            });
            let hamstring = SpellDisplay {
                range_index: 2,
                ..Default::default()
            };
            let melee = SpellRange {
                min: 0.0,
                max: 5.0,
                flags: 1,
            };
            world
                .run_system_once(move |targeting: CastTargeting| {
                    targeting.context().range.refusal(
                        &hamstring,
                        Some(&melee),
                        &crate::spell::SpellModifiers::default(),
                    )
                })
                .expect("the context builds")
        };
        assert_eq!(refusal(true, true, 7.0), None, "both running");
        assert_eq!(refusal(true, true, 8.0), Some(0x59), "past 7.667");
        assert_eq!(refusal(true, false, 7.0), Some(0x59), "the mob stands");
        assert_eq!(refusal(false, true, 7.0), Some(0x59), "the caster stands");
        assert_eq!(refusal(false, false, 4.9), None, "the floor stands");
    }
}
