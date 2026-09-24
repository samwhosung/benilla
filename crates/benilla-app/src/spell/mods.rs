//! The **talent spell-modifier tables** — the client-side half of every talent that cheapens a
//! spell, shortens its cast or cooldown, extends its range or radius, or lengthens its duration.
//!
//! Two `i32[64][29]` tables (`SPELLMOD_FLAT 0xcead60`, `SPELLMOD_PCT 0xcecb30`), one handler that
//! fills them (`Spell_C::HandleSetSpellModifier 0x6e9950`, registered for both
//! `SMSG_SET_FLAT_SPELL_MODIFIER` and `SMSG_SET_PCT_SPELL_MODIFIER` by `Spell_C::SystemInitialize
//! 0x6e7150`), and one reader that answers them (`GetSpellModifiers 0x6e6b30`). This module is
//! that machinery turned into the one place benilla asks "does a talent change this number?".
//!
//! **The index is `mask_bit * 29 + op`, in both directions, and it is the thing to get right.**
//! The writer multiplies its FIRST wire byte (`6e9993: imul eax,eax,0x1d`); the reader
//! strength-reduces the same address to `4*op + 0x74*i` over a loop counter bounded by
//! `cmp eax,0x40`, which forces the first factor to be the 0..63 `SpellFamilyFlags` **bit index**
//! and the second the 0..28 SpellModOp. The transposed form addresses 47% of the array
//! (`29*28 + 63 = 875` against 1856 cells) and makes writer and reader touch systematically
//! disjoint cells — every modifier silently dropped, nothing to see in a log. [`index`] is the
//! single expression both sides here go through, so they cannot drift apart.
//!
//! The server agrees, independently and in source: vmangos `Player::SendSpellMod`
//! (`Objects/Player.cpp:17815`) loops `for (int eff = 0; eff < 64; ++eff)` over the modifier's own
//! 64-bit mask and writes `uint8(eff)`, `uint8(mod->op)`, `int32(val)` for each set bit — first
//! byte the family-flag bit, second the op — with `MAX_SPELLMOD = 29` as the enum's size.
//!
//! **The value is absolute.** The store is a plain `mov`, and vmangos `Player::SendSpellMod` sends
//! one packet per set mask bit carrying that `(bit, op)` pair's total. Nothing accumulates, nothing
//! expires, and there is no talent-change clear: a respec is a fresh set of absolute cells.
//!
//! **The read is three conjunct gates and then a sum over all 64 bits** ([`SpellModifiers::apply`]
//! via [`SpellModifiers::modifiers`]): the spell's `SpellFamilyName` must be nonzero, must equal
//! the local player's own class family ([`SpellModifiers::class_family`]), and the spell must not
//! carry `AttributesEx3` bit 29. Then every set bit of the spell's 64-bit `SpellFamilyFlags`
//! contributes one cell from each table — **no early break**, and the high dword is live (322
//! shipped spells set more than one bit; the highest index in the file is 35).
//!
//! **The trap this module's shape exists to make unrepresentable.** `0x6e6b30` has four exits.
//! Three write `(flat, pct) = (0, 100)`; the fourth — *the loop ran and nothing accumulated*, which
//! is the COMMON case — leaves `pct = 0`, because it never reaches the `add ebx,0x64`. The
//! reference gets away with it because all three appliers gate on the returned boolean. A
//! re-implementation that hands a caller a bare `(flat, pct)` pair multiplies every unmodified
//! spell by **zero**: every spell would cost 0 mana. So [`SpellModifiers::modifiers`] returns
//! `Option<SpellMod>` — `None` IS that exit, and it is not a pair anyone can multiply by — and
//! [`SpellMod`] is private with [`SpellMod::apply`] its only verb. `apply` is the reference's own
//! integer applier `0x6e6af0`, boolean gate included.
//!
//! ## The 29 ops
//!
//! All 29 are stored: the stride is what the index arithmetic is made of, and nine of them have no
//! reader anywhere in the reference image while the handler will happily write them. The numbers
//! are VERIFIED (every one of the 35 call sites passes a literal); the names are vmangos
//! corroboration, and **op 24's name is INFERRED and suspect** — the client routes aura 65
//! (*casting speed*) to it while vmangos calls it `SPELL_BONUS_DAMAGE`, so key on the number.
//!
//! | op | name | benilla consumer |
//! |---|---|---|
//! | 0 | DAMAGE | — |
//! | 1 | DURATION | — |
//! | 2 | THREAT | — |
//! | 3 | ATTACK_POWER | — |
//! | 4 | CHARGES | — |
//! | 5 | RANGE | — |
//! | 6 | RADIUS | — |
//! | 7 | CRITICAL_CHANCE | never read by the reference |
//! | 8 | ALL_EFFECTS | — |
//! | 9 | NOT_LOSE_CASTING_TIME | never read |
//! | 10 | CASTING_TIME | — |
//! | 11 | COOLDOWN | — |
//! | 12 | SPEED | — |
//! | 13 | *(no enumerator)* | never read |
//! | **14** | **COST** | [`OP_COST`] — `spell::usable::power_cost` |
//! | 15 | CRIT_DAMAGE_BONUS | never read |
//! | 16 | RESIST_MISS_CHANCE | never read |
//! | 17 | JUMP_TARGETS | — |
//! | 18 | CHANCE_OF_SUCCESS | — |
//! | 19 | ACTIVATION_TIME | — |
//! | 20 | EFFECT_PAST_FIRST | never read |
//! | 21 | GLOBAL_COOLDOWN | — |
//! | 22 | DOT | — |
//! | 23 | HASTE | — |
//! | 24 | *(aura-65 op; name INFERRED)* | — |
//! | 25 | *(no enumerator)* | never read |
//! | 26 | *(no enumerator)* | never read |
//! | 27 | MULTIPLE_VALUE | — |
//! | 28 | RESIST_DISPEL_CHANCE | never read |
//!
//! ## Where this diverges from the reference, and why
//!
//! - **Both wire bytes are range-checked.** The reference checks neither: `field1 = 64, field2 = 19`
//!   writes at byte 7500 from the FLAT table's base — 76 past its end, straight onto `[0xcecaac]`,
//!   the class-family global the reader itself compares against — and `field1 = 65, field2 = 23`
//!   lands on element 0 of the PCT table. That is a buffer overrun, not a behaviour, and reproducing it is not
//!   fidelity. [`SpellModifiers::set`] drops the packet with a `warn!` naming both values.
//! - **The lifecycle hangs off the session edge, not a process global.** The reference clears both
//!   tables (and the class family) in `0x6e7150`, which runs once per **world-enter** — the Enter
//!   World click, through `CGlueMgr::Update 0x46b930` — and nowhere else; its teardown
//!   `Spell_C::Destroy 0x6e99e0` deliberately does not touch them. [`clear_on_world_enter`] is that
//!   same edge, `OnEnter(ClientState::InWorld)`.

