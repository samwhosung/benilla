//! The water splash. The reference's per-frame water decision `0x6030c0` compares
//! `surface − feet` against `0.4·collisionHeight` (`0x60314a`) and plays a positioned unit sound
//! on a crossing in either direction, waist-deep and before swimming starts at `0.75·h`.
//! Wading below the line makes only the footstep splashes.
//!
//! The reference picks Small, Medium or Large through the unit's sound emitter
//! (`[CGUnit+0xb18]`), whose selector is untraced; the Medium kit plays for everyone here.

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;

use crate::entities::CollisionHeight;
use crate::net::NetEntity;
use benilla_assets::WorldAssets;
use benilla_world::schedule::WorldStage;

use super::kit::{play_kit_ext, source_kit_playing, KitRef, PlayExtras, SoundCategory, SoundKits};
use super::{AudioListener, SoundConfig, SoundOutput};

/// SoundEntries 1096 `CharacterSplashSoundMedium`.
const SPLASH_KIT: u32 = 1096;

/// The splash line as a fraction of the unit's collision height, on depth below the surface
/// from the feet (`0x60314a`).
const SPLASH_DEPTH_FRAC: f32 = 0.4;

/// What [`water_splashes`] reads per unit; `WorldPoint` looks up the unit's room by entity.
type SplashQuery = (Entity, &'static Transform, Option<&'static CollisionHeight>);

/// Only units whose depth can have changed: depth is a function of pose and collision height,
/// so an unmoved unit cannot cross the line. A first frame counts as changed, so arming happens.
type SplashGate = Or<(Changed<Transform>, Changed<CollisionHeight>)>;

/// Play the water splash on a unit's `0.4·h` depth-line crossing, in either direction.
fn water_splashes(
    units: Query<SplashQuery, (With<NetEntity>, SplashGate)>,
    world: benilla_world::world_point::WorldPoint,
    mut wet: Local<EntityHashMap<bool>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for (entity, transform, collision) in &units {
        let wow = bevy_to_wow(transform.translation);
        // `None` only before the stamp runs; the constructor default covers it, as in the
        // reference.
        let h = collision.copied().unwrap_or_default().0;
        // By the unit's room claim, so a lake overhead does not splash a unit dry beneath it.
        let submerged = world
            .water_surface_at(benilla_world::world_point::Subject::Unit(entity), wow)
            .is_some_and(|s| s - wow[2] > SPLASH_DEPTH_FRAC * h);
        let was = wet.insert(entity, submerged);
        // Any crossing splashes; a first-seen unit arms silently. A crossing while the unit's last
        // splash still sounds is dropped, so a jump's quick double crossing splashes once, as in
        // the reference. The reference routes the splash through the unit's emitter
        // (`[CGUnit+0xb18]`); a busy emitter suppressing it is benilla's model of that.
        if was.is_some_and(|w| w != submerged) && !source_kit_playing(&out, entity, SPLASH_KIT) {
            if let Err(e) = play_kit_ext(
                &mut kits,
                &assets,
                &mut out,
                &config,
                listener,
                KitRef::Id(SPLASH_KIT),
                Some(transform.translation),
                SoundCategory::Sfx,
                PlayExtras {
                    source: Some(entity),
                    ..default()
                },
            ) {
                warn!("water splash: {e:#}");
            }
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, water_splashes.in_set(WorldStage::Present));
}
