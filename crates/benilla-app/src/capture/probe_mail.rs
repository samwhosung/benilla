//! The mail-arc live probe (`WOW_PROBE_MAIL=1`) — decision 0544/0548's end-to-end instrument:
//! GM-mail the probe's OWN character, walk to the Goldshire mailbox, open it on the real wire,
//! and drive the inbox/take/send/delete surface through the live Lua VM exactly as a click would,
//! printing a `PROBE_MAIL:` trace line with a PASS/FAIL/SKIP verdict per step and a final
//! `PROBE_MAIL: DONE pass=<n> fail=<m>` summary. Modeled closely on [`super::probe_taxi`] (same
//! phase-machine shape, same trace style, same self-terminating exit).
//!
//! ## The GM `.send` syntax (verified against the vmangos source, `Chat/Chat.cpp` +
//! `Commands/MiscCommands.cpp`)
//!
//! `sendCommandTable[]` (Chat.cpp ~l.931): `.send mail <name> "subject" "text"` is
//! `SEC_MODERATOR` (1); `.send items <name> "subject" "text" item[:count]…` and
//! `.send money <name> "subject" "text" <copper>` are both `SEC_ADMINISTRATOR` (6)
//! (`AccountTypes`, `shared/Common.h` l.138-145: `SEC_MODERATOR=1`, `SEC_GAMEMASTER=3`,
//! `SEC_ADMINISTRATOR=6`). **Probe accounts are gmlevel 6**, so all three land. They did *not*
//! until 2026-07-26 — 0645 recorded the raise but never applied it, and the accounts sat at 3
//! (0651). While they did, steps (c)/(d) below degraded to SKIP against a permission floor that
//! was real, and that SKIP hid a defect in the probe itself: it re-found the money/item row by
//! predicate each tick, so a *successful* take — which zeroes the money and clears the attachment
//! — made the row stop matching and read as "no row ever appeared". Both steps now remember the
//! id they took from, and a missing row is a FAIL, because at gmlevel 6 there is no longer an
//! innocent reason for one. All three GM
//! sends build their `MailDraft` and call `MailDraft::SendMailTo` DIRECTLY
//! (`HandleSendMailCommand`/`HandleSendItemsCommand`/`HandleSendMoneyCommand`, `MiscCommands.cpp`
//! ~l.1012-1145) — never through `WorldSession::HandleSendMail` (`MailHandler.cpp`), whose
//! `MAIL_ERR_CANNOT_SEND_TO_SELF` gate (l.204) lives only in the CMSG-driven path. So a GM
//! `.send mail <self>` bypasses the self-check entirely and lands in the sender's own mailbox —
//! confirmed by reading `MailDraft::SendMailTo` (`Mail/Mail.cpp` l.300+) end to end: no self
//! comparison anywhere in it. The GM path's `deliver_delay` also defaults to `0`
//! (`Mail.h` l.254; the command handlers pass none) — unlike the player CMSG path's
//! `MailDeliveryDelay` (decision 0544's 1 h note), so a successful GM send (permission allowing)
//! is in the inbox on the very next `CMSG_GET_MAIL_LIST`, no extra wait needed. Sender identity:
//! `MailSender(MAIL_NORMAL, <the GM's own char guid counter>, MAIL_STATIONERY_GM)` — a real player
//! sender guid, GM stationery (61, `ui_mail.rs`'s `STATIONERY_GM`).
//!
//! ## The run recipe
//!
//! ```text
//! WOW_DATA=<vanilla Data dir> WOW_USER=probe2 WOW_PASS=pprobe2 WOW_CHAR=Probetwo \
//!     WOW_PROBE_MAIL=1 cargo run -q -p benilla
//! ```
//! (the checkout's probe identity — `.probe-identity`, or WOW_USER/WOW_PASS/WOW_CHAR; the `probe`
//! skill). `WOW_CHAR` doubles as the probe's own mail-send target — read
//! once at world-enter, never hardcoded. An outer `timeout` + grep on `PROBE_MAIL:` is the whole
//! harness; the probe self-exits (the [`super::probes::ProbeExitPlugin`] pattern) once DONE.

use bevy::prelude::*;

use benilla_protocol::messages::MailListEntry;
use benilla_protocol::EntityKind;
use benilla_ui::script::UiScript;

