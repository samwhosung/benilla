//! The party feed and drain: push the roster and the `party1..4` and `raid1..40` unit snapshots
//! into the VM, fire the party events on their edges, and turn the Lua [`PartyRequest`] intents
//! into `CMSG_*` sends. The state and its lines live in the parent module.

use benilla_protocol::messages::{
    member_status, GroupLootInfo, GroupMemberEntry, PartyMemberStatsInfo, GROUP_MEMBER_ASSISTANT,
};
use benilla_ui::script::{
    PartyMemberInfo, PartyRequest, PartyState, RaidMemberInfo, SavedInstanceInfo, ScriptValue,
    UiScript, UnitState,
};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{
    ClientCommand, FieldChanged, FieldEdges, Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer,
};
use crate::target::Selection;
use crate::ui_script::gate;

use super::GroupState;

// ─── The VM feed and drain ───────────────────────────────────────────────────────────────────────

/// What the feed last pushed: the values its event edges fire on.
#[derive(Default)]
pub(super) struct FedParty {
    roster: Vec<u64>,
    leader: u64,
    loot: Option<GroupLootInfo>,
    invite: Option<String>,
    units: [Option<UnitState>; 4],
    /// Each party slot's (member guid, object streamed) pair, the `PARTY_MEMBER_ENABLE`/`DISABLE`
    /// edge; with the guid in it a new occupant is a roster edge, not an activation.
    presence: [(u64, bool); 4],
    pushed_party: Option<PartyState>,
    /// The gate's counters: the name cache's landed generation (other feeds' per-frame resolves
    /// defeat its `is_changed`) and the area under us (our raid row's zone).
    names_generation: gate::Watch,
    area: gate::Watch,
    raid_units: Vec<Option<UnitState>>,
    /// The raid roster's identity (guid, rank, subgroup, online): `RAID_ROSTER_UPDATE`'s edge.
    raid_key: Vec<(u64, u32, u32, bool)>,
    saved: Vec<SavedInstanceInfo>,
    saved_answers: u32,
    ready_check: u32,
    ready_check_requests: u32,
    answers_forwarded: usize,
    raid_targets: [u64; 8],
}

pub(crate) const PARTY_TOKENS: [&str; 4] = ["party1", "party2", "party3", "party4"];

/// A new list or a new answer: `RaidFrame.lua:43-52` decides the Raid Info button on the second
/// `UPDATE_INSTANCE_INFO`, which an unchanging empty list reaches only through the answer count.
fn saved_instances_moved(saved: &[SavedInstanceInfo], answers: u32, fed: &FedParty) -> bool {
    saved != fed.saved || answers != fed.saved_answers
}

/// The `raid1..raid40` unit tokens, one per `MAX_RAID_MEMBERS` (40, `RaidFrame.lua:2`).
#[rustfmt::skip]
pub(crate) const RAID_TOKENS: [&str; 40] = [
    "raid1", "raid2", "raid3", "raid4", "raid5", "raid6", "raid7", "raid8", "raid9", "raid10",
    "raid11", "raid12", "raid13", "raid14", "raid15", "raid16", "raid17", "raid18", "raid19",
    "raid20", "raid21", "raid22", "raid23", "raid24", "raid25", "raid26", "raid27", "raid28",
    "raid29", "raid30", "raid31", "raid32", "raid33", "raid34", "raid35", "raid36", "raid37",
    "raid38", "raid39", "raid40",
];

/// `SMSG_GROUP_LIST`'s first byte for a raid; 0 is a party (vmangos `Group/Group.h:119`).
pub(crate) const GROUPTYPE_RAID: u8 = 1;

/// The subgroup bits (0-2) of a member's flags byte; `0x80` is the assistant bit.
pub(crate) const GROUP_MEMBER_SUBGROUP: u8 = 0x07;

