//! The guild-charter live probe (`WOW_PROBE_CHARTER=1`) — decision 1672's end-to-end instrument:
//! log in, GM-hop to the Stormwind guild registrar, open his gossip menu on the real wire, assert
//! the charter row's icon reads **petition**, select it by its wire index, buy a charter through
//! the registrar's own window, right-click the charter in the bags the way a player does, watch the
//! petition window fill in a round trip later, rename it, and destroy it again so the next run can
//! do the same. One `PROBE_CHARTER: <step> PASS/FAIL/SKIP <detail>` line per step, then a final
//! `PROBE_CHARTER: DONE pass=<n> fail=<m>`. Modeled closely on [`super::probe_binder`] — same phase
//! machine, same trace style, same self-terminating exit ([`super::probes::ProbeExitPlugin`]'s
//! pattern), same live-VM observation idiom (`script.eval` against the real UI VM) — with
//! [`super::probe_clam`]'s bag-click leg (`UseContainerItem` through the live VM, so the real
//! dispatcher runs) and its watch-the-cleanup-land exit.
//!
//! **Unit tests cannot reach any of this.** Every claim the charter slice rests on is about what a
//! real server sends and in what order: a packet that had no const and no parse arm until 1672
//! landed, a window that deliberately opens *empty* and fills a round trip later, and an item-use
//! fork arm whose whole evidence is "no other arm accepts a charter". Those are only visible
//! against real bytes.
//!
//! ## The registrar (live-DB verified against the local vmangos, `mangos` DB)
//!
//! Aldwin Laughlin, the Stormwind guild registrar — `creature_template.entry = 4974`, spawn
//! `creature.guid = 79681`, **map 0**, position `(-8885.25, 614.395, 95.2576)`,
//! `creature_template.npc_flags = 1537` = `0x601` = GOSSIP | **PETITIONER (0x200)** |
//! TABARDDESIGNER (0x400), `gossip_menu_id = 708`. Both of the top two bits matter: vmangos's
//! buy handler fetches the NPC with `GetNPCIfCanInteractWith(guid, UNIT_NPC_FLAG_PETITIONER)` and
//! then *also* refuses anything that is not `IsTabardDesigner()`
//! (`Handlers/PetitionsHandler.cpp:44-52`), so a petitioner without the tabard bit sells nothing.
//!
//! **`UNIT_NPC_FLAG_PETITIONER` is `0x200`** (vmangos `Objects/UnitDefines.h:666`). The probe
//! reuses [`crate::target::cursor_mode::npc_flags::PETITIONER`] rather than keeping a second copy
//! — a duplicated flag table is exactly how B249's icon map went stale.
//!
//! ## The charter row (live-DB verified this session)
//!
//! `gossip_menu_option` for menu 708 carries **exactly two rows, both unconditional**
//! (`condition_id = 0`), so a GM probe and a player see the same two — unlike the innkeeper menu
//! [`super::probe_binder`] walks, where GM mode adds holiday rows:
//!
//! | wire index | `option_icon` | `option_id`                    | text                            |
//! |------------|---------------|--------------------------------|---------------------------------|
//! | 0          | **7**         | 10 (`GOSSIP_OPTION_PETITIONER`)| *"How do I form a guild?"*      |
//! | 1          | 8             | 11 (tabard designer)           | *"I want to create a guild crest."* |
//!
//! Icon byte **7** is `"petition"` in [`crate::ui_gossip`]'s `GOSSIP_ICON_TYPES` — the client's own
//! `0x84b7ac` table, byte-verified in decision 1335. The probe finds the row **by that icon byte
//! and selects by the row's wire index, never by list position**: that is the lesson
//! [`super::probe_binder`]'s header records, and it is why this probe survives a menu that grows a
//! row. The label guard is a lowercase substring for the same file's other lesson — vmangos
//! prefers `option_broadcast_text` (3413) over the `option_text` column, so an equality test would
//! be asserting which of two copies of the same sentence the server happened to pick.
//!
//! ## The charter item and its price (live-DB + vmangos source, verified this session)
//!
//! Entry **5863** "Guild Charter", `display_id = 16161`, `flags = 8192` (`0x2000` =
//! `ITEM_FLAG_CHARTER`), `max_count = 1`, `inventory_type = 0`, `start_quest = 0`, `page_text = 0`,
//! **no ON_USE spell** and not LOOTABLE. That combination is the whole of
//! [`crate::ui_items::ItemUseRoute::ShowPetition`]'s evidence: every other arm of the item-use fork
//! declines a charter, so before 1672 a right-click reached `Nothing` and sent nothing at all.
//!
//! The price is **1000 copper** (10 silver) — vmangos `GUILD_CHARTER_COST`
//! (`Handlers/PetitionsHandler.cpp:39`), which arrives in `SMSG_PETITION_SHOWLIST`'s `charterCost`
//! field and is what `GetGuildCharterCost()` must quote.
//!
//! `MAX_CHARTER_NAME` is **24** UTF-8 characters (`ObjectMgr.h:401`, enforced by
//! `ObjectMgr::IsValidCharterName`, which allows digits and spaces —
//! `isValidString(…, numericOrSpace = true)`). The probe's names are 22 characters.
//!
//! ## What each step can and cannot conclude
//!
//! | step | what a FAIL there means | `ui_petition` claim it bears on |
//! |------|-------------------------|---------------------------------|
//! | 0 | the probe left a guild behind and `GuildLeave`/`GuildDisband` did not take — every later step is meaningless, because a buy is refused **silently** while guilded | — (re-runnability) |
//! | 1 | environmental: the `.go` was refused or Stormwind never streamed — SKIP, not a defect | — |
//! | 2 | the gossip wire or the B292 text-query hold is broken for this NPC | — |
//! | 3 | the icon table regressed one row over from B249: byte 7 must read `"petition"` | — |
//! | 4 | `SMSG_PETITION_SHOWLIST` is not parsed, does not fire `GUILD_REGISTRAR_SHOW`, or its `charterCost` is not what `GetGuildCharterCost()` reads | **INFERRED #1**, and `script::petition`'s "what `GetGuildCharterCost` reads" |
//! | 5 | the buy never reached the server, or was refused — there is **no confirmation packet at all**, so the item arriving *is* the answer | — |
//! | 6 | using a charter does not open the petition window: either the fork arm or `SMSG_PETITION_SHOW_SIGNATURES` → `PETITION_SHOW` | **INFERRED #6**, and **#2** |
//! | 7 | the lazy record fill is broken — the window would sit titleless forever, which is the whole two-caches design | `script::petition`'s "the repaint path" |
//! | 8 | `MSG_PETITION_RENAME`'s echo does not patch the cached record (the echo is sent **only on success**) | — |
//! | 9 | the charter is still in the bags, and the **next** run's buy will be refused silently for "the owner already has one" (`PetitionsHandler.cpp:70`) — a failure that looks nothing like its cause | — (re-runnability) |
//!
//! Three of `ui_petition`'s six INFERRED claims are **not** touched here, and none of the three is
//! an oversight. #4 (re-asking for the signature list on an `OK` sign result) needs a second
//! account to sign, and #5 (`ERR_GUILD_FOUNDER_S` on a successful turn-in) needs nine of them —
//! neither is reachable from one client, so the whole sign/offer/turn-in half of the family
//! (`SignPetition`, `OfferPetition`, `TurnInGuildCharter`, `MSG_PETITION_DECLINE`) still has no
//! live coverage after this probe. #3 (the two closes send nothing) is *superseded*: closing a
//! charter you do NOT own can send `MSG_PETITION_DECLINE` (`0x4f3f60`), so this probe deliberately
//! asserts nothing about either close rather than pinning a claim that is being rewritten.
//!
//! ## The run recipe
//!
//! ```text
//! WOW_NOSOUND=1 WOW_USER=probe0 WOW_PASS=pprobe0 WOW_CHAR=Probezero \
//!     WOW_PROBE_CHARTER=1 cargo run -q -p benilla
//! ```
//! (the checkout's probe identity, the `probe` skill. **Never a player's account** — a login on it kicks
//! the director's live session.) `WOW_NOSOUND=1` because an unattended probe must not play zone
//! music into the director's room; `caffeinate -dis` is **not** needed for a run this short — the
//! whole sequence is a handful of round trips and finishes in well under a minute.
//!
//! Non-combat: this probe never fights and never targets anything. GM mode is left exactly as
//! found. An outer `timeout` + grep on `PROBE_CHARTER:` is the whole harness; the probe self-exits
//! once DONE, and every wait is bounded — a hung leg FAILs with a legible detail rather than
//! hanging the run.

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

