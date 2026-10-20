//! The taxi map's app side: [`TaxiState`] holds the `SMSG_SHOWTAXINODES` map and the one-shot
//! replies, [`feed_taxi`] pushes the known nodes on the current node's continent with their
//! routes (built in [`routing`]) and fires the map events, and [`drain_taxi`] sends the activate.

use benilla_protocol::messages::TaxiMask;
use bevy::prelude::*;

use benilla_ui::script::{TaxiUiState, UiScript};

use crate::names::NameCache;
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::player::UNIT_FLAG_TAXI_FLIGHT;
use crate::query_cache::Ask;
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};
use crate::ui_session::{CoreCloses, LatchApp, LatchedWindow, NpcWindow};

mod routing;
use routing::{build_nodes_for, load_taxi_catalogs, taxi_error_key, TaxiCatalogs, TaxiRouteCache};

/// The open map, `SMSG_SHOWTAXINODES` as the wire gave it.
pub(crate) struct TaxiOpen {
    pub(crate) flightmaster: u64,
    /// The node nearest the flight master, typed `Current`.
    pub(crate) nearest_node: u32,
    /// The known nodes: the only ones listed, and the only ones a route passes through.
    pub(crate) known: TaxiMask,
}

/// The taxi session: the open map, plus the one-shot replies [`feed_taxi`] consumes.
#[derive(Resource, Default)]
pub(crate) struct TaxiState {
    pub(crate) open: Option<TaxiOpen>,
    /// The last `SMSG_ACTIVATETAXIREPLY` code, for [`feed_taxi`] to show, or on `OK` to close the
    /// map.
    pub(crate) reply: Option<u32>,
    /// A first-visit learn (`SMSG_NEW_TAXI_PATH`) for [`feed_taxi`] to announce.
    pub(crate) discovered: bool,
}

impl TaxiState {
    pub(crate) fn open(&mut self, flightmaster: u64, nearest_node: u32, known: TaxiMask) {
        self.open = Some(TaxiOpen {
            flightmaster,
            nearest_node,
            known,
        });
    }

    /// A client-side close; the staged replies stay.
    pub(crate) fn clear(&mut self) {
        self.open = None;
    }

    /// The disconnect clear, staged replies included.
    pub(crate) fn clear_session(&mut self) {
        self.open = None;
        self.reply = None;
        self.discovered = false;
    }
}

/// The flight master's cell `[0xbb4ab8]`; `CloseTaxiMap` (`0x4dcce0`) is the core itself.
impl LatchedWindow for TaxiState {
    const WINDOW: NpcWindow = NpcWindow::Taxi;

    fn cell(&self) -> Option<u64> {
        self.open.as_ref().map(|o| o.flightmaster)
    }

    fn dispatched(world: &mut World) {
        close_core(world);
    }

    fn take_close_verb(script: &mut UiScript) -> bool {
        script.take_taxi_close()
    }
}

/// The close core `0x4dc0f0`: with a map open, the latch release (`0x4dc105`), the cell cleared
/// (`0x4dc113`) and each slot's route freed (`0x4dc130`-`0x4dc145`), then `TAXIMAP_CLOSED`
/// (`0x4dc15f`). The slots stay, their fares 0 with no flight master
/// ([`UiScript::close_taxi`]). An activate's `OK` (`0x5ed203`), a map from another flight master
/// (`0x4db73a`) and the leave-world sweep's leg ([`leave_world`]) call it as it is.
pub(crate) fn close_core(world: &mut World) {
    let Some(flightmaster) = world.resource::<TaxiState>().cell() else {
        return;
    };
    crate::ui_session::release(world, flightmaster);
    world.resource_mut::<TaxiState>().clear();
    if let Some(mut cache) = world.get_resource_mut::<TaxiRouteCache>() {
        for slot in &mut cache.slots {
            slot.chain.clear();
        }
    }
    crate::ui_session::note_closed(world, NpcWindow::Taxi);
    crate::ui_session::fire(world, |script| {
        script.close_taxi();
        script.fire_event("TAXIMAP_CLOSED", vec![]);
    });
}

