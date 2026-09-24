//! **The font engine** — an on-demand glyph cache keyed by exact integer device-pixel size.
//!
//! ## What this replaced, and why
//!
//! Until decision 1342 this module *baked*: at startup (and, after 1296, whenever the raster
//! environment moved) it rasterized the whole charset for every registered face at a fixed
//! **ladder** of sizes, packed it into one texture, and published it. A request for a size the
//! ladder did not carry snapped to the nearest rung, and the finished quads were then rescaled by
//! `k = requested / snapped`.
//!
//! That rescale is gone, and with it the ladder. It had to be, because `k` was never a rendering
//! detail — it was a second, parallel definition of where text sits, and every seam that forgot to
//! apply it produced a measure/render disagreement: a caret 28 % past its own text, an
//! ellipsis eating a word that fits, a tooltip past its border, a Main Menu label
//! losing its last letter (1339). And one that names the mechanism better than any of them:
//! **letters that do not share a baseline.** That is what a rescale *is*. The emit pass rounds each
//! glyph onto the device-pixel grid — uniformly, so the line is flat — and the rescale then
//! multiplies each rounded position by a non-integer `k` about an anchor. Glyphs have different
//! vertical offsets (an `O` is not an `x` is not a `p`), so after the multiply every letter lands on
//! a *different* sub-pixel phase and resamples at a different offset. The line stops being a line.
//! No amount of care at the call sites fixes that; only not having a `k` does.
//!
//! ## What the real client does
//!
//! In the reference, a `CGxFont` exists per (face, flags, **exact pixel size**),
//! the size being `min(32, round(H · max(reqSize, 2/H)))` (`0x5ca030` → `[CGxFont+0x24c]`) —
//! integer device pixels, always. Glyphs are rasterized **on demand** by `NewCodeDesc` (`0x5cabd0`)
//! into a per-font `TSHashTable<codepoint → CharCodeDesc>` (`+0x30/+0x38`) and land in a
//! row-partitioned texture cache (the cell-size setter `0x5cf360`, cell size `em + 2·outline pad`,
//! free-slot search `0x5cf5a0`) of up to 8 pages (`CGxFont+0x18c`). One bitmap per (font,
//! codepoint), zero subpixel phases — the face is sized by `FT_Set_Pixel_Sizes` to a square integer
//! ppem and the module calls no `FT_Set_Transform` anywhere.
//!
//! There is no size ladder and no snapping. **`k` has no counterpart in the ordinary UI path**,
//! and the reason is precise: `CSimpleFontString`'s constructor sets the one-to-one bit
//! (`+0x120 & 0x200`, `0x770dd3`), which makes `GetFontHeight` return the font's own quantized em
//! back — so the draw's `scale = ScreenToPixelHeight / [CGxFont+0x24c]` comes out exactly `1.0`
//! and every glyph is an integer 1 texel : 1 pixel blit.
//!
//! So: [`TextEngine::ppem`] rounds a logical height to whole device pixels, everything downstream
//! is keyed by that integer, and [`TextEngine::logical_size`] is the `0x200` bit — the size a
//! string measures and lays out at is the size its glyphs were rasterized at, not the float that
//! was asked for. That is the mechanism that makes a rescale unnecessary, and it is the one worth
//! copying. (It is also the modern idiomatic design: cosmic-text's `SwashCache` over a shelf
//! allocator, which is what glyphon and egui do.)
//!
//! ## Where this deliberately goes further — and where it is thinner
//!
//! Being exact about the divergences, because "the client does it this way" is doing a lot of work
//! above and it should not be doing more than it has earned:
//!
//! - **The client scales cells in three places, and we scale in none.** `SetTextHeight` clears the
//!   one-to-one bit (`0x771600`) and draws at a size the atlas was not rasterized for; a requested
//!   em past 32 px clamps the raster while the numerator keeps growing; and every 3-D unit name
//!   goes through `UNIT_NAME_FONT` (created at size `0.99f`, so its em is permanently 32) with
//!   string flag `0x80` — the explicit magnified mode, ±½-texel UV insets and all. We rasterize
//!   all three at their true size instead. That is not a new posture: it is exactly the recorded
//!   divergence at [`super::FONTSTRING_EM_CAP`] — the 32-px ceiling is 2004 raster-memory
//!   budgeting, and following it literally on a modern display converges every string toward the
//!   same size and stretches the ones above it (the era's blurry big crits). **Crisper than the
//!   reference, on purpose, in the same three places we had already chosen to be.**
//! - **Measuring rasterizes, in the client.** All five measure/layout kernels
//!   (`0x5c6940`, `0x5c6b70`, `0x5c6c50`, `0x5c7300`, `0x5c7470`) call `NewCodeDesc`
//!   unconditionally per character, and its miss branch runs `FT_Load_Glyph` + `FT_Render_Glyph`
//!   with no flag that can skip the render — the advance the measure needs is written *by the
//!   rasterizer*. [`TextEngine::ensure_metrics`] is therefore a **divergence**: we shape for the
//!   metrics and skip the bitmap. It produces the same number (the step law reads only the
//!   advance) and it is what makes a `GetStringWidth` probe over a string nobody draws cost no
//!   texture at all.
//! - **The client evicts, we reset.** `NewCodeDesc`'s full-atlas path (`0x5cad2b`) is a true LRU
//!   over `[CGxFont+0x64..0x6c]`: evict the single least-recently-used glyph, free its cell, repack
//!   into the hole, and dirty every string on that font that used that page. We drop everything at
//!   once instead ([`super::pack::Sheet::reset`]) — coarser, but our sheet is far larger than the
//!   client's 256×256 pages, and a shelf allocator cannot reclaim an interior cell without the
//!   repack the client's row lists are built for. Recorded, with the occupancy instrument that
//!   would overturn it.
//! - **The client flushes on a viewport change; we do not.** `0x5c2b50` (edge-triggered from the
//!   OS resize event) walks every live `CGxFont` and calls `0x5ca6f0`, which clears the whole glyph
//!   cache in place and re-issues `FT_Set_Pixel_Sizes` — keeping the FreeType face. That is a
//!   perfectly good answer to raster-size churn and we may yet want it; what stopped us taking it
//!   now is that our nameplate meshes bake UVs, so a flush costs a mesh rebuild on every resize
//!   frame, while simply *keeping* the dead cells costs only sheet space. Same instrument decides.
//!
//! ## The shape of the thing
//!
//! Two caches, filled by one operation and read by two very different callers:
//!
//! - **[`CharCell`]** — per `(face, ppem, char)`: which glyphs the character shapes to, their
//!   advances, and the pre-summed floor the width law needs. **Never touches the GPU**, which is
//!   what lets the script VM's synchronous measurer fill it from inside a Lua call.
//! - **The cells** — per `(face, glyph, ppem, outline radius)`: a rasterized bitmap packed into a
//!   [page][super::pack]. Only the emit pass asks for these.
//!
//! Both live in one engine behind one lock, shared by the render path and the VM's measurer, so a
//! string measured mid-tick and the same string measured at extract are not merely equal numbers —
//! they are the same lookup in the same table. (That was already 1289's property; it survives the
//! rewrite because it is the property that keeps measure and render from drifting.)
//!
//! **Lock discipline:** the guard is taken by a leaf entry point and released before it returns.
//! Nothing may hold it across a call into the script VM — the VM's measurer takes the same lock.

mod faces;
mod gpu;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use bevy::image::Image;
use bevy::math::Rect;
use bevy::prelude::*;
use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap,
};

use benilla_assets::{LockRecover, WorldAssets};

use faces::{hhea_ascent_ratio, register_font, CLIENT_FONTS};
pub(crate) use gpu::UiTextPlugin;

use super::outline::outlined_cell;
use super::pack::{Cell, Sheet};

/// The raster size floor, in device pixels — the client's own `max(2, …)` (`0x5ca030`). Below this
/// a face has no readable ink and FreeType's metrics degenerate.
const MIN_PPEM: u16 = 2;