/// Aldwin Laughlin's spawn (vmangos `creature` guid 79681, entry 4974) — the `.go xyz` target.
const REGISTRAR_AT: [f32; 3] = [-8885.25, 614.395, 95.2576];
/// His map — Eastern Kingdoms. `.go xyz` takes the map id as its fourth argument.
const REGISTRAR_MAP: u32 = 0;
/// His creature template entry — the streamed-unit identity check (module doc). The `0x200` npc
/// flag is only the fallback, exactly as the binder probe splits it.
const REGISTRAR_ENTRY: u32 = 4974;
/// The wire `GOSSIP_ICON` byte menu 708's charter row sends (module doc) — decision 1335's table
/// indexes it to [`ICON_TYPE_PETITION`].
const ICON_PETITION: u8 = 7;
/// The type string [`crate::ui_gossip`]'s table must produce for [`ICON_PETITION`].
const ICON_TYPE_PETITION: &str = "petition";
/// What a broken icon table produces instead — the chat bubble. Seeing it back here is B249's
/// regression one row over, not a flake.
const ICON_TYPE_REGRESSION: &str = "gossip";
/// The substring the charter row's label must carry before the probe is willing to select it. A
/// lowercase *substring*, never an equality: the label on the wire is the row's
/// `option_broadcast_text` (3413) rather than its `option_text` column, and the exact wording is
/// the server's to choose (module doc).
const CHARTER_LABEL_HINT: &str = "guild";
/// Scan radius around the `.go` landing, generously wide so a slightly-off hop still finds him
/// (the bank/mail/binder probes' shared idiom).
const SCAN_RANGE: f32 = 12.0;
/// The price `GetGuildCharterCost()` must quote, in copper — vmangos `GUILD_CHARTER_COST`
/// (`PetitionsHandler.cpp:39`), carried in `SMSG_PETITION_SHOWLIST`'s `charterCost`.
const CHARTER_COST_COPPER: i64 = 1000;
/// What `GetPetitionInfo()`'s `maxSignatures` must read once the record lands — vmangos answers
/// `SMSG_PETITION_QUERY_RESPONSE` with `minSignatures = maxSignatures = 9`, hardcoded
/// (`PetitionsHandler.cpp:182-183`), whatever `MinPetitionSigns` is configured to.
const REQUIRED_SIGNATURES: i64 = 9;
/// A fresh charter has **no** signatures: `GetNumPetitionNames()` counts signers, and the owner is
/// not one of them (the reference paints the owner into its own font string).
const FRESH_SIGNATURES: i64 = 0;
/// Copper handed to the probe body up front so the buy can never fail for funds. `.modify money`
/// is `SEC_BASIC_ADMIN` (4) in vmangos's `Chat.cpp` command table and every `probeN` account is
/// gmlevel **6** (docs/METHOD.md, decision 0651), so it lands; with no selection it targets the sender
/// (`ChatHandler::GetSelectedPlayer`, `Chat.cpp:2601-2612`), which is why it is sent before the
/// probe touches an NPC.
const FUND_COPPER: u32 = 100_000;

