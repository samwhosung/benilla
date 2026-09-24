//! The shared asset store: the one open patch chain, the dedup caches every model and texture load
//! goes through, and the helpers that turn BLP pixels into textures and per-submesh materials. It
//! reads no game state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_resource::{Buffer, Face};

use crate::materials::{WowModelExt, WowModelMaterial, VANILLA_ALPHA_KEY_REF};
use crate::SpatialCache;
use benilla_formats::{blp_to_rgba, read_texture_native_chain, tga_to_rgba, Chain, ModelBlend};

/// BLP and RGBA to `Image` helpers, re-exported.
mod images;
pub use images::*;

/// The shared asset store: the one open patch chain and the dedup caches, so each BLP decodes once
/// and submeshes sharing a texture and blend share a material that Bevy batches.
#[derive(Resource)]
pub struct WorldAssets {
    /// The one open patch chain. The `Mutex` is `Send + Sync` glue for the streaming loaders'
    /// `&mut Chain`, not a read serializer: `Chain` reads are `&self` on a fresh OS handle.
    pub chain: Arc<Mutex<Chain>>,
    /// Decoded GPU textures by `(normalized path, wrap_u, wrap_v)`: one BLP can be sampled both
    /// ways (tiled on a wall, clamped on a cutout card), and the address mode lives on the sampler.
    pub textures: SpatialCache<(String, bool, bool), Handle<Image>>,
    /// UI sprites by resolved path, misses cached as `None` so a bad path never re-walks the chain
    /// per frame.
    sprites: HashMap<String, Option<Handle<Image>>>,
    /// UI sprites by path and per-axis wrap, since the sampler bakes into the `Image`.
    tiled_sprites: HashMap<(String, bool, bool), Option<Handle<Image>>>,
    /// Sprites resampled to an exact physical size, by `(path, w, h)`.
    resampled_sprites: HashMap<(String, u32, u32), Option<Handle<Image>>>,
    /// The decoded pixels behind [`Self::resampled_sprites`], so a resize does not re-decode.
    resample_sources: HashMap<String, Option<(u32, u32, Vec<u8>)>>,
    /// Portrait sprites, the circular mask baked in ([`portrait_image`]).
    portraits: HashMap<String, Option<Handle<Image>>>,
    /// Coverage masks ([`mask_image`]): the minimap's circular clip.
    masks: HashMap<String, Option<Handle<Image>>>,
    /// UI sprites whose pixels the client computes ([`Self::generated_sprite`]).
    generated: HashMap<&'static str, Handle<Image>>,
    /// The AddOns folder, where `Interface\AddOns\` paths resolve as loose files, as the reference
    /// reads them from the game directory; `None` until the app installs it and in capture runs.
    loose_root: Option<PathBuf>,
    /// Materials deduped by [`MaterialKey`].
    pub model_materials: SpatialCache<MaterialKey, Handle<WowModelMaterial>>,
    /// The shared global-light buffer, cloned into every deduped model material's `light_buf`.
    pub shared_light: Buffer,
}

/// Identity of a deduped model material; everything textureless shares one fallback per WMO flag.
/// The cutoff and fade distance are in it so ground clutter never shares a tree's material.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum MaterialKey {
    /// `(texture, blend, two_sided, alpha_key_u8, fade_far_yd_u16, is_wmo, is_fade_variant,
    /// wrap_u, wrap_v)`; the address mode selects a different `Image` upload.
    Textured(String, ModelBlend, bool, u8, u16, bool, bool, bool, bool),
    Fallback(bool),
}

/// Fold case and slashes so variants of one file share a cache entry, as MPQ lookup does.
pub fn normalize_path(path: &str) -> String {
    path.replace('/', "\\").to_ascii_lowercase()
}

/// The cache key a UI sprite reference folds to: [`normalize_path`], then any three-character
/// extension stripped, as the reference strips it (`0x449590`) and keys its texture cache on the
/// stem. `Foo`, `Foo.blp` and `Foo.tga` are one entry; `Foo.jpeg` keys as itself.
pub fn sprite_key(path: &str) -> String {
    let key = normalize_path(path);
    match key.len().checked_sub(4) {
        Some(dot) if key.as_bytes()[dot] == b'.' => key[..dot].to_string(),
        _ => key,
    }
}

