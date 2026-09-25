//! The **placed-doodad** animation-event scanner — the third and last producer feeding the
//! [`AnimSoundEvent`] stream, beside the creature scanner ([`crate::creature_anim`]) and the
//! GameObject one ([`crate::go_anim`]).
//!
//! Bug B345 ("every world-placed M2 doodad is silent") was this hole: a lamp's hum, a campfire's
//! crackle, a waterfall, a windmill and a gnome machine are all authored as `$DSL`/`$DSO`/`$SND`
//! keys on the model's own idle sequence — and **nothing in benilla ever wrote a doodad's events
//! into the stream**. The routing end ([`crate::sound::anim_events`]) has handled those tags since
//! decision 0070 slice 3; it simply never received one from a doodad, because the only two
//! scanners gate on `AnimDriver` (units) and `GoAnim` (server-sent GameObjects), neither of which a
//! terrain-stream placement ever gets.
//!
//! **Why it is not enough to scan the hosts that already exist.** The corpus says 273 models author
//! a doodad sound marker (`benilla-extract soundeventscan`); only 165 of them are the `FirstSeq`
//! tier that carries an `AnimationPlayer`. The other 108 — including the reported lamp,
//! `KalidarStreetLamp01.m2`, one looping 3.333 s Stand that keys no bone at all and carries exactly
//! one `$DSL` → `NightElfStreetLampLoop` — are bind-posed, and decision 0130's content gate
//! deliberately builds them no rig. That gate is right about pixels and was never a claim about the
//! clock: the reference arms **every** placed doodad it creates and cycles it whenever the doodad is
//! in the frame's animate set (`0x695100` — see the gate below). So
//! `doodad_anim::spawn_anim_host` gives a sound-carrying model a clock-only host —
//! the arm bookkeeping, no player — and this scanner reads that arm.
//!
//! **The scan is gated on the frame's ANIMATE SET, and that gate is the whole mechanism** — not a
//! budget, and not an optimisation. The reference's event scan `0x719370` is
//! reached from exactly one place: `0x7074b0`'s walk of the per-frame M2 scene worklist
//! `[CM2Scene+0x20]`, which is **emptied every frame** and refilled only by `0x710b90(model, 1)`
//! from the terrain doodad drain `0x683f80` — and the drain walks only the bucket the scene-walk
//! cull `0x683700` appended to, i.e. a doodad that passed frustum + horizon-occlusion **and** came
//! out of the radius-tiered horizontal distance fade with `alpha > 0`. A doodad that fails any of
//! those is not in the worklist, so its event track is never scanned, so **its `$DSL` never
//! fires and it never takes one of the emitter pool's 32 entries**.
//!
//! Ungated, this scanner registered **every resident doodad in the streaming radius**, and with the
//! pool's four channels arbitrated by claim order (`super::sound::emitter_pool`) that is not a
//! harmless surplus — it is silence. Measured at the director's Stratholme pin
//! (`.go xyz 3471.91 -3366.89 136.85 329`, 2026-09-07): all four channels held by
//! `UndeadCampfireSmall` / `CauldronLoop` / `TorchLoop` / `SlimeWaterfall`, **every one of them
//! past its own `DistanceCutoff` and therefore inaudible**, while the fifth entitled entry —
//! `StratholmeFireSmokeEmberSLoop`, the burning-building layer the director reported missing, with
//! 88 emitters right there — was withheld by the cap. `0 sounding`, on the same frame that reported
//! `parked=1004`: a thousand hosts outside the animate set, each still holding its registration.
//!
//! **"A campfire behind you keeps crackling" is still true, and this gate does not break it** — but
//! by the other mechanism. A registration is released by `$DSE`, by a `$DSL` naming a different id,
//! or by the doodad's own teardown, and by nothing else (the handler `0x6951e0`, the teardown
//! `0x6a0840`): turning away stops the *marker firing*, never the entry. The prose this module and
//! [`DoodadAnimHost::arm_clock`] used to carry — "the reference gates the cycle on residency, not
//! the draw" — read *linkage* as "loaded" when it means spliced into `[scene+0x20]`, which **is**
//! the drawn/faded set: a doodad culled out of the drain stops advancing and resumes on re-link.
//!
//! The clock stays [`DoodadAnimHost::arm_clock`]'s shared one rather than the `AnimationPlayer`'s:
//! while a host is in the animate set the two agree by construction (the gate's resume seeks the
//! player to exactly this value), and the clock-only tier has no player to read at all.
//!
//! **Across a park the memory is DROPPED, not carried** — because the reference's scan window is
//! one frame wide and it never replays. `0x719370` computes `prev = max(windowLo, now − dt)` and
//! `cur = now` (`0x71950f`, for a looping sequence), **both off the absolute scene clock**, which
//! `0x7074c2` advances every frame whether or not the model is linked; a write census over the
//! whole function finds no persisted cursor. So the markers a culled doodad crossed are **lost**,
//! and what actually fires on re-link is the completion watchdog re-arming at `startOffset = 0`
//! (`0x6951b0`, as decision 2065 records). Keeping [`advance_track`]'s entry across the park
//! would hand the resume frame a `prev` from seconds ago and fire the whole span at once —
//! harmless for a `$DSL` (its re-reach only repositions) and a burst of one-shots for
//! `$DSO`/`$SND`. Dropping it makes the resume an **arm frame**, which fires nothing, and the
//! frame after opens the head window `[0, cur]` — the re-arm, in the shape this scanner already
//! has.

