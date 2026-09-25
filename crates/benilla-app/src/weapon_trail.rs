//! The weapon **swing trail** — `SpellVisualKit` CharProc type **8**, the ribbon a melee ability
//! smears behind its blade.
//!
//! For **16 of the 27 trail kits a live spell reaches, this is the entire visual**: kit 324
//! (Heroic Strike / Overpower / Mortal Strike / Bloodthirst / Rend — 218 spells), 506
//! (Revenge / Riposte / Mongoose Bite), 557 (Sunder Armor), 646 (Hamstring / Wing Clip), 399
//! (Sinister Strike), 822 (Garrote), 823 (Feint / Disengage) name no emitter slot at all, so
//! without this module those abilities render nothing beyond the swing animation.
//!
//! ## The mechanism
//!
//! The proc does not draw. It **latches two words on the unit** and the unit's *next* animation
//! fires them:
//!
//! ```text
//! 0x60d80a  the type-8 dispatcher arm      unit+0xd1c = ftol(ParamZero) | (ftol(ParamThree) << 24)
//!                                          unit+0xd20 = ftol(ParamTwo)          -- ms
//! 0x5fe48e  CGUnit::PlayAnimation, the      if (unit+0xd20 != 0) 0x60e550(&colour, duration)
//!           fields' ONLY reader             unit+0xd20 = 0                       -- one-shot
//! 0x60e550  fan out to CGUnit+0xd34[0..1]   one WTOBJECT per weapon hand
//! 0x6c6750  arm a SWING on that object      re-arming a live one RESETS its ring, never stacks
//! 0x6c67f0  the weapon model's per-frame cb sample $WTB/$WTT -> 0x6c6560
//! 0x6c6560  append / fade / draw / die      the arithmetic transcribed in [`Swing::step`]
//! ```
//!
//! [`WeaponTrail`] is the WTOBJECT: one per spawned weapon prop whose model authors both markers,
//! carrying the reference's nullable `SWING` as [`Swing`]. The prop's own despawn (a weapon swap,
//! a shapeshift) is the teardown, exactly as `0x6c6970` frees the object with the model reference
//! it holds.
//!
//! ## The light term — ambient only, and no `N·L` half
//!
//! The callback turns lighting **on** (EGxRs `0x0e = 1` at `0x6c6847`) over a vertex format that
//! carries **no normal** (format 7 is position + colour, stride 16). That combination is not a
//! contradiction and it is not a no-op — it selects a specific, reduced term:
//!
//! ```text
//! out = (Σ enabled lights' Ambient) × authoredColour  +  inherited material Emissive
//! ```
//!
//! - The vertex colour **is** the material: `0x5a1e30` sets both `DIFFUSEMATERIALSOURCE` and
//!   `AMBIENTMATERIALSOURCE` to `D3DMCS_COLOR1` off the format's diffuse bit, which format 7 has.
//! - The normal is **explicitly disabled**, not stale: `0x592a60` replaces the format mask rather
//!   than OR-ing it, and `0x59c100` issues `glDisableClientState(GL_NORMAL_ARRAY)` when the bit is
//!   clear (D3D's format-7 descriptor carries `−1` in the normal column). So there is no `N·L` to
//!   compute and the **diffuse half contributes nothing** — which is why this does not simply take
//!   [`benilla_world::particles::buffer::EffectLighting::Scene`], whose term is
//!   `clamp(ambient + diffuse·max(N·L, 0))` against the world up axis. Applying that here would
//!   add a sun term the reference does not have.
//! - The global `D3DRS_AMBIENT` (EGxRs id 6) has **zero writers image-wide** and defaults to 0, so
//!   the whole ambient contribution comes from the M2 lighting collector's own enabled lights —
//!   the same lights our lit emitters take their `ambient` from.
//!
//! So the trail **darkens with the scene** rather than burning at its authored colour — and the
//! scene it darkens with is its **wearer's**, not the world's. The draw runs inside the weapon
//! model's own per-frame callback during the wearer's model draw, so the enabled lights it
//! inherits are the ones the M2 collector committed for that unit; the held weapon's light node is
//! the wearer's own by aliasing (`[item+0x3b8] = [wearer+0x3b8]`, `0x718960`):
//!
//! - **outdoors** a unit's committed ambient IS the day/night ambient (`0x69e770`), so
//!   [`benilla_world::lighting::WowLighting`] is exact, not an approximation;
//! - **indoors** it is the light node's own ramped word, chasing `cap96(MOCV)` (`0x69e4c0`) — the
//!   room's light, never the sky's ([`benilla_world::interior::NodeAmbient`], the ambient half of
//!   the same committed words [`benilla_world::interior::ParticleLight`] folds whole). Its absence
//!   is the exterior lane, which is why the fallback above is the right one and not a guess.
//!
//! One named approximation is left: the **emissive** term is not modelled. It is inherited
//! material state whose value is not pinned, and `EMISSIVEMATERIALSOURCE` is set nowhere,
//! so it is a constant we would be inventing rather than reproducing.
//!
//! The fold happens on the CPU, into the vertex colours, and the draw declares
//! [`benilla_world::particles::buffer::EffectLighting::None`] — the ambient is a per-draw constant,
//! and per-draw constants belong in the vertex stream on a lane whose whole design is one shared
//! buffer of them (the effect lane's own rule; it is what the `Committed` variant exists for).
//!
//! ## What is deliberately NOT here
//!
//! - **No transport ride-frame.** A committed sample is stored in absolute world coordinates and
//!   never re-projected. `0x6c67f0` takes `0x7131b0`'s world position straight into the ring and
//!   neither it nor `0x6c6560` reads `[CM2Model+0x17c]` — unlike the particle and ribbon lanes,
//!   whose birth sites (`0x7b5160`, `0x7b7bc0`) do (`crate` `benilla_world::ride_frame`, 1591).
//!   A swing on a moving deck smears in the reference too; that is the mechanism, not a gap.
//! - **No sheath gate, and no handedness test.** `0x608d60` resolves the weapon's attachment
//!   through `0x47a070(SheatheType, isMainHand)` and hangs the object wherever the weapon
//!   currently sits, so a stowed weapon is still trail-capable; and neither of `[vtbl+0x98]`'s two
//!   bodies (`0x5ec240` for a player, `0x605e30` for a creature) contains an InventoryType or
//!   handedness test. A two-hander gets one trail only because the server leaves the off-hand slot
//!   empty — which is exactly what our own off-hand slot does. **Creatures get trails too**, off
//!   `UNIT_VIRTUAL_ITEM_SLOT_DISPLAY`; this is not a player-only effect.

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use benilla_formats::TrailProc;
use benilla_world::particles::buffer::{EffectVertex, WorldEffectDraw};
use benilla_world::view::WorldCamera;

