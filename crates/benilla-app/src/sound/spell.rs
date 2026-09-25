//! Spell-visual kit sounds: route [`SpellKitSound`] — the kit's own
//! `SoundEntries.dbc` id (`SpellVisualKit` field 13), resolved by the cast-edge router
//! (`crate::creature_anim::spell_visual`) — to audio at the casting unit, mirroring the client's
//! looping-test split (`0x458830`): a plain kit rings as a fire-and-forget positioned one-shot
//! (`0x458870`); a **LOOPING** kit (Fireball's precast buildup 702, Arcane Missiles' channel hum
//! 3136) becomes a channel **tracked to the caster** (`0x61fec0`) and is reaped by
//! [`SpellKitSound::StopHold`] when the hold ends (the client kills the effect's sound at
//! `0x614150`) — without the reap, a `/castvis 133` buildup loops forever past its own release.
//! Unit despawn is covered by the greeting module's source-channel reaper.

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use crate::creature_anim::SpellKitSound;
use crate::net::NetEntity;
use benilla_assets::WorldAssets;
use benilla_world::schedule::WorldStage;

use super::kit::{
    kit_looping, play_kit, play_kit_ext, KitRef, PlayExtras, SoundCategory, SoundKits,
};
use super::{AudioListener, SoundConfig, SoundOutput};

fn route_spell_kit_sounds(
    mut events: MessageReader<SpellKitSound>,
    transforms: Query<&Transform, Without<Camera3d>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
    // Each unit's live tracked hold-loop kit, so StopHold reaps exactly that kit's channels and
    // never a sibling tagged channel (the caster's greeting line keeps talking).
    mut hold_loops: Local<EntityHashMap<u32>>,
    mut despawned: RemovedComponents<NetEntity>,
) {
    for entity in despawned.read() {
        hold_loops.remove(&entity); // the channel itself dies via the greeting despawn reaper
    }
    if events.is_empty() {
        return;
    }
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    let listener = listener.pos;
    // Same-drain dedup: the client rings a kit's sound once per PlaySpellVisualKit call, and a
    // state kit is legitimately played twice in one packet burst (the impact hand-off's flash +
    // the aura watcher's ADD edge — both real reference callers). One frame, one ring;
    // plays in different frames (a missile impact preceding the aura by a beat) both ring, as
    // the reference's two calls would.
    let mut played_now: Vec<(Entity, u32)> = Vec::new();
    for ev in events.read() {
        match *ev {
            SpellKitSound::Play { entity, kit_sound } => {
                if played_now.contains(&(entity, kit_sound)) {
                    continue;
                }
                played_now.push((entity, kit_sound));
                let pos = transforms.get(entity).map(|t| t.translation).ok();
                let looping = kit_looping(&kits, kit_sound);
                // The kit player is otherwise invisible in logs — this line is what a headless
                // probe greps to prove a spell's sound actually fired (0451 mount-up probe).
                debug!("spell kit sound {kit_sound} on {entity:?} (looping {looping})");
                let played = if looping {
                    play_kit_ext(
                        &mut kits,
                        &assets,
                        &mut out,
                        &config,
                        listener,
                        KitRef::Id(kit_sound),
                        pos,
                        SoundCategory::Sfx,
                        PlayExtras {
                            source: Some(entity),
                            ..default()
                        },
                    )
                } else {
                    play_kit(
                        &mut kits,
                        &assets,
                        &mut out,
                        &config,
                        listener,
                        KitRef::Id(kit_sound),
                        pos,
                        SoundCategory::Sfx,
                    )
                };
                match played {
                    Ok(_) if looping => {
                        hold_loops.insert(entity, kit_sound);
                    }
                    Ok(_) => {}
                    Err(e) => warn!("spell kit sound {kit_sound}: {e:#}"),
                }
            }
            SpellKitSound::PlayAt { pos, kit_sound } => {
                // The kit-sound leg's `extra`-override arm: a bare positional one-shot, no owner
                // to track a loop on and no dedup ledger — its one caller is a missile's ground
                // arrival, and two projectiles never land on the same point in the same frame.
                debug!("spell kit sound {kit_sound} at {pos:?}");
                if let Err(e) = play_kit(
                    &mut kits,
                    &assets,
                    &mut out,
                    &config,
                    listener,
                    KitRef::Id(kit_sound),
                    Some(pos),
                    SoundCategory::Sfx,
                ) {
                    warn!("spell kit sound {kit_sound}: {e:#}");
                }
            }
            SpellKitSound::StopHold { entity } => {
                if let Some(kit_sound) = hold_loops.remove(&entity) {
                    super::kit::stop_source_kit(&mut out, entity, kit_sound);
                }
            }
            SpellKitSound::StopKit { entity, kit_sound } => {
                // Kit-scoped (an aura-drop reap): stop exactly this kit's channels; the
                // ledger entry goes only if it is this kit, so an unrelated hold loop survives.
                if hold_loops.get(&entity) == Some(&kit_sound) {
                    hold_loops.remove(&entity);
                }
                super::kit::stop_source_kit(&mut out, entity, kit_sound);
            }
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, route_spell_kit_sounds.in_set(WorldStage::Present));
}
