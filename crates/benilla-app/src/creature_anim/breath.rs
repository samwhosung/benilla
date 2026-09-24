//! **Cold breath** — the visible vapour a unit puffs from its mouth in a cold zone (B233,
//! decision 1149).
//!
//! The client hangs three mutually-exclusive puffs off one animation event, and they are the
//! `$BTH` family: cold vapour, underwater bubbles, and the drunk's bubbles. This module owns the
//! **cold** one — the only one of the three that was reported missing, and the only one whose
//! every input benilla already has.
//!
//! ## The mechanism, in the order the client runs it
//!
//! 1. **The classifier** `0x607710` — `CGUnit_C` vtable slot 14, run per frame from the
//!    WorldFrame's visible-object callback, on **every unit with a loaded model** (NPCs and
//!    creatures too; there is no local-player gate anywhere in it). It **clears both**
//!    `[unit+0xc58]` bits at entry and then sets **at most one**: `0x20` submerged, `0x40` cold.
//!    It stamps `[unit+0xc18] = now + 10 s` on every leg, so **the classification is cached for
//!    ten seconds per unit** — crossing a zone border does not take effect instantly.
//!    [`classify_breath`].
//! 2. **The cold test** `0x67e9c0` — `AreaTable.Flags` bit `0x1`, inherited ONE hop from the
//!    parent zone unless the leaf sets bit `0x2`. That single hop is the whole mechanism: only
//!    four rows in 5875 author the bit (Dun Morogh, Winterspring, Razorfen Downs, Naxxramas) and
//!    every sub-area a player actually stands in carries `0x40`, so a leaf-only read finds
//!    nothing anywhere. [`benilla_formats::AreaTableCatalog::is_cold`].
//!    It is **independent of the weather** — snowfall and breath share no input — and there is no
//!    indoor suppression: the Thunderbrew Distillery is as cold as the road outside.
//! 3. **The handler** `0x5ffbd0` — on the `$BTH` anim event, the ladder
//!    `drunk ≥ 50 → underwater → cold → nothing`, strictly exclusive. [`fire_breath`].
//! 4. **The puff** — `SpellVisualEffectName` row 107 `"HARDCODED Breath Cold"` →
//!    `Particles\ColdBreath.m2`, attached at **M2 AttachmentID `0x11`** (a bone under the head —
//!    key bone 21, whose pivot is the mouth), inheriting the bone's full orientation. The asset
//!    is emitter-only (no mesh): one non-looping 1.5 s sequence, one continuous particle emitter,
//!    zero gravity, a ~1.6° cone — a tight puff, not a spray. It self-terminates at the end of
//!    its own clip (the `0x5fbf50` one-shot terminator, the same lifetime the mount poof uses).
//!
//! **`$BTH` is authored on the whole idle family**, so the cadence is the animation's, not a
//! timer's: `Stand` and its variants, every `Ready*`, `Stun`, `Sleep`, `KneelLoop`,
//! `StealthStand`, `SwimIdle`, several emotes and `Drown` — on every player race/gender (HumanMale
//! `Stand` keys one at 667 ms), and on 148 of the 430 creature models. There is no fire-once
//! latch, so it re-fires every loop of the carrying clip.
//!
//! ## Stated divergences
//!
//! - **The underwater rung is not modelled** — but it is now fully specified rather than open:
//!   what is missing is the work, not the law (`0x607710`'s liquid arm). The test is
//!
//!   ```text
//!   underwater ⟺ 5.0 + SCALE_X · boxHeight < liquidSurfaceZ − unitZ
//!   ```
//!
//!   where `boxHeight` is the M2 header's **box A** — `bounding_box_max.z − bounding_box_min.z`,
//!   the loose all-animation render box at MD20 `0xb4`, raw model space
//!   (`benilla_m2::M2Header::bounding_box_min`/`_max`). Emphatically **not** the collision box and
//!   not `CreatureModelData.CollisionHeight`: those are the *other* box (`0xd0`) and agree with
//!   each other, not with this one. `SCALE_X` is the only multiplier — no DBC scale enters.
//!   `unitZ` is the unit's **feet** (its movement-block position), and `liquidSurfaceZ` is the
//!   ordinary liquid query's own cached surface, not a second sample.
//!
//!   For a scale-1 HumanMale that threshold is **8.84 yd** of water above the feet (box A `Δz` =
//!   3.8415); DwarfMale 8.03, GnomeFemale 7.11, TaurenMale 10.21; a unit whose model has not
//!   loaded uses the ctor default `1.0`, i.e. 6.0. **So it is a diving test, not a swimming one**,
//!   and that is what bounds the omission: a unit in a cold zone puffs vapour where the reference
//!   would puff bubbles only once genuinely deep. The bubbles themselves
//!   (`Particles\Bubbles.m2` — 3.3 s and *looping*, so they need the replace-on-respawn dedup
//!   this module's one-shot does not) are a separate, unreported feature. Two details for whoever
//!   builds it: while **mounted** the box is the MOUNT's model, and `SCALE_X` runs through a 2 s
//!   cosine ease after a scale change (bit-exact at rest, so only that window differs).
//! - **The drunk rung gates but does not spawn.** `drunk ≥ 50` correctly suppresses cold breath
//!   (the ladder is exclusive); the inebriated bubbles themselves are unreported and their clip
//!   semantics are unpinned, so nothing is drawn there.
//! - **A remote unit's area is the terrain's alone.** The player's area comes from
//!   [`CurrentArea`], which races a WMO interior claim ahead of the terrain chunk exactly as the
//!   client's `GetAreaID` does; there is no per-unit equivalent of that claim, so other units
//!   read [`area_id_under`]. It differs only inside a WMO whose `WMOAreaTable` row names a
//!   different area than the ground beneath it.
//! - **Overlap is refused rather than replaced.** The client's `0x6208e0` destroys a still-running
//!   puff when a new one spawns on the same unit at the same tag; our one-shot instances carry no
//!   reap key, so [`fire_breath`] declines a new puff while the last one is still inside its clip
//!   instead. Same observable — never two overlapping puffs — with the running one finishing
//!   rather than restarting.

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::entities::Creatures;
use crate::net::{NetEntity, ObjectStore, SelfPlayer};

