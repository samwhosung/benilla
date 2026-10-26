//! Duels: the challenge, the countdown, the bounds and the outcome line, after the 1.12 client's
//! handlers (registered at `0x4d4710`). The challenge (`0x4d49d0`) reaches both players, and the
//! challenger's own copy accepts at once (`0x4d4830`). Bounds (`0x4d4aa0`, `0x4d4ac0`) only fire
//! events: the server enforces them (`Player::CheckDuelDistance`: 75 yd out, 70 yd back in, 10 s
//! to return). Each handler's events and lines go on the dialogs' lane at its packet's place
//! ([`crate::ui_dialog_verbs::DialogLane`]), as the reference signals inside them.
//!
//! Deviation: the challenger's name comes from the [`NameCache`], so a name still being queried
//! delays the popup to the name's answer rather than dropping it; the reference reads it off the
//! streamed player and fires nothing without one (`0x4d4a72`).

use benilla_ui::script::{DuelRequest, ScriptValue};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{ClientCommand, GuidIndex, NetCommands};
use crate::query_cache::Ask;
use crate::spell::CastCommit;
use crate::ui_action::Spells;
use crate::ui_dialog_verbs::{DialogLane, DialogSignal};
use crate::ui_script::{SweepApp, SweepLeg};

/// The `Effect[0]` that marks the duel spell (7266 on 1.12 data): the reference's spell-learned
/// walk stores any learned spell with `SpellRec+0xf4 == 0x53` in `[0xb71130]` (`0x4b2605`), which
/// `0x4d4810` casts.
const SPELL_EFFECT_DUEL: u32 = 83;

/// A line named by its `GlobalStrings` key, resolved where the lane fires it. The three keys are
/// engine-composed, not message-catalog rows, so they go out as `Shown::unkeyed`: through
/// `Shown::keyed` an unknown key turns the line red.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OwedLine {
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

/// The duel session: written by the net handlers, read by the answers and by [`tick_countdown`].
#[derive(Resource, Default)]
pub(crate) struct DuelState {
    /// The duel flag's guid, `[0xb73240]`: set by the request, echoed on accept and cancel, and
    /// cleared by completion, which fires `DUEL_FINISHED`, and silently by the leave-world sweep
    /// ([`clear_duel_on_leave_world`]). `0` is no duel.
    pub(crate) arbiter: u64,
    /// A challenge whose popup waits on its challenger's name (the module's deviation).
    challenger: Option<u64>,
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
        self.challenger = None;
        Some(challenger) == own
    }

    /// The partner probe's accept hook (`WOW_PROBE=partner`): taking the challenger discharges the
    /// popup the name's answer still owes.
    pub(crate) fn take_challenger(&mut self) -> Option<u64> {
        self.challenger.take()
    }

    /// `SMSG_DUEL_COMPLETE` (`0x4d4b20`), its signals in order onto `lane`. Gated on an arbiter
    /// (`0x4d4b35`), except the countdown cancel, which the reference also runs outside the gate:
    /// "Duel cancelled." for an unstarted duel (`0x4d4b44`), then `DUEL_FINISHED` (`0x4d4b51`).
    fn apply_complete(&mut self, started: bool, lane: &mut DialogLane) {
        self.countdown = None;
        if self.arbiter == 0 {
            return;
        }
        self.arbiter = 0;
        self.challenger = None;
        if !started {
            lane.push(DialogSignal::Line("ERR_DUEL_CANCELLED"));
        }
        lane.event("DUEL_FINISHED", vec![]);
    }

    /// `SMSG_DUEL_COUNTDOWN` (`0x4d4ae0`): the seconds stored and `ProcessCountdown`'s body run
    /// before its timer is armed, so the first line is the packet's; the line it owes.
    /// Deviation: zero arms nothing, where the reference prints 0 and re-arms forever on the
    /// `dec`'s wraparound, a bug vmangos never reaches (it sends 3000).
    fn apply_countdown(&mut self, seconds: u32) -> Option<OwedLine> {
        self.countdown = (seconds > 0).then(|| Countdown {
            remaining: seconds,
            tick: Timer::from_seconds(1.0, TimerMode::Repeating),
        });
        self.count_down()
    }

    /// `ProcessCountdown`'s body (`0x4d4930`): the line for the seconds left, and the count down
    /// by one; at 0 the timer ends.
    fn count_down(&mut self) -> Option<OwedLine> {
        let countdown = self.countdown.as_mut()?;
        let remaining = countdown.remaining;
        countdown.remaining -= 1;
        if countdown.remaining == 0 {
            self.countdown = None;
        }
        Some(OwedLine::Countdown(remaining))
    }
}

