//! The dialog engine's verbs, app half (decision 1963): the feeds behind the stock
//! `StaticPopup.lua` dialogs benilla never raised and the drains behind their buttons, each to
//! the reference's bytes unless a line says INFERRED.
//!
//! * **Pet trainer** — `SMSG_PET_UNLEARN_CONFIRM {guid, cost}` latches both and owes
//!   `CONFIRM_PET_UNLEARN(cost)`; `ConfirmPetUnlearn()` answers with `CMSG_PET_UNLEARN {guid}` unless
//!   the cost outruns the purse (`ERR_NOT_ENOUGH_MONEY`, nothing sent). The talent-wipe twin
//!   (`crate::ui_talent_wipe`), latch for latch, leash for leash: the trainer walking out of
//!   `INTERACT_DISTANCE` closes the question, which is what `CheckPetUntrainerDist()` polls.
//! * **Instance boot** — every `SMSG_RAID_GROUP_ONLY {delayMs, reason}` fires an event: a positive
//!   delay arms the deadline and `INSTANCE_BOOT_START`, zero clears it and `INSTANCE_BOOT_STOP`,
//!   and only the zero leg names reason 1/2 on screen. `GetInstanceBootTimeRemaining()` reads
//!   whole seconds off the deadline; nothing clears it but a zero packet.
//! * **Area spirit healer** — `SMSG_AREA_SPIRIT_HEALER_TIME {guid, ms}` arms the wave clock and
//!   fires `AREA_SPIRIT_HEALER_IN_RANGE` when the guid is the cached healer's; Accept sends
//!   `0x2E3` with that guid, Cancel is the cancel-aura of spell 2584 plus `_OUT_OF_RANGE`. **The
//!   cache has two writers** (2291), which is what made Accept reachable at all: the reference's
//!   per-frame proximity scan [`poll_area_spirit_healer`] (`0x4923b0` — ghost-gated, acquire at
//!   20 yd, retain to 22) and the SPIRITGUIDE click arm [`AreaSpiritHealer::click_guide`]
//!   (`0x5df950`, whose deliberate cache-bust makes a second click re-ask). Until 2291 this said
//!   "no writer yet", which was true when 1963 wrote it and false from the moment the scan landed.
//! * **Battlefield queue** — `SMSG_BATTLEFIELD_STATUS` fills one of three slots and fires
//!   `UPDATE_BATTLEFIELD_STATUS`; `AcceptBattlefieldPort(index, accept)` sends the slot's map id
//!   with the answer as one byte.
//! * **Meeting stone** (1974) — two globals: the queued area
//!   (`[0xb72038]`) and the cached status text (`[0xb7203c]`). `SMSG 0x295 {areaId, status}`
//!   latches the old area, stores the new one unconditionally, prints one of five chat lines by
//!   the status byte (with `0x4ca230`'s two asymmetries: status 0 names the OLD area and is
//!   silent when it has no row; status 1 is skipped entirely when the area did not change, names
//!   the NEW one with an `UNKNOWN` fallback, and plays the `HARDCODED Meeting Stone Join` visual
//!   on the player), then — on EVERY path, an out-of-range status included — rebuilds the text
//!   (`MEETINGSTONE_TOOLTIP` over the area's name or `UNKNOWN`, into a 256-byte buffer) and fires
//!   `MEETINGSTONE_CHANGED`. World enter resets the text to the bare `UNKNOWN` and sends the empty
//!   `CMSG 0x296` once per world session; world leave drops the text to none.
//!   `CancelMeetingStoneRequest()` sends `0x293` unless in a party led by someone else
//!   (`ERR_MEETING_STONE_NOT_LEADER`). The four display-only replies (`0x297/0x298/0x299/0x2BB`)
//!   are chat lines with no state; the status-1 arm also triggers the Meeting Stones tutorial
//!   (`crate::tutorial`, 1976). **JOINING** is the click's own leg (decision 2283): type 23's
//!   use slot is its own validator, not the shared `CMSG_GAMEOBJ_USE` sender — four client-side
//!   refusals ([`meeting_stone_join_refusal`]), then `CMSG 0x292 {u64 goGuid}`.

use std::time::Instant;

use benilla_protocol::messages::{BattlefieldStatus, MeetingStoneNotice};
use benilla_ui::script::{ScriptValue, UiScript};
use bevy::ecs::system::NonSendMut;
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::creature_anim::spell_visual::{meeting_stone_join_fx, SpellKitFx, SpellVisuals};
use crate::names::NameCache;

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfGuid, SelfPlayer};
use crate::ui_party::GroupState;
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

/// The pet trainer's pending question: the latch (`0xc4d7b0/b4`) and the cost (`0xc4d7b8`).
#[derive(Resource, Default)]
pub(crate) struct PetUnlearnState {
    npc: Option<u64>,
    cost: u32,
    ask: bool,
}

impl PetUnlearnState {
    /// The inbound `SMSG_PET_UNLEARN_CONFIRM`: latch and owe the dialog.
    pub(crate) fn ask(&mut self, npc: u64, cost: u32) {
        self.npc = Some(npc);
        self.cost = cost;
        self.ask = true;
    }

    fn pending(&self) -> Option<u64> {
        self.npc
    }
}

impl NpcSession for PetUnlearnState {
    fn npc(&self) -> Option<u64> {
        self.npc
    }
    fn close(&mut self) {
        self.npc = None;
        self.cost = 0;
        self.ask = false;
    }
}

/// The instance-boot clock (`[0xb4e34c]`), and what the last packet owes the UI.
#[derive(Resource, Default)]
pub(crate) struct InstanceBoot {
    deadline: Option<Instant>,
    /// Events owed, in arrival order (`INSTANCE_BOOT_START` / `_STOP`).
    events: Vec<&'static str>,
    /// Error lines owed (`ERR_RAID_GROUP_ONLY` / `_FULL`), the zero-delay leg's.
    errors: Vec<&'static str>,
}

impl InstanceBoot {
    /// `SMSG_RAID_GROUP_ONLY`: `delay > 0` arms, else clears — and the event fires either way.
    pub(crate) fn apply(&mut self, delay_ms: u32, reason: u32, now: Instant) {
        if delay_ms > 0 {
            self.deadline = Some(now + std::time::Duration::from_millis(u64::from(delay_ms)));
            self.events.push("INSTANCE_BOOT_START");
        } else {
            self.deadline = None;
            self.events.push("INSTANCE_BOOT_STOP");
            match reason {
                1 => self.errors.push("ERR_RAID_GROUP_ONLY"),
                2 => self.errors.push("ERR_RAID_GROUP_FULL"),
                _ => {}
            }
        }
    }

    /// Whole seconds left, 0 when idle or past — the reference's unsigned divide of a clamped
    /// millisecond remainder.
    pub(crate) fn secs(&self, now: Instant) -> u32 {
        self.deadline
            .map(|d| d.saturating_duration_since(now).as_secs())
            .map_or(0, |s| u32::try_from(s).unwrap_or(u32::MAX))
    }
}

/// `SPIRITGUIDE` — `UNIT_NPC_FLAGS` bit 6, the flag the acquire callback `0x4924c0` tests at
/// `0x4924fc shr eax,0x6; test al,1`.
const NPC_FLAG_SPIRITGUIDE: u32 = 1 << 6;

/// The area spirit healer's aura, `0xA18` = 2584 — the one spell `0x4921c0`'s cancel leg and
/// `CancelAreaSpiritHeal` both name, and the only spell id `0x6e7040` fires
/// `AREA_SPIRIT_HEALER_OUT_OF_RANGE` for (`0x6e70b6 cmp esi,0xa18`).
pub(crate) const AREA_SPIRIT_HEALER_AURA: u32 = 2584;

/// The area spirit healer's **acquire** radius — the `.rdata` f32 `[0x8044d0] = 20.0`, compared
/// squared in the enumerate callback `0x4924c0`.
const SPIRIT_GUIDE_ACQUIRE_YD: f32 = 20.0;
/// The **retain** radius: the same f32 times `[0x804580] = 1.1`, i.e. 22.0 (`0x492406`). A healer
/// is adopted inside 20 yd and kept until 22 — the hysteresis is the reference's, and without it
/// a body standing on the boundary would send a query every frame it jittered across.
const SPIRIT_GUIDE_RETAIN_YD: f32 = SPIRIT_GUIDE_ACQUIRE_YD * 1.1;

/// What [`AreaSpiritHealer::set_healer`] owes the rest of the frame.
///
/// The reference does both of these inside `0x4921c0` itself; here the resource is a plain data
/// type with no access to the VM or the socket, so it *reports* them and the systems pay them.
#[derive(Default, PartialEq, Eq, Debug)]
pub(crate) struct SetHealerOutcome {
    /// A wave deadline was pending and the healer changed, so `0x4921fc mov ecx,0xa18;
    /// call 0x6e7040` ran: `AREA_SPIRIT_HEALER_OUT_OF_RANGE` fires and `CMSG_CANCEL_AURA(2584)`
    /// goes out. **This is how walking away from a graveyard closes the wave dialog** — no Lua
    /// call is involved, which is why the event has no argument and no caller.
    pub(crate) cancel_aura: bool,
    /// The newly adopted healer, if the change landed on a non-zero guid: `CMSG 0x2E2` with it.
    pub(crate) query: Option<u64>,
}

/// The current-area spirit healer (`[0xb4e330/334]`) and its wave clock (`[0xb4e338]`).
///
/// **The writer is [`Self::set_healer`], and it is the reference's `0x4921c0` to the branch.**
/// 1963 shipped this resource with the note "no writer yet"; the reference has the whole acquire
/// trio: the setter, the poll `0x4923b0` with its radii, and the click arm `0x5df950`. Both roads
/// in are built here.
#[derive(Resource, Default)]
pub(crate) struct AreaSpiritHealer {
    /// The cached healer (`[0xb4e330/334]`), written only by [`Self::set_healer`].
    healer: Option<u64>,
    deadline: Option<Instant>,
    in_range: bool,
}

impl AreaSpiritHealer {
    /// `SMSG_AREA_SPIRIT_HEALER_TIME`: for the cached healer with a positive time, arm the clock
    /// (a zero-landing deadline reads as 1 ms in the reference) and owe `_IN_RANGE`.
    pub(crate) fn on_time(&mut self, healer: u64, ms: u32, now: Instant) {
        if self.healer == Some(healer) && ms > 0 {
            self.deadline = Some(now + std::time::Duration::from_millis(u64::from(ms)));
            self.in_range = true;
        }
    }

    /// **`0x4921c0` — "set current area spirit healer"**, decoded at the branch:
    ///
    /// ```text
    /// if (cached == new) return;            // 0x4921cf/0x4921dd — guards EVERYTHING below
    /// cached = new;                         // 0x4921e5 / 0x4921f5
    /// if (deadline != 0) CancelAura(0xA18); // 0x4921fc — fires _OUT_OF_RANGE, sends 0x136
    /// deadline = 0;                         // 0x492211
    /// if (cached != 0) send CMSG 0x2E2;     // 0x492217 / 0x492219
    /// ```
    ///
    /// The early return at the top is the load-bearing part and the one a paraphrase loses: with
    /// the guid unchanged this routine does **nothing at all** — no cancel, no deadline clear, no
    /// packet. That is what lets the per-frame poll call it unconditionally (it clears by calling
    /// `set_healer(None)` every frame it has no ghost) at zero cost.
    pub(crate) fn set_healer(&mut self, new: Option<u64>) -> SetHealerOutcome {
        // The reference keeps a 0:0 guid where we keep `None`; normalise so a zero guid arriving
        // from the wire cannot masquerade as a real healer.
        let new = new.filter(|&g| g != 0);
        if self.healer == new {
            return SetHealerOutcome::default();
        }
        self.healer = new;
        let cancel_aura = self.deadline.take().is_some();
        if cancel_aura {
            // The cancel closes the dialog, so the `_IN_RANGE` this frame would have owed is
            // stale — the reference cannot have one pending here either (the event is fired from
            // the handler, which runs the poll first).
            self.in_range = false;
        }
        SetHealerOutcome {
            cancel_aura,
            query: new,
        }
    }

