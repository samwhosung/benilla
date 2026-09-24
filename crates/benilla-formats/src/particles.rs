//! Vanilla (build 5875 / MD20 v256) M2 **particle emitter** parsing — read straight from raw bytes.
//!
//! `benilla-m2` parses only the render path and deliberately skips the cosmetic chunks, so the
//! particle emitters are read here. The post-vanilla `M2ParticleEmitter` (with `texture_file_data_ids`,
//! `enable_encryption`, WoD flag bits) does not match 1.12; the vanilla layout is a fixed
//! **0x1f8-byte** record. The byte layout is the reference's (`FUN_0070ebd0` @ `0x70ebd0`) and
//! **difftested** here against `ElwynnCampfire.m2` (see `tests/`):
//!
//! ```text
//! header array : count @ MD20+0x13c, ptr @ MD20+0x140         record stride 0x1f8
//! +0x04 flags(u32)  +0x08 position(C3Vector)  +0x14 bone(u16)  +0x16 texture(u16, index→M2 textures)
//! +0x28 blendingType(u8)  +0x2a emitterType(u16:1=Plane,2=Sphere,3=Spline)
//! +0x2c particleType(u16:0=head,1=tail,2=both)  +0x30 tileRows(u16)  +0x32 tileCols(u16)
//! +0x34.. ten emission M2Tracks (vanilla 28-byte track; the runtime uses value[0] of each):
//!   +0x34 emissionSpeed  +0x50 speedVariation  +0x6c verticalRange  +0x88 horizontalRange
//!   +0xa4 gravity  +0xc0 lifespan  +0xdc emissionRate  +0xf8 areaLength  +0x114 areaWidth
//!   +0x130 zSource
//! +0x14c.. baked over-life color/opacity/scale/cell ramps (see `OverLife`).
//! +0x17c tailTime(f32, SECONDS — the tail streak's length is velocity·tailTime)
//! +0x180 twinkleSpeed  +0x184 twinklePercent  +0x188/+0x18c twinkleScale{min,max}
//! +0x190 inheritScale(f32)  +0x194 drag(f32, plain scalar — the velocity decay)
//! +0x198 spin(f32, rad/s quad rotation)
//! +0x1c4..+0x1d0 followSpeed1/followScale1/followSpeed2/followScale2 (the follow-delta line)
//! +0x1dc enabled(M2Track<u8>, step) — the emission ON/OFF gate, closing the record at 0x1f8.
//! ```
//!
//! The two keyed tracks here (emission rate, enabled) bake **one loop per sequence** into
//! [`EmitTiming`] through the same key-search kernel (`0x713d50`) as the material alpha
//! (`models::key_anim`): the reference samples both per frame through the
//! *playing* sequence's own key window (`0x713d50`), so what an emitter does is a function of
//! which sequence its model instance is playing. A quest GameObject authors its
//! explosion inside one-shot clips and an OFF window in every idle sequence; baking only
//! sequence 0's band parked that choreography at its end value forever (bug B27).
//!
//! The record tail (+0x180..) is the reference's own map: the loader `0x70ebd0` remaps
//! file+0x188/+0x18c → runtime twinkleScale min/max
//! (delta = max − min at rt+0x1c0), NOT a size multiplier — see [`ParticleEmitterDef::twinkle`].

use std::io::Cursor;

use anyhow::Result;
use benilla_m2::{parse_m2, M2ScalarTrack};

use crate::emit_timing::{EmitParams, EmitTiming};
use crate::models::SeqSlot;

/// Emitter spawn shape (file `emitterType` @ +0x2a). Only `Plane` is needed for campfires/torches;
/// `Sphere` (e.g. dust clouds) and `Spline` follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticleShape {
    Plane,
    Sphere,
    Spline,
}

/// How a particle batch blends (file `blendingType` @ +0x28 = the M2/EGxBlend enum). Additive is the
/// flame/glow case (the campfire is `4`); mod/mod2x still fold to alpha (rare for the props we
/// render, and they will get their own path when a model needs them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticleBlend {
    /// `3` NoAlphaAdd / `4` Add — `(SRC_ALPHA, ONE)`. Flames, glows, embers. No depth write/sort.
    Add,
    /// `2` Alpha — `(SRC_ALPHA, ONE_MINUS_SRC_ALPHA)`. Smoke.
    Alpha,
    /// `1` AlphaKey — blending **OFF**, depth-write ON, and the fixed-function **alpha test**
    /// `GEQUAL 224/255` carving the texture's silhouette out of the quad. The chip/debris family:
    /// solid little rocks, bone shards, ice splinters, each authored as a shape on a
    /// fully-transparent field.
    ///
    /// The steps (see [`ParticleBlend::Opaque`] for the shared half):
    /// the emitter's own blend field decides its list at `0x7085db` (`cmp word[rec+0x28],1; ja` —
    /// mode ≤ 1 **and** instance alpha ≥ 0.99999 lands in the `+0x4c` **opaque** list, i.e.
    /// pass 0), pass 0 takes promotion-table row 0 `0x811fe0 = {0,1,2,10,3,4,5}` so mode 1 stays
    /// **EGxBlend 1**, EGxBlend 0/1 → `glDisable(GL_BLEND)` (`0x59d563`), and `0x70c256` sets the
    /// ref from the RAW mode — `round(instanceAlpha × 224.0)`, `[0x812034] = 224.0f` — never from
    /// the pass. Depth-write stays ON because the synthetic material only sets its z-write-off bit
    /// for `blendMode > 1` (`0x70d8f1`).
    ///
    /// Folding this into `Opaque` — no discard at all — paints the transparent field solid: the
    /// Lesser Rock Elemental's debris (`PARTROCK.BLP`, 31 % of its texels at α = 0) came out as
    /// flat pale squares instead of tumbling chips.
    AlphaKey,
    /// `0` Opaque — blending OFF, depth-write ON, and **no** alpha test (`0x70c237`'s mode-0 arm
    /// sets ref 0, which `0x59d5b9` turns into `glDisable(GL_ALPHA_TEST)`). Genuinely rare for
    /// particles; the fallback for an out-of-range blend byte too.
    Opaque,
}

/// One segment's integer **flipbook cell ramp** — the authored `(begin, end)` pair and the
/// `(base, span)` the reference derives from it at load (`0x7b9da0` head / `0x7b9de0` tail, both
/// pure-integer).
///
/// The two build arms are deliberately **asymmetric**, and that asymmetry is the whole mechanism:
/// the ramp covers `N = |end − begin| + 1` cells, each getting `1/N` of the segment, travelling in
/// the **authored direction**.
///
/// ```text
/// end >= begin : base = begin,      span = end − begin + 1   (forward)
/// end <  begin : base = begin + 1,  span = end − begin − 1   (NEGATIVE — plays backwards)
/// ```
///
/// **A decreasing pair is legal, shipped, and means "run the flipbook in reverse"** — four models
/// author one (`DwarvenBrazier01`'s settling flame, `ShadowWordSilence_Breath`'s 32-frame reverse
/// sweep). There is no swap and no clamp anywhere on the reference's path; an earlier reading here
/// clamped into `[begin, end]`, which both mangled the reverse ramps and panicked outright on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRamp {
    /// The authored pair, verbatim (the reference archives it at `rec+0x3c/+0x40` and never reads
    /// it back — kept here because it is what a dump should show).
    pub begin: u16,
    pub end: u16,
    base: i32,
    span: i32,
}

