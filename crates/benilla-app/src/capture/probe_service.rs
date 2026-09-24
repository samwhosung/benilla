//! The **NPC-service ladder** live probe (`WOW_PROBE_SERVICE=1`) — decision 1861's end-to-end
//! instrument, and the answer to "does right-clicking a trainer or a quest giver really open their
//! window instead of a gossip menu?".
//!
//! 1861 replaced a cursor-kind dispatch with the reference's own first-match-wins walk over
//! `UNIT_NPC_FLAGS` ([`crate::target::click::service_arm`]; `0x5f0130`). **Bit 0 (GOSSIP) is
//! tested first**, so the flag — not the profession — decides: a trainer or questgiver that also
//! carries GOSSIP still opens a gossip menu, and only a flagless one opens its own window. That
//! precedence is the whole question, and it is invisible to a unit test: the bits come off the
//! wire, and what appears on screen is the *server's* answer to the opcode the ladder picked.
//!
//! So the probe walks four real Northshire/Stormwind NPCs, one per interesting flag shape, and for
//! each one reports the wire's `UNIT_NPC_FLAGS`, the arm the shipped ladder takes, the opcode it
//! sends, and which window actually opened:
//!
//! | NPC | `npc_flags` | arm | window |
//! |---|---|---|---|
//! | Marshal McBride (197) | `0x03` GOSSIP\|QUESTGIVER | Gossip | gossip menu |
//! | Llane Beshere (911) | `0x13` GOSSIP\|QUESTGIVER\|TRAINER | Gossip | gossip menu |
//! | *any pure questgiver with an offer* | `0x02`, GOSSIP clear | Questgiver | quest frame |
//! | Alma Jainrose (812) | `0x10` TRAINER only | Trainer | trainer frame |
//!
//! The flag column is live-DB verified against the local vmangos (`mangos`,
//! `SELECT entry, name, npc_flags FROM creature_template`), and reported again from the wire on
//! every run — a template edit shows up as a note beside the reading rather than as a mystery FAIL.
//!
//! **The ladder is called, never re-implemented.** The probe reads the same two inputs the click
//! reads (`ObjectStore::unit_npc_flags` and [`QuestGiver::status`]) and calls the shipped
//! `service_arm`/`service_action`; only the mouse hit-test and the cursor's range gray sit outside
//! it. A private copy of the bit table is exactly how a probe goes quietly stale (the B249 icon
//! map), so there isn't one.
//!
//! **The pure-questgiver row is found by predicate, not by entry.** Whether Deputy Willem has
//! anything for *this* probe character depends on what it has already turned in, and the bit-1 arm
//! is gated on that status (`[unit+0xcb8] ∉ {0,1}`). So that leg hops into Northshire and takes the
//! first streamed unit that is QUESTGIVER, not GOSSIP, and actually has an offer — reporting which
//! one it picked. If the character has cleared the whole valley it SKIPs and says so.
//!
//! One `PROBE_SERVICE: <leg> PASS/FAIL/SKIP <detail>` line per leg, then a final
//! `PROBE_SERVICE: DONE pass=<n> fail=<m> skip=<k>`. A wrong arm or a wrong window is a FAIL; an
//! environmental problem (the `.go` refused, the NPC never streamed, no offer left in the valley)
//! is a SKIP with the reason.
//!
//! ## The run recipe
//!
//! ```text
//! WOW_DATA=WoW/Data WOW_USER=probe4 WOW_PASS=pprobe4 WOW_CHAR=Probefour \
//!     WOW_UNATTENDED=1 WOW_PROBE_SERVICE=1 cargo run -q -p benilla
//! ```
//! (the checkout's probe identity (`.probe-identity`, or WOW_USER/WOW_PASS/WOW_CHAR — the `probe` skill)). Non-combat, GM mode
//! left as found, nothing bought and nothing turned in: every leg opens a window and closes it
//! client-side, which is what the reference's own close does (`ui_gossip`: there is no
//! `CMSG_GOSSIP_CLOSE` in 1.12).

use bevy::prelude::*;

use benilla_protocol::EntityKind;

use super::probes::ProbeClock;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::target::click::{service_action, service_arm, ServiceAction, ServiceArm};
use crate::target::cursor_mode::npc_flags as f;
use crate::ui_gossip::GossipState;
use benilla_ui::script::UiScript;

use crate::ui_merchant::MerchantOpen;
use crate::ui_quest::QuestGiver;
use crate::ui_session::InteractNpc;
use crate::ui_trainer::TrainerOpen;

