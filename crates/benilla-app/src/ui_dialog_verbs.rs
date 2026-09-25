//! The app half of the verbs behind the stock `StaticPopup.lua` dialogs: the packet feeds that
//! raise them and the drains behind their buttons, for the pet trainer's unlearn, the instance
//! boot countdown, the area spirit healer's wave, the battleground queue and the meeting stone.
//! A meeting stone's click runs the stone's own use slot, which refuses a join client-side or
//! sends `CMSG_MEETINGSTONE_JOIN`, never the shared `CMSG_GAMEOBJ_USE`.

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
    events: Vec<&'static str>,
    errors: Vec<&'static str>,
}

impl InstanceBoot {
    /// `SMSG_RAID_GROUP_ONLY`: a positive delay arms, zero clears; an event fires either way.
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

    /// Whole seconds left, 0 when idle or past, as the reference divides a clamped ms remainder.
    pub(crate) fn secs(&self, now: Instant) -> u32 {
        self.deadline
            .map(|d| d.saturating_duration_since(now).as_secs())
            .map_or(0, |s| u32::try_from(s).unwrap_or(u32::MAX))
    }
}

/// `UNIT_NPC_FLAGS` bit 6, `SPIRITGUIDE`, which the acquire callback `0x4924c0` tests (`0x4924fc`).
const NPC_FLAG_SPIRITGUIDE: u32 = 1 << 6;

/// The area spirit healer's aura, cancelled by `0x4921c0` and `CancelAreaSpiritHeal`; `0x6e7040`
/// fires `AREA_SPIRIT_HEALER_OUT_OF_RANGE` for this spell id alone (`0x6e70b6`).
pub(crate) const AREA_SPIRIT_HEALER_AURA: u32 = 2584;

/// The acquire radius, `[0x8044d0]` = 20.0, compared squared in the enumerate callback `0x4924c0`.
const SPIRIT_GUIDE_ACQUIRE_YD: f32 = 20.0;
/// The retain radius, the acquire radius times `[0x804580]` = 1.1 (`0x492406`): a healer adopted
/// inside 20 yd is kept until 22.
const SPIRIT_GUIDE_RETAIN_YD: f32 = SPIRIT_GUIDE_ACQUIRE_YD * 1.1;

/// What [`AreaSpiritHealer::set_healer`] owes the frame; the reference does both in `0x4921c0`.
#[derive(Default, PartialEq, Eq, Debug)]
pub(crate) struct SetHealerOutcome {
    /// A wave was pending and the healer changed (`0x4921fc`): `AREA_SPIRIT_HEALER_OUT_OF_RANGE`
    /// fires and `CMSG_CANCEL_AURA` goes out for 2584; this, not Lua, closes the dialog.
    pub(crate) cancel_aura: bool,
    /// A newly adopted non-zero healer: `CMSG_AREA_SPIRIT_HEALER_QUERY` with it.
    pub(crate) query: Option<u64>,
}

/// The current-area spirit healer (`[0xb4e330/334]`) and its wave clock (`[0xb4e338]`).
#[derive(Resource, Default)]
pub(crate) struct AreaSpiritHealer {
    /// Written only by [`Self::set_healer`], the reference's `0x4921c0`.
    healer: Option<u64>,
    deadline: Option<Instant>,
    in_range: bool,
}

impl AreaSpiritHealer {
    /// `SMSG_AREA_SPIRIT_HEALER_TIME`: arms the clock only for the cached healer (`0x4922a0`) and
    /// a positive time, and owes `AREA_SPIRIT_HEALER_IN_RANGE`.
    pub(crate) fn on_time(&mut self, healer: u64, ms: u32, now: Instant) {
        if self.healer == Some(healer) && ms > 0 {
            self.deadline = Some(now + std::time::Duration::from_millis(u64::from(ms)));
            self.in_range = true;
        }
    }