/// The leave-world sweep's leg `0x4db700`: the free `0x4db630` (the map art at `0x4db6d0`, the
/// lines, and the slot count to 0 at `0x4db6bf`), then the close core, which finds no slot.
fn leave_world(world: &mut World) {
    if let Some(mut cache) = world.get_resource_mut::<TaxiRouteCache>() {
        cache.slots.clear();
    }
    if let Some(mut script) = world.get_non_send_resource_mut::<UiScript>() {
        script.set_taxi(None);
    }
    close_core(world);
}

/// `SMSG_SHOWTAXINODES`'s open (`0x4db710`): a map already on this flight master takes nothing
/// (`0x4db730`); one on another is closed first through the core (`0x4db73a`); then the slots
/// reset for [`feed_taxi`]'s build (`0x4db748`), the latch (`0x4dba5c`) and the cell
/// (`0x4dba63`).
pub(crate) fn open_map(world: &mut World, flightmaster: u64, nearest_node: u32, known: TaxiMask) {
    match world.resource::<TaxiState>().cell() {
        Some(open) if open == flightmaster => return,
        Some(_) => close_core(world),
        None => {}
    }
    if let Some(mut cache) = world.get_resource_mut::<TaxiRouteCache>() {
        cache.slots.clear();
    }
    if let Some(mut script) = world.get_non_send_resource_mut::<UiScript>() {
        script.reset_taxi_slots();
    }
    crate::ui_session::register(world, flightmaster);
    world
        .resource_mut::<TaxiState>()
        .open(flightmaster, nearest_node, known);
}

/// A flight master's `SMSG_TAXINODE_STATUS`: `known = false` shows the green `TalkToMeGreen`
/// overhead icon (`0x5ecdd0` to `0x607480`, resource table `0xc4d9d8` index 4). The query and the
/// teardown belong to [`crate::quest_markers::query`]: the reference's only sender, `0x5eb170`, is
/// called only from `0x607380` (at `0x6073e8`), the per-unit function that also sends the
/// questgiver query, and both icons share one marker slot (`unit+0xb2c`).
#[derive(Component, Clone, Copy)]
pub(crate) struct FlightMasterStatus {
    pub(crate) known: bool,
}