/// How wide to look around a `.go` landing for the leg's NPC — **and no wider than the reference's
/// own service reach**, because the click cannot act past it either: beyond
/// [`crate::target::SERVICE_RANGE_SQ`] (5.5556 yd, `0xc4c28c`/`0xb4b32c`) the cursor goes gray, the
/// click sends nothing, and vmangos's `GetNPCIfCanInteractWith` refuses the opcode silently.
///
/// This constant is the reason the questgiver leg first read as a client defect: its hop point was
/// an invented Northshire "hub" ~29 yd from Deputy Willem, so the probe called a ladder the click
/// would never have reached, the server dropped the packet on its own distance check, and the
/// timeout printed a FAIL about the client (docs/METHOD.md §6 — prove the run before reading the
/// result). Every hop point is now an NPC's own spawn, and the scan re-checks the reach.
const SCAN_RANGE_SQ: f32 = crate::target::SERVICE_RANGE_SQ;
/// Let the hop land and the tile stream before the first scan.
const SETTLE_SECS: f64 = 3.0;
/// How long a leg waits for its NPC to stream in before calling the hop environmental.
const SCAN_TIMEOUT_SECS: f64 = 20.0;
/// How long a leg waits for its window after the opcode goes out. Generous for the same reason the
/// binder probe's is: the observer is starved during a terrain load, never the wire.
const WINDOW_TIMEOUT_SECS: f64 = 20.0;
/// How long the re-click gate's latch may lag the window it belongs to.
///
/// It no longer lags by construction: `feed_interact_npc` is seated after the net apply since
/// decision 2022, so the token arms on the very frame the window opens. It USED to be allowed to
/// lag a frame (`ui_session` was "deliberately unordered" against the apply, on decision 1741's
/// reading that a window's first frame is invisible) — and "one frame" and "never" are the same
/// reading if you sample once, which is exactly what the first run of this assert did: it failed
/// the quest and trainer legs, whose windows open on the packet, and passed the gossip legs only
/// because the gossip frame holds shut for several frames waiting on its greeting query (B292).
/// The poll stays for the other reason a single sample lies — a starved observer during a terrain
/// load — and prints how long the latch actually took.
const GATE_TIMEOUT_SECS: f64 = 3.0;

/// **The vendor fork's leg** (decision 1914) — Brother Danil, `creature_template.entry = 152`,
/// `npc_flags = 4` (VENDOR and nothing else, so the ladder cannot reach him by any other arm),
/// spawned in Northshire at his own `creature.position_*` (live-DB verified this session).
const VENDOR_ENTRY: u32 = 152;
const VENDOR_AT: [f32; 3] = [-8901.59, -112.716, 82.0314];
const VENDOR_MAP: u32 = 0;
/// Tough Jerky — a 1-copper vendor trash item, `.additem`'d so the sell leg has something of its
/// own to sell and never touches whatever the probe character was already carrying.
const SELL_ITEM_ENTRY: u32 = 117;
/// How long to wait for `.additem` to land in a bag, and for the sold item to leave it.
const BAG_TIMEOUT_SECS: f64 = 20.0;

/// Which NPC a leg is looking for.
enum Ident {
    /// A specific `creature_template.entry` — the identity check that cannot be confused by a
    /// neighbour who happens to share a flag.
    Entry(u32),
    /// The bit-1 arm's own shape: QUESTGIVER set, GOSSIP clear, and an offer live for THIS
    /// character (`SMSG_QUESTGIVER_STATUS` ∉ {0,1}) — see the module doc.
    PureQuestgiverWithOffer,
}

/// Which window the leg's arm must put on screen.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Window {
    Gossip,
    Quest,
    Trainer,
}

/// One leg: hop here, find this NPC, run the ladder on it, expect this arm and this window.
struct Leg {
    /// What the log calls it.
    name: &'static str,
    ident: Ident,
    map: u32,
    /// The spawn points to try, in order — each an NPC's own `creature.position_*`, so the landing
    /// is inside the service reach. A leg with several is one whose subject depends on what this
    /// character has already turned in (the questgiver leg); it moves to the next candidate when
    /// the one it hopped to has nothing to offer.
    at: &'static [[f32; 3]],
    /// `creature_template.npc_flags` as the world DB held it when this probe was written — printed
    /// beside the wire's value so a template edit reads as a note rather than a mystery.
    db_flags: u32,
    want_arm: ServiceArm,
    want_window: Window,
}