use bevy::prelude::*;

use benilla_formats::SpellDisplay;

use crate::chr_classes::ChrClassTable;
use crate::net::{ObjectStore, SelfPlayer};

/// The number of SpellModOps — the table's column count, and therefore the index **stride**
/// (`imul eax,eax,0x1d`, and the reader's `add edx,0x74` = `29*4`).
const OPS: usize = 29;
/// The number of `SpellFamilyFlags` bits — the table's row count (`cmp eax,0x40`).
const BITS: usize = 64;
/// `0x740` dwords per table, exactly (`6e72ba: mov ecx,0x740` before each `rep stosd`), and
/// `29*63 + 28 == 1855` fills it with nothing left over — which is itself one of the three
/// discriminators that settle the index's factor roles.
const CELLS: usize = BITS * OPS;

/// `SPELLMOD_COST` — the one op benilla reads today ([`crate::spell::usable::power_cost`], the
/// reference's `GetPowerCost 0x6e31b0` @ `6e32e3`). The module header's table names the other 28.
pub(crate) const OP_COST: u8 = 14;

/// `AttributesEx3` bit 29 (`SPELL_ATTR_EX3_IGNORE_CASTER_MODIFIERS`, vmangos
/// `SpellDefines.h:940`) — the third gate, `0x6e6b52: test [SpellRec+0x24], 0x20000000`. A spell
/// carrying it takes no modifier at all, whatever the tables say.
const ATTR_EX3_IGNORE_CASTER_MODIFIERS: u32 = 0x2000_0000;