    /// The **spirit-guide click arm**'s two calls (`0x5df950`: `0x4921c0(0,0)` then
    /// `0x4921c0(guid)`). The first is a deliberate cache-bust — its own early return suppresses
    /// the send for a zero guid — so the second **always** transmits, even for the healer already
    /// cached. Clicking the guide you are standing next to therefore re-asks for the clock, which
    /// is exactly what a player does when the dialog has been dismissed.
    pub(crate) fn click_guide(&mut self, guid: u64) -> SetHealerOutcome {
        let bust = self.set_healer(None);
        let set = self.set_healer(Some(guid));
        SetHealerOutcome {
            cancel_aura: bust.cancel_aura || set.cancel_aura,
            query: set.query,
        }
    }

    /// The cached healer — `AcceptAreaSpiritHeal`'s guid and the poll's retain subject.
    pub(crate) fn healer(&self) -> Option<u64> {
        self.healer
    }

    fn secs(&self, now: Instant) -> u32 {
        self.deadline
            .map(|d| d.saturating_duration_since(now).as_secs())
            .map_or(0, |s| u32::try_from(s).unwrap_or(u32::MAX))
    }
}

/// The three battleground queue slots (`0xb6e9d0`, stride `0x20`), each with the moment its
/// status landed — the clock every stamp in the slot is relative to. Kept across an in-session
/// world enter (`0x4a9db0`); zeroed whole at the session end, as the reference's module init
/// zeroes it at every login (`net::on_session_end`).
#[derive(Resource, Default)]
pub(crate) struct BattlefieldQueue {
    slots: [Option<(BattlefieldStatus, Instant)>; 3],
    changed: bool,
    /// The slot the player is IN (`[0x8457cc]`, the status-3 arm) and its map.
    active: Option<(usize, u32)>,
    /// The instance's two clocks (`[0xb6ebbc]`/`[0xb6ebb8]`): the run-time stamp `now − Δ₂` and
    /// the expiration `now + Δ₁`, set by a status-3 message and zeroed by ANY non-clearing message
    /// of another status — whatever slot it is about (an anomaly of the handler `0x4aa850`,
    /// reproduced: 1972 zeroed them only for the active slot; 1974 corrects it to the handler's
    /// unconditional clear).
    run_started: Option<Instant>,
    instance_expiration: Option<Instant>,
    /// The status-3 arm rebuilds the scoreboard and fires `UPDATE_BATTLEFIELD_SCORE` before
    /// `UPDATE_BATTLEFIELD_STATUS` (`0x4aaa5a`, then `0x4aab05`) — the score feed reads this first.
    score_dirty: bool,
    /// The handler's two tutorial arms (`0x2f` on queued, `0x30` on confirm; 1976), owed to the
    /// tutorial system on the next feed.
    tutorials: Vec<u32>,
}

impl BattlefieldQueue {
    /// `SMSG_BATTLEFIELD_STATUS` (`0x4aa850`): an out-of-range slot abandons the message; a zero
    /// map takes the clear arm (the slot emptied, the instance clocks zeroed only when this was the
    /// active slot — and the active index itself left alone, as the handler leaves `[0x8457cc]`);
    /// status 3 stamps the instance clocks and names the slot active; every other status zeroes
    /// the instance clocks unconditionally and un-names the slot if it was the active one.
    pub(crate) fn apply(&mut self, status: BattlefieldStatus) {
        self.apply_at(status, Instant::now());
    }

    fn apply_at(&mut self, status: BattlefieldStatus, now: Instant) {
        let index = status.slot as usize;
        let Some(slot) = self.slots.get_mut(index) else {
            return;
        };
        if status.map_id == 0 {
            if self.active.is_some_and(|(i, _)| i == index) {
                self.run_started = None;
                self.instance_expiration = None;
            }
            *slot = None;
            self.changed = true;
            return;
        }
        match status.status {
            1 => self.tutorials.push(crate::tutorial::id::BATTLEGROUND_QUEUE),
            2 => self
                .tutorials
                .push(crate::tutorial::id::PORT_TO_BATTLEGROUND),
            _ => {}
        }
        match status.in_progress {
            Some((expires_ms, elapsed_ms)) => {
                self.active = Some((index, status.map_id));
                self.instance_expiration = (expires_ms != 0)
                    .then(|| now + std::time::Duration::from_millis(u64::from(expires_ms)));
                self.run_started = (elapsed_ms != 0)
                    .then(|| now - std::time::Duration::from_millis(u64::from(elapsed_ms)));
                self.score_dirty = true;
            }
            None => {
                self.run_started = None;
                self.instance_expiration = None;
                if self.active.is_some_and(|(i, _)| i == index) {
                    self.active = None;
                }
            }
        }
        *slot = Some((status, now));
        self.changed = true;
    }

    /// The three slots with the instant each status landed — the queue verbs' view builder
    /// (`crate::ui_battlefield`) reduces their stamps against `now`.
    pub(crate) fn slots(&self) -> &[Option<(BattlefieldStatus, Instant)>; 3] {
        &self.slots
    }

    /// `GetBattlefieldInstanceExpiration()`: `deadline − now` in ms, 0 when unset or past
    /// (`[0xb6ebb8]`, the `jns` guard).
    pub(crate) fn instance_expiration_ms(&self, now: Instant) -> u32 {
        self.instance_expiration.map_or(0, |d| {
            d.saturating_duration_since(now)
                .as_millis()
                .min(u128::from(u32::MAX)) as u32
        })
    }

    /// The map of the battleground the player is in — `LeaveBattlefield`'s payload; `None` = 0.
    pub(crate) fn active_map(&self) -> Option<u32> {
        self.active.map(|(_, map)| map)
    }

    /// `GetBattlefieldInstanceRunTime()`: ms since the status-3 stamp, 0 with none.
    pub(crate) fn run_time_ms(&self, now: Instant) -> u32 {
        self.run_started.map_or(0, |t| {
            now.saturating_duration_since(t)
                .as_millis()
                .min(u128::from(u32::MAX)) as u32
        })
    }

    /// The status-3 arm's scoreboard rebuild, once per arrival.
    pub(crate) fn take_score_dirty(&mut self) -> bool {
        std::mem::take(&mut self.score_dirty)
    }

    /// The map id `AcceptBattlefieldPort` sends for a 1-based slot, if the slot holds a queue.
    fn map_id(&self, index: u8) -> Option<u32> {
        self.slots
            .get(usize::from(index).checked_sub(1)?)
            .and_then(|s| s.as_ref())
            .map(|(s, _)| s.map_id)
    }
}

/// The cached status text's three states (`[0xb7203c]`): none from process start and after world
/// leave; the bare localized `UNKNOWN` from world enter until the server's `0x295` lands; a
/// built line after that. The two localized halves are resolved against the VM at push time.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
enum StoneText {
    #[default]
    None,
    Unknown,
    Built(String),
}

/// `SStrPrintf`'s buffer at the rebuild: 256 bytes, so 255 of text.
const STONE_TEXT_BYTES: usize = 255;

/// The meeting-stone queue: the two globals and what the wire still owes the screen.
#[derive(Resource, Default)]
pub(crate) struct MeetingStone {
    /// `[0xb72038]` — the queued area id, `0` = none.
    pub(crate) area: u32,
    text: StoneText,
    /// `0x295` arrivals since the last feed: `(the area BEFORE the store, status)`; the new area
    /// is already in `area` (the handler stores it before it switches).
    updates: Vec<(u32, u8)>,
    notices: Vec<MeetingStoneNotice>,
    /// `0x299` guids whose name has not resolved yet.
    pending_members: Vec<u64>,
    /// The VM's copy of the two globals is stale.
    dirty: bool,
}

impl MeetingStone {
    /// `SMSG 0x295`: the area is stored unconditionally; the line, the rebuild and the event
    /// follow on the next feed, with the VM.
    pub(crate) fn apply(&mut self, area: u32, status: u8) {
        let old = self.area;
        self.area = area;
        self.updates.push((old, status));
    }

    /// One of the four display-only replies.
    pub(crate) fn apply_notice(&mut self, notice: MeetingStoneNotice) {
        self.notices.push(notice);
    }

    /// The enter-world bring-up (`0x4c9f40`): the text becomes the bare `UNKNOWN`; the area is
    /// untouched (the server's reply resets it).
    fn enter_world(&mut self) {
        self.text = StoneText::Unknown;
        self.dirty = true;
    }

    /// The leave-world sweep (`0x4c9f80`): the text goes, the area stays.
    fn leave_world(&mut self) {
        self.text = StoneText::None;
        self.dirty = true;
    }
}

/// A `GlobalStrings` value as the client's `GetText` reads it: the string, or `""` when the Lua
/// global is missing (`0x882748`, the shared empty-string constant — never NULL).
fn global_text(script: &UiScript, key: &str) -> String {
    script
        .lua()
        .globals()
        .get::<String>(key)
        .unwrap_or_default()
}

/// The area's localized name, or `None` where the reference's three-part AreaTable resolve fails.
fn area_name(areas: Option<&AreaTableRes>, id: u32) -> Option<&str> {
    areas.and_then(|a| a.0.name(id))
}

/// The status-text rebuild `0x4ca070`: `MEETINGSTONE_TOOLTIP` (or `""` when missing) over the
/// queued area's name (or `UNKNOWN`), printed into a 256-byte buffer.
fn build_stone_text(script: &UiScript, areas: Option<&AreaTableRes>, area: u32) -> String {
    let name = area_name(areas, area)
        .map(str::to_string)
        .unwrap_or_else(|| global_text(script, "UNKNOWN"));
    let mut text = global_text(script, "MEETINGSTONE_TOOLTIP").replacen("%s", &name, 1);
    if text.len() > STONE_TEXT_BYTES {
        let mut cut = STONE_TEXT_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    text
}

/// The `0x295` handler's five-way table (`0x4ca3a4`), as the line it prints for
/// `(old, new, status)` — `None` where the reference prints nothing: status 0 with no row for the
/// OLD area, status 1 with an unchanged area, and any status past 4.
fn stone_line(
    script: &UiScript,
    areas: Option<&AreaTableRes>,
    old: u32,
    new: u32,
    status: u8,
) -> Option<crate::ui_action::Shown> {
    match status {
        0 => {
            let name = area_name(areas, old)?;
            crate::ui_action::keyed_line_s(script, "ERR_MEETING_STONE_LEFT_QUEUE_S", &[name])
        }
        1 => {
            if new == old {
                return None;
            }
            let name = area_name(areas, new)
                .map(str::to_string)
                .unwrap_or_else(|| global_text(script, "UNKNOWN"));
            crate::ui_action::keyed_line_s(script, "ERR_MEETING_STONE_IN_QUEUE_S", &[&name])
        }
        2 => crate::ui_action::keyed_line(script, "ERR_MEETING_STONE_OTHER_MEMBER_LEFT"),
        3 => crate::ui_action::keyed_line(script, "ERR_MEETING_STONE_PARTY_KICKED_FROM_QUEUE"),
        4 => crate::ui_action::keyed_line(script, "ERR_MEETING_STONE_MEMBER_STILL_IN_QUEUE"),
        _ => None,
    }
}

/// The inputs the meeting-stone feed reads beside the VM and its own state — bundled because the
/// feed sits at Bevy's parameter ceiling otherwise.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MeetingStoneInputs<'w, 's> {
    areas: Option<Res<'w, AreaTableRes>>,
    names: Res<'w, NameCache>,
    commands: Res<'w, NetCommands>,
    visuals: Option<Res<'w, SpellVisuals>>,
    fx: MessageWriter<'w, SpellKitFx>,
    self_q: Query<'w, 's, Entity, With<SelfPlayer>>,
    tutorials: Option<MessageWriter<'w, crate::tutorial::TutorialEvent>>,
}

