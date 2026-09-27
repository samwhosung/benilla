//! The world click's two commits, the legs of the click dispatcher `0x492ce0`. While targeting,
//! the pick flags (`0x481050`) come only from the word `0xcecac0`, so the word picks the leg:
//!
//! - terrain (`0x492c90` → `0x492580` → `BindLocation 0x6e60f0`): [`commit_ground_cast_on_click`]
//! - object (`0x4925d0` → `SetSelection 0x493540` → `BindTarget 0x6e5b40`), whose GameObject,
//!   unit or corpse arm the picked object's type chooses: [`commit_object_cast_on_click`]
//!
//! A location-only word yields pick flags 3, whose `& 0x7c == 0` disables the object pick, so an
//! AoE reticle clicks through a chest. The terrain and GameObject arms gate on nothing, not range,
//! validity or the lock: the server judges and its `SMSG_CAST_RESULT` is the refusal. The unit and
//! corpse arms run `BindTarget`'s relation and range checks ([`super::bind_target_unit`],
//! [`super::corpse::bind_target_corpse`]).

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;

use crate::spell::cast_send::TargetedBind;
use crate::target::go_is_nearest;
#[cfg(test)]
use crate::target::{Hovered, HoveredObject};
use benilla_world::interact::WorldClick;

use super::TargetingWants;

/// The terrain leg (`0x492580`): bind the press's ground point and send, with no range check and
/// no error path, then arm the pending cast and GCD and end the mode. No ground hit (sky) commits
/// nothing and keeps the mode. [`crate::target::click::select_on_click`] holds off while targeting,
/// so the click neither selects nor deselects.
///
/// Runs after `select_on_click`, which reads the mode this clears.
pub(crate) fn commit_ground_cast_on_click(
    mut clicks: MessageReader<WorldClick>,
    // The point the press ray hit (the reference's `+0x360`), read unchanged at the release.
    press: Res<crate::target::PressPick>,
    mut ladder: crate::spell::CastLadder,
) {
    let occlusion = press.occlusion;
    if !ladder.ground.active() {
        // A click buffered while idle must not replay as a commit once the mode turns on.
        clicks.clear();
        return;
    }
    if clicks.read().last().is_none() {
        return;
    }
    // `TargetingWantsLocation 0x6e6320`: a word without it binds nothing and the mode stays.
    let Some((spell_id, commit)) = ladder.ground.pending_for(TargetingWants::Location) else {
        return;
    };
    let Some(point) = occlusion.point else {
        // Sky: the nothing leg, no commit, the mode stays.
        return;
    };
    let at = bevy_to_wow(point);
    // `BindLocation 0x6e60f0`: SOURCE (`0x20`) first, then DEST (`0x40`). `None` cannot happen
    // behind `pending_for(Location)`; the `let else` fails closed.
    let Some(bound) = ladder.ground.location_bind(at) else {
        return;
    };
    debug!(
        "ui_action: ground cast {spell_id} committed at wow ({:.2}, {:.2}, {:.2}) as {bound:?}",
        at[0], at[1], at[2]
    );
    // `SendCast 0x6e54f0`: a thrown grenade commits as `CMSG_USE_ITEM` with the location block.
    ladder.commit_targeted(spell_id, commit, bound);
}