use crate::entities::HeldAttached;

/// The ring's slot count — `0x6c677c` zeroes `0x80` entries of 12 bytes, and `0x6c64a4`'s
/// `and eax,0x7f` is the write mask. Two vertices are appended per frame, so this is **64 frames**
/// of blade history.
const RING_SLOTS: u32 = 0x80;

/// The **reader's** modulus, and it is not [`RING_SLOTS`]: `0x6c64e5`/`0x6c64ea`
/// (`mov ecx,0x7f; idiv ecx`) takes `i % 127` where the writer stored at `i & 127`. The two
/// genuinely disagree in build 5875, so once the write counter passes 127 the reader picks up a
/// neighbouring sample instead of the one it wants. See [`Swing::read`] for what that looks like
/// and which kits reach it.
const READ_MODULUS: u32 = 0x7f;

/// `0x811278` in `.rdata` — `0x3b5a740e`, the nearest f32 to `1/300`, and the per-millisecond fade
/// coefficient the step multiplies by.
const FADE_PER_MS: f32 = 0.0033333334;

/// The type-8 dispatcher **arming** a unit — the two words `0x60d828`/`0x60d835` write to
/// `unit+0xd1c` / `unit+0xd20`. Written for every kit play that carries the proc, at every stage,
/// exactly where the beam case is written; consumed by [`fire_weapon_trails`].
#[derive(Message, Clone, Copy)]
pub(crate) struct TrailArm {
    /// The unit the kit played on — the wielder, never the target.
    pub(crate) entity: Entity,
    pub(crate) trail: TrailProc,
}

/// The per-unit **pending arm** — `unit+0xd1c` / `unit+0xd20` as a map, the same shape
/// [`super::creature_anim::spell_visual::MorphLatch`] gives `[unit+0xd54]`.
///
/// One slot per unit, because the reference has one pair of fields: a second proc before the
/// first has fired **overwrites** it rather than queueing. Consumed and cleared by the unit's next
/// animation (`0x5fe4b1 mov [ebx+0xd20],edi`), and swept when the unit goes.
#[derive(Resource, Default)]
pub(crate) struct TrailLatch(EntityHashMap<TrailProc>);