use super::events::AnimSoundEvent;
use super::spell_visual::{FxClass, FxSlot, FxStage, SpellKitFx, SpellVisuals};

/// The event that asks "what does this unit breathe?" — `0x5fffad cmp eax,0x48544224`.
const BTH: [u8; 4] = *b"$BTH";

/// Hardcoded-effect index **3**'s baked lookup name (the `0x8617b8` string table, matched by the
/// boot name-resolve `0x61f5b0`) → `SpellVisualEffectName` row 107 → `Particles\ColdBreath.m2`.
const COLD_BREATH_EFFECT: &str = "HARDCODED Breath Cold";

/// The `$BTH` family's attach tag — `DAT_0080c968[2..=3]` and `[7]` are all `0x11`, where the loot
/// art / ding / mount poof are `0x13`. A **raw M2 `AttachmentID`**, resolved through the model's
/// own `attachment_lookup` (the index it lands on differs per model: 17 on HumanMale, 13 on
/// HumanFemale, 8 on GnomeFemale, 2 on Wolf — never use the tag as an array index).
const BREATH_ATTACH: u16 = 0x11;

/// `[unit+0xc18] = now + 0x2710` — the classifier re-runs at most every 10 s per unit.
const RECLASSIFY_SECS: f32 = 10.0;

/// `PLAYER_BYTES_3` byte 1, clamped 100, `×0.01 ≥ 0.5` (`0x5ffbd0`) — i.e. the raw byte at 50.
/// Read on **players only** (`OBJECT_FIELD_TYPE` bit 4, `0x600018`).
const DRUNK_THRESHOLD: u8 = 50;