/// The object leg. A left-click on an object goes `0x492ce0` → `0x4925d0` → `SetSelection
/// 0x493540`, whose first act while targeting is `BindTarget 0x6e5b40` and return, so the click
/// never changes the player's target and never reaches the GameObject's unselectable check.
///
/// `BindTarget` picks its arm by the clicked object's typemask. The GameObject arm, when a word in
/// `0x4800` puts GameObjects in the pick and one is the nearest hit ([`go_is_nearest`]), writes
/// wire bit `TARGET_FLAG_GAMEOBJECT` (`0x800`), clears those `0x4800` word bits, parks the guid
/// (`0xceac60`), and the zero word lets it call `SendCast 0x6e54f0`, with no gate before the send,
/// not range nor the lock: the refusal is the server's `SMSG_CAST_RESULT`. The right-click path
/// ([`crate::target::click`], `0x5f33e0`) does resolve the lock and can refuse locally. Otherwise
/// the picked unit goes to the unit arm ([`super::bind_target_unit`]) and the picked corpse to the
/// corpse arm ([`super::corpse::bind_target_corpse`]); a unit click under a lock word binds nothing
/// there, since every unit arm tests a bit the word lacks.
///
/// Runs after `select_on_click`, as the terrain commit does.
pub(crate) fn commit_object_cast_on_click(
    mut clicks: MessageReader<WorldClick>,
    // The press's pick: the object the gesture started on.
    press: Res<crate::target::PressPick>,
    checks: super::BindChecks,
    mut ladder: crate::spell::CastLadder,
) {
    let (hovered, hovered_object) = (press.hovered, press.object);
    if !ladder.ground.active() {
        // A click buffered while idle must not replay as a commit once the mode turns on.
        clicks.clear();
        return;
    }
    if clicks.read().last().is_none() {
        return;
    }
    // `TargetingWantsGameObject 0x6e62d0`: without it the pick holds no GameObject.
    if !(ladder.ground.wants(TargetingWants::GameObject)
        && go_is_nearest(&hovered, &hovered_object))
    {
        if let Some((entity, guid)) = hovered.target.zip(hovered.guid) {
            super::bind_target_unit(&mut ladder, &checks, entity, guid);
        } else if let Some((entity, guid)) = hovered.corpse.zip(hovered.corpse_guid) {
            super::corpse::bind_target_corpse(&mut ladder, &checks, entity, guid);
        }
        return;
    }
    let Some((spell_id, commit)) = ladder.ground.pending_for(TargetingWants::GameObject) else {
        return;
    };
    let Some(guid) = hovered_object.guid else {
        return;
    };
    debug!("ui_action: cast {spell_id} committed at gameobject {guid:#x}");
    // `CMSG_CAST_SPELL` mask `0x800` and the packed guid, or `CMSG_USE_ITEM` for a key.
    ladder.commit_targeted(spell_id, commit, TargetedBind::Object(guid));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{ClientCommand, NetCommands};
    use bevy::ecs::system::SystemId;
    use crossbeam_channel::Receiver;

    const OPENING: u32 = 3365;
    const CHEST: u64 = 0xF110_000C_1F00_A3B2;
    /// An opener's lock word: `Targets 0x4000` plus implicit arm 23's `0x800`.
    const LOCK_WORD: u16 = 0x4800;

    /// The ladder's resources, the press latch and the click message; the packet's shape is
    /// tested in `benilla-protocol`.
    fn fixture() -> (World, Receiver<ClientCommand>, SystemId) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<crate::items::Items>();
        world.init_resource::<crate::net::GuidIndex>();
        world.insert_resource(crate::net::Reputations(Vec::new()));
        world.init_resource::<crate::spell::PendingCast>();
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::Cooldowns>();
        world.init_resource::<crate::spell::SpellModifiers>();
        world.init_resource::<crate::ui_action::CastErrors>();
        world.init_resource::<crate::ui_action::UiErrorKeys>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<crate::ui_tradeskill::TradeSkillOpens>();
        world.init_resource::<super::super::SpellTargeting>();
        world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<Messages<WorldClick>>();
        // The commit legs read the press latch, not the live hover.
        world.init_resource::<crate::target::PressPick>();
        // Registered, not `run_system_once`: the reader drain is state across frames, and a fresh
        // system per call would start every read at cursor 0.
        let id = world.register_system(commit_object_cast_on_click);
        (world, rx, id)
    }

    /// Put a GameObject in the press latch at `distance`, nearer than any unit.
    fn hover_go(world: &mut World, distance: f32) {
        world.resource_mut::<crate::target::PressPick>().object = HoveredObject {
            target: Some(Entity::from_raw_u32(1).unwrap()),
            guid: Some(CHEST),
            distance,
        };
    }

    fn click(world: &mut World, id: SystemId) {
        world
            .resource_mut::<Messages<WorldClick>>()
            .write(WorldClick);
        world.run_system(id).expect("the object commit runs");
    }

    /// `BindLocation 0x6e60f0`: the standing word alone binds the point to SOURCE (`Targets 0x20`,
    /// spell 265) or DEST (`Targets 0x40`, Blizzard).
    #[test]
    fn the_terrain_click_binds_source_or_dest_by_the_standing_word() {
        const BLIZZARD: u32 = 10;
        const AREA_DEATH: u32 = 265;
        let commit = crate::spell::cast_send::CastCommit::Spell;

        let ground = |world: &mut World| {
            world.resource_mut::<crate::target::PressPick>().occlusion =
                crate::target::PickOcclusion {
                    distance: 5.0,
                    point: Some(Vec3::new(1.0, 2.0, 3.0)),
                };
        };
        let run = |world: &mut World, id: SystemId| {
            world
                .resource_mut::<Messages<WorldClick>>()
                .write(WorldClick);
            world.run_system(id).expect("the ground commit runs");
        };

        // DEST word: the dest opcode.
        let (mut world, rx, _) = fixture();
        let id = world.register_system(commit_ground_cast_on_click);
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(BLIZZARD, commit, 0x0040);
        ground(&mut world);
        run(&mut world, id);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpellAtDest {
                spell_id: BLIZZARD,
                ..
            })
        ));

        // SOURCE word: the source opcode, from the identical click.
        let (mut world, rx, _) = fixture();
        let id = world.register_system(commit_ground_cast_on_click);
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(AREA_DEATH, commit, 0x0020);
        ground(&mut world);
        run(&mut world, id);
        let sent = rx.try_recv().expect("a source word still commits");
        assert!(
            matches!(
                sent,
                ClientCommand::CastSpellAtSource {
                    spell_id: AREA_DEATH,
                    ..
                }
            ),
            "a 0x20 word binds the SOURCE slot, not the dest: {sent:?}"
        );
        assert!(
            !world.resource::<super::super::SpellTargeting>().active(),
            "and the commit clears the one word"
        );

        // Both bits: SOURCE wins. No 1.12 spell carries both; only the precedence is built.
        let (mut world, rx, _) = fixture();
        let id = world.register_system(commit_ground_cast_on_click);
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(AREA_DEATH, commit, 0x0060);
        ground(&mut world);
        run(&mut world, id);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpellAtSource { .. })
        ));
    }

    /// A lock word, a chest under the cursor, one click: the cast at the chest, the mode ended.
    #[test]
    fn a_click_on_a_hovered_gameobject_commits_the_lock_cast() {
        let (mut world, rx, id) = fixture();
        world.resource_mut::<super::super::SpellTargeting>().enter(
            OPENING,
            crate::spell::cast_send::CastCommit::Spell,
            LOCK_WORD,
        );
        hover_go(&mut world, 5.0);

        click(&mut world, id);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpellGameObject {
                spell_id: OPENING,
                go_guid: CHEST,
            })
        ));
        assert!(
            !world.resource::<super::super::SpellTargeting>().active(),
            "the commit clears the one word"
        );
    }

    /// A unit word waits for a valid unit click and sends that unit in the ordinary
    /// `TARGET_FLAG_UNIT` spell shape. An assist word does not bind a neutral unit.
    #[test]
    fn a_click_on_a_hovered_unit_commits_the_hand_cursor_cast() {
        const HEAL: u32 = 2050;
        const ALLY: u64 = 0xF130_0000_0000_0001;
        let (mut world, rx, id) = fixture();
        world.resource_mut::<super::super::SpellTargeting>().enter(
            HEAL,
            crate::spell::cast_send::CastCommit::Spell,
            0x0002,
        );
        world.resource_mut::<crate::target::PressPick>().hovered = Hovered {
            target: Some(Entity::from_raw_u32(1).unwrap()),
            guid: Some(ALLY),
            distance: 5.0,
            ..Hovered::default()
        };
        click(&mut world, id);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpell {
                spell_id: HEAL,
                target: Some(ALLY),
            })
        ));
        assert!(
            !world.resource::<super::super::SpellTargeting>().active(),
            "the unit bind clears the hand cursor"
        );

        // With no faction catalog the target's reaction is neutral (3): an ASSIST word must stay
        // armed and send nothing, exactly as when that unit was selected before the cast.
        let (mut world, rx, id) = fixture();
        world.resource_mut::<super::super::SpellTargeting>().enter(
            HEAL,
            crate::spell::cast_send::CastCommit::Spell,
            0x0100,
        );
        world.resource_mut::<crate::target::PressPick>().hovered = Hovered {
            target: Some(Entity::from_raw_u32(1).unwrap()),
            guid: Some(ALLY),
            distance: 5.0,
            ..Hovered::default()
        };
        click(&mut world, id);
        assert!(
            rx.try_recv().is_err(),
            "a neutral unit must not receive the heal"
        );
        assert!(
            world.resource::<super::super::SpellTargeting>().active(),
            "an invalid unit click leaves the targeting word standing"
        );

        // A unit word puts no GameObject in the pick, so a chest nearer than the unit is no
        // obstacle: the unit arm still binds.
        let (mut world, rx, id) = fixture();
        world.resource_mut::<super::super::SpellTargeting>().enter(
            HEAL,
            crate::spell::cast_send::CastCommit::Spell,
            0x0002,
        );
        hover_go(&mut world, 2.0);
        world.resource_mut::<crate::target::PressPick>().hovered = Hovered {
            target: Some(Entity::from_raw_u32(1).unwrap()),
            guid: Some(ALLY),
            distance: 5.0,
            ..Hovered::default()
        };
        click(&mut world, id);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpell {
                spell_id: HEAL,
                target: Some(ALLY),
            })
        ));
    }

    /// `BindTarget`'s range leg (`6e6063`): a unit the relation checks accept but out of the
    /// spell's range raises "Out of range." and the cursor stays armed; a click in range commits.
    /// Row 5 is 0 to 30 yd, padded by both 1.5 combat reaches to 33.
    #[test]
    fn an_out_of_range_unit_click_raises_and_keeps_the_cursor() {
        use std::collections::HashMap;
        const HEAL: u32 = 2050;
        const ALLY: u64 = 0xF130_0000_0000_0001;
        let (mut world, rx, id) = fixture();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(HashMap::from([(
            HEAL,
            benilla_formats::SpellDisplay {
                range_index: 5,
                ..Default::default()
            },
        )]));
        spells.ranges = benilla_formats::SpellRangeCatalog::from_rows(HashMap::from([(
            5,
            benilla_formats::SpellRange {
                min: 0.0,
                max: 30.0,
                flags: 0,
            },
        )]));
        world.insert_resource(spells);
        world.spawn((crate::net::SelfPlayer, GlobalTransform::default()));
        let ally = world
            .spawn(GlobalTransform::from_translation(Vec3::new(40.0, 0.0, 0.0)))
            .id();
        world.resource_mut::<super::super::SpellTargeting>().enter(
            HEAL,
            crate::spell::cast_send::CastCommit::Spell,
            0x0002,
        );
        world.resource_mut::<crate::target::PressPick>().hovered = Hovered {
            target: Some(ally),
            guid: Some(ALLY),
            distance: 5.0,
            ..Hovered::default()
        };

        click(&mut world, id);
        assert!(rx.try_recv().is_err(), "out of range sends nothing");
        assert_eq!(
            std::mem::take(&mut world.resource_mut::<crate::ui_action::CastErrors>().0),
            vec![crate::ui_action::CastFail::local(HEAL, 0x59)],
            "\"Out of range.\""
        );
        assert!(
            world.resource::<super::super::SpellTargeting>().active(),
            "the cursor stays armed"
        );

        world
            .entity_mut(ally)
            .insert(GlobalTransform::from_translation(Vec3::new(10.0, 0.0, 0.0)));
        click(&mut world, id);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpell {
                spell_id: HEAL,
                target: Some(ALLY),
            })
        ));
        assert!(world
            .resource::<crate::ui_action::CastErrors>()
            .0
            .is_empty());
        assert!(!world.resource::<super::super::SpellTargeting>().active());
    }

    /// `BindTarget`'s corpse arm from a world click on a released player's corpse: the ally bit on
    /// a friend's corpse commits with `0x8000` and the corpse guid, the enemy bit on a hostile one
    /// with `0x200`; bones, a hostile corpse under the ally bit and a word the bit does not empty
    /// wait silently; out of range raises "Out of range." and waits. An item's corpse cast (Goblin
    /// Jumper Cables) commits as `CMSG_USE_ITEM` with the same block.
    #[test]
    fn a_click_on_a_released_corpse_binds_the_corpse_arm() {
        use super::super::corpse_fixture as fx;
        use benilla_protocol::messages::{CorpseTarget, UseItemTarget};
        let commit = crate::spell::cast_send::CastCommit::Spell;

        let clicked = |word: u16,
                       commit: crate::spell::cast_send::CastCommit,
                       corpse: crate::net::ObjectStore,
                       distance: f32| {
            let (mut world, rx, id) = fixture();
            world.insert_resource(fx::spells());
            world.insert_resource(fx::factions());
            world.spawn((
                crate::net::SelfPlayer,
                GlobalTransform::default(),
                fx::caster(1.5),
            ));
            let body = world
                .spawn((
                    GlobalTransform::from_translation(Vec3::new(distance, 0.0, 0.0)),
                    corpse,
                ))
                .id();
            world.resource_mut::<crate::target::PressPick>().hovered = Hovered {
                corpse: Some(body),
                corpse_guid: Some(fx::CORPSE),
                distance: 5.0,
                ..Hovered::default()
            };
            world.resource_mut::<super::super::SpellTargeting>().enter(
                fx::RESURRECTION,
                commit,
                word,
            );
            click(&mut world, id);
            let sent = rx.try_recv().ok();
            let errors =
                std::mem::take(&mut world.resource_mut::<crate::ui_action::CastErrors>().0);
            let armed = world.resource::<super::super::SpellTargeting>().active();
            (sent, errors, armed)
        };
        let friend = || fx::corpse(fx::HUMAN, false);
        let foe = || fx::corpse(fx::ORC, false);

        let (sent, errors, armed) = clicked(0x8000, commit, friend(), 10.0);
        assert!(
            matches!(
                sent,
                Some(ClientCommand::CastSpellCorpse {
                    spell_id: fx::RESURRECTION,
                    target: CorpseTarget::Ally,
                    corpse_guid: fx::CORPSE,
                })
            ),
            "{sent:?}"
        );
        assert!(errors.is_empty() && !armed, "the bind ends the cursor");

        let (sent, _, _) = clicked(0x0200, commit, foe(), 10.0);
        assert!(matches!(
            sent,
            Some(ClientCommand::CastSpellCorpse {
                target: CorpseTarget::Enemy,
                ..
            })
        ));

        // Silent refusals: nothing sent, no error, the cursor still up (`6e6026`).
        for (word, corpse, what) in [
            (0x8000, fx::corpse(fx::HUMAN, true), "bones"),
            (0x8000, foe(), "a hostile corpse under the ally bit"),
            (0x8002, friend(), "a word the corpse bit does not empty"),
            (0x0002, friend(), "a word with no corpse bit"),
        ] {
            let (sent, errors, armed) = clicked(word, commit, corpse, 10.0);
            assert!(sent.is_none() && errors.is_empty() && armed, "{what}");
        }

        // Out of range (30 + 2 × 1.5 = 33): "Out of range." and the cursor stays.
        let (sent, errors, armed) = clicked(0x8000, commit, friend(), 40.0);
        assert!(sent.is_none() && armed);
        assert_eq!(
            errors,
            vec![crate::ui_action::CastFail::local(fx::RESURRECTION, 0x59)]
        );

        // An item's cast: the same corpse block under `CMSG_USE_ITEM`.
        let cables = crate::spell::cast_send::CastCommit::Item {
            bag_index: 255,
            slot: 24,
            entry: 7148,
            spell_index: 0,
            on_object: None,
        };
        let (sent, _, _) = clicked(0x8000, cables, friend(), 10.0);
        assert!(
            matches!(
                sent,
                Some(ClientCommand::UseItem {
                    target: UseItemTarget::Corpse(CorpseTarget::Ally, fx::CORPSE),
                    ..
                })
            ),
            "{sent:?}"
        );
    }

    /// Every way the object click must not commit.
    #[test]
    fn the_object_commit_holds_its_fire() {
        let commit = crate::spell::cast_send::CastCommit::Spell;

        // (a) Nothing armed: the click is drained so it cannot replay once the mode turns on.
        let (mut world, rx, id) = fixture();
        hover_go(&mut world, 5.0);
        click(&mut world, id);
        assert!(rx.try_recv().is_err(), "idle binds nothing");
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(OPENING, commit, LOCK_WORD);
        world.run_system(id).expect("the next frame, no new click");
        assert!(
            rx.try_recv().is_err(),
            "a click buffered while idle must not replay once the word stands"
        );

        // (b) A poison's bare ITEM word: `0x10 & 0x4800 == 0`, nothing binds, the cursor stays.
        let (mut world, rx, id) = fixture();
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(8679, commit, 0x0010);
        hover_go(&mut world, 5.0);
        click(&mut world, id);
        assert!(rx.try_recv().is_err(), "a poison has no world leg");
        assert!(
            world.resource::<super::super::SpellTargeting>().active(),
            "and the cursor survives the click it cannot consume"
        );

        // (c) Nothing hovered: `BindTarget` is never reached.
        let (mut world, rx, id) = fixture();
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(OPENING, commit, LOCK_WORD);
        click(&mut world, id);
        assert!(rx.try_recv().is_err(), "no object, no bind");
        assert!(world.resource::<super::super::SpellTargeting>().active());

        // (d) A unit is nearer than the GameObject: every unit arm of `0x6e5b40` tests a bit this
        // word lacks, so nothing is written.
        let (mut world, rx, id) = fixture();
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(OPENING, commit, LOCK_WORD);
        hover_go(&mut world, 9.0);
        world.resource_mut::<crate::target::PressPick>().hovered = Hovered {
            target: Some(Entity::from_raw_u32(2).unwrap()),
            guid: Some(0x1234),
            distance: 4.0,
            ..Hovered::default()
        };
        click(&mut world, id);
        assert!(rx.try_recv().is_err(), "a unit in front of the chest wins");
        assert!(world.resource::<super::super::SpellTargeting>().active());
    }
}
