//! `benilla-assets` — the bridge from the WoW 1.12.1 MPQ patch chain into Bevy's asset system.
//!
//! Registers an `mpq://` [`AssetSource`](bevy::asset::io::AssetSource) backed by
//! [`benilla_formats::Chain`], so every WoW asset loads through the standard
//! [`AssetServer`](bevy::asset::AssetServer) as a `Handle<T>` — gaining async loading, handle dedup,
//! a dependency graph, and hot-reload for free, instead of the bespoke caches and hand-rolled
//! worker/finalize pipeline the old client carried. The per-format
//! [`AssetLoader`](bevy::asset::AssetLoader)s (BLP→`Image`, M2/WMO→model, ADT→tile, DBC→catalog)
//! build on this foundation.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use benilla_formats::Chain;
use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSourceBuilder, ErasedAssetReader, PathStream, Reader,
    VecReader,
};
use bevy::asset::AssetApp;
use bevy::prelude::*;

pub mod column_grid;
pub mod coords;
pub mod materials;
pub mod minimap_grid;
mod spatial_cache;
pub mod trace;

mod anim_rng;
pub use anim_rng::AnimRng;
pub use spatial_cache::SpatialCache;
mod world_assets;
pub use world_assets::*;

mod model;
pub use model::{
    bone_target_id, merged_static_mesh_faded, submesh_to_skinned_mesh, submesh_to_static_mesh,
    AnimClip, BillboardInfo, ClipEvent, GlobalBone, GlobalSeqChannel, ModelAnimations,
    ModelAttachment, ModelJoint, ModelMarker, ModelSkeleton, ModelSubmesh, PoseBone, PoseClip,
    PoseNode, PoseSource, PoseTrack, ATTRIBUTE_WOW_FADE_SPHERE, ATTRIBUTE_WOW_JOINT_INDEX,
    ATTRIBUTE_WOW_JOINT_WEIGHT, ATTRIBUTE_WOW_MERGED_SLOT,
};
mod adt;
mod terrain;
mod wdt;
pub use adt::{chunk_to_mesh, chunks_to_mesh, AdtLoader, AdtTile, ChunkShading};
pub use wdt::{WdtIndex, WdtIndexLoader};
mod blp;
pub use blp::{BlpImageLoader, BlpLoaderSettings, BlpVariant};
/// Whether this run's GPU can eat WoW's stored DXT blocks, and the chain form that follows.
mod gpu_blp;
pub use gpu_blp::{bc_supported, for_upload, publish_bc_support, BlpGpuSupportPlugin, UploadChain};
mod tex_filter;
pub use tex_filter::{publish_tex_filter, tex_filter, TexFilterSetting, ANISO_RANGE};
mod m2;
pub use m2::{
    EmitterBillboard, M2Model, M2ModelLoader, M2SequenceInfo, ModelEmitter, ModelLight,
    ModelRibbon, PortraitCamera,
};
mod wmo;
pub use benilla_formats::{WmoPortalInfo, WmoPortalRef};
pub use wmo::{
    cap96, collision_tri_bounds, collision_tri_grids, floor168, footprint_tri_bounds,
    footprint_tri_grids, DoodadBase, WmoGroupNav, WmoModel, WmoModelLoader,
};

/// The asset-source id for MPQ-backed assets: load paths look like `mpq://World/Azeroth/foo.adt`.
pub const MPQ_SOURCE: &str = "mpq";

/// A Bevy [`AssetReader`] over the vanilla MPQ patch chain. Cheap to clone — every clone shares the
/// one open chain ([`Chain`] reads through fresh per-call handles, so this is `Send + Sync`).
#[derive(Clone)]
pub struct MpqAssetReader {
    chain: Arc<Chain>,
}

impl MpqAssetReader {
    /// Open the patch chain from a `Data` directory (or a single `.MPQ`), ready to back an
    /// `mpq://` source.
    pub fn open(data_dir: &Path) -> Result<Self> {
        Ok(Self {
            chain: Arc::new(Chain::open(data_dir)?),
        })
    }
}

