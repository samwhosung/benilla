//! Emote sounds: `SMSG_TEXT_EMOTE` plays the performer's race/sex voice kit (`EmotesTextSound`)
//! for the active player and the party only. Our own `/wave` goes out as `CMSG_TEXT_EMOTE` and
//! plays through the server echo, as in 1.12. The catalog also serves `crate::ui_chat` and
//! `crate::creature_anim`.

use bevy::prelude::*;

use benilla_formats::EmoteSoundCatalog;
use benilla_protocol::messages::ObjectType;

use crate::entities::{AttachPoints, Creatures};
use crate::net::{EmoteKind, EmoteMessage, ObjectStore, SelfGuid};
use crate::spell::group_relation::GroupRoster;
use crate::ui_social::SocialState;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::kit::{
    play_kit_ext, stop_older_emote_voices, KitRef, Latch, PlayExtras, SoundCategory, SoundKits,
};
use super::{AudioListener, SoundConfig, SoundOutput};

/// The emote-audio catalog (also the chat sender's `/name` → text-id resolver).
#[derive(Resource)]
pub(crate) struct EmoteSounds(pub(crate) EmoteSoundCatalog);

impl EmoteSounds {
    /// Resolve a `/command` name (case-insensitive) to its EmotesText id.
    pub(crate) fn text_id(&self, name: &str) -> Option<u32> {
        self.0.text_id(name)
    }

    /// An `Emotes.dbc` id's `AnimID`.
    pub(crate) fn anim(&self, emote_id: u32) -> Option<u32> {
        self.0.anim(emote_id)
    }

    /// An `Emotes.dbc` row's raw `AnimID` for the `UNIT_NPC_EMOTESTATE` resolver, `0` included.
    pub(crate) fn state_anim(&self, emote_id: u32) -> Option<u32> {
        self.0.state_anim(emote_id)
    }

    /// A text emote's `Emotes.dbc` id (`EmotesText.dbc` `EmoteID`); 0 is chat-only (`/thank`).
    pub(crate) fn text_emote(&self, text_id: u32) -> Option<u32> {
        self.0.text_emote(text_id)
    }

    /// The `Emotes.dbc` id in one of the five hard-coded gesture slots.
    pub(crate) fn gesture(&self, slot: usize) -> Option<u32> {
        self.0.gesture(slot)
    }

    /// An `Emotes.dbc` id's raw `EmoteFlags`, read by the posture-eligibility gate (`0x47db40`).
    pub(crate) fn emote_flags(&self, emote_id: u32) -> Option<u32> {
        self.0.emote_flags(emote_id)
    }

    /// The stand state a posture emote (`EmoteSpecProc == 1`) sets, as in `DoEmote`'s state
    /// branch (`0x5ef560` → `0x5ed430`).
    pub(crate) fn posture_state(&self, emote_id: u32) -> Option<u32> {
        self.0.posture_state(emote_id)
    }

    /// The `$ESD` anim event's kit for a looping state emote: `EventSoundID` only when
    /// `EmoteSpecProc == 2` (`row[+0x10] == 2` in the handler `0x6239f0`), so a one-shot emote id
    /// in the state field stays silent.
    pub(crate) fn state_event_sound(&self, emote_id: u32) -> Option<u32> {
        (self.0.spec_proc(emote_id) == Some(2))
            .then(|| self.0.event_sound(emote_id))
            .flatten()
    }
}

fn load_emote_sounds(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_emote_sound_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} emote commands", cat.len());
            commands.insert_resource(EmoteSounds(cat));
        }
        Err(e) => warn!("sound: emote catalog failed to load: {e:#}"),
    }
}

/// The attachment the emote voice plays at (`0x623c3a push 0x11`).
pub(super) const VOICE_ATTACH: u16 = 17;

/// Who hears a text emote's voice: the composer's tail (`0x623c80`) plays it for relation class
/// 0, the active player, or 2, a player in one of the four party slots (`0x623cc2`-`0x623cce`,
/// `0x5efea0`). A raid-only member is class 4, a hostile player 6, a creature 1/3/5/7/8 and an
/// unstreamed performer 9, none of which play. An ignored performer never reaches the composer
/// (`0x49dc36`).
fn voice_audible(
    performer: u64,
    me: Option<u64>,
    performer_is_player: bool,
    roster: &GroupRoster,
    ignored: bool,
) -> bool {
    if ignored {
        return false;
    }
    Some(performer) == me || performer_is_player && roster.in_party(me, performer)
}

/// `0x623c10`: play `kit` at the unit's attachment `0x11`, then stop the unit's previous emote
/// voice (`[unit+0xb28]`, `0x623c63`-`0x623c70`); a play that opens no channel stops nothing
/// (`0x623c55`). Shared by the text-emote voice and the `$CSD` anim event.
pub(super) fn play_emote_voice(
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    listener: Vec3,
    kit: u32,
    unit: Entity,
    at: Vec3,
) -> anyhow::Result<bool> {
    let extras = PlayExtras {
        source: Some(unit),
        latch: Latch::EmoteVoice,
        ..PlayExtras::default()
    };
    let opened = play_kit_ext(
        kits,
        assets,
        out,
        config,
        listener,
        KitRef::Id(kit),
        Some(at),
        SoundCategory::Sfx,
        extras,
    )?;
    if opened {
        stop_older_emote_voices(out, unit);
    }
    Ok(opened)
}

