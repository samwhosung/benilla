//! The **art extent** of a glue scene — how far its art actually paints around its authored
//! camera, measured off the geometry and the textures.
//!
//! Every `UI_<Race>` / `UI_MainMenu` diorama was composed for a 4:3 screen (1587: the Lua design
//! space is `1024×768`), and its backdrop is *finite*: a sky card of some authored width, a ground
//! plane that stops. A camera law that widens the view on a wide window (1587's hor+) keeps showing
//! more of the diorama until the art runs out — and past that edge the frame is the render target's
//! clear colour, which no taste calls a composition (`UI_MainMenu`'s backdrop edges show at
//! 16:9, `UI_Tauren`'s at ~2.24:1). The reference never shows that void only because it never
//! widens; it zooms instead (0116/1543's diagonal-FOV law) and crops the character at 21:9.
//!
//! So the framing law needs one more authored fact per scene: **how wide is the art**. This module
//! measures it. In camera 0's own tan-space — `x' = x/z`, `y' = y/z` in a right-handed eye frame,
//! the space a projection's half-extents live in — it rasterises every batch that paints into a
//! coverage grid and asks:
//!
//! - across the authored **vertical** opening (`±tan(fovy/2)` at 4:3), what horizontal half-width
//!   does *every* row reach, left and right of the axis? → [`ArtExtent::half_w`]
//! - across the authored **horizontal** opening, what vertical half-height does every column
//!   reach? → [`ArtExtent::half_h`]
//!
//! The minimum over rows is the point: a sky card seen in perspective is a trapezoid on screen, and
//! the frame runs out of it at the *narrow* row first (MainMenu's edges slant inward toward the
//! top).
//!
//! **What counts as painted is the texel, not the batch.** An `Opaque` batch paints every pixel it
//! covers. A `Blend`/`AlphaTest` batch paints where its texture's alpha would pass the reference's
//! own alpha test — **`≥ 224/255`**, the 1.12 alpha-key reference (`0x70c256`, not Cata's 128) —
//! sampled per pixel through the batch's UVs, perspective-correct, with its wrap/clamp mode.
//! Both halves of that rule matter on the real art: the artists drew `UI_Human`'s street and its
//! sky card as *blend* batches over opaque textures (so "opaque batches only" measured that scene
//! at half its authored box), and `UI_NightElf`'s edges are an alpha-tested wall of trees over a
//! sky card that ends at exactly 4:3 (so "opaque textures only" measured it a full step narrower
//! than what is on screen). A cloud, a canopy, a ground shadow, `UI_Tauren`'s 0.55 corner vignette
//! paint only where their texels pass, which is nowhere at their soft edges — the void this
//! measures for. `Mod`/`Mod2x` only tint what is already drawn. Backfaces of single-sided batches
//! are skipped the way the renderer skips them (`Face::Back` cull, CCW front — the model's own
//! winding survives the axis remap, a proper rotation), so a ground plane seen from below does not
//! count as sky.
//!
//! Because the rule reads textures, the measurement runs **offline**, off the chain: `benilla-
//! extract glueextent` prints every scene's numbers, and the client carries them as
//! [`shipped_glue_art_extent`] — a transcription of the shipped 1.12.1 art's authored width,
//! pinned by [`tests::the_shipped_table_matches_the_measurement`], which re-measures every scene
//! against the table. (A runtime probe at scene spawn would have to wait for every batch texture
//! to land before it could answer, and until it answered the login gate would open with its void
//! edges showing, then snap.)
//!
//! Positions are the M2's own model space (WoW axes, Z up) and so is the camera — nothing here
//! needs the engine's frame. The consumer ([`benilla`]'s glue framing) turns the two numbers into a
//! ceiling on 1587's law.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use super::records::M2PortraitCamera;
use super::types::{ModelBlend, RenderSubmesh};
use crate::Chain;

/// How far a scene's art paints around its authored camera, as projection-space half-extents
/// (tan units — the same space `tan(fov/2)` lives in).
///
/// `half_w` is measured across the authored 4:3 **vertical** opening, `half_h` across the authored
/// **horizontal** one: each is the widest symmetric box of *that* shape the art still fills. A scene
/// whose art does not even fill the authored box (an axis row with no painted pixel over it)
/// reports `0.0` on that axis — the consumer clamps to the authored extent, never below it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArtExtent {
    pub half_w: f32,
    pub half_h: f32,
}

/// The authored aspect every glue composition was made for (1587, `1024×768`).
pub const GLUE_AUTHORED_ASPECT: f32 = 4.0 / 3.0;

/// The 1.12 client's alpha-key reference: a texel passes its alpha test at `alpha ≥ 224`
/// (`0x70c256` — `ALPHAREF` 224, `GEQUAL`). The one threshold this module paints by.
pub const ALPHA_KEY_REF: u8 = 224;

