//! [`ChatEvent`]: the reference's `CHAT_MSG_*` event and its `arg1..arg10`, typed. Every source,
//! the wire and the client-composed lines alike, produces one, and [`super::frames::route`] fires
//! it at the VM, where the stock `ChatFrame_OnEvent` prints it.

use benilla_ui::script::ScriptValue;

/// The chat-event kinds: the `ChatTypeInfo` keys benilla produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ChatEventKind {
    Say,
    Party,
    Raid,
    Guild,
    Officer,
    Yell,
    Whisper,
    WhisperInform,
    Emote,
    TextEmote,
    System,
    MonsterSay,
    MonsterYell,
    MonsterEmote,
    MonsterWhisper,
    Channel,
    ChannelJoin,
    ChannelLeave,
    ChannelNotice,
    ChannelNoticeUser,
    ChannelList,
    Afk,
    Dnd,
    Ignored,
    Skill,
    Loot,
    Money,
    /// The client-composed XP line (`SMSG_LOG_XPGAIN`).
    CombatXpGain,
    /// The client-composed honor line (`SMSG_PVP_CREDIT`), built once the victim's name resolves.
    CombatHonorGain,
    RaidLeader,
    RaidWarning,
    RaidBossEmote,
    /// `CHAT_MSG_FILTERED` (`0x5B`), the server's "your message was filtered" notice; `arg2` is the
    /// addressee the stock frame formats `CHAT_FILTERED` over (`ChatFrame.lua:1406`).
    Filtered,
    Battleground,
    BattlegroundLeader,
    BgSystemNeutral,
    BgSystemAlliance,
    BgSystemHorde,
    // ── the combat log ───────────────────────────────────────────────────────────────
    // Every kind below has a producer.
    /// `0x19`, the reference's catch-all combat line: `DURABILITYDAMAGE_DEATH`, `SPELLDISMISSPET*`,
    /// `PET_LOYALTY_*`, `SPELLHAPPINESSDRAIN*` and the "string not found" warning.
    CombatMiscInfo,
    CombatSelfHits,
    CombatSelfMisses,
    CombatPetHits,
    CombatPetMisses,
    CombatPartyHits,
    CombatPartyMisses,
    CombatFriendlyPlayerHits,
    CombatFriendlyPlayerMisses,
    CombatHostilePlayerHits,
    CombatHostilePlayerMisses,
    CombatCreatureVsSelfHits,
    CombatCreatureVsSelfMisses,
    CombatCreatureVsPartyHits,
    CombatCreatureVsPartyMisses,
    CombatCreatureVsCreatureHits,
    CombatCreatureVsCreatureMisses,
    /// `0x2b`, a death whose victim classifies 0..5 (you, yours, a friendly player or their pet).
    CombatFriendlyDeath,
    /// `0x2c`, every other death: a hostile player, a creature, an unresolvable victim.
    CombatHostileDeath,
    SpellSelfDamage,
    SpellSelfBuff,
    SpellPetDamage,
    SpellPetBuff,
    SpellPartyDamage,
    SpellPartyBuff,
    SpellFriendlyPlayerDamage,
    SpellFriendlyPlayerBuff,
    SpellHostilePlayerDamage,
    SpellHostilePlayerBuff,
    SpellCreatureVsSelfDamage,
    SpellCreatureVsSelfBuff,
    SpellCreatureVsPartyDamage,
    SpellCreatureVsPartyBuff,
    SpellCreatureVsCreatureDamage,
    SpellCreatureVsCreatureBuff,
    /// `0x3e`, the item a cast produced: `TRADESKILL_LOG_*` and `FEEDPET_LOG_*`.
    SpellTradeskills,
    SpellDamageShieldsOnSelf,
    SpellDamageShieldsOnOthers,
    /// `0x41`, an aura leaving you or your pet (`AURAREMOVED*`).
    SpellAuraGoneSelf,
    /// `0x42`, an aura leaving a party member or their pet.
    SpellAuraGoneParty,
    /// `0x43`, an aura leaving anyone else.
    SpellAuraGoneOther,
    /// `0x44`, an enchant landing on or fading from an item (`ITEMENCHANTMENT*`).
    SpellItemEnchantments,
    /// `0x45`, an aura dispelled or stolen (`AURADISPEL*`, `AURASTOLEN*`).
    SpellBreakAura,
    SpellPeriodicSelfDamage,
    SpellPeriodicSelfBuffs,
    SpellPeriodicPartyDamage,
    SpellPeriodicPartyBuffs,
    SpellPeriodicFriendlyPlayerDamage,
    SpellPeriodicFriendlyPlayerBuffs,
    SpellPeriodicHostilePlayerDamage,
    SpellPeriodicHostilePlayerBuffs,
    SpellPeriodicCreatureDamage,
    SpellPeriodicCreatureBuffs,
    /// `0x50`, your own cast that failed (`SPELLFAIL{CAST,PERFORM}SELF`).
    SpellFailedLocalPlayer,
    /// `0x55`, a reputation delta (`FACTION_STANDING_INCREASED`/`_DECREASED`).
    CombatFactionChange,
}

