//! Creature voice from CreatureSoundData: the barks (aggro, pet, death), the alert flare, the
//! anim-tag vocals, the body loop, the pet-dismiss sound and the hard-landing grunt. The swing
//! vocals and the `$AH0..3` custom attacks are in [`super::combat`].
//!
//! The reference never plays the stun (class 4, column 7) or jump-start (class 11, column 25)
//! vocals: no call site dispatches them. Jump-end (class 12, column 26) is dispatched with force 1
//! on landing (`0x602d00`, at `0x602dac` and `0x602dcb`), but columns 25 and 26 are empty in every
//! `CreatureSoundData.dbc` row. None of the three is played here.

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use benilla_formats::CreatureVoiceCatalog;
use benilla_protocol::EntityKind;

use crate::creature_anim::AnimSoundEvent;
use crate::entities::mount::{MountBody, MountChild};
use crate::net::{FieldChanged, NetEntity, ObjectStore, SelfPlayer};
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::kit::{
    bark_chance_pass, object_sound_playing, play_kit, play_kit_ext, source_kit_playing,
    stop_source_kit, stop_unit_voice, unit_voice_state, KitRef, Latch, PlayExtras, SoundCategory,
    SoundKits, STAND_CHANCE, STAND_COOLDOWN,
};
use super::{AudioListener, SoundConfig, SoundOutput};

/// The display→voice catalog (CreatureDisplayInfo.SoundID → CreatureSoundData).
#[derive(Resource)]
pub(super) struct CreatureVoices(pub(super) CreatureVoiceCatalog);

fn load_creature_voices(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_creature_voice_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} creature voice rows", cat.len());
            commands.insert_resource(CreatureVoices(cat));
        }
        Err(e) => warn!("sound: creature voices failed to load: {e:#}"),
    }
}

/// `0x623a40`'s bark states in its jump table `0x623afc`; the number is the priority (higher
/// wins, `0x623a82`). State 3 plays nothing and has no known caller.
const BARK_AGGRO: u8 = 0;
const BARK_PET_ORDER: u8 = 1;
const BARK_PET_ATTACK: u8 = 2;
const BARK_DEATH: u8 = 4;

/// The priority test (`0x623a66`-`0x623a88`) against the sounding bark's state: a free slot
/// admits every state, a held one only a strictly higher one (`jle`), so a burst plays once.
const fn bark_admitted(latched: Option<u8>, state: u8) -> bool {
    match latched {
        None => true,
        Some(held) => state > held,
    }
}

/// The jump table `0x623afc`; 0 plays nothing, for state 3 (`0x623af0`), a state past 4
/// (`0x623aa0`) and an empty column alike.
const fn bark_kit(voice: &benilla_formats::CreatureVoice, state: u8) -> u32 {
    match state {
        BARK_AGGRO => voice.aggro,
        BARK_PET_ORDER => voice.pet_order,
        BARK_PET_ATTACK => voice.pet_attack,
        BARK_DEATH => voice.death,
        _ => 0,
    }
}

/// The creature bark dispatcher `0x623a40(unit, state)`: 0 the hostile aggro flare, 1 and 2 the
/// pet's order and attack barks, 4 the death cry. Its order is the reference's:
///
/// 1. No voice row, no bark (`0x623a4a`).
/// 2. A server-pushed object sound live on the unit mutes it (`0x623a59` → `0x4591f0`); a direct
///    call, so unlike the class route `0x623490`/`0x62f880` players are not exempt.
/// 3. While `[unit+0xb20]` holds a live bark, a state at or below its own is dropped.
/// 4. Stop the old bark and latch the new (`0x623a8d`/`0x623a95`), before the column is read.
/// 5. Resolve the column and play: a zero column still silences step 4's victim (`0x623aee`).
fn play_bark(
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    listener: Vec3,
    unit: Entity,
    pos: Vec3,
    voice: &benilla_formats::CreatureVoice,
    state: u8,
) {
    if object_sound_playing(out, unit) {
        return;
    }
    if !bark_admitted(unit_voice_state(out, unit), state) {
        return;
    }
    stop_unit_voice(out, unit);
    let kit = bark_kit(voice, state);
    if kit == 0 {
        return;
    }
    if let Err(e) = play_kit_ext(
        kits,
        assets,
        out,
        config,
        listener,
        KitRef::Id(kit),
        Some(pos),
        SoundCategory::Sfx,
        PlayExtras {
            source: Some(unit),
            latch: Latch::Voice(state),
            ..default()
        },
    ) {
        warn!("creature bark state {state} (kit {kit}): {e:#}");
    }
}

