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
use crate::net::NetEntity;
use benilla_assets::WorldAssets;
use benilla_world::schedule::WorldStage;

use super::kit::{
    kit_looping, play_kit, play_kit_ext, KitRef, PlayExtras, SoundCategory, SoundKits,
};
use super::{AudioListener, SoundConfig, SoundOutput};

/// Each unit's live looping kit sounds, one per kit, as each rides an effect of its own on the
/// unit's list (`0x61fedc`). The effect carries its spell (`+0x18`), and `0x614150(spell, 0)` takes
/// every matching effect (`0x614167`), so one spell's stop spares another spell's loops.
#[derive(Default)]
pub(crate) struct HoldLoops(EntityHashMap<Vec<Hold>>);

#[derive(Clone, Copy)]
struct Hold {
    /// The spell whose kit rang it, and the kit.
    kit: (u32, u32),
    /// Rung by a cast's hold ([`SpellKitSound::Play`]'s `cast_hold`).
    cast_hold: bool,
    /// Its packet's place, which a display rebuild compares.
    at: Stamp,
}

/// One kit sound an edge stops on its unit, and how it fades.
pub(crate) struct KitStop {
    pub(crate) entity: Entity,
    pub(crate) kit_sound: u32,
    pub(crate) fade: kira::Tween,
}

impl HoldLoops {
    /// A looping kit (`spell_id`, `kit_sound`) rang on `entity`, one of its loops now; a kit rung
    /// again is the same loop, as a stop takes every channel on it. A play without a packet
    /// stamp rang now, after every packet already read.
    pub(crate) fn rang(
        &mut self,
        entity: Entity,
        kit: (u32, u32),
        cast_hold: bool,
        at: Option<Stamp>,
    ) {
        let at = at.unwrap_or_else(crate::lanes::stamp);
        let holds = self.0.entry(entity).or_default();
        holds.retain(|h| h.kit.1 != kit.1);
        holds.push(Hold { kit, cast_hold, at });
    }

    /// The kit sounds `edge` stops on its unit, the books moved to match: a hold's stop and a
    /// rebuild take the loops they name, an aura's stop and a replay their kit, a play nothing.
    /// Each fades as `0x6203e0` stops an effect's sound ([`kit_stop_fade`]).
    pub(crate) fn stops(&mut self, edge: &SpellKitSound) -> Vec<KitStop> {
        let (entity, kits) = match *edge {
            SpellKitSound::Play { .. } | SpellKitSound::PlayAt { .. } => return Vec::new(),
            SpellKitSound::StopHold { entity, spell_id } => {
                (entity, self.stop_hold(entity, spell_id))
            }
            SpellKitSound::StopKit { entity, kit_sound } => {
                // An aura-drop reap stops the kit's channels; an unrelated loop survives.
                self.take(entity, |h| h.kit.1 == kit_sound);
                (entity, vec![kit_sound])
            }
            SpellKitSound::Rebuilt { entity, at } => (entity, self.take(entity, |h| h.at < at)),
            SpellKitSound::Replay {
                entity, kit_sound, ..
            } => {
                // Whatever the unit still sounds on the kit fades as the rebuild stopped its
                // effect; the replay rings afresh.
                self.take(entity, |h| h.kit.1 == kit_sound);
                (entity, vec![kit_sound])
            }
        };
        kits.into_iter()
            .map(|kit_sound| KitStop {
                entity,
                kit_sound,
                fade: kit_stop_fade(),
            })
            .collect()
    }

    /// [`SpellKitSound::StopHold`]: the kits to stop, every loop `spell_id`'s kits rang, or with
    /// none every loop a cast's hold rang.
    pub(crate) fn stop_hold(&mut self, entity: Entity, spell_id: Option<u32>) -> Vec<u32> {
        self.take(entity, |h| match spell_id {
            Some(spell_id) => h.kit.0 == spell_id,
            None => h.cast_hold,
        })
    }

    /// Drop `entity`'s loops that are `gone`, returning their kits.
    fn take(&mut self, entity: Entity, mut gone: impl FnMut(&Hold) -> bool) -> Vec<u32> {
        let Some(holds) = self.0.get_mut(&entity) else {
            return Vec::new();
        };
        let mut kits = Vec::new();
        holds.retain(|h| {
            let stop = gone(h);
            if stop {
                kits.push(h.kit.1);
            }
            !stop
        });
        if holds.is_empty() {
            self.0.remove(&entity);
        }
        kits
    }

    /// The loops the unit holds, in ring order: each one's spell and kit.
    #[cfg(test)]
    pub(crate) fn held(&self, entity: Entity) -> Vec<(u32, u32)> {
        self.0
            .get(&entity)
            .map_or_else(Vec::new, |holds| holds.iter().map(|h| h.kit).collect())
    }
}