/// Push the map into the VM and fire its events, show an activate refusal or close on `OK`, and
/// announce a first-visit discovery.
fn feed_taxi(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut state: ResMut<TaxiState>,
    catalogs: Option<Res<TaxiCatalogs>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut cache: ResMut<TaxiRouteCache>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    relations: crate::target::RelationWorld,
    mut last: crate::ui_script::WindowMemo<Option<TaxiUiState>>,
    mut last_name: crate::ui_script::WindowMemo<Option<String>>,
    mut closes: CoreCloses,
    mut sink: crate::ui_action::MessageSink,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        let last = last.get(script);
        let last_name = last_name.get(script);
        if closes.since_last(script, NpcWindow::Taxi) {
            *last = None;
            *last_name = None;
        }

        // The activate reply: a refusal shows where its message row says; `OK` closed the map in
        // its handler, through the core.
        if let Some(code) = state.reply.take() {
            if let Some(key) = taxi_error_key(code) {
                let text = script
                    .lua()
                    .globals()
                    .get::<String>(key)
                    .unwrap_or_default();
                if !text.is_empty() {
                    crate::ui_action::show_messages(
                        script,
                        &mut sink,
                        "ui_taxi",
                        [crate::ui_action::Shown::keyed(key, text)],
                    );
                }
            }
        }

        // The first-visit learn: the reference shows message `0xf2`, `ERR_NEWTAXIPATH`, whose row
        // routes it to the yellow `UI_INFO_MESSAGE` (`0x4945b0`) and plays its `TaxiNodeDiscovered`
        // sound kit (`0x458030`); the catalog row carries both.
        if std::mem::take(&mut state.discovered) {
            let text = script
                .lua()
                .globals()
                .get::<String>("ERR_NEWTAXIPATH")
                .unwrap_or_default();
            if !text.is_empty() {
                crate::ui_action::show_messages(
                    script,
                    &mut sink,
                    "ui_taxi",
                    [crate::ui_action::Shown::keyed("ERR_NEWTAXIPATH", text)],
                );
            }
        }

        let Some(catalogs) = catalogs else {
            return;
        };

        // A closed map keeps its slots for the verbs (`0x4dc0f0`) and the next open resets them
        // ([`open_map`]); only the memo goes, so that open pushes and fires anew.
        let Some(open) = state.open.as_ref() else {
            *last = None;
            *last_name = None;
            return;
        };
        let flightmaster = index
            .0
            .get(&open.flightmaster)
            .and_then(|&e| stores.get(e).ok());

        // The continent is the current node's own, as the packet left it (`build_nodes_for`). The
        // fares take the flight master's price discount for the player (`0x4dc45d`, `0x4dc4a2`),
        // and the route search skips the other side's nodes by the player's race (`0x4dbd2d`). A
        // map that builds no slot shows nothing and fires nothing (`0x4dbab0`).
        let discount = (|| {
            let (master, player, me) = (flightmaster?, self_q.iter().next()?, relations.me()?);
            let master = relations.unit_of(open.flightmaster, master);
            Some(crate::target::vendor_price_discount(
                &relations.get(),
                &master,
                &me,
                player,
            ))
        })();
        let player = relations
            .me()
            .zip(relations.factions())
            .map(|(me, factions)| (me.race, factions.catalog()));
        let Some((map_id, nodes, resolved)) = build_nodes_for(open, &catalogs, discount, player)
        else {
            return;
        };
        cache.current = open.nearest_node;
        cache.slots = resolved;
        let fresh = TaxiUiState {
            art: format!("Interface\\TaxiFrame\\TAXIMAP{map_id}"),
            nodes,
        };

        // The flight master's name is tracked so its landing re-fires `TAXIMAP_OPENED`, whose stock
        // handler titles the map with `UnitName("npc")` (`TaxiFrame.lua:27`); the reference fires
        // it only on open. `UnitName("npc")` looks it up with a callback (`0x5171b8`), at the
        // open's event.
        let flightmaster_name = names
            .resolve_unit(
                open.flightmaster,
                flightmaster,
                Ask::at(last.is_none(), &commands),
            )
            .map(str::to_string);
        if last.as_ref() != Some(&fresh) || *last_name != flightmaster_name {
            script.set_taxi(Some(fresh.clone()));
            // No arguments: the event's one fire site, `0x4dba96`, is a plain
            // `FrameScript_SignalEvent` (`0x703e50`). The close is the core's.
            script.fire_event("TAXIMAP_OPENED", Vec::new());
            *last = Some(fresh);
            *last_name = flightmaster_name;
        }
    })
}

/// Push `UnitOnTaxi("player")`: the reference's verb (`0x517a40`) answers bit 20 of
/// `UNIT_FIELD_FLAGS` (`0x517a86`), which vmangos sets only for a flight
/// (`FlightPathMovementGenerator`). Any other server spline, such as a fear, is not a taxi: stock
/// `UIParent.lua:474-478` closes the windows on `PLAYER_CONTROL_LOST` unless on a taxi. No self
/// store reads as not on a taxi.
fn feed_on_taxi(
    mut hosted_vm: crate::script_calls::HostedVm,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut last: Local<crate::ui_script::VmMemo<Option<bool>>>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        let last = last.get(script);
        let on_taxi = self_q
            .iter()
            .next()
            .is_some_and(|s| s.0.unit_flags() & UNIT_FLAG_TAXI_FLIGHT != 0);
        if *last != Some(on_taxi) {
            script.set_on_taxi(on_taxi);
            *last = Some(on_taxi);
        }
    })
}

