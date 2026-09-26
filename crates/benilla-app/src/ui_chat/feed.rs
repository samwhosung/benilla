//! Every inbound chat source becomes one [`ChatEvent`] here, its names resolved ask-once through
//! [`crate::names::NameCache`] with bounded re-checks, then routed by [`super::frames::route`].

use bevy::prelude::*;

use benilla_protocol::messages::{
    channel_notice, ChannelNoticeTail, ChatMessage, LevelUpInfo, XpGain, MACRO_EXPANDED_TYPES,
};

use benilla_assets::{LockRecover, WorldAssets};
use benilla_formats::{EmoteLine, EmoteTextCatalog};

use crate::names::NameCache;
use crate::net::{GuidIndex, NetCommands, ObjectStore, SelfGuid};

use super::edit::ChannelState;
use super::event::{flag_of_tag, kind_of_wire, language_name, ChatEvent, ChatEventKind};
use super::frames::{route, ChatWindows};

/// Frames a line waits on a name, about 2 s at 60 fps, before it gives up.
const NAME_MAX_TRIES: u16 = 120;

/// The text-emote sentence tables, read once off the patch chain.
#[derive(Resource)]
pub(crate) struct EmoteTexts(pub(crate) EmoteTextCatalog);

/// The DBC string column: 0, enUS; the reference reads its locale slot `[0xc0e080]`.
const LOCALE: usize = 0;

/// Load `EmotesText.dbc` with `EmotesTextData.dbc`; before `AssetSet::Open` it loads nothing.
pub(super) fn load_emote_texts(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_emote_text_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("chat: {} emote sentence tables", cat.len());
            commands.insert_resource(EmoteTexts(cat));
        }
        Err(e) => warn!("chat: emote text catalog failed to load: {e:#}"),
    }
}

/// One queued item awaiting its turn through [`feed_chat`].
enum Pending {
    /// A decoded `SMSG_MESSAGECHAT`; the player types wait on the sender's name.
    Wire { msg: ChatMessage, tries: u16 },
    /// A channel notice with guids to name (`b_guid` is a kick's or ban's actor, 0 when absent).
    Notice {
        notice: u8,
        channel: String,
        a_guid: u64,
        b_guid: u64,
        tries: u16,
    },
    /// An inbound addon line waiting on its sender's name: the reference fires `CHAT_MSG_ADDON`
    /// (event 227, `0x49a95f`) from the name-query callback (`0x49ccc0`), so `sender` is a name.
    Addon {
        prefix: String,
        message: String,
        distribution: String,
        guid: u64,
        tries: u16,
    },
    /// A `/random` broadcast awaiting the roller's name.
    Roll {
        min: u32,
        max: u32,
        roll: u32,
        guid: u64,
        tries: u16,
    },
    /// An XP award waiting on the victim's name (0: unnamed); `bonus` is total minus base.
    XpGain {
        victim: u64,
        total: u32,
        bonus: u32,
        tries: u16,
    },
    /// An honor award waiting on the victim's name; `rank` (0 none) becomes a title at the drain,
    /// from the victim's side in the same name-query record.
    HonorGain {
        victim: u64,
        honor: i32,
        rank: u8,
        tries: u16,
    },
    /// A text emote waiting on its performer's name: the reference's `PENDINGTEXTEMOTE` node
    /// (`0x49cc00`), queued when `0x49dbe0` finds no cached name and drained by `0x49d0d0`.
    /// `target` is the wire's name, empty when untargeted, used as sent.
    TextEmote {
        performer: u64,
        text_id: u32,
        target: String,
        tries: u16,
    },
    /// An area discovery (`SMSG_EXPLORATION_EXPERIENCE`), fired at the drain, which holds the VM.
    Discovery { area: String, xp: u32 },
    /// One combat-log line, classified at the packet, waiting on its two endpoint names; the
    /// reference's deferred queue (`0xc4e208`) waits only on an item name.
    Combat(Box<super::combat::PendingCombat>),
    /// A ready event (client-composed lines; name-carrying notices).
    Event(ChatEvent),
}

/// Fire `CHAT_MSG_ADDON(prefix, message, distribution, sender)` (`0x49a95f`), not through
/// [`route`]: it is no `ChatTypeInfo` key and takes four arguments, not ten.
pub(super) fn fire_addon_message(
    script: &mut benilla_ui::script::UiScript,
    prefix: String,
    message: String,
    distribution: String,
    sender: String,
) {
    use benilla_ui::script::ScriptValue::Str;
    script.fire_event(
        "CHAT_MSG_ADDON",
        vec![Str(prefix), Str(message), Str(distribution), Str(sender)],
    );
}

/// A text emote's chat event, or `None` when the sentence is blank, as SIT, STAND and TRAIN are
/// in every locale (`0x49b4bd`). `sender` is the performer, seen only by addons as arg2:
/// `TEXT_EMOTE` prints verbatim (`ChatFrame.lua:1395`), and the fire (`0x49b495`) pushes the name
/// record resolved at `0x49b289`.
pub(super) fn text_emote_event(
    cat: &EmoteTextCatalog,
    text_id: u32,
    line: &EmoteLine,
) -> Option<ChatEvent> {
    Some(ChatEvent {
        kind: Some(ChatEventKind::TextEmote),
        text: cat.compose(text_id, line, LOCALE)?,
        sender: line.performer.to_string(),
        ..Default::default()
    })
}

