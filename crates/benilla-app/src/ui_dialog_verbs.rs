//! The app half of the verbs behind the stock `StaticPopup.lua` dialogs: the packet feeds that
//! raise them and what their buttons do, for the pet trainer's unlearn, the instance boot
//! countdown, the area spirit healer's wave, the battleground queue and the meeting stone.
//! A meeting stone's click runs the stone's own use slot, which refuses a join client-side or
//! sends `CMSG_MEETINGSTONE_JOIN`, never the shared `CMSG_GAMEOBJ_USE`. The handlers signal on the
//! dialogs' lane ([`lane`]) and the getters read the state live through the host ([`live`]).

mod lane;
mod live;

pub(crate) use lane::{add_dialog_lane, DialogLane, DialogSignal};
pub(crate) use live::DialogLive;

/// The service dialogs' range to the NPC, read at the call. The question and the Accept test it
/// inline in `0x5df980`, `0x5dfba0` and `0x5dfdc0`: the NPC's unit looked up (`0x5df9b5`,
/// `0x5dfbd5`, `0x5dfdf5`) and held within `d² <= [0xc4c28c]` of the player (`0x5dfa0d`,
/// `0x5dfc2d`, `0x5dfe4d`, `je`: equality and a NaN pass); with no unit or out of range the call
/// does nothing ([`Self::holds`]). The interface's polls `CheckTalentMasterDist`,
/// `CheckPetUntrainerDist` and `CheckBinderDist` (`0x48d194`, `0x48d1e4`, `0x48d234`) call the
/// same test on the latched guid (`0x5dfb10`, `0x5dfd30`, `0x5dff50`), whose `jp` fails a NaN
/// ([`Self::polled`]). No engine path clears the three latches (`0xc4d7a0`, `0xc4d7b0`,
/// `0xc4d7c0`, written only at `0x5dfa21`, `0x5dfc41`, `0x5dfe64`), and none tests their range
/// outside these calls: walking away hides the dialog through the stock `OnUpdate`
/// (`StaticPopup.lua:1297`, `1313`, `1329`).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DialogReach<'w, 's> {
    index: Res<'w, crate::net::GuidIndex>,
    me: Query<'w, 's, &'static Transform, With<SelfPlayer>>,
    units: Query<'w, 's, &'static Transform>,
}

impl DialogReach<'_, '_> {
    /// `npc`'s squared distance from the player, `None` with no unit for it or no player.
    fn dist_sq(&self, npc: u64) -> Option<f32> {
        let at = self
            .index
            .0
            .get(&npc)
            .and_then(|&e| self.units.get(e).ok())?;
        let me = self.me.iter().next()?;
        Some(at.translation.distance_squared(me.translation))
    }

    /// Whether `npc` passes the question's and the Accept's gate, where a NaN passes.
    pub(crate) fn holds(&self, npc: u64) -> bool {
        self.dist_sq(npc).is_some_and(|d| {
            d.partial_cmp(&crate::target::SERVICE_RANGE_SQ) != Some(std::cmp::Ordering::Greater)
        })
    }

    /// Whether `npc` passes the interface's poll, where a NaN fails.
    pub(crate) fn polled(&self, npc: u64) -> bool {
        self.dist_sq(npc)
            .is_some_and(|d| d <= crate::target::SERVICE_RANGE_SQ)
    }
}

use std::time::Instant;

use benilla_protocol::messages::{BattlefieldStatus, MeetingStoneNotice};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::creature_anim::spell_visual::{meeting_stone_join_fx, SpellKitFx, SpellVisuals};
use crate::names::NameCache;

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfGuid, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_party::GroupState;
use crate::ui_script::{SweepApp, SweepLeg, UiTick};

/// The pet trainer's pending question: the latch (`0xc4d7b0/b4`) and the cost (`0xc4d7b8`).
#[derive(Resource, Default)]
pub(crate) struct PetUnlearnState {
    npc: Option<u64>,
    cost: u32,
}

impl PetUnlearnState {
    /// The inbound `SMSG_PET_UNLEARN_CONFIRM`: the latch (`0x5dfc41`-`0x5dfc5a`).
    pub(crate) fn ask(&mut self, npc: u64, cost: u32) {
        self.npc = Some(npc);
        self.cost = cost;
    }

    pub(crate) fn pending(&self) -> Option<u64> {
        self.npc
    }
}

/// The instance-boot clock (`[0xb4e34c]`).
#[derive(Resource, Default)]
pub(crate) struct InstanceBoot {
    deadline: Option<Instant>,
}