/// Settle after the `.go` before scanning — the hop's own travel plus a frame for the stream.
const SETTLE_SECS: f64 = 3.0;
/// The waits are deliberately generous, for the reason [`super::probe_binder`] measured: a `.go`
/// can land inside a terrain load, and the probe then gets roughly one frame per second to poll
/// in. A timeout tight enough to trip on that would report a FAIL about the wire, which is the one
/// thing an instrument must never do.
const GUILD_CLEAR_TIMEOUT_SECS: f64 = 20.0;
const SCAN_TIMEOUT_SECS: f64 = 25.0;
const MENU_TIMEOUT_SECS: f64 = 20.0;
/// How long step 4 will wait for the portrait booth's `"npc"` token before reporting it empty.
///
/// Not a gate — the step passes either way. It exists so the detail line says something true: the
/// booth is fed by a system that is deliberately unordered against the apply pass, so the token is
/// `None` on the frame the window opens and resolves on the next.
const REGISTRAR_TOKEN_SETTLE_SECS: f64 = 1.0;

const REGISTRAR_TIMEOUT_SECS: f64 = 15.0;
/// The buy's wait is the longest of the action legs: nothing acknowledges it, so this is the time
/// the *item* gets to arrive and its template with it.
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

/// The probe's phase machine plus the identities discovered along the way (the binder/bank probes'
/// shape: a `Copy` phase snapshotted out of the resource each tick, so an arm can mutate `probe`
/// freely).
#[derive(Resource, Default)]
struct CharterProbe {
    phase: Phase,
    /// The registrar's guid, once streamed in.
    registrar: Option<u64>,
    /// The charter row's **wire** `index` — the value the packet carried and the value
    /// `CMSG_GOSSIP_SELECT_OPTION` must echo back. vmangos numbers them over the rows it actually
    /// sends (`GossipDef.cpp:188`), so it is the row's 0-based position in *this* menu — neither
    /// the DB's `gossip_menu_option.id` nor the Lua menu's 1-based position, and the probe assumes
    /// no relation between them for the same reason the real drain doesn't.
    charter_row: Option<u32>,
    /// Where the bought charter is: the wire `(bag_index, slot)` pair and the instance guid, from
    /// [`find_item`]. The Lua click position is derived from it by [`lua_bag_pos`].
    charter: Option<(u8, u8, u64)>,
    /// The guild name bought in step 5 — what step 7 waits for the title to become.
    bought: String,
    /// The name step 8 renames to — what step 8 waits for the title to become.
    renamed: String,
    /// The title read at the instant the petition window first became visible (step 6). Step 7
    /// asserts it was either empty (the packet carries no text) or already filled.
    title_at_open: String,
    passes: u32,
    fails: u32,
    /// Latched once [`Phase::Done`] has fired its exit (never re-fire on a later frame).
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
    /// Step 0 — the money is sent and any guild an earlier run founded is being left. `sent`
    /// distinguishes "haven't asked yet" from "asked, waiting for the descriptor to clear".
    Unguild {
        since: f64,
        sent: bool,
    },
    /// Step 1 — `.go` issued; settling before the world streams the registrar in.
    Settling {
        sent_at: f64,
    },
    /// Step 2 — `GossipHello` sent; waiting for the parsed menu AND its push into the VM.
    Menu {
        sent_at: f64,
    },
    /// Step 4 — the charter row selected; waiting for `SMSG_PETITION_SHOWLIST` to reach
    /// [`GuildRegistrarState`] and `GUILD_REGISTRAR_SHOW` to open the window.
    Registrar {
        since: f64,
    },
    /// Step 5 — the registrar's purchase panel driven; waiting for the charter item to arrive.
    Buying {
        since: f64,
        sent: bool,
    },
    /// Step 6 — the charter right-clicked through the live VM; waiting for the petition window.
    Opening {
        since: f64,
        sent: bool,
    },
    /// Step 7 — waiting for `SMSG_PETITION_QUERY_RESPONSE` to fill the title in.
    Record {
        since: f64,
    },
    /// Step 8 — `RenamePetition` run in the live VM; waiting for the echo to patch the record.
    Renaming {
        since: f64,
        sent: bool,
    },
    /// Step 9 — the charter destroyed; watching it actually leave the bags before exiting.
    Destroying {
        since: f64,
        sent: bool,
    },
    Done,
}

