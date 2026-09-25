//! **Riding a transport** — the boat/zeppelin/tram platform frame (decision 0438 phase 2, widened
//! by 0470's solid cargo). Three acts, in the order the controller runs them:
//!
//! 1. [`carry`] — *before* any input integrates, recompose the rider from the deck's THIS-frame
//!    pose. The deck's motion carries a standing player and its yaw delta turns the whole rider
//!    assembly with it.
//! 2. [`update_attachment`] — *after* the mover has stepped, decide whether we are still aboard,
//!    and re-snapshot the deck-local pose the next frame's [`carry`] recomposes from.
//! 3. The `ride` **trace** tag, emitted inside (2) because it needs the same three reads. The
//!    deck's own motion lives in the boat's transform and the rider's in world space; the
//!    difference between them is the only thing that says whether the carry composed, and no other
//!    instrument records it.

use bevy::prelude::*;

use super::{wrap_pi, FlyCam, Player, PlayerRide, TransportQuery};

/// Recompose the rider from the deck's this-frame pose, before any input integrates.
pub(super) fn carry(player: &mut Player, cam: &mut FlyCam, transports: &TransportQuery) {
    // The platform carry (decision 0438 phase 2): while attached to a transport, recompose the
    // feet from the boat's THIS-frame pose (the transport tick runs on the Net→Input edge, so it's
    // fresh) before any input integrates — the deck's motion carries the standing player, and its
    // per-frame yaw delta turns them with it (applied incrementally so it composes with whatever
    // mouse-look already wrote to `face_yaw` this frame). A despawned boat (streamed out) detaches
    // into an ordinary fall from the last world pose.
    //
    // The carry is rigid for the WHOLE rider — aim (`face_yaw`), rendered body (`model_yaw`), and
    // camera (`cam.yaw`) take the same delta, all HERE. Carrying only the aim leaves the standing
    // body-chase to close the gap frame after frame, and that chase-step is exactly what latches
    // the turn-in-place foot-shuffle (whose keyframes fire step sounds): a sailing boat's spline
    // yaw drifts continuously, so the rider shuffled and clacked the whole voyage (director,
    // 2026-07-17). The deck turning under you is not you turning — the chase and its shuffle only
    // see input turns.
    //
    // The camera's share is unconditional — a deck turn is FRAME motion, not an input turn, so it
    // never routes through `seat_camera`'s look-session gate (that gate protects the camera from
    // *keyboard* turns while a drag owns it). Routing it there was the right-drag drift bug
    // (director, 2026-07-18): during a look session the gate ate the camera's share, and the
    // right-drag coupling `face_yaw = cam.yaw` (which runs first next frame) then yanked the aim
    // back to the world-fixed camera — undoing the deck carry, so with the mouse still the scene
    // swung across the screen and the rider visibly spun against the deck. Carrying all three here
    // keeps the drag's orbit offset (`cam.yaw − face_yaw`) exactly as the hand left it while the
    // whole rider assembly turns with the boat — the reference's camera rides the transport-local
    // player rig the same way.
    if let Some(ride) = player.ride.as_ref() {
        match transports.get(ride.entity) {
            Ok((boat, _, _)) => {
                let world = boat.translation + boat.rotation * ride.local_pos;
                let yaw_now = boat.rotation.to_euler(EulerRot::YXZ).0;
                let mut dyaw = yaw_now - ride.boat_yaw;
                // `to_euler` wraps to (−π, π]; a boat crossing that seam reads as a ±2π hop.
                dyaw = (dyaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI;
                player.pos = world;
                player.face_yaw += dyaw;
                player.model_yaw = wrap_pi(player.model_yaw + dyaw);
                cam.yaw += dyaw;
                if let Some(r) = player.ride.as_mut() {
                    r.boat_yaw = yaw_now;
                }
            }
            Err(_) => player.ride = None,
        }
    }
}

/// Attach/detach against this frame's walkable support, then re-snapshot the deck-local pose.
/// `ground` is the mover's resolved support entity ([`super::mover::Outcome::ground`]).
pub(super) fn update_attachment(
    player: &mut Player,
    transports: &TransportQuery,
    child_of: &Query<&ChildOf>,
    ground: Option<Entity>,
    grounded: bool,
    swimming: bool,
) {
    // Transport attach/detach (decision 0438 phase 2). Attach when the walkable support is a
    // transport's collider — the boat's own hull, OR a deck prop's collider child (solid
    // cargo): the walk resolves the support upward through the parent chain to the
    // Transport that owns it, so standing on a crate is standing on the boat. Detach when
    // support resolves to world geometry or we enter the water. Airborne keeps the current
    // attachment — [`carry`] keeps composing, so a jump above the deck is deck-frame
    // ballistics and lands where it took off (jumping off the side detaches at whatever it
    // lands on). Then re-snapshot the local pose from this frame's FINAL world pose against
    // the boat's (unchanged-this-frame) transform, which is what next frame's carry
    // recomposes from.
    let owning_transport = |mut e: Entity| {
        for _ in 0..4 {
            if let Ok((t, g, _)) = transports.get(e) {
                return Some((e, t, g));
            }
            e = child_of.get(e).ok()?.parent();
        }
        None
    };
    if swimming {
        if player.ride.take().is_some() {
            info!("transport: deboard (entered the water)");
        }
    } else if grounded {
        match ground.and_then(owning_transport) {
            Some((entity, _, guid)) => {
                if player.ride.as_ref().map(|r| r.entity) != Some(entity) {
                    info!("transport: board {:#x} (support is its deck)", guid.0);
                }
                player.ride = Some(PlayerRide {
                    entity,
                    guid: guid.0,
                    local_pos: Vec3::ZERO, // filled by the snapshot just below
                    boat_yaw: 0.0,
                });
            }
            None => {
                if player.ride.take().is_some() {
                    info!("transport: deboard (support is world geometry)");
                }
            }
        }
    }
    let feet = player.pos;
    // **The ride trace** (`WOW_MOVE_TRACE_TAGS=ride`) — one line per frame while attached, plus
    // the frame after a detach, because "what happened on the boat" is otherwise unanswerable:
    // the deck's own motion is in the boat's transform, the rider's in world space, and the
    // difference between them is the only thing that says whether the carry composed. The
    // director's report — *stepped off a ledge on a boat and it threw me back across the boat
    // until I landed* — is a statement about the DECK-relative path, which no other instrument
    // here records.
    if benilla_assets::trace::enabled_for("ride") {
        let boat_pose = player
            .ride
            .as_ref()
            .and_then(|r| transports.get(r.entity).ok())
            .map(|(t, _, aabb)| {
                (
                    t.translation,
                    t.rotation.to_euler(EulerRot::YXZ).0,
                    aabb.copied(),
                )
            });
        if let (Some(ride), Some((bpos, byaw, baabb))) = (player.ride.as_ref(), boat_pose) {
            let local = Quat::from_euler(EulerRot::YXZ, byaw, 0.0, 0.0)
                .inverse()
                .mul_vec3(feet - bpos);
            // The deck's **broad-phase box**, and the gap from its underside to the feet. The
            // candidate enumeration every mover probe rides
            // (`SpatialQuery::aabb_intersections_with_aabb`) tests exactly this component —
            // and avian refreshes it in `PhysicsSchedule` (`FixedPostUpdate`, *before*
            // `Update`), while `tick_transports` writes the deck's pose *in* `Update`. So the
            // box is always one frame of deck travel behind the deck, and on a long frame it
            // is left behind entirely: a positive `gap` means the down-probe cannot even see
            // the deck the feet are standing on, which is the difference between
            // `support=deck` and `support=NONE` while `local` still reads (0, 0.05, 0).
            let aabb_col = match baabb {
                Some(a) => format!(
                    " | aabbY[{:8.2},{:8.2}] gap{:+7.2}",
                    a.min.y,
                    a.max.y,
                    a.min.y - feet.y,
                ),
                None => " | aabbY[  absent]".to_string(),
            };
            benilla_assets::trace::line(
                "ride",
                &format!(
                    "on {:#x} deck({:8.2},{:7.2},{:8.2}) yaw{:+.3} | feet({:8.2},{:7.2},{:8.2})                          local({:7.2},{:6.2},{:7.2}) | grounded={} support={} vy={:+6.2}{}",
                    ride.guid,
                    bpos.x,
                    bpos.y,
                    bpos.z,
                    byaw,
                    feet.x,
                    feet.y,
                    feet.z,
                    local.x,
                    local.y,
                    local.z,
                    grounded as u8,
                    match ground.and_then(owning_transport) {
                        Some(_) => "deck",
                        None if ground.is_some() => "world",
                        None => "NONE",
                    },
                    player.vel_y,
                    aabb_col,
                ),
            );
        } else if !swimming {
            // Not riding: only worth a line when something under us *is* a transport, i.e. the
            // frames where an attach should have happened and did not.
            if let Some((_, _, guid)) = ground.and_then(owning_transport) {
                benilla_assets::trace::line(
                    "ride",
                    &format!(
                        "OFF but standing on {:#x} at ({:8.2},{:7.2},{:8.2}) grounded={}",
                        guid.0, feet.x, feet.y, feet.z, grounded as u8
                    ),
                );
            }
        }
    }
    if let Some(ride) = player.ride.as_mut() {
        if let Ok((boat, _, _)) = transports.get(ride.entity) {
            ride.local_pos = boat.compute_affine().inverse().transform_point3(feet);
            ride.boat_yaw = boat.rotation.to_euler(EulerRot::YXZ).0;
        }
    }
}