    /// `0x4921c0`: an unchanged guid returns before anything else (`0x4921cf`/`0x4921dd`), so the
    /// poll calls it every frame; a change cancels a pending wave (`0x4921fc`), zeroes the
    /// deadline (`0x492211`) and queries a non-zero healer (`0x492217`).
    pub(crate) fn set_healer(&mut self, new: Option<u64>) -> SetHealerOutcome {
        // The reference's zero guid is our `None`.
        let new = new.filter(|&g| g != 0);
        if self.healer == new {
            return SetHealerOutcome::default();
        }
        self.healer = new;
        let cancel_aura = self.deadline.take().is_some();
        if cancel_aura {
            // The cancel closes the dialog, so an `_IN_RANGE` still owed is stale.
            self.in_range = false;
        }
        SetHealerOutcome {
            cancel_aura,
            query: new,
        }
    }

    /// The spirit-guide click (`0x5df950`): `0x4921c0(0)` then `0x4921c0(guid)`, a cache-bust that
    /// makes the second call query even for the healer already cached.
    pub(crate) fn click_guide(&mut self, guid: u64) -> SetHealerOutcome {
        let bust = self.set_healer(None);
        let set = self.set_healer(Some(guid));
        SetHealerOutcome {
            cancel_aura: bust.cancel_aura || set.cancel_aura,
            query: set.query,
        }
    }

    /// The cached healer: `AcceptAreaSpiritHeal`'s guid and the poll's retain subject.
    pub(crate) fn healer(&self) -> Option<u64> {
        self.healer
    }

    fn secs(&self, now: Instant) -> u32 {
        self.deadline
            .map(|d| d.saturating_duration_since(now).as_secs())
            .map_or(0, |s| u32::try_from(s).unwrap_or(u32::MAX))
    }
}

/// The three battleground queue slots (`0xb6e9d0`, stride `0x20`), each stamped with the moment
/// its status landed. Kept across an in-session world enter (`0x4a9db0`), zeroed at session end.
#[derive(Resource, Default)]
pub(crate) struct BattlefieldQueue {
    slots: [Option<(BattlefieldStatus, Instant)>; 3],
    changed: bool,
    /// The slot the player is in (`[0x8457cc]`, set by status 3) and its map.
    active: Option<(usize, u32)>,
    /// The instance's run-time stamp (`[0xb6ebbc]`) and expiration (`[0xb6ebb8]`): set by status
    /// 3, zeroed by any other non-clearing status for any slot, as the handler `0x4aa850` does.
    run_started: Option<Instant>,
    instance_expiration: Option<Instant>,
    /// Status 3 fires `UPDATE_BATTLEFIELD_SCORE` before `_STATUS` (`0x4aaa5a`, then `0x4aab05`).
    score_dirty: bool,
    /// Tutorials the handler triggers (`0x2f` on queued, `0x30` on confirm), owed to the next feed.
    tutorials: Vec<u32>,
}

impl BattlefieldQueue {
    /// `SMSG_BATTLEFIELD_STATUS` (`0x4aa850`). A zero map clears the slot, and the instance clocks
    /// only if it was active, but leaves the active index (`[0x8457cc]`) alone.
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

    pub(crate) fn slots(&self) -> &[Option<(BattlefieldStatus, Instant)>; 3] {
        &self.slots
    }

    /// `GetBattlefieldInstanceExpiration()`: ms left on `[0xb6ebb8]`, 0 when unset or past.
    pub(crate) fn instance_expiration_ms(&self, now: Instant) -> u32 {
        self.instance_expiration.map_or(0, |d| {
            d.saturating_duration_since(now)
                .as_millis()
                .min(u128::from(u32::MAX)) as u32
        })
    }

    /// The map of the battleground the player is in, `LeaveBattlefield`'s payload (0 for `None`).
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

/// The status text (`[0xb7203c]`): none before world enter and after world leave, the bare
/// `UNKNOWN` until the server's `0x295` lands, then a built line; localized at push time.
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
    /// `[0xb72038]`, the queued area id, 0 for none.
    pub(crate) area: u32,
    text: StoneText,
    /// `0x295` arrivals as `(previous area, status)`; the handler stores the new area before it
    /// switches, so that one is already in `area`.
    updates: Vec<(u32, u8)>,
    notices: Vec<MeetingStoneNotice>,
    /// `0x299` guids whose name has not resolved yet.
    pending_members: Vec<u64>,
    /// The VM's copy of the two globals is stale.
    dirty: bool,
}

