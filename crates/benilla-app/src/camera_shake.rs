//! Camera shake: a heavy creature's footfall, its death thud, and spell effects, all spawned
//! through the reference's one spawner, `AddShake(id, worldPos)` (`0x511d40`).
//!
//! `CreatureModelData.FootstepShakeSize` (field 11, on the visual footfall tags, not `$FSD`) and
//! `DeathThudShakeSize` (field 12, on `$DTH`) name a `CameraShakes.dbc` preset. `SpellVisualKit`
//! field 14 and the `$SHK` event name a `SpellEffectCameraShakes.dbc` group of up to three.
//!
//! Per live record, per frame (`0x511760`/`0x5116e0`):
//!
//! ```text
//! t = (now − start) + phase                 // seconds; phase is a time pre-roll, not an angle
//! if !(t < duration) → retire               // a hard cutoff, no taper
//! A = amplitude / 36                        // the DBC column is inches
//! d² = |eye − pos|²                         // pos is snapshotted at spawn
//! if d² > 6400 → contribute nothing (the record survives)
//! if d² >   81 → A *= 0.7^((√d² − 9) / 9)
//! a = A · sin(2π · frequency · t)
//! if shake_type == 1 → a *= exp(−coefficient · t)
//! ```
//!
//! The result translates the eye along an axis of the followed unit's body frame (`direction` 0
//! forward, 1 left, 2 up), re-read every frame. Each axis keeps only its strongest live shake, so
//! same-axis shakes never sum.

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

/// Beyond 80 yd a record contributes nothing but is not retired (`0x5116e9`).
const CULL_DISTANCE_SQ: f32 = 6400.0;
/// Inside 9 yd a shake plays at full authored strength.
const FULL_DISTANCE_SQ: f32 = 81.0;
/// Beyond 9 yd the amplitude falls by 0.7 per 9 yd.
const FALLOFF_BASE: f32 = 0.7;
const FALLOFF_SPAN: f32 = 9.0;
/// `CameraShakes.Amplitude` is in inches; the client scales it at spawn (`0x511d78`).
const INCHES_TO_YARDS: f32 = 1.0 / 36.0;

/// One live shake.
struct LiveShake {
    row: CameraShake,
    /// World position, snapshotted at spawn: a shake outlives the unit that spawned it.
    pos: Vec3,
    /// App-clock seconds at spawn.
    start: f32,
}

/// The live shake set.
#[derive(Resource, Default)]
pub(crate) struct CameraShakes {
    live: Vec<LiveShake>,
}

impl CameraShakes {
    /// Enqueue a shake at a world point.
    fn add(&mut self, row: CameraShake, pos: Vec3, now: f32) {
        self.live.push(LiveShake {
            row,
            pos,
            start: now,
        });
    }

    /// Enqueue a `SpellEffectCameraShakes` group (`0x6ecb40`): its three slots in order, zeros
    /// skipped, a preset the table lacks dropped (`0x511d40`'s bounds check). A slot repeated
    /// (group 4 is `15 · 14 · 15`) enqueues twice and the copy loses the tie-break.
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

    /// Retire what has expired and compose this frame's offset. A suspended frame (`0x50ea87`,
    /// `0x50ea8b` → `0x50eb01`) yields zero and expires nothing, so a shake resumes afterwards.
    fn evaluate(&mut self, eye: Vec3, facing_yaw: f32, now: f32, suspended: bool) -> Vec3 {
        if suspended {
            return Vec3::ZERO;
        }
        // Distance decides contribution, never lifetime.
        self.live.retain(|s| s.elapsed(now) < s.row.duration);

        // One (compare key, signed value) slot per axis; walked oldest first with a strict
        // compare, so a tie keeps the older record, as the reference does.
        let mut slots: [Option<(f32, f32)>; 3] = [None; 3];
        for s in &self.live {
            let Some((key, value)) = s.sample(eye, now) else {
                continue;
            };
            let axis = s.row.direction as usize;
            let Some(slot) = slots.get_mut(axis) else {
                continue; // direction ≥ 3 writes nothing
            };
            if slot.is_none_or(|(best, _)| key > best) {
                *slot = Some((key, value));
            }
        }

        // Axes 0/1 are the followed unit's forward/left, 2 is world up; our yaw about +Y is the
        // WoW facing.
        let (sin, cos) = facing_yaw.sin_cos();
        let forward = Vec3::new(-sin, 0.0, -cos);
        let left = Vec3::new(-cos, 0.0, sin);
        let value = |i: usize| slots[i].map_or(0.0, |(_, v)| v);
        forward * value(0) + left * value(1) + Vec3::Y * value(2)
    }
}