impl ChatEventKind {
    /// The combat log's own kinds. The reference tests the `COMBAT_`/`SPELL_` prefix
    /// (`ChatFrame.lua:1397-1399`); `COMBAT_XP_GAIN` and `COMBAT_HONOR_GAIN`, composed apart from
    /// the combat log, are outside this set.
    pub(crate) fn is_combat_log(self) -> bool {
        use ChatEventKind as K;
        matches!(
            self,
            K::CombatMiscInfo
                | K::CombatSelfHits
                | K::CombatSelfMisses
                | K::CombatPetHits
                | K::CombatPetMisses
                | K::CombatPartyHits
                | K::CombatPartyMisses
                | K::CombatFriendlyPlayerHits
                | K::CombatFriendlyPlayerMisses
                | K::CombatHostilePlayerHits
                | K::CombatHostilePlayerMisses
                | K::CombatCreatureVsSelfHits
                | K::CombatCreatureVsSelfMisses
                | K::CombatCreatureVsPartyHits
                | K::CombatCreatureVsPartyMisses
                | K::CombatCreatureVsCreatureHits
                | K::CombatCreatureVsCreatureMisses
                | K::CombatFriendlyDeath
                | K::CombatHostileDeath
                | K::SpellSelfDamage
                | K::SpellSelfBuff
                | K::SpellPetDamage
                | K::SpellPetBuff
                | K::SpellPartyDamage
                | K::SpellPartyBuff
                | K::SpellFriendlyPlayerDamage
                | K::SpellFriendlyPlayerBuff
                | K::SpellHostilePlayerDamage
                | K::SpellHostilePlayerBuff
                | K::SpellCreatureVsSelfDamage
                | K::SpellCreatureVsSelfBuff
                | K::SpellCreatureVsPartyDamage
                | K::SpellCreatureVsPartyBuff
                | K::SpellCreatureVsCreatureDamage
                | K::SpellCreatureVsCreatureBuff
                | K::SpellTradeskills
                | K::SpellDamageShieldsOnSelf
                | K::SpellDamageShieldsOnOthers
                | K::SpellAuraGoneSelf
                | K::SpellAuraGoneParty
                | K::SpellAuraGoneOther
                | K::SpellItemEnchantments
                | K::SpellBreakAura
                | K::SpellPeriodicSelfDamage
                | K::SpellPeriodicSelfBuffs
                | K::SpellPeriodicPartyDamage
                | K::SpellPeriodicPartyBuffs
                | K::SpellPeriodicFriendlyPlayerDamage
                | K::SpellPeriodicFriendlyPlayerBuffs
                | K::SpellPeriodicHostilePlayerDamage
                | K::SpellPeriodicHostilePlayerBuffs
                | K::SpellPeriodicCreatureDamage
                | K::SpellPeriodicCreatureBuffs
                | K::SpellFailedLocalPlayer
                | K::CombatFactionChange
        )
    }