/// The death cry `0x623a40(4)` on either edge: health falling to 0 (`0x6046f0` at `0x6047a3`, the
/// death handler `0x605860`) or `UNIT_DYNFLAG_DEAD` set (`0x600543`), so a feign cries too. A
/// unit that streams in dead cries nothing: create edges are suppressed.
fn death_vocals(
    mut edges: MessageReader<FieldChanged>,
    units: Query<(&NetEntity, &Transform)>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    use benilla_protocol::field::{FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_HEALTH, UNIT_DYNFLAG_DEAD};
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for e in edges.read() {
        let died = (e.unit_field(FIELD_UNIT_HEALTH) && e.old > 0 && e.new == 0)
            || (e.unit_field(FIELD_UNIT_DYNAMIC_FLAGS)
                && e.old & UNIT_DYNFLAG_DEAD == 0
                && e.new & UNIT_DYNFLAG_DEAD != 0);
        if !died {
            continue;
        }
        let Ok((net, transform)) = units.get(e.entity) else {
            continue;
        };
        let Some(voice) = net.display_id.and_then(|d| voices.0.for_display(d)) else {
            continue;
        };
        play_bark(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            e.entity,
            transform.translation,
            voice,
            BARK_DEATH,
        );
    }
}

/// `SMSG_AI_REACTION` vocals, audio only (`0x6056e0` plays no animation). HOSTILE is the aggro
/// bark (column 10) through [`play_bark`], whose slot drops a repeat while one sounds: vmangos
/// sends HOSTILE on every target acquisition (`Unit::Attack`) and every pet autocast or
/// self-picked target (`Unit::SendPetAIReaction`), so a pet draws them in bursts.
///
/// ALERT is the class-8 route (`vtable+0x88(8,0)` → `0x623490` → column 13): its roll threshold
/// is 100, inclusive, so it always plays unless a server-pushed object sound
/// (`SMSG_PLAY_OBJECT_SOUND`, registered as `Latch::ObjectSound`, tested by `0x4591f0`) is live on
/// a non-player unit. It stores no latch.
fn ai_reaction_vocals(
    mut reactions: MessageReader<crate::net::AiReactionMessage>,
    units: Query<(&NetEntity, &Transform, Option<&MountChild>)>,
    mounts: Query<&NetEntity, With<MountBody>>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if reactions.is_empty() {
        return;
    }
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for r in reactions.read() {
        let Ok((net, transform, mount_child)) = units.get(r.unit) else {
            continue;
        };
        // ALERT reads the mount-preferred row (`0x60c480`: `+0xb44 ?: +0xb40`), so a mounted unit
        // alerts in its mount's voice; HOSTILE, like death, reads the rider's own row.
        let display = if r.hostile {
            net.display_id
        } else {
            mount_child
                .and_then(|mc| mounts.get(mc.0).ok())
                .and_then(|m| m.display_id)
                .or(net.display_id)
        };
        let Some(voice) = display.and_then(|d| voices.0.for_display(d)) else {
            continue;
        };
        // HOSTILE is the bark route `0x623a40(0)`.
        if r.hostile {
            play_bark(
                &mut kits,
                &assets,
                &mut out,
                &config,
                listener,
                r.unit,
                transform.translation,
                voice,
                BARK_AGGRO,
            );
            continue;
        }
        // ALERT neither tests nor stores the voice slot. Its object-sound mute (`0x6234cb` →
        // `0x4591f0`) runs through a virtual whose CGPlayer twin `0x62f880` omits it.
        if voice.alert == 0 {
            continue;
        }
        if net.kind != EntityKind::Player && object_sound_playing(&out, r.unit) {
            continue;
        }
        if let Err(e) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(voice.alert),
            Some(transform.translation),
            SoundCategory::Sfx,
        ) {
            warn!("alert vocal (kit {}): {e:#}", voice.alert);
        }
    }
}

