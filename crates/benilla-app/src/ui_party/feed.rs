//! The party lane, a new VM's seed and the drain: fire the party events at their handlers'
//! places, seat a new VM's place on the lane and the lockouts before its files run, and answer
//! what the party calls left ([`super::calls`]). The group getters read the roster at the call
//! ([`super::live`]), and the `party1..4`, `raid1..40` and their pets' tokens off the members'
//! objects and records ([`crate::ui_unit::live`]). The state and its lines live in the parent
//! module.

use benilla_protocol::messages::{
    member_status, GroupLootInfo, GroupMemberEntry, ObjectType, PartyMemberStatsInfo,
    GROUP_MEMBER_ASSISTANT,
};
use benilla_ui::script::{PartyRequest, RaidMemberInfo, SavedInstanceInfo, ScriptValue, UiScript};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, NetCommands, ObjectStore, SelfPlayer};

use super::{GroupSignal, GroupState};
use crate::query_cache::Ask;

// ─── The party lane and a new VM's seed ──────────────────────────────────────────────────────────

/// The lockouts with their `Map.dbc` names, the map id standing for a name the catalog lacks.
fn saved_list(
    entries: &[benilla_protocol::messages::RaidInstanceEntry],
    catalog: Option<&benilla_formats::MapCatalog>,
) -> Vec<SavedInstanceInfo> {
    entries
        .iter()
        .map(|e| SavedInstanceInfo {
            name: catalog
                .and_then(|c| c.name(e.map))
                .map_or_else(|| e.map.to_string(), str::to_string),
            instance: e.instance,
            reset: e.reset,
        })
        .collect()
}

/// The party lane's place in [`GroupState::signals`]: the sequence number of the first entry it
/// has not fired. A new VM's birth moves it past what was logged ([`seed_vm_party`]).
#[derive(Resource, Default)]
pub(crate) struct PartyLane {
    fired: u64,
}

/// The party lane's head ([`crate::lanes`]): the stamp of the first entry it has not fired.
pub(super) fn peek_group_signals(world: &World) -> Option<crate::lanes::Stamp> {
    let lane = world.get_resource::<PartyLane>()?;
    let log = &world.get_resource::<GroupState>()?.signals;
    log.at(lane.fired.max(log.first())).map(|(stamp, _)| *stamp)
}

/// The party lane's `fire_until`: each group event `until` admits, signalled with the host lent at
/// its handler's place among the other lanes' items; the getters its handlers call read the
/// roster at the call. Nothing gates one on a difference: a list that changes nothing still
/// signals, and a stats packet signals none. `RAID_ROSTER_UPDATE`'s fourth site, the world entry's
/// fill of a zero row (`0x4ba1a2`), is not built: our row holds our guid ([`GroupState::raid`]).
/// The lane stops after an event whose firing stamped anything, so what it pushed fires next.
#[allow(clippy::type_complexity)] // the lane's own state: the log, its place and the map names
pub(super) fn fire_group_signals(
    In(until): In<crate::lanes::Until>,
    mut vm: crate::script_calls::VmWith<(
        Res<'static, GroupState>,
        ResMut<'static, PartyLane>,
        Option<Res<'static, benilla_assets::MapCatalogRes>>,
    )>,
    mut sink: crate::ui_action::MessageSink,
    // Absent in a harness without the tutorials.
    tutorials: Option<Res<crate::tutorial::Tutorials>>,
) {
    let tutorials = tutorials.as_deref();
    loop {
        let next = {
            let (group, mut lane, maps) = vm.own();
            let seq = lane.fired.max(group.signals.first());
            match group.signals.at(seq) {
                Some((stamp, signal)) if until.admits(*stamp) => {
                    lane.fired = seq + 1;
                    // The lockouts' names, as the catalog names them now.
                    let saved = match signal {
                        GroupSignal::InstanceInfo(entries) => {
                            saved_list(entries, maps.as_deref().map(|m| &m.0))
                        }
                        _ => Vec::new(),
                    };
                    Some((*stamp, signal.clone(), saved))
                }
                _ => None,
            }
        };
        let Some((stamp, signal, saved)) = next else {
            return;
        };
        let ((), moved) = crate::lanes::firing(stamp, || {
            vm.run(|script| {
                if let Some(script) = script {
                    fire_signal(script, &mut sink, tutorials, signal, saved);
                }
            })
        });
        if moved {
            return;
        }
    }
}

/// The group events waiting, fired alone, for a harness with no call host and no other lanes: each
/// with the roster's reads lent ([`super::RosterLive`], [`UiScript::reading`]), as the app's lane
/// fires with its host's, so a listener reads the group at the call; its calls wait in the VM, as
/// with no host they do.
#[cfg(test)]
pub(crate) fn fire_party_lane(world: &mut World) {
    use bevy::ecs::system::SystemState;
    struct Roster<'a, 'w, 's>(&'a mut super::RosterLive<'w, 's>);
    impl benilla_ui::script::LiveReader for Roster<'_, '_, '_> {
        fn read_live(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
            use crate::script_calls::live::LiveFamily;
            self.0.answer(ask)
        }
    }
    let mut state = SystemState::<(
        super::RosterLive,
        crate::ui_action::MessageSink,
        Option<Res<crate::tutorial::Tutorials>>,
    )>::new(world);
    loop {
        let next = {
            let Some(group) = world.get_resource::<GroupState>() else {
                return;
            };
            let seq = world
                .resource::<PartyLane>()
                .fired
                .max(group.signals.first());
            group.signals.at(seq).map(|(stamp, signal)| {
                let maps = world
                    .get_resource::<benilla_assets::MapCatalogRes>()
                    .map(|m| &m.0);
                let saved = match signal {
                    GroupSignal::InstanceInfo(entries) => saved_list(entries, maps),
                    _ => Vec::new(),
                };
                (seq, *stamp, signal.clone(), saved)
            })
        };
        let Some((seq, stamp, signal, saved)) = next else {
            return;
        };
        world.resource_mut::<PartyLane>().fired = seq + 1;
        let mut script = world.remove_non_send_resource::<UiScript>().expect("a VM");
        crate::lanes::firing(stamp, || {
            let (mut roster, mut sink, tutorials) = state.get(world);
            script.reading(&mut Roster(&mut roster), |script| {
                fire_signal(script, &mut sink, tutorials.as_deref(), signal, saved);
            });
        });
        world.insert_non_send_resource(script);
    }
}

/// One group event as its handler signals it.
fn fire_signal(
    script: &mut UiScript,
    sink: &mut crate::ui_action::MessageSink,
    tutorials: Option<&crate::tutorial::Tutorials>,
    signal: GroupSignal,
    saved: Vec<SavedInstanceInfo>,
) {
    let (signalled, args) = match signal {
        GroupSignal::MembersChanged => ("PARTY_MEMBERS_CHANGED", vec![]),
        GroupSignal::LeaderChanged => ("PARTY_LEADER_CHANGED", vec![]),
        GroupSignal::LootMethodChanged => ("PARTY_LOOT_METHOD_CHANGED", vec![]),
        GroupSignal::RaidRosterUpdate => ("RAID_ROSTER_UPDATE", vec![]),
        GroupSignal::RaidTargetUpdate => {
            script.refresh_nameplate_raid_icons();
            ("RAID_TARGET_UPDATE", vec![])
        }
        // With the 1-based slot as a number (`%d`, `0x835154`). Stock FrameXML ignores them
        // (`PartyMemberFrame.lua:145` is commented out).
        GroupSignal::MemberEnabled(slot) => (
            "PARTY_MEMBER_ENABLE",
            vec![ScriptValue::Int(i64::from(slot))],
        ),
        GroupSignal::MemberDisabled(slot) => (
            "PARTY_MEMBER_DISABLE",
            vec![ScriptValue::Int(i64::from(slot))],
        ),
        // `0x5e6730` signals the event (`0x5e6747`), then shows `ERR_INVITED_TO_GROUP_S` (message
        // `0x3c`, `0x5e6752`), so the line follows the popup's handlers.
        GroupSignal::InviteRequest(inviter) => {
            script.fire_event(
                "PARTY_INVITE_REQUEST",
                vec![ScriptValue::Str(inviter.clone())],
            );
            if let Some(line) =
                crate::ui_action::keyed_line_s(script, "ERR_INVITED_TO_GROUP_S", &[&inviter])
            {
                crate::ui_action::show_now(script, sink, "ui_party", line);
            }
            return;
        }
        // The open form arms the engine's side (`0x4ba535`) before the popup's event (`0x4ba53a`).
        GroupSignal::ReadyCheckRequest { we_lead } => {
            script.ready_check_request(we_lead);
            return;
        }
        GroupSignal::ReadyCheck => ("READY_CHECK", vec![]),
        GroupSignal::RaidRebuilt { gone } => {
            script.raid_roster_rebuilt(gone);
            return;
        }
        GroupSignal::Tutorial(id) => {
            if let Some(published) = tutorials.and_then(|t| t.lock().fire_now(id)) {
                crate::tutorial::fire_published(script, &sink.sounds, published);
            }
            return;
        }
        GroupSignal::ReadyCheckAnswer { guid, ready } => {
            script.ready_check_answered(guid, ready);
            return;
        }
        // The list lands, then the event follows the answer (`0x49e1a7`).
        GroupSignal::InstanceInfo(_) => {
            script.set_saved_instances(saved);
            ("UPDATE_INSTANCE_INFO", vec![])
        }
        GroupSignal::Line(line) => {
            let shown = crate::ui_action::ui_error_text(&line, &|key| {
                script.lua().globals().get::<String>(key).ok()
            })
            .map(|text| crate::ui_action::Shown::keyed(line.key, text));
            if let Some(shown) = shown {
                crate::ui_action::show_now(script, sink, "ui_party", shown);
            }
            return;
        }
    };
    script.fire_event(signalled, args);
}

/// What `InitializeGame`'s group init zeroes at a world login and a `/reload` leaves (`0x4ba0b0`,
/// from `0x48f603`): the ready check a dying VM held and the raid roster selection's guid
/// (`[0xb713e8]`, `0x4ba0e7`), for the next VM's seed ([`seed_vm_party`]).
#[derive(Resource, Default)]
pub(crate) struct ReadyCheckCarry(benilla_ui::script::ReadyCheckState, u64);

/// Fold the ready check and the raid selection out of the dying VM, with the item locks: the
/// deadline (`[0xb713f4]`), the pending flags and the selection are engine state, which no reload
/// path writes.
pub(crate) fn fold_dying_vm_ready_check(world: &mut World) {
    let Some((state, selection)) = world.get_non_send_resource_mut::<UiScript>().map(|mut s| {
        let selection = s.take_raid_selection();
        (s.take_ready_check(), selection)
    }) else {
        return;
    };
    world.insert_resource(ReadyCheckCarry(state, selection));
}

/// Seat a new in-game VM's group state before its files run, at either birth: the ready check and
/// the raid selection, a `/reload`'s the dying VM's and a world entry's none, as `InitializeGame`'s
/// group init zeroes them (`0x4ba130`, `0x4ba0e7`, from `0x48f603`); the lockouts; and the party
/// lane's place, past every signal logged before it.
///
/// The lockouts are the engine's, which a reload keeps: the handler stores the list and its count
/// (`[0xb6e5e8]`, `0x49e1a2`) before it signals (`0x49e1a7`), and `GetNumSavedInstances` reads the
/// count at the call (`0x4a18f6`). The lane tells the new VM only what is signalled after its
/// birth. A world entry builds its interface before the login goes out (`UI_Init` from
/// `0x46c236`, the send at `0x46c272`), so nothing of its session was signalled before it, as the
/// session's packets wait behind the load here (`net::apply::InboundHold`). A `/reload` signals
/// none of the group events: their sites are the group packets' handlers and the name callback,
/// and the world entry's raid fill signals only for a zero row (`0x4ba140`, `0x4ba15c`), which the
/// first entry filled; its frames read the group as they load (`PartyMemberFrame.lua:20-24`,
/// `RaidFrame.lua:32`), at the call.
pub(crate) fn seed_vm_party(world: &mut World, script: &mut UiScript, reload: bool) {
    let carried = world
        .remove_resource::<ReadyCheckCarry>()
        .unwrap_or_default();
    if reload {
        script.seed_ready_check(carried.0);
        script.seed_raid_selection(carried.1);
    }
    let Some(group) = world.get_resource::<GroupState>() else {
        return;
    };
    let catalog = world
        .get_resource::<benilla_assets::MapCatalogRes>()
        .map(|c| &c.0);
    script.set_saved_instances(saved_list(&group.saved_instances, catalog));
    let end = group.signals.end();
    if let Some(mut lane) = world.get_resource_mut::<PartyLane>() {
        lane.fired = lane.fired.max(end);
    }
}

/// `SMSG_GROUP_LIST`'s first byte for a raid; 0 is a party (vmangos `Group/Group.h:119`).
pub(crate) const GROUPTYPE_RAID: u8 = 1;

/// The subgroup bits (0-2) of a member's flags byte; `0x80` is the assistant bit.
pub(crate) const GROUP_MEMBER_SUBGROUP: u8 = 0x07;

/// `GetLootMethod`'s method word, its second return and `GetLootThreshold`, off the stored cells
/// with no group test (`0x4e91b6`, `0x4e91fb`, `0x4e94e0`).
pub(super) fn loot_read(
    cells: super::Cells<'_>,
    self_guid: Option<u64>,
) -> (&'static str, Option<u32>, u32) {
    let loot = cells.loot_cells();
    // Methods 0 to 4 through the table `0x4e9280`, `ERROR!` above (`0x4e91ef`).
    let method = match loot.method {
        0 => "freeforall",
        1 => "roundrobin",
        2 => "master",
        3 => "group",
        4 => "needbeforegreed",
        _ => "ERROR!",
    };
    // `GetLootMethod`'s second return, for any looter in the cell whatever the method
    // (`0x4e9209`): 0 when it is us (`0x4e9215`), else its party slot + 1 over the four slots
    // only (`0x4e9238`, `0x4e81a0`, even in a raid), else nil. A looter in another raid
    // subgroup reads nil, never 0, which would light the crown on our portrait
    // (`PlayerFrame.lua:48`).
    let master = (loot.master != 0)
        .then(|| {
            if Some(loot.master) == self_guid {
                Some(0)
            } else {
                cells
                    .party_slots()
                    .position(|m| m.guid == loot.master)
                    .map(|i| i as u32 + 1)
            }
        })
        .flatten();
    (method, master, u32::from(loot.threshold))
}

