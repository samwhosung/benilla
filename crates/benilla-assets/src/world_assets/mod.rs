//! **The shared asset store** — the one open patch chain, the dedup caches every model/texture load
//! goes through, and the helpers that turn raw BLP pixels into Bevy textures and per-submesh
//! materials.
//!
//! It lives here rather than in the client because it is a layer *under* both sides, and 1164
//! measured how far under: `WorldAssets` alone is named by 63 files, `LockRecover` by 43 and
//! `AssetSet` by 37, and they serve UI sprites, portraits and the minimap mask exactly as much as
//! they serve terrain. Nothing in here reads game state — it knows what pixel shape something
//! wants, not what a unit frame *is*.
//!
//! What stayed up in the client is the 115-line plugin shell that drives it: opening the chain at
//! startup needs the shared light buffer, evicting on a map change needs the world-map message, and
//! the residency sweep needs the art-scope instrument. Those three are the only upward reaches the
//! module ever had — the data core below has none.
//!
//! Deduping by path/material means each unique BLP is decoded + uploaded once, and submeshes sharing
//! a texture+blend share a material handle — which is what lets Bevy batch draws.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_resource::{Buffer, Face};

use crate::materials::{WowModelExt, WowModelMaterial, VANILLA_ALPHA_KEY_REF};
use crate::SpatialCache;
use benilla_formats::{blp_to_rgba, read_texture_native_chain, tga_to_rgba, Chain, ModelBlend};

/// BLP/RGBA -> Bevy `Image` texture helpers (+ solid/cursor/liquid-frame textures). Split out for
/// size; re-exported so callers keep using `benilla_assets::{solid_layer_chain, …}`.
mod images;
pub use images::*;

