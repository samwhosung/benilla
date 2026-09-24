//! WMO **group**-file parsing: the MOGP header + sub-chunks (doodad/light refs, footprint faces)
//! and the render-batch → [`RenderSubmesh`] build against a parsed root.

use std::io::Cursor;

use anyhow::Result;
use benilla_wmo::{parse_wmo, Color, ParsedWmo, WmoGroup, WmoLiquid};

use super::{find_wmo_chunk, WmoRoot};
use crate::liquid::{LiquidKind, LiquidMesh};
use crate::models::{remap_submesh, ModelBlend, RenderSubmesh, WmoBatchClass};

/// The MLIQ grid vertex spacing (world yards between adjacent grid vertices) — the reference's
/// `[0x810cc0]` = `4.166666507720947` (`0x40855555`). The liquid render dispatch `0x6b62e0`
/// branches on the type nibble: **water** → `0x6b6420`/`0x6b6630`, magma/slime → `0x6b68f0`
/// (`0x6b68f0` is the magma path, not a water kernel). Either kernel places grid vertex `(i, j)`
/// at `base + (i·STEP, j·STEP)`.
const MLIQ_CELL_STEP: f32 = f32::from_bits(0x4085_5555);

/// Yards per water-texture repeat: **one repeat per grid cell**, `MLIQ_CELL_STEP` ≈ 4.167 yd.
///
/// The reference's water kernel `0x6b6630` writes `u = (float)i`, `v = (float)j` — the raw integer
/// tile indices, from loop counters that both start at a literal 0. One repeat per cell means
/// every surface boundary lands on an exact repeat boundary, so a neighbour restarting at 0 is
/// invisible: the seam-free look is the *scale*.
///
/// **This used to be `4 ×` that**, i.e. every WMO water surface in the game carried a texture four
/// times too large, on a mechanism that was later refuted — seamlessness was credited to a
/// world-anchored `GL_TEXTURE` matrix at transform index 8, but index 8 is the world/modelview
/// transform and the water arms push no texture matrix at all. Decision 1271 carried
/// the correction and deliberately did not apply it, the look being the director's call; B136 and
/// the director's "Stormwind water seems too rough on the surface" are that call. The 4× cost more
/// than size: it is exactly **two mip levels**, so the surface sat on mip 0 across the whole near
/// field and never reached the authored chain that drops the sheet's peak ripple alpha 255 → 136 →
/// 68 → a flat 51. Measured at the Stormwind canal before the fix, near water and mid-distance water
/// had the same ripple contrast (σ 15.9 vs 15.2) — the ripple was not attenuating with distance at
/// all.
///
/// We still anchor each vertex's UV to its **model-space position** rather than emitting the raw
/// `(i, j)`. At one repeat per cell the two agree wherever surfaces are cell-aligned (they differ by
/// a whole number of repeats, which `Repeat` wrapping erases), and where they are *not* aligned the
/// position anchor is the one that does not seam.
///
/// Magma/slime have a separate correction still pending: the reference reads their `st` from the
/// MLIQ vertex's authored `int16` pair × `1/256` (`movsx`, `0x6b6993`), which we do not parse at all
/// and generate over this period instead.
const MLIQ_UV_PERIOD: f32 = MLIQ_CELL_STEP;

/// The **opacity ramp coordinate** fed to a WMO water vertex: its authored byte, normalised.
///
/// WMO liquid carries no bathymetry, so there is nothing to ramp by depth the way ADT MCLQ water is
/// ramped — and the reference does not try. It indexes a 256-entry alpha ramp with the vertex's own
/// byte 0 (`0x6b64c6 movzx`), and that ramp is a plain **linear** lerp between the zone's river
/// shallow and deep alphas: `LUT[i] = (c0·256 + i·(c1 − c0)) >> 8`, rebuilt per frame, which for the
/// static-init endpoints (0.5, 1.0) runs 127 → 191 → 254. **Not** the ADT path's
/// `1.6·(i/63)^8` curve; the two systems do not even share a delivery mechanism (the ADT ramp is
/// uploaded as a GPU texture and never CPU-indexed). The builder is `0x6b6b60`.
///
/// So `byte / 255` is exactly the lerp coordinate our shader already applies to the shallow/deep
/// alpha pair off the shared light buffer, and passing it through the existing per-vertex channel
/// reproduces the reference's LUT without a second mechanism.
///
/// This replaces a hard `1.0` — the deep, fully-opaque endpoint, pinned for every WMO pool in the
/// game because we believed those bytes were flow data. That pin is why Blackfathom's Pool of
/// Ask'ar rendered as a sheet you cannot see through where the real client shows 9–15 yd of bottom
/// (B136), and it was documented as "tunable; the exact shore LUT is deferred".
fn wmo_water_alpha_v(opacity_byte: u8) -> f32 {
    f32::from(opacity_byte) / 255.0
}

