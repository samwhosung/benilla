//! The guild-charter live probe (`WOW_PROBE_CHARTER=1`): hop to the Stormwind guild registrar,
//! open his gossip menu, check the charter row's icon reads `petition`, select it by wire index,
//! buy a charter through the registrar's window, right-click it in the bags, watch the petition
//! window fill a round trip later, rename it and destroy it so the next run can repeat. One
//! `PROBE_CHARTER: <step> PASS/FAIL/SKIP <detail>` line per step, then
//! `PROBE_CHARTER: DONE pass=<n> fail=<m>`. Inert without the env.
//!
//! The registrar is Aldwin Laughlin (entry 4974, spawn guid 79681, map 0), `npc_flags` `0x601`:
//! the buy handler wants the petitioner flag and also refuses anything that is not a tabard
//! designer (`PetitionsHandler.cpp:44-52`). His menu 708 sends two unconditional rows, the charter
//! at wire index 0 with icon 7 (`petition` in the reference's `0x84b7ac` table) and the tabard
//! designer with icon 8. The charter is entry 5863, flags `0x2000` (`ITEM_FLAG_CHARTER`), no use
//! spell, at 1000 copper (`PetitionsHandler.cpp:38`); a charter name is at most 24 characters
//! (`ObjectMgr.h:401`), digits and spaces allowed.
//!
//! One client cannot sign, offer or turn in a charter: that half needs other accounts. Closing the
//! window is not asserted: closing another player's charter sends `MSG_PETITION_DECLINE`
//! (`0x4f3f60`).
//!
//! Non-combat; GM mode is left as found, every wait is bounded and the probe exits once done. How
//! to run it is `docs/CONTRIBUTING.md`, "Running it unattended".

use bevy::prelude::*;

use benilla_protocol::messages::{
    BAG_PLAYER_INVENTORY, CHARTER_ITEM_ENTRY, ITEM_FLAG_CHARTER, SLOT_BAG_FIRST, SLOT_PACK_FIRST,
};
use benilla_protocol::EntityKind;
use benilla_ui::script::UiScript;

use super::probes::ProbeClock;
use crate::items::Items;
use crate::net::{
    ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, Objects, SelfPlayer,
};
use crate::player::Player;
use crate::target::cursor_mode::npc_flags;
use crate::ui_gossip::GossipState;
use crate::ui_items::{find_item, ItemSearch, PACK_SLOTS};
use crate::ui_petition::GuildRegistrarState;
use crate::ui_session::NpcSession;

/// Aldwin Laughlin's spawn, the `.go xyz` target.
const REGISTRAR_AT: [f32; 3] = [-8885.25, 614.395, 95.2576];
/// Eastern Kingdoms, `.go xyz`'s fourth argument.
const REGISTRAR_MAP: u32 = 0;
/// His creature entry, the streamed-unit check; the petitioner flag is the fallback.
const REGISTRAR_ENTRY: u32 = 4974;
/// The wire icon byte of menu 708's charter row.
const ICON_PETITION: u8 = 7;
/// The type string [`crate::ui_gossip`]'s table must produce for [`ICON_PETITION`].
const ICON_TYPE_PETITION: &str = "petition";
/// What a wrong icon table produces instead: the chat bubble.
const ICON_TYPE_REGRESSION: &str = "gossip";
/// A lowercase substring of the charter row's label: the wire label is the row's broadcast text,
/// not its `option_text` column.
const CHARTER_LABEL_HINT: &str = "guild";
/// Scan radius around the `.go` landing.
const SCAN_RANGE: f32 = 12.0;
/// `GUILD_CHARTER_COST` in copper (`PetitionsHandler.cpp:38`), the showlist's `charterCost`.
const CHARTER_COST_COPPER: i64 = 1000;
/// `GetPetitionInfo()`'s `maxSignatures`: vmangos hardcodes min and max to 9
/// (`PetitionsHandler.cpp:182-183`) whatever `MinPetitionSigns` is.
const REQUIRED_SIGNATURES: i64 = 9;
/// `GetNumPetitionNames()` counts signers, and the owner is not one.
const FRESH_SIGNATURES: i64 = 0;
/// Copper sent up front so the buy never fails for funds. `.modify money` is `SEC_BASIC_ADMIN`
/// (`Chat.cpp:586`) and targets the sender with nothing selected (`Chat.cpp:2601-2612`), so it
/// goes before any NPC is targeted.
const FUND_COPPER: u32 = 100_000;

/// Settle after the `.go` before scanning.
const SETTLE_SECS: f64 = 3.0;
/// Generous: a `.go` can land inside a terrain load, where the probe polls about once a second.
const GUILD_CLEAR_TIMEOUT_SECS: f64 = 20.0;
const SCAN_TIMEOUT_SECS: f64 = 25.0;
const MENU_TIMEOUT_SECS: f64 = 20.0;
/// How long step 4 waits for the portrait's `"npc"` token, reported, not asserted:
/// `feed_interact_npc` is unordered against the apply pass, so it resolves a frame late.
const REGISTRAR_TOKEN_SETTLE_SECS: f64 = 1.0;

const REGISTRAR_TIMEOUT_SECS: f64 = 15.0;
/// The longest wait: nothing acknowledges a buy, so this is for the item and its template.
const BUY_TIMEOUT_SECS: f64 = 25.0;
const PETITION_TIMEOUT_SECS: f64 = 15.0;
const RECORD_TIMEOUT_SECS: f64 = 15.0;
const RENAME_TIMEOUT_SECS: f64 = 15.0;
const DESTROY_TIMEOUT_SECS: f64 = 15.0;

