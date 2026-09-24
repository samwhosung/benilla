//! Model loading — the shared `models` module face.
//!
//! Parsing is delegated to `benilla-m2` (M2) / `benilla-wmo` (WMO). Both produce a list of
//! [`RenderSubmesh`] — one per render batch, each with its own (remapped) vertices, texture, and blend
//! mode. M2 models and WMO groups are split into batches that each have their own material (a tree =
//! opaque trunk + alpha leaves; a building = many wall/floor/roof textures), so they render separately
//! or the wrong texture/blend lands on parts of the model. Vertices are model space (Z-up ≈ WoW axes).
//!
//! Split by concern: this file keeps the top-level dispatch ([`load_object_model`]) + the byte/remap
//! helpers the submodules share; [`types`] holds the shared render types ([`RenderSubmesh`],
//! [`ModelBlend`], billboard/light data), [`m2_batches`] the M2 render-batch assembly loop, [`bounds`]
//! the authored-bounds/selection-ring reads, [`anim`] skeleton + animation, [`wmo`] the WMO parse, and
//! [`collision`] the collision hulls.

use std::collections::HashMap;

use anyhow::Result;

use crate::Chain;

mod anim;
mod anim_summary;
mod art_extent;
mod bounds;
mod collision;
mod key_anim;
mod m2_batches;
mod mat_anim;
mod records;
mod tex_anim;
mod types;
mod wmo;
pub use anim::*;
pub use anim_summary::*;
pub use art_extent::*;
pub use bounds::*;
pub use collision::*;
pub(crate) use key_anim::{bake_track, SeqSlot};
pub use key_anim::{KeyAnim, SeqLoops};
pub use m2_batches::*;
pub use mat_anim::{AlphaAnim, AlphaSeq, RgbAnim, ScalarAnim};
pub use records::*;
pub use tex_anim::{rotation_2x2, uv_transform, UvAnim, UvRotAnim};
pub use types::*;
pub use wmo::*;

/// Build a submesh from a stream of global vertex indices, remapping to a local vertex subset so
/// each batch is a compact standalone mesh. `vertex` fetches `(position, normal, uv, colour)` for a
/// global index. Also returns, parallel to the submesh's local vertices, each one's **global** index
/// — so the M2 path can fill the per-vertex skin binding ([`RenderSubmesh::joints`]) afterward (the
/// skin lives on the M2 vertex, not in the geometry closure, and WMO has none).
fn remap_submesh(
    global_indices: impl Iterator<Item = u32>,
    vertex: impl Fn(u32) -> ([f32; 3], [f32; 3], [f32; 2], [f32; 4]),
    texture: Option<String>,
    blend: ModelBlend,
    two_sided: bool,
    interior: bool,
    emissive: bool,
) -> (RenderSubmesh, Vec<u32>) {
    let mut map: HashMap<u32, u32> = HashMap::new();
    let (mut positions, mut normals, mut uvs, mut indices, mut vertex_colors) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut globals: Vec<u32> = Vec::new();
    for g in global_indices {
        let local = *map.entry(g).or_insert_with(|| {
            let (p, n, uv, c) = vertex(g);
            positions.push(p);
            normals.push(n);
            uvs.push(uv);
            vertex_colors.push(c);
            globals.push(g);
            (positions.len() - 1) as u32
        });
        indices.push(local);
    }
    (
        RenderSubmesh {
            positions,
            normals,
            uvs,
            indices,
            texture,
            skin_slot: None,
            geoset_id: 0, // set by the caller per batch (the M2 path; WMO leaves it 0)
            char_slot: None, // set by the caller per batch (M2 only)
            icon_slot: false, // set by the caller per batch (M2 texture type 14)
            blend,
            // Repeat is the pre-0763 behaviour and stays WMO's; the M2 batch loop overrides both
            // from the texture record's own flags.
            wrap_x: true,
            wrap_y: true,
            two_sided,
            vertex_colors,
            joints: Vec::new(), // M2 path fills these from `globals`; WMO leaves them empty
            weights: Vec::new(), //
            interior,
            emissive,
            sidn: None, // set by the WMO path from MOMT SIDN (0x10) + its authored colour
            window: false, // set by the WMO path from MOMT WINDOW (0x20)
            additive: false, // set by the M2 path from the blend mode (3/4)
            no_depth_write: false, // set by the M2 path from render flag 0x10 (WMO keeps standard depth)
            no_depth_test: false,  // set by the M2 path from render flag 0x08
            fog_policy: FogPolicy::Scene, // refined by the M2 path (flag 0x02 + the blend table); WMO batches fog with the scene
            billboard: None,              // set by the M2 path after detecting the batch's bone
            welded_billboard: false,      // set by the M2 path from the separability gate (0839)
            alpha_anim: None, // set by the M2 path from the batch's colour/weight tracks
            uv_anim: None,    // set by the M2 path from the batch's texture transform
            uv_seq: None,     // …and its per-sequence set, when the slots disagree (1408)
            uv_rot_seq: None,
            uv_scale_seq: None,
            rgb_anim: None,  // set by the M2 path from the batch's colour RGB track
            rgb_seq: None,   // …ditto (1408)
            wmo_batch: None, // set by the WMO path from the MOGP batch-section counts
            env_map: false,  // set by the M2 path from texture_unit_lookup[texCoordSet] > 2
            section: None,   // set by the M2 path from the batch's skin_section_index
        },
        globals,
    )
}

/// Normalize a model path for the chain: lowercase (case-insensitive lookup), `.mdx`/`.mdl` → `.m2`.
pub(crate) fn model_path(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    match lower
        .strip_suffix(".mdx")
        .or_else(|| lower.strip_suffix(".mdl"))
    {
        Some(stem) => format!("{stem}.m2"),
        None => lower,
    }
}

/// Panicking little-endian readers, kept **only** for raw offset walks that predate the decision-0064
/// migration to `benilla_bytes::ByteExt` — [`anim`]'s bone/track walk, [`anim_summary`]'s header
/// reads, and [`m2_batches`]'s sequence-0 time-band lookup all still import these three.
pub(crate) fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub(crate) fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub(crate) fn le_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Load a world model by path, dispatching on extension: `.wmo` → [`load_wmo`], otherwise
/// (`.mdx`/`.mdl`/`.m2`) → [`load_m2_mesh`]. Used for GameObject display models, which are mostly M2
/// with a few WMO.
pub fn load_object_model(chain: &mut Chain, raw_path: &str) -> Result<Vec<RenderSubmesh>> {
    if raw_path.to_ascii_lowercase().ends_with(".wmo") {
        load_wmo(chain, raw_path)
    } else {
        load_m2_mesh(chain, raw_path)
    }
}
