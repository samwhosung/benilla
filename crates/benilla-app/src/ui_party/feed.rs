//! The party VM feed/drain (decision 0434 phase 2) — the systems half of [`super`]: push the
//! roster + `party1..party4` snapshots into the VM, fire the party events on their edges, and
//! drain the Lua-side [`PartyRequest`] intents into their `CMSG_*` sends. The state + the line
//! composer live in the parent module; this file owns only the per-frame bridge.

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

// ─── The VM feed/drain (decision 0434 phase 2) ───────────────────────────────────────────────────

/// The last-pushed fingerprints the feed fires Era event edges on.
#[derive(Default)]
pub(super) struct FedParty {
    roster: Vec<u64>,
    leader: u64,
    loot: Option<GroupLootInfo>,
    invite: Option<String>,
    units: [Option<UnitState>; 4],
    /// Each `party1..party4` slot's `(member guid, is their object streamed)` — the edge
    /// `PARTY_MEMBER_ENABLE`/`DISABLE` fire on (decision 1640). The **guid rides along** because
    /// the events are an *object's* activation, not a slot's: a slot whose occupant changed is a
    /// roster edge, and firing ENABLE there would announce an arrival nobody made. Guid `0` is an
    /// empty slot.
    presence: [(u64, bool); 4],
    /// The whole roster-level snapshot last pushed — `set_party`'s own diff (1439; it used to
    /// re-push, allocations and all, every frame).
    pushed_party: Option<PartyState>,
    /// The gate's counter memories (1439): the name cache by its landed counter (the resolves
    /// other feeds run per frame poison `is_changed` on it), and the leaf area under us (our
    /// own raid row's zone — a plain value watched as a counter).
    names_generation: gate::Watch,
    area: gate::Watch,
    /// The `raid1..raid40` snapshots, same per-token diff as [`Self::units`] (decision 1549).
    /// A `Vec` rather than a `[_; 40]`: `UnitState` is not `Copy`, and forty of them in a `Local`
    /// that a solo player never fills is worth the one allocation a raid pays.
    raid_units: Vec<Option<UnitState>>,
    /// The raid roster's IDENTITY, which is what `RAID_ROSTER_UPDATE` fires on — see the fire
    /// site for why it is these four fields and not the whole row.
    raid_key: Vec<(u64, u32, u32, bool)>,
    /// The saved-instance list last pushed, and the answer ticket it came in on
    /// ([`GroupState::saved_instances_answers`]).
    saved: Vec<SavedInstanceInfo>,
    saved_answers: u32,
    /// The ready-check ticket last seen ([`GroupState::ready_check`]).
    ready_check: u32,
    /// The request generation last seen ([`GroupState::ready_check_requests`]) and how far into
    /// that request's answer log the engine has been fed (decision 1989).
    ready_check_requests: u32,
    answers_forwarded: usize,
    /// The raid-target icon board last pushed — what `RAID_TARGET_UPDATE` fires on. Eight guids,
    /// so a plain copy rather than the `Vec` diffs above.
    raid_targets: [u64; 8],
}

pub(crate) const PARTY_TOKENS: [&str; 4] = ["party1", "party2", "party3", "party4"];

/// **The saved-instance edge** — a new LIST *or* a new ANSWER (1561).
///
/// Its own named rule because the second half is the one that is easy to lose, and losing it is
/// silent: every other edge in this feed is a diff, this one cannot be. The reference throws its
/// first `UPDATE_INSTANCE_INFO` away (`RaidFrame.hasRaidInfo`) and decides the Raid Info button on
/// the second, so a player with no lockouts — whose list is empty and never changes — has to reach
/// a second answer through the ticket alone. Diff the list only, and their button never dies.
fn saved_instances_moved(saved: &[SavedInstanceInfo], answers: u32, fed: &FedParty) -> bool {
    saved != fed.saved || answers != fed.saved_answers
}

/// `raid1`..`raid40` — the unit tokens the RaidFrame's rows target, tooltip and re-read levels
/// through. Spelled out rather than `format!`ed per push: [`UiScript::set_unit`] wants a `&str`,
/// and a table of forty `&'static str` costs nothing where forty `String`s per roster change
/// would (decision 1549). `MAX_RAID_MEMBERS` is the reference's own 40.
#[rustfmt::skip]
pub(crate) const RAID_TOKENS: [&str; 40] = [
    "raid1", "raid2", "raid3", "raid4", "raid5", "raid6", "raid7", "raid8", "raid9", "raid10",
    "raid11", "raid12", "raid13", "raid14", "raid15", "raid16", "raid17", "raid18", "raid19",
    "raid20", "raid21", "raid22", "raid23", "raid24", "raid25", "raid26", "raid27", "raid28",
    "raid29", "raid30", "raid31", "raid32", "raid33", "raid34", "raid35", "raid36", "raid37",
    "raid38", "raid39", "raid40",
];

/// `GROUPTYPE_RAID` — `SMSG_GROUP_LIST`'s first byte (`0` party, `1` raid; vmangos `Group.h:116`).
pub(crate) const GROUPTYPE_RAID: u8 = 1;

/// The subgroup index in a member's flags byte — bits 0-2 (`GroupMemberEntry::flags`'s own doc;
/// the assistant bit `0x80` is its neighbour, and the `0x7f` mask `party_slots` uses is a
/// different question, "same subgroup AND same assistant state").
pub(crate) const GROUP_MEMBER_SUBGROUP: u8 = 0x07;

