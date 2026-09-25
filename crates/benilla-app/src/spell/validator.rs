//! The requirement validator `0x6094f0`: the press-time gates `TryCast 0x6e4b60` runs between
//! the target bind and the commit, plus the range compare `IsTargetInRange 0x6e47b0` that
//! `CanTargetUnit 0x6e4440` runs before it. Pure predicates answering the reference's reason codes,
//! called by [`super::cast_send`]'s ladder in its order. Only `TryCast` reaches `0x6094f0`, never
//! the greying walk `0x6e3d60`.

use benilla_formats::{SpellDisplay, SpellRange};

/// The range refusals `CanTargetUnit 0x6e4440` emits: "Out of range." and "Target too close".
pub(super) const ERR_OUT_OF_RANGE: u8 = 0x59;
pub(super) const ERR_TOO_CLOSE: u8 = 0x76;

/// The range refusal, before `ArmCast`/`SendCast`, so an out-of-range press never runs the commit
/// tail (the ranged sheath snap `0x6e5930` included): squared 3D distance against
/// [`benilla_formats::min_max_range`], beyond max² out of range, inside a nonzero min² too close.
/// No range row or no distance passes; the server judges.
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

/// `PreventionType` (`Spell.dbc` column 165): 1 silence, 2 pacify, 0 neither.
const PREVENTION_SILENCE: u32 = 1;
const PREVENTION_PACIFY: u32 = 2;

