//! The texture-transform (UV animation) bake: a batch's `textureTransformComboIndex` (texUnit
//! `+0x16`) goes through texAnimLookup (header `0xac`, `0xffff` for none) to a transform whose
//! tracks bake on the material-alpha bake's clocks. Translation feeds the world's shared-material
//! lane; rotation and scaling, authored only by UI, spell-effect and gameobject models, bake per
//! file sequence slot for lanes that own a material per instance (the UI cooldown indicator's
//! four quadrants). The values stay raw; [`uv_transform`] composes them.

use benilla_m2::M2Model;

use super::key_anim::{bake_track, KeyAnim, SeqLoops, SeqSlot};

/// One baked UV-offset loop, the translation channel's [`KeyAnim`] of raw `(x, y)`.
pub type UvAnim = KeyAnim<[f32; 2]>;

impl KeyAnim<[f32; 2]> {
    /// The UV offset at `elapsed` seconds; `[0, 0]` for an empty loop.
    pub fn sample(&self, elapsed: f32) -> [f32; 2] {
        self.sample_or(elapsed, [0.0, 0.0])
    }
}

/// One baked rotation loop of raw quaternion keys, component-lerped ([`bake_uv_rot_seqs`]).
pub type UvRotAnim = KeyAnim<[f32; 4]>;

impl KeyAnim<[f32; 4]> {
    /// The raw quaternion at `elapsed` seconds; the identity for an empty loop.
    pub fn sample(&self, elapsed: f32) -> [f32; 4] {
        self.sample_or(elapsed, [0.0, 0.0, 0.0, 1.0])
    }
}

fn is_zero(v: [f32; 2]) -> bool {
    v[0].abs() < 1e-6 && v[1].abs() < 1e-6
}

/// Bake the batch's UV-offset loop for one sequence, or `None` without a transform or when its
/// translation never moves. One loop, because the shared per-material registry it feeds
/// (`UvAnimMaterials`) has no sequence to key on; [`bake_uv_seqs`] covers slots that differ.
pub(super) fn bake_uv_anim(
    model: &M2Model,
    combo_index: u16,
    seq0: Option<SeqSlot>,
) -> Option<UvAnim> {
    let ti = *model.texture_transform_lookup.get(combo_index as usize)?;
    let t = model.texture_transforms.get(ti as usize)?;
    bake_track(
        &t.translation,
        &model.global_sequences,
        seq0,
        |v| [v[0], v[1]],
        is_zero,
        is_zero,
    )
}

/// [`bake_uv_anim`] per file sequence slot, `None` when no slot animates. The caller keeps the
/// shared lane's loop and takes this only when [`SeqLoops::uniform`] declines.
pub(super) fn bake_uv_seqs(
    model: &M2Model,
    combo_index: u16,
    slots: &[SeqSlot],
) -> Option<SeqLoops<[f32; 2]>> {
    let ti = *model.texture_transform_lookup.get(combo_index as usize)?;
    let t = model.texture_transforms.get(ti as usize)?;
    SeqLoops::new(
        slots
            .iter()
            .map(|&slot| {
                bake_track(
                    &t.translation,
                    &model.global_sequences,
                    Some(slot),
                    |v| [v[0], v[1]],
                    is_zero,
                    is_zero,
                )
            })
            .collect(),
    )
}

fn is_quat_identity(q: [f32; 4]) -> bool {
    q[0].abs() < 1e-6 && q[1].abs() < 1e-6 && q[2].abs() < 1e-6 && (q[3] - 1.0).abs() < 1e-6
}

fn is_scale_identity(v: [f32; 2]) -> bool {
    (v[0] - 1.0).abs() < 1e-6 && (v[1] - 1.0).abs() < 1e-6
}

/// Bake the batch's rotation loop per file sequence slot: the raw quaternion keys lerped per
/// component and never normalised, as the reference does (`0x713ea0` lerps, `0x7bddb0` uses the
/// result as is, so between two 22.5° keys `|q|` dips to cos 11.25° and the block shrinks a
/// little). `None` when the rotation never leaves the identity.
pub(super) fn bake_uv_rot_seqs(
    model: &M2Model,
    combo_index: u16,
    slots: &[SeqSlot],
) -> Option<SeqLoops<[f32; 4]>> {
    let ti = *model.texture_transform_lookup.get(combo_index as usize)?;
    let t = model.texture_transforms.get(ti as usize)?;
    SeqLoops::new(
        slots
            .iter()
            .map(|&slot| {
                bake_track(
                    &t.rotation,
                    &model.global_sequences,
                    Some(slot),
                    |q| q,
                    is_quat_identity,
                    is_quat_identity,
                )
            })
            .collect(),
    )
}

