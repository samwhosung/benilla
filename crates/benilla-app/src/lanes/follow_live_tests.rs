//! The follow's ends at their packets. Each handler that ends an auto-move calls the canceller
//! `StopAutoMove` (`0x60fb60`), which signals `AUTOFOLLOW_END` for a follow first (`0x60fb84`),
//! inside the handler: the taxi flag's watcher (`0x5ff55a`), our own death (`0x605938`), our own
//! swing's report (`0x625a21`), the mover switch of a control update (`0x600742`), the mover's
//! release (`0x5fbc3b` → `0x6006c2`) and a near teleport (`0x60300e` → `0x60e9b5`). The follow's
//! arm reads the follow as the packets left it (`0x611130` → `0x60fb60`).

use benilla_protocol::field::{FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::AttackerState;
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, created, fall, frame, health, values};

/// [`client`]'s active player and its target.
const ME: u64 = 0x7;
const WOLF: u64 = 0xF130_0000_0044_0001;
const BOAR: u64 = 0xF130_0000_0045_0002;
/// Another player, followed.
const LEADER: u64 = 0x52;

/// [`client`] following [`LEADER`], with a spy on the follow's two events.
fn following() -> App {
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("AUTOFOLLOW_BEGIN")
            f:RegisterEvent("AUTOFOLLOW_END")
            f:RegisterEvent("PLAYER_REGEN_DISABLED")
            f:RegisterEvent("UNIT_AURA")
            f:RegisterEvent("PLAYER_TARGET_CHANGED")
            f:RegisterEvent("PLAYER_FARSIGHT_FOCUS_CHANGED")
            f:RegisterEvent("PLAYER_DEAD")
            f:SetScript("OnEvent", function() table.insert(Seen, event) end)
            "#,
        )
        .unwrap();
    let world = app.world_mut();
    assert!(
        world.resource::<crate::net::MoverClaim>().holds(ME),
        "our create claims our own mover"
    );
    world
        .resource_mut::<crate::player::FollowState>()
        .start(LEADER, "Leader".into());
    app
}

/// `seen` holds one `AUTOFOLLOW_END`, after the first fall's `UNIT_COMBAT` and before the second's.
fn ends_between_the_falls(seen: &[String]) {
    let falls: Vec<usize> = (0..seen.len())
        .filter(|&i| seen[i].starts_with("UNIT_COMBAT:player:"))
        .collect();
    let ends: Vec<usize> = (0..seen.len())
        .filter(|&i| seen[i] == "AUTOFOLLOW_END")
        .collect();
    assert_eq!((falls.len(), ends.len()), (2, 1), "{seen:?}");
    assert!(falls[0] < ends[0] && ends[0] < falls[1], "{seen:?}");
}

/// Our swing at the wolf, as the server reports it.
fn our_swing() -> SessionEvent {
    SessionEvent::AttackerState(AttackerState {
        attacker: ME,
        victim: WOLF,
        hit_info: 2,
        damage: 5,
        victim_state: 1,
        absorb: 0,
        resist: 0,
        blocked: 0,
        melee_spell_id: 0,
        school: 0,
    })
}

/// The taxi flag rising on us ends the follow inside its update's notify pass (`0x5ff538`-
/// `0x5ff55a`).
#[test]
fn the_taxi_flag_ends_the_follow_at_its_packet() {
    let mut app = following();
    let taxi = values(
        ME,
        &[(FIELD_UNIT_FLAGS, crate::player::UNIT_FLAG_TAXI_FLIGHT)],
    );
    ends_between_the_falls(&frame(&mut app, vec![fall(5), taxi, fall(5)]));
    assert_eq!(
        app.world().resource::<crate::player::FollowState>().guid,
        None
    );
}

/// The flags watcher's active-player arm runs first (`0x5ff5b4` → `0x5ff4e0`), the taxi stop last
/// in it, after the combat bit's `PLAYER_REGEN_DISABLED` (`0x5ff533`) and ahead of the watcher's
/// `UNIT_AURA` on bit 0x8000000 (`0x5ff7e2`): one flags write, three signals in that order.
#[test]
fn the_taxi_stop_signals_in_the_flags_watchers_order() {
    let mut app = following();
    let flags = crate::player::UNIT_FLAG_TAXI_FLIGHT | 0x8_0000 | 0x0800_0000;
    let seen = frame(&mut app, vec![values(ME, &[(FIELD_UNIT_FLAGS, flags)])]);
    let ours: Vec<&str> = seen
        .iter()
        .map(String::as_str)
        .filter(|e| !e.starts_with("UNIT_COMBAT") && !e.starts_with("UNIT_HEALTH"))
        .collect();
    assert_eq!(
        ours,
        ["PLAYER_REGEN_DISABLED", "AUTOFOLLOW_END", "UNIT_AURA"],
        "{seen:?}"
    );
}

