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
mod mono;
mod sfnt;
mod smooth;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use bevy::image::Image;
use bevy::math::Rect;
use bevy::prelude::*;
use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap,
};

use benilla_assets::WorldAssets;

use faces::{charmap_glyph, hhea_ascent_ratio, register_font, unicode_charmap, CLIENT_FONTS};
pub(crate) use gpu::{FontAtlasInit, UiTextPlugin};

use super::outline::{mono_cell, outlined_cell, Bake};
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
    /// The face's own tables, for FreeType's metrics ([`sfnt::Sfnt::glyph`]); `None` for a font
    /// with no TrueType outlines, which falls back to the shaper's.
    sfnt: Option<Arc<sfnt::Sfnt>>,
    /// The cmap encoding record characters map through ([`faces::unicode_charmap`]).
    charmap: u16,
}

/// `(face, glyph, ppem, bake)`: one bitmap per exact device-pixel size and bake.
type GlyphKey = (fontdb::ID, u16, u16, Bake);

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
    /// Swash's raster key at zero subpixel offset, so another bake needs no re-shaping.
    key: CacheKey,
    /// The advance in whole physical px, as the reference's FreeType rounds it
    /// ([`sfnt::GlyphPx::advance`]); the step law adds its bias ([`TextEngine::step`]).
    pub(super) px: f32,
    /// The glyph as its line's last, in physical px ([`sfnt::GlyphPx::ink`]).
    pub(super) ink: f32,
    /// Offset from the baseline in physical px; 0 for every glyph the four client faces shape.
    pub(super) y_off: f32,
}

/// One character at one `(face, ppem)`. The step law is per glyph ([`TextEngine::step`]), so a
/// character shaping to two glyphs would take two steps; in the client faces none does.
pub(super) struct CharCell {
    pub(super) glyphs: Vec<GlyphRef>,
}

/// What the cache instrument (`WOW_GLYPH_CACHE=1`) counts.
#[derive(Default, Clone, Copy)]
struct CacheStats {
    chars_shaped: u64,
    cells_rasterized: u64,
    resets: u64,
}

// ── The engine ──

