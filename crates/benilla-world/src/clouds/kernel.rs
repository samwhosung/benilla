//! The reference's procedural cloud field (`0xce98e8`): a scrolling 128×128 byte tile of 4-octave
//! toroidal value noise, thresholded by the Light.dbc cloud density, shaped by a fixed tone curve
//! (`0x6d0900`) and colored into the texels the dome draws. The glare samples the same tile.

use bevy::math::Vec3;

use super::tables::{fade_table, gradient_table, CURVE, PERM};

/// Tile side at `SkyCloudLOD` 0, its default (`128 << LOD`, `0x6d1d60`); only LOD 0 is built.
pub const COLS: usize = 128;
/// `log2(COLS)`, the sampler's row-pitch shift (`[cfg+0x20]`).
pub const SHIFT: u32 = 7;
/// Rows regenerated per fire (`[cfg+0x14]`).
pub const ROWS_PER_TICK: usize = 32;
/// Octaves (`[cfg+0x28]`).
pub const OCTAVES: usize = 4;
/// Regen countdown reset in seconds (`0x8115b4`): a 10 Hz cadence.
pub const REGEN_PERIOD: f32 = 0.1;
/// Per-octave lattice frequencies, LOD 0 row of the base table `0x86f3dc` (`(16 >> LOD) << oct`).
const BASE_FREQ: [u16; OCTAVES] = [16, 32, 64, 128];

#[inline]
fn perm(i: u32) -> u32 {
    u32::from(PERM[(i & 0xff) as usize])
}

/// Per-octave lattice walk state: `0x6cffc0`'s 0x54-byte stack record as named fields.
struct Octave {
    freq: u16,
    /// Row key (`B+4`), from `scroll·freq`, `+freq` per row: high byte the cell, low byte the fade.
    row_key: u16,
    col_key: u16,
    amp: f32,
    /// Row corner seeds (`B+0x1e..0x2a`): `x*` through the current time slice, `y*` the next.
    x0: u32,
    x1: u32,
    y0: u32,
    y1: u32,
    g00: f32,
    g00d: f32,
    g10: f32,
    g10d: f32,
    g01: f32,
    g01d: f32,
    g11: f32,
    g11d: f32,
    cached: u32,
}

/// The cloud coverage field: the byte tile, its colored texels and the regen state.
pub struct CloudKernel {
    /// Coverage bytes (`[cfg+0x44]`), `COLS²` row-major; `byte/255` is the coverage `R`.
    tile: Vec<u8>,
    accum: Vec<f32>,
    /// Per-cell slopes from the octave-2 leg (`[cfg+0x68]`), the glow normal `(dx, dy, 1)`.
    deriv: Vec<[f32; 2]>,
    prevrow: Vec<f32>,
    /// Colored texels (`[cfg+0x38]`), the buffer the reference binds as its texture (`0x58ac70`).
    rgba: Vec<[u8; 4]>,
    /// The tile row the next band starts at (`[cfg+0x18]`).
    scroll: usize,
    /// The noise's time axis (`[cfg+0xa0]`), +1 per full tile wrap.
    phase: u16,
    /// Regen countdown (`[cfg+0xb0]`); starts expired so the first tick fires.
    countdown: f32,
    gradient: [f32; 256],
    fade: [f32; 256],
}

/// The color pass inputs (`0x6cfb00`'s per-frame setup), from [`crate::lighting::WowLighting`].
#[derive(Clone, Copy, PartialEq)]
pub struct CloudFrame {
    /// Sun-glow palette (IntBand sub-10), sRGB 0..1.
    pub(crate) sun: [f32; 3],
    /// Gradient slope (IntBand sub-11).
    pub(crate) slope: [f32; 3],
    /// Gradient base (IntBand sub-12).
    pub(crate) gbase: [f32; 3],
    /// The weather storm blend, which sets the glow's z-bias and dim.
    pub(crate) bcc: f32,
    /// Camera→glow body direction (Bevy frame): the sun by day, the moon by night.
    pub(crate) glow_dir: Vec3,
    /// The glow's day envelope, the reference's static track at `0xce9ab8`.
    pub(crate) glow_track: f32,
}