/// `0x6203e0`'s stop of an effect's sound, a 0.15 s fade (`0x6204b6` → `0x7a5a10`): every kit
/// sound's stop, a hold's and an aura's through `0x614150` (`0x6141d8`, as a kit sound's effect
/// from `0x61fec0` has no model) and the rebuild's through its loop `0x60ac10`.
fn kit_stop_fade() -> kira::Tween {
    super::mixer::fade(KIT_STOP_FADE_MS)
}

/// [`kit_stop_fade`]'s length.
const KIT_STOP_FADE_MS: u64 = 150;

fn route_spell_kit_sounds(
    mut events: MessageReader<SpellKitSound>,
    transforms: Query<&Transform, Without<Camera3d>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
    // Each unit's live loops, so a stop reaps only the kits it names.
    mut hold_loops: Local<HoldLoops>,
    mut despawned: RemovedComponents<NetEntity>,
) {
    for entity in despawned.read() {
        hold_loops.0.remove(&entity); // the channels die with the source-channel despawn reaper
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
    // Each edge where it falls: every play a router stamps is written ahead of the visuals set
    // that raises a rebuild, and the rebuild's replay after it.
    for ev in events.read() {
        for stop in hold_loops.stops(ev) {
            super::kit::stop_source_kit_with(&mut out, stop.entity, stop.kit_sound, stop.fade);
        }
        let (entity, spell_id, kit_sound, cast_hold, at) = match *ev {
            SpellKitSound::Play {
                entity,
                spell_id,
                kit_sound,
                cast_hold,
                at,
            } => {
                if played_now.contains(&(entity, kit_sound)) {
                    continue;
                }
                (entity, spell_id, kit_sound, cast_hold, at)
            }
            // The replay rings afresh, a ring this frame or not.
            SpellKitSound::Replay {
                entity,
                spell_id,
                kit_sound,
                cast_hold,
                at,
            } => (entity, spell_id, kit_sound, cast_hold, Some(at)),
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
                continue;
            }
            SpellKitSound::StopHold { .. }
            | SpellKitSound::StopKit { .. }
            | SpellKitSound::Rebuilt { .. } => continue,
        };
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
            Ok(_) if looping => hold_loops.rang(entity, (spell_id, kit_sound), cast_hold, at),
            Ok(_) => {}
            Err(e) => warn!("spell kit sound {kit_sound}: {e:#}"),
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

    const ARCANE_MISSILES: u32 = 5143;
    const AM_HUM: u32 = 3136;
    const FIREBALL: u32 = 133;
    const BUILDUP: u32 = 702;
    const ARCANE_INTELLECT: u32 = 1459;
    const AURA_LOOP: u32 = 4001;
    const NONE: [u32; 0] = [];

    /// The router's books over one edge: the kits it stops, then its ring, every kit a loop.
    fn edge(loops: &mut HoldLoops, edge: SpellKitSound) -> Vec<u32> {
        let stopped = loops
            .stops(&edge)
            .into_iter()
            .map(|s| s.kit_sound)
            .collect();
        match edge {
            SpellKitSound::Play {
                entity,
                spell_id,
                kit_sound,
                cast_hold,
                at,
            } => loops.rang(entity, (spell_id, kit_sound), cast_hold, at),
            SpellKitSound::Replay {
                entity,
                spell_id,
                kit_sound,
                cast_hold,
                at,
            } => loops.rang(entity, (spell_id, kit_sound), cast_hold, Some(at)),
            _ => {}
        }
        stopped
    }

    fn play(spell_id: u32, kit_sound: u32, cast_hold: bool, at: Option<Stamp>) -> SpellKitSound {
        SpellKitSound::Play {
            entity: Entity::PLACEHOLDER,
            spell_id,
            kit_sound,
            cast_hold,
            at,
        }
    }

    /// `0x614150(spell, 0)` takes only its own spell's effects (`0x614167`): a stop keyed to
    /// another spell leaves the loop, its own takes it, and a replacing hold takes any cast's.
    #[test]
    fn a_hold_stop_takes_only_its_own_spells_loop() {
        let unit = Entity::PLACEHOLDER;
        let mut loops = HoldLoops::default();
        edge(&mut loops, play(ARCANE_MISSILES, AM_HUM, true, None));
        assert_eq!(loops.stop_hold(unit, Some(10)), NONE, "Blizzard's stop");
        assert_eq!(loops.held(unit), [(ARCANE_MISSILES, AM_HUM)]);
        assert_eq!(loops.stop_hold(unit, Some(ARCANE_MISSILES)), [AM_HUM]);
        assert_eq!(loops.held(unit), []);
        edge(&mut loops, play(ARCANE_MISSILES, AM_HUM, true, None));
        assert_eq!(loops.stop_hold(unit, None), [AM_HUM]);
    }

    /// Each loop is an effect of its own on the unit's list (`0x61fedc`): a precast rung after a
    /// rebuild, in the frame the rebuild replays an aura's loop, still stops at its spell's go
    /// (`0x614150`), and the aura's at its removal; a replacing hold spares the aura's.
    #[test]
    fn each_loop_stops_with_its_own_kit() {
        let unit = Entity::PLACEHOLDER;
        let mut loops = HoldLoops::default();
        edge(
            &mut loops,
            play(
                ARCANE_INTELLECT,
                AURA_LOOP,
                false,
                Some(crate::lanes::stamp()),
            ),
        );
        let rebuild = crate::lanes::stamp();
        // The rebuild's frame in the router's order: a later packet's precast, then the rebuild
        // and its replay.
        let precast = play(FIREBALL, BUILDUP, true, Some(crate::lanes::stamp()));
        assert_eq!(edge(&mut loops, precast), NONE);
        let rebuilt = SpellKitSound::Rebuilt {
            entity: unit,
            at: rebuild,
        };
        assert_eq!(edge(&mut loops, rebuilt), [AURA_LOOP], "rang before it");
        let replay = SpellKitSound::Replay {
            entity: unit,
            spell_id: ARCANE_INTELLECT,
            kit_sound: AURA_LOOP,
            cast_hold: false,
            at: rebuild,
        };
        edge(&mut loops, replay);
        assert_eq!(
            loops.held(unit),
            [(FIREBALL, BUILDUP), (ARCANE_INTELLECT, AURA_LOOP)]
        );

        let replacing = SpellKitSound::StopHold {
            entity: unit,
            spell_id: None,
        };
        assert_eq!(edge(&mut loops, replacing), [BUILDUP], "a new cast's");
        edge(&mut loops, precast);
        let go = SpellKitSound::StopHold {
            entity: unit,
            spell_id: Some(FIREBALL),
        };
        assert_eq!(edge(&mut loops, go), [BUILDUP], "the precast's go");
        let removal = SpellKitSound::StopKit {
            entity: unit,
            kit_sound: AURA_LOOP,
        };
        assert_eq!(edge(&mut loops, removal), [AURA_LOOP], "the aura's removal");
        assert_eq!(loops.held(unit), []);
    }

    /// The rebuild's loop `0x60ac10` stops every effect the unit holds, kit sounds included
    /// (`0x61fedc` links them there): a loop rung before the swap stops, the channel's too, whose
    /// kit the replay rings afresh; one rung after it plays on.
    #[test]
    fn a_rebuild_stops_every_hold_loop_rung_before_it() {
        let unit = Entity::PLACEHOLDER;
        let mut loops = HoldLoops::default();
        let rebuilt = |at| SpellKitSound::Rebuilt { entity: unit, at };

        edge(&mut loops, play(ARCANE_MISSILES, AM_HUM, true, None));
        let swap = crate::lanes::stamp();
        let precast = play(FIREBALL, BUILDUP, true, Some(crate::lanes::stamp()));
        edge(&mut loops, precast);
        assert_eq!(edge(&mut loops, rebuilt(swap)), [AM_HUM], "the channel's");
        assert_eq!(loops.held(unit), [(FIREBALL, BUILDUP)], "rang after");
    }

    /// Every stop of a kit's sound is `0x6203e0`'s, a 0.15 s fade (`0x6204b6` → `0x7a5a10`): a
    /// hold's and an aura's through `0x614150` (`0x6141d8`), the rebuild's through its loop
    /// `0x60ac10`, never the 15 ms de-click of a force-stop.
    #[test]
    fn every_kit_stop_fades_over_0_15_s() {
        let unit = Entity::PLACEHOLDER;
        for i in 0..5 {
            let mut loops = HoldLoops::default();
            edge(&mut loops, play(FIREBALL, BUILDUP, true, None));
            edge(&mut loops, play(ARCANE_INTELLECT, AURA_LOOP, false, None));
            let at = crate::lanes::stamp();
            let stop = [
                SpellKitSound::StopHold {
                    entity: unit,
                    spell_id: Some(FIREBALL),
                },
                SpellKitSound::StopHold {
                    entity: unit,
                    spell_id: None,
                },
                SpellKitSound::StopKit {
                    entity: unit,
                    kit_sound: AURA_LOOP,
                },
                SpellKitSound::Rebuilt { entity: unit, at },
                SpellKitSound::Replay {
                    entity: unit,
                    spell_id: ARCANE_INTELLECT,
                    kit_sound: AURA_LOOP,
                    cast_hold: false,
                    at,
                },
            ][i];
            let fades: Vec<_> = loops
                .stops(&stop)
                .into_iter()
                .map(|s| s.fade.duration)
                .collect();
            assert!(
                !fades.is_empty()
                    && fades
                        .iter()
                        .all(|&d| d == std::time::Duration::from_millis(150)),
                "{stop:?}: {fades:?}"
            );
        }
    }
}