/// MOGP `groupLiquid`'s sentinel: **this group holds no liquid of its own**. Any other value is the
/// whole-group submersion override — see [`WmoGroupHeader::group_liquid`].
pub const NO_GROUP_LIQUID: u32 = 0xf;

/// The per-group fields the visibility pass reads from a **group file**'s MOGP header: the group
/// `flags` (offset `0x08`) and the group's slice into [`super::WmoPortals::refs`]
/// (`portal_ref_start` @ `0x24`, `portal_ref_count` @ `0x26`). VERIFIED layout (`grp+0x10` flags,
/// `+0x2c`/`+0x30` ref span seeded from MOGP `+0x24`/`+0x26`). `None` if the bytes aren't a
/// parseable group.
#[derive(Debug, Clone, Copy)]
pub struct WmoGroupHeader {
    /// MOGP group flags. `& 0x8` = EXTERIOR (the outdoor bit the flood defers on); `& 0x48 == 0` =
    /// a true interior group (see [`super::WmoGroupInfo`]).
    pub flags: u32,
    /// First index into [`super::WmoPortals::refs`] for this group's portals.
    pub portal_ref_start: u16,
    /// Number of portal refs for this group.
    pub portal_ref_count: u16,
    /// MOGP `uniqueID` @ `0x38` — this group's `WMOAreaTable.WMOGroupID` key (byte-verified
    /// 2026-07-03: Chapel_000 interior → 478, matching its WMOAreaTable CAVE/ambience row).
    /// `0` when the header is too short to carry it.
    pub area_table_id: u32,
    /// MOGP fog indices @ `0x30` — four indices into the root's [`super::WmoFog`] table, walked by
    /// the client's interior-fog selector (`0x69de20`). Offset pinned empirically against the
    /// Goldshire inn (its groups carry `[1,0,0,0]` / `[0,0,0,0]` here — valid indices into its
    /// 2-record MFOG — while `uniqueID @0x38` and the no-liquid sentinel `@0x34` self-validate the
    /// layout; see `tests/wmo_fogs.rs`).
    /// `[0; 4]` when the header is too short to carry them.
    pub fog_indices: [u8; 4],
    /// MOGP `groupLiquid` @ `0x34` — and `0xf` on all but **13 of the 5220 shipped group files**,
    /// where it means "this group holds no liquid of its own".
    ///
    /// The other value is the client's **whole-group submersion override**: `0x6b9f10` reads it
    /// first and, when it is not `0xf`, returns an unconditional hit — the raw value as the liquid
    /// type, height `FLT_MAX`, **no Z compare and no MLIQ test at all**. That is how a flooded
    /// tunnel or an underwater cave reads wet with no liquid grid in the file, and all 13 carry no
    /// `MLIQ` chunk (census with our own reader).
    ///
    /// [`super::wmo_group_liquid_mesh`] already reads the same word for a different purpose — it
    /// overrides the *kind* of a grid that exists. This field is the other half: the groups where
    /// there is no grid to override.
    ///
    /// `0xf` when the header is too short to carry it, which reads as "no liquid" — the safe way
    /// round, since the override only ever adds water.
    pub group_liquid: u32,
}

/// Read one WMO **group** file's MOGP header fields used by the visibility pass (see
/// [`WmoGroupHeader`]). The MOGP super-chunk's payload begins with the 68-byte group header.
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