/// A player name answered, a record or a refusal: the held challenge from `guid` fires its
/// `DUEL_REQUESTED` with the name, or, refused, is spent silently.
pub(crate) fn name_answered(
    duel: &mut DuelState,
    lane: &mut DialogLane,
    guid: u64,
    name: Option<&str>,
) {
    if duel.challenger != Some(guid) {
        return;
    }
    duel.challenger = None;
    if let Some(name) = name {
        lane.event("DUEL_REQUESTED", vec![ScriptValue::Str(name.to_string())]);
    }
}

/// Resolve an owed line against the VM's string table; a missing string is no line.
pub(crate) fn owed_text(line: &OwedLine, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
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

/// The duel's packet handlers, each signalling on the dialogs' lane.
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
        mut lane: ResMut<DialogLane>,
        commands: Res<NetCommands>,
        names: Res<NameCache>,
        self_guid: Res<crate::net::SelfGuid>,
        social: Res<crate::ui_social::SocialState>,
    ) {
        if let SessionEvent::DuelRequested {
            arbiter,
            challenger,
        } = ev
        {
            let challenge = Challenge {
                arbiter,
                challenger,
                own: self_guid.0,
                ignored: social.is_ignored(challenger),
            };
            requested(&mut duel, &mut lane, &commands, &names, challenge);
        }
    }

    /// `0x4d4aa0` and `0x4d4ac0`: their event and nothing else, per packet.
    fn on_bounds(In(ev): In<SessionEvent>, mut lane: ResMut<DialogLane>) {
        match ev {
            SessionEvent::DuelOutOfBounds => lane.event("DUEL_OUTOFBOUNDS", vec![]),
            SessionEvent::DuelInBounds => lane.event("DUEL_INBOUNDS", vec![]),
            _ => {}
        }
    }

    fn on_complete(
        In(ev): In<SessionEvent>,
        mut duel: ResMut<DuelState>,
        mut lane: ResMut<DialogLane>,
    ) {
        if let SessionEvent::DuelComplete { started } = ev {
            duel.apply_complete(started, &mut lane);
        }
    }

    /// `SMSG_DUEL_WINNER`: the outcome line, composed client-side (`0x4d4ba0`); the server
    /// broadcasts it, so bystanders see it too.
    fn on_winner(In(ev): In<SessionEvent>, mut lane: ResMut<DialogLane>) {
        if let SessionEvent::DuelWinner {
            fled,
            winner,
            loser,
        } = ev
        {
            lane.push(DialogSignal::Duel(OwedLine::Winner {
                fled,
                winner,
                loser,
            }));
        }
    }

    fn on_countdown(
        In(ev): In<SessionEvent>,
        mut duel: ResMut<DuelState>,
        mut lane: ResMut<DialogLane>,
    ) {
        if let SessionEvent::DuelCountdown { seconds } = ev {
            if let Some(line) = duel.apply_countdown(seconds) {
                lane.push(DialogSignal::Duel(line));
            }
        }
    }

    /// The duel dies with the socket, as on the server (`Player::DuelComplete(DUEL_FLED)` on
    /// logout); a stale arbiter would make the next `AcceptDuel` echo a dead object.
    fn on_session_end(In(_): In<SessionEvent>, mut duel: ResMut<DuelState>) {
        *duel = DuelState::default();
    }

    /// One `SMSG_DUEL_REQUESTED`'s two guids and what the handler weighs them by.
    pub(crate) struct Challenge {
        pub(crate) arbiter: u64,
        pub(crate) challenger: u64,
        pub(crate) own: Option<u64>,
        pub(crate) ignored: bool,
    }

    /// `SMSG_DUEL_REQUESTED` (`0x4d49d0`): the flag guid stored (`0xb73240`); our own challenge
    /// shows `ERR_DUEL_REQUESTED` and accepts at once (`0x4d4a05`, `0x4d4a12`); one from an
    /// ignored player is cancelled with the stored guid and the duel cleared, showing nothing
    /// (`0x4d4a33`); any other fires `DUEL_REQUESTED` with the challenger's name (`0x4d4a83`),
    /// asked once here and held to its answer on a miss (the module's deviation).
    pub(crate) fn requested(
        duel: &mut DuelState,
        lane: &mut DialogLane,
        commands: &NetCommands,
        names: &NameCache,
        c: Challenge,
    ) {
        let ours = duel.apply_requested(c.arbiter, c.challenger, c.own);
        if ours {
            lane.push(DialogSignal::Line("ERR_DUEL_REQUESTED"));
            let _ = commands
                .0
                .send(ClientCommand::DuelAccepted { arbiter: c.arbiter });
            return;
        }
        if c.ignored {
            let _ = commands
                .0
                .send(ClientCommand::DuelCancelled { arbiter: c.arbiter });
            duel.arbiter = 0;
            return;
        }
        match names.resolve_player(c.challenger, Ask::at(true, commands)) {
            Some(name) => lane.event("DUEL_REQUESTED", vec![ScriptValue::Str(name.to_string())]),
            None => duel.challenger = Some(c.challenger),
        }
    }
}

