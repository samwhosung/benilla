//! The chat sources → [`ChatEvent`] bridge: every inbound line — a decoded
//! `SMSG_MESSAGECHAT`, a channel notice, a `/random` roll, a client-composed loot/system line —
//! becomes one typed event here, names resolved ask-once through [`crate::names::NameCache`]
//! (a line whose sender name is still in flight re-checks each frame, bounded), then routes
//! through [`super::frames::route`] — the one composer/color/fan-out seam.

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

/// Give up re-checking a line's pending sender name after this many frames (a negative-cached or
/// genuinely-unknown guid never resolves; ~2s at 60fps is well past a normal name-query
/// round-trip). The line renders with a placeholder rather than being lost.
const NAME_MAX_TRIES: u16 = 120;

/// The text-emote sentence tables, read once off the patch chain.
#[derive(Resource)]
pub(crate) struct EmoteTexts(pub(crate) EmoteTextCatalog);

/// **The locale column is 0.** `[0xc0e080]` is the client's locale slot; only enUS is populated in
/// this install, and every other DBC catalog in the tree reads column 0 for the same reason.
const LOCALE: usize = 0;

/// Load `EmotesText.dbc` × `EmotesTextData.dbc`. `.after(benilla_assets::AssetSet::Open)` at the
/// call site is load-bearing for the reason [`super::channels::load_chat_channels`] records:
/// without it the patch chain does not exist yet and this silently loads nothing.
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
    /// A decoded `SMSG_MESSAGECHAT`: needs kind mapping + (player kinds) an ask-once name resolve.
    Wire { msg: ChatMessage, tries: u16 },
    /// A channel notice whose tail carries guids to resolve (`a` = affected/actor, `b` = the
    /// second actor of kicked/banned/unbanned; `0` = absent).
    Notice {
        notice: u8,
        channel: String,
        a_guid: u64,
        b_guid: u64,
        tries: u16,
    },
    /// An inbound addon line awaiting its SENDER's name — `CHAT_MSG_ADDON` (event 227, fired at
    /// `0x49a95f`). Parked here rather than fired at decode because the reference fires it
    /// *downstream* of the name resolve, from the `CMSG_NAME_QUERY` callback (`0x49ccc0`), so
    /// `sender` is a NAME and never a guid.
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
    /// A kill-XP award awaiting the victim's name ("%s dies, you gain %d experience." —
    /// COMBATLOG_XPGAIN_FIRSTPERSON). `bonus` = rested (total − base).
    XpGain {
        victim: u64,
        total: u32,
        bonus: u32,
        tries: u16,
    },
    /// An honor award awaiting the victim's name (`SMSG_PVP_CREDIT`) — the XP
    /// node's twin, parked for the identical reason: the sentence's first `%s` is a NAME and the
    /// packet carries a guid.
    ///
    /// `rank` is the victim's **internal** rank as the packet sent it (0 = none), turned into a
    /// title by the drain — which is the right place for it, because the title's GlobalString key
    /// needs the victim's own **side and sex**, and those come out of the very `NameCache` record
    /// the name resolve below is already waiting on (`SMSG_NAME_QUERY_RESPONSE` carries
    /// race/class/gender beside the name). Resolving it at push time would have needed the
    /// victim's descriptor, which is gone the moment their body despawns.
    HonorGain {
        victim: u64,
        honor: i32,
        rank: u8,
        tries: u16,
    },
    /// A text emote (`SMSG_TEXT_EMOTE`) awaiting the PERFORMER's name.
    ///
    /// Parked here for the reference's own reason, not merely for convenience: `0x49dbe0` composes
    /// the sentence immediately when the `NameCache` already holds the performer, and otherwise
    /// queues a `PENDINGTEXTEMOTE` node (`0x49cc00`) that the name-query callback (`0x49d0d0`)
    /// drains into the same composer. This queue *is* that node, and the bounded `tries` is its
    /// give-up edge.
    ///
    /// `target` is the raw wire name (empty = untargeted), never re-resolved — the reference
    /// passes the server's string straight into the format.
    TextEmote {
        performer: u64,
        text_id: u32,
        target: String,
        tries: u16,
    },
    /// An area discovery (`SMSG_EXPLORATION_EXPERIENCE`): the toast + conditional chat line pair
    /// (the drain fires them — the toast needs the script, which only [`feed_chat`] holds).
    Discovery { area: String, xp: u32 },
    /// One combat-log line, classified and family-picked at the packet, waiting only on the two
    /// endpoint names. The reference's deferred-message queue `DAT_00c4e208`, whose records
    /// carry a type tag and two guids for exactly this reason and are replayed by the name-ready
    /// callback `0x6294b0`.
    Combat(Box<super::combat::PendingCombat>),
    /// A ready event (client-composed lines; name-carrying notices).
    Event(ChatEvent),
}

