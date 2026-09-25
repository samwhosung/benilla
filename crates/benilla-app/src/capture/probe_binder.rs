//! The innkeeper-bind live probe (`WOW_PROBE_BINDER=1`): hops to Innkeeper Keldamyr (entry 6736,
//! Dolanaar, map 1), opens her gossip on the real wire, checks the bind row's icon maps to
//! `binder`, selects it, checks `SMSG_BINDER_CONFIRM` reaches `CONFIRM_BINDER`, answers with
//! `ConfirmBinder()`, and checks the hearthstone moved and the bind announced itself. One
//! `PROBE_BINDER: <step> PASS/FAIL/SKIP <detail>` line per step, then `PROBE_BINDER: DONE
//! pass=<n> fail=<m>`, and it exits. An environmental problem SKIPs; a wrong value FAILs.
//! Non-combat; the switches are `docs/CONTRIBUTING.md`, "Running it unattended".
//!
//! - `UNIT_NPC_FLAG_INNKEEPER` is `0x80` (vmangos `UnitDefines.h:664`); the `(65536)` beside
//!   `GOSSIP_OPTION_INNKEEPER` in `GossipDef.h:45` is a later client's value.
//! - Every `GOSSIP_OPTION_INNKEEPER` row in the world DB sends icon 5.
//! - The wire label is the row's broadcast text ("Make this inn your home."), not `option_text`.
//! - vmangos does not condition-filter a GM's menu, so a probe in GM mode sees extra rows: the
//!   row is found by its wire icon and selected by its wire index, never by position.
//! - `EffectBind` sends `SMSG_BINDPOINTUPDATE` even for a rebind to the same place
//!   (`SpellEffects.cpp:5806`), so the probe clears `HomeBind` first and waits for it to return.

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

/// Innkeeper Keldamyr's spawn (vmangos `creature` guid 46343), the `.go xyz` target.
const INNKEEPER_AT: [f32; 3] = [9802.21, 982.608, 1313.98];
/// Her map, Kalimdor; `.go xyz` takes the map id as its fourth argument.
const INNKEEPER_MAP: u32 = 1;
/// Her creature template entry, the primary identity check.
const INNKEEPER_ENTRY: u32 = 6736;
/// `UNIT_NPC_FLAG_INNKEEPER` on 1.12, the scan's fallback identity check.
const NPC_FLAG_INNKEEPER: u32 = 0x80;
/// The wire `GOSSIP_ICON` byte every `GOSSIP_OPTION_INNKEEPER` row sends.
const ICON_INNKEEPER: u8 = 5;
/// The type string [`crate::ui_gossip`]'s table must produce for [`ICON_INNKEEPER`].
const ICON_TYPE_BINDER: &str = "binder";
/// The chat bubble, what an unmapped icon byte falls back to.
const ICON_TYPE_REGRESSION: &str = "gossip";
/// A substring the bind row's label must carry before the probe selects it.
const BIND_LABEL_HINT: &str = "home";
/// Scan radius around the `.go` landing, wide enough for a slightly-off hop.
const SCAN_RANGE: f32 = 12.0;

const SETTLE_SECS: f64 = 3.0;
/// Generous: the hop lands on another map, and its terrain load leaves the probe about one frame
/// a second to poll in, though the packets arrive promptly.
const SCAN_TIMEOUT_SECS: f64 = 20.0;
const MENU_TIMEOUT_SECS: f64 = 20.0;
const CONFIRM_TIMEOUT_SECS: f64 = 20.0;
const BIND_TIMEOUT_SECS: f64 = 20.0;
const LINE_TIMEOUT_SECS: f64 = 20.0;

/// `ERR_DEATHBIND_SUCCESS_S` (`GlobalStrings.lua:1544`), written out so step 7 asserts against
/// the reference string, not `ui_binder`'s.
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

