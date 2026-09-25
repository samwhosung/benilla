//! Live appearance changes: a values delta that moves `UNIT_FIELD_DISPLAYID` or
//! `GAMEOBJECT_DISPLAYID` swaps the entity's model in place, and one that moves
//! `OBJECT_FIELD_SCALE_X` eases its render scale.
//!
//! The reference's display handler rebuilds the model (`0x60abe0`) when the display record changed
//! (`0x60ae10`, against `[unit+0xb34]`), re-resolving its model facts (`0x60afb0`) and the stand or
//! ride animation (`0x60ce70`): an instant swap, a ghost's revive too. Its scale handler eases
//! over 2 s with a cosine smoothstep (`0x614bbf`). Here a swap tears the visual down for
//! `attach_entity_visuals` to rebuild without the spawn fade; the unit's
//! [`super::spell_fx::FxAttached`] list outlives it, as the reference's per-unit `+0xb4` effect
//! list does, so persistent effects re-spawn on the new body.

use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::net::{Guid, NetEntity, ObjectStore};

use super::collision_height::{collision_height_for, CollisionHeight};
use super::{Creatures, VisualAttached};

/// The reference's scale-ease window: 2 s, cosine smoothstep (`0x614bbf`).
const SCALE_EASE_SECS: f32 = 2.0;

/// The display id the visual was built with, cube included: [`refresh_live_display`]'s diff key.
#[derive(Component)]
pub(super) struct AppliedDisplay(pub(super) Option<u32>);

/// A live display swap tore this entity's visual down. The reference's rebuild ends by replaying
/// the pending morph's impact kit (`0x60ad67`), which `crate::creature_anim`'s morph-latch watcher
/// does on this edge.
#[derive(Message, Clone, Copy)]
pub(crate) struct DisplaySwapped {
    pub(crate) entity: Entity,
}

/// A render-scale ease toward [`NetEntity::scale`], written absolutely each tick so a mid-ease
/// rebuild's snap is overwritten.
#[derive(Component)]
pub(super) struct ScaleEase {
    from: f32,
    to: f32,
    elapsed: f32,
}

/// The live display id, read per kind as `benilla_protocol` reads it at create: `0` is absent, so
/// it never tears a visual down to a cube.
fn live_display_id(kind: EntityKind, store: &ObjectStore) -> Option<u32> {
    match kind {
        EntityKind::Unit | EntityKind::Player => store
            .0
            .unit_displayid()
            .filter(|&d| d > 0)
            .map(|d| d as u32),
        EntityKind::GameObject => store
            .0
            .gameobject_displayid()
            .filter(|&d| d > 0)
            .map(|d| d as u32),
        // A death-time snapshot. The in-place flesh-to-bones flip, which the reference reloads on
        // (`CORPSE_FIELD_FLAGS` handler `0x5d5fa0`, `0x5d6d60`), is not built: vmangos removes the
        // corpse and adds a separate bones object under the same guid (`Map.cpp:3608-3649`).
        EntityKind::Corpse => store.0.corpse_display_id(),
        _ => None,
    }
}

