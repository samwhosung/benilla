//! The talent spell-modifier tables: two `i32[64][29]` tables (`SPELLMOD_FLAT 0xcead60`,
//! `SPELLMOD_PCT 0xcecb30`), filled by `HandleSetSpellModifier 0x6e9950` from
//! `SMSG_SET_FLAT_SPELL_MODIFIER` / `SMSG_SET_PCT_SPELL_MODIFIER` and read by `GetSpellModifiers
//! 0x6e6b30`.
//!
//! The cell index is `mask_bit * 29 + op` on both sides: the first wire byte is the
//! `SpellFamilyFlags` bit (0..63), the second the SpellModOp (0..28) (writer `0x6e9993`, reader
//! bound `cmp eax,0x40`; vmangos `Player::SendSpellMod`, `Player.cpp:17815`). A transposed index
//! makes writer and reader touch disjoint cells, silently dropping every modifier. Each packet
//! carries the absolute total for its cell: nothing accumulates or expires.
//!
//! The read gates on `SpellFamilyName` nonzero and equal to the player's class family and on
//! `AttributesEx3` bit 29 clear, then sums one cell per set bit of all 64 family bits. Trap: the
//! reference's nothing-matched exit leaves pct 0 and relies on its callers testing the returned
//! boolean, so a bare `(flat, pct)` pair would zero every unmodified spell; hence
//! [`SpellModifiers::modifiers`] returns `Option` and [`SpellMod`] is private.
//!
//! All 29 ops are stored. Op 24 is the one the client routes aura 65 (casting speed) to,
//! whatever vmangos names it.

use bevy::prelude::*;

use benilla_formats::{SpellDisplay, SpellRange};

use crate::chr_classes::ChrClassTable;
use crate::net::{ObjectStore, SelfPlayer};

/// SpellModOps per row: the index stride (`imul eax,eax,0x1d`; the reader's `add edx,0x74`).
const OPS: usize = 29;
/// `SpellFamilyFlags` bits: the row count (`cmp eax,0x40`).
const BITS: usize = 64;
/// `0x740` dwords per table (`6e72ba: mov ecx,0x740` before each `rep stosd`).
const CELLS: usize = BITS * OPS;

/// `SPELLMOD_COST`, read by [`crate::spell::usable::power_cost`] (`GetPowerCost 0x6e31b0` at
/// `6e32e3`).
pub(crate) const OP_COST: u8 = 14;
/// `SPELLMOD_CASTING_TIME`, applied by `GetCastTime 0x6e3340` at `0x6e33b7`.
pub(crate) const OP_CAST_TIME: u8 = 10;
/// `GetMinMaxRange 0x6e3480` applies op 5 to its maximum at `0x6e3744`.
pub(crate) const OP_RANGE: u8 = 5;
/// `GetCurrentCastRadius 0x6e6350` applies op 6 before the reticle's 20-yard cap.
pub(crate) const OP_RADIUS: u8 = 6;
/// `StartGlobalCooldown 0x6e2de0` applies op 21 to `startRecoveryTime`.
pub(crate) const OP_GCD: u8 = 21;
/// `StartCooldown 0x6e2c60` and the tooltip apply op 11 to both recovery columns.
pub(crate) const OP_COOLDOWN: u8 = 11;

/// `SPELL_ATTR_EX3_IGNORE_CASTER_MODIFIERS` (vmangos `SpellDefines.h:940`): a spell carrying it
/// takes no modifier (`0x6e6b52`).
const ATTR_EX3_IGNORE_CASTER_MODIFIERS: u32 = 0x2000_0000;

/// A cell's offset in either table; the one expression writer and reader share. Callers
/// range-check both arguments.
fn index(mask_bit: u8, op: u8) -> usize {
    usize::from(mask_bit) * OPS + usize::from(op)
}

/// What a talent does to one number, `(value + flat) * pct / 100`; spent only through
/// [`Self::apply`]. [`Default`] is the identity `(0, 100)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SpellMod {
    flat: i32,
    /// `max(0, 100 + sum)`, the `+100` added once in the reader (`6e6bb4`): 100 is unchanged.
    pct: i32,
}