impl MeetingStone {
    /// `SMSG 0x295` (`0x4ca230`): the area is stored at once; the line and event wait for the feed.
    pub(crate) fn apply(&mut self, area: u32, status: u8) {
        let old = self.area;
        self.area = area;
        self.updates.push((old, status));
    }

    /// One of the four display-only replies.
    pub(crate) fn apply_notice(&mut self, notice: MeetingStoneNotice) {
        self.notices.push(notice);
    }

    /// The enter-world bring-up (`0x4c9f40`): the text becomes the bare `UNKNOWN`, the area stays.
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

/// A `GlobalStrings` value as the client's `GetText` reads it: `""` when the global is missing
/// (`0x882748`, the shared empty string, never NULL).
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

/// The `0x295` handler's five-way table (`0x4ca3a4`): status 0 names the old area and is silent
/// without its row, status 1 names the new one and is silent when unchanged, past 4 is silent.
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

/// The meeting-stone feed's other inputs, bundled to stay under Bevy's system-parameter limit.
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
            // Status 1's extra block (`0x4ca363`): `Effect_C` kind `0xc` on the local player,
            // then the Meeting Stones tutorial.
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
        // Every status, silent or out of range, rebuilds and fires. The globals reach the VM
        // first: the stock handler's first call is `IsInMeetingStoneQueue()` (`Minimap.xml:222`).
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
/// Once per VM, a [`crate::ui_script::VmMemo`] claim: the UI teardown clears the run-once byte
/// `[0xb4b424]` (`0x490a8d`) and `UI_Init` (`0x48fbf0`) re-runs the bring-up, so a `/reload`
/// re-asks the server; until the reply fires `MEETINGSTONE_CHANGED` (`0x4ca38f`) the minimap icon
/// is hidden, as in the reference. The queued area survives the reload.
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

/// A right-click on a `GAMEOBJECT_TYPE_MEETINGSTONE` (23) that passed the shared click gates.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MeetingStoneUse {
    pub(crate) go_guid: u64,
}

/// What the meeting stone's use slot `0x5f69d0` does with one click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoneJoin {
    /// No local player (`0x5f69f8`): no message and no packet.
    Silent,
    /// One of the four refusals (`0x496720`): an error-frame line with no sound, never chat.
    Refuse(&'static str),
    /// The tail `0x5f6af6`: `CMSG 0x292` with the stone's guid, and nothing else.
    Send,
}

/// The meeting stone's use slot `0x5f69d0`. With no local player it is silent (`0x5f69f8`);
/// otherwise four refusals in this order, the first two only in a group (`0x5f6a10`):
///
/// 1. not the leader (`0x5f6a2f`, the full 64-bit guid against `[0xbc75f8]`);
/// 2. four other members (`0x5f6a4f`, `GetNumPartyMembers` at `0x4e86d0`);
/// 3. a level outside `data[0]..=data[1]`, unsigned (`0x5f6ab4`);
/// 4. in a raid (`0x5f6ad3`, the raid member count `[0xb713e0]`).
///
/// vmangos repeats all but the level test (`LFGHandler.cpp:49-68`), which is the client's alone.
/// An uncached template reads 0/0 (`0x5f8150`), so it refuses every level.
pub(crate) fn meeting_stone_join_refusal(
    group: Option<&GroupState>,
    self_guid: Option<u64>,
    level: Option<u32>,
    stone: Option<crate::go_templates::MeetingStoneTemplate>,
) -> StoneJoin {
    let (Some(self_guid), Some(level)) = (self_guid, level) else {
        return StoneJoin::Silent;
    };
    let in_group = group.is_some_and(|g| g.in_group);
    if in_group {
        if group.map(|g| g.leader) != Some(self_guid) {
            return StoneJoin::Refuse("ERR_MEETING_STONE_MUST_BE_LEADER");
        }
        if group.is_some_and(|g| g.members.len() >= MEETING_STONE_PARTY_CAP) {
            return StoneJoin::Refuse("ERR_MEETING_STONE_GROUP_FULL");
        }
    }
    // An uncached template reads 0/0, which refuses.
    let (min_level, max_level) = stone.map_or((0, 0), |s| (s.min_level, s.max_level));
    if level < min_level || level > max_level {
        return StoneJoin::Refuse("ERR_MEETING_STONE_INVALID_LEVEL");
    }
    if in_group && group.is_some_and(|g| g.group_type == crate::ui_party::GROUPTYPE_RAID) {
        return StoneJoin::Refuse("ERR_MEETING_STONE_NO_RAID_GROUP");
    }
    StoneJoin::Send
}

