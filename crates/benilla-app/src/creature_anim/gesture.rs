//! The client-local gesture, which no packet announces: the chat display path picks a code from
//! the message and calls `0x60bb30(unit, code)`, which resolves it through five `Emotes.dbc` flag
//! slots and plays it through `0x5fcd20`, the player `SMSG_EMOTE` uses. Its other caller is the
//! NPC-interact path, always with code 0 (talk), so [`crate::target::click`] pushes through here.

use bevy::prelude::*;

use crate::net::GuidIndex;

use super::{EmoteAnim, PlaySeq};

/// A gesture code, the integer `0x60bb30` takes: one of five `EmoteFlags` bits, resolved by
/// scanning `Emotes.dbc` for it ([`benilla_formats::EmoteSoundCatalog::gesture`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Gesture {
    /// `EmoteFlags 0x08`: a plain say, and every NPC interaction.
    Talk,
    /// `EmoteFlags 0x10`: a say whose last byte is `?`.
    Question,
    /// `EmoteFlags 0x20`: a say whose last byte is `!`.
    Exclamation,
    /// `EmoteFlags 0x40`: any yell, whatever its text.
    Shout,
    /// `EmoteFlags 0x100`: a message that is one of the `LAUGH_WORDn` globals, whole and
    /// case-insensitively.
    Laugh,
}

impl Gesture {
    /// The client's `code`.
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

/// Gestures asked for and not yet played, `(speaker guid, code)`, from the chat display path and
/// the interact click, the two callers of `0x60bb30`.
#[derive(Resource, Default)]
pub(crate) struct GestureQueue(Vec<(u64, Gesture)>);

impl GestureQueue {
    /// A zero guid (a system line, a channel notice) has no speaker and is dropped.
    pub(crate) fn push(&mut self, guid: u64, gesture: Gesture) {
        if guid != 0 {
            self.0.push((guid, gesture));
        }
    }
}

/// The gesture a chat line plays, by wire chat type and text (`0x49d7d0` to `0x49d8ae`). Only
/// `SAY`, `YELL` and `PARTY` gesture, never a creature's `MONSTER_SAY`/`MONSTER_YELL`. A text
/// equal, case-insensitively, to a `LAUGH_WORDn` global laughs (the scan stops at the first
/// missing or empty one); otherwise `PARTY` does nothing, `YELL` shouts whatever its text, and
/// `SAY` reads its last byte only: `?` asks, `!` exclaims, anything else talks. `laugh_word(n)`
/// reads the global off the player's own FrameXML; `None` ends the list.
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

/// Drain [`GestureQueue`]: the client's `0x60bb30` and its tail call into `0x5fcd20`.
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
            continue; // the speaker is not streamed
        };
        // A slot no row carries: the client bails too (`0x60bb43`).
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

    /// Invented words: the shipped `LAUGH_WORDn` are install content, which never enters the repo.
    fn words(n: u32) -> Option<String> {
        ["snrk", "guffaw", "tehe"]
            .get(n as usize - 1)
            .map(|s| s.to_string())
    }
    fn no_words(_: u32) -> Option<String> {
        None
    }

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

    /// A creature's bark arrives as `MONSTER_SAY`/`MONSTER_YELL`, not `SAY`/`YELL`.
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

    /// The real system over the real `Emotes.dbc`; a unit in combat plays nothing.
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

        // A plain unit and one flagged in combat (`UNIT_FIELD_FLAGS` is field 46).
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
        // A speaker never streamed produces nothing.
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

    #[test]
    fn a_speakerless_line_is_dropped_at_the_queue() {
        let mut q = GestureQueue::default();
        q.push(0, Gesture::Talk);
        q.push(7, Gesture::Talk);
        assert_eq!(q.0, vec![(7, Gesture::Talk)]);
    }
}