/// A cell's offset in either table. **The one expression**, so the writer and the reader can never
/// disagree about which of the two bytes is the row — the exact failure this system is famous for.
///
/// Callers have already range-checked both arguments ([`SpellModifiers::set`] refuses the packet,
/// [`SpellModifiers::modifiers`] refuses the op and iterates the bit itself), so this cannot leave
/// the array.
fn index(mask_bit: u8, op: u8) -> usize {
    usize::from(mask_bit) * OPS + usize::from(op)
}

/// What a talent does to one number: a flat term added first, then a percentage applied — the
/// reference's `(value + flat) * pct / 100`.
///
/// Private, and [`Self::apply`] is the only way to spend it, because a `(flat, pct)` pair loose in
/// the codebase is exactly the shape that multiplies an unmodified spell by zero (module header).
/// [`Default`] is the identity, matching the three gate-failure exits that write `(0, 100)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SpellMod {
    flat: i32,
    /// Already `max(0, 100 + Σ)` — the `+100` happens **once**, inside the reader (`6e6bb4`), and
    /// never in an applier. So this is a raw percentage multiplier: 100 is "unchanged".
    pct: i32,
}

impl Default for SpellMod {
    fn default() -> Self {
        SpellMod { flat: 0, pct: 100 }
    }
}

impl SpellMod {
    /// The reference's integer applier `0x6e6af0`: `*v = (*v + flat) * pct / 100`, with the flat
    /// term added **before** the percentage and the division truncating **toward zero** (C signed
    /// division — `6e6b14`'s magic-multiply/`sar 5`/`shr 31`/`add` sequence is exactly that).
    ///
    /// Computed in `i64` and saturated back, where the reference's 32-bit `imul` simply wraps: a
    /// wrap here would be a debug panic on a packet nobody sent, and no real modifier comes within
    /// several orders of magnitude of the boundary (a cost is a few hundred, a pct a few hundred).
    fn apply(self, value: i32) -> i32 {
        let scaled = (i64::from(value) + i64::from(self.flat)) * i64::from(self.pct) / 100;
        scaled.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    }
}

/// The two tables and the class family the gate compares against — benilla's `0xcead60` /
/// `0xcecb30` / `0xcecaac`.
///
/// Filled only by the wire (`spell::net::set_spell_modifier`), cleared only at world-enter
/// ([`clear_on_world_enter`]), and read live at every call site: like the reference, there is no
/// derived cache to invalidate.
#[derive(Resource)]
pub(crate) struct SpellModifiers {
    flat: [i32; CELLS],
    pct: [i32; CELLS],
    /// The local player's class spell-family (`ChrClasses.dbc` field 15). `0` until the avatar
    /// resolves, which is why the reader's first conjunct (`SpellFamilyName != 0`) exists at all:
    /// it covers exactly that window, where a family-0 spell would otherwise match.
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
    /// Store one cell, absolutely — `HandleSetSpellModifier 0x6e9950`'s whole body.
    ///
    /// The range check is benilla's, not the reference's (module header): a `mask_bit` of 64 is a
    /// write into the neighbouring global there, and we would rather drop a packet than reproduce
    /// an overrun. Nothing legitimate trips it — vmangos derives both bytes from its own enums.
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

    /// Set the gate's right-hand side — the reference's `0x6e6ca0`, whose only caller is the
    /// local-player create-finalise. [`track_class_family`] is that caller here.
    pub(crate) fn set_class_family(&mut self, family: u32) {
        self.class_family = family;
    }

    /// Drop everything — both tables and the class family, the exact set `0x6e7150` zeroes.
    pub(crate) fn clear(&mut self) {
        self.flat = [0; CELLS];
        self.pct = [0; CELLS];
        self.class_family = 0;
    }

