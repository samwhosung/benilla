//! The app-side **taxi feed/drain** (decision 0484 phases 1-2) — the two-way half of the taxi-map
//! seam around [`benilla_ui::script`]'s `taxi` module ([`crate::ui_trainer`]'s feed/drain shape).
//!
//! **Phase 1** (unchanged): the net bridge (`crate::ui_taxi::net`) fills [`TaxiState`] from the
//! wire — `SMSG_SHOWTAXINODES` opens the map (flight master + nearest node + known-node bitmask),
//! `SMSG_ACTIVATETAXIREPLY` stages the activate verdict, the first-visit "learn" pair stages the
//! discovery flag — and the standardized NPC-session range guard client-side-closes the map when
//! the player walks out of the flight master's service range.
//!
//! **Phase 2 + the 0496 fold-back** (the laws below are byte-verified — decision 0496 resolves
//! every 0484 INTERIM): [`routing`] loads `TaxiNodes.dbc`/`TaxiPath.dbc`/`WorldMapContinent.dbc`
//! once and holds the pure projection/route-search/node-building logic. [`feed_taxi`] turns
//! [`TaxiOpen`] into the engine-facing `TaxiUiState` snapshot each frame: every known
//! `TaxiNodes.dbc` row on the **current node's own continent** (packet-cached, never a live
//! player-map read), the flight master's node typed `Current`, every other node routed from it
//! over the **geo-distance** search — `Reachable` with its fare/route-hop segments, or **absent**
//! when unroutable (the ref's DISTANT is a dead branch). It fires `TAXIMAP_OPENED`/
//! `TAXIMAP_CLOSED`, surfaces a refusal (`SMSG_ACTIVATETAXIREPLY` ≠ OK) on the red error line
//! (byte-exact `ERR_TAXI*` `GlobalStrings`), closes the map on an OK verdict (the flight starts —
//! 0260's self-spline rails render it), and presents the first-visit discovery (the yellow
//! ERR_NEWTAXIPATH info line + the "TaxiNodeDiscovered" sound kit). [`drain_taxi`] pulls
//! `TakeTaxiNode`/`CloseTaxiMap` back out: a target with a **direct `TaxiPath` edge** sends
//! `CMSG_ACTIVATETAXI` (even when the drawn route detours), an edge-less one
//! `CMSG_ACTIVATETAXIEXPRESS` with the full node chain; a click on the `Current` node is a
//! client-side no-op.
//!
//! Stock `Interface\FrameXML\TaxiFrame.xml` is the window; the toc's header comment
//! carries the three engine-forced deviations from the literal reference Lua (a static node-button
//! pool, the title reading an event arg, the error line's call target).

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

/// The open taxi map (`SMSG_SHOWTAXINODES`'s payload, held exactly as the wire delivered it).
pub(crate) struct TaxiOpen {
    /// The flight master the map opened on.
    pub(crate) flightmaster: u64,
    /// The node nearest the flight master — the map's "you are here" marker, typed `Current`.
    pub(crate) nearest_node: u32,
    /// The full known-node bitmask — the node list's visibility gate and the route search's
    /// traversal restriction (the byte-verified route law — decision 0496 §TU-3).
    pub(crate) known: TaxiMask,
}

/// The taxi session, filled by the net bridge and read by phase 2's feed. `open` is the live map
/// (`None` = no map open); `reply`/`discovered` are one-shot wire events staged for the window to
/// drain and clear — the [`crate::ui_trainer::TrainerErrors`] pattern, folded into one resource
/// since the taxi window has no other error-line consumer yet.
#[derive(Resource, Default)]
pub(crate) struct TaxiState {
    /// The open taxi map; `None` = no flight-master window open.
    pub(crate) open: Option<TaxiOpen>,
    /// The last `SMSG_ACTIVATETAXIREPLY` code (a [`benilla_protocol::messages::taxi_reply`]
    /// value), staged for [`feed_taxi`] to surface as the red error line (or, on `OK`, close the
    /// map) and clear.
    pub(crate) reply: Option<u32>,
    /// Whether a first-visit "learn" landed (`SMSG_NEW_TAXI_PATH` + `SMSG_TAXINODE_STATUS(known
    /// = true)`) since the last feed frame — [`feed_taxi`] presents it (the byte-verified yellow
    /// ERR_NEWTAXIPATH info line + the "TaxiNodeDiscovered" sound kit, decision 0516) and
    /// clears it.
    pub(crate) discovered: bool,
}

