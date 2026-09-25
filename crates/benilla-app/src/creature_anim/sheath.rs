//! Weapon sheath state: the one setter's request, the draw/stow ceremony overlays and the
//! `AnimationData.dbc` weapon flags; `drive_animations` executes every transition.

use benilla_assets::ModelAnimations;
use benilla_formats::AnimDataCatalog;
use bevy::prelude::*;

use benilla_assets::{LockRecover, WorldAssets};

use super::{find_resolved, select, AnimDriver, Wielded};

/// A unit's visual sheath state per arm while a draw/stow ceremony plays: each weapon moves at its
/// own clip's `$SHL`/`$SHR` key, not when the state changes. Absent, the committed state rules.
#[derive(Component, Clone, Copy)]
pub(crate) struct VisualSheath(pub(crate) [u8; 2]);

impl VisualSheath {
    /// The state placing one held slot: mainhand right, offhand left, ranged by [`ranged_arm`].
    pub(crate) fn for_slot(self, slot: usize, inv_type: u32) -> u8 {
        self.0[match slot {
            0 => ARM_RIGHT,
            1 => ARM_LEFT,
            _ if matches!(inv_type, 0x1a | 0x19) => ARM_RIGHT,
            _ => ARM_LEFT,
        }]
    }
}

/// A ceremony leg in flight: a masked one-shot on one arm over whatever the body does (`0x60b770`).
pub(super) struct SheathArm {
    node: bevy::animation::graph::AnimationNodeIndex,
    /// The authored `$SHL`/`$SHR` time, when this arm's weapon moves.
    swap_at: f32,
    /// The clip ends with the weapon in hand (the client's `+0xd58` phase bit); a stow is `false`.
    drawing: bool,
    /// The held slot this leg moves: 0 mainhand, 1 offhand, 2 ranged.
    slot: u8,
    crossed: bool,
}

/// The draw/stow ceremony in flight: up to one leg per arm, each advancing on its own.
pub(super) struct SheathSwap {
    arms: [Option<SheathArm>; 2],
    /// The state the ceremony left (the client's `+0xd3c` PREV); phase 2's gate.
    prev: u8,
}

impl SheathSwap {
    /// The state one arm shows: its leg's side of the swap, or `cur` with no leg.
    fn arm_state(&self, arm: usize, cur: u8) -> u8 {
        match &self.arms[arm] {
            None => cur,
            Some(a) if a.drawing => {
                if a.crossed {
                    cur
                } else {
                    0
                }
            }
            Some(a) => {
                if a.crossed {
                    0
                } else {
                    self.prev
                }
            }
        }
    }
}

/// The ceremony's weight over the gait on the arm bones, about 8:1, where the client's per-bone
/// arming gives it the arm outright.
const SHEATH_OVERLAY_WEIGHT: f32 = 8.0;

/// A sheath state change, the client's one setter `SetSheatheState(newState, bInstant,
/// bFireEvent)` (`0x611cf0`), which `drive_animations` executes. Of its 24 call sites only the
/// manual `ToggleSheath` passes `bInstant = 0`: every other change snaps.
#[derive(Message, Clone, Copy)]
pub(crate) struct SheathRequest {
    pub(crate) entity: Entity,
    /// The requested state: 0 stowed, 1 melee drawn, 2 ranged drawn.
    pub(crate) state: u8,
    /// Play the draw/stow ceremony (the manual toggle's `bInstant = 0`); everything else snaps.
    pub(crate) ceremony: bool,
}

/// The state a Z press asks for next, or `None` where the reference makes no call (`ToggleSheath`,
/// `0x5eb642`–`0x5eb6a8`, over the committed state `[unit+0xd40]`): melee, then ranged, then
/// stowed, each gated on what is worn (`GetWeapon(0/1/2)` at `0x5eb5f0`–`0x5eb610`).
/// Deviation: the class lookup that can also clear the ranged candidate (`0x5eb616`–`0x5eb640`,
/// `0xc0def4`, meaning inferred) is skipped, because a class failing it cannot equip one.
pub(crate) fn toggle_sheath_next(cur: u8, (melee, ranged): (bool, bool)) -> Option<u8> {
    match cur {
        0 if melee => Some(1),
        0 if ranged => Some(2),
        0 => None,
        1 if ranged => Some(2),
        1 | 2 => Some(0),
        _ => None, // no call for a state outside {0, 1, 2} (the `dec ecx; jne` tail)
    }
}

