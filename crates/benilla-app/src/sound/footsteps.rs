//! Footsteps (decision 0070 slice 3): the `$FSD` anim tag resolved through the terrain-type
//! chain — `surface under the unit → TerrainType → × the unit's footstep class →
//! FootstepTerrainLookup → SoundEntries`.
//!
//! **The surface is [`benilla_world::surface`]'s to answer, not this module's**. It is the
//! client's two-leg down-ray: a building that owns the column supplies its own floor's material,
//! and only outdoors does the ADT ground-effect layer decide. Reading the ADT unconditionally is
//! what put a snow crunch under the Kharanos inn's floorboards. `None` there is the reference's
//! `−1` and means silent — never "ask the other leg".
//!
//! **Trigger: `$FSD` and nothing else**. A footfall is two disjoint channels in
//! the client's event dispatcher `0x5ffbd0` — `$FSD → 0x623390` is the *sound*, the per-foot side
//! tags (`$FL/$FR/$RL/$RR/$SL/$SR/$BL/$BR/$WL/$WR`) `→ 0x5fbf70` are the *visual* footfall (the
//! decal + spray, [`crate::footprints`]) and play nothing. Reading both as steps rang **every gait
//! at double rate** — HumanMale's Walk
//! keys `$FR0 · $FSD · $FL0 · $FSD`, a horse's gallop four of each per 0.8 s cycle — and made the
//! turn-in-place shuffle, whose *only* keys are `$SL0 $SR0` at `t = 0.000`, clatter where the
//! reference is silent ([`crate::creature_anim::is_footstep_sound`]).
//!
//! ## A footfall is two sounds
//!
//! `$FSD` fires the terrain step **and** an armor foley, and the reference orders them: the foley
//! is `0x623390`'s first act after the state gates (`0x6233d9 call [vt+0x8c]`), *ahead of* the
//! class gate and every terrain lookup below. So a creature whose footstep class is 0 still
//! rustles, and the two land on different buses — the foley uncapped on bus 0, the step on bus
//! 9's cap of 6. Both are voiced here, in that order, rather than in two systems, because they
//! share those three gates and the client shares them too.
//!
//! The foley's material has two sources, one per vtable slot:
//! - **a unit** (`0x623610`) — `CreatureModelData.FoleyMaterialID` off its display
//!   (`[[unit+0xb3c]+0x28]`), i.e. what the *model* is dressed in.
//! - **a player** (`0x62fa30`) — the **chest** item's `Material`, read through the equipment
//!   GUID array at `[player+0x1d38]` element 4. That array is populated only for the local
//!   player (its ctor writes a count of 113 when the object's guid matches
//!   `0x468550`'s and **0 otherwise**, `0x5dd454`), so in the reference *other* players are
//!   silent-bodied. benilla reproduces that for free: `PLAYER_FIELD_INV_SLOT_*` is a private
//!   field the server sends only to you, so the same read is naturally self-only.
//!
//! `Material.dbc` then names the kit, and only three of its eight rows carry one — chain, plate
//! and leather. **Cloth is silent**, which is data, not a gap: a robed mage rustles nothing.
//!
//! The unit's class comes from its voice row (`CreatureSoundData.FootstepID`) through the
//! generic display→sound chain incl. the model fallback (`benilla_formats::creature_sound` —
//! characters reach class 7 as *data*). **Class 0, or no row at all, = no footstep sounds**:
//! the client's `$FSD` handler bails on a zero class before any lookup (`0x6233ec`); the lookup's
//! class-0 rows are the Ancient
//! Protector's stomps (kit 661), reached only by that model's own nonzero class.
//!
//! Ahead of the class, `0x623390` opens on the same three state gates the visual handler does,
//! read on the ROOT unit (the rider of a mounted composite): **hover** (`0x6233aa`, move flag
//! `0x4000_0000`), **stealth/CREEP** (`0x62339c`, `BYTES_1` byte 3 bit `0x2` — not death) and
//! **player ghost** (`0x6233d3`, `PLAYER_FLAGS & 0x10`) — a stealthed unit is silent-footed.
//!
//! Wading (feet below a water surface, down to the unit's own swim boundary `0.75·h`) picks the
//! lookup's **splash** slot instead of dry (falling back to dry when the class has no splash kit);
//! deeper the unit swims and footfalls go silent. The flat 2.0-yd stand-in that boundary used to be
//! is retired now 0464's `collisionHeight` plumb has landed — a murloc's footfalls
//! now go quiet in water that only reaches a human's knees. Still open: this reads a *depth* even
//! for the local player rather than its real mode flag — the named follow-up in.

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

/// The loaded terrain-chain catalog. `pub(crate)`: [`crate::footprints`] reads the same chain's
/// `TerrainType.Flags` gate (`leaves_footprints`) — one load, two footfall consumers.
#[derive(Resource)]
pub(crate) struct Footsteps(pub(crate) FootstepCatalog);