impl Default for CloudKernel {
    fn default() -> Self {
        CloudKernel {
            tile: vec![0; COLS * COLS],
            accum: vec![0.0; COLS * COLS],
            deriv: vec![[0.0; 2]; COLS * COLS],
            prevrow: vec![0.0; COLS],
            // Deviation: the reference starts this buffer at 0xFFFFFFFF; alpha 0 keeps a dome
            // drawn before the first build from flashing white.
            rgba: vec![[255, 255, 255, 0]; COLS * COLS],
            scroll: 0,
            phase: 0,
            countdown: 0.0,
            gradient: gradient_table(),
            fade: fade_table(),
        }
    }
}

impl CloudKernel {
    /// Regenerates one band when the countdown runs out (`0x6cffc0`); true if the tile changed.
    pub fn tick(&mut self, dt: f32, density: f32, frame: &CloudFrame) -> bool {
        self.countdown -= dt;
        if self.countdown > 0.0 {
            return false;
        }
        self.countdown = REGEN_PERIOD;
        self.regen(density, ROWS_PER_TICK, frame);
        true
    }

    /// Regenerates every row (`0x6cff90`), as the reference does on init, zone and LOD changes.
    pub fn rebuild(&mut self, density: f32, frame: &CloudFrame) {
        self.scroll = 0;
        self.regen(density, COLS, frame);
        self.countdown = REGEN_PERIOD;
    }

    /// Re-runs the color pass over the whole tile without touching coverage, for the frozen clock.
    pub fn recolor(&mut self, frame: &CloudFrame) {
        self.color_band(0, COLS, frame);
    }

    /// One fire of `0x6cffc0`: noise, quantize and color `rows` rows from `scroll`, then advance.
    fn regen(&mut self, density: f32, rows: usize, frame: &CloudFrame) {
        // Threshold (`0x6d0970`): the reference does not clamp; authored densities stay in [0, 1].
        let threshold = ((1.0 - density.clamp(0.0, 1.0)) * 255.0) as i32;

        // The time axis: the phase's high byte picks the permutation slice pair.
        let seed = u32::from(self.phase >> 8);
        let slice_a = perm(seed); // current time slice
        let slice_b = perm(seed + 1); // next time slice
        let fade_t = f64::from(self.fade[(self.phase & 0xff) as usize]); // time fraction

        let scroll = self.scroll;
        let mut oct: Vec<Octave> = (0..OCTAVES)
            .map(|c| {
                let freq = BASE_FREQ[c];
                Octave {
                    freq,
                    // Keyed by the absolute row, so a band regenerates identically at one phase.
                    row_key: (scroll as u32).wrapping_mul(u32::from(freq)) as u16,
                    col_key: 0,
                    amp: 1.0 / (1u32 << c) as f32,
                    x0: 0,
                    x1: 0,
                    y0: 0,
                    y1: 0,
                    g00: 0.0,
                    g00d: 0.0,
                    g10: 0.0,
                    g10d: 0.0,
                    g01: 0.0,
                    g01d: 0.0,
                    g11: 0.0,
                    g11d: 0.0,
                    cached: u32::MAX,
                }
            })
            .collect();

        for v in &mut self.accum[scroll * COLS..(scroll + rows).min(COLS) * COLS] {
            *v = 0.0;
        }

        for row in 0..rows {
            let base = (scroll + row) * COLS;
            for o in oct.iter_mut() {
                let bv = u32::from(o.row_key >> 8);
                o.x0 = perm(slice_a + bv);
                o.x1 = perm(slice_a + bv + 1);
                o.y0 = perm(bv + slice_b);
                o.y1 = perm(bv + 1 + slice_b);
                o.col_key = self.phase;
                o.cached = u32::MAX;
            }
            // The previous column's three-octave partial sum, reset per row.
            let mut prev_accum = 0.0f32;
            for col in 0..COLS {
                let cell = base + col;
                for (oi, o) in oct.iter_mut().enumerate() {
                    let fade_row = f64::from(self.fade[(o.row_key & 0xff) as usize]);
                    let cz = u32::from(o.col_key >> 8); // column lattice cell
                    if cz != o.cached {
                        o.cached = cz;
                        let g = |s: u32| self.gradient[perm(s) as usize];
                        let (s0, s1, s2, s3) = (o.x0 + cz, o.x1 + cz, o.y0 + cz, o.y1 + cz);
                        o.g00 = g(s0);
                        o.g00d = g(s0 + 1) - o.g00;
                        o.g10 = g(s1);
                        o.g10d = g(s1 + 1) - o.g10;
                        o.g01 = g(s2);
                        o.g01d = g(s2 + 1) - o.g01;
                        o.g11 = g(s3);
                        o.g11d = g(s3 + 1) - o.g11;
                    }
                    // The reference's f64 chain, with its one f32 round trip (`v8`).
                    let fx = f64::from(self.fade[(o.col_key & 0xff) as usize]);
                    let v7 = fx * f64::from(o.g00d) + f64::from(o.g00);
                    let v8 = f64::from((fx * f64::from(o.g01d) + f64::from(o.g01)) as f32);
                    let v7b = (fx * f64::from(o.g10d) + f64::from(o.g10) - v7) * fade_row + v7;
                    let v8b = (fx * f64::from(o.g11d) + f64::from(o.g11) - v8) * fade_row + v8;
                    o.col_key = o.col_key.wrapping_add(o.freq);
                    let acc = f64::from(self.accum[cell]);
                    let stored = (((v8b - v7b) * fade_t + v7b) * f64::from(o.amp) + acc) as f32;
                    self.accum[cell] = stored;
                    // Octave 2 stores the running sum's slopes: the color pass's glow normal.
                    if oi == 2 {
                        let scale = f64::from((1i32 << ((SHIFT - 7) & 0x1f)) as f32);
                        self.deriv[cell][0] =
                            ((f64::from(prev_accum) - f64::from(stored)) * scale) as f32;
                        let pr = f64::from(self.prevrow[col]);
                        self.deriv[cell][1] = ((pr - f64::from(stored)) * scale) as f32;
                        prev_accum = stored;
                        self.prevrow[col] = stored;
                    }
                }
            }
            for o in oct.iter_mut() {
                o.row_key = o.row_key.wrapping_add(o.freq);
            }
        }

        // Quantize by the reference's float-bits pack, then threshold and tone curve.
        for cell in scroll * COLS..(scroll + rows).min(COLS) * COLS {
            let q = ((f64::from(self.accum[cell]) * 64.0 + 128.0 + 512.0) as f32).to_bits();
            let idx = ((q >> 14) & 0xff) as i32 - threshold;
            self.tile[cell] = if idx >= 0 { CURVE[idx as usize] } else { 0 };
        }

        // The color pass (`0x6cfb00`) runs before the scroll advance.
        self.color_band(scroll, rows, frame);

        self.scroll += rows;
        if self.scroll >= COLS {
            self.phase = self.phase.wrapping_add(1);
            self.scroll = 0;
        }
    }