impl CellRamp {
    /// Build from an authored `(begin, end)` pair — `0x7b9da0`'s two arms, verbatim.
    pub fn new(begin: u16, end: u16) -> Self {
        let (b, e) = (i32::from(begin), i32::from(end));
        let (base, span) = if e >= b {
            (b, e - b + 1)
        } else {
            (b + 1, e - b - 1)
        };
        Self {
            begin,
            end,
            base,
            span,
        }
    }

    /// The cell index at segment fraction `t` — **`t` must already carry the endpoint inset** (and
    /// the repeat wrap, if any); see [`OverLife::sample`].
    ///
    /// `floor(base + span·t) & 0xFF`. The mask is the reference's mod-256 wrap (its fast-floor
    /// idiom `bits(x + 512.0) >> 14` yields a byte), **not** a clamp into the authored range or the
    /// atlas — bounding the index is the atlas walk's business, and it only masks the column.
    pub fn sample(&self, t: f32) -> u16 {
        // Saturating float→int (NaN → 0) rather than the reference's raw mantissa read: a `mid` of
        // 0 or 1 makes the reference's own segment span 1/0 and walks NaN into this floor, and we
        // will not reproduce a fault. No shipped emitter authors that mid (corpus-swept).
        let v = self.base as f32 + self.span as f32 * t;
        ((v.floor() as i32) & 0xFF) as u16
    }
}

/// The baked **over-life** ramps (record tail, +0x14c..) sampled by a particle's normalized age `u =
/// age/lifespan`. Colour and size are **3-key ramps** evaluated as two linear segments split at
/// [`Self::mid`]: key0→key1 over `u∈[0,mid]`, then key1→key2 over `u∈[mid,1]`. The flipbook cells
/// are a different shape — a `(begin, end)` [`CellRamp`] **per segment**, and there are two
/// independent sets of them (head quad, tail streak). Byte layout + math are the reference's
/// (`FUN_0070ebd0` builder, `FUN_007b9b10` evaluator) and difftested
/// against `ElwynnCampfire.m2`. These drive the believable flame fade / grow / flicker.
#[derive(Debug, Clone, Copy)]
pub struct OverLife {
    /// Normalized age (`+0x14c`) splitting the two ramp segments. The reference's split is
    /// **inclusive toward segment A** (`age > lifespan·mid` is the only way into B).
    pub mid: f32,
    /// RGBA keys k0/k1/k2 (`+0x150/+0x154/+0x158`, packed BGRA u32 → linear 0..1). The **A** channel
    /// is the particle's additive weight (its over-life opacity); there is no separate alpha array.
    pub color: [[f32; 4]; 3],
    /// Uniform particle size keys k0/k1/k2 (`+0x15c/+0x160/+0x164`, yards).
    pub scale: [f32; 3],
    /// The **head** quad's flipbook ramp, per segment: A (`+0x168/+0x16a`), B (`+0x16e/+0x170`).
    pub head_cells: [CellRamp; 2],
    /// The **tail** streak's own, independent flipbook ramp: A (`+0x174/+0x176`), B
    /// (`+0x178/+0x17a`). The reference emits two cell indices per particle and the tail quad reads
    /// this one — a model whose tail ramp differs from its head ramp animates the streak
    /// separately (the tail quad's read, `0x7b304e`).
    pub tail_cells: [CellRamp; 2],
    /// Per-segment flipbook **repeat count** (`+0x16c`/`+0x172`, `fild`-converted at load). `1.0`
    /// (the ctor default, and what nearly all content authors) is one pass; anything else cycles
    /// the cell ramp `fmod(t·repeat, 1.0)` times across the segment. Colour and size do not cycle.
    pub repeat: [f32; 2],
}

/// One evaluation of the over-life ramps — the reference's `0x7b9b10` writes exactly these four
/// outputs (colour, **two** cell indices, size) from one call.
#[derive(Debug, Clone, Copy)]
pub struct OverLifeSample {
    /// Linear RGBA; the A channel is the particle's over-life opacity/additive weight.
    pub color: [f32; 4],
    /// Half-extent in yards.
    pub size: f32,
    /// Atlas cell for the head quad.
    pub head_cell: u16,
    /// Atlas cell for the tail streak (independent of [`Self::head_cell`]).
    pub tail_cell: u16,
}

impl OverLife {
    /// Sample every ramp at normalized age `u` (0..1).
    pub fn sample(&self, u: f32) -> OverLifeSample {
        let u = u.clamp(0.0, 1.0);
        // A `mid` of 0 divides by zero in the reference (`0x7b9cf0`'s `fdivr`: it walks NaN into
        // the sampler); no shipped emitter authors one, and we refuse to reproduce the fault.
        let mid = self.mid.clamp(1e-3, 1.0);
        let (k0, k1, t, seg) = if u <= mid {
            (0, 1, u / mid, 0)
        } else {
            (1, 2, (u - mid) / (1.0 - mid).max(1e-3), 1)
        };
        // **The endpoint inset, and it is the only normalized time the reference has.** `0x7b9b10`
        // computes `t·0.99 + 0.005` once, over its own `age` argument slot, and every consumer
        // reloads that slot — all four colour channels, the size, and both cell ramps (read
        // instruction by instruction). So a particle at a segment's start sits **0.5 % along** its
        // colour and size ramps, never exactly on the authored key, and 99.5 % at the end.
        //
        // For the CELLS the inset is not a rounding artefact but load-bearing: co-designed with
        // the ramp's ±1, it is the only thing making `cell(0) == begin` and `cell(1) == end` hold
        // in BOTH directions (a raw `t` floors one past the end going forward, one past the begin
        // going backward). Exact while `0.005·N < 1` — 200 cells; the largest shipped atlas is 8×8.
        let t = t.clamp(0.0, 1.0) * 0.99 + 0.005;
        let mut color = [0.0; 4];
        for (c, slot) in color.iter_mut().enumerate() {
            *slot = self.color[k0][c] + (self.color[k1][c] - self.color[k0][c]) * t;
        }
        let size = self.scale[k0] + (self.scale[k1] - self.scale[k0]) * t;
        // The repeat count cycles the FLIPBOOK ONLY — an explicit negative, and visible in
        // the reference's own ordering: colour and size are computed and stored out before the
        // `rec+0x50 != 1.0` branch is even evaluated, and the wrapped coefficient never becomes `t`
        // (`0x7b9bcc`). So a 5× swarm emitter flaps its wings five times while fading exactly once.
        let ct = if self.repeat[seg] != 1.0 {
            (t * self.repeat[seg]).fract()
        } else {
            t
        };
        OverLifeSample {
            color,
            size,
            head_cell: self.head_cells[seg].sample(ct),
            tail_cell: self.tail_cells[seg].sample(ct),
        }
    }
}

/// A SPLINE (type-3) emitter's authored curve: a flattened **cubic Bézier chain** (K segments,
/// `3K+1` control points: `P, out-tangent, in-tangent, P, …`), **arc-length parameterized** — the
/// reference computes per-segment arc lengths into a knot array at load (`0x7b9a80` →
/// `0x4532e0`; the knots are NOT on disk) and the spawn kernel `0x7b9500`'s `t ∈ [0,1]` walks the
/// chain by normalized arc length. Our
/// per-segment length is a 16-chord subdivision (the reference's `0x453e50` method is untraced —
/// any smooth-curve length approximation lands within a fraction of a percent).
#[derive(Debug, Clone)]
pub struct SplineData {
    /// Control points, model-local WoW axes (`3K+1`, from file+0x1d4/+0x1d8).
    pub points: Vec<[f32; 3]>,
    /// Cumulative normalized arc length at each segment boundary (`K+1` entries, 0 → 1).
    knots: Vec<f32>,
}

