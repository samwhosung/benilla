//! The wound flinch: a decaying blend in the struck bone's secondary slot (`0x60ea70`).

use benilla_assets::ModelAnimations;
use benilla_formats::AnimDataCatalog;
use bevy::prelude::*;

use super::super::{find_resolved, AnimDriver, MovementState, Wound};
use super::select::{self, STAND};

/// A wound trigger into `0x60ea70`: a melee hit, severity its crit bit, or a spell, severity 0.
#[derive(Clone, Copy)]
pub(super) enum WoundEdge {
    Melee(u32),
    Spell,
}

/// Decays the wound every frame, above the death override: the kernel advances every armed
/// secondary slot unconditionally, and the slot self-releases at λ = 0 (`0x7147b9`).
pub(super) fn wound_upkeep(entity: Entity, drv: &mut AnimDriver, player: &mut AnimationPlayer) {
    if let Some(wd) = drv.wound {
        let finished = match player.animation_mut(wd.node) {
            Some(a) if !a.is_finished() => {
                let remaining = 1.0 - a.seek_time() / wd.span;
                // The base, plus a live one-shot overlay when masked; a fading clip is not counted.
                let others = if wd.masked && drv.overlay.is_some() {
                    1.0 + super::ONESHOT_OVERLAY_WEIGHT
                } else {
                    1.0
                };
                a.set_weight(select::wound_weight(remaining, others));
                false
            }
            _ => true,
        };
        if finished {
            player.stop(wd.node);
            drv.wound = None;
            if benilla_assets::trace::enabled() {
                benilla_assets::trace::line(
                    "fct",
                    &format!("wound expire unit={entity} (λ reached 0, slot released)"),
                );
            }
        }
    }
}

/// Evicts the wound on a blended primary re-arm of its bone: op4 with `blendFlag≠0` copies the
/// outgoing pose over the secondary slot (`+0xc4..`). The other bone's plays leave it decaying
/// (`0x714260`); a mode or gait change counts as a bone-0 play.
pub(super) fn wound_evict(
    entity: Entity,
    drv: &mut AnimDriver,
    player: &mut AnimationPlayer,
    masked_played: bool,
    base_played: bool,
) {
    if let Some(wd) = drv.wound {
        let evicted = if wd.masked {
            masked_played
        } else {
            base_played
        };
        if evicted {
            player.stop(wd.node);
            drv.wound = None;
            if benilla_assets::trace::enabled() {
                benilla_assets::trace::line(
                    "fct",
                    &format!(
                        "wound evict unit={entity} masked={} (a blended re-arm took the bone's secondary)",
                        wd.masked
                    ),
                );
            }
        }
    }
}

/// Lays wound clip `id` (8–10) into the secondary slot as a decaying 0.75-amplitude blend over
/// whatever plays (`0x60ea70`, op4 `linkFlag=0`), with no mid-swing gate: the base track and the
/// one-shot slot run on underneath. It calls op4, not `PlayAnimation`, so the sheath reconcile and
/// event scan never see it; it is the frame's last write, as in the packet order.
/// The caller holds the entry gates; the reference's attached-spell-effect gate is not built.
pub(super) fn wound_trigger(
    entity: Entity,
    drv: &mut AnimDriver,
    player: &mut AnimationPlayer,
    anims: &ModelAnimations,
    catalog: Option<&AnimDataCatalog>,
    rng: &mut benilla_assets::AnimRng,
    id: u16,
    mv: &MovementState,
    mounted: bool,
) {
    // `0x605f90`'s third IsDead clause, stand state 7; the dead branch took the other two.
    if mv.stand_state != 7 {
        let base = drv.resolved_anim(anims, catalog).unwrap_or(STAND);
        let full = select::wound_full_body(id, base, mv.flags, mounted);
        // Rolls its variation (op4's `variationIdx −1`). A zero span expires on arrival
        // (`end = clock`) and no clip at all is the `0x711a20` asset-presence abort: both skip.
        let clip = find_resolved(anims, id, catalog)
            .and_then(|h| anims.pick_variation(h.anim_id, rng.draw()))
            .filter(|c| c.duration > 0.0);
        let node = clip.and_then(|c| {
            if full {
                Some((c, c.node, false))
            } else {
                // No split bone: the full-body node, still a blend, never a replace.
                c.upper_node
                    .map(|n| (c, n, true))
                    .or(Some((c, c.node, false)))
            }
        });
        if let Some((c, node, masked)) = node {
            // A re-trigger re-seeds the slot: the client overwrites the secondary.
            if let Some(prev) = drv.wound.take() {
                player.stop(prev.node);
            }
            // Still active after that stop: the base track owns the node, so nothing to layer.
            if player.animation(node).is_none() {
                let others = if masked && drv.overlay.is_some() {
                    1.0 + super::ONESHOT_OVERLAY_WEIGHT
                } else {
                    1.0
                };
                let active = player.play(node);
                active.replay();
                // One pass: the decay window is one span (`+0x100 = clock + span`), whatever R.
                active.set_repeat(bevy::animation::RepeatAnimation::Never);
                active.set_weight(select::wound_weight(1.0, others));
                drv.wound = Some(Wound {
                    node,
                    span: c.duration,
                    masked,
                });
                if benilla_assets::trace::enabled() {
                    benilla_assets::trace::line(
                        "fct",
                        &format!(
                            "wound trigger unit={entity} id={id} masked={masked} span={:.3} base={base} others={others}",
                            c.duration
                        ),
                    );
                }
            } else if benilla_assets::trace::enabled() {
                benilla_assets::trace::line(
                    "fct",
                    &format!(
                        "wound trigger unit={entity} id={id} SKIPPED (the base track owns the node)"
                    ),
                );
            }
        } else if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "fct",
                &format!(
                    "wound trigger unit={entity} id={id} SKIPPED (no playable clip, or a zero span)"
                ),
            );
        }
    }
}
