//! Area triggers — the client's half of every portal, instance entrance and "explore here" quest.
//!
//! `AreaTrigger.dbc` is a table of invisible volumes. The client's *entire* job is geometry: notice
//! that the player has walked into one, and send `CMSG_AREATRIGGER` naming its id. The **server**
//! decides what the trigger means — a teleport (`areatrigger_teleport`: the Darnassus/Rut'theran
//! portals, every dungeon and raid entrance), a quest's explore objective
//! (`areatrigger_involvedrelation`), the inn's rested state, a battleground's entrance list. The
//! client never knows and never needs to.
//!
//! Until this module existed, benilla sent that opcode from nowhere: a portal did nothing, and an
//! instance entrance did nothing, because the server was never told we were standing in one (ledger
//! **B70**, **N02**). The teleport that answers it has worked since.
//!
//! ## The law, from the reference
//!
//! From `0x5e2110` (the per-frame check) and `0x5e22d0` (containment); the geometry itself is
//! [`AreaTriggerRow::contains`](benilla_formats::AreaTriggerRow::contains).
//!
//! - The map's rows are the only candidates (`0x5e2080` narrows the map-sorted table to a
//!   `[first, end)` window before any test).
//! - **One latch, and it is an *exit* latch.** The check holds the trigger it is currently inside;
//!   while the player is still inside *that* volume it returns immediately and sends nothing. Only
//!   once they have left does it scan for a new one — so standing in a portal fires it exactly once,
//!   and overlapping volumes never fight.
//! - The **first** containing row in file order wins.
//! - A map change clears the latch.
//!
//! Two things the reference's shape buys us, worth stating because they are load-bearing: a trigger
//! cannot re-fire while you stand in it (a portal would otherwise re-teleport you every frame), and
//! the *destination* of a portal pair is authored to land outside the return trigger (Darnassus's
//! exit lands 13.9 yd from the entrance's 10-yd sphere), so a round trip is not a loop.
//!
//! The server re-checks our claim with 5 yd of slop and ignores it while taxi-flying
//! (`HandleAreaTriggerOpcode`, vmangos `Handlers/MiscHandler.cpp:622`), so a wrong id is refused
//! rather than obeyed.
//!
//! ## ⚠ Probing this with `.go` is not the same as walking in
//!
//! A GM teleport that drops you *inside* a volume sends this message in the same millisecond as the
//! teleport ack, and the server re-checks the claim against the position it has stored — which is
//! sometimes still the pre-teleport one. Measured: five such teleport-ins were obeyed and one
//! (Darnassus's trigger 527) was silently ignored, while drifting into the same trigger under
//! movement fired it and teleported 40 ms later. So an ignored `.go` probe is the harness racing
//! the server, not this check failing — and the latch, faithfully, does not retry (it latches on
//! the *send*; the client never learns whether the server obeyed). Step out and back in, or better,
//! park outside and move in.

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::AreaTriggerCatalog;
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands};
use crate::player::Player;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;
use benilla_world::world_map::CurrentMap;

/// The `AreaTrigger.dbc` catalog, bucketed by map. Absent when the client data didn't load — the
/// check then does nothing, like every other data-driven system here.
#[derive(Resource)]
pub(crate) struct AreaTriggers(pub(crate) AreaTriggerCatalog);

/// The trigger we are currently standing **inside**, with the map it is on — the reference's single
/// "current trigger" pointer (`DAT_00c4d73c`).
///
/// Keyed by map so it can never leak across a worldport (the reference clears it on the map change;
/// carrying the map id makes the same guarantee without a hook). It deliberately survives a
/// reconnect on the same map, which is also what the reference does — its clear runs off the map
/// change, not the socket — and it is the safer half of that coin: reconnecting inside a portal
/// does not teleport you.
#[derive(Resource, Default)]
pub(crate) struct InsideTrigger(Option<(u32, u32)>);

impl InsideTrigger {
    /// One check, as the reference orders it (`0x5e2110`): if we are still inside the latched
    /// volume, report nothing; otherwise unlatch, and latch + report the **first** trigger on this
    /// map containing `p`, if any. `Some(id)` means "send `CMSG_AREATRIGGER` for this one".
    ///
    /// Pure, and separate from the system, because the property that matters is a state machine
    /// rather than a query: it must fire **once** per entry. Firing per frame would be a packet
    /// flood at best and, on a teleport trigger, an infinite loop — arrive, fire, arrive.
    fn step(&mut self, triggers: &AreaTriggerCatalog, map_id: u32, p: [f32; 3]) -> Option<u32> {
        if let Some((latched_map, id)) = self.0 {
            let still_in = latched_map == map_id
                && triggers
                    .on_map(map_id)
                    .iter()
                    .find(|t| t.id == id)
                    .is_some_and(|t| t.contains(p));
            if still_in {
                return None;
            }
            self.0 = None;
        }
        let entered = triggers.first_containing(map_id, p)?;
        self.0 = Some((map_id, entered.id));
        Some(entered.id)
    }
}

pub(crate) struct AreaTriggerPlugin;

