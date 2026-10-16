//! An M2 (MD20) model reader for WoW 1.12.1 (versions 256–257) and Wrath of the Lich King 3.3.5
//! (version 264): a header of `M2Array`s (`count`, file `offset`), the arrays they name, and the
//! skin profiles. A Wrath file is 20 bytes narrower in the header, packs its bones and tracks
//! differently, and keeps its view geometry in external `*.skin` files. Particle emitters and
//! lights are not read here; benilla-formats reads them from the raw bytes.

mod error;
mod layout;
mod model;
mod skin;
mod track;

pub use error::Error;
pub use layout::M2Layout;
pub use model::{
    M2ArrayString, M2Attachment, M2BlendMode, M2Bone, M2BoneFlags, M2Camera, M2EventMarker,
    M2Format, M2Header, M2Material, M2Model, M2PlayableAnim, M2RawData, M2RenderFlags, M2Texture,
    M2TextureTransform, M2TextureType, M2Vertex, C2, C3,
};
pub use skin::{Skin, SkinBatch, SkinSection};
pub use track::{
    track_quat_wrath, track_read_wrath, track_vec3_timed_wrath, CubicValue, M2QuatTrack,
    M2ScalarSplineTrack, M2ScalarTrack, M2SplineKey, M2Track, M2Vec3SplineTrack, M2Vec3Track,
};

use std::ffi::CString;
use std::io::Cursor;

use benilla_bytes::{capped, ByteExt};

use error::Result;
use track::{
    track_fix16, track_fix16_wrath, track_quat, track_spline_f32, track_spline_f32_wrath,
    track_spline_vec3, track_spline_vec3_wrath, track_vec3_timed,
};

/// A `C3Vector` (3×f32) at `o`.
fn rd_c3(b: &[u8], o: usize) -> Option<C3> {
    Some(C3 {
        x: b.f32_at(o)?,
        y: b.f32_at(o + 4)?,
        z: b.f32_at(o + 8)?,
    })
}

