//! **The message catalog's sound half** — the branch of `CGGameUI::DisplayError 0x496720` that
//! nobody had wired.
//!
//! Every displayed client message is a row of the registry at `0xb4b498`
//! ([`benilla_ui::messages`]), and the dispatcher reads two of that row's fields
//! before it ever reaches the text: `+0x08`, a **sound cue name**, and `+0x0c`, a **type tag**.
//! The test is one comparison (`0x49673d cmp [row+0xc],0x44 / je 0x496784`) and it picks between
//! two entirely different sounds:
//!
//! - **tag `== 0x44`** — play `+0x08` as a `SoundEntries` **name** through `PlaySoundByName`
//!   (`0x458030`), unless the name is the literal `"NONE"`. 30 of the 465 rows name a cue:
//!   `QUESTADDED`, `igQuestFailed`, `LEVELUP`, `TaxiNodeDiscovered`, `FRIENDJOINGAME`, …
//! - **tag `!= 0x44`** — ignore `+0x08` entirely and speak the tag as an **error-speech line id**
//!   in the player's own race and gender voice ([`super::vocal`]). 56 rows do this.
//!
//! benilla's catalog has carried both fields since 1770 and read neither; the one place a message
//! is displayed (`crate::ui_action::show_messages`) queues the record here instead, and this drains
//! it. So a message's sound now comes from the same row its text and its surface do, and cannot
//! drift from them — which is the whole argument for the table existing.
//!
//! **Where in `0x496720` this happens matters and is easy to get backwards** — the sound branch
//! runs *first*, before the row's key guard and before any text is resolved. See
//! [`MessageSounds::push`].
//!
//! **The reference's own gate ordering is preserved across the two arms and it is not symmetric.**
//! The cue arm plays like any other UI kit and is silenced by `MasterSoundEffects` at the mixer,
//! the way every other kit here is; the speech arm tests `MasterSoundEffects` *and*
//! `EnableErrorSpeech` in its own body, before touching any state (`0x458264`/`0x45827f`) — see
//! [`super::vocal::speak_line`].

use bevy::prelude::*;

use benilla_ui::messages::MessageRecord;

use crate::net::{ObjectStore, SelfPlayer};
use benilla_assets::WorldAssets;

use super::kit::{self, KitRef, SoundCategory, SoundKits};
use super::vocal::{self, VocalSpeech, NO_SPEECH_TAG};
use super::{AudioListener, SoundConfig, SoundOutput};

/// Catalog rows displayed this frame, awaiting their sound.
///
/// Pushed by `crate::ui_action::show_messages` — the one sink every displayed message passes
/// through — and drained by [`play_message_sounds`] after the UI input pass, so a refusal sounds on
/// the frame its line appears. Rows, not resolved sounds: the *reading* of `+0x08`/`+0x0c` belongs
/// here beside the players, not scattered across the windows that raise messages.
#[derive(Resource, Default)]
pub(crate) struct MessageSounds {
    records: Vec<&'static MessageRecord>,
    /// Cues by `PlaySoundByName` name with no catalog row behind them — the tutorial popup
    /// (`0x4b5390`'s `"TutorialPopup"`, 1976): played on the same 2D SFX path as a row's cue.
    cues: Vec<&'static str>,
}

impl MessageSounds {
    /// Queue one displayed message's sound.
    ///
    /// **The reference sounds a message BEFORE it decides whether to draw it**, and that ordering
    /// is read, not assumed: `0x496720`'s sound branch is at `0x49673d`, ahead of the
    /// `[record+0x00]` key guard at `0x4967bd`/`0x4967c5` and far ahead of the sink's
    /// empty-*text* guard at `0x4945b4`. So a message whose GlobalStrings entry is missing still
    /// makes its noise there, while benilla — where every raise site drops an empty line before it
    /// reaches [`show_messages`](crate::ui_action::show_messages) — would stay silent.
    ///
    /// **On 5875's data that difference has no case**: of the 86 rows that sound at all (56 voice
    /// lines + 30 cues), every single one resolves a non-empty string in the shipped
    /// `GlobalStrings.lua` — measured, not assumed. The rows with no text are the
    /// silent ones, `ERR_CANT_BE_DISENCHANTED` among them, so nothing is lost by hanging the sound
    /// off the display. It is written down because the *ordering* is the surprising part, and a
    /// future locale or a patched table could give it a tenant.
    pub(crate) fn push(&mut self, record: &'static MessageRecord) {
        self.records.push(record);
    }

    /// Queue a named cue with no message behind it.
    pub(crate) fn push_cue(&mut self, name: &'static str) {
        self.cues.push(name);
    }

    /// What is queued — for the tests that drive the producer
    /// (`crate::ui_action::show_messages`) and read back what the drain would be handed.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> &[&'static MessageRecord] {
        &self.records
    }
}

/// Drain [`MessageSounds`]: cue by name, or speech by line (module doc).
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
    // Drained even when there is nothing to play it with (headless, no client data), so the queue
    // can never grow unbounded — `super::ui`'s posture exactly.
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    // The reference reads the player's sex per play (`0x49676f` → `0x5ed5b0`), not at table-build
    // time; a descriptor that has not arrived yet means no voice, not a guessed one.
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
                continue; // the 435 silent rows, and the `"NONE"` the generator already folded in
            };
            if let Err(e) = kit::play_kit(
                &mut kits,
                &assets,
                &mut out,
                &config,
                listener.pos,
                KitRef::Name(name),
                None, // 2D — `0x458030`'s plays are non-positional UI kits
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
        // After the UI input pass, beside `super::ui`'s own drain and for the same reason: the
        // message feeds run before it, so a refusal raised this frame sounds this frame.
        play_message_sounds.after(crate::ui_script::UiInput),
    );
}

#[cfg(test)]
mod tests {
    use benilla_ui::messages::{by_key, MsgKind};

    use super::NO_SPEECH_TAG;

    /// The two arms are disjoint over the real table, which is what lets the drain branch on one
    /// comparison: **no row both names a cue and carries a speech line**. (30 rows name a cue and
    /// every one of them is tagged `0x44`; 56 carry a line and none names a cue.)
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

    /// The lines a player hears most, pinned to the tags the shipped `VocalUISounds.dbc` voices —
    /// the join this whole module exists to make, asserted on both tables at once so a
    /// regenerated catalog that moved a tag fails here rather than going quiet in the world.
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
            // Not every voiced line is a red one: the taxi's refusal is the YELLOW info line and
            // still speaks, which is the reason the drain never reads `kind`.
            ("ERR_TAXINOTENOUGHMONEY", 0x36, MsgKind::Info),
            // …nor is every one a UIErrorsFrame line at all — this one is a chat row that speaks.
            ("ERR_ALREADY_IN_GROUP_S", 0x14, MsgKind::Chat),
        ] {
            let r = by_key(key).unwrap_or_else(|| panic!("{key} is not a catalog row"));
            assert_eq!(r.type_tag, tag, "{key}");
            assert_eq!(r.kind, kind, "{key}");
        }
    }
}