/// `SMSG_PET_ACTION_SOUND`: `0x6040c0` plays selector 0 as `0x623a40(1)`, 1 as `0x623a40(2)`,
/// and any other value not at all. Only the Imp, Succubus, Doomguard and Voidwalker voices carry
/// these columns, and vmangos sends it only for a `SUMMON_PET`, on a 10% roll per order
/// (`Unit.cpp:8960`, `PetHandler.cpp:522`, `PetAI.cpp:335`).
fn pet_talk_vocals(
    mut talks: MessageReader<crate::net::PetTalkMessage>,
    units: Query<(&NetEntity, &Transform)>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if talks.is_empty() {
        return;
    }
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for t in talks.read() {
        let state = match t.talk {
            benilla_protocol::messages::PET_TALK_ORDER => BARK_PET_ORDER,
            benilla_protocol::messages::PET_TALK_ATTACK => BARK_PET_ATTACK,
            _ => continue,
        };
        let Ok((net, transform)) = units.get(t.unit) else {
            continue;
        };
        // `0x6040c0` resolves the guid with the `TYPEMASK_UNIT` test `0x468460(ecx = 8)`, which a
        // player passes.
        if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
            continue;
        }
        // The pet's own row: `0x623a40` reads the base `[unit+0xb40]`, not the mount redirect.
        let Some(voice) = net.display_id.and_then(|d| voices.0.for_display(d)) else {
            continue;
        };
        play_bark(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            t.unit,
            transform.translation,
            voice,
            state,
        );
    }
}

/// `SMSG_PET_DISMISS_SOUND`: `CreatureSoundData` column 29 at the packet's point. Not a bark:
/// `0x604140` resolves the row by model id (`CreatureModelData[id].SoundID`, no display step, no
/// fallback), reads `[row+0x74]` and plays it with no unit, latch or mute. vmangos never sends it.
fn pet_dismiss_sounds(
    mut dismissals: MessageReader<crate::net::PetDismissSoundMessage>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if dismissals.is_empty() {
        return;
    }
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for d in dismissals.read() {
        let kit = voices
            .0
            .for_model(d.model_id)
            .map(|v| v.pet_dismiss)
            .unwrap_or(0);
        if kit == 0 {
            continue;
        }
        if let Err(e) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(kit),
            Some(d.pos),
            SoundCategory::Sfx,
        ) {
            warn!("pet dismiss sound (kit {kit}): {e:#}");
        }
    }
}

/// The body loop (column 23, an elemental's rumble, a shredder's engine) while alive, under the
/// reference's `0x623800` gate: health > 0, `UNIT_DYNFLAG_DEAD` clear, the column nonzero, not
/// already playing. Reconciled every frame, which also re-arms it after an out-of-range cull. It
/// loops whatever the kit's `0x200` flag, which half the column's kits omit: `0x461d80` never
/// reads the kit's flags.
fn creature_body_loops(
    units: Query<(
        Entity,
        &NetEntity,
        &ObjectStore,
        &Transform,
        Option<&MountChild>,
    )>,
    mounts: Query<&NetEntity, With<MountBody>>,
    // The kit last armed per unit: a mount swap changes it, and the old loop must stop.
    mut armed: Local<EntityHashMap<u32>>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for (entity, net, store, transform, mount_child) in &units {
        if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
            continue;
        }
        // The mount-preferred row (`0x60c480`: `+0xb44 ?: +0xb40`): a mechanostrider's engine.
        let display = mount_child
            .and_then(|mc| mounts.get(mc.0).ok())
            .and_then(|m| m.display_id)
            .or(net.display_id);
        let kit = display
            .and_then(|d| voices.0.for_display(d))
            .map(|v| v.loop_sound)
            .unwrap_or(0);
        // `0x623800`'s gate: raw health (`0x623817`), absent read as 0 so a unit awaiting its
        // snapshot stays silent, and `UNIT_DYNFLAG_DEAD` (`0x62381e`, re-evaluated at `0x60053c`).
        let alive = store.0.unit_health().unwrap_or(0) > 0 && !store.0.unit_reads_dead();
        let desired = if alive { kit } else { 0 };
        // Stop a superseded loop: death, or a mount swap that changed the row.
        if let Some(&prev) = armed.get(&entity) {
            if prev != desired && source_kit_playing(&out, entity, prev) {
                stop_source_kit(&mut out, entity, prev);
            }
        }
        if desired == 0 {
            armed.remove(&entity);
            continue;
        }
        armed.insert(entity, desired);
        if !source_kit_playing(&out, entity, desired) {
            // Out of range this allocates nothing, and the next frame retries.
            if let Err(e) = play_kit_ext(
                &mut kits,
                &assets,
                &mut out,
                &config,
                listener,
                KitRef::Id(desired),
                Some(transform.translation),
                SoundCategory::Sfx,
                PlayExtras {
                    source: Some(entity),
                    force_loop: true,
                    ..default()
                },
            ) {
                warn!("body loop (kit {desired}): {e:#}");
            }
        }
    }
}