/// The raster size ceiling, in device pixels. **Not** the client's `min(32, …)`: that cap is a
/// 2004 raster-memory ceiling which we deliberately apply in LOGICAL units instead, at the
/// [`super::fontstring_em`] seam, so a capped zone splash scales with the screen and stays crisp
/// (the recorded divergence — see [`super::FONTSTRING_EM_CAP`]). World text
/// ([`crate::combat_text`], the nameplates) sizes by its own viewport laws and passes through
/// uncapped. This is the backstop under all of that: a request that reaches it is a bug upstream,
/// not a font to rasterize.
const MAX_PPEM: u16 = 256;

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The cache's own types
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// One registered face.
struct Face {
    id: fontdb::ID,
    /// The path the caller asked for — carried so the substitution WARN can name it (2123).
    path: String,
    family: String,
    /// The three CSS axes `cosmic-text` matches on, read off the face itself. See
    /// [`faces::Registered`]: naming only the family asks for a normal-weight, normal-style face
    /// and silently gets a different one when the named face is not that.
    weight: fontdb::Weight,
    style: fontdb::Style,
    stretch: fontdb::Stretch,
    /// `hhea.asc / (asc + |desc|)` — see [`hhea_ascent_ratio`]. Friz's ≈ 0.794 is the fallback for
    /// a face whose tables would not parse (never the four shipped client fonts).
    ascent_ratio: f32,
}

/// `(face, glyph, ppem, outline radius)` — the raster cache's key. The size term is the **exact
/// integer device-pixel size** the glyph was rasterized at, which is the whole point: there is one
/// bitmap per size a caller actually asked for, and it is drawn one texel per device pixel.
type GlyphKey = (fontdb::ID, u16, u16, u8);

/// One rasterized cell. **All fields are PHYSICAL px** — the layout divides by
/// [`TextEngine::dpi`] on the way into logical space.
#[derive(Clone, Copy, Debug)]
pub(super) struct GlyphInfo {
    pub(super) uv: Rect,
    pub(super) px_w: f32,
    pub(super) px_h: f32,
    /// Swash convention: `left` rightward from the pen, `top` upward from the baseline.
    pub(super) bearing_x: f32,
    pub(super) bearing_top: f32,
}

/// One glyph a character shaped to, as the cache remembers it — everything the **pen** needs, with
/// no bitmap involved.
#[derive(Clone, Copy, Debug)]
pub(super) struct GlyphRef {
    pub(super) glyph_id: u16,
    /// Swash's rasterization key for this glyph at this ppem, at zero subpixel offset — kept so a
    /// cell can be rasterized later (for a different outline radius) without re-shaping.
    key: CacheKey,
    /// The face's own advance, physical px, **unfloored**. The step law floors it
    /// ([`super::layout::client_step`]).
    pub(super) advance: f32,
    /// Vertical offset from the line baseline, physical px (0 for every glyph these four Latin
    /// faces shape — cached rather than assumed).
    pub(super) y_off: f32,
}

/// One character's whole contribution at one `(face, ppem)`.
///
/// Two numbers rather than one because the client's step law ([`super::layout::client_step`]) adds
/// its bias **per glyph**, not per character: a character that shapes to two glyphs takes two
/// biases. Every character these four faces shape takes one — `glyphs` is length 1 everywhere
/// today — but the law stays the law if that ever stops being true.
pub(super) struct CharCell {
    pub(super) glyphs: Vec<GlyphRef>,
    /// Σ `advance.floor()` over the glyphs, in **physical** px — the width law's per-character
    /// term, pre-summed so a measure is a hash lookup and an add.
    pub(super) floor_sum: f32,
}

/// What the cache instrument (`WOW_GLYPH_CACHE=1`) counts.
#[derive(Default, Clone, Copy)]
struct CacheStats {
    chars_shaped: u64,
    cells_rasterized: u64,
    resets: u64,
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The engine
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The two stores a font path can come from — the same pair, in the same order, that
/// [`benilla_assets::WorldAssets`]'s sprite decoder walks (generalised by 2103).
///
/// Chain first: no MPQ holds an `Interface\AddOns\` path, so the order is unobservable and every
/// client path stays on exactly the code it always ran.
struct FontSource {
    chain: Arc<Mutex<benilla_formats::Chain>>,
    /// The ONE AddOns root, from `ui_script::addons::root()` — `None` under `$WOW_CAPTURE`, which
    /// is what keeps a capture hermetic for free.
    loose_root: Option<std::path::PathBuf>,
}

/// The font engine: faces, the two caches, and the texture pages.
pub(crate) struct TextEngine {
    font_system: FontSystem,
    swash: SwashCache,
    faces: Vec<Face>,
    /// Font path (lowercased) → index into [`Self::faces`]. Seeded with the four client faces and
    /// grown on demand by [`Self::face_for`] — an addon's own TTF is a path like any other.
    path_to_face: HashMap<String, usize>,
    /// Where a face the map does not carry is read from: the patch chain, then the one AddOns
    /// folder for `Interface\AddOns\…` paths ([`FontSource`]). `None` in a VM
    /// with no install, which is the only state in which a font path cannot resolve at all.
    source: Option<FontSource>,
    /// Paths that failed to load, so a miss costs one read and one WARN rather than one per
    /// measure. Cleared by nothing: a font file does not appear mid-session.
    missing_fonts: HashSet<String>,
    /// The fallback face (Friz Quadrata) for a FontString with no/unknown font path.
    default_face: usize,
    /// The window's `scale_factor` — physical px per logical px. Every raster size derives from it
    /// ([`Self::ppem`]), so moving it invalidates no cell: it simply means new sizes get asked for.
    /// (The *measures* answered under the old one are stale, which is the extract pass's business,
    /// not the cache's.)
    dpi: f32,
    /// `(face, ppem, char) → what the pen needs`. **Never touches the GPU** — this is the half the
    /// script VM's measurer fills from inside a Lua call.
    chars: HashMap<(usize, u16, char), CharCell>,
    /// `(face, glyph, ppem, radius) → cell`. `None` records a glyph with no ink (a space) or one
    /// the pages could not fit, so a miss is paid for exactly once.
    cells: HashMap<GlyphKey, Option<GlyphInfo>>,
    sheet: Sheet,
    /// Bumped when the cache [resets][Sheet::reset] — the **only** event that can move a UV, and
    /// therefore the only thing a cross-frame holder of glyph UVs ([`crate::nameplates`]) has to
    /// watch. Under the size ladder this fired on every window resize; now it fires when a session
    /// has minted more distinct raster sizes than the pages hold, which is to say almost never.
    generation: u64,
    /// Set when an allocation failed. Acted on at the frame boundary, never mid-string.
    reset_pending: bool,
    /// Characters no face could shape, and sizes past the ceiling — reported once each rather than
    /// once per frame.
    complained: HashSet<char>,
    /// Faces the shaper answered for with a different font — warned once each (2123).
    substituted: HashSet<usize>,
    over_ceiling: bool,
    stats: CacheStats,
}

/// A `cosmic-text` font system over an **empty** database — the client's faces are the only faces.
///
/// `FontSystem::new()` and `new_with_fonts()` both call `fontdb::Database::load_system_fonts()`
/// (`cosmic-text-0.16.0/src/font/system.rs:400`), which puts every font on the machine into the
/// same pool as the four client TTFs. That is not a harmless extra: `get_font_matches` does not
/// select a face, it *orders* every face in the database and shapes with the first one that can
/// draw the character (`system.rs:326-369`), so a family the query cannot match — or a single
/// character the client face happens to lack — is answered by whatever the machine has. It is
/// invisible by construction: the text draws, at the right size, in a face nobody asked for.
/// Measured on this machine: the reference's own `Fonts\ARIALN.TTF` was being
/// shaped by macOS's Arial Narrow, and Friz Quadrata by `.SFNS-Regular`, the system UI font.
///
/// So the pool holds exactly what a 1.12 client has: the four TTFs out of `fonts.MPQ`, plus
/// whatever faces an addon ships. A character none of them carries draws nothing
/// and warns once — which is both what the reference does and what our own width law already
/// says ("a character no face can shape contributes 0"): with a system fallback in the pool the
/// shaper-less measure and the shaped draw silently disagreed about that character's width.
///
/// The locale is `en-US` because everything else about our string data is (the `WORDS` table, the
/// GlobalStrings fallbacks); it only picks which *language's* family name a face answers to.
fn client_font_system() -> FontSystem {
    FontSystem::new_with_locale_and_db("en-US".to_string(), fontdb::Database::new())
}

impl TextEngine {
    /// Read the client TTFs through the app's own patch chain ([`WorldAssets::chain`] — never
    /// `std::fs`) and register them. `None` if Friz Quadrata (the fallback face) is unreadable, in
    /// which case text simply will not render — the same graceful-absence posture
    /// [`crate::ui_script`]'s extraction takes.
    fn load(world_assets: &WorldAssets, images: &Assets<Image>, dpi: f32) -> Option<Self> {
        let mut font_system = client_font_system();
        let mut faces: Vec<Face> = Vec::new();
        let mut path_to_face = HashMap::new();
        for &path in CLIENT_FONTS {
            let bytes = {
                let chain = world_assets.chain.lock_recover();
                match chain.read(path) {
                    Ok(b) => b,
                    Err(e) => {
                        warn!("ui_text: failed to read {path} from the patch chain: {e:#}");
                        continue;
                    }
                }
            };
            let ascent_ratio = hhea_ascent_ratio(&bytes).unwrap_or(0.794);
            match register_font(&mut font_system, bytes) {
                Ok(r) => {
                    path_to_face.insert(path.to_ascii_lowercase(), faces.len());
                    faces.push(Face {
                        id: r.id,
                        path: path.to_string(),
                        family: r.family,
                        weight: r.weight,
                        style: r.style,
                        stretch: r.stretch,
                        ascent_ratio,
                    });
                }
                Err(e) => warn!("ui_text: failed to register {path}: {e:#}"),
            }
        }
        // Friz is index 0 in CLIENT_FONTS and is the fallback; without it nothing downstream has a
        // face to fall back to.
        let default_face = *path_to_face.get(&CLIENT_FONTS[0].to_ascii_lowercase())?;
        info!(
            "ui_text: font engine ready — {} face(s), glyphs rasterized on demand at {dpi}× \
             device pixels",
            faces.len()
        );
        Some(Self {
            font_system,
            swash: SwashCache::new(),
            faces,
            path_to_face,
            source: Some(FontSource {
                chain: world_assets.chain.clone(),
                loose_root: crate::ui_script::addons::root(),
            }),
            missing_fonts: HashSet::new(),
            default_face,
            dpi,
            chars: HashMap::new(),
            cells: HashMap::new(),
            sheet: Sheet::new(images),
            generation: 0,
            reset_pending: false,
            complained: HashSet::new(),
            substituted: HashSet::new(),
            over_ceiling: false,
            stats: CacheStats::default(),
        })
    }

