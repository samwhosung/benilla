//! The corpse object (`TYPEID_CORPSE`) drawn as the dead body: a released player's body, and the
//! bone pile the server turns it into once it is reclaimed, looted or timed out.
//!
//! The reference forks once, on `CORPSE_FLAG_BONES` (dress `0x5d6260`, model getter `0x5d6700`).
//! A fresh body (`0x5d6297`) is a player body: `CORPSE_FIELD_DISPLAY_ID` down the usual display
//! chain (`0x5d6759`), dressed from the corpse's own appearance bytes and item slots through the
//! living player's compositor (`0x478cb0`), armour only. A bone pile (`0x5d6291`) has no
//! appearance and takes a static two-bone `<Race><Sex>DeathSkeleton` model (`0x5d670c`).
//!
//! No selection decal: `0x5d6fe0` draws one only for the locked target, and `SetSelection`
//! (`0x493540`) refuses a corpse, whose selectability slot `0x469fe0` returns 0.
//!
//! The reference takes the drowned verdict inside the create, from the scene node's cached liquid
//! probe, and an unprobed node (`[node+0x90] & 0x20`) falls to `Dead`; whether it is probed that
//! early is untraced. This asks the world at attach time, so it can show `Drowned` where the
//! reference shows `Dead`, never the reverse.

use std::collections::HashMap;

use benilla_assets::{m2_url, AnimClip, ModelAnimations};
use benilla_protocol::{CorpseLook, EntityKind};
use bevy::prelude::*;

use super::display::{empty_shell, DisplayModel, ModelHandle};
use crate::creature_anim::AnimData;
use crate::net::{NetEntity, ObjectStore};

/// `AnimationData.dbc` 6 `Dead`, the settled corpse pose (`0x5d63fe push 0x6`).
const DEAD: u16 = 6;
/// `AnimationData.dbc` 132 `Drowned`, the submerged corpse pose (`0x5d63f7 push 0x84`).
const DROWNED: u16 = 132;
/// `[0x80abfc]`, in yards: how far under a liquid surface a corpse lies drowned rather than dead;
/// the wading test (`0x60a740`) uses the same constant.
const DROWNED_DEPTH: f32 = 0.666_666_7;

/// The bone-pile models by `(race, sex)`, apart from [`super::Creatures`] because a skeleton has no
/// `CreatureDisplayInfo` id.
#[derive(Resource, Default)]
pub(crate) struct BonesModels(pub(crate) HashMap<(u8, u8), DisplayModel>);

/// A corpse's model source, the `0x5d6700` fork, read by both the display build and the attach
/// pass so they agree on which cache holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::entities) enum CorpseModel {
    /// `CORPSE_FLAG_BONES`: `<Race><Sex>DeathSkeleton`, from [`BonesModels`].
    Bones(u8, u8),
    /// A fresh body: a `CreatureDisplayInfo` id, from [`super::Creatures`].
    Flesh(u32),
}

/// `None` for anything but a corpse, before its descriptor lands, or for a bone pile whose race the
/// client data cannot name.
pub(in crate::entities) fn corpse_model(
    net: &NetEntity,
    store: Option<&ObjectStore>,
) -> Option<CorpseModel> {
    if net.kind != EntityKind::Corpse {
        return None;
    }
    let s = &store?.0;
    if s.corpse_is_bones() {
        let look = s.corpse_look()?;
        return Some(CorpseModel::Bones(look.race, look.sex.min(1)));
    }
    // The dead player's own body display; a corpse whose create carried none draws nothing, not
    // the debug cube, like the reference's null-row leg.
    net.display_id.map(CorpseModel::Flesh)
}

/// A fresh corpse's appearance snapshot; a bone pile has none, as the reference builds it no
/// character component.
pub(in crate::entities) fn corpse_char_look(store: Option<&ObjectStore>) -> Option<CorpseLook> {
    let s = &store?.0;
    (!s.corpse_is_bones()).then(|| s.corpse_look())?
}

/// `0x5d673c`'s format string `0x85fb30` with `ChrRaces[race]` column 15 and the sex table
/// `0x856450`, clamped to its two shipped entries: the third, `NOSEX`, has no skeleton file.
fn bones_model_path(race_file: &str, sex: u8) -> String {
    let sex = if sex == 0 { "Male" } else { "Female" };
    format!("World\\Generic\\PassiveDoodads\\DeathSkeletons\\{race_file}{sex}DeathSkeleton.mdx")
}

/// Request a `(race, sex)` bone-pile model on first ask; a race with no `ChrRaces` fileString
/// caches an empty display, so the miss is asked once.
pub(in crate::entities) fn ensure_bones_display(
    bones: &mut BonesModels,
    races: &benilla_formats::CharCreateCatalog,
    key: (u8, u8),
    asset_server: &AssetServer,
) {
    if bones.0.contains_key(&key) {
        return;
    }
    let dm = match races.race_file(key.0) {
        Some(file) => DisplayModel {
            handle: ModelHandle::M2(asset_server.load(m2_url(&bones_model_path(file, key.1)))),
            ..empty_shell()
        },
        None => super::display::empty_display(),
    };
    bones.0.insert(key, dm);
}

/// The corpse's pose is armed, once: `0x5d6260` caches the verdict at `[corpse+0x2ac]` and nothing
/// recomputes it.
#[derive(Component)]
pub(super) struct CorpsePosed;

