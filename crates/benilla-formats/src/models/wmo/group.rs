//! WMO group-file parsing: the MOGP header and sub-chunks, and the render batches built into
//! [`RenderSubmesh`]es against a parsed root.

use std::io::Cursor;

use anyhow::Result;
use benilla_wmo::{parse_wmo, Color, ParsedWmo, WmoGroup, WmoLiquid};

use super::{find_wmo_chunk, WmoRoot};
use crate::liquid::{LiquidKind, LiquidMesh};
use crate::models::{remap_submesh, ModelBlend, RenderSubmesh, WmoBatchClass};

/// MLIQ grid spacing in yards, the reference's `[0x810cc0]` (`0x40855555`): the water
/// (`0x6b6420`/`0x6b6630`) and magma/slime (`0x6b68f0`) kernels both place vertex `(i, j)` at
/// `base + (i·STEP, j·STEP)`.
const MLIQ_CELL_STEP: f32 = f32::from_bits(0x4085_5555);

/// Yards per water-texture repeat: one per grid cell, as the reference's water kernel `0x6b6630`
/// writes `u = (float)i`, `v = (float)j`. Deviation: the UV comes from the model-space position,
/// not each surface's own `(i, j)`, because the two agree on cell-aligned surfaces and the position
/// never seams where they are not. Magma and slime use this period too; the reference reads their
/// UV from the vertex's authored `int16` pair × 1/256 (`0x6b6993`), which is not parsed.
const MLIQ_UV_PERIOD: f32 = MLIQ_CELL_STEP;

/// A WMO water vertex's opacity-ramp coordinate, its authored byte 0 normalised: the reference
/// indexes a linear LUT between the zone's river shallow and deep alphas with it (`0x6b64c6`,
/// built by `0x6b6b60`), not the ADT `1.6·(i/63)^8` curve, and the shader's lerp reproduces it.
fn wmo_water_alpha_v(opacity_byte: u8) -> f32 {
    f32::from(opacity_byte) / 255.0
}

/// MOGP `groupLiquid` for a group with no liquid of its own; any other value is the whole-group
/// submersion override.
pub const NO_GROUP_LIQUID: u32 = 0xf;

/// A group's MOGP header fields. `flags` sits at `0x08` and the portal-ref slice at `0x24`/`0x26`,
/// which the reference copies to `grp+0x10` and `+0x2c`/`+0x30`.
#[derive(Debug, Clone, Copy)]
pub struct WmoGroupHeader {
    /// `0x8` is exterior, the bit the portal flood defers on; `& 0x48 == 0` is interior.
    pub flags: u32,
    /// First index of this group's portals in [`super::WmoPortals::refs`].
    pub portal_ref_start: u16,
    pub portal_ref_count: u16,
    /// MOGP `uniqueID` at `0x38`, the group's `WMOAreaTable.WMOGroupID` key; 0 for a short header.
    pub area_table_id: u32,
    /// MOGP fog indices at `0x30` into the root's [`super::WmoFog`] records, walked by the
    /// reference's interior-fog selector `0x69de20`; the offset is fixed empirically
    /// (`tests/wmo_fogs.rs`). Zero if the header is short.
    pub fog_indices: [u8; 4],
    /// MOGP `groupLiquid` at `0x34`: [`NO_GROUP_LIQUID`] on all but 13 of the 5220 shipped group
    /// files. Any other value makes the reference's `0x6b9f10` report the whole group submerged in
    /// that liquid type, height `FLT_MAX`, with no Z or MLIQ test; none of the 13 has an MLIQ. A
    /// short header reads as no liquid.
    pub group_liquid: u32,
}

/// Read a group file's [`WmoGroupHeader`]; the MOGP payload opens with the 68-byte header.
pub fn wmo_group_header(group_bytes: &[u8]) -> Option<WmoGroupHeader> {
    let mogp = find_wmo_chunk(group_bytes, b"PGOM")?; // MOGP
    if mogp.len() < 0x28 {
        return None;
    }
    let u32_at = |i: usize| {
        (mogp.len() >= i + 4)
            .then(|| u32::from_le_bytes([mogp[i], mogp[i + 1], mogp[i + 2], mogp[i + 3]]))
    };
    Some(WmoGroupHeader {
        flags: u32::from_le_bytes([mogp[8], mogp[9], mogp[10], mogp[11]]),
        portal_ref_start: u16::from_le_bytes([mogp[0x24], mogp[0x25]]),
        portal_ref_count: u16::from_le_bytes([mogp[0x26], mogp[0x27]]),
        area_table_id: u32_at(0x38).unwrap_or(0),
        fog_indices: if mogp.len() >= 0x34 {
            [mogp[0x30], mogp[0x31], mogp[0x32], mogp[0x33]]
        } else {
            [0; 4]
        },
        group_liquid: u32_at(0x34).unwrap_or(NO_GROUP_LIQUID),
    })
}