use super::probes::ProbeClock;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_mail::MailOpen;

/// The Goldshire mailbox (vmangos `gameobject` guid 2978, entry 142075, map 0) — live-DB verified
/// position.
const MAILBOX_AT: [f32; 3] = [-9455.99, 45.82, 56.44];
/// `GAMEOBJECT_TYPE_MAILBOX` (decision 0544/0548) — the GO strategy type this probe scans for.
const GO_TYPE_MAILBOX: i32 = 19;
/// The mailbox's server-side interaction check is 5 yd (`CheckMailBox`, decision 0544); scan
/// generously wide so a slightly-off `.go` landing still finds it.
const MAILBOX_SCAN_RANGE: f32 = 10.0;
/// The probe's GM-mailed item: Linen Cloth ×5 (the task's fixture entry).
const ITEM_ENTRY: u32 = 2589;
const ITEM_COUNT: u32 = 5;
/// The probe's GM-mailed money, in copper (~1234c, the task's fixture amount).
const MONEY_COPPER: u32 = 1234;

/// `checked` mask bit READ (`0x1`, vmangos `Mail.h`, decision 0544) — redeclared locally (the
/// `ui_mail` copy is private to that module) purely for this trace's printed flags.
const CHECKED_READ: u32 = 0x1;
/// `checked` mask bit COPIED (`0x4`) — the wire's `textCreated`: step (b2) asserts the
/// `CMSG_MAIL_CREATE_TEXT_ITEM` round-trip stamps it.
const CHECKED_COPIED: u32 = 0x4;

const SETTLE_SECS: f64 = 3.0;
const SCAN_TIMEOUT_SECS: f64 = 15.0;
const LIST_TIMEOUT_SECS: f64 = 15.0;
const BODY_TIMEOUT_SECS: f64 = 10.0;
/// How long a GM-sent row is given to show up before the dependent step FAILs — generous, but
/// bounded (never hangs).
const ROW_GRACE_SECS: f64 = 5.0;
const ACTION_TIMEOUT_SECS: f64 = 8.0;

pub(crate) struct ProbeMailPlugin;

impl Plugin for ProbeMailPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MailProbe>()
            .add_systems(Update, mail_probe);
    }
}

/// The probe's phase machine + the identities it discovers along the way (kept resource-level,
/// not per-variant, since several later phases re-resolve the same row by its stable
/// `message_id` — list order can shift after a take/delete re-sync).
#[derive(Resource, Default)]
struct MailProbe {
    phase: Phase,
    /// `WOW_CHAR` — the probe's own name, its GM-mail target and its self-send target. Read once
    /// at [`Phase::Wait`] (the "read it from the same place login does" instruction — `net/io.rs`
    /// reads the identical env var).
    char_name: String,
    /// The plain letter's `message_id`, once found (stable across resyncs; row order isn't).
    letter_id: Option<u32>,
    passes: u32,
    fails: u32,
    /// Latched once [`Phase::Done`] has fired its exit (never re-fire on a later frame).
    exited: bool,
}