/// The **shipped** scenes' measured extents and authored fovs — `benilla-extract glueextent` on
/// the 1.12.1 (5875) chain, transcribed. Keyed by the scene token (`UI_<token>.m2`). The test
/// re-measures them.
///
/// Read as aspects (`half_w / t0`, the window aspect past which the art runs out of width under
/// the authored vertical): MainMenu 1.54 · Human 1.47 · Orc 1.57 · Dwarf 1.42 · NightElf 1.31 ·
/// Scourge 1.57 · Tauren 1.46 — every one of the seven runs out **before 16:9**. Not one diorama
/// was drawn wider than about 3:2; the reference never showed it because its diagonal law zooms
/// from 4:3 onward.
///
/// `UI_NightElf` is the odd one on both axes, and honestly so: its sky card is exactly 4:3 wide,
/// and its `half_h` is *inside* the authored box — the gaps between its tree-cutout leaves at the
/// frame's sides paint nothing even at 4:3. Both are the artist's intent: it is a night scene, and
/// what shows through is the page behind the scene, near-black, read as sky. The framing law never
/// opens past the authored box on an axis the art does not reach, so the number costs nothing
/// there.
/// One degree in radians — the unit the `UI_*` cameras were dialled in. Every one of the seven
/// authored fovs is a whole number of degrees, to the last bit of the `f32` the M2 carries
/// (`benilla-extract glueextent`: 86°, 80°, 65°, 60°), so the table says so rather than
/// transcribing decimals.
const DEG: f32 = std::f32::consts::PI / 180.0;

/// One shipped glue scene, as the framing law needs it: the scene token, its authored camera-0
/// **fov** ([`DEG`] — 86° on the gate, 80° on the human street, 65° on the four orc-family
/// stages, 60° in the night elf grove), and the measured
/// [`ArtExtent`]. The fov rides the table because the framing constant is derived from it
/// ([`benilla`]'s `GLUE_BOX_ASPECT`) and both halves are re-measured by the same chain test.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShippedGlueScene {
    pub token: &'static str,
    pub fov: f32,
    pub art: ArtExtent,
}

/// The seven shipped scenes: the login gate plus the six race stages (Gnome shares Dwarf's,
/// Troll shares Orc's). See [`ShippedGlueScene`].
pub const SHIPPED_GLUE_SCENES: [ShippedGlueScene; 7] = [
    ShippedGlueScene {
        token: "MainMenu",
        fov: 86.0 * DEG,
        art: ArtExtent {
            half_w: 0.7431,
            half_h: 0.5067,
        },
    },
    ShippedGlueScene {
        token: "Human",
        fov: 80.0 * DEG,
        art: ArtExtent {
            half_w: 0.6548,
            half_h: 0.4866,
        },
    },
    ShippedGlueScene {
        token: "Orc",
        fov: 65.0 * DEG,
        art: ArtExtent {
            half_w: 0.5573,
            half_h: 0.3954,
        },
    },
    ShippedGlueScene {
        token: "Dwarf",
        fov: 65.0 * DEG,
        art: ArtExtent {
            half_w: 0.5040,
            half_h: 0.4272,
        },
    },
    ShippedGlueScene {
        token: "NightElf",
        fov: 60.0 * DEG,
        art: ArtExtent {
            half_w: 0.4262,
            half_h: 0.1221,
        },
    },
    ShippedGlueScene {
        token: "Scourge",
        fov: 65.0 * DEG,
        art: ArtExtent {
            half_w: 0.5542,
            half_h: 0.4776,
        },
    },
    ShippedGlueScene {
        token: "Tauren",
        fov: 65.0 * DEG,
        art: ArtExtent {
            half_w: 0.5177,
            half_h: 0.4066,
        },
    },
];

/// The shipped scene's measured [`ArtExtent`] by token, `None` for a token that is not one of
/// the seven `UI_*` dioramas (the consumer then keeps 1587's unbounded law).
pub fn shipped_glue_art_extent(token: &str) -> Option<ArtExtent> {
    SHIPPED_GLUE_SCENES
        .iter()
        .find(|s| s.token == token)
        .map(|s| s.art)
}

/// The authored **vertical** half-extent (tan units) of a glue camera's `fov` at 4:3 — the number
/// 1587's law pins: `tan(fovy/2)` with `fovy = fov/√((4/3)²+1)`.
pub fn authored_half_height(fov: f32) -> f32 {
    (fov / (GLUE_AUTHORED_ASPECT * GLUE_AUTHORED_ASPECT + 1.0).sqrt() * 0.5).tan()
}