    /// Every kind, for the exhaustive sweeps; `ui_chat::tests::every_kind_is_in_all` fails when one
    /// is missing.
    #[cfg(test)]
    pub(crate) const ALL: &'static [ChatEventKind] = {
        use ChatEventKind as K;
        &[
            K::Say,
            K::Party,
            K::Raid,
            K::Guild,
            K::Officer,
            K::Yell,
            K::Whisper,
            K::WhisperInform,
            K::Emote,
            K::TextEmote,
            K::System,
            K::MonsterSay,
            K::MonsterYell,
            K::MonsterEmote,
            K::MonsterWhisper,
            K::Channel,
            K::ChannelJoin,
            K::ChannelLeave,
            K::ChannelNotice,
            K::ChannelNoticeUser,
            K::ChannelList,
            K::Afk,
            K::Dnd,
            K::Ignored,
            K::Skill,
            K::Loot,
            K::Money,
            K::CombatXpGain,
            K::CombatHonorGain,
            K::RaidLeader,
            K::RaidWarning,
            K::RaidBossEmote,
            K::Filtered,
            K::Battleground,
            K::BattlegroundLeader,
            K::BgSystemNeutral,
            K::BgSystemAlliance,
            K::BgSystemHorde,
            K::CombatMiscInfo,
            K::CombatSelfHits,
            K::CombatSelfMisses,
            K::CombatPetHits,
            K::CombatPetMisses,
            K::CombatPartyHits,
            K::CombatPartyMisses,
            K::CombatFriendlyPlayerHits,
            K::CombatFriendlyPlayerMisses,
            K::CombatHostilePlayerHits,
            K::CombatHostilePlayerMisses,
            K::CombatCreatureVsSelfHits,
            K::CombatCreatureVsSelfMisses,
            K::CombatCreatureVsPartyHits,
            K::CombatCreatureVsPartyMisses,
            K::CombatCreatureVsCreatureHits,
            K::CombatCreatureVsCreatureMisses,
            K::CombatFriendlyDeath,
            K::CombatHostileDeath,
            K::SpellSelfDamage,
            K::SpellSelfBuff,
            K::SpellPetDamage,
            K::SpellPetBuff,
            K::SpellPartyDamage,
            K::SpellPartyBuff,
            K::SpellFriendlyPlayerDamage,
            K::SpellFriendlyPlayerBuff,
            K::SpellHostilePlayerDamage,
            K::SpellHostilePlayerBuff,
            K::SpellCreatureVsSelfDamage,
            K::SpellCreatureVsSelfBuff,
            K::SpellCreatureVsPartyDamage,
            K::SpellCreatureVsPartyBuff,
            K::SpellCreatureVsCreatureDamage,
            K::SpellCreatureVsCreatureBuff,
            K::SpellTradeskills,
            K::SpellDamageShieldsOnSelf,
            K::SpellDamageShieldsOnOthers,
            K::SpellAuraGoneSelf,
            K::SpellAuraGoneParty,
            K::SpellAuraGoneOther,
            K::SpellItemEnchantments,
            K::SpellBreakAura,
            K::SpellPeriodicSelfDamage,
            K::SpellPeriodicSelfBuffs,
            K::SpellPeriodicPartyDamage,
            K::SpellPeriodicPartyBuffs,
            K::SpellPeriodicFriendlyPlayerDamage,
            K::SpellPeriodicFriendlyPlayerBuffs,
            K::SpellPeriodicHostilePlayerDamage,
            K::SpellPeriodicHostilePlayerBuffs,
            K::SpellPeriodicCreatureDamage,
            K::SpellPeriodicCreatureBuffs,
            K::SpellFailedLocalPlayer,
            K::CombatFactionChange,
        ]
    };
}

/// One `CHAT_MSG_*` fire, typed. The fire helper `0x49b0b0` signals it (`0x703f50`) with the
/// format `"%s%s%s%s%s%s%d%d%s%d"` (`.rdata 0x844608`): arg7, arg8 and arg10 are numbers, the
/// rest strings, never nil, as `ChatFrame_OnEvent` compares `arg7 > 0` bare.
///
/// | arg | field | what `ChatFrame_OnEvent` reads |
/// |---|---|---|
/// | 1 | `text`, `notice` | the body, or a channel notice's `CHAT_<token>_NOTICE` token |
/// | 2 | `sender` | the speaker, or a notice's affected player |
/// | 3 | `language` | a language name; empty prints no header |
/// | 4 | `channel` | the display form, "N. Name - Zone" when numbered |
/// | 5 | `target` | a two-name notice's second name, "X kicked by Y" |
/// | 6 | `flag` | "AFK", "DND" or "GM", as `CHAT_FLAG_<flag>` |
/// | 7 | `zone_channel_id` | the ChannelID, 0 for a custom channel (`ChatFrame.lua:1379`) |
/// | 8 | `channel_number` | the local slot, `ChatTypeInfo["CHANNEL"..arg8]` |
/// | 9 | `channel_base` | the channel name without its number |
/// | 10 | none | the split index, 0 from vmangos (`Chat/Channel.cpp:823-827`) |
///
/// arg4, arg7, arg8 and arg9 are one record ([`super::edit::ChannelState::stamp_channel`]). arg1
/// is the garbled text: the reference fills one buffer (`SStrCopy` at `0x49a9f0`, the garble
/// `0x49b560` at `0x49aa7c`) that the line, arg1 and the bubble share ([`super::language`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ChatEvent {
    pub kind: Option<ChatEventKind>,
    pub text: String,
    pub sender: String,
    pub language: String,
    pub channel: String,
    pub target: String,
    pub flag: String,
    /// arg7, the ChannelID behind a zone channel (1 General, 2 Trade, 22 LocalDefense), else 0.
    pub zone_channel_id: u32,
    pub channel_number: u32,
    pub channel_base: String,
    pub notice: String,
    /// Our slot's state for this channel as the notice arrived (`None`: no slot), set before
    /// [`super::feed::deliver`] changes it: the reference's notice arms read `slot+0x9c` for the
    /// token (`0x49c0c2`) before they write it (`0x49bb20`).
    pub slot_state: Option<super::edit::SlotState>,
}

