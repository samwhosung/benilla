//! The above-water liquid ambient loops: ocean, river, lava and slime beds as 3D loops over the
//! zone ambience, as the reference's driver `0x462b50` (groups `0xb230b8`) plays them. Liquid near
//! the player, not the camera, arms its class's loop; the class is the cell's MCLQ/MLIQ low
//! nibble, mapped to a kit by `SoundWaterType.dbc`. Submerging stops them hard
//! (`0x458650` → `0x462e10` → `0x462b10`); resurfacing restarts them at full volume.
//!
//! The nearest point is the footprint's AABB clamp where the reference walks cells, so a fade-in
//! can lead by a couple of yards on L-shaped shores; the tick is a frame. The reference's own
//! `MapWaterSounds` switch (registered at `0x462a40`) is not built; the loops ride the ambience
//! bucket, so `EnableAmbience` and the ambience slider gate them.

use bevy::prelude::*;

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_formats::WaterSoundCatalog;

use crate::net::Embodied;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::kit::{
    self, play_kit_ext, set_source_kit_gain, source_kit_playing, stop_source_kit, KitRef,
    PlayExtras, SoundCategory, SoundKits,
};
use super::{SoundConfig, SoundOutput};

/// The scan radius (yd) around the player (`0x462b50`).
const TRIGGER_RADIUS: f32 = 9.0;
/// The emitter's slew step per tick (yd, `0x462960`).
const SLEW_PER_TICK: f32 = 0.166_67;
/// The emitter's near-field clamp (yd): √2·4.16667, the cell diagonal.
const NEAR_CLAMP: f32 = 5.892_557;
/// Fade in and out (s): `[0x80355c]`/`[0x803560]`.
const FADE_SECS: f32 = 5.0;
/// Concurrent class loops, by priority River > Ocean > Magma > Slime.
const MAX_CONCURRENT: usize = 2;

/// The `SoundWaterType.dbc` class → kit map.
#[derive(Resource)]
pub(super) struct WaterSounds(WaterSoundCatalog);

/// One class's armed loop.
struct ClassLoop {
    /// The slewed emitter; the pump's tracked-follow reads its `Transform`.
    emitter: Entity,
    kit: u32,
    /// The pump-lane gain, 0 → 1 on arm and 1 → 0 on leave; the channel stops at 0.
    gain: f32,
    /// A superseded kit (the nearest cell's speed nibble changed) fading out on the same
    /// emitter: `(kit, gain)`.
    retiring: Option<(u32, f32)>,
}

/// The four class slots (index `nibble & 3`) and the submerge edge latch.
#[derive(Resource, Default)]
struct LiquidLoopState {
    classes: [Option<ClassLoop>; 4],
    was_underwater: bool,
}

fn load_water_sounds(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_water_sound_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} SoundWaterType rows", cat.len());
            commands.insert_resource(WaterSounds(cat));
        }
        Err(e) => warn!("sound: SoundWaterType failed to load: {e:#}"),
    }
}

