//! Spell-visual kit sounds: [`SpellKitSound`] (the kit's `SoundEntries.dbc` id, `SpellVisualKit`
//! field 13) played at the casting unit, split as the reference's looping test (`0x458830`)
//! splits it. A plain kit is a positioned one-shot (`0x458870`); a looping kit (Fireball's
//! buildup 702, Arcane Missiles' hum 3136) is a channel tracked to the caster (`0x61fec0`),
//! reaped by [`SpellKitSound::StopHold`] as the reference kills the effect's sound at `0x614150`,
//! keyed by the spell whose kit rang it ([`HoldLoops`]).

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use crate::creature_anim::SpellKitSound;
use crate::lanes::Stamp;
use crate::net::{NetEntity, ObjectStore};
use benilla_assets::WorldAssets;
use benilla_world::schedule::WorldStage;

use super::kit::{
    kit_looping, play_kit, play_kit_ext, KitRef, PlayExtras, SoundCategory, SoundKits,
};
use super::{AudioListener, SoundConfig, SoundOutput};

/// Each unit's live hold loop. The reference's sound rides the kit's effect, which carries its
/// spell (`+0x18`), and `0x614150(spell, 0)` takes only a matching effect (`0x614167`), so one
/// spell's stop spares another spell's loop.
#[derive(Default)]
pub(crate) struct HoldLoops(EntityHashMap<Hold>);

#[derive(Clone, Copy)]
struct Hold {
    /// The spell whose kit rang it, and the kit.
    kit: (u32, u32),
    /// Rung by a stage-2 play ([`SpellKitSound::Play`]'s `state`).
    state: bool,
    /// Its packet's place, which a display rebuild compares.
    at: Stamp,
}

impl HoldLoops {
    /// A looping kit (`spell_id`, `kit_sound`) rang on `entity`: the unit's hold loop now. A
    /// play without a packet stamp rang now, after every packet already read.
    pub(crate) fn rang(&mut self, entity: Entity, kit: (u32, u32), state: bool, at: Option<Stamp>) {
        let at = at.unwrap_or_else(crate::lanes::stamp);
        self.0.insert(entity, Hold { kit, state, at });
    }

    /// [`SpellKitSound::StopHold`]: the kit to stop, the unit's loop when `spell_id` names its
    /// spell or is `None`.
    pub(crate) fn stop_hold(&mut self, entity: Entity, spell_id: Option<u32>) -> Option<u32> {
        let (held, kit_sound) = self.0.get(&entity)?.kit;
        if spell_id.is_some_and(|s| s != held) {
            return None;
        }
        self.0.remove(&entity);
        Some(kit_sound)
    }

    /// An aura's kit sound stopped by id: the loop is gone when it was that kit.
    fn stopped_kit(&mut self, entity: Entity, kit_sound: u32) {
        if self.0.get(&entity).is_some_and(|h| h.kit.1 == kit_sound) {
            self.0.remove(&entity);
        }
    }

    /// [`SpellKitSound::Rebuilt`]: the kit to stop, the unit's loop when it rang before `at`
    /// and the rebuild does not play it again, which it does for a stage-2 loop whose spell
    /// `replays` (the channel's or an aura slot's).
    pub(crate) fn rebuilt(
        &mut self,
        entity: Entity,
        at: Stamp,
        replays: impl FnOnce(u32) -> bool,
    ) -> Option<u32> {
        let hold = *self.0.get(&entity)?;
        if hold.at >= at || (hold.state && replays(hold.kit.0)) {
            return None;
        }
        self.0.remove(&entity);
        Some(hold.kit.1)
    }

    /// The loop the unit holds: its spell and kit.
    #[cfg(test)]
    pub(crate) fn held(&self, entity: Entity) -> Option<(u32, u32)> {
        self.0.get(&entity).map(|h| h.kit)
    }
}