/// One interior WMO group's raw render triangles + per-vertex baked MOCV + per-triangle MOPY flags —
/// the face set the interior FOOTPRINT sample walks. Its live consumer is the **GameObject M2**:
/// the entity light node's env-update attach (`0x6717d0` → `0x69e4c0`, the entity twin of the
/// ADT-MDDF `0x6a8410`) down-rays the group RENDER mesh (`MOVT`/`MOVI`/`MOPY`) under the object and
/// bakes the hit's barycentric MOCV — floor-168/cap-96 — as its committed light (decisive in a
/// reference trace of the abbey INNBENCH draws: diffuse max exactly 168).
/// `None` for a non-group, an exterior group, or a group without baked colours (never a footprint
/// source). (First built for decision 0290's MODD reading, deleted by 0306 when MODD props turned
/// out to use their own baked colour — resurrected now that the chain's true consumer is pinned.)
#[derive(Clone)]
pub struct FootprintTris {
    /// Vertex positions, WMO model space (WoW axes).
    pub positions: Vec<[f32; 3]>,
    /// Triangle list into [`Self::positions`] (MOVI order — MOPY is parallel per triangle).
    pub indices: Vec<u16>,
    /// Per-vertex baked MOCV as RGB bytes, parallel to positions (bytes, not 0–1: the floor/cap
    /// arithmetic is exact u8 fixed-point).
    pub mocv: Vec<[u8; 3]>,
    /// Per-TRIANGLE MOPY flags (`indices.len() / 3` entries). Bit `0x1` on the hit face selects the
    /// day/night exterior colours as the object's base instead of the MOCV sample (the binary
    /// selector in `0x6b9a50`).
    pub mopy_flags: Vec<u8>,
    /// Per-TRIANGLE MOPY **material id** (parallel to [`Self::mopy_flags`]) — the index into the
    /// ROOT's MOMT, and through it the face's `TerrainType` (`WmoRoot::material_ground_types`).
    /// This is the footstep surface's WMO leg: the client's material ray reads exactly this byte
    /// off the hit face (`0x6a26fc`).
    ///
    /// `0xFF` occurs on disk and is not an index — those faces all carry MOPY flag `0x08` and are
    /// already dropped by [`FOOTPRINT_REJECT`] before anything can sample them. Consumers still
    /// bounds-check: the client does not, the shipped assets simply never break the invariant.
    pub mopy_material: Vec<u8>,
}

/// The footprint ray's per-face MOPY reject mask (the BSP
/// face walk `0x6b92b0`/`0x6bc370`): a face with COLLISION `0x08` or VISITED `0x80` set is never a
/// footprint candidate. This is what keeps a floor authored as coplanar layers (a RENDER|DETAIL
/// sheet over a COLLISION sheet, observed on the Goldshire forge + inn) from being a per-position
/// coin toss: only the render layer survives, so the sample is deterministic and regional.
const FOOTPRINT_REJECT: u8 = 0x88;

/// Parse one WMO **group**'s bytes into its footprint-sample face set ([`FootprintTris`]).
/// `None` unless the bytes are a group, the group is INTERIOR (`flags & 0x48 == 0`), and it carries
/// MOCV parallel to its positions. Faces matching [`FOOTPRINT_REJECT`] are dropped here (the
/// client rejects them per-ray inside the BSP walk; the set is static, so filtering at build is
/// equivalent and free).
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

/// Find a sub-chunk of a group file's **MOGP** super-chunk by on-disk (reversed) magic. The MOGP
/// payload is the 0x44-byte group header followed by ordinary `[magic][size][data]` sub-chunks
/// (MOPY/MOVT/…/MODR/MOLR); this walks them.
///
/// Clamps the last sub-chunk to the payload end for the same reason [`find_wmo_chunk`] does — and it
/// is the same file: once MOGP itself is clamped to EOF, the sub-chunk that ran to the old declared
/// end is now the short one, so a break here would just move `Undercity_144`'s loss one level down.
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

/// A group's **MODR** doodad references: the MODD indices this group instantiates. This is the
/// faithful doodad→group OWNERSHIP relation — the real client creates a WMO doodad per (MODD index,
/// referencing group) from exactly this list (`0x695aa0` loops `group+0xe8`/count `+0x144`), and the
/// owning group's interior class + MOLR light list decide the doodad's lighting. Empty when the
/// group has no MODR (no doodads).
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

/// A group's **MOLR** light references: the MOLT indices whose lights this group folds into its own
/// doodads' committed light (`0x695c00` builds the per-group active-light list from exactly this; a
/// group WITHOUT an MOLR chunk provably lights its doodads with nothing — the abbey's group 3 has
/// none, and the director's stand receives zero point light, its own flame 3.7 yd away included).
/// Empty when the group has no MOLR.
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

/// Build one WMO **group**'s MLIQ liquid surface as a [`LiquidMesh`] in **WMO model space** (WoW
/// axes, yards) — the flat animated water/lava/slime the group embeds (Stormwind's canals, the
/// Ironforge lava, dungeon pools). `None` when the group carries no `MLIQ`, or the grid has no wet
/// tiles / an unmapped type. The placement transform lifts the mesh into the world at spawn.
///
/// Faithful model (the render dispatch is `0x6b62e0`, water kernels `0x6b6420`/`0x6b6630`): grid
/// vertex `(i, j)` sits at `base + (i·STEP, j·STEP, heights[j·xverts + i])`; a
/// tile renders iff its flag low nibble `!= 0xf` (`0xf` = hole); the type is the group's
/// `groupLiquid` override when set, else the per-tile nibble, mapped to a texture via
/// [`LiquidKind::from_nibble`]. The whole surface takes ONE resolved kind (the reference resolves a
/// single type per surface — `0x6ba970`), with per-tile hole membership.
pub fn wmo_group_liquid_mesh(group_bytes: &[u8]) -> Option<LiquidMesh> {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut Cursor::new(group_bytes)) else {
        return None;
    };
    let lq = group.liquid?;
    build_wmo_liquid_mesh(&lq, group.group_liquid)
}