/// What our own raid row reads off our object: `SMSG_GROUP_LIST` never lists the recipient, and
/// the row itself is the one [`super::RaidRow::own`] marks.
pub(super) struct RaidSelf {
    pub(super) guid: u64,
    pub(super) level: u32,
    /// The area under us, walked to a zone by the caller (the reference reads `0xb4e314`).
    pub(super) area: Option<u32>,
    /// Return 9's live-object arm, health 0 or below: `unit_is_dead`, not `unit_reads_dead`, so
    /// a feigning hunter does not read dead.
    pub(super) dead: bool,
    /// Our class byte, off our descriptor: the login seeds our own name without traits
    /// (`net::session::connected`), so the name cache never has our class.
    pub(super) class: Option<u8>,
}

/// The raid rows as guids ([`GroupState::raid_rows`]), the order [`raid_row_at`] reads, which
/// the RaidFrame's drag, kick and menu verbs address by index: the wire's members, then us
/// (`0x5e6e73`). Empty while no roster is held.
pub(crate) fn raid_row_guids(group: &GroupState) -> Vec<u64> {
    group.raid_rows().collect()
}

/// The guid at a 1-based raid row, [`raid_row_guids`]`[index - 1]` without the allocation.
pub(crate) fn raid_row_guid(group: &GroupState, index: usize) -> Option<u64> {
    group.raid_rows().nth(index.checked_sub(1)?)
}

/// A 1-based raid row index to its guid; row 0, a row past the end or no raid gives `None`.
pub(super) fn raid_guid_at(group: &GroupState, index: u32) -> Option<u64> {
    raid_row_guid(group, usize::try_from(index).ok()?)
}

/// A 1-based raid row's subgroup as the roster holds it, the row's `+8` (`0x4ba8ea`, the flags
/// under 0x7f): a member's from the list's flags, ours, the row after them, from our own
/// (`0x5e6e82`). No row, no subgroup.
pub(super) fn raid_row_subgroup(group: &GroupState, index: u32) -> Option<u8> {
    let row = usize::try_from(index).ok()?.checked_sub(1)?;
    group.raid.get(row).map(|r| r.subgroup)
}

/// The name the subgroup verbs send: the cache's record, looked up with no callback
/// (`0x4bba5e`, `0x4bbbc4`, `0x4bbbe5`), so a name still to come sends nothing.
pub(super) fn cached_player_name(names: &NameCache, guid: u64) -> Option<String> {
    names.resolve_player(guid, Ask::Read).map(str::to_string)
}

/// What a raid row reads: the roster's cells and records, our own object, the object manager, the
/// name cache, the class table and the area names.
pub(super) struct RowReads<'r, 'a> {
    pub(super) cells: super::Cells<'r>,
    pub(super) records: &'r std::collections::HashMap<u64, PartyMemberStatsInfo>,
    pub(super) me: Option<&'r RaidSelf>,
    pub(super) held: &'r dyn Fn(u64) -> Option<&'a ObjectStore>,
    pub(super) names: &'r NameCache,
    pub(super) unit_names: crate::ui_unit::UnitNames<'r>,
    pub(super) zone_name: &'r dyn Fn(u32) -> Option<String>,
}

/// The stored raid row at 0-based `index`, `GetRaidRosterInfo`'s array (`0x4bb560`) in
/// [`raid_row_guids`]' order: the wire's members in list order, then us; `None` past the last, and
/// while no roster is held, so `GetNumRaidMembers()` answers 0 in a party and in a raid the list
/// named nobody else in. The reference's array holds the local player and the wire's list does
/// not, so our row comes from [`RaidSelf`]; with no object of ours, as across a worldport, the row
/// stays, online as the list wrote it (`0x5e6e94`), its level, zone and class not derived.
///
/// A row whose unit the object manager holds (`0x4bb64b`, `held`) reads it: online (`0x4bb777`),
/// its descriptor's level (`0x4bb65d`), the zone under us (`0x4bb739`, `[0xb4e314]`) and dead for
/// health 0 or below (`0x4bb796`). A row with no object reads its record's online bit
/// (`[row+0x18] & 1`), which gates the level (`0x4bb668`-`0x4bb682`), the zone (`0x4bb6e5`, the
/// binding's `PLAYER_OFFLINE`) and dead, which needs the record's dead bit too (`0x4bb70a`-
/// `0x4bb717`). The roster status byte reaches none of them: a list writes only the record's bit 0
/// (`0x4ba947`-`0x4ba960`).
pub(super) fn raid_row_at(r: &RowReads, index: usize) -> Option<RaidMemberInfo> {
    let RowReads {
        cells,
        records,
        me,
        held,
        names,
        unit_names,
        zone_name,
    } = r;
    let stored = cells.raid.get(index)?;
    // The class byte comes from the name cache for a member, from our descriptor for us.
    let row = |name: String,
               level: u32,
               zone: Option<String>,
               online: bool,
               ninth: bool,
               class_byte: Option<u8>| {
        let class = class_byte.and_then(|c| unit_names.class(c));
        RaidMemberInfo {
            name,
            guid: stored.guid,
            rank: stored.rank,
            // Stored 0-based; the binding adds one (`0x4bb61a`).
            subgroup: u32::from(stored.subgroup & GROUP_MEMBER_SUBGROUP),
            level,
            class: class.map(|(n, _)| n.to_string()),
            class_file: class.map(|(_, f)| f.to_string()),
            zone,
            online,
            ninth,
        }
    };
    let our_zone = || me.and_then(|me| me.area).and_then(zone_name);
    let guid = stored.guid;
    if !stored.own {
        let class = names.player_traits(guid).map(|(_, class, _)| class);
        let (level, zone, online, ninth) = match held(guid) {
            Some(store) => (
                store.0.unit_level().unwrap_or(0),
                our_zone(),
                true,
                store.0.unit_health().unwrap_or(0) as i32 <= 0,
            ),
            None => {
                let record = records.get(&guid);
                let online = record.is_some_and(PartyMemberStatsInfo::is_online);
                let status = record.and_then(|r| r.status).unwrap_or(0);
                (
                    record
                        .filter(|_| online)
                        .and_then(|r| r.level)
                        .map_or(0, u32::from),
                    record
                        .filter(|_| online)
                        .and_then(|r| r.zone)
                        .and_then(|z| zone_name(u32::from(z))),
                    online,
                    // Both `0x4` and `0x1`, vmangos's `DEAD` and `ONLINE` (`Group/Group.cpp:45-63`):
                    // an offline dead member answers nil.
                    online && status & member_status::DEAD != 0,
                )
            }
        };
        return Some(row(stored.name.clone(), level, zone, online, ninth, class));
    }
    let me = me.filter(|me| me.guid == guid);
    Some(row(
        // An unresolved name gets the binding's miss tuple, not a half-filled row (`0x4bb5f8`).
        names
            .resolve_unit(guid, None, Ask::Read)
            .unwrap_or_default()
            .to_string(),
        me.map_or(0, |me| me.level),
        me.and(our_zone()),
        true,
        me.is_some_and(|me| me.dead),
        me.and_then(|me| me.class),
    ))
}

/// Every frame: age the signal log, and answer what the party calls left
/// ([`super::calls::GroupEdits`]): the sandbox's group-changing calls, echoed onto the mirror as
/// the server would answer them.
pub(crate) fn drain_party(
    mut group: ResMut<GroupState>,
    mut edits: ResMut<super::calls::GroupEdits>,
    self_q: Query<&Guid, With<SelfPlayer>>,
    mut roster: Option<ResMut<crate::spell::group_relation::GroupRoster>>,
) {
    // What the feed has fired goes ([`super::SignalLog`]). Not a change the feed's gate should
    // open on.
    group.bypass_change_detection().signals.age();
    let self_guid = self_q.iter().next().map(|g| g.0);
    for echo in std::mem::take(&mut edits.sandbox) {
        match echo {
            super::calls::SandboxEcho::Request(request) => {
                let echoed = test_apply_local(&mut group, &request, self_guid);
                debug_assert!(echoed, "only a sandboxed call is echoed: {request:?}");
                // The tables the relation reads take, as the list the echo stands for writes them.
                crate::spell::group_relation::publish_group_roster(&group, roster.as_mut());
            }
            super::calls::SandboxEcho::RaidTarget { icon, guid } => {
                echo_raid_target(&mut group, icon, guid);
            }
        }
    }
}

/// `UninviteByGuid` (`0x5e9510`), the sender `UninviteFromParty` (`0x48a560`) and
/// `UninviteFromRaid` (`0x48a5ed`) both end in. It asks the object manager for the guid's player
/// object (`0x5e951f`, type mask `0x10`): one held is kicked by its unit's name in
/// `CMSG_GROUP_UNINVITE` (`0x5e95a7`-`0x5e95d0`), which `GetUnitName` (`0x609210`) reads from the
/// player-name cache with a callback, else as `UNKNOWNOBJECT`; any other, by its guid in
/// `CMSG_GROUP_UNINVITE_GUID` (`0x5e953e`). Only `UninviteByName` always sends the name
/// (`0x48a62a`).
pub(super) fn uninvite_by_guid(
    guid: u64,
    held: Option<&ObjectStore>,
    names: &NameCache,
    commands: &NetCommands,
    script: &UiScript,
) {
    let player = held.filter(|s| matches!(s.0.object_type(), Some(ObjectType::Player)));
    let command = match player {
        Some(store) => ClientCommand::GroupUninvite {
            name: names
                .resolve_unit(guid, Some(store), Ask::Event(commands))
                .map_or_else(|| script.unknown_object_text(), str::to_string),
        },
        None => ClientCommand::GroupUninviteGuid { guid },
    };
    let _ = commands.0.send(command);
}

/// What `SetRaidTarget` (`0x4bbec0`) does for the nonzero guid its token named.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum RaidTargetMark {
    /// `MSG_RAID_TARGET_UPDATE` with this wire slot and guid (`0x4bc01b`-`0x4bc03a`).
    Send { icon: u8, guid: u64 },
    /// `ERR_INVALID_RAID_TARGET` (`0x4bbfd2`), and nothing sent.
    Invalid,
    /// Nothing at all.
    Nothing,
}

/// `SetRaidTarget`'s tail for `guid` and the index's low byte (`0x4bbf23`): a byte of 1 to 8 marks
/// slot `byte - 1` when `markable` holds (`0x4bbf48`-`0x4bbfd0`); any other byte, 0 included
/// (`0x4bbf29` wraps it), clears the slot the guid holds by sending it with guid 0
/// (`0x4bbf36`-`0x4bbf40`), and nothing for a guid holding none (`0x4bbfed`). A clear asks
/// nothing of the unit.
pub(super) fn raid_target_mark(
    group: &GroupState,
    guid: u64,
    index: u8,
    markable: impl FnOnce() -> bool,
) -> RaidTargetMark {
    let slot = index.wrapping_sub(1);
    if slot >= 8 {
        return match group.raid_target_index(guid) {
            0 => RaidTargetMark::Nothing,
            held => RaidTargetMark::Send {
                icon: held - 1,
                guid: 0,
            },
        };
    }
    if markable() {
        RaidTargetMark::Send { icon: slot, guid }
    } else {
        RaidTargetMark::Invalid
    }
}

/// The sandbox's stand-in for the server's answer to a mark: `Group::SetTargetIcon` clears the
/// unit's old icon first, one mark per unit, and its `MSG_RAID_TARGET_UPDATE` has one signal
/// (`0x4ba348`).
fn echo_raid_target(group: &mut GroupState, icon: u8, guid: u64) {
    if guid != 0 {
        for slot in group.raid_targets.iter_mut() {
            if *slot == guid {
                *slot = 0;
            }
        }
    }
    group.apply_raid_target_update(icon, guid);
}

/// The sandbox's echo of one group-changing call ([`drain_party`]), applied to the mirror as the
/// server's answer would be. False for the calls that stay real (invites, accept and decline),
/// which [`super::calls`] never hands it.
fn test_apply_local(group: &mut GroupState, req: &PartyRequest, self_guid: Option<u64>) -> bool {
    let was_held = group.raid_held();
    let handled = apply_test_intent(group, req, self_guid);
    // The server answers each of these with a fresh `SMSG_GROUP_LIST`, signalled in the order a
    // real one is ([`GroupState::apply_list_awaiting`]): `PARTY_MEMBERS_CHANGED` whether or not it
    // changed anything (`0x5e6c66`), the leader store's index and its signal for a list naming a
    // leader, the loot store's signal for one listing members, the raid roster as a raid list
    // writes it, our row after any members (`0x5e6e6f`-`0x5e6e9c`), with its `RAID_ROSTER_UPDATE`
    // (`0x4babef`) or the one that ends a raid for the roster it drops (`0x4ba57b`), and a party's
    // board.
    if matches!(
        req,
        PartyRequest::Leave
            | PartyRequest::UninviteGuid(_)
            | PartyRequest::LootMethod { .. }
            | PartyRequest::LootThreshold(_)
            | PartyRequest::ConvertToRaid
            | PartyRequest::SetSubgroup { .. }
            | PartyRequest::SwapSubgroup { .. }
            | PartyRequest::UninviteRaid(_)
            | PartyRequest::PromoteGuid(_)
            | PartyRequest::AssistantLeader { .. }
    ) {
        let raid = group.group_type == GROUPTYPE_RAID;
        group.raid = if raid {
            super::RaidRow::rebuilt(
                &group.members,
                (self_guid.unwrap_or(0), group.own_flags),
                group.leader,
            )
        } else {
            Vec::new()
        };
        group.store_leader_index();
        group.signal(GroupSignal::MembersChanged);
        if group.leader != 0 {
            group.signal(GroupSignal::LeaderChanged);
        }
        if !group.members.is_empty() {
            group.signal(GroupSignal::LootMethodChanged);
        }
        if raid || was_held {
            group.signal(GroupSignal::RaidRosterUpdate);
        }
        // A party's list clears the board and signals (`0x4ba5db`), and vmangos resends any icons
        // right after it (`Group.cpp:1358`), so the board comes back with a second signal.
        if group.group_type != GROUPTYPE_RAID {
            group.signal(GroupSignal::RaidTargetUpdate);
            if group.raid_targets != [0; 8] && group.in_group {
                group.signal(GroupSignal::RaidTargetUpdate);
            } else {
                group.raid_targets = [0; 8];
            }
        }
    }
    handled
}