/// Push the roster and the party unit snapshots into the VM and fire the party events on their
/// edges. A member's unit state is their live descriptor when streamed, else their roster record.
pub(super) fn feed_party(
    // `ChrClasses.dbc` field 16, `UnitHasRelicSlot`'s input; without it no class has a relic slot.
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,
    script: Option<NonSendMut<UiScript>>,
    group: Res<GroupState>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    changed_stores: Query<(), Changed<ObjectStore>>,
    mut removed_stores: RemovedComponents<ObjectStore>,
    mut edges: MessageReader<FieldChanged>,
    self_q: Query<(Entity, &Guid, &ObjectStore), With<SelfPlayer>>,
    factions: Option<Res<crate::target::Factions>>,
    names: Res<NameCache>,
    areas: Option<Res<crate::area::AreaTableRes>>,
    // The area under us, through the accessor `crate::area` uses: `terrain_stream::CurrentArea`
    // would cross the world-API wall (`tests/world_api_wall.rs`).
    here: benilla_world::world_point::WorldPoint,
    map_catalog: Option<Res<benilla_assets::MapCatalogRes>>,
    mut fed: Local<crate::ui_script::VmMemo<FedParty>>,
    mut chat: ResMut<crate::ui_chat::ChatLog>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (fed, vm_reset) = fed.get_reset(&script);
    let edges = FieldEdges::collect(&mut edges);
    // The ready-check summary lines the timeout tick composed, pushed as system chat ahead of the
    // gate: the tick runs on the frame clock, which the gate does not watch.
    for line in script.take_ready_check_lines() {
        chat.push_event(crate::ui_chat::ChatEvent::text_only(
            crate::ui_chat::ChatEventKind::System,
            line,
        ));
    }
    let chr = classes.as_deref().map(|t| &t.0);
    // The gate also opens on a despawn, which `Changed` misses. Solo, it almost always stays shut.
    let names_moved = fed.names_generation.moved(names.generation());
    let area_moved = fed.area.moved(here.area().map_or(u64::MAX, u64::from));
    let group_changed = group.is_changed();
    let index_changed = index.is_changed();
    // Only the stores the merged view reads: a crowd's other stores change every frame.
    let stores_changed = self_q
        .iter()
        .next()
        .is_some_and(|(e, _, _)| changed_stores.get(e).is_ok())
        || group.members.iter().any(|m| {
            index
                .0
                .get(&m.guid)
                .is_some_and(|&e| changed_stores.get(e).is_ok())
        });
    let stores_removed = !removed_stores.is_empty();
    let factions_changed = factions.as_ref().is_some_and(|r| r.is_changed());
    let areas_changed = areas.as_ref().is_some_and(|r| r.is_changed());
    let maps_changed = map_catalog.as_ref().is_some_and(|r| r.is_changed());
    gate::trace(
        "feed_party",
        &[
            ("vm_reset", vm_reset),
            ("names", names_moved),
            ("area", area_moved),
            ("group", group_changed),
            ("index", index_changed),
            ("stores", stores_changed),
            ("removed", stores_removed),
            ("factions", factions_changed),
            ("areas", areas_changed),
            ("maps", maps_changed),
        ],
    );
    let gate = gate::Gate::new(
        vm_reset
            || names_moved
            || area_moved
            || group_changed
            || index_changed
            || stores_changed
            || stores_removed
            || factions_changed
            || areas_changed
            || maps_changed,
    );
    removed_stores.clear();
    if gate.skip() {
        return;
    }
    let self_pair = self_q.iter().next();
    let self_guid = self_pair.map(|(_, g, _)| g.0);
    // The party's PvP faction group is our own: a 1.12 party is one faction, and an unstreamed
    // member has no descriptor to read one from.
    let own_group = self_pair
        .and_then(|(_, _, store)| crate::ui_unit::faction_group(store, factions.as_deref()));

    let slots: Vec<&GroupMemberEntry> = group.party_slots().collect();

    // Leader and master looter as party indices, 0 for us. They cover our own subgroup's slots
    // only, so a raid leader elsewhere reads 0 here; `GetRaidRosterInfo` carries every subgroup.
    let members: Vec<PartyMemberInfo> = slots
        .iter()
        .map(|m| PartyMemberInfo {
            name: m.name.clone(),
            guid: m.guid,
        })
        .collect();
    let leader_index = if Some(group.leader) == self_guid {
        0
    } else {
        slots
            .iter()
            .position(|m| m.guid == group.leader)
            .map_or(0, |i| i as u32 + 1)
    };
    let (loot_method, master_looter, loot_threshold) = match &group.loot {
        Some(loot) => {
            let method = match loot.method {
                0 => "freeforall",
                1 => "roundrobin",
                2 => "master",
                4 => "needbeforegreed",
                _ => "group",
            };
            // `GetLootMethod`'s second return (`0x4e91b0`): 0 when the looter is us, else the
            // looter's party slot + 1 over the four slots only (`0x4e81a0`, even in a raid), else
            // nil. A master looter in another raid subgroup reads nil, never 0, which would light
            // the crown on our portrait (`PlayerFrame.lua:48`).
            let master = (loot.method == 2 && loot.master != 0)
                .then(|| {
                    if Some(loot.master) == self_guid {
                        Some(0)
                    } else {
                        slots
                            .iter()
                            .position(|m| m.guid == loot.master)
                            .map(|i| i as u32 + 1)
                    }
                })
                .flatten();
            (method.to_string(), master, u32::from(loot.threshold))
        }
        None => ("group".to_string(), None, 0),
    };
    // The raid roster, the one list `GetNumRaidMembers`, `GetRaidRosterInfo` and `UnitInRaid` read.
    let me = self_pair.map(|(_, g, store)| RaidSelf {
        guid: g.0,
        flags: group.own_flags,
        level: store.0.unit_level().unwrap_or(0),
        area: here.area(),
        dead: store.0.unit_is_dead(),
        class: store.0.unit_class(),
    });
    let zone_name = |area: u32| {
        let areas = areas.as_ref()?;
        // A member's wire zone is already a zone; our own area is the finest leaf, so walk it up.
        // `top_zone` leaves a zone as it is, so one resolver serves both.
        areas
            .0
            .name(areas.0.top_zone(area).unwrap_or(area))
            .map(str::to_string)
    };
    let raid = raid_roster(&group, me.as_ref(), &names, &zone_name);
    // The `RAID_ROSTER_UPDATE` key, taken before the roster moves into the snapshot.
    let raid_key: Vec<(u64, u32, u32, bool)> = raid
        .iter()
        .map(|r| (r.guid, r.rank, r.subgroup, r.online))
        .collect();

    let fresh = PartyState {
        members,
        leader_index,
        // `UnitIsPartyLeader` compares a resolved token's guid with it, which an index cannot do
        // for an unstreamed member; zero when ungrouped (see `PartyState::leader_guid`).
        leader_guid: group.leader,
        own_guid: self_guid.unwrap_or(0),
        raid,
        loot_method,
        master_looter,
        loot_threshold,
    };
    if fed.pushed_party.as_ref() != Some(&fresh) {
        gate.audit("feed_party", "the roster snapshot");
        script.set_party(fresh.clone());
        fed.pushed_party = Some(fresh);
    }

    for (i, token) in PARTY_TOKENS.iter().enumerate() {
        let member = slots.get(i);
        let snap = member.map(|m| {
            member_unit_state(
                m,
                group.stats.get(&m.guid),
                index.0.get(&m.guid).and_then(|e| stores.get(*e).ok()),
                &group,
                own_group.clone(),
                chr,
            )
        });
        if fed.units[i] != snap {
            gate.audit("feed_party", "a party-token snapshot");
            script.set_unit(token, snap.clone());
            if let Some(cur) = &snap {
                crate::ui_unit::fire_transitions(
                    &mut script,
                    token,
                    fed.units[i].as_ref(),
                    cur,
                    &edges,
                );
            }
            fed.units[i] = snap;
        }
        // ── PARTY_MEMBER_ENABLE / PARTY_MEMBER_DISABLE ──────────────────────
        //
        // Fired as a slot's object enters (`0x4e86b6`) or leaves (`0x5e9b78`) the object manager,
        // with the 1-based slot as a string. Stock FrameXML ignores them
        // (`PartyMemberFrame.lua:145` is commented out).
        let presence = member.map_or((0, false), |m| (m.guid, index.0.contains_key(&m.guid)));
        if fed.presence[i] != presence {
            gate.audit("feed_party", "a party-slot presence edge");
            // Only the same member's object crossing is an activation, and not on the first look
            // after a VM reset, which would announce an arrival that never happened.
            if !vm_reset && presence.0 != 0 && fed.presence[i].0 == presence.0 {
                let event = if presence.1 {
                    "PARTY_MEMBER_ENABLE"
                } else {
                    "PARTY_MEMBER_DISABLE"
                };
                script.fire_event(event, vec![ScriptValue::Str((i + 1).to_string())]);
            }
            fed.presence[i] = presence;
        }
    }

    // ── raid1..raid40 ──────────────────────────────────────────────────────
    //
    // Token N is `GetRaidRosterInfo` row N, both in `raid_row_guids`' order. Our own row comes off
    // our descriptor: the wire list never contains us.
    let raid_guids = raid_row_guids(&group, self_guid);
    fed.raid_units.resize(RAID_TOKENS.len(), None);
    for (i, token) in RAID_TOKENS.iter().enumerate() {
        let snap = raid_guids.get(i).and_then(|guid| {
            if Some(*guid) == self_guid {
                let (_, _, store) = self_pair?;
                let name = names.peek(*guid).map(str::to_string);
                let mut s = crate::ui_unit::snapshot(store, name, 0, chr);
                s.is_player = true;
                s.guid = *guid;
                s.raid_target = group.raid_target_index(*guid);
                s.faction_group = own_group.clone();
                // A same-faction friendly player, as in `member_unit_state`.
                s.reaction = 5;
                s.is_connected = true;
                Some(s)
            } else {
                let m = group.members.iter().find(|m| m.guid == *guid)?;
                Some(member_unit_state(
                    m,
                    group.stats.get(guid),
                    index.0.get(guid).and_then(|e| stores.get(*e).ok()),
                    &group,
                    own_group.clone(),
                    chr,
                ))
            }
        });
        if fed.raid_units[i] != snap {
            gate.audit("feed_party", "a raid-token snapshot");
            script.set_unit(token, snap.clone());
            if let Some(cur) = &snap {
                crate::ui_unit::fire_transitions(
                    &mut script,
                    token,
                    fed.raid_units[i].as_ref(),
                    cur,
                    &edges,
                );
            }
            fed.raid_units[i] = snap;
        }
    }

    let roster: Vec<u64> = group.members.iter().map(|m| m.guid).collect();
    if roster != fed.roster {
        gate.audit("feed_party", "the roster edge");
        script.fire_event("PARTY_MEMBERS_CHANGED", vec![]);
        fed.roster = roster;
    }
    if group.leader != fed.leader {
        gate.audit("feed_party", "the leader edge");
        script.fire_event("PARTY_LEADER_CHANGED", vec![]);
        fed.leader = group.leader;
    }
    if group.loot != fed.loot {
        gate.audit("feed_party", "the loot-method edge");
        script.fire_event("PARTY_LOOT_METHOD_CHANGED", vec![]);
        fed.loot = group.loot;
    }
    // ── RAID_ROSTER_UPDATE ──────────────────────────────────────────────────
    //
    // Fired when the roster's identity moves (members, order, rank, subgroup, online), not on
    // level or health, which `RaidGroupFrame_OnEvent` re-reads on `UNIT_LEVEL` and `UNIT_HEALTH`.
    // The reference fires it after each raid `SMSG_GROUP_LIST` rebuild (`0x4babef`, or `0x4bada6`
    // once the member names it waits on land), on leaving a raid (`0x4ba57b`) and when world entry
    // fills in our own row (`0x4ba1a2`).
    if raid_key != fed.raid_key {
        gate.audit("feed_party", "the raid-roster edge");
        fed.raid_key = raid_key;
        script.fire_event("RAID_ROSTER_UPDATE", vec![]);
    }

    // ── RAID_TARGET_UPDATE ──────────────────────────────────────────────────────────────────
    //
    // Fired when the board moves, with no argument; `TargetFrame.lua:99` re-reads the icon on it.
    if group.raid_targets != fed.raid_targets {
        gate.audit("feed_party", "the raid-target board edge");
        fed.raid_targets = group.raid_targets;
        script.fire_event("RAID_TARGET_UPDATE", vec![]);
    }

    // ── READY_CHECK ─────────────────────────────────────────────────────────
    //
    // Fired by the reference's open handler `0x4ba360` (at `0x4ba53a`), which also arms the 30 s
    // deadline (`0xb713f4`). `UIParent.lua:571` shows the popup on it, and UIParent's OnUpdate
    // ticks the deadline through `CheckReadyCheckTime` (`UIParent.xml:24`): no clock here.
    if group.ready_check != fed.ready_check {
        gate.audit("feed_party", "the ready-check edge");
        fed.ready_check = group.ready_check;
        // Not on the first look after a VM reset, which would pop a check that has ended.
        if !vm_reset {
            script.fire_event("READY_CHECK", vec![]);
        }
    }
    // Each request arms the engine's side (the leader's close, a member's 30 s deadline); the
    // answer log then replays from this memo's cursor.
    if group.ready_check_requests != fed.ready_check_requests {
        fed.ready_check_requests = group.ready_check_requests;
        fed.answers_forwarded = 0;
        if !vm_reset {
            script.ready_check_request(Some(group.leader) == self_guid);
        }
    }
    for &(guid, ready) in group.ready_check_answers.iter().skip(fed.answers_forwarded) {
        script.ready_check_answered(guid, ready);
    }
    fed.answers_forwarded = group.ready_check_answers.len();

    // ── UPDATE_INSTANCE_INFO ────────────────────────────────────────────────
    //
    // The lockout list with its `Map.dbc` names resolved; a missing catalog shows the map id.
    let saved: Vec<SavedInstanceInfo> = group
        .saved_instances
        .iter()
        .map(|e| SavedInstanceInfo {
            name: map_catalog
                .as_ref()
                .and_then(|c| c.0.name(e.map))
                .map_or_else(|| e.map.to_string(), str::to_string),
            instance: e.instance,
            reset: e.reset,
        })
        .collect();
    // The event follows the answer, as the reference signals once per packet. A fresh VM is
    // re-seeded in silence, since no packet arrived; the pane's `RequestRaidInfo` on show
    // (`RaidFrame.xml:350`) fetches the real list.
    if saved_instances_moved(&saved, group.saved_instances_answers, fed) {
        gate.audit("feed_party", "the saved-instance edge");
        fed.saved = saved.clone();
        fed.saved_answers = group.saved_instances_answers;
        script.set_saved_instances(saved);
        if !vm_reset {
            script.fire_event("UPDATE_INSTANCE_INFO", vec![]);
        }
    }

    if group.pending_invite != fed.invite {
        gate.audit("feed_party", "the invite edge");
        match &group.pending_invite {
            Some(inviter) => script.fire_event(
                "PARTY_INVITE_REQUEST",
                vec![ScriptValue::Str(inviter.clone())],
            ),
            // Also fires on accept and decline: hiding a hidden popup is a no-op, and the accepted
            // guard stops a second decline.
            None => script.fire_event("PARTY_INVITE_CANCEL", vec![]),
        }
        fed.invite = group.pending_invite.clone();
    }
}