    /// The color pass (`0x6cfb00`): a hole copies the previous cell's RGB at alpha 0; a covered
    /// cell is a gradient in its coverage plus a glow by the angle to its slope normal.
    fn color_band(&mut self, start: usize, rows: usize, frame: &CloudFrame) {
        let f = f64::from;
        let z_bias = (f(frame.bcc) * 192.0 + 64.0) as f32;
        let intensity = (f(frame.glow_track) * (1.0 - f(frame.bcc) * 0.75)) as f32;
        let body = body_cells(frame.glow_dir);
        for row in start..(start + rows).min(COLS) {
            let row_base = row as f32; // the absolute tile row
            for col in 0..COLS {
                let g = row * COLS + col;
                let t = self.tile[g];
                if t == 0 {
                    if col != 0 {
                        let prev = self.rgba[g - 1];
                        self.rgba[g] = [prev[0], prev[1], prev[2], 0];
                    }
                    continue;
                }
                // The angle byte, in [64, 191].
                let n = u32::from((255u8.wrapping_sub(t) >> 1).wrapping_add(0x40));
                let p = f(INV_255) * f64::from(n);
                let mut ch = [
                    (f(frame.slope[0]) * p + f(frame.gbase[0])) as f32,
                    (f(frame.slope[1]) * p + f(frame.gbase[1])) as f32,
                    (f(frame.slope[2]) * p + f(frame.gbase[2])) as f32,
                ];
                if let Some((su, sv)) = body {
                    // The reference's sum and product order: a one-ulp change moves output bytes.
                    let vx = (f(su) - f64::from(col as u32)) as f32;
                    let vy = (f(sv) - f(row_base)) as f32;
                    let vz = z_bias;
                    let s = self.deriv[g];
                    let len_v_sq = ((f(vz) * f(vz) + f(vy) * f(vy)) + f(vx) * f(vx)) as f32;
                    let len_s_sq = ((f(s[0]) * f(s[0]) + f(s[1]) * f(s[1])) + 1.0) as f32;
                    let dot = f(vx) * f(s[0]) + f(vy) * f(s[1]) + f(vz);
                    let cos_t = dot * (f(fisr(len_v_sq)) * f(fisr(len_s_sq)));
                    if cos_t > 0.0 {
                        let m = cos_t * f(intensity);
                        ch[0] = (f(frame.sun[0]) * m + f(ch[0])) as f32;
                        ch[1] = (f(frame.sun[1]) * m + f(ch[1])) as f32;
                        ch[2] = (f(frame.sun[2]) * m + f(ch[2])) as f32;
                    }
                }
                self.rgba[g] = [
                    pack_channel(f(ch[0])),
                    pack_channel(f(ch[1])),
                    pack_channel(f(ch[2])),
                    t,
                ];
            }
        }
    }

