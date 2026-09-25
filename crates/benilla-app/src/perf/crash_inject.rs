//! `WOW_CRASH_INJECT=<at_secs>` — one deliberate main-thread panic, mid-run: the crash reporter's
//! standing test affordance (`crate::crash`), in the shape of the stall
//! sampler's own injectors. The end-to-end falsifier is a run with it set: the process must die
//! with a `crash-<unix>.txt` whose log tail ends in the `crash-inject` line below.
//!
//! **Why it lives here and not in `crash.rs`, and not in `stall.rs`.** An instrument's home is a
//! dev root (`run_mode::the_dev_plane_has_exactly_one_door`, 1176): the reporter ships to
//! players and knows nothing of the `dev` seam, so its injector cannot sit beside it under a
//! `cfg`. And it was first armed from inside the stall sampler, which is macOS-only
//! (`/usr/bin/sample`) — so on Linux and Windows the injector was dead code, `-D warnings` made
//! that a red cross-platform compile on both, and the macOS gates stayed green for six days. An
//! instrument's cfg is `dev`, never a platform's; `PerfPlugin` arms this on every platform.

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
