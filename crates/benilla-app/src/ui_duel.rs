//! Duels — the challenge, the countdown, the bounds timer, and the outcome line.
//!
//! The smallest complete multiplayer verb: no window of its own, two StaticPopups, four events,
//! four Era globals, and one descriptor-driven consequence — a duel opponent reads *hostile*, so
//! the whole targeting/attack stack lights up without a special case (see
//! [`crate::target::ring`]'s duel leg).
//!
//! **Both ends of every mechanism here are byte-pinned** — vmangos for what the server sends,
//! WoW.exe's own duel TU (`E:\build\buildWoW\WoW\Source\Ui\DuelInfo.cpp`, handlers registered at
//! `0x4d4710`) for what the client does with it. The laws this module implements, each with its
//! address:
//!
//! - **The challenge is symmetric** (`0x4d49d0`). `SMSG_DUEL_REQUESTED` goes to challenger and
//!   challenged alike. The client stores the arbiter guid, then compares the challenger guid
//!   against its own: **equal** ⇒ show `ERR_DUEL_REQUESTED` ("You have requested a duel.") and
//!   **immediately send `CMSG_DUEL_ACCEPTED`** (`call 0x4d4830` — a no-op server-side, which
//!   drops an accept from the initiator, but it is what goes on the wire); **different** ⇒ resolve
//!   the challenger's player object and fire `DUEL_REQUESTED` with its name.
//! - **The countdown is client-driven** (`0x4d4ae0` → `0x4d4930`, timer named `ProcessCountdown`).
//!   The wire carries milliseconds; the client divides by 1000, prints
//!   `format(DUEL_COUNTDOWN, n)` as a **CHAT_MSG_SYSTEM chat line** — not an on-screen banner —
//!   then decrements and re-arms a 1000 ms timer while non-zero. vmangos's 3000 therefore reads
//!   "Duel starting: 3 / 2 / 1" one second apart.
//! - **Completion** (`0x4d4b20`). Only acts if an arbiter is held. `started == 0` (the duel never
//!   began) additionally shows `ERR_DUEL_CANCELLED` ("Duel cancelled."). Either way the arbiter is
//!   cleared, `DUEL_FINISHED` fires, and any running countdown is cancelled.
//! - **The outcome line** (`0x4d4ba0`) is composed client-side from `SMSG_DUEL_WINNER`'s flag and
//!   two names against `DUEL_WINNER_KNOCKOUT` / `DUEL_WINNER_RETREAT`, and printed as
//!   CHAT_MSG_SYSTEM. The server broadcasts it to everyone nearby, so bystanders read it too.
//! - **Bounds** (`0x4d4aa0` / `0x4d4ac0`) are bare event fires; the 10 s forfeit clock and its
//!   text belong to the `DUEL_OUTOFBOUNDS` popup, and the *enforcement* is entirely the server's
//!   (`Player::CheckDuelDistance`: 75 yd out, 70 yd back in, 10 s to return).
//!
//! One deviation, stated rather than hidden (the ignore branch — `0x4d4a33`'s silent
//! `CMSG_DUEL_CANCELLED` for a challenge from an ignored player — is implemented as of decision
//! 0668, which supplied the ignore list it needs):
//!
//! 1. **The challenger's name is resolved asynchronously.** The reference reads it off the
//!    challenger's `CGPlayer` and fires nothing at all if the object is missing (`0x4d4a72`).
//!    benilla resolves through the [`NameCache`], which may need a `CMSG_NAME_QUERY` round trip —
//!    so the popup can arrive a frame or two late instead of never. In practice the challenger is
//!    always streamed (the duel spell's range is 10 yd), which is exactly the case where the
//!    reference has the name for free.

use benilla_ui::script::{DuelRequest, ScriptValue, UiScript};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, SelfPlayer};
use crate::target::Selection;
use crate::ui_action::Spells;
use crate::ui_script::{UiFeed, UiInput};

