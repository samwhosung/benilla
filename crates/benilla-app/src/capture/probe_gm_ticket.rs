//! The GM ticket live probe (`WOW_PROBE_GMTICKET=1`): drives the five ticket opcodes through the
//! live Lua VM's bindings (`GetGMStatus`, `DeleteGMTicket`, `GetGMTicket`, `NewGMTicket`,
//! `UpdateGMTicket`) and observes each answer as the Help window does, as an `UPDATE_TICKET` or
//! `UPDATE_GM_STATUS` event. Logs one `PROBE_GMTICKET: <step> PASS/FAIL/SKIP <detail>` line per
//! step, then `PROBE_GMTICKET: DONE pass=<n> fail=<m>`, and exits. Non-combat and sends no GM
//! command; the switches are `docs/CONTRIBUTING.md`, "Running it unattended".
//!
//! vmangos answers several refusals with silence (`GMTicketHandler.cpp:88-113`): no packet for a
//! create when the queue is off, the player is under `GMTickets.MinLevel` or the category is
//! `>= GMTICKET_MAX` (11), nor for a delete with no ticket (`:73-86`).
//!
//! The steps:
//! 1. queue: `GetGMStatus()` must fire `UPDATE_GM_STATUS` with 1 (`GMTickets.Enable` defaults on,
//!    `World.cpp:684`); a 0 is the queue switched off, and the rest SKIPs.
//! 2. clean: `DeleteGMTicket()` then `GetGMTicket()`, expecting `arg1 == 0`; the get's answer is
//!    waited on, since a delete with no ticket gets none.
//! 3. create: `NewGMTicket(4, <unique text>)` then `GetGMTicket()`, expecting category 4 and the
//!    text exactly, which proves the create body's layout.
//! 4. db: always SKIP. The client never reads map and position back, so the probe prints the row
//!    it expects (`db-expect`, with a `drift=` bound) and the row is checked by hand.
//! 5. edit: `UpdateGMTicket(4, <second text>)` then `GetGMTicket()`, expecting the new text
//!    exactly, which proves the category byte on `CMSG_GMTICKET_UPDATETEXT` (cmangos-classic reads
//!    a bare cstring there).
//! 6. abandon: `DeleteGMTicket()` then `GetGMTicket()`, expecting `arg1 == 0` again.
//!
//! Lua's calls reach the wire in call order, so each step only waits on the server's round trip.
//! Answers are not correlated to asks: a GM command, vmangos's post-delete `SendTicket(nullptr)`
//! and the engine's own re-ask on create-ok and update-ok (reference `0x5e4479`) all fire
//! `UPDATE_TICKET` unasked, so each step takes any matching answer after its baseline, and matches
//! a unique text in steps 3 and 5.
//!
//! vmangos's abandon is `CloseTicket`, which sets `closed_by` and keeps the row
//! (`GMTicketMgr.cpp:401-410`); an edit replaces `message` and the type but keeps the create's map
//! and position (`GMTicketHandler.cpp:59-60`).
//!
//! An environmental problem (no UI VM, the queue off, a binding that will not run) SKIPs; only a
//! wrong value FAILs.

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_ui::script::UiScript;

use super::probes::ProbeClock;
use crate::net::SelfPlayer;
use crate::player::Player;

/// `GMTicketCategory.dbc` id 4, "Item", which is vmangos's `GMTICKET_ITEM`
/// (`SharedDefines.h:1779`): the id is the wire value, not a list index.
const CATEGORY_ITEM: u32 = 4;

/// `SMSG_GMTICKETSYSTEMSTATUS`'s `GMTICKET_QUEUE_STATUS_ENABLED`. The field is signed (the
/// reference fires it at `0x5e4689` through `%d`, a `fild dword` at `0x704fa6`), so the window's
/// -1 arm is readable; vmangos sends 0 or 1.
const QUEUE_ENABLED: f64 = 1.0;

/// The `UPDATE_TICKET` `arg1` for no ticket; the Help window tests `arg1 and arg1 ~= 0`.
const NO_TICKET: f64 = 0.0;

/// Settle before the first ask, so the Help window's login-time `GetGMTicket()` is answered before
/// a baseline is latched.
const SETTLE_SECS: f64 = 3.0;

/// The gap between a write and its `GetGMTicket()` read-back; the drain keeps call order, so it
/// only keeps one exchange per trace line.
const WRITE_GAP_SECS: f64 = 1.0;

