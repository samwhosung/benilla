//! The chat arc's internal currency (decision 0288 §1): [`ChatEvent`] mirrors the reference
//! client's `CHAT_MSG_*` event + `arg1..argN` shape as typed fields, so every source — the wire
//! (`SMSG_MESSAGECHAT`, `SMSG_CHANNEL_NOTIFY`, whisper-fail errors, `SMSG_TEXT_EMOTE`) and the
//! client-composed feeds (loot receive lines, played time, rolls) — speaks one vocabulary, and
//! the router/composer ([`super::frames`]) is the single place lines are formatted, colored, and
//! fanned across the docked windows.
//!
//! The kind set is the renderable subset of the reference's `ChatTypeInfo` (its `COMBAT_*`/
//! `SPELL_*` block is the combat-log content arc, deliberately out — 0288 §3, except
//! `COMBAT_XP_GAIN`, pulled in by the ding arc 0304: the XP line is part of leveling feedback);
//! the group tables are `ChatTypeGroup` transcribed; the colors are the complete shipped default
//! table (the ref client's own `chat-cache.txt` COLORS block ≡ the
//! `.rdata 0x804710` table, double-sourced in 0288's pin).
//!
//! This module also carries the **Lua face** of that currency: [`event_name`]
//! (kind → the reference's `CHAT_MSG_*` event name) and [`ChatEvent::script_args`] (the ten
//! positional args the client's own fire helper passes). 0288 §1 left that door open in its own
//! words — *"a future 0068 addon-API phase can move it into the VM (fire CHAT_MSG_* events at Lua)
//! without touching sources or sinks"* — and this is that phase: the router
//! ([`super::frames::route`]) now fires the real event beside the Rust render, so an addon sees the
//! same chat the window does.

use benilla_ui::script::ScriptValue;

/// The renderable chat-event kinds — `ChatTypeInfo`'s keys, minus the combat-log block.
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
    /// The client-composed XP line (`SMSG_LOG_XPGAIN`, decision 0304) — the one `COMBAT_*`
    /// family member modeled (0288 §3 keeps the rest for the combat-log content arc).
    CombatXpGain,
    /// The client-composed honor line (`SMSG_PVP_CREDIT`, decision 1512) — the XP line's twin,
    /// and the second `COMBAT_*` family member modeled. Composed here rather than fired from the
    /// wire because the packet carries a guid and a rank *number*: the sentence needs the
    /// victim's NAME and their rank TITLE, so it is built after the name resolve exactly as
    /// [`Self::CombatXpGain`] is.
    CombatHonorGain,
    RaidLeader,
    RaidWarning,
    RaidBossEmote,
    /// `CHAT_MSG_FILTERED` (`0x5B`) — the server's "your message was filtered" notice, whose
    /// `arg2` is the addressee the stock frame formats `CHAT_FILTERED` over (2077).
    Filtered,
    Battleground,
    BattlegroundLeader,
    BgSystemNeutral,
    BgSystemAlliance,
    BgSystemHorde,
    // ── the combat log (B297; completed by 1703) ───────────────────────────────────────────
    // The `COMBAT_*`/`SPELL_*` block 0288 §3 held back as "the combat-log content arc". 1571 shipped
    // the damage/heal/power 44; 1703 added the eleven leaves it named as "deliberately out because
    // their wire sources are undecoded" — the death pair, the three AURA_GONE rows, MISC_INFO,
    // TRADESKILLS, ITEM_ENCHANTMENTS, BREAK_AURA, FACTION_CHANGE and FAILED_LOCALPLAYER. Every kind
    // here has a producer in [`super::combat`]; a kind with none is dead weight the sweeps cannot
    // police, which is why the enum grew only as the arms did.
    /// `0x19` — the reference's catch-all combat line: `DURABILITYDAMAGE_DEATH`, `SPELLDISMISSPET*`,
    /// `PET_LOYALTY_*`, `SPELLHAPPINESSDRAIN*`, and the "string not found" warning.
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
    /// `0x2b` — a death whose victim classifies 0..5 (you, yours, a friendly player or their pet).
    CombatFriendlyDeath,
    /// `0x2c` — every other death: a hostile player, a creature, an unresolvable victim.
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
    /// `0x3e` — the item a cast produced: `TRADESKILL_LOG_*` and `FEEDPET_LOG_*`.
    SpellTradeskills,
    SpellDamageShieldsOnSelf,
    SpellDamageShieldsOnOthers,
    /// `0x41` — an aura leaving you or your pet (`AURAREMOVED*`).
    SpellAuraGoneSelf,
    /// `0x42` — an aura leaving a party member or their pet.
    SpellAuraGoneParty,
    /// `0x43` — an aura leaving anyone else.
    SpellAuraGoneOther,
    /// `0x44` — an enchant landing on or fading from an item (`ITEMENCHANTMENT*`).
    SpellItemEnchantments,
    /// `0x45` — an aura dispelled or stolen (`AURADISPEL*`, `AURASTOLEN*`).
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
    /// `0x50` — your own cast that failed (`SPELLFAIL{CAST,PERFORM}SELF`).
    SpellFailedLocalPlayer,
    /// `0x55` — a reputation delta (`FACTION_STANDING_INCREASED`/`_DECREASED`).
    CombatFactionChange,
}