/// An interior group's render triangles, per-vertex MOCV and per-triangle MOPY: the faces a game
/// object's light attach (`0x6717d0` → `0x69e4c0`) down-rays, taking the hit's barycentric MOCV
/// (floor 168, cap 96) as its light.
#[derive(Clone)]
pub struct FootprintTris {
    /// Vertex positions, WMO model space.
    pub positions: Vec<[f32; 3]>,
    /// Triangle list in MOVI order; the MOPY vectors run parallel, one entry per triangle.
    pub indices: Vec<u16>,
    /// Per-vertex MOCV RGB, kept as bytes because the floor/cap arithmetic is exact u8 fixed-point.
    pub mocv: Vec<[u8; 3]>,
    /// Per-triangle MOPY flags; `0x1` on the hit face selects the day/night exterior colours
    /// instead of the MOCV sample (`0x6b9a50`).
    pub mopy_flags: Vec<u8>,
    /// Per-triangle MOPY material id, an index into the root's MOMT; the reference's footstep ray
    /// reads it off the hit face (`0x6a26fc`). The on-disk `0xFF` non-index occurs only on faces
    /// [`FOOTPRINT_REJECT`] drops; consumers still bounds-check.
    pub mopy_material: Vec<u8>,
}

/// The footprint ray's MOPY reject mask (the reference's BSP walk `0x6b92b0`/`0x6bc370`): a
/// collision (`0x08`) or visited (`0x80`) face is never a candidate, so of a floor's coplanar
/// render and collision sheets only the render one is sampled.
const FOOTPRINT_REJECT: u8 = 0x88;

/// A group's [`FootprintTris`], `None` unless it is an interior group with MOCV parallel to its
/// positions. Rejected faces drop at build: the set is static, so this equals the per-ray reject.
pub fn wmo_group_footprint_tris(group_bytes: &[u8]) -> Option<FootprintTris> {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut Cursor::new(group_bytes)) else {
        return None;
    };
    if group.flags & 0x48 != 0 || group.vertex_colors.len() != group.vertex_positions.len() {
        return None;
    }
    let mut indices = Vec::new();
    let mut mopy_flags = Vec::new();
    let mut mopy_material = Vec::new();
    for (ti, tri) in group.vertex_indices.as_chunks::<3>().0.iter().enumerate() {
        let flags = group.material_info.get(ti).map_or(0, |m| m.flags);
        if flags & FOOTPRINT_REJECT != 0 {
            continue;
        }
        indices.extend_from_slice(tri);
        mopy_flags.push(flags);
        mopy_material.push(group.material_info.get(ti).map_or(0xFF, |m| m.material_id));
    }
    Some(FootprintTris {
        positions: group
            .vertex_positions
            .iter()
            .map(|v| [v.x, v.y, v.z])
            .collect(),
        indices,
        mocv: group
            .vertex_colors
            .iter()
            .map(|c| [c.r, c.g, c.b])
            .collect(),
        mopy_flags,
        mopy_material,
    })
}

/// Find a MOGP sub-chunk by its reversed magic, past the `0x44`-byte header. The last one clamps to
/// the payload end as in [`find_wmo_chunk`], since a clamped MOGP leaves its last sub-chunk short.
fn find_mogp_subchunk<'a>(group_bytes: &'a [u8], magic: &[u8; 4]) -> Option<&'a [u8]> {
    let mogp = find_wmo_chunk(group_bytes, b"PGOM")?;
    let mut off = 0x44usize; // past the fixed group header
    while off + 8 <= mogp.len() {
        let size = u32::from_le_bytes([mogp[off + 4], mogp[off + 5], mogp[off + 6], mogp[off + 7]])
            as usize;
        let data_start = off + 8;
        let data_end = data_start.saturating_add(size).min(mogp.len());
        if &mogp[off..off + 4] == magic {
            return Some(&mogp[data_start..data_end]);
        }
        off = data_end;
    }
    None
}

