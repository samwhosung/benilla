//! The sheath **policy layer**'s types + ceremony mechanics: the one-setter
//! request, the client-side state cache's visual pin, the draw/stow ceremony overlays, and the
//! `AnimationData.dbc` policy table. The driver systems in [`super::driver`] execute these — the setter,
//! the field-apply adopt, and the per-animation reconcile all live in `drive_animations`, so
//! every sheath transition has exactly one author.

use benilla_assets::ModelAnimations;
use benilla_formats::AnimDataCatalog;
use bevy::prelude::*;

use benilla_assets::{LockRecover, WorldAssets};

use super::{find_resolved, select, AnimDriver, Wielded};

/// Holds a unit's *visual* sheath state **per arm** while the draw/stow one-shots play, so each
/// weapon model swaps hand↔hip/back at its own clip's authored `$SHL`/`$SHR` event (~the moment
/// that hand reaches the stow point — 500/567 ms into the 1 s clips, dumped from HumanMale.m2)
/// instead of the instant the state changes. Per-arm because the ceremony is: a warrior going
/// melee → ranged stows both hands *and then* reaches for the bow, so the sword leaves the right
/// hand a full clip before the bow arrives in the left. Removed when every arm has settled — the
/// equipment resolver then falls through to the committed sheath state. Absent = no transition in
/// flight. Index by [`ARM_RIGHT`]/[`ARM_LEFT`] — or by held slot, via [`Self::for_slot`].
#[derive(Component, Clone, Copy)]
pub(crate) struct VisualSheath(pub(crate) [u8; 2]);

impl VisualSheath {
    /// The effective sheath state governing one **held slot**'s placement: mainhand on the right
    /// arm, offhand on the left, and the ranged item on whichever arm its `InventoryType` puts it
    /// (the byte-verified `0x1a`/`0x19` compare — see [`ranged_arm`]).
    pub(crate) fn for_slot(self, slot: usize, inv_type: u32) -> u8 {
        self.0[match slot {
            0 => ARM_RIGHT,
            1 => ARM_LEFT,
            _ if matches!(inv_type, 0x1a | 0x19) => ARM_RIGHT,
            _ => ARM_LEFT,
        }]
    }
}

/// One arm's leg of the ceremony in flight — a masked one-shot on that arm's subtree (the client's
/// per-slot `0x60b770` plays on sub-sequence 3/2), composed over whatever the body is doing: walk,
/// run, jump; never cancelled.
pub(super) struct SheathArm {
    node: bevy::animation::graph::AnimationNodeIndex,
    /// The authored `$SHL`/`$SHR` moment — when this arm's weapon actually moves.
    swap_at: f32,
    /// This clip ends with the weapon **in the hand** — the client's `+0xd58` phase bit (see
    /// [`ArmLeg::drawing`]). `false` = a stow, and the one thing phase 2 keys on.
    drawing: bool,
    /// The held slot this leg moves: 0 mainhand · 1 offhand · 2 ranged.
    slot: u8,
    /// The clip has passed [`Self::swap_at`] — the weapon has moved, the sound has rung.
    crossed: bool,
}

/// The draw/stow ceremony in flight — up to one leg per arm, each advancing independently.
pub(super) struct SheathSwap {
    arms: [Option<SheathArm>; 2],
    /// The state the ceremony left (the client's `+0xd3c` PREV): the pre-swap placement for a
    /// stow leg, and phase 2's gate.
    prev: u8,
}