/// Build the [`LiquidMesh`] for a parsed [`WmoLiquid`] grid + its group's `groupLiquid` override.
/// Split out so a unit test can exercise it without synthesising a full group file.
fn build_wmo_liquid_mesh(lq: &WmoLiquid, group_liquid: u32) -> Option<LiquidMesh> {
    let (xv, yv, xt, yt) = (
        lq.xverts as usize,
        lq.yverts as usize,
        lq.xtiles as usize,
        lq.ytiles as usize,
    );
    // The grid must be tile-consistent (`xtiles = xverts − 1`) and carry its full vertex/tile arrays.
    if xv < 2 || yv < 2 || xt + 1 != xv || yt + 1 != yv {
        return None;
    }
    if lq.heights.len() < xv * yv || lq.tile_flags.len() < xt * yt {
        return None;
    }

    // Resolve ONE kind for the whole surface: the `groupLiquid` override when set (`!= 0xf`), else
    // the first wet tile's nibble (`0x6ba970` returns the first non-hole nibble). Unmapped ⇒ None.
    let type_nibble = if group_liquid != NO_GROUP_LIQUID {
        (group_liquid & 0xf) as u8
    } else {
        let first_wet = lq.tile_flags.iter().map(|&f| f & 0xf).find(|&n| n != 0xf)?;
        first_wet
    };
    let kind = LiquidKind::from_nibble(type_nibble)?;
    // Fullbright kinds read no ramp at all (their sheet IS the body), and their leading four vertex
    // bytes are an authored int16 UV pair rather than an opacity — so the byte is meaningless there
    // and the channel stays 0, a statement rather than a value.
    let water = !kind.is_fullbright();

    // 2 tris per wet tile (low nibble `!= 0xf`). Winding is irrelevant (drawn two-sided). Built
    // BEFORE the vertices because which vertices the triangles actually reference is what decides
    // the substitution below.
    let mut indices = Vec::with_capacity(xt * yt * 6);
    let mut wet = vec![false; xt * yt];
    // The `0x80` "a neighbour also claims this cell" bit — kept per cell, gated model-wide later
    // (see `LiquidMesh::shared`).
    let mut shared = vec![false; xt * yt];
    let mut drawn = vec![false; xv * yv];
    for ty in 0..yt {
        for tx in 0..xt {
            if lq.tile_flags[ty * xt + tx] & 0xf == 0xf {
                continue; // hole — no liquid on this tile
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

    // The height every **unreferenced** vertex is filled with — see the loop below. The lowest
    // drawn height, so the substitute can never push the AABB past the geometry that is really
    // there. `indices` is non-empty by the check above, so at least one drawn height exists.
    let fill_z = (0..xv * yv)
        .filter(|&n| drawn[n] && lq.heights[n].is_finite())
        .map(|n| lq.heights[n])
        .fold(f32::INFINITY, f32::min);
    let fill_z = if fill_z.is_finite() {
        fill_z
    } else {
        lq.base[2]
    };

    // Grid vertices (row-major `j·xverts + i`), WMO model space. Every vertex is emitted — hole
    // corners included — so the tile indices line up.
    //
    // **A hole corner's authored height is not a height.** MLIQ carries a full `xverts × yverts`
    // height array whether or not a tile is wet, and the shipped files leave the hole interiors at
    // a literal `0.0`: Blackfathom g007 has 186 of 868 (its water sits at model z −58.28) and even
    // Stormwind's canal g099 has 36 of 108 (water at −6.48). Emitting those verbatim is invisible in
    // the draw — no triangle references them — but it stretches the Bevy mesh AABB from the flat
    // sheet all the way to model z 0, i.e. 58 yd of phantom vertical extent on that one pool, and
    // every AABB consumer (frustum culling first) then reasons about a box that is mostly nothing.
    // (The comment that used to sit here claimed the opposite — "the AABB stays tight because WMO
    // liquid heights are real, flat values" — which is false on both files above. The MCLQ path has
    // always substituted for its own `FLT_MAX` sentinel for exactly this reason; WMO liquid needed
    // the same treatment against a different degenerate value, and the robust test is *is this
    // vertex drawn*, not *does its height look wrong*.)
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
            // UV from the vertex's MODEL-SPACE position, NOT its per-surface tile index. A per-surface
            // `(i·¼, j·¼)` resets to 0 at each surface's origin; a surface's tile count is arbitrary
            // (Stormwind g099 is 11 tiles = 2.75 repeats), so that reset lands mid-texture and SEAMS at
            // every surface boundary. Dividing the shared model-space position by the repeat period
            // ties all of a WMO's MLIQ surfaces to ONE continuous UV field, so grid-aligned neighbours
            // tile seamlessly. (Same per-cell ¼ step as before — `i·STEP/period = i·¼` — plus the
            // `base/period` offset that couples the surfaces.)
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
        // The `0x6ba970`-resolved nibble (groupLiquid override / first wet tile) IS the WMO
        // surface's sound class — the ambient-loop key (see `LiquidMesh::sound_nibble`).
        sound_nibble: type_nibble,
        // How an interior pool finds its body colour: MOMT lives in the ROOT, so the group file
        // cannot resolve it and the spawner does (see `liquid::surface::spawn_wmo_liquids`).
        material_id: Some(lq.material_id),
        kind,
    })
}

/// Where the bright-doorway fade stops: a vertex `≥ 6.6667` yd from every exterior-neighbour portal
/// keeps its authored bake (`[0x811110]`, VERIFIED off `.rdata`).
const DOORWAY_FADE_REACH: f32 = 6.666_666_5;

/// The per-yard fade slope: `t = 1 − 0.15·dist` (`[0x7ffd6c]`, VERIFIED).
const DOORWAY_FADE_SLOPE: f64 = 0.15;

/// The plane epsilon inside the distance kernel (`0x6c4240`): a vertex within `1/6` yd of a portal's
/// plane reports distance **0** — the "standing in the doorway" case, which whitens outright.
const PORTAL_PLANE_EPS: f32 = 1.0 / 6.0;

/// The reference's float→fixed magic bias: the f32 sum's BIT pattern is shifted `>> 14`, never
/// numerically converted, so the byte falls out of the mantissa's low byte (`[0x8029cc]`).
const FIXED_BIAS: f64 = 512.0;

/// Distance from a model-space point to one portal polygon, the reference's kernel (`0x6c4240`):
/// **0** when the point projects *through the opening* — inside the polygon and within
/// [`PORTAL_PLANE_EPS`] of its plane — otherwise the minimum distance to the polygon's edge segments.
///
/// The containment half is what keeps a doorway's plane from reaching across a room. A portal plane is
/// infinite; several of these openings are not doorways at all but whole ceilings (Dire Maul's sunken
/// courtyard joins its sky through a 283×154 yd quad), so "on the plane" alone would whiten geometry
/// tens of yards from any opening. Points on the plane but *outside* the outline fall through to the
/// edge distance, which is the reference's own fallback and is never smaller than the truth.
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

/// One byte through the reference's fixed-point white-lerp: `((255 − ch)·t + ch + 512)` as f32, whose
/// **bit pattern** shifted `>> 14` is the new byte (`0x6c43d0`, byte-verified against `WoW.exe`).
fn white_lerp_byte(ch: u8, t: f64) -> u8 {
    let chf = f64::from(ch);
    let s = ((255.0 - chf) * t + chf + FIXED_BIAS) as f32;
    ((s.to_bits() >> 14) & 0xff) as u8
}

/// **FixColorVertexAlpha** (`0x6c43d0`) — the bright-doorway portal
/// fade, run once per group *before* it is ever drawn, in place over its MOCV buffer.
///
/// This is not a nicety: a WMO's short transition corridors are authored with **no usable bake at
/// all** — Dire Maul's five entrance passages floor their walkway at MOCV `(10,10,40,α=0)`, a near-black
/// navy — because the runtime fade is what lights them. Skip it and the interior TRANS law renders
/// exactly what the file says: `tex × (10,10,40)` at lit-factor 1, a black floor between lit walls
/// (the director's Dire Maul entrance report). The floor's four corners sit *on* the doorway portal
/// polygons, so the fade takes them to white and the quad interpolates lit end to end.
///
/// The mechanism:
/// 1. **Distance**: per vertex, the minimum [`portal_distance`] over this group's portal refs whose
///    **neighbour group is exterior** (`MOGI.flags & 0x48 != 0`). Portals to interior neighbours
///    whiten nothing — the fade is specific to interior↔exterior openings, observed live. A group
///    with no such portal is untouched.
/// 2. **Branches**: `dist == 0` ⇒ the slot goes white-opaque outright; `dist < 6.6667` **and** the
///    authored alpha is 0 ⇒ RGB white-lerped by `t = 1 − 0.15·dist` with alpha `t·255`; else untouched.
///
/// **The MOPY pre-pass (`0x6c41e0`) is deliberately NOT run**, on live evidence over the static read.
/// Statically it forces `alpha = 0xFF` on every triangle with `MOPY.flags & 1 == 0` — but it is
/// gated (`DAT_00ca8064 == 0`), and the reference's own D3D capture of the abbey
/// shows the uploaded MOCV pool **byte-exact against the
/// file except for the whitened set** — 678/678 and 496/506 vertices identical, alphas included. A
/// pre-pass that had run would have rewritten hundreds of alphas in that same buffer (372 of g003's
/// 506 by this reader's count). It costs nothing here either way: the pre-pass can only *suppress* the
/// partial-lerp branch, never the `dist == 0` whitening the dark corridors depend on.
///
/// A group without MOCV, or without an exterior-neighbour portal, is left exactly as authored.
fn fix_color_vertex_alpha(
    colors: &mut [Color],
    group: &WmoGroup,
    root: &WmoRoot,
    header_refs: (u16, u16),
) {
    let portals = root.portals();
    let infos = root.group_infos();
    let (start, count) = (header_refs.0 as usize, header_refs.1 as usize);
    // The doorway set: portals of THIS group whose far side is an exterior (daylit) group.
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

/// The group's MOCV **as the renderer uploads it** — the authored buffer with the bright-doorway fade
/// already applied (see [`fix_color_vertex_alpha`]), BGRA bytes parallel to the group's vertices.
/// `None` when the bytes aren't a group or carry no MOCV parallel to their positions.
///
/// The seam the `wmo_mocv` instrument reads: a batch that censuses black in the file and white here
/// is a transition corridor the fade lights; black in both is a genuinely dark room.
pub fn wmo_group_fixed_colors(group_bytes: &[u8], root: &WmoRoot) -> Option<Vec<[u8; 4]>> {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut Cursor::new(group_bytes)) else {
        return None;
    };
    let colors = fixed_colors(&group, group_bytes, root)?;
    Some(colors.iter().map(|c| [c.b, c.g, c.r, c.a]).collect())
}

/// The group's MOCV **exactly as authored**, BGRA bytes parallel to its vertices — the before to
/// [`wmo_group_fixed_colors`]'s after. `None` when the bytes aren't a group or carry no MOCV parallel
/// to their positions.
pub fn wmo_group_raw_colors(group_bytes: &[u8]) -> Option<Vec<[u8; 4]>> {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut Cursor::new(group_bytes)) else {
        return None;
    };
    let colors = parallel_colors(&group)?;
    Some(colors.iter().map(|c| [c.b, c.g, c.r, c.a]).collect())
}

