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
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    app.insert_non_send_resource(UiScript::new().expect("a VM"));
    (app, rx)
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
        .resource_mut::<crate::tutorial::Tutorials>()
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
    world
        .run_system_once(crate::tutorial::drain_tutorials)
        .expect("the tutorial drain runs on the built client");
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