/// A near teleport of our mover ends the follow inside its packet (`0x602fb0` → `0x60e990(1, 0)`
/// at `0x60300e`), ahead of the move it queues.
#[test]
fn a_near_teleport_ends_the_follow_at_its_packet() {
    let mut app = following();
    let teleport = SessionEvent::Teleport {
        guid: ME,
        counter: 1,
        position: [1.0, 2.0, 3.0],
        orientation: 0.0,
    };
    ends_between_the_falls(&frame(&mut app, vec![fall(5), teleport, fall(5)]));
}

/// The active mover's release ends its auto-move (`0x5fbc26`-`0x5fbc3b` → `0x6006b0` →
/// `0x6006c2`): a possessed boar destroyed under us.
#[test]
fn the_movers_release_ends_the_follow_at_its_packet() {
    let mut app = following();
    let world = app.world_mut();
    let (tx, _rx) = crossbeam_channel::unbounded();
    let net = crate::net::NetCommands(tx);
    world
        .resource_mut::<crate::net::MoverClaim>()
        .claim(BOAR, &net);
    let gone = SessionEvent::ObjectDestroyed(BOAR);
    ends_between_the_falls(&frame(&mut app, vec![fall(5), gone, fall(5)]));
    assert!(!app.world().resource::<crate::net::MoverClaim>().holds(BOAR));
}

/// Our death ends the follow inside the health's packet, through the health watcher's input
/// tick (`0x60479c` → `0x5146d6`) ahead of `OnDied` (`0x6047a3`): `AUTOFOLLOW_END`, then our
/// death's target clear (`0x5e9a45`) signals `PLAYER_TARGET_CHANGED`, then `PLAYER_DEAD`.
#[test]
fn our_death_ends_the_follow_at_its_packet() {
    let mut app = following();
    app.world_mut()
        .resource_mut::<crate::net::NetStatus>()
        .connected = true;
    let died = values(ME, &health(0));
    let seen = frame(&mut app, vec![fall(5), died, fall(5)]);
    ends_between_the_falls(&seen);
    let ours: Vec<&str> = seen
        .iter()
        .map(String::as_str)
        .filter(|e| ["AUTOFOLLOW_END", "PLAYER_TARGET_CHANGED", "PLAYER_DEAD"].contains(e))
        .collect();
    assert_eq!(
        ours,
        ["AUTOFOLLOW_END", "PLAYER_TARGET_CHANGED", "PLAYER_DEAD"],
        "{seen:?}"
    );
}

/// Our own swing's report ends the follow (`0x625a03`-`0x625a21`), inside its handler.
#[test]
fn our_swing_ends_the_follow_at_its_packet() {
    let mut app = following();
    ends_between_the_falls(&frame(&mut app, vec![fall(5), our_swing(), fall(5)]));
}

/// A control update that stops our mover zeroes the active mover (`0x5fa678`-`0x5fa696`), whose
/// switch ends the follow (`0x600731`-`0x600742`), inside its handler.
#[test]
fn losing_control_ends_the_follow_at_its_packet() {
    let mut app = following();
    let lost = SessionEvent::ClientControl {
        mover: ME,
        allow_move: false,
    };
    ends_between_the_falls(&frame(&mut app, vec![fall(5), lost, fall(5)]));
}

