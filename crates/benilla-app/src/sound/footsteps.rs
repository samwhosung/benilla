//! Footsteps: the `$FSD` tag through the terrain chain, `surface under the unit → TerrainType ×
//! the unit's footstep class → FootstepTerrainLookup → SoundEntries`.
//!
//! The surface is [`benilla_world::surface`]'s: a building that owns the column supplies its
//! floor's material, and only outdoors does the ADT ground-effect layer decide. `None` is the
//! reference's -1 and is silent.
//!
//! Only `$FSD` sounds (`0x5ffbd0` → `0x623390`); the per-foot side tags (`$FL`, `$FR`, `$SL`, …)
//! go to the visual footfall `0x5fbf70` ([`crate::footprints`]) and play nothing
//! ([`crate::creature_anim::is_footstep_sound`]).
//!
//! `$FSD` plays an armor foley and then the terrain step. The foley is `0x623390`'s first act after
//! the state gates (`0x6233d9 call [vt+0x8c]`), ahead of the class gate, so a class-0 creature
//! still rustles; the foley is uncapped on bus 0, the step on bus 9 with its cap of 6. The foley's
//! material:
//! - a unit (`0x623610`): `CreatureModelData.FoleyMaterialID` off its display
//!   (`[[unit+0xb3c]+0x28]`).
//! - a player (`0x62fa30`): the chest item's `Material`, through the equipment guid array at
//!   `[player+0x1d38]` element 4, which is filled only for the local player (`0x5dd454`); the
//!   private `PLAYER_FIELD_INV_SLOT_*` makes the same read self-only here.
//!
//! `Material.dbc` names a kit only for chain, plate and leather: cloth is silent.
//!
//! The footstep class is `CreatureSoundData.FootstepID` through the display→sound chain with the
//! model fallback (`benilla_formats::creature_sound`). Class 0 or no row is silent: the handler
//! bails on a zero class before any lookup (`0x6233ec`).
//!
//! Before the class, `0x623390` gates on the root unit (the rider of a mount): hover (`0x6233aa`,
//! move flag `0x4000_0000`), stealth (`0x62339c`, `BYTES_1` byte 3 bit `0x2`) and player ghost
//! (`0x6233d3`, `PLAYER_FLAGS & 0x10`).
//!
//! Wading, feet below a water surface down to the unit's swim boundary (`0.75·h`), takes the
//! lookup's splash kit, falling back to dry when the class has none; deeper, the unit swims and
//! steps are silent. The local player's wading is judged by depth too, not by its swim flag.

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::FootstepCatalog;

use crate::creature_anim::{is_footstep_sound, move_flags, AnimSoundEvent, MovementState};
use crate::entities::{CollisionHeight, Creatures};
use crate::items::Items;
use crate::net::{NetCommands, NetEntity, ObjectStore};
use crate::player::swim_enter_depth;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_protocol::EntityKind;
use benilla_world::schedule::WorldStage;

use super::creature::CreatureVoices;
use super::kit::{play_kit_ext, Bus, KitRef, PlayExtras, SoundCategory, SoundKits};
use super::{AudioListener, SoundConfig, SoundOutput};

/// The terrain-chain catalog; [`crate::footprints`] reads its `TerrainType.Flags` gate too.
#[derive(Resource)]
pub(crate) struct Footsteps(pub(crate) FootstepCatalog);

/// The foley's height above the unit's origin (`0x45851d fadd [0x801628]`), a fixed lift, not
/// model-derived; WoW's Z is Bevy's Y.
const FOLEY_HEIGHT: f32 = 2.0;

fn load_footsteps(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_footstep_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} footstep lookup rows", cat.len());
            commands.insert_resource(Footsteps(cat));
        }
        Err(e) => warn!("sound: footstep catalog failed to load: {e:#}"),
    }
}

