//! The cloud coverage kernel — a faithful port of the reference's procedural cloud field.
//!
//! The real client maintains a scrolling 128×128 byte tile of cloud coverage (the SkyManager
//! working-set `0xce98e8`), regenerated in 32-row bands at ~10 Hz by the coverage-noise kernel
//! (`WoW.exe 0x6cffc0`): 4-octave
//! toroidal value noise (lacunarity 2, persistence 0.5) → `rawByte`, thresholded by the authored
//! Light.dbc cloud density `C` (`T = trunc((1−C)·255)`, `0x6d0970`) and shaped through a fixed
//! 256-byte tone curve (`0x6d0900`, gamma 0.96 — frozen here as [`CURVE`]). One field serves every
//! consumer: the glare occlusion `occ1` samples the same tile the visible layer renders from
//! (`0x6cfa90`).
//!
//! The noise lattice is keyed by three axes: tile row (`row_key`, advancing `freq` per row), tile
//! column (`col_key`, advancing `freq` per column), and time (the u16 `phase`, bumped once per full
//! tile wrap ≈ 0.4 s — its high byte picks the permutation slice pair, its low byte the fade
//! weight between them). High key bytes select lattice cells through the permutation table; low
//! bytes index the raised-cosine [`fade`] table as interpolation weights. Band regeneration at an
//! unchanged `phase`/`T` is idempotent (keys derive from absolute tile coordinates), so the field
//! only *moves* when the phase advances or the authored density changes — exactly the reference's
//! slow cloud drift.
//!
//! Each regen fire ends with the **color pass** (`0x6cfb00` — diffed
//! bit-exact): the coverage bytes become RGBA texels (gradient + sun-aligned glow, alpha = the
//! coverage byte), and *that image* is what the dome uploads — the reference binds its color
//! buffer zero-copy to the gx texture (`0x58ac70`). The glow's per-cell surface normal comes
//! from the octave-2 derivative leg (`[cfg+0x68]`, written by `0x6cffc0`, read by `0x6cfb00`).
//!
//! Deviations from the bytes, all in never-hit or non-visual domains (recorded in the decision
//! record): the `acos` argument is clamped to ±1 (the reference NaNs above ~70° elevation, a
//! domain its sun/moon never reach); LUT reads wrap toroidally instead of running off the flat
//! heap at the measure-zero `u == 1.0` edge; the gradient table uses the reference's MSVC-LCG
//! *formula* with a fixed seed (the reference seed is process-random and not visually
//! load-bearing — any 256 uniform values in [−1, 1] are equivalent, `0x6d0c90`);
//! and the color buffer seeds alpha-0 instead of `0xFFFFFFFF` (no white flash before the first
//! build).

use bevy::math::Vec3;

use super::tables::{fade_table, gradient_table, CURVE, PERM};

/// Tile side at `SkyCloudLOD 0` (`cols = 128 << LOD`; the CVar clamps to [0,1] and defaults to 0 —
/// `0x6d1d60`). We implement LOD 0.
pub const COLS: usize = 128;
/// `log2(COLS)` — the row-pitch shift the sampler uses (`[cfg+0x20]`).
pub const SHIFT: u32 = 7;
/// Rows regenerated per fire (`[cfg+0x14]`, default 32).
pub const ROWS_PER_TICK: usize = 32;
/// Octave count (`[cfg+0x28]`, constant 4).
pub const OCTAVES: usize = 4;
/// Regen countdown reset (`0x8115b4` = 0.1 s) — the ~10 Hz cadence.
pub const REGEN_PERIOD: f32 = 0.1;
/// Per-octave lattice frequencies, LOD 0 row of the base table `0x86f3dc` (`(16 >> LOD) << oct`).
const BASE_FREQ: [u16; OCTAVES] = [16, 32, 64, 128];

#[inline]
fn perm(i: u32) -> u32 {
    u32::from(PERM[(i & 0xff) as usize])
}