/// Shared asset store: the **one** open patch chain plus dedup caches, used by every model/texture
/// load (terrain layers, doodads, WMOs, creatures, GameObjects). Replaces the three independent
/// chains we used to open. Deduping by path/material means each unique BLP is decoded + uploaded
/// once, and submeshes sharing a texture+blend share a `StandardMaterial` handle — which is what
/// lets Bevy batch their draw calls.
#[derive(Resource)]
pub struct WorldAssets {
    /// The **one** open patch chain, behind `Arc<Mutex<…>>` because the streaming loaders thread a
    /// `&mut Chain` (the `read_file` shim); the `Mutex` is `Send + Sync` glue, not a read serializer —
    /// `Chain` reads are `&self` and open a fresh OS handle, so they share no seek state.
    /// Main-thread reads lock via [`LockRecover`] (poison-tolerant); contention is ~nil since after
    /// startup the only main-thread reads are the occasional map-change WDL load + lazy clutter M2s.
    pub chain: Arc<Mutex<Chain>>,
    /// Decoded GPU textures by normalized path.
    /// Keyed by `(normalized path, wrap_u, wrap_v)`: the SAME BLP is legitimately sampled both
    /// ways by different models (a sheet that tiles on a wall and clamps on a cutout card), and the
    /// address mode lives on the Bevy `Image`'s sampler — so each mode needs its own upload.
    /// Before that key existed, whichever model loaded the texture first decided
    /// the mode for every later one.
    pub textures: SpatialCache<(String, bool, bool), Handle<Image>>,
    /// Decoded UI sprite textures by resolved path (`None` = a miss, cached so a bad path never
    /// re-walks the chain per frame). Separate from `textures`: sprites are sRGB/clamped, world
    /// art is Unorm/repeat — the same BLP can legitimately live in both.
    sprites: HashMap<String, Option<Handle<Image>>>,
    /// Decoded UI sprite textures sampled with **repeat** addressing on one or both axes (frame
    /// `Backdrop` pieces — tiled edges/bg — and the reference's `SetTexCoord(0, n, 0, 1)` strips).
    /// Keyed by resolved path **and** the per-axis wrap, separate from `sprites`: the *same* BLP
    /// can be wanted clamp-sampled as a plain sprite and repeat-sampled as a backdrop piece, and
    /// each address-mode pair needs its own GPU image (the sampler is baked into the `Image`).
    /// See [`Self::sprite_texture_wrapped`].
    tiled_sprites: HashMap<(String, bool, bool), Option<Handle<Image>>>,
    /// Sprites RESAMPLED to an exact physical size, by `(path, w, h)` — the nameplate border's
    /// 0188 sharpen and nothing else so far. Keyed by size because that is the whole point: the
    /// art is re-rasterised whenever the plate's pixel size moves, and reused on every frame in
    /// between (a resize is rare; a plate is drawn 60 times a second).
    resampled_sprites: HashMap<(String, u32, u32), Option<Handle<Image>>>,
    /// The decoded source pixels behind [`Self::resampled_sprites`], so a resize re-samples
    /// instead of re-decoding the BLP.
    resample_sources: HashMap<String, Option<(u32, u32, Vec<u8>)>>,
    /// Decoded **portrait** sprites — [`Self::sprite_texture`]'s clamp/sRGB sprite with a circular
    /// alpha mask baked in ([`portrait_image`]). Its own cache, like `tiled_sprites`: the mask bakes
    /// into the GPU image, so the same BLP wanted as a plain icon and as a portrait needs two images.
    portraits: HashMap<String, Option<Handle<Image>>>,
    /// Decoded coverage **masks** ([`mask_image`] — linear, not sRGB): the minimap's circular clip.
    /// Its own cache for the same reason as the others: the format is baked into
    /// the GPU image.
    masks: HashMap<String, Option<Handle<Image>>>,
    /// **Generated** UI sprites — pixels this client computes rather than decodes, keyed by a
    /// caller-chosen static name ([`Self::generated_sprite`]). Its own cache for the same reason as
    /// every other one: the sampler bakes into the GPU image. Nothing about the *content* lives
    /// here — the caller supplies the RGBA — so this stays a cache, not a second art pipeline.
    generated: HashMap<&'static str, Handle<Image>>,
    /// The **loose-file root for `Interface\AddOns\` sprite paths** — the AddOns folder, installed
    /// by the app ([`Self::set_loose_addon_root`]), `None` until then and in every capture run
    /// (the app resolves it through `local_state`, which is hermetic under `$WOW_CAPTURE`).
    ///
    /// Addon art never lives in an MPQ: the reference's Storm open reads loose files from the game
    /// directory, which is how `<Texture file="Interface\AddOns\Atlas\Images\…"/>` works at all.
    /// Our equivalent maps that virtual prefix onto the one addon root (decision 1185's single
    /// folder). Only the UI-sprite decoders consult it — world art has no business
    /// under `AddOns\`.
    loose_root: Option<PathBuf>,
    /// Materials deduped by their identity (texture path + blend, or the untextured fallback).
    pub model_materials: SpatialCache<MaterialKey, Handle<WowModelMaterial>>,
    /// The shared global-light storage buffer (`lighting::global_light`). Held here so `model_material`
    /// can clone it into every deduped model material's `light_buf` without threading a `Buffer` through
    /// the many call sites. One buffer for the whole scene, updated in place each frame.
    pub shared_light: Buffer,
}

/// Identity of a deduped model material: a textured material is uniquely determined by its texture
/// path, blend mode, sidedness, alpha-key cutoff, and clutter fade-distance; everything textureless
/// (missing/failed texture) shares one fallback. Cutoff + fade are in the key so ground clutter (lower
/// alpha ref + a distance fade) doesn't collide with a tree sharing the same texture.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum MaterialKey {
    /// `(texture, blend, two_sided, alpha_key_u8, fade_far_yd_u16, is_wmo, is_fade_variant,
    /// wrap_u, wrap_v)` — the address mode is part of the identity because it selects a different
    /// `Image` upload.
    Textured(String, ModelBlend, bool, u8, u16, bool, bool, bool, bool),
    Fallback(bool),
}

/// Normalize an internal asset path so case/slash variants of the same file share a cache entry
/// (MPQ lookup is case-insensitive and accepts either slash).
pub fn normalize_path(path: &str) -> String {
    path.replace('/', "\\").to_ascii_lowercase()
}

