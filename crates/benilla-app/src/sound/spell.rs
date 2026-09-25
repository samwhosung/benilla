//! Spell-visual kit sounds: [`SpellKitSound`] (the kit's `SoundEntries.dbc` id, `SpellVisualKit`
//! field 13) played at the casting unit, split as the reference's looping test (`0x458830`)
//! splits it. A plain kit is a positioned one-shot (`0x458870`); a looping kit (Fireball's
//! buildup 702, Arcane Missiles' hum 3136) is a channel tracked to the caster (`0x61fec0`),
//! reaped by [`SpellKitSound::StopHold`] as the reference kills the effect's sound at `0x614150`.

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
    // Each unit's live hold-loop kit, so StopHold reaps only that kit's channels.
    mut hold_loops: Local<EntityHashMap<u32>>,
    mut despawned: RemovedComponents<NetEntity>,
) {
    for entity in despawned.read() {
        hold_loops.remove(&entity); // the channel dies with the source-channel despawn reaper
    }
    if events.is_empty() {
        return;
    }
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    let listener = listener.pos;
    // One ring per unit and kit per frame: the impact flash and the aura watcher's add edge can
    // both play a state kit in one packet burst. Plays in different frames both ring.
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
                // The line a headless probe greps to prove a spell's sound fired.
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
                // A bare positional one-shot with no owner or dedup: its one caller is a
                // missile's ground arrival.
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
                // An aura-drop reap: stop this kit's channels; an unrelated hold loop survives.
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