    /// The face a font path resolves to, loading it on first use, or the fallback.
    ///
    /// **The four client TTFs are the ones we know the names of, not the ones that exist**.
    /// `SetFont`/`<FontString font=>`/`CreateFont` take an arbitrary path, and an
    /// addon that ships its own faces — Mik's Scrolling Battle Text ships thirty-one — names them
    /// `Interface\Addons\<Addon>\Fonts\<x>.ttf`. Before this, every one of those silently
    /// resolved to Friz Quadrata: the text drew, in the wrong face, with nothing anywhere saying
    /// so. So a path the map does not carry is READ (chain, then the loose AddOns folder — the
    /// same two stores in the same order 1322 gave textures) and registered.
    ///
    /// A path that will not load is remembered in [`Self::missing_fonts`] and warns once, then
    /// falls back to Friz exactly as before — the reference degrades the same way (a `CGxFont`
    /// that fails to build leaves the string on the font object it inherits).
    pub(super) fn face_for(&mut self, path: Option<&str>) -> usize {
        let Some(p) = path.filter(|p| !p.is_empty()) else {
            return self.default_face;
        };
        let key = p.to_ascii_lowercase();
        if let Some(&i) = self.path_to_face.get(&key) {
            return i;
        }
        if self.missing_fonts.contains(&key) {
            return self.default_face;
        }
        match self.load_face(p, &key) {
            Some(i) => i,
            None => {
                warn!(
                    "ui_text: font miss: '{p}' does not resolve in the patch chain or the AddOns \
                     folder — falling back to {}",
                    CLIENT_FONTS[0]
                );
                self.missing_fonts.insert(key);
                self.default_face
            }
        }
    }

    /// Read and register one font path. `None` if no store has it or the bytes are not a face.
    fn load_face(&mut self, path: &str, key: &str) -> Option<usize> {
        let source = self.source.as_ref()?;
        let bytes = read_font_bytes(source, path)?;
        let ascent_ratio = hhea_ascent_ratio(&bytes).unwrap_or(0.794);
        let r = register_font(&mut self.font_system, bytes)
            .inspect_err(|e| warn!("ui_text: failed to register {path}: {e:#}"))
            .ok()?;
        let index = self.faces.len();
        self.faces.push(Face {
            id: r.id,
            path: path.to_string(),
            family: r.family,
            weight: r.weight,
            style: r.style,
            stretch: r.stretch,
            ascent_ratio,
        });
        self.path_to_face.insert(key.to_string(), index);
        info!("ui_text: loaded font face {path}");
        Some(index)
    }

    /// **The size law.** A logical height becomes the exact integer device-pixel size it will be
    /// rasterized and drawn at — the client's `round((height/768)·deviceH)` with the 768-seam
    /// already folded into `logical` by the caller ([`super::drawn_px`]), clamped to the raster
    /// bounds ([`MIN_PPEM`]/[`MAX_PPEM`]).
    ///
    /// Everything downstream is keyed by this integer. It is why there is no `k`: the size a string
    /// is measured at, the size its glyphs are rasterized at, and the size it is drawn at are one
    /// number by construction, rather than three numbers kept in agreement by discipline.
    pub(super) fn ppem(&mut self, logical: f32) -> u16 {
        let px = (logical * self.dpi).round();
        if !px.is_finite() {
            return MIN_PPEM;
        }
        if px > f32::from(MAX_PPEM) && !self.over_ceiling {
            self.over_ceiling = true;
            warn!(
                "ui_text: a {logical} logical-px request rasterizes at {px} device px, past the \
                 {MAX_PPEM} ceiling — clamped. A size this large is an upstream bug, not a font."
            );
        }
        (px as i64).clamp(i64::from(MIN_PPEM), i64::from(MAX_PPEM)) as u16
    }

    /// The **logical** height a ppem draws at — `ppem / dpi`. The pitch, the block height and every
    /// measured extent are this, not the height that was requested: the request rounds to whole
    /// device pixels first, exactly as the client rounds it, and everything then agrees with what
    /// is actually on the screen.
    pub(super) fn logical_size(&self, ppem: u16) -> f32 {
        f32::from(ppem) / self.dpi
    }

    /// Physical px per logical px — the divisor the layout uses to bring cell metrics back into
    /// logical space.
    pub(super) fn dpi(&self) -> f32 {
        self.dpi
    }

    /// The face's baseline-ascender fraction — the `[CGxFont+0x17c]` load_param the layout seats
    /// each line's baseline with.
    pub(super) fn ascent_ratio_of(&self, face: usize) -> f32 {
        self.faces.get(face).map_or(0.794, |f| f.ascent_ratio)
    }

