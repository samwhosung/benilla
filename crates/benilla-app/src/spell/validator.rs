//! **The requirement validator's rungs** — `Spell_C`'s `0x6094f0`, the press-time gates
//! `TryCast 0x6e4b60` runs between the target bind and the commit, plus the range compare
//! `IsTargetInRange 0x6e47b0` that `CanTargetUnit 0x6e4440` runs just before it. Pure
//! predicates over the caster's fields and the spell record, each answering the reference's own
//! reason code; [`super::cast_send`]'s ladder calls them in the reference's order, and the two
//! casts still outside the ladder (the world click's skin cast and the corpse insignia,
//! `crate::target::click`) ask the mounted one directly.
//!
//! Until decision 2330 these sat in `ui_action`'s dynamic-state feed, beside the `IsUsableAction`
//! family they are not part of: the feed greys a button, the validator refuses a press, and the
//! reference keeps them apart (`0x6094f0` is reached only from `TryCast`, never from the greying
//! predicate `0x6e3d60`). Now they are the spell's, with the ladder.

use benilla_formats::{SpellDisplay, SpellRange};

/// The client's cast-fail reasons for the two range refusals ("Out of range." / "Target too
/// close" in `crate::ui_action`'s cast-fail table) — what `CanTargetUnit 0x6e4440` emits when
/// `IsTargetInRange 0x6e47b0` fails on its max² / min² compare.
pub(super) const ERR_OUT_OF_RANGE: u8 = 0x59;
pub(super) const ERR_TOO_CLOSE: u8 = 0x76;

/// The **pre-send** range refusal — the client's `TryCast` ladder runs `CanTargetUnit 0x6e4440`
/// → `IsTargetInRange 0x6e47b0` BEFORE `ArmCast`/`SendCast`, so
/// an out-of-range or too-close press fails locally and the commit tail — the ranged sheath
/// snap `0x6e5930` included — never runs. This is why a too-close Throw/Auto Shot must NOT draw
/// the ranged weapon. Squared 3D distance against [`benilla_formats::min_max_range`]'s {min, max}: beyond max² →
/// [`ERR_OUT_OF_RANGE`], inside a nonzero min² → [`ERR_TOO_CLOSE`]. Untestable inputs (no range
/// row, unknown distance) pass — the server still judges the cast.
pub(super) fn cast_range_refusal(
    spell: &SpellDisplay,
    row: Option<&SpellRange>,
    self_reach: f32,
    target_reach: Option<f32>,
    dist_sq: Option<f32>,
) -> Option<u8> {
    let (min, max) = benilla_formats::min_max_range(spell, row, self_reach, target_reach)?;
    let d2 = dist_sq?;
    if d2 > max * max {
        return Some(ERR_OUT_OF_RANGE);
    }
    if min > 0.0 && d2 < min * min {
        return Some(ERR_TOO_CLOSE);
    }
    None
}

/// `PreventionType` values (`Spell.dbc` column 165): which crowd-control flag can refuse this
/// spell locally. `0` = neither.
const PREVENTION_SILENCE: u32 = 1;
const PREVENTION_PACIFY: u32 = 2;

