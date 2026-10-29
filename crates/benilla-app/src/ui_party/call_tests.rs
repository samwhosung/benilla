//! The party and raid verbs through the whole client's host: each sends inside its call, so a
//! chunk's packets go out in the order its verbs ran, whatever system each belongs to.
//! `InviteByName` sends at `0x48a4c8`, `LeaveParty` through `SendGroupDisband 0x5e9780`,
//! `RequestRaidInfo` at `0x4a1850` and `ConvertToRaid` at `0x4bbcee`, each before it returns.

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use benilla_ui::script::UiScript;

use crate::net::{ClientCommand, NetCommands};
use crate::script_calls::with_hosted_vm;

/// The whole client, headless, with a VM and the write channel captured.
fn client() -> (App, Receiver<ClientCommand>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    app.insert_non_send_resource(UiScript::new().expect("a VM"));
    (app, rx)
}

/// Our player's object and a party we lead, `members` in the slots, in the world the gates read
/// at the call.
fn lead(world: &mut World, me: u64, members: &[(u64, &str)]) {
    world.spawn((
        crate::net::SelfPlayer,
        crate::net::Guid(me),
        crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x19)])),
    ));
    let mut group = world.resource_mut::<crate::ui_party::GroupState>();
    group.members = members
        .iter()
        .map(
            |&(guid, name)| benilla_protocol::messages::GroupMemberEntry {
                name: name.into(),
                guid,
                status: 1,
                flags: 0,
            },
        )
        .collect();
    group.leader = me;
}

/// Run `chunk` with the host lent, then the party drain, and read what went out of the kinds the
/// chunk's verbs send, in order.
fn sent_by(app: &mut App, rx: &Receiver<ClientCommand>, chunk: &str) -> Vec<String> {
    let world = app.world_mut();
    with_hosted_vm(world, |script| {
        script.expect("a VM").run(chunk).expect("the chunk runs")
    });
    world
        .run_system_once(crate::ui_party::drain_party)
        .expect("the party drain runs on the built client");
    rx.try_iter()
        .filter_map(|c| {
            Some(match c {
                ClientCommand::RandomRoll { min, max } => format!("roll {min}-{max}"),
                ClientCommand::GroupInvite { name } => format!("invite {name}"),
                ClientCommand::GroupUninvite { name } => format!("kick {name}"),
                ClientCommand::GroupLeave => "leave".into(),
                ClientCommand::RequestRaidInfo => "raid info".into(),
                ClientCommand::GroupRaidConvert => "convert".into(),
                _ => return None,
            })
        })
        .collect()
}

/// The party verbs' packets go out in call order with every other verb's: a roll between two
/// party verbs is sent between them.
#[test]
fn a_chunks_party_verbs_send_in_call_order_with_every_other_verb() {
    let (mut app, rx) = client();
    // Our player's object and a party we lead: `LeaveParty` (`0x4e9196`) and `ConvertToRaid`
    // (`0x4bbc96`-`0x4bbcbc`) send only then.
    lead(app.world_mut(), 0x10, &[(0x21, "Bob")]);
    let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
    script.set_unit_guids(&benilla_ui::script::UnitGuids {
        player: 0x10,
        ..Default::default()
    });
    assert_eq!(
        sent_by(
            &mut app,
            &rx,
            r#"RandomRoll(1, 2) InviteByName("Bob") RandomRoll(3, 4) LeaveParty()
               UninviteByName("Zed") RequestRaidInfo() RandomRoll(5, 6) ConvertToRaid()"#,
        ),
        [
            "roll 1-2",
            "invite Bob",
            "roll 3-4",
            "leave",
            "kick Zed",
            "raid info",
            "roll 5-6",
            "convert",
        ]
    );
}