impl SplineData {
    /// Build from the raw control points (must be `3K+1`; returns `None` below one segment).
    pub fn new(points: Vec<[f32; 3]>) -> Option<Self> {
        let k = points.len().checked_sub(1)? / 3;
        if k == 0 || points.len() != 3 * k + 1 {
            return None;
        }
        let mut knots = vec![0.0f32];
        for seg in 0..k {
            let mut len = 0.0;
            let mut prev = Self::bezier(&points[3 * seg..3 * seg + 4], 0.0);
            for i in 1..=16 {
                let p = Self::bezier(&points[3 * seg..3 * seg + 4], i as f32 / 16.0);
                len += ((p[0] - prev[0]).powi(2)
                    + (p[1] - prev[1]).powi(2)
                    + (p[2] - prev[2]).powi(2))
                .sqrt();
                prev = p;
            }
            knots.push(knots[seg] + len);
        }
        let total = *knots.last().unwrap();
        if total > 0.0 {
            for kn in &mut knots {
                *kn /= total;
            }
        }
        Some(Self { points, knots })
    }

    fn bezier(p: &[[f32; 3]], u: f32) -> [f32; 3] {
        let w = [
            (1.0 - u).powi(3),
            3.0 * u * (1.0 - u).powi(2),
            3.0 * u * u * (1.0 - u),
            u.powi(3),
        ];
        std::array::from_fn(|c| (0..4).map(|i| w[i] * p[i][c]).sum())
    }

    fn bezier_deriv(p: &[[f32; 3]], u: f32) -> [f32; 3] {
        let w = [
            -3.0 * (1.0 - u).powi(2),
            3.0 * (1.0 - u) * (1.0 - 3.0 * u),
            3.0 * u * (2.0 - 3.0 * u),
            3.0 * u * u,
        ];
        std::array::from_fn(|c| (0..4).map(|i| w[i] * p[i][c]).sum())
    }

    /// Locate `(segment, local u)` for a normalized arc fraction `t`.
    fn locate(&self, t: f32) -> (usize, f32) {
        let k = self.knots.len() - 1;
        let seg = self.knots[1..k]
            .iter()
            .position(|&kn| t < kn)
            .unwrap_or(k - 1);
        let (a, b) = (self.knots[seg], self.knots[seg + 1]);
        (seg, ((t - a) / (b - a).max(1e-6)).clamp(0.0, 1.0))
    }

    /// The curve point at arc fraction `t` — first/last control point outside `[0,1]` (the
    /// reference's `0x453390` clamp legs).
    pub fn eval(&self, t: f32) -> [f32; 3] {
        if t <= 0.0 {
            return self.points[0];
        }
        if t >= 1.0 {
            return *self.points.last().unwrap();
        }
        let (seg, u) = self.locate(t);
        Self::bezier(&self.points[3 * seg..3 * seg + 4], u)
    }

    /// The (unnormalized) curve tangent at arc fraction `t` (the reference's `0x453420` leg —
    /// its caller renormalizes).
    pub fn tangent(&self, t: f32) -> [f32; 3] {
        let (seg, u) = self.locate(t.clamp(0.0, 1.0));
        Self::bezier_deriv(&self.points[3 * seg..3 * seg + 4], u)
    }
}

/// One parsed vanilla M2 particle emitter, reduced to what the renderer needs. Positions/vectors are in
/// **model-local** space (WoW axes, Z up) — compose with the instance world transform at spawn.
#[derive(Debug, Clone)]
pub struct ParticleEmitterDef {
    pub flags: u32,
    /// Emitter origin, model-local. (Bone-follow @ +0x14 is deferred — static props sit on the root.)
    pub position: [f32; 3],
    pub bone: u16,
    pub shape: ParticleShape,
    pub blend: ParticleBlend,
    /// Does the scene's light multiply this emitter's quads? See [`lit_of`] for the byte law — it
    /// needs the RAW blend field, which [`ParticleBlend`] collapses (5/6 fold to `Alpha`), so the
    /// verdict is baked here at parse rather than re-derived downstream.
    pub lit: bool,
    /// **Geometry model** (file+0x18 count / +0x1c offset, an `M2Array<char>` path, loaded by
    /// `0x7b1c80`): when it names a resolvable `.m2`, this emitter spawns tiny
    /// 3-D MODEL instances instead of billboard quads (Whirlwind's blades, Cone of Cold's
    /// shards, the cyclones). `None` when unauthored.
    pub geometry_model: Option<String>,
    /// **Recursion model** (file+0x20 / +0x24 — wired by `0x7b5dd0`, spawned per particle at
    /// `0x7b5b9f`): the named model's OWN particle emitters (capped at 4) become CHILD emitters
    /// driven once per live parent particle per frame at the particle's position (Fire Blast's
    /// impact sparks, bomb explosions). `None` when unauthored.
    pub recursion_model: Option<String>,
    /// Particle texture (`.blp` path, as embedded in the M2 textures table). `None` if unresolved.
    pub texture: Option<String>,
    /// Texture atlas size for cell animation (`1×1` = a single static cell). **Both are non-zero
    /// powers of two**: the reference's setter (`0x7b4ed0`) demands it — it derives `log2(cols)`
    /// and the two reciprocals — and on a bad pair writes *none* of its five fields, leaving the
    /// ctor's `1×1`. [`parse_m2_particle_emitters`] mirrors that fallback, so the atlas walk can
    /// mask instead of divide. No shipped emitter trips it (corpus-swept).
    pub tile_rows: u16,
    pub tile_cols: u16,
    /// 0 = head (camera quad), 1 = tail (speed-stretched), 2 = both. We render head first.
    pub head_tail: u8,
    /// The two per-frame-sampled emission tracks — spawn rate (`+0xdc`) and the ON/OFF gate
    /// (`+0x1dc`) — baked **one loop per sequence** ([`EmitTiming`]).
    pub timing: EmitTiming,
    /// The other NINE emission M2Tracks (speed, speedVar, latitude, longitude, gravity, lifespan,
    /// areaLength, areaWidth, zSource — bases `+0x34..+0x130`), baked the same way and sampled by
    /// the sim **each frame on the emitter's clock** ([`EmitParams`]). These animate for real:
    /// Frost Nova rides its emission radius 0.19 → 13.2 yd out with the expanding ring, Arcane
    /// Explosion 0 → 7.2 yd with the growing dome — the retired `value[0]` flatten birthed both
    /// entirely at the caster's feet.
    pub params: EmitParams,
    /// Velocity **drag** (file +0x194; a plain scalar, not a track — the builder copies it straight to
    /// the runtime emitter's `+0x1e0` at `0x70ffdd`, *outside* the ten track setters). Each
    /// frame the runtime applies `vel −= min(dt·drag, 1)·vel` (the integrator `0x7b2680`, after
    /// its gravity step) — exponential
    /// velocity decay that caps a particle's total travel near `speed/drag`. Load-bearing for props
    /// that author a *fast, long-lived, zero-gravity* jet and rely on drag to contain it: e.g.
    /// `CandelabraTallWall01` (speed 0.56, life 6, gravity 0, **drag 10** → a ~0.06 yd flicker; with
    /// no drag the same particle coasts 3.3 yd to the ceiling). 0 = no drag.
    pub drag: f32,
    /// **Tail time** (file +0x17c, seconds — the quad writer's tail block `0x7b3041`):
    /// a tail-mode particle (`head_tail` 1/2) renders a velocity-projected streak of world length
    /// `|velocity| · tail_time`, trailing behind the motion; file flag `0x400` additionally clamps
    /// the time to the particle's age (the streak grows from zero at birth).
    pub tail_time: f32,
    /// **Tumble** — the model particles' angular-velocity range (file +0x19c min / +0x1a8 max,
    /// C3Vectors, rad/s — rolled in the spawn `0x7b2420`): each birth rolls a body-frame
    /// spin vector. The reference's roll carries a VERIFIED original-client asymmetry — only X
    /// honors `min + u·range`; Y and Z multiply a raw `[1,2)` mantissa by their range alone
    /// (their min is loaded but never read) — which a fidelity consumer must replicate.
    pub angular_velocity_min: [f32; 3],
    pub angular_velocity_max: [f32; 3],
    /// **Emitter-motion inherit scale** (file +0x190 → runtime +0x1c4, `0x70ff71/0x70ff77`):
    /// scales the inherited velocity a flag-0x40 emitter feeds its births
    /// (see [`Self::inherits_emitter_motion`]).
    pub inherit_scale: f32,
    /// **Follow pairs** (file +0x1c4..+0x1d0 — the `0x7b5d30` setter): two authored (emitter
    /// speed → follow fraction) samples defining the follow-delta response line; see
    /// [`Self::follow_line`].
    pub follow_speed1: f32,
    pub follow_scale1: f32,
    pub follow_speed2: f32,
    pub follow_scale2: f32,
    /// **Twinkle** — the per-frame flicker modulation (file +0x180/+0x184/+0x188/+0x18c; the quad
    /// writer `0x7b2a50`). The rendered half-
    /// size is the over-life scale ramp × a **gated** twinkle multiplier:
    /// `min ≠ max ⇒ noise(speed·age)·(max − min) + min`, **skipped entirely when `min == max`**
    /// (a degenerate range — `{0,0}` and `{1,1}` alike burn steady at ramp size; the kobold candle's
    /// `{0,0}` is NOT size zero). This corrects the old `scale_base + rand·scale_variation` reading,
    /// which inflated every `{1,1}` flame 1–2× and collapsed `{0,0}` to invisible.
    pub twinkle_speed: f32,
    /// Fraction of frames the particle draws at all while twinkling (the reference's separate
    /// draw-gate; exact byte condition thin — consumers may treat `≥ 1` / `0` as always-draw).
    pub twinkle_percent: f32,
    pub twinkle_min: f32,
    pub twinkle_max: f32,
    /// SPLINE (type-3) curve data — `Some` only when the record authors a valid Bézier chain
    /// (file+0x1d4 count / +0x1d8 offset, `N = 3·⌊count/3⌋+1` C3Vectors — copied via
    /// `0x7b9a80`). For splines the generic emission fields are
    /// **repurposed** (each setter reads its track's `values[0]`): `area_length`/`area_width`
    /// = **tMin/tMax** (the spawned arc-fraction window, clamped [0,1]), `vertical_range` =
    /// the **tangent-spin range ψ** (birth velocity = +Z rotated about the local tangent by
    /// S11·ψ), `horizontal_range` = the **scatter** (a `U01·scatter` position jitter along the
    /// velocity), and the rate track's `values[0]` doubles as the load-time arcScale (a
    /// one-frame transient — the per-frame rate sample overwrites it; we always sample the
    /// track, so it needs no separate field).
    pub spline: Option<SplineData>,
    /// Quad **spin** (file +0x198 → runtime +0x18c): the billboard rotates in-plane by
    /// `angle = spin · age` (rad; fcos/fsin @ `0x7b2ddc` in the quad writer). 0 = no rotation.
    /// A **negative** spin is the author's alternating-direction switch: the writer negates the
    /// (negative) angle on half the particles (pointer-bit-5 hash, `0x7b2dda`), counter-rotating
    /// the cloud — vanilla's only rotation randomizer (the record has no phase/variance field).
    pub spin: f32,
    /// Baked over-life ramps (color/opacity/size/cell) — see [`OverLife`].
    pub over_life: OverLife,
}