/// The **crowd-control leg** of the same requirement validator `0x6094f0`, sitting **above** its
/// mounted block (`0x609c6c`) — so a stunned mounted caster is told about the stun (decision
/// 1904). It refuses before any packet,
/// which is why it must be local.
///
/// **Six arms, in the reference's order, first match wins** — 1863's fold-back recorded four:
///
/// | # | arm | gate | reason |
/// |---|---|---|---|
/// | 1 | CHARMED | `UNIT_FIELD_CHARMEDBY != 0` and the charmer is not us | `0x14` |
/// | 2 | STUNNED | bit 18 — **no per-spell gate, every spell** | `0x64` |
/// | 3 | SILENCED | bit 13 **and** `PreventionType == 1` | `0x60` |
/// | 4 | PACIFIED | bit 17 **and** `PreventionType == 2` | `0x5a` |
/// | 5 | FLEEING | bit 23 | `0x1e` |
/// | 6 | CONFUSED | bit 22 | `0x16` |
///
/// The asymmetry is the finding: STUNNED, FLEEING, CHARMED and CONFUSED carry no per-spell gate at
/// all, while SILENCED and PACIFIED run only where the spell's own `PreventionType` names them —
/// so a silence stops casts and leaves melee abilities alone, and a pacify does the reverse.
/// Reading `UNIT_FLAG_SILENCED` as "no casting" is the mistake this replaces.
///
/// **A dead caster skips all six** (`0x60980d`, a `jle` on `UNIT_FIELD_HEALTH`).
///
/// This ladder is **TryCast-only** — one caller, no address-takes — so a button does **not** grey
/// while stunned, silenced or pacified: it refuses on the press. (Fear, confuse and charm *are*
/// greyed, by a second copy of the same helpers inside `0x6e3d60`; that copy is not built here.)
///
/// Its byte-shape is a trap for a binary scan: SILENCED's read is `f6 c4 20 test ah,0x20`, a
/// sub-register byte-lane form with no dword immediate, invisible to an immediate scan.
///
/// **The exemption scan is built** (closing 1925's deferral): each arm first asks
/// whether any of the caster's own auras grants immunity to what is blocking it — a scan of
/// `UNIT_FIELD_AURA[0..47]`'s raw spell ids for an aura of the arm's own type, then
/// [`benilla_formats::grants_immunity`] on each match. A hit **lifts** the refusal; a rejection
/// names the blocking mechanic and turns the arm's own reason into `0x8d`. Each arm scans for a
/// different set of aura types — charm `{6, 177, 2}`, stun `{12}`, silence `{27, 12, 60}`, pacify
/// `{25, 12, 60}`, fear `{7}`, confuse `{5}` — and those sets are the reference's, not a family
/// resemblance.
pub(crate) fn cast_cc_refusal(
    unit_flags: u32,
    health: Option<u32>,
    charmed_by_other: bool,
    spell: Option<&SpellDisplay>,
    exempt: &mut impl FnMut(&[u32]) -> benilla_formats::CcExemption,
) -> Option<(u8, Option<u32>)> {
    use crate::player::UNIT_FLAG_STUNNED;
    /// `UNIT_FIELD_FLAGS` bits 13/17/22/23 (vmangos `UnitDefines.h`).
    const UNIT_FLAG_SILENCED: u32 = 0x0000_2000;
    const UNIT_FLAG_PACIFIED: u32 = 0x0002_0000;
    const UNIT_FLAG_CONFUSED: u32 = 0x0040_0000;
    const UNIT_FLAG_FLEEING: u32 = 0x0080_0000;

    // The dead caster's skip (`0x60980d`): a corpse is refused by an earlier rung, not this one.
    if health == Some(0) {
        return None;
    }
    let prevention = spell.map_or(0, |d| d.prevention_type);
    // One arm: ask its exemption scan first, and let its answer pick between silence and one of
    // two messages. `exempt` returning `exempt: true` means one of the caster's own auras grants
    // immunity to whatever is blocking — the arm is SKIPPED and the cast proceeds. Otherwise the
    // refusal is `0x8d` "Can't do that while %s" when the scan named a mechanic, and the arm's own
    // reason when it did not.
    let mut arm = |aura_types: &[u32], own_reason: u8| -> Option<(u8, Option<u32>)> {
        let scan = exempt(aura_types);
        if scan.exempt {
            return None;
        }
        Some(if scan.mechanic != 0 {
            // The mechanic rides along as the message's `%s` — the one client-LOCAL refusal that
            // carries an argument word.
            (REASON_PREVENTED_BY_MECHANIC, Some(scan.mechanic))
        } else {
            (own_reason, None)
        })
    };

    if charmed_by_other {
        if let Some(r) = arm(&[6, 177, 2], 0x14) {
            return Some(r);
        }
    }
    if unit_flags & UNIT_FLAG_STUNNED != 0 {
        if let Some(r) = arm(&[12], 0x64) {
            return Some(r);
        }
    }
    if unit_flags & UNIT_FLAG_SILENCED != 0 && prevention == PREVENTION_SILENCE {
        if let Some(r) = arm(&[27, 12, 60], 0x60) {
            return Some(r);
        }
    }
    if unit_flags & UNIT_FLAG_PACIFIED != 0 && prevention == PREVENTION_PACIFY {
        if let Some(r) = arm(&[25, 12, 60], 0x5a) {
            return Some(r);
        }
    }
    if unit_flags & UNIT_FLAG_FLEEING != 0 {
        if let Some(r) = arm(&[7], 0x1e) {
            return Some(r);
        }
    }
    if unit_flags & UNIT_FLAG_CONFUSED != 0 {
        if let Some(r) = arm(&[5], 0x16) {
            return Some(r);
        }
    }
    None
}

/// `SPELL_FAILED_PREVENTED_BY_MECHANIC` — "Can't do that while %s", `%s` being the blocking aura's
/// `SpellMechanic.dbc` name. Every crowd-control arm carries this **as well as** its own reason,
/// and which one appears is decided by whether the exemption scan named a mechanic.
const REASON_PREVENTED_BY_MECHANIC: u8 = 0x8d;

/// The **pre-send** mounted refusal — the requirement validator `0x6094f0`'s
/// mounted block (`0x609c6c`): a live
/// `UNIT_FIELD_MOUNTDISPLAYID` refuses the cast with reason `0x39` ("You are mounted") unless
/// the spell carries Attributes bit 24 (`0x01000000`, castable-while-mounted — the exemption
/// test at `0x609c6f`, the exact vmangos `SPELL_ATTR_ALLOW_WHILE_MOUNTED` mirror). A spell
/// with no loaded record has no exemption to claim — the gate holds (the ref always has the
/// record; refusing without data errs toward the ref's visible behavior). The sibling
/// mount-REQUIRED gate (reason 0x53, `0x609c05`) is recorded but unbuilt — no 1.12 player spell
/// exercises it. It is **two-armed** like the water pair below, not the single `+0x5c & 0x40`
/// read this comment used to claim: arm A is `AuraInterruptFlags & 0x40` (the `cl` at
/// `0x609c05` is the untouched low byte of `ecx = [esi+0x58]`, loaded 0x104 bytes earlier for
/// the unsheathed leg), arm B is `ChannelInterruptFlags & 0x40` at `0x609c3a` — corrected
/// in 1063. Note the plain mounted gate above is NOT in
/// that family: it is single-armed on `Attributes`.
pub(crate) fn cast_mounted_refusal(mounted: bool, spell: Option<&SpellDisplay>) -> bool {
    mounted && spell.is_none_or(|d| d.attributes & 0x0100_0000 == 0)
}

