//! When to ask the server for a questgiver's `!` or `?`; [`super`] renders them. The server never
//! pushes a status, it only answers the query (vmangos `QuestHandler.cpp:36-77`), so every refresh
//! is the client's to trigger ([`query_statuses`]).
//!
//! The gold `!` and the flight master's green `!` share one slot, `unit+0xb2c`: `0x607480`
//! installs whichever arrives, `0x6073f0` zeroes it with the status at `+0xcb8`, and the per-unit
//! path `0x607380` tears it down, then re-issues `0x182` on `UNIT_NPC_FLAGS` bit 1 and `0x1aa` on
//! bit 3. So one system owns both asks, and `ui_taxi` keeps only the answer.
//!
//! A quest-flagged GameObject is asked about and never rendered: the answer handler `0x5dc9f0`
//! resolves the guid with typemask 8 (`0x468460`), so its answer dies at `0x5dca2f`.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_protocol::EntityKind;

use crate::net::{ClientCommand, GuidIndex, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::target::cursor_mode::go_reaction;
use crate::target::ring::Factions;
use crate::ui_quest::QuestGiver;

/// `UNIT_NPC_FLAGS` questgiver service bit.
const NPC_FLAG_QUESTGIVER: u32 = 0x2;
/// `UNIT_NPC_FLAGS` flightmaster service bit, the other half of `0x607380`'s re-gate (`0x6073de`)
/// and the gate on the only `0x1aa` sender.
const NPC_FLAG_FLIGHTMASTER: u32 = 0x8;
/// `GAMEOBJECT_FLAGS` bit 2, the sweep's whole GameObject gate (`0x5eb0ef`..`0x5eb0f5`: shift 2,
/// so mask `0x4`, not the unit leg's `0x2`). The name is vmangos's (`GameObjectDefines.h:76`).
const GO_FLAG_INTERACT_COND: u32 = 0x4;

/// Asks the dialog status of every questgiver-flagged creature, and re-asks when the answer could
/// have changed. The reference sweeps every visible object (`0x5eb070 → 0x468380`) from its
/// self-player field watches (`0x468070`) and four packet handlers.
///
/// - Sweep: [`self_generation`] (the field watches) XOR [`QuestGiver::reask_epoch`] (the packet
///   handlers); a changed generation is a sweep.
/// - Per unit: asked from the create path and when its service bits (`0x60b490`, `0x60b4c5`) or
///   reaction inputs (`0x606f0a`) move, our [`unit_ask_key`]. The entry goes when the guid leaves
///   [`GuidIndex`], as the reference's cache at `unit+0xcb8` dies with the object.
/// - GameObject: asked only on a sweep (`0x5eb159`, `0x5eb456`), gated on
///   [`GO_FLAG_INTERACT_COND`] and a reaction toward us above 1 ([`go_reaction`], `0x5f7fd0`). Its
///   constructor calls no sender, so there is no query at first sight, and with no status slot it
///   is never torn down.
///
/// The light sweep's callback `0x5eb0a0` sends `0x182` for a questgiver and tears down
/// (`0x5eb143`) any Hated or Hostile creature, questgiver or not, never on the flag alone. A flag
/// change reaches `0x607380` through the `UNIT_NPC_FLAGS` watch (`0x60b420`), which tears down
/// first (`0x607384`), then re-gates on reaction. The full sweep `0x5eb3c0` (your revive, your own
/// reaction inputs) runs `0x607380` on every creature (`0x5eb3f0`): `full` below.
///
/// Not covered: the reference's `ITEM` watch (`0x5d9375`) also sweeps when an item moves between
/// containers; with no item objects, only our equipment and bag guids are folded. Per unit, its
/// reaction refresh also keys on charm, persuade, duel team and `PLAYER_BYTES_3`; ours on the
/// faction template, a standing change coming through the reputation sweep.
#[allow(clippy::type_complexity)] // a Bevy system: one param per resource
pub(super) fn query_statuses(
    self_q: Query<Ref<ObjectStore>, With<SelfPlayer>>,
    objects: Query<
        (
            Entity,
            &crate::net::Guid,
            &NetEntity,
            Ref<ObjectStore>,
            Option<&crate::ui_taxi::FlightMasterStatus>,
        ),
        Without<SelfPlayer>,
    >,
    index: Res<GuidIndex>,
    factions: Option<Res<Factions>>,
    reputations: Res<crate::net::Reputations>,
    mut quest: ResMut<QuestGiver>,
    commands: Res<NetCommands>,
    mut ecs: Commands,
    mut state: Local<QueryState>,
) {
    let Some(store) = self_q.iter().next() else {
        return;
    };
    // The descriptor walk runs only when our store changed; the epoch is a plain counter.
    if store.is_changed() {
        state.fields = self_generation(&store.0);
    }
    let generation = state.fields ^ (u64::from(quest.reask_epoch()) << 32);
    // The full sweep's triggers (`0x5eb3c0`): your revive, the `≤0 → >0` crossing (`0x6046f0`),
    // and your own reaction inputs moving (`0x606e20`). Edges, since death is a light sweep.
    let alive = store.0.unit_health().unwrap_or(1) != 0;
    // Compared only between frames that both saw the field, or the template's arrival at login
    // would fire a full sweep.
    let my_faction = store.0.unit_faction_template();
    let full = (alive && state.was_dead)
        || (my_faction.is_some() && state.my_faction.is_some() && state.my_faction != my_faction);
    state.was_dead = !alive;
    if my_faction.is_some() {
        state.my_faction = my_faction;
    }
    // A sweep frame, the only time a GameObject is asked about.
    let swept = state.generation != generation || full;
    state.generation = generation;
    // A guid that left the world drops its ask key and cached status. A sweep never clears the
    // map: only a unit's own key moving is a `0x607380`. Pruned only when the index moved, as the
    // `ResMut` retain marks `QuestGiver` changed.
    if index.is_changed() {
        state.asked.retain(|guid, _| index.0.contains_key(guid));
        quest.retain_statuses(|npc| index.0.contains_key(&npc));
    }
    // Off a sweep, a unit's verdict depends only on its descriptor and the two reaction
    // catalogs; with all three still it is a no-op, so the unit is skipped.
    let catalogs_moved =
        reputations.is_changed() || factions.as_ref().is_some_and(|f| f.is_changed());
    for (entity, guid, net, obj, fm) in &objects {
        if !swept && !full && !catalogs_moved && !obj.is_changed() {
            continue;
        }
        // `0x6073f0` zeroes the marker (`+0xb2c`, the green `!` too) and the status (`+0xcb8`)
        // together, so every teardown takes both.
        let tear_down = |quest: &mut QuestGiver, ecs: &mut Commands| {
            quest.clear_status(guid.0);
            if fm.is_some() {
                ecs.entity(entity)
                    .remove::<crate::ui_taxi::FlightMasterStatus>();
            }
        };
        match net.kind {
            // Typemask exactly `OBJECT|UNIT` (`0x5eb0d3`): a player, `0x19`, leaves the sweep here.
            EntityKind::Unit => {
                // The unit's reaction toward us (`0x6061e0`, this = the unit, at `0x5eb12a`), the
                // direction `ring_reaction` resolves: `<= 1` is Hated or Hostile, and a missing
                // input reads Neutral, so a cold catalog never blanks a marker.
                let reaction = crate::target::ring_reaction(
                    factions.as_deref(),
                    &reputations,
                    Some(&*obj),
                    Some(&store),
                );
                if reaction <= 1 {
                    // The sweep's one teardown (`0x5eb143`), questgiver or not. Dropping the ask
                    // key brings the marker back through a `0x607380` when the reaction recovers.
                    state.asked.remove(&guid.0);
                    tear_down(&mut quest, &mut ecs);
                    continue;
                }
                let key = unit_ask_key(&obj.0);
                let key_moved = state.asked.insert(guid.0, key) != Some(key);
                // `0x607380`, from the unit's field watches, its create path and the full sweep:
                // tear down first (`0x607384`), then re-gate. An escort giver's flag drop, with
                // the quest-log write, moves the ask key and lands here.
                let per_unit = key_moved || full;
                if per_unit {
                    tear_down(&mut quest, &mut ecs);
                }
                // The re-gate, the reaction being above 1. `0x182` goes out from `0x607380`
                // (`0x6073cd`) and from the light sweep (`0x5eb159`), so every sweep re-asks;
                // `0x1aa` has one sender, `0x607380` (`0x6073e8`), so never on a light sweep.
                let flags = obj.0.unit_npc_flags();
                if (per_unit || swept) && flags & NPC_FLAG_QUESTGIVER != 0 {
                    let _ = commands
                        .0
                        .send(ClientCommand::QuestgiverStatusQuery { npc: guid.0 });
                }
                if per_unit && flags & NPC_FLAG_FLIGHTMASTER != 0 {
                    let _ = commands
                        .0
                        .send(ClientCommand::TaxiNodeStatusQuery { guid: guid.0 });
                }
            }
            // `TYPEMASK_GAMEOBJECT` (`0x5eb0da`): sweep only, no ask key, no teardown.
            EntityKind::GameObject if swept => {
                if obj.0.gameobject_flags() & GO_FLAG_INTERACT_COND == 0 {
                    continue;
                }
                // A reaction toward us above 1 (`0x5eb101`): Unfriendly and Neutral pass, a
                // faction-less object reads Neutral, and `None` (no catalog yet) passes, as in the
                // cursor's gate.
                let reaction = go_reaction(
                    factions.as_deref(),
                    obj.0.gameobject_faction(),
                    Some(&store),
                );
                if !reaction.is_none_or(|r| r > 1) {
                    continue;
                }
                let _ = commands
                    .0
                    .send(ClientCommand::QuestgiverStatusQuery { npc: guid.0 });
            }
            _ => {}
        }
    }
}

/// The per-unit ask key: `UNIT_NPC_FLAGS`, which carries both watched service bits, and the
/// faction template, the reaction input we can see.
fn unit_ask_key(fields: &benilla_protocol::ObjectFields) -> u64 {
    u64::from(fields.unit_npc_flags())
        | (u64::from(fields.unit_faction_template().unwrap_or(0)) << 32)
}

/// Folds the self descriptor fields the reference watches, one handler each (`0x468070`), into one
/// value: level, the quest log, money, every skill, `PLAYER_FLAGS`, alive or dead, and the
/// equipment and bag guids in place of the reference's `ITEM` watch.
fn self_generation(fields: &benilla_protocol::ObjectFields) -> u64 {
    const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;
    let mut h = 0xcbf2_9ce4_8422_2325u64; // FNV offset
    let mut fold = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(FNV_PRIME);
    };
    fold(u64::from(fields.unit_level().unwrap_or(0)));
    // Health enters as the alive bit: the `UNIT_FIELD_HEALTH` watch (`0x6046f0`) sweeps only on a
    // zero crossing, death to the light sweep (`0x604774`) and revive to the full one
    // (`0x6047f0`); damage and regen ticks reach neither.
    fold(u64::from(fields.unit_health().unwrap_or(1) == 0));
    fold(u64::from(fields.player_flags()));
    fold(u64::from(fields.player_money().unwrap_or(0)));
    for slot in 0..benilla_protocol::messages::PLAYER_QUEST_LOG_SLOTS {
        if let Some(s) = fields.player_quest_log(slot) {
            if s.quest_id != 0 {
                fold(u64::from(s.quest_id) ^ (u64::from(s.state) << 32));
            }
        }
    }
    // The reference watches the skill value word (`edx=0x84c`): a rank-up, not a bonus ticking.
    for slot in 0..benilla_protocol::messages::PLAYER_SKILL_SLOTS {
        if let Some(s) = fields.player_skill(slot) {
            if s.skill_id != 0 {
                fold(u64::from(s.skill_id) ^ (u64::from(s.value) << 32));
            }
        }
    }
    for i in 0..23 {
        if let Some(guid) = fields.player_inv_slot(i) {
            fold(guid);
        }
    }
    h
}

