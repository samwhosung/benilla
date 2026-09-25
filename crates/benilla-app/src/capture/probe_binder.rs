//! The innkeeper-bind live probe (`WOW_PROBE_BINDER=1`) — decision 1331's end-to-end instrument and
//! the evidence that closes **B249** ("setting your Hearthstone shows a wrong icon and doesn't
//! take"): log in, GM-hop to the exact innkeeper in the bug's screenshot, open her gossip menu on
//! the real wire, assert the bind row's icon reads **binder** (half 1), select it, assert the
//! server's `SMSG_BINDER_CONFIRM` arrived and reached the Lua dialog (half 2's question), answer it
//! through the reference's own `ConfirmBinder()`, and assert the hearthstone actually moved (half
//! 2's answer). One `PROBE_BINDER: <step> PASS/FAIL/SKIP <detail>` line per step, then a final
//! `PROBE_BINDER: DONE pass=<n> fail=<m>`. Modeled closely on [`super::probe_bank`] — same phase
//! machine, same trace style, same self-terminating exit ([`super::probes::ProbeExitPlugin`]'s
//! pattern), same live-VM observation idiom (a small Lua hook appending to a probe table, read back
//! with `script.eval`).
//!
//! Unit tests cannot close B249. Half 1's old mapping was self-consistent (it just described a
//! *later* client's icon art), and half 2's packet was never parsed at all — both defects are only
//! visible against a real server sending real bytes, which is what this probe puts in front of
//! them.
//!
//! ## The innkeeper (live-DB verified against the local vmangos, `mangos` DB)
//!
//! Innkeeper Keldamyr — `creature_template.entry = 6736`, spawn `creature.guid = 46343`, **map 1**
//! (Teldrassil, Dolanaar), position `(9802.21, 982.608, 1313.98)`,
//! `creature_template.npc_flags = 135` = `0x87` = GOSSIP|QUESTGIVER|VENDOR|**INNKEEPER**,
//! `gossip_menu_id = 1293`. She is the NPC in the bug's screenshot, which is why the probe hops to
//! her rather than to whichever innkeeper is nearest.
//!
//! **`UNIT_NPC_FLAG_INNKEEPER` is `0x80` (128) on 1.12** — vmangos `Objects/UnitDefines.h:610`,
//! read this session. `GossipDef.h:45`'s comment beside `GOSSIP_OPTION_INNKEEPER` says `(65536)`,
//! which is a *later* client's value for the same flag: exactly the stale-comment trap decision
//! 1331 is about (half 1 was a hand-written icon map that trusted that same header's enum *names*).
//! [`NPC_FLAG_INNKEEPER`] below is the verified `0x80`, and it is only the scan's fallback — the
//! primary identity check is the template entry.
//!
//! ## The bind row (live-DB verified this session)
//!
//! `gossip_menu_option` for menu 1293 carries five rows; the innkeeper's is
//! `option_icon = 5`, `option_id = 8` (`GOSSIP_OPTION_INNKEEPER`), `npc_option_npcflag = 128`,
//! text *"Make this inn my home."* — and **every one of the 21 `option_id = 8` rows in the whole
//! world DB uses `option_icon = 5`** (`SELECT option_icon, COUNT(*) … WHERE option_id = 8 GROUP BY
//! option_icon` → a single row, `5 | 21`). So icon 5 is *the* innkeeper icon, and 5 was precisely
//! the byte the pre-1331 map had no entry for: it fell through to the chat bubble. Step 3 FAILs
//! loudly on `"gossip"` for that reason — that string is the B249 regression itself.
//!
//! Two things a live reading shows that the DB row does not, both verified on the first run:
//!
//! - **The label on the wire is not `option_text`.** vmangos prefers the row's
//!   `option_broadcast_text` (2822 -> *"Make this inn your home."*) over the `option_text` column
//!   (*"Make this inn **my** home."*). Step 4's guard is therefore a lowercase `"home"` substring,
//!   not an equality — the exact wording is the server's to choose.
//! - **A probe login sees five rows here, not the three in the bug's screenshot.** GM mode is the
//!   probe default (the preflight banner says so every run), and vmangos does not
//!   condition-filter a GM's menu — it appends `"(GM mode is ON)"` to the two holiday rows and
//!   sends them anyway. So the bind row sits at list position 2 with wire index 1 on a probe, and
//!   would sit at position 1 on a player. That is exactly why step 3 finds the row by its **wire
//!   icon byte** and step 4 selects by its **wire index**, and why neither ever counts positions.
//!
//! ## What each step can and cannot conclude
//!
//! Step 5 is the load-bearing one. Before 1331 `SMSG_BINDER_CONFIRM` (0x2eb) had no const, no parse
//! arm and no dialog: it fell through `parse.rs`'s tail into `ServerPacket::Other` and the click
//! produced a closing gossip window and nothing else. A PASS there means the packet is parsed, the
//! session state took it, and the `CONFIRM_BINDER` event reached the live VM carrying a real area
//! name.
//!
//! Step 6 answers it the way a player does — `ConfirmBinder()` run in the live VM, so the whole
//! dialog→engine→drain→wire chain (`CMSG_BINDER_ACTIVATE`, 0x1b5) is exercised rather than a
//! synthesized packet. **A rebind to the place you are already bound is invisible unless you make
//! it visible**: vmangos's `EffectBind` (`SpellEffects.cpp`) writes the homebind and sends
//! `SMSG_BINDPOINTUPDATE` *unconditionally*, but our [`HomeBind`] would simply be re-set to the
//! value it already held. So the probe clears `HomeBind` to `None` immediately before answering
//! and waits for it to become `Some` again — that transition is the fresh packet, and it is the
//! only reading that survives a probe character who already hearths in Dolanaar from an earlier
//! run.
//!
//! ## The run recipe
//!
//! ```text
//! WOW_DATA=WoW/Data WOW_USER=probe1 WOW_PASS=pprobe1 WOW_CHAR=Probeone \
//!     WOW_PROBE_BINDER=1 cargo run -q -p benilla
//! ```
//! (the checkout's probe identity — `.probe-identity`, or WOW_USER/WOW_PASS/WOW_CHAR; the `probe`
//! skill). Non-combat, and GM mode is left exactly as found. An
//! outer `timeout` + grep on `PROBE_BINDER:` is the whole harness; the probe self-exits once DONE.
//!
//! Every step SKIPs with a note rather than FAILing for an environmental problem (the NPC never
//! streamed, a GM command refused, no UI VM in this build). A genuine wrong value is a FAIL.