/// The interrupt-flag water pair — the two bits that say *which side of the surface this aura
/// can live on*: `0x80` cancels it on ENTERING water, `0x100` on LEAVING it (vmangos
/// `AURA_INTERRUPT_UNDER_WATER_CANCELS` / `AURA_INTERRUPT_ABOVE_WATER_CANCELS`, bits 7/8). The
/// requirement validator reads the same pair to refuse the *cast* whose aura could not survive
/// the caster's current side.
const INTERRUPT_UNDER_WATER: u32 = 0x80;
const INTERRUPT_ABOVE_WATER: u32 = 0x100;

/// `AttributesEx & (IS_CHANNELED 0x4 | IS_SELF_CHANNELED 0x40)` — the gate on **arm B** of every
/// leg in this family (byte-verified `0x609d6a`/`0x609db1`). A channeled spell's requirement
/// bits live in its CHANNEL column, so the validator reads that column too — but only for a
/// spell that actually channels, which is what keeps Summon Baby Shark 25849 (`Channel 0x100`,
/// `AttributesEx 0`) out of the gate.
const ATTR_EX_CHANNELED: u32 = 0x44;

/// `SPELL_FAILED_ONLY_ABOVEWATER` — "Cannot use while swimming".
pub(super) const ERR_ONLY_ABOVEWATER: u8 = 0x50;
/// `SPELL_FAILED_ONLY_UNDERWATER` — "Can only use while swimming".
pub(super) const ERR_ONLY_UNDERWATER: u8 = 0x58;

/// The **pre-send** water refusal — the requirement validator
/// `0x6094f0`'s environment block `0x609d33–0x609de2`. It sits after the mounted/posture/day/night
/// legs and before the moving gate (`0x609de3`), which is where the ladder runs it — so a druid
/// standing on land is refused **before** the form gate `0x612480` ever evaluates.
///
/// One gate, two faces, and each face has **two arms** reading the same bit in two columns:
///
/// | reason | bit | arm A — `AuraInterruptFlags` (+0x58) | arm B — `ChannelInterruptFlags` (+0x5c), if channeled |
/// |---|---|---|---|
/// | [`ERR_ONLY_UNDERWATER`] `0x58` | `0x100` | `0x609d36` / swim `0x609d46` | `0x609d6f` / swim `0x609d7b` |
/// | [`ERR_ONLY_ABOVEWATER`] `0x50` | `0x80` | `0x609da4` / swim `0x609dac` | `0x609db5` / swim `0x609dc2` |
///
/// The swimming state is `[[caster+0x118]+0x40] & 0x200000` — the same wire-layout movement word
/// the moving gate reads, four times over, twice in each face.
///
/// **Arm B is not dead code, and leaving it out is visibly wrong**: Fishing rank 1 (7620)
/// carries `AuraInterruptFlags 0x80`, but ranks 2–4 (7731/7732/18248) carry **zero** and reach
/// the gate only through their `ChannelInterruptFlags 0x3cac`. Arm A alone would refuse rank 1
/// mid-swim and let its own upgrades through.
///
/// What the two bits actually select in the 5875 data: `0x80` is 245 rows — every mount, Travel
/// Form, the campfires, all of Food/Drink (`0x40080`), Fishing — and `0x100` is exactly five:
/// Aquatic Form 1066, the Lava/Slime swim auras 16455/16456, Master Angler 24346/24347.
///
/// **No exemption skips this block** (every jump into the run was enumerated) — unlike the
/// mounted gate's `Attributes` bit 24. And the gate must be LOCAL: vmangos's `CheckCast` gates
/// water only on `SPELL_AURA_MOUNTED` (`Spell.cpp:6379`), so it happily grants a druid aquatic
/// form on dry cobblestone (ledger B176, with the screenshot to prove it). An uncataloged spell
/// passes, like every other data-driven rung on this ladder.
///
/// The legs are evaluated `0x58` before `0x50`, the binary's own order; they are mutually
/// exclusive on the swim bit, so the order is fidelity, not behaviour.
pub(super) fn cast_water_refusal(move_flags_word: u32, spell: Option<&SpellDisplay>) -> Option<u8> {
    let d = spell?;
    let swimming = move_flags_word & crate::creature_anim::move_flags::SWIMMING != 0;
    // Arm A always; arm B only for a spell that actually channels.
    let requires = |bit: u32| {
        d.aura_interrupt_flags & bit != 0
            || (d.attributes_ex & ATTR_EX_CHANNELED != 0 && d.channel_interrupt_flags & bit != 0)
    };
    if !swimming && requires(INTERRUPT_ABOVE_WATER) {
        return Some(ERR_ONLY_UNDERWATER);
    }
    if swimming && requires(INTERRUPT_UNDER_WATER) {
        return Some(ERR_ONLY_ABOVEWATER);
    }
    None
}

/// The AuraInterruptFlags-space MOVING|TURNING pair (`0x18`) — the moving gate's
/// "movement would matter anyway" arms test it on BOTH `AuraInterruptFlags` (+0x58) and
/// `ChannelInterruptFlags` (+0x5c), byte-verified at `0x609e0e`/`0x609e1c`.
const AURA_INTERRUPT_MOVING_TURNING: u32 = 0x18;

