//! The frame-phase breakdown (`WOW_FRAME_PHASES=<ms>`): which phase of a slow frame spent it.
//!
//! - Between the main schedules the stamps are marker schedules spliced into
//!   [`bevy::app::MainScheduleOrder`], so each stamp is a schedule boundary, a hard sync point.
//! - Inside `Update` the stamps are exclusive systems around the four [`WorldStage`] sets. That
//!   perturbs `Update`'s parallelism, which is why the instrument is opt-in.
//!
//! The total runs frame start to frame start, and the remainder after `Last` prints as
//! `render+present`: the render sub-app, including pipelines compiled inline on the render
//! thread, is invisible to every stamp inside `Main`.
//!
//! One line per slow frame, spans in the order they closed:
//!
//! ```text
//! [phase] frame 812 total=60.5ms  PreUpdate=1.1 StateTransition=8.9 … render+present=31.2
//! ```
//!
//! `WOW_FRAME_PHASES=0` prints every frame.

use std::time::Instant;

use bevy::app::{First, Last, MainScheduleOrder, PostUpdate, PreUpdate, Update};
use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;

use benilla_world::schedule::WorldStage;

/// The marker schedules spliced between the main ones; the index is the position.
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PhaseMark(u8);

/// Each mark and the name of the span it closes; mark 0, before `First`, is the frame's t0.
const MARKS: &[(u8, &str)] = &[
    (0, ""), // before First: the frame's t0
    (1, "First"),
    (2, "PreUpdate"),
    (3, "StateTransition"),
    (4, "Update"),
    (5, "PostUpdate"),
    (6, "Last"),
];

/// The per-frame stamp tape, reported at the next frame's opening mark so the render sub-app's
/// share falls inside the window.
#[derive(Resource)]
struct Phases {
    /// Print a frame whose total exceeds this (ms).
    threshold_ms: f32,
    /// `(span name, instant at its end)`, in frame order; the first entry is the frame's t0.
    marks: Vec<(&'static str, Instant)>,
    frame: u64,
}

/// The armed threshold, read once.
fn threshold() -> Option<f32> {
    static T: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *T.get_or_init(|| {
        std::env::var("WOW_FRAME_PHASES")
            .ok()
            .map(|v| v.trim().parse::<f32>().unwrap_or(25.0))
    })
}

pub(super) fn plugin(app: &mut App) {
    let Some(threshold_ms) = threshold() else {
        return;
    };
    info!("frame phases: armed — printing any frame over {threshold_ms} ms (WOW_FRAME_PHASES)");
    app.insert_resource(Phases {
        threshold_ms,
        marks: Vec::with_capacity(16),
        frame: 0,
    });

    // Mark 0 opens the frame; every other mark closes the schedule it follows. `StatesPlugin`
    // has inserted `StateTransition` after `PreUpdate` by the time this plugin builds.
    {
        let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
        order.insert_before(First, PhaseMark(0));
        order.insert_after(First, PhaseMark(1));
        order.insert_after(PreUpdate, PhaseMark(2));
        order.insert_after(bevy::state::state::StateTransition, PhaseMark(3));
        order.insert_after(Update, PhaseMark(4));
        order.insert_after(PostUpdate, PhaseMark(5));
        order.insert_after(Last, PhaseMark(6));
    }
    for (i, name) in MARKS {
        let (i, name) = (*i, *name);
        app.add_systems(PhaseMark(i), move |mut p: ResMut<Phases>| {
            if i == 0 {
                // The previous frame closes here, not at the end of `Last`, so the render
                // sub-app and the present fall inside it.
                report(&mut p);
                p.frame += 1;
                p.marks.clear();
            }
            p.marks.push((name, Instant::now()));
        });
    }

    // Exclusive systems, so each stamp is a sync point and each span is its stage's own.
    app.add_systems(
        Update,
        (
            stamp("Update/pre-Net").before(WorldStage::Net),
            stamp("Update/Net")
                .after(WorldStage::Net)
                .before(WorldStage::Input),
            stamp("Update/Input")
                .after(WorldStage::Input)
                .before(WorldStage::Stream),
            stamp("Update/Stream")
                .after(WorldStage::Stream)
                .before(WorldStage::Present),
            stamp("Update/Present").after(WorldStage::Present),
        ),
    );
}

/// One exclusive stamp closing the named span.
fn stamp(name: &'static str) -> impl IntoSystem<(), (), ()> {
    IntoSystem::into_system(move |world: &mut World| {
        if let Some(mut p) = world.get_resource_mut::<Phases>() {
            p.marks.push((name, Instant::now()));
        }
    })
}

/// Print the previous frame's tape if it was slow. The `Update` stages fold in where they fell,
/// so the marker `Update` span is only what ran after `WorldStage::Present`.
fn report(p: &mut Phases) {
    let Some((_, t0)) = p.marks.first().copied() else {
        return;
    };
    let end = Instant::now();
    let total = (end - t0).as_secs_f32() * 1000.0;
    if total < p.threshold_ms {
        return;
    }
    let mut line = format!("[phase] frame {} total={total:.1}ms ", p.frame);
    let mut prev = t0;
    for (name, at) in p.marks.iter().skip(1) {
        let ms = (*at - prev).as_secs_f32() * 1000.0;
        prev = *at;
        if ms >= 0.05 {
            line.push_str(&format!(" {name}={ms:.1}"));
        }
    }
    let residue = (end - prev).as_secs_f32() * 1000.0;
    line.push_str(&format!(" render+present={residue:.1}"));
    println!("{line}");
}