/// How long a step waits for a matching answer; generous, as an occluded window polls at ~1 fps.
const ANSWER_TIMEOUT_SECS: f64 = 20.0;

/// A ticket records the body's position, so the create waits for it to rest: under this many yd
/// of movement for [`REST_HOLD_SECS`].
const REST_EPS: f32 = 0.02;
const REST_HOLD_SECS: f64 = 0.5;
/// The cap on that wait; past it the probe files anyway and flags the position as approximate.
const REST_TIMEOUT_SECS: f64 = 20.0;

/// How far (yd) the body may move between the position sample and the frame the drain stamps the
/// packet before the db-expect position is flagged.
const STAMP_DRIFT_EPS: f32 = 0.05;

/// Step 4's query against the server's `characters` database, printed for a manual check.
const DB_QUERY: &str = "SELECT ticket_id, name, ticket_type, map, position_x, position_y, \
                        position_z, closed_by, message FROM gm_tickets ORDER BY ticket_id DESC \
                        LIMIT 1;";

pub(crate) struct ProbeGmTicketPlugin;

impl Plugin for ProbeGmTicketPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GmTicketProbe>()
            .add_systems(Update, gm_ticket_probe);
    }
}

/// The probe's phase machine and what it discovered along the way.
#[derive(Resource, Default)]
struct GmTicketProbe {
    phase: Phase,
    /// The text step 3 filed, unique per run so no stale answer satisfies it.
    create_text: String,
    /// The text step 5 edited it to.
    edit_text: String,
    /// Map and WoW-space position sampled at the `NewGMTicket` call, which the drain stamps into
    /// `CMSG_GMTICKET_CREATE` that frame or the next.
    create_map: u32,
    create_pos: [f32; 3],
    /// How far the body had moved by the frame after the create call: the db-expect position's
    /// error bound.
    stamp_drift: Option<f32>,
    passes: u32,
    fails: u32,
    /// Latched once [`Phase::Done`] has fired its exit.
    exited: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Wait,
    /// Hook installed; settling before the first ask.
    Settling {
        since: f64,
    },
    /// `GetGMStatus()` called; waiting for `UPDATE_GM_STATUS` (step 1).
    Queue {
        since: f64,
        baseline: usize,
    },
    /// `DeleteGMTicket()` called; letting the drain flush it before the read-back (step 2a).
    CleanGap {
        since: f64,
    },
    /// `GetGMTicket()` called; waiting for `arg1 == 0` (step 2b).
    CleanGet {
        since: f64,
        baseline: usize,
    },
    /// Waiting for the body to rest; `last` is the previous sample, `still_since` when it last
    /// moved.
    Rest {
        since: f64,
        last: [f32; 3],
        still_since: f64,
    },
    /// `NewGMTicket(4, …)` called; letting the drain flush it (step 3a).
    CreateGap {
        since: f64,
    },
    /// `GetGMTicket()` called; waiting for the filed ticket to come back (step 3b).
    CreateGet {
        since: f64,
        baseline: usize,
    },
    /// `UpdateGMTicket(4, …)` called; letting the drain flush it (step 5a).
    EditGap {
        since: f64,
    },
    /// `GetGMTicket()` called; waiting for the new text (step 5b).
    EditGet {
        since: f64,
        baseline: usize,
    },
    /// `DeleteGMTicket()` called; letting the drain flush it (step 6a).
    AbandonGap {
        since: f64,
    },
    /// `GetGMTicket()` called; waiting for `arg1 == 0` again (step 6b).
    AbandonGet {
        since: f64,
        baseline: usize,
    },
    Done,
}

/// The `UPDATE_TICKET` answers seen, newest last, as `(category, text)`. The text is `""` when the
/// category is 0: that fire carries one argument, and `arg2` would be stale.
fn ticket_answers(script: &UiScript) -> Vec<(f64, String)> {
    let cats = script
        .eval::<Vec<f64>>("return ProbeGmTicketCat or {}")
        .unwrap_or_default();
    let texts = script
        .eval::<Vec<String>>("return ProbeGmTicketText or {}")
        .unwrap_or_default();
    cats.into_iter().zip(texts).collect()
}

/// The `UPDATE_GM_STATUS` values the live VM has seen, newest last.
fn status_answers(script: &UiScript) -> Vec<f64> {
    script
        .eval::<Vec<f64>>("return ProbeGmStatus or {}")
        .unwrap_or_default()
}