pub(crate) struct ProbeCharterPlugin;

impl Plugin for ProbeCharterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CharterProbe>()
            .add_systems(Update, charter_probe);
    }
}

/// The probe's phase machine and what it found; the `Copy` phase is snapshotted each tick so an
/// arm can mutate `probe` freely.
#[derive(Resource, Default)]
struct CharterProbe {
    phase: Phase,
    /// The registrar's guid, once streamed in.
    registrar: Option<u64>,
    /// The charter row's wire index, which `CMSG_GOSSIP_SELECT_OPTION` echoes: vmangos numbers the
    /// rows it sends from 0 (`GossipDef.cpp:188`), neither the DB id nor the Lua position.
    charter_row: Option<u32>,
    /// The bought charter's wire `(bag_index, slot)` and item guid, from [`find_item`].
    charter: Option<(u8, u8, u64)>,
    /// The guild name bought in step 5, the title step 7 waits for.
    bought: String,
    /// The name step 8 renames to, the title step 8 waits for.
    renamed: String,
    /// The title when the petition window first showed (step 6): empty, since the packet carries
    /// no text, or already filled.
    title_at_open: String,
    passes: u32,
    fails: u32,
    /// Latched once [`Phase::Done`] has fired its exit.
    exited: bool,
}

impl CharterProbe {
    fn pass(&mut self, step: u8, name: &str, detail: String) {
        self.passes += 1;
        info!("PROBE_CHARTER: {step} PASS ({name}) — {detail}");
    }

    fn fail(&mut self, step: u8, name: &str, detail: String) {
        self.fails += 1;
        error!("PROBE_CHARTER: {step} FAIL ({name}) — {detail}");
    }

    fn skip(&mut self, step: u8, name: &str, detail: String) {
        warn!("PROBE_CHARTER: {step} SKIP ({name}) — {detail}");
    }
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Wait,
    /// Step 0: money sent; leaving any guild an earlier run founded (`sent` once the verb ran).
    Unguild {
        since: f64,
        sent: bool,
    },
    /// Step 1: `.go` sent; settling while the registrar streams in.
    Settling {
        sent_at: f64,
    },
    /// Step 2: `GossipHello` sent; waiting for the parsed menu and its push into the VM.
    Menu {
        sent_at: f64,
    },
    /// Step 4: the charter row selected; waiting for `SMSG_PETITION_SHOWLIST` to reach
    /// [`GuildRegistrarState`] and `GUILD_REGISTRAR_SHOW` to open the window.
    Registrar {
        since: f64,
    },
    /// Step 5: the purchase sent; waiting for the charter item.
    Buying {
        since: f64,
        sent: bool,
    },
    /// Step 6: the charter right-clicked through the live VM; waiting for the petition window.
    Opening {
        since: f64,
        sent: bool,
    },
    /// Step 7: waiting for `SMSG_PETITION_QUERY_RESPONSE` to fill the title in.
    Record {
        since: f64,
    },
    /// Step 8: `RenamePetition` run; waiting for the echo to patch the record.
    Renaming {
        since: f64,
        sent: bool,
    },
    /// Step 9: the charter destroyed; waiting for it to leave the bags.
    Destroying {
        since: f64,
        sent: bool,
    },
    Done,
}

// ── The live-VM readings ─────────────────────────────────────────────────────────────────────
// Each answers a default on an eval error, read as "nothing yet"; the timeouts give the verdict.

/// The `CHAT_MSG_SYSTEM` / `UI_ERROR_MESSAGE` lines seen since the hook went in, newest last: a
/// refused buy or rename comes back only as `SMSG_GUILD_COMMAND_RESULT`, printed as a system line.
fn probe_lines(script: &UiScript) -> Vec<String> {
    script
        .eval::<Vec<String>>("return ProbeCharterLines or {}")
        .unwrap_or_default()
}

/// How many values the live `GetGossipOptions()` returns, flat `(label, type)` pairs: on a first
/// visit the menu reaches the VM only once `SMSG_NPC_TEXT_UPDATE` lands.
fn vm_gossip_values(script: &UiScript) -> i64 {
    script
        .eval::<i64>("local t = { GetGossipOptions() } return table.getn(t)")
        .unwrap_or(0)
}

/// The icon type string for the 1-based menu row `pos`, read through `GetGossipOptions()` as the
/// stock frame reads it.
fn vm_icon_type(script: &UiScript, pos: usize) -> String {
    script
        .eval::<String>(&format!(
            "local t = {{ GetGossipOptions() }} return t[{}] or \"\"",
            pos * 2
        ))
        .unwrap_or_default()
}

/// The charter's bag tooltip, line by line, from `SetBagItem` in the live VM; step 8 checks only
/// that the renamed title reaches it, the wording is a unit test's.
fn charter_tooltip_lines(script: &UiScript, pos: Option<(i64, u32)>) -> Vec<String> {
    let Some((bag, slot)) = pos else {
        return Vec::new();
    };
    let chunk = format!(
        r#"
        local a = getglobal("BenillaProbeTipAnchor")
        if not a then
            a = CreateFrame("Button", "BenillaProbeTipAnchor")
            a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        end
        GameTooltip:SetOwner(a, "ANCHOR_RIGHT")
        GameTooltip:SetBagItem({bag}, {slot})
        local out = {{}}
        for i = 1, GameTooltip:NumLines() do
            local L = getglobal("GameTooltipTextLeft" .. i)
            if L then table.insert(out, L:GetText() or "") end
        end
        return table.concat(out, " | ")
    "#
    );
    script
        .eval::<String>(&chunk)
        .map(|s| s.split(" | ").map(str::to_string).collect())
        .unwrap_or_default()
}

