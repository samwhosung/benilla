//! Mounts (decisions 0441 / this session's re-seat): the projection from
//! `UNIT_FIELD_MOUNTDISPLAYID` — the wire's one mounted signal — to a second creature visual under
//! the unit. The mount is a child entity carrying a plain creature `NetEntity`, built by the
//! ordinary attach path; the rider's rig re-roots under the mount's attachment-0 seat joint (the
//! `0x60ce70` present-test law); the rider's base pins to Mount(91) while the mount child
//! locomotes through the untouched gait driver, fed the host's own movement view
//! ([`crate::creature_anim`]'s host-view redirect). This module owns the transition.
//!
//! ## The transition is a RE-SEAT, not a rebuild
//!
//! The change handler `0x5ffa50` runs *teardown old, then build new*, and neither half touches the
//! rider's own model.
//!
//! - **Mount up, `0x607a00`:** load the mount M2 → `0x613d80 SetMountModel` into `CGUnit+0xdc` →
//!   `0x712f70 CM2Model::attachChild(this = BODY [+0xd8], parent = MOUNT, slot 0)`, which sets
//!   `body+0x1cc = mount` and links the body into the mount's child list. The **body model is
//!   re-parented, not re-created**; its world matrix is recomposed each frame from the mount's live
//!   posed attachment-0 bone (the animate kernel's child recursion `0x718657`–`0x71876f`).
//! - **Dismount, `0x607ce0`:** `0x713020` **detaches** the body, `0x613d80(0)` destroys the mount
//!   model, the body goes to op4 seq 0 (Stand). Instant, no transition anim — and again the body
//!   model itself is untouched.
//!
//! benilla used to answer the same field change with a **full teardown** of the rider's visual
//! (`despawn_related::<Children>()` + a strip of the rig, the player, the driver, the attach table)
//! and let `attach_entity_visuals` rebuild it — the shape 0835 removed from gear changes for the
//! same reason it is wrong here. Everything hanging off the rider died with it, and the one class
//! that never came back was the **persistent aura kit instance**: `FxAttached` lives on the *unit*,
//! survived the teardown holding a dangling root, and `attach_spell_fx`'s spawn gate is
//! `root.is_some()` — so a mage's Ice Barrier glow was destroyed on mount-up and never re-attached,
//! not even on dismount (the aura watcher is edge-driven off the slot set, and the aura never left
//! it). That is B199, and 0835 named exactly this teardown as the surviving source.
//!
//! So the transition now moves the rig's **consumer anchors** — the only children the rig's model
//! frame owns — from the unit's own frame onto the seat anchor and back, and
//! re-points [`RigPose::joints_root`]. Skinned parts render purely from the palette and never move;
//! held items, spell-effect instances and overhead riders hang off the anchors and ride along, so
//! nothing on the rider is destroyed, re-created, re-composited or re-faded by mounting.

use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::net::{NetEntity, ObjectStore};
use benilla_world::rig_anim::RigPose;

/// The mount child — the second creature visual a mounted unit carries (the client's
/// `unit+0xdc` secondary model instance). Spawned by [`seat_or_spawn_mount`] (from the first
/// build or from a transition), despawned by the dismount leg or with its parent.
#[derive(Component)]
pub(crate) struct MountBody {
    /// The mounted unit — the streamed entity whose field this child projects. The animation
    /// driver reads the HOST's movement view through this to locomote the mount.
    pub(crate) host: Entity,
}

/// On the unit: its live mount child (the client's `[unit+0xdc]` handle).
#[derive(Component)]
pub(crate) struct MountChild(pub(crate) Entity);

/// On the unit: the mount display id its rig is currently **seated on** (`0` = standing on its own
/// frame) — [`reseat_mounts`]' diff key, the `AppliedEquipment` pattern. Distinct from the live
/// field while a transition waits on the mount model to load.
#[derive(Component)]
pub(super) struct AppliedMount(pub(super) u32);

/// Spawn a unit's mount child — the client's `0x607a00` model creation. Its `NetEntity` registers
/// the display want (`update_display_models` scans every `NetEntity`) and `attach_entity_visuals`
/// builds it as an ordinary creature; `scale` is the **CDI `creatureModelScale` column alone**
/// (byte law, `0x607a75` — the unit root's `SCALE_X` composes through the hierarchy).
///
/// `fade_skip` suppresses the appear-fade: a mount blinking into existence under its rider is what
/// the teardown-era transition rendered (every rebuilt child carried `Reattached`), and it is what
/// a mount cast should look like — the horse is *there* the instant the cast lands, not ramped in
/// over the streaming fade meant for units coming into view.
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

/// The seat anchor — the rider's model frame while mounted: a child of the mount's attachment-0
/// joint at the authored offset, counter-scaled so the rider keeps its own size (the client's body
/// carries a compensating own/mount base ratio, `0x607b49 fld [esi+0x98]; fdiv [esi+0x9c]` →
/// `0x710620`).
///
/// [`benilla_world::rig_anim::RigFrame`] marks it as `rider`'s model frame so the world pass cascades
/// a re-seat into the rider's palette.
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