/// One arm's planned leg: the clip, and whether the arm ends with its weapon in hand (the
/// client's `+0xd58` phase bit, set by the drawers and cleared by the stows in `0x611b60`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct ArmLeg {
    pub(super) clip: u16,
    pub(super) drawing: bool,
}

/// Arm indices; the client plays the mainhand on sub-sequence 3, the offhand on 2.
pub(super) const ARM_RIGHT: usize = 0;
pub(super) const ARM_LEFT: usize = 1;

/// The arm a worn ranged weapon occupies: `InventoryType` 0x1a (gun, crossbow, wand) or 0x19
/// (thrown) the right, a bow (0x0f) the left (`0x611c74`, `0x6118db`, `0x611998`, `0x611a9a`).
fn ranged_arm(w: &Wielded) -> Option<usize> {
    w.ranged.map(|_| {
        if matches!(w.ranged_inv, 0x1a | 0x19) {
            ARM_RIGHT
        } else {
            ARM_LEFT
        }
    })
}

/// The draw leg one arm plays for a committed state, as the drawers `0x6118a0` (right),
/// `0x611960` (left) and `0x611a20` (both) pick it; `None` releases the arm to its idle.
fn draw_leg(arm: usize, cur: u8, w: &Wielded) -> Option<ArmLeg> {
    let (item, sheath) = match (cur, arm) {
        // The drawers read `GetWeapon(slot, 0)`: a disarmed hand has nothing to draw (`0x5eb480`).
        (1, ARM_RIGHT) => (w.armed_main(), w.main_sheath),
        (1, ARM_LEFT) => (w.armed_off(), w.off_sheath),
        // The ranged weapon draws on exactly one arm; the other has no leg at all.
        (2, _) if ranged_arm(w) == Some(arm) => (w.ranged, w.ranged_sheath),
        _ => return None,
    };
    item.map(|_| ArmLeg {
        clip: select::sheath_clip(sheath),
        drawing: true,
    })
}

/// Phase 2, the deferred draw: when a stow clip (89/90) finishes on sub-sequence 2 or 3 with the
/// arm's phase bit clear, the finish handler calls that arm's drawer (`0x5fc920` @
/// `0x5fca62`–`0x5fcab6`: `0x611960` @ `0x5fcaaf` left, `0x6118a0` @ `0x5fca9a` right). The
/// drawers refuse out of `prev == 0` (`0x6118a5`, `0x611965`), a draw phase 1 already played.
pub(super) fn sheath_phase2(arm: usize, prev: u8, cur: u8, w: &Wielded) -> Option<ArmLeg> {
    (prev != 0).then(|| draw_leg(arm, cur, w)).flatten()
}

/// Phase 1, what the setter plays by PREV (`0x611b60`–`0x611ce6`, the `bInstant == 0` fork): out
/// of stowed both arms draw at once (`0x611a20`); otherwise an arm holding something stows it and
/// defers its draw to [`sheath_phase2`], and a free arm draws now. A ranged weapon always stows
/// with a literal Sheath 89, never its own pick (`0x611c8c`, `0x611cd3`).
pub(super) fn sheath_phase1(prev: u8, cur: u8, w: &Wielded) -> [Option<ArmLeg>; 2] {
    const RANGED_STOW: ArmLeg = ArmLeg {
        clip: 89,
        drawing: false,
    };
    let stow = |sheath: u8| {
        Some(ArmLeg {
            clip: select::sheath_clip(sheath),
            drawing: false,
        })
    };
    match prev {
        // Nothing in hand to put away: both arms draw at once (`0x611ce6` to `0x611a20`).
        0 => [draw_leg(ARM_RIGHT, cur, w), draw_leg(ARM_LEFT, cur, w)],
        1 => [
            match w.armed_main() {
                Some(_) => stow(w.main_sheath),
                None => sheath_phase2(ARM_RIGHT, prev, cur, w),
            },
            match w.armed_off() {
                Some(_) => stow(w.off_sheath),
                None => sheath_phase2(ARM_LEFT, prev, cur, w),
            },
        ],
        2 => match ranged_arm(w) {
            None => [draw_leg(ARM_RIGHT, cur, w), draw_leg(ARM_LEFT, cur, w)],
            Some(ARM_LEFT) => [sheath_phase2(ARM_RIGHT, prev, cur, w), Some(RANGED_STOW)],
            Some(_) => [Some(RANGED_STOW), sheath_phase2(ARM_LEFT, prev, cur, w)],
        },
        _ => [None, None],
    }
}