/// Every field is `Copy` (f64/bool/i64) — the phase is snapshotted out of the resource each tick
/// ([`mail_probe`]'s `let phase = probe.phase;`) so the match arms are free to mutate `probe`
/// (counters, `letter_id`, `char_name`) without fighting the borrow checker over `probe.phase`
/// itself; an arm that wants to "keep waiting" simply never writes `probe.phase`, leaving the
/// pre-match value in place.
#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Wait,
    /// GM sends + `.go` issued; settling before the world streams the mailbox GO in.
    Settling {
        sent_at: f64,
    },
    /// Mailbox clicked ([`MailOpen::click`], decision 0544/0548 — no packet); waiting for the
    /// first `SMSG_MAIL_LIST_RESULT` (step a).
    WaitList {
        clicked_at: f64,
    },
    /// The list landed — `GetInboxText` the plain letter (mark-read + ask-once body, step b).
    OpenLetter,
    /// Waiting for the letter's `CMSG_ITEM_TEXT_QUERY` reply to land in the body cache.
    WaitBody {
        since: f64,
    },
    /// `TakeInboxTextItem` on the opened letter — the letter button's permanent-copy verb
    /// (`CMSG_MAIL_CREATE_TEXT_ITEM`, step b2): PASS when the re-synced row carries COPIED.
    CopyLetter {
        since: f64,
        sent: bool,
    },
    /// Read the permanent copy (step b3): find the Plain Letter (8383) in the bags via the
    /// container Lua, right-click it through the real `UseContainerItem` route, PASS when the
    /// reader window paints the GM letter's body — then destroy the copy (else bags accumulate
    /// one per run).
    ReadLetter {
        since: f64,
        /// The letter's `(lua bag, 1-based slot)` once found and clicked.
        slot: Option<(i64, i64)>,
    },
    /// `TakeInboxMoney` on the money row (step c).
    TakeMoney {
        since: f64,
        /// The message id we sent the take for, once sent. It is remembered rather than re-found
        /// each tick because **taking is what makes the row stop matching**: a successful
        /// `TakeInboxMoney` zeroes `money` (or drops the row), so a predicate re-search returns
        /// `None` on exactly the ticks that prove success. That bug reported every successful take
        /// as "no money row appeared" and hid behind the old gmlevel-3 floor, where the row really
        /// never arrived (0651).
        taken: Option<u32>,
    },
    /// `TakeInboxItem` on the item row (step d). Same remembered-id reason as [`Self::TakeMoney`].
    TakeItem {
        since: f64,
        taken: Option<u32>,
    },
    /// `SendMail` to the probe's own name — expects `CANNOT_SEND_TO_SELF` (step e).
    SendSelf {
        since: f64,
        sent: bool,
        baseline: i64,
    },
    /// `SendMail` to a name that doesn't exist — expects `RECIPIENT_NOT_FOUND` (step f).
    SendBad {
        since: f64,
        sent: bool,
        baseline: i64,
    },
    /// `DeleteInboxItem` on the now-read plain letter (step g).
    DeleteLetter {
        since: f64,
        sent: bool,
    },
    Done,
}

/// Find the first row matching `pred`, by stable `message_id` (never a position — resyncs shift
/// rows around).
fn find_by(mail: &MailOpen, pred: impl Fn(&MailListEntry) -> bool) -> Option<u32> {
    mail.mails.iter().find(|e| pred(e)).map(|e| e.message_id)
}

/// The 1-based display index a `message_id` currently sits at, or `None` if it's gone (taken/
/// deleted/expired-and-purged).
fn index_of(mail: &MailOpen, mail_id: u32) -> Option<u32> {
    mail.mails
        .iter()
        .position(|e| e.message_id == mail_id)
        .map(|i| i as u32 + 1)
}

fn entry_of(mail: &MailOpen, mail_id: u32) -> Option<&MailListEntry> {
    mail.mails.iter().find(|e| e.message_id == mail_id)
}

/// Read the Lua-side `ProbeMailEvents` log length (the `UI_ERROR_MESSAGE` hook) — `0` on any eval
/// hiccup (treated as "nothing observed yet", never a panic).
fn events_len(script: &UiScript) -> i64 {
    script
        .eval::<i64>("return table.getn(ProbeMailEvents or {})")
        .unwrap_or(0)
}

/// Read the newest `ProbeMailEvents` entry (the just-fired `UI_ERROR_MESSAGE` text).
fn last_event(script: &UiScript) -> String {
    script
        .eval::<String>("return ProbeMailEvents[table.getn(ProbeMailEvents)] or \"\"")
        .unwrap_or_default()
}