/// **The foley's Z offset** — `0x45851d fadd [0x801628]`, a flat `2.0` added to the emitter's Z
/// before the play. The rustle comes from the body, not the boots, and the reference lifts it by
/// a fixed two yards rather than anything model-derived. WoW's Z is Bevy's Y at the same scale
/// (`benilla_assets::coords`), so this adds to `translation.y`.
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
    // GlobalTransform: a mounted unit's steps are the MOUNT model's own tags, fired by the
    // mount CHILD entity — whose local Transform is the seat-relative ~origin. World position
    // is the only correct read for both parented and top-level sources (0441 fold-back).
    units: Query<(&NetEntity, &GlobalTransform, Option<&CollisionHeight>)>,
    // The handler's state gates read the ROOT unit — a mount child's `$FSD` is the rider's
    // footfall, and it is the RIDER's stealth/hover/ghost the client tests (`0x623390`'s `this`
    // is the unit the mount model's event stream is registered against, `0x607a00`). Same
    // walk-to-root the footprint spawner does.
    parents: Query<&ChildOf>,
    root_state: Query<(Option<&ObjectStore>, Option<&MovementState>)>,
    footsteps: Option<Res<Footsteps>>,
    // The foley half: the material table and the creature catalog that answers a unit's
    // material, as ONE param (the 16-SystemParam ceiling this signature sits at), plus the item
    // store that answers a player's (a chest template ask rides the same once-per-entry
    // discipline every other consumer uses) and the object index its guid resolves through.
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
        // The three state gates, on the root unit (module docs): hover · stealth · player ghost.
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
        // **The armor foley** — `0x6233d9 call [vt+0x8c]`, the handler's first act past the gates
        // above and ahead of every gate below (module docs). Emitted on the stepping entity, not
        // the root: a mount's own body is what rustles under the rider.
        if let (Some(materials), Some(it)) = (materials.as_deref(), items.as_mut()) {
            let material = match net.kind {
                // The player override (`0x62fa30`) reads the chest through the *private*
                // inv-slot array, so this resolves for you and no one else — the reference's
                // own reach, not a restriction added here.
                EntityKind::Player => {
                    super::worn_chest_material(store, &objects, it, &net_commands)
                }
                _ => net
                    .display_id
                    .and_then(|d| creatures.as_deref()?.foley_material(d)),
            };
            if let Some(kit) = material.and_then(|m| materials.0.foley_kit(m)) {
                // The unit's own origin, raised — NOT the fired key's point. Byte-confirmed at
                // `0x45851d fadd [0x801628]` (see the terrain leg below for the split).
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
                    PlayExtras::default(), // bus 0, uncapped, volume 1.0 — `0x458870`'s own
                ) {
                    warn!("foley (kit {kit}): {e:#}");
                }
            }
        }
        // The unit's footstep class (module docs): the voice row's class verbatim; zero or no
        // row = silent (the client's class-0 gate `0x6233ec` — no code default exists).
        let class = match net.display_id.and_then(|d| voices.0.for_display(d)) {
            Some(v) if v.footstep_class != 0 => v.footstep_class,
            _ => continue,
        };
        // Wading picks the splash slot; swimming (deeper than the wade ceiling) is silent.
        let wow = bevy_to_wow(transform.translation());
        // The unit's own room claim — before it, every unit walking under an ADT lake
        // picked the splash slot on dry indoor stone.
        let who = benilla_world::world_point::Subject::Unit(ev.entity);
        let depth = world
            .water_surface_at(who, wow)
            .map(|s| s - wow[2])
            .filter(|d| *d > 0.0);
        let wade_max = swim_enter_depth(collision.copied().unwrap_or_default().0);
        if depth.is_some_and(|d| d > wade_max) {
            continue;
        }
        // What is under the foot — the WMO leg when a building owns this column, the ADT ground
        // effect otherwise ([`benilla_world::surface`]). `None` is the client's −1: silent, and never a
        // reason to fall back to the ground beneath a floor.
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
        // Which leg answered, and what it said. The wrong-surface family is invisible in the kit
        // name alone once two legs can produce one (a `*Dirt` step is right indoors and wrong on a
        // dirt road ten yards away), so the next report should not need a DBC dump to triage.
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
            // **The terrain footstep is at the EVENT's point, the foley above is not** — one arm,
            // two sounds, two positions.
            // `0x623390` hands its `pos` argument to `0x458380`, where it is *both* the
            // audibility-radius reference and the 3D play position (`0x458434`); the foley leg
            // `0x6233d9 call [vt+0x8c]` is argument-less and re-derives `GetPosition + 2.0 z`
            // itself. Playing both at the same place would over-move one of them.
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