/// How a batch paints the pixels it covers.
#[derive(Clone, Debug)]
pub enum Coverage {
    /// Every covered pixel is painted (an opaque batch, or a blend over a texture with no alpha
    /// below the key).
    Full,
    /// Painted where the texture's alpha, sampled through the batch's UVs, is `≥` [`ALPHA_KEY_REF`].
    Alpha(Arc<AlphaMap>),
}

/// A texture's alpha channel, for [`Coverage::Alpha`].
#[derive(Clone, Debug)]
pub struct AlphaMap {
    pub width: u32,
    pub height: u32,
    /// Row-major, one byte per texel.
    pub alpha: Vec<u8>,
}

impl AlphaMap {
    /// The alpha at `(u, v)` under the batch's wrap (`true`) or clamp-to-edge (`false`) modes,
    /// nearest texel — the same texel the sampler's mip 0 would return at its centre.
    fn sample(&self, u: f32, v: f32, wrap_x: bool, wrap_y: bool) -> u8 {
        let axis = |t: f32, n: u32, wrap: bool| -> u32 {
            let t = if wrap {
                t - t.floor()
            } else {
                t.clamp(0.0, 1.0)
            };
            ((t * n as f32) as u32).min(n - 1)
        };
        if self.width == 0 || self.height == 0 {
            return 0;
        }
        let x = axis(u, self.width, wrap_x);
        let y = axis(v, self.height, wrap_y);
        self.alpha[(y * self.width + x) as usize]
    }
}

/// The coverage rule with the chain in hand: `Opaque` → [`Coverage::Full`]; `Blend`/`AlphaTest`
/// → the texture's alpha, read once per path (a texture that never dips below the key collapses
/// to `Full`); no texture, or `Mod`/`Mod2x` → `None` (paints nothing that counts).
pub struct CoverageReader<'c> {
    chain: &'c mut Chain,
    cache: HashMap<String, Option<Coverage>>,
}

impl<'c> CoverageReader<'c> {
    pub fn new(chain: &'c mut Chain) -> Self {
        Self {
            chain,
            cache: HashMap::new(),
        }
    }

    /// How `sub` paints, or `None` when it counts for nothing.
    pub fn coverage(&mut self, sub: &RenderSubmesh) -> Result<Option<Coverage>> {
        match sub.blend {
            ModelBlend::Opaque => Ok(Some(Coverage::Full)),
            ModelBlend::Mod | ModelBlend::Mod2x => Ok(None),
            ModelBlend::AlphaTest | ModelBlend::Blend => {
                let Some(path) = sub.texture.as_deref() else {
                    return Ok(None);
                };
                if let Some(known) = self.cache.get(path) {
                    return Ok(known.clone());
                }
                let (width, height, rgba) = crate::read_texture_rgba(self.chain, path)?;
                let alpha: Vec<u8> = rgba.as_chunks::<4>().0.iter().map(|px| px[3]).collect();
                let cov = if alpha.iter().all(|&a| a >= ALPHA_KEY_REF) {
                    Some(Coverage::Full)
                } else if alpha.iter().all(|&a| a < ALPHA_KEY_REF) {
                    None
                } else {
                    Some(Coverage::Alpha(Arc::new(AlphaMap {
                        width,
                        height,
                        alpha,
                    })))
                };
                self.cache.insert(path.to_string(), cov.clone());
                Ok(cov)
            }
        }
    }
}

/// Measure a scene's [`ArtExtent`]: its batches, its authored camera, and how each batch paints
/// (`coverage` — [`CoverageReader::coverage`] with the chain, or anything a test hands in).
///
/// Front faces only unless two-sided; triangles are clipped to the camera's own near plane,
/// exactly as the render would clip them.
pub fn glue_art_extent<'a>(
    subs: impl IntoIterator<Item = &'a RenderSubmesh>,
    cam: &M2PortraitCamera,
    mut coverage: impl FnMut(&RenderSubmesh) -> Option<Coverage>,
) -> ArtExtent {
    let t0 = authored_half_height(cam.fov);
    let h0 = t0 * GLUE_AUTHORED_ASPECT;
    let mut grid = Grid::new(t0);
    if let Some(frame) = EyeFrame::of(cam) {
        for s in subs {
            if let Some(cov) = coverage(s) {
                grid.paint_batch(s, &frame, cam.near_clip.max(1e-3), &cov);
            }
        }
    }
    ArtExtent {
        half_w: grid.half_extent(Axis::Row, t0),
        half_h: grid.half_extent(Axis::Column, h0),
    }
}

/// The coverage grid over the camera's tan-space: `x' ∈ ±X_SPAN·t0`, `y' ∈ ±Y_SPAN·t0`. The spans
/// cover what the framing law can ask for — a wide window holds the width at the art's edge, and
/// the widest edge any diorama has is under `2·t0`; a narrow window opens the height up to the
/// art's, and `2.7·t0` is a 1:2 window.
struct Grid {
    cells: Vec<bool>,
    /// Tan units per cell, both axes.
    dx: f32,
    dy: f32,
    x_lo: f32,
    y_lo: f32,
}

