//! The rider lane: an attached model (a helm, a pauldron, a held weapon, a shield) draws from a
//! palette frame in its wearer's rig frame, never from an absolute f32 world position.
//!
//! As a scene-graph child its world position adds a yard-sized bone offset to a coordinate of
//! thousands of yards each frame, landing on the f32 grid (`2⁻¹⁰ yd` from 8192 to 16384) and
//! hopping while the body moves smoothly. At bind pose every palette row is the placement
//! `F = host_basis × pose.model[bone] × T(offset)`, with `host_basis` the host's frame minus its
//! translation, and the slot's `rig_origin` adds the world position back camera-relative.
//!
//! The entity keeps its absolute `GlobalTransform`: collision, picking, the item glow and the
//! bowstring read it; only the rendered vertices take this route.

use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

use crate::rig_anim::RigPose;
use crate::rig_palette::{rebase_origin, RigPalettes, RigSkin};

/// An attached model placed from a bone of `host`'s rig, in that rig's frame; it sits beside the
/// [`crate::rig_palette::RigSkin`] that owns its palette slot, so both go on one despawn.
#[derive(Component, Clone, Copy)]
pub struct RigRider {
    /// The rig this model hangs off.
    pub host: Entity,
    /// The host bone it hangs from. Attachment points never sit on a camera-faced bone, so the
    /// composed `pose.model`, without the billboard replacement, is the right frame.
    pub bone: u16,
    /// The attachment point's own offset from that bone, model space.
    pub local: Vec3,
    /// The palette slot written; a bind-pose rider repeats one frame into every row.
    pub slot: u16,
}