/// The four flag shapes worth a live reading (module doc's table). Northshire first — three of the
/// four legs share one hop's worth of terrain — then the Stormwind first-aid trainer.
const LEGS: &[Leg] = &[
    Leg {
        name: "Marshal McBride — GOSSIP|QUESTGIVER",
        ident: Ident::Entry(197),
        map: 0,
        at: &[[-8902.59, -162.606, 82.0223]],
        db_flags: 0x3,
        want_arm: ServiceArm::Gossip,
        want_window: Window::Gossip,
    },
    Leg {
        name: "Llane Beshere — GOSSIP|QUESTGIVER|TRAINER",
        ident: Ident::Entry(911),
        map: 0,
        at: &[[-8918.36, -208.411, 82.3088]],
        db_flags: 0x13,
        want_arm: ServiceArm::Gossip,
        want_window: Window::Gossip,
    },
    Leg {
        name: "a pure questgiver with an offer — QUESTGIVER, no GOSSIP",
        ident: Ident::PureQuestgiverWithOffer,
        map: 0,
        // Deputy Willem (823), Eagan Peltskinner (196), Falkhaan Isenstrider (6774) — the three
        // pure questgivers of Northshire, each at its own spawn (live-DB verified this session).
        at: &[
            [-8933.54, -136.523, 83.4466],
            [-8869.22, -163.237, 80.9719],
            [-9044.56, -45.9817, 88.4193],
        ],
        db_flags: 0x2,
        want_arm: ServiceArm::Questgiver,
        want_window: Window::Quest,
    },
    Leg {
        name: "Alma Jainrose — TRAINER, no GOSSIP",
        ident: Ident::Entry(812),
        map: 0,
        at: &[[-9237.78, -2041.65, 78.1678]],
        db_flags: 0x10,
        want_arm: ServiceArm::Trainer,
        want_window: Window::Trainer,
    },
];

pub(crate) struct ProbeServicePlugin;

impl Plugin for ProbeServicePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ServiceProbe>()
            .add_systems(Update, service_probe);
    }
}

#[derive(Resource, Default)]
struct ServiceProbe {
    phase: Phase,
    passes: u32,
    fails: u32,
    skips: u32,
    /// How many frames the current scan actually got to poll in. A starved observer and a missing
    /// NPC look identical in a SKIP line otherwise — and on a loaded machine the first is far more
    /// likely: at load 61 this probe logged `frame hitch: ~1010 ms` on repeat (the ~1 fps regime
    /// decisions 0713/0777/1355 name) and its 20-second scan window bought about twenty samples.
    /// A leg runner answers the same problem with a load guard (1157); a probe that does one thing and
    /// exits is better served by reporting what it actually got.
    polls: u32,
    /// Latched once [`Phase::Done`] has fired its exit (never re-fire on a later frame).
    exited: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    /// Not in-world yet, or between legs: `.go` for leg `i` still to send.
    #[default]
    Start,
    Hop {
        i: usize,
        /// Which of the leg's spawn points we hopped to.
        hop: usize,
        sent_at: f64,
    },
    /// The arm's action has gone out; waiting for the window it must open.
    Await {
        i: usize,
        since: f64,
        guid: u64,
    },
    /// The window is open; waiting for the re-click gate's `"npc"` token to name its NPC
    /// (decision 1905). Its own phase because the latch is allowed to lag the window by a frame —
    /// see [`GATE_TIMEOUT_SECS`].
    Gate {
        i: usize,
        since: f64,
        guid: u64,
    },
    /// The vendor fork's own chain (decision 1914) — `.go` to the pure vendor.
    VendorHop {
        sent_at: f64,
    },
    /// The EMPTY-cursor leg: `CMSG_LIST_INVENTORY` sent, waiting for the merchant window.
    VendorOpen {
        since: f64,
        guid: u64,
    },
    /// `.additem` sent, waiting for the jerky to appear in a bag so it can be picked up.
    VendorArm {
        since: f64,
        guid: u64,
    },
    /// The HELD-cursor leg: `CMSG_SELL_ITEM` sent, waiting for the slot to empty.
    VendorSell {
        since: f64,
        bag: i64,
        slot: u32,
    },
    Done,
}

/// The bag slot holding [`SELL_ITEM_ENTRY`], as `"bag,slot"`, or `""` — asked of the live VM
/// through the same bindings a player's bag UI uses, so the probe never needs its own view of the
/// container fields.
fn find_item_slot(script: &UiScript) -> Option<(i64, u32)> {
    let found = script
        .eval::<String>(&format!(
            "for b = 0, 4 do \
               local n = GetContainerNumSlots(b) or 0 \
               for s = 1, n do \
                 local l = GetContainerItemLink(b, s) \
                 if l and string.find(l, \"Hitem:{SELL_ITEM_ENTRY}\") then return b .. \",\" .. s end \
               end \
             end return \"\""
        ))
        .unwrap_or_default();
    let (b, s) = found.split_once(',')?;
    Some((b.parse().ok()?, s.parse().ok()?))
}