/// [`test_apply_local`]'s mirror edit: the intent's effect on the group, whatever list answers it.
fn apply_test_intent(group: &mut GroupState, req: &PartyRequest, self_guid: Option<u64>) -> bool {
    match req {
        PartyRequest::Leave => {
            // The all-zero list's reset: the group facts and sandbox flag, not the session state.
            group.leave_group();
            true
        }
        PartyRequest::UninviteGuid(guid) => {
            group.members.retain(|m| m.guid != *guid);
            group.stats.remove(guid);
            true
        }
        PartyRequest::LootMethod {
            method,
            master,
            threshold,
        } => {
            group.loot = Some(GroupLootInfo {
                method: *method as u8,
                master: *master,
                threshold: *threshold as u8,
            });
            true
        }
        PartyRequest::LootThreshold(threshold) => {
            let (method, master) = group.loot.map_or((3, 0), |l| (l.method, l.master));
            group.loot = Some(GroupLootInfo {
                method,
                master,
                threshold: *threshold as u8,
            });
            true
        }
        // ── The raid verbs, sandboxed ─────────────────────────────────────────
        //
        // So `/partytest raid` exercises the drag, the ready check and the kick without a raid.
        PartyRequest::ConvertToRaid => {
            group.group_type = 1;
            true
        }
        PartyRequest::SetSubgroup { index, group: sub } => {
            if (1..=8).contains(sub) {
                set_test_subgroup(group, self_guid, *index, (*sub - 1) as u8);
            }
            true
        }
        PartyRequest::SwapSubgroup { index, other } => {
            let a = test_subgroup_of(group, self_guid, *index);
            let b = test_subgroup_of(group, self_guid, *other);
            if let (Some(a), Some(b)) = (a, b) {
                set_test_subgroup(group, self_guid, *index, b);
                set_test_subgroup(group, self_guid, *other, a);
            }
            true
        }
        PartyRequest::UninviteRaid(index) => {
            // Never our own row; the server ignores that too.
            if let Some(guid) = raid_guid_at(group, *index).filter(|g| Some(*g) != self_guid) {
                group.members.retain(|m| m.guid != guid);
                group.stats.remove(&guid);
            }
            true
        }
        PartyRequest::PromoteGuid(guid) => {
            if group.members.iter().any(|m| m.guid == *guid) {
                group.leader = *guid;
            }
            true
        }
        PartyRequest::AssistantLeader { guid, grant } => {
            if let Some(m) = group.members.iter_mut().find(|m| m.guid == *guid) {
                if *grant {
                    m.flags |= GROUP_MEMBER_ASSISTANT;
                } else {
                    m.flags &= !GROUP_MEMBER_ASSISTANT;
                }
            }
            true
        }
        PartyRequest::ReadyCheckStart => {
            // The server's echo to the whole raid, us included. Only the leader has the button,
            // and the leader's echo takes the collection arm: no popup for the presser.
            group.apply_ready_check_request(true, &|_| None);
            true
        }
        PartyRequest::RequestRaidInfo => {
            // Answered at once with the seeded list, as the server answers a second ask; the
            // re-apply counts the answer that fires `UPDATE_INSTANCE_INFO`.
            let held = std::mem::take(&mut group.saved_instances);
            group.apply_raid_instance_info(held);
            true
        }
        _ => false,
    }
}

/// The sandbox's subgroup read: raid row `index`'s 0-based subgroup, ours from `own_flags` and
/// everyone else's from their roster entry, as the wire splits them.
fn test_subgroup_of(group: &GroupState, self_guid: Option<u64>, index: u32) -> Option<u8> {
    let guid = raid_guid_at(group, index)?;
    if Some(guid) == self_guid {
        return Some(group.own_flags & GROUP_MEMBER_SUBGROUP);
    }
    group
        .members
        .iter()
        .find(|m| m.guid == guid)
        .map(|m| m.flags & GROUP_MEMBER_SUBGROUP)
}

/// The sandbox's subgroup write: bits 0-2 only, so the assistant bit in that byte survives.
fn set_test_subgroup(group: &mut GroupState, self_guid: Option<u64>, index: u32, sub: u8) {
    let Some(guid) = raid_guid_at(group, index) else {
        return;
    };
    let put =
        |flags: &mut u8| *flags = (*flags & !GROUP_MEMBER_SUBGROUP) | (sub & GROUP_MEMBER_SUBGROUP);
    if Some(guid) == self_guid {
        put(&mut group.own_flags);
    } else if let Some(m) = group.members.iter_mut().find(|m| m.guid == guid) {
        put(&mut m.flags);
    }
}

/// A list for a sandbox, whose cache is not behind it: the lines name a member by the roster's
/// own name, the new list's or the one it replaces, and every name is taken as held. The stored
/// loot cells are the real session's (`0xbc7600`...), so the sandbox compares against them and
/// leaves them as it found them, and a real group after `/partytest off` is compared against what
/// the real lists last carried.
fn apply_sandbox_list(
    group: &mut GroupState,
    (group_type, own_flags): (u8, u8),
    members: Vec<GroupMemberEntry>,
    leader: u64,
    loot: GroupLootInfo,
    self_guid: Option<u64>,
) -> Vec<crate::ui_action::UiError> {
    let roster: Vec<(u64, String)> = group
        .members
        .iter()
        .chain(&members)
        .map(|m| (m.guid, m.name.clone()))
        .collect();
    let cached_name = |guid: u64| {
        roster
            .iter()
            .rev()
            .find(|(g, _)| *g == guid)
            .map(|(_, name)| name.clone())
    };
    let stored = group.loot_stored;
    let lines = group
        .apply_list_awaiting(
            group_type,
            own_flags,
            members,
            leader,
            Some(loot),
            self_guid,
            &|guid| Some(cached_name(guid).unwrap_or_default()),
            &cached_name,
        )
        .lines;
    group.loot_stored = stored;
    // Its members are no session's: their selection teardowns go nowhere.
    let _ = group.take_released();
    lines
}

/// The `/partytest` roster: four unstreamed synthetic members with mixed statuses and records,
/// through the real apply path, so every party frame state shows with no server. `player_xy` (our
/// live WoW position) places their minimap blips: Alice 30 yd out (dot), Bob 80 yd (dot, a rim
/// arrow one zoom in), Carol 300 yd (rim arrow), Dave offline (none).
pub(crate) fn synthetic_roster(
    group: &mut GroupState,
    player_xy: Option<(f32, f32)>,
) -> Vec<crate::ui_action::UiError> {
    let members = vec![
        GroupMemberEntry {
            name: "Alice".into(),
            guid: 0xF001,
            status: member_status::ONLINE,
            flags: 0,
        },
        GroupMemberEntry {
            name: "Bob".into(),
            guid: 0xF002,
            status: member_status::ONLINE | member_status::AFK,
            flags: 0,
        },
        GroupMemberEntry {
            name: "Carol".into(),
            guid: 0xF003,
            status: member_status::ONLINE | member_status::DEAD,
            flags: 0,
        },
        GroupMemberEntry {
            name: "Dave".into(),
            guid: 0xF004,
            status: member_status::OFFLINE,
            flags: 0,
        },
    ];
    let lines = apply_sandbox_list(
        group,
        (0, 0),
        members,
        0xF001,
        GroupLootInfo {
            method: 2,
            master: 0xF003,
            threshold: 3,
        },
        None,
    );
    // Blip offsets from us on WoW axes (+x north, +y west), truncated to `i16` as on the wire.
    let seat = |dx: f32, dy: f32| player_xy.map(|(px, py)| ((px + dx) as i16, (py + dy) as i16));
    for (guid, hp, max, level, power_type, pos) in [
        (0xF001u64, 820u16, 1240u16, 32u16, 0u8, seat(30.0, 0.0)),
        (0xF002, 455, 980, 30, 3, seat(0.0, 80.0)),
        (0xF003, 0, 1105, 31, 0, seat(-300.0, 0.0)),
        (0xF004, 0, 0, 0, 0, None),
    ] {
        // A server's full stats carry the status, which the record's online bit is read from.
        let status = group
            .members
            .iter()
            .find(|m| m.guid == guid)
            .map(|m| m.status);
        group.apply_stats(
            guid,
            true,
            PartyMemberStatsInfo {
                status,
                cur_hp: Some(hp),
                max_hp: Some(max),
                level: Some(level),
                power_type: Some(power_type),
                cur_power: Some(300),
                max_power: Some(410),
                position: pos,
                ..Default::default()
            },
        );
    }
    // Sandbox on, after `apply_list` cleared it: the drain now applies menu intents locally.
    group.test = true;
    lines
}

/// The `/partytest raid` roster: 24 synthetic members in subgroups 1 to 5 plus us, a raid we lead
/// with every class colour and a dead, an offline and an AFK member. Their classes go into the
/// name cache, where [`raid_row_at`] reads them; six lockouts fill the Raid Info panel.
pub(crate) fn synthetic_raid(
    group: &mut GroupState,
    names: &mut NameCache,
    self_guid: Option<u64>,
) -> Vec<crate::ui_action::UiError> {
    // (name, class id, race id) as `ChrClasses`/`ChrRaces` ids: eight classes, so every
    // `RAID_CLASS_COLORS` colour shows; the races are cosmetic.
    const ROSTER: [(&str, u8, u8); 24] = [
        ("Alaric", 1, 1),  // Warrior
        ("Brienne", 2, 1), // Paladin
        ("Cassian", 3, 3), // Hunter
        ("Dara", 4, 4),    // Rogue
        ("Elowen", 5, 1),  // Priest
        ("Fenwick", 7, 3), // Shaman, Horde-only in 1.12, here for its colour
        ("Gwendal", 8, 7), // Mage
        ("Halvard", 9, 1), // Warlock
        ("Isolde", 11, 4), // Druid
        ("Jorund", 1, 3),
        ("Kestrel", 4, 4),
        ("Lysa", 5, 4),
        ("Mordred", 9, 1),
        ("Nessa", 8, 7),
        ("Oswin", 2, 1),
        ("Perrin", 3, 4),
        ("Quilla", 11, 4),
        ("Roderick", 1, 1),
        ("Sable", 4, 4),
        ("Tarrin", 5, 3),
        ("Ulric", 2, 1),
        ("Vesper", 8, 7),
        ("Wystan", 9, 1),
        ("Yorick", 3, 3),
    ];
    let mut members = Vec::with_capacity(ROSTER.len());
    for (i, (name, class, race)) in ROSTER.iter().enumerate() {
        let guid = 0xF100 + i as u64;
        // Subgroups 1 to 5, five apiece, with our seat in group 1: `(i + 1) / 5` lands 4/5/5/5/5.
        let subgroup = ((i + 1) / 5) as u8 & GROUP_MEMBER_SUBGROUP;
        let mut status = member_status::ONLINE;
        let mut flags = subgroup;
        match i {
            // One assistant (the `(A)` token), one dead (red), one offline (grey), one AFK.
            0 => flags |= GROUP_MEMBER_ASSISTANT,
            3 => status |= member_status::DEAD,
            7 => status = member_status::OFFLINE,
            12 => status |= member_status::AFK,
            _ => {}
        }
        names.insert_player(guid, (*name).to_string(), Some((*race, *class, 0)));
        members.push(GroupMemberEntry {
            name: (*name).to_string(),
            guid,
            status,
            flags,
        });
    }
    // A raid we lead, so `IsRaidLeader()` is true and the leader-only controls are live.
    let leader = self_guid.unwrap_or(0);
    let lines = apply_sandbox_list(
        group,
        // Raid; our flags: subgroup 1 (0-based 0), no assistant bit.
        (1, 0),
        members,
        leader,
        GroupLootInfo {
            method: 2,
            master: leader,
            threshold: 3,
        },
        self_guid,
    );
    // Records for every member: fake guids never stream, so the merged view shows these.
    for (i, _) in ROSTER.iter().enumerate() {
        let guid = 0xF100 + i as u64;
        let dead = i == 3;
        let status = group
            .members
            .iter()
            .find(|m| m.guid == guid)
            .map(|m| m.status);
        group.apply_stats(
            guid,
            true,
            PartyMemberStatsInfo {
                status,
                cur_hp: Some(if dead {
                    0
                } else {
                    2100 + (i as u16 * 37) % 900
                }),
                max_hp: Some(3000),
                level: Some(58 + (i as u16 % 3)),
                power_type: Some(0),
                cur_power: Some(1200),
                max_power: Some(2400),
                ..Default::default()
            },
        );
    }
    // Six lockouts: the Raid Info panel shows four rows and scrolls from five
    // (`RaidFrame.lua:108`). Real map ids, so the rows carry `Map.dbc` names.
    group.apply_raid_instance_info(
        [
            (409, 3 * 86_400 + 7_200, 1234), // Molten Core
            (249, 14_400, 77),               // Onyxia's Lair
            (469, 5 * 86_400, 812),          // Blackwing Lair
            (309, 2_700, 3),                 // Zul'Gurub
            (509, 2 * 86_400 + 60, 640),     // Ruins of Ahn'Qiraj
            (533, 6 * 86_400, 91),           // Naxxramas
        ]
        .into_iter()
        .map(
            |(map, reset, instance)| benilla_protocol::messages::RaidInstanceEntry {
                map,
                reset,
                instance,
            },
        )
        .collect(),
    );
    // Sandbox on, after `apply_list` cleared it.
    group.test = true;
    lines
}

#[cfg(test)]
mod roster_update_tests;

#[cfg(test)]
mod tests {
    use super::super::pets::tests::{
        app, app_with, flag, frame, lent, member, members_changed_lent, party, stream, ME,
    };
    use super::*;
    use crate::net::GuidIndex;
    use crate::ui_party::party_record;
    use benilla_ui::script::UiScript;

    mod token_verb_tests;

