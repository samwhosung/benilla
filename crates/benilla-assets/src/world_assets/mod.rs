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
use benilla_formats::{
    blp_to_rgba, read_texture_native_chain, tga_texture_size, tga_to_rgba, Chain, ModelBlend,
    TGA_GATE_PREFIX,
};
pub use benilla_formats::{LooseFile, LooseIndex, LooseIndexCell, LooseOpen};

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
    /// The UI sprite caches.
    cache: SpriteCaches,
    /// UI sprites whose pixels the client computes ([`Self::generated_sprite`]).
    generated: HashMap<&'static str, Handle<Image>>,
    /// The loose files a flagged open reads before the chain: the install's index, and the AddOns
    /// folder once the app installs it (never in capture runs).
    loose: LooseFiles,
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
/// FrameXML unit. Decoded, so layout gets the size the screen shows, and read by an
/// interface texture's open ([`LooseOpen::Index`]).
pub fn sprite_dimensions(
    chain: &Mutex<Chain>,
    loose: &LooseFiles,
    path: &str,
) -> Option<(u32, u32)> {
    decode_texture(chain, loose, path, LooseOpen::Index).map(|(w, h, _)| (w, h))
}

/// Decode the first of [`sprite_candidates`] that reads and decodes, each read by
/// [`read_loose_or_chain`]'s rule for `open`, as the reference's two loaders fail silently. A miss
/// warns, once per key since every cache stores its misses: a missing sprite draws nothing, and
/// nothing else says why.
fn decode_sprite(
    chain: &Mutex<Chain>,
    loose: &LooseFiles,
    path: &str,
    open: LooseOpen,
) -> Option<(u32, u32, Vec<u8>)> {
    decode_sprite_as(chain, loose, path, open, OnRefusal::Miss)
}

/// [`decode_sprite`] for a load that `TextureCreate` (`0x449d90`) makes: a `.tga` whose header
/// opens is the texture, and a decode that refuses it leaves the opaque white default
/// ([`OnRefusal::WhiteDefault`]).
fn decode_texture(
    chain: &Mutex<Chain>,
    loose: &LooseFiles,
    path: &str,
    open: LooseOpen,
) -> Option<(u32, u32, Vec<u8>)> {
    decode_sprite_as(chain, loose, path, open, OnRefusal::WhiteDefault)
}

/// What a candidate file that opens but will not decode becomes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OnRefusal {
    /// Nothing: the next candidate is tried, then the sprite is a miss.
    Miss,
    /// `TextureCreate`'s TGA loader `0x44a140` returns the texture once the header opens
    /// (`0x44a169`) and decodes later, in the callback `0x44a260`; a refused decode fills opaque
    /// white at the header's size (`0x44a2eb` into `0x448920`) and logs the reference's line
    /// (`0x835538`), with no next candidate. A header that makes no texture (`0x448450`, see
    /// [`tga_texture_size`]) fails the candidate as a missing file does, whether the pixels would
    /// decode or not. A BLP candidate takes another callback (`0x44a500`), whose failure is a
    /// solid green 8 by 8 texture instead.
    WhiteDefault,
}

