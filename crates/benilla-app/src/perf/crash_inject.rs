//! `WOW_CRASH_INJECT=<at_secs>`: one deliberate main-thread panic mid-run, the test affordance
//! for the crash reporter (`crate::crash`); a run with it set must die with a `crash-<unix>.txt`
//! whose log tail ends in the `crash-inject` line.
//!
//! It lives in the dev plane, not beside the reporter, which ships to players, and is armed on
//! every platform: an instrument's cfg is `dev`, never a platform's.

use bevy::prelude::*;

/// Arm the injector when the switch is set; register nothing otherwise.
pub(super) fn arm(app: &mut App) {
    if let Some(at) = std::env::var("WOW_CRASH_INJECT")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        app.insert_resource(CrashInject { at });
        app.add_systems(Update, crash_inject);
    }
}

#[derive(Resource)]
struct CrashInject {
    at: f32,
}

fn crash_inject(inject: Res<CrashInject>, time: Res<Time<bevy::time::Real>>) {
    if time.elapsed_secs() >= inject.at {
        warn!(
            "crash-inject: panicking the main thread at {:.1} s",
            inject.at
        );
        panic!("crash-inject: deliberate panic (WOW_CRASH_INJECT)");
    }
}