    /// `GetSpellModifiers 0x6e6b30` for one spell and one op: the three gates, then the sum over
    /// every set family bit.
    ///
    /// `None` = "no modifier applies", covering **all four** of the reference's false exits — the
    /// three gate failures and the both-sums-are-zero one (`6e6ba8`/`6e6bad`). The caller uses its
    /// base value unchanged; see the module header for why this is an `Option` and not a pair.
    ///
    /// The op is range-checked here too. The reference does not check it either (`shl edx,2` on an
    /// unmasked argument) and is safe only because all 35 of its call sites pass a literal ≤ 27 —
    /// so do ours, and this costs one compare to make that structural rather than conventional.
    fn modifiers(&self, d: &SpellDisplay, op: u8) -> Option<SpellMod> {
        if d.spell_family == 0
            || d.spell_family != self.class_family
            || d.attributes_ex3 & ATTR_EX3_IGNORE_CASTER_MODIFIERS != 0
            || usize::from(op) >= OPS
        {
            return None;
        }
        // All 64 iterations, no early break: `n` set bits accumulate `n` cells from each table
        // (`6e6b8f`/`6e6b97` are `add [mem],reg`, and the only branch in the body is the skip).
        // `wrapping_add` because the reference's accumulators do, and because a server we do not
        // control should not be able to panic a debug build.
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
        // The `+100` and the clamp, once, here — never in an applier (`6e6bb4` … `6e6bc7`).
        Some(SpellMod {
            flat,
            pct: pct.wrapping_add(100).max(0),
        })
    }

    /// Put a spell's modifier for `op` onto `value`, or hand `value` back untouched — the
    /// reference's applier plus its `test al,al` gate, in one verb so a caller cannot skip the
    /// gate.
    pub(crate) fn apply(&self, d: &SpellDisplay, op: u8, value: i32) -> i32 {
        self.modifiers(d, op).map_or(value, |m| m.apply(value))
    }
}

/// Entering the world drops both tables and the class family — `Spell_C::SystemInitialize
/// 0x6e7150`'s two `rep stosd`s and its `[0xcecaac] = 0`, reached from
/// `CGlueMgr::Update 0x46b930` **once per world entry**, not once per process.
///
/// There is deliberately nothing else: no talent-change clear (the server re-sends absolute cells),
/// and no world-LEAVE clear (the reference's own teardown `0x6e99e0` never touches them, and the
/// next entry is what zeroes them).
pub(super) fn clear_on_world_enter(mut mods: ResMut<SpellModifiers>) {
    mods.clear();
}