/// A `FollowUnit` a listener calls between two packets reads the follow the first one ended: its
/// arm finds none to stop, so it signals `AUTOFOLLOW_BEGIN` alone (`0x611130` → `0x60fb70`).
#[test]
fn the_follows_arm_reads_the_follow_an_earlier_packet_ended() {
    use crate::target::relations::fixture::{factions, player, store_of, HUMAN};
    let mut app = following();
    let human = |guid| -> Vec<(u16, u32)> {
        store_of(&player(guid, HUMAN))
            .0
            .raw_fields()
            .filter(|&(i, _)| i != 2)
            .collect()
    };
    let mut leader = human(LEADER);
    leader.extend([(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)]);
    let world = app.world_mut();
    world.insert_resource(factions());
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(LEADER, "Leader".into(), None);
    frame(
        &mut app,
        vec![
            created(LEADER, EntityKind::Player, &leader),
            values(ME, &human(ME)),
        ],
    );
    let world = app.world_mut();
    let index = world.resource::<crate::net::GuidIndex>();
    let (me, them) = (index.0[&ME], index.0[&LEADER]);
    world.entity_mut(me).insert(crate::net::ActiveMover);
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(them),
        guid: Some(LEADER),
        ..Default::default()
    };
    world
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_COMBAT")
            f:SetScript("OnEvent", function()
                if Arm then Arm = nil FollowUnit("target") end
            end)
            Arm = true
            "#,
        )
        .unwrap();
    let seen = frame(&mut app, vec![our_swing(), fall(5)]);
    let follow: Vec<&str> = seen
        .iter()
        .map(String::as_str)
        .filter(|e| e.starts_with("AUTOFOLLOW") || e.starts_with("UNIT_COMBAT:player:"))
        .collect();
    assert_eq!(
        follow,
        [
            "AUTOFOLLOW_END",
            "UNIT_COMBAT:player:100",
            "AUTOFOLLOW_BEGIN"
        ],
        "{seen:?}"
    );
    assert_eq!(
        app.world().resource::<crate::player::FollowState>().guid,
        Some(LEADER)
    );
}

/// [`following`] with autorun held and the frame's call store, the stop's forward release
/// (`0x60fc05` → `0x60e7f0`) to watch.
fn autorunning() -> App {
    let mut app = following();
    let world = app.world_mut();
    world.init_resource::<crate::player::CallAutorun>();
    world.init_resource::<crate::player::Player>();
    let mut player = world.resource_mut::<crate::player::Player>();
    player.active = true;
    player.autorun_for_test(Some(true));
    app
}

/// A stop that ends a follow owes its forward release: autorun clears inside the handler
/// (`0x514630` at `0x60e83f`), and the frame's steer applies it ahead of the controller's step,
/// which stops the move. A second stop finds no auto-move (`0x6103a0`) and owes nothing.
#[test]
fn the_stop_owes_its_autorun_release() {
    let mut app = autorunning();
    frame(&mut app, vec![our_swing()]);
    let world = app.world_mut();
    assert!(world
        .resource::<crate::player::FollowState>()
        .owes_release());
    assert_eq!(
        world.resource::<crate::player::CallAutorun>().called(),
        Some(false)
    );
    crate::player::release_owed_for_test(world);
    let mut player = world.resource_mut::<crate::player::Player>();
    assert!(!player.autorun_for_test(None));
    player.autorun_for_test(Some(true));
    frame(&mut app, vec![our_swing()]);
    let world = app.world_mut();
    assert!(!world
        .resource::<crate::player::FollowState>()
        .owes_release());
    crate::player::release_owed_for_test(world);
    assert!(world
        .resource_mut::<crate::player::Player>()
        .autorun_for_test(None));
}

/// A `ToggleAutoRun` after the stop, before the frame's latch, reads the bit the stop cleared, so
/// it presses autorun on (`0x513de7`), as the stop's clear ran inside its handler (`0x514630`).
#[test]
fn a_toggle_after_the_stop_reads_autorun_clear() {
    use bevy::ecs::system::RunSystemOnce;
    let mut app = autorunning();
    app.world_mut()
        .resource_mut::<crate::bindings::BindingsState>()
        .open_for_test();
    let seen = frame(&mut app, vec![our_swing()]);
    assert!(seen.iter().any(|e| e == "AUTOFOLLOW_END"), "{seen:?}");
    let world = app.world_mut();
    world
        .run_system_once(|mut steps: crate::player::CommandSteps| {
            steps.call(benilla_ui::script::BindingInput::Fire(
                benilla_ui::script::FiredInput::ToggleAutoRun,
            ))
        })
        .unwrap();
    assert_eq!(
        world.resource::<crate::player::CallAutorun>().called(),
        Some(true)
    );
}

