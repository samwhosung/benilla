//! Emote sounds (decision 0070 slice 4): `SMSG_TEXT_EMOTE` plays the performer's race/sex voice
//! kit (`EmotesTextSound`); `SMSG_EMOTE` plays the anim emote's `EventSoundID` (`Emotes.dbc`).
//! Both arrive via `net::EmoteMessage` with the performer resolved to an entity — race/sex come
//! from its descriptor store (`UNIT_FIELD_BYTES_0`), position from its transform.
//!
//! The catalog also serves the **send** side: `/wave`-style chat lines resolve their EmotesText
//! id through [`EmoteSounds::text_id`] (`crate::ui_chat`), go out as `CMSG_TEXT_EMOTE` — gated first
//! by [`EmoteSounds::text_emote`] + [`EmoteSounds::emote_flags`] (the posture-eligibility gate,
//! `0x47db40`) — and the server echo plays our own emote
//! through this same receive path — vanilla's actual loop.
//!
//! [`EmoteSounds::anim`] promotes the catalog's `Emotes.dbc` → `AnimID` column for
//! `crate::creature_anim`'s animation consumers (the `SMSG_EMOTE` one-shot and the
//! `UNIT_NPC_EMOTESTATE` looping idle) — the one DBC load serves both sound and animation.

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

    /// An `Emotes.dbc` id's `AnimID` (0/absent = none) — promoted for `crate::creature_anim`'s
    /// animation consumers so they don't load the DBC a second time.
    pub(crate) fn anim(&self, emote_id: u32) -> Option<u32> {
        self.0.anim(emote_id)
    }

    /// A text-emote's `EmotesText.dbc` `EmoteID` → its `Emotes.dbc` id (0/absent = chat-only, no
    /// anim emote — e.g. `/thank`). Promoted for `crate::ui_chat`'s send-side posture-eligibility gate.
    pub(crate) fn text_emote(&self, text_id: u32) -> Option<u32> {
        self.0.text_emote(text_id)
    }

    /// The `Emotes.dbc` id in one of the five hard-coded **gesture slots** — promoted for
    /// `crate::creature_anim::gesture`, the client-local chat/interact gesture producer.
    pub(crate) fn gesture(&self, slot: usize) -> Option<u32> {
        self.0.gesture(slot)
    }

    /// An `Emotes.dbc` id's raw `EmoteFlags` bits — promoted for `crate::ui_chat`'s send-side
    /// posture-eligibility gate (`0x47db40`).
    pub(crate) fn emote_flags(&self, emote_id: u32) -> Option<u32> {
        self.0.emote_flags(emote_id)
    }

    /// The **stand state** this emote sets, if it is a posture emote (`EmoteSpecProc == 1`) —
    /// `DoEmote`'s state branch (`0x5ef560` → `0x5ed430`). Promoted for `crate::ui_chat`: it is
    /// what makes `/sit` actually sit.
    pub(crate) fn posture_state(&self, emote_id: u32) -> Option<u32> {
        self.0.posture_state(emote_id)
    }

    /// The `$ESD` anim event's kit for a unit in this looping state emote: the row's
    /// `EventSoundID`, gated on `EmoteSpecProc == 2` — the client's `row[+0x10] == 2` test in the
    /// `$ESD` handler `0x6239f0` before it reads `row[+0x18]`. A one-shot emote id
    /// parked in the state field stays silent, exactly like the reference.
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
/// plays its event kit. A performer without race/sex in its store yet (partial snapshot) stays
/// silent rather than guessing a voice.
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
                // **`EmoteSounds` gates the received text-emote voice, and only that** — the
                // reference looks the CVar up by name at play time and, on a zero, never fetches
                // the kit at all (silence, not a muted play). The `Anim` arm below is the
                // `Emotes.dbc` `EventSoundID` one-shot, a different channel with no such gate.
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
            continue; // most emotes are voiceless (/wave); silence is correct
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
