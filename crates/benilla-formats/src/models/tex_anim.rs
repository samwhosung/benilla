//! The **texture-transform** (UV-animation) bake (decision 0130 phase 3): each M2 batch's texture
//! transform — batch `textureTransformComboIndex` (texUnit `+0x16`) → texAnimLookup (header `0xac`,
//! `0xffff`/out-of-range = none) → the record's **translation** track, baked to a loopable UV-offset
//! loop on the same clocks as the material-alpha bake (gseq wrap / seq-0 band).
//!
//! The translation channel is what the world's shared-material lane consumes: the full 7968-model
//! 1.12 corpus sweep found 281 of 286 transforms translation-keyed, and every rotation/scaling
//! user is a UI/spell-effect/goober model — **zero placed world doodads**. The rotation and
//! scaling channels bake per file sequence slot ([`bake_uv_rot_seqs`] / [`bake_uv_scale_seqs`],
//! decision 2019) for the lanes that own their materials per instance — the UI model tiles, whose
//! cooldown indicator is four quadrant quads turned by their rotation tracks. The baked values are
//! the tracks' raw numbers; the law that composes them is [`uv_transform`], and the shader is its
//! twin.

use benilla_m2::M2Model;

use super::key_anim::{bake_track, KeyAnim, SeqLoops, SeqSlot};

/// One baked UV-offset loop, seconds — see [`KeyAnim`]. The texture-transform translation
/// channel's instantiation: values are the track's raw `(x, y)`.
pub type UvAnim = KeyAnim<[f32; 2]>;

impl KeyAnim<[f32; 2]> {
    /// The UV offset at `elapsed` seconds on the loop clock (`[0, 0]` — the additive identity —
    /// for a defensively-empty loop the bake never emits).
    pub fn sample(&self, elapsed: f32) -> [f32; 2] {
        self.sample_or(elapsed, [0.0, 0.0])
    }
}

/// One baked texture-transform **rotation** loop — the raw quaternion keys, component-lerped
/// (see [`bake_uv_rot_seqs`]).
pub type UvRotAnim = KeyAnim<[f32; 4]>;

impl KeyAnim<[f32; 4]> {
    /// The raw quaternion at `elapsed` seconds on the loop clock (the identity `(0, 0, 0, 1)`
    /// for a defensively-empty loop the bake never emits).
    pub fn sample(&self, elapsed: f32) -> [f32; 4] {
        self.sample_or(elapsed, [0.0, 0.0, 0.0, 1.0])
    }
}

/// `(x, y)` within a hair of zero — the offset that moves nothing.
fn is_zero(v: [f32; 2]) -> bool {
    v[0].abs() < 1e-6 && v[1].abs() < 1e-6
}

/// Bake the batch's UV-offset loop, or `None` when the batch has no texture transform (the
/// overwhelmingly common case), the looked-up record is absent (`0xffff` sentinel falls out of the
/// bounds check), or its translation track never moves the UVs.
///
/// Deliberately **one sequence**, unlike the material-alpha bake next door: a UV loop is consumed
/// through a shared per-MATERIAL registry (`doodad_anim::UvAnimMaterials` — one uniform for every
/// instance of a batch), which has no per-instance sequence to key on. Making it per-sequence needs
/// that registry to become per-instance first; every UV-animating model measured is a scroll that
/// runs the same in every sequence, so nothing observable rests on it (recorded, not assumed).
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

/// Bake the batch's UV-offset loop **for every file sequence slot** — the per-sequence form of
/// [`bake_uv_anim`], and the answer to the question that function's own doc used to settle by
/// measurement ("every UV-animating model measured is a scroll that runs the same in every
/// sequence"). `benilla-extract uvslotscan` re-ran that measurement and it is false: 22 of the 32
/// multi-slot UV batch-channels in the 1.12 corpus have a **dead slot 0** beside a live later slot.
///
/// `None` when no slot animates at all. The caller keeps [`bake_uv_anim`]'s slot-0 loop for the
/// shared-material lane and reaches for this only when [`SeqLoops::uniform`] declines (decision
/// 1408).
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

/// A rotation quaternion within a hair of the identity `(0, 0, 0, 1)`.
fn is_quat_identity(q: [f32; 4]) -> bool {
    q[0].abs() < 1e-6 && q[1].abs() < 1e-6 && q[2].abs() < 1e-6 && (q[3] - 1.0).abs() < 1e-6
}

/// A scale within a hair of `(1, 1)`.
fn is_scale_identity(v: [f32; 2]) -> bool {
    (v[0] - 1.0).abs() < 1e-6 && (v[1] - 1.0).abs() < 1e-6
}

/// Bake the batch's texture-transform **rotation** loop per file sequence slot — the raw
/// quaternion keys, **component-lerped and never normalised**, which is what the reference does
/// (`0x713ea0` lerps each component and `0x7bddb0` consumes the result as is: between two 22.5°
/// keys `|q|` dips to cos 11.25° and the block is a rotation with a slight shrink). `None` when the
/// transform is absent or its rotation never leaves the identity.
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