/// The first answer after `baseline` that satisfies `want`; answers are not correlated to asks.
fn matching<T>(answers: &[T], baseline: usize, want: impl Fn(&T) -> bool) -> Option<usize> {
    answers
        .iter()
        .enumerate()
        .skip(baseline)
        .find(|(_, a)| want(a))
        .map(|(i, _)| i)
}

/// A ticket text unique per run (to the second); the suffix tells a run's two texts apart.
fn unique_text(suffix: &str) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("benilla probe {secs}{suffix}")
}

fn gm_ticket_probe(
    time: ProbeClock,
    mut probe: ResMut<GmTicketProbe>,
    script: Option<NonSendMut<UiScript>>,
    self_player: Query<(), With<SelfPlayer>>,
    player: Res<Player>,
    map: Option<Res<benilla_world::world_map::CurrentMap>>,
    mut exit: MessageWriter<AppExit>,
) {
    if self_player.is_empty() {
        return; // not in-world yet
    }
    let Some(script) = script else {
        return; // no UI VM in a headless build
    };
    let now = time.elapsed_secs_f64();
    let phase = probe.phase;

    match phase {
        Phase::Wait => {
            // The hook goes in before any ask, so it sees the unsolicited answers too.
            if let Err(e) = script.run(
                r#"
                if not ProbeGmTicketHooked then
                    ProbeGmTicketHooked = true
                    ProbeGmTicketCat = {}
                    ProbeGmTicketText = {}
                    ProbeGmStatus = {}
                    local f = CreateFrame("Frame")
                    f:RegisterEvent("UPDATE_TICKET")
                    f:RegisterEvent("UPDATE_GM_STATUS")
                    f:SetScript("OnEvent", function()
                        if event == "UPDATE_TICKET" then
                            local c = arg1 or 0
                            table.insert(ProbeGmTicketCat, c)
                            if c == 0 then
                                table.insert(ProbeGmTicketText, "")
                            else
                                table.insert(ProbeGmTicketText, arg2 or "")
                            end
                        else
                            table.insert(ProbeGmStatus, arg1 or 0)
                        end
                    end)
                end
                "#,
            ) {
                // Without the hook nothing is observable, so stop rather than FAIL every step.
                warn!(
                    "PROBE_GMTICKET: SKIP (0 hook) — the UPDATE_TICKET/UPDATE_GM_STATUS hook would \
                     not install in the live VM: {e}. Nothing can be observed, so no step below \
                     could be trusted (environmental, not a wire failure)."
                );
                probe.phase = Phase::Done;
                return;
            }
            info!("PROBE_GMTICKET: hook installed; settling {SETTLE_SECS}s before the first ask");
            probe.phase = Phase::Settling { since: now };
        }

        Phase::Settling { since } => {
            if now - since < SETTLE_SECS {
                return;
            }
            let baseline = status_answers(&script).len();
            if !run_or_skip(&mut probe, &script, "1 queue", "GetGMStatus()") {
                return;
            }
            probe.phase = Phase::Queue {
                since: now,
                baseline,
            };
        }

        // Step 1, the queue status: no event at all means the round trip is dead.
        Phase::Queue { since, baseline } => {
            let seen = status_answers(&script);
            let Some(i) = matching(&seen, baseline, |_| true) else {
                if now - since > ANSWER_TIMEOUT_SECS {
                    error!(
                        "PROBE_GMTICKET: FAIL (1 queue) — no UPDATE_GM_STATUS within \
                         {ANSWER_TIMEOUT_SECS}s of GetGMStatus(). The CMSG_GMTICKET_SYSTEMSTATUS \
                         → SMSG_GMTICKETSYSTEMSTATUS round trip is dead: either the send never \
                         reached the wire or the answer never decoded. (values seen this run: \
                         {seen:?})"
                    );
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                }
                return;
            };
            let status = seen[i];
            if status != QUEUE_ENABLED {
                warn!(
                    "PROBE_GMTICKET: SKIP (1 queue) — the round trip WORKS (UPDATE_GM_STATUS \
                     fired carrying {status}), but the server's ticket queue is not enabled. \
                     vmangos then answers create with silence (GMTicketHandler.cpp: \
                     `GetStatus() == GMTICKET_QUEUE_STATUS_DISABLED` → return), so every later \
                     step would fail for a server setting rather than a client defect. Set \
                     GMTickets.Enable = 1 and re-run."
                );
                probe.phase = Phase::Done;
                return;
            }
            info!(
                "PROBE_GMTICKET: PASS (1 queue) — UPDATE_GM_STATUS fired carrying \
                 {QUEUE_ENABLED} (the petition queue is up; the window's PETITION_QUEUE_ACTIVE \
                 gate opens)"
            );
            probe.passes += 1;
            if !run_or_skip(&mut probe, &script, "2 clean", "DeleteGMTicket()") {
                return;
            }
            probe.phase = Phase::CleanGap { since: now };
        }

        // Step 2, the clean slate. The delete is not waited on: with no ticket vmangos sends
        // nothing.
        Phase::CleanGap { since } => {
            if now - since < WRITE_GAP_SECS {
                return;
            }
            let baseline = ticket_answers(&script).len();
            if !run_or_skip(&mut probe, &script, "2 clean", "GetGMTicket()") {
                return;
            }
            probe.phase = Phase::CleanGet {
                since: now,
                baseline,
            };
        }
        Phase::CleanGet { since, baseline } => {
            let seen = ticket_answers(&script);
            if matching(&seen, baseline, |(c, _)| *c == NO_TICKET).is_some() {
                info!(
                    "PROBE_GMTICKET: PASS (2 clean) — UPDATE_TICKET fired with arg1 == 0: no open \
                     ticket, and the 4-byte GMTICKET_STATUS_DEFAULT answer decodes"
                );
                probe.passes += 1;
                probe.phase = Phase::Rest {
                    since: now,
                    last: bevy_to_wow(player.pos),
                    still_since: now,
                };
            } else if now - since > ANSWER_TIMEOUT_SECS {
                error!(
                    "PROBE_GMTICKET: FAIL (2 clean) — no UPDATE_TICKET with arg1 == 0 within \
                     {ANSWER_TIMEOUT_SECS}s of DeleteGMTicket() + GetGMTicket(). Answers seen \
                     after the baseline: {:?}. A leftover ticket that refuses to clear, or a \
                     GETTICKET round trip that never answers.",
                    &seen[baseline.min(seen.len())..]
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }

        // The rest gate: a ticket records the position, so file it from a body at rest.
        Phase::Rest {
            since,
            last,
            still_since,
        } => {
            let here = bevy_to_wow(player.pos);
            if distance(here, last) > REST_EPS {
                probe.phase = Phase::Rest {
                    since,
                    last: here,
                    still_since: now,
                };
                return;
            }
            let timed_out = now - since > REST_TIMEOUT_SECS;
            if !timed_out && now - still_since < REST_HOLD_SECS {
                return;
            }
            if timed_out {
                warn!(
                    "PROBE_GMTICKET: (3 create) the body never came to rest within \
                     {REST_TIMEOUT_SECS}s (still at {here:?}); filing anyway — the wire is still \
                     testable, but treat the db-expect position as approximate."
                );
            }
            let text = unique_text("");
            probe.create_pos = here;
            probe.create_map = map.map(|m| m.0).unwrap_or(0);
            if !run_or_skip(
                &mut probe,
                &script,
                "3 create",
                &format!("NewGMTicket({CATEGORY_ITEM}, \"{text}\")"),
            ) {
                return;
            }
            probe.create_text = text;
            probe.phase = Phase::CreateGap { since: now };
        }

        // Step 3: the create body's layout, proven by the echo.
        Phase::CreateGap { since } => {
            // The first tick after the create call is the last frame the drain can stamp the
            // packet in, so the drift is measured here and not at the later db-expect print.
            if probe.stamp_drift.is_none() {
                let drift = distance(bevy_to_wow(player.pos), probe.create_pos);
                probe.stamp_drift = Some(drift);
                if drift > STAMP_DRIFT_EPS {
                    warn!(
                        "PROBE_GMTICKET: (3 create) the body moved {drift:.3} yd in the frame the \
                         packet was stamped; the db-expect position below is good only to that."
                    );
                }
            }
            if now - since < WRITE_GAP_SECS {
                return;
            }
            let baseline = ticket_answers(&script).len();
            if !run_or_skip(&mut probe, &script, "3 create", "GetGMTicket()") {
                return;
            }
            probe.phase = Phase::CreateGet {
                since: now,
                baseline,
            };
        }
        Phase::CreateGet { since, baseline } => {
            let seen = ticket_answers(&script);
            let want = probe.create_text.clone();
            if let Some(i) = matching(&seen, baseline, |(_, t)| *t == want) {
                let (cat, text) = &seen[i];
                if *cat != f64::from(CATEGORY_ITEM) {
                    error!(
                        "PROBE_GMTICKET: FAIL (3 create) — the ticket came back with the right \
                         text but category {cat}, wanted {CATEGORY_ITEM}. The category byte is \
                         the head of CMSG_GMTICKET_CREATE; a wrong value there shifts the map id \
                         and the whole position."
                    );
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                    return;
                }
                info!(
                    "PROBE_GMTICKET: PASS (3 create) — the server filed and echoed the ticket: \
                     arg1 == {cat} (Item) and arg2 == {text:?} byte for byte. The u8 category, \
                     the map/position block, the text and the trailing \"Reserved for future \
                     use\" cstring are all laid out the way vmangos reads them."
                );
                probe.passes += 1;
                report_db_expectation(&probe, "after create", &want);
                warn!(
                    "PROBE_GMTICKET: SKIP (4 db) — operator step, by design: the client never \
                     reads map/position back, so only the server's own row proves they landed. \
                     After the run, against the server's `characters` database: {DB_QUERY} — \
                     and compare against the db-expect line(s)."
                );
                let text = unique_text(" edited");
                if !run_or_skip(
                    &mut probe,
                    &script,
                    "5 edit",
                    &format!("UpdateGMTicket({CATEGORY_ITEM}, \"{text}\")"),
                ) {
                    return;
                }
                probe.edit_text = text;
                probe.phase = Phase::EditGap { since: now };
            } else if now - since > ANSWER_TIMEOUT_SECS {
                error!(
                    "PROBE_GMTICKET: FAIL (3 create) — the ticket never came back within \
                     {ANSWER_TIMEOUT_SECS}s. Wanted arg2 == {want:?}; answers seen after the \
                     baseline: {:?}. vmangos answers a REFUSED create with silence — queue off, \
                     under GMTickets.MinLevel, or category >= 11 — and a malformed body reads as \
                     exactly this. Step 1 already proved the queue is up, so a wrong create body \
                     is the live suspect.",
                    &seen[baseline.min(seen.len())..]
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }

        // Step 5: the edit, and with it the category byte on CMSG_GMTICKET_UPDATETEXT.
        Phase::EditGap { since } => {
            if now - since < WRITE_GAP_SECS {
                return;
            }
            let baseline = ticket_answers(&script).len();
            if !run_or_skip(&mut probe, &script, "5 edit", "GetGMTicket()") {
                return;
            }
            probe.phase = Phase::EditGet {
                since: now,
                baseline,
            };
        }
        Phase::EditGet { since, baseline } => {
            let seen = ticket_answers(&script);
            let want = probe.edit_text.clone();
            // Equality, never `contains`: a stray leading byte is the failure this step catches.
            if let Some(i) = matching(&seen, baseline, |(_, t)| *t == want) {
                let (cat, text) = &seen[i];
                info!(
                    "PROBE_GMTICKET: PASS (5 edit) — the edit landed and echoed EXACTLY: arg2 == \
                     {text:?} (arg1 == {cat}). No stray leading byte, so the server read our \
                     category as a category and the text as text — CMSG_GMTICKET_UPDATETEXT's \
                     `u8 type; cstring text` is right for this server."
                );
                probe.passes += 1;
                report_db_expectation(&probe, "after edit", &want);
                if !run_or_skip(&mut probe, &script, "6 abandon", "DeleteGMTicket()") {
                    return;
                }
                probe.phase = Phase::AbandonGap { since: now };
            } else if now - since > ANSWER_TIMEOUT_SECS {
                let stray = seen
                    .iter()
                    .skip(baseline)
                    .any(|(_, t)| t.len() > want.len() && t.ends_with(&want));
                error!(
                    "PROBE_GMTICKET: FAIL (5 edit) — the edited text never came back within \
                     {ANSWER_TIMEOUT_SECS}s. Wanted arg2 == {want:?}; answers seen after the \
                     baseline: {:?}. leading-byte-swallowed={stray} — if that is true, the server \
                     read our category byte as the first character of the text, which is the \
                     cmangos-classic behaviour.",
                    &seen[baseline.min(seen.len())..]
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }

        // Step 6, abandon. vmangos's `CloseTicket` keeps the row, so step 4's query still finds it.
        Phase::AbandonGap { since } => {
            if now - since < WRITE_GAP_SECS {
                return;
            }
            let baseline = ticket_answers(&script).len();
            if !run_or_skip(&mut probe, &script, "6 abandon", "GetGMTicket()") {
                return;
            }
            probe.phase = Phase::AbandonGet {
                since: now,
                baseline,
            };
        }
        Phase::AbandonGet { since, baseline } => {
            let seen = ticket_answers(&script);
            if matching(&seen, baseline, |(c, _)| *c == NO_TICKET).is_some() {
                info!(
                    "PROBE_GMTICKET: PASS (6 abandon) — UPDATE_TICKET fired with arg1 == 0 again: \
                     the ticket is gone from the player's view (vmangos marks it closed_by and \
                     keeps the row, so the db-expect rows above still resolve)"
                );
                probe.passes += 1;
            } else if now - since > ANSWER_TIMEOUT_SECS {
                error!(
                    "PROBE_GMTICKET: FAIL (6 abandon) — still holding a ticket \
                     {ANSWER_TIMEOUT_SECS}s after DeleteGMTicket() + GetGMTicket(). Answers seen \
                     after the baseline: {:?}",
                    &seen[baseline.min(seen.len())..]
                );
                probe.fails += 1;
            } else {
                return;
            }
            probe.phase = Phase::Done;
        }

        Phase::Done => {
            if probe.exited {
                return;
            }
            probe.exited = true;
            info!(
                "PROBE_GMTICKET: DONE pass={} fail={}",
                probe.passes, probe.fails
            );
            // AppExit plus a hard-exit backstop, so a teardown hang cannot keep the account held.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_GMTICKET: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}

/// Runs one binding in the live VM, or SKIPs the whole probe and returns `false`: a binding that
/// will not run is environmental, never a wire verdict.
fn run_or_skip(probe: &mut GmTicketProbe, script: &UiScript, step: &str, chunk: &str) -> bool {
    match script.run(chunk) {
        Ok(()) => {
            info!("PROBE_GMTICKET: ({step}) {chunk} run in the live VM");
            true
        }
        Err(e) => {
            warn!(
                "PROBE_GMTICKET: SKIP ({step}) — `{chunk}` would not run in the live VM: {e} \
                 (environmental, not a wire failure)"
            );
            probe.phase = Phase::Done;
            false
        }
    }
}

/// Prints what step 4's DB row must carry. `drift=` bounds the position's error; at or under
/// [`STAMP_DRIFT_EPS`] it is exact, and the map id and `ticket_type` are always exact.
fn report_db_expectation(probe: &GmTicketProbe, when: &str, message: &str) {
    let [x, y, z] = probe.create_pos;
    info!(
        "PROBE_GMTICKET: db-expect ({when}) map={} pos={x},{y},{z} ticket_type={CATEGORY_ITEM} \
         message={message:?} drift={:.3}",
        probe.create_map,
        probe.stamp_drift.unwrap_or(0.0)
    );
}

/// Straight-line distance between two WoW-space points, in yards.
fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unsolicited answers share the stream, so any match after the baseline counts, never one
    /// before it.
    #[test]
    fn a_step_matches_after_its_baseline_and_never_before_it() {
        let answers = vec![
            (0.0, String::new()),
            (4.0, "mine".to_string()),
            (0.0, String::new()),
            (4.0, "mine".to_string()),
        ];
        assert_eq!(matching(&answers, 0, |(_, t)| t == "mine"), Some(1));
        // Baseline past the first match: the later identical answer is the one taken.
        assert_eq!(matching(&answers, 2, |(_, t)| t == "mine"), Some(3));
        // Nothing after the baseline matches, and an earlier answer must not count.
        assert_eq!(matching(&answers, 4, |(_, t)| t == "mine"), None);
        assert_eq!(matching(&answers, 2, |(c, _)| *c == NO_TICKET), Some(2));
    }

    /// Step 5 asserts the text changed, which needs the edit's text to differ from the create's.
    #[test]
    fn the_create_and_edit_texts_are_distinct() {
        assert_ne!(unique_text(""), unique_text(" edited"));
        assert!(unique_text(" edited").ends_with(" edited"));
    }
}