impl InstanceBoot {
    /// `SMSG_RAID_GROUP_ONLY` (`0x5e48f1`), its signals in order onto `lane`: a delay above 0,
    /// signed (`0x496340`, `jle`), arms and raises `INSTANCE_BOOT_START`; any other clears and
    /// raises `INSTANCE_BOOT_STOP`, after `ERR_RAID_GROUP_ONLY` or `_FULL` for a delay of exactly 0
    /// (`0x5e4914`-`0x5e4928`).
    pub(crate) fn apply(
        &mut self,
        delay_ms: u32,
        reason: u32,
        now: Instant,
        lane: &mut DialogLane,
    ) {
        let delay = delay_ms as i32;
        if delay > 0 {
            self.deadline = Some(now + std::time::Duration::from_millis(u64::from(delay_ms)));
            return lane.event("INSTANCE_BOOT_START", vec![]);
        }
        self.deadline = None;
        match (delay, reason) {
            (0, 1) => lane.push(DialogSignal::Line("ERR_RAID_GROUP_ONLY")),
            (0, 2) => lane.push(DialogSignal::Line("ERR_RAID_GROUP_FULL")),
            _ => {}
        }
        lane.event("INSTANCE_BOOT_STOP", vec![]);
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
#[derive(Clone, Default, PartialEq, Eq, Debug)]
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
}

impl AreaSpiritHealer {
    /// `SMSG_AREA_SPIRIT_HEALER_TIME`: arms the clock only for the cached healer (`0x4922a0`) and
    /// a time above 0, compared signed (`0x4922b0 test esi,esi`, `jle`); whether it did, for the
    /// `AREA_SPIRIT_HEALER_IN_RANGE` that follows the store (`0x4922cc`-`0x4922d1`).
    pub(crate) fn on_time(&mut self, healer: u64, ms: u32, now: Instant) -> bool {
        if self.healer != Some(healer) || ms as i32 <= 0 {
            return false;
        }
        self.deadline = Some(now + std::time::Duration::from_millis(u64::from(ms)));
        true
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

    /// Whether a wave's deadline is armed (`[0xb4e338]` non-zero).
    #[cfg(test)]
    pub(crate) fn armed(&self) -> bool {
        self.deadline.is_some()
    }

    /// The cached healer: `AcceptAreaSpiritHeal`'s guid and the poll's retain subject.
    pub(crate) fn healer(&self) -> Option<u64> {
        self.healer
    }

    pub(crate) fn secs(&self, now: Instant) -> u32 {
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
    /// The slot the player is in (`[0x8457cc]`, set by status 3) and its map.
    active: Option<(usize, u32)>,
    /// The instance's run-time stamp (`[0xb6ebbc]`) and expiration (`[0xb6ebb8]`): set by status
    /// 3, zeroed by any other non-clearing status for any slot, as the handler `0x4aa850` does.
    /// The stamp is kept as the status's arrival and the wire's elapsed span, read forwards, so no
    /// `Instant` precedes the clock's origin.
    run_started: Option<(Instant, std::time::Duration)>,
    instance_expiration: Option<Instant>,
}

/// What one `SMSG_BATTLEFIELD_STATUS` did, for the rest of its handler (`0x4aa850`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StatusApplied {
    /// The tutorial it arms: `0x2f` on queued, `0x30` on confirm (`0x4b5390`).
    pub(crate) tutorial: Option<u32>,
    /// The map of a status 3, whose arm builds the stat columns and resets the filter.
    pub(crate) active_map: Option<u32>,
    /// A zero map cleared the slot.
    pub(crate) cleared: bool,
}

impl BattlefieldQueue {
    /// `SMSG_BATTLEFIELD_STATUS` (`0x4aa850`). A zero map clears the slot, and the instance clocks
    /// only if it was active, but leaves the active index (`[0x8457cc]`) alone. `None` for a slot
    /// past the three, which ends the handler before any write or signal.
    pub(crate) fn apply(&mut self, status: BattlefieldStatus) -> Option<StatusApplied> {
        self.apply_at(status, Instant::now())
    }

    fn apply_at(&mut self, status: BattlefieldStatus, now: Instant) -> Option<StatusApplied> {
        let index = status.slot as usize;
        let slot = self.slots.get_mut(index)?;
        if status.map_id == 0 {
            if self.active.is_some_and(|(i, _)| i == index) {
                self.run_started = None;
                self.instance_expiration = None;
            }
            *slot = None;
            return Some(StatusApplied {
                cleared: true,
                ..Default::default()
            });
        }
        let mut applied = StatusApplied {
            tutorial: match status.status {
                1 => Some(crate::tutorial::id::BATTLEGROUND_QUEUE),
                2 => Some(crate::tutorial::id::PORT_TO_BATTLEGROUND),
                _ => None,
            },
            ..Default::default()
        };
        match status.in_progress {
            Some((expires_ms, elapsed_ms)) => {
                self.active = Some((index, status.map_id));
                self.instance_expiration = (expires_ms != 0)
                    .then(|| now + std::time::Duration::from_millis(u64::from(expires_ms)));
                self.run_started = (elapsed_ms != 0)
                    .then(|| (now, std::time::Duration::from_millis(u64::from(elapsed_ms))));
                applied.active_map = Some(status.map_id);
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
        Some(applied)
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
        self.run_started.map_or(0, |(at, elapsed)| {
            (now.saturating_duration_since(at) + elapsed)
                .as_millis()
                .min(u128::from(u32::MAX)) as u32
        })
    }

    /// The map id `AcceptBattlefieldPort` sends for a 1-based slot, if the slot holds a queue.
    pub(crate) fn map_id(&self, index: u8) -> Option<u32> {
        self.slots
            .get(usize::from(index).checked_sub(1)?)
            .and_then(|s| s.as_ref())
            .map(|(s, _)| s.map_id)
    }
}

/// The status text (`[0xb7203c]`): none before world enter and after world leave, the bare
/// `UNKNOWN` until the server's `0x295` lands, then the rebuild over the queued area (`0x4ca070`),
/// formatted against the VM's strings where the getter reads it
/// ([`benilla_ui::script::meeting_stone_text`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum StoneText {
    #[default]
    None,
    Unknown,
    Built,
}

/// The meeting-stone queue: the two globals and the members whose line waits on a name.
#[derive(Resource, Default)]
pub(crate) struct MeetingStone {
    /// `[0xb72038]`, the queued area id, 0 for none.
    pub(crate) area: u32,
    pub(crate) text: StoneText,
    /// `0x299` guids whose name has not resolved yet, each waiting on the name cache's callback
    /// `0x4ca510` (`0x4ca4b2`).
    pending_members: Vec<u64>,
    /// Where the last `0x295` rebuilt the text, by the clock: one after our own create's place,
    /// which the world entry's reset runs ahead of (`0x4908c0` inside `0x5dea50`), keeps its text
    /// past the reset the arrival lane fires later in the frame.
    rebuilt_at: Option<crate::lanes::Mark>,
}

impl MeetingStone {
    /// `SMSG 0x295` (`0x4ca230`): the old area remembered, the new one stored (`0x4ca24a`), the text
    /// rebuilt (`0x4ca385`); the old area, for the line.
    pub(crate) fn apply(&mut self, area: u32) -> u32 {
        let old = std::mem::replace(&mut self.area, area);
        self.text = StoneText::Built;
        self.rebuilt_at = Some(crate::lanes::mark());
        old
    }

    /// The enter-world bring-up (`0x4c9f40`): the text becomes the bare `UNKNOWN`, the area stays.
    fn enter_world(&mut self) {
        self.text = StoneText::Unknown;
    }

    /// The leave-world sweep (`0x4c9f80`): the text goes, the area stays.
    fn leave_world(&mut self) {
        self.text = StoneText::None;
    }

    /// The status the getters read, the rebuilt text named by the area's row
    /// (`[AreaTable row + 0x2c + 4*locale]`, `0x4ca0a2`).
    pub(crate) fn status(
        &self,
        areas: Option<&AreaTableRes>,
    ) -> benilla_ui::script::MeetingStoneStatus {
        use benilla_ui::script::MeetingStoneText as T;
        benilla_ui::script::MeetingStoneStatus {
            area: self.area,
            text: match self.text {
                StoneText::None => T::Unset,
                StoneText::Unknown => T::Unknown,
                StoneText::Built => T::Built(area_name(areas, self.area).map(str::to_string)),
            },
        }
    }

