//! **The death thud** — the body-fall impact as a corpse lands, fired on the `$DTH` animation
//! event (`0x6236e0`). The *other* half of the same event: its camera shake is
//! [`crate::camera_shake`]'s (`0x625c30`), and the two are siblings, not one system — the shake
//! is authored per model in `CreatureModelData` and only 49 of the 430 shipped models carry one,
//! while the **sound is universal**: **329 of the 407 creature-model files the archives hold** key
//! a `$DTH` (`benilla-extract thudcensus`; 430 rows collapse to 410 distinct paths, 407 of which
//! ship), and only the *sample* scales with the body. **Players included** — every character model keys one,
//! and `[unit+0xb34]` is filled from `UNIT_FIELD_DISPLAYID` for a player exactly as for a creature
//! (`0x60afb0`), so your own body lands audibly too.
//!
//! ```text
//! $DTH → sizeClass = CreatureDisplayInfo.SizeClass ?? CreatureModelData.SizeClass
//!        terrain   = the surface under the unit → TerrainType → TerrainType.SoundID
//!        DeathThudLookups[sizeClass][terrainSound] → SoundEntries (land | water)
//! ```
//!
//! The terrain leg is **exactly the footstep sound's** ([`super::footsteps`]) — the reference
//! keeps one cached terrain-type dword per unit (`CGUnit+0xc60`) and the `$DTH` handler reads that
//! same dword at `0x623749`, as the `$FSD` handler does at `0x62341d`. `None` (the client's `−1`)
//! is silence here too, never "ask the other leg".
//!
//! ## Indoors is silent, and that is the data
//!
//! The two consumers share the terrain id and then **diverge on it**, which is the single most
//! surprising thing about this system. Both tables are keyed on `TerrainType.SoundID`, and
//! `TerrainType 10 "None"` — the id a WMO surface takes when its `MOMT+0x20` says nothing — has
//! `SoundID = 0`. `FootstepTerrainLookup` carries a row at terrain sound **0** for 17 footstep
//! classes; `DeathThudLookups` carries **none at all**. So the same floor that creaks underfoot
//! swallows the body that lands on it.
//!
//! That is not a rounding error in the content: `benilla-extract thudcensus` measures **10 075 of
//! the 10 299 shipped MOMT materials (97.8 %) at `TerrainType 10`**, and **694 of the 815 WMO
//! roots have no surface that can thud at all** — every inn and tavern in the game among them.
//! What *does* thud indoors is the blacksmiths, barns, barracks, chapels, Stormwind, Ironforge and
//! most dungeon sets, whose floors carry a real Stone/Metallic/Wood material.
//!
//! So "a big corpse hit the ground silently *indoors*" is the **reference's own behaviour**, not a
//! gap — and the retest for this system has to be run outdoors, or in one of the 121 buildings the
//! census names.
//!
//! **Three gates, and no others.** The handler's early-returns in order (`0x6236e0`):
//!
//! 1. **In liquid, and more than `2.0` yd of it over the corpse's feet** (`0x62372a`, the float
//!    at `0x801628` — the same dword the armor foley lifts its emitter by) ⇒ **completely
//!    silent**. Not "play the land one": a body that sinks makes no sound at all.
//! 2. **Size class outside `0..=4`** (`0x623744`, an *unsigned* `>= 5`, so a `−1` from both DBC
//!    rows lands here) ⇒ silent. No shipped display reaches it.
//! 3. **No terrain, or no lookup row** ⇒ silent, and a resolved kit of `0` is silence too.
//!
//! And that is the whole list. Unlike the footstep it does **not** gate on hover, stealth,
//! player-ghost, a CVar or distance — the same "no gates at all" the sibling camera shake has.
//! Nor does it walk to a root unit: a `$DTH` is a corpse's own collapse, and nothing rides a
//! dying mount.
//!
//! **One deliberate deviation, on gate 1's *other* half.** The reference picks the water column
//! on "the unit's liquid node reports liquid" alone (`[node+0x90] & 0x20`, `0x670630`) — a flag
//! with no depth test of its own, so a body lying on a beach beside a lake would splash. benilla
//! asks the question the footstep splash already asks: is the surface **above the feet**
//! (`depth > 0`). Same answer wherever the reference is right, and the shore case is the one
//! place they differ. (`0x670630`'s own semantics are still open; if the node flag turns out
//! to already mean "submerged", the two collapse into one.)
//!
//! The play is `0x458870` — **bus 0, uncapped** (`0x458880 xor ecx,ecx`; cap `0x7FFFFFFF`), 3D at
//! the unit's own position origin (its feet, `vtbl+0x14`), with no Z lift. That is the same call
//! the armor foley makes, and deliberately *not* the capped [`Bus::FOOTSTEP`] the step itself
//! takes: a battlefield's worth of bodies all land. The `1.0` it passes is a **multiplier** on the
//! kit's authored `SoundEntries.Volume`, not the final gain — the shipped rows carry 0.4 for
//! Small/Medium, 1.0 for Large/Giant/Colossal, 0.3/0.6 in water — and the `-1` beside it is a
//! **file-variation index** (`variant != -1`), not a channel. Every `DeathThud*` row sets flag
//! `0x400`, so the kit player's own ±15 % pitch draw applies (`0x458da0`, [`super::math`]).

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

