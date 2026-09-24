//! ADT → terrain-tile asset loader.
//!
//! Decodes one vanilla ADT tile into an [`AdtTile`]: the decoded per-MCNK chunks + a
//! [`ChunkShading`] per chunk (the splat-array indices resolved while packing), plus the tile's
//! three `texture_2d_array`s — ground layers, per-chunk alpha (blend) maps, per-chunk MCSH shadow
//! maps — and the raw doodad/WMO placement lists (for the spawn system to load as
//! `M2Model`/`WmoModel`).
//!
//! **The loader ships no meshes.** A tile's ~256 chunk meshes used to be labeled sub-assets built
//! here — which made a tile's landing atomic: the whole set hit the render world's
//! extract/prepare in ONE frame (~80–105 ms of process CPU per fresh row, measured — the B181
//! first-contact spike), and no app-side budget downstream of the load could spread work the
//! loader had already packaged. The app builds each cell's mesh itself via [`chunk_to_mesh`], a
//! few cells per frame (`terrain_stream::furnish`), so the render-world cost of a landing is
//! paced at the only place pacing is possible: mesh-asset creation.
//!
//! Each chunk mesh carries its array indices on every one of its vertices — `COLOR` (4 layer
//! indices) and `UV1` (alpha index `.x`, shadow index `.y`, `-1` = none) — which is why all 256
//! chunks of a tile still share **one** material; the app's terrain material reads them.
//!
//! Layer textures are read via [`LoadContext::read_asset_bytes`] (raw bytes, dependency-tracked) and
//! packed by [`crate::terrain`] — the BLP's authored mips verbatim (the C2 fidelity invariant).
//! Materials, the collider, and the placement *models* are built by the app at spawn — this loader
//! produces decoded data only.

use std::collections::HashMap;

use benilla_formats::{
    adt_to_tile_mesh, blp_bytes_to_native_chain, ChunkMesh, Doodad, WmoInstance, ALPHA_MAP_SIZE,
    SHADOW_MAP_SIZE,
};
use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext, RenderAssetUsages};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::reflect::TypePath;

use crate::coords::wow_to_bevy;
use crate::terrain::{
    alpha_array_image, layer_array_image, pack_layers, shadow_array_image, RawLayer, LAYER_TEX_SIZE,
};

/// A loaded ADT terrain tile: the decoded chunks + per-chunk shading indices + the tile's three
/// texture arrays, plus the raw doodad/WMO placements (which the spawn system resolves to
/// `M2Model`/`WmoModel` handles). Cell meshes are built by the app, per chunk, via
/// [`chunk_to_mesh`] — see the module doc for why the loader ships none.
#[derive(Asset, TypePath)]
pub struct AdtTile {
    /// The splat-array indices for each entry of `chunks` (index-parallel; a hole-emptied chunk
    /// carries the default, which [`chunk_to_mesh`] never reads because it returns `None` first).
    /// Resolved here because only the loader knows the arrays' layout — it packs them.
    pub shading: Vec<ChunkShading>,
    /// `texture_2d_array` of the tile's ground textures (the C2-faithful authored mip chains).
    pub layer_array: Handle<Image>,
    /// `texture_2d_array` of per-chunk alpha (blend-weight) maps.
    pub alpha_array: Handle<Image>,
    /// `texture_2d_array` of per-chunk MCSH baked shadow maps.
    pub shadow_array: Handle<Image>,
    /// M2 doodad placements (raw WoW coords).
    pub doodads: Vec<Doodad>,
    /// WMO building placements (raw WoW coords).
    pub wmos: Vec<WmoInstance>,
    /// The tile's decoded per-MCNK chunks — the source the arrays were built from and the cell
    /// meshes are built from ([`chunk_to_mesh`]), kept resident so the app can derive everything
    /// else it owes the tile: the terrain collider, the MCLQ liquid surfaces (each
    /// `ChunkMesh.liquid`) and the ground-clutter scatter (per-chunk layers/normals/MCSH + the
    /// app-side ground-effect catalog). Bounded by the loaded-tile count; the heaviest field on the
    /// asset, carried because clutter/liquid are app concerns (the loader stays catalog-independent).
    pub chunks: Vec<ChunkMesh>,
}

