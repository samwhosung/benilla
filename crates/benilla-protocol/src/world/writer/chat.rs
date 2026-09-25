//! The chat sends. Chat speaks [`WorldWriter::chat_language`]: vmangos drops `Universal` from
//! clients and any tongue the character does not know.

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

    /// Send a `/yell` line (`CHAT_MSG_YELL`, `SharedDefines.h:1199`).
    pub fn send_yell(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(messages::CHAT_TYPE_YELL, self.chat_language, message),
        )
    }

    /// Send a custom `/emote` line (`CHAT_MSG_EMOTE`), shown verbatim as `"PlayerName <text>"`.
    pub fn send_emote_chat(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(messages::CHAT_TYPE_EMOTE, self.chat_language, message),
        )
    }

    /// Send a `/whisper` line; the body carries the name before the message (`Chat.cpp:3-12`).
    pub fn send_whisper(&mut self, target: &str, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat_whisper(self.chat_language, target, message),
        )
    }

    /// Send a `/p` party line, dropped silently when ungrouped (`ChatHandler.cpp:472-493`).
    pub fn send_party(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(messages::CHAT_TYPE_PARTY, self.chat_language, message),
        )
    }

    /// Send a `/ra` raid line (`CHAT_MSG_RAID`); needs a raid (`ChatHandler.cpp:514-536`).
    pub fn send_raid(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(messages::CHAT_TYPE_RAID, self.chat_language, message),
        )
    }

    /// Send a `/g` guild line (`CHAT_MSG_GUILD`); needs a guild (`ChatHandler.cpp:494-503`).
    pub fn send_guild(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(messages::CHAT_TYPE_GUILD, self.chat_language, message),
        )
    }

    /// Send a `/o` officer line (`CHAT_MSG_OFFICER`); needs a guild (`ChatHandler.cpp:504-513`).
    pub fn send_officer(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(messages::CHAT_TYPE_OFFICER, self.chat_language, message),
        )
    }

    /// Send a `/rl` raid-leader line, leader only (`ChatHandler.cpp:538-559`).
    pub fn send_raid_leader(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(messages::CHAT_TYPE_RAID_LEADER, self.chat_language, message),
        )
    }

    /// Send a `/rw` raid warning, leader or assistant only (`ChatHandler.cpp:561-576`).
    pub fn send_raid_warning(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(
                messages::CHAT_TYPE_RAID_WARNING,
                self.chat_language,
                message,
            ),
        )
    }

    /// Send a battleground line; needs a battleground group (`ChatHandler.cpp:579-593`).
    pub fn send_battleground(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(
                messages::CHAT_TYPE_BATTLEGROUND,
                self.chat_language,
                message,
            ),
        )
    }

    /// Send a battleground-leader line, leader only (`ChatHandler.cpp:595-609`).
    pub fn send_battleground_leader(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(
                messages::CHAT_TYPE_BATTLEGROUND_LEADER,
                self.chat_language,
                message,
            ),
        )
    }

    /// Toggle AFK, `message` the auto-reply if any; it clears DND (`ChatHandler.cpp:611-630`).
    pub fn send_afk(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(messages::CHAT_TYPE_AFK, self.chat_language, message),
        )
    }

    /// Toggle DND (`CHAT_MSG_DND`), which clears AFK (`ChatHandler.cpp:632-648`).
    pub fn send_dnd(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(messages::CHAT_TYPE_DND, self.chat_language, message),
        )
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

    /// Send a channel line by name, dropped unless we are on it (`ChatHandler.cpp:255-327`).
    pub fn send_channel(&mut self, channel: &str, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat_channel(self.chat_language, channel, message),
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
