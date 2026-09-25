//! The death thud: the body-fall sound as a corpse lands, on the `$DTH` event (`0x6236e0`); its
//! camera shake is [`crate::camera_shake`]'s (`0x625c30`). Every model that keys `$DTH` thuds,
//! players included (`[unit+0xb34]` holds a player's display too, `0x60afb0`); only the sample
//! scales with the body.
//!
//! ```text
//! $DTH → sizeClass = CreatureDisplayInfo.SizeClass ?? CreatureModelData.SizeClass
//!        terrain   = the surface under the unit → TerrainType → TerrainType.SoundID
//!        DeathThudLookups[sizeClass][terrainSound] → SoundEntries (land | water)
//! ```
//!
//! The terrain is the footstep's ([`super::footsteps`]): one cached terrain dword per unit
//! (`CGUnit+0xc60`), read by `$DTH` at `0x623749` and by `$FSD` at `0x62341d`. `None` (the
//! reference's -1) is silence.
//!
//! Most indoor floors are silent in the reference too: a WMO surface with no material is
//! `TerrainType 10 "None"`, `SoundID` 0, which `FootstepTerrainLookup` has a row for and
//! `DeathThudLookups` does not. That is 97.8 % of shipped WMO materials; stone, metal and wood
//! floors (Stormwind, Ironforge, most dungeons) thud.
//!
//! The handler's gates, in order (`0x6236e0`); there is no hover, stealth, ghost, CVar or distance
//! gate:
//!
//! 1. In liquid more than 2.0 yd over the feet (`0x62372a`): silent, not the land kit.
//! 2. Size class outside `0..=4` (`0x623744`, unsigned, so a -1 lands here): silent.
//! 3. No terrain, no lookup row or a kit of 0: silent.
//!
//! Deviation: the water column is chosen on liquid above the feet (`depth > 0`, the footstep
//! splash's test), because the reference's test, the flag saying liquid was found at the unit's
//! sampled position (`[node+0x90] & 0x20`, `0x670630`), has no depth and splashes a body on a
//! lake's shore.
//!
//! The play is `0x458870`: bus 0, uncapped (`0x458880`), at the unit's feet with no lift, not the
//! capped footstep bus. Its `1.0` multiplies the kit's `SoundEntries.Volume`, and its `-1` is a
//! file-variation index. Every `DeathThud*` row sets flag `0x400`, the ±15 % pitch draw
//! (`0x458da0`, [`super::math`]).

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::{DeathThudCatalog, FootstepCatalog};

use crate::creature_anim::AnimSoundEvent;
use crate::entities::Creatures;
use crate::net::NetEntity;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::footsteps::Footsteps;
use super::kit::{play_kit_ext, KitRef, PlayExtras, SoundCategory, SoundKits};
use super::{AudioListener, SoundConfig, SoundOutput};

/// The liquid depth over the feet that silences the thud (`0x62372a fcomp [0x801628]`), measured
/// `surfaceZ − feetZ` as the swim decision `0x6030c0` does.
const DROWNED_DEPTH: f32 = 2.0;

/// The drowned gate, the `TerrainType.SoundID` hop and the lookup; `depth` is `surfaceZ − feetZ`,
/// `None` out of liquid.
fn pick_kit(
    thuds: &DeathThudCatalog,
    steps: &FootstepCatalog,
    size_class: u32,
    terrain: u32,
    depth: Option<f32>,
) -> Option<u32> {
    if depth.is_some_and(|d| d > DROWNED_DEPTH) {
        return None; // gate 1: the body sank
    }
    let terrain_sound = steps.sound_class_of(terrain)?; // gate 3a: no TerrainType row
    thuds.kit(size_class, terrain_sound, depth.is_some()) // gate 3b: no row, or a kit of 0
}

/// `DeathThudLookups.dbc` + the `TerrainTypeSounds.dbc` domain, loaded once.
#[derive(Resource)]
pub(crate) struct DeathThuds(pub(crate) DeathThudCatalog);

fn load_death_thuds(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_death_thud_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} death-thud lookup rows", cat.len());
            commands.insert_resource(DeathThuds(cat));
        }
        Err(e) => warn!("sound: death thud catalog failed to load: {e:#}"),
    }
}

