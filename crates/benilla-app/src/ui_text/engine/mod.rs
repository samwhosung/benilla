//! The font engine: an on-demand glyph cache keyed by exact integer device-pixel size. The
//! reference keeps a `CGxFont` per face, flags and size `min(32, round(H · max(reqSize, 2/H)))`
//! (`0x5ca030` → `[CGxFont+0x24c]`), rasterizes each glyph on first use (`NewCodeDesc`,
//! `0x5cabd0`) into up to 8 pages (`[CGxFont+0x18c]`; cell setter `0x5cf360`, free-slot search
//! `0x5cf5a0`) with no subpixel phase, and draws it 1:1, as `CSimpleFontString` sets the
//! one-to-one bit (`+0x120 & 0x200`, `0x770dd3`): a string lays out at its raster size.
//!
//! Deviation: text the reference magnifies from a smaller raster is rasterized at its true size
//! here: a `SetTextHeight` size (the bit cleared at `0x771600`), an em past 32 px, and 3-D unit
//! names (`UNIT_NAME_FONT`, em 32, string flag `0x80`), because the 32 px cap is a raster-memory
//! budget that blurs big text on a modern display ([`super::FONTSTRING_EM_CAP`]).
//!
//! Deviation: the reference flushes every glyph cache on a window resize (`0x5c2b50` →
//! `0x5ca6f0`); this keeps the old cells, because nameplate meshes bake UVs and a flush would
//! rebuild them every resize frame.
//!
//! Lock discipline: the render path and the script VM's measurer share this engine behind one lock,
//! so nothing may hold it across a call into the VM; a leaf entry point releases it on return.

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

/// The raster size floor in device pixels, the reference's `max(2, …)` (`0x5ca030`).
const MIN_PPEM: u16 = 2;

/// The raster size ceiling in device pixels, a backstop: reaching it is an upstream bug. The
/// reference's 32 px cap is applied in logical units, to UI text only ([`super::fontstring_em`]).
const MAX_PPEM: u16 = 256;

// ── The cache's own types ──

struct Face {
    id: fontdb::ID,
    path: String,
    family: String,
    /// The CSS axes the shaper matches on, read off the face ([`faces::Registered`]).
    weight: fontdb::Weight,
    style: fontdb::Style,
    stretch: fontdb::Stretch,
    /// `hhea.asc / (asc + |desc|)`; Friz's 0.794 when the tables do not parse.
    ascent_ratio: f32,
}

/// `(face, glyph, ppem, outline radius)`: one bitmap per exact device-pixel size.
type GlyphKey = (fontdb::ID, u16, u16, u8);

/// One rasterized cell, all in physical px; the layout divides by [`TextEngine::dpi`].
#[derive(Clone, Copy, Debug)]
pub(super) struct GlyphInfo {
    pub(super) uv: Rect,
    pub(super) px_w: f32,
    pub(super) px_h: f32,
    /// Swash convention: `left` rightward from the pen, `top` upward from the baseline.
    pub(super) bearing_x: f32,
    pub(super) bearing_top: f32,
}

/// One glyph a character shaped to: what the pen needs, with no bitmap.
#[derive(Clone, Copy, Debug)]
pub(super) struct GlyphRef {
    pub(super) glyph_id: u16,
    /// Swash's raster key at zero subpixel offset, so another outline radius needs no re-shaping.
    key: CacheKey,
    /// The face's advance in physical px, unfloored; [`super::layout::client_step`] floors it.
    pub(super) advance: f32,
    /// Offset from the baseline in physical px; 0 for every glyph the four client faces shape.
    pub(super) y_off: f32,
}

/// One character at one `(face, ppem)`. The step law's bias is per glyph, so a character shaping
/// to two glyphs would take two ([`super::layout::client_step`]); in the client faces none does.
pub(super) struct CharCell {
    pub(super) glyphs: Vec<GlyphRef>,
    /// Σ `advance.floor()` over the glyphs in physical px, the width law's per-character term.
    pub(super) floor_sum: f32,
}

/// What the cache instrument (`WOW_GLYPH_CACHE=1`) counts.
#[derive(Default, Clone, Copy)]
struct CacheStats {
    chars_shaped: u64,
    cells_rasterized: u64,
    resets: u64,
}

// ── The engine ──

/// The two stores a font path can come from: the patch chain, then the AddOns folder.
struct FontSource {
    chain: Arc<Mutex<benilla_formats::Chain>>,
    /// `ui_script::addons::root()`; `None` under `$WOW_CAPTURE`, which keeps a capture hermetic.
    loose_root: Option<std::path::PathBuf>,
}