/// `Particles\ColdBreath.m2`'s only sequence: 3333 → 4833 ms, `flags = 0x1` (non-looping). The
/// window inside which a second `$BTH` is declined — see the module docs' overlap divergence.
const COLD_BREATH_CLIP: f32 = 1.5;

/// What a unit's environment says it breathes — the client's `[unit+0xc58]` bits `0x20`/`0x40`,
/// which the classifier clears together and sets at most one of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Breath {
    /// Bit `0x40` — a cold area. The puff.
    Cold,
    /// Neither bit: nothing to see. (Bit `0x20`, submerged, is the unmodelled rung — module docs.)
    None,
}

/// A unit's cached breath classification and when it next goes stale — the client's
/// `[unit+0xc58]` bits behind their `[unit+0xc18]` 10-second stamp.
#[derive(Component)]
pub(crate) struct BreathEnv {
    kind: Breath,
    /// `Time::elapsed_secs` at which [`classify_breath`] re-resolves this unit.
    stale_at: f32,
}

/// When the last cold-breath puff started on this unit — the overlap guard standing in for the
/// client's `0x6208e0` replace-on-respawn dedup (module docs).
#[derive(Resource, Default)]
pub(super) struct LastPuff(EntityHashMap<f32>);

/// Resolve `[unit+0xc58]`'s bits for every unit whose 10-second stamp has expired — the client's
/// `0x607710`, which runs on every unit with a loaded model and caches for 10 s.
///
/// A unit seen for the first time has no [`BreathEnv`] and is classified at once, so a unit that
/// streams in inside a cold zone breathes on its next idle loop rather than up to 10 s later.
pub(super) fn classify_breath(
    mut commands: Commands,
    time: Res<Time>,
    units: Query<(Entity, &GlobalTransform, Option<&BreathEnv>), With<NetEntity>>,
    self_units: Query<(), With<SelfPlayer>>,
    areas: Option<Res<AreaTableRes>>,
    world: benilla_world::world_point::WorldPoint,
) {
    let Some(areas) = areas else {
        return; // no client data — the DBC-resource degrade shape, as everywhere else
    };
    let now = time.elapsed_secs();
    for (entity, transform, env) in &units {
        if env.is_some_and(|e| now < e.stale_at) {
            continue;
        }
        // The player's area is `CurrentArea` — the client's own `GetAreaID`, WMO-interior claim
        // included. Every other unit gets the outdoor leg alone (module docs).
        let area = if self_units.contains(entity) {
            world.area()
        } else {
            world.area_id_under(transform.translation())
        };
        let kind = match area {
            Some(id) if areas.0.is_cold(id) => Breath::Cold,
            _ => Breath::None,
        };
        // try_insert: a streamed unit can despawn (teardown, teleport, range-out) between this
        // system's parallel run and its sync point — a plain insert panics on the dead entity.
        commands.entity(entity).try_insert(BreathEnv {
            kind,
            stale_at: now + RECLASSIFY_SECS,
        });
    }
}

