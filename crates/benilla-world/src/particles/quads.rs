//! The particle **quad expansion** — one pool of live particles → the camera-facing (or
//! XY-plane) billboard mesh, exactly the reference's quad writer laws (`0x7b2a50` head,
//! `0x7b3041` tail, the twinkle LUT, the `0x7b2dda` spin negate). Shared verbatim by a parent
//! emitter and its CHILD emitters (`0x7b5dd0`), which differ only in whose pool
//! and def feed it.

use benilla_assets::coords::wow_to_bevy;
use benilla_formats::ParticleEmitterDef;
use bevy::prelude::*;

use super::buffer::EffectVertex;
use super::{rand01, Particle};

/// The 128-entry twinkle noise table — the reference seeds `DAT_00cf58f0` with uniform-random f32
/// in [0,1) at startup (`0x706790`, incl. the fill loop; we mirror the distribution with a fixed
/// seed, not the reference's stream).
static TWINKLE_LUT: std::sync::LazyLock<[f32; 128]> = std::sync::LazyLock::new(|| {
    let mut s = 0xC0FF_EE11u32;
    let mut t = [0.0f32; 128];
    for v in &mut t {
        *v = rand01(&mut s);
    }
    t
});

/// The twinkle noise sample for a particle: the byte-verified index (`0x7b2a86`) is
/// `(floor(clamp(twinkleSpeed · age, 0, 255)) + particlePhase) & 0x7f` — the reference's phase is
/// a per-particle pointer hash; ours is a spawn-time random ([`Particle::phase`]). The phase
/// de-syncs the flicker across particles (without it a whole flame pulses in lockstep).
fn twinkle_noise(twinkle_speed: f32, age: f32, phase: u32) -> f32 {
    let idx = ((twinkle_speed * age).clamp(0.0, 255.0) as u32).wrapping_add(phase) as usize & 0x7f;
    TWINKLE_LUT[idx]
}

/// The quad-spin angle (byte-verified: the negate at `0x7b2dda`, immediately before the fsincos
/// fold, shared by the plain-rotated and Rodrigues legs): `spin·age`, **negated when the raw angle
/// is negative on a particle whose pool-slot pointer carries bit 5** — the 0x20-byte `CParticle2`
/// slots alternate that bit, so this is vanilla's entire rotation randomizer: a NEGATIVE-spin
/// emitter counter-rotates half its cloud (the Fire Blast impact smoke reads as churning volume),
/// a positive-spin one rotates uniformly. Same-age burst particles rotating in lockstep otherwise
/// turn the whole cloud as one rigid picture — the director's "2D smoke". The reference's half-
/// split is a pointer hash; ours is the same spawn-time [`Particle::phase`] the twinkle index uses
/// (stable per particle, which a pool INDEX under `retain_mut` compaction would not be).
pub(super) fn spin_angle(spin: f32, age: f32, phase: u32) -> f32 {
    let angle = spin * age;
    if angle < 0.0 && phase & 0x20 != 0 {
        -angle
    } else {
        angle
    }
}

/// The camera basis one frame of expansion billboards against. (No facing normal: the effect
/// shader is unlit and never backface-culled, and the lane's vertex stream carries none — the
/// old per-vertex `cam.face_normal` existed only to satisfy the mesh pipeline's layout.)
pub(super) struct CamBasis {
    pub(crate) right: Vec3,
    pub(crate) up: Vec3,
}

