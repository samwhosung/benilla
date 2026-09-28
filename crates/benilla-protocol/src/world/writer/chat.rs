//! The chat sends. A line speaks the language `SendChatMessage` named, else
//! [`WorldWriter::chat_language`]: vmangos drops `Universal` outside AFK and DND
//! (`ChatHandler.cpp:105`) and any tongue the character does not know (`:175`).

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Send a `/say` line; dot-commands go out this way, parsed after the language gate.
    pub fn send_chat(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(messages::CHAT_TYPE_SAY, self.chat_language, message),
        )
    }

    /// Send a chat line of any `ChatMsg` type, as the reference's generic builder writes it
    /// (`0x49f6c3`-`0x49f72a`). `language` is the `Languages.dbc` id `SendChatMessage` resolved;
    /// `None` is the character's own tongue, [`WorldWriter::chat_language`].
    pub fn send_message_chat(
        &mut self,
        chat_type: u32,
        language: Option<u32>,
        target: Option<&str>,
        message: &str,
    ) -> Result<()> {
        let body = message_chat_body(self.chat_language, chat_type, language, target, message);
        self.send(opcode::CMSG_MESSAGECHAT, &body)
    }

    /// Send an addon message (`SendAddonMessage`): `text` is the caller's composed `prefix` TAB
    /// `message`, on the party, raid, guild or battleground lane. Its language,
    /// [`messages::LANGUAGE_ADDON`], is the only mark of addon data (`0x49f920`); vmangos gates it
    /// on `AddonChannel` and skips the language gate, flood control and sanitizing.
    pub fn send_addon_message(&mut self, chat_type: u32, text: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(chat_type, messages::LANGUAGE_ADDON, text),
        )
    }

    /// Tell the server we ignore `guid`; that player gets a `CHAT_MSG_IGNORED` notice.
    pub fn chat_ignored(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_CHAT_IGNORED, &messages::full_guid(guid))
    }

    /// Ask our played time (`CMSG_PLAYED_TIME`), answered by `SMSG_PLAYED_TIME`.
    pub fn played_time(&mut self) -> Result<()> {
        self.send(opcode::CMSG_PLAYED_TIME, &messages::played_time())
    }

    /// Roll `/random`; the server checks `min <= max <= 10000` and answers on the same opcode.
    pub fn random_roll(&mut self, min: u32, max: u32) -> Result<()> {
        self.send(opcode::MSG_RANDOM_ROLL, &messages::random_roll(min, max))
    }

    /// Perform a text emote (EmotesText id, target or 0), echoed to everyone in range, us included.
    pub fn text_emote(&mut self, text_id: u32, target: u64) -> Result<()> {
        self.send(
            opcode::CMSG_TEXT_EMOTE,
            &messages::text_emote(text_id, target),
        )
    }
}

/// The `CMSG_MESSAGECHAT` body: type, language (`0x49f6f9`), the target cstring only for a whisper
/// or a channel (`0x49f701`-`0x49f712`, types `6` and `0xe`), then the text. `speaker` is the
/// language a send that names none carries.
fn message_chat_body(
    speaker: u32,
    chat_type: u32,
    language: Option<u32>,
    target: Option<&str>,
    message: &str,
) -> Vec<u8> {
    let target = matches!(
        chat_type,
        messages::CHAT_TYPE_WHISPER | messages::CHAT_TYPE_CHANNEL
    )
    .then(|| target.unwrap_or_default());
    messages::messagechat_kind(chat_type, language.unwrap_or(speaker), target, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_language_lands_in_the_body() {
        // A Night Elf's `SendChatMessage("hi", "SAY", "Darnassian")`: type 0, language 2.
        assert_eq!(
            message_chat_body(7, messages::CHAT_TYPE_SAY, Some(2), None, "hi"),
            [0, 0, 0, 0, 2, 0, 0, 0, b'h', b'i', 0]
        );
        // No language named: the speaker's Common (7).
        assert_eq!(
            message_chat_body(7, messages::CHAT_TYPE_YELL, None, None, "hi"),
            [5, 0, 0, 0, 7, 0, 0, 0, b'h', b'i', 0]
        );
        // A Tauren's Taur-ahe (3) whisper: the name before the text.
        assert_eq!(
            message_chat_body(1, messages::CHAT_TYPE_WHISPER, Some(3), Some("Bo"), "yo"),
            [6, 0, 0, 0, 3, 0, 0, 0, b'B', b'o', 0, b'y', b'o', 0]
        );
    }

    #[test]
    fn only_a_whisper_or_a_channel_carries_a_target() {
        assert_eq!(
            message_chat_body(1, messages::CHAT_TYPE_PARTY, None, Some("Bo"), "x"),
            [1, 0, 0, 0, 1, 0, 0, 0, b'x', 0]
        );
        assert_eq!(
            message_chat_body(1, messages::CHAT_TYPE_CHANNEL, Some(14), None, "x"),
            [0xe, 0, 0, 0, 14, 0, 0, 0, 0, b'x', 0],
            "a channel with no name still writes the empty cstring"
        );
    }
}
