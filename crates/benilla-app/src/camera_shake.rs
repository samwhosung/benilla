//! **Camera shake** — the thump a heavy creature's footfall puts through the camera, the one-off
//! jolt as its body lands, and the jolt a spell effect puts through it (B298; decisions 1540/1849).
//!
//! **Four producers, two id spaces, one evaluator.** Every producer ends at the same spawner —
//! `AddShake(id, worldPos)` (`0x511d40`), whose five call sites are the whole population (proven by
//! xref sweep: no address-dword reference to it anywhere, and no Lua, console, cinematic or quake
//! path).
//!
//! The **creature** pair names a `CameraShakes.dbc` preset **directly**:
//!
//! - **`CreatureModelData.FootstepShakeSize`** (field 11) → fired from the **visual** footfall
//!   channel — the per-foot plant tags (`$FL0`/`$FR0`/`$RL0`/`$RR0`…), the same stream
//!   [`crate::footprints`] reads. **Not `$FSD`**, which is the sound handler's alone.
//! - **`CreatureModelData.DeathThudShakeSize`** (field 12) → the same table, fired on `$DTH`.
//!
//! The **spell** pair names a `SpellEffectCameraShakes.dbc` **group** — up to three presets at one
//! point ([`benilla_formats::SpellShakeGroup`]):
//!
//! - **`SpellVisualKit` field 14** (`kit+0x38`, the `ShakeID` column) → once per kit play, via
//!   [`SpellKitShake`] and [`fire_kit_shakes`], which carries the mechanism and the one deviation.
//! - **the `$SHK` animation event** → its payload *is* a group id, fired ungated at the event's own
//!   bone-transformed point — the fired key's own, carried on the event (decision 1904).
//!
//! **`$SHK` is decoded by exactly two handlers**, hanging off the GameObject (typemask `0x20`,
//! `0x5f3e20`) and DynamicObject (`0x40`, `0x5d58c0`) trampolines — the dword `0x4b485324` occurs
//! twice image-wide, and of the six dispatchers `0x7133a0` registers only those two decode it.
//! **`CGUnit_C::HandleAnimEvent` is not one of them**, so a `$SHK` on a creature M2 logs
//! `UNHANDLEDANIMEVENT` and does nothing — which is not academic: CryptLord and Thunderaan both
//! author markers on their death/emerge clips and the reference plays neither. That gate is why no
//! creature shake can be `$SHK`-driven, and it is enforced in [`fire_shakes`].
//!
//! Only 25 of the 430 shipped models carry a footstep shake, and the set is exactly the
//! thumping-giant list; 49 carry one of the two, and 58 of the 1772 kits carry a shake group. See
//! `benilla-extract shakecensus`, which maps all four producers onto the 24 presets.
//!
//! ## The law
//!
//! **Emission — the footstep**, gated in this order (`0x5fbf70`): not hovering · `BYTES_1` byte 3
//! bit `0x2` clear (our stealth bit) · not a player-ghost · **`|camera − footplant|² ≤ 2500`
//! (50 yd)**. The shake sits *outside* the `showfootprints` CVar gate and *outside* the 25 yd gate
//! that follows it, so it reaches twice as far as a footprint decal and does not turn off with
//! them. The position is the planted foot's world point, exactly as the decal derives it.
//!
//! **Emission — the death thud** (`0x625c30`): **no gates at all** — no hover, no ghost, no
//! distance, no CVar — at the unit's own world position.
//!
//! **Per live record, per frame** (`0x511760`/`0x5116e0`):
//!
//! ```text
//! t = (now − start) + phase                 // seconds; phase is a TIME pre-roll, not an angle
//! if !(t < duration) → retire
//! A = amplitude / 36                        // the DBC column is inches; yards on the wire out
//! d² = |eye − pos|²                         // pos is snapshotted at spawn
//! if d² > 6400 → contribute nothing (the record survives)
//! if d² >   81 → A *= 0.7^((√d² − 9) / 9)
//! a = A · sin(2π · frequency · t)
//! if shake_type == 1 → a *= exp(−coefficient · t)
//! ```
//!
//! Three corrections to the conventional column map, all from the consumer: **`Phase` is a time
//! pre-roll in seconds**, not an angle — it advances the sine *and* the decay, and shortens the
//! real life to `duration − phase`; **`Duration` is a hard cutoff with no taper**, so a
//! `shake_type == 0` row is cut off mid-swing at full strength; and **`ShakeType` is a one-bit
//! decay switch**, not a type — `== 1` exactly enables the `exp` envelope, and `Coefficient` is
//! that rate in 1/s and is dead data on every `shake_type == 0` row. The decay is base **e**.
//!
//! **What moves.** A pure world-space **translation of the eye** — no rotation, no FOV change; the
//! look-at target is rebuilt as `eye + forward`, so it rides along. `Direction` selects an axis in
//! the **followed unit's body frame**, re-read every frame, which is why turning the player
//! rotates an in-flight horizontal shake:
//!
//! | `direction` | reference (WoW axes) | ours (Bevy) |
//! |---|---|---|
//! | 0 | `(a·cos φ, a·sin φ, 0)` | `a ·` forward |
//! | 1 | `(−a·sin φ, a·cos φ, 0)` | `a ·` left |
//! | 2 | `(0, 0, a)` | `a ·` up |
//!
//! (`bevy = (−wow.y, wow.z, −wow.x)` — decision 0002 — and our yaw about `+Y` equals the WoW
//! facing, so axis 0/1 land exactly on the unit's forward/left.) **Every creature row is
//! `direction = 2`**, so in practice a footstep shake is purely vertical; the other two axes are
//! reachable only from the spell-side table and are implemented for completeness. Axes 0 and 1 are
//! *not* independent — both sum into the horizontal plane.
//!
//! **Combination.** Three slots, one per axis, each keeping the single strongest live shake — same-axis
//! shakes do **not** sum, the losers are dropped outright, and at most three shakes play in a frame.
//! This is what a spell **group** is shaped for: its three slots name the `direction` 0·1·2 members
//! of one preset family, so they land on three different axes and all three contribute. They are
//! slots and not axes, though — group 4 is `15 · 14 · 15`, and the duplicate loses the tie-break.
//! Ties keep the incumbent, and the walk is oldest→newest, so ties keep the **older** record.
//!
//! **Lifetime.** A record holds a *snapshot* position and no reference to its source, so a shake
//! fully outlives the creature that spawned it. The whole evaluation is skipped — offset zero,
//! nothing expiring — while the **followed unit** (the far-sight subject when there is one, our own
//! body otherwise) is **swimming** or riding a **fly-or-swim spline**: the reference's two suspend
//! gates, both of which jump the same block.
//!
//! Not modelled, deliberately: the reference's second wholesale free on `SetTarget(nullptr)`, and
//! its destructor's unlink-without-free (a shipped leak — our `Vec` cannot reproduce it).