/// Bake the batch's texture-transform **scaling** loop per file sequence slot (`(x, y)` of the
/// track's vec3). `None` when the transform is absent or its scale never leaves `(1, 1)`.
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

/// **The texture transform's law**, from the file to the texel (the texture-transform loop
/// `0x715f25`):
///
/// > `uv' = R_q((uv + t − p) ⊙ s) + p`, `p = (½, ½)`
///
/// — the translation is added first, the scale is applied about the pivot, then the rotation
/// about the pivot. `R_q` is the standard active rotation of the raw (unnormalised) quaternion;
/// for the z-only quaternions every shipped UI asset authors, `c = 1 − 2z²`, `s = 2zw`, and
/// `+θ` turns counter-clockwise in the `u`-right / `v`-up frame. A quaternion with x/y components
/// would turn the plane out of itself; this reads its z-rotation alone (the shipped population
/// has none). `wow_model.wgsl`'s UV fold is this function's twin: keep the two in step.
pub fn uv_transform(uv: [f32; 2], t: [f32; 2], q: [f32; 4], s: [f32; 2]) -> [f32; 2] {
    let (c, sn) = rotation_2x2(q);
    let dx = (uv[0] + t[0] - 0.5) * s[0];
    let dy = (uv[1] + t[1] - 0.5) * s[1];
    [0.5 + dx * c - dy * sn, 0.5 + dx * sn + dy * c]
}

/// The `(cos, sin)` of a raw quaternion's z-rotation, as the reference's matrix builder yields
/// them — `1 − 2z²`, `2zw`, unnormalised (see [`uv_transform`]).
pub fn rotation_2x2(q: [f32; 4]) -> (f32, f32) {
    let (z, w) = (q[2], q[3]);
    (1.0 - 2.0 * z * z, 2.0 * z * w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_m2::M2Vec3Track;

    /// The law's handedness, worked through an example: `θ = +90°` (`z = w = √½`),
    /// `p = (½, ½)`, no translation, unit scale — `(1, 0.5) ↦ (0.5, 1.0)`, a counter-clockwise
    /// turn in the `u`-right / `v`-up frame; and a `z < 0` key (every key of the cooldown model)
    /// turns the other way.
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
        // Identity in, identity out — the static path's whole population.
        let id = uv_transform([0.2, 0.7], [0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0, 1.0]);
        assert!((id[0] - 0.2).abs() < 1e-6 && (id[1] - 0.7).abs() < 1e-6);
        // A translation is added BEFORE the pivoted rotation: `(uv + t − p)` turns.
        let tr = uv_transform([0.5, 0.5], [0.5, 0.0], [0.0, 0.0, r, r], [1.0, 1.0]);
        assert!(
            (tr[0] - 0.5).abs() < 1e-6 && (tr[1] - 1.0).abs() < 1e-6,
            "{tr:?}"
        );
        // The lerped, unnormalised midpoint between 0° and 45° keys shrinks: |q| < 1 ⇒ the
        // block's scale is below 1 (the same cos 11.25° shrink, at a coarser pair here).
        let (c, sn) = rotation_2x2([
            0.0,
            0.0,
            0.5 * (0.0 + (22.5f32).to_radians().sin()),
            0.5 * (1.0 + (22.5f32).to_radians().cos()),
        ]);
        assert!((c * c + sn * sn).sqrt() < 1.0);
    }

    /// The cooldown indicator's own rotation tracks, straight off the client data: transform 0's
    /// keys turn `0 → −90°` over the first 250 ms of sequence 0 (`z` from 0 to `−√½`) and hold
    /// there to the sequence's end; sequence 1's window is the identity throughout (its keys at
    /// 1167 and 2167 ms are both `(0, 0, 0, 1)`). Skips without client data.
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
        // The lerp between the 0 and 62 ms keys is component-wise on the raw quaternion.
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
        // Slot 0 as the UV lane meets it: the loop a placed doodad's one-time arm plays.
        let slot = seq0.map(|band| SeqSlot {
            index: 0,
            band,
            looping: true,
        });
        super::super::key_anim::bake_track(t, gseq, slot, |v| [v[0], v[1]], is_zero, is_zero)
    }

    /// A keyless or never-moving track vanishes; a constant non-zero offset is kept period-0 (a
    /// static UV shift the vertex data doesn't carry).
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

    /// The fountain shape (gseq clock): a 2-key linear ramp wrapping the global-sequence duration —
    /// the offset accumulates to a full texture period each loop.
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

    /// The real Elwynn waterfall, straight off the client data: both batches carry seq-0-band
    /// translation loops (V-scroll, −1.0 and −2.0 per loop — the 2× foam layer), wired batch →
    /// texAnimLookup → transform. Guards the whole chain: the vec3-track parse (benilla-m2), the
    /// 0xac lookup slot, the combo two-hop, and the band-clock bake. Skips without client data.
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
        // The two layers scroll at different rates (the −1.0 vs −2.0 authoring).
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