/// The cloud's draw frame: where its vertices are relative to ([`anchor`]) and how stored
/// coordinates reach the world (the anchored/model split of [`super::Particle`]'s doc).
pub(super) struct DrawFrame {
    pub(crate) anchored: bool,
    /// The WORLD-mode store's **ride frame** `A` ([`crate::ride_frame`]) — the
    /// reference's `[ebp+8]` at `0x7b3d20`, folded forward on the `0x7b3f4f` leg. `None` off a
    /// transport, which is every cloud on the ground, and then every fold below is the identity.
    /// Model mode never reads it: `0x7b3efb` leaves `[ebp+8]` untouched.
    pub(crate) ride: crate::ride_frame::StoredFrame,
    /// The owning MODEL's render alpha, folded into every particle's alpha channel — the
    /// reference's `emitter+0x1a8`. 1.0 for a model that isn't fading.
    pub(crate) alpha: f32,
    /// The lane's **size unit** — what one model unit of a particle's half-extent is in this
    /// cloud's stored frame. `1.0` everywhere in the world (a yard is a yard). A UI model tile
    /// stores its particles in device pixels, and the reference maps a
    /// widget's particle half-extent through the screen — `768·√(a²+1)` FrameXML units per
    /// unit, the instance scale NOT included (`0x7b2ba6`) — so
    /// the tile sets this to that many pixels per unit; an emitter flagged to scale with its
    /// instance already reads the instance through `placement.scale` and takes 1.0 here.
    pub(crate) size_scale: f32,
}

/// One particle's world-space quad centre — THE point [`expand_quads`] rasterizes around, factored
/// out so the `WOW_PARTICLE_DEPTHDUMP` instrument reports the same value the GPU gets, not a
/// re-derivation that can drift.
pub(super) fn particle_center(frame: &DrawFrame, placement: &Transform, p: &Particle) -> Vec3 {
    if frame.anchored {
        // WORLD mode (`0x10` CLEAR): the store is absolute in its OWN frame and the draw never
        // folds `rt+0x1fc` back — the reference's `0x7b3f48`. The
        // one matrix that does fold is the ride frame, on the `0x7b3f4f` leg: `A · T · S`. Off a
        // transport `A` is NULL and the store is world, which is the `0x7b3f95` leg.
        frame.ride.to_world(p.pos)
    } else {
        // MODEL mode (`0x10` SET): the live emitter matrix, re-applied every frame — `0x7b3efb`.
        placement.transform_point(wow_to_bevy([p.pos.x, p.pos.y, p.pos.z]))
    }
}

/// The twinklePercent draw-gate — mirrors [`expand_quads`]'s `0x7b2adc` skip, exported so the
/// depth dump reports only particles that actually emit a quad. **If that condition changes,
/// change this.**
pub(super) fn draw_gated(def: &ParticleEmitterDef, p: &Particle) -> bool {
    let noise = twinkle_noise(def.twinkle_speed, p.age, p.phase);
    def.twinkle_percent < 1.0 && noise > def.twinkle_percent
}

/// One particle's rendered half-extent — mirrors the `half` term inside [`expand_quads`] (over-life
/// size × gated twinkle × instance scale), which computes it inline because it needs the noise and
/// over-life samples for other attributes too. **If that expression changes, change this.**
pub(super) fn particle_half(
    def: &ParticleEmitterDef,
    placement: &Transform,
    p: &Particle,
    size_scale: f32,
) -> f32 {
    let noise = twinkle_noise(def.twinkle_speed, p.age, p.phase);
    let u_age = (p.age / p.life).clamp(0.0, 1.0);
    let scale = if def.scale_size_by_instance() {
        placement.scale.x.max(1e-4)
    } else {
        size_scale
    };
    def.over_life.sample(u_age).size * def.twinkle(noise) * scale
}