/// Routes the CreatureSoundData anim tags. A mounted unit's tags fire on the mount child, which
/// carries the mount's display, so they resolve the mount's voice (the reference's `0x60c480`).
///
/// - `$FD1..$FD4`: fidget columns 1 to 4, ungated; `0x6232c0` → `0x623440` → `0x6230a0` reads
///   `row[+0x38 + 4*(n-1)]`, and a zero column is silent. `$FD5..$FD9` are silent in the
///   reference too, which reads only four fidget columns. The reference plays these and `$FDX`
///   at the unit's origin + 2.0 z (`0x6232c0`); here they play at the origin.
/// - `$WNG`/`$WGG`: wing flap and glide, on the class route (`0x5fff9c` class 7, `0x5fff78` class
///   10) against a threshold of 100, so always played, outside the mute set {0, 1, 2, 3, 8}.
/// - `$FDX`: the stand vocal, never the local player's (`0x6236c2`), then a [`STAND_CHANCE`] roll,
///   then a [`STAND_COOLDOWN`] on one world-wide timestamp. `Crocodile.m2` keys it at t=0 of a
///   looping Stand, so ungated it would croak every 4 s.
fn creature_anim_vocals(
    mut events: MessageReader<AnimSoundEvent>,
    // GlobalTransform: the mount child's local Transform is seat-relative.
    units: Query<(&NetEntity, &GlobalTransform, Has<SelfPlayer>)>,
    // The class-5 window: one timestamp for the whole world, the reference's `[0xc4e0e4]`.
    mut stand_window: Local<Option<std::time::Duration>>,
    time: Res<Time>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if events.is_empty() {
        return;
    }
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    let now = time.elapsed();
    for ev in events.read() {
        let Ok((net, transform, is_self)) = units.get(ev.entity) else {
            continue;
        };
        let Some(voice) = net.display_id.and_then(|d| voices.0.for_display(d)) else {
            continue;
        };
        // `$FDX`'s gates run before the zero-column bail, and any allowed attempt stamps the
        // window, even a silent or culled one (`0x623290`).
        if ev.ident == *b"$FDX" {
            if is_self {
                continue; // `0x6236c2`
            }
            if !bark_chance_pass(STAND_CHANCE, kits.roll()) {
                continue;
            }
            if stand_window.is_some_and(|last| now < last + STAND_COOLDOWN) {
                continue;
            }
            *stand_window = Some(now);
        }
        let kit = match &ev.ident {
            b"$FD1" => voice.fidget[0],
            b"$FD2" => voice.fidget[1],
            b"$FD3" => voice.fidget[2],
            b"$FD4" => voice.fidget[3],
            b"$FDX" => voice.stand,
            b"$WNG" => voice.wing_flap,
            b"$WGG" => voice.wing_glide,
            _ => 0,
        };
        if kit == 0 {
            continue;
        }
        if let Err(e) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(kit),
            Some(transform.translation()),
            SoundCategory::Sfx,
        ) {
            warn!("creature vocal (kit {kit}): {e:#}");
        }
    }
}