/// `BENILLA_GOSSIP_ICONS.petition` in the live VM, `""` when absent. The stock
/// `GossipFrame.lua:123` builds the icon path from the type string and defines no such table.
fn vm_petition_texture(script: &UiScript) -> String {
    script
        .eval::<String>("return (BENILLA_GOSSIP_ICONS and BENILLA_GOSSIP_ICONS.petition) or \"\"")
        .unwrap_or_default()
}

/// Is a named frame visible in the live UI; a missing global reads `false`.
fn vm_visible(script: &UiScript, frame: &str) -> bool {
    script
        .eval::<bool>(&format!(
            "return ({frame} and {frame}:IsVisible()) and 1 or nil"
        ))
        .unwrap_or(false)
}

/// `GetGuildCharterCost()`, the registrar's price in copper.
fn vm_charter_cost(script: &UiScript) -> i64 {
    script
        .eval::<i64>("return GetGuildCharterCost()")
        .unwrap_or(-1)
}

/// One of `GetPetitionInfo()`'s six returns, by 1-based position; `""` with nothing open, when it
/// returns no values.
fn vm_petition_str(script: &UiScript, slot: usize) -> String {
    let binds = "_,".repeat(slot - 1);
    script
        .eval::<String>(&format!(
            "local {binds}v = GetPetitionInfo() return v or \"\""
        ))
        .unwrap_or_default()
}

/// `GetPetitionInfo()`'s fourth return, the signature requirement.
fn vm_petition_max(script: &UiScript) -> i64 {
    script
        .eval::<i64>("local _,_,_,v = GetPetitionInfo() return v or -1")
        .unwrap_or(-1)
}

/// `GetPetitionInfo()`'s sixth return, `isOriginator`: `1` or `nil`, never a boolean.
fn vm_is_originator(script: &UiScript) -> i64 {
    script
        .eval::<i64>("local _,_,_,_,_,v = GetPetitionInfo() return v or 0")
        .unwrap_or(0)
}

/// `GetNumPetitionNames()`, the signers, never the owner.
fn vm_num_names(script: &UiScript) -> i64 {
    script
        .eval::<i64>("return GetNumPetitionNames()")
        .unwrap_or(-1)
}

/// The inverse of [`crate::ui_items::wire_pos`] for the backpack (Lua container `0`) and the four
/// equipped bags (`1..=4`); `None` anywhere else.
fn lua_bag_pos(bag_index: u8, slot: u8) -> Option<(i64, u32)> {
    if bag_index == BAG_PLAYER_INVENTORY {
        let inner = slot.checked_sub(SLOT_PACK_FIRST)?;
        (inner < PACK_SLOTS).then_some((0, u32::from(inner) + 1))
    } else {
        let bag = bag_index.checked_sub(SLOT_BAG_FIRST)?;
        (bag < 4 && slot < 36).then_some((i64::from(bag) + 1, u32::from(slot) + 1))
    }
}

/// A fresh guild name: a 13-character prefix, a space and the low 8 digits of the wall clock, 22
/// characters within `MAX_CHARTER_NAME` (24); `IsValidCharterName` allows digits and spaces.
fn probe_guild_name(prefix: &str) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        % 100_000_000;
    format!("{prefix} {secs:08}")
}