fn footstep_sounds(
    mut events: MessageReader<AnimSoundEvent>,
    // GlobalTransform: a mount's steps fire on the mount child, whose local Transform is
    // seat-relative.
    units: Query<(&NetEntity, &GlobalTransform, Option<&CollisionHeight>)>,
    // The state gates read the root unit: `0x623390`'s `this` is the unit the mount model's
    // events are registered against (`0x607a00`), the rider.
    parents: Query<&ChildOf>,
    root_state: Query<(Option<&ObjectStore>, Option<&MovementState>)>,
    footsteps: Option<Res<Footsteps>>,
    // The foley's material table and creature catalog, one tuple for the 16-param ceiling.
    foley: (Option<Res<super::Materials>>, Option<Res<Creatures>>),
    objects: crate::net::Objects,
    mut items: Option<ResMut<Items>>,
    net_commands: Res<NetCommands>,
    voices: Option<Res<CreatureVoices>>,
    world: benilla_world::world_point::WorldPoint,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if events.is_empty() {
        return;
    }
    let (materials, creatures) = foley;
    let (Some(footsteps), Some(voices), Some(mut kits), Some(assets)) =
        (footsteps, voices, kits, assets)
    else {
        return;
    };
    let listener = listener.pos;
    for ev in events.read() {
        if !is_footstep_sound(&ev.ident) {
            continue;
        }
        let Ok((net, transform, collision)) = units.get(ev.entity) else {
            continue;
        };
        // The state gates on the root unit: hover, stealth, player ghost.
        let mut root = ev.entity;
        while let Ok(child_of) = parents.get(root) {
            root = child_of.parent();
        }
        let Ok((store, movement)) = root_state.get(root) else {
            continue;
        };
        if movement.is_some_and(|m| m.flags & move_flags::HOVER != 0) {
            continue;
        }
        if store.is_some_and(|s| s.0.unit_is_stealthed() || s.0.player_is_ghost()) {
            continue;
        }
        // The armor foley (`0x6233d9`), ahead of the gates below, on the stepping entity: a
        // mount's body rustles under the rider.
        if let (Some(materials), Some(it)) = (materials.as_deref(), items.as_mut()) {
            let material = match net.kind {
                // `0x62fa30` reads the chest through the private inv-slot array: self only.
                EntityKind::Player => {
                    super::worn_chest_material(store, &objects, it, &net_commands)
                }
                _ => net
                    .display_id
                    .and_then(|d| creatures.as_deref()?.foley_material(d)),
            };
            if let Some(kit) = material.and_then(|m| materials.0.foley_kit(m)) {
                // The unit's origin, raised, not the fired key's point (`0x45851d`).
                let mut at = transform.translation();
                at.y += FOLEY_HEIGHT;
                if let Err(e) = play_kit_ext(
                    &mut kits,
                    &assets,
                    &mut out,
                    &config,
                    listener,
                    KitRef::Id(kit),
                    Some(at),
                    SoundCategory::Sfx,
                    PlayExtras::default(), // bus 0, uncapped, volume 1.0: `0x458870`'s
                ) {
                    warn!("foley (kit {kit}): {e:#}");
                }
            }
        }
        // The voice row's class; zero or no row is silent (`0x6233ec`), with no default.
        let class = match net.display_id.and_then(|d| voices.0.for_display(d)) {
            Some(v) if v.footstep_class != 0 => v.footstep_class,
            _ => continue,
        };
        // Wading picks the splash slot; swimming (deeper than the wade ceiling) is silent.
        let wow = bevy_to_wow(transform.translation());
        // The unit's own room claim, so an indoor floor under an ADT lake is dry.
        let who = benilla_world::world_point::Subject::Unit(ev.entity);
        let depth = world
            .water_surface_at(who, wow)
            .map(|s| s - wow[2])
            .filter(|d| *d > 0.0);
        let wade_max = swim_enter_depth(collision.copied().unwrap_or_default().0);
        if depth.is_some_and(|d| d > wade_max) {
            continue;
        }
        // `None` is the reference's -1: silent, never the ground beneath a floor.
        let Some(terrain) = world.terrain_type(&footsteps.0, who, transform.translation()) else {
            continue;
        };
        let Some((dry, splash)) = footsteps.0.resolve_terrain(class, terrain) else {
            continue; // no row for this class/terrain: silent (ethereal classes)
        };
        let kit = match depth {
            Some(_) if splash != 0 => splash,
            _ => dry,
        };
        if kit == 0 {
            continue;
        }
        // Which surface answered: the kit name alone does not tell a WMO floor from the ADT.
        debug!(
            "footstep: {} terrain {terrain} class {class} kit {kit}",
            world
                .room_group(who)
                .map_or_else(|| "adt".to_string(), |g| format!("wmo g{g}"))
        );
        if let Err(e) = play_kit_ext(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(kit),
            // The step plays at the event's point: `0x623390` hands `pos` to `0x458380` as both
            // the audibility reference and the play position (`0x458434`).
            Some(ev.pos.unwrap_or_else(|| transform.translation())),
            SoundCategory::Sfx,
            PlayExtras {
                bus: Bus::FOOTSTEP,
                ..default()
            },
        ) {
            warn!("footstep (kit {kit}): {e:#}");
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, load_footsteps.after(AssetSet::Open))
        .add_systems(Update, footstep_sounds.in_set(WorldStage::Present));
}
