//! **Carried M2 lights** — the dynamic point lights an *entity* brings into the world, as opposed to
//! the ones a placed ADT doodad / WMO prop brings (`benilla_world::terrain_stream`'s `spawn_lights_for`).
//!
//! The law is one law. `0x718960` runs per frame over **every** CM2Model the scene draws — a placed
//! doodad, a creature, a GameObject, and (recursing at `7191b9`/`719286`) each attached child model —
//! gathers that model's own `type==1` light blocks, transforms each def position by its **live bone
//! matrix**, and registers the result into the world scene's light DB (`0x71b650` → `0x71bb60`). Every
//! lit surface then selects its ≤3 nearest from that same DB. Nothing in the
//! chain distinguishes "prop" from "unit": a torch is a torch whether it is staked in the ground or
//! held in a hand.
//!
//! benilla had implemented only the placed half, so a torch-bearing NPC carried a flame that lit
//! nothing — the director's report from Westfall (Remy "Two Times", whose `Club_1H_Torch_A_01.m2`
//! authors exactly one warm point light) is the reference doing the other half: the fence rails and
//! the grass around him light up.
//!
//! **The bone ride is the whole reason this isn't just the placed spawner again.** A placed prop's
//! light bone never moves, so the rest pose is exact and the light can be baked to a world point. An
//! entity's does move — the hand swings — so each light is spawned as a **child of its host bone's
//! joint entity** with the def position rebased into that bone's frame (`position − bone_pivot`),
//! exactly as the emitters and ribbons ride (0130 phase 4). Bevy's transform propagation then walks
//! the light through the animation for free, and the per-frame light packer
//! ([`benilla_world::lighting`]) reads its `GlobalTransform` like any other point light.

use benilla_assets::coords::wow_to_bevy;
use benilla_assets::ModelLight;
use bevy::prelude::*;

use benilla_world::terrain_stream::point_light;

/// Spawn a `PointLight` child for each **casting** (`type==1`, not visibility-gated dark) M2 light of
/// an entity's model.
///
/// `joint` resolves a light's host bone index to the instance's live joint entity — `None` for a
/// boneless/skeleton-less instance (a held item spawns no skeleton; its `root` already *is* the
/// item's model frame), a `-1` bone, or a bone the instance doesn't carry. A light with a joint rides
/// it in bone-local space; a light without one hangs off `frame` in plain model space, which is the
/// exact rest-pose special case.
///
/// Children, not free entities: the light's lifecycle and its frame both come from the hierarchy, so
/// a gear change, a despawn, or a mount transition takes its lights with it.
pub(super) fn spawn_carried_lights(
    commands: &mut Commands,
    lights: &[ModelLight],
    frame: Entity,
    joint: impl Fn(i16) -> Option<Entity>,
) {
    for l in lights {
        if !l.def.casts() {
            continue; // directionals feed an ambient term; a static `0` visibility key is dark
        }
        let (parent, local) = match joint(l.def.bone) {
            Some(j) => (
                j,
                [
                    l.def.position[0] - l.bone_pivot[0],
                    l.def.position[1] - l.bone_pivot[1],
                    l.def.position[2] - l.bone_pivot[2],
                ],
            ),
            None => (frame, l.def.position),
        };
        let glow = commands
            .spawn((
                point_light(l.def.diffuse_color, l.def.diffuse_intensity),
                Transform::from_translation(wow_to_bevy(local)),
                Visibility::default(),
            ))
            .id();
        commands.entity(parent).add_child(glow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::M2Light;

    fn light(light_type: u16, bone: i16, position: [f32; 3], visibility_off: bool) -> ModelLight {
        ModelLight {
            def: M2Light {
                light_type,
                bone,
                position,
                bone_z: [0.0, 0.0, 1.0],
                ambient_color: [1.0; 3],
                ambient_intensity: 0.0,
                diffuse_color: [0.466_666_7, 0.290_196_1, 0.133_333_34], // the real torch's warm orange
                diffuse_intensity: 3.0,
                attenuation_start: 1.388_889,
                attenuation_end: 2.222_222_3,
                visibility_off,
            },
            bone_pivot: [1.0, 0.0, 0.5],
        }
    }

    /// GOLDEN — the carried-light spawn law. Only a **casting** light spawns (`type==1`, not held
    /// dark by a static `0` visibility key — the gather gates in `0x718960`, the shape 11 of
    /// the corpus's 85 point lights actually ship), it lands as a CHILD of its host bone's joint
    /// so the animation carries it, and its offset is the def position rebased into that bone's
    /// frame (`position − bone_pivot`, wow→bevy). Colour × intensity survives the `PointLight`
    /// round trip the packer inverts (`intensity/4π`).
    #[test]
    fn only_casting_lights_spawn_and_they_ride_their_bone() {
        let mut app = App::new();
        let frame = app.world_mut().spawn(Transform::IDENTITY).id();
        let joint = app.world_mut().spawn(Transform::IDENTITY).id();
        let lights = [
            light(1, 3, [2.0, 0.0, 1.5], false),  // casts, on bone 3
            light(0, 3, [2.0, 0.0, 1.5], false),  // directional → ambient term, never a GL light
            light(1, 3, [2.0, 0.0, 1.5], true),   // point but authored dark
            light(1, -1, [0.0, 0.0, 4.0], false), // casts, boneless → the frame itself
        ];
        app.world_mut().commands().queue(move |world: &mut World| {
            let mut q = world.commands();
            spawn_carried_lights(&mut q, &lights, frame, move |bone| {
                (bone == 3).then_some(joint)
            });
        });
        app.world_mut().flush();

        let mut spawned: Vec<(Entity, Vec3, Entity)> = app
            .world_mut()
            .query::<(
                Entity,
                &benilla_world::lighting::WorldPointLight,
                &Transform,
                &ChildOf,
            )>()
            .iter(app.world())
            .map(|(e, _, t, c)| (e, t.translation, c.parent()))
            .collect();
        // Spawn order, i.e. light-table order — `Entity`'s own `Ord` is not index-ascending.
        spawned.sort_by_key(|(e, ..)| e.index());
        assert_eq!(
            spawned.len(),
            2,
            "the directional and the dark one stay out"
        );

        // Bone-ridden: parented to the joint, offset rebased into the bone frame.
        assert_eq!(spawned[0].2, joint);
        assert_eq!(spawned[0].1, wow_to_bevy([1.0, 0.0, 1.0]));
        // Boneless (`-1`): hangs off the frame at plain model-space position — the rest-pose case
        // a held item always takes (it spawns no skeleton).
        assert_eq!(spawned[1].2, frame);
        assert_eq!(spawned[1].1, wow_to_bevy([0.0, 0.0, 4.0]));

        let pl = app
            .world()
            .entity(spawned[0].0)
            .get::<benilla_world::lighting::WorldPointLight>()
            .unwrap();
        let recovered = pl.intensity / (4.0 * std::f32::consts::PI);
        assert!(
            (pl.color[0] * recovered - 1.4).abs() < 1e-3,
            "colour × intensity survives the packing"
        );
    }
}