    /// The record a member with no object reads (`0x496400`).
    fn record_state(record: &PartyMemberStatsInfo) -> benilla_ui::script::UnitState {
        let mut s = benilla_ui::script::UnitState::default();
        crate::ui_party::seat_member_record(&mut s, record);
        s
    }

    #[test]
    fn an_out_of_range_member_reads_the_record_and_divides_its_rage() {
        let record = PartyMemberStatsInfo {
            status: Some(member_status::ONLINE),
            cur_hp: Some(2400),
            max_hp: Some(3000),
            level: Some(41),
            power_type: Some(1), // POWER_RAGE
            cur_power: Some(570),
            max_power: Some(1000),
            ..PartyMemberStatsInfo::default()
        };
        let s = record_state(&record);
        assert_eq!(
            (s.health, s.max_health),
            (2400, 3000),
            "the bars keep their numbers"
        );
        assert_eq!(s.level, 41);
        assert_eq!(
            (s.power_type, s.power, s.max_power),
            (1, 57, 100),
            "rage reads 57/100, not 570/1000 — `UnitMana`'s record leg divides too (0x517744)"
        );
        assert!(s.is_player && s.is_connected);
    }

    /// A member out of view answers race, class and sex from the name cache record of its guid,
    /// read with no callback (`UnitRace` `0x5182bc`-`0x5182d9`, `+0x138`; `UnitClass` `0x518429`,
    /// `+0x140`; `UnitSex` `0x517f80`, `+0x13c` through `{2, 3, 1, 6}` at `0x808be4`); a miss
    /// leaves them unset, nil and `UnitSex` 2 (`0x517f95`).
    #[test]
    fn an_out_of_view_member_reads_race_class_and_sex_from_the_name_cache() {
        let tables = crate::ui_unit::TestNames::english();
        let mut names = NameCache::default();
        let state = |names: &NameCache| {
            crate::ui_unit::unheld_state(names, &GroupState::default(), 0x1234, tables.names())
        };
        let s = state(&names);
        assert_eq!((s.race, s.class, s.sex), (None, None, 0), "no record");
        // Night Elf (4) Priest (5), female (1).
        names.insert_player(0x1234, "Brisca".into(), Some((4, 5, 1)));
        let s = state(&names);
        assert_eq!(
            (s.race.as_deref(), s.race_file.as_deref()),
            (Some("Night Elf"), Some("NightElf"))
        );
        assert_eq!(
            (s.class.as_deref(), s.class_file.as_deref()),
            (Some("Priest"), Some("PRIEST"))
        );
        assert_eq!(s.sex, 3, "female");
    }

    /// The tooltip's group record (`0x496400`): the party's for a member of our own slots, else the
    /// raid roster's (`0x4bb0f0`) for one in another subgroup, and none for a guid off both.
    #[test]
    fn a_raid_member_off_our_party_slots_has_a_group_record() {
        let m = |n: u64, flags: u8| GroupMemberEntry {
            name: format!("M{n}"),
            guid: member(n),
            status: member_status::ONLINE,
            flags,
        };
        let mut group = GroupState::default();
        group.apply_list(
            GROUPTYPE_RAID,
            0,
            vec![m(1, 0), m(2, 1)],
            ME,
            None,
            Some(ME),
        );
        for n in [1, 2] {
            let mut record = PartyMemberStatsInfo::default();
            record.set_online(n == 1);
            group.stats.insert(member(n), record);
        }
        assert_eq!(
            group.party_slots().map(|e| e.guid).collect::<Vec<_>>(),
            [member(1)],
            "the second member sits in another subgroup"
        );
        let online = |guid| party_record(group.as_of_now(), guid, None).map(|r| r.online);
        assert_eq!(online(member(1)), Some(true), "the party's record");
        assert_eq!(online(member(2)), Some(false), "the raid roster's record");
        assert_eq!(online(member(3)), None, "off the roster");
    }

    /// vmangos re-sends a member's status on join, login and the AFK, DND, PvP, FFA and ghost
    /// toggles, never on a death, so the roster byte can say alive while the record says dead.
    #[test]
    fn an_out_of_range_members_dead_ghost_and_pvp_come_off_the_record() {
        let dead = PartyMemberStatsInfo {
            status: Some(member_status::ONLINE | member_status::DEAD),
            ..PartyMemberStatsInfo::default()
        };
        let s = record_state(&dead);
        assert!(
            s.dead,
            "the record says dead even though the roster echo does not"
        );
        assert!(!s.ghost);

        let ghost = PartyMemberStatsInfo {
            status: Some(member_status::ONLINE | member_status::GHOST),
            ..PartyMemberStatsInfo::default()
        };
        let s = record_state(&ghost);
        assert!(s.ghost);
        assert!(!s.dead, "a released ghost is not `dead` — only a ghost");

        // A list writes only bit 0 of the record (`0x4e836e`-`0x4e837b`): the record a list seats
        // carries none of the other bits, whatever the roster byte says.
        let s = record_state(&PartyMemberStatsInfo::placeholder(true));
        assert_eq!(
            (s.dead, s.ghost, s.pvp, s.is_pvp_ffa),
            (false, false, false, false)
        );

        // The record's PvP (`& 2`, `0x516502`) and free-for-all (`& 0x10`, `0x5165f1`) bits.
        let flagged = PartyMemberStatsInfo {
            status: Some(member_status::ONLINE | member_status::PVP | member_status::PVP_FFA),
            ..PartyMemberStatsInfo::default()
        };
        let s = record_state(&flagged);
        assert!(s.pvp && s.is_pvp_ffa);
    }

    /// The reported shape, through the feed: a roster that lags a held member's descriptor moves
    /// none of the four predicates, which read the object alone while it is held (`0x517b16`,
    /// `0x517bef`, `0x5164d1`, `0x51659f`), on `partyN` and `raidN` alike.
    #[test]
    fn a_held_members_dead_ghost_and_pvp_come_off_the_descriptor_alone() {
        const OBJECT_TYPE: u16 = 2;
        const HEALTH: u16 = 22;
        const MAXHEALTH: u16 = 28;
        const UNIT_FLAGS: u16 = 46;
        const PLAYER_FLAGS: u16 = 190;
        let lagging = member_status::ONLINE
            | member_status::DEAD
            | member_status::GHOST
            | member_status::PVP
            | member_status::PVP_FFA;
        let predicates = |app: &mut App, token: &str| {
            [
                "UnitIsDead",
                "UnitIsGhost",
                "UnitIsDeadOrGhost",
                "UnitIsPVP",
                "UnitIsPVPFreeForAll",
            ]
            .map(|p| flag(app, &format!("{p}('{token}')")))
        };
        for group_type in [0, GROUPTYPE_RAID] {
            let token = if group_type == 0 { "party1" } else { "raid1" };
            let mut app = app();
            party(&mut app, 1, true);
            let held = stream(&mut app, member(1), &[(HEALTH, 500), (MAXHEALTH, 900)]);
            // Resurrected and unflagged in front of us; the roster still says otherwise.
            seat(&mut app, group_type, lagging);
            frame(&mut app);
            assert_eq!(predicates(&mut app, token), [None; 5], "{token}");

            // Dead, a ghost, PvP and free-for-all on the descriptor; the roster says none. A
            // player carries `UNIT_FIELD_FLAGS` 0x8, which free-for-all asks before the player
            // that controls it, here the member itself (`0x5165ad`, `0x606170`).
            app.world_mut().entity_mut(held).insert(ObjectStore(
                benilla_protocol::messages::ObjectFields::from_pairs(&[
                    (OBJECT_TYPE, 0x19),
                    (HEALTH, 0),
                    (MAXHEALTH, 900),
                    (UNIT_FLAGS, 0x1000 | 0x8),
                    (PLAYER_FLAGS, 0x10 | 0x80),
                ]),
            ));
            seat(&mut app, group_type, member_status::ONLINE);
            frame(&mut app);
            assert_eq!(predicates(&mut app, token), [Some(1.0); 5], "{token}");
        }
    }

    /// `GetGuildInfo` (`0x4c9330`) resolves any token (`0x4c9382`) and reads the object's guild
    /// from the cache (`0x4c93d2`), so a held member answers through `partyN` and `raidN` as the
    /// player does, from the frame the record lands.
    #[test]
    fn a_held_members_guild_answers_through_its_party_and_raid_tokens() {
        /// `PLAYER_GUILDID` and `PLAYER_GUILDRANK`.
        const GUILD_ID: u16 = 191;
        const GUILD_RANK: u16 = 192;
        for group_type in [0, GROUPTYPE_RAID] {
            let token = if group_type == 0 { "party1" } else { "raid1" };
            let mut app = app();
            party(&mut app, 1, true);
            stream(&mut app, member(1), &[(GUILD_ID, 7), (GUILD_RANK, 2)]);
            seat(&mut app, group_type, member_status::ONLINE);
            frame(&mut app);
            let info = |app: &mut App| -> (Option<String>, i64) {
                let (name, _, rank): (Option<String>, Option<String>, i64) =
                    lent(app, &format!("GetGuildInfo({token:?})"));
                (name, rank)
            };
            assert_eq!(info(&mut app), (None, 0), "{token}: not cached");
            app.world_mut()
                .resource_mut::<crate::ui_guild::GuildState>()
                .land_for_test(7, "Probe Guild");
            frame(&mut app);
            assert_eq!(
                info(&mut app),
                (Some("Probe Guild".to_string()), 2),
                "{token}"
            );
        }
    }

    /// Send member 1 alone as the party or raid roster with `status`, as a server's list does: the
    /// member's record, which [`party`] seated, takes its online bit from the list.
    fn seat(app: &mut App, group_type: u8, status: u8) {
        let mut group = app.world_mut().resource_mut::<GroupState>();
        group.apply_list(
            group_type,
            0,
            vec![GroupMemberEntry {
                name: "M1".into(),
                guid: member(1),
                status,
                flags: 0,
            }],
            ME,
            None,
            Some(ME),
        );
    }

    /// `UnitIsConnected` (`0x517d50`) answers 1 for any unit the object manager holds
    /// (`0x517daf`), whatever the roster's status byte says; the record is not read.
    #[test]
    fn a_held_member_reads_connected_whatever_the_roster_status_says() {
        let mut app = app();
        party(&mut app, 1, false);
        stream(&mut app, member(1), &[]);
        frame(&mut app);
        assert_eq!(
            flag(&mut app, "UnitIsConnected('party1')"),
            Some(1.0),
            "held, roster and record without the online bit"
        );

        seat(&mut app, GROUPTYPE_RAID, member_status::OFFLINE);
        frame(&mut app);
        assert_eq!(
            flag(&mut app, "UnitIsConnected('raid1')"),
            Some(1.0),
            "the raid token reads the same view"
        );
    }

    /// The reported shape: a member goes link-dead while still in view. The roster marks them
    /// offline and the reference keeps them connected until the object leaves (`0x517daf`), then
    /// reads the record's online bit (`0x517dd3`).
    #[test]
    fn a_held_member_going_offline_reads_disconnected_only_once_the_object_leaves() {
        let mut app = app();
        party(&mut app, 1, true);
        let held = stream(&mut app, member(1), &[]);
        frame(&mut app);
        assert_eq!(flag(&mut app, "UnitIsConnected('party1')"), Some(1.0));

        seat(&mut app, 0, member_status::OFFLINE);
        frame(&mut app);
        assert_eq!(
            flag(&mut app, "UnitIsConnected('party1')"),
            Some(1.0),
            "link-dead, still in view"
        );

        app.world_mut().despawn(held);
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .remove(&member(1));
        frame(&mut app);
        assert_eq!(
            flag(&mut app, "UnitIsConnected('party1')"),
            None,
            "out of view, the record's clear online bit decides"
        );
    }

    /// With no object, the record's online bit (`+0x08 & 1`) is the answer: clear is nil
    /// (`0x517dfd`).
    #[test]
    fn an_unheld_member_with_the_online_bit_clear_reads_nil() {
        let mut app = app();
        party(&mut app, 1, false);
        frame(&mut app);
        assert_eq!(flag(&mut app, "UnitIsConnected('party1')"), None);

        seat(&mut app, GROUPTYPE_RAID, member_status::OFFLINE);
        frame(&mut app);
        assert_eq!(flag(&mut app, "UnitIsConnected('raid1')"), None);
    }

    /// Set, it is 1 (`0x517dd7`).
    #[test]
    fn an_unheld_member_with_the_online_bit_set_reads_connected() {
        let mut app = app();
        party(&mut app, 1, true);
        frame(&mut app);
        assert_eq!(flag(&mut app, "UnitIsConnected('party1')"), Some(1.0));

        seat(&mut app, GROUPTYPE_RAID, member_status::ONLINE);
        frame(&mut app);
        assert_eq!(flag(&mut app, "UnitIsConnected('raid1')"), Some(1.0));
    }

    /// The answer is the record's bit, not the roster byte the list also carries: a stats packet's
    /// status moves the record alone (`0x5e54b7`-`0x5e54d8`), and `UnitIsConnected` reads the record
    /// (`0x517dca`-`0x517dd3`), so the two can disagree between lists.
    #[test]
    fn an_unheld_members_connected_follows_the_record_when_only_a_stats_packet_moves_it() {
        let mut app = app();
        party(&mut app, 1, true);
        frame(&mut app);
        assert_eq!(flag(&mut app, "UnitIsConnected('party1')"), Some(1.0));

        let status = |app: &mut App, status: u8| {
            app.world_mut().resource_mut::<GroupState>().apply_stats(
                member(1),
                false,
                PartyMemberStatsInfo {
                    status: Some(status),
                    ..Default::default()
                },
            );
            frame(app);
        };
        status(&mut app, member_status::OFFLINE);
        assert_eq!(
            flag(&mut app, "UnitIsConnected('party1')"),
            None,
            "the record went offline, the roster byte still says online"
        );
        status(&mut app, member_status::ONLINE);
        assert_eq!(flag(&mut app, "UnitIsConnected('party1')"), Some(1.0));

        // The list writes the same bit, so the next one agrees with the byte it carries.
        seat(&mut app, 0, member_status::OFFLINE);
        frame(&mut app);
        assert_eq!(flag(&mut app, "UnitIsConnected('party1')"), None);
        status(&mut app, member_status::ONLINE);
        assert_eq!(
            flag(&mut app, "UnitIsConnected('party1')"),
            Some(1.0),
            "the record online again, the roster byte still offline"
        );

        seat(&mut app, GROUPTYPE_RAID, member_status::ONLINE);
        frame(&mut app);
        status(&mut app, member_status::OFFLINE);
        assert_eq!(
            flag(&mut app, "UnitIsConnected('raid1')"),
            None,
            "the raid row"
        );
    }