impl ChatEvent {
    /// A bare kind+text event (SYSTEM lines, loot receive lines, TEXT_EMOTE sentences).
    pub(crate) fn text_only(kind: ChatEventKind, text: String) -> Self {
        ChatEvent {
            kind: Some(kind),
            text,
            ..Default::default()
        }
    }

    /// The `SMSG_CHANNEL_NOTIFY` byte behind a channel notice, which `notice` holds in decimal.
    pub(crate) fn notice_byte(&self) -> Option<u8> {
        self.notice.parse().ok()
    }

    /// This event's `arg1..arg10` in the reference's order and types; ten values, never nil.
    pub(crate) fn script_args(&self) -> Vec<ScriptValue> {
        // arg1 is the token for a channel notice, else the body.
        let arg1 = match (self.kind, self.notice_byte()) {
            (Some(ChatEventKind::ChannelNotice | ChatEventKind::ChannelNoticeUser), Some(byte)) => {
                notice_token(byte, self.slot_state)
                    .unwrap_or_default()
                    .to_string()
            }
            _ => self.text.clone(),
        };
        vec![
            ScriptValue::Str(arg1),
            ScriptValue::Str(self.sender.clone()),
            ScriptValue::Str(self.language.clone()),
            ScriptValue::Str(self.channel.clone()),
            ScriptValue::Str(self.target.clone()),
            ScriptValue::Str(self.flag.clone()),
            ScriptValue::Int(i64::from(self.zone_channel_id)),
            ScriptValue::Int(i64::from(self.channel_number)),
            ScriptValue::Str(self.channel_base.clone()),
            // arg10: always 0, see the struct doc.
            ScriptValue::Int(0),
        ]
    }
}