/// A flag word as the log wants it — the hex plus the bit names, so a reader never has to decode
/// `0x13` by hand.
fn flag_names(flags: u32) -> String {
    const NAMES: [(u32, &str); 14] = [
        (f::GOSSIP, "GOSSIP"),
        (f::QUESTGIVER, "QUESTGIVER"),
        (f::VENDOR, "VENDOR"),
        (f::FLIGHTMASTER, "FLIGHTMASTER"),
        (f::TRAINER, "TRAINER"),
        (f::SPIRITHEALER, "SPIRITHEALER"),
        (f::SPIRITGUIDE, "SPIRITGUIDE"),
        (f::INNKEEPER, "INNKEEPER"),
        (f::BANKER, "BANKER"),
        (f::PETITIONER, "PETITIONER"),
        (f::TABARDDESIGNER, "TABARDDESIGNER"),
        (f::BATTLEMASTER, "BATTLEMASTER"),
        (f::AUCTIONEER, "AUCTIONEER"),
        (f::STABLEMASTER, "STABLEMASTER"),
    ];
    let set: Vec<&str> = NAMES
        .iter()
        .filter(|(bit, _)| flags & bit != 0)
        .map(|(_, n)| *n)
        .collect();
    match set.is_empty() {
        true => format!("{flags:#x} (none)"),
        false => format!("{flags:#x} {}", set.join("|")),
    }
}

/// Which window is open on `guid`, if any — the observation every leg is judged on.
fn open_window(
    guid: u64,
    gossip: &GossipState,
    giver: &QuestGiver,
    trainer: &TrainerOpen,
) -> Option<Window> {
    // The gossip frame does not open on the packet: it holds closed until the greeting's
    // `CMSG_NPC_TEXT_QUERY` answers (B292's hold, `ui_gossip`), so the greeting is part of "open".
    if gossip.npc == Some(guid) && gossip.greeting.is_some() {
        return Some(Window::Gossip);
    }
    if giver.npc == Some(guid) && giver.view.is_some() {
        return Some(Window::Quest);
    }
    if trainer.trainer == Some(guid) {
        return Some(Window::Trainer);
    }
    None
}