use bevy::prelude::*;

use benilla_world::doodad_anim::DoodadAnimHost;
use benilla_world::schedule::WorldStage;

use crate::creature_anim::{advance_track, scan_events, AnimSoundEvent, TrackMemory};
use benilla_assets::ModelAnimations;

/// What the placed-doodad event scanner reads per host: its clock and the frame its fired keys
/// resolve in ([`crate::creature_anim::EventFrame`]).
type ScannedDoodad = (
    Entity,
    &'static DoodadAnimHost,
    &'static ModelAnimations,
    &'static GlobalTransform,
    Option<&'static benilla_world::rig_anim::RigPose>,
);

/// Fire the event keyframes each placed doodad's armed clip crossed this frame.
///
/// The arming rules are the shared [`advance_track`]/[`scan_events`] ones the other two scanners
/// use — an arm frame fires nothing, the frame after it opens the clip's `t = 0` head (which is
/// where essentially every doodad sound key sits), and a loop wrap fires tail-then-head. A
/// variation re-roll ([`benilla_world::doodad_anim`]'s self-sustaining re-arm) changes the node, so
/// [`advance_track`] sees a fresh arm and never scans across the seam.
fn fire_doodad_anim_events(
    time: Res<Time>,
    hosts: Query<ScannedDoodad>,
    globals: Query<&GlobalTransform>,
    mut last: Local<TrackMemory>,
    mut out: MessageWriter<AnimSoundEvent>,
) {
    let now = time.elapsed_secs();
    for (entity, host, anims, world, pose) in &hosts {
        // The worklist gate (module docs): a host outside this frame's animate set is not in
        // `[CM2Scene+0x20]`, so the reference never scans its event track. `active` is benilla's
        // one spelling of that membership — `gate_doodad_anim` writes it from the composed
        // far-clip + distance-fade + portal verdict on the placement's submeshes, or, for a
        // meshless (particles-only) model, from the same fade-sphere + frustum law its emitters
        // gate on. It is a `PostUpdate` write and this reads it in `Update`, so the verdict is
        // one frame old — the same accepted lag the interior and billboard laws carry, and
        // nothing here can tell a 16 ms-late ambience from an on-time one.
        if !host.active {
            // …and forget where its clip stood. See the module docs: the reference's window is one
            // frame wide off a clock that never stops, so a resume must re-arm rather than replay
            // everything the park skipped.
            last.remove(&entity);
            continue;
        }
        let Some((node, cur)) = host.arm_clock(now) else {
            continue; // gseq-only host with no sound arm: nothing is armed, so nothing fires
        };
        let Some(clip) = anims.clips.iter().find(|c| c.node == node) else {
            continue;
        };
        if let Some(prev) = advance_track(&mut last, entity, node, cur) {
            // The clock-only tier carries no rig at all, which is not a shortfall here: with no
            // bone matrices the kernel's product is the placement alone, and the corpus says no
            // `$DSL` record rides an animated bone. The lamp's hum lands on the lamp.
            let frame = crate::creature_anim::EventFrame {
                world,
                rig: pose.and_then(|p| Some((p, globals.get(p.joints_root).ok()?))),
            };
            scan_events(clip, entity, prev, cur, &frame, &mut out);
        }
    }
    // Drop memory for hosts that no longer exist. The other two scanners leave their `Local` to
    // grow — a unit or GameObject population churns with the session — but placements do not:
    // crossing a continent streams tens of thousands of them in and out, and an unreaped entry per
    // placement is an unbounded leak on the one lane where it actually bites. `Query::contains` is
    // a lookup, so this is one pass over a map sized by the live sound-host count. It lives here
    // rather than in its own system because a `Local` belongs to exactly one system.
    last.retain(|e, _| hosts.contains(*e));
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Update, fire_doodad_anim_events.in_set(WorldStage::Present));
}