impl TaxiState {
    /// The map opened or refreshed (`SMSG_SHOWTAXINODES`).
    pub(crate) fn open(&mut self, flightmaster: u64, nearest_node: u32, known: TaxiMask) {
        self.open = Some(TaxiOpen {
            flightmaster,
            nearest_node,
            known,
        });
    }

    /// Close the open map (a client-side close — the range guard, or the window's close button).
    /// Keeps nothing: a re-open re-lists off a fresh `SMSG_SHOWTAXINODES`.
    pub(crate) fn clear(&mut self) {
        self.open = None;
    }

    /// Disconnect: drop the whole session (mirrors the gossip/merchant/trainer session clears).
    pub(crate) fn clear_session(&mut self) {
        self.open = None;
        self.reply = None;
        self.discovered = false;
    }
}

/// A flight master's answered node status (`SMSG_TAXINODE_STATUS`, upserted by the net bridge):
/// `known = false` (an undiscovered nearest node) shows the green `TalkToMeGreen` overhead icon —
/// the client's `0x5ecdd0` handler → `0x607480` marker swap (resource table `0xc4d9d8` index 4),
/// byte-verified in the 0497 §5.
///
/// **The query and the teardown are [`crate::quest_markers::query`]'s, not this module's**
/// (decision 1918). `0x5eb170` — the only `CMSG_TAXINODE_STATUS_QUERY` sender in the image — has
/// exactly one live caller, `0x607380` @`0x6073e8`, which is the same per-unit function that issues
/// the questgiver query and which tears the shared marker slot down before either. The green `!`
/// and the gold `!` are the *same* `unit+0xb2c`, so they cannot have separate lifetimes; this
/// component is the fact, and its owner is the sweep.
#[derive(Component, Clone, Copy)]
pub(crate) struct FlightMasterStatus {
    pub(crate) known: bool,
}