impl AssetReader for MpqAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let raw = path
            .to_str()
            .ok_or_else(|| AssetReaderError::NotFound(path.to_path_buf()))?;
        // Strip the sampler-mode marker ([`texture_url`]) before the archive lookup — it is part of
        // the ASSET identity, not of the file name.
        let stripped = strip_sampler_marker(raw);
        let internal = stripped.as_deref().unwrap_or(raw);
        if !self.chain.contains(internal) {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        }
        let bytes = self
            .chain
            .read(internal)
            .map_err(|e| AssetReaderError::Io(Arc::new(std::io::Error::other(format!("{e:#}")))))?;
        Ok(VecReader::new(bytes))
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        // MPQ assets carry no sidecar `.meta`; Bevy falls back to default meta on `NotFound`.
        Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        // We load by explicit path; directory enumeration / folder-watching isn't supported.
        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn is_directory<'a>(&'a self, _path: &'a Path) -> Result<bool, AssetReaderError> {
        Ok(false)
    }
}

/// The sampler-mode marker separator in an `mpq://` texture URL (see [`texture_url`]).
const SAMPLER_MARKER: char = '@';

/// Build the `mpq://` URL for a model texture at a given sampler address mode.
///
/// The address mode lives on the GPU sampler, which in Bevy rides the `Image`, which is keyed by
/// asset path — so two modes of one `.blp` need two asset paths. **243 of the corpus's 4677 texture
/// paths are asked for more than one mode** (`benilla-extract texmodescan`), so this is not a
/// hypothetical: without it, whichever model loaded a shared sheet first would decide the mode for
/// every later one.
///
/// Repeat/repeat — the overwhelming majority — keeps the bare path, so the common case is one upload
/// and every pre-existing URL is unchanged. Any other mode gets a marker **before the extension**
/// (`…\leaves01@cc.blp`), because Bevy selects the loader by extension and a trailing marker would
/// stop `.blp` resolving. [`MpqAssetReader::read`] strips it back off for the archive lookup.
/// Decision 0763.
pub fn texture_url(internal: &str, wrap: (bool, bool)) -> String {
    let path = internal.replace('\\', "/").to_ascii_lowercase();
    if wrap == (true, true) {
        return format!("mpq://{path}");
    }
    let tag = match wrap {
        (true, false) => "rc",
        (false, true) => "cr",
        _ => "cc",
    };
    match path.rsplit_once('.') {
        Some((stem, ext)) => format!("mpq://{stem}{SAMPLER_MARKER}{tag}.{ext}"),
        None => format!("mpq://{path}{SAMPLER_MARKER}{tag}"),
    }
}

/// Bevy resource wrapper around the format crate's [`MapCatalog`] (`mapId` -> directory +
/// `LoadingScreenID`), read out of `Map.dbc`.
///
/// The newtype exists because the orphan rule forbids `Resource` on a foreign type, and it lives
/// here rather than in the client for the same reason the rest of this crate does: a `.dbc` table
/// turned into something Bevy can hold is exactly this layer's job, and the three readers are the
/// WDL streamer, the world-map UI and the loading screen — one engine, two game.
/// The *loader* stays up top with the patch chain's plugin shell, which is what inserts it.
#[derive(Resource)]
pub struct MapCatalogRes(pub benilla_formats::MapCatalog);

/// The `mpq://` URL an authored **model** path resolves to.
///
/// The archive holds 1.12.1 `.mdx`/`.mdl`, the loader produces an `M2Model`, and Bevy picks a loader
/// by extension — so every reference is normalised to one lowercase `mpq://…m2`, which is also what
/// makes the handle dedup work (a path differing only in case or slash would load twice). The WMO and
/// skin builders are the same rewrite for their own extensions. They sit beside [`texture_url`]
/// because they are the same act: turning a path the game files wrote into the one URL this crate's
/// asset source answers to.
/// A model reference path (`.mdx`/`.mdl`, mixed case, backslashes) → its `mpq://…m2` load URL.
/// Lowercased so case variants share one `AssetServer` handle; the physical archive file is `.m2`.
pub fn m2_url(raw: &str) -> String {
    let p = raw.to_ascii_lowercase().replace('\\', "/");
    let stem = p
        .strip_suffix(".mdx")
        .or_else(|| p.strip_suffix(".mdl"))
        .or_else(|| p.strip_suffix(".m2"))
        .unwrap_or(&p);
    format!("mpq://{stem}.m2")
}

