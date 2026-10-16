//! The session's edges: the world entry, the logout, a refused
//! login and a dead session move [`ClientState`] and the pick, in one chain with the roster policy
//! ([`super::roster`]). What a screen shows of them is the screen's.

use bevy::prelude::*;

use super::roster::{apply_roster_policy, Roster};
use crate::client_state::ClientState;
use crate::net::{CharacterLoginFailedMessage, EnteredWorldMessage, LoggedOutMessage};

/// The session's edges as one set, which a screen that shows them runs after.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SessionEdges;

/// The session's edges, in every state: the reconnect auto-answer and the logout edge arrive
/// `InWorld`. A logout or a refused login clears the pick before the roster policy reads it: the
/// roster comes right behind them on the kept link, often in the same drain, and must be shown,
/// not answered with the pick just left. The refusal and `back_on_disconnect` run after the entry:
/// the entry is announced with `CMSG_PLAYER_LOGIN`, so a refusal or a session that died during
/// entry queues both edges into one frame, and the later word must win (`0x5aad70` leaves the
/// player on the select screen).
pub(crate) fn session_edges(
) -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (
        back_on_logout,
        enter_on_connected,
        back_on_login_refused,
        apply_roster_policy,
        back_on_disconnect,
    )
        .chain()
        .into_configs()
}

/// A confirmed `/logout`: back to select, the pick cleared so the next roster is shown.
fn back_on_logout(
    mut msgs: MessageReader<LoggedOutMessage>,
    mut roster: ResMut<Roster>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if msgs.read().next().is_some() {
        roster.pending_pick = None;
        next.set(ClientState::CharSelect);
    }
}

/// `Connected` ([`EnteredWorldMessage`]): the world owns the session.
fn enter_on_connected(
    mut msgs: MessageReader<EnteredWorldMessage>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if msgs.read().next().is_some() {
        next.set(ClientState::InWorld);
    }
}

/// The session died: back to the login screen, as `GlueParent.lua`'s `DISCONNECTED_FROM_SERVER`
/// does. Chained after [`enter_on_connected`]: a stream dying during entry drains `Connected` and
/// `Disconnected` in one frame, and the dead session must be the last word.
fn back_on_disconnect(
    mut msgs: MessageReader<crate::net::DisconnectedMessage>,
    mut roster: ResMut<Roster>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if !msgs.read().any(|m| m.session_over) {
        return;
    }
    // With no reconnect coming, a kept pick would auto-answer the next roster.
    roster.pending_pick = None;
    next.set(ClientState::Login);
}

