//! The party and raid bindings. The app pushes a roster snapshot ([`UiScript::set_party`]) that
//! the getters read; the verbs queue a [`PartyRequest`] the app drains
//! ([`UiScript::take_party_requests`]) into the send. Per-member game state is the unit snapshot
//! under `party1`..`party4` ([`super::unit::UnitState`]).
//!
//! The raid roster ([`PartyState::raid`]) is one list, the player included, because the
//! reference's count (`0xb713e0`) bounds the very array `GetRaidRosterInfo` indexes (`0xb712a8`).
//! The saved-instance list ([`SavedInstanceInfo`]) is a separate push, as a roster push replaces
//! [`PartyState`] whole.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{self, number_arg};
use super::by_name::Filter;
use super::Model;
use crate::strings::{fill, global, Arg};

/// One party member; everything else about them is the unit snapshot under their `partyN` token.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartyMemberInfo {
    pub name: String,
    /// The identity `UnitInParty` matches any token against; `0` never matches.
    pub guid: u64,
}

/// One saved raid lockout, `GetSavedInstanceInfo`'s three returns, from `SMSG_RAID_INSTANCE_INFO`
/// with the `Map.dbc` name resolved by the app ([`UiScript::set_saved_instances`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedInstanceInfo {
    /// The `Map.dbc` name, e.g. `"Molten Core"`.
    pub name: String,
    pub instance: u32,
    /// Seconds until the lockout resets.
    pub reset: u32,
}

/// One raid roster row, `GetRaidRosterInfo`'s nine returns in push order (`0x4bb560`). The
/// binding, not the row, applies the subgroup's +1 and the offline zone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RaidMemberInfo {
    /// Return 1, from the name cache (`0x4bb5ee`). Empty means not cached, which the reference
    /// answers with the miss tuple, as for an empty slot (`0x4bb5f8 je 0x4bb7b9`).
    pub name: String,
    /// The identity `UnitInRaid` matches a token against; `0` never matches, as the reference's
    /// membership test rejects a zero guid (`0x4baee0`).
    pub guid: u64,
    /// Return 2, exposed as stored (`0x4bb607 fild [edi+0xc]`): 0 member, 1 assistant, 2 leader,
    /// the convention addons read (`ChatLog.lua:351-353`).
    pub rank: u32,
    /// Return 3, stored 0-based as the wire's `GroupMemberEntry::flags` bits 0-2 carry it; the
    /// binding exposes it 1-based (`0x4bb61a inc eax`).
    pub subgroup: u32,
    /// Return 4, `0` until an object or a stats packet carries it, as in the reference's third arm.
    pub level: u32,
    /// Return 5, the localized class name.
    pub class: Option<String>,
    /// Return 6, the class token (`"WARRIOR"`), the one unlocalized string.
    pub class_file: Option<String>,
    /// Return 7 while [`Self::online`]; offline, the binding pushes the `PLAYER_OFFLINE` global
    /// instead and never reads this.
    pub zone: Option<String>,
    /// Return 8, which also picks return 7's offline arm.
    pub online: bool,
    /// Return 9, unnamed: `1` for a streamed member whose health (`[obj+0x110 +0x40]`) is 0 or
    /// below, or an unstreamed one whose roster status (`[+0x18]`) has both `0x1` and `0x4`,
    /// vmangos's `ONLINE` and `DEAD`, from which the app fills it. Addons read it as `isDead`.
    pub ninth: bool,
}

/// The party and raid roster snapshot the app pushes whole when it changes
/// ([`UiScript::set_party`]); the default is ungrouped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartyState {
    /// The other party members in `party1`..`party4` order, empty when ungrouped. The player is
    /// never in it, as in `SMSG_GROUP_LIST`; in a raid it is our own subgroup's slice.
    pub members: Vec<PartyMemberInfo>,
    /// `GetPartyLeaderIndex`: the leader-index cell `0x84c150` plus one, as the binding pushes it
    /// (`0x4e9104`). The leader's `members` slot, `1..=4`; `0` for a leader in none of the slots,
    /// the player and a raid leader in another subgroup among them, and before any list; `1` once
    /// a list, a kick, a leave or a disband stored no leader. Not the leadership test:
    /// [`Self::leader_guid`] is.
    pub leader_index: u32,
    /// The cached group-leader guid (the reference's `[0xbc75f8]:[0xbc75fc]`), `0` when
    /// ungrouped, fed from the wire rather than derived from [`Self::leader_index`].
    /// `IsRaidLeader` (`0x4bb8c0`) and `IsPartyLeader` (`0x4e9130`) compare it with
    /// [`Self::own_guid`]; `UnitIsPartyLeader` (`0x516210`) compares a resolved token's guid with
    /// it and, unlike `IsPartyLeader`, has no zero guard, so an unresolvable token answers `1`
    /// while solo.
    pub leader_guid: u64,
    /// The player's own guid (`0x468550`, `[0xb41414]+0xc0`), `0` out of world. Compared with
    /// [`Self::leader_guid`] by the leadership tests and the ready-check timeout's leader gate.
    pub own_guid: u64,
    /// The whole raid roster, 1-based for `GetRaidRosterInfo`, empty outside a raid. It includes
    /// the player, as the reference's array does, so `UnitInRaid("player")` answers `1`;
    /// `GetNumRaidMembers()` is its length.
    pub raid: Vec<RaidMemberInfo>,
    /// `GetLootMethod`'s method: `"freeforall"`, `"roundrobin"`, `"master"`, `"group"` or
    /// `"needbeforegreed"`. Empty, the default, reads as `"freeforall"`, the reference's answer
    /// before any group.
    pub loot_method: String,
    /// The master looter as a party index (`0` the player, `1..=4` a `members` slot),
    /// `GetLootMethod`'s second return.
    pub master_looter: Option<u32>,
    /// `GetLootThreshold`'s item quality, `2..=4` once grouped.
    pub loot_threshold: u32,
}

/// Outbound party and raid intents, drained by the app ([`UiScript::take_party_requests`]) into
/// the matching send.
#[derive(Clone, Debug, PartialEq)]
pub enum PartyRequest {
    /// `AcceptGroup()`.
    Accept,
    /// `DeclineGroup()`.
    Decline,
    /// `LeaveParty()`.
    Leave,
    /// `InviteByName(name)`.
    InviteName(String),
    /// `InviteToParty(unit)`, by unit token; the app resolves it to a name.
    InviteUnit(String),
    /// `UninviteFromParty(unit)` after the resolver (`0x48a54d`): the guid the kick sender
    /// `0x5e9510` takes.
    UninviteGuid(u64),
    /// `SetLootMethod(method[, masterName][, threshold])` after the binding's own checks
    /// (`0x4e92a0`): the wire's `CMSG_LOOT_METHOD` fields (`0x5e97e0`).
    LootMethod {
        /// 0 `freeforall`, 1 `roundrobin`, 2 `master`, 3 `group`, 4 `needbeforegreed`.
        method: u32,
        /// The master looter's guid for `master`, else 0.
        master: u64,
        /// The binding's threshold, 2 when it is not a number (`0x4e946d`).
        threshold: u32,
    },
    /// `SetLootThreshold(n)`.
    LootThreshold(u32),
    /// `SetRaidTarget(unit, index)`: icon 1 to 8, or 0 to clear; the app resolves the token to a
    /// guid for `MSG_RAID_TARGET_UPDATE`.
    SetRaidTarget { unit: String, index: u8 },
    // ── The raid-management verbs ───────────────────────────────────────────────────────────
    /// `ConvertToRaid()`, leader only (`CMSG_GROUP_RAID_CONVERT`).
    ConvertToRaid,
    /// `SetRaidSubgroup(index, group)`, both 1-based; the binding raises for a `group` outside 1
    /// to 8. The wire (`CMSG_GROUP_CHANGE_SUB_GROUP`) takes a name and a 0-based subgroup, which
    /// the app resolves.
    SetSubgroup { index: u32, group: u32 },
    /// `SwapRaidSubgroup(index, other)`: trade two raid rows' subgroups
    /// (`CMSG_GROUP_SWAP_SUB_GROUP`, two names on the wire).
    SwapSubgroup { index: u32, other: u32 },
    /// `PromoteToPartyLeader(unit)` or `PromoteByName(name)` after the resolver (`0x48a7fd`,
    /// `0x48a86a`): `CMSG_GROUP_SET_LEADER` with the guid.
    PromoteGuid(u64),
    /// `PromoteToAssistant(name)` / `DemoteAssistant(name)` after the resolver (`0x4bbd6b`,
    /// `0x4bbe3b`): `CMSG_GROUP_ASSISTANT_LEADER` with the guid.
    AssistantLeader { guid: u64, grant: bool },
    /// `UninviteFromRaid(index)`: a 1-based raid row the app resolves to the name
    /// `CMSG_GROUP_UNINVITE` takes.
    UninviteRaid(u32),
    /// `DoReadyCheck()`, leader only (`MSG_RAID_READY_CHECK`, empty body).
    ReadyCheckStart,
    /// `ConfirmReadyCheck(ready)` (`MSG_RAID_READY_CHECK`, one byte).
    ReadyCheckAnswer(bool),
    /// `RequestRaidInfo()` (`CMSG_REQUEST_RAID_INFO`).
    RequestRaidInfo,
}

impl super::UiScript {
    /// Push the roster snapshot, replacing it, and run `SMSG_GROUP_LIST`'s ready-check leg over the
    /// new roster. The app fires the `PARTY_*` events.
    pub fn set_party(&mut self, state: PartyState) {
        // The ready-check leg (`0x4ba5f0`, from the `0x7d` handler): a member who left drops their
        // pending flag, and nobody left pending force-closes the check.
        let lua = self.lua();
        let close = {
            let mut model = self.model_mut();
            model.party = state;
            if model.ready_check.deadline.is_some() {
                let roster: Vec<u64> = model.party.raid.iter().map(|m| m.guid).collect();
                model.ready_check.unanswered.retain(|g| roster.contains(g));
                model.ready_check.unanswered.is_empty()
            } else {
                false
            }
        };
        if close {
            ready_check_force_close(lua);
        }
    }

    /// The `MSG_RAID_READY_CHECK` open form arrived (handler `0x4ba360`). The leader force-closes
    /// if nobody is left pending; anyone else arms the 30 s deadline (`0x4ba535`), which their
    /// leader-gated tick never acts on. The app fires the `READY_CHECK` popup.
    pub fn ready_check_request(&mut self, we_lead: bool) {
        let lua = self.lua();
        let now = clock(lua);
        let close = {
            let mut model = self.model_mut();
            if we_lead {
                model.ready_check.deadline.is_some() && !ready_check_pending_online(&model)
            } else {
                model.ready_check.deadline = Some(now + READY_CHECK_SECONDS);
                false
            }
        };
        if close {
            ready_check_force_close(lua);
        }
    }

    /// One member's answer, relayed to the leader (handler `0x4ba360`, guid matched in full at
    /// `0x4ba3f9`/`0x4ba405`). Any answer clears the member's pending flag, as the handler stores a
    /// constant 0 whatever the status byte (`0x4ba40e`); a `0` also prints `RAID_MEMBER_NOT_READY`
    /// for them (`0x4ba414`). Once no member is both pending and online the check closes at once
    /// (`0x4ba4ce`), so an offline member never holds it open but is still listed as AFK.
    ///
    /// The string is read through `0x703bf0` (`lua_gettable` on the globals, `0x704373`), so a
    /// `_G` `__index` may run and call any verb: the read is made with no Model borrow held.
    pub fn ready_check_answered(&mut self, guid: u64, ready: bool) {
        let lua = self.lua();
        let name = {
            let mut model = self.model_mut();
            let Some(member) = model.party.raid.iter().find(|m| m.guid == guid) else {
                return; // an unmatched guid writes nothing (`0x4ba491`)
            };
            let name = member.name.clone();
            model.ready_check.unanswered.retain(|g| *g != guid);
            name
        };
        if !ready {
            let template: String = lua
                .globals()
                .get::<String>("RAID_MEMBER_NOT_READY")
                .unwrap_or_default();
            self.model_mut()
                .ready_check
                .lines
                .push(template.replacen("%s", &name, 1));
        }
        let close = {
            let model = self.model_mut();
            model.ready_check.deadline.is_some() && !ready_check_pending_online(&model)
        };
        if close {
            ready_check_force_close(lua);
        }
    }