/// Fire `CHAT_MSG_ADDON` with the reference's four arguments, in the reference's order.
///
/// Split out of the drain so the shape that reaches Lua is testable without standing up
/// `feed_chat`'s dozen resources — the scaffolding is not what can be wrong here; the argument
/// ORDER is. The reference fires `SignalEvent2(227, "%s%s%s%s", prefix, message, distribution,
/// sender)` (`0x49a95f`), and `BigWigs` independently self-delivers
/// `self:CHAT_MSG_ADDON("BigWigs", msg, "RAID", playerName)` — a 2006 addon author and the binary
/// agreeing.
///
/// Fired DIRECTLY rather than through [`route`]: `CHAT_MSG_ADDON` is not a `ChatTypeInfo` key and
/// carries four arguments against the chat family's ten, so the chat pipeline would mis-shape it
/// and `every_fired_event_name_is_a_chat_type_info_key` would rightly reject it.
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

/// One text emote's chat event — the sentence plus the performer name — or `None` when the
/// sentence table has nothing to say (the reference's `0x49b4bd` tail: SIT/STAND/TRAIN ship blank
/// in every locale, so a vanilla `/sit` prints no line at all).
///
/// Split out of the drain for [`fire_addon_message`]'s reason: the scaffolding around it is not
/// what can be wrong, the **shape** is — which sentence form the facts select, and which slot the
/// performer's name lands in.
///
/// **`sender` is the PERFORMER**, and it never reaches the rendered line: TEXT_EMOTE is one of the
/// verbatim families in `ChatFrame_OnEvent`, so the sentence *is* the line and this slot only ever
/// reaches addons as arg2. VERIFIED at the bytes: the last push before the event fire at
/// `0x49b495` is `[ebp-0x4]`, the `NameCache` record resolved for the performer at `0x49b289`
/// (`record+0` is the name string).
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

/// The pending chat items the net/loot/quest feeds fill and [`feed_chat`] drains. Cleared on
/// disconnect (a half-resolved line from a dead session must not leak into the next).
///
/// **Two queues, two drains.** [`Self::pending`] is the ordinary one. [`Self::broadcasts`] is the
/// world broadcasts' ([`super::broadcast`]), and it is separate for one reason: their resolve needs
/// the `AreaTable.dbc` catalog, the zone under the player and the joined-channel walk, which
/// [`feed_chat`] does not hold and cannot grow to hold (it is at Bevy's `SystemParam` ceiling).
/// `feed_broadcasts` drains this one a step earlier in the same schedule and pushes the resolved
/// lines back onto [`Self::pending`], so a broadcast still lands on the frame it decodes.
#[derive(Resource, Default)]
pub(crate) struct ChatLog {
    pending: Vec<Pending>,
    broadcasts: Vec<super::broadcast::Broadcast>,
    /// The ding's gain tuple, parked for `ui_unit`'s `PLAYER_LEVEL_UP` fire.
    ///
    /// The net layer has no `UiScript` — deliberately — so the gains arrive here, on the same
    /// net-to-UI channel `broadcasts` uses, and the feed that owns the level edge picks them up.
    /// They are matched BY LEVEL rather than just drained, because the trigger is a descriptor
    /// diff and the gains are packet-borne: the two land together today, and matching means a
    /// stale entry can never attach itself to a later ding if they ever stop doing so.
    level_up_gains: Vec<(LevelUpInfo, u32)>,
}

impl ChatLog {
    /// Queue one world broadcast for [`super::broadcast::feed_broadcasts`]'s resolve pass.
    pub(crate) fn push_broadcast(&mut self, b: super::broadcast::Broadcast) {
        self.broadcasts.push(b);
    }

    /// How many broadcasts are waiting — the drain's cheap early-out.
    pub(crate) fn broadcasts_pending(&self) -> usize {
        self.broadcasts.len()
    }

    /// Take the parked broadcasts, leaving the queue empty.
    pub(crate) fn take_broadcasts(&mut self) -> Vec<super::broadcast::Broadcast> {
        std::mem::take(&mut self.broadcasts)
    }

    /// Park the ding's gains for the `PLAYER_LEVEL_UP` fire (see [`ChatLog::level_up_gains`]).
    pub(crate) fn push_level_up_gains(&mut self, info: &LevelUpInfo, talent_points: u32) {
        self.level_up_gains.push((*info, talent_points));
    }

    /// The parked gains for `level`, removed from the queue. `None` when the level edge came from
    /// somewhere the packet did not — a GM demotion's descriptor write, or a first observation —
    /// in which case the caller fires zeros, which is what those gains actually are.
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

    /// Queue a ready, client-composed event (loot receive lines, quest/system lines, played
    /// time — [`ChatEvent::text_only`] covers the common case).
    pub(crate) fn push_event(&mut self, event: ChatEvent) {
        self.pending.push(Pending::Event(event));
    }

