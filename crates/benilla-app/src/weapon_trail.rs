//! The weapon swing trail: `SpellVisualKit` CharProc type 8, the ribbon a melee ability smears
//! behind its blade, and the whole visual of 16 of the 27 trail kits a live spell reaches.
//!
//! The proc latches two words on the unit and the unit's next animation fires them:
//!
//! ```text
//! 0x60d80a  the type-8 dispatcher arm      unit+0xd1c = ftol(ParamZero) | (ftol(ParamThree) << 24)
//!                                          unit+0xd20 = ftol(ParamTwo)          -- ms
//! 0x5fe48e  CGUnit::PlayAnimation, the      if (unit+0xd20 != 0) 0x60e550(&colour, duration)
//!           fields' only reader             unit+0xd20 = 0                       -- one-shot
//! 0x60e550  fan out to CGUnit+0xd34[0..1]   one WTOBJECT per weapon hand
//! 0x6c6750  arm a SWING on that object      re-arming a live one resets its ring, never stacks
//! 0x6c67f0  the weapon model's per-frame cb sample $WTB/$WTT -> 0x6c6560
//! 0x6c6560  append / fade / draw / die      the arithmetic transcribed in [`Swing::step`]
//! ```
//!
//! Lighting is on (`0x6c6847`) over a vertex format with no normal (format 7, position and
//! colour), which leaves:
//!
//! ```text
//! out = (Σ enabled lights' Ambient) × authoredColour
//! ```
//!
//! The vertex colour is the material (`0x5a1e30` sources diffuse and ambient from `COLOR1`). The
//! normal array is disabled (`0x592a60`, `0x59c100`), so by the D3D9 lighting model, inferred
//! rather than traced, the diffuse half contributes nothing. `D3DRS_AMBIENT` has no writers and
//! stays 0, and the trail is a type-5 render record, for which `0x70c190` commits emissive 0
//! (`SetState(2, 0)` at `0x70ca30`). The lights are the wearer's (`[item+0x3b8] =
//! [wearer+0x3b8]`, `0x718960`): the day/night ambient outdoors (`0x69e770`), the light node's
//! ramped word indoors (`0x69e4c0`). The ambient folds into the vertex colours on the CPU.
//!
//! The reference has no transport ride frame for a trail (`0x6c67f0` stores world positions, so a
//! swing on a moving deck smears), no sheath gate and no handedness test (`0x608d60`); creatures
//! trail too.

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use benilla_formats::TrailProc;
use benilla_world::particles::buffer::{EffectVertex, WorldEffectDraw};
use benilla_world::view::WorldCamera;

use crate::entities::HeldAttached;

/// The ring's slot count (`0x6c677c`; write mask `and eax,0x7f` at `0x6c64a4`): two samples a
/// frame, so 64 frames of blade.
const RING_SLOTS: u32 = 0x80;

/// The reader's modulus, not [`RING_SLOTS`]: `0x6c64e5` reads `i % 127` where the writer stored
/// at `i & 127`, a shipped mismatch ([`Swing::read`]).
const READ_MODULUS: u32 = 0x7f;

/// `0x811278`: `0x3b5a740e`, the nearest f32 to 1/300, the per-millisecond fade coefficient.
const FADE_PER_MS: f32 = 0.0033333334;

/// The type-8 dispatcher arming a unit (`0x60d828`/`0x60d835` write `unit+0xd1c`/`unit+0xd20`),
/// at every stage of every kit play carrying the proc.
#[derive(Message, Clone, Copy)]
pub(crate) struct TrailArm {
    /// The unit the kit played on: the wielder, never the target.
    pub(crate) entity: Entity,
    pub(crate) trail: TrailProc,
}

/// The per-unit pending arm, `unit+0xd1c`/`unit+0xd20`: one slot, so a second proc before the
/// first fires overwrites it; cleared by the unit's next animation (`0x5fe4b1`).
#[derive(Resource, Default)]
pub(crate) struct TrailLatch(EntityHashMap<TrailProc>);

/// A weapon prop whose model authors both `$WTB` and `$WTT`: the reference's `WTOBJECT` (pool
/// `0xce86d8`), one per hand. A model missing either marker gets none (`0x6c67f0`'s `0x7130e0`).
#[derive(Component)]
pub(crate) struct WeaponTrail {
    /// `$WTT`, the blade tip, in the prop root's frame.
    top: Vec3,
    /// `$WTB`, the blade base.
    bottom: Vec3,
    /// The live `SWING`, boxed so an idle weapon costs a pointer, not the ring.
    swing: Option<Box<Swing>>,
}