/// Our own raid row, which `GroupState` lacks: `SMSG_GROUP_LIST` never lists the recipient.
pub(super) struct RaidSelf {
    pub(super) guid: u64,
    /// Our subgroup bits and assistant flag, `SMSG_GROUP_LIST`'s second byte.
    pub(super) flags: u8,
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

/// The raid rows as guids, in the order [`raid_roster`] builds them, which the RaidFrame's drag,
/// kick and menu verbs address by index: the two must agree. Us first, then the wire's members;
/// the reference appends the local player after them (`0x5e6e73`). Empty outside a raid.
pub(crate) fn raid_row_guids(group: &GroupState, self_guid: Option<u64>) -> Vec<u64> {
    raid_rows(group, self_guid).collect()
}

/// [`raid_row_guids`] as an iterator, so a per-token lookup allocates nothing.
fn raid_rows(group: &GroupState, self_guid: Option<u64>) -> impl Iterator<Item = u64> + '_ {
    let in_raid = group.group_type == GROUPTYPE_RAID;
    self_guid.filter(|_| in_raid).into_iter().chain(
        group
            .members
            .iter()
            .filter(move |_| in_raid)
            .map(|m| m.guid),
    )
}

/// The guid at a 1-based raid row, [`raid_row_guids`]`[index - 1]` without the allocation.
pub(crate) fn raid_row_guid(
    group: &GroupState,
    self_guid: Option<u64>,
    index: usize,
) -> Option<u64> {
    raid_rows(group, self_guid).nth(index.checked_sub(1)?)
}

