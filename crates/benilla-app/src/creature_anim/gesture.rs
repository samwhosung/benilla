//! The **client-local gesture** — the second producer of a one-shot emote, and the one benilla was
//! missing (bug B157, decision 1469).
//!
//! Nothing on the wire announces it. The real client's chat *display* path picks a gesture code from
//! the message itself and calls `0x60bb30(unit, code)`; that resolves the code through a five-slot
//! table of `Emotes.dbc` rows and hands the emote id to `0x5fcd20` — the **same** function
//! `SMSG_EMOTE`'s handler calls, and the only one in the whole client that turns an `Emotes.dbc` id
//! into a played animation. So the gesture is not a new animation path: it is a new *producer* for
//! the one benilla already has ([`super::EmoteAnim`]). A client that only reacts to `SMSG_EMOTE`
//! plays nothing at all, which is exactly what was reported.
//!
//! The same dispatcher has a second caller — the **NPC-interact** path, always with code 0 (talk) —
//! which is why [`crate::target::click`] pushes through here too instead of writing a raw AnimID.

use bevy::prelude::*;

use crate::net::GuidIndex;

use super::{EmoteAnim, PlaySeq};

/// A gesture code — an index into `Emotes.dbc`'s five hard-coded `EmoteFlags` slots
/// (`benilla_formats::emotes::GESTURE_FLAG_BITS`), which is literally the integer the client's
/// `0x60bb30` takes. **The ids and AnimIDs behind these are content, not code**: the client scans
/// the DBC for the flag bit, and so do we ([`benilla_formats::EmoteSoundCatalog::gesture`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Gesture {
    /// `EmoteFlags 0x08` — a plain say, and every NPC interaction.
    Talk,
    /// `EmoteFlags 0x10` — a say whose last byte is `?`.
    Question,
    /// `EmoteFlags 0x20` — a say whose last byte is `!`.
    Exclamation,
    /// `EmoteFlags 0x40` — any yell, whatever its text.
    Shout,
    /// `EmoteFlags 0x100` — a message that IS one of the `LAUGH_WORDn` globals, whole and
    /// case-insensitively.
    Laugh,
}

impl Gesture {
    /// The slot index — the client's `code`.
    pub(crate) fn slot(self) -> usize {
        match self {
            Self::Talk => 0,
            Self::Question => 1,
            Self::Exclamation => 2,
            Self::Shout => 3,
            Self::Laugh => 4,
        }
    }
}

/// Gestures asked for this frame and not yet played: `(speaker guid, code)`. Filled by the chat
/// display path and the interact click — the client's two callers of `0x60bb30` — and drained by
/// [`drive_gestures`]. A queue rather than a direct write because the producers sit in the UI lane
/// and the consumer needs the world's object index; the chat bubble is queued the same way and for
/// the same reason.
#[derive(Resource, Default)]
pub(crate) struct GestureQueue(Vec<(u64, Gesture)>);

impl GestureQueue {
    /// Ask for `gesture` on the unit with `guid`. A zero guid (a system line, a channel notice) has
    /// no speaker and is dropped here rather than failing the lookup later.
    pub(crate) fn push(&mut self, guid: u64, gesture: Gesture) {
        if guid != 0 {
            self.0.push((guid, gesture));
        }
    }
}

/// Pick the gesture a chat line plays, from the **raw wire chat type** and the message text — the
/// client's selector at `0x49d7d0`–`0x49d8ae`, in its exact order.
///
/// 1. Only `SAY`, `YELL` and `PARTY` are eligible at all. Note this is the *wire* type, so
///    `MONSTER_SAY`/`MONSTER_YELL` — what a creature's ambient barks arrive as — are **not**
///    eligible: a shouting vendor does not gesture.
/// 2. The message is compared **whole and case-insensitively** against `LAUGH_WORD1`, `2`, …,
///    stopping at the first global that is missing or empty. A match is [`Gesture::Laugh`] — and
///    this is the one branch `PARTY` can reach, so a party line that IS a laugh word laughs, while `/p hello!` does nothing.
/// 3. Otherwise `PARTY` stops here.
/// 4. `YELL` is always [`Gesture::Shout`] — its text is never inspected, so a yell ending in `!`
///    still shouts rather than exclaiming.
/// 5. `SAY` looks at the **last byte only** — `?` → [`Gesture::Question`], `!` →
///    [`Gesture::Exclamation`], anything else (including an empty message) → [`Gesture::Talk`].
///    Not "ends with `!!`", not a punctuation count, no trailing-whitespace trim.
///
/// `laugh_word(n)` returns the `LAUGH_WORDn` global — `None` ends the list. It is a callback so the
/// words come off the player's own FrameXML at call time, which is where they belong: the
/// *enumeration* is the mechanism, the word list is content.
pub(crate) fn select_gesture(
    chat_type: u8,
    text: &str,
    mut laugh_word: impl FnMut(u32) -> Option<String>,
) -> Option<Gesture> {
    use benilla_protocol::messages as m;
    if !matches!(
        chat_type,
        m::CHAT_MSG_SAY | m::CHAT_MSG_YELL | m::CHAT_MSG_PARTY
    ) {
        return None;
    }
    for n in 1.. {
        match laugh_word(n) {
            Some(word) if word.is_empty() => break,
            Some(word) if word.eq_ignore_ascii_case(text) => return Some(Gesture::Laugh),
            Some(_) => continue,
            None => break,
        }
    }
    match chat_type {
        m::CHAT_MSG_YELL => Some(Gesture::Shout),
        m::CHAT_MSG_SAY => Some(match text.as_bytes().last() {
            Some(b'?') => Gesture::Question,
            Some(b'!') => Gesture::Exclamation,
            _ => Gesture::Talk,
        }),
        _ => None, // PARTY, having failed the laugh scan
    }
}

