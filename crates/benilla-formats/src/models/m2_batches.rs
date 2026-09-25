//! M2 render-batch assembly: one [`RenderSubmesh`] per skin batch, split across billboard bones so
//! each card turns about its own pivot.

use std::io::Cursor;

use anyhow::{Context, Result};
use benilla_bytes::ByteExt;
use benilla_m2::parse_m2;
use benilla_m2::M2TextureType;

use crate::Chain;

use super::anim::parse_m2_animations;
use super::key_anim::SeqSlot;
use super::mat_anim;
use super::tex_anim;
use super::{le_u16, le_u32, model_path, remap_submesh};
use super::{
    AlphaAnim, Billboard, BillboardKind, BoneScaleAnim, CharSkinSlot, FogPolicy, ModelBlend,
    RenderSubmesh,
};

fn parent_dir(path: &str) -> &str {
    match path.rfind(['\\', '/']) {
        Some(i) => &path[..i],
        None => "",
    }
}

/// A batch's `.blp`: `Monster1/2/3` come from the creature's skin variations
/// (`<model-dir>\<name>.blp`), else the embedded filename.
fn resolve_texture(
    tex: &benilla_m2::M2Texture,
    dir: &str,
    skins: &[Option<String>],
) -> (Option<String>, Option<u8>, Option<CharSkinSlot>, bool) {
    // Texture type 14 is the icon slot, `ReplaceIconTexture`'s target.
    let icon_slot = matches!(tex.texture_type, M2TextureType::Other(14));
    let embedded = {
        let f = tex.filename.string.to_string_lossy();
        (!f.is_empty()).then(|| f.into_owned())
    };
    let variation = |i: usize| {
        skins
            .get(i)
            .and_then(|o| o.clone())
            .map(|name| format!("{dir}\\{name}.blp"))
    };
    // Character slots with no embedded path, filled per player at spawn: type 1 the body atlas,
    // 2 the object skin (item or cape), 6 the hair texture, 8 the extra skin (tauren fur).
    let char_slot = match tex.texture_type {
        M2TextureType::Other(1) => Some(CharSkinSlot::Body),
        M2TextureType::Other(2) => Some(CharSkinSlot::Object),
        M2TextureType::Other(6) => Some(CharSkinSlot::Hair),
        M2TextureType::Other(8) => Some(CharSkinSlot::SkinExtra),
        _ => None,
    };
    // The skin slot is reported so a skin-less load can fill it at spawn.
    match tex.texture_type {
        M2TextureType::Monster1 => (variation(0).or(embedded), Some(0), None, false),
        M2TextureType::Monster2 => (variation(1).or(embedded), Some(1), None, false),
        M2TextureType::Monster3 => (variation(2).or(embedded), Some(2), None, false),
        _ => (embedded, None, char_slot, icon_slot),
    }
}

/// A visibility track's static value: `None` if time-varying, `1.0` for an empty track.
fn track_constant(track: &benilla_m2::M2ScalarTrack) -> Option<f32> {
    if track.keys.is_empty() {
        return Some(1.0);
    }
    track.constant()
}

/// The 4 raw `u8` weights as `f32` summing to 1; a weightless vertex binds wholly to bone 0.
fn normalize_weights(w: [u8; 4]) -> [f32; 4] {
    let sum = w.iter().map(|&x| f32::from(x)).sum::<f32>();
    if sum <= 0.0 {
        return [1.0, 0.0, 0.0, 0.0];
    }
    [
        f32::from(w[0]) / sum,
        f32::from(w[1]) / sum,
        f32::from(w[2]) / sum,
        f32::from(w[3]) / sum,
    ]
}