/// Push the roster snapshot + the `party1..party4` unit snapshots into the VM and fire the party
/// events on their edges. The per-member unit state is the 0434 §2 **merged view**: a streamed
/// member's live descriptor wins; the `PARTY_MEMBER_STATS` snapshot covers the rest — and the
/// roster status byte overlays both (the descriptor never carries connected/AFK/DND).
pub(super) fn feed_party(
    // `ChrClasses.dbc` field 16 — `UnitHasRelicSlot`'s only input. Absent when the client data
    // failed to load, in which case no class reads as having a relic slot.
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,
    script: Option<NonSendMut<UiScript>>,
    group: Res<GroupState>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    changed_stores: Query<(), Changed<ObjectStore>>,
    mut removed_stores: RemovedComponents<ObjectStore>,
    // The per-field edges (decision 2297), for `fire_transitions`' watch-bridge arms.
    mut edges: MessageReader<FieldChanged>,
    self_q: Query<(Entity, &Guid, &ObjectStore), With<SelfPlayer>>,
    factions: Option<Res<crate::target::Factions>>,
    names: Res<NameCache>,
    areas: Option<Res<crate::area::AreaTableRes>>,
    // The leaf area under us, through the SAME accessor `crate::area`'s zone-text resolver uses.
    // Deliberately not `terrain_stream::CurrentArea` directly: that item is named today only by
    // the instruments, and naming it from a game module would push it across the world-API wall
    // (`tests/world_api_wall.rs`, decision 1164) for a value this already answers.
    here: benilla_world::world_point::WorldPoint,
    // `Map.dbc`'s display names — `SMSG_RAID_INSTANCE_INFO` carries a map id and the Raid Info
    // panel shows a name (decision 1549). `Option` like every other catalog here: an engine-less
    // harness has none, and a lockout then shows its map id, never a blank row.
    map_catalog: Option<Res<benilla_assets::MapCatalogRes>>,
    mut fed: Local<crate::ui_script::VmMemo<FedParty>>,
    mut chat: ResMut<crate::ui_chat::ChatLog>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (fed, vm_reset) = fed.get_reset(&script);
    let edges = FieldEdges::collect(&mut edges);
    // The ready-check summary the timeout tick composed (decision 1989) — a client-composed
    // `CHAT_MSG_SYSTEM` line, pushed the way every other one is. Ahead of the gate: the tick
    // runs on the frame clock, not on anything the gate watches.
    for line in script.take_ready_check_lines() {
        chat.push_event(crate::ui_chat::ChatEvent::text_only(
            crate::ui_chat::ChatEventKind::System,
            line,
        ));
    }
    let chr = classes.as_deref().map(|t| &t.0);
    // The gate (1439): the group state, any member/self descriptor change or DESPAWN (a removed
    // store is invisible to `Changed`), the streamed-guid index the merged view resolves
    // through, the two catalogs, the name cache by its landed counter, and our own leaf area
    // (the raid row's zone). The gate closing needs the party quiet — solo, it almost always is.
    let names_moved = fed.names_generation.moved(names.generation());
    let area_moved = fed.area.moved(here.area().map_or(u64::MAX, u64::from));
    let group_changed = group.is_changed();
    let index_changed = index.is_changed();
    // The members' descriptors and our own — the only stores the merged view below reads. This
    // used to be "any store in the world moved", which in a crowd is true on every frame: the
    // crowd profile of 2225 read this feed re-pushing a 39-member raid to the VM every frame of
    // the pin, 0.47 ms traced, for rows that had not changed.
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
    // The party's PvP faction group (decision 0646 §1): our own. A 1.12 party is always one
    // faction, and a member out of streaming range has no descriptor to resolve one from — so
    // reading it off ourselves is exact for every member, present or not.
    let own_group = self_pair
        .and_then(|(_, _, store)| crate::ui_unit::faction_group(store, factions.as_deref()));

    // The party1..4 slot view: own-subgroup members, packet order (`GroupState::party_slots`,
    // the 0440 byte law) — in a plain party this is simply the roster.
    let slots: Vec<&GroupMemberEntry> = group.party_slots().collect();

    // The roster-level snapshot (leader/master indices on the Lua scale: 0 = the player). These
    // indices are the PARTY view — our own subgroup's four slots — so in a raid a leader in
    // another subgroup still reads as index 0 here; the whole-raid answer is `raid` below, which
    // `GetRaidRosterInfo` reads and which does carry every subgroup.
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
            // `GetLootMethod`'s second return, and it is deliberately narrow: the binding
            // (`0x4e91b0`) pushes 0 when the stored looter guid is our own, else searches ONLY
            // the four party slots (`0x4e81a0`, bounded to 4 even in a raid) and pushes index+1
            // on a hit — and pushes **nil** on a miss, the same value it pushes when there is no
            // master looter at all. So a master looter sitting in another raid subgroup is not
            // expressible here; that is a real, verified asymmetry with `SetLootMethod`, which
            // does accept raid members.
            //
            // The miss arm is what this used to get wrong: it folded an unfound master to 0, and
            // 0 means "the player" — so a raid master looter in another subgroup lit the master
            // crown on OUR portrait (`PlayerFrame_UpdatePartyLeader` shows it on `lootMaster == 0`).
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
    // The raid roster — `GetNumRaidMembers`/`GetRaidRosterInfo`/`UnitInRaid` all read this one
    // list, so the count can never disagree with the array the way it would if we kept both.
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
        // The wire's per-member `zone` is already a top-level zone id; our own `CurrentArea` is
        // the finest MCNK leaf, so it needs the parent walk first. `top_zone` is idempotent on a
        // row that is already a zone, which is what lets one resolver serve both.
        areas
            .0
            .name(areas.0.top_zone(area).unwrap_or(area))
            .map(str::to_string)
    };
    let raid = raid_roster(&group, me.as_ref(), &names, &zone_name);
    // The `RAID_ROSTER_UPDATE` key, taken before the roster moves into the snapshot below (the
    // fire site further down carries why it is these four fields).
    let raid_key: Vec<(u64, u32, u32, bool)> = raid
        .iter()
        .map(|r| (r.guid, r.rank, r.subgroup, r.online))
        .collect();

    let fresh = PartyState {
        members,
        leader_index,
        // The wire's leader GUID verbatim — `UnitIsPartyLeader`'s second leg compares the
        // RESOLVED token against it, so an index cannot stand in (an out-of-range member has no
        // descriptor, and the compare must still answer). Zero when ungrouped, and deliberately
        // not guarded against zero at the comparison — see `PartyState::leader_guid`.
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

    // party1..party4 unit snapshots + their per-field UNIT_* transitions — pushed on diff (1439;
    // an identical snapshot re-pushed is invisible to the VM, so only a change pays the clone).
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
        // ── PARTY_MEMBER_ENABLE / PARTY_MEMBER_DISABLE (decision 1640) ──────────────────────
        //
        // The pair the reference fires from the very hooks decision 1640 built the rest of this
        // arc on: `PARTY_MEMBER_DISABLE` (`0xdd`) at the end of the deactivate virtual
        // `0x5e9aa0`, `PARTY_MEMBER_ENABLE` (`0xdc`) from the activate leg `0x4e85d0(mode 1)` —
        // i.e. exactly this slot's object entering or leaving the object manager, with the
        // **1-based slot index** as the (string) argument.
        //
        // Fired here rather than at the net edge because this is where the VM is, and because the
        // slot number is a party-slot fact, not an object one. **Nothing in 1.12.1's FrameXML
        // reads them** — `PartyMemberFrame_OnEvent`'s two arms are commented out in the shipped
        // Lua — so this is addon-facing fidelity, and it is deliberately not the wire the frame's
        // own repaint rides (that is `UNIT_*` + `PARTY_MEMBERS_CHANGED`, above).
        let presence = member.map_or((0, false), |m| (m.guid, index.0.contains_key(&m.guid)));
        if fed.presence[i] != presence {
            gate.audit("feed_party", "a party-slot presence edge");
            // Only the **same** member's object crossing the boundary is an activation. Not on
            // the first observation after a VM reset either: a fresh VM starting at `false`
            // against a member who has been standing there all along would announce an arrival
            // that did not happen (the `READY_CHECK` rule, one system down).
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

    // ── raid1..raid40 (decision 1549) ──────────────────────────────────────────────────────
    //
    // The RaidFrame's rows carry a `raid<N>` token and use it for everything a party row uses a
    // `party<N>` token for: left-click targets it, the tooltip reads it, and the reference's own
    // `UNIT_LEVEL`/`UNIT_HEALTH` handlers re-read the row it names. The token index is the
    // GetRaidRosterInfo row index — `raid_row_guids`' order, the one place that is decided — so
    // `raid7` and `GetRaidRosterInfo(7)` can never be two different people.
    //
    // Row 1 is US, and our snapshot comes off our own descriptor rather than the roster: the wire
    // list never contains the recipient, so there is no `GroupMemberEntry` to build it from.
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
                // A raid member is a same-faction friendly player, exactly as `member_unit_state`
                // holds for the others — the popup's cooperate gates read it.
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

    // The party events, on edges.
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
    // ── RAID_ROSTER_UPDATE (decision 1549) ──────────────────────────────────────────────────
    //
    // Fired on the roster's IDENTITY moving — who is in it, in what order, at what rank, in which
    // subgroup, online or not — and NOT on the whole row. That split is the reference's own, read
    // off its consumers rather than guessed: `RaidGroupFrame_OnEvent` re-reads a member's LEVEL
    // from `UNIT_LEVEL` and their dead colour from `UNIT_HEALTH`, so those two fields are
    // expected to move *without* this event, and firing on them would make the whole raid pane
    // repaint every time somebody took damage.
    //
    // The exact fire SITE is not pinned to bytes. The reference has the event (FrameScript id
    // `0x1f3`, its name slot `0xbe1964`) and the raid-roster TU that owns the neighbouring lines
    // (`0x4ba220`/`0x4ba550`), but which of that TU's arms signal it is not identified. What IS
    // constrained: `SMSG_GROUP_LIST` is the only packet that can move any of these four fields,
    // and the reference's RaidFrame repaints on this event and on `PARTY_MEMBERS_CHANGED` alike
    // — so a client that fires it on every identity change of the roster cannot show a stale
    // pane, whatever the extra arms turn out to be. INFERRED, and named as such rather than left
    // to be discovered from a bug.
    if raid_key != fed.raid_key {
        gate.audit("feed_party", "the raid-roster edge");
        fed.raid_key = raid_key;
        script.fire_event("RAID_ROSTER_UPDATE", vec![]);
    }

    // ── RAID_TARGET_UPDATE ──────────────────────────────────────────────────────────────────
    //
    // The board moving is an EVENT, and we were not firing it at all. Stock `TargetFrame.lua:30`
    // registers `RAID_TARGET_UPDATE` and its `:99` arm calls `TargetFrame_UpdateRaidTargetIcon`,
    // so without this the skull/star on your CURRENT target never appears or clears — it only
    // corrects itself on the next `PLAYER_TARGET_CHANGED`, i.e. when you re-target. That file is
    // on our chain, so this was live.
    //
    // Fired on the whole board rather than per unit because that is the shape the reference's
    // handler has: it takes no argument and re-reads the unit it is showing.
    if group.raid_targets != fed.raid_targets {
        gate.audit("feed_party", "the raid-target board edge");
        fed.raid_targets = group.raid_targets;
        script.fire_event("RAID_TARGET_UPDATE", vec![]);
    }

    // ── READY_CHECK (decision 1549) ─────────────────────────────────────────────────────────
    //
    // The reference's event `0x218`, fired by the `MSG_RAID_READY_CHECK` open handler
    // (`0x4ba360`), which is also where its 30 s deadline is armed (`0xb713f4 = clock + 0x7530`).
    // UIParent registers it and calls `ShowReadyCheck()`; the countdown itself is the popup's own
    // OnUpdate, so the deadline is Lua-side here rather than a second clock in Rust.
    if group.ready_check != fed.ready_check {
        gate.audit("feed_party", "the ready-check edge");
        fed.ready_check = group.ready_check;
        // Not on the first observation after a VM reset: `ready_check` is a session counter and a
        // fresh VM starting at 0 against a live 3 would pop a popup for a check that ended.
        if !vm_reset {
            script.fire_event("READY_CHECK", vec![]);
        }
    }
    // The request generation — both arms — is the engine's state half (decision 1989): the leader
    // arm's close-if-nobody-pending, the member arm's 30 s deadline. Then the answer log, replayed
    // from where this memo left off; a new request restarts the cursor with the log.
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

    // ── UPDATE_INSTANCE_INFO (decision 1549) ────────────────────────────────────────────────
    //
    // The saved-lockout list, pushed with its map names already resolved (the wire carries
    // `Map.dbc` ids; a missing catalog degrades to the id rather than to a blank row).
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
    // **The event follows the ANSWER, not the list** (1561). It is the one edge here that is not a
    // diff, and it cannot be: the reference throws its first `UPDATE_INSTANCE_INFO` away
    // (`RaidFrame.hasRaidInfo`) and decides the Raid Info button on the second, so a player with no
    // lockouts — whose list is empty and stays empty — would never reach a second one and would
    // keep a live button onto an empty panel. The client signals per packet, empty answers
    // included; `GroupState::saved_instances_answers` carries the bytes that settle it.
    //
    // The list is diffed as well, so a fresh VM is re-seeded with lockouts it never saw arrive —
    // but the EVENT is a packet's to fire, and no packet arrives on a `/reload`, so a reset seeds
    // in silence and the pane's own `RequestRaidInfo` on show fetches the real one.
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
            // Fires on Accept/Decline edges too — hiding an already-hidden popup is a no-op,
            // and the accepted-guard keeps the hide path from double-declining.
            None => script.fire_event("PARTY_INVITE_CANCEL", vec![]),
        }
        fed.invite = group.pending_invite.clone();
    }
}