// ── The live-VM readings ─────────────────────────────────────────────────────────────────────
// Every one of these answers a default on an eval hiccup and is treated as "nothing observed
// yet", never a panic (the bank/binder probes' idiom): the probe's own timeouts are what turn a
// persistently missing reading into a verdict.

/// The `CHAT_MSG_SYSTEM` / `UI_ERROR_MESSAGE` lines seen since the hook went in, newest last.
///
/// This is the probe's window into the family's **silent** refusals: a buy or a rename that the
/// server turns down comes back as `SMSG_GUILD_COMMAND_RESULT` on the *guild* family's channel,
/// which [`crate::ui_guild::lines`] prints as a system line and nothing else records. Without it a
/// refused buy and a lost packet read identically.
fn probe_lines(script: &UiScript) -> Vec<String> {
    script
        .eval::<Vec<String>>("return ProbeCharterLines or {}")
        .unwrap_or_default()
}

/// How many values the live `GetGossipOptions()` returns — flat `(label, type)` pairs, so twice the
/// row count. The probe waits on this rather than assuming the feed already ran this frame, and it
/// is also what makes the wait honour [`crate::ui_gossip`]'s B292 hold: on the first visit to a
/// `text_id` the menu deliberately does not reach the VM until `SMSG_NPC_TEXT_UPDATE` lands.
fn vm_gossip_values(script: &UiScript) -> i64 {
    script
        .eval::<i64>("local t = { GetGossipOptions() } return table.getn(t)")
        .unwrap_or(0)
}

/// The icon **type string** the app mapped for the 1-based menu row `pos` — read exactly where the
/// FrameXML reads it, out of the pushed snapshot through the Era `GetGossipOptions()` vararg.
fn vm_icon_type(script: &UiScript, pos: usize) -> String {
    script
        .eval::<String>(&format!(
            "local t = {{ GetGossipOptions() }} return t[{}] or \"\"",
            pos * 2
        ))
        .unwrap_or_default()
}

/// The texture path `BENILLA_GOSSIP_ICONS.petition` resolves to in the live VM — `""` if the table
/// or the key is missing, which would mean the row draws the fallback bubble whatever the app
/// mapped ([`super::probe_binder`]'s second half of the same assert).
/// The charter's own bag tooltip, line by line — `SetBagItem` on the slot the buy landed in, read
/// back out of the live VM.
///
/// Reported in step 8's detail rather than asserted, for §7's reason: the wording and the ORDER are
/// a unit test's job, and what a live run adds is only that the petition record travels from the
/// query response into the bag slot's pushed view at all.
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