/// The chat items the feeds queue and [`feed_chat`] drains. World broadcasts wait apart, as their
/// resolve needs more than [`feed_chat`] can hold at Bevy's `SystemParam` limit;
/// `feed_broadcasts` resolves them onto `pending` earlier in the same frame.
#[derive(Resource, Default)]
pub(crate) struct ChatLog {
    pending: Vec<Pending>,
    broadcasts: Vec<super::broadcast::Broadcast>,
    /// A level-up's gains for `ui_unit`'s `PLAYER_LEVEL_UP` fire, matched by level: the fire
    /// follows a descriptor change and the gains a packet.
    level_up_gains: Vec<(LevelUpInfo, u32)>,
}

impl ChatLog {
    /// Queue one world broadcast for [`super::broadcast::feed_broadcasts`]'s resolve pass.
    pub(crate) fn push_broadcast(&mut self, b: super::broadcast::Broadcast) {
        self.broadcasts.push(b);
    }

    /// How many broadcasts are waiting, the drain's early-out.
    pub(crate) fn broadcasts_pending(&self) -> usize {
        self.broadcasts.len()
    }

    /// Take the parked broadcasts, leaving the queue empty.
    pub(crate) fn take_broadcasts(&mut self) -> Vec<super::broadcast::Broadcast> {
        std::mem::take(&mut self.broadcasts)
    }

    /// Park a level-up's gains for the `PLAYER_LEVEL_UP` fire.
    pub(crate) fn push_level_up_gains(&mut self, info: &LevelUpInfo, talent_points: u32) {
        self.level_up_gains.push((*info, talent_points));
    }

    /// Take the parked gains for `level`; with none (a GM's level write), the caller fires zeros.
    pub(crate) fn take_level_up_gains(&mut self, level: u32) -> Option<(LevelUpInfo, u32)> {
        let i = self
            .level_up_gains
            .iter()
            .position(|(l, _)| l.level == level)?;
        Some(self.level_up_gains.remove(i))
    }
}

impl ChatLog {
    /// Queue a decoded wire line (`SMSG_MESSAGECHAT`).
    pub(crate) fn push_wire(&mut self, msg: ChatMessage) {
        self.pending.push(Pending::Wire { msg, tries: 0 });
    }

    /// Queue a client-composed event: loot, quest and system lines, the `/played` answer.
    pub(crate) fn push_event(&mut self, event: ChatEvent) {
        self.pending.push(Pending::Event(event));
    }

    /// Queue one combat-log line, classified at the packet by [`super::combat`], for its names.
    pub(crate) fn push_combat(&mut self, line: super::combat::PendingCombat) {
        self.pending.push(Pending::Combat(Box::new(line)));
    }

    /// Queue a decoded `SMSG_CHANNEL_NOTIFY`; a guid tail waits for its names.
    pub(crate) fn push_channel_notice(
        &mut self,
        notice_byte: u8,
        channel: String,
        tail: &ChannelNoticeTail,
    ) {
        let (a_guid, b_guid, name) = match tail {
            ChannelNoticeTail::Guid(g) | ChannelNoticeTail::Actor(g) => (*g, 0, None),
            ChannelNoticeTail::Actors { target, source } => (*target, *source, None),
            ChannelNoticeTail::Name(n) => (0, 0, Some(n.clone())),
            ChannelNoticeTail::YouJoined { .. } | ChannelNoticeTail::Empty => (0, 0, None),
            ChannelNoticeTail::ModeChange { .. } => return, // silent in the 1.12 UI (no string)
        };
        if a_guid != 0 {
            self.pending.push(Pending::Notice {
                notice: notice_byte,
                channel,
                a_guid,
                b_guid,
                tries: 0,
            });
        } else {
            if let Some(event) = notice_event(notice_byte, channel, name, None) {
                self.pending.push(Pending::Event(event));
            }
        }
    }

    /// Queue an inbound addon line for its sender's name. The text splits on its first tab, and
    /// with none it is all prefix (`0x49a8d0`); `distribution` names PARTY, RAID, GUILD and
    /// BATTLEGROUND, any other type `"UNKNOWN"` (`0x49aff4`).
    pub(crate) fn push_addon(&mut self, text: &str, chat_type: u8, guid: u64) {
        let (prefix, message) = match text.find('\t') {
            Some(i) => (text[..i].to_string(), text[i + 1..].to_string()),
            None => (text.to_string(), String::new()),
        };
        // The protocol's constants, the same ones `crate::net::addon_wire_chat_type` sends.
        let distribution = {
            use benilla_protocol::messages as m;
            match u32::from(chat_type) {
                m::CHAT_TYPE_PARTY => "PARTY",
                m::CHAT_TYPE_RAID => "RAID",
                m::CHAT_TYPE_GUILD => "GUILD",
                m::CHAT_TYPE_BATTLEGROUND => "BATTLEGROUND",
                _ => "UNKNOWN",
            }
        }
        .to_string();
        self.pending.push(Pending::Addon {
            prefix,
            message,
            distribution,
            guid,
            tries: 0,
        });
    }