    /// The **logical** size a request actually draws at — [`Self::ppem`] rounded and back again.
    ///
    /// The crate-facing shape of the size law, for the world-pass callers that have to normalize
    /// their own geometry against it ([`crate::nameplates`]'s mesh bake). Everything inside
    /// `ui_text` gets it from the resolved spec instead.
    pub(crate) fn drawn_size(&mut self, logical: f32) -> f32 {
        let ppem = self.ppem(logical);
        self.logical_size(ppem)
    }

    /// The baseline-ascender fraction for a font path — the `[CGxFont+0x17c]` load_param, for a
    /// caller that already holds the lock.
    pub(crate) fn ascent_ratio(&mut self, path: Option<&str>) -> f32 {
        let face = self.face_for(path);
        self.ascent_ratio_of(face)
    }

    /// Make sure every character of `text` is in the caches at this `(face, ppem, radius)`,
    /// rasterizing whatever is missing. Call it once before walking a string; every
    /// [`Self::char_cell`] / [`Self::cell`] lookup afterwards is a hit.
    ///
    /// The client has no such pre-pass — its layout kernels call `NewCodeDesc` (`0x5cabd0`) per
    /// character as they walk, which is the same work in a different order. (`AllGlyphsCached`
    /// `0x5c9fa0` is *not* it, despite the name: its one caller is `0x5cd3f0` and it gates
    /// geometry invalidation after an eviction, never rasterization.) Hoisting it here is what
    /// lets the walk itself take `&TextEngine` and stay a pure table read.
    pub(super) fn ensure_str(&mut self, face: usize, ppem: u16, radius: u8, text: &str) {
        for ch in text.chars() {
            self.ensure_char(face, ppem, Some(radius), ch);
        }
    }

    /// [`Self::ensure_str`]'s **metrics-only** twin: shape what is missing, rasterize nothing.
    ///
    /// This is the split that makes the whole design work. A width needs only the face and the
    /// ppem — no bitmap, no packing, no GPU — so a string the script VM measures inside a Lua call
    /// costs a shaping and a hash insert, and a string that is measured but never drawn (every
    /// `GetStringWidth` probe, every wrap candidate the ellipsis seam backs off through) never
    /// touches the sheet at all.
    ///
    /// **A deliberate divergence**, and worth naming as one: the real client's measure path
    /// rasterizes (module doc). The number is identical either way — the step law reads only the
    /// advance, which `FT_Load_Glyph` fixes before the render — so what we skip is work, not
    /// fidelity.
    pub(super) fn ensure_metrics(&mut self, face: usize, ppem: u16, text: &str) {
        for ch in text.chars() {
            self.ensure_char(face, ppem, None, ch);
        }
    }

    /// Move the raster environment under a test. Production drives this from the window
    /// ([`publish_sheet`]); nothing else may set it, because a mid-frame change would put two
    /// raster sizes in one laid-out string.
    #[cfg(test)]
    pub(super) fn set_dpi_for_test(&mut self, dpi: f32) {
        self.dpi = dpi;
    }

    /// One character: shape it alone (filling its [`CharCell`]) and rasterize its glyphs at
    /// `radius`.
    ///
    /// **Shaped alone, deliberately.** The advance that lands in the width sum must be the face's
    /// own advance for the glyph, carrying no neighbour term — the client's law (`ComputeStep`
    /// `0x5ca2d0`) drops kerning, so a string's width is a pure function of its characters. Shaping
    /// each character by itself is what makes that true rather than approximately true, and it is
    /// what lets one table answer a measure and a draw.
    fn ensure_char(&mut self, face: usize, ppem: u16, radius: Option<u8>, ch: char) {
        if let Some(known) = self.chars.get(&(face, ppem, ch)) {
            let Some(radius) = radius else { return };
            // The pen metrics are there; the cells for THIS radius may not be (an outlined font
            // asking for a character a plain one already drew).
            let glyphs = known.glyphs.clone();
            let Some(id) = self.faces.get(face).map(|f| f.id) else {
                return;
            };
            for g in &glyphs {
                self.ensure_cell(id, ppem, radius, g);
            }
            return;
        }
        // **A control character is not a missing glyph.** `\n` reaches here because the pre-warm
        // walks raw text (`ensure_str`), and no face shapes it — so it used to pay a full shape,
        // then warn as if a face were missing one. Caching the miss stopped the repeat work but
        // still left the warn, which is noise about a character that is SUPPOSED to have no glyph:
        // the markup parser consumes `\n` as a line break and it never reaches a draw. Answered
        // here, before the shape, so the warn stays meaningful for a real missing glyph.
        if ch.is_control() {
            self.chars.insert(
                (face, ppem, ch),
                CharCell {
                    glyphs: Vec::new(),
                    floor_sum: 0.0,
                },
            );
            return;
        }
        let Some(f) = self.faces.get(face) else {
            return;
        };
        // Owned, because the buffer below borrows `self.font_system` mutably.
        let (face_id, family, weight, style, stretch) =
            (f.id, f.family.clone(), f.weight, f.style, f.stretch);
        let attrs = Attrs::new()
            .family(Family::Name(&family))
            .weight(weight)
            .style(style)
            .stretch(stretch);
        let px = f32::from(ppem);
        let mut glyphs: Vec<GlyphRef> = Vec::new();
        let mut floor_sum = 0.0f32;
        let mut shaped_by = None;
        {
            let mut buf = Buffer::new(&mut self.font_system, Metrics::new(px, px));
            buf.set_wrap(&mut self.font_system, Wrap::None);
            let mut one = [0u8; 4];
            buf.set_text(
                &mut self.font_system,
                ch.encode_utf8(&mut one),
                &attrs,
                Shaping::Advanced,
                None,
            );
            buf.shape_until_scroll(&mut self.font_system, false);
            for run in buf.layout_runs() {
                for g in run.glyphs {
                    // A lone first glyph on the line has x == y == 0.0, so this is the
                    // zero-subpixel canonical rasterization for (face, glyph, ppem).
                    let physical = g.physical((0.0, 0.0), 1.0);
                    shaped_by.get_or_insert(g.font_id);
                    glyphs.push(GlyphRef {
                        glyph_id: g.glyph_id,
                        key: physical.cache_key,
                        advance: g.w,
                        y_off: g.y,
                    });
                    floor_sum += g.w.floor();
                }
            }
        }
        // **The shaper's answer is checked against the ask**. Naming a face to
        // `cosmic-text` is a *query*, not a selection: it walks every registered face and takes
        // the first that can draw the character, so a family it cannot match is answered by some
        // other face — correct-looking text in the wrong one, which is exactly the silent failure
        // 2103 set out to end and did not, because it never compared what came back. The glyph
        // ids and the advances below are then that other face's, so the string measures wrong
        // too. Once per face, naming both sides.
        if let Some(got) = shaped_by.filter(|&got| got != face_id) {
            if self.substituted.insert(face) {
                let got_name = self
                    .font_system
                    .db()
                    .face(got)
                    .map_or_else(|| format!("{got:?}"), |i| i.post_script_name.clone());
                let want = self.faces.get(face).map_or("?", |f| f.path.as_str());
                warn!(
                    "ui_text: '{want}' loaded as family {family:?} (weight {}, style {style:?}) \
                     but {ch:?} shaped in {got_name} instead — text in this face draws and \
                     measures in the wrong one",
                    weight.0
                );
            }
        }
        if glyphs.is_empty() {
            // No face shaped it — it draws nothing and, per the width law's own note, measures
            // nothing. Report once, not once per frame.
            if self.complained.insert(ch) {
                warn!("ui_text: no glyph for {ch:?} in any registered face");
            }
            // **Cache the miss.** `complained` only silenced the WARNING; the shape itself was
            // re-run on every call, because the insert below was never reached. The pre-warm walks
            // `text.chars()` raw (`ensure_str`), so every `\n` in a drawn multi-line string paid a
            // full `Buffer::new` + `set_text` + `shape_until_scroll` per FontString per frame,
            // forever — `RAID_DESCRIPTION` alone carries three. An empty cell is exactly what the
            // consumers already do with the `None` they used to get: `char_cell` hands back zero
            // glyphs, so the pen steps nothing and draws nothing, and `floor_sum` sums nothing.
            self.chars.insert(
                (face, ppem, ch),
                CharCell {
                    glyphs,
                    floor_sum: 0.0,
                },
            );
            return;
        }
        self.stats.chars_shaped += 1;
        if let Some(radius) = radius {
            for g in &glyphs {
                let g = *g;
                self.ensure_cell(face_id, ppem, radius, &g);
            }
        }
        self.chars
            .insert((face, ppem, ch), CharCell { glyphs, floor_sum });
    }