/// The leave-world sweep's duel leg (`0x490afd` → `0x4d47d0`): the arbiter is zeroed
/// (`0x4d47d8`, `0x4d47e2`) and the `ProcessCountdown` timer cancelled (`0x4d47ee`), with no
/// event, so a completion after it finds no duel (`0x4d4b20`).
fn clear_duel_on_leave_world(mut duel: ResMut<DuelState>) {
    duel.arbiter = 0;
    duel.countdown = None;
}

/// `ProcessCountdown` (`0x4d4930`, armed by `0x4d4ae0`): after the packet's own line, one a second
/// on the dialogs' lane, stamped at the tick.
fn tick_countdown(time: Res<Time>, mut duel: ResMut<DuelState>, mut lane: ResMut<DialogLane>) {
    let Some(countdown) = duel.countdown.as_mut() else {
        return;
    };
    if !countdown.tick.tick(time.delta()).just_finished() {
        return;
    }
    if let Some(line) = duel.count_down() {
        lane.push(DialogSignal::Duel(line));
    }
}

/// The duel verbs inside their calls ([`DuelRequest`]): the popup's answers, and the duel spell a
/// challenge casts, whose cast the host's ladder sends.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DuelSteps<'w> {
    /// Absent in a world with no duel session, which answers with guid 0.
    duel: Option<Res<'w, DuelState>>,
    actions: Res<'w, crate::ui_action::PlayerActions>,
    spells: Option<Res<'w, Spells>>,
    net: Res<'w, NetCommands>,
}

impl DuelSteps<'_> {
    /// `AcceptDuel()` (`0x4d4830`) and `CancelDuel()` (`0x4d48b0`): the packet with the stored flag
    /// guid `[0xb73240]`, with no test that a duel exists.
    pub(crate) fn answer(&self, request: &DuelRequest) {
        let arbiter = self.duel.as_deref().map_or(0, |d| d.arbiter);
        let command = match request {
            DuelRequest::Accept => ClientCommand::DuelAccepted { arbiter },
            DuelRequest::Cancel => ClientCommand::DuelCancelled { arbiter },
            DuelRequest::StartByGuid(_) => return,
        };
        let _ = self.net.0.send(command);
    }

    /// The duel spell `[0xb71130]` a challenge casts (`0x4d4810`).
    pub(crate) fn duel_spell(&self) -> Option<u32> {
        duel_spell(self.spells.as_deref(), &self.actions)
    }
}