    /// A frame that counts the `PARTY_MEMBERS_CHANGED`s the VM is told of; [`changed`] takes them.
    fn watch_members_changed(app: &mut App) {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        script
            .run(
                r#"
                CHANGED = 0
                local f = CreateFrame("Frame")
                f:RegisterEvent("PARTY_MEMBERS_CHANGED")
                f:SetScript("OnEvent", function() CHANGED = CHANGED + 1 end)
                "#,
            )
            .unwrap();
    }

    /// Run a frame and count the `PARTY_MEMBERS_CHANGED`s it fired.
    fn changed(app: &mut App) -> u32 {
        frame(app);
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let fired: f64 = script.eval("return CHANGED").unwrap();
        script.run("CHANGED = 0").unwrap();
        fired as u32
    }

    /// `SMSG_GROUP_LIST` as the server re-sends it: party members `1..` with their status byte,
    /// under `leader` and `loot`, and nothing else told to the client.
    fn resend(app: &mut App, statuses: &[u8], leader: u64, loot: Option<GroupLootInfo>) {
        let list = statuses
            .iter()
            .enumerate()
            .map(|(i, &status)| GroupMemberEntry {
                name: format!("M{}", i + 1),
                guid: member(i as u64 + 1),
                status,
                flags: 0,
            })
            .collect();
        app.world_mut()
            .resource_mut::<GroupState>()
            .apply_list(0, 0, list, leader, loot, Some(ME));
    }

    const ON: u8 = member_status::ONLINE;
    const OFF: u8 = member_status::OFFLINE;

    /// The reported shape: a member out of view logs off, and the server sends the list again with
    /// their status alone moved. The reference signals after every list (`0x5e6c61`), so the party
    /// frame rereads them.
    #[test]
    fn a_list_that_moves_only_a_members_status_fires_party_members_changed() {
        let mut app = app();
        watch_members_changed(&mut app);
        party(&mut app, 2, true);
        assert_eq!(changed(&mut app), 1, "the first look answers the list");
        assert_eq!(changed(&mut app), 0, "a steady frame is silent");

        resend(&mut app, &[OFF, ON], ME, None);
        assert_eq!(changed(&mut app), 1, "member 1 went offline");
        resend(&mut app, &[OFF, ON | member_status::AFK], ME, None);
        assert_eq!(changed(&mut app), 1, "member 2 went away");
        assert_eq!(changed(&mut app), 0);
    }

    /// The leader and the loot method ride the same list, and neither moves a guid.
    #[test]
    fn a_list_that_moves_only_the_leader_or_the_loot_method_fires_party_members_changed() {
        let mut app = app();
        watch_members_changed(&mut app);
        party(&mut app, 2, true);
        changed(&mut app);

        resend(&mut app, &[ON, ON], member(1), None);
        assert_eq!(changed(&mut app), 1, "a new leader");
        let master = GroupLootInfo {
            method: 2,
            master: member(1),
            threshold: 3,
        };
        resend(&mut app, &[ON, ON], member(1), Some(master));
        assert_eq!(changed(&mut app), 1, "a new loot method");
        let raised = GroupLootInfo {
            threshold: 4,
            ..master
        };
        resend(&mut app, &[ON, ON], member(1), Some(raised));
        assert_eq!(changed(&mut app), 1, "a new threshold");
    }

    /// The handler has no compare against the roster it holds (`0x5e6a40`): a list that says what
    /// the last one said is signalled all the same.
    #[test]
    fn an_identical_list_fires_party_members_changed_again() {
        let mut app = app();
        watch_members_changed(&mut app);
        party(&mut app, 2, true);
        changed(&mut app);

        resend(&mut app, &[ON, ON], ME, None);
        assert_eq!(changed(&mut app), 1);
        resend(&mut app, &[ON, ON], ME, None);
        assert_eq!(changed(&mut app), 1);
    }

    /// Each packet signals, so the lists one drain applies fire once apiece, after the snapshots
    /// the last of them left.
    #[test]
    fn each_list_of_one_drain_fires_its_own_party_members_changed() {
        let mut app = app();
        watch_members_changed(&mut app);
        party(&mut app, 2, true);
        changed(&mut app);

        resend(&mut app, &[OFF, ON], ME, None);
        resend(&mut app, &[OFF, OFF], ME, None);
        resend(&mut app, &[ON, ON], ME, None);
        assert_eq!(changed(&mut app), 3);
        assert_eq!(flag(&mut app, "UnitIsConnected('party1')"), Some(1.0));
    }

    /// An empty list jumps straight to the signal (`0x5e6b3c`), the all-zero "you left" list and a
    /// solo leader's alike, and a second one to a group already empty is not skipped.
    #[test]
    fn an_empty_list_fires_party_members_changed() {
        let mut app = app();
        watch_members_changed(&mut app);
        party(&mut app, 1, true);
        changed(&mut app);

        app.world_mut()
            .resource_mut::<GroupState>()
            .apply_list(0, 0, vec![], 0, None, Some(ME));
        assert_eq!(changed(&mut app), 1, "the leave list");
        app.world_mut()
            .resource_mut::<GroupState>()
            .apply_list(0, 0, vec![], 0, None, Some(ME));
        assert_eq!(
            changed(&mut app),
            1,
            "and again, with nobody left to change"
        );
        app.world_mut()
            .resource_mut::<GroupState>()
            .apply_list(0, 0, vec![], ME, None, Some(ME));
        assert_eq!(changed(&mut app), 1, "a leader with no one yet");
    }

    /// A VM up while the lists land hears each at its place, and a VM seeded at its birth after
    /// some hears none of those, only what is signalled after it ([`seed_vm_party`]); its getters
    /// read the group at the call. A player who never had a group hears nothing.
    #[test]
    fn a_vm_hears_each_list_signalled_after_its_birth() {
        let mut app = app();
        watch_members_changed(&mut app);
        party(&mut app, 2, true);
        resend(&mut app, &[OFF, ON], ME, None);
        resend(&mut app, &[OFF, OFF], ME, None);
        assert_eq!(changed(&mut app), 3, "each list");
        assert_eq!(changed(&mut app), 0);

        resend(&mut app, &[ON, ON], ME, None);
        let mut born = UiScript::new().unwrap();
        seed_vm_party(app.world_mut(), &mut born, false);
        born.run("SEEN = {}").unwrap();
        app.insert_non_send_resource(born);
        watch_members_changed(&mut app);
        assert_eq!(changed(&mut app), 0, "a list before the birth");
        assert_eq!(
            lent::<f64>(&mut app, "GetNumPartyMembers()"),
            2.0,
            "read at the call"
        );
        resend(&mut app, &[ON, OFF], ME, None);
        assert_eq!(changed(&mut app), 1, "a list after it");

        let mut solo = self::app();
        watch_members_changed(&mut solo);
        assert_eq!(changed(&mut solo), 0, "no list, no event");
    }

    /// The party lane and the drain in the schedule's order, with a system between the lane and
    /// the drain that logs a signal, as the chat input's `/partytest` can: the drain's
    /// age keeps it, the next frame's lane fires it once, and the ages after drop it.
    #[test]
    fn a_signal_logged_between_the_lane_and_the_drain_fires_on_the_next_frame_once() {
        #[derive(Resource, Default)]
        struct Mark(bool);
        fn mark(mut on: ResMut<Mark>, mut group: ResMut<GroupState>) {
            if std::mem::take(&mut on.0) {
                group.apply_raid_target_update(7, member(1));
            }
        }
        let mut app = app();
        app.init_resource::<crate::net::Reputations>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<Mark>();
        app.add_systems(
            Update,
            (mark.after(super::fire_party_lane), drain_party.after(mark)),
        );
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        script
            .run(
                r#"
                MARKS = 0
                local f = CreateFrame("Frame")
                f:RegisterEvent("RAID_TARGET_UPDATE")
                f:SetScript("OnEvent", function() MARKS = MARKS + 1 end)
                "#,
            )
            .unwrap();
        let marks = |app: &mut App| {
            frame(app);
            let script = app.world_mut().non_send_resource_mut::<UiScript>();
            let n: f64 = script.eval("return MARKS").unwrap();
            script.run("MARKS = 0").unwrap();
            n as u32
        };
        marks(&mut app);

        app.world_mut().resource_mut::<Mark>().0 = true;
        assert_eq!(marks(&mut app), 0, "logged after this frame's lane");
        assert_eq!(marks(&mut app), 1, "the next frame's lane");
        marks(&mut app);
        assert_eq!(marks(&mut app), 0, "once");
        let log = &app.world().resource::<GroupState>().signals;
        assert_eq!(log.since(0), [], "and the ages dropped it");
    }

    /// The sandbox's echo signals as a real list does: one naming the leader signals the leader
    /// store whether or not it moved (`0x4e8250`), one with members the loot store (`0x4e87fa`),
    /// and the all-zero echo of a leave neither.
    #[test]
    fn the_sandbox_echo_signals_both_stores_on_every_list() {
        use GroupSignal::*;
        let mut g = GroupState::default();
        synthetic_roster(&mut g, None);
        g.signals.take_events();
        let same = PartyRequest::LootThreshold(3);
        assert!(test_apply_local(&mut g, &same, Some(ME)));
        assert_eq!(
            g.signals.take_events(),
            [
                MembersChanged,
                LeaderChanged,
                LootMethodChanged,
                RaidTargetUpdate
            ],
            "a threshold it already had, under the leader it already had"
        );
        assert!(test_apply_local(&mut g, &PartyRequest::Leave, Some(ME)));
        assert_eq!(g.signals.take_events(), [MembersChanged, RaidTargetUpdate]);
    }

    /// The sandbox stands in for the server, whose echo of each group intent is a list.
    #[test]
    fn the_sandbox_answers_a_group_intent_with_a_party_members_changed() {
        let mut app = app();
        watch_members_changed(&mut app);
        synthetic_roster(&mut app.world_mut().resource_mut::<GroupState>(), None);
        assert_eq!(changed(&mut app), 1);

        let me = Some(ME);
        let intents = [
            PartyRequest::UninviteGuid(0xF002),
            PartyRequest::PromoteGuid(0xF001),
            PartyRequest::LootThreshold(4),
            PartyRequest::Leave,
        ];
        for req in intents {
            let mut group = app.world_mut().resource_mut::<GroupState>();
            assert!(test_apply_local(&mut group, &req, me));
            assert_eq!(changed(&mut app), 1, "{req:?}");
        }
        let mut group = app.world_mut().resource_mut::<GroupState>();
        echo_raid_target(&mut group, 0, ME);
        assert_eq!(changed(&mut app), 0, "a mark is no list");
    }

    /// The stock party frame, loaded whole off the player's own chain: a member out of view logs
    /// off, and the server sends the list again with their status alone moved. The stock frame
    /// repaints on the event that list signals: `UnitFrameManaBar_Update` greys the bar for a
    /// member who is not connected, and `PartyMemberFrame_UpdatePet` hides their pet frame
    /// (`PartyMemberFrame.lua:73`).
    #[test]
    fn the_stock_party_frame_greys_a_member_who_logs_off_out_of_view() {
        benilla_formats::wow_data_or_skip!();
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        let failures = crate::ui_script::load_default_ui(&script);
        assert!(failures.is_empty(), "load failures: {failures:?}");
        let mut app = app_with(script);
        let bar = |app: &mut App| {
            app.world_mut()
                .non_send_resource::<UiScript>()
                .eval::<(f64, f64, f64)>("return PartyMemberFrame1ManaBar:GetStatusBarColor()")
                .unwrap()
        };

        party(&mut app, 1, true);
        frame(&mut app);
        members_changed_lent(&mut app);
        assert_eq!(
            bar(&mut app),
            (0.0, 0.0, benilla_ui::color::quant::read_via_f32(1.0)),
            "the mana colour"
        );
        assert_eq!(
            flag(&mut app, "PartyMemberFrame1PetFrame:IsShown()"),
            Some(1.0)
        );

        // The list alone, as the server sends it: the member's record is not touched.
        resend(&mut app, &[OFF], ME, None);
        frame(&mut app);
        members_changed_lent(&mut app);
        assert_eq!(flag(&mut app, "UnitIsConnected('party1')"), None);
        let grey = benilla_ui::color::quant::read(0.5);
        let grey32 = benilla_ui::color::quant::read_via_f32(0.5);
        assert_eq!(
            bar(&mut app),
            (grey, grey32, grey32),
            "the disconnected grey"
        );
        assert_eq!(flag(&mut app, "PartyMemberFrame1PetFrame:IsShown()"), None);

        resend(&mut app, &[ON], ME, None);
        frame(&mut app);
        members_changed_lent(&mut app);
        assert_eq!(
            bar(&mut app),
            (0.0, 0.0, benilla_ui::color::quant::read_via_f32(1.0)),
            "back online"
        );
        assert_eq!(
            flag(&mut app, "PartyMemberFrame1PetFrame:IsShown()"),
            Some(1.0)
        );
        let errors = app.world().non_send_resource::<UiScript>().errors();
        assert!(errors.is_empty(), "script errors: {errors:?}");
    }