/// The stores a font path can come from: the loose files, then the patch chain.
struct FontSource {
    chain: Arc<Mutex<benilla_formats::Chain>>,
    /// The install's index and `ui_script::addons::root()`, whose folder is `None` under
    /// `$WOW_CAPTURE`, which keeps a capture hermetic.
    loose: benilla_assets::LooseFiles,
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
    /// Paths that failed to load, read and warned once, until the install's index is emptied
    /// ([`Self::loose_generation`]).
    missing_fonts: HashSet<String>,
    /// The install index's generation [`Self::missing_fonts`] was filled under: a font added loose
    /// is found after the index is rebuilt (a `ReloadUI`, a logout), and not before. A face already
    /// loaded stays, as the client faces load once.
    loose_generation: u64,
    default_face: usize,
    /// The window's `scale_factor`. Every raster size derives from it ([`Self::ppem`]), so a
    /// change invalidates no cell, only the measures taken under the old value.
    dpi: f32,
    /// `(face, ppem, char)` to pen metrics; never touches the GPU, so the VM's measurer fills it.
    chars: HashMap<(usize, u16, char), CharCell>,
    /// `(face, glyph, ppem, bake)` to cell; `None` for no ink or no room, so a miss is paid once.
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
/// face. A character its own face's charmap lacks draws and measures nothing, as in the reference
/// ([`TextEngine::ensure_char`]). The locale only picks which language's family name a face
/// answers to.
fn client_font_system() -> FontSystem {
    FontSystem::new_with_locale_and_db("en-US".to_string(), fontdb::Database::new())
}

impl TextEngine {
    /// Register the client TTFs, each read as a font face opens ([`read_font_bytes`]); `None`, and
    /// no text, without Friz Quadrata.
    fn load(world_assets: &WorldAssets, images: &Assets<Image>, dpi: f32) -> Option<Self> {
        let mut font_system = client_font_system();
        let mut faces: Vec<Face> = Vec::new();
        let mut path_to_face = HashMap::new();
        let source = FontSource {
            chain: world_assets.chain.clone(),
            loose: benilla_assets::LooseFiles {
                addons: crate::ui_script::addons::root(),
                ..world_assets.loose().clone()
            },
        };
        for &path in CLIENT_FONTS {
            let Some(bytes) = read_font_bytes(&source, path) else {
                warn!("ui_text: failed to read {path} from the loose files or the patch chain");
                continue;
            };
            let Some(charmap) = unicode_charmap(&bytes) else {
                warn!("ui_text: {path} has no Unicode charmap, so it does not open");
                continue;
            };
            let ascent_ratio = hhea_ascent_ratio(&bytes).unwrap_or(0.794);
            let sfnt = sfnt::Sfnt::parse(bytes.clone()).map(Arc::new);
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
                        sfnt,
                        charmap,
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
            source: Some(source),
            missing_fonts: HashSet::new(),
            loose_generation: 0,
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
        let generation = self.source.as_ref().and_then(|s| s.loose.install.as_ref());
        if let Some(generation) = generation.map(|cell| cell.generation()) {
            if generation != self.loose_generation {
                self.loose_generation = generation;
                self.missing_fonts.clear();
            }
        }
        if self.missing_fonts.contains(&key) {
            return self.default_face;
        }
        match self.load_face(p, &key) {
            Some(i) => i,
            None => {
                warn!(
                    "ui_text: font miss: '{p}' does not load from the loose files or the patch \
                     chain — falling back to {}",
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
        let Some(charmap) = unicode_charmap(&bytes) else {
            warn!("ui_text: {path} has no Unicode charmap, so it does not open");
            return None;
        };
        let ascent_ratio = hhea_ascent_ratio(&bytes).unwrap_or(0.794);
        let sfnt = sfnt::Sfnt::parse(bytes.clone()).map(Arc::new);
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
            sfnt,
            charmap,
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

    /// Shape and rasterize whatever `text` lacks at this `(face, ppem, bake)`, so every lookup
    /// after it hits; the reference does this per character in its layout kernels (`0x5cabd0`).
    /// `AllGlyphsCached` (`0x5c9fa0`) is not this: it gates geometry invalidation after eviction.
    pub(super) fn ensure_str(&mut self, face: usize, ppem: u16, bake: Bake, text: &str) {
        for ch in text.chars() {
            self.ensure_char(face, ppem, Some(bake), ch);
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

    /// One character: shape it alone into its [`CharCell`] and rasterize its glyphs for `bake`.
    /// Alone, so a glyph's metrics hold no neighbour term (the pair kern is [`Self::step`]'s) and
    /// one table answers a measure and a draw.
    fn ensure_char(&mut self, face: usize, ppem: u16, bake: Option<Bake>, ch: char) {
        if let Some(known) = self.chars.get(&(face, ppem, ch)) {
            let Some(bake) = bake else { return };
            // The pen metrics are cached; the cells for this bake may not be.
            let glyphs = known.glyphs.clone();
            let Some(id) = self.faces.get(face).map(|f| f.id) else {
                return;
            };
            for g in &glyphs {
                self.ensure_cell(id, ppem, bake, g);
            }
            return;
        }
        // A control character (`\n`, which the markup parser takes as a line break) has no glyph
        // by design: an empty cell, before any shaping, so it neither re-shapes nor warns.
        if ch.is_control() {
            self.chars
                .insert((face, ppem, ch), CharCell { glyphs: Vec::new() });
            return;
        }
        let Some(f) = self.faces.get(face) else {
            return;
        };
        let (face_id, family, weight, style, stretch) =
            (f.id, f.family.clone(), f.weight, f.style, f.stretch);
        // The face's own charmap answers first, as `FT_Get_Char_Index` (`0x7ce960`) does ahead of
        // the load (`0x5d1154`): a code point it lacks fails the glyph (`0x5d1162`), so the lookup
        // `0x5cabd0` returns none and the line builder skips the character with no quad and no
        // step (`0x5cced6`). The shaper would draw the face's `.notdef` box in its place.
        let charmap = f.charmap;
        let mapped = self
            .font_system
            .db()
            .with_face_data(face_id, |data, index| {
                charmap_glyph(data, index, charmap, ch)
            })
            .flatten();
        if mapped.is_none() {
            if self.complained.insert(ch) {
                info!(
                    "ui_text: {} has no glyph for {ch:?}; it draws nothing",
                    self.faces.get(face).map_or("?", |f| f.path.as_str())
                );
            }
            self.chars
                .insert((face, ppem, ch), CharCell { glyphs: Vec::new() });
            return;
        }
        let attrs = Attrs::new()
            .family(Family::Name(&family))
            .weight(weight)
            .style(style)
            .stretch(stretch);
        let px = f32::from(ppem);
        let mut glyphs: Vec<GlyphRef> = Vec::new();
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
                        px: g.w.floor(),
                        ink: 0.0,
                        y_off: g.y,
                    });
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
            self.chars.insert((face, ppem, ch), CharCell { glyphs });
            return;
        }
        self.stats.chars_shaped += 1;
        // The metrics off the face's own tables, as FreeType computes them; a glyph another face
        // shaped keeps the shaper's advance and reads its ink off the outline.
        let sfnt = self.faces.get(face).and_then(|f| f.sfnt.clone());
        for g in &mut glyphs {
            let own = sfnt
                .as_deref()
                .filter(|_| g.key.font_id == face_id)
                .and_then(|t| t.glyph(g.glyph_id, ppem));
            match own {
                Some(m) => (g.px, g.ink) = (m.advance, m.ink),
                None => g.ink = glyph_ink(&mut self.swash, &mut self.font_system, g.key, ppem),
            }
        }
        if let Some(bake) = bake {
            for g in &glyphs {
                let g = *g;
                self.ensure_cell(face_id, ppem, bake, &g);
            }
        }
        self.chars.insert((face, ppem, ch), CharCell { glyphs });
    }

    /// The step from glyph `left` to the glyph after it in device px, `ComputeStep` (`0x5ca2d0`):
    /// `ceil(stepBase + min(kern, 0) · x_ppem/upem)`, the sum narrowed to f32 (`0x5ca378`) before
    /// the ceil (`0x73fdf5`). `stepBase` is the whole-pixel advance plus the law's bias
    /// (`step_extra`: `0x5d12a9`, and `0x5ca2b0` for THICK); the kern is the face's unscaled pair
    /// value (`FT_Get_Kerning` mode 2). With no glyph after it, the step is the base.
    pub(super) fn step(
        &self,
        face: usize,
        ppem: u16,
        step_extra: f32,
        left: &GlyphRef,
        right: Option<&GlyphRef>,
    ) -> f32 {
        let base = left.px + step_extra;
        let Some(right) = right else {
            return base;
        };
        let Some(f) = self.faces.get(face) else {
            return base;
        };
        let Some(t) = f.sfnt.as_deref() else {
            return base;
        };
        if left.key.font_id != f.id || right.key.font_id != f.id {
            return base;
        }
        let kern = t.neg_kern(left.glyph_id, right.glyph_id);
        if kern == 0 {
            return base;
        }
        let sum = f64::from(base) + f64::from(kern) * f64::from(t.kern_scale(ppem));
        (sum as f32).ceil()
    }

    /// Rasterize and pack one glyph's cell, unless it is already there. The raster is FreeType's
    /// unhinted one off the face's own outline, 1-bit for a monochrome bake ([`mono::rasterize`])
    /// and smooth otherwise ([`smooth::rasterize`]); a glyph another face shaped, whose tables are
    /// not held, takes the shaper's unhinted antialiased raster.
    ///
    /// The cell's left edge sits at `ceil(horiBearingX)` from the pen, the quad x of `0x5ccbe0`
    /// (`0x5cb080` mode 0, `0x5cd099`), not at the bitmap's `floor(xMin)`; the bakes put the ink
    /// `pad` columns in (`0x5cf237` plain, `0x5ce481`/`0x5cea7e` outlined, `0x5ce48a`/`0x5cea87` one more for THICK).
    /// Its top sits `pad` rows over FreeType's `bitmap_top`, which the reference's vertical seat
    /// (`0x5d1360`) hangs from the ascender, so the ink keeps the bitmap's own row.
    fn ensure_cell(&mut self, face_id: fontdb::ID, ppem: u16, bake: Bake, g: &GlyphRef) {
        let key: GlyphKey = (face_id, g.glyph_id, ppem, bake);
        if self.cells.contains_key(&key) {
            return;
        }
        let outline = self
            .faces
            .iter()
            .find(|f| f.id == g.key.font_id)
            .and_then(|f| f.sfnt.clone())
            .and_then(|t| t.outline(g.glyph_id, ppem));
        // `(cell, w, h, pad, bearing, top)`: the baked cell, its ring's reach, and the bitmap's
        // seat in whole px.
        let baked = match &outline {
            Some(o) if bake.mono => mono::rasterize(o, ppem).map(|m| {
                let (rgba, w, h, pad) = mono_cell(&m.bits, m.w, m.h, bake.radius, self.dpi);
                (OwnedCell::Rgba(rgba), w, h, pad, o.bearing(), m.top)
            }),
            Some(o) => smooth::rasterize(o).map(|m| {
                let (cell, w, h, pad) = self.coverage_cell(m.coverage, m.w, m.h, bake);
                (cell, w, h, pad, o.bearing(), m.top)
            }),
            None => self.shaper_raster(g.key).map(|(cov, p, bearing)| {
                let (cell, w, h, pad) = self.coverage_cell(cov, p.width, p.height, bake);
                (cell, w, h, pad, Some(bearing), p.top)
            }),
        };
        let Some((cell, w, h, pad, Some(bearing), top)) = baked else {
            // Whitespace / zero-ink glyph: real metrics, no cell.
            self.cells.insert(key, None);
            return;
        };
        self.stats.cells_rasterized += 1;
        let uv = self.sheet.alloc(w, h, &cell.as_cell());
        self.place(key, uv, w, h, bearing as f32, (top + pad as i32) as f32);
    }

    /// A coverage bitmap baked for `bake`: as it is when plain, ringed when outlined, the cell then
    /// grown by `pad` texels each side. `(cell, w, h, pad)`.
    fn coverage_cell(
        &self,
        cov: Vec<u8>,
        w: u32,
        h: u32,
        bake: Bake,
    ) -> (OwnedCell, u32, u32, u32) {
        if bake.radius == 0 {
            return (OwnedCell::Coverage(cov), w, h, 0);
        }
        let (rgba, ow, oh, pad) = outlined_cell(&cov, w, h, bake.radius, self.dpi);
        (OwnedCell::Rgba(rgba), ow, oh, pad)
    }

    /// The shaper's unhinted antialiased raster of a glyph, its placement and `ceil` of its
    /// outline's `xMin`, for a glyph whose face's tables are not held; `None` with no ink.
    fn shaper_raster(&mut self, key: CacheKey) -> Option<(Vec<u8>, cosmic_text::Placement, i32)> {
        let key = CacheKey {
            flags: key.flags | CacheKeyFlags::DISABLE_HINTING,
            ..key
        };
        let Self {
            swash, font_system, ..
        } = self;
        let (x0, ..) = outline_box(swash, font_system, key)?;
        let image = swash.get_image_uncached(font_system, key)?;
        let p = image.placement;
        (p.width > 0 && p.height > 0).then(|| (image.data, p, x0.ceil() as i32))
    }

    /// Record a packed cell, or its absence when the sheet had no room.
    fn place(&mut self, key: GlyphKey, uv: Option<Rect>, w: u32, h: u32, bx: f32, bt: f32) {
        match uv {
            Some(uv) => {
                self.cells.insert(
                    key,
                    Some(GlyphInfo {
                        uv,
                        px_w: w as f32,
                        px_h: h as f32,
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
        bake: Bake,
        glyph_id: u16,
    ) -> Option<GlyphInfo> {
        let id = self.faces.get(face)?.id;
        self.cells
            .get(&(id, glyph_id, ppem, bake))
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

    /// The DPI the engine lays out at, which follows the window's at the frame boundary
    /// (`gpu::publish_sheet`): a holder of laid-out glyphs re-lays them when it moves.
    pub(crate) fn dpi(&self) -> f32 {
        self.lock().dpi
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

/// Read one font path as a font face opens it (`0x5d0060`, flags 3 at `0x5d0121`): a loose file
/// by its bare name, then by its whole path, then the chain
/// ([`benilla_assets::read_loose_or_chain`]).
fn read_font_bytes(source: &FontSource, path: &str) -> Option<Vec<u8>> {
    benilla_assets::read_loose_or_chain(
        &source.chain,
        &source.loose,
        path,
        benilla_assets::LooseOpen::BareNameFirst,
    )
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
        let charmap = unicode_charmap(&bytes)?;
        let ascent_ratio = hhea_ascent_ratio(&bytes).unwrap_or(0.794);
        let sfnt = sfnt::Sfnt::parse(bytes.clone()).map(Arc::new);
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
            sfnt,
            charmap,
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
            loose: benilla_assets::LooseFiles::default(),
        }),
        missing_fonts: HashSet::new(),
        loose_generation: 0,
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
impl UiFontAtlas {
    /// A glyph-sheet texel's alpha as this frame queued it ([`Sheet::pending_alpha`]).
    pub(crate) fn pending_alpha_for_test(&self, x: u32, y: u32) -> Option<u8> {
        self.lock().sheet.pending_alpha(x, y)
    }

    /// [`test_engine`] as the app holds it, for a host test that draws text; `None` as there.
    pub(crate) fn for_test(dpi: f32) -> Option<Self> {
        Some(UiFontAtlas {
            engine: Arc::new(Mutex::new(test_engine(dpi)?)),
            generation: 0,
            ellipsis: crate::ui_text::EllipsisMemo::default(),
        })
    }
}

/// A glyph's ink as its line's last, in physical px, read off the scaled outline: the fallback for
/// a glyph another face shaped, whose tables [`sfnt::Sfnt::glyph`] does not hold. The reference
/// measures a line's last glyph by `ceil(horiBearingX)` plus `out[4]`, the bitmap width plus the
/// outline's growth (`0x5c6b70` → `0x5cb080`, `0x5c6c24`), both stored by the rasterizer
/// (`0x5d1120`: `out[6]` the bearing, `out[4] = out[2] + growth`); the growth is the caller's.
/// FreeType's bitmap spans the outline's control box rounded out, `ceil(xMax) − floor(xMin)`, its
/// bearing is `xMin`, both on the 26.6 grid; a glyph with no ink gets a blank `(ppem + 3) >> 2` px
/// wide (`0x5d11da`) and no bearing. The outline is unhinted, as the reference loads every glyph
/// (`0x5d1300`: both load-flag words carry `FT_LOAD_NO_HINTING`).
fn glyph_ink(swash: &mut SwashCache, fs: &mut FontSystem, key: CacheKey, ppem: u16) -> f32 {
    let blank = f32::from((ppem + 3) >> 2);
    let Some((x0, x1, y0, y1)) = outline_box(swash, fs, key) else {
        return blank;
    };
    let (width, rows) = (x1.ceil() - x0.floor(), y1.ceil() - y0.floor());
    if !(width > 0.0 && rows > 0.0) {
        return blank;
    }
    x0.ceil() + width
}

/// The unhinted outline's control box `(xMin, xMax, yMin, yMax)` in px, on FreeType's 26.6 grid so
/// an edge a float hair past a whole pixel does not round out; `None` with no outline.
fn outline_box(
    swash: &mut SwashCache,
    fs: &mut FontSystem,
    key: CacheKey,
) -> Option<(f32, f32, f32, f32)> {
    let key = CacheKey {
        flags: key.flags | CacheKeyFlags::DISABLE_HINTING,
        ..key
    };
    let commands = swash.get_outline_commands_uncached(fs, key)?;
    let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for c in commands.iter() {
        let pts = match *c {
            cosmic_text::Command::MoveTo(p) | cosmic_text::Command::LineTo(p) => {
                [Some(p), None, None]
            }
            cosmic_text::Command::QuadTo(a, p) => [Some(a), Some(p), None],
            cosmic_text::Command::CurveTo(a, b, p) => [Some(a), Some(b), Some(p)],
            cosmic_text::Command::Close => [None, None, None],
        };
        for p in pts.into_iter().flatten() {
            (x0, x1) = (x0.min(p.x), x1.max(p.x));
            (y0, y1) = (y0.min(p.y), y1.max(p.y));
        }
    }
    let q = |v: f32| (v * 64.0).round() / 64.0;
    (x0 <= x1).then(|| (q(x0), q(x1), q(y0), q(y1)))
}

/// A baked cell's texels, owned until the sheet copies them ([`Cell`]).
enum OwnedCell {
    Coverage(Vec<u8>),
    Rgba(Vec<u8>),
}

impl OwnedCell {
    fn as_cell(&self) -> Cell<'_> {
        match self {
            OwnedCell::Coverage(c) => Cell::Coverage(c),
            OwnedCell::Rgba(c) => Cell::Rgba(c),
        }
    }
}

#[cfg(test)]
mod line_law_tests {
    use super::*;
    use crate::ui_text::{string_width, FontSpec};
    use benilla_ui::script::Outline;

    const FRIZ: &str = "Fonts\\FRIZQT__.TTF";

    /// `GetStringWidth`'s measure (`0x5c6940`) in device px, computed outside this code base off
    /// FRIZQT__.TTF's own tables with FreeType 2.1's fixed point (`FT_MulFix` advances and outline,
    /// `>> 6`, the control box rounded out) and the kernel's law (kerned `ceil` steps, the last
    /// glyph at `ceil(bearing) + width`, the outline's lead and growth, the shadow's seed, the
    /// carry across a break): `(text, ppem, outline, shadow px, width px)`.
    const GOLDEN: &[(&str, u16, Outline, f32, f32)] = &[
        ("Gold", 12, Outline::None, 0.0, 31.0),
        ("Gold", 12, Outline::None, 1.0, 32.0),
        ("1", 12, Outline::None, 0.0, 7.0),
        ("1", 12, Outline::None, 1.0, 8.0),
        ("1234", 12, Outline::None, 0.0, 32.0),
        ("1234", 12, Outline::None, 1.0, 33.0),
        // `1`→`2` kerns −86 units, −1.03 px: the step is 7, not 8.
        ("12", 12, Outline::None, 0.0, 16.0),
        ("Ironforge", 12, Outline::None, 0.0, 59.0),
        // `f` advances 4.0 px in 26.6, 3.996 to a float shaper.
        ("fi.", 12, Outline::None, 0.0, 13.0),
        ("fff", 12, Outline::None, 0.0, 16.0),
        ("Staff of the Shadow Flame", 12, Outline::None, 0.0, 164.0),
        ("Stormwind City", 12, Outline::None, 0.0, 98.0),
        ("i\nIronforge", 12, Outline::None, 0.0, 63.0),
        ("ab\ncd", 12, Outline::None, 0.0, 24.0),
        ("1234", 12, Outline::Normal, 0.0, 36.0),
        ("1234", 12, Outline::Thick, 0.0, 43.0),
        ("Gold", 14, Outline::Normal, 0.0, 39.0),
        ("11", 14, Outline::None, 0.0, 15.0),
        ("1234", 17, Outline::None, 1.0, 45.0),
        ("Ironforge", 17, Outline::None, 0.0, 79.0),
    ];

    /// The measure answers the reference's widths to the pixel, at both DPIs.
    #[test]
    fn the_measure_answers_the_reference_widths() {
        benilla_formats::wow_data_or_skip!();
        let mut e = test_engine(1.0).expect("the client faces open");
        for dpi in [1.0f32, 2.0] {
            e.set_dpi_for_test(dpi);
            for &(text, ppem, outline, shadow, want) in GOLDEN {
                let spec = FontSpec {
                    path: Some(FRIZ),
                    height: Some(f32::from(ppem) / dpi),
                    outline,
                    monochrome: false,
                    spacing: 0.0,
                    shadow_x: 0.0,
                    alpha_gradient: None,
                    non_space_wrap: false,
                    max_lines: 0,
                };
                let got = string_width(&mut e, text, spec, shadow / dpi);
                assert_eq!(
                    got * dpi,
                    want,
                    "{text:?} @{ppem} {outline:?} shadow {shadow}, dpi {dpi}"
                );
            }
        }
    }

    /// A code point its face's charmap lacks draws nothing and takes no room (`0x7ce960` → glyph
    /// 0 → `0x5cced6`), where the shaper would draw `.notdef`. Friz, the bubbles' and plates' face,
    /// has no Cyrillic, so `1Ж2` is `12`, kerned pair and all; Arial Narrow, the chat frame's,
    /// carries `Ж` and draws it.
    #[test]
    fn a_code_point_the_face_lacks_draws_nothing_and_takes_no_room() {
        benilla_formats::wow_data_or_skip!();
        let mut e = test_engine(1.0).expect("the client faces open");
        let (friz, arial) = (
            e.face_for(Some(FRIZ)),
            e.face_for(Some("Fonts\\ARIALN.TTF")),
        );
        for ch in ['Ж', 'я', '中'] {
            e.ensure_str(friz, 12, Bake::ring(0), &ch.to_string());
            let cell = e.char_cell(friz, 12, ch).expect("the miss is cached");
            assert!(
                cell.glyphs.is_empty(),
                "Friz has no {ch:?}: no glyph, no step"
            );
        }
        e.ensure_str(arial, 12, Bake::ring(0), "Ж");
        let g = e.char_cell(arial, 12, 'Ж').expect("shaped").glyphs[0];
        assert!(
            g.glyph_id != 0 && g.px > 0.0,
            "ARIALN maps Ж to a real glyph"
        );
        assert!(
            e.cell(arial, 12, Bake::ring(0), g.glyph_id).is_some(),
            "…and draws it"
        );
        let spec = |path| FontSpec {
            path: Some(path),
            height: Some(12.0),
            outline: Outline::None,
            monochrome: false,
            spacing: 0.0,
            shadow_x: 0.0,
            alpha_gradient: None,
            non_space_wrap: false,
            max_lines: 0,
        };
        assert_eq!(string_width(&mut e, "1Ж2", spec(FRIZ), 0.0), 16.0);
        assert_eq!(string_width(&mut e, "Ж", spec(FRIZ), 0.0), 0.0);
    }

    /// A composite glyph's ink as the reference's FreeType builds it (`0x7d40e6`–`0x7d4246`): its
    /// components scaled first, each offset scaled alone and, under `ROUND_XY_TO_GRID`, rounded to
    /// a whole pixel. Every composite in ARIALN carries that flag; these ink widths, a lone
    /// character's measure, were computed off the font's tables outside this code base.
    #[test]
    fn a_composite_glyph_is_built_as_the_reference_builds_it() {
        benilla_formats::wow_data_or_skip!();
        let mut e = test_engine(1.0).expect("the client faces open");
        for &(text, ppem, want) in &[
            ("ì", 12u16, 4.0f32),
            ("î", 12, 5.0),
            ("Î", 12, 5.0),
            ("í", 13, 5.0),
            ("Ï", 13, 5.0),
            ("½", 16, 13.0),
            ("¾", 16, 13.0),
            ("í", 18, 6.0),
            ("ì", 18, 5.0),
            ("½", 18, 13.0),
            ("¼", 18, 13.0),
        ] {
            let spec = FontSpec {
                path: Some("Fonts\\ARIALN.TTF"),
                height: Some(f32::from(ppem)),
                outline: Outline::None,
                monochrome: false,
                spacing: 0.0,
                shadow_x: 0.0,
                alpha_gradient: None,
                non_space_wrap: false,
                max_lines: 0,
            };
            assert_eq!(
                string_width(&mut e, text, spec, 0.0),
                want,
                "{text:?} @{ppem}"
            );
        }
    }

    /// The step's advance is FreeType's 26.6 one shifted down (`0x5d1298`), not a float's floor.
    #[test]
    fn an_advance_is_the_fixed_point_one() {
        benilla_formats::wow_data_or_skip!();
        let mut e = test_engine(1.0).expect("the client faces open");
        let face = e.face_for(Some(FRIZ));
        e.ensure_metrics(face, 12, "f");
        let g = e.char_cell(face, 12, 'f').expect("shaped").glyphs[0];
        assert_eq!(g.px, 4.0, "`f` at 12 px advances 4.0 in 26.6");
        assert_eq!(e.step(face, 12, 1.0, &g, None), 5.0);
    }
}

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
                        let want = shape_whole(&mut e, &family, ppem, text);
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

    /// The whole string shaped in one buffer: its glyph ids.
    fn shape_whole(e: &mut TextEngine, family: &str, ppem: u16, text: &str) -> Vec<u16> {
        if text.is_empty() {
            return Vec::new();
        }
        let attrs = Attrs::new().family(Family::Name(family));
        let px = f32::from(ppem);
        let mut buf = Buffer::new(&mut e.font_system, Metrics::new(px, px));
        buf.set_wrap(&mut e.font_system, Wrap::None);
        buf.set_text(&mut e.font_system, text, &attrs, Shaping::Advanced, None);
        buf.shape_until_scroll(&mut e.font_system, false);
        buf.layout_runs()
            .flat_map(|run| run.glyphs.iter().map(|g| g.glyph_id))
            .collect()
    }
}

#[cfg(test)]
mod ppem_tests {
    use super::*;
    use benilla_assets::LockRecover;

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
        // `BENILLA_ERA_WINDOW_SCALE` (0.78), which the Options window and the Game Menu wear.
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
        e.ensure_str(face, 14, Bake::ring(0), "Ab");
        let (shaped, cells) = (e.stats.chars_shaped, e.stats.cells_rasterized);
        assert_eq!(shaped, 2, "two characters");
        assert_eq!(cells, 2, "two plain cells");

        e.ensure_str(face, 14, Bake::ring(0), "Ab");
        assert_eq!(e.stats.chars_shaped, shaped, "a repeat costs no shaping");
        assert_eq!(e.stats.cells_rasterized, cells, "…and no raster");

        e.ensure_str(face, 14, Bake::ring(1), "Ab");
        assert_eq!(
            e.stats.chars_shaped, shaped,
            "the pen metrics were already there"
        );
        assert_eq!(
            e.stats.cells_rasterized,
            cells + 2,
            "…but the ring is a new cell"
        );

        e.ensure_str(face, 15, Bake::ring(0), "Ab");
        assert_eq!(e.stats.chars_shaped, shaped + 2);

        for ppem in [14u16, 15] {
            for ch in "Ab".chars() {
                let c = e.char_cell(face, ppem, ch).expect("shaped");
                for g in &c.glyphs {
                    assert!(
                        e.cell(face, ppem, Bake::ring(0), g.glyph_id).is_some(),
                        "{ch} @{ppem}"
                    );
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
        e.ensure_str(face, 14, Bake::ring(0), " ");
        let c = e.char_cell(face, 14, ' ').expect("a space still shapes");
        assert_eq!(c.glyphs.len(), 1);
        assert!(c.glyphs[0].px > 0.0, "a space is wide");
        assert!(
            e.cell(face, 14, Bake::ring(0), c.glyphs[0].glyph_id)
                .is_none(),
            "…and has no ink"
        );
        let raster = e.stats.cells_rasterized;
        e.ensure_str(face, 14, Bake::ring(0), " ");
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
        e.ensure_str(face, 14, Bake::ring(0), "\n");
        let c = e
            .char_cell(face, 14, '\n')
            .expect("the miss is cached, so the second ask short-circuits");
        assert!(c.glyphs.is_empty(), "a newline draws nothing");
        assert!(c.glyphs.is_empty(), "…and steps nothing");
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
        e.source.as_mut().unwrap().loose.addons = Some(root.clone());

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

    /// A font face opens as `0x5d0060` does, with flags 3: a TTF loose at the top of the install
    /// answers any path ending in its name, ahead of the path itself and the chain.
    #[test]
    fn a_font_loose_in_the_install_answers_by_its_bare_name() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        let bytes = {
            let source = e.source.as_ref().expect("the test engine carries a chain");
            let chain = source.chain.lock_recover();
            chain.read(TEST_FACES[2]).expect("MORPHEUS is in the chain")
        };
        let install =
            std::env::temp_dir().join(format!("benilla-install-font-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&install);
        std::fs::create_dir_all(install.join("Data")).unwrap();
        std::fs::write(install.join("Data").join("Custom.TTF"), &bytes).unwrap();
        let root = install.clone();
        let cell = benilla_formats::LooseIndexCell::new(move || {
            benilla_formats::LooseIndex::build(Some(&root), &root.join("Data"))
        });
        e.source.as_mut().unwrap().loose.install = Some(Arc::new(cell));

        let face = e.face_for(Some("Interface\\Nowhere\\custom.ttf"));
        assert_ne!(face, e.face_for(None), "the loose face, not Friz");
        assert!(e.missing_fonts.is_empty(), "and not recorded as a miss");
        let _ = std::fs::remove_dir_all(&install);
    }

    /// A font added loose after a miss stays a miss until the install's index is emptied, then
    /// loads: the remembered miss goes with the index it was asked of.
    #[test]
    fn a_loose_font_added_mid_session_loads_after_the_index_is_emptied() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        let bytes = {
            let source = e.source.as_ref().expect("the test engine carries a chain");
            let chain = source.chain.lock_recover();
            chain.read(TEST_FACES[2]).expect("MORPHEUS is in the chain")
        };
        let install =
            std::env::temp_dir().join(format!("benilla-install-late-font-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&install);
        std::fs::create_dir_all(install.join("Data")).unwrap();
        let root = install.clone();
        let cell = Arc::new(benilla_formats::LooseIndexCell::new(move || {
            benilla_formats::LooseIndex::build(Some(&root), &root.join("Data"))
        }));
        e.source.as_mut().unwrap().loose.install = Some(cell.clone());

        let path = "Fonts\\Late.ttf";
        assert_eq!(e.face_for(Some(path)), e.face_for(None), "not there yet");
        std::fs::write(install.join("Data").join("Late.ttf"), &bytes).unwrap();
        assert_eq!(
            e.face_for(Some(path)),
            e.face_for(None),
            "the built index stays"
        );
        cell.clear();
        assert_ne!(
            e.face_for(Some(path)),
            e.face_for(None),
            "found after the rebuild"
        );
        let _ = std::fs::remove_dir_all(&install);
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
        e.source.as_mut().unwrap().loose.addons = Some(root.clone());

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
    /// `pad` on every side, its left edge kept at `ceil(horiBearingX)` and its top raised by `pad`.
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
        e.source.as_mut().unwrap().loose.addons = Some(root.clone());

        let face = e.face_for(Some(
            "Interface\\Addons\\MikScrollingBattleText\\Fonts\\porky.ttf",
        ));
        assert_ne!(face, e.face_for(None), "the addon's own face, not Friz");
        // The addon's default, `SetFont(porky, 18, "OUTLINE")`: radius 1 (`outline::Bake::of`).
        let ppem = e.ppem(18.0);
        e.ensure_str(face, ppem, Bake::ring(0), "6");
        e.ensure_str(face, ppem, Bake::ring(1), "6");
        let g = e.char_cell(face, ppem, '6').expect("shaped").glyphs[0].glyph_id;
        let plain = e.cell(face, ppem, Bake::ring(0), g).expect("a plain cell");
        let ringed = e
            .cell(face, ppem, Bake::ring(1), g)
            .expect("an outlined cell");
        // `dpi` is 1 here, so NORMAL is one dilation pass: `pad` = 1 texel every side.
        assert_eq!(
            (ringed.px_w, ringed.px_h),
            (plain.px_w + 2.0, plain.px_h + 2.0),
            "the ring grows the cell by pad on every side"
        );
        // The quad x is the pen plus `ceil(horiBearingX)` for every bake (`0x5cb080`), and the
        // bake puts the ink one column in (`0x5cea7e`); its top row stays the bitmap's.
        assert_eq!(
            (ringed.bearing_x, ringed.bearing_top),
            (plain.bearing_x, plain.bearing_top + 1.0),
            "…its left edge stays and its top rises by the ring"
        );
        // THICK is two passes and a third cell.
        e.ensure_str(face, ppem, Bake::ring(2), "6");
        let thick = e.cell(face, ppem, Bake::ring(2), g).expect("a THICK cell");
        assert_eq!(
            (thick.px_w, thick.px_h, thick.bearing_x, thick.bearing_top),
            (
                plain.px_w + 4.0,
                plain.px_h + 4.0,
                plain.bearing_x,
                plain.bearing_top + 2.0
            )
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

    /// One glyph's cell at `(face, ppem, bake)` and its texels, RGBA rows from the top, read back
    /// off the upload the raster queued.
    fn cell_texels(
        e: &mut TextEngine,
        face: usize,
        ppem: u16,
        bake: Bake,
        ch: char,
    ) -> (GlyphInfo, Vec<[u8; 4]>) {
        let _ = e.sheet.take_pending();
        e.ensure_str(face, ppem, bake, &ch.to_string());
        let gid = e.char_cell(face, ppem, ch).expect("shaped").glyphs[0].glyph_id;
        let info = e.cell(face, ppem, bake, gid).expect("a cell");
        let (_, ups) = e.sheet.take_pending();
        let up = ups.last().expect("the cell's upload");
        let side = crate::ui_text::pack::SHEET_SIZE as f32;
        let (cx, cy) = (
            (info.uv.min.x * side).round() as u32 - up.x,
            (info.uv.min.y * side).round() as u32 - up.y,
        );
        let (w, h) = (info.px_w as u32, info.px_h as u32);
        let texels = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let i = (((cy + y) * up.w + cx + x) * 4) as usize;
                [up.rgba[i], up.rgba[i + 1], up.rgba[i + 2], up.rgba[i + 3]]
            })
            .collect();
        (info, texels)
    }

    /// An antialiased cell is the unhinted raster (`0x5d1300`: both load-flag words carry
    /// `FT_LOAD_NO_HINTING`, rendered in mode 0 at `0x7cec20`). Arial Narrow's `e` at 18 px, the
    /// chat frame's, as FreeType's smooth rasterizer draws it unhinted, computed outside this code
    /// base: the bar spreads over two partial rows, where a hinter snaps it to one full row.
    #[test]
    fn an_antialiased_cell_is_the_unhinted_raster() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        let face = e.face_for(Some("Fonts\\ARIALN.TTF"));
        let golden = [
            "000021777e2e0000",
            "0042f6e4dafc5b00",
            "05e4a6040081f211",
            "44ff1c000006f35b",
            "6efb90909090ed83",
            "78f6bcbcbcbcbc68",
            "68f5010000000000",
            "33ff41000010e853",
            "00c7de442ab6ee13",
            "001cc9ffffe34000",
            "0000002429040000",
        ];
        let (info, texels) = cell_texels(&mut e, face, 18, Bake::ring(0), 'e');
        assert_eq!(
            (info.px_w, info.px_h, info.bearing_top),
            (8.0, 11.0, 10.0),
            "FreeType's bitmap: the control box rounded out, `bitmap_top` 10"
        );
        let got: Vec<String> = texels
            .chunks(8)
            .map(|row| row.iter().map(|t| format!("{:02x}", t[3])).collect())
            .collect();
        assert_eq!(got, golden);
    }

    /// A cell's left edge is the pen plus `ceil(horiBearingX)`, the quad x of `0x5ccbe0` (`0x5cb080`
    /// mode 0, `0x5cd099`), with no padding taken off, and its bake puts the ink 0, 1 or 2 columns
    /// in for plain, NORMAL and THICK (`0x5cf237`; `0x5ce481`/`0x5cea7e`, `0x5ce48a`/`0x5cea87`), monochrome
    /// or not. Arial Narrow at 14 px: `a`'s bearing is 25/64 px and `1`'s 79/64 (FreeType,
    /// unhinted), so their bitmaps' `floor(xMin)` columns 0 and 1 land at 1 and 2.
    #[test]
    fn a_cell_sits_at_its_rounded_up_bearing_with_its_ink_the_bake_column_in() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        let face = e.face_for(Some("Fonts\\ARIALN.TTF"));
        for (ch, bearing) in [('a', 1.0f32), ('1', 2.0)] {
            for (radius, mono) in [(0u8, false), (1, false), (2, false), (0, true), (2, true)] {
                let bake = Bake { radius, mono };
                let (info, texels) = cell_texels(&mut e, face, 14, bake, ch);
                assert_eq!(
                    info.bearing_x, bearing,
                    "{ch:?} {bake:?}: the cell's left edge"
                );
                // The ink is the fill, white; a ring is black.
                let w = info.px_w as usize;
                let ink = (0..w)
                    .find(|&x| {
                        texels
                            .iter()
                            .skip(x)
                            .step_by(w)
                            .any(|t| t[3] > 0 && t[0] > 0)
                    })
                    .expect("ink");
                assert_eq!(
                    ink,
                    usize::from(radius),
                    "{ch:?} {bake:?}: the ink's column"
                );
            }
        }
        // Through the layout: a left-justified `a` at x 10 draws its cell at 11.
        let spec = crate::ui_text::FontSpec {
            path: Some("Fonts\\ARIALN.TTF"),
            height: Some(14.0),
            outline: benilla_ui::script::Outline::None,
            monochrome: false,
            spacing: 0.0,
            shadow_x: 0.0,
            alpha_gradient: None,
            non_space_wrap: false,
            max_lines: 0,
        };
        let quads = crate::ui_text::layout_text_quads(
            &mut e,
            "a",
            Rect::new(10.0, 0.0, 210.0, 20.0),
            [1.0; 4],
            crate::ui_text::Justify {
                h: benilla_ui::script::JustifyH::Left,
                v: benilla_ui::script::JustifyV::Top,
            },
            0,
            spec,
            crate::ui_text::TextSeat::UiGrid,
        );
        assert_eq!(quads[0].rect.min.x, 11.0);
    }

    /// The action bar's hotkey numeral, `NumberFontNormalSmallGray` (Arial Narrow, monochrome,
    /// THICK) at 15 px: its cell holds the 1-bit glyph, an opaque ring one texel round the
    /// strokes and the second ring at 7/15 beyond, so the `2`'s counters show through rather than
    /// filling one opaque box.
    #[test]
    fn a_monochrome_thick_numeral_rings_its_strokes_not_a_box() {
        let Some(mut e) = engine_or_skip() else {
            return;
        };
        let face = e.face_for(Some("Fonts\\ARIALN.TTF"));
        let bake = Bake {
            radius: 2,
            mono: true,
        };
        let _ = e.sheet.take_pending();
        e.ensure_str(face, 15, bake, "2");
        let gid = e.char_cell(face, 15, '2').expect("shaped").glyphs[0].glyph_id;
        let info = e.cell(face, 15, bake, gid).expect("a cell");
        let (_, ups) = e.sheet.take_pending();
        let up = &ups[0];
        let side = crate::ui_text::pack::SHEET_SIZE as f32;
        let (cx, cy) = (
            (info.uv.min.x * side).round() as u32 - up.x,
            (info.uv.min.y * side).round() as u32 - up.y,
        );
        let (w, h) = (info.px_w as u32, info.px_h as u32);
        let alpha = |x: u32, y: u32| up.rgba[(((cy + y) * up.w + cx + x) * 4 + 3) as usize];

        // The glyph and its 8-neighbours, off the same 1-bit raster.
        let sfnt = e.faces[face].sfnt.clone().expect("Arial Narrow's tables");
        let g = mono::rasterize(&sfnt.outline(gid, 15).expect("outline"), 15).expect("ink");
        assert_eq!(
            (w, h),
            (g.w + 4, g.h + 4),
            "THICK grows the cell two texels a side"
        );
        let inked = |x: i64, y: i64| {
            (0..i64::from(g.w)).contains(&x)
                && (0..i64::from(g.h)).contains(&y)
                && g.bits[(y * i64::from(g.w) + x) as usize] != 0
        };
        for y in 0..h {
            for x in 0..w {
                let (gx, gy) = (i64::from(x) - 2, i64::from(y) - 2);
                let near = (-1..=1).any(|dy| (-1..=1).any(|dx| inked(gx + dx, gy + dy)));
                let a = alpha(x, y);
                if near {
                    assert_eq!(a, 255, "glyph or first ring at {x},{y}");
                } else {
                    assert!(a == 0 || a == 119, "{a} past the first ring at {x},{y}");
                }
            }
        }
    }
}
