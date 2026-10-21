//! The party feed and drain: push the roster and the `party1..4` and `raid1..40` unit snapshots
//! into the VM, fire the party events on their edges, and answer what the party calls left
//! ([`super::calls`]). The state and its lines live in the parent module.

use benilla_protocol::messages::{
    member_status, GroupLootInfo, GroupMemberEntry, ObjectType, PartyMemberStatsInfo,
    GROUP_MEMBER_ASSISTANT,
};
use benilla_ui::script::{
    PartyMemberInfo, PartyRequest, PartyState, RaidMemberInfo, SavedInstanceInfo, ScriptValue,
    UiScript, UnitState,
};
use bevy::prelude::*;

use crate::creature_type::CreatureTypeSources;
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_script::gate;

use super::pets::{self, Roster};
use super::{GroupSignal, GroupState};
use crate::query_cache::Ask;

// ─── The VM feed and drain ───────────────────────────────────────────────────────────────────────

/// What the feed last pushed: the values its event edges fire on.
#[derive(Default)]
pub(crate) struct FedParty {
    /// How far into [`GroupState::signals`] the feed has fired.
    signals: u64,
    units: [Option<UnitState>; 4],
    /// Each party slot's (member guid, object streamed) pair, the `PARTY_MEMBER_ENABLE`/`DISABLE`
    /// edge; with the guid in it a new occupant is a roster edge, not an activation.
    presence: [(u64, bool); 4],
    pushed_party: Option<PartyState>,
    /// The gate's counters: the name cache's landed generation (other feeds' per-frame resolves
    /// defeat its `is_changed`) and the area under us (our raid row's zone).
    names_generation: gate::Watch,
    area: gate::Watch,
    /// The guild cache's generation, every held member's guild leg.
    guild_generation: gate::Watch,
    raid_units: Vec<Option<UnitState>>,
    /// `partypet1..4` and `raidpet1..40` (see [`pets`]).
    pets: pets::FedPets,
    saved: Vec<SavedInstanceInfo>,
    saved_answers: u32,
    ready_check: u32,
    ready_check_requests: u32,
    answers_forwarded: usize,
}

pub(crate) const PARTY_TOKENS: [&str; 4] = ["party1", "party2", "party3", "party4"];

/// `partypet1..partypet4`, the pet of each [`PARTY_TOKENS`] slot (`0x4e81d0`).
pub(crate) const PARTY_PET_TOKENS: [&str; 4] = ["partypet1", "partypet2", "partypet3", "partypet4"];

/// A new list or a new answer: `RaidFrame.lua:43-52` decides the Raid Info button on the second
/// `UPDATE_INSTANCE_INFO`, which an unchanging empty list reaches only through the answer count.
fn saved_instances_moved(saved: &[SavedInstanceInfo], answers: u32, fed: &FedParty) -> bool {
    saved != fed.saved || answers != fed.saved_answers
}

/// What a world entry's fresh VM is told in place of the signals it never heard: one of each
/// event the state it finds still carries, in the order a list signals them. The reference has no
/// such pass: its world entry (`0x4908c0`) signals the roster only through the raid leg's fill of
/// a zero row (`0x4ba140`, signalling at `0x4ba1a2`), and stock `RaidFrame.lua:32` reads the raid
/// on `PLAYER_LOGIN` instead. It is benilla's seam: the in-world VM is built once the loading
/// cover is up (`ui_script::lifecycle`), so a list can land before any VM hears it. A reload's VM
/// is seeded before its load instead and told none ([`seed_reloaded_vm`]).
fn catch_up(group: &GroupState) -> Vec<GroupSignal> {
    [
        (GroupSignal::MembersChanged, group.listed),
        (GroupSignal::LeaderChanged, group.leader != 0),
        (GroupSignal::LootMethodChanged, group.loot.is_some()),
        (GroupSignal::RaidRosterUpdate, group.raid_held()),
        (GroupSignal::RaidTargetUpdate, group.raid_targets != [0; 8]),
    ]
    .into_iter()
    .filter_map(|(signal, owed)| owed.then_some(signal))
    .collect()
}

/// The VM a `/reload` built and [`seed_reloaded_vm`] seeded, by its session, and how far into
/// [`GroupState::signals`] the seed reached: the feed tells that VM only what was signalled after.
#[derive(Resource, Default)]
pub(crate) struct ReloadSeed(Option<(u64, u64)>);

/// Seed a reload's VM with the group before the stock frames load into it: the roster and the
/// party and raid unit tokens, as the feed pushes them, with no event. The reference's reload
/// builds its frames over the engine's group, which they read as they load
/// (`PartyMemberFrame.lua:20-24`, `RaidFrame.lua:32`), and signals none of the group events: their
/// sites are the group packets' handlers and the name callback, and the world entry's raid fill
/// signals only for an empty row (`0x4ba140`, `0x4ba15c`), which the first entry filled. So the
/// feed tells this VM no [`catch_up`].
pub(crate) fn seed_reloaded_vm(world: &mut World, script: &mut UiScript) {
    world.init_resource::<ReloadSeed>();
    let seeded = world.run_system_cached_with(
        |bevy::ecs::system::InMut(script): bevy::ecs::system::InMut<UiScript>,
         view: GroupView,
         mut seed: ResMut<ReloadSeed>| {
            let mut fed = FedParty::default();
            push_group(&view, script, &mut fed, &gate::Gate::new(true), true);
            seed.0 = Some((script.session(), view.group.signals.end()));
        },
        script,
    );
    if let Err(e) = seeded {
        warn!("ui_party: the reload's VM is not seeded with the group: {e}");
    }
}

/// The ready check a dying VM held ([`fold_dying_vm_ready_check`]), for the next VM's seed
/// ([`seed_vm_ready_check`]).
#[derive(Resource, Default)]
pub(crate) struct ReadyCheckCarry(benilla_ui::script::ReadyCheckState);

/// Fold the ready check out of the dying VM, with the item locks: its deadline (`[0xb713f4]`) and
/// pending flags are engine state, which no reload path writes.
pub(crate) fn fold_dying_vm_ready_check(world: &mut World) {
    let Some(state) = world
        .get_non_send_resource_mut::<UiScript>()
        .map(|mut s| s.take_ready_check())
    else {
        return;
    };
    world.insert_resource(ReadyCheckCarry(state));
}