    /// Queue a `/random` broadcast (`MSG_RANDOM_ROLL`) for the roller-name resolve.
    pub(crate) fn push_roll(&mut self, min: u32, max: u32, roll: u32, guid: u64) {
        self.pending.push(Pending::Roll {
            min,
            max,
            roll,
            guid,
            tries: 0,
        });
    }

    /// Queue an XP award's line (`SMSG_LOG_XPGAIN`); a kill waits on the victim's name.
    pub(crate) fn push_xp_gain(&mut self, x: &XpGain) {
        // Both forks park: the template resolves at the drain; `victim: 0` is the unnamed form.
        let named = x.kill && x.victim != 0;
        self.pending.push(Pending::XpGain {
            victim: if named { x.victim } else { 0 },
            total: x.total,
            bonus: if named {
                x.total.saturating_sub(x.base)
            } else {
                0
            },
            tries: 0,
        });
    }

    /// Queue an honor award's line (`SMSG_PVP_CREDIT`); victim 0 is a bonus or objective award.
    pub(crate) fn push_pvp_credit(&mut self, honor: i32, victim: u64, rank: u8) {
        // Both forks park: the template resolves at the drain.
        self.pending.push(Pending::HonorGain {
            victim,
            honor,
            rank,
            tries: 0,
        });
    }

    /// Queue a text emote (`SMSG_TEXT_EMOTE`) for its performer's name, then its sentence.
    pub(crate) fn push_text_emote(&mut self, performer: u64, text_id: u32, target: String) {
        self.pending.push(Pending::TextEmote {
            performer,
            text_id,
            target,
            tries: 0,
        });
    }

    /// Queue an area discovery: the `ERR_ZONE_EXPLORED` toast on every packet, never chat, and the
    /// `ERR_ZONE_EXPLORED_XP` system line when `xp > 0`.
    pub(crate) fn push_exploration(&mut self, area_name: &str, xp: u32) {
        self.pending.push(Pending::Discovery {
            area: area_name.to_string(),
            xp,
        });
    }

    /// Drop every pending item on disconnect.
    pub(crate) fn clear_session(&mut self) {
        self.pending.clear();
    }

    /// The parked addon lines as `(prefix, message, distribution)`.
    #[cfg(test)]
    pub(crate) fn pending_addons(&self) -> Vec<(String, String, String)> {
        self.pending
            .iter()
            .filter_map(|p| match p {
                Pending::Addon {
                    prefix,
                    message,
                    distribution,
                    ..
                } => Some((prefix.clone(), message.clone(), distribution.clone())),
                _ => None,
            })
            .collect()
    }

    /// How many parked items will render in a chat window; addon lines never do.
    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending
            .iter()
            .filter(|p| !matches!(p, Pending::Addon { .. }))
            .count()
    }

    /// The text of every composed line waiting to render, in order.
    #[cfg(test)]
    pub(crate) fn pending_lines(&self) -> Vec<String> {
        self.pending
            .iter()
            .filter_map(|p| match p {
                Pending::Event(e) => Some(e.text.clone()),
                _ => None,
            })
            .collect()
    }
}

/// The VM's globals as a string lookup, borrowed per composition so `route` can take the VM.
fn globals(script: &benilla_ui::script::UiScript) -> impl Fn(&str) -> Option<String> + '_ {
    |key: &str| script.lua().globals().get::<String>(key).ok()
}