impl ParticleEmitterDef {
    /// File flag `0x10` (→ runtime `0x100` — the loader's flag word is a **non-identity remap**,
    /// byte block `0x70faf8–0x70fc44`):
    /// **the storage-space choice, and so the ride-vs-trail switch** (spawn `0x7b8a9a`, draw
    /// `0x7b3ef9`). SET ⇒ the spawn stores raw emitter-LOCAL
    /// pos/vel and the draw folds the live emitter matrix back in every frame: the whole cloud
    /// rides the emitter rigidly, rotation and all (the chandelier's candle flames ride the swing;
    /// a carried torch is flagged for it). CLEAR ⇒ the spawn bakes pos/vel into WORLD and the draw
    /// never re-applies the emitter matrix: each particle **hangs where it was born**, so a moving
    /// host lays a trail `host speed × particle lifetime` long.
    ///
    /// This doc said the opposite until 1578 — "a moving model carries its flame; there is NO
    /// world-frozen trail mode" rested on a null test (a kobold's 0.2 s candle smear, too small
    /// to see); the spawn/draw pairing and a 7860-emitter corpus census refute it.
    pub fn model_space(&self) -> bool {
        self.flags & 0x10 != 0
    }
    /// File flag `0x4000` (→ runtime `0x40000` — the integrator's follow add at `0x7b2744`):
    /// live particles keep exactly **[`Self::follow_line`]'s fraction (≤ 1) of
    /// the emitter's per-frame world motion** — at saturation the trail rides the emitter
    /// rigidly, below it lags toward a world-frozen trail; it never leads. (The reference's
    /// baseline for this content class is world-frozen — its emitter motion folds into the
    /// emitter matrix — and its `+fraction·Δ` add recovers the ride; over an anchor-riding
    /// store the same observable is a `(fraction−1)·Δ` move.) The hunter missiles author it
    /// (19 emitters / 15 spell models).
    pub fn follow_emitter(&self) -> bool {
        self.flags & 0x4000 != 0
    }
    /// The follow-delta response line (the reference's load-time `0x7b5d30`): `(slope,
    /// intercept)` of the line through the two authored `(speed, fraction)` samples, so the
    /// per-frame fraction is `clamp(slope·|Δpos|/dt + intercept, 0, 1)`. `None` when the two
    /// speeds coincide (the reference zeroes both — no follow response).
    pub fn follow_line(&self) -> Option<(f32, f32)> {
        ((self.follow_speed2 - self.follow_speed1).abs() >= 1e-6).then(|| {
            let slope = (self.follow_scale2 - self.follow_scale1)
                / (self.follow_speed2 - self.follow_speed1);
            (slope, self.follow_scale1 - slope * self.follow_speed1)
        })
    }
    /// File flag `0x40` (→ runtime `0x400` — the inherit block at `0x7b53ce`):
    /// births inherit the emitter's recent motion. The emitter keeps a ~30 Hz inherit-velocity
    /// vector — at each trigger (accumulated dt > 1/30 s), `oneFrameΔ · ((1/30)/accum) ·
    /// inherit_scale`, zeroed while no particles are live — and each birth adds
    /// `(1 + S11·speed_variation) · inherit` to its velocity (the shape kernels' closing
    /// block). The enchant hands / Bloodlust / Death Wish family authors it (70 emitters / 33
    /// spell models).
    pub fn inherits_emitter_motion(&self) -> bool {
        self.flags & 0x40 != 0
    }
    /// File flag `0x20` (→ runtime `0x200`): the rendered particle size is multiplied by the
    /// emitter transform's scale magnitude. Torches/campfires author it; without it an
    /// instance-scaled prop scales its particle *positions* only.
    pub fn scale_size_by_instance(&self) -> bool {
        self.flags & 0x20 != 0
    }
    /// File flag `0x100` on a **sphere** emitter (→ runtime `0x4000`, mapped only when the type
    /// word is 2): birth velocity is straight `+Z` instead of radial through the shell point.
    pub fn sphere_up(&self) -> bool {
        self.flags & 0x100 != 0
    }
    /// File flag `0x80` on a **sphere** emitter (→ runtime `0x800` — like [`Self::sphere_up`]
    /// the loader maps it only when the type word is 2, so a plane's authored `0x80` is dead
    /// data): kill the particle the frame its motion turns AWAY from the emitter origin — the
    /// integrator's tail tests `dot(stepVelocity, updatedPos) > 0` and returns dead (`0x7b2680`,
    /// the `rt 0x800` branch). The suction containment: every corpus author pairs it with a
    /// NEGATIVE emission speed (Blink/Conjure/Detect Magic converge inward), and the kill is
    /// what stops the stream at the centre instead of letting it spray out the far side.
    pub fn kill_outbound(&self) -> bool {
        self.shape == ParticleShape::Sphere && self.flags & 0x80 != 0
    }
    /// File flag `0x400` (→ runtime `0x10000`): a tail streak's `tail_time` is clamped to the
    /// particle's age — the streak grows from zero at birth instead of popping in full-length.
    pub fn tail_clamps_to_age(&self) -> bool {
        self.flags & 0x400 != 0
    }
    /// File flag `0x200` (→ runtime `0x8000` — tested in the spawn `0x7b2420`): each
    /// tumble axis's rolled angular velocity is independently sign-flipped with probability ½
    /// (three extra draws) — the model particles' spin-direction randomizer.
    pub fn tumble_random_sign(&self) -> bool {
        self.flags & 0x200 != 0
    }
    /// File flag `0x2000` (→ runtime `0x20000`):
    /// the **at-spawn ground snap**. Once, at each birth (anchored mode only — the shape
    /// kernels call the hook `0x7b2140` inside their bit-0x100-CLEAR branch), the client
    /// probes **20 yd straight down** for terrain OR WMO/doodad geometry (`0x672b60` →
    /// `0x6aa160`, flags 0x100111); on a hit the particle's up-coordinate becomes `surfaceZ +
    /// its birth over-life SIZE` (the sampler's float leg is the scale ramp — `0x7b9e20`
    /// writes bank+0x24/+0x28 from file+0x15c/0x160/0x164 — so the quad stands ON the surface
    /// by its half-extent). No surface within 20 yd leaves the spawn position untouched. NOT a
    /// per-frame clamp, NOT terrain-only. Fire/Frost Nova, the ground fogs (37 emitters / 17
    /// spell models).
    pub fn ground_snap(&self) -> bool {
        self.flags & 0x2000 != 0
    }
    /// File flag `0x8000` (bit 15): the emission model is a **one-shot BURST**, not continuous.
    /// The reference's per-frame emitter pass branches on this bit (`0x718ec8`): a burst emitter
    /// gets a rising-edge trigger — the frame `(enabled && rate > 0)` first turns true it requests
    /// ONE spawn of `ftol(rate · density · LOD)` particles (`0x7b5c50` → the spawn driver's
    /// self-clearing burst branch `0x7b55ae`/`0x7b563d`) — and the continuous-enable bit is
    /// skipped, so the `rate·dt` pour can never run for it. Re-arms only when the gate falls
    /// (a looping clip's next pass). This flag is why the Feint/Eviscerate impact's plume and
    /// crescents are over ~0.5 s while the same-shaped `Eviscerate_Cast_Hands` flame (bit clear)
    /// pours for its whole 1.6 s clip.
    pub fn burst(&self) -> bool {
        self.flags & 0x8000 != 0
    }
    /// File flag `0x1000` (→ runtime `0x2000`): the head quad does NOT billboard — it lies flat
    /// in the emitter's model-space XY plane, carried by the live model→world matrix (the
    /// community "XYQuad"; the impact crescents, state rings, fish-school splash rings).
    /// Corners = the ±1 XY unit square (z = 0) × the draw
    /// matrix — camera-independent, unit half-extent exactly like the billboard corner table —
    /// and quad spin is Rodrigues about the quad-plane normal (consumption `0x7b2c25`, the
    /// corner-table build `0x7b41a3`).
    pub fn xy_quad(&self) -> bool {
        self.flags & 0x1000 != 0
    }
    /// The gated twinkle size multiplier, given a noise sample `noise ∈ [0,1)` from the caller's
    /// LUT (indexed off `twinkle_speed · age`): identity when the range is degenerate.
    pub fn twinkle(&self, noise: f32) -> f32 {
        if (self.twinkle_max - self.twinkle_min).abs() < 1e-6 {
            1.0
        } else {
            noise * (self.twinkle_max - self.twinkle_min) + self.twinkle_min
        }
    }
}