fn vm_petition_texture(script: &UiScript) -> String {
    script
        .eval::<String>("return (BENILLA_GOSSIP_ICONS and BENILLA_GOSSIP_ICONS.petition) or \"\"")
        .unwrap_or_default()
}

/// Is a named frame up, asked of the live UI rather than of our own state — the window is what a
/// player looks for. Nil-guarded on the global so a build without the file reads `false` instead of
/// raising.
fn vm_visible(script: &UiScript, frame: &str) -> bool {
    script
        .eval::<bool>(&format!(
            "return ({frame} and {frame}:IsVisible()) and 1 or nil"
        ))
        .unwrap_or(false)
}

/// `GetGuildCharterCost()` — the registrar's price in copper.
fn vm_charter_cost(script: &UiScript) -> i64 {
    script
        .eval::<i64>("return GetGuildCharterCost()")
        .unwrap_or(-1)
}

/// One of `GetPetitionInfo()`'s six returns, by 1-based position, as a string. With nothing open
/// the getter returns **no values at all**, so every slot reads `""`.
fn vm_petition_str(script: &UiScript, slot: usize) -> String {
    let binds = "_,".repeat(slot - 1);
    script
        .eval::<String>(&format!(
            "local {binds}v = GetPetitionInfo() return v or \"\""
        ))
        .unwrap_or_default()
}

/// `GetPetitionInfo()`'s fourth return — the signature requirement off the wire.
fn vm_petition_max(script: &UiScript) -> i64 {
    script
        .eval::<i64>("local _,_,_,v = GetPetitionInfo() return v or -1")
        .unwrap_or(-1)
}

/// `GetPetitionInfo()`'s sixth return — `isOriginator`, an Era `1`/`nil` (never `true`/`false`).
fn vm_is_originator(script: &UiScript) -> i64 {
    script
        .eval::<i64>("local _,_,_,_,_,v = GetPetitionInfo() return v or 0")
        .unwrap_or(0)
}

/// `GetNumPetitionNames()` — the signers, which never include the owner.
fn vm_num_names(script: &UiScript) -> i64 {
    script
        .eval::<i64>("return GetNumPetitionNames()")
        .unwrap_or(-1)
}

/// The inverse of [`crate::ui_items::wire_pos`] for the two regions a **bag right-click** can
/// address: the backpack (Lua container `0`) and the four equipped bags (`1..=4`).
///
/// `None` for anywhere else — the bank, the keyring, a doll slot. A bought charter cannot land in
/// any of them, so a `None` here is the probe refusing to click a slot it cannot name rather than
/// guessing one.
fn lua_bag_pos(bag_index: u8, slot: u8) -> Option<(i64, u32)> {
    if bag_index == BAG_PLAYER_INVENTORY {
        let inner = slot.checked_sub(SLOT_PACK_FIRST)?;
        (inner < PACK_SLOTS).then_some((0, u32::from(inner) + 1))
    } else {
        let bag = bag_index.checked_sub(SLOT_BAG_FIRST)?;
        (bag < 4 && slot < 36).then_some((i64::from(bag) + 1, u32::from(slot) + 1))
    }
}