/// Keep the gate's class family in step with the avatar — the reference's `0x6e6ca0`, whose sole
/// caller is the local-player create-finalise `0x5debcc`: it reads `UNIT_FIELD_BYTES_0` byte 1 and
/// stores `ChrClasses[class]` field 15.
///
/// One writer rather than a line wherever the avatar is built, for the reason
/// `char_select::publish_world_live` gives: a mirror a future transition can forget is worse than
/// the coupling it replaces. And it **never writes 0 back** — an absent avatar leaves the last
/// value standing, because a cross-map worldport drops the entity mid-session while everything the
/// server told us stays true (decision 0900's shape). Only the world-enter clear zeroes it.
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
    // Guarded so the write is a real edge: an unconditional store would mark the resource changed
    // every frame, and the tooltip feed rebuilds on exactly that signal.
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

    /// **The index, and the transposition that would pass a lazier test.** The cell the server
    /// wrote for `(bit 5, op 14)` must be the cell the reader reads for a spell with bit 5 set —
    /// and, crucially, must NOT be the one it reads for a spell with bit 14 set, which is where
    /// `op * 29 + bit` lands the same packet.
    ///
    /// Both halves are needed: a symmetric pair like `(14, 14)` agrees under either arithmetic, so
    /// this uses 5 and 14 and asserts the cross-read is empty.
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

        // The spell the packet is about: bit 5 set, op 14 read.
        assert_eq!(mods.apply(&mage(&[5]), OP_COST, 100), 70);
        // The spell the TRANSPOSED index would have served: bit 14 set. Under `op*29 + bit` the
        // write would have landed where this read looks, and this would be 70.
        assert_eq!(
            mods.apply(&mage(&[14]), OP_COST, 100),
            100,
            "bit 14 is not bit 5 — a transposed index would have crossed them"
        );
    }

    /// **The pct-0 trap.** The common path is "this spell has no talent on it": the reader's
    /// scanned-and-nothing-matched exit leaves `pct = 0` in the reference, and a consumer that
    /// ignored the boolean would multiply by it. Here the exit is `None`, so the base value
    /// survives — for a spell with no bits at all, for a spell whose bits are all zero cells, and
    /// for each of the three gate failures.
    #[test]
    fn nothing_applying_leaves_the_value_exactly_alone() {
        let mut mods = mage_tables();
        // A live modifier somewhere else in the table, so "the tables are empty" is not what is
        // being tested.
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

        // Gate 1: SpellFamilyName == 0 (every creature and item spell in the file).
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
        // An op with no cells is not a wipe either.
        assert_eq!(mods.apply(&mage(&[30]), 0, 100), 100);
    }

    /// The sum runs over **every** set bit of a 64-bit mask — both dwords — and the flat term is
    /// added before the percentage.
    #[test]
    fn every_set_bit_contributes_and_flat_lands_before_pct() {
        let mut mods = mage_tables();
        mods.set(true, 5, OP_COST, -10);
        mods.set(true, 35, OP_COST, -15);
        mods.set(false, 5, OP_COST, -20);
        mods.set(false, 35, OP_COST, -30);

        // Bit 35 alone: the HIGH dword, which a 32-bit mask read would drop entirely.
        assert_eq!(
            mods.apply(&mage(&[35]), OP_COST, 100),
            (100 - 15) * 70 / 100
        );
        // Both: flat −25 first, then 100 − 50 = 50 percent.
        assert_eq!(
            mods.apply(&mage(&[5, 35]), OP_COST, 100),
            (100 - 25) * 50 / 100
        );
        // Order matters — `value * pct / 100 + flat` would give 25, not 37.
        assert_eq!(mods.apply(&mage(&[5, 35]), OP_COST, 100), 37);
    }

    /// The division truncates **toward zero**, both signs — C's `/`, not a floor and not a round.
    #[test]
    fn the_division_truncates_toward_zero() {
        let mut mods = mage_tables();
        // 99 × 50% = 49.5 → 49, not 50.
        mods.set(false, 5, OP_COST, -50);
        assert_eq!(mods.apply(&mage(&[5]), OP_COST, 99), 49);

        // The negative side: −99 × 50% = −49.5 → −49 (toward zero), where a floor gives −50.
        assert_eq!(SpellMod { flat: 0, pct: 50 }.apply(-99), -49);
        // And the identity really is the identity.
        assert_eq!(SpellMod::default().apply(-7), -7);
    }

    /// The percentage sum is clamped at 0 — a −150% modifier zeroes the value, it does not invert
    /// it (`6e6bbc`'s `sets`/`dec`/`and`, on the SIGNED sum).
    #[test]
    fn a_percentage_below_minus_one_hundred_clamps_to_zero() {
        let mut mods = mage_tables();
        mods.set(false, 5, OP_COST, -150);
        assert_eq!(mods.apply(&mage(&[5]), OP_COST, 200), 0);
    }

    /// Out-of-range wire bytes are dropped, not stored — and dropped without disturbing the cell
    /// the reference's own overrun would have hit.
    #[test]
    fn an_out_of_range_packet_is_dropped() {
        let mut mods = mage_tables();
        // `(64, 19)` is the reference's documented overrun into the class-family global.
        mods.set(true, 64, 19, 999);
        // `(65, 23)` is the one that lands on PCT element 0 there.
        mods.set(true, 65, 23, 999);
        mods.set(true, 0, 29, 999);
        mods.set(true, 255, 255, 999);

        assert_eq!(mods.class_family, 3, "the class family is untouched");
        assert!(
            mods.flat.iter().chain(mods.pct.iter()).all(|&v| v == 0),
            "not one cell was written"
        );
    }

    /// World-enter drops everything — both tables and the family — which is the only clear there
    /// is.
    #[test]
    fn world_enter_clears_both_tables_and_the_family() {
        // A bare app with the world-enter clear alone: the state machine it hangs off is
        // all this needs.
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

    /// The three worked multi-bit spells from the real `Spell.dbc`, driven through the whole
    /// module: the same `(bit, op)` cells the server would send, read back through each spell's
    /// own shipped family mask. Cleanse 4987 is the one that matters — bits 12 **and** 33 — and it
    /// fails under any read that stops at 32 bits. Skips without client data.
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
        // Cure Poison 526 is a SHAMAN spell (family 11), and bit 35 is the only bit it sets — so
        // even with that exact cell live, the paladin's family gate refuses it. Same row, wrong
        // family: conjunct 2 is what keeps one class's talents off another's spells.
        mods.set(true, 35, OP_COST, -50);
        assert_eq!(
            mods.apply(cat.get(526).expect("Cure Poison"), OP_COST, 100),
            100
        );

        // A mage: Frostbolt 116, bits 5/19/20/30 — four cells, summed.
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
}