const STRIDE: usize = 0x1f8;
const HDR_COUNT: usize = 0x13c;
const HDR_PTR: usize = 0x140;

fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn le_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn le_vec3(b: &[u8], o: usize) -> [f32; 3] {
    [le_f32(b, o), le_f32(b, o + 4), le_f32(b, o + 8)]
}

/// Read one raw vanilla M2Track (28 bytes at `track`) into the shared typed shape — **absolute**
/// key timestamps plus the per-sequence `ranges` windows the key-search bake indexes.
/// `benilla-m2` skips the cosmetic chunks, so the two emitter timing tracks are lifted here;
/// `elem`/`read` decode one
/// value (`f32`, or the enabled gate's `u8` projected to 0/1).
fn read_raw_track(
    b: &[u8],
    track: usize,
    elem: usize,
    read: impl Fn(&[u8], usize) -> f32,
) -> M2ScalarTrack {
    let mut out = M2ScalarTrack {
        gseq: 0xffff,
        ..M2ScalarTrack::default()
    };
    if track + 0x1c > b.len() {
        return out;
    }
    out.interp = le_u16(b, track);
    out.gseq = le_u16(b, track + 2);
    let (rn, ro) = (le_u32(b, track + 4) as usize, le_u32(b, track + 8) as usize);
    let (tn, to) = (
        le_u32(b, track + 0x0c) as usize,
        le_u32(b, track + 0x10) as usize,
    );
    let (vn, vo) = (
        le_u32(b, track + 0x14) as usize,
        le_u32(b, track + 0x18) as usize,
    );
    if ro + rn * 8 <= b.len() {
        out.ranges = (0..rn)
            .map(|i| (le_u32(b, ro + i * 8), le_u32(b, ro + i * 8 + 4)))
            .collect();
    }
    let n = tn.min(vn);
    if n > 0 && to + n * 4 <= b.len() && vo + n * elem <= b.len() {
        out.keys = (0..n)
            .map(|i| (le_u32(b, to + i * 4), read(b, vo + i * elem)))
            .collect();
    }
    out
}

fn blend_of(v: u8) -> ParticleBlend {
    match v {
        3 | 4 => ParticleBlend::Add,
        2 | 5 | 6 => ParticleBlend::Alpha, // 5/6 (mod/mod2x) fold to alpha until a model needs them
        1 => ParticleBlend::AlphaKey,
        _ => ParticleBlend::Opaque,
    }
}

/// `DAT_00811fa8` — the reference's per-blend-mode lighting table, indexed by the emitter's RAW
/// blend field (file+0x28): the multiply/modulate modes light nothing, every other mode does
/// (read at `0x70bb0a`).
const LIGHTING_BY_BLEND: [bool; 7] = [true, true, true, true, true, false, false];