impl Default for SpellMod {
    fn default() -> Self {
        SpellMod { flat: 0, pct: 100 }
    }
}

impl SpellMod {
    /// The integer applier `0x6e6af0`: flat first, then the percentage, the division truncating
    /// toward zero (`6e6b14`). Saturates where the reference's 32-bit `imul` wraps; no real
    /// modifier comes near the boundary.
    fn apply(self, value: i32) -> i32 {
        let scaled = (i64::from(value) + i64::from(self.flat)) * i64::from(self.pct) / 100;
        scaled.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    }
}

/// The two tables and the class family the gate compares against (`0xcead60`, `0xcecb30`,
/// `0xcecaac`). Written only by the wire, cleared only at world-enter, read live.
#[derive(Clone, Resource)]
pub(crate) struct SpellModifiers {
    flat: [i32; CELLS],
    pct: [i32; CELLS],
    /// The local player's class spell-family (`ChrClasses.dbc` field 15); 0 until the avatar
    /// resolves, which the reader's `SpellFamilyName != 0` gate covers.
    class_family: u32,
}

impl Default for SpellModifiers {
    fn default() -> Self {
        SpellModifiers {
            flat: [0; CELLS],
            pct: [0; CELLS],
            class_family: 0,
        }
    }
}

impl SpellModifiers {
    /// Store one cell absolutely (`HandleSetSpellModifier 0x6e9950`). Deviation: an out-of-range
    /// byte drops the packet, because the reference's unchecked write overruns the table
    /// (`(64, 19)` lands on the class family at `0xcecaac`).
    pub(crate) fn set(&mut self, flat: bool, mask_bit: u8, op: u8, value: i32) {
        if usize::from(mask_bit) >= BITS || usize::from(op) >= OPS {
            warn!(
                "spell_mods: dropping an out-of-range {} modifier — mask bit {mask_bit} \
                 (0..=63), op {op} (0..=28), value {value}",
                if flat { "flat" } else { "pct" }
            );
            return;
        }
        let cell = index(mask_bit, op);
        if flat {
            self.flat[cell] = value;
        } else {
            self.pct[cell] = value;
        }
    }

    /// Set the gate's class family (`0x6e6ca0`).
    pub(crate) fn set_class_family(&mut self, family: u32) {
        self.class_family = family;
    }

    /// Zero both tables and the class family, the set `0x6e7150` zeroes.
    pub(crate) fn clear(&mut self) {
        self.flat = [0; CELLS];
        self.pct = [0; CELLS];
        self.class_family = 0;
    }

    /// The two family gates of `GetSpellModifiers 0x6e6b30` (`SpellFamilyName` nonzero and the
    /// class's, `AttributesEx3` bit 29 clear).
    fn gates_pass(&self, d: &SpellDisplay) -> bool {
        d.spell_family != 0
            && d.spell_family == self.class_family
            && d.attributes_ex3 & ATTR_EX3_IGNORE_CASTER_MODIFIERS == 0
    }

    /// The `SpellFamilyFlags` bits whose cells any read for `d` can sum, whatever the op: its
    /// whole mask past the gates, none before them. A change to another bit's cells cannot move
    /// a value modified for `d`; a change of the class family can, whatever the bit.
    pub(crate) fn read_bits(&self, d: &SpellDisplay) -> u64 {
        if self.gates_pass(d) {
            d.spell_family_flags
        } else {
            0
        }
    }

    /// What differs from `prev`: the `SpellFamilyFlags` bits with any changed cell, in either
    /// table and at any op, and whether the class family moved.
    pub(crate) fn diff(&self, prev: &SpellModifiers) -> ModsDiff {
        let bits = (0..BITS).fold(0u64, |bits, bit| {
            let row = bit * OPS..(bit + 1) * OPS;
            let same = self.flat[row.clone()] == prev.flat[row.clone()]
                && self.pct[row.clone()] == prev.pct[row];
            bits | u64::from(!same) << bit
        });
        ModsDiff {
            bits,
            class_family: self.class_family != prev.class_family,
        }
    }