/// Fill a key's template; a missing or empty string prints no line, as in the reference.
fn keyed(
    get: &dyn Fn(&str) -> Option<String>,
    key: &str,
    args: &[benilla_ui::strings::Arg<'_>],
) -> Option<String> {
    let text = benilla_ui::strings::fill(&get(key)?, args);
    (!text.is_empty()).then_some(text)
}

/// The XP line: `COMBATLOG_XPGAIN_FIRSTPERSON`, the `_UNNAMED` form, or the rested
/// `_EXHAUSTION1`, whose last two arguments are the signed bonus and the state word.
pub(super) fn xp_gain_line(
    victim: Option<&str>,
    total: u32,
    bonus: u32,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    use benilla_ui::strings::Arg;
    match victim {
        Some(name) if bonus > 0 => keyed(
            get,
            "COMBATLOG_XPGAIN_EXHAUSTION1",
            &[
                Arg::S(name),
                Arg::D(i64::from(total)),
                Arg::S(&format!("+{bonus}")),
                Arg::S(RESTED_STATE),
            ],
        ),
        Some(name) => keyed(
            get,
            "COMBATLOG_XPGAIN_FIRSTPERSON",
            &[Arg::S(name), Arg::D(i64::from(total))],
        ),
        None => keyed(
            get,
            "COMBATLOG_XPGAIN_FIRSTPERSON_UNNAMED",
            &[Arg::D(i64::from(total))],
        ),
    }
}

/// The `_EXHAUSTION1` template's state word. 1.12 has no GlobalString for it and the reference's
/// own table is untraced; rested is the only bonus state vmangos sets (`Player.cpp:17990-17993`).
const RESTED_STATE: &str = "Rested";

/// The honor line (`0x625270`): `COMBATLOG_HONORAWARD` with no victim, `COMBATLOG_HONORGAIN` for
/// `honor > 0`, else `COMBATLOG_DISHONORGAIN`. No `rank_title` leaves the rank slot empty, as in
/// the reference; vmangos floors a rankless player at rank 5 (`HonorMgr.cpp:1086-1087`).
pub(super) fn honor_gain_line(
    victim: Option<&str>,
    rank_title: Option<&str>,
    honor: i32,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    use benilla_ui::strings::Arg;
    match victim {
        None => keyed(get, "COMBATLOG_HONORAWARD", &[Arg::D(i64::from(honor))]),
        Some(name) if honor <= 0 => keyed(get, "COMBATLOG_DISHONORGAIN", &[Arg::S(name)]),
        Some(name) => keyed(
            get,
            "COMBATLOG_HONORGAIN",
            &[
                Arg::S(name),
                Arg::S(rank_title.unwrap_or_default()),
                Arg::D(i64::from(honor)),
            ],
        ),
    }
}

/// The `ERR_ZONE_EXPLORED` toast (error route 1, `AddErrorMessage` `0x4945b0`), every packet.
pub(super) fn exploration_toast(
    area_name: &str,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    keyed(
        get,
        "ERR_ZONE_EXPLORED",
        &[benilla_ui::strings::Arg::S(area_name)],
    )
}

/// The `ERR_ZONE_EXPLORED_XP` system line, only when the packet carries XP (`jle` at `0x5e422f`).
pub(super) fn exploration_line(
    area_name: &str,
    xp: u32,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    use benilla_ui::strings::Arg;
    keyed(
        get,
        "ERR_ZONE_EXPLORED_XP",
        &[Arg::S(area_name), Arg::D(i64::from(xp))],
    )
}

/// The event a channel notice becomes; [`super::frames::compose_notice`] reads its `notice`.
pub(super) fn notice_event(
    notice_byte: u8,
    channel: String,
    a: Option<String>,
    b: Option<String>,
) -> Option<ChatEvent> {
    // Each arm's `mov edi` in the `0x49c60c` jump table: MODE_CHANGE fires nothing, a byte past
    // THROTTLED fires SYSTEM, the rest CHANNEL_NOTICE (0x12) or CHANNEL_NOTICE_USER (0x13).
    use channel_notice as n;
    let kind = match notice_byte {
        n::JOINED => ChatEventKind::ChannelJoin,
        n::LEFT => ChatEventKind::ChannelLeave,
        n::MODE_CHANGE => return None,
        n::YOU_JOINED
        | n::YOU_LEFT
        | n::WRONG_PASSWORD
        | n::NOT_MEMBER
        | n::NOT_MODERATOR
        | n::NOT_OWNER
        | n::MUTED
        | n::BANNED
        | n::THROTTLED => ChatEventKind::ChannelNotice,
        n::PASSWORD_CHANGED
        | n::OWNER_CHANGED
        | n::PLAYER_NOT_FOUND
        | n::CHANNEL_OWNER
        | n::ANNOUNCEMENTS_ON
        | n::ANNOUNCEMENTS_OFF
        | n::MODERATION_ON
        | n::MODERATION_OFF
        | n::PLAYER_KICKED
        | n::PLAYER_BANNED
        | n::PLAYER_UNBANNED
        | n::PLAYER_NOT_BANNED
        | n::PLAYER_ALREADY_MEMBER
        | n::INVITE
        | n::INVITE_WRONG_FACTION
        | n::WRONG_FACTION
        | n::INVALID_NAME
        | n::NOT_MODERATED
        | n::PLAYER_INVITED
        | n::PLAYER_INVITE_BANNED => ChatEventKind::ChannelNoticeUser,
        _ => ChatEventKind::System,
    };
    Some(ChatEvent {
        kind: Some(kind),
        sender: a.unwrap_or_default(),
        target: b.unwrap_or_default(),
        channel,
        notice: notice_byte.to_string(),
        ..Default::default()
    })
}

/// Deliver one built event: a `YOU_JOINED` claims its slot before the render, a `YOU_LEFT` frees
/// it after, so the leave line keeps its number and colour ("Left Channel: [2. General - Elwynn
/// Forest]") and a handler's `GetChannelName` still finds it. The reference flags the teardown
/// (`0x49c115`), fires the event (`0x49c5b0` calling `0x49a870`), then drops the record
/// (`0x49c5b5`, `0x49c5c2` calling `0x49bbd0`); its suspended leg (`0x49c0e9`) skips the flag.
/// Both arms mirror the list into the VM for `GetChannelName`.
pub(super) fn deliver(
    script: &mut benilla_ui::script::UiScript,
    windows: &mut ChatWindows,
    channels: &mut ChannelState,
    event: &mut ChatEvent,
) {
    let notice = event
        .notice_byte()
        .filter(|_| event.kind == Some(ChatEventKind::ChannelNotice));
    // Read first: the reference picks the token from `slot+0x9c` (`0x49c0c2`) before writing it
    // (`0x49bb20`), and the renamed and suspended tokens keep the window's registration.
    event.slot_state = channels.slot_state(&event.channel);
    if let Some(byte) = notice {
        // The server's half of every join and leave, for a log to tell a request from its answer.
        debug!(
            "chat: channel notice {byte:#04x} for {:?} (slot {:?}, {:?})",
            event.channel,
            channels.number_of(&event.channel),
            event.slot_state
        );
    }
    if notice == Some(channel_notice::YOU_JOINED) {
        // A confirmed join sets the `ZONECHANNELS` bit (`0x49bbaf`), its only runtime growth.
        channels.note_zone_channel_joined(&event.channel);
        // A custom channel's confirmed join enters the chat cache's re-join list.
        channels.note_custom_channel_joined(&event.channel);
        // A slot the walk renamed is numbered already, but its state must return to `Joined`.
        channels.confirm_slot(&event.channel);
    }
    if notice == Some(channel_notice::YOU_JOINED) && channels.number_of(&event.channel).is_none() {
        match channels.claim_slot(&event.channel) {
            Some(slot) => {
                debug!(
                    "chat: server confirms channel {:?} joined (slot {slot})",
                    event.channel
                );
                script.set_joined_channels(channels.names());
            }
            // All ten slots taken: no number, as in the reference, whose slot allocator also
            // prints `ERR_TOO_MANY_CHAT_CHANNELS` (`0x199` at `0x49b9c5`); here it is only logged.
            None => warn!(
                "chat: server confirms channel {:?} joined but all {} slots are taken — it has no \
                 number, so /N cannot reach it",
                event.channel,
                super::edit::MAX_CHANNELS
            ),
        }
    }
    // The wire name, kept before `stamp_channel` decorates arg4 with the slot number.
    let leaving = (notice == Some(channel_notice::YOU_LEFT)).then(|| event.channel.clone());
    // The channel renders numbered when its slot is known.
    channels.stamp_channel(event);
    route(script, windows, event);
    if let Some(name) = leaving {
        // A suspended slot (state 3, `0x49c0e0`) survives its leave: the reference skips the
        // teardown (`0x49c115`), so leaving a capital keeps Trade's number for the re-join.
        if event.slot_state == Some(super::edit::SlotState::Suspended) {
            debug!("chat: channel {name:?} suspended — the record and its number stay");
        } else {
            // Cleared in place, never compacted: slot 3 stays 3 when slot 2 empties.
            let freed = channels.free_slot(&name);
            debug!("chat: server confirms channel {name:?} left (slot {freed:?} now free)");
            script.set_joined_channels(channels.names());
        }
    }
}

/// The speaker's bubble and gesture, bundled for Bevy's sixteen-parameter limit on
/// [`feed_chat`]; the bubble has a 20 yd range test and two CVars, the gesture neither.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct SpeakerEffects<'w> {
    bubbles: ResMut<'w, crate::chat_bubble::BubbleQueue>,
    bubble_cfg: Res<'w, crate::chat_bubble::BubbleConfig>,
    gestures: ResMut<'w, crate::creature_anim::GestureQueue>,
}