impl ChatEventKind {
    /// The `COMBAT_*` / `SPELL_*` block — the combat log's own types.
    ///
    /// The reference asks this by string prefix (`ChatFrame_OnEvent` l.1397/1399:
    /// `strsub(type,1,7) == "COMBAT_"`, `strsub(type,1,6) == "SPELL_"`), which over a typed enum is
    /// a membership test. Both `COMBAT_XP_GAIN` and `COMBAT_HONOR_GAIN` are inside the reference's
    /// prefix and are deliberately **outside** this one: they were already modeled by the ding and
    /// honour arcs, already listed in the verbatim arm by name, and answer to a different composer
    /// path — including them here would change nothing but would make the predicate lie about what
    /// [`super::combat`] produces.
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

    /// Every kind, for the sweeps that must be exhaustive to be worth anything — chiefly
    /// `ui_script::chat_tests::fired_event_names_are_all_chat_type_info_keys`, which checks each
    /// name we fire against the live `ChatTypeInfo` table rather than against a second copy of the
    /// same list. Adding a variant makes [`event_name`]'s match fail to compile; the length
    /// assertion in `tests::every_kind_is_in_all` is what makes you add it here too.
    ///
    /// Test-only: the app itself never sweeps the kinds — it always has one in hand.
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

/// One chat event — the reference's `CHAT_MSG_*` fire, typed.
///
/// **The arg list is TEN wide, and its shape is byte-pinned.** The client's per-type fire helper
/// `0x49b0b0` (reached from the chat chokepoint `0x49a870` at `0x49ac9a`) calls
/// `FrameScript_SignalEvent 0x703f50` with the format string `"%s%s%s%s%s%s%d%d%s%d"`
/// (`.rdata 0x844608`). So **arg1..arg6 are strings, arg7 and arg8 are numbers, arg9
/// is a string, arg10 is a number**, and not one of them is ever `nil`: `ChatFrame_OnEvent`
/// compares `arg7 > 0` and `arg10 > 0` bare, which under Lua 5.0 errors on a nil. Every slot is
/// always passed — zero or empty when unused.
///
/// Field ↔ arg mapping, each slot named by the consumer that reads it in the shipped
/// `ChatFrame.lua` (0288's pin; line numbers are that file's):
///
/// | arg | field | what it is |
/// |---|---|---|
/// | 1 | `text` / `notice` | the message body; for the CHANNEL_NOTICE family instead the **notice token** selecting `CHAT_<token>_NOTICE` (l.1416/1424) |
/// | 2 | `sender` | the speaker, or the notice's affected player (l.1404, l.1416) |
/// | 3 | `language` | already a *name* ("Orcish"); empty = no header (l.1442) |
//
// (arg3's empty case, checked rather than assumed: l.1442 also guards `arg3 ~= "Universal"`, which
// reads like the client passes that word for language 0. It does not — "Universal" is in neither
// `Languages.dbc` (13 rows, ids 1-33, no 0) nor `WoW.exe` nor `GlobalStrings.lua`, so that arm is
// vestigial in 1.12 and `strlen(arg3) > 0` is what actually suppresses the header. Our
// [`language_name`] answering "" for 0 is therefore the right shape, not a shortcut.)
/// | 4 | `channel` | the display form, "N. Name - Zone" when numbered (l.1373, l.1463) |
/// | 5 | `target` | the second name of a two-name notice — "X kicked by Y" (l.1414-1416) |
/// | 6 | `flag` | "AFK"/"DND"/"GM", empty none; read as `CHAT_FLAG_<flag>` (l.1431) |
/// | 7 | `zone_channel_id` | the **`ChatChannels.dbc` ChannelID** behind a zone channel, 0 for a custom one — matched against `ChatFrame.zoneChannelList` (l.1379) |
/// | 8 | `channel_number` | the client-local joined-channel slot; `ChatTypeInfo["CHANNEL"..arg8]` (l.1381) |
/// | 9 | `channel_base` | the channel name **without** the leading number (l.1378's own comment) |
/// | 10 | — (always 0) | the channel's **split/instance index**, appended as `arg4.." "..arg10` when `> 0` (l.1421-1423) |
///
/// **arg10 is deliberately not a field.** It is `slot+0x98`, the same value `GetChannelName`
/// returns third, and its wire source is the **second** `u32` of `SMSG_CHANNEL_NOTIFY`'s YOU_JOINED
/// tail — the one `Channel::MakeYouJoined` (`Chat/Channel.cpp:823-827`) hardcodes to 0 with the
/// comment *"the non-zero number will be appended to the channel name"*, which is `ChatFrame.lua`
/// l.1421-1423 exactly. (The *first* u32 of that tail is the channel flags, which the client reads
/// and discards.) Our decode already drops the second for that reason
/// (`ChannelNoticeTail::YouJoined`), so [`ChatEvent::script_args`] passes the literal 0 rather than
/// carrying a field that can only ever hold it.
///
/// **arg4/arg7/arg8/arg9 are one record**, all `""`/`0`/`0`/`""` together when the channel is not
/// in the local list — see [`super::edit::ChannelState::stamp_channel`], which is the only place
/// they are written.
///
/// **arg1 IS the garbled text** (B262, decision 1485). The reference fills `0x49a870`'s one buffer
/// exactly once — a plain `SStrCopy` at `0x49a9f0` or the garble `0x49b560` at `0x49aa7c` — and
/// never reads the raw wire pointer again, so every consumer downstream shares it: the chat line,
/// this event's arg1, and the bubble. **An addon receiving a foreign-language line cannot recover
/// the plaintext**, and no other slot in this ten-argument tuple carries the body. Ours now behaves
/// the same way ([`super::language`] owns the gate, [`benilla_formats::garble`] the substitution).
///
/// Still unmodelled: the profanity filter `0x4a1ca0` (`0x49ab23`), which in the reference can
/// suppress the whole event for non-whisper types.
///
/// **Corrections on record.** This comment previously listed args 1-6, 8 and 9 only — arg7 and
/// arg10 were absent, and arg7 is a real slot the reference's own channel routing turns on. The
/// omission was ours. The whole map is now read end to end off `0x49b0b0`, which also **refutes**
/// 0288's standing lead that an earlier trace had covered this marshaller: that trace was opcode
/// `0x92` = SMSG_GUILD_EVENT, misfiled. Real `SMSG_MESSAGECHAT` is `0x96` → `0x49d560` and matches
/// vmangos branch for branch, which is what benilla's decode already did.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ChatEvent {
    pub kind: Option<ChatEventKind>,
    pub text: String,
    pub sender: String,
    pub language: String,
    pub channel: String,
    pub target: String,
    pub flag: String,
    /// arg7 — the `ChatChannels.dbc` ChannelID behind a zone channel (1 General, 2 Trade,
    /// 22 LocalDefense); 0 for a custom channel. Filled from [`super::edit::ChannelState`].
    pub zone_channel_id: u32,
    pub channel_number: u32,
    pub channel_base: String,
    pub notice: String,
    /// The state of our own slot for this channel **as the notice arrived**, or `None` when we
    /// hold no slot for it. Set once at the top of [`super::feed::deliver`], before that function
    /// changes any of it — because the reference's notice arms read `slot+0x9c` to pick the token
    /// and only then move it (`0x49c0c2` reads, `0x49bb20` writes).
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