/// Drain [`GestureQueue`] into the one-shot player: resolve the speaker, resolve the code to an
/// `Emotes.dbc` id through the DBC's own flag-bit scan, gate, and write [`EmoteAnim`] — the client's
/// `0x60bb30` and its tail-call into the shared `0x5fcd20`.
pub(super) fn drive_gestures(
    mut queue: ResMut<GestureQueue>,
    mut out: MessageWriter<EmoteAnim>,
    mut play_seq: ResMut<PlaySeq>,
    index: Res<GuidIndex>,
    emotes: Option<Res<crate::sound::EmoteSounds>>,
    units: super::emote_anim::PerformerQuery,
) {
    let asked = std::mem::take(&mut queue.0);
    let Some(emotes) = emotes else { return };
    for (guid, gesture) in asked {
        let Some(&entity) = index.0.get(&guid) else {
            continue; // the speaker is not streamed to us — nothing to animate
        };
        // The slot may be empty if a patch ever ships without a row carrying the bit; the client
        // bails the same way (`0x60bb43`).
        let Some(emote_id) = emotes.gesture(gesture.slot()) else {
            continue;
        };
        let Some(anim_id) = emotes.anim(emote_id) else {
            continue;
        };
        let (store, movement, remote, engaged) =
            units.get(entity).unwrap_or((None, None, None, false));
        if !super::emote_anim::play_eligible(store, movement, remote, engaged) {
            debug!("gesture: {gesture:?} suppressed for {entity:?}");
            continue;
        }
        out.write(EmoteAnim {
            entity,
            anim_id: anim_id as u16,
            seq: play_seq.next(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages as m;

    /// A stand-in `LAUGH_WORDn` list. The words are **invented on purpose**: the shipped ones are
    /// the install's own `GlobalStrings.lua` content, which never enters this repo (docs/METHOD.md), and
    /// the mechanism under test is the enumeration, not the vocabulary. A test that passes with
    /// nonsense words is a test that proves the list is not baked into the code.
    fn words(n: u32) -> Option<String> {
        ["snrk", "guffaw", "tehe"]
            .get(n as usize - 1)
            .map(|s| s.to_string())
    }
    fn no_words(_: u32) -> Option<String> {
        None
    }

    /// The reporter's own message: `fdfdf!!` said aloud is an EXCLAMATION, because only the last
    /// byte is read. This is the case the screenshot shows.
    #[test]
    fn the_last_byte_alone_picks_the_say_gesture() {
        let say = |t: &str| select_gesture(m::CHAT_MSG_SAY, t, no_words);
        assert_eq!(say("fdfdf!!"), Some(Gesture::Exclamation));
        assert_eq!(say("what?"), Some(Gesture::Question));
        assert_eq!(say("hello there"), Some(Gesture::Talk));
        assert_eq!(
            say("! leading"),
            Some(Gesture::Talk),
            "the LAST byte, not any"
        );
        assert_eq!(say("trailing space! "), Some(Gesture::Talk), "no trim");
        assert_eq!(say(""), Some(Gesture::Talk), "an empty say still talks");
    }

    /// A yell never reads its text — punctuation and all.
    #[test]
    fn a_yell_always_shouts() {
        assert_eq!(
            select_gesture(m::CHAT_MSG_YELL, "run!", no_words),
            Some(Gesture::Shout)
        );
        assert_eq!(
            select_gesture(m::CHAT_MSG_YELL, "where?", no_words),
            Some(Gesture::Shout)
        );
    }

    /// The laugh scan is whole-string and case-insensitive, and it runs BEFORE the say/yell split —
    /// so it is the only gesture a party line can reach, and it beats a yell's shout.
    #[test]
    fn the_laugh_scan_is_whole_string_and_outranks_the_rest() {
        assert_eq!(
            select_gesture(m::CHAT_MSG_SAY, "SnRk", words),
            Some(Gesture::Laugh),
            "case-insensitive"
        );
        assert_eq!(
            select_gesture(m::CHAT_MSG_SAY, "snrk that was good", words),
            Some(Gesture::Talk),
            "whole-string equality, never a substring"
        );
        assert_eq!(
            select_gesture(m::CHAT_MSG_YELL, "guffaw", words),
            Some(Gesture::Laugh),
            "the scan runs before the yell branch"
        );
        assert_eq!(
            select_gesture(m::CHAT_MSG_PARTY, "tehe", words),
            Some(Gesture::Laugh),
            "the one gesture a party line reaches"
        );
        assert_eq!(
            select_gesture(m::CHAT_MSG_PARTY, "hello!", words),
            None,
            "a party line with no laugh word gestures nothing"
        );
    }

    /// The list ends at the first missing OR empty global, so a hole hides everything after it.
    #[test]
    fn an_empty_global_ends_the_laugh_list() {
        let holed = |n: u32| match n {
            1 => Some("snrk".to_string()),
            2 => Some(String::new()),
            3 => Some("tehe".to_string()),
            _ => None,
        };
        assert_eq!(
            select_gesture(m::CHAT_MSG_SAY, "snrk", holed),
            Some(Gesture::Laugh)
        );
        assert_eq!(
            select_gesture(m::CHAT_MSG_SAY, "tehe", holed),
            Some(Gesture::Talk),
            "the scan stopped at the empty LAUGH_WORD2"
        );
    }

    /// Everything else on the wire is ineligible — including a creature's ambient bark, which
    /// arrives as MONSTER_SAY/MONSTER_YELL, not SAY/YELL.
    #[test]
    fn only_say_yell_and_party_are_eligible() {
        for ty in [
            m::CHAT_MSG_MONSTER_SAY,
            m::CHAT_MSG_MONSTER_YELL,
            m::CHAT_MSG_WHISPER,
            m::CHAT_MSG_GUILD,
            m::CHAT_MSG_CHANNEL,
            m::CHAT_MSG_EMOTE,
            m::CHAT_MSG_SYSTEM,
        ] {
            assert_eq!(
                select_gesture(ty, "hello!", words),
                None,
                "chat type {ty:#x} must not gesture"
            );
        }
    }

    /// Drive the real system over the real `Emotes.dbc`: a queued gesture on a streamed unit must
    /// come out as an [`EmoteAnim`] carrying the shipped AnimID — and a unit **in combat** must come
    /// out with nothing (the shared player's gate).
    ///
    /// This is the test that catches the mechanism being built and never firing: it exercises the
    /// queue, the guid resolve, the DBC scan and the gate, not just the selector.
    #[test]
    fn a_queued_gesture_reaches_the_one_shot_player() {
        use crate::net::{GuidIndex, ObjectStore};
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = benilla_formats::load_emote_sound_catalog(&mut chain).expect("emote catalog");

        let mut app = App::new();
        app.init_resource::<GestureQueue>()
            .init_resource::<PlaySeq>()
            .init_resource::<GuidIndex>()
            .insert_resource(crate::sound::EmoteSounds(cat))
            .add_message::<EmoteAnim>()
            .add_systems(Update, drive_gestures);

        // A plain streamed unit, and a second one flagged IN_COMBAT (UNIT_FIELD_FLAGS = field 46).
        let speaker = app.world_mut().spawn(ObjectStore::default()).id();
        let fighter = app
            .world_mut()
            .spawn(ObjectStore(ObjectFields::from_pairs(&[(46, 0x800)])))
            .id();
        {
            let mut index = app.world_mut().resource_mut::<GuidIndex>();
            index.0.insert(11, speaker);
            index.0.insert(22, fighter);
        }
        app.world_mut()
            .resource_mut::<GestureQueue>()
            .push(11, Gesture::Exclamation);
        app.world_mut()
            .resource_mut::<GestureQueue>()
            .push(22, Gesture::Talk);
        // A speaker we have never streamed must not panic or produce anything.
        app.world_mut()
            .resource_mut::<GestureQueue>()
            .push(999, Gesture::Talk);
        app.update();

        let played: Vec<_> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<EmoteAnim>>()
            .drain()
            .collect();
        assert_eq!(played.len(), 1, "only the eligible speaker gestures");
        assert_eq!(played[0].entity, speaker);
        assert_eq!(
            played[0].anim_id, 64,
            "EXCLAMATION resolves through the shipped DBC to EmoteTalkExclamation"
        );
        assert!(
            app.world().resource::<GestureQueue>().0.is_empty(),
            "the queue drains every frame"
        );
    }

    /// A zero guid has no speaker and never reaches the lookup.
    #[test]
    fn a_speakerless_line_is_dropped_at_the_queue() {
        let mut q = GestureQueue::default();
        q.push(0, Gesture::Talk);
        q.push(7, Gesture::Talk);
        assert_eq!(q.0, vec![(7, Gesture::Talk)]);
    }
}
