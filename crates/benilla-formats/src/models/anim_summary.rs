//! Per-model **animation-channel summary** — how much of an M2 actually animates, across every
//! channel family the renderer might ever drive (bone sequences, global-sequence bones, transparency/
//! color tracks, texture transforms, particle/ribbon emitters). Split out of [`super::anim`] as its
//! own concern: the rest of that module parses the skeleton + a *playing* sequence's keyframes for the
//! render/skin path; this one is a read-only diagnostic over the *whole* model, built to ground the
//! doodad-animation scope/perf decision (`benilla-extract m2anim`/`doodadscan`) — most placed doodads
//! (trees, rocks, fences, wall props) carry zero animated channels and can render as static meshes
//! forever, and this is the instrument that counts how many actually don't, and by which mechanism.

use std::io::Cursor;

use anyhow::{Context, Result};
use benilla_m2::parse_m2;

use crate::Chain;

use super::{le_u32, model_path, parse_m2_animations, parse_m2_global_sequence_bones};

/// Raw header read: the model's **texture-transform** (UV-animation) array count — MD20 `0x74`
/// (count) / `0x78` (offset). It IS the textureTransform array (the header walk `0x71cdf0`; runtime
/// 0x98) = 3 M2Tracks translation(vec3) / rotation(quat) / scaling(vec3) — a UV-space TRS animated
/// over time. `benilla-m2` doesn't parse the track contents (no consumer yet), so this reads only
/// the count: whether the model authors any UV animation at all.
pub fn m2_texture_transform_count(b: &[u8]) -> usize {
    if b.len() < 0x78 || &b[0..4] != b"MD20" {
        return 0;
    }
    le_u32(b, 0x74) as usize
}

/// Raw header read: the model's **ribbon emitter** array count — MD20 `0x134` (count) / `0x138`
/// (offset), file stride `0xdc` (the header walk `0x71cdf0`, and the loader's own copy loop,
/// `0x70ebd0` `base+=0xdc @ ribbon 0x138`; runtime `CRibbonEmitter`). Ribbon records aren't parsed
/// here (deferred — the most version-variant MD20 record); the count alone says whether a model
/// authors any trail effect (weapon glow trails, banner streamers).
pub fn m2_ribbon_emitter_count(b: &[u8]) -> usize {
    if b.len() < 0x138 || &b[0..4] != b"MD20" {
        return 0;
    }
    le_u32(b, 0x134) as usize
}

/// One particle emitter's **bone linkage**: which bone hosts it, and whether that bone's *chain*
/// (the bone itself or any ancestor — bone motion composes down the hierarchy) actually animates.
/// The join that grounds emitter bone-follow (0130 phase 4): an emitter on a static chain sits at
/// the rest pose forever, so only `chain_seq0 || chain_gseq` emitters can visibly move.
#[derive(Debug, Clone, Copy)]
pub struct EmitterBoneLink {
    pub bone: u16,
    /// The emitter's raw M2 flag word (`+0x04`) — carried for corpus inspection (which simulation-
    /// space / render modes placed content actually authors), semantics live with the consumer.
    pub flags: u32,
    /// A bone in the chain carries a >1-key T/R/S track in sequence 0's band.
    pub chain_seq0: bool,
    /// A bone in the chain carries a global-sequence channel.
    pub chain_gseq: bool,
}

impl EmitterBoneLink {
    /// The emitter's host bone chain animates at all — its emission point moves off the rest pose.
    pub fn chain_animated(&self) -> bool {
        self.chain_seq0 || self.chain_gseq
    }
}