    /// Every `SMSG_RAID_INSTANCE_INFO` signals `UPDATE_INSTANCE_INFO` at its packet, the second
    /// empty answer too: the Raid Info button is decided on it (`RaidFrame.lua:43`), and a diff over
    /// the list alone can never reach it. Each listener reads the list its own answer carried.
    #[test]
    fn every_instance_answer_signals_its_event_with_its_own_list() {
        let mut app = app();
        party(&mut app, 1, true);
        frame(&mut app);
        app.world_mut()
            .non_send_resource::<UiScript>()
            .run(
                r#"HEARD = {}
                   local f = CreateFrame("Frame")
                   f:RegisterEvent("UPDATE_INSTANCE_INFO")
                   f:SetScript("OnEvent", function() table.insert(HEARD, GetNumSavedInstances()) end)"#,
            )
            .unwrap();
        let lockout = benilla_protocol::messages::RaidInstanceEntry {
            map: 409,
            reset: 86_400,
            instance: 7,
        };
        {
            let mut group = app.world_mut().resource_mut::<GroupState>();
            group.apply_raid_instance_info(Vec::new());
            group.apply_raid_instance_info(Vec::new());
            group.apply_raid_instance_info(vec![lockout]);
        }
        frame(&mut app);
        let heard: Vec<f64> = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval("return HEARD")
            .unwrap();
        assert_eq!(heard, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn partytest_sandbox_applies_menu_intents_locally() {
        let mut group = GroupState::default();
        synthetic_roster(&mut group, None);
        assert!(group.test, "the synthetic roster arms the sandbox");
        let me = Some(0x5E1Fu64);

        // Skull on the target, then Cross: one mark per unit.
        let mark = |group: &mut GroupState, i| match raid_target_mark(group, 0xB0B0, i, || true) {
            RaidTargetMark::Send { icon, guid } => echo_raid_target(group, icon, guid),
            other => panic!("index {i}: {other:?}"),
        };
        mark(&mut group, 8);
        assert_eq!(group.raid_target_index(0xB0B0), 8);
        mark(&mut group, 7);
        assert_eq!(group.raid_target_index(0xB0B0), 7, "the old icon clears");
        assert_eq!(group.raid_targets[7], 0);
        mark(&mut group, 0);
        assert_eq!(group.raid_target_index(0xB0B0), 0, "NONE clears");

        // Promote party1 (Alice), kick party2 (Bob), retune the loot.
        test_apply_local(&mut group, &PartyRequest::PromoteGuid(0xF001), me);
        assert_eq!(group.leader, 0xF001);
        test_apply_local(&mut group, &PartyRequest::UninviteGuid(0xF002), me);
        assert!(!group.members.iter().any(|m| m.guid == 0xF002));
        assert!(!group.stats.contains_key(&0xF002));
        test_apply_local(
            &mut group,
            &PartyRequest::LootMethod {
                method: 4,
                master: 0,
                threshold: 2,
            },
            me,
        );
        test_apply_local(&mut group, &PartyRequest::LootThreshold(4), me);
        assert_eq!(
            group.loot,
            Some(GroupLootInfo {
                method: 4,
                master: 0,
                threshold: 4
            })
        );

        // Invites stay real; Leave disbands the sandbox; a real list turns the flag off.
        assert!(!test_apply_local(
            &mut group,
            &PartyRequest::InviteName("Zed".into()),
            me
        ));
        test_apply_local(&mut group, &PartyRequest::Leave, me);
        assert!(!group.in_group && !group.test);
        synthetic_roster(&mut group, None);
        group.apply_list(0, 0, vec![], 0x123, None, None);
        assert!(!group.test, "the real wire always wins");
    }

    /// The roster status bytes here say the opposite of each record, so every column shows which
    /// one `0x4bb560` reads: a held row its object, an unheld one its record, never the byte.
    /// Every stored raid row as [`raid_row_at`] reads it, `GetRaidRosterInfo`'s whole array.
    fn raid_roster<'a>(
        group: &GroupState,
        me: Option<&RaidSelf>,
        held: &dyn Fn(u64) -> Option<&'a ObjectStore>,
        names: &NameCache,
        unit_names: crate::ui_unit::UnitNames<'_>,
        zone_name: &dyn Fn(u32) -> Option<String>,
    ) -> Vec<RaidMemberInfo> {
        let reads = RowReads {
            cells: group.cells(),
            records: &group.stats,
            me,
            held,
            names,
            unit_names,
            zone_name,
        };
        (0..group.raid.len())
            .map_while(|index| raid_row_at(&reads, index))
            .collect()
    }

    #[test]
    fn the_raid_roster_maps_the_wire_to_get_raid_roster_info() {
        const CAROL: u64 = 0xCA201;
        let mut names = NameCache::default();
        // Our own name has no traits, as live: the login seeds it without them.
        names.insert_player(0x5E1F, "Me".into(), None);
        names.insert_player(0xA11CE, "Alice".into(), Some((1, 1, 1))); // Human WARRIOR
        names.insert_player(0xB0B, "Bob".into(), None); // no traits yet

        let member = |guid: u64, name: &str, status: u8, flags: u8| GroupMemberEntry {
            name: name.into(),
            guid,
            status,
            flags,
        };
        let mut group = GroupState {
            group_type: GROUPTYPE_RAID,
            own_flags: 0,
            leader: 0x5E1F,
            members: vec![
                // Alice: subgroup 2 (stored), an assistant; the byte says offline.
                member(
                    0xA11CE,
                    "Alice",
                    member_status::OFFLINE,
                    2 | GROUP_MEMBER_ASSISTANT,
                ),
                // Bob: subgroup 0; the byte says online and alive.
                member(0xB0B, "Bob", member_status::ONLINE, 0),
                // Carol: held; the byte says offline.
                member(CAROL, "Carol", member_status::OFFLINE, 1),
            ],
            ..Default::default()
        };
        group.rebuild_raid(0x5E1F);
        let record = |status: u8, level: u16| PartyMemberStatsInfo {
            status: Some(status),
            level: Some(level),
            zone: Some(1537),
            ..Default::default()
        };
        // Alice's record: online and dead.
        group.stats.insert(
            0xA11CE,
            record(member_status::ONLINE | member_status::DEAD, 58),
        );
        // Bob's: offline and dead.
        group.stats.insert(0xB0B, record(member_status::DEAD, 40));
        // Carol's: offline, but her object is held, at 0 health.
        group
            .stats
            .insert(CAROL, record(member_status::OFFLINE, 12));
        let carol = ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[
            (22, 0),   // UNIT_FIELD_HEALTH
            (28, 900), // UNIT_FIELD_MAXHEALTH
            (34, 33),  // UNIT_FIELD_LEVEL
        ]));
        let held = |guid: u64| (guid == CAROL).then_some(&carol);
        let zone = |id: u32| match id {
            1537 => Some("Ironforge".to_string()),
            1519 => Some("Stormwind City".to_string()),
            _ => None,
        };
        let me = RaidSelf {
            guid: 0x5E1F,
            level: 60,
            // We stand in Stormwind, the member records in Ironforge.
            area: Some(1519),
            dead: false,
            // Off our descriptor, the only source of it for us. 11 is Druid.
            class: Some(11),
        };

        let korean = crate::ui_unit::TestNames::korean();
        let roster = raid_roster(&group, Some(&me), &held, &names, korean.names(), &zone);
        assert_eq!(roster.len(), 4, "the player is spliced back in");

        // Row 1: an assistant, whose record is online and dead, the pair the reference's
        // no-object arm tests (`0x4bb70a`-`0x4bb717`), with its level and zone (`0x4bb66e`,
        // `0x4bb740`).
        assert_eq!(roster[0].name, "Alice");
        assert_eq!((roster[0].rank, roster[0].subgroup), (1, 2));
        assert_eq!(roster[0].class_file.as_deref(), Some("WARRIOR"));
        assert!(roster[0].online && roster[0].ninth, "online AND dead");
        assert_eq!(roster[0].level, 58);
        assert_eq!(roster[0].zone.as_deref(), Some("Ironforge"));

        // Row 2: the record is offline, so level 0 (`0x4bb680`), the binding's offline zone, and
        // return 9 nil though dead; an unresolved class is nil.
        assert!(!roster[1].online, "offline");
        assert_eq!(roster[1].level, 0, "an offline record's level is not read");
        assert!(!roster[1].ninth, "0x4 without 0x1 is not the arm");
        assert_eq!(roster[1].class, None);

        // Row 3: held, so online whatever the record (`0x4bb777`), the descriptor's level
        // (`0x4bb65d`), the zone under us (`0x4bb739`) and dead at 0 health (`0x4bb796`).
        assert!(roster[2].online);
        assert_eq!(roster[2].level, 33);
        assert_eq!(roster[2].zone.as_deref(), Some("Stormwind City"));
        assert!(roster[2].ninth);

        // Row 4, us, after the members (`0x5e6e73`): leader (rank 2), subgroup stored 0, class off
        // the descriptor.
        assert_eq!(roster[3].name, "Me");
        assert_eq!(roster[3].rank, 2);
        assert_eq!(
            roster[3].subgroup, 0,
            "stored 0-based; the BINDING adds one"
        );
        assert_eq!(roster[3].level, 60);
        assert_eq!(roster[3].class_file.as_deref(), Some("DRUID"));
        assert_eq!(
            roster[3].class.as_deref(),
            Some("드루이드"),
            "the display name is the table's, the token is not localized"
        );
        assert_eq!(roster[3].zone.as_deref(), Some("Stormwind City"));
        assert!(roster[3].online && !roster[3].ninth);

        // With no object of ours, as across a worldport, our row stays where it was, online as the
        // list wrote it: the roster's guids do not move.
        let away = raid_roster(&group, None, &held, &names, korean.names(), &zone);
        assert_eq!(
            away.iter().map(|r| (r.guid, r.online)).collect::<Vec<_>>(),
            roster
                .iter()
                .map(|r| (r.guid, r.online))
                .collect::<Vec<_>>()
        );
        assert_eq!((away[3].name.as_str(), away[3].level), ("Me", 0));