/// The **cache key** a UI sprite reference folds to: [`normalize_path`], then an extension of
/// **exactly three characters** stripped — a dot at `len - 4`.
///
/// That is the reference's own rule (`0x449590`, which strips a 3-char extension and returns which
/// of `{".tga", ".blp"}` it was), and its texture cache is keyed on the resulting stem — so
/// `Foo`, `Foo.blp` and `Foo.tga` are **one** entry, not three. Ours keys the same way, which is
/// why this is separate from [`sprite_candidates`]: the key identifies the texture, the candidates
/// are the files that might hold it.
///
/// Note a 3-char extension is stripped whatever it is: `Foo.bmp` keys as `Foo`. A longer or shorter
/// one is not an extension to the client at all — `Foo.jpeg` keys as `Foo.jpeg` and looks for
/// `Foo.jpeg.blp`.
pub fn sprite_key(path: &str) -> String {
    let key = normalize_path(path);
    match key.len().checked_sub(4) {
        Some(dot) if key.as_bytes()[dot] == b'.' => key[..dot].to_string(),
        _ => key,
    }
}

/// The two archive names a UI sprite reference may live under, **in the order the reference tries
/// them** — `TextureCreate`'s fixed two-candidate chain (`0x449d90`, extension table `0x835248 =
/// {".tga", ".blp"}`). The supplied extension's pipeline goes first, the other second; both open
/// through the same Storm primitive, and a miss on the first is silent, so the second is a genuine
/// fallback rather than an error path.
///
/// | reference | first | second |
/// |---|---|---|
/// | `Foo` (no 3-char extension) | `Foo.blp` | `Foo.tga` |
/// | `Foo.blp` | `Foo.blp` | `Foo.tga` |
/// | `Foo.tga` | `Foo.tga` | `Foo.blp` |
/// | `Foo.bmp` (any other 3-char) | `Foo.blp` | `Foo.tga` |
/// | `Foo.jpeg` (not 3 chars) | `Foo.jpeg.blp` | `Foo.jpeg.tga` |
///
/// This chain is what makes `Interface\Icons\Ability_Druid_Mangle.tga` — a real macro-chooser entry,
/// since `Ability_Druid_Mangle.tga.blp` ships and the chooser stores names extension-stripped —
/// resolve to `…Mangle.blp` on the second try instead of drawing a white square (bug B221). The old
/// rule here passed an extensioned path through untouched and had no second try at all.
///
/// One function, used by the decoders below *and* by the sweeps that assert our own paths resolve
/// (`ui_script::shipped_xml_tests`, `ui_macro::tests`): a sweep re-implementing this could agree
/// with itself while disagreeing with what actually draws.
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

/// The **texel dimensions** of a UI sprite reference, or `None` when nothing resolves — the number
/// the client's `CSimpleTexture` size getters read out of `[tex+0x144]`/`[tex+0x148]` when a region
/// authored no size on an axis (`0x770720`/`0x770790`, decision 1349: one texel is one FrameXML
/// unit).
///
/// It goes through [`decode_sprite`] rather than sniffing a header, so the size layout uses is the
/// size the screen shows: the same candidate order, the same BLP-vs-TGA content sniff, and the same
/// verdict on a candidate that reads but will not decode. The caller memoises — this is asked once
/// per distinct path, for the handful of regions whose rect is derived from their art.
pub fn sprite_dimensions(
    chain: &Mutex<Chain>,
    loose_root: Option<&Path>,
    path: &str,
) -> Option<(u32, u32)> {
    decode_sprite(chain, loose_root, path).map(|(w, h, _)| (w, h))
}