    /// A player name answered: the members waiting on `guid`, taken.
    fn take_member(&mut self, guid: u64) -> usize {
        let before = self.pending_members.len();
        self.pending_members.retain(|&g| g != guid);
        before - self.pending_members.len()
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

/// What the meeting stone's handlers read beside the queue: the name cache the member lines look
/// up and status 1's visual and tutorial, bundled to stay under Bevy's system-parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MeetingStoneInputs<'w, 's> {
    names: Res<'w, NameCache>,
    commands: Res<'w, NetCommands>,
    visuals: Option<Res<'w, SpellVisuals>>,
    fx: MessageWriter<'w, SpellKitFx>,
    self_q: Query<'w, 's, Entity, With<SelfPlayer>>,
    tutorials: Option<Res<'w, crate::tutorial::Tutorials>>,
}

/// `SMSG 0x295` (`0x4ca230`): the area stored, the line for the status (`0x4ca3a4`), status 1's
/// extra block for a changed area (`0x4ca363`: `Effect_C` kind `0xc` on the local player, then
/// the Meeting Stones tutorial), then, whatever the status, the rebuild and
/// `MEETINGSTONE_CHANGED` (`0x4ca385`, `0x4ca38f`), all inside the handler.
fn stone_set_queue(
    stone: &mut MeetingStone,
    lane: &mut DialogLane,
    inputs: &mut MeetingStoneInputs,
    area: u32,
    status: u8,
) {
    let old = stone.apply(area);
    lane.push(DialogSignal::StoneLine {
        old,
        new: area,
        status,
    });
    if status == 1 && area != old {
        if let (Some(visuals), Ok(entity)) = (inputs.visuals.as_deref(), inputs.self_q.single()) {
            if let Some(fx) = meeting_stone_join_fx(visuals, entity) {
                inputs.fx.write(fx);
            }
        }
        if let Some(t) = inputs.tutorials.as_deref() {
            t.trigger_now(crate::tutorial::id::MEETING_STONES);
        }
    }
    lane.event("MEETINGSTONE_CHANGED", vec![]);
}

/// One of the four display-only replies, its line inside its handler; `0x299`'s looks the
/// member's name up with the callback `0x4ca510` (`0x4ca4b2`), its line now on a hit, else at the
/// name's answer ([`DialogNameAnswers`]).
fn stone_notice(
    stone: &mut MeetingStone,
    lane: &mut DialogLane,
    inputs: &MeetingStoneInputs,
    notice: MeetingStoneNotice,
) {
    let line = match notice {
        MeetingStoneNotice::Success => "ERR_MEETING_STONE_SUCCESS",
        MeetingStoneNotice::InProgress => "ERR_MEETING_STONE_IN_PROGRESS",
        MeetingStoneNotice::MemberAdded { guid } => {
            let ask = Ask::at(true, &inputs.commands);
            match inputs.names.resolve_player(guid, ask) {
                Some(name) => lane.push(DialogSignal::LineS(
                    "ERR_MEETING_STONE_MEMBER_ADDED_S",
                    vec![name.to_string()],
                )),
                None => stone.pending_members.push(guid),
            }
            return;
        }
        MeetingStoneNotice::JoinFailed { code } => match code {
            1 => "ERR_MEETING_STONE_MUST_BE_LEADER",
            2 => "ERR_MEETING_STONE_GROUP_FULL",
            3 => "ERR_MEETING_STONE_NO_RAID_GROUP",
            _ => return,
        },
    };
    lane.push(DialogSignal::Line(line));
}

/// The dialogs' callbacks a player-name answer runs, a record or a refusal, after the cache files
/// it (`0x55f426`): each meeting-stone member line waiting on the name (`0x4ca510`), which a
/// refusal spends silently, and the duel's held challenge ([`crate::ui_duel`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DialogNameAnswers<'w, 's> {
    /// Each absent in a harness without the dialogs.
    lane: Option<ResMut<'w, DialogLane>>,
    stone: Option<ResMut<'w, MeetingStone>>,
    duel: Option<ResMut<'w, crate::ui_duel::DuelState>>,
    /// The resurrect offer whose caster's name it waited on, and the active player its show
    /// tests.
    death: Option<ResMut<'w, crate::death::DeathNet>>,
    me: Query<'w, 's, &'static ObjectStore, With<crate::net::SelfPlayer>>,
}

impl DialogNameAnswers<'_, '_> {
    /// `guid`'s answer just filed in `names`.
    pub(crate) fn answered(&mut self, guid: u64, names: &NameCache) {
        let Some(lane) = self.lane.as_deref_mut() else {
            return;
        };
        let name = names.resolve_player(guid, Ask::Read);
        if let Some(stone) = self.stone.as_deref_mut() {
            for _ in 0..stone.take_member(guid) {
                if let Some(name) = name {
                    lane.push(DialogSignal::LineS(
                        "ERR_MEETING_STONE_MEMBER_ADDED_S",
                        vec![name.to_string()],
                    ));
                }
            }
        }
        if let Some(duel) = self.duel.as_deref_mut() {
            crate::ui_duel::name_answered(duel, lane, guid, name);
        }
        if let Some(death) = self.death.as_deref_mut() {
            crate::death::net::resurrecter_named(death, self.me.single().ok(), lane, guid, name);
        }
    }
}

/// The world-enter cascade's meeting-stone leg (`0x490a14` → `0x4c9f40`), after the battlefield
/// status, in the cascade's place ([`crate::ui_world_enter`]): the text reset, then the empty
/// `CMSG 0x296` (`0x4ca1c0`). The cascade runs at login, after every cross-map worldport and on
/// every `/reload`; until the reply fires `MEETINGSTONE_CHANGED` (`0x4ca38f`) the minimap icon is
/// hidden, as in the reference. The queued area survives.
pub(crate) fn meeting_stone_enter_world(
    mut stone: Option<ResMut<MeetingStone>>,
    commands: Res<NetCommands>,
) {
    if let Some(stone) = stone.as_deref_mut() {
        // The reset is the create's, ahead of the frame's later packets: a `0x295` among them
        // keeps its rebuild.
        let at = crate::lanes::firing_now();
        let later = stone
            .rebuilt_at
            .take()
            .is_some_and(|m| at.is_some_and(|at| !at.since(m)));
        if !later {
            stone.enter_world();
        }
    }
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
/// otherwise four refusals in this order, the first two only with party slot 1 set (`0x5f6a05`,
/// `[0xbc6f48]`), never from the raid roster or the wire's member list:
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
    // The party slots alone: none after an ending (`0x4e84a0`), our own subgroup in a raid.
    let party = group.map_or(0, |g| g.party_slots().count());
    if party > 0 {
        if group.map(|g| g.leader) != Some(self_guid) {
            return StoneJoin::Refuse("ERR_MEETING_STONE_MUST_BE_LEADER");
        }
        if party >= MEETING_STONE_PARTY_CAP {
            return StoneJoin::Refuse("ERR_MEETING_STONE_GROUP_FULL");
        }
    }
    // An uncached template reads 0/0, which refuses.
    let (min_level, max_level) = stone.map_or((0, 0), |s| (s.min_level, s.max_level));
    if level < min_level || level > max_level {
        return StoneJoin::Refuse("ERR_MEETING_STONE_INVALID_LEVEL");
    }
    // The raid roster's count, whatever the party test found (`0x5f6aca`).
    if group.is_some_and(GroupState::raid_held) {
        return StoneJoin::Refuse("ERR_MEETING_STONE_NO_RAID_GROUP");
    }
    StoneJoin::Send
}

/// Four other members is a full party: [`GroupState::members`] omits the player, so this is
/// vmangos's five (`Group/Group.h:49,232`) and the client's `>= 4` (`0x5f6a4f`), with no `+ 1`.
const MEETING_STONE_PARTY_CAP: usize = 4;

/// The join drain: the meeting stone's use slot, run with the click's guid.
fn drain_meeting_stone_joins(
    // The click's uses, which the call host's click writes: read before the host is lent.
    mut vm: crate::script_calls::VmWith<MessageReader<'static, 'static, MeetingStoneUse>>,
    group: Option<Res<GroupState>>,
    self_guid: Res<SelfGuid>,
    templates: Res<crate::go_templates::GameObjectTemplates>,
    objects: crate::net::Objects,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
) {
    let uses: Vec<MeetingStoneUse> = vm.own().read().copied().collect();
    vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        let level = self_q.single().ok().and_then(|s| s.0.unit_level());
        let mut lines = Vec::new();
        for &MeetingStoneUse { go_guid } in &uses {
            let stone = templates
                .of_guid(&objects, go_guid)
                .and_then(|t| t.meeting_stone);
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
                StoneJoin::Refuse(key) => lines.extend(crate::ui_action::keyed_line(script, key)),
                StoneJoin::Send => {
                    debug!("meeting stone: join {go_guid:#x}");
                    let _ = commands.0.send(ClientCommand::MeetingStoneJoin { go_guid });
                }
            }
        }
        if !lines.is_empty() {
            crate::ui_action::show_messages(script, &mut sink, "ui_dialog_verbs", lines);
        }
    })
}