/// Parse an MD20 model; the cursor's position is ignored.
pub fn parse_m2(cursor: &mut Cursor<&[u8]>) -> Result<M2Format> {
    let b: &[u8] = cursor.get_ref();
    if b.len() < 8 || &b[0..4] != b"MD20" {
        return Err(Error::NotMd20);
    }
    let version = b.u32_at(4).ok_or(Error::Truncated)?;
    // The layout for this version, refusing what benilla does not read (256–263, 264).
    let lay = M2Layout::from_version(version).ok_or(Error::UnsupportedVersion(version))?;

    // The header's `(count, offset)` M2Arrays at the layout's offsets.
    let arr = |ofs: usize| -> Result<(u32, u32)> {
        Ok((
            b.u32_at(ofs).ok_or(Error::Truncated)?,
            b.u32_at(ofs + 4).ok_or(Error::Truncated)?,
        ))
    };
    let _name = arr(lay.name)?;
    let _global = arr(lay.global_loops)?;
    let _anim = arr(lay.sequences)?;
    // AnimationLookup (`+0x24`), which the reference's ownership test `0x711960` reads.
    let animation_lookup_arr = arr(lay.animation_lookup)?;
    // PlayableAnimationLookup (`+0x2c`): pre-Wrath only; a Wrath file has no playable-animation
    // lookup, so there is nothing to read.
    let playable_animation_lookup_arr = match lay.playable_animation_lookup {
        Some(ofs) => arr(ofs)?,
        None => (0, 0),
    };
    let bones = arr(lay.bones)?;
    let _key_bone = arr(lay.key_bone)?;
    let vertices = arr(lay.vertices)?;
    // The skin slot: an embedded skin-profile `M2Array` pre-Wrath, a bare profile count in Wrath,
    // whose view geometry lives in external `*.skin` files.
    let views = if lay.wrath {
        (b.u32_at(lay.skin).ok_or(Error::Truncated)?, 0)
    } else {
        arr(lay.skin)?
    };
    let colors_arr = arr(lay.colors)?;
    let textures = arr(lay.textures)?;
    let transparency_arr = arr(lay.transparency)?;
    let tex_anim_arr = arr(lay.tex_transforms)?;
    let _color_repl = arr(lay.texture_indices_by_id)?;
    let render_flags = arr(lay.materials)?;
    let _bone_lookup = arr(lay.bone_combos)?;
    let texture_lookup_table = arr(lay.texture_combos)?;
    // texture_unit_lookup, a stage's UV channel or environment coordinate. These three lookups sit
    // where the reference's header walk `0x71cdf0` puts them, not a slot earlier.
    let texture_unit_lookup_arr = arr(lay.tex_coord_combos)?;
    // transparency_lookup (u16), `weight_combo_index` to a weight track.
    let transparency_lookup_arr = arr(lay.tex_weight_combos)?;
    // texture_animation_lookup (u16), `texture_transform_combo_index` to a texture transform
    // (`0x70b897`); 0xffff is none.
    let tex_anim_lookup_arr = arr(lay.tex_transform_combos)?;
    // Authored bounds: AABB (min C3, max C3) then sphere radius.
    let read_box = |o: usize| -> Result<[f32; 3]> {
        Ok([
            b.f32_at(o).ok_or(Error::Truncated)?,
            b.f32_at(o + 4).ok_or(Error::Truncated)?,
            b.f32_at(o + 8).ok_or(Error::Truncated)?,
        ])
    };
    let bounding_box_min = read_box(lay.bounding_box_min)?;
    let bounding_box_max = read_box(lay.bounding_box_max)?;
    let bounding_sphere_radius = b
        .f32_at(lay.bounding_sphere_radius)
        .ok_or(Error::Truncated)?;
    // The collision box and sphere radius: the tight hull.
    let collision_box_min = read_box(lay.collision_box_min)?;
    let collision_box_max = read_box(lay.collision_box_max)?;
    let collision_sphere_radius = b
        .f32_at(lay.collision_sphere_radius)
        .ok_or(Error::Truncated)?;
    let bounding_triangles = arr(lay.collision_indices)?;
    let bounding_vertices = arr(lay.collision_positions)?;
    let _bounding_normals = arr(lay.collision_face_normals)?;
    let attachments = arr(lay.attachments)?;
    let attach_lookup = arr(lay.attach_lookup)?;
    // The animation events (`$CSL`, `$BWR`, `$SND`…); only their positions are read.
    let events = arr(lay.events)?;
    // The follow camera's pivot height (`0x50cbc0`): attachment id 17's model-space Z.
    let pivot_attach_z = attachment_z(b, 17, attachments, attach_lookup, lay.attachment_stride);

    // --- read the arrays we keep ---
    let get = |start: usize, len: usize| -> Result<&[u8]> {
        b.bytes_at(start, len).ok_or(Error::Truncated)
    };

    // Vertices: 48 bytes each. The reservation is capped by the file; a short file fails at `get`.
    let vert_avail = b.len().saturating_sub(vertices.1 as usize);
    let mut verts = Vec::with_capacity(capped(vertices.0 as usize, 48, vert_avail));
    for i in 0..vertices.0 as usize {
        let v = get(vertices.1 as usize + i * 48, 48)?;
        verts.push(M2Vertex {
            position: rd_c3(v, 0).ok_or(Error::Truncated)?,
            bone_weights: [v[12], v[13], v[14], v[15]],
            bone_indices: [v[16], v[17], v[18], v[19]],
            normal: rd_c3(v, 20).ok_or(Error::Truncated)?,
            tex_coords: C2 {
                x: v.f32_at(32).ok_or(Error::Truncated)?,
                y: v.f32_at(36).ok_or(Error::Truncated)?,
            },
        });
    }

    // Textures: 16 bytes each (type u32, flags u32, filename M2Array).
    let tex_avail = b.len().saturating_sub(textures.1 as usize);
    let mut texs = Vec::with_capacity(capped(textures.0 as usize, 16, tex_avail));
    for i in 0..textures.0 as usize {
        let t = get(textures.1 as usize + i * 16, 16)?;
        let ttype = M2TextureType::from_u32(t.u32_at(0).ok_or(Error::Truncated)?);
        // `+0x04`, the address mode: bit 0 repeats U, bit 1 V, clear clamps to edge. Cutout cards
        // put UVs outside `0..1` to clamp to their transparent edge; repeated, they draw solid.
        let tflags = t.u32_at(4).ok_or(Error::Truncated)?;
        let (fcount, fofs) = (
            t.u32_at(8).ok_or(Error::Truncated)? as usize,
            t.u32_at(12).ok_or(Error::Truncated)? as usize,
        );
        let raw = b.bytes_at(fofs, fcount).unwrap_or(&[]);
        let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
        let string = CString::new(&raw[..end]).unwrap_or_default();
        texs.push(M2Texture {
            texture_type: ttype,
            wrap_x: tflags & 0x1 != 0,
            wrap_y: tflags & 0x2 != 0,
            filename: M2ArrayString { string },
        });
    }

    // Materials (render flags): 4 bytes each (flags u16, blend_mode u16).
    let flags_avail = b.len().saturating_sub(render_flags.1 as usize);
    let mut mats = Vec::with_capacity(capped(render_flags.0 as usize, 4, flags_avail));
    for i in 0..render_flags.0 as usize {
        let m = get(render_flags.1 as usize + i * 4, 4)?;
        mats.push(M2Material {
            flags: M2RenderFlags(m.u16_at(0).ok_or(Error::Truncated)?),
            blend_mode: M2BlendMode(m.u16_at(2).ok_or(Error::Truncated)?),
        });
    }

    // Bones: key bone i32, flags u32, parent i16, submesh u16, then three tracks and a pivot C3.
    // Pre-Wrath the record is 108 bytes (28-byte tracks, pivot @96); Wrath packs an 88-byte
    // record (a name CRC, 20-byte tracks, pivot @0x4c). The reference zeroes a NaN pivot.
    let bones_avail = b.len().saturating_sub(bones.1 as usize);
    let mut bone_list = Vec::with_capacity(capped(bones.0 as usize, lay.bone_stride, bones_avail));
    for i in 0..bones.0 as usize {
        let bn = get(bones.1 as usize + i * lay.bone_stride, lay.bone_stride)?;
        let mut pivot = rd_c3(bn, lay.pivot_ofs).ok_or(Error::Truncated)?;
        if pivot.x.is_nan() {
            pivot.x = 0.0;
        }
        if pivot.y.is_nan() {
            pivot.y = 0.0;
        }
        if pivot.z.is_nan() {
            pivot.z = 0.0;
        }
        bone_list.push(M2Bone {
            key_bone: bn.u32_at(0).ok_or(Error::Truncated)? as i32 as i16,
            flags: M2BoneFlags(bn.u32_at(4).ok_or(Error::Truncated)?),
            parent: bn.u16_at(8).ok_or(Error::Truncated)? as i16,
            pivot,
        });
    }

    // Attachments (id, bone, position C3 @8, a skipped position track). A record whose id or bone
    // overflows `u16`, or whose bone is out of range, is dropped; real ids stop at 36.
    let att_count = attachments.0 as usize;
    let att_avail = b.len().saturating_sub(attachments.1 as usize);
    // The table must fit the file first: `emitted_at` below is sized from the raw count.
    if capped(att_count, lay.attachment_stride, att_avail) < att_count {
        return Err(Error::Truncated);
    }
    let mut attachment_list = Vec::with_capacity(att_count);
    // AttachLookup indexes the file's records; a dropped one shifts ours, so map file to emitted.
    let mut emitted_at = vec![0xffffu16; att_count]; // proven to fit above
    for (i, emitted) in emitted_at.iter_mut().enumerate() {
        let a = get(
            attachments.1 as usize + i * lay.attachment_stride,
            lay.attachment_stride,
        )?;
        let id = a.u32_at(0).ok_or(Error::Truncated)?;
        let bone = a.u32_at(4).ok_or(Error::Truncated)?;
        let position = rd_c3(a, 8).ok_or(Error::Truncated)?;
        let (Ok(id), Ok(bone)) = (u16::try_from(id), u16::try_from(bone)) else {
            continue;
        };
        if bone as usize >= bone_list.len() {
            continue;
        }
        *emitted = u16::try_from(attachment_list.len()).unwrap_or(0xffff);
        attachment_list.push(M2Attachment {
            id,
            bone,
            position: [position.x, position.y, position.z],
        });
    }

    // AttachLookup (`+0x10c`): attachment id to its one record, the reference's only id map
    // (`0x710310`). Shipped weapons repeat ids (`Stave_2H_Long_D_05`: 0-3 twice) and the lookup
    // names one. Stored as emitted indices, `0xffff` for none.
    let lk_avail = b.len().saturating_sub(attach_lookup.1 as usize);
    let mut attach_lookup_list = Vec::with_capacity(capped(attach_lookup.0 as usize, 2, lk_avail));
    for i in 0..attach_lookup.0 as usize {
        let raw = get(attach_lookup.1 as usize + i * 2, 2)?
            .u16_at(0)
            .ok_or(Error::Truncated)?;
        attach_lookup_list.push(emitted_at.get(raw as usize).copied().unwrap_or(0xffff));
    }

    // Event markers (4CC, data u32, bone u32 @8, position C3 @12, a skipped timer track): 44
    // bytes pre-Wrath, 36 in Wrath, whose timer track carries no value array. Dropped like
    // attachments, kept in file order.
    let ev_avail = b.len().saturating_sub(events.1 as usize);
    let mut event_markers =
        Vec::with_capacity(capped(events.0 as usize, lay.event_stride, ev_avail));
    for i in 0..events.0 as usize {
        let e = get(events.1 as usize + i * lay.event_stride, lay.event_stride)?;
        let ident = [e[0], e[1], e[2], e[3]];
        let bone = e.u32_at(8).ok_or(Error::Truncated)?;
        let position = rd_c3(e, 12).ok_or(Error::Truncated)?;
        let Ok(bone) = u16::try_from(bone) else {
            continue;
        };
        if bone as usize >= bone_list.len() {
            continue;
        }
        event_markers.push(M2EventMarker {
            ident,
            bone,
            position: [position.x, position.y, position.z],
        });
    }

    // AnimationLookup: one u16 each.
    let al_avail = b.len().saturating_sub(animation_lookup_arr.1 as usize);
    let mut animation_lookup =
        Vec::with_capacity(capped(animation_lookup_arr.0 as usize, 2, al_avail));
    for i in 0..animation_lookup_arr.0 as usize {
        animation_lookup.push(
            get(animation_lookup_arr.1 as usize + i * 2, 2)?
                .u16_at(0)
                .ok_or(Error::Truncated)?,
        );
    }

    // PlayableAnimationLookup: one dword each, low16 resolved id / high16 dir flags.
    let pal_avail = b
        .len()
        .saturating_sub(playable_animation_lookup_arr.1 as usize);
    let mut playable_animation_lookup = Vec::with_capacity(capped(
        playable_animation_lookup_arr.0 as usize,
        4,
        pal_avail,
    ));
    for i in 0..playable_animation_lookup_arr.0 as usize {
        let dword = get(playable_animation_lookup_arr.1 as usize + i * 4, 4)?
            .u32_at(0)
            .ok_or(Error::Truncated)?;
        playable_animation_lookup.push(M2PlayableAnim {
            resolved_id: (dword & 0xffff) as u16,
            dir_flags: (dword >> 16) as u16,
        });
    }

    // Texture lookup table: u16 each.
    let tlt_avail = b.len().saturating_sub(texture_lookup_table.1 as usize);
    let mut tlt = Vec::with_capacity(capped(texture_lookup_table.0 as usize, 2, tlt_avail));
    for i in 0..texture_lookup_table.0 as usize {
        tlt.push(
            get(texture_lookup_table.1 as usize + i * 2, 2)?
                .u16_at(0)
                .ok_or(Error::Truncated)?,
        );
    }

    // Band starts to rebase Wrath per-sequence tracks onto the shared absolute timeline.
    let band_starts = band_starts_for(b, &lay);
    // A track reader that dispatches on the version: Wrath's 20-byte nested tracks, rebased by
    // `band_starts`, or the pre-Wrath 28-byte flat tracks.
    let read_fix16 = |ofs: usize| {
        if lay.wrath {
            track_fix16_wrath(b, ofs, &band_starts)
        } else {
            track_fix16(b, ofs)
        }
    };
    let read_vec3 = |ofs: usize| {
        if lay.wrath {
            track_vec3_timed_wrath(b, ofs, &band_starts)
        } else {
            track_vec3_timed(b, ofs)
        }
    };
    let read_quat = |ofs: usize| {
        if lay.wrath {
            track_quat_wrath(b, ofs, &band_starts)
        } else {
            track_quat(b, ofs)
        }
    };
    // M2Color (two tracks: RGB @+0, fix16 alpha @+track) and M2TextureWeight (one weight track).
    // An out-of-range track reads as no keys, so these loops run over the records the file holds,
    // never the raw count.
    let color_stride = 2 * lay.track_stride;
    let colors_avail = b.len().saturating_sub(colors_arr.1 as usize);
    let colors_cap = capped(colors_arr.0 as usize, color_stride, colors_avail);
    let mut color_alpha_tracks = Vec::with_capacity(colors_cap);
    let mut color_rgb_tracks = Vec::with_capacity(colors_cap);
    for i in 0..colors_cap {
        let base = colors_arr.1 as usize + i * color_stride;
        color_alpha_tracks.push(read_fix16(base + lay.track_stride));
        color_rgb_tracks.push(read_vec3(base));
    }
    let transparency_avail = b.len().saturating_sub(transparency_arr.1 as usize);
    let transparency_cap = capped(
        transparency_arr.0 as usize,
        lay.track_stride,
        transparency_avail,
    );
    let mut transparency_tracks = Vec::with_capacity(transparency_cap);
    for i in 0..transparency_cap {
        transparency_tracks.push(read_fix16(
            transparency_arr.1 as usize + i * lay.track_stride,
        ));
    }
    let tulookup_avail = b.len().saturating_sub(texture_unit_lookup_arr.1 as usize);
    let mut texture_unit_lookup = Vec::with_capacity(capped(
        texture_unit_lookup_arr.0 as usize,
        2,
        tulookup_avail,
    ));
    for i in 0..texture_unit_lookup_arr.0 as usize {
        texture_unit_lookup.push(
            get(texture_unit_lookup_arr.1 as usize + i * 2, 2)?
                .u16_at(0)
                .ok_or(Error::Truncated)?,
        );
    }
    let tlookup_avail = b.len().saturating_sub(transparency_lookup_arr.1 as usize);
    let mut transparency_lookup =
        Vec::with_capacity(capped(transparency_lookup_arr.0 as usize, 2, tlookup_avail));
    for i in 0..transparency_lookup_arr.0 as usize {
        transparency_lookup.push(
            get(transparency_lookup_arr.1 as usize + i * 2, 2)?
                .u16_at(0)
                .ok_or(Error::Truncated)?,
        );
    }

    // M2TextureTransform (three tracks: translation @+0, rotation @+track, scaling @+2·track),
    // bounded by the file like the colour tracks.
    let ttf_stride = 3 * lay.track_stride;
    let ttf_avail = b.len().saturating_sub(tex_anim_arr.1 as usize);
    let ttf_cap = capped(tex_anim_arr.0 as usize, ttf_stride, ttf_avail);
    let mut texture_transforms = Vec::with_capacity(ttf_cap);
    for i in 0..ttf_cap {
        let base = tex_anim_arr.1 as usize + i * ttf_stride;
        texture_transforms.push(M2TextureTransform {
            translation: read_vec3(base),
            rotation: read_quat(base + lay.track_stride),
            scaling: read_vec3(base + 2 * lay.track_stride),
        });
    }
    let talookup_avail = b.len().saturating_sub(tex_anim_lookup_arr.1 as usize);
    let mut texture_transform_lookup =
        Vec::with_capacity(capped(tex_anim_lookup_arr.0 as usize, 2, talookup_avail));
    for i in 0..tex_anim_lookup_arr.0 as usize {
        texture_transform_lookup.push(
            get(tex_anim_lookup_arr.1 as usize + i * 2, 2)?
                .u16_at(0)
                .ok_or(Error::Truncated)?,
        );
    }

    // Global-sequence durations (u32 ms): the clocks `gseq` tracks wrap on.
    let gseq_arr = (
        b.u32_at(lay.global_loops).ok_or(Error::Truncated)?,
        b.u32_at(lay.global_loops + 4).ok_or(Error::Truncated)?,
    );
    let gseq_avail = b.len().saturating_sub(gseq_arr.1 as usize);
    let mut global_sequences = Vec::with_capacity(capped(gseq_arr.0 as usize, 4, gseq_avail));
    for i in 0..gseq_arr.0 as usize {
        let Some(ms) = b.u32_at(gseq_arr.1 as usize + i * 4) else {
            break;
        };
        global_sequences.push(ms);
    }

    // The collision hull, raw: triangle indices (u16) and vertices (C3Vector).
    let bt_bytes = get(
        bounding_triangles.1 as usize,
        bounding_triangles.0 as usize * 2,
    )?
    .to_vec();
    let bv_bytes = get(
        bounding_vertices.1 as usize,
        bounding_vertices.0 as usize * 12,
    )?
    .to_vec();

    Ok(M2Format {
        model: M2Model {
            vertices: verts,
            textures: texs,
            materials: mats,
            color_alpha_tracks,
            color_rgb_tracks,
            transparency_tracks,
            transparency_lookup,
            texture_unit_lookup,
            texture_transforms,
            texture_transform_lookup,
            global_sequences,
            bones: bone_list,
            cameras: parse_cameras(b),
            camera_lookup: parse_camera_lookup(b),
            raw_data: M2RawData {
                texture_lookup_table: tlt,
                bounding_triangles: bt_bytes,
                bounding_vertices: bv_bytes,
            },
            header: M2Header {
                bounding_box_min,
                bounding_box_max,
                bounding_sphere_radius,
                collision_box_min,
                collision_box_max,
                collision_sphere_radius,
            },
            pivot_attach_z,
            attachments: attachment_list,
            attach_lookup: attach_lookup_list,
            event_markers,
            animation_lookup,
            playable_animation_lookup,
            views,
            version,
        },
    })
}