/// One drawn chunk's resolved splat indices — everything [`chunk_to_mesh`] needs beyond the
/// decoded [`ChunkMesh`] itself. Uniform across the chunk's own vertices by construction.
#[derive(Clone, Copy)]
pub struct ChunkShading {
    /// The chunk's up-to-4 layer-array indices (vertex `COLOR`; unused slots repeat slot 0, whose
    /// alpha weight is 0, so they never show).
    pub layers: [u32; 4],
    /// The chunk's alpha-array index (vertex `UV1.x`; 0 — a weight-0 slot — when it has no map).
    pub alpha: u32,
    /// The chunk's shadow-array index (vertex `UV1.y`; `-1.0` = no MCSH shadow map).
    pub shadow: f32,
}

impl Default for ChunkShading {
    fn default() -> Self {
        Self {
            layers: [0; 4],
            alpha: 0,
            shadow: -1.0,
        }
    }
}

/// Build one drawn MCNK chunk's render mesh (Bevy space, absolute world coords) — called by the
/// app a few cells per frame, never by the loader (see the module doc: a loader-built set lands
/// atomically, and the landing IS the B181 first-contact spike).
///
/// **Per chunk, not per tile, because the cull's unit is the chunk**: the
/// exterior-scene cull tests one AABB per drawn object, and the reference's object there is the
/// 33.333 yd cell (`0x683bf0`), never the 533 yd tile — a slab the camera stands on intersects
/// every portal window, so it could never be hidden from inside a building. Nothing is duplicated
/// by the split: the merged form never shared a vertex across a chunk boundary either.
///
/// `RenderAssetUsages::RENDER_WORLD`: the render world takes the buffers on extract — no clone on
/// landing, no resident main-world copy per cell. Nothing reads the *mesh* main-side: the
/// collider, liquid, and clutter all derive from the decoded [`AdtTile::chunks`]. The one thing
/// the main world does need from it — the cell's `Aabb`, which the exterior cull FAILS OPEN
/// without — the caller takes via `Mesh::compute_aabb` before handing the mesh over.
///
/// Returns `None` for a chunk the hole mask emptied (it draws nothing: no mesh, no entity).
pub fn chunk_to_mesh(chunk: &ChunkMesh, shading: &ChunkShading) -> Option<Mesh> {
    chunks_to_mesh(&[(chunk, shading)])
}