/// A spawned **weapon prop** whose model authors both `$WTB` and `$WTT` — the reference's
/// `WTOBJECT` (pool `0xce86d8`), one per weapon hand, alive for as long as the weapon is.
///
/// A model missing either marker gets no component at all, which is `0x6c67f0`'s own
/// `0x7130e0` presence gate (`je -> return`: absent ⇒ nothing is drawn).
#[derive(Component)]
pub(crate) struct WeaponTrail {
    /// `$WTT` — the blade tip, prop-local Bevy space (the prop root's frame, as
    /// [`crate::bowstring`] uses the same pair).
    ///
    /// The reference's per-hand index (`CGUnit+0xd34[0..1]`) is not carried: our two objects are
    /// two entities, reached through the wielder's own main/off slots, so the ordinal that
    /// distinguishes them there is the query here.
    top: Vec3,
    /// `$WTB` — the blade base.
    bottom: Vec3,
    /// The live `SWING`, or `None` when this hand is not trailing. Boxed so an idle weapon costs
    /// a pointer rather than the whole ring.
    swing: Option<Box<Swing>>,
}

impl WeaponTrail {
    /// The component a spawned weapon prop takes, given its model's baked `[top, bottom]`
    /// anchors. Called from the held-item attach.
    pub(crate) fn new(top: Vec3, bottom: Vec3) -> Self {
        Self {
            top,
            bottom,
            swing: None,
        }
    }

    /// Arm this hand — `0x6c6750`. An already-live swing **resets its ring in place** rather than
    /// allocating a second one (`0x6c675f: [eax+4] = 0 ; [eax] = 0`), so a second proc on the same
    /// weapon restarts the trail instead of stacking one on top of it.
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

/// The reference's `SWING` record (pool `0xce86ec`) — the ring of blade samples plus the colour,
/// duration and clocks that fade it out. Layout, field for field:
/// `+0x00` head · `+0x04` tail · `+0x08` the 128-slot ring · `+0x608` the packed colour
/// (byte 3 = the live alpha) · `+0x60c` duration · `+0x610` start · `+0x614` last.
struct Swing {
    /// `+0x08`. Written at `head & 0x7f`, read at `i % 127` — see [`READ_MODULUS`].
    ring: [Vec3; RING_SLOTS as usize],
    /// `+0x00` — the monotonic write cursor, never masked itself.
    head: u32,
    /// `+0x04` — the floor: the oldest sample still retained.
    tail: u32,
    /// The trail's colour, `0..=1` per channel. Constant for the swing's whole life; only the
    /// alpha ramps.
    rgb: [f32; 3],
    /// The **live** alpha byte (`+0x60b`). Not just an opacity — it is the clock: the fade rate,
    /// the retained segment count and the termination test are all derived from it.
    alpha: u8,
    /// `+0x60c` — how long the ring keeps *appending*. The trail outlives it by the fade.
    duration_ms: u32,
    /// `+0x610`.
    start_ms: u32,
    /// `+0x614` — the previous evaluation's timestamp. The step's first factor is the delta
    /// against this, **not** the elapsed time (`0x6c65c1`: feeding elapsed makes `fadeStep`
    /// explode and kills the trail after one or two frames).
    last_ms: u32,
}

/// One evaluated frame of a swing: the strip to draw, or the verdict that it is over.
enum Step {
    /// `0x6c6560` returned 0 — the caller frees the SWING and unregisters the callback.
    Ended,
    /// The retained samples, oldest first, as `(bottom, top, alpha 0..=1)` per appended frame.
    /// The **oldest** pair carries the highest alpha and the pair at the weapon carries ≈ 0.
    Strip(Vec<(Vec3, Vec3, f32)>),
}

impl Swing {
    /// `0x6c64a0` — push one sample. `slot = head & 0x7f`, then `head++`, then the floor follows
    /// the head so the ring never reports more than its capacity.
    fn push(&mut self, p: Vec3) {
        self.ring[(self.head & (RING_SLOTS - 1)) as usize] = p;
        self.head += 1;
        // `if (tail < head - 0x80) tail = head - 0x80`, on a counter that starts at 0: below the
        // capacity the right-hand side is negative and the floor stays where it is.
        self.tail = self.tail.max(self.head.saturating_sub(RING_SLOTS));
    }

    /// `0x6c6540(n)` — raise the floor so at most `n` samples remain, and report how many do.
    fn trim(&mut self, n: u32) -> u32 {
        self.tail = self.tail.max(self.head.saturating_sub(n));
        self.head - self.tail
    }

