//! `WOW_PROBE_PITCH`: the scripted dive, aiming a swimming avatar's nose up or down. In play the
//! swim pitch (the wire's pitch tail, the body-pitch render `0x60a110`) is only written by
//! mouse-look, which needs a moving OS cursor in the viewport.
//!
//! Format: `WOW_PROBE_PITCH="<deg>@<start_s>[:<deg_per_sec>][;...]"`, nose-up positive like
//! `Player::mover_pitch` and the wire. `"-30@22;30@40"` aims 30 degrees down at 22 s and up at 40;
//! `"-45@20:9"` sweeps at 9 degrees/s from 20 s. The latest started entry wins and is re-applied
//! every frame. It writes [`Player::aim_pitch`], the field the reference's `SetPitch`
//! (`0x7c6f70`) writes, under the same 89-degree clamp.

use bevy::prelude::*;

use crate::player::Player;
use benilla_world::schedule::WorldStage;

use super::ProbeClock;

/// One scripted aim: pitch (radians, +up), start and sweep rate (rad/s), in wall-clock seconds
/// ([`ProbeClock`]).
struct Aim {
    pitch: f32,
    at: f32,
    sweep: f32,
}

#[derive(Resource)]
pub(crate) struct ProbePitch {
    aims: Vec<Aim>,
}

/// Parses `WOW_PROBE_PITCH`; unparseable entries are skipped with a warning.
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

/// Holds the latest started entry's aim, before the controller, so the same frame renders and
/// streams it.
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
    // The latest armed entry wins: a script is a timeline, not a sum.
    let Some(aim) = probe
        .aims
        .iter()
        .filter(|a| now >= a.at)
        .max_by(|a, b| a.at.total_cmp(&b.at))
    else {
        return;
    };
    // Log the stored value: `aim_pitch` clamps at 89 degrees either way.
    let held = player.aim_pitch(aim.pitch + aim.sweep * (now - aim.at));
    // One line per whole degree crossed.
    let whole = held.to_degrees().round();
    if announced.replace(whole) != Some(whole) {
        info!("probe-pitch: aiming {whole:+.0}° at t={now:.1}s");
    }
}

/// Registers `WOW_PROBE_PITCH` before [`crate::player::PlayerControlSet`]; inert if the script
/// parses to nothing.
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