/// The font engine: faces, the two caches, and the texture sheet.
pub(crate) struct TextEngine {
    font_system: FontSystem,
    swash: SwashCache,
    faces: Vec<Face>,
    /// Lowercased font path to face index: the four client faces, then any path loaded on demand.
    path_to_face: HashMap<String, usize>,
    /// Where an unmapped face is read from; `None` only in a VM with no install.
    source: Option<FontSource>,
    /// Paths that failed to load, read and warned once; a font file does not appear mid-session.
    missing_fonts: HashSet<String>,
    default_face: usize,
    /// The window's `scale_factor`. Every raster size derives from it ([`Self::ppem`]), so a
    /// change invalidates no cell, only the measures taken under the old value.
    dpi: f32,
    /// `(face, ppem, char)` to pen metrics; never touches the GPU, so the VM's measurer fills it.
    chars: HashMap<(usize, u16, char), CharCell>,
    /// `(face, glyph, ppem, radius)` to cell; `None` for no ink or no room, so a miss is paid once.
    cells: HashMap<GlyphKey, Option<GlyphInfo>>,
    sheet: Sheet,
    /// Bumped on a [reset][Sheet::reset], the only event that moves a UV, so it is what a
    /// cross-frame holder of glyph UVs ([`crate::nameplates`]) watches.
    generation: u64,
    /// Set when an allocation failed; acted on at the frame boundary, never mid-string.
    reset_pending: bool,
    complained: HashSet<char>,
    substituted: HashSet<usize>,
    over_ceiling: bool,
    stats: CacheStats,
}

/// A `cosmic-text` font system over an empty database: the client's faces are the only faces.
/// `FontSystem::new()` loads the system fonts, and `get_font_matches` shapes with the first face in
/// the database that can draw a character, so a system face would silently stand in for a client
/// face. A character no client or addon face carries draws nothing, as in the reference, and
/// measures 0. The locale only picks which language's family name a face answers to.
fn client_font_system() -> FontSystem {
    FontSystem::new_with_locale_and_db("en-US".to_string(), fontdb::Database::new())
}

impl TextEngine {
    /// Register the client TTFs off the patch chain; `None`, and no text, without Friz Quadrata.
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

    /// The face a font path resolves to, loading it on first use: `SetFont`, `<FontString font=>`
    /// and `CreateFont` take any path, an addon's own TTF included. A path that will not load
    /// warns once and falls back to Friz; the reference leaves the string on the font object it
    /// inherits when a `CGxFont` fails to build.
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

    /// The size law: a logical height becomes the integer device-pixel size it is rasterized,
    /// measured and drawn at, the reference's `round((height/768)·deviceH)` with the 768 seam
    /// already folded into `logical` by [`super::drawn_px`].
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

    /// The logical height a ppem draws at. Pitch, block height and every measured extent use it,
    /// not the requested height, so they match the rounded size on screen.
    pub(super) fn logical_size(&self, ppem: u16) -> f32 {
        f32::from(ppem) / self.dpi
    }

    /// Physical px per logical px, the divisor that brings cell metrics into logical space.
    pub(super) fn dpi(&self) -> f32 {
        self.dpi
    }

    /// The face's ascender fraction, the `[CGxFont+0x17c]` term each line's baseline seats on.
    pub(super) fn ascent_ratio_of(&self, face: usize) -> f32 {
        self.faces.get(face).map_or(0.794, |f| f.ascent_ratio)
    }

    /// The logical size a request actually draws at, for world-pass callers that normalize their
    /// own geometry against the size law ([`crate::nameplates`]).
    pub(crate) fn drawn_size(&mut self, logical: f32) -> f32 {
        let ppem = self.ppem(logical);
        self.logical_size(ppem)
    }

    /// [`Self::ascent_ratio_of`] by font path, for a caller that already holds the lock.
    pub(crate) fn ascent_ratio(&mut self, path: Option<&str>) -> f32 {
        let face = self.face_for(path);
        self.ascent_ratio_of(face)
    }

    /// Shape and rasterize whatever `text` lacks at this `(face, ppem, radius)`, so every lookup
    /// after it hits; the reference does this per character in its layout kernels (`0x5cabd0`).
    /// `AllGlyphsCached` (`0x5c9fa0`) is not this: it gates geometry invalidation after eviction.
    pub(super) fn ensure_str(&mut self, face: usize, ppem: u16, radius: u8, text: &str) {
        for ch in text.chars() {
            self.ensure_char(face, ppem, Some(radius), ch);
        }
    }

    /// [`Self::ensure_str`] without the raster: shape what is missing, so a string that is measured
    /// and never drawn never touches the sheet. Deviation: the reference's measure kernels
    /// (`0x5c6940`, `0x5c6b70`, `0x5c6c50`, `0x5c7300`, `0x5c7470`) rasterize each character, but
    /// the step law reads only the advance, so skipping the bitmap changes no width.
    pub(super) fn ensure_metrics(&mut self, face: usize, ppem: u16, text: &str) {
        for ch in text.chars() {
            self.ensure_char(face, ppem, None, ch);
        }
    }