impl SheathSwap {
    /// The effective sheath state for one arm: a stow leg shows [`Self::prev`] until its weapon
    /// moves and stowed (0) after; a draw leg shows stowed until its weapon arrives and the
    /// committed state after; an arm with no leg is simply snapped.
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

/// The per-arm overlay's weight over the gait on the arm bones both key: the ceremony dominates the
/// walk's arm-swing ≈ 8:1 (the client's per-bone arming gives it the arm outright; a small bleed is
/// the cost of Bevy's weighted blend). Legs are excluded entirely by the mask.
const SHEATH_OVERLAY_WEIGHT: f32 = 8.0;

/// A request to change a unit's sheath state — benilla's analogue of the client's **one setter**
/// `SetSheatheState(newState, bInstant, bFireEvent)` (`0x611cf0`, decision 0080 structure 1).
/// Every path that changes the state funnels here — the manual Z toggle (the only `ceremony`
/// sender), the attack-start auto-draw, the stand-state stow rider — and `drive_animations` is
/// the sole executor: the idempotency refusal, the commit to the client-side cache, the
/// `CMSG_SETSHEATHED` volunteer for the local player (`bFireEvent = 1`), and the ceremony-vs-snap
/// visual all live in that one place. Across all 24 client call sites, **only the manual
/// `ToggleSheath` passes `bInstant = 0`** — every reactive trigger and the server-field apply snap.
#[derive(Message, Clone, Copy)]
pub(crate) struct SheathRequest {
    pub(crate) entity: Entity,
    /// The requested state: 0 unarmed/stowed · 1 melee drawn · 2 ranged drawn.
    pub(crate) state: u8,
    /// Play the draw/stow ceremony (the manual toggle's `bInstant = 0`); everything else snaps.
    pub(crate) ceremony: bool,
}

/// The manual `ToggleSheath` **cycle** — the state a Z press asks for next, or `None` where the
/// ref makes no `SetSheatheState` call at all. Byte-read off `0x5eb642`–`0x5eb6a8` (the four
/// ToggleSheath call sites; the dispatch is a `sub 0; je` / `dec; je` / `dec; jne` walk over the
/// committed state `[unit+0xd40]`):
///
/// ```text
/// CUR 0   mainhand or offhand worn -> 1 melee      (0x5eb6a0: push 1, 0, 1)
///         else ranged worn         -> 2 ranged     (0x5eb699: push 1, ebx=0, 2)
///         else                     -> no call      (0x5eb697: je 0x5eb6ad)
/// CUR 1   ranged worn              -> 2 ranged     (0x5eb671)
///         else                     -> 0 stowed     (0x5eb67f)
/// CUR 2                            -> 0 stowed     (0x5eb653)
/// ```
///
/// So the press walks **three** states — melee, then ranged, then stowed — never a two-state
/// flip. `melee`/`ranged` are the ref's three `GetWeapon(0/1/2)` results (vtable `+0x98`, called
/// at `0x5eb5f0`/`0x5eb600`/`0x5eb610`); ours are [`Wielded`]'s slots, carrying the same "an item
/// is worn there" fact. **Named deviation:** the ref also zeroes its ranged candidate when the
/// unit's class byte fails a `DAT_00c0def4` "can hold a ranged weapon" lookup
/// (`0x5eb616`–`0x5eb640`, INFERRED semantics) — unobservable here, since a class that fails it
/// cannot equip a ranged item to begin with.
pub(crate) fn toggle_sheath_next(cur: u8, (melee, ranged): (bool, bool)) -> Option<u8> {
    match cur {
        0 if melee => Some(1),
        0 if ranged => Some(2),
        0 => None,
        1 if ranged => Some(2),
        1 | 2 => Some(0),
        _ => None, // the ref's `dec ecx; jne` tail — no call for a state outside {0, 1, 2}
    }
}

/// One arm's leg of a draw/stow ceremony: the clip to play, and whether that arm ends with its
/// weapon **in the hand**. `drawing` is the client's `+0xd58` per-arm phase bit — set by the
/// drawers (`0x6118a0 |= 0x100000`, `0x611960 |= 0x200000`, `0x611a20 |= 0x300000`), cleared by
/// every stow leg (`0x611b60`'s `& 0xffefffff` / `& 0xffdfffff`). Layer B reads it at the clip's
/// `$SHL`/`$SHR` event to decide hand-vs-sheath-bone, which is exactly what it means here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct ArmLeg {
    pub(super) clip: u16,
    pub(super) drawing: bool,
}

/// Arm indices — the client's animation **sub-sequence** slots, which are per-arm: the mainhand
/// plays on 3 (HandRight), the offhand on 2 (HandLeft), and the ranged weapon on whichever its
/// `InventoryType` picks (bow ⇒ left, gun/crossbow/wand/thrown ⇒ right).
pub(super) const ARM_RIGHT: usize = 0;
pub(super) const ARM_LEFT: usize = 1;