/// A 1-based raid row index to its guid; row 0, a row past the end or no raid gives `None`.
fn raid_guid_at(group: &GroupState, self_guid: Option<u64>, index: u32) -> Option<u64> {
    raid_row_guid(group, self_guid, usize::try_from(index).ok()?)
}

/// A guid to its name for the by-name opcodes: ours from the name cache, others' from the roster.
fn raid_name_of(
    group: &GroupState,
    self_guid: Option<u64>,
    names: &NameCache,
    guid: u64,
) -> Option<String> {
    if Some(guid) == self_guid {
        return names.peek(guid).map(str::to_string);
    }
    group
        .members
        .iter()
        .find(|m| m.guid == guid)
        .map(|m| m.name.clone())
}

/// A name to the guid `CMSG_GROUP_SET_LEADER` and `CMSG_GROUP_ASSISTANT_LEADER` want (UnitPopup
/// addresses raid rows by name), matched case-insensitively like every roster name compare.
fn raid_guid_for_name(
    group: &GroupState,
    self_guid: Option<u64>,
    names: &NameCache,
    name: &str,
) -> Option<u64> {
    if let Some(g) = self_guid {
        if names.peek(g).is_some_and(|n| n.eq_ignore_ascii_case(name)) {
            return Some(g);
        }
    }
    group
        .members
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(name))
        .map(|m| m.guid)
}

/// `GetRaidRosterInfo`'s array (`0x4bb560`) in [`raid_row_guids`]' order: empty outside a raid,
/// so `GetNumRaidMembers()` answers 0 in a party. The reference's array holds the local player
/// and the wire's list does not, so our row comes from [`RaidSelf`].
fn raid_roster(
    group: &GroupState,
    me: Option<&RaidSelf>,
    names: &NameCache,
    zone_name: &dyn Fn(u32) -> Option<String>,
) -> Vec<RaidMemberInfo> {
    if group.group_type != GROUPTYPE_RAID {
        return Vec::new();
    }
    // Rank 2 leader, 1 assistant, 0 member, as the reference's roster rebuild writes it
    // (`0x4ba90c`, `0x4ba920`, `0x4ba929`) and the binding returns it unchanged.
    let rank_of = |guid: u64, flags: u8| {
        if guid == group.leader {
            2
        } else if flags & GROUP_MEMBER_ASSISTANT != 0 {
            1
        } else {
            0
        }
    };
    // The class byte comes from the name cache for a member, from our descriptor for us.
    let row = |guid: u64,
               name: String,
               flags: u8,
               level: u32,
               zone: Option<String>,
               online: bool,
               ninth: bool,
               class_byte: Option<u8>| {
        let class = class_byte.and_then(crate::ui_unit::class_names);
        RaidMemberInfo {
            name,
            guid,
            rank: rank_of(guid, flags),
            // Stored 0-based; the binding adds one (`0x4bb61a`).
            subgroup: u32::from(flags & GROUP_MEMBER_SUBGROUP),
            level,
            class: class.map(|(n, _)| n.to_string()),
            class_file: class.map(|(_, f)| f.to_string()),
            zone,
            online,
            ninth,
        }
    };
    let mut roster = Vec::with_capacity(group.members.len() + 1);
    if let Some(me) = me {
        roster.push(row(
            me.guid,
            // An unresolved name gets the binding's miss tuple, not a half-filled row (`0x4bb5f8`).
            names.peek(me.guid).unwrap_or_default().to_string(),
            me.flags,
            me.level,
            me.area.and_then(zone_name),
            true,
            me.dead,
            me.class,
        ));
    }
    for m in &group.members {
        let stats = group.stats.get(&m.guid);
        let online = m.status & member_status::ONLINE != 0;
        roster.push(row(
            m.guid,
            m.name.clone(),
            m.flags,
            stats.and_then(|s| s.level).map_or(0, u32::from),
            stats
                .and_then(|s| s.zone)
                .and_then(|z| zone_name(u32::from(z))),
            online,
            // Return 9's cached arm needs both `0x4` and `0x1` in `[edi+0x18]`, vmangos's `DEAD`
            // and `ONLINE` (`Group/Group.cpp:45-63`): an offline dead member answers nil.
            online && m.status & member_status::DEAD != 0,
            names.player_traits(m.guid).map(|(_, class, _)| class),
        ));
    }
    roster
}

/// One member's merged-view unit snapshot (see [`feed_party`]).
fn member_unit_state(
    m: &GroupMemberEntry,
    stats: Option<&PartyMemberStatsInfo>,
    // Their live descriptor while the object manager holds them (the reference's `0x468460`).
    store: Option<&ObjectStore>,
    group: &GroupState,
    own_group: Option<String>,
    // `ChrClasses.dbc`, for the relic slot alone: only the live leg has a class byte to key it
    // by, so an unstreamed paladin reads no relic slot.
    classes: Option<&benilla_formats::ChrClasses>,
) -> UnitState {
    let mut s = match store {
        Some(store) => crate::ui_unit::snapshot(store, Some(m.name.clone()), 0, classes),
        // Unstreamed: the roster record, snapshotted from the descriptor at despawn (`0x5f0880`),
        // seated at 1/1 for a member never seen (`0x4e82d0`) and patched by the wire. The
        // reference's getters read the descriptor, then the party record (`0x496400`), then the
        // pet record (`0x496420`); a `partyN` token is never a pet, so the pet leg cannot arise.
        None => UnitState {
            exists: true,
            name: Some(m.name.clone()),
            health: stats.and_then(|s| s.cur_hp).map_or(0, u32::from),
            max_health: stats.and_then(|s| s.max_hp).map_or(0, u32::from),
            level: stats.and_then(|s| s.level).map_or(0, u32::from),
            power_type: stats.map_or(0, PartyMemberStatsInfo::shown_power_type),
            // Divided as on the live leg (`UnitMana`'s record path, `0x517744`-`0x51775e`).
            power: stats.map_or(0, PartyMemberStatsInfo::shown_power),
            max_power: stats.map_or(0, PartyMemberStatsInfo::shown_max_power),
            // Dead and ghost from the record, as the reference's `UnitIsDead` (`0x517b5d`,
            // `+0x08 & 4`) and `UnitIsGhost` (`0x517c32`, `& 8`) read it: fresher than the roster
            // byte, which only `SMSG_GROUP_LIST` moves. Connected stays the roster's, though the
            // reference reads the record's bit 0 (`0x517dd3`); its no-object PvP and FFA reads are
            // untraced, and `UnitIsAFK`/`UnitIsDND` are not 1.12 bindings.
            dead: stats.is_some_and(|s| s.status.unwrap_or(0) & member_status::DEAD != 0),
            ghost: stats.is_some_and(|s| s.status.unwrap_or(0) & member_status::GHOST != 0),
            ..Default::default()
        },
    };
    s.is_player = true;
    // A party member is always a same-faction friendly player: the popup's `UnitCanCooperate`
    // gate reads the reaction, which neither leg resolves for a party token.
    s.guid = m.guid;
    s.raid_target = group.raid_target_index(m.guid);
    s.reaction = 5;
    s.faction_group = own_group;
    // The roster status byte overlays both legs.
    s.is_connected = m.status & member_status::ONLINE != 0;
    s.is_afk = m.status & member_status::AFK != 0;
    s.is_dnd = m.status & member_status::DND != 0;
    s.is_pvp_ffa = m.status & member_status::PVP_FFA != 0;
    s.pvp = s.pvp || m.status & member_status::PVP != 0;
    s.ghost = s.ghost || m.status & member_status::GHOST != 0;
    s.dead = s.dead || m.status & member_status::DEAD != 0;
    s
}