/// A ceremony arm crossing its `$SHL`/`$SHR` key, once per arm; `sound::sheathe` rings the slot's
/// item off it. A snap plays no clip and so is silent, as in the reference.
#[derive(Message, Clone, Copy)]
pub(crate) struct SheathSwapMessage {
    pub(crate) entity: Entity,
    /// The held slot whose model just moved: 0 mainhand, 1 offhand, 2 ranged.
    pub(crate) slot: u8,
    /// Whether this arm drew its weapon rather than putting it away.
    pub(crate) drawing: bool,
}

/// `AnimationData.dbc`'s rows, whose WeaponFlags drive the per-animation sheath reconcile; absent,
/// only the engaged draw and the remote server-byte pull apply.
#[derive(Resource)]
pub(crate) struct AnimData(pub(crate) benilla_formats::AnimDataCatalog);

/// Load `AnimationData.dbc` off the patch chain at startup.
pub(super) fn load_anim_data(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_anim_data_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("anim: {} AnimationData policy rows", cat.len());
            commands.insert_resource(AnimData(cat));
        }
        Err(e) => warn!("anim: AnimationData failed to load: {e:#}"),
    }
}

/// Play an [`ArmLeg`] as a masked overlay on one arm; `None` (no clip or no arm mask) snaps it.
fn arm_leg(
    arm: usize,
    leg: ArmLeg,
    state: u8,
    player: &mut AnimationPlayer,
    anims: &ModelAnimations,
    catalog: Option<&AnimDataCatalog>,
) -> Option<SheathArm> {
    let c = find_resolved(anims, leg.clip, catalog)?;
    let arm_nodes = c.arm_nodes?;
    let node = if arm == ARM_RIGHT {
        arm_nodes.0
    } else {
        arm_nodes.1
    };
    let active = player.play(node);
    active.replay();
    active.set_weight(SHEATH_OVERLAY_WEIGHT);
    Some(SheathArm {
        node,
        swap_at: c
            .events
            .iter()
            .find(|e| matches!(&e.ident, b"$SHL" | b"$SHR"))
            .map(|e| e.time)
            .unwrap_or(c.duration * 0.5),
        drawing: leg.drawing,
        // A ranged state moves the ranged slot; otherwise the arm is the slot.
        slot: if state == 2 { 2 } else { arm as u8 },
        crossed: false,
    })
}

/// Start a requested ceremony's phase 1 ([`sheath_phase1`]); with no playable leg it all snaps.
pub(super) fn start_sheath_ceremony(
    commands: &mut Commands,
    entity: Entity,
    drv: &mut AnimDriver,
    player: &mut AnimationPlayer,
    anims: &ModelAnimations,
    wielded: Option<&Wielded>,
    old_state: u8,
    new_state: u8,
    catalog: Option<&AnimDataCatalog>,
) {
    let w = wielded.copied().unwrap_or_default();
    let legs = sheath_phase1(old_state, new_state, &w);
    let mut arms: [Option<SheathArm>; 2] = [None, None];
    for (arm, leg) in legs.into_iter().enumerate() {
        let Some(leg) = leg else { continue };
        // A stow leg moves the old state's item; a draw leg, the new state's.
        let state = if leg.drawing { new_state } else { old_state };
        arms[arm] = arm_leg(arm, leg, state, player, anims, catalog);
    }
    if arms.iter().any(Option::is_some) {
        let swap = SheathSwap {
            arms,
            prev: old_state,
        };
        commands.entity(entity).insert(VisualSheath([
            swap.arm_state(0, new_state),
            swap.arm_state(1, new_state),
        ]));
        drv.sheath_swap = Some(swap);
    }
}