use std::f32::consts::TAU;

use bevy::prelude::*;

use benilla_formats::{CameraShake, CameraShakeCatalog, SpellShakeGroup};

use crate::creature_anim::{
    footfall_culls, footfall_side, move_flags, AnimSoundEvent, MovementState, SpellKitShake,
};
use crate::entities::Creatures;
use crate::net::{Embodied, NetEntity, ObjectStore, Spline};
use crate::player::ViewSubject;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_protocol::EntityKind;
use benilla_world::schedule::WorldStage;
use benilla_world::view::WorldCamera;

/// Beyond this the record contributes nothing but is **not** retired (`0x5116e9`, `80 yd²`).
const CULL_DISTANCE_SQ: f32 = 6400.0;
/// Inside this the shake plays at full authored strength (`9 yd²`).
const FULL_DISTANCE_SQ: f32 = 81.0;
/// The falloff's half-life base and its per-9-yd exponent divisor.
const FALLOFF_BASE: f32 = 0.7;
const FALLOFF_SPAN: f32 = 9.0;
/// `CameraShakes.Amplitude` is authored in inches; the client scales it at spawn (`0x511d78`).
const INCHES_TO_YARDS: f32 = 1.0 / 36.0;

/// One live shake: the authored row, where it happened, and when it started.
struct LiveShake {
    row: CameraShake,
    /// World position, **snapshotted at spawn** — the record never looks at its source again.
    pos: Vec3,
    /// App-clock seconds at spawn.
    start: f32,
}

/// The live shake set and the eye offset it composes for this frame.
#[derive(Resource, Default)]
pub(crate) struct CameraShakes {
    live: Vec<LiveShake>,
}

impl CameraShakes {
    /// Enqueue a shake at a world point. The row is copied in whole — the DBC is the only source
    /// of shape, and nothing downstream re-reads the catalog.
    fn add(&mut self, row: CameraShake, pos: Vec3, now: f32) {
        self.live.push(LiveShake {
            row,
            pos,
            start: now,
        });
    }

    /// Enqueue a whole `SpellEffectCameraShakes` **group** at a world point — the spell side's
    /// only spawn shape (`0x6ecb40`, byte-read 2026-09-02): walk the three slots `+0x4/+0x8/+0xc`
    /// in ascending order, skip the zeros, and `AddShake(id, pos)` each. A slot the preset table
    /// does not carry is dropped by the same bounds check the reference makes (`0x511d40`'s
    /// `cmp ecx,[maxId]`); a group naming the same preset twice (group 4 is `15 · 14 · 15`)
    /// enqueues it twice and the duplicate simply loses [`Self::evaluate`]'s strict-`>` tie-break.
    fn add_group(
        &mut self,
        group: &SpellShakeGroup,
        table: &CameraShakeCatalog,
        pos: Vec3,
        now: f32,
    ) {
        for row in group.shakes().filter_map(|id| table.get(id)) {
            self.add(*row, pos, now);
        }
    }

