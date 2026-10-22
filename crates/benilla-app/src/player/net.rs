//! The player's packet handlers: `SMSG_STANDSTATE_UPDATE` goes to [`super::posture`] as a
//! [`ServerStandState`], applied in `control`, which holds the body query and sheath queue;
//! `SMSG_PLAYER_SKINNED`, among the player object's messages in the reference (`0x5e3010`),
//! signals `PLAYER_SKINNED`.

use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::ScriptValue;
use bevy::prelude::*;

use super::ServerStandState;
use crate::net::NetHandlerApp;

/// Registers the handlers; called from [`super::PlayerPlugin`].
pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::StandStateUpdate, on_stand_state_update)
        .net_handler(SessionEventKind::PlayerSkinned, on_player_skinned)
        .init_resource::<SkinnedFeed>()
        .add_systems(
            Update,
            feed_player_skinned
                .in_set(crate::ui_script::UiFeed)
                // A signal fired into a VM with no frames yet is lost: it waits for the UI.
                .run_if(crate::ui_script::ingame_ui_up),
        )
        .add_systems(
            OnExit(crate::client_state::ClientState::InWorld),
            drop_unsignalled_skins,
        );
}

fn on_stand_state_update(In(ev): In<SessionEvent>, mut out: MessageWriter<ServerStandState>) {
    if let SessionEvent::StandStateUpdate { state } = ev {
        out.write(ServerStandState { state });
    }
}

/// The `SMSG_PLAYER_SKINNED` byte truths not yet signalled, in packet order.
#[derive(Resource, Default)]
struct SkinnedFeed(Vec<bool>);

fn on_player_skinned(In(ev): In<SessionEvent>, mut feed: ResMut<SkinnedFeed>) {
    if let SessionEvent::PlayerSkinned { free_repop } = ev {
        feed.0.push(free_repop);
    }
}

/// `PLAYER_SKINNED` with the byte's truth as `arg1` (`0x48f400`, `%d`); the stock `UIParent.lua`
/// handler hides the resurrect offers and prints `DEATH_CORPSE_SKINNED`.
fn feed_player_skinned(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut feed: ResMut<SkinnedFeed>,
) {
    hosted_vm.run(|script| {
        // No VM (a capture or headless run): drop them so the queue stays bounded.
        let drained = feed.0.drain(..);
        let Some(script) = script else { return };
        for free_repop in drained {
            script.fire_event(
                "PLAYER_SKINNED",
                vec![ScriptValue::Int(i64::from(free_repop))],
            );
        }
    })
}

/// `OnExit(InWorld)`: a signal still waiting dies with the world's interface, which the reference
/// would have fired it into.
fn drop_unsignalled_skins(mut feed: ResMut<SkinnedFeed>) {
    feed.0.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::script::UiScript;

    /// Each packet signals once, in order, with its byte's truth as `arg1` (`0x5e7d54`,
    /// `0x48f407`), once the interface is up to hear it.
    #[test]
    fn player_skinned_signals_its_byte_as_arg1() {
        let mut app = App::new();
        register(&mut app);
        let script = UiScript::new().unwrap();
        script
            .run(
                "SKINNED = {}\n\
                 local f = CreateFrame(\"Frame\")\n\
                 f:RegisterEvent(\"PLAYER_SKINNED\")\n\
                 f:SetScript(\"OnEvent\", function() tinsert(SKINNED, arg1) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![
                SessionEvent::PlayerSkinned { free_repop: false },
                SessionEvent::PlayerSkinned { free_repop: true },
            ],
        );
        let skinned = |app: &App| -> Vec<i64> {
            app.world()
                .non_send_resource::<UiScript>()
                .eval("return SKINNED")
                .unwrap()
        };
        app.update();
        assert_eq!(
            skinned(&app),
            Vec::<i64>::new(),
            "no interface yet: they wait"
        );

        app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        app.update();
        assert_eq!(skinned(&app), [0, 1]);
    }

    /// One still waiting when the world is left is not fired into the next world's interface.
    #[test]
    fn a_waiting_player_skinned_dies_with_the_world() {
        let mut app = App::new();
        register(&mut app);
        let script = UiScript::new().unwrap();
        script
            .run(
                "SKINNED = 0\n\
                 local f = CreateFrame(\"Frame\")\n\
                 f:RegisterEvent(\"PLAYER_SKINNED\")\n\
                 f:SetScript(\"OnEvent\", function() SKINNED = SKINNED + 1 end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::PlayerSkinned { free_repop: false }],
        );
        app.update();
        app.world_mut()
            .run_schedule(OnExit(crate::client_state::ClientState::InWorld));
        app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        app.update();
        let fired: i64 = app
            .world()
            .non_send_resource::<UiScript>()
            .eval("return SKINNED")
            .unwrap();
        assert_eq!(fired, 0);
    }
}