/// A group's MODR list, the MODD indices it owns: the reference creates one doodad per (index,
/// group) from it (`0x695aa0`), lit by that group's interior class and MOLR lights.
pub fn wmo_group_doodad_refs(group_bytes: &[u8]) -> Vec<u16> {
    find_mogp_subchunk(group_bytes, b"RDOM")
        .map(|c| {
            c.as_chunks::<2>()
                .0
                .iter()
                .map(|r| u16::from_le_bytes([r[0], r[1]]))
                .collect()
        })
        .unwrap_or_default()
}

/// A group's MOLR list, the MOLT lights its doodads receive (`0x695c00`); a group without one
/// gives its doodads no point light at all.
pub fn wmo_group_light_refs(group_bytes: &[u8]) -> Vec<u16> {
    find_mogp_subchunk(group_bytes, b"RLOM")
        .map(|c| {
            c.as_chunks::<2>()
                .0
                .iter()
                .map(|r| u16::from_le_bytes([r[0], r[1]]))
                .collect()
        })
        .unwrap_or_default()
}

/// A group's MLIQ surface as a [`LiquidMesh`] in WMO model space, as the reference's dispatch
/// `0x6b62e0` draws it: vertex `(i, j)` at `base + (i·STEP, j·STEP, heights[j·xverts + i])`, a
/// tile drawn unless its low nibble is `0xf`, and one kind for the whole surface (`0x6ba970`).
pub fn wmo_group_liquid_mesh(group_bytes: &[u8]) -> Option<LiquidMesh> {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut Cursor::new(group_bytes)) else {
        return None;
    };
    let lq = group.liquid?;
    build_wmo_liquid_mesh(&lq, group.group_liquid)
}

/// The [`LiquidMesh`] for a parsed [`WmoLiquid`] grid and its group's `groupLiquid`.
fn build_wmo_liquid_mesh(lq: &WmoLiquid, group_liquid: u32) -> Option<LiquidMesh> {
    let (xv, yv, xt, yt) = (
        lq.xverts as usize,
        lq.yverts as usize,
        lq.xtiles as usize,
        lq.ytiles as usize,
    );
    if xv < 2 || yv < 2 || xt + 1 != xv || yt + 1 != yv {
        return None;
    }
    if lq.heights.len() < xv * yv || lq.tile_flags.len() < xt * yt {
        return None;
    }

    // One kind per surface: the `groupLiquid` override, else the first wet nibble (`0x6ba970`).
    let type_nibble = if group_liquid != NO_GROUP_LIQUID {
        (group_liquid & 0xf) as u8
    } else {
        let first_wet = lq.tile_flags.iter().map(|&f| f & 0xf).find(|&n| n != 0xf)?;
        first_wet
    };
    let kind = LiquidKind::from_nibble(type_nibble)?;
    // Fullbright kinds read no ramp and their vertex bytes are a UV pair: the channel stays 0.
    let water = !kind.is_fullbright();

    // Two triangles per wet tile, drawn two-sided; built first, as `drawn` decides the fill below.
    let mut indices = Vec::with_capacity(xt * yt * 6);
    let mut wet = vec![false; xt * yt];
    // `0x80`: a neighbour also claims the cell (gated model-wide in `LiquidMesh::shared`).
    let mut shared = vec![false; xt * yt];
    let mut drawn = vec![false; xv * yv];
    for ty in 0..yt {
        for tx in 0..xt {
            if lq.tile_flags[ty * xt + tx] & 0xf == 0xf {
                continue; // a hole
            }
            wet[ty * xt + tx] = true;
            shared[ty * xt + tx] = lq.tile_flags[ty * xt + tx] & 0x80 != 0;
            let tl = (ty * xv + tx) as u32;
            let tr = tl + 1;
            let bl = ((ty + 1) * xv + tx) as u32;
            let br = bl + 1;
            indices.extend_from_slice(&[tl, bl, br, tl, br, tr]);
            for v in [tl, tr, bl, br] {
                drawn[v as usize] = true;
            }
        }
    }
    if indices.is_empty() {
        return None; // grid present but every tile a hole
    }

    // An undrawn vertex takes the lowest drawn height, so it never stretches the AABB.
    let fill_z = (0..xv * yv)
        .filter(|&n| drawn[n] && lq.heights[n].is_finite())
        .map(|n| lq.heights[n])
        .fold(f32::INFINITY, f32::min);
    let fill_z = if fill_z.is_finite() {
        fill_z
    } else {
        lq.base[2]
    };

    // Every grid vertex is emitted, row-major, so the tile indices line up. An undrawn vertex's
    // authored height is not a height: shipped files leave hole interiors at 0.0 (Blackfathom g007
    // has 186 of 868, its water at z −58.28), which would stretch the AABB to z 0.
    let mut positions = Vec::with_capacity(xv * yv);
    let mut uvs = Vec::with_capacity(xv * yv);
    let mut depths = Vec::with_capacity(xv * yv);
    for j in 0..yv {
        for i in 0..xv {
            let n = j * xv + i;
            let mx = lq.base[0] + i as f32 * MLIQ_CELL_STEP;
            let my = lq.base[1] + j as f32 * MLIQ_CELL_STEP;
            let h = lq.heights[n];
            let z = if drawn[n] && h.is_finite() { h } else { fill_z };
            positions.push([mx, my, z]);
            depths.push(if water {
                wmo_water_alpha_v(lq.opacity.get(n).copied().unwrap_or(0))
            } else {
                0.0
            });
            // UV from the model-space position, one field for all surfaces (`MLIQ_UV_PERIOD`).
            uvs.push([mx / MLIQ_UV_PERIOD, my / MLIQ_UV_PERIOD]);
        }
    }
    Some(LiquidMesh {
        grid: [xv as u32, yv as u32],
        wet,
        shared,
        positions,
        uvs,
        depths,
        indices,
        // The resolved nibble is also the surface's ambient-sound key.
        sound_nibble: type_nibble,
        // The body colour's MOMT index; MOMT is in the root, so the spawner resolves it.
        material_id: Some(lq.material_id),
        kind,
    })
}