/// The `$BTH` handler `0x5ffbd0`: run the exclusive ladder and puff.
///
/// The event fires on the model that authored it, which for a mounted rider is the body hanging
/// off the mount — so the **puff goes on the event's own entity** (its mouth), while the unit
/// state the ladder reads (drunk, the environment) comes from the composite's **root**, exactly
/// as the footfall visuals split them.
pub(super) fn fire_breath(
    mut events: MessageReader<AnimSoundEvent>,
    time: Res<Time>,
    models: Query<&NetEntity>,
    parents: Query<&ChildOf>,
    roots: Query<(Option<&ObjectStore>, Option<&BreathEnv>)>,
    visuals: Option<Res<SpellVisuals>>,
    creatures: Option<Res<Creatures>>,
    mut last: ResMut<LastPuff>,
    mut fx: MessageWriter<SpellKitFx>,
) {
    if events.is_empty() {
        return;
    }
    let (Some(visuals), Some(creatures)) = (visuals, creatures) else {
        return;
    };
    let Some((effect, path)) = visuals.0.hardcoded_effect(COLD_BREATH_EFFECT) else {
        return; // no such row — no breath (the DBC-resource degrade shape)
    };
    let path = path.to_string();
    let now = time.elapsed_secs();
    for ev in events.read() {
        if ev.ident != BTH {
            continue;
        }
        // `CreatureModelData.Flags & 0x2` suppresses the whole `$BTH` family — skeletons, ghosts,
        // elementals, golems, slimes, totems: 99 of the 430 shipped rows. Read off the model that
        // fired the event, which is the model that would wear the puff.
        let breathes = models
            .get(ev.entity)
            .ok()
            .and_then(|n| n.display_id)
            .is_none_or(|d| creatures.breathes(d));
        if !breathes {
            continue;
        }
        let mut root = ev.entity;
        while let Ok(child_of) = parents.get(root) {
            root = child_of.parent();
        }
        let Ok((store, env)) = roots.get(root) else {
            continue;
        };
        // The ladder, strictly exclusive (`0x5ffbd0`): drunk ≻ underwater ≻ cold ≻ nothing. The
        // drunk rung reads PLAYER_BYTES_3 byte 1 and is a PLAYER-only field, so a creature never
        // takes it; the underwater rung is the unmodelled one (module docs).
        if store.is_some_and(|s| {
            s.0.player_drunk_byte()
                .is_some_and(|b| b >= DRUNK_THRESHOLD)
        }) {
            continue;
        }
        if env.map(|e| e.kind) != Some(Breath::Cold) {
            continue;
        }
        // The overlap guard: one puff at a time on a unit (module docs).
        if last
            .0
            .get(&ev.entity)
            .is_some_and(|&t| now - t < COLD_BREATH_CLIP)
        {
            continue;
        }
        last.0.insert(ev.entity, now);
        debug!(
            "anim: $BTH in a cold area — the breath puffs ({})",
            ev.entity
        );
        fx.write(SpellKitFx::Begin {
            entity: ev.entity,
            spell_id: 0,
            persistent: false,
            class: FxClass::Hold,
            // `0x5fbf50` — destroy at the first completion; the shipped clip runs 1.5 s.
            stage: FxStage::OneShot,
            effects: vec![FxSlot {
                tag: BREATH_ATTACH,
                effect,
                path: path.clone(),
            }],
        });
    }
    // Streamed units despawn on range-out — drop their puff memory with them.
    last.0.retain(|e, _| models.contains(*e));
}

/// The resource the overlap guard lives in — the systems themselves are wired into
/// `creature_anim`'s own chain beside every other `$`-tag consumer.
pub(super) fn register(app: &mut App) {
    app.init_resource::<LastPuff>();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 10-second stamp: a freshly-seen unit classifies at once, and a classified one is not
    /// re-resolved until its stamp expires — the client's `[unit+0xc18] = now + 0x2710`.
    #[test]
    fn classification_is_cached_for_ten_seconds() {
        let env = BreathEnv {
            kind: Breath::Cold,
            stale_at: 10.0,
        };
        assert!(9.99 < env.stale_at, "still fresh at 9.99 s");
        assert!(10.01 >= env.stale_at, "stale at 10.01 s");
        assert!(
            (RECLASSIFY_SECS - 10.0).abs() < f32::EPSILON,
            "0x2710 ms = 10 s"
        );
    }

    /// The tag is `0x11`, not the loot/ding/poof `0x13` — the one number that decides whether the
    /// vapour comes out of the mouth or the unit's base.
    #[test]
    fn breath_attaches_at_the_mouth_tag() {
        assert_eq!(BREATH_ATTACH, 0x11);
        assert_eq!(BTH, *b"$BTH");
        assert_eq!(DRUNK_THRESHOLD, 50);
    }
}