impl WeaponTrail {
    /// The component a spawned weapon prop takes, given its model's baked anchors.
    pub(crate) fn new(top: Vec3, bottom: Vec3) -> Self {
        Self {
            top,
            bottom,
            swing: None,
        }
    }

    /// Arm this hand (`0x6c6750`): a live swing resets its ring in place (`0x6c675f`), so a second
    /// proc restarts the trail rather than stacking one.
    pub(crate) fn arm(&mut self, trail: TrailProc, now_ms: u32) {
        let [r, g, b] = trail.rgb();
        let swing = self.swing.get_or_insert_with(|| {
            Box::new(Swing {
                ring: [Vec3::ZERO; RING_SLOTS as usize],
                head: 0,
                tail: 0,
                rgb: [f32::from(r), f32::from(g), f32::from(b)],
                alpha: 0,
                duration_ms: 0,
                start_ms: 0,
                last_ms: 0,
            })
        });
        swing.head = 0;
        swing.tail = 0;
        swing.rgb = [
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
        ];
        swing.alpha = trail.alpha();
        swing.duration_ms = trail.duration_ms;
        swing.start_ms = now_ms;
        swing.last_ms = now_ms;
    }
}

/// The reference's `SWING` record (pool `0xce86ec`): the ring of blade samples, the packed colour
/// at `+0x608` (byte 3 the live alpha) and the clocks that fade it.
struct Swing {
    /// `+0x08`: written at `head & 0x7f`, read at `i % 127`.
    ring: [Vec3; RING_SLOTS as usize],
    /// `+0x00`: the monotonic write cursor, never masked itself.
    head: u32,
    /// `+0x04`: the oldest sample still retained.
    tail: u32,
    /// The colour, `0..=1` per channel; only the alpha ramps.
    rgb: [f32; 3],
    /// The live alpha byte (`+0x60b`), which also sets the fade rate, the retained segment count
    /// and the end test.
    alpha: u8,
    /// `+0x60c`: how long the ring appends; the fade outlasts it.
    duration_ms: u32,
    /// `+0x610`.
    start_ms: u32,
    /// `+0x614`: the previous evaluation's time; the step takes the delta against it, not the
    /// elapsed time (`0x6c65c1`).
    last_ms: u32,
}

/// One evaluated frame of a swing: the strip to draw, or the verdict that it is over.
enum Step {
    /// `0x6c6560` returned 0: the caller frees the SWING.
    Ended,
    /// The retained samples, oldest first, as `(bottom, top, alpha 0..=1)`; the oldest pair is the
    /// most opaque.
    Strip(Vec<(Vec3, Vec3, f32)>),
}

impl Swing {
    /// `0x6c64a0`: store at `head & 0x7f`, advance the head, keep the floor within capacity.
    fn push(&mut self, p: Vec3) {
        self.ring[(self.head & (RING_SLOTS - 1)) as usize] = p;
        self.head += 1;
        // `if (tail < head - 0x80) tail = head - 0x80`; below capacity the floor stays put.
        self.tail = self.tail.max(self.head.saturating_sub(RING_SLOTS));
    }

    /// `0x6c6540(n)`: raise the floor so at most `n` samples remain, and report how many do.
    fn trim(&mut self, n: u32) -> u32 {
        self.tail = self.tail.max(self.head.saturating_sub(n));
        self.head - self.tail
    }

    /// Read counter `i` through the reader's `% 127`. Past 127 a read lands on the sample one
    /// counter later, swapping the ribbon's two edges; only a trail appending over 127 samples
    /// reaches it, such as Whirlwind's 10 s spin (kits 369, 370, 4213).
    fn read(&self, i: u32) -> Vec3 {
        self.ring[(i % READ_MODULUS) as usize]
    }