/// The bright-doorway fade's reach in yards (`[0x811110]`): a vertex farther from every exterior
/// portal keeps its bake.
const DOORWAY_FADE_REACH: f32 = 6.666_666_5;

/// The fade's per-yard slope, `t = 1 − 0.15·dist` (`[0x7ffd6c]`).
const DOORWAY_FADE_SLOPE: f64 = 0.15;

/// The distance kernel's plane epsilon in yards (`0x6c4240`): a vertex this close to a portal's
/// plane, inside its outline, is at distance 0 and whitens outright.
const PORTAL_PLANE_EPS: f32 = 1.0 / 6.0;

/// The reference's float-to-fixed bias (`[0x8029cc]`): the f32 sum's bit pattern `>> 14` is the
/// byte, never a numeric conversion.
const FIXED_BIAS: f64 = 512.0;

/// Distance from a point to a portal polygon, the reference's kernel `0x6c4240`: 0 inside the
/// outline within [`PORTAL_PLANE_EPS`] of the plane, else the nearest edge distance. The outline
/// test matters: some portals are whole ceilings (Dire Maul's courtyard, 283×154 yd).
fn portal_distance(p: [f32; 3], poly: &[[f32; 3]], plane: [f32; 4]) -> f32 {
    let n = [plane[0], plane[1], plane[2]];
    let pd = n[0] * p[0] + n[1] * p[1] + n[2] * p[2] + plane[3];
    let mut best = f32::MAX;
    let mut inside = pd * pd < PORTAL_PLANE_EPS * PORTAL_PLANE_EPS;
    let proj = [p[0] - n[0] * pd, p[1] - n[1] * pd, p[2] - n[2] * pd];
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let e = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        // Outward edge test about the plane normal: a projection left of every edge is inside.
        let out = [
            e[1] * n[2] - e[2] * n[1],
            e[2] * n[0] - e[0] * n[2],
            e[0] * n[1] - e[1] * n[0],
        ];
        let w = [proj[0] - a[0], proj[1] - a[1], proj[2] - a[2]];
        if out[0] * w[0] + out[1] * w[1] + out[2] * w[2] > 0.0 {
            inside = false;
        }
        let wp = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
        let len2 = e[0] * e[0] + e[1] * e[1] + e[2] * e[2];
        let t = if len2 > 0.0 {
            ((wp[0] * e[0] + wp[1] * e[1] + wp[2] * e[2]) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let d = [wp[0] - e[0] * t, wp[1] - e[1] * t, wp[2] - e[2] * t];
        best = best.min((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt());
    }
    if inside {
        0.0
    } else {
        best
    }
}

/// One byte of the white-lerp (`0x6c43d0`): the bits of `(255 − ch)·t + ch + 512` as f32, `>> 14`.
fn white_lerp_byte(ch: u8, t: f64) -> u8 {
    let chf = f64::from(ch);
    let s = ((255.0 - chf) * t + chf + FIXED_BIAS) as f32;
    ((s.to_bits() >> 14) & 0xff) as u8
}

/// The reference's bright-doorway fade, FixColorVertexAlpha (`0x6c43d0`), run once per group in
/// place over its MOCV before any draw; transition corridors are authored near-black and rely on
/// it. Per vertex, `dist` is the nearest [`portal_distance`] over portals to exterior neighbours
/// (`MOGI.flags & 0x48 != 0`): at 0 the vertex goes white-opaque, under [`DOORWAY_FADE_REACH`] with
/// alpha 0 it is white-lerped by `t = 1 − 0.15·dist` with alpha `t·255`, else it is untouched. The
/// MOPY pre-pass `0x6c41e0` is gated (`DAT_00ca8064`) and a reference capture shows it unrun, so it
/// is not run here.
fn fix_color_vertex_alpha(
    colors: &mut [Color],
    group: &WmoGroup,
    root: &WmoRoot,
    header_refs: (u16, u16),
) {
    let portals = root.portals();
    let infos = root.group_infos();
    let (start, count) = (header_refs.0 as usize, header_refs.1 as usize);
    // This group's portals whose far side is an exterior group.
    let doors: Vec<(Vec<[f32; 3]>, [f32; 4])> = portals
        .refs
        .get(start..start + count)
        .unwrap_or(&[])
        .iter()
        .filter(|r| infos.get(r.group as usize).is_some_and(|g| !g.interior))
        .filter_map(|r| portals.infos.get(r.portal as usize))
        .map(|info| {
            let v = (0..info.count as usize)
                .filter_map(|k| {
                    portals
                        .vertices
                        .get(info.start_vertex as usize + k)
                        .copied()
                })
                .collect();
            (v, info.plane)
        })
        .filter(|(v, _): &(Vec<[f32; 3]>, _)| v.len() >= 3)
        .collect();
    if doors.is_empty() {
        return;
    }
    for (i, c) in colors.iter_mut().enumerate() {
        let Some(p) = group.vertex_positions.get(i) else {
            continue;
        };
        let p = [p.x, p.y, p.z];
        let dist = doors
            .iter()
            .map(|(poly, plane)| portal_distance(p, poly, *plane))
            .fold(f32::MAX, f32::min);
        if dist == 0.0 {
            *c = Color {
                b: 0xff,
                g: 0xff,
                r: 0xff,
                a: 0xff,
            };
        } else if dist < DOORWAY_FADE_REACH && c.a == 0 {
            let t = 1.0 - f64::from(dist) * DOORWAY_FADE_SLOPE;
            c.b = white_lerp_byte(c.b, t);
            c.g = white_lerp_byte(c.g, t);
            c.r = white_lerp_byte(c.r, t);
            c.a = ((((t * 255.0) + FIXED_BIAS) as f32).to_bits() >> 14 & 0xff) as u8;
        }
    }
}

/// The group's MOCV as the renderer uploads it, BGRA, with the doorway fade applied.
pub fn wmo_group_fixed_colors(group_bytes: &[u8], root: &WmoRoot) -> Option<Vec<[u8; 4]>> {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut Cursor::new(group_bytes)) else {
        return None;
    };
    let colors = fixed_colors(&group, group_bytes, root)?;
    Some(colors.iter().map(|c| [c.b, c.g, c.r, c.a]).collect())
}

/// The group's MOCV as authored, BGRA.
pub fn wmo_group_raw_colors(group_bytes: &[u8]) -> Option<Vec<[u8; 4]>> {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut Cursor::new(group_bytes)) else {
        return None;
    };
    let colors = parallel_colors(&group)?;
    Some(colors.iter().map(|c| [c.b, c.g, c.r, c.a]).collect())
}