fn mail_probe(
    time: ProbeClock,
    mut probe: ResMut<MailProbe>,
    mut mail: ResMut<MailOpen>,
    script: Option<NonSendMut<UiScript>>,
    self_player: Query<(), With<SelfPlayer>>,
    player: Res<Player>,
    objects: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<NetCommands>,
    mut exit: MessageWriter<AppExit>,
) {
    if self_player.is_empty() {
        return; // not in-world yet
    }
    let Some(script) = script else {
        return; // no UI VM this build (headless net-only) — nothing this probe can drive
    };
    let now = time.elapsed_secs_f64();
    // A cheap `Copy` snapshot (see [`Phase`]'s doc) — frees every arm below to mutate `probe`
    // freely and to leave `probe.phase` untouched when it just wants to keep waiting.
    let phase = probe.phase;

    match phase {
        Phase::Wait => {
            probe.char_name = std::env::var("WOW_CHAR").unwrap_or_default();
            if probe.char_name.is_empty() {
                error!("PROBE_MAIL: FAIL — WOW_CHAR is unset; nothing to GM-mail or self-send to");
                probe.fails += 1;
                probe.phase = Phase::Done;
                return;
            }
            // The UI_ERROR_MESSAGE hook (steps e/f's observation channel) — a hidden frame that
            // logs `arg1` into a Lua table this probe polls, the same "reach out of the VM"
            // pattern as `ProbeLuaPlugin`'s `ProbeLog`, but as a readable log instead of a
            // one-shot info! line (we need to distinguish two different error texts in order).
            if let Err(e) = script.run(
                r#"
                if not ProbeMailHooked then
                    ProbeMailHooked = true
                    ProbeMailEvents = {}
                    local f = CreateFrame("Frame")
                    f:RegisterEvent("UI_ERROR_MESSAGE")
                    f:SetScript("OnEvent", function()
                        table.insert(ProbeMailEvents, arg1 or "")
                    end)
                end
                "#,
            ) {
                error!("PROBE_MAIL: installing the UI_ERROR_MESSAGE hook: {e}");
            }
            let name = &probe.char_name;
            info!("PROBE_MAIL: GM-mailing {name} (plain letter, money, item) then heading to the Goldshire mailbox");
            // Three GM sends (verified syntax above) + the teleport, one chat burst (the
            // `ProbeChatPlugin`/`probe_taxi` idiom: GM dot-commands ride as plain Say lines).
            let [x, y, z] = MAILBOX_AT;
            for text in [
                format!(".send mail {name} \"probe letter\" \"hello\""),
                format!(".send money {name} \"probe money\" \"here is money\" {MONEY_COPPER}"),
                format!(".send items {name} \"probe items\" \"here are items\" {ITEM_ENTRY}:{ITEM_COUNT}"),
                format!(".go xyz {x} {y} {z} 0"),
            ] {
                let _ = net.0.send(ClientCommand::Chat {
                    kind: ChatKind::Say,
                    target: None,
                    text,
                });
            }
            probe.phase = Phase::Settling { sent_at: now };
        }
        Phase::Settling { sent_at } => {
            if now - sent_at < SETTLE_SECS {
                return;
            }
            let me = player.pos;
            let mailbox = objects.iter().find(|(_, net_e, store, tf)| {
                net_e.kind == EntityKind::GameObject
                    && store.0.gameobject_type_id() == GO_TYPE_MAILBOX
                    && tf.translation.distance(me) < MAILBOX_SCAN_RANGE
            });
            if let Some((guid, ..)) = mailbox {
                info!("PROBE_MAIL: mailbox {:#x} in range — clicking (local open, no packet, decision 0544/0548)", guid.0);
                mail.click(guid.0);
                probe.phase = Phase::WaitList { clicked_at: now };
            } else if now - sent_at > SCAN_TIMEOUT_SECS {
                error!("PROBE_MAIL: FAIL (a) — no type-19 GameObject streamed in within {SCAN_TIMEOUT_SECS}s of the mailbox spawn");
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::WaitList { clicked_at } => {
            if !mail.mails.is_empty() {
                info!(
                    "PROBE_MAIL: (a) inbox list landed — {} row(s) (3 GM sends attempted; \
                     probe accounts are gmlevel 6, so all three — `.send mail`(1), \
                     `.send money`(6), `.send items`(6) — are expected to have landed)",
                    mail.mails.len()
                );
                for e in &mail.mails {
                    let item = e
                        .item
                        .as_ref()
                        .map(|a| format!("{}x{}", a.entry, a.count))
                        .unwrap_or_else(|| "none".into());
                    info!(
                        "PROBE_MAIL: row {}: sender={} subject={:?} money={} cod={} item={} \
                         checked={:#x} expire_days={:.2}",
                        e.message_id,
                        e.sender_guid
                            .map(|g| format!("{g:#x}"))
                            .unwrap_or_else(|| "none".into()),
                        e.subject,
                        e.money,
                        e.cod,
                        item,
                        e.checked,
                        e.expire_days
                    );
                }
                probe.passes += 1;
                probe.phase = Phase::OpenLetter;
            } else if now - clicked_at > LIST_TIMEOUT_SECS {
                error!("PROBE_MAIL: FAIL (a) — no SMSG_MAIL_LIST_RESULT within {LIST_TIMEOUT_SECS}s of the click");
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::OpenLetter => {
            let Some(letter_id) = find_by(&mail, |e| e.item.is_none() && e.money == 0) else {
                error!("PROBE_MAIL: FAIL (b) — no plain-letter row found (the `.send mail` GM send should always land one)");
                probe.fails += 1;
                probe.phase = Phase::Done;
                return;
            };
            probe.letter_id = Some(letter_id);
            let Some(idx) = index_of(&mail, letter_id) else {
                return; // shouldn't happen the same frame we just found it — re-poll
            };
            // CheckInbox() called twice, idempotently (`0x4aeab0`'s 60s client-side throttle,
            // decision 0548 §2/0544) — proves a rapid re-call is a no-op, not a packet storm.
            if let Err(e) = script.run(&format!("CheckInbox() CheckInbox() GetInboxText({idx})")) {
                error!("PROBE_MAIL: FAIL (b) — GetInboxText({idx}) errored: {e}");
                probe.fails += 1;
                probe.phase = Phase::Done;
                return;
            }
            info!("PROBE_MAIL: (b) opened letter row {idx} (message {letter_id}) — mark-read + ask-once body queued");
            probe.phase = Phase::WaitBody { since: now };
        }
        Phase::WaitBody { since } => {
            let Some(letter_id) = probe.letter_id else {
                probe.phase = Phase::Done;
                return;
            };
            let landed = entry_of(&mail, letter_id).and_then(|e| {
                (e.item_text_id != 0)
                    .then(|| mail.bodies.get(e.item_text_id).cloned())
                    .flatten()
                    .map(|body| (body, e.checked & CHECKED_READ != 0))
            });
            if let Some((body, was_read)) = landed {
                info!("PROBE_MAIL: (b) body landed: {body:?} (was_read={was_read})");
                probe.passes += 1;
                probe.phase = Phase::CopyLetter {
                    since: now,
                    sent: false,
                };
            } else if now - since > BODY_TIMEOUT_SECS {
                error!(
                    "PROBE_MAIL: FAIL (b) — letter body never landed within {BODY_TIMEOUT_SECS}s"
                );
                probe.fails += 1;
                probe.phase = Phase::CopyLetter {
                    since: now,
                    sent: false,
                };
            }
        }
        Phase::CopyLetter { since, sent } => {
            let Some(letter_id) = probe.letter_id else {
                probe.phase = Phase::ReadLetter {
                    since: now,
                    slot: None,
                };
                return;
            };
            // Same tuple-match rationale as TakeMoney below.
            match (
                entry_of(&mail, letter_id).map(|e| e.checked & CHECKED_COPIED != 0),
                sent,
            ) {
                (Some(true), false) => {
                    warn!("PROBE_MAIL: SKIP (b2) — letter {letter_id} already COPIED (leftover of a prior run; the UI hides the letter button for it)");
                    probe.phase = Phase::ReadLetter {
                        since: now,
                        slot: None,
                    };
                }
                (Some(false), false) => {
                    let Some(idx) = index_of(&mail, letter_id) else {
                        return;
                    };
                    if let Err(e) = script.run(&format!("TakeInboxTextItem({idx})")) {
                        error!("PROBE_MAIL: FAIL (b2) — TakeInboxTextItem({idx}) errored: {e}");
                        probe.fails += 1;
                        probe.phase = Phase::ReadLetter {
                            since: now,
                            slot: None,
                        };
                        return;
                    }
                    info!("PROBE_MAIL: (b2) TakeInboxTextItem({idx}) sent (CMSG_MAIL_CREATE_TEXT_ITEM, message {letter_id})");
                    probe.phase = Phase::CopyLetter {
                        since: now,
                        sent: true,
                    };
                }
                (Some(true), true) => {
                    info!("PROBE_MAIL: PASS (b2) — letter {letter_id} re-synced COPIED (textCreated): the permanent copy landed");
                    probe.passes += 1;
                    probe.phase = Phase::ReadLetter {
                        since: now,
                        slot: None,
                    };
                }
                (Some(false), true) => {
                    if now - since > ACTION_TIMEOUT_SECS {
                        error!("PROBE_MAIL: FAIL (b2) — letter {letter_id} never re-synced COPIED within {ACTION_TIMEOUT_SECS}s");
                        probe.fails += 1;
                        probe.phase = Phase::ReadLetter {
                            since: now,
                            slot: None,
                        };
                    }
                }
                (None, _) => {
                    error!("PROBE_MAIL: FAIL (b2) — letter {letter_id} vanished from the list");
                    probe.fails += 1;
                    probe.phase = Phase::ReadLetter {
                        since: now,
                        slot: None,
                    };
                }
            }
        }
        Phase::ReadLetter { since, slot } => {
            match slot {
                None => {
                    // Find the Plain Letter (8383) in the bags via the container Lua the bag UI
                    // itself uses; encoded bag*100+slot (-1 = not there yet).
                    let found = script
                        .eval::<i64>(
                            "for bag=0,4 do local n=GetContainerNumSlots(bag) or 0 \
                             for slot=1,n do local link=GetContainerItemLink(bag,slot) \
                             if link and string.find(link,'item:8383',1,true) then return bag*100+slot end end end \
                             return -1",
                        )
                        .unwrap_or(-1);
                    if found >= 0 {
                        let (bag, lslot) = (found / 100, found % 100);
                        if let Err(e) = script.run(&format!("UseContainerItem({bag}, {lslot})")) {
                            error!("PROBE_MAIL: FAIL (b3) — UseContainerItem({bag}, {lslot}) errored: {e}");
                            probe.fails += 1;
                            probe.phase = Phase::TakeMoney {
                                since: now,
                                taken: None,
                            };
                            return;
                        }
                        info!("PROBE_MAIL: (b3) Plain Letter at bag {bag} slot {lslot} — right-clicked (the read route, no CMSG_USE_ITEM)");
                        probe.phase = Phase::ReadLetter {
                            since: now,
                            slot: Some((bag, lslot)),
                        };
                    } else if now - since > ACTION_TIMEOUT_SECS {
                        error!("PROBE_MAIL: FAIL (b3) — no Plain Letter (8383) in the bags within {ACTION_TIMEOUT_SECS}s of the copy");
                        probe.fails += 1;
                        probe.phase = Phase::TakeMoney {
                            since: now,
                            taken: None,
                        };
                    }
                }
                Some((bag, lslot)) => {
                    let painted = script
                        .eval::<bool>(
                            "return ItemTextFrame:IsShown() \
                             and string.find(ItemTextPageText:GetText() or '', 'hello', 1, true) ~= nil",
                        )
                        .unwrap_or(false);
                    if painted {
                        info!("PROBE_MAIL: PASS (b3) — the reader painted the letter body (title/creator/text via ITEM_TEXT_BEGIN→READY)");
                        probe.passes += 1;
                        // Cleanup: close the reader, destroy the copy (else bags gain one per run).
                        crate::ui_script::run_or_warn(&script, "ItemTextCloseButton:Click()");
                        crate::ui_script::run_or_warn(
                            &script,
                            &format!("PickupContainerItem({bag}, {lslot}) DeleteCursorItem()"),
                        );
                        probe.phase = Phase::TakeMoney {
                            since: now,
                            taken: None,
                        };
                    } else if now - since > ACTION_TIMEOUT_SECS {
                        error!("PROBE_MAIL: FAIL (b3) — the reader never painted the body within {ACTION_TIMEOUT_SECS}s of the click");
                        probe.fails += 1;
                        probe.phase = Phase::TakeMoney {
                            since: now,
                            taken: None,
                        };
                    }
                }
            }
        }
        Phase::TakeMoney { since, taken } => {
            // The id is REMEMBERED across ticks, never re-found: a successful take is exactly what
            // makes the row stop matching `money > 0`, so re-searching would read success as
            // absence (0651). Matched as a pair rather than with guards — guarded `Some(_) if …`
            // arms can't be proven exhaustive by rustc, forcing a dead catch-all.
            let next = Phase::TakeItem {
                since: now,
                taken: None,
            };
            match taken {
                None => match find_by(&mail, |e| e.money > 0) {
                    Some(money_id) => {
                        let Some(idx) = index_of(&mail, money_id) else {
                            return;
                        };
                        if let Err(e) = script.run(&format!("TakeInboxMoney({idx})")) {
                            error!("PROBE_MAIL: FAIL (c) — TakeInboxMoney({idx}) errored: {e}");
                            probe.fails += 1;
                            probe.phase = next;
                            return;
                        }
                        info!("PROBE_MAIL: (c) money row found (message {money_id}) — TakeInboxMoney({idx}) sent");
                        probe.phase = Phase::TakeMoney {
                            since: now,
                            taken: Some(money_id),
                        };
                    }
                    None if now - since > ROW_GRACE_SECS => {
                        error!("PROBE_MAIL: FAIL (c) — no money row within {ROW_GRACE_SECS}s. Probe accounts are gmlevel 6, so `.send money` (SEC_ADMINISTRATOR 6) should have landed one — check the `net: server says` lines for the GM send's own answer");
                        probe.fails += 1;
                        probe.phase = next;
                    }
                    None => {}
                },
                Some(money_id) => {
                    let money_now = entry_of(&mail, money_id).map(|e| e.money);
                    if matches!(money_now, Some(0) | None) {
                        info!(
                            "PROBE_MAIL: PASS (c) — money row {} (row {money_id})",
                            if money_now.is_none() {
                                "gone"
                            } else {
                                "money=0"
                            }
                        );
                        probe.passes += 1;
                        probe.phase = next;
                    } else if now - since > ACTION_TIMEOUT_SECS {
                        error!("PROBE_MAIL: FAIL (c) — money row {money_id} never re-synced to 0 within {ACTION_TIMEOUT_SECS}s");
                        probe.fails += 1;
                        probe.phase = next;
                    }
                }
            }
        }
        Phase::TakeItem { since, taken } => {
            let next = Phase::SendSelf {
                since: now,
                sent: false,
                baseline: events_len(&script),
            };
            match taken {
                None => match find_by(&mail, |e| e.item.is_some()) {
                    Some(item_id) => {
                        let Some(idx) = index_of(&mail, item_id) else {
                            return;
                        };
                        if let Err(e) = script.run(&format!("TakeInboxItem({idx})")) {
                            error!("PROBE_MAIL: FAIL (d) — TakeInboxItem({idx}) errored: {e}");
                            probe.fails += 1;
                            probe.phase = next;
                            return;
                        }
                        info!("PROBE_MAIL: (d) item row found (message {item_id}) — TakeInboxItem({idx}) sent");
                        probe.phase = Phase::TakeItem {
                            since: now,
                            taken: Some(item_id),
                        };
                    }
                    None if now - since > ROW_GRACE_SECS => {
                        error!("PROBE_MAIL: FAIL (d) — no item row within {ROW_GRACE_SECS}s. Probe accounts are gmlevel 6, so `.send items` (SEC_ADMINISTRATOR 6) should have landed one — check the `net: server says` lines for the GM send's own answer");
                        probe.fails += 1;
                        probe.phase = next;
                    }
                    None => {}
                },
                Some(item_id) => {
                    let has_item = entry_of(&mail, item_id).map(|e| e.item.is_some());
                    if matches!(has_item, Some(false) | None) {
                        info!("PROBE_MAIL: PASS (d) — item row {item_id} taken (item attachment cleared)");
                        probe.passes += 1;
                        probe.phase = next;
                    } else if now - since > ACTION_TIMEOUT_SECS {
                        error!("PROBE_MAIL: FAIL (d) — item row {item_id} never re-synced within {ACTION_TIMEOUT_SECS}s");
                        probe.fails += 1;
                        probe.phase = next;
                    }
                }
            }
        }
        Phase::SendSelf {
            since,
            sent,
            baseline,
        } => {
            if !sent {
                let name = probe.char_name.clone();
                if let Err(e) = script.run(&format!(
                    "SendMail('{name}', 'probe self-send', 'this should refuse')"
                )) {
                    error!("PROBE_MAIL: FAIL (e) — SendMail(self) errored: {e}");
                    probe.fails += 1;
                    probe.phase = Phase::SendBad {
                        since: now,
                        sent: false,
                        baseline: events_len(&script),
                    };
                    return;
                }
                info!("PROBE_MAIL: (e) SendMail('{name}', …) sent — expecting CANNOT_SEND_TO_SELF");
                probe.phase = Phase::SendSelf {
                    since: now,
                    sent: true,
                    baseline,
                };
                return;
            }
            let seen = events_len(&script);
            if seen > baseline {
                let text = last_event(&script);
                if text.to_lowercase().contains("yourself") {
                    info!("PROBE_MAIL: PASS (e) — surfaced error: {text:?}");
                    probe.passes += 1;
                } else {
                    error!("PROBE_MAIL: FAIL (e) — unexpected surfaced text: {text:?} (wanted \"yourself\")");
                    probe.fails += 1;
                }
                probe.phase = Phase::SendBad {
                    since: now,
                    sent: false,
                    baseline: events_len(&script),
                };
            } else if now - since > ACTION_TIMEOUT_SECS {
                error!("PROBE_MAIL: FAIL (e) — no UI_ERROR_MESSAGE observed within {ACTION_TIMEOUT_SECS}s");
                probe.fails += 1;
                probe.phase = Phase::SendBad {
                    since: now,
                    sent: false,
                    baseline: events_len(&script),
                };
            }
        }
        Phase::SendBad {
            since,
            sent,
            baseline,
        } => {
            if !sent {
                if let Err(e) =
                    script.run("SendMail('Zzznonexistent', 'probe bad-send', 'this should refuse')")
                {
                    error!("PROBE_MAIL: FAIL (f) — SendMail(nonexistent) errored: {e}");
                    probe.fails += 1;
                    probe.phase = Phase::DeleteLetter {
                        since: now,
                        sent: false,
                    };
                    return;
                }
                info!("PROBE_MAIL: (f) SendMail('Zzznonexistent', …) sent — expecting RECIPIENT_NOT_FOUND");
                probe.phase = Phase::SendBad {
                    since: now,
                    sent: true,
                    baseline,
                };
                return;
            }
            let seen = events_len(&script);
            if seen > baseline {
                let text = last_event(&script);
                if text.to_lowercase().contains("recipient") {
                    info!("PROBE_MAIL: PASS (f) — surfaced error: {text:?}");
                    probe.passes += 1;
                } else {
                    error!("PROBE_MAIL: FAIL (f) — unexpected surfaced text: {text:?} (wanted \"recipient\")");
                    probe.fails += 1;
                }
                probe.phase = Phase::DeleteLetter {
                    since: now,
                    sent: false,
                };
            } else if now - since > ACTION_TIMEOUT_SECS {
                error!("PROBE_MAIL: FAIL (f) — no UI_ERROR_MESSAGE observed within {ACTION_TIMEOUT_SECS}s");
                probe.fails += 1;
                probe.phase = Phase::DeleteLetter {
                    since: now,
                    sent: false,
                };
            }
        }
        Phase::DeleteLetter { since, sent } => {
            let Some(letter_id) = probe.letter_id else {
                probe.phase = Phase::Done;
                return;
            };
            if !sent {
                let Some(idx) = index_of(&mail, letter_id) else {
                    error!("PROBE_MAIL: FAIL (g) — the read letter (message {letter_id}) is already gone");
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                    return;
                };
                if let Err(e) = script.run(&format!("DeleteInboxItem({idx})")) {
                    error!("PROBE_MAIL: FAIL (g) — DeleteInboxItem({idx}) errored: {e}");
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                    return;
                }
                info!("PROBE_MAIL: (g) DeleteInboxItem({idx}) sent for message {letter_id}");
                probe.phase = Phase::DeleteLetter {
                    since: now,
                    sent: true,
                };
                return;
            }
            if index_of(&mail, letter_id).is_none() {
                info!("PROBE_MAIL: PASS (g) — DELETED OK (message {letter_id} gone from the list)");
                probe.passes += 1;
                probe.phase = Phase::Done;
            } else if now - since > ACTION_TIMEOUT_SECS {
                error!("PROBE_MAIL: FAIL (g) — message {letter_id} still listed {ACTION_TIMEOUT_SECS}s after delete");
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
                "PROBE_MAIL: DONE pass={} fail={}",
                probe.passes, probe.fails
            );
            // The probe self-exit pattern (`ProbeExitPlugin::fire_probe_exit`): a polite AppExit
            // plus a hard backstop thread, so a net/winit teardown hang can't leave a zombie
            // client holding the probe account.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_MAIL: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