/// [`query_statuses`]'s memory across frames.
#[derive(Default)]
pub(super) struct QueryState {
    /// The last [`self_generation`], kept so the walk runs only when our store changes.
    fields: u64,
    /// `fields` combined with the packet epoch; a change here is the sweep.
    generation: u64,
    /// Each asked guid's [`unit_ask_key`]; pruned by object lifetime only, never by a sweep.
    asked: HashMap<u64, u64>,
    /// Last frame's alive state and faction template, the two full-sweep edges; the first frame in
    /// a world is never a full sweep.
    was_dead: bool,
    my_faction: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_is_re_asked_on_a_ding_on_re_entry_and_dropped_with_the_flag() {
        use crate::net::{Guid, NetEntity};
        use benilla_protocol::{EntityKind, ObjectFields};

        const NPC: u64 = 0xdead_beef;
        const FIELD_LEVEL: u16 = 34; // UNIT_FIELD_LEVEL
        const FIELD_NPC_FLAGS: u16 = 147; // UNIT_NPC_FLAGS

        let net_entity = || NetEntity {
            kind: EntityKind::Unit,
            display_id: None,
            scale: 1.0,
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .add_systems(Update, query_statuses);

        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                net_entity(),
                Guid(1),
                ObjectStore(ObjectFields::from_pairs(&[(FIELD_LEVEL, 5)])),
            ))
            .id();
        let spawn_npc = |app: &mut App| {
            let e = app
                .world_mut()
                .spawn((
                    net_entity(),
                    Guid(NPC),
                    ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0x2)])),
                ))
                .id();
            app.world_mut().resource_mut::<GuidIndex>().0.insert(NPC, e);
            e
        };
        let asked = |app: &mut App| -> usize {
            app.update();
            rx.try_iter()
                .filter(
                    |c| matches!(c, ClientCommand::QuestgiverStatusQuery { npc } if *npc == NPC),
                )
                .count()
        };

        let npc = spawn_npc(&mut app);
        assert_eq!(asked(&mut app), 1, "asked once when it comes into view");
        assert_eq!(asked(&mut app), 0, "not again while nothing has changed");

        // The ding: `SatisfyQuestLevel` made the answer UNAVAILABLE, so the whole set is re-asked.
        *app.world_mut()
            .entity_mut(me)
            .get_mut::<ObjectStore>()
            .unwrap() = ObjectStore(ObjectFields::from_pairs(&[(FIELD_LEVEL, 6)]));
        assert_eq!(asked(&mut app), 1, "our level changed — re-ask everyone");
        assert_eq!(asked(&mut app), 0, "…once");

        // Out of view: the cached status dies with the object, as the reference's does.
        app.world_mut()
            .resource_mut::<QuestGiver>()
            .set_status(NPC, 5);
        app.world_mut().resource_mut::<GuidIndex>().0.remove(&NPC);
        app.world_mut().entity_mut(npc).despawn();
        assert_eq!(asked(&mut app), 0, "gone: nothing to ask");
        assert!(
            app.world().resource::<QuestGiver>().status(NPC).is_none(),
            "the cached status goes with the object"
        );

        let npc = spawn_npc(&mut app);
        assert_eq!(asked(&mut app), 1, "back in view — asked afresh");

        // The flag goes off: drop the answer, which drops the marker with it.
        app.world_mut()
            .resource_mut::<QuestGiver>()
            .set_status(NPC, 5);
        *app.world_mut()
            .entity_mut(npc)
            .get_mut::<ObjectStore>()
            .unwrap() = ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0x1)]));
        assert_eq!(
            asked(&mut app),
            0,
            "no longer a questgiver — nothing to ask"
        );
        assert!(
            app.world().resource::<QuestGiver>().status(NPC).is_none(),
            "and its stale answer is dropped"
        );
    }

    /// Accepting an escort, vmangos writes the quest-log slot and zeroes the giver's
    /// `UNIT_NPC_FLAGS` in the same update (`ScriptedEscortAI.cpp:470`,
    /// `ScriptedFollowerAI.cpp:328`): the frame is a sweep, and the flag drop must still tear the
    /// marker down, while a giver whose flag stays on keeps its answer.
    #[test]
    fn an_escort_giver_loses_its_marker_when_the_flag_drops_with_the_quest_log_write() {
        use crate::net::{Guid, NetEntity};
        use benilla_protocol::{EntityKind, ObjectFields};

        const ESCORT: u64 = 0x5115; // Mist: gives the quest, then follows with npcflags off
        const PLAIN: u64 = 0x5116; // an ordinary giver standing next to her
        const FIELD_NPC_FLAGS: u16 = 147;
        const FIELD_QUEST_LOG_1_1: u16 = 198;

        let net_entity = || NetEntity {
            kind: EntityKind::Unit,
            display_id: None,
            scale: 1.0,
        };
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .add_systems(Update, query_statuses);

        let me = app
            .world_mut()
            .spawn((SelfPlayer, net_entity(), Guid(1), ObjectStore::default()))
            .id();
        let spawn = |app: &mut App, guid: u64| {
            let e = app
                .world_mut()
                .spawn((
                    net_entity(),
                    Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0x2)])),
                ))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
            e
        };
        let escort = spawn(&mut app, ESCORT);
        spawn(&mut app, PLAIN);
        app.update(); // both asked, both marked in `asked`

        // The server's answer: a gold `!` over each.
        for npc in [ESCORT, PLAIN] {
            app.world_mut()
                .resource_mut::<QuestGiver>()
                .set_status(npc, 5);
        }

        // The accept as one update: our quest-log slot (a sweep) and the escort's flag drop.
        *app.world_mut()
            .entity_mut(me)
            .get_mut::<ObjectStore>()
            .unwrap() = ObjectStore(ObjectFields::from_pairs(&[(FIELD_QUEST_LOG_1_1, 938)]));
        *app.world_mut()
            .entity_mut(escort)
            .get_mut::<ObjectStore>()
            .unwrap() = ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0x0)]));
        app.update();

        assert!(
            app.world()
                .resource::<QuestGiver>()
                .status(ESCORT)
                .is_none(),
            "the escortee stopped being a questgiver: its stale AVAILABLE status — and the `!` \
             the marker layer draws from it — must go, sweep or no sweep"
        );
        assert_eq!(
            app.world().resource::<QuestGiver>().status(PLAIN),
            Some(5),
            "the control: a giver whose flag is still on keeps its answer until the server \
             replies to the sweep's fresh query"
        );

        // And it self-heals: when the escort ends and the bit returns, the NPC is asked again.
        *app.world_mut()
            .entity_mut(escort)
            .get_mut::<ObjectStore>()
            .unwrap() = ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0x2)]));
        app.update();
        assert!(
            app.world()
                .resource::<QuestGiver>()
                .status(ESCORT)
                .is_none(),
            "still nothing cached — the answer arrives from the server, not from us"
        );
    }

    /// The light sweep (`0x5eb0a0`, death among its triggers) sends without tearing down; the full
    /// sweep (`0x5eb3f0`: your revive, your own reaction inputs) tears down first, in `0x607380`.
    #[test]
    fn a_light_sweep_re_asks_but_only_a_revive_tears_the_marker_down_first() {
        use crate::net::{Guid, NetEntity};
        use benilla_protocol::{EntityKind, ObjectFields};

        const NPC: u64 = 0x7777;
        const FIELD_LEVEL: u16 = 34;
        const FIELD_HEALTH: u16 = 22;
        const FIELD_FACTION: u16 = 35; // UNIT_FIELD_FACTIONTEMPLATE
        const FIELD_NPC_FLAGS: u16 = 147;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .add_systems(Update, query_statuses);

        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                Guid(1),
                ObjectStore(ObjectFields::from_pairs(&[
                    (FIELD_LEVEL, 5),
                    (FIELD_HEALTH, 100),
                    (FIELD_FACTION, 1),
                ])),
            ))
            .id();
        let npc = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                Guid(NPC),
                ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0x2)])),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(NPC, npc);
        let asked = |app: &mut App| -> usize {
            app.update();
            rx.try_iter()
                .filter(
                    |c| matches!(c, ClientCommand::QuestgiverStatusQuery { npc } if *npc == NPC),
                )
                .count()
        };
        let set_self = |app: &mut App, pairs: &[(u16, u32)]| {
            *app.world_mut()
                .entity_mut(me)
                .get_mut::<ObjectStore>()
                .unwrap() = ObjectStore(ObjectFields::from_pairs(pairs));
        };
        let restore = |app: &mut App| {
            app.world_mut()
                .resource_mut::<QuestGiver>()
                .set_status(NPC, 5);
        };
        let held = |app: &App| app.world().resource::<QuestGiver>().status(NPC);

        assert_eq!(asked(&mut app), 1, "the create-path query");
        restore(&mut app);

        // A packet's light sweep (a turn-in, a reputation or roster change): the `!` stays up.
        app.world_mut().resource_mut::<QuestGiver>().bump_reask();
        assert_eq!(asked(&mut app), 1, "a light sweep re-asks");
        assert_eq!(held(&app), Some(5), "…and does NOT tear the marker down");

        // Death, the `>0 → ≤0` edge, is a light sweep (`0x6046f0`).
        set_self(
            &mut app,
            &[(FIELD_LEVEL, 5), (FIELD_HEALTH, 0), (FIELD_FACTION, 1)],
        );
        assert_eq!(asked(&mut app), 1, "death re-asks");
        assert_eq!(held(&app), Some(5), "…and is still a light sweep");

        // Revive, the `≤0 → >0` edge, is the full sweep.
        set_self(
            &mut app,
            &[(FIELD_LEVEL, 5), (FIELD_HEALTH, 100), (FIELD_FACTION, 1)],
        );
        assert_eq!(asked(&mut app), 1, "revive re-asks");
        assert_eq!(
            held(&app),
            None,
            "…and tears every creature's marker down first (0x5eb3f0 -> 0x607380)"
        );

        // Our own faction moving, the full sweep's other trigger, is outside the generation, so
        // `full` alone raises the sweep.
        restore(&mut app);
        set_self(
            &mut app,
            &[(FIELD_LEVEL, 5), (FIELD_HEALTH, 100), (FIELD_FACTION, 2)],
        );
        assert_eq!(asked(&mut app), 1, "our own faction change sweeps");
        assert_eq!(held(&app), None, "…fully");
    }

    /// The sweep tears down on hostility (`0x5eb143`), the flag deciding only the send; on the real
    /// `FactionTemplate.dbc`.
    #[test]
    fn a_hostile_questgiver_is_torn_down_and_never_asked_about() {
        use crate::net::{Guid, NetEntity};
        use benilla_protocol::{EntityKind, ObjectFields};

        const FRIENDLY: u64 = 0x1001;
        const HOSTILE: u64 = 0x1002;
        const FIELD_LEVEL: u16 = 34;
        const FIELD_FACTION: u16 = 35;
        const FIELD_NPC_FLAGS: u16 = 147;
        /// `FactionTemplate.dbc` 35 is friendly to players, 14 the monster template hostile to all.
        const TPL_FRIENDLY: u32 = 35;
        const TPL_MONSTER: u32 = 14;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_faction_catalog(&mut chain).expect("dbc");

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .insert_resource(crate::target::Factions::from_catalog(catalog))
            .add_systems(Update, query_statuses);

        app.world_mut().spawn((
            SelfPlayer,
            Guid(1),
            // Us: faction template 1 (PLAYER, Human).
            ObjectStore(ObjectFields::from_pairs(&[
                (FIELD_LEVEL, 5),
                (FIELD_FACTION, 1),
            ])),
        ));
        for (guid, tpl) in [(FRIENDLY, TPL_FRIENDLY), (HOSTILE, TPL_MONSTER)] {
            let e = app
                .world_mut()
                .spawn((
                    NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                    Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(&[
                        (FIELD_NPC_FLAGS, 0x2),
                        (FIELD_FACTION, tpl),
                    ])),
                ))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
        }
        let drain = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<u64> {
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::QuestgiverStatusQuery { npc } => Some(npc),
                    _ => None,
                })
                .collect()
        };

        // First sight is a `0x607380` teardown for each, so statuses are seeded after it.
        app.update();
        assert_eq!(
            drain(&rx),
            vec![FRIENDLY],
            "on sight, only the non-hostile questgiver is asked about"
        );

        // Now both wear a marker, and a sweep runs.
        for g in [FRIENDLY, HOSTILE] {
            app.world_mut()
                .resource_mut::<QuestGiver>()
                .set_status(g, 5);
        }
        app.world_mut().resource_mut::<QuestGiver>().bump_reask();
        app.update();
        let asked = drain(&rx);
        let quest = app.world().resource::<QuestGiver>();
        assert_eq!(
            asked,
            vec![FRIENDLY],
            "and the sweep asks about the same one — {asked:x?}"
        );
        assert_eq!(
            quest.status(FRIENDLY),
            Some(5),
            "the control: a friendly giver keeps its marker"
        );
        assert_eq!(
            quest.status(HOSTILE),
            None,
            "and a Hated/Hostile one is torn down (0x5eb143), questgiver flag or not"
        );
    }

    /// `CMSG_TAXINODE_STATUS_QUERY`'s one sender is `0x607380` (`0x6073e8`): on sight, on the
    /// unit's own key moving and on a full sweep, never a light one and never on mouseover
    /// (`0x5eb220` is the callback of a seeder nothing calls). `0x6073f0` clears both markers.
    #[test]
    fn a_flight_master_is_asked_by_the_per_unit_path_only_and_loses_its_green_with_the_gold() {
        use crate::net::{Guid, NetEntity};
        use crate::ui_taxi::FlightMasterStatus;
        use benilla_protocol::{EntityKind, ObjectFields};

        const FM: u64 = 0x9001; // flightmaster only
        const GIVER: u64 = 0x9002; // questgiver only, the control
        const VENDOR: u64 = 0x9003; // neither: never asked
        const FIELD_LEVEL: u16 = 34;
        const FIELD_HEALTH: u16 = 22;
        const FIELD_NPC_FLAGS: u16 = 147;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .add_systems(Update, query_statuses);

        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                Guid(1),
                ObjectStore(ObjectFields::from_pairs(&[
                    (FIELD_LEVEL, 5),
                    (FIELD_HEALTH, 100),
                ])),
            ))
            .id();
        let spawn = |app: &mut App, guid: u64, flags: u32| {
            let e = app
                .world_mut()
                .spawn((
                    NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                    Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, flags)])),
                ))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
            e
        };
        let fm = spawn(&mut app, FM, NPC_FLAG_FLIGHTMASTER);
        spawn(&mut app, GIVER, NPC_FLAG_QUESTGIVER);
        spawn(&mut app, VENDOR, 0x4);

        let drain = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> (Vec<u64>, Vec<u64>) {
            let (mut taxi, mut giver) = (Vec::new(), Vec::new());
            for c in rx.try_iter() {
                match c {
                    ClientCommand::TaxiNodeStatusQuery { guid } => taxi.push(guid),
                    ClientCommand::QuestgiverStatusQuery { npc } => giver.push(npc),
                    _ => {}
                }
            }
            (taxi, giver)
        };

        // On sight: the create path is a `0x607380`, so each service is asked once.
        app.update();
        let (taxi, giver) = drain(&rx);
        assert_eq!(taxi, vec![FM], "the flight master is asked on sight");
        assert_eq!(
            giver,
            vec![GIVER],
            "and so is the questgiver — the vendor never"
        );

        app.update();
        assert_eq!(drain(&rx), (vec![], vec![]), "then quiet");

        // A light sweep re-asks the questgiver; `0x5eb0a0` never tests bit `0x8`.
        app.world_mut().resource_mut::<QuestGiver>().bump_reask();
        app.update();
        let (taxi, giver) = drain(&rx);
        assert_eq!(giver, vec![GIVER], "a light sweep re-asks the questgiver");
        assert_eq!(
            taxi,
            vec![] as Vec<u64>,
            "…and never the flight master: 0x1aa has one sender site and it is not in the sweep"
        );

        // With the green marker up, a full sweep (a revive) takes it down and re-asks.
        app.world_mut()
            .entity_mut(fm)
            .insert(FlightMasterStatus { known: false });
        let set_self = |app: &mut App, hp: u32| {
            *app.world_mut()
                .entity_mut(me)
                .get_mut::<ObjectStore>()
                .unwrap() = ObjectStore(ObjectFields::from_pairs(&[
                (FIELD_LEVEL, 5),
                (FIELD_HEALTH, hp),
            ]));
        };
        set_self(&mut app, 0); // death: a light sweep
        app.update();
        assert_eq!(
            drain(&rx).0,
            vec![] as Vec<u64>,
            "death does not re-ask taxi"
        );
        assert!(
            app.world().entity(fm).get::<FlightMasterStatus>().is_some(),
            "…and a light sweep leaves the green marker up"
        );

        set_self(&mut app, 100); // revive: a full sweep
        app.update();
        assert_eq!(drain(&rx).0, vec![FM], "a revive re-asks the flight master");
        assert!(
            app.world().entity(fm).get::<FlightMasterStatus>().is_none(),
            "…after tearing its green marker down first (0x607384, the shared 0x6073f0)"
        );

        // And its own key moving is a `0x607380` too: the flightmaster bit going off takes the
        // marker with it, and does not re-ask.
        app.world_mut()
            .entity_mut(fm)
            .insert(FlightMasterStatus { known: false });
        *app.world_mut()
            .entity_mut(fm)
            .get_mut::<ObjectStore>()
            .unwrap() = ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0)]));
        app.update();
        assert_eq!(
            drain(&rx).0,
            vec![] as Vec<u64>,
            "no longer a flight master"
        );
        assert!(
            app.world().entity(fm).get::<FlightMasterStatus>().is_none(),
            "and its green marker goes with the bit"
        );
    }

    /// A GameObject's only send sites sit in sweep callbacks: no query at first sight, where the
    /// unit control asks, and one per sweep.
    #[test]
    fn a_gameobject_is_asked_on_a_sweep_and_never_at_first_sight() {
        use crate::net::{Guid, NetEntity};
        use benilla_protocol::ObjectFields;

        /// The Goldshire Wanted Poster (template 68, spawn 26843) as the server sends it, flags
        /// `INTERACT_COND | NODESPAWN`, read off the wire by `WOW_PROBE_GOQUEST`.
        const POSTER: u64 = 0xf110_0000_0044_68db;
        const POSTER_FLAGS: u32 = 0x24;
        /// A GameObject next to it with no quest condition: a plain door, `NODESPAWN` only.
        const DOOR: u64 = 0xf110_0000_0045_0001;
        const DOOR_FLAGS: u32 = 0x20;
        /// The control: a creature questgiver.
        const NPC: u64 = 0x2222;
        const FIELD_LEVEL: u16 = 34;
        const FIELD_NPC_FLAGS: u16 = 147;
        const FIELD_GAMEOBJECT_FLAGS: u16 = 9;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .add_systems(Update, query_statuses);

        let spawn = |app: &mut App, guid: u64, kind: EntityKind, fields: &[(u16, u32)]| {
            let e = app
                .world_mut()
                .spawn((
                    NetEntity {
                        kind,
                        display_id: None,
                        scale: 1.0,
                    },
                    Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(fields)),
                ))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
        };
        app.world_mut().spawn((
            SelfPlayer,
            Guid(1),
            ObjectStore(ObjectFields::from_pairs(&[(FIELD_LEVEL, 5)])),
        ));
        let asked = |app: &mut App| -> Vec<u64> {
            app.update();
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::QuestgiverStatusQuery { npc } => Some(npc),
                    _ => None,
                })
                .collect()
        };

        // The first frame in a fresh world is a sweep, so first sight comes a frame later.
        assert!(asked(&mut app).is_empty(), "nothing in view yet");

        spawn(
            &mut app,
            POSTER,
            EntityKind::GameObject,
            &[(FIELD_GAMEOBJECT_FLAGS, POSTER_FLAGS)],
        );
        spawn(
            &mut app,
            DOOR,
            EntityKind::GameObject,
            &[(FIELD_GAMEOBJECT_FLAGS, DOOR_FLAGS)],
        );
        spawn(
            &mut app,
            NPC,
            EntityKind::Unit,
            &[(FIELD_NPC_FLAGS, NPC_FLAG_QUESTGIVER)],
        );

        assert_eq!(
            asked(&mut app),
            vec![NPC],
            "first sight: the creature is asked from its own create path, the poster is NOT — \
             the GameObject class has no bring-up query"
        );
        assert!(
            asked(&mut app).is_empty(),
            "and the frames after it stay silent"
        );

        // A sweep, through the packet epoch.
        app.world_mut().resource_mut::<QuestGiver>().bump_reask();
        let swept = asked(&mut app);
        assert!(
            swept.contains(&POSTER),
            "the sweep asks about the poster — {swept:x?}"
        );
        assert!(
            swept.contains(&NPC),
            "and re-asks the creature, as it always did — {swept:x?}"
        );
        assert!(
            !swept.contains(&DOOR),
            "but never the GameObject without GAMEOBJECT_FLAGS bit 2 — {swept:x?}"
        );
        assert_eq!(
            swept.iter().filter(|g| **g == POSTER).count(),
            1,
            "exactly once per sweep"
        );

        assert!(
            asked(&mut app).is_empty(),
            "between sweeps, nothing — a GameObject has no per-object key to re-ask on"
        );

        // Not one-shot: the reference keeps no per-GameObject memo of having asked.
        app.world_mut().resource_mut::<QuestGiver>().bump_reask();
        assert!(
            asked(&mut app).contains(&POSTER),
            "and again on the next sweep"
        );
    }

    #[test]
    fn every_recorded_trigger_re_asks_exactly_once() {
        use crate::net::{Guid, NetEntity};
        use benilla_protocol::{EntityKind, ObjectFields};

        const NPC: u64 = 0x1234;
        const FIELD_HEALTH: u16 = 22; // UNIT_FIELD_HEALTH
        const FIELD_NPC_FLAGS: u16 = 147;
        const FIELD_FACTION: u16 = 35; // UNIT_FIELD_FACTIONTEMPLATE
        const FIELD_PLAYER_FLAGS: u16 = 190;
        const FIELD_COINAGE: u16 = 1176;
        const FIELD_SKILL_1_1: u16 = 718;
        const FIELD_INV_SLOT_0: u16 = 486; // PLAYER_FIELD_INV_SLOT_HEAD

        let net_entity = || NetEntity {
            kind: EntityKind::Unit,
            display_id: None,
            scale: 1.0,
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .add_systems(Update, query_statuses);

        // Every watched field starts present, so each leg below is a change, not an arrival.
        let base_self = vec![
            (34u16, 5u32),
            (FIELD_HEALTH, 100),
            (FIELD_PLAYER_FLAGS, 0),
            (FIELD_COINAGE, 500),
            (FIELD_SKILL_1_1, 186 | (1 << 16)), // skill id 186, step 1
            (FIELD_SKILL_1_1 + 1, 75 | (300 << 16)), // value 75 / max 300
            (FIELD_INV_SLOT_0, 0xaaaa),
            (FIELD_INV_SLOT_0 + 1, 0),
        ];
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                net_entity(),
                Guid(1),
                ObjectStore(ObjectFields::from_pairs(&base_self)),
            ))
            .id();
        let npc_e = app
            .world_mut()
            .spawn((
                net_entity(),
                Guid(NPC),
                ObjectStore(ObjectFields::from_pairs(&[
                    (FIELD_NPC_FLAGS, 0x2),
                    (FIELD_FACTION, 35),
                ])),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(NPC, npc_e);

        let asked = |app: &mut App| -> usize {
            app.update();
            rx.try_iter()
                .filter(
                    |c| matches!(c, ClientCommand::QuestgiverStatusQuery { npc } if *npc == NPC),
                )
                .count()
        };
        // One field replaced, the rest kept: a values-update delta's shape.
        let set_self = |app: &mut App, field: u16, value: u32| {
            let mut pairs = base_self.clone();
            match pairs.iter_mut().find(|(f, _)| *f == field) {
                Some(p) => p.1 = value,
                None => pairs.push((field, value)),
            }
            *app.world_mut()
                .entity_mut(me)
                .get_mut::<ObjectStore>()
                .unwrap() = ObjectStore(ObjectFields::from_pairs(&pairs));
        };

        assert_eq!(asked(&mut app), 1, "the first sight");
        assert_eq!(asked(&mut app), 0, "then quiet");

        for (label, field, value) in [
            ("death", FIELD_HEALTH, 0u32), // the alive bit, not the raw value
            ("PLAYER_FLAGS", FIELD_PLAYER_FLAGS, 0x10),
            ("money", FIELD_COINAGE, 900),
            ("a skill rank", FIELD_SKILL_1_1 + 1, 80 | (300 << 16)),
            ("an equipped item", FIELD_INV_SLOT_0, 0xbbbb),
        ] {
            set_self(&mut app, field, value);
            assert_eq!(asked(&mut app), 1, "{label} changed — re-ask");
            assert_eq!(asked(&mut app), 0, "{label}: exactly once");
        }

        // The packet half: reputation, the group roster and the quest packets all land here.
        app.world_mut().resource_mut::<QuestGiver>().bump_reask();
        assert_eq!(asked(&mut app), 1, "a swept packet — re-ask");
        assert_eq!(asked(&mut app), 0, "exactly once");

        // Per unit: its flightmaster bit, then its faction template.
        let set_npc = |app: &mut App, pairs: &[(u16, u32)]| {
            *app.world_mut()
                .entity_mut(npc_e)
                .get_mut::<ObjectStore>()
                .unwrap() = ObjectStore(ObjectFields::from_pairs(pairs));
        };
        set_npc(
            &mut app,
            &[(FIELD_NPC_FLAGS, 0x2 | 0x8), (FIELD_FACTION, 35)],
        );
        assert_eq!(asked(&mut app), 1, "its flightmaster bit — re-ask that one");
        assert_eq!(asked(&mut app), 0, "exactly once");

        set_npc(
            &mut app,
            &[(FIELD_NPC_FLAGS, 0x2 | 0x8), (FIELD_FACTION, 12)],
        );
        assert_eq!(asked(&mut app), 1, "its faction template — re-ask that one");
        assert_eq!(asked(&mut app), 0, "exactly once");
    }
}