    /// Retire what has expired and compose this frame's offset.
    ///
    /// `suspended` is the reference's skip — **two** gates that both jump the same block
    /// (`0x50ea87` / `0x50ea8b` → `0x50eb01`): the followed unit
    /// is **swimming** (`[[unit+0x118]+0x40] & 0x200000`), or it is riding a **flying server
    /// spline** (`MI = [[unit+0x118]+0xa4]` non-null, not done, Flying bit set) — a taxi flight.
    /// The accumulator is zeroed *before* both gates, so a suspended frame yields the offset zero
    /// **and expires nothing**: a shake resumes mid-flight on surfacing or landing, rather than
    /// having quietly aged out while the camera was not reading it.
    fn evaluate(&mut self, eye: Vec3, facing_yaw: f32, now: f32, suspended: bool) -> Vec3 {
        if suspended {
            return Vec3::ZERO;
        }
        // Retire on the *unattenuated* clock: distance decides contribution, never lifetime.
        self.live.retain(|s| s.elapsed(now) < s.row.duration);

        // One slot per axis, each holding (compare key, signed value). The walk is oldest→newest
        // and the compare is strict, so a tie keeps the older record — the reference's own
        // `jne`-skips-the-write shape.
        let mut slots: [Option<(f32, f32)>; 3] = [None; 3];
        for s in &self.live {
            let Some((key, value)) = s.sample(eye, now) else {
                continue;
            };
            let axis = s.row.direction as usize;
            let Some(slot) = slots.get_mut(axis) else {
                continue; // direction ≥ 3 writes nothing (and corrupts the reference's frame)
            };
            if slot.is_none_or(|(best, _)| key > best) {
                *slot = Some((key, value));
            }
        }

        // Axis 0/1 are the followed unit's forward/left, 2 is world up (module doc's table). Our
        // yaw about +Y is the WoW facing, so forward is exactly `(−sin, 0, −cos)`.
        let (sin, cos) = facing_yaw.sin_cos();
        let forward = Vec3::new(-sin, 0.0, -cos);
        let left = Vec3::new(-cos, 0.0, sin);
        let value = |i: usize| slots[i].map_or(0.0, |(_, v)| v);
        forward * value(0) + left * value(1) + Vec3::Y * value(2)
    }
}

impl LiveShake {
    /// Seconds into the shake, including `phase` — which is a **time pre-roll**, so it advances
    /// both the sine and the decay and shortens the real life to `duration − phase`.
    fn elapsed(&self, now: f32) -> f32 {
        (now - self.start) + self.row.phase
    }

    /// This record's `(compare key, signed offset)` at `eye`, or `None` when out of cull range.
    ///
    /// The compare key is the **distance-attenuated amplitude** — a per-record quantity that does
    /// not swing with the sine — the stored slot the reference compares at `0x5118a5`.
    fn sample(&self, eye: Vec3, now: f32) -> Option<(f32, f32)> {
        let d2 = eye.distance_squared(self.pos);
        if d2 > CULL_DISTANCE_SQ {
            return None;
        }
        let mut amp = self.row.amplitude * INCHES_TO_YARDS;
        if d2 > FULL_DISTANCE_SQ {
            amp *= FALLOFF_BASE.powf((d2.sqrt() - FALLOFF_SPAN) / FALLOFF_SPAN);
        }
        let t = self.elapsed(now);
        let mut value = amp * (TAU * self.row.frequency * t).sin();
        // `shake_type == 1` exactly — the one-bit decay switch, base e, rate in 1/s.
        if self.row.shake_type == 1 {
            value *= (-self.row.coefficient * t).exp();
        }
        Some((amp, value))
    }
}

pub(crate) struct CameraShakePlugin;

impl Plugin for CameraShakePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraShakes>()
            .add_systems(Startup, load_shakes.after(AssetSet::Open))
            // Present, like the footprints and the footstep sounds: the same event stream, after
            // the frame's animation drive and transforms have settled the foot bones.
            //
            // Capture-gated in step with the applier ([`crate::player`] schedules that one). The
            // pairing is not cosmetic: the applier is what retires expired records, so an emitter
            // still running while it is gated off would push a record per footfall that nothing
            // ever reaps — a slow leak across a long capture.
            .add_systems(
                Update,
                (fire_shakes, fire_kit_shakes)
                    .in_set(WorldStage::Present)
                    .run_if(not(resource_exists::<crate::run_mode::CaptureMode>)),
            );
    }
}

/// Read `CameraShakes.dbc` into its catalog resource.
fn load_shakes(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let table = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_camera_shakes(&mut chain)
    };
    match table {
        Ok(t) => {
            debug!("camera_shake: {} presets", t.len());
            commands.insert_resource(Shakes(t));
        }
        Err(e) => warn!("camera_shake: CameraShakes.dbc failed to load: {e:#}"),
    }
}

/// `CameraShakes.dbc`, loaded once.
#[derive(Resource)]
struct Shakes(CameraShakeCatalog);