/// [`following`] with the boar's loot window open, its own events taken, and a spy on its close.
fn looting() -> App {
    let mut app = following();
    let world = app.world_mut();
    world
        .resource_mut::<crate::ui_loot::LootState>()
        .open(BOAR, 1, 0, Vec::new());
    world.resource_mut::<crate::ui_loot::LootLatch>().0 = Some(BOAR);
    world
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("LOOT_CLOSED")
            f:SetScript("OnEvent", function() table.insert(Seen, event) end)
            "#,
        )
        .unwrap();
    frame(&mut app, vec![]);
    app
}

/// `seen`'s events between the two falls, in order.
fn past_the_falls(seen: &[String]) -> Vec<&str> {
    let falls: Vec<usize> = (0..seen.len())
        .filter(|&i| seen[i].starts_with("UNIT_COMBAT:player:"))
        .collect();
    seen[falls[0] + 1..falls[1]]
        .iter()
        .map(String::as_str)
        .filter(|e| !e.starts_with("UNIT_HEALTH"))
        .collect()
}

/// A near teleport's movement guard (`0x60e990(1, 0)` at `0x60300e`) ends the follow, then closes
/// the active player's loot window as a movement start does (`0x60e9dd` → `0x48f200(1, 1, 0)`).
#[test]
fn a_near_teleport_closes_the_loot_window_after_the_follow() {
    let mut app = looting();
    let teleport = SessionEvent::Teleport {
        guid: ME,
        counter: 1,
        position: [1.0, 2.0, 3.0],
        orientation: 0.0,
    };
    let seen = frame(&mut app, vec![fall(5), teleport, fall(5)]);
    ends_between_the_falls(&seen);
    assert_eq!(past_the_falls(&seen), ["AUTOFOLLOW_END", "LOOT_CLOSED"]);
}

/// A knockback runs the same guard ahead of its queue (`QueueKnockBack 0x602670`, `0x60267e`).
#[test]
fn a_knockback_ends_the_follow_and_closes_the_loot_window() {
    let mut app = looting();
    let knock = SessionEvent::KnockBack {
        guid: ME,
        counter: 1,
        launch: Default::default(),
    };
    let seen = frame(&mut app, vec![fall(5), knock, fall(5)]);
    ends_between_the_falls(&seen);
    assert_eq!(past_the_falls(&seen), ["AUTOFOLLOW_END", "LOOT_CLOSED"]);
}

/// The feign-death bit's rise on our mover runs its tick with `apply` clear (`0x6004f5`-
/// `0x600530`), whose teardown leg ends the follow, inside the update's notify pass.
#[test]
fn feign_death_ends_the_follow_at_its_packet() {
    use benilla_protocol::field::FIELD_UNIT_DYNAMIC_FLAGS;
    let mut app = following();
    let feign = values(ME, &[(FIELD_UNIT_DYNAMIC_FLAGS, 0x20)]);
    ends_between_the_falls(&frame(&mut app, vec![fall(5), feign, fall(5)]));
}

/// The stun bit's flip runs the active player's tick first in the flags watcher (`0x5ff501`): a
/// rooted mover that is now stunned may neither translate nor turn, so the follow ends there.
#[test]
fn a_stun_on_a_rooted_mover_ends_the_follow_at_its_packet() {
    let mut app = following();
    let world = app.world_mut();
    world.init_resource::<crate::player::Player>();
    world
        .resource_mut::<crate::player::Player>()
        .root_for_test();
    let stun = values(ME, &[(FIELD_UNIT_FLAGS, crate::player::UNIT_FLAG_STUNNED)]);
    ends_between_the_falls(&frame(&mut app, vec![fall(5), stun, fall(5)]));
}

/// A server path on our mover.
fn monster_path() -> SessionEvent {
    SessionEvent::MonsterMove {
        guid: ME,
        transport: None,
        start: [0.0; 3],
        spline_id: 1,
        path: vec![[0.0; 3], [5.0, 0.0, 0.0]],
        facing: benilla_protocol::messages::MonsterMoveFacing::None,
        stop: false,
        duration_ms: 1000,
        flying: false,
        run_mode: true,
    }
}

/// A server path on our mover runs its tick (`0x602053`), whose precondition the spline fails:
/// the follow ends inside the move's packet.
#[test]
fn a_monster_move_on_us_ends_the_follow_at_its_packet() {
    let mut app = following();
    let path = monster_path();
    ends_between_the_falls(&frame(&mut app, vec![fall(5), path, fall(5)]));
}

