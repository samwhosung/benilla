//! The background networking threads — the socket half of the net bridge.
//!
//! [`spawn_net`] starts a read thread with **two park points** (decisions 0193 + 0539): it first
//! parks **pre-logon** on the credentials channel — the login screen's pause — then, once a
//! [`LoginRequest`] (credentials *and* the realmlist to dial) walks logon → realm → world
//! handshake (emitting [`SessionEvent::LoginStage`]s,
//! and [`SessionEvent::LoginFailed`] + re-park on any pre-roster failure), it **parks at character
//! select**: it emits the roster as a [`SessionEvent::CharacterList`] and blocks until the app
//! answers with a guid over the pick channel. The pick sends `CMSG_PLAYER_LOGIN` and announces
//! the connection **without waiting for the server's verdict** (the entry's head start, decision
//! 0777 — the destination's tiles stream a round-trip early), and the thread streams decoded
//! [`SessionEvent`]s from there. A `SMSG_CHARACTER_LOGIN_FAILED` in that stream is the verdict
//! arriving late: it ends the cycle like a logout ([`Cycle::LoginRefused`]). All *policy*
//! (credential auto-resubmit and its pacing, the env fast path, auto-relogin on reconnect, the
//! director's click) is app-side
//! ([`crate::login`], [`crate::char_select`]); this thread is a pure sequencer — it never sleeps.
//!
//! On a stream failure it emits a [`SessionEvent::Disconnected`] carrying
//! [`SessionEnd::Lost`] and returns to the login park; a clean in-game logout
//! ([`SessionEvent::LoggedOut`]) does the same with [`SessionEnd::LoggedOut`], and so does a
//! **refused character login** ([`SessionEvent::CharacterLoginFailed`]) — the pick is announced
//! optimistically, so a refusal is an entry taken back rather than a failure to enter. What happens next is
//! the app's, and the two answers differ (decision 1262): the logout relists, while a loss ends the
//! session at the account screen unless nobody is there to type. A single long-lived sibling write thread drains
//! [`ClientCommand`](super::ClientCommand)s down to the server; each successful connection hands it
//! the fresh [`WorldWriter`] over a swap channel, so "exactly one writer" is structural, not
//! signalled. The ECS half (draining the events into components each frame, and tearing the
//! streamed world down on disconnect) lives in the parent module; the halves communicate only
//! through the channels, so they split cleanly.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use benilla_assets::LockRecover;
use benilla_protocol::{
    host_port, messages, AuthReject, CharAction, LoginStage, Poll, SessionEnd, SessionEvent,
    WardenRequired, WorldSession, WorldWriter, WORLD_PORT,
};
use crossbeam_channel::{Receiver, Sender};

use super::{CharRequest, ChatKind, ClientCommand, RealmRequest};

/// **The inbound census** — every packet the read thread pulls off the world socket, whatever its
/// opcode, and the wall-clock of the most recent one (unix ms). Two numbers, one job: telling a
/// *silent server* apart from a *dead socket* (decision 0621).
///
/// When a remote mover freezes into a dead-reckoned runaway, the client cannot otherwise say which
/// of those it is looking at — and they are opposite bugs. If these keep climbing while the mover
/// starves, the connection is healthy and the server simply stopped relaying that unit; if they
/// freeze with it, the whole stream died without anyone noticing. Read by the runaway watch, which
/// prints them on every line ([`crate::net::motion`]).
pub(crate) static INBOUND_PACKETS: AtomicU64 = AtomicU64::new(0);
static LAST_INBOUND_UNIX_MS: AtomicU64 = AtomicU64::new(0);

/// Count one packet off the wire. Called for every `poll()` return — parsed or skipped, since a
/// packet we could not decode still proves the socket is alive.
fn note_inbound() {
    INBOUND_PACKETS.fetch_add(1, Ordering::Relaxed);
    LAST_INBOUND_UNIX_MS.store(unix_ms(), Ordering::Relaxed);
}

/// Wall-clock unix milliseconds — the one clock two processes share (the trace header's `t0` is in
/// the same units, so a mover's file and an observer's file line up).
fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// `(packets seen, ms since the last one)` — `None` for the age until a first packet has landed.
pub(crate) fn inbound_census() -> (u64, Option<u64>) {
    let last = LAST_INBOUND_UNIX_MS.load(Ordering::Relaxed);
    (
        INBOUND_PACKETS.load(Ordering::Relaxed),
        (last != 0).then(|| unix_ms().saturating_sub(last)),
    )
}

/// One submitted login attempt (decision 0539): the credentials, plus the abandon generation at
/// submit time — the thread discards the attempt at its next stage boundary if the shared counter
/// has moved (a Cancel bumps it). A counter, not a flag: a flag cleared by the *next* submit would
/// un-cancel the attempt still in flight.
#[derive(Clone)]
pub(crate) struct LoginRequest {
    pub(crate) user: String,
    pub(crate) pass: String,
    /// The realmlist to dial, `host[:port]` (decision 1667). **Per-attempt, exactly like the
    /// credentials beside it** — the login screen can now repoint the client between attempts, and
    /// an address travelling with its attempt means an edit made mid-dial cannot silently retarget
    /// the connection already in flight. It is also the only shape under which the abandon
    /// generation stays meaningful: what a cancel abandons is *this* attempt, at *that* server.
    pub(crate) host: String,
    pub(crate) generation: u64,
}

/// **The three park-answer receivers**, in the order a cycle blocks on them: credentials, realm,
/// character. One struct rather than three parameters because they are one concern — the app's side
/// of every place this thread stops and waits — and because a cycle that grew a fourth park should
/// not have to re-thread every signature to say so.
pub(super) struct Parks {
    pub(super) login_rx: Receiver<LoginRequest>,
    pub(super) realm_rx: Receiver<RealmRequest>,
    pub(super) pick_rx: Receiver<CharRequest>,
}

/// How long a realm-list refresh waits for realmd before giving up on the connection.
///
/// The refresh runs **inside whichever park is serving the realm list**, so this is a bound on how
/// long that park can be deaf to the player's click. Generous enough that an ordinary round trip
/// never trips it, short enough that a server which accepts the request and says nothing costs one
/// beat rather than the screen.
const REALM_REFRESH_TIMEOUT: Duration = Duration::from_secs(5);

/// The keepalive cadence — the real client's 30 000 ms ping timer (`0x537ff0`: the connection
/// drain arms the next ping 30 s out). vmangos *kicks* a player socket
/// whose pings repeat faster than 27 s apart more than twice (`WorldSocket::_HandlePing`), so this
/// must never shrink below that.
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// How many round trips [`PingClock`] keeps — **fifteen**, which is the reference's *usable*
/// depth even though its array holds sixteen.
///
/// `conn+0x1a6c` is 16 u32 slots, and its head/tail indices wrap at 16. But
/// the averager treats `read == write` as its **empty** sentinel (`0x537fa8`), so `HandlePong`'s
/// conditional read-index advance (`0x537de8`) makes the full state unreachable: the 16th sample
/// pushes the oldest out of view as it lands, and every reading from then on is over 15. Our
/// `VecDeque` holds what can actually be seen, so the bound is the reachable number rather than
/// the allocation — and the mean is over the same samples the reference's is.
const RTT_RING: usize = 15;

/// **The connection's ping/RTT stats** — the reference's own per-connection stats block
/// (`conn+0x1a6c` ring, `+0x1aac/+0x1ab0` head/tail, `+0x1a64` send stamp, `+0x1a68` expected
/// sequence), behind the same one lock it guards them with (`conn+0x1ac0`'s
/// critical section, taken by both `HandlePong 0x537d60` and the `GetNetStats` math at
/// `0x537f20`).
///
/// **Shared by all three threads, and the sharing is the point.** The write thread stamps each
/// `CMSG_PING` here; the **read thread** measures the `SMSG_PONG` echo against it the instant the
/// packet comes off the socket; the app reads [`Self::avg_latency_ms`] for `GetNetStats`.
///
/// That middle one is the whole reason this type owns the ring (bug B346). The measurement used
/// to happen in the ECS drain — `Instant::elapsed` called one frame or more after the pong had
/// already arrived — so the "round trip" it reported was the network round trip **plus a client
/// frame**. The meter therefore read high exactly when the client was slow, and the worst frames
/// of any session are the ones right after entering the world (`pipe_warm` alone grinds ~1051 ms
/// main-thread hitches warming the game's pipeline set; the collider builder costs hundreds of ms
/// more). A pong drained on one of those frames is recorded as a ~600 ms round trip, and at a
/// 30 s cadence the ring then carries that sample for minutes — which is exactly what B346
/// reported: a meter that opens at ~600 ms and sinks towards the truth over two or three minutes.
///
/// Measured on a localhost run (where the true RTT is 0-1 ms), the drain added **58 ms** to the
/// pong that landed 2.5 s after world enter against 8-11 ms once the session settled — and the
/// pong was the *first* event of that drain with 0 ms spent inside it, so every bit of the
/// difference was waiting for the frame to come round, not queue backlog.
///
/// The reference never had this problem, because it never measures on the game thread: `OnData`
/// (`0x537b10`) peeks the opcode and sends `SMSG_PONG` **straight** to `HandlePong` inline on the
/// network thread, bypassing the message queue every other opcode is copied onto. [`record_pong`]
/// is that function, and the read loop calls it from the same position.
///
/// [`record_pong`]: Self::record_pong
#[derive(Default)]
pub(crate) struct PingClock {
    /// Sequence of the most recent ping sent (the real client's ++counter; reset per connection).
    pub(crate) sequence: u32,
    /// When that ping went out — `None` until the first ping of a connection.
    pub(crate) sent_at: Option<Instant>,
    /// The last measured round trip (ms) — echoed in the next ping's lastRtt field, which is what
    /// the real client puts on the wire, and shown as the debug panel's readout. Kept apart from
    /// the ring because it is the *last* sample, not the average.
    pub(crate) last_rtt_ms: Option<u32>,
    /// The most recent [`RTT_RING`] round trips (ms), oldest first — the reference's own RTT
    /// history, which [`Self::avg_latency_ms`] averages for `GetNetStats`.
    rtt_ring: std::collections::VecDeque<u32>,
}

impl PingClock {
    /// **`HandlePong`** — one `SMSG_PONG` off the socket, measured and filed, on the read thread.
    /// A stale or mismatched sequence (a pong straddling a reconnect) is dropped, as the
    /// reference drops it. Returns the measured round trip when one was recorded.
    pub(crate) fn record_pong(&mut self, sequence: u32) -> Option<u32> {
        let sent = self.sent_at.filter(|_| self.sequence == sequence)?;
        let rtt = sent.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
        self.file_rtt(rtt);
        Some(rtt)
    }