/// Arm each newly attached corpse's pose on bone 0 through `0x7121a0` (`0x5d63de`, `0x5d6402`):
/// `Dead`, or `Drowned` past [`DROWNED_DEPTH`] under a liquid (`0x5d6540`), held at the clip's end.
/// The corpse is created already lying down, and the reference re-arms only from the cached
/// verdict when the model loads (`0x5d6850`). No `AnimDriver`: that would enrol a corpse in the
/// unit gait selector.
#[allow(clippy::type_complexity)] // one query's tuple + its Without filter
pub(super) fn pose_corpses(
    mut commands: Commands,
    mut corpses: Query<
        (
            Entity,
            &NetEntity,
            &GlobalTransform,
            &ModelAnimations,
            &mut AnimationPlayer,
            &mut bevy::animation::transition::AnimationTransitions,
        ),
        Without<CorpsePosed>,
    >,
    anim_data: Option<Res<AnimData>>,
    world: benilla_world::world_point::WorldPoint,
) {
    for (entity, net, tf, anims, mut player, mut transitions) in &mut corpses {
        if net.kind != EntityKind::Corpse {
            continue;
        }
        let wow = benilla_assets::coords::bevy_to_wow(tf.translation());
        // `0x5d6540` takes any liquid (the generic probe `0x670630`, so lava and slime too), and
        // strictly: equality and NaN read as dry. It subtracts `CORPSE_FIELD_POS_Z` (`0x5d7690`),
        // the same number as the entity pose: vmangos writes both from one position
        // (`Corpse.cpp:86-103`) and never moves a corpse.
        let submerged = world
            .liquid_at(benilla_world::world_point::Subject::Unit(entity), wow)
            .is_some_and(|hit| hit.surface_z - wow[2] > DROWNED_DEPTH);
        let want = if submerged { DROWNED } else { DEAD };
        // The model's fallback table (`0x711c10`, `0x712470`): a character body authors neither,
        // and walks `Dead` to `Death` (1) and `Drowned` to `Drown` (131).
        let catalog = anim_data.as_deref().map(|a| &a.0);
        let resolved = catalog.map_or(want, |cat| anims.resolve(want, cat).id);
        let Some(clip) = anims.find(resolved) else {
            // No clip (a static bone pile): posed anyway, so it is not retried every frame.
            commands.entity(entity).insert(CorpsePosed);
            continue;
        };
        arm_settled(&mut player, &mut transitions, clip);
        debug!(
            "corpse pose: {entity} arms {} ({want} -> {resolved}) held at {:.3}s",
            if submerged { "Drowned" } else { "Dead" },
            clip.duration
        );
        commands.entity(entity).insert(CorpsePosed);
    }
}

/// Play `clip` held at its end pose: the settled corpse, never the collapse.
fn arm_settled(
    player: &mut AnimationPlayer,
    transitions: &mut bevy::animation::transition::AnimationTransitions,
    clip: &AnimClip,
) {
    let active = transitions.play(player, clip.node, std::time::Duration::ZERO);
    if clip.looping {
        active.repeat();
    } else {
        active.seek_to(clip.duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 16 shipped skeleton files, spelled exactly as `0x5d673c`'s format string builds them.
    #[test]
    fn bones_paths_match_the_shipped_files() {
        assert_eq!(
            bones_model_path("Human", 0),
            "World\\Generic\\PassiveDoodads\\DeathSkeletons\\HumanMaleDeathSkeleton.mdx"
        );
        assert_eq!(
            bones_model_path("Scourge", 1),
            "World\\Generic\\PassiveDoodads\\DeathSkeletons\\ScourgeFemaleDeathSkeleton.mdx"
        );
        // The client formats `.mdx`; the loader swaps it for the shipped `.m2`.
        assert_eq!(
            m2_url(&bones_model_path("NightElf", 0)),
            "mpq://world/generic/passivedoodads/deathskeletons/nightelfmaledeathskeleton.m2"
        );
    }

    /// A bone pile keeps the body's display id (vmangos copies it over, `Map.cpp:3633`), yet
    /// resolves to a skeleton.
    #[test]
    fn bones_flag_beats_the_display_id() {
        use benilla_protocol::ObjectFields;
        let net = NetEntity {
            kind: EntityKind::Corpse,
            display_id: Some(49),
            scale: 1.0,
        };
        // race 1 (Human), sex 0, in CORPSE_FIELD_BYTES_1 bytes 1/2.
        let bytes_1 = 1u32 << 8;
        let flesh = ObjectStore(
            ObjectFields::from_pairs(&[(32, bytes_1), (33, 0)])
                .into_created(benilla_protocol::messages::ObjectType::Corpse),
        );
        assert_eq!(
            corpse_model(&net, Some(&flesh)),
            Some(CorpseModel::Flesh(49))
        );
        let bones = ObjectStore(
            ObjectFields::from_pairs(&[(32, bytes_1), (33, 0), (35, 0x01)])
                .into_created(benilla_protocol::messages::ObjectType::Corpse),
        );
        assert_eq!(
            corpse_model(&net, Some(&bones)),
            Some(CorpseModel::Bones(1, 0))
        );
        assert!(corpse_char_look(Some(&bones)).is_none());
        assert!(corpse_char_look(Some(&flesh)).is_some());
    }
}
