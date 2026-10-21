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