/// The player's own raid row — the one entry `GroupState` cannot supply, because
/// `SMSG_GROUP_LIST` never lists the recipient.
pub(super) struct RaidSelf {
    pub(super) guid: u64,
    /// Our own subgroup bits + the assistant flag, `SMSG_GROUP_LIST`'s second byte.
    pub(super) flags: u8,
    pub(super) level: u32,
    /// The finest area under us (`WorldPoint::area()`), walked to a zone by the caller's resolver
    /// — the reference reads a zone-id global here (`0xb4e314`) on the live-object arm.
    pub(super) area: Option<u32>,
    /// `UNIT_FIELD_HEALTH <= 0` — the reference's live-object arm for return 9, and *only* that:
    /// deliberately `unit_is_dead` rather than `unit_reads_dead`, so a feigning hunter does not
    /// take the arm a health test would not.
    pub(super) dead: bool,
    /// **Our own `UNIT_FIELD_BYTES_0` class byte**, and it has to come from the descriptor rather
    /// than from the name cache the way everyone else's does (decision 1549).
    ///
    /// `NameCache::player_traits` is filled by `SMSG_NAME_QUERY_RESPONSE`, and **we never query
    /// ourselves**: the login seeds our own name with `traits: None` precisely so `"player"` needs
    /// no round trip (`net::session::connected`). So for our own row that lookup is `None`
    /// *forever*, which painted the local player's raid row with no class column and the plain
    /// font colour instead of their class's — caught in a live `/partytest raid` run, invisible to
    /// every unit test because the fixtures seed the traits by hand.
    ///
    /// The descriptor is the right source anyway: it is where `"player"`'s own snapshot reads
    /// class from (`ui_unit::snapshot`), and it is correct the instant the object lands.
    pub(super) class: Option<u8>,
}