/// Drain [`ChatLog`]: resolve names (ask-once, bounded), build the events and [`route`] them.
pub(super) fn feed_chat(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut log: ResMut<ChatLog>,
    mut windows: ResMut<ChatWindows>,
    mut channels: ResMut<ChannelState>,
    names: Res<NameCache>,
    mut speaker: SpeakerEffects,
    commands: Res<NetCommands>,
    // The text-emote tables, and our guid for the "are you the performer?" test.
    emote_texts: Option<Res<EmoteTexts>>,
    self_guid: Res<SelfGuid>,
    // The `$`-macro subject: monster and BG lines expand against their addressee's descriptors.
    guids: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    states: Res<crate::world_state::WorldStates>,
    // The language gate: the word pool, this character's fluency and the GM bit.
    langs: Res<super::language::ChatLanguages>,
    // Item names for `TRADESKILL_LOG`, `FEEDPET_LOG`, `ITEMENCHANTMENT*`, `SPELLDURABILITYDAMAGE`.
    items: Res<crate::items::Items>,
    // The spam and profanity filters, which run in the reference's display function `0x49a870`.
    mut text_filter: crate::text_filter::ChatTextFilter,
) {
    let Some(mut script) = script else {
        return;
    };
    if log.pending.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut log.pending);
    let mut still = Vec::new();
    for item in pending {
        match item {
            Pending::Event(mut event) => {
                deliver(&mut script, &mut windows, &mut channels, &mut event);
            }
            Pending::Wire { msg, tries } => {
                let kind = kind_of_wire(msg.chat_type);
                if kind.is_none() {
                    warn!(
                        "chat: unmodeled wire type {:#04x} dropped: {:?}",
                        msg.chat_type, msg.text
                    );
                    continue;
                }
                // A monster line carries its name inline; a player line resolves its guid.
                let name = match &msg.sender_name {
                    Some(n) => Some(n.clone()),
                    None if needs_name(msg.chat_type) && msg.sender_guid != 0 => names
                        .resolve(msg.sender_guid, &commands)
                        .map(str::to_string),
                    _ => None,
                };
                if name.is_none()
                    && needs_name(msg.chat_type)
                    && msg.sender_guid != 0
                    && tries < NAME_MAX_TRIES
                {
                    still.push(Pending::Wire {
                        msg,
                        tries: tries + 1,
                    });
                    continue;
                }
                // The base name; `stamp_channel` adds arg4's number and arg7-arg9 below.
                let channel_base = msg.channel.clone().unwrap_or_default();
                // `$`-macros expand for the monster, boss and BG system types only, against the
                // addressee. A failed expansion (`0x49dac2`-`0x49db1e`, `0x49d9c9`) with a zero
                // subject or a known name drops the line, one with an unknown name holds it for
                // the name query (`tries`), and only a second failure shows the raw text.
                let expanded = if MACRO_EXPANDED_TYPES.contains(&msg.chat_type) {
                    // The addressee, else the sender: the BG system lines carry one guid only.
                    let subject_guid = if msg.target_guid != 0 {
                        msg.target_guid
                    } else {
                        msg.sender_guid
                    };
                    let subject = crate::npc_text::subject_for_guid(
                        subject_guid,
                        &guids,
                        &stores,
                        &names,
                        &commands,
                    );
                    let (text, clean) = crate::npc_text::substitute_checked(
                        &msg.text,
                        &crate::npc_text::MacroContext {
                            subject: subject.as_ref(),
                            states: &states,
                        },
                    );
                    if clean {
                        Some(text)
                    } else if subject_guid == 0 || names.peek(subject_guid).is_some() {
                        debug!(
                            "chat: dropping unexpandable [{:#04x}] {:?} (subject {subject_guid:#x})",
                            msg.chat_type, msg.text
                        );
                        continue; // the reference prints no line
                    } else if tries < NAME_MAX_TRIES {
                        // `subject_for_guid` sent the name query: hold the raw line until it lands.
                        still.push(Pending::Wire {
                            msg,
                            tries: tries + 1,
                        });
                        continue;
                    } else {
                        None // the second failure: show the raw text
                    }
                } else {
                    None
                };
                let plain = expanded.unwrap_or_else(|| msg.text.clone());
                // The language gate: the garble writes the one buffer the line, `arg1` and the
                // bubble read, as `0x49a870` fills `[ebp-0xd0c]` once (`0x49a9f0` copy, `0x49aa7c`
                // garble) and never reads the wire text again.
                let language = langs.effective_language(msg.chat_type, msg.language);
                let mut text = langs.garble(language, &plain);

                // The text filters, on the garbled buffer as in `0x49a870`: spam first, then the
                // mask, so a dropped line consumes no mask indices (the phase is global).
                if text_filter.should_drop(msg.chat_type, msg.chat_tag, langs.is_gm(), &text) {
                    // Nothing shows in its place: the not-a-whisper leg (`0x49ab33`) jumps to the
                    // epilogue `0x49afd7`. A whisper with a non-zero report guid also sends
                    // `CMSG_CHAT_FILTERED` (`0x331`, the `u64` guid, `0x49ab50`) in the reference;
                    // not sent here, as vmangos leaves the opcode unhandled (`Opcodes.cpp:932`).
                    debug!(
                        "chat: spam filter dropped [{:#04x}] {:?}",
                        msg.chat_type, text
                    );
                    continue;
                }
                text_filter.mask_chat(msg.chat_type, &mut text);
                let text = text;
                let mut event = ChatEvent {
                    kind,
                    text: text.clone(),
                    sender: name.unwrap_or_else(|| {
                        if needs_name(msg.chat_type) && msg.sender_guid != 0 {
                            "Unknown".to_string()
                        } else {
                            String::new()
                        }
                    }),
                    // The effective language, not the wire's: narration types and GM mode force
                    // 0, which also drops the `[Language]` header.
                    language: language_name(language).to_string(),
                    channel: channel_base,
                    flag: flag_of_tag(msg.chat_tag).to_string(),
                    ..Default::default()
                };
                channels.stamp_channel(&mut event);
                route(&mut script, &mut windows, &event);
                // The bubble gets the same text: the reference spawns it in this path (`0x49acd9`).
                if let Some(kind) = event.kind {
                    speaker
                        .bubbles
                        .push(&speaker.bubble_cfg, msg.sender_guid, kind, &text);
                }
                // The gesture reads the raw type and `plain`, not the garbled text: the selector is
                // in the parser (`0x49d560`, `0x49d820`-`0x49d8ae`) on the buffer `0x49dbc2` hands
                // `0x49a870`, so a Horde `lol` laughs for every observer.
                if let Some(gesture) =
                    crate::creature_anim::select_gesture(msg.chat_type, &plain, |n| {
                        script
                            .lua()
                            .globals()
                            .get::<String>(format!("LAUGH_WORD{n}"))
                            .ok()
                    })
                {
                    speaker.gestures.push(msg.sender_guid, gesture);
                }
            }
            Pending::Notice {
                notice,
                channel,
                a_guid,
                b_guid,
                tries,
            } => {
                let a = names.resolve(a_guid, &commands).map(str::to_string);
                let b = if b_guid != 0 {
                    names.resolve(b_guid, &commands).map(str::to_string)
                } else {
                    Some(String::new())
                };
                if (a.is_none() || b.is_none()) && tries < NAME_MAX_TRIES {
                    still.push(Pending::Notice {
                        notice,
                        channel,
                        a_guid,
                        b_guid,
                        tries: tries + 1,
                    });
                    continue;
                }
                // Not stamped: a guid-tail notice (a member joining or leaving, a kick, a
                // moderation change) has arg7-arg9 empty, so the stock filter
                // (`ChatFrame.lua:1374-1392`) finds no channel for it and it never prints; the
                // reference stamps it as it stamps speech.
                if let Some(event) = notice_event(
                    notice,
                    channel,
                    Some(a.unwrap_or_else(|| "Unknown".into())),
                    b,
                ) {
                    route(&mut script, &mut windows, &event);
                }
            }
            Pending::Addon {
                prefix,
                message,
                distribution,
                guid,
                tries,
            } => {
                let name = names.resolve(guid, &commands).map(str::to_string);
                if name.is_none() && tries < NAME_MAX_TRIES {
                    still.push(Pending::Addon {
                        prefix,
                        message,
                        distribution,
                        guid,
                        tries: tries + 1,
                    });
                    continue;
                }
                fire_addon_message(
                    &mut script,
                    prefix,
                    message,
                    distribution,
                    name.unwrap_or_else(|| "Unknown".into()),
                );
            }
            Pending::Roll {
                min,
                max,
                roll,
                guid,
                tries,
            } => {
                let name = names.resolve(guid, &commands).map(str::to_string);
                if name.is_none() && tries < NAME_MAX_TRIES {
                    still.push(Pending::Roll {
                        min,
                        max,
                        roll,
                        guid,
                        tries: tries + 1,
                    });
                    continue;
                }
                // `RANDOM_ROLL_RESULT`, "%s rolls %d (%d-%d)" (`GlobalStrings.lua:3290`).
                let name = name.unwrap_or_else(|| "Unknown".into());
                let line = {
                    use benilla_ui::strings::Arg;
                    keyed(
                        &globals(&script),
                        "RANDOM_ROLL_RESULT",
                        &[
                            Arg::S(&name),
                            Arg::D(i64::from(roll)),
                            Arg::D(i64::from(min)),
                            Arg::D(i64::from(max)),
                        ],
                    )
                };
                if let Some(text) = line {
                    route(
                        &mut script,
                        &mut windows,
                        &ChatEvent::text_only(ChatEventKind::System, text),
                    );
                }
            }
            Pending::XpGain {
                victim,
                total,
                bonus,
                tries,
            } => {
                // `victim == 0` is the unnamed form, with no name to wait for.
                let name = if victim == 0 {
                    None
                } else {
                    let resolved = names.resolve(victim, &commands).map(str::to_string);
                    if resolved.is_none() && tries < NAME_MAX_TRIES {
                        still.push(Pending::XpGain {
                            victim,
                            total,
                            bonus,
                            tries: tries + 1,
                        });
                        continue;
                    }
                    Some(resolved.unwrap_or_else(|| "Unknown".into()))
                };
                let line = xp_gain_line(name.as_deref(), total, bonus, &globals(&script));
                if let Some(text) = line {
                    route(
                        &mut script,
                        &mut windows,
                        &ChatEvent::text_only(ChatEventKind::CombatXpGain, text),
                    );
                }
            }
            Pending::HonorGain {
                victim,
                honor,
                rank,
                tries,
            } => {
                // `victim == 0` is the award form: no name, no rank title.
                let name = if victim == 0 {
                    None
                } else {
                    let resolved = names.resolve(victim, &commands).map(str::to_string);
                    if resolved.is_none() && tries < NAME_MAX_TRIES {
                        still.push(Pending::HonorGain {
                            victim,
                            honor,
                            rank,
                            tries: tries + 1,
                        });
                        continue;
                    }
                    resolved
                };
                // The title's side is the victim's and its gender ours (`0x625270` genders against
                // the local player). No name record means side 0: a creature, where only a racial
                // leader is ranked (19, "Leader" either side, `HonorMgr.cpp:1075-1076`).
                let team = names
                    .player_traits(victim)
                    .and_then(|(race, _, _)| crate::ui_unit::race_faction_group(race))
                    .map_or(0, |group| u8::from(group == "Alliance"));
                let female = self_guid
                    .0
                    .and_then(|g| names.player_traits(g))
                    .is_some_and(|(_, _, gender)| gender == 1);
                let title = (victim != 0).then(|| script.pvp_rank_title(rank, team, female));
                let name = (victim != 0).then(|| name.unwrap_or_else(|| "Unknown".into()));
                let line = honor_gain_line(
                    name.as_deref(),
                    title.flatten().as_deref(),
                    honor,
                    &globals(&script),
                );
                if let Some(text) = line {
                    route(
                        &mut script,
                        &mut windows,
                        &ChatEvent::text_only(ChatEventKind::CombatHonorGain, text),
                    );
                }
            }
            Pending::TextEmote {
                performer,
                text_id,
                target,
                tries,
            } => {
                // Both names ask-once: the performer's, and ours for the "is the target me?" test.
                let performer_name = names.resolve(performer, &commands).map(str::to_string);
                let your_name = self_guid
                    .0
                    .and_then(|g| names.resolve(g, &commands).map(str::to_string));
                if (performer_name.is_none() || your_name.is_none()) && tries < NAME_MAX_TRIES {
                    still.push(Pending::TextEmote {
                        performer,
                        text_id,
                        target,
                        tries: tries + 1,
                    });
                    continue;
                }
                // No performer name, no line (not "Unknown"): the reference bails when its
                // player-only name cache misses (`0x49b28c`); the animation rode `EmoteMessage`.
                let (Some(performer_name), Some(cat)) = (performer_name, emote_texts.as_deref())
                else {
                    debug!("chat: text emote {text_id} from {performer:#x} has no sentence source");
                    continue;
                };
                let event = text_emote_event(
                    &cat.0,
                    text_id,
                    &EmoteLine {
                        performer: &performer_name,
                        performer_is_you: self_guid.0 == Some(performer),
                        // Sex 1 is female, where the reference reads it (`record+0x13c`).
                        performer_female: names
                            .player_traits(performer)
                            .is_some_and(|(_, _, sex)| sex == 1),
                        target: &target,
                        your_name: your_name.as_deref().unwrap_or_default(),
                    },
                );
                // No sentence is a real outcome: `/sit` prints nothing.
                let Some(event) = event else { continue };
                route(&mut script, &mut windows, &event);
            }
            Pending::Combat(mut line) => {
                // Both names, ask-once within `tries`; guid 0 is already filled (`/chattest`).
                let mut wait = false;
                for (guid, slot) in [(line.subject, 0usize), (line.object, 1usize)] {
                    if guid == 0 {
                        continue;
                    }
                    match super::combat::object_name(
                        guid,
                        guids.0.get(&guid).and_then(|e| stores.get(*e).ok()),
                        &names,
                        &commands,
                    ) {
                        Some(name) if slot == 0 => line.fills.attacker = name,
                        Some(name) => line.fills.victim = name,
                        None => wait = true,
                    }
                }
                // An item the server answered unknown does not wait: the line composes with what
                // the arm left in `named` (the reference falls back to `"UKNOWNOBJECT"`).
                match line.named {
                    super::combat::Named::Ready => {}
                    super::combat::Named::Item(entry) => {
                        match items.template(entry, 0, &commands).map(|t| t.name.clone()) {
                            Some(name) => line.fills.named = name,
                            None if !items.template_answered_unknown(entry) => wait = true,
                            None => {}
                        }
                    }
                    super::combat::Named::Unit(guid) => {
                        match super::combat::object_name(
                            guid,
                            guids.0.get(&guid).and_then(|e| stores.get(*e).ok()),
                            &names,
                            &commands,
                        ) {
                            Some(name) => line.fills.named = name,
                            None => wait = true,
                        }
                    }
                }
                if wait {
                    if line.tries < NAME_MAX_TRIES {
                        line.tries += 1;
                        still.push(Pending::Combat(line));
                    }
                    continue;
                }
                let composed =
                    super::combat::compose_line(&script, line.family, line.variant, &line.fills);
                if let Some(text) = composed {
                    route(
                        &mut script,
                        &mut windows,
                        &ChatEvent::text_only(line.kind, text),
                    );
                }
            }
            Pending::Discovery { area, xp } => {
                // The toast fires every time; the chat line only rides XP.
                let (toast, line) = {
                    let get = globals(&script);
                    (
                        exploration_toast(&area, &get),
                        (xp > 0)
                            .then(|| exploration_line(&area, xp, &get))
                            .flatten(),
                    )
                };
                if let Some(toast) = toast {
                    script.fire_event(
                        "UI_INFO_MESSAGE",
                        vec![benilla_ui::script::ScriptValue::Str(toast)],
                    );
                }
                if let Some(text) = line {
                    route(
                        &mut script,
                        &mut windows,
                        &ChatEvent::text_only(ChatEventKind::System, text),
                    );
                }
            }
        }
    }
    log.pending = still;
}