    /// Set the DPI under a test. Otherwise only `gpu::publish_sheet` sets it, at the frame
    /// boundary: a mid-frame change would put two raster sizes in one laid-out string.
    #[cfg(test)]
    pub(super) fn set_dpi_for_test(&mut self, dpi: f32) {
        self.dpi = dpi;
    }

    /// One character: shape it alone into its [`CharCell`] and rasterize its glyphs at `radius`.
    /// Alone, so the width sum holds each face's own advance with no neighbour term (kerning is
    /// dropped, per the `ui_text` module doc) and one table answers a measure and a draw.
    fn ensure_char(&mut self, face: usize, ppem: u16, radius: Option<u8>, ch: char) {
        if let Some(known) = self.chars.get(&(face, ppem, ch)) {
            let Some(radius) = radius else { return };
            // The pen metrics are cached; the cells for this radius may not be.
            let glyphs = known.glyphs.clone();
            let Some(id) = self.faces.get(face).map(|f| f.id) else {
                return;
            };
            for g in &glyphs {
                self.ensure_cell(id, ppem, radius, g);
            }
            return;
        }
        // A control character (`\n`, which the markup parser takes as a line break) has no glyph
        // by design: an empty cell, before any shaping, so it neither re-shapes nor warns.
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
                    // A lone first glyph sits at x = y = 0: the zero-subpixel raster key.
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
        // Naming a face to `cosmic-text` is a query that takes the first face able to draw the
        // character, so an unmatched family draws and measures in another face: warned once.
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
            // No face shaped it: it draws and measures nothing, and warns once.
            if self.complained.insert(ch) {
                warn!("ui_text: no glyph for {ch:?} in any registered face");
            }
            // Cache the miss as an empty cell, or every frame's pre-warm shapes it again.
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

    /// Rasterize and pack one glyph's cell, unless it is already there.
    fn ensure_cell(&mut self, face_id: fontdb::ID, ppem: u16, radius: u8, g: &GlyphRef) {
        let key: GlyphKey = (face_id, g.glyph_id, ppem, radius);
        if self.cells.contains_key(&key) {
            return;
        }
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
        // An outlined cell grows by `pad` each side and its bearings move out; the advance stays.
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

    /// The sheet could not fit a cell: record it empty, so the glyph is missing for at most one
    /// frame, and ask for a reset at the frame boundary, not here, where glyphs already pushed this
    /// frame hold UVs into the sheet. Deviation: the reference evicts the least recently used glyph
    /// and repacks its hole (`0x5cad2b`, over `[CGxFont+0x64..0x6c]`); this drops every cell,
    /// because a shelf allocator cannot reclaim an interior cell and the sheet is far larger than
    /// the reference's 256×256 pages.
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

    /// One character's pen metrics, once ensured; a character no face shapes has no glyphs.
    pub(super) fn char_cell(&self, face: usize, ppem: u16, ch: char) -> Option<&CharCell> {
        self.chars.get(&(face, ppem, ch))
    }

    /// One glyph's cell; `None` for a zero-ink glyph, and for one frame after the sheet fills.
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

// ── The Bevy face of it ──

/// The font engine as the app holds it: one [`TextEngine`] behind one lock, shared with the
/// script VM's synchronous measurer ([`super::AtlasMeasurer`]), plus the per-region ellipsis memo.
#[derive(Resource)]
pub(crate) struct UiFontAtlas {
    engine: Arc<Mutex<TextEngine>>,
    /// Mirrored from the engine each frame, so the extract gate reads it without the lock.
    pub(crate) generation: u64,
    /// Per-region ellipsis display strings, the reference's `CGxString+0xf8` cache.
    pub(super) ellipsis: super::EllipsisMemo,
}

impl UiFontAtlas {
    /// Take the engine lock; release it before anything that can re-enter through the measurer.
    pub(crate) fn lock(&self) -> MutexGuard<'_, TextEngine> {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The shared engine, for the VM's measurer.
    pub(crate) fn engine(&self) -> Arc<Mutex<TextEngine>> {
        Arc::clone(&self.engine)
    }

    /// The one texture every glyph draws from, stable for the life of the process, for world-pass
    /// consumers ([`crate::nameplates`]) that bind glyph cells onto 3-D geometry.
    pub(crate) fn image(&self) -> Handle<Image> {
        self.lock().sheet_image()
    }
}

/// Read one font path by the rule every by-path addon asset follows, chain first
/// ([`benilla_assets::read_chain_or_loose`]).
fn read_font_bytes(source: &FontSource, path: &str) -> Option<Vec<u8>> {
    benilla_assets::read_chain_or_loose(&source.chain, source.loose_root.as_deref(), path)
}

/// A real-font engine for a test, the client faces off the patch chain; `None` without an install
/// or when the chain or a face will not open, and every caller then skips.
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

    /// Names, item names, prose, ligature and kerning bait, the Latin-1 tail and the money digits.
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

    /// In the client fonts a string's glyphs are its characters' glyphs concatenated, the property
    /// [`TextEngine::ensure_char`] rests on; a ligature or contextual substitution would break it.
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
                // A small and a large size, so a size-dependent substitution cannot hide.
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

    /// The width is the sum of per-glyph steps off each glyph's own advance, and differs from
    /// whole-string shaping, which kerns: the reference kerns only negative pairs, rounded up
    /// (`ComputeStep`, `0x5ca2d0`), and this drops even those, so shaping whole runs is no fix.
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

    /// Glyph count of `text` at `(face, ppem)`, which the step bias multiplies.
    fn kerned_glyphs(e: &mut TextEngine, face: usize, ppem: u16, text: &str) -> f32 {
        e.ensure_metrics(face, ppem, text);
        text.chars()
            .filter_map(|c| e.char_cell(face, ppem, c))
            .map(|c| c.glyphs.len() as f32)
            .sum()
    }

    /// The whole string shaped in one buffer: its glyph ids and kerned, floored advance sum.
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

    #[test]
    fn a_logical_height_becomes_whole_device_pixels() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        assert_eq!(e.ppem(12.0), 12, "an exact size is itself");
        assert_eq!(e.ppem(12.48), 12, "…and a fractional one rounds, not snaps");
        assert_eq!(e.ppem(12.5), 13);
        // `ERA_WINDOW_SCALE` (0.78), which the Options window and the Game Menu wear.
        assert_eq!(e.ppem(16.0 * 0.78), 12);
        e.dpi = 2.0;
        assert_eq!(e.ppem(12.0), 24);
        assert_eq!(e.ppem(16.0 * 0.78), 25);
        assert_eq!(e.ppem(0.0), MIN_PPEM);
        assert_eq!(e.ppem(-3.0), MIN_PPEM);
        assert_eq!(e.ppem(f32::NAN), MIN_PPEM);
        assert_eq!(e.ppem(10_000.0), MAX_PPEM);
    }

    #[test]
    fn the_drawn_logical_size_is_the_ppem_back_again() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        assert_eq!(e.drawn_size(12.0), 12.0);
        e.dpi = 2.0;
        assert_eq!(e.drawn_size(12.0), 12.0);
        // 12.3 at dpi 2 rounds to 25 device px, which draws at 12.5.
        assert_eq!(e.drawn_size(12.3), 12.5);
    }

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