/// Does the reference draw this emitter's quads **LIT** by the scene?
///
/// A particle emitter has no material of its own: the reference *synthesizes* an `M2Material` from
/// the file record every draw (`0x70d8b0`) and runs it through the SAME batch state producer as an
/// ordinary mesh submesh, so `GL_LIGHTING` (EGxRs id `0x0e`) is decided exactly as it is for a
/// mesh. At `0x70baf0` @`0x70bb00`:
///
/// ```text
/// lit  ⇔  (file+0x04 bit 0x1 CLEAR)  AND  DAT_00811fa8[file+0x28] ≠ 0
/// ```
///
/// **File bit `0x1` is the UNLIT flag** — the *inverse* of the wowdev-wiki lore ("0x1 = affected by
/// lighting"), and matching the M2 render-flag `0x01 = unlit` convention everywhere else in the
/// format. That polarity is the whole point: the fire/spell corpus authors `0x1` and stays bright at
/// night, while the ambient environment sheets that CLEAR it — waterfall spray, chimney smoke,
/// blown dust and snow — are shaded by the world's own light like the geometry they sit against.
/// Rendering those unlit puts a full-white foam sheet in a shaded jungle (the Zul'Gurub waterfall).
fn lit_of(flags: u32, blend_byte: u8) -> bool {
    flags & 0x1 == 0
        && LIGHTING_BY_BLEND
            .get(usize::from(blend_byte))
            .copied()
            .unwrap_or(true)
}

fn shape_of(v: u16) -> ParticleShape {
    match v {
        2 => ParticleShape::Sphere,
        3 => ParticleShape::Spline,
        _ => ParticleShape::Plane,
    }
}