    /// `0x6c6560`, one frame: append while the duration holds, compute the fade step, end or trim,
    /// build the strip, then decay the alpha; the strip takes the pre-decay alpha (`0x6c669a`,
    /// then `0x6c669f`).
    fn step(&mut self, now_ms: u32, bottom: Vec3, top: Vec3) -> Step {
        // `6c657c`: an unsigned test, so appending stops the moment the duration is reached.
        let elapsed = now_ms.wrapping_sub(self.start_ms);
        if elapsed < self.duration_ms {
            self.push(bottom); // `$WTB` first (0x6c6587) …
            self.push(top); // … then `$WTT` (0x6c65a1)
        }
        let dt = now_ms.wrapping_sub(self.last_ms);
        self.last_ms = now_ms;
        let step = fade_step(dt, self.alpha);
        // `6c65fa`: one segment or fewer and the trail is over.
        let segments = u32::from(self.alpha) / step;
        if segments <= 1 {
            return Step::Ended;
        }
        let live = self.trim(2 * segments);
        if live == 0 {
            // `6c6620`: an empty ring ends the trail.
            return Step::Ended;
        }
        // `0x6c64d0`, tail to head: the alpha drops once per pair before that pair's colour is
        // stored, so the oldest pair is the opaque end and the weapon's ≈ 0. Do not invert it.
        let mut alpha = i32::from(self.alpha);
        let mut strip = Vec::with_capacity((live as usize).div_ceil(2));
        let mut i = self.tail;
        while i + 1 < self.head {
            alpha -= step as i32;
            strip.push((
                self.read(i),
                self.read(i + 1),
                (alpha.max(0) as f32) / 255.0,
            ));
            i += 2;
        }
        // `6c669f`: full strength for the first half of the duration, then the decay.
        if elapsed > self.duration_ms / 2 {
            self.alpha = self
                .alpha
                .saturating_sub(step.min(u32::from(u8::MAX)) as u8);
        }
        Step::Strip(strip)
    }
}

/// `fadeStep = max(1, ftol(dt_ms · (1/300) · alpha))` (`0x6c65c6`–`0x6c65f8`), the floor tested
/// on the low byte only (`cmp al,1`): 256, a ≈ 768 ms hitch at alpha 100, takes 1.
fn fade_step(dt_ms: u32, alpha: u8) -> u32 {
    // `fild`, `fmul [0x811278]`, `fimul`, `_ftol` at PC_53: f64 with an f32 constant, which
    // `dt · alpha / 300.0` is not.
    let raw = (f64::from(dt_ms) * f64::from(FADE_PER_MS) * f64::from(alpha)).trunc() as i64 as u32;
    if raw & 0xff >= 1 {
        raw
    } else {
        1
    }
}

/// Absorb this frame's arms into the latch, then fire it on every unit that started an animation:
/// `0x5fe48e`'s read, `0x60e550`'s fan-out to both hands, `0x5fe4b1`'s clear.
///
/// The drain comes first so a kit with both the proc and an anim fires in the frame it plays, as
/// in `0x60edf0` (dispatcher `0x60f35c`, animation `0x60f3c5`); a kit with anim `-1` (Charge's kit
/// 44) waits for whatever the unit plays next. A combat clip re-timing itself returns before the
/// latch read (`0x5fe43c`), and the driver raises no started edge there either. Nothing recomputes
/// per frame (`0x5fd8b0` has one caller), so an arm can wait for the next event, even a footstep.
pub(crate) fn fire_weapon_trails(
    time: Res<Time>,
    mut arms: MessageReader<TrailArm>,
    mut latch: ResMut<TrailLatch>,
    units: Query<(Entity, &crate::creature_anim::AnimDriver, &HeldAttached)>,
    mut trails: Query<&mut WeaponTrail>,
) {
    for arm in arms.read() {
        // One slot: a second arm before the first fires replaces it.
        latch.0.insert(arm.entity, arm.trail);
    }
    let now_ms = time.elapsed().as_millis() as u32;
    for (entity, drv, held) in &units {
        if !drv.started_anim() {
            continue;
        }
        // `0x5fe4a0`: gated on a nonzero duration, which `TrailProc` only decodes for; the clear
        // at `0x5fe4b1` is unconditional.
        let Some(trail) = latch.0.remove(&entity) else {
            continue;
        };
        // `0x60e550` walks two slots, skipping a null one: ours are the main- and off-hand props.
        for root in held.spawned_slots().iter().take(2).flatten() {
            if let Ok(mut t) = trails.get_mut(*root) {
                t.arm(trail, now_ms);
            }
        }
    }
    // Entries die with the unit (`0x5fbb60`).
    latch.0.retain(|e, _| units.contains(*e));
}