    /// The notice byte behind a CHANNEL_NOTICE(_USER) event, if this is one.
    ///
    /// `notice` carries the `SMSG_CHANNEL_NOTIFY` byte in decimal because the composer and the
    /// event bridge both need it and the field predates both; this is the single parse.
    pub(crate) fn notice_byte(&self) -> Option<u8> {
        self.notice.parse().ok()
    }

    /// This event's `arg1..arg10`, in the reference's own order and types (see the struct doc's
    /// table). Ten values, always — `nil` is not a legal value in any slot.
    pub(crate) fn script_args(&self) -> Vec<ScriptValue> {
        // arg1 is the notice TOKEN for the notice family and the body for everything else — the
        // one slot whose meaning is type-dependent (`ChatFrame_OnEvent` l.1416/1424 vs l.1396).
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
            // arg10 — see the struct doc: always 0 off this server.
            ScriptValue::Int(0),
        ]
    }
}

/// The reference's event NAME for a kind — `"CHAT_MSG_" ++ <the `ChatTypeInfo` key>`.
///
/// The key spellings are the reference's own (they are what `ChatFrame_OnEvent` recovers with
/// `strsub(event, 10)` and looks up in `ChatTypeInfo`), so this table and the `ChatTypeInfo` the
/// engine seeds are the same set by construction — asserted in
/// `ui_script::chat_tests::every_fired_event_name_is_a_chat_type_info_key`.
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