fn route_spell_kit_sounds(
    mut events: MessageReader<SpellKitSound>,
    transforms: Query<&Transform, Without<Camera3d>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
    // Each unit's live hold loop, so StopHold reaps only that kit's channels.
    mut hold_loops: Local<HoldLoops>,
    mut despawned: RemovedComponents<NetEntity>,
    stores: Query<&ObjectStore>,
) {
    for entity in despawned.read() {
        hold_loops.0.remove(&entity); // the channel dies with the source-channel despawn reaper
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
    // Applied after the frame's plays, which the stamps order against them, whatever system
    // wrote each.
    let mut rebuilds: Vec<(Entity, Stamp)> = Vec::new();
    for ev in events.read() {
        match *ev {
            SpellKitSound::Play {
                entity,
                spell_id,
                kit_sound,
                state,
                at,
            } => {
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
                    Ok(_) if looping => hold_loops.rang(entity, (spell_id, kit_sound), state, at),
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
            SpellKitSound::StopHold { entity, spell_id } => {
                if let Some(kit_sound) = hold_loops.stop_hold(entity, spell_id) {
                    super::kit::stop_source_kit(&mut out, entity, kit_sound);
                }
            }
            SpellKitSound::StopKit { entity, kit_sound } => {
                // An aura-drop reap: stop this kit's channels; an unrelated hold loop survives.
                hold_loops.stopped_kit(entity, kit_sound);
                super::kit::stop_source_kit(&mut out, entity, kit_sound);
            }
            SpellKitSound::Rebuilt { entity, at } => rebuilds.push((entity, at)),
        }
    }
    for (entity, at) in rebuilds {
        let replays = |spell| {
            spell != 0
                && stores.get(entity).is_ok_and(|store| {
                    store.0.unit_channel_spell() == spell
                        || store.0.unit_aura_ids().any(|a| a == spell)
                })
        };
        if let Some(kit_sound) = hold_loops.rebuilt(entity, at, replays) {
            super::kit::stop_source_kit(&mut out, entity, kit_sound);
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, route_spell_kit_sounds.in_set(WorldStage::Present));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x614150(spell, 0)` takes only its own spell's effects (`0x614167`): a stop keyed to
    /// another spell leaves the loop, its own takes it, and a replacing hold takes any.
    #[test]
    fn a_hold_stop_takes_only_its_own_spells_loop() {
        const ARCANE_MISSILES: u32 = 5143;
        const AM_HUM: u32 = 3136;
        const BLIZZARD: u32 = 10;
        let unit = Entity::PLACEHOLDER;
        let mut loops = HoldLoops::default();
        loops.rang(unit, (ARCANE_MISSILES, AM_HUM), true, None);
        assert_eq!(loops.stop_hold(unit, Some(BLIZZARD)), None);
        assert_eq!(loops.held(unit), Some((ARCANE_MISSILES, AM_HUM)));
        assert_eq!(loops.stop_hold(unit, Some(ARCANE_MISSILES)), Some(AM_HUM));
        assert_eq!(loops.held(unit), None);
        loops.rang(unit, (ARCANE_MISSILES, AM_HUM), true, None);
        assert_eq!(loops.stop_hold(unit, None), Some(AM_HUM));
    }

    /// The rebuild's loop `0x60ac10` stops every effect the unit holds, kit sounds included
    /// (`0x61fedc` links them there), and replays only the channel's kit (`0x612a30`) and the
    /// aura slots' (`0x5ff130`): a precast's hold rung before the swap stops, one rung after it
    /// plays on, and a channel's hold stays while the rebuild rings it again.
    #[test]
    fn a_rebuild_stops_the_hold_loops_rung_before_it_that_it_does_not_replay() {
        const FIREBALL: u32 = 133;
        const BUILDUP: u32 = 702;
        const ARCANE_MISSILES: u32 = 5143;
        const AM_HUM: u32 = 3136;
        let unit = Entity::PLACEHOLDER;
        let channel = |spell| spell == ARCANE_MISSILES;
        let mut loops = HoldLoops::default();

        let precast = crate::lanes::stamp();
        let swap = crate::lanes::stamp();
        loops.rang(unit, (FIREBALL, BUILDUP), false, Some(precast));
        assert_eq!(
            loops.rebuilt(unit, swap, channel),
            Some(BUILDUP),
            "rang before: stops"
        );
        assert_eq!(loops.held(unit), None);

        let swap = crate::lanes::stamp();
        let precast = crate::lanes::stamp();
        loops.rang(unit, (FIREBALL, BUILDUP), false, Some(precast));
        assert_eq!(
            loops.rebuilt(unit, swap, channel),
            None,
            "rang after: plays on"
        );
        assert_eq!(loops.held(unit), Some((FIREBALL, BUILDUP)));

        loops.rang(unit, (ARCANE_MISSILES, AM_HUM), true, None);
        let swap = crate::lanes::stamp();
        assert_eq!(
            loops.rebuilt(unit, swap, channel),
            None,
            "the channel's: rung again"
        );
        assert_eq!(loops.held(unit), Some((ARCANE_MISSILES, AM_HUM)));
        assert_eq!(
            loops.rebuilt(unit, crate::lanes::stamp(), |_| false),
            Some(AM_HUM)
        );
    }
}