/// A challenge's cast inside its call: `CastDuelSpell 0x4d4810`, the duel spell through `0x6e5a90`,
/// the cast attempt with the guid as its explicit target, so the bind's refusal of the caster, the
/// range test and the server's answer decide what shows.
pub(crate) fn cast_duel(cast: &mut crate::spell::ScriptCast, spell_id: u32, guid: u64) {
    let ctx = cast.targeting.context_at_guid(guid);
    cast.ladder.send(spell_id, &ctx, CastCommit::Spell);
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
        .find(|g| {
            names
                .resolve_unit(*g, None, Ask::Read)
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
}

/// The learned spell whose `Effect[0]` is [`SPELL_EFFECT_DUEL`], the reference's `[0xb71130]`.
fn duel_spell(spells: Option<&Spells>, actions: &crate::ui_action::PlayerActions) -> Option<u32> {
    let spells = spells?;
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
        crate::ui_dialog_verbs::add_dialog_lane(app);
        app.init_resource::<DuelState>()
            .add_sweep_leg(SweepLeg::Duel, clear_duel_on_leave_world)
            .add_systems(
                Update,
                // A due timer runs first in its tick (`0x428510` at `0x420d0c`), ahead of the
                // packets (`0x420d55`); before the pass: the duel answers' calls read the session.
                tick_countdown
                    .before(benilla_world::schedule::WorldStage::Net)
                    .before(crate::vm_pass::VmPass::Start),
            );
    }
}

#[cfg(test)]
mod challenge_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge(challenger: u64, ignored: bool) -> net::Challenge {
        net::Challenge {
            arbiter: 0xA1,
            challenger,
            own: Some(7),
            ignored,
        }
    }

    /// The challenge looks its challenger's name up once, as its packet's: a hit fires at once, a
    /// miss waits for the answer, which a refusal spends silently; the next challenge asks again.
    #[test]
    fn a_refused_challenger_is_asked_by_the_next_challenge_never_by_a_frame() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut names = NameCache::default();
        let (mut duel, mut lane) = (DuelState::default(), DialogLane::default());
        net::requested(&mut duel, &mut lane, &commands, &names, challenge(9, false));
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        assert!(lane.take().is_empty(), "held for the name");
        names.insert_player(9, String::new(), None);
        name_answered(&mut duel, &mut lane, 9, None);
        assert!(lane.take().is_empty(), "a refusal fires nothing");
        assert_eq!(duel.challenger, None);
        net::requested(&mut duel, &mut lane, &commands, &names, challenge(9, false));
        assert_eq!(
            crate::query_cache::queries(&rx).len(),
            1,
            "the next challenge asks again"
        );
        name_answered(&mut duel, &mut lane, 9, Some("Nine"));
        assert_eq!(
            lane.take(),
            [DialogSignal::Event(
                "DUEL_REQUESTED".into(),
                vec![ScriptValue::Str("Nine".into())]
            )]
        );
        names.insert_player(9, "Nine".into(), None);
        net::requested(&mut duel, &mut lane, &commands, &names, challenge(9, false));
        assert_eq!(lane.take().len(), 1, "a cached name fires at its packet");
    }

    /// Both sides store the arbiter, so `DUEL_FINISHED` reaches both; our own challenge shows
    /// `ERR_DUEL_REQUESTED` and accepts (`0x4d4a05`, `0x4d4a12`), and an ignored one is cancelled
    /// with the stored guid and cleared (`0x4d4a33`).
    #[test]
    fn our_own_challenge_takes_the_error_branch() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let names = NameCache::default();
        let (mut duel, mut lane) = (DuelState::default(), DialogLane::default());
        net::requested(&mut duel, &mut lane, &commands, &names, challenge(7, false));
        assert_eq!(duel.arbiter, 0xA1);
        assert_eq!(duel.challenger, None);
        assert_eq!(lane.take(), [DialogSignal::Line("ERR_DUEL_REQUESTED")]);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::DuelAccepted { arbiter: 0xA1 })
        ));

        net::requested(&mut duel, &mut lane, &commands, &names, challenge(9, true));
        assert_eq!(duel.arbiter, 0, "the ignored challenge clears the duel");
        assert!(lane.take().is_empty());
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::DuelCancelled { arbiter: 0xA1 })
        ));
    }

    /// Only an unstarted duel earns "Duel cancelled.", ahead of `DUEL_FINISHED` (`0x4d4b20`).
    #[test]
    fn completion_is_gated_on_holding_an_arbiter() {
        let finished = || DialogSignal::Event("DUEL_FINISHED".into(), vec![]);
        let mut duel = DuelState::default();
        let mut lane = DialogLane::default();
        duel.apply_complete(false, &mut lane);
        assert!(lane.take().is_empty(), "no duel held → silent");

        duel.apply_requested(0xA1, 9, Some(7));
        duel.apply_countdown(3);
        duel.apply_complete(false, &mut lane);
        assert_eq!(
            lane.take(),
            [DialogSignal::Line("ERR_DUEL_CANCELLED"), finished()],
            "unstarted → the cancelled line first"
        );
        assert_eq!(duel.arbiter, 0);
        assert!(duel.countdown.is_none(), "the countdown is cancelled too");

        duel.apply_requested(0xA1, 9, Some(7));
        duel.apply_complete(true, &mut lane);
        assert_eq!(lane.take(), [finished()], "a real ending");
    }

    /// The packet's own line, then one per tick down to 1 (`0x4d4ae0`, `0x4d4930`).
    #[test]
    fn the_countdown_prints_at_its_packet_then_each_second() {
        let mut duel = DuelState::default();
        assert_eq!(duel.apply_countdown(0), None, "zero arms nothing");
        assert!(duel.countdown.is_none());
        assert_eq!(duel.apply_countdown(3), Some(OwedLine::Countdown(3)));
        assert_eq!(duel.count_down(), Some(OwedLine::Countdown(2)));
        assert_eq!(duel.count_down(), Some(OwedLine::Countdown(1)));
        assert!(duel.countdown.is_none(), "the timer ends at 0");
        assert_eq!(duel.count_down(), None);
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
