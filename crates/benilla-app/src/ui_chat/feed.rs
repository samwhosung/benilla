//! Every inbound chat source becomes one [`ChatEvent`] here, its names resolved ask-once through
//! [`crate::names::NameCache`] with bounded re-checks, then routed by [`super::frames::route`].

use bevy::prelude::*;

use benilla_protocol::messages::{
    channel_notice, ChannelNoticeTail, ChatMessage, LevelUpInfo, MACRO_EXPANDED_TYPES,
};

use benilla_assets::{LockRecover, WorldAssets};
use benilla_formats::{EmoteLine, EmoteTextCatalog};

use crate::names::NameCache;
use crate::net::{GuidIndex, NetCommands, ObjectStore, SelfGuid};

use super::edit::ChannelState;
use super::event::{flag_of_tag, kind_of_wire, language_name, ChatEvent, ChatEventKind};
use super::frames::{route, ChatWindows};
use crate::query_cache::Ask;

/// Frames a line waits on a name, about 2 s at 60 fps, before it gives up.
const NAME_MAX_TRIES: u16 = 120;

/// The text-emote sentence tables, read once off the patch chain.
#[derive(Resource)]
pub(crate) struct EmoteTexts(pub(crate) EmoteTextCatalog);

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
    /// A `UNIT_COMBAT`, fired at its place among the lines its handler raised beside it.
    UnitCombat(crate::ui_unit::UnitCombatFeedback),
    /// The attack handler's `PLAYER_ENTER_COMBAT` or `PLAYER_LEAVE_COMBAT` (`0x6256ff`,
    /// `0x625778`), argless, fired at its place among the lines its packet raised.
    CombatEdge { entered: bool },
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
    /// An XP award; `victim` 0 is the unnamed form. `group` is the packet's float, 1.0 for the
    /// forms that never read one.
    XpGain {
        victim: u64,
        total: u32,
        base: u32,
        group: f32,
    },
    /// An XP loss, `total` as the packet carries it.
    XpLoss { total: u32 },
    /// An honor award; `rank` (0 none) becomes a title at the drain, on the victim's `side` (0
    /// Horde, 1 Alliance), read off its object at the packet.
    HonorGain {
        victim: u64,
        honor: i32,
        rank: u8,
        side: u8,
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
    /// "You are now <rank> with <faction>." (`0x62c670`), parked by the wire handler in packet
    /// order and resolved against the VM's strings at the drain.
    Standing {
        faction: String,
        rank: u8,
        female: bool,
    },
    /// One combat-log line, classified at the packet; it waits only on an item's name, as the
    /// reference's deferred queue (`0xc4e208`) does, until the cache answers.
    Combat(Box<super::combat::PendingCombat>),
    /// A ready event (client-composed lines; name-carrying notices).
    Event(ChatEvent),
    /// A line whose event fired where it was written ([`ChatLog::write_combat_now`]): its tee to
    /// the log files alone, in order with the lines around it.
    Tee { combat: bool, line: String },
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
        text: cat.compose(text_id, line)?,
        sender: line.performer.to_string(),
        ..Default::default()
    })
}

/// The chat items the feeds queue and [`feed_chat`] drains. World broadcasts wait apart, as their
/// resolve needs more than [`feed_chat`] can hold at Bevy's `SystemParam` limit;
/// `feed_broadcasts` resolves them onto `pending` earlier in the same frame.
///
/// An append-only sink shared through `&self`: a system that lends the call host appends here
/// while a call the host applies inside its Lua appends a refusal's line too, as the reference's
/// verb writes the chat frame in the call. One lock keeps every append in the order it was made.
#[derive(Resource, Default)]
pub(crate) struct ChatLog(std::sync::Mutex<ChatQueue>);

/// What [`ChatLog`] holds.
#[derive(Default)]
struct ChatQueue {
    pending: Vec<Queued>,
    /// Every item ever queued, the next one's [`Queued::seq`]: the drain's marks read it after
    /// each handler call ([`crate::ui_unit::wire_order`]).
    pushed: u64,
    broadcasts: Vec<super::broadcast::Broadcast>,
    /// A level-up's gains for `ui_unit`'s `PLAYER_LEVEL_UP` fire, matched by level: the fire
    /// follows a descriptor change and the gains a packet.
    level_up_gains: Vec<(LevelUpInfo, u32)>,
}

/// One item in the queue, with its place in the order everything was queued in.
struct Queued {
    seq: u64,
    /// Parked again by the drain, waiting for a name: out of the wire order from then on.
    parked: bool,
    item: Pending,
}

impl ChatQueue {
    fn push(&mut self, item: Pending) {
        self.pending.push(Queued {
            seq: self.pushed,
            parked: false,
            item,
        });
        self.pushed += 1;
    }
}

impl ChatLog {
    /// The queue, held for one step; a poisoned lock still holds whole items.
    fn q(&self) -> std::sync::MutexGuard<'_, ChatQueue> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Drop everything queued: the session's end. The count runs on, as the drain's marks read it.
    pub(crate) fn reset(&self) {
        let mut q = self.q();
        let pushed = q.pushed;
        *q = ChatQueue {
            pushed,
            ..ChatQueue::default()
        };
    }

    /// Take what is parked, for the drain.
    fn take_pending(&self) -> Vec<Queued> {
        std::mem::take(&mut self.q().pending)
    }

    /// Take what was queued since `mark` ([`Self::pushed`]), leaving what was queued before: the
    /// lines a notice the drain fires shows.
    fn take_pending_since(&self, mark: u64) -> Vec<Queued> {
        let mut q = self.q();
        let (since, before) = std::mem::take(&mut q.pending)
            .into_iter()
            .partition(|item| item.seq >= mark);
        q.pending = before;
        since
    }

    /// Park `still` again ahead of what was queued while the drain ran: the lines a call its
    /// handlers made appended meanwhile.
    fn park_again(&self, mut still: Vec<Queued>) {
        let mut q = self.q();
        still.append(&mut q.pending);
        q.pending = still;
    }

    /// How many items were ever queued: the next one's place in the order.
    pub(crate) fn pushed(&self) -> u64 {
        self.q().pushed
    }

    /// The place of the first item the drain fires in the order its packets came, one not parked
    /// for a name; `None` with none.
    pub(crate) fn first_live_seq(&self) -> Option<u64> {
        self.q().pending.iter().find(|q| !q.parked).map(|q| q.seq)
    }
}