        // No roster held, as after a party list (`0x4ba550`): empty, so `GetNumRaidMembers()`
        // answers 0, while `IsRaidLeader()` still answers 1.
        group.raid.clear();
        assert!(raid_roster(&group, Some(&me), &held, &names, korean.names(), &zone).is_empty());
    }

    #[test]
    fn a_raid_row_index_names_the_same_player_the_roster_array_does() {
        let mut group = GroupState {
            group_type: GROUPTYPE_RAID,
            ..Default::default()
        };
        let wire = |name: &str, guid: u64| GroupMemberEntry {
            name: name.into(),
            guid,
            status: member_status::ONLINE,
            flags: 0,
        };
        group.members = vec![wire("Alice", 0xA11CE), wire("Bob", 0xB0B)];
        group.rebuild_raid(0x5E1F);

        assert_eq!(raid_row_guids(&group), vec![0xA11CE, 0xB0B, 0x5E1F]);
        assert_eq!(
            raid_guid_at(&group, 1),
            Some(0xA11CE),
            "row 1 is the first listed"
        );
        assert_eq!(raid_guid_at(&group, 3), Some(0x5E1F), "the last row is us");
        for miss in [0, 4, 500] {
            assert_eq!(
                raid_guid_at(&group, miss),
                None,
                "index {miss} names nobody"
            );
        }

        // Each row's subgroup is its flags under 0x7f, ours our own, the row after the members.
        group.members[1].flags = 0x82;
        group.own_flags = 3;
        group.rebuild_raid(0x5E1F);
        let subgroups: Vec<Option<u8>> = (0..=4).map(|i| raid_row_subgroup(&group, i)).collect();
        assert_eq!(subgroups, [None, Some(0), Some(2), Some(3), None]);

        // With no roster held all are empty, so a raid verb in a party sends nothing.
        group.raid.clear();
        assert!(raid_row_guids(&group).is_empty());
        assert_eq!(raid_guid_at(&group, 1), None);
        assert_eq!(raid_row_subgroup(&group, 1), None);
    }

    #[test]
    fn the_sandbox_moves_and_swaps_subgroups_locally() {
        let mut group = GroupState::default();
        let me = Some(0x5E1Fu64);
        let mut names = NameCache::default();
        names.insert_player(0x5E1F, "Kel".into(), None);
        synthetic_raid(&mut group, &mut names, me);
        assert!(group.test, "the synthetic raid arms the sandbox");
        assert_eq!(group.group_type, GROUPTYPE_RAID);

        // Row 1 is the first wire member, and `synthetic_raid` gives it the assistant bit; we are
        // the last row.
        let us = group.members.len() as u32 + 1;
        assert_eq!(test_subgroup_of(&group, me, 1), Some(0));
        assert!(group.members[0].flags & GROUP_MEMBER_ASSISTANT != 0);

        // Move it to subgroup 8 in Lua, 7 on the wire.
        assert!(test_apply_local(
            &mut group,
            &PartyRequest::SetSubgroup { index: 1, group: 8 },
            me
        ));
        assert_eq!(test_subgroup_of(&group, me, 1), Some(7));
        assert!(
            group.members[0].flags & GROUP_MEMBER_ASSISTANT != 0,
            "the assistant bit rides the same byte and must survive the move"
        );

        // Swap it with our row, whose subgroup lives in `own_flags`.
        let mine = test_subgroup_of(&group, me, us).unwrap();
        assert!(test_apply_local(
            &mut group,
            &PartyRequest::SwapSubgroup {
                index: us,
                other: 1
            },
            me
        ));
        assert_eq!(test_subgroup_of(&group, me, us), Some(7));
        assert_eq!(test_subgroup_of(&group, me, 1), Some(mine));

        assert!(test_apply_local(
            &mut group,
            &PartyRequest::SetSubgroup { index: 1, group: 9 },
            me
        ));
        assert_eq!(
            test_subgroup_of(&group, me, 1),
            Some(mine),
            "9 is not a subgroup"
        );

        // Echoed to us as leader, our ready check arms the engine's side, with no popup's event.
        group.signals.take();
        assert!(test_apply_local(
            &mut group,
            &PartyRequest::ReadyCheckStart,
            me
        ));
        assert_eq!(
            group.signals.take(),
            [GroupSignal::ReadyCheckRequest { we_lead: true }]
        );

        let n = group.members.len();
        assert!(test_apply_local(
            &mut group,
            &PartyRequest::UninviteRaid(us),
            me
        ));
        assert_eq!(
            group.members.len(),
            n,
            "the last row is us; the server would refuse too"
        );
        assert!(test_apply_local(
            &mut group,
            &PartyRequest::UninviteRaid(3),
            me
        ));
        assert_eq!(group.members.len(), n - 1);
    }

    /// Hold our own player: the feed reads our guid off the `SelfPlayer` entity, and the object
    /// manager finds a player object by its type word (`0x468460`, mask `0x10`).
    fn seat_self(app: &mut App) -> Entity {
        let e = app
            .world_mut()
            .spawn((Guid(ME), player_object(), SelfPlayer))
            .id();
        app.world_mut().resource_mut::<GuidIndex>().0.insert(ME, e);
        e
    }

    /// `OBJECT_FIELD_TYPE` of a player: object, unit and player bits.
    const PLAYER_TYPE: [(u16, u32); 1] = [(2, 0x19)];

    fn player_object() -> ObjectStore {
        ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(
            &PLAYER_TYPE,
        ))
    }

    fn listed(n: u64, subgroup: u8) -> GroupMemberEntry {
        GroupMemberEntry {
            name: format!("M{n}"),
            guid: member(n),
            status: member_status::ONLINE,
            flags: subgroup,
        }
    }

    /// `IsRaidLeader` (`0x4bb8c0`) and `IsPartyLeader` (`0x4e9130`) compare the leader cell with our
    /// guid, whichever subgroup either of us sits in: a leader in another subgroup holds none of our
    /// party slots and leads nobody here, and one alone in a subgroup of their own, with no slot
    /// held at all, leads.
    #[test]
    fn the_leader_tests_follow_the_guid_through_a_raid_whatever_the_subgroups() {
        let mut app = app();
        seat_self(&mut app);
        let raid = |app: &mut App, members: Vec<GroupMemberEntry>, leader: u64| {
            app.world_mut().resource_mut::<GroupState>().apply_list(
                GROUPTYPE_RAID,
                0,
                members,
                leader,
                None,
                Some(ME),
            );
            frame(app);
        };

        // We sit in subgroup 0 with M2; M1 leads from subgroup 1.
        raid(&mut app, vec![listed(1, 1), listed(2, 0)], member(1));
        assert_eq!(flag(&mut app, "GetPartyMember(1)"), Some(1.0), "M2 is ours");
        assert_eq!(flag(&mut app, "IsRaidLeader()"), None);
        assert_eq!(flag(&mut app, "IsPartyLeader()"), None);
        assert_eq!(flag(&mut app, "GetPartyLeaderIndex()"), Some(0.0));

        // We lead, alone in subgroup 0.
        raid(&mut app, vec![listed(1, 1), listed(2, 1)], ME);
        assert_eq!(
            flag(&mut app, "GetPartyMember(1)"),
            None,
            "nobody else is ours"
        );
        assert_eq!(flag(&mut app, "IsRaidLeader()"), Some(1.0));
        assert_eq!(flag(&mut app, "IsPartyLeader()"), Some(1.0));
    }

    /// `GetPartyLeaderIndex` pushes the leader-index cell plus one (`0x4e9104`-`0x4e9109`). The
    /// session starts it at -1 (`0x4e7c9f`); every list and every ending zeroes it (`0x5e6b19`,
    /// `0x5e6855`, `0x5e6898`, `0x5e68fb` into `0x4e85b5`) before the leader store, which returns
    /// at once for a zero leader (`0x4e8267`) and otherwise stores -1 (`0x4e827a`), then the slot
    /// holding the leader (`0x4e82b8`).
    #[test]
    fn get_party_leader_index_answers_the_cell_plus_one() {
        let mut app = app();
        seat_self(&mut app);
        let list = |app: &mut App, members: Vec<GroupMemberEntry>, leader: u64| {
            app.world_mut().resource_mut::<GroupState>().apply_list(
                0,
                0,
                members,
                leader,
                None,
                Some(ME),
            );
            frame(app);
        };
        let index = |app: &mut App| flag(app, "GetPartyLeaderIndex()");
        frame(&mut app);
        assert_eq!(index(&mut app), Some(0.0), "the session's start");

        list(&mut app, vec![listed(1, 0), listed(2, 0)], member(2));
        assert_eq!(index(&mut app), Some(2.0), "party2 leads");
        list(&mut app, vec![listed(1, 0), listed(2, 0)], ME);
        assert_eq!(index(&mut app), Some(0.0), "we lead");
        list(&mut app, Vec::new(), 0);
        assert_eq!(index(&mut app), Some(1.0), "the all-zero list");

        type Ending = fn(&mut GroupState);
        let endings: [Ending; 3] = [
            |g| {
                let _ = g.apply_uninvited();
            },
            |g| {
                let _ = g.apply_destroyed();
            },
            |g| {
                let _ = g.apply_command_result(
                    benilla_protocol::messages::party_operation::LEAVE,
                    "Us",
                    benilla_protocol::messages::party_result::OK,
                );
            },
        ];
        for (i, end) in endings.into_iter().enumerate() {
            list(&mut app, vec![listed(1, 0)], member(1));
            assert_eq!(index(&mut app), Some(1.0), "ending {i}: party1 leads");
            end(&mut app.world_mut().resource_mut::<GroupState>());
            frame(&mut app);
            assert_eq!(index(&mut app), Some(1.0), "ending {i}");
            list(&mut app, vec![listed(1, 0)], ME);
            assert_eq!(index(&mut app), Some(0.0), "ending {i}: we lead again");
        }

        app.world_mut()
            .resource_mut::<GroupState>()
            .apply_uninvited();
        app.world_mut().resource_mut::<GroupState>().clear_session();
        frame(&mut app);
        assert_eq!(index(&mut app), Some(0.0), "a new session");
    }

    /// `GetLootMethod` (`0x4e91b0`) and `GetLootThreshold` (`0x4e94e0`) read the stored cells
    /// (`0xbc7600`, `0xbc7608`, `0xbc7610`) with no group test: zero before the first list with
    /// members, kept by every list without and by the end of the group. The method is named for 0
    /// to 4 through `0x4e9280`, `ERROR!` above (`0x4e91ef`); the looter's index needs a nonzero
    /// cell alone (`0x4e9209`): 0 for us (`0x4e9215`), a party slot plus one (`0x4e9238`), else nil.
    #[test]
    fn the_loot_getters_read_the_stored_cells_with_no_group() {
        let mut app = app();
        seat_self(&mut app);
        let list = |app: &mut App, members: Vec<GroupMemberEntry>, tail: Option<(u8, u64, u8)>| {
            let loot = tail.map(|(method, master, threshold)| GroupLootInfo {
                method,
                master,
                threshold,
            });
            let leader = if members.is_empty() { 0 } else { ME };
            app.world_mut().resource_mut::<GroupState>().apply_list(
                0,
                0,
                members,
                leader,
                loot,
                Some(ME),
            );
            frame(app);
        };
        let loot = |app: &mut App| -> (String, Option<f64>, f64) {
            let (method, master): (String, Option<f64>) = lent(app, "GetLootMethod()");
            (method, master, lent(app, "GetLootThreshold()"))
        };
        frame(&mut app);
        assert_eq!(
            loot(&mut app),
            ("freeforall".into(), None, 0.0),
            "no list yet"
        );

        let party = || vec![listed(1, 0), listed(2, 0)];
        list(&mut app, party(), Some((2, member(2), 3)));
        assert_eq!(loot(&mut app), ("master".into(), Some(2.0), 3.0));
        list(&mut app, Vec::new(), None);
        assert_eq!(
            loot(&mut app),
            ("master".into(), None, 3.0),
            "the all-zero list keeps the cells; the looter holds no slot now"
        );
        list(&mut app, party(), Some((3, member(1), 2)));
        assert_eq!(
            loot(&mut app),
            ("group".into(), Some(1.0), 2.0),
            "a looter under group loot"
        );
        list(&mut app, party(), Some((1, ME, 2)));
        assert_eq!(loot(&mut app), ("roundrobin".into(), Some(0.0), 2.0), "us");
        list(&mut app, party(), Some((5, 0, 4)));
        assert_eq!(loot(&mut app), ("ERROR!".into(), None, 4.0), "above 4");
        app.world_mut().resource_mut::<GroupState>().clear_session();
        frame(&mut app);
        assert_eq!(
            loot(&mut app),
            ("ERROR!".into(), None, 4.0),
            "a new session"
        );
    }

    /// The drain after the party lane, and the commands it sends.
    fn drain_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let mut app = app();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.init_resource::<crate::net::Reputations>()
            .init_resource::<crate::ui_action::UiErrorKeys>();
        app.add_systems(Update, drain_party.after(crate::ui_party::fire_party_lane));
        (app, rx)
    }

    /// Run `chunk` with a host lent that applies the party calls inside their verbs, as the app's
    /// host does ([`super::super::calls::PartyCalls`]), and answers the group getters at the call
    /// ([`super::super::RosterLive`]); other calls go nowhere.
    pub(super) fn hosted_run(app: &mut App, chunk: &str) {
        use bevy::ecs::system::SystemState;
        let world = app.world_mut();
        let mut script = world.remove_non_send_resource::<UiScript>().expect("a VM");
        let mut state =
            SystemState::<(super::super::calls::PartyCalls, super::super::RosterLive)>::new(world);
        let ran = {
            let (mut calls, roster) = state.get_mut(world);
            let mut host = crate::script_calls::FnHostReading(
                |s: &mut UiScript, call| {
                    if let benilla_ui::script::ScriptCall::Party(request) = call {
                        calls.apply(s, request);
                    }
                },
                roster,
            );
            script.hosted(&mut host, |s| s.run(chunk))
        };
        state.apply(world);
        world.insert_non_send_resource(script);
        ran.expect("the chunk runs");
    }

    /// In the `/partytest` sandbox a group-changing call sends nothing: the drain answers it on
    /// the mirror as the server's list would, and an accept still goes out, as a real group ends
    /// the sandbox.
    #[test]
    fn the_sandbox_echoes_a_group_call_and_sends_an_accept() {
        let (mut app, rx) = drain_app();
        seat_self(&mut app);
        resolve_party(&mut app, [0; 4]);
        let mut group = app.world_mut().resource_mut::<GroupState>();
        let _ = synthetic_roster(&mut group, None);
        // `/partytest lead`: we lead, so `ConvertToRaid` passes its gate (`0x4bbca5`).
        group.leader = ME;
        frame(&mut app);
        let _ = rx.try_iter().count();
        hosted_run(&mut app, "ConvertToRaid() AcceptGroup()");
        let sent: Vec<bool> = rx
            .try_iter()
            .map(|c| matches!(c, ClientCommand::GroupAccept))
            .collect();
        assert_eq!(sent, [true], "the accept alone");
        assert_eq!(app.world().resource::<GroupState>().group_type, 0);
        frame(&mut app);
        assert_eq!(
            app.world().resource::<GroupState>().group_type,
            GROUPTYPE_RAID,
            "the drain's echo"
        );
    }

    /// The kicks among the commands sent, `name:<name>` or `guid:<guid>`, in order.
    fn kicks(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<String> {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::GroupUninvite { name } => Some(format!("name:{name}")),
                ClientCommand::GroupUninviteGuid { guid } => Some(format!("guid:{guid:#x}")),
                _ => None,
            })
            .collect()
    }

    /// The token resolver's stand-in: us, and the party slots.
    fn resolve_party(app: &mut App, party: [u64; 4]) {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_unit_guids(&benilla_ui::script::UnitGuids {
                player: ME,
                party,
                ..Default::default()
            });
    }

    fn cache_names(app: &mut App, who: &[(u64, &str)]) {
        let mut names = app.world_mut().resource_mut::<NameCache>();
        for (guid, name) in who {
            names.insert_player(*guid, (*name).into(), None);
        }
    }

    /// `UninviteFromParty` (`0x48a510`) and `UninviteFromRaid` (`0x48a580`) share one sender
    /// (`0x5e9510`): it asks the object manager for the guid's player object (`0x5e951f`, mask
    /// `0x10`), sends `CMSG_GROUP_UNINVITE` with the unit's name when it is held (`0x5e95a7`-
    /// `0x5e95d0`) and `CMSG_GROUP_UNINVITE_GUID` with the guid when it is not (`0x5e953e`).
    #[test]
    fn a_party_kick_goes_by_name_for_a_held_member_and_by_guid_for_the_rest() {
        let (mut app, rx) = drain_app();
        party(&mut app, 2, true);
        resolve_party(&mut app, [member(1), member(2), 0, 0]);
        cache_names(&mut app, &[(member(1), "M1"), (member(2), "M2")]);
        stream(&mut app, member(2), &PLAYER_TYPE);
        frame(&mut app);
        let _ = kicks(&rx);

        hosted_run(
            &mut app,
            r#"UninviteFromParty("party1") UninviteFromParty("party2")"#,
        );
        frame(&mut app);
        assert_eq!(kicks(&rx), ["guid:0x1001", "name:M2"]);
    }

    /// The raid row's kick takes the same sender: a row out of range goes by guid, one held by
    /// name, our own row too (our object is held).
    #[test]
    fn a_raid_kick_goes_by_name_for_a_held_row_and_by_guid_for_the_rest() {
        let (mut app, rx) = drain_app();
        seat_self(&mut app);
        cache_names(
            &mut app,
            &[(ME, "Me"), (member(1), "M1"), (member(2), "M2")],
        );
        stream(&mut app, member(2), &PLAYER_TYPE);
        app.world_mut().resource_mut::<GroupState>().apply_list(
            GROUPTYPE_RAID,
            0,
            vec![listed(1, 0), listed(2, 1)],
            ME,
            None,
            Some(ME),
        );
        frame(&mut app);
        let _ = kicks(&rx);

        hosted_run(
            &mut app,
            "UninviteFromRaid(1) UninviteFromRaid(2) UninviteFromRaid(3)",
        );
        frame(&mut app);
        assert_eq!(kicks(&rx), ["guid:0x1001", "name:M2", "name:Me"]);
    }

    /// A held member's name is `GetUnitName`'s (`0x609210`, from `0x5e95aa`): the cache's, else the
    /// text of `UNKNOWNOBJECT` (`0x609324`-`0x60932e`).
    #[test]
    fn a_held_member_the_cache_has_no_name_for_is_kicked_as_unknown_object() {
        let (mut app, rx) = drain_app();
        party(&mut app, 1, true);
        // The list's name lookup unanswered.
        *app.world_mut().resource_mut::<NameCache>() = NameCache::default();
        resolve_party(&mut app, [member(1), 0, 0, 0]);
        stream(&mut app, member(1), &PLAYER_TYPE);
        let script = app.world_mut().non_send_resource::<UiScript>();
        script.run(r#"UNKNOWNOBJECT = "Unknown Entity""#).unwrap();
        frame(&mut app);
        let _ = kicks(&rx);

        hosted_run(&mut app, r#"UninviteFromParty("party1")"#);
        frame(&mut app);
        assert_eq!(kicks(&rx), ["name:Unknown Entity"]);
    }

    /// `SetRaidSubgroup` (`0x4bb990`) sends nothing for a row already in that subgroup (`0x4bba3c`,
    /// the row's `+8`, its flags under 0x7f) or whose name the cache lacks, looked up with no
    /// callback (`0x4bba5e`-`0x4bba68`); `SwapRaidSubgroup` (`0x4bbb00`) nothing for two rows of one
    /// subgroup (`0x4bbba5`) or for either name missing (`0x4bbbec`-`0x4bbbf7`). The names sent are
    /// the cache's records (`0x4bba9c`, `0x4bbc2b`, `0x4bbc34`).
    #[test]
    fn the_subgroup_verbs_send_only_a_move_with_every_name_cached() {
        let (mut app, rx) = drain_app();
        seat_self(&mut app);
        // Row 1 in subgroup 2 under a cached spelling of its own, row 2 in subgroup 3 with no
        // name cached, row 3 us in subgroup 1.
        cache_names(&mut app, &[(ME, "Me"), (member(1), "Cached")]);
        app.world_mut().resource_mut::<GroupState>().apply_list(
            GROUPTYPE_RAID,
            0,
            vec![listed(1, 1), listed(2, 2)],
            ME,
            None,
            Some(ME),
        );
        frame(&mut app);
        let _ = rx.try_iter().count();

        hosted_run(
            &mut app,
            "SetRaidSubgroup(1, 2) SetRaidSubgroup(2, 1) SetRaidSubgroup(3, 1) \
             SetRaidSubgroup(1, 3) SwapRaidSubgroup(1, 1) SwapRaidSubgroup(1, 2) \
             SwapRaidSubgroup(2, 3) SwapRaidSubgroup(1, 3)",
        );
        frame(&mut app);
        let sent: Vec<String> = rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::GroupChangeSubGroup { name, group } => {
                    Some(format!("move:{name}:{group}"))
                }
                ClientCommand::GroupSwapSubGroup { name, other } => {
                    Some(format!("swap:{name}:{other}"))
                }
                _ => None,
            })
            .collect();
        assert_eq!(sent, ["move:Cached:2", "swap:Cached:Me"]);
    }

    /// `UninviteFromParty` (`0x48a510`) and `PromoteToPartyLeader` (`0x48a7c0`) turn any token into
    /// a guid through `GetGUIDFromName` (`0x48a54d`, `0x48a7fd`, into `0x515970`) and give a nonzero
    /// one to the kick sender (`0x5e9510`) or the leader sender (`0x5e9640`), with no party test:
    /// the target, a raid row and a party slot alike.
    #[test]
    fn the_token_verbs_kick_and_promote_whatever_unit_the_token_names() {
        let (mut app, rx) = drain_app();
        seat_self(&mut app);
        app.world_mut().resource_mut::<GroupState>().apply_list(
            GROUPTYPE_RAID,
            0,
            vec![listed(1, 0), listed(2, 1)],
            ME,
            None,
            Some(ME),
        );
        frame(&mut app);
        let _ = rx.try_iter().count();
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.set_unit_guids(&benilla_ui::script::UnitGuids {
            player: ME,
            target: member(2),
            party: [member(1), 0, 0, 0],
            raid: vec![member(1), member(2), ME],
            ..Default::default()
        });
        hosted_run(
            &mut app,
            r#"UninviteFromParty("target") PromoteToPartyLeader("raid2")
               UninviteFromParty("raid1") PromoteToPartyLeader("party1")
               UninviteFromParty("party2") PromoteToPartyLeader("mouseover")"#,
        );
        frame(&mut app);
        let sent: Vec<String> = rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::GroupUninvite { name } => Some(format!("kick:{name}")),
                ClientCommand::GroupUninviteGuid { guid } => Some(format!("kick:{guid:#x}")),
                ClientCommand::GroupSetLeader { guid } => Some(format!("lead:{guid:#x}")),
                _ => None,
            })
            .collect();
        assert_eq!(
            sent,
            ["kick:0x1002", "lead:0x1002", "kick:0x1001", "lead:0x1001"]
        );
    }

    /// The open form arms the 30 s deadline (`0x4ba535`) before it signals `READY_CHECK`
    /// (`0x4ba53a`), so a handler that answers at once finds the check running
    /// (`GetReadyCheckMsLeft 0x4bb2f0`, read at `0x4bc099`) and its answer goes out.
    #[test]
    fn a_ready_check_handler_answers_inside_the_check() {
        let mut app = app();
        party(&mut app, 2, true);
        frame(&mut app);
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.keep_calls();
        script
            .run(
                r#"local f = CreateFrame("Frame")
                   f:RegisterEvent("READY_CHECK")
                   f:SetScript("OnEvent", function() ConfirmReadyCheck(1) end)"#,
            )
            .unwrap();
        let _ = app
            .world_mut()
            .resource_mut::<GroupState>()
            .apply_ready_check_request(false, &|_| None);
        frame(&mut app);
        assert_eq!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .take_party_requests(),
            [PartyRequest::ReadyCheckAnswer(1)]
        );
    }

    /// `SMSG_GROUP_INVITE`'s handler signals `PARTY_INVITE_REQUEST` with the inviter (`0x5e6747`
    /// → `0x48f420`, event `0x118`) for every invite; `PARTY_INVITE_CANCEL` is
    /// `SMSG_GROUP_CANCEL`'s alone (`0x5e6784` → `0x48f440`), and `AcceptGroup` and
    /// `DeclineGroup` only send (`0x48aa80`, `0x48aab0`). So an answer signals nothing, and the
    /// same inviter asking again is asked about again.
    /// `SMSG_GROUP_INVITE`'s handler (`0x5e6730`) signals `PARTY_INVITE_REQUEST` (`0x5e6747`), then
    /// shows `ERR_INVITED_TO_GROUP_S` (message `0x3c`, `0x5e6752`): one pass, the event first.
    #[test]
    fn an_invite_fires_its_request_then_shows_its_line() {
        let (mut app, _rx) = drain_app();
        resolve_party(&mut app, [0; 4]);
        app.world_mut()
            .non_send_resource::<UiScript>()
            .run(
                r#"ERR_INVITED_TO_GROUP_S = "%s has invited you to join a group."
                   HEARD = {}
                   local f = CreateFrame("Frame")
                   for _, e in ipairs({ "PARTY_INVITE_REQUEST", "CHAT_MSG_SYSTEM",
                                        "UI_INFO_MESSAGE", "UI_ERROR_MESSAGE" }) do
                       f:RegisterEvent(e)
                   end
                   f:SetScript("OnEvent", function()
                       table.insert(HEARD, event .. ":" .. tostring(arg1))
                   end)"#,
            )
            .unwrap();
        frame(&mut app);
        app.world_mut()
            .resource_mut::<GroupState>()
            .apply_invited("Bob");
        frame(&mut app);
        let heard: Vec<String> = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval("return HEARD")
            .unwrap();
        assert_eq!(
            heard,
            [
                "PARTY_INVITE_REQUEST:Bob",
                "CHAT_MSG_SYSTEM:Bob has invited you to join a group."
            ]
        );
    }

    #[test]
    fn every_invite_signals_its_request_and_an_answer_signals_nothing() {
        let (mut app, _rx) = drain_app();
        resolve_party(&mut app, [0; 4]);
        app.world_mut()
            .non_send_resource::<UiScript>()
            .run(
                r#"INVITES = {}
                   local f = CreateFrame("Frame")
                   f:RegisterEvent("PARTY_INVITE_REQUEST")
                   f:RegisterEvent("PARTY_INVITE_CANCEL")
                   f:SetScript("OnEvent", function()
                       table.insert(INVITES, event .. ":" .. tostring(arg1))
                   end)"#,
            )
            .unwrap();
        let invites = |app: &mut App| -> Vec<String> {
            app.world_mut()
                .non_send_resource::<UiScript>()
                .eval("local i = INVITES; INVITES = {}; return i")
                .unwrap()
        };
        let invite = |app: &mut App| {
            app.world_mut()
                .resource_mut::<GroupState>()
                .apply_invited("Bob");
            frame(app);
        };
        // The VM's first look, which owes it nothing.
        frame(&mut app);
        invite(&mut app);
        assert_eq!(invites(&mut app), ["PARTY_INVITE_REQUEST:Bob"]);
        hosted_run(&mut app, "AcceptGroup()");
        frame(&mut app);
        frame(&mut app);
        assert_eq!(invites(&mut app), Vec::<String>::new(), "the accept");
        invite(&mut app);
        assert_eq!(
            invites(&mut app),
            ["PARTY_INVITE_REQUEST:Bob"],
            "asked again"
        );
        invite(&mut app);
        assert_eq!(
            invites(&mut app),
            ["PARTY_INVITE_REQUEST:Bob"],
            "unanswered, asked again"
        );
    }

    /// The group events a VM heard, in order, once [`hear_group_events`] is installed.
    fn heard(app: &mut App) -> Vec<String> {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("local h = HEARD; HEARD = {}; return h")
            .unwrap()
    }

    /// Note the five group events in `HEARD`, with the `SEEN` the harness's frame reads.
    fn hear_group_events(script: &UiScript) {
        script
            .run(
                r#"
                SEEN = {}
                HEARD = {}
                local f = CreateFrame("Frame")
                for _, e in ipairs({ "PARTY_MEMBERS_CHANGED", "PARTY_LEADER_CHANGED",
                                     "PARTY_LOOT_METHOD_CHANGED", "RAID_ROSTER_UPDATE",
                                     "RAID_TARGET_UPDATE" }) do
                    f:RegisterEvent(e)
                end
                f:SetScript("OnEvent", function() table.insert(HEARD, event) end)
                "#,
            )
            .unwrap();
    }

    /// A `/reload` signals none of the group events in 1.12.1: their sites are the group packets'
    /// handlers (`ReadGroupList 0x5e6a40`, `SetGroupLeader 0x4e8250`, `ClearParty 0x4e84a0`, the
    /// raid target handler `0x4ba220`), the name callback `0x4bad90` and the world entry's fill of
    /// a zero raid row (`0x4ba140`, `0x4ba15c`), and the stock frames read the group as they load
    /// (`PartyMemberFrame.lua:20-24`, `RaidFrame.lua:32`). So the reload's VM is seeded past the
    /// group's signals before its files run ([`seed_vm_party`]), its files read the group at the
    /// call, and a list that lands later signals as ever.
    #[test]
    fn a_reloaded_vm_reads_the_group_as_it_loads_and_hears_no_group_event() {
        let mut app = app();
        party(&mut app, 2, true);
        frame(&mut app);
        // A list after this frame's lane, which the reload loads over.
        resend(&mut app, &[ON, ON], ME, None);

        let mut reloaded = UiScript::new().unwrap();
        seed_vm_party(app.world_mut(), &mut reloaded, true);
        // The files run with the host lent, as the load's phases do.
        app.insert_non_send_resource(reloaded);
        crate::ui_unit::live::with_lent_objects(app.world_mut(), |reloaded| {
            reloaded
                .run(r#"LOADED = GetNumPartyMembers() .. "," .. tostring(UnitName("party2"))"#)
                .unwrap();
            hear_group_events(reloaded);
        });
        frame(&mut app);
        let loaded: String = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval("return LOADED")
            .unwrap();
        assert_eq!(loaded, "2,M2", "the group is there for the files");
        assert_eq!(heard(&mut app), Vec::<String>::new(), "the reload");

        party(&mut app, 2, true);
        frame(&mut app);
        assert_eq!(
            heard(&mut app),
            [
                "PARTY_MEMBERS_CHANGED",
                "PARTY_LEADER_CHANGED",
                "RAID_TARGET_UPDATE"
            ],
            "a list after it"
        );
    }

    /// The stock party frames, loaded into a reload's seeded VM, show the members at once.
    #[test]
    fn a_reloaded_vm_has_its_party_frames_filled_as_it_loads() {
        benilla_formats::wow_data_or_skip!();
        let mut app = app();
        party(&mut app, 1, true);
        frame(&mut app);

        let mut script = UiScript::new().unwrap();
        seed_vm_party(app.world_mut(), &mut script, true);
        script.set_screen_size(1024.0, 768.0);
        // The files load with the host lent, as the load's phases do.
        app.insert_non_send_resource(script);
        let failures = crate::ui_unit::live::with_lent_objects(app.world_mut(), |script| {
            let failures = crate::ui_script::load_default_ui(script);
            script.run("SEEN = {}").unwrap();
            failures
        });
        assert!(failures.is_empty(), "load failures: {failures:?}");
        assert_eq!(flag(&mut app, "PartyMemberFrame1:IsShown()"), Some(1.0));
        assert_eq!(
            app.world_mut()
                .non_send_resource::<UiScript>()
                .eval::<Option<String>>("return PartyMemberFrame1Name:GetText()")
                .unwrap()
                .as_deref(),
            Some("M1")
        );
    }

    /// The stored loot cells (`0xbc7600`...) are the real session's: `/partytest` compares against
    /// them and leaves them as found, so `/partytest off` and a real group after it are measured
    /// against what the real lists last carried, never the sandbox's tail.
    #[test]
    fn the_sandbox_rosters_leave_the_stored_loot_cells_as_they_found_them() {
        let real = GroupLootInfo {
            method: 4,
            master: 0,
            threshold: 2,
        };
        let mut group = GroupState {
            loot_stored: real,
            ..GroupState::default()
        };
        let lines = synthetic_roster(&mut group, None);
        assert!(
            lines.iter().any(|l| l.key == "ERR_SET_LOOT_MASTER"),
            "the sandbox still says what its tail changes"
        );
        assert_eq!(group.loot_stored, real);

        let mut names = NameCache::default();
        synthetic_raid(&mut group, &mut names, Some(ME));
        assert_eq!(group.loot_stored, real);

        group.clear_session();
        assert_eq!(group.loot_stored, real, "`/partytest off`");
    }
}
