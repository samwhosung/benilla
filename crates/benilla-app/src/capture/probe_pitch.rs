//! `WOW_PROBE_PITCH` — the scripted **dive**: aim a swimming avatar's nose up or down without a
//! human on the mouse. The twin of [`super::probe_look`]'s scripted turn, and it closes the same
//! shape of gap.
//!
//! The swim body pitch has exactly one writer in gameplay — the mouse-look push in
//! `player::controller`, gated on `mouselook` (a held right button or MOVEANDSTEER) *and* a moving
//! OS cursor inside the viewport. An unfocused probe window has neither. So the whole observed-swim
//! lane — the wire's pitch tail, the dead-reckon's pitched travel basis, and the body-pitch render
//! law (`0x60a110`) — was **unreachable from any script**: a benilla client could be
//! made to swim, but only ever level. The tilt shipped in July 2026 with no test, no trace field
//! and no way to drive it, which is why "do other swimmers tilt?" had no answer short of the
//! director's eye, and why a two-client probe of it could not be written at all.
//!
//! Format: `WOW_PROBE_PITCH="<deg>@<start_s>[:<deg_per_sec>][;…]"` — `deg` is nose-**up** positive,
//! matching `Player::mover_pitch` and the wire's own sign. `"-30@22;30@40"` aims 30° down at
//! twenty-two seconds and 30° up at forty; `"-45@20:9"` starts level-ish and sweeps at 9°/s from
//! twenty seconds on, which is what a slow real dive looks like on the wire. The last entry whose
//! start has passed wins, and it is re-asserted **every frame** — the reference holds an unsteered
//! pitch rather than levelling it, so a one-shot write would be indistinguishable from a
//! held one; re-asserting is what makes the two separable when only one of them is under test.
//!
//! It writes [`Player::aim_pitch`] — the field the reference's `SetPitch` (`0x7c6f70`) writes,
//! under the same ±89° clamp — rather than faking mouse motion, for [`super::probe_look`]'s reason
//! exactly: the camera plumbing is not what is under test, and its `cursor_in_viewport` gate is
//! unreliable for a window nobody is looking at.

use bevy::prelude::*;

use crate::player::Player;
use benilla_world::schedule::WorldStage;

use super::ProbeClock;

/// One scripted aim: the target pitch (radians, +up), when it takes effect, and an optional sweep
/// rate (rad/s) applied from that moment — **wall-clock seconds** ([`ProbeClock`]),
/// for [`super::probe_look`]'s reason: a dive scripted to reach 30° by second 25 has to reach it at
/// second 25 whatever the frame rate did, and the wire tail it feeds is paced on the real clock.
struct Aim {
    pitch: f32,
    at: f32,
    sweep: f32,
}

#[derive(Resource)]
pub(crate) struct ProbePitch {
    aims: Vec<Aim>,
}

/// Parse the env script. Absent or unparseable entries yield no aims (with a warning), so the probe
/// is inert unless asked for.
pub(crate) fn from_env() -> Option<ProbePitch> {
    let spec = std::env::var("WOW_PROBE_PITCH").ok()?;
    let aims: Vec<Aim> = spec
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            let (deg, rest) = s.split_once('@')?;
            let (at, sweep) = match rest.split_once(':') {
                Some((at, sweep)) => (at, sweep.trim().parse::<f32>().ok()?),
                None => (rest, 0.0),
            };
            match (deg.trim().parse::<f32>(), at.trim().parse::<f32>()) {
                (Ok(deg), Ok(at)) => Some(Aim {
                    pitch: deg.to_radians(),
                    at,
                    sweep: sweep.to_radians(),
                }),
                _ => {
                    warn!("probe-pitch: unparseable aim {s:?} (want e.g. -30@22:9) — skipped");
                    None
                }
            }
        })
        .collect();
    (!aims.is_empty()).then_some(ProbePitch { aims })
}

/// Hold the aim of the latest entry whose start has passed. Runs in `WorldStage::Input` **before**
/// the controller, so the swim frame that reads the pitch is the frame that renders and streams it.
pub(crate) fn drive_probe_pitch(
    probe: Res<ProbePitch>,
    time: ProbeClock,
    mut player: ResMut<Player>,
    self_player: Query<(), With<crate::net::SelfPlayer>>,
    mut announced: Local<Option<f32>>,
) {
    if self_player.is_empty() {
        return; // in-world only, like the other probes
    }
    let now = time.elapsed_secs();
    // The LAST armed entry wins, so a script reads as a timeline rather than a sum — a dive
    // followed by a climb is two entries, not an arithmetic accident.
    let Some(aim) = probe
        .aims
        .iter()
        .filter(|a| now >= a.at)
        .max_by(|a, b| a.at.total_cmp(&b.at))
    else {
        return;
    };
    // The STORED value, not the requested one: `aim_pitch` clamps at ±89°, and a sweep that runs
    // past it must not log a number the body never held.
    let held = player.aim_pitch(aim.pitch + aim.sweep * (now - aim.at));
    // One line per whole degree crossed: enough to read the timeline back out of the log, few
    // enough not to drown it. The observer's own reading is the point of comparison.
    let whole = held.to_degrees().round();
    if announced.replace(whole) != Some(whole) {
        info!("probe-pitch: aiming {whole:+.0}° at t={now:.1}s");
    }
}

/// `WOW_PROBE_PITCH`'s registration — the same shape (and the same ordering rationale) as
/// [`super::probe_look`]'s: inert with no resource and no systems when the script parses to
/// nothing, and ordered before [`crate::player::PlayerControlSet`] so the controller knows nothing
/// about it.
pub(crate) struct ProbePitchPlugin;

impl Plugin for ProbePitchPlugin {
    fn build(&self, app: &mut App) {
        let Some(pitch) = from_env() else {
            return;
        };
        app.insert_resource(pitch).add_systems(
            Update,
            drive_probe_pitch
                .in_set(WorldStage::Input)
                .before(crate::player::PlayerControlSet)
                .in_set(crate::char_select::InWorldGated),
        );
    }
}