/// Bone `bone_idx`'s scale track (`+0x44`) as a global-sequence [`BoneScaleAnim`], at the offsets
/// `0x714260` reads; `None` for a sequence track or fewer than two keys.
fn parse_bone_scale_anim(bytes: &[u8], bone_idx: usize) -> Option<BoneScaleAnim> {
    let bone_count = bytes.u32_at(0x34)? as usize;
    let bones_ofs = bytes.u32_at(0x38)? as usize;
    if bone_idx >= bone_count {
        return None;
    }
    let track = bones_ofs.checked_add(bone_idx * 0x6c)?.checked_add(0x44)?;
    let interp = bytes.u16_at(track)? != 0;
    let gseq = bytes.u16_at(track + 0x02)?;
    if gseq == 0xffff {
        return None; // a sequence track, not a global-sequence loop
    }
    let nkeys = bytes.u32_at(track + 0x0c)? as usize;
    let ts_ofs = bytes.u32_at(track + 0x10)? as usize;
    let nval = bytes.u32_at(track + 0x14)? as usize;
    let val_ofs = bytes.u32_at(track + 0x18)? as usize;
    if nkeys <= 1 || nval < nkeys {
        return None; // static: nothing to animate
    }
    let gseq_count = bytes.u32_at(0x14)? as usize;
    let gseq_o = bytes.u32_at(0x18)? as usize;
    if gseq as usize >= gseq_count {
        return None;
    }
    let duration_ms = bytes.u32_at(gseq_o.checked_add(gseq as usize * 4)?)?;
    if duration_ms == 0 {
        return None;
    }
    let keys = (0..nkeys)
        .map(|k| {
            let t = bytes.u32_at(ts_ofs.checked_add(k * 4)?)?;
            let v = val_ofs.checked_add(k * 12)?;
            Some((
                t,
                [bytes.f32_at(v)?, bytes.f32_at(v + 4)?, bytes.f32_at(v + 8)?],
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(BoneScaleAnim {
        duration_ms,
        interp,
        keys,
    })
}

/// Bone `bone_idx`'s translation track (`+0x0c`) cut to one sequence's `band` as a loop (the
/// questgiver bob: anim 0 on load, 190 raised, `0x6076c0`); `None` under two keys in the band.
fn parse_bone_seq_translation(
    bytes: &[u8],
    bone_idx: usize,
    band: (u32, u32),
) -> Option<BoneScaleAnim> {
    let (start, end) = band;
    let duration_ms = end.checked_sub(start).filter(|d| *d > 0)?;
    let bone_count = bytes.u32_at(0x34)? as usize;
    let bones_ofs = bytes.u32_at(0x38)? as usize;
    if bone_idx >= bone_count {
        return None;
    }
    let track = bones_ofs.checked_add(bone_idx * 0x6c)?.checked_add(0x0c)?;
    let interp = bytes.u16_at(track)? != 0;
    if bytes.u16_at(track + 0x02)? != 0xffff {
        return None; // a global-sequence loop
    }
    let nkeys = bytes.u32_at(track + 0x0c)? as usize;
    let ts_ofs = bytes.u32_at(track + 0x10)? as usize;
    let nval = bytes.u32_at(track + 0x14)? as usize;
    let val_ofs = bytes.u32_at(track + 0x18)? as usize;
    if nval < nkeys {
        return None;
    }
    // Every sequence's keys share one absolute timeline: keep this band's, rebased to its start.
    let mut keys = Vec::new();
    for k in 0..nkeys {
        let t = bytes.u32_at(ts_ofs.checked_add(k * 4)?)?;
        if t < start || t > end {
            continue;
        }
        let v = val_ofs.checked_add(k * 12)?;
        keys.push((
            t - start,
            [bytes.f32_at(v)?, bytes.f32_at(v + 4)?, bytes.f32_at(v + 8)?],
        ));
    }
    if keys.len() <= 1 {
        return None; // static in this band
    }
    Some(BoneScaleAnim {
        duration_ms,
        interp,
        keys,
    })
}

/// Load an M2 from the chain as render submeshes; creatures use [`load_m2_mesh_skinned`].
pub fn load_m2_mesh(chain: &mut Chain, raw_path: &str) -> Result<Vec<RenderSubmesh>> {
    load_m2_mesh_skinned(chain, raw_path, &[])
}

/// [`load_m2_mesh`] with `Monster1/2/3` filled from `skins` (`CreatureDisplayInfo` variations).
pub fn load_m2_mesh_skinned(
    chain: &mut Chain,
    raw_path: &str,
    skins: &[Option<String>],
) -> Result<Vec<RenderSubmesh>> {
    let path = model_path(raw_path);
    let dir = parent_dir(&path).to_string();
    let bytes = chain
        .read_file(&path)
        .with_context(|| format!("reading M2 {path}"))?;
    parse_m2_render_submeshes(&bytes, &dir, skins).with_context(|| format!("parsing M2 {path}"))
}

/// [`crate::m2_bone_spins`] for a model read from the chain.
pub fn load_m2_bone_spins(
    chain: &mut Chain,
    raw_path: &str,
) -> Result<std::collections::HashMap<u16, crate::BoneSpin>> {
    let path = model_path(raw_path);
    let bytes = chain
        .read_file(&path)
        .with_context(|| format!("reading M2 {path}"))?;
    Ok(crate::m2_bone_spins(&bytes))
}

/// The billboard bones the card split declines as not rigidly separable.
pub fn non_separable_billboard_bones(bytes: &[u8]) -> Vec<u16> {
    let Ok(format) = parse_m2(&mut Cursor::new(bytes)) else {
        return Vec::new();
    };
    let model = format.model();
    let Ok(skin) = model.parse_embedded_skin(bytes, 0) else {
        return Vec::new();
    };
    let separable = separable_billboard_bones(model, skin.triangles(), skin.indices());
    model
        .bones
        .iter()
        .enumerate()
        .filter(|(i, b)| b.is_billboard() && !separable.get(*i).copied().unwrap_or(false))
        .map(|(i, _)| i as u16)
        .collect()
}

/// Per bone: may the card split claim it? Yes for a non-billboard bone, and for a billboard bone
/// that (1) wholly owns every vertex it influences and (2) every triangle touching one, checked
/// over the whole skin, since a flap's seam ring can sit in another batch than its tip.
fn separable_billboard_bones(
    model: &benilla_m2::M2Model,
    tris: &[u16],
    lookup: &[u16],
) -> Vec<bool> {
    let mut separable = vec![true; model.bones.len()];
    let mut deny = |b: usize| {
        if let Some(s) = separable.get_mut(b) {
            *s = false;
        }
    };
    // (1) Any partial weight on a bone disqualifies it.
    for v in &model.vertices {
        for i in 0..4 {
            let w = v.bone_weights[i];
            if w != 0 && w != u8::MAX {
                deny(v.bone_indices[i] as usize);
            }
        }
    }
    // The bone a vertex is wholly on, if any.
    let sole_bone = |g: usize| -> Option<usize> {
        let v = model.vertices.get(g)?;
        (0..4)
            .find(|&i| v.bone_weights[i] == u8::MAX)
            .map(|i| v.bone_indices[i] as usize)
    };
    // (2) A triangle not wholly on one bone denies every bone it touches, hard straddles too.
    for t in tris.as_chunks::<3>().0 {
        let g: Vec<usize> = t
            .iter()
            .filter_map(|&i| lookup.get(i as usize).map(|&x| x as usize))
            .collect();
        let [a, b, c] = g[..] else { continue };
        let (sa, sb, sc) = (sole_bone(a), sole_bone(b), sole_bone(c));
        if sa.is_some() && sa == sb && sb == sc {
            continue; // wholly on one bone
        }
        for &v in &[a, b, c] {
            let Some(vert) = model.vertices.get(v) else {
                continue;
            };
            for i in 0..4 {
                if vert.bone_weights[i] != 0 {
                    deny(vert.bone_indices[i] as usize);
                }
            }
        }
    }
    separable
}

/// An in-memory M2 as render submeshes; `dir` resolves texture paths, `skins` fill `Monster1/2/3`.
pub fn parse_m2_render_submeshes(
    bytes: &[u8],
    dir: &str,
    skins: &[Option<String>],
) -> Result<Vec<RenderSubmesh>> {
    let format =
        parse_m2(&mut Cursor::new(bytes)).map_err(|e| anyhow::anyhow!("parsing M2: {e}"))?;
    let model = format.model();
    let skin = model
        .parse_embedded_skin(bytes, 0)
        .map_err(|e| anyhow::anyhow!("embedded skin: {e}"))?;

    let vertex = |g: u32| {
        let v = &model.vertices[g as usize];
        (
            [v.position.x, v.position.y, v.position.z],
            [v.normal.x, v.normal.y, v.normal.z],
            [v.tex_coords.x, v.tex_coords.y],
            [1.0, 1.0, 1.0, 1.0], // M2 has no MOCV; cleared below so no ATTRIBUTE_COLOR is emitted
        )
    };
    // `triangles` index into `indices`, which index the global vertices.
    let lookup = skin.indices();
    let tris = skin.triangles();
    let global = |t_range: std::ops::Range<usize>| -> Vec<u32> {
        tris.get(t_range)
            .unwrap_or(&[])
            .iter()
            .filter_map(|&t| lookup.get(t as usize).copied())
            .map(u32::from)
            .filter(|&g| (g as usize) < model.vertices.len())
            .collect()
    };

    // A split card turns rigidly about its pivot, while the reference skins every vertex through
    // the palette (`0x71a460`), a billboard bone's camera-replaced matrix (`0x7151f9`) one more
    // weight. The two agree only for a rigidly separable bone; the rest stay whole, skinned on a
    // rigged spawn (`billboard_joint_palette`).
    let separable = separable_billboard_bones(model, tris, lookup);

    // Every sequence's (anim id, absolute band, loops). The loop flag is the baked loop's clock
    // (`KeyAnim::wrap`): a one-shot such as Death must not wrap back to its opening alpha.
    let seq_bands: Vec<(u16, (u32, u32), bool)> = {
        let (n, o) = (le_u32(bytes, 0x1c) as usize, le_u32(bytes, 0x20) as usize);
        (0..n)
            .map_while(|i| {
                let e = o + i * 0x44;
                (e + 0x44 <= bytes.len()).then(|| {
                    (
                        le_u16(bytes, e),
                        (le_u32(bytes, e + 0x04), le_u32(bytes, e + 0x08)),
                        le_u32(bytes, e + 0x10) & 1 == 0,
                    )
                })
            })
            .collect()
    };
    // File slot, band and clock; the slot indexes key ranges, so this keeps file order.
    let seq_slots: Vec<SeqSlot> = seq_bands
        .iter()
        .enumerate()
        .map(|(index, &(_, band, looping))| SeqSlot {
            index,
            band,
            looping,
        })
        .collect();
    // Slot 0: the one band the shared-material UV and tint registries key on.
    let seq0_slot = seq_slots.first().copied();

    let sections = skin.submeshes();
    let mut out = Vec::new();
    for batch in skin.batches() {
        let Some(section) = sections.get(batch.skin_section_index as usize) else {
            continue;
        };
        let start = section.triangle_start as usize;
        let global_indices = global(start..start + section.triangle_count as usize);
        if global_indices.is_empty() {
            continue;
        }
        let tex_record = model
            .raw_data
            .texture_lookup_table
            .get(batch.texture_combo_index as usize)
            .and_then(|&ti| model.textures.get(ti as usize));
        // The record's address mode (`flags & 0x1/0x2`); repeat without a record.
        let (wrap_x, wrap_y) = tex_record.map_or((true, true), |t| (t.wrap_x, t.wrap_y));
        let (texture, skin_slot, char_slot, icon_slot) = tex_record
            .map(|t| resolve_texture(t, dir, skins))
            .unwrap_or((None, None, None, false));
        let material = model.materials.get(batch.material_index as usize);
        let blend = match material.map(|m| m.blend_mode.bits()) {
            Some(0) | None => ModelBlend::Opaque,
            Some(1) => ModelBlend::AlphaTest,
            // Modes 5/6 multiply (the `0x811fe0` remap): 5 Mod is `DST_COLOR/ZERO`, 6 Mod2x
            // `DST_COLOR/SRC_COLOR`, the ARMORREFLECT sheen layers.
            Some(5) => ModelBlend::Mod,
            Some(6) => ModelBlend::Mod2x,
            Some(_) => ModelBlend::Blend,
        };
        // Blend modes 3 (NoAlphaAdd) and 4 (Add) add the batch's colour (glow cards, coronae).
        let additive = matches!(material.map(|m| m.blend_mode.bits()), Some(3) | Some(4));
        // Render flag 0x04: two-sided; without it the reference culls back faces.
        let two_sided = material.is_some_and(|m| m.flags.bits() & 0x04 != 0);
        // `texCoordSet (+0x12) → texture_unit_lookup (0x9c)` above 2 is a generated environment
        // coordinate, as the reference gates it at `0x70b8bd`; stage 0 only, one texture a batch.
        let env_map = model.stage_is_env_mapped(batch, 0);
        // Render flag 0x01, unlit: full texture brightness (lamp glass, glow cards).
        let emissive = material.is_some_and(|m| m.flags.bits() & 0x01 != 0);
        // Render flags 0x10 (no depth write) and 0x08 (no depth test): the reference keys depth
        // state on these bits, not on the blend mode (`0x70c190`).
        let no_depth_write = material.is_some_and(|m| m.flags.bits() & 0x10 != 0);
        let no_depth_test = material.is_some_and(|m| m.flags.bits() & 0x08 != 0);
        // Fog colour (setter `0x70baf0`): off under render flag 0x02 (`0x70bb24`), else by blend
        // mode from `DAT_811fc4 = {1,1,1,2,2,3,4}` (`0x70bddf`, jump table `0x70c17c`): modes
        // 0/1/2 the scene colour, 3/4 black, 5 white, 6 grey.
        let fog_policy = match material {
            Some(m) if m.flags.bits() & 0x02 != 0 => FogPolicy::Off,
            Some(m) => match m.blend_mode.bits() {
                3 | 4 => FogPolicy::Black,
                5 => FogPolicy::White,
                6 => FogPolicy::Grey,
                _ => FogPolicy::Scene,
            },
            None => FogPolicy::Scene,
        };
        // Static cull (`0x707b3a`): the reference skips any batch, opaque too, whose
        // `instanceAlpha · colorAlpha · transparencyWeight ≤ 0`; a constant-0 track is dropped here
        // (`OrgrimmarFloatingEmbers`' box), as instance alpha is 1 at build.
        let color_alpha = if (batch.color_index as usize) < model.color_alpha_tracks.len() {
            track_constant(&model.color_alpha_tracks[batch.color_index as usize])
        } else {
            Some(1.0) // colorIndex out of range (0xffff is none): no colour factor
        };
        let weight = if batch.texture_count != 0 {
            model
                .transparency_lookup
                .get(batch.weight_combo_index as usize)
                .and_then(|&t| model.transparency_tracks.get(t as usize))
                .map_or(Some(1.0), track_constant)
        } else {
            Some(1.0) // textureCount 0: no transparency factor
        };
        if let (Some(c), Some(w)) = (color_alpha, weight) {
            if c * w <= 0.0 {
                continue; // constant-invisible: the reference never draws it
            }
        }
        // What the static cull cannot fold away bakes to an `AlphaAnim`, one per sequence, as the
        // reference reads the playing sequence's key window each frame.
        let alpha_anim = {
            let color_track = model.color_alpha_tracks.get(batch.color_index as usize);
            let weight_track = (batch.texture_count != 0)
                .then(|| {
                    model
                        .transparency_lookup
                        .get(batch.weight_combo_index as usize)
                        .and_then(|&t| model.transparency_tracks.get(t as usize))
                })
                .flatten();
            let per_seq = seq_slots
                .iter()
                .map(|&slot| mat_anim::AlphaSeq {
                    color: color_track.and_then(|t| {
                        mat_anim::bake_scalar_anim(t, &model.global_sequences, Some(slot))
                    }),
                    weight: weight_track.and_then(|t| {
                        mat_anim::bake_scalar_anim(t, &model.global_sequences, Some(slot))
                    }),
                })
                .collect();
            AlphaAnim::new(per_seq)
        };
        // The UV loop: batch combo → `texAnimLookup` → translation track, on the same clocks.
        let uv_anim = tex_anim::bake_uv_anim(model, batch.texture_transform_combo_index, seq0_slot);
        // Per sequence only when the slots disagree: the shared registry cannot key on one.
        let uv_seq = tex_anim::bake_uv_seqs(model, batch.texture_transform_combo_index, &seq_slots)
            .filter(|set| set.uniform().is_none());
        // Rotation and scaling per slot, for lanes owning a material per instance (UI model tiles).
        let uv_rot_seq =
            tex_anim::bake_uv_rot_seqs(model, batch.texture_transform_combo_index, &seq_slots);
        let uv_scale_seq =
            tex_anim::bake_uv_scale_seqs(model, batch.texture_transform_combo_index, &seq_slots);
        // The animated M2Color tint: only a time-varying track bakes, replacing the vertex tint.
        let rgb_track = model.color_rgb_tracks.get(batch.color_index as usize);
        let rgb_anim =
            rgb_track.and_then(|t| mat_anim::bake_rgb_anim(t, &model.global_sequences, seq0_slot));
        let rgb_seq = rgb_track
            .and_then(|t| mat_anim::bake_rgb_seqs(t, &model.global_sequences, &seq_slots))
            .filter(|set| set.uniform().is_none());
        // The reference turns each billboard bone to the camera about its own pivot, and a batch
        // can hold cards on several bones (a candelabra's glows): one submesh per billboard bone.
        let make_billboard = |bone_idx: usize| -> Option<Billboard> {
            let bone = model.bones.get(bone_idx)?;
            Some(Billboard {
                pivot: [bone.pivot.x, bone.pivot.y, bone.pivot.z],
                bone: bone_idx as u16,
                kind: BillboardKind::from_bone_flags(bone.flags.bits())?,
                // The glow-card pulse: the bone's global-sequence scale track.
                scale_anim: parse_bone_scale_anim(bytes, bone_idx),
                // Per-sequence translation loops (the questgiver bob: anim 0 low, 190 raised).
                seq_translations: seq_bands
                    .iter()
                    .filter_map(|&(id, band, _)| {
                        Some((id, parse_bone_seq_translation(bytes, bone_idx, band)?))
                    })
                    .collect(),
            })
        };
        // A vertex's primary bone if a separable billboard bone; `None` is the ordinary group.
        let primary_billboard_bone = |g: u32| -> Option<usize> {
            let b = model.vertices.get(g as usize)?.bone_indices[0] as usize;
            (model.bones.get(b).is_some_and(|bone| bone.is_billboard())
                && separable.get(b).copied().unwrap_or(false))
            .then_some(b)
        };
        let mut groups: Vec<(Option<usize>, Vec<u32>)> = Vec::new();
        for tri in global_indices.as_chunks::<3>().0 {
            let key = primary_billboard_bone(tri[0]);
            if let Some(pos) = groups.iter().position(|(k, _)| *k == key) {
                groups[pos].1.extend_from_slice(tri);
            } else {
                groups.push((key, tri.to_vec()));
            }
        }
        // M2Color (header `0x54`, by `texUnit.colorIndex`), which the reference multiplies into the
        // vertex colour: a constant bakes in here, RGB only, as its alpha is already in the cull
        // and `alpha_anim`. It is all the warmth of a glow on a neutral texture (the Orgrimmar
        // bonfire's `GenericGlow_Alpha_128`).
        let color_tint: Option<[f32; 4]> = match &rgb_anim {
            Some(_) => None,
            // Mod and Mod2x discard the M2Color RGB (`0x70c507`/`0x70c5b8` zero the tint·M2Color
            // term), so their vertex colours stay untinted.
            None if matches!(blend, ModelBlend::Mod | ModelBlend::Mod2x) => None,
            None => model
                .color_rgb_tracks
                .get(batch.color_index as usize)
                .and_then(|t| t.keys.first())
                .map(|&(_, rgb)| [rgb[0], rgb[1], rgb[2], 1.0]),
        };
        for (bone, idx) in groups {
            let (mut sub, globals) = remap_submesh(
                idx.into_iter(),
                vertex,
                texture.clone(),
                blend,
                two_sided,
                false, // M2 has no interior/exterior group concept
                emissive,
            );
            sub.billboard = bone.and_then(make_billboard);
            // Geometry on a billboard bone the split refused, by the same `separable` table.
            sub.welded_billboard = globals.iter().any(|&g| {
                let v = &model.vertices[g as usize];
                (0..4).any(|i| {
                    let b = v.bone_indices[i] as usize;
                    v.bone_weights[i] != 0
                        && model.bones.get(b).is_some_and(|bone| bone.is_billboard())
                        && !separable.get(b).copied().unwrap_or(false)
                })
            });
            sub.env_map = env_map;
            sub.additive = additive;
            sub.no_depth_write = no_depth_write;
            sub.no_depth_test = no_depth_test;
            sub.fog_policy = fog_policy;
            sub.skin_slot = skin_slot;
            sub.geoset_id = section.id;
            sub.section = Some(batch.skin_section_index);
            sub.wrap_x = wrap_x;
            sub.wrap_y = wrap_y;
            sub.char_slot = char_slot;
            sub.icon_slot = icon_slot;
            sub.joints = globals
                .iter()
                .map(|&g| {
                    let bi = model.vertices[g as usize].bone_indices;
                    [
                        u16::from(bi[0]),
                        u16::from(bi[1]),
                        u16::from(bi[2]),
                        u16::from(bi[3]),
                    ]
                })
                .collect();
            sub.weights = globals
                .iter()
                .map(|&g| normalize_weights(model.vertices[g as usize].bone_weights))
                .collect();

            // No tint leaves the vec empty, so the renderer emits no `ATTRIBUTE_COLOR`.
            match color_tint {
                Some(c) => sub.vertex_colors = vec![c; sub.positions.len()],
                None => sub.vertex_colors.clear(),
            }
            sub.alpha_anim = alpha_anim.clone();
            sub.uv_anim = uv_anim.clone();
            sub.uv_seq = uv_seq.clone();
            sub.uv_rot_seq = uv_rot_seq.clone();
            sub.uv_scale_seq = uv_scale_seq.clone();
            sub.rgb_anim = rgb_anim.clone();
            sub.rgb_seq = rgb_seq.clone();
            out.push(sub);
        }
    }
    // The invisible `SpellObject_InvisibleTrap` placeholder, which the reference draws at alpha ≈0
    // (8.5e-05 in a trace) from an untraced source; this drops its geometry instead.
    if is_white1_placeholder(
        model.header.bounding_box_min,
        model.header.bounding_box_max,
        &out,
    ) {
        out.clear();
    }
    Ok(out)
}

/// A flat render box whose every batch is opaque `WHITE1.BLP`: no GameObject model but the
/// placeholder matches, and textured flat decals stay.
fn is_white1_placeholder(bbox_min: [f32; 3], bbox_max: [f32; 3], subs: &[RenderSubmesh]) -> bool {
    if subs.is_empty() {
        return false;
    }
    let degenerate = (0..3).any(|k| (bbox_max[k] - bbox_min[k]).abs() < 1e-3);
    degenerate
        && subs.iter().all(|s| {
            matches!(s.blend, ModelBlend::Opaque)
                && !s.additive
                && s.texture
                    .as_deref()
                    .is_some_and(|t| t.to_ascii_lowercase().ends_with("white1.blp"))
        })
}

/// How far a model's transparent batches sort from its origin, yards: the reference draws a
/// model's emitters after its batches, which one sorted list gets by biasing past them. A batch
/// sorts at its bind-pose box centre, not [`super::M2Bounds::vert_max_from_origin`].
pub fn m2_owner_reach(subs: &[RenderSubmesh]) -> f32 {
    subs.iter()
        .filter(|s| {
            s.additive
                || matches!(
                    s.blend,
                    ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x
                )
        })
        .filter(|s| !s.positions.is_empty())
        .map(|s| {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &s.positions {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            // A length, so the same in WoW and Bevy axes (a signed axis permutation apart).
            let c: [f32; 3] = std::array::from_fn(|k| 0.5 * (lo[k] + hi[k]));
            (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt()
        })
        .fold(0.0f32, f32::max)
}

/// The `Transparent3d` depth bias sorting an effect after its owner's transparent batches and no
/// further: the next whole yard above the reach, since Bevy keys pipelines on `depth_bias as i32`
/// and a batch centred at the reach must not tie.
pub fn owner_last_rung(reach_world: f32) -> f32 {
    // Deviation: owners past the ceiling (Maraudon's waterfalls, the glue screens) keep the
    // interleave with their emitters, because a bias past a 200-yard owner would jump every
    // transparent surface in the zone.
    const MAX_RUNG: f32 = 32.0;
    (reach_world.max(0.0).floor() + 1.0).min(MAX_RUNG)
}

/// The rungs a mesh-shard material may carry, a pipeline-key axis through `depth_bias`: a closed
/// set `pipe_warm` compiles behind the loading cover. Most spell-kit owners fit the first.
pub const OWNER_RUNG_BUCKETS: [f32; 3] = [4.0, 12.0, 32.0];

/// Snap a rung up to its bucket, the last one past the end, so the key space stays closed.
pub fn owner_last_rung_bucket(rung: f32) -> f32 {
    for b in OWNER_RUNG_BUCKETS {
        if rung <= b {
            return b;
        }
    }
    OWNER_RUNG_BUCKETS[OWNER_RUNG_BUCKETS.len() - 1]
}

/// The textures sequence `anim_id` shows, in batch order: a model can hold alternative arts whose
/// M2Color alpha steps to 1 only in their own band (`Rotating-MinimapArrow`). Each track holds its
/// last key at or before the band, searched in the sequence's key window as the reference does.
pub fn m2_sequence_visible_textures(bytes: &[u8], anim_id: u16) -> Option<Vec<String>> {
    let format = parse_m2(&mut Cursor::new(bytes)).ok()?;
    let model = format.model();
    let skin = model.parse_embedded_skin(bytes, 0).ok()?;
    let seq = parse_m2_animations(bytes)
        .into_iter()
        .find(|a| a.anim_id == anim_id)?;

    // An empty window (`lo >= hi`) gives `keys[lo]`, as the reference's key search does.
    let at_band_start = |track: &benilla_m2::M2ScalarTrack| -> f32 {
        let (lo, hi) = track
            .ranges
            .get(seq.seq_index)
            .copied()
            .unwrap_or((0, track.keys.len().saturating_sub(1) as u32));
        let (lo, hi) = (
            lo as usize,
            (hi as usize).min(track.keys.len().saturating_sub(1)),
        );
        let window = track.keys.get(lo..=hi).unwrap_or(&[]);
        window
            .iter()
            .take_while(|(ts, _)| *ts <= seq.start_ms)
            .last()
            .or(window.first())
            .map_or(1.0, |&(_, v)| v)
    };

    let mut shown = Vec::new();
    for b in skin.batches() {
        let alpha = model
            .color_alpha_tracks
            .get(b.color_index as usize)
            .map_or(1.0, at_band_start);
        let weight = model
            .transparency_lookup
            .get(b.weight_combo_index as usize)
            .and_then(|&t| model.transparency_tracks.get(t as usize))
            .map_or(1.0, at_band_start);
        if alpha <= 0.0 || weight <= 0.0 {
            continue;
        }
        let Some(tex) = model
            .raw_data
            .texture_lookup_table
            .get(b.texture_combo_index as usize)
            .and_then(|&t| model.textures.get(t as usize))
        else {
            continue;
        };
        shown.push(tex.filename.string.to_string_lossy().to_string());
    }
    Some(shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bucket never sorts under the exact rung, which its sibling quad cloud keeps.
    #[test]
    fn owner_rung_buckets_cover_every_reachable_rung() {
        let max = OWNER_RUNG_BUCKETS[OWNER_RUNG_BUCKETS.len() - 1];
        assert_eq!(owner_last_rung(f32::MAX), max, "one shared ceiling");
        let mut prev = 0.0f32;
        for r in 1..=32 {
            let b = owner_last_rung_bucket(r as f32);
            assert!(b >= r as f32, "bucket({r}) sorts under the exact rung");
            assert!(b >= prev, "bucket must be monotonic");
            assert!(
                OWNER_RUNG_BUCKETS.contains(&b),
                "bucket({r}) not in the set"
            );
            prev = b;
        }
        // The snap-up scan relies on the set ascending.
        assert!(OWNER_RUNG_BUCKETS.windows(2).all(|w| w[0] < w[1]));
    }

    /// `TaurenMale.m2`'s type-8 fur, opaque core and alpha-cut two-sided fringe alike, surfaces as
    /// [`CharSkinSlot::SkinExtra`]; unmapped, it draws flat white.
    #[test]
    fn tauren_fur_batches_carry_the_skin_extra_slot() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let subs = load_m2_mesh(&mut chain, "Character\\Tauren\\Male\\TaurenMale.m2")
            .expect("parse TaurenMale");
        let extra: Vec<_> = subs
            .iter()
            .filter(|s| s.char_slot == Some(CharSkinSlot::SkinExtra))
            .collect();
        assert!(
            extra.len() > 20,
            "the tauren fur is many batches, got {}",
            extra.len()
        );
        assert!(
            extra.iter().all(|s| s.texture.is_none()),
            "type 8 has no embedded path — the spawn site fills it"
        );
        assert!(
            extra
                .iter()
                .any(|s| matches!(s.blend, ModelBlend::Opaque) && !s.two_sided),
            "the opaque single-sided fur core flavor exists"
        );
        assert!(
            extra
                .iter()
                .any(|s| matches!(s.blend, ModelBlend::AlphaTest) && s.two_sided),
            "the alpha-cut two-sided fringe flavor exists"
        );
    }

    /// `Axe_2H_Horde_C_01.m2`'s ARMORREFLECT layer is material `{flags 0x10, blendMode 6}`.
    #[test]
    fn whirlwind_axe_reflect_layer_is_mod2x() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let subs = load_m2_mesh(
            &mut chain,
            "Item\\ObjectComponents\\Weapon\\Axe_2H_Horde_C_01.m2",
        )
        .expect("parse Axe_2H_Horde_C_01");
        assert_eq!(subs.len(), 3, "handle + blade + reflect layer");
        let object: Vec<_> = subs
            .iter()
            .filter(|s| s.char_slot == Some(CharSkinSlot::Object))
            .collect();
        assert_eq!(
            object.len(),
            2,
            "handle + blade bind the item's Object skin"
        );
        let reflect = subs
            .iter()
            .find(|s| {
                s.texture
                    .as_deref()
                    .is_some_and(|t| t.to_ascii_uppercase().contains("ARMORREFLECT"))
            })
            .expect("the ARMORREFLECT layer exists");
        assert_eq!(reflect.blend, ModelBlend::Mod2x);
        assert!(!reflect.additive, "Mod2x is a multiply, not an add");
        assert!(reflect.no_depth_write, "render flag 0x10");
    }

    /// `LShoulder_Plate_PVPAlliance_A_01.m2`'s spikes ride billboard bones through 50/50 seam rings
    /// that sit in the body's batch, so only a model-wide test sees the weld.
    #[test]
    fn a_welded_billboard_spike_is_never_split_into_a_card() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let path = "Item\\ObjectComponents\\Shoulder\\LShoulder_Plate_PVPAlliance_A_01.m2";
        let subs = load_m2_mesh(&mut chain, path).expect("parse LShoulder_Plate_PVPAlliance_A_01");
        assert_eq!(subs.len(), 1, "body + both spikes stay one welded batch");
        assert!(
            subs[0].billboard.is_none(),
            "a welded bone is never claimed by the card split"
        );
        assert_eq!(subs[0].positions.len(), 152, "every vertex is present");
        // The seam is present, or a build that dropped the spikes would pass.
        let seam = subs[0]
            .weights
            .iter()
            .filter(|w| w.iter().any(|&x| x > 0.0 && x < 0.999))
            .count();
        assert_eq!(
            seam, 16,
            "the two 8-vertex 50/50 seam rings are in the mesh"
        );
        assert!(
            subs[0].welded_billboard,
            "the welded batch announces itself to the render lanes"
        );
        let bytes = chain.read_file(path).expect("read the m2");
        assert_eq!(non_separable_billboard_bones(&bytes), vec![1, 2]);
    }

    /// `Sword_2H_PVPAlliance_A_01.m2`'s hilt glows are 4-vertex quads wholly on their own bones.
    #[test]
    fn a_detached_glow_card_is_still_split_out() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let path = "Item\\ObjectComponents\\Weapon\\Sword_2H_PVPAlliance_A_01.m2";
        let subs = load_m2_mesh(&mut chain, path).expect("parse Sword_2H_PVPAlliance_A_01");
        let cards: Vec<_> = subs.iter().filter(|s| s.billboard.is_some()).collect();
        assert_eq!(cards.len(), 2, "both hilt glows stay cards");
        assert!(
            cards.iter().all(|c| c.positions.len() == 4),
            "each is its own 4-vertex quad"
        );
        assert!(
            subs.iter().all(|s| !s.welded_billboard),
            "…so no batch of it asks a lane to skin anything"
        );
        let bytes = chain.read_file(path).expect("read the m2");
        assert!(
            non_separable_billboard_bones(&bytes).is_empty(),
            "nothing on this model is welded"
        );
    }

    /// The questgiver `?` bobs z 0.000 → −0.089 in anim 0 and +0.517 → +0.427 in anim 190, the
    /// raised bob the client arms under an overhead name (`0x6076c0`).
    #[test]
    fn questionmark_marker_batch_carries_both_bob_loops() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let subs = load_m2_mesh(&mut chain, "Interface\\Buttons\\TalkToMeQuestionMark.mdx")
            .expect("parse TalkToMeQuestionMark");
        let bb = subs
            .iter()
            .find_map(|s| s.billboard.as_ref())
            .expect("the ? marker rides a billboard bone");
        assert!(matches!(bb.kind, BillboardKind::LockZ));
        let bob = |id: u16| {
            bb.seq_translations
                .iter()
                .find(|(a, _)| *a == id)
                .map(|(_, l)| l)
        };
        let low = bob(0).expect("the load-arm loop bakes");
        assert_eq!(low.duration_ms, 1533, "the first sequence's band");
        assert!(low.keys.len() >= 2, "a moving track: {:?}", low.keys);
        let (a, b) = (low.sample(0), low.sample(low.duration_ms / 2));
        assert_ne!(a, b, "the sampled offset varies across the loop");
        assert!(
            low.keys.iter().all(|(_, v)| v[2] <= 0.0),
            "anim 0 bobs at/below the attach point: {:?}",
            low.keys
        );
        let raised = bob(190).expect("the raised loop bakes");
        assert_eq!(raised.duration_ms, 1533, "same period as the low bob");
        assert!(
            raised.keys.iter().all(|(_, v)| v[2] > 0.4),
            "anim 190 is the authored raised bob: {:?}",
            raised.keys
        );
    }
}