/// Seat a new in-game VM's ready check before its load runs: a `/reload`'s is the dying VM's, a
/// world entry's none, as `InitializeGame`'s group init zeroes it (`0x4ba130`, from `0x48f603`).
pub(crate) fn seed_vm_ready_check(world: &mut World, script: &mut UiScript, reload: bool) {
    let carried = world
        .remove_resource::<ReadyCheckCarry>()
        .unwrap_or_default();
    if reload {
        script.seed_ready_check(carried.0);
    }
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

/// `raidpet1..raidpet40`, the pet of each [`RAID_TOKENS`] row (`0x491960`).
#[rustfmt::skip]
pub(crate) const RAID_PET_TOKENS: [&str; 40] = [
    "raidpet1", "raidpet2", "raidpet3", "raidpet4", "raidpet5", "raidpet6", "raidpet7",
    "raidpet8", "raidpet9", "raidpet10", "raidpet11", "raidpet12", "raidpet13", "raidpet14",
    "raidpet15", "raidpet16", "raidpet17", "raidpet18", "raidpet19", "raidpet20", "raidpet21",
    "raidpet22", "raidpet23", "raidpet24", "raidpet25", "raidpet26", "raidpet27", "raidpet28",
    "raidpet29", "raidpet30", "raidpet31", "raidpet32", "raidpet33", "raidpet34", "raidpet35",
    "raidpet36", "raidpet37", "raidpet38", "raidpet39", "raidpet40",
];

/// `SMSG_GROUP_LIST`'s first byte for a raid; 0 is a party (vmangos `Group/Group.h:119`).
pub(crate) const GROUPTYPE_RAID: u8 = 1;

/// The subgroup bits (0-2) of a member's flags byte; `0x80` is the assistant bit.
pub(crate) const GROUP_MEMBER_SUBGROUP: u8 = 0x07;

/// What the feed reads to push the group into a VM ([`push_group`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GroupView<'w, 's> {
    // `ChrClasses.dbc` field 16, `UnitHasRelicSlot`'s input, and the form table the creature type
    // reads.
    tables: crate::ui_unit::SnapshotTables<'w>,
    group: Res<'w, GroupState>,
    index: Res<'w, GuidIndex>,
    stores: Query<'w, 's, &'static ObjectStore>,
    self_q: Query<'w, 's, (Entity, &'static Guid, &'static ObjectStore), With<SelfPlayer>>,
    factions: Option<Res<'w, crate::target::Factions>>,
    names: pets::Names<'w>,
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    // The area under us, through the accessor `crate::area` uses: `terrain_stream::CurrentArea`
    // would cross the world-API wall (`tests/world_api_wall.rs`).
    here: benilla_world::world_point::WorldPoint<'w, 's>,
    map_catalog: Option<Res<'w, benilla_assets::MapCatalogRes>>,
    // The guild cache, every held member's guild leg.
    guild: Res<'w, crate::ui_guild::GuildState>,
}