/// Enqueue a shake for each qualifying footfall and death thud on the frame's event stream.
fn fire_shakes(
    mut events: MessageReader<AnimSoundEvent>,
    time: Res<Time>,
    units: Query<(&NetEntity, &GlobalTransform)>,
    parents: Query<&ChildOf>,
    roots: Query<(
        Option<&ObjectStore>,
        Option<&MovementState>,
        Option<&NetEntity>,
    )>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    creatures: Option<Res<Creatures>>,
    shakes: Option<Res<Shakes>>,
    mut live: ResMut<CameraShakes>,
) {
    if events.is_empty() {
        return;
    }
    let (Some(creatures), Some(shakes)) = (creatures, shakes) else {
        return;
    };
    let Ok(eye) = camera.single().map(|t| t.translation()) else {
        return;
    };
    let now = time.elapsed_secs();
    for ev in events.read() {
        let thud = &ev.ident == b"$DTH";
        let shk = &ev.ident == b"$SHK";
        if !thud && !shk && footfall_side(&ev.ident).is_none() {
            continue; // `$FSD` is the sound handler's; only the VISUAL channel shakes
        }
        let Ok((net, transform)) = units.get(ev.entity) else {
            continue;
        };
        if shk {
            // `$SHK`: the payload is a **group** id, and the tag is decoded by exactly two
            // handlers — the GameObject (`0x5f3e20`, typemask 0x20) and DynamicObject
            // (`0x5d58c0`, 0x40) dispatchers. `CGUnit_C::HandleAnimEvent` decodes no `$SHK`
            // (the dword occurs twice image-wide), so a creature's marker is inert and this gate
            // is what keeps it that way: CryptLord and Thunderaan both author one on their
            // death/emerge clips, and the reference plays neither.
            if !matches!(net.kind, EntityKind::GameObject | EntityKind::DynamicObject) {
                continue;
            }
            let Some(group) = shakes.0.group(ev.data) else {
                continue; // a payload the group table lacks — the reference bounds-checks too
            };
            // **No gates at all** — no distance, visibility, CVar or state test (the sibling
            // `$DSL` arm and the footstep path both have them; this one has none). The position is
            // the event's own bone-transformed point, not the object's origin.
            let at = ev.pos.unwrap_or_else(|| transform.translation());
            live.add_group(group, &shakes.0, at, now);
            continue;
        }
        // The shake reads the ROOT unit's own model row — a mount's row is never stored there, so
        // a rider on a kodo gets the kodo's *footprints* and not its shake (`0x607a00` writes only
        // the decal fields). The root is also where the state gates live.
        let mut root = ev.entity;
        while let Ok(child_of) = parents.get(root) {
            root = child_of.parent();
        }
        let Ok((store, movement, root_net)) = roots.get(root) else {
            continue;
        };
        let display = root_net.unwrap_or(net).display_id;
        let Some(id) = display.and_then(|d| {
            if thud {
                creatures.death_thud_shake(d)
            } else {
                creatures.footstep_shake(d)
            }
        }) else {
            continue; // the overwhelming majority of models shake nothing
        };
        let Some(row) = shakes.0.get(id) else {
            continue;
        };
        if thud {
            // No gates whatsoever, and the unit's own world position — not a bone.
            live.add(*row, transform.translation(), now);
            continue;
        }
        // The footstep's three state gates, in the reference's order.
        if movement.is_some_and(|m| m.flags & move_flags::HOVER != 0) {
            continue;
        }
        if let Some(store) = store {
            if store.0.unit_is_stealthed() || store.0.player_is_ghost() {
                continue;
            }
        }
        // The planted foot: the FIRED key's own point, resolved once by the scanner exactly as
        // the kernel snapshots it (decision 1904), not a by-4CC re-find of the marker table. The
        // difference is real — 75 shipped models author some 4CC more than once at different
        // points — and it is the same quantity the decal derives.
        let foot = ev.pos.unwrap_or_else(|| transform.translation());
        if footfall_culls(eye, foot) {
            continue;
        }
        live.add(*row, foot, now);
    }
}

/// Fire the camera shake a spell-visual kit's field 14 names — the **spell-side producer**
/// (decision 1849), the counterpart of `crate::sound::spell`'s kit-sound route.
///
/// One shake per kit play, unconditionally: the reference reaches it from the first-created effect
/// node's one-time arm pass (`0x620e11`, gated on that node's flags snapshot carrying bit `0x10`)
/// and falls back to the kit tail (`0x60f4e6`) only when the play created no node at all. Neither
/// site tests the stage, and there is no cancel path — a shake outlives its kit.
///
/// **One deliberate deviation, and it is the whole reason this doc paragraph exists.** The
/// reference's primary site passes `&node+0x48` — a position field the bone-attached node
/// constructor `0x61fdd0` **never writes**, and which the site that would write it (`0x62101d`)
/// fills twelve instructions *later*, for the sound. So in the real client a kit whose first
/// created node is bone-attached shakes at `(0, 0, 0)` and is culled by the 80 yd cut: **26 of the
/// 58 shipped shake kits are silently dead**, including every summon (kit 138) and Goblin Sapper
/// Charge (1424). That is a read-before-write of a zero-initialised field, not a design — the
/// value it wants is unambiguous, being the same one the sound leg uses a moment later — so we
/// spawn at the kit play's real position and the authored data plays. benilla implements the
/// mechanism, not the quirk (`docs/METHOD.md` §3, `docs/METHOD.md` step 3); the reference's own numbers are
/// in `decisions/1849`, and reverting to them would mean deliberately modelling an uninitialised
/// field we do not have.
fn fire_kit_shakes(
    mut events: MessageReader<SpellKitShake>,
    time: Res<Time>,
    transforms: Query<&GlobalTransform>,
    shakes: Option<Res<Shakes>>,
    mut live: ResMut<CameraShakes>,
) {
    if events.is_empty() {
        return;
    }
    let Some(shakes) = shakes else { return };
    let now = time.elapsed_secs();
    for ev in events.read() {
        let (group, at) = match *ev {
            SpellKitShake::Play { entity, group } => {
                let Ok(t) = transforms.get(entity) else {
                    continue; // the unit went away between the kit play and this drain
                };
                (group, t.translation())
            }
            SpellKitShake::PlayAt { pos, group } => (group, pos),
        };
        let Some(row) = shakes.0.group(group) else {
            continue; // a kit naming a group the table lacks — none ship, the bounds check is the
                      // reference's own (`0x6ecb4d`)
        };
        live.add_group(row, &shakes.0, at, now);
    }
}