/// The leave-world sweep's meeting-stone leg (`0x490b2f` → `0x4c9f80`): the text dropped, the
/// area kept.
fn meeting_stone_leave_world(mut stone: ResMut<MeetingStone>) {
    stone.leave_world();
}

/// Drop the cached spirit guide when the world does, so no healer, wave or event reaches the
/// next session; the reference clears it only through `0x4921c0`, its world-leave path untraced.
fn area_spirit_healer_leave_world(mut spirit: ResMut<AreaSpiritHealer>) {
    *spirit = AreaSpiritHealer::default();
}

/// `ConfirmPetUnlearn()` inside its call (`0x48dca0` → `0x5dfba0`).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PetUnlearnSteps<'w, 's> {
    /// Absent in a harness without the pet trainer's latch.
    pet: Option<Res<'w, PetUnlearnState>>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    reach: DialogReach<'w, 's>,
    net: Res<'w, NetCommands>,
    sink: crate::ui_action::MessageSink<'w>,
}

impl PetUnlearnSteps<'_, '_> {
    /// `PetUnlearnDialog(0, 0)` (`0x5dfba0`) on the remembered trainer (`[0xc4d7b0]`): the trainer
    /// looked up (`0x5dfbd5`) and tested within the interaction range at the call
    /// ([`DialogReach::holds`], `0x5dfc2d`), else nothing, the trainer still remembered; then a
    /// cost above the player's money shows `ERR_NOT_ENOUGH_MONEY` (`0x5dfca3`) and sends nothing,
    /// and otherwise `CMSG_PET_UNLEARN` goes to the trainer (`0x2F0`).
    pub(crate) fn confirm(&mut self, script: &mut UiScript) {
        let Some((npc, cost)) = self
            .pet
            .as_deref()
            .and_then(|p| p.pending().map(|npc| (npc, p.cost)))
        else {
            return;
        };
        if !self.reach.holds(npc) {
            debug!("pet unlearn: trainer {npc:#x} missing or out of range at the confirm");
            return;
        }
        let money = self
            .me
            .single()
            .ok()
            .and_then(|store| store.0.player_money())
            .unwrap_or(0);
        if cost > money {
            let input = script.input_events();
            if let Some(line) = crate::ui_action::keyed_line(script, "ERR_NOT_ENOUGH_MONEY") {
                crate::ui_action::show_now(
                    script,
                    &mut self.sink,
                    "ui_dialog_verbs",
                    line.under(input),
                );
            }
            return;
        }
        let _ = self.net.0.send(ClientCommand::PetUnlearn { trainer: npc });
    }
}

/// `CancelMeetingStoneRequest()` inside its call ([`benilla_ui::script::MeetingStoneCall`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MeetingStoneSteps<'w> {
    group: Option<Res<'w, GroupState>>,
    self_guid: Res<'w, SelfGuid>,
    net: Res<'w, NetCommands>,
    sink: crate::ui_action::MessageSink<'w>,
}

impl MeetingStoneSteps<'_> {
    /// `0x4ca120`: in a party whose leader is not the player, `ERR_MEETING_STONE_NOT_LEADER`
    /// (`0x4ca14e`); otherwise `0x293`, queued or not.
    pub(crate) fn cancel(&mut self, script: &mut UiScript) {
        let not_leader = self
            .group
            .as_deref()
            .is_some_and(|g| g.in_group && Some(g.leader) != self.self_guid.0);
        if not_leader {
            let input = script.input_events();
            if let Some(line) = crate::ui_action::keyed_line(script, "ERR_MEETING_STONE_NOT_LEADER")
            {
                crate::ui_action::show_now(
                    script,
                    &mut self.sink,
                    "ui_dialog_verbs",
                    line.under(input),
                );
            }
            return;
        }
        let _ = self.net.0.send(ClientCommand::MeetingStoneLeave);
    }
}

