//! The art extent of a glue scene: how far its art paints around camera 0, texel by texel. The
//! glue framing widens past 4:3 (hor+) where the reference zooms on its diagonal-FOV law, and a
//! diorama's art is finite, so past its edge the frame would show the clear colour. Reading
//! textures makes this an offline measurement (`benilla-extract glueextent`), shipped as
//! [`SHIPPED_GLUE_SCENES`]. Model space throughout (WoW axes, Z up).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use super::records::M2PortraitCamera;
use super::types::{ModelBlend, RenderSubmesh};
use crate::Chain;

/// The tan-space half-extents the art fills: `half_w` across the authored 4:3 vertical opening,
/// `half_h` across the horizontal one, `0.0` where it does not fill even the authored box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArtExtent {
    pub half_w: f32,
    pub half_h: f32,
}

/// The aspect every glue composition was made for, the `1024×768` design space.
pub const GLUE_AUTHORED_ASPECT: f32 = 4.0 / 3.0;

/// The 1.12 client's alpha test passes a texel at `alpha ≥ 224` (`0x70c256`, `GEQUAL`).
pub const ALPHA_KEY_REF: u8 = 224;

/// One degree: every authored glue fov is a whole number of degrees, to the last `f32` bit.
const DEG: f32 = std::f32::consts::PI / 180.0;

/// A shipped scene; its fov rides along because the app's `GLUE_BOX_ASPECT` derives from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShippedGlueScene {
    pub token: &'static str,
    pub fov: f32,
    pub art: ArtExtent,
}

/// The seven shipped scenes (Gnome shares Dwarf's, Troll Orc's), re-measured by a chain test.
/// Every one runs out of width before 16:9. `UI_NightElf`'s `half_h` is inside the authored box:
/// the gaps between its tree cutouts paint nothing even at 4:3, and read as night sky.
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

/// A shipped scene's measured [`ArtExtent`] by token; `None` for any other token.
pub fn shipped_glue_art_extent(token: &str) -> Option<ArtExtent> {
    SHIPPED_GLUE_SCENES
        .iter()
        .find(|s| s.token == token)
        .map(|s| s.art)
}

/// A glue camera's authored vertical half-extent at 4:3, `tan(fovy/2)`, `fovy = fov/√((4/3)²+1)`.
pub fn authored_half_height(fov: f32) -> f32 {
    (fov / (GLUE_AUTHORED_ASPECT * GLUE_AUTHORED_ASPECT + 1.0).sqrt() * 0.5).tan()
}

/// How a batch paints the pixels it covers.
#[derive(Clone, Debug)]
pub enum Coverage {
    /// Every covered pixel paints: an opaque batch, or a texture never below the key.
    Full,
    /// Painted where the texture's alpha through the batch's UVs is `≥` [`ALPHA_KEY_REF`].
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
    /// The nearest texel's alpha at `(u, v)`, each axis wrapping (`true`) or clamping to the edge.
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

/// The coverage rule off the chain: `Opaque` paints, `Blend`/`AlphaTest` paint by the texture's
/// alpha (read once per path), and `Mod`/`Mod2x` or a missing texture count for nothing.
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

/// Measure a scene's [`ArtExtent`]: the half-width every row reaches, the half-height every column
/// does. The narrowest row decides, as a sky card in perspective is a trapezoid.
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

/// The coverage grid over tan-space, `x' ∈ ±X_SPAN·t0`, `y' ∈ ±Y_SPAN·t0`: no diorama's edge is
/// as far out as `2·t0`, and `2.7·t0` of height is a 1:2 window.
struct Grid {
    cells: Vec<bool>,
    /// Tan units per cell, both axes.
    dx: f32,
    dy: f32,
    x_lo: f32,
    y_lo: f32,
}

/// Cells per axis, even so the axis is a cell boundary: `0.0021·t0` a cell, 1.5 px at 1440p.
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