    /// The coverage in [0, 1] toward camera-relative `d` (`0x6cfa90`). `d` is not normalized: the
    /// glare samples at its 12-unit sky point, so the zenith shift weighs `cos45°/|d|`.
    pub fn coverage(&self, d: Vec3) -> f32 {
        let Some((u, v)) = project_cells(d) else {
            return f32::from(self.tile[(COLS / 2) * COLS + COLS / 2]) / 255.0;
        };
        let (col, row) = (u as i32, v as i32);
        // Deviation: the index wraps where the reference reads past the tile, only at `u == 1.0`.
        let cell = ((row as usize & (COLS - 1)) << SHIFT) + (col as usize & (COLS - 1));
        f32::from(self.tile[cell]) / 255.0
    }

    /// The colored texels the visible layer uploads (`0x58ac70`).
    pub fn rgba(&self) -> &[[u8; 4]] {
        &self.rgba
    }

    #[cfg(test)]
    pub(crate) fn tile(&self) -> &[u8] {
        &self.tile
    }

    #[cfg(test)]
    pub(crate) fn set_phase(&mut self, phase: u16) {
        self.phase = phase;
    }
}

/// The azimuthal tile projection (`0x6cf870`): the zenith at the centre, the radius growing with
/// the angle off a `+cos(π/4)`-shifted axis up to the 45° rim, where lower directions clamp.
fn project_cells(d: Vec3) -> Option<(f32, f32)> {
    let len = f64::from(d.length());
    if len < 1e-6 {
        return None;
    }
    let quarter_pi = f64::from(std::f32::consts::FRAC_PI_4);
    let c = f64::from(d.y) + f64::from(std::f32::consts::FRAC_PI_4.cos());
    // Deviation: clamped; the reference NaNs above about 70° elevation, where its bodies never go.
    let theta = (c / len).clamp(-1.0, 1.0).acos();
    let phase = theta.min(quarter_pi) / quarter_pi * 0.5;
    let hyp = (f64::from(d.x) * f64::from(d.x) + f64::from(d.z) * f64::from(d.z)).sqrt();
    let (cx, cy) = if hyp > 1e-5 {
        let inv = (1.0 / hyp) as f32;
        (
            f64::from(inv) * f64::from(d.x),
            f64::from(inv) * f64::from(d.z),
        )
    } else {
        (0.0, 0.0)
    };
    Some((
        ((cx * phase + 0.5) * COLS as f64) as f32,
        ((cy * phase + 0.5) * COLS as f64) as f32,
    ))
}

/// The glow body's tile cell (`0x6cfb00` setup): the camera→body ray's far hit on the unit dome
/// shifted by `−cos(π/4)` (`0x6cf9c0`), projected onto the tile.
fn body_cells(dir: Vec3) -> Option<(f32, f32)> {
    let f = f64::from;
    // k = −cos(π/4), the dome's shift.
    let k = -(f(0.25f32) * f(std::f32::consts::PI)).cos();
    let a = (f(dir.x) * f(dir.x) + f(dir.y) * f(dir.y) + f(dir.z) * f(dir.z)) as f32;
    let b = {
        // −k·up, doubled; the reference's up is z, ours is y.
        let nk = -k * f(dir.y);
        (nk + nk) as f32
    };
    let c = (k * k - 1.0) as f32;
    let t2 = quadratic_larger_root(a, b, c)?;
    let hit = Vec3::new(
        (f(t2) * f(dir.x)) as f32,
        (f(t2) * f(dir.y)) as f32,
        (f(t2) * f(dir.z)) as f32,
    );
    project_cells(hit)
}