/// The per-frame driver: scan, arm or retire by priority, slew, fade.
fn drive_liquid_loops(
    mut state: ResMut<LiquidLoopState>,
    water_sounds: Option<Res<WaterSounds>>,
    world: benilla_world::world_point::WorldPoint,
    player: Query<&Transform, With<Embodied>>,
    mut emitters: Query<&mut Transform, Without<Embodied>>,
    time: Res<Time>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<super::AudioListener>,
    mut commands: Commands,
) {
    let (Some(water_sounds), Some(mut kits), Some(assets)) = (water_sounds, kits, assets) else {
        return;
    };
    // No audio device: a loop that fails to start is never held, so it would retry and warn every
    // frame. The startup warning already covers it.
    if out.mixer.is_none() {
        return;
    }

    // Submerged: a hard stop with no fade, and the resurface edge restarts at full.
    if world.submersion().is_water() {
        for slot in &mut state.classes {
            if let Some(cl) = slot.take() {
                stop_source_kit(&mut out, cl.emitter, cl.kit);
                if let Some((old, _)) = cl.retiring {
                    stop_source_kit(&mut out, cl.emitter, old);
                }
            }
        }
        state.was_underwater = true;
        return;
    }
    let resurfaced = std::mem::take(&mut state.was_underwater);

    let Ok(player_tf) = player.single() else {
        return; // no avatar yet
    };
    let player_pos = player_tf.translation;
    let player_wow = bevy_to_wow(player_pos);

    // The nearest wet point per class within the radius (the reference walk is `0x6723f0`).
    let best = world.nearest_liquid_per_class(player_wow, TRIGGER_RADIUS);

    // The class indices are the priority order.
    let mut budget = MAX_CONCURRENT;
    let dt = time.delta_secs();
    let fade_step = if FADE_SECS > 0.0 { dt / FADE_SECS } else { 1.0 };

    for (class, scanned) in best.into_iter().enumerate() {
        let candidate = scanned.filter(|_| budget > 0);
        let desired_kit = candidate
            .as_ref()
            .and_then(|c| water_sounds.0.kit_for_nibble(c.nibble));
        if desired_kit.is_some() {
            budget -= 1;
        }

        let slot = &mut state.classes[class];
        match (slot.as_mut(), desired_kit) {
            (None, Some(kit_id)) => {
                // Arm: spawn the emitter at the near-clamped nearest point and start the loop.
                let point = candidate.expect("desired_kit implies a candidate").point;
                let pos = near_clamped(wow_to_bevy(point), player_pos);
                let emitter = commands.spawn((Transform::from_translation(pos),)).id();
                let gain = if resurfaced { 1.0 } else { 0.0 };
                start_loop(
                    &mut kits,
                    &assets,
                    &mut out,
                    &config,
                    listener.pos,
                    kit_id,
                    pos,
                    emitter,
                    gain,
                );
                *slot = Some(ClassLoop {
                    emitter,
                    kit: kit_id,
                    gain,
                    retiring: None,
                });
            }
            (Some(cl), Some(kit_id)) => {
                if cl.kit != kit_id {
                    // The speed nibble changed (still → fast river): the old kit fades out on
                    // the same emitter while the new one fades in.
                    if let Some((old, g)) = cl.retiring.take() {
                        // A second swap mid-fade drops the oldest outright.
                        let _ = g;
                        stop_source_kit(&mut out, cl.emitter, old);
                    }
                    cl.retiring = Some((cl.kit, cl.gain));
                    cl.kit = kit_id;
                    cl.gain = 0.0;
                }
                // Slew the emitter toward the nearest point.
                let point = candidate.expect("desired_kit implies a candidate").point;
                if let Ok(mut tf) = emitters.get_mut(cl.emitter) {
                    let target = near_clamped(wow_to_bevy(point), player_pos);
                    let step = target - tf.translation;
                    let len = step.length();
                    tf.translation += if len > SLEW_PER_TICK {
                        step * (SLEW_PER_TICK / len)
                    } else {
                        step
                    };
                }
                // Fade in (at full on the resurface edge), and re-arm a channel the device
                // dropped.
                cl.gain = if resurfaced {
                    1.0
                } else {
                    (cl.gain + fade_step).min(1.0)
                };
                if !source_kit_playing(&out, cl.emitter, cl.kit) {
                    let pos = emitters
                        .get(cl.emitter)
                        .map(|t| t.translation)
                        .unwrap_or(player_pos);
                    start_loop(
                        &mut kits,
                        &assets,
                        &mut out,
                        &config,
                        listener.pos,
                        cl.kit,
                        pos,
                        cl.emitter,
                        cl.gain,
                    );
                }
                set_source_kit_gain(&mut out, cl.emitter, cl.kit, cl.gain);
            }
            (Some(cl), None) => {
                // Out of range or out-prioritised: fade out, then stop.
                cl.gain -= fade_step;
                if cl.gain <= 0.0 {
                    stop_source_kit(&mut out, cl.emitter, cl.kit);
                    if let Some((old, _)) = cl.retiring.take() {
                        stop_source_kit(&mut out, cl.emitter, old);
                    }
                    let emitter = cl.emitter;
                    *slot = None;
                    commands.entity(emitter).despawn();
                } else {
                    set_source_kit_gain(&mut out, cl.emitter, cl.kit, cl.gain);
                }
            }
            (None, None) => {}
        }

        // The retiring kit's fade-out, independent of the live one.
        if let Some(cl) = state.classes[class].as_mut() {
            if let Some((old, mut g)) = cl.retiring.take() {
                g -= fade_step;
                if g <= 0.0 {
                    stop_source_kit(&mut out, cl.emitter, old);
                } else {
                    set_source_kit_gain(&mut out, cl.emitter, old, g);
                    cl.retiring = Some((old, g));
                }
            }
        }
    }
}

/// Keep the emitter at least [`NEAR_CLAMP`] from the player, pushed out along player → emitter.
fn near_clamped(pos: Vec3, player: Vec3) -> Vec3 {
    let d = pos - player;
    let len = d.length();
    if len >= NEAR_CLAMP {
        pos
    } else if len > 1e-4 {
        player + d * (NEAR_CLAMP / len)
    } else {
        player + Vec3::X * NEAR_CLAMP
    }
}

/// Start one class loop on the ambience bucket, tagged to its emitter, at an initial gain.
/// Force-looped: the type-22 kits are all authored loops but the lava pool's lacks the `0x200`
/// flag; how the reference loops that one is untraced.
fn start_loop(
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    listener: Vec3,
    kit_id: u32,
    pos: Vec3,
    emitter: Entity,
    gain: f32,
) {
    if let Err(e) = play_kit_ext(
        kits,
        assets,
        out,
        config,
        listener,
        KitRef::Id(kit_id),
        Some(pos),
        SoundCategory::Ambience,
        PlayExtras {
            source: Some(emitter),
            force_loop: true,
            ..default()
        },
    ) {
        warn!("liquid loop kit {kit_id}: {e:#}");
        return;
    }
    set_source_kit_gain(out, emitter, kit_id, gain);
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<LiquidLoopState>()
        .add_systems(Startup, load_water_sounds.after(AssetSet::Open))
        .add_systems(
            Update,
            // Before the channel pump, which applies this frame's gains and positions.
            drive_liquid_loops
                .in_set(WorldStage::Present)
                .before(kit::pump_channels),
        );
}