use bevy::prelude::*;

use benilla_protocol::EntityKind;
use benilla_ui::script::UiScript;

use super::probes::ProbeClock;
use crate::area::AreaTableRes;
use crate::net::SelfPlayer;
use crate::net::{ChatKind, ClientCommand, Guid, HomeBind, NetCommands, NetEntity, ObjectStore};
use crate::player::Player;
use crate::ui_binder::BinderState;
use crate::ui_gossip::GossipState;
use crate::ui_session::NpcSession;

/// Innkeeper Keldamyr's spawn (vmangos `creature` guid 46343, entry 6736) — the `.go xyz` target.
const INNKEEPER_AT: [f32; 3] = [9802.21, 982.608, 1313.98];
/// Her map — Teldrassil. `.go xyz` takes the map id as its fourth argument.
const INNKEEPER_MAP: u32 = 1;
/// Her creature template entry — the streamed-unit identity check (module doc).
const INNKEEPER_ENTRY: u32 = 6736;
/// `UNIT_NPC_FLAG_INNKEEPER` — `0x80` on 1.12, NOT the `65536` `GossipDef.h`'s comment names
/// (module doc). The scan's fallback when the entry read is unavailable.
const NPC_FLAG_INNKEEPER: u32 = 0x80;
/// The wire `GOSSIP_ICON` byte every `GOSSIP_OPTION_INNKEEPER` row in the world DB sends
/// (21/21, verified this session) — decision 1331's table indexes it to `"binder"`.
const ICON_INNKEEPER: u8 = 5;
/// The type string [`crate::ui_gossip`]'s table must produce for [`ICON_INNKEEPER`].
const ICON_TYPE_BINDER: &str = "binder";
/// What the pre-1331 map produced instead — the chat bubble. Seeing it back is B249, not a flake.
const ICON_TYPE_REGRESSION: &str = "gossip";
/// The substring the bind row's label must carry before the probe is willing to select it — a
/// guard against binding whatever else the menu happens to offer if the DB row ever moves.
const BIND_LABEL_HINT: &str = "home";
/// Scan radius around the `.go` landing, generously wide so a slightly-off hop still finds her
/// (the bank/mail probes' idiom).
const SCAN_RANGE: f32 = 12.0;