/// Parse the MD20 camera array alone. The header offset, the record stride, and the spline-track
/// format are all version-dependent (Wrath packs 20-byte nested tracks where pre-Wrath packs 28).
/// A truncated table yields the records that fit.
pub fn parse_cameras(b: &[u8]) -> Vec<M2Camera> {
    let Some(lay) = M2Layout::of(b) else {
        return Vec::new();
    };
    let (Some(count), Some(ofs)) = (b.u32_at(lay.cameras), b.u32_at(lay.cameras + 4)) else {
        return Vec::new();
    };
    let band_starts = band_starts_for(b, &lay);
    let read_spline_vec3 = |rec: usize| {
        if lay.wrath {
            track_spline_vec3_wrath(b, rec, &band_starts)
        } else {
            track_spline_vec3(b, rec)
        }
    };
    let read_spline_f32 = |rec: usize| {
        if lay.wrath {
            track_spline_f32_wrath(b, rec, &band_starts)
        } else {
            track_spline_f32(b, rec)
        }
    };
    let avail = b.len().saturating_sub(ofs as usize);
    let mut out = Vec::with_capacity(capped(count as usize, lay.camera_stride, avail));
    for i in 0..count as usize {
        let rec = ofs as usize + i * lay.camera_stride;
        // A record is read whole or not at all.
        if rec
            .checked_add(lay.camera_stride)
            .is_none_or(|end| end > b.len())
        {
            break;
        }
        let (Some(camera_type), Some(fov), Some(far_clip), Some(near_clip)) = (
            b.u32_at(rec).map(|v| v as i32),
            b.f32_at(rec + 0x04),
            b.f32_at(rec + 0x08),
            b.f32_at(rec + 0x0c),
        ) else {
            break;
        };
        let (Some(position_base), Some(target_base)) = (
            rd_c3(b, rec + lay.camera_position_base).map(|c| [c.x, c.y, c.z]),
            rd_c3(b, rec + lay.camera_target_base).map(|c| [c.x, c.y, c.z]),
        ) else {
            break;
        };
        out.push(M2Camera {
            camera_type,
            fov,
            far_clip,
            near_clip,
            positions: read_spline_vec3(rec + lay.camera_position_track),
            position_base,
            target: read_spline_vec3(rec + lay.camera_target_track),
            target_base,
            roll: read_spline_f32(rec + lay.camera_roll_track),
        });
    }
    out
}