/// Route a text emote to the performer's race/sex voice. A performer whose race/sex has not
/// arrived yet stays silent. `SMSG_EMOTE` plays no sound: the one-shot path never reads
/// `EventSoundID` (`0x5e66b0` -> `0x5fcd20`), which only the state emote's `$ESD` does (`0x623a21`).
fn emote_sounds(
    mut msgs: MessageReader<EmoteMessage>,
    units: Query<(&ObjectStore, &Transform)>,
    emotes: Option<Res<EmoteSounds>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    creatures: Option<Res<Creatures>>,
    roster: Option<Res<GroupRoster>>,
    social: Res<SocialState>,
    self_guid: Res<SelfGuid>,
    attach: AttachPoints,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if msgs.is_empty() {
        return;
    }
    let (Some(emotes), Some(mut kits), Some(assets)) = (emotes, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    let no_roster = GroupRoster::default();
    for m in msgs.read() {
        let EmoteKind::Text(text_id) = m.kind else {
            continue;
        };
        // The `EmoteSounds` CVar gates the text-emote voice, read at play time; a zero never
        // fetches the kit.
        if !config.emote_sounds {
            continue;
        }
        let Some((unit, (store, transform))) =
            m.source.and_then(|e| units.get(e).ok().map(|row| (e, row)))
        else {
            continue;
        };
        let is_player = store.0.object_type() == Some(ObjectType::Player);
        if !voice_audible(
            m.guid,
            self_guid.0,
            is_player,
            roster.as_deref().unwrap_or(&no_roster),
            social.is_ignored(m.guid),
        ) {
            continue;
        }
        // The display override's race and sex before the descriptor's (`0x60c6a0`/`0x60c6c0`).
        let overridden = store
            .0
            .unit_displayid()
            .and_then(|d| u32::try_from(d).ok())
            .and_then(|d| creatures.as_deref()?.display_race_sex(d));
        let (race, sex) = match overridden {
            Some(rs) => rs,
            None => {
                let (Some(race), Some(sex)) = (store.0.unit_race(), store.0.unit_gender()) else {
                    continue;
                };
                (race, sex)
            }
        };
        let Some(kit) = emotes
            .0
            .voice(text_id, race as u32, sex as u32)
            .filter(|&k| k != 0)
        else {
            continue; // most emotes are voiceless (/wave)
        };
        let at = attach.point(unit, VOICE_ATTACH, transform.translation);
        if let Err(e) = play_emote_voice(
            &mut kits, &assets, &mut out, &config, listener, kit, unit, at,
        ) {
            warn!("emote sound (kit {kit}): {e:#}");
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, load_emote_sounds.after(AssetSet::Open))
        .add_systems(Update, emote_sounds.in_set(WorldStage::Present));
}

#[cfg(test)]
mod tests {
    use super::super::kit::occupies_emote_slot;
    use super::*;

    const ME: u64 = 1;
    const MATE: u64 = 2;
    const RAIDER: u64 = 3;
    const STRANGER: u64 = 4;

    fn roster() -> GroupRoster {
        GroupRoster {
            party: vec![MATE],
            raid: vec![ME, MATE, RAIDER],
        }
    }

    /// `0x623cc2`-`0x623cce`: class 0 and class 2 play, every other class is silent.
    #[test]
    fn only_you_and_the_party_are_heard() {
        let r = roster();
        let heard = |who, player, ignored| voice_audible(who, Some(ME), player, &r, ignored);
        assert!(heard(ME, true, false), "class 0, you");
        assert!(heard(MATE, true, false), "class 2, a party member");
        assert!(
            !heard(RAIDER, true, false),
            "class 4: the raid is not the party slots"
        );
        assert!(
            !heard(STRANGER, true, false),
            "class 4 or 6, any other player"
        );
        assert!(
            !heard(MATE, false, false),
            "a creature never classes as a player"
        );
        assert!(!voice_audible(
            MATE,
            None,
            true,
            &GroupRoster::default(),
            false
        ));
    }

    /// An ignored performer is dropped before the composer (`0x49dc36`), even you or a mate.
    #[test]
    fn an_ignored_performer_is_silent() {
        let r = roster();
        assert!(!voice_audible(MATE, Some(ME), true, &r, true));
        assert!(!voice_audible(ME, Some(ME), true, &r, true));
    }

    /// The slot holder is the unit's own emote voice, never its bark or another unit's.
    #[test]
    fn the_emote_slot_is_the_units_own() {
        let mut world = World::new();
        let (a, b) = (world.spawn_empty().id(), world.spawn_empty().id());
        assert!(occupies_emote_slot(Some(a), Latch::EmoteVoice, a));
        assert!(!occupies_emote_slot(Some(b), Latch::EmoteVoice, a));
        assert!(!occupies_emote_slot(Some(a), Latch::Voice(0), a));
        assert!(!occupies_emote_slot(None, Latch::EmoteVoice, a));
    }
}
