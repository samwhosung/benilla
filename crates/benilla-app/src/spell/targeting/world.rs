//! The world click's two commits, the legs of the click dispatcher `0x492ce0`. While targeting,
//! the pick flags (`0x481050`) come only from the word `0xcecac0`, so the word picks the leg:
//!
//! - terrain (`0x492c90` → `0x492580` → `BindLocation 0x6e60f0`): [`commit_ground_cast_on_click`]
//! - object (`0x4925d0` → `SetSelection 0x493540` → `BindTarget 0x6e5b40`):
//!   [`commit_object_cast_on_click`]
//!
//! A location-only word yields pick flags 3, whose `& 0x7c == 0` disables the object pick, so an
//! AoE reticle clicks through a chest. Neither leg gates on range, validity or the lock: the
//! server judges and its `SMSG_CAST_RESULT` is the refusal.

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
/// `BindTarget` picks its arm by the clicked object's typemask; the GameObject arm tests the word's
/// `0x4800`, writes wire bit `TARGET_FLAG_GAMEOBJECT` (`0x800`), clears those `0x4800` word bits,
/// parks the guid (`0xceac60`), and the zero word lets it call `SendCast 0x6e54f0`. A unit click
/// under a lock word binds nothing, since every unit arm tests a bit the word lacks; this is
/// [`go_is_nearest`] here.
///
/// There is no gate before the send, not range nor the lock: the refusal is the server's
/// `SMSG_CAST_RESULT`. The right-click path ([`crate::target::click`], `0x5f33e0`) does resolve the
/// lock and can refuse locally.
///
/// Runs after `select_on_click`, as the terrain commit does.
pub(crate) fn commit_object_cast_on_click(
    mut clicks: MessageReader<WorldClick>,
    // The press's pick: the object the gesture started on.
    press: Res<crate::target::PressPick>,
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
    // `TargetingWantsGameObject 0x6e62d0`: a poison's bare `0x10` word binds nothing here.
    let Some((spell_id, commit)) = ladder.ground.pending_for(TargetingWants::GameObject) else {
        return;
    };
    // The object leg only wins when a GameObject is the nearest hit.
    if !go_is_nearest(&hovered, &hovered_object) {
        return;
    }
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