const SETTLE_SECS: f64 = 3.0;
/// The waits are deliberately generous, and the reason is measured rather than guessed: the `.go`
/// lands on a **different map**, so the whole leg after the hop runs inside Teldrassil's terrain
/// load — the first live run logged five consecutive `frame hitch: ~1050 ms` lines and a 5.0 s
/// loading screen across steps 4→5, i.e. the probe got roughly one frame per second to poll in.
/// The packets were prompt; the *observer* was starved. A timeout tight enough to trip on that
/// would report a FAIL about the wire, which is the one thing an instrument must never do.
const SCAN_TIMEOUT_SECS: f64 = 20.0;
const MENU_TIMEOUT_SECS: f64 = 20.0;
const CONFIRM_TIMEOUT_SECS: f64 = 20.0;
const BIND_TIMEOUT_SECS: f64 = 20.0;
const LINE_TIMEOUT_SECS: f64 = 20.0;

/// `ERR_DEATHBIND_SUCCESS_S` (GlobalStrings.lua:1543, verbatim) — what step 7 expects to read back
/// off CHAT_MSG_SYSTEM, composed here rather than imported so the probe asserts against the
/// reference's own string and not against whatever `ui_binder` happens to hold.
fn bound_line(area_name: &str) -> String {
    "%s is now your home.".replace("%s", area_name)
}

pub(crate) struct ProbeBinderPlugin;

impl Plugin for ProbeBinderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BinderProbe>()
            .add_systems(Update, binder_probe);
    }
}

/// The probe's phase machine plus the identities discovered along the way (the bank probe's shape:
/// a `Copy` phase snapshotted out of the resource each tick, so an arm can mutate `probe` freely).
#[derive(Resource, Default)]
struct BinderProbe {
    phase: Phase,
    /// The innkeeper's guid, once streamed in.
    innkeeper: Option<u64>,
    /// The bind row's **wire** `index` — the value the packet carried and the value
    /// `CMSG_GOSSIP_SELECT_OPTION` must echo back. vmangos numbers them `data << uint32(iI)` over
    /// the rows it actually sends (`GossipDef.cpp:188`), so it is the row's **0-based** position in
    /// *this* menu — neither the DB's `gossip_menu_option.id` nor the Lua menu's 1-based position,
    /// and the probe assumes no relation between them for the same reason the real drain doesn't.
    bind_index: Option<u32>,
    /// The area id [`HomeBind`] held before step 6 cleared it — reported in the verdict so a
    /// rebind-to-the-same-place reads as the deliberate no-visible-change case it is.
    baseline_area: Option<u32>,
    /// The area name the bind resolved to — step 7 composes its expected line from it.
    bound_name: String,
    passes: u32,
    fails: u32,
    /// Latched once [`Phase::Done`] has fired its exit (never re-fire on a later frame).
    exited: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Wait,
    /// `.go` issued; settling before the world streams the innkeeper in (step 1).
    Settling {
        sent_at: f64,
    },
    /// `GossipHello` sent; waiting for the parsed menu AND its push into the VM (step 2).
    Menu {
        sent_at: f64,
    },
    /// The bind row selected; waiting for `SMSG_BINDER_CONFIRM` to reach both [`BinderState`] and
    /// the `CONFIRM_BINDER` Lua event (step 5).
    Confirm {
        since: f64,
        events_baseline: i64,
    },
    /// `ConfirmBinder()` run in the live VM (after clearing [`HomeBind`]); waiting for a fresh
    /// `SMSG_BINDPOINTUPDATE` and the VM's `GetBindLocation()` to agree with it (step 6).
    Accept {
        since: f64,
        sent: bool,
    },
    /// Step 7 — the bind landed; waiting for `SMSG_PLAYERBOUND`'s
    /// `ERR_DEATHBIND_SUCCESS_S` line to reach the VM as CHAT_MSG_SYSTEM.
    Line {
        since: f64,
    },
    Done,
}