/// The two files a UI sprite reference may live in, in the order of `TextureCreate`'s two-candidate
/// chain (`0x449d90`, extension table `0x835248 = {".tga", ".blp"}`): `.tga` first for a `.tga`
/// reference, `.blp` first otherwise, and a miss on the first is silent. The sweeps that assert our
/// own paths resolve call this too, so they cannot disagree with what draws.
pub fn sprite_candidates(path: &str) -> [String; 2] {
    let stem = sprite_key(path);
    let normalized = normalize_path(path);
    // `normalize_path` has already folded case, so a plain compare is the case-insensitive one.
    if normalized.len() >= 4 && normalized[normalized.len() - 4..] == *".tga" {
        [format!("{stem}.tga"), format!("{stem}.blp")]
    } else {
        [format!("{stem}.blp"), format!("{stem}.tga")]
    }
}

/// The texel size of a UI sprite, which `CSimpleTexture`'s size getters read (`[tex+0x144]`,
/// `[tex+0x148]`; `0x770720`, `0x770790`) for a region with no authored size, one texel per
/// FrameXML unit. Decoded, so layout gets the size the screen shows.
pub fn sprite_dimensions(
    chain: &Mutex<Chain>,
    loose_root: Option<&Path>,
    path: &str,
) -> Option<(u32, u32)> {
    decode_sprite(chain, loose_root, path).map(|(w, h, _)| (w, h))
}

/// Decode the first of [`sprite_candidates`] that reads and decodes, from the chain and then the
/// AddOns folder, as the reference's two loaders fail silently. A miss warns, once per key since
/// every cache stores its misses: a missing sprite draws nothing, and nothing else says why.
fn decode_sprite(
    chain: &Mutex<Chain>,
    loose_root: Option<&Path>,
    path: &str,
) -> Option<(u32, u32, Vec<u8>)> {
    let candidates = sprite_candidates(path);
    let mut chain = chain.lock_recover();
    for candidate in &candidates {
        if let Ok(bytes) = chain.read_file(candidate) {
            if let Ok(decoded) = decode_sprite_bytes(&bytes) {
                return Some(decoded);
            }
        }
        if let Some(file) = loose_root.and_then(|root| loose_addon_file(root, candidate)) {
            if let Ok(decoded) = std::fs::read(&file)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| decode_sprite_bytes(&bytes))
            {
                return Some(decoded);
            }
        }
    }
    // "Not there" and "there but would not decode" are different faults; say which.
    let found_but_undecodable: Vec<&String> = candidates
        .iter()
        .filter(|c| {
            chain.read_file(c).is_ok()
                || loose_root.is_some_and(|root| loose_addon_file(root, c).is_some())
        })
        .collect();
    if found_but_undecodable.is_empty() {
        warn!(
            "texture miss: '{path}' does not resolve in the patch chain{} (tried {})",
            if loose_root.is_some() {
                " or the AddOns folder"
            } else {
                ""
            },
            candidates.join(", ")
        );
    } else {
        warn!(
            "texture miss: '{path}' RESOLVES but will not decode ({}) — the file is there and the \
             decoder refused it",
            found_but_undecodable
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    None
}

/// Decode by content, not extension, since addon folders mislabel both ways: a `BLP2` magic is a
/// BLP, anything else goes to the TGA decoder, whose header check is the gate.
fn decode_sprite_bytes(bytes: &[u8]) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    if bytes.starts_with(b"BLP2") {
        blp_to_rgba(bytes)
    } else {
        tga_to_rgba(bytes)
    }
}

/// Read a file from the patch chain, then, for an `Interface\AddOns\` path, the loose addon folder:
/// how art, fonts and audio an addon ships all load. The reference asks the install tree first
/// (`0x647e60`'s attempt #4 is the MPQ); the chain holds no `AddOns\` path, so the order is moot.
pub fn read_chain_or_loose(
    chain: &Mutex<Chain>,
    loose_root: Option<&Path>,
    path: &str,
) -> Option<Vec<u8>> {
    if let Ok(bytes) = chain.lock_recover().read(path) {
        return Some(bytes);
    }
    let file = loose_addon_file(loose_root?, &normalize_path(path))?;
    std::fs::read(file).ok()
}

/// Map a normalized `interface\addons\…` path onto a file under the addon root, matching each
/// component case-insensitively as the Windows reference does; a dot-component is refused, so a
/// path cannot escape the root.
pub fn loose_addon_file(root: &Path, candidate: &str) -> Option<PathBuf> {
    let rel = candidate.strip_prefix("interface\\addons\\")?;
    let mut at = root.to_path_buf();
    for comp in rel.split('\\') {
        if comp.is_empty() || comp.starts_with('.') {
            return None;
        }
        let direct = at.join(comp);
        if direct.exists() {
            at = direct;
            continue;
        }
        let found = std::fs::read_dir(&at).ok()?.flatten().find(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(comp))
        })?;
        at = found.path();
    }
    at.is_file().then_some(at)
}