/// The probe's phase machine plus the identities found along the way.
#[derive(Resource, Default)]
struct BinderProbe {
    phase: Phase,
    /// The innkeeper's guid, once streamed in.
    innkeeper: Option<u64>,
    /// The bind row's wire index, echoed by `CMSG_GOSSIP_SELECT_OPTION`: its 0-based position
    /// among the rows sent (`GossipDef.cpp:188`), neither the DB id nor the Lua position.
    bind_index: Option<u32>,
    /// The area id [`HomeBind`] held before step 6 cleared it, reported in the verdict.
    baseline_area: Option<u32>,
    /// The area name the bind resolved to; step 7 composes its expected line from it.
    bound_name: String,
    passes: u32,
    fails: u32,
    /// Latched once [`Phase::Done`] has fired its exit.
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
    /// `GossipHello` sent; waiting for the parsed menu and its push into the VM (step 2).
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
    /// Step 7: waiting for `SMSG_PLAYERBOUND`'s `ERR_DEATHBIND_SUCCESS_S` as `CHAT_MSG_SYSTEM`.
    Line {
        since: f64,
    },
    Done,
}

/// The Lua-side `ProbeBinderEvents` log length (the `CONFIRM_BINDER` hook); 0 on an eval error.
fn events_len(script: &UiScript) -> i64 {
    script
        .eval::<i64>("return table.getn(ProbeBinderEvents or {})")
        .unwrap_or(0)
}

/// The newest `ProbeBinderEvents` entry: `CONFIRM_BINDER`'s `arg1`, the area name that fills the
/// dialog's `"Do you want to make %s your new home?"`.
fn last_event(script: &UiScript) -> String {
    script
        .eval::<String>("return ProbeBinderEvents[table.getn(ProbeBinderEvents)] or \"\"")
        .unwrap_or_default()
}

/// The `CHAT_MSG_SYSTEM` lines seen since the hook went in, newest last. `SMSG_PLAYERBOUND`
/// prints `ERR_DEATHBIND_SUCCESS_S` there (`0x5e3d3f` → `DisplayError(0x138)`, chat type 238).
fn system_lines(script: &UiScript) -> Vec<String> {
    script
        .eval::<Vec<String>>("return ProbeBinderSystemLines or {}")
        .unwrap_or_default()
}

/// How many values the live `GetGossipOptions()` returns: flat `(label, type)` pairs, twice the
/// row count.
fn vm_gossip_values(script: &UiScript) -> i64 {
    script
        .eval::<i64>("local t = { GetGossipOptions() } return table.getn(t)")
        .unwrap_or(0)
}

/// The icon type string `GetGossipOptions()` returns for the 1-based menu row `pos`, read where
/// `GossipFrame.lua` reads it.
fn vm_icon_type(script: &UiScript, pos: usize) -> String {
    script
        .eval::<String>(&format!(
            "local t = {{ GetGossipOptions() }} return t[{}] or \"\"",
            pos * 2
        ))
        .unwrap_or_default()
}

/// `BENILLA_GOSSIP_ICONS.binder` in the live VM, `""` when absent. The stock
/// `GossipFrame.lua:123` builds the icon path from the type string and defines no such table.
fn vm_binder_texture(script: &UiScript) -> String {
    script
        .eval::<String>("return (BENILLA_GOSSIP_ICONS and BENILLA_GOSSIP_ICONS.binder) or \"\"")
        .unwrap_or_default()
}

/// The live VM's `GetBindLocation()`, the hearthstone's own answer.
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
        return; // no UI VM in this build, so nothing to drive
    };
    let now = time.elapsed_secs_f64();
    let phase = probe.phase;

    match phase {
        Phase::Wait => {
            // The `CONFIRM_BINDER` and `CHAT_MSG_SYSTEM` hook, live before the question can arrive.
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
                // Clear the bind before answering: a rebind to the same area re-sends an
                // identical `SMSG_BINDPOINTUPDATE`, so only None → Some proves a fresh packet.
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
            // `SMSG_PLAYERBOUND` prints `ERR_DEATHBIND_SUCCESS_S` as a system line (`0x5e3d3f`).
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
            // `ProbeExitPlugin::fire_probe_exit`'s pattern: `AppExit` plus a hard backstop, so a
            // teardown hang cannot leave a client holding the probe account.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_BINDER: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}

/// Steps 3 and 4: the bind row's icon, then the click. Sets `probe.phase` on every path.
fn assert_icon_and_select(
    probe: &mut BinderProbe,
    gossip: &GossipState,
    script: &UiScript,
    net: &NetCommands,
    innkeeper: u64,
    now: f64,
) {
    // Step 3, the icon: the row is found by its wire byte, the fact under test.
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

    // Step 4, the click: guarded on the label, sent with the row's wire index from the packet.
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