/// The taxi window is an NPC session: the standardized range guard ([`crate::ui_session`])
/// client-side-closes it — the exact clear the close button does — when the player walks out of
/// the flight master's service range or it despawns.
impl NpcSession for TaxiState {
    fn npc(&self) -> Option<u64> {
        self.open.as_ref().map(|o| o.flightmaster)
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// Push the current taxi map into the VM, fire `TAXIMAP_OPENED`/`TAXIMAP_CLOSED` on a transition
/// (or a content/name change), surface an activate refusal on the red error line (closing the map
/// on `OK` instead — the flight starts and the map has nothing left to show), present a
/// first-visit discovery. Diffed against `Local` memory, the trainer/merchant feed shape. (The
/// `UnitOnTaxi` flag is [`feed_on_taxi`]'s — it reads the descriptor, not the taxi window.)
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

    // The activate verdict (SMSG_ACTIVATETAXIREPLY), staged by the net bridge: a refusal goes to
    // the surface its message record names ([`taxi_error_key`] — seven of the twelve are the
    // YELLOW info line, not the red one); OK clears the map — vmangos's own send order is mount +
    // the flight's SMSG_MONSTER_MOVE right behind the reply, so by the time this lands the ride is
    // already starting (0260's self-spline rails render it) and the taxi map has nothing left to
    // show, matching the real client's own close-on-success.
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

    // The first-visit "learn" (SMSG_NEW_TAXI_PATH): the real client shows message 0xf2 —
    // ERR_NEWTAXIPATH, "New flight path discovered!" — via its descriptor (decision 0516
    // resolving 0501 §1's INTERIM): channel 1 routes the text to the
    // YELLOW `UI_INFO_MESSAGE` FrameScript event (`0x4945b0` → event 0xe1 — good news, not the
    // red warning), and tag 0x44 plays the descriptor's `+0x08` string as a SOUND-KIT NAME
    // through `PlaySoundByName` (`0x458030`, the `MasterSoundEffects`-gated kit lookup) —
    // "TaxiNodeDiscovered", `igNewTaxiNodeDiscovered.wav`. There is NO FrameScript event of
    // that name — 0496 §TU-5's "named-event hashtable" was a mislabel of the sound-kit table
    // (the 0516 correction).
    //
    // **All three of those facts now come from the row itself** (decision 1815) — the surface from
    // `+0x04`, the text from the key, the cue from `+0x08` — where the surface and the cue used to
    // be hand-carried here, which is exactly the drift the catalog exists to stop.
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

    // The continent (art + rect + node filter) is the CURRENT NODE's own continentId,
    // packet-cached — never a live player-map lookup (0496 §TU-2; `build_nodes` resolves it).
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

    // The flight master's name resolves through the NameCache (ask-once, `UnitName("npc")`'s
    // real-client equivalent — see TaxiFrame.xml's deviation note on why the name rides an event
    // arg rather than a live "npc" UnitState read). None/empty while in flight.
    let flightmaster_name = state
        .open
        .as_ref()
        .and_then(|open| names.resolve(open.flightmaster, &commands))
        .map(str::to_string);
    let name_changed = *last_name != flightmaster_name;

    if fresh != *last || (fresh.is_some() && name_changed) {
        script.set_taxi(fresh.clone());
        match (&*last, &fresh) {
            // **No arguments** — `0x4dba96` is the event's one fire site image-wide and it is a
            // `FrameScript_SignalEvent 0x703e50`, `__fastcall(ecx = id)` with a plain `ret` and
            // no vararg push at all. The flight master's name we used to pass was an invention:
            // `TaxiFrame_OnEvent` reads `UnitName("npc")` for it and never looks at `arg1`
            // (decision 2140, found by the argument gate).
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

/// Push `UnitOnTaxi("player")` — **our own descriptor's `UNIT_FLAG_TAXI_FLIGHT`, and nothing
/// else**. The reference's verb (`0x517a40`) resolves the token, then reads `UNIT_FIELD_FLAGS`
/// (`[[obj+0x110]+0xa0]`) and answers `1` iff bit 20 is set (`0x517a86 shr ecx,0x14; test cl,1`).
/// vmangos sets and clears that bit exactly around a flight (`WaypointMovementGenerator.cpp`, the
/// `FlightPathMovementGenerator` initialize/finalize).
///
/// It used to read [`crate::player::Player::server_riding`] — "a server spline owns the avatar" —
/// which is also true under a fear's flee path, a Charge and a knockback. That was not a cosmetic
/// over-answer: stock `UIParent.lua`'s `PLAYER_CONTROL_LOST` handler returns early on
/// `UnitOnTaxi("player")`, so a feared player kept every window open that the reference closes.
///
/// No self store streamed yet reads as not on a taxi — the verb's own absent-object `nil`.
/// Diffed like every other single-value push.
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

/// Drain the Lua intents: `TakeTaxiNode(i)` maps `i` back to its resolved route
/// ([`TaxiRouteCache`]) and sends the activate — the discriminator is the byte-verified one
/// (decision 0496 §TU-3, `0x4dbad0`): **a direct `TaxiPath` edge current→target sends
/// `CMSG_ACTIVATETAXI`** — even when the drawn route detours multi-hop — and only an edge-less
/// target sends `CMSG_ACTIVATETAXIEXPRESS` with the full node chain and its shown fare. A
/// routeless click (`Current`) is a client-side no-op. `CloseTaxiMap()` → a local clear (no
/// packet — the server holds no open-window session for the map).
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
                    // Range-close before the feed so the clear turns into TAXIMAP_CLOSED the same
                    // frame; push before the input pass so an open/close is on screen the same
                    // frame; drain after it (mirrors ui_merchant/ui_trainer).
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

    /// `UnitOnTaxi("player")` answers our descriptor's `UNIT_FLAG_TAXI_FLIGHT` — `1` with the
    /// bit, `nil` without it — whatever else owns the avatar's movement. (A fear's flee path, a
    /// Charge or a knockback are server splines too, and the feed reads nothing that knows about
    /// them: it takes no `Player` at all.)
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

        // A client-side close (the range guard) drops the map, nothing else.
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
