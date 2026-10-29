//! The logout replies' signals at their packets. `SMSG_LOGOUT_RESPONSE`'s handler signals
//! `PLAYER_CAMPING` or `PLAYER_QUITING` in place (`0x5aaef0`, `0x5aaf17`), and
//! `SMSG_LOGOUT_CANCEL_ACK`'s `LOGOUT_CANCEL` while a request is pending (`0x5aaf60`, `0x5aaf72`),
//! so each fires between the events of the packets around it.

use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, fall, frame};

/// A fall's `UNIT_COMBAT`, as [`client`]'s spy records it.
const FELL: &str = "UNIT_COMBAT:player:100";

/// [`client`] with the world handle seated, a write channel kept, and a spy on the logout
/// signals.
fn logout_client() -> (App, crossbeam_channel::Receiver<crate::net::ClientCommand>) {
    let mut app = client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    app.world_mut()
        .resource_mut::<crate::ui_logout::LogoutState>()
        .seat_world_handle_for_test();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"PLAYER_CAMPING", "PLAYER_QUITING", "LOGOUT_CANCEL"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function() table.insert(Seen, event) end)
            "#,
        )
        .unwrap();
    (app, rx)
}

/// Run `chunk` with the host lent.
fn run(app: &mut App, chunk: &str) {
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| s.expect("a VM").run(chunk))
        .expect("runs");
}

/// The response's `PLAYER_CAMPING` fires at its packet's place, between a fall's `UNIT_COMBAT` on
/// each side.
#[test]
fn player_camping_fires_between_neighbouring_packets_events() {
    let (mut app, _rx) = logout_client();
    run(&mut app, "Logout()");
    let response = SessionEvent::LogoutResponse {
        reason: 0,
        instant: false,
    };
    let seen = frame(&mut app, vec![fall(5), response, fall(5)]);
    assert_eq!(seen, [FELL, "PLAYER_CAMPING", FELL]);
}

/// A quit's response signals `PLAYER_QUITING` at its packet, the quit byte its request stored
/// (`0x5ab053`) added to the id (`0x5aaf0c`).
#[test]
fn player_quiting_fires_at_its_packet() {
    let (mut app, _rx) = logout_client();
    run(&mut app, "Quit()");
    let response = SessionEvent::LogoutResponse {
        reason: 0,
        instant: false,
    };
    let seen = frame(&mut app, vec![fall(5), response, fall(5)]);
    assert_eq!(seen, [FELL, "PLAYER_QUITING", FELL]);
}

/// A cancel ack while a request is pending signals `LOGOUT_CANCEL` at its packet.
#[test]
fn logout_cancel_fires_at_the_acks_packet() {
    let (mut app, _rx) = logout_client();
    run(&mut app, "Logout()");
    let seen = frame(
        &mut app,
        vec![fall(5), SessionEvent::LogoutCancelled, fall(5)],
    );
    assert_eq!(seen, [FELL, "LOGOUT_CANCEL", FELL]);
}