/// Whether a wire chat type carries a player guid to name (not a monster's inline name).
fn needs_name(chat_type: u8) -> bool {
    use benilla_protocol::messages as m;
    matches!(
        chat_type,
        m::CHAT_MSG_SAY
            | m::CHAT_MSG_PARTY
            | m::CHAT_MSG_RAID
            | m::CHAT_MSG_GUILD
            | m::CHAT_MSG_OFFICER
            | m::CHAT_MSG_YELL
            | m::CHAT_MSG_WHISPER
            | m::CHAT_MSG_WHISPER_INFORM
            | m::CHAT_MSG_EMOTE
            | m::CHAT_MSG_CHANNEL
            | m::CHAT_MSG_AFK
            | m::CHAT_MSG_DND
            | m::CHAT_MSG_IGNORED
            | m::CHAT_MSG_RAID_LEADER
            | m::CHAT_MSG_RAID_WARNING
            | m::CHAT_MSG_BATTLEGROUND
            | m::CHAT_MSG_BATTLEGROUND_LEADER
    )
}

/// `/played` for scripts: each `RequestTimePlayed()` sends one empty `CMSG_PLAYED_TIME`, and the
/// answer fires `TIME_PLAYED_MSG`. The stock `ChatFrame_OnEvent` prints that event as the
/// `TIME_PLAYED_TOTAL` and `TIME_PLAYED_LEVEL` lines (`ChatFrame.lua:1279`), so the English lines
/// `ui_chat::net::played_time` also prints are a duplicate.
pub(crate) fn played_time_bridge(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    commands: Res<NetCommands>,
    answer: Option<ResMut<crate::net::PlayedTimeAnswer>>,
) {
    let Some(mut script) = script else {
        return;
    };
    for _ in 0..script.take_played_time_asks() {
        let _ = commands.0.send(crate::net::ClientCommand::PlayedTime);
    }
    let Some(mut answer) = answer else {
        return;
    };
    if let Some((total, level)) = answer.0.take() {
        script.fire_event(
            "TIME_PLAYED_MSG",
            vec![
                benilla_ui::script::ScriptValue::Int(i64::from(total)),
                benilla_ui::script::ScriptValue::Int(i64::from(level)),
            ],
        );
    }
}
