//! The placed-doodad animation-event scanner, the third producer of [`AnimSoundEvent`] beside
//! [`crate::creature_anim`] and [`crate::go_anim`]: a lamp's hum or a campfire's crackle is a
//! `$DSL`/`$DSO`/`$SND` key on the doodad's own idle sequence.
//!
//! The reference arms every placed doodad (`0x695100`) but scans an event track (`0x719370`) only
//! from the per-frame M2 worklist `[CM2Scene+0x20]` (`0x7074b0`), refilled each frame through
//! `0x710b90` by the terrain doodad drain (`0x683f80`) with what the scene-walk cull (`0x683700`)
//! passed: frustum, horizon occlusion, and a distance fade above zero. A doodad outside that set
//! fires nothing and holds none of the emitter pool's 32 entries; scanning every resident doodad
//! would let far, inaudible ones take the pool's channels by claim order. A bind-posed model with
//! a sound marker gets a clock-only host (`doodad_anim::spawn_anim_host`).
//!
//! A registration is released only by `$DSE`, a `$DSL` naming another id, or the doodad's
//! teardown (`0x6951e0`, `0x6a0840`), so a campfire behind the camera keeps sounding.
//!
//! The clock is [`DoodadAnimHost::arm_clock`]'s, which the gate's resume seeks the player to.
//!
//! The scan window (`0x71950f`) is one frame wide off the absolute scene clock, which `0x7074c2`
//! advances whether or not the model is linked, with no persisted cursor: markers crossed while
//! culled are lost, and on re-link the watchdog re-arms at offset 0 (`0x6951b0`). So a parked
//! host's track memory is dropped and its resume is an arm frame.

use bevy::prelude::*;

use benilla_world::doodad_anim::DoodadAnimHost;
use benilla_world::schedule::WorldStage;

use crate::creature_anim::{advance_track, scan_events, AnimSoundEvent, TrackMemory};
use benilla_assets::ModelAnimations;

/// Per host: its clock and the frame its fired keys resolve in.
type ScannedDoodad = (
    Entity,
    &'static DoodadAnimHost,
    &'static ModelAnimations,
    &'static GlobalTransform,
    Option<&'static benilla_world::rig_anim::RigPose>,
);

/// Fires the event keys each placed doodad's armed clip crossed this frame. A variation re-roll
/// changes the node, so [`advance_track`] sees a fresh arm and never scans across it.
fn fire_doodad_anim_events(
    time: Res<Time>,
    hosts: Query<ScannedDoodad>,
    globals: Query<&GlobalTransform>,
    mut last: Local<TrackMemory>,
    mut out: MessageWriter<AnimSoundEvent>,
) {
    let now = time.elapsed_secs();
    for (entity, host, anims, world, pose) in &hosts {
        // The worklist gate: `active` is `gate_doodad_anim`'s animate-set verdict, written in
        // `PostUpdate`, so it is one frame old here.
        if !host.active {
            // A resume re-arms rather than replaying what the park skipped.
            last.remove(&entity);
            continue;
        }
        let Some((node, cur)) = host.arm_clock(now) else {
            continue; // a gseq-only host with no sound arm
        };
        let Some(clip) = anims.clips.iter().find(|c| c.node == node) else {
            continue;
        };
        if let Some(prev) = advance_track(&mut last, entity, node, cur) {
            // A clock-only host has no rig, so keys resolve at the placement; no `$DSL` record
            // rides an animated bone.
            let frame = crate::creature_anim::EventFrame {
                world,
                rig: pose.and_then(|p| Some((p, globals.get(p.joints_root).ok()?))),
            };
            scan_events(clip, entity, prev, cur, &frame, &mut out);
        }
    }
    // Reap despawned hosts: placements stream in and out by the tens of thousands.
    last.retain(|e, _| hosts.contains(*e));
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Update, fire_doodad_anim_events.in_set(WorldStage::Present));
}