/// Mutate an asset only when `differs` says it does not already match: `Assets::get_mut` alone
/// marks it Modified, a uniform re-upload and, on Metal's non-bindless path, a bind-group rebuild.
/// Pair it with values quantized to display precision ([`quantize`], [`quant255`]).
pub fn write_gated<M: bevy::asset::Asset>(
    assets: &mut Assets<M>,
    handle: &Handle<M>,
    differs: impl Fn(&M) -> bool,
    apply: impl FnOnce(&mut M),
) {
    if assets.get(handle).is_none_or(differs) {
        if let Some(m) = assets.get_mut(handle) {
            apply(m);
        }
    }
}

/// Quantize to `n`ths, the write gate's floor for drifting inputs: 255 for colour ([`quant255`]),
/// a sub-pixel step for geometry.
pub fn quantize(x: f32, n: f32) -> f32 {
    (x * n).round() / n
}

/// [`quantize`] each channel to 1/255: the reference packs its sky and celestial colours to bytes.
pub fn quant255(c: [f32; 3]) -> [f32; 3] {
    [
        quantize(c[0], 255.0),
        quantize(c[1], 255.0),
        quantize(c[2], 255.0),
    ]
}

/// Lock a `Mutex`, recovering the guard if a holder panicked, so one failed read of the shared
/// chain does not fail every later lock; a half-read texture is harmless.
pub trait LockRecover<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> LockRecover<T> for Mutex<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl WorldAssets {
    /// Open the store over an opened patch chain. `shared_light` is a raw `Buffer`, not the
    /// client's `SharedLightBuffer`, which keeps this crate below the renderer.
    pub fn open(chain: Chain, shared_light: Buffer) -> Self {
        Self {
            chain: Arc::new(Mutex::new(chain)),
            textures: SpatialCache::default(),
            sprites: HashMap::new(),
            resampled_sprites: HashMap::new(),
            resample_sources: HashMap::new(),
            tiled_sprites: HashMap::new(),
            portraits: HashMap::new(),
            masks: HashMap::new(),
            generated: HashMap::new(),
            loose_root: None,
            model_materials: SpatialCache::default(),
            shared_light,
        }
    }

    /// [`read_chain_or_loose`] over this store, for an asset class with no decoder here (audio:
    /// `PlayMusic`, `PlaySoundFile`).
    pub fn read_file_or_loose(&self, path: &str) -> Option<Vec<u8>> {
        read_chain_or_loose(&self.chain, self.loose_root.as_deref(), path)
    }

    /// Install or clear the AddOns folder. Cached misses are evicted so a path asked before it
    /// existed gets a second look; a cached hit came from the chain and stays right.
    pub fn set_loose_addon_root(&mut self, root: Option<PathBuf>) {
        if self.loose_root == root {
            return;
        }
        self.loose_root = root;
        self.sprites.retain(|_, v| v.is_some());
        self.tiled_sprites.retain(|_, v| v.is_some());
        self.portraits.retain(|_, v| v.is_some());
        self.masks.retain(|_, v| v.is_some());
    }

    /// Decode and upload a BLP once per path and wrap; later requests share the handle.
    pub fn texture(
        &mut self,
        path: &str,
        wrap: (bool, bool),
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let key = (normalize_path(path), wrap.0, wrap.1);
        if let Some(handle) = self.textures.fetch(&key) {
            return Some(handle);
        }
        // World art uploads as the BLP stores it; nothing reads it main-side.
        let chain = read_texture_native_chain(&mut self.chain.lock_recover(), &key.0).ok()?;
        let handle = images.add(repeat_texture_authored(crate::for_upload(chain), wrap));
        self.textures.insert(key, handle.clone());
        Some(handle)
    }

    /// A clamp-sampled sRGB sprite, mip 0 only: the celestial discs and every UI quad.
    pub fn sprite_texture(
        &mut self,
        path: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let key = sprite_key(path);
        // Hits and misses are cached: the UI extractor asks per quad per frame, and a fresh handle
        // would defeat its dirty check.
        if let Some(cached) = self.sprites.get(&key) {
            return cached.clone();
        }
        let loaded = decode_sprite(&self.chain, self.loose_root.as_deref(), path)
            .map(|(w, h, rgba)| images.add(sprite_image(w, h, rgba)));
        self.sprites.insert(key, loaded.clone());
        loaded
    }