/// Decode a UI sprite key to RGBA8, **warning once per key that fails to resolve**.
///
/// Every sprite cache below stores misses as well as hits, and each calls this on the miss path
/// *before* inserting — so the warning is self-limiting by construction (one line per distinct key
/// for the process's life), with no separate "already warned" set to keep.
///
/// It exists because the renderer's fallback for an unresolvable path is **silent**. It used to be
/// silent *and* look like art: a `Texture` region whose file could not be found still pushed a
/// quad, which `ui_pass` drew with the shared 1×1 white image tinted white — an opaque WHITE
/// RECTANGLE at the region's exact rect, which is how bug B221's macro-chooser icons looked. That
/// half is gone (`ui_script::extract` now drops the quad outright when the resolve misses), so the
/// miss draws *nothing* rather than a white square. It is no less silent for that: nothing on
/// screen and nothing in Lua says why, and `shipped_xml_tests` sweeps only static XML `file=`
/// attributes, never a path that arrives at runtime from a DBC or from an addon. So the report
/// still belongs here.
///
/// Walks [`sprite_candidates`] in order and takes the first that both reads and decodes; only when
/// **all** fail is it a miss. A candidate that reads but won't decode falls through exactly like
/// one that isn't there — the reference's two loaders both fail silently, so a bad first candidate
/// can't poison the second.
///
/// Each candidate is asked of **two stores**: the patch chain, then — for `Interface\AddOns\`
/// paths — the loose addon folder ([`loose_addon_file`]). The chain never holds an
/// `AddOns\` path (Blizzard's own `Blizzard_*` stubs aside, addon art only exists on disk), so the
/// order between the stores is unobservable; chain-first keeps every non-addon path on exactly the
/// code it always ran.
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
    // **"Not there" and "there but would not decode" are different faults, and saying only the
    // first sends the reader hunting a path that is sitting on disk.** Three colour-mapped TGAs in
    // the addon corpus read fine and failed to decode for years while this line asserted they did
    // not resolve.
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

/// Decode one candidate's bytes to RGBA8, picking the format by content: a `BLP2` magic is a BLP,
/// anything else is offered to the TGA decoder (TGA has no magic; its header validation is the
/// gate). Content, not extension, because the ecosystem mislabels freely — a BLP named `.tga` and
/// the reverse both ship in real addon folders, and the pixel bytes always know what they are.
fn decode_sprite_bytes(bytes: &[u8]) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    if bytes.starts_with(b"BLP2") {
        blp_to_rgba(bytes)
    } else {
        tga_to_rgba(bytes)
    }
}

/// Map a **normalized** addon-relative path (`interface\addons\<addon>\<…>`, lowercase,
/// backslashed — [`normalize_path`] has run) onto a file under the loose addon root, or `None` if
/// it is not an `Interface\AddOns\` path or nothing is there.
///
/// **Not sprite-specific, and named for that**: 1322 built this for addon-shipped
/// Read one file from the **two stores the client's file layer has**: the patch chain, then — for
/// an `Interface\AddOns\` path — the loose addon folder ([`loose_addon_file`]).
///
/// The rule shared by every by-path asset an addon can ship: art (the sprite decoder), fonts (the
/// face loader) and **audio** (`PlayMusic`/`PlaySoundFile`) all reach a file the same way, and
/// three copies of that would be three things to keep in step — the font leg was already a
/// hand-rolled second copy of the sprite leg's order when audio became the third caller.
///
/// The reference asks the install tree *before* the archive (`0x647e60`'s attempt #4 is the MPQ);
/// the order is flipped here for the same reason [`decode_sprite`] flips it, and it is
/// unobservable: the chain carries no `AddOns\` path, so every non-addon read stays on exactly the
/// code it always ran.
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

/// BLP/TGA art, and an addon's own TTFs reach the client by exactly the same route — a loose file
/// under the one AddOns root, named by a virtual `Interface\AddOns\…` path no MPQ carries. One
/// resolver, one prefix rule, one sandbox.
///
/// The walk matches each component **case-insensitively** (exact join first — free on the
/// case-insensitive filesystems macOS installs default to — then a `read_dir` scan): the reference
/// is a Windows client and addons reference their own art in arbitrary case. The candidate cannot
/// escape the root — `normalize_path` leaves no `/`, and any dot-component is refused before a
/// filesystem call happens (same lexical posture as `ui_script::addons::read_under`).
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

/// Mutate an asset only when `differs` says its current state doesn't already match.
///
/// `Assets::get_mut` alone marks the asset Modified — a uniform re-upload and (on the Metal
/// non-bindless path) a bind-group rebuild that frame — so a per-frame writer re-pushing an
/// unchanged value is pure cost (the teleport leak's CPU engine was exactly this shape; the sky
/// family idled at ~17 such writes per frame). The gate decides on the immutable view; pair it
/// with values quantized to display precision ([`quantize`]/[`quant255`]) or continuous inputs
/// defeat the compare.
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