/// **The depth that silences the thud** — `0x62372a fcomp [0x801628]`, the float `2.0`. Read as
/// `surfaceZ − feetZ`, the same depth the swim decision (`0x6030c0`) measures, so this is "two
/// yards of water over the body's feet", not two yards of body under the surface.
const DROWNED_DEPTH: f32 = 2.0;

/// **The whole decision**, given what the world has already answered: the drowned gate, the
/// `TerrainType.SoundID` hop, and the join. `depth` is `surfaceZ − feetZ` and `None` when the
/// corpse is not in liquid — the same shape [`super::footsteps`] wades on.
///
/// Pure, so the three gates can be exercised against the real tables without a running client.
fn pick_kit(
    thuds: &DeathThudCatalog,
    steps: &FootstepCatalog,
    size_class: u32,
    terrain: u32,
    depth: Option<f32>,
) -> Option<u32> {
    if depth.is_some_and(|d| d > DROWNED_DEPTH) {
        return None; // gate 1 — the body sank
    }
    let terrain_sound = steps.sound_class_of(terrain)?; // gate 3a — no such TerrainType row
    thuds.kit(size_class, terrain_sound, depth.is_some()) // gate 3b — no row, or a kit of 0
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
    // GlobalTransform for the same reason every other anim-event consumer takes one: the tag can
    // arrive on a parented child whose local Transform is not a world position.
    units: Query<(&NetEntity, &GlobalTransform)>,
    thuds: Option<Res<DeathThuds>>,
    // The terrain leg's own catalog — `world.terrain_type` resolves the ground-effect hop through
    // it, and `sound_class_of` is the `TerrainType.SoundID` step the reference does inline.
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
        // Gate 2 — the size class. The display's column overrides the model's; both `−1` (or a
        // missing row) is silence, as is anything past Colossal.
        let Some(size_class) = net.display_id.and_then(|d| creatures.size_class(d)) else {
            continue;
        };
        // The liquid over the body (gate 1's input). `water_surface_at` is the unit's own room
        // claim, so a corpse on an indoor floor under an ADT lake is not "in water".
        let who = benilla_world::world_point::Subject::Unit(ev.entity);
        let wow = bevy_to_wow(transform.translation());
        let depth = world
            .water_surface_at(who, wow)
            .map(|s| s - wow[2])
            .filter(|d| *d > 0.0);
        // Gate 3's input — the terrain leg. `None` is the client's `−1`: silent, and never a
        // reason to fall back to the ground beneath a floor.
        let Some(terrain) = world.terrain_type(&footsteps.0, who, transform.translation()) else {
            continue;
        };
        let Some(kit) = pick_kit(&thuds.0, &footsteps.0, size_class, terrain, depth) else {
            continue;
        };
        // Which leg answered and what it said — the same triage line the footstep prints, because
        // the wrong-surface family is invisible in the kit name alone once two legs can produce one.
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
            PlayExtras::default(), // bus 0, uncapped, volume 1.0 — `0x458870`'s own
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

    /// The three gates on the real shipped tables — the join a corpse actually walks.
    ///
    /// Anchors, all read off `benilla-extract thudcensus`: `TerrainType 5` is Grass, whose
    /// `SoundID` is 6; `TerrainType 4` is Wood; `TerrainType 1` is Metallic (`SoundID` 2), the
    /// row whose water column is 0; `TerrainType 10` is the unauthored `"None"`, `SoundID` 0.
    #[test]
    fn the_three_gates_on_real_tables() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let thuds = benilla_formats::load_death_thud_catalog(&mut chain).expect("thud catalog");
        let steps = benilla_formats::load_footstep_catalog(&mut chain).expect("footstep catalog");
        let pick = |size, terrain, depth| pick_kit(&thuds, &steps, size, terrain, depth);

        // Dry land: a Colossal body on grass → `DeathThudColossalGrass`; a Small one → the Small
        // kit. The size axis is the whole reason the sound is not one sample.
        assert_eq!(pick(4, 5, None), Some(928));
        assert_eq!(pick(0, 5, None), Some(907 + 1));

        // Wading — up to and including 2.0 yd over the feet — takes the water column.
        assert_eq!(pick(4, 5, Some(0.1)), Some(1269), "a splash, not a thud");
        assert_eq!(
            pick(4, 5, Some(DROWNED_DEPTH)),
            Some(1269),
            "exactly 2.0 still sounds"
        );
        // Deeper, the body sank: silent outright, NOT the land kit.
        assert_eq!(pick(4, 5, Some(2.01)), None);
        assert_eq!(pick(4, 5, Some(20.0)), None);

        // A terrain whose water column is 0 is silent in water and audible out of it — the same
        // "0 is silence, never a fallback" the table half of the census prints as `—`.
        assert_eq!(
            pick(0, 1, None),
            Some(910),
            "Metallic borrows the Stone kit"
        );
        assert_eq!(pick(0, 1, Some(0.5)), None, "and nothing in the water");

        // `TerrainType "None"` (a WMO floor with no material) resolves to sound class 0, which is
        // not a `TerrainTypeSounds` row: silent, on any body.
        assert_eq!(pick(4, 10, None), None);
        // A terrain id off the table entirely, and a size class past Colossal.
        assert_eq!(pick(4, 99, None), None);
        assert_eq!(pick(5, 5, None), None);
    }

    /// **Indoors is silent, and the footstep on the same floor is not** — the asymmetry in the
    /// module docs, pinned as an executable claim because it is the one a future reader will
    /// disbelieve and "fix".
    ///
    /// `TerrainType 10 "None"` is 97.8 % of shipped WMO material, and it resolves to terrain sound
    /// `0`. `FootstepTerrainLookup` has a row there; `DeathThudLookups` has none. A building whose
    /// floor carries a real material (a blacksmith's stone, a barn's wood) thuds normally — that
    /// is the control, and it must not change.
    #[test]
    fn an_unmaterialed_floor_footsteps_but_never_thuds() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let thuds = benilla_formats::load_death_thud_catalog(&mut chain).expect("thud catalog");
        let steps = benilla_formats::load_footstep_catalog(&mut chain).expect("footstep catalog");

        // The unmaterialed floor: a footstep, on every body, at any size.
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
        // The control — a floor with a real material still thuds, at both ends of the size axis.
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