    /// The largest half-extent along `axis` that every scanline within `±opening` reaches on both
    /// sides, `0.0` if any leaves the axis unpainted; half a cell short, so it never over-reports.
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

/// Twice the signed area of a projected triangle, positive for counter-clockwise.
fn signed_area(t: &[Vert; 3]) -> f32 {
    edge(&t[0], &t[1], t[2].x, t[2].y)
}

/// Where one batch lands in the camera's tan-space, to trace a measured edge to the card that
/// sets it (`benilla-extract glueextent --batches`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatchFootprint {
    /// Triangles in front of the near plane that face the camera (or are two-sided).
    pub front: usize,
    /// Single-sided triangles facing away, which the renderer culls.
    pub back: usize,
    /// Clipped away entirely (behind the near plane).
    pub clipped: usize,
    /// The front faces' `x'` range, `None` with no front face.
    pub x: Option<(f32, f32)>,
    /// The front faces' `y'` range.
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

/// An eye-space vertex `(x, y, depth)` with its UV.
type EyeVert = ((f32, f32, f32), [f32; 2]);

/// Clip an eye-space triangle to `depth ≥ near`, fanning what is left back into triangles.
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

/// The camera's eye frame in model space, as the glue booth's `Transform::looking_at` rig builds
/// it with `+Z` rolled about forward as up. The engine's axis remap is a proper rotation, so
/// winding agrees between the two frames.
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

    /// A card facing the camera at depth `d`, `±w` wide (world −Y is screen right) and `±h` tall,
    /// wound CCW from the camera, UVs `(0,0)` at screen left-bottom to `(1,1)` at right-top.
    fn card(d: f32, w: f32, h: f32, blend: ModelBlend, two_sided: bool) -> RenderSubmesh {
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
        // ±5 by ±4 at depth 10 projects to ±0.5 by ±0.4, both past the authored box.
        let sub = card(10.0, 5.0, 4.0, ModelBlend::Opaque, false);
        let ext = glue_art_extent([&sub], &cam(FOV), full);
        assert!((ext.half_w - 0.5).abs() < TOL, "half_w {}", ext.half_w);
        assert!((ext.half_h - 0.4).abs() < TOL, "half_h {}", ext.half_h);
    }

    #[test]
    fn a_card_narrower_than_the_authored_box_reports_zero() {
        // ±2 at depth 10 is ±0.2, under t0 = 0.309: the authored opening is not even covered.
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
        // Opaque only across u ∈ [0.2, 0.8] of a ±0.5 card: ±0.3 wide, inside the authored
        // ±0.413, so a column of the box goes unpainted and the height reads 0.
        let sub = card(10.0, 5.0, 4.0, ModelBlend::AlphaTest, false);
        let map = band(0.2, 0.8);
        let ext = glue_art_extent([&sub], &cam(FOV), |_| Some(Coverage::Alpha(map.clone())));
        assert!(
            (ext.half_w - 0.3).abs() < TOL + 1.0 / 64.0,
            "half_w {}",
            ext.half_w
        );
        assert_eq!(ext.half_h, 0.0, "a column inside the box is unpainted");
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
        // A trapezoid, ±5 wide at the bottom and ±2.5 at the top (depth 10): the frame's top row
        // (y' = t0 = 0.309, Z = 3.09) sees w = 5 − 2.5·(3.09+4)/8 = 2.78, so 0.278.
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
        // A ground plane from behind the eye to far ahead: finite, lower rows only, so 0.
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

    fn measure(chain: &mut Chain, token: &str) -> (ArtExtent, f32, f32) {
        let name = format!("Interface\\Glues\\Models\\UI_{token}\\UI_{token}.m2");
        let bytes = chain.read_file(&name).expect("read scene");
        let subs = crate::parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
        let cam = crate::parse_m2_camera(&bytes, 0).expect("camera 0");
        let mut reader = CoverageReader::new(chain);
        let ext = glue_art_extent(&subs, &cam, |s| reader.coverage(s).expect("texture"));
        (ext, cam.fov, authored_half_height(cam.fov))
    }

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