/// Per-model animation-channel summary (see the module doc).
#[derive(Debug, Clone)]
pub struct M2AnimSummary {
    /// Sequence count (file order — record count in the `animations` M2Array).
    pub sequence_count: usize,
    /// Sequence **0** (file order — the model's first-authored sequence, NOT necessarily
    /// `AnimationData.dbc` id 0/Stand) has at least one bone with a >1-key T/R/S track inside its own
    /// time band: the model visibly moves when this sequence plays/loops.
    pub seq0_has_bone_motion: bool,
    /// How many distinct bones carry a >1-key track (any of T/R/S) in sequence 0's band.
    pub seq0_animated_bone_count: usize,
    /// How many sequences share sequence 0's `anim_id` — its **variation chain** length (file-order
    /// contiguous, retail exporters). The real client's effective load arm rolls `variationIdx = −1`
    /// (a frequency-weighted pick over this chain, `0x695100`), so a count > 1
    /// means instances of this model should NOT all play the same first-sequence variation.
    pub seq0_variation_count: usize,
    /// Every bone's **global-sequence** channel: `(bone, "T"/"R"/"S", period_ms)` — a free-running
    /// loop independent of the playing sequence (see [`super::GlobalSeqBone`]); e.g. the character
    /// eye-blink.
    pub global_seq_channels: Vec<(u16, &'static str, u32)>,
    /// `M2Color` **alpha** tracks: `(total records, records with >1 key)`. A constant (≤1 key) track
    /// never changes, so only the second number reflects real animation.
    pub color_alpha_tracks: (usize, usize),
    /// `M2Color` **RGB** tracks: `(total records, records with >1 key)`.
    pub color_rgb_tracks: (usize, usize),
    /// `M2TextureWeight` transparency tracks: `(total records, records with >1 key)`.
    pub transparency_tracks: (usize, usize),
    /// Raw header count of the **texture-transform** (UV animation) array (see
    /// [`m2_texture_transform_count`]) — presence-only, the track contents aren't parsed.
    pub texture_transform_count: usize,
    /// Particle emitter count ([`crate::parse_m2_particle_emitters`]).
    pub particle_emitter_count: usize,
    /// Each particle emitter's host bone + whether its chain animates (same order as the emitter
    /// records; `len() == particle_emitter_count`). See [`EmitterBoneLink`].
    pub emitter_bones: Vec<EmitterBoneLink>,
    /// Raw header count of the **ribbon emitter** array (see [`m2_ribbon_emitter_count`]) —
    /// presence-only, the records aren't parsed.
    pub ribbon_emitter_count: usize,
}

impl M2AnimSummary {
    /// `true` iff **no** channel family animates anything: no seq-0 bone motion, no global-sequence
    /// bones, no animated transparency/color, and no texture transforms/particles/ribbons authored at
    /// all. Such a model can render as a permanently-static mesh with zero animation cost.
    pub fn is_fully_static(&self) -> bool {
        !self.seq0_has_bone_motion
            && self.global_seq_channels.is_empty()
            && self.transparency_tracks.1 == 0
            && self.color_rgb_tracks.1 == 0
            && self.color_alpha_tracks.1 == 0
            && self.texture_transform_count == 0
            && self.particle_emitter_count == 0
            && self.ribbon_emitter_count == 0
    }
}

/// Parse an M2's animation-channel summary (see [`M2AnimSummary`]) from raw bytes.
pub fn parse_m2_animation_summary(bytes: &[u8]) -> Result<M2AnimSummary> {
    let format =
        parse_m2(&mut Cursor::new(bytes)).map_err(|e| anyhow::anyhow!("parsing M2: {e}"))?;
    let model = format.model();

    let seqs = parse_m2_animations(bytes);
    let sequence_count = seqs.len();
    let seq0_variation_count = seqs
        .first()
        .map(|s0| seqs.iter().filter(|s| s.anim_id == s0.anim_id).count())
        .unwrap_or(0);
    let seq0_animated_bone_count = seqs
        .first()
        .map(|seq0| {
            seq0.bones
                .iter()
                .filter(|b| b.translation.len() > 1 || b.rotation.len() > 1 || b.scale.len() > 1)
                .count()
        })
        .unwrap_or(0);

    let mut global_seq_channels = Vec::new();
    for b in parse_m2_global_sequence_bones(bytes) {
        if let Some(t) = &b.translation {
            global_seq_channels.push((b.bone, "T", t.period_ms));
        }
        if let Some(r) = &b.rotation {
            global_seq_channels.push((b.bone, "R", r.period_ms));
        }
        if let Some(s) = &b.scale {
            global_seq_channels.push((b.bone, "S", s.period_ms));
        }
    }

    // The emitter → bone-chain join (0130 phase 4): which bones move (seq0 band / global
    // sequence), then walk each emitter's bone up its parent chain — motion composes down the
    // hierarchy, so an emitter on a static child of a swinging parent still moves.
    let seq0_bones: std::collections::HashSet<u16> = seqs
        .first()
        .map(|seq0| {
            seq0.bones
                .iter()
                .filter(|b| b.translation.len() > 1 || b.rotation.len() > 1 || b.scale.len() > 1)
                .map(|b| b.bone)
                .collect()
        })
        .unwrap_or_default();
    let gseq_bones: std::collections::HashSet<u16> =
        global_seq_channels.iter().map(|&(b, _, _)| b).collect();
    let skeleton = crate::parse_m2_skeleton(bytes)?;
    let chain_flags = |bone: u16| -> (bool, bool) {
        let (mut seq0, mut gseq) = (false, false);
        let mut b = bone;
        // Hop guard: a malformed parent loop can't spin us — a real chain is ≤ bone count deep.
        for _ in 0..=skeleton.bones.len() {
            let Some(rec) = skeleton.bones.get(b as usize) else {
                break; // out-of-range host bone (or -1 parent cast) — chain ends
            };
            seq0 |= seq0_bones.contains(&b);
            gseq |= gseq_bones.contains(&b);
            if rec.parent < 0 {
                break;
            }
            b = rec.parent as u16;
        }
        (seq0, gseq)
    };
    let emitter_bones: Vec<EmitterBoneLink> = crate::parse_m2_particle_emitters(bytes)?
        .iter()
        .map(|e| {
            let (chain_seq0, chain_gseq) = chain_flags(e.bone);
            EmitterBoneLink {
                bone: e.bone,
                flags: e.flags,
                chain_seq0,
                chain_gseq,
            }
        })
        .collect();

    // A track is "animated" iff it is genuinely time-varying — multi-key AND not all-equal (an
    // all-equal multi-key track is a constant the renderer folds statically).
    let count_animated = |tracks: &[benilla_m2::M2ScalarTrack]| -> (usize, usize) {
        (
            tracks.len(),
            tracks
                .iter()
                .filter(|t| t.keys.len() > 1 && t.constant().is_none())
                .count(),
        )
    };

    Ok(M2AnimSummary {
        sequence_count,
        seq0_has_bone_motion: seq0_animated_bone_count > 0,
        seq0_animated_bone_count,
        seq0_variation_count,
        global_seq_channels,
        color_alpha_tracks: count_animated(&model.color_alpha_tracks),
        transparency_tracks: count_animated(&model.transparency_tracks),
        color_rgb_tracks: (
            model.color_rgb_tracks.len(),
            model
                .color_rgb_tracks
                .iter()
                .filter(|t| t.keys.len() > 1)
                .count(),
        ),
        texture_transform_count: m2_texture_transform_count(bytes),
        particle_emitter_count: emitter_bones.len(),
        emitter_bones,
        ribbon_emitter_count: m2_ribbon_emitter_count(bytes),
    })
}

/// Read an M2's animation-channel summary from the chain (see [`M2AnimSummary`]). Uses the same path
/// normalisation as [`load_m2_mesh`](super::load_m2_mesh) so `.mdx`/`.mdl` doodad paths (MDDF/MODD)
/// resolve identically.
pub fn load_m2_animation_summary(chain: &mut Chain, raw_path: &str) -> Result<M2AnimSummary> {
    let path = model_path(raw_path);
    let bytes = chain
        .read_file(&path)
        .with_context(|| format!("reading M2 {path}"))?;
    parse_m2_animation_summary(&bytes)
        .with_context(|| format!("parsing M2 animation summary {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal MD20 header (0x138 bytes — through the ribbon array descriptor) with the
    /// texture-transform count at `0x74` and the ribbon count at `0x134` — the two raw header reads
    /// [`m2_texture_transform_count`]/[`m2_ribbon_emitter_count`] guard against an offset regression.
    fn header_with_counts(tex_transform_count: u32, ribbon_count: u32) -> Vec<u8> {
        let mut b = vec![0u8; 0x138];
        b[0..4].copy_from_slice(b"MD20");
        b[4..8].copy_from_slice(&256u32.to_le_bytes());
        b[0x74..0x78].copy_from_slice(&tex_transform_count.to_le_bytes());
        b[0x134..0x138].copy_from_slice(&ribbon_count.to_le_bytes());
        b
    }

    #[test]
    fn texture_transform_and_ribbon_counts_read_the_verified_header_offsets() {
        let b = header_with_counts(3, 7);
        assert_eq!(m2_texture_transform_count(&b), 3);
        assert_eq!(m2_ribbon_emitter_count(&b), 7);
    }

    #[test]
    fn texture_transform_and_ribbon_counts_are_zero_on_a_too_short_or_non_md20_buffer() {
        assert_eq!(m2_texture_transform_count(&[]), 0);
        assert_eq!(m2_ribbon_emitter_count(&[]), 0);
        let short = header_with_counts(3, 7);
        assert_eq!(m2_texture_transform_count(&short[..0x50]), 0);
        assert_eq!(m2_ribbon_emitter_count(&short[..0x50]), 0);
        let mut not_md20 = header_with_counts(3, 7);
        not_md20[0..4].copy_from_slice(b"XXXX");
        assert_eq!(m2_texture_transform_count(&not_md20), 0);
        assert_eq!(m2_ribbon_emitter_count(&not_md20), 0);
    }

    /// A hermetic (no real client data required) end-to-end check of [`parse_m2_animation_summary`]'s
    /// static/dynamic classification: an all-zero-array header (no sequences, no bones, no emitters, no
    /// transforms) must report fully static with every count at zero.
    #[test]
    fn empty_model_summary_is_fully_static() {
        let b = header_with_counts(0, 0);
        let summary = parse_m2_animation_summary(&b).expect("an all-zero-array header parses");
        assert_eq!(summary.sequence_count, 0);
        assert!(!summary.seq0_has_bone_motion);
        assert_eq!(summary.seq0_animated_bone_count, 0);
        assert_eq!(summary.seq0_variation_count, 0);
        assert!(summary.global_seq_channels.is_empty());
        assert_eq!(summary.transparency_tracks, (0, 0));
        assert_eq!(summary.color_rgb_tracks, (0, 0));
        assert_eq!(summary.color_alpha_tracks, (0, 0));
        assert_eq!(summary.texture_transform_count, 0);
        assert_eq!(summary.particle_emitter_count, 0);
        assert!(summary.emitter_bones.is_empty());
        assert_eq!(summary.ribbon_emitter_count, 0);
        assert!(summary.is_fully_static());
    }
}