/// The area-spirit-healer scan `0x4923b0`: only a ghost (`[[player+0xe68]+8]` bit 4) holds a
/// healer; a cached one is kept within 22 yd, else the last unit in the walk that is a
/// `SPIRITGUIDE`, assistable and within 20 yd is adopted. Last, not nearest: the enumerate callback
/// `0x4924c0` never stops the walk. `CGWorldFrame`'s `OnUpdate` runs it every frame (`0x4818ca`),
/// `AcceptAreaSpiritHeal` before its send (`0x492316`) and the time packet before its arm
/// (`0x492296`).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SpiritScan<'w, 's> {
    me: Query<'w, 's, (&'static ObjectStore, &'static Transform), With<SelfPlayer>>,
    #[allow(clippy::type_complexity)] // the walk's unit view
    units: Query<
        'w,
        's,
        (
            &'static crate::net::Guid,
            &'static crate::net::NetEntity,
            &'static ObjectStore,
            &'static Transform,
        ),
        Without<SelfPlayer>,
    >,
    relations: crate::target::RelationWorld<'w, 's>,
}

impl SpiritScan<'_, '_> {
    /// The scan on `spirit`: what each healer change it makes owes, in order.
    pub(crate) fn run(&self, spirit: &mut AreaSpiritHealer) -> Vec<SetHealerOutcome> {
        let Ok((self_store, self_tf)) = self.me.single() else {
            return vec![spirit.set_healer(None)];
        };
        if !self_store.0.player_is_ghost() {
            return vec![spirit.set_healer(None)];
        }
        let here = self_tf.translation;
        let mut out = Vec::new();

        // Retain: a guid that no longer streams drops like one out of range (an `ObjectPtr` miss).
        if let Some(cached) = spirit.healer() {
            let still = self.units.iter().find(|(guid, ..)| guid.0 == cached);
            let keep = still.is_some_and(|(_, _, _, tf)| {
                tf.translation.distance_squared(here)
                    <= SPIRIT_GUIDE_RETAIN_YD * SPIRIT_GUIDE_RETAIN_YD
            });
            if !keep {
                out.push(spirit.set_healer(None));
            }
        }
        if spirit.healer().is_some() {
            return out;
        }

        // Acquire: `CanAssist 0x6066f0` with the guide as `this` (`0x492506`).
        let relations = &self.relations;
        let (rel, Some(me)) = (relations.get(), relations.me()) else {
            return out;
        };
        let mut adopted = None;
        for (guid, kind, store, tf) in self.units.iter() {
            if kind.kind != benilla_protocol::EntityKind::Unit {
                continue;
            }
            if store.0.unit_npc_flags() & NPC_FLAG_SPIRITGUIDE == 0 {
                continue;
            }
            if tf.translation.distance_squared(here)
                > SPIRIT_GUIDE_ACQUIRE_YD * SPIRIT_GUIDE_ACQUIRE_YD
            {
                continue;
            }
            if !rel.can_assist(&relations.unit_of(guid.0, store), &me) {
                continue;
            }
            adopted = Some(guid.0); // last wins: the callback never stops the walk
        }
        if let Some(guid) = adopted {
            out.push(spirit.set_healer(Some(guid)));
        }
        out
    }
}

/// What one healer change owes outside a script call, as `0x4921c0` does it: a pending wave's
/// cancel, the whole aura cancel `0x6e7040(0xA18)` (`0x492201`), whose
/// `AREA_SPIRIT_HEALER_OUT_OF_RANGE` fires into `script`, if given; then the new healer's query.
fn apply_healer_change(
    outcome: &SetHealerOutcome,
    script: Option<&mut UiScript>,
    net: &NetCommands,
    aura: &crate::ui_aura::AuraSteps,
) {
    if outcome.cancel_aura {
        aura.cancel(script, AREA_SPIRIT_HEALER_AURA);
    }
    send_healer_query(outcome, net);
}

/// A healer change's query of the new healer (`0x492219`-`0x49225b`).
pub(crate) fn send_healer_query(outcome: &SetHealerOutcome, net: &NetCommands) {
    if let Some(healer) = outcome.query {
        let _ = net.0.send(ClientCommand::AreaSpiritHealerQuery { healer });
    }
}

/// The scan's every-frame run (`0x4818ca` → `0x4923b0`).
fn poll_area_spirit_healer(
    commands: Res<NetCommands>,
    aura: crate::ui_aura::AuraSteps,
    // The relation inputs, which a call the event's handlers make can write, and the healer the
    // accept's call reads: read and written first.
    mut vm: crate::script_calls::VmWith<(
        SpiritScan<'static, 'static>,
        ResMut<'static, AreaSpiritHealer>,
    )>,
) {
    let outcomes = {
        let (scan, mut spirit) = vm.own();
        scan.run(&mut spirit)
    };
    vm.run(|mut script| {
        for outcome in &outcomes {
            apply_healer_change(outcome, script.as_deref_mut(), &commands, &aura);
        }
    })
}