/// Expand one pool into the shared effect-quad stream (whole quads, corner order closed by the
/// lane's static `[0,1,2, 0,2,3]` index pattern). Vertices are **world-space** — the draw
/// record carries the sort anchor now (see [`super::buffer`]).
pub(super) fn expand_quads(
    def: &ParticleEmitterDef,
    particles: &[Particle],
    frame: &DrawFrame,
    placement: &Transform,
    cam: &CamBasis,
    out: &mut Vec<EffectVertex>,
) {
    let anchored = frame.anchored;
    let (cam_right, cam_up) = (cam.right, cam.up);
    // Size scales with the instance transform only when the emitter flags it (0x200) — an
    // instance-scaled prop otherwise scales its particle *positions* only (`0x7b2a50`). The
    // unflagged size is in the lane's size unit ([`DrawFrame::size_scale`] — a yard, or a UI
    // tile's pixels per model unit).
    let scale = if def.scale_size_by_instance() {
        placement.scale.x.max(1e-4)
    } else {
        frame.size_scale
    };
    // The XY-quad head basis (file flag 0x1000, `0x7b41a3`): the quad lies flat in the
    // emitter's model-space XY plane carried by the
    // LIVE model→world matrix — camera-independent (the impact crescents, state rings,
    // fish-school splashes). Corners inherit the matrix's scale (the reference's `S·M`;
    // separate from — and stacking with — the flag-0x200 size multiply above), and the
    // live placement orients the plane in BOTH sim modes: the reference folds the emitter
    // orientation into the corner matrix even when birth-baking positions (anchored mode).
    // The corner square rides the same R(+Z,90°)-prepended emitter matrix as the particles
    // (`0x719114`; we fold R at emission — `emit_local` — so here the
    // basis vectors carry it explicitly): X̂ → R·X̂ = Ŷ, Ŷ → R·Ŷ = −X̂ — an in-plane quarter
    // turn of every flat quad (invisible on round textures, load-bearing on crescents).
    let plane_basis = def.xy_quad().then(|| {
        let s = placement.scale.x.max(1e-4);
        (
            placement.rotation * (wow_to_bevy([0.0, 1.0, 0.0]) * s),
            placement.rotation * (wow_to_bevy([-1.0, 0.0, 0.0]) * s),
        )
    });
    // Atlas walk (`0x7b2a50` @0x7b2bd5 head / @0x7b304e tail): `col = idx & (COLUMNS−1)`,
    // `row = idx >> log2(COLUMNS)`.
    // COLUMNS is a power of two by construction — the reference validates it at load and falls
    // back to a 1×1 grid otherwise, which the parse mirrors. **The column WRAPS and the row does
    // NOT**: there is no clamp into the atlas anywhere on the reference's path, so an index past
    // the last cell yields `V ≥ 1.0` and is handed to the sampler's (repeat) addressing — landing
    // back on row 0, not on the final cell. 553 emitters author exactly that, at the tail of a
    // flame's flipbook; a `min(rows·cols − 1)` here held the last cell instead.
    let (cols, rows) = (def.tile_cols, def.tile_rows);
    let (inv_cols, inv_rows) = (1.0 / cols as f32, 1.0 / rows as f32);
    let cell_uv = |idx: u16| {
        let cx = f32::from(idx & (cols - 1));
        let cy = f32::from(idx >> cols.trailing_zeros());
        (
            (cx * inv_cols, (cx + 1.0) * inv_cols),
            (cy * inv_rows, (cy + 1.0) * inv_rows),
        )
    };
    for p in particles {
        let noise = twinkle_noise(def.twinkle_speed, p.age, p.phase);
        // The twinklePercent draw-gate (byte-verified `0x7b2adc`): while percent < 1 (never on
        // placed content, which authors 1.0), a frame whose LUT sample exceeds it emits no
        // quad at all — the reference's hard scintillation.
        if def.twinkle_percent < 1.0 && noise > def.twinkle_percent {
            continue;
        }
        let u_age = (p.age / p.life).clamp(0.0, 1.0);
        let ol = def.over_life.sample(u_age);
        let (mut rgba, size) = (ol.color, ol.size);
        // The MODEL's render alpha, folded into the ALPHA channel only — exactly where and how the
        // reference does it: the sampler `0x7b9b10` takes it as its second argument and applies it
        // at `0x7b9b42 fmul` to the alpha byte alone, RGB untouched. So a unit
        // fading in fades its effects in with it, and a first-person avatar's torch stops burning
        // in your face.
        rgba[3] *= frame.alpha;
        // RAW authored RGB — the gamma-space decode happens ONCE, at the FFX combine
        // (decision 0161's gamma lane; 0152/0150 are its shader-placement ancestry). Alpha is
        // a blend weight — raw everywhere. Additive stacks sum in gamma like the reference's
        // bytes; the byte buffer's SATURATION (a stack can never exceed 255) is restored at
        // the FFX chain's scene reads (`ffx_glow.wgsl`, the Frost Nova mist diagnosis) — a
        // too-bright additive stack is never this ramp's bug, do not "fix" it here.
        // World-mode positions are already world and are drawn raw; model mode folds the whole
        // live placement transform (the reference's rt-0x100 render fold, `0x7b3d20`).
        let center = particle_center(frame, placement, p);
        // `size` is the half-extent: the reference quad corners are ±1.0 (`0x7b2d0c`), so a
        // vertex sits at `center ± size` and the world quad edge spans 2·size.
        // Rendered half-size (`0x7b2a50`): the over-life ramp is the
        // base, × the GATED twinkle flicker (skipped when min == max — `{0,0}` and `{1,1}`
        // alike burn steady; the old base+rand reading collapsed the kobold candle to zero),
        // × the instance scale iff flagged.
        let half = size * def.twinkle(noise) * scale;
        // Within one cloud, quad order stays pool order — the reference's global per-particle
        // painter sort is a named deliberate simplification. Between clouds, the draw record's
        // anchor gives each its real sorted depth (the old anchor-relative vertex encoding's
        // job — two overlapping smoke plumes must never sort-tie and swap per frame).
        let mut push_quad = |corners: [Vec3; 4], quv: [[f32; 2]; 4]| {
            for (c, t) in corners.iter().zip(quv) {
                out.push(EffectVertex {
                    pos: c.to_array(),
                    uv: t,
                    color: rgba,
                });
            }
        };
        // HEAD quad (particleType 0/2), drawn first (`0x7b2bc9`): a camera-facing billboard,
        // or — XY-quad emitters — the flat plane basis computed above.
        if def.head_tail != 1 {
            let ((u0, u1), (v0, v1)) = cell_uv(ol.head_cell);
            let (base_r, base_u) = plane_basis.unwrap_or((cam_right, cam_up));
            // Quad spin (file +0x198): rotate in-plane by `angle = spin·age` (the fcos/fsin
            // fold at `0x7b2ddc`; on an XY quad the same form IS the reference's Rodrigues
            // about the quad-plane normal, since `normal × base_r = base_u`). 0 for nearly
            // every prop. A NEGATIVE spin counter-rotates half the cloud ([`spin_angle`]).
            let (r, u) = if def.spin != 0.0 {
                let (sa, ca) = spin_angle(def.spin, p.age, p.phase).sin_cos();
                (
                    (base_r * ca + base_u * sa) * half,
                    (base_u * ca - base_r * sa) * half,
                )
            } else {
                (base_r * half, base_u * half)
            };
            push_quad(
                [
                    center - r - u,
                    center + r - u,
                    center + r + u,
                    center - r + u,
                ],
                [[u0, v1], [u1, v1], [u1, v0], [u0, v0]],
            );
        }
        // TAIL quad (particleType 1/2, byte-verified `0x7b3041`): a velocity-projected streak
        // trailing the motion — world length |velocity|·tailTime (flag 0x400 grows it from
        // zero with age), width 2·half perpendicular IN SCREEN SPACE, U running along the
        // tail (0 at the particle → 1 at the tip). A view-parallel velocity (degenerate screen
        // length) falls back to a plain billboard.
        //
        // The streak reads its **own** flipbook cell — a second, independently authored ramp
        // (file+0x174..+0x17b), not the head's. The reference emits two cell indices per particle
        // and the tail block reads the second (`[ebp-0x40]` @0x7b3054). 14 shipped emitters on a
        // real atlas — Fire Bolt / Flamestrike / Banish / Hellfire / Vanish impacts, the elemental
        // totems — author a head ramp that walks all 64 cells and a tail ramp pinned at cell 0;
        // handing the head's cell to the streak animated it through the whole sheet (decision
        // 0685).
        if def.head_tail >= 1 {
            let ((u0, u1), (v0, v1)) = cell_uv(ol.tail_cell);
            let vel_world = if anchored {
                // The tail streak points back along the world velocity, so the stored direction
                // takes the ride frame's rotation (never its translation — this is a vector).
                frame.ride.dir_to_world(p.vel)
            } else {
                placement.rotation * (placement.scale * wow_to_bevy(p.vel.to_array()))
            };
            let t_eff = if def.tail_clamps_to_age() {
                def.tail_time.min(p.age)
            } else {
                def.tail_time
            };
            let tail = -vel_world * t_eff;
            let (tr, tu) = (tail.dot(cam_right), tail.dot(cam_up));
            let l2 = tr * tr + tu * tu;
            if l2 < 7.7e-4 {
                // Degenerate: the reference's plain-billboard fallback (`0x7b33fa`).
                let (r, u) = (cam_right * half, cam_up * half);
                push_quad(
                    [
                        center - r - u,
                        center + r - u,
                        center + r + u,
                        center - r + u,
                    ],
                    [[u0, v1], [u1, v1], [u1, v0], [u0, v0]],
                );
            } else {
                let inv_l = half / l2.sqrt();
                let perp = (cam_up * tr - cam_right * tu) * inv_l;
                push_quad(
                    [
                        center - perp,
                        center + perp,
                        center + tail + perp,
                        center + tail - perp,
                    ],
                    [[u0, v1], [u0, v0], [u1, v0], [u1, v1]],
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{expand_quads, spin_angle, CamBasis, DrawFrame, Particle};
    use bevy::prelude::{Quat, Transform, Vec3};

    /// The `0x7b2dda` negate: only a NEGATIVE angle on a bit-5 particle flips — a negative-spin
    /// emitter counter-rotates exactly its bit-5 half, a positive-spin one never splits.
    #[test]
    fn negative_spin_counter_rotates_the_bit5_half() {
        assert_eq!(spin_angle(-3.0, 0.5, 0x20), 1.5, "bit 5 set: negated");
        assert_eq!(spin_angle(-3.0, 0.5, 0x1f), -1.5, "bit 5 clear: kept");
        assert_eq!(
            spin_angle(3.0, 0.5, 0x20),
            1.5,
            "positive spin: never negated"
        );
        assert_eq!(spin_angle(3.0, 0.5, 0x1f), 1.5);
        assert_eq!(spin_angle(0.0, 0.5, 0xff), 0.0);
    }

    /// **The model's render alpha reaches its particles, and ONLY their alpha**.
    /// The reference folds `emitter+0x1a8` (a per-frame copy of the model's `CM2Model+0x19c`)
    /// inside the over-life sampler `0x7b9b10`, at `0x7b9b42 fmul` — which writes the **alpha byte
    /// alone** (`mov [eax+3],cl`); R/G/B never see it. A fold into RGB would look
    /// almost right on an additive quad and be wrong on every alpha-blended one, so the channel
    /// split is what this pins.
    #[test]
    fn the_models_render_alpha_scales_particle_alpha_and_nothing_else() {
        let mut def = crate::particles::tests::plain_def();
        def.over_life.color = [[0.8, 0.4, 0.2, 0.5]; 3];
        let pool = [Particle {
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            age: 0.0,
            life: 1.0,
            phase: 0,
            fresh: false,
            quat: Quat::IDENTITY,
            angvel: Vec3::ZERO,
        }];
        let cam = CamBasis {
            right: Vec3::X,
            up: Vec3::Y,
        };
        let shoot = |alpha| {
            let frame = DrawFrame {
                anchored: true,
                ride: crate::ride_frame::StoredFrame::default(),
                alpha,
                size_scale: 1.0,
            };
            let mut out = Vec::new();
            expand_quads(&def, &pool, &frame, &Transform::IDENTITY, &cam, &mut out);
            out[0].color
        };

        let opaque = shoot(1.0);
        assert_eq!(opaque, [0.8, 0.4, 0.2, 0.5], "no fade: the authored ramp");
        let half = shoot(0.5);
        assert_eq!(
            [half[0], half[1], half[2]],
            [0.8, 0.4, 0.2],
            "RGB is untouched by the model alpha"
        );
        assert!((half[3] - 0.25).abs() < 1e-6, "alpha halves: {half:?}");
        assert_eq!(shoot(0.0)[3], 0.0, "an invisible model draws nothing");
    }
}
