//! Collision-hull parsing: the coarse M2 bounding hull and the WMO group collision/camera-collision
//! triangle gathers (the avian colliders the streamer bakes). Split out of the model parser.

use std::collections::HashMap;
use std::io::Cursor;

use anyhow::{Context, Result};
use benilla_m2::parse_m2;
use benilla_wmo::{parse_wmo, ParsedWmo};

use crate::Chain;

use super::model_path;

/// Collision geometry in **raw model-local coords** — the same space as [`super::RenderSubmesh::positions`]
/// (Z-up ≈ WoW axes). The benilla bake applies the *identical* placement transform the renderer
/// uses, so the collider coincides with the visible model. `indices` is a triangle list into
/// `positions`. Both vecs empty ⇒ the source carries no collision geometry (caller skips it).
#[derive(Debug, Clone, Default)]
pub struct CollisionMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl CollisionMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
    /// Triangle count (`indices.len() / 3`).
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Decode an M2 doodad's **collision hull** — the coarse solid shape (trunk/walls) the client collides
/// against, separate from and much smaller than the render mesh (verified: hull span ≪ render bbox, so
/// you're blocked by the trunk but walk through the canopy). Source = the M2 header
/// `bounding_{vertices,triangles}` arrays (raw bytes in `raw_data`): vertices are `C3Vector` (12 B),
/// triangles a `u16` index list — one collision normal per face, which we don't need (parry derives
/// face normals). The Legion `PCOL` `collision_mesh_data` is always absent in 1.12. A doodad with no
/// hull (`nBoundingTriangles == 0` — many small props) returns an empty mesh: **collide-iff-hull** is
/// the sole gate (verified). Validated 2026-06-02.
pub fn load_m2_collision_hull(chain: &mut Chain, raw_path: &str) -> Result<CollisionMesh> {
    let path = model_path(raw_path);
    let bytes = chain
        .read_file(&path)
        .with_context(|| format!("reading M2 {path}"))?;
    parse_m2_collision_hull(&bytes).with_context(|| format!("parsing M2 {path}"))
}

/// Parse an M2 doodad's collision hull straight from its file bytes (the byte-fed twin of
/// [`load_m2_collision_hull`], for the asset-loader path that already holds the bytes). Empty mesh ⇒
/// no hull (`nBoundingTriangles == 0`). See [`load_m2_collision_hull`] for the format details.
pub fn parse_m2_collision_hull(bytes: &[u8]) -> Result<CollisionMesh> {
    let format =
        parse_m2(&mut Cursor::new(bytes)).map_err(|e| anyhow::anyhow!("parsing M2 hull: {e}"))?;
    let rd = &format.model().raw_data;
    let positions: Vec<[f32; 3]> = rd
        .bounding_vertices
        .as_chunks::<12>()
        .0
        .iter()
        .map(|c| {
            [
                f32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                f32::from_le_bytes([c[4], c[5], c[6], c[7]]),
                f32::from_le_bytes([c[8], c[9], c[10], c[11]]),
            ]
        })
        .collect();
    let n = positions.len() as u32;
    let indices: Vec<u32> = rd
        .bounding_triangles
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u32::from(u16::from_le_bytes([c[0], c[1]])))
        .collect();
    // Guard (verified true on every real 1.12 hull): whole triangles, all indices in range. Bail to
    // "no hull" rather than emit broken geometry if a future/odd file violates it.
    if indices.is_empty() || !indices.len().is_multiple_of(3) || indices.iter().any(|&i| i >= n) {
        return Ok(CollisionMesh::default());
    }
    Ok(CollisionMesh { positions, indices })
}

/// MOPY per-face flags (1.12) and the two collision gather masks — both from `WoW.exe` 5875
/// (objdump).
/// The skip predicate is a pure function of a 32-bit collision *class* word built per query; the two
/// classes benilla reproduces, one collider mesh each:
///   - **WALKING / movement** (the player body): skip a face **iff `flags & 0x04`** (DETAIL/decal). Reject
///     mask `0x84` from class word `0x10_0111` (collision `0x6315f0`); WMO BSP leaf `0x6bc700`.
///   - **CAMERA / LOS** (third-person occlusion): skip a face **iff `flags & 0x02`** (NOCAMCOLLIDE). It does
///     NOT exclude DETAIL, so the camera collides with visible decals/overhangs the player walks *under*
///     (e.g. forge pipes). Reject mask `0x82` from class word `0x10_0171` (ui `0x50e570`).
///
/// So the difference is exactly: walking drops DETAIL; the camera instead drops NOCAMCOLLIDE. (The older
/// `FUN_006bca50` citation was a superseded prototype lead.)
const MOPY_DETAIL: u8 = 0x04;
/// The bit the **camera/LOS** gather excludes — NOCAMCOLLIDE, the faces the third-person camera passes
/// through. See [`MOPY_DETAIL`] for the full walking-vs-camera mask derivation.
const MOPY_NOCAMCOLLIDE: u8 = 0x02;