/// The notice TOKEN a `SMSG_CHANNEL_NOTIFY` byte becomes in `arg1` — the token
/// `ChatFrame_OnEvent` splices into `getglobal("CHAT_"..arg1.."_NOTICE")` (l.1416/1424).
///
/// The token set is read off the shipped `GlobalStrings.lua`'s own `CHAT_<X>_NOTICE` keys
/// (l.494-745 of the extracted file), paired to the vmangos notice byte that produces each line —
/// the same pairing [`super::frames::compose_notice`] already renders, which is why the two tables
/// are asserted against each other rather than left to drift
/// (`ui_chat::tests::every_rendered_notice_has_a_token`).
///
/// Byte-for-byte identical to the client's own jump table (`0x49c60c`, 32 direct arms) — checked
/// against that table after the fact, not derived from it.
///
/// `None` = a byte the reference passes no token for: `0x00`/`0x01` are the CHANNEL_JOIN /
/// CHANNEL_LEAVE member lines (their arg1 is the empty string, not a token), `0x0C` MODE_CHANGE
/// fires **no chat event at all** (`0x49c24d` calls `0x49e910` and returns — which is why
/// [`super::feed::ChatLog::push_channel_notice`] drops it before it becomes an event), and anything
/// past `0x1F` is outside vmangos's range.
///
/// **Two of the arms are state-dependent** (decision 2130): the client answers `"YOU_CHANGED"` for
/// `0x02` and `"SUSPENDED"` for `0x03` when its own channel record is in the matching state
/// (`rec+0x9c == 2` at `0x49c087` / `== 3` at `0x49c0e0`), and both alternates are real
/// `CHAT_<X>_NOTICE` strings —
/// *"Changed Channel: [%s]"* and *"Left Channel: [%s]"*. We modelled neither until the zone walk's
/// registration loss made it matter: `arg1` is what the stock `ChatFrame_OnEvent` branches on, and
/// the `YOU_LEFT` branch **deletes the window's channel registration** (`ChatFrame.lua`
/// l.1382-1384). A suspended channel that answers the plain token loses its registration for good.
pub(crate) fn notice_token(
    byte: u8,
    state: Option<super::edit::SlotState>,
) -> Option<&'static str> {
    use super::edit::SlotState;
    use benilla_protocol::messages::channel_notice as n;
    Some(match byte {
        // The confirming notice for a slot the zone walk renamed — crossing a zone border prints
        // "Changed Channel: [1. General - Westfall]", not a leave and a join.
        n::YOU_JOINED if state == Some(SlotState::Renamed) => "YOU_CHANGED",
        n::YOU_JOINED => "YOU_JOINED",
        // Walking out of a capital: the record and its number stay, so this must NOT be the token
        // that tears the registration down.
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
        // MODE_CHANGE (0x0C) has no CHAT_*_NOTICE string in 1.12 — the reference renders nothing,
        // so there is no token. JOINED/LEFT (0x00/0x01) are not notices at all: they are the
        // CHANNEL_JOIN/CHANNEL_LEAVE member-line events.
        _ => return None,
    })
}