/// A guild name that cannot collide with a previous run or a real guild, and cannot be refused for
/// length: a 13-character prefix, a space, and the low 8 digits of the wall clock — 22 characters,
/// inside vmangos's `MAX_CHARTER_NAME` of 24. Digits and spaces are both allowed by
/// `IsValidCharterName` (`isValidString(…, numericOrSpace = true)`, verified this session).
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
        return; // no UI VM this build (headless net-only) — nothing this probe can drive
    };
    let now = time.elapsed_secs_f64();
    let phase = probe.phase;

    match phase {
        Phase::Wait => {
            // The refusal channel, installed up front so it is live long before the first verb.
            // Both events land on the same list because the probe only ever reads it as "what did
            // the client say while this leg ran".
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
            // Money first, before anything is targeted: `.modify money` with no selection targets
            // the sender, and with a *creature* selected it answers "no character selected".
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
        // ── Step 0 — the preconditions ──────────────────────────────────────────────────────
        // A buy is refused **silently** while the buyer is in a guild (`PetitionsHandler.cpp:66`,
        // a bare `return`), so a run that founded one and never left it would make step 5 fail
        // for a reason with no trace anywhere. Leaving is therefore a precondition, not tidiness,
        // and it is what makes this probe re-runnable rather than a one-shot.
        //
        // A FAIL here means `GuildLeave()`/`GuildDisband()` did not take: the descriptor's
        // `PLAYER_GUILDID` never came back to 0, which is the same field `IsInGuild()` reads and
        // the same edge that fires `PLAYER_GUILD_UPDATE`. Every step below it would be meaningless.
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
                // Disband if we are the master (a guild master cannot simply leave — vmangos
                // answers that with `ERR_GUILD_LEADER_LEAVE_S`), else leave. The verb runs through
                // the live VM's own globals so the whole engine→drain→wire chain is exercised, but
                // the *decision* is read straight off the descriptor rather than out of
                // `IsGuildLeader()`: rank 0 IS the guild master, `PLAYER_GUILDRANK` is mirrored
                // into the model by a feed this probe is not ordered against, and on the first
                // frame in-world that one frame of lag would pick the wrong verb.
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
        // ── Step 1 — the hop ────────────────────────────────────────────────────────────────
        // A SKIP here is environmental and says so: the `.go` may have been refused, or Stormwind
        // may never have streamed. It is not evidence about the charter flow either way.
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
        // ── Step 2 — the menu ───────────────────────────────────────────────────────────────
        // A FAIL means the gossip wire is broken for this NPC. The wait is for the menu **in the
        // VM**, not for the packet: `ui_gossip`'s B292 hold deliberately keeps a first visit's
        // menu closed until `SMSG_NPC_TEXT_UPDATE` answers the greeting query, so a wait on the
        // parse alone would race the hold and read a menu the player cannot see.
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
        // ── Step 4's second half — the window ───────────────────────────────────────────────
        // **The load-bearing step.** Before decision 1672 `SMSG_PETITION_SHOWLIST` had no const
        // and no parse arm: it fell through `parse.rs`'s tail into `ServerPacket::Other`, and
        // clicking "How do I form a guild?" closed the gossip window and did nothing else. A FAIL
        // here is exactly that shape, and it is the evidence for `ui_petition`'s INFERRED #1 —
        // that this packet is what fires `GUILD_REGISTRAR_SHOW`. The cost reading is the second
        // half: `GetGuildCharterCost` is modelled as the showlist row's `charterCost` in copper,
        // and 1000 is the only number vmangos can send.
        Phase::Registrar { since } => {
            let Some(npc) = probe.registrar else {
                probe.phase = Phase::Done;
                return;
            };
            let parked = registrar.npc() == Some(npc);
            let visible = vm_visible(&script, "GuildRegistrarFrame");
            let cost = vm_charter_cost(&script);
            if parked && visible && cost == CHARTER_COST_COPPER {
                // The portrait booth's `"npc"` token is reported, never asserted: it is what
                // paints the window's face and name banner, and a blank one is a look question,
                // which is the director's to call and not a probe's.
                //
                // **Sampled one frame late, on purpose.** `feed_interact_npc` runs in
                // `WorldStage::Net` deliberately unordered against the apply pass (its own doc says
                // so: a session is open for seconds, so one frame either way is invisible), which
                // means on the very frame the registrar opens `InteractNpc` is still `None` and
                // this token reads `""` every single time. A detail line that is always empty is an
                // instrument that lies — it reads exactly like the black-disc bug this arm was
                // added to fix.
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
                // The window opened, so the packet parsed — this is a value bug, not a wire one,
                // and saying which saves the next reader the bisect.
                probe.fail(
                    4,
                    "registrar",
                    format!(
                        "the registrar opened on {npc:#x} but GetGuildCharterCost() reads {cost}, \
                         not {CHARTER_COST_COPPER} (vmangos GUILD_CHARTER_COST, \
                         PetitionsHandler.cpp:39) — the showlist row's charterCost is not what \
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
                         Before decision 1672 SMSG_PETITION_SHOWLIST had no parse arm at all and \
                         fell through to ServerPacket::Other — that is what this reading looks \
                         like. Lines seen: {:?}",
                        registrar.npc(),
                        probe_lines(&script)
                    ),
                );
                probe.phase = Phase::Done;
            }
        }
        // ── Step 5 — the buy ────────────────────────────────────────────────────────────────
        // Driven through the registrar's own window: `GuildRegistrar_ShowPurchaseFrame()` is the
        // reference's pure local panel swap (and the only place the price is painted), then
        // `BuyGuildCharter(name)` is the Purchase button's own send. The real button also hides
        // the window afterwards (`GuildRegistrar_Purchase`); the probe deliberately does not, so
        // the registrar session stays observable — and it costs nothing, because step 6's
        // `ShowUIPanel(PetitionFrame)` displaces the left-slot incumbent anyway
        // (`SeatLeftAreaPanel`'s two-pushable-0 arm).
        //
        // **There is no confirmation packet at all for a successful buy** — the item arriving IS
        // the answer (`PetitionsHandler.cpp:130` pushes it and nothing else) — so this is a poll
        // with a timeout, and a FAIL means either the send never happened or the server refused.
        // The four silent refusals are: already in a guild, already owns a petition, the name is
        // taken, or not enough money; the name-taken one is the only one that says anything, and
        // it says it on the guild family's channel, which is why the detail dumps the lines.
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
            // The template has to have landed too, and it is folded into the SAME `Option` as the
            // item rather than short-circuiting on its own: the click dispatcher's charter arm is
            // a **template flag** test (`ITEM_FLAG_CHARTER`), so a click made before the answer
            // arrives falls through to a plain use and proves nothing — but an early `return` on a
            // template that never answers would sail straight past the timeout below and hang the
            // run, which is the one thing an instrument may never do. Reading the flags here also
            // cross-checks the DB fact (`flags = 8192`) against what the live server sends.
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
                // Nothing usable arrived, but `find_item` may still have seen a charter — hand it
                // to the cleanup rather than to `Done`, or the next run's buy is refused silently.
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
        // ── Step 6 — the item-use fork ──────────────────────────────────────────────────────
        // Driven as a bag right-click through the live VM's own `UseContainerItem`, so
        // `ui_items::drain::drain_container_uses` runs its real dispatcher and reaches
        // `ItemUseRoute::ShowPetition` — **not** a synthesized `CMSG_PETITION_SHOW_SIGNATURES`.
        // That distinction is the whole point of this step: the arm's position in the fork is
        // INFERRED (`ui_petition`'s #6), and the only thing that can exercise it is a real click.
        //
        // A FAIL means one of two things, and the detail says which it cannot separate: either
        // the fork never reached the charter arm (the charter fell through to `Nothing`, which is
        // exactly the pre-1672 behaviour — right-click a charter, nothing happens at all), or the
        // answer never became a window (INFERRED #2, `SMSG_PETITION_SHOW_SIGNATURES` →
        // `PETITION_SHOW`).
        //
        // **What the window's identity rests on is the control, not a field.** Every reading here
        // is the live VM's, and none of the charter getters exposes the open charter's ITEM guid,
        // so "the window is on the charter we just bought" is established by the BEFORE control
        // (no petition window existed before the click) plus the click itself, rather than by
        // matching guids. `PetitionState::open_item` would give the direct form in one term; it is
        // private to `ui_petition`, and widening it is that module's call to make, not this
        // probe's.
        Phase::Opening { since, sent } => {
            let Some((bag_index, slot, guid)) = probe.charter else {
                probe.phase = Phase::Done;
                return;
            };
            if !sent {
                // The control, the clam probe's first reading: a window that was already up would
                // make everything below it meaningless.
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
                         falls through to `Nothing` sends nothing at all — the pre-1672 \
                         behaviour) or the answer never became a window. Lines seen: {:?}",
                        probe_lines(&script)
                    ),
                );
                probe.phase = Phase::Destroying {
                    since: now,
                    sent: false,
                };
            }
        }
        // ── Step 7 — the lazy record fill ───────────────────────────────────────────────────
        // The window opens with an EMPTY title: `SMSG_PETITION_SHOW_SIGNATURES` carries an item
        // guid, an owner guid, a petition id and signer guids, and no text at all. The name
        // arrives a round trip later on `SMSG_PETITION_QUERY_RESPONSE`, keyed by petition id, and
        // the feed re-fires `PETITION_SHOW` so the window repaints in place.
        //
        // A PASS here is the whole two-caches design observed working end to end. A FAIL means the
        // ask-once record query never went out, its answer never landed, or the repaint edge never
        // fired — in every one of which the window sits titleless forever, which is what a player
        // would report as "the charter has no name".
        Phase::Record { since } => {
            let title = vm_petition_str(&script, 2);
            let max = vm_petition_max(&script);
            if title == probe.bought && max == REQUIRED_SIGNATURES {
                // The first half of the claim, checked against what step 6 latched: the title at
                // open was either empty (the packet carries none) or already the bought name (the
                // record beat the paint). Anything else is a title from somewhere it cannot have
                // come from.
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
        // ── Step 8 — the rename echo ────────────────────────────────────────────────────────
        // `RenamePetition(name)` in the live VM — the `RENAME_GUILD` popup's own Accept. vmangos
        // sends `MSG_PETITION_RENAME` back **only on success** (`PetitionsHandler.cpp:209-215`,
        // inside `if (petition->Rename(...))`), so the title changing IS the echo arriving; a
        // refusal is silent apart from a guild-command result on the other channel.
        //
        // A FAIL means the echo never arrived or never patched the cached record — the window
        // would keep showing the old name indefinitely, since nothing re-queries.
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
            // The bag tooltip has to reach the SAME name, not merely be non-empty. It is fed by a
            // different path — the container snapshot, whose rebuild is gated (decision 1439) —
            // so the two can disagree, and requiring convergence is what proves the rename's
            // record patch reaches the gate rather than only the window. Held inside the step's
            // own timeout: a tooltip that never catches up FAILs here rather than being reported
            // as a curiosity.
            let tip = charter_tooltip_lines(
                &script,
                probe.charter.and_then(|(b, sl, _)| lua_bag_pos(b, sl)),
            );
            let tip_caught_up = tip.iter().any(|l| l.contains(&probe.renamed));
            if title == probe.renamed && tip_caught_up {
                let bought = probe.bought.clone();
                // The item TOOLTIP's line 3 while we are here — the charter's guild name and
                // master, which the director reported missing. Reported, not asserted: the plate's
                // wording and placement are pinned by the unit test
                // (`charter_lines_sit_between_the_name_and_the_signable_line`); what only a live
                // run can show is that the petition record actually reaches the bag slot's view,
                // which is a different question from whether the renderer would print it.
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
        // ── Step 9 — the cleanup that makes the probe re-runnable ───────────────────────────
        // `CMSG_DESTROYITEM` on the charter's own wire slot. vmangos cascades that into deleting
        // the petition (`Player.cpp:10811-10817`: `if (pItem->IsCharter())` → `DeletePetition`),
        // which is the only reason destroying the item is enough.
        //
        // **Without this, the NEXT run's buy is refused silently** — "Cannot buy a petition if the
        // owner already has one", a bare `return` at `PetitionsHandler.cpp:70` — and step 5 would
        // fail for a reason that looks nothing like its cause: no packet, no error line, just an
        // item that never arrives. That is exactly the failure this step exists to prevent, and it
        // is why every exit from steps 5-8 routes through here rather than to `Done`.
        //
        // The send is watched to land, the clam probe's lesson: `AppExit` tears the net thread
        // down within a frame or two, so a fire-and-forget destroy written on the way out never
        // reaches the wire.
        Phase::Destroying { since, sent } => {
            let Some((bag_index, slot, guid)) = probe.charter else {
                probe.phase = Phase::Done; // nothing was bought — nothing to clean up
                return;
            };
            if !sent {
                // The position is read FRESH rather than reused from step 5's latch. `count: 0`
                // destroys whatever whole stack sits at the addressed position, so a stale pair
                // would destroy the wrong item; nothing is expected to move a charter, and that is
                // exactly the kind of expectation a destructive send must not rest on.
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
            // The probe self-exit pattern (`ProbeExitPlugin::fire_probe_exit`): a polite AppExit
            // plus a hard backstop thread, so a net/winit teardown hang can't leave a zombie
            // client holding the probe account.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_CHARTER: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}

/// Step 1's send, shared by both of step 0's exits so the hop is spelled once.
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

/// Steps 3 and 4's first half — the icon, then the click.
///
/// Split out of the phase match because it is one straight line of asserts with several exits, and
/// inlining it buried the phase machine's shape ([`super::probe_binder`]'s own split). Sets
/// `probe.phase` on every path.
///
/// **Step 3's FAIL is the icon-table regression, one row over from B249.** Byte 7 is `"petition"`
/// in the client's own `0x84b7ac` table (decision 1335); a `"gossip"` here means the row draws the
/// chat bubble, which is what the pre-1331 hand-written map did to the innkeeper's byte 5.
///
/// **Step 4's send is by WIRE INDEX**, read off the packet, never derived from where the row sits
/// in the list — the drain's own rule, and the reason a menu that grows a row cannot silently make
/// this probe click the tabard designer.
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
                 {ICON_TYPE_REGRESSION:?}, the chat bubble. That is B249's regression one row \
                 over: decision 1335's table must index byte 7 to {ICON_TYPE_PETITION:?}",
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
    // The type string being right is not the row drawing right: the XML looks the type up in
    // `BENILLA_GOSSIP_ICONS`, and a missing key falls back to the chat bubble with no error
    // anywhere. Both halves, exactly as the binder probe asserts them.
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

    // Step 4's click. Guarded on the label — a lowercase substring, never an equality, because the
    // wire's label is the row's broadcast text rather than its `option_text` column.
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