/// Diff each attached entity's live appearance against what its visual was built with: a display
/// move swaps the model, a scale move arms the 2 s ease, and a change of the native display or
/// `SCALE_X` restamps [`CollisionHeight`]. Mount children carry no [`ObjectStore`]: their display
/// is the host's field, diffed by `mount::reseat_mounts`.
#[allow(clippy::type_complexity)]
pub(super) fn refresh_live_display(
    mut commands: Commands,
    creatures: Option<Res<Creatures>>,
    mut entities: Query<
        (
            Entity,
            &Guid,
            &mut NetEntity,
            &ObjectStore,
            &AppliedDisplay,
            Option<&CollisionHeight>,
            &Transform,
        ),
        With<VisualAttached>,
    >,
    mut swapped: MessageWriter<DisplaySwapped>,
) {
    for (entity, guid, mut net, store, applied, height, tf) in &mut entities {
        // ── The display-id swap ──────────────────────────────────────────────────────────────
        let live = live_display_id(net.kind, store);
        if let Some(live) = live {
            if applied.0 != Some(live) {
                info!(
                    "display swap: guid {:016x} {:?} {:?} -> {} (instant, the 0x60abe0 rebuild)",
                    guid.0, net.kind, applied.0, live
                );
                net.display_id = Some(live);
                swapped.write(DisplaySwapped { entity });
                // `attach_entity_visuals` rebuilds from scratch, fade-skipped by `Reattached`.
                commands
                    .entity(entity)
                    .despawn_related::<Children>()
                    .remove::<(
                        VisualAttached,
                        AppliedDisplay,
                        super::equipment::AppliedEquipment,
                        super::mount::AppliedMount,
                        super::mount::MountChild,
                        AnimationPlayer,
                        bevy::animation::transition::AnimationTransitions,
                        AnimationGraphHandle,
                        benilla_assets::ModelAnimations,
                        crate::creature_anim::AnimDriver,
                        (
                            benilla_world::rig_anim::RigPose,
                            crate::creature_anim::BodyTwist,
                            benilla_world::rig_anim::GlobalSeqDrive,
                        ),
                        benilla_world::rig_palette::RigSkin,
                        super::BoneAttach,
                        super::equipment::HeldAttached,
                    )>()
                    .insert(super::equipment::Reattached);
            }
        }

        // ── The scale ease ───────────────────────────────────────────────────────────────────
        // The kinds the create path scales: a kind whose create ignored the field ignores its
        // deltas too, or the first delta would undo the create's 1.0.
        let scaled_kind = matches!(
            net.kind,
            EntityKind::Unit | EntityKind::Player | EntityKind::GameObject | EntityKind::Corpse
        );
        if let Some(live) = store.0.object_scale_x().filter(|s| *s > 0.0 && scaled_kind) {
            if live != net.scale {
                info!(
                    "scale change: guid {:016x} {} -> {} (2 s cosine ease, 0x614bbf)",
                    guid.0, net.scale, live
                );
                net.scale = live;
                commands.entity(entity).insert(ScaleEase {
                    from: tf.scale.x,
                    to: live,
                    elapsed: 0.0,
                });
            }
        }

        // ── The collision prism ──────────────────────────────────────────────────────────────
        // From the native display (`[unit+0x110]+0x1f8`) and `SCALE_X`, not the rendered swap
        // above; recomputed and diffed each frame, as no change gate covers both. It snaps to the
        // target scale: easing it would drag the depth lines through two seconds of depths.
        let prism = super::collision_height::prism_display_id(Some(store), net.display_id);
        let h = collision_height_for(creatures.as_deref(), prism, net.scale);
        if height != Some(&h) {
            debug!(
                "collision height restamp: guid {:016x} {:?} -> {:.3} (native display {:?}, \
                 rendered {:?}, scale {:.3})",
                guid.0,
                height.map(|c| c.0),
                h.0,
                prism,
                net.display_id,
                net.scale
            );
            commands.entity(entity).insert(h);
        }
    }
}

/// Tick every [`ScaleEase`], `from + (to − from) · (0.5 − 0.5·cos(π·t/2 s))` (`0x614bbf`), then
/// land exactly on the target.
pub(super) fn tick_scale_ease(
    mut commands: Commands,
    time: Res<Time>,
    mut easing: Query<(Entity, &mut Transform, &mut ScaleEase)>,
) {
    for (entity, mut tf, mut ease) in &mut easing {
        ease.elapsed += time.delta_secs();
        let t = (ease.elapsed / SCALE_EASE_SECS).min(1.0);
        let w = 0.5 - 0.5 * (std::f32::consts::PI * t).cos();
        tf.scale = Vec3::splat(ease.from + (ease.to - ease.from) * w);
        if t >= 1.0 {
            tf.scale = Vec3::splat(ease.to);
            commands.entity(entity).remove::<ScaleEase>();
        }
    }
}

/// Palette headroom the healer waits for, under the doodad reaper's low-water (256) so the reaper
/// makes room first; rebuilding into a tight table would starve again.
const HEAL_MIN_HEADROOM: usize = 128;

/// Rebuilds per frame, so a starved stream-in burst heals over a second or two, not in one spike.
const HEAL_PER_FRAME: usize = 2;