/// A WMO root path → its `mpq://…wmo` load URL (already `.wmo`; lowercased for handle dedup).
pub fn wmo_url(raw: &str) -> String {
    format!("mpq://{}", raw.to_ascii_lowercase().replace('\\', "/"))
}

/// A creature skin variation → its `mpq://…blp` URL: `<model-dir>\<name>.blp`. `model_dir` is the
/// directory of the creature's model path (where its `Monster1/2/3` skins live).
pub fn skin_url(model_dir: &str, name: &str) -> String {
    let dir = model_dir.replace('\\', "/").to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    if dir.is_empty() {
        format!("mpq://{name}.blp")
    } else {
        format!("mpq://{dir}/{name}.blp")
    }
}

/// The address mode encoded in an asset path by [`texture_url`]; repeat/repeat when unmarked.
pub fn sampler_mode_of(path: &str) -> (bool, bool) {
    let Some((stem, _)) = path.rsplit_once('.') else {
        return (true, true);
    };
    match stem.rsplit_once(SAMPLER_MARKER) {
        Some((_, "rc")) => (true, false),
        Some((_, "cr")) => (false, true),
        Some((_, "cc")) => (false, false),
        _ => (true, true),
    }
}

/// Remove a [`texture_url`] marker, yielding the real archive path. `None` when there is none.
fn strip_sampler_marker(path: &str) -> Option<String> {
    let (stem, ext) = path.rsplit_once('.')?;
    let (base, tag) = stem.rsplit_once(SAMPLER_MARKER)?;
    matches!(tag, "rc" | "cr" | "cc").then(|| format!("{base}.{ext}"))
}

/// Register the `mpq://` asset source on `app`, backed by the patch chain at `data_dir`.
///
/// Must be called **before** Bevy's `AssetPlugin` builds (i.e. before `DefaultPlugins`): asset
/// sources are read when the plugin initializes the [`AssetServer`](bevy::asset::AssetServer).
pub fn register_mpq_source(app: &mut App, data_dir: &Path) -> Result<()> {
    let reader = MpqAssetReader::open(data_dir)?;
    app.register_asset_source(
        MPQ_SOURCE,
        AssetSourceBuilder::new(move || -> Box<dyn ErasedAssetReader> { Box::new(reader.clone()) }),
    );
    Ok(())
}