/// Drain the Lua intents. `TakeTaxiNode(i)` (`0x4dc8d0`) takes a slot under the count
/// (`0x4dc8dc`), then in order: a mounted player is refused with `0xb2`,
/// `ERR_TAXIPLAYERALREADYMOUNTED` (`0x4dc909`-`0x4dc918`), and the current node with `0xaa`,
/// `ERR_TAXISAMENODE` (`0x4dc939`-`0x4dc942`); a `TaxiPath` from the current node to the slot's
/// sends `CMSG_ACTIVATETAXI`, routed or not (`0x4dc952`, `0x4dc9be`); else the slot's route sends
/// `CMSG_ACTIVATETAXIEXPRESS` with its raw fare sum (`0x4dc965`-`0x4dc98e`); and a slot with
/// neither shows `0xab`, `ERR_TAXINOSUCHPATH` (`0x4dc999`). `CloseTaxiMap()` runs the core, with
/// no packet.
fn drain_taxi(
    mut hosted_vm: crate::script_calls::HostedVm,
    state: Res<TaxiState>,
    cache: Res<TaxiRouteCache>,
    catalogs: Option<Res<TaxiCatalogs>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
    mut world_commands: Commands,
    mut sink: crate::ui_action::MessageSink,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        // The flight master's cell, cleared by a close that keeps the slots (`0x4dc113`).
        let guid = state.open.as_ref().map_or(0, |open| open.flightmaster);
        for idx in script.take_taxi_node() {
            let Some(slot) = idx.checked_sub(1).and_then(|i| cache.slots.get(i)) else {
                continue;
            };
            // `UNIT_FIELD_MOUNTDISPLAYID` read signed (`[+0x110]+0x1fc`, `jle` at `0x4dc911`).
            let mounted = self_q
                .iter()
                .next()
                .is_some_and(|s| s.0.unit_mount_display_id() as i32 > 0);
            let (src, dest) = (cache.current, slot.node);
            let direct = catalogs
                .as_ref()
                .is_some_and(|c| c.paths.between(src, dest).is_some());
            let refusal = if mounted {
                Some("ERR_TAXIPLAYERALREADYMOUNTED")
            } else if dest == src {
                Some("ERR_TAXISAMENODE")
            } else if !direct && slot.chain.is_empty() {
                Some("ERR_TAXINOSUCHPATH")
            } else {
                None
            };
            if let Some(key) = refusal {
                debug!("ui_taxi: TakeTaxiNode({idx}) refused — {key}");
                let line = crate::ui_action::keyed_line(script, key);
                crate::ui_action::show_messages(script, &mut sink, "ui_taxi", line);
                continue;
            }
            if direct {
                debug!(
                    "ui_taxi: activate {src} -> {dest} (direct path, {} copper)",
                    slot.cost
                );
                let _ = commands.0.send(ClientCommand::ActivateTaxi {
                    guid,
                    source_node: src,
                    dest_node: dest,
                });
            } else {
                debug!(
                    "ui_taxi: activate express {:?} ({} copper)",
                    slot.chain, slot.cost
                );
                let _ = commands.0.send(ClientCommand::ActivateTaxiExpress {
                    guid,
                    total_cost: slot.cost,
                    nodes: slot.chain.clone(),
                });
            }
        }
        if script.take_taxi_close() {
            debug!("ui_taxi: client-side close (no packet)");
            crate::ui_session::queue(&mut world_commands, close_core);
        }
    })
}

mod net;

pub(crate) struct UiTaxiPlugin;

impl Plugin for UiTaxiPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<TaxiState>()
            .init_resource::<TaxiRouteCache>()
            .add_latched_window::<TaxiState>()
            .add_sweep_leg(SweepLeg::Taxi, leave_world)
            .add_systems(
                Update,
                (
                    load_taxi_catalogs,
                    // After the roster publish, whose party slots the master's reaction reads.
                    feed_taxi
                        .in_set(UiFeed)
                        .after(crate::spell::group_relation::publish_group_roster),
                    feed_on_taxi.in_set(UiFeed),
                    drain_taxi.in_set(crate::ui_script::UiDrain),
                ),
            );
    }
}