    /// Queue one combat-log line ([`super::combat`] did the classification at the packet; this
    /// only parks it for its names).
    pub(crate) fn push_combat(&mut self, line: super::combat::PendingCombat) {
        self.pending.push(Pending::Combat(Box::new(line)));
    }

    /// Queue a decoded `SMSG_CHANNEL_NOTIFY`. JOINED/LEFT become the ref's CHANNEL_JOIN/LEAVE
    /// *events* (a member line, hyperlinked — ChatFrame.lua's ChatTypeGroup["CHANNEL"]); every
    /// other notice becomes a CHANNEL_NOTICE composed by the `CHAT_<X>_NOTICE` law. Guid tails
    /// park here for the ask-once resolve; name tails are ready immediately.
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

    /// Queue an inbound addon line (`SMSG_MESSAGECHAT` carrying `LANG_ADDON`) for the sender-name
    /// resolve, then `CHAT_MSG_ADDON`.
    ///
    /// **The split is the counter-intuitive part and it is the reference's** (`0x49a8d0`): the text
    /// divides on its **FIRST** tab, and with **no tab at all the whole text is the PREFIX** and
    /// the message is empty — not the other way round, which is what a reimplementation guesses.
    ///
    /// `distribution` is the remap at `0x49aff4`: only PARTY / RAID / GUILD / BATTLEGROUND have
    /// names, and every other type byte reports `"UNKNOWN"` rather than being dropped — the
    /// reference hands the addon a string it can branch on either way.
    pub(crate) fn push_addon(&mut self, text: &str, chat_type: u8, guid: u64) {
        let (prefix, message) = match text.find('\t') {
            Some(i) => (text[..i].to_string(), text[i + 1..].to_string()),
            None => (text.to_string(), String::new()),
        };
        // **The protocol constants, not hand-copied bytes.** Three of the four literals here were
        // WRONG: RAID is `0x2` and this said `0x03`, GUILD is `0x3` and this said `0x04`, and
        // BATTLEGROUND is `0x5C` and this said `0x18`. The visible effect was not a dropped
        // message but a MISLABELLED one — a real guild addon message arrived at
        // `CHAT_MSG_ADDON` as `"RAID"`, while real raid and battleground traffic fell to
        // `"UNKNOWN"` — so an addon branching on the distribution acted on the wrong lane.
        //
        // The outbound half (`net::addon_wire_chat_type`) always used the named constants and its
        // own doc calls itself "the one place a distribution becomes a wire byte". It was not: this
        // was the other one, and it disagreed. Both ends now read the same symbols, so they cannot
        // drift apart again.
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

    /// Queue an XP award's chat line (`SMSG_LOG_XPGAIN` → CHAT_MSG_COMBAT_XP_GAIN; decision
    /// 0304). A named kill waits on the victim's name; everything else composes immediately
    /// (COMBATLOG_XPGAIN_FIRSTPERSON_UNNAMED "You gain %d experience.").
    pub(crate) fn push_xp_gain(&mut self, x: &XpGain) {
        // Both forks park. The unnamed one has no name to wait for — `victim: 0` is what says so
        // at the drain — but it still has a template to resolve, and the string table is the VM's.
        // One node, one composer, one place the wording can be wrong.
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

    /// Queue an honor award's chat line (`SMSG_PVP_CREDIT` → CHAT_MSG_COMBAT_HONOR_GAIN;
    /// decision 1512). A credit with **no victim guid** is the bonus/objective form and needs no
    /// resolve at all, so it composes here and now — the same fork [`Self::push_xp_gain`] takes.
    pub(crate) fn push_pvp_credit(&mut self, honor: i32, victim: u64, rank: u8) {
        // Both forks park, for [`Self::push_xp_gain`]'s reason: `victim: 0` is the award form and
        // waits for nothing, but its template still has to be resolved where the VM is.
        self.pending.push(Pending::HonorGain {
            victim,
            honor,
            rank,
            tries: 0,
        });
    }

    /// Queue a text emote's chat line (`SMSG_TEXT_EMOTE` → CHAT_MSG_TEXT_EMOTE) for
    /// the performer-name resolve, then the `EmotesText`/`EmotesTextData` composition.
    pub(crate) fn push_text_emote(&mut self, performer: u64, text_id: u32, target: String) {
        self.pending.push(Pending::TextEmote {
            performer,
            text_id,
            target,
            tries: 0,
        });
    }

    /// Queue an area discovery's announcement (`SMSG_EXPLORATION_EXPERIENCE`; decision 0828,
    /// surfaces corrected by the 0829 RE): the ERR_ZONE_EXPLORED toast fires on **every** packet
    /// (UIErrorsFrame, via `UI_INFO_MESSAGE` — never chat), and the ERR_ZONE_EXPLORED_XP chat
    /// system line rides **additionally** iff `xp > 0`. The caller resolved `area_name` from
    /// `AreaTable.dbc` by the packet's area id.
    pub(crate) fn push_exploration(&mut self, area_name: &str, xp: u32) {
        self.pending.push(Pending::Discovery {
            area: area_name.to_string(),
            xp,
        });
    }

    /// Disconnect: drop every pending item (mirrors the merchant/gossip/loot session clears).
    pub(crate) fn clear_session(&mut self) {
        self.pending.clear();
    }

    /// The parked addon lines as `(prefix, message, distribution)` — test-only, so the split and
    /// the remap can be asserted without standing up a name cache and a VM.
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

    /// How many parked items are headed for a CHAT WINDOW.
    ///
    /// Addon lines are excluded on purpose. They park in the same queue for the same ask-once name
    /// resolve, but they are not speech and never render — so counting them here would make
    /// `addon_chat_never_reaches_the_chat_window` fail the moment the receive half opened, which is
    /// exactly what it did. "Pending" stopped meaning "will render" when `Pending::Addon` arrived.
    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending
            .iter()
            .filter(|p| !matches!(p, Pending::Addon { .. }))
            .count()
    }

    /// The text of every already-composed line waiting to render, in order — the queue's
    /// [`Pending::Event`] entries, which is what everything that pushes a finished sentence
    /// ([`Self::push_event`]) lands as. The `Wire`/`Notice`/`Roll` shapes are deliberately absent:
    /// those have no text yet, and a caller asserting on a sentence wants only the ones that do.
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

/// The VM's own string table as a lookup — `getglobal(key)`, which is where every sentence this
/// feed shows comes from. Taken for exactly as long as one composition needs it,
/// so the `route` that follows can take the VM mutably.
fn globals(script: &benilla_ui::script::UiScript) -> impl Fn(&str) -> Option<String> + '_ {
    |key: &str| script.lua().globals().get::<String>(key).ok()
}

/// Fill a key's template, or nothing at all if the chain has no string for it — the reference's
/// data-suppression face, and the reason nothing here carries a fallback sentence.
fn keyed(
    get: &dyn Fn(&str) -> Option<String>,
    key: &str,
    args: &[benilla_ui::strings::Arg<'_>],
) -> Option<String> {
    let text = benilla_ui::strings::fill(&get(key)?, args);
    (!text.is_empty()).then_some(text)
}

/// The XP award's chat line: `COMBATLOG_XPGAIN_FIRSTPERSON`, its `_EXHAUSTION1` rested form, or
/// the `_UNNAMED` form (no victim).
///
/// **The rested template takes four arguments, not two** —
/// `"%s dies, you gain %d experience. (%s exp %s bonus)"` — and the last two are the bonus
/// *rendered with its sign* and the state WORD. That word is the one thing on this line the
/// reference does not keep in GlobalStrings: 1.12 ships no `EXHAUSTION_STATE*` key, so it comes
/// out of the client's own table and stays a literal here, with the same INTERIM it always had.
/// "Rested" is the only state the live server produces (the beta tired/exhausted penalties are
/// dead data); the client's table is the in-flight 0304's to pin.
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

/// The rest-state word the `_EXHAUSTION1` template's fourth argument takes. Not a GlobalString in
/// 1.12 — see [`xp_gain_line`].
const RESTED_STATE: &str = "Rested";

/// The honor line an `SMSG_PVP_CREDIT` becomes — the three GlobalStrings forms (COMBATLOG_HONORAWARD
/// :786, COMBATLOG_HONORGAIN :787, COMBATLOG_DISHONORGAIN :785).
///
/// **The fork** (formatter `0x625270`) is three-way, not two: no victim guid → AWARD; victim and
/// `honor > 0` → GAIN; victim and **`honor <= 0`** → DISHONOR. The boundary is `<=`, not `<` — a
/// zero-honor kill takes the dishonorable arm, which the earlier reading had on the honorable side.
///
/// An absent `rank_title` fills an EMPTY rank slot rather than dropping the clause. That is the
/// reference's own shape — it is precisely the emptiness vmangos floors a rankless victim's rank at
/// 5 to avoid showing (1512), and compensating for it a second time on our side would hide what the
/// server is actually sending.
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

/// The discovery toast — `ERR_ZONE_EXPLORED` ("Discovered: %s"), fired on every exploration packet
/// to the UIErrorsFrame (byte-verified: error-table route 1 → `AddErrorMessage 0x4945b0` →
/// UI_INFO_MESSAGE).
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

/// The discovery chat line — `ERR_ZONE_EXPLORED_XP` ("Discovered %s: %d experience gained"), fired
/// **in addition to** the toast iff the packet carried XP (byte-verified: the signed `jle` skip at
/// `0x5e422f`; route 0 → CHAT_MSG_SYSTEM).
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

/// Build the event a channel notice becomes: JOINED/LEFT → the member-line kinds; the rest →
/// CHANNEL_NOTICE (composed by [`super::frames::compose_notice`], the notice byte riding the
/// event's `notice` field).
pub(super) fn notice_event(
    notice_byte: u8,
    channel: String,
    a: Option<String>,
    b: Option<String>,
) -> Option<ChatEvent> {
    // The display type per notice byte — the `0x49c60c` jump table, read
    // off each arm's `mov edi`: 0x00/0x01 are the join/leave lines, MODE_CHANGE (0x0c) fires no
    // chat event at all, a byte past THROTTLED fires SYSTEM with an empty token, and the rest
    // split between CHANNEL_NOTICE (0x12) and CHANNEL_NOTICE_USER (0x13) exactly as listed.
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

/// Deliver one built event: the joined-list upkeep on **both sides** of the render, with the
/// channel stamp and [`route`] between them — the client's own split, and the reason a leave line
/// still knows its number.
///
/// **YOU_JOINED lands before, YOU_LEFT lands after.** The notice's arg4/arg7/arg8/arg9 are read off
/// the client's channel record ([`ChannelState::stamp_channel`] is our leg of that), so a record
/// torn down before the line is composed costs it the slot number — and, since the color resolves
/// through `ChatTypeInfo["CHANNEL"..arg8]` — the stock window's own Lua since 1948 — its color
/// with it.
/// The reference's YOU_LEFT arm only *flags* the teardown (`0x49c115 mov dword [ebp-0xc],1`); the
/// event fires first (`0x49c5b0 call 0x49a870`) and only then does `0x49c5b5 test` /
/// `0x49c5c2 call 0x49bbd0` destroy the record — VERIFIED by disassembly at those addresses (1275).
/// We removed first and printed "Left Channel: [General]" where the client prints "Left Channel:
/// [2. General - Elwynn Forest]"; an addon reading `GetChannelName` from its own handler also saw
/// the channel already gone, which the reference never shows it either.
///
/// **Unmodeled, and visible right there in that arm:** the teardown flag is the *YOU_LEFT* leg's
/// alone. Notice `0x03` splits on `rec+0x9c == 3` — the SUSPENDED leg (`0x49c0e9`) takes a
/// different token AND jumps past `0x49c115`, so a suspended channel keeps its record and its
/// number. We do not model that state field (it is the same one that makes `0x02` "YOU_CHANGED"),
/// so every `0x03` here is a genuine leave.
///
/// The two arms log, because this edge is where "we asked to join" becomes "the server says we are
/// in": [`super::channels`]'s walk only ever proves the request went out, and the round trip is
/// what actually arms an addon (it is the `CHAT_MSG_CHANNEL_NOTICE` Ace2's whole init gate waits
/// on). A join the server refuses is otherwise completely silent on this side. Each arm also
/// mirrors the list into the VM, where `GetChannelName` reads it (17 corpus sites across 6 addons):
/// these two are the only places it ever changes.
pub(super) fn deliver(
    script: &mut benilla_ui::script::UiScript,
    windows: &mut ChatWindows,
    channels: &mut ChannelState,
    event: &mut ChatEvent,
) {
    let notice = event
        .notice_byte()
        .filter(|_| event.kind == Some(ChatEventKind::ChannelNotice));
    // **First, before anything below moves it.** The reference's notice arms read `slot+0x9c` to
    // choose the token and only then write it (`0x49c0c2` reads, `0x49bb20` writes), and the two
    // alternates are what keep a renamed or suspended channel registered with the window
    // ([`super::event::notice_token`]).
    event.slot_state = channels.slot_state(&event.channel);
    if let Some(byte) = notice {
        // The one trace of the server's half of every join and leave — a probe log's only way to
        // tell "we asked" from "the server agreed" (decision 2144's live runs read it).
        debug!(
            "chat: channel notice {byte:#04x} for {:?} (slot {:?}, {:?})",
            event.channel,
            channels.number_of(&event.channel),
            event.slot_state
        );
    }
    if notice == Some(channel_notice::YOU_JOINED) {
        // The reference's `0x49bbaf`: the confirmed join is what sets the channel's
        // `ZONECHANNELS` bit, and it is the ONLY thing that grows that mask at runtime.
        // Outside the slot claim below because it is not about slots — a
        // re-confirmation of a channel we already number still owns the bit.
        channels.note_zone_channel_joined(&event.channel);
        // …and a custom channel's confirmed join is what enters the durable re-join list the
        // chat cache writes (`ChannelState::custom`), the same "server agreed" edge.
        channels.note_custom_channel_joined(&event.channel);
        // …and the slot the walk RENAMED is already numbered, so the claim below skips it — but
        // its state still has to come back to `Joined`, or the next notice on that row reads as
        // another rename. A no-op for a channel we hold no slot for, which is the claim's case.
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
            // The reference's own ceiling, reached: ten slots, all taken. It answers with a chat
            // error and no record, so the channel stays unnumbered here too.
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
    // A member-line / notice channel renders numbered when we know its slot.
    channels.stamp_channel(event);
    route(script, windows, event);
    if let Some(name) = leaving {
        // **A suspended slot survives its own leave** (`0x49c0e0`). The reference's
        // `0x03` arm jumps past the teardown (`0x49c115`) when the record is in state 3, so walking
        // out of a capital keeps `Trade`'s record AND its number — which is what lets walking back
        // in re-join through the state-3 bypass, and what stops the stock handler deregistering it.
        if event.slot_state == Some(super::edit::SlotState::Suspended) {
            debug!("chat: channel {name:?} suspended — the record and its number stay");
        } else {
            // Cleared in place, never compacted: slot 2 going empty must not make slot 3 into 2
            // ([`ChannelState`], 1286).
            let freed = channels.free_slot(&name);
            debug!("chat: server confirms channel {name:?} left (slot {freed:?} now free)");
            script.set_joined_channels(channels.names());
        }
    }
}

/// What the **speaker** does when a line lands — the over-the-head bubble and the talk/laugh
/// gesture, bundled because the reference arms both from the one display path (`ChatFrame.cpp`) and
/// because a Bevy system takes at most sixteen parameters, which [`feed_chat`] had already reached.
///
/// They stay separate mechanisms inside the bundle: the bubble has a 20 yd range test and two CVars
/// ([`crate::chat_bubble`]), the gesture has neither.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct SpeakerEffects<'w> {
    bubbles: ResMut<'w, crate::chat_bubble::BubbleQueue>,
    bubble_cfg: Res<'w, crate::chat_bubble::BubbleConfig>,
    gestures: ResMut<'w, crate::creature_anim::GestureQueue>,
}

/// Drain [`ChatLog`]: resolve names (ask-once, bounded), build events, [`route`] them. Also ticks
/// the whisper-chime throttle.
pub(super) fn feed_chat(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut log: ResMut<ChatLog>,
    mut windows: ResMut<ChatWindows>,
    mut channels: ResMut<ChannelState>,
    names: Res<NameCache>,
    mut speaker: SpeakerEffects,
    commands: Res<NetCommands>,
    // The text-emote sentence seam: the tables, plus the guid the composer's
    // "are you the performer?" test compares against.
    emote_texts: Option<Res<EmoteTexts>>,
    self_guid: Res<SelfGuid>,
    // The `$`-macro subject seam: monster/BG lines expand against the guid the line is ADDRESSED to,
    // which needs the object index + the streamed unit's descriptors. See [`macro_subject`].
    guids: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    states: Res<crate::world_state::WorldStates>,
    // The language gate: the word pool + this character's fluency + the GM bit.
    langs: Res<super::language::ChatLanguages>,
    // The combat log's item-name seam (1703): the four families whose sentence names an ITEM
    // (`TRADESKILL_LOG`, `FEEDPET_LOG`, `ITEMENCHANTMENT*`, `SPELLDURABILITYDAMAGE`) resolve it
    // here, through the same ask-once cache the reference's own deferred queue re-runs against.
    items: Res<crate::items::Items>,
    // The two 1.12 text filters, bundled for the same reason `SpeakerEffects` is:
    // this list is at the sixteen-parameter ceiling. This IS the reference's `0x49a870`, so both
    // arms belong here and nowhere else.
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
                // The channel's base name; the numbered display form, its slot number and its
                // zone id are stamped on below ([`ChannelState::stamp_channel`]) — arg4/arg7-arg9.
                let channel_base = msg.channel.clone().unwrap_or_default();
                // `$`-macro expansion (corrected by 0759): the reference runs its one
                // server-text expander over the monster/boss + BG-system types and nothing else,
                // against the guid the line is ADDRESSED to. Every other type reaches the frame
                // verbatim.
                //
                // The FAILURE arms are the reference's, and they are not what a panel does: the chat
                // path never puts a `$` on screen. VERIFIED `0x49dac2-0x49db1e` (mirrored at
                // `0x49d9c9`): expanded → show; failed with a zero subject → **drop the line**;
                // failed with the name already known → **drop** (a known name means retrying cannot
                // help); failed with the name still unknown → hold the RAW text, let the name query
                // run, and re-expand when it answers — only a second failure there shows the raw
                // text. Our bounded `tries` retry IS that hold, and exhausting it is that second
                // failure.
                let expanded = if MACRO_EXPANDED_TYPES.contains(&msg.chat_type) {
                    // The addressee where the shape carries one, else the only guid it has (the
                    // `default:`-shaped BG_SYSTEM lines have no target slot at all).
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
                        continue; // the reference drops it — no line at all
                    } else if tries < NAME_MAX_TRIES {
                        // Name query is already in flight (`subject_for_guid` issued it); hold the
                        // raw line and re-expand when it lands.
                        still.push(Pending::Wire {
                            msg,
                            tries: tries + 1,
                        });
                        continue;
                    } else {
                        None // the post-query second failure: show the raw text verbatim
                    }
                } else {
                    None
                };
                let plain = expanded.unwrap_or_else(|| msg.text.clone());
                // **The language gate**. The wire always
                // carries plaintext; whether this character can read it is entirely ours to decide,
                // and the answer is one rewritten buffer that EVERY consumer below shares — the
                // chat line, the Lua `arg1`, the bubble, the gesture. That is the reference's own
                // shape: `0x49a870` fills `[ebp-0xd0c]` exactly once (either a plain `SStrCopy` at
                // `0x49a9f0` or the garble at `0x49aa7c`) and never reads the raw wire pointer
                // again, so an addon receiving a foreign-language line cannot recover the
                // plaintext. Ours cannot either, deliberately.
                let language = langs.effective_language(msg.chat_type, msg.language);
                let mut text = langs.garble(language, &plain);

                // ── The two text filters, in the reference's own order ─────────
                //
                // They run HERE, on the one buffer the garble just filled, because that is where
                // `0x49a870` runs them: it fills `[ebp-0xd0c]` once (plain copy or garble) and
                // both arms operate on that buffer, so a filtered line is filtered in the language
                // the viewer actually reads.
                //
                // **Spam first, mask second, fire third**, and the order is not cosmetic: a line
                // the spam filter drops is never masked, so its characters must not consume mask
                // indices — the mask phase is a process-global that carries across every message
                // and subsystem, and masking before deciding to drop would drift it.
                if text_filter.should_drop(msg.chat_type, msg.chat_tag, langs.is_gm(), &text) {
                    // **Nothing is shown in its place.** `0x49ab33`'s not-a-whisper leg jumps to
                    // `0x49afd7`, the bare epilogue — no event, no substitute text, no chat line.
                    //
                    // NOT built, and named rather than faked: the whisper leg. A dropped
                    // `CHAT_MSG_WHISPER` with a non-zero report guid also sends `CMSG_CHAT_FILTERED`
                    // (`0x331`) before returning — still firing no event, so the player sees the
                    // same nothing either way. The packet's BODY is not settled (`0x508680`'s
                    // send-path verdict is still open), vmangos has no handler for the
                    // opcode, and inventing a body to send a server that ignores it would be worse
                    // than the honest gap.
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
                    // The EFFECTIVE language, not the wire's: a narration type and GM mode both
                    // force it to 0, which is one decision in the reference rather than two — the
                    // `[Language]` header keys off this same field, so suppressing the garble
                    // suppresses the header with it.
                    language: language_name(language).to_string(),
                    channel: channel_base,
                    flag: flag_of_tag(msg.chat_tag).to_string(),
                    ..Default::default()
                };
                channels.stamp_channel(&mut event);
                route(&mut script, &mut windows, &event);
                // The speech bubble spawns the moment the line routes — the reference's
                // `0x49acd9` sits in the same SMSG display path ([`crate::chat_bubble`]).
                if let Some(kind) = event.kind {
                    // The bubble shows the same expanded line the feed does — the reference's
                    // bubble spawn sits inside this same SMSG display path, downstream of the
                    // expander, so a `$n` must never survive into it either.
                    speaker
                        .bubbles
                        .push(&speaker.bubble_cfg, msg.sender_guid, kind, &text);
                }
                // …and the speaker gestures. Independent of the bubble in the reference too
                // (different function, different gates: the bubble has a 20 yd range test and two
                // CVars, the gesture has neither). The selector reads the RAW wire type and the
                // expanded text, and takes its laugh words off the player's own FrameXML globals —
                // the enumeration is the mechanism, the words are content.
                //
                // **It reads `plain`, NOT the garbled text.** The selector is not on
                // this display path at all: it lives in the *parser* `0x49d560` at
                // `0x49d820`-`0x49d8ae`, and the slot it matches against — `[ebp-0x10]` — is the
                // very buffer `0x49dbc2` then hands to `0x49a870` as its `src`. The garbled buffer
                // is a local of a frame that does not exist yet.
                //
                // So the gesture is **language-independent**: a Horde player yelling `lol` laughs
                // for every observer, Alliance included. We had this wired to the garbled text on
                // an inference from the consumer census — that census was complete for
                // `0x49a870` and could never have found a consumer reading the pre-garble value in
                // the *caller's* frame.
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
                // NOT stamped, deliberately — and this arm is inconsistent with the other two
                // because of it. A guid-tail notice (a join/leave member line, a kick, a
                // moderation change) reaches the composer with its channel name UNNUMBERED, so it
                // renders "[World] Ann joined channel." where the same channel's speech renders
                // "[1. World]". The reference numbers both: `ChatFrame_OnEvent` l.1463 strips only
                // the " - Zone" tail from arg4, never the number.
                //
                // Adding `channels.stamp_channel(&mut event)` here fixes it in one line — and
                // changes what the player sees, which this pass is not allowed to do. Left for the
                // director's call; the cost of leaving it is that arg7/arg8/arg9 are 0/0/empty on
                // these events alone.
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
                // RANDOM_ROLL_RESULT = "%s rolls %d (%d-%d)" (GlobalStrings:3290).
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
                // `victim == 0` is the UNNAMED form — nothing to resolve, straight to the fill.
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
                // `victim == 0` is the AWARD form — no name, no rank title, nothing to wait for.
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
                // **The side is the VICTIM's and the gender is OURS**, and that asymmetry is the
                // reference's own (`0x625270`): the team digit is computed inline over the victim's
                // faction template, while the gendered GlobalString resolve runs against the local
                // player. It reads like a bug in the real client and it is what the bytes do. The
                // victim's side rides the same name-query record we just resolved the name from;
                // ours rides our own.
                //
                // A victim with no record is a creature — a racial leader is the only one the
                // server ranks, at 19, and "Leader" is the same word on both sides — or an
                // unanswered guid, so the side digit cannot change the answer there.
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
                // Both names resolve ask-once: the performer's is the sentence's `%s`, and our own
                // is what the "is the target me?" compare needs (`GetOwnName` in the reference,
                // which never has to wait for it — we can, so we park like every other line).
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
                // No name for the performer ⇒ **no line**, not an "Unknown" one: the reference
                // bails outright when `NameCache::GetRecord` misses (`0x49b28c`), and that cache
                // is player-only. The emote is still an animation and a voice — those rode the
                // `EmoteMessage` path at decode and do not depend on this.
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
                        // Sex 1 = Female, from the same `SMSG_NAME_QUERY_RESPONSE` the name came
                        // from — exactly where the reference reads it (`record+0x13c`).
                        performer_female: names
                            .player_traits(performer)
                            .is_some_and(|(_, _, sex)| sex == 1),
                        target: &target,
                        your_name: your_name.as_deref().unwrap_or_default(),
                    },
                );
                // A dry ladder is a real outcome, not a failure: SIT/STAND/TRAIN ship blank in
                // every locale, so a vanilla `/sit` prints nothing.
                let Some(event) = event else { continue };
                route(&mut script, &mut windows, &event);
            }
            Pending::Combat(mut line) => {
                // Both names, ask-once. The reference's queue holds the record until its
                // creature-name DBC row is loaded and replays it then; ours holds it until the
                // name query answers, bounded by the same `tries` budget every other pending item
                // uses so a guid the server will never name cannot pin the queue.
                // Guid 0 means the arm already put the name in the fills (`/chattest`); any other
                // guid is asked for, once, and the line waits.
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
                // The `Named` slot, on the same terms as the endpoints. A cached NEGATIVE on an
                // item (the server does not know the entry) is not something to wait on — it would
                // pin the line for the whole `tries` budget and then drop it — so it composes with
                // whatever the arm left in `named`, one level down the reference's own
                // `"UKNOWNOBJECT"` degrade.
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

/// Whether a wire chat type carries a **player guid** whose name must be resolved (vs a monster
/// type with its name inline, or a nameless system line). The player-message families, now
/// including the tagged self-notice types (AFK/DND auto-replies, IGNORED) and the raid/BG set.
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

/// **`RequestTimePlayed()` out, `TIME_PLAYED_MSG` back** — the two halves of `/played` for an addon.
///
/// The request is a count-drain ([`benilla_ui::script::UiScript::take_played_time_asks`], the pvp
/// queue's shape): the packet is empty, so two asks are two sends. The answer is a one-slot mailbox
/// the net apply pass fills, and it is delivered as the event rather than a return value, because
/// that is how the API answers — `RequestTimePlayed()` itself returns nothing.
///
/// **This does NOT replace the chat breakdown beside it.** `ui_chat::net::played_time` prints
/// the TIME_PLAYED_TOTAL/LEVEL lines because we do not ship `ChatFrame_DisplayTimePlayed`, which is
/// what the reference's own `TIME_PLAYED_MSG` handler does. The two are the reference's two
/// consumers of one packet, not a doubling: an addon that registers the event does its own thing
/// with the numbers, and the player still sees `/played` answer in chat.
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