/// Which arm the ranged weapon occupies, and whether one is worn at all. Byte-verified compare
/// (`0x611c74`, `0x6118db`, `0x611998`, `0x611a9a` — identical at every site): `[+3] == 0x1a`
/// (RANGEDRIGHT — gun/crossbow/wand) or `== 0x19` (THROWN) ⇒ the **right** arm; anything else
/// (`0x0f`, INVTYPE_RANGED — a bow) ⇒ the **left**.
fn ranged_arm(w: &Wielded) -> Option<usize> {
    w.ranged.map(|_| {
        if matches!(w.ranged_inv, 0x1a | 0x19) {
            ARM_RIGHT
        } else {
            ARM_LEFT
        }
    })
}

/// The **draw** leg one arm would play for a committed state — the shared body of the client's
/// two per-arm drawers (`0x6118a0` right, `0x611960` left) and of the both-arms `0x611a20`, all
/// of which run the identical `(1 << (rec[+4] & 0x1f)) & 0x88` pick on the slot's own record.
/// `None` = that arm has nothing to draw for this state, and is released to its idle.
fn draw_leg(arm: usize, cur: u8, w: &Wielded) -> Option<ArmLeg> {
    let (item, sheath) = match (cur, arm) {
        // The drawers read `GetWeapon(slot, 0)` like the rest of the sheath machine (1863), so a
        // disarmed hand has nothing to draw — the reference's `0x5eb480` takes its unarmed branch.
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

/// **Phase 2** — the deferred draw, and the half of the ceremony benilla never had. The client
/// runs it from its **on-anim-finish** handler (`0x5fc920` @ `0x5fca62`–`0x5fcab6`): when a
/// `0x59`/`0x5a` clip finishes on sub-sequence 2 or 3 and that arm's phase bit is still **clear**
/// (i.e. what just finished was a *stow*), it calls that arm's drawer — `0x611960` @ `0x5fcaaf`
/// for the left, `0x6118a0` @ `0x5fca9a` for the right; a bit already set falls through to
/// `0x7121a0(subSeq, -1, …)`, releasing the arm to its idle. A leg here is the second full
/// movement: the arms come back to neutral, and only *then* does a hand reach for the new weapon.
///
/// The `prev == 0` refusal is the drawers' own first test (`0x6118a5` / `0x611965`:
/// `if (PREV == 0) return 0`) — a draw **out of the stowed state** is a single movement, already
/// played in full by [`sheath_phase1`]'s `0x611a20` leg, so there is nothing left to defer.
pub(super) fn sheath_phase2(arm: usize, prev: u8, cur: u8, w: &Wielded) -> Option<ArmLeg> {
    (prev != 0).then(|| draw_leg(arm, cur, w)).flatten()
}

/// **Phase 1** — what the setter itself plays, keyed on **PREV** (the client's `0x611b60`, the
/// `bInstant == 0` fork of `SetSheatheState`). Byte-read at `0x611b60`–`0x611ce6`:
///
/// ```text
/// PREV 0  -> 0x611a20: draw BOTH arms for CUR now, set both bits — ONE movement
/// PREV 1  -> right: mainhand worn ? stow it : the right drawer (0x6118a0)
///            left : offhand  worn ? stow it : the left  drawer (0x611960)
/// PREV 2  -> no ranged item  ? 0x611a20 (as PREV 0)
///            bow (left arm)  : stow LEFT with a literal 89, right drawer (0x611c8c/0x611ca1)
///            gun/xbow/thrown : stow RIGHT with a literal 89, left drawer (0x611cd3/0x611ce1)
/// ```
///
/// So an arm with nothing to put away **draws immediately** — the ref's tail calls into the
/// drawers — while an arm that does put something away defers its draw to [`sheath_phase2`]. A
/// warrior with both hands full stows both, *then* reaches: two movements. A lone mainhand leaves
/// the other arm free, so the draw rides along with the stow: one.
///
/// **Named byte detail:** the PREV==2 stow legs push a literal `0x59` rather than running the
/// `& 0x88` pick (`0x611c8c`, `0x611cd3`), so a ranged weapon is always put away with Sheath 89
/// even where its own sheath type would have picked HipSheath 90 on the way out.
pub(super) fn sheath_phase1(prev: u8, cur: u8, w: &Wielded) -> [Option<ArmLeg>; 2] {
    /// The ranged **stow** clip — a literal, not a pick (see above).
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
        // `LAB_00611ce6 -> 0x611a20`: nothing is in hand to put away, so both arms draw at once
        // and no phase 2 can follow (the drawers' `PREV == 0` refusal).
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

/// One executed draw/stow swap at its hand-touches-weapon moment — fired only by the *ceremony*,
/// **once per arm** as that arm's clip crosses its authored `$SHL`/`$SHR` event. Snap transitions
/// never fire it: in the client the draw/stow sound rides the ceremony clip's own keyframes, and a
/// `bInstant` path plays no clip — so snaps are silent (director-verified on the ref).
/// `sound::sheathe` rings [`Self::slot`]'s item off it, so a two-movement melee→ranged toggle
/// rings the swords going away and then, a clip later, the bow coming out.
#[derive(Message, Clone, Copy)]
pub(crate) struct SheathSwapMessage {
    pub(crate) entity: Entity,
    /// The held slot whose model just moved: 0 mainhand · 1 offhand · 2 ranged.
    pub(crate) slot: u8,
    /// `true` = this arm drew its weapon; `false` = it put it away.
    pub(crate) drawing: bool,
}

/// `AnimationData.dbc` policy rows — the WeaponFlags column driving the per-animation sheath
/// reconcile (decision 0080 structure 3). Optional: absent (no client data) the reconcile
/// degrades to the engaged-draw + the remote server-byte pull-through.
#[derive(Resource)]
pub(crate) struct AnimData(pub(crate) benilla_formats::AnimDataCatalog);

/// Load `AnimationData.dbc` off the patch chain at startup (the sound catalogs' pattern).
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

/// Arm an [`ArmLeg`] as a live masked overlay on one arm's subtree: the clip resolved through the
/// model's own baked fallback first, played over whatever the body is doing. `None`
/// when the model has no such clip or no arm mask — that arm then simply snaps.
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
        // The moving slot is the ranged one whenever the leg's own state is ranged; otherwise the
        // arm *is* the slot (mainhand right, offhand left).
        slot: if state == 2 { 2 } else { arm as u8 },
        crossed: false,
    })
}