/// The raid roster's ROW ORDER, as guids — **the one place the array's shape is decided**.
///
/// [`raid_roster`] fills the rows and the RaidFrame's drag/kick/menu paths address them by index,
/// and those two must never disagree about which player row 7 is: an off-by-one here kicks the
/// wrong person. So both read this, and the ordering law lives in one function — self first (the
/// wire's list excludes the recipient; [`raid_roster`]'s doc carries why that position is not
/// itself derived), then the wire's own member order.
///
/// Empty outside a raid, which is what makes every raid-index verb a no-op in a plain party.
pub(crate) fn raid_row_guids(group: &GroupState, self_guid: Option<u64>) -> Vec<u64> {
    raid_rows(group, self_guid).collect()
}

/// **The raid row ORDER, and the one place it is decided** — us first, then the roster in wire
/// order — as an iterator, so a caller that wants one row does not build all forty.
///
/// [`raid_row_guids`] collects it for the feed (which pushes every row anyway) and
/// [`raid_row_guid`] indexes it for the resolvers, which ask per token: `crate::ui_unit`'s reach
/// feed asks forty times a frame, and forty `Vec`s a frame for one `u64` each is the kind of cost
/// that only shows up in a raid, i.e. exactly where it hurts.
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

/// The guid at a 1-based raid row, or `None` for a row that is not there — the allocation-free
/// [`raid_row_guids`]`[index - 1]`.
pub(crate) fn raid_row_guid(
    group: &GroupState,
    self_guid: Option<u64>,
    index: usize,
) -> Option<u64> {
    raid_rows(group, self_guid).nth(index.checked_sub(1)?)
}

/// A raid row index (1-based, the Lua scale) → the guid it names, or `None` for a row that is not
/// there. Every raid-management verb goes through this, so "index 0", "index 500" and "not in a
/// raid" all collapse to the same quiet no-op the reference's own bindings produce.
fn raid_guid_at(group: &GroupState, self_guid: Option<u64>, index: u32) -> Option<u64> {
    raid_row_guid(group, self_guid, usize::try_from(index).ok()?)
}

/// A guid → the character name the wire wants for the by-name group opcodes. Ours comes from the
/// name cache (we are never in our own roster list); everyone else's is on the roster itself.
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

/// A character name → the guid the guid-bodied opcodes want (`CMSG_GROUP_SET_LEADER`,
/// `CMSG_GROUP_ASSISTANT_LEADER`). UnitPopup addresses raid rows by NAME, and the wire wants a
/// guid for exactly two of the four rank verbs, so the walk back happens here rather than in Lua.
/// Case-insensitive, like every other name compare against the roster.
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