/// Push the roster and the party unit snapshots into the VM and fire the party events on their
/// edges. A member's unit state is their live descriptor when streamed, else their roster record.
pub(crate) fn feed_party(
    view: GroupView,
    mut hosted_vm: crate::script_calls::HostedVm,
    changed_stores: Query<(), Changed<ObjectStore>>,
    mut removed_stores: RemovedComponents<ObjectStore>,
    mut fed: Local<crate::ui_script::VmMemo<FedParty>>,
    chat: Res<crate::ui_chat::ChatLog>,
    seeded: Option<Res<ReloadSeed>>,
    mut sink: crate::ui_action::MessageSink,
) {
    let GroupView {
        tables,
        group,
        index,
        stores,
        self_q,
        factions,
        names,
        areas,
        here,
        map_catalog,
        guild,
    } = &view;
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        let (fed, vm_reset) = fed.get_reset(script);
        // The ready-check summary lines the timeout tick composed, pushed as system chat ahead of
        // the gate: the tick runs on the frame clock, which the gate does not watch.
        for line in script.take_ready_check_lines() {
            chat.push_event(crate::ui_chat::ChatEvent::text_only(
                crate::ui_chat::ChatEventKind::System,
                line,
            ));
        }
        let types = tables.types(names);
        // The gate also opens on a despawn, which `Changed` misses. Solo, it almost always stays
        // shut.
        let names_moved = fed.names_generation.moved(names.generation());
        let guild_moved = fed.guild_generation.moved(guild.identity_generation());
        let area_moved = fed.area.moved(here.area().map_or(u64::MAX, u64::from));
        let group_changed = group.is_changed();
        let index_changed = index.is_changed();
        let look = pets::Lookup::new(
            index,
            stores,
            names,
            types,
            tables.catalog(),
            tables.study(),
        );
        // Only the stores the merged view reads: a crowd's other stores change every frame. The
        // members' pets are the group's too, `partypetN` and `raidpetN` reading their descriptors.
        let stores_changed = self_q
            .iter()
            .next()
            .is_some_and(|(e, _, _)| changed_stores.get(e).is_ok())
            || group.members.iter().any(|m| {
                index
                    .0
                    .get(&m.guid)
                    .is_some_and(|&e| changed_stores.get(e).is_ok())
            })
            || pets::store_changed(
                &look,
                group,
                self_q.iter().next().map(|(_, g, store)| (g.0, store)),
                &changed_stores,
            );
        let stores_removed = !removed_stores.is_empty();
        let factions_changed = factions.as_ref().is_some_and(|r| r.is_changed());
        let areas_changed = areas.as_ref().is_some_and(|r| r.is_changed());
        let maps_changed = map_catalog.as_ref().is_some_and(|r| r.is_changed());
        gate::trace(
            "feed_party",
            &[
                ("vm_reset", vm_reset),
                ("names", names_moved),
                ("guild", guild_moved),
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
                || guild_moved
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
        push_group(&view, script, fed, &gate, vm_reset);
        let self_guid = self_q.iter().next().map(|(_, g, _)| g.0);

        // ── The group events ────────────────────────────────────────────────────
        //
        // `PARTY_MEMBERS_CHANGED`, `PARTY_LEADER_CHANGED`, `PARTY_LOOT_METHOD_CHANGED`,
        // `RAID_ROSTER_UPDATE` and `RAID_TARGET_UPDATE`, each as its handler signalled it, in
        // packet order and once ([`GroupState::signals`]), after the snapshots above, which their
        // handlers read. Nothing gates one on a difference: a list that changes nothing still
        // signals, and a stats packet signals none. `RAID_ROSTER_UPDATE`'s fourth site, the world
        // entry's fill of a zero row (`0x4ba1a2`), is not built: our row holds our guid
        // ([`GroupState::own_raid_row`]). A world entry's fresh VM heard none of them and is told
        // [`catch_up`] instead; a reload's seeded VM only what came after its seed.
        let seed = seeded
            .as_ref()
            .and_then(|s| s.0)
            .filter(|&(session, _)| session == script.session());
        let owed = if let (true, Some((_, cursor))) = (vm_reset, seed) {
            group.signals.since(cursor).to_vec()
        } else if vm_reset {
            catch_up(group)
        } else {
            group.signals.since(fed.signals).to_vec()
        };
        fed.signals = group.signals.end();
        if !owed.is_empty() {
            gate.audit("feed_party", "a group signal");
        }
        for signal in owed {
            let (signalled, args) = match signal {
                GroupSignal::MembersChanged => ("PARTY_MEMBERS_CHANGED", vec![]),
                GroupSignal::LeaderChanged => ("PARTY_LEADER_CHANGED", vec![]),
                GroupSignal::LootMethodChanged => ("PARTY_LOOT_METHOD_CHANGED", vec![]),
                GroupSignal::RaidRosterUpdate => ("RAID_ROSTER_UPDATE", vec![]),
                GroupSignal::RaidTargetUpdate => ("RAID_TARGET_UPDATE", vec![]),
                // `0x5e6730` signals the event (`0x5e6747`), then shows `ERR_INVITED_TO_GROUP_S`
                // (message `0x3c`, `0x5e6752`), so the line follows the popup's handlers.
                GroupSignal::InviteRequest(inviter) => {
                    script.fire_event(
                        "PARTY_INVITE_REQUEST",
                        vec![ScriptValue::Str(inviter.clone())],
                    );
                    if let Some(line) = crate::ui_action::keyed_line_s(
                        script,
                        "ERR_INVITED_TO_GROUP_S",
                        &[&inviter],
                    ) {
                        crate::ui_action::show_now(script, &mut sink, "ui_party", line);
                    }
                    continue;
                }
            };
            script.fire_event(signalled, args);
        }

        // ── READY_CHECK ─────────────────────────────────────────────────────────
        //
        // Fired by the reference's open handler `0x4ba360` (at `0x4ba53a`), once it has armed the
        // 30 s deadline (`0xb713f4`, `0x4ba535`), so a handler's `ConfirmReadyCheck` finds the
        // check running. `UIParent.lua:571` shows the popup on it, and UIParent's OnUpdate ticks
        // the deadline through `CheckReadyCheckTime` (`UIParent.xml:24`): no clock here.
        //
        // Each request arms the engine's side first (the leader's close, a member's 30 s
        // deadline); the answer log then replays from this memo's cursor.
        if group.ready_check_requests != fed.ready_check_requests {
            fed.ready_check_requests = group.ready_check_requests;
            fed.answers_forwarded = 0;
            if !vm_reset {
                script.ready_check_request(Some(group.leader) == self_guid);
            }
        }
        if group.ready_check != fed.ready_check {
            gate.audit("feed_party", "the ready-check edge");
            fed.ready_check = group.ready_check;
            // Not on the first look after a VM reset, which would pop a check that has ended.
            if !vm_reset {
                script.fire_event("READY_CHECK", vec![]);
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
    })
}

/// Push the roster, the party and raid unit snapshots and their pets' into `script`, against what
/// `fed` says it last pushed: the feed's pass, and a reload's seed ([`seed_reloaded_vm`]).
fn push_group(
    view: &GroupView,
    script: &mut UiScript,
    fed: &mut FedParty,
    gate: &gate::Gate,
    vm_reset: bool,
) {
    let GroupView {
        tables,
        group,
        index,
        stores,
        self_q,
        factions,
        names,
        areas,
        here,
        map_catalog: _,
        guild,
    } = view;
    let chr = tables.unit_names();
    let types = tables.types(names);
    let look = pets::Lookup::new(
        index,
        stores,
        names,
        types,
        tables.catalog(),
        tables.study(),
    );
    let self_pair = self_q.iter().next();
    let self_guid = self_pair.map(|(_, g, _)| g.0);
    // The party's PvP faction group is our own: a 1.12 party is one faction, and an unstreamed
    // member has no descriptor to read one from.
    let own_group = self_pair
        .and_then(|(_, _, store)| crate::ui_unit::faction_group(store, factions.as_deref()));

    let slots: Vec<&GroupMemberEntry> = group.party_slots().collect();

    // The master looter as a party index, 0 for us. It covers our own subgroup's slots only,
    // as the leader index does; `GetRaidRosterInfo` carries every subgroup.
    let members: Vec<PartyMemberInfo> = slots
        .iter()
        .map(|m| PartyMemberInfo {
            name: m.name.clone(),
            guid: m.guid,
        })
        .collect();
    // `GetLootMethod` and `GetLootThreshold` read the stored cells, with no group test.
    let loot = group.loot_cells();
    // Methods 0 to 4 through the table `0x4e9280`, `ERROR!` above (`0x4e91ef`).
    let loot_method = match loot.method {
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
    let master_looter = (loot.master != 0)
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
    let (loot_method, loot_threshold) = (loot_method.to_string(), u32::from(loot.threshold));
    // The raid roster, the one list `GetNumRaidMembers`, `GetRaidRosterInfo` and `UnitInRaid`
    // read.
    let me = self_pair.map(|(_, g, store)| RaidSelf {
        guid: g.0,
        level: store.0.unit_level().unwrap_or(0),
        area: here.area(),
        dead: store.0.unit_is_dead(),
        class: store.0.unit_class(),
    });
    let zone_name = |area: u32| {
        let areas = areas.as_ref()?;
        // A member's wire zone is already a zone; our own area is the finest leaf, so walk it
        // up. `top_zone` leaves a zone as it is, so one resolver serves both.
        areas
            .0
            .name(areas.0.top_zone(area).unwrap_or(area))
            .map(str::to_string)
    };
    let held_store = |guid: u64| index.0.get(&guid).and_then(|e| stores.get(*e).ok());
    let raid = raid_roster(group, me.as_ref(), &held_store, names, chr, &zone_name);

    let fresh = PartyState {
        members,
        leader_index: group.leader_index,
        // `UnitIsPartyLeader` compares a resolved token's guid with it, which an index cannot
        // do for an unstreamed member; zero when ungrouped (see `PartyState::leader_guid`).
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

    let area_names = areas.as_deref().map(|a| &a.0);
    for (i, token) in PARTY_TOKENS.iter().enumerate() {
        let member = slots.get(i);
        let snap = member.map(|m| {
            let mut s = member_unit_state(
                m,
                group.stats.get(&m.guid),
                index.0.get(&m.guid).and_then(|e| stores.get(*e).ok()),
                group,
                own_group.clone(),
                names,
                guild,
                chr,
                types,
            );
            s.tooltip.party = party_record(group, m.guid, area_names);
            s
        });
        if fed.units[i] != snap {
            gate.audit("feed_party", "a party-token snapshot");
            script.set_unit(token, snap.clone());
            fed.units[i] = snap;
        }
        // ── PARTY_MEMBER_ENABLE / PARTY_MEMBER_DISABLE ──────────────────────
        //
        // Fired as a slot's object enters (`0x4e86b6`) or leaves (`0x5e9b78`) the object
        // manager, with the 1-based slot as a number (`%d`, `esi+1` at `0x4e869d`). Stock
        // FrameXML ignores them (`PartyMemberFrame.lua:145` is commented out).
        let presence = member.map_or((0, false), |m| (m.guid, index.0.contains_key(&m.guid)));
        if fed.presence[i] != presence {
            gate.audit("feed_party", "a party-slot presence edge");
            // Only the same member's object crossing is an activation, and not on the first
            // look after a VM reset, which would announce an arrival that never happened.
            if !vm_reset && presence.0 != 0 && fed.presence[i].0 == presence.0 {
                let event = if presence.1 {
                    "PARTY_MEMBER_ENABLE"
                } else {
                    "PARTY_MEMBER_DISABLE"
                };
                script.fire_event(event, vec![ScriptValue::Int(i as i64 + 1)]);
            }
            fed.presence[i] = presence;
        }
    }

    // ── partypet1..partypet4 ────────────────────────────────────────────────
    //
    // Each slot's pet, off its descriptor or its record, after the members' own tokens and
    // before the roster event whose handler asks `UnitExists("partypetN")`
    // (`PartyMemberFrame.lua:73`).
    for (i, token) in PARTY_PET_TOKENS.iter().enumerate() {
        let owner = slots.get(i).map_or(0, |m| m.guid);
        let now = pets::member_pet(&look, group, owner, look.store(owner), Roster::Party);
        pets::feed_token(script, gate, &mut fed.pets.party[i], token, now);
    }

    // ── raid1..raid40 ──────────────────────────────────────────────────────
    //
    // Token N is `GetRaidRosterInfo` row N, both in `raid_row_guids`' order. Our own row comes
    // off our descriptor: the wire list never contains us.
    let raid_guids = raid_row_guids(group);
    fed.raid_units.resize(RAID_TOKENS.len(), None);
    for (i, token) in RAID_TOKENS.iter().enumerate() {
        let snap = raid_guids.get(i).and_then(|guid| {
            if Some(*guid) == group.own_raid_row {
                let name = names
                    .resolve_unit(*guid, None, Ask::Read)
                    .map(str::to_string);
                let mut s = match self_pair.filter(|(_, g, _)| g.0 == *guid) {
                    Some((_, _, store)) => {
                        let mut s = crate::ui_unit::snapshot(store, *guid, name, 0, chr, types);
                        s.guild = crate::ui_guild::cached_unit_guild(&store.0, guild);
                        s
                    }
                    // No object of ours, as across a worldport: the row's guid still names us,
                    // through the roster (`0x491900`), the name cache (`0x5171eb`) and the row
                    // the list wrote online (`0x5e6e94`).
                    None => {
                        let mut s = UnitState {
                            exists: true,
                            guid: *guid,
                            name,
                            is_connected: true,
                            ..Default::default()
                        };
                        crate::ui_unit::traits_from_name_cache(&mut s, names, *guid, chr);
                        s
                    }
                };
                s.is_player = true;
                s.raid_target = group.raid_target_index(*guid);
                s.faction_group = own_group.clone();
                Some(s)
            } else {
                let m = group.members.iter().find(|m| m.guid == *guid)?;
                let mut s = member_unit_state(
                    m,
                    group.stats.get(guid),
                    index.0.get(guid).and_then(|e| stores.get(*e).ok()),
                    group,
                    own_group.clone(),
                    names,
                    guild,
                    chr,
                    types,
                );
                s.tooltip.party = party_record(group, *guid, area_names);
                Some(s)
            }
        });
        if fed.raid_units[i] != snap {
            gate.audit("feed_party", "a raid-token snapshot");
            script.set_unit(token, snap.clone());
            fed.raid_units[i] = snap;
        }
    }

    // ── raidpet1..raidpet40 ────────────────────────────────────────────────
    //
    // Row N's pet; our own row's is `"pet"`'s unit.
    fed.pets
        .raid
        .resize_with(RAID_PET_TOKENS.len(), Default::default);
    for (i, token) in RAID_PET_TOKENS.iter().enumerate() {
        let owner = raid_guids.get(i).copied().unwrap_or(0);
        let ours = Some(owner) == self_guid;
        let held = if ours {
            self_pair.map(|(_, _, store)| store)
        } else {
            look.store(owner)
        };
        let now = pets::member_pet(&look, group, owner, held, Roster::Raid);
        pets::feed_token(script, gate, &mut fed.pets.raid[i], token, now);
    }
}

/// What our own raid row reads off our object: `SMSG_GROUP_LIST` never lists the recipient, and
/// the row itself is [`GroupState::own_raid_row`].
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

/// The raid rows as guids ([`GroupState::raid_rows`]), the order [`raid_roster`] builds, which
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
    if !group.raid_held() {
        return None;
    }
    match group.members.get(row) {
        Some(m) => Some(m.flags & 0x7f),
        None => (row == group.members.len()).then_some(group.own_flags & 0x7f),
    }
}

/// The name the subgroup verbs send: the cache's record, looked up with no callback
/// (`0x4bba5e`, `0x4bbbc4`, `0x4bbbe5`), so a name still to come sends nothing.
pub(super) fn cached_player_name(names: &NameCache, guid: u64) -> Option<String> {
    names.resolve_player(guid, Ask::Read).map(str::to_string)
}

/// `GetRaidRosterInfo`'s array (`0x4bb560`) in [`raid_row_guids`]' order: empty while no roster
/// is held, so `GetNumRaidMembers()` answers 0 in a party and in a raid the list named nobody
/// else in. The reference's array holds the local player and the wire's list does not, so our
/// row comes from [`RaidSelf`]; with no object of ours, as across a worldport, the row stays,
/// online as the list wrote it (`0x5e6e94`), its level, zone and class not derived.
///
/// A row whose unit the object manager holds (`0x4bb64b`, `held`) reads it: online (`0x4bb777`),
/// its descriptor's level (`0x4bb65d`), the zone under us (`0x4bb739`, `[0xb4e314]`) and dead for
/// health 0 or below (`0x4bb796`). A row with no object reads its record's online bit
/// (`[row+0x18] & 1`), which gates the level (`0x4bb668`-`0x4bb682`), the zone (`0x4bb6e5`, the
/// binding's `PLAYER_OFFLINE`) and dead, which needs the record's dead bit too (`0x4bb70a`-
/// `0x4bb717`). The roster status byte reaches none of them: a list writes only the record's bit 0
/// (`0x4ba947`-`0x4ba960`).
fn raid_roster<'a>(
    group: &GroupState,
    me: Option<&RaidSelf>,
    held: &dyn Fn(u64) -> Option<&'a ObjectStore>,
    names: &NameCache,
    unit_names: crate::ui_unit::UnitNames<'_>,
    zone_name: &dyn Fn(u32) -> Option<String>,
) -> Vec<RaidMemberInfo> {
    if !group.raid_held() {
        return Vec::new();
    }
    // Rank 2 leader, 1 assistant, 0 member, as the reference's roster rebuild writes it
    // (`0x4ba90c`, `0x4ba920`, `0x4ba929`) against the leader of that rebuild, and the binding
    // returns it unchanged.
    let rank_of = |guid: u64, flags: u8| {
        if guid == group.raid_leader {
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
        let class = class_byte.and_then(|c| unit_names.class(c));
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
    let our_zone = || me.and_then(|me| me.area).and_then(zone_name);
    let mut roster = Vec::with_capacity(group.members.len() + 1);
    for m in &group.members {
        let class = names.player_traits(m.guid).map(|(_, class, _)| class);
        let (level, zone, online, ninth) = match held(m.guid) {
            Some(store) => (
                store.0.unit_level().unwrap_or(0),
                our_zone(),
                true,
                store.0.unit_health().unwrap_or(0) as i32 <= 0,
            ),
            None => {
                let record = group.stats.get(&m.guid);
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
        roster.push(row(
            m.guid,
            m.name.clone(),
            m.flags,
            level,
            zone,
            online,
            ninth,
            class,
        ));
    }
    if let Some(guid) = group.own_raid_row {
        let me = me.filter(|me| me.guid == guid);
        roster.push(row(
            guid,
            // An unresolved name gets the binding's miss tuple, not a half-filled row (`0x4bb5f8`).
            names
                .resolve_unit(guid, None, Ask::Read)
                .unwrap_or_default()
                .to_string(),
            group.own_flags,
            me.map_or(0, |me| me.level),
            me.and(our_zone()),
            true,
            me.is_some_and(|me| me.dead),
            me.and_then(|me| me.class),
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
    // The name cache, whose record answers race, class and sex for a member with no object.
    names: &NameCache,
    // The guild cache, which a held member's guild leg reads without asking.
    guild: &crate::ui_guild::GuildState,
    // `ChrRaces.dbc` and `ChrClasses.dbc`: the names and the relic slot, which only the live leg
    // has a class byte to key by, so an unstreamed paladin reads no relic slot.
    classes: crate::ui_unit::UnitNames<'_>,
    // The creature type resolves from the live descriptor alone, so an unstreamed member has none.
    types: CreatureTypeSources<'_>,
) -> UnitState {
    let mut s = match store {
        Some(store) => {
            let mut s =
                crate::ui_unit::snapshot(store, m.guid, Some(m.name.clone()), 0, classes, types);
            // `GetGuildInfo` reads any token's object (`0x4c9382`), its guild from the cache.
            s.guild = crate::ui_guild::cached_unit_guild(&store.0, guild);
            s
        }
        // Unstreamed: the roster record, snapshotted from the descriptor at despawn (`0x5f0880`),
        // seated at 1/1 for a member never seen (`0x4e82d0`) and patched by the wire. The
        // reference's getters read the descriptor, then the party record (`0x496400`), then the
        // pet record (`0x496420`); a `partyN` token names a player, so only `partypetN` reaches
        // that last leg ([`pets`]).
        None => UnitState {
            exists: true,
            guid: m.guid,
            name: Some(m.name.clone()),
            health: stats.and_then(|s| s.cur_hp).map_or(0, u32::from),
            max_health: stats.and_then(|s| s.max_hp).map_or(0, u32::from),
            level: stats.and_then(|s| s.level).map_or(0, u32::from),
            power_type: stats.map_or(0, PartyMemberStatsInfo::shown_power_type),
            // Divided as on the live leg (`UnitMana`'s record path, `0x517744`-`0x51775e`).
            power: stats.map_or(0, PartyMemberStatsInfo::shown_power),
            max_power: stats.map_or(0, PartyMemberStatsInfo::shown_max_power),
            // Connected is the record's own online bit (`+0x08 & 1`, `0x517dd3`), which each list
            // rewrites from its row's status (`0x4e836e`-`0x4e837b`) and a stats packet's status
            // moves alone, so the roster byte is not read. A held member never gets here, its 1 is
            // `snapshot`'s (`0x517daf`); a member with no record has no bit to read.
            is_connected: stats.is_some_and(PartyMemberStatsInfo::is_online),
            // The record's other status bits, the no-object leg of each predicate: `UnitIsDead`
            // `+0x08 & 4` (`0x517b5d`), `UnitIsGhost` `& 8` (`0x517c32`), `UnitIsPVP` `& 2`
            // (`0x516502`), `UnitIsPVPFreeForAll` `& 0x10` (`0x5165f1`). A list writes only bit 0
            // of the record (`0x4e836e`-`0x4e837b`), so the roster byte reaches none of them; the
            // deactivate snapshot and the stats packet's status do. 1.12 has no AFK or DND
            // predicate to feed.
            dead: record_bit(stats, member_status::DEAD),
            ghost: record_bit(stats, member_status::GHOST),
            pvp: record_bit(stats, member_status::PVP),
            is_pvp_ffa: record_bit(stats, member_status::PVP_FFA),
            ..Default::default()
        },
    };
    if store.is_none() {
        crate::ui_unit::traits_from_name_cache(&mut s, names, m.guid, classes);
    }
    s.is_player = true;
    s.raid_target = group.raid_target_index(m.guid);
    s.faction_group = own_group;
    // A held member's predicates read its descriptor alone (`0x517b16`, `0x517bef`, `0x5164d1`,
    // `0x51659f`): `snapshot`'s, the roster byte never.
    s
}

/// The group record the unit tooltip finds by guid (`0x496400`): the party's (`0x4e8820`, our own
/// party slots), else the raid roster's (`0x4bb0f0`, its 40 entries); its online bit and the name of
/// its area (`0x67e460`, with no parent zone). The roster's records are one per member either way.
fn party_record(
    group: &GroupState,
    guid: u64,
    areas: Option<&benilla_formats::AreaTableCatalog>,
) -> Option<benilla_ui::script::PartyRecord> {
    let in_party = group.party_slots().any(|m| m.guid == guid);
    if !in_party && !group.raid_rows().any(|g| g == guid) {
        return None;
    }
    let record = group.stats.get(&guid)?;
    Some(benilla_ui::script::PartyRecord {
        online: record.is_online(),
        area: record
            .zone
            .and_then(|zone| areas?.name(u32::from(zone)))
            .map(str::to_string),
    })
}

/// A status bit of a member's record (`+0x08`), false with no record.
fn record_bit(stats: Option<&PartyMemberStatsInfo>, bit: u8) -> bool {
    stats.is_some_and(|s| s.status.unwrap_or(0) & bit != 0)
}

/// Every frame: age the signal log, and answer what the party calls left
/// ([`super::calls::GroupEdits`]): the sandbox's group-changing calls, echoed onto the mirror as
/// the server would answer them.
pub(crate) fn drain_party(
    mut group: ResMut<GroupState>,
    mut edits: ResMut<super::calls::GroupEdits>,
    self_q: Query<&Guid, With<SelfPlayer>>,
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
        group.own_raid_row = (raid && !group.members.is_empty()).then(|| self_guid.unwrap_or(0));
        group.raid_leader = if raid { group.leader } else { 0 };
        group.store_leader_index();
        group.listed = true;
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
    use super::super::pets::tests::{app, app_with, flag, frame, member, party, stream, ME};
    use super::*;
    use benilla_ui::script::UiScript;

    mod token_verb_tests;

    #[test]
    fn an_out_of_range_member_reads_the_record_and_divides_its_rage() {
        let m = GroupMemberEntry {
            name: "Brisca".into(),
            guid: 0x1234,
            status: member_status::ONLINE,
            flags: 0,
        };
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
        let s = member_unit_state(
            &m,
            Some(&record),
            None,
            &GroupState::default(),
            None,
            &NameCache::default(),
            &crate::ui_guild::GuildState::default(),
            Default::default(),
            Default::default(),
        );
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
        let bare = member_unit_state(
            &m,
            None,
            None,
            &GroupState::default(),
            None,
            &NameCache::default(),
            &crate::ui_guild::GuildState::default(),
            Default::default(),
            Default::default(),
        );
        assert_eq!((bare.health, bare.max_health, bare.power), (0, 0, 0));
        assert!(bare.exists);
    }

    /// A member out of view answers race, class and sex from the name cache record of its guid,
    /// read with no callback (`UnitRace` `0x5182bc`-`0x5182d9`, `+0x138`; `UnitClass` `0x518429`,
    /// `+0x140`; `UnitSex` `0x517f80`, `+0x13c` through `{2, 3, 1, 6}` at `0x808be4`); a miss
    /// leaves them unset, nil and `UnitSex` 2 (`0x517f95`). A held member reads its descriptor.
    #[test]
    fn an_out_of_view_member_reads_race_class_and_sex_from_the_name_cache() {
        let m = GroupMemberEntry {
            name: "Brisca".into(),
            guid: 0x1234,
            status: member_status::ONLINE,
            flags: 0,
        };
        let tables = crate::ui_unit::TestNames::english();
        let mut names = NameCache::default();
        let state = |names: &NameCache, store: Option<&ObjectStore>| {
            member_unit_state(
                &m,
                None,
                store,
                &GroupState::default(),
                None,
                names,
                &crate::ui_guild::GuildState::default(),
                tables.names(),
                Default::default(),
            )
        };
        let s = state(&names, None);
        assert_eq!((s.race, s.class, s.sex), (None, None, 0), "no record");
        // Night Elf (4) Priest (5), female (1).
        names.insert_player(0x1234, "Brisca".into(), Some((4, 5, 1)));
        let s = state(&names, None);
        assert_eq!(
            (s.race.as_deref(), s.race_file.as_deref()),
            (Some("Night Elf"), Some("NightElf"))
        );
        assert_eq!(
            (s.class.as_deref(), s.class_file.as_deref()),
            (Some("Priest"), Some("PRIEST"))
        );
        assert_eq!(s.sex, 3, "female");
        // Held, a Human (1) Warrior (1), male: the descriptor's, not the cache's.
        const BYTES_0: u16 = 36;
        let held = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
            BYTES_0,
            1 | (1 << 8),
        )]));
        let s = state(&names, Some(&held));
        assert_eq!(
            (s.race.as_deref(), s.class.as_deref(), s.sex),
            (Some("Human"), Some("Warrior"), 2)
        );
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
        let online = |guid| party_record(&group, guid, None).map(|r| r.online);
        assert_eq!(online(member(1)), Some(true), "the party's record");
        assert_eq!(online(member(2)), Some(false), "the raid roster's record");
        assert_eq!(online(member(3)), None, "off the roster");
    }

    /// vmangos re-sends a member's status on join, login and the AFK, DND, PvP, FFA and ghost
    /// toggles, never on a death, so the roster byte can say alive while the record says dead.
    #[test]
    fn an_out_of_range_members_dead_ghost_and_pvp_come_off_the_record() {
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
        let s = member_unit_state(
            &m,
            Some(&dead),
            None,
            &GroupState::default(),
            None,
            &NameCache::default(),
            &crate::ui_guild::GuildState::default(),
            Default::default(),
            Default::default(),
        );
        assert!(
            s.dead,
            "the record says dead even though the roster echo does not"
        );
        assert!(!s.ghost);

        let ghost = PartyMemberStatsInfo {
            status: Some(member_status::ONLINE | member_status::GHOST),
            ..PartyMemberStatsInfo::default()
        };
        let s = member_unit_state(
            &m,
            Some(&ghost),
            None,
            &GroupState::default(),
            None,
            &NameCache::default(),
            &crate::ui_guild::GuildState::default(),
            Default::default(),
            Default::default(),
        );
        assert!(s.ghost);
        assert!(!s.dead, "a released ghost is not `dead` — only a ghost");

        // A list writes only bit 0 of the record (`0x4e836e`-`0x4e837b`): a roster byte carrying
        // the other bits reaches no predicate.
        let stale = GroupMemberEntry {
            status: member_status::ONLINE
                | member_status::DEAD
                | member_status::GHOST
                | member_status::PVP
                | member_status::PVP_FFA,
            ..m.clone()
        };
        let s = member_unit_state(
            &stale,
            Some(&PartyMemberStatsInfo::placeholder(true)),
            None,
            &GroupState::default(),
            None,
            &NameCache::default(),
            &crate::ui_guild::GuildState::default(),
            Default::default(),
            Default::default(),
        );
        assert_eq!(
            (s.dead, s.ghost, s.pvp, s.is_pvp_ffa),
            (false, false, false, false)
        );

        // The record's PvP (`& 2`, `0x516502`) and free-for-all (`& 0x10`, `0x5165f1`) bits.
        let flagged = PartyMemberStatsInfo {
            status: Some(member_status::ONLINE | member_status::PVP | member_status::PVP_FFA),
            ..PartyMemberStatsInfo::default()
        };
        let s = member_unit_state(
            &m,
            Some(&flagged),
            None,
            &GroupState::default(),
            None,
            &NameCache::default(),
            &crate::ui_guild::GuildState::default(),
            Default::default(),
            Default::default(),
        );
        assert!(s.pvp && s.is_pvp_ffa);
    }

    /// The reported shape, through the feed: a roster that lags a held member's descriptor moves
    /// none of the four predicates, which read the object alone while it is held (`0x517b16`,
    /// `0x517bef`, `0x5164d1`, `0x51659f`), on `partyN` and `raidN` alike.
    #[test]
    fn a_held_members_dead_ghost_and_pvp_come_off_the_descriptor_alone() {
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

            // Dead, a ghost, PvP and free-for-all on the descriptor; the roster says none.
            app.world_mut().entity_mut(held).insert(ObjectStore(
                benilla_protocol::messages::ObjectFields::from_pairs(&[
                    (HEALTH, 0),
                    (MAXHEALTH, 900),
                    (UNIT_FLAGS, 0x1000),
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
                app.world_mut()
                    .non_send_resource::<UiScript>()
                    .eval::<(Option<String>, Option<String>, i64)>(&format!(
                        "return GetGuildInfo({token:?})"
                    ))
                    .map(|(name, _, rank)| (name, rank))
                    .unwrap()
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

    /// A VM minted after the lists landed was told of none of them: one catch-up for a group, not
    /// the history, and silence for a player who never had one.
    #[test]
    fn a_fresh_vm_answers_the_lists_that_landed_before_it_once() {
        let mut app = app();
        watch_members_changed(&mut app);
        party(&mut app, 2, true);
        resend(&mut app, &[OFF, ON], ME, None);
        resend(&mut app, &[OFF, OFF], ME, None);
        assert_eq!(changed(&mut app), 1);
        assert_eq!(changed(&mut app), 0);

        let mut solo = self::app();
        watch_members_changed(&mut solo);
        assert_eq!(changed(&mut solo), 0, "no list, no event");
    }

    /// A fresh VM is told one of each event the state carries, in a list's order, and never the
    /// history: a group left this session still owes its members change, and nothing else.
    #[test]
    fn a_fresh_vm_is_told_what_the_state_carries_in_a_lists_order() {
        use GroupSignal::*;
        let mut g = GroupState::default();
        assert_eq!(catch_up(&g), [], "no list this session");
        let loot = GroupLootInfo {
            method: 2,
            master: member(1),
            threshold: 3,
        };
        let raid = vec![GroupMemberEntry {
            name: "M1".into(),
            guid: member(1),
            status: ON,
            flags: 1,
        }];
        g.apply_list(GROUPTYPE_RAID, 0, raid, ME, Some(loot), Some(ME));
        g.apply_raid_target_update(7, member(1));
        assert_eq!(
            catch_up(&g),
            [
                MembersChanged,
                LeaderChanged,
                LootMethodChanged,
                RaidRosterUpdate,
                RaidTargetUpdate
            ]
        );
        g.apply_list(0, 0, Vec::new(), 0, None, Some(ME));
        assert_eq!(catch_up(&g), [MembersChanged], "the group ended");
    }

    /// The feed and the drain in the schedule's order, with a system between them that logs a
    /// signal, as the chat input's `/partytest` can: the drain's age keeps it, the next frame's
    /// feed fires it once, and the ages after drop it.
    #[test]
    fn a_signal_logged_between_the_feed_and_the_drain_fires_on_the_next_frame_once() {
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
        app.add_systems(Update, (mark.after(feed_party), drain_party.after(mark)));
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
        assert_eq!(marks(&mut app), 0, "logged after this frame's feed");
        assert_eq!(marks(&mut app), 1, "the next frame's feed");
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
        g.signals.take();
        let same = PartyRequest::LootThreshold(3);
        assert!(test_apply_local(&mut g, &same, Some(ME)));
        assert_eq!(
            g.signals.take(),
            [
                MembersChanged,
                LeaderChanged,
                LootMethodChanged,
                RaidTargetUpdate
            ],
            "a threshold it already had, under the leader it already had"
        );
        assert!(test_apply_local(&mut g, &PartyRequest::Leave, Some(ME)));
        assert_eq!(g.signals.take(), [MembersChanged, RaidTargetUpdate]);
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
        assert_eq!(
            bar(&mut app),
            (0.0, 0.0, benilla_ui::color::quant::read(1.0)),
            "the mana colour"
        );
        assert_eq!(
            flag(&mut app, "PartyMemberFrame1PetFrame:IsShown()"),
            Some(1.0)
        );

        // The list alone, as the server sends it: the member's record is not touched.
        resend(&mut app, &[OFF], ME, None);
        frame(&mut app);
        assert_eq!(flag(&mut app, "UnitIsConnected('party1')"), None);
        let grey = benilla_ui::color::quant::read(0.5);
        assert_eq!(bar(&mut app), (grey, grey, grey), "the disconnected grey");
        assert_eq!(flag(&mut app, "PartyMemberFrame1PetFrame:IsShown()"), None);

        resend(&mut app, &[ON], ME, None);
        frame(&mut app);
        assert_eq!(
            bar(&mut app),
            (0.0, 0.0, benilla_ui::color::quant::read(1.0)),
            "back online"
        );
        assert_eq!(
            flag(&mut app, "PartyMemberFrame1PetFrame:IsShown()"),
            Some(1.0)
        );
        let errors = app.world().non_send_resource::<UiScript>().errors();
        assert!(errors.is_empty(), "script errors: {errors:?}");
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
            own_raid_row: Some(0x5E1F),
            leader: 0x5E1F,
            raid_leader: 0x5E1F,
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
        group.own_raid_row = None;
        assert!(raid_roster(&group, Some(&me), &held, &names, korean.names(), &zone).is_empty());
    }

    #[test]
    fn a_raid_row_index_names_the_same_player_the_roster_array_does() {
        let mut group = GroupState {
            group_type: GROUPTYPE_RAID,
            own_raid_row: Some(0x5E1F),
            ..Default::default()
        };
        let wire = |name: &str, guid: u64| GroupMemberEntry {
            name: name.into(),
            guid,
            status: member_status::ONLINE,
            flags: 0,
        };
        group.members = vec![wire("Alice", 0xA11CE), wire("Bob", 0xB0B)];

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
        let subgroups: Vec<Option<u8>> = (0..=4).map(|i| raid_row_subgroup(&group, i)).collect();
        assert_eq!(subgroups, [None, Some(0), Some(2), Some(3), None]);

        // With no roster held all are empty, so a raid verb in a party sends nothing.
        group.own_raid_row = None;
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

        // Echoed to us as leader, our ready check moves the request count, not the popup's.
        let (ticket, requests) = (group.ready_check, group.ready_check_requests);
        assert!(test_apply_local(
            &mut group,
            &PartyRequest::ReadyCheckStart,
            me
        ));
        assert_eq!(
            (group.ready_check, group.ready_check_requests),
            (ticket, requests + 1)
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
            let script = app.world_mut().non_send_resource::<UiScript>();
            let (method, master): (String, Option<f64>) =
                script.eval("return GetLootMethod()").unwrap();
            (
                method,
                master,
                script.eval("return GetLootThreshold()").unwrap(),
            )
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

    /// The drain over a group the feed has pushed, and the commands it sends.
    fn drain_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let mut app = app();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.init_resource::<crate::net::Reputations>()
            .init_resource::<crate::ui_action::UiErrorKeys>();
        app.add_systems(Update, drain_party.after(feed_party));
        (app, rx)
    }

    /// Run `chunk` with a host lent that applies the party calls inside their verbs, as the app's
    /// host does ([`super::super::calls::PartyCalls`]); other calls go nowhere.
    pub(super) fn hosted_run(app: &mut App, chunk: &str) {
        use bevy::ecs::system::SystemState;
        let world = app.world_mut();
        let mut script = world.remove_non_send_resource::<UiScript>().expect("a VM");
        let mut state = SystemState::<super::super::calls::PartyCalls>::new(world);
        let ran = {
            let mut host = crate::script_calls::FnHost(|s: &mut UiScript, call| {
                if let benilla_ui::script::ScriptCall::Party(request) = call {
                    state.get_mut(world).apply(s, request);
                }
            });
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

    /// The token resolver's guids as `ui_unit`'s feed pushes them: us, and the party slots.
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
            .apply_ready_check_request(false);
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
    /// (`PartyMemberFrame.lua:20-24`, `RaidFrame.lua:32`). So the reload's VM is seeded with the
    /// group before its files run ([`seed_reloaded_vm`]) and told nothing after; a list that
    /// lands later signals as ever, and a VM nobody seeded, the world entry's first, still hears
    /// the catch-up.
    #[test]
    fn a_reloaded_vm_reads_the_group_as_it_loads_and_hears_no_group_event() {
        let mut app = app();
        party(&mut app, 2, true);
        frame(&mut app);

        let mut reloaded = UiScript::new().unwrap();
        seed_reloaded_vm(app.world_mut(), &mut reloaded);
        reloaded
            .run(r#"LOADED = GetNumPartyMembers() .. "," .. tostring(UnitName("party2"))"#)
            .unwrap();
        hear_group_events(&reloaded);
        app.insert_non_send_resource(reloaded);
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

        let entry = UiScript::new().unwrap();
        hear_group_events(&entry);
        app.insert_non_send_resource(entry);
        frame(&mut app);
        assert_eq!(
            heard(&mut app),
            ["PARTY_MEMBERS_CHANGED", "PARTY_LEADER_CHANGED"],
            "a VM nobody seeded hears the catch-up"
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
        seed_reloaded_vm(app.world_mut(), &mut script);
        script.set_screen_size(1024.0, 768.0);
        let failures = crate::ui_script::load_default_ui(&script);
        assert!(failures.is_empty(), "load failures: {failures:?}");
        script.run("SEEN = {}").unwrap();
        app.insert_non_send_resource(script);
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