/// The **pre-send** moving refusal — the requirement validator `0x6094f0`'s
/// moving block (`0x609de3–0x609e48`; the sole client-local emitter of reason `0x2e` "Can't do
/// that while moving"): a press while the
/// caster's live CMovement flags carry any of {forward, backward, strafe L/R, JUMPING} refuses
/// locally — no packet, no cast bar, no GCD. Without it, vmangos *accepts* the cast (its
/// CheckCast moving-reject covers only autorepeat/sit-still spells, `Spell.cpp:5432`) and then
/// `Spell::update`'s 0.5-yd movement interrupt kills it — the start-then-cancel grief this gate
/// exists to prevent. The full reject condition, gate for gate:
///
/// - **entry**: `InterruptFlags & 0x1` (movement-interruptible — instants without it pass);
/// - **the movement word**: the WIRE `MovementFlags` layout (`[unit+0x9a8]+0x40`), mask
///   `0x200f` = forward|backward|strafe + JUMPING — turning and pitch are outside it, and so is
///   FALLINGFAR (`0x4000`; the client has NO falling/Stuck exemption — that's vmangos-only);
/// - **exemption**: an auto-repeat spell (`AttributesEx2 & 0x20` — Auto Shot, Shoot) never
///   refuses, whatever else it carries;
/// - **would movement matter**: a nonzero resolved cast time ([`super::Spells::cast_time_ms`]),
///   OR the [`AURA_INTERRUPT_MOVING_TURNING`] bits on the aura/channel interrupt columns — the
///   OR-arms are how a zero-cast-time *channel* is still refused at initiation.
///
/// An uncataloged spell passes (every record-read above needs the row; the ladder's other
/// data-driven legs — cooldown, GCD, range — share the disposition, and the server's own
/// interrupt stays the safety net). In the validator's order this sits after the mounted block
/// and before the shapeshift-form leg (`0x609e50`), which is where [`super::cast_send`]'s ladder
/// runs it.
pub(super) fn cast_moving_refusal(
    move_flags_word: u32,
    cast_time_ms: u32,
    spell: Option<&SpellDisplay>,
) -> bool {
    use crate::creature_anim::move_flags;
    // The verified 0x200f: ANY_MOVE (0xf) | FALLING (0x2000), in our identical wire layout.
    const MOVING_MASK: u32 = move_flags::ANY_MOVE | move_flags::FALLING;
    let Some(d) = spell else { return false };
    d.interrupt_flags & crate::spell::SPELL_INTERRUPT_MOVEMENT != 0
        && move_flags_word & MOVING_MASK != 0
        && !d.auto_repeat()
        && (cast_time_ms != 0
            || d.aura_interrupt_flags & AURA_INTERRUPT_MOVING_TURNING != 0
            || d.channel_interrupt_flags & AURA_INTERRUPT_MOVING_TURNING != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spell_with_range(range_index: u32, attributes: u32) -> SpellDisplay {
        SpellDisplay {
            range_index,
            attributes,
            ..Default::default()
        }
    }

    /// The pre-send refusal (`IsTargetInRange 0x6e47b0`'s two compares over the resolved
    /// bounds): Auto Shot's {8, 35} row + the unit reach pad — a point-blank target refuses
    /// TOO_CLOSE, a distant one OUT_OF_RANGE, the sweet spot passes; untestable inputs pass.
    #[test]
    fn cast_range_refusal_follows_the_two_compares() {
        let d = spell_with_range(114, 0);
        let auto_shot = SpellRange {
            min: 8.0,
            max: 35.0,
            flags: 0,
        };
        let reach = Some(1.5);
        // Both bounds carry the bare reach pad (self 1.5 + target 1.5): min = 11, max = 38.
        let refuse = |d2: f32| cast_range_refusal(&d, Some(&auto_shot), 1.5, reach, Some(d2));
        assert_eq!(refuse(3.0 * 3.0), Some(ERR_TOO_CLOSE));
        assert_eq!(refuse(20.0 * 20.0), None);
        assert_eq!(refuse(60.0 * 60.0), Some(ERR_OUT_OF_RANGE));

        // The regression: a min-0 ranged row (Fireball/Shadow Bolt) must pass point-blank —
        // its min never grows a reach pad — while the max compare still holds.
        let fireball = SpellRange {
            min: 0.0,
            max: 35.0,
            flags: 0,
        };
        let refuse = |d2: f32| cast_range_refusal(&d, Some(&fireball), 1.5, reach, Some(d2));
        assert_eq!(refuse(0.1), None);
        assert_eq!(refuse(60.0 * 60.0), Some(ERR_OUT_OF_RANGE));

        // A melee-family row has min 0 — never TOO_CLOSE, still OUT_OF_RANGE beyond reach.
        let melee = SpellRange {
            min: 0.0,
            max: 5.0,
            flags: 1,
        };
        let melee_spell = spell_with_range(2, 0);
        assert_eq!(
            cast_range_refusal(&melee_spell, Some(&melee), 1.5, reach, Some(0.1)),
            None
        );
        assert_eq!(
            cast_range_refusal(&melee_spell, Some(&melee), 1.5, reach, Some(15.0 * 15.0)),
            Some(ERR_OUT_OF_RANGE)
        );

        // No row / no distance: nothing to test locally — pass (the server judges).
        assert_eq!(cast_range_refusal(&d, None, 1.5, reach, Some(1.0)), None);
        assert_eq!(
            cast_range_refusal(&d, Some(&auto_shot), 1.5, reach, None),
            None
        );
    }

    /// The mounted refusal (`0x609c6c`): a live mount blocks unless Attributes carries the
    /// bit-24 exemption (`0x609c6f`); unmounted always passes; a missing record can claim no
    /// exemption, so the gate holds.
    #[test]
    fn cast_mounted_refusal_honors_the_bit24_exemption() {
        let plain = SpellDisplay::default();
        let exempt = SpellDisplay {
            attributes: 0x0100_0000,
            ..Default::default()
        };
        assert!(cast_mounted_refusal(true, Some(&plain)));
        assert!(!cast_mounted_refusal(true, Some(&exempt)));
        assert!(!cast_mounted_refusal(false, Some(&plain)));
        assert!(!cast_mounted_refusal(false, None));
        assert!(cast_mounted_refusal(true, None), "no record, no exemption");
    }

    /// The water refusal (`0x609d33–0x609de2`) — both faces of the environment gate and both
    /// arms of each face, on the real 5875 columns: Aquatic Form's `0x100` needs the SWIMMING
    /// bit set, the mount/Travel-Form/food `0x80` needs it clear, Fishing's ranks reach it
    /// through the CHANNEL column, and a spell carrying neither bit passes on both sides.
    #[test]
    fn cast_water_refusal_reads_both_sides_of_the_surface() {
        use crate::creature_anim::move_flags as mf;
        // Aquatic Form 1066: AuraInterruptFlags 0x100 — above-water cancels it, so the cast
        // needs water. This is ledger B176: on land it must refuse, and it must not send.
        let aquatic = SpellDisplay {
            aura_interrupt_flags: 0x100,
            ..Default::default()
        };
        assert_eq!(
            cast_water_refusal(0, Some(&aquatic)),
            Some(ERR_ONLY_UNDERWATER)
        );
        assert_eq!(cast_water_refusal(mf::SWIMMING, Some(&aquatic)), None);
        // Travel Form 783 / every mount: 0x80 — entering water cancels it, so it refuses the
        // other way round.
        let travel = SpellDisplay {
            aura_interrupt_flags: 0x80,
            ..Default::default()
        };
        assert_eq!(
            cast_water_refusal(mf::SWIMMING, Some(&travel)),
            Some(ERR_ONLY_ABOVEWATER)
        );
        assert_eq!(cast_water_refusal(0, Some(&travel)), None);
        // Food 433's shape (0x40080 = STANDING_CANCELS | UNDER_WATER_CANCELS) — the eating half
        // of ledger B155 rides the same 0x80 arm.
        let food = SpellDisplay {
            aura_interrupt_flags: 0x4_0080,
            ..Default::default()
        };
        assert_eq!(
            cast_water_refusal(mf::SWIMMING, Some(&food)),
            Some(ERR_ONLY_ABOVEWATER)
        );
        assert_eq!(cast_water_refusal(0, Some(&food)), None);
        // ARM B: a CHANNELED spell's requirement bits live in its channel
        // column. Fishing ranks 2–4's shape — AuraInterruptFlags zero, ChannelInterruptFlags
        // 0x3cac (which carries 0x80), AttributesEx 0x21004004 (IS_CHANNELED).
        let fishing_r2 = SpellDisplay {
            aura_interrupt_flags: 0,
            channel_interrupt_flags: 0x3cac,
            attributes_ex: 0x2100_4004,
            ..Default::default()
        };
        assert_eq!(
            cast_water_refusal(mf::SWIMMING, Some(&fishing_r2)),
            Some(ERR_ONLY_ABOVEWATER),
            "arm A is empty here — only the channel column refuses it"
        );
        assert_eq!(cast_water_refusal(0, Some(&fishing_r2)), None);
        // …and the `AttributesEx & 0x44` gate on arm B is load-bearing: Summon Baby Shark
        // 25849's shape carries the channel bit but does not channel, so it is NOT gated.
        let not_channeled = SpellDisplay {
            channel_interrupt_flags: 0x100,
            attributes_ex: 0,
            ..Default::default()
        };
        assert_eq!(cast_water_refusal(0, Some(&not_channeled)), None);
        assert_eq!(cast_water_refusal(mf::SWIMMING, Some(&not_channeled)), None);
        // Cat Form 768 / Bear Form 5487 carry neither bit: usable on both sides, always.
        let cat = SpellDisplay::default();
        assert_eq!(cast_water_refusal(0, Some(&cat)), None);
        assert_eq!(cast_water_refusal(mf::SWIMMING, Some(&cat)), None);
        // No record, nothing to read — the press passes, like every other data-driven rung.
        assert_eq!(cast_water_refusal(0, None), None);
        assert_eq!(cast_water_refusal(mf::SWIMMING, None), None);
    }

    /// The water gate against the **real 5875 Spell.dbc** — the census the whole gate rests on.
    /// The leg↔bit assignment is not a naming choice we could get backwards: exactly five rows
    /// in the shipped data carry the water-REQUIRED bit, and Aquatic Form is one of them, while
    /// the water-FORBIDDEN bit is a broad set led by the mounts, Travel Form and food/drink.
    /// Skips without client data.
    #[test]
    fn the_water_bits_split_the_5875_data_the_way_the_gate_assumes() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");

        // Every spell the client would refuse OUT of water, by id.
        let mut needs_water: Vec<u32> = catalog
            .iter()
            .filter(|(_, d)| d.aura_interrupt_flags & INTERRUPT_ABOVE_WATER != 0)
            .map(|(id, _)| id)
            .collect();
        needs_water.sort_unstable();
        assert_eq!(
            needs_water,
            vec![1066, 16455, 16456, 24346, 24347],
            "the water-required set: Aquatic Form, the Lava/Slime swim auras, Master Angler"
        );

        // The other face: broad, and led by exactly the things you cannot do mid-swim.
        let forbids_water = |id: u32| {
            catalog
                .get(id)
                .is_some_and(|d| d.aura_interrupt_flags & INTERRUPT_UNDER_WATER != 0)
        };
        assert!(forbids_water(783), "Travel Form");
        assert!(forbids_water(458), "Brown Horse");
        assert!(forbids_water(433), "Food");
        assert!(forbids_water(430), "Drink");
        assert!(forbids_water(818), "Basic Campfire");

        // ARM B's reason to exist, on the real rows. Fishing rank 1 carries the
        // bit in the AURA column; its own upgrades carry NOTHING there and reach the gate only
        // through the CHANNEL column. Arm A alone would refuse rank 1 mid-swim and let 2–4 fish.
        let swim = crate::creature_anim::move_flags::SWIMMING;
        assert!(forbids_water(7620), "Fishing rank 1 — arm A");
        for rank in [7731, 7732, 18248] {
            let d = catalog
                .get(rank)
                .unwrap_or_else(|| panic!("Fishing {rank} missing"));
            assert_eq!(d.aura_interrupt_flags, 0, "Fishing {rank}: arm A is empty");
            assert!(d.attributes_ex & ATTR_EX_CHANNELED != 0, "and it channels");
            assert_eq!(
                cast_water_refusal(swim, Some(d)),
                Some(ERR_ONLY_ABOVEWATER),
                "Fishing {rank} still refuses mid-swim, through the channel column"
            );
            assert_eq!(cast_water_refusal(0, Some(d)), None, "and fishes on shore");
        }

        // The forms that carry neither bit — usable on both sides, and the control that says the
        // gate is reading a real per-spell column and not a class-wide accident.
        for (id, name) in [(768, "Cat Form"), (5487, "Bear Form"), (2645, "Ghost Wolf")] {
            let d = catalog.get(id).unwrap_or_else(|| panic!("{name} missing"));
            assert_eq!(d.aura_interrupt_flags & 0x180, 0, "{name} is side-agnostic");
            assert_eq!(cast_water_refusal(0, Some(d)), None, "{name} on land");
            assert_eq!(cast_water_refusal(swim, Some(d)), None, "{name} swimming");
        }

        // And end to end, on the real rows: B176's exact press.
        let aquatic = catalog.get(1066).expect("Aquatic Form");
        assert_eq!(
            cast_water_refusal(0, Some(aquatic)),
            Some(ERR_ONLY_UNDERWATER),
            "on land, Aquatic Form refuses — the B176 report"
        );
        assert_eq!(
            cast_water_refusal(crate::creature_anim::move_flags::SWIMMING, Some(aquatic)),
            None,
            "swimming, it goes through"
        );
    }

    /// The moving refusal (`0x609de3`) — every leg of the byte-verified condition: the
    /// `InterruptFlags & 0x1` entry, the `0x200f` wire mask (turn/FALLINGFAR outside it,
    /// JUMPING inside), the auto-repeat exemption, and the "would movement matter" arms (cast
    /// time / aura / channel `0x18` bits).
    #[test]
    fn cast_moving_refusal_follows_the_validator_condition() {
        use crate::creature_anim::move_flags as mf;
        // Fireball's shape: ordinary timed cast (interrupt 0xf, nonzero cast time).
        let timed = SpellDisplay {
            interrupt_flags: 0xf,
            ..Default::default()
        };
        // Moving forward refuses; standing still doesn't.
        assert!(cast_moving_refusal(mf::FORWARD, 1500, Some(&timed)));
        assert!(!cast_moving_refusal(0, 1500, Some(&timed)));
        // Strafe and JUMPING are in the mask; turn and FALLING_FAR are not (`0x200f`).
        assert!(cast_moving_refusal(mf::STRAFE_LEFT, 1500, Some(&timed)));
        assert!(cast_moving_refusal(mf::FALLING, 1500, Some(&timed)));
        assert!(!cast_moving_refusal(mf::TURN_LEFT, 1500, Some(&timed)));
        assert!(!cast_moving_refusal(mf::FALLING_FAR, 1500, Some(&timed)));
        // An instant WITHOUT the movement interrupt bit passes (Fire Blast's shape)…
        let instant = SpellDisplay {
            interrupt_flags: 0xe,
            ..Default::default()
        };
        assert!(!cast_moving_refusal(mf::FORWARD, 0, Some(&instant)));
        // …and even WITH it, a zero cast time passes unless an 0x18 arm bites.
        assert!(!cast_moving_refusal(mf::FORWARD, 0, Some(&timed)));
        // Arcane Missiles' shape: zero cast time, but the channel column's moving bits refuse
        // at initiation (the OR-arm; 0x7c0c & 0x18 != 0).
        let channel = SpellDisplay {
            interrupt_flags: 0xf,
            channel_interrupt_flags: 0x7c0c,
            ..Default::default()
        };
        assert!(cast_moving_refusal(mf::FORWARD, 0, Some(&channel)));
        // The aura-column arm (food/drink sit-still bits).
        let sit_still = SpellDisplay {
            interrupt_flags: 0x1,
            aura_interrupt_flags: 0x18,
            ..Default::default()
        };
        assert!(cast_moving_refusal(mf::FORWARD, 0, Some(&sit_still)));
        // Auto-repeat (AttributesEx2 & 0x20) is unconditionally exempt — Auto Shot fires on
        // the run whatever its columns say.
        let auto_shot = SpellDisplay {
            interrupt_flags: 0x1,
            attributes_ex2: 0x20,
            aura_interrupt_flags: 0x18,
            ..Default::default()
        };
        assert!(!cast_moving_refusal(mf::FORWARD, 0, Some(&auto_shot)));
        // No record: nothing to read, the press passes (the server stays the net).
        assert!(!cast_moving_refusal(mf::FORWARD, 1500, None));
    }
}