/// Writes every rider's frame, with the palette's writers ([`crate::rig_palette::plugin`]): after
/// propagation and the billboard joint pass, before the publish. A missing host, pose or bone
/// leaves the rows at their last frame; [`RigPalettes::write_rider`] is idempotent, so a standing
/// unit's riders cost a compare. A rider with its own pose, the ranged weapon the reference
/// re-arms on `[CGUnit+0xd24]`'s instance (BowPull 160 at `$BWP`, BowRelease 161 or Stand 0 at
/// `$BWR`), writes row `b` as `F × own.model[b] × ibp[b]`, every frame.
pub(crate) fn write_rig_riders(
    riders: Query<(Entity, &RigRider)>,
    poses: Query<&RigPose>,
    skins: Query<&RigSkin>,
    frames: Query<&GlobalTransform>,
    ibps: Res<Assets<SkinnedMeshInverseBindposes>>,
    // Row scratch for the posed arm, reused across frames and riders.
    mut scratch: Local<Vec<GlobalTransform>>,
    mut palettes: ResMut<RigPalettes>,
) {
    for (entity, rider) in &riders {
        let Ok(pose) = poses.get(rider.host) else {
            continue;
        };
        // `joints_root`, never the host entity: the rig's frame is the seat anchor when mounted
        // and the conform node when terrain-tilted, and the body's rows are written from it too.
        let Ok(root_g) = frames.get(pose.joints_root) else {
            continue;
        };
        let Some(bone) = pose.model.get(rider.bone as usize) else {
            continue;
        };
        let origin = rebase_origin(root_g.translation());
        let mut basis = root_g.affine();
        basis.translation -= bevy::math::Vec3A::from(origin);
        let frame = basis * *bone * bevy::math::Affine3A::from_translation(rider.local);
        // A prop with its own pose: its bone frames compose onto the placement and are written as
        // a body's rows.
        let posed = poses
            .get(entity)
            .ok()
            .filter(|own| !own.model.is_empty())
            .zip(skins.get(entity).ok())
            .and_then(|(own, skin)| Some((own, skin, ibps.get(&skin.ibp)?)));
        match posed {
            Some((own, skin, ibp)) => {
                scratch.clear();
                scratch.extend(own.model.iter().map(|m| GlobalTransform::from(frame * *m)));
                palettes.write_rig_worlds(skin, &scratch, ibp, origin);
            }
            None => palettes.write_rider(rider.slot, frame, origin),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig_palette::RigSkin;
    use bevy::math::{Affine3A, DVec3, Mat4};

    #[test]
    fn a_rider_composes_through_the_rigs_own_frame_not_the_units() {
        let mut app = App::new();
        app.init_resource::<RigPalettes>();
        // The posed arm's asset lookup; `AssetPlugin` provides it in the app.
        app.init_resource::<Assets<SkinnedMeshInverseBindposes>>();
        // A saddle, exaggerated so the wrong frame cannot land near the right answer by luck.
        const SEAT: Vec3 = Vec3::new(1.0, 1.5, -2.0);
        let seat = app
            .world_mut()
            .spawn(GlobalTransform::from(Transform {
                translation: SEAT,
                rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                scale: Vec3::ONE,
            }))
            .id();
        // The unit stands somewhere else entirely, as it does while mounted.
        let unit = app
            .world_mut()
            .spawn(GlobalTransform::from(Transform::from_translation(
                Vec3::new(-5.0, 0.0, 7.0),
            )))
            .id();
        let mut pose = crate::testing::test_rig_pose(seat, &[Vec3::ZERO, Vec3::new(0.0, 0.4, 0.0)]);
        pose.compose();
        app.world_mut().entity_mut(unit).insert(pose);
        let skin = {
            let mut palettes = app.world_mut().resource_mut::<RigPalettes>();
            RigSkin::allocate_bones(&mut palettes, 1, Handle::default()).unwrap()
        };
        let slot = skin.slot;
        let local = Vec3::new(0.05, 0.0, 0.02);
        app.world_mut().spawn((
            skin,
            RigRider {
                host: unit,
                bone: 1,
                local,
                slot,
            },
        ));
        app.add_systems(Update, write_rig_riders);
        app.update();

        let (origin, row) = app
            .world()
            .resource::<RigPalettes>()
            .rider_placement(slot)
            .expect("the rider was written");
        // The vertex stage adds the two back together; the pair is what it reads.
        let placed = origin + row;
        let seat_g = *app.world().entity(seat).get::<GlobalTransform>().unwrap();
        let want = seat_g.affine()
            * Affine3A::from_translation(Vec3::new(0.0, 0.4, 0.0))
            * Affine3A::from_translation(local);
        assert!(
            placed.abs_diff_eq(Vec3::from(want.translation), 1.0e-5),
            "the rider must sit on the SEAT's bone: {placed:?} vs {:?}",
            want.translation
        );
        assert_eq!(origin, SEAT, "…and be measured from the rig's own frame");
        let unit_g = *app.world().entity(unit).get::<GlobalTransform>().unwrap();
        let wrong = unit_g.affine()
            * Affine3A::from_translation(Vec3::new(0.0, 0.4, 0.0))
            * Affine3A::from_translation(local);
        assert!(
            placed.distance(Vec3::from(wrong.translation)) > 1.0,
            "the two frames must be far apart, or this test proves nothing"
        );
    }

    #[test]
    fn a_posed_rider_composes_each_bone_instead_of_repeating_the_placement() {
        let mut app = App::new();
        app.init_resource::<RigPalettes>();
        app.init_resource::<Assets<SkinnedMeshInverseBindposes>>();
        // The host: a unit standing off the origin, its own rig frame at its feet.
        const AT: Vec3 = Vec3::new(-9451.0, 42.0, 61.0);
        let unit = app
            .world_mut()
            .spawn(GlobalTransform::from(Transform::from_translation(AT)))
            .id();
        let mut host_pose =
            crate::testing::test_rig_pose(unit, &[Vec3::ZERO, Vec3::new(0.0, 1.4, 0.0)]);
        host_pose.compose();
        app.world_mut().entity_mut(unit).insert(host_pose);

        // Identity inverse bindposes, so a row is `F × model[b]`.
        let ibp = app
            .world_mut()
            .resource_mut::<Assets<SkinnedMeshInverseBindposes>>()
            .add(SkinnedMeshInverseBindposes::from(vec![Mat4::IDENTITY; 2]));
        let skin = {
            let mut palettes = app.world_mut().resource_mut::<RigPalettes>();
            RigSkin::allocate_bones(&mut palettes, 2, ibp).unwrap()
        };
        let slot = skin.slot;
        // The prop's own pose: bone 1 a third of a yard out, standing in for a firearm's muzzle.
        const MUZZLE: Vec3 = Vec3::new(0.33, 0.0, 0.0);
        let prop = app.world_mut().spawn_empty().id();
        let mut prop_pose = crate::testing::test_rig_pose(prop, &[Vec3::ZERO, MUZZLE]);
        prop_pose.compose();
        app.world_mut().entity_mut(prop).insert((
            skin,
            prop_pose,
            RigRider {
                host: unit,
                bone: 1,
                local: Vec3::ZERO,
                slot,
            },
        ));
        app.add_systems(Update, write_rig_riders);
        app.update();

        let (origin, row0) = app
            .world()
            .resource::<RigPalettes>()
            .row_placement(slot, 0)
            .expect("written");
        let (_, row1) = app
            .world()
            .resource::<RigPalettes>()
            .row_placement(slot, 1)
            .expect("written");
        assert_eq!(origin, AT, "still measured from the HOST's frame (1609)");
        assert!(
            row1.distance(row0) > 0.3,
            "row 1 must carry the prop's own pose, not a copy of row 0 ({row0:?} vs {row1:?})"
        );
        // Row 0 is the placement itself: the host bone 1.4 yd up, rig-relative.
        assert!(
            row0.abs_diff_eq(Vec3::new(0.0, 1.4, 0.0), 1.0e-4),
            "row 0 is the attach frame: {row0:?}"
        );
        assert!(
            row1.abs_diff_eq(Vec3::new(0.0, 1.4, 0.0) + MUZZLE, 1.0e-4),
            "row 1 is that frame composed with the prop's bone: {row1:?}"
        );
    }

    /// Scores both routes' second difference against an f64 oracle of the same chain: a bone
    /// swinging ±0.02 rad at Goldshire, where the absolute route adds the 2⁻¹⁰ yd grid of a
    /// ~9481 yd coordinate and the rider's frame only the chain's own curvature.
    #[test]
    fn a_riders_frame_does_not_re_round_an_attachment_at_world_scale() {
        // Bevy-space Goldshire: |z| ≈ 9481.5 puts the ULP at exactly 2⁻¹⁰ yd.
        const HOST: Vec3 = Vec3::new(-76.1, 58.47, 9481.53);
        const ULP: f32 = 1.0 / 1024.0;
        const FRAMES: usize = 240;
        const DT: f64 = 1.0 / 60.0;
        let offset = Vec3::new(0.02, 0.03, 0.04);

        // The bone swings about X, so its tip moves in y and z, and Bevy z is the ~9.5 k axis.
        let theta = |f: usize| 0.02 * (std::f64::consts::TAU * (f as f64 * DT) / 4.0).sin();
        let (mut abs, mut rel, mut oracle) = (Vec::new(), Vec::new(), Vec::new());
        for f in 0..FRAMES {
            let t = theta(f);
            let bone = Affine3A::from_rotation_x(t as f32)
                * Affine3A::from_translation(Vec3::new(0.0, 1.9, 0.0));
            let local = Affine3A::from_translation(offset);
            // The scene-graph route: every product lands in absolute f32 world space.
            let host = Affine3A::from_translation(HOST);
            abs.push(Vec3::from((host * bone * local).translation));
            // The rider's route: the same chain in the host's frame; `rig_origin` carries the rest.
            let basis = Affine3A::from_translation(Vec3::ZERO);
            rel.push(Vec3::from((basis * bone * local).translation));
            // f64 truth for the same chain, measured from the host.
            let (s, c) = (t.sin(), t.cos());
            let tip = DVec3::new(0.0, 1.9 * c, 1.9 * s);
            let off = DVec3::new(offset.x as f64, offset.y as f64, offset.z as f64);
            let rot_off = DVec3::new(off.x, off.y * c - off.z * s, off.y * s + off.z * c);
            oracle.push(tip + rot_off);
        }

        // Frame-to-frame second differences, in yards.
        let d2 = |v: &[Vec3]| {
            (2..v.len())
                .map(|i| (v[i] - 2.0 * v[i - 1] + v[i - 2]).length())
                .fold(0.0f32, f32::max)
        };
        let (abs_d2, rel_d2) = (d2(&abs), d2(&rel));
        let true_d2 = (2..oracle.len())
            .map(|i| (oracle[i] - 2.0 * oracle[i - 1] + oracle[i - 2]).length())
            .fold(0.0f64, f64::max) as f32;

        // The premise: the chain moves along the large axis, or the comparison measures nothing.
        let travel = (1..abs.len())
            .map(|i| (abs[i].z - abs[i - 1].z).abs())
            .fold(0.0f32, f32::max);
        assert!(
            travel > 0.5 * ULP,
            "premise: the chain must move along the ~9.5 k axis (moved {travel} yd/frame)"
        );
        assert!(
            abs_d2 > 0.8 * ULP,
            "premise: the absolute route must spend an f32 ULP of the world coordinate \
             (Δ² = {abs_d2} yd, ULP = {ULP})"
        );

        assert!(
            rel_d2 < 2.0 * true_d2 + 1.0e-6,
            "the rider frame must add nothing to the chain's own curvature \
             (Δ² = {rel_d2} yd vs a true {true_d2} yd)"
        );
        assert!(
            abs_d2 > 20.0 * rel_d2,
            "the two routes must not be comparable: absolute Δ² = {abs_d2} yd, \
             rider Δ² = {rel_d2} yd"
        );

        let rel_err = rel
            .iter()
            .zip(&oracle)
            .map(|(v, o)| (v.as_dvec3() - *o).length())
            .fold(0.0f64, f64::max);
        let abs_err = abs
            .iter()
            .zip(&oracle)
            .map(|(v, o)| ((v.as_dvec3() - DVec3::new(-76.1, 58.47, 9481.53)) - *o).length())
            .fold(0.0f64, f64::max);
        assert!(rel_err < 1.0e-6, "rider frame vs f64 oracle: {rel_err} yd");
        assert!(
            abs_err > 20.0 * rel_err,
            "absolute route vs f64 oracle: {abs_err} yd — expected the world coordinate's ULP"
        );
    }
}