/// Cells per axis. Even, so the axis (`x' = 0`, `y' = 0`) is a cell boundary and the run around
/// it is read symmetrically. 2048 cells over `4.4·t0` is `0.0021·t0` per cell — 1.5 px at 1440p —
/// and the reported extent is shortened by half a cell, so the quantisation only ever
/// under-reports.
const CELLS: usize = 2048;
const X_SPAN: f32 = 2.2;
const Y_SPAN: f32 = 2.7;

impl Grid {
    fn new(t0: f32) -> Self {
        let x_lo = -X_SPAN * t0;
        let y_lo = -Y_SPAN * t0;
        Self {
            cells: vec![false; CELLS * CELLS],
            dx: (2.0 * X_SPAN * t0) / CELLS as f32,
            dy: (2.0 * Y_SPAN * t0) / CELLS as f32,
            x_lo,
            y_lo,
        }
    }

    /// Rasterise every front-facing, near-clipped triangle of `sub` into the grid.
    fn paint_batch(&mut self, sub: &RenderSubmesh, frame: &EyeFrame, near: f32, cov: &Coverage) {
        for tri in sub.indices.as_chunks::<3>().0 {
            let Some(eye) = tri
                .iter()
                .map(|&i| {
                    let p = sub.positions.get(i as usize)?;
                    let uv = sub.uvs.get(i as usize).copied().unwrap_or([0.0; 2]);
                    Some((frame.to_eye(*p), uv))
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            for piece in clip_near(&eye, near) {
                let proj: [Vert; 3] = piece.map(|((x, y, z), uv)| Vert {
                    x: x / z,
                    y: y / z,
                    inv_z: 1.0 / z,
                    u_z: uv[0] / z,
                    v_z: uv[1] / z,
                });
                let area = signed_area(&proj);
                // The renderer culls back faces of single-sided batches: CCW on screen is front.
                if !sub.two_sided && area <= 0.0 {
                    continue;
                }
                if area == 0.0 {
                    continue;
                }
                self.paint_triangle(&proj, area, sub, cov);
            }
        }
    }

    fn paint_triangle(&mut self, t: &[Vert; 3], area: f32, sub: &RenderSubmesh, cov: &Coverage) {
        let (x_min, x_max) = t
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                (lo.min(v.x), hi.max(v.x))
            });
        let (y_min, y_max) = t
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                (lo.min(v.y), hi.max(v.y))
            });
        let col_of = |x: f32| ((x - self.x_lo) / self.dx).floor();
        let row_of = |y: f32| ((y - self.y_lo) / self.dy).floor();
        let c0 = col_of(x_min).max(0.0) as usize;
        let c1 = (col_of(x_max) as isize).min(CELLS as isize - 1);
        let r0 = row_of(y_min).max(0.0) as usize;
        let r1 = (row_of(y_max) as isize).min(CELLS as isize - 1);
        if c1 < 0 || r1 < 0 || c0 > c1 as usize || r0 > r1 as usize {
            return;
        }
        let inv_area = 1.0 / area;
        for r in r0..=r1 as usize {
            let y = self.y_lo + (r as f32 + 0.5) * self.dy;
            for c in c0..=c1 as usize {
                let x = self.x_lo + (c as f32 + 0.5) * self.dx;
                // Barycentrics from the edge functions (all same sign as `area` ⇒ inside).
                let w0 = edge(&t[1], &t[2], x, y) * inv_area;
                let w1 = edge(&t[2], &t[0], x, y) * inv_area;
                let w2 = edge(&t[0], &t[1], x, y) * inv_area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let painted = match cov {
                    Coverage::Full => true,
                    Coverage::Alpha(map) => {
                        // Perspective-correct UV: interpolate u/z, v/z, 1/z, divide.
                        let inv_z = w0 * t[0].inv_z + w1 * t[1].inv_z + w2 * t[2].inv_z;
                        let u = (w0 * t[0].u_z + w1 * t[1].u_z + w2 * t[2].u_z) / inv_z;
                        let v = (w0 * t[0].v_z + w1 * t[1].v_z + w2 * t[2].v_z) / inv_z;
                        map.sample(u, v, sub.wrap_x, sub.wrap_y) >= ALPHA_KEY_REF
                    }
                };
                if painted {
                    self.cells[r * CELLS + c] = true;
                }
            }
        }
    }

    /// The largest half-extent along `axis` that every scanline across the authored opening
    /// (`±opening` on the other axis) reaches on both sides of the axis. `0.0` if any scanline
    /// leaves the axis unpainted. Shortened by half a cell, so quantisation under-reports.
    fn half_extent(&self, axis: Axis, opening: f32) -> f32 {
        let (lines, cells_per_line, step, lo, line_step) = match axis {
            Axis::Row => (CELLS, CELLS, self.dx, self.y_lo, self.dy),
            Axis::Column => (CELLS, CELLS, self.dy, self.x_lo, self.dx),
        };
        let mut extent = f32::INFINITY;
        let mut any = false;
        for line in 0..lines {
            let pos = lo + (line as f32 + 0.5) * line_step;
            if pos < -opening || pos > opening {
                continue;
            }
            any = true;
            let at = |i: usize| match axis {
                Axis::Row => self.cells[line * CELLS + i],
                Axis::Column => self.cells[i * CELLS + line],
            };
            let mid = cells_per_line / 2; // the axis sits between `mid - 1` and `mid`
            let mut right = 0usize;
            while mid + right < cells_per_line && at(mid + right) {
                right += 1;
            }
            let mut left = 0usize;
            while left < mid && at(mid - 1 - left) {
                left += 1;
            }
            if left == 0 || right == 0 {
                return 0.0;
            }
            extent = extent.min(left.min(right) as f32 * step - step * 0.5);
        }
        if any && extent.is_finite() {
            extent.max(0.0)
        } else {
            0.0
        }
    }
}