#[cfg(test)]
mod cc_refusal_tests {
    use super::*;

    const STUNNED: u32 = 0x0004_0000;
    const SILENCED: u32 = 0x0000_2000;
    const PACIFIED: u32 = 0x0002_0000;
    const CONFUSED: u32 = 0x0040_0000;
    const FLEEING: u32 = 0x0080_0000;

    fn spell(prevention_type: u32) -> SpellDisplay {
        SpellDisplay {
            prevention_type,
            ..Default::default()
        }
    }

    /// A scan that finds no aura at all — `exempt: false, mechanic: 0`, so every arm falls to its
    /// OWN reason. This is the ordinary case: the exemption only ever fires for a caster wearing
    /// an immunity.
    fn no_auras() -> impl FnMut(&[u32]) -> benilla_formats::CcExemption {
        |_: &[u32]| benilla_formats::CcExemption::default()
    }

    /// The reason alone, dropping the mechanic — most of these tests are about which arm fires.
    fn reason_of(v: Option<(u8, Option<u32>)>) -> Option<u8> {
        v.map(|(r, _)| r)
    }

    /// A live caster, nobody else at the reins, no auras.
    fn cc(flags: u32, d: &SpellDisplay) -> Option<u8> {
        cast_cc_refusal(flags, Some(100), false, Some(d), &mut no_auras()).map(|(r, _)| r)
    }

