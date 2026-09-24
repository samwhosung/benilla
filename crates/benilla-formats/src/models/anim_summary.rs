//! An M2's animation-channel summary, telling a doodad that can render as a static mesh from one
//! that animates, and by which channel (`benilla-extract m2anim`, `doodadscan`).

use std::io::Cursor;

use anyhow::{Context, Result};
use benilla_m2::parse_m2;

use crate::Chain;

use super::{le_u32, model_path, parse_m2_animations, parse_m2_global_sequence_bones};

/// The texture-transform (UV animation) array count, MD20 `0x74` (count) / `0x78` (offset), as
/// the header walk `0x71cdf0` reads it: whether the model authors any UV animation.
pub fn m2_texture_transform_count(b: &[u8]) -> usize {
    if b.len() < 0x78 || &b[0..4] != b"MD20" {
        return 0;
    }
    le_u32(b, 0x74) as usize
}

/// The ribbon emitter array count, MD20 `0x134` (count) / `0x138` (offset), file stride `0xdc`
/// (the header walk `0x71cdf0`, the loader's copy loop `0x70ebd0`); the records are not parsed.
pub fn m2_ribbon_emitter_count(b: &[u8]) -> usize {
    if b.len() < 0x138 || &b[0..4] != b"MD20" {
        return 0;
    }
    le_u32(b, 0x134) as usize
}

/// A particle emitter's host bone, and whether that bone's chain (itself or any ancestor, since
/// motion composes down the hierarchy) animates: an emitter on a static chain never moves.
#[derive(Debug, Clone, Copy)]
pub struct EmitterBoneLink {
    pub bone: u16,
    /// The emitter's raw M2 flag word (`+0x04`).
    pub flags: u32,
    /// A bone in the chain carries a >1-key T/R/S track in sequence 0's band.
    pub chain_seq0: bool,
    /// A bone in the chain carries a global-sequence channel.
    pub chain_gseq: bool,
}

impl EmitterBoneLink {
    pub fn chain_animated(&self) -> bool {
        self.chain_seq0 || self.chain_gseq
    }
}

/// Per-model animation-channel summary.
#[derive(Debug, Clone)]
pub struct M2AnimSummary {
    pub sequence_count: usize,
    /// File sequence 0 (not necessarily Stand) has a multi-key bone track in its band.
    pub seq0_has_bone_motion: bool,
    pub seq0_animated_bone_count: usize,
    /// Sequences sharing sequence 0's `anim_id`, its variation chain. The reference arms with
    /// `variationIdx = −1`, a frequency-weighted pick over this chain (`0x695100`), so above 1 the
    /// instances do not all play the same variation.
    pub seq0_variation_count: usize,
    /// Bone global-sequence channels as `(bone, "T"/"R"/"S", period_ms)`, such as the eye-blink.
    pub global_seq_channels: Vec<(u16, &'static str, u32)>,
    /// `M2Color` alpha tracks as `(total, time-varying)`.
    pub color_alpha_tracks: (usize, usize),
    /// `M2Color` RGB tracks as `(total, multi-key)`.
    pub color_rgb_tracks: (usize, usize),
    /// `M2TextureWeight` tracks as `(total, time-varying)`.
    pub transparency_tracks: (usize, usize),
    pub texture_transform_count: usize,
    pub particle_emitter_count: usize,
    /// Each emitter's [`EmitterBoneLink`], in emitter record order.
    pub emitter_bones: Vec<EmitterBoneLink>,
    pub ribbon_emitter_count: usize,
}

impl M2AnimSummary {
    /// No channel animates, so the model can render as a static mesh.
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

    // Which bones move (in sequence 0's band or on a global sequence), then each emitter's bone
    // walked up its parent chain: an emitter on a static child of a swinging parent still moves.
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
        // Bounded by the bone count, so a malformed parent loop cannot spin.
        for _ in 0..=skeleton.bones.len() {
            let Some(rec) = skeleton.bones.get(b as usize) else {
                break; // an out-of-range bone ends the chain
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

    // Time-varying: more than one key, not all equal (an all-equal track is a folded constant).
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

/// Read an M2's animation-channel summary from the chain, by model or doodad (`.mdx`/`.mdl`) path.
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

    /// A minimal MD20 header through the ribbon array, with the two counts set.
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