    /// Drain the ready-check lines composed since the last call; the reference prints each as a
    /// `CHAT_MSG_SYSTEM` line (`0x49a870(text, 10)`).
    pub fn take_ready_check_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().ready_check.lines)
    }

    /// `PromoteByName(name)` for the app's Rust slash parser: the binding's own resolve and queue.
    pub fn promote_by_name(&mut self, name: &str) {
        promote_by_name(&mut self.model_mut(), name);
    }

    /// Drain the party/loot intents queued since the last call.
    pub fn take_party_requests(&mut self) -> Vec<PartyRequest> {
        std::mem::take(&mut self.model_mut().party_requests)
    }

    /// Push the saved raid-lockout list, replacing it; the app fires `UPDATE_INSTANCE_INFO`.
    pub fn set_saved_instances(&mut self, saved: Vec<SavedInstanceInfo>) {
        self.model_mut().saved_instances = saved;
    }
}

/// `SetLootMethod`'s usage line (`0x84c42c`).
const USAGE_SET_LOOT_METHOD: &str = "Usage: SetLootMethod(\"method\" [,master][,threshold])";

/// `PromoteByName`: `0x493aa0` with typemask 0x10 and mode 2 (`0x48a86a`), then
/// `CMSG_GROUP_SET_LEADER` (`0x78`) for the guid it returns; a zero guid sends nothing, silently
/// (`0x48a875`). No membership test: the server judges the guid.
fn promote_by_name(model: &mut Model, name: &str) {
    if let Some(guid) = model.closest_player(name, Filter::AssistableAlive) {
        model.party_requests.push(PartyRequest::PromoteGuid(guid));
    }
}

/// `PromoteToAssistant` (`0x4bbd6b`) and `DemoteAssistant` (`0x4bbe3b`): the same resolver call,
/// then `CMSG_GROUP_ASSISTANT_LEADER` (`0x28f`) with the guid and a byte, 1 or 0.
fn assistant_by_name(model: &mut Model, name: &str, grant: bool) {
    if let Some(guid) = model.closest_player(name, Filter::AssistableAlive) {
        model
            .party_requests
            .push(PartyRequest::AssistantLeader { guid, grant });
    }
}

/// Whether `guid` is ourselves or on the party slots (`0x4e7f70`), or on the raid roster
/// (`0x4baee0`); a zero guid is neither.
fn is_group_member(lua: &Lua, guid: u64) -> bool {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let party = &model.party;
    guid != 0
        && (guid == party.own_guid
            || party.members.iter().take(4).any(|m| m.guid == guid)
            || party.raid.iter().any(|m| m.guid == guid))
}

/// The checks both loot setters open with: not in a group (the first party guid and the raid
/// count, `0x4e92a6`, `0x4e92b8`; `0x4e9500`, `0x4e9512`) shows game error 0x48, then not the
/// group leader (`0x4e92d4`..`0x4e92eb`; `0x4e952b`..`0x4e9542`) 0x4b. `false` once one was shown.
fn loot_gates(lua: &Lua) -> bool {
    let (grouped, leads) = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let party = &model.party;
        (
            party.members.first().is_some_and(|m| m.guid != 0) || !party.raid.is_empty(),
            party.own_guid != 0 && party.own_guid == party.leader_guid,
        )
    };
    if !grouped {
        chat_error(lua, "ERR_NOT_IN_GROUP", &[]);
        return false;
    }
    if !leads {
        chat_error(lua, "ERR_NOT_LEADER", &[]);
        return false;
    }
    true
}

/// A catalog row of kind 0, game error `key` shown through `0x496720` from inside a verb: the
/// template from the VM's globals filled with `args`, a chat line when it is not empty.
fn chat_error(lua: &Lua, key: &'static str, args: &[Arg<'_>]) {
    if let Some(template) = global(lua, key) {
        let text = fill(&template, args);
        if !text.is_empty() {
            super::tick::show_chat_line_into(lua, key, text);
        }
    }
}

/// `IsRaidLeader` (`0x4bb8c0`): the cached leader guid (`[0xbc75f8]`, `[0xbc75fc]`) against the
/// player's own (`0x468550`), both words (`0x4bb8d6`-`0x4bb8dc`), and nothing else: no raid flag,
/// so it is true for a party leader, and no subgroup slot. Unlike `IsPartyLeader` it has no zero
/// guard, so with no active player and no group, both zero, it answers 1.
fn is_raid_leader(party: &PartyState) -> bool {
    party.leader_guid == party.own_guid
}

/// `IsPartyLeader` (`0x4e9130`): [`is_raid_leader`]'s compare behind a zero guard on the leader
/// cell (`0x4e9141 or ecx,eax; je`), so an ungrouped player is nobody's leader.
fn is_party_leader(party: &PartyState) -> bool {
    party.leader_guid != 0 && is_raid_leader(party)
}

/// `GetRaidRosterInfo`'s values for one row, or the miss tuple for `None`. Nine on every path
/// (`0x4bb560`, every return `mov eax,9`): out of range, an empty slot and a name-cache miss all
/// reach `0x4bb7b9`, which pushes `nil, 0, 1, 1` and five `nil`s. Takes the row by value because
/// every arm re-enters Lua, and a callback drops its `app_data` borrow before that.
fn raid_roster_info(lua: &Lua, row: Option<RaidMemberInfo>) -> mlua::Result<MultiValue> {
    let Some(m) = row else {
        return Ok(MultiValue::from_vec(vec![
            Value::Nil,
            Value::Integer(0),
            Value::Integer(1),
            Value::Integer(1),
            Value::Nil,
            Value::Nil,
            Value::Nil,
            Value::Nil,
            Value::Nil,
        ]));
    };
    let opt_str = |s: &Option<String>| match s {
        Some(s) => lua.create_string(s).map(Value::String),
        None => Ok(Value::Nil),
    };
    // Return 7: the zone while online, else the `PLAYER_OFFLINE` global read from the VM
    // (`0x703bf0`), so a translated GlobalStrings.lua translates it; a missing global pushes `nil`
    // (`0x6f3890`).
    let zone = if m.online {
        opt_str(&m.zone)?
    } else {
        match lua.globals().get::<Value>("PLAYER_OFFLINE") {
            Ok(v @ Value::String(_)) => v,
            _ => Value::Nil,
        }
    };
    let flag = |b: bool| if b { Value::Integer(1) } else { Value::Nil };
    Ok(MultiValue::from_vec(vec![
        lua.create_string(&m.name).map(Value::String)?,
        Value::Integer(i64::from(m.rank)),
        // Stored 0-based, exposed 1-based (`0x4bb61a inc eax`).
        Value::Integer(i64::from(m.subgroup) + 1),
        Value::Integer(i64::from(m.level)),
        opt_str(&m.class)?,
        opt_str(&m.class_file)?,
        zone,
        flag(m.online),
        flag(m.ninth),
    ]))
}

