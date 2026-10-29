//! The follow's ends at their packets. Each handler that ends an auto-move calls the canceller
//! `StopAutoMove` (`0x60fb60`), which signals `AUTOFOLLOW_END` for a follow first (`0x60fb84`),
//! inside the handler: the taxi flag's watcher (`0x5ff55a`), our own death (`0x605938`), our own
//! swing's report (`0x625a21`) and the mover switch of a control update (`0x600742`). The follow's
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

/// Our death ends the follow in `OnDied`'s tail (`0x605938`), inside the health's packet.
#[test]
fn our_death_ends_the_follow_at_its_packet() {
    let mut app = following();
    let died = values(ME, &health(0));
    ends_between_the_falls(&frame(&mut app, vec![fall(5), died, fall(5)]));
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
