//! The taxi map's app side: [`TaxiState`] holds the `SMSG_SHOWTAXINODES` map and the one-shot
//! replies, [`feed_taxi`] pushes the known nodes on the current node's continent with their
//! routes (built in [`routing`]) and fires the map events, and [`drain_taxi`] sends the activate.

use benilla_protocol::messages::TaxiMask;
use bevy::prelude::*;

use benilla_ui::script::{TaxiUiState, UiScript};

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::player::UNIT_FLAG_TAXI_FLIGHT;
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

mod routing;
use routing::{build_nodes, load_taxi_catalogs, taxi_error_key, TaxiCatalogs, TaxiRouteCache};

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

/// A flight master's `SMSG_TAXINODE_STATUS`: `known = false` shows the green `TalkToMeGreen`
/// overhead icon (`0x5ecdd0` to `0x607480`, resource table `0xc4d9d8` index 4). The query and the
/// teardown belong to [`crate::quest_markers::query`]: the reference's only sender, `0x5eb170`, is
/// called only from `0x607380` (at `0x6073e8`), the per-unit function that also sends the
/// questgiver query, and both icons share one marker slot (`unit+0xb2c`).
#[derive(Component, Clone, Copy)]
pub(crate) struct FlightMasterStatus {
    pub(crate) known: bool,
}