/// The applier's read of the followed unit: its facing (the shake's body frame), its move flags
/// (the swim half of the reference's skip) and its live path (the spline half).
type FollowedUnit = (
    &'static Transform,
    Option<&'static MovementState>,
    Option<&'static Spline>,
);

/// Is the whole shake block skipped this frame? The reference's two suspend gates, OR'd — both
/// `jmp` past the same block (`0x50ea87` / `0x50ea8b` → `0x50eb01`), on the **followed unit**.
///
/// **Swimming** is its move flags (`[[unit+0x118]+0x40] & 0x200000`).
///
/// **The spline gate** is read through to the LIVE path, exactly as the anim selector's `unify`
/// does ([`crate::creature_anim`], `0x5fd19c`) — the reference re-reads
/// `[[unit+0x118]+0xa4]` every frame rather than trusting a stamped flag, and our stored
/// [`MovementState::flying`] is the selector's own derived value, left `false` on the controller's
/// component. The [`Spline`]'s *presence* is the reference's "descriptor non-null **and** not done"
/// (`sample_splines` removes the component the frame the path completes — the `& 0x4` DONE term),
/// and `!grounded` is spline flag `0x200`.
///
/// **`0x200` is the shared fly-*or*-swim spline bit, not "taxi"** (`0x616cb0`) —
/// so this arm covers a swimming server path as well as a flight, which is the same reason it
/// sits beside a swim gate at all. Naming it "the taxi arm" (as 1540 did, and as this was first
/// built) over-specifies what the bit means. A **ground** spline — Charge, knockback, a fleeing
/// walk, all of which install the same component — does not suspend anything.
fn suspended(mv: Option<&MovementState>, spline: Option<&Spline>) -> bool {
    mv.is_some_and(|m| m.flags & move_flags::SWIMMING != 0) || spline.is_some_and(|s| !s.grounded)
}