/// The meeting stone's feed: the `0x295` lines, the rebuild and `MEETINGSTONE_CHANGED` per
/// arrival; the four display replies; the two globals pushed when they moved.
fn feed_meeting_stone(
    script: Option<NonSendMut<UiScript>>,
    mut stone: ResMut<MeetingStone>,
    mut inputs: MeetingStoneInputs,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let areas = inputs.areas.as_deref();
    let mut lines = Vec::new();

    for (old, status) in std::mem::take(&mut stone.updates) {
        let new = stone.area;
        lines.extend(stone_line(&script, areas, old, new, status));
        if status == 1 && new != old {
            // The status-1 arm's extra block: `Effect_C` kind `0xc` on the local player, then
            // the Meeting Stones tutorial (`0x4ca363`, 1976).
            if let (Some(visuals), Ok(entity)) = (inputs.visuals.as_deref(), inputs.self_q.single())
            {
                if let Some(fx) = meeting_stone_join_fx(visuals, entity) {
                    inputs.fx.write(fx);
                }
            }
            if let Some(t) = inputs.tutorials.as_mut() {
                t.write(crate::tutorial::TutorialEvent::trigger(
                    crate::tutorial::id::MEETING_STONES,
                ));
            }
        }
        // Every path — the silent legs and an out-of-range status included — rebuilds and fires.
        // The two globals reach the VM BEFORE the event: the stock handler's first act is
        // `IsInMeetingStoneQueue()`, which has to see the area this packet stored.
        let text = build_stone_text(&script, areas, new);
        stone.text = StoneText::Built(text.clone());
        stone.dirty = false;
        script.set_meeting_stone(new, Some(text));
        script.fire_event("MEETINGSTONE_CHANGED", vec![]);
    }

    for notice in std::mem::take(&mut stone.notices) {
        match notice {
            MeetingStoneNotice::Success => {
                lines.extend(crate::ui_action::keyed_line(
                    &script,
                    "ERR_MEETING_STONE_SUCCESS",
                ));
            }
            MeetingStoneNotice::InProgress => {
                lines.extend(crate::ui_action::keyed_line(
                    &script,
                    "ERR_MEETING_STONE_IN_PROGRESS",
                ));
            }
            MeetingStoneNotice::MemberAdded { guid } => stone.pending_members.push(guid),
            MeetingStoneNotice::JoinFailed { code } => {
                let key = match code {
                    1 => "ERR_MEETING_STONE_MUST_BE_LEADER",
                    2 => "ERR_MEETING_STONE_GROUP_FULL",
                    3 => "ERR_MEETING_STONE_NO_RAID_GROUP",
                    _ => continue,
                };
                lines.extend(crate::ui_action::keyed_line(&script, key));
            }
        }
    }
    // `0x299`'s name-cache callback: the line when the name lands, nothing until then.
    let pending = std::mem::take(&mut stone.pending_members);
    for guid in pending {
        match inputs
            .names
            .resolve(guid, &inputs.commands)
            .map(str::to_string)
        {
            Some(name) => lines.extend(crate::ui_action::keyed_line_s(
                &script,
                "ERR_MEETING_STONE_MEMBER_ADDED_S",
                &[&name],
            )),
            None => stone.pending_members.push(guid),
        }
    }
    if !lines.is_empty() {
        crate::ui_action::show_messages(&mut script, &mut sink, "ui_dialog_verbs", lines);
    }

    if std::mem::take(&mut stone.dirty) {
        let text = match &stone.text {
            StoneText::None => None,
            StoneText::Unknown => Some(global_text(&script, "UNKNOWN")),
            StoneText::Built(t) => Some(t.clone()),
        };
        script.set_meeting_stone(stone.area, text);
    }
}

/// The enter-world bring-up's meeting-stone leg: the text reset, then the empty `CMSG 0x296`.
///
/// **Once per UI LIFECYCLE, not once per world session**, and that distinction is the whole bug.
/// The reference's run-once byte `[0xb4b424]` is *cleared by the UI teardown* — `0x490bd0` calls
/// `0x490a80` at `0x490c20`, which zeroes it at `0x490a8d` — and `UI_Init` (`0x48fbf0`) then calls
/// `0x4908c0` at `0x490168` behind the same live-player gate, finds the byte clear, and re-runs
/// this entire bring-up: `0x490a14` → `0x4c9f40` → `0x4ca1c0` → `PutUInt32(0x296)` + Send. So a
/// `ReloadUI()` **re-asks the server**, and that is what brings the icon back.
///
/// Gated on `MessageReader<EnteredWorldMessage>`, this leg never ran for a `/reload`, which never
/// leaves the world (1291) and so produces no such message. Nothing re-armed
/// [`MeetingStone::dirty`] and nothing re-queried, so `IsInMeetingStoneQueue()` and
/// `GetMeetingStoneStatusText()` answered nil for the rest of the session — and stock
/// `Minimap.xml`'s `MiniMapMeetingStoneFrame` (built `hidden="true"`, shown only by
/// `MEETINGSTONE_CHANGED`, whose single firing site image-wide is the `0x295` handler at
/// `0x4ca38f`) stayed gone while the player was still queued.
///
/// **A [`crate::ui_script::VmMemo`] claim IS the reference's gate.** A byte the UI teardown clears
/// is exactly "once per VM", and 1290's name for that is `claim` — so this reads as the same
/// question the binary asks, rather than as a workaround for the missing message. The one
/// round trip during which the icon is genuinely absent is faithful, not a defect: the reference
/// has the same gap, because only the server's reply fires the event.
///
/// The queued area itself is untouched here, matching `[0xb72038]`, which is
/// referenced six times image-wide and by nothing in either reload closure — it survives, and
/// [`MeetingStone::enter_world`]'s `dirty` is what re-pushes it to the fresh VM.
fn meeting_stone_enter_world(
    script: Option<NonSendMut<UiScript>>,
    mut stone: ResMut<MeetingStone>,
    commands: Res<NetCommands>,
    mut asked: Local<crate::ui_script::VmMemo<bool>>,
) {
    let Some(script) = script else {
        return;
    };
    if !asked.claim(&script) {
        return;
    }
    stone.enter_world();
    let _ = commands.0.send(ClientCommand::MeetingStoneStatusQuery);
}

/// A right-click on a `GAMEOBJECT_TYPE_MEETINGSTONE` (23) that got past the shared gates — the
/// GO click ladder's hand-off to this module (decision 2283).
///
/// It travels as a message for the same reason the GameObject opener's cast does (2199): the
/// click system sits at Bevy's 16-`SystemParam` ceiling and cannot also hold the roster, the
/// template cache and the wire. The reference has no such split — `0x5f69d0` is one function —
/// so the *verdict* stays one function here too ([`meeting_stone_join_refusal`]); only the
/// plumbing is two systems.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MeetingStoneUse {
    pub(crate) go_guid: u64,
}

/// What MEETINGSTONE(23)'s own use slot `0x5f69d0` does with one click — its three outcomes, as
/// the binary has them (decision 2283).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoneJoin {
    /// `0x5f69f8` — no local player object: `false` with **no message and no packet**. Not a
    /// refusal; the validator never starts.
    Silent,
    /// One of the four refusals: `push <id>; call 0x496720` then `xor al,al; ret`. Every one is a
    /// catalog **kind 2** row, so it paints the error frame (`UI_ERROR_MESSAGE`), never a chat
    /// line — and none carries a sound (type tag `0x44`, cue `"NONE"`).
    Refuse(&'static str),
    /// The tail `0x5f6af6`: `CMSG 0x292 {u64 goGuid}`, and nothing else at all.
    Send,
}

/// The four client-side refusals inside MEETINGSTONE(23)'s own use slot (`0x5f69d0`, whose tail
/// `0x5f6af6` is the sole caller of the `CMSG 0x292` builder `0x4c9ff0`).
///
/// Two gates run before any of them. `0x5f69f8`: no local player ⇒ [`StoneJoin::Silent`].
/// `0x5f6a10 je 0x5f6a65`: **not in a group ⇒ both group refusals are skipped**, and a solo player
/// drops straight to the level test.
///
/// | # | at | refusal | predicate | key |
/// |---|---|---|---|---|
/// | 1 | `0x5f6a2f` | in a group we do not lead | the leader guid `[0xbc75f8]` against the active player's, full 64-bit | `ERR_MEETING_STONE_MUST_BE_LEADER` (`0x1b1`) |
/// | 2 | `0x5f6a4f` | the group is full | `0x4e86d0` is `GetNumPartyMembers` — the **other** members, 0..4 — and `cmp eax,4 / jb` refuses at ≥ 4 | `ERR_MEETING_STONE_GROUP_FULL` (`0x1ae`) |
/// | 3 | `0x5f6ab4` | the wrong level for this stone | `data[0] <= level <= data[1]`, **both bounds inclusive and unsigned**, over the player's own `UNIT_FIELD_LEVEL` | `ERR_MEETING_STONE_INVALID_LEVEL` (`0x1b0`) |
/// | 4 | `0x5f6ad3` | in a raid | `[0xb713e0] != 0` — the raid member **count** (`GetNumRaidMembers`) | `ERR_MEETING_STONE_NO_RAID_GROUP` (`0x1b2`) |
///
/// **Three of the four are refused a second time by the server** (vmangos
/// `HandleMeetingStoneJoinOpcode` → `MEETINGSTONE_FAIL_PARTYLEADER` / `_FULL_GROUP` /
/// `_RAID_GROUP`, which come back as `SMSG 0x2BB` and print the same three strings 1974 already
/// built). That copy is not redundant — it is what makes the refusal instant. **The level term is
/// the client's alone**: `HandleMeetingStoneJoinOpcode` reads `gInfo->meetingstone.areaID` and
/// nothing else off the template, so a client that skips it queues a level-1 character for a
/// sixty-level dungeon and the server agrees.
///
/// **An unanswered template REFUSES, and that is the reference's own arithmetic rather than a
/// fail-closed choice of ours.** `0x5f8150` reads the cached template through `[GO+0x214]`, and an
/// uncached object returns **0 for both bounds** — so every level ≥ 1 falls outside `0..=0` and
/// takes the level refusal. The permissive "skip the term while the query is in flight" default
/// the highlight column takes is *wrong* here, and the first cut of this had it. The same
/// arithmetic makes a shipped `data[0] = data[1] = 0` refuse everyone; `0/60` is the open band.
pub(crate) fn meeting_stone_join_refusal(
    group: Option<&GroupState>,
    self_guid: Option<u64>,
    level: Option<u32>,
    stone: Option<crate::go_templates::MeetingStoneTemplate>,
) -> StoneJoin {
    // `0x5f69f8` — the validator needs an active player before it asks anything.
    let (Some(self_guid), Some(level)) = (self_guid, level) else {
        return StoneJoin::Silent;
    };
    // `0x5f6a10` — no group at all skips past both group terms.
    let in_group = group.is_some_and(|g| g.in_group);
    if in_group {
        if group.map(|g| g.leader) != Some(self_guid) {
            return StoneJoin::Refuse("ERR_MEETING_STONE_MUST_BE_LEADER");
        }
        if group.is_some_and(|g| g.members.len() >= MEETING_STONE_PARTY_CAP) {
            return StoneJoin::Refuse("ERR_MEETING_STONE_GROUP_FULL");
        }
    }
    // An uncached template reads `0/0` here, which refuses — see the doc above.
    let (min_level, max_level) = stone.map_or((0, 0), |s| (s.min_level, s.max_level));
    if level < min_level || level > max_level {
        return StoneJoin::Refuse("ERR_MEETING_STONE_INVALID_LEVEL");
    }
    if in_group && group.is_some_and(|g| g.group_type == crate::ui_party::GROUPTYPE_RAID) {
        return StoneJoin::Refuse("ERR_MEETING_STONE_NO_RAID_GROUP");
    }
    StoneJoin::Send
}