/// The group's MOCV as a buffer **parallel to its positions**, or `None` if it has no usable bake.
///
/// The length test cannot be plain equality. A WMO's last chunk is allowed to run past EOF and clamp
/// (decision 0972), and when the clamped chunk is MOCV the buffer comes back a whole *record* short:
/// `Undercity_144.wmo` holds 1159 of its declared 1160 colour bytes, so `chunks_exact(4)` yields 289
/// colours for 290 vertices. Demanding equality threw away **289 good colours over one missing byte**
/// and fell the whole group back to untinted white — a corridor lit `tex × 1.0` inside a city whose
/// every other interior surface is multiplied by a dark bake (decision 0977).
///
/// So a shortfall inside one record's worth is padded from the last complete colour; anything larger
/// is a genuinely broken bake and still reads `None`, because padding hundreds of vertices from one
/// sample would invent lighting rather than recover it.
fn parallel_colors(group: &WmoGroup) -> Option<Vec<Color>> {
    let (have, want) = (group.vertex_colors.len(), group.vertex_positions.len());
    if have == want {
        return Some(group.vertex_colors.clone());
    }
    // `want - have == 1` is the clamped-tail case and the only shortfall we reconstruct.
    let last = (have + 1 == want).then(|| group.vertex_colors.last().copied())??;
    let mut colors = group.vertex_colors.clone();
    colors.push(last);
    Some(colors)
}