/// Step every live swing and write its strip into the shared effect stream, after propagation so
/// the prop's `GlobalTransform` is this frame's blade pose.
fn draw_weapon_trails(
    time: Res<Time>,
    mut draw: WorldEffectDraw,
    white: Res<TrailWhite>,
    lighting: Option<Res<benilla_world::lighting::WowLighting>>,
    world_cam: Query<Entity, With<WorldCamera>>,
    // The wearer's committed ambient word, present only while its light node is indoors.
    indoors: Query<&benilla_world::interior::NodeAmbient>,
    mut trails: Query<(
        Entity,
        &mut WeaponTrail,
        &GlobalTransform,
        &InheritedVisibility,
        &benilla_world::model_fade::ParentModel,
    )>,
) {
    let Ok(cam) = world_cam.single() else {
        return;
    };
    // The exterior ambient, a unit's own outdoors (`0x69e4ad`); before the first lighting
    // resolve, the authored colour.
    let scene = lighting.as_deref().map_or([1.0; 3], |l| l.ambient);
    let now_ms = time.elapsed().as_millis() as u32;
    for (entity, mut trail, prop, vis, wearer) in &mut trails {
        // Checked through `&` first, so an idle weapon is not marked changed.
        if trail.swing.is_none() {
            continue;
        }
        // Indoors, the node's own ramped ambient toward `cap96(MOCV)`: the room's light, not the
        // sky's.
        let ambient = indoors
            .get(wearer.0)
            .map_or(scene, |a| a.0)
            .map(|c| c.clamp(0.0, 1.0));
        let bottom = prop.transform_point(trail.bottom);
        let top = prop.transform_point(trail.top);
        let Some(swing) = trail.swing.as_mut() else {
            continue;
        };
        let (rgb, strip) = (swing.rgb, swing.step(now_ms, bottom, top));
        let Step::Strip(strip) = strip else {
            trail.swing = None;
            continue;
        };
        // The visibility gate is benilla's, not the reference's: a hidden prop has no blade on
        // screen. The swing keeps ageing so it does not resume mid-arc when shown.
        if !vis.get() || strip.len() < 2 {
            continue;
        }
        let anchor = {
            let (b, t, _) = strip[strip.len() - 1];
            (b + t) * 0.5
        };
        // The callback's render states (`0x6c6825`–`0x6c686e`): blend mode 2, `SRC_ALPHA /
        // INV_SRC_ALPHA`, not additive; depth test and write off, so the swinger's own shoulder
        // never hides the arc; two-sided, unfogged, lit. Mode 2's alpha test (`GEQUAL 1/255`,
        // `0x593741`) drops only what blending already makes invisible.
        let mut batch = draw
            .batch(cam, white.0.id())
            .alpha()
            .over_everything()
            .anchored(anchor)
            .owner(entity);
        let verts = batch.verts_mut();
        for w in strip.windows(2) {
            let ((b0, t0, a0), (b1, t1, a1)) = (w[0], w[1]);
            // The lane indexes a quad `[0,1,2, 0,2,3]`, so `[b0, b1, t1, t0]` reproduces the
            // reference's strip (`primType 4`).
            for (pos, a) in [(b0, a0), (b1, a1), (t1, a1), (t0, a0)] {
                verts.push(EffectVertex {
                    pos: pos.to_array(),
                    uv: [0.0, 0.0],
                    color: [
                        rgb[0] * ambient[0],
                        rgb[1] * ambient[1],
                        rgb[2] * ambient[2],
                        a,
                    ],
                });
            }
        }
        batch.quads();
    }
}

/// The 1×1 white texel the trail samples: the reference's 16-byte trail vertex has no texcoord
/// (`0x6c6523`), so the ribbon is pure vertex colour.
#[derive(Resource)]
struct TrailWhite(Handle<Image>);

fn init_trail_white(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(TrailWhite(images.add(Image::default())));
}

/// Registers the trail lane.
pub(crate) struct WeaponTrailPlugin;

impl Plugin for WeaponTrailPlugin {
    fn build(&self, app: &mut App) {
        // `fire_weapon_trails` runs in `creature_anim`'s chain, right after the driver that sets
        // the edge it reads.
        app.add_message::<TrailArm>()
            .init_resource::<TrailLatch>()
            .add_systems(Startup, init_trail_white)
            .add_systems(
                PostUpdate,
                draw_weapon_trails
                    .in_set(benilla_world::billboard::BillboardPlace)
                    .after(benilla_world::rig_anim::finalize_rig_worlds)
                    // A lane that writes before the frame's clear loses everything it pushed.
                    .after(benilla_world::particles::buffer::begin_effect_frame),
            );
    }
}

#[cfg(test)]
mod tests;