/// Quantize to `n`-ths — the write-gate's precision floor for continuously-drifting inputs
/// (time-of-day bands, camera-aim envelopes). Choose `n` at or below what the output can show:
/// the reference packs its celestial/sky lanes to bytes (`floor(255·…)`, the 0xFF broadcasts)
/// and re-submits them as per-frame vertex data, so a 1/255 step IS the reference's own
/// precision for color/alpha; geometry scalars gate at a sub-pixel step instead.
pub fn quantize(x: f32, n: f32) -> f32 {
    (x * n).round() / n
}

/// [`quantize`] each channel to the display's 1/255 LSB — the byte precision the reference's own
/// color lanes carry.
pub fn quant255(c: [f32; 3]) -> [f32; 3] {
    [
        quantize(c[0], 255.0),
        quantize(c[1], 255.0),
        quantize(c[2], 255.0),
    ]
}

/// Lock a `Mutex`, recovering the guard even if a previous holder **poisoned** it by panicking.
///
/// The shared [`WorldAssets::chain`] is read from many systems; one panic mid-read would otherwise
/// poison the lock and turn every subsequent `lock().unwrap()` — including the render loop's — into a
/// cascade. The chain read is a best-effort decode (a half-read texture is harmless), so we recover
/// the guard and carry on. (Phase 3 deleted the net subsystem's shared mutexes entirely; this remains
/// only for the asset foundation, which is its one legitimate user.)
pub trait LockRecover<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> LockRecover<T> for Mutex<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl WorldAssets {
    /// Open the store over an already-opened patch chain.
    ///
    /// `shared_light` is the one global-light storage buffer, taken as a **`Buffer`** — a raw wgpu
    /// handle — and not as the client's `SharedLightBuffer`. That is the whole reason this crate can
    /// sit under the renderer: the store's only use of it is `clone()` into every deduped model
    /// material's `light_buf`, so the parameter severs what would otherwise be a dependency on the
    /// lighting layout and, through it, the rig-palette regions.
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

    /// Install (or clear) the loose-file root for `Interface\AddOns\` sprite paths — see the
    /// [`Self::loose_root`] field doc. Called by the app once it knows the AddOns folder; evicts
    /// cached **misses** so a path asked before the root existed gets a second look (a cached hit
    /// can only have come from the chain and stays right).
    /// [`read_chain_or_loose`] over this store's own two halves — the by-path read for an asset
    /// class with no decoder of its own here (audio: `PlayMusic`, `PlaySoundFile`).
    pub fn read_file_or_loose(&self, path: &str) -> Option<Vec<u8>> {
        read_chain_or_loose(&self.chain, self.loose_root.as_deref(), path)
    }

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