fn death_thud_sounds(
    mut events: MessageReader<AnimSoundEvent>,
    // GlobalTransform: the tag can arrive on a parented child.
    units: Query<(&NetEntity, &GlobalTransform)>,
    thuds: Option<Res<DeathThuds>>,
    // The terrain catalog; `sound_class_of` is the `TerrainType.SoundID` step.
    footsteps: Option<Res<Footsteps>>,
    creatures: Option<Res<Creatures>>,
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
    let (Some(thuds), Some(footsteps), Some(creatures), Some(mut kits), Some(assets)) =
        (thuds, footsteps, creatures, kits, assets)
    else {
        return;
    };
    let listener = listener.pos;
    for ev in events.read() {
        if &ev.ident != b"$DTH" {
            continue;
        }
        let Ok((net, transform)) = units.get(ev.entity) else {
            continue;
        };
        // Gate 2: the display's size class overrides the model's; -1 in both, or past Colossal,
        // is silent.
        let Some(size_class) = net.display_id.and_then(|d| creatures.size_class(d)) else {
            continue;
        };
        // Gate 1's input, on the unit's own room claim, so a corpse on an indoor floor under an
        // ADT lake is not in water.
        let who = benilla_world::world_point::Subject::Unit(ev.entity);
        let wow = bevy_to_wow(transform.translation());
        let depth = world
            .water_surface_at(who, wow)
            .map(|s| s - wow[2])
            .filter(|d| *d > 0.0);
        // Gate 3's input; `None` is the reference's -1: silent, never the ground beneath a floor.
        let Some(terrain) = world.terrain_type(&footsteps.0, who, transform.translation()) else {
            continue;
        };
        let Some(kit) = pick_kit(&thuds.0, &footsteps.0, size_class, terrain, depth) else {
            continue;
        };
        // Which surface answered, as the footstep logs it.
        debug!(
            "death thud: {} terrain {terrain} size {size_class}{} kit {kit}",
            world
                .room_group(who)
                .map_or_else(|| "adt".to_string(), |g| format!("wmo g{g}")),
            if depth.is_some() { " in water" } else { "" },
        );
        if let Err(e) = play_kit_ext(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(kit),
            Some(transform.translation()),
            SoundCategory::Sfx,
            PlayExtras::default(), // bus 0, uncapped, volume 1.0: `0x458870`'s
        ) {
            warn!("death thud (kit {kit}): {e:#}");
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, load_death_thuds.after(AssetSet::Open))
        .add_systems(Update, death_thud_sounds.in_set(WorldStage::Present));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three gates on the shipped tables (`benilla-extract thudcensus`): `TerrainType` 5 is
    /// Grass (`SoundID` 6), 4 Wood, 1 Metallic (`SoundID` 2, water column 0), 10 `"None"`
    /// (`SoundID` 0).
    #[test]
    fn the_three_gates_on_real_tables() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let thuds = benilla_formats::load_death_thud_catalog(&mut chain).expect("thud catalog");
        let steps = benilla_formats::load_footstep_catalog(&mut chain).expect("footstep catalog");
        let pick = |size, terrain, depth| pick_kit(&thuds, &steps, size, terrain, depth);

        // Dry land: a Colossal body on grass → `DeathThudColossalGrass`; a Small one → the Small
        // kit.
        assert_eq!(pick(4, 5, None), Some(928));
        assert_eq!(pick(0, 5, None), Some(907 + 1));

        // Wading, up to and including 2.0 yd over the feet, takes the water column.
        assert_eq!(pick(4, 5, Some(0.1)), Some(1269), "a splash, not a thud");
        assert_eq!(
            pick(4, 5, Some(DROWNED_DEPTH)),
            Some(1269),
            "exactly 2.0 still sounds"
        );
        // Deeper, the body sank: silent, not the land kit.
        assert_eq!(pick(4, 5, Some(2.01)), None);
        assert_eq!(pick(4, 5, Some(20.0)), None);

        // A water column of 0 is silent in water, never a fallback to the land kit.
        assert_eq!(
            pick(0, 1, None),
            Some(910),
            "Metallic borrows the Stone kit"
        );
        assert_eq!(pick(0, 1, Some(0.5)), None, "and nothing in the water");

        // `TerrainType "None"` resolves to sound class 0, not a `TerrainTypeSounds` row: silent.
        assert_eq!(pick(4, 10, None), None);
        // A terrain id off the table entirely, and a size class past Colossal.
        assert_eq!(pick(4, 99, None), None);
        assert_eq!(pick(5, 5, None), None);
    }

    /// A floor with no material (`TerrainType 10`) takes a footstep but never a thud; a real
    /// material still thuds. This matches the reference.
    #[test]
    fn an_unmaterialed_floor_footsteps_but_never_thuds() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let thuds = benilla_formats::load_death_thud_catalog(&mut chain).expect("thud catalog");
        let steps = benilla_formats::load_footstep_catalog(&mut chain).expect("footstep catalog");

        // The unmaterialed floor takes a footstep.
        assert_eq!(
            steps.resolve_terrain(7, 10).map(|(dry, _)| dry),
            Some(560),
            r#"a character still steps on TerrainType "None""#
        );
        for size in 0..=4 {
            assert_eq!(
                pick_kit(&thuds, &steps, size, 10, None),
                None,
                "…and no size of body thuds on it"
            );
        }
        // The control: a real material thuds at both ends of the size axis.
        assert_eq!(
            pick_kit(&thuds, &steps, 3, 4, None),
            Some(926),
            "Giant on wood"
        );
        assert_eq!(
            pick_kit(&thuds, &steps, 0, 2, None),
            Some(910),
            "Small on stone"
        );
    }
}