/// `SMSG_CHARACTER_LOGIN_FAILED`: back to select. The IO thread announces the entry with the pick,
/// so this undoes a half-built one. The pick must be cleared, or the roster behind the refusal is
/// auto-answered and the refused login loops. The refusal's words are the glue manager's state 9.
fn back_on_login_refused(
    mut msgs: MessageReader<CharacterLoginFailedMessage>,
    mut roster: ResMut<Roster>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if msgs.read().last().is_none() {
        return;
    }
    roster.pending_pick = None;
    next.set(ClientState::CharSelect);
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::glue_session::roster::test_character as character;
    use crate::net::{CharListMessage, CharPick};

    /// A socket dying mid-entry (a displacement kick) drains `Connected` and `Disconnected` in one
    /// frame; the ordering alone makes the disconnect win.
    #[test]
    fn a_session_lost_during_entry_beats_the_entry() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::CharSelect)
            .init_resource::<Roster>()
            .init_resource::<crate::ui_script::PlayerUiHover>()
            .init_resource::<crate::ui_script::UiKeyboardCapture>()
            .add_message::<EnteredWorldMessage>()
            .add_message::<crate::net::DisconnectedMessage>()
            .add_systems(Update, (enter_on_connected, back_on_disconnect).chain());
        app.world_mut().resource_mut::<Roster>().pending_pick = Some(7);

        app.world_mut().write_message(EnteredWorldMessage {
            tutorial_flags: None,
        });
        app.world_mut()
            .write_message(crate::net::DisconnectedMessage {
                reason: "disconnected: world stream closed: failed to fill whole buffer".into(),
                end: benilla_protocol::SessionEnd::Lost,
                session_over: true,
            });
        app.update();
        app.update(); // `StateTransition` applies the pending state at the next frame

        assert_eq!(
            *app.world().resource::<State<ClientState>>().get(),
            ClientState::Login,
            "the dead session must be the last word — the reference's DISCONNECTED_FROM_SERVER \
             puts the client back on the account screen, not into a world with no session",
        );
        assert_eq!(
            app.world().resource::<Roster>().pending_pick,
            None,
            "and the reconnect's pending pick goes with it, or the next roster walks straight \
             back into the world the player was just thrown out of",
        );
    }

    /// A teardown whose session is not over (a logout, or a reconnect) still enters the world.
    #[test]
    fn a_teardown_that_is_not_the_end_leaves_the_entry_alone() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::CharSelect)
            .init_resource::<Roster>()
            .init_resource::<crate::ui_script::PlayerUiHover>()
            .init_resource::<crate::ui_script::UiKeyboardCapture>()
            .add_message::<EnteredWorldMessage>()
            .add_message::<crate::net::DisconnectedMessage>()
            .add_systems(Update, (enter_on_connected, back_on_disconnect).chain());

        app.world_mut().write_message(EnteredWorldMessage {
            tutorial_flags: None,
        });
        app.world_mut()
            .write_message(crate::net::DisconnectedMessage {
                reason: "logged out".into(),
                end: benilla_protocol::SessionEnd::LoggedOut,
                session_over: false,
            });
        app.update();
        app.update();

        assert_eq!(
            *app.world().resource::<State<ClientState>>().get(),
            ClientState::InWorld,
        );
    }

    /// `Connected` is emitted with `CMSG_PLAYER_LOGIN`, and vmangos refuses before touching the
    /// database, so the refusal lands in the same drain and must be the last word.
    #[test]
    fn a_refused_login_beats_the_entry_it_revokes() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::CharSelect)
            .insert_resource(
                Roster::with_pending_pick(vec![character(7, "Seven")], 7).env_settled(),
            )
            .insert_resource(CharPick(tx.into()))
            .init_resource::<crate::glue_session::roster::RunPick>()
            .init_resource::<crate::cvars::Cvars>()
            .add_message::<CharListMessage>()
            .add_message::<EnteredWorldMessage>()
            .add_message::<LoggedOutMessage>()
            .add_message::<CharacterLoginFailedMessage>()
            .add_message::<crate::net::DisconnectedMessage>()
            // The production chain, in its own order.
            .add_systems(Update, session_edges());

        app.world_mut().write_message(EnteredWorldMessage {
            tutorial_flags: None,
        });
        app.world_mut()
            .write_message(CharacterLoginFailedMessage { result: 0x01 });
        app.update();
        app.update(); // `StateTransition` applies the pending state at the next frame

        assert_eq!(
            *app.world().resource::<State<ClientState>>().get(),
            ClientState::CharSelect,
            "the refusal must win — the reference leaves the player on the select screen it \
             never actually took them off",
        );
        assert_eq!(
            app.world().resource::<Roster>().pending_pick,
            None,
            "and the pick goes with it: the relist behind the refusal is auto-answered with \
             `pending_pick`, so keeping it would re-enter the character just refused, forever",
        );
        assert!(rx.try_recv().is_err(), "nothing picked again");
    }

    /// A logout, or a refused login, and the roster the kept link sends right behind it can land
    /// in one drain: the pick the world was entered with goes first, so the roster is shown and
    /// not answered with that pick again.
    #[test]
    fn a_return_and_its_roster_in_one_drain_show_the_roster() {
        for refused in [false, true] {
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
                .insert_state(ClientState::InWorld)
                .insert_resource(
                    Roster::with_pending_pick(vec![character(7, "Seven")], 7).env_settled(),
                )
                .insert_resource(CharPick(tx.into()))
                .init_resource::<crate::glue_session::roster::RunPick>()
                .init_resource::<crate::cvars::Cvars>()
                .add_message::<CharListMessage>()
                .add_message::<EnteredWorldMessage>()
                .add_message::<LoggedOutMessage>()
                .add_message::<CharacterLoginFailedMessage>()
                .add_message::<crate::net::DisconnectedMessage>()
                .add_systems(Update, session_edges());
            if refused {
                app.world_mut()
                    .write_message(CharacterLoginFailedMessage { result: 0x01 });
            } else {
                app.world_mut().write_message(LoggedOutMessage);
            }
            app.world_mut().write_message(CharListMessage {
                characters: vec![character(7, "Seven")],
                realm: None,
                cause: benilla_protocol::RosterCause::Unasked,
                epoch: 1,
            });
            app.update();
            app.update();

            assert_eq!(
                *app.world().resource::<State<ClientState>>().get(),
                ClientState::CharSelect,
                "refused: {refused}"
            );
            assert!(
                rx.try_recv().is_err(),
                "refused: {refused} — the roster was answered with the pick just left"
            );
            assert_eq!(app.world().resource::<Roster>().pending_pick, None);
        }
    }
}