    /// Decode a UI sprite to raw RGBA8 without uploading or caching, for a caller that resamples.
    pub fn decode_rgba(&mut self, path: &str) -> Option<(u32, u32, Vec<u8>)> {
        decode_sprite(&self.chain, self.loose_root.as_deref(), path)
    }

    /// A UI sprite resampled to an exact pixel size by the caller's kernel, cached by size: the
    /// nameplate border, whose 1 px bevel bilinear magnification would smear.
    pub fn resampled_sprite(
        &mut self,
        path: &str,
        (w, h): (u32, u32),
        images: &mut Assets<Image>,
        resample: impl FnOnce(&[u8], u32, u32, u32, u32) -> Vec<u8>,
    ) -> Option<Handle<Image>> {
        let key = (path.to_string(), w, h);
        if let Some(cached) = self.resampled_sprites.get(&key) {
            return cached.clone();
        }
        if !self.resample_sources.contains_key(path) {
            let decoded = decode_sprite(&self.chain, self.loose_root.as_deref(), path);
            self.resample_sources.insert(path.to_string(), decoded);
        }
        let made = self.resample_sources[path]
            .as_ref()
            .map(|(sw, sh, src)| images.add(sprite_image(w, h, resample(src, *sw, *sh, w, h))));
        self.resampled_sprites.insert(key, made.clone());
        made
    }

    /// A UI sprite repeating on both axes, for a frame `Backdrop`'s tiled pieces (`0x77f0c0`).
    pub fn sprite_texture_tiled(
        &mut self,
        path: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        self.sprite_texture_wrapped(path, (true, true), images)
    }

    /// A UI sprite repeating on the axes `wrap` names ([`sprite_image_wrapped`]).
    pub fn sprite_texture_wrapped(
        &mut self,
        path: &str,
        wrap: (bool, bool),
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let key = (sprite_key(path), wrap.0, wrap.1);
        if let Some(cached) = self.tiled_sprites.get(&key) {
            return cached.clone();
        }
        let loaded = decode_sprite(&self.chain, self.loose_root.as_deref(), path)
            .map(|(w, h, rgba)| images.add(sprite_image_wrapped(w, h, rgba, wrap)));
        self.tiled_sprites.insert(key, loaded.clone());
        loaded
    }

    /// A sprite whose pixels the client computes, built once by `make`: the colour picker's
    /// `<ColorWheelTexture>` and `<ColorValueTexture>`, which name no `file=`.
    pub fn generated_sprite(
        &mut self,
        key: &'static str,
        images: &mut Assets<Image>,
        make: impl FnOnce() -> (u32, u32, Vec<u8>),
    ) -> Handle<Image> {
        if let Some(cached) = self.generated.get(key) {
            return cached.clone();
        }
        let (w, h, rgba) = make();
        let handle = images.add(sprite_image(w, h, rgba));
        self.generated.insert(key, handle.clone());
        handle
    }

    /// A unit-frame portrait, the circular mask baked in ([`portrait_image`]).
    pub fn portrait_texture(
        &mut self,
        path: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let key = sprite_key(path);
        if let Some(cached) = self.portraits.get(&key) {
            return cached.clone();
        }
        let loaded = decode_sprite(&self.chain, self.loose_root.as_deref(), path)
            .map(|(w, h, rgba)| images.add(portrait_image(w, h, rgba)));
        self.portraits.insert(key, loaded.clone());
        loaded
    }

    /// The tabard designer's emblem cell for the caller to tint: white carrying its own alpha,
    /// `(a << 24) | 0x00FFFFFF` per texel (the reference's `0x503431` loop). A file that fails to
    /// open gives `None`, where the reference keeps its static array's stale contents.
    pub fn emblem_mask_texture(
        &mut self,
        path: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let key = format!("emblem-mask:{}", sprite_key(path));
        if let Some(cached) = self.sprites.get(&key) {
            return cached.clone();
        }
        let loaded =
            decode_sprite(&self.chain, self.loose_root.as_deref(), path).map(|(w, h, mut rgba)| {
                for px in rgba.as_chunks_mut::<4>().0 {
                    px[0] = 0xFF;
                    px[1] = 0xFF;
                    px[2] = 0xFF;
                }
                images.add(sprite_image(w, h, rgba))
            });
        self.sprites.insert(key, loaded.clone());
        loaded
    }

