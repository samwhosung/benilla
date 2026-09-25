//! The engine-drawn **bowstring**: bow M2s carry NO string geometry — the real client registers a
//! bow-only per-frame draw callback (`0x611ff0`) that spans the bow's `$WTT`/`$WTB` limb-tip event
//! markers with a **2-segment line list**, middle vertex at the character's HandArrow attach while
//! the nock latch (`[+0xd58] & 0x4000`) holds, else the tip midpoint — so the string tracks the
//! draw hand while nocked and relaxes to a straight chord at rest.
//!
//! Benilla's transcription draws the two segments through Bevy's gizmo lines — the same
//! immediate-mode screen-space-width primitive class as the client's GX lines. Named
//! deviation (decision record): the string color/width are inferred (the callback emits a
//! packed vertex color that is not decoded — a dark cord is used).
//!
//! **The tips are POSED**. The bow prop animates — `$BWP` arms BowPull(160) on the
//! prop's own model and `$BWR` returns it to Stand(0) — so the `$WTT`/`$WTB` markers ride limb
//! bones that bend through the draw. Composing them through the prop's rigid root frame instead
//! (which is all this file could do while the prop rested at bind pose) leaves the chord pinned to
//! the un-bent limb tips while the mesh around it moves: the string would visibly leave the bow.

use bevy::prelude::*;

use crate::creature_anim::NockLatch;
use crate::entities::BoneAttach;

/// The HandArrow attach id (35 — `0x6121b8`): the string's middle control point while nocked,
/// the same point the nocked-arrow model rides.
const HAND_ARROW: u16 = 0x23;

/// Marks a spawned **bow prop root** (the held-item child of the hand joint) whose model authors
/// the `$WTT`/`$WTB` string anchors. Inserted by the held-item attach; despawned with the prop
/// (a sheath change / weapon swap tears the prop down, which is the client's clear too).
#[derive(Component)]
pub(crate) struct Bowstring {
    /// The unit wearing the bow — the HandArrow middle vertex and nock latch live on it.
    pub(crate) owner: Entity,
    /// The `$WTT` top / `$WTB` bottom anchors as `(bone, model-local Bevy offset)` — the bone is
    /// load-bearing on an animated prop (see the module note): the limb tips move with the draw.
    pub(crate) top: (u16, Vec3),
    pub(crate) bottom: (u16, Vec3),
}

/// Draw every visible bow's string (per frame, post-propagation so the prop/joint frames are
/// this frame's). Two segments: tip → middle → tip.
fn draw_bowstrings(
    bows: Query<(
        &Bowstring,
        &GlobalTransform,
        &InheritedVisibility,
        Option<&benilla_world::rig_anim::RigPose>,
    )>,
    owners: Query<(
        &BoneAttach,
        &benilla_world::rig_anim::RigPose,
        Has<NockLatch>,
    )>,
    joints: Query<&GlobalTransform>,
    mut gizmos: Gizmos,
) {
    // A dark waxed-cord tone, inferred: the ref's packed vertex color is not decoded.
    const STRING_COLOR: Color = Color::srgb(0.12, 0.10, 0.08);
    for (bs, prop, vis, flex) in &bows {
        if !vis.get() {
            continue;
        }
        // The prop's own pose when it flexes (`joints_root` IS this entity, so its
        // `GlobalTransform` is the frame `posed_point` wants); its rigid frame otherwise, which is
        // the same answer for a bow standing at bind pose.
        let tip = |(bone, offset): (u16, Vec3)| {
            flex.and_then(|p| p.posed_point(prop, bone, offset))
                .unwrap_or_else(|| prop.transform_point(offset))
        };
        let top = tip(bs.top);
        let bottom = tip(bs.bottom);
        // The middle control point: the owner's HandArrow attach world position while the nock
        // latch holds (the drawn string follows the hand), else the relaxed straight chord.
        let middle = owners
            .get(bs.owner)
            .ok()
            .filter(|(_, _, latched)| *latched)
            .and_then(|(bones, pose, _)| {
                // A pure position read — `posed_point` off the composed pose, no anchor entity.
                let &(bone, offset) = bones.points.get(&HAND_ARROW)?;
                pose.posed_point(joints.get(pose.joints_root).ok()?, bone, offset)
            })
            .unwrap_or_else(|| (top + bottom) / 2.0);
        gizmos.line(top, middle, STRING_COLOR);
        gizmos.line(middle, bottom, STRING_COLOR);
    }
}

/// Registers the string drawer after the palette pass (the middle vertex reads the HandArrow
/// joint's frame — its translation is palette-invariant, but same-frame ordering keeps every
/// consumer of joint frames on one side of the rewrite).
pub(crate) struct BowstringPlugin;

impl Plugin for BowstringPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            draw_bowstrings.in_set(benilla_world::billboard::BillboardPlace),
        );
    }
}
