//! Every inbound chat source becomes one [`ChatEvent`] here, its names resolved ask-once through
//! [`crate::names::NameCache`] with bounded re-checks, then routed by [`super::frames::route`].

use bevy::prelude::*;

use benilla_protocol::messages::{
    channel_notice, ChannelNoticeTail, ChatMessage, MACRO_EXPANDED_TYPES,
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
pub(crate) const NAME_MAX_TRIES: u16 = 120;

/// The most monster and battleground lines held at once for their `$`-macro subject's name.
pub(super) const MAX_HELD_SUBJECT_LINES: usize = 64;

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

/// One queued item awaiting its turn through the chat lane ([`fire_chat`]).
enum Pending {
    /// A `UNIT_COMBAT`, fired at its place among the lines its handler raised beside it.
    UnitCombat(crate::ui_unit::UnitCombatFeedback),
    /// The attack handler's `PLAYER_ENTER_COMBAT` or `PLAYER_LEAVE_COMBAT` (`0x6256ff`,
    /// `0x625778`), argless, fired at its place among the lines its packet raised.
    CombatEdge { entered: bool },
    /// A decoded `SMSG_MESSAGECHAT`; the player types wait on the sender's name.
    Wire { msg: ChatMessage, tries: u16 },
    /// A wire line the display latch queued past its name and its expansion (`0x49db5c`): its
    /// text the expanded one the queue call takes (`[ebp-0xa0c]` → `0x49db8c`), its sender's name
    /// the one looked up at its packet. The latch's drain shows it (`0x49d230`).
    Composed {
        msg: ChatMessage,
        name: Option<String>,
    },
    /// A channel notice with guids to name (`b_guid` is a kick's or ban's actor, 0 when absent).
    Notice {
        notice: u8,
        channel: String,
        a_guid: u64,
        b_guid: u64,
        tries: u16,
    },
    /// A member's flags changing (`0x49e910`): up to two `CHANNEL_NOTICE_USER` lines when the
    /// member's name is cached at the packet, nothing otherwise (the callback it registers holds
    /// no line), so it never waits.
    ModeChange {
        channel: String,
        guid: u64,
        old_flags: u8,
        new_flags: u8,
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
    /// One combat-log line, written by its formatter at the packet or at its item's callback
    /// ([`crate::combat_log`]); a line whose item missed waits in the wait list, not here.
    Combat(Box<super::combat::PendingCombat>),
    /// A ready event (client-composed lines; name-carrying notices).
    Event(ChatEvent),
    /// A ready line the display latch holds (`0x49cae0`): a defence broadcast's (`0x49dcc0`,
    /// `0x49de30`) or the billing warning's (`0x49577a`).
    Display(ChatEvent),
    /// A world broadcast at its packet's place, resolved into its lines where it fires
    /// ([`super::broadcast::resolve`]).
    Broadcast(super::broadcast::Broadcast),
    /// An item the display latch holds, past its name lookup (`0x49db5c`, `0x49ce3c`): it does not
    /// age, and fires once the world-enter cascade's release clears the latch (`0x4989c0`).
    Latched(Box<Pending>),
    /// A line whose event fired where it was written ([`ChatLog::write_combat_now`]): its tee to
    /// the log files alone, in order with the lines around it.
    Tee { combat: bool, line: String },
    /// A point where the held lines are retried, from place `from` on ([`Release`]).
    Release { cause: Release, from: u64 },
}

impl Pending {
    /// A monster or battleground line, which waits only on its `$`-macro subject's name and is
    /// retried only by that name's answer (`0x49ccc0`), never by the frame's age pass.
    fn held_for_subject(&self) -> bool {
        matches!(self, Pending::Wire { msg, .. } if MACRO_EXPANDED_TYPES.contains(&msg.chat_type))
    }

    /// The same item `step` tries older: only a lookup's first try asks the server.
    fn waited(mut self, step: u16) -> Self {
        match &mut self {
            Pending::Wire { tries, .. }
            | Pending::Notice { tries, .. }
            | Pending::Addon { tries, .. }
            | Pending::Roll { tries, .. }
            | Pending::TextEmote { tries, .. } => *tries = tries.saturating_add(step),
            Pending::Combat(line) => line.tries = line.tries.saturating_add(step),
            _ => {}
        }
        self
    }

    /// The records the item waits on that it names itself: a player's name, as `0x49ccc0`'s list
    /// holds a line for its sender's (a player line's sender, a channel notice's two, an addon
    /// line's or a roll's sender, an emote's performer) and a monster or battleground line for
    /// its `$`-macro subject's, by guid whatever the subject is (`0x49daea` asks the name cache
    /// `0xc0e228` through `0x55f080` with that callback).
    fn waits_on(&self) -> [Option<Answer>; 2] {
        let some = |g: u64| (g != 0).then_some(Answer::Player(g));
        match self {
            Pending::Wire { msg, .. } => [
                (msg.sender_name.is_none() && needs_name(msg.chat_type))
                    .then(|| some(msg.sender_guid))
                    .flatten(),
                self.macro_subject().map(Answer::Player),
            ],
            Pending::Notice { a_guid, b_guid, .. } => [some(*a_guid), some(*b_guid)],
            Pending::Addon { guid, .. } | Pending::Roll { guid, .. } => [some(*guid), None],
            Pending::TextEmote { performer, .. } => [some(*performer), None],
            _ => [None, None],
        }
    }

    /// The `$`-macro subject of a monster or battleground line whose expansion needs it
    /// ([`macro_subject_guid`]). A line that expands with no subject never waits (`0x49dac2`
    /// `jne 0x49db31` past the queue).
    fn macro_subject(&self) -> Option<u64> {
        match self {
            Pending::Wire { msg, .. }
                if MACRO_EXPANDED_TYPES.contains(&msg.chat_type)
                    && crate::npc_text::needs_subject(&msg.text) =>
            {
                Some(macro_subject_guid(msg)).filter(|&g| g != 0)
            }
            _ => None,
        }
    }
}

/// The guid a macro-expanded line's `$`-tokens expand against: a battleground system line's one
/// guid (`0x49d961`, read at `0x49d967`), the addressee of any other (`0x49da60`, passed at
/// `0x49daab`). The speaker's guid a monster say or yell carries first (`0x49d5dc`) never reaches
/// the expander, so a line with no addressee has no subject.
fn macro_subject_guid(msg: &ChatMessage) -> u64 {
    use benilla_protocol::messages::{
        CHAT_MSG_BG_SYSTEM_ALLIANCE, CHAT_MSG_BG_SYSTEM_HORDE, CHAT_MSG_BG_SYSTEM_NEUTRAL,
    };
    match msg.chat_type {
        CHAT_MSG_BG_SYSTEM_NEUTRAL | CHAT_MSG_BG_SYSTEM_ALLIANCE | CHAT_MSG_BG_SYSTEM_HORDE => {
            msg.sender_guid
        }
        _ => msg.target_guid,
    }
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

/// The chat lane ([`crate::lanes`]): the items the handlers and feeds queue, each stamped as it is
/// pushed, which [`fire_chat`] fires in stamp order among the other lanes.
///
/// An append-only sink shared through `&self`: a system that lends the call host appends here
/// while a call the host applies inside its Lua appends a refusal's line too, as the reference's
/// verb writes the chat frame in the call. One lock keeps every append in the order it was made.
#[derive(Resource, Default)]
pub(crate) struct ChatLog(std::sync::Mutex<ChatQueue>);

/// What [`ChatLog`] holds.
#[derive(Default)]
struct ChatQueue {
    /// The lane: every item not held, in stamp order.
    pending: std::collections::VecDeque<Queued>,
    /// The lines waiting for a name or an item's, out of the merge, in the order they began to
    /// wait, each with its place in that order.
    held: Vec<(u64, Pending)>,
    /// The next held line's place.
    held_count: u64,
    /// How many player-name answers `pending` holds, so a line looks for a later one only then.
    answers: usize,
    /// The display latch `[0x8435fc]`, as the place it was set: a line fired at or after it is
    /// held, not shown ([`Pending::Latched`]), until the world-enter cascade's release
    /// (`0x4989c0`).
    latched: Option<crate::lanes::Mark>,
    /// The combat log's wait list (`0xc4e208`): one slot per line whose item missed at its
    /// formatter's lookup, the lowest free slot taken first (`0x6293f0`).
    waits: Vec<Option<CombatWait>>,
}

/// A combat line's entry in the wait list: its formatter's kind at `+0`, its item at `+4`, and
/// what its callback writes the line again from (`+0x8` on).
struct CombatWait {
    kind: super::combat::ItemLog,
    item: u32,
    replay: super::combat::Replay,
}

/// One item in the lane, with its stamp ([`crate::lanes::stamp`]).
struct Queued {
    stamp: crate::lanes::Stamp,
    item: Pending,
}

/// A record a held line may read, landed in a packet: a name by guid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    Player(u64),
}

/// Why a [`Pending::Release`] retries the held lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Release {
    /// A record a held line may read landed.
    Answer(Answer),
    /// The frame's pass, which ages each line held before `before` by a frame.
    Age { before: u64 },
    /// The display latch's release (`0x4989c0` → `0x49d230`): every held line shows that can.
    Display,
}

impl ChatQueue {
    /// Stamp `item` and queue it at its place: behind everything stamped before it, and a nested
    /// item ahead of what its cause fired before.
    fn push(&mut self, item: Pending) {
        let stamp = crate::lanes::stamp();
        if matches!(
            item,
            Pending::Release {
                cause: Release::Answer(_),
                ..
            }
        ) {
            self.answers += 1;
        }
        let at = self.pending.partition_point(|q| q.stamp < stamp);
        self.pending.insert(at, Queued { stamp, item });
    }
}

impl ChatLog {
    /// The queue, held for one step; a poisoned lock still holds whole items.
    fn q(&self) -> std::sync::MutexGuard<'_, ChatQueue> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Drop everything queued: the session's end.
    pub(crate) fn reset(&self) {
        *self.q() = ChatQueue::default();
    }

    /// Set the display latch `[0x8435fc]` here, as the world session's start finds it (1 from
    /// start-up) and the leave-world sweep sets it (`0x490a9e` → `0x498a10`): the lines fired from
    /// here on are held until the world-enter cascade's release ([`release_display`]). A latch
    /// already set keeps its place.
    pub(crate) fn latch(&self) {
        let mut q = self.q();
        if q.latched.is_none() {
            q.latched = Some(crate::lanes::mark());
        }
    }

    /// Whether the display latch holds a line fired at `at`: set at or before its place.
    pub(crate) fn latched_at(&self, at: crate::lanes::Stamp) -> bool {
        self.q().latched.is_some_and(|m| at.since(m))
    }

    /// The lane's head, for the drain ([`crate::lanes`]).
    pub(crate) fn peek(&self) -> Option<crate::lanes::Stamp> {
        self.q().pending.front().map(|q| q.stamp)
    }

    /// Take the head if `until` admits it.
    fn pop(&self, until: crate::lanes::Until) -> Option<Queued> {
        let mut q = self.q();
        if !until.admits(q.pending.front()?.stamp) {
            return None;
        }
        let head = q.pending.pop_front()?;
        if matches!(
            head.item,
            Pending::Release {
                cause: Release::Answer(_),
                ..
            }
        ) {
            q.answers -= 1;
        }
        Some(head)
    }

    /// Hold a line that waits, out of the merge, behind the lines already waiting.
    ///
    /// Deviation: the reference holds every line that waits on its `$`-macro subject's name in
    /// the name cache's callback list (`0x49daea` → `0x55f080`) until the name answers, with no
    /// bound; here at most [`MAX_HELD_SUBJECT_LINES`] wait, the oldest dropped past that, as a
    /// subject no player owns never answers.
    fn hold(&self, item: Pending) {
        let mut q = self.q();
        if item.held_for_subject() {
            let held: Vec<usize> = (0..q.held.len())
                .filter(|&i| q.held[i].1.held_for_subject())
                .collect();
            if held.len() >= MAX_HELD_SUBJECT_LINES {
                q.held.remove(held[0]);
            }
        }
        let seq = q.held_count;
        q.held_count += 1;
        q.held.push((seq, item));
    }

    /// Hold again a line a release point retried, at its place.
    fn hold_again(&self, seq: u64, item: Pending) {
        let mut q = self.q();
        let at = q.held.partition_point(|(s, _)| *s < seq);
        q.held.insert(at, (seq, item));
    }

    /// Take the first held line at place `from` or later, and before `before` when given.
    fn take_held(&self, from: u64, before: Option<u64>) -> Option<(u64, Pending)> {
        let mut q = self.q();
        let at = q.held.partition_point(|(s, _)| *s < from);
        let (seq, _) = q.held.get(at)?;
        if before.is_some_and(|b| *seq >= b) {
            return None;
        }
        Some(q.held.remove(at))
    }

    /// Queue a release point for the held lines from place `from` on.
    fn push_release(&self, cause: Release, from: u64) {
        self.q().push(Pending::Release { cause, from });
    }

    /// Whether `answer` lands later in the lane than `at`: a line stamped `at` that reads the
    /// record waits for it, as the reference's lookup missed at its packet.
    fn answered_after(&self, answer: Answer, at: crate::lanes::Stamp) -> bool {
        let q = self.q();
        q.answers > 0
            && q.pending.iter().any(|item| {
                item.stamp > at
                    && matches!(
                        item.item,
                        Pending::Release {
                            cause: Release::Answer(a),
                            ..
                        } if a == answer
                    )
            })
    }

    /// A name a held line may read landed in the packet being handled. The lines waiting on it
    /// show here, in the packets' order, as the name cache runs the callbacks waiting on a record
    /// (`0x55f426`-`0x55f452`), whose chat callback shows the lines (`0x49ccc0`). Queued only when
    /// a line could be waiting: one held, or one queued ahead of the answer that reads it.
    pub(crate) fn answer_landed(&self, answer: Answer) {
        let mut q = self.q();
        let waits = !q.held.is_empty()
            || q.pending
                .iter()
                .any(|i| i.item.waits_on().contains(&Some(answer)));
        if waits {
            q.push(Pending::Release {
                cause: Release::Answer(answer),
                from: 0,
            });
        }
    }

    /// The frame's age pass ahead of the drain: the lines held now are retried after every item
    /// the frame's packets and feeds queued, each a frame older, and one that waited
    /// `NAME_MAX_TRIES` frames shows with its names unknown.
    pub(crate) fn begin_frame(&self) {
        let mut q = self.q();
        if !q.held.is_empty() {
            let before = q.held_count;
            q.push(Pending::Release {
                cause: Release::Age { before },
                from: 0,
            });
        }
    }
}

impl ChatLog {
    /// Queue one world broadcast at its packet's place, resolved where it fires.
    pub(crate) fn push_broadcast(&self, b: super::broadcast::Broadcast) {
        self.q().push(Pending::Broadcast(b));
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

    /// Queue a ready line the display latch holds (`0x49cae0`): a defence broadcast's or the
    /// billing warning's.
    pub(crate) fn push_display(&self, event: ChatEvent) {
        self.q().push(Pending::Display(event));
    }

    /// Show a client-composed line inside the verb that adds it, as `0x49a870` does: its event
    /// fires now, and only its log-file tee waits for the drain, in order with the lines around
    /// it. For a line whose composition reads no default language.
    pub(crate) fn show_now(&self, script: &mut benilla_ui::script::UiScript, event: &ChatEvent) {
        if let Some((name, args)) = self.write_now(script, event) {
            script.fire_event(name, args);
        }
    }

    /// [`Self::show_now`] for a caller that signals in place: the line's tee queued, its event
    /// handed back.
    pub(crate) fn write_now(
        &self,
        script: &benilla_ui::script::UiScript,
        event: &ChatEvent,
    ) -> Option<(&'static str, Vec<benilla_ui::script::ScriptValue>)> {
        let kind = event.kind?;
        let line = super::frames::compose(event, kind, "", &|key| {
            script.lua().globals().get::<String>(key).ok()
        });
        if let Some(line) = line {
            self.q().push(Pending::Tee {
                combat: kind.is_combat_log(),
                line,
            });
        }
        Some((super::event::event_name(kind), event.script_args()))
    }

    /// Queue one combat-log line, classified at the packet by [`super::combat`], for its names.
    pub(crate) fn push_combat(&self, line: super::combat::PendingCombat) {
        self.q().push(Pending::Combat(Box::new(line)));
    }

    /// Keep a combat-log line whose item missed at its formatter's lookup in the wait list: the
    /// lowest free slot (`0x6293f0`), its formatter's `kind`, its `item` and what the item's
    /// callback writes it again from ([`Self::take_wait`]). Nothing is queued on the lane.
    pub(crate) fn push_combat_waiting(
        &self,
        kind: super::combat::ItemLog,
        item: u32,
        replay: super::combat::Replay,
    ) {
        let mut q = self.q();
        let wait = Some(CombatWait { kind, item, replay });
        match q.waits.iter().position(Option::is_none) {
            Some(slot) => q.waits[slot] = wait,
            None => q.waits.push(wait),
        }
    }

    /// The last waiting entry of `kind` and `item`, scanned from the back and freed (`0x6295b0`):
    /// what one item callback writes its line again from, so lines of one kind and item come out
    /// newest first, one per callback.
    pub(crate) fn take_wait(
        &self,
        kind: super::combat::ItemLog,
        item: u32,
    ) -> Option<super::combat::Replay> {
        let mut q = self.q();
        let slot = q
            .waits
            .iter()
            .rposition(|w| w.as_ref().is_some_and(|w| w.kind == kind && w.item == item))?;
        q.waits[slot].take().map(|w| w.replay)
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
            ChannelNoticeTail::ModeChange {
                guid,
                old_flags,
                new_flags,
            } => {
                self.q().push(Pending::ModeChange {
                    channel,
                    guid: *guid,
                    old_flags: *old_flags,
                    new_flags: *new_flags,
                });
                return;
            }
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
            if let Some(mut event) = notice_event(notice_byte, channel, name, None) {
                if let ChannelNoticeTail::YouJoined { instance, .. } = tail {
                    event.joined_instance = *instance;
                }
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

    /// Drop every pending and held item on disconnect, and the combat log's wait list with them:
    /// the item cache drops their callbacks there ([`crate::items::Items::clear_session`]), so an
    /// entry left would be freed by a later line's callback in that line's place.
    pub(crate) fn clear_session(&self) {
        let mut q = self.q();
        q.pending.clear();
        q.held.clear();
        q.answers = 0;
        q.waits.clear();
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
                Pending::Composed { .. } => "composed",
                Pending::Notice { .. } => "notice",
                Pending::ModeChange { .. } => "mode change",
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
                Pending::Display(_) => "display",
                Pending::Broadcast(_) => "broadcast",
                Pending::Latched(_) => "latched",
                Pending::Tee { .. } => "tee",
                Pending::Release { .. } => "release",
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

    /// Every wire line waiting to render, in order.
    #[cfg(test)]
    pub(crate) fn pending_wire(&self) -> Vec<ChatMessage> {
        self.q()
            .pending
            .iter()
            .filter_map(|q| match &q.item {
                Pending::Wire { msg, .. } => Some(msg.clone()),
                _ => None,
            })
            .collect()
    }

    /// The text of every composed line waiting to render, in order.
    #[cfg(test)]
    pub(crate) fn pending_lines(&self) -> Vec<String> {
        self.pending_events().into_iter().map(|e| e.text).collect()
    }

    /// The text of every line written since the drain, in order: those waiting to render, and
    /// the tees of those whose event fired where they were written.
    #[cfg(test)]
    pub(crate) fn written_lines(&self) -> Vec<String> {
        self.q()
            .pending
            .iter()
            .filter_map(|q| match &q.item {
                Pending::Event(e) | Pending::Display(e) => Some(e.text.clone()),
                Pending::Tee { line, .. } => Some(line.clone()),
                _ => None,
            })
            .collect()
    }

    /// Every composed event waiting to render, in order.
    #[cfg(test)]
    pub(crate) fn pending_events(&self) -> Vec<ChatEvent> {
        self.q()
            .pending
            .iter()
            .map(|q| &q.item)
            .filter_map(|p| match p {
                Pending::Event(e) | Pending::Display(e) => Some(e.clone()),
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
    // Each arm's `mov edi` in the `0x49c60c` jump table: MODE_CHANGE's arm fires its own lines
    // (`mode_change_events`), a byte past THROTTLED fires SYSTEM, the rest CHANNEL_NOTICE (0x12)
    // or CHANNEL_NOTICE_USER (0x13).
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

/// The lines of a member's flag change (`0x49e910`): `SET_`/`UNSET_MODERATOR` for bit 2, then
/// `SET_`/`UNSET_VOICE` for bit 4, each a `CHANNEL_NOTICE_USER` (type 0x13) naming the member. A
/// bit gained (old clear, new set) is the set text, a bit lost (old set, new clear) the unset
/// text, an unchanged one nothing (`0x49e8a8`-`0x49e8e3`).
fn mode_change_events(channel: &str, name: &str, old: u8, new: u8) -> Vec<ChatEvent> {
    [
        (0x02, "SET_MODERATOR", "UNSET_MODERATOR"),
        (0x04, "SET_VOICE", "UNSET_VOICE"),
    ]
    .into_iter()
    .filter_map(|(bit, set, unset)| {
        let token = match (old & bit != 0, new & bit != 0) {
            (false, true) => set,
            (true, false) => unset,
            _ => return None,
        };
        Some(ChatEvent {
            kind: Some(ChatEventKind::ChannelNoticeUser),
            sender: name.to_string(),
            channel: channel.to_string(),
            notice: channel_notice::MODE_CHANGE.to_string(),
            token: Some(token),
            ..Default::default()
        })
    })
    .collect()
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

/// The slots and the call host of the chat lane's firing, borrowed in turn: the host is lent around Lua
/// alone, and the slots are written between.
type SlotsAndHost<'w, 's> = crate::script_calls::disjoint::Pair<
    'w,
    's,
    ChatSlots<'static>,
    Option<crate::script_calls::ScriptCallHost<'w, 's>>,
>;

/// The chat lane's channel slots, and the bubble and filter switches a line reads as it shows:
/// CVar knobs, which the host's callbacks write inside the calls that write their rows.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ChatSlots<'w> {
    channels: ResMut<'w, ChannelState>,
    bubbles: Res<'w, crate::chat_bubble::BubbleConfig>,
    filters: Res<'w, crate::text_filter::TextFilterSwitches>,
    /// The last init's scope, which the defense flag's flip tests before it signals (`0x49bdc0`).
    world_states: Option<Res<'w, crate::world_state::WorldStates>>,
}

/// [`deliver`] with the host lent around the event alone: a join a handler makes takes its slot
/// through the host against the slots the confirm left, and the free after it leaves that join's
/// slot where it is, as the notify handler frees after its event (`0x49c5b0`, `0x49c5b5`). A
/// confirm or a free that flips the zone-defense flag signals the readout in place, the confirm's
/// ahead of the event and the free's after it (`0x49bbbf`, `0x49bc41` → `0x49bd50`).
fn deliver_hosted(
    script: &mut benilla_ui::script::UiScript,
    set: &mut SlotsAndHost,
    windows: &mut ChatWindows,
    event: &mut ChatEvent,
) {
    let flag = set.p0().channels.defense_channel;
    let dropping = deliver_before(script, &mut set.p0().channels, event);
    defense_flip(script, set, flag);
    crate::script_calls::lent(script, set.p1().as_mut(), |script| {
        route(script, windows, event)
    });
    let flag = set.p0().channels.defense_channel;
    deliver_after(script, &mut set.p0().channels, dropping);
    defense_flip(script, set, flag);
}

/// `0x49bd50`'s tail when the zone-defense flag moved off `before` (`0x49bdb8`-`0x49bdd6`): the
/// readout's list follows the flag, which its getters read at the call, and `UPDATE_WORLD_STATES`
/// signals in place, with the host lent, once an init has set the scope (`0x49bdc0`).
fn defense_flip(script: &mut benilla_ui::script::UiScript, set: &mut SlotsAndHost, before: bool) {
    let slots = set.p0();
    if slots.channels.defense_channel == before {
        return;
    }
    let Some(states) = slots.world_states.as_deref() else {
        return;
    };
    let scoped = states.scope().is_some();
    crate::script_calls::lent(script, set.p1().as_mut(), |script| {
        if scoped {
            script.fire_event("UPDATE_WORLD_STATES", Vec::new());
        }
    });
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
            Some(slot) => {
                // `0x49bb20` stores the notice's instance with the confirm.
                channels.store_instance(slot, event.joined_instance);
                debug!(
                    "chat: server confirms channel {:?} joined (slot {slot})",
                    event.channel
                );
            }
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
/// [`ChatReads`]; the bubble has a 20 yd range test and two CVars, the gesture neither.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct SpeakerEffects<'w> {
    bubbles: ResMut<'w, crate::chat_bubble::BubbleQueue>,
    gestures: Res<'w, crate::creature_anim::GestureQueue>,
}

/// `UNIT_FIELD_FLAGS` bit `0x20000000` (`PREVENT_ANIM`: Polymorph, Kidney Shot, Gouge, Sleep).
const UNIT_FLAG_PREVENT_ANIM: u32 = 0x2000_0000;

/// Whether the speaker's flags suppress the chat talk gesture. Only the handler's own copy of
/// the selector tests the bit (`0x49d7f7`, the one `0xa0` test of `0x20000000` in the image),
/// which runs when the sender's name is already cached (`immediate`); a line held for the name
/// query or the display latch gestures from the deferred copies (`0x49cf2b`, `0x49d3bb`), which
/// do not.
pub(super) fn gesture_prevented(immediate: bool, speaker_flags: Option<u32>) -> bool {
    immediate && speaker_flags.is_some_and(|f| f & UNIT_FLAG_PREVENT_ANIM != 0)
}

/// What the chat lane's firing reads besides the VM and the log: the windows, the names, the
/// speaker's effects, the language gate and the filters, bundled for Bevy's parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct ChatReads<'w, 's> {
    windows: ResMut<'w, ChatWindows>,
    names: Res<'w, NameCache>,
    speaker: SpeakerEffects<'w>,
    commands: Res<'w, NetCommands>,
    // The text-emote tables, and our guid for the "are you the performer?" test.
    emote_texts: Option<Res<'w, EmoteTexts>>,
    // Our guid, for the lines that test or name the active player.
    self_guid: Res<'w, SelfGuid>,
    // The `$`-macro subject: monster and BG lines expand against their addressee's descriptors.
    guids: Res<'w, GuidIndex>,
    stores: Query<'w, 's, &'static ObjectStore>,
    states: Res<'w, crate::world_state::WorldStates>,
    // The language gate: the word pool, and this character's fluency and GM bit as the line
    // shows.
    langs: super::language::LanguageReads<'w, 's>,
    // Item names for `TRADESKILL_LOG`, `FEEDPET_LOG`, `ITEMENCHANTMENT*`, `SPELLDURABILITYDAMAGE`,
    // and with the game object records the names of a combat line's held objects.
    items: Res<'w, crate::items::Items>,
    gameobjects: Res<'w, crate::go_templates::GameObjectTemplates>,
    unit_names: crate::ui_unit::NameTables<'w>,
    // The spam and profanity filters, which run in the reference's display function `0x49a870`.
    text_filter: crate::text_filter::ChatTextFilter<'w>,
    // The catalogs and the zone a world broadcast resolves against.
    broadcasts: super::broadcast::BroadcastReads<'w>,
}

/// The chat lane's firing: the VM, the slots the channel notices write between their Lua and the
/// host its calls apply through; the log; and the rest of what a line reads.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ChatFiring<'w, 's> {
    vm: crate::script_calls::VmWith<'w, 's, ChatSlots<'static>>,
    log: Res<'w, ChatLog>,
    reads: ChatReads<'w, 's>,
}

impl ChatFiring<'_, '_> {
    /// Fire the log's items in stamp order while `until` admits them ([`crate::lanes`]), and
    /// stop after one whose firing stamped anything, so the drain takes what it pushed in its
    /// place. A line that waits for a name or an item's leaves the merge, held
    /// ([`ChatLog::hold`]); the answers and the frame's age pass retry the held lines in the
    /// order they began to wait. With no VM nothing fires and the items wait.
    fn fire(&mut self, until: crate::lanes::Until) {
        let Self { vm, log, reads } = self;
        let (Some(script), set) = vm.parts() else {
            return;
        };
        while let Some(Queued { stamp, item }) = log.pop(until) {
            let moved = if let Pending::Release { cause, from } = item {
                release(reads, log, script, set, stamp, cause, from)
            } else {
                let (waits, moved) =
                    crate::lanes::firing(stamp, || fire_one(reads, log, script, set, item, stamp));
                if let Some(item) = waits {
                    log.hold(item.waited(1));
                }
                moved
            };
            if moved {
                break;
            }
        }
    }
}

/// A release point's retry of the held lines, in the order they began to wait: those held at
/// `from` or later, and for the frame's age pass only those held before it began (`Age`), each a
/// frame older. A line that fires now fires here, at the point's stamp, as the name's answer
/// shows the lines that waited on it (`0x49ccc0` → `0x49a870`, `0x49cfc1`); one whose firing
/// stamped anything ends the pass, and a point nested behind what it pushed takes up the rest.
/// Whether anything was stamped.
#[allow(clippy::too_many_arguments)]
fn release(
    reads: &mut ChatReads,
    log: &ChatLog,
    script: &mut benilla_ui::script::UiScript,
    set: &mut SlotsAndHost,
    at: crate::lanes::Stamp,
    cause: Release,
    from: u64,
) -> bool {
    let (step, before) = match cause {
        Release::Answer(_) | Release::Display => (0, None),
        Release::Age { before } => (1, Some(before)),
    };
    let mut next = from;
    while let Some((seq, item)) = log.take_held(next, before) {
        next = seq + 1;
        if before.is_some() && item.held_for_subject() {
            log.hold_again(seq, item);
            continue;
        }
        let (waits, moved) =
            crate::lanes::firing(at, || fire_one(reads, log, script, set, item, at));
        if let Some(item) = waits {
            log.hold_again(seq, item.waited(step));
        }
        if moved {
            crate::lanes::firing(at, || log.push_release(cause, next));
            return true;
        }
    }
    false
}

/// Fire one item at `at`, or hand it back to wait: for a name the cache lacks, for one whose
/// answer comes later in the frame than `at`, so the record the reference's lookup missed there
/// is not read early (a name), or for an item's name.
fn fire_one(
    reads: &mut ChatReads,
    log: &ChatLog,
    script: &mut benilla_ui::script::UiScript,
    set: &mut SlotsAndHost,
    item: Pending,
    at: crate::lanes::Stamp,
) -> Option<Pending> {
    // A held item waits, unaged, for the cascade's release; once the latch is clear it runs as
    // it came in, its names now cached.
    let item = match item {
        Pending::Latched(inner) if log.latched_at(at) => return Some(Pending::Latched(inner)),
        Pending::Latched(inner) => *inner,
        item => item,
    };
    if item
        .waits_on()
        .into_iter()
        .flatten()
        .any(|answer| log.answered_after(answer, at))
    {
        return Some(item);
    }
    // A broadcast's lines, each fired here at its packet's place; a defence line the latch holds
    // waits as a held line.
    if let Pending::Broadcast(b) = item {
        let lines = super::broadcast::resolve(&b, script, &set.p0().channels, &reads.broadcasts);
        for line in lines {
            let line = match line {
                super::broadcast::BroadcastLine::Display(e) => Pending::Display(e),
                super::broadcast::BroadcastLine::Event(e) => Pending::Event(e),
            };
            if let Some(waits) = fire_one(reads, log, script, set, line, at) {
                log.hold(waits);
            }
        }
        return None;
    }
    let ChatReads {
        windows,
        names,
        speaker,
        commands,
        emote_texts,
        self_guid,
        guids,
        stores,
        states,
        langs,
        items,
        gameobjects,
        unit_names,
        text_filter,
        broadcasts: _,
    } = reads;
    let windows: &mut ChatWindows = windows;
    // A channel line's record, read as the composer reads it (`0x49aa1e`-`0x49aa48`) before
    // the line's Lua runs.
    let record = match &item {
        Pending::Wire { msg, .. } | Pending::Composed { msg, .. } => msg
            .channel
            .as_deref()
            .and_then(|name| set.p0().channels.stamp_of(name)),
        _ => None,
    };
    // A channel notice writes its slot around its event, outside the host's lend.
    let item = match item {
        Pending::Event(mut event) => {
            deliver_hosted(script, set, windows, &mut event);
            None
        }
        Pending::Display(event) if log.latched_at(at) => {
            return Some(Pending::Latched(Box::new(Pending::Display(event))));
        }
        Pending::Display(mut event) => {
            deliver_hosted(script, set, windows, &mut event);
            None
        }
        Pending::Notice {
            notice,
            channel,
            a_guid,
            b_guid,
            tries,
        } => {
            let ask = Ask::at(tries == 0, commands);
            let a = names.resolve_player(a_guid, ask).map(str::to_string);
            let b = if b_guid != 0 {
                names.resolve_player(b_guid, ask).map(str::to_string)
            } else {
                Some(String::new())
            };
            if (a.is_none() || b.is_none()) && tries < NAME_MAX_TRIES {
                return Some(Pending::Notice {
                    notice,
                    channel,
                    a_guid,
                    b_guid,
                    tries,
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
                deliver_hosted(script, set, windows, &mut event);
            }
            None
        }
        Pending::ModeChange {
            channel,
            guid,
            old_flags,
            new_flags,
        } => {
            // `0x49e934` asks the name cache once: a miss, or an answer that lands later in the
            // frame than this packet, shows nothing (`0x49e93d`).
            let known = (!log.answered_after(Answer::Player(guid), at))
                .then(|| names.resolve_player(guid, Ask::at(true, commands)))
                .flatten()
                .map(str::to_string);
            if let Some(name) = known {
                for mut event in mode_change_events(&channel, &name, old_flags, new_flags) {
                    deliver_hosted(script, set, windows, &mut event);
                }
            }
            None
        }
        item => Some(item),
    };
    let item = item?;
    // The knobs as the line's Lua begins; a write its handlers make moves the next line's.
    let (filters, bubbles) = {
        let slots = set.p0();
        (*slots.filters, *slots.bubbles)
    };
    crate::script_calls::lent(script, set.p1().as_mut(), |script| {
        match item {
            Pending::UnitCombat(ev) => {
                crate::ui_unit::fire_unit_combat(script, &ev);
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
            Pending::Event(_)
            | Pending::Display(_)
            | Pending::Broadcast(_)
            | Pending::Latched(_)
            | Pending::Notice { .. }
            | Pending::ModeChange { .. }
            | Pending::Release { .. } => {}
            item @ (Pending::Wire { .. } | Pending::Composed { .. }) => {
                // The speaker's talk or laugh gesture, its object read where it plays.
                let gesture_of = |chat_type: u8, text: &str, guid: u64, immediate: bool| {
                    let speaker_flags = guids
                        .0
                        .get(&guid)
                        .and_then(|e| stores.get(*e).ok())
                        .map(|s| s.0.unit_flags());
                    crate::creature_anim::select_gesture(chat_type, text, |n| {
                        script
                            .lua()
                            .globals()
                            .get::<String>(format!("LAUGH_WORD{n}"))
                            .ok()
                    })
                    .filter(|_| !gesture_prevented(immediate, speaker_flags))
                };
                let (msg, name, plain) = match item {
                    Pending::Composed { msg, name } => {
                        // The latch's drain gestures again, on the queued text, its speaker read
                        // there (`0x49d306`, `0x49d3bb`), with no anim-prevent test.
                        if let Some(gesture) =
                            gesture_of(msg.chat_type, &msg.text, msg.sender_guid, false)
                        {
                            speaker.gestures.push(msg.sender_guid, gesture);
                        }
                        let plain = msg.text.clone();
                        (msg, name, plain)
                    }
                    Pending::Wire { msg, tries } => {
                        let kind = kind_of_wire(msg.chat_type);
                        if kind.is_none() {
                            warn!(
                                "chat: unmodeled wire type {:#04x} dropped: {:?}",
                                msg.chat_type, msg.text
                            );
                            return None;
                        }
                        // A monster line carries its name inline; a player line resolves its guid.
                        let name = match &msg.sender_name {
                            Some(n) => Some(n.clone()),
                            None if needs_name(msg.chat_type) && msg.sender_guid != 0 => names
                                .resolve_player(msg.sender_guid, Ask::at(tries == 0, commands))
                                .map(str::to_string),
                            _ => None,
                        };
                        if name.is_none()
                            && needs_name(msg.chat_type)
                            && msg.sender_guid != 0
                            && tries < NAME_MAX_TRIES
                        {
                            return Some(Pending::Wire { msg, tries });
                        }
                        // `$`-macros expand for the monster, boss and BG system types only, against
                        // [`macro_subject_guid`]. A failed expansion (`0x49dac2`-`0x49db1e`, `0x49d9c9`)
                        // with a zero subject or a known name drops the line, and one with an unknown name
                        // holds it until the name's answer, whose failure shows the raw text.
                        let expanded = if MACRO_EXPANDED_TYPES.contains(&msg.chat_type) {
                            let subject_guid = macro_subject_guid(&msg);
                            let subject = crate::npc_text::subject_for_guid(
                                subject_guid,
                                guids,
                                stores,
                                names,
                                Ask::at(tries == 0, commands),
                                unit_names.get(),
                                || script.unknown_object_text(),
                            );
                            let (text, clean) = crate::npc_text::substitute_checked(
                                &msg.text,
                                &crate::npc_text::MacroContext {
                                    subject: subject.as_ref(),
                                    states,
                                },
                            );
                            // The player-name cache's record, valid (`0x49daea`, `0x49daf1`).
                            let named = names.resolve_player(subject_guid, Ask::Read).is_some();
                            if clean {
                                Some(text)
                            } else if subject_guid == 0 || (named && tries == 0) {
                                debug!(
                                    "chat: dropping unexpandable [{:#04x}] {:?} (subject \
                                     {subject_guid:#x})",
                                    msg.chat_type, msg.text
                                );
                                return None; // the reference prints no line
                            } else if !named {
                                // `subject_for_guid` sent the name query: the line waits for the name
                                // however long it takes, as the name cache's callback list holds it
                                // (`0x55f080`), and shows nothing before.
                                return Some(Pending::Wire { msg, tries });
                            } else {
                                // The name's answer re-expands it, and a failure there shows the raw
                                // text (`0x49cf70`, `0x49cf82`).
                                None
                            }
                        } else {
                            None
                        };
                        // The speaker's gesture, at its packet past the name (`0x49d7d0`-`0x49d8ae`),
                        // ahead of the latch and the line function's filters: the selector reads
                        // the raw type and the wire text, not the garbled one, so a Horde `lol`
                        // laughs for every observer. The packet alone refuses a speaker that
                        // cannot animate (`0x49d7f7`); a line that waited on its name gestures in
                        // the answer's callback, past its own latch test (`0x49ce3c`, `0x49cf2b`).
                        let latched = log.latched_at(at);
                        if tries == 0 || !latched {
                            if let Some(gesture) =
                                gesture_of(msg.chat_type, &msg.text, msg.sender_guid, tries == 0)
                            {
                                speaker.gestures.push(msg.sender_guid, gesture);
                            }
                        }
                        let plain = expanded.unwrap_or_else(|| msg.text.clone());
                        // The display latch, past the name and the expansion (`0x49db5c`): the
                        // line is queued with its expanded text (`0x49db8c`) for the cascade's
                        // release.
                        if latched {
                            let msg = ChatMessage { text: plain, ..msg };
                            return Some(Pending::Latched(Box::new(Pending::Composed {
                                msg,
                                name,
                            })));
                        }
                        (msg, name, plain)
                    }
                    _ => unreachable!("matched as a wire line"),
                };
                let kind = kind_of_wire(msg.chat_type);
                // The base name; `stamp_channel` adds arg4's number and arg7-arg9 below.
                let channel_base = msg.channel.clone().unwrap_or_default();
                // The language gate: the garble writes the one buffer the line, `arg1` and the
                // bubble read, as `0x49a870` fills `[ebp-0xd0c]` once (`0x49a9f0` copy,
                // `0x49aa7c` garble) and never reads the wire text again.
                let langs = langs.gate();
                let language = langs.effective_language(msg.chat_type, msg.language);
                let mut text = langs.garble(language, &plain);

                // The text filters, on the garbled buffer as in `0x49a870`: spam first, then
                // the mask, so a dropped line consumes no mask indices (the phase is global).
                if text_filter.should_drop(
                    filters,
                    msg.chat_type,
                    msg.chat_tag,
                    langs.is_gm(),
                    &text,
                ) {
                    // Nothing shows in its place: the not-a-whisper leg (`0x49ab33`) jumps to
                    // the epilogue `0x49afd7`. A whisper with a non-zero report guid also sends
                    // `CMSG_CHAT_FILTERED` (`0x331`, the `u64` guid, `0x49ab50`) in the
                    // reference; not sent here, as vmangos leaves the opcode unhandled
                    // (`Opcodes.cpp:932`).
                    debug!(
                        "chat: spam filter dropped [{:#04x}] {:?}",
                        msg.chat_type, text
                    );
                    return None;
                }
                text_filter.mask_chat(filters, msg.chat_type, &mut text);
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
                if let Some(record) = record {
                    record.apply(&mut event);
                }
                route(script, windows, &event);
                // The bubble gets the same text: the reference spawns it in this path
                // (`0x49acd9`).
                if let Some(kind) = event.kind {
                    speaker.bubbles.push(&bubbles, msg.sender_guid, kind, &text);
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
                    .resolve_player(guid, Ask::at(tries == 0, commands))
                    .map(str::to_string);
                if name.is_none() && tries < NAME_MAX_TRIES {
                    return Some(Pending::Addon {
                        prefix,
                        message,
                        distribution,
                        guid,
                        tries,
                    });
                }
                // The display latch, past the name (`0x49ce3c`, the addon test downstream at
                // `0x49dbcb`).
                if log.latched_at(at) {
                    return Some(Pending::Latched(Box::new(Pending::Addon {
                        prefix,
                        message,
                        distribution,
                        guid,
                        tries,
                    })));
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
                    .resolve_player(guid, Ask::at(tries == 0, commands))
                    .map(str::to_string);
                if name.is_none() && tries < NAME_MAX_TRIES {
                    return Some(Pending::Roll {
                        min,
                        max,
                        roll,
                        guid,
                        tries,
                    });
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
                        windows,
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
                        names,
                        Ask::at(true, commands),
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
                        windows,
                        &ChatEvent::text_only(ChatEventKind::CombatXpGain, text),
                    );
                }
            }
            Pending::XpLoss { total } => {
                let line = xp_loss_line(total, &globals(script));
                if let Some(text) = line {
                    route(
                        script,
                        windows,
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
                // active player (`0x612bf0`, `0x625374`), its real gender (the Deviation on
                // `unit_gender`); a dishonorable kill's template names no rank.
                let name = (victim != 0).then(|| {
                    super::combat::unit_name(
                        victim,
                        guids.0.get(&victim).and_then(|e| stores.get(*e).ok()),
                        names,
                        Ask::at(true, commands),
                    )
                    .unwrap_or_else(|| script.unknown_object_text())
                });
                let female = self_guid
                    .0
                    .and_then(|g| guids.0.get(&g))
                    .and_then(|e| stores.get(*e).ok())
                    .and_then(|me| me.0.unit_gender())
                    == Some(1);
                let title =
                    (victim != 0 && honor > 0).then(|| script.pvp_rank_title(rank, side, female));
                write_honor_line(
                    script,
                    windows,
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
                // The performer's name, looked up as the packet's (`0x49dc59`); a named emote
                // under the display latch is held apart and never shown (`0x49dc7f` →
                // `0x49dc95`, `0x49d165`), whatever our own name's state.
                let ask = Ask::at(tries == 0, commands);
                let performer_name = names.resolve_player(performer, ask).map(str::to_string);
                if performer_name.is_some() && log.latched_at(at) {
                    return None;
                }
                // Ours, for the "is the target me?" test.
                let your_name = self_guid
                    .0
                    .and_then(|g| names.resolve_player(g, ask).map(str::to_string));
                if (performer_name.is_none() || your_name.is_none()) && tries < NAME_MAX_TRIES {
                    return Some(Pending::TextEmote {
                        performer,
                        text_id,
                        target,
                        tries,
                    });
                }
                // No performer name, no line (not "Unknown"): the reference bails when its
                // player-only name cache misses (`0x49b28c`); the animation rode
                // `EmoteMessage`.
                let (Some(performer_name), Some(cat)) = (performer_name, emote_texts.as_deref())
                else {
                    debug!("chat: text emote {text_id} from {performer:#x} has no sentence source");
                    return None;
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
                let event = event?;
                route(script, windows, &event);
            }
            Pending::Combat(mut line) => {
                // Its item was looked up by its formatter, at the packet or again at the item's
                // callback ([`crate::combat_log`]); here it is only read. Names looked up with a
                // callback as the line's (`0x6265a7`, `0x6265e0`), and asked again only on its
                // first drain.
                let ask = Ask::at(line.tries == 0, commands);
                let sources = super::combat::NameSources {
                    names,
                    gameobjects,
                    items,
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
                    |entry| match items.template(entry, 0, Ask::Read).map(|t| t.name.clone()) {
                        Some(name) => super::combat::ItemName::Named(name),
                        None if items.template_answered_unknown(entry) => {
                            super::combat::ItemName::Unknown
                        }
                        None => super::combat::ItemName::Waiting,
                    },
                );
                match ready {
                    super::combat::Filled::Waiting => return Some(Pending::Combat(line)),
                    super::combat::Filled::Dropped => return None,
                    super::combat::Filled::Ready => write_combat_line(script, windows, &line),
                }
            }
            Pending::Standing {
                faction,
                rank,
                female,
            } => {
                let line = crate::ui_reputation::standing_text(
                    benilla_ui::strings::Strings::Vm(script.lua()),
                    rank,
                    female,
                    &faction,
                );
                if let Some(text) = line {
                    route(
                        script,
                        windows,
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
                        windows,
                        &ChatEvent::text_only(ChatEventKind::System, text),
                    );
                }
            }
        }
        None
    })
}

/// The chat lane's `fire_until` ([`crate::lanes`]).
pub(super) fn fire_chat(In(until): In<crate::lanes::Until>, mut firing: ChatFiring) {
    firing.fire(until);
}

/// The world-enter cascade's chat release (`0x490974` → `0x4989c0`), in its place
/// ([`crate::ui_world_enter`]), after both login events: the display latch cleared (`0x4989fd`), then
/// the queued lines shown (`0x498a03` → `0x49d230`), each that can, in the order they were held, at
/// the cascade's stamp, with the host lent; one still waiting on its name stays held. The AFK
/// mirror's reseed beside it is [`super::away`]'s. With no VM the latch clears and the lines wait.
pub(crate) fn release_display(mut firing: ChatFiring) {
    let ChatFiring { vm, log, reads } = &mut firing;
    log.q().latched = None;
    let (Some(script), set) = vm.parts() else {
        return;
    };
    let at = crate::lanes::firing_now().unwrap_or_else(crate::lanes::stamp);
    release(reads, log, script, set, at, Release::Display, 0);
}

/// The leave-world sweep's chat leg (`0x490a9e` → `0x498a10`): the display latch set.
pub(super) fn latch_on_leave_world(log: Option<Res<ChatLog>>) {
    if let Some(log) = log {
        log.latch();
    }
}

/// One frame of the chat lane alone, for a test: the age pass, then every item.
#[cfg(test)]
pub(super) fn feed_chat(mut firing: ChatFiring) {
    firing.log.begin_frame();
    let until = crate::lanes::Until::all();
    while firing.log.peek().is_some_and(|s| until.admits(s)) {
        let head = firing.log.peek();
        firing.fire(until);
        if firing.log.peek() == head {
            return;
        }
    }
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