    /// A coverage mask, its bytes handed to the shader 1:1 ([`mask_image`]): the minimap's circle.
    pub fn mask_texture(
        &mut self,
        path: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let key = sprite_key(path);
        if let Some(cached) = self.masks.get(&key) {
            return cached.clone();
        }
        let loaded = decode_sprite(&self.chain, self.loose_root.as_deref(), path)
            .map(|(w, h, rgba)| images.add(mask_image(w, h, rgba)));
        self.masks.insert(key, loaded.clone());
        loaded
    }

    /// Decode a cursor BLP (`Interface\Cursor\Point.blp`) for winit's custom OS cursor; macOS
    /// drives `NSCursor` from the raw RGBA instead.
    #[cfg(not(target_os = "macos"))]
    pub fn decode_cursor(
        &mut self,
        path: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let (w, h, rgba) = benilla_formats::read_texture_rgba(
            &mut self.chain.lock_recover(),
            &normalize_path(path),
        )
        .ok()?;
        Some(images.add(cursor_texture(w, h, rgba)))
    }

    /// A model submesh material, deduped by [`MaterialKey`] so submeshes sharing a texture share a
    /// handle and batch; a missing texture falls back to one shared untextured material.
    pub fn model_material(
        &mut self,
        texture: Option<&str>,
        blend: ModelBlend,
        two_sided: bool,
        alpha_cutoff: Option<f32>,
        fade_far: Option<f32>,
        is_wmo: bool,
        is_fade_variant: bool,
        // The batch's authored sampler address mode (`RenderSubmesh::wrap_x/wrap_y`).
        wrap: (bool, bool),
        images: &mut Assets<Image>,
        materials: &mut Assets<WowModelMaterial>,
    ) -> Handle<WowModelMaterial> {
        // Ground clutter passes its own cutoff (128/255); everything else takes the M2 alpha key.
        let cutoff = alpha_cutoff.unwrap_or(VANILLA_ALPHA_KEY_REF);
        // Ground-clutter fade: near is 0.75 of far, the client's ratio.
        let clutter_fade = match fade_far {
            Some(far) => Vec4::new(far * 0.75, far, 0.0, 1.0),
            None => Vec4::ZERO,
        };
        let resolved = texture.and_then(|t| {
            self.texture(t, wrap, images)
                .map(|h| (normalize_path(t), h))
        });
        // Only AlphaTest reads the cutoff, so opaque and blend materials do not split on it.
        let key_alpha = if blend == ModelBlend::AlphaTest {
            (cutoff * 255.0).round() as u8
        } else {
            0
        };
        let key_fade = fade_far.map(|f| f.round() as u16).unwrap_or(0);
        let key = match &resolved {
            Some((path, _)) => MaterialKey::Textured(
                path.clone(),
                blend,
                two_sided,
                key_alpha,
                key_fade,
                is_wmo,
                is_fade_variant,
                wrap.0,
                wrap.1,
            ),
            None => MaterialKey::Fallback(is_wmo),
        };
        if let Some(handle) = self.model_materials.fetch(&key) {
            return handle;
        }
        // This builder serves ground clutter, which never authors Mod/Mod2x; every Mod-capable
        // consumer uses `model_render::model_material`.
        let alpha_mode = match blend {
            ModelBlend::Opaque => AlphaMode::Opaque,
            ModelBlend::AlphaTest => AlphaMode::Mask(cutoff),
            ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x => AlphaMode::Blend,
        };
        // Single-sided unless the M2's 0x04 flag is set, as in the reference.
        let cull_mode = if two_sided { None } else { Some(Face::Back) };
        let base = match resolved {
            Some((_, image)) => StandardMaterial {
                base_color_texture: Some(image),
                alpha_mode,
                double_sided: two_sided,
                cull_mode,
                ..default()
            },
            // No texture: an untextured white draw, the reference's disabled stage (`0x59d020`).
            None => StandardMaterial {
                base_color: Color::WHITE,
                ..default()
            },
        };
        let handle = materials.add(WowModelMaterial {
            base,
            extension: WowModelExt {
                clutter_fade,
                // x = WMO (FFP N·L × MOCV), y = the fade blend twin (no caller passes it here).
                model_flags: Vec4::new(
                    if is_wmo { 1.0 } else { 0.0 },
                    if is_fade_variant { 1.0 } else { 0.0 },
                    0.0,
                    0.0,
                ),
                // Clutter lights on the FFP N·L path with its own MCSH tint; no selector.
                sun_scale: Vec4::new(1.0, 0.0, 0.0, 0.0),
                tint: Vec4::ONE, // clutter has no animated M2Color tint and is not a WMO batch
                sidn: Vec4::ZERO, // clutter is never SIDN/WINDOW glass (WMO-only)
                anim_slots: Vec4::ZERO,
                light_buf: self.shared_light.clone(),
            },
        });
        self.model_materials.insert(key, handle.clone());
        handle
    }
}