impl ChatLog {
    /// Queue one world broadcast for [`super::broadcast::feed_broadcasts`]'s resolve pass.
    pub(crate) fn push_broadcast(&self, b: super::broadcast::Broadcast) {
        self.q().broadcasts.push(b);
    }

    /// How many broadcasts are waiting, the drain's early-out.
    pub(crate) fn broadcasts_pending(&self) -> usize {
        self.q().broadcasts.len()
    }

    /// Take the parked broadcasts, leaving the queue empty.
    pub(crate) fn take_broadcasts(&self) -> Vec<super::broadcast::Broadcast> {
        std::mem::take(&mut self.q().broadcasts)
    }

    /// Park a level-up's gains for the `PLAYER_LEVEL_UP` fire.
    pub(crate) fn push_level_up_gains(&self, info: &LevelUpInfo, talent_points: u32) {
        self.q().level_up_gains.push((*info, talent_points));
    }

    /// Take the parked gains for `level`; with none (a GM's level write), the caller fires zeros.
    pub(crate) fn take_level_up_gains(&self, level: u32) -> Option<(LevelUpInfo, u32)> {
        let mut q = self.q();
        let i = q
            .level_up_gains
            .iter()
            .position(|(l, _)| l.level == level)?;
        Some(q.level_up_gains.remove(i))
    }
}

impl ChatLog {
    /// Queue a decoded wire line (`SMSG_MESSAGECHAT`).
    pub(crate) fn push_wire(&self, msg: ChatMessage) {
        self.q().push(Pending::Wire { msg, tries: 0 });
    }

    /// Queue a client-composed event: loot, quest and system lines, the `/played` answer.
    pub(crate) fn push_event(&self, event: ChatEvent) {
        self.q().push(Pending::Event(event));
    }

    /// Show a client-composed line inside the verb that adds it, as `0x49a870` does: its event
    /// fires now, and only its log-file tee waits for the drain, in order with the lines around
    /// it. For a line whose composition reads no default language.
    pub(crate) fn show_now(&self, script: &mut benilla_ui::script::UiScript, event: &ChatEvent) {
        let Some(kind) = event.kind else {
            return;
        };
        let line = super::frames::compose(event, kind, "", &|key| {
            script.lua().globals().get::<String>(key).ok()
        });
        if let Some(line) = line {
            self.q().push(Pending::Tee {
                combat: kind.is_combat_log(),
                line,
            });
        }
        script.fire_event(super::event::event_name(kind), event.script_args());
    }

    /// Queue one combat-log line, classified at the packet by [`super::combat`], for its names.
    pub(crate) fn push_combat(&self, line: super::combat::PendingCombat) {
        self.q().push(Pending::Combat(Box::new(line)));
    }

