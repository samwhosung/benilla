//! Writes the frame onto the driven body after the mover and [`super::gait`]: the transform and
//! `MovementState` any streamed unit carries, the landing report and the camera-pivot target.

use bevy::prelude::*;

use super::{model_pivot_height, wrap_pi, BodyQuery, CameraPivot, Player};

/// Writes this frame onto the driven body and returns its camera-pivot target height;
/// `anim_flags` is [`super::gait::drive_body_heading`]'s result.
pub(super) fn drive(
    player: &Player,
    body: &mut BodyQuery,
    hard_landing: &mut MessageWriter<crate::creature_anim::HardLanding>,
    swimming: bool,
    swim_pitch: f32,
    move_flags_now: u32,
    anim_flags: u32,
    landed: bool,
    stand_now: u8,
) -> Option<f32> {
    // Scale is left alone: the renderer bakes the display scale in.
    let mut cam_pivot_target = None;
    if let Ok((entity, mut t, motion, pivot, .., twist, _, net_entity)) = body.single_mut() {
        t.translation = player.pos;
        // The swim pitch law every observed mover shares. `swim_pitch` is the aim, leveled along
        // the surface when the rest-line cap bites, and the wire tail streams the same value.
        t.rotation =
            crate::creature_anim::swim_body_rotation(player.model_yaw, move_flags_now, swim_pitch);
        // The landing predictor (`0x602d00`) plays a hard landing's grunt and dust this frame;
        // the server's `SMSG_ENVIRONMENTALDAMAGELOG` dust follows a round trip later, as in the
        // reference. `fall_start_y` still holds this arc's launch height.
        if landed {
            hard_landing.write(crate::creature_anim::HardLanding {
                entity,
                descent: player.fall_start_y - player.pos.y,
            });
        }
        // `0x50f880` picks the swim pivot preset off the camera target's own SWIMMING bit, read
        // from the pitch's word so the two agree on the frame the water starts.
        cam_pivot_target = pivot_target(
            pivot,
            net_entity,
            move_flags_now & crate::creature_anim::move_flags::SWIMMING != 0,
        );
        if let Some(mut motion) = motion {
            // A swimmer strokes at the flag-scalar speed whatever its pitch; ground gaits take the
            // achieved horizontal speed, already directional (run-back while backpedaling).
            motion.speed = if swimming {
                player.swim_stroke_speed
            } else {
                player.horiz_vel.length()
            };
            motion.vertical_speed = player.vel_y;
            motion.flags = anim_flags;
            motion.stand_state = stand_now;
        }
        // The counter-twist gap: the aim's offset from the rendered body.
        if let Some(mut twist) = twist {
            twist.yaw_gap =
                twist_gap_override().unwrap_or_else(|| wrap_pi(player.face_yaw - player.model_yaw));
        }
    }
    cam_pivot_target
}

/// The camera pivot's target height: the model-local [`CameraPivot`] times the body's raw
/// `OBJECT_FIELD_SCALE_X`, not the eased render scale, clamped. `swimming` picks the preset as
/// `0x50f880` does (`0x50f89e`, `cam+0x124`), lower by [`CameraPivot::swim_drop_local`];
/// [`super::camera::PivotGlide`] glides between presets. `None` until the model attaches: the
/// reference skips the camera update while the model is unresolved (`0x50e907`), so a display
/// swap holds, then glides once.
pub(super) fn pivot_target(
    pivot: Option<&CameraPivot>,
    net: Option<&crate::net::NetEntity>,
    swimming: bool,
) -> Option<f32> {
    pivot.map(|p| model_pivot_height(p, net.map_or(1.0, |n| n.scale), swimming))
}

/// `WOW_TWIST_GAP=<radians>` pins the counter-twist's yaw gap; the env is read once.
fn twist_gap_override() -> Option<f32> {
    static G: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *G.get_or_init(|| {
        std::env::var("WOW_TWIST_GAP")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
    })
}