    /// **The arms are not symmetric** (widened by 1925): stun, fear, charm and
    /// confuse refuse EVERY spell; silence and pacify only the rows whose `PreventionType` names
    /// them. Reading `UNIT_FLAG_SILENCED` as "no casting at all" — which our own preflight banner
    /// used to say — is the mistake this pins.
    #[test]
    fn crowd_control_refuses_by_prevention_type_except_the_unconditional_arms() {
        // Fireball-shaped (silence-preventable), Heroic-Strike-shaped (pacify-preventable), and
        // the auto-attack's neither — the three real values, pinned in `spell_catalog`.
        let cast = spell(1);
        let melee = spell(2);
        let neither = spell(0);

        // The four arms with NO per-spell gate: every row refuses.
        for (flags, reason) in [(STUNNED, 0x64), (FLEEING, 0x1e), (CONFUSED, 0x16)] {
            for d in [&cast, &melee, &neither] {
                assert_eq!(cc(flags, d), Some(reason), "flags {flags:#x}");
            }
        }
        // …and charm, which is not a flag at all but "somebody else holds the reins".
        assert_eq!(
            reason_of(cast_cc_refusal(
                0,
                Some(100),
                true,
                Some(&neither),
                &mut no_auras()
            )),
            Some(0x14)
        );

        // SILENCED takes the casts and leaves the rest alone.
        assert_eq!(cc(SILENCED, &cast), Some(0x60));
        assert_eq!(cc(SILENCED, &melee), None);
        assert_eq!(cc(SILENCED, &neither), None);

        // PACIFIED is the mirror.
        assert_eq!(cc(PACIFIED, &melee), Some(0x5a));
        assert_eq!(cc(PACIFIED, &cast), None);

        // Nothing up, nothing refused; and no record claims no prevention.
        assert_eq!(cc(0, &cast), None);
        assert_eq!(
            reason_of(cast_cc_refusal(
                SILENCED,
                Some(100),
                false,
                None,
                &mut no_auras()
            )),
            None
        );
        assert_eq!(
            reason_of(cast_cc_refusal(
                STUNNED,
                Some(100),
                false,
                None,
                &mut no_auras()
            )),
            Some(0x64),
            "the stun needs no record"
        );
    }