/// The kind's row in the shipped colour table — what the chat bubble and the edit box's header
/// tint with. The window's own line colour is `ChatTypeInfo`'s, in the reference's Lua (1948).
///
/// The complete shipped table (chat-cache COLORS ≡ `.rdata 0x804710`, both quoted in
/// 0288's pin; entries this kind set carries).
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
        // `ChatTypeInfo["FILTERED"] = { sticky = 0 }` (ChatFrame.lua l.112) carries **no colour**,
        // so the stock frame passes nil r/g/b to `AddMessage` and the line takes the window's own
        // default. White is that default, and this table's consumers (the bubble tint, the edit
        // box header) never see this kind anyway — it is not a speech line.
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
        // The two `8080ff` rows, both from the reference's table: the catch-all combat line and
        // the reputation line share a colour and nothing else.
        K::CombatMiscInfo | K::CombatFactionChange => [128, 128, 255],
        // ── the combat log ───────────────────────────────────────────────────────────────
        // The shipped defaults are overwhelmingly plain white; the three that are not are the
        // ones about YOU being hit, and they are the reason the block is grouped by colour
        // rather than listed in table order — the exceptions are the content.
        // Your own spell work: the one gold pair in the block.
        K::SpellSelfDamage | K::SpellSelfBuff => [255, 255, 0],
        // A creature hitting YOU — the red that makes incoming melee read.
        K::CombatCreatureVsSelfHits | K::CombatCreatureVsSelfMisses => [255, 47, 47],
        // A creature's spell landing on YOU.
        K::SpellCreatureVsSelfDamage => [202, 76, 217],
        // Everything else in the block: plain white.
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

/// Map a wire `ChatMsg` byte (`SMSG_MESSAGECHAT.chat_type`) to its event kind. `None` = a type
/// vmangos never emits as wire chat (the combat-log block) or one we don't model — the router
/// drops it loudly.
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

/// The `<AFK>`/`<DND>`/`<GM>` flag token for a wire chat-tag byte (`Player::GetChatTag`) — the
/// event's `flag` field (the ref's arg6), consumed as `CHAT_FLAG_<flag>`.
pub(crate) fn flag_of_tag(chat_tag: u8) -> &'static str {
    use benilla_protocol::messages::chat_tag as t;
    match chat_tag {
        t::AFK => "AFK",
        t::DND => "DND",
        t::GM => "GM",
        _ => "",
    }
}

/// The 1.12 language names by wire id (vmangos `SharedDefines.h` `LANG_*` — the small racial
/// set), for the `[Language]` header when a line isn't Universal/our default. Unknown ids render
/// no header (v1: benilla speaks Common; the picker is a later arc).
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
        _ => "", // 0 = Universal (no header), unknowns likewise
    }
}
