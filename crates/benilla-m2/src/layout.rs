//! The MD20 file layout by version: the header offset of every `M2Array` and the stride of each
//! fixed record. A Wrath (264) file is narrower than a pre-Wrath (≤263) one — no
//! `playable_animation_lookup`, a 4-byte skin count in place of the skin-profile `M2Array`, no
//! `texture_flipbooks` — so every field after `animation_lookup` sits 8–20 bytes earlier, and the
//! bone and track records are packed differently.

use benilla_bytes::ByteExt;

/// The file layout of an MD20 model for one version range. Offsets are file-relative; record
/// strides are in bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct M2Layout {
    /// The version word the file carries, as the layout was chosen for it.
    pub version: u32,
    /// A Wrath (3.3.5) file: the skin geometry lives in external `*.skin` files, the bones carry
    /// a name CRC and compressed-quaternion tracks, and the header is 20 bytes narrower.
    pub wrath: bool,

    // ── The `(count, offset)` header arrays, by file offset ────────────────────────────────────
    pub name: usize,
    /// The `global_flags` u32, alone (not an array).
    pub global_flags: usize,
    pub global_loops: usize,
    pub sequences: usize,
    pub animation_lookup: usize,
    /// `None` in a Wrath file, which has no playable-animation lookup.
    pub playable_animation_lookup: Option<usize>,
    pub bones: usize,
    /// `boneIndicesById`, the key-bone lookup.
    pub key_bone: usize,
    pub vertices: usize,
    /// The skin slot: a skin-profile `M2Array` pre-Wrath, a bare skin-count u32 in Wrath.
    pub skin: usize,
    pub colors: usize,
    pub textures: usize,
    /// `texture_weights`: the M2TextureWeight transparency tracks.
    pub transparency: usize,
    pub tex_transforms: usize,
    /// `textureIndicesById` (a.k.a. replacable texture lookup).
    pub texture_indices_by_id: usize,
    /// The materials (render flags + blend mode).
    pub materials: usize,
    /// `boneCombos` (a.k.a. bone lookup table).
    pub bone_combos: usize,
    /// `textureCombos` (a.k.a. texture lookup table).
    pub texture_combos: usize,
    /// `textureCoordCombos` (a.k.a. texture mapping lookup table).
    pub tex_coord_combos: usize,
    /// `textureWeightCombos` (a.k.a. transparency lookup table).
    pub tex_weight_combos: usize,
    /// `textureTransformCombos` (a.k.a. texture transforms lookup table).
    pub tex_transform_combos: usize,
    pub bounding_box_min: usize,
    pub bounding_box_max: usize,
    pub bounding_sphere_radius: usize,
    pub collision_box_min: usize,
    pub collision_box_max: usize,
    pub collision_sphere_radius: usize,
    pub collision_indices: usize,
    pub collision_positions: usize,
    pub collision_face_normals: usize,
    pub attachments: usize,
    /// `attachmentIndicesById` (a.k.a. attachment lookup table).
    pub attach_lookup: usize,
    pub events: usize,
    pub lights: usize,
    pub cameras: usize,
    /// `cameraIndicesById` (a.k.a. camera lookup table).
    pub camera_lookup: usize,
    pub ribbon: usize,
    pub particle: usize,

    // ── Record strides and in-record offsets ───────────────────────────────────────────────────
    /// One sequence record: 68 pre-Wrath (`start`/`end`), 64 in Wrath (`length`).
    pub sequence_stride: usize,
    /// The sequence's authored `CAaBox` min corner, within its record: `+0x24` pre-Wrath,
    /// `+0x20` in Wrath.
    pub seq_box_min: usize,
    /// The sequence box's max corner (its Z is the chat-bubble and swim-pivot read): `+0x30`
    /// pre-Wrath, `+0x2c` in Wrath.
    pub seq_box_max: usize,
    /// One bone record: 108 pre-Wrath, 88 in Wrath (a name CRC, and 20-byte tracks not 28).
    pub bone_stride: usize,
    /// One M2Track: 28 pre-Wrath (flat keys), 20 in Wrath (a `M2Array` of per-sequence keys).
    pub track_stride: usize,
    /// One attachment record (`id`, `bone`, `position`, then a position track that benilla does
    /// not read): 48 pre-Wrath, 40 in Wrath.
    pub attachment_stride: usize,
    /// One event record (`ident`, `data`, `bone`, `position`, then a timer track that benilla
    /// does not read): 44 pre-Wrath, 36 in Wrath, whose value-less timer track is 12 bytes.
    pub event_stride: usize,
    /// A bone's pivot `C3Vector`, within its record.
    pub pivot_ofs: usize,
    /// A bone's name-CRC u32, within its record; `None` pre-Wrath.
    pub bone_crc_ofs: Option<usize>,
    /// A bone's translation track, within its record.
    pub bone_translation_ofs: usize,
    /// A bone's rotation track, within its record.
    pub bone_rotation_ofs: usize,
    /// A bone's scale track, within its record.
    pub bone_scale_ofs: usize,
    /// One camera record: 0x7c pre-Wrath (28-byte spline tracks), 0x64 in Wrath (20-byte).
    pub camera_stride: usize,
    /// One particle emitter record: 0x1f8 pre-Wrath, 0x1dc in Wrath (the restructured 3.3.5
    /// `sParticles`: a byte blend/emitter, a `Color_Replaceable` word, and keyed over-life
    /// tracks in place of the fixed 3-key ramp).
    pub particle_stride: usize,
    /// One ribbon emitter record: 0xdc pre-Wrath, 0xb0 in Wrath (20-byte tracks plus the 3.3.5
    /// `PriorityPlane`/`RibbonColorIndex`/`TextureTransformLookupIndex` trailer).
    pub ribbon_stride: usize,
    /// A camera's position track, within its record.
    pub camera_position_track: usize,
    /// A camera's position base `C3Vector`, within its record.
    pub camera_position_base: usize,
    /// A camera's target track, within its record.
    pub camera_target_track: usize,
    /// A camera's target base `C3Vector`, within its record.
    pub camera_target_base: usize,
    /// A camera's roll track, within its record.
    pub camera_roll_track: usize,
}

