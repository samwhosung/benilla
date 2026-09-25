//! The FRAME-PHASE breakdown (`WOW_FRAME_PHASES=<ms>`) — *which phase* of a slow frame spent it.
//!
//! The instruments we had bracket this question from both sides and answer neither half of it.
//! `WOW_STREAM_TRACE` says a frame cost 411 ms and how many tiles/meshes/pipelines it touched;
//! the stall sampler shells `/usr/bin/sample` at a *600 ms* main-thread stall and hands
//! back a stack. Between them sits the whole class this project actually reports — the
//! 40–500 ms world-entry and teleport hitches (0962, 1116, 1345) — where the diagnosis has each
//! time been *"guess the suspect, then time it by hand"*. 1345 named that cost outright: the
//! FrameXML burst was found by reading a log's wall-clock stamps and subtracting.
//!
//! So: stamp the frame at boundaries that are **exact by construction**, and print the
//! breakdown for any frame over the threshold.
//!
//! - **Between the main schedules** the stamps are their own *marker schedules*, spliced into
//!   [`bevy::app::MainScheduleOrder`]. A schedule boundary is a hard sync point, so the stamp is
//!   the boundary — no ordering constraint to get wrong, no executor freedom to float in.
//! - **Inside `Update`** the stamps ride the four [`WorldStage`] sets the frame contract already
//!   defines (0737's Net → Input → Stream → Present chain), as **exclusive** systems so each one
//!   is a sync point too. That is a real perturbation of `Update`'s parallelism — which is why it
//!   is opt-in, and why the printed line carries `total=` measured across the whole frame rather
//!   than summed from the parts.
//!
//! **The residue is the point, not an afterthought.** A frame is `Main` *plus* the render
//! sub-app — extract, prepare, queue, render, present — and on macOS every first-sight pipeline
//! variant is compiled with `block_on` **inline on the render thread** (`pipe_warm`'s header),
//! which is invisible to every stamp inside `Main`. So the total is measured frame-start to
//! frame-start and the unaccounted remainder is printed as its own span, `render+present`. The
//! first cut of this instrument closed the tape at the end of `Last` and silently reported a
//! 506 ms frame as fitting inside 20 ms of `Main`.
//!
//! One line per slow frame, spans in the order they closed:
//!
//! ```text
//! [phase] frame 1972 total=60.5ms  First=0.2 PreUpdate=1.1 StateTransition=8.9 … render+present=31.2
//! ```
//!
//! Off unless `WOW_FRAME_PHASES` is set; `WOW_FRAME_PHASES=0` prints every frame.

use std::time::Instant;

use bevy::app::{First, Last, MainScheduleOrder, PostUpdate, PreUpdate, Update};
use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;

use benilla_world::schedule::WorldStage;

/// The marker schedules spliced between the main ones. A tuple label rather than seven unit
/// structs: the index IS the position, so the splice table below reads as the frame's shape.
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PhaseMark(u8);

/// Where each mark is spliced, and what the *preceding* span is called. `None` = the frame's
/// opening stamp (spliced before `First`), which names no span of its own.
const MARKS: &[(u8, &str)] = &[
    (0, ""), // before First — the frame's t0
    (1, "First"),
    (2, "PreUpdate"),
    (3, "StateTransition"),
    (4, "Update"),
    (5, "PostUpdate"),
    (6, "Last"),
];

/// The per-frame stamp tape. Read (and reported) at the NEXT frame's opening mark, so the
/// render sub-app's share of this frame is inside the window — see the module doc.
#[derive(Resource)]
struct Phases {
    /// Print a frame whose total exceeds this (ms).
    threshold_ms: f32,
    /// `(span name, instant at its END)`, in frame order; the first entry is the frame's t0.
    marks: Vec<(&'static str, Instant)>,
    frame: u64,
}

/// Is the instrument armed, and at what threshold? (Read once.)
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

    // Splice the marker schedules. `insert_before(First, …)` opens the frame; every other mark
    // closes the schedule it is inserted after. `StateTransition` is inserted by `StatesPlugin`
    // after `PreUpdate`, so it is in the list by the time this plugin builds.
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
                // The previous frame closes HERE, not at the end of `Last`: everything between
                // `Last` and this stamp is the render sub-app and the present.
                report(&mut p);
                p.frame += 1;
                p.marks.clear();
            }
            p.marks.push((name, Instant::now()));
        });
    }

    // The four `Update` stages, stamped as EXCLUSIVE systems so each stamp is a sync point and
    // the span either side of it is the stage's own (see the module doc's perturbation note).
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

/// Print the previous frame's tape if it was slow. Spans print in the order they closed, with
/// the `Update` stages folded in where they fell — the marker-schedule `Update` entry then
/// reports only what ran after `WorldStage::Present` — and the remainder between the end of
/// `Last` and this call as `render+present`.
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