/// How many **other** party members make the stone's `GROUP_FULL` refusal fire — a party it could
/// add nobody to.
///
/// **Four, not five, and the difference is a real trap.** A vanilla party holds five *including*
/// the player, and vmangos refuses on exactly that: `Group::IsFull()` is
/// `m_memberSlots.size() >= MAX_GROUP_SIZE (5)`, counting the leader
/// (`Group/Group.h:49,232`) → `MEETINGSTONE_FAIL_FULL_GROUP`. But `SMSG_GROUP_LIST` never lists
/// the recipient (0440), so [`GroupState::members`] is the other four and the comparison is
/// against **4** with no `+ 1`. Adding one — the first cut of this did — refuses a legal
/// four-person party the server would have queued, and the string says which reading is right:
/// `ERR_MEETING_STONE_GROUP_FULL` is *"You are already in a full group"*, and a group of four is
/// not full.
const MEETING_STONE_PARTY_CAP: usize = 4;

/// The join drain: MEETINGSTONE(23)'s use slot, with the click's guid.
fn drain_meeting_stone_joins(
    script: Option<NonSendMut<UiScript>>,
    mut uses: MessageReader<MeetingStoneUse>,
    group: Option<Res<GroupState>>,
    self_guid: Res<SelfGuid>,
    templates: Res<crate::go_templates::GameObjectTemplates>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        uses.clear();
        return;
    };
    let level = self_q.single().ok().and_then(|s| s.0.unit_level());
    let mut lines = Vec::new();
    for &MeetingStoneUse { go_guid } in uses.read() {
        let stone = templates.get(go_guid).and_then(|t| t.meeting_stone);
        let verdict = meeting_stone_join_refusal(group.as_deref(), self_guid.0, level, stone);
        // The interact chain's last link for this type, on the same `use` tag the click's own
        // lines carry (2283): a stone that goes nowhere is one of four refusals, the no-player
        // silence, or a send the server ignored — and one trace now says which.
        if benilla_assets::trace::enabled_for("use") {
            benilla_assets::trace::line(
                "use",
                &match verdict {
                    StoneJoin::Silent => format!("meeting stone {go_guid:#x}: no local player"),
                    StoneJoin::Refuse(key) => {
                        format!("meeting stone {go_guid:#x} refused: {key}")
                    }
                    StoneJoin::Send => {
                        format!("SEND CMSG_MEETINGSTONE_JOIN guid={go_guid:#x}")
                    }
                },
            );
        }
        match verdict {
            StoneJoin::Silent => {}
            StoneJoin::Refuse(key) => lines.extend(crate::ui_action::keyed_line(&script, key)),
            StoneJoin::Send => {
                debug!("meeting stone: join {go_guid:#x}");
                let _ = commands.0.send(ClientCommand::MeetingStoneJoin { go_guid });
            }
        }
    }
    if !lines.is_empty() {
        crate::ui_action::show_messages(&mut script, &mut sink, "ui_dialog_verbs", lines);
    }
}

/// The leave-world sweep's leg: the text dropped, the area kept.
fn meeting_stone_leave_world(mut stone: ResMut<MeetingStone>) {
    stone.leave_world();
}

/// Drop the cached spirit guide when the world does.
///
/// **A resurrect wave is the most session-bound state this module holds**, and it was the one
/// resource here with no leave-world leg. A ghost who logs out at a Warsong Gulch graveyard with a
/// guide latched and a wave armed carried `healer`, `deadline` and `in_range` into the character
/// screen and into the *next* character's session — where, on the first frame the interface came
/// up, [`feed_dialog_verbs`] pushed the stale healer into the fresh VM and could fire
/// `AREA_SPIRIT_HEALER_IN_RANGE` at a living body standing in Stormwind, and the poll's ghost gate
/// then cleared it and sent a `CMSG_CANCEL_AURA(2584)` the reference never sends there.
///
/// The reference does not need this leg because its equivalents are process-lifetime globals whose
/// poll keeps running over the character screen's frames; ours is a resource in a world that goes
/// away. Resetting at the seam is how every other per-session cache here behaves
/// (`crate::ui_aura::end_session_aura_state` is the established shape).
fn area_spirit_healer_leave_world(mut spirit: ResMut<AreaSpiritHealer>) {
    *spirit = AreaSpiritHealer::default();
}

pub(crate) fn feed_dialog_verbs(
    script: Option<NonSendMut<UiScript>>,
    mut pet: ResMut<PetUnlearnState>,
    mut boot: ResMut<InstanceBoot>,
    mut spirit: ResMut<AreaSpiritHealer>,
    mut queue: ResMut<BattlefieldQueue>,
    mut sink: crate::ui_action::MessageSink,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let now = Instant::now();

    script.set_pet_untrainer_pending(pet.pending().is_some());
    if pet.ask {
        pet.ask = false;
        script.fire_event(
            "CONFIRM_PET_UNLEARN",
            vec![ScriptValue::Int(i64::from(pet.cost))],
        );
    }

    script.set_instance_boot_secs(boot.secs(now));
    for event in std::mem::take(&mut boot.events) {
        script.fire_event(event, vec![]);
    }
    let lines: Vec<_> = std::mem::take(&mut boot.errors)
        .into_iter()
        .filter_map(|key| crate::ui_action::keyed_line(&script, key))
        .collect();
    if !lines.is_empty() {
        crate::ui_action::show_messages(&mut script, &mut sink, "ui_dialog_verbs", lines);
    }

    let secs = spirit.secs(now);
    script.set_area_spirit_healer(spirit.healer().is_some(), secs);
    if std::mem::take(&mut spirit.in_range) {
        script.fire_event("AREA_SPIRIT_HEALER_IN_RANGE", vec![]);
    }

    for id in std::mem::take(&mut queue.tutorials) {
        if let Some(t) = tutorials.as_mut() {
            t.write(crate::tutorial::TutorialEvent::trigger(id));
        }
    }
    if std::mem::take(&mut queue.changed) {
        script.fire_event("UPDATE_BATTLEFIELD_STATUS", vec![]);
    }
}

/// The pet trainer's confirm and the spirit healer's accept — the two drains over a latch.
fn drain_latch_verbs(
    script: Option<NonSendMut<UiScript>>,
    pet: Res<PetUnlearnState>,
    spirit: Res<AreaSpiritHealer>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };

    // ConfirmPetUnlearn: the latch, then the money gate — `cost > coinage` shows
    // ERR_NOT_ENOUGH_MONEY and sends nothing; otherwise `0x2F0` with the latched guid.
    let confirms = script.take_pet_unlearn_confirms();
    if confirms > 0 {
        if let Some(npc) = pet.pending() {
            let money = self_q
                .single()
                .ok()
                .and_then(|store| store.0.player_money())
                .unwrap_or(0);
            if pet.cost > money {
                if let Some(line) = crate::ui_action::keyed_line(&script, "ERR_NOT_ENOUGH_MONEY") {
                    crate::ui_action::show_messages(
                        &mut script,
                        &mut sink,
                        "ui_dialog_verbs",
                        [line],
                    );
                }
            } else {
                for _ in 0..confirms {
                    let _ = commands.0.send(ClientCommand::PetUnlearn { trainer: npc });
                }
            }
        }
    }

    // AcceptAreaSpiritHeal: the cached healer's guid (the binding was silent without one).
    let accepts = script.take_area_spirit_accepts();
    if let Some(healer) = spirit.healer() {
        for _ in 0..accepts {
            let _ = commands
                .0
                .send(ClientCommand::AreaSpiritHealerQueue { healer });
        }
    }
}

/// The battleground port and the meeting-stone leave — the two drains over a queue.
fn drain_queue_verbs(
    script: Option<NonSendMut<UiScript>>,
    queue: Res<BattlefieldQueue>,
    group: Option<Res<GroupState>>,
    self_guid: Res<SelfGuid>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };

    // AcceptBattlefieldPort: the slot's map id and the one-byte answer.
    for (index, accept) in script.take_battlefield_port_requests() {
        if let Some(map_id) = queue.map_id(index) {
            let _ = commands
                .0
                .send(ClientCommand::BattlefieldPort { map_id, accept });
        }
    }

    // CancelMeetingStoneRequest: in a party and not its leader → ERR_MEETING_STONE_NOT_LEADER;
    // otherwise `0x293`, whatever is or is not queued.
    let cancels = script.take_meeting_stone_cancels();
    if cancels > 0 {
        let not_leader = group
            .as_deref()
            .is_some_and(|g| g.in_group && Some(g.leader) != self_guid.0);
        if not_leader {
            if let Some(line) =
                crate::ui_action::keyed_line(&script, "ERR_MEETING_STONE_NOT_LEADER")
            {
                crate::ui_action::show_messages(&mut script, &mut sink, "ui_dialog_verbs", [line]);
            }
        } else {
            for _ in 0..cancels {
                let _ = commands.0.send(ClientCommand::MeetingStoneLeave);
            }
        }
    }
}

