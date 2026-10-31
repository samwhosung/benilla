//! Area triggers: notice the player entering an `AreaTrigger.dbc` volume and send
//! `CMSG_AREATRIGGER`; the server decides what the trigger does.
//!
//! The reference's check (`0x5e2110`, containment `0x5e22d0`): only the current map's rows are
//! candidates (`0x5e2080`); while the player is still inside the latched trigger nothing is sent;
//! once they leave, the first containing row in file order is latched and sent; a map change
//! clears the latch. So a portal fires once per entry, and a portal's destination is authored
//! outside the return trigger, so a round trip is not a loop.
//!
//! The server re-checks the claim with 5 yd of slop and ignores it while taxi-flying
//! (vmangos `Handlers/MiscHandler.cpp:622`).
//!
//! A GM teleport straight into a volume sends this in the same instant as the teleport ack, and
//! the server may test it against the pre-teleport position and ignore it; the latch does not
//! retry. Probe by moving into the volume instead.

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::AreaTriggerCatalog;
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::player::Player;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;
use benilla_world::world_map::CurrentMap;

/// The `AreaTrigger.dbc` catalog, bucketed by map; absent when the data did not load.
#[derive(Resource)]
pub(crate) struct AreaTriggers(pub(crate) AreaTriggerCatalog);

/// The latched trigger and its map: the reference's current-trigger pointer (`0xc4d73c`), cleared
/// on a map change and kept across a same-map reconnect, as the reference does.
#[derive(Resource, Default)]
pub(crate) struct InsideTrigger(Option<(u32, u32)>);

impl InsideTrigger {
    /// One check in the reference's order (`0x5e2110`): nothing while still inside the latched
    /// volume, else latch and return the first trigger on this map containing `p`.
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
            // After Input has moved the avatar, so the position tested is the one the movement
            // stream reports, which the server re-checks the claim against.
            .add_systems(
                Update,
                check_area_triggers
                    .in_set(WorldStage::Stream)
                    .in_set(crate::client_state::InWorldGated),
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

/// Whether the check runs at all: the reference's gate `0x5e215e` wants the player's health
/// above 0 (signed), and no player object skips it too (`0x5e214f`).
fn check_runs(store: Option<&ObjectStore>) -> bool {
    store.is_some_and(|s| !s.0.unit_is_dead())
}

/// An entry's two sends, in the reference's order: the heartbeat with the live pose
/// (`0x5e221e`, `0x600a10` with 0xee), then CMSG_AREATRIGGER (`0x5e225d`).
fn send_entry(net: &NetCommands, player: &mut Player, now: f32, trigger_id: u32) {
    crate::player::send_heartbeat(&net.0, player, now);
    let _ = net.0.send(ClientCommand::AreaTrigger { trigger_id });
}

/// The per-frame check (`0x5e2110`).
fn check_area_triggers(
    triggers: Option<Res<AreaTriggers>>,
    map: Option<Res<CurrentMap>>,
    mut player: ResMut<Player>,
    me: Query<&ObjectStore, With<SelfPlayer>>,
    time: Res<Time>,
    mut inside: ResMut<InsideTrigger>,
    net: Res<NetCommands>,
) {
    let (Some(triggers), Some(map)) = (triggers, map) else {
        return;
    };
    // Before the server has placed us, `pos` is not a server position.
    if !player.active || !check_runs(me.single().ok()) {
        return;
    }
    let here = bevy_to_wow(player.pos);
    let Some(trigger_id) = inside.step(&triggers.0, map.0, here) else {
        return;
    };
    send_entry(&net, &mut player, time.elapsed_secs(), trigger_id);
    // At `info`: entries are rare, and this line tells whether the client saw the volume at all.
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

    /// The state machine over the real table: the Darnassus portal pair and the Southshore inn's
    /// box trigger. Skips without client data.
    #[test]
    fn a_trigger_fires_once_per_entry_and_re_arms_on_leaving() {
        let Some(cat) = real_catalog() else { return };
        let mut inside = InsideTrigger::default();

        // Rut'theran Village's portal (542, a 10 yd sphere).
        let ruttheran = [8799.41, 969.787, 30.2409];
        assert_eq!(inside.step(&cat, 1, ruttheran), Some(542));
        assert_eq!(
            inside.step(&cat, 1, ruttheran),
            None,
            "standing in a portal must not re-report it — that is the teleport loop"
        );

        // The Darnassus landing spot lies outside the return trigger (527, 10 yd), 17.2 yd from
        // its centre: nothing fires and the latch re-arms.
        let darnassus_arrival = [9946.25, 2612.97, 1316.49];
        assert_eq!(inside.step(&cat, 1, darnassus_arrival), None);

        assert_eq!(inside.step(&cat, 1, [9947.48, 2630.04, 1318.6]), Some(527));

        // The Southshore inn's box trigger (708) on map 0 fires while one is latched on map 1.
        assert_eq!(
            inside.step(&cat, 0, [-854.547, -576.314, 18.4659]),
            Some(708)
        );
        assert_eq!(inside.step(&cat, 0, [-854.593, -576.207, 18.5563]), None);

        // The Deadmines entrance (78), an instance portal: a 7 yd sphere on map 0.
        assert_eq!(inside.step(&cat, 0, [-11208.5, 1685.34, 25.7612]), Some(78));
    }

    #[test]
    fn an_entry_sends_the_heartbeat_with_the_live_pose_before_the_trigger() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let mut player = Player::default();
        player.pos = benilla_assets::coords::wow_to_bevy([8799.41, 969.787, 30.2409]);
        send_entry(&net, &mut player, 5.0, 542);
        let ClientCommand::Move { kind, pos, .. } = rx.try_recv().expect("heartbeat first") else {
            panic!("the first send is a move");
        };
        assert_eq!(kind, crate::net::MoveKind::Heartbeat);
        for (got, want) in pos.iter().zip([8799.41, 969.787, 30.2409]) {
            assert!((got - want).abs() < 0.01, "{pos:?}");
        }
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::AreaTrigger { trigger_id: 542 })
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_dead_player_or_no_player_skips_the_check() {
        let alive = |hp: u32| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                benilla_protocol::field::FIELD_UNIT_HEALTH,
                hp,
            )]))
        };
        assert!(check_runs(Some(&alive(10))));
        assert!(!check_runs(Some(&alive(0))));
        assert!(!check_runs(None));
    }

    #[test]
    fn open_ground_and_unknown_maps_are_silent() {
        let Some(cat) = real_catalog() else { return };
        let mut inside = InsideTrigger::default();
        assert_eq!(inside.step(&cat, 0, [0.0, 0.0, 0.0]), None);
        assert_eq!(inside.step(&cat, 9999, [8799.41, 969.787, 30.2409]), None);
    }
}