/// Start the draw/stow **ceremony** for a [`SheathRequest`] that asked for one (the manual
/// toggle's `bInstant = 0`) — **phase 1** ([`sheath_phase1`], the client's `0x611b60`): each arm
/// gets at most one masked overlay, and [`VisualSheath`] holds that arm's pre-swap placement until
/// its clip reaches the authored `$SHL`/`$SHR` moment. An arm whose leg has no playable clip
/// snaps; no arm playing anything at all → no ceremony, and the whole transition snaps.
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
        // A stow leg moves what the OLD state had in that hand; a draw leg, what the new one will.
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

/// Advance a ceremony in flight: cross each arm's swap point (moving that arm's weapon and ringing
/// it), and — the client's **phase 2** — when a *stow* clip finishes, hand that arm to
/// [`sheath_phase2`] so it can reach for the new weapon. The ceremony ends when every arm has run
/// out of legs, at which point [`VisualSheath`] is dropped and the resolver falls back to the
/// committed state.
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
        // An overlay that vanished under us (a model rebuild) counts as finished — never leave a
        // weapon pinned to a placement nothing is animating towards.
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
        // **Phase 2** (`0x5fc920` @ `0x5fca8c`/`0x5fcaa1`): a finished *stow* — the phase bit still
        // clear — hands this arm to its drawer. A finished draw (bit set) falls through to the
        // arm's idle, exactly as the ref releases the sub-sequence with `0x7121a0(subSeq, -1, …)`.
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

    /// The `ToggleSheath` cycle exactly as the bytes branch (`0x5eb642`–`0x5eb6a8`): three states,
    /// each leg gated on what is actually worn. The director's report — "Z should take out swords
    /// then range then nothing" — is the first row.
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

        // No ranged weapon: the CUR=1 leg falls through to the stow (`0x5eb67f`) — the two-state
        // flip, which is correct only here.
        assert_eq!(toggle_sheath_next(0, MELEE_ONLY), Some(1));
        assert_eq!(toggle_sheath_next(1, MELEE_ONLY), Some(0));

        // Nothing in either hand: the draw goes straight to ranged (`0x5eb699`).
        assert_eq!(toggle_sheath_next(0, BOW_ONLY), Some(2));
        assert_eq!(toggle_sheath_next(2, BOW_ONLY), Some(0));

        // Nothing worn at all: the ref makes no call (`0x5eb697: je 0x5eb6ad`) — the press is a
        // silent no-op, not a draw of empty hands.
        assert_eq!(toggle_sheath_next(0, EMPTY), None);
        assert_eq!(toggle_sheath_next(1, EMPTY), Some(0));
        assert_eq!(toggle_sheath_next(2, EMPTY), Some(0));
    }

    /// A sword-and-board warrior carrying a bow — the director's loadout. Sheath types: sword on
    /// the hip (3 ⇒ HipSheath 90), shield on the back (4 ⇒ Sheath 89), bow on the back (1 ⇒ 89).
    fn warrior() -> Wielded {
        Wielded {
            main: Some((2, 7)),   // one-handed sword
            off: Some((4, 6)),    // shield
            ranged: Some((2, 2)), // bow
            main_sheath: 3,
            off_sheath: 4,
            ranged_sheath: 1,
            ranged_inv: 0x0f,     // INVTYPE_RANGED — the left arm
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

    /// **The director's report, as a table.** Both hands full: a melee → ranged toggle puts the
    /// sword and shield away and draws *nothing* — the bow waits for phase 2, which fires when
    /// each stow clip finishes and reaches for it on the left arm alone. Byte-read at `0x611b60`
    /// (phase 1) and `0x5fc920` @ `0x5fca8c`/`0x5fcaa1` (phase 2).
    #[test]
    fn a_full_pair_of_hands_stows_first_and_only_then_reaches_for_the_bow() {
        let w = warrior();

        // Phase 1 of melee → ranged: two stows, no draw anywhere. This is movement one.
        assert_eq!(sheath_phase1(1, 2, &w), [Some(STOW_HIP), Some(STOW_BACK)]);
        // Phase 2, once those finish: the right arm has nothing to draw for a bow and is released
        // to its idle; the left reaches over the shoulder. Movement two.
        assert_eq!(sheath_phase2(ARM_RIGHT, 1, 2, &w), None);
        assert_eq!(sheath_phase2(ARM_LEFT, 1, 2, &w), Some(DRAW_BACK));

        // …and back the same way (the director's "that's how it puts it back too"): the bow is
        // stowed with a LITERAL 89 (`0x611c8c`) rather than its own `& 0x88` pick, and the
        // mainhand — whose hand is free — draws right away, while the shield waits for phase 2.
        assert_eq!(sheath_phase1(2, 1, &w), [Some(DRAW_HIP), Some(STOW_BACK)]);
        assert_eq!(sheath_phase2(ARM_LEFT, 2, 1, &w), Some(DRAW_BACK));
    }

    /// The one-movement cases, which must NOT grow a second phase: every draw out of the stowed
    /// state (`0x611b60` → `0x611a20`, both bits set) and every stow into it.
    #[test]
    fn drawing_from_stowed_is_a_single_movement() {
        let w = warrior();

        // 0 → 1 draws both hands at once; 0 → 2 puts the bow in the left and leaves the right out.
        assert_eq!(sheath_phase1(0, 1, &w), [Some(DRAW_HIP), Some(DRAW_BACK)]);
        assert_eq!(sheath_phase1(0, 2, &w), [None, Some(DRAW_BACK)]);
        // The drawers' own `if (PREV == 0) return 0` (`0x6118a5`/`0x611965`) — nothing deferred.
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

    /// A lone mainhand leaves the other arm free, so its draw rides along with the stow — the ref
    /// falls straight through `0x611b60`'s `if (oh != 0) … return` into `FUN_00611960`. One
    /// movement, and the reason the two-phase shape only shows up with both hands occupied.
    #[test]
    fn a_free_hand_draws_without_waiting() {
        let w = Wielded {
            off: None,
            ..warrior()
        };
        assert_eq!(sheath_phase1(1, 2, &w), [Some(STOW_HIP), Some(DRAW_BACK)]);

        // A gun sits on the RIGHT arm (`[+3] == 0x1a`), so the same loadout inverts: the mainhand
        // must be put away first, and the gun's draw is what defers.
        let g = Wielded {
            ranged_inv: 0x1a,
            ..w
        };
        assert_eq!(sheath_phase1(1, 2, &g), [Some(STOW_HIP), None]);
        assert_eq!(sheath_phase2(ARM_RIGHT, 1, 2, &g), Some(DRAW_BACK));
    }
}
