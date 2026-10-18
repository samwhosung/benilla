//! Model loading over `benilla-m2` and `benilla-wmo`: a model becomes one [`RenderSubmesh`] per
//! render batch, each with its own vertices, texture and blend, in model space (Z up, WoW axes).

use std::collections::HashMap;

use anyhow::Result;

use crate::Chain;

mod anim;
mod anim_summary;
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

/// Build a compact submesh from global vertex indices. Also returns each local vertex's global
/// index, from which the M2 path fills [`RenderSubmesh::joints`].
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
            // The per-batch fields start at their defaults; the M2 and WMO paths set their own.
            skin_slot: None,
            geoset_id: 0,
            char_slot: None,
            icon_slot: false,
            blend,
            // WMO keeps repeat; the M2 batch loop sets both from the texture record's flags.
            wrap_x: true,
            wrap_y: true,
            two_sided,
            vertex_colors,
            joints: Vec::new(),
            weights: Vec::new(),
            interior,
            emissive,
            sidn: None,
            window: false,
            additive: false,
            no_depth_write: false,
            no_depth_test: false,
            fog_policy: FogPolicy::Scene,
            billboard: None,
            welded_billboard: false,
            alpha_anim: None,
            uv_anim: None,
            uv_seq: None,
            uv_rot_seq: None,
            uv_scale_seq: None,
            rgb_anim: None,
            rgb_seq: None,
            wmo_batch: None,
            env_map: false,
            stage1: None,
            combine: 1,
            section: None,
        },
        globals,
    )
}

/// The model `0x707350` creates in place of a file the cache refuses (`0x70737f`).
pub const ERROR_CUBE: &str = "Spells\\ErrorCube.mdx";

/// The bytes of a model path the cache reads: `0x706a50` copies it into a 0x104-byte buffer with
/// `SStrCopy` (`0x706a6e`), which stops one short of the size (`0x64a5cf`, `0x64a5e2`).
const NAME_CAP: usize = 0x103;

/// The archive file a model path names under the cache's rule (`0x706a50`), lowercased: the path
/// cut to [`NAME_CAP`] bytes, its extension taken at the last dot (`strrchr`, `0x706a80`), `.mdx`
/// and `.mdl` in any case made `.m2` and `.m2` kept (`0x706a92`-`0x706adf`). `None` for no dot or
/// any other extension (`0x706a8c`, `0x706adf`), which the caller stands [`ERROR_CUBE`] for.
pub fn m2_file_name(raw: &str) -> Option<String> {
    let bytes = &raw.as_bytes()[..raw.len().min(NAME_CAP)];
    let dot = bytes.iter().rposition(|&b| b == b'.')?;
    let ext = &bytes[dot + 1..];
    let known = ["mdx", "mdl", "m2"]
        .iter()
        .any(|e| ext.eq_ignore_ascii_case(e.as_bytes()));
    known.then(|| {
        format!(
            "{}.m2",
            String::from_utf8_lossy(&bytes[..dot]).to_ascii_lowercase()
        )
    })
}

/// The archive file a model created through `0x707350` reads: [`m2_file_name`], or the error cube
/// for a refused path.
pub(crate) fn model_path(raw: &str) -> String {
    m2_file_name(raw).unwrap_or_else(|| model_name(ERROR_CUBE))
}

/// A model name normalized for the chain by [`m2_file_name`], left as it is (lowercased) when the
/// rule refuses it: for a name whose model is made later, from a longer path or not at all.
pub(crate) fn model_name(raw: &str) -> String {
    m2_file_name(raw).unwrap_or_else(|| raw.to_ascii_lowercase())
}

/// Panicking little-endian reads at a byte offset, for the raw walks not on
/// `benilla_bytes::ByteExt`.
pub(crate) fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub(crate) fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub(crate) fn le_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Load a GameObject display model by path: a `.wmo` through [`load_wmo`], anything else as an M2.
pub fn load_object_model(chain: &mut Chain, raw_path: &str) -> Result<Vec<RenderSubmesh>> {
    if raw_path.to_ascii_lowercase().ends_with(".wmo") {
        load_wmo(chain, raw_path)
    } else {
        load_m2_mesh(chain, raw_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x706a50`: the last dot's extension, `.mdx`/`.mdl` in any case made `.m2`, `.m2` kept,
    /// anything else refused; a creating lane reads the error cube, a name-only one keeps it.
    #[test]
    fn the_cache_takes_the_last_extension() {
        assert_eq!(
            m2_file_name("Creature\\Wolf\\Wolf.MDX").as_deref(),
            Some("creature\\wolf\\wolf.m2")
        );
        assert_eq!(m2_file_name("a.Mdl").as_deref(), Some("a.m2"));
        assert_eq!(m2_file_name("a.M2").as_deref(), Some("a.m2"));
        for refused in ["a", "a.blp", "a.mdx.bak", "World\\a.mdx\\b", ""] {
            assert_eq!(m2_file_name(refused), None, "{refused}");
        }
        assert_eq!(model_path("a.blp"), "spells\\errorcube.m2");
        assert_eq!(model_name("A.blp"), "a.blp");
    }

    /// The name is cut to 0x103 bytes before the dot is looked for (`0x64a5a0` into the 0x104-byte
    /// buffer): an extension that ends past the cut is refused, or read short.
    #[test]
    fn a_path_past_the_buffer_is_cut_before_its_extension() {
        let at = |n: usize, ext: &str| format!("{}{ext}", "x".repeat(n - ext.len()));
        assert_eq!(
            m2_file_name(&at(0x103, ".mdx")),
            Some(format!("{}.m2", "x".repeat(0x103 - 4)))
        );
        assert_eq!(m2_file_name(&at(0x104, ".mdx")), None, "cut to `.md`");
        assert_eq!(
            m2_file_name(&format!("{}.m2x", "x".repeat(0x100))),
            Some(format!("{}.m2", "x".repeat(0x100))),
            "cut to `.m2`"
        );
    }
}