/// `SPELL_EFFECT_DUEL` — the `Effect[0]` value that identifies the duel spell in the player's own
/// spellbook (7266 "Duel" on 1.12 data, granted to every race/class at creation). The reference
/// does exactly this lookup rather than hardcoding the id: its spell-learned walk stores the id
/// of any learned spell whose `SpellRec+0xf4` is `0x53` into the duel-spell global `[0xb71130]`
/// (`0x4b2605`), which `StartDuel`/`StartDuelUnit` then cast (`0x4d4810`). We mirror the
/// mechanism, so a data change moves with the data.
const SPELL_EFFECT_DUEL: u32 = 83;

/// A line this window owes but cannot yet write: the VM that holds the string table is
/// [`feed_duel`]'s, not the net drain's (the text is the player's `GlobalStrings`,
/// so it is resolved where the VM is, and named by key everywhere else).
///
/// **These three keys are not message-catalog rows** — `DUEL_COUNTDOWN` and the two
/// `DUEL_WINNER_*` appear nowhere in the reference's FrameXML either, which is what marks them
/// engine-composed. So they cannot ride `UiErrorKeys`: an unknown key takes `Shown::keyed`'s
/// fallback and turns the line red. They go out as `Shown::unkeyed` on the system chat channel
/// they have always used (2054's `/ginfo` shape). The two `ERR_DUEL_*` lines beside them ARE rows
/// and take the other road entirely.
enum OwedLine {
    /// `DUEL_COUNTDOWN` ("Duel starting: %d") with the seconds left.
    Countdown(u32),
    /// `DUEL_WINNER_RETREAT` when the loser fled, else `DUEL_WINNER_KNOCKOUT`.
    ///
    /// Both templates are **positional** — `"%1$s has defeated %2$s in a duel"` against
    /// `"%2$s has fled from %1$s in a duel"` — so the same (winner, loser) pair fills them in
    /// opposite orders. It is the canonical case for why an ordered fill is not enough and why a
    /// re-typed English sentence cannot carry the rule at all.
    Winner {
        fled: bool,
        winner: String,
        loser: String,
    },
}

/// The duel session mirror. Written by the net drain's duel arms, read by [`feed_duel`] (which
/// fires the Era events on its edges) and [`tick_countdown`]. Cleared on disconnect beside the
/// other per-login resources.
#[derive(Resource, Default)]
pub(crate) struct DuelState {
    /// Lines composed here but resolvable only where the VM is — [`feed_duel`] drains them
    /// (see [`OwedLine`]). The `ui_guild`/`ui_petition` shape.
    owed: Vec<OwedLine>,
    /// The duel-flag GameObject guid identifying the pending or running duel; `0` = none. This is
    /// the client's `[0xb73240]`: set by the request, echoed on accept/cancel, cleared only by
    /// completion. Its non-zero→zero edge is what fires `DUEL_FINISHED`.
    pub(crate) arbiter: u64,
    /// The challenger's guid while a challenge popup is owed to the UI — `None` once fired (or
    /// when we are ourselves the challenger, who never gets a popup).
    challenger: Option<u64>,
    /// We are outside the duel bubble; drives the `DUEL_OUTOFBOUNDS`/`DUEL_INBOUNDS` edge pair.
    out_of_bounds: bool,
    /// The running "Duel starting: N" countdown: ticks left to print, and the 1 s re-arm.
    countdown: Option<Countdown>,
}

/// The `ProcessCountdown` timer's state (`0x4d4930`): a remaining count and a one-second re-arm.
struct Countdown {
    remaining: u32,
    tick: Timer,
}

impl DuelState {
    /// `SMSG_DUEL_REQUESTED`. Returns the error line to display when the challenge is our own —
    /// the reference's `DisplayError(ERR_DUEL_REQUESTED)` — and leaves [`Self::challenger`] set
    /// for the feed to turn into the popup when it is someone else's.
    ///
    /// The caller sends the challenger's auto-accept: the reference's own request handler calls
    /// `AcceptDuel` inline (`0x4d4a12`), so the accept is on the wire before any UI runs.
    fn apply_requested(&mut self, arbiter: u64, challenger: u64, own: Option<u64>) -> bool {
        self.arbiter = arbiter;
        let ours = Some(challenger) == own;
        self.challenger = (!ours).then_some(challenger);
        ours
    }

