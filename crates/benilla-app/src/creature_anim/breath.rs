//! Cold breath: the vapour a unit puffs from its mouth in a cold zone.
//!
//! The classifier `0x607710` runs on every unit with a loaded model and caches its verdict for
//! 10 s ([`classify_breath`]). A cold area is `AreaTable.Flags & 0x1`, inherited one hop from the
//! parent zone unless the leaf sets `0x2` (`0x67e9c0`); weather and indoors play no part. On `$BTH`
//! the handler `0x5ffbd0` runs `drunk ≥ 50 → underwater → cold → nothing`, exclusively
//! ([`fire_breath`]), and the cold puff (`SpellVisualEffectName` 107, a 1.5 s one-shot) hangs at
//! the mouth. `$BTH` is authored across the idle family and nothing latches it, so the puff
//! recurs with every idle loop.
//!
//! Not built: the underwater rung, whose bubbles the reference shows once
//! `5.0 + SCALE_X · boxHeight < liquidSurfaceZ − unitZ`, with `boxHeight` the M2 header's render
//! box (MD20 `0xb4`, not the collision box) and `unitZ` the feet: about 8.8 yd deep for a
//! HumanMale, so a diving unit in a cold zone puffs vapour here. Nor are the drunk bubbles (the
//! drunk rung still gates) or a WMO interior area for any unit but the player.
//!
//! Deviation: a new puff inside the last one's clip is declined instead of replacing it
//! (`0x6208e0`), because one-shot effects carry no reap key; either way two never overlap.

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::entities::Creatures;
use crate::net::{NetEntity, ObjectStore, SelfPlayer};

use super::events::AnimSoundEvent;
use super::spell_visual::{FxClass, FxSlot, FxStage, SpellKitFx, SpellVisuals};

/// The breath event (`0x5fffad` compares against it).
const BTH: [u8; 4] = *b"$BTH";

/// Hardcoded effect 3's name (`0x8617b8`, resolved at boot by `0x61f5b0`): `SpellVisualEffectName`
/// row 107, `Particles\ColdBreath.m2`.
const COLD_BREATH_EFFECT: &str = "HARDCODED Breath Cold";

/// The `$BTH` attach tag (entries 2, 3 and 7 of `0x80c968`), a raw M2 `AttachmentID`: resolve it
/// through the model's `attachment_lookup`, never use it as an index (17 on HumanMale, 2 on Wolf).
const BREATH_ATTACH: u16 = 0x11;

/// `[unit+0xc18] = now + 0x2710`: a unit is reclassified at most every 10 s.
const RECLASSIFY_SECS: f32 = 10.0;

/// `PLAYER_BYTES_3` byte 1 clamped to 100, `× 0.01 ≥ 0.5` (`0x5ffbd0`); players only (`0x600018`).
const DRUNK_THRESHOLD: u8 = 50;

/// `ColdBreath.m2`'s one non-looping sequence, 3333 to 4833 ms: no second puff inside it.
const COLD_BREATH_CLIP: f32 = 1.5;

/// The client's `[unit+0xc58]` breath bits, at most one set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Breath {
    /// Bit `0x40`: a cold area.
    Cold,
    /// Neither bit; `0x20`, submerged, is not built.
    None,
}

/// A unit's cached classification and its `[unit+0xc18]` expiry.
#[derive(Component)]
pub(crate) struct BreathEnv {
    kind: Breath,
    stale_at: f32,
}

/// When each unit's last puff started: the overlap guard.
#[derive(Resource, Default)]
pub(super) struct LastPuff(EntityHashMap<f32>);

/// The classifier `0x607710` for each unit whose stamp expired; a new unit classifies at once.
pub(super) fn classify_breath(
    mut commands: Commands,
    time: Res<Time>,
    units: Query<(Entity, &GlobalTransform, Option<&BreathEnv>), With<NetEntity>>,
    self_units: Query<(), With<SelfPlayer>>,
    areas: Option<Res<AreaTableRes>>,
    world: benilla_world::world_point::WorldPoint,
) {
    let Some(areas) = areas else {
        return; // no client data
    };
    let now = time.elapsed_secs();
    for (entity, transform, env) in &units {
        if env.is_some_and(|e| now < e.stale_at) {
            continue;
        }
        // The player's area includes the WMO interior claim; other units get the terrain's.
        let area = if self_units.contains(entity) {
            world.area()
        } else {
            world.area_id_under(transform.translation())
        };
        let kind = match area {
            Some(id) if areas.0.is_cold(id) => Breath::Cold,
            _ => Breath::None,
        };
        // try_insert: a streamed unit can despawn before the commands apply.
        commands.entity(entity).try_insert(BreathEnv {
            kind,
            stale_at: now + RECLASSIFY_SECS,
        });
    }
}

/// The `$BTH` handler `0x5ffbd0`. The puff goes on the model that fired the event (a rider's body,
/// not the mount), while the ladder reads the composite root's state.
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
        return; // no such row, no breath
    };
    let path = path.to_string();
    let now = time.elapsed_secs();
    for ev in events.read() {
        if ev.ident != BTH {
            continue;
        }
        // `CreatureModelData.Flags & 0x2` never breathes (skeletons, elementals, totems).
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
        // The ladder: drunk (a player field, so never a creature), underwater (not built), cold.
        if store.is_some_and(|s| {
            s.0.player_drunk_byte()
                .is_some_and(|b| b >= DRUNK_THRESHOLD)
        }) {
            continue;
        }
        if env.map(|e| e.kind) != Some(Breath::Cold) {
            continue;
        }
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
            // `0x5fbf50`: destroyed at its first completion.
            stage: FxStage::OneShot,
            effects: vec![FxSlot {
                tag: BREATH_ATTACH,
                effect,
                path: path.clone(),
            }],
        });
    }
    // Forget despawned units.
    last.0.retain(|e, _| models.contains(*e));
}

/// The overlap guard's resource; the systems run in `creature_anim`'s own chain.
pub(super) fn register(app: &mut App) {
    app.init_resource::<LastPuff>();
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// `0x11`, not the loot, ding and mount-poof `0x13`: the mouth, not the unit's base.
    #[test]
    fn breath_attaches_at_the_mouth_tag() {
        assert_eq!(BREATH_ATTACH, 0x11);
        assert_eq!(BTH, *b"$BTH");
        assert_eq!(DRUNK_THRESHOLD, 50);
    }
}