/// Bake the batch's scaling loop (the vec3's `(x, y)`) per file sequence slot; `None` when the
/// scale never leaves `(1, 1)`.
pub(super) fn bake_uv_scale_seqs(
    model: &M2Model,
    combo_index: u16,
    slots: &[SeqSlot],
) -> Option<SeqLoops<[f32; 2]>> {
    let ti = *model.texture_transform_lookup.get(combo_index as usize)?;
    let t = model.texture_transforms.get(ti as usize)?;
    SeqLoops::new(
        slots
            .iter()
            .map(|&slot| {
                bake_track(
                    &t.scaling,
                    &model.global_sequences,
                    Some(slot),
                    |v| [v[0], v[1]],
                    is_scale_identity,
                    is_scale_identity,
                )
            })
            .collect(),
    )
}

/// The texture transform, `uv' = R_q((uv + t − p) ⊙ s) + p` with `p = (½, ½)` (`0x715f25`):
/// translate, scale about the pivot, then rotate about it. `R_q`, the raw quaternion's rotation,
/// is read here as its z-rotation alone, `c = 1 − 2z²`, `s = 2zw`, `+θ` counter-clockwise with `u`
/// right and `v` up: no shipped rotation has x or y components. `wow_model.wgsl`'s UV fold must
/// stay in step.
pub fn uv_transform(uv: [f32; 2], t: [f32; 2], q: [f32; 4], s: [f32; 2]) -> [f32; 2] {
    let (c, sn) = rotation_2x2(q);
    let dx = (uv[0] + t[0] - 0.5) * s[0];
    let dy = (uv[1] + t[1] - 0.5) * s[1];
    [0.5 + dx * c - dy * sn, 0.5 + dx * sn + dy * c]
}

