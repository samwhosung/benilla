//! Duels: the challenge, the countdown, the bounds and the outcome line, after the 1.12 client's
//! handlers (registered at `0x4d4710`). The challenge (`0x4d49d0`) reaches both players, and the
//! challenger's own copy accepts at once (`0x4d4830`). Bounds (`0x4d4aa0`, `0x4d4ac0`) only fire
//! events: the server enforces them (`Player::CheckDuelDistance`: 75 yd out, 70 yd back in, 10 s
//! to return).
//!
//! Deviation: the challenger's name comes from the [`NameCache`], so a name still being queried
//! delays the popup rather than dropping it; the reference reads it off the streamed player and
//! fires nothing without one (`0x4d4a72`).

use benilla_ui::script::{DuelRequest, ScriptValue, UiScript};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, SelfPlayer};
use crate::target::Selection;
use crate::ui_action::Spells;
use crate::ui_script::{UiFeed, UiInput};

/// The `Effect[0]` that marks the duel spell (7266 on 1.12 data): the reference's spell-learned
/// walk stores any learned spell with `SpellRec+0xf4 == 0x53` in `[0xb71130]` (`0x4b2605`), which
/// `0x4d4810` casts.
const SPELL_EFFECT_DUEL: u32 = 83;

/// A line named by its `GlobalStrings` key, resolved in [`feed_duel`] where the VM is. The three
/// keys are engine-composed, not message-catalog rows, so they go out as `Shown::unkeyed`: through
/// `Shown::keyed` an unknown key turns the line red.
enum OwedLine {
    /// `DUEL_COUNTDOWN` ("Duel starting: %d") with the seconds left.
    Countdown(u32),
    /// `DUEL_WINNER_RETREAT` when the loser fled, else `DUEL_WINNER_KNOCKOUT`; both are positional
    /// (`%1$s` winner, `%2$s` loser), and the retreat line names the loser first.
    Winner {
        fled: bool,
        winner: String,
        loser: String,
    },
}

/// The duel session: written by the net handlers, read by [`feed_duel`], which fires the events on
/// its edges, and by [`tick_countdown`].
#[derive(Resource, Default)]
pub(crate) struct DuelState {
    owed: Vec<OwedLine>,
    /// The duel flag's guid, `[0xb73240]`: set by the request, echoed on accept and cancel, and
    /// cleared only by completion, which fires `DUEL_FINISHED`. `0` is no duel.
    pub(crate) arbiter: u64,
    /// Set while the challenge popup is owed; the challenger never gets one.
    challenger: Option<u64>,
    out_of_bounds: bool,
    countdown: Option<Countdown>,
}

/// The `ProcessCountdown` timer (`0x4d4930`).
struct Countdown {
    remaining: u32,
    tick: Timer,
}

impl DuelState {
    /// `SMSG_DUEL_REQUESTED`: true when the challenge is our own, and the caller then shows
    /// `ERR_DUEL_REQUESTED` and accepts inline, as the reference's handler does (`0x4d4a12`).
    fn apply_requested(&mut self, arbiter: u64, challenger: u64, own: Option<u64>) -> bool {
        self.arbiter = arbiter;
        let ours = Some(challenger) == own;
        self.challenger = (!ours).then_some(challenger);
        ours
    }

    /// The partner probe's accept hook (`WOW_PROBE=partner`): taking the challenger discharges the
    /// popup as the feed's `DUEL_REQUESTED` edge does.
    pub(crate) fn take_challenger(&mut self) -> Option<u64> {
        self.challenger.take()
    }

    /// `SMSG_DUEL_COMPLETE`: true when "Duel cancelled." is owed. Gated on an arbiter, as
    /// `0x4d4b20` is, except the countdown cancel, which the reference also runs outside the gate.
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

    /// `SMSG_DUEL_COUNTDOWN`. Deviation: zero arms nothing, where the reference prints 0 and
    /// re-arms forever on the `dec`'s wraparound, a bug vmangos never reaches (it sends 3000).
    fn apply_countdown(&mut self, seconds: u32) {
        self.countdown = (seconds > 0).then(|| Countdown {
            remaining: seconds,
            tick: Timer::from_seconds(1.0, TimerMode::Repeating),
        });
    }
}

/// Resolve an owed line against the VM's string table; a missing string is no line.
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
            // Always winner then loser; the template's `%1$s`/`%2$s` picks the order.
            fill(&get(key)?, &[Arg::S(winner), Arg::S(loser)])
        }
    };
    (!text.is_empty()).then_some(text)
}

/// The duel's packet handlers; the events fire off the state's edges in [`feed_duel`].
pub(crate) mod net {
    use super::*;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

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