/// Parse the vanilla M2's particle emitters from its file bytes. Texture indices are resolved against
/// the M2 textures table (parsed by `benilla-m2`; the particle *records* are read here). Returns an
/// empty vec if the model has no emitters or isn't a parseable M2.
pub fn parse_m2_particle_emitters(bytes: &[u8]) -> Result<Vec<ParticleEmitterDef>> {
    // Texture names come from `benilla-m2`'s textures table.
    let textures: Vec<Option<String>> = match parse_m2(&mut Cursor::new(bytes)) {
        Ok(fmt) => fmt
            .model()
            .textures
            .iter()
            .map(|t| {
                let f = t
                    .filename
                    .string
                    .to_string_lossy()
                    .trim_end_matches('\0')
                    .to_string();
                (!f.is_empty()).then_some(f)
            })
            .collect(),
        Err(_) => Vec::new(),
    };

    if bytes.len() < HDR_PTR + 4 || &bytes[..4] != b"MD20" {
        return Ok(Vec::new());
    }
    let count = le_u32(bytes, HDR_COUNT) as usize;
    let base = le_u32(bytes, HDR_PTR) as usize;
    if count == 0 || count > 256 || base + count * STRIDE > bytes.len() {
        return Ok(Vec::new());
    }

    // Every FILE sequence slot's band + loop flag (entry stride 0x44: band start/end @ +0x04/+0x08,
    // flags @ +0x10, bit 0 CLEAR = loops) — the addressing unit the per-sequence timing bake needs,
    // the same walk as `m2_batches`. A sequence-less model gets one synthetic whole-timeline slot
    // so an authored track still bakes (raw timestamps, the pre-sequence degenerate).
    let mut seq_slots: Vec<SeqSlot> = {
        let (n, o) = (le_u32(bytes, 0x1c) as usize, le_u32(bytes, 0x20) as usize);
        (0..n)
            .map_while(|i| {
                let e = o + i * 0x44;
                (e + 0x44 <= bytes.len()).then(|| SeqSlot {
                    index: i,
                    band: (le_u32(bytes, e + 0x04), le_u32(bytes, e + 0x08)),
                    looping: le_u32(bytes, e + 0x10) & 1 == 0,
                })
            })
            .collect()
    };
    if seq_slots.is_empty() {
        seq_slots.push(SeqSlot {
            index: 0,
            band: (0, u32::MAX),
            looping: true,
        });
    }
    // The global-sequence duration table (header +0x14/+0x18) — a gseq-tagged timing track loops
    // on its own free clock, same law as every other channel.
    let gseq: Vec<u32> = {
        let (n, o) = (le_u32(bytes, 0x14) as usize, le_u32(bytes, 0x18) as usize);
        (0..n)
            .map_while(|i| (o + i * 4 + 4 <= bytes.len()).then(|| le_u32(bytes, o + i * 4)))
            .collect()
    };

    // Decode a packed BGRA `CImVector` color key → linear RGBA 0..1 (A = over-life opacity).
    let color_key = |o: usize| -> [f32; 4] {
        let v = le_u32(bytes, o);
        [
            ((v >> 16) & 0xff) as f32 / 255.0, // R
            ((v >> 8) & 0xff) as f32 / 255.0,  // G
            (v & 0xff) as f32 / 255.0,         // B
            ((v >> 24) & 0xff) as f32 / 255.0, // A
        ]
    };

    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = base + i * STRIDE;
        let tex_index = le_u16(bytes, e + 0x16) as usize;
        let texture = textures.get(tex_index).cloned().flatten();
        let shape = shape_of(le_u16(bytes, e + 0x2a));
        // The two per-emitter model paths (`M2Array<char>`, offset pre-fixup = file-relative):
        // geometry (model particles) and recursion (child emitters).
        let model_path = |cnt_off: usize| -> Option<String> {
            let n = le_u32(bytes, e + cnt_off) as usize;
            let ofs = le_u32(bytes, e + cnt_off + 4) as usize;
            if n < 2 || ofs + n > bytes.len() {
                return None;
            }
            let s = String::from_utf8_lossy(&bytes[ofs..ofs + n])
                .trim_end_matches('\0')
                .to_string();
            (!s.is_empty()).then_some(s)
        };
        let geometry_model = model_path(0x18);
        let recursion_model = model_path(0x20);
        // SPLINE control points (file+0x1d4/+0x1d8, raw M2Array dwords): the reference copies
        // N = 3·⌊count/3⌋+1 C3Vectors — the flattened cubic Bézier chain.
        let spline = (shape == ParticleShape::Spline)
            .then(|| {
                let q = le_u32(bytes, e + 0x1d4) as usize;
                let ofs = le_u32(bytes, e + 0x1d8) as usize;
                let n = 3 * (q / 3) + 1;
                if q < 3 || ofs + n * 12 > bytes.len() {
                    return None;
                }
                SplineData::new((0..n).map(|p| le_vec3(bytes, ofs + p * 12)).collect())
            })
            .flatten();
        // ROWS @+0x30 / COLUMNS @+0x32, both required non-zero powers of two — the reference's
        // `0x7b4ed0` bails on a bad pair having written nothing, leaving its ctor's 1×1.
        let tiles = match (le_u16(bytes, e + 0x30), le_u16(bytes, e + 0x32)) {
            (r, c) if r.is_power_of_two() && c.is_power_of_two() => (r, c),
            _ => (1, 1),
        };
        let over_life = OverLife {
            mid: le_f32(bytes, e + 0x14c),
            color: [
                color_key(e + 0x150),
                color_key(e + 0x154),
                color_key(e + 0x158),
            ],
            scale: [
                le_f32(bytes, e + 0x15c),
                le_f32(bytes, e + 0x160),
                le_f32(bytes, e + 0x164),
            ],
            // The +0x168..+0x17b block is TEN u16s, read `{head A, head B, tail A, tail B}` with a
            // repeat count wedged after each head pair — not the eight that wowdev's
            // `lifespanUVAnim[3]/decayUVAnim[3]/tailUVAnim[2]` naming implies (`0x70ebd0`'s reads;
            // +0x17c is already the tail LENGTH dword).
            head_cells: [
                CellRamp::new(le_u16(bytes, e + 0x168), le_u16(bytes, e + 0x16a)),
                CellRamp::new(le_u16(bytes, e + 0x16e), le_u16(bytes, e + 0x170)),
            ],
            tail_cells: [
                CellRamp::new(le_u16(bytes, e + 0x174), le_u16(bytes, e + 0x176)),
                CellRamp::new(le_u16(bytes, e + 0x178), le_u16(bytes, e + 0x17a)),
            ],
            repeat: [
                f32::from(le_u16(bytes, e + 0x16c)),
                f32::from(le_u16(bytes, e + 0x172)),
            ],
        };
        out.push(ParticleEmitterDef {
            flags: le_u32(bytes, e + 0x04),
            position: le_vec3(bytes, e + 0x08),
            bone: le_u16(bytes, e + 0x14),
            shape,
            spline,
            geometry_model,
            recursion_model,
            blend: blend_of(bytes[e + 0x28]),
            lit: lit_of(le_u32(bytes, e + 0x04), bytes[e + 0x28]),
            texture,
            tile_rows: tiles.0,
            tile_cols: tiles.1,
            head_tail: bytes[e + 0x2c],
            timing: EmitTiming::bake(
                &read_raw_track(bytes, e + 0xdc, 4, le_f32),
                &read_raw_track(bytes, e + 0x1dc, 1, |b, o| f32::from(b[o] != 0)),
                &seq_slots,
                &gseq,
            ),
            // The nine parameter tracks, full-key ([`EmitParams`] field order — bases 0x1c apart,
            // the emitter phase's own walk). NOT `value[0]`: several effects animate them.
            params: EmitParams::bake(
                [
                    &read_raw_track(bytes, e + 0x34, 4, le_f32),  // speed
                    &read_raw_track(bytes, e + 0x50, 4, le_f32),  // speedVar
                    &read_raw_track(bytes, e + 0x6c, 4, le_f32),  // latitude
                    &read_raw_track(bytes, e + 0x88, 4, le_f32),  // longitude
                    &read_raw_track(bytes, e + 0xa4, 4, le_f32),  // gravity
                    &read_raw_track(bytes, e + 0xc0, 4, le_f32),  // lifespan
                    &read_raw_track(bytes, e + 0xf8, 4, le_f32),  // areaLength
                    &read_raw_track(bytes, e + 0x114, 4, le_f32), // areaWidth
                    &read_raw_track(bytes, e + 0x130, 4, le_f32), // zSource
                ],
                &seq_slots,
                &gseq,
            ),
            drag: le_f32(bytes, e + 0x194),
            tail_time: le_f32(bytes, e + 0x17c),
            angular_velocity_min: le_vec3(bytes, e + 0x19c),
            angular_velocity_max: le_vec3(bytes, e + 0x1a8),
            inherit_scale: le_f32(bytes, e + 0x190),
            follow_speed1: le_f32(bytes, e + 0x1c4),
            follow_scale1: le_f32(bytes, e + 0x1c8),
            follow_speed2: le_f32(bytes, e + 0x1cc),
            follow_scale2: le_f32(bytes, e + 0x1d0),
            twinkle_speed: le_f32(bytes, e + 0x180),
            twinkle_percent: le_f32(bytes, e + 0x184),
            twinkle_min: le_f32(bytes, e + 0x188),
            twinkle_max: le_f32(bytes, e + 0x18c),
            spin: le_f32(bytes, e + 0x198),
            over_life,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **File bit 0x1 is UNLIT, not "lit"** — the polarity this whole mechanism turns on, and the
    /// one the community table gets backwards (wowdev: "0x1 = affected by lighting"; the binary
    /// forces `GL_LIGHTING` OFF on that bit @`0x70bb00`).
    /// Inverting it would light exactly the wrong 95% of the corpus: every fire and spell effect
    /// would go dim at night while the environment sheets stayed full-white cutouts — a change
    /// that "works" on any single model you happen to check first.
    #[test]
    fn bit_one_is_the_unlit_flag_and_mod_blends_never_light() {
        // The Zul'Gurub / Elwynn waterfall: bit 0x1 CLEAR, blend 2 (alpha) ⇒ the scene lights it.
        assert!(
            lit_of(0x0002, 2),
            "an emitter that CLEARS 0x1 takes the light"
        );
        // The Orgrimmar bonfire's flame (0x29) and smoke (0x21): both SET 0x1 ⇒ both unlit, which
        // is what keeps a campfire bright at night.
        assert!(!lit_of(0x0029, 4), "0x1 set: unlit, whatever the blend");
        assert!(!lit_of(0x0021, 2));
        // `DAT_00811fa8` = {1,1,1,1,1,0,0}: the multiply modes light nothing even with 0x1 clear —
        // a lit modulate would darken twice, once through the light and once through the blend.
        for blend in 0u8..=4 {
            assert!(lit_of(0x0000, blend), "blend {blend} lights");
        }
        assert!(!lit_of(0x0000, 5), "Mod never lights");
        assert!(!lit_of(0x0000, 6), "Mod2x never lights");
        // A blend byte past the table is not a panic and not a silent "unlit" — the reference
        // indexes a 7-entry table it never overruns, and every such value already folds to
        // `Opaque`, which lights.
        assert!(lit_of(0x0000, 7));
    }

    /// The real corpus, at the emitter the director reported: the waterfall's ONE emitter clears
    /// bit 0x1 and blends alpha, so it must come back LIT — the asset-side half of the foam fix.
    #[test]
    fn the_waterfall_emitter_is_lit() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file(
                "World\\Azeroth\\Elwynn\\PassiveDoodads\\Waterfall\\ElwynnTallWaterfall01.m2",
            )
            .expect("read ElwynnTallWaterfall01.m2");
        let defs = parse_m2_particle_emitters(&bytes).expect("parse emitters");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].flags, 0x0002, "authored flag word");
        assert_eq!(defs[0].blend, ParticleBlend::Alpha);
        assert!(defs[0].lit, "the spray sheet is shaded by the world");
    }

    /// The spline chain is **arc-length** parameterized: two straight segments of length 1 and
    /// 3 (control points at exact thirds, so the cubic degenerates to linear) put the segment
    /// boundary at t = 0.25, not 0.5 — plus the reference's outside-[0,1] clamp legs.
    #[test]
    fn spline_chain_is_arc_length_parameterized() {
        let x = |v: f32| [v, 0.0, 0.0];
        let s = SplineData::new(vec![
            x(0.0),
            x(1.0 / 3.0),
            x(2.0 / 3.0),
            x(1.0), // segment 0: 1 yd
            x(2.0),
            x(3.0),
            x(4.0), // segment 1: 3 yd
        ])
        .expect("3K+1 chain");
        assert!((s.eval(0.25)[0] - 1.0).abs() < 1e-4, "boundary at arc 1/4");
        assert!((s.eval(0.625)[0] - 2.5).abs() < 1e-4, "mid of segment 1");
        assert_eq!(s.eval(-0.5), [0.0, 0.0, 0.0], "clamp to first point");
        assert_eq!(s.eval(1.5), [4.0, 0.0, 0.0], "clamp to last point");
        let tan = s.tangent(0.1);
        assert!(tan[0] > 0.0 && tan[1] == 0.0 && tan[2] == 0.0, "+X tangent");
        assert!(SplineData::new(vec![x(0.0)]).is_none(), "below one segment");
    }

    /// The real `BloodSpurt.m2` (decision 0140 fold-back — the melee impact flash): 4 emitters,
    /// and the three the old `value[0]` bake silently killed are **keyed bursts** — the starflash
    /// (STARFLASH_GREY, additive) rates 0→20→0 over the first 133 ms. Pins the burst keys and
    /// that the spray (emitter 0) still reads its constant-equivalent first key of 100/s.
    #[test]
    fn real_blood_spurt_emitters_are_keyed_bursts() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Particles\\BloodSpurts\\BloodSpurt.m2")
            .expect("read BloodSpurt.m2");
        let defs = parse_m2_particle_emitters(&bytes).expect("parse emitters");
        assert_eq!(defs.len(), 4);
        // Emitter 0 — the red spray: starts hot (100/s at t=0), the old bake's one working case.
        assert_eq!(defs[0].timing.rate(None, 0.0, 0.0), 100.0);
        // Emitter 3 — the starflash: additive, STARFLASH texture, burst-keyed (first key 0,
        // peak 20/s inside the 67–100 ms window, dead again by 133 ms).
        let flash = &defs[3];
        assert!(flash
            .texture
            .as_deref()
            .is_some_and(|t| t.to_ascii_uppercase().contains("STARFLASH")));
        assert_eq!(flash.blend, ParticleBlend::Add);
        assert_eq!(flash.timing.rate(None, 0.0, 0.0), 0.0, "silent at t=0");
        assert_eq!(flash.timing.peak_rate(), 20.0);
        assert_eq!(flash.timing.rate(None, 0.080, 0.0), 20.0);
        assert_eq!(flash.timing.rate(None, 0.200, 0.0), 0.0);
        // Emitters 1/2 — the glowball droplets, same burst shape, peaks 200.
        assert_eq!(defs[1].timing.peak_rate(), 200.0);
        assert_eq!(defs[2].timing.peak_rate(), 200.0);
        // The spray's enabled track cuts emission at 500 ms — exactly where its rate track goes
        // negative (the authored tail the old always-on read let the floor-at-0 hide).
        assert!(defs[0].timing.emitting(None, 0.4, 0.0));
        assert!(!defs[0].timing.emitting(None, 0.6, 0.0));
    }

    /// The real `Feint_Impact_Chest.m2` (the Eviscerate/Feint impact) — the BURST-flag split
    /// that bounds its visible burst at ~0.5 s on the reference: plume (e0) and crescents (e3)
    /// author file flag 0x8000 (one-shot burst — a single `ftol(rate)` puff at their 67 ms key),
    /// lava (e1) and dust (e2) are continuous, choreographed by their enabled windows. The
    /// cast-side `Eviscerate_Cast_Hands.m2` is the counter-case: the same 1.6 s asset shape with
    /// bit 15 clear — a continuous pour for the whole clip.
    #[test]
    fn real_feint_impact_authors_burst_emitters() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let impact = parse_m2_particle_emitters(
            &chain
                .read_file("Spells\\Feint_Impact_Chest.m2")
                .expect("read Feint_Impact_Chest.m2"),
        )
        .expect("parse emitters");
        assert_eq!(impact.len(), 4);
        assert!(impact[0].burst(), "plume is a one-shot burst");
        assert!(!impact[1].burst(), "lava pours (enabled window 0–333 ms)");
        assert!(!impact[2].burst(), "dust pours (enabled window 67–200 ms)");
        assert!(impact[3].burst(), "crescents are a one-shot burst");
        // The STEP rate law that arms the burst count: silent before the 67 ms key, the full
        // value at it, held past it (where the burst latch keeps it from ever mattering).
        assert_eq!(impact[0].timing.rate(None, 0.050, 0.0), 0.0);
        assert_eq!(impact[0].timing.rate(None, 0.067, 0.0), 30.0);
        assert_eq!(impact[0].timing.rate(None, 1.500, 0.0), 30.0);
        let cast = parse_m2_particle_emitters(
            &chain
                .read_file("Spells\\Eviscerate_Cast_Hands.m2")
                .expect("read Eviscerate_Cast_Hands.m2"),
        )
        .expect("parse emitters");
        assert!(
            cast.iter().all(|e| !e.burst()),
            "the cast-hands flame is continuous — same asset shape, opposite flag"
        );
    }

    /// The real `FlameStrike_Area.m2` — the spline-record law on real content: e2..e5 are
    /// type-3 emitters whose
    /// control-point counts land exactly on the `3K+1` flattened-Bézier law (16 and 19), with
    /// the repurposed fields reading sanely (t window [0,1], no spin/scatter — the fire
    /// columns sit ON their authored descent curves with zero birth velocity).
    #[test]
    fn real_flamestrike_authors_spline_chains() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let defs = parse_m2_particle_emitters(
            &chain
                .read_file("Spells\\FlameStrike_Area.m2")
                .expect("read FlameStrike_Area.m2"),
        )
        .expect("parse emitters");
        let splines: Vec<_> = defs
            .iter()
            .filter(|d| d.shape == ParticleShape::Spline)
            .collect();
        assert_eq!(splines.len(), 4, "the four fire-column emitters");
        for d in &splines {
            let s = d.spline.as_ref().expect("chain parses");
            assert_eq!(s.points.len() % 3, 1, "3K+1 control points");
            assert!(s.points.len() >= 16);
            let now = d.params.sample(None, 0.0, 0.0);
            assert_eq!(now.area_length, 0.0, "tMin");
            assert_eq!(now.area_width, 1.0, "tMax");
            assert_eq!(now.vertical_range, 0.0, "no tangent spin");
            assert!(d.burst(), "one puff of standing flames");
            // The chain starts high (the authored descent) — a sane, in-model coordinate.
            assert!(
                (5.0..15.0).contains(&s.points[0][2]),
                "z {}",
                s.points[0][2]
            );
            assert_eq!(s.eval(0.0), s.points[0], "t=0 is the first point");
        }
    }

    /// The real `Fire_Cast_Hand.m2` (spell 133's "go" flash) and `MoltenBlast_Impact_Chest.m2`
    /// (its impact): the authored choreography this parse exists to carry. Cast hand: constant
    /// rates, enabled for exactly the first 200 ms of the clip. Impact emitter 0 (the plume
    /// burst): rate ramps 0→60 across the first 133 ms — a full second late under the old raw
    /// global-timeline read (seq 0 spans [1000, 2600]).
    #[test]
    fn real_fireball_effects_rebase_to_clip_time() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cast = parse_m2_particle_emitters(
            &chain
                .read_file("Spells\\Fire_Cast_Hand.m2")
                .expect("read Fire_Cast_Hand.m2"),
        )
        .expect("parse emitters");
        assert_eq!(cast.len(), 3);
        for em in &cast {
            assert!(
                em.timing.emitting(None, 0.1, 0.0),
                "the flash is ON at its start"
            );
            assert!(
                !em.timing.emitting(None, 0.25, 0.0),
                "and OFF from 200 ms — the 1.0 s clip does not burn through"
            );
        }
        let impact = parse_m2_particle_emitters(
            &chain
                .read_file("Spells\\MoltenBlast_Impact_Chest.m2")
                .expect("read MoltenBlast_Impact_Chest.m2"),
        )
        .expect("parse emitters");
        assert_eq!(impact.len(), 6);
        assert_eq!(impact[0].timing.rate(None, 0.0, 0.0), 0.0);
        assert_eq!(
            impact[0].timing.rate(None, 0.133, 0.0),
            60.0,
            "the plume bursts at impact"
        );
        assert!(impact[1].timing.emitting(None, 0.1, 0.0));
        assert!(
            !impact[1].timing.emitting(None, 0.4, 0.0),
            "shockwave window is 300 ms"
        );
        assert!(
            !impact[4].timing.emitting(None, 0.05, 0.0),
            "smoke starts staggered…"
        );
        assert!(impact[4].timing.emitting(None, 0.2, 0.0));
        assert!(
            !impact[4].timing.emitting(None, 0.7, 0.0),
            "…and ends by 567 ms"
        );
    }
}