    /// The filing half of [`Self::record_pong`], split out so a test can hand it exact
    /// milliseconds — the measuring half reads a real clock, and a ring test wants known numbers.
    /// The newest sample in, the oldest out at depth.
    fn file_rtt(&mut self, rtt: u32) {
        self.last_rtt_ms = Some(rtt);
        if self.rtt_ring.len() == RTT_RING {
            self.rtt_ring.pop_front();
        }
        self.rtt_ring.push_back(rtt);
    }

    /// The latency `GetNetStats` reports: the mean of the ring, truncated — `None` while it is
    /// empty (no pong since the connection came up), which the UI feed renders as the reference's
    /// own literal 0 (`0x537fd2`).
    ///
    /// Byte-for-byte the reference's own arithmetic, not an approximation of it: `0x537f20` walks
    /// read→write summing into `eax` and counting into `edi`, then does one `xor edx,edx; div edi`
    /// (`0x537fce`) — **unsigned, truncating, remainder discarded**, with no rounding term
    /// anywhere in the function. So the mean is over the samples actually present rather than a
    /// fixed sixteen, and integer `sum / len` in milliseconds *is* the reference's answer.
    pub(crate) fn avg_latency_ms(&self) -> Option<u32> {
        if self.rtt_ring.is_empty() {
            return None;
        }
        let sum: u64 = self.rtt_ring.iter().map(|&ms| u64::from(ms)).sum();
        Some((sum / self.rtt_ring.len() as u64) as u32)
    }