    /// Take the challenger owed a response, if any — the partner probe's accept hook
    /// (`WOW_PROBE=partner`). Taking it discharges the popup
    /// debt exactly as the feed's `DUEL_REQUESTED` edge does, so the probe never leaves a dialog
    /// owed to a UI it isn't driving.
    pub(crate) fn take_challenger(&mut self) -> Option<u64> {
        self.challenger.take()
    }

    /// `SMSG_DUEL_COMPLETE`. Returns whether the "Duel cancelled." line is owed (the duel never
    /// started). Gated on holding an arbiter, exactly like `0x4d4b20` — a stray completion with
    /// no duel in flight is silent. Cancels any countdown unconditionally, which the reference
    /// does *outside* that gate.
    fn apply_complete(&mut self, started: bool) -> bool {
        self.countdown = None;
        if self.arbiter == 0 {
            return false;
        }
        self.arbiter = 0;
        self.challenger = None;
        self.out_of_bounds = false;
        !started
    }

    /// `SMSG_DUEL_COUNTDOWN`. A zero count arms nothing: the reference would print "Duel starting:
    /// 0" and then re-arm forever on the `dec`'s wraparound, an unreachable path on 1.12 servers
    /// (vmangos always sends 3000) that is not worth reproducing.
    fn apply_countdown(&mut self, seconds: u32) {
        self.countdown = (seconds > 0).then(|| Countdown {
            remaining: seconds,
            tick: Timer::from_seconds(1.0, TimerMode::Repeating),
        });
    }
}

/// Resolve one owed line against the VM's own string table. `None` = no string, no line — the
/// reference's data-suppression face.
fn owed_text(line: &OwedLine, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    use benilla_ui::strings::{fill, Arg};
    let text = match line {
        OwedLine::Countdown(n) => fill(&get("DUEL_COUNTDOWN")?, &[Arg::D(i64::from(*n))]),
        OwedLine::Winner {
            fled,
            winner,
            loser,
        } => {
            let key = if *fled {
                "DUEL_WINNER_RETREAT"
            } else {
                "DUEL_WINNER_KNOCKOUT"
            };
            // Winner first, loser second — always. Which one the sentence *names* first is the
            // template's business, through its `%1$s`/`%2$s`.
            fill(&get(key)?, &[Arg::S(winner), Arg::S(loser)])
        }
    };
    (!text.is_empty()).then_some(text)
}