    /// The duel dies with the socket, as on the server (`Player::DuelComplete(DUEL_FLED)` on
    /// logout); a stale arbiter would make the next `AcceptDuel` echo a dead object.
    fn on_session_end(In(_): In<SessionEvent>, mut duel: ResMut<DuelState>) {
        *duel = DuelState::default();
    }

    /// `SMSG_DUEL_REQUESTED`. A challenge from an ignored player is declined with
    /// `CMSG_DUEL_CANCELLED` and shows nothing (`0x4d4a33`).
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

    /// `SMSG_DUEL_WINNER`: the outcome line, composed client-side (`0x4d4ba0`); the server
    /// broadcasts it, so bystanders see it too.
    pub(crate) fn winner(duel: &mut DuelState, fled: bool, winner: &str, loser: &str) {
        duel.owed.push(OwedLine::Winner {
            fled,
            winner: winner.to_string(),
            loser: loser.to_string(),
        });
    }

    pub(crate) fn countdown(duel: &mut DuelState, seconds: u32) {
        duel.apply_countdown(seconds);
    }

    pub(crate) fn bounds(duel: &mut DuelState, outside: bool) {
        duel.out_of_bounds = outside;
    }
}

/// Fire the four duel events on [`DuelState`]'s edges.
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

    // Lines before events: a line describes the transition its event announces.
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

    // `DUEL_REQUESTED(name)`, held until the name resolves (the module's deviation).
    if let Some(guid) = duel.challenger {
        if let Some(name) = names.resolve(guid, &commands).map(str::to_string) {
            script.fire_event("DUEL_REQUESTED", vec![ScriptValue::Str(name)]);
            duel.challenger = None;
        }
    }

    // `DUEL_FINISHED` when the arbiter clears (`0x4d4b51`).
    let held = duel.arbiter != 0;
    if fed.held && !held {
        script.fire_event("DUEL_FINISHED", Vec::new());
    }
    fed.held = held;

    // The bounds pair (`0x4d4aa0`, `0x4d4ac0`).
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

/// `ProcessCountdown` (`0x4d4930`, armed by `0x4d4ae0`): a `DUEL_COUNTDOWN` system chat line on
/// arrival, since the reference runs the tick body before arming its timer, then one a second.
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

/// Drain the duel verbs' intents into their sends.
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
                // `StartDuelUnit` (`0x4d4c90`) resolves the token through `0x515970`, with no
                // player gate; these are the tokens the unit popup passes.
                let guid = match token.as_str() {
                    "target" => selection.guid,
                    "player" => self_guid,
                    _ => None,
                };
                challenge(guid, self_guid, &index, &spells, &actions, &commands);
            }
            DuelRequest::StartByName(name) => {
                // `/duel <name>`: `StartDuel` (`0x4d4c40`) matches the name through `0x493aa0`
                // with typemask `0x10` (`0x4d4c66`), players only; here, the streamed guid index
                // named through the cache.
                let guid = streamed_player_named(&name, &index, &names);
                challenge(guid, self_guid, &index, &spells, &actions, &commands);
            }
        }
    }
}

/// A streamed player by name, case-insensitive, for a typed `/duel` argument.
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

/// Cast the duel spell at `guid`, a plain spell cast as in the reference's `0x4d4810`, not a duel
/// opcode. Drops a missing, self, non-player or unstreamed target; the reference's bindings drop
/// only a zero guid, plus a non-player in `StartDuel`'s name match.
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

/// The learned spell whose `Effect[0]` is [`SPELL_EFFECT_DUEL`], the reference's `[0xb71130]`.
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

/// Duels: the wire session, the countdown tick, the events and the outbound intents.
pub(crate) struct UiDuelPlugin;

impl Plugin for UiDuelPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<DuelState>().add_systems(
            Update,
            (
                // The tick names its line and `feed_duel` resolves it, so the tick runs first or
                // each "Duel starting: N" lands a frame late.
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

    /// Both sides store the arbiter, so `DUEL_FINISHED` reaches both.
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

    /// Only an unstarted duel earns "Duel cancelled." (`0x4d4b20`).
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

    #[test]
    fn zero_countdown_arms_nothing() {
        let mut duel = DuelState::default();
        duel.apply_countdown(0);
        assert!(duel.countdown.is_none());
        duel.apply_countdown(3);
        assert_eq!(duel.countdown.as_ref().map(|c| c.remaining), Some(3));
    }

    /// 7266 "Duel", which vmangos grants every race and class (`playercreateinfo_spell`), is the
    /// only `SPELL_EFFECT_DUEL` spell in the shipped `Spell.dbc`, so `[0xb71130]` is unambiguous.
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

    /// Against the shipped `GlobalStrings.lua`: the retreat template names the loser first.
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

    /// `ERR_DUEL_CANCELLED` is the yellow info line and `ERR_DUEL_REQUESTED` plays `LEVELUP`; the
    /// three templates are not rows, which `Shown::keyed` would turn red.
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
