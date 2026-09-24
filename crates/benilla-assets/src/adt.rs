//! ADT terrain tile loader: the decoded MCNK chunks with a [`ChunkShading`] each, the tile's three
//! `texture_2d_array`s (ground layers, per-chunk alpha, per-chunk MCSH shadow) and the raw doodad
//! and WMO placements. It builds no meshes: a tile's chunk meshes built here would all land in one
//! frame, so the app builds them a few per frame. Every vertex carries its chunk's array indices,
//! `COLOR` the 4 layers and `UV1` the alpha (`.x`) and shadow (`.y`, `-1` none), so a whole tile
//! shares one material.

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

/// A loaded ADT tile; the app builds its meshes with [`chunk_to_mesh`].
#[derive(Asset, TypePath)]
pub struct AdtTile {
    /// The array indices of each entry of `chunks`; a hole-emptied chunk carries the default.
    pub shading: Vec<ChunkShading>,
    /// `texture_2d_array` of the tile's ground textures, authored mips verbatim.
    pub layer_array: Handle<Image>,
    /// `texture_2d_array` of per-chunk alpha (blend-weight) maps.
    pub alpha_array: Handle<Image>,
    /// `texture_2d_array` of per-chunk MCSH baked shadow maps.
    pub shadow_array: Handle<Image>,
    /// M2 doodad placements (raw WoW coords).
    pub doodads: Vec<Doodad>,
    /// WMO building placements (raw WoW coords).
    pub wmos: Vec<WmoInstance>,
    /// The decoded MCNK chunks, kept resident: the app derives the collider, the MCLQ liquids and
    /// the ground clutter from them.
    pub chunks: Vec<ChunkMesh>,
}

/// One drawn chunk's array indices, uniform over its vertices.
#[derive(Clone, Copy)]
pub struct ChunkShading {
    /// Up to 4 layer-array indices (vertex `COLOR`); an unused slot repeats slot 0 and never shows,
    /// as its alpha weight is 0.
    pub layers: [u32; 4],
    /// The alpha-array index (vertex `UV1.x`); 0 when the chunk has no map.
    pub alpha: u32,
    /// The shadow-array index (vertex `UV1.y`); `-1.0` when there is no MCSH map.
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

/// One drawn MCNK chunk's mesh in Bevy space and absolute world coords; `None` for a chunk the
/// hole mask emptied. The reference's exterior cull tests one AABB per 33.333 yd chunk
/// (`0x683bf0`). The mesh is `RENDER_WORLD` only, so the caller takes its `Aabb` first
/// (`Mesh::compute_aabb`): the exterior cull fails open without one.
pub fn chunk_to_mesh(chunk: &ChunkMesh, shading: &ChunkShading) -> Option<Mesh> {
    chunks_to_mesh(&[(chunk, shading)])
}

/// [`chunk_to_mesh`] over several chunks of one tile as one mesh, the terrain cell: the indices
/// the shader reads are per vertex and the tile shares one material, so the merge draws the same
/// pixels in one draw. `None` when every chunk is a hole.
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

/// Loads a `*.adt` tile into an [`AdtTile`].
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

        // Layer array: a solid-green fallback at index 0, then each unique layer texture, packed
        // once the whole tile is read.
        let mut layers: Vec<RawLayer> = Vec::new();
        let mut layer_index: HashMap<String, u32> = HashMap::new();

        let mut alpha_buf: Vec<u8> = Vec::new();
        let mut alpha_count = 0u32;
        let mut shadow_buf: Vec<u8> = Vec::new();
        let mut shadow_count = 0u32;

        let mut shading: Vec<ChunkShading> = Vec::with_capacity(tile.chunks.len());

        for chunk in tile.chunks.iter() {
            // A hole-emptied chunk takes no alpha or shadow slot either.
            if chunk.indices.len() < 3 {
                shading.push(ChunkShading::default());
                continue;
            }
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
        // Per tile, so a log shows which tiles fell back to the decoded format.
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

/// Read a layer texture with its blocks intact, preferring the `_s` specular variant (sheen mask in
/// alpha); the base BLP alone is flagged `matte`.
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

/// An internal path as an `mpq://` URL.
fn mpq_url(key: &str) -> String {
    format!("mpq://{}", key.replace('\\', "/"))
}

/// Normalize an internal asset path so case and slash variants share one layer-array index.
fn normalize_path(path: &str) -> String {
    path.replace('/', "\\").to_ascii_lowercase()
}

/// Normals for the rare chunk without authored MCNR.
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