/// Register the party and raid globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumPartyMembers() → the other members, never counting the player; 0 when ungrouped.
    g.set(
        "GetNumPartyMembers",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.party.members.len() as i64)
        })?,
    )?;

    // GetNumRaidMembers() → the raid roster's length, 0 outside a raid.
    g.set(
        "GetNumRaidMembers",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.party.raid.len() as i64)
        })?,
    )?;

    // GetRaidRosterInfo(index) → name, rank, subgroup, level, class, fileName, zone, online and an
    // unnamed ninth, 1-based. Its only raise is a non-number argument (`0x4bb582 call 0x6f34d0`,
    // `0x4bb591 call 0x6f4940`, usage string `0x8474a0`).
    g.set(
        "GetRaidRosterInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetRaidRosterInfo(index)")?;
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                // One unsigned compare after the decrement (`0x4bb5bb dec eax`, `0x4bb5be jae`)
                // catches an index of 0 or below and one past the count; `dec` wraps, and
                // `wrapping_sub` keeps that true of `i32::MIN`.
                usize::try_from(index.wrapping_sub(1))
                    .ok()
                    .and_then(|i| model.party.raid.get(i))
                    // A name-cache miss shares that same arm (`0x4bb5f8 je 0x4bb7b9`).
                    .filter(|m| !m.name.is_empty())
                    .cloned()
            };
            raid_roster_info(lua, row)
        })?,
    )?;

    // IsRaidLeader() → 1 or nil, true for a party leader too ([`is_raid_leader`]).
    g.set(
        "IsRaidLeader",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if is_raid_leader(&model.party) {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetPartyMember(id) → 1 for a filled 1-based slot, else nil. A non-number, or an index
    // whose `id - 1` is 4 or more unsigned, raises `Usage: GetPartyMember(1-4)` (`0x84c3c4`,
    // `0x4e90b3`, `0x4e90e4`).
    g.set(
        "GetPartyMember",
        lua.create_function(|lua, id: Value| {
            const USAGE: &str = "Usage: GetPartyMember(1-4)";
            let id = binding_abi::index_arg32(lua, id, USAGE)?;
            if id.wrapping_sub(1) >= 4 {
                return Err(mlua::Error::runtime(USAGE));
            }
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (id, n) = (i64::from(id), model.party.members.len() as i64);
            Ok(if id <= n {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetPartyLeaderIndex() → the leader-index cell plus one ([`PartyState::leader_index`]).
    g.set(
        "GetPartyLeaderIndex",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.party.leader_index))
        })?,
    )?;

    // IsPartyLeader() → 1 when the leader cell holds our guid, else nil ([`is_party_leader`]).
    g.set(
        "IsPartyLeader",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if is_party_leader(&model.party) {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetLootMethod() → lootmethod, masterlooterPartyID: two returns in 1.12.
    g.set(
        "GetLootMethod",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // Never grouped, the reference reads zero-filled cells past `.data`'s end, and method
            // 0 is `freeforall`.
            let method = if model.party.loot_method.is_empty() {
                "freeforall"
            } else {
                model.party.loot_method.as_str()
            };
            let master = match model.party.master_looter {
                Some(idx) => Value::Integer(i64::from(idx)),
                None => Value::Nil,
            };
            Ok((Value::String(lua.create_string(method)?), master))
        })?,
    )?;

    // GetLootThreshold() → the loot quality threshold.
    g.set(
        "GetLootThreshold",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.party.loot_threshold))
        })?,
    )?;

    // The party verbs queue a `PartyRequest` and return nothing.
    g.set(
        "AcceptGroup",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::Accept);
            Ok(())
        })?,
    )?;
    g.set(
        "DeclineGroup",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::Decline);
            Ok(())
        })?,
    )?;
    g.set(
        "LeaveParty",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::Leave);
            Ok(())
        })?,
    )?;
    g.set(
        "InviteByName",
        lua.create_function(|lua, name: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::InviteName(name));
            Ok(())
        })?,
    )?;
    g.set(
        "InviteToParty",
        lua.create_function(|lua, unit: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::InviteUnit(unit));
            Ok(())
        })?,
    )?;
    // UninviteFromParty("unit") (`0x48a510`) and PromoteToPartyLeader("unit") (`0x48a7c0`): a
    // string or a number (`0x6f3510`) else the usage line (`0x84277c`, `0x8427e0`), then the token's
    // guid through `GetGUIDFromName` (`0x48a54d`, `0x48a7fd`), which raises for an unknown token;
    // with our player object (`0x48a538`) and a nonzero guid, the kick sender `0x5e9510` or the
    // leader sender `0x5e9640`. No party test: any unit the token names.
    type GuidRequest = fn(u64) -> PartyRequest;
    let token_verbs: [(&str, &'static str, GuidRequest); 2] = [
        (
            "UninviteFromParty",
            r#"Usage: UninviteFromParty("unit")"#,
            PartyRequest::UninviteGuid,
        ),
        (
            "PromoteToPartyLeader",
            r#"Usage: PromoteToPartyLeader("unit")"#,
            PartyRequest::PromoteGuid,
        ),
    ];
    for (binding, usage, request) in token_verbs {
        g.set(
            binding,
            lua.create_function(move |lua, unit: Value| {
                let token = binding_abi::string_arg(lua, unit, usage)?;
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let guid = model.unit_guids.guid_of(&token)?.unwrap_or(0);
                if model.unit_guids.player != 0 && guid != 0 {
                    model.party_requests.push(request(guid));
                }
                Ok(())
            })?,
        )?;
    }
    // SetLootMethod("method" [,master] [,threshold]) (`0x4e92a0`), in the binding's own order:
    // not in a group (the first party guid and the raid count, `0x4e92a6`, `0x4e92b8`) shows game
    // error 0x48 and returns, then not the group leader (`0x4e92d4`..`0x4e92eb`) 0x4b. Argument 1
    // must be a number or string (`0x6f3510`) else the usage line (`0x84c42c`); it is matched
    // without case (`0x64a4c0`, a `strnicmp`) against `freeforall`, `roundrobin`, `master`,
    // `group` and `needbeforegreed`, else `Invalid loot method` (`0x84c418`, `0x4e94c7`). Only
    // "master" reads a name, as argument 2 (`0x4e93e3` moves the threshold to argument 3): empty or
    // missing shows 0xed (`0x4e9453`), one that resolves to nobody in the party, the raid or
    // ourselves (`0x4e7f70`, `0x4baee0`) shows 0x49 with the name (`0x4e9440`); each returns without
    // a send. The threshold is `lua_isnumber`-gated (`0x4e9472`) and `_ftol`ed, 2 on a miss.
    g.set(
        "SetLootMethod",
        lua.create_function(|lua, args: mlua::Variadic<Value>| {
            if !loot_gates(lua) {
                return Ok(());
            }
            let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Nil);
            let Some(method) = binding_abi::optional_string(lua, &arg(0)) else {
                return Err(mlua::Error::runtime(USAGE_SET_LOOT_METHOD));
            };
            let Some(method_id) = [
                "freeforall",
                "roundrobin",
                "master",
                "group",
                "needbeforegreed",
            ]
            .iter()
            .position(|m| m.eq_ignore_ascii_case(&method)) else {
                return Err(mlua::Error::runtime("Invalid loot method"));
            };
            let master = method_id == 2;
            let mut master_guid = 0;
            if master {
                let name = binding_abi::optional_string(lua, &arg(1)).filter(|n| !n.is_empty());
                let Some(name) = name else {
                    lua.app_data_mut::<Model>()
                        .expect("model app_data")
                        .push_ui_error("ERR_SPECIFY_MASTER_LOOTER");
                    return Ok(());
                };
                let found = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    model.closest_player(&name, Filter::AssistableAlive)
                };
                // The pick must be ourselves (`0x4e7f70`, which also tests the four party slots)
                // or in the raid (`0x4baee0`); a stranger the resolver preferred is refused with
                // the typed name, and no next-best member is tried.
                let Some(guid) = found.filter(|g| is_group_member(lua, *g)) else {
                    chat_error(lua, "ERR_TARGET_NOT_IN_GROUP_S", &[Arg::S(&name)]);
                    return Ok(());
                };
                master_guid = guid;
            }
            let threshold = binding_abi::is_number(lua, arg(if master { 2 } else { 1 }))
                .map_or(2, |n| binding_abi::ftol(n) as u32);
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .party_requests
                .push(PartyRequest::LootMethod {
                    method: method_id as u32,
                    master: master_guid,
                    threshold,
                });
            Ok(())
        })?,
    )?;
    // SetLootThreshold(threshold) (`0x4e9500`): the same group and leader checks as SetLootMethod
    // (game errors 0x48 and 0x4b, `0x4e9505`..`0x4e9553`) before the argument is read.
    g.set(
        "SetLootThreshold",
        lua.create_function(|lua, n: Value| {
            if !loot_gates(lua) {
                return Ok(());
            }
            // `is-number` else `Usage: SetLooThreshold(threshold)` (`0x84c464`, `0x4e9564`).
            let n = binding_abi::index_arg32(lua, n, "Usage: SetLooThreshold(threshold)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::LootThreshold(n));
            Ok(())
        })?,
    )?;
    // SetRaidTarget(unit, index) is the engine verb. `SetRaidTargetIcon` is FrameXML's toggle
    // around it (`TargetFrame.lua:486`), and `TargetFrame.lua:488`, `:490` and `Bindings.xml:1160`
    // call it bare.
    g.set(
        "SetRaidTarget",
        lua.create_function(|lua, (unit, index): (Value, Value)| {
            // `is-number-or-string` on arg 1, `is-number` on arg 2 else `Usage: SetRaidTarget(unit,
            // index)` (`0x847578`, `0x4bc066`); the index is `_ftol`'s low byte (`0x4bbf23`).
            const USAGE: &str = "Usage: SetRaidTarget(unit, index)";
            let unit = binding_abi::string_arg(lua, unit, USAGE)?;
            let index = binding_abi::number_arg(lua, index, USAGE)? as u8;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model
                .party_requests
                .push(PartyRequest::SetRaidTarget { unit, index });
            Ok(())
        })?,
    )?;

    // IsRaidOfficer() → 1 or nil (`0x4bb910`): it walks the raid roster (`0xb712a8`, count
    // `[0xb713e0]`) for the row whose guid is the local player's (`0x468550`, `edx:eax`) and pushes
    // 1 when that row's rank (`[row+0xc]`, `GetRaidRosterInfo`'s return 2) is above 0, so for an
    // assistant and for the leader. Outside a raid, without a player or without our row: nil. This
    // is what lets assistants set raid target icons (`UnitPopup.lua`) and invite to the raid.
    g.set(
        "IsRaidOfficer",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let me = model.unit("player").map_or(0, |u| u.guid);
            let officer = me != 0 && model.party.raid.iter().any(|m| m.guid == me && m.rank > 0);
            Ok(if officer {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // ── The raid-management verbs ────────────────────────────────────────────
    // Each only marshals its arguments into a client call or a send (`ConvertToRaid 0x4bbc90`,
    // `SetRaidSubgroup 0x4bb990`, `SwapRaidSubgroup 0x4bbb00`, `PromoteToAssistant 0x4bbd20`,
    // `RequestRaidInfo 0x4a1850`, `GetSavedInstanceInfo 0x4a1920`, `UninviteFromRaid 0x48a580`,
    // `SetRaidRosterSelection 0x4bb820`), so the verbs here queue a request.
    g.set(
        "ConvertToRaid",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::ConvertToRaid);
            Ok(())
        })?,
    )?;
    // SetRaidSubgroup(index, subgroup) / SwapRaidSubgroup(index, other), the drag's two drops. A
    // non-number raises the usage line (`0x4bbae2`), and so does a subgroup whose truncation minus
    // one is not below 8 (`0x4bb9f1`-`0x4bba0c`, string `0x8474e8`), before any row is looked up.
    g.set(
        "SetRaidSubgroup",
        lua.create_function(|lua, (index, group): (Value, Value)| {
            const USAGE: &str = "Usage: SetRaidSubgroup(index, subgroup)";
            let index = number_arg(lua, index, USAGE)?;
            let group = number_arg(lua, group, USAGE)?;
            if (group.wrapping_sub(1) as u32) >= 8 {
                return Err(mlua::Error::runtime(USAGE));
            }
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::SetSubgroup {
                index: index.max(0) as u32,
                group: group as u32,
            });
            Ok(())
        })?,
    )?;
    g.set(
        "SwapRaidSubgroup",
        lua.create_function(|lua, (index, other): (Value, Value)| {
            let index = number_arg(lua, index, "Usage: SwapRaidSubgroup(index1, index2)")?;
            let other = number_arg(lua, other, "Usage: SwapRaidSubgroup(index1, index2)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::SwapSubgroup {
                index: index.max(0) as u32,
                other: other.max(0) as u32,
            });
            Ok(())
        })?,
    )?;
    g.set(
        "PromoteByName",
        lua.create_function(|lua, name: String| {
            promote_by_name(
                &mut lua.app_data_mut::<Model>().expect("model app_data"),
                &name,
            );
            Ok(())
        })?,
    )?;
    for (binding, grant) in [("PromoteToAssistant", true), ("DemoteAssistant", false)] {
        g.set(
            binding,
            lua.create_function(move |lua, name: String| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                assistant_by_name(&mut model, &name, grant);
                Ok(())
            })?,
        )?;
    }
    g.set(
        "UninviteFromRaid",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: UninviteFromRaid(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model
                .party_requests
                .push(PartyRequest::UninviteRaid(index.max(0) as u32));
            Ok(())
        })?,
    )?;
    g.set(
        "DoReadyCheck",
        lua.create_function(|lua, ()| {
            let now = clock(lua);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // The worker `0x4bb1d0` flags every roster member but the caller as pending
            // (`0x4bb24c`, `0x4bb258`) and arms the 30 s deadline (`0x4bb2b6`), then sends. A
            // party has no raid roster, so nothing is flagged and the leader's echo closes it.
            let own = model.party.own_guid;
            model.ready_check.unanswered = model
                .party
                .raid
                .iter()
                .map(|m| m.guid)
                .filter(|g| *g != own)
                .collect();
            model.ready_check.deadline = Some(now + READY_CHECK_SECONDS);
            model.party_requests.push(PartyRequest::ReadyCheckStart);
            Ok(())
        })?,
    )?;
    // CheckReadyCheckTime() (`0x4bc120` → `0x4bb310`), called every frame from stock
    // `UIParent.xml`'s `OnUpdate`, alone drives the 30 s expiry. Once an armed deadline passes, and
    // only on the leader's client, it disarms and prints `RAID_MEMBERS_AFK` with the pending names
    // joined by `", "`, or `READY_CHECK_NO_AFK`, as a `CHAT_MSG_SYSTEM` line. The strings are read
    // raw from `_G` with an empty default (`0x704350`), not through `GetText`, with no Model
    // borrow held (see `ready_check_tick`); nothing is sent.
    g.set(
        "CheckReadyCheckTime",
        lua.create_function(|lua, ()| {
            ready_check_tick(lua, clock(lua));
            Ok(())
        })?,
    )?;
    // ConfirmReadyCheck(ready) (`0x4bc090`) reads its argument through `GetBoolOrDefault`
    // (`0x4bc0ad`) with default 0: the reference's No button passes no argument, Yes passes `1`.
    g.set(
        "ConfirmReadyCheck",
        lua.create_function(|lua, ready: Option<Value>| {
            let ready = crate::script::binding_abi::bool_or_default(ready.as_ref(), false);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model
                .party_requests
                .push(PartyRequest::ReadyCheckAnswer(ready));
            Ok(())
        })?,
    )?;
    g.set(
        "RequestRaidInfo",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::RequestRaidInfo);
            Ok(())
        })?,
    )?;

    // GetNumSavedInstances() / GetSavedInstanceInfo(index), 1-based over the pushed list; a miss
    // pushes nil, 0, nil (`0x4a19e9`).
    g.set(
        "GetNumSavedInstances",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.saved_instances.len() as i64)
        })?,
    )?;
    g.set(
        "GetSavedInstanceInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetSavedInstanceInfo(index)")?;
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                usize::try_from(index.wrapping_sub(1))
                    .ok()
                    .and_then(|i| model.saved_instances.get(i))
                    .cloned()
            };
            Ok(match row {
                Some(r) => MultiValue::from_vec(vec![
                    Value::String(lua.create_string(&r.name)?),
                    Value::Integer(i64::from(r.instance)),
                    Value::Integer(i64::from(r.reset)),
                ]),
                None => super::binding_abi::tuple(&[None, Some(0), None]),
            })
        })?,
    )?;

    // SetRaidRosterSelection(index) / GetRaidRosterSelection(): a client-side cursor; the
    // reference's `0x4bb820` writes a global and sends nothing.
    g.set(
        "SetRaidRosterSelection",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: SetRaidRosterSelection(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.raid_selection = i64::from(index);
            Ok(())
        })?,
    )?;
    g.set(
        "GetRaidRosterSelection",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.raid_selection)
        })?,
    )?;

    Ok(())
}

/// The ready-check deadline, `0x7530` ms after the arm (`0x4ba535`, `0x4bb2b6`).
const READY_CHECK_SECONDS: f64 = 30.0;

/// The ready-check state: the armed deadline (`[0xb713f4]`), the members whose pending flag
/// (`[entry+0x158]`) is set, and summary lines not yet pushed to chat. Outside [`PartyState`]
/// because a roster push replaces that and this survives it.
#[derive(Clone, Debug, Default)]
pub(crate) struct ReadyCheckState {
    pub(crate) deadline: Option<f64>,
    pub(crate) unanswered: Vec<u64>,
    pub(crate) lines: Vec<String>,
}