/// The duel's packet handlers (in the net handler table since 2312), beside the
/// state they drive: the session mirror + the two `DisplayError` lines the handlers emit inline;
/// the Era events fire off the mirror's edges in [`feed_duel`], and the countdown ticks in its own
/// system. `own` is our own guid.
pub(crate) mod net {
    use super::*;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    /// Register the duel's handlers — called from [`UiDuelPlugin`]. One per kind, plus the
    /// session-end listener.
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::DuelRequested, on_requested)
            .net_handler(K::DuelOutOfBounds, on_bounds)
            .net_handler(K::DuelInBounds, on_bounds)
            .net_handler(K::DuelComplete, on_complete)
            .net_handler(K::DuelWinner, on_winner)
            .net_handler(K::DuelCountdown, on_countdown)
            .net_handler(K::Disconnected, on_session_end);
    }

    fn on_requested(
        In(ev): In<SessionEvent>,
        mut duel: ResMut<DuelState>,
        mut errors: ResMut<crate::ui_action::UiErrorKeys>,
        commands: Res<NetCommands>,
        self_guid: Res<crate::net::SelfGuid>,
        social: Res<crate::ui_social::SocialState>,
    ) {
        if let SessionEvent::DuelRequested {
            arbiter,
            challenger,
        } = ev
        {
            requested(
                &mut duel,
                &mut errors,
                &commands,
                arbiter,
                challenger,
                self_guid.0,
                social.is_ignored(challenger),
            );
        }
    }

    fn on_bounds(In(ev): In<SessionEvent>, mut duel: ResMut<DuelState>) {
        match ev {
            SessionEvent::DuelOutOfBounds => bounds(&mut duel, true),
            SessionEvent::DuelInBounds => bounds(&mut duel, false),
            _ => {}
        }
    }

    fn on_complete(
        In(ev): In<SessionEvent>,
        mut duel: ResMut<DuelState>,
        mut errors: ResMut<crate::ui_action::UiErrorKeys>,
    ) {
        if let SessionEvent::DuelComplete { started } = ev {
            complete(&mut duel, &mut errors, started);
        }
    }

    fn on_winner(In(ev): In<SessionEvent>, mut duel: ResMut<DuelState>) {
        if let SessionEvent::DuelWinner {
            fled,
            winner: won,
            loser,
        } = ev
        {
            winner(&mut duel, fled, &won, &loser);
        }
    }

    fn on_countdown(In(ev): In<SessionEvent>, mut duel: ResMut<DuelState>) {
        if let SessionEvent::DuelCountdown { seconds } = ev {
            countdown(&mut duel, seconds);
        }
    }

    /// A pending challenge, a running duel, and its countdown all die with the socket (decision
    /// 0633) — the server drops the duel too (`Player::DuelComplete(DUEL_FLED)` on logout), and a
    /// stale arbiter guid would make the next AcceptDuel echo a dead object. A listener on the
    /// session end (a second handler on the kind, after the bridge's own teardown).
    fn on_session_end(In(_): In<SessionEvent>, mut duel: ResMut<DuelState>) {
        *duel = DuelState::default();
    }

    /// `SMSG_DUEL_REQUESTED` — store the arbiter, and either own the challenge (error line +
    /// immediate accept) or hand the popup to the feed.
    ///
    /// `ignored` short-circuits both: a challenge from a player on the ignore list is answered
    /// with `CMSG_DUEL_CANCELLED` and nothing is shown (`0x4d4a33`). This is the branch decision
    /// 0633 recorded as absent for want of an ignore list; decision 0668 supplies one.
    pub(crate) fn requested(
        duel: &mut DuelState,
        errors: &mut crate::ui_action::UiErrorKeys,
        commands: &NetCommands,
        arbiter: u64,
        challenger: u64,
        own: Option<u64>,
        ignored: bool,
    ) {
        if ignored {
            let _ = commands.0.send(ClientCommand::DuelCancelled { arbiter });
            return;
        }
        if duel.apply_requested(arbiter, challenger, own) {
            errors
                .0
                .push(crate::ui_action::UiError::key("ERR_DUEL_REQUESTED"));
            let _ = commands.0.send(ClientCommand::DuelAccepted { arbiter });
        }
    }

    /// `SMSG_DUEL_COMPLETE` — clear the session, cancel the countdown, and raise
    /// `ERR_DUEL_CANCELLED` when the duel never started.
    pub(crate) fn complete(
        duel: &mut DuelState,
        errors: &mut crate::ui_action::UiErrorKeys,
        started: bool,
    ) {
        if duel.apply_complete(started) {
            errors
                .0
                .push(crate::ui_action::UiError::key("ERR_DUEL_CANCELLED"));
        }
    }

    /// `SMSG_DUEL_WINNER` — the outcome line, owed to the feed.
    pub(crate) fn winner(duel: &mut DuelState, fled: bool, winner: &str, loser: &str) {
        duel.owed.push(OwedLine::Winner {
            fled,
            winner: winner.to_string(),
            loser: loser.to_string(),
        });
    }

    /// `SMSG_DUEL_COUNTDOWN` — arm the client-side tick.
    pub(crate) fn countdown(duel: &mut DuelState, seconds: u32) {
        duel.apply_countdown(seconds);
    }

    /// `SMSG_DUEL_OUTOFBOUNDS` / `SMSG_DUEL_INBOUNDS` — the feed turns the edge into its event.
    pub(crate) fn bounds(duel: &mut DuelState, outside: bool) {
        duel.out_of_bounds = outside;
    }
}