impl Plugin for AreaTriggerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InsideTrigger>()
            .add_systems(Startup, load_area_triggers.after(AssetSet::Open))
            // In the Stream band: after Input has moved the avatar (and applied any teleport
            // snap), so the position tested is the one this frame ends at — the same one the
            // movement stream reports, which is what the server re-checks us against.
            .add_systems(
                Update,
                check_area_triggers
                    .in_set(WorldStage::Stream)
                    .in_set(crate::char_select::InWorldGated),
            );
    }
}

fn load_area_triggers(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let mut chain = assets.chain.lock_recover();
    match benilla_formats::load_area_trigger_catalog(&mut chain) {
        Ok(cat) => {
            info!("area_trigger: {} triggers", cat.len());
            commands.insert_resource(AreaTriggers(cat));
        }
        Err(e) => warn!("area_trigger: AreaTrigger.dbc failed to load: {e:#}"),
    }
}

/// The per-frame check — the reference's `0x5e2110`, in the same order it runs.
fn check_area_triggers(
    triggers: Option<Res<AreaTriggers>>,
    map: Option<Res<CurrentMap>>,
    player: Res<Player>,
    mut inside: ResMut<InsideTrigger>,
    net: Res<NetCommands>,
) {
    let (Some(triggers), Some(map)) = (triggers, map) else {
        return;
    };
    // Before the server has placed us, `pos` is wherever the free-flying camera left the avatar —
    // not a position the server would recognise, and not one worth reporting.
    if !player.active {
        return;
    }
    let here = bevy_to_wow(player.pos);
    let Some(trigger_id) = inside.step(&triggers.0, map.0, here) else {
        return;
    };
    // Fire-and-forget, like every other send: a down write thread drops it.
    let _ = net.0.send(ClientCommand::AreaTrigger { trigger_id });
    // At `info`, deliberately: entering a trigger is rare (you must cross a volume boundary), and
    // when a portal or instance entrance "does nothing", the first question is whether the client
    // saw the volume at all. One line answers it.
    info!(
        "area_trigger: entered {trigger_id} on map {} at [{:.2}, {:.2}, {:.2}]",
        map.0, here[0], here[1], here[2]
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real_catalog() -> Option<AreaTriggerCatalog> {
        let data = benilla_formats::wow_data_or_skip!(None);
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        Some(benilla_formats::load_area_trigger_catalog(&mut chain).expect("AreaTrigger.dbc"))
    }

    /// The whole state machine over the **real** table, walking the exact route the live probe
    /// walked: the Darnassus portal pair and the Southshore inn's box (the shape the sphere
    /// tests can't reach). Skips without client data.
    ///
    /// The second assertion is the load-bearing one — a portal that reported every frame would
    /// teleport you the instant you arrived, forever.
    #[test]
    fn a_trigger_fires_once_per_entry_and_re_arms_on_leaving() {
        let Some(cat) = real_catalog() else { return };
        let mut inside = InsideTrigger::default();

        // Rut'theran Village's portal (542, a 10-yd sphere): step in, and it reports once.
        let ruttheran = [8799.41, 969.787, 30.2409];
        assert_eq!(inside.step(&cat, 1, ruttheran), Some(542));
        assert_eq!(
            inside.step(&cat, 1, ruttheran),
            None,
            "standing in a portal must not re-report it — that is the teleport loop"
        );

        // The server answers by putting us at the Darnassus end. That landing spot is deliberately
        // OUTSIDE the return trigger (527, also 10 yd) — 17.2 yd from its centre, and 13.9 yd the
        // other direction — which is why a round trip is not a loop. Nothing fires, and the latch
        // re-arms.
        let darnassus_arrival = [9946.25, 2612.97, 1316.49];
        assert_eq!(inside.step(&cat, 1, darnassus_arrival), None);

        // Walk into the return portal proper: it reports, having re-armed.
        assert_eq!(inside.step(&cat, 1, [9947.48, 2630.04, 1318.6]), Some(527));

        // A map change cannot leak the latch: the Southshore inn's BOX trigger (708) is on map 0
        // and fires immediately, even though a trigger is still latched from map 1.
        assert_eq!(
            inside.step(&cat, 0, [-854.547, -576.314, 18.4659]),
            Some(708)
        );
        assert_eq!(inside.step(&cat, 0, [-854.593, -576.207, 18.5563]), None);

        // Deadmines' entrance (78) — the instance-portal case (N02), a 7-yd sphere on map 0.
        assert_eq!(inside.step(&cat, 0, [-11208.5, 1685.34, 25.7612]), Some(78));
    }

    /// Empty air reports nothing, and an unknown map is not an error.
    #[test]
    fn open_ground_and_unknown_maps_are_silent() {
        let Some(cat) = real_catalog() else { return };
        let mut inside = InsideTrigger::default();
        assert_eq!(inside.step(&cat, 0, [0.0, 0.0, 0.0]), None);
        assert_eq!(inside.step(&cat, 9999, [8799.41, 969.787, 30.2409]), None);
    }
}