    /// Decode + upload a BLP once per path; later requests for the same texture share the handle.
    /// `None` if the texture is missing or fails to decode.
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
        // Passthrough: world art is uploaded in the form the BLP stores it.
        // Nothing reads a world texture main-side — the same reasoning that already makes this
        // lane `RenderAssetUsages::RENDER_WORLD` — so there are no CPU texels to lose.
        let chain = read_texture_native_chain(&mut self.chain.lock_recover(), &key.0).ok()?;
        let handle = images.add(repeat_texture_authored(crate::for_upload(chain), wrap));
        self.textures.insert(key, handle.clone());
        Some(handle)
    }

    /// Decode a BLP into a clamp-sampled **sRGB** sprite image (mip 0 only) — the celestial discs
    /// (sun/moon) and every player-UI quad. Unlike [`Self::texture`] (repeat-sampled, gamma-space
    /// `Unorm`, mipmapped — for tiling world art), this is sRGB + clamp-to-edge with no mip chain.
    /// Extensionless references get `.blp` appended (the FrameXML convention). Cached by resolved
    /// path — hits and misses both — in its own `sprites` map, separate from the `textures` cache
    /// (the same BLP can live in both, with different sampling).
    pub fn sprite_texture(
        &mut self,
        path: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        // FrameXML/DBC texture references are canonically EXTENSIONLESS
        // (`Interface\Buttons\UI-Quickslot2`) — the real client appends `.blp` at resolve. Paths
        // that already carry an extension (`textures\moon.blp`) pass through unchanged.
        let key = sprite_key(path);
        // Cached (hits AND misses): the UI extractor asks per quad per frame — uncached, every
        // frame re-decoded every BLP into a fresh `Image` (unbounded asset growth) and the fresh
        // handles defeated the quad-list dirty check.
        if let Some(cached) = self.sprites.get(&key) {
            return cached.clone();
        }
        let loaded = decode_sprite(&self.chain, self.loose_root.as_deref(), path)
            .map(|(w, h, rgba)| images.add(sprite_image(w, h, rgba)));
        self.sprites.insert(key, loaded.clone());
        loaded
    }

    /// Decode a texture to raw RGBA8 `(w, h, bytes)` **without** uploading a GPU image — for callers
    /// that resample the pixels themselves before upload (the V-plate border sharpen, decision 0188,
    /// which resizes the 128×32 frame to the plate's exact size). Same extensionless→`.blp` resolve
    /// as [`Self::sprite_texture`]; not cached (a one-shot decode at load, not a per-frame ask).
    pub fn decode_rgba(&mut self, path: &str) -> Option<(u32, u32, Vec<u8>)> {
        decode_sprite(&self.chain, self.loose_root.as_deref(), path)
    }

    /// A UI sprite **resampled to an exact pixel size** by the caller's own kernel, cached by that
    /// size — the nameplate border.
    ///
    /// The plate's frame art is a 128 × 32 BLP drawn at whatever size the plate is, which past the
    /// 1024×768 knee (and always on a retina framebuffer) is a magnification: the GPU's bilinear
    /// filter smears the 1 px gold bevel across several soft output pixels, which is the director's
    /// "blurry border". Resampling the SAME pixels to the target size with a sharp kernel keeps the
    /// art and loses the smear. This lives here rather than in the plate driver because since
    /// decision 2148 the plate is a widget like any other and its border is an ordinary texture
    /// region — the substitution has to happen where a texture path becomes an image.
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

    /// A UI sprite decoded with **repeat** (wrap) addressing on both axes — the frame `Backdrop`
    /// tiled pieces (`0x77f0c0`): a border edge strip samples UVs `[0..N]` and a tiled
    /// bg `[0..w/period]`, so the texture must wrap, not clamp. [`Self::sprite_texture_wrapped`]
    /// with both axes on; see there for the cache and the decode.
    pub fn sprite_texture_tiled(
        &mut self,
        path: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        self.sprite_texture_wrapped(path, (true, true), images)
    }

    /// A UI sprite decoded with **repeat** addressing on the axes `wrap` names and clamp on the
    /// rest — the reference's one-axis tiling idiom (`SetTexCoord(0, n, 0, 1)` on the stance
    /// shelf's middle strip) wraps along its length and must clamp across it, or the strip's
    /// bottom row bleeds into its top edge ([`sprite_image_wrapped`]). Same
    /// sRGB/no-mip decode + extensionless→`.blp` resolve as [`Self::sprite_texture`], but its
    /// own cache keyed by the wrap pair (the sampler is baked into the `Image`, so a path wanted
    /// under two address modes needs two GPU images). Cached hits **and** misses.
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

    /// A sprite whose pixels this client **computes**, cached under `key` and built at most once.
    /// `make` returns `(width, height, rgba8)` and runs only on the first ask.
    ///
    /// The customer is the colour picker: the reference's `<ColorWheelTexture>` and
    /// `<ColorValueTexture>` carry no `file=` at all, because the hue disc and the brightness ramp
    /// are generated by the client and no BLP in the chain is either of them. The generation itself
    /// deliberately stays with the caller — the pixel law belongs to the widget, not to the asset
    /// layer, which knows only how to cache and upload.
    ///
    /// Uploaded through [`sprite_image`], so it samples exactly like every other UI sprite:
    /// sRGB, clamp-to-edge, linear, no mips.
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

    /// A **portrait** sprite: [`Self::sprite_texture`]'s clamp/sRGB decode with a circular alpha mask
    /// baked in ([`portrait_image`]) — the unit-frame portrait, round like the client's. Same
    /// extensionless→`.blp` resolve; its own cache (the mask bakes into the GPU image, so a path
    /// wanted both plain and portrait needs two images). Cached hits **and** misses.
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

    /// The tabard designer's emblem cell: the emblem BLP decoded and rewritten to
    /// **white carrying its own alpha** — `(a << 24) | 0x00FFFFFF` per texel, the reference's
    /// `0x503431` loop — as an ordinary clamped sprite the caller tints. Cached per path like a
    /// sprite; a file that fails to open yields `None` (the reference installs its static array's
    /// stale contents instead; nothing here has a previous emblem to show).
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

    /// A coverage **mask** ([`mask_image`] — linear/clamp, the texel bytes handed to the shader
    /// 1:1): the minimap's `MinimapMask.blp` circle ([`crate::ui_pass::UiQuadMask`]).
    /// Same extensionless→`.blp` resolve as [`Self::sprite_texture`]; its own cache, hits AND misses.
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

    /// Decode a cursor BLP (e.g. `Interface\Cursor\Point.blp`) into a single-mip GPU image for
    /// winit's custom OS cursor (`CursorIcon::Custom`) — *not* the mipmapped, repeat-sampled
    /// [`repeat_texture_authored`] used for world art. `None` if the file is missing or fails to decode.
    /// macOS doesn't use this (it drives `NSCursor` from the raw RGBA directly — see `crate::cursor`).
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

    /// A `StandardMaterial` for a model submesh, deduped by (texture, blend) so submeshes/models
    /// sharing a texture share one material handle (enabling draw-call batching). A missing/failed
    /// texture falls back to a single shared untextured material.
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
        // Alpha-test cutoff: callers may override (ground clutter uses the detail-doodad ref ≈ 0.5);
        // everything else uses the general vanilla model alpha key.
        let cutoff = alpha_cutoff.unwrap_or(VANILLA_ALPHA_KEY_REF);
        // Ground-clutter distance fade (None for normal models). Near = 0.75× far (the client's ratio).
        let clutter_fade = match fade_far {
            Some(far) => Vec4::new(far * 0.75, far, 0.0, 1.0),
            None => Vec4::ZERO,
        };
        let resolved = texture.and_then(|t| {
            self.texture(t, wrap, images)
                .map(|h| (normalize_path(t), h))
        });
        // Only AlphaTest consumes the cutoff, so don't fragment opaque/blend materials by it.
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
        // Per-submesh blend (opaque trunk/wall vs alpha-cut leaves/windows). This builder serves
        // ground clutter, which never authors the multiply modes — Mod/Mod2x fall to plain Blend
        // here rather than growing this path the marker bits `model_render::model_material` packs
        // (that builder is the one every Mod-capable consumer uses).
        let alpha_mode = match blend {
            ModelBlend::Opaque => AlphaMode::Opaque,
            ModelBlend::AlphaTest => AlphaMode::Mask(cutoff),
            ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x => AlphaMode::Blend,
        };
        // Single-sided unless the M2's 0x04 flag is set — matches the real client (many canopy planes
        // are visible from one direction only).
        let cull_mode = if two_sided { None } else { Some(Face::Back) };
        // The base StandardMaterial carries texture/alpha/cull; our `wow_model.wgsl` extension does
        // the WoW lighting (PBR fields like reflectance/roughness are ignored by it). Light uniforms
        // are placeholders — `apply_wow_lighting` fills them from Light.dbc + the panel knobs.
        let base = match resolved {
            Some((_, image)) => StandardMaterial {
                base_color_texture: Some(image),
                alpha_mode,
                double_sided: two_sided,
                cull_mode,
                ..default()
            },
            // No resolved texture → untextured WHITE draw (stage disabled in the reference —
            // `0x59d020`; see model_render.rs for the full note).
            None => StandardMaterial {
                base_color: Color::WHITE,
                ..default()
            },
        };
        let handle = materials.add(WowModelMaterial {
            base,
            extension: WowModelExt {
                clutter_fade,
                // x = WMO (FFP N·L × MOCV, not SH); y = distance-fade blend variant (depth-write-on
                // via `specialize` — see WowModelKey; no live caller passes it here).
                model_flags: Vec4::new(
                    if is_wmo { 1.0 } else { 0.0 },
                    if is_fade_variant { 1.0 } else { 0.0 },
                    0.0,
                    0.0,
                ),
                // x = doodad-lobe sun scale; `1.0` (lit) here — this builder serves clutter, which lights on
                // the FFP N·L path with its own baked MCSH tint, so the lobe's `sun_scale` is unused.
                sun_scale: Vec4::new(1.0, 0.0, 0.0, 0.0),
                tint: Vec4::ONE, // clutter never carries an animated M2Color tint (w: not a WMO batch)
                sidn: Vec4::ZERO, // clutter is never SIDN/WINDOW glass (WMO-only)
                anim_slots: Vec4::ZERO,
                // The shared global light (light/fog/SH come from here, updated in place each frame).
                light_buf: self.shared_light.clone(),
            },
        });
        self.model_materials.insert(key, handle.clone());
        handle
    }
}

