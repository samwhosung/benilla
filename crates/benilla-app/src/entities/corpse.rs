//! **The corpse OBJECT rendered as the dead body** (decision 1706) — the deferral decision 0308 §7
//! opened ("the corpse *object* rendered as the dead body — CGCorpse law") and this closes.
//!
//! A `TYPEID_CORPSE` (7) object is what you run back to: the body a released player leaves behind,
//! and the **bone pile** the server converts it into once it is reclaimed, looted, or times out.
//! Until 1706 it streamed in, spawned an entity, latched its guid for the reclaim send — and drew
//! nothing at all, because [`EntityKind`] had no variant for it and every model lane matched on
//! `Unit | Player | GameObject`.
//!
//! ## The reference's law (`CGCorpse_C` dress `0x5d6260`, model getter `0x5d6700`)
//!
//! It forks once, on `CORPSE_FIELD_FLAGS` bit 0 (`CORPSE_FLAG_BONES`), and the two halves share
//! nothing:
//!
//! - **A fresh body** (`0x5d6297`) allocates a `CCharacterComponent` at `[corpse+0x29c]` and fills
//!   it from the corpse's OWN snapshot — the seven `CORPSE_FIELD_BYTES_1/_2` bytes (`+0x69..+0x6f`)
//!   and the 19 `CORPSE_FIELD_ITEM` slots (`+0x1c + slot*4`, low 24 bits = the ItemDisplayInfo id)
//!   — through the very same compositor entry `0x478cb0` a living player is dressed by. Its model
//!   is `CORPSE_FIELD_DISPLAY_ID` down the ordinary CreatureDisplayInfo → CreatureModelData chain
//!   (`0x5d6759`), which is why the whole character pipeline (decisions 0041/0044/0045/0074) simply
//!   applies: a corpse *is* a player body wearing a wire-supplied look.
//! - **A bone pile** (`0x5d6291 jne`) builds **no component at all** — no appearance, no gear — and
//!   takes its model from race/sex instead: `0x5d670c` formats
//!   `World\Generic\PassiveDoodads\DeathSkeletons\%s%sDeathSkeleton.mdx` from `ChrRaces[race]+0x3c`
//!   (the client fileString: `"Human"`, `"Scourge"`, …) and `["Male","Female","NOSEX"][sex]`. Those
//!   16 shipped models are fully static, two-boned, one hardcoded texture each.
//!
//! Three slots are carved out of the dress loop, and none of them is a "skip if empty":
//! `0x5d6465`/`0x5d6470` skip slot 0 (head) and slot 0xe (back) when this corpse's own
//! `CORPSE_FLAG_HIDE_HELM 0x08` / `HIDE_CLOAK 0x10` are set — its own bits on its own field, and the
//! opposite instruction polarity to the player lane (`0x5ed700`) — and
//! `0x5d644e` skips slot 0x11 (ranged) unconditionally.
//!
//! ## What a corpse deliberately does NOT wear
//!
//! **No weapons.** Slots 0xf/0x10 detour at `0x5d645a`/`0x5d645f` to `0x5d649b`, which pushes the
//! raw `CORPSE_FIELD_ITEM` word as a **guid** into the object-manager lookup `0x468460` with
//! typemask 2 (`TYPEMASK_ITEM`) and skips the slot on the null return. That word is
//! `DisplayInfoID | (InventoryType << 24)` — never a guid — so the lookup cannot succeed and the
//! branch is dead. We reproduce the *outcome* (a corpse wears armour, not weapons), not the
//! mechanism: aping a dead lookup would be aping a quirk, which §3 of the contract is against.
//!
//! ## The ground decal, and the one divergence
//!
//! `[corpse+0x2b0]` is **not** a render scale (its old ledger gloss); it is `0.5 ×` the MD20 render
//! bounding-sphere radius, read by exactly one site — `0x5d6fe0`, which sizes the corpse's
//! `UnitSelectTexture` ground decal by it × `OBJECT_FIELD_SCALE_X`.
//!
//! **That decal is dead code, and we owe it nothing** (1727, closed by 1732). Its one dispatcher
//! image-wide gates on the object being the **Locked Target** (`[0xb4e2d8]`/`[0xb4e2dc]`), and
//! `SetSelection 0x493540` refuses any object whose `[vt+0x58]` selectability slot returns 0 —
//! CGCorpse's is `0x469fe0`, literally `33 c0 c3`. No corpse, bones or fresh, can ever be the
//! locked target, so `0x5d6fe0` never runs. (The same refusal is independently why a click on a
//! body leaves your current target alone: `0x4935f3` bails *before* the clear-the-old-selection
//! call.) Mouseover is a different slot — `+0x54` = `0x5d76d0`, which a corpse *does* pass — and
//! that is the highlight/cursor/tooltip route we ship, not a ring.
//!
//! **The divergence to know about** (still open, not settled at the bytes): the
//! reference computes the drowned verdict *inside* the create, from the scene node's cached liquid
//! probe — and its own `[node+0x90] & 0x20` gate means an un-probed node falls through to `Dead`.
//! Whether the node is already probed that early is a byte question nobody has closed. Ours asks
//! the world directly at attach time, a frame or more later, so our `Drowned` leg is reachable
//! whether or not theirs is. The failure is one-sided — we can only ever show `Drowned` where the
//! reference might show `Dead`, never the reverse — and it needs a drowned corpse to observe.
//!
//! ## The pose
//!
//! `0x5d63de`/`0x5d6402` arm bone 0 through the shared M2 arm `0x7121a0` with **AnimationData id 6
//! (`Dead`)**, or **132 (`Drowned`)** when `0x5d6540` says so. That predicate is a **liquid**
//! query: `0x670630` reads the corpse scene node's cached probe, whose `[node+0x98]` is named at
//! its *writer* (`0x69e280` → the terrain liquid query `0x69b6d0`) as the **liquid surface
//! height**. The comparison is
//!
//! ```text
//! liquidSurfaceZ − CORPSE_FIELD_POS_Z > [0x80abfc]        // 0x3f2aaaab = 0.66666669
//! ```
//!
//! strict (`test ah,0x41` read by `jne`, so equality and NaN are both false), and 2/3 yd is this
//! client's submersion depth generally — `0x60a740`, whose result is pushed as the `$FSD` **wading**
//! flag, runs the same subtraction against the same constant.
//!
//! Both ids resolve through the model's own fallback table before playing (which is what
//! `0x7121a0` itself does: `0x711c10` playableAnimationLookup, then `0x712470` animationLookup,
//! before any `nSequences` bounds check): `HumanMale.m2` authors neither 6 nor 132's head directly,
//! and `AnimationData.dbc` walks `Dead → Death(1)` and `Drowned → Drown(131)`.
//!
//! The clip is armed **once, seeked to its end**, which is the reference's own shape: `0x5d6260`
//! runs exactly once per object (one call site, no data-section pointer to it anywhere in the PE),
//! it caches the verdict in `[corpse+0x2ac]`, and nothing ever recomputes it. `0x5d6850` re-arms
//! from that cached byte, but it is the **model-ready callback** (vtable `+0x34`, fired by the
//! trampoline `0x613d70` that SetModel registers), not a per-frame tick. A corpse object never
//! collapses in front of you either — it is created after the release, already lying down — so this
//! is a settled pose, never a replay.