    /// Rasterize + pack one glyph's cell, unless it is already there.
    fn ensure_cell(&mut self, face_id: fontdb::ID, ppem: u16, radius: u8, g: &GlyphRef) {
        let key: GlyphKey = (face_id, g.glyph_id, ppem, radius);
        if self.cells.contains_key(&key) {
            return;
        }
        // Copied out rather than borrowed, so the pages can be written below.
        let raster = {
            let Self {
                swash, font_system, ..
            } = self;
            swash
                .get_image(font_system, g.key)
                .as_ref()
                .map(|i| (i.placement, i.data.clone()))
        };
        let Some((placement, cov)) = raster else {
            self.cells.insert(key, None);
            return;
        };
        let (w, h) = (placement.width, placement.height);
        if w == 0 || h == 0 {
            // Whitespace / zero-ink glyph: real metrics, no cell.
            self.cells.insert(key, None);
            return;
        }
        self.stats.cells_rasterized += 1;
        // An outlined cell grows by `pad` on every side; the bearings move out with it, the
        // advance does not (the step law owns tracking).
        let (uv, cw, ch, bx, bt) = if radius == 0 {
            (
                self.sheet.alloc(w, h, &Cell::Coverage(&cov)),
                w,
                h,
                placement.left as f32,
                placement.top as f32,
            )
        } else {
            let (rgba, ow, oh, pad) = outlined_cell(&cov, w, h, radius, self.dpi);
            (
                self.sheet.alloc(ow, oh, &Cell::Rgba(&rgba)),
                ow,
                oh,
                placement.left as f32 - pad as f32,
                placement.top as f32 + pad as f32,
            )
        };
        match uv {
            Some(uv) => {
                self.cells.insert(
                    key,
                    Some(GlyphInfo {
                        uv,
                        px_w: cw as f32,
                        px_h: ch as f32,
                        bearing_x: bx,
                        bearing_top: bt,
                    }),
                );
            }
            None => self.note_exhausted(key),
        }
    }

    /// The pages could not fit a cell. Record nothing for it (so the glyph draws nothing for at
    /// most one frame) and ask for a reset at the frame boundary.
    ///
    /// A shelf allocator cannot reclaim an interior cell, so there is no piecemeal eviction to
    /// reach for: the honest move is to drop everything and refill from what is actually on screen,
    /// which costs one frame of rasterization for the visible text. What must NOT happen is doing
    /// it here — glyphs already pushed this frame hold UVs into the pages, and moving them mid-pass
    /// would draw this frame's text as fragments of other letters (the exact shape of 1339's fault
    /// 2, which is worth not rebuilding).
    fn note_exhausted(&mut self, key: GlyphKey) {
        self.cells.insert(key, None);
        if !self.reset_pending {
            self.reset_pending = true;
            let (used, total) = self.sheet.occupancy();
            warn!(
                "ui_text: glyph sheet full ({used}/{total} texels, {} cells) — resetting at the \
                 frame boundary",
                self.cells.len()
            );
        }
    }

    /// One character's pen metrics — `None` for a character no face can shape (it draws nothing and
    /// measures nothing, the one narrow place the measure differs from the render pen).
    pub(super) fn char_cell(&self, face: usize, ppem: u16, ch: char) -> Option<&CharCell> {
        self.chars.get(&(face, ppem, ch))
    }

    /// One glyph's cell — `None` for a zero-ink glyph, and for the one frame after an exhaustion.
    pub(super) fn cell(
        &self,
        face: usize,
        ppem: u16,
        radius: u8,
        glyph_id: u16,
    ) -> Option<GlyphInfo> {
        let id = self.faces.get(face)?.id;
        self.cells
            .get(&(id, glyph_id, ppem, radius))
            .copied()
            .flatten()
    }