impl M2Layout {
    /// The layout for `version`, or `None` when it is not one benilla reads: 256–263 (vanilla and
    /// TBC, pre-Wrath) and 264 (Wrath, 3.3.5).
    pub fn from_version(version: u32) -> Option<Self> {
        let wrath = version == 264;
        let pre_wrath = (256..=263).contains(&version);
        if !(wrath || pre_wrath) {
            return None;
        }
        // Pre-Wrath keeps the playable-animation lookup and the skin-profile array and the
        // texture flipbooks, so its fields start where Wrath's `bones` would be.
        let bones = if wrath { 0x02c } else { 0x034 };
        let key_bone = bones + 8;
        let vertices = key_bone + 8;
        let skin = vertices + 8;
        // Wrath's skin slot is a bare u32 count (4 bytes); pre-Wrath's is an M2Array (8).
        let colors = if wrath { skin + 4 } else { skin + 8 };
        // Wrath drops the texture flipbooks, so its texture transforms start 8 earlier.
        let transparency = colors + 16;
        let tex_transforms = if wrath {
            transparency + 8
        } else {
            transparency + 16
        };
        let tex_weight_combos = tex_transforms + 48;
        let tex_transform_combos = tex_weight_combos + 8;
        // The authored bounds and everything after run contiguously from the last combo.
        let rest = tex_transform_combos;
        Some(Self {
            version,
            wrath,
            name: 0x008,
            global_flags: 0x010,
            global_loops: 0x014,
            sequences: 0x01c,
            animation_lookup: 0x024,
            playable_animation_lookup: (!wrath).then_some(0x02c),
            bones,
            key_bone,
            vertices,
            skin,
            colors,
            textures: colors + 8,
            transparency,
            tex_transforms,
            texture_indices_by_id: tex_transforms + 8,
            materials: tex_transforms + 16,
            bone_combos: tex_transforms + 24,
            texture_combos: tex_transforms + 32,
            tex_coord_combos: tex_transforms + 40,
            tex_weight_combos,
            tex_transform_combos,
            bounding_box_min: rest + 8,
            bounding_box_max: rest + 20,
            bounding_sphere_radius: rest + 32,
            collision_box_min: rest + 36,
            collision_box_max: rest + 48,
            collision_sphere_radius: rest + 60,
            collision_indices: rest + 64,
            collision_positions: rest + 72,
            collision_face_normals: rest + 80,
            attachments: rest + 88,
            attach_lookup: rest + 96,
            events: rest + 104,
            lights: rest + 112,
            cameras: rest + 120,
            camera_lookup: rest + 128,
            ribbon: rest + 136,
            particle: rest + 144,
            sequence_stride: if wrath { 0x40 } else { 0x44 },
            seq_box_min: if wrath { 0x20 } else { 0x24 },
            seq_box_max: if wrath { 0x2c } else { 0x30 },
            bone_stride: if wrath { 88 } else { 108 },
            track_stride: if wrath { 20 } else { 28 },
            attachment_stride: if wrath { 40 } else { 48 },
            event_stride: if wrath { 36 } else { 44 },
            pivot_ofs: if wrath { 0x4c } else { 0x60 },
            bone_crc_ofs: wrath.then_some(0x0c),
            bone_translation_ofs: if wrath { 0x10 } else { 0x0c },
            bone_rotation_ofs: if wrath { 0x24 } else { 0x28 },
            bone_scale_ofs: if wrath { 0x38 } else { 0x44 },
            camera_stride: if wrath { 0x64 } else { 0x7c },
            camera_position_track: 0x10,
            camera_position_base: if wrath { 0x24 } else { 0x2c },
            camera_target_track: if wrath { 0x30 } else { 0x38 },
            camera_target_base: if wrath { 0x44 } else { 0x54 },
            camera_roll_track: if wrath { 0x50 } else { 0x60 },
            particle_stride: if wrath { 0x1dc } else { 0x1f8 },
            ribbon_stride: if wrath { 0xb0 } else { 0xdc },
        })
    }