    /// `GetSpellModifiers 0x6e6b30` for one spell and op. `None` covers all four of its false
    /// exits: the three gates and both sums zero (`6e6ba8`/`6e6bad`). The op range check is ours;
    /// the reference's call sites all pass a literal.
    fn modifiers(&self, d: &SpellDisplay, op: u8) -> Option<SpellMod> {
        if !self.gates_pass(d) || usize::from(op) >= OPS {
            return None;
        }
        // All 64 bits, no early break (`6e6b8f`/`6e6b97`); the accumulators wrap, as the
        // reference's do.
        let (mut flat, mut pct) = (0i32, 0i32);
        for bit in 0..BITS as u8 {
            if d.spell_family_flags >> bit & 1 == 0 {
                continue;
            }
            let cell = index(bit, op);
            flat = flat.wrapping_add(self.flat[cell]);
            pct = pct.wrapping_add(self.pct[cell]);
        }
        if flat == 0 && pct == 0 {
            return None;
        }
        // The `+100` and the clamp at 0, once, here (`6e6bb4`..`6e6bc7`).
        Some(SpellMod {
            flat,
            pct: pct.wrapping_add(100).max(0),
        })
    }

    /// Apply a spell's modifier for `op` to `value`: the applier with its `test al,al` gate.
    pub(crate) fn apply(&self, d: &SpellDisplay, op: u8, value: i32) -> i32 {
        self.modifiers(d, op).map_or(value, |m| m.apply(value))
    }

    /// The reference applies op 11 separately to RecoveryTime and to CategoryRecoveryTime
    /// after `0x6e2b60` adds the ranged attack time to the latter.
    pub(crate) fn spell_cooldowns(&self, d: &SpellDisplay, ranged_ms: u32) -> (u32, u32) {
        let recovery = self.apply(d, OP_COOLDOWN, d.recovery_ms as i32).max(0) as u32;
        let category_base = d.category_recovery_ms.saturating_add(ranged_ms);
        let category = self.apply(d, OP_COOLDOWN, category_base as i32).max(0) as u32;
        (recovery, category)
    }

    /// The reference's float applier `0x6e6bf0`: add the flat integer, then multiply by the
    /// summed percentage and its stored single-precision 0.01 constant.
    pub(crate) fn apply_float(&self, d: &SpellDisplay, op: u8, value: f32) -> f32 {
        self.modifiers(d, op).map_or(value, |m| {
            ((f64::from(value) + f64::from(m.flat)) * f64::from(m.pct) * f64::from(0.01f32)) as f32
        })
    }

    /// The software-float applier `0x6e6c30` that `GetEffectPoints` calls
    /// ([`benilla_formats::soft_modify`]).
    pub(crate) fn apply_soft(&self, d: &SpellDisplay, op: u8, value: f32) -> f32 {
        self.modifiers(d, op).map_or(value, |m| {
            benilla_formats::soft_modify(value, m.flat, m.pct)
        })
    }

    /// `GetMinMaxRange` modifies only the maximum at its common tail, after the moving bonus
    /// (`0x6e3648`-`0x6e36a2`, then op 5 at `0x6e3746`). The on-next-swing `Attributes & 0x404` arm
    /// returns before that tail (`0x6e350f`).
    pub(crate) fn min_max_range(
        &self,
        d: &SpellDisplay,
        row: Option<&SpellRange>,
        caster: benilla_formats::RangeUnit,
        targets: benilla_formats::RangeTargets,
    ) -> Option<(f32, f32)> {
        let (min, max) = benilla_formats::min_max_range(d, row, caster, targets)?;
        Some((
            min,
            if d.on_next_swing() {
                max
            } else {
                self.apply_float(d, OP_RANGE, max)
            },
        ))
    }
}

/// How one snapshot of the tables differs from an earlier one ([`SpellModifiers::diff`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModsDiff {
    /// Bit `b` set: a cell of family-flag bit `b` changed, in either table at any op.
    pub(crate) bits: u64,
    pub(crate) class_family: bool,
}