use std::collections::HashMap;

use benilla_assets::{m2_url, AnimClip, ModelAnimations};
use benilla_protocol::{CorpseLook, EntityKind};
use bevy::prelude::*;

use super::display::{empty_shell, DisplayModel, ModelHandle};
use crate::creature_anim::AnimData;
use crate::net::{NetEntity, ObjectStore};

/// `AnimationData.dbc` **6 `Dead`** — the settled corpse pose (`0x5d63fe push 0x6`).
const DEAD: u16 = 6;
/// `AnimationData.dbc` **132 `Drowned`** — the submerged corpse pose (`0x5d63f7 push 0x84`).
const DROWNED: u16 = 132;
/// `[0x80abfc]`, read out of the shipped PE's `.rdata`: how far a corpse must be under a liquid
/// surface before it lies drowned rather than dead. Yards.
const DROWNED_DEPTH: f32 = 0.666_666_7;

/// The **bone-pile** body models, keyed `(race, sex)` — 16 at most, one per playable ChrRaces row ×
/// sex, and every one a static two-bone prop. Kept out of [`super::Creatures`] on purpose: that map
/// is keyed by `CreatureDisplayInfo` id, a real DBC keyspace, and a skeleton has no id in it.
#[derive(Resource, Default)]
pub(crate) struct BonesModels(pub(crate) HashMap<(u8, u8), DisplayModel>);

/// Where one corpse's body model comes from — the `0x5d6700` fork, resolved once and read by both
/// the display-build pass and the attach pass so the two can never disagree about which cache holds
/// this corpse's model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::entities) enum CorpseModel {
    /// `CORPSE_FLAG_BONES` — `<Race><Sex>DeathSkeleton`, from [`BonesModels`].
    Bones(u8, u8),
    /// A fresh body — a `CreatureDisplayInfo` id, from [`super::Creatures`] like any other body.
    Flesh(u32),
}

/// Classify a corpse entity's model source. `None` when it is not a corpse, or while its descriptor
/// has not landed (the one-frame window between the create's entity spawn and the pending-fields
/// flush), or for a bone pile whose race/sex the client data cannot name.
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
    // The wire's display id, which for a corpse is the dead player's own body display. A corpse
    // whose create carried none has no body to build — the debug cube would be the wrong answer
    // here (nothing named a model), so it draws nothing, like the reference's null-row leg.
    net.display_id.map(CorpseModel::Flesh)
}