/// The reference's event name for a kind, `CHAT_MSG_` and its `ChatTypeInfo` key, which
/// `ChatFrame_OnEvent` recovers with `strsub(event, 10)`.
pub(crate) fn event_name(kind: ChatEventKind) -> &'static str {
    use ChatEventKind as K;
    match kind {
        K::Say => "CHAT_MSG_SAY",
        K::Party => "CHAT_MSG_PARTY",
        K::Raid => "CHAT_MSG_RAID",
        K::Guild => "CHAT_MSG_GUILD",
        K::Officer => "CHAT_MSG_OFFICER",
        K::Yell => "CHAT_MSG_YELL",
        K::Whisper => "CHAT_MSG_WHISPER",
        K::WhisperInform => "CHAT_MSG_WHISPER_INFORM",
        K::Emote => "CHAT_MSG_EMOTE",
        K::TextEmote => "CHAT_MSG_TEXT_EMOTE",
        K::System => "CHAT_MSG_SYSTEM",
        K::MonsterSay => "CHAT_MSG_MONSTER_SAY",
        K::MonsterYell => "CHAT_MSG_MONSTER_YELL",
        K::MonsterEmote => "CHAT_MSG_MONSTER_EMOTE",
        K::MonsterWhisper => "CHAT_MSG_MONSTER_WHISPER",
        K::Channel => "CHAT_MSG_CHANNEL",
        K::ChannelJoin => "CHAT_MSG_CHANNEL_JOIN",
        K::ChannelLeave => "CHAT_MSG_CHANNEL_LEAVE",
        K::ChannelNotice => "CHAT_MSG_CHANNEL_NOTICE",
        K::ChannelNoticeUser => "CHAT_MSG_CHANNEL_NOTICE_USER",
        K::ChannelList => "CHAT_MSG_CHANNEL_LIST",
        K::Afk => "CHAT_MSG_AFK",
        K::Dnd => "CHAT_MSG_DND",
        K::Ignored => "CHAT_MSG_IGNORED",
        K::Skill => "CHAT_MSG_SKILL",
        K::Loot => "CHAT_MSG_LOOT",
        K::Money => "CHAT_MSG_MONEY",
        K::CombatXpGain => "CHAT_MSG_COMBAT_XP_GAIN",
        K::CombatHonorGain => "CHAT_MSG_COMBAT_HONOR_GAIN",
        K::RaidLeader => "CHAT_MSG_RAID_LEADER",
        K::RaidWarning => "CHAT_MSG_RAID_WARNING",
        K::RaidBossEmote => "CHAT_MSG_RAID_BOSS_EMOTE",
        K::Filtered => "CHAT_MSG_FILTERED",
        K::Battleground => "CHAT_MSG_BATTLEGROUND",
        K::BattlegroundLeader => "CHAT_MSG_BATTLEGROUND_LEADER",
        K::BgSystemNeutral => "CHAT_MSG_BG_SYSTEM_NEUTRAL",
        K::BgSystemAlliance => "CHAT_MSG_BG_SYSTEM_ALLIANCE",
        K::BgSystemHorde => "CHAT_MSG_BG_SYSTEM_HORDE",
        K::CombatMiscInfo => "CHAT_MSG_COMBAT_MISC_INFO",
        K::CombatSelfHits => "CHAT_MSG_COMBAT_SELF_HITS",
        K::CombatSelfMisses => "CHAT_MSG_COMBAT_SELF_MISSES",
        K::CombatPetHits => "CHAT_MSG_COMBAT_PET_HITS",
        K::CombatPetMisses => "CHAT_MSG_COMBAT_PET_MISSES",
        K::CombatPartyHits => "CHAT_MSG_COMBAT_PARTY_HITS",
        K::CombatPartyMisses => "CHAT_MSG_COMBAT_PARTY_MISSES",
        K::CombatFriendlyPlayerHits => "CHAT_MSG_COMBAT_FRIENDLYPLAYER_HITS",
        K::CombatFriendlyPlayerMisses => "CHAT_MSG_COMBAT_FRIENDLYPLAYER_MISSES",
        K::CombatHostilePlayerHits => "CHAT_MSG_COMBAT_HOSTILEPLAYER_HITS",
        K::CombatHostilePlayerMisses => "CHAT_MSG_COMBAT_HOSTILEPLAYER_MISSES",
        K::CombatCreatureVsSelfHits => "CHAT_MSG_COMBAT_CREATURE_VS_SELF_HITS",
        K::CombatCreatureVsSelfMisses => "CHAT_MSG_COMBAT_CREATURE_VS_SELF_MISSES",
        K::CombatCreatureVsPartyHits => "CHAT_MSG_COMBAT_CREATURE_VS_PARTY_HITS",
        K::CombatCreatureVsPartyMisses => "CHAT_MSG_COMBAT_CREATURE_VS_PARTY_MISSES",
        K::CombatCreatureVsCreatureHits => "CHAT_MSG_COMBAT_CREATURE_VS_CREATURE_HITS",
        K::CombatCreatureVsCreatureMisses => "CHAT_MSG_COMBAT_CREATURE_VS_CREATURE_MISSES",
        K::CombatFriendlyDeath => "CHAT_MSG_COMBAT_FRIENDLY_DEATH",
        K::CombatHostileDeath => "CHAT_MSG_COMBAT_HOSTILE_DEATH",
        K::SpellSelfDamage => "CHAT_MSG_SPELL_SELF_DAMAGE",
        K::SpellSelfBuff => "CHAT_MSG_SPELL_SELF_BUFF",
        K::SpellPetDamage => "CHAT_MSG_SPELL_PET_DAMAGE",
        K::SpellPetBuff => "CHAT_MSG_SPELL_PET_BUFF",
        K::SpellPartyDamage => "CHAT_MSG_SPELL_PARTY_DAMAGE",
        K::SpellPartyBuff => "CHAT_MSG_SPELL_PARTY_BUFF",
        K::SpellFriendlyPlayerDamage => "CHAT_MSG_SPELL_FRIENDLYPLAYER_DAMAGE",
        K::SpellFriendlyPlayerBuff => "CHAT_MSG_SPELL_FRIENDLYPLAYER_BUFF",
        K::SpellHostilePlayerDamage => "CHAT_MSG_SPELL_HOSTILEPLAYER_DAMAGE",
        K::SpellHostilePlayerBuff => "CHAT_MSG_SPELL_HOSTILEPLAYER_BUFF",
        K::SpellCreatureVsSelfDamage => "CHAT_MSG_SPELL_CREATURE_VS_SELF_DAMAGE",
        K::SpellCreatureVsSelfBuff => "CHAT_MSG_SPELL_CREATURE_VS_SELF_BUFF",
        K::SpellCreatureVsPartyDamage => "CHAT_MSG_SPELL_CREATURE_VS_PARTY_DAMAGE",
        K::SpellCreatureVsPartyBuff => "CHAT_MSG_SPELL_CREATURE_VS_PARTY_BUFF",
        K::SpellCreatureVsCreatureDamage => "CHAT_MSG_SPELL_CREATURE_VS_CREATURE_DAMAGE",
        K::SpellCreatureVsCreatureBuff => "CHAT_MSG_SPELL_CREATURE_VS_CREATURE_BUFF",
        K::SpellTradeskills => "CHAT_MSG_SPELL_TRADESKILLS",
        K::SpellDamageShieldsOnSelf => "CHAT_MSG_SPELL_DAMAGESHIELDS_ON_SELF",
        K::SpellDamageShieldsOnOthers => "CHAT_MSG_SPELL_DAMAGESHIELDS_ON_OTHERS",
        K::SpellAuraGoneSelf => "CHAT_MSG_SPELL_AURA_GONE_SELF",
        K::SpellAuraGoneParty => "CHAT_MSG_SPELL_AURA_GONE_PARTY",
        K::SpellAuraGoneOther => "CHAT_MSG_SPELL_AURA_GONE_OTHER",
        K::SpellItemEnchantments => "CHAT_MSG_SPELL_ITEM_ENCHANTMENTS",
        K::SpellBreakAura => "CHAT_MSG_SPELL_BREAK_AURA",
        K::SpellPeriodicSelfDamage => "CHAT_MSG_SPELL_PERIODIC_SELF_DAMAGE",
        K::SpellPeriodicSelfBuffs => "CHAT_MSG_SPELL_PERIODIC_SELF_BUFFS",
        K::SpellPeriodicPartyDamage => "CHAT_MSG_SPELL_PERIODIC_PARTY_DAMAGE",
        K::SpellPeriodicPartyBuffs => "CHAT_MSG_SPELL_PERIODIC_PARTY_BUFFS",
        K::SpellPeriodicFriendlyPlayerDamage => "CHAT_MSG_SPELL_PERIODIC_FRIENDLYPLAYER_DAMAGE",
        K::SpellPeriodicFriendlyPlayerBuffs => "CHAT_MSG_SPELL_PERIODIC_FRIENDLYPLAYER_BUFFS",
        K::SpellPeriodicHostilePlayerDamage => "CHAT_MSG_SPELL_PERIODIC_HOSTILEPLAYER_DAMAGE",
        K::SpellPeriodicHostilePlayerBuffs => "CHAT_MSG_SPELL_PERIODIC_HOSTILEPLAYER_BUFFS",
        K::SpellPeriodicCreatureDamage => "CHAT_MSG_SPELL_PERIODIC_CREATURE_DAMAGE",
        K::SpellPeriodicCreatureBuffs => "CHAT_MSG_SPELL_PERIODIC_CREATURE_BUFFS",
        K::SpellFailedLocalPlayer => "CHAT_MSG_SPELL_FAILED_LOCALPLAYER",
        K::CombatFactionChange => "CHAT_MSG_COMBAT_FACTION_CHANGE",
    }
}