/// Four other members is a full party: [`GroupState::members`] omits the player, so this is
/// vmangos's five (`Group/Group.h:49,232`) and the client's `>= 4` (`0x5f6a4f`), with no `+ 1`.
const MEETING_STONE_PARTY_CAP: usize = 4;

/// The join drain: the meeting stone's use slot, run with the click's guid.
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
        // Traced on the click's own `use` tag: a refusal, the no-player silence, or the send.
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

/// Drop the cached spirit guide when the world does, so no healer, wave or event reaches the
/// next session; the reference clears it only through `0x4921c0`, its world-leave path untraced.
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

/// The pet trainer's confirm and the spirit healer's accept, the two drains over a latch.
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

    // ConfirmPetUnlearn: `cost > coinage` shows ERR_NOT_ENOUGH_MONEY and sends nothing.
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

    // AcceptAreaSpiritHeal: the cached healer's guid, nothing without one.
    let accepts = script.take_area_spirit_accepts();
    if let Some(healer) = spirit.healer() {
        for _ in 0..accepts {
            let _ = commands
                .0
                .send(ClientCommand::AreaSpiritHealerQueue { healer });
        }
    }
}

/// The battleground port and the meeting-stone leave, the two drains over a queue.
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

    for (index, accept) in script.take_battlefield_port_requests() {
        if let Some(map_id) = queue.map_id(index) {
            let _ = commands
                .0
                .send(ClientCommand::BattlefieldPort { map_id, accept });
        }
    }

    // CancelMeetingStoneRequest: in a party we do not lead, ERR_MEETING_STONE_NOT_LEADER;
    // otherwise `0x293`, queued or not.
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

/// The area-spirit-healer poll `0x4923b0`, run every frame from `CGWorldFrame`'s `OnUpdate`
/// (`0x4818ca`): only a ghost (`[[player+0xe68]+8]` bit 4) holds a healer; a cached one is kept
/// within 22 yd, else the last unit in the walk that is a `SPIRITGUIDE`, assistable and within
/// 20 yd is adopted. Last, not nearest: the enumerate callback `0x4924c0` never stops the walk.
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
            // `0x6e7040(0xA18)`: the event with no argument, the packet with no guid.
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

    // Retain: a guid that no longer streams drops like one out of range (an `ObjectPtr` miss).
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

    // Acquire: `can_assist` is `0x6066f0`; its owner lookup wants a store by guid.
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
        adopted = Some(guid.0); // last wins: the callback never stops the walk
    }
    if let Some(guid) = adopted {
        apply(spirit.set_healer(Some(guid)));
    }
}