impl LiveShake {
    /// Seconds into the shake, `phase` included, so the real life is `duration − phase`.
    fn elapsed(&self, now: f32) -> f32 {
        (now - self.start) + self.row.phase
    }

    /// This record's `(compare key, signed offset)` at `eye`; the key is the distance-attenuated
    /// amplitude, which the reference compares at `0x5118a5`.
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
        // `shake_type == 1` exactly: base-e decay, `coefficient` in 1/s.
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
            // Capture-gated with the applier (in `crate::player`), which is what retires
            // records: an emitter running without it would leak a record per footfall.
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
            continue; // `$FSD` is the sound handler's; only the visual channel shakes
        }
        let Ok((net, transform)) = units.get(ev.entity) else {
            continue;
        };
        if shk {
            // `$SHK` names a group; only the GameObject (`0x5f3e20`) and DynamicObject
            // (`0x5d58c0`) handlers decode it, so on a creature it does nothing.
            if !matches!(net.kind, EntityKind::GameObject | EntityKind::DynamicObject) {
                continue;
            }
            let Some(group) = shakes.0.group(ev.data) else {
                continue; // a payload the group table lacks; the reference bounds-checks too
            };
            // No gates; at the event's own bone-transformed point, not the object's origin.
            let at = ev.pos.unwrap_or_else(|| transform.translation());
            live.add_group(group, &shakes.0, at, now);
            continue;
        }
        // The root unit's own model row and state: a rider gets its mount's footprints but not
        // its shake (`0x607a00` writes only the decal fields).
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
            continue; // most models shake nothing
        };
        let Some(row) = shakes.0.get(id) else {
            continue;
        };
        if thud {
            // The death thud (`0x625c30`): no gates, at the unit's own position, not a bone.
            live.add(*row, transform.translation(), now);
            continue;
        }
        // The footstep gates in the reference's order (`0x5fbf70`): hover, stealth, ghost, then
        // 50 yd from the camera, outside the `showfootprints` gate.
        if movement.is_some_and(|m| m.flags & move_flags::HOVER != 0) {
            continue;
        }
        if let Some(store) = store {
            if store.0.unit_is_stealthed() || store.0.player_is_ghost() {
                continue;
            }
        }
        // The fired key's own point, as the decal derives it: 75 shipped models author some
        // 4CC more than once.
        let foot = ev.pos.unwrap_or_else(|| transform.translation());
        if footfall_culls(eye, foot) {
            continue;
        }
        live.add(*row, foot, now);
    }
}

/// Fire the shake group a spell-visual kit's field 14 names, once per kit play, with no stage test
/// and no cancel (`0x620e11`, or `0x60f4e6` when the play created no effect node).
///
/// The shake spawns at the kit play's position. The reference reads `&node+0x48` before
/// `0x62101d` writes it, so a bone-attached first node (`0x61fdd0`) shakes at the origin and is
/// culled, silencing 26 of the 58 shipped shake kits; that read-before-write is not copied.
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
            continue; // a group the table lacks; the reference bounds-checks too (`0x6ecb4d`)
        };
        live.add_group(row, &shakes.0, at, now);
    }
}

/// The followed unit's facing, move flags and live path.
type FollowedUnit = (
    &'static Transform,
    Option<&'static MovementState>,
    Option<&'static Spline>,
);

/// The reference's two suspend gates on the followed unit (`0x50ea87`, `0x50ea8b`): swimming
/// (move flag `0x200000`), or a live, not-done spline (`[[unit+0x118]+0xa4]`) carrying `0x200`,
/// the shared fly-or-swim bit (`0x616cb0`). A ground spline such as Charge does not suspend.
fn suspended(mv: Option<&MovementState>, spline: Option<&Spline>) -> bool {
    mv.is_some_and(|m| m.flags & move_flags::SWIMMING != 0) || spline.is_some_and(|s| !s.grounded)
}