/// A rooted mover starts no server path (`0x6187c2`-`0x6187c7` returns 0 on the root bit), so
/// `0x601fee` skips the tick and the follow goes on.
#[test]
fn a_monster_move_on_a_rooted_mover_keeps_the_follow() {
    let mut app = following();
    let world = app.world_mut();
    world.init_resource::<crate::player::Player>();
    world
        .resource_mut::<crate::player::Player>()
        .root_for_test();
    let seen = frame(&mut app, vec![monster_path()]);
    assert!(!seen.iter().any(|e| e == "AUTOFOLLOW_END"), "{seen:?}");
    assert_eq!(
        app.world().resource::<crate::player::FollowState>().guid,
        Some(LEADER)
    );
}

/// Far sight on a held object stops the mover's auto-move inside the field watcher
/// (`0x5de0d0` → `0x5ee4a2`), ahead of its `PLAYER_FARSIGHT_FOCUS_CHANGED` (`0x5de167`).
#[test]
fn far_sight_ends_the_follow_ahead_of_its_focus_change() {
    let mut app = following();
    let seen = frame(&mut app, vec![fall(5), far_sight(BOAR), fall(5)]);
    assert_eq!(
        past_the_falls(&seen),
        ["AUTOFOLLOW_END", "PLAYER_FARSIGHT_FOCUS_CHANGED"],
        "{seen:?}"
    );
}

/// Our `PLAYER_FARSIGHT` set to `object`.
fn far_sight(object: u64) -> SessionEvent {
    /// `PLAYER_FARSIGHT` (`+0x830` of the player section).
    const FIELD_PLAYER_FARSIGHT: u16 = 712;
    values(
        ME,
        &[
            (FIELD_PLAYER_FARSIGHT, object as u32),
            (FIELD_PLAYER_FARSIGHT + 1, (object >> 32) as u32),
        ],
    )
}

/// Far sight on a unit we control takes the active mover (`0x5ee45d` → `0x5ee477`): the switch
/// stops the outgoing mover's auto-move (`0x600742`), and when the unit already is the mover it
/// switches nothing (`0x6006fb`) and runs no tick, so the follow goes on. `IsClientControlled`
/// (`0x5fa5f0`) is the unit's own bit, which the control update writes (`0x5fa636`) whoever holds
/// the mover.
#[test]
fn far_sight_on_a_unit_we_control_stops_only_the_mover_it_switches_from() {
    let ends = |mover: u64, granted: Option<u64>, object: u64| {
        let mut app = following();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let net = crate::net::NetCommands(tx);
        if let Some(unit) = granted {
            let grant = SessionEvent::ClientControl {
                mover: unit,
                allow_move: true,
            };
            frame(&mut app, vec![grant]);
        }
        let world = app.world_mut();
        world
            .resource_mut::<crate::net::MoverClaim>()
            .claim(mover, &net);
        // The grant's own switch may have ended it.
        world
            .resource_mut::<crate::player::FollowState>()
            .start(LEADER, "Leader".into());
        let seen = frame(&mut app, vec![far_sight(object)]);
        seen.iter().filter(|e| *e == "AUTOFOLLOW_END").count()
    };
    assert_eq!(
        ends(BOAR, Some(BOAR), BOAR),
        0,
        "the possessed boar already is the mover"
    );
    assert_eq!(ends(ME, None, ME), 0, "our own body already is the mover");
    assert_eq!(
        ends(BOAR, None, ME),
        1,
        "our own body takes the mover from the boar"
    );
    assert_eq!(
        ends(WOLF, Some(BOAR), BOAR),
        1,
        "a controlled boar takes it from the wolf"
    );
    assert_eq!(
        ends(ME, None, WOLF),
        1,
        "an uncontrolled unit runs the tick"
    );
}

/// The stun's tick reads the far-sight latch as the packets left it (`[mover+0x1c70] & 1`,
/// `0x514540`): far sight on our own body sets it at its packet and stops nothing (`0x6006fb`), so
/// a stun later in the same frame finds the view out and its tick tears the follow down, unrooted.
#[test]
fn a_stun_after_far_sight_in_one_frame_reads_the_latch_its_packet_set() {
    let mut app = following();
    let stun = values(ME, &[(FIELD_UNIT_FLAGS, crate::player::UNIT_FLAG_STUNNED)]);
    let seen = frame(&mut app, vec![far_sight(ME), stun]);
    assert_eq!(
        seen.iter().filter(|e| *e == "AUTOFOLLOW_END").count(),
        1,
        "{seen:?}"
    );
}
