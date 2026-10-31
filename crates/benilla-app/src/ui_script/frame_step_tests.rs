//! The UI's clock through the real UI pass: `OnUpdate(self, elapsed)` gets the frame time the
//! loop measured (`0x420f70` → `0x442330` → `0x442350` → `0x764330` → `0x76b2c0`), here the paced
//! frame step, so a mis-split pair of real deltas reaches the script as two even steps and a stall
//! reaches it whole. benilla's `GetTime` advances by the same step (a deviation, `script/clock.rs`).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::time::{Real, TimeSystems, Virtual};
use bevy::window::PrimaryWindow;

use benilla_ui::script::UiScript;
use benilla_world::frame_pace::FrameStep;

use super::extract::tick_script;
use crate::portrait::PortraitImages;

const P: f64 = 1.0 / 60.0;

/// The wall instants the next frames' `Time<Real>` reads, one as each frame starts.
#[derive(Resource, Default)]
struct Wall(VecDeque<Instant>);

/// Bevy's clock tick, fed from [`Wall`]; the pacer runs after it, as in the app.
fn tick_clock(
    mut wall: ResMut<Wall>,
    mut real: ResMut<Time<Real>>,
    mut virt: ResMut<Time<Virtual>>,
    mut generic: ResMut<Time>,
) {
    if let Some(t) = wall.0.pop_front() {
        real.update_with_instant(t);
    }
    bevy::time::update_virtual_time(&mut generic, &mut virt, &real);
}

/// The interface pass with an `OnUpdate` probe and a primary window, no clock yet.
fn ui_app() -> App {
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"
        STEPS, TIMES = {}, {}
        local f = CreateFrame("Frame", "Probe")
        f:SetScript("OnUpdate", function()
            table.insert(STEPS, arg1)
            table.insert(TIMES, GetTime())
        end)
    "#,
        )
        .unwrap();
    let mut app = App::new();
    app.insert_non_send_resource(script);
    app.init_resource::<super::UiPassState>();
    app.init_resource::<Assets<Image>>();
    app.init_resource::<PortraitImages>();
    app.init_resource::<crate::portrait::BoothPanes>();
    app.init_resource::<crate::portrait::PaneRequests>();
    app.init_resource::<crate::ui_models::UiModelTiles>();
    app.init_resource::<crate::minimap::MinimapWidget>();
    app.init_resource::<super::UiFrameCost>();
    app.init_resource::<super::UiCostWanted>();
    app.init_resource::<super::UiScaleCvar>();
    app.init_resource::<super::UiClock>();
    app.add_systems(Update, tick_script);
    app.world_mut().spawn((
        Window {
            resolution: UVec2::new(1024, 768).into(),
            ..default()
        },
        PrimaryWindow,
    ));
    app
}

fn app(t0: Instant) -> App {
    let mut app = ui_app();
    app.insert_resource(Time::<Real>::new(t0));
    app.init_resource::<Time<Virtual>>();
    app.init_resource::<Time>();
    app.init_resource::<Wall>();
    app.add_plugins(benilla_world::frame_pace::plugin);
    app.add_systems(First, tick_clock.in_set(TimeSystems));
    app
}

/// One frame at wall instant `t`.
fn frame(app: &mut App, t: Instant) {
    app.world_mut().resource_mut::<Wall>().0.push_back(t);
    app.update();
}

fn eval(app: &App, chunk: &str) -> f64 {
    app.world()
        .non_send_resource::<UiScript>()
        .eval::<f64>(chunk)
        .unwrap()
}

/// The last `OnUpdate`'s `elapsed` and `GetTime()`.
fn last(app: &App) -> (f64, f64) {
    (
        eval(app, "return STEPS[table.getn(STEPS)]"),
        eval(app, "return TIMES[table.getn(TIMES)]"),
    )
}

#[test]
fn mis_split_real_deltas_reach_on_update_and_gettime_as_even_steps_and_a_stall_whole() {
    let t0 = Instant::now();
    let mut app = app(t0);
    let secs = Duration::from_secs_f64;
    let mut t = t0;
    frame(&mut app, t);
    // A steady 60 Hz window, so the pacer sees the cadence.
    for _ in 0..30 {
        t += secs(P);
        frame(&mut app, t);
    }
    let (_, start) = last(&app);
    let step_start = app.world().resource::<FrameStep>().elapsed_secs_f64();
    let wall_start = t;
    let mut prev_time = start;
    // Every third pair mis-split at 26.4 ms, the halves still two refreshes apart on the wall.
    for i in 0..60 {
        t += secs(match i % 6 {
            4 => 0.0264,
            5 => 2.0 * P - 0.0264,
            _ => P,
        });
        frame(&mut app, t);
        let (elapsed, now) = last(&app);
        assert!(
            (elapsed - P).abs() < 1.0e-3,
            "frame {i}: OnUpdate got {elapsed} s, not one refresh"
        );
        assert!(
            (now - prev_time - elapsed).abs() < 1.0e-6,
            "frame {i}: GetTime moved {} s for an elapsed of {elapsed} s",
            now - prev_time
        );
        prev_time = now;
        let step = *app.world().resource::<FrameStep>();
        let clock = app.world().resource::<super::UiClock>();
        assert_eq!(
            clock.anchor, step.at,
            "frame {i}: the clock pair is the frame step's"
        );
    }
    let wall = (t - wall_start).as_secs_f64();
    let paced = app.world().resource::<FrameStep>().elapsed_secs_f64() - step_start;
    assert!(
        (prev_time - start - paced).abs() < 1.0e-5 && (paced - wall).abs() < 0.011,
        "GetTime advanced {} s, the frame step {paced} s, over {wall} s of wall clock",
        prev_time - start
    );
    // A real stall, longer than the virtual clock's 250 ms ceiling.
    t += secs(1.82);
    frame(&mut app, t);
    let (elapsed, now) = last(&app);
    assert!(
        (elapsed - 1.82).abs() < 0.011,
        "the stall reached OnUpdate as {elapsed} s"
    );
    assert!(
        (now - prev_time - elapsed).abs() < 1.0e-5,
        "and GetTime by the same step"
    );
}