/// Fire the four duel events on their state edges — the [`crate::ui_party::feed`] pattern: the
/// drain mutates [`DuelState`], the feed diffs it against what it last announced.
fn feed_duel(
    script: Option<NonSendMut<UiScript>>,
    mut duel: ResMut<DuelState>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
    mut fed: Local<crate::ui_script::VmMemo<FedDuel>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let fed = fed.get(&script);

    // The owed countdown/outcome lines, resolved against the VM's own string table. They land
    // ahead of the event edges below for the same reason the social feed's do: the line describes
    // the transition the event announces.
    let owed = std::mem::take(&mut duel.owed);
    let lines: Vec<crate::ui_action::Shown> = {
        let get = |key: &str| script.lua().globals().get::<String>(key).ok();
        owed.iter()
            .filter_map(|line| {
                owed_text(line, &get).map(|text| {
                    crate::ui_action::Shown::unkeyed(benilla_ui::messages::MsgKind::Chat, text)
                })
            })
            .collect()
    };
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_duel", lines);

    // DUEL_REQUESTED(challengerName) — held until the name resolves (module doc, deviation 2).
    if let Some(guid) = duel.challenger {
        if let Some(name) = names.resolve(guid, &commands).map(str::to_string) {
            script.fire_event("DUEL_REQUESTED", vec![ScriptValue::Str(name)]);
            duel.challenger = None;
        }
    }

    // DUEL_FINISHED on the arbiter's non-zero→zero edge (`0x4d4b51`).
    let held = duel.arbiter != 0;
    if fed.held && !held {
        script.fire_event("DUEL_FINISHED", Vec::new());
    }
    fed.held = held;

    // The bounds pair (`0x4d4aa0` / `0x4d4ac0`).
    if fed.out_of_bounds != duel.out_of_bounds {
        fed.out_of_bounds = duel.out_of_bounds;
        let event = if duel.out_of_bounds {
            "DUEL_OUTOFBOUNDS"
        } else {
            "DUEL_INBOUNDS"
        };
        script.fire_event(event, Vec::new());
    }
}

/// What [`feed_duel`] last announced.
#[derive(Default)]
struct FedDuel {
    held: bool,
    out_of_bounds: bool,
}

/// `ProcessCountdown` (`0x4d4930`): print "Duel starting: N" now, then once a second while the
/// count is non-zero. The first line lands the frame the packet arrives — the reference calls the
/// tick body directly before arming the timer — so the timer here starts already elapsed.
fn tick_countdown(time: Res<Time>, mut duel: ResMut<DuelState>, mut started: Local<bool>) {
    let Some(countdown) = duel.countdown.as_mut() else {
        *started = false;
        return;
    };
    let fire = if *started {
        countdown.tick.tick(time.delta()).just_finished()
    } else {
        *started = true;
        true
    };
    if !fire {
        return;
    }
    let remaining = countdown.remaining;
    countdown.remaining -= 1;
    let done = countdown.remaining == 0;
    duel.owed.push(OwedLine::Countdown(remaining));
    if done {
        duel.countdown = None;
        *started = false;
    }
}

/// Drain the Era API's duel intents into their sends.
fn drain_duel(
    script: Option<NonSendMut<UiScript>>,
    duel: Res<DuelState>,
    spells: Option<Res<Spells>>,
    actions: Res<crate::ui_action::PlayerActions>,
    selection: Res<Selection>,
    index: Res<GuidIndex>,
    self_q: Query<&Guid, With<SelfPlayer>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    let requests = script.take_duel_requests();
    if requests.is_empty() {
        return;
    }
    let self_guid = self_q.iter().next().map(|g| g.0);
    for req in requests {
        match req {
            DuelRequest::Accept => {
                if duel.arbiter != 0 {
                    let _ = commands.0.send(ClientCommand::DuelAccepted {
                        arbiter: duel.arbiter,
                    });
                }
            }
            DuelRequest::Cancel => {
                if duel.arbiter != 0 {
                    let _ = commands.0.send(ClientCommand::DuelCancelled {
                        arbiter: duel.arbiter,
                    });
                }
            }
            DuelRequest::StartByUnit(token) => {
                // The reference resolves the token to a CGObject and requires typemask 0x10 —
                // a player (`0x4d4c66`). Ours are the tokens the unit popup actually passes.
                let guid = match token.as_str() {
                    "target" => selection.guid,
                    "player" => self_guid,
                    _ => None,
                };
                challenge(guid, self_guid, &index, &spells, &actions, &commands);
            }
            DuelRequest::StartByName(name) => {
                // `/duel <name>`: the reference looks the name up in the **object manager**
                // (`0x515970`) — only a streamed player can be found this way, which is also the
                // spell's own requirement. Same population here: the streamed guid index, named
                // through the cache.
                let guid = streamed_player_named(&name, &index, &names);
                challenge(guid, self_guid, &index, &spells, &actions, &commands);
            }
        }
    }
}