/// The `arg1` token for a `SMSG_CHANNEL_NOTIFY` byte, read as `CHAT_<token>_NOTICE`
/// (`GlobalStrings.lua:494-745`), matching the client's jump table `0x49c60c`. `None`: the
/// member-line bytes `0x00`/`0x01`, MODE_CHANGE `0x0C`, which fires no event (`0x49c24d`), and
/// anything past `0x1F`. `0x02` answers `YOU_CHANGED` in state 2 (`0x49c087`); `0x03` answers
/// `SUSPENDED` in state 3 (`0x49c0e0`), where `YOU_LEFT` would delete the window's channel.
pub(crate) fn notice_token(
    byte: u8,
    state: Option<super::edit::SlotState>,
) -> Option<&'static str> {
    use super::edit::SlotState;
    use benilla_protocol::messages::channel_notice as n;
    Some(match byte {
        // A border crossing prints "Changed Channel: [1. General - Westfall]".
        n::YOU_JOINED if state == Some(SlotState::Renamed) => "YOU_CHANGED",
        n::YOU_JOINED => "YOU_JOINED",
        // Walking out of a capital keeps the record, so not the token that deletes it.
        n::YOU_LEFT if state == Some(SlotState::Suspended) => "SUSPENDED",
        n::YOU_LEFT => "YOU_LEFT",
        n::WRONG_PASSWORD => "WRONG_PASSWORD",
        n::NOT_MEMBER => "NOT_MEMBER",
        n::NOT_MODERATOR => "NOT_MODERATOR",
        n::PASSWORD_CHANGED => "PASSWORD_CHANGED",
        n::OWNER_CHANGED => "OWNER_CHANGED",
        n::PLAYER_NOT_FOUND => "PLAYER_NOT_FOUND",
        n::NOT_OWNER => "NOT_OWNER",
        n::CHANNEL_OWNER => "CHANNEL_OWNER",
        n::ANNOUNCEMENTS_ON => "ANNOUNCEMENTS_ON",
        n::ANNOUNCEMENTS_OFF => "ANNOUNCEMENTS_OFF",
        n::MODERATION_ON => "MODERATION_ON",
        n::MODERATION_OFF => "MODERATION_OFF",
        n::MUTED => "MUTED",
        n::PLAYER_KICKED => "PLAYER_KICKED",
        n::BANNED => "BANNED",
        n::PLAYER_BANNED => "PLAYER_BANNED",
        n::PLAYER_UNBANNED => "PLAYER_UNBANNED",
        n::PLAYER_NOT_BANNED => "PLAYER_NOT_BANNED",
        n::PLAYER_ALREADY_MEMBER => "PLAYER_ALREADY_MEMBER",
        n::INVITE => "INVITE",
        n::INVITE_WRONG_FACTION => "INVITE_WRONG_FACTION",
        n::WRONG_FACTION => "WRONG_FACTION",
        n::INVALID_NAME => "INVALID_NAME",
        n::NOT_MODERATED => "NOT_MODERATED",
        n::PLAYER_INVITED => "PLAYER_INVITED",
        n::PLAYER_INVITE_BANNED => "PLAYER_INVITE_BANNED",
        n::THROTTLED => "THROTTLED",
        _ => return None,
    })
}

