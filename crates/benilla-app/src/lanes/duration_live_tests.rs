//! The player's buff timers as a listener reads them. `SMSG_UPDATE_AURA_DURATION`'s handler
//! stores `now + ms` over the slot's expiry (`0x4e4390` into `0xbc5f68`), and
//! `GetPlayerBuffTimeLeft` (`0x4e4930`) reads that array at the call (`0x4e4450`): the expiry of
//! the slot the buff sits in, less the clock, 0 once past. So a getter run with the host lent
//! reads the stamps the world holds, where the snapshot it falls back to holds what the last
//! push joined.

use benilla_ui::script::{AuraState, UiScript};
use bevy::prelude::*;

use crate::ui_aura::AuraDurations;

/// The buff in descriptor slot 3, at cache position 0.
fn buff() -> AuraState {
    AuraState {
        spell_id: 1126,
        slot: 3,
        count: 1,
        helpful: true,
        cancelable: true,
        ..Default::default()
    }
}

/// A client with a VM, one frame run (so the feed has kept the clock pair), and the buff pushed
/// with a stale expiry of its own.
fn client() -> bevy::app::App {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, _sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (_inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let script = UiScript::new().expect("a VM");
    script
        .run("function Left() return GetPlayerBuffTimeLeft(0) end")
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.update();
    let mut stale = buff();
    stale.expiration_time = 1000.0;
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_player_auras(vec![stale]);
    app
}

/// The stamps' clock where the last frame's feed read it.
fn stamp_clock(app: &bevy::app::App) -> f64 {
    app.world()
        .resource::<benilla_world::frame_pace::FrameStep>()
        .before_step()
        .1
}

/// A duration packet's handler: `ms` stored over `slot`, stamped at the stamps' clock.
fn stamp(app: &mut bevy::app::App, slot: u8, ms: i32) {
    let now = stamp_clock(app);
    app.world_mut()
        .resource_mut::<AuraDurations>()
        .set(slot, ms, now);
}

/// What `GetPlayerBuffTimeLeft(0)` answers, through the lent host when `live`.
fn left(app: &mut bevy::app::App, live: bool) -> f64 {
    if live {
        crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
            s.expect("a VM").eval::<f64>("return Left()").unwrap()
        })
    } else {
        crate::script_calls::eval_in_vm(app.world(), "return Left()")
    }
}

/// A stamp a handler wrote reads at once, with no push between, and a later one reads over it;
/// the snapshot holds the push's expiry throughout.
#[test]
fn a_timer_reads_the_stamp_the_world_holds() {
    let mut app = client();
    assert_eq!(left(&mut app, false), 1000.0, "control: the snapshot's");
    assert_eq!(left(&mut app, true), 0.0, "no stamp in slot 3 yet");
    stamp(&mut app, 3, 60_000);
    assert_eq!(left(&mut app, true), 60.0, "the packet's span");
    stamp(&mut app, 3, 30_000);
    assert_eq!(
        left(&mut app, true),
        30.0,
        "the next packet's, with no push between"
    );
    stamp(&mut app, 4, 90_000);
    assert_eq!(
        left(&mut app, true),
        30.0,
        "another slot's stamp is not this buff's"
    );
    stamp(&mut app, 3, -1);
    assert_eq!(
        left(&mut app, true),
        0.0,
        "a lapsed span reads 0 (`0x4e4472`)"
    );
    assert_eq!(left(&mut app, false), 1000.0, "the snapshot never moved");
}