/// Add this frame's shake to the camera's seated eye.
///
/// Filtered on [`WorldCamera`], never on `Camera3d`: the portrait booths are `Camera3d`s too, and a
/// bare `Camera3d` query here would shake an off-screen booth — the same trap that once yanked the
/// booths to the scenario eye and blanked every portrait. `WorldCamera` and `FlyCam` sit on the one
/// entity `control` seats, which is the entity this must add to.
///
/// Runs **after** the camera is seated, which is what makes the falloff honest: `control` rewrites
/// the base pose every frame, so the transform this reads is the un-shaken eye rather than last
/// frame's shaken one. A zero offset writes nothing at all, preserving the camera's bit-equality
/// no-op gate (decision 1362) — a still camera stays bit-stable and its propagation stays quiet.
///
/// **The body frame and both suspend gates come from the FOLLOWED unit** — `[cam+0x88/0x8c]` in
/// the reference, never `0x468550` (the active player). Ordinarily they are the same object; under
/// far sight (Mind Vision, Eye of Kilrogg, and possession, which rides the same field) they are
/// not, and the camera orbits the subject while our body walks on somewhere else. Reading our own
/// body there would frame the shake on the wrong facing and suspend it on the wrong unit's swim
/// state. [`ViewSubject`] already resolves exactly this object for the rig.
///
/// The reference additionally requires the followed object to be **TYPEMASK_UNIT**
/// (`0x50e90d`–`0x50e92c`) before reading its `CMovement` at all. That falls out here: the
/// fallback is our own body, and a resolved far-sight subject with no [`MovementState`] and no
/// [`Spline`] — a GameObject or a DynamicObject — reads as "not suspended", which is what a
/// unit-only gate on a non-unit yields.
pub(crate) fn apply_camera_shake(
    mut live: ResMut<CameraShakes>,
    time: Res<Time>,
    mut camera: Query<&mut Transform, With<WorldCamera>>,
    subject: Res<ViewSubject>,
    units: Query<FollowedUnit, Without<WorldCamera>>,
    body: Query<Entity, (With<Embodied>, Without<WorldCamera>)>,
) {
    let Ok(mut cam) = camera.single_mut() else {
        return;
    };
    let followed = subject
        .remote
        .map(|r| r.entity)
        .or_else(|| body.single().ok());
    let (yaw, suspended) = followed
        .and_then(|e| units.get(e).ok())
        .map_or((0.0, false), |(t, mv, spline)| {
            (t.rotation.to_euler(EulerRot::YXZ).0, suspended(mv, spline))
        });
    let offset = live.evaluate(cam.translation, yaw, time.elapsed_secs(), suspended);
    if offset != Vec3::ZERO {
        cam.translation += offset;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `CameraShakes.dbc` row **1** — the Ancient Protector's / kodo's footstep, verbatim.
    const FOOTSTEP_1: CameraShake = CameraShake {
        id: 1,
        shake_type: 1,
        direction: 2,
        amplitude: 2.0,
        frequency: 3.0,
        duration: 0.4,
        phase: 0.06,
        coefficient: 1.0,
    };
    /// Row **2** — the Ancient of Lore/War, the giants and the dragons. Amplitude 7.0.
    const FOOTSTEP_2: CameraShake = CameraShake {
        amplitude: 7.0,
        ..FOOTSTEP_1
    };

    fn at(row: CameraShake, pos: Vec3) -> CameraShakes {
        let mut s = CameraShakes::default();
        s.add(row, pos, 0.0);
        s
    }

    /// The signed vertical offset a shake produces, with the eye at `d` yards from it.
    fn vertical(row: CameraShake, d: f32, t: f32) -> f32 {
        at(row, Vec3::ZERO).evaluate(Vec3::X * d, 0.0, t, false).y
    }

    /// `Phase` is a **time pre-roll in seconds**, not an angle: at `t = 0` the sine is already
    /// advanced to `2π·f·phase`. Row 1 opens at `sin(2π·3·0.06) = +0.905`, so a footstep kicks the
    /// eye UP first — the verdict's own observable.
    #[test]
    fn phase_is_a_time_preroll_and_the_first_kick_is_upward() {
        let expected = (2.0 / 36.0) * (TAU * 3.0 * 0.06).sin() * (-0.06f32).exp();
        let got = vertical(FOOTSTEP_1, 0.0, 0.0);
        assert!((got - expected).abs() < 1e-6, "{got} vs {expected}");
        assert!(got > 0.0, "the first kick is UP, not down: {got}");
        // Read as an angle instead, the opening sample would be sin(0.06) ≈ 0.06 — 15× smaller.
        let as_angle = (2.0 / 36.0) * 0.06f32.sin();
        assert!(got > as_angle * 10.0, "phase must not be read as an angle");
    }

    /// `Phase` also shortens the real life: the record retires at `elapsed + phase >= duration`.
    #[test]
    fn phase_shortens_the_life() {
        let mut s = at(FOOTSTEP_1, Vec3::ZERO);
        // 0.33 s in: 0.33 + 0.06 = 0.39 < 0.4, still alive.
        assert!(s.evaluate(Vec3::ZERO, 0.0, 0.33, false).y != 0.0);
        assert_eq!(s.live.len(), 1);
        // 0.35 s in: 0.41 >= 0.4 — gone, a full 0.05 s before its nominal duration.
        assert_eq!(s.evaluate(Vec3::ZERO, 0.0, 0.35, false), Vec3::ZERO);
        assert!(s.live.is_empty(), "retired at duration − phase");
    }

    /// `Duration` is a hard cutoff with **no taper** — a `shake_type == 0` row is still at full
    /// authored strength on its last frame. (The creature rows all decay; this is the spell shape.)
    #[test]
    fn duration_is_a_cutoff_not_a_taper() {
        let undecayed = CameraShake {
            shake_type: 0,
            phase: 0.0,
            duration: 1.0,
            frequency: 0.25, // a quarter-cycle at t = 1 → sine near its peak
            ..FOOTSTEP_1
        };
        let last = vertical(undecayed, 0.0, 0.999);
        let peak = 2.0 / 36.0;
        assert!(
            (last - peak).abs() < 1e-3,
            "no taper: {last} should still be ~{peak}"
        );
        assert_eq!(
            vertical(undecayed, 0.0, 1.0),
            0.0,
            "and then it is simply gone"
        );
    }

    /// The decay is base **e** at `coefficient` 1/s, and only when `shake_type == 1`.
    #[test]
    fn the_decay_switch_is_one_bit() {
        let t = 0.2;
        let decayed = vertical(FOOTSTEP_1, 0.0, t);
        let plain = vertical(
            CameraShake {
                shake_type: 0,
                ..FOOTSTEP_1
            },
            0.0,
            t,
        );
        let ratio = decayed / plain;
        let expected = (-(t + 0.06)).exp();
        assert!(
            (ratio - expected).abs() < 1e-5,
            "base-e decay on the full elapsed (phase included): {ratio} vs {expected}"
        );
    }

    /// Full strength inside 9 yd; `0.7^((d−9)/9)` beyond; **nothing past 80 yd — but the record
    /// survives**, so walking back into range resumes it.
    #[test]
    fn the_distance_falloff_culls_without_retiring() {
        let near = vertical(FOOTSTEP_2, 0.0, 0.0);
        assert_eq!(
            vertical(FOOTSTEP_2, 9.0, 0.0),
            near,
            "≤ 9 yd is full strength"
        );
        let far = vertical(FOOTSTEP_2, 18.0, 0.0);
        assert!(
            (far / near - 0.7f32).abs() < 1e-5,
            "one 9-yd span out = ×0.7, got {}",
            far / near
        );
        let mut s = at(FOOTSTEP_2, Vec3::ZERO);
        assert_eq!(s.evaluate(Vec3::X * 81.0, 0.0, 0.0, false), Vec3::ZERO);
        assert_eq!(s.live.len(), 1, "culled, not retired");
    }

    /// Same-axis shakes do **not** sum — the strongest wins outright — and the key is the
    /// distance-attenuated amplitude, so a tie keeps the OLDER record.
    #[test]
    fn same_axis_shakes_do_not_sum() {
        let mut s = CameraShakes::default();
        s.add(FOOTSTEP_1, Vec3::ZERO, 0.0); // amplitude 2
        s.add(FOOTSTEP_2, Vec3::ZERO, 0.0); // amplitude 7 — wins
        let both = s.evaluate(Vec3::ZERO, 0.0, 0.0, false).y;
        let alone = vertical(FOOTSTEP_2, 0.0, 0.0);
        assert!(
            (both - alone).abs() < 1e-6,
            "the loser is dropped, not added: {both} vs {alone}"
        );
        // A tie keeps the incumbent, which is the older — the reference's jne-skips-the-write.
        let mut t = CameraShakes::default();
        t.add(FOOTSTEP_1, Vec3::ZERO, 0.0);
        t.add(FOOTSTEP_1, Vec3::X * 40.0, 0.0); // same row, further away ⇒ strictly smaller key
        assert_eq!(t.evaluate(Vec3::ZERO, 0.0, 0.0, false).y, alone / 3.5);
    }

    /// `Direction` is an axis in the followed unit's **body frame**: 2 is up, 0 is forward, 1 is
    /// left — so turning the player rotates an in-flight horizontal shake.
    #[test]
    fn direction_selects_the_body_frame_axis() {
        let up = at(FOOTSTEP_1, Vec3::ZERO).evaluate(Vec3::ZERO, 0.0, 0.0, false);
        assert!(
            up.x.abs() < 1e-7 && up.z.abs() < 1e-7 && up.y > 0.0,
            "{up:?}"
        );

        let surge = CameraShake {
            direction: 0,
            ..FOOTSTEP_1
        };
        // Yaw 0: Bevy forward is −Z.
        let f = at(surge, Vec3::ZERO).evaluate(Vec3::ZERO, 0.0, 0.0, false);
        assert!(f.y.abs() < 1e-7 && f.x.abs() < 1e-6 && f.z < 0.0, "{f:?}");
        // Yaw +90°: forward becomes −X.
        let turned =
            at(surge, Vec3::ZERO).evaluate(Vec3::ZERO, std::f32::consts::FRAC_PI_2, 0.0, false);
        assert!(turned.z.abs() < 1e-6 && turned.x < 0.0, "{turned:?}");

        let sway = CameraShake {
            direction: 1,
            ..FOOTSTEP_1
        };
        let l = at(sway, Vec3::ZERO).evaluate(Vec3::ZERO, 0.0, 0.0, false);
        assert!(l.y.abs() < 1e-7 && l.z.abs() < 1e-6 && l.x < 0.0, "{l:?}");
    }

    /// The three spell rows a `SpellEffectCameraShakes` group names, one per axis — group 1's
    /// `4 · 5 · 6`, verbatim.
    const SPELL_4: CameraShake = CameraShake {
        id: 4,
        shake_type: 0,
        direction: 0,
        amplitude: 2.0,
        frequency: 6.0,
        duration: 4.0,
        phase: 0.0,
        coefficient: 0.4,
    };
    const SPELL_5: CameraShake = CameraShake {
        id: 5,
        direction: 1,
        frequency: 4.0,
        ..SPELL_4
    };
    const SPELL_6: CameraShake = CameraShake {
        id: 6,
        direction: 2,
        amplitude: 4.0,
        frequency: 5.2,
        ..SPELL_4
    };

    /// A catalog holding the three spell presets plus the two groups the tests below name.
    fn spell_catalog() -> CameraShakeCatalog {
        CameraShakeCatalog::default()
            .with_row(SPELL_4)
            .with_row(SPELL_5)
            .with_row(SPELL_6)
            // Group 1 as shipped: one preset per axis.
            .with_group(SpellShakeGroup {
                id: 1,
                slots: [4, 5, 6],
            })
            // Group 3's shape: one populated slot, two zeros.
            .with_group(SpellShakeGroup {
                id: 3,
                slots: [6, 0, 0],
            })
            // A group naming a preset the table lacks, beside one it has — the reference's own
            // bounds check drops the first and keeps the second.
            .with_group(SpellShakeGroup {
                id: 9,
                slots: [4, 999, 0],
            })
    }

    /// A **group** is the spell side's whole spawn shape: three slots walked in order, zeros
    /// skipped, each landing on its own axis — so one group play moves the eye on all three.
    #[test]
    fn a_group_spawns_one_shake_per_populated_slot() {
        let table = spell_catalog();
        let mut s = CameraShakes::default();
        s.add_group(table.group(1).unwrap(), &table, Vec3::ZERO, 0.0);
        assert_eq!(s.live.len(), 3, "one record per populated slot");

        // A quarter-second in, all three axes are moving: 4 → forward, 5 → left, 6 → up.
        let at = 0.25;
        let offset = s.evaluate(Vec3::ZERO, 0.0, at, false);
        assert!(
            offset.z != 0.0 && offset.x != 0.0 && offset.y != 0.0,
            "{offset:?}"
        );
        // And each axis carries exactly its own preset's value — the slots do not sum.
        assert_eq!(
            offset.y,
            vertical(SPELL_6, 0.0, at),
            "axis 2 is preset 6's alone"
        );
    }

    /// Zeros are skipped and a slot the preset table does not carry is dropped — the reference's
    /// own `test/je` and its `cmp ecx,[maxId]`. Neither is an error; both just contribute nothing.
    #[test]
    fn a_group_skips_zero_slots_and_unknown_presets() {
        let table = spell_catalog();
        let mut one = CameraShakes::default();
        one.add_group(table.group(3).unwrap(), &table, Vec3::ZERO, 0.0);
        assert_eq!(one.live.len(), 1, "two zero slots contribute nothing");

        let mut dangling = CameraShakes::default();
        dangling.add_group(table.group(9).unwrap(), &table, Vec3::ZERO, 0.0);
        assert_eq!(dangling.live.len(), 1, "the id the table lacks is dropped");
        assert_eq!(dangling.live[0].row.id, 4);
    }

    /// The shipped groups 4 and 7 name the **same preset twice** (`15 · 14 · 15`), which is the
    /// proof the three slots are slots and not axes. Both records spawn; the duplicate loses the
    /// strict-`>` tie-break, so the axis reads exactly as it would with one.
    #[test]
    fn a_duplicate_slot_spawns_but_never_doubles_the_offset() {
        let table = spell_catalog();
        let mut s = CameraShakes::default();
        s.add_group(
            &SpellShakeGroup {
                id: 4,
                slots: [6, 5, 6],
            },
            &table,
            Vec3::ZERO,
            0.0,
        );
        assert_eq!(s.live.len(), 3, "the duplicate is a real record");
        let at = 0.25;
        assert_eq!(
            s.evaluate(Vec3::ZERO, 0.0, at, false).y,
            vertical(SPELL_6, 0.0, at),
            "the duplicate contributes nothing on top of its twin"
        );
    }

    /// A `direction` the reference cannot index writes nothing (rather than panicking on our side).
    #[test]
    fn an_out_of_range_direction_contributes_nothing() {
        let bad = CameraShake {
            direction: 3,
            ..FOOTSTEP_1
        };
        assert_eq!(
            at(bad, Vec3::ZERO).evaluate(Vec3::ZERO, 0.0, 0.0, false),
            Vec3::ZERO
        );
    }

    /// A suspended frame bypasses the whole block: zero offset, and **nothing expires** — the
    /// shake resumes on surfacing (or on landing off a taxi).
    #[test]
    fn swimming_freezes_rather_than_retires() {
        let mut s = at(FOOTSTEP_1, Vec3::ZERO);
        assert_eq!(s.evaluate(Vec3::ZERO, 0.0, 10.0, true), Vec3::ZERO);
        assert_eq!(s.live.len(), 1, "long past its duration, still not retired");
        assert_eq!(s.evaluate(Vec3::ZERO, 0.0, 10.0, false), Vec3::ZERO);
        assert!(s.live.is_empty(), "and it retires the moment we surface");
    }

    /// The two suspend gates are independent and either one alone suspends — and a **ground**
    /// spline (a charge, a knockback, a fleeing walk) is not one of them: only spline flag
    /// `0x200`, the shared fly-or-swim bit, is.
    #[test]
    fn either_gate_suspends_and_a_ground_spline_does_not() {
        let swimming = MovementState {
            flags: move_flags::SWIMMING,
            ..default()
        };
        let path = |grounded: bool| Spline {
            deck: None,
            points: vec![[0.0; 3], [1.0, 0.0, 0.0]],
            start: std::time::Instant::now(),
            duration: std::time::Duration::from_secs(1),
            id: 1,
            grounded,
            run_mode: true,
        };

        assert!(!suspended(None, None), "walking about: the shake plays");
        assert!(!suspended(Some(&default()), None));
        assert!(suspended(Some(&swimming), None), "swimming");
        assert!(suspended(None, Some(&path(false))), "a fly-or-swim path");
        assert!(
            !suspended(None, Some(&path(true))),
            "a ground spline still shakes — Charge is not a taxi"
        );
        assert!(suspended(Some(&swimming), Some(&path(false))), "both");
    }
}