/// Register benilla's asset loaders on `app`. Call **after** Bevy's `AssetPlugin` (loaders register
/// into the live [`AssetServer`](bevy::asset::AssetServer)); pair with [`register_mpq_source`], which
/// must run *before* `AssetPlugin`.
pub fn register_asset_loaders(app: &mut App) {
    app.init_asset::<M2Model>();
    app.init_asset::<WmoModel>();
    app.init_asset::<AdtTile>();
    app.init_asset::<WdtIndex>();
    // Publishes whether this device can eat DXT blocks. A plugin, because the
    // answer only exists after `RenderPlugin::finish` — see `BlpGpuSupportPlugin`.
    app.add_plugins(BlpGpuSupportPlugin);
    app.register_asset_loader(BlpImageLoader);
    app.register_asset_loader(M2ModelLoader);
    app.register_asset_loader(WmoModelLoader);
    app.register_asset_loader(AdtLoader);
    app.register_asset_loader(WdtIndexLoader);
    // The render materials' WGSL, compiled in rather than served off the host binary's asset root
    // — same "after AssetPlugin" requirement as the loaders above, so it rides here.
    materials::register_shaders(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::primitives::MeshAabb;
    use bevy::tasks::block_on;

    /// The sampler-mode URL round-trips, and repeat/repeat stays byte-identical to the old bare
    /// path — so the common case keeps ONE upload and no pre-existing URL changed.
    #[test]
    fn sampler_mode_rides_the_asset_path_and_round_trips() {
        let tex = "World\\KhazModan\\Ironforge\\PassiveDoodads\\Trees\\IronForgeleaves01.blp";
        // The default is the bare path — unchanged from before this scheme existed.
        let repeat = texture_url(tex, (true, true));
        assert_eq!(
            repeat,
            "mpq://world/khazmodan/ironforge/passivedoodads/trees/ironforgeleaves01.blp"
        );
        assert_eq!(sampler_mode_of(&repeat), (true, true));
        // Every other mode marks the STEM, so the `.blp` extension still selects the loader.
        for wrap in [(false, false), (true, false), (false, true)] {
            let url = texture_url(tex, wrap);
            assert!(url.ends_with(".blp"), "extension must survive: {url}");
            assert_ne!(url, repeat, "a marked mode is a distinct asset path");
            assert_eq!(sampler_mode_of(&url), wrap, "round-trip {wrap:?}");
            // ...and the marker comes back off for the archive lookup.
            assert_eq!(
                strip_sampler_marker(url.strip_prefix("mpq://").unwrap()).as_deref(),
                Some("world/khazmodan/ironforge/passivedoodads/trees/ironforgeleaves01.blp"),
            );
        }
        // A bare path has nothing to strip, and an unrelated `@` is not a marker.
        assert_eq!(strip_sampler_marker("world/foo.blp"), None);
        assert_eq!(strip_sampler_marker("world/foo@bar.blp"), None);
        assert_eq!(sampler_mode_of("world/foo@bar.blp"), (true, true));
    }

    #[test]
    fn mpq_reader_loads_real_client_bytes_through_the_assetreader() {
        let data = benilla_formats::wow_data_or_skip!();
        let reader = MpqAssetReader::open(&data).expect("open mpq reader");

        // A real file resolves and drains to its bytes through the Bevy AssetReader/Reader traits.
        let bytes = block_on(async {
            let mut r = AssetReader::read(&reader, Path::new("DBFilesClient/Spell.dbc"))
                .await
                .expect("read Spell.dbc via AssetReader");
            let mut buf = Vec::new();
            r.read_to_end(&mut buf).await.expect("drain reader");
            buf
        });
        assert_eq!(&bytes[..4], b"WDBC", "Spell.dbc starts with the WDBC magic");
        assert!(bytes.len() > 1_000_000, "Spell.dbc should be sizable");

        // A missing path yields NotFound — the variant Bevy relies on for meta fallback.
        let missing = block_on(AssetReader::read(&reader, Path::new("does/not/exist.blp")));
        assert!(
            matches!(missing, Err(AssetReaderError::NotFound(_))),
            "missing path should be NotFound"
        );
    }

    #[test]
    fn loads_a_blp_image_through_the_full_mpq_pipeline() {
        let data = benilla_formats::wow_data_or_skip!();
        // Full pipeline, headless: mpq:// source → AssetServer → BlpImageLoader → Handle<Image>.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        register_mpq_source(&mut app, &data).expect("register mpq source");
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        register_asset_loaders(&mut app);

        let handle: Handle<Image> = app
            .world()
            .resource::<AssetServer>()
            .load("mpq://Interface/Icons/Spell_Holy_ArcaneIntellect.blp");

        let mut got = None;
        // Same generous ceiling as the WMO test below — parallel-session load starves the IO pool.
        for _ in 0..15_000 {
            app.update();
            if let Some(img) = app.world().resource::<Assets<Image>>().get(&handle) {
                got = Some((
                    img.width(),
                    img.height(),
                    img.texture_descriptor.mip_level_count,
                ));
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let (w, h, mips) =
            got.expect("the spell icon should load via mpq:// + BlpImageLoader within 300 ticks");
        assert_eq!((w, h), (64, 64), "spell icon is 64x64");
        assert!(
            mips >= 1,
            "authored mip levels should be present, got {mips}"
        );
    }

    /// Regression guard: `load_with_settings(Sprite)` must reach the loader on the async `mpq://`
    /// path so a streamed tile lands as `Rgba8UnormSrgb`, not the loader's `WorldArt` default
    /// (`Rgba8Unorm`). The minimap streams its tiles this way (`minimap.rs`), and the UI pass's
    /// contract is sRGB textures (linearize → multiply → re-encode); a silent regression to the
    /// gamma-space default re-encodes every tile ~2× brighter — the decision-0178 over-bright class
    /// of bug, but for the minimap. Skips without the vanilla client data.
    #[test]
    fn minimap_tile_settings_reach_the_async_loader() {
        use bevy::render::render_resource::TextureFormat;
        let data = benilla_formats::wow_data_or_skip!();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        register_mpq_source(&mut app, &data).expect("register mpq source");
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        register_asset_loaders(&mut app);

        // IN ISOLATION — the real app (minimap.rs) only ever loads a tile via
        // `load_with_settings(Sprite)`, never a bare `load()` of the same path. Loading both here
        // would let Bevy's path-dedup hand the second call the first's settings, masking the truth.
        let tile = "mpq://textures/Minimap/ea283abc0bf9637c3fad5e840a65b38b.blp";
        let server = app.world().resource::<AssetServer>().clone();
        let sprite_h: Handle<Image> =
            server.load_with_settings(tile, |s: &mut BlpLoaderSettings| {
                s.variant = BlpVariant::Sprite;
            });

        let mut sprite_fmt = None;
        // Same generous ceiling as the WMO test below — parallel-session load starves the IO pool.
        for _ in 0..15_000 {
            app.update();
            if let Some(img) = app.world().resource::<Assets<Image>>().get(&sprite_h) {
                sprite_fmt = Some(img.texture_descriptor.format);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        eprintln!("Sprite settings (isolated) -> {sprite_fmt:?}");
        assert_eq!(
            sprite_fmt,
            Some(TextureFormat::Rgba8UnormSrgb),
            "load_with_settings(Sprite) must produce an sRGB tile on the async mpq:// path"
        );
    }

    /// The WMO interior-minimap tile grid (`crate::minimap::group_axis_grid`) verified against real
    /// authored data: load Ironforge and check the per-group tile count each axis matches the
    /// `md5translate.trs` ground truth — group 66 = 2×2, 44 = 1(X)×2(Y), 89 = 2(X)×1(Y), and the
    /// small groups 1×1. Grounds the (RE-inferred) footprint→grid bake the interior renderer rests on.
    #[test]
    fn ironforge_group_grid_matches_trs() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        register_mpq_source(&mut app, &data).expect("mpq");
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        app.init_asset::<Mesh>();
        register_asset_loaders(&mut app);
        let h: Handle<WmoModel> = app
            .world()
            .resource::<AssetServer>()
            .load("mpq://World/wmo/KhazModan/Cities/Ironforge/Ironforge.wmo");
        let mut model = None;
        // A generous ~30 s ceiling (healthy runs finish in well under a second): the async IO
        // pool gets starved when a parallel session runs its own full gates on this machine, and
        // the old ~4 s budget flaked exactly then (2026-07-10, two concurrent workspace runs).
        for _ in 0..15_000 {
            app.update();
            if let Some(m) = app.world().resource::<Assets<WmoModel>>().get(&h) {
                model = Some(m.clone());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let m = model.expect("Ironforge loads");
        // (group index, expected X-count, expected Y-count) — read off the trs tile names.
        let grid = |ext: f32| crate::minimap_grid::group_axis_grid(ext).0;
        for &(g, ex_n, ey_n) in &[
            (1, 1, 1),
            (2, 1, 1),
            (10, 1, 1),
            (44, 1, 2),
            (66, 2, 2),
            (89, 2, 1),
        ] {
            let gn = m.group_nav.get(g).expect("group present");
            let nx = grid(gn.bbox_max[0] - gn.bbox_min[0]);
            let ny = grid(gn.bbox_max[1] - gn.bbox_min[1]);
            assert_eq!((nx, ny), (ex_n, ey_n), "group {g} grid");
        }
    }

    #[test]
    fn loads_an_m2_model_through_the_pipeline() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        register_mpq_source(&mut app, &data).expect("register mpq source");
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        app.init_asset::<Mesh>();
        // The M2 loader emits labeled sub-assets the skinned + animated path needs: the
        // inverse bind poses, plus the idle `AnimationClip` + `AnimationGraph`. The real app registers
        // these via `DefaultPlugins` (bevy_mesh's `MeshPlugin` + `AnimationPlugin`); the minimal harness
        // registers them here so the campfire's M2 (which has bones + a Stand sequence) loads.
        app.init_asset::<bevy::mesh::skinning::SkinnedMeshInverseBindposes>();
        app.init_asset::<bevy::animation::AnimationClip>();
        app.init_asset::<bevy::animation::graph::AnimationGraph>();
        register_asset_loaders(&mut app); // inits M2Model + registers BLP/M2 loaders

        // A doodad with embedded (hardcoded) textures — the campfire (`.mdx` ref → `.m2` physical file).
        let handle: Handle<M2Model> = app
            .world()
            .resource::<AssetServer>()
            .load("mpq://World/Azeroth/Elwynn/PassiveDoodads/Campfire/ElwynnCampfire.m2");

        let mut info = None;
        for _ in 0..600 {
            app.update();
            if let Some(m) = app.world().resource::<Assets<M2Model>>().get(&handle) {
                let geometries: Vec<_> = m.submeshes.iter().map(|s| s.geometry.clone()).collect();
                let textured = m.submeshes.iter().filter(|s| s.texture.is_some()).count();
                info = Some((geometries, textured, m.bounds.is_some()));
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let (geometries, textured, has_bounds) =
            info.expect("the campfire M2 should load via mpq:// + M2ModelLoader within 600 ticks");
        assert!(
            !geometries.is_empty(),
            "campfire should have render batches"
        );
        assert!(has_bounds, "M2 carries authored bounds");
        assert!(textured > 0, "campfire batches reference embedded textures");

        // The loader ships geometry, no meshes — the app builds the render form.
        // Exercise both builders per batch: non-empty, and the static form must yield the Aabb
        // the spawn side inserts explicitly (RENDER_WORLD meshes race `calculate_bounds`).
        for g in &geometries {
            let mesh = submesh_to_static_mesh(g);
            assert!(mesh.count_vertices() > 0, "submesh has vertices");
            assert!(mesh.compute_aabb().is_some(), "static form yields an Aabb");
            let skinned = submesh_to_skinned_mesh(g);
            assert_eq!(
                skinned.count_vertices(),
                mesh.count_vertices(),
                "the skinned twin bakes the same geometry"
            );
        }
    }

    #[test]
    fn loads_a_wmo_model_through_the_pipeline() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        register_mpq_source(&mut app, &data).expect("register mpq source");
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        app.init_asset::<Mesh>();
        register_asset_loaders(&mut app);

        // The Goldshire Inn — a root + group WMO; the loader reads the groups via read_asset_bytes.
        let handle: Handle<WmoModel> = app
            .world()
            .resource::<AssetServer>()
            .load("mpq://World/wmo/Azeroth/Buildings/GoldshireInn/GoldshireInn.wmo");

        let mut info = None;
        for _ in 0..600 {
            app.update();
            if let Some(m) = app.world().resource::<Assets<WmoModel>>().get(&handle) {
                let geometries: Vec<_> = m.submeshes.iter().map(|s| s.geometry.clone()).collect();
                let textured = m.submeshes.iter().filter(|s| s.texture.is_some()).count();
                info = Some((geometries, textured));
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let (geometries, textured) =
            info.expect("the Goldshire Inn WMO should load via mpq:// + WmoModelLoader");
        assert!(
            !geometries.is_empty(),
            "the inn has render batches across its groups"
        );
        assert!(textured > 0, "WMO batches reference textures");

        // Geometry, no meshes: the static build is the WMO's one render form.
        for g in &geometries {
            let mesh = submesh_to_static_mesh(g);
            assert!(mesh.count_vertices() > 0, "group submesh has vertices");
            assert!(mesh.compute_aabb().is_some(), "static form yields an Aabb");
        }
    }

    #[test]
    fn loads_an_adt_terrain_tile_through_the_pipeline() {
        let data = benilla_formats::wow_data_or_skip!();
        // Find an existing Elwynn-area Azeroth tile (don't hardcode exact coords).
        let reader = benilla_formats::Chain::open(&data).expect("open chain");
        let mut url = None;
        'find: for tx in 28..36u32 {
            for ty in 46..52u32 {
                if reader.contains(&format!("World\\Maps\\Azeroth\\Azeroth_{tx}_{ty}.adt")) {
                    url = Some(format!("mpq://World/Maps/Azeroth/Azeroth_{tx}_{ty}.adt"));
                    break 'find;
                }
            }
        }
        let url = url.expect("an Azeroth ADT tile should exist near Elwynn");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        register_mpq_source(&mut app, &data).expect("register mpq source");
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        app.init_asset::<Mesh>();
        register_asset_loaders(&mut app);

        let handle: Handle<AdtTile> = app.world().resource::<AssetServer>().load(url);
        let mut info = None;
        for _ in 0..2000 {
            app.update();
            if let Some(t) = app.world().resource::<Assets<AdtTile>>().get(&handle) {
                info = Some((
                    t.chunks
                        .iter()
                        .zip(&t.shading)
                        .map(|(c, s)| (c.clone(), *s))
                        .collect::<Vec<_>>(),
                    t.shading.len(),
                    t.layer_array.clone(),
                    t.alpha_array.clone(),
                    t.shadow_array.clone(),
                    t.doodads.len(),
                ));
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let (cells, n_shading, layer_h, alpha_h, shadow_h, n_doodads) =
            info.expect("the ADT tile should load via mpq:// + AdtLoader");

        // One drawn mesh per MCNK cell, never one merged slab — the exterior-scene
        // cull's unit is the chunk, and a tile that loads as a single object cannot be culled from
        // inside a building. The loader ships the shading (index-parallel with the chunks); the
        // mesh itself is the app's paced `chunk_to_mesh` build — exercised per cell here. Every
        // drawn cell must be a 145-vertex 9×9+8×8 grid.
        assert_eq!(n_shading, cells.len(), "one ChunkShading per decoded chunk");
        let drawn: Vec<Mesh> = cells
            .iter()
            .filter_map(|(c, s)| chunk_to_mesh(c, s))
            .collect();
        assert!(
            (1..=256).contains(&drawn.len()),
            "a tile draws as its MCNK cells, got {} meshes",
            drawn.len()
        );
        for mesh in &drawn {
            assert_eq!(
                mesh.count_vertices(),
                145,
                "an MCNK cell is 9×9 + 8×8 vertices"
            );
            assert!(
                mesh.compute_aabb().is_some(),
                "a cell mesh must yield the Aabb the exterior cull fails open without"
            );
        }

        let images = app.world().resource::<Assets<Image>>();
        let layer = images.get(&layer_h).expect("layer array present");
        assert_eq!(
            layer.texture_descriptor.size.width, 256,
            "layer array packed at LAYER_TEX_SIZE"
        );
        assert!(
            layer.texture_descriptor.mip_level_count > 1,
            "layer array carries the authored mip chain"
        );
        assert!(images.get(&alpha_h).is_some(), "alpha array present");
        assert!(images.get(&shadow_h).is_some(), "shadow array present");
        // Placement lists are carried as data (count is tile-dependent — some tiles are bare).
        eprintln!(
            "ADT tile loaded: {} MCNK cells, {n_doodads} doodad placements",
            drawn.len()
        );
    }
}