/// World/render configuration read once at startup from the environment, shared by the subsystems
/// that need it: terrain reads `unload_budget` (the release lane).
/// Inserted by [`AssetPlugin`] alongside [`WorldAssets`]; its *presence* is also the "there is a
/// client install" gate the world-side setups key on.
///
/// How far terrain streams is deliberately **not** here: the residency window derives from the
/// live `farclip` view distance (`benilla_world::view::ViewDistance` — the player's Terrain
/// Distance setting), as the reference derives it.
#[derive(Resource, Clone, Copy)]
pub struct RenderConfig {
    /// Stale tiles released per frame on a within-map window shift (`$WOW_TILE_UNLOAD`,
    /// default 1; `0` = unbudgeted — the whole trailing row in one frame, the pre-B181
    /// behaviour, kept as the controlled A/B leg on one build).
    pub unload_budget: usize,
}

/// Startup ordering seam: [`AssetPlugin`] opens the patch chain in this set so every other
/// subsystem's startup (catalog loads, terrain/lighting setup) can run `.after(AssetSet::Open)` and
/// borrow the shared [`WorldAssets`].
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetSet {
    Open,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The disk tree spells its names in mixed case, the candidate arrives normalized-lowercase
    /// (as [`sprite_candidates`] always hands it over) — the walk must land on the file anyway,
    /// because the reference is a Windows client and addon authors never matched their own case.
    #[test]
    fn loose_addon_file_maps_addon_paths_case_insensitively() {
        let root =
            std::env::temp_dir().join(format!("benilla-loose-sprite-test-{}", std::process::id()));
        let dir = root.join("Atlas").join("Images").join("Maps");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("BlackrockDepths.blp"), b"x").unwrap();

        // The returned SPELLING is filesystem-dependent (a case-insensitive filesystem answers
        // the exact-join fast path with the candidate's own casing), so the claim is that the
        // path opens the right file, not how it is spelt.
        let hit = loose_addon_file(
            &root,
            "interface\\addons\\atlas\\images\\maps\\blackrockdepths.blp",
        )
        .expect("mixed-case tree resolves a lowercase candidate");
        assert_eq!(std::fs::read(&hit).unwrap(), b"x");

        // Not an AddOns path → not this store's question.
        assert_eq!(loose_addon_file(&root, "interface\\icons\\foo.blp"), None);
        // A directory is not a file, and a missing file is a miss, not an error.
        assert_eq!(loose_addon_file(&root, "interface\\addons\\atlas"), None);
        assert_eq!(
            loose_addon_file(&root, "interface\\addons\\atlas\\images\\maps\\nope.blp"),
            None
        );
        // Dot-components never reach the filesystem (the read_under posture; `normalize_path`
        // already forbids `/`, so this is the one lexical escape left to refuse).
        assert_eq!(
            loose_addon_file(&root, "interface\\addons\\..\\..\\etc\\passwd"),
            None
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// **The by-path read every addon-shipped asset resolves by** — chain first, then the one
    /// loose folder ([`read_chain_or_loose`]). Audio is that rule's third caller, after art and
    /// fonts, and an addon's own track is exactly the file no MPQ can hold: this leg is the whole
    /// reason `PlayMusic("Interface\\AddOns\\…")` can make a sound at all.
    ///
    /// Against the **real** chain, because both legs matter — a client file must still come from
    /// the archive, and only a real chain can show the loose folder is a *fallback* rather than a
    /// shadow over it.
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
        // No folder configured (a capture run — `local_state` is hermetic there): chain only.
        assert!(
            read_chain_or_loose(&chain, None, "Interface\\AddOns\\Jukebox\\Track.mp3").is_none()
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