/// Read the Lua-side `ProbeBinderEvents` log length (the `CONFIRM_BINDER` hook) — `0` on any eval
/// hiccup, treated as "nothing observed yet", never a panic (the bank probe's idiom).
fn events_len(script: &UiScript) -> i64 {
    script
        .eval::<i64>("return table.getn(ProbeBinderEvents or {})")
        .unwrap_or(0)
}

/// The newest `ProbeBinderEvents` entry — `CONFIRM_BINDER`'s `arg1`, the area name that fills the
/// dialog's `"Do you want to make %s your new home?"`.
fn last_event(script: &UiScript) -> String {
    script
        .eval::<String>("return ProbeBinderEvents[table.getn(ProbeBinderEvents)] or \"\"")
        .unwrap_or_default()
}

/// The `CHAT_MSG_SYSTEM` lines seen since the hook went in, newest last — step 6's second
/// observation. `SMSG_PLAYERBOUND` prints `ERR_DEATHBIND_SUCCESS_S` here (`0x5e3d3f` →
/// `DisplayError(0x138)`, chat type 238 = CHAT_MSG_SYSTEM).
fn system_lines(script: &UiScript) -> Vec<String> {
    script
        .eval::<Vec<String>>("return ProbeBinderSystemLines or {}")
        .unwrap_or_default()
}

/// How many values the live `GetGossipOptions()` returns — flat `(label, type)` pairs, so twice the
/// row count. The probe waits on this rather than assuming the feed already ran this frame.
fn vm_gossip_values(script: &UiScript) -> i64 {
    script
        .eval::<i64>("local t = { GetGossipOptions() } return table.getn(t)")
        .unwrap_or(0)
}

/// The icon **type string** the app mapped for the 1-based menu row `pos` — read exactly where the
/// FrameXML reads it, out of the pushed [`benilla_ui::script::GossipMenu`] snapshot through the
/// Era `GetGossipOptions()` vararg. (`ui_gossip::gossip_icon_type` is private to its module; the
/// snapshot is the same value the real menu draws from, which is the stronger assert anyway.)
fn vm_icon_type(script: &UiScript, pos: usize) -> String {
    script
        .eval::<String>(&format!(
            "local t = {{ GetGossipOptions() }} return t[{}] or \"\"",
            pos * 2
        ))
        .unwrap_or_default()
}

/// The texture path `BENILLA_GOSSIP_ICONS.binder` resolves to in the live VM — `""` if the table or
/// the key is missing, which would mean the row draws the fallback bubble whatever the app mapped.
fn vm_binder_texture(script: &UiScript) -> String {
    script
        .eval::<String>("return (BENILLA_GOSSIP_ICONS and BENILLA_GOSSIP_ICONS.binder) or \"\"")
        .unwrap_or_default()
}

/// The live VM's `GetBindLocation()` — the hearthstone's own answer for where you are bound.
fn vm_bind_location(script: &UiScript) -> String {
    script
        .eval::<String>("return GetBindLocation()")
        .unwrap_or_default()
}