/// `GetTime()`'s clock, the session seconds `UiScript::tick` advances.
fn clock(lua: &Lua) -> f64 {
    crate::script::clock::now(lua)
}

/// The timeout worker `0x4bb310`. Past its gates it disarms (`0x4bb377`), then reads
/// `RAID_MEMBERS_AFK` before it walks the roster (`0x4bb372`-`0x4bb381`), and `READY_CHECK_NO_AFK`
/// as well when the walk finds no name (`0x4bb470`-`0x4bb48e`). Each read is `0x703bf0`, a
/// `lua_gettable` on the globals (`0x704373`) that runs an `_G` `__index`, which may call any verb;
/// so none is made while the Model is borrowed, and an `__index` that re-enters finds the check
/// already disarmed, as the reference's does.
fn ready_check_tick(lua: &Lua, now: f64) {
    let names = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let Some(deadline) = model.ready_check.deadline else {
            return;
        };
        if now < deadline {
            return;
        }
        if model.party.own_guid != model.party.leader_guid {
            return;
        }
        model.ready_check.deadline = None;
        // A pending member the roster cannot name is skipped (`0x55f080`, its query arm off).
        let names: Vec<String> = model
            .ready_check
            .unanswered
            .iter()
            .filter_map(|g| model.party.raid.iter().find(|m| m.guid == *g))
            .map(|m| m.name.clone())
            .filter(|n| !n.is_empty())
            .collect();
        names
    };
    let raw = |key: &str| lua.globals().get::<String>(key).unwrap_or_default();
    let template = raw("RAID_MEMBERS_AFK");
    let text = if names.is_empty() {
        raw("READY_CHECK_NO_AFK")
    } else {
        template.replacen("%s", &names.join(", "), 1)
    };
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .ready_check
        .lines
        .push(text);
}

/// The `0x322` handler's close test (`0x4ba498`-`0x4ba4cc`): a member holds the check open only
/// while pending and online (`[entry+0x18]` bit 0, `SMSG_GROUP_LIST`'s online bit). An empty
/// roster closes at once (`0x4ba3ea`).
fn ready_check_pending_online(model: &Model) -> bool {
    model
        .ready_check
        .unanswered
        .iter()
        .any(|g| model.party.raid.iter().any(|m| m.guid == *g && m.online))
}

/// The force-close sites (`0x4ba4d4`, `0x4bacb4`): an armed deadline becomes `now - 1` and the
/// worker runs at once.
fn ready_check_force_close(lua: &Lua) {
    let now = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if model.ready_check.deadline.is_none() {
            return;
        }
        let now = model.now;
        model.ready_check.deadline = Some(now - 1.0);
        now
    };
    ready_check_tick(lua, now);
}

#[cfg(test)]
mod tests {
    use crate::script::{PartyMemberInfo, PartyRequest, PartyState, UiScript};

    fn two_member_party() -> PartyState {
        PartyState {
            members: vec![
                PartyMemberInfo {
                    name: "Alice".into(),
                    guid: 0xA11CE,
                },
                PartyMemberInfo {
                    name: "Bob".into(),
                    guid: 0xB0B,
                },
            ],
            leader_index: 1, // Alice (party1) leads
            leader_guid: 0xA11CE,
            own_guid: 0x5E1F,
            raid: Vec::new(),
            loot_method: "group".into(),
            master_looter: None,
            loot_threshold: 2,
        }
    }

    /// The two-member party with the player leading it.
    fn led_party() -> PartyState {
        PartyState {
            leader_index: 0,
            leader_guid: 0x5E1F,
            ..two_member_party()
        }
    }

