//! Mounts: `UNIT_FIELD_MOUNTDISPLAYID` drawn as a second creature visual, a child entity with its
//! own `NetEntity` built by the ordinary attach path. The rider's rig roots under the mount's
//! attachment-0 seat (`0x60ce70`) and holds Mount (91), while the mount child walks on the host's
//! movement ([`crate::creature_anim`]).
//!
//! The change handler `0x5ffa50` tears the old seat down, then builds the new, and neither half
//! touches the rider's model. Mounting (`0x607a00`) sets the mount model (`0x613d80`, into
//! `CGUnit+0xdc`) and re-parents the body onto its attachment 0 (`0x712f70`), recomposed each frame
//! from that live bone (`0x718657`..`0x71876f`); dismounting (`0x607ce0`) detaches the body
//! (`0x713020`), destroys the mount model and stands the body, instantly. Here the rig's consumer
//! anchors move between the unit's frame and the seat, re-pointing [`RigPose::joints_root`], so
//! nothing that rides the rider is rebuilt.

use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::net::{NetEntity, ObjectStore};
use benilla_world::rig_anim::RigPose;

/// The mount child, the client's `unit+0xdc` second model.
#[derive(Component)]
pub(crate) struct MountBody {
    /// The mounted unit, whose movement the animation driver reads to walk the mount.
    pub(crate) host: Entity,
}

/// On the unit: its live mount child (the client's `[unit+0xdc]` handle).
#[derive(Component)]
pub(crate) struct MountChild(pub(crate) Entity);

/// On the unit: the mount display its rig is seated on (`0` standing), [`reseat_mounts`]' diff
/// key; it lags the field while a mount model loads.
#[derive(Component)]
pub(super) struct AppliedMount(pub(super) u32);