/// Render configuration read once from the environment at startup, inserted beside
/// [`WorldAssets`]; its presence is the "there is a client install" gate world setups key on.
/// Terrain residency is not here: it derives from the live `farclip`, as in the reference.
#[derive(Resource, Clone, Copy)]
pub struct RenderConfig {
    /// Stale tiles released per frame on a within-map window shift (`$WOW_TILE_UNLOAD`, default 1;
    /// `0` releases the whole trailing row in one frame, the unbudgeted A/B leg).
    pub unload_budget: usize,
}

/// Startup ordering: the patch chain opens in [`AssetSet::Open`], so other startup systems run
/// after it and borrow [`WorldAssets`].
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetSet {
    Open,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loose_addon_file_maps_addon_paths_case_insensitively() {
        let root =
            std::env::temp_dir().join(format!("benilla-loose-sprite-test-{}", std::process::id()));
        let dir = root.join("Atlas").join("Images").join("Maps");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("BlackrockDepths.blp"), b"x").unwrap();

        // The returned spelling depends on the filesystem's case sensitivity, so check the file.
        let hit = loose_addon_file(
            &root,
            "interface\\addons\\atlas\\images\\maps\\blackrockdepths.blp",
        )
        .expect("mixed-case tree resolves a lowercase candidate");
        assert_eq!(std::fs::read(&hit).unwrap(), b"x");

        assert_eq!(loose_addon_file(&root, "interface\\icons\\foo.blp"), None);
        // A directory is not a file, and a missing file is a miss, not an error.
        assert_eq!(loose_addon_file(&root, "interface\\addons\\atlas"), None);
        assert_eq!(
            loose_addon_file(&root, "interface\\addons\\atlas\\images\\maps\\nope.blp"),
            None
        );
        // Dot-components never reach the filesystem; `normalize_path` already leaves no `/`.
        assert_eq!(
            loose_addon_file(&root, "interface\\addons\\..\\..\\etc\\passwd"),
            None
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// Against the real chain: a client file must still come from the archive, and the loose
    /// folder is a fallback, not a shadow over it.
    #[test]
    fn read_chain_or_loose_asks_the_chain_then_an_addons_own_file_on_disk() {
        let data = benilla_formats::wow_data_or_skip!();
        let chain = Mutex::new(Chain::open(&data).expect("open the chain"));
        let root =
            std::env::temp_dir().join(format!("benilla-loose-audio-test-{}", std::process::id()));
        let addons = root.join("AddOns");
        std::fs::create_dir_all(addons.join("Jukebox")).unwrap();
        std::fs::write(addons.join("Jukebox").join("Track.mp3"), b"ID3!").unwrap();

        // The archive leg: a client track the chain does carry.
        let theme = read_chain_or_loose(
            &chain,
            Some(&addons),
            "Sound\\Music\\GlueScreenMusic\\wow_main_theme.mp3",
        )
        .expect("the glue theme lives in sound.MPQ");
        assert!(theme.len() > 1_000_000, "the theme is a ~3 MB stored MP3");

        // The loose leg: an addon's own track, named by the virtual path, spelt in its own case.
        assert_eq!(
            read_chain_or_loose(
                &chain,
                Some(&addons),
                "Interface\\AddOns\\Jukebox\\Track.mp3"
            )
            .as_deref(),
            Some(&b"ID3!"[..])
        );
        // In neither store: a plain miss, which every caller renders as silence.
        assert!(read_chain_or_loose(&chain, Some(&addons), "Sound\\Music\\nope.mp3").is_none());
        // No folder configured (a capture run, where `local_state` is hermetic): chain only.
        assert!(
            read_chain_or_loose(&chain, None, "Interface\\AddOns\\Jukebox\\Track.mp3").is_none()
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
