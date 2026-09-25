//! The cast-cancel live probe (`WOW_PROBE=castcancel`): once in-world it uses the Hearthstone (a
//! 10 s cast), then 2 s into the bar presses `W` through `ButtonInput<KeyCode>`, the controller
//! path a player's key takes to `spell::local_self_cancel` and `CMSG_CANCEL_CAST`.
//!
//! With `WOW_CAST_TRACE=1` the `LOCAL self-cancel` line must land frames after the `SEND move
//! StartForward` line, and the server's `RECV CAST_RESULT failure` must follow without repainting.
//! Every bar phase change is logged ([`bar_timeline`]) so the hold, burst and fade are measured
//! from timestamps. Non-combat: a failed cancel only hearths the character home. The switches are
//! `docs/CONTRIBUTING.md`, "Running it unattended".

use bevy::prelude::*;

use super::probes::ProbeClock;
use crate::net::SelfPlayer;

pub(crate) struct ProbeCastCancelPlugin;

impl Plugin for ProbeCastCancelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ProbeCastCancel>()
            .add_systems(Update, (cast_cancel_probe, bar_timeline));
    }
}

/// The bar's phase, read from the stock `CastingBarFrame`'s state fields.
const BAR_PHASE_CHUNK: &str = "return (function()\n\
    local f = CastingBarFrame\n\
    if not f then return \"noui\" end\n\
    if not f:IsVisible() then return \"hidden\" end\n\
    local txt = CastingBarText:GetText() or \"?\"\n\
    if f.casting then return \"casting|\" .. txt end\n\
    if f.channeling then return \"channel|\" .. txt end\n\
    if GetTime() < (f.holdTime or 0) then return \"hold|\" .. txt end\n\
    if f.flash then return \"burst|\" .. txt end\n\
    if f.fadeOut then return \"fade|\" .. txt end\n\
    return \"shown|\" .. txt\n\
 end)()";

/// Logs each cast-bar phase change with a timestamp. Stock `CastingBarFrame.lua` holds
/// `CASTING_BAR_HOLD_TIME` (1 s), then steps the flash by 0.2 and the fade by 0.05 once per
/// `OnUpdate` (`CastingBarFrame.lua:132-147`), so burst and fade last 5 and 20 frames.
fn bar_timeline(
    time: ProbeClock,
    script: Option<NonSend<benilla_ui::script::UiScript>>,
    self_player: Query<(), With<SelfPlayer>>,
    mut last: Local<crate::ui_script::VmMemo<String>>,
) {
    if self_player.is_empty() {
        return;
    }
    let Some(script) = script else {
        return;
    };
    // Session-keyed: a phase string left from the last login would swallow the first transition.
    let last = last.get(&script);
    let Ok(state) = script.eval::<String>(BAR_PHASE_CHUNK) else {
        return;
    };
    if state != *last {
        info!(
            "castcancel probe: bar {:.3}s {}",
            time.elapsed_secs(),
            state
        );
        *last = state;
    }
}

/// Probe state: when we entered the world, and the next phase to run.
#[derive(Resource, Default)]
struct ProbeCastCancel {
    entered: Option<f32>,
    phase: u8,
    used_at: Option<f32>,
}

/// Finds the Hearthstone in the backpack by item link and uses it; `false` while the item query
/// is still in flight, so the caller retries each frame.
const USE_HEARTH_CHUNK: &str = "return (function()\n\
    for s = 1, GetContainerNumSlots(0) do\n\
      local link = GetContainerItemLink(0, s)\n\
      if link and string.find(link, \"item:6948\", 1, true) then\n\
        UseContainerItem(0, s)\n\
        return true\n\
      end\n\
    end\n\
    return false\n\
 end)()";

/// From 3 s after world entry, use the Hearthstone (deadline 10 s), press `W` 2 s into the cast
/// and release it 0.3 s later; every phase logs.
fn cast_cancel_probe(
    time: ProbeClock,
    mut probe: ResMut<ProbeCastCancel>,
    self_player: Query<(), With<SelfPlayer>>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
) {
    if self_player.is_empty() {
        return;
    }
    let entered = *probe.entered.get_or_insert(time.elapsed_secs());
    let t = time.elapsed_secs() - entered;
    match probe.phase {
        0 if t >= 3.0 => {
            let Some(script) = script else {
                probe.phase = 99;
                error!("castcancel probe: no UI VM — cannot use the Hearthstone");
                return;
            };
            match script.eval::<bool>(USE_HEARTH_CHUNK) {
                Ok(true) => {
                    probe.phase = 1;
                    probe.used_at = Some(t);
                    info!("castcancel probe: using the Hearthstone (10 s cast opens the bar)");
                }
                Ok(false) if t >= 10.0 => {
                    probe.phase = 99;
                    error!("castcancel probe: no Hearthstone link resolved by t+10 s");
                }
                Ok(false) => {} // still resolving, retry next frame
                Err(e) => {
                    probe.phase = 99;
                    error!("castcancel probe: {e}");
                }
            }
        }
        1 if t >= probe.used_at.unwrap_or(f32::MAX) + 2.0 => {
            probe.phase = 2;
            info!("castcancel probe: pressing W mid-cast — the local cancel should fire NOW");
            keys.press(KeyCode::KeyW);
        }
        2 if t >= probe.used_at.unwrap_or(f32::MAX) + 2.3 => {
            probe.phase = 3;
            info!("castcancel probe: releasing W — read the cast-trace for the verdict");
            keys.release(KeyCode::KeyW);
        }
        _ => {}
    }
}