/// The larger root of `a·t² + b·t + c = 0`, rounded as the reference's solver does (`0x454f40`).
fn quadratic_larger_root(a: f32, b: f32, c: f32) -> Option<f32> {
    let f = f64::from;
    let g = f(a) * f(c) * 4.0;
    let h = f(b) * f(b);
    if h.is_nan() || g.is_nan() || h <= g {
        return None;
    }
    let q = (h - g).sqrt();
    let bpm = if b > 0.0 { f(b) + q } else { f(b) - q };
    let s = bpm * -0.5;
    let inv = 1.0 / (f(a) * s);
    let inv_f32 = inv as f32;
    let root_b = (inv * s * s) as f32;
    let root_a = (f(inv_f32) * f(a) * f(c)) as f32;
    Some(if root_b.is_nan() || root_b >= root_a {
        root_b
    } else {
        root_a
    })
}

/// `1/255` as the reference stores it (`0x8026c8`).
const INV_255: f32 = f32::from_bits(0x3b80_8081);

/// The reference's inverse-sqrt seed (`0x456330`), no Newton step: its error shapes the glow.
fn fisr(x: f32) -> f32 {
    f32::from_bits(0x5f39_97bbu32.wrapping_sub((x.to_bits() >> 1) & 0x3fff_ffff))
}

/// Packs a channel as the reference does (`0x6cfef6..0x6cff44`): clamped above only, floored.
fn pack_channel(ch: f64) -> u8 {
    let clamped = if ch < 1.0 { ch } else { 1.0 };
    (((clamped * 255.0 + 512.0) as f32).to_bits() >> 14) as u8
}

/// The sun glare's cloud occlusion (`0x6cf7b0`): the flare dims linearly with coverage.
pub fn occ1_sun(r: f32) -> f32 {
    1.0 - r
}

