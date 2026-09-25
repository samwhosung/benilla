//! `WOW_PROBE_LOOK`: the scripted mouse-turn, which drives the per-frame facing stream a key press
//! cannot.
//!
//! Format: `WOW_PROBE_LOOK="<deg_per_sec>@<start_s>:<duration_s>[;...]"`, e.g. `"90@20:6"` turns
//! the aim 90 degrees/s for 6 s from 20 s in; a negative rate turns the other way. It writes
//! [`Player::face_yaw`] directly, since the look session gates on the OS cursor being inside the
//! viewport; from there it is the path a mouse-turn takes to `MSG_MOVE_SET_FACING`.

use bevy::prelude::*;

use crate::player::Player;
use benilla_world::schedule::WorldStage;

use super::ProbeClock;

/// One scripted turn: `rate` (rad/s) over a window in wall-clock seconds ([`ProbeClock`]); the
/// virtual clock clamps its delta to 250 ms, which would under-rotate across a hitch.
struct Turn {
    rate: f32,
    at: f32,
    until: f32,
}

#[derive(Resource)]
pub(crate) struct ProbeLook {
    turns: Vec<Turn>,
}

/// Parses `WOW_PROBE_LOOK`; unparseable entries are skipped with a warning.
pub(crate) fn from_env() -> Option<ProbeLook> {
    let spec = std::env::var("WOW_PROBE_LOOK").ok()?;
    let turns: Vec<Turn> = spec
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            let (rate, rest) = s.split_once('@')?;
            let (at, dur) = rest.split_once(':')?;
            match (
                rate.trim().parse::<f32>(),
                at.trim().parse::<f32>(),
                dur.trim().parse::<f32>(),
            ) {
                (Ok(rate), Ok(at), Ok(dur)) => Some(Turn {
                    rate: rate.to_radians(),
                    at,
                    until: at + dur,
                }),
                _ => {
                    warn!("probe-look: unparseable turn {s:?} (want e.g. 90@20:6) — skipped");
                    None
                }
            }
        })
        .collect();
    (!turns.is_empty()).then_some(ProbeLook { turns })
}

/// Rotates the aim for every turn whose window covers this frame, before the controller, so the
/// same frame streams it.
pub(crate) fn drive_probe_look(
    probe: Res<ProbeLook>,
    time: ProbeClock,
    mut player: ResMut<Player>,
    self_player: Query<(), With<crate::net::SelfPlayer>>,
) {
    if self_player.is_empty() {
        return; // in-world only, like the other probes
    }
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    let rate: f32 = probe
        .turns
        .iter()
        .filter(|t| now >= t.at && now < t.until)
        .map(|t| t.rate)
        .sum();
    if rate != 0.0 {
        player.turn_aim(rate * dt);
    }
}

/// Registers `WOW_PROBE_LOOK` before [`crate::player::control`]; inert if the script parses to
/// nothing.
pub(crate) struct ProbeLookPlugin;

impl Plugin for ProbeLookPlugin {
    fn build(&self, app: &mut App) {
        let Some(look) = from_env() else {
            return;
        };
        app.insert_resource(look).add_systems(
            Update,
            drive_probe_look
                .in_set(WorldStage::Input)
                .before(crate::player::PlayerControlSet)
                .in_set(crate::char_select::InWorldGated),
        );
    }
}