/// Build `GetRaidRosterInfo`'s array (decision 0434 §6's roster, `0x4bb560`). Empty outside a
/// raid — `GroupState::group_type` is `1` only for one — which is what makes `GetNumRaidMembers()`
/// answer `0` in a party.
///
/// **The player is row 1.** The reference's array contains the local player (it is why
/// `UnitInRaid("player")` answers `1`), and the wire's list does not, so the recipient is spliced
/// back in here. *Where* the real client puts itself is **not derived** — the binding is derived,
/// not the array's fill order — and nothing observed depends on it: every corpus consumer
/// sweeps `1..GetNumRaidMembers()` (or `1..MAX_RAID_MEMBERS`) and keys the result by name.
///
/// A pure function over plain data so the shape is testable without a second account in a raid:
/// the tuple this fills is the part a live raid could confirm and a unit test cannot, but the
/// *mapping* — rank, subgroup, the online/dead bit pair — is the part that can be, and is below.
fn raid_roster(
    group: &GroupState,
    me: Option<&RaidSelf>,
    names: &NameCache,
    zone_name: &dyn Fn(u32) -> Option<String>,
) -> Vec<RaidMemberInfo> {
    if group.group_type != GROUPTYPE_RAID {
        return Vec::new();
    }
    // rank: 2 leader · 1 assistant · 0 member. The binding exposes `[edi+0xc]` unadjusted and
    // the convention cannot be derived from its bytes; the corpus can and does —
    // `ChatLog.lua:351-353` prints `@` for 2 and `*` for 1.
    let rank_of = |guid: u64, flags: u8| {
        if guid == group.leader {
            2
        } else if flags & GROUP_MEMBER_ASSISTANT != 0 {
            1
        } else {
            0
        }
    };
    // The class BYTE, resolved to its (display name, token) pair. Two sources, because the client
    // has two: everyone else's rides `SMSG_NAME_QUERY_RESPONSE` into the name cache, and our own
    // comes off our descriptor ([`RaidSelf::class`] carries why).
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
            // Stored 0-based; the binding adds the `0x4bb61a inc`.
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
            // An unresolved name takes the reference's name-cache-miss arm at the binding: the
            // whole 9-tuple becomes the fixed miss tuple, not a half-filled row.
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
            // The reference's cached arm: `[edi+0x18]` must carry BOTH `0x4` and `0x1`. Our wire
            // spells those `member_status::DEAD` and `ONLINE` (vmangos `Group.cpp:45-63`), which
            // is the corroboration recorded on `RaidMemberInfo::ninth` — an offline dead member
            // answers nil there, and reproducing the conjunction is the point.
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
    // The member's live descriptor, if their object is in the manager — the reference's
    // `0x468460`, resolved by the caller. Taking the *answer* rather than the index+query pair
    // is what makes the out-of-range leg (the whole of report B334) testable at all.
    store: Option<&ObjectStore>,
    group: &GroupState,
    own_group: Option<String>,
    // `ChrClasses.dbc`, for the relic column alone — see [`crate::ui_unit::snapshot`]. Only the
    // in-range leg can use it; the out-of-range roster record carries no class byte to key on,
    // so an out-of-range paladin reads no relic slot until their object streams back.
    classes: Option<&benilla_formats::ChrClasses>,
) -> UnitState {
    let mut s = match store {
        // In visibility range: the live descriptor is the truth (the server keeps it current).
        Some(store) => crate::ui_unit::snapshot(store, Some(m.name.clone()), 0, classes),
        // Out of range: **the roster record** — which is not only the `PARTY_MEMBER_STATS` wire
        // any more (decision 1640). It is seeded from the member's own live descriptor at the
        // instant their object leaves the manager (`0x5f0880`, `ui_party::net::
        // member_deactivated`), seated with the `1/1` placeholder when they join the roster
        // unseen (`0x4e82d0`), and patched by the wire afterwards — so this leg never reads the
        // `0/0` report B334 is about, and there is always a record to read.
        //
        // The reference's own getter chain, in order: live descriptor → party record (`0x496400`)
        // → pet record (`0x496420`) → 0. The pet leg is not reachable here (a `partyN` token is a
        // player guid; the `partypetN` tokens resolve nowhere in benilla yet), so this is the
        // whole of it.
        None => UnitState {
            exists: true,
            name: Some(m.name.clone()),
            health: stats.and_then(|s| s.cur_hp).map_or(0, u32::from),
            max_health: stats.and_then(|s| s.max_hp).map_or(0, u32::from),
            level: stats.and_then(|s| s.level).map_or(0, u32::from),
            power_type: stats.map_or(0, PartyMemberStatsInfo::shown_power_type),
            // **Divided, like the live leg** — `UnitMana` applies the raw→display scale on its
            // record path too (`0x517744`-`0x51775e`), which this arm did not: a warrior out of
            // range read ten times the rage of one in range.
            power: stats.map_or(0, PartyMemberStatsInfo::shown_power),
            max_power: stats.map_or(0, PartyMemberStatsInfo::shown_max_power),
            // **The record's status bits, for the two predicates the reference pins to it**:
            // `UnitIsDead 0x517b5d` reads `+0x08 & 4` and `UnitIsGhost 0x517c32` reads `& 8` on
            // the no-object leg. They matter because they are *fresher than the roster byte*: the
            // roster only moves on a `SMSG_GROUP_LIST`, while this byte is rewritten by the
            // descriptor snapshot at the despawn edge and by every stats delta after it — so a
            // member who was dead when they walked over the hill reads dead, where the last
            // roster echo still had them alive.
            //
            // Connected / AFK / DND / PvP / FFA deliberately stay the roster's below: the
            // reference's no-object path (`0x496400`) is known for the health, power, level and
            // dead/ghost/connected getters, and nothing is known of `UnitIsAFK` and kin. Taking
            // the record for those would be a guess, and vmangos only flags the status byte on the
            // AFK/DND/PvP/FFA toggles anyway — never on death, which is exactly why the two above
            // are the pair worth reading.
            dead: stats.is_some_and(|s| s.status.unwrap_or(0) & member_status::DEAD != 0),
            ghost: stats.is_some_and(|s| s.status.unwrap_or(0) & member_status::GHOST != 0),
            ..Default::default()
        },
    };
    s.is_player = true;
    // Identity + the raid-target mark (decision 0434 §5/§6), and a party member is always a
    // same-faction FRIENDLY player in 1.12 — the popup's UnitCanCooperate gate (whisper/trade
    // rows) reads the reaction, which neither merged-view leg resolves for party tokens.
    s.guid = m.guid;
    s.raid_target = group.raid_target_index(m.guid);
    s.reaction = 5;
    // The group PvP icon's faction (decision 0646 §1 — closes 0434's named party-icon deferral).
    s.faction_group = own_group;
    // The roster status byte overlays BOTH paths (GetGroupMemberStatus's Lua-predicate bits).
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
    // The same slot view the feed publishes the tokens from — the mapping must agree.
    (1..=4)
        .contains(&n)
        .then(|| group.party_slots().nth(n - 1))?
}