#[derive(Clone, Copy)]
enum Axis {
    /// Scan horizontal rows; the extent is along `x'`.
    Row,
    /// Scan vertical columns; the extent is along `y'`.
    Column,
}

/// A projected vertex with what perspective-correct interpolation needs.
#[derive(Clone, Copy)]
struct Vert {
    x: f32,
    y: f32,
    inv_z: f32,
    u_z: f32,
    v_z: f32,
}

/// The edge function of `a → b` at `(x, y)`: twice the signed area of `(a, b, p)`.
fn edge(a: &Vert, b: &Vert, x: f32, y: f32) -> f32 {
    (b.x - a.x) * (y - a.y) - (b.y - a.y) * (x - a.x)
}

/// Twice the signed area of a projected triangle — positive for counter-clockwise.
fn signed_area(t: &[Vert; 3]) -> f32 {
    edge(&t[0], &t[1], t[2].x, t[2].y)
}

/// Where ONE batch lands in the camera's tan-space — the per-batch diagnostic beside
/// [`glue_art_extent`]: how many of its triangles face the camera (and so can paint), how many
/// face away, and the box its front faces cover. `benilla-extract glueextent --batches` prints it,
/// so a measured edge can be traced to the card that sets it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatchFootprint {
    /// Triangles in front of the near plane that face the camera (or are two-sided).
    pub front: usize,
    /// Single-sided triangles facing away — culled by the renderer, painting nothing.
    pub back: usize,
    /// Clipped away entirely (behind the near plane).
    pub clipped: usize,
    /// The front faces' `x'` range, `None` with no front face.
    pub x: Option<(f32, f32)>,
    /// …and `y'` range.
    pub y: Option<(f32, f32)>,
}