/// The hard-landing grunt: the reference's landing predictor `0x602d00` voices a landing past the
/// hard threshold with the class-2 wound vocal (`0x602d84`, column `+0xc`, `injury[0]`), predicted
/// client-side; `SMSG_ENVIRONMENTALDAMAGELOG` plays nothing. The dust in
/// `creature_anim::env_damage` shares [`crate::creature_anim::HARD_LANDING_DESCENT`].
///
/// This gates on fall height alone. The reference also sends a mover with SAFE_FALL, SWIMMING or
/// FIXED_Z (`0x602d48`) to the soft landing at every height, and a dead or ghost unit
/// (`0x605f30`, `0x602d71`) in the 13-70 yd band.
fn fall_landing_vocals(
    mut landings: MessageReader<crate::creature_anim::HardLanding>,
    units: Query<(&NetEntity, &Transform)>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if landings.is_empty() {
        return;
    }
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for l in landings.read() {
        if l.descent <= crate::creature_anim::HARD_LANDING_DESCENT {
            continue;
        }
        let Ok((net, transform)) = units.get(l.entity) else {
            continue;
        };
        let kit = net
            .display_id
            .and_then(|d| voices.0.for_display(d))
            .map(|v| v.injury[0])
            .unwrap_or(0);
        if kit == 0 {
            continue;
        }
        if let Err(e) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(kit),
            Some(transform.translation),
            SoundCategory::Sfx,
        ) {
            warn!("fall-landing vocal (kit {kit}): {e:#}");
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, load_creature_voices.after(AssetSet::Open))
        .add_systems(
            Update,
            (
                death_vocals,
                creature_anim_vocals,
                ai_reaction_vocals,
                pet_talk_vocals,
                pet_dismiss_sounds,
                fall_landing_vocals,
                creature_body_loops,
            )
                .in_set(WorldStage::Present),
        );
}

#[cfg(test)]
mod tests {
    use super::super::kit::occupies_voice_slot;
    use super::*;

    /// Every `SMSG_AI_REACTION` HOSTILE arrival for a bear pet in one traced session, in seconds
    /// from the first: 63 inside 19 s, the rate vmangos sends by design.
    const BEAR_BURST_S: [f32; 63] = [
        0.000, 0.000, 0.000, 1.417, 2.950, 4.481, 5.997, 7.530, 9.046, 10.589, 10.670, 10.792,
        12.312, 12.413, 12.546, 12.603, 12.713, 12.813, 12.912, 13.013, 13.112, 13.214, 13.315,
        13.424, 13.528, 13.624, 13.740, 13.829, 13.928, 14.032, 14.137, 14.239, 14.331, 14.449,
        14.545, 14.646, 14.745, 14.847, 16.393, 16.499, 16.684, 16.789, 16.909, 17.013, 17.096,
        17.205, 17.303, 17.411, 17.540, 17.612, 17.711, 17.811, 17.920, 18.032, 18.111, 18.232,
        18.340, 18.418, 18.553, 18.623, 18.728, 18.827, 18.928,
    ];

    /// Kit 478 `BearAggro`'s one variation, `mBearAggroA.wav`: 3.166 s at 22 050 Hz.
    const BEAR_AGGRO_S: f32 = 3.166;

    /// Replays a burst through the `[unit+0xb20]` slot and returns the arrivals that sound; the
    /// slot frees when its sound ends.
    fn barks_that_sound(arrivals: &[f32], clip: f32) -> Vec<f32> {
        let bear = Entity::from_raw_u32(1).expect("valid entity id");
        let mut slot: Option<(Option<Entity>, Latch, f32)> = None;
        let mut sounded = Vec::new();
        for &t in arrivals {
            if slot.is_some_and(|(_, _, ends)| ends <= t) {
                slot = None;
            }
            if slot.is_some_and(|(src, l, _)| occupies_voice_slot(src, l, bear)) {
                continue; // category 0 is the lowest: a live bark wins
            }
            slot = Some((Some(bear), Latch::Voice(BARK_AGGRO), t + clip));
            sounded.push(t);
        }
        sounded
    }