/// Find a streamed player by name — the reference's object-manager name lookup. Case-insensitive:
/// the caller is a `/duel` argument the player typed.
pub(crate) fn streamed_player_named(
    name: &str,
    index: &GuidIndex,
    names: &NameCache,
) -> Option<u64> {
    index
        .0
        .keys()
        .copied()
        .filter(|g| benilla_protocol::guid::is_player(*g))
        .find(|g| names.peek(*g).is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

/// Cast the duel spell at `guid` — the reference's `0x4d4810`, which is a plain spell cast at a
/// unit target, not a duel opcode. Silently drops a missing/self/non-player/unstreamed target the
/// way the reference's guid and typemask gates do.
fn challenge(
    guid: Option<u64>,
    own: Option<u64>,
    index: &GuidIndex,
    spells: &Option<Res<Spells>>,
    actions: &crate::ui_action::PlayerActions,
    commands: &NetCommands,
) {
    let Some(guid) = guid.filter(|g| *g != 0 && Some(*g) != own) else {
        return;
    };
    if !benilla_protocol::guid::is_player(guid) || !index.0.contains_key(&guid) {
        return;
    }
    let Some(spell_id) = duel_spell(spells, actions) else {
        return;
    };
    let _ = commands.0.send(ClientCommand::CastSpell {
        spell_id,
        target: Some(guid),
    });
}

/// The learned spell whose `Effect[0]` is [`SPELL_EFFECT_DUEL`] — the reference's `[0xb71130]`,
/// filled by its spell-learned walk rather than hardcoded (see the constant's note).
fn duel_spell(
    spells: &Option<Res<Spells>>,
    actions: &crate::ui_action::PlayerActions,
) -> Option<u32> {
    let spells = spells.as_ref()?;
    actions.spells.iter().copied().find(|id| {
        spells
            .catalog
            .get(*id)
            .is_some_and(|s| s.effects[0] == SPELL_EFFECT_DUEL)
    })
}

/// Duels: the wire session, the countdown tick, the Era events, and the outbound intents.
pub(crate) struct UiDuelPlugin;

impl Plugin for UiDuelPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<DuelState>().add_systems(
            Update,
            (
                // `tick_countdown` only *names* its line now — `feed_duel` resolves it against
                // the VM — so the tick has to run first or every "Duel starting: N" lands a
                // frame late.
                tick_countdown.before(feed_duel),
                feed_duel.in_set(UiFeed),
                drain_duel.after(UiInput),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The challenge is symmetric: our own request takes the error-line branch and leaves no
    /// popup owed; someone else's leaves the challenger for the feed. Either way the arbiter is
    /// stored — the challenger holds one too, which is why `DUEL_FINISHED` reaches both.
    #[test]
    fn our_own_challenge_takes_the_error_branch() {
        let mut duel = DuelState::default();
        assert!(duel.apply_requested(0xA1, 7, Some(7)));
        assert_eq!(duel.arbiter, 0xA1);
        assert_eq!(duel.challenger, None);

        let mut duel = DuelState::default();
        assert!(!duel.apply_requested(0xA1, 9, Some(7)));
        assert_eq!(duel.challenger, Some(9));
    }

    /// Completion is gated on holding an arbiter (`0x4d4b20`), and only an unstarted duel earns
    /// "Duel cancelled.".
    #[test]
    fn completion_is_gated_on_holding_an_arbiter() {
        let mut duel = DuelState::default();
        assert!(!duel.apply_complete(false), "no duel held → silent");

        duel.apply_requested(0xA1, 9, Some(7));
        duel.apply_countdown(3);
        assert!(duel.apply_complete(false), "unstarted → cancelled line");
        assert_eq!(duel.arbiter, 0);
        assert!(duel.countdown.is_none(), "the countdown is cancelled too");

        duel.apply_requested(0xA1, 9, Some(7));
        assert!(
            !duel.apply_complete(true),
            "a real duel ending is silent here"
        );
    }

    /// A zero countdown arms nothing (the reference's unreachable wraparound path).
    #[test]
    fn zero_countdown_arms_nothing() {
        let mut duel = DuelState::default();
        duel.apply_countdown(0);
        assert!(duel.countdown.is_none());
        duel.apply_countdown(3);
        assert_eq!(duel.countdown.as_ref().map(|c| c.remaining), Some(3));
    }

    /// The duel-spell lookup against the **real 5875 Spell.dbc** — the fact the whole challenge
    /// path rests on, and the one a unit test with a synthetic catalog could not catch. Asserts
    /// what the reference's own spell-learned walk asserts by construction: scanning a spellbook
    /// for `Effect[0] == SPELL_EFFECT_DUEL` finds **exactly one** spell, and it is 7266 "Duel"
    /// (the id vmangos grants every race/class in `playercreateinfo_spell` and re-grants at every
    /// login through `Player::LoadFromDB` → `LearnDefaultSpells`). A second match would mean the
    /// reference's single global `[0xb71130]` is ambiguous — worth knowing if it ever happens.
    /// Skips without client data.
    #[test]
    fn the_duel_spell_resolves_to_7266_on_real_data() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let mut hits: Vec<u32> = catalog
            .iter()
            .filter(|(_, s)| s.effects[0] == SPELL_EFFECT_DUEL)
            .map(|(id, _)| id)
            .collect();
        hits.sort_unstable();
        assert_eq!(hits, vec![7266], "exactly one SPELL_EFFECT_DUEL spell");
        assert_eq!(catalog.get(7266).unwrap().name, "Duel");
    }

    /// **The two outcome templates take (winner, loser) in that order, and one of them prints
    /// them back to front.** That is the whole reason they are positional, and the reason this is
    /// resolved from the player's own `GlobalStrings.lua` rather than typed here: the swap lives
    /// in the *string*, so a locale that orders the clause differently gets it right for free and
    /// a re-typed English sentence never could.
    ///
    /// Resolved against the real shipped file, because a stub would be asserting our own idea of
    /// the templates back at us.
    #[test]
    fn the_outcome_line_places_both_names_positionally() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let get = |key: &str| s.lua().globals().get::<String>(key).ok();

        let outcome = |fled| {
            owed_text(
                &OwedLine::Winner {
                    fled,
                    winner: "Onerogue".into(),
                    loser: "Twomage".into(),
                },
                &get,
            )
        };
        assert_eq!(
            outcome(false).as_deref(),
            Some("Onerogue has defeated Twomage in a duel")
        );
        assert_eq!(
            outcome(true).as_deref(),
            Some("Twomage has fled from Onerogue in a duel")
        );
        assert_eq!(
            owed_text(&OwedLine::Countdown(3), &get).as_deref(),
            Some("Duel starting: 3")
        );
    }

    /// **The two `ERR_DUEL_*` lines are catalog rows, and the catalog disagrees with where this
    /// window used to put them.** `ERR_DUEL_CANCELLED` is `MsgKind::Info` — the yellow
    /// `UI_INFO_MESSAGE`, not a chat line — and `ERR_DUEL_REQUESTED` carries the `LEVELUP` cue,
    /// which a straight `chat_log.push_event` had no row in hand to play. Both were invisible
    /// while the sentences were composed here; both are read now.
    ///
    /// The three engine-composed templates are asserted to be the opposite case — **not** rows —
    /// because that is what forbids them the `Shown::keyed` road, where an unknown key takes the
    /// red-line fallback.
    #[test]
    fn the_error_pair_are_catalog_rows_and_the_templates_are_not() {
        use benilla_ui::messages::{by_key, MsgKind};
        let requested = by_key("ERR_DUEL_REQUESTED").expect("a catalog row");
        assert_eq!(requested.kind, MsgKind::Chat);
        assert_eq!(requested.sound, Some("LEVELUP"));
        let cancelled = by_key("ERR_DUEL_CANCELLED").expect("a catalog row");
        assert_eq!(
            cancelled.kind,
            MsgKind::Info,
            "the yellow info line, not chat"
        );
        for key in [
            "DUEL_COUNTDOWN",
            "DUEL_WINNER_KNOCKOUT",
            "DUEL_WINNER_RETREAT",
        ] {
            assert!(
                by_key(key).is_none(),
                "{key} is engine-composed — it must not go out through Shown::keyed"
            );
        }
    }
}