        e.ensure_str(face, 15, 0, "Ab");
        assert_eq!(e.stats.chars_shaped, shaped + 2);

        for ppem in [14u16, 15] {
            for ch in "Ab".chars() {
                let c = e.char_cell(face, ppem, ch).expect("shaped");
                for g in &c.glyphs {
                    assert!(e.cell(face, ppem, 0, g.glyph_id).is_some(), "{ch} @{ppem}");
                }
            }
        }
    }

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

    /// A `\n` is cached as an empty cell before any shaping, so the pre-warm never shapes it.
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
        assert!(c.glyphs.is_empty(), "a newline draws nothing");
        assert_eq!(c.floor_sum, 0.0, "…and steps nothing");
        assert_eq!(
            e.stats.chars_shaped, shaped,
            "a control character is answered before the shape, not by failing one"
        );
    }

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

    /// Overwrite `OS/2.usWeightClass` (table offset 4) in a raw sfnt, making a bold face out of a
    /// client one; `false` without an `OS/2` table.
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

    /// A bold face named by path is the face that shapes, where default attrs (weight 400) would
    /// let a normal-weight face answer. The fixture is a client TTF patched to `usWeightClass` 700.
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

    /// An addon's TTF (a copied client face) loads from the AddOns root and registers, requested
    /// with a different case from the folder's on every component, as addons spell their paths.
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
        // A path under the same root that is not there is still a miss.
        assert_eq!(
            friz,
            e.face_for(Some(
                "Interface\\Addons\\MikScrollingBattleText\\Fonts\\nope.ttf"
            ))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A face read from the AddOns root takes the outline path: one cell per radius, each grown by
    /// `pad` on every side with its bearings moved out, and the plain cell untouched.
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
        // The addon's default, `SetFont(porky, 18, "OUTLINE")`: radius 1 (`outline::radius_of`).
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
        // THICK is two passes and a third cell.
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
        // An empty path, `SetFont("")`, names no face and is not a miss.
        assert_eq!(friz, e.face_for(Some("")));
        assert!(!e.missing_fonts.contains(""));
    }
}
