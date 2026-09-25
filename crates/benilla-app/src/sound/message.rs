//! The message catalog's sound half: the sound branch of `CGGameUI::DisplayError` (`0x496720`).
//!
//! Each displayed message is a row of the registry at `0xb4b498` ([`benilla_ui::messages`]), and
//! one comparison on its type tag `+0x0c` picks the sound (`0x49673d cmp [row+0xc],0x44`):
//!
//! - `== 0x44`: play `+0x08` as a `SoundEntries` name through `PlaySoundByName` (`0x458030`),
//!   unless it is `"NONE"`.
//! - `!= 0x44`: speak the tag as an error-speech line in the player's race and gender voice
//!   ([`super::vocal`]).
//!
//! The two arms gate differently: a cue is silenced by `MasterSoundEffects` at the mixer, while
//! speech tests `MasterSoundEffects` and `EnableErrorSpeech` in its own body
//! (`0x458264`/`0x45827f`).

use bevy::prelude::*;

use benilla_ui::messages::MessageRecord;

use crate::net::{ObjectStore, SelfPlayer};
use benilla_assets::WorldAssets;

use super::kit::{self, KitRef, SoundCategory, SoundKits};
use super::vocal::{self, VocalSpeech, NO_SPEECH_TAG};
use super::{AudioListener, SoundConfig, SoundOutput};

/// Catalog rows displayed this frame, awaiting their sound. Pushed by
/// `crate::ui_action::show_messages`, the sink every displayed message passes through.
#[derive(Resource, Default)]
pub(crate) struct MessageSounds {
    records: Vec<&'static MessageRecord>,
    /// `PlaySoundByName` cues with no catalog row, such as the tutorial popup's
    /// `"TutorialPopup"` (`0x4b5390`), played on the same 2D SFX path.
    cues: Vec<&'static str>,
}

impl MessageSounds {
    /// Queue one displayed message's sound.
    ///
    /// The reference sounds a message before deciding to draw it (`0x49673d`, ahead of the key
    /// guard at `0x4967bd`/`0x4967c5` and the empty-text guard at `0x4945b4`), while benilla drops
    /// an empty line before it gets here. No sounding row in the shipped `GlobalStrings.lua` has
    /// empty text, so the two agree on 1.12.1's data.
    pub(crate) fn push(&mut self, record: &'static MessageRecord) {
        self.records.push(record);
    }

    /// Queue a named cue with no message behind it.
    pub(crate) fn push_cue(&mut self, name: &'static str) {
        self.cues.push(name);
    }

    /// What is queued, for the producer's tests.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> &[&'static MessageRecord] {
        &self.records
    }

    /// The named cues queued, for the producer's tests.
    #[cfg(test)]
    pub(crate) fn queued_cues(&self) -> &[&'static str] {
        &self.cues
    }
}

/// Drain [`MessageSounds`]: a cue by name, or speech by line.
fn play_message_sounds(
    mut queue: ResMut<MessageSounds>,
    mut speech: ResMut<VocalSpeech>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if queue.records.is_empty() && queue.cues.is_empty() {
        return;
    }
    let records = std::mem::take(&mut queue.records);
    let cues = std::mem::take(&mut queue.cues);
    // Drained even with nothing to play it on, so the queue cannot grow unbounded.
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    // The reference reads the player's sex per play (`0x49676f` → `0x5ed5b0`); none yet means
    // no voice.
    let sex = self_q.iter().next().and_then(|s| s.0.unit_gender());
    for name in cues {
        if let Err(e) = kit::play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener.pos,
            KitRef::Name(name),
            None,
            SoundCategory::Sfx,
        ) {
            warn!("sound(message): cue {name:?}: {e:#}");
        }
    }
    for record in records {
        if record.type_tag == NO_SPEECH_TAG {
            let Some(name) = record.sound else {
                continue; // a silent row; the generator folds `"NONE"` into `None`
            };
            if let Err(e) = kit::play_kit(
                &mut kits,
                &assets,
                &mut out,
                &config,
                listener.pos,
                KitRef::Name(name),
                None, // 2D: `0x458030` plays non-positional UI kits
                SoundCategory::Sfx,
            ) {
                warn!("sound(message): cue {name:?} for {}: {e:#}", record.key);
            }
            continue;
        }
        let Some(sex) = sex else { continue };
        vocal::speak_line(
            record.type_tag,
            &mut speech,
            u32::from(sex),
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener.pos,
        );
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<MessageSounds>().add_systems(
        Update,
        // After the UI input pass, so a refusal raised this frame sounds this frame.
        play_message_sounds.after(crate::ui_script::UiInput),
    );
}

#[cfg(test)]
mod tests {
    use benilla_ui::messages::{by_key, MsgKind};

    use super::NO_SPEECH_TAG;

    /// No row both names a cue and carries a speech line, so the drain branches on one test.
    #[test]
    fn a_row_is_either_a_cue_or_a_voice_line_never_both() {
        let mut cues = 0;
        let mut voices = 0;
        for r in benilla_ui::messages::CATALOG {
            if r.type_tag == NO_SPEECH_TAG {
                cues += usize::from(r.sound.is_some());
            } else {
                voices += 1;
                assert!(
                    r.sound.is_none(),
                    "{} is both a cue and a voice line",
                    r.key
                );
            }
        }
        assert_eq!((cues, voices), (30, 56));
    }

    /// The most-heard refusals keep the tags the shipped `VocalUISounds.dbc` voices.
    #[test]
    fn the_everyday_refusals_carry_their_voice_lines() {
        for (key, tag, kind) in [
            ("ERR_OUT_OF_MANA", 0x0f, MsgKind::Error),
            ("ERR_OUT_OF_RAGE", 0x3f, MsgKind::Error),
            ("ERR_OUT_OF_ENERGY", 0x40, MsgKind::Error),
            ("ERR_SPELL_OUT_OF_RANGE", 0x2e, MsgKind::Error),
            ("ERR_SPELL_COOLDOWN", 0x0c, MsgKind::Error),
            ("ERR_ABILITY_COOLDOWN", 0x32, MsgKind::Error),
            ("ERR_INV_FULL", 0x00, MsgKind::Error),
            ("ERR_BAG_FULL", 0x1d, MsgKind::Error),
            ("ERR_NOT_ENOUGH_MONEY", 0x28, MsgKind::Error),
            ("ERR_ITEM_LOCKED", 0x3d, MsgKind::Error),
            ("ERR_GENERIC_NO_TARGET", 0x2d, MsgKind::Error),
            // An info line still speaks, so the drain never reads `kind`.
            ("ERR_TAXINOTENOUGHMONEY", 0x36, MsgKind::Info),
            // A chat row speaks too.
            ("ERR_ALREADY_IN_GROUP_S", 0x14, MsgKind::Chat),
        ] {
            let r = by_key(key).unwrap_or_else(|| panic!("{key} is not a catalog row"));
            assert_eq!(r.type_tag, tag, "{key}");
            assert_eq!(r.kind, kind, "{key}");
        }
    }
}