/// The group's MOCV parallel to its positions. A MOCV clamped at EOF comes back one colour short
/// (`Undercity_144.wmo` holds 1159 of 1160 bytes) and is padded from the last colour; a larger
/// shortfall is a broken bake and reads `None`.
fn parallel_colors(group: &WmoGroup) -> Option<Vec<Color>> {
    let (have, want) = (group.vertex_colors.len(), group.vertex_positions.len());
    if have == want {
        return Some(group.vertex_colors.clone());
    }
    let last = (have + 1 == want).then(|| group.vertex_colors.last().copied())??;
    let mut colors = group.vertex_colors.clone();
    colors.push(last);
    Some(colors)
}

/// The authored MOCV with the doorway fade run over it.
fn fixed_colors(group: &WmoGroup, group_bytes: &[u8], root: &WmoRoot) -> Option<Vec<Color>> {
    let mut colors = parallel_colors(group)?;
    let refs =
        wmo_group_header(group_bytes).map_or((0, 0), |h| (h.portal_ref_start, h.portal_ref_count));
    fix_color_vertex_alpha(&mut colors, group, root, refs);
    Some(colors)
}

/// Build one group's render submeshes against `root`; empty if the bytes are not a group.
pub fn wmo_group_submeshes(group_bytes: &[u8], root: &WmoRoot) -> Result<Vec<RenderSubmesh>> {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut Cursor::new(group_bytes)) else {
        return Ok(Vec::new());
    };
    // The reference fades once per group, gated on its `+0xbc` done-byte (`0x6b3f90`); a group
    // loads once, so the load is the gate.
    let colors = fixed_colors(&group, group_bytes, root).unwrap_or_default();
    let ParsedWmo::Root(root) = &root.parsed else {
        anyhow::bail!("WmoRoot is not a root");
    };
    // MOGP `+0x28`/`+0x2a`: MOBA batches run TRANS, then INT, then EXT, the class deciding an
    // interior group's lighting; a short header reads all EXT.
    let (trans_n, int_n) = find_wmo_chunk(group_bytes, b"PGOM")
        .filter(|m| m.len() >= 0x2c)
        .map_or((0usize, 0usize), |m| {
            (
                u16::from_le_bytes([m[0x28], m[0x29]]) as usize,
                u16::from_le_bytes([m[0x2a], m[0x2b]]) as usize,
            )
        });
    let mut out = Vec::new();
    {
        let has_normals = group.vertex_normals.len() == group.vertex_positions.len();
        let has_colors = colors.len() == group.vertex_positions.len();
        // Neither exterior (`0x8`) nor exterior-lit (`0x40`): the reference's interior test.
        let interior = (group.flags & 0x48) == 0;
        let vertex = |g: u32| {
            let p = &group.vertex_positions[g as usize];
            let n = group
                .vertex_normals
                .get(g as usize)
                .map_or([0.0, 1.0, 0.0], |n| [n.x, n.y, n.z]);
            let uv = group
                .texture_coords
                .get(g as usize)
                .map_or([0.0, 0.0], |t| [t.u, t.v]);
            // MOCV RGB is the shade multiplier; the alpha rides along and is forced opaque below
            // where it is not lighting data. No MOCV is white.
            let c = colors.get(g as usize).map_or([1.0, 1.0, 1.0, 1.0], |c| {
                [
                    c.r as f32 / 255.0,
                    c.g as f32 / 255.0,
                    c.b as f32 / 255.0,
                    c.a as f32 / 255.0,
                ]
            });
            ([p.x, p.y, p.z], n, uv, c)
        };
        for (bi, batch) in group.render_batches.iter().enumerate() {
            let class = if bi < trans_n {
                WmoBatchClass::Trans
            } else if bi < trans_n + int_n {
                WmoBatchClass::Int
            } else {
                WmoBatchClass::Ext
            };
            let start = batch.start_index as usize;
            let global_indices: Vec<u32> = group
                .vertex_indices
                .get(start..start + batch.count as usize)
                .unwrap_or(&[])
                .iter()
                .map(|&i| u32::from(i))
                .filter(|&g| (g as usize) < group.vertex_positions.len())
                .collect();
            if global_indices.is_empty() {
                continue;
            }
            let material = root.materials.get(batch.material_id as usize);
            let texture = material
                .map(|m| m.get_texture1_index(&root.texture_offset_index_map))
                .and_then(|i| root.textures.get(i as usize))
                .cloned()
                .filter(|s| !s.is_empty());
            let blend = match material.map(|m| m.blend_mode) {
                Some(0) | None => ModelBlend::Opaque,
                Some(1) => ModelBlend::AlphaTest,
                // `MOMT.blendMode` indexes EGxBlend directly, no M2 remap (`0x6b500f`): 4 is Mod
                // (DST_COLOR/ZERO), 5 Mod2x (DST_COLOR/SRC_COLOR).
                Some(4) => ModelBlend::Mod,
                Some(5) => ModelBlend::Mod2x,
                Some(_) => ModelBlend::Blend,
            };
            // MOMT flags (drawers `0x6b4f10`/`0x6b5190`, SIDN updater `0x6b4090`): UNLIT `0x01`
            // draws an exterior batch fullbright and is ignored by the interior drawer; SIDN `0x10`
            // adds the night-glow colour inside the lit sum; WINDOW `0x20` gives an interior batch
            // the brighter midpoint light.
            let flags = material.map_or(0, |m| m.flags);
            let emissive = !interior && flags & 0x01 != 0;
            // UNCULLED `0x04`: without it the reference back-face culls the batch (`0x6b4fd7`,
            // `0x6b52bf`). Two-sided cloth is authored as the same faces twice with reversed
            // winding, so drawing both sides z-fights. A WMO has no skeleton: the skin is unused.
            let (mut submesh, _globals) = remap_submesh(
                global_indices.into_iter(),
                vertex,
                texture,
                blend,
                flags & 0x04 != 0,
                interior,
                emissive,
            );
            submesh.wmo_batch = Some(class);
            submesh.sidn = (flags & 0x10 != 0)
                .then(|| material.map(|m| m.sidn_rgb))
                .flatten();
            submesh.window = flags & 0x20 != 0;
            if !has_normals {
                submesh.normals.clear(); // no MONR: the renderer computes flat normals
            }
            if !has_colors {
                submesh.vertex_colors.clear(); // no MOCV: untinted
            }
            // MOCV alpha is lighting data on interior batches: TRANS reads it as the lit-to-bake
            // lerp factor, INT as the self-illumination mask (the reference's
            // `tex·MOCV·(1 + 4·a)`). Elsewhere it is blend state, forced opaque.
            if !(interior && matches!(class, WmoBatchClass::Trans | WmoBatchClass::Int)) {
                for c in &mut submesh.vertex_colors {
                    c[3] = 1.0;
                }
            }
            out.push(submesh);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::test_bytes::chunk;
    use super::*;

    #[test]
    fn builds_wmo_water_mesh_skipping_holes() {
        // A 3×2-vertex grid (2×1 tiles): left tile wet (lake_a, nibble 4), right tile a hole (0xf).
        let lq = WmoLiquid {
            xverts: 3,
            yverts: 2,
            xtiles: 2,
            ytiles: 1,
            base: [100.0, 200.0, -3.0],
            material_id: 0,
            heights: vec![-3.0; 6],
            // Byte 0 per vertex, the authored opacity index.
            opacity: vec![0, 51, 102, 153, 204, 255],
            tile_flags: vec![0x44, 0x0f], // wet (type 4 + fishable), hole
        };
        let m = build_wmo_liquid_mesh(&lq, 0xf).expect("a wet mesh");
        assert_eq!(m.kind, LiquidKind::Still);
        assert_eq!(m.positions.len(), 6); // full 3×2 grid emitted
        assert_eq!(m.indices.len(), 6); // one wet tile → 2 tris → 6 indices
        assert!((m.positions[1][0] - (100.0 + MLIQ_CELL_STEP)).abs() < 1e-3);
        assert_eq!(m.positions[0], [100.0, 200.0, -3.0]);
        // Opacity is the vertex's own authored byte, normalised.
        for (n, &v) in m.depths.iter().enumerate() {
            assert!(
                (v - f32::from(lq.opacity[n]) / 255.0).abs() < 1e-6,
                "vertex {n}: depth channel {v} should carry byte {}/255",
                lq.opacity[n]
            );
        }
        // The wet tile is the left one, corners 0, 1, 3 and 4.
        assert!(m.indices.iter().all(|&i| [0u32, 1, 3, 4].contains(&i)));
    }

    /// Surface B starts at A's last column (2·STEP east), so their shared vertex must get one UV.
    #[test]
    fn adjacent_surfaces_share_a_continuous_uv_field() {
        let base = [100.0, 200.0, -3.0];
        let a = WmoLiquid {
            xverts: 3,
            yverts: 2,
            xtiles: 2,
            ytiles: 1,
            base,
            material_id: 0,
            heights: vec![-3.0; 6],
            opacity: Vec::new(),
            tile_flags: vec![0x44, 0x44], // both tiles wet
        };
        let b = WmoLiquid {
            xverts: 3,
            yverts: 2,
            xtiles: 2,
            ytiles: 1,
            base: [base[0] + 2.0 * MLIQ_CELL_STEP, base[1], base[2]],
            material_id: 0,
            heights: vec![-3.0; 6],
            opacity: Vec::new(),
            tile_flags: vec![0x44, 0x44],
        };
        let ma = build_wmo_liquid_mesh(&a, 0xf).unwrap();
        let mb = build_wmo_liquid_mesh(&b, 0xf).unwrap();
        // A's vertex (2, 0) and B's (0, 0) are the same point.
        let (ua, ub) = (ma.uvs[2], mb.uvs[0]);
        assert!(
            (ua[0] - ub[0]).abs() < 1e-4 && (ua[1] - ub[1]).abs() < 1e-4,
            "seam: shared-vertex UVs differ A={ua:?} B={ub:?}"
        );
        // One repeat per cell, the reference's `u = (float)i` (`0x6b6630`).
        assert!((ma.uvs[1][0] - ma.uvs[0][0] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn wmo_liquid_type_override_and_all_holes() {
        // groupLiquid override = 2 (magma) forces the kind regardless of the wet tile's nibble.
        let lq = WmoLiquid {
            xverts: 2,
            yverts: 2,
            xtiles: 1,
            ytiles: 1,
            base: [0.0, 0.0, 0.0],
            material_id: 0,
            heights: vec![0.0; 4],
            opacity: Vec::new(),
            tile_flags: vec![0x40], // nibble 0 (would be Still), but the override wins
        };
        let m = build_wmo_liquid_mesh(&lq, 2).expect("magma mesh");
        assert_eq!(m.kind, LiquidKind::Magma);
        assert!(m.depths.iter().all(|&v| v == 0.0)); // fullbright ⇒ depth V unused

        let all_holes = WmoLiquid {
            tile_flags: vec![0x0f],
            ..lq
        };
        assert!(build_wmo_liquid_mesh(&all_holes, 0xf).is_none());
    }

    #[test]
    fn reads_group_header_flags_and_portal_ref_span() {
        let mut hdr = vec![0u8; 68];
        hdr[8..12].copy_from_slice(&0x8u32.to_le_bytes()); // EXTERIOR
        hdr[0x24..0x26].copy_from_slice(&5u16.to_le_bytes());
        hdr[0x26..0x28].copy_from_slice(&3u16.to_le_bytes());
        hdr[0x30..0x34].copy_from_slice(&[2, 0, 0, 0]); // fog indices
        let mut group = Vec::new();
        group.extend(chunk(b"REVM", &17u32.to_le_bytes()));
        group.extend(chunk(b"PGOM", &hdr));

        let h = wmo_group_header(&group).expect("group header");
        assert_eq!(h.flags, 0x8);
        assert_eq!(h.portal_ref_start, 5);
        assert_eq!(h.portal_ref_count, 3);
        assert_eq!(h.fog_indices, [2, 0, 0, 0]);
        assert_eq!(
            h.group_liquid, 0,
            "0x34 is read raw: a zeroed header says liquid type 0, NOT the 0xf sentinel — the \
             reader must never conflate 'no liquid' with 'this whole group is water'"
        );
    }

    /// All 13 shipped overrides are `0`, water, so the no-liquid sentinel cannot be zero.
    #[test]
    fn reads_the_whole_group_liquid_override() {
        let group_with = |liquid: u32| {
            let mut hdr = vec![0u8; 68];
            hdr[0x34..0x38].copy_from_slice(&liquid.to_le_bytes());
            let mut group = Vec::new();
            group.extend(chunk(b"REVM", &17u32.to_le_bytes()));
            group.extend(chunk(b"PGOM", &hdr));
            group
        };

        // The shipped case: the whole group is water, with no MLIQ.
        let flooded = wmo_group_header(&group_with(0)).expect("group header");
        assert_eq!(flooded.group_liquid, 0);
        assert_ne!(flooded.group_liquid, NO_GROUP_LIQUID);

        // The other 5207 groups.
        assert_eq!(
            wmo_group_header(&group_with(NO_GROUP_LIQUID))
                .expect("group header")
                .group_liquid,
            NO_GROUP_LIQUID
        );

        // A header short of 0x34 reads as no liquid.
        let mut short = Vec::new();
        short.extend(chunk(b"REVM", &17u32.to_le_bytes()));
        short.extend(chunk(b"PGOM", &[0u8; 0x30]));
        assert_eq!(
            wmo_group_header(&short)
                .expect("a short header still parses")
                .group_liquid,
            NO_GROUP_LIQUID
        );
    }
}