/// Spawn a unit's mount child (the client's `0x607a00` model creation), built as an ordinary
/// creature. `scale` is the `CreatureDisplayInfo.creatureModelScale` column alone (`0x607a75`), as
/// the unit's `SCALE_X` composes through the hierarchy. `fade_skip` drops the streaming fade, so a
/// cast mount is there at once: our choice, as whether the reference fades a new mount in is
/// untraced.
pub(super) fn spawn_mount_child(
    commands: &mut Commands,
    host: Entity,
    display: u32,
    scale: f32,
    fade_skip: bool,
) -> Entity {
    let child = commands
        .spawn((
            NetEntity {
                kind: EntityKind::Unit,
                display_id: Some(display),
                scale,
            },
            MountBody { host },
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    if fade_skip {
        commands.entity(child).insert(super::equipment::Reattached);
    }
    commands.entity(host).add_child(child);
    commands.entity(host).insert(MountChild(child));
    child
}

/// The seat anchor, the rider's model frame while mounted: under the mount's attachment-0 joint,
/// counter-scaled so the rider keeps its size (`0x607b49`, `0x710620`), and a
/// [`benilla_world::rig_anim::RigFrame`] so a re-seat cascades into the rider's palette.
fn spawn_seat_anchor(
    commands: &mut Commands,
    joint: Entity,
    offset: Vec3,
    mount_scale: f32,
    rider: Entity,
) -> Entity {
    let anchor = commands
        .spawn((
            Transform::from_translation(offset)
                .with_scale(Vec3::splat(1.0 / mount_scale.max(0.001))),
            Visibility::default(),
            benilla_world::rig_anim::RigFrame(rider),
        ))
        .id();
    commands.entity(joint).add_child(anchor);
    anchor
}

/// Where the seat leg leaves the rider: [`Seat::Wait`] while the mount model builds,
/// [`Seat::Frame`] to root there, or [`Seat::UnitMatrix`] when the mount authors no attachment 0
/// and the reference leaves the body at the unit matrix (`0x60ce70`'s miss).
pub(super) enum Seat {
    Wait,
    Frame(Entity),
    UnitMatrix,
}

/// The mount children's build state: attached yet, its anchors, and the display it was built
/// with, which the field can move off while the model loads.
pub(super) type MountChildren<'w, 's> = Query<
    'w,
    's,
    (
        Has<super::VisualAttached>,
        Option<&'static super::BoneAttach>,
        &'static NetEntity,
    ),
    With<MountBody>,
>;

/// The seat leg of the first build and the transition: spawn the mount child when missing, drop
/// one built for a display the field has left, and resolve the seat once its rig is up.
pub(super) fn seat_or_spawn_mount(
    commands: &mut Commands,
    children: &MountChildren,
    poses: &mut Query<&mut RigPose>,
    creatures: Option<&super::Creatures>,
    unit: Entity,
    child: Option<Entity>,
    mount_display: u32,
    fade_skip: bool,
) -> Seat {
    // `0x607a75`: `SCALE_X` × `creatureModelScale` alone, without `CreatureModelData.modelScale`.
    let mount_scale = creatures
        .and_then(|c| c.catalog.display_scale(mount_display))
        .unwrap_or(1.0);
    let Some(child) = child else {
        spawn_mount_child(commands, unit, mount_display, mount_scale, fade_skip);
        return Seat::Wait;
    };
    // Built for a display the field has left, or gone: drop it and start over next pass.
    let built = children.get(child).map(|(_, _, n)| n.display_id).ok();
    if built != Some(Some(mount_display)) {
        if let Ok(mut ec) = commands.get_entity(child) {
            ec.despawn();
        }
        commands.entity(unit).remove::<MountChild>();
        return Seat::Wait;
    }
    let Ok((true, Some(bones), _)) = children.get(child) else {
        return Seat::Wait; // the mount child is still building
    };
    // The mount's attachment-0 joint, made on demand in the mount's own `RigPose.anchors`, where
    // the world pass's patch walk finds the seat and cascades the rider.
    let Some((joint, offset)) = bones.points.get(&0).and_then(|&(bone, offset)| {
        poses
            .get_mut(child)
            .ok()
            .and_then(|mut p| p.anchor_for(commands, child, bone))
            .map(|j| (j, offset))
    }) else {
        warn!(
            "MOUNTDISPLAYIDNOMOUNTATTACHMENT: display {mount_display} authors no attachment 0 — \
             rider stays at the unit matrix"
        );
        return Seat::UnitMatrix;
    };
    Seat::Frame(spawn_seat_anchor(
        commands,
        joint,
        offset,
        mount_scale,
        unit,
    ))
}

/// The rider's ground frame: its own entity, or a fresh conform node under it for a model the
/// reference tilts to the terrain (`GlobalModelFlags & 3` of 1 or 3). Mounted, the composite
/// tilts through the mount's node instead.
fn ground_frame(
    commands: &mut Commands,
    unit: Entity,
    net: &NetEntity,
    creatures: Option<&super::Creatures>,
) -> Entity {
    let tilt = net
        .display_id
        .and_then(|d| creatures?.models.get(&d))
        .map_or(0, |dm| dm.terrain_tilt);
    if tilt == 0 {
        return unit;
    }
    let node = commands
        .spawn((
            super::conform::ConformNode { unit, mode: tilt },
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    commands.entity(unit).add_child(node);
    node
}

/// Move a rig's consumer anchors onto `frame` and make it the model frame, as `0x712f70` attaches
/// and `0x713020` detaches; the anchors carry every held item, effect and overhead rider.
/// `pose_dirty` re-seats them and the palette in the same frame's compose.
fn reseat_rig(commands: &mut Commands, rig: &mut RigPose, frame: Entity, conform: bool) {
    if rig.joints_root == frame {
        return;
    }
    let anchors: Vec<Entity> = rig.anchors.iter().map(|&(_, a)| a).collect();
    if !anchors.is_empty() {
        commands.entity(frame).add_children(&anchors);
    }
    let old = std::mem::replace(&mut rig.joints_root, frame);
    rig.pose_dirty = true;
    // A conform node the rig just left is empty, so it goes; a seat anchor dies with its mount.
    if conform {
        if let Ok(mut ec) = commands.get_entity(old) {
            ec.despawn();
        }
    }
}

/// Re-seat a unit's rig when its mount field changes, the client's `0x5ffa50`: tear the old seat
/// down, then build the new, with the rider's own visual untouched. The legs run in separate
/// frames, so a remount stands the rider on its feet while the new mount model loads.
#[allow(clippy::type_complexity)]
pub(super) fn reseat_mounts(
    mut commands: Commands,
    units: Query<
        (
            Entity,
            &NetEntity,
            &ObjectStore,
            Option<&AppliedMount>,
            Option<&MountChild>,
        ),
        With<super::VisualAttached>,
    >,
    // One pose query for both sides, the rider's rig and the mount's, borrowed in sequence below.
    mut poses: Query<&mut RigPose>,
    children: MountChildren,
    conforms: Query<(), With<super::conform::ConformNode>>,
    creatures: Option<Res<super::Creatures>>,
) {
    for (entity, net, store, applied, child) in &units {
        if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
            continue;
        }
        let live = store.0.unit_mount_display_id();
        let applied = applied.map_or(0, |a| a.0);
        // Settled: seated on what the field says, and holding no mount model when it says none.
        // A mount-up whose field zeroes before the model loads leaves an unseated child (`applied`
        // never left 0), which would otherwise stand under the dismounted rider for good.
        if live == applied && (live != 0 || child.is_none()) {
            continue;
        }
        // A model-less unit (the cube) has no rig to seat: stamp the field so it never churns, and
        // sweep any child, which the settled test would otherwise re-fire on every frame.
        let Ok(seated_root) = poses.get(entity).map(|r| r.joints_root) else {
            if let Some(&MountChild(child)) = child {
                if let Ok(mut ec) = commands.get_entity(child) {
                    ec.despawn();
                }
                commands.entity(entity).remove::<MountChild>();
            }
            commands.entity(entity).insert(AppliedMount(live));
            continue;
        };
        let child = child.map(|&MountChild(c)| c);
        // ── Leg 1 · `0x607ce0`: detach the body onto its own frame, destroy the mount model.
        // Taken when seated on a mount the field left or when the field says none; a unit waiting
        // on its first mount model must not take it, or it would tear down its own order.
        if applied != 0 || live == 0 {
            // Only a body on a seat detaches: a rig on its own entity or its conform node stays,
            // or a second conform node would be orphaned beside the first.
            if seated_root != entity && !conforms.contains(seated_root) {
                let ground = ground_frame(&mut commands, entity, net, creatures.as_deref());
                if let Ok(mut rig) = poses.get_mut(entity) {
                    reseat_rig(&mut commands, &mut rig, ground, false);
                }
            }
            if let Some(child) = child {
                if let Ok(mut ec) = commands.get_entity(child) {
                    ec.despawn();
                }
            }
            commands
                .entity(entity)
                .remove::<MountChild>()
                .insert(AppliedMount(0));
            continue;
        }
        // ── Leg 2 · `0x607a00`: build the mount model and attach the body to its seat.
        match seat_or_spawn_mount(
            &mut commands,
            &children,
            &mut poses,
            creatures.as_deref(),
            entity,
            child,
            live,
            true,
        ) {
            Seat::Wait => {}
            Seat::Frame(anchor) => {
                if let Ok(mut rig) = poses.get_mut(entity) {
                    let leaving_conform = conforms.contains(seated_root);
                    reseat_rig(&mut commands, &mut rig, anchor, leaving_conform);
                }
                commands.entity(entity).insert(AppliedMount(live));
            }
            Seat::UnitMatrix => {
                commands.entity(entity).insert(AppliedMount(live));
            }
        }
    }
}

#[cfg(test)]
mod tests;