    #[test]
    fn read_natives_report_the_pushed_roster() {
        let mut s = UiScript::new().unwrap();
        s.set_party(two_member_party());

        assert_eq!(s.eval::<i64>("return GetNumPartyMembers()").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return GetNumRaidMembers()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetPartyMember(1)").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetPartyMember(2)").unwrap(), 1);
        assert!(s.eval::<bool>("return GetPartyMember(3) == nil").unwrap());
        let e = s.run("GetPartyMember(0)").unwrap_err().to_string();
        assert!(e.contains("Usage: GetPartyMember(1-4)"), "{e}");
        assert_eq!(s.eval::<i64>("return GetPartyLeaderIndex()").unwrap(), 1);
        assert!(s.eval::<bool>("return IsPartyLeader() == nil").unwrap());
        let (method, master) = s
            .eval::<(String, Option<i64>)>("return GetLootMethod()")
            .unwrap();
        assert_eq!(method, "group");
        assert_eq!(master, None);
        assert_eq!(s.eval::<i64>("return GetLootThreshold()").unwrap(), 2);
    }

    #[test]
    fn is_party_leader_reports_when_the_player_leads() {
        let mut s = UiScript::new().unwrap();
        s.set_party(led_party());
        assert_eq!(s.eval::<i64>("return IsPartyLeader()").unwrap(), 1);
    }

    #[test]
    fn get_loot_method_reports_the_assigned_master() {
        let mut s = UiScript::new().unwrap();
        let mut party = two_member_party();
        party.loot_method = "master".into();
        party.master_looter = Some(2); // Bob (party2)
        s.set_party(party);
        let (method, master) = s.eval::<(String, i64)>("return GetLootMethod()").unwrap();
        assert_eq!(method, "master");
        assert_eq!(master, 2);
    }

    #[test]
    fn empty_state_reports_the_solo_player_shape() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumPartyMembers()").unwrap(), 0);
        assert!(s.eval::<bool>("return GetPartyMember(1) == nil").unwrap());
        assert!(s.eval::<bool>("return IsPartyLeader() == nil").unwrap());
        let (method, master) = s
            .eval::<(String, Option<i64>)>("return GetLootMethod()")
            .unwrap();
        // Never grouped reads method 0, `freeforall`; the `group` above is a real party's.
        assert_eq!(method, "freeforall");
        assert_eq!(master, None);
    }

    // ── The raid trio (`UnitInRaid`, `GetRaidRosterInfo`, `IsRaidLeader`) ────────────────────

    fn raider(name: &str, guid: u64) -> crate::script::RaidMemberInfo {
        crate::script::RaidMemberInfo {
            name: name.into(),
            guid,
            rank: 0,
            subgroup: 0,
            level: 60,
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            zone: Some("Molten Core".into()),
            online: true,
            ninth: false,
        }
    }

    /// Ten rows as the list writes them: the nine others as listed, the first of them leading,
    /// then us (`0x5e6e73`).
    fn ten_player_raid() -> PartyState {
        let mut raid: Vec<crate::script::RaidMemberInfo> = (0u64..10)
            .map(|i| raider(&format!("Raider{i}"), 0x100 + i))
            .collect();
        raid[0].rank = 2; // the leader
        raid[3].subgroup = 1; // stored 0-based, exposed 2
        raid[9].name = "Me".into();
        PartyState {
            raid,
            ..Default::default()
        }
    }

    /// Every return site in `0x4bb560` is `mov eax,9`, the miss arm `0x4bb7b9` included.
    #[test]
    fn get_raid_roster_info_returns_nine_values_on_every_path() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());

        for index in ["1", "10", "0", "-1", "41", "9999"] {
            assert_eq!(
                s.arity(&format!("GetRaidRosterInfo({index})")).unwrap(),
                9,
                "index {index} must still be a nine-value answer"
            );
        }
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetRaidRosterInfo(1)").unwrap(), 9);
    }

    /// `0x4bb7b9` pushes `nil, 0, 1, 1` and five nils.
    #[test]
    fn get_raid_roster_info_misses_with_the_fixed_tuple() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());
        let miss = "local a,b,c,d,e,f,g,h,i = GetRaidRosterInfo(99) \
                    return a == nil, b, c, d, e == nil, f == nil, g == nil, h == nil, i == nil";
        let (a, b, c, d, e, f, g, h, i): (bool, i64, i64, i64, bool, bool, bool, bool, bool) =
            s.eval(miss).unwrap();
        assert!(a, "name is nil");
        assert_eq!(
            (b, c, d),
            (0, 1, 1),
            "rank 0, subgroup 1, level 1 — numbers"
        );
        assert!(e && f && g && h && i, "the last five are nil");

        // An in-range member with no cached name takes the same arm (`0x4bb5f8 je 0x4bb7b9`).
        let mut party = ten_player_raid();
        party.raid[1].name = String::new();
        s.set_party(party);
        let (name_nil, rank): (bool, i64) = s
            .eval("local a,b = GetRaidRosterInfo(2) return a == nil, b")
            .unwrap();
        assert!(name_nil && rank == 0, "an uncached name is a full miss");
    }

    #[test]
    fn get_raid_roster_info_reports_the_pushed_row() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());
        let (name, rank, subgroup, level, class, file, zone, online, ninth): (
            String,
            i64,
            i64,
            i64,
            String,
            String,
            String,
            i64,
            Option<i64>,
        ) = s
            .eval("local a,b,c,d,e,f,g,h,i = GetRaidRosterInfo(1) return a,b,c,d,e,f,g,h,i")
            .unwrap();
        assert_eq!(name, "Raider0");
        assert_eq!(rank, 2, "as stored, no adjustment");
        assert_eq!(subgroup, 1, "stored 0 → exposed 1 (`0x4bb61a inc eax`)");
        assert_eq!(level, 60);
        assert_eq!((class.as_str(), file.as_str()), ("Warrior", "WARRIOR"));
        assert_eq!(zone, "Molten Core");
        assert_eq!(online, 1);
        assert_eq!(ninth, None, "1/nil, never true/false");
        // Subgroup is the one adjusted field: member 4 is stored in subgroup 1 and reads 2.
        assert_eq!(
            s.eval::<i64>("local _,_,g = GetRaidRosterInfo(4) return g")
                .unwrap(),
            2
        );
    }

    /// Return 8 goes nil on the same arm (`0x4bb705`).
    #[test]
    fn get_raid_roster_info_puts_player_offline_in_the_zone_slot() {
        let mut s = UiScript::new().unwrap();
        let mut party = ten_player_raid();
        party.raid[1].online = false;
        party.raid[1].zone = Some("Molten Core".into()); // ignored on the offline arm
        s.set_party(party);
        // The binding reads whatever `PLAYER_OFFLINE` the VM holds.
        s.run(r#"PLAYER_OFFLINE = "Offline""#).unwrap();
        let (zone, online): (String, Option<i64>) = s
            .eval("local _,_,_,_,_,_,g,h = GetRaidRosterInfo(2) return g, h")
            .unwrap();
        assert_eq!(zone, "Offline");
        assert_eq!(online, None);
    }

    /// The raise at `0x4bb591` goes through `0x6f4940`, which never returns.
    #[test]
    fn get_raid_roster_info_raises_only_on_a_non_number() {
        let s = UiScript::new().unwrap();
        let err = s.arity("GetRaidRosterInfo({})").unwrap_err();
        assert!(
            format!("{err}").contains("Usage: GetRaidRosterInfo(index)"),
            "got {err}"
        );
        // A numeric string coerces (`lua_isnumber`) and does not raise.
        assert_eq!(s.arity(r#"GetRaidRosterInfo("3")"#).unwrap(), 9);
    }

    /// The 1 is the constant double at `0x51637e`.
    #[test]
    fn unit_in_raid_answers_one_not_an_index() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());
        s.set_unit("player", Some(unit(true, 0x109))); // the last row
        s.set_unit("target", Some(unit(true, 0x105))); // roster row 6
        s.set_unit("mouseover", Some(unit(true, 0xDEAD)));

        assert_eq!(s.eval::<i64>(r#"return UnitInRaid("player")"#).unwrap(), 1);
        assert_eq!(
            s.eval::<i64>(r#"return UnitInRaid("target")"#).unwrap(),
            1,
            "row 6 still answers 1 — this is not an index"
        );
        assert!(s
            .eval::<bool>(r#"return UnitInRaid("mouseover") == nil"#)
            .unwrap());
        // Nil for a missing, wrong-typed or unmatched token: `0x6f3690` gives NULL, `0x515970` maps
        // it to guid 0, and `0x4baee0` rejects guid 0.
        for call in ["UnitInRaid()", "UnitInRaid(nil)", r#"UnitInRaid("party3")"#] {
            assert!(
                s.eval::<bool>(&format!("return {call} == nil")).unwrap(),
                "{call} must answer nil"
            );
        }
        // A token matching none of the nine prefixes raises `Unknown unit name: %s`, and a number
        // is coerced to a string first.
        for call in ["UnitInRaid(7)", r#"UnitInRaid("nosuchtoken")"#] {
            assert!(
                s.run(call).is_err(),
                "{call} must raise — the token resolves to no prefix at all"
            );
        }
    }

    /// `0x4bb8c0` reads the same leader guid as `0x4e9130`, and no raid flag.
    #[test]
    fn is_raid_leader_is_true_for_a_party_leader() {
        let mut s = UiScript::new().unwrap();
        s.set_party(led_party()); // we lead a party, with no raid roster
        assert!(
            s.eval::<i64>("return GetNumRaidMembers()").unwrap() == 0,
            "no raid, and that is exactly the point"
        );
        assert_eq!(s.eval::<i64>("return IsRaidLeader()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return IsPartyLeader()").unwrap(), 1);

        s.set_party(two_member_party()); // Alice leads
        assert!(s.eval::<bool>("return IsRaidLeader() == nil").unwrap());
        assert!(s.eval::<bool>("return IsPartyLeader() == nil").unwrap());

        // Ungrouped, in the world: the cached leader guid is 0 and matches nobody.
        s.set_party(PartyState {
            own_guid: 0x5E1F,
            ..PartyState::default()
        });
        assert!(s.eval::<bool>("return IsRaidLeader() == nil").unwrap());
    }

    /// `IsRaidLeader` (`0x4bb8c0`) and `IsPartyLeader` (`0x4e9130`) compare the leader cell with
    /// the player's own guid and read no subgroup slot. A raid leader in another subgroup holds
    /// none of our slots (`GetPartyLeaderIndex` is `0x84c150 + 1`, so 0) and leads nobody here;
    /// one alone in their subgroup has no slots at all and still leads.
    #[test]
    fn the_leader_tests_compare_guids_whatever_slots_the_subgroup_holds() {
        let mut s = UiScript::new().unwrap();
        s.set_party(PartyState {
            leader_index: 0,
            leader_guid: 0xFA2,
            ..two_member_party()
        });
        assert!(s.eval::<bool>("return IsRaidLeader() == nil").unwrap());
        assert!(s.eval::<bool>("return IsPartyLeader() == nil").unwrap());
        assert_eq!(s.eval::<i64>("return GetPartyLeaderIndex()").unwrap(), 0);

        s.set_party(PartyState {
            leader_guid: 0x5E1F,
            own_guid: 0x5E1F,
            ..PartyState::default()
        });
        assert_eq!(s.eval::<i64>("return IsRaidLeader()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return IsPartyLeader()").unwrap(), 1);
    }

    /// Only `IsPartyLeader` guards a zero leader cell (`0x4e9141 or ecx,eax; je`); `IsRaidLeader`
    /// compares the two words bare (`0x4bb8d6`-`0x4bb8dc`), so with no active player and no group,
    /// both zero (`0x468550`), it answers 1. A player in the world, grouped or not, matches only
    /// a cell that holds their guid.
    #[test]
    fn only_is_party_leader_guards_a_zero_leader_cell() {
        let mut s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return IsPartyLeader() == nil").unwrap());
        assert_eq!(s.eval::<i64>("return IsRaidLeader()").unwrap(), 1);

        s.set_party(PartyState {
            own_guid: 0x5E1F,
            ..PartyState::default()
        });
        assert!(s.eval::<bool>("return IsPartyLeader() == nil").unwrap());
        assert!(s.eval::<bool>("return IsRaidLeader() == nil").unwrap());
    }

    /// `SetRaidSubgroup` (`0x4bb990`) truncates its second argument and takes one off
    /// (`0x4bb9f1`-`0x4bb9fd`); unless that is below 8 (`0x4bb9fe`) it raises its usage line
    /// (`0x8474e8`), ahead of any row lookup, so outside a raid too. A non-number raises the same
    /// text (`0x4bbae2`).
    #[test]
    fn set_raid_subgroup_raises_its_usage_line_outside_one_to_eight() {
        const USAGE: &str = "Usage: SetRaidSubgroup(index, subgroup)";
        let mut s = UiScript::new().unwrap();
        for call in [
            "SetRaidSubgroup(3, 0)",
            "SetRaidSubgroup(3, 9)",
            "SetRaidSubgroup(3, -1)",
            "SetRaidSubgroup(3, 0.9)",
            "SetRaidSubgroup(3, 9.5)",
            "SetRaidSubgroup(0, 9)",
            "SetRaidSubgroup(3, 'x')",
            "SetRaidSubgroup(3)",
            "SetRaidSubgroup('x', 3)",
        ] {
            let e = s.run(call).unwrap_err().to_string();
            assert!(e.contains(USAGE), "{call}: {e}");
        }
        assert!(s.take_party_requests().is_empty());
        assert!(!s
            .eval::<bool>("return pcall(SetRaidSubgroup, 3, 9)")
            .unwrap());

        // 8.9 truncates to 8, and a numeric string is a number.
        s.run("SetRaidSubgroup(3, 8.9) SetRaidSubgroup(4, '5')")
            .unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![
                PartyRequest::SetSubgroup { index: 3, group: 8 },
                PartyRequest::SetSubgroup { index: 4, group: 5 },
            ]
        );
    }

    #[test]
    fn get_num_raid_members_bounds_the_roster_it_indexes() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());
        assert_eq!(s.eval::<i64>("return GetNumRaidMembers()").unwrap(), 10);
        assert!(s
            .eval::<bool>("return GetRaidRosterInfo(GetNumRaidMembers()) ~= nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetRaidRosterInfo(GetNumRaidMembers() + 1) == nil")
            .unwrap());
    }

    #[test]
    fn intent_natives_queue_the_exact_request_sequence() {
        let mut s = UiScript::new().unwrap();
        hold(&mut s, "player", 0x5E1F, "Me");
        s.set_party(led_party());
        s.set_unit_guids(&crate::script::UnitGuids {
            player: 0x5E1F,
            party: [0xA11CE, 0xB0B, 0, 0],
            ..Default::default()
        });
        assert!(s.take_party_requests().is_empty());

        s.run("AcceptGroup()").unwrap();
        s.run("DeclineGroup()").unwrap();
        s.run("LeaveParty()").unwrap();
        s.run(r#"InviteByName("Bob")"#).unwrap();
        s.run(r#"InviteToParty("target")"#).unwrap();
        s.run(r#"UninviteFromParty("party2")"#).unwrap();
        s.run(r#"PromoteToPartyLeader("party2")"#).unwrap();
        s.run(r#"SetLootMethod("master", "Bob")"#).unwrap();
        s.run(r#"SetLootThreshold(3)"#).unwrap();

        assert_eq!(
            s.take_party_requests(),
            vec![
                PartyRequest::Accept,
                PartyRequest::Decline,
                PartyRequest::Leave,
                PartyRequest::InviteName("Bob".into()),
                PartyRequest::InviteUnit("target".into()),
                PartyRequest::UninviteGuid(0xB0B),
                PartyRequest::PromoteGuid(0xB0B),
                PartyRequest::LootMethod {
                    method: 2,
                    master: 0xB0B,
                    threshold: 2,
                },
                PartyRequest::LootThreshold(3),
            ]
        );
        assert!(s.take_party_requests().is_empty());
    }

    /// `UninviteFromParty` (`0x48a510`) and `PromoteToPartyLeader` (`0x48a7c0`) take a string or
    /// a number (`0x6f3510`), else their usage line (`0x84277c`, `0x8427e0`); the token goes through
    /// `0x515970`, which raises for one it does not know (`0x515c14`). Nothing is queued without our
    /// player object or for a token naming nobody (`0x48a552`-`0x48a55a`, `0x48a802`-`0x48a80a`).
    #[test]
    fn the_token_verbs_raise_their_usage_and_queue_nothing_for_nobody() {
        let mut s = UiScript::new().unwrap();
        for (call, error) in [
            ("UninviteFromParty()", r#"Usage: UninviteFromParty("unit")"#),
            (
                "PromoteToPartyLeader({})",
                r#"Usage: PromoteToPartyLeader("unit")"#,
            ),
            (r#"UninviteFromParty("bogus")"#, "Unknown unit name: bogus"),
            ("PromoteToPartyLeader(5)", "Unknown unit name: 5"),
        ] {
            let raised = s.run(call).unwrap_err().to_string();
            assert!(raised.contains(error), "{call}: {raised}");
        }
        s.set_unit_guids(&crate::script::UnitGuids {
            target: 0xB0B,
            ..Default::default()
        });
        s.run(r#"UninviteFromParty("target") PromoteToPartyLeader("target")"#)
            .unwrap();
        s.set_unit_guids(&crate::script::UnitGuids {
            player: 0x5E1F,
            ..Default::default()
        });
        s.run(r#"UninviteFromParty("party1") PromoteToPartyLeader("target")"#)
            .unwrap();
        assert!(s.take_party_requests().is_empty());
    }

    fn loot_vm(party: PartyState) -> UiScript {
        let mut s = UiScript::new().unwrap();
        // The active player object, without which the resolver finds nobody (`0x493ae0`).
        hold(&mut s, "player", party.own_guid, "Me");
        s.set_party(party);
        s.run(
            r#"
            ERR_NOT_IN_GROUP = "You aren't in a group."
            ERR_NOT_LEADER = "You are not the group leader."
            ERR_TARGET_NOT_IN_GROUP_S = "%s is not in your group."
        "#,
        )
        .unwrap();
        s
    }

    fn loot_request(method: u32, master: u64, threshold: u32) -> PartyRequest {
        PartyRequest::LootMethod {
            method,
            master,
            threshold,
        }
    }

    #[test]
    fn set_loot_method_without_a_master_name_queues_none() {
        let mut s = loot_vm(led_party());
        s.run(r#"SetLootMethod("freeforall")"#).unwrap();
        assert_eq!(s.take_party_requests(), vec![loot_request(0, 0, 2)]);
    }

    /// `0x4e92a0`: every method but "master" reads its threshold as argument 2 (`0x4e9469`); "master"
    /// reads the name as argument 2 and the threshold as argument 3 (`0x4e93e3`). The threshold is
    /// `lua_isnumber`-gated (`0x4e9472`) and a miss is 2 (`0x4e946d`), never a raise.
    #[test]
    fn the_threshold_is_argument_two_unless_the_method_is_master() {
        let mut s = loot_vm(led_party());
        for (call, want) in [
            (r#"SetLootMethod("group", 4)"#, loot_request(3, 0, 4)),
            (r#"SetLootMethod("roundrobin", "5")"#, loot_request(1, 0, 5)),
            (r#"SetLootMethod("group", 3.9)"#, loot_request(3, 0, 3)),
            (r#"SetLootMethod("group", nil, 4)"#, loot_request(3, 0, 2)),
            (r#"SetLootMethod("group", "x")"#, loot_request(3, 0, 2)),
            (r#"SetLootMethod("group", {})"#, loot_request(3, 0, 2)),
            (
                r#"SetLootMethod("master", "Bob", 3)"#,
                loot_request(2, 0xB0B, 3),
            ),
            (
                r#"SetLootMethod("MASTER", "bob", "x")"#,
                loot_request(2, 0xB0B, 2),
            ),
            (
                r#"SetLootMethod("master", "Bob")"#,
                loot_request(2, 0xB0B, 2),
            ),
            (
                r#"SetLootMethod("needbeforegreed", 3)"#,
                loot_request(4, 0, 3),
            ),
        ] {
            s.run(call).unwrap();
            assert_eq!(s.take_party_requests(), vec![want], "{call}");
        }
    }

    /// The group checks come first (`0x4e92a6`..`0x4e92eb`): game errors 0x48 and 0x4b, kind 0 chat
    /// lines, and nothing is sent or raised, whatever the arguments.
    #[test]
    fn set_loot_method_needs_a_group_and_its_leader_before_it_reads_anything() {
        let mut solo = loot_vm(PartyState::default());
        solo.run("SetLootMethod(nil)").unwrap();
        assert!(solo.take_party_requests().is_empty());
        assert_eq!(
            solo.take_shown_chat_lines(),
            vec![("ERR_NOT_IN_GROUP", "You aren't in a group.".to_string())]
        );
        let mut member = loot_vm(two_member_party());
        member.run(r#"SetLootMethod("bogus")"#).unwrap();
        assert!(member.take_party_requests().is_empty());
        assert_eq!(
            member.take_shown_chat_lines(),
            vec![(
                "ERR_NOT_LEADER",
                "You are not the group leader.".to_string()
            )]
        );
    }

    /// Argument 1 must be a number or string, else the usage line (`0x6f3510`, `0x84c42c`); a
    /// string that names no method is `Invalid loot method` (`0x84c418`, `0x4e94c7`).
    #[test]
    fn set_loot_method_raises_its_usage_line_and_the_invalid_method() {
        let mut s = loot_vm(led_party());
        for call in [
            "SetLootMethod()",
            "SetLootMethod(nil)",
            "SetLootMethod({})",
            "SetLootMethod(true)",
        ] {
            let e = s.run(call).unwrap_err().to_string();
            assert!(
                e.contains(r#"Usage: SetLootMethod("method" [,master][,threshold])"#),
                "{call}: {e}"
            );
        }
        for call in [
            r#"SetLootMethod("bogus")"#,
            "SetLootMethod(7)",
            r#"SetLootMethod("")"#,
        ] {
            let e = s.run(call).unwrap_err().to_string();
            assert!(e.contains("Invalid loot method"), "{call}: {e}");
        }
        assert!(s.take_party_requests().is_empty());
    }

    /// `SetLootThreshold` (`0x4e9500`) opens with the same two checks, then wants a number
    /// (`0x84c464`, the reference's own spelling of its name).
    #[test]
    fn set_loot_threshold_needs_a_group_and_its_leader_then_a_number() {
        let mut solo = loot_vm(PartyState::default());
        solo.run("SetLootThreshold(nil)").unwrap();
        assert!(solo.take_party_requests().is_empty());
        assert_eq!(
            solo.take_shown_chat_lines(),
            vec![("ERR_NOT_IN_GROUP", "You aren't in a group.".to_string())]
        );
        let mut member = loot_vm(two_member_party());
        member.run("SetLootThreshold(3)").unwrap();
        assert!(member.take_party_requests().is_empty());
        assert_eq!(
            member.take_shown_chat_lines(),
            vec![(
                "ERR_NOT_LEADER",
                "You are not the group leader.".to_string()
            )]
        );
        let mut leader = loot_vm(led_party());
        let e = leader.run("SetLootThreshold('x')").unwrap_err().to_string();
        assert!(e.contains("Usage: SetLooThreshold(threshold)"), "{e}");
        leader.run("SetLootThreshold('4')").unwrap();
        assert_eq!(
            leader.take_party_requests(),
            vec![PartyRequest::LootThreshold(4)]
        );
    }

    /// "master" with no name shows game error 0xed (`0x4e9453`); a name nobody in the party, the
    /// raid or ourselves answers to shows 0x49 with it (`0x4e9440`); neither sends.
    #[test]
    fn set_loot_method_master_needs_a_name_in_the_group() {
        let mut s = loot_vm(led_party());
        for call in [
            r#"SetLootMethod("master")"#,
            r#"SetLootMethod("master", "")"#,
            r#"SetLootMethod("master", nil, 3)"#,
            r#"SetLootMethod("master", {})"#,
        ] {
            s.run(call).unwrap();
            assert!(s.take_party_requests().is_empty(), "{call}");
            let errors = s.take_ui_errors();
            assert_eq!(errors, vec!["ERR_SPECIFY_MASTER_LOOTER"], "{call}");
        }
        s.run(r#"SetLootMethod("master", "Nobody")"#).unwrap();
        assert!(s.take_party_requests().is_empty());
        assert_eq!(
            s.take_shown_chat_lines(),
            vec![(
                "ERR_TARGET_NOT_IN_GROUP_S",
                "Nobody is not in your group.".to_string()
            )]
        );
        // A raid member resolves too.
        let mut raid = led_party();
        raid.raid = vec![crate::script::RaidMemberInfo {
            name: "Carol".into(),
            guid: 0xCA401,
            ..Default::default()
        }];
        let mut s = loot_vm(raid);
        s.run(r#"SetLootMethod("master", "carol")"#).unwrap();
        assert_eq!(s.take_party_requests(), vec![loot_request(2, 0xCA401, 2)]);
    }

    /// `0x493aa0` (mode 2) resolves the master's name over the group: an exact name, else the longest
    /// case-folded prefix shared (at least one letter), the first of equal ones in roster order.
    #[test]
    fn the_master_name_resolves_by_closest_match() {
        let party = |names: &[(&str, u64)]| PartyState {
            members: names
                .iter()
                .map(|(n, g)| PartyMemberInfo {
                    name: (*n).into(),
                    guid: *g,
                })
                .collect(),
            ..led_party()
        };
        let resolve = |names: &[(&str, u64)], call: &str| {
            let mut s = loot_vm(party(names));
            s.run(call).unwrap();
            (s.take_party_requests(), s.take_shown_chat_lines())
        };
        let alice_bob = [("Alice", 0xA), ("Bob", 0xB)];
        // Exact, in any case, and partial.
        for (call, guid) in [
            (r#"SetLootMethod("master", "Bob")"#, 0xB),
            (r#"SetLootMethod("master", "bOB")"#, 0xB),
            (r#"SetLootMethod("master", "Bo")"#, 0xB),
            (r#"SetLootMethod("master", "B")"#, 0xB),
            (r#"SetLootMethod("master", "al")"#, 0xA),
            (r#"SetLootMethod("master", "Alicia")"#, 0xA),
            (r#"SetLootMethod("master", "Bxx")"#, 0xB),
        ] {
            let (sent, chat) = resolve(&alice_bob, call);
            assert_eq!(sent, vec![loot_request(2, guid, 2)], "{call}");
            assert!(chat.is_empty(), "{call}: {chat:?}");
        }
        // No shared first letter finds nobody: game error 0x49 with the name, nothing sent.
        for call in [
            r#"SetLootMethod("master", "Zed")"#,
            r#"SetLootMethod("master", "xob")"#,
        ] {
            let (sent, chat) = resolve(&alice_bob, call);
            assert!(sent.is_empty(), "{call}");
            assert_eq!(chat.len(), 1, "{call}");
            assert_eq!(chat[0].0, "ERR_TARGET_NOT_IN_GROUP_S");
        }
        // A longer prefix beats a shorter one wherever it sits; equal ones keep roster order, and
        // an exact name later in the roster does not displace an equal-length earlier prefix
        // (its distance is not below the best, `0x493c93`).
        let bobs = [("Bobby", 0x1), ("Bob", 0x2), ("Bo", 0x3)];
        for (call, guid) in [
            (r#"SetLootMethod("master", "Bo")"#, 0x1),
            (r#"SetLootMethod("master", "Bob")"#, 0x1),
            (r#"SetLootMethod("master", "Bobb")"#, 0x1),
            (r#"SetLootMethod("master", "Bobx")"#, 0x1),
        ] {
            let (sent, _) = resolve(&bobs, call);
            assert_eq!(sent, vec![loot_request(2, guid, 2)], "{call}");
        }
        let (sent, _) = resolve(
            &[("Bo", 0x3), ("Bobby", 0x1)],
            r#"SetLootMethod("master", "Bob")"#,
        );
        assert_eq!(
            sent,
            vec![loot_request(2, 0x1, 2)],
            "the longer prefix wins"
        );
    }

    /// A held player the app's candidate feed pushes: `name` at `dist2`, `assistable_alive` as the
    /// mode-2 filter reads it.
    fn held_player(
        guid: u64,
        name: &str,
        dist2: f32,
        assistable_alive: bool,
    ) -> crate::script::NameCandidate {
        crate::script::NameCandidate {
            name: name.into(),
            dist2,
            state: crate::script::UnitState {
                exists: true,
                has_object: true,
                guid,
                name: Some(name.into()),
                is_player: true,
                ..Default::default()
            },
            assistable_alive,
        }
    }

    /// The party token `token` has an object, so the roster passes leave it to the object pass.
    fn hold(s: &mut UiScript, token: &str, guid: u64, name: &str) {
        s.set_unit(
            token,
            Some(crate::script::UnitState {
                exists: true,
                has_object: true,
                guid,
                name: Some(name.into()),
                ..Default::default()
            }),
        );
    }

    /// `0x493aa0` runs the object pass after the roster passes (`0x493c2c`), so an assistable
    /// player in range outranks a groupmate with no object at an equal prefix (the roster's
    /// placeholder distance is the larger, `0x4f1502f9`); the pick is then no groupmate and the
    /// binding prints game error 0x49 with the typed name (`0x4e9440`, `esi` the argument).
    #[test]
    fn a_held_stranger_outranks_an_unheld_groupmate_and_is_refused() {
        let mut s = loot_vm(led_party());
        s.set_name_candidate(0xB0B1, Some(held_player(0xB0B1, "Bobby", 25.0, true)));
        s.run(r#"SetLootMethod("master", "Bo")"#).unwrap();
        assert!(s.take_party_requests().is_empty(), "Bobby is no groupmate");
        assert_eq!(
            s.take_shown_chat_lines(),
            vec![(
                "ERR_TARGET_NOT_IN_GROUP_S",
                "Bo is not in your group.".to_string()
            )]
        );
        // A longer prefix still wins at any distance: the unheld Bob answers "Bob".
        s.run(r#"SetLootMethod("master", "Bob")"#).unwrap();
        assert_eq!(s.take_party_requests(), vec![loot_request(2, 0xB0B, 2)]);
        // Only the matcher's best is tried: the stranger is not skipped for the next-best member.
        let mut s = loot_vm(led_party());
        s.set_name_candidate(0xA1, Some(held_player(0xA1, "Alicia", 4.0, true)));
        s.run(r#"SetLootMethod("master", "Ali")"#).unwrap();
        assert!(s.take_party_requests().is_empty());
        assert_eq!(s.take_shown_chat_lines().len(), 1);
    }

    /// Mode 2's filter (`0x493eca`) is `CanAssist` and health above 0: a held groupmate who fails it
    /// is not offered by the roster passes (they skip held members, `0x493b4d`) nor by the object
    /// pass, so nobody resolves; and a creature is outside typemask 0x10.
    #[test]
    fn a_held_groupmate_who_is_dead_or_unassistable_is_not_resolved() {
        let mut s = loot_vm(led_party());
        hold(&mut s, "party2", 0xB0B, "Bob");
        s.set_name_candidate(0xB0B, Some(held_player(0xB0B, "Bob", 9.0, false)));
        s.run(r#"SetLootMethod("master", "Bob")"#).unwrap();
        assert!(s.take_party_requests().is_empty());
        assert_eq!(s.take_shown_chat_lines().len(), 1);
        // Alive again, the same candidate resolves.
        s.set_name_candidate(0xB0B, Some(held_player(0xB0B, "Bob", 9.0, true)));
        s.run(r#"SetLootMethod("master", "Bob")"#).unwrap();
        assert_eq!(s.take_party_requests(), vec![loot_request(2, 0xB0B, 2)]);
        // A held creature is outside the typemask, so its longer prefix does not displace the
        // unheld groupmate Bob's.
        let mut s = loot_vm(led_party());
        let mut cat = held_player(0xF130_0001, "Bobcat", 1.0, true);
        cat.state.is_player = false;
        s.set_name_candidate(0xF130_0001, Some(cat));
        s.run(r#"SetLootMethod("master", "Bobc")"#).unwrap();
        assert_eq!(s.take_party_requests(), vec![loot_request(2, 0xB0B, 2)]);
    }

    /// `0x493ae0`: with no active player object the resolver finds nobody, a groupmate with no
    /// object included, so the binding shows game error 0x49.
    #[test]
    fn without_the_player_object_the_master_name_finds_nobody() {
        let mut s = UiScript::new().unwrap();
        s.set_party(led_party());
        s.run(r#"ERR_NOT_LEADER = "x" ERR_TARGET_NOT_IN_GROUP_S = "%s is not in your group.""#)
            .unwrap();
        s.run(r#"SetLootMethod("master", "Bob")"#).unwrap();
        assert!(s.take_party_requests().is_empty());
        assert_eq!(s.take_shown_chat_lines().len(), 1);
    }

    /// Of two held groupmates with an equal prefix the nearer wins (`0x493cfc`, strictly below the
    /// best); the roster order decides nothing once both are held.
    #[test]
    fn of_two_held_groupmates_the_nearer_wins() {
        let party = PartyState {
            members: vec![
                PartyMemberInfo {
                    name: "Bob".into(),
                    guid: 0xB1,
                },
                PartyMemberInfo {
                    name: "Bobby".into(),
                    guid: 0xB2,
                },
            ],
            ..led_party()
        };
        for (bob, bobby, guid) in [(400.0, 16.0, 0xB2), (16.0, 400.0, 0xB1)] {
            let mut s = loot_vm(party.clone());
            hold(&mut s, "party1", 0xB1, "Bob");
            hold(&mut s, "party2", 0xB2, "Bobby");
            s.set_name_candidate(0xB1, Some(held_player(0xB1, "Bob", bob, true)));
            s.set_name_candidate(0xB2, Some(held_player(0xB2, "Bobby", bobby, true)));
            s.run(r#"SetLootMethod("master", "Bo")"#).unwrap();
            assert_eq!(s.take_party_requests(), vec![loot_request(2, guid, 2)]);
        }
    }

    #[test]
    fn set_raid_target_icon_queues_the_token_and_index() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SetRaidTarget("target", 8)"#).unwrap();
        s.run(r#"SetRaidTarget("party2", 0)"#).unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![
                PartyRequest::SetRaidTarget {
                    unit: "target".into(),
                    index: 8,
                },
                PartyRequest::SetRaidTarget {
                    unit: "party2".into(),
                    index: 0,
                },
            ]
        );
    }

    /// `0x4bb910`: our own roster row, rank above 0 (assistant or leader); nil otherwise.
    #[test]
    fn is_raid_officer_reads_our_own_roster_rank() {
        let officer = |s: &UiScript| s.eval::<Option<i64>>("return IsRaidOfficer()").unwrap();
        let mut s = UiScript::new().unwrap();
        assert_eq!(officer(&s), None, "ungrouped");
        // We are the last row, a plain member under row 1's lead.
        let mut raid = ten_player_raid();
        raid.raid[1].rank = 1; // someone else assists
        s.set_party(raid.clone());
        assert_eq!(officer(&s), None, "no player unit yet");
        s.set_unit("player", Some(unit(true, 0x109)));
        assert_eq!(
            officer(&s),
            None,
            "a member, another row's rank does not count"
        );
        raid.raid[9].rank = 1;
        s.set_party(raid.clone());
        assert_eq!(officer(&s), Some(1), "assistant");
        raid.raid[0].rank = 0;
        raid.raid[9].rank = 2;
        s.set_party(raid);
        assert_eq!(officer(&s), Some(1), "leader");
        s.set_unit("player", Some(unit(true, 0x999)));
        assert_eq!(officer(&s), None, "no row of ours");
    }

    // ── The identity predicates ─────────────────────────────────────────────

    fn unit(exists: bool, guid: u64) -> crate::script::UnitState {
        crate::script::UnitState {
            exists,
            guid,
            ..Default::default()
        }
    }

    #[test]
    fn unit_is_unit_compares_guids_and_tokens() {
        let mut s = UiScript::new().unwrap();
        s.set_unit("player", Some(unit(true, 0x10)));
        s.set_unit("target", Some(unit(true, 0x10)));
        s.set_unit("party1", Some(unit(true, 0x20)));
        assert_eq!(
            s.eval::<i64>(r#"return UnitIsUnit("target", "player")"#)
                .unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>(r#"return UnitIsUnit("player", "player")"#)
                .unwrap(),
            1
        );
        assert!(s
            .eval::<bool>(r#"return UnitIsUnit("party1", "player") == nil"#)
            .unwrap());
        // Zero guids never match across tokens.
        s.set_unit("target", Some(unit(true, 0)));
        s.set_unit("mouseover", Some(unit(true, 0)));
        assert!(s
            .eval::<bool>(r#"return UnitIsUnit("target", "mouseover") == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return UnitIsUnit("pet", "player") == nil"#)
            .unwrap());
    }

    /// A member's own guid, or a unit whose owner is a member: the party's for
    /// `UnitPlayerOrPetInParty`, the raid roster's for `UnitPlayerOrPetInRaid`.
    #[test]
    fn unit_player_or_pet_in_party_reads_the_owner() {
        let mut s = UiScript::new().unwrap();
        s.set_party(two_member_party());
        s.set_unit("player", Some(unit(true, 0x10)));
        let mut alices_pet = unit(true, 0xC0FFEE);
        alices_pet.owner = 0xA11CE;
        s.set_unit("target", Some(alices_pet));
        assert_eq!(
            s.eval::<i64>(r#"return UnitPlayerOrPetInParty("target")"#)
                .unwrap(),
            1,
            "Alice's pet is in the party by its owner"
        );
        assert!(
            s.eval::<bool>(r#"return UnitPlayerOrPetInRaid("target") == nil"#)
                .unwrap(),
            "no raid: the raid twin says nil"
        );
        let mut strangers_pet = unit(true, 0xC0FFEE);
        strangers_pet.owner = 0xDEAD;
        s.set_unit("target", Some(strangers_pet));
        assert!(s
            .eval::<bool>(r#"return UnitPlayerOrPetInParty("target") == nil"#)
            .unwrap());
        // A member's own guid still answers, as UnitInParty does.
        s.set_unit("target", Some(unit(true, 0xB0B)));
        assert_eq!(
            s.eval::<i64>(r#"return UnitPlayerOrPetInParty("target")"#)
                .unwrap(),
            1
        );
        s.set_party(ten_player_raid());
        let mut raiders_pet = unit(true, 0xC0FFEE);
        raiders_pet.owner = ten_player_raid().raid[0].guid;
        s.set_unit("target", Some(raiders_pet));
        assert_eq!(
            s.eval::<i64>(r#"return UnitPlayerOrPetInRaid("target")"#)
                .unwrap(),
            1
        );
        s.set_party(crate::script::PartyState::default());
        assert!(s
            .eval::<bool>(r#"return UnitPlayerOrPetInParty("target") == nil"#)
            .unwrap());
    }

    /// `UnitInParty` (`0x516290`) resolves the token to a guid (`0x515970`) and asks `0x4e7f70`:
    /// the player's own guid or one of the four slots'. No object and no group are needed.
    #[test]
    fn unit_in_party_answers_for_the_guid_the_token_names() {
        let mut s = UiScript::new().unwrap();
        let guids = |party: [u64; 4], target: u64| crate::script::UnitGuids {
            player: 0x10,
            target,
            party,
            ..Default::default()
        };
        let one =
            |s: &UiScript, call: &str| s.eval::<bool>(&format!("return {call} == 1")).unwrap();
        let nil =
            |s: &UiScript, call: &str| s.eval::<bool>(&format!("return {call} == nil")).unwrap();

        // The reported shape: solo, the player is in their own party.
        s.set_unit_guids(&guids([0; 4], 0));
        assert!(one(&s, r#"UnitInParty("player")"#), "solo");
        assert!(nil(&s, r#"UnitInParty("party1")"#));
        assert!(nil(&s, r#"UnitInParty("target")"#), "no target");

        // party2 is out of view and has no snapshot; `target` names them too.
        s.set_unit_guids(&guids([0xA11CE, 0xB0B, 0, 0], 0xB0B));
        assert!(one(&s, r#"UnitInParty("party2")"#), "no object needed");
        assert!(one(&s, r#"UnitInParty("target")"#), "by guid, not by text");
        assert!(one(&s, r#"UnitInParty("player")"#));
        s.set_unit_guids(&guids([0xA11CE, 0xB0B, 0, 0], 0xDEAD));
        assert!(nil(&s, r#"UnitInParty("target")"#), "a stranger");

        // Nobody is guid 0, a miss; an unrecognised token raises in the resolver.
        for nobody in [
            "UnitInParty()",
            r#"UnitInParty("")"#,
            r#"UnitInParty("party5")"#,
        ] {
            assert!(nil(&s, nobody), "{nobody}");
        }
        assert!(s.run(r#"UnitInParty("bogus")"#).is_err());
        // With no player object `player` names nobody (`0x5159c9`).
        s.set_unit_guids(&crate::script::UnitGuids::default());
        assert!(nil(&s, r#"UnitInParty("player")"#));
    }

    #[test]
    fn the_raid_verbs_queue_what_they_name() {
        let mut s = UiScript::new().unwrap();
        hold(&mut s, "player", 0x5E1F, "Me");
        s.set_party(two_member_party());
        s.run(
            r#"
            ConvertToRaid()
            SetRaidSubgroup(7, 3)
            SwapRaidSubgroup(7, 12)
            PromoteByName("Alice")
            PromoteToAssistant("Bob")
            DemoteAssistant("Bob")
            UninviteFromRaid(9)
            DoReadyCheck()
            RequestRaidInfo()
            "#,
        )
        .unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![
                PartyRequest::ConvertToRaid,
                PartyRequest::SetSubgroup { index: 7, group: 3 },
                PartyRequest::SwapSubgroup {
                    index: 7,
                    other: 12
                },
                PartyRequest::PromoteGuid(0xA11CE),
                PartyRequest::AssistantLeader {
                    guid: 0xB0B,
                    grant: true
                },
                PartyRequest::AssistantLeader {
                    guid: 0xB0B,
                    grant: false
                },
                PartyRequest::UninviteRaid(9),
                PartyRequest::ReadyCheckStart,
                PartyRequest::RequestRaidInfo,
            ]
        );
        assert!(s.take_party_requests().is_empty(), "the drain empties");
    }

    /// `PromoteByName`, `PromoteToAssistant` and `DemoteAssistant` call `0x493aa0` with typemask
    /// 0x10 and mode 2 (`0x48a86a`, `0x4bbd6b`, `0x4bbe3b`) and send the guid it returns: a prefix
    /// finds a groupmate with no object, a held stranger nearer than the roster's placeholder is
    /// sent too (the server judges the guid), a held groupmate who is dead is not found, and a miss
    /// sends nothing and prints nothing.
    #[test]
    fn the_leader_verbs_send_the_resolvers_guid_whoever_it_is() {
        let calls = [
            (r#"PromoteByName("Ali")"#, false),
            (r#"PromoteToAssistant("Ali")"#, true),
            (r#"DemoteAssistant("Ali")"#, false),
        ];
        let sent = |guid: u64, call: &str, grant: bool| {
            if call.starts_with("Promote") && !call.contains("Assistant") {
                PartyRequest::PromoteGuid(guid)
            } else {
                PartyRequest::AssistantLeader { guid, grant }
            }
        };
        for (call, grant) in calls {
            // The prefix finds Alice, who has no object.
            let mut s = loot_vm(two_member_party());
            s.run(call).unwrap();
            assert_eq!(
                s.take_party_requests(),
                vec![sent(0xA11CE, call, grant)],
                "{call}"
            );
            // A held stranger "Alina" is nearer than the placeholder and an equal prefix.
            s.set_name_candidate(0x57, Some(held_player(0x57, "Alina", 30.0, true)));
            s.run(call).unwrap();
            assert_eq!(
                s.take_party_requests(),
                vec![sent(0x57, call, grant)],
                "{call}"
            );
            // The same stranger dead is no candidate (`0x493eca`), so Alice answers again.
            s.set_name_candidate(0x57, Some(held_player(0x57, "Alina", 30.0, false)));
            s.run(call).unwrap();
            assert_eq!(
                s.take_party_requests(),
                vec![sent(0xA11CE, call, grant)],
                "{call}"
            );
            // No shared first letter: nothing is sent.
            s.run(&call.replace("Ali", "Zed")).unwrap();
            assert!(s.take_party_requests().is_empty(), "{call}");
            assert!(s.take_shown_chat_lines().is_empty(), "{call}");
        }
    }

    /// The reference's No button passes no argument, Yes passes `1`.
    #[test]
    fn confirm_ready_check_treats_an_absent_argument_as_not_ready() {
        let mut s = UiScript::new().unwrap();
        s.run("ConfirmReadyCheck(1) ConfirmReadyCheck() ConfirmReadyCheck(false) ConfirmReadyCheck(0)")
            .unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![
                PartyRequest::ReadyCheckAnswer(true),
                PartyRequest::ReadyCheckAnswer(false),
                PartyRequest::ReadyCheckAnswer(false),
                // `0` truncates to 0 (`GetBoolOrDefault`, `0x4bc0ad`), where Lua calls it true.
                PartyRequest::ReadyCheckAnswer(false),
            ]
        );
    }

    #[test]
    fn saved_instance_info_reads_the_pushed_list() {
        use crate::script::SavedInstanceInfo;
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSavedInstances()").unwrap(), 0);
        s.set_saved_instances(vec![
            SavedInstanceInfo {
                name: "Molten Core".into(),
                instance: 1234,
                reset: 86_400,
            },
            SavedInstanceInfo {
                name: "Onyxia's Lair".into(),
                instance: 77,
                reset: 3_600,
            },
        ]);
        assert_eq!(s.eval::<i64>("return GetNumSavedInstances()").unwrap(), 2);
        let (name, id, reset) = s
            .eval::<(String, i64, i64)>("return GetSavedInstanceInfo(1)")
            .unwrap();
        assert_eq!((name.as_str(), id, reset), ("Molten Core", 1234, 86_400));
        assert_eq!(
            s.eval::<String>("return GetSavedInstanceInfo(2)").unwrap(),
            "Onyxia's Lair"
        );
        for miss in ["0", "3", "-1"] {
            assert!(
                s.eval::<bool>(&format!("return GetSavedInstanceInfo({miss}) == nil"))
                    .unwrap(),
                "index {miss} is a miss"
            );
        }
        assert!(s
            .eval::<i64>(r#"return GetSavedInstanceInfo("x")"#)
            .is_err());
    }

    #[test]
    fn the_raid_roster_selection_is_a_local_cursor() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetRaidRosterSelection()").unwrap(), 0);
        s.run("SetRaidRosterSelection(11)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetRaidRosterSelection()").unwrap(),
            11
        );
        assert!(
            s.take_party_requests().is_empty(),
            "nothing about a selection goes on the wire"
        );
    }

    #[test]
    fn get_raid_target_index_reads_the_fed_mark() {
        let mut s = UiScript::new().unwrap();
        let mut marked = unit(true, 0x40);
        marked.raid_target = 8;
        s.set_unit("target", Some(marked));
        assert_eq!(
            s.eval::<i64>(r#"return GetRaidTargetIndex("target")"#)
                .unwrap(),
            8
        );
        s.set_unit("target", Some(unit(true, 0x40)));
        assert!(s
            .eval::<bool>(r#"return GetRaidTargetIndex("target") == nil"#)
            .unwrap());
    }

    // ── The ready-check timeout ─────────────────────────────────────────────

    /// Us, Alice and Bob, with the two summary strings set.
    fn raid_we_lead(s: &UiScript) -> PartyState {
        s.run(
            r#"RAID_MEMBERS_AFK = "The following players are AFK: %s"
               READY_CHECK_NO_AFK = "No players are AFK""#,
        )
        .unwrap();
        let row = |name: &str, guid: u64| crate::script::RaidMemberInfo {
            name: name.into(),
            guid,
            online: true,
            ..Default::default()
        };
        PartyState {
            members: vec![
                PartyMemberInfo {
                    name: "Alice".into(),
                    guid: 0xA11CE,
                },
                PartyMemberInfo {
                    name: "Bob".into(),
                    guid: 0xB0B,
                },
            ],
            leader_index: 0,
            leader_guid: 0x5E1F,
            own_guid: 0x5E1F,
            raid: vec![
                row("Probefour", 0x5E1F),
                row("Alice", 0xA11CE),
                row("Bob", 0xB0B),
            ],
            loot_method: "group".into(),
            master_looter: None,
            loot_threshold: 2,
        }
    }

    #[test]
    fn the_leaders_check_times_out_into_the_afk_list_and_disarms() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        assert_eq!(s.take_party_requests(), vec![PartyRequest::ReadyCheckStart]);

        s.set_now(129.9);
        s.run("CheckReadyCheckTime()").unwrap();
        assert!(
            s.take_ready_check_lines().is_empty(),
            "not before the deadline"
        );
        s.set_now(130.0);
        s.run("CheckReadyCheckTime()").unwrap();
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["The following players are AFK: Alice, Bob".to_string()]
        );
        s.set_now(200.0);
        s.run("CheckReadyCheckTime()").unwrap();
        assert!(
            s.take_ready_check_lines().is_empty(),
            "disarmed by the summary"
        );
    }

    /// The answer that leaves nobody pending closes the check at once (`0x4ba4d9`).
    #[test]
    fn an_answer_clears_its_member_and_the_last_one_closes_the_check_at_once() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        s.ready_check_answered(0xA11CE, true);
        assert!(
            s.take_ready_check_lines().is_empty(),
            "Bob is still pending"
        );
        s.ready_check_answered(0xB0B, true);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["No players are AFK".to_string()],
            "nobody left pending ⇒ the summary now, not at 130"
        );
        s.set_now(130.0);
        s.run("CheckReadyCheckTime()").unwrap();
        assert!(s.take_ready_check_lines().is_empty());
    }

    /// The leader arm stores a constant 0 whatever the status byte (`0x4ba40e`).
    #[test]
    fn a_not_ready_answer_prints_its_line_and_still_clears_the_member() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.run(r#"RAID_MEMBER_NOT_READY = "%s is not ready""#)
            .unwrap();
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        s.ready_check_answered(0xDEAD, false);
        assert!(
            s.take_ready_check_lines().is_empty(),
            "an unmatched guid writes nothing"
        );
        s.ready_check_answered(0xA11CE, false);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["Alice is not ready".to_string()]
        );
        s.set_now(130.0);
        s.run("CheckReadyCheckTime()").unwrap();
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["The following players are AFK: Bob".to_string()],
            "Alice answered — not ready is not AFK"
        );
    }

    /// The close tests online (`0x4ba4a7 test byte [eax+0x18],dl`); the summary's walk does not
    /// (`0x4bb3a8`).
    #[test]
    fn an_offline_member_never_blocks_the_close_but_is_still_listed() {
        let mut s = UiScript::new().unwrap();
        let mut party = raid_we_lead(&s);
        party
            .raid
            .iter_mut()
            .for_each(|m| m.online = m.guid != 0xB0B); // Bob is offline
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        s.ready_check_answered(0xA11CE, true);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["The following players are AFK: Bob".to_string()],
            "Alice's answer was the last ONLINE one, so the check closed and listed offline Bob"
        );
    }

    /// A member's client arms the deadline, never clears it, and never prints.
    #[test]
    fn the_tick_is_inert_for_a_non_leader() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"READY_CHECK_NO_AFK = "No players are AFK""#)
            .unwrap();
        s.set_party(two_member_party()); // Alice leads; we are 0x5E1F
        s.set_now(100.0);
        s.ready_check_request(false);
        s.set_now(130.0);
        s.run("CheckReadyCheckTime()").unwrap();
        assert!(s.take_ready_check_lines().is_empty());
        assert_eq!(s.model_mut().ready_check.deadline, Some(130.0));
    }

    #[test]
    fn a_roster_change_with_nobody_left_pending_closes_the_check() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party.clone());
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        s.ready_check_answered(0xA11CE, true);
        let mut without_bob = party;
        without_bob.raid.retain(|m| m.guid != 0xB0B);
        without_bob.members.retain(|m| m.guid != 0xB0B);
        s.set_party(without_bob);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["No players are AFK".to_string()]
        );
    }

    /// A party has no raid roster, so `DoReadyCheck` flags nobody.
    #[test]
    fn the_leaders_own_echo_closes_an_empty_check_at_once() {
        let mut s = UiScript::new().unwrap();
        let mut party = raid_we_lead(&s);
        party.raid.clear();
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        s.ready_check_request(true);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["No players are AFK".to_string()]
        );
    }

    /// Both keys are read raw from `_G` with an empty default.
    #[test]
    fn a_missing_summary_string_prints_an_empty_line() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party);
        s.set_now(100.0);
        s.run("RAID_MEMBERS_AFK = nil DoReadyCheck()").unwrap();
        s.set_now(130.0);
        s.run("CheckReadyCheckTime()").unwrap();
        assert_eq!(s.take_ready_check_lines(), vec![String::new()]);
    }

    /// Make the three ready-check strings come from an `_G` `__index` that touches the Model, as a
    /// strict-mode or global-tracking addon's does (`CreateFrame` borrows it mutably), and record
    /// each key it is asked for in `reads`, in order. The globals are cleared so every read reaches
    /// it: the reference's lookup is `lua_gettable` on the globals (`0x704373`).
    fn model_touching_index(s: &UiScript) {
        s.run(
            r#"reads = ""
               RAID_MEMBERS_AFK = nil
               READY_CHECK_NO_AFK = nil
               RAID_MEMBER_NOT_READY = nil
               local text = {
                   RAID_MEMBER_NOT_READY = "%s is not ready",
                   RAID_MEMBERS_AFK = "AFK: %s",
                   READY_CHECK_NO_AFK = "No players are AFK",
               }
               setmetatable(getfenv(0), { __index = function(t, k)
                   if text[k] then
                       reads = reads .. k .. ","
                       CreateFrame("Frame"):SetWidth(10)
                       return text[k]
                   end
               end })"#,
        )
        .unwrap();
    }

    fn reads(s: &UiScript) -> String {
        s.eval::<String>("return reads").unwrap()
    }

    /// `0x4ba414` reads `RAID_MEMBER_NOT_READY` through `0x703bf0`, so a `_G` `__index` runs inside
    /// the answer. It must find the Model free.
    #[test]
    fn a_not_ready_answer_reads_its_string_with_the_model_free() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        model_touching_index(&s);
        s.ready_check_answered(0xA11CE, false);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["Alice is not ready".to_string()]
        );
        assert_eq!(reads(&s), "RAID_MEMBER_NOT_READY,");
        assert!(s.take_errors().is_empty());
    }

    /// The last answer force-closes (`0x4ba4ce` → `0x4bb310`): the worker reads `RAID_MEMBERS_AFK`
    /// first, before it walks the roster (`0x4bb381`), and `READY_CHECK_NO_AFK` when it finds no
    /// name (`0x4bb48e`), so both lookups run for an empty list.
    #[test]
    fn the_force_close_from_the_last_answer_reads_its_strings_with_the_model_free() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        model_touching_index(&s);
        s.ready_check_answered(0xA11CE, true);
        s.ready_check_answered(0xB0B, false);
        assert_eq!(
            s.take_ready_check_lines(),
            vec![
                "Bob is not ready".to_string(),
                "No players are AFK".to_string()
            ]
        );
        assert_eq!(
            reads(&s),
            "RAID_MEMBER_NOT_READY,RAID_MEMBERS_AFK,READY_CHECK_NO_AFK,"
        );
        assert!(s.take_errors().is_empty());
    }

    /// The deadline passing reads one string when a name is pending, and an `__index` that
    /// re-enters the tick finds the check already disarmed (`0x4bb377` precedes `0x4bb381`), so the
    /// summary prints once.
    #[test]
    fn the_timeout_reads_its_string_with_the_model_free_and_already_disarmed() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        s.ready_check_answered(0xA11CE, true);
        model_touching_index(&s);
        s.run(
            r#"local inner = getmetatable(getfenv(0)).__index
               getmetatable(getfenv(0)).__index = function(t, k)
                   if k == "RAID_MEMBERS_AFK" then CheckReadyCheckTime() end
                   return inner(t, k)
               end"#,
        )
        .unwrap();
        s.set_now(130.0);
        s.run("CheckReadyCheckTime()").unwrap();
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["AFK: Bob".to_string()],
            "one summary"
        );
        assert_eq!(reads(&s), "RAID_MEMBERS_AFK,");
        assert!(s.take_errors().is_empty());
    }

    /// The roster push and the request echo reach the same force-close.
    #[test]
    fn the_force_close_from_a_roster_push_and_from_the_echo_reads_with_the_model_free() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party.clone());
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        s.ready_check_answered(0xA11CE, true);
        model_touching_index(&s);
        let mut without_bob = party;
        without_bob.raid.retain(|m| m.guid != 0xB0B);
        without_bob.members.retain(|m| m.guid != 0xB0B);
        s.set_party(without_bob);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["No players are AFK".to_string()]
        );

        let mut s = UiScript::new().unwrap();
        let mut party = raid_we_lead(&s);
        party.raid.clear();
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        model_touching_index(&s);
        s.ready_check_request(true);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["No players are AFK".to_string()]
        );
        assert!(s.take_errors().is_empty());
    }
}