/// The moon glare's cloud occlusion (`0x6cf7d0`): a tent, so the halo blooms only in thin cloud.
pub fn occ1_moon(r: f32) -> f32 {
    1.0 - (2.0 * (r - 0.5)).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> CloudFrame {
        CloudFrame {
            sun: [1.0, 0.78, 0.54],
            slope: [0.17, 0.41, 0.52],
            gbase: [0.1, 0.1, 0.12],
            bcc: 0.0,
            glow_dir: Vec3::new(0.6, 0.5, 0.1).normalize(),
            glow_track: 1.0,
        }
    }

    /// Density 0 puts the threshold at 255, so every cell quantizes to 0.
    #[test]
    fn clear_sky_is_empty_coverage() {
        let mut k = CloudKernel::default();
        k.rebuild(0.0, &frame());
        assert!(k.tile().iter().all(|&b| b == 0));
        assert!(k.rgba().iter().all(|px| px[3] == 0));
        let sun_dir = Vec3::new(0.3, 0.5, 0.2).normalize() * 12.0;
        assert_eq!(k.coverage(sun_dir), 0.0);
        assert_eq!(occ1_sun(0.0), 1.0);
        assert_eq!(occ1_moon(0.0), 0.0);
    }

    /// Density 1 is overcast; 0.6 gives the reference's init threshold, 101, and scattered cloud.
    #[test]
    fn density_shapes_the_field_and_regen_is_deterministic() {
        let mut a = CloudKernel::default();
        let mut b = CloudKernel::default();
        a.rebuild(1.0, &frame());
        b.rebuild(1.0, &frame());
        assert_eq!(a.tile(), b.tile());
        assert_eq!(a.rgba(), b.rgba());
        // Measured: mean about 242, minimum 135.
        assert!(
            a.tile().iter().all(|&v| v > 0),
            "overcast leaves no clear cell"
        );
        let mean = a.tile().iter().map(|&v| u32::from(v)).sum::<u32>() / a.tile().len() as u32;
        assert!(mean > 200, "overcast mean {mean}");
        let mut mid = CloudKernel::default();
        mid.rebuild(0.6, &frame());
        let clear = mid.tile().iter().filter(|&&v| v == 0).count();
        let covered = mid.tile().iter().filter(|&&v| v > 100).count();
        assert!(
            clear > 0 && covered > 0,
            "expected scattered cover, got clear={clear} covered={covered}"
        );
    }

    /// Row 0's texels differ: its row slope reads the previous-row scratch, which a fresh rebuild
    /// and a scrolled band see differently, as in the reference.
    #[test]
    fn incremental_bands_tile_the_full_field() {
        let mut inc = CloudKernel::default();
        inc.rebuild(0.6, &frame()); // leaves phase 1, scroll 0
        for _ in 0..4 {
            inc.tick(1.0, 0.6, &frame());
        }
        let mut full = CloudKernel::default();
        full.set_phase(1);
        full.rebuild(0.6, &frame());
        assert_eq!(inc.tile(), full.tile());
        assert_eq!(inc.rgba()[COLS..], full.rgba()[COLS..]);
    }

    #[test]
    fn color_pass_matches_the_byte_math() {
        let mut k = CloudKernel::default();
        let mut f = frame();
        f.glow_track = 0.0; // glow off: every texel is the pure gradient
        k.rebuild(1.0, &f);
        for (g, px) in k.rgba().iter().enumerate() {
            let t = k.tile()[g];
            assert_eq!(px[3], t);
            let n = f64::from(((255 - t) >> 1) + 64);
            let p = f64::from(INV_255) * n;
            let want = |sl: f32, gb: f32| {
                pack_channel(f64::from((f64::from(sl) * p + f64::from(gb)) as f32))
            };
            assert_eq!(px[0], want(f.slope[0], f.gbase[0]), "cell {g}");
            assert_eq!(px[1], want(f.slope[1], f.gbase[1]));
            assert_eq!(px[2], want(f.slope[2], f.gbase[2]));
        }
        let lit = frame();
        let mut kl = CloudKernel::default();
        kl.rebuild(1.0, &lit);
        let brighter = kl
            .rgba()
            .iter()
            .zip(k.rgba())
            .filter(|(a, b)| a[0] > b[0])
            .count();
        assert!(brighter > 0, "the glow never fired");
        assert!(kl.rgba().iter().zip(k.rgba()).all(|(a, b)| a[0] >= b[0]));
    }

    #[test]
    fn holes_copy_the_left_neighbour_rgb() {
        let mut k = CloudKernel::default();
        k.rebuild(0.6, &frame());
        let g = k.tile().iter().position(|&t| t > 0).unwrap();
        let col = g % COLS;
        if col + 1 < COLS {
            k.tile[g + 1] = 0;
            k.recolor(&frame());
            let (a, b) = (k.rgba()[g], k.rgba()[g + 1]);
            assert_eq!([b[0], b[1], b[2], b[3]], [a[0], a[1], a[2], 0]);
        }
    }

    #[test]
    fn fisr_is_the_binary_seed() {
        assert_eq!(fisr(1.0).to_bits(), 0x3f79_97bb);
        assert!((fisr(4.0) - 0.5).abs() < 0.02);
    }

    #[test]
    fn sampler_projects_zenith_to_centre_and_horizon_to_rim() {
        let mut k = CloudKernel::default();
        k.tile.fill(0);
        let mid = COLS / 2;
        k.tile[mid * COLS + mid] = 255;
        assert_eq!(k.coverage(Vec3::new(0.0, 12.0, 0.0)), 1.0);
        // Horizontal +X clamps to the rim: col = COLS, which wraps to 0, row = mid.
        k.tile[mid * COLS] = 51;
        let r = k.coverage(Vec3::new(12.0, 0.0, 0.0));
        assert!((r - 0.2).abs() < 1e-3, "rim read {r}");
        assert_eq!(k.coverage(Vec3::new(12.0, -4.0, 0.0)), r);
    }

    #[test]
    fn moon_tent_shape() {
        assert_eq!(occ1_moon(0.5), 1.0);
        assert_eq!(occ1_moon(1.0), 0.0);
        assert!((occ1_moon(0.25) - 0.5).abs() < 1e-6);
    }
}