    /// Forget this connection's measurements — the next connection's latency is its own.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Cap on consecutive "command dropped/failed" warns per connection epoch, so a movement stream
/// during an outage can't flood the log. Reset when a fresh writer arrives.
const SEND_WARN_CAP: u32 = 8;

/// What is left of the per-process connection parameters: `WOW_CHAR`, and nothing else.
///
/// Neither the credentials nor the **address** live here any more. 0539 moved the credentials onto
/// each [`LoginRequest`]; decision 1667 moved the host the same way and for the same reason — it
/// is now a setting the player edits on the login screen (`crate::realmlist`), so a value latched
/// out of the environment once at spawn could only ever be stale. `$WOW_HOST` is still honoured;
/// it is read where every other env-overridable setting is read, by `Realmlist::default()`.
pub(super) struct NetConfig {
    /// `WOW_CHAR`, when explicitly set. Here it only names the create-if-empty character on a fresh
    /// account; as a *pick* it is app-side policy (`crate::char_select` auto-answers the roster with
    /// it — the dev fast path past the select screen).
    character: Option<String>,
}

impl NetConfig {
    pub(super) fn from_env() -> Self {
        NetConfig {
            character: std::env::var("WOW_CHAR").ok(),
        }
    }
}

/// What one wake-up at the character park asked for — the `select!`'s answer, so that every jump
/// out of that park is made in one readable `match` rather than inside a macro's expansion.
enum Parked {
    /// `CMSG_PLAYER_LOGIN` with this guid: leave the park for the world.
    Play(u64),
    /// A create/delete was serviced in place; its result byte still has to go out.
    Acted(CharAction, u8),
    /// Nothing to do — stay parked (a realm-list refresh, or a Cancel over this screen).
    StayPut,
    /// Dial this realm instead: drop the parked world session and go round again.
    Realm(benilla_protocol::RealmInfo),
    /// Select's Back — return to the pre-logon park.
    Repark,
    /// The app dropped a channel end.
    Exit,
}

/// How one connection cycle ended (the `Err` case — a stream failure — rides `Result` instead).
enum Cycle {
    /// The app dropped a channel end (exit) — end the read thread.
    Exit,
    /// Back to the pre-logon park with nothing to announce: a pre-roster failure (its
    /// [`SessionEvent::LoginFailed`] already went out), a canceled/superseded attempt, or a
    /// select-screen Back ([`CharRequest::Abandon`]).
    Repark,
    /// A clean in-game logout: emit the teardown `Disconnected` (decision 0065's path), then park.
    /// The app's pending credentials re-establish the roster silently (decision 0539 §3).
    LoggedOut,
    /// The server refused the character we picked (`SMSG_CHARACTER_LOGIN_FAILED`) — we are not in
    /// the world, and the optimistic entry this thread already announced has to be taken back.
    ///
    /// It ends the cycle **exactly like a logout**, and for the same reason: by the time the
    /// refusal lands the session has been split and the writer handed away, so there is no way
    /// back to the character-select park on this connection. The relist that follows is the
    /// logout path's, unchanged — the player sees their roster again, one reconnect they never
    /// asked about behind it. (The reference keeps its connection here; the refusal is rare
    /// enough, and the divergence invisible enough, that paying for it with the machinery a
    /// logout already has is the trade decision 0065 made for the identical case.)
    LoginRefused,
}

/// Everything [`spawn_net`] hands the app: the inbound event stream, the outbound command sender,
/// the three park-answer channels (credentials, realm, character pick), the shared abandon
/// generation the Cancel button bumps, and the ping clock.
pub(super) struct NetHandles {
    pub(super) events: Receiver<SessionEvent>,
    pub(super) commands: Sender<ClientCommand>,
    pub(super) pick: Sender<CharRequest>,
    pub(super) realm: Sender<RealmRequest>,
    pub(super) login: Sender<LoginRequest>,
    pub(super) login_abandon: Arc<AtomicU64>,
    pub(super) ping: Arc<Mutex<PingClock>>,
}

/// Spawn the background read thread (with its park/cycle loop) and the single long-lived write
/// thread. Each cycle failure emits a [`SessionEvent`] and returns to the pre-logon park; the app
/// keeps rendering regardless, and its policy decides what (if anything) answers the park.
pub(super) fn spawn_net(cfg: NetConfig, connect: bool) -> NetHandles {
    let (events_tx, events_rx) = crossbeam_channel::unbounded();
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    let (pick_tx, pick_rx) = crossbeam_channel::unbounded::<CharRequest>();
    let (realm_tx, realm_rx) = crossbeam_channel::unbounded::<RealmRequest>();
    let (login_tx, login_rx) = crossbeam_channel::unbounded::<LoginRequest>();
    let login_abandon = Arc::new(AtomicU64::new(0));
    let ping_clock = Arc::new(Mutex::new(PingClock::default()));
    if connect {
        // The writer thread outlives connections; the read thread hands it each new WorldWriter.
        let (writer_tx, writer_rx) = crossbeam_channel::unbounded::<WorldWriter>();
        let clock = Arc::clone(&ping_clock);
        // The read thread's own handle: `SMSG_PONG` is measured where it lands, not where it is
        // drained (B346 — see [`PingClock`]).
        let read_clock = Arc::clone(&ping_clock);
        let abandon = Arc::clone(&login_abandon);
        thread::Builder::new()
            .name("wow-net-write".into())
            .spawn(move || {
                // Latency-sensitive: movement packets queue here (thread QoS, decision 0609).
                benilla_world::thread_qos::promote_current_thread(
                    benilla_world::thread_qos::QosClass::UserInitiated,
                );
                writer_loop(&cmd_rx, writer_rx, &clock)
            })
            .expect("spawn wow-net-write thread");
        thread::Builder::new()
            .name("wow-net".into())
            .spawn(move || {
                benilla_world::thread_qos::promote_current_thread(
                    benilla_world::thread_qos::QosClass::UserInitiated,
                );
                let parks = Parks {
                    login_rx,
                    realm_rx,
                    pick_rx,
                };
                // Opcodes whose decoder has already been caught leaving bytes behind — one
                // announcement per opcode for the life of the process, like the dropped-packet
                // tally's first-occurrence line. Lives here, above the cycle loop, so a reconnect
                // does not re-announce what the log already says.
                let mut tails_announced = std::collections::HashSet::new();
                loop {
                    // **A cycle starts with no measurements.** Every way the last one ended — a
                    // stream failure, a logout, a re-park — lands here, so one clear covers them
                    // all, and it runs on the thread that owns the connection instead of racing
                    // in from the app's drain a frame later. (`writer_loop` clears again when the
                    // fresh writer arrives, and still has to: between the old socket dying and
                    // that handover the keepalive tick can still fire on the stale writer.)
                    read_clock.lock_recover().clear();
                    match run(
                        &cfg,
                        &events_tx,
                        &writer_tx,
                        &parks,
                        &abandon,
                        &read_clock,
                        &mut tails_announced,
                    ) {
                        Ok(Cycle::Exit) => return,
                        Ok(Cycle::Repark) => {}
                        Ok(end @ (Cycle::LoggedOut | Cycle::LoginRefused)) => {
                            // Clean logout: the Disconnected tears the streamed world down app-side
                            // (decision 0065's path); the app's pending credentials re-park us live.
                            // A refused login rides the same edge — the world it tears down is the
                            // one the entry had only started to build.
                            let reason = match end {
                                Cycle::LoggedOut => "logged out",
                                _ => "character login refused",
                            };
                            if events_tx
                                .send(SessionEvent::Disconnected {
                                    reason: reason.into(),
                                    end: SessionEnd::LoggedOut,
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(e) => {
                            // A live-stream failure — including a displacement kick, which reaches
                            // us as a bare EOF and nothing else (decision 1262). No sleep: what
                            // happens next is the app's policy, not this thread's (0539 §3).
                            bevy::log::error!("net: {e:#}");
                            if events_tx
                                .send(SessionEvent::Disconnected {
                                    reason: format!("disconnected: {e:#}"),
                                    end: SessionEnd::Lost,
                                })
                                .is_err()
                            {
                                return; // app exited mid-failure
                            }
                        }
                    }
                }
            })
            .expect("spawn wow-net thread");
    }
    // When not connecting (capture mode), the receivers/`events_tx` drop here: outbound sends
    // become harmless `Err`s (every call site ignores them) and the event stream stays empty forever.
    NetHandles {
        events: events_rx,
        commands: cmd_tx,
        pick: pick_tx,
        realm: realm_tx,
        login: login_tx,
        login_abandon,
        ping: ping_clock,
    }
}

/// One connection cycle: **park pre-logon** for credentials (decision 0539) → logon → **park at
/// the realm list** until the app names a realm → world handshake → the character roster → **park
/// at character select** until the app picks → enter the
/// world, hand the writer to the write thread, then stream decoded [`SessionEvent`]s until the
/// socket dies (`Err`), the character logs out ([`Cycle::LoggedOut`]), or the app drops a channel
/// end ([`Cycle::Exit`]). Every pre-roster failure emits [`SessionEvent::LoginFailed`] and
/// re-parks ([`Cycle::Repark`]) — never a retry loop; resubmission is the app's policy.
///
/// **Three parks, one rule.** Each is the thread blocking on a channel for an answer only the app
/// can give, and at each one the thread stays a policy-free sequencer: which credentials, which
/// realm and which character are all the app's to decide.
fn run(
    cfg: &NetConfig,
    events_tx: &Sender<SessionEvent>,
    writer_tx: &Sender<WorldWriter>,
    parks: &Parks,
    abandon: &AtomicU64,
    ping_clock: &Mutex<PingClock>,
    tails_announced: &mut std::collections::HashSet<u16>,
) -> Result<Cycle> {
    let Parks {
        login_rx,
        realm_rx,
        pick_rx,
    } = parks;
    // ── The pre-logon park (decision 0539): block for credentials. ──────────────────────────────
    bevy::log::info!("net: parked at the login screen — waiting for credentials");
    let req = match login_rx.recv() {
        Ok(req) => req,
        Err(_) => return Ok(Cycle::Exit),
    };
    // The attempt is live until the abandon generation moves past its submit-time value (Cancel
    // bumps it; the next submit carries the bumped value). Checked at every stage boundary — a
    // blocking dial can't be interrupted, but its result is discarded silently.
    let generation = req.generation;
    let canceled = move || abandon.load(Ordering::SeqCst) != generation;
    let stage = |s: LoginStage| {
        let _ = events_tx.send(SessionEvent::LoginStage { stage: s });
    };
    let fail = |refusal: Option<benilla_protocol::LoginRefusal>, reason: String| {
        let _ = events_tx.send(SessionEvent::LoginFailed {
            refusal,
            reason,
            terminal: false,
            dial: None,
        });
        Ok(Cycle::Repark)
    };
    // The dial that never opened a socket — the only failure whose cause the *screen* can act on,
    // now that the address is something a player types (1667). Kept separate from `fail` so the
    // classification happens once, at the one place holding the error object.
    let fail_dial = |dial: benilla_protocol::DialFailure, reason: String| {
        let _ = events_tx.send(SessionEvent::LoginFailed {
            refusal: None,
            reason,
            terminal: false,
            dial: Some(dial),
        });
        Ok(Cycle::Repark)
    };
    // A failure resubmitting cannot fix — the app shows it and stops (no paced retry).
    let fail_terminal = |reason: String| {
        let _ = events_tx.send(SessionEvent::LoginFailed {
            refusal: None,
            reason,
            terminal: true,
            dial: None,
        });
        Ok(Cycle::Repark)
    };

    // Logon (the dial + SRP6 exchange — one blocking sequence against realmd).
    let mut logon = {
        stage(LoginStage::Connecting);
        match benilla_protocol::logon(&req.host, &req.user, &req.pass) {
            Ok(l) => l,
            Err(e) => {
                if canceled() {
                    return Ok(Cycle::Repark);
                }
                // A server refusal carries its auth result byte (the app maps it to the client's
                // own AUTH_* string); a transport failure carries None. A failure to get a socket at
                // all carries the dial verdict, which is the one the screen can turn into advice.
                if let Some(dial) = e.downcast_ref::<benilla_protocol::DialFailure>() {
                    return fail_dial(dial.clone(), format!("{e:#}"));
                }
                let refusal = e
                    .downcast_ref::<AuthReject>()
                    .map(|r| benilla_protocol::LoginRefusal::Logon(r.code));
                return fail(refusal, format!("{e:#}"));
            }
        }
    };
    if canceled() {
        return Ok(Cycle::Repark);
    }

    // ── The realm park: **which realm to dial is the app's answer, not this thread's.** ──────
    //
    // This used to be `logon.realms.first()` — the client walked into whichever realm the server
    // happened to list first and there was no way to say otherwise. The realm the player picks is
    // policy, and policy lives app-side, so the thread does here exactly what it does at the other
    // two parks: publish the facts, block, obey.
    //
    // **This is the LOGIN-side realm list** — the one whose Cancel means "back to the login
    // screen", because the login screen is what the reference's dialog is standing on
    // (`GlueScreenInfo` has no `realmlist` entry: `RealmList` is a `frameStrata="DIALOG"` frame
    // shown over the current glue screen, and `RealmList_OnCancel` only hides it). The *other*
    // one — Change Realm — is served without leaving the character park, below.
    //
    // A **realm-less** server never reaches the park: with nothing to choose between, blocking
    // would be a hang, so the old fallback address stands and the cycle carries straight on.
    let mut realm = if logon.realms.is_empty() {
        None
    } else {
        // An answer queued during a dead cycle must not answer THIS list.
        while realm_rx.try_recv().is_ok() {}
        loop {
            if events_tx
                .send(SessionEvent::RealmList {
                    realms: logon.realms.clone(),
                })
                .is_err()
            {
                return Ok(Cycle::Exit);
            }
            bevy::log::info!(
                "net: parked at the realm list — {} realm(s) published",
                logon.realms.len()
            );
            let Ok(req) = realm_rx.recv() else {
                return Ok(Cycle::Exit);
            };
            if canceled() {
                return Ok(Cycle::Repark);
            }
            match req {
                // The realm list's Cancel, over the login screen — the dialog hides and the
                // screen underneath is the one we came from.
                RealmRequest::Abandon => return Ok(Cycle::Repark),
                // The reference re-requests the list every 5 s while its window is up. A refresh
                // that fails is not an error the player needs: the list we hold stays on screen
                // and `Logon` stops asking (see `Logon::refresh_realms`).
                RealmRequest::Refresh => {
                    logon.refresh_realms(REALM_REFRESH_TIMEOUT);
                }
                // By name, so a list that changed under the click cannot enter the wrong realm.
                // A name that is no longer there falls through to re-publishing the list, which
                // is the honest answer: the realm they clicked is gone.
                RealmRequest::Enter(name) => {
                    if let Some(realm) = logon.realms.iter().find(|r| r.name == name) {
                        break Some(realm.clone());
                    }
                }
            }
        }
    };

    // ── The world half, once per realm. ─────────────────────────────────────────────────────────
    //
    // **Change Realm re-enters this loop; it does not end the cycle.** The SRP6 session key
    // authenticates against any world server on the account's list and the list is already in
    // hand, so switching realms costs one world dial — no re-authentication, no return to the
    // login screen, and (this is the part that mattered) no window in which the app is looking at
    // a realm list while this thread has already walked on to the character park.
    'realm: loop {
        // Named, because "connected to 127.0.0.1:8085" cannot tell a chosen realm from the
        // realm-less fallback that dials the same address.
        if let Some(r) = &realm {
            bevy::log::info!(
                "net: realm {:?} ({} of {})",
                r.name,
                r.address,
                logon.realms.len()
            );
        }
        let world_addr = realm
            .as_ref()
            .map(|r| r.address.clone())
            // Strip any explicit auth `:port` off the realmlist — the fallback world port is its own.
            .unwrap_or_else(|| format!("{}:{}", host_port(&req.host, WORLD_PORT).0, WORLD_PORT));

        stage(LoginStage::Handshaking);
        // The realm we are dialing, for the queue dialog to name — the roster that would otherwise
        // carry it is on the far side of the queue, which is exactly when the name is wanted.
        let realm_name = realm.as_ref().map(|r| r.name.clone());
        // Report our place, and keep waiting only while the attempt is still wanted. A queue can
        // last minutes, so unlike every other handshake stage it has to test the abandon generation
        // itself — otherwise a Cancel would close the dialog while this thread quietly held its place
        // in line and then walked into the world anyway.
        let mut on_queue = |position: Option<u32>| {
            let _ = events_tx.send(SessionEvent::LoginQueued {
                position,
                realm: realm_name.clone(),
            });
            !canceled()
        };
        let mut session = match WorldSession::connect_queued(
            &world_addr,
            &req.user,
            logon.session_key,
            &mut on_queue,
        ) {
            Ok(s) => s,
            Err(e) => {
                if canceled() {
                    return Ok(Cycle::Repark);
                }
                // A Warden refusal is the server's own answer, not a transport fault — say it plainly
                // rather than wrapping it in handshake noise.
                if let Some(w) = e.downcast_ref::<WardenRequired>() {
                    return fail_terminal(w.to_string());
                }
                // The world server's own refusal, in its own enum — the screen owes the player the
                // authored `AUTH_*` string for it, which it cannot recover from a formatted message.
                if let Some(r) = e.downcast_ref::<benilla_protocol::WorldAuthReject>() {
                    return fail(
                        Some(benilla_protocol::LoginRefusal::World(r.code)),
                        format!("{e:#}"),
                    );
                }
                return fail(None, format!("world handshake with {world_addr}: {e:#}"));
            }
        };
        // The handshake can now block for minutes (the queue), so a cancel that landed while it did
        // must not be overtaken by the roster it is about to fetch.
        if canceled() {
            return Ok(Cycle::Repark);
        }

        // The roster (creating a starter character on a fresh account so PLAYER_LOGIN has a target).
        // Failures here are still pre-roster: surface as LoginFailed, re-park. (An immediately-run
        // closure, so the `?`-shaped sequence borrows `session` only for the call.)
        let mut characters = match (|| -> Result<Vec<benilla_protocol::Character>> {
            let mut characters = session.char_enum()?;
            if characters.is_empty() {
                let name = cfg.character.as_deref().unwrap_or("One");
                let starter = messages::CharCreateReq {
                    name: name.to_string(),
                    race: messages::RACE_HUMAN,
                    class: messages::CLASS_WARRIOR,
                    gender: messages::GENDER_MALE,
                    skin: 0,
                    face: 0,
                    hair_style: 0,
                    hair_color: 0,
                    facial_hair: 0,
                };
                match session.create_character(&starter)? {
                    messages::CHAR_CREATE_SUCCESS | messages::CHAR_CREATE_NAME_IN_USE => {}
                    other => bail!("character creation failed: result {other:#x}"),
                }
                characters = session.char_enum()?;
            }
            Ok(characters)
        })() {
            Ok(c) => c,
            Err(e) => {
                if canceled() {
                    return Ok(Cycle::Repark);
                }
                if let Some(w) = e.downcast_ref::<WardenRequired>() {
                    return fail_terminal(w.to_string());
                }
                return fail(None, format!("character roster: {e:#}"));
            }
        };
        if canceled() {
            return Ok(Cycle::Repark);
        }
        // A pick queued during a dead cycle must not answer THIS roster — the app re-sends what it
        // still wants (its pending pick), and a deliberate logout must land on the list, not bounce
        // straight back into the world off a stale pick. Same for a realm answer aimed at a list this
        // park has already left behind.
        while pick_rx.try_recv().is_ok() {}
        while realm_rx.try_recv().is_ok() {}
        if events_tx
            .send(SessionEvent::CharacterList {
                characters: characters.clone(),
                realm: realm.clone(),
            })
            .is_err()
        {
            return Ok(Cycle::Exit);
        }

        // Park at character select until the app answers (its pick policy: auto-relogin on reconnect,
        // the WOW_CHAR fast path, or the director's click). Create/delete requests are serviced *in
        // place* (decision 0423): send, read the one result byte, on success re-enum + re-emit the
        // roster, emit the result, and loop back to the park — the thread stays a policy-free blocking
        // sequencer. The channel only closes on app exit. If the server kicked the parked socket
        // meanwhile, the login below fails → the caller cycles → a fresh roster → the app auto-resends
        // its pick — self-healing, no keep-alive needed (decision 0193).
        bevy::log::info!("net: parked at character select");
        let guid = loop {
            // **Two channels, one park.** The realm list is a dialog, not a screen (`RealmList.xml`
            // is `frameStrata="DIALOG"` over whatever glue screen is up), so Change Realm raises it
            // *over* character select and this thread must keep serving both while it is open —
            // refreshing the list on the still-live realmd connection, and staying exactly where it
            // is if the player cancels. The version that ended the cycle here is what stranded the
            // app: it walked on to this park while the app was still looking at a realm list, so the
            // next Cancel sent an `Abandon` no one was listening for and every later login queued
            // behind a thread parked two parks away.
            //
            // Named `pick` rather than `req`: the cycle's own `req` (the credentials) is still live.
            //
            // **No `break` or `continue` inside the `select!` arms.** The macro expands into a
            // loop of its own, so loop control written in an arm would bind to *that* loop rather
            // than this park — a silent spin. The arms answer with a [`Parked`] and every jump is
            // made below, in plain sight.
            let answer = crossbeam_channel::select! {
                recv(realm_rx) -> req => match req {
                    Err(_) => Parked::Exit,
                    // `RequestRealmList` — refresh on the realmd socket this cycle still holds,
                    // then republish. Also how Change Realm asks for its first list.
                    Ok(RealmRequest::Refresh) => {
                        logon.refresh_realms(REALM_REFRESH_TIMEOUT);
                        if events_tx
                            .send(SessionEvent::RealmList { realms: logon.realms.clone() })
                            .is_err()
                        {
                            Parked::Exit
                        } else {
                            Parked::StayPut
                        }
                    }
                    // `RealmList_OnCancel` over character select: the dialog hides and **nothing
                    // else happens**. The session it is standing on is untouched, which is the
                    // whole point of the reference making this a frame rather than a screen.
                    Ok(RealmRequest::Abandon) => Parked::StayPut,
                    // `ChangeRealm(category, index)`: drop this world session and dial the chosen
                    // realm with the session key we already hold. A name that is no longer on the
                    // list changes nothing, which is the honest answer.
                    Ok(RealmRequest::Enter(name)) => logon
                        .realms
                        .iter()
                        .find(|r| r.name == name)
                        .cloned()
                        .map_or(Parked::StayPut, Parked::Realm),
                },
                recv(pick_rx) -> pick => match pick {
                    Err(_) => Parked::Exit,
                    Ok(CharRequest::Enter(guid)) => Parked::Play(guid),
                    // Select's Back (decision 0539): drop the parked session, return to the login park.
                    Ok(CharRequest::Abandon) => Parked::Repark,
                    Ok(CharRequest::Create(create)) => {
                        Parked::Acted(CharAction::Create, session.create_character(&create)?)
                    }
                    Ok(CharRequest::Delete(target)) => {
                        Parked::Acted(CharAction::Delete, session.delete_character(target)?)
                    }
                },
            };
            let (action, code) = match answer {
                Parked::Exit => return Ok(Cycle::Exit),
                Parked::Repark => return Ok(Cycle::Repark),
                Parked::StayPut => continue,
                Parked::Play(guid) => break guid,
                Parked::Realm(chosen) => {
                    realm = Some(chosen);
                    continue 'realm;
                }
                Parked::Acted(action, code) => (action, code),
            };
            // Success (create's SUCCESS or delete's SUCCESS) changed the roster — re-enumerate and
            // re-emit it *before* the result, so the screen already has the fresh list when it reacts.
            let changed = match action {
                CharAction::Create => code == messages::CHAR_CREATE_SUCCESS,
                CharAction::Delete => code == messages::CHAR_DELETE_SUCCESS,
            };
            if changed {
                characters = session.char_enum()?;
                if events_tx
                    .send(SessionEvent::CharacterList {
                        characters: characters.clone(),
                        realm: realm.clone(),
                    })
                    .is_err()
                {
                    return Ok(Cycle::Exit);
                }
            }
            if events_tx
                .send(SessionEvent::CharActionResult { action, code })
                .is_err()
            {
                return Ok(Cycle::Exit);
            }
        };
        let name = characters
            .iter()
            .find(|c| c.guid == guid)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        session.player_login(refuse_once(guid))?;
        session.set_active_mover(guid)?;

        let billing_time_rested = session.billing_time_rested();
        let tutorial_flags = session.take_tutorial_flags();
        // `SMSG_ADDON_INFO`'s verdict, paired back against the block we sent — the reply carries no
        // names (decision 2175). No reply at all means no hidden addons *and* no Lua index space;
        // the distinction is the script's to draw, so both arrive as the same empty list here and
        // the "did it answer" bit rides separately.
        let statuses = session.take_addon_info();
        let addon_info = statuses.as_deref().map(|statuses| {
            benilla_protocol::messages::hidden_from_reply(
                statuses,
                &benilla_protocol::messages::STOCK_SECURE_ADDONS,
            )
        });
        match (&statuses, &addon_info) {
            (Some(statuses), Some(hidden)) => bevy::log::info!(
                "net: SMSG_ADDON_INFO — {} record(s), {} hidden from the AddOn index space",
                statuses.len(),
                hidden.len()
            ),
            _ => bevy::log::warn!(
                "net: no SMSG_ADDON_INFO — GetNumAddOns() stays 0, as the reference's would"
            ),
        }
        let (mut reader, writer) = session.into_split()?;
        if events_tx
            .send(SessionEvent::Connected {
                self_guid: guid,
                name,
                billing_time_rested,
                tutorial_flags,
                addon_info,
            })
            .is_err()
        {
            return Ok(Cycle::Exit);
        }
        if writer_tx.send(writer).is_err() {
            // The writer thread only ends when the app drops every command sender — app exit.
            return Ok(Cycle::Exit);
        }
        bevy::log::info!("net: connected to {world_addr}");

        // Blocking read loop. `poll` skips packets the message layer can't parse (keeping the stream
        // aligned); a long run of consecutive skips means the stream desynced — guard against a busy spin.
        let (mut skip_run, mut skip_logged) = (0u32, 0u32);
        loop {
            let polled = reader.poll()?;
            note_inbound(); // one packet off the wire, parsed or not — the census counts liveness
            match polled {
                Poll::Events {
                    opcode,
                    events,
                    tail,
                } => {
                    skip_run = 0;
                    // **The decode-length instrument** (decision 2265 §B1). A body is
                    // length-framed, so a decoder shorter than the server's layout succeeds
                    // silently and the field it never read is invisible from outside. This is
                    // the one line that shows it — once per opcode, at info, and NEVER a skip:
                    // the packet decoded and its events are real; a trailing field we have no
                    // use for is drift to look at, not a packet to drop.
                    if tail > 0 && tails_announced.insert(opcode) {
                        bevy::log::info!(
                            "net: opcode {} ({opcode:#06x}) left {tail} trailing byte(s) after \
                             decode (first occurrence; announced once per opcode)",
                            benilla_protocol::messages::opcode_name(opcode).unwrap_or("?"),
                        );
                    }
                    // The full inbound opcode stream (tag `in`, decision 0624) — the last place a
                    // packet could hide. `skip` covers what failed to parse and `rly` covers what
                    // reached the mover replay; between them sits the packet that parsed into *no*
                    // event, which no instrument could see. With this line every packet off the wire
                    // is accounted for by name, so "the server stopped relaying" and "we dropped it on
                    // the floor" are finally different pictures instead of the same silence.
                    if benilla_assets::trace::enabled() {
                        benilla_assets::trace::line(
                            "in",
                            &format!(
                                "{opcode:#06x} {} ev={}",
                                benilla_protocol::messages::opcode_name(opcode).unwrap_or("?"),
                                events.len()
                            ),
                        );
                    }
                    for ev in events {
                        // **The pong bypass** (B346), the reference's own shape: `OnData 0x537b10`
                        // peeks the opcode and hands `SMSG_PONG` straight to `HandlePong 0x537d60`
                        // inline, instead of copying it onto the queue the game thread drains. So do
                        // we — the round trip is measured here, against the clock the write thread
                        // stamped, and the event stops here. Measuring it after a drain instead added
                        // a whole client frame to every reading, which is a frame's worth of the
                        // client's own slowness reported as the server's distance.
                        if let SessionEvent::Pong { sequence } = ev {
                            if let Some(rtt) = ping_clock.lock_recover().record_pong(sequence) {
                                bevy::log::debug!("net: pong seq={sequence} rtt={rtt}ms");
                            }
                            continue;
                        }
                        // A confirmed logout — and a refused character login — end the cycle
                        // *after* the app hears about it, so the screen always has the reason
                        // before the teardown that acts on it.
                        let ends = match ev {
                            SessionEvent::LoggedOut => Some(Cycle::LoggedOut),
                            SessionEvent::CharacterLoginFailed { .. } => Some(Cycle::LoginRefused),
                            _ => None,
                        };
                        // Receiver dropped → the app exited; end the thread cleanly.
                        if events_tx.send(ev).is_err() {
                            return Ok(Cycle::Exit);
                        }
                        if let Some(cycle) = ends {
                            return Ok(cycle);
                        }
                    }
                }
                Poll::Skipped { opcode, reason } => {
                    skip_run += 1;
                    // **Every** skip, uncapped, into the trace (tag `skip`, decision 0623). A packet that
                    // arrives and fails to parse is indistinguishable, from outside, from one that never
                    // arrived: the inbound census counts it either way, and no `rly` line is emitted
                    // either way. That ambiguity is what made a starving remote mover unattributable —
                    // so the skips get their own line, with the opcode that died.
                    if benilla_assets::trace::enabled() {
                        benilla_assets::trace::line(
                            "skip",
                            &format!("opcode={opcode:#06x} {reason}"),
                        );
                    }
                    // Log which packet we dropped (opcode + message name), capped so it can't itself
                    // flood — enough to capture a post-teleport burst of unparseable object updates.
                    if skip_logged < 40 {
                        bevy::log::warn!("net: skipping unparseable packet — {reason}");
                        skip_logged += 1;
                    }
                    // Feed the app's dropped-packet tally (the debug panel instrument) — a parse
                    // *error* is a coverage gap the same as an unknown opcode, just a worse one.
                    if events_tx
                        .send(SessionEvent::PacketDropped {
                            opcode,
                            unparseable: true,
                        })
                        .is_err()
                    {
                        return Ok(Cycle::Exit);
                    }
                    if skip_run > 1024 {
                        return Err(anyhow!("world stream desynced after 1024 skipped packets"));
                    }
                }
            }
        }
    } // 'realm
}

/// **The refusal rehearsal** (`WOW_REFUSE_LOGIN=1`): make the server refuse the *first* character
/// login of the run, so the path a refusal takes is walkable on demand.
///
/// A refused `CMSG_PLAYER_LOGIN` is rare on a local server and cannot be staged by hand — every
/// guid the screen can offer is a real character on the account — which is exactly why the client
/// sat on an unclearable loading screen for so long without anyone noticing. This sends the pick
/// with a guid vmangos cannot read as a player (`HIGHGUID_UNIT`), which its
/// `!packet.guid.IsPlayer()` guard refuses immediately, touching nothing else. The guid on the
/// wire is the only thing altered: everything the app is told still names the real character, so
/// what runs afterwards is the genuine path and not a special case.
///
/// **Once**, not every pick — so the run continues into the recovery (dialog → relist → a second
/// pick that works) rather than looping on a refusal the `WOW_CHAR` fast path would re-answer
/// forever. Inert without the env.
fn refuse_once(guid: u64) -> u64 {
    /// `HIGHGUID_UNIT | 1` — a guid no player can have, and one the server rejects before it
    /// looks anything up. Shared with `benilla-protocol`'s `login_refusal_probe`.
    const NOT_A_PLAYER: u64 = 0xF130_0000_0000_0001;
    static ARMED: std::sync::OnceLock<AtomicU64> = std::sync::OnceLock::new();
    let armed = ARMED
        .get_or_init(|| AtomicU64::new(u64::from(std::env::var_os("WOW_REFUSE_LOGIN").is_some())));
    if armed.swap(0, Ordering::SeqCst) == 1 {
        bevy::log::warn!("net: WOW_REFUSE_LOGIN — sending this pick with a non-player guid");
        NOT_A_PLAYER
    } else {
        guid
    }
}

/// Drain the writer's sent-packet log into the trace as `out` lines — one per packet that reached
/// the socket, by opcode name and body length. A no-op unless the `out` tag armed the log when the
/// connection was handed over.
fn trace_sends(w: &mut WorldWriter) {
    w.drain_sent(|opcode, len| {
        benilla_assets::trace::line(
            "out",
            &format!(
                "{opcode:#06x} {} len={len}",
                benilla_protocol::messages::opcode_name(opcode).unwrap_or("?")
            ),
        );
    });
}

/// The single write thread: `select!` between app commands, writer swaps from the read thread, and
/// the 30 s keepalive tick ([`PING_INTERVAL`] — the real client's ping cadence). While disconnected
/// (no writer yet, or the socket died under the current one), commands drop with a capped warn and
/// the tick no-ops — they are meaningless without a live session, and the server re-syncs our state
/// from the reconnect handshake anyway. Ends when the app drops every command sender.
fn writer_loop(
    cmd_rx: &Receiver<ClientCommand>,
    mut writer_rx: Receiver<WorldWriter>,
    ping_clock: &Mutex<PingClock>,
) {
    let mut writer: Option<WorldWriter> = None;
    let mut warned = 0u32;
    // **Armed by the connection, re-armed by each send — never free-running** (`0x537ff0`:
    // `now - lastSent - 30000 >= 0`, evaluated at the connection's own drain tail,
    // and `0x537bcf` stamps `lastSent` with the current tick at connect). It was a process-
    // lifetime `tick`, which is a different clock in two ways that both showed: the first ping of
    // a session landed anywhere in the 30 s after entering the world rather than at the end of it
    // — sometimes squarely inside the world-load storm, which is where B346's inflated sample
    // came from — and the cadence never re-phased on a reconnect.
    let mut ping_tick = crossbeam_channel::never();
    loop {
        crossbeam_channel::select! {
            recv(writer_rx) -> w => match w {
                Ok(mut w) => {
                    // Arm the outbound opcode trace for this connection (tag `out`). Armed here
                    // rather than at construction because the sink is an app-side concern and a
                    // writer outlives none of them; a fresh socket starts a fresh log.
                    if benilla_assets::trace::enabled_for("out") {
                        w.watch_sends();
                    }
                    writer = Some(w);
                    warned = 0;
                    // A fresh connection restarts the keepalive from scratch, like the real
                    // client: sequence 1 is the new socket's first ping, and a stale in-flight
                    // pong from the old socket can no longer match.
                    ping_clock.lock_recover().clear();
                    // The connect stamp: the first keepalive of a connection is a full interval
                    // out, not on the next drain (`0x537bcf` — verified; the alternative reading,
                    // that a zeroed stamp fires one immediately, is what the bytes ruled out).
                    ping_tick = crossbeam_channel::after(PING_INTERVAL);
                }
                // The read thread ended (app exit). Stop selecting the dead channel (a
                // disconnected receiver is always-ready — it would busy-spin the select);
                // drain commands until the app side closes too.
                Err(_) => writer_rx = crossbeam_channel::never(),
            },
            recv(ping_tick) -> _ => {
                // The keepalive (30 s, the verified real-client cadence). Sent only while a live
                // writer exists — the parked char-select socket goes without (decision 0193's
                // self-healing covers it), and vmangos would kick a faster cadence as overspeed.
                // Disarmed unless a send is attempted, so a dead connection stops pinging and the
                // next writer re-arms from its own connect.
                ping_tick = crossbeam_channel::never();
                if let Some(w) = writer.as_mut() {
                    // Re-armed from the SEND, so the interval is measured the way the reference
                    // measures it — and a failed write still counts, since what the cadence is
                    // spacing is our attempts on the socket, not the server's answers.
                    ping_tick = crossbeam_channel::after(PING_INTERVAL);
                    let (sequence, last_rtt) = {
                        let mut c = ping_clock.lock_recover();
                        c.sequence += 1;
                        c.sent_at = Some(Instant::now());
                        // `lastRtt`: the most recent single sample, never the mean (VERIFIED —
                        // the reference reads `ring[write-1]`, folded into
                        // `[esi + 4*w + 0x1a68]` at `0x537e87`). **One deliberate divergence.**
                        // Its `jbe` guard at `0x537e85` substitutes a literal 0 whenever the write
                        // index is 0 — true before the first pong, and again on every sixteenth
                        // ping thereafter, so a healthy real client reports a 0 ms latency to the
                        // server once in sixteen. That is an artefact of its index arithmetic, not
                        // a behaviour: nothing renders it, only the server stores it. We send 0
                        // for the first case (no sample yet, which is the honest value) and the
                        // real sample for the rest.
                        (c.sequence, c.last_rtt_ms.unwrap_or(0))
                    };
                    if let Err(e) = w.ping(sequence, last_rtt) {
                        if warned < SEND_WARN_CAP {
                            bevy::log::warn!("net: ping send failed: {e:#}");
                            warned += 1;
                        }
                    }
                    trace_sends(w);
                }
            },
            recv(cmd_rx) -> cmd => {
                let Ok(cmd) = cmd else { return }; // all app senders dropped → app exit
                let Some(w) = writer.as_mut() else {
                    // No live writer: the session is gone and this command evaporates. Traced
                    // unconditionally — this is the state in which a client keeps *deciding* to send
                    // movement (`snd` lines) that no one will ever receive (decision 0621).
                    // **Both lines name the command.** They used to write a fixed string, so a
                    // login that dropped five commands before the writer existed said only that
                    // five of something went missing — and since `writer` is set exactly once and
                    // never reset, this can only ever fire before the first `player_login`, which
                    // makes the WHICH the entire question. A census of all 322 send sites could
                    // not answer it from the source; `cmd` is owned and unused here and
                    // `ClientCommand` derives `Debug`, so one login now answers it outright.
                    if benilla_assets::trace::enabled() {
                        benilla_assets::trace::line(
                            "wire",
                            &format!("DROPPED — no live session: {cmd:?}"),
                        );
                    }
                    if warned < SEND_WARN_CAP {
                        bevy::log::warn!("net: dropping command — not connected: {cmd:?}");
                        warned += 1;
                    }
                    continue;
                };
                let result = match cmd {
                    ClientCommand::Move {
                        kind,
                        flags,
                        pos,
                        orientation,
                        pitch,
                        fall_time,
                        jump,
                        transport,
                    } => w.send_movement(
                        kind.opcode(),
                        flags,
                        pos,
                        orientation,
                        pitch,
                        fall_time,
                        jump,
                        transport,
                    ),
                    ClientCommand::MoveSplineDone {
                        flags,
                        pos,
                        orientation,
                        spline_id,
                    } => w.move_spline_done(flags, pos, orientation, spline_id),
                    ClientCommand::MoveTimeSkipped { guid, lag_ms } => {
                        w.move_time_skipped(guid, lag_ms)
                    }
                    ClientCommand::ForceSpeedAck {
                        kind,
                        guid,
                        counter,
                        speed,
                        flags,
                        pos,
                        orientation,
                        pitch,
                        fall_time,
                        jump,
                        transport,
                    } => w.force_speed_change_ack(
                        kind,
                        guid,
                        counter,
                        speed,
                        flags,
                        pos,
                        orientation,
                        pitch,
                        fall_time,
                        jump,
                        transport,
                    ),
                    ClientCommand::SetActiveMover { guid } => w.set_active_mover(guid),
                    ClientCommand::NotActiveMover {
                        guid,
                        flags,
                        pos,
                        orientation,
                        fall_time,
                    } => w.move_not_active_mover(guid, flags, pos, orientation, fall_time),
                    ClientCommand::FarSight { engage } => w.far_sight(engage),
                    ClientCommand::TeleportAck { guid, counter } => w.teleport_ack(guid, counter),
                    ClientCommand::WorldportAck => w.worldport_ack(),
                    ClientCommand::SetSelection { guid } => w.set_selection(guid),
                    ClientCommand::CancelAutoRepeat => w.cancel_auto_repeat(),
                    ClientCommand::CancelCast { spell_id } => w.cancel_cast(spell_id),
                    ClientCommand::CancelChannelling { spell_id } => w.cancel_channelling(spell_id),
                    ClientCommand::Chat { kind, target, text } => match kind {
                        ChatKind::Say => w.send_chat(&text),
                        ChatKind::Yell => w.send_yell(&text),
                        ChatKind::Emote => w.send_emote_chat(&text),
                        ChatKind::Whisper => {
                            w.send_whisper(target.as_deref().unwrap_or_default(), &text)
                        }
                        ChatKind::Party => w.send_party(&text),
                        ChatKind::Raid => w.send_raid(&text),
                        ChatKind::RaidLeader => w.send_raid_leader(&text),
                        ChatKind::RaidWarning => w.send_raid_warning(&text),
                        ChatKind::Guild => w.send_guild(&text),
                        ChatKind::Officer => w.send_officer(&text),
                        ChatKind::Battleground => w.send_battleground(&text),
                        ChatKind::BattlegroundLeader => w.send_battleground_leader(&text),
                        ChatKind::Afk => w.send_afk(&text),
                        ChatKind::Dnd => w.send_dnd(&text),
                        ChatKind::Channel => {
                            w.send_channel(target.as_deref().unwrap_or_default(), &text)
                        }
                    },
                    // The addon lane (decision 1235). The distribution arrived as an enum and the
                    // map is TOTAL — no "unknown, guess SAY" arm exists, which is what the enum
                    // seam is for — so the whole arm is one call.
                    ClientCommand::AddonMessage { distribution, text } => {
                        w.send_addon_message(super::addon_wire_chat_type(distribution), &text)
                    }
                    ClientCommand::JoinChannel { name, password } => {
                        w.join_channel(&name, &password)
                    }
                    ClientCommand::LeaveChannel { name } => w.leave_channel(&name),
                    ClientCommand::ChannelList { name } => w.channel_list(&name),
                    ClientCommand::RandomRoll { min, max } => w.random_roll(min, max),
                    ClientCommand::ChannelOwner { name } => w.channel_owner(&name),
                    ClientCommand::ChannelSetOwner { name, player } => {
                        w.channel_set_owner(&name, &player)
                    }
                    ClientCommand::ChannelPassword { name, password } => {
                        w.channel_password(&name, &password)
                    }
                    ClientCommand::ChannelModerator { name, player } => {
                        w.channel_moderator(&name, &player)
                    }
                    ClientCommand::ChannelUnmoderator { name, player } => {
                        w.channel_unmoderator(&name, &player)
                    }
                    ClientCommand::ChannelMute { name, player } => w.channel_mute(&name, &player),
                    ClientCommand::ChannelUnmute { name, player } => {
                        w.channel_unmute(&name, &player)
                    }
                    ClientCommand::ChannelInvite { name, player } => {
                        w.channel_invite(&name, &player)
                    }
                    ClientCommand::ChannelKick { name, player } => w.channel_kick(&name, &player),
                    ClientCommand::ChannelBan { name, player } => w.channel_ban(&name, &player),
                    ClientCommand::ChannelUnban { name, player } => {
                        w.channel_unban(&name, &player)
                    }
                    ClientCommand::ChannelAnnouncements { name } => w.channel_announcements(&name),
                    ClientCommand::ChannelModerate { name } => w.channel_moderate(&name),
                    ClientCommand::PlayedTime => w.played_time(),
                    ClientCommand::NameQuery { guid } => w.name_query(guid),
                    ClientCommand::CreatureQuery { entry, guid } => w.creature_query(entry, guid),
                    ClientCommand::PetNameQuery { pet_number, guid } => {
                        w.pet_name_query(pet_number, guid)
                    }
                    ClientCommand::ItemQuery { entry, guid } => w.item_query(entry, guid),
                    ClientCommand::UseItem {
                        bag_index,
                        slot,
                        spell_index,
                        target,
                    } => w.use_item(bag_index, slot, spell_index, target),
                    ClientCommand::OpenItem { bag_index, slot } => w.open_item(bag_index, slot),
                    ClientCommand::WrapItem {
                        gift_bag,
                        gift_slot,
                        item_bag,
                        item_slot,
                    } => w.wrap_item(gift_bag, gift_slot, item_bag, item_slot),
                    ClientCommand::AutoEquipItem { bag_index, slot } => {
                        w.auto_equip_item(bag_index, slot)
                    }
                    ClientCommand::SetAmmo { entry } => w.set_ammo(entry),
                    ClientCommand::SwapInvItem { src_slot, dst_slot } => {
                        w.swap_inv_item(src_slot, dst_slot)
                    }
                    ClientCommand::SwapItem {
                        dst_bag,
                        dst_slot,
                        src_bag,
                        src_slot,
                    } => w.swap_item(dst_bag, dst_slot, src_bag, src_slot),
                    ClientCommand::AutoStoreBagItem {
                        src_bag,
                        src_slot,
                        dst_bag,
                    } => w.auto_store_bag_item(src_bag, src_slot, dst_bag),
                    ClientCommand::SplitItem {
                        src_bag,
                        src_slot,
                        dst_bag,
                        dst_slot,
                        count,
                    } => w.split_item(src_bag, src_slot, dst_bag, dst_slot, count),
                    ClientCommand::DestroyItem {
                        bag_index,
                        slot,
                        count,
                    } => w.destroy_item(bag_index, slot, count),
                    ClientCommand::CastSpell { spell_id, target } => w.cast_spell(spell_id, target),
                    ClientCommand::CastSpellAtDest { spell_id, dest } => {
                        w.cast_spell_at_dest(spell_id, dest)
                    }
                    ClientCommand::CastSpellAtSource { spell_id, src } => {
                        w.cast_spell_at_source(spell_id, src)
                    }
                    ClientCommand::CancelAura { spell_id } => w.cancel_aura(spell_id),
                    ClientCommand::SetActionButton { button, packed } => {
                        w.set_action_button(button, packed)
                    }
                    ClientCommand::SetActionBarToggles { toggles } => {
                        w.set_actionbar_toggles(toggles)
                    }
                    ClientCommand::PetAction {
                        pet_guid,
                        packed,
                        target_guid,
                    } => w.pet_action(pet_guid, packed, target_guid),
                    ClientCommand::PetSetAction { pet_guid, entries } => {
                        w.pet_set_action(pet_guid, &entries)
                    }
                    ClientCommand::PetStopAttack { pet_guid } => w.pet_stop_attack(pet_guid),
                    ClientCommand::PetCancelAura { pet_guid, spell_id } => {
                        w.pet_cancel_aura(pet_guid, spell_id)
                    }
                    ClientCommand::PetSpellAutocast {
                        pet_guid,
                        spell_id,
                        enabled,
                    } => w.pet_spell_autocast(pet_guid, spell_id, enabled),
                    ClientCommand::PetAbandon { pet_guid } => w.pet_abandon(pet_guid),
                    ClientCommand::PetRename { pet_guid, name } => w.pet_rename(pet_guid, &name),
                    ClientCommand::AttackSwing { guid } => w.attack_swing(guid),
                    ClientCommand::AttackStop => w.attack_stop(),
                    ClientCommand::SetSheathed { state } => w.set_sheathed(state),
                    ClientCommand::StandStateChange { state } => w.stand_state_change(state),
                    ClientCommand::MountSpecial => w.mount_special(),
                    ClientCommand::TextEmote { text_id, target } => w.text_emote(text_id, target),
                    ClientCommand::GossipHello { guid } => w.gossip_hello(guid),
                    ClientCommand::GossipSelectOption { guid, option } => {
                        // v1 sends no code — coded options are greyed, never selected (decision 0081).
                        w.gossip_select_option(guid, option, None)
                    }
                    ClientCommand::NpcTextQuery { text_id, guid } => w.npc_text_query(text_id, guid),
                    ClientCommand::ListInventory { guid } => w.list_inventory(guid),
                    ClientCommand::BuyItem {
                        vendor,
                        entry,
                        count,
                    } => w.buy_item(vendor, entry, count),
                    ClientCommand::BuyItemInSlot {
                        vendor,
                        entry,
                        bag_guid,
                        bag_slot,
                        count,
                    } => w.buy_item_in_slot(vendor, entry, bag_guid, bag_slot, count),
                    ClientCommand::SellItem {
                        vendor,
                        item_guid,
                        count,
                    } => w.sell_item(vendor, item_guid, count),
                    ClientCommand::BuybackItem { vendor, slot } => w.buyback_item(vendor, slot),
                    ClientCommand::RepairItem { vendor, item_guid } => {
                        w.repair_item(vendor, item_guid)
                    }
                    ClientCommand::GmTicketCreate {
                        category,
                        map,
                        pos,
                        text,
                    } => w.gm_ticket_create(category, map, pos, &text),
                    ClientCommand::GmTicketUpdate { category, text } => {
                        w.gm_ticket_updatetext(category, &text)
                    }
                    ClientCommand::GmTicketGet => w.gm_ticket_get(),
                    ClientCommand::GmTicketDelete => w.gm_ticket_delete(),
                    ClientCommand::GmTicketSystemStatus => w.gm_ticket_system_status(),
                    ClientCommand::BinderActivate { binder } => w.binder_activate(binder),
                    ClientCommand::SummonResponse { summoner } => w.summon_response(summoner),
                    ClientCommand::TalentWipeConfirm { trainer } => w.talent_wipe_confirm(trainer),
                    ClientCommand::ForceLogout => w.player_logout(),
                    ClientCommand::AreaSpiritHealerQueue { healer } => {
                        w.area_spirit_healer_queue(healer)
                    }
                    ClientCommand::AreaSpiritHealerQuery { healer } => {
                        w.area_spirit_healer_query(healer)
                    }
                    ClientCommand::BattlefieldPort { map_id, accept } => {
                        w.battlefield_port(map_id, accept)
                    }
                    ClientCommand::RequestBattlefieldScoreData => {
                        w.request_battlefield_score_data()
                    }
                    ClientCommand::LeaveBattlefield { map_id } => w.leave_battlefield(map_id),
                    ClientCommand::MeetingStoneJoin { go_guid } => w.meeting_stone_join(go_guid),
                    ClientCommand::MeetingStoneLeave => w.meeting_stone_leave(),
                    ClientCommand::MeetingStoneStatusQuery => w.meeting_stone_status_query(),
                    ClientCommand::TutorialFlag { id } => w.tutorial_flag(id),
                    ClientCommand::TutorialClear => w.tutorial_clear(),
                    ClientCommand::TutorialReset => w.tutorial_reset(),
                    ClientCommand::BattlefieldList { map_id } => w.battlefield_list(map_id),
                    ClientCommand::RequestBattlefieldPositions => {
                        w.request_battlefield_positions()
                    }
                    ClientCommand::TabardVendorActivate { npc } => w.tabard_vendor_activate(npc),
                    ClientCommand::SaveGuildEmblem { vendor, design } => {
                        w.save_guild_emblem(vendor, design)
                    }
                    ClientCommand::BattlemasterHello { npc } => w.battlemaster_hello(npc),
                    ClientCommand::BattlemasterJoin {
                        battlemaster,
                        map_id,
                        instance_id,
                        as_group,
                    } => w.battlemaster_join(battlemaster, map_id, instance_id, as_group),
                    ClientCommand::BattlefieldJoin {
                        map_id,
                        instance_id,
                        as_group,
                    } => w.battlefield_join(map_id, instance_id, as_group),
                    ClientCommand::BattlefieldStatusRequest => w.battlefield_status(),
                    ClientCommand::PetUnlearn { trainer } => w.pet_unlearn(trainer),
                    ClientCommand::BankerActivate { guid } => w.banker_activate(guid),
                    ClientCommand::BuyBankSlot { guid } => w.buy_bank_slot(guid),
                    ClientCommand::AutoBankItem { bag, slot } => w.autobank_item(bag, slot),
                    ClientCommand::AutoStoreBankItem { bag, slot } => {
                        w.autostore_bank_item(bag, slot)
                    }
                    ClientCommand::TrainerList { trainer } => w.trainer_list(trainer),
                    ClientCommand::TrainerBuySpell { trainer, spell_id } => {
                        w.trainer_buy_spell(trainer, spell_id)
                    }
                    ClientCommand::ListStabledPets { npc } => w.list_stabled_pets(npc),
                    ClientCommand::StablePet { npc } => w.stable_pet(npc),
                    ClientCommand::UnstablePet { npc, pet_number } => {
                        w.unstable_pet(npc, pet_number)
                    }
                    ClientCommand::StableSwapPet { npc, pet_number } => {
                        w.stable_swap_pet(npc, pet_number)
                    }
                    ClientCommand::BuyStableSlot { npc } => w.buy_stable_slot(npc),
                    ClientCommand::LearnTalent { talent_id, rank } => {
                        w.learn_talent(talent_id, rank)
                    }
                    ClientCommand::UnlearnSkill { skill_id } => w.unlearn_skill(skill_id),
                    ClientCommand::SetFactionAtWar {
                        rep_list_id,
                        at_war,
                    } => w.set_faction_at_war(rep_list_id, at_war),
                    ClientCommand::SetFactionInactive {
                        rep_list_id,
                        inactive,
                    } => w.set_faction_inactive(rep_list_id, inactive),
                    ClientCommand::SetWatchedFaction { rep_list_id } => {
                        w.set_watched_faction(rep_list_id)
                    }
                    ClientCommand::GameObjUse { guid } => w.gameobj_use(guid),
                    ClientCommand::AreaTrigger { trigger_id } => w.area_trigger(trigger_id),
                    ClientCommand::GameObjectQuery { entry, guid } => w.gameobject_query(entry, guid),
                    ClientCommand::PageTextQuery { page_id, guid } => {
                        w.page_text_query(page_id, guid)
                    }
                    ClientCommand::CastSpellGameObject { spell_id, go_guid } => {
                        w.cast_spell_gameobject(spell_id, go_guid)
                    }
                    ClientCommand::CastSpellItem {
                        spell_id,
                        item_guid,
                    } => w.cast_spell_item(spell_id, item_guid),
                    ClientCommand::LootMasterGive { guid, slot, target } => {
                        w.loot_master_give(guid, slot, target)
                    }
                    ClientCommand::Loot { guid } => w.loot(guid),
                    ClientCommand::AutostoreLootItem { slot } => w.autostore_loot_item(slot),
                    ClientCommand::LootMoney => w.loot_money(),
                    ClientCommand::LootRelease { guid } => w.loot_release(guid),
                    ClientCommand::LootRoll {
                        looted_target,
                        item_slot,
                        roll_type,
                    } => w.loot_roll(looted_target, item_slot, roll_type),
                    ClientCommand::QuestgiverQuery { npc, quest } => {
                        w.questgiver_query_quest(npc, quest)
                    }
                    ClientCommand::QuestgiverAccept { npc, quest } => {
                        w.questgiver_accept_quest(npc, quest)
                    }
                    ClientCommand::QuestgiverComplete { npc, quest } => {
                        w.questgiver_complete_quest(npc, quest)
                    }
                    ClientCommand::QuestgiverRequestReward { npc, quest } => {
                        w.questgiver_request_reward(npc, quest)
                    }
                    ClientCommand::QuestgiverChooseReward { npc, quest, choice } => {
                        w.questgiver_choose_reward(npc, quest, choice)
                    }
                    ClientCommand::QuestQuery { quest } => w.quest_query(quest),
                    ClientCommand::QuestgiverStatusQuery { npc } => w.questgiver_status_query(npc),
                    ClientCommand::QuestgiverHello { npc } => w.questgiver_hello(npc),
                    ClientCommand::QuestlogRemove { slot } => w.questlog_remove_quest(slot),
                    ClientCommand::PushQuestToParty { quest } => w.push_quest_to_party(quest),
                    ClientCommand::QuestConfirmAccept { quest } => w.quest_confirm_accept(quest),
                    ClientCommand::QuestPushResult { sharer, msg } => {
                        w.quest_push_result(sharer, msg)
                    }
                    ClientCommand::GetMailList { mailbox } => w.get_mail_list(mailbox),
                    ClientCommand::SendMail {
                        mailbox,
                        receiver,
                        subject,
                        body,
                        stationery,
                        item_guid,
                        money,
                        cod,
                    } => w.send_mail(
                        mailbox, &receiver, &subject, &body,
                        // The stationery the player selected (1970) and package 0 — vmangos
                        // discards both and stores MAIL_STATIONERY_DEFAULT (41, decision 0544),
                        // but the wire carries what the client chose, as the reference's does.
                        stationery, 0, item_guid, money, cod,
                    ),
                    ClientCommand::MailTakeMoney { mailbox, mail_id } => {
                        w.mail_take_money(mailbox, mail_id)
                    }
                    ClientCommand::MailTakeItem { mailbox, mail_id } => {
                        w.mail_take_item(mailbox, mail_id)
                    }
                    ClientCommand::MailMarkAsRead { mailbox, mail_id } => {
                        w.mail_mark_as_read(mailbox, mail_id)
                    }
                    ClientCommand::MailReturnToSender { mailbox, mail_id } => {
                        w.mail_return_to_sender(mailbox, mail_id)
                    }
                    ClientCommand::MailDelete { mailbox, mail_id } => {
                        w.mail_delete(mailbox, mail_id)
                    }
                    ClientCommand::MailCreateTextItem { mailbox, mail_id } => {
                        w.mail_create_text_item(mailbox, mail_id)
                    }
                    ClientCommand::ItemTextQuery { text_id, mail_id } => {
                        w.item_text_query(text_id, mail_id)
                    }
                    ClientCommand::QueryNextMailTime => w.query_next_mail_time(),
                    // The auction house arc (decision 1511 P0) — the CMSG verbs onto the
                    // P0 writers; the auctioneer guid rides on every one.
                    ClientCommand::AuctionHello { auctioneer } => w.auction_hello(auctioneer),
                    ClientCommand::AuctionListItems {
                        auctioneer,
                        list_from,
                        searched_name,
                        level_min,
                        level_max,
                        slot_id,
                        main_category,
                        sub_category,
                        quality,
                        usable,
                    } => w.auction_list_items(
                        auctioneer,
                        list_from,
                        &searched_name,
                        level_min,
                        level_max,
                        slot_id,
                        main_category,
                        sub_category,
                        quality,
                        usable,
                    ),
                    ClientCommand::AuctionListOwnerItems {
                        auctioneer,
                        list_from,
                    } => w.auction_list_owner_items(auctioneer, list_from),
                    ClientCommand::AuctionListBidderItems {
                        auctioneer,
                        list_from,
                        auction_ids,
                    } => w.auction_list_bidder_items(auctioneer, list_from, &auction_ids),
                    ClientCommand::AuctionSellItem {
                        auctioneer,
                        item_guid,
                        bid,
                        buyout,
                        etime_minutes,
                    } => w.auction_sell_item(auctioneer, item_guid, bid, buyout, etime_minutes),
                    ClientCommand::AuctionPlaceBid {
                        auctioneer,
                        auction_id,
                        price,
                    } => w.auction_place_bid(auctioneer, auction_id, price),
                    ClientCommand::AuctionRemoveItem {
                        auctioneer,
                        auction_id,
                    } => w.auction_remove_item(auctioneer, auction_id),
                    ClientCommand::QueryTime => w.query_time(),
                    // The inspect request (decision 0631) — no reply is awaited; see the writer.
                    ClientCommand::Inspect { target } => w.inspect(target),
                    // The inspect-honor query (decision 1512) — this one IS answered; the reply
                    // rides the same opcode back.
                    ClientCommand::InspectHonorStats { target } => w.inspect_honor_stats(target),
                    // The player-trade arc (decision 0592) — the CMSG verbs onto the P0 writers.
                    ClientCommand::InitiateTrade { target } => w.initiate_trade(target),
                    ClientCommand::BeginTrade => w.begin_trade(),
                    ClientCommand::BusyTrade => w.busy_trade(),
                    ClientCommand::IgnoreTrade => w.ignore_trade(),
                    ClientCommand::AcceptTrade => w.accept_trade(),
                    ClientCommand::UnacceptTrade => w.unaccept_trade(),
                    ClientCommand::CancelTrade => w.cancel_trade(),
                    ClientCommand::SetTradeGold { copper } => w.set_trade_gold(copper),
                    ClientCommand::SetTradeItem {
                        trade_slot,
                        bag,
                        slot,
                    } => w.set_trade_item(trade_slot, bag, slot),
                    ClientCommand::ClearTradeItem { trade_slot } => w.clear_trade_item(trade_slot),
                    ClientCommand::Logout => w.logout_request(),
                    ClientCommand::LogoutCancel => w.logout_cancel(),
                    ClientCommand::CompleteCinematic => w.complete_cinematic(),
                    ClientCommand::NextCinematicCamera => w.next_cinematic_camera(),
                    ClientCommand::MoveModeAck {
                        guid,
                        counter,
                        mode,
                        apply,
                        flags,
                        pos,
                        orientation,
                    } => w.move_mode_ack(guid, counter, mode, apply, flags, (pos, orientation)),
                    ClientCommand::KnockBackAck {
                        guid,
                        counter,
                        launch,
                        flags,
                        pos,
                        orientation,
                        transport,
                    } => {
                        w.knock_back_ack(guid, counter, launch, flags, (pos, orientation), transport)
                    }
                    ClientCommand::RepopRequest => w.repop_request(),
                    ClientCommand::CorpseQuery => w.corpse_query(),
                    ClientCommand::ReclaimCorpse { corpse } => w.reclaim_corpse(corpse),
                    ClientCommand::SelfRes => w.self_res(),
                    ClientCommand::SpiritHealerActivate { npc } => w.spirit_healer_activate(npc),
                    ClientCommand::ResurrectResponse { caster, accept } => {
                        w.resurrect_response(caster, accept)
                    }
                    ClientCommand::GroupInvite { name } => w.group_invite(&name),
                    ClientCommand::GroupAccept => w.group_accept(),
                    ClientCommand::GroupDecline => w.group_decline(),
                    ClientCommand::GroupUninvite { name } => w.group_uninvite(&name),
                    ClientCommand::GroupSetLeader { guid } => w.group_set_leader(guid),
                    ClientCommand::GroupLeave => w.group_disband(),
                    ClientCommand::GroupRaidConvert => w.group_raid_convert(),
                    ClientCommand::RequestPartyMemberStats { guid } => {
                        w.request_party_member_stats(guid)
                    }
                    ClientCommand::LootMethod {
                        method,
                        master,
                        threshold,
                    } => w.loot_method(method, master, threshold),
                    ClientCommand::SetRaidTarget { icon, guid } => w.raid_target_set(icon, guid),
                    ClientCommand::MinimapPing { x, y } => w.minimap_ping(x, y),
                    ClientCommand::GroupChangeSubGroup { name, group } => {
                        w.group_change_sub_group(&name, group)
                    }
                    ClientCommand::GroupSwapSubGroup { name, other } => {
                        w.group_swap_sub_group(&name, &other)
                    }
                    ClientCommand::GroupAssistantLeader { guid, grant } => {
                        w.group_assistant_leader(guid, grant)
                    }
                    ClientCommand::ReadyCheckStart => w.ready_check_start(),
                    ClientCommand::ReadyCheckAnswer { ready } => w.ready_check_answer(ready),
                    ClientCommand::RequestRaidInfo => w.request_raid_info(),
                    ClientCommand::ResetInstances => w.reset_instances(),
                    ClientCommand::DuelAccepted { arbiter } => w.duel_accepted(arbiter),
                    ClientCommand::DuelCancelled { arbiter } => w.duel_cancelled(arbiter),
                    ClientCommand::TogglePvp => w.toggle_pvp(),
                    ClientCommand::ToggleHelm => w.toggle_helm(),
                    ClientCommand::ToggleCloak => w.toggle_cloak(),
                    ClientCommand::FriendListRequest => w.friend_list(),
                    ClientCommand::AddFriend { name } => w.add_friend(&name),
                    ClientCommand::SetLookingForGroup { slots, comment } => {
                        w.set_looking_for_group(slots, &comment)
                    }
                    ClientCommand::DelFriend { guid } => w.del_friend(guid),
                    ClientCommand::AddIgnore { name } => w.add_ignore(&name),
                    ClientCommand::DelIgnore { guid } => w.del_ignore(guid),
                    ClientCommand::Who { request } => w.who(&request),
                    ClientCommand::ChatIgnored { guid } => w.chat_ignored(guid),
                    ClientCommand::GuildQuery { guild_id } => w.guild_query(guild_id),
                    ClientCommand::GuildCreate { name } => w.guild_create(&name),
                    ClientCommand::GuildInvite { name } => w.guild_invite(&name),
                    ClientCommand::GuildAccept => w.guild_accept(),
                    ClientCommand::GuildDecline => w.guild_decline(),
                    ClientCommand::GuildInfoRequest => w.guild_info(),
                    ClientCommand::GuildRosterRequest => w.guild_roster(),
                    ClientCommand::GuildPromote { name } => w.guild_promote(&name),
                    ClientCommand::GuildDemote { name } => w.guild_demote(&name),
                    ClientCommand::GuildLeave => w.guild_leave(),
                    ClientCommand::GuildRemove { name } => w.guild_remove(&name),
                    ClientCommand::GuildDisband => w.guild_disband(),
                    ClientCommand::GuildLeader { name } => w.guild_leader(&name),
                    ClientCommand::GuildMotd { motd } => w.guild_motd(&motd),
                    ClientCommand::GuildRank {
                        rank_id,
                        rights,
                        name,
                    } => w.guild_rank(rank_id, rights, &name),
                    ClientCommand::GuildAddRank { name } => w.guild_add_rank(&name),
                    ClientCommand::GuildDelRank => w.guild_del_rank(),
                    ClientCommand::GuildSetPublicNote { name, note } => {
                        w.guild_set_public_note(&name, &note)
                    }
                    ClientCommand::GuildSetOfficerNote { name, note } => {
                        w.guild_set_officer_note(&name, &note)
                    }
                    ClientCommand::GuildInfoText { text } => w.guild_info_text(&text),
                    // The petition family (decision 1672) — founding a guild.
                    ClientCommand::PetitionShowList { npc } => w.petition_show_list(npc),
                    ClientCommand::PetitionBuy { npc, name } => w.petition_buy(npc, &name),
                    ClientCommand::PetitionShowSignatures { item } => {
                        w.petition_show_signatures(item)
                    }
                    ClientCommand::PetitionSign { item, byte } => w.petition_sign(item, byte),
                    ClientCommand::OfferPetition { item, player } => w.offer_petition(item, player),
                    ClientCommand::TurnInPetition { item } => w.turn_in_petition(item),
                    ClientCommand::PetitionQuery { petition_id, item } => {
                        w.petition_query(petition_id, item)
                    }
                    ClientCommand::PetitionRename { item, name } => w.petition_rename(item, &name),
                    ClientCommand::PetitionDecline { item } => w.petition_decline(item),
                    ClientCommand::TaxiNodeStatusQuery { guid } => w.taxi_node_status_query(guid),
                    ClientCommand::TaxiQueryNodes { guid } => w.taxi_query_available_nodes(guid),
                    ClientCommand::ActivateTaxi {
                        guid,
                        source_node,
                        dest_node,
                    } => w.activate_taxi(guid, source_node, dest_node),
                    ClientCommand::ActivateTaxiExpress {
                        guid,
                        total_cost,
                        nodes,
                    } => w.activate_taxi_express(guid, total_cost, &nodes),
                };
                // **What actually reached the socket** (tag `wire`, decision 0621). The controller's
                // `snd` line is written before the command is even queued, so it records a decision,
                // not a transmission — a client whose session died goes on producing `snd` lines into
                // a dead channel forever, which is exactly the ambiguity that cost us a hunt. Only
                // failures are traced: a silent `wire` log beside a busy `snd` log means every packet
                // went out.
                if let Err(e) = result {
                    if benilla_assets::trace::enabled() {
                        benilla_assets::trace::line("wire", &format!("SEND FAILED: {e:#}"));
                    }
                    if warned < SEND_WARN_CAP {
                        bevy::log::warn!("net: send failed: {e:#}");
                        warned += 1;
                    }
                }
                // **What actually reached the socket, by name** (tag `out`) — the outbound twin of
                // the `in` line above. One command can be more than one packet, so this drains
                // rather than naming the command: the log is the writer's own, recorded after each
                // successful write, so a line here is a transmission and never an intention. It is
                // what `wire`'s "a silent failure log beside a busy `snd` log means every packet
                // went out" was standing in for, said directly.
                trace_sends(w);
            },
        }
    }
}

#[cfg(test)]
mod rtt_tests {
    use super::{PingClock, RTT_RING};
    use std::time::Instant;

    /// Arm the clock as the write thread does — a ping sent "now" under `sequence`.
    fn sent(clock: &mut PingClock, sequence: u32) {
        clock.sequence = sequence;
        clock.sent_at = Some(Instant::now());
    }

    /// The reported latency is the ring's mean, and the ring is bounded at the reference depth —
    /// so a single spike moves the meter by a fifteenth, not the whole way (which is the point of
    /// averaging at all: the ping cadence is 30 s, and one bad sample must not sit on a red bar
    /// for seven minutes).
    #[test]
    fn the_reported_latency_is_the_mean_of_a_bounded_ring() {
        let mut clock = PingClock::default();
        assert_eq!(clock.avg_latency_ms(), None, "no pong yet");

        clock.file_rtt(40);
        assert_eq!(clock.avg_latency_ms(), Some(40));
        clock.file_rtt(60);
        assert_eq!(clock.avg_latency_ms(), Some(50), "the mean, not the last");
        assert_eq!(
            clock.last_rtt_ms,
            Some(60),
            "the last sample stays separate"
        );

        // Fill past the ring: only the newest RTT_RING samples count, so the two above age out.
        for _ in 0..RTT_RING {
            clock.file_rtt(100);
        }
        assert_eq!(clock.rtt_ring.len(), RTT_RING);
        assert_eq!(clock.avg_latency_ms(), Some(100));

        clock.clear();
        assert_eq!(clock.avg_latency_ms(), None, "a disconnect forgets it all");
        assert_eq!(clock.last_rtt_ms, None);
    }

    /// **B346's regression.** A pong is only a measurement if it answers the ping we are timing:
    /// the sequence has to match, and there has to be a live send to measure against. The echo of
    /// a ping from a dead socket (a pong straddling a reconnect, where `clear` has already run)
    /// must not enter the ring — it would be timed against nothing, or worse against the *new*
    /// connection's send, and one bogus sample sits on the meter for minutes at a 30 s cadence.
    #[test]
    fn only_the_pong_we_are_waiting_for_is_recorded() {
        let mut clock = PingClock::default();
        assert_eq!(clock.record_pong(1), None, "no ping is in flight");

        sent(&mut clock, 7);
        assert_eq!(
            clock.record_pong(6),
            None,
            "a stale sequence is not our ping"
        );
        assert_eq!(clock.record_pong(8), None, "nor is one we never sent");
        assert!(clock.rtt_ring.is_empty(), "neither entered the ring");

        assert!(clock.record_pong(7).is_some(), "the one we are timing");
        assert_eq!(clock.rtt_ring.len(), 1);
        assert!(clock.avg_latency_ms().is_some());

        // The reconnect edge: cleared, so the old socket's echo has nothing to match.
        clock.clear();
        assert_eq!(
            clock.record_pong(7),
            None,
            "the dead socket's echo is dropped"
        );
        assert_eq!(clock.avg_latency_ms(), None);
    }
}