/// [`chunk_to_mesh`] over several chunks of ONE tile as one mesh — the terrain CELL (decision
/// 1944): every per-chunk fact the shader reads (the layer indices, the alpha/shadow slot) is
/// already baked per vertex, and a tile's chunks share its material, so concatenating them
/// changes nothing a pixel can see and turns sixteen draws into one. Positions are absolute,
/// so no per-chunk transform is folded. A hole-emptied chunk contributes nothing; a cell of
/// nothing but holes is `None`.
pub fn chunks_to_mesh(parts: &[(&ChunkMesh, &ChunkShading)]) -> Option<Mesh> {
    let live: Vec<&(&ChunkMesh, &ChunkShading)> =
        parts.iter().filter(|(c, _)| c.indices.len() >= 3).collect();
    if live.is_empty() {
        return None;
    }
    let total: usize = live.iter().map(|(c, _)| c.positions.len()).sum();
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(total);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(total);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(total);
    let mut uv1: Vec<[f32; 2]> = Vec::with_capacity(total);
    let mut colors: Vec<[f32; 4]> = Vec::with_capacity(total);
    let mut indices: Vec<u32> = Vec::with_capacity(live.iter().map(|(c, _)| c.indices.len()).sum());
    for (chunk, shading) in live {
        let base = u32::try_from(positions.len()).expect("a cell's vertex count fits u32");
        let chunk_positions: Vec<[f32; 3]> = chunk
            .positions
            .iter()
            .map(|p| wow_to_bevy(*p).to_array())
            .collect();
        if chunk.normals.len() == chunk.positions.len() {
            normals.extend(chunk.normals.iter().map(|n| wow_to_bevy(*n).to_array()));
        } else {
            normals.extend(computed_normals(&chunk_positions, &chunk.indices));
        }
        let li = shading.layers;
        let col = [li[0] as f32, li[1] as f32, li[2] as f32, li[3] as f32];
        let n = chunk_positions.len();
        uv1.extend(std::iter::repeat_n(
            [shading.alpha as f32, shading.shadow],
            n,
        ));
        colors.extend(std::iter::repeat_n(col, n));
        uvs.extend_from_slice(&chunk.uvs);
        indices.extend(chunk.indices.iter().map(|i| i + base));
        positions.extend(chunk_positions);
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uv1);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

/// Bevy [`AssetLoader`] decoding a vanilla `*.adt` tile → [`AdtTile`].
#[derive(Default, TypePath)]
pub struct AdtLoader;

impl AssetLoader for AdtLoader {
    type Asset = AdtTile;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        ctx: &mut LoadContext<'_>,
    ) -> Result<AdtTile, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let to_io = |e: anyhow::Error| std::io::Error::other(format!("{e:#}"));
        let tile = adt_to_tile_mesh(&bytes).map_err(to_io)?;

        // Layer array: a solid-green fallback at index 0, then each unique referenced layer texture.
        // The layers are collected in their **authored** form and packed only once the tile is fully
        // read — one `texture_2d_array` has one format, so whether this tile's DXT blocks go up
        // untouched is a question about the whole set (`terrain::pack_layers`).
        let mut layers: Vec<RawLayer> = Vec::new();
        let mut layer_index: HashMap<String, u32> = HashMap::new();

        let mut alpha_buf: Vec<u8> = Vec::new();
        let mut alpha_count = 0u32;
        let mut shadow_buf: Vec<u8> = Vec::new();
        let mut shadow_count = 0u32;

        // One shading entry per chunk (holes get the default). COLOR carries the chunk's 4 layer
        // indices, UV1 its alpha/shadow — uniform over the chunk's own vertices, which is what
        // keeps ONE material per tile. The MESH built from these lands later, app-side and paced
        // (`chunk_to_mesh`); only the array-index resolution belongs here, with the arrays.
        let mut shading: Vec<ChunkShading> = Vec::with_capacity(tile.chunks.len());

        for chunk in tile.chunks.iter() {
            // A chunk the hole mask emptied draws nothing: no mesh, no entity, and no alpha/shadow
            // array slot either (the indices below are running counters, so skipping here is free).
            if chunk.indices.len() < 3 {
                shading.push(ChunkShading::default());
                continue;
            }
            // Resolve up to 4 layer textures to array indices (appending new ones). Missing slots
            // reuse layer 0 (its alpha weight is 0, so it never shows).
            let mut li = [0u32; 4];
            for (slot, name) in chunk.layer_textures.iter().take(4).enumerate() {
                let key = normalize_path(name);
                li[slot] = if let Some(&i) = layer_index.get(&key) {
                    i
                } else if let Some(layer) = read_layer(ctx, &key).await {
                    // +1: index 0 is the fallback layer, which is not in `layers`.
                    let index = layers.len() as u32 + 1;
                    layers.push(layer);
                    layer_index.insert(key, index);
                    index
                } else {
                    0
                };
            }
            for slot in chunk.layer_textures.len().min(4)..4 {
                li[slot] = li[0];
            }

            let ai = match &chunk.alpha_map {
                Some(rgba) => {
                    alpha_buf.extend_from_slice(rgba);
                    alpha_count += 1;
                    alpha_count - 1
                }
                None => 0,
            };
            let si: f32 = match &chunk.shadow {
                Some(map) => {
                    shadow_buf.extend_from_slice(map);
                    shadow_count += 1;
                    (shadow_count - 1) as f32
                }
                None => -1.0,
            };
            shading.push(ChunkShading {
                layers: li,
                alpha: ai,
                shadow: si,
            });
        }

        // A `texture_2d_array` needs ≥1 layer even when a tile carries no alpha / shadow maps.
        if alpha_count == 0 {
            alpha_buf = vec![0u8; (ALPHA_MAP_SIZE * ALPHA_MAP_SIZE * 4) as usize];
            alpha_count = 1;
        }
        if shadow_count == 0 {
            shadow_buf = vec![0u8; (SHADOW_MAP_SIZE * SHADOW_MAP_SIZE) as usize];
            shadow_count = 1;
        }

        let layer_count = layers.len() as u32 + 1;
        let packed = pack_layers([107, 133, 82, 0], layers);
        // Which lane this tile took, and what it cost. Per-tile rather than aggregated because the
        // question after a perf report is always "which tiles fell back, and where am I standing" —
        // and one greppable format name per tile answers it from a player's log with no machinery
        // (`grep -c Bc2RgbaUnorm` against `grep -c Rgba8Unorm`) (1646).
        debug!(
            "adt {}: layer array {layer_count} x {:?}, {} KiB",
            ctx.path(),
            packed.format,
            packed.data.len() >> 10
        );
        let layer_array = layer_array_image(LAYER_TEX_SIZE, layer_count, packed);
        let alpha_array = alpha_array_image(ALPHA_MAP_SIZE, alpha_count, alpha_buf);
        let shadow_array = shadow_array_image(SHADOW_MAP_SIZE, shadow_count, shadow_buf);

        Ok(AdtTile {
            shading,
            layer_array: ctx.add_labeled_asset("layer_array".to_string(), layer_array),
            alpha_array: ctx.add_labeled_asset("alpha_array".to_string(), alpha_array),
            shadow_array: ctx.add_labeled_asset("shadow_array".to_string(), shadow_array),
            doodads: tile.doodads,
            wmos: tile.wmos,
            chunks: tile.chunks,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["adt"]
    }
}

/// Read a layer texture in its authored form. Prefers the `_s` specular variant (sheen mask in
/// alpha); else falls back to the base BLP, flagged `matte` so no sheen rides it. `key` is a
/// normalized internal path.
///
/// **Native, not decoded**: the blocks are kept here so [`pack_layers`] still has the choice. A
/// Raw1/Raw3 BLP has no block form and comes back decoded anyway, which is why the caller never has
/// to ask — it is [`benilla_formats::BlpMipChain::texels`] that says what arrived.
async fn read_layer(ctx: &mut LoadContext<'_>, key: &str) -> Option<RawLayer> {
    if let Some(spec) = key.strip_suffix(".blp").map(|stem| format!("{stem}_s.blp")) {
        if let Ok(bytes) = ctx.read_asset_bytes(mpq_url(&spec)).await {
            if let Ok(chain) = blp_bytes_to_native_chain(&bytes) {
                return Some(RawLayer {
                    chain,
                    matte: false,
                });
            }
        }
    }
    let bytes = ctx.read_asset_bytes(mpq_url(key)).await.ok()?;
    let chain = blp_bytes_to_native_chain(&bytes).ok()?;
    Some(RawLayer { chain, matte: true })
}

/// An internal (`\`/lowercase) path → an `mpq://` URL.
fn mpq_url(key: &str) -> String {
    format!("mpq://{}", key.replace('\\', "/"))
}

/// Normalize an internal asset path so case/slash variants share one layer-array index.
fn normalize_path(path: &str) -> String {
    path.replace('/', "\\").to_ascii_lowercase()
}

/// Flat per-chunk normals for the rare chunk lacking authored MCNR (most carry it).
fn computed_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut acc = vec![Vec3::ZERO; positions.len()];
    for tri in indices.as_chunks::<3>().0 {
        let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let (va, vb, vc) = (
            Vec3::from(positions[a]),
            Vec3::from(positions[b]),
            Vec3::from(positions[c]),
        );
        let n = (vb - va).cross(vc - va);
        acc[a] += n;
        acc[b] += n;
        acc[c] += n;
    }
    acc.into_iter()
        .map(|n| n.normalize_or_zero().to_array())
        .collect()
}