/// Add this frame's shake to the seated eye.
///
/// Filtered on [`WorldCamera`], never `Camera3d`: the portrait booths are `Camera3d`s too. Runs
/// after the camera is seated, so it reads the un-shaken eye, and a zero offset writes nothing so
/// a still camera stays bit-stable. The body frame and suspend gates come from the followed unit
/// (`[cam+0x88/0x8c]`, the far-sight subject when there is one), never the active player; a
/// non-unit subject reads as not suspended, as the reference's unit-only gate (`0x50e90d`) does.
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

    /// `CameraShakes.dbc` row 1, the kodo's footstep, verbatim.
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
    /// Row 2, the giants' and dragons' footstep.
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

    /// Row 1 opens at `sin(2π·3·0.06) = +0.905`, so a footstep kicks the eye up first.
    #[test]
    fn phase_is_a_time_preroll_and_the_first_kick_is_upward() {
        let expected = (2.0 / 36.0) * (TAU * 3.0 * 0.06).sin() * (-0.06f32).exp();
        let got = vertical(FOOTSTEP_1, 0.0, 0.0);
        assert!((got - expected).abs() < 1e-6, "{got} vs {expected}");
        assert!(got > 0.0, "the first kick is UP, not down: {got}");
        let as_angle = (2.0 / 36.0) * 0.06f32.sin();
        assert!(got > as_angle * 10.0, "phase must not be read as an angle");
    }

    #[test]
    fn phase_shortens_the_life() {
        let mut s = at(FOOTSTEP_1, Vec3::ZERO);
        // 0.33 + 0.06 = 0.39 < 0.4: alive.
        assert!(s.evaluate(Vec3::ZERO, 0.0, 0.33, false).y != 0.0);
        assert_eq!(s.live.len(), 1);
        // 0.35 + 0.06 = 0.41 >= 0.4: gone.
        assert_eq!(s.evaluate(Vec3::ZERO, 0.0, 0.35, false), Vec3::ZERO);
        assert!(s.live.is_empty(), "retired at duration − phase");
    }

    #[test]
    fn duration_is_a_cutoff_not_a_taper() {
        let undecayed = CameraShake {
            shake_type: 0,
            phase: 0.0,
            duration: 1.0,
            frequency: 0.25, // a quarter cycle at t = 1: the sine's peak
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

    #[test]
    fn same_axis_shakes_do_not_sum() {
        let mut s = CameraShakes::default();
        s.add(FOOTSTEP_1, Vec3::ZERO, 0.0); // amplitude 2
        s.add(FOOTSTEP_2, Vec3::ZERO, 0.0); // amplitude 7, wins
        let both = s.evaluate(Vec3::ZERO, 0.0, 0.0, false).y;
        let alone = vertical(FOOTSTEP_2, 0.0, 0.0);
        assert!(
            (both - alone).abs() < 1e-6,
            "the loser is dropped, not added: {both} vs {alone}"
        );
        let mut t = CameraShakes::default();
        t.add(FOOTSTEP_1, Vec3::ZERO, 0.0);
        t.add(FOOTSTEP_1, Vec3::X * 40.0, 0.0); // same row, farther: a smaller key
        assert_eq!(t.evaluate(Vec3::ZERO, 0.0, 0.0, false).y, alone / 3.5);
    }

    /// 2 is up, 0 is forward, 1 is left, so turning rotates an in-flight horizontal shake.
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

    /// Group 1's presets `4 · 5 · 6`, one per axis, verbatim.
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

    /// The three spell presets and the groups the tests below name.
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
            // Group 3's shape: one slot, two zeros.
            .with_group(SpellShakeGroup {
                id: 3,
                slots: [6, 0, 0],
            })
            // A preset the table lacks, beside one it has.
            .with_group(SpellShakeGroup {
                id: 9,
                slots: [4, 999, 0],
            })
    }

    #[test]
    fn a_group_spawns_one_shake_per_populated_slot() {
        let table = spell_catalog();
        let mut s = CameraShakes::default();
        s.add_group(table.group(1).unwrap(), &table, Vec3::ZERO, 0.0);
        assert_eq!(s.live.len(), 3, "one record per populated slot");

        // 4 → forward, 5 → left, 6 → up.
        let at = 0.25;
        let offset = s.evaluate(Vec3::ZERO, 0.0, at, false);
        assert!(
            offset.z != 0.0 && offset.x != 0.0 && offset.y != 0.0,
            "{offset:?}"
        );
        // Each axis carries only its own preset's value.
        assert_eq!(
            offset.y,
            vertical(SPELL_6, 0.0, at),
            "axis 2 is preset 6's alone"
        );
    }

    /// The reference's own `test/je` and `cmp ecx,[maxId]`.
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

    /// The shipped groups 4 and 7 name a preset twice (`15 · 14 · 15`).
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

    #[test]
    fn swimming_freezes_rather_than_retires() {
        let mut s = at(FOOTSTEP_1, Vec3::ZERO);
        assert_eq!(s.evaluate(Vec3::ZERO, 0.0, 10.0, true), Vec3::ZERO);
        assert_eq!(s.live.len(), 1, "long past its duration, still not retired");
        assert_eq!(s.evaluate(Vec3::ZERO, 0.0, 10.0, false), Vec3::ZERO);
        assert!(s.live.is_empty(), "and it retires the moment we surface");
    }

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