fn decode_sprite_as(
    chain: &Mutex<Chain>,
    loose: &LooseFiles,
    path: &str,
    open: LooseOpen,
    on_refusal: OnRefusal,
) -> Option<(u32, u32, Vec<u8>)> {
    let open = loose_open_for(path, open);
    let candidates = sprite_candidates(path);
    // Asked before the chain's lock, so an index build never stalls an archive reader.
    let hits: Vec<Option<LooseFile>> = candidates.iter().map(|c| loose.find(c, open)).collect();
    let mut chain = chain.lock_recover();
    for (candidate, hit) in candidates.iter().zip(&hits) {
        // A loose file answers the candidate whether or not it decodes ([`read_loose_or_chain`]).
        let bytes = match hit {
            Some(file) => file.read(),
            None => chain.read_file(candidate).ok(),
        };
        match bytes.map(|b| decode_candidate(candidate, &b, on_refusal)) {
            Some(Candidate::Pixels(width, height, rgba)) => return Some((width, height, rgba)),
            Some(Candidate::Nothing) => return None,
            Some(Candidate::Next) | None => {}
        }
    }
    // "Not there" and "there but would not decode" are different faults; say which.
    let found_but_undecodable: Vec<&String> = candidates
        .iter()
        .zip(&hits)
        .filter(|(c, hit)| hit.is_some() || chain.read_file(c).is_ok())
        .map(|(c, _)| c)
        .collect();
    if found_but_undecodable.is_empty() {
        warn!(
            "texture miss: '{path}' does not resolve in the patch chain{} (tried {})",
            if open == LooseOpen::Archives {
                ""
            } else {
                ", the install's loose files or the AddOns folder"
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

/// The open a name makes: `open`, unless the name is written with `/`, which no loose file
/// answers ([`LooseFiles::find`]), so it reads as an open with flag 0 would. The candidates the
/// sprite loaders try are normalized, so the rule is taken from the name as the caller wrote it.
fn loose_open_for(path: &str, open: LooseOpen) -> LooseOpen {
    if path.contains('/') {
        LooseOpen::Archives
    } else {
        open
    }
}

/// Whether an interface texture's open of `path` makes a texture, in the loose files or the chain,
/// for the probes that answer `SetTexture`'s 1|nil before anything decodes: a candidate that opens
/// and, if it is a TGA, whose header passes the size gate ([`makes_texture`]). A fresh
/// [`SpriteProbe`] answers it, reading a TGA each time; a host asked per frame keeps one.
pub fn sprite_found(chain: &Mutex<Chain>, loose: &LooseFiles, path: &str) -> bool {
    SpriteProbe::default().found(chain, loose, path)
}

/// `SetTexture`'s 1|nil probe ([`sprite_found`]), which keeps the size gate's answer for each
/// resolved file it has read: an addon sets an icon from its `OnUpdate`, every frame. The host
/// makes one probe per UI load, so an answer lasts that load and a `/reload` asks again; a file
/// edited meanwhile is not read again. A file that will not read is asked again, and whether a
/// name opens at all is asked each time, since the loose index can change.
#[derive(Default)]
pub struct SpriteProbe {
    /// The gate's answer by resolved file ([`source_key`]).
    gate: std::cell::RefCell<HashMap<String, bool>>,
    /// How many TGA headers [`Self::found`] has read.
    header_reads: std::cell::Cell<usize>,
}

impl SpriteProbe {
    /// See [`sprite_found`].
    pub fn found(&self, chain: &Mutex<Chain>, loose: &LooseFiles, path: &str) -> bool {
        let open = loose_open_for(path, LooseOpen::Index);
        let candidates = sprite_candidates(path);
        // The loose files first, outside the chain's lock.
        let hits: Vec<Option<LooseFile>> = candidates.iter().map(|c| loose.find(c, open)).collect();
        candidates.iter().zip(hits).any(|(candidate, hit)| {
            let opens = hit.is_some() || chain.lock_recover().contains(candidate);
            if !opens || !candidate.ends_with(".tga") {
                return opens;
            }
            let key = match &hit {
                Some(file) => source_key(file),
                None => format!("chain:{candidate}"),
            };
            self.gate(&key, candidate, || match hit {
                Some(file) => file.read_prefix(TGA_GATE_PREFIX),
                None => chain.lock_recover().read_file(candidate).ok(),
            })
        })
    }

    /// Whether a `.tga` candidate makes a texture, from the bytes `read` gives, for the resolved
    /// file `key` names.
    fn gate(&self, key: &str, candidate: &str, read: impl FnOnce() -> Option<Vec<u8>>) -> bool {
        if let Some(&answer) = self.gate.borrow().get(key) {
            return answer;
        }
        self.header_reads.set(self.header_reads.get() + 1);
        let Some(bytes) = read() else {
            return false;
        };
        let answer = makes_texture(candidate, &bytes);
        self.gate.borrow_mut().insert(key.to_string(), answer);
        answer
    }
}

/// The file an entry opens, as a key: the path on disk, and an archive's member.
fn source_key(file: &LooseFile) -> String {
    match file {
        LooseFile::Plain(path) => path.display().to_string(),
        LooseFile::Archive { archive, member } => format!("{}!{member}", archive.display()),
    }
}

/// Whether `TextureCreate` (`0x449d90`) makes a texture of a candidate's bytes: a TGA the loader
/// `0x44a140` reads needs a header that `0x448450` makes a device texture at ([`tga_texture_size`]),
/// decoded or not. A `.blp` candidate, or bytes that are a BLP under a `.tga` name, are not gated
/// here.
fn makes_texture(candidate: &str, bytes: &[u8]) -> bool {
    !is_tga_load(candidate, bytes) || tga_texture_size(bytes).is_some()
}

/// Whether `candidate` is read by the TGA loader: its name is `.tga` and its bytes are no BLP, which
/// the content sniff ([`decode_sprite_bytes`]) sends to the BLP decoder.
fn is_tga_load(candidate: &str, bytes: &[u8]) -> bool {
    candidate.ends_with(".tga") && !bytes.starts_with(b"BLP2")
}

/// What one candidate file yields for a load.
#[derive(Debug, PartialEq)]
enum Candidate {
    /// Pixels to draw.
    Pixels(u32, u32, Vec<u8>),
    /// A texture of no size, which draws nothing and ends the search: the size gate lets a zero
    /// through (`0x44849d`, `test ebx, ebx-1`).
    Nothing,
    /// No texture: the next candidate is tried.
    Next,
}

/// One candidate's bytes through [`decode_sprite_bytes`]. By `on_refusal`, a TGA load gates its
/// header's size first ([`makes_texture`]), decoded or not, and a refused decode of one that
/// passes leaves that size of opaque white.
fn decode_candidate(candidate: &str, bytes: &[u8], on_refusal: OnRefusal) -> Candidate {
    let size = if on_refusal == OnRefusal::WhiteDefault && is_tga_load(candidate, bytes) {
        match tga_texture_size(bytes) {
            Some(size) => Some(size),
            None => return Candidate::Next,
        }
    } else {
        None
    };
    if let Ok((width, height, rgba)) = decode_sprite_bytes(bytes) {
        return Candidate::Pixels(width, height, rgba);
    }
    match size {
        None => Candidate::Next,
        Some((0, _) | (_, 0)) => Candidate::Nothing,
        Some((width, height)) => {
            warn!("Texture {candidate} not loaded -- replaced with default.");
            Candidate::Pixels(
                width,
                height,
                vec![0xff; width as usize * height as usize * 4],
            )
        }
    }
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

/// The loose files an open that sets the loose flag reads before the archive chain, as the
/// reference's resolver (`0x647e60`) answers from its index of the install (`0x647f89`) before the
/// chain (`0x648116`): the install's index for any path, and benilla's AddOns folder for an
/// `Interface\AddOns\` path ([`loose_addon_file`]), which stands in for the install's by a
/// written deviation (`benilla-app`'s `ui_script::addons::root`).
#[derive(Clone, Default)]
pub struct LooseFiles {
    /// benilla's AddOns folder, or `None` (always under `$WOW_CAPTURE`).
    pub addons: Option<PathBuf>,
    /// The install's index ([`benilla_formats::install_index`]), or `None` for a test's chain.
    pub install: Option<Arc<LooseIndexCell>>,
}

impl LooseFiles {
    /// The player's install, with no AddOns folder until the app installs one.
    pub fn install() -> Self {
        LooseFiles {
            addons: None,
            install: Some(benilla_formats::install_index()),
        }
    }

    /// The loose file that answers an open of `path` with `open`, or `None` for the archives:
    /// the font open's bare name first ([`LooseOpen::BareNameFirst`]), then the whole name, an
    /// `Interface\AddOns\` one in the AddOns folder, which the install's index leaves out. A name
    /// written with `/` finds nothing, as the index's compare does not read it as `\` (`0x647f37`);
    /// a caller whose path benilla joined with `/` turns it back first.
    pub fn find(&self, path: &str, open: LooseOpen) -> Option<LooseFile> {
        if open == LooseOpen::Archives || path.contains('/') {
            return None;
        }
        let install = self.install.as_ref().map(|cell| cell.get());
        if open == LooseOpen::BareNameFirst {
            let bare = path.rsplit('\\').next().unwrap_or(path);
            if let Some(file) = install.as_ref().and_then(|ix| ix.get(bare)) {
                return Some(file);
            }
        }
        let norm = normalize_path(path);
        if norm.starts_with(ADDONS_PREFIX) {
            return self
                .addons
                .as_deref()
                .and_then(|root| loose_addon_file(root, &norm))
                .map(LooseFile::Plain);
        }
        install.as_ref().and_then(|ix| ix.get(&norm))
    }
}

/// Read a file by path for an open of kind `open`: the loose file that answers it
/// ([`LooseFiles::find`]), else the patch chain. A file the loose files answer and the read then
/// fails is a miss, with no turn to the chain (`0x647865`).
pub fn read_loose_or_chain(
    chain: &Mutex<Chain>,
    loose: &LooseFiles,
    path: &str,
    open: LooseOpen,
) -> Option<Vec<u8>> {
    match loose.find(path, open) {
        Some(file) => file.read(),
        None => chain.lock_recover().read(path).ok(),
    }
}

/// The normalized prefix of an addon path.
const ADDONS_PREFIX: &str = "interface\\addons\\";

/// Map a normalized `interface\addons\…` path onto a file under the addon root, matching each
/// component case-insensitively as the Windows reference does; a dot-component, or one the host
/// reads as more than a plain name (a Windows `C:`), is refused, so a path cannot escape the root.
pub fn loose_addon_file(root: &Path, candidate: &str) -> Option<PathBuf> {
    let rel = candidate.strip_prefix(ADDONS_PREFIX)?;
    let mut at = root.to_path_buf();
    for (depth, comp) in rel.split('\\').enumerate() {
        let plain = matches!(
            Path::new(comp).components().collect::<Vec<_>>()[..],
            [std::path::Component::Normal(_)]
        );
        if !plain || comp.starts_with('.') {
            return None;
        }
        at = child_ignoring_case(&at, comp)?;
        if depth == 0 && moved_aside(&at, comp) {
            return None;
        }
    }
    at.is_file().then_some(at)
}

/// `dir`'s entry named `name` without regard to case, as on the reference's Windows filesystem.
fn child_ignoring_case(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.exists() {
        return Some(direct);
    }
    let found = std::fs::read_dir(dir).ok()?.flatten().find(|e| {
        e.file_name()
            .to_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
    })?;
    Some(found.path())
}

/// Whether the AddOns folder `dir`, named `name`, is one the glue build moves aside before the
/// add-on scan: a `Blizzard*` folder holding `<name>\<name>.toc` (`0x51fbf0`, walked by `0x51fb50`
/// at `0x46a8eb`), renamed `%s.old` so the stock add-on loads from the archive. benilla moves
/// nothing a player put there; it registers no add-on from such a folder and reads no file in it.
pub fn moved_aside(dir: &Path, name: &str) -> bool {
    name.get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("blizzard"))
        && child_ignoring_case(dir, &format!("{name}.toc")).is_some_and(|toc| toc.is_file())
}

/// Sprite handles by key, a miss as `None`, each with the open that made it. The key holds no
/// open: the reference's texture cache keys on the stripped name and the flags word `0x449ae0`
/// builds from the filter and force arguments (`0x449dd2` to `0x449dec`, compared at `0x449f39`
/// and `0x449f47`), and the file flag is read only by the loaders (`0x449e20`, `0x449e32`), so a
/// name's first open wins while it is cached (`0x449df3`).
struct Opened<K>(HashMap<K, (LooseOpen, Option<Handle<Image>>)>);

impl<K> Default for Opened<K> {
    fn default() -> Self {
        Opened(HashMap::new())
    }
}

impl<K: Eq + std::hash::Hash> Opened<K> {
    /// The cached answer, `Some(None)` for a cached miss.
    fn get(&self, key: &K) -> Option<Option<Handle<Image>>> {
        self.0.get(key).map(|(_, handle)| handle.clone())
    }

    fn insert(&mut self, key: K, open: LooseOpen, handle: Option<Handle<Image>>) {
        self.0.insert(key, (open, handle));
    }

    /// Keep the entries `keep(key, open, hit)` says to.
    fn retain(&mut self, mut keep: impl FnMut(&K, LooseOpen, bool) -> bool) {
        self.0
            .retain(|key, (open, handle)| keep(key, *open, handle.is_some()));
    }
}

/// The UI sprite caches, and the install index's generation they were filled under.
#[derive(Default)]
struct SpriteCaches {
    /// UI sprites by [`sprite_key`].
    sprites: Opened<String>,
    /// UI sprites loaded with the wrap flag, by key: the sampler bakes into the `Image`.
    tiled: Opened<String>,
    /// Sprites resampled to an exact physical size, by `(path, w, h)`, always opened loose.
    resampled: HashMap<(String, u32, u32), Option<Handle<Image>>>,
    /// The decoded pixels behind [`Self::resampled`], so a resize does not re-decode.
    resample_sources: HashMap<String, Option<(u32, u32, Vec<u8>)>>,
    /// Portrait sprites, the circular mask baked in ([`portrait_image`]).
    portraits: Opened<String>,
    /// Coverage masks ([`mask_image`]): the minimap's circular clip.
    masks: Opened<String>,
    /// The [`LooseIndexCell::generation`] the entries opened loose were filled under.
    loose_generation: u64,
}

impl SpriteCaches {
    /// Drop every entry opened with the loose flag when the install's index was emptied since
    /// they filled, so the next request opens again, as the reference opens a texture again once
    /// it was released (every UI texture at a `ReloadUI` or a logout, the moments that empty the
    /// index). Entries opened with flag 0 stay: nothing loose answered them.
    fn sync(&mut self, generation: Option<u64>) {
        let Some(generation) = generation.filter(|g| *g != self.loose_generation) else {
            return;
        };
        self.loose_generation = generation;
        let archives = |_: &_, open: LooseOpen, _| open == LooseOpen::Archives;
        self.sprites.retain(archives);
        self.tiled.retain(|_, open, _| open == LooseOpen::Archives);
        self.portraits.retain(archives);
        self.masks.retain(archives);
        self.resampled.clear();
        self.resample_sources.clear();
    }

    /// Drop every miss and every `Interface\AddOns\` entry, when the AddOns folder changes: a path
    /// asked before it existed gets a second look, and a loose file shadows the chain's
    /// ([`read_loose_or_chain`]); any other hit came from the chain and stays.
    fn drop_addons_and_misses(&mut self) {
        // `emblem_mask_texture` prefixes its `sprites` key, so the test looks past a tag.
        let stays =
            |key: &str, hit: bool| hit && !normalize_path(key).contains("interface\\addons\\");
        self.sprites.retain(|k, _, hit| stays(k, hit));
        self.tiled.retain(|k, _, hit| stays(k, hit));
        self.resampled.retain(|k, v| stays(&k.0, v.is_some()));
        self.resample_sources.retain(|k, v| stays(k, v.is_some()));
        self.portraits.retain(|k, _, hit| stays(k, hit));
        self.masks.retain(|k, _, hit| stays(k, hit));
    }
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
            cache: SpriteCaches::default(),
            generated: HashMap::new(),
            loose: LooseFiles::install(),
            model_materials: SpatialCache::default(),
            shared_light,
        }
    }

    /// [`read_loose_or_chain`] over this store with the loose flag set ([`LooseOpen::Index`]), for
    /// an open with no decoder here: every sound file (`0x7a8860`), as FMOD's file callbacks open
    /// with flag 1, and the glue's own interface files.
    pub fn read_file_or_loose(&self, path: &str) -> Option<Vec<u8>> {
        read_loose_or_chain(&self.chain, &self.loose, path, LooseOpen::Index)
    }

    /// The loose files this store reads, for a probe that must answer what draws.
    pub fn loose(&self) -> &LooseFiles {
        &self.loose
    }

    /// Install or clear the AddOns folder. Cached misses are evicted so a path asked before it
    /// existed gets a second look, and so is every `Interface\AddOns\` entry, since a loose file
    /// shadows the chain's ([`read_loose_or_chain`]); any other hit came from the chain and stays.
    pub fn set_loose_addon_root(&mut self, root: Option<PathBuf>) {
        if self.loose.addons == root {
            return;
        }
        self.loose.addons = root;
        self.cache.drop_addons_and_misses();
    }

    /// [`SpriteCaches::sync`] to the install index's generation.
    fn sync_loose(&mut self) {
        let generation = self.loose.install.as_ref().map(|c| c.generation());
        self.cache.sync(generation);
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

    /// A clamp-sampled sRGB sprite, mip 0 only, opened as an interface texture
    /// ([`LooseOpen::Index`], as `CSimpleTexture::SetTexture` loads at `0x770281`): every UI quad.
    pub fn sprite_texture(
        &mut self,
        path: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        self.sprite_texture_opened(path, LooseOpen::Index, images)
    }

    /// [`Self::sprite_texture`] with the open the caller's reference load makes: the celestial
    /// discs, the loading screen and the minimap's default art open with flag 0
    /// ([`LooseOpen::Archives`]).
    pub fn sprite_texture_opened(
        &mut self,
        path: &str,
        open: LooseOpen,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        self.sync_loose();
        let key = sprite_key(path);
        // Hits and misses are cached: the UI extractor asks per quad per frame, and a fresh handle
        // would defeat its dirty check.
        if let Some(cached) = self.cache.sprites.get(&key) {
            return cached;
        }
        let loaded = decode_texture(&self.chain, &self.loose, path, open)
            .map(|(w, h, rgba)| images.add(sprite_image(w, h, rgba)));
        let open = loose_open_for(path, open);
        self.cache.sprites.insert(key, open, loaded.clone());
        loaded
    }

    /// Decode a sprite to raw RGBA8 without uploading or caching, for a caller that resamples,
    /// read by `open`.
    pub fn decode_rgba(&mut self, path: &str, open: LooseOpen) -> Option<(u32, u32, Vec<u8>)> {
        decode_sprite(&self.chain, &self.loose, path, open)
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
        self.sync_loose();
        let key = (path.to_string(), w, h);
        if let Some(cached) = self.cache.resampled.get(&key) {
            return cached.clone();
        }
        if !self.cache.resample_sources.contains_key(path) {
            let decoded = decode_texture(&self.chain, &self.loose, path, LooseOpen::Index);
            self.cache
                .resample_sources
                .insert(path.to_string(), decoded);
        }
        let made = self.cache.resample_sources[path]
            .as_ref()
            .map(|(sw, sh, src)| images.add(sprite_image(w, h, resample(src, *sw, *sh, w, h))));
        self.cache.resampled.insert(key, made.clone());
        made
    }

    /// A UI sprite loaded with the wrap flag, repeating on both axes ([`sprite_image_tiled`]).
    pub fn sprite_texture_wrapped(
        &mut self,
        path: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        self.sync_loose();
        let key = sprite_key(path);
        if let Some(cached) = self.cache.tiled.get(&key) {
            return cached;
        }
        let loaded = decode_texture(&self.chain, &self.loose, path, LooseOpen::Index)
            .map(|(w, h, rgba)| images.add(sprite_image_tiled(w, h, rgba)));
        let open = loose_open_for(path, LooseOpen::Index);
        self.cache.tiled.insert(key, open, loaded.clone());
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

    /// A unit-frame portrait, the circular mask baked in ([`portrait_image`]), opened with the loose
    /// flag as `SetPortraitTexture` opens it (through `0x5a7b40`).
    pub fn portrait_texture(
        &mut self,
        path: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        self.sync_loose();
        let key = sprite_key(path);
        if let Some(cached) = self.cache.portraits.get(&key) {
            return cached;
        }
        let loaded = decode_sprite(&self.chain, &self.loose, path, LooseOpen::Index)
            .map(|(w, h, rgba)| images.add(portrait_image(w, h, rgba)));
        let open = loose_open_for(path, LooseOpen::Index);
        self.cache.portraits.insert(key, open, loaded.clone());
        loaded
    }

    /// The tabard designer's emblem cell for the caller to tint: white carrying its own alpha,
    /// `(a << 24) | 0x00FFFFFF` per texel (the reference's `0x503431` loop). A file that fails to
    /// open gives `None`, where the reference keeps its static array's stale contents. Read from
    /// the archives alone, as the emblems' `0x5a7b40` opens with flag 0.
    pub fn emblem_mask_texture(
        &mut self,
        path: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let key = format!("emblem-mask:{}", sprite_key(path));
        if let Some(cached) = self.cache.sprites.get(&key) {
            return cached;
        }
        let open = LooseOpen::Archives;
        let loaded = decode_sprite(&self.chain, &self.loose, path, open).map(|(w, h, mut rgba)| {
            for px in rgba.as_chunks_mut::<4>().0 {
                px[0] = 0xFF;
                px[1] = 0xFF;
                px[2] = 0xFF;
            }
            images.add(sprite_image(w, h, rgba))
        });
        self.cache.sprites.insert(key, open, loaded.clone());
        loaded
    }

    /// A coverage mask, its bytes handed to the shader 1:1 ([`mask_image`]), read by `open`: the
    /// minimap's circle.
    pub fn mask_texture(
        &mut self,
        path: &str,
        open: LooseOpen,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        self.sync_loose();
        let key = sprite_key(path);
        if let Some(cached) = self.cache.masks.get(&key) {
            return cached;
        }
        let loaded = decode_sprite(&self.chain, &self.loose, path, open)
            .map(|(w, h, rgba)| images.add(mask_image(w, h, rgba)));
        let open = loose_open_for(path, open);
        self.cache.masks.insert(key, open, loaded.clone());
        loaded
    }

    /// A cursor BLP's RGBA (`Interface\Cursor\Point.blp`), opened with the loose flag as the
    /// cursor code's `0x449610` opens it.
    pub fn cursor_rgba(&self, path: &str) -> Option<(u32, u32, Vec<u8>)> {
        let bytes = read_loose_or_chain(
            &self.chain,
            &self.loose,
            &normalize_path(path),
            LooseOpen::Index,
        )?;
        blp_to_rgba(&bytes).ok()
    }

    /// Decode a cursor BLP ([`Self::cursor_rgba`]) for winit's custom OS cursor; macOS drives
    /// `NSCursor` from the raw RGBA instead.
    #[cfg(not(target_os = "macos"))]
    pub fn decode_cursor(
        &mut self,
        path: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let (w, h, rgba) = self.cursor_rgba(path)?;
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
                stage1: Vec4::ZERO, // a tuft is one texture
                stage1_texture: None,
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

    /// Against the real chain: a client file comes from the archive, an addon's own file from
    /// disk, and a loose copy of a path the archive also holds shadows it, as `0x647e60` asks the
    /// install tree (`0x647f89`) before the chain (`0x648116`), even when it will not read.
    #[test]
    fn read_loose_or_chain_asks_an_addons_own_file_on_disk_then_the_chain() {
        let data = benilla_formats::wow_data_or_skip!();
        let chain = Mutex::new(Chain::open(&data).expect("open the chain"));
        let root =
            std::env::temp_dir().join(format!("benilla-loose-audio-test-{}", std::process::id()));
        let addons = root.join("AddOns");
        std::fs::create_dir_all(addons.join("Jukebox")).unwrap();
        std::fs::write(addons.join("Jukebox").join("Track.mp3"), b"ID3!").unwrap();
        let folder = LooseFiles {
            addons: Some(addons.clone()),
            install: None,
        };
        let none = LooseFiles::default();
        let open = LooseOpen::Index;

        // The archive leg: a client track the chain does carry.
        let theme = read_loose_or_chain(
            &chain,
            &folder,
            "Sound\\Music\\GlueScreenMusic\\wow_main_theme.mp3",
            open,
        )
        .expect("the glue theme lives in sound.MPQ");
        assert!(theme.len() > 1_000_000, "the theme is a ~3 MB stored MP3");

        // The loose leg: an addon's own track, named by the virtual path, spelt in its own case.
        assert_eq!(
            read_loose_or_chain(
                &chain,
                &folder,
                "Interface\\AddOns\\Jukebox\\Track.mp3",
                open
            )
            .as_deref(),
            Some(&b"ID3!"[..])
        );

        // The shadow: a stock addon's file the archive carries, with a loose copy on disk.
        let stock = "Interface\\AddOns\\Blizzard_AuctionUI\\Blizzard_AuctionUI.lua";
        let archived = chain
            .lock_recover()
            .read(stock)
            .expect("interface.MPQ carries it");
        assert_eq!(
            read_loose_or_chain(&chain, &none, stock, open).as_deref(),
            Some(&archived[..]),
            "with no folder the archive answers"
        );
        std::fs::create_dir_all(addons.join("Blizzard_AuctionUI")).unwrap();
        std::fs::write(
            addons
                .join("Blizzard_AuctionUI")
                .join("Blizzard_AuctionUI.lua"),
            b"-- loose",
        )
        .unwrap();
        assert_eq!(
            read_loose_or_chain(&chain, &folder, stock, open).as_deref(),
            Some(&b"-- loose"[..]),
            "the loose copy shadows the archive's"
        );
        // An open with flag 0 never sees it.
        assert_eq!(
            read_loose_or_chain(&chain, &folder, stock, LooseOpen::Archives).as_deref(),
            Some(&archived[..]),
            "a flag-0 open reads the archive"
        );
        // A loose file that will not read is a miss: the index answered, so the archive is not
        // asked (`0x647865`).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let loose = addons
                .join("Blizzard_AuctionUI")
                .join("Blizzard_AuctionUI.lua");
            std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o000)).unwrap();
            let unreadable = std::fs::read(&loose).is_err();
            let got = read_loose_or_chain(&chain, &folder, stock, open);
            std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).unwrap();
            if unreadable {
                assert_eq!(got, None, "no fallback to the archive copy");
            }
        }

        // The folder holding its own `.toc` is moved aside: the archive's copy answers again.
        std::fs::write(
            addons
                .join("Blizzard_AuctionUI")
                .join("Blizzard_AuctionUI.toc"),
            b"## Interface: 11200",
        )
        .unwrap();
        assert_eq!(
            read_loose_or_chain(&chain, &folder, stock, open).as_deref(),
            Some(&archived[..]),
            "a moved-aside folder answers nothing"
        );

        // In neither store: a plain miss, which every caller renders as silence.
        assert!(read_loose_or_chain(&chain, &folder, "Sound\\Music\\nope.mp3", open).is_none());
        // No folder configured (a capture run, where `local_state` is hermetic): chain only.
        assert!(
            read_loose_or_chain(&chain, &none, "Interface\\AddOns\\Jukebox\\Track.mp3", open)
                .is_none()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A fresh folder under the temp dir, removed when dropped.
    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> Tree {
            let dir = std::env::temp_dir().join(format!(
                "benilla-loose-files-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Tree(dir)
        }

        fn file(&self, rel: &str, body: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn body(file: Option<LooseFile>) -> Option<String> {
        file.map(|f| String::from_utf8(f.read().unwrap()).unwrap())
    }

    /// A flagged open finds an install's loose file by its path below the install or `Data\`, a
    /// font's by its bare name first; an `Interface\AddOns\` path reads benilla's AddOns folder
    /// alone; an open with flag 0, or a name written with `/`, finds nothing loose.
    #[test]
    fn loose_files_answer_the_flagged_opens_from_the_install_and_the_addons_folder() {
        let install = Tree::new("install");
        install.file("Fonts/FRIZQT__.TTF", "fonts dir");
        install.file("Data/FRIZQT__.TTF", "data root");
        install.file("Data/Interface/Glues/Common/Glues-WoW-Logo.blp", "logo");
        install.file("Interface/AddOns/Atlas/Atlas.toc", "install addon");
        let addons = Tree::new("addons");
        addons.file("Atlas/Atlas.toc", "benilla addon");
        let root = install.0.clone();
        let cell = LooseIndexCell::new(move || LooseIndex::build(Some(&root), &root.join("Data")));
        let loose = LooseFiles {
            addons: Some(addons.0.clone()),
            install: Some(Arc::new(cell)),
        };
        let logo = "Interface\\Glues\\Common\\Glues-WoW-Logo.blp";
        assert_eq!(
            body(loose.find(logo, LooseOpen::Index)).as_deref(),
            Some("logo")
        );
        assert_eq!(loose.find(logo, LooseOpen::Archives), None);
        let font = "Fonts\\FRIZQT__.TTF";
        assert_eq!(
            body(loose.find(font, LooseOpen::Index)).as_deref(),
            Some("fonts dir")
        );
        assert_eq!(
            body(loose.find(font, LooseOpen::BareNameFirst)).as_deref(),
            Some("data root")
        );
        assert_eq!(
            body(loose.find("Interface\\AddOns\\atlas\\ATLAS.toc", LooseOpen::Index)).as_deref(),
            Some("benilla addon")
        );
        assert_eq!(
            loose.find(
                "Interface/Glues/Common/Glues-WoW-Logo.blp",
                LooseOpen::Index
            ),
            None
        );
        assert_eq!(
            loose.find("Fonts/FRIZQT__.TTF", LooseOpen::BareNameFirst),
            None
        );
        assert_eq!(
            loose.find("Interface/AddOns/Atlas/Atlas.toc", LooseOpen::Index),
            None
        );
        let no_folder = LooseFiles {
            addons: None,
            install: loose.install.clone(),
        };
        assert_eq!(
            no_folder.find("Interface\\AddOns\\Atlas\\Atlas.toc", LooseOpen::Index),
            None,
            "the install's AddOns folder is never read"
        );
    }

    /// `SetTexture`'s probe and decode find a texture loose in the install by its name as written
    /// with `\`, and not when it is written with `/`, which the index's compare does not fold.
    #[test]
    fn a_loose_texture_answers_its_backslash_name_and_not_its_slash_name() {
        let data = benilla_formats::wow_data_or_skip!();
        let chain = Mutex::new(Chain::open(&data).expect("open the chain"));
        let bytes = chain
            .lock_recover()
            .read("Interface\\Icons\\INV_Misc_QuestionMark.blp")
            .expect("the question-mark icon is in the chain");
        let install = Tree::new("slash");
        let path = install.0.join("Interface/Glues/Loose/Custom.blp");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let root = install.0.clone();
        let cell = LooseIndexCell::new(move || LooseIndex::build(Some(&root), &root.join("Data")));
        let loose = LooseFiles {
            addons: None,
            install: Some(Arc::new(cell)),
        };
        let back = "Interface\\Glues\\Loose\\Custom";
        let slash = "Interface/Glues/Loose/Custom";
        assert!(sprite_found(&chain, &loose, back));
        assert!(sprite_dimensions(&chain, &loose, back).is_some());
        assert!(!sprite_found(&chain, &loose, slash));
        assert!(sprite_dimensions(&chain, &loose, slash).is_none());
    }

    /// An emptied index drops every sprite opened with the loose flag, hit or miss, and keeps those
    /// opened with flag 0; the same generation again drops nothing.
    #[test]
    fn a_new_index_generation_drops_the_loose_opened_sprites() {
        let mut c = SpriteCaches::default();
        let hit = Some(Handle::<Image>::default());
        c.sprites.insert("ui".into(), LooseOpen::Index, hit.clone());
        c.sprites.insert("miss".into(), LooseOpen::Index, None);
        c.sprites
            .insert("sun".into(), LooseOpen::Archives, hit.clone());
        c.tiled.insert("ui".into(), LooseOpen::Index, hit.clone());
        c.portraits
            .insert("p".into(), LooseOpen::Index, hit.clone());
        c.masks
            .insert("mask".into(), LooseOpen::Archives, hit.clone());
        c.resampled.insert(("ui".into(), 4, 4), hit.clone());
        c.sync(Some(0));
        assert!(
            c.sprites.get(&"ui".to_string()).is_some(),
            "an unchanged generation keeps all"
        );
        c.sync(Some(1));
        assert!(c.sprites.get(&"ui".to_string()).is_none());
        assert!(c.sprites.get(&"miss".to_string()).is_none());
        assert!(c.sprites.get(&"sun".to_string()).is_some());
        assert!(c.tiled.get(&"ui".to_string()).is_none());
        assert!(c.portraits.get(&"p".to_string()).is_none());
        assert!(c.masks.get(&"mask".to_string()).is_some());
        assert!(c.resampled.is_empty());
    }

    /// A `Blizzard*` folder holding its own `.toc`, any case, is moved aside and answers no path;
    /// one without it, or a folder of another name, answers as before.
    #[test]
    fn a_blizzard_folder_with_its_toc_is_moved_aside() {
        let addons = Tree::new("aside");
        addons.file("Blizzard_Foo/Blizzard_Foo.toc", "toc");
        addons.file("Blizzard_Foo/Foo.lua", "foo");
        addons.file("BLIZZARD_Bar/blizzard_bar.TOC", "toc");
        addons.file("BLIZZARD_Bar/Bar.lua", "bar");
        addons.file("Blizzard_AuctionUI/Blizzard_AuctionUI.pub", "pub");
        addons.file("Blizzard_AuctionUI/Templates.xml", "templates");
        addons.file("MyBlizzard/MyBlizzard.toc", "toc");
        addons.file("MyBlizzard/Mine.lua", "mine");
        let root = &addons.0;
        assert!(moved_aside(&root.join("Blizzard_Foo"), "Blizzard_Foo"));
        assert!(moved_aside(&root.join("BLIZZARD_Bar"), "BLIZZARD_Bar"));
        assert!(!moved_aside(
            &root.join("Blizzard_AuctionUI"),
            "Blizzard_AuctionUI"
        ));
        assert!(!moved_aside(&root.join("MyBlizzard"), "MyBlizzard"));
        let get = |p: &str| body(loose_addon_file(root, &normalize_path(p)).map(LooseFile::Plain));
        assert_eq!(get("Interface\\AddOns\\Blizzard_Foo\\Foo.lua"), None);
        assert_eq!(get("Interface\\AddOns\\blizzard_bar\\Bar.lua"), None);
        assert_eq!(
            get("Interface\\AddOns\\Blizzard_AuctionUI\\Templates.xml").as_deref(),
            Some("templates")
        );
        assert_eq!(
            get("Interface\\AddOns\\MyBlizzard\\Mine.lua").as_deref(),
            Some("mine")
        );
    }
    /// A TGA header of `w` by `h` at 32 bits with no alpha bits and `data` after it. `0x5a37e0`
    /// refuses the pixels, and `0x44a140` still makes the texture.
    fn refused_tga(w: u16, h: u16, data: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; 18];
        out[2] = 2;
        out[12..14].copy_from_slice(&w.to_le_bytes());
        out[14..16].copy_from_slice(&h.to_le_bytes());
        out[16] = 32;
        out[17] = 0x20;
        out.extend_from_slice(data);
        out
    }

    /// `0x44a140` returns the texture once the header opens, and the refused decode in `0x44a260`
    /// fills it opaque white (`0x44a2eb` into `0x448920`) at the size `0x448450` made it. A header
    /// that makes no texture fails the candidate as a missing file does, whether the pixels would
    /// decode or not, and so does a candidate the TGA loader does not read, or any load that is not
    /// `TextureCreate`'s.
    #[test]
    fn a_refused_tga_texture_is_opaque_white_at_its_headers_size() {
        let tga = "interface\\addons\\foo\\icon.tga";
        let blp = "interface\\addons\\foo\\icon.blp";
        let white =
            |candidate, bytes: &[u8]| decode_candidate(candidate, bytes, OnRefusal::WhiteDefault);
        let bytes = refused_tga(4, 2, &[0; 32]);
        assert!(
            decode_sprite_bytes(&bytes).is_err(),
            "the decoder refuses it"
        );
        assert_eq!(
            white(tga, &bytes),
            Candidate::Pixels(4, 2, vec![0xff; 4 * 2 * 4])
        );
        // The pixels need not be there at all.
        assert_eq!(
            white(tga, &refused_tga(1, 512, &[])),
            Candidate::Pixels(1, 512, vec![0xff; 512 * 4])
        );

        // Next candidate: not a size `0x448450` makes (decoded or not), a header that does not
        // open, a BLP candidate (`0x44a500` substitutes a green texture instead), BLP bytes.
        let mut wide = refused_tga(6, 1, &[9; 24]);
        wide[17] = 0x28;
        assert!(
            decode_sprite_bytes(&wide).is_ok(),
            "it decodes, and still fails the gate"
        );
        for (candidate, bytes) in [
            (tga, refused_tga(3, 2, &[0; 24])),
            (tga, refused_tga(2048, 2, &[0; 24])),
            (tga, wide),
            (tga, vec![2, 0, 2]),
            (blp, refused_tga(4, 2, &[0; 32])),
            (
                tga,
                [&b"BLP2"[..], &refused_tga(4, 2, &[0; 32])[4..]].concat(),
            ),
        ] {
            assert_eq!(white(candidate, &bytes), Candidate::Next, "{candidate}");
        }
        // A load that is no `TextureCreate`'s leaves them all to the next candidate.
        for bytes in [&bytes, &refused_tga(3, 2, &[0; 24])] {
            assert_eq!(
                decode_candidate(tga, bytes, OnRefusal::Miss),
                Candidate::Next
            );
        }

        // A zero size passes the gate (`0x44849d`) and draws nothing, trying no other candidate.
        assert_eq!(white(tga, &refused_tga(0, 2, &[])), Candidate::Nothing);

        // A TGA that decodes is its pixels, white or not.
        let mut ok = refused_tga(2, 1, &[1, 2, 3, 77, 4, 5, 6, 88]);
        ok[17] = 0x28;
        assert_eq!(
            white(tga, &ok),
            Candidate::Pixels(2, 1, vec![3, 2, 1, 77, 6, 5, 4, 88])
        );
    }

    /// The addon corpus' three `BigWigs` ready-check icons are 32-bit TGAs with no alpha bits:
    /// `0x44a140` returns each as a texture of its header's size once the header opens, and the
    /// refused decode leaves it white (`0x44a2eb`).
    #[test]
    fn the_corpus_ready_check_icons_are_textures_of_their_size() {
        let data = benilla_formats::wow_data_or_skip!();
        let corpus = benilla_formats::addon_corpus_or_skip!();
        let chain = Mutex::new(Chain::open(&data).expect("open the chain"));
        let loose = LooseFiles {
            addons: Some(corpus),
            install: None,
        };
        for name in ["Ready", "Waiting", "NotReady"] {
            let path = format!("Interface\\AddOns\\BigWigs\\Icons\\ReadyCheck-{name}");
            assert!(sprite_found(&chain, &loose, &path), "{name}");
            assert_eq!(
                sprite_dimensions(&chain, &loose, &path),
                Some((64, 64)),
                "{name}"
            );
            let (_, _, rgba) = decode_texture(&chain, &loose, &path, LooseOpen::Index).unwrap();
            assert!(rgba.iter().all(|&b| b == 0xff), "{name}: opaque white");
        }
    }

    /// Through the store: a refused `.tga` in the AddOns folder is a texture of its header's size
    /// for `SetTexture`'s loads. A header that makes no texture (`0x448450` takes powers of two
    /// below 1024) is a miss, its pixels decoded or not, and a zero size passes.
    #[test]
    fn a_refused_tga_in_the_addons_folder_is_a_texture_of_its_size() {
        let data = benilla_formats::wow_data_or_skip!();
        let chain = Mutex::new(Chain::open(&data).expect("open the chain"));
        let addons = Tree::new("white");
        let dir = addons.0.join("Foo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Icon.tga"), refused_tga(8, 4, &[0; 128])).unwrap();
        std::fs::write(dir.join("Odd.tga"), refused_tga(6, 4, &[0; 96])).unwrap();
        let loose = LooseFiles {
            addons: Some(addons.0.clone()),
            install: None,
        };
        let icon = "Interface\\AddOns\\Foo\\Icon";
        assert!(sprite_found(&chain, &loose, icon));
        assert_eq!(sprite_dimensions(&chain, &loose, icon), Some((8, 4)));
        let (w, h, rgba) = decode_texture(&chain, &loose, icon, LooseOpen::Index).unwrap();
        assert_eq!((w, h, rgba.len()), (8, 4, 8 * 4 * 4));
        assert!(rgba.iter().all(|&b| b == 0xff));
        // A load that is not `TextureCreate`'s still gets no pixels.
        assert!(decode_sprite(&chain, &loose, icon, LooseOpen::Index).is_none());
        // Not a size `0x448450` makes (`0x4484ae`-`0x4484d0`): `TextureCreate` fails the candidate
        // whether its pixels decode or not, and `SetTexture` answers nil.
        std::fs::write(dir.join("Wide.tga"), decodable_tga(6, 1)).unwrap();
        std::fs::write(dir.join("Fine.tga"), decodable_tga(2, 1)).unwrap();
        for (name, found, size) in [
            ("Odd", false, None),
            ("Wide", false, None),
            ("Fine", true, Some((2, 1))),
        ] {
            let path = format!("Interface\\AddOns\\Foo\\{name}");
            assert_eq!(sprite_found(&chain, &loose, &path), found, "{name}");
            assert_eq!(sprite_dimensions(&chain, &loose, &path), size, "{name}");
        }
        // A zero size passes the gate (`0x44849d`, `test ebx, ebx-1`), and draws nothing.
        std::fs::write(dir.join("Zero.tga"), refused_tga(0, 4, &[])).unwrap();
        let zero = "Interface\\AddOns\\Foo\\Zero";
        assert!(sprite_found(&chain, &loose, zero));
        assert_eq!(sprite_dimensions(&chain, &loose, zero), None);
    }

    /// A 32-bit TGA with 8 alpha bits, `w` by `h`, that the decoder reads.
    fn decodable_tga(w: u16, h: u16) -> Vec<u8> {
        let mut out = refused_tga(w, h, &vec![9; usize::from(w) * usize::from(h) * 4]);
        out[17] = 0x28;
        out
    }
    /// `SetTexture` runs per frame in the addons that set an icon every `OnUpdate`: a TGA's header
    /// is read once for a resolved file, and a read that fails is asked again.
    #[test]
    fn the_size_gate_reads_a_resolved_file_once() {
        let probe = SpriteProbe::default();
        let tga = "interface\\addons\\foo\\icon.tga";
        let good = refused_tga(8, 4, &[]);
        let reads = std::cell::Cell::new(0);
        let read = |bytes: Option<&Vec<u8>>| {
            reads.set(reads.get() + 1);
            bytes.cloned()
        };
        for _ in 0..3 {
            assert!(probe.gate("loose", tga, || read(Some(&good))));
        }
        assert_eq!(reads.get(), 1, "the same file answered from the memo");
        assert_eq!(probe.header_reads.get(), 1);

        // Another resolved file under the same name is its own question: the chain's copy, once a
        // loose one shadows it, or the other way round.
        assert!(!probe.gate("chain", tga, || read(Some(&refused_tga(3, 4, &[])))));
        assert!(!probe.gate("chain", tga, || read(Some(&good))));
        assert_eq!(reads.get(), 2);

        // A file that will not read says no and is asked again.
        assert!(!probe.gate("gone", tga, || read(None)));
        assert!(probe.gate("gone", tga, || read(Some(&good))));
        assert_eq!(reads.get(), 4);
    }

    /// The same, through the store: a second `SetTexture` of the path reads no file.
    #[test]
    fn a_second_probe_of_a_tga_reads_no_file() {
        let data = benilla_formats::wow_data_or_skip!();
        let chain = Mutex::new(Chain::open(&data).expect("open the chain"));
        let addons = Tree::new("probe");
        let dir = addons.0.join("Foo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Icon.tga"), refused_tga(8, 4, &[0; 128])).unwrap();
        std::fs::write(dir.join("Odd.tga"), refused_tga(6, 4, &[0; 96])).unwrap();
        let loose = LooseFiles {
            addons: Some(addons.0.clone()),
            install: None,
        };
        let probe = SpriteProbe::default();
        for _ in 0..3 {
            assert!(probe.found(&chain, &loose, "Interface\\AddOns\\Foo\\Icon"));
            assert!(!probe.found(&chain, &loose, "Interface\\AddOns\\Foo\\Odd"));
            assert!(!probe.found(&chain, &loose, "Interface\\AddOns\\Foo\\Missing"));
        }
        // One read each of the two files, and none of a name that is not there.
        assert_eq!(probe.header_reads.get(), 2);
    }
}