/// Decode a WMO's **collidable triangles** — the faces the client collides against — flattened across
/// every group file into one mesh in **raw WMO-local coords** (same space as [`load_wmo`]'s render
/// submeshes; the benilla bake reuses the identical MODF placement transform). Selection = the
/// binary-VERIFIED client filter: a face collides **iff `(flags & 0x04 DETAIL) == 0`** (exclude
/// render-only decals only) — the walking-gather mask 0x84 read from `WoW.exe` 5875 (`FUN_006bca50`,
/// objdump). This is *more* inclusive than the old
/// server-vmap-extractor rule `COLLISION || (RENDER && !DETAIL)`, which wrongly dropped the
/// flags-0x00 / 0x40-only faces the real client collides. *(Whole-group skips — mogpFlags
/// unreachable/antiportal — are a later refinement; including all rendered groups is safe for ground-snap.)*
pub fn load_wmo_collision_tris(chain: &mut Chain, raw_path: &str) -> Result<CollisionMesh> {
    let root_path = raw_path.to_ascii_lowercase();
    let bytes = chain
        .read_file(&root_path)
        .with_context(|| format!("reading WMO {root_path}"))?;
    let ParsedWmo::Root(root) = parse_wmo(&mut Cursor::new(&bytes))
        .map_err(|e| anyhow::anyhow!("parsing {root_path}: {e}"))?
    else {
        anyhow::bail!("{root_path} is not a WMO root file");
    };
    let stem = root_path.strip_suffix(".wmo").unwrap_or(&root_path);
    let (mut positions, mut indices): (Vec<[f32; 3]>, Vec<u32>) = (Vec::new(), Vec::new());
    for gi in 0..root.n_groups {
        let group_path = format!("{stem}_{gi:03}.wmo");
        let Ok(gbytes) = chain.read_file(&group_path) else {
            continue;
        };
        accumulate_wmo_group_collision(&gbytes, &mut positions, &mut indices);
    }
    Ok(CollisionMesh { positions, indices })
}

/// Accumulate one WMO **group** file's *walking*-collidable triangles (raw WMO-local coords) into
/// `positions`/`indices` — the player-body gather: a face collides **unless** it's a DETAIL/decal face
/// (`flags & 0x04`). The byte-fed unit shared by [`load_wmo_collision_tris`] (chain) and the asset
/// loader (which already holds each group's bytes). See [`accumulate_wmo_group_camera_collision`] for
/// the camera/LOS gather, which keeps DETAIL but drops NOCAMCOLLIDE instead.
pub fn accumulate_wmo_group_collision(
    group_bytes: &[u8],
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    accumulate_wmo_group_faces(group_bytes, MOPY_DETAIL, 0, positions, indices);
}

/// Accumulate one WMO **group** file's *camera/LOS*-collidable triangles (raw WMO-local coords): the
/// third-person-camera gather — a face collides **unless** it's NOCAMCOLLIDE (`flags & 0x02`). Unlike
/// the walking gather it keeps DETAIL faces (visible decals/overhangs like forge pipes), so the camera
/// can't slip behind them. See [`accumulate_wmo_group_collision`] and [`MOPY_DETAIL`].
pub fn accumulate_wmo_group_camera_collision(
    group_bytes: &[u8],
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    accumulate_wmo_group_faces(group_bytes, MOPY_NOCAMCOLLIDE, 0, positions, indices);
}

/// Accumulate one WMO **group** file's *camera-only* triangles — the faces the camera collides with
/// but the walking gather drops: DETAIL (`flags & 0x04`) set, NOCAMCOLLIDE (`flags & 0x02`) clear.
/// Exactly the camera gather minus the walking gather, kept as its own set so the down-ray's
/// camera-void fallback can race it *after* the faithful walking leg has missed,
/// without double-counting the faces the walking leg already saw.
pub fn accumulate_wmo_group_camera_only_collision(
    group_bytes: &[u8],
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    accumulate_wmo_group_faces(
        group_bytes,
        MOPY_NOCAMCOLLIDE,
        MOPY_DETAIL,
        positions,
        indices,
    );
}

/// Shared WMO group-face gather: accumulate every triangle whose MOPY flags don't intersect `skip_mask`
/// (and carry all of `require_mask`, when non-zero), remapping verts to a compact subset *per group*
/// (equal local indices in different groups are distinct verts). A group that doesn't parse is skipped.
/// The two collision audiences differ only in `skip_mask` — `0x04` DETAIL for walking, `0x02`
/// NOCAMCOLLIDE for the camera (see [`MOPY_DETAIL`]); the camera-only set requires DETAIL on top.
fn accumulate_wmo_group_faces(
    group_bytes: &[u8],
    skip_mask: u8,
    require_mask: u8,
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut Cursor::new(group_bytes)) else {
        return;
    };
    let n_tri = group.vertex_indices.len() / 3;
    let mut map: HashMap<u16, u32> = HashMap::new();
    for t in 0..n_tri {
        let Some(mopy) = group.material_info.get(t) else {
            continue;
        };
        if mopy.flags & skip_mask != 0 || mopy.flags & require_mask != require_mask {
            continue;
        }
        for k in 0..3 {
            let Some(&vidx) = group.vertex_indices.get(t * 3 + k) else {
                continue;
            };
            let Some(p) = group.vertex_positions.get(vidx as usize) else {
                continue;
            };
            let global = *map.entry(vidx).or_insert_with(|| {
                positions.push([p.x, p.y, p.z]);
                (positions.len() - 1) as u32
            });
            indices.push(global);
        }
    }
}