/// **The per-frame area-spirit-healer poll — `0x4923b0`**, which the reference runs from
/// `CGWorldFrame`'s own `OnUpdate` (`0x4818ca`) on **every frame**, mouse focus or not.
///
/// ```text
/// player = the active player object; none            -> set_healer(None); return
/// not a GHOST ([[player+0xe68]+8] bit 4)              -> set_healer(None); return
/// if cached:
///     the cached guid no longer resolves to a unit    -> set_healer(None)
///     else d²(player, healer) > 22²                   -> set_healer(None)
/// if still cached: return                             // 0x492487
/// enumerate every object: the first (last, really —   // 0x492495
///   the callback never stops the walk) unit that is
///   SPIRITGUIDE, CanAssist, and within 20 yd          -> set_healer(it)
/// ```
///
/// **Ghost-gated at the top**, so a living player never holds a healer and never sends a query —
/// the graveyard's wave clock only exists for the dead. That gate is why this costs nothing in
/// ordinary play: one flag read per frame and out.
///
/// The callback `0x4924c0` returns 1 unconditionally, so the enumeration is **not** stopped by a
/// match — the last qualifying unit in enumeration order is the one that sticks. `set_healer`'s
/// own early return makes that harmless for a stable set (only a genuine change sends), and
/// reproducing "last wins" rather than "nearest wins" matters at a graveyard with two guides in
/// range: the reference does not pick the closer one, and neither does this.
fn poll_area_spirit_healer(
    mut spirit: ResMut<AreaSpiritHealer>,
    me: Query<(&ObjectStore, &Transform), With<SelfPlayer>>,
    units: Query<
        (
            &crate::net::Guid,
            &crate::net::NetEntity,
            &ObjectStore,
            &Transform,
        ),
        Without<SelfPlayer>,
    >,
    factions: Option<Res<crate::target::Factions>>,
    reputations: Res<crate::net::Reputations>,
    index: Option<Res<crate::net::GuidIndex>>,
    stores: Query<&ObjectStore>,
    commands: Res<NetCommands>,
    mut script: Option<NonSendMut<UiScript>>,
) {
    let mut apply = |outcome: SetHealerOutcome| {
        if outcome.cancel_aura {
            // `0x6e7040(0xA18)` does both halves, and both are the reference's: the event with no
            // argument, and the packet with no guid.
            if let Some(script) = script.as_deref_mut() {
                script.fire_event("AREA_SPIRIT_HEALER_OUT_OF_RANGE", vec![]);
            }
            let _ = commands.0.send(ClientCommand::CancelAura {
                spell_id: AREA_SPIRIT_HEALER_AURA,
            });
        }
        if let Some(healer) = outcome.query {
            let _ = commands
                .0
                .send(ClientCommand::AreaSpiritHealerQuery { healer });
        }
    };

    let Ok((self_store, self_tf)) = me.single() else {
        apply(spirit.set_healer(None));
        return;
    };
    if !self_store.0.player_is_ghost() {
        apply(spirit.set_healer(None));
        return;
    }
    let here = self_tf.translation;

    // The retain leg. A cached guid that no longer streams to us is dropped exactly as one that
    // walked out of range is — the reference's `ObjectPtr` miss falls into the same clear.
    if let Some(cached) = spirit.healer() {
        let still = units.iter().find(|(guid, ..)| guid.0 == cached);
        let keep = still.is_some_and(|(_, _, _, tf)| {
            tf.translation.distance_squared(here) <= SPIRIT_GUIDE_RETAIN_YD * SPIRIT_GUIDE_RETAIN_YD
        });
        if !keep {
            apply(spirit.set_healer(None));
        }
    }
    if spirit.healer().is_some() {
        return;
    }

    // The acquire walk. `can_assist` is `0x6066f0`, the same predicate the target scanner and the
    // buff gate already run — its owner chase wants a store by guid, which is what the index is
    // for.
    let store_of = |guid: u64| -> Option<ObjectStore> {
        let entity = *index.as_ref()?.0.get(&guid)?;
        stores.get(entity).ok().cloned()
    };
    let mut adopted = None;
    for (guid, kind, store, tf) in units.iter() {
        if kind.kind != benilla_protocol::EntityKind::Unit {
            continue;
        }
        if store.0.unit_npc_flags() & NPC_FLAG_SPIRITGUIDE == 0 {
            continue;
        }
        if tf.translation.distance_squared(here) > SPIRIT_GUIDE_ACQUIRE_YD * SPIRIT_GUIDE_ACQUIRE_YD
        {
            continue;
        }
        if !crate::target::can_assist(
            Some(store),
            factions.as_deref(),
            &reputations,
            Some(self_store),
            store_of,
        ) {
            continue;
        }
        adopted = Some(guid.0); // last wins — the callback never stops the walk
    }
    if let Some(guid) = adopted {
        apply(spirit.set_healer(Some(guid)));
    }
}

/// The dialog verbs' packet handlers (in the net handler table since 2313) — each parks a
/// question or a countdown on its own store for the feed to turn into a StaticPopup.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{AreaSpiritHealer, BattlefieldQueue, InstanceBoot, MeetingStone, PetUnlearnState};
    use crate::net::NetHandlerApp;

    /// Register the handlers — called from [`super::UiDialogVerbsPlugin`].
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::PetUnlearnConfirm, on_pet_unlearn_confirm)
            .net_handler(K::RaidGroupOnly, on_raid_group_only)
            .net_handler(K::AreaSpiritHealerTime, on_area_spirit_healer_time)
            .net_handler(K::BattlefieldStatus, on_battlefield_status)
            .net_handler(K::MeetingStoneSetQueue, on_meeting_stone)
            .net_handler(K::MeetingStoneNotice, on_meeting_stone)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// The battleground queue is zeroed at every login (module init `0x4a9c40`, from
    /// `InitializeGame` — the three slots, `[0x8457cc] = -1`, the
    /// scalars), and vmangos never sends a clear for the queue of a player who logged out. A
    /// listener on the session end (a second handler on the kind, after the bridge's own
    /// teardown). The in-session world enter (`0x4a9db0`) is a different edge that KEEPS the slots,
    /// and it does not come through here.
    fn on_session_end(In(_): In<SessionEvent>, mut queue: ResMut<BattlefieldQueue>) {
        *queue = BattlefieldQueue::default();
    }

    /// The pet trainer's question (decision 1963) — the talent wipe's twin
    /// ([`crate::ui_talent_wipe`]); a zero guid is the reference's own `ERR_TALENT_WIPE_ERROR`
    /// leg, carried over as observed.
    fn on_pet_unlearn_confirm(
        In(ev): In<SessionEvent>,
        mut unlearn: ResMut<PetUnlearnState>,
        mut errors: ResMut<crate::ui_action::UiErrorKeys>,
    ) {
        if let SessionEvent::PetUnlearnConfirm { trainer, cost } = ev {
            if trainer == 0 {
                debug!("net: pet unlearn refused (zero trainer) — no dialog");
                errors
                    .0
                    .push(crate::ui_action::UiError::key("ERR_TALENT_WIPE_ERROR"));
            } else {
                debug!("net: trainer {trainer:#x} asks to unlearn the pet for {cost} copper");
                unlearn.ask(trainer, cost);
            }
        }
    }

    fn on_raid_group_only(In(ev): In<SessionEvent>, mut boot: ResMut<InstanceBoot>) {
        if let SessionEvent::RaidGroupOnly { delay_ms, reason } = ev {
            boot.apply(delay_ms, reason, std::time::Instant::now());
        }
    }

    fn on_area_spirit_healer_time(In(ev): In<SessionEvent>, mut spirit: ResMut<AreaSpiritHealer>) {
        if let SessionEvent::AreaSpiritHealerTime { healer, ms } = ev {
            spirit.on_time(healer, ms, std::time::Instant::now());
        }
    }

    fn on_battlefield_status(In(ev): In<SessionEvent>, mut queue: ResMut<BattlefieldQueue>) {
        if let SessionEvent::BattlefieldStatus(status) = ev {
            queue.apply(status);
        }
    }

    fn on_meeting_stone(In(ev): In<SessionEvent>, mut stone: ResMut<MeetingStone>) {
        match ev {
            SessionEvent::MeetingStoneSetQueue { area, status } => stone.apply(area, status),
            SessionEvent::MeetingStoneNotice(notice) => stone.apply_notice(notice),
            _ => {}
        }
    }
}

pub(crate) struct UiDialogVerbsPlugin;

impl Plugin for UiDialogVerbsPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<PetUnlearnState>()
            .init_resource::<InstanceBoot>()
            .init_resource::<AreaSpiritHealer>()
            .init_resource::<BattlefieldQueue>()
            .init_resource::<MeetingStone>()
            .add_message::<MeetingStoneUse>()
            .add_systems(
                Update,
                (
                    close_npc_session_out_of_range::<PetUnlearnState>.before(feed_dialog_verbs),
                    // Gated on the interface being up (decision 2279): `InstanceBoot::events`
                    // is server-driven — vmangos sends `SMSG_RAID_GROUP_ONLY` from
                    // `Player::UpdateHomebindTime` on the first map tick after a login inside a
                    // raid instance with no raid group, i.e. inside the login burst's own
                    // drain — and `INSTANCE_BOOT_START`, the event that raises the stock
                    // countdown popup, would be fired at the boot VM in 2214's one-frame window
                    // and lost. Gated, the queue waits; nothing here clears without a VM.
                    feed_dialog_verbs
                        .in_set(UiFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                    // **After the frame's pick, which is the reference's own order.** The poll
                    // is `0x4923b0`, called from `CGWorldFrame`'s OnUpdate at `0x4818ca` — and
                    // that same function ran the mouse pick eighty bytes earlier, at `0x48184a`.
                    // So a right-click that adopts a spirit guide is decided BEFORE the poll gets
                    // to look, not after; ordering it `.after(TargetUpdate)` (the set
                    // `act_on_right_click` chains inside) reproduces that, and settles the one
                    // real write-write pair this system has — both it and the click's
                    // `ServiceArms` hold `AreaSpiritHealer`.
                    //
                    // The cost is that an adopt reaches `feed_dialog_verbs` on the NEXT frame
                    // rather than this one, because `UiInput` sits before `WorldStage::Input` and
                    // `TargetUpdate` after it — the two orders cannot both hold. That is the
                    // right way round to lose: the event this system fires itself
                    // (`_OUT_OF_RANGE`) is immediate, and what lags is one frame of a clock the
                    // server ticks in seconds.
                    //
                    // **Gated on the interface being up** like its neighbour, and safely so: this
                    // poll is LEVEL-triggered, not edge-triggered. It re-derives the whole cache
                    // from the ghost flag, the streamed guides and the distances every frame, so a
                    // frame it does not run is a frame it simply has not got to yet — the very
                    // next one adopts and queries. That is what makes it exempt in substance from
                    // the one-shot loss 2214's window causes (2220/2232): there is no edge here to
                    // lose. The `_OUT_OF_RANGE` it can fire is the one thing that needs the VM,
                    // and it can only follow an adopt this same system made.
                    poll_area_spirit_healer
                        .after(crate::target::TargetUpdate)
                        .run_if(crate::ui_script::ingame_ui_up)
                        .in_set(crate::char_select::InWorldGated),
                    // In-world only: the claim is about the VM, but the query is a world
                    // packet, and the boot VM exists at the glue screen too.
                    meeting_stone_enter_world
                        .in_set(crate::ui_script::UiFeed)
                        .before(feed_meeting_stone)
                        .in_set(crate::char_select::InWorldGated),
                    feed_meeting_stone.in_set(UiFeed),
                    // **Before the target chain, not merely after the input pass** — and this one
                    // is a correctness order, not a tidiness one.
                    //
                    // `drain_latch_verbs` takes `AcceptAreaSpiritHeal`'s presses with
                    // `take_area_spirit_accepts()` **unconditionally**, and only then gates the
                    // send on `spirit.healer()`. Both systems that can *clear* that healer —
                    // `poll_area_spirit_healer` (which drops it past the 22 yd retain radius) and
                    // `act_on_right_click` (which re-points it at another guide) — live in or
                    // after `TargetUpdate`, and `.after(UiInput)` alone constrains this against
                    // neither: `UiInput` sits *before* `WorldStage::Input` and `TargetUpdate`
                    // after it. So with the order undeclared, a ghost who clicks **Accept** on the
                    // same frame they step out of range either sends
                    // `CMSG_AREA_SPIRIT_HEALER_QUEUE` or has the press silently eaten — decided by
                    // graph layout, with no message either way.
                    //
                    // `before` is the reference's own answer, not a coin toss: the dialog's Lua
                    // handler runs inside the UI dispatch, while `0x4923b0` (the poll) and
                    // `0x48184a` (the pick) both run later in the same frame off `CGWorldFrame`'s
                    // OnUpdate. The press is resolved against the healer the frame *began* with.
                    // Its neighbour `drain_meeting_stone_joins` below argued the same hazard and
                    // declared its way out of it; this one was missed.
                    drain_latch_verbs
                        .after(UiInput)
                        .before(crate::target::TargetUpdate),
                    drain_queue_verbs.after(UiInput),
                    // MEETINGSTONE(23)'s use slot (2283): the click resolved the object, this
                    // runs the validator and sends. Ordered after the **target chain**, not just
                    // after the input pass like its neighbours — `UiInput` sits *before*
                    // `WorldStage::Input` and `TargetUpdate` after it, so "after UiInput" alone
                    // says nothing about the writer and would let the join drift a frame. A
                    // message survives that (two-frame lifetime) and 16 ms would not be visible,
                    // but a click's own packet should not leave on an undefined frame.
                    drain_meeting_stone_joins
                        .after(UiInput)
                        .after(crate::target::TargetUpdate),
                ),
            )
            .add_systems(
                OnExit(crate::char_select::ClientState::InWorld),
                (meeting_stone_leave_world, area_spirit_healer_leave_world),
            );
    }
}

