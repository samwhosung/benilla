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
    let mut script = UiScript::new().expect("a VM");
    // A clock far from the stamps', so a swap of the pair's two clocks shows.
    script.set_now(1000.0);
    crate::ui_script::test_ui::load_tooltip_template(&script);
    script
        .run(
            r#"
            function Left() return GetPlayerBuffTimeLeft(0) end
            SPELL_TIME_REMAINING_SEC = "%d second remaining"
            SPELL_TIME_REMAINING_SEC_P1 = "%d seconds remaining"
            AuraTip = CreateFrame("GameTooltip", "AuraTip", nil, "TestTooltipTemplate")
            AuraOwner = CreateFrame("Frame")
            function Lines()
                AuraTip:SetOwner(AuraOwner, 'ANCHOR_NONE')
                AuraTip:SetPlayerBuff(0)
                local t = {}
                for i = 1, AuraTip:NumLines() do
                    table.insert(t, getglobal('AuraTipTextLeft' .. i):GetText() or '')
                end
                return table.concat(t, '|')
            end
            "#,
        )
        .unwrap();
    script.set_aura_tooltip(
        benilla_ui::script::AuraKey {
            spell_id: 1126,
            level: 0,
            stack: 1,
        },
        benilla_ui::script::AuraTooltipView {
            name: "Mark".into(),
            description: "Stats.".into(),
            ..Default::default()
        },
    );
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.update();
    let mut stale = buff();
    stale.expiration_time = 1040.0;
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
    assert_eq!(left(&mut app, false), 40.0, "control: the snapshot's");
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
    assert_eq!(left(&mut app, false), 40.0, "the snapshot never moved");
}

/// The buff tooltip's time line reads the same stamp at the call (`0x532be3` → `0x4e4450`).
#[test]
fn the_buff_tooltip_reads_the_stamp_the_world_holds() {
    let mut app = client();
    let lines = |app: &mut bevy::app::App, live: bool| -> String {
        if live {
            crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
                s.expect("a VM").eval::<String>("return Lines()").unwrap()
            })
        } else {
            crate::script_calls::eval_in_vm(app.world(), "return Lines()")
        }
    };
    assert_eq!(lines(&mut app, false), "Mark|Stats.|40 seconds remaining");
    assert_eq!(lines(&mut app, true), "Mark|Stats.|0 seconds remaining");
    stamp(&mut app, 3, 30_000);
    assert_eq!(lines(&mut app, true), "Mark|Stats.|30 seconds remaining");
}