/// Shared body of [`wmo_group_fixed_colors`] and the submesh build: clone the authored MOCV and run
/// the fade over it. `None` when the group carries no MOCV parallel to its positions.
fn fixed_colors(group: &WmoGroup, group_bytes: &[u8], root: &WmoRoot) -> Option<Vec<Color>> {
    let mut colors = parallel_colors(group)?;
    let refs =
        wmo_group_header(group_bytes).map_or((0, 0), |h| (h.portal_ref_start, h.portal_ref_count));
    fix_color_vertex_alpha(&mut colors, group, root, refs);
    Some(colors)
}

/// Build the render submeshes for one WMO **group** (its bytes), resolving textures/materials against
/// `root`. Empty if the bytes aren't a parseable group. The bytes-in entry point for the Bevy
/// `AssetLoader`; [`super::load_wmo`] is the chain-reading wrapper.
pub fn wmo_group_submeshes(group_bytes: &[u8], root: &WmoRoot) -> Result<Vec<RenderSubmesh>> {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut Cursor::new(group_bytes)) else {
        return Ok(Vec::new());
    };
    // The bright-doorway fade runs ONCE per group, over the group's own MOCV buffer, before any draw
    // reads it (`0x6b3f90` gates it on the group's `+0xbc` done-byte). We load a group once, so the
    // once-gate is the load itself: fix the colours here and every batch below sees the fixed buffer.
    let colors = fixed_colors(&group, group_bytes, root).unwrap_or_default();
    let ParsedWmo::Root(root) = &root.parsed else {
        anyhow::bail!("WmoRoot is not a root");
    };
    // The MOGP batch-section counts (`+0x28/+0x2a/+0x2c`, after the byte-verified portal-ref span):
    // MOBA batches are laid out TRANS, then INT, then EXT — the class decides an interior group's
    // per-batch lighting law (see [`WmoBatchClass`]). Absent/short header ⇒ everything reads EXT
    // (the conservative exterior law).
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
        // WMO groups carry MONR normals + MOCV colours (parallel to positions); fall back when absent.
        let has_normals = group.vertex_normals.len() == group.vertex_positions.len();
        let has_colors = colors.len() == group.vertex_positions.len();
        // Interior iff neither the EXTERIOR (0x8) nor exterior-lit (0x40) bit is set — the same
        // `groupFlags & 0x48 == 0` test the real client branches on (RenderSubmesh::interior).
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
            // MOCV is BGRA; RGB is the per-vertex shade multiplier, and the REAL alpha rides along —
            // on an interior TRANS batch it is the lit↔bake lerp factor (the FixColorVertexAlpha
            // product, applied above); every other batch class forces it opaque below (alpha encodes
            // blend/unused there — the exterior alpha→0xFF fixup). Absent → white.
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
                // MOMT.blendMode is a DIRECT EGxBlend index (no M2-style remap —
                // `0x6b500f`): 4 = Mod (DST_COLOR/ZERO), 5 = Mod2x
                // (DST_COLOR/SRC_COLOR). Decision 0528.
                Some(4) => ModelBlend::Mod,
                Some(5) => ModelBlend::Mod2x,
                Some(_) => ModelBlend::Blend,
            };
            // The MOMT window/glass flags, each its own mechanism (the drawers
            // `0x6b4f10`/`0x6b5190`, the SIDN updater `0x6b4090`):
            //   UNLIT (0x01)  — the EXTERIOR drawer turns lighting off per material → `tex × white`
            //                   fullbright (the inn's always-lit outside panes, MM_ELWYNN_WND_EXT).
            //                   The INTERIOR drawer ignores it — lit/unlit there is dictated by the
            //                   batch section alone, so `emissive` stays false on interior groups.
            //   SIDN (0x10)   — the authored night-glow emissive colour, ramped by the night
            //                   fraction and added inside the lit sum (the abbey's stained glass
            //                   glowing after dusk). Lit lanes only; carried with its colour.
            //   WINDOW (0x20) — interior-drawer batches swap to the brighter midpoint light (the
            //                   warm pane seen from inside — MM_ELWYNN_WND_INT, MM_STAINED_01).
            let flags = material.map_or(0, |m| m.flags);
            let emissive = !interior && flags & 0x01 != 0;
            // UNCULLED (0x04): the real client backface-culls every WMO batch unless this MOMT
            // flag is set — at the render-state write (`0x6b4fd7`/`0x6b52bf`: bit 0x04
            // inverted → EGxRs 0x14 GL_CULL_FACE; default cull ON, GL_BACK/CCW) — the same law the
            // M2 path already honours. This used to be hardcoded two-sided ("buildings aren't the
            // canopy issue"), and a building IS the canopy issue: 1.12 authors both-sides-visible
            // cloth as the SAME faces twice with reversed winding (one copy per side — B38's tent
            // awning, `wmo_doubled`), each meant to be culled from its wrong side. Drawing them
            // two-sided rasterises both copies into an ulp-level depth tie, and the per-pixel
            // winner is floating-point noise — the B38 flicker/latch (decision 0680).
            // WMO has no skeleton, so the per-vertex skin (`_globals`) is unused.
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
                submesh.normals.clear(); // no MONR → let the renderer recompute flat normals
            }
            if !has_colors {
                submesh.vertex_colors.clear(); // no MOCV → empty (white, no tint)
            }
            // The MOCV **alpha** is per-vertex LIGHTING data on interior batches: a TRANS batch
            // reads it as the lit↔bake lerp factor, an INT batch as the ×4 self-illumination mask
            // (the reference's interior pixel shader `tex·MOCV·(1 + 4·a)` — the inn's fireplace
            // surround bakes α≈100, the portal-seam whitening 255). Exterior batches encode
            // blend/unused state there — force those opaque so bevy's vertex-colour fold can't
            // perturb their alpha semantics (the shader un-folds coverage for the interior lanes).
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
            // Byte 0 per vertex — the authored opacity index (see `wmo_water_alpha_v`).
            opacity: vec![0, 51, 102, 153, 204, 255],
            tile_flags: vec![0x44, 0x0f], // wet (type 4 + fishable), hole
        };
        let m = build_wmo_liquid_mesh(&lq, 0xf).expect("a wet mesh");
        assert_eq!(m.kind, LiquidKind::Still);
        assert_eq!(m.positions.len(), 6); // full 3×2 grid emitted
        assert_eq!(m.indices.len(), 6); // one wet tile → 2 tris → 6 indices
                                        // Grid vertex (i=1, j=0) sits at base + (1·STEP, 0, height).
        assert!((m.positions[1][0] - (100.0 + MLIQ_CELL_STEP)).abs() < 1e-3);
        assert_eq!(m.positions[0], [100.0, 200.0, -3.0]);
        // Opacity is the vertex's OWN authored byte, normalised — not a pinned deep endpoint.
        // Before this, every WMO pool in the game was rendered at a hard 1.0 (fully opaque).
        for (n, &v) in m.depths.iter().enumerate() {
            assert!(
                (v - f32::from(lq.opacity[n]) / 255.0).abs() < 1e-6,
                "vertex {n}: depth channel {v} should carry byte {}/255",
                lq.opacity[n]
            );
        }
        // The wet tile is the LEFT one (indices reference the left cell's 4 corners: 0,1,3,4).
        assert!(m.indices.iter().all(|&i| [0u32, 1, 3, 4].contains(&i)));
    }

    /// Two grid-aligned WMO liquid surfaces (a canal split across groups) must share ONE continuous
    /// UV field, so the animated water texture doesn't seam at the boundary. Surface B begins exactly
    /// where A's last column sits (base offset by 2·STEP in X), so the vertex they share in model space
    /// must get the SAME UV in both — the property a per-surface `(i, j)` reset would break wherever
    /// surfaces are NOT cell-aligned (each
    /// surface restarted at UV 0, and an arbitrary tile count put that restart mid-texture → the seam).
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
        // B's origin column coincides with A's rightmost column (one 2-tile span east).
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
        // A's rightmost-bottom vertex (i=2, j=0 → index 2) and B's leftmost-bottom (i=0, j=0 → index 0)
        // are the same model-space point; their UVs must match (no seam).
        let (ua, ub) = (ma.uvs[2], mb.uvs[0]);
        assert!(
            (ua[0] - ub[0]).abs() < 1e-4 && (ua[1] - ub[1]).abs() < 1e-4,
            "seam: shared-vertex UVs differ A={ua:?} B={ub:?}"
        );
        // The field is position-derived, and it steps exactly ONE repeat per cell — the reference's
        // `u = (float)i` (`0x6b6630`). This used to assert ¼, i.e. a texture 4x too large; correcting
        // it is what took the ripple off mip 0 across the whole near field (`MLIQ_UV_PERIOD`).
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

        // A grid where every tile is a hole → no mesh.
        let all_holes = WmoLiquid {
            tile_flags: vec![0x0f],
            ..lq
        };
        assert!(build_wmo_liquid_mesh(&all_holes, 0xf).is_none());
    }

    #[test]
    fn reads_group_header_flags_and_portal_ref_span() {
        // A 68-byte MOGP header: flags @0x08, portal_ref_start @0x24, portal_ref_count @0x26.
        let mut hdr = vec![0u8; 68];
        hdr[8..12].copy_from_slice(&0x8u32.to_le_bytes()); // EXTERIOR
        hdr[0x24..0x26].copy_from_slice(&5u16.to_le_bytes());
        hdr[0x26..0x28].copy_from_slice(&3u16.to_le_bytes());
        hdr[0x30..0x34].copy_from_slice(&[2, 0, 0, 0]); // fog indices
                                                        // Group file: MVER then MOGP(super-chunk = header + (no) sub-chunks).
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

    /// The whole-group submersion override at MOGP `0x34`, and the two ways a header can fail to
    /// carry one. `0` is a live value (all 13 shipped groups that set it say `0` = water), so the
    /// sentinel has to be spelled out rather than defaulted to zero.
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

        // The shipped case: water, over the whole group, with no MLIQ anywhere in the file.
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

        // A header truncated before 0x34 reads as "no liquid" — the safe way round, since the
        // override only ever adds water and a short header is not evidence of a flooded room.
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