/// Parse the MD20 CameraLookup (`u16` each) at the layout's offset; see [`M2Model::camera_lookup`].
pub fn parse_camera_lookup(b: &[u8]) -> Vec<u16> {
    let Some(lay) = M2Layout::of(b) else {
        return Vec::new();
    };
    let (Some(count), Some(ofs)) = (b.u32_at(lay.camera_lookup), b.u32_at(lay.camera_lookup + 4))
    else {
        return Vec::new();
    };
    let avail = b.len().saturating_sub(ofs as usize);
    let mut out = Vec::with_capacity(capped(count as usize, 2, avail));
    for i in 0..count as usize {
        let Some(v) = b.u16_at(ofs as usize + i * 2) else {
            break;
        };
        out.push(v);
    }
    out
}

/// The absolute start of each sequence's band, in file order: pre-Wrath sequences sit on one
/// timeline and carry their `start` directly; a Wrath sequence owns a clock, so its band starts
/// where the previous one's duration left off. Empty pre-Wrath (the flat tracks are already
/// absolute) and when there are no sequences. benilla-formats cuts the bone channels per band
/// with the same starts, so the reader is shared.
pub fn band_starts_for(b: &[u8], lay: &M2Layout) -> Vec<u32> {
    if !lay.wrath {
        return Vec::new();
    }
    let Some(scnt) = b.u32_at(lay.sequences).map(|n| n as usize) else {
        return Vec::new();
    };
    let Some(sofs) = b.u32_at(lay.sequences + 4).map(|n| n as usize) else {
        return Vec::new();
    };
    let mut acc = 0u32;
    (0..scnt)
        .map(|s| {
            let start = acc;
            acc = acc.saturating_add(b.u32_at(sofs + s * 0x40 + 4).unwrap_or(0));
            start
        })
        .collect()
}

/// Attachment `id`'s model-space Z through the lookup, from the file's `(count, offset)` pairs.
/// `stride` is the attachment record size for the file's version (48 pre-Wrath, 40 in Wrath).
fn attachment_z(
    b: &[u8],
    id: usize,
    attachments: (u32, u32),
    lookup: (u32, u32),
    stride: usize,
) -> Option<f32> {
    if id >= lookup.0 as usize {
        return None;
    }
    let li = lookup.1 as usize + id * 2;
    let idx = b.u16_at(li)? as usize;
    if idx >= attachments.0 as usize {
        return None;
    }
    let rec = attachments.1 as usize + idx * stride;
    // `position` is at +8 of the record, so its Z is at +16.
    b.f32_at(rec + 16)
}

#[cfg(test)]
mod tests;