/// The dialog verbs' packet handlers: each parks a question or a countdown for the feed.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{
        AreaSpiritHealer, BattlefieldQueue, DialogLane, DialogSignal, InstanceBoot, MeetingStone,
        PetUnlearnState,
    };
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::PetUnlearnConfirm, on_pet_unlearn_confirm)
            .net_handler(K::RaidGroupOnly, on_raid_group_only)
            .net_handler(K::AreaSpiritHealerTime, on_area_spirit_healer_time)
            .net_handler(K::MeetingStoneSetQueue, on_meeting_stone)
            .net_handler(K::MeetingStoneNotice, on_meeting_stone)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// Zero the battleground queue at session end, as module init `0x4a9c40` does at every login
    /// (`[0x8457cc]` to -1); vmangos sends no clear for a player who logged out.
    fn on_session_end(In(_): In<SessionEvent>, mut queue: ResMut<BattlefieldQueue>) {
        *queue = BattlefieldQueue::default();
    }

    /// The pet trainer's question, `PetUnlearnDialog(&guid, cost)` (`0x5dfba0`): past the range
    /// gate ([`super::DialogReach`]), the latch (`0x5dfc41`), then `CONFIRM_PET_UNLEARN` with the
    /// cost inside the handler; a zero guid shows `ERR_TALENT_WIPE_ERROR` instead
    /// (`0x5e4a26`-`0x5e4a85`).
    fn on_pet_unlearn_confirm(
        In(ev): In<SessionEvent>,
        mut unlearn: ResMut<PetUnlearnState>,
        mut lane: ResMut<DialogLane>,
        reach: super::DialogReach,
    ) {
        if let SessionEvent::PetUnlearnConfirm { trainer, cost } = ev {
            if trainer == 0 {
                debug!("net: pet unlearn refused (zero trainer) — no dialog");
                lane.push(DialogSignal::Line("ERR_TALENT_WIPE_ERROR"));
            } else if !reach.holds(trainer) {
                debug!("net: trainer {trainer:#x} missing or out of range — the question drops");
            } else {
                debug!("net: trainer {trainer:#x} asks to unlearn the pet for {cost} copper");
                unlearn.ask(trainer, cost);
                lane.event(
                    "CONFIRM_PET_UNLEARN",
                    vec![benilla_ui::script::ScriptValue::Int(i64::from(cost))],
                );
            }
        }
    }

    fn on_raid_group_only(
        In(ev): In<SessionEvent>,
        mut boot: ResMut<InstanceBoot>,
        mut lane: ResMut<DialogLane>,
    ) {
        if let SessionEvent::RaidGroupOnly { delay_ms, reason } = ev {
            boot.apply(delay_ms, reason, std::time::Instant::now(), &mut lane);
        }
    }

    /// `0x492290`: the scan first (`0x492296`), each change it makes acting inside it
    /// (`0x4921c0`: a pending wave's whole aura cancel, a new healer's query), then the arm for the
    /// healer it left cached (`0x4922a0`) and its `AREA_SPIRIT_HEALER_IN_RANGE` (`0x4922d1`). The
    /// sends go out here, so a frame with no VM to fire the lane still cancels and queries; the
    /// signals, the cancel's `AREA_SPIRIT_HEALER_OUT_OF_RANGE` and the arm's, go on the notice
    /// lane at the packet's place.
    fn on_area_spirit_healer_time(
        In(ev): In<SessionEvent>,
        scan: super::SpiritScan,
        mut spirit: ResMut<AreaSpiritHealer>,
        mut lane: ResMut<crate::field_watch::NoticeLane>,
        aura: crate::ui_aura::AuraSteps,
        net: Res<crate::net::NetCommands>,
    ) {
        use crate::field_watch::{id, FieldNotice};
        if let SessionEvent::AreaSpiritHealerTime { healer, ms } = ev {
            for change in scan.run(&mut spirit) {
                if change.cancel_aura && aura.cancel_at_packet(super::AREA_SPIRIT_HEALER_AURA) {
                    lane.write(FieldNotice::Global {
                        id: id::AREA_SPIRIT_HEALER_OUT_OF_RANGE,
                    });
                }
                super::send_healer_query(&change, &net);
            }
            if spirit.on_time(healer, ms, std::time::Instant::now()) {
                lane.write(FieldNotice::Global {
                    id: id::AREA_SPIRIT_HEALER_IN_RANGE,
                });
            }
        }
    }

    fn on_meeting_stone(
        In(ev): In<SessionEvent>,
        mut stone: ResMut<MeetingStone>,
        mut lane: ResMut<DialogLane>,
        mut inputs: super::MeetingStoneInputs,
    ) {
        match ev {
            SessionEvent::MeetingStoneSetQueue { area, status } => {
                super::stone_set_queue(&mut stone, &mut lane, &mut inputs, area, status)
            }
            SessionEvent::MeetingStoneNotice(notice) => {
                super::stone_notice(&mut stone, &mut lane, &inputs, notice)
            }
            _ => {}
        }
    }
}

pub(crate) struct UiDialogVerbsPlugin;