#[cfg(test)]
mod drain_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::field::FIELD_UNIT_FLAGS;
    use benilla_protocol::ObjectFields;

    /// `1` with the bit, `nil` without it; the feed reads nothing about server splines.
    #[test]
    fn unit_on_taxi_reads_the_taxi_flight_flag() {
        let mut app = App::new();
        app.add_systems(Update, feed_on_taxi);
        app.insert_non_send_resource(UiScript::new().unwrap());
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                ObjectStore(ObjectFields::from_pairs(&[(FIELD_UNIT_FLAGS, 0x1000)])),
            ))
            .id();
        let on_taxi = |app: &mut App| -> bool {
            app.update();
            let s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.eval::<Option<i64>>(r#"return UnitOnTaxi("player")"#)
                .unwrap()
                .is_some()
        };
        assert!(!on_taxi(&mut app), "no TAXI_FLIGHT bit: nil");

        app.world_mut()
            .entity_mut(me)
            .insert(ObjectStore(ObjectFields::from_pairs(&[(
                FIELD_UNIT_FLAGS,
                0x1000 | UNIT_FLAG_TAXI_FLIGHT,
            )])));
        assert!(on_taxi(&mut app), "the bit set: 1");

        app.world_mut()
            .entity_mut(me)
            .insert(ObjectStore(ObjectFields::from_pairs(&[(
                FIELD_UNIT_FLAGS,
                0,
            )])));
        assert!(!on_taxi(&mut app), "landed: nil again");
    }

    /// The map's fares take the flight master's discount and the express leg sends the raw route
    /// sum (`0x4dc3d0` against `0x4dc96f`). Honored, a PvP-flagged flight master and honor rank
    /// byte 6 make 0.15: Sentinel Hill's direct 110 shows 94, and Thelsamar's route through
    /// Ironforge, 50 + 110, shows 136 and sends 160. With the flight master unresolved the fares
    /// show 0 (`0x4dc423`).
    #[test]
    fn the_map_shows_discounted_fares_and_the_express_sends_the_raw_sum() {
        use benilla_protocol::field::FIELD_UNIT_FACTIONTEMPLATE;
        use bevy::ecs::system::RunSystemOnce;

        /// `UNIT_FIELD_BYTES_0`, `PLAYER_BYTES_3`: absolute descriptor indices.
        const BYTES_0: u16 = 36;
        const PLAYER_BYTES_3: u16 = 195;
        const MASTER: u64 = 0xF130_0000_0000_0099;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalogs = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
        let (factions, template, reps) = crate::target::stormwind_fixture(&mut chain, 9000);

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TaxiState>()
            .init_resource::<TaxiRouteCache>()
            .init_resource::<GuidIndex>()
            .init_resource::<NameCache>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .insert_resource(NetCommands(tx))
            .insert_resource(catalogs)
            .insert_resource(factions)
            .insert_resource(reps)
            .insert_non_send_resource(UiScript::new().unwrap());
        // A human warrior player: the master's standing leg needs the player's type, flag `0x8`
        // and template (`0x606576`-`0x6065b6`).
        app.world_mut().spawn((
            SelfPlayer,
            crate::net::Guid(0x10),
            ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x19), // OBJECT_FIELD_TYPE
                (FIELD_UNIT_FACTIONTEMPLATE, 1),
                (benilla_protocol::field::FIELD_UNIT_FLAGS, 0x8),
                (BYTES_0, crate::target::HUMAN_WARRIOR),
                (PLAYER_BYTES_3, 6 << 24),
            ])),
        ));
        let master = app
            .world_mut()
            .spawn((
                crate::net::Guid(MASTER),
                ObjectStore(ObjectFields::from_pairs(&[
                    (FIELD_UNIT_FACTIONTEMPLATE, template),
                    (FIELD_UNIT_FLAGS, 0x1000),
                ])),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(MASTER, master);
        // At Stormwind (2), knowing Sentinel Hill (4), Ironforge (6) and Thelsamar (8).
        app.world_mut().resource_mut::<TaxiState>().open(
            MASTER,
            2,
            TaxiMask([0b1010_1010, 0, 0, 0, 0, 0, 0, 0]),
        );

        let node = |app: &mut App, name: &str| -> (i64, i64) {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval(&format!(
                    "for i = 1, NumTaxiNodes() do \
                       if TaxiNodeName(i) == {name:?} then return i, TaxiNodeCost(i) end \
                     end"
                ))
                .unwrap()
        };
        app.world_mut().run_system_once(feed_taxi).unwrap();
        assert_eq!(node(&mut app, "Sentinel Hill, Westfall").1, 94);
        assert_eq!(node(&mut app, "Ironforge, Dun Morogh").1, 42);
        let (thelsamar, shown) = node(&mut app, "Thelsamar, Loch Modan");
        assert_eq!(shown, 136);

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(&format!("TakeTaxiNode({thelsamar})"))
            .unwrap();
        app.world_mut().run_system_once(drain_taxi).unwrap();
        // The feed's name lookup queries the flight master; only the activates matter here.
        let sent: Vec<_> = rx
            .try_iter()
            .filter(|c| {
                matches!(
                    c,
                    ClientCommand::ActivateTaxi { .. } | ClientCommand::ActivateTaxiExpress { .. }
                )
            })
            .collect();
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::ActivateTaxiExpress { total_cost: 160, nodes, .. }]
                    if nodes == &[2, 6, 8]
            ),
            "the express carries the undiscounted sum, got {sent:?}"
        );

        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .remove(&MASTER);
        app.world_mut().run_system_once(feed_taxi).unwrap();
        assert_eq!(node(&mut app, "Sentinel Hill, Westfall").1, 0);
    }

    /// The open fires `TAXIMAP_OPENED`, whose handler titles the map with `UnitName("npc")`, which
    /// looks the name up with a callback (`0x5171b8`); the frames between read. A refused name is
    /// asked by no frame, and again by the next open.
    #[test]
    fn a_refused_flight_master_name_is_asked_at_the_next_open_never_by_a_frame() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalogs = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
        // Not streamed, so its name is the creature cache's by the guid's entry bits.
        const MASTER: u64 = 0xF130_0000_4500_0001;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TaxiState>()
            .init_resource::<TaxiRouteCache>()
            .init_resource::<GuidIndex>()
            .init_resource::<NameCache>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .insert_resource(NetCommands(tx))
            .insert_resource(catalogs)
            .insert_non_send_resource(UiScript::new().unwrap())
            .add_systems(Update, feed_taxi);
        let open = |app: &mut App| {
            app.world_mut().resource_mut::<TaxiState>().open(
                MASTER,
                2,
                TaxiMask([0b1010_1010, 0, 0, 0, 0, 0, 0, 0]),
            );
        };
        open(&mut app);
        app.update();
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_creature(0x45, None);
        for _ in 0..600 {
            app.update();
        }
        assert!(crate::query_cache::queries(&rx).is_empty());
        app.world_mut().resource_mut::<TaxiState>().clear();
        app.update();
        open(&mut app);
        app.update();
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
    }

    #[test]
    fn open_close_and_session_clear() {
        let mut state = TaxiState::default();
        assert_eq!(state.cell(), None);

        state.open(0x42, 2, TaxiMask([0xA, 0, 0, 0, 0, 0, 0, 0]));
        assert_eq!(state.cell(), Some(0x42));
        assert_eq!(state.open.as_ref().unwrap().nearest_node, 2);

        // A client-side close drops the map, nothing else.
        state.reply = Some(3);
        state.discovered = true;
        state.clear();
        assert_eq!(state.cell(), None);
        assert_eq!(
            state.reply,
            Some(3),
            "close leaves staged reply/discovery alone"
        );

        // Disconnect drops everything.
        state.open(0x42, 2, TaxiMask::default());
        state.clear_session();
        assert_eq!(state.cell(), None);
        assert_eq!(state.reply, None);
        assert!(!state.discovered);
    }
}