/// Per-octave lattice walk state — the 0x54-byte stack record of `0x6cffc0`, kept as named fields.
struct Octave {
    /// Lattice frequency (`base_table[LOD·5 + oct]`; the record's `B+8`/`B+0xa` key delta).
    freq: u16,
    /// Row axis key (`B+4`): starts at `scroll·freq`, advances `freq` per row. High byte = row
    /// lattice cell, low byte = row fade index.
    row_key: u16,
    /// Column axis key (`B+2`): re-seeded to `phase` each row, advances `freq` per column. High
    /// byte = column lattice cell (`uVar14`), low byte = column fade index.
    col_key: u16,
    /// Octave amplitude `1 / 2^oct` (`B+0x1a`).
    amp: f32,
    /// Per-row lattice corner seeds (`B+0x1e..0x2a`): row cell hashed through the phase-selected
    /// permutation slices (`x*` = current time slice, `y*` = next).
    x0: u32,
    x1: u32,
    y0: u32,
    y1: u32,
    /// Cached corner gradients + deltas for the current column cell (`B+0x2e..0x4a`), rebuilt
    /// lazily when the column cell changes (`B+0x4c` cache key).
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

/// The cloud coverage field: the byte tile, the colored texture, and the regen state.
pub struct CloudKernel {
    /// The coverage byte tile (`[cfg+0x44]`), `COLS²` row-major. `byte/255 = R ∈ [0,1]`.
    tile: Vec<u8>,
    /// The float accumulation tile (`[cfg+0x50]`).
    accum: Vec<f32>,
    /// The per-cell shape derivative pairs (`[cfg+0x68]`, 2 f32/cell) — written by the octave-2
    /// leg, consumed by the color pass as the glow surface normal `S = (dx, dy, 1)`
    /// (`0x6cffc0` writes it, `0x6cfb00` reads it).
    deriv: Vec<[f32; 2]>,
    /// The previous-row scratch (`[cfg+0x5c]`, `COLS` f32) feeding the row derivative.
    prevrow: Vec<f32>,
    /// The colored RGBA texels (`[cfg+0x38]`, 4 B/cell: gradient+glow RGB, alpha = the coverage
    /// byte) — **this is what the reference uploads** (`0x58ac70` binds `[cfg+0x38]` zero-copy).
    rgba: Vec<[u8; 4]>,
    /// Scroll position — the tile row the next band starts at (`[cfg+0x18]`).
    scroll: usize,
    /// Noise-space phase (`[cfg+0xa0]`, u16) — +1 per full tile wrap; the time axis.
    phase: u16,
    /// Regen countdown (`[cfg+0xb0]`); starts expired so the first tick fires.
    countdown: f32,
    /// The gradient table (`0xce92d8`) — 256 × f32 in [−1, 1], built once (see [`gradient_table`]).
    gradient: [f32; 256],
    /// The fade table (`0xce8dd8`) — `0.5·(1 − cos(iπ/256))`, built once.
    fade: [f32; 256],
}

/// Per-fire inputs to the color pass — `0x6cfb00`'s per-frame setup, resolved by the caller from
/// [`crate::lighting::WowLighting`]: the three Light.dbc cloud palette rows, the weather blend,
/// and the glow body (sun by day / moon by night, with its 8-key day envelope).
#[derive(Clone, Copy, PartialEq)]
pub struct CloudFrame {
    /// Sun-glow palette (IntBand sub-10), sRGB 0..1.
    pub(crate) sun: [f32; 3],
    /// Gradient slope (IntBand sub-11).
    pub(crate) slope: [f32; 3],
    /// Gradient base (IntBand sub-12).
    pub(crate) gbase: [f32; 3],
    /// The weather storm blend `bcc` — feeds the glow z-bias (`bcc·192 + 64`) and the dim
    /// (`1 − 0.75·bcc`).
    pub(crate) bcc: f32,
    /// Camera→body direction of the glow body (Bevy frame).
    pub(crate) glow_dir: Vec3,
    /// The glow day-envelope factor (`0xce9ab8` track — `daynight::cloud_glow_track`).
    pub(crate) glow_track: f32,
}

impl Default for CloudKernel {
    fn default() -> Self {
        CloudKernel {
            tile: vec![0; COLS * COLS],
            accum: vec![0.0; COLS * COLS],
            deriv: vec![[0.0; 2]; COLS * COLS],
            prevrow: vec![0.0; COLS],
            // The reference inits the color buffer 0xFFFFFFFF; we keep alpha 0 until the first
            // build so an unprimed dome can never flash white (cosmetic-only deviation).
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
    /// Advance the countdown and regenerate one 32-row band when it expires (`0x6cffc0`'s
    /// self-throttle: fire when the decremented countdown ≤ 0, reset to 0.1). Returns whether the
    /// tile changed.
    pub fn tick(&mut self, dt: f32, density: f32, frame: &CloudFrame) -> bool {
        self.countdown -= dt;
        if self.countdown > 0.0 {
            return false;
        }
        self.countdown = REGEN_PERIOD;
        self.regen(density, ROWS_PER_TICK, frame);
        true
    }

    /// Full-tile rebuild (`0x6cff90`): regenerate every row at once — the reference runs this on
    /// discontinuities (init / zone / LOD change) by forcing the row count to `cols`.
    pub fn rebuild(&mut self, density: f32, frame: &CloudFrame) {
        self.scroll = 0;
        self.regen(density, COLS, frame);
        self.countdown = REGEN_PERIOD;
    }

    /// Re-run the color pass over the whole tile without touching the coverage — the frozen
    /// capture clock's path when only the palette/sun/weather inputs moved.
    pub fn recolor(&mut self, frame: &CloudFrame) {
        self.color_band(0, COLS, frame);
    }

    /// One regeneration fire: `rows` tile rows starting at `self.scroll` (noise + quantize +
    /// the color pass, the reference's `0x6cffc0` → `0x6cfb00` order), then the scroll/phase
    /// advance. The body is the `0x6cffc0` transcription in named-field form.
    fn regen(&mut self, density: f32, rows: usize, frame: &CloudFrame) {
        // Threshold refresh (`0x6d0970`, called at the top of every fire): T = trunc((1−C)·255).
        // The reference does not clamp C; authored bands stay in [0,1] and we clamp for safety.
        let threshold = ((1.0 - density.clamp(0.0, 1.0)) * 255.0) as i32;

        // Phase-selected permutation slice pair: the time axis. `seed` = phase HIGH byte.
        let seed = u32::from(self.phase >> 8);
        let slice_a = perm(seed); // local_18 — current time slice
        let slice_b = perm(seed + 1); // local_1c — next time slice
        let fade_t = f64::from(self.fade[(self.phase & 0xff) as usize]); // f_b — time fraction

        let scroll = self.scroll;
        let mut oct: Vec<Octave> = (0..OCTAVES)
            .map(|c| {
                let freq = BASE_FREQ[c];
                Octave {
                    freq,
                    // B+4 init: `(u16)(tile_x · key)` — absolute-row keyed, which is what makes
                    // band regeneration idempotent at a fixed phase.
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

        // Float-tile zero-clear over the band.
        for v in &mut self.accum[scroll * COLS..(scroll + rows).min(COLS) * COLS] {
            *v = 0.0;
        }

        for row in 0..rows {
            let base = (scroll + row) * COLS;
            // Per-row lattice setup: hash the row cell (row_key high byte) through the two time
            // slices; re-seed the column walk from the phase; invalidate the corner cache.
            for o in oct.iter_mut() {
                let bv = u32::from(o.row_key >> 8);
                o.x0 = perm(slice_a + bv);
                o.x1 = perm(slice_a + bv + 1);
                o.y0 = perm(bv + slice_b);
                o.y1 = perm(bv + 1 + slice_b);
                o.col_key = self.phase;
                o.cached = u32::MAX;
            }
            // fVar4 — the previous column's third-octave partial sum (reset per row: the
            // `fld 0.0` before the column loop).
            let mut prev_accum = 0.0f32;
            for col in 0..COLS {
                let cell = base + col;
                for (oi, o) in oct.iter_mut().enumerate() {
                    let fade_row = f64::from(self.fade[(o.row_key & 0xff) as usize]); // f_a
                    let cz = u32::from(o.col_key >> 8); // uVar14 — column lattice cell
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
                    // The fade-interpolated bilinear + time lerp, mirroring the binary's f64
                    // in-register chain with its one f32 round-trip (`v8`).
                    let fx = f64::from(self.fade[(o.col_key & 0xff) as usize]);
                    let v7 = fx * f64::from(o.g00d) + f64::from(o.g00);
                    let v8 = f64::from((fx * f64::from(o.g01d) + f64::from(o.g01)) as f32);
                    let v7b = (fx * f64::from(o.g10d) + f64::from(o.g10) - v7) * fade_row + v7;
                    let v8b = (fx * f64::from(o.g11d) + f64::from(o.g11) - v8) * fade_row + v8;
                    o.col_key = o.col_key.wrapping_add(o.freq);
                    let acc = f64::from(self.accum[cell]);
                    let stored = (((v8b - v7b) * fade_t + v7b) * f64::from(o.amp) + acc) as f32;
                    self.accum[cell] = stored;
                    // The octave-2 derivative leg (`0x6cffc0`, the `local_10 == 2` branch): the
                    // column/row slopes of the three-octave partial sum, into the pair buffer the
                    // color pass reads as the glow surface normal. `scale = 1 << (shift − 7)` — 1
                    // at LOD 0.
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
            // Per-row key advance: walks the row axis one `freq` step.
            for o in oct.iter_mut() {
                o.row_key = o.row_key.wrapping_add(o.freq);
            }
        }

        // Palette-quantize the band into the byte tile: the binary's float-bits pack
        // (`fmul 64; fadd 128; fadd 512; fstp` then bits `>> 14`), threshold, tone curve.
        for cell in scroll * COLS..(scroll + rows).min(COLS) * COLS {
            let q = ((f64::from(self.accum[cell]) * 64.0 + 128.0 + 512.0) as f32).to_bits();
            let idx = ((q >> 14) & 0xff) as i32 - threshold;
            self.tile[cell] = if idx >= 0 { CURVE[idx as usize] } else { 0 };
        }

        // The color pass over the same band (`0x6cfb00`, called before the scroll advance).
        self.color_band(scroll, rows, frame);

        // Scroll advance with wrap: the wrap bumps the noise-space phase — the time axis moves.
        self.scroll += rows;
        if self.scroll >= COLS {
            self.phase = self.phase.wrapping_add(1);
            self.scroll = 0;
        }
    }

    /// The color pass — the exact `0x6cfb00` per-cell algorithm (diffed
    /// bit-exact): per cell `t` = the coverage byte — `t == 0` copies the previous
    /// cell's RGB with alpha 0 (the filtering-friendly hole fill); else the gradient
    /// `slope·p + gbase` with `p = (((255−t)>>1) + 64)/255`, plus the sun-aligned glow
    /// `sun·(cosθ·intensity)` where `cosθ` aligns the cell→body vector (tile-cell units, z =
    /// `bcc·192 + 64`) against the cell's shape normal `(dx, dy, 1)` through the binary's integer
    /// fast-inverse-sqrt. Channels clamp at 1 and pack `floor(ch·255)`; alpha = `t`.
    fn color_band(&mut self, start: usize, rows: usize, frame: &CloudFrame) {
        let f = f64::from;
        let z_bias = (f(frame.bcc) * 192.0 + 64.0) as f32;
        let intensity = (f(frame.glow_track) * (1.0 - f(frame.bcc) * 0.75)) as f32;
        let body = body_cells(frame.glow_dir);
        for row in start..(start + rows).min(COLS) {
            let row_base = row as f32; // [ebp-0x84] = f18 + row — the absolute tile row
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
                // The angle byte: n = (((255 − t) >> 1) + 0x40) & 0xff ∈ [64, 191].
                let n = u32::from((255u8.wrapping_sub(t) >> 1).wrapping_add(0x40));
                let p = f(INV_255) * f64::from(n);
                let mut ch = [
                    (f(frame.slope[0]) * p + f(frame.gbase[0])) as f32,
                    (f(frame.slope[1]) * p + f(frame.gbase[1])) as f32,
                    (f(frame.slope[2]) * p + f(frame.gbase[2])) as f32,
                ];
                if let Some((su, sv)) = body {
                    // V = (Su − col, Sv − row, z_bias); S = (dx, dy, 1). Accumulation and
                    // product order per the bytes (±1-ulp load-bearing in the diff).
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

    /// Sample the coverage `R ∈ [0,1]` toward a camera-relative offset `d` (Bevy frame, +Y up) —
    /// the reference's `FUN_006cfa90`: project onto the tile, read the byte. `d` is the
    /// un-normalized body offset — the glare samples at its 12-unit sky point, so the zenith
    /// shift contributes `cos45°/|d|`.
    pub fn coverage(&self, d: Vec3) -> f32 {
        let Some((u, v)) = project_cells(d) else {
            return f32::from(self.tile[(COLS / 2) * COLS + COLS / 2]) / 255.0;
        };
        let (col, row) = (u as i32, v as i32); // _ftol
                                               // Toroidal mask (the reference reads the flat heap unchecked; `u == 1.0` is measure-zero).
        let cell = ((row as usize & (COLS - 1)) << SHIFT) + (col as usize & (COLS - 1));
        f32::from(self.tile[cell]) / 255.0
    }

    /// The colored RGBA texels for the visible-layer texture upload (`0x58ac70`'s zero-copy bind).
    pub fn rgba(&self) -> &[[u8; 4]] {
        &self.rgba
    }

    /// Raw coverage bytes (tests).
    #[cfg(test)]
    pub(crate) fn tile(&self) -> &[u8] {
        &self.tile
    }

    #[cfg(test)]
    pub(crate) fn set_phase(&mut self, phase: u16) {
        self.phase = phase;
    }
}

/// The azimuthal tile projection (`FUN_006cf870`): camera-relative offset → fractional tile cell
/// `(col, row)`. LUT centre = zenith, radius grows with the angle off a `+cos(π/4)`-shifted
/// zenith axis, saturating at 45° (the rim; below-horizon directions clamp there). `None` for a
/// degenerate zero offset.
fn project_cells(d: Vec3) -> Option<(f32, f32)> {
    let len = f64::from(d.length());
    if len < 1e-6 {
        return None;
    }
    let quarter_pi = f64::from(std::f32::consts::FRAC_PI_4);
    let c = f64::from(d.y) + f64::from(std::f32::consts::FRAC_PI_4.cos());
    // The reference feeds `c/len` to acos unclamped and NaNs above ~70° elevation — a domain
    // its bodies never reach; we clamp (recorded deviation).
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

/// The glow body's tile cell (`0x6cfb00` setup, steps 1–2): intersect the camera→body ray with
/// the unit sky dome (`0x6cf9c0` — the `−cos(π/4)`-shifted sphere, larger
/// quadratic root) and project the hit point onto the tile. `None` on the degenerate no-root /
/// zero-direction case.
fn body_cells(dir: Vec3) -> Option<(f32, f32)> {
    let f = f64::from;
    // k = −cos(0.25·π), the quarter-arc constant both functions open with.
    let k = -(f(0.25f32) * f(std::f32::consts::PI)).cos();
    let a = (f(dir.x) * f(dir.x) + f(dir.y) * f(dir.y) + f(dir.z) * f(dir.z)) as f32;
    let b = {
        // `fchs; fmul v.z; fadd st,st` — (−k·up) doubled (the reference's z-up ↔ our y-up).
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

/// The cmath quadratic solver (`0x454f40`): larger root of `a·t² + b·t + c = 0`, Vieta-stable
/// exactly as the binary rounds it; `None` on the no-root leg (`b² ≤ 4ac` or unordered).
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

/// `1/255` as the binary stores it (`0x8026c8` = `0x3b808081`).
const INV_255: f32 = f32::from_bits(0x3b80_8081);

/// The binary's integer fast-inverse-sqrt leaf (`0x456330`): a one-shot seed, NO Newton step —
/// deterministic bit-manipulation, so ported verbatim (the approximation error is part of the
/// reference's glow shape).
fn fisr(x: f32) -> f32 {
    f32::from_bits(0x5f39_97bbu32.wrapping_sub((x.to_bits() >> 1) & 0x3fff_ffff))
}

/// Pack one color channel to a byte exactly as the binary does (`0x6cfef6..0x6cff44`): clamp
/// `≤ 1.0` (no lower clamp), then the `bits(ch·255 + 512) >> 14` trick — `floor(ch·255)` after
/// f32 rounding.
fn pack_channel(ch: f64) -> u8 {
    let clamped = if ch < 1.0 { ch } else { 1.0 };
    (((clamped * 255.0 + 512.0) as f32).to_bits() >> 14) as u8
}

/// Sun glare cloud occlusion (`occ1_sun = 1 − R`, `0x6cf7b0`): a cloud over the sun dims the
/// flare linearly with coverage.
pub fn occ1_sun(r: f32) -> f32 {
    1.0 - r
}

/// Moon glare cloud occlusion — the tent `1 − |2(R − 0.5)|` (`0x6cf7d0`): zero at R=0 *and* R=1.
/// The reference's moon halo is a thin-cloud effect — off in a perfectly clear patch of sky,
/// blooming when a wisp crosses the moon.
pub fn occ1_moon(r: f32) -> f32 {
    1.0 - (2.0 * (r - 0.5)).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A daytime frame: mid-grey slope over a dark base, warm glow, clear weather, the sun high
    /// in the +X sky at full envelope.
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

    /// Clear sky (C=0 ⇒ T=255): every cell quantizes below the threshold ⇒ R = 0 everywhere ⇒
    /// occ1_sun = 1 (full flare), occ1_moon = 0 (no halo). The colored
    /// texels are all alpha 0 (the t==0 hole fill).
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

    /// Full density (C=1 ⇒ T=0): every raw byte clears the threshold, so the whole sky reads as
    /// heavy cover through the tone curve's high region — a storm's overcast field, not a
    /// scattered one. Regeneration is deterministic. Intermediate density (C=0.6, the reference's
    /// own init threshold) cuts the threshold through the noise distribution: a real mix of
    /// clear (0) and covered cells — the scattered-cloud texture.
    #[test]
    fn density_shapes_the_field_and_regen_is_deterministic() {
        let mut a = CloudKernel::default();
        let mut b = CloudKernel::default();
        a.rebuild(1.0, &frame());
        b.rebuild(1.0, &frame());
        assert_eq!(a.tile(), b.tile());
        assert_eq!(a.rgba(), b.rgba());
        // Overcast: no cell clears (the noise's whole range sits above T=0), and the curve pushes
        // the bulk high (measured mean ≈ 242, min 135 — the accumulator tail through the toe).
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

    /// Band regeneration is keyed by absolute tile coordinates: four 32-row ticks at a fixed
    /// phase reproduce exactly the tile a full rebuild computes at that phase. This pins the
    /// toroidal walk (row_key seeding, per-row advance, quantize windows) against the reference
    /// structure — a wrong scroll seed or window shears the field between bands. The colored
    /// texels agree from row 1 (row 0's row-derivative reads the persistent prev-row scratch,
    /// which legitimately differs between a fresh full pass and a scrolled one — the reference's
    /// own post-rebuild wart, gone by the next wrap).
    #[test]
    fn incremental_bands_tile_the_full_field() {
        let mut inc = CloudKernel::default();
        inc.rebuild(0.6, &frame()); // ends with phase bumped to 1, scroll 0
        for _ in 0..4 {
            inc.tick(1.0, 0.6, &frame()); // four band fires cover the whole tile at phase 1
        }
        let mut full = CloudKernel::default();
        full.set_phase(1);
        full.rebuild(0.6, &frame());
        assert_eq!(inc.tile(), full.tile());
        assert_eq!(inc.rgba()[COLS..], full.rgba()[COLS..]);
    }

    /// The color pass (`0x6cfb00`): a covered cell away from the glow carries the pure gradient
    /// bytes (`floor((slope·p + gbase)·255)`, `p = (((255−t)>>1)+64)/255`) with alpha = t; a
    /// cell whose body-alignment is positive gains the sun term; a hole copies its left
    /// neighbour's RGB at alpha 0.
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
        // Glow on: texels toward the sun brighten, none darken below the gradient base.
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

    /// The hole fill: force a hole next to a covered cell and recolor — the hole copies its left
    /// neighbour's RGB with alpha 0 (the reference's filtering-friendly early-out).
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

    /// The integer fast-inverse-sqrt seed (`0x456330`): the exact bit formula, ~3% accurate.
    #[test]
    fn fisr_is_the_binary_seed() {
        assert_eq!(fisr(1.0).to_bits(), 0x3f79_97bb);
        assert!((fisr(4.0) - 0.5).abs() < 0.02);
    }

    /// The azimuthal projection: zenith reads the tile centre; a low direction lands on the rim
    /// ring at its azimuth; below-horizon clamps to the same rim radius.
    #[test]
    fn sampler_projects_zenith_to_centre_and_horizon_to_rim() {
        let mut k = CloudKernel::default();
        k.tile.fill(0);
        let mid = COLS / 2;
        k.tile[mid * COLS + mid] = 255;
        // Straight up: phase 0 ⇒ the centre cell.
        assert_eq!(k.coverage(Vec3::new(0.0, 12.0, 0.0)), 1.0);
        // A horizontal +X direction: phase clamps to 0.5 ⇒ col = COLS (wraps to 0), row = mid.
        k.tile[mid * COLS] = 51;
        let r = k.coverage(Vec3::new(12.0, 0.0, 0.0));
        assert!((r - 0.2).abs() < 1e-3, "rim read {r}");
        // Below the horizon: same clamp, same cell.
        assert_eq!(k.coverage(Vec3::new(12.0, -4.0, 0.0)), r);
    }

    /// The moon tent peaks at half coverage and vanishes at both extremes (`0x6cf7d0`).
    #[test]
    fn moon_tent_shape() {
        assert_eq!(occ1_moon(0.5), 1.0);
        assert_eq!(occ1_moon(1.0), 0.0);
        assert!((occ1_moon(0.25) - 0.5).abs() < 1e-6);
    }
}