/// The `(cos, sin)` of a raw quaternion's z-rotation as the reference builds them, unnormalised.
pub fn rotation_2x2(q: [f32; 4]) -> (f32, f32) {
    let (z, w) = (q[2], q[3]);
    (1.0 - 2.0 * z * z, 2.0 * z * w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_m2::M2Vec3Track;

    /// `θ = +90°` (`z = w = √½`) takes `(1, 0.5)` to `(0.5, 1.0)`, counter-clockwise; a `z < 0`
    /// key, as in every key of the cooldown model, turns the other way.
    #[test]
    fn the_uv_law_turns_counter_clockwise_for_a_positive_z() {
        let r = std::f32::consts::FRAC_1_SQRT_2;
        let got = uv_transform([1.0, 0.5], [0.0, 0.0], [0.0, 0.0, r, r], [1.0, 1.0]);
        assert!(
            (got[0] - 0.5).abs() < 1e-6 && (got[1] - 1.0).abs() < 1e-6,
            "{got:?}"
        );
        let cw = uv_transform([1.0, 0.5], [0.0, 0.0], [0.0, 0.0, -r, r], [1.0, 1.0]);
        assert!(
            (cw[0] - 0.5).abs() < 1e-6 && (cw[1] - 0.0).abs() < 1e-6,
            "{cw:?}"
        );
        // Identity in, identity out.
        let id = uv_transform([0.2, 0.7], [0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0, 1.0]);
        assert!((id[0] - 0.2).abs() < 1e-6 && (id[1] - 0.7).abs() < 1e-6);
        // The translation is added before the pivoted rotation: `(uv + t − p)` turns.
        let tr = uv_transform([0.5, 0.5], [0.5, 0.0], [0.0, 0.0, r, r], [1.0, 1.0]);
        assert!(
            (tr[0] - 0.5).abs() < 1e-6 && (tr[1] - 1.0).abs() < 1e-6,
            "{tr:?}"
        );
        // The unnormalised midpoint of 0° and 45° keys has `|q| < 1`, so the block shrinks.
        let (c, sn) = rotation_2x2([
            0.0,
            0.0,
            0.5 * (0.0 + (22.5f32).to_radians().sin()),
            0.5 * (1.0 + (22.5f32).to_radians().cos()),
        ]);
        assert!((c * c + sn * sn).sqrt() < 1.0);
    }

    /// The cooldown indicator's transform 0 turns 0 to −90° over sequence 0's first 250 ms (`z`
    /// from 0 to `−√½`) and holds; sequence 1's keys at 1167 and 2167 ms are both the identity.
    #[test]
    fn the_cooldown_indicators_rotation_bakes_per_sequence() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file(r"Interface\Cooldown\UI-Cooldown-Indicator.m2")
            .expect("read the cooldown indicator");
        let subs = super::super::parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
        let rot = subs[0]
            .uv_rot_seq
            .as_ref()
            .expect("the top-right quadrant's rotation loop");
        let seq0 = rot.seq(Some(0)).expect("sequence 0 keys it");
        assert!(!seq0.wrap, "sequence 0 clamps (flags 0x1)");
        let r = std::f32::consts::FRAC_1_SQRT_2;
        let at = |t: f32| seq0.sample(t);
        assert!(at(0.0)[2].abs() < 1e-5, "{:?}", at(0.0));
        assert!(
            (at(0.25)[2] + r).abs() < 1e-3,
            "−90° at 250 ms: {:?}",
            at(0.25)
        );
        assert!(
            (at(0.9)[2] + r).abs() < 1e-3,
            "held to the end: {:?}",
            at(0.9)
        );
        // Between the 0 and 62 ms keys, a per-component lerp of the raw quaternion.
        let mid = at(0.031);
        assert!(mid[2] < 0.0 && mid[2] > -r);
        assert!(subs[0].uv_scale_seq.is_none(), "no scale track");
        assert!(subs[4].uv_rot_seq.is_none(), "the star has no transform");
    }

    fn track(gseq: u16, interp: u16, keys: &[(u32, [f32; 3])]) -> M2Vec3Track {
        M2Vec3Track {
            interp,
            gseq,
            ranges: Vec::new(),
            keys: keys.to_vec(),
        }
    }

    fn bake(t: &M2Vec3Track, gseq: &[u32], seq0: Option<(u32, u32)>) -> Option<UvAnim> {
        // Slot 0, looping, as the UV lane bakes it.
        let slot = seq0.map(|band| SeqSlot {
            index: 0,
            band,
            looping: true,
        });
        super::super::key_anim::bake_track(t, gseq, slot, |v| [v[0], v[1]], is_zero, is_zero)
    }

    /// A constant non-zero offset is a static UV shift the vertex data does not carry.
    #[test]
    fn bake_drops_the_identity_and_keeps_a_static_shift() {
        assert_eq!(bake(&track(0xffff, 1, &[]), &[], None), None);
        assert_eq!(
            bake(
                &track(0xffff, 1, &[(0, [0.0; 3]), (500, [0.0; 3])]),
                &[],
                None
            ),
            None
        );
        let shift = bake(&track(0xffff, 1, &[(0, [0.25, 0.5, 9.0])]), &[], None).unwrap();
        assert_eq!(shift.period, 0.0);
        assert_eq!(shift.sample(77.0), [0.25, 0.5]); // z discarded
    }

    /// The fountain shape: a 2-key ramp over the global-sequence duration, one texture per loop.
    #[test]
    fn gseq_scroll_wraps_and_lerps() {
        let a = bake(
            &track(3, 1, &[(0, [0.0; 3]), (1333, [0.0, -1.0, 0.0])]),
            &[1, 1, 1, 1333],
            None,
        )
        .unwrap();
        assert!((a.period - 1.333).abs() < 1e-6);
        let mid = a.sample(1.333 / 2.0);
        assert!((mid[1] + 0.5).abs() < 1e-3, "half-loop offset ≈ -0.5 V");
        assert!((a.sample(1.333 + 0.1)[1] - a.sample(0.1)[1]).abs() < 1e-4);
    }

    /// The Elwynn waterfall's two batches scroll V by −1.0 and −2.0 per loop (the foam layer).
    #[test]
    fn elwynn_waterfall_bakes_two_v_scroll_loops() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file(
                "World\\Azeroth\\Elwynn\\PassiveDoodads\\Waterfall\\ElwynnTallWaterfall01.m2",
            )
            .expect("read ElwynnTallWaterfall01.m2");
        let subs = super::super::parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
        let anims: Vec<&UvAnim> = subs.iter().filter_map(|s| s.uv_anim.as_ref()).collect();
        assert_eq!(anims.len(), 2, "both waterfall batches carry UV loops");
        for a in &anims {
            assert!(a.period > 1.0, "a real loop, not a constant");
            let (v0, v1) = (a.sample(0.0), a.sample(a.period * 0.5));
            assert!(
                (v1[1] - v0[1]).abs() > 0.01,
                "the V offset moves over the loop ({v0:?} vs {v1:?})"
            );
        }
        let (e0, e1) = (
            anims[0].keys.last().unwrap().1[1],
            anims[1].keys.last().unwrap().1[1],
        );
        assert!(
            (e0 - e1).abs() > 0.5,
            "distinct layer rates survive the bake ({e0} vs {e1})"
        );
    }
}