/// Rebuild units whose attach was denied a palette rig (`RigStarved`), which otherwise stay at bind
/// pose: the display swap's teardown, fade-skipped; a table full again re-marks them.
#[allow(clippy::type_complexity)] // one query's two-marker filter
pub(super) fn heal_rig_starved(
    mut commands: Commands,
    palettes: Res<benilla_world::rig_palette::RigPalettes>,
    starved: Query<
        (Entity, &Guid),
        (
            With<benilla_world::rig_palette::RigStarved>,
            With<VisualAttached>,
        ),
    >,
) {
    if starved.is_empty() || palettes.slot_headroom() < HEAL_MIN_HEADROOM {
        return;
    }
    for (entity, guid) in starved.iter().take(HEAL_PER_FRAME) {
        info!(
            "rig heal: guid {:016x} was denied a palette rig at attach — rebuilding (0x60abe0 shape)",
            guid.0
        );
        commands
            .entity(entity)
            .despawn_related::<Children>()
            .remove::<(
                VisualAttached,
                AppliedDisplay,
                super::equipment::AppliedEquipment,
                super::mount::AppliedMount,
                super::mount::MountChild,
                AnimationPlayer,
                bevy::animation::transition::AnimationTransitions,
                AnimationGraphHandle,
                benilla_assets::ModelAnimations,
                crate::creature_anim::AnimDriver,
                (
                    benilla_world::rig_anim::RigPose,
                    crate::creature_anim::BodyTwist,
                    benilla_world::rig_anim::GlobalSeqDrive,
                ),
                benilla_world::rig_palette::RigSkin,
                super::BoneAttach,
                super::equipment::HeldAttached,
                benilla_world::rig_palette::RigStarved,
            )>()
            .insert(super::equipment::Reattached);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_starved_unit_rebuilds_when_the_table_has_room_and_waits_when_it_has_not() {
        let mut app = App::new();
        app.init_resource::<benilla_world::rig_palette::RigPalettes>();
        app.add_systems(Update, heal_rig_starved);
        let child = app.world_mut().spawn_empty().id();
        let unit = app
            .world_mut()
            .spawn((
                benilla_world::rig_palette::RigStarved,
                VisualAttached,
                crate::net::Guid(0xB0B),
                AppliedDisplay(Some(7)),
            ))
            .add_child(child)
            .id();

        // Fill the table past the heal's minimum headroom: the unit is left alone.
        let hoard: Vec<benilla_world::rig_palette::RigSkin> = {
            let mut palettes = app
                .world_mut()
                .resource_mut::<benilla_world::rig_palette::RigPalettes>();
            (0..(benilla_world::mesh_tag::MAX_RIG_SLOTS - 1 - HEAL_MIN_HEADROOM / 2))
                .filter_map(|_| {
                    benilla_world::rig_palette::RigSkin::allocate_bones(
                        &mut palettes,
                        1,
                        Handle::default(),
                    )
                })
                .collect()
        };
        app.update();
        assert!(
            app.world().entity(unit).contains::<VisualAttached>(),
            "no headroom ⇒ the healer waits"
        );

        // Room opens, as the reaper makes it in the app: the rebuild teardown lands.
        {
            let mut palettes = app
                .world_mut()
                .resource_mut::<benilla_world::rig_palette::RigPalettes>();
            for rig in hoard.iter().take(HEAL_MIN_HEADROOM) {
                let slot = rig.slot;
                palettes.free(slot);
            }
        }
        app.update();
        let e = app.world().entity(unit);
        assert!(!e.contains::<VisualAttached>(), "attach re-armed");
        assert!(
            !e.contains::<benilla_world::rig_palette::RigStarved>(),
            "marker consumed"
        );
        assert!(
            e.contains::<super::super::equipment::Reattached>(),
            "fade-skipped rebuild — a heal is not a spawn"
        );
        assert!(
            app.world().get_entity(child).is_err(),
            "the old visual's children despawned"
        );
    }

    #[test]
    fn scale_ease_is_the_2s_cosine_smoothstep() {
        let w = |elapsed: f32| {
            let t = (elapsed / SCALE_EASE_SECS).min(1.0);
            0.5 - 0.5 * (std::f32::consts::PI * t).cos()
        };
        assert_eq!(w(0.0), 0.0);
        assert!((w(1.0) - 0.5).abs() < 1e-6); // cos(π/2) = 0: half-way in value at 1 s
        assert_eq!(w(2.0), 1.0);
        assert_eq!(w(3.0), 1.0); // clamped past the window
        assert!(w(0.5) < 0.25);
        assert!(w(1.5) > 0.75);
    }
}