/// The corpse's appearance snapshot — [`ObjectFields::corpse_look`](benilla_protocol::ObjectFields::corpse_look)
/// for a corpse that is not a bone pile. `None` for anything else, which is exactly the reference's
/// gate: a bone pile builds no character component, so it has no look to read.
pub(in crate::entities) fn corpse_char_look(store: Option<&ObjectStore>) -> Option<CorpseLook> {
    let s = &store?.0;
    (!s.corpse_is_bones()).then(|| s.corpse_look())?
}

/// `World\Generic\PassiveDoodads\DeathSkeletons\<Race><Sex>DeathSkeleton.m2` — `0x5d673c`'s format
/// string `0x85fb30` with `ChrRaces[race]` column 15 and the sex table `0x856450`.
///
/// Sex is clamped to the two shipped halves: the reference's third string is `"NOSEX"`, for which no
/// skeleton file exists, and no player corpse can carry it.
fn bones_model_path(race_file: &str, sex: u8) -> String {
    let sex = if sex == 0 { "Male" } else { "Female" };
    format!("World\\Generic\\PassiveDoodads\\DeathSkeletons\\{race_file}{sex}DeathSkeleton.mdx")
}

/// Ensure a `(race, sex)` bone-pile display exists in the cache, requesting its model on first ask.
/// A race with no `ChrRaces` fileString (nothing playable — no skeleton ships) caches an empty
/// display, so the miss is asked once rather than every frame.
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

/// This corpse's held pose has been armed — once-only, like the reference's: `0x5d6260` computes
/// the verdict once per object and caches it in `[corpse+0x2ac]`, and no site recomputes it.
#[derive(Component)]
pub(super) struct CorpsePosed;

/// Arm each freshly-attached corpse's settled pose: `Dead`, or `Drowned` when it lies more than
/// [`DROWNED_DEPTH`] under a liquid surface, seeked to the clip's end.
///
/// Runs off the attach's own output (an `AnimationPlayer` + the model's [`ModelAnimations`]) rather
/// than off a driver: a corpse has no state machine to run, and giving it an
/// [`AnimDriver`](crate::creature_anim::AnimDriver) would enrol it in the unit gait selector, which
/// has no meaning for an object with no movement, no sheath and no combat.
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
        // The reference's `0x5d6540`: ANY liquid, not water alone — it is the generic `0x670630`
        // probe, the same one the breath classifier runs, so lava and slime count too. Strict `>`,
        // like the emitted `jne` (equality and NaN both read as not-submerged).
        //
        // The reference subtracts `CORPSE_FIELD_POS_Z` (its position getter `0x5d7690` copies
        // `[[corpse+0x110]+0xc/0x10/0x14]`), where this reads the entity's world pose. They are the
        // same number: vmangos writes the descriptor POS fields and the movement block's
        // `HAS_POSITION` pose from one `Relocate` (`Corpse.cpp:100-103`), and nothing moves a
        // corpse afterwards.
        let submerged = world
            .liquid_at(benilla_world::world_point::Subject::Unit(entity), wow)
            .is_some_and(|hit| hit.surface_z - wow[2] > DROWNED_DEPTH);
        let want = if submerged { DROWNED } else { DEAD };
        // The model's own fallback resolution (decision 0082) — a character body authors neither
        // `Dead` nor `Drowned` directly and walks to `Death`/`Drown`.
        let catalog = anim_data.as_deref().map(|a| &a.0);
        let resolved = catalog.map_or(want, |cat| anims.resolve(want, cat).id);
        let Some(clip) = anims.find(resolved) else {
            // No clip to hold: mark it posed anyway so this doesn't re-run every frame for a body
            // whose model authors nothing (a bone pile — static, and correct as it stands).
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

/// Play `clip` and hold it at its end pose — the settled corpse, never the collapse.
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
        // `.mdx` is the authored extension the client formats; the loader swaps it for the shipped
        // `.m2` like every other model path.
        assert_eq!(
            m2_url(&bones_model_path("NightElf", 0)),
            "mpq://world/generic/passivedoodads/deathskeletons/nightelfmaledeathskeleton.m2"
        );
    }

    /// The model fork is the BONES bit, and nothing else: a bone pile carries a perfectly good
    /// display id (the server's conversion copies it verbatim) and must still resolve to a skeleton.
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
        // …and a bone pile has no look to dress from, where a fresh body does.
        assert!(corpse_char_look(Some(&bones)).is_none());
        assert!(corpse_char_look(Some(&flesh)).is_some());
    }
}