/// `"party1".."party4"` → the roster entry it names.
fn member_for_token<'a>(group: &'a GroupState, token: &str) -> Option<&'a GroupMemberEntry> {
    let n: usize = token.strip_prefix("party")?.parse().ok()?;
    // The slot view the feed publishes the tokens from, so the two agree.
    (1..=4)
        .contains(&n)
        .then(|| group.party_slots().nth(n - 1))?
}

/// Drain the Lua party intents into their `CMSG_*` sends.
pub(super) fn drain_party(
    script: Option<NonSendMut<UiScript>>,
    mut group: ResMut<GroupState>,
    selection: Res<Selection>,
    names: Res<NameCache>,
    self_q: Query<&Guid, With<SelfPlayer>>,
    commands: Res<NetCommands>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let self_guid = self_q.iter().next().map(|g| g.0);
    for req in script.take_party_requests() {
        // In the sandbox the server holds no group, so the group-changing intents apply to the
        // mirror as its echo would. Invites and accept/decline stay real: a real group ends it.
        if group.test && test_apply_local(&mut group, &req, self_guid, selection.guid) {
            continue;
        }
        match req {
            PartyRequest::Accept => {
                let _ = commands.0.send(ClientCommand::GroupAccept);
                group.pending_invite = None;
            }
            PartyRequest::Decline => {
                let _ = commands.0.send(ClientCommand::GroupDecline);
                group.pending_invite = None;
            }
            PartyRequest::Leave => {
                let _ = commands.0.send(ClientCommand::GroupLeave);
            }
            PartyRequest::InviteName(name) => {
                if let Some(t) = tutorials.as_mut() {
                    t.write(crate::tutorial::TutorialEvent::Acknowledge {
                        id: crate::tutorial::id::GROUPING,
                    });
                }
                let _ = commands.0.send(ClientCommand::GroupInvite { name });
            }
            PartyRequest::InviteUnit(token) => {
                // `target` resolves through the selection when it is a player
                // (`InviteToParty(unit)`); a roster token already has a name.
                let name = if token == "target" {
                    selection
                        .guid
                        .filter(|g| benilla_protocol::guid::is_player(*g))
                        .and_then(|g| names.resolve(g, &commands).map(str::to_string))
                } else {
                    member_for_token(&group, &token).map(|m| m.name.clone())
                };
                if let Some(name) = name {
                    if let Some(t) = tutorials.as_mut() {
                        t.write(crate::tutorial::TutorialEvent::Acknowledge {
                            id: crate::tutorial::id::GROUPING,
                        });
                    }
                    let _ = commands.0.send(ClientCommand::GroupInvite { name });
                }
            }
            PartyRequest::UninviteUnit(token) => {
                if let Some(m) = member_for_token(&group, &token) {
                    let _ = commands.0.send(ClientCommand::GroupUninvite {
                        name: m.name.clone(),
                    });
                }
            }
            PartyRequest::PromoteUnit(token) => {
                if let Some(m) = member_for_token(&group, &token) {
                    let _ = commands
                        .0
                        .send(ClientCommand::GroupSetLeader { guid: m.guid });
                }
            }
            PartyRequest::LootMethod {
                method,
                master_name,
                threshold: asked,
            } => {
                let Some(method_id) = loot_method_id(&method) else {
                    continue;
                };
                let master = if method_id == 2 {
                    master_name
                        .as_deref()
                        .and_then(|n| {
                            group
                                .members
                                .iter()
                                .find(|m| m.name.eq_ignore_ascii_case(n))
                                .map(|m| m.guid)
                        })
                        .or(self_guid)
                        .unwrap_or(0)
                } else {
                    0
                };
                // An absent threshold keeps the group's current one (at least 2), where the
                // reference's binding defaults it to 2 (`0x4e946d`), so a method change resets it.
                let threshold = asked.unwrap_or_else(|| {
                    group
                        .loot
                        .map(|l| u32::from(l.threshold))
                        .filter(|t| *t >= 2)
                        .unwrap_or(2)
                });
                let _ = commands.0.send(ClientCommand::LootMethod {
                    method: method_id,
                    master,
                    threshold,
                });
            }
            PartyRequest::SetRaidTarget { unit, index } => {
                // Us, the selection or a roster slot; an unresolvable token is a no-op.
                let guid = match unit.as_str() {
                    "player" => self_q.iter().next().map(|g| g.0),
                    "target" => selection.guid,
                    t => member_for_token(&group, t).map(|m| m.guid),
                };
                let Some(guid) = guid else { continue };
                // Lua marks 1..8 are wire icons 0..7. Setting sends (mark - 1, guid) and the
                // server clears the unit's old icon (`Group::SetTargetIcon`); clearing re-sends
                // the unit's current icon with guid 0, since the wire has no clear by unit.
                if index >= 1 {
                    let _ = commands.0.send(ClientCommand::SetRaidTarget {
                        icon: index - 1,
                        guid,
                    });
                } else {
                    let current = group.raid_target_index(guid);
                    if current >= 1 {
                        let _ = commands.0.send(ClientCommand::SetRaidTarget {
                            icon: current - 1,
                            guid: 0,
                        });
                    }
                }
            }
            PartyRequest::LootThreshold(threshold) => {
                let (method, master) = group
                    .loot
                    .map_or((3, 0), |l| (u32::from(l.method), l.master));
                let _ = commands.0.send(ClientCommand::LootMethod {
                    method,
                    master,
                    threshold,
                });
            }
            // ── The raid-management verbs ────────────────────────────────────
            //
            // The RaidFrame names rows by index, UnitPopup by name. A row that is not there sends
            // nothing, as in the reference's bindings (`UninviteFromRaid`, `0x48a5c9`).
            PartyRequest::ConvertToRaid => {
                let _ = commands.0.send(ClientCommand::GroupRaidConvert);
            }
            PartyRequest::SetSubgroup { index, group: sub } => {
                // Lua subgroups 1..8 are wire 0..7, the flags' bits 0-2
                // (`Server/Packets/Group.cpp:158`); out-of-range ones are dropped, never wrapped.
                let Some(sub) = (1..=8).contains(&sub).then(|| (sub - 1) as u8) else {
                    continue;
                };
                let Some(name) = raid_guid_at(&group, self_guid, index)
                    .and_then(|g| raid_name_of(&group, self_guid, &names, g))
                else {
                    continue;
                };
                let _ = commands
                    .0
                    .send(ClientCommand::GroupChangeSubGroup { name, group: sub });
            }
            PartyRequest::SwapSubgroup { index, other } => {
                let pair = raid_guid_at(&group, self_guid, index)
                    .zip(raid_guid_at(&group, self_guid, other))
                    .and_then(|(a, b)| {
                        Some((
                            raid_name_of(&group, self_guid, &names, a)?,
                            raid_name_of(&group, self_guid, &names, b)?,
                        ))
                    });
                if let Some((name, other)) = pair {
                    let _ = commands
                        .0
                        .send(ClientCommand::GroupSwapSubGroup { name, other });
                }
            }
            PartyRequest::PromoteName(name) => {
                if let Some(guid) = raid_guid_for_name(&group, self_guid, &names, &name) {
                    let _ = commands.0.send(ClientCommand::GroupSetLeader { guid });
                }
            }
            PartyRequest::AssistantLeader { name, grant } => {
                if let Some(guid) = raid_guid_for_name(&group, self_guid, &names, &name) {
                    let _ = commands
                        .0
                        .send(ClientCommand::GroupAssistantLeader { guid, grant });
                }
            }
            PartyRequest::UninviteRaid(index) => {
                // Sent by name. The reference's `UninviteFromRaid` and `UninviteFromParty` share
                // the sender `0x5e9510`, which kicks by name when the target's object is present
                // and by `CMSG_GROUP_UNINVITE_GUID` (`0x5e953e`) only when it is absent.
                if let Some(name) = raid_guid_at(&group, self_guid, index)
                    .and_then(|g| raid_name_of(&group, self_guid, &names, g))
                {
                    let _ = commands.0.send(ClientCommand::GroupUninvite { name });
                }
            }
            PartyRequest::ReadyCheckStart => {
                let _ = commands.0.send(ClientCommand::ReadyCheckStart);
            }
            PartyRequest::ReadyCheckAnswer(ready) => {
                let _ = commands.0.send(ClientCommand::ReadyCheckAnswer { ready });
            }
            PartyRequest::RequestRaidInfo => {
                let _ = commands.0.send(ClientCommand::RequestRaidInfo);
            }
        }
    }
}