/// The crowd-control leg of `0x6094f0`, above its mounted block (`0x609c6c`), so a stunned
/// mounted caster is told about the stun. Six arms in the reference's order, first match wins:
///
/// | # | arm | gate | reason |
/// |---|---|---|---|
/// | 1 | CHARMED | `UNIT_FIELD_CHARMEDBY != 0` and the charmer is not us | `0x14` |
/// | 2 | STUNNED | bit 18, every spell | `0x64` |
/// | 3 | SILENCED | bit 13 and `PreventionType == 1` | `0x60` |
/// | 4 | PACIFIED | bit 17 and `PreventionType == 2` | `0x5a` |
/// | 5 | FLEEING | bit 23 | `0x1e` |
/// | 6 | CONFUSED | bit 22 | `0x16` |
///
/// A dead caster skips all six (`0x60980d`). Only `TryCast` calls this, so stun, silence and
/// pacify refuse on the press without greying a button; the greying walk's own copy of these
/// helpers (fear, confuse, charm) is not built. SILENCED reads `test ah,0x20`, invisible to a
/// dword-immediate scan.
///
/// Each arm first scans the caster's `UNIT_FIELD_AURA[0..47]` for auras of its own types (charm
/// `{6, 177, 2}`, stun `{12}`, silence `{27, 12, 60}`, pacify `{25, 12, 60}`, fear `{7}`, confuse
/// `{5}`) and asks [`benilla_formats::grants_immunity`] of each: a hit lifts the refusal; a named
/// blocking mechanic turns the reason into `0x8d`.
pub(crate) fn cast_cc_refusal(
    unit_flags: u32,
    health: Option<u32>,
    charmed_by_other: bool,
    spell: Option<&SpellDisplay>,
    exempt: &mut impl FnMut(&[u32]) -> benilla_formats::CcExemption,
) -> Option<(u8, Option<u32>)> {
    use crate::player::UNIT_FLAG_STUNNED;
    /// `UNIT_FIELD_FLAGS` bits 13, 17, 22 and 23 (vmangos `UnitDefines.h`).
    const UNIT_FLAG_SILENCED: u32 = 0x0000_2000;
    const UNIT_FLAG_PACIFIED: u32 = 0x0002_0000;
    const UNIT_FLAG_CONFUSED: u32 = 0x0040_0000;
    const UNIT_FLAG_FLEEING: u32 = 0x0080_0000;

    // A dead caster skips the leg (`0x60980d`); an earlier rung refuses a corpse.
    if health == Some(0) {
        return None;
    }
    let prevention = spell.map_or(0, |d| d.prevention_type);
    // An exempt scan skips the arm; otherwise `0x8d` when it named a mechanic, else the arm's own
    // reason.
    let mut arm = |aura_types: &[u32], own_reason: u8| -> Option<(u8, Option<u32>)> {
        let scan = exempt(aura_types);
        if scan.exempt {
            return None;
        }
        Some(if scan.mechanic != 0 {
            // The mechanic is the message's `%s`, the one local refusal with an argument word.
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

/// `SPELL_FAILED_PREVENTED_BY_MECHANIC`, "Can't do that while %s" with the blocking aura's
/// `SpellMechanic.dbc` name.
const REASON_PREVENTED_BY_MECHANIC: u8 = 0x8d;

/// The mounted refusal, `0x6094f0`'s mounted block (`0x609c6c`): a live
/// `UNIT_FIELD_MOUNTDISPLAYID` refuses with `0x39` "You are mounted" unless Attributes bit 24
/// (`0x01000000`, vmangos `SPELL_ATTR_ALLOW_WHILE_MOUNTED`, tested at `0x609c6f`) is set. A spell
/// with no record has no exemption. The mount-required gate (`0x53`, `0x609c05`) is not built, as
/// no 1.12 player spell needs it; it reads `0x40` in `AuraInterruptFlags` and, at `0x609c3a`, in
/// `ChannelInterruptFlags`.
pub(crate) fn cast_mounted_refusal(mounted: bool, spell: Option<&SpellDisplay>) -> bool {
    mounted && spell.is_none_or(|d| d.attributes & 0x0100_0000 == 0)
}

/// The interrupt-flag water pair: `0x80` cancels an aura on entering water, `0x100` on leaving it
/// (vmangos `AURA_INTERRUPT_UNDER_WATER_CANCELS` / `AURA_INTERRUPT_ABOVE_WATER_CANCELS`). The
/// validator refuses a cast whose aura could not survive the caster's current side.
const INTERRUPT_UNDER_WATER: u32 = 0x80;
const INTERRUPT_ABOVE_WATER: u32 = 0x100;

/// `AttributesEx & (IS_CHANNELED 0x4 | IS_SELF_CHANNELED 0x40)`, the gate on arm B
/// (`0x609d6a`/`0x609db1`): only a channeled spell has its channel column read, which keeps
/// Summon Baby Shark 25849 (`Channel 0x100`, `AttributesEx 0`) out.
const ATTR_EX_CHANNELED: u32 = 0x44;

/// `SPELL_FAILED_ONLY_ABOVEWATER`, "Cannot use while swimming".
pub(super) const ERR_ONLY_ABOVEWATER: u8 = 0x50;
/// `SPELL_FAILED_ONLY_UNDERWATER`, "Can only use while swimming".
pub(super) const ERR_ONLY_UNDERWATER: u8 = 0x58;

/// The water refusal, `0x6094f0`'s environment block `0x609d33`..`0x609de2`: after the
/// mounted, posture and day/night legs, before the moving gate, so a druid on land is refused
/// before the form gate `0x612480` runs. Each face reads its bit in two columns:
///
/// | reason | bit | arm A, `AuraInterruptFlags` | arm B, `ChannelInterruptFlags` |
/// |---|---|---|---|
/// | [`ERR_ONLY_UNDERWATER`] `0x58` | `0x100` | `0x609d36`, `0x609d46` | `0x609d6f`, `0x609d7b` |
/// | [`ERR_ONLY_ABOVEWATER`] `0x50` | `0x80` | `0x609da4`, `0x609dac` | `0x609db5`, `0x609dc2` |
///
/// Swimming is `[[caster+0x118]+0x40] & 0x200000`, the movement word the moving gate reads. Arm B
/// matters: Fishing ranks 2-4 (7731/7732/18248) carry the bit only in `ChannelInterruptFlags
/// 0x3cac`. No exemption skips this block. It must be local: vmangos gates water only for
/// `SPELL_AURA_MOUNTED` (`Spell.cpp:6379`) and grants Aquatic Form on dry land. An uncataloged
/// spell passes.
pub(super) fn cast_water_refusal(move_flags_word: u32, spell: Option<&SpellDisplay>) -> Option<u8> {
    let d = spell?;
    let swimming = move_flags_word & crate::creature_anim::move_flags::SWIMMING != 0;
    // Arm A always; arm B only for a channeled spell.
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

/// The MOVING|TURNING interrupt bits, tested on both `AuraInterruptFlags` (+0x58) and
/// `ChannelInterruptFlags` (+0x5c) (`0x609e0e`/`0x609e1c`).
const AURA_INTERRUPT_MOVING_TURNING: u32 = 0x18;

/// The moving refusal, `0x6094f0`'s moving block (`0x609de3`..`0x609e48`), the only local emitter
/// of `0x2e` "Can't do that while moving": no packet, no cast bar, no GCD. vmangos accepts the
/// cast (its moving reject covers only autorepeat and sit-still spells, `Spell.cpp:5437`) and
/// then interrupts it. It refuses when all hold:
///
/// - `InterruptFlags & 0x1`;
/// - the wire `MovementFlags` (`[unit+0x9a8]+0x40`) carry any of `0x200f`, forward, backward,
///   strafe or JUMPING; turning, pitch and FALLINGFAR are outside it, with no falling exemption;
/// - the spell is not auto-repeat (`AttributesEx2 & 0x20`);
/// - a nonzero resolved cast time, or the [`AURA_INTERRUPT_MOVING_TURNING`] bits in either
///   interrupt column, which refuses a zero-cast-time channel.
///
/// An uncataloged spell passes. It sits after the mounted block and before the shapeshift-form
/// leg (`0x609e50`).
pub(super) fn cast_moving_refusal(
    move_flags_word: u32,
    cast_time_ms: u32,
    spell: Option<&SpellDisplay>,
) -> bool {
    use crate::creature_anim::move_flags;
    // `0x200f`: ANY_MOVE (0xf) | FALLING (0x2000), the wire layout.
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

    /// `IsTargetInRange 0x6e47b0`'s two compares, on Auto Shot's {8, 35} row plus the reach pad.
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

        // A min-0 ranged row (Fireball) passes point-blank: its min takes no reach pad.
        let fireball = SpellRange {
            min: 0.0,
            max: 35.0,
            flags: 0,
        };
        let refuse = |d2: f32| cast_range_refusal(&d, Some(&fireball), 1.5, reach, Some(d2));
        assert_eq!(refuse(0.1), None);
        assert_eq!(refuse(60.0 * 60.0), Some(ERR_OUT_OF_RANGE));

        // A melee row has min 0: never too close.
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

        // No row or no distance passes.
        assert_eq!(cast_range_refusal(&d, None, 1.5, reach, Some(1.0)), None);
        assert_eq!(
            cast_range_refusal(&d, Some(&auto_shot), 1.5, reach, None),
            None
        );
    }

    /// The mounted refusal (`0x609c6c`) and its bit-24 exemption (`0x609c6f`).
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

    /// The water refusal's two faces and both arms of each, on the real rows' column values.
    #[test]
    fn cast_water_refusal_reads_both_sides_of_the_surface() {
        use crate::creature_anim::move_flags as mf;
        // Aquatic Form 1066: AuraInterruptFlags 0x100, so the cast needs water.
        let aquatic = SpellDisplay {
            aura_interrupt_flags: 0x100,
            ..Default::default()
        };
        assert_eq!(
            cast_water_refusal(0, Some(&aquatic)),
            Some(ERR_ONLY_UNDERWATER)
        );
        assert_eq!(cast_water_refusal(mf::SWIMMING, Some(&aquatic)), None);
        // Travel Form 783 and every mount: 0x80, refused in water.
        let travel = SpellDisplay {
            aura_interrupt_flags: 0x80,
            ..Default::default()
        };
        assert_eq!(
            cast_water_refusal(mf::SWIMMING, Some(&travel)),
            Some(ERR_ONLY_ABOVEWATER)
        );
        assert_eq!(cast_water_refusal(0, Some(&travel)), None);
        // Food 433: 0x40080, STANDING_CANCELS | UNDER_WATER_CANCELS.
        let food = SpellDisplay {
            aura_interrupt_flags: 0x4_0080,
            ..Default::default()
        };
        assert_eq!(
            cast_water_refusal(mf::SWIMMING, Some(&food)),
            Some(ERR_ONLY_ABOVEWATER)
        );
        assert_eq!(cast_water_refusal(0, Some(&food)), None);
        // Arm B: Fishing ranks 2-4, AuraInterruptFlags 0, ChannelInterruptFlags 0x3cac (with
        // 0x80), AttributesEx 0x21004004 (IS_CHANNELED).
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
        // Summon Baby Shark 25849 carries the channel bit but does not channel.
        let not_channeled = SpellDisplay {
            channel_interrupt_flags: 0x100,
            attributes_ex: 0,
            ..Default::default()
        };
        assert_eq!(cast_water_refusal(0, Some(&not_channeled)), None);
        assert_eq!(cast_water_refusal(mf::SWIMMING, Some(&not_channeled)), None);
        // Cat Form 768 and Bear Form 5487 carry neither bit.
        let cat = SpellDisplay::default();
        assert_eq!(cast_water_refusal(0, Some(&cat)), None);
        assert_eq!(cast_water_refusal(mf::SWIMMING, Some(&cat)), None);
        // No record passes.
        assert_eq!(cast_water_refusal(0, None), None);
        assert_eq!(cast_water_refusal(mf::SWIMMING, None), None);
    }

    /// The shipped `Spell.dbc`: exactly five rows need water, Aquatic Form among them, while the
    /// water-forbidden bit is broad (mounts, Travel Form, food and drink).
    #[test]
    fn the_water_bits_split_the_5875_data_the_way_the_gate_assumes() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");

        // Every spell refused out of water.
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

        // Fishing rank 1 carries the bit in the aura column, ranks 2-4 only in the channel column.
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

        // Forms carrying neither bit.
        for (id, name) in [(768, "Cat Form"), (5487, "Bear Form"), (2645, "Ghost Wolf")] {
            let d = catalog.get(id).unwrap_or_else(|| panic!("{name} missing"));
            assert_eq!(d.aura_interrupt_flags & 0x180, 0, "{name} is side-agnostic");
            assert_eq!(cast_water_refusal(0, Some(d)), None, "{name} on land");
            assert_eq!(cast_water_refusal(swim, Some(d)), None, "{name} swimming");
        }

        // Aquatic Form on land.
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

    /// Every condition of the moving refusal (`0x609de3`).
    #[test]
    fn cast_moving_refusal_follows_the_validator_condition() {
        use crate::creature_anim::move_flags as mf;
        // Fireball's shape: interrupt 0xf, a timed cast.
        let timed = SpellDisplay {
            interrupt_flags: 0xf,
            ..Default::default()
        };
        assert!(cast_moving_refusal(mf::FORWARD, 1500, Some(&timed)));
        assert!(!cast_moving_refusal(0, 1500, Some(&timed)));
        // Strafe and JUMPING are in `0x200f`; turn and FALLING_FAR are not.
        assert!(cast_moving_refusal(mf::STRAFE_LEFT, 1500, Some(&timed)));
        assert!(cast_moving_refusal(mf::FALLING, 1500, Some(&timed)));
        assert!(!cast_moving_refusal(mf::TURN_LEFT, 1500, Some(&timed)));
        assert!(!cast_moving_refusal(mf::FALLING_FAR, 1500, Some(&timed)));
        // An instant without the movement interrupt bit passes (Fire Blast's shape).
        let instant = SpellDisplay {
            interrupt_flags: 0xe,
            ..Default::default()
        };
        assert!(!cast_moving_refusal(mf::FORWARD, 0, Some(&instant)));
        // With it, a zero cast time passes unless an 0x18 bit is set.
        assert!(!cast_moving_refusal(mf::FORWARD, 0, Some(&timed)));
        // Arcane Missiles' shape: zero cast time, channel column 0x7c0c carries 0x18.
        let channel = SpellDisplay {
            interrupt_flags: 0xf,
            channel_interrupt_flags: 0x7c0c,
            ..Default::default()
        };
        assert!(cast_moving_refusal(mf::FORWARD, 0, Some(&channel)));
        // The aura-column arm (food and drink).
        let sit_still = SpellDisplay {
            interrupt_flags: 0x1,
            aura_interrupt_flags: 0x18,
            ..Default::default()
        };
        assert!(cast_moving_refusal(mf::FORWARD, 0, Some(&sit_still)));
        // Auto-repeat (AttributesEx2 & 0x20) is always exempt.
        let auto_shot = SpellDisplay {
            interrupt_flags: 0x1,
            attributes_ex2: 0x20,
            aura_interrupt_flags: 0x18,
            ..Default::default()
        };
        assert!(!cast_moving_refusal(mf::FORWARD, 0, Some(&auto_shot)));
        // No record passes.
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

    /// A scan that finds no aura: every arm answers its own reason.
    fn no_auras() -> impl FnMut(&[u32]) -> benilla_formats::CcExemption {
        |_: &[u32]| benilla_formats::CcExemption::default()
    }

    /// The reason alone, dropping the mechanic.
    fn reason_of(v: Option<(u8, Option<u32>)>) -> Option<u8> {
        v.map(|(r, _)| r)
    }

    /// A live caster, nobody else at the reins, no auras.
    fn cc(flags: u32, d: &SpellDisplay) -> Option<u8> {
        cast_cc_refusal(flags, Some(100), false, Some(d), &mut no_auras()).map(|(r, _)| r)
    }

    /// Stun, fear, charm and confuse refuse every spell; silence and pacify only the rows whose
    /// `PreventionType` names them.
    #[test]
    fn crowd_control_refuses_by_prevention_type_except_the_unconditional_arms() {
        // Fireball's shape (silence), Heroic Strike's (pacify), auto-attack's (neither).
        let cast = spell(1);
        let melee = spell(2);
        let neither = spell(0);

        // The arms with no per-spell gate refuse every row.
        for (flags, reason) in [(STUNNED, 0x64), (FLEEING, 0x1e), (CONFUSED, 0x16)] {
            for d in [&cast, &melee, &neither] {
                assert_eq!(cc(flags, d), Some(reason), "flags {flags:#x}");
            }
        }
        // Charm: somebody else holds the reins.
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

        assert_eq!(cc(SILENCED, &cast), Some(0x60));
        assert_eq!(cc(SILENCED, &melee), None);
        assert_eq!(cc(SILENCED, &neither), None);

        assert_eq!(cc(PACIFIED, &melee), Some(0x5a));
        assert_eq!(cc(PACIFIED, &cast), None);

        // No record claims no prevention.
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

    /// The reference's order, first match wins.
    #[test]
    fn the_arms_are_tried_in_the_references_order() {
        let cast = spell(1);
        // Charm (1) beats a stun.
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
        // Stun (2) beats everything below it.
        assert_eq!(
            cc(STUNNED | SILENCED | PACIFIED | FLEEING | CONFUSED, &cast),
            Some(0x64)
        );
        // Silence (3) beats fear (5) and confuse (6).
        assert_eq!(cc(SILENCED | FLEEING | CONFUSED, &cast), Some(0x60));
        // Out of silence's reach, fear takes it before confuse.
        assert_eq!(cc(SILENCED | FLEEING | CONFUSED, &spell(0)), Some(0x1e));
    }

    /// The exemption's three outcomes at an arm.
    #[test]
    fn the_exemption_skips_an_arm_or_renames_its_refusal() {
        let cast = spell(1);
        let scan = |exempt: bool, mechanic: u32| {
            move |_: &[u32]| benilla_formats::CcExemption { exempt, mechanic }
        };

        // No aura of the arm's type: its own reason.
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
        // A blocking aura the cast does not counter: renamed to `0x8d`.
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
        // The cast grants immunity (Ice Block while stunned): the arm is skipped.
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
        // Skipping the stun arm still leaves silence below it.
        assert_eq!(
            reason_of(cast_cc_refusal(
                STUNNED | SILENCED,
                Some(100),
                false,
                Some(&cast),
                &mut |types: &[u32]| {
                    // Exempt from the stun arm `{12}` only.
                    benilla_formats::CcExemption {
                        exempt: types == [12],
                        mechanic: 0,
                    }
                }
            )),
            Some(0x60)
        );
    }

    /// Only the renamed refusal carries the mechanic.
    #[test]
    fn the_renamed_refusal_carries_the_mechanic_and_the_others_carry_nothing() {
        let cast = spell(1);
        let scan = |exempt: bool, mechanic: u32| {
            move |_: &[u32]| benilla_formats::CcExemption { exempt, mechanic }
        };

        assert_eq!(
            cast_cc_refusal(STUNNED, Some(100), false, Some(&cast), &mut scan(false, 12)),
            Some((0x8d, Some(12)))
        );
        assert_eq!(
            cast_cc_refusal(STUNNED, Some(100), false, Some(&cast), &mut scan(false, 0)),
            Some((0x64, None))
        );
        assert_eq!(
            cast_cc_refusal(STUNNED, Some(100), false, Some(&cast), &mut scan(true, 0)),
            None
        );
    }

    /// A dead caster skips all six arms (`0x60980d`'s `jle` on `UNIT_FIELD_HEALTH`).
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
        // No health field yet is not death.
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