/// Drain the Lua-side party intents into their `CMSG_*` sends (the popup's Accept/Decline, the
/// future UnitPopup's invite/uninvite/promote/loot calls).
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
        // Sandbox mode: the roster is synthetic and the server holds no group, so the CMSG
        // sends below would vanish into a void — the group-mutating intents apply to the local
        // mirror instead (what the echo would have done). Invites and the popup's
        // accept/decline stay real: a genuine group forming switches the sandbox off.
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
                // "target" resolves through the selection iff it's a player (the ref's
                // InviteToParty(unit) path); a roster token is already named.
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
                // The caller's own threshold wins when it passed one. Absent, we keep the
                // group's current floor.
                //
                // **Stated divergence** (decision 1675): the real binding defaults the absent
                // argument to a literal 2, so on the reference client changing the loot method
                // with no third argument silently RESETS the threshold to Uncommon. Ours is
                // sticky. The reference behaviour is one line (`.unwrap_or(2)` on `asked` alone);
                // it is left to the director's call because it is quietly destructive and nothing
                // in this arc needs it.
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
                // Token → guid: self, the selection, or a roster slot (the popup passes the
                // token it was opened for). An unresolvable token is a no-op, like TargetUnit.
                let guid = match unit.as_str() {
                    "player" => self_q.iter().next().map(|g| g.0),
                    "target" => selection.guid,
                    t => member_for_token(&group, t).map(|m| m.guid),
                };
                let Some(guid) = guid else { continue };
                // Lua marks are 1..8 over wire icons 0..7. Setting sends (mark-1, guid) — the
                // server clears the unit's old icon itself; clearing (Lua 0) re-sends the
                // unit's CURRENT icon with guid 0 (vmangos Group::SetTargetIcon's shape — there
                // is no "clear by unit" on the wire).
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
            // ── The raid-management verbs (decision 1549) ────────────────────────────────────
            //
            // Three address forms meet one wire here (see `script::party`'s module doc): the
            // RaidFrame hands us raid ROW INDICES, UnitPopup hands us NAMES, and the wire wants
            // whichever of name/guid vmangos declared per opcode. Every resolution failure is a
            // quiet no-op — the reference's bindings marshal and send, and a row that is not
            // there produces no packet rather than an error line.
            PartyRequest::ConvertToRaid => {
                let _ = commands.0.send(ClientCommand::GroupRaidConvert);
            }
            PartyRequest::SetSubgroup { index, group: sub } => {
                // The Lua scale is 1..8 and the wire's is 0..7 (`SMSG_GROUP_LIST`'s own flag bits
                // 0-2, vmangos `Group.cpp:158`). Out-of-range subgroups are dropped rather than
                // wrapped: an 8-value field with a 3-bit home is exactly where a silent modulo
                // would move somebody into the wrong raid group.
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
                // `CMSG_GROUP_UNINVITE` takes a NAME even though a `_GUID` twin exists; the ref's
                // party path already uses the name form, and one form for both keeps the server's
                // "not in your party" reply meaning the same thing on either.
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

/// The sandbox half of the drain ([`drain_party`]'s `group.test` branch): apply one
/// group-mutating intent to the local mirror, mimicking the server echo a real group would have
/// sent — the same board/roster/loot writes the apply arms make from the wire. Returns `false`
/// for the intents that stay real even while testing (invites, accept/decline).
fn test_apply_local(
    group: &mut GroupState,
    req: &PartyRequest,
    self_guid: Option<u64>,
    target_guid: Option<u64>,
) -> bool {
    match req {
        PartyRequest::Leave => {
            // A local disband: exactly what the real path's all-zero list resets — the group
            // facts, sandbox flag included (`/partytest` starts a fresh one), and not the
            // session's tickets.
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
                // Same rule as the live drain: the caller's threshold wins when it passed one,
                // else the group keeps its current floor.
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
                // The server's SetTargetIcon clears the unit's old icon before setting the
                // new one — one mark per unit.
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
        // ── The raid verbs, sandboxed (decision 1549) ─────────────────────────────────────────
        //
        // These are the whole reason `/raidtest` can be a look-pass instrument rather than a
        // still photograph: the drag really moves someone, Ready Check really opens the popup,
        // and the kick really empties a slot — through the same Lua the live client runs, with
        // this standing in for the server echo. A raid needs 40 accounts otherwise.
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
            // Row 1 is us; "kick yourself" is not a thing the server would do either.
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
            // The echo a real server sends back to the whole raid, us included. The presser is the
            // leader (the stock Raid tab offers the button to no one else), and the leader's own
            // echo takes the response-collection arm: no popup for the person who pressed it,
            // which is the reference's behaviour (decision 1989) — the summary line is theirs.
            group.apply_ready_check_request(true);
            true
        }
        PartyRequest::RequestRaidInfo => {
            // `/partytest raid` seeds the lockouts up front, so the ask is answered on the spot —
            // with the list it already holds, which is exactly what the server does for a second
            // ask. Re-applying it is what bumps the answer ticket, and the ticket is what fires
            // `UPDATE_INSTANCE_INFO` (1561), so the sandbox reaches the second answer the Raid
            // Info button is decided on instead of stalling on the first.
            let held = std::mem::take(&mut group.saved_instances);
            group.apply_raid_instance_info(held);
            true
        }
        _ => false,
    }
}

/// `/raidtest`'s subgroup read: the 0-based subgroup bits of the raid row `index` names (`None`
/// for a row that is not there). Our own row lives in `own_flags`, everyone else's in their
/// roster entry — the same split the wire has.
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

/// `/raidtest`'s subgroup write ([`test_subgroup_of`]'s twin) — bits 0-2 only, so the assistant
/// flag riding the same byte survives a move.
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

/// The `/partytest` instrument (decision 0434, the 0288 `/chattest` pattern): a synthetic
/// 4-member roster with mixed statuses + out-of-range stats, pumped through the REAL apply path
/// (so the composer's lines print too) — the whole frame surface eyeballable with no server.
/// The guids are unstreamed player-range fakes, so every member exercises the stats-snapshot leg
/// of the merged view. `player_xy` (the caller's live WoW position) plants stats positions
/// around the player so the minimap blips show too: Alice 30 yd out (dot), Bob 80 yd (dot that
/// becomes a rim arrow one zoom in), Carol 300 yd (rim arrow), Dave offline (no blip).
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
    // The blip seats: WoW-axis offsets from the player (+x north, +y west), i16-truncated
    // exactly like the wire. Dave (offline) gets none.
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
    // Sandbox on (apply_list just cleared it — the wire-wins default): the drain now applies
    // group-mutating menu intents locally, so the popup is exercisable serverless too.
    group.test = true;
    lines
}