impl Plugin for UiDialogVerbsPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        add_dialog_lane(app);
        app.init_resource::<PetUnlearnState>()
            .init_resource::<InstanceBoot>()
            .init_resource::<AreaSpiritHealer>()
            .init_resource::<BattlefieldQueue>()
            .init_resource::<MeetingStone>()
            .add_message::<MeetingStoneUse>()
            .add_sweep_leg(SweepLeg::MeetingStone, meeting_stone_leave_world)
            .add_systems(
                Update,
                (
                    // After the mouse pick, the reference's order: `CGWorldFrame`'s OnUpdate picks
                    // at `0x48184a`, then polls at `0x4818ca`. Gating is safe: the poll is
                    // level-triggered and re-derives the cache every frame.
                    poll_area_spirit_healer
                        .after(crate::target::TargetUpdate)
                        .run_if(crate::ui_script::ingame_ui_up)
                        .in_set(crate::client_state::InWorldGated),
                    // After the target chain, which writes `MeetingStoneUse`, so the join leaves
                    // on the click's own frame.
                    drain_meeting_stone_joins
                        .after(UiTick)
                        .after(crate::target::TargetUpdate),
                ),
            )
            .add_systems(
                OnExit(crate::client_state::ClientState::InWorld),
                area_spirit_healer_leave_world.in_set(crate::ui_script::SessionEnd::Clears),
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

    /// A member-added packet looks its name up with the callback `0x4ca510` (`0x4ca4b2`): a miss
    /// asks once and waits, a refusal spends the wait silently, and the next packet naming the
    /// member asks again.
    #[test]
    fn a_refused_stone_member_is_asked_by_the_next_packet_never_by_a_frame() {
        use bevy::ecs::system::RunSystemOnce;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<MeetingStone>()
            .init_resource::<DialogLane>()
            .init_resource::<NameCache>()
            .add_message::<SpellKitFx>();
        let added = |app: &mut App| {
            app.world_mut()
                .run_system_once(
                    |mut stone: ResMut<MeetingStone>,
                     mut lane: ResMut<DialogLane>,
                     inputs: MeetingStoneInputs| {
                        let notice = MeetingStoneNotice::MemberAdded { guid: 0x9 };
                        stone_notice(&mut stone, &mut lane, &inputs, notice);
                    },
                )
                .unwrap();
        };
        let answer = |app: &mut App| {
            app.world_mut()
                .run_system_once(|names: Res<NameCache>, mut dialogs: DialogNameAnswers| {
                    dialogs.answered(0x9, &names)
                })
                .unwrap();
        };
        added(&mut app);
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(0x9, String::new(), None);
        answer(&mut app);
        assert!(
            app.world_mut()
                .resource_mut::<DialogLane>()
                .take()
                .is_empty(),
            "a refusal shows nothing"
        );
        assert!(app
            .world()
            .resource::<MeetingStone>()
            .pending_members
            .is_empty());
        added(&mut app);
        assert_eq!(
            crate::query_cache::queries(&rx).len(),
            1,
            "the next packet asks again"
        );
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(0x9, "Ana".into(), None);
        answer(&mut app);
        assert_eq!(
            app.world_mut().resource_mut::<DialogLane>().take(),
            [DialogSignal::LineS(
                "ERR_MEETING_STONE_MEMBER_ADDED_S",
                vec!["Ana".into()]
            )],
            "the line at the name's answer"
        );
    }

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
            crate::net::Guid(0x1),
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
            .query_filtered::<Entity, (With<crate::net::Guid>, Without<SelfPlayer>)>()
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
        let party = |leader: u64, others: usize, group_type: u8| {
            let mut group = GroupState {
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
            if group_type == crate::ui_party::GROUPTYPE_RAID {
                group.rebuild_raid(ME);
            }
            group
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

        // The party terms read the party slots (`0x5f6a05`, `0x4e86d0`), not the wire's list: a
        // raid leader with one other in their subgroup is not full, and the raid refuses.
        let mut raid = party(ME, 6, 1);
        for m in raid.members.iter_mut().skip(1) {
            m.flags = 1;
        }
        assert_eq!(
            meeting_stone_join_refusal(Some(&raid), Some(ME), Some(40), stone),
            StoneJoin::Refuse("ERR_MEETING_STONE_NO_RAID_GROUP")
        );
        // After a leave or a kick the slots are empty and the leader cell 0: no party terms.
        let mut ended = party(MATE, 2, 0);
        ended.slots_emptied = true;
        ended.leader = 0;
        assert_eq!(
            meeting_stone_join_refusal(Some(&ended), Some(ME), Some(40), stone),
            StoneJoin::Send
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
        // The template cache is keyed by the entry in the stone's descriptor, not its guid bits.
        const STONE_ENTRY: u32 = 0x5701;
        const STONE: u64 = 0xF110 << 48 | 0x22;
        const ME: u64 = 0x5e1f;
        const MATE: u64 = 0xa11e;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut templates = GameObjectTemplates::default();
        let mut data = [0i32; 24];
        (data[0], data[1], data[2]) = (15, 60, 1519);
        templates.insert(STONE_ENTRY, 23, "Stonard Meeting Stone".into(), &data);
        app.insert_resource(templates)
            .insert_resource(NetCommands(tx))
            .insert_resource(SelfGuid(Some(ME)))
            .init_resource::<GroupState>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_message::<MeetingStoneUse>()
            .add_systems(Update, drain_meeting_stone_joins);
        crate::items::test_spawn_object(
            app.world_mut(),
            STONE,
            crate::go_templates::test_fields(STONE_ENTRY),
        );
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
        {
            let mut group = app.world_mut().resource_mut::<GroupState>();
            group.in_group = true;
            group.leader = MATE;
            group.members = vec![benilla_protocol::messages::GroupMemberEntry {
                name: "Mate".into(),
                guid: MATE,
                status: 1,
                flags: 0,
            }];
        }
        app.world_mut()
            .write_message(MeetingStoneUse { go_guid: STONE });
        app.update();
        assert!(
            joins(&rx).is_empty(),
            "a refusal is a local line, never a packet"
        );
    }

    /// The two meeting-stone legs behind the worldport's sweep producer, the latch armed as our
    /// own create arms it; the cascade is written by hand, and a `/reload`'s sweep run by hand.
    fn stone_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<MeetingStone>()
            .insert_resource(NetCommands(tx))
            .add_sweep_leg(SweepLeg::MeetingStone, meeting_stone_leave_world);
        crate::ui_script::add_worldport_sweep(&mut app);
        app.world_mut()
            .resource_mut::<crate::ui_script::LeavingWorldArmed>()
            .arm();
        (app, rx)
    }

    fn stone_queries(rx: &crossbeam_channel::Receiver<ClientCommand>) -> usize {
        rx.try_iter()
            .filter(|c| matches!(c, ClientCommand::MeetingStoneStatusQuery))
            .count()
    }

    /// The server's answer has landed, in an earlier frame: queued at `area`, the line built.
    fn answered(app: &mut App, area: u32) {
        let mut stone = app.world_mut().resource_mut::<MeetingStone>();
        stone.apply(area);
        stone.rebuilt_at = None;
    }

    /// The cascade's meeting-stone leg, as the arrival lane runs it.
    fn enter(app: &mut App) {
        app.world_mut()
            .run_system_cached(meeting_stone_enter_world)
            .expect("the stone's leg runs");
    }

    fn stone_text(app: &App) -> StoneText {
        app.world().resource::<MeetingStone>().text
    }

    /// A frame with a load posted when `needs_ack`, which the objects' leg (absent here) takes.
    fn stone_port(app: &mut App, needs_ack: bool) {
        if needs_ack {
            app.world_mut()
                .resource_mut::<crate::net::PendingWorldports>()
                .post_for_test();
        }
        app.update();
        app.world_mut()
            .resource_mut::<crate::net::PendingWorldports>()
            .take_for_test();
    }

    /// Each world-enter cascade (`0x490a14` → `0x4c9f40`) resets the text to the bare `UNKNOWN`
    /// and sends `CMSG 0x296`, a VM or not; frames without one send nothing.
    #[test]
    fn every_world_enter_cascade_resets_the_text_and_asks_again() {
        let (mut app, rx) = stone_app();
        app.update();
        assert_eq!(stone_queries(&rx), 0, "no cascade, no query");

        for entry in [
            "the login create",
            "a worldport's create",
            "a second worldport's",
        ] {
            answered(&mut app, 1519);
            enter(&mut app);
            assert_eq!(stone_queries(&rx), 1, "{entry} asks the server");
            assert_eq!(
                stone_text(&app),
                StoneText::Unknown,
                "{entry} resets the text"
            );
            let stone = app.world().resource::<MeetingStone>();
            assert_eq!(
                stone.area, 1519,
                "{entry} keeps the queued area (`[0xb72038]`)"
            );
            app.update();
            assert_eq!(stone_queries(&rx), 0, "{entry} asks once, not every frame");
        }
    }

    /// A cross-map worldport runs the sweep from the local player's destructor (`0x5dd543`),
    /// which drops the text (`0x490b2f` → `0x4c9f80`); the login map's verify-world destroys
    /// nothing, so the text stays.
    #[test]
    fn a_cross_map_worldport_drops_the_text_and_the_login_map_does_not() {
        let (mut app, rx) = stone_app();
        enter(&mut app);
        answered(&mut app, 1519);

        stone_port(&mut app, false);
        assert_eq!(stone_text(&app), StoneText::Built, "an arrival keeps it");

        stone_port(&mut app, true);
        assert_eq!(
            stone_text(&app),
            StoneText::None,
            "the worldport's sweep drops it"
        );
        let stone = app.world().resource::<MeetingStone>();
        assert_eq!(stone.area, 1519, "the queued area survives the sweep");
        assert_eq!(
            stone_queries(&rx),
            1,
            "the sweep sends nothing; the new create's cascade will"
        );
    }

    /// `/reload` sweeps (`0x490c20`) and then re-enters the cascade (`0x490168`) in one frame:
    /// the drop, then the reset and the query, in that order.
    #[test]
    fn a_reload_drops_the_text_then_resets_it_and_asks_again() {
        let (mut app, rx) = stone_app();
        enter(&mut app);
        assert_eq!(stone_queries(&rx), 1);
        answered(&mut app, 1519);

        assert!(crate::ui_script::run_leave_world_sweep(app.world_mut()));
        enter(&mut app);
        assert_eq!(stone_queries(&rx), 1, "the reload re-asks the server");
        assert_eq!(
            stone_text(&app),
            StoneText::Unknown,
            "the reset is the last word"
        );
        assert_eq!(app.world().resource::<MeetingStone>().area, 1519);
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

    /// `0x496340`'s signed test and the delay-0 line ahead of its stop (`0x5e4914`-`0x5e4928`).
    #[test]
    fn the_boot_clock_arms_on_a_delay_and_clears_on_zero_with_the_reason() {
        let start = || DialogSignal::Event("INSTANCE_BOOT_START".into(), vec![]);
        let stop = || DialogSignal::Event("INSTANCE_BOOT_STOP".into(), vec![]);
        let mut boot = InstanceBoot::default();
        let mut lane = DialogLane::default();
        let now = Instant::now();
        let mut apply = |boot: &mut InstanceBoot, delay, reason| {
            boot.apply(delay, reason, now, &mut lane);
            lane.take()
        };
        assert_eq!(apply(&mut boot, 60_500, 0), [start()]);
        assert_eq!(boot.secs(now), 60, "whole seconds, truncated");
        assert_eq!(
            apply(&mut boot, 0, 1),
            [DialogSignal::Line("ERR_RAID_GROUP_ONLY"), stop()],
            "the line first"
        );
        assert_eq!(boot.secs(now), 0);
        assert_eq!(apply(&mut boot, 0, 7), [stop()], "a reason outside 1/2");
        apply(&mut boot, 60_500, 0);
        assert_eq!(
            apply(&mut boot, u32::MAX, 2),
            [stop()],
            "a negative delay stops with no line"
        );
        assert_eq!(boot.secs(now), 0);
    }

    #[test]
    fn the_spirit_healer_clock_only_arms_for_the_cached_healer() {
        let mut spirit = AreaSpiritHealer::default();
        let now = Instant::now();
        assert!(
            !spirit.on_time(0x77, 30_000, now),
            "no healer cached: the packet is ignored"
        );
        spirit.healer = Some(0x77);
        assert!(!spirit.on_time(0x78, 30_000, now), "another guid: ignored");
        assert!(!spirit.on_time(0x77, 0, now), "a zero time: ignored");
        assert!(spirit.on_time(0x77, 30_000, now));
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
    fn a_run_longer_than_any_uptime_reads_forwards() {
        let mut q = BattlefieldQueue::default();
        let now = Instant::now();
        q.apply_at(
            BattlefieldStatus {
                slot: 0,
                map_id: 489,
                bracket: 0,
                instance_id: 3,
                status: 3,
                time_ms: None,
                in_progress: Some((0, u32::MAX)),
                queued: None,
            },
            now,
        );
        assert_eq!(
            q.run_time_ms(now),
            u32::MAX,
            "49.7 days elapsed, no stamp needed"
        );
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
        assert_eq!(
            q.apply_at(active.clone(), now).and_then(|a| a.active_map),
            Some(489),
            "the status-3 arm's map, for the stat columns"
        );
        assert_eq!(q.active_map(), Some(489));
        assert_eq!(q.instance_expiration_ms(now), 90_000);
        assert_eq!(q.run_time_ms(now), 30_000);
        assert_eq!(
            q.run_time_ms(now + std::time::Duration::from_secs(2)),
            32_000,
            "the stamp runs on from the status"
        );
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
        let olds: Vec<u32> = [1519, 1519, 0, 12].map(|a| stone.apply(a)).into();
        assert_eq!(stone.area, 12);
        assert_eq!(olds, [0, 1519, 1519, 0]);
        assert_eq!(stone.text, StoneText::Built);
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
        use benilla_ui::script::{meeting_stone_text, MeetingStoneText};
        let mut stone = MeetingStone::default();
        stone.apply(1519);
        assert_eq!(
            stone.status(a).text,
            MeetingStoneText::Built(Some("Stormwind City".into()))
        );
        assert_eq!(
            meeting_stone_text(s.lua(), &stone.status(a).text).as_deref(),
            Some("Looking for more for Stormwind City")
        );
        stone.apply(0);
        assert_eq!(
            meeting_stone_text(s.lua(), &stone.status(a).text).as_deref(),
            Some("Looking for more for Unknown"),
            "no row names it UNKNOWN"
        );
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

        let q = app.world().resource::<BattlefieldQueue>();
        assert!(
            q.slots().iter().all(Option::is_none),
            "all three slots empty"
        );
        assert_eq!(q.active_map(), None);
        assert_eq!(q.run_time_ms(now), 0);
        assert_eq!(q.instance_expiration_ms(now), 0);
    }
}