    /// The one texture every glyph quad samples.
    pub(super) fn sheet_image(&self) -> Handle<Image> {
        self.sheet.handle()
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The Bevy face of it
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The font engine as the app holds it: one [`TextEngine`] behind one lock, plus the per-region
/// ellipsis memo.
///
/// The lock is what lets the same engine answer the render path and the script VM's synchronous
/// measurer ([`super::AtlasMeasurer`]), which reaches in from inside a Lua call. See the module
/// doc's lock discipline.
#[derive(Resource)]
pub(crate) struct UiFontAtlas {
    engine: Arc<Mutex<TextEngine>>,
    /// Mirrored out of the engine each frame so the extract gate can read it without taking the
    /// lock. Moves only on a cache reset.
    pub(crate) generation: u64,
    /// Per-region ellipsis display strings ([`super::EllipsisMemo`]) — the client's own
    /// `CGxString+0xf8` cache, keyed by the inputs instead of a dirty flag.
    pub(super) ellipsis: super::EllipsisMemo,
}

impl UiFontAtlas {
    /// Take the engine lock. Release it before calling anything that might re-enter through the
    /// VM's measurer.
    pub(crate) fn lock(&self) -> MutexGuard<'_, TextEngine> {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The shared engine, for the VM's measurer.
    pub(crate) fn engine(&self) -> Arc<Mutex<TextEngine>> {
        Arc::clone(&self.engine)
    }

    /// The one texture every glyph draws from — for the world-pass consumers
    /// ([`crate::nameplates`]) that bind glyph cells onto 3-D geometry instead of UI quads.
    ///
    /// Stable for the life of the process, which is new: under the size ladder every re-bake
    /// published a fresh `Handle<Image>`, and a cache that kept the old one drew the new bake's
    /// UVs through the old bake's texture. Cells are written into this
    /// texture in place now, so there is no successor to go stale against.
    pub(crate) fn image(&self) -> Handle<Image> {
        self.lock().sheet_image()
    }
}

/// Read one font path from the two stores, chain first —
/// [`benilla_assets::read_chain_or_loose`], the one rule every by-path addon asset resolves by.
///
/// It inherits that rule whole rather than restating it: only `Interface\AddOns\` paths reach the
/// folder, the component walk is case-insensitive (MSBT names its own files `Interface\Addons\…`
/// with a lowercase `d`, and ships `mailrays.TTF` while its table says `mailrays.ttf`), and a
/// dot-component is refused before any filesystem call.
fn read_font_bytes(source: &FontSource, path: &str) -> Option<Vec<u8>> {
    benilla_assets::read_chain_or_loose(&source.chain, source.loose_root.as_deref(), path)
}

/// A real-font engine for a test: the client faces, read through the app's own patch chain. `None`
/// when there is no install, or the chain or a face will not open — every caller **skips** rather
/// than fails (`wow_data_or_skip!`).
#[cfg(test)]
pub(super) fn test_engine(dpi: f32) -> Option<TextEngine> {
    let data = benilla_formats::wow_data()?;
    let chain = benilla_formats::open_chain(&data).ok()?;
    let mut font_system = client_font_system();
    let mut faces: Vec<Face> = Vec::new();
    let mut path_to_face = HashMap::new();
    for &path in CLIENT_FONTS {
        let Ok(bytes) = chain.read(path) else {
            continue;
        };
        let ascent_ratio = hhea_ascent_ratio(&bytes).unwrap_or(0.794);
        let r = register_font(&mut font_system, bytes).ok()?;
        path_to_face.insert(path.to_ascii_lowercase(), faces.len());
        faces.push(Face {
            id: r.id,
            path: path.to_string(),
            family: r.family,
            weight: r.weight,
            style: r.style,
            stretch: r.stretch,
            ascent_ratio,
        });
    }
    let default_face = *path_to_face.get(&CLIENT_FONTS[0].to_ascii_lowercase())?;
    Some(TextEngine {
        font_system,
        swash: SwashCache::new(),
        faces,
        path_to_face,
        // The same chain the four client faces were just read from, so a test can ask for a fifth.
        source: Some(FontSource {
            chain: Arc::new(Mutex::new(chain)),
            loose_root: None,
        }),
        missing_fonts: HashSet::new(),
        default_face,
        dpi,
        chars: HashMap::new(),
        cells: HashMap::new(),
        sheet: Sheet::new(&Assets::<Image>::default()),
        generation: 0,
        reset_pending: false,
        complained: HashSet::new(),
        substituted: HashSet::new(),
        over_ceiling: false,
        stats: CacheStats::default(),
    })
}

/// The client font paths, for tests that name a face.
#[cfg(test)]
pub(super) const TEST_FACES: &[&str] = CLIENT_FONTS;

#[cfg(test)]
mod differential_tests {
    use super::*;

    /// Real strings of the kinds that actually reach a measure: character names, item names,
    /// prose, the sequences a ligature table would target, the Latin-1 tail, and the digits the
    /// money frame sums.
    const CORPUS: &[&str] = &[
        "",
        " ",
        "Onewarrior",
        "Probezero",
        "Small Brown Pouch",
        "Staff of the Shadow Flame",
        "The affluent fiend's fickle offer",
        "fi fl ffi ffl ff",
        "AV Ta Wo Yo LT P, r. v.",
        "0123456789",
        "Grüße, Ärger; élan côté",
        "!@#$%^&*()_+-=[]{}|;':\",./<>?",
    ];

    /// **The character walk selects the glyphs whole-string shaping selects** — over the real
    /// client fonts.
    ///
    /// Both halves of this module's design rest on one property: a string's glyph sequence is the
    /// concatenation of its characters' glyph sequences. That is what lets
    /// [`TextEngine::ensure_char`] shape one character at a time — which in turn is what lets a
    /// measure be answered from inside a Lua call, and what lets the emit pass walk characters
    /// instead of running a shaper.
    ///
    /// It is a property of the FONTS, not of the code, and exactly the kind that would go silently
    /// wrong: a face with a ligature or a contextual substitution would break it and nothing else
    /// would notice. Skips without an install.
    #[test]
    fn a_character_walk_selects_what_the_whole_string_selects() {
        let Some(mut e) = test_engine(1.0) else {
            eprintln!("skipping: no install / patch chain");
            return;
        };
        let mut checked = 0usize;
        for dpi in [1.0f32, 2.0] {
            e.set_dpi_for_test(dpi);
            for face in 0..e.faces.len() {
                let family = e.faces[face].family.clone();
                // One small size and one large, so a size-dependent substitution could not hide
                // between them.
                for ppem in [10u16, 20] {
                    for text in CORPUS {
                        let (want, _) = shape_whole(&mut e, &family, ppem, text);
                        e.ensure_metrics(face, ppem, text);
                        let got: Vec<u16> = text
                            .chars()
                            .filter_map(|c| e.char_cell(face, ppem, c))
                            .flat_map(|c| c.glyphs.iter().map(|g| g.glyph_id))
                            .collect();
                        assert_eq!(
                            got, want,
                            "{family} @{ppem} dpi {dpi}: {text:?} — the character walk and the \
                             whole-string shaping must select the same glyphs"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(
            checked > 0,
            "no client font was readable — nothing was proven"
        );
    }

    /// **The width is the sum of unkerned per-character steps, and that is on purpose.**
    ///
    /// `measure_line_width` reads [`CharCell::floor_sum`], which is pre-summed at shaping time.
    /// This recomputes it from each glyph's own stored advance, so a mistake in the pre-summing or
    /// in the physical→logical divide fails here rather than on screen.
    ///
    /// The second half is the one that matters more: it asserts the answer **differs** from
    /// shaping the whole string, on a corpus of kerning pairs. cosmic-text's `Shaping::Advanced`
    /// kerns `AV`, `Ta`, `Wo`, `Yo`, `LT` and friends; the client's law does not
    /// (`ComputeStep 0x5ca2d0` applies only negative pair kerns and rounds, which we drop
    /// entirely — the module doc's stated v1 simplification). Without this assertion, someone
    /// "fixing" the measure to shape whole runs would make text quietly narrower than the
    /// reference and nothing would object.
    #[test]
    fn the_width_is_the_unkerned_per_character_sum() {
        let Some(mut e) = test_engine(1.0) else {
            eprintln!("skipping: no install / patch chain");
            return;
        };
        let mut kerning_seen = 0usize;
        for dpi in [1.0f32, 2.0] {
            e.set_dpi_for_test(dpi);
            for face in 0..e.faces.len() {
                let family = e.faces[face].family.clone();
                for ppem in [10u16, 20] {
                    // Both step biases: plain/NORMAL (+1) and THICK (+2).
                    for step_extra in [1.0f32, 2.0] {
                        for text in CORPUS {
                            let got = super::super::layout::measure_line_width_for_test(
                                &mut e, face, ppem, step_extra, text,
                            );
                            let by_glyph: f32 = text
                                .chars()
                                .filter_map(|c| e.char_cell(face, ppem, c))
                                .flat_map(|c| c.glyphs.iter())
                                .map(|g| g.advance.floor() + step_extra)
                                .sum::<f32>()
                                / dpi;
                            assert_eq!(
                                got, by_glyph,
                                "{family} @{ppem} dpi {dpi} extra {step_extra}: {text:?} — the \
                                 pre-summed floor must be the per-glyph sum"
                            );
                            let (_, kerned) = shape_whole(&mut e, &family, ppem, text);
                            let kerned =
                                kerned + step_extra * kerned_glyphs(&mut e, face, ppem, text) / dpi;
                            if (kerned - got).abs() > 1e-3 {
                                kerning_seen += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(
            kerning_seen > 0,
            "the corpus is full of kerning pairs — if the shaped sum never differs from ours, \
             either kerning stopped being dropped or this test stopped measuring it"
        );
    }

    /// Glyph count of `text` at `(face, ppem)` — the multiplier the step bias takes.
    fn kerned_glyphs(e: &mut TextEngine, face: usize, ppem: u16, text: &str) -> f32 {
        e.ensure_metrics(face, ppem, text);
        text.chars()
            .filter_map(|c| e.char_cell(face, ppem, c))
            .map(|c| c.glyphs.len() as f32)
            .sum()
    }

    /// The reference: shape the WHOLE string in one buffer. Returns its glyph ids and the sum of
    /// its **kerned** floored advances (no step bias — the caller adds it).
    fn shape_whole(e: &mut TextEngine, family: &str, ppem: u16, text: &str) -> (Vec<u16>, f32) {
        if text.is_empty() {
            return (Vec::new(), 0.0);
        }
        let attrs = Attrs::new().family(Family::Name(family));
        let px = f32::from(ppem);
        let mut buf = Buffer::new(&mut e.font_system, Metrics::new(px, px));
        buf.set_wrap(&mut e.font_system, Wrap::None);
        buf.set_text(&mut e.font_system, text, &attrs, Shaping::Advanced, None);
        buf.shape_until_scroll(&mut e.font_system, false);
        let (mut ids, mut w) = (Vec::new(), 0.0f32);
        for run in buf.layout_runs() {
            for g in run.glyphs {
                ids.push(g.glyph_id);
                w += g.w.floor() / e.dpi;
            }
        }
        (ids, w)
    }
}

#[cfg(test)]
mod ppem_tests {
    use super::*;

    fn engine_or_skip() -> Option<TextEngine> {
        match test_engine(1.0) {
            Some(e) => Some(e),
            None => {
                eprintln!("skipping: no client install / patch chain");
                None
            }
        }
    }

    /// **The size law, which is the whole decision.** A logical height becomes whole device pixels
    /// and nothing else; a request "between sizes" does not exist, because there is nothing to be
    /// between. Under the ladder, 12.48 snapped to 12 and the draw then stretched every finished
    /// quad by 1.04 about an anchor — which is what took the letters off their shared baseline.
    #[test]
    fn a_logical_height_becomes_whole_device_pixels() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        assert_eq!(e.ppem(12.0), 12, "an exact size is itself");
        assert_eq!(e.ppem(12.48), 12, "…and a fractional one rounds, not snaps");
        assert_eq!(e.ppem(12.5), 13);
        // The era-shaped windows' `SetScale(0.78)` — the case a fixed ladder could never carry,
        // because the Options window and the Game Menu are off it BY CONSTRUCTION.
        assert_eq!(e.ppem(16.0 * 0.78), 12);
        // Retina: the same logical height, twice the pixels, still an integer.
        e.dpi = 2.0;
        assert_eq!(e.ppem(12.0), 24);
        assert_eq!(e.ppem(16.0 * 0.78), 25);
        // The clamps.
        assert_eq!(e.ppem(0.0), MIN_PPEM);
        assert_eq!(e.ppem(-3.0), MIN_PPEM);
        assert_eq!(e.ppem(f32::NAN), MIN_PPEM);
        assert_eq!(e.ppem(10_000.0), MAX_PPEM);
    }

    /// The logical height a ppem draws at round-trips — which is what makes the pitch, the block
    /// height and every measured extent agree with the pixels on the screen rather than with the
    /// float that was asked for.
    #[test]
    fn the_drawn_logical_size_is_the_ppem_back_again() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        assert_eq!(e.drawn_size(12.0), 12.0);
        e.dpi = 2.0;
        assert_eq!(e.drawn_size(12.0), 12.0);
        // A request that does not land on a whole device pixel draws at the size it rounded to,
        // and says so — no second, different number anywhere downstream.
        assert_eq!(e.drawn_size(12.3), 12.5);
    }

    /// A character is shaped once and rasterized once per (size, radius); asking again is free, and
    /// asking for a second radius adds a cell without re-shaping.
    #[test]
    fn a_character_is_shaped_once_and_cached_per_size_and_radius() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        let face = e.face_for(None);
        e.ensure_str(face, 14, 0, "Ab");
        let (shaped, cells) = (e.stats.chars_shaped, e.stats.cells_rasterized);
        assert_eq!(shaped, 2, "two characters");
        assert_eq!(cells, 2, "two plain cells");

        e.ensure_str(face, 14, 0, "Ab");
        assert_eq!(e.stats.chars_shaped, shaped, "a repeat costs no shaping");
        assert_eq!(e.stats.cells_rasterized, cells, "…and no raster");

        // A second outline radius: new cells, no new shaping.
        e.ensure_str(face, 14, 1, "Ab");
        assert_eq!(
            e.stats.chars_shaped, shaped,
            "the pen metrics were already there"
        );
        assert_eq!(
            e.stats.cells_rasterized,
            cells + 2,
            "…but the ring is a new cell"
        );

        // A different size is a different raster, as it must be — that is the point.
        e.ensure_str(face, 15, 0, "Ab");
        assert_eq!(e.stats.chars_shaped, shaped + 2);

        // …and the cells are all really there, at both sizes.
        for ppem in [14u16, 15] {
            for ch in "Ab".chars() {
                let c = e.char_cell(face, ppem, ch).expect("shaped");
                for g in &c.glyphs {
                    assert!(e.cell(face, ppem, 0, g.glyph_id).is_some(), "{ch} @{ppem}");
                }
            }
        }
    }

    /// A space has real metrics and no cell — it must step the pen and draw nothing, and it must
    /// not be re-asked every frame.
    #[test]
    fn a_zero_ink_character_keeps_its_advance() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        let face = e.face_for(None);
        e.ensure_str(face, 14, 0, " ");
        let c = e.char_cell(face, 14, ' ').expect("a space still shapes");
        assert_eq!(c.glyphs.len(), 1);
        assert!(c.floor_sum > 0.0, "a space is wide");
        assert!(
            e.cell(face, 14, 0, c.glyphs[0].glyph_id).is_none(),
            "…and has no ink"
        );
        let raster = e.stats.cells_rasterized;
        e.ensure_str(face, 14, 0, " ");
        assert_eq!(e.stats.cells_rasterized, raster, "the miss is paid once");
    }

    /// **A character no face shapes is asked once too.** The sibling above pins that for a
    /// character that DOES shape; this one pins the miss, which was the leak: `complained`
    /// silenced the warning and nothing cached the result, so the shape re-ran on every ask. The
    /// pre-warm walks raw text (`ensure_str`), so a drawn multi-line FontString re-shaped every
    /// `\n` it held, every frame, for the life of the session.
    #[test]
    fn an_unshapeable_character_is_cached_as_a_miss() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        let face = e.face_for(None);
        let shaped = e.stats.chars_shaped;
        e.ensure_str(face, 14, 0, "\n");
        let c = e
            .char_cell(face, 14, '\n')
            .expect("the miss is cached, so the second ask short-circuits");
        // An empty cell is what every consumer already did with the `None` it used to get.
        assert!(c.glyphs.is_empty(), "a newline draws nothing");
        assert_eq!(c.floor_sum, 0.0, "…and steps nothing");
        assert_eq!(
            e.stats.chars_shaped, shaped,
            "a control character is answered before the shape, not by failing one"
        );
    }

    /// Each face resolves to itself, and an unknown path falls back to Friz.
    #[test]
    fn a_font_path_resolves_to_its_own_face() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        let friz = e.face_for(Some(TEST_FACES[0]));
        assert_eq!(friz, e.face_for(None), "Friz is the fallback");
        assert_eq!(friz, e.face_for(Some("Fonts\\NOSUCH.TTF")));
        assert_eq!(friz, e.face_for(Some("fonts\\frizqt__.ttf")), "case-folded");
        assert_ne!(friz, e.face_for(Some(TEST_FACES[1])), "ARIALN is its own");
    }

    /// Overwrite `OS/2.usWeightClass` (offset 4 in the table) in a raw sfnt, in place. `false` if
    /// the table directory has no `OS/2`. Test-only: it makes a *bold* face out of a face we
    /// already have, so the weight axis can be exercised without shipping a second TTF.
    fn set_weight_class(bytes: &mut [u8], weight: u16) -> bool {
        let Some(num) = bytes.get(4..6) else {
            return false;
        };
        let num = u16::from_be_bytes(num.try_into().unwrap()) as usize;
        for i in 0..num {
            let Some(rec) = bytes.get(12 + 16 * i..12 + 16 * i + 16) else {
                return false;
            };
            if &rec[0..4] != b"OS/2" {
                continue;
            }
            let off = u32::from_be_bytes(rec[8..12].try_into().unwrap()) as usize;
            let Some(slot) = bytes.get_mut(off + 4..off + 6) else {
                return false;
            };
            slot.copy_from_slice(&weight.to_be_bytes());
            return true;
        }
        false
    }
    /// **The font pool is the CLIENT's faces and nothing else**.
    ///
    /// `cosmic-text`'s two convenience constructors both call `load_system_fonts()`, and its
    /// shaper does not *select* a face — it orders every face in the database and takes the first
    /// that can draw the character. So one `FontSystem::new()` anywhere in this file puts every
    /// font on the developer's machine in front of the four the client actually has, and the only
    /// symptom is text in a face nobody asked for. The count is the pin: four faces registered,
    /// four faces in the database.
    #[test]
    fn the_font_pool_holds_only_the_clients_own_faces() {
        let Some(e) = engine_or_skip() else {
            return;
        };
        assert_eq!(
            e.font_system.db().len(),
            e.faces.len(),
            "the database must hold exactly the faces we registered — a system font in the pool \
             is a face the shaper can silently substitute"
        );
    }

    /// **A face the client names is the face that shapes — including a BOLD one**.
    ///
    /// The report was MSBT drawing in a plain sans instead of its own Porky. Porky declares
    /// `OS/2.usWeightClass = 700`; the attrs we handed the shaper were `Attrs::new()`, i.e. weight
    /// 400, so `fontdb::Database::query` never matched it and `get_font_matches`' weight-sorted
    /// walk put every normal-weight face ahead of it. The face loaded, the WARN never fired, the
    /// text drew — in the wrong face, at the wrong widths.
    ///
    /// The fixture is a real client TTF with its `usWeightClass` patched to 700, so the only thing
    /// that differs from the face beside it is the axis that broke.
    #[test]
    fn a_bold_addon_face_is_the_face_that_shapes() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        let mut bytes = {
            let source = e.source.as_ref().expect("the test engine carries a chain");
            let chain = source.chain.lock_recover();
            chain.read(TEST_FACES[2]).expect("MORPHEUS is in the chain")
        };
        assert!(
            set_weight_class(&mut bytes, 700),
            "patched OS/2 usWeightClass"
        );
        let root = std::env::temp_dir().join(format!("benilla-bold-font-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fonts = root.join("Bolded").join("Fonts");
        std::fs::create_dir_all(&fonts).unwrap();
        std::fs::write(fonts.join("heavy.ttf"), &bytes).unwrap();
        e.source.as_mut().unwrap().loose_root = Some(root.clone());

        let bold = e.face_for(Some("Interface\\AddOns\\Bolded\\Fonts\\heavy.ttf"));
        assert_ne!(bold, e.default_face, "the bold face registered");
        assert_eq!(
            e.faces[bold].weight,
            fontdb::Weight(700),
            "and fontdb read the patched weight — otherwise this test proves nothing"
        );
        e.ensure_metrics(bold, 18, "Rage");
        assert!(
            e.substituted.is_empty(),
            "the shaper answered with a different face for a face we named"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **An addon-shipped TTF loads out of the ONE AddOns root** — the leg the bug
    /// was: MSBT ships thirty-one faces and names them `Interface\\Addons\\…\\Fonts\\<x>.ttf`,
    /// a shape no MPQ carries, and every one of them silently drew as Friz Quadrata.
    ///
    /// The file is a real TTF (a client face, copied) because the assertion is that it REGISTERED,
    /// not merely that a path matched; the reference spelling deliberately mismatches the folder's
    /// case on every component, as the ecosystem's do (MSBT writes `Addons`, ships `mailrays.TTF`
    /// and asks for `mailrays.ttf`).
    #[test]
    fn an_addon_shipped_font_loads_out_of_the_addons_root() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        let bytes = {
            let source = e.source.as_ref().expect("the test engine carries a chain");
            let chain = source.chain.lock_recover();
            chain.read(TEST_FACES[2]).expect("MORPHEUS is in the chain")
        };
        let root = std::env::temp_dir().join(format!("benilla-addon-font-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fonts = root.join("MikScrollingBattleText").join("Fonts");
        std::fs::create_dir_all(&fonts).unwrap();
        std::fs::write(fonts.join("porky.TTF"), &bytes).unwrap();
        e.source.as_mut().unwrap().loose_root = Some(root.clone());

        let friz = e.face_for(Some(TEST_FACES[0]));
        let addon = e.face_for(Some(
            "Interface\\Addons\\mikscrollingbattletext\\fonts\\porky.ttf",
        ));
        assert_ne!(
            friz, addon,
            "an addon's own face must not fall back to Friz"
        );
        assert!(
            e.missing_fonts.is_empty(),
            "and it must not be recorded as a miss"
        );
        // A sibling path under the same root that is not there is still a miss — the folder is a
        // store, not a wildcard.
        assert_eq!(
            friz,
            e.face_for(Some(
                "Interface\\Addons\\MikScrollingBattleText\\Fonts\\nope.ttf"
            ))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **An addon-shipped face wears an OUTLINE like any other** — the two halves of the MSBT
    /// look, together, which nothing pinned. 2103 pins that the face LOADS;
    /// [`super::outline`]'s own tests pin the composite recipe on a synthetic bitmap; the cell
    /// arithmetic was only ever exercised on the fallback face. This runs a face read out of the
    /// AddOns root through the outline path and asserts the composite cell it produces: the same
    /// glyph, one cell per radius, each grown by `pad` on every side with its bearings moved out
    /// to match, and the plain cell untouched.
    #[test]
    fn an_addon_shipped_face_rasterizes_an_outlined_cell() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        let bytes = {
            let source = e.source.as_ref().expect("the test engine carries a chain");
            let chain = source.chain.lock_recover();
            chain.read(TEST_FACES[2]).expect("MORPHEUS is in the chain")
        };
        let root =
            std::env::temp_dir().join(format!("benilla-addon-outline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fonts = root.join("MikScrollingBattleText").join("Fonts");
        std::fs::create_dir_all(&fonts).unwrap();
        std::fs::write(fonts.join("porky.ttf"), &bytes).unwrap();
        e.source.as_mut().unwrap().loose_root = Some(root.clone());

        let face = e.face_for(Some(
            "Interface\\Addons\\MikScrollingBattleText\\Fonts\\porky.ttf",
        ));
        assert_ne!(face, e.face_for(None), "the addon's own face, not Friz");
        // MSBT's default master size, and its default flag — `SetFont(porky, 18, "OUTLINE")`
        // resolves to radius 1 (`super::outline::radius_of`).
        let ppem = e.ppem(18.0);
        e.ensure_str(face, ppem, 0, "6");
        e.ensure_str(face, ppem, 1, "6");
        let g = e.char_cell(face, ppem, '6').expect("shaped").glyphs[0].glyph_id;
        let plain = e.cell(face, ppem, 0, g).expect("a plain cell");
        let ringed = e.cell(face, ppem, 1, g).expect("an outlined cell");
        // `dpi` is 1 here, so NORMAL is one dilation pass: `pad` = 1 texel every side.
        assert_eq!(
            (ringed.px_w, ringed.px_h),
            (plain.px_w + 2.0, plain.px_h + 2.0),
            "the ring grows the cell by pad on every side"
        );
        assert_eq!(
            (ringed.bearing_x, ringed.bearing_top),
            (plain.bearing_x - 1.0, plain.bearing_top + 1.0),
            "…and the bearings move out with it, so the ink sits where it did"
        );
        // THICK is the second pass, and is a THIRD cell — not a re-use of either.
        e.ensure_str(face, ppem, 2, "6");
        let thick = e.cell(face, ppem, 2, g).expect("a THICK cell");
        assert_eq!(
            (thick.px_w, thick.px_h),
            (plain.px_w + 4.0, plain.px_h + 4.0)
        );
        assert_ne!(thick.uv, ringed.uv, "each radius packs its own cell");
        assert_ne!(plain.uv, ringed.uv);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A path that resolves nowhere falls back to Friz and is remembered, so the read and the WARN
    /// happen once rather than once per measure.
    #[test]
    fn an_unresolvable_font_path_is_remembered_as_missing() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        let friz = e.face_for(Some(TEST_FACES[0]));
        let bogus = "Interface\\Addons\\NoSuchAddon\\Fonts\\nope.ttf";
        assert_eq!(friz, e.face_for(Some(bogus)));
        assert!(e.missing_fonts.contains(&bogus.to_ascii_lowercase()));
        assert_eq!(friz, e.face_for(Some(bogus)), "and the second ask is free");
        // An empty path is the `SetFont("")` case and is not a miss — it is "no face named".
        assert_eq!(friz, e.face_for(Some("")));
        assert!(!e.missing_fonts.contains(""));
    }
}
