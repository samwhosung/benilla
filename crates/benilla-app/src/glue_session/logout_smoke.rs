//! The logout smoke, `scripts/smoke.sh`'s round trip: a world entry, a logout back to character
//! select on the same link, and a second entry. Ungated, since it crosses states and runs over
//! either glue screen set.

use bevy::prelude::*;

use super::roster::{press_enter_world, send_pick, Roster};
use crate::client_state::ClientState;
use crate::net::{CharListMessage, CharPick};
use crate::ui_script::glue::{StockGlueProbe, StockVm};

/// Enter the world again as the roster's first character, the way a player does on the screens
/// that stand: on the stock screens their Enter World ([`press_enter_world`]); the native screens'
/// Enter World is the pick itself.
pub(crate) fn reenter_first(stock: Option<StockVm>, roster: &mut Roster, pick: &CharPick) {
    match stock {
        Some(vm) => press_enter_world(vm, 0),
        None => {
            if let Some(guid) = roster.chars.first().map(|c| c.guid) {
                send_pick(roster, pick, guid);
            }
        }
    }
}

/// The logout smoke (`WOW_LOGOUT_SMOKE=1`, with `WOW_CHAR`): once in the world, `/logout`, return
/// to CharSelect, wait for the roster the logout brings on the link the world was entered by,
/// enter the world again and close the window. The second entry streams a map
/// `terrain_stream::release_world` tore down. It ends by a window close, not an `AppExit`, since
/// only the close reaches the shutdown tail that saves variables and addon files.
pub(super) fn debug_logout_smoke(
    state: Res<State<ClientState>>,
    player: Res<crate::player::Player>,
    commands: Res<crate::net::NetCommands>,
    mut roster: ResMut<Roster>,
    pick: Res<CharPick>,
    streamer: Res<benilla_world::terrain_stream::TerrainStreamer>,
    time: Res<Time>,
    mut exit: MessageWriter<AppExit>,
    mut close: MessageWriter<bevy::window::WindowCloseRequested>,
    windows: Query<Entity, With<bevy::window::PrimaryWindow>>,
    mut lists: MessageReader<CharListMessage>,
    stock: StockGlueProbe,
    // The world session's number at the entry, and whether a roster has come since the logout.
    mut link: Local<Option<u64>>,
    mut relisted: Local<bool>,
    mut phase: Local<u8>,
    mut mark: Local<f32>,
) {
    if std::env::var("WOW_LOGOUT_SMOKE").is_err() {
        return;
    }
    for list in lists.read() {
        if *phase < 2 {
            *link = Some(list.epoch);
        } else if !*relisted {
            *relisted = true;
            if Some(list.epoch) == *link {
                info!(
                    "logout-smoke: roster after the logout on the same link — {} char(s)",
                    list.characters.len()
                );
            } else {
                error!(
                    "logout-smoke: FAILED — the roster after the logout came on world session {} \
                     where the world was entered on {:?}",
                    list.epoch, *link
                );
            }
        }
    }
    let now = time.elapsed_secs();
    match *phase {
        0 if *state.get() == ClientState::InWorld && player.active => {
            info!("logout-smoke: seated in world — lingering");
            (*phase, *mark) = (1, now);
        }
        1 if now - *mark > 3.0 => {
            info!("logout-smoke: requesting logout");
            let _ = commands.0.send(crate::net::ClientCommand::Logout);
            *phase = 2;
        }
        2 if *state.get() == ClientState::CharSelect => {
            info!("logout-smoke: back at character select — lingering");
            (*phase, *mark) = (3, now);
        }
        // Residency is read here, not on arrival: `release_world` runs about 150 ms after the
        // state edge, on the world-live falling edge.
        // The roster the logout brings, not the one that survived it, decides the re-entry. On
        // the stock screens, once their select screen stands to press.
        3 if now - *mark > 4.0
            && *relisted
            && stock
                .vm()
                .is_none_or(|vm| vm.frame_visible("CharacterSelect")) =>
        {
            match roster.chars.first().map(|c| c.name.clone()) {
                Some(name) => {
                    info!(
                        "logout-smoke: {} tiles resident after the release (must be 0)",
                        streamer.residency().1
                    );
                    info!("logout-smoke: re-entering the world as {name}");
                    reenter_first(stock.vm(), &mut roster, &pick);
                    (*phase, *mark) = (4, now);
                }
                None => {
                    warn!("logout-smoke: empty roster — cannot test re-entry");
                    *phase = 5;
                }
            }
        }
        4 if *state.get() == ClientState::InWorld && player.active && now - *mark > 3.0 => {
            // The suppressor reading catches a character that re-entered unable to move;
            // `scripts/smoke.sh` fails on anything but `none`. A GM account's logout is instant,
            // not rooted, so it does not exercise the rooted path.
            info!(
                "logout-smoke: re-entered — {} tiles resident, suppressors: {}, done",
                streamer.residency().1,
                player.movement_suppressors()
            );
            *phase = 5;
        }
        5 => {
            info!("logout-smoke: done");
            // Close the window as a player does; a headless run has none and exits directly.
            match windows.single() {
                Ok(window) => {
                    close.write(bevy::window::WindowCloseRequested { window });
                }
                Err(_) => {
                    warn!("logout-smoke: no primary window — exiting by AppExit instead");
                    exit.write(AppExit::Success);
                }
            }
            *phase = 6;
        }
        _ => {}
    }
}
