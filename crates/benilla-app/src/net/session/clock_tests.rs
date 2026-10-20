//! The game clock's four messages through the bridge: each lands on the one clock at the frame's
//! tick (`gametime/messages` in benilla-re).

use benilla_protocol::{SessionEvent, WowTime};
use benilla_world::frame_pace::FrameStep;
use bevy::prelude::*;

use super::on_clock;
use crate::net::ServerTime;

/// The handler over the resources the bridge takes, the frame five seconds in.
fn app() -> App {
    let mut app = App::new();
    app.init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::net::SelfGuid>()
        .init_resource::<crate::net::NetStatus>()
        .init_resource::<crate::names::NameCache>()
        .init_resource::<crate::items::Items>()
        .init_resource::<crate::spell::Cooldowns>()
        .init_resource::<crate::net::Reputations>()
        .init_resource::<ServerTime>()
        .init_resource::<crate::net::ServerWallClock>()
        .init_resource::<crate::net::HomeBind>()
        .init_resource::<crate::net::Proficiencies>()
        .init_resource::<crate::net::DroppedOpcodes>()
        .insert_resource(crate::net::NetCommands(crossbeam_channel::unbounded().0))
        .insert_resource(FrameStep {
            elapsed: std::time::Duration::from_secs(5),
            ..default()
        });
    app
}

fn send(app: &mut App, ev: SessionEvent) {
    app.world_mut()
        .run_system_cached_with(on_clock, ev)
        .expect("the handler runs");
}

fn clock(app: &App) -> crate::net::GameTime {
    app.world().resource::<ServerTime>().0
}

/// 2026-10-18, a Sunday, at 14:30.
fn sunday_1430() -> WowTime {
    WowTime::decode((26 << 24) | (9 << 20) | (17 << 14) | (14 << 6) | 30)
}

/// `0x6c6120`, `0x6c6010`, `0x6c5de0`: a set, a sync and a rate reach the clock; the set anchors
/// the sky's phase at the tick of the frame it came in.
#[test]
fn the_three_server_messages_reach_the_clock() {
    let mut app = app();
    send(&mut app, SessionEvent::GameSpeedSet { speed: 2.0 });
    assert_eq!(clock(&app).scale(), 2.0);
    assert!(!clock(&app).live(), "a rate alone is no time");

    send(
        &mut app,
        SessionEvent::GameTimeSet {
            time: sunday_1430(),
        },
    );
    let set = clock(&app);
    assert!(set.live());
    assert_eq!((set.hour(), set.minute()), (14, 30));
    assert!((set.phase(5_000) * 1440.0 - 870.0).abs() < 1e-3);

    let mut behind = sunday_1430();
    behind.minute = 20;
    send(&mut app, SessionEvent::GameTimeUpdate { time: behind });
    let synced = clock(&app);
    assert_eq!(
        (synced.hour(), synced.minute(), synced.skip()),
        (14, 30, 10)
    );
}

/// `0x6c5e80`: the login packet sets the time and the rate; an unset hour and minute land at 00:01.
#[test]
fn the_login_packet_sets_the_clock_and_its_rate() {
    let mut app = app();
    send(
        &mut app,
        SessionEvent::TimeSpeed {
            time: WowTime::decode(0xffff_ffff),
            speed: 1.0,
        },
    );
    let c = clock(&app);
    assert_eq!(
        (c.hour(), c.minute(), c.scale(), c.live()),
        (0, 1, 1.0, true)
    );
}