/// The sandbox half of [`drain_party`]: apply one group-changing intent to the mirror as the
/// server's echo would. False for the intents that stay real (invites, accept and decline).
fn test_apply_local(
    group: &mut GroupState,
    req: &PartyRequest,
    self_guid: Option<u64>,
    target_guid: Option<u64>,
) -> bool {
    match req {
        PartyRequest::Leave => {
            // The all-zero list's reset: the group facts and sandbox flag, not the session state.
            group.leave_group();
            true
        }
        PartyRequest::UninviteUnit(token) => {
            if let Some(guid) = member_for_token(group, token).map(|m| m.guid) {
                group.members.retain(|m| m.guid != guid);
                group.stats.remove(&guid);
            }
            true
        }
        PartyRequest::PromoteUnit(token) => {
            if let Some(guid) = member_for_token(group, token).map(|m| m.guid) {
                group.leader = guid;
            }
            true
        }
        PartyRequest::LootMethod {
            method,
            master_name,
            threshold,
        } => {
            if let Some(method_id) = loot_method_id(method) {
                let master = if method_id == 2 {
                    master_name
                        .as_deref()
                        .and_then(|n| {
                            group
                                .members
                                .iter()
                                .find(|m| m.name.eq_ignore_ascii_case(n))
                                .map(|m| m.guid)
                        })
                        .or(self_guid)
                        .unwrap_or(0)
                } else {
                    0
                };
                // The live drain's rule: an absent threshold keeps the current one.
                let threshold = threshold
                    .map_or_else(|| group.loot.map_or(2, |l| l.threshold.max(2)), |t| t as u8);
                group.loot = Some(GroupLootInfo {
                    method: method_id as u8,
                    master,
                    threshold,
                });
            }
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
        PartyRequest::SetRaidTarget { unit, index } => {
            let guid = match unit.as_str() {
                "player" => self_guid,
                "target" => target_guid,
                t => member_for_token(group, t).map(|m| m.guid),
            };
            let Some(guid) = guid else { return true };
            if *index >= 1 {
                // The server clears the unit's old icon first (`SetTargetIcon`): one mark per unit.
                for slot in group.raid_targets.iter_mut() {
                    if *slot == guid {
                        *slot = 0;
                    }
                }
                group.apply_raid_target(index - 1, guid);
            } else {
                let current = group.raid_target_index(guid);
                if current >= 1 {
                    group.apply_raid_target(current - 1, 0);
                }
            }
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
            if let Some(guid) =
                raid_guid_at(group, self_guid, *index).filter(|g| Some(*g) != self_guid)
            {
                group.members.retain(|m| m.guid != guid);
                group.stats.remove(&guid);
            }
            true
        }
        PartyRequest::PromoteName(name) => {
            if let Some(m) = group
                .members
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(name))
            {
                group.leader = m.guid;
            }
            true
        }
        PartyRequest::AssistantLeader { name, grant } => {
            if let Some(m) = group
                .members
                .iter_mut()
                .find(|m| m.name.eq_ignore_ascii_case(name))
            {
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
            group.apply_ready_check_request(true);
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
    let guid = raid_guid_at(group, self_guid, index)?;
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
    let Some(guid) = raid_guid_at(group, self_guid, index) else {
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

/// `SetLootMethod`'s string → the wire's `LootMethod` id.
fn loot_method_id(method: &str) -> Option<u32> {
    Some(match method {
        "freeforall" => 0,
        "roundrobin" => 1,
        "master" => 2,
        "group" => 3,
        "needbeforegreed" => 4,
        _ => return None,
    })
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
    let lines = group.apply_list(
        0,
        0,
        members,
        0xF001,
        Some(GroupLootInfo {
            method: 2,
            master: 0xF003,
            threshold: 3,
        }),
    );
    // Blip offsets from us on WoW axes (+x north, +y west), truncated to `i16` as on the wire.
    let seat = |dx: f32, dy: f32| player_xy.map(|(px, py)| ((px + dx) as i16, (py + dy) as i16));
    for (guid, hp, max, level, power_type, pos) in [
        (0xF001u64, 820u16, 1240u16, 32u16, 0u8, seat(30.0, 0.0)),
        (0xF002, 455, 980, 30, 3, seat(0.0, 80.0)),
        (0xF003, 0, 1105, 31, 0, seat(-300.0, 0.0)),
        (0xF004, 0, 0, 0, 0, None),
    ] {
        group.apply_stats(
            guid,
            true,
            PartyMemberStatsInfo {
                status: None,
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
/// name cache, where [`raid_roster`] reads them; six lockouts fill the Raid Info panel.
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
    let lines = group.apply_list(
        1,
        0, // our flags: subgroup 1 (0-based 0), no assistant bit
        members,
        leader,
        Some(GroupLootInfo {
            method: 2,
            master: leader,
            threshold: 3,
        }),
    );
    // Records for every member: fake guids never stream, so the merged view shows these.
    for (i, _) in ROSTER.iter().enumerate() {
        let guid = 0xF100 + i as u64;
        let dead = i == 3;
        group.apply_stats(
            guid,
            true,
            PartyMemberStatsInfo {
                status: None,
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
mod tests {
    use super::*;

    #[test]
    fn an_out_of_range_member_reads_the_record_and_divides_its_rage() {
        let m = GroupMemberEntry {
            name: "Brisca".into(),
            guid: 0x1234,
            status: member_status::ONLINE,
            flags: 0,
        };
        let record = PartyMemberStatsInfo {
            cur_hp: Some(2400),
            max_hp: Some(3000),
            level: Some(41),
            power_type: Some(1), // POWER_RAGE
            cur_power: Some(570),
            max_power: Some(1000),
            ..PartyMemberStatsInfo::default()
        };
        let s = member_unit_state(&m, Some(&record), None, &GroupState::default(), None, None);
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
        assert!(s.exists && s.is_player && s.is_connected);

        // No record at all (a real roster always seats one): an existing player with empty bars.
        let bare = member_unit_state(&m, None, None, &GroupState::default(), None, None);
        assert_eq!((bare.health, bare.max_health, bare.power), (0, 0, 0));
        assert!(bare.exists);
    }

    /// vmangos re-sends a member's status on join, login and the AFK, DND, PvP, FFA and ghost
    /// toggles, never on a death, so the roster byte can say alive while the record says dead.
    #[test]
    fn an_out_of_range_members_dead_and_ghost_come_off_the_record() {
        let m = GroupMemberEntry {
            name: "Brisca".into(),
            guid: 0x1234,
            // The stale roster echo: online, alive.
            status: member_status::ONLINE,
            flags: 0,
        };
        let dead = PartyMemberStatsInfo {
            status: Some(member_status::ONLINE | member_status::DEAD),
            ..PartyMemberStatsInfo::default()
        };
        let s = member_unit_state(&m, Some(&dead), None, &GroupState::default(), None, None);
        assert!(
            s.dead,
            "the record says dead even though the roster echo does not"
        );
        assert!(!s.ghost);

        let ghost = PartyMemberStatsInfo {
            status: Some(member_status::ONLINE | member_status::GHOST),
            ..PartyMemberStatsInfo::default()
        };
        let s = member_unit_state(&m, Some(&ghost), None, &GroupState::default(), None, None);
        assert!(s.ghost);
        assert!(!s.dead, "a released ghost is not `dead` — only a ghost");

        // The overlay ORs: a roster byte carrying the bit still wins over a record without it.
        let stale = GroupMemberEntry {
            status: member_status::ONLINE | member_status::DEAD,
            ..m.clone()
        };
        let s = member_unit_state(
            &stale,
            Some(&PartyMemberStatsInfo::placeholder(true)),
            None,
            &GroupState::default(),
            None,
            None,
        );
        assert!(s.dead);
    }

    #[test]
    fn a_second_empty_answer_is_still_a_saved_instance_edge() {
        let mut fed = FedParty::default();
        assert!(
            saved_instances_moved(&[], 1, &fed),
            "the first answer moves it"
        );
        fed.saved_answers = 1;
        assert!(
            !saved_instances_moved(&[], 1, &fed),
            "and nothing has happened since"
        );
        assert!(
            saved_instances_moved(&[], 2, &fed),
            "the SECOND empty answer is an edge too — the button is decided on it, and a diff \
             over the list alone can never reach it"
        );
        let one = [SavedInstanceInfo {
            name: "Molten Core".into(),
            instance: 1234,
            reset: 86_400,
        }];
        assert!(saved_instances_moved(&one, 1, &fed));
    }

    #[test]
    fn partytest_sandbox_applies_menu_intents_locally() {
        let mut group = GroupState::default();
        synthetic_roster(&mut group, None);
        assert!(group.test, "the synthetic roster arms the sandbox");
        let (me, mob) = (Some(0x5E1Fu64), Some(0xB0B0u64));

        // Skull on the target, then Cross: one mark per unit.
        let mark = |i| PartyRequest::SetRaidTarget {
            unit: "target".into(),
            index: i,
        };
        assert!(test_apply_local(&mut group, &mark(8), me, mob));
        assert_eq!(group.raid_target_index(0xB0B0), 8);
        test_apply_local(&mut group, &mark(7), me, mob);
        assert_eq!(group.raid_target_index(0xB0B0), 7, "the old icon clears");
        assert_eq!(group.raid_targets[7], 0);
        test_apply_local(&mut group, &mark(0), me, mob);
        assert_eq!(group.raid_target_index(0xB0B0), 0, "NONE clears");

        // Promote party1 (Alice), kick party2 (Bob), retune the loot.
        test_apply_local(
            &mut group,
            &PartyRequest::PromoteUnit("party1".into()),
            me,
            None,
        );
        assert_eq!(group.leader, 0xF001);
        test_apply_local(
            &mut group,
            &PartyRequest::UninviteUnit("party2".into()),
            me,
            None,
        );
        assert!(!group.members.iter().any(|m| m.guid == 0xF002));
        assert!(!group.stats.contains_key(&0xF002));
        test_apply_local(
            &mut group,
            &PartyRequest::LootMethod {
                method: "needbeforegreed".into(),
                master_name: None,
                threshold: None,
            },
            me,
            None,
        );
        test_apply_local(&mut group, &PartyRequest::LootThreshold(4), me, None);
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
            me,
            None
        ));
        test_apply_local(&mut group, &PartyRequest::Leave, me, None);
        assert!(!group.in_group && !group.test);
        synthetic_roster(&mut group, None);
        group.apply_list(0, 0, vec![], 0x123, None);
        assert!(!group.test, "the real wire always wins");
    }

    #[test]
    fn the_raid_roster_maps_the_wire_to_get_raid_roster_info() {
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
                // Alice: subgroup 2 (stored), an assistant, online and dead.
                member(
                    0xA11CE,
                    "Alice",
                    member_status::ONLINE | member_status::DEAD,
                    2 | GROUP_MEMBER_ASSISTANT,
                ),
                // Bob: subgroup 0, offline and dead, which must not set return 9.
                member(0xB0B, "Bob", member_status::DEAD, 0),
            ],
            ..Default::default()
        };
        group.stats.insert(
            0xA11CE,
            PartyMemberStatsInfo {
                level: Some(58),
                zone: Some(1537),
                ..Default::default()
            },
        );
        let zone = |id: u32| (id == 1537).then(|| "Ironforge".to_string());
        let me = RaidSelf {
            guid: 0x5E1F,
            flags: 0,
            level: 60,
            area: Some(1537),
            dead: false,
            // Off our descriptor, the only source of it for us. 11 is Druid.
            class: Some(11),
        };

        let roster = raid_roster(&group, Some(&me), &names, &zone);
        assert_eq!(roster.len(), 3, "the player is spliced back in");

        // Row 1, us: leader (rank 2), subgroup stored 0, class off the descriptor.
        assert_eq!(roster[0].name, "Me");
        assert_eq!(roster[0].rank, 2);
        assert_eq!(
            roster[0].subgroup, 0,
            "stored 0-based; the BINDING adds one"
        );
        assert_eq!(roster[0].level, 60);
        assert_eq!(roster[0].class_file.as_deref(), Some("DRUID"));
        assert_eq!(roster[0].zone.as_deref(), Some("Ironforge"));
        assert!(roster[0].online && !roster[0].ninth);

        // Row 2: an assistant, online and dead, the pair the reference's cached arm tests.
        assert_eq!((roster[1].rank, roster[1].subgroup), (1, 2));
        assert_eq!(roster[1].level, 58);
        assert_eq!(roster[1].class_file.as_deref(), Some("WARRIOR"));
        assert!(roster[1].online && roster[1].ninth, "online AND dead");

        // Row 3: offline, so return 9 is nil though dead, and an unresolved class is nil.
        assert!(!roster[2].online, "offline");
        assert!(!roster[2].ninth, "0x4 without 0x1 is not the arm");
        assert_eq!(roster[2].class, None);
        assert_eq!(roster[2].zone, None, "no stats packet, no zone");

        // A party is not a raid: the list is empty, so `GetNumRaidMembers()` answers 0, while
        // `IsRaidLeader()` still answers 1.
        group.group_type = 0;
        assert!(raid_roster(&group, Some(&me), &names, &zone).is_empty());
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
        let me = Some(0x5E1Fu64);

        assert_eq!(raid_row_guids(&group, me), vec![0x5E1F, 0xA11CE, 0xB0B]);
        assert_eq!(raid_guid_at(&group, me, 1), Some(0x5E1F), "row 1 is us");
        assert_eq!(raid_guid_at(&group, me, 3), Some(0xB0B));
        for miss in [0, 4, 500] {
            assert_eq!(
                raid_guid_at(&group, me, miss),
                None,
                "index {miss} names nobody"
            );
        }

        let mut names = NameCache::default();
        names.insert_player(0x5E1F, "Kel".into(), None);
        assert_eq!(
            raid_name_of(&group, me, &names, 0x5E1F).as_deref(),
            Some("Kel")
        );
        assert_eq!(
            raid_name_of(&group, me, &names, 0xA11CE).as_deref(),
            Some("Alice")
        );
        assert_eq!(raid_name_of(&group, me, &names, 0xDEAD), None);

        assert_eq!(
            raid_guid_for_name(&group, me, &names, "alice"),
            Some(0xA11CE)
        );
        assert_eq!(raid_guid_for_name(&group, me, &names, "KEL"), Some(0x5E1F));
        assert_eq!(raid_guid_for_name(&group, me, &names, "Nobody"), None);

        // Outside a raid all are empty, so a raid verb in a party sends nothing.
        group.group_type = 0;
        assert!(raid_row_guids(&group, me).is_empty());
        assert_eq!(raid_guid_at(&group, me, 1), None);
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

        // Row 2 is the first wire member, and `synthetic_raid` gives it the assistant bit.
        assert_eq!(test_subgroup_of(&group, me, 2), Some(0));
        assert!(group.members[0].flags & GROUP_MEMBER_ASSISTANT != 0);

        // Move it to subgroup 8 in Lua, 7 on the wire.
        assert!(test_apply_local(
            &mut group,
            &PartyRequest::SetSubgroup { index: 2, group: 8 },
            me,
            None
        ));
        assert_eq!(test_subgroup_of(&group, me, 2), Some(7));
        assert!(
            group.members[0].flags & GROUP_MEMBER_ASSISTANT != 0,
            "the assistant bit rides the same byte and must survive the move"
        );

        // Swap it with row 1, us, whose subgroup lives in `own_flags`.
        let mine = test_subgroup_of(&group, me, 1).unwrap();
        assert!(test_apply_local(
            &mut group,
            &PartyRequest::SwapSubgroup { index: 1, other: 2 },
            me,
            None
        ));
        assert_eq!(test_subgroup_of(&group, me, 1), Some(7));
        assert_eq!(test_subgroup_of(&group, me, 2), Some(mine));

        assert!(test_apply_local(
            &mut group,
            &PartyRequest::SetSubgroup { index: 2, group: 9 },
            me,
            None
        ));
        assert_eq!(
            test_subgroup_of(&group, me, 2),
            Some(mine),
            "9 is not a subgroup"
        );

        // Echoed to us as leader, our ready check moves the request count, not the popup's.
        let (ticket, requests) = (group.ready_check, group.ready_check_requests);
        assert!(test_apply_local(
            &mut group,
            &PartyRequest::ReadyCheckStart,
            me,
            None
        ));
        assert_eq!(
            (group.ready_check, group.ready_check_requests),
            (ticket, requests + 1)
        );

        let n = group.members.len();
        assert!(test_apply_local(
            &mut group,
            &PartyRequest::UninviteRaid(1),
            me,
            None
        ));
        assert_eq!(
            group.members.len(),
            n,
            "row 1 is us; the server would refuse too"
        );
        assert!(test_apply_local(
            &mut group,
            &PartyRequest::UninviteRaid(3),
            me,
            None
        ));
        assert_eq!(group.members.len(), n - 1);
    }
}