/// The `/partytest raid` instrument (decision 1549) — [`synthetic_roster`]'s raid twin, and the
/// only way the Raid tab's grid is eyeballable without forty accounts.
///
/// 24 synthetic members across subgroups 1-5 plus us in group 1 = a 25-row raid: every colour the
/// pane can paint is on screen at once (eight class colours, one dead member in red, one offline
/// in grey), one assistant carries the `(A)` token and we carry `(L)`, and **we lead**, so Convert
/// To Raid is correctly hidden, Ready Check and Add Member are live, and a drag really moves
/// somebody (the sandbox drain applies the subgroup verbs locally — [`test_apply_local`]).
///
/// The names go into the NAME CACHE with race/class traits, because the raid roster resolves a
/// member's class from there rather than from the wire ([`raid_roster`]): without that seeding the
/// grid would paint 25 white rows with an empty class column, which is precisely the thing the
/// instrument exists to let someone look at.
///
/// Two fake lockouts are seeded too, so the Raid Info panel has rows.
pub(crate) fn synthetic_raid(
    group: &mut GroupState,
    names: &mut NameCache,
    self_guid: Option<u64>,
) -> Vec<crate::ui_action::UiError> {
    // (name, class id, race id) — `benilla-formats`' own `ChrClasses`/`ChrRaces` ids, the pair
    // `NameCache::player_traits` hands `ui_unit::class_names`. Eight classes so every colour in
    // `RAID_CLASS_COLORS` shows; the races are Alliance-side and cosmetic here.
    const ROSTER: [(&str, u8, u8); 24] = [
        ("Alaric", 1, 1),  // Warrior
        ("Brienne", 2, 1), // Paladin
        ("Cassian", 3, 3), // Hunter
        ("Dara", 4, 4),    // Rogue
        ("Elowen", 5, 1),  // Priest
        ("Fenwick", 7, 3), // Shaman — Horde-only in 1.12; the pane does not care and the
        ("Gwendal", 8, 7), // colour is the point
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
        // Subgroups 1-5, five to a group — but we occupy one seat of group 1, so the first four
        // fill it and the rest lay out five apiece. `i / 5` on 24 members lands 4/5/5/5/5.
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
    // groupType 1 = raid, and the leader is US: `apply_list` takes the leader guid, and passing
    // our own makes `IsRaidLeader()` true so the leader-only surface is live.
    let leader = self_guid.unwrap_or(0);
    let lines = group.apply_list(
        1,
        0, // our own flags: subgroup 1 (0-based 0), no assistant bit — we are the leader
        members,
        leader,
        Some(GroupLootInfo {
            method: 2,
            master: leader,
            threshold: 3,
        }),
    );
    // Levels and health, so the level column is not 25 blanks and the merged view has something
    // to show. Every member is out of streaming range by construction (fake guids), so this is
    // the `PARTY_MEMBER_STATS` leg — the same one a real raid spread over an instance uses.
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
    // SIX lockouts for the Raid Info panel, not the two this seeded through 1560. The panel fits
    // four rows and grows a scroll bar at five (`RaidInfoFrame_Update`), and that bar has to be
    // re-seated onto the trough art drawn behind it — so with two rows the one part of the panel
    // with any geometry in it was unreachable from the instrument, and the misalignment shipped
    // (1561). An instrument that can only reach the states a real character reaches by accident is
    // not an instrument. Map ids are real ones, so the rows carry `Map.dbc`'s own names.
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
    // Sandbox on, after `apply_list` cleared it (the wire-wins default) — see [`synthetic_roster`].
    group.test = true;
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The out-of-range party frame reads the roster record** (decision 1640, report B334) —
    /// the reference's `UnitHealth`/`UnitMana`/`UnitLevel` no-object leg, `0x496400` into
    /// `0xbc70b0 + slot·0x148`.
    ///
    /// Two things this pins, and both were wrong before it. The bars come off the record at all
    /// (they used to read the wire-only snapshot and map an absent field to `0`, which is the
    /// blanked frame Goudy reported); and the power pair is **divided** by the raw→display scale
    /// on this leg exactly as on the live one — a warrior's rage rides the wire ×10, so without
    /// the divide an out-of-range warrior reads ten times an in-range one.
    #[test]
    fn an_out_of_range_member_reads_the_record_and_divides_its_rage() {
        let m = GroupMemberEntry {
            name: "Thalyn".into(),
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

        // And a member with no record at all still reads as an existing, connected player — the
        // seat law means this cannot happen for a real roster, but the mapping must not invent
        // numbers when it does.
        let bare = member_unit_state(&m, None, None, &GroupState::default(), None, None);
        assert_eq!((bare.health, bare.max_health, bare.power), (0, 0, 0));
        assert!(bare.exists);
    }

    /// **The record's dead/ghost bits are read out of range** — `UnitIsDead 0x517b5d` (`+0x08 &
    /// 4`) and `UnitIsGhost 0x517c32` (`& 8`), the two predicates the reference pins to the
    /// no-object leg.
    ///
    /// The falsifier is the roster byte's staleness: it only moves on a `SMSG_GROUP_LIST`, and
    /// vmangos never flags the party status byte on death (`Player.cpp`'s five setters are the
    /// AFK/DND/PvP/FFA toggles). So a member who was dead at the moment they walked out of range
    /// is dead in the record — written there by the despawn snapshot — and alive in the roster
    /// echo that predates it. Read only the roster and their frame stays lit.
    #[test]
    fn an_out_of_range_members_dead_and_ghost_come_off_the_record() {
        let m = GroupMemberEntry {
            name: "Thalyn".into(),
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
        assert!(!s.dead, "a released ghost is not `dead` — the 0308 §1 trio");

        // The roster byte still wins when IT is the one carrying the bit (an offline member whose
        // record was never filled): the overlay ORs, it does not replace.
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

    /// A repeat answer is an edge even when it repeats *nothing* — the rule the Raid Info button
    /// hangs off, and the one that was missing (1561).
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
        // The list half still works on its own, for the answer that actually brings something.
        let one = [SavedInstanceInfo {
            name: "Molten Core".into(),
            instance: 1234,
            reset: 86_400,
        }];
        assert!(saved_instances_moved(&one, 1, &fed));
    }

    /// The sandbox drain half: each group-mutating intent lands on the local mirror exactly as
    /// the server echo would have — and a real wire list always switches the sandbox back off.
    #[test]
    fn partytest_sandbox_applies_menu_intents_locally() {
        let mut group = GroupState::default();
        synthetic_roster(&mut group, None);
        assert!(group.test, "the synthetic roster arms the sandbox");
        let (me, mob) = (Some(0x5E1Fu64), Some(0xB0B0u64));

        // Mark the target with Skull, then move it to Cross — one mark per unit.
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

    /// The raid roster mapping — the half a unit test can actually settle. The nine-value *shape*
    /// is pinned in `benilla-ui`'s own tests; what is pinned here is which wire fact lands in
    /// which slot, because that is what a live raid would otherwise be the only witness to.
    #[test]
    fn the_raid_roster_maps_the_wire_to_get_raid_roster_info() {
        let mut names = NameCache::default();
        // **Our own row has NO traits in the cache, and that is the live shape, not a gap in the
        // fixture** (decision 1549): the login seeds our name with `traits: None` so `"player"`
        // never needs a name query, so `player_traits(self)` is None forever. This used to be
        // `Some((4, 11, 0))` here, which is why the local player's raid row shipped colourless —
        // the fixture was seeding a packet the client never sends itself.
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
                // Alice: subgroup 2 (stored), an assistant, online and DEAD.
                member(
                    0xA11CE,
                    "Alice",
                    member_status::ONLINE | member_status::DEAD,
                    2 | GROUP_MEMBER_ASSISTANT,
                ),
                // Bob: subgroup 0, offline — and offline-AND-dead, which must NOT set return 9.
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
            // Off our DESCRIPTOR, which is the only source that has it for us. 11 = Druid.
            class: Some(11),
        };

        let roster = raid_roster(&group, Some(&me), &names, &zone);
        assert_eq!(roster.len(), 3, "the player is spliced back in");

        // Row 1 — us. Leader (rank 2), subgroup stored 0, and the class off the DESCRIPTOR.
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

        // Row 2 — an assistant, and the online+dead pair the reference's cached arm tests.
        assert_eq!((roster[1].rank, roster[1].subgroup), (1, 2));
        assert_eq!(roster[1].level, 58);
        assert_eq!(roster[1].class_file.as_deref(), Some("WARRIOR"));
        assert!(roster[1].online && roster[1].ninth, "online AND dead");

        // Row 3 — offline. Return 9 needs BOTH bits, so a dead-but-offline member is nil there,
        // and an unresolved class is nil rather than a guess.
        assert!(!roster[2].online, "offline");
        assert!(!roster[2].ninth, "0x4 without 0x1 is not the arm");
        assert_eq!(roster[2].class, None);
        assert_eq!(roster[2].zone, None, "no stats packet, no zone");

        // A PARTY is not a raid: the whole list is empty, so `GetNumRaidMembers()` answers 0
        // while `IsRaidLeader()` still answers 1 — the pair that surprises people.
        group.group_type = 0;
        assert!(raid_roster(&group, Some(&me), &names, &zone).is_empty());
    }

    /// The row ORDER, and the two resolutions every raid verb goes through (decision 1549).
    ///
    /// This is the off-by-one that kicks the wrong person, so it is asserted against the same
    /// helper `raid_roster` fills its array from — the whole reason that helper exists rather than
    /// each site re-deriving "self first, then the wire's order".
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

        // Names: ours comes from the cache (we are never in our own roster list), everyone
        // else's from the roster row.
        let mut names = NameCache::default();
        names.insert_player(0x5E1F, "Sam".into(), None);
        assert_eq!(
            raid_name_of(&group, me, &names, 0x5E1F).as_deref(),
            Some("Sam")
        );
        assert_eq!(
            raid_name_of(&group, me, &names, 0xA11CE).as_deref(),
            Some("Alice")
        );
        assert_eq!(raid_name_of(&group, me, &names, 0xDEAD), None);

        // And the walk back, which the two guid-bodied opcodes need. Case-insensitive.
        assert_eq!(
            raid_guid_for_name(&group, me, &names, "alice"),
            Some(0xA11CE)
        );
        assert_eq!(raid_guid_for_name(&group, me, &names, "SAM"), Some(0x5E1F));
        assert_eq!(raid_guid_for_name(&group, me, &names, "Nobody"), None);

        // Outside a raid every one of them is empty — which is what makes a raid verb typed in a
        // party a quiet no-op rather than a packet about the wrong player.
        group.group_type = 0;
        assert!(raid_row_guids(&group, me).is_empty());
        assert_eq!(raid_guid_at(&group, me, 1), None);
    }

    /// `/partytest raid`'s sandbox half: the subgroup verbs land on the local mirror the way the
    /// server echo would, and the assistant bit sharing the byte survives a move.
    #[test]
    fn the_sandbox_moves_and_swaps_subgroups_locally() {
        let mut group = GroupState::default();
        let me = Some(0x5E1Fu64);
        let mut names = NameCache::default();
        names.insert_player(0x5E1F, "Sam".into(), None);
        synthetic_raid(&mut group, &mut names, me);
        assert!(group.test, "the synthetic raid arms the sandbox");
        assert_eq!(group.group_type, GROUPTYPE_RAID);

        // Row 2 is the first wire member, and `synthetic_raid` gives it the assistant bit.
        assert_eq!(test_subgroup_of(&group, me, 2), Some(0));
        assert!(group.members[0].flags & GROUP_MEMBER_ASSISTANT != 0);

        // Move it to subgroup 8 (Lua) = 7 (wire).
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

        // Swap it with row 1 — us, whose subgroup lives in `own_flags` rather than in the list.
        let mine = test_subgroup_of(&group, me, 1).unwrap();
        assert!(test_apply_local(
            &mut group,
            &PartyRequest::SwapSubgroup { index: 1, other: 2 },
            me,
            None
        ));
        assert_eq!(test_subgroup_of(&group, me, 1), Some(7));
        assert_eq!(test_subgroup_of(&group, me, 2), Some(mine));

        // An out-of-range subgroup is dropped, never wrapped into a 3-bit field.
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

        // Ready Check echoes back to us as the leader: the response-collection arm, which bumps
        // the request generation and never the popup ticket (decision 1989).
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

        // And the kick empties a seat — but never our own row.
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