/// Advance a ceremony: cross each arm's swap point, hand a finished stow to [`sheath_phase2`], and
/// drop [`VisualSheath`] once no arm has a leg left.
pub(super) fn advance_sheath_ceremony(
    commands: &mut Commands,
    entity: Entity,
    drv: &mut AnimDriver,
    player: &mut AnimationPlayer,
    anims: &ModelAnimations,
    wielded: Option<&Wielded>,
    cur: u8,
    catalog: Option<&AnimDataCatalog>,
    swaps: &mut MessageWriter<SheathSwapMessage>,
) {
    let Some(mut swap) = drv.sheath_swap.take() else {
        return;
    };
    let w = wielded.copied().unwrap_or_default();
    let prev = swap.prev;
    for arm in [ARM_RIGHT, ARM_LEFT] {
        let Some(leg) = &mut swap.arms[arm] else {
            continue;
        };
        // An overlay lost to a model rebuild counts as finished.
        let (crossed, finished) = match player.animation(leg.node) {
            Some(a) => (a.seek_time() >= leg.swap_at, a.is_finished()),
            None => (true, true),
        };

        if crossed && !leg.crossed {
            leg.crossed = true;
            swaps.write(SheathSwapMessage {
                entity,
                slot: leg.slot,
                drawing: leg.drawing,
            });
        }
        if !finished {
            continue;
        }
        let (node, drawing) = (leg.node, leg.drawing);
        player.stop(node);
        // Phase 2 (`0x5fc920` @ `0x5fca8c`/`0x5fcaa1`): a finished stow hands the arm to its
        // drawer; a finished draw releases it to its idle (`0x7121a0(subSeq, -1, …)`).
        swap.arms[arm] = (!drawing)
            .then(|| sheath_phase2(arm, prev, cur, &w))
            .flatten()
            .and_then(|next| arm_leg(arm, next, cur, player, anims, catalog));
    }
    if swap.arms.iter().any(Option::is_some) {
        commands.entity(entity).insert(VisualSheath([
            swap.arm_state(0, cur),
            swap.arm_state(1, cur),
        ]));
        drv.sheath_swap = Some(swap);
    } else {
        commands.entity(entity).remove::<VisualSheath>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_z_press_walks_melee_then_ranged_then_stowed() {
        const MELEE_AND_BOW: (bool, bool) = (true, true);
        const MELEE_ONLY: (bool, bool) = (true, false);
        const BOW_ONLY: (bool, bool) = (false, true);
        const EMPTY: (bool, bool) = (false, false);

        // Sword + bow: the full three-state walk, and round.
        assert_eq!(toggle_sheath_next(0, MELEE_AND_BOW), Some(1));
        assert_eq!(toggle_sheath_next(1, MELEE_AND_BOW), Some(2));
        assert_eq!(toggle_sheath_next(2, MELEE_AND_BOW), Some(0));

        // No ranged weapon: CUR 1 falls through to the stow (`0x5eb67f`).
        assert_eq!(toggle_sheath_next(0, MELEE_ONLY), Some(1));
        assert_eq!(toggle_sheath_next(1, MELEE_ONLY), Some(0));

        // Nothing in either hand: the draw goes straight to ranged (`0x5eb699`).
        assert_eq!(toggle_sheath_next(0, BOW_ONLY), Some(2));
        assert_eq!(toggle_sheath_next(2, BOW_ONLY), Some(0));

        // Nothing worn: no call at all (`0x5eb697: je 0x5eb6ad`).
        assert_eq!(toggle_sheath_next(0, EMPTY), None);
        assert_eq!(toggle_sheath_next(1, EMPTY), Some(0));
        assert_eq!(toggle_sheath_next(2, EMPTY), Some(0));
    }

    /// Sword, shield and bow: sheath types 3 (hip, 90), 4 (back, 89) and 1 (back, 89).
    fn warrior() -> Wielded {
        Wielded {
            main: Some((2, 7)),   // one-handed sword
            off: Some((4, 6)),    // shield
            ranged: Some((2, 2)), // bow
            main_sheath: 3,
            off_sheath: 4,
            ranged_sheath: 1,
            ranged_inv: 0x0f,     // INVTYPE_RANGED, the left arm
            materials: [1, 6, 2], // metal sword, plate shield, wood bow (real 5875 values)
            disarmed: false,
        }
    }

    const STOW_HIP: ArmLeg = ArmLeg {
        clip: 90,
        drawing: false,
    };
    const STOW_BACK: ArmLeg = ArmLeg {
        clip: 89,
        drawing: false,
    };
    const DRAW_HIP: ArmLeg = ArmLeg {
        clip: 90,
        drawing: true,
    };
    const DRAW_BACK: ArmLeg = ArmLeg {
        clip: 89,
        drawing: true,
    };

    #[test]
    fn a_full_pair_of_hands_stows_first_and_only_then_reaches_for_the_bow() {
        let w = warrior();

        // Phase 1 of melee → ranged: two stows, no draw.
        assert_eq!(sheath_phase1(1, 2, &w), [Some(STOW_HIP), Some(STOW_BACK)]);
        // Phase 2: the right arm is released to its idle; the left draws the bow.
        assert_eq!(sheath_phase2(ARM_RIGHT, 1, 2, &w), None);
        assert_eq!(sheath_phase2(ARM_LEFT, 1, 2, &w), Some(DRAW_BACK));

        // Back: the bow stows with a literal 89 (`0x611c8c`), the free right hand draws at once,
        // and the shield waits for phase 2.
        assert_eq!(sheath_phase1(2, 1, &w), [Some(DRAW_HIP), Some(STOW_BACK)]);
        assert_eq!(sheath_phase2(ARM_LEFT, 2, 1, &w), Some(DRAW_BACK));
    }

    #[test]
    fn drawing_from_stowed_is_a_single_movement() {
        let w = warrior();

        // 0 → 1 draws both hands at once; 0 → 2 puts the bow in the left and leaves the right out.
        assert_eq!(sheath_phase1(0, 1, &w), [Some(DRAW_HIP), Some(DRAW_BACK)]);
        assert_eq!(sheath_phase1(0, 2, &w), [None, Some(DRAW_BACK)]);
        // The drawers' `if (PREV == 0) return 0` (`0x6118a5`/`0x611965`): nothing deferred.
        for arm in [ARM_RIGHT, ARM_LEFT] {
            assert_eq!(sheath_phase2(arm, 0, 1, &w), None);
            assert_eq!(sheath_phase2(arm, 0, 2, &w), None);
        }

        // Stowing: phase 1 puts both away, and phase 2 finds nothing to draw for state 0.
        assert_eq!(sheath_phase1(1, 0, &w), [Some(STOW_HIP), Some(STOW_BACK)]);
        assert_eq!(sheath_phase1(2, 0, &w), [None, Some(STOW_BACK)]);
        for arm in [ARM_RIGHT, ARM_LEFT] {
            assert_eq!(sheath_phase2(arm, 1, 0, &w), None);
            assert_eq!(sheath_phase2(arm, 2, 0, &w), None);
        }
    }

    /// No offhand: the left arm draws during the right's stow (`0x611b60` into `0x611960`).
    #[test]
    fn a_free_hand_draws_without_waiting() {
        let w = Wielded {
            off: None,
            ..warrior()
        };
        assert_eq!(sheath_phase1(1, 2, &w), [Some(STOW_HIP), Some(DRAW_BACK)]);

        // A gun sits on the right arm (`[+3] == 0x1a`): the mainhand stows, the gun's draw defers.
        let g = Wielded {
            ranged_inv: 0x1a,
            ..w
        };
        assert_eq!(sheath_phase1(1, 2, &g), [Some(STOW_HIP), None]);
        assert_eq!(sheath_phase2(ARM_RIGHT, 1, 2, &g), Some(DRAW_BACK));
    }
}