fn charter_probe(
    time: ProbeClock,
    mut probe: ResMut<CharterProbe>,
    gossip: Res<GossipState>,
    registrar: Res<GuildRegistrarState>,
    objects: Objects,
    items: Res<Items>,
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    player: Res<Player>,
    units: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<NetCommands>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(store) = self_q.single() else {
        return; // not in-world yet
    };
    let Some(script) = script else {
        return; // no UI VM (headless net-only)
    };
    let now = time.elapsed_secs_f64();
    let phase = probe.phase;

    match phase {
        Phase::Wait => {
            // The refusal channel, installed before the first verb.
            if let Err(e) = script.run(
                r#"
                if not ProbeCharterHooked then
                    ProbeCharterHooked = true
                    ProbeCharterLines = {}
                    local f = CreateFrame("Frame")
                    f:RegisterEvent("CHAT_MSG_SYSTEM")
                    f:RegisterEvent("UI_ERROR_MESSAGE")
                    f:SetScript("OnEvent", function()
                        table.insert(ProbeCharterLines, (event or "") .. ": " .. (arg1 or ""))
                    end)
                end
                "#,
            ) {
                error!("PROBE_CHARTER: installing the refusal hook: {e}");
            }
            // Money first: with a creature selected, `.modify money` answers "no character
            // selected".
            let _ = net.0.send(ClientCommand::Chat {
                kind: ChatKind::Say,
                target: None,
                text: format!(".modify money {FUND_COPPER}"),
            });
            info!(
                "PROBE_CHARTER: 0 (precheck) — funding the body with {FUND_COPPER} copper so the \
                 buy cannot fail for money"
            );
            probe.phase = Phase::Unguild {
                since: now,
                sent: false,
            };
        }
        // ── Step 0: the preconditions ───────────────────────────────────────────────────────
        // A buy is refused silently while the buyer is in a guild (`PetitionsHandler.cpp:66`), so
        // any guild an earlier run founded is left first; FAIL if `PLAYER_GUILDID` never clears.
        Phase::Unguild { since, sent } => {
            let guild_id = store.0.player_guild_id();
            if !sent {
                if guild_id == 0 {
                    probe.pass(
                        0,
                        "precheck",
                        "not in a guild — nothing to leave, and the buy's silent guilded-refusal \
                         cannot apply"
                            .to_string(),
                    );
                    return hop(&mut probe, &net, now);
                }
                // A guild master cannot leave (`ERR_GUILD_LEADER_LEAVE`), so rank 0 disbands.
                // The rank is read off the descriptor, not `IsGuildLeader()`, whose feed can lag
                // a frame behind on the first frame in-world.
                let verb = if store.0.player_guild_rank() == 0 {
                    "GuildDisband()"
                } else {
                    "GuildLeave()"
                };
                if let Err(e) = script.run(verb) {
                    probe.fail(
                        0,
                        "precheck",
                        format!(
                            "in guild {guild_id} and {verb} would not run in the live VM: {e} — \
                             the buy in step 5 would be refused silently"
                        ),
                    );
                    probe.phase = Phase::Done;
                    return;
                }
                info!(
                    "PROBE_CHARTER: 0 (precheck) — still in guild {guild_id} from an earlier run; \
                     {verb} sent through the live VM"
                );
                probe.phase = Phase::Unguild {
                    since: now,
                    sent: true,
                };
            } else if guild_id == 0 {
                probe.pass(
                    0,
                    "precheck",
                    "the guild an earlier run founded is gone — PLAYER_GUILDID is back to 0"
                        .to_string(),
                );
                hop(&mut probe, &net, now);
            } else if now - since > GUILD_CLEAR_TIMEOUT_SECS {
                probe.fail(
                    0,
                    "precheck",
                    format!(
                        "still in guild {guild_id} {GUILD_CLEAR_TIMEOUT_SECS}s after the leave — \
                         the buy in step 5 would be refused SILENTLY (a bare `return` at \
                         PetitionsHandler.cpp:66), so the run is stopped here rather than \
                         reporting that as a wire failure. Lines seen: {:?}",
                        probe_lines(&script)
                    ),
                );
                probe.phase = Phase::Done;
            }
        }
        // ── Step 1: the hop ─────────────────────────────────────────────────────────────────
        // A SKIP here is environmental: the `.go` was refused or Stormwind never streamed.
        Phase::Settling { sent_at } => {
            if now - sent_at < SETTLE_SECS {
                return;
            }
            let me = player.pos;
            let found = units.iter().find(|(_, net_e, store, tf)| {
                net_e.kind == EntityKind::Unit
                    && (store.0.object_entry() == Some(REGISTRAR_ENTRY)
                        || store.0.unit_npc_flags() & npc_flags::PETITIONER != 0)
                    && tf.translation.distance(me) < SCAN_RANGE
            });
            if let Some((guid, ..)) = found {
                probe.pass(
                    1,
                    "hop",
                    format!(
                        "registrar {:#x} streamed within {SCAN_RANGE}yd of the landing",
                        guid.0
                    ),
                );
                probe.registrar = Some(guid.0);
                let _ = net.0.send(ClientCommand::GossipHello { guid: guid.0 });
                info!("PROBE_CHARTER: 2 (menu) GossipHello({:#x}) sent", guid.0);
                probe.phase = Phase::Menu { sent_at: now };
            } else if now - sent_at > SCAN_TIMEOUT_SECS {
                probe.skip(
                    1,
                    "hop",
                    format!(
                        "no entry {REGISTRAR_ENTRY} / petitioner-flagged unit streamed in within \
                         {SCAN_TIMEOUT_SECS}s of the hop (the `.go` may have been refused, or \
                         Stormwind never streamed) — environmental, not a defect"
                    ),
                );
                probe.phase = Phase::Done;
            }
        }
        // ── Step 2: the menu ────────────────────────────────────────────────────────────────
        // Waits for the menu in the VM, not the packet: `ui_gossip` holds a first visit's menu
        // until `SMSG_NPC_TEXT_UPDATE` answers the greeting query.
        Phase::Menu { sent_at } => {
            let Some(npc) = probe.registrar else {
                probe.phase = Phase::Done;
                return;
            };
            let open = gossip.npc == Some(npc) && !gossip.options.is_empty();
            let pushed = vm_gossip_values(&script) as usize == gossip.options.len() * 2;
            if open && pushed {
                probe.pass(
                    2,
                    "menu",
                    format!(
                        "{} option(s) open on {npc:#x}: {}",
                        gossip.options.len(),
                        gossip
                            .options
                            .iter()
                            .map(|o| format!("[{} icon={} {:?}]", o.index, o.icon, o.message))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ),
                );
                assert_icon_and_select(&mut probe, &gossip, &script, &net, npc, now);
            } else if now - sent_at > MENU_TIMEOUT_SECS {
                probe.fail(
                    2,
                    "menu",
                    format!(
                        "no gossip menu for {npc:#x} within {MENU_TIMEOUT_SECS}s (parsed \
                         npc={:?} options={} vm_values={})",
                        gossip.npc,
                        gossip.options.len(),
                        vm_gossip_values(&script)
                    ),
                );
                probe.phase = Phase::Done;
            }
        }
        // ── Step 4's second half: the window ────────────────────────────────────────────────
        // `SMSG_PETITION_SHOWLIST` must fire `GUILD_REGISTRAR_SHOW`, and `GetGuildCharterCost()`
        // must read the showlist's `charterCost`, which vmangos always sends as 1000.
        Phase::Registrar { since } => {
            let Some(npc) = probe.registrar else {
                probe.phase = Phase::Done;
                return;
            };
            let parked = registrar.npc() == Some(npc);
            let visible = vm_visible(&script, "GuildRegistrarFrame");
            let cost = vm_charter_cost(&script);
            if parked && visible && cost == CHARTER_COST_COPPER {
                // The portrait's `"npc"` token is reported, not asserted, and sampled up to
                // `REGISTRAR_TOKEN_SETTLE_SECS` late: `feed_interact_npc` is unordered against
                // the apply pass, so it is empty on the frame the window opens.
                let token = script
                    .eval::<String>("return UnitName(\"npc\") or \"\"")
                    .unwrap_or_default();
                if token.is_empty() && now - since < REGISTRAR_TOKEN_SETTLE_SECS {
                    return;
                }
                let row = probe.charter_row;
                probe.pass(
                    4,
                    "registrar",
                    format!(
                        "wire row {row:?} selected: SMSG_PETITION_SHOWLIST parked on {npc:#x}, \
                         GUILD_REGISTRAR_SHOW opened GuildRegistrarFrame, and \
                         GetGuildCharterCost() reads {cost} copper (portrait token \
                         UnitName(\"npc\") = {token:?})"
                    ),
                );
                probe.phase = Phase::Buying {
                    since: now,
                    sent: false,
                };
            } else if parked && visible && cost != CHARTER_COST_COPPER {
                // The window opened, so the packet parsed: a value bug, not a wire one.
                probe.fail(
                    4,
                    "registrar",
                    format!(
                        "the registrar opened on {npc:#x} but GetGuildCharterCost() reads {cost}, \
                         not {CHARTER_COST_COPPER} (vmangos GUILD_CHARTER_COST, \
                         PetitionsHandler.cpp:38) — the showlist row's charterCost is not what \
                         the getter reads, or the visible-row `&1` rule picked the wrong row"
                    ),
                );
                probe.phase = Phase::Done;
            } else if now - since > REGISTRAR_TIMEOUT_SECS {
                probe.fail(
                    4,
                    "registrar",
                    format!(
                        "no registrar window within {REGISTRAR_TIMEOUT_SECS}s of the select: \
                         GuildRegistrarState npc={:?} (wanted {npc:#x}), \
                         GuildRegistrarFrame:IsVisible()={visible}, GetGuildCharterCost()={cost}. \
                         An SMSG_PETITION_SHOWLIST with no parse arm at all \
                         falls through to ServerPacket::Other — that is what this reading looks \
                         like. Lines seen: {:?}",
                        registrar.npc(),
                        probe_lines(&script)
                    ),
                );
                probe.phase = Phase::Done;
            }
        }
        // ── Step 5: the buy ─────────────────────────────────────────────────────────────────
        // `GuildRegistrar_ShowPurchaseFrame()` then the Purchase button's `BuyGuildCharter(name)`.
        // The stock button also hides the window (`GuildRegistrarFrame.xml:269-270`); the probe
        // does not, and step 6's `PetitionFrame` displaces it (both left, pushable 0,
        // `UIParent.lua:33-34`). A successful buy has no reply, only the item
        // (`PetitionsHandler.cpp:130`); of the refusals only a taken name says anything.
        Phase::Buying { since, sent } => {
            if !sent {
                let name = probe_guild_name("Probe Charter");
                if let Err(e) = script.run(&format!(
                    "GuildRegistrar_ShowPurchaseFrame() BuyGuildCharter(\"{name}\")"
                )) {
                    probe.skip(
                        5,
                        "buy",
                        format!(
                            "the purchase chunk would not run in the live VM: {e} \
                             (environmental, not a wire failure)"
                        ),
                    );
                    probe.phase = Phase::Done;
                    return;
                }
                info!(
                    "PROBE_CHARTER: 5 (buy) — GuildRegistrar_ShowPurchaseFrame() then \
                     BuyGuildCharter({name:?}) run in the live VM; waiting for item \
                     {CHARTER_ITEM_ENTRY} to reach the bags"
                );
                probe.bought = name;
                probe.phase = Phase::Buying {
                    since: now,
                    sent: true,
                };
                return;
            }
            let found = find_item(
                &store.0,
                &objects,
                CHARTER_ITEM_ENTRY,
                ItemSearch::default(),
            );
            // The template must have landed too: the click's charter arm tests its
            // `ITEM_FLAG_CHARTER`. Folded into one `Option` so a missing template still times out.
            let ready = found.and_then(|(bag_index, slot, guid)| {
                items
                    .template(CHARTER_ITEM_ENTRY, guid, &net)
                    .map(|t| (bag_index, slot, guid, t.flags))
            });
            if let Some((bag_index, slot, guid, flags)) = ready {
                if flags & ITEM_FLAG_CHARTER == 0 {
                    probe.fail(
                        5,
                        "buy",
                        format!(
                            "item {CHARTER_ITEM_ENTRY} arrived at wire {bag_index}/{slot} but its \
                             template flags are {flags:#x}, with no ITEM_FLAG_CHARTER \
                             ({ITEM_FLAG_CHARTER:#x}) — the live server disagrees with the world \
                             DB's `flags = 8192`, and the item-use fork's charter arm cannot fire"
                        ),
                    );
                    probe.charter = Some((bag_index, slot, guid));
                    probe.phase = Phase::Destroying {
                        since: now,
                        sent: false,
                    };
                    return;
                }
                probe.pass(
                    5,
                    "buy",
                    format!(
                        "charter {guid:#x} (entry {CHARTER_ITEM_ENTRY}, template flags \
                         {flags:#x}) is in the bags at wire {bag_index}/{slot} — the only \
                         acknowledgement a successful buy has"
                    ),
                );
                probe.charter = Some((bag_index, slot, guid));
                probe.phase = Phase::Opening {
                    since: now,
                    sent: false,
                };
            } else if now - since > BUY_TIMEOUT_SECS {
                let bought = probe.bought.clone();
                probe.fail(
                    5,
                    "buy",
                    format!(
                        "no usable item {CHARTER_ITEM_ENTRY} in the bags {BUY_TIMEOUT_SECS}s \
                         after BuyGuildCharter({bought:?}) — find_item says {found:?} and its \
                         template {}. Nothing acks a buy, so a missing item is either a send that \
                         never happened or one of vmangos's four refusals (already guilded, \
                         already owns a petition, the name is taken/invalid, not enough money); an \
                         item present with no template is the ask-once ItemQuery going \
                         unanswered. Lines seen: {:?}",
                        if found.is_some() {
                            "never answered"
                        } else {
                            "was never asked for"
                        },
                        probe_lines(&script)
                    ),
                );
                // A charter seen without a template still goes to the cleanup.
                probe.charter = found;
                probe.phase = if found.is_some() {
                    Phase::Destroying {
                        since: now,
                        sent: false,
                    }
                } else {
                    Phase::Done
                };
            }
        }
        // ── Step 6: the item-use fork ───────────────────────────────────────────────────────
        // A real bag right-click through `UseContainerItem`, so `drain_container_uses` reaches
        // `ItemUseRoute::ShowPetition`. A FAIL is either the fork missing the charter arm or
        // `SMSG_PETITION_SHOW_SIGNATURES` never becoming `PETITION_SHOW`. No getter exposes the
        // open charter's item guid, so its identity rests on no window being up before the click.
        Phase::Opening { since, sent } => {
            let Some((bag_index, slot, guid)) = probe.charter else {
                probe.phase = Phase::Done;
                return;
            };
            if !sent {
                // The control: a window already up would make the step meaningless.
                if vm_visible(&script, "PetitionFrame") {
                    probe.fail(
                        6,
                        "open",
                        "PetitionFrame was ALREADY visible before the click — the control failed, \
                         so nothing this step could read would be evidence"
                            .to_string(),
                    );
                    probe.phase = Phase::Destroying {
                        since: now,
                        sent: false,
                    };
                    return;
                }
                let Some((container, lua_slot)) = lua_bag_pos(bag_index, slot) else {
                    probe.skip(
                        6,
                        "open",
                        format!(
                            "the charter landed at wire {bag_index}/{slot}, which is not a \
                             position a bag right-click can address — refusing to click a slot \
                             the probe cannot name"
                        ),
                    );
                    probe.phase = Phase::Destroying {
                        since: now,
                        sent: false,
                    };
                    return;
                };
                if let Err(e) = script.run(&format!("UseContainerItem({container}, {lua_slot})")) {
                    probe.skip(
                        6,
                        "open",
                        format!("the click chunk did not run in the live VM: {e}"),
                    );
                    probe.phase = Phase::Destroying {
                        since: now,
                        sent: false,
                    };
                    return;
                }
                info!(
                    "PROBE_CHARTER: 6 (open) — UseContainerItem({container}, {lua_slot}) through \
                     the live VM for charter {guid:#x} (wire {bag_index}/{slot})"
                );
                probe.phase = Phase::Opening {
                    since: now,
                    sent: true,
                };
                return;
            }
            let visible = vm_visible(&script, "PetitionFrame");
            let kind = vm_petition_str(&script, 1);
            let originator = vm_is_originator(&script);
            let names = vm_num_names(&script);
            if visible
                && kind == benilla_ui::script::PETITION_TYPE_CHARTER
                && originator == 1
                && names == FRESH_SIGNATURES
            {
                let at_open = vm_petition_str(&script, 2);
                probe.title_at_open = at_open.clone();
                probe.pass(
                    6,
                    "open",
                    format!(
                        "the click on charter {guid:#x} opened PetitionFrame: \
                         petitionType={kind:?}, isOriginator=1, GetNumPetitionNames()={names} \
                         (the owner is not a signer), title at open {at_open:?}"
                    ),
                );
                probe.phase = Phase::Record { since: now };
            } else if now - since > PETITION_TIMEOUT_SECS {
                probe.fail(
                    6,
                    "open",
                    format!(
                        "charter {guid:#x} did not open within {PETITION_TIMEOUT_SECS}s of the \
                         click: PetitionFrame:IsVisible()={visible}, petitionType={kind:?}, \
                         isOriginator={originator}, GetNumPetitionNames()={names}. Either the \
                         item-use fork never reached ItemUseRoute::ShowPetition (a charter that \
                         falls through to `Nothing` sends nothing at all — a silent \
                         failure) or the answer never became a window. Lines seen: {:?}",
                        probe_lines(&script)
                    ),
                );
                probe.phase = Phase::Destroying {
                    since: now,
                    sent: false,
                };
            }
        }
        // ── Step 7: the lazy record fill ────────────────────────────────────────────────────
        // `SMSG_PETITION_SHOW_SIGNATURES` carries guids and a petition id but no text; the name
        // arrives on `SMSG_PETITION_QUERY_RESPONSE`, keyed by petition id, and the feed re-fires
        // `PETITION_SHOW`. A FAIL leaves the window titleless.
        Phase::Record { since } => {
            let title = vm_petition_str(&script, 2);
            let max = vm_petition_max(&script);
            if title == probe.bought && max == REQUIRED_SIGNATURES {
                // At open the title was empty or already the bought name (the record beat the
                // paint); anything else came from another charter's record.
                let at_open = probe.title_at_open.clone();
                if at_open.is_empty() || at_open == probe.bought {
                    probe.pass(
                        7,
                        "record",
                        format!(
                            "the window opened with title {at_open:?} and filled to {title:?} a \
                             round trip later, maxSignatures={max} — the record cache and its \
                             PETITION_SHOW repaint both working"
                        ),
                    );
                } else {
                    probe.fail(
                        7,
                        "record",
                        format!(
                            "the title filled to {title:?} correctly, but at open it read \
                             {at_open:?} — neither empty nor the bought name, so it came from a \
                             record that is not this charter's"
                        ),
                    );
                }
                probe.phase = Phase::Renaming {
                    since: now,
                    sent: false,
                };
            } else if now - since > RECORD_TIMEOUT_SECS {
                let bought = probe.bought.clone();
                probe.fail(
                    7,
                    "record",
                    format!(
                        "the petition record never landed within {RECORD_TIMEOUT_SECS}s: title \
                         reads {title:?} (wanted {bought:?}), maxSignatures={max} (wanted \
                         {REQUIRED_SIGNATURES} — vmangos hardcodes 9 at \
                         PetitionsHandler.cpp:182-183). The window opens titleless by design, so \
                         this is the lazy CMSG_PETITION_QUERY, its answer, or the repaint edge"
                    ),
                );
                probe.phase = Phase::Destroying {
                    since: now,
                    sent: false,
                };
            }
        }
        // ── Step 8: the rename echo ─────────────────────────────────────────────────────────
        // `RenamePetition(name)`, the `RENAME_GUILD` popup's Accept. vmangos echoes
        // `MSG_PETITION_RENAME` only on success (`PetitionsHandler.cpp:209-215`), and nothing
        // re-queries, so the echo must patch the cached record.
        Phase::Renaming { since, sent } => {
            if !sent {
                let name = probe_guild_name("Probe Renamed");
                if let Err(e) = script.run(&format!("RenamePetition(\"{name}\")")) {
                    probe.skip(
                        8,
                        "rename",
                        format!("RenamePetition would not run in the live VM: {e}"),
                    );
                    probe.phase = Phase::Destroying {
                        since: now,
                        sent: false,
                    };
                    return;
                }
                info!("PROBE_CHARTER: 8 (rename) — RenamePetition({name:?}) run in the live VM");
                probe.renamed = name;
                probe.phase = Phase::Renaming {
                    since: now,
                    sent: true,
                };
                return;
            }
            let title = vm_petition_str(&script, 2);
            // The bag tooltip must reach the same name: it is fed by the container snapshot,
            // whose rebuild is gated, so it can lag the window.
            let tip = charter_tooltip_lines(
                &script,
                probe.charter.and_then(|(b, sl, _)| lua_bag_pos(b, sl)),
            );
            let tip_caught_up = tip.iter().any(|l| l.contains(&probe.renamed));
            if title == probe.renamed && tip_caught_up {
                let bought = probe.bought.clone();
                // The tooltip lines are reported; their wording is pinned by
                // `charter_lines_sit_between_the_name_and_the_signable_line`.
                probe.pass(
                    8,
                    "rename",
                    format!(
                        "the title changed from {bought:?} to {title:?} — MSG_PETITION_RENAME's \
                         echo arrived and patched the cached record in place; the bag tooltip \
                         reads {tip:?}"
                    ),
                );
                probe.phase = Phase::Destroying {
                    since: now,
                    sent: false,
                };
            } else if now - since > RENAME_TIMEOUT_SECS {
                let renamed = probe.renamed.clone();
                probe.fail(
                    8,
                    "rename",
                    format!(
                        "the title still reads {title:?} {RENAME_TIMEOUT_SECS}s after \
                         RenamePetition({renamed:?}). The echo is sent ONLY on success, so either \
                         the rename was refused (name taken/invalid/reserved) or the echo did not \
                         patch the record. Lines seen: {:?}",
                        probe_lines(&script)
                    ),
                );
                probe.phase = Phase::Destroying {
                    since: now,
                    sent: false,
                };
            }
        }
        // ── Step 9: the cleanup ─────────────────────────────────────────────────────────────
        // `CMSG_DESTROYITEM` deletes the petition with the charter (`Player.cpp:10811-10817`);
        // without it the next run's buy is refused silently (`PetitionsHandler.cpp:70`), so every
        // exit from steps 5-8 comes here. The destroy is watched to land before `AppExit` tears
        // the net thread down.
        Phase::Destroying { since, sent } => {
            let Some((bag_index, slot, guid)) = probe.charter else {
                probe.phase = Phase::Done; // nothing was bought
                return;
            };
            if !sent {
                // The position is read fresh: `count: 0` destroys whatever stack sits there.
                let Some((bag_index, slot, _)) = find_item(
                    &store.0,
                    &objects,
                    CHARTER_ITEM_ENTRY,
                    ItemSearch::default(),
                ) else {
                    probe.pass(
                        9,
                        "cleanup",
                        format!(
                            "charter {guid:#x} was already out of the bags before the destroy — \
                             nothing left to clean up (latched at wire {bag_index}/{slot})"
                        ),
                    );
                    probe.phase = Phase::Done;
                    return;
                };
                let _ = net.0.send(ClientCommand::DestroyItem {
                    bag_index,
                    slot,
                    count: 0,
                });
                info!(
                    "PROBE_CHARTER: 9 (cleanup) — CMSG_DESTROYITEM on wire {bag_index}/{slot} \
                     (charter {guid:#x}); vmangos deletes the petition with it"
                );
                probe.phase = Phase::Destroying {
                    since: now,
                    sent: true,
                };
                return;
            }
            let still_there = find_item(
                &store.0,
                &objects,
                CHARTER_ITEM_ENTRY,
                ItemSearch::default(),
            );
            if still_there.is_none() {
                probe.pass(
                    9,
                    "cleanup",
                    format!(
                        "charter {guid:#x} left the bags and its petition went with it — the next \
                         run's buy will not hit the silent already-owns-a-petition refusal"
                    ),
                );
                probe.phase = Phase::Done;
            } else if now - since > DESTROY_TIMEOUT_SECS {
                probe.fail(
                    9,
                    "cleanup",
                    format!(
                        "charter {guid:#x} is STILL in the bags {DESTROY_TIMEOUT_SECS}s after \
                         CMSG_DESTROYITEM. Remove it by hand before the next run — otherwise its \
                         step 5 is refused silently for `the owner already has one` and reads as a \
                         buy that never sent. Lines seen: {:?}",
                        probe_lines(&script)
                    ),
                );
                probe.phase = Phase::Done;
            }
        }
        Phase::Done => {
            if probe.exited {
                return;
            }
            probe.exited = true;
            info!(
                "PROBE_CHARTER: DONE pass={} fail={}",
                probe.passes, probe.fails
            );
            // `AppExit` plus a hard-exit thread, so a teardown hang cannot hold the account.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_CHARTER: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}

/// Step 1's send, from both of step 0's exits.
fn hop(probe: &mut CharterProbe, net: &NetCommands, now: f64) {
    let [x, y, z] = REGISTRAR_AT;
    info!(
        "PROBE_CHARTER: 1 (hop) — hopping to Aldwin Laughlin (entry {REGISTRAR_ENTRY}) at \
         ({x}, {y}, {z}) map {REGISTRAR_MAP}"
    );
    let _ = net.0.send(ClientCommand::Chat {
        kind: ChatKind::Say,
        target: None,
        text: format!(".go xyz {x} {y} {z} {REGISTRAR_MAP}"),
    });
    probe.phase = Phase::Settling { sent_at: now };
}

/// Step 3, the icon (byte 7 is `"petition"` in the reference's `0x84b7ac` table), then step 4's
/// select by the row's wire index, never its list position. Sets `probe.phase` on every path.
fn assert_icon_and_select(
    probe: &mut CharterProbe,
    gossip: &GossipState,
    script: &UiScript,
    net: &NetCommands,
    npc: u64,
    now: f64,
) {
    let Some((pos, opt)) = gossip
        .options
        .iter()
        .enumerate()
        .find(|(_, o)| o.icon == ICON_PETITION)
    else {
        probe.fail(
            3,
            "icon",
            format!(
                "no wire icon=={ICON_PETITION} row in Aldwin's menu (icons seen: {:?}); menu 708 \
                 pairs option_id 10 (GOSSIP_OPTION_PETITIONER) with icon 7 unconditionally, so \
                 either the parse or the menu is wrong",
                gossip.options.iter().map(|o| o.icon).collect::<Vec<_>>()
            ),
        );
        probe.phase = Phase::Done;
        return;
    };
    let ty = vm_icon_type(script, pos + 1);
    if ty == ICON_TYPE_REGRESSION {
        probe.fail(
            3,
            "icon",
            format!(
                "the charter row {:?} (wire icon={ICON_PETITION}) maps to \
                 {ICON_TYPE_REGRESSION:?}, the chat bubble: the binder's icon regression \
                 one row over: `ui_gossip`'s table must index byte 7 to {ICON_TYPE_PETITION:?}",
                opt.message
            ),
        );
        probe.phase = Phase::Done;
        return;
    }
    if ty != ICON_TYPE_PETITION {
        probe.fail(
            3,
            "icon",
            format!(
                "the charter row {:?} (wire icon={ICON_PETITION}) maps to {ty:?}, wanted \
                 {ICON_TYPE_PETITION:?}",
                opt.message
            ),
        );
        probe.phase = Phase::Done;
        return;
    }
    // The stock interface defines no `BENILLA_GOSSIP_ICONS` (see `vm_petition_texture`).
    let texture = vm_petition_texture(script);
    if texture.is_empty() {
        probe.fail(
            3,
            "icon",
            format!(
                "the app maps the row to {ICON_TYPE_PETITION:?} but BENILLA_GOSSIP_ICONS.petition \
                 resolves to nothing in the live VM, so the row would still draw the fallback \
                 bubble"
            ),
        );
        probe.phase = Phase::Done;
        return;
    }
    probe.pass(
        3,
        "icon",
        format!(
            "row {} {:?}: wire icon={ICON_PETITION} index={} → type {ty:?}, \
             BENILLA_GOSSIP_ICONS.petition = {texture:?}",
            pos + 1,
            opt.message,
            opt.index
        ),
    );

    // Step 4's click, guarded on the label substring.
    if !opt.message.to_lowercase().contains(CHARTER_LABEL_HINT) {
        probe.skip(
            4,
            "registrar",
            format!(
                "the icon=={ICON_PETITION} row reads {:?}, which does not contain \
                 {CHARTER_LABEL_HINT:?}; refusing to select something that may not be the charter \
                 line",
                opt.message
            ),
        );
        probe.phase = Phase::Done;
        return;
    }
    let _ = net.0.send(ClientCommand::GossipSelectOption {
        guid: npc,
        option: opt.index,
    });
    info!(
        "PROBE_CHARTER: 4 (registrar) GossipSelectOption({npc:#x}, wire index {}) sent for {:?}",
        opt.index, opt.message
    );
    probe.charter_row = Some(opt.index);
    probe.phase = Phase::Registrar { since: now };
}