/// The dialog verbs' packet handlers: each parks a question or a countdown for the feed.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{AreaSpiritHealer, BattlefieldQueue, InstanceBoot, MeetingStone, PetUnlearnState};
    use crate::net::NetHandlerApp;

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

    /// Zero the battleground queue at session end, as module init `0x4a9c40` does at every login
    /// (`[0x8457cc]` to -1); vmangos sends no clear for a player who logged out.
    fn on_session_end(In(_): In<SessionEvent>, mut queue: ResMut<BattlefieldQueue>) {
        *queue = BattlefieldQueue::default();
    }

    /// The pet trainer's question; a zero guid shows `ERR_TALENT_WIPE_ERROR` instead.
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
                    // Gated on the in-game interface: vmangos can send `SMSG_RAID_GROUP_ONLY` in
                    // the login burst (`Player.cpp:18686`), and an `INSTANCE_BOOT_START` fired at
                    // the boot VM would be lost; the queue waits instead.
                    feed_dialog_verbs
                        .in_set(UiFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                    // After the mouse pick, the reference's order: `CGWorldFrame`'s OnUpdate picks
                    // at `0x48184a`, then polls at `0x4818ca`. Gating is safe: the poll is
                    // level-triggered and re-derives the cache every frame.
                    poll_area_spirit_healer
                        .after(crate::target::TargetUpdate)
                        .run_if(crate::ui_script::ingame_ui_up)
                        .in_set(crate::char_select::InWorldGated),
                    // In world only: the query is a world packet, and the glue screen has a VM.
                    meeting_stone_enter_world
                        .in_set(crate::ui_script::UiFeed)
                        .before(feed_meeting_stone)
                        .in_set(crate::char_select::InWorldGated),
                    feed_meeting_stone.in_set(UiFeed),
                    // Before the target chain, where the poll and the click can change the
                    // healer: the reference resolves an Accept press in the UI dispatch, before
                    // its pick and poll run later in the frame.
                    drain_latch_verbs
                        .after(UiInput)
                        .before(crate::target::TargetUpdate),
                    drain_queue_verbs.after(UiInput),
                    // After the target chain, which writes `MeetingStoneUse`, so the join leaves
                    // on the click's own frame.
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

/// `CancelAreaSpiritHeal`'s spell, the same 2584 as [`AREA_SPIRIT_HEALER_AURA`].
#[cfg(test)]
const AREA_SPIRIT_HEALER_SPELL: u32 = AREA_SPIRIT_HEALER_AURA;

/// The cancel-aura routine `0x6e7040` sends nothing when `AttributesEx` has bit 13 set, bit 2
/// clear and `0x5ee290(player)` holds; these are the first two terms.
#[cfg(test)]
fn cancel_gate_could_apply(attributes_ex: u32) -> bool {
    attributes_ex & 0x2000 != 0 && attributes_ex & 0x4 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `UNIT_FIELD_LEVEL`, absolute field 34.
    const LEVEL_FIELD: u16 = 34;

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

    /// `UNIT_FIELD_FLAGS`, absolute field 46; `UNIT_FLAG_PVP` (`0x1000`) passes `can_assist`.
    const UNIT_FLAGS_FIELD: u16 = 46;
    /// `UNIT_NPC_FLAGS`, absolute field 147.
    const NPC_FLAGS_FIELD: u16 = 147;
    /// `PLAYER_FLAGS`, absolute field 190; bit `0x10` is GHOST.
    const PLAYER_FLAGS_FIELD: u16 = 190;

    fn fields(pairs: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs))
    }

    /// What [`poll_area_spirit_healer`] reads: the body at the origin, one unit `dist` yards along
    /// +X. Faction templates 35 (friendly to all) and 1 (Human) are a real friendly pair, so the
    /// acquire walk needs the shipped catalog; `None` serves the cases refused before the walk.
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

    #[test]
    fn a_living_body_never_adopts_a_spirit_guide() {
        let (mut world, rx) = poll_world(false, true, 1.0, None);
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), None);
        assert!(queries(&rx).is_empty(), "no clock is asked for");
    }

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

    #[test]
    fn a_guide_past_the_acquire_radius_is_not_adopted() {
        let (mut world, rx) = poll_world(true, true, 21.0, None);
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), None);
        assert!(queries(&rx).is_empty());
    }

    #[test]
    fn an_adopted_guide_is_retained_past_the_acquire_radius_and_dropped_past_the_retain_one() {
        let catalog = catalog_or_skip!();
        let (mut world, rx) = poll_world(true, true, 19.0, Some(catalog));
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), Some(GUIDE));
        let _ = queries(&rx);

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

        // 23 yd: outside both.
        world
            .entity_mut(guide)
            .insert(Transform::from_xyz(23.0, 0.0, 0.0));
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), None);
    }

    #[test]
    fn a_unit_without_the_spiritguide_flag_is_never_adopted() {
        let (mut world, rx) = poll_world(true, false, 1.0, None);
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), None);
        assert!(queries(&rx).is_empty());
    }

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

    #[test]
    fn the_meeting_stone_join_refuses_in_the_references_order() {
        use crate::go_templates::MeetingStoneTemplate;
        const ME: u64 = 0x5e1f;
        const MATE: u64 = 0xa11e;
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

        assert_eq!(
            meeting_stone_join_refusal(None, Some(ME), Some(40), stone),
            StoneJoin::Send
        );
        // Leading a party of four, three others: not full.
        assert_eq!(
            meeting_stone_join_refusal(Some(&party(ME, 3, 0)), Some(ME), Some(40), stone),
            StoneJoin::Send
        );

        // 1: in a party someone else leads.
        assert_eq!(
            meeting_stone_join_refusal(Some(&party(MATE, 1, 0)), Some(ME), Some(40), stone),
            StoneJoin::Refuse("ERR_MEETING_STONE_MUST_BE_LEADER")
        );
        // 2: leading a full party, four others.
        assert_eq!(
            meeting_stone_join_refusal(Some(&party(ME, 4, 0)), Some(ME), Some(40), stone),
            StoneJoin::Refuse("ERR_MEETING_STONE_GROUP_FULL")
        );
        // 3: the level band, both ends inclusive.
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
        // 4: a raid, its leader included.
        assert_eq!(
            meeting_stone_join_refusal(Some(&party(ME, 1, 1)), Some(ME), Some(40), stone),
            StoneJoin::Refuse("ERR_MEETING_STONE_NO_RAID_GROUP")
        );

        // Where two terms hold, the leader term answers first.
        assert_eq!(
            meeting_stone_join_refusal(Some(&party(MATE, 4, 1)), Some(ME), Some(40), stone),
            StoneJoin::Refuse("ERR_MEETING_STONE_MUST_BE_LEADER")
        );

        // An uncached template reads 0/0 (`0x5f8150`) and refuses.
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

        // No active player, whether the guid or the level is missing: silent (`0x5f69f8`).
        assert_eq!(
            meeting_stone_join_refusal(None, Some(ME), None, stone),
            StoneJoin::Silent
        );
        assert_eq!(
            meeting_stone_join_refusal(None, None, Some(40), stone),
            StoneJoin::Silent
        );
    }

    #[test]
    fn the_join_drain_sends_the_clicked_stone_and_refuses_silently_on_the_wire() {
        use crate::go_templates::GameObjectTemplates;
        // A real GameObject guid, `counter | entry << 24 | HIGH_GAMEOBJECT << 48`: the template
        // cache is keyed by the entry it carries.
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

        // In a party led by someone else: the same click sends nothing.
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

    /// A registered schedule: `run_system_once` would build a fresh `Local<VmMemo<bool>>` per call,
    /// an unclaimed memo every frame.
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

        app.update();
        assert_eq!(queries(&rx), 0, "no VM, no bring-up");

        app.insert_non_send_resource(UiScript::new().expect("VM"));
        app.update();
        assert_eq!(queries(&rx), 1, "the first VM asks the server");
        app.update();
        app.update();
        assert_eq!(queries(&rx), 0, "…and does not ask again on later frames");

        // The server answers: the player is queued.
        app.world_mut().resource_mut::<MeetingStone>().area = 1519;
        app.world_mut().resource_mut::<MeetingStone>().dirty = false;

        // `ReloadUI()`: a fresh VM, with no world leave.
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
            "spell 2584 AttributesEx = {ex:#x}: the gate's third leg would decide, and it is open"
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
        // A queued status for slot 2 zeroes both clocks too.
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
        // Re-arm, then clear the active slot: the clocks go, the index stays (`[0x8457cc]`).
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