/// The description expander's view of the tables.
impl benilla_formats::SpellMods for SpellModifiers {
    fn apply_int(&self, d: &SpellDisplay, op: u8, value: i32) -> i32 {
        self.apply(d, op, value)
    }
    fn apply_float(&self, d: &SpellDisplay, op: u8, value: f32) -> f32 {
        SpellModifiers::apply_float(self, d, op, value)
    }
    fn apply_soft(&self, d: &SpellDisplay, op: u8, value: f32) -> f32 {
        SpellModifiers::apply_soft(self, d, op, value)
    }
}

/// Entering the world clears the tables and class family: `Spell_C::SystemInitialize 0x6e7150`,
/// run once per world entry from `CGlueMgr::Update 0x46b930`. The teardown `0x6e99e0` leaves
/// them, and there is no talent-change clear.
pub(super) fn clear_on_world_enter(mut mods: ResMut<SpellModifiers>) {
    mods.clear();
}

/// Keep the class family in step with the avatar (`0x6e6ca0`, called from `0x5debcc`: byte 1, the
/// class, of `UNIT_FIELD_BYTES_0` into `ChrClasses` field 15). An absent avatar keeps the last
/// value, as a cross-map worldport drops the entity mid-session; only world-enter zeroes it.
pub(super) fn track_class_family(
    mut mods: ResMut<SpellModifiers>,
    classes: Option<Res<ChrClassTable>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
) {
    let Some(classes) = classes else { return };
    let Some(class) = self_q.iter().next().and_then(|s| s.0.unit_class()) else {
        return;
    };
    let family = classes.0.spell_family(u32::from(class));
    // Guarded: the tooltip feed diffs the tables whenever the resource is marked changed.
    if mods.class_family != family {
        mods.set_class_family(family);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::char_select::ClientState;

    /// A mage spell (family 3) with the given bits set, against a mage player.
    fn mage(bits: &[u8]) -> SpellDisplay {
        SpellDisplay {
            spell_family: 3,
            spell_family_flags: bits.iter().fold(0u64, |m, b| m | 1 << b),
            ..Default::default()
        }
    }

    fn mage_tables() -> SpellModifiers {
        let mut mods = SpellModifiers::default();
        mods.set_class_family(3);
        mods
    }

    /// Uses the asymmetric pair (5, 14): a transposed index would serve bit 14's spell instead.
    #[test]
    fn the_index_is_mask_bit_times_29_plus_op() {
        assert_eq!(index(5, 14), 5 * 29 + 14);
        assert_ne!(
            index(5, 14),
            14 * 29 + 5,
            "the transposed form is a different cell"
        );

        let mut mods = mage_tables();
        mods.set(true, 5, OP_COST, -30);

        assert_eq!(mods.apply(&mage(&[5]), OP_COST, 100), 70);
        assert_eq!(
            mods.apply(&mage(&[14]), OP_COST, 100),
            100,
            "bit 14 is not bit 5 — a transposed index would have crossed them"
        );
    }

    /// The pct-0 trap: no bits, zero cells and each gate failure all leave the value unchanged.
    #[test]
    fn nothing_applying_leaves_the_value_exactly_alone() {
        let mut mods = mage_tables();
        // A live cell elsewhere, so the tables are not empty.
        mods.set(false, 30, OP_COST, -100);

        assert_eq!(mods.apply(&mage(&[]), OP_COST, 100), 100, "no family bits");
        assert_eq!(
            mods.apply(&mage(&[5]), OP_COST, 100),
            100,
            "bit 5's cells are zero"
        );
        assert_eq!(
            mods.apply(&mage(&[30]), OP_COST, 100),
            0,
            "and the live cell really is live — a −100% cost, not an inert table"
        );

        // Gate 1: SpellFamilyName == 0.
        let mut d = mage(&[30]);
        d.spell_family = 0;
        assert_eq!(mods.apply(&d, OP_COST, 100), 100);
        // Gate 2: another class's spell.
        let mut d = mage(&[30]);
        d.spell_family = 4;
        assert_eq!(mods.apply(&d, OP_COST, 100), 100);
        // Gate 3: IGNORE_CASTER_MODIFIERS.
        let mut d = mage(&[30]);
        d.attributes_ex3 = ATTR_EX3_IGNORE_CASTER_MODIFIERS;
        assert_eq!(mods.apply(&d, OP_COST, 100), 100);
        assert_eq!(mods.apply(&mage(&[30]), 0, 100), 100);
    }

    #[test]
    fn every_set_bit_contributes_and_flat_lands_before_pct() {
        let mut mods = mage_tables();
        mods.set(true, 5, OP_COST, -10);
        mods.set(true, 35, OP_COST, -15);
        mods.set(false, 5, OP_COST, -20);
        mods.set(false, 35, OP_COST, -30);

        // Bit 35 is in the high dword.
        assert_eq!(
            mods.apply(&mage(&[35]), OP_COST, 100),
            (100 - 15) * 70 / 100
        );
        assert_eq!(
            mods.apply(&mage(&[5, 35]), OP_COST, 100),
            (100 - 25) * 50 / 100
        );
        // `value * pct / 100 + flat` would give 25.
        assert_eq!(mods.apply(&mage(&[5, 35]), OP_COST, 100), 37);
    }

    #[test]
    fn the_division_truncates_toward_zero() {
        let mut mods = mage_tables();
        mods.set(false, 5, OP_COST, -50);
        assert_eq!(mods.apply(&mage(&[5]), OP_COST, 99), 49);

        // A floor would give -50.
        assert_eq!(SpellMod { flat: 0, pct: 50 }.apply(-99), -49);
        assert_eq!(SpellMod::default().apply(-7), -7);
    }

    /// The signed percentage sum clamps at 0 (`6e6bbc`).
    #[test]
    fn a_percentage_below_minus_one_hundred_clamps_to_zero() {
        let mut mods = mage_tables();
        mods.set(false, 5, OP_COST, -150);
        assert_eq!(mods.apply(&mage(&[5]), OP_COST, 200), 0);
    }

    #[test]
    fn an_out_of_range_packet_is_dropped() {
        let mut mods = mage_tables();
        // `(64, 19)` overruns onto the class family in the reference.
        mods.set(true, 64, 19, 999);
        // `(65, 23)` lands on PCT element 0 there.
        mods.set(true, 65, 23, 999);
        mods.set(true, 0, 29, 999);
        mods.set(true, 255, 255, 999);

        assert_eq!(mods.class_family, 3, "the class family is untouched");
        assert!(
            mods.flat.iter().chain(mods.pct.iter()).all(|&v| v == 0),
            "not one cell was written"
        );
    }

    #[test]
    fn world_enter_clears_both_tables_and_the_family() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::Login)
            .init_resource::<SpellModifiers>()
            .add_systems(OnEnter(ClientState::InWorld), clear_on_world_enter);

        {
            let mut mods = app.world_mut().resource_mut::<SpellModifiers>();
            mods.set_class_family(3);
            mods.set(true, 5, OP_COST, -30);
            mods.set(false, 35, OP_COST, -50);
            assert_eq!(mods.apply(&mage(&[5]), OP_COST, 100), 70);
        }

        app.world_mut()
            .resource_mut::<NextState<ClientState>>()
            .set(ClientState::InWorld);
        app.update();

        let mods = app.world().resource::<SpellModifiers>();
        assert_eq!(mods.class_family, 0);
        assert!(mods.flat.iter().chain(mods.pct.iter()).all(|&v| v == 0));
    }

    /// Real `Spell.dbc` family masks; Cleanse 4987 sets bits 12 and 33, across both dwords.
    #[test]
    fn real_spells_sum_their_own_family_bits() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell.dbc");

        // A paladin: Cleanse 4987 is family 10, bits 12 and 33.
        let mut mods = SpellModifiers::default();
        mods.set_class_family(10);
        mods.set(true, 12, OP_COST, -5);
        mods.set(true, 33, OP_COST, -7);
        mods.set(false, 33, OP_COST, -25);
        let cleanse = cat.get(4987).expect("Cleanse");
        assert_eq!(
            mods.apply(cleanse, OP_COST, 100),
            (100 - 12) * 75 / 100,
            "both dwords contribute"
        );
        // Cure Poison 526 is a shaman spell (family 11) setting only bit 35: the family gate
        // refuses it.
        mods.set(true, 35, OP_COST, -50);
        assert_eq!(
            mods.apply(cat.get(526).expect("Cure Poison"), OP_COST, 100),
            100
        );

        // A mage: Frostbolt 116, bits 5, 19, 20 and 30.
        let mut mods = SpellModifiers::default();
        mods.set_class_family(3);
        for bit in [5u8, 19, 20, 30] {
            mods.set(true, bit, OP_COST, -1);
        }
        assert_eq!(
            mods.apply(cat.get(116).expect("Frostbolt"), OP_COST, 100),
            96,
            "four set bits, four cells"
        );
    }

    /// `GetCastTime` applies op 10 after resolving the DBC row; an instant modifier lifts the
    /// moving-cast refusal for Frostbolt, while another class's spell keeps its cast time.
    #[test]
    fn cast_time_uses_the_live_modifier() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell.dbc");
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.cast_times =
            benilla_formats::load_spell_cast_times(&mut chain).expect("load SpellCastTimes.dbc");
        let frostbolt = catalog.get(116).expect("Frostbolt");
        let mut mods = mage_tables();
        let base = spells.cast_time_ms(frostbolt, 60, &mods);
        assert!(base > 0);
        mods.set(false, 5, OP_CAST_TIME, -100);
        assert_eq!(spells.cast_time_ms(frostbolt, 60, &mods), 0);
    }

    #[test]
    fn range_modifier_changes_only_the_maximum_after_reach() {
        let mut mods = mage_tables();
        let d = mage(&[5]);
        let row = SpellRange {
            min: 8.0,
            max: 35.0,
            flags: 0,
        };
        mods.set(false, 5, OP_RANGE, 20);
        let caster = benilla_formats::RangeUnit::still(1.5);
        let target = benilla_formats::RangeTargets {
            target: Some(benilla_formats::RangeUnit::still(1.5)),
            attack_target: None,
        };
        let (min, max) = mods.min_max_range(&d, Some(&row), caster, target).unwrap();
        assert_eq!(min, 11.0);
        assert!((max - 45.6).abs() < 0.001, "38 yards with +20% range");
        assert_eq!(mods.min_max_range(&d, None, caster, target), None);

        let on_next_swing = SpellDisplay {
            attributes: 0x404,
            ..d
        };
        assert_eq!(
            mods.min_max_range(&on_next_swing, Some(&row), caster, target),
            benilla_formats::min_max_range(&on_next_swing, Some(&row), caster, target),
            "the reference's early return bypasses op 5"
        );
    }

    /// The moving bonus is part of the max before op 5 scales it (`0x6e3648`-`0x6e36a2`, then
    /// `0x6e3746`): a +20% talent on the melee row of two running units scales 5 + 2.6667.
    #[test]
    fn range_modifier_scales_the_moving_bonus() {
        let mut mods = mage_tables();
        let d = mage(&[5]);
        let melee = SpellRange {
            min: 0.0,
            max: 5.0,
            flags: 1,
        };
        mods.set(false, 5, OP_RANGE, 20);
        let runner = benilla_formats::RangeUnit {
            motion: benilla_formats::UnitMotion {
                flags: 1,
                speed: 7.0,
                walk_speed: 2.5,
            },
            ..benilla_formats::RangeUnit::still(1.5)
        };
        let targets = benilla_formats::RangeTargets {
            target: Some(runner),
            attack_target: None,
        };
        let (_, max) = mods
            .min_max_range(&d, Some(&melee), runner, targets)
            .unwrap();
        let bonus = benilla_formats::MOVING_RANGE_BONUS;
        assert!(
            (max - (5.0 + bonus) * 1.2).abs() < 1e-4,
            "the modifier scales the bonused max, not the bare 5.0: {max}"
        );
    }
}