    /// Read counter `i` back out — through the reader's **`% 127`**, which is the shipped
    /// mismatch against the writer's `& 127`.
    ///
    /// For `i < 127` the two agree and the ring is exact. Past that the reader is offset: slot
    /// `s` was last written by counter `s + 128k`, so a read at `i ∈ [128, 254]` lands on the
    /// sample written **one counter later** — and since consecutive counters alternate
    /// bottom/top, that swaps the ribbon's two edges. Only a trail that appends more than 127
    /// samples reaches it: at 60 Hz that is 64 frames, so every 300–700 ms kit stops short of it
    /// and **Whirlwind's 10 000 ms spin (kits 369/370/4213) is the one that lives there.**
    fn read(&self, i: u32) -> Vec3 {
        self.ring[(i % READ_MODULUS) as usize]
    }

    /// `0x6c6560` — one frame: append while the duration holds, compute the fade step, decide
    /// whether the trail is over, trim to the segments the step affords, build the strip, then
    /// decay the live alpha past the half-duration mark.
    ///
    /// Order matters and is the reference's: the strip is coloured from the **pre-decay** alpha,
    /// and the decay happens after the draw (`0x6c669a` then `0x6c669f`).
    fn step(&mut self, now_ms: u32, bottom: Vec3, top: Vec3) -> Step {
        // `6c657c: eax = now - start` / `jae 0x6c65b9` — an unsigned test, so appending stops the
        // moment the duration is reached and the ribbon freezes where the blade left it.
        let elapsed = now_ms.wrapping_sub(self.start_ms);
        if elapsed < self.duration_ms {
            self.push(bottom); // `$WTB` first (0x6c6587) …
            self.push(top); // … then `$WTT` (0x6c65a1)
        }
        let dt = now_ms.wrapping_sub(self.last_ms);
        self.last_ms = now_ms;
        let step = fade_step(dt, self.alpha);
        // `6c65fa: eax = alpha / fadeStep` then `cmp eax,1 ; jg` — one segment or fewer and the
        // trail is over.
        let segments = u32::from(self.alpha) / step;
        if segments <= 1 {
            return Step::Ended;
        }
        let live = self.trim(2 * segments);
        if live == 0 {
            // `6c6620 je -> return 0` — an empty ring ends the trail rather than drawing nothing.
            return Step::Ended;
        }
        // `0x6c64d0`, tail → head. The alpha is decremented once per PAIR (`test bl,1`) BEFORE
        // that pair's colour is stored, so the oldest retained pair is the opaque end and the
        // pair at the weapon lands at ≈ 0. Counter-intuitive but byte-exact: do not invert it.
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
        // `6c669f: ecx = duration>>1 ; if (elapsed > duration/2) [esi+0x60b] -= fadeStep`. The
        // trail holds full strength for its first half and only then begins to go.
        if elapsed > self.duration_ms / 2 {
            self.alpha = self
                .alpha
                .saturating_sub(step.min(u32::from(u8::MAX)) as u8);
        }
        Step::Strip(strip)
    }
}

/// `fadeStep = max(1, ftol(dt_ms · (1/300) · alpha))` — `0x6c65c6`–`0x6c65f8`.
///
/// The floor is applied through `cmp al,1 / jae`, comparing only the **low byte** of the
/// truncated result: a value whose low byte is zero — 256, i.e. a ≈ 768 ms hitch at alpha 100 —
/// takes the floor of 1 rather than its own huge value, and the trail survives the stall instead
/// of ending on it. Transcribed as the byte comparison it is, not as `max(1, …)`.
fn fade_step(dt_ms: u32, alpha: u8) -> u32 {
    // `fild dt` → `fmul dword [0x811278]` → `fimul dword [alpha]` → `_ftol`. The constant is the
    // nearest f32 to 1/300 and the x87 evaluates at PC_53 with it widened, so the product runs in
    // **f64 with an f32 constant** — `dt · alpha / 300.0` is different arithmetic and is not what
    // the binary does.
    let raw = (f64::from(dt_ms) * f64::from(FADE_PER_MS) * f64::from(alpha)).trunc() as i64 as u32;
    if raw & 0xff >= 1 {
        raw
    } else {
        1
    }
}

/// Absorb this frame's arms into the latch, then fire the latch on every unit that started an
/// animation — `0x5fe48e`'s read, `0x60e550`'s fan-out to both hands, and `0x5fe4b1`'s clear.
///
/// The drain precedes the consume so a kit carrying **both** a proc-8 and an anim id self-fires in
/// the frame it plays, which is the reference's own ordering inside `0x60edf0` (dispatcher at
/// `0x60f35c`, animation at `0x60f3c5`). A kit whose anim id is `-1` — Charge's kit 44, 38 spells —
/// arms and waits: `0x60f366 jl` skips the animation leg outright, so the trail starts on whatever
/// the unit plays next, and because `0x5fe2f0` is the image's **single** animation entry point
/// that is the charge's own run, not the swing at the end of it.
///
/// **The combat fast path is honoured, and benilla got it for free.** `0x5fe43c` returns before
/// the latch read when the unit is *already* playing a combat animation and requests another one —
/// it re-times the current clip instead of restarting it — so the arm survives that call. Missing
/// that would not have invented trails out of nothing, but it would have drawn trails from arms
/// the reference **supersedes before firing** (`0x60d835` is a plain `mov` into a one-slot field,
/// so a second proc overwrites the first), with the superseded colours and durations, on 23 of the
/// 34 type-8 kits. We do not fire there because the driver's own request loop already models the
/// fast path (`select::is_combat_anim` = `0x5fcc10`'s byte-decoded set): it
/// `continue`s without playing, so neither `base_played` nor `masked_played` is raised and the
/// edge stays low. Pinned by
/// [`the fast path test`](crate::creature_anim::driver::tests).
///
/// There is **no per-frame recompute** in the reference (`0x5fd8b0` has one caller;
/// `0x5fd9e0`'s 38 sites are all event-driven), so an arm on a unit that then changes nothing sits
/// armed indefinitely and the trail starts late — at whatever event finally plays something. The
/// arm can be spent on a footstep. [`crate::creature_anim::AnimDriver::started_anim`] is the same
/// edge-triggered shape for the same reason.
pub(crate) fn fire_weapon_trails(
    time: Res<Time>,
    mut arms: MessageReader<TrailArm>,
    mut latch: ResMut<TrailLatch>,
    units: Query<(Entity, &crate::creature_anim::AnimDriver, &HeldAttached)>,
    mut trails: Query<&mut WeaponTrail>,
) {
    for arm in arms.read() {
        // One slot: a second arm before the first fires replaces it, as two writes to one field do.
        latch.0.insert(arm.entity, arm.trail);
    }
    let now_ms = time.elapsed().as_millis() as u32;
    for (entity, drv, held) in &units {
        if !drv.started_anim() {
            continue;
        }
        // `0x5fe4a0 je` — the read is gated on a nonzero duration, and `TrailProc` only decodes
        // for one, so the latch's presence IS that test. Removed either way: the clear at
        // `0x5fe4b1` is unconditional once the branch is taken.
        let Some(trail) = latch.0.remove(&entity) else {
            continue;
        };
        // `0x60e550`: `lea esi,[ecx+0xd34]` over TWO slots, skipping a null handle. Ours are the
        // main- and off-hand prop roots; a hand holding nothing, or a weapon whose model authors
        // neither marker, simply has no component to arm.
        for root in held.spawned_slots().iter().take(2).flatten() {
            if let Ok(mut t) = trails.get_mut(*root) {
                t.arm(trail, now_ms);
            }
        }
    }
    // Entries die with the unit (the reference drops the pending arm in the CGUnit teardown
    // `0x5fbb60`, eight instructions after it destroys both trail objects).
    latch.0.retain(|e, _| units.contains(*e));
}

/// Step every live swing and write its strip into the shared effect stream.
///
/// Runs post-propagation so the prop's `GlobalTransform` is *this* frame's blade pose — the
/// anchors ride the prop root's frame exactly as [`crate::bowstring`]'s do.
fn draw_weapon_trails(
    time: Res<Time>,
    mut draw: WorldEffectDraw,
    white: Res<TrailWhite>,
    lighting: Option<Res<benilla_world::lighting::WowLighting>>,
    world_cam: Query<Entity, With<WorldCamera>>,
    // The wearer's committed ambient word, present only while its light node is on the interior
    // bake lane — the trail takes its WEARER's light, not the scene's (see the module doc).
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
    // The EXTERIOR ambient — the fallback, and the right answer outdoors: a unit's own committed
    // ambient there is the day/night one (`0x69e4ad`'s exterior intensity leg). Absent before the
    // first lighting resolve — burn at the authored colour rather than at black.
    let scene = lighting.as_deref().map_or([1.0; 3], |l| l.ambient);
    let now_ms = time.elapsed().as_millis() as u32;
    for (entity, mut trail, prop, vis, wearer) in &mut trails {
        // No swing, no work — asked through `&` first: every drawn weapon carries a trail, and
        // reaching the swing through `as_mut()` below marked each one changed every frame while
        // paying the ambient lookup and two transforms for a strip that was never built.
        if trail.swing.is_none() {
            continue;
        }
        // Indoors the wearer's node carries a committed ambient word of its own — the ramped chase
        // toward `cap96(MOCV)`, the room's own light rather than the sky's. Its ABSENCE is the
        // exterior lane. A Goldshire-inn character's word is ≈ (0.30, 0.22, 0.14) warm against a
        // night sky's ≈ (0.22, 0.26, 0.29) cool: similar in magnitude, opposite in hue, which is
        // why taking the scene's outdoors-and-in tinted every indoor trail with the sky.
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
        // The vis chain is ours, not the reference's: a hidden prop has no blade on screen to
        // trail from. The swing keeps ageing above so it does not resume mid-arc when shown.
        if !vis.get() || strip.len() < 2 {
            continue;
        }
        let anchor = {
            let (b, t, _) = strip[strip.len() - 1];
            (b + t) * 0.5
        };
        // The six render states the callback writes, byte for byte (`0x6c6825`–`0x6c686e`) —
        // three of which contradict what a spell ribbon looks like it should be, and every one of
        // the three is visible:
        //
        // | EGxRs | value | |
        // |---|---|---|
        // | `0x07` | **2** | ordinary `SRC_ALPHA / INV_SRC_ALPHA` — **not** additive (that is mode 3) |
        // | `0x10` | **0** | depth test **OFF** — the arc is never eaten by the swinger's own shoulder |
        // | `0x12` | 0 | depth write off — so it occludes nothing in turn |
        // | `0x14` | 0 | two-sided (the lane never backface-culls) |
        // | `0x0f` | 0 | unfogged (the lane's default) |
        // | `0x0e` | 1 | lighting ON — see the module doc's named deviation |
        //
        // Setting `0x07` also cascades EGxRs `0x08` (`0x593741`–`0x593764`) to an alpha test of
        // `GEQUAL 1/255`, which discards the ramp's zero-alpha end. Under alpha blending a
        // zero-alpha fragment contributes nothing anyway, so it is reproduced by arithmetic.
        let mut batch = draw
            .batch(cam, white.0.id())
            .alpha()
            .over_everything()
            .anchored(anchor)
            .owner(entity);
        let verts = batch.verts_mut();
        for w in strip.windows(2) {
            let ((b0, t0, a0), (b1, t1, a1)) = (w[0], w[1]);
            // The lane indexes a quad `[0,1,2, 0,2,3]`, so perimeter order `[b0, b1, t1, t0]`
            // reproduces the strip triangles the reference's `primType 4` draws
            // (`benilla_world::ribbons` converts its own strip the same way).
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

/// The 1×1 opaque-white texel the trail samples. The reference's trail vertex is **16 bytes** —
/// a `C3Vector` and a packed colour, no texcoord (`0x6c6523 add esi,0x10`) — so the ribbon is
/// pure vertex colour; the shared effect lane always samples a texture, and white is the
/// identity for it.
#[derive(Resource)]
struct TrailWhite(Handle<Image>);

fn init_trail_white(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(TrailWhite(images.add(Image::default())));
}

/// Registers the trail lane.
pub(crate) struct WeaponTrailPlugin;

impl Plugin for WeaponTrailPlugin {
    fn build(&self, app: &mut App) {
        // The arm edge and its latch register here; `fire_weapon_trails` itself is placed inside
        // `creature_anim`'s own chain, immediately after the driver that sets the edge it reads.
        app.add_message::<TrailArm>()
            .init_resource::<TrailLatch>()
            .add_systems(Startup, init_trail_white)
            .add_systems(
                PostUpdate,
                draw_weapon_trails
                    .in_set(benilla_world::billboard::BillboardPlace)
                    .after(benilla_world::rig_anim::finalize_rig_worlds)
                    // B161's edge: a lane that writes before the frame's clear loses everything it
                    // pushed, silently, with its arithmetic perfect.
                    .after(benilla_world::particles::buffer::begin_effect_frame),
            );
    }
}

#[cfg(test)]
mod tests;
