//! Emote sounds: `SMSG_TEXT_EMOTE` plays the performer's race/sex voice kit
//! (`EmotesTextSound`), `SMSG_EMOTE` plays the anim emote's `EventSoundID` (`Emotes.dbc`). Our
//! own `/wave` goes out as `CMSG_TEXT_EMOTE` and plays through the server echo, as in 1.12. The
//! catalog also serves `crate::ui_chat` and `crate::creature_anim`.

use bevy::prelude::*;

use benilla_formats::EmoteSoundCatalog;

use crate::net::{EmoteKind, EmoteMessage, ObjectStore};
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::kit::{play_kit, KitRef, SoundCategory, SoundKits};
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

/// Route the bridged emotes: a text emote plays the performer's race/sex voice; an anim emote
/// plays its event kit. A performer whose race/sex has not arrived yet stays silent.
fn emote_sounds(
    mut msgs: MessageReader<EmoteMessage>,
    units: Query<(&ObjectStore, &Transform)>,
    emotes: Option<Res<EmoteSounds>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
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
    for m in msgs.read() {
        let Some((store, transform)) = m.source.and_then(|e| units.get(e).ok()) else {
            continue;
        };
        let kit = match m.kind {
            EmoteKind::Text(text_id) => {
                // The `EmoteSounds` CVar gates only the text-emote voice, read at play time; a
                // zero never fetches the kit. The `Anim` one-shot has no such gate.
                if !config.emote_sounds {
                    continue;
                }
                let (Some(race), Some(sex)) = (store.0.unit_race(), store.0.unit_gender()) else {
                    continue;
                };
                emotes.0.voice(text_id, race as u32, sex as u32)
            }
            EmoteKind::Anim(emote_id) => emotes.0.event_sound(emote_id),
        };
        let Some(kit) = kit.filter(|&k| k != 0) else {
            continue; // most emotes are voiceless (/wave)
        };
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
            warn!("emote sound (kit {kit}): {e:#}");
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, load_emote_sounds.after(AssetSet::Open))
        .add_systems(Update, emote_sounds.in_set(WorldStage::Present));
}