/// The loading screen's layer covers the unit square with flag bit 0 clear, so the frame pass takes
/// it out of what the interface's layer can see (`0x442431`-`0x442437`) and skips the empty layer
/// (`0x4424b9`/`0x4424ca`): no `OnUpdate` under it. The elapsed time is cleared after each pass
/// (`0x4426c3`), so the first walk after the reveal gets one frame's, and `GetTime` keeps the wall.
#[test]
fn no_on_update_runs_under_the_loading_screen_and_the_first_after_it_gets_one_frame() {
    let t0 = Instant::now();
    let mut app = app(t0);
    let secs = Duration::from_secs_f64;
    let mut t = t0;
    frame(&mut app, t);
    for _ in 0..30 {
        t += secs(P);
        frame(&mut app, t);
    }
    let walks = eval(&app, "return table.getn(STEPS)");
    let (_, before) = last(&app);
    app.insert_resource(crate::loading_screen::LoadingScreen::test_covering());
    for _ in 0..20 {
        t += secs(P);
        frame(&mut app, t);
    }
    assert_eq!(
        eval(&app, "return table.getn(STEPS)"),
        walks,
        "an OnUpdate ran under the loading screen"
    );
    app.insert_resource(crate::loading_screen::LoadingScreen::default());
    t += secs(P);
    frame(&mut app, t);
    let (elapsed, now) = last(&app);
    assert!(
        (elapsed - P).abs() < 1.0e-3,
        "the first OnUpdate after the reveal got {elapsed} s, not one frame's"
    );
    assert!(
        (now - before - 21.0 * P).abs() < 1.0e-3,
        "GetTime moved {} s across the cover, not the wall's",
        now - before
    );
}

/// A main-thread stall the next frame's [`stall`] system sleeps through, after the interface pass.
#[derive(Resource, Default)]
struct Stall(Option<Duration>);

fn stall(mut stall: ResMut<Stall>) {
    if let Some(d) = stall.0.take() {
        std::thread::sleep(d);
    }
}

/// A 600 ms main-thread stall reaches `GetTime` by the next interface pass, as 1.12.1's clock does:
/// its loop reads the frame time once per tick on the main thread (`0x420f70`) and `GetTime` reads
/// it live. Under pipelined rendering Bevy stamps `Time<Real>` with the instant the render world
/// sent after presenting the frame before, so the frame after a stall is stamped before the stall
/// ended and bills it a frame late, so a `GetTime` read in that frame sat the stall behind the
/// wall. The render thread is emulated here: each
/// frame it hands over the instant the previous frame started, which the stall has not reached.
#[test]
fn a_600_ms_stall_reaches_gettime_by_the_next_frame_under_pipelined_rendering() {
    let mut app = ui_app();
    app.add_plugins(bevy::time::TimePlugin);
    let (render_clock, receiver) = bevy::time::create_time_channels();
    app.insert_resource(receiver);
    app.add_plugins(benilla_world::frame_pace::plugin);
    app.init_resource::<Stall>();
    app.add_systems(PostUpdate, stall);
    let mut presented = Instant::now();
    let mut frame = |app: &mut App| {
        let start = Instant::now();
        // The render thread may already be gone (dropped receiver): Bevy ignores that error too.
        let _ = render_clock.0.try_send(presented);
        app.update();
        presented = start;
    };
    for _ in 0..10 {
        frame(&mut app);
    }
    let (_, before) = last(&app);
    app.world_mut().resource_mut::<Stall>().0 = Some(Duration::from_millis(600));
    frame(&mut app);
    let (_, during) = last(&app);
    let wall = Instant::now();
    frame(&mut app);
    let (elapsed, after) = last(&app);
    let wall = wall.elapsed().as_secs_f64();
    assert!(
        after - before >= 0.595,
        "GetTime moved {} s over the frame that stalled 600 ms and the next ({} s, then {} s)",
        after - before,
        during - before,
        after - during
    );
    assert!(
        elapsed >= 0.595 && (after - during - elapsed).abs() < 1.0e-5,
        "the frame after the stall handed OnUpdate {elapsed} s and moved GetTime {} s",
        after - during
    );
    // And not ahead of the wall: nothing past the stall and the frames around it.
    assert!(
        after - before < 0.6 + wall + 0.25,
        "GetTime ran ahead: {} s",
        after - before
    );
}
