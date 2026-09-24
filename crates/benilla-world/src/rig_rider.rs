//! **The rider lane**: an attached model — a helm, a pauldron, a held weapon, a
//! shield — placed by a palette frame measured in its WEARER's rig frame, so its position is
//! never an absolute f32 world coordinate that gets recomputed every frame.
//!
//! ## Why an attached model needs its own lane at all
//!
//! Decision 0974 moved the model lane camera-relative: a rig's palette rows are composed with the
//! root's translation zeroed, the row's world position rides a per-slot `rig_origin`, and the
//! vertex stage adds it back as `frame·v + (origin − camera)`. A standing unit's origin is
//! **constant**, so its body is exact — the one f32 rounding left in it never moves.
//!
//! Its attachments were not. They are ordinary scene-graph entities: the attach anchor's
//! `GlobalTransform` is `joints_root_world × bone_local`, a ~1-yard quantity added to a ~9500-yard
//! one, recomputed every frame. WoW's coordinates run to ±17066 yd and an f32 in `[8192, 16384)`
//! has an ULP of exactly `2⁻¹⁰ yd = 0.9766 mm`, so the sum lands on a 0.98 mm grid **on the large
//! axis** while the bone under it moves smoothly across it. The rendered helm therefore holds
//! still for a few frames and then snaps one whole ULP sideways, for ever, while the body it sits
//! on moves exactly. Measured on the Goldshire Stormwind Guard (`WOW_JITTER`, 2698 frames): the
//! anchors' per-frame Δ along that axis was **exactly 0 or exactly one ULP and nothing between**
//! (1835 / 839 frames), their second difference was exactly one ULP on 53 % of frames, and the
//! body bones' was 0.0094 mm — a factor of 104. At the reported close-up that is 0.83 px of hop
//! every other frame on the helm, both pauldrons, the sword and the shield. 0974 named this
//! residual and did not take it; this is taking it.
//!
//! ## The mechanism
//!
//! An attached model rests at **bind pose** — the item lane arms no `AnimationPlayer` and no
//! global-sequence drive, which is why it draws a static mesh today. At bind pose every palette
//! row of it is the *same* matrix: a row is `world_from_joint × inverse_bindpose`, and
//! `world_from_joint = F × bind_model_j` with `inverse_bindpose_j = bind_model_j⁻¹`, so every row
//! collapses to the placement `F` itself. So the whole model needs **one frame**, written into
//! each of its rows, and the picture it draws is identical to the static mesh's — the change is
//! precision and nothing else.
//!
//! And `F` is available exactly: `F = host_basis × pose.model[bone] × T(offset)`, where
//! `host_basis` is the host's own frame with its translation zeroed — the same rig-relative basis
//! 0974 composes the body in. Every factor is rig-sized, so `F` costs ~1e-7 yd, and the slot's
//! `rig_origin` carries the host's world position through to the vertex stage, where it is added
//! camera-relative. Nothing on the path is ever a big-plus-small sum.
//!
//! **The entity keeps its absolute `GlobalTransform`.** That is the whole reason this is a
//! *palette* lane and not a rebase of the scene graph: collision, picking, the item glow, the
//! bowstring and the booth mirrors all read an attached model's world position and all keep
//! reading exactly what they read before. Only the rendered vertices change route.
//!
//! **Named residuals** (they keep the absolute route, and the staircase with it): billboard cards
//! (spawned at world root by decision 0153), item glow instances (children of the item root with
//! their own attachment offsets), the engine-drawn bowstring/fishing line, and the seven
//! `welds_billboard` displays whose 0841/0854 joint rig is camera-replaced per frame and so is not
//! a bind-pose rigid frame.

use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

use crate::rig_anim::RigPose;
use crate::rig_palette::{rebase_origin, RigPalettes, RigSkin};

/// An attached model whose palette slot is placed from a bone of `host`'s rig, in `host`'s own rig
/// frame. Lives on the attached model's root, beside the [`crate::rig_palette::RigSkin`]
/// that owns the slot — so the slot is freed, and the rider forgotten, on the same despawn edge.
#[derive(Component, Clone, Copy)]
pub struct RigRider {
    /// The rig this model hangs off.
    pub host: Entity,
    /// The host bone it hangs from. Attachment points never sit on a camera-faced bone (the same
    /// fact `RigPose::posed_point` rests on), so the composed `pose.model` frame — which carries
    /// the `flags & 0x7` arm but not the world pass's billboard replacement — is the right input.
    pub bone: u16,
    /// The attachment point's own offset from that bone, model space.
    pub local: Vec3,
    /// The palette slot the frame is written into; every row in it takes the same value.
    pub slot: u16,
}