    /// **The order is the reference's, first match wins** — charm outranks the stun, the stun
    /// outranks everything below it. When several hold at once the player sees exactly one line,
    /// and which one is not arbitrary.
    #[test]
    fn the_arms_are_tried_in_the_references_order() {
        let cast = spell(1);
        // Charm is arm 1: it beats a simultaneous stun.
        assert_eq!(
            reason_of(cast_cc_refusal(
                STUNNED | SILENCED,
                Some(100),
                true,
                Some(&cast),
                &mut no_auras()
            )),
            Some(0x14)
        );
        // Stun is arm 2: it beats silence, pacify, fear and confuse.
        assert_eq!(
            cc(STUNNED | SILENCED | PACIFIED | FLEEING | CONFUSED, &cast),
            Some(0x64)
        );
        // Silence (3) beats fear (5) and confuse (6).
        assert_eq!(cc(SILENCED | FLEEING | CONFUSED, &cast), Some(0x60));
        // With the spell out of silence's reach, fear takes it before confuse.
        assert_eq!(cc(SILENCED | FLEEING | CONFUSED, &spell(0)), Some(0x1e));
    }

    /// **What the exemption does to an arm** — the three outcomes, at the arm.
    #[test]
    fn the_exemption_skips_an_arm_or_renames_its_refusal() {
        let cast = spell(1);
        let scan = |exempt: bool, mechanic: u32| {
            move |_: &[u32]| benilla_formats::CcExemption { exempt, mechanic }
        };

        // No aura of the arm's type: the arm's OWN reason, as everywhere else.
        assert_eq!(
            reason_of(cast_cc_refusal(
                STUNNED,
                Some(100),
                false,
                Some(&cast),
                &mut scan(false, 0)
            )),
            Some(0x64)
        );
        // A blocking aura the cast does NOT counter: the refusal survives but is renamed to
        // `0x8d` "Can't do that while %s", the mechanic naming the aura.
        assert_eq!(
            reason_of(cast_cc_refusal(
                STUNNED,
                Some(100),
                false,
                Some(&cast),
                &mut scan(false, 12)
            )),
            Some(0x8d)
        );
        // The cast grants immunity: the arm is SKIPPED and the cast goes out. This is the whole
        // point — Ice Block cast while stunned.
        assert_eq!(
            reason_of(cast_cc_refusal(
                STUNNED,
                Some(100),
                false,
                Some(&cast),
                &mut scan(true, 0)
            )),
            None
        );
        // …and skipping one arm does not skip the ladder: a silenced-and-stunned caster whose
        // spell counters only the stun still refuses on silence below it.
        assert_eq!(
            reason_of(cast_cc_refusal(
                STUNNED | SILENCED,
                Some(100),
                false,
                Some(&cast),
                &mut |types: &[u32]| {
                    // Exempt from the stun arm `{12}` only; the silence arm `{27, 12, 60}` is not.
                    benilla_formats::CcExemption {
                        exempt: types == [12],
                        mechanic: 0,
                    }
                }
            )),
            Some(0x60)
        );
    }