/// One batch's [`BatchFootprint`] under `cam`, regardless of how it paints.
pub fn batch_footprint(sub: &RenderSubmesh, cam: &M2PortraitCamera) -> BatchFootprint {
    let mut fp = BatchFootprint {
        front: 0,
        back: 0,
        clipped: 0,
        x: None,
        y: None,
    };
    let Some(frame) = EyeFrame::of(cam) else {
        return fp;
    };
    let near = cam.near_clip.max(1e-3);
    for tri in sub.indices.as_chunks::<3>().0 {
        let Some(eye) = tri
            .iter()
            .map(|&i| {
                sub.positions
                    .get(i as usize)
                    .map(|&p| (frame.to_eye(p), [0.0; 2]))
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let pieces = clip_near(&eye, near);
        if pieces.is_empty() {
            fp.clipped += 1;
            continue;
        }
        for piece in pieces {
            let proj: [Vert; 3] = piece.map(|((x, y, z), _)| Vert {
                x: x / z,
                y: y / z,
                inv_z: 1.0 / z,
                u_z: 0.0,
                v_z: 0.0,
            });
            if !sub.two_sided && signed_area(&proj) <= 0.0 {
                fp.back += 1;
                continue;
            }
            fp.front += 1;
            for p in proj {
                fp.x = Some(fp.x.map_or((p.x, p.x), |(lo, hi)| (lo.min(p.x), hi.max(p.x))));
                fp.y = Some(fp.y.map_or((p.y, p.y), |(lo, hi)| (lo.min(p.y), hi.max(p.y))));
            }
        }
    }
    fp
}

/// An eye-space vertex `(x, y, depth)` with its UV — the unit `clip_near` works on.
type EyeVert = ((f32, f32, f32), [f32; 2]);

/// Clip an eye-space triangle against `depth = near` (keep `depth ≥ near`), fanning the resulting
/// polygon (0, 3 or 4 vertices) back into triangles. UVs are interpolated with the position.
fn clip_near(tri: &[EyeVert], near: f32) -> Vec<[EyeVert; 3]> {
    let inside = |p: &EyeVert| p.0 .2 >= near;
    let mut poly: Vec<EyeVert> = Vec::with_capacity(4);
    for i in 0..3 {
        let a = tri[i];
        let b = tri[(i + 1) % 3];
        let (ia, ib) = (inside(&a), inside(&b));
        if ia {
            poly.push(a);
        }
        if ia != ib {
            let t = (near - a.0 .2) / (b.0 .2 - a.0 .2);
            let lerp = |p: f32, q: f32| p + (q - p) * t;
            poly.push((
                (lerp(a.0 .0, b.0 .0), lerp(a.0 .1, b.0 .1), near),
                [lerp(a.1[0], b.1[0]), lerp(a.1[1], b.1[1])],
            ));
        }
    }
    match poly.len() {
        3 => vec![[poly[0], poly[1], poly[2]]],
        4 => vec![[poly[0], poly[1], poly[2]], [poly[0], poly[2], poly[3]]],
        _ => Vec::new(),
    }
}

/// The camera's eye frame in model space: `right`, `up`, `forward` unit vectors + the eye.
///
/// Mirrors the engine's `Transform::looking_at(target, up)` with `up = roll about forward ⋅ +Z`
/// (the glue booth's own rig): `forward = target − eye`, `right = forward × up`, `up' = right ×
/// forward`. Model space is WoW's (Z up); the engine's remap is a proper rotation, so cross
/// products — and with them winding — agree between the two frames.
struct EyeFrame {
    eye: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    forward: [f32; 3],
}

impl EyeFrame {
    fn of(cam: &M2PortraitCamera) -> Option<Self> {
        let forward = normalize(sub(cam.target, cam.position))?;
        let up0 = rotate_about(WOW_UP, forward, cam.roll);
        let right = normalize(cross(forward, up0))?;
        let up = cross(right, forward);
        Some(Self {
            eye: cam.position,
            right,
            up,
            forward,
        })
    }

    /// A model-space point in eye coordinates: `(x, y, depth)` with `depth` along `forward`.
    fn to_eye(&self, p: [f32; 3]) -> (f32, f32, f32) {
        let d = sub(p, self.eye);
        (dot(d, self.right), dot(d, self.up), dot(d, self.forward))
    }
}

/// WoW model space is Z-up.
const WOW_UP: [f32; 3] = [0.0, 0.0, 1.0];

// ---- small vector helpers (model space; no engine types in this crate) ----

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = dot(v, v).sqrt();
    (len > 1e-6).then(|| [v[0] / len, v[1] / len, v[2] / len])
}

/// Rodrigues: rotate `v` about the unit `axis` by `angle` radians.
fn rotate_about(v: [f32; 3], axis: [f32; 3], angle: f32) -> [f32; 3] {
    if angle == 0.0 {
        return v;
    }
    let (s, c) = angle.sin_cos();
    let k = cross(axis, v);
    let d = dot(axis, v);
    [
        v[0] * c + k[0] * s + axis[0] * d * (1.0 - c),
        v[1] * c + k[1] * s + axis[1] * d * (1.0 - c),
        v[2] * c + k[2] * s + axis[2] * d * (1.0 - c),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A camera at the origin looking down +X (WoW), Z up: `right` = −Y, `up` = +Z, `forward` = +X.
    fn cam(fov: f32) -> M2PortraitCamera {
        M2PortraitCamera {
            fov,
            far_clip: 100.0,
            near_clip: 0.1,
            position: [0.0, 0.0, 0.0],
            target: [1.0, 0.0, 0.0],
            roll: 0.0,
        }
    }

    /// An axis-aligned card facing the camera at depth `d`, spanning `±w` horizontally (world −Y
    /// is screen right) and `±h` vertically (Z), wound CCW as seen from the camera, UVs
    /// `(0,0)` at screen left-bottom to `(1,1)` at right-top.
    fn card(d: f32, w: f32, h: f32, blend: ModelBlend, two_sided: bool) -> RenderSubmesh {
        // Screen-right is −Y: corners in screen order (left-bottom, right-bottom, right-top,
        // left-top) are y = +w, −w, −w, +w.
        RenderSubmesh {
            positions: vec![[d, w, -h], [d, -w, -h], [d, -w, h], [d, w, h]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            indices: vec![0, 1, 2, 0, 2, 3],
            blend,
            two_sided,
            ..Default::default()
        }
    }

    /// `fov = 1`: `t0 = tan(0.3) = 0.309`, `h0 = 0.413`; the grid spans `±0.68` wide, `±0.84`
    /// tall. The cards below sit inside the span and outside the authored box.
    const FOV: f32 = 1.0;
    /// Two grid cells, the quantisation any extent can be short by.
    const TOL: f32 = 2.0 * 2.0 * X_SPAN * 0.31 / CELLS as f32;

    fn full(_: &RenderSubmesh) -> Option<Coverage> {
        Some(Coverage::Full)
    }

    /// An alpha map opaque only across `lo..hi` of `u`, below the key elsewhere.
    fn band(lo: f32, hi: f32) -> Arc<AlphaMap> {
        let (w, h) = (64u32, 16u32);
        let alpha = (0..w * h)
            .map(|i| {
                let u = (i % w) as f32 / w as f32;
                if (lo..hi).contains(&u) {
                    255
                } else {
                    ALPHA_KEY_REF - 1
                }
            })
            .collect();
        Arc::new(AlphaMap {
            width: w,
            height: h,
            alpha,
        })
    }

    #[test]
    fn a_wide_card_reports_its_own_half_extents() {
        // At depth 10, a card ±5 wide / ±4 tall projects to ±0.5 / ±0.4 — both larger than the
        // authored box, so the measured extents are the card's.
        let sub = card(10.0, 5.0, 4.0, ModelBlend::Opaque, false);
        let ext = glue_art_extent([&sub], &cam(FOV), full);
        assert!((ext.half_w - 0.5).abs() < TOL, "half_w {}", ext.half_w);
        assert!((ext.half_h - 0.4).abs() < TOL, "half_h {}", ext.half_h);
    }

    #[test]
    fn a_card_narrower_than_the_authored_box_reports_zero() {
        // ±2 at depth 10 → ±0.2 < t0 = 0.309: the authored vertical opening is not even covered.
        let sub = card(10.0, 2.0, 2.0, ModelBlend::Opaque, false);
        let ext = glue_art_extent([&sub], &cam(FOV), full);
        assert_eq!(
            ext,
            ArtExtent {
                half_w: 0.0,
                half_h: 0.0
            }
        );
    }

    #[test]
    fn backfaces_are_not_coverage_unless_two_sided() {
        let mut back = card(10.0, 5.0, 4.0, ModelBlend::Opaque, false);
        back.indices.reverse(); // now wound CW as seen from the camera
        assert_eq!(glue_art_extent([&back], &cam(FOV), full).half_w, 0.0);
        back.two_sided = true;
        assert!((glue_art_extent([&back], &cam(FOV), full).half_w - 0.5).abs() < TOL);
    }

    #[test]
    fn a_batch_that_counts_for_nothing_paints_nothing() {
        let sub = card(10.0, 5.0, 4.0, ModelBlend::Blend, false);
        assert_eq!(glue_art_extent([&sub], &cam(FOV), |_| None).half_w, 0.0);
    }

    #[test]
    fn an_alpha_texture_paints_only_where_it_passes_the_key() {
        // A ±5 card (±0.5 at depth 10) whose texture is opaque only across u ∈ [0.2, 0.8]: it
        // paints ±0.3 — the rows read that band; and since ±0.3 is inside the authored ±0.413,
        // a column of the box goes unpainted and the height reads 0.
        let sub = card(10.0, 5.0, 4.0, ModelBlend::AlphaTest, false);
        let map = band(0.2, 0.8);
        let ext = glue_art_extent([&sub], &cam(FOV), |_| Some(Coverage::Alpha(map.clone())));
        assert!(
            (ext.half_w - 0.3).abs() < TOL + 1.0 / 64.0,
            "half_w {}",
            ext.half_w
        );
        assert_eq!(ext.half_h, 0.0, "a column inside the box is unpainted");
        // A band wider than the box (u ∈ [0.05, 0.95] → ±0.45) paints the full height.
        let map = band(0.05, 0.95);
        let ext = glue_art_extent([&sub], &cam(FOV), |_| Some(Coverage::Alpha(map.clone())));
        assert!((ext.half_h - 0.4).abs() < TOL, "half_h {}", ext.half_h);
        assert!(
            (ext.half_w - 0.45).abs() < TOL + 1.0 / 64.0,
            "half_w {}",
            ext.half_w
        );
    }

    #[test]
    fn the_narrowest_row_wins() {
        // A trapezoid: ±5 wide at the bottom, ±2.5 at the top (depth 10) — the frame's top row
        // (y' = +t0 = 0.309, i.e. Z = 3.09) sees a half-width interpolated between the two:
        // at Z = 3.09 of a ±4-tall card, w = 5 − 2.5·(3.09+4)/8 = 2.78 → 0.278.
        let mut sub = card(10.0, 5.0, 4.0, ModelBlend::Opaque, false);
        sub.positions[2] = [10.0, -2.5, 4.0];
        sub.positions[3] = [10.0, 2.5, 4.0];
        let ext = glue_art_extent([&sub], &cam(FOV), full);
        let t0 = authored_half_height(FOV);
        let expect = (5.0 - 2.5 * (t0 * 10.0 + 4.0) / 8.0) / 10.0;
        assert!(
            (ext.half_w - expect).abs() < 3e-3,
            "{} vs {expect}",
            ext.half_w
        );
    }

    #[test]
    fn a_card_behind_the_camera_is_clipped_away_and_one_straddling_it_is_clipped_to_near() {
        let behind = card(-10.0, 5.0, 4.0, ModelBlend::Opaque, true);
        assert_eq!(glue_art_extent([&behind], &cam(FOV), full).half_w, 0.0);
        // A ground plane running from behind the camera to far ahead, below the eye: its near
        // clip leaves finite geometry, the projection stays finite, and the covered rows are the
        // lower half only — so the *full* authored opening is not covered and the extent is 0,
        // without any NaN or panic on the way.
        let mut ground = card(0.0, 50.0, 0.0, ModelBlend::Opaque, true);
        ground.positions = vec![
            [-5.0, 50.0, -1.0],
            [-5.0, -50.0, -1.0],
            [50.0, -50.0, -1.0],
            [50.0, 50.0, -1.0],
        ];
        let ext = glue_art_extent([&ground], &cam(FOV), full);
        assert!(ext.half_w.is_finite() && ext.half_h.is_finite());
        assert_eq!(ext.half_w, 0.0);
    }

    #[test]
    fn adjacent_cards_paint_one_run_across_their_shared_edge() {
        let mut left = card(10.0, 5.0, 4.0, ModelBlend::Opaque, false);
        left.positions = vec![
            [10.0, 5.0, -4.0],
            [10.0, 0.0, -4.0],
            [10.0, 0.0, 4.0],
            [10.0, 5.0, 4.0],
        ];
        let mut right = card(10.0, 5.0, 4.0, ModelBlend::Opaque, false);
        right.positions = vec![
            [10.0, 0.0, -4.0],
            [10.0, -5.0, -4.0],
            [10.0, -5.0, 4.0],
            [10.0, 0.0, 4.0],
        ];
        let ext = glue_art_extent([&left, &right], &cam(FOV), full);
        assert!((ext.half_w - 0.5).abs() < TOL, "half_w {}", ext.half_w);
    }

    /// Measure one shipped scene off the chain, the way the tool does.
    fn measure(chain: &mut Chain, token: &str) -> (ArtExtent, f32, f32) {
        let name = format!("Interface\\Glues\\Models\\UI_{token}\\UI_{token}.m2");
        let bytes = chain.read_file(&name).expect("read scene");
        let subs = crate::parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
        let cam = crate::parse_m2_camera(&bytes, 0).expect("camera 0");
        let mut reader = CoverageReader::new(chain);
        let ext = glue_art_extent(&subs, &cam, |s| reader.coverage(s).expect("texture"));
        (ext, cam.fov, authored_half_height(cam.fov))
    }

    /// The transcription is the measurement: every shipped scene re-measures to the table's
    /// numbers (within the grid's quantisation), every one covers (about) its authored 4:3 box
    /// in width, and every one runs out of width before 16:9 — the fact the whole law rests on.
    #[test]
    fn the_shipped_table_matches_the_measurement() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        for scene in SHIPPED_GLUE_SCENES {
            let token = scene.token;
            let shipped = scene.art;
            let (ext, fov, t0) = measure(&mut chain, token);
            assert!(
                (ext.half_w - shipped.half_w).abs() < 2e-3
                    && (ext.half_h - shipped.half_h).abs() < 2e-3,
                "UI_{token}: measured {ext:?}, table {shipped:?}"
            );
            // The fov rides the table because the glue framing constant is derived from it.
            assert!(
                (fov - scene.fov).abs() < 1e-6,
                "UI_{token}: camera 0 fov {fov}, table {}",
                scene.fov
            );
            let runs_out_at = ext.half_w / t0;
            assert!(
                (GLUE_AUTHORED_ASPECT - 0.03..16.0 / 9.0).contains(&runs_out_at),
                "UI_{token} covers up to aspect {runs_out_at:.3}"
            );
            // Every stage but the night elves' fills its authored box top to bottom; theirs
            // shows black night sky between the leaves at its sides (the table's doc).
            if token == "NightElf" {
                assert!(
                    ext.half_h < t0,
                    "UI_NightElf: half_h {} — leaf gaps closed?",
                    ext.half_h
                );
            } else {
                assert!(
                    ext.half_h >= t0,
                    "UI_{token}: half_h {} < t0 {t0}",
                    ext.half_h
                );
            }
        }
    }
}