/// The seat leg shared by the first build and the transition: given a unit whose mount field says
/// `display`, make sure the mount child exists and — once it has attached — return the frame the
/// rider's rig should root under.
///
/// Three outcomes, and the caller acts on each: [`Seat::Wait`] (the mount model is still
/// building — nothing to do this frame), [`Seat::Frame`] (seat it here) and [`Seat::UnitMatrix`]
/// (the mount authors no attachment 0 — the reference logs and leaves the body at the unit matrix,
/// `0x60ce70`'s present-test miss).
pub(super) enum Seat {
    Wait,
    Frame(Entity),
    UnitMatrix,
}

/// The mount children's build state a [`seat_or_spawn_mount`] caller reads: attached yet, its
/// anchors, and the display it was actually built with (the staleness check — the field can move
/// again while the model loads).
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

/// Drive one unit's mount side to the point where the rider can be seated. Spawns the child when
/// it is missing, drops and retries a child built for a display the field has since moved off, and
/// resolves the seat frame once the child's rig is up.
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
    // The mount scale law (`0x607a75`): rendered =
    // `SCALE_X × CreatureDisplayInfo.creatureModelScale` — the CDI column ALONE (no
    // CreatureModelData.modelScale).
    let mount_scale = creatures
        .and_then(|c| c.catalog.display_scale(mount_display))
        .unwrap_or(1.0);
    let Some(child) = child else {
        spawn_mount_child(commands, unit, mount_display, mount_scale, fade_skip);
        return Seat::Wait;
    };
    // A child built for a display the field has since moved off (or one that is simply gone):
    // drop it and let the next pass start over.
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
    // The mount's attachment-0 joint, spawned on first demand from the MOUNT's composed pose
    // (`RigPose::anchor_for`) — registered into the mount's own `RigPose.anchors`,
    // which is what lets the world pass's patch walk find the seat subtree and cascade the rider.
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

/// The rider's **ground** frame: its own entity, or — for a model the reference tilts to the
/// terrain (`GlobalModelFlags & 3 ∈ {1,3}`) — a fresh conform node under it.
/// The mounted rider never gets one: the tilt dispatch is on the mount-preferred model, and the
/// composite tilts through the *mount's* node, seat joint included.
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

/// Move a rig's consumer anchors onto `frame` and make it the rig's model frame — the ECS twin of
/// `0x712f70 CM2Model::attachChild` / `0x713020`'s detach. The anchors are the only children the
/// frame owns (skinned parts render from the palette and hang off the unit), so
/// this one re-parent carries every held item, spell-effect instance and overhead rider with it.
///
/// `pose_dirty` is raised so the same frame's compose re-seats the anchors' locals and re-writes
/// the palette against the new root, rather than waiting for the next pose change.
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
    // A conform node the rig has just left has nothing under it any more — it belongs to the
    // frame, not to the visual, so it goes with it. (A seat anchor needs no such sweep: it dies
    // with the mount child that carries it.)
    if conform {
        if let Ok(mut ec) = commands.get_entity(old) {
            ec.despawn();
        }
    }
}

/// Re-seat a unit's rig when its mount field changes — the client's `0x5ffa50` change handler:
/// **tear the old seat down, then build the new one**, with the rider's own visual untouched
/// throughout (module docs).
///
/// The two legs are separate frames on purpose, exactly as they are separate calls there: a
/// re-mount (id→id′) detaches onto the ground frame first, and the build leg then waits however
/// many frames the new mount model needs to load. A rider mid-transition therefore stands on its
/// own feet rather than vanishing — the teardown era rendered *nothing at all* for that window.
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
    // One pose query for BOTH sides of the seat — the rider's rig (re-rooted onto the seat) and
    // the mount's (its attachment-0 anchor resolves on demand). Split from the
    // `units` query because the two sides are different entities out of the same component, and
    // borrowed strictly in sequence below.
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
        // Settled: seated on what the field says — and, when the field says nothing, holding no
        // mount model either. The second clause is not redundant: a mount-up whose field zeroes
        // again *before* the model finishes loading leaves an ordered-but-unseated child behind
        // (`applied` never left 0), and without it that horse would stand under a dismounted
        // rider for the rest of the unit's life.
        if live == applied && (live != 0 || child.is_none()) {
            continue;
        }
        // A model-less unit (the cube fallback) has no rig to seat, and the build path never
        // orders it a mount — stamp the field so it can never churn, exactly as the first build
        // does, and sweep a child if one ever did reach it (the stamp alone would then re-fire
        // every frame, since the settled test asks about the child too).
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
        // ── Leg 1 · `0x607ce0` — detach the body onto its own frame, destroy the mount model.
        // It runs when we are seated on a mount the field has moved off, and when the field says
        // "not mounted" at all (which covers the pending-child case above). A unit merely *waiting*
        // for its first mount model must not take it, or the build leg would tear down the child
        // it just ordered, every frame, for ever.
        if applied != 0 || live == 0 {
            // Only a body that is actually ON a seat needs detaching: a rig already standing —
            // on its own entity, or on the conform node that tilts it — is where it belongs, and
            // asking for a ground frame anyway would orphan a second conform node beside the
            // first.
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
        // ── Leg 2 · `0x607a00` — build the mount model and attach the body to its seat.
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