    /// The `(count, offset)` `M2Array` at `ofs`, or `None` past the end of `b`.
    pub fn array_at(&self, b: &[u8], ofs: usize) -> Option<(u32, u32)> {
        Some((b.u32_at(ofs)?, b.u32_at(ofs + 4)?))
    }

    /// The layout a parsed model carries: read the version word and dispatch.
    pub fn of(bytes: &[u8]) -> Option<Self> {
        let version = bytes.u32_at(4)?;
        Self::from_version(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vanilla_offsets_match_the_reference_walk() {
        let l = M2Layout::from_version(256).expect("vanilla is supported");
        assert!(!l.wrath);
        assert_eq!((l.bones, l.vertices, l.skin), (0x034, 0x044, 0x04c));
        assert_eq!(l.playable_animation_lookup, Some(0x02c));
        assert_eq!(
            (l.colors, l.textures, l.transparency, l.tex_transforms),
            (0x054, 0x05c, 0x064, 0x074)
        );
        assert_eq!(
            (
                l.attachments,
                l.events,
                l.lights,
                l.cameras,
                l.camera_lookup
            ),
            (0x104, 0x114, 0x11c, 0x124, 0x12c)
        );
        assert_eq!(
            (l.sequence_stride, l.seq_box_min, l.seq_box_max),
            (0x44, 0x24, 0x30)
        );
        assert_eq!(
            (l.bone_stride, l.track_stride, l.pivot_ofs),
            (108, 28, 0x60)
        );
        assert_eq!((l.attachment_stride, l.event_stride), (48, 44));
        assert_eq!((l.particle_stride, l.ribbon_stride), (0x1f8, 0xdc));
        assert_eq!(l.bone_crc_ofs, None);
    }

    #[test]
    fn wrath_offsets_match_the_335_header() {
        let l = M2Layout::from_version(264).expect("wrath is supported");
        assert!(l.wrath);
        assert_eq!(
            (l.bones, l.key_bone, l.vertices, l.skin),
            (0x02c, 0x034, 0x03c, 0x044)
        );
        assert_eq!(l.playable_animation_lookup, None);
        assert_eq!(
            (l.colors, l.textures, l.transparency, l.tex_transforms),
            (0x048, 0x050, 0x058, 0x060)
        );
        assert_eq!(
            (l.bounding_box_min, l.bounding_sphere_radius),
            (0x0a0, 0x0b8)
        );
        assert_eq!(
            (
                l.attachments,
                l.attach_lookup,
                l.events,
                l.lights,
                l.cameras,
                l.camera_lookup
            ),
            (0x0f0, 0x0f8, 0x100, 0x108, 0x110, 0x118)
        );
        assert_eq!((l.ribbon, l.particle), (0x120, 0x128));
        assert_eq!(
            (l.sequence_stride, l.seq_box_min, l.seq_box_max),
            (0x40, 0x20, 0x2c)
        );
        assert_eq!((l.bone_stride, l.track_stride, l.pivot_ofs), (88, 20, 0x4c));
        assert_eq!((l.attachment_stride, l.event_stride), (40, 36));
        assert_eq!((l.particle_stride, l.ribbon_stride), (0x1dc, 0xb0));
        assert_eq!(l.bone_crc_ofs, Some(0x0c));
        assert_eq!(
            (
                l.bone_translation_ofs,
                l.bone_rotation_ofs,
                l.bone_scale_ofs
            ),
            (0x10, 0x24, 0x38)
        );
    }

    #[test]
    fn unsupported_versions_are_refused() {
        assert_eq!(M2Layout::from_version(999), None);
        assert_eq!(M2Layout::from_version(265), None);
        assert_eq!(M2Layout::from_version(255), None);
    }
}