    /// **The mechanic rides out with the refusal** — the half that turns
    /// "Can't do that while %s" into a sentence. It is the ONE client-local refusal that carries
    /// an argument word, and it is `None` for every arm that fell to its own reason.
    #[test]
    fn the_renamed_refusal_carries_the_mechanic_and_the_others_carry_nothing() {
        let cast = spell(1);
        let scan = |exempt: bool, mechanic: u32| {
            move |_: &[u32]| benilla_formats::CcExemption { exempt, mechanic }
        };

        // Rejected by a blocking aura: reason `0x8d` AND the mechanic that names it.
        assert_eq!(
            cast_cc_refusal(STUNNED, Some(100), false, Some(&cast), &mut scan(false, 12)),
            Some((0x8d, Some(12)))
        );
        // No aura: the arm's own reason, and NO argument — the message has no `%s` to fill.
        assert_eq!(
            cast_cc_refusal(STUNNED, Some(100), false, Some(&cast), &mut scan(false, 0)),
            Some((0x64, None))
        );
        // Exempt: nothing at all.
        assert_eq!(
            cast_cc_refusal(STUNNED, Some(100), false, Some(&cast), &mut scan(true, 0)),
            None
        );
    }

    /// **A dead caster skips all six** (`0x60980d`'s `jle` on `UNIT_FIELD_HEALTH`) — a corpse is
    /// refused by an earlier rung, and reporting a stun over it would be the wrong line.
    #[test]
    fn a_dead_caster_takes_no_crowd_control_refusal() {
        let cast = spell(1);
        assert_eq!(
            reason_of(cast_cc_refusal(
                STUNNED,
                Some(0),
                false,
                Some(&cast),
                &mut no_auras()
            )),
            None
        );
        assert_eq!(
            reason_of(cast_cc_refusal(
                0,
                Some(0),
                true,
                Some(&cast),
                &mut no_auras()
            )),
            None
        );
        // Alive again, and the arm is back.
        assert_eq!(
            reason_of(cast_cc_refusal(
                STUNNED,
                Some(1),
                false,
                Some(&cast),
                &mut no_auras()
            )),
            Some(0x64)
        );
        // No health field at all is not death — the descriptor simply has not landed.
        assert_eq!(
            reason_of(cast_cc_refusal(
                STUNNED,
                None,
                false,
                Some(&cast),
                &mut no_auras()
            )),
            Some(0x64)
        );
    }
}