/// The kind's row in the shipped default colour table (`.rdata 0x804710`), which the chat bubble
/// and the edit box header tint with; the window's line colour is `ChatTypeInfo`'s.
pub(crate) fn default_color(kind: ChatEventKind) -> [u8; 3] {
    use ChatEventKind as K;
    match kind {
        K::Say => [255, 255, 255],
        K::Party => [170, 170, 255],
        K::Raid => [255, 127, 0],
        K::Guild => [64, 255, 64],
        K::Officer => [64, 192, 64],
        K::Yell => [255, 64, 64],
        K::Whisper | K::WhisperInform | K::Afk | K::Dnd => [255, 128, 255],
        K::Emote | K::TextEmote => [255, 128, 64],
        K::System => [255, 255, 0],
        K::MonsterSay => [255, 255, 159],
        K::MonsterYell => [255, 64, 64],
        K::MonsterEmote => [255, 128, 64],
        K::MonsterWhisper => [179, 179, 179],
        K::Channel => [255, 192, 192],
        K::ChannelJoin | K::ChannelLeave | K::ChannelList => [192, 128, 128],
        K::ChannelNotice | K::ChannelNoticeUser => [192, 192, 192],
        K::Ignored => [255, 0, 0],
        // `ChatTypeInfo["FILTERED"]` has no colour (`ChatFrame.lua:112`), so the line takes the
        // window's default, white; no bubble or edit box header shows this kind.
        K::Filtered => [255, 255, 255],
        K::Skill => [85, 85, 255],
        K::Loot => [0, 170, 0],
        K::Money => [255, 255, 0],
        K::CombatXpGain => [111, 111, 255],
        K::CombatHonorGain => [224, 202, 10],
        K::RaidLeader | K::RaidWarning | K::RaidBossEmote | K::BattlegroundLeader => {
            [255, 219, 183]
        }
        K::Battleground => [255, 127, 0],
        K::BgSystemNeutral => [255, 120, 10],
        K::BgSystemAlliance => [0, 174, 239],
        K::BgSystemHorde => [255, 0, 0],
        K::CombatMiscInfo | K::CombatFactionChange => [128, 128, 255],
        // ── the combat log ───────────────────────────────────────────────────────────────
        // White but for your own spells and what hits you.
        K::SpellSelfDamage | K::SpellSelfBuff => [255, 255, 0],
        K::CombatCreatureVsSelfHits | K::CombatCreatureVsSelfMisses => [255, 47, 47],
        K::SpellCreatureVsSelfDamage => [202, 76, 217],
        K::CombatSelfHits
        | K::CombatSelfMisses
        | K::CombatPetHits
        | K::CombatPetMisses
        | K::CombatPartyHits
        | K::CombatPartyMisses
        | K::CombatFriendlyPlayerHits
        | K::CombatFriendlyPlayerMisses
        | K::CombatHostilePlayerHits
        | K::CombatHostilePlayerMisses
        | K::CombatCreatureVsPartyHits
        | K::CombatCreatureVsPartyMisses
        | K::CombatCreatureVsCreatureHits
        | K::CombatCreatureVsCreatureMisses
        | K::SpellPetDamage
        | K::SpellPetBuff
        | K::SpellPartyDamage
        | K::SpellPartyBuff
        | K::SpellFriendlyPlayerDamage
        | K::SpellFriendlyPlayerBuff
        | K::SpellHostilePlayerDamage
        | K::SpellHostilePlayerBuff
        | K::SpellCreatureVsSelfBuff
        | K::SpellCreatureVsPartyDamage
        | K::SpellCreatureVsPartyBuff
        | K::SpellCreatureVsCreatureDamage
        | K::SpellCreatureVsCreatureBuff
        | K::SpellDamageShieldsOnSelf
        | K::SpellDamageShieldsOnOthers
        | K::SpellPeriodicSelfDamage
        | K::SpellPeriodicSelfBuffs
        | K::SpellPeriodicPartyDamage
        | K::SpellPeriodicPartyBuffs
        | K::SpellPeriodicFriendlyPlayerDamage
        | K::SpellPeriodicFriendlyPlayerBuffs
        | K::SpellPeriodicHostilePlayerDamage
        | K::SpellPeriodicHostilePlayerBuffs
        | K::SpellPeriodicCreatureDamage
        | K::SpellPeriodicCreatureBuffs
        | K::CombatFriendlyDeath
        | K::CombatHostileDeath
        | K::SpellTradeskills
        | K::SpellAuraGoneSelf
        | K::SpellAuraGoneParty
        | K::SpellAuraGoneOther
        | K::SpellItemEnchantments
        | K::SpellBreakAura
        | K::SpellFailedLocalPlayer => [255, 255, 255],
    }
}