    /// Through the voice slot the measured burst is five barks, none overlapping.
    #[test]
    fn the_voice_slot_collapses_the_measured_bear_burst() {
        assert_eq!(
            BEAR_BURST_S.len(),
            63,
            "ungated, every arrival sounds — the reported symptom",
        );

        let sounded = barks_that_sound(&BEAR_BURST_S, BEAR_AGGRO_S);
        assert_eq!(
            sounded,
            vec![0.0, 4.481, 9.046, 12.312, 16.393],
            "the slot admits a bark only once the last one has finished",
        );
        for pair in sounded.windows(2) {
            assert!(
                pair[1] - pair[0] >= BEAR_AGGRO_S,
                "two barks {pair:?} overlap — the phasing the report describes",
            );
        }
    }

    /// The slot is per unit: a second bear keeps its own voice.
    #[test]
    fn the_voice_slot_is_per_unit() {
        let (a, b) = (
            Entity::from_raw_u32(1).expect("valid entity id"),
            Entity::from_raw_u32(2).expect("valid entity id"),
        );
        let barking = (Some(a), Latch::Voice(BARK_AGGRO));
        assert!(occupies_voice_slot(barking.0, barking.1, a));
        assert!(!occupies_voice_slot(barking.0, barking.1, b));
    }

    /// The priority rule at each boundary; refusing an equal state is what collapses a burst.
    #[test]
    fn a_held_bark_admits_only_a_strictly_higher_state() {
        // A free slot admits everything, including the lowest state.
        for state in [BARK_AGGRO, BARK_PET_ORDER, BARK_PET_ATTACK, BARK_DEATH] {
            assert!(bark_admitted(None, state), "free slot, state {state}");
        }
        // Equal is refused: `jle`, not `jl`.
        for state in [BARK_AGGRO, BARK_PET_ORDER, BARK_PET_ATTACK, BARK_DEATH] {
            assert!(!bark_admitted(Some(state), state), "equal, state {state}");
        }
        // A pet's attack bark cuts through its own aggro flare and order bark.
        assert!(bark_admitted(Some(BARK_AGGRO), BARK_PET_ORDER));
        assert!(bark_admitted(Some(BARK_AGGRO), BARK_PET_ATTACK));
        assert!(bark_admitted(Some(BARK_PET_ORDER), BARK_PET_ATTACK));
        // Nothing cuts through the death cry, the table's maximum.
        assert!(bark_admitted(Some(BARK_PET_ATTACK), BARK_DEATH));
        for state in [BARK_AGGRO, BARK_PET_ORDER, BARK_PET_ATTACK] {
            assert!(
                !bark_admitted(Some(BARK_DEATH), state),
                "over death: {state}"
            );
        }
        // The order bark does not interrupt an attack bark.
        assert!(!bark_admitted(Some(BARK_PET_ATTACK), BARK_PET_ORDER));
    }

    /// The five-arm jump table, column for column, and its two silences.
    #[test]
    fn the_bark_table_maps_each_state_to_its_column() {
        let voice = benilla_formats::CreatureVoice {
            exertion: [0; 2],
            injury: [0; 3],
            death: 314,
            stun: 0,
            stand: 0,
            footstep_class: 0,
            aggro: 694,
            wing_flap: 0,
            wing_glide: 0,
            alert: 1107,
            fidget: [0; 4],
            custom_attack: [0; 4],
            loop_sound: 0,
            impact_type: 0,
            jump_start: 0,
            jump_end: 0,
            pet_attack: 9097,
            pet_order: 9098,
            pet_dismiss: 9096,
        };
        assert_eq!(bark_kit(&voice, BARK_AGGRO), 694);
        assert_eq!(bark_kit(&voice, BARK_PET_ORDER), 9098);
        assert_eq!(bark_kit(&voice, BARK_PET_ATTACK), 9097);
        assert_eq!(bark_kit(&voice, BARK_DEATH), 314);
        // State 3 reads no column and a state past 4 is the range check's default; `alert`
        // (column 13) and `pet_dismiss` (column 29) are reached off this table.
        assert_eq!(bark_kit(&voice, 3), 0);
        assert_eq!(bark_kit(&voice, 5), 0);
        assert_eq!(bark_kit(&voice, 255), 0);
    }
}