/// Write every rider's frame. Ordered with the palette's own writers (see
/// [`crate::rig_palette::plugin`]): after propagation and the billboard joint pass, so the host's
/// `GlobalTransform` and composed pose are both this frame's, and before the publish.
///
/// A host that is missing, has no pose, or does not carry the named bone leaves the rider's rows
/// alone — they hold the last frame written, which is what a torn-down joint gets in the entity
/// lane too. [`RigPalettes::write_rider`] is idempotent, so a standing unit's riders cost one
/// compare each and never reach the upload.
///
/// **A rider that carries a pose of its OWN is posed, not repeated**. 1609's
/// "every row is the placement" collapse is a consequence of bind pose, and the ranged weapon prop
/// does not rest at bind pose: the reference re-arms `[CGUnit+0xd24]`'s own M2 instance to
/// BowPull(160) at `$BWP` and to BowRelease(161) / Stand(0) at `$BWR`, so its bones move. Such a
/// rider's row `b` is `F × own.model[b] × ibp[b]` — the same placement `F`, composed with the
/// prop's own model-space pose, every factor still rig-sized, so the precision property this whole
/// lane exists for is untouched. The population is one prop per unit holding a DRAWN animated
/// ranged weapon (46 of the 571 shipped weapon models author 160/161), which is why the posed arm
/// re-writes unconditionally where the rigid arm compares first.
pub(crate) fn write_rig_riders(
    riders: Query<(Entity, &RigRider)>,
    poses: Query<&RigPose>,
    skins: Query<&RigSkin>,
    frames: Query<&GlobalTransform>,
    ibps: Res<Assets<SkinnedMeshInverseBindposes>>,
    // The posed arm's row scratch, reused across frames and riders — a posed prop is re-derived
    // every frame and a fresh `Vec` per rider per frame is pure churn.
    mut scratch: Local<Vec<GlobalTransform>>,
    mut palettes: ResMut<RigPalettes>,
) {
    for (entity, rider) in &riders {
        let Ok(pose) = poses.get(rider.host) else {
            continue;
        };
        // **`joints_root`, never the host entity.** `pose.model` is model space *relative to the
        // rig's own frame*, and that frame is only the unit itself in the ordinary case: a mounted
        // rider's is its SEAT anchor and a terrain-conformed model's is its conform node
        // (`entities::mount`, `entities::attach`). Composing through the unit's own transform
        // instead would place a mounted character's helm and weapon by the gap between its feet
        // and its saddle. This is the same frame `finalize_rig_worlds` writes the body's rows
        // from and the same one `RigPose::posed_point` demands, so the body and everything hanging
        // off it stay in ONE frame — which is the property the whole lane exists to keep.
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
        // The prop's OWN pose, when it has one: compose each of its model-space bone frames onto
        // the placement and let the ordinary world write lay the rows down. `write_rig_worlds`
        // takes it from there — same `frame × ibp` row math, same origin word, same dirty push.
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

    /// **The rig's frame is `joints_root`'s, not the host entity's** — the one distinction that
    /// separates a helm sitting on a head from one sitting a saddle's height below it.
    ///
    /// `RigPose::model` is model space measured from the rig's own frame, and that frame is the
    /// unit itself only in the ordinary case: `entities::mount` re-points `joints_root` at the
    /// SEAT anchor when a character mounts, and `entities::attach` at the conform node for a
    /// terrain-tilted model. Both leave the unit's own `GlobalTransform` where it always was, so
    /// composing a rider through it is wrong by exactly the seat offset — and wrong only while
    /// mounted, which is the shape of bug that ships.
    #[test]
    fn a_rider_composes_through_the_rigs_own_frame_not_the_units() {
        let mut app = App::new();
        app.init_resource::<RigPalettes>();
        // The posed arm's asset lookup; `AssetPlugin` provides it in the app.
        app.init_resource::<Assets<SkinnedMeshInverseBindposes>>();
        // The seat: a yard and a half up and turned 90° — a mount's saddle, exaggerated so a
        // composition through the wrong frame cannot land near the right answer by luck.
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
        // The failure this forbids: composing through the unit's own transform instead.
        let unit_g = *app.world().entity(unit).get::<GlobalTransform>().unwrap();
        let wrong = unit_g.affine()
            * Affine3A::from_translation(Vec3::new(0.0, 0.4, 0.0))
            * Affine3A::from_translation(local);
        assert!(
            placed.distance(Vec3::from(wrong.translation)) > 1.0,
            "the two frames must be far apart, or this test proves nothing"
        );
    }

    /// **A rider that poses writes DIFFERENT rows** — the property that makes the
    /// gun's bolt recoil and the bow's limbs bend without giving either the absolute-frame ULP
    /// staircase 1609 removed.
    ///
    /// The subject is the flexing prop's shape: two bones, the second displaced by its own pose,
    /// hanging off a host bone. Bind-pose riders repeat one frame into every row, so the test that
    /// this lane has really changed is that row 1 is **not** row 0 — and that row 1 lands where the
    /// composition `F × model[1]` says, in the host's frame with the host's origin word.
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

        // Identity inverse bindposes, so a row reduces to `F × model[b]` and the arithmetic in the
        // assertion is the mechanism rather than a second implementation of it.
        let ibp = app
            .world_mut()
            .resource_mut::<Assets<SkinnedMeshInverseBindposes>>()
            .add(SkinnedMeshInverseBindposes::from(vec![Mat4::IDENTITY; 2]));
        let skin = {
            let mut palettes = app.world_mut().resource_mut::<RigPalettes>();
            RigSkin::allocate_bones(&mut palettes, 2, ibp).unwrap()
        };
        let slot = skin.slot;
        // The prop's OWN pose: bone 1 a third of a yard down its length — stand-in for the muzzle
        // bone a firearm's BowRelease swings.
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

    /// **The pin**, in the shape decision 0974 set for the body: score both
    /// routes' *rendered motion* against an f64 oracle of the identical chain, at Goldshire, with
    /// real glam f32 arithmetic — not a model of it.
    ///
    /// The subject is one attachment on a slow idle: a bone 1.9 yd up the model swinging ±0.02 rad
    /// with a 4 s period, so it travels about a millimetre a frame — the regime the report came
    /// from, and the one where an f32 ULP is a large fraction of the real motion.
    ///
    /// What separates the two routes is the SECOND difference. The chain's own curvature is
    /// `A·ω²·dt²`, tens of microns here; the absolute route adds the ULP grid of a ~9481-yard
    /// coordinate (`2⁻¹⁰ yd = 0.977 mm`) on top, because the rendered position holds still for
    /// several frames and then hops one whole grid step. That is the staircase the director saw on
    /// the helm, and it is what this test forbids coming back.
    ///
    /// The premise is asserted, not assumed (0974's own caution, which cost it a first version
    /// that measured nothing): if a future edit leaves the chain moving along a *small* world
    /// axis, the absolute route stops spending an ULP and the comparison silently proves nothing.
    #[test]
    fn a_riders_frame_does_not_re_round_an_attachment_at_world_scale() {
        // Bevy-space Goldshire, from the report: |z| ≈ 9481.5 puts the ULP at exactly 2⁻¹⁰ yd.
        const HOST: Vec3 = Vec3::new(-76.1, 58.47, 9481.53);
        const ULP: f32 = 1.0 / 1024.0;
        const FRAMES: usize = 240;
        const DT: f64 = 1.0 / 60.0;
        // The M2 attachment point's own offset from the bone — an ordinary rig-sized quantity.
        let offset = Vec3::new(0.02, 0.03, 0.04);

        // The bone swings about X, so its tip travels in y/z — and bevy z IS the ~9.5 k axis here.
        let theta = |f: usize| 0.02 * (std::f64::consts::TAU * (f as f64 * DT) / 4.0).sin();
        let (mut abs, mut rel, mut oracle) = (Vec::new(), Vec::new(), Vec::new());
        for f in 0..FRAMES {
            let t = theta(f);
            let bone = Affine3A::from_rotation_x(t as f32)
                * Affine3A::from_translation(Vec3::new(0.0, 1.9, 0.0));
            let local = Affine3A::from_translation(offset);
            // The route the scene graph takes: the host's world affine × the bone × the offset,
            // every product landing back in absolute f32 world space.
            let host = Affine3A::from_translation(HOST);
            abs.push(Vec3::from((host * bone * local).translation));
            // The rider's route: the same chain in the host's own frame (translation zeroed). The
            // world position rides the slot's `rig_origin` and is added camera-relative, so it
            // contributes nothing to the MOTION measured here.
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

        // The premise: this chain really does exercise the large world axis, so the absolute route
        // really is spending a whole ULP. Without this, a chain that drifted onto a small axis
        // would make the comparison below pass while measuring nothing.
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

        // The verdict: the rider's frame carries the chain's own curvature and nothing else, while
        // the absolute route carries a whole grid step on top of it — two orders apart.
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

        // And the rider's frame agrees with f64 truth at rig scale, where the absolute route
        // cannot: its answer is a ~9481-yard number and its error is that number's ULP.
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