/// The area spirit healer's aura as the test below names it — the same 2584 as
/// [`AREA_SPIRIT_HEALER_AURA`], kept under its own name because the test is about
/// `CancelAreaSpiritHeal`'s spell and the constant above is about `0x4921c0`'s.
#[cfg(test)]
const AREA_SPIRIT_HEALER_SPELL: u32 = AREA_SPIRIT_HEALER_AURA;

/// The generic cancel-aura routine's refusal (`0x6e7040`): it returns without sending when the
/// spell's `AttributesEx` has bit 13 set and bit 2
/// clear **and** `0x5ee290(player)` holds. Whether the third leg ever matters for spell 2584 is
/// decided by the first two, read off the shipped Spell.dbc in [`tests::spell_2584_never_trips_the_cancel_gate`].
#[cfg(test)]
fn cancel_gate_could_apply(attributes_ex: u32) -> bool {
    attributes_ex & 0x2000 != 0 && attributes_ex & 0x4 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `UNIT_FIELD_LEVEL`, absolute field 34.
    const LEVEL_FIELD: u16 = 34;

    /// **`0x4921c0`'s early return guards everything.** The first two compares
    /// (`0x4921cf cmp ecx,eax` / `0x4921dd cmp edx,ecx`) jump straight to the epilogue at
    /// `0x492282` when the guid is unchanged — so a repeat call sends no packet, cancels no aura
    /// and does **not** clear the pending deadline. That last one is the part a paraphrase loses,
    /// and it is what lets the per-frame poll call this routine unconditionally.
    #[test]
    fn setting_the_same_healer_is_a_complete_no_op() {
        let mut spirit = AreaSpiritHealer::default();
        assert_eq!(
            spirit.set_healer(Some(7)),
            SetHealerOutcome {
                cancel_aura: false,
                query: Some(7)
            },
            "a fresh adopt asks for the clock"
        );
        let now = Instant::now();
        spirit.on_time(7, 30_000, now);
        assert_eq!(spirit.secs(now), 30, "the wave clock is armed");

        assert_eq!(
            spirit.set_healer(Some(7)),
            SetHealerOutcome::default(),
            "the same guid again: no query, no cancel"
        );
        assert_eq!(
            spirit.secs(now),
            30,
            "and the deadline SURVIVES — 0x492211's clear sits past the early return"
        );
    }

    /// The change legs, in the order the routine runs them: a pending deadline is cancelled
    /// (`0x4921fc`, which is how walking out of range fires `_OUT_OF_RANGE` with no Lua call),
    /// the deadline is zeroed unconditionally (`0x492211`), and only a **non-zero** new guid
    /// sends (`0x492217`'s `je`).
    #[test]
    fn dropping_a_healer_cancels_the_wave_and_sends_nothing() {
        let mut spirit = AreaSpiritHealer::default();
        spirit.set_healer(Some(7));
        let now = Instant::now();
        spirit.on_time(7, 30_000, now);

        assert_eq!(
            spirit.set_healer(None),
            SetHealerOutcome {
                cancel_aura: true,
                query: None
            },
            "walking away: the aura cancel, and no query for a zero guid"
        );
        assert_eq!(spirit.secs(now), 0, "the clock is zeroed");
        assert_eq!(spirit.healer(), None);

        // And a drop with no clock armed cancels nothing.
        spirit.set_healer(Some(9));
        assert_eq!(
            spirit.set_healer(None),
            SetHealerOutcome {
                cancel_aura: false,
                query: None
            },
            "no deadline pending — 0x4921fa's `je` skips the cancel"
        );
    }

    /// `SMSG_AREA_SPIRIT_HEALER_TIME` is addressed: a clock for a healer that is **not** the
    /// cached one is dropped (`0x4922a0 cmp eax,[ebp+8]`), which is what keeps a stale reply from
    /// a graveyard you already left out of the dialog.
    #[test]
    fn a_clock_for_another_healer_is_ignored() {
        let mut spirit = AreaSpiritHealer::default();
        spirit.set_healer(Some(7));
        let now = Instant::now();
        spirit.on_time(8, 30_000, now);
        assert_eq!(spirit.secs(now), 0, "not our healer");
        spirit.on_time(7, 0, now);
        assert_eq!(spirit.secs(now), 0, "a zero time arms nothing");
        spirit.on_time(7, 25_000, now);
        assert_eq!(spirit.secs(now), 25);
    }

    /// **The click's cache-bust.** `0x5df950` calls the setter twice — `(0,0)` then the guid —
    /// precisely so the second call is never swallowed by the unchanged-guid early return. So
    /// clicking the guide you are already standing next to DOES re-ask for the clock, which is
    /// the behaviour a player relies on after dismissing the dialog.
    #[test]
    fn clicking_the_cached_guide_still_asks_again() {
        let mut spirit = AreaSpiritHealer::default();
        spirit.set_healer(Some(7));
        let now = Instant::now();
        spirit.on_time(7, 30_000, now);

        let outcome = spirit.click_guide(7);
        assert_eq!(
            outcome.query,
            Some(7),
            "the bust makes the second call a real change"
        );
        assert!(
            outcome.cancel_aura,
            "and the bust's own leg cancelled the pending wave"
        );
        assert_eq!(spirit.healer(), Some(7));
    }

    /// `UNIT_FIELD_FLAGS`, absolute field 46 — `UNIT_FLAG_PVP` (`0x1000`) is what carries a
    /// non-player-controlled unit through `can_assist`'s last gate.
    const UNIT_FLAGS_FIELD: u16 = 46;
    /// `UNIT_NPC_FLAGS`, absolute field 147.
    const NPC_FLAGS_FIELD: u16 = 147;
    /// `PLAYER_FLAGS`, absolute field 190; bit `0x10` is GHOST.
    const PLAYER_FLAGS_FIELD: u16 = 190;

    fn fields(pairs: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs))
    }

    /// A world holding exactly what [`poll_area_spirit_healer`] reads, with the body at the origin
    /// and one candidate unit at `dist` yards along +X.
    ///
    /// `ghost` drives `PLAYER_FLAGS 0x10`; `guide` drives `UNIT_NPC_FLAGS` bit 6. The candidate
    /// carries `UNIT_FLAG_PVP` and faction template **35** ("friendly to all"), against a body on
    /// template **1** (PLAYER, Human) — a real friendly pair out of the shipped DBC, so
    /// `can_assist` passes for the same reason it passes on the live server rather than by
    /// accident. A test that wants the acquire walk to *run* must therefore hand over the real
    /// catalog; `None` serves the cases that refuse **before** the walk (not a ghost, not a guide,
    /// too far), which is why those need no client data and never skip.
    fn poll_world(
        ghost: bool,
        guide: bool,
        dist: f32,
        factions: Option<benilla_formats::FactionCatalog>,
    ) -> (World, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<AreaSpiritHealer>();
        world.init_resource::<crate::net::Reputations>();
        world.init_resource::<crate::net::GuidIndex>();
        if let Some(catalog) = factions {
            world.insert_resource(crate::target::Factions::from_catalog(catalog));
        }
        world.spawn((
            SelfPlayer,
            fields(&[
                (PLAYER_FLAGS_FIELD, if ghost { 0x10 } else { 0 }),
                (FACTION_TEMPLATE_FIELD, 1),
            ]),
            Transform::from_xyz(0.0, 0.0, 0.0),
        ));
        world.spawn((
            crate::net::Guid(GUIDE),
            crate::net::NetEntity {
                kind: benilla_protocol::EntityKind::Unit,
                display_id: None,
                scale: 1.0,
            },
            fields(&[
                (NPC_FLAGS_FIELD, if guide { 1 << 6 } else { 0 }),
                (UNIT_FLAGS_FIELD, 0x1000),
                (FACTION_TEMPLATE_FIELD, 35),
            ]),
            Transform::from_xyz(dist, 0.0, 0.0),
        ));
        (world, rx)
    }

    /// `UNIT_FIELD_FACTIONTEMPLATE`, absolute field 35.
    const FACTION_TEMPLATE_FIELD: u16 = 35;

    /// The shipped FactionTemplate.dbc, or an early return when this machine has no client data —
    /// the same `wow_data_or_skip!` shape `target::ring`'s own reaction tests use.
    macro_rules! catalog_or_skip {
        () => {{
            let data = benilla_formats::wow_data_or_skip!();
            let mut chain = benilla_formats::open_chain(&data).expect("open chain");
            benilla_formats::load_faction_catalog(&mut chain).expect("FactionTemplate.dbc")
        }};
    }

    const GUIDE: u64 = 0x9111;

    fn run_poll(world: &mut World) {
        use bevy::ecs::system::RunSystemOnce;
        world.run_system_once(poll_area_spirit_healer).unwrap();
    }

    fn queries(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<u64> {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::AreaSpiritHealerQuery { healer } => Some(healer),
                _ => None,
            })
            .collect()
    }

    /// **The ghost gate is the poll's first test** (`0x4923b0`, the `0x5df74a`-shaped read of
    /// `[[player+0xe68]+8] bit 4`). A living body standing on top of a spirit guide adopts nothing
    /// and sends nothing — which is what keeps the resurrect dialog off the screen of everyone who
    /// is merely walking through a graveyard.
    #[test]
    fn a_living_body_never_adopts_a_spirit_guide() {
        let (mut world, rx) = poll_world(false, true, 1.0, None);
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), None);
        assert!(queries(&rx).is_empty(), "no clock is asked for");
    }

    /// The acquire leg: a ghost inside 20 yd adopts, and the adopt is what sends
    /// `CMSG_AREA_SPIRIT_HEALER_QUERY` — the packet that was unreachable before 2291.
    #[test]
    fn a_ghost_adopts_a_guide_inside_the_acquire_radius_and_asks_for_the_clock() {
        let catalog = catalog_or_skip!();
        let (mut world, rx) = poll_world(true, true, 19.0, Some(catalog));
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), Some(GUIDE));
        assert_eq!(
            queries(&rx),
            vec![GUIDE],
            "exactly one query, for that guide"
        );
    }

    /// …and 20 yd is a real boundary, not decoration: one yard further and nothing is adopted.
    #[test]
    fn a_guide_past_the_acquire_radius_is_not_adopted() {
        let (mut world, rx) = poll_world(true, true, 21.0, None);
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), None);
        assert!(queries(&rx).is_empty());
    }

    /// **The hysteresis, which is the whole reason there are two radii.** A guide adopted at 19 yd
    /// is RETAINED at 21 — past the acquire radius — and only dropped past 22
    /// (`20.0 × 1.1`). Equal radii would re-query every frame a body jittered over the boundary.
    /// The drop is not silent: it cancels the wave (`CMSG_CANCEL_AURA` on 2584), which on vmangos
    /// takes the player out of the resurrect queue.
    #[test]
    fn an_adopted_guide_is_retained_past_the_acquire_radius_and_dropped_past_the_retain_one() {
        let catalog = catalog_or_skip!();
        let (mut world, rx) = poll_world(true, true, 19.0, Some(catalog));
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), Some(GUIDE));
        let _ = queries(&rx);

        // 21 yd: outside acquire, inside retain — kept, and no second query.
        let guide = world
            .query_filtered::<Entity, With<crate::net::Guid>>()
            .iter(&world)
            .next()
            .expect("the guide entity");
        world
            .entity_mut(guide)
            .insert(Transform::from_xyz(21.0, 0.0, 0.0));
        run_poll(&mut world);
        assert_eq!(
            world.resource::<AreaSpiritHealer>().healer(),
            Some(GUIDE),
            "retained between the two radii"
        );
        assert!(queries(&rx).is_empty(), "a retained guide is not re-asked");

        // 23 yd: outside both — dropped.
        world
            .entity_mut(guide)
            .insert(Transform::from_xyz(23.0, 0.0, 0.0));
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), None);
    }

    /// The unit filter is the `SPIRITGUIDE` bit and nothing else. A unit standing in the same spot
    /// without it — a battle master, a herald, another player's corpse-side NPC — is skipped.
    #[test]
    fn a_unit_without_the_spiritguide_flag_is_never_adopted() {
        let (mut world, rx) = poll_world(true, false, 1.0, None);
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), None);
        assert!(queries(&rx).is_empty());
    }

    /// Resurrecting drops the guide. The poll is **level**-triggered, so the release happens on the
    /// first frame the ghost flag clears — with the guide still standing right there.
    #[test]
    fn losing_the_ghost_state_drops_the_guide() {
        let catalog = catalog_or_skip!();
        let (mut world, _rx) = poll_world(true, true, 5.0, Some(catalog));
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), Some(GUIDE));

        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .iter(&world)
            .next()
            .expect("the body");
        world
            .entity_mut(me)
            .insert(fields(&[(PLAYER_FLAGS_FIELD, 0)]));
        run_poll(&mut world);
        assert_eq!(
            world.resource::<AreaSpiritHealer>().healer(),
            None,
            "alive again: the cache clears even though the guide has not moved"
        );
    }

    /// The four client-side refusals of MEETINGSTONE(23)'s use slot (`0x5f69d0`, decision 2283),
    /// in the reference's own order — and the pass that reaches `CMSG 0x292`.
    #[test]
    fn the_meeting_stone_join_refuses_in_the_references_order() {
        use crate::go_templates::MeetingStoneTemplate;
        const ME: u64 = 0x5e1f;
        const MATE: u64 = 0xa11e;
        // A stone anybody 15-60 may use — the shape most of 1.12's dungeon stones carry.
        let stone = Some(MeetingStoneTemplate {
            min_level: 15,
            max_level: 60,
            area: 1519,
        });
        let party = |leader: u64, others: usize, group_type: u8| GroupState {
            in_group: true,
            group_type,
            leader,
            members: (0..others)
                .map(|i| benilla_protocol::messages::GroupMemberEntry {
                    name: format!("Mate{i}"),
                    guid: 0xb000 + i as u64,
                    status: 1,
                    flags: 0,
                })
                .collect(),
            ..GroupState::default()
        };

        // Solo, in range of the level band: nothing refuses.
        assert_eq!(
            meeting_stone_join_refusal(None, Some(ME), Some(40), stone),
            StoneJoin::Send
        );
        // Leading a party of FOUR (three others): still fine — a full party is five, and the
        // stone's job is to find the fifth. This is the assertion the `+ 1` bug failed.
        assert_eq!(
            meeting_stone_join_refusal(Some(&party(ME, 3, 0)), Some(ME), Some(40), stone),
            StoneJoin::Send
        );

        // 1 — in a party someone else leads.
        assert_eq!(
            meeting_stone_join_refusal(Some(&party(MATE, 1, 0)), Some(ME), Some(40), stone),
            StoneJoin::Refuse("ERR_MEETING_STONE_MUST_BE_LEADER")
        );
        // 2 — leading a FULL party: four others, five including us, which is what
        // `Group::IsFull()` refuses server-side too.
        assert_eq!(
            meeting_stone_join_refusal(Some(&party(ME, 4, 0)), Some(ME), Some(40), stone),
            StoneJoin::Refuse("ERR_MEETING_STONE_GROUP_FULL")
        );
        // 3 — the level band, both ends, inclusive.
        assert_eq!(
            meeting_stone_join_refusal(None, Some(ME), Some(14), stone),
            StoneJoin::Refuse("ERR_MEETING_STONE_INVALID_LEVEL")
        );
        assert_eq!(
            meeting_stone_join_refusal(None, Some(ME), Some(61), stone),
            StoneJoin::Refuse("ERR_MEETING_STONE_INVALID_LEVEL")
        );
        assert_eq!(
            meeting_stone_join_refusal(None, Some(ME), Some(15), stone),
            StoneJoin::Send
        );
        assert_eq!(
            meeting_stone_join_refusal(None, Some(ME), Some(60), stone),
            StoneJoin::Send
        );
        // 4 — a raid. Its leader is refused too, which is what puts it BELOW the leader term.
        assert_eq!(
            meeting_stone_join_refusal(Some(&party(ME, 1, 1)), Some(ME), Some(40), stone),
            StoneJoin::Refuse("ERR_MEETING_STONE_NO_RAID_GROUP")
        );

        // The order is observable where two terms hold at once: a raid we do not lead answers
        // MUST_BE_LEADER, not NO_RAID_GROUP.
        assert_eq!(
            meeting_stone_join_refusal(Some(&party(MATE, 4, 1)), Some(ME), Some(40), stone),
            StoneJoin::Refuse("ERR_MEETING_STONE_MUST_BE_LEADER")
        );

        // **An unanswered template refuses**, because `0x5f8150` reads `0/0` off an uncached
        // object and every level >= 1 is outside `0..=0`. The permissive "skip the term while the
        // query is in flight" default the highlight column takes is not this slot's — the first
        // cut of this code had it.
        assert_eq!(
            meeting_stone_join_refusal(None, Some(ME), Some(5), None),
            StoneJoin::Refuse("ERR_MEETING_STONE_INVALID_LEVEL")
        );
        // The same arithmetic on a shipped `0/0` band; `0/60` is the open one.
        let closed = Some(MeetingStoneTemplate {
            min_level: 0,
            max_level: 0,
            area: 1519,
        });
        let open = Some(MeetingStoneTemplate {
            min_level: 0,
            max_level: 60,
            area: 1519,
        });
        assert_eq!(
            meeting_stone_join_refusal(None, Some(ME), Some(1), closed),
            StoneJoin::Refuse("ERR_MEETING_STONE_INVALID_LEVEL")
        );
        assert_eq!(
            meeting_stone_join_refusal(None, Some(ME), Some(60), open),
            StoneJoin::Send
        );

        // `0x5f69f8` — no active player: SILENT, and not one of the four refusals. Neither a
        // missing guid nor a level the store has not streamed reaches a message or a packet.
        assert_eq!(
            meeting_stone_join_refusal(None, Some(ME), None, stone),
            StoneJoin::Silent
        );
        assert_eq!(
            meeting_stone_join_refusal(None, None, Some(40), stone),
            StoneJoin::Silent
        );
    }

    /// The join drain end to end: a clicked stone the player may use puts `CMSG 0x292` on the
    /// wire with that object's guid, and a refused one puts **nothing** there.
    #[test]
    fn the_join_drain_sends_the_clicked_stone_and_refuses_silently_on_the_wire() {
        use crate::go_templates::GameObjectTemplates;
        // A real GameObject guid: `counter | (entry << 24) | (HIGH_GAMEOBJECT << 48)` — the
        // template cache is keyed by the entry the guid carries, so a made-up number would
        // silently miss and skip the level term.
        const STONE_ENTRY: u32 = 0x5701;
        const STONE: u64 = 0xF110 << 48 | (STONE_ENTRY as u64) << 24 | 0x22;
        const ME: u64 = 0x5e1f;
        const MATE: u64 = 0xa11e;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut templates = GameObjectTemplates::default();
        let mut data = [0i32; 24];
        (data[0], data[1], data[2]) = (15, 60, 1519);
        assert_eq!(benilla_protocol::guid::entry(STONE), Some(STONE_ENTRY));
        templates.insert(STONE_ENTRY, 23, "Stonard Meeting Stone".into(), &data);
        app.insert_resource(templates)
            .insert_resource(NetCommands(tx))
            .insert_resource(SelfGuid(Some(ME)))
            .init_resource::<GroupState>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_message::<MeetingStoneUse>()
            .add_systems(Update, drain_meeting_stone_joins);
        app.insert_non_send_resource(UiScript::new().expect("VM"));
        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                LEVEL_FIELD,
                40,
            )])),
        ));

        let joins = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::MeetingStoneJoin { go_guid } => Some(go_guid),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        app.world_mut()
            .write_message(MeetingStoneUse { go_guid: STONE });
        app.update();
        assert_eq!(joins(&rx), vec![STONE], "the clicked stone's own guid");

        // Now in a party led by someone else: the same click sends nothing at all.
        app.world_mut().resource_mut::<GroupState>().in_group = true;
        app.world_mut().resource_mut::<GroupState>().leader = MATE;
        app.world_mut()
            .write_message(MeetingStoneUse { go_guid: STONE });
        app.update();
        assert!(
            joins(&rx).is_empty(),
            "a refusal is a local line, never a packet"
        );
    }

    /// **The `/reload` re-query** — the meeting-stone half of 1290's class, and the reference's
    /// own behaviour rather than an invention of ours.
    ///
    /// The run-once byte `[0xb4b424]` is cleared by the UI teardown at `0x490a8d`, and
    /// `UI_Init` re-runs the bring-up and re-sends `CMSG 0x296`. So the gate is once per **VM**,
    /// not once per world session — and against the old `MessageReader<EnteredWorldMessage>`
    /// shape this fails on the second VM, which is exactly the reported symptom: the queued
    /// player's minimap icon never comes back after a `/reload`.
    ///
    /// **A registered schedule, not `run_system_once`**: the gate is a `Local<VmMemo<bool>>`, and
    /// `run_system_once` builds a fresh system — and so a fresh `Local` — on every call, which
    /// would make the claim look unclaimed every frame (the same trap `ui_loot`'s tests name).
    #[test]
    fn a_rebuilt_vm_re_asks_the_server_for_the_meeting_stone_queue() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<MeetingStone>()
            .insert_resource(NetCommands(tx))
            .add_systems(Update, meeting_stone_enter_world);

        let queries = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
            rx.try_iter()
                .filter(|c| matches!(c, ClientCommand::MeetingStoneStatusQuery))
                .count()
        };

        // No VM yet — the glue screen: nothing is asked.
        app.update();
        assert_eq!(queries(&rx), 0, "no VM, no bring-up");

        // The first VM: the bring-up runs once, however many frames pass.
        app.insert_non_send_resource(UiScript::new().expect("VM"));
        app.update();
        assert_eq!(queries(&rx), 1, "the first VM asks the server");
        app.update();
        app.update();
        assert_eq!(queries(&rx), 0, "…and does not ask again on later frames");

        // The server answers: the player IS queued, and the host holds that.
        app.world_mut().resource_mut::<MeetingStone>().area = 1519;
        app.world_mut().resource_mut::<MeetingStone>().dirty = false;

        // `ReloadUI()`: a fresh VM, and nothing on the wire.
        app.insert_non_send_resource(UiScript::new().expect("VM"));
        app.update();
        assert_eq!(
            queries(&rx),
            1,
            "a rebuilt VM re-asks — the reference's UI teardown clears the run-once byte, so \
             `UI_Init` re-sends `CMSG 0x296` (0x490a8d / 0x490168)"
        );

        let stone = app.world().resource::<MeetingStone>();
        assert!(
            stone.dirty,
            "the bring-up re-armed the push, so the fresh VM is told the queued area again"
        );
        assert_eq!(
            stone.area, 1519,
            "the queued area itself is untouched, matching `[0xb72038]` surviving the reload"
        );
    }

    /// Spell 2584's flags off the shipped Spell.dbc: the cancel-aura gate's first two legs.
    #[test]
    fn spell_2584_never_trips_the_cancel_gate() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let ex = catalog
            .get(AREA_SPIRIT_HEALER_SPELL)
            .map(|d| d.attributes_ex)
            .expect("spell 2584 in Spell.dbc");
        assert!(
            !cancel_gate_could_apply(ex),
            "spell 2584 AttributesEx = {ex:#x}: the gate's third leg would decide, and it is uncarved"
        );
    }

    #[test]
    fn the_boot_clock_arms_on_a_delay_and_clears_on_zero_with_the_reason() {
        let mut boot = InstanceBoot::default();
        let now = Instant::now();
        boot.apply(60_500, 0, now);
        assert_eq!(boot.events, vec!["INSTANCE_BOOT_START"]);
        assert_eq!(boot.secs(now), 60, "whole seconds, truncated");
        boot.apply(0, 1, now);
        assert_eq!(
            boot.events,
            vec!["INSTANCE_BOOT_START", "INSTANCE_BOOT_STOP"]
        );
        assert_eq!(boot.errors, vec!["ERR_RAID_GROUP_ONLY"]);
        assert_eq!(boot.secs(now), 0);
        boot.apply(0, 7, now);
        assert_eq!(boot.errors.len(), 1, "a reason outside 1/2 names nothing");
    }

    #[test]
    fn the_spirit_healer_clock_only_arms_for_the_cached_healer() {
        let mut spirit = AreaSpiritHealer::default();
        let now = Instant::now();
        spirit.on_time(0x77, 30_000, now);
        assert!(!spirit.in_range, "no healer cached: the packet is ignored");
        spirit.healer = Some(0x77);
        spirit.on_time(0x78, 30_000, now);
        assert!(!spirit.in_range, "another guid: ignored");
        spirit.on_time(0x77, 0, now);
        assert!(!spirit.in_range, "a zero time: ignored");
        spirit.on_time(0x77, 30_000, now);
        assert!(spirit.in_range);
        assert_eq!(spirit.secs(now), 30);
    }

    #[test]
    fn the_queue_keeps_three_slots_and_answers_a_port_by_map() {
        let mut q = BattlefieldQueue::default();
        let status = |slot, map_id| BattlefieldStatus {
            slot,
            map_id,
            bracket: 0,
            instance_id: 0,
            status: 2,
            time_ms: Some(0),
            in_progress: None,
            queued: None,
        };
        q.apply(status(1, 489));
        q.apply(status(5, 30));
        assert_eq!(
            q.map_id(2),
            Some(489),
            "1-based from Lua, 0-based on the wire"
        );
        assert_eq!(q.map_id(1), None);
        assert_eq!(q.map_id(4), None);
        q.apply(status(1, 0));
        assert_eq!(q.map_id(2), None, "a zero map clears the slot");
    }

    /// The instance clocks (`0x4aa850`): stamped by status 3, zeroed by any other status of ANY
    /// slot, and by a clear of the active slot only — which leaves the active index alone.
    #[test]
    fn the_instance_clocks_follow_the_status_handler() {
        let mut q = BattlefieldQueue::default();
        let now = Instant::now();
        let mut active = BattlefieldStatus {
            slot: 0,
            map_id: 489,
            bracket: 0,
            instance_id: 3,
            status: 3,
            time_ms: None,
            in_progress: Some((90_000, 30_000)),
            queued: None,
        };
        q.apply_at(active.clone(), now);
        assert_eq!(q.active_map(), Some(489));
        assert_eq!(q.instance_expiration_ms(now), 90_000);
        assert_eq!(q.run_time_ms(now), 30_000);
        assert!(q.take_score_dirty());
        // A QUEUED update for slot 2 zeroes both clocks — the handler's unconditional arm.
        let mut queued = active.clone();
        queued.slot = 1;
        queued.map_id = 529;
        queued.status = 1;
        queued.in_progress = None;
        queued.queued = Some((60_000, 5_000));
        q.apply_at(queued, now);
        assert_eq!(
            q.active_map(),
            Some(489),
            "another slot's status leaves the active index"
        );
        assert_eq!(q.instance_expiration_ms(now), 0);
        assert_eq!(q.run_time_ms(now), 0);
        assert_eq!(q.map_id(2), Some(529));
        // Re-arm, then clear the ACTIVE slot: the clocks go, the index stays (`[0x8457cc]`).
        q.apply_at(active.clone(), now);
        active.map_id = 0;
        q.apply_at(active, now);
        assert_eq!(q.instance_expiration_ms(now), 0);
        assert_eq!(q.map_id(1), None);
        assert_eq!(
            q.active_map(),
            Some(489),
            "the clear arm never resets the active index"
        );
    }

    /// `0x295`'s store is unconditional and the old id is latched first; the feed reads both.
    #[test]
    fn the_stone_latches_the_old_area_before_storing_the_new() {
        let mut stone = MeetingStone::default();
        stone.apply(1519, 1);
        stone.apply(1519, 1);
        stone.apply(0, 0);
        stone.apply(12, 9);
        assert_eq!(stone.area, 12);
        assert_eq!(stone.updates, vec![(0, 1), (1519, 1), (1519, 0), (0, 9)]);
        stone.enter_world();
        assert_eq!(stone.text, StoneText::Unknown);
        assert_eq!(
            stone.area, 12,
            "the enter-world reset leaves the area alone"
        );
        stone.leave_world();
        assert_eq!(stone.text, StoneText::None);
    }

    /// The five-way table with its two asymmetries, and the rebuild's fallbacks and buffer.
    #[test]
    fn the_status_table_and_the_rebuild_follow_the_handler() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let areas =
            AreaTableRes(benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable"));
        let s = UiScript::new().unwrap();
        s.run(
            r#"MEETINGSTONE_TOOLTIP = "Looking for more for %s" UNKNOWN = "Unknown"
               ERR_MEETING_STONE_LEFT_QUEUE_S = "You are no longer queued for %s."
               ERR_MEETING_STONE_IN_QUEUE_S = "You are now in the queue to join a party for %s."
               ERR_MEETING_STONE_OTHER_MEMBER_LEFT = "left"
               ERR_MEETING_STONE_PARTY_KICKED_FROM_QUEUE = "kicked"
               ERR_MEETING_STONE_MEMBER_STILL_IN_QUEUE = "still""#,
        )
        .unwrap();
        let a = Some(&areas);
        let text = |l: Option<crate::ui_action::Shown>| l.map(|l| l.text().to_string());
        // Status 0 names the OLD area — and is silent when it has no row.
        assert_eq!(
            text(stone_line(&s, a, 1519, 0, 0)).as_deref(),
            Some("You are no longer queued for Stormwind City.")
        );
        assert_eq!(
            text(stone_line(&s, a, 0, 0, 0)),
            None,
            "no row for area 0: silent"
        );
        assert_eq!(text(stone_line(&s, a, 999_999, 0, 0)), None);
        // Status 1 names the NEW area, falls back to UNKNOWN, and is skipped when unchanged.
        assert_eq!(
            text(stone_line(&s, a, 0, 1519, 1)).as_deref(),
            Some("You are now in the queue to join a party for Stormwind City.")
        );
        assert_eq!(
            text(stone_line(&s, a, 0, 999_999, 1)).as_deref(),
            Some("You are now in the queue to join a party for Unknown.")
        );
        assert_eq!(
            text(stone_line(&s, a, 1519, 1519, 1)),
            None,
            "unchanged: skipped"
        );
        assert_eq!(text(stone_line(&s, a, 0, 0, 2)).as_deref(), Some("left"));
        assert_eq!(text(stone_line(&s, a, 0, 0, 3)).as_deref(), Some("kicked"));
        assert_eq!(text(stone_line(&s, a, 0, 0, 4)).as_deref(), Some("still"));
        assert_eq!(
            text(stone_line(&s, a, 0, 0, 5)),
            None,
            "past the table: nothing"
        );
        // The rebuild.
        assert_eq!(
            build_stone_text(&s, a, 1519),
            "Looking for more for Stormwind City"
        );
        assert_eq!(build_stone_text(&s, a, 0), "Looking for more for Unknown");
        s.run("MEETINGSTONE_TOOLTIP = string.rep('x', 300) .. '%s'")
            .unwrap();
        assert_eq!(
            build_stone_text(&s, a, 1519).len(),
            STONE_TEXT_BYTES,
            "the 256-byte buffer"
        );
        s.run("MEETINGSTONE_TOOLTIP = nil").unwrap();
        assert_eq!(
            build_stone_text(&s, a, 1519),
            "",
            "a missing template is GetText's empty string, not the unreachable %s fallback"
        );
    }

    #[test]
    fn the_pet_question_latches_and_closes_like_the_talent_wipe() {
        let mut pet = PetUnlearnState::default();
        pet.ask(0x2b, 10_000);
        assert_eq!(pet.pending(), Some(0x2b));
        assert!(pet.ask);
        pet.close();
        assert_eq!(pet.pending(), None);
        assert_eq!(pet.cost, 0);
    }

    /// **The queue is zeroed at every login, not kept across it**: module init `0x4a9c40`, run
    /// from `InitializeGame`, zeroes the three slots, the active index (`[0x8457cc] = -1`) and the
    /// scalars, and vmangos never sends a clear for a queue whose player logged out. So a slot,
    /// the active map and the instance clocks from the last session must not reach the next —
    /// only an in-session world enter (`0x4a9db0`) keeps the slots, and that is
    /// `ui_battlefield`'s, untouched here.
    #[test]
    fn the_session_end_zeroes_the_battlefield_queue() {
        let mut app = App::new();
        app.init_resource::<BattlefieldQueue>();
        net::register(&mut app);
        let now = Instant::now();
        {
            let mut q = app.world_mut().resource_mut::<BattlefieldQueue>();
            q.apply_at(
                BattlefieldStatus {
                    slot: 0,
                    map_id: 489,
                    bracket: 0,
                    instance_id: 3,
                    status: 3,
                    time_ms: None,
                    in_progress: Some((90_000, 30_000)),
                    queued: None,
                },
                now,
            );
            q.apply_at(
                BattlefieldStatus {
                    slot: 1,
                    map_id: 30,
                    bracket: 0,
                    instance_id: 0,
                    status: 1,
                    time_ms: Some(0),
                    in_progress: None,
                    queued: None,
                },
                now,
            );
        }

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::Disconnected {
                reason: "logged out".into(),
                end: benilla_protocol::SessionEnd::LoggedOut,
            }],
        );

        let mut q = app.world_mut().resource_mut::<BattlefieldQueue>();
        assert!(
            q.slots().iter().all(Option::is_none),
            "all three slots empty"
        );
        assert_eq!(q.active_map(), None);
        assert_eq!(q.run_time_ms(now), 0);
        assert_eq!(q.instance_expiration_ms(now), 0);
        assert!(!q.take_score_dirty());
    }
}