/// `InviteToParty` reaches `InviteByName 0x5e9490` through `InviteByGuid` (`0x5e9477`), whose first
/// act acknowledges the grouping tutorial (`SetTutorialFlag(0x11)` at `0x5e949c`), its
/// `CMSG_TUTORIAL_FLAG` ahead of the invite; the binding `InviteByName` (`0x48a420`) sends with no
/// acknowledge (`0x48a495`-`0x48a4c8`).
#[test]
fn invite_to_party_flags_the_grouping_tutorial_before_its_invite() {
    use crate::net::{Guid, GuidIndex, ObjectStore};
    const ME: u64 = 0x10;
    const BOB: u64 = 0x21;
    let (mut app, rx) = client();
    let world = app.world_mut();
    world
        .resource::<crate::tutorial::Tutorials>()
        .lock()
        .apply_flags(&[0; 32]);
    let bob = world
        .spawn((
            Guid(BOB),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x19)])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(BOB, bob);
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(BOB, "Bob".into(), None);
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(bob),
        guid: Some(BOB),
        ..Default::default()
    };
    world
        .non_send_resource_mut::<UiScript>()
        .set_unit_guids(&benilla_ui::script::UnitGuids {
            player: ME,
            target: BOB,
            ..Default::default()
        });
    with_hosted_vm(world, |script| {
        script
            .expect("a VM")
            .run(r#"InviteByName("Zed") InviteToParty("target") InviteToParty("target")"#)
            .expect("the chunk runs")
    });
    let sent: Vec<String> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::GroupInvite { name } => Some(format!("invite {name}")),
            ClientCommand::TutorialFlag { id } => Some(format!("flag {id:#x}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        ["invite Zed", "flag 0x11", "invite Bob", "invite Bob"]
    );
}

/// `DoReadyCheck` (`0x4bb1d0`) sends (`0x4bb295`), arms (`0x4bb2b6`), then adds
/// `READY_CHECK_START` as a system line (`AddChatLine 0x49a870(text, 10)` at `0x4bb2c7`), so its
/// `CHAT_MSG_SYSTEM` fires before the verb returns.
#[test]
fn do_ready_check_shows_its_start_line_inside_the_call() {
    let (mut app, rx) = client();
    let world = app.world_mut();
    lead(world, 0x10, &[]);
    let script = world.non_send_resource_mut::<UiScript>();
    script
        .run(
            r#"READY_CHECK_START = "Ready check started."
               HEARD = {}
               local f = CreateFrame("Frame")
               f:RegisterEvent("CHAT_MSG_SYSTEM")
               f:SetScript("OnEvent", function() table.insert(HEARD, arg1) end)"#,
        )
        .unwrap();
    let heard = with_hosted_vm(world, |script| {
        script
            .expect("a VM")
            .eval::<Vec<String>>("DoReadyCheck() return HEARD")
            .expect("the chunk runs")
    });
    assert_eq!(heard, ["Ready check started."]);
    assert!(rx
        .try_iter()
        .any(|c| matches!(c, ClientCommand::ReadyCheckStart)));
}

/// The verbs' gates read the group as it stands at the call, through the lent host, not the
/// roster the feed last pushed: `SetLootThreshold` (`0x4e9500`-`0x4e9542`), `ConvertToRaid`
/// (`0x4bbc96`-`0x4bbcbc`) and `DoReadyCheck` (`0x4bb1d6`-`0x4bb1fd`) send for a party the world
/// has us lead though the push says we are alone, and refuse one the push says we lead though the
/// world has us alone.
#[test]
fn the_group_verbs_gate_on_the_world_group_at_the_call() {
    let sends = |rx: &Receiver<ClientCommand>| -> Vec<&'static str> {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::LootMethod { .. } => Some("threshold"),
                ClientCommand::GroupRaidConvert => Some("convert"),
                ClientCommand::ReadyCheckStart => Some("ready check"),
                _ => None,
            })
            .collect()
    };
    let chunk = "SetLootThreshold(3) ConvertToRaid() DoReadyCheck()";

    let (mut app, rx) = client();
    let world = app.world_mut();
    lead(world, 0x10, &[(0x21, "Bob")]);
    with_hosted_vm(world, |script| {
        script.expect("a VM").run(chunk).expect("the chunk runs")
    });
    assert_eq!(
        sends(&rx),
        ["threshold", "convert", "ready check"],
        "the world's party, the push's nothing"
    );

    let (mut app, rx) = client();
    let world = app.world_mut();
    lead(world, 0x10, &[]);
    world.resource_mut::<crate::ui_party::GroupState>().leader = 0;
    world
        .non_send_resource_mut::<UiScript>()
        .set_party(benilla_ui::script::PartyState {
            members: vec![benilla_ui::script::PartyMemberInfo {
                name: "Bob".into(),
                guid: 0x21,
            }],
            leader_guid: 0x10,
            own_guid: 0x10,
            ..Default::default()
        });
    with_hosted_vm(world, |script| {
        script.expect("a VM").run(chunk).expect("the chunk runs")
    });
    assert!(
        sends(&rx).is_empty(),
        "the push's party, the world's nothing"
    );
}

/// The by-name verbs find the name in the roster as it stands at the call (`0x493aa0`'s roster
/// passes, `0x493b22` and `0x493ba7`): `PromoteByName` names `party1`'s unheld member the world
/// seats though the push holds no party.
#[test]
fn the_by_name_verbs_find_the_name_in_the_world_group_at_the_call() {
    let (mut app, rx) = client();
    let world = app.world_mut();
    lead(world, 0x10, &[(0x21, "Bob")]);
    world.non_send_resource_mut::<UiScript>().set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            has_object: true,
            guid: 0x10,
            ..Default::default()
        }),
    );
    with_hosted_vm(world, |script| {
        script
            .expect("a VM")
            .run(r#"PromoteByName("Bo")"#)
            .expect("the chunk runs")
    });
    let promoted: Vec<u64> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::GroupSetLeader { guid } => Some(guid),
            _ => None,
        })
        .collect();
    assert_eq!(promoted, [0x21]);
}