fn binder_probe(
    time: ProbeClock,
    mut probe: ResMut<BinderProbe>,
    gossip: Res<GossipState>,
    binder: Res<BinderState>,
    mut home: ResMut<HomeBind>,
    areas: Option<Res<AreaTableRes>>,
    script: Option<NonSendMut<UiScript>>,
    self_player: Query<(), With<SelfPlayer>>,
    player: Res<Player>,
    units: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
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
    let phase = probe.phase;

    match phase {
        Phase::Wait => {
            // The CONFIRM_BINDER hook (step 5's observation channel — the bank probe's exact
            // pattern), installed up front so it is live long before the question can arrive.
            if let Err(e) = script.run(
                r#"
                if not ProbeBinderHooked then
                    ProbeBinderHooked = true
                    ProbeBinderEvents = {}
                    ProbeBinderSystemLines = {}
                    local f = CreateFrame("Frame")
                    f:RegisterEvent("CONFIRM_BINDER")
                    f:RegisterEvent("CHAT_MSG_SYSTEM")
                    f:SetScript("OnEvent", function()
                        if event == "CONFIRM_BINDER" then
                            table.insert(ProbeBinderEvents, arg1 or "")
                        else
                            table.insert(ProbeBinderSystemLines, arg1 or "")
                        end
                    end)
                end
                "#,
            ) {
                error!("PROBE_BINDER: installing the CONFIRM_BINDER hook: {e}");
            }
            let [x, y, z] = INNKEEPER_AT;
            info!(
                "PROBE_BINDER: hopping to Innkeeper Keldamyr (entry {INNKEEPER_ENTRY}) at \
                 ({x}, {y}, {z}) map {INNKEEPER_MAP}"
            );
            let _ = net.0.send(ClientCommand::Chat {
                kind: ChatKind::Say,
                target: None,
                text: format!(".go xyz {x} {y} {z} {INNKEEPER_MAP}"),
            });
            probe.phase = Phase::Settling { sent_at: now };
        }
        Phase::Settling { sent_at } => {
            if now - sent_at < SETTLE_SECS {
                return;
            }
            let me = player.pos;
            let found = units.iter().find(|(_, net_e, store, tf)| {
                net_e.kind == EntityKind::Unit
                    && (store.0.object_entry() == Some(INNKEEPER_ENTRY)
                        || store.0.unit_npc_flags() & NPC_FLAG_INNKEEPER != 0)
                    && tf.translation.distance(me) < SCAN_RANGE
            });
            if let Some((guid, ..)) = found {
                info!(
                    "PROBE_BINDER: PASS (1 hop) — innkeeper {:#x} streamed within {SCAN_RANGE}yd \
                     of the landing",
                    guid.0
                );
                probe.passes += 1;
                probe.innkeeper = Some(guid.0);
                let _ = net.0.send(ClientCommand::GossipHello { guid: guid.0 });
                info!("PROBE_BINDER: (2 menu) GossipHello({:#x}) sent", guid.0);
                probe.phase = Phase::Menu { sent_at: now };
            } else if now - sent_at > SCAN_TIMEOUT_SECS {
                warn!(
                    "PROBE_BINDER: SKIP (1 hop) — no entry {INNKEEPER_ENTRY}/innkeeper-flagged \
                     unit streamed in within {SCAN_TIMEOUT_SECS}s of the hop (the `.go` may have \
                     been refused, or Teldrassil never streamed) — environmental, not a defect"
                );
                probe.phase = Phase::Done;
            }
        }
        Phase::Menu { sent_at } => {
            let Some(innkeeper) = probe.innkeeper else {
                probe.phase = Phase::Done;
                return;
            };
            let open = gossip.npc == Some(innkeeper) && !gossip.options.is_empty();
            // Wait for the feed's push as well as the parse: step 3 reads the icon type back out
            // of the VM, so the snapshot must already be there.
            let pushed = vm_gossip_values(&script) as usize == gossip.options.len() * 2;
            if open && pushed {
                info!(
                    "PROBE_BINDER: PASS (2 menu) — {} option(s) open on {innkeeper:#x}: {}",
                    gossip.options.len(),
                    gossip
                        .options
                        .iter()
                        .map(|o| format!("[{} icon={} {:?}]", o.index, o.icon, o.message))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                probe.passes += 1;
                assert_icon_and_select(&mut probe, &gossip, &script, &net, innkeeper, now);
            } else if now - sent_at > MENU_TIMEOUT_SECS {
                error!(
                    "PROBE_BINDER: FAIL (2 menu) — no gossip menu for {innkeeper:#x} within \
                     {MENU_TIMEOUT_SECS}s (parsed npc={:?} options={} vm_values={})",
                    gossip.npc,
                    gossip.options.len(),
                    vm_gossip_values(&script)
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Confirm {
            since,
            events_baseline,
        } => {
            let Some(innkeeper) = probe.innkeeper else {
                probe.phase = Phase::Done;
                return;
            };
            let pending = binder.npc() == Some(innkeeper);
            let fired = events_len(&script) > events_baseline;
            let area = last_event(&script);
            if pending && fired && !area.is_empty() {
                info!(
                    "PROBE_BINDER: PASS (5 confirm) — SMSG_BINDER_CONFIRM parked on \
                     {innkeeper:#x} and CONFIRM_BINDER fired with area {area:?} (the dialog reads \
                     \"Do you want to make {area} your new home?\")"
                );
                probe.passes += 1;
                probe.phase = Phase::Accept {
                    since: now,
                    sent: false,
                };
            } else if now - since > CONFIRM_TIMEOUT_SECS {
                error!(
                    "PROBE_BINDER: FAIL (5 confirm) — no answered question within \
                     {CONFIRM_TIMEOUT_SECS}s of the select: BinderState pending={:?} (wanted \
                     {innkeeper:#x}), CONFIRM_BINDER fired={fired}, arg1={area:?}. Before decision \
                     1331 the packet had no parse arm at all and fell through to \
                     ServerPacket::Other — that is what this reading looks like.",
                    binder.npc()
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Accept { since, sent } => {
            if !sent {
                // Clear the bind BEFORE answering: a rebind to the area we already hearth in
                // re-sends SMSG_BINDPOINTUPDATE with an identical payload, so only the
                // None → Some transition proves a fresh packet landed (module doc).
                probe.baseline_area = home.0;
                home.0 = None;
                if let Err(e) = script.run("ConfirmBinder()") {
                    warn!(
                        "PROBE_BINDER: SKIP (6 bind) — ConfirmBinder() would not run in the live \
                         VM: {e} (environmental, not a wire failure)"
                    );
                    probe.phase = Phase::Done;
                    return;
                }
                info!(
                    "PROBE_BINDER: (6 bind) ConfirmBinder() run in the live VM — HomeBind cleared \
                     from {:?}, waiting for a fresh SMSG_BINDPOINTUPDATE",
                    probe.baseline_area
                );
                probe.phase = Phase::Accept {
                    since: now,
                    sent: true,
                };
                return;
            }
            let name = home
                .0
                .and_then(|id| areas.as_deref()?.0.name(id))
                .unwrap_or_default()
                .to_string();
            let vm = vm_bind_location(&script);
            if !name.is_empty() && vm == name {
                info!(
                    "PROBE_BINDER: PASS (6 bind) — HomeBind repopulated to area {:?} = {name:?} \
                     (was {:?}), and the VM's GetBindLocation() agrees: {vm:?}",
                    home.0, probe.baseline_area
                );
                probe.passes += 1;
                probe.bound_name = name;
                probe.phase = Phase::Line { since: now };
            } else if now - since > BIND_TIMEOUT_SECS {
                error!(
                    "PROBE_BINDER: FAIL (6 bind) — the hearthstone did not take within \
                     {BIND_TIMEOUT_SECS}s of ConfirmBinder(): HomeBind={:?} AreaTable name={name:?} \
                     GetBindLocation()={vm:?} (was area {:?} before the clear)",
                    home.0, probe.baseline_area
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Line { since } => {
            // The feedback half of B249: "accepting appears to change nothing" was partly that
            // nothing ever SAID it had. SMSG_PLAYERBOUND prints ERR_DEATHBIND_SUCCESS_S
            // ("%s is now your home.") as CHAT_MSG_SYSTEM — VERIFIED at `0x5e3d3f`.
            let want = bound_line(&probe.bound_name);
            let lines = system_lines(&script);
            if lines.iter().any(|l| l == &want) {
                info!("PROBE_BINDER: PASS (7 line) — the bind announced itself: {want:?}");
                probe.passes += 1;
                probe.phase = Phase::Done;
            } else if now - since > LINE_TIMEOUT_SECS {
                error!(
                    "PROBE_BINDER: FAIL (7 line) — no {want:?} within {LINE_TIMEOUT_SECS}s of the \
                     bind; CHAT_MSG_SYSTEM lines seen: {lines:?}"
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
                "PROBE_BINDER: DONE pass={} fail={}",
                probe.passes, probe.fails
            );
            // The probe self-exit pattern (`ProbeExitPlugin::fire_probe_exit`): a polite AppExit
            // plus a hard backstop thread, so a net/winit teardown hang can't leave a zombie
            // client holding the probe account.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_BINDER: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}

/// Steps 3 and 4 — B249's first half, then the click.
///
/// Split out of the phase match because it is one straight line of asserts with several exits, and
/// inlining it buried the phase machine's shape. Sets `probe.phase` on every path.
fn assert_icon_and_select(
    probe: &mut BinderProbe,
    gossip: &GossipState,
    script: &UiScript,
    net: &NetCommands,
    innkeeper: u64,
    now: f64,
) {
    // Step 3 — the icon. The row is found by the WIRE byte, because that byte is the fact under
    // test: every GOSSIP_OPTION_INNKEEPER row in the world DB sends 5 (module doc).
    let Some((pos, opt)) = gossip
        .options
        .iter()
        .enumerate()
        .find(|(_, o)| o.icon == ICON_INNKEEPER)
    else {
        error!(
            "PROBE_BINDER: FAIL (3 icon) — no wire icon=={ICON_INNKEEPER} row in Keldamyr's menu \
             (icons seen: {:?}); the DB pairs option_id 8 with icon 5 on all 21 of its rows, so \
             either the parse or the menu is wrong",
            gossip.options.iter().map(|o| o.icon).collect::<Vec<_>>()
        );
        probe.fails += 1;
        probe.phase = Phase::Done;
        return;
    };
    let ty = vm_icon_type(script, pos + 1);
    let texture = vm_binder_texture(script);
    if ty == ICON_TYPE_REGRESSION {
        error!(
            "PROBE_BINDER: FAIL (3 icon) — the innkeeper's row {:?} (wire icon={ICON_INNKEEPER}) \
             maps to {ICON_TYPE_REGRESSION:?}, the chat bubble. THIS IS THE B249 REGRESSION: \
             decision 1331's table must index byte 5 to {ICON_TYPE_BINDER:?}.",
            opt.message
        );
        probe.fails += 1;
        probe.phase = Phase::Done;
        return;
    }
    if ty != ICON_TYPE_BINDER {
        error!(
            "PROBE_BINDER: FAIL (3 icon) — the innkeeper's row {:?} (wire icon={ICON_INNKEEPER}) \
             maps to {ty:?}, wanted {ICON_TYPE_BINDER:?}",
            opt.message
        );
        probe.fails += 1;
        probe.phase = Phase::Done;
        return;
    }
    if texture.is_empty() {
        error!(
            "PROBE_BINDER: FAIL (3 icon) — the app maps the row to {ICON_TYPE_BINDER:?} but \
             BENILLA_GOSSIP_ICONS.binder resolves to nothing in the live VM, so the row would \
             still draw the fallback bubble"
        );
        probe.fails += 1;
        probe.phase = Phase::Done;
        return;
    }
    info!(
        "PROBE_BINDER: PASS (3 icon) — row {} {:?}: wire icon={ICON_INNKEEPER} index={} → type \
         {ty:?}, BENILLA_GOSSIP_ICONS.binder = {texture:?}",
        pos + 1,
        opt.message,
        opt.index
    );
    probe.passes += 1;

    // Step 4 — the click. Guarded on the label, and sent with the row's WIRE index — read off the
    // packet, never derived from where the row sits in the list (the drain's own rule).
    if !opt.message.to_lowercase().contains(BIND_LABEL_HINT) {
        warn!(
            "PROBE_BINDER: SKIP (4 select) — the icon=={ICON_INNKEEPER} row reads {:?}, which does \
             not contain {BIND_LABEL_HINT:?}; refusing to select something that may not be the \
             bind line",
            opt.message
        );
        probe.phase = Phase::Done;
        return;
    }
    let _ = net.0.send(ClientCommand::GossipSelectOption {
        guid: innkeeper,
        option: opt.index,
    });
    info!(
        "PROBE_BINDER: (4 select) GossipSelectOption({innkeeper:#x}, wire index {}) sent for {:?}",
        opt.index, opt.message
    );
    probe.bind_index = Some(opt.index);
    probe.phase = Phase::Confirm {
        since: now,
        events_baseline: events_len(script),
    };
}