/// Map a wire `ChatMsg` byte to its kind; `None` for a type vmangos never sends as wire chat (the
/// combat log) or one not modelled, which the router drops loudly.
pub(crate) fn kind_of_wire(chat_type: u8) -> Option<ChatEventKind> {
    use benilla_protocol::messages as m;
    use ChatEventKind as K;
    Some(match chat_type {
        m::CHAT_MSG_SAY => K::Say,
        m::CHAT_MSG_PARTY => K::Party,
        m::CHAT_MSG_RAID => K::Raid,
        m::CHAT_MSG_GUILD => K::Guild,
        m::CHAT_MSG_OFFICER => K::Officer,
        m::CHAT_MSG_YELL => K::Yell,
        m::CHAT_MSG_WHISPER => K::Whisper,
        m::CHAT_MSG_WHISPER_INFORM => K::WhisperInform,
        m::CHAT_MSG_EMOTE => K::Emote,
        m::CHAT_MSG_SYSTEM => K::System,
        m::CHAT_MSG_MONSTER_SAY => K::MonsterSay,
        m::CHAT_MSG_MONSTER_YELL => K::MonsterYell,
        m::CHAT_MSG_MONSTER_EMOTE => K::MonsterEmote,
        m::CHAT_MSG_CHANNEL => K::Channel,
        m::CHAT_MSG_AFK => K::Afk,
        m::CHAT_MSG_DND => K::Dnd,
        m::CHAT_MSG_IGNORED => K::Ignored,
        m::CHAT_MSG_MONSTER_WHISPER | m::CHAT_MSG_RAID_BOSS_WHISPER => K::MonsterWhisper,
        m::CHAT_MSG_RAID_LEADER => K::RaidLeader,
        m::CHAT_MSG_RAID_WARNING => K::RaidWarning,
        m::CHAT_MSG_RAID_BOSS_EMOTE => K::RaidBossEmote,
        m::CHAT_MSG_FILTERED => K::Filtered,
        m::CHAT_MSG_BATTLEGROUND => K::Battleground,
        m::CHAT_MSG_BATTLEGROUND_LEADER => K::BattlegroundLeader,
        m::CHAT_MSG_BG_SYSTEM_NEUTRAL => K::BgSystemNeutral,
        m::CHAT_MSG_BG_SYSTEM_ALLIANCE => K::BgSystemAlliance,
        m::CHAT_MSG_BG_SYSTEM_HORDE => K::BgSystemHorde,
        _ => return None,
    })
}

/// The arg6 flag token for a wire chat-tag byte (vmangos `Player::GetChatTag`), read as
/// `CHAT_FLAG_<flag>`.
pub(crate) fn flag_of_tag(chat_tag: u8) -> &'static str {
    use benilla_protocol::messages::chat_tag as t;
    match chat_tag {
        t::AFK => "AFK",
        t::DND => "DND",
        t::GM => "GM",
        _ => "",
    }
}

/// The 1.12 language names by wire id (vmangos `SharedDefines.h`, `LANG_*`), for the `[Language]`
/// header. Language 0 has no `Languages.dbc` row, so the stock `arg3 ~= "Universal"` test never
/// fires: an empty arg3 is what suppresses the header (`ChatFrame.lua:1442`).
pub(crate) fn language_name(id: u32) -> &'static str {
    match id {
        1 => "Orcish",
        2 => "Darnassian",
        3 => "Taurahe",
        6 => "Dwarvish",
        7 => "Common",
        8 => "Demonic",
        9 => "Titan",
        10 => "Thalassian",
        11 => "Draconic",
        12 => "Kalimag",
        13 => "Gnomish",
        14 => "Troll",
        33 => "Gutterspeak",
        _ => "", // 0, Universal, and unknown ids: no header
    }
}