    /// Queue a decoded `SMSG_CHANNEL_NOTIFY`; a guid tail waits for its names.
    pub(crate) fn push_channel_notice(
        &self,
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
            self.q().push(Pending::Notice {
                notice: notice_byte,
                channel,
                a_guid,
                b_guid,
                tries: 0,
            });
        } else {
            if let Some(event) = notice_event(notice_byte, channel, name, None) {
                self.q().push(Pending::Event(event));
            }
        }
    }

    /// Queue an inbound addon line for its sender's name. The text splits on its first tab, and
    /// with none it is all prefix (`0x49a8d0`); `distribution` names PARTY, RAID, GUILD and
    /// BATTLEGROUND, any other type `"UNKNOWN"` (`0x49aff4`).
    pub(crate) fn push_addon(&self, text: &str, chat_type: u8, guid: u64) {
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
        self.q().push(Pending::Addon {
            prefix,
            message,
            distribution,
            guid,
            tries: 0,
        });
    }

    /// Queue a `/random` broadcast (`MSG_RANDOM_ROLL`) for the roller-name resolve.
    pub(crate) fn push_roll(&self, min: u32, max: u32, roll: u32, guid: u64) {
        self.q().push(Pending::Roll {
            min,
            max,
            roll,
            guid,
            tries: 0,
        });
    }

    /// Queue an XP gain's line (`SMSG_LOG_XPGAIN`); `victim` 0 is the unnamed form.
    pub(crate) fn push_xp_gain(&self, victim: u64, total: u32, base: u32, group: f32) {
        self.q().push(Pending::XpGain {
            victim,
            total,
            base,
            group,
        });
    }

    /// Queue an XP loss's line, an `SMSG_LOG_XPGAIN` naming you as the victim (`0x637f70`).
    pub(crate) fn push_xp_loss(&self, total: u32) {
        self.q().push(Pending::XpLoss { total });
    }

    /// Queue a `UNIT_COMBAT` behind what is already queued, so it fires in the order its handler
    /// raised it against its lines.
    pub(crate) fn push_unit_combat(&self, ev: crate::ui_unit::UnitCombatFeedback) {
        self.q().push(Pending::UnitCombat(ev));
    }

    /// Queue the active player's combat edge, `entered` for `PLAYER_ENTER_COMBAT`, behind what is
    /// already queued, so it fires in the order its packet handler signalled it against the lines
    /// and `UNIT_COMBAT`s around it.
    pub(crate) fn push_combat_edge(&self, entered: bool) {
        self.q().push(Pending::CombatEdge { entered });
    }

    /// Write a combat-log line no name waits on where its formatter runs: `AddCombatLogLine
    /// 0x626850` fires its `CHAT_MSG_*` at once (`0x49b1b0` → `0x49b0b0`), so the line's event and
    /// then its `COMBAT_TEXT_UPDATE` word come back for the caller to signal in place, and only
    /// the log-file tee waits for the drain. Nothing for a missing template, which writes neither.
    pub(crate) fn write_combat_now(
        &self,
        script: &benilla_ui::script::UiScript,
        line: &super::combat::PendingCombat,
    ) -> Vec<(&'static str, Vec<benilla_ui::script::ScriptValue>)> {
        let Some(text) =
            super::combat::compose_line(script, line.family, line.variant, &line.fills)
        else {
            return Vec::new();
        };
        self.q().push(Pending::Tee {
            combat: line.kind.is_combat_log(),
            line: text.clone(),
        });
        let event = ChatEvent::text_only(line.kind, text);
        let mut out = vec![(super::event::event_name(line.kind), event.script_args())];
        if let Some(word) = &line.word {
            out.push((
                "COMBAT_TEXT_UPDATE",
                crate::ui_unit::combat_text_args(
                    word.message_type,
                    line.word_data(word),
                    word.extra.clone(),
                ),
            ));
        }
        out
    }

    /// Queue an honor award's line (`SMSG_PVP_CREDIT`); victim 0 is a bonus or objective award.
    pub(crate) fn push_pvp_credit(&self, honor: i32, victim: u64, rank: u8, side: u8) {
        // Both forks park: the template resolves at the drain.
        self.q().push(Pending::HonorGain {
            victim,
            honor,
            rank,
            side,
        });
    }

    /// Queue a text emote (`SMSG_TEXT_EMOTE`) for its performer's name, then its sentence.
    pub(crate) fn push_text_emote(&self, performer: u64, text_id: u32, target: String) {
        self.q().push(Pending::TextEmote {
            performer,
            text_id,
            target,
            tries: 0,
        });
    }

    /// Queue an area discovery: the `ERR_ZONE_EXPLORED` toast on every packet, never chat, and the
    /// `ERR_ZONE_EXPLORED_XP` system line when `xp > 0`.
    pub(crate) fn push_exploration(&self, area_name: &str, xp: u32) {
        self.q().push(Pending::Discovery {
            area: area_name.to_string(),
            xp,
        });
    }

    /// Queue a reputation rank line (`FACTION_STANDING_CHANGED`) for the drain to fill.
    pub(crate) fn push_standing(&self, faction: &str, rank: u8, female: bool) {
        self.q().push(Pending::Standing {
            faction: faction.to_string(),
            rank,
            female,
        });
    }

    /// Drop every pending item on disconnect.
    pub(crate) fn clear_session(&self) {
        self.q().pending.clear();
    }

    /// What is parked, in order, by kind: the order a packet batch left the lines in.
    #[cfg(test)]
    pub(crate) fn pending_kinds(&self) -> Vec<&'static str> {
        self.q()
            .pending
            .iter()
            .map(|q| &q.item)
            .map(|p| match p {
                Pending::Wire { .. } => "wire",
                Pending::Notice { .. } => "notice",
                Pending::Addon { .. } => "addon",
                Pending::Roll { .. } => "roll",
                Pending::XpGain { .. } => "xp",
                Pending::XpLoss { .. } => "xp loss",
                Pending::UnitCombat(_) => "unit combat",
                Pending::CombatEdge { entered: true } => "PLAYER_ENTER_COMBAT",
                Pending::CombatEdge { entered: false } => "PLAYER_LEAVE_COMBAT",
                Pending::HonorGain { .. } => "honor",
                Pending::TextEmote { .. } => "emote",
                Pending::Discovery { .. } => "discovery",
                Pending::Standing { .. } => "standing",
                Pending::Combat(_) => "combat",
                Pending::Event(_) => "event",
                Pending::Tee { .. } => "tee",
            })
            .collect()
    }

    /// The parked combat lines and `UNIT_COMBAT`s in queue order: a line as its family's stem, a
    /// `UNIT_COMBAT` as `"UNIT_COMBAT <action>"`.
    #[cfg(test)]
    pub(crate) fn pending_combat_order(&self) -> Vec<String> {
        self.q()
            .pending
            .iter()
            .map(|q| &q.item)
            .filter_map(|p| match p {
                Pending::Combat(line) => Some(line.family.stem.to_string()),
                Pending::UnitCombat(ev) => Some(format!("UNIT_COMBAT {}", ev.action)),
                _ => None,
            })
            .collect()
    }

    /// The parked `UNIT_COMBAT`s, in order.
    #[cfg(test)]
    pub(crate) fn pending_unit_combat(&self) -> Vec<crate::ui_unit::UnitCombatFeedback> {
        self.q()
            .pending
            .iter()
            .map(|q| &q.item)
            .filter_map(|p| match p {
                Pending::UnitCombat(ev) => Some(*ev),
                _ => None,
            })
            .collect()
    }

    /// The parked XP lines as `(victim, total, base, group)`, in order; victim 0 is the unnamed
    /// form.
    #[cfg(test)]
    pub(crate) fn pending_xp(&self) -> Vec<(u64, u32, u32, f32)> {
        self.q()
            .pending
            .iter()
            .map(|q| &q.item)
            .filter_map(|p| match *p {
                Pending::XpGain {
                    victim,
                    total,
                    base,
                    group,
                } => Some((victim, total, base, group)),
                _ => None,
            })
            .collect()
    }

    /// The parked honor credits as `(victim, honor, rank, side)`, in order.
    #[cfg(test)]
    pub(crate) fn pending_honor(&self) -> Vec<(u64, i32, u8, u8)> {
        self.q()
            .pending
            .iter()
            .map(|q| &q.item)
            .filter_map(|p| match *p {
                Pending::HonorGain {
                    victim,
                    honor,
                    rank,
                    side,
                } => Some((victim, honor, rank, side)),
                _ => None,
            })
            .collect()
    }

    /// The parked combat lines, in order.
    #[cfg(test)]
    pub(crate) fn pending_combat(&self) -> Vec<super::combat::PendingCombat> {
        self.q()
            .pending
            .iter()
            .map(|q| &q.item)
            .filter_map(|p| match p {
                Pending::Combat(line) => Some((**line).clone()),
                _ => None,
            })
            .collect()
    }

    /// The parked rank lines as `(faction, rank, female)`, in order.
    #[cfg(test)]
    pub(crate) fn pending_standing(&self) -> Vec<(String, u8, bool)> {
        self.q()
            .pending
            .iter()
            .map(|q| &q.item)
            .filter_map(|p| match p {
                Pending::Standing {
                    faction,
                    rank,
                    female,
                } => Some((faction.clone(), *rank, *female)),
                _ => None,
            })
            .collect()
    }

    /// The parked addon lines as `(prefix, message, distribution)`.
    #[cfg(test)]
    pub(crate) fn pending_addons(&self) -> Vec<(String, String, String)> {
        self.q()
            .pending
            .iter()
            .map(|q| &q.item)
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
        self.q()
            .pending
            .iter()
            .map(|q| &q.item)
            .filter(|p| !matches!(p, Pending::Addon { .. }))
            .count()
    }

    /// The text of every composed line waiting to render, in order.
    #[cfg(test)]
    pub(crate) fn pending_lines(&self) -> Vec<String> {
        self.pending_events().into_iter().map(|e| e.text).collect()
    }

    /// Every composed event waiting to render, in order.
    #[cfg(test)]
    pub(crate) fn pending_events(&self) -> Vec<ChatEvent> {
        self.q()
            .pending
            .iter()
            .map(|q| &q.item)
            .filter_map(|p| match p {
                Pending::Event(e) => Some(e.clone()),
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

/// The active player's rest state (`PLAYER_BYTES_2` byte 3, `0x637b0c`-`0x637b12`) and that
/// state's `Exhaustion.dbc` row, (name, XP factor), when the store holds one.
pub(super) struct RestState<'a> {
    pub state: u8,
    pub row: Option<(&'a str, f64)>,
}

/// `|x| < 2^-22`, the XP factor tests' epsilon (`[0x8029d4]`).
const XP_FACTOR_EPSILON: f64 = 1.0 / 4_194_304.0;

/// The XP line, `0x637ac0` (a named kill) and `0x637cd0` (the plain forms). No victim is
/// `COMBATLOG_XPGAIN_FIRSTPERSON_UNNAMED`, which no float reaches: its callers pass 1.0
/// (`0x637f90`) or find the victim they stored the gain on. A named kill needs the active player
/// (`0x637b06`; `rest` `None` writes nothing), and is plain `COMBATLOG_XPGAIN_FIRSTPERSON` unless
/// its rest state has a row whose factor is neither 0 nor 1 (`0x637b3e`-`0x637b6a`). Then it is
/// that state's key in the table `0x864330`, `COMBATLOG_XPGAIN_EXHAUSTION<state>`, with the
/// signed bonus, `total - base` prefixed `+` unless negative (`0x637c07`-`0x637c25`), and the
/// row's name as the state word (`0x637c31`). The table has no entry 0 and ends at 5: a row there
/// writes nothing. State 3's entry names the global `NULL`, which 1.12 does not define: `GetText`
/// answers the empty string for it (`0x703c02`) and `0x637bef` tests only for null, so the
/// reference formats that line from an empty template, which `keyed` drops. No shipped row
/// reaches it: `Exhaustion.dbc` gives state 3 a factor of 1, the plain form.
///
/// The packet's group float ([`group_split`]) appends `_GROUP` or `_RAID` to the named key and
/// adds the difference as the last number (`0x637b70`-`0x637bd5`, `0x637d33`-`0x637d67`); a
/// rested kill compares on its base XP.
pub(super) fn xp_gain_line(
    victim: Option<&str>,
    total: u32,
    base: u32,
    group: f32,
    rest: Option<RestState>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    use benilla_ui::strings::Arg;
    let Some(name) = victim else {
        return keyed(
            get,
            "COMBATLOG_XPGAIN_FIRSTPERSON_UNNAMED",
            &[Arg::D(i64::from(total))],
        );
    };
    let rest = rest?;
    let plain = || {
        let (suffix, diff) = group_split(total, group);
        let named = [Arg::S(name), Arg::D(i64::from(total))];
        match suffix {
            None => keyed(get, "COMBATLOG_XPGAIN_FIRSTPERSON", &named),
            Some(suffix) => keyed(
                get,
                &format!("COMBATLOG_XPGAIN_FIRSTPERSON{suffix}"),
                &[named[0], named[1], Arg::D(i64::from(diff))],
            ),
        }
    };
    let Some((state_word, factor)) = rest.row else {
        return plain();
    };
    if factor.abs() < XP_FACTOR_EPSILON || (factor - 1.0).abs() < XP_FACTOR_EPSILON {
        return plain();
    }
    let key = match rest.state {
        1 => "COMBATLOG_XPGAIN_EXHAUSTION1",
        2 => "COMBATLOG_XPGAIN_EXHAUSTION2",
        3 => "NULL",
        4 => "COMBATLOG_XPGAIN_EXHAUSTION4",
        5 => "COMBATLOG_XPGAIN_EXHAUSTION5",
        _ => return None,
    };
    // `sub`, `jns`: the 32-bit difference's sign.
    let bonus = total.wrapping_sub(base) as i32;
    let sign = if bonus >= 0 { "+" } else { "" };
    let bonus = format!("{sign}{bonus}");
    let (suffix, diff) = group_split(base, group);
    let args = [
        Arg::S(name),
        Arg::D(i64::from(total)),
        Arg::S(&bonus),
        Arg::S(state_word),
    ];
    match suffix {
        None => keyed(get, key, &args),
        Some(suffix) => keyed(
            get,
            &format!("{key}{suffix}"),
            &[args[0], args[1], args[2], args[3], Arg::D(i64::from(diff))],
        ),
    }
}

/// How the packet's group float moves a kill's XP (`0x637cdb`-`0x637d3a`, `0x637b70`-`0x637bbc`):
/// `__ftol(xp / group)` against `xp`, both as signed 32-bit values. Equal is no suffix; below is
/// `_GROUP`, the bonus; above is `_RAID`, the penalty. The second number is the absolute
/// difference (`cdq`, `xor`, `sub`), which wraps like the reference's.
fn group_split(xp: u32, group: f32) -> (Option<&'static str>, i32) {
    let xp = xp as i32;
    let scaled = benilla_ui::script::ftol(f64::from(xp) / f64::from(group));
    let diff = xp.wrapping_sub(scaled).wrapping_abs();
    match scaled.cmp(&xp) {
        std::cmp::Ordering::Equal => (None, 0),
        std::cmp::Ordering::Less => (Some("_GROUP"), diff),
        std::cmp::Ordering::Greater => (Some("_RAID"), diff),
    }
}

/// The XP loss line, `COMBATLOG_XPLOSS_FIRSTPERSON_UNNAMED` with the packet's XP negated as a
/// 32-bit signed value (`0x637f70`).
pub(super) fn xp_loss_line(total: u32, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    keyed(
        get,
        "COMBATLOG_XPLOSS_FIRSTPERSON_UNNAMED",
        &[benilla_ui::strings::Arg::D(i64::from(
            (total as i32).wrapping_neg(),
        ))],
    )
}

/// A combat line whose names are in: composed, routed, then the word its formatter raises after
/// it. A line that does not compose (no template, no school name) writes neither.
pub(super) fn write_combat_line(
    script: &mut benilla_ui::script::UiScript,
    windows: &mut ChatWindows,
    line: &super::combat::PendingCombat,
) {
    let Some(text) = super::combat::compose_line(script, line.family, line.variant, &line.fills)
    else {
        return;
    };
    route(script, windows, &ChatEvent::text_only(line.kind, text));
    if let Some(word) = &line.word {
        crate::ui_unit::fire_combat_text(
            script,
            word.message_type,
            line.word_data(word),
            word.extra.clone(),
        );
    }
}

/// An honor credit's line, routed, then the `HONOR_GAINED` word `0x625270` raises after it
/// (`0x62539d`, `0x6253e4`), signed: the stock handler prefixes "+" only to a positive number
/// (`Blizzard_CombatText.lua:234`). A missing template writes neither (`0x6253ee`).
pub(super) fn write_honor_line(
    script: &mut benilla_ui::script::UiScript,
    windows: &mut ChatWindows,
    victim: Option<&str>,
    rank_title: Option<&str>,
    honor: i32,
) {
    let Some(text) = honor_gain_line(victim, rank_title, honor, &globals(script)) else {
        return;
    };
    route(
        script,
        windows,
        &ChatEvent::text_only(ChatEventKind::CombatHonorGain, text),
    );
    crate::ui_unit::fire_combat_text(script, "HONOR_GAINED", Some(honor.to_string()), None);
}

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

/// Deliver one built event: a `YOU_JOINED` confirms its slot before the render, and a notice that
/// drops its slot frees it after, so the line keeps its number and colour ("Left Channel: [2.
/// General - Elwynn Forest]") and a handler's `GetChannelName` still finds it. The reference sets
/// its drop flag for `YOU_LEFT` (`0x49c115`, skipped on the suspended leg `0x49c0e9`),
/// `WRONG_PASSWORD` (`0x49c13c`), `BANNED` and `WRONG_FACTION` (`0x49c446`, which `BANNED`'s arm
/// `0x49c39a` reaches through `0x49c43a`), `NOT_MEMBER` and `INVALID_NAME` (`0x49c46d`), fires the
/// event (`0x49c5b0` calling `0x49a870`), then frees the slot of the notice's name
/// (`0x49c5b5`-`0x49c5c2`, `0x49bbd0`). Both arms mirror the slots into the VM, whose
/// `GetChannelName` and `SendChatMessage` read them.
#[cfg(test)]
pub(super) fn deliver(
    script: &mut benilla_ui::script::UiScript,
    windows: &mut ChatWindows,
    channels: &mut ChannelState,
    event: &mut ChatEvent,
) {
    let dropping = deliver_before(script, channels, event);
    route(script, windows, event);
    deliver_after(script, channels, dropping);
}

/// The slots and the call host of [`feed_chat`], borrowed in turn: the host is lent around Lua
/// alone, and the slots are written between.
type SlotsAndHost<'w, 's> = ParamSet<
    'w,
    's,
    (
        ResMut<'static, ChannelState>,
        Option<crate::script_calls::ScriptCallHost<'w, 's>>,
    ),
>;

/// [`deliver`] with the host lent around the event alone: a join a handler makes takes its slot
/// through the host against the slots the confirm left, and the free after it leaves that join's
/// slot where it is, as the notify handler frees after its event (`0x49c5b0`, `0x49c5b5`).
fn deliver_hosted(
    script: &mut benilla_ui::script::UiScript,
    set: &mut SlotsAndHost,
    windows: &mut ChatWindows,
    event: &mut ChatEvent,
) {
    let dropping = deliver_before(script, &mut set.p0(), event);
    crate::script_calls::lent(script, set.p1().as_mut(), |script| {
        route(script, windows, event)
    });
    deliver_after(script, &mut set.p0(), dropping);
}

/// [`deliver`]'s half before the event: the confirm and the stamp. Returns the name whose slot
/// frees after the event.
fn deliver_before(
    script: &mut benilla_ui::script::UiScript,
    channels: &mut ChannelState,
    event: &mut ChatEvent,
) -> Option<String> {
    let notice = event.notice_byte().filter(|_| {
        matches!(
            event.kind,
            Some(ChatEventKind::ChannelNotice | ChatEventKind::ChannelNoticeUser)
        )
    });
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
        // The slot was taken when the join was sent; the notice only confirms it, which a
        // handler's `SendChatMessage` to it reads from this event on.
        match channels.confirm_slot(&event.channel) {
            Some(slot) => debug!(
                "chat: server confirms channel {:?} joined (slot {slot})",
                event.channel
            ),
            None => debug!("chat: {:?} joined with no slot to confirm", event.channel),
        }
        channels.mirror_into(script);
    }
    use channel_notice as n;
    let drops = match notice {
        Some(n::YOU_LEFT) => event.slot_state != Some(super::edit::SlotState::Suspended),
        Some(
            n::WRONG_PASSWORD | n::NOT_MEMBER | n::BANNED | n::WRONG_FACTION | n::INVALID_NAME,
        ) => true,
        _ => false,
    };
    // The wire name, kept before `stamp_channel` decorates arg4 with the slot number.
    let dropping = drops.then(|| event.channel.clone());
    // The channel renders numbered when its slot is known.
    channels.stamp_channel(event);
    dropping
}

/// [`deliver`]'s half after the event: the free of the slot the notice drops.
fn deliver_after(
    script: &mut benilla_ui::script::UiScript,
    channels: &mut ChannelState,
    dropping: Option<String>,
) {
    if let Some(name) = dropping {
        // Cleared in place, never compacted: slot 3 stays 3 when slot 2 empties.
        let freed = channels.free_slot(&name);
        debug!("chat: channel {name:?} dropped (slot {freed:?} now free)");
        channels.mirror_into(script);
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

/// `UNIT_FIELD_FLAGS` bit `0x20000000` (`PREVENT_ANIM`: Polymorph, Kidney Shot, Gouge, Sleep).
const UNIT_FLAG_PREVENT_ANIM: u32 = 0x2000_0000;

/// Whether the speaker's flags suppress the chat talk gesture. Only the handler's own copy of
/// the selector tests the bit (`0x49d7f7`, the one `0xa0` test of `0x20000000` in the image),
/// which runs when the sender's name is already cached (`tries == 0`); a line held for the name
/// query gestures from the deferred copies (`0x49cf2b`, `0x49d3bb`), which do not.
pub(super) fn gesture_prevented(tries: u16, speaker_flags: Option<u32>) -> bool {
    tries == 0 && speaker_flags.is_some_and(|f| f & UNIT_FLAG_PREVENT_ANIM != 0)
}

/// Drain [`ChatLog`]: resolve names (ask-once, bounded), build the events and [`route`] them, with
/// the field notices that waited for an item here fired among the items in their packets' order
/// ([`crate::ui_unit::wire_order`]).
pub(super) fn feed_chat(
    // The VM, the slots the channel notices write between their Lua, and the host its calls apply
    // through, whose selection `UNIT_COMBAT`'s `"target"` token reads between the queue's items.
    mut vm: crate::script_calls::VmWith<ResMut<'static, ChannelState>>,
    // `Option`: a chat-only harness has no field notices to fire among the items.
    (log, mut notices): (
        Res<ChatLog>,
        Option<crate::ui_unit::field_events::NoticeFiring>,
    ),
    mut windows: ResMut<ChatWindows>,
    names: Res<NameCache>,
    mut speaker: SpeakerEffects,
    commands: Res<NetCommands>,
    // The text-emote tables, and our guid for the "are you the performer?" test.
    emote_texts: Option<Res<EmoteTexts>>,
    // Our guid: `UNIT_COMBAT`'s `"player"` token.
    self_guid: Res<SelfGuid>,
    // The `$`-macro subject: monster and BG lines expand against their addressee's descriptors.
    guids: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    states: Res<crate::world_state::WorldStates>,
    // The language gate: the word pool, this character's fluency and the GM bit.
    langs: Res<super::language::ChatLanguages>,
    // Item names for `TRADESKILL_LOG`, `FEEDPET_LOG`, `ITEMENCHANTMENT*`, `SPELLDURABILITYDAMAGE`,
    // and with the game object records the names of a combat line's held objects.
    (items, gameobjects): (
        Res<crate::items::Items>,
        Res<crate::go_templates::GameObjectTemplates>,
    ),
    unit_names: crate::ui_unit::NameTables,
    // The spam and profanity filters, which run in the reference's display function `0x49a870`.
    text_filter: crate::text_filter::ChatTextFilter,
) {
    let (Some(script), set) = vm.parts() else {
        return;
    };
    // Each item, and whether the notices due before it are still to fire.
    let mut work: std::collections::VecDeque<(Queued, bool)> =
        log.take_pending().into_iter().map(|q| (q, true)).collect();
    if work.is_empty() && !notices.as_ref().is_some_and(|n| n.any_waiting()) {
        return;
    }
    let mut parked = Vec::new();
    // The waiting notices fire one at a time: a chat line one shows reaches the chat frames as it
    // fires (`0x496720` → `0x49a870`), so it shows here, ahead of the next notice and the next
    // item. The next notice fires if it comes before chat item `before`, or with `None` once the
    // items are done; its lines come back.
    let mut fire_next =
        |before: Option<u64>, script: &mut benilla_ui::script::UiScript, set: &mut SlotsAndHost| {
            let notices = notices.as_mut()?;
            let call = before.map(|seq| notices.call_of_chat(seq));
            let mark = log.pushed();
            let fired = crate::script_calls::lent(script, set.p1().as_mut(), |script| {
                notices.fire_next(script, call)
            });
            fired.then(|| log.take_pending_since(mark))
        };
    loop {
        let Some((queued, due)) = work.pop_front() else {
            // The notices left after every item's.
            match fire_next(None, script, set) {
                Some(shown) => work.extend(shown.into_iter().map(|q| (q, false))),
                None => break,
            }
            continue;
        };
        // The notices a packet wrote ahead of this item's, or in the same handler call: each one's
        // lines, then this item, which fires the next one first.
        if due {
            if let Some(shown) = fire_next(Some(queued.seq), script, set) {
                work.push_front((queued, true));
                for line in shown.into_iter().rev() {
                    work.push_front((line, false));
                }
                continue;
            }
        }
        let Queued { seq, item, .. } = queued;
        let mut still = Vec::new();
        // The token lister runs as `SignalUnitCombat` (`0x494600`) signals: a handler earlier in
        // the queue may have moved the target, its call applied inside the verb.
        let is_target = match &item {
            Pending::UnitCombat(ev) => set
                .p1()
                .as_mut()
                .and_then(|h| h.target())
                .is_some_and(|t| guids.0.get(&ev.guid) == Some(&t)),
            _ => false,
        };
        // A channel line's record, read as the composer reads it (`0x49aa1e`-`0x49aa48`) before
        // the line's Lua runs.
        let stamp = match &item {
            Pending::Wire { msg, .. } => msg
                .channel
                .as_deref()
                .and_then(|name| set.p0().stamp_of(name)),
            _ => None,
        };
        // A channel notice writes its slot around its event, outside the host's lend.
        let item = match item {
            Pending::Event(mut event) => {
                deliver_hosted(script, set, &mut windows, &mut event);
                None
            }
            Pending::Notice {
                notice,
                channel,
                a_guid,
                b_guid,
                tries,
            } => {
                let ask = Ask::at(tries == 0, &commands);
                let a = names.resolve_player(a_guid, ask).map(str::to_string);
                let b = if b_guid != 0 {
                    names.resolve_player(b_guid, ask).map(str::to_string)
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
                } else if let Some(mut event) = notice_event(
                    notice,
                    channel,
                    Some(a.unwrap_or_else(|| "Unknown".into())),
                    b,
                ) {
                    // Stamped against the channel list as it stands at delivery: the reference
                    // hands the composer `0x49a870` the notice's channel name, directly when the
                    // names are cached (`0x49c59f`-`0x49c5b0`), else from the queued line's node
                    // (`+0x2c`) in the name-query callback (`0x49cfb4`-`0x49cfc1`), and the
                    // composer looks it up among the joined channels (`0x49aa1e`-`0x49aa31`).
                    // Unstamped, arg7-arg9 are empty and the stock filter
                    // (`ChatFrame.lua:1374-1392`) drops the line.
                    deliver_hosted(script, set, &mut windows, &mut event);
                }
                None
            }
            item => Some(item),
        };
        let Some(item) = item else {
            parked.extend(still.into_iter().map(|item| Queued {
                seq,
                parked: true,
                item,
            }));
            continue;
        };
        crate::script_calls::lent(script, set.p1().as_mut(), |script| {
            match item {
                Pending::UnitCombat(ev) => {
                    crate::ui_unit::fire_unit_combat(
                        script,
                        &ev,
                        self_guid.0 == Some(ev.guid),
                        is_target,
                    );
                }
                Pending::CombatEdge { entered } => {
                    let combat_edge = if entered {
                        "PLAYER_ENTER_COMBAT"
                    } else {
                        "PLAYER_LEAVE_COMBAT"
                    };
                    script.fire_event(combat_edge, vec![]);
                }
                Pending::Tee { combat, line } => windows.logs.record(combat, &line),
                // Delivered above.
                Pending::Event(_) | Pending::Notice { .. } => {}
                Pending::Wire { msg, tries } => {
                    let kind = kind_of_wire(msg.chat_type);
                    if kind.is_none() {
                        warn!(
                            "chat: unmodeled wire type {:#04x} dropped: {:?}",
                            msg.chat_type, msg.text
                        );
                        return;
                    }
                    // A monster line carries its name inline; a player line resolves its guid.
                    let name = match &msg.sender_name {
                        Some(n) => Some(n.clone()),
                        None if needs_name(msg.chat_type) && msg.sender_guid != 0 => names
                            .resolve_player(msg.sender_guid, Ask::at(tries == 0, &commands))
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
                        return;
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
                            Ask::at(tries == 0, &commands),
                            unit_names.get(),
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
                        } else if subject_guid == 0
                            || names.resolve_unit(subject_guid, None, Ask::Read).is_some()
                        {
                            debug!(
                                "chat: dropping unexpandable [{:#04x}] {:?} (subject \
                             {subject_guid:#x})",
                                msg.chat_type, msg.text
                            );
                            return; // the reference prints no line
                        } else if tries < NAME_MAX_TRIES {
                            // `subject_for_guid` sent the name query: hold the raw line until it
                            // lands.
                            still.push(Pending::Wire {
                                msg,
                                tries: tries + 1,
                            });
                            return;
                        } else {
                            None // the second failure: show the raw text
                        }
                    } else {
                        None
                    };
                    let plain = expanded.unwrap_or_else(|| msg.text.clone());
                    // The language gate: the garble writes the one buffer the line, `arg1` and the
                    // bubble read, as `0x49a870` fills `[ebp-0xd0c]` once (`0x49a9f0` copy,
                    // `0x49aa7c` garble) and never reads the wire text again.
                    let language = langs.effective_language(msg.chat_type, msg.language);
                    let mut text = langs.garble(language, &plain);

                    // The text filters, on the garbled buffer as in `0x49a870`: spam first, then
                    // the mask, so a dropped line consumes no mask indices (the phase is global).
                    if text_filter.should_drop(msg.chat_type, msg.chat_tag, langs.is_gm(), &text) {
                        // Nothing shows in its place: the not-a-whisper leg (`0x49ab33`) jumps to
                        // the epilogue `0x49afd7`. A whisper with a non-zero report guid also sends
                        // `CMSG_CHAT_FILTERED` (`0x331`, the `u64` guid, `0x49ab50`) in the
                        // reference; not sent here, as vmangos leaves the opcode unhandled
                        // (`Opcodes.cpp:932`).
                        debug!(
                            "chat: spam filter dropped [{:#04x}] {:?}",
                            msg.chat_type, text
                        );
                        return;
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
                    if let Some(stamp) = stamp {
                        stamp.apply(&mut event);
                    }
                    route(script, &mut windows, &event);
                    // The bubble gets the same text: the reference spawns it in this path
                    // (`0x49acd9`).
                    if let Some(kind) = event.kind {
                        speaker
                            .bubbles
                            .push(&speaker.bubble_cfg, msg.sender_guid, kind, &text);
                    }
                    // The gesture reads the raw type and `plain`, not the garbled text: the
                    // selector is in the parser (`0x49d560`, `0x49d820`-`0x49d8ae`) on the buffer
                    // `0x49dbc2` hands `0x49a870`, so a Horde `lol` laughs for every observer. The
                    // immediate path alone refuses a speaker that cannot animate (`0x49d7f7`).
                    let speaker_flags = guids
                        .0
                        .get(&msg.sender_guid)
                        .and_then(|e| stores.get(*e).ok())
                        .map(|s| s.0.unit_flags());
                    if let Some(gesture) =
                        crate::creature_anim::select_gesture(msg.chat_type, &plain, |n| {
                            script
                                .lua()
                                .globals()
                                .get::<String>(format!("LAUGH_WORD{n}"))
                                .ok()
                        })
                        .filter(|_| !gesture_prevented(tries, speaker_flags))
                    {
                        speaker.gestures.push(msg.sender_guid, gesture);
                    }
                }
                Pending::Addon {
                    prefix,
                    message,
                    distribution,
                    guid,
                    tries,
                } => {
                    let name = names
                        .resolve_player(guid, Ask::at(tries == 0, &commands))
                        .map(str::to_string);
                    if name.is_none() && tries < NAME_MAX_TRIES {
                        still.push(Pending::Addon {
                            prefix,
                            message,
                            distribution,
                            guid,
                            tries: tries + 1,
                        });
                        return;
                    }
                    fire_addon_message(
                        script,
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
                    let name = names
                        .resolve_player(guid, Ask::at(tries == 0, &commands))
                        .map(str::to_string);
                    if name.is_none() && tries < NAME_MAX_TRIES {
                        still.push(Pending::Roll {
                            min,
                            max,
                            roll,
                            guid,
                            tries: tries + 1,
                        });
                        return;
                    }
                    // `RANDOM_ROLL_RESULT`, "%s rolls %d (%d-%d)" (`GlobalStrings.lua:3290`).
                    let name = name.unwrap_or_else(|| "Unknown".into());
                    let line = {
                        use benilla_ui::strings::Arg;
                        keyed(
                            &globals(script),
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
                            script,
                            &mut windows,
                            &ChatEvent::text_only(ChatEventKind::System, text),
                        );
                    }
                }
                Pending::XpGain {
                    victim,
                    total,
                    base,
                    group,
                } => {
                    // A victim's name never waits: each template names it through `GetUnitName`
                    // (`0x609210`, from `0x637ac0`), whose miss without `UNKNOWNOBJECT` is the
                    // literal "Unknown Being", where `GetObjectName`'s is `UKNOWNOBJECT`.
                    let name = (victim != 0).then(|| {
                        super::combat::unit_name(
                            victim,
                            guids.0.get(&victim).and_then(|e| stores.get(*e).ok()),
                            &names,
                            Ask::at(true, &commands),
                        )
                        .unwrap_or_else(|| script.unknown_object_text())
                    });
                    let state = self_guid
                        .0
                        .and_then(|g| guids.0.get(&g))
                        .and_then(|e| stores.get(*e).ok())
                        .map(|me| me.0.player_rest_state().unwrap_or(0));
                    let row = state.and_then(|s| script.exhaustion_row(s));
                    let rest = state.map(|state| RestState {
                        state,
                        row: row.as_ref().map(|(n, f)| (n.as_str(), *f)),
                    });
                    let line =
                        xp_gain_line(name.as_deref(), total, base, group, rest, &globals(script));
                    if let Some(text) = line {
                        route(
                            script,
                            &mut windows,
                            &ChatEvent::text_only(ChatEventKind::CombatXpGain, text),
                        );
                    }
                }
                Pending::XpLoss { total } => {
                    let line = xp_loss_line(total, &globals(script));
                    if let Some(text) = line {
                        route(
                            script,
                            &mut windows,
                            &ChatEvent::text_only(ChatEventKind::CombatXpGain, text),
                        );
                    }
                }
                Pending::HonorGain {
                    victim,
                    honor,
                    rank,
                    side,
                } => {
                    // `victim == 0` is the award form: no name, no rank title. A victim's name
                    // never waits (`GetUnitName`, `0x625387`), and its title genders against the
                    // active player (`0x612bf0`, `0x625374`), whose gender `0x5ed5b0` reads from
                    // `PLAYER_BYTES_3` byte 0; a dishonorable kill's template names no rank.
                    let name = (victim != 0).then(|| {
                        super::combat::unit_name(
                            victim,
                            guids.0.get(&victim).and_then(|e| stores.get(*e).ok()),
                            &names,
                            Ask::at(true, &commands),
                        )
                        .unwrap_or_else(|| script.unknown_object_text())
                    });
                    let female = self_guid
                        .0
                        .and_then(|g| guids.0.get(&g))
                        .and_then(|e| stores.get(*e).ok())
                        .and_then(|me| me.0.player_gender())
                        == Some(1);
                    let title = (victim != 0 && honor > 0)
                        .then(|| script.pvp_rank_title(rank, side, female));
                    write_honor_line(
                        script,
                        &mut windows,
                        name.as_deref(),
                        title.flatten().as_deref(),
                        honor,
                    );
                }
                Pending::TextEmote {
                    performer,
                    text_id,
                    target,
                    tries,
                } => {
                    // Both names, looked up as the packet's (`0x49dc59`): the performer's, and ours
                    // for the "is the target me?" test.
                    let ask = Ask::at(tries == 0, &commands);
                    let performer_name = names.resolve_player(performer, ask).map(str::to_string);
                    let your_name = self_guid
                        .0
                        .and_then(|g| names.resolve_player(g, ask).map(str::to_string));
                    if (performer_name.is_none() || your_name.is_none()) && tries < NAME_MAX_TRIES {
                        still.push(Pending::TextEmote {
                            performer,
                            text_id,
                            target,
                            tries: tries + 1,
                        });
                        return;
                    }
                    // No performer name, no line (not "Unknown"): the reference bails when its
                    // player-only name cache misses (`0x49b28c`); the animation rode
                    // `EmoteMessage`.
                    let (Some(performer_name), Some(cat)) =
                        (performer_name, emote_texts.as_deref())
                    else {
                        debug!(
                            "chat: text emote {text_id} from {performer:#x} has no sentence source"
                        );
                        return;
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
                    let Some(event) = event else { return };
                    route(script, &mut windows, &event);
                }
                Pending::Combat(mut line) => {
                    // Names looked up with a callback as the line's (`0x6265a7`, `0x6265e0`), and
                    // asked again only on its first drain; a line waits on an item alone, and as
                    // long as the cache takes to answer: the reference's list has no timer or
                    // count, and only the answer or the session's end removes an entry
                    // (`0x6293a0`, `0x6294cc`, `0x629a90`).
                    let ask = Ask::at(line.tries == 0, &commands);
                    let sources = super::combat::NameSources {
                        names: &names,
                        gameobjects: &gameobjects,
                        items: &items,
                    };
                    let ready = line.fill_names(
                        |guid| {
                            super::combat::logged_name(
                                guid,
                                guids.0.get(&guid).and_then(|e| stores.get(*e).ok()),
                                &sources,
                                ask,
                                script,
                            )
                        },
                        |entry| match items.template(entry, 0, ask).map(|t| t.name.clone()) {
                            Some(name) => super::combat::ItemName::Named(name),
                            None if items.template_answered_unknown(entry) => {
                                super::combat::ItemName::Unknown
                            }
                            None => super::combat::ItemName::Waiting,
                        },
                    );
                    if !ready {
                        line.tries = line.tries.saturating_add(1);
                        still.push(Pending::Combat(line));
                        return;
                    }
                    write_combat_line(script, &mut windows, &line);
                }
                Pending::Standing {
                    faction,
                    rank,
                    female,
                } => {
                    let line = crate::ui_reputation::standing_text(
                        &globals(script),
                        rank,
                        female,
                        &faction,
                    );
                    if let Some(text) = line {
                        route(
                            script,
                            &mut windows,
                            &ChatEvent::text_only(ChatEventKind::System, text),
                        );
                    }
                }
                Pending::Discovery { area, xp } => {
                    // The toast fires every time; the chat line only rides XP.
                    let (toast, line) = {
                        let get = globals(script);
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
                            script,
                            &mut windows,
                            &ChatEvent::text_only(ChatEventKind::System, text),
                        );
                    }
                }
            }
        });
        parked.extend(still.into_iter().map(|item| Queued {
            seq,
            parked: true,
            item,
        }));
    }
    log.park_again(parked);
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
    mut hosted_vm: crate::script_calls::HostedVm,
    commands: Res<NetCommands>,
    answer: Option<ResMut<crate::net::PlayedTimeAnswer>>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
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
    })
}