/// The range guard closes the map, as the close button does, when the flight master is out of
/// range or gone.
impl NpcSession for TaxiState {
    fn npc(&self) -> Option<u64> {
        self.open.as_ref().map(|o| o.flightmaster)
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// Push the map into the VM and fire its events, show an activate refusal or close on `OK`, and
/// announce a first-visit discovery.
fn feed_taxi(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<TaxiState>,
    catalogs: Option<Res<TaxiCatalogs>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut cache: ResMut<TaxiRouteCache>,
    mut last: Local<crate::ui_script::VmMemo<Option<TaxiUiState>>>,
    mut last_name: Local<crate::ui_script::VmMemo<Option<String>>>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_name = last_name.get(&script);

    // The activate reply: a refusal shows where its message row says; `OK` closes the map, as
    // the reference's handler does for code 0.
    if let Some(code) = state.reply.take() {
        match taxi_error_key(code) {
            Some(key) => {
                let text = script
                    .lua()
                    .globals()
                    .get::<String>(key)
                    .unwrap_or_default();
                if !text.is_empty() {
                    crate::ui_action::show_messages(
                        &mut script,
                        &mut sink,
                        "ui_taxi",
                        [crate::ui_action::Shown::keyed(key, text)],
                    );
                }
            }
            None => state.open = None,
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
                &mut script,
                &mut sink,
                "ui_taxi",
                [crate::ui_action::Shown::keyed("ERR_NEWTAXIPATH", text)],
            );
        }
    }

    let Some(catalogs) = catalogs else {
        return;
    };

    // The continent is the current node's own, as the packet left it (`build_nodes`).
    let fresh = state.open.as_ref().and_then(|open| {
        let (map_id, nodes, resolved) = build_nodes(open, &catalogs)?;
        cache.0 = resolved;
        Some(TaxiUiState {
            art: format!("Interface\\TaxiFrame\\TAXIMAP{map_id}"),
            nodes,
        })
    });
    if fresh.is_none() {
        cache.0.clear();
    }

    // The flight master's name is tracked so its landing re-fires `TAXIMAP_OPENED`, whose stock
    // handler titles the map with `UnitName("npc")` (`TaxiFrame.lua:27`); the reference fires it
    // only on open.
    let flightmaster_name = state
        .open
        .as_ref()
        .and_then(|open| names.resolve(open.flightmaster, &commands))
        .map(str::to_string);
    let name_changed = *last_name != flightmaster_name;

    if fresh != *last || (fresh.is_some() && name_changed) {
        script.set_taxi(fresh.clone());
        match (&*last, &fresh) {
            // No arguments: the event's one fire site, `0x4dba96`, is a plain
            // `FrameScript_SignalEvent` (`0x703e50`).
            (None, Some(_)) | (Some(_), Some(_)) => {
                script.fire_event("TAXIMAP_OPENED", Vec::new());
            }
            (Some(_), None) => script.fire_event("TAXIMAP_CLOSED", vec![]),
            (None, None) => {}
        }
        *last = fresh;
        *last_name = flightmaster_name;
    }
}

/// Push `UnitOnTaxi("player")`: the reference's verb (`0x517a40`) answers bit 20 of
/// `UNIT_FIELD_FLAGS` (`0x517a86`), which vmangos sets only for a flight
/// (`FlightPathMovementGenerator`). Any other server spline, such as a fear, is not a taxi: stock
/// `UIParent.lua:474-478` closes the windows on `PLAYER_CONTROL_LOST` unless on a taxi. No self
/// store reads as not on a taxi.
fn feed_on_taxi(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut last: Local<crate::ui_script::VmMemo<Option<bool>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let on_taxi = self_q
        .iter()
        .next()
        .is_some_and(|s| s.0.unit_flags() & UNIT_FLAG_TAXI_FLIGHT != 0);
    if *last != Some(on_taxi) {
        script.set_on_taxi(on_taxi);
        *last = Some(on_taxi);
    }
}

/// Drain the Lua intents. `TakeTaxiNode(i)` sends `CMSG_ACTIVATETAXI` when a direct `TaxiPath`
/// edge joins the current node and the target, even if the drawn route detours, and otherwise
/// `CMSG_ACTIVATETAXIEXPRESS` with the chain and its fare, as the reference's `0x4dbad0` test
/// decides; the `Current` node sends nothing. `CloseTaxiMap()` clears locally, with no packet.
fn drain_taxi(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<TaxiState>,
    cache: Res<TaxiRouteCache>,
    catalogs: Option<Res<TaxiCatalogs>>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for idx in script.take_taxi_node() {
        let Some(open) = &state.open else { continue };
        let Some(resolved) = idx.checked_sub(1).and_then(|i| cache.0.get(i)) else {
            continue;
        };
        let chain = resolved.chain.as_slice();
        let (Some(&src), Some(&dest)) = (chain.first(), chain.last()) else {
            continue;
        };
        if src == dest {
            debug!("ui_taxi: TakeTaxiNode({idx}) is the current node — ignored");
            continue;
        }
        let direct = catalogs
            .as_ref()
            .is_some_and(|c| c.paths.between(src, dest).is_some());
        if direct {
            debug!(
                "ui_taxi: activate {src} -> {dest} (direct edge, {} copper)",
                resolved.cost
            );
            let _ = commands.0.send(ClientCommand::ActivateTaxi {
                guid: open.flightmaster,
                source_node: src,
                dest_node: dest,
            });
        } else {
            debug!(
                "ui_taxi: activate express {chain:?} ({} copper)",
                resolved.cost
            );
            let _ = commands.0.send(ClientCommand::ActivateTaxiExpress {
                guid: open.flightmaster,
                total_cost: resolved.cost,
                nodes: chain.to_vec(),
            });
        }
    }
    if script.take_taxi_close() {
        debug!("ui_taxi: client-side close (no packet)");
        state.clear();
    }
}

mod net;

pub(crate) struct UiTaxiPlugin;

impl Plugin for UiTaxiPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<TaxiState>()
            .init_resource::<TaxiRouteCache>()
            .add_systems(
                Update,
                (
                    load_taxi_catalogs,
                    // Range-close first so the clear fires `TAXIMAP_CLOSED` the same frame.
                    close_npc_session_out_of_range::<TaxiState>.before(feed_taxi),
                    feed_taxi.in_set(UiFeed),
                    feed_on_taxi.in_set(UiFeed),
                    drain_taxi.after(UiInput),
                ),
            );
    }
}

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

    #[test]
    fn open_close_and_session_clear() {
        let mut state = TaxiState::default();
        assert_eq!(state.npc(), None);

        state.open(0x42, 2, TaxiMask([0xA, 0, 0, 0, 0, 0, 0, 0]));
        assert_eq!(state.npc(), Some(0x42));
        assert_eq!(state.open.as_ref().unwrap().nearest_node, 2);

        // A client-side close drops the map, nothing else.
        state.reply = Some(3);
        state.discovered = true;
        NpcSession::close(&mut state);
        assert_eq!(state.npc(), None);
        assert_eq!(
            state.reply,
            Some(3),
            "close leaves staged reply/discovery alone"
        );

        // Disconnect drops everything.
        state.open(0x42, 2, TaxiMask::default());
        state.clear_session();
        assert_eq!(state.npc(), None);
        assert_eq!(state.reply, None);
        assert!(!state.discovered);
    }
}