fn service_probe(
    time: ProbeClock,
    mut probe: ResMut<ServiceProbe>,
    mut gossip: ResMut<GossipState>,
    mut giver: ResMut<QuestGiver>,
    mut trainer: ResMut<TrainerOpen>,
    // `[0xb4e2d0]`'s mirror — what the re-click gate reads (decision 1905). The probe asserts it
    // live because the gate's risk is never the `==`, it is whether this really is armed by a
    // window and only by a window.
    interact: Res<InteractNpc>,
    // The vendor fork's leg (decision 1914): the window the empty-cursor leg must open, the VM the
    // held-cursor leg picks an item up in, and the two the app needs to turn a (bag, slot) into
    // the guid the wire addresses.
    mut merchant: ResMut<MerchantOpen>,
    script: Option<NonSendMut<UiScript>>,
    objects: crate::net::Objects,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    self_player: Query<(), With<SelfPlayer>>,
    player: Res<Player>,
    units: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<NetCommands>,
    mut exit: MessageWriter<AppExit>,
) {
    if self_player.is_empty() {
        return; // not in-world yet
    }
    let now = time.elapsed_secs_f64();

    match probe.phase {
        Phase::Start => {
            hop(&mut probe, &net, 0, 0, now);
        }
        Phase::Hop { i, hop: h, sent_at } => {
            if now - sent_at < SETTLE_SECS {
                return;
            }
            probe.polls += 1;
            let leg = &LEGS[i];
            let me = player.pos;
            let found = units.iter().find(|(guid, net_e, store, tf)| {
                // The click's own gate: `dist² > SERVICE_RANGE_SQ` is the cursor's `unable`, and
                // the arm never runs. Acting outside it would measure nothing.
                if net_e.kind != EntityKind::Unit
                    || tf.translation.distance_squared(me) > SCAN_RANGE_SQ
                {
                    return false;
                }
                let flags = store.0.unit_npc_flags();
                match leg.ident {
                    Ident::Entry(entry) => store.0.object_entry() == Some(entry),
                    Ident::PureQuestgiverWithOffer => {
                        flags & f::QUESTGIVER != 0
                            && flags & f::GOSSIP == 0
                            && crate::target::cursor_mode::questgiver_has_quest(
                                giver.status(guid.0),
                            )
                    }
                }
            });
            let Some((guid, _, store, _)) = found else {
                if now - sent_at > SCAN_TIMEOUT_SECS {
                    if h + 1 < leg.at.len() {
                        info!(
                            "PROBE_SERVICE: ({}) nothing eligible at spawn {}/{} — trying the next",
                            leg.name,
                            h + 1,
                            leg.at.len()
                        );
                        hop(&mut probe, &net, i, h + 1, now);
                        return;
                    }
                    let polls = probe.polls;
                    let fps = f64::from(polls) / (now - sent_at).max(0.001);
                    warn!(
                        "PROBE_SERVICE: SKIP ({}) — nothing matching streamed inside the \
                         {:.4}yd service reach of any of this leg's {} spawn point(s) in \
                         {SCAN_TIMEOUT_SECS}s each. Environmental. Scanned {polls} frame(s) \
                         ({fps:.1} fps){}",
                        leg.name,
                        SCAN_RANGE_SQ.sqrt(),
                        leg.at.len(),
                        match fps < 5.0 {
                            // The reading is about the OBSERVER, not the world: below a handful of
                            // frames a second this leg never really looked (0713/0777/1355).
                            true =>
                                " — THE OBSERVER WAS STARVED, so this SKIP says nothing about \
                                     the NPC. Re-run on an idle machine (`uptime`; a leg runner's guard \
                                     is load < 3).",
                            false =>
                                " — the machine was keeping up, so the `.go` was refused, the \
                                      tile never streamed, or this leg's NPC genuinely was not \
                                      there.",
                        }
                    );
                    probe.skips += 1;
                    next(&mut probe, &net, i, now);
                }
                return;
            };
            let guid = guid.0;
            let flags = store.0.unit_npc_flags();
            let entry = store.0.object_entry().unwrap_or(0);
            let status = giver.status(guid);

            // The shipped ladder, on the shipped inputs.
            let arm = service_arm(flags, status);
            let note = match flags == leg.db_flags {
                true => String::new(),
                false => format!(
                    " [note: the world DB read {} when this leg was written]",
                    flag_names(leg.db_flags)
                ),
            };
            let Some(arm) = arm else {
                error!(
                    "PROBE_SERVICE: FAIL ({}) — entry {entry} guid {guid:#x} flags {} \
                     status={status:?}: the ladder matched NO bit, so the click sends nothing. \
                     Wanted {:?}.{note}",
                    leg.name,
                    flag_names(flags),
                    leg.want_arm
                );
                probe.fails += 1;
                next(&mut probe, &net, i, now);
                return;
            };
            if arm != leg.want_arm {
                error!(
                    "PROBE_SERVICE: FAIL ({}) — entry {entry} guid {guid:#x} flags {} \
                     status={status:?}: the ladder took {arm:?}, wanted {:?}. First-match-wins \
                     low→high over UNIT_NPC_FLAGS is the reference's own order \
                     (`0x5f0289`…`0x5f05bc`); a disagreement here is the dispatch, not the \
                     server.{note}",
                    leg.name,
                    flag_names(flags),
                    leg.want_arm
                );
                probe.fails += 1;
                next(&mut probe, &net, i, now);
                return;
            }

            // A stale window from the previous leg would answer this one's question. The
            // reference's own close sends no packet, so a local clear is the whole of it.
            gossip.clear();
            giver.clear();
            trainer.clear();

            // **The first click must always get through** (decision 1905). Nothing is open here —
            // this leg has just cleared, and the reference arms `[0xb4e2d0]` from window openers
            // only — so the re-click gate must read disarmed at the moment of the send. If it ever
            // reads armed here, the gate would be eating first clicks and this probe would still
            // pass every other leg, which is why the assert is at the send and not after it.
            if interact.1.is_some() {
                error!(
                    "PROBE_SERVICE: FAIL ({}) — the npc token is armed on {:#x} BEFORE the send; \
                     the re-click gate would eat first clicks{note}",
                    leg.name,
                    interact.1.unwrap_or(0)
                );
                probe.fails += 1;
                next(&mut probe, &net, i, now);
                return;
            }

            // `None` = an empty cursor: no leg here holds an item, and the vendor arm's fork is
            // the merchant probe's business, not this one's (decision 1914).
            let sent = match service_action(arm, guid, false, None) {
                ServiceAction::Send(cmd) => {
                    let named = format!("{cmd:?}");
                    let _ = net.0.send(cmd);
                    named
                }
                // No leg here takes a dialog arm; if one ever does, say so rather than hang.
                other => {
                    let why: &str = match other {
                        ServiceAction::AskBinder => "CONFIRM_BINDER, no packet",
                        ServiceAction::AskSpiritHealer => "CONFIRM_XP_LOSS, no packet",
                        ServiceAction::AcquireSpiritGuide => {
                            "adopts the area spirit healer; the packet leaves from the cache"
                        }
                        ServiceAction::Silent(w) => w,
                        ServiceAction::Send(_) | ServiceAction::SellFromCursor(_) => {
                            unreachable!()
                        }
                    };
                    warn!(
                        "PROBE_SERVICE: SKIP ({}) — arm {arm:?} opens no window from a packet \
                         ({why}); this probe's legs are the four that do.{note}",
                        leg.name
                    );
                    probe.skips += 1;
                    next(&mut probe, &net, i, now);
                    return;
                }
            };
            info!(
                "PROBE_SERVICE: ({}) entry {entry} guid {guid:#x} flags {} status={status:?} \
                 → arm {arm:?} → {sent}{note}",
                leg.name,
                flag_names(flags)
            );
            probe.phase = Phase::Await {
                i,
                since: now,
                guid,
            };
        }
        Phase::Await { i, since, guid } => {
            let leg = &LEGS[i];
            match open_window(guid, &gossip, &giver, &trainer) {
                Some(open) if open == leg.want_window => {
                    info!(
                        "PROBE_SERVICE: PASS ({}) — {:?} window open on {guid:#x}{}",
                        leg.name,
                        open,
                        match open {
                            Window::Gossip => format!(
                                " ({} option(s), {} quest row(s), greeting {:?})",
                                gossip.options.len(),
                                gossip.quests.len(),
                                gossip.greeting.as_deref().unwrap_or("")
                            ),
                            Window::Quest => format!(" ({:?})", giver.view.as_ref().map(discr)),
                            Window::Trainer => format!(
                                " ({} service(s), type {})",
                                trainer.services.len(),
                                trainer.trainer_type
                            ),
                        }
                    );
                    probe.passes += 1;
                    probe.phase = Phase::Gate {
                        i,
                        since: now,
                        guid,
                    };
                }
                Some(open) => {
                    error!(
                        "PROBE_SERVICE: FAIL ({}) — {open:?} window opened on {guid:#x}, wanted \
                         {:?}",
                        leg.name, leg.want_window
                    );
                    probe.fails += 1;
                    next(&mut probe, &net, i, now);
                }
                None if now - since > WINDOW_TIMEOUT_SECS => {
                    // Distinguish the one near-miss that is a real defect from a dead wire: the
                    // gossip session latched but its greeting never resolved, so the frame stayed
                    // shut (`GossipState::resolve_greeting`'s "Missing gossip text!" path).
                    let held = gossip.npc == Some(guid) && gossip.greeting.is_none();
                    error!(
                        "PROBE_SERVICE: FAIL ({}) — no {:?} window on {guid:#x} within \
                         {WINDOW_TIMEOUT_SECS}s of the send{}",
                        leg.name,
                        leg.want_window,
                        match held {
                            true => format!(
                                " (the gossip session latched on text_id {} but the greeting never \
                                 resolved — the frame never opens on that path)",
                                gossip.text_id
                            ),
                            false => String::new(),
                        }
                    );
                    probe.fails += 1;
                    next(&mut probe, &net, i, now);
                }
                None => {}
            }
        }
        Phase::Gate { i, since, guid } => {
            let leg = &LEGS[i];
            if interact.1 == Some(guid) {
                info!(
                    "PROBE_SERVICE: PASS ({}) — re-click gate armed on {guid:#x} {:.0} ms after \
                     the window: a second right-click sends nothing (`0x5f0251`)",
                    leg.name,
                    (now - since) * 1000.0
                );
                probe.passes += 1;
                next(&mut probe, &net, i, now);
            } else if now - since > GATE_TIMEOUT_SECS {
                error!(
                    "PROBE_SERVICE: FAIL ({}) — the window has been open on {guid:#x} for \
                     {GATE_TIMEOUT_SECS}s and the npc token still reads {:?}; the re-click gate \
                     cannot fire and every re-click re-sends",
                    leg.name, interact.1
                );
                probe.fails += 1;
                next(&mut probe, &net, i, now);
            }
        }
        // ── The vendor fork (decision 1914) ──────────────────────────────────────────────────
        Phase::VendorHop { sent_at } => {
            if now - sent_at < SETTLE_SECS {
                return;
            }
            let me = player.pos;
            let found = units.iter().find(|(_, net_e, store, tf)| {
                net_e.kind == EntityKind::Unit
                    && store.0.object_entry() == Some(VENDOR_ENTRY)
                    && tf.translation.distance_squared(me) <= SCAN_RANGE_SQ
            });
            let Some((guid, _, store, _)) = found else {
                if now - sent_at > SCAN_TIMEOUT_SECS {
                    warn!(
                        "PROBE_SERVICE: SKIP (vendor fork) — entry {VENDOR_ENTRY} never streamed \
                         inside the service reach. Environmental."
                    );
                    probe.skips += 1;
                    probe.phase = Phase::Done;
                }
                return;
            };
            let guid = guid.0;
            let flags = store.0.unit_npc_flags();
            gossip.clear();
            giver.clear();
            trainer.clear();
            merchant.clear();

            // The EMPTY-cursor leg. The arm is asserted, not assumed: a vendor who had grown a
            // GOSSIP bit would take the gossip arm and this leg would be measuring nothing.
            let arm = service_arm(flags, None);
            if arm != Some(ServiceArm::Vendor) {
                warn!(
                    "PROBE_SERVICE: SKIP (vendor fork) — entry {VENDOR_ENTRY} reads {} and takes \
                     {arm:?}, not the vendor arm. Environmental (a world-DB change).",
                    flag_names(flags)
                );
                probe.skips += 1;
                probe.phase = Phase::Done;
                return;
            }
            match service_action(ServiceArm::Vendor, guid, false, None) {
                ServiceAction::Send(cmd) => {
                    info!(
                        "PROBE_SERVICE: (vendor fork, empty cursor) {guid:#x} flags {} → {cmd:?}",
                        flag_names(flags)
                    );
                    let _ = net.0.send(cmd);
                    probe.phase = Phase::VendorOpen { since: now, guid };
                }
                _ => {
                    error!(
                        "PROBE_SERVICE: FAIL (vendor fork, empty cursor) — an EMPTY cursor took \
                         the sale leg; `0x5df5e0 je` must fall to CMSG_LIST_INVENTORY"
                    );
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                }
            }
        }
        Phase::VendorOpen { since, guid } => {
            if merchant.vendor == Some(guid) {
                info!(
                    "PROBE_SERVICE: PASS (vendor fork, empty cursor) — merchant window open on \
                     {guid:#x}: the zero leg is still the plain list open"
                );
                probe.passes += 1;
                // The window would arm the re-click gate and eat the next click, so close it the
                // way its own close button does (no packet) before the held-cursor leg.
                merchant.clear();
                let _ = net.0.send(ClientCommand::Chat {
                    kind: ChatKind::Say,
                    target: None,
                    text: format!(".additem {SELL_ITEM_ENTRY} 1"),
                });
                probe.phase = Phase::VendorArm { since: now, guid };
            } else if now - since > WINDOW_TIMEOUT_SECS {
                error!(
                    "PROBE_SERVICE: FAIL (vendor fork, empty cursor) — no merchant window on \
                     {guid:#x} within {WINDOW_TIMEOUT_SECS}s"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::VendorArm { since, guid } => {
            let Some(mut script) = script else {
                warn!("PROBE_SERVICE: SKIP (vendor fork, held cursor) — no UI VM in this build");
                probe.skips += 1;
                probe.phase = Phase::Done;
                return;
            };
            let Some((bag, slot)) = find_item_slot(&script) else {
                if now - since > BAG_TIMEOUT_SECS {
                    warn!(
                        "PROBE_SERVICE: SKIP (vendor fork, held cursor) — item {SELL_ITEM_ENTRY} \
                         never reached a bag within {BAG_TIMEOUT_SECS}s of `.additem` \
                         (a full bag, or the command was refused). Environmental."
                    );
                    probe.skips += 1;
                    probe.phase = Phase::Done;
                }
                return;
            };
            // Pick it up exactly as a player does — through the binding, not by writing the model.
            if let Err(e) = script.run(&format!("PickupContainerItem({bag}, {slot})")) {
                error!("PROBE_SERVICE: SKIP (vendor fork, held cursor) — PickupContainerItem: {e}");
                probe.skips += 1;
                probe.phase = Phase::Done;
                return;
            }
            let Some(held) = script.cursor_item() else {
                error!(
                    "PROBE_SERVICE: FAIL (vendor fork, held cursor) — PickupContainerItem({bag}, \
                     {slot}) left nothing on the cursor, so the fork has no input"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
                return;
            };
            let slot0 = u8::try_from(held.slot.saturating_sub(1)).unwrap_or(0);
            let Some(item_guid) = self_store
                .single()
                .ok()
                .and_then(|s| crate::ui_items::slot_guid(&s.0, held.bag, slot0, &objects))
            else {
                error!(
                    "PROBE_SERVICE: FAIL (vendor fork, held cursor) — bag {} slot {} is on the \
                     cursor but resolves to no item guid, so the arm cannot address the packet",
                    held.bag, held.slot
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
                return;
            };
            match service_action(ServiceArm::Vendor, guid, false, Some(item_guid)) {
                ServiceAction::SellFromCursor(cmd) => {
                    info!(
                        "PROBE_SERVICE: (vendor fork, held cursor) bag {bag} slot {slot} = item \
                         {item_guid:#x} → {cmd:?}"
                    );
                    let _ = net.0.send(cmd);
                    script.take_cursor_item_for_sale();
                    if script.cursor_payload().is_some() {
                        error!(
                            "PROBE_SERVICE: FAIL (vendor fork, held cursor) — the sell clear left \
                             the item on the cursor"
                        );
                        probe.fails += 1;
                    }
                    probe.phase = Phase::VendorSell {
                        since: now,
                        bag,
                        slot,
                    };
                }
                other => {
                    let what = match other {
                        ServiceAction::Send(cmd) => format!("{cmd:?}"),
                        _ => "a non-send arm".to_string(),
                    };
                    error!(
                        "PROBE_SERVICE: FAIL (vendor fork, held cursor) — an item is on the cursor \
                         and the vendor arm still answered {what}; `0x5df5e0` must take the sale"
                    );
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                }
            }
        }
        Phase::VendorSell { since, bag, slot } => {
            let Some(script) = script else {
                probe.phase = Phase::Done;
                return;
            };
            // The server's own verdict: the sold stack leaves the bag. Nothing client-side can
            // fake this — it is the inventory update answering the packet.
            let gone = script
                .eval::<bool>(&format!(
                    "return GetContainerItemLink({bag}, {slot}) == nil"
                ))
                .unwrap_or(false);
            if gone {
                info!(
                    "PROBE_SERVICE: PASS (vendor fork, held cursor) — the server took the sale: \
                     bag {bag} slot {slot} is empty {:.0} ms after CMSG_SELL_ITEM",
                    (now - since) * 1000.0
                );
                probe.passes += 1;
                probe.phase = Phase::Done;
            } else if now - since > BAG_TIMEOUT_SECS {
                error!(
                    "PROBE_SERVICE: FAIL (vendor fork, held cursor) — bag {bag} slot {slot} still \
                     holds the item {BAG_TIMEOUT_SECS}s after the sale went out; the server \
                     refused it (wrong vendor guid, wrong item guid, or out of range)"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Done => {
            if probe.exited {
                return;
            }
            probe.exited = true;
            info!(
                "PROBE_SERVICE: DONE pass={} fail={} skip={}",
                probe.passes, probe.fails, probe.skips
            );
            // The probe self-exit pattern (`ProbeExitPlugin::fire_probe_exit`): a polite AppExit
            // plus a hard backstop thread, so a net/winit teardown hang can't leave a zombie
            // client holding the probe account.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_SERVICE: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}

/// The open quest panel's shape, for the PASS line — `Greeting` (the multi-quest list) vs
/// `Detail` (the single quest's own frame) is exactly the distinction a reader wants here.
fn discr(view: &crate::ui_quest::QuestView) -> &'static str {
    use crate::ui_quest::QuestView as V;
    match view {
        V::Greeting(_) => "Greeting",
        V::Detail(_) => "Detail",
        V::Progress(_) => "Progress",
        V::Reward(_) => "Reward",
    }
}

/// Send leg `i`'s `.go` for spawn point `h` and enter [`Phase::Hop`].
fn hop(probe: &mut ServiceProbe, net: &NetCommands, i: usize, h: usize, now: f64) {
    let leg = &LEGS[i];
    let [x, y, z] = leg.at[h];
    info!(
        "PROBE_SERVICE: hopping to {} at ({x}, {y}, {z}) map {} (spawn {}/{})",
        leg.name,
        leg.map,
        h + 1,
        leg.at.len()
    );
    let _ = net.0.send(ClientCommand::Chat {
        kind: ChatKind::Say,
        target: None,
        text: format!(".go xyz {x} {y} {z} {}", leg.map),
    });
    probe.polls = 0;
    probe.phase = Phase::Hop {
        i,
        hop: h,
        sent_at: now,
    };
}

/// Advance past leg `i`, or finish.
fn next(probe: &mut ServiceProbe, net: &NetCommands, i: usize, now: f64) {
    match i + 1 < LEGS.len() {
        true => hop(probe, net, i + 1, 0, now),
        // The ladder legs are done; the vendor fork's own chain runs after them.
        false => {
            let [x, y, z] = VENDOR_AT;
            info!(
                "PROBE_SERVICE: hopping to the pure vendor (entry {VENDOR_ENTRY}) at \
                 ({x}, {y}, {z}) map {VENDOR_MAP}"
            );
            let _ = net.0.send(ClientCommand::Chat {
                kind: ChatKind::Say,
                target: None,
                text: format!(".go xyz {x} {y} {z} {VENDOR_MAP}"),
            });
            probe.phase = Phase::VendorHop { sent_at: now };
        }
    }
}
