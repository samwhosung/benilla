//! The requirement validator `0x6094f0`: the press-time gates `TryCast 0x6e4b60` runs between
//! the target bind and the commit, plus the range compare `IsTargetInRange 0x6e47b0` that
//! `CanTargetUnit 0x6e4440` runs before it. Pure predicates answering the reference's reason codes,
//! called by [`super::cast_send`]'s ladder in its order: the cooldown and power rungs there, the
//! rest through [`caster_refusal`]. Only `TryCast` reaches `0x6094f0`, never the greying walk
//! `0x6e3d60`.

use super::SpellModifiers;
use benilla_formats::{CcExemption, RangeUnit, SpellDisplay, SpellRange};
use benilla_protocol::ObjectFields;

/// The range refusals `CanTargetUnit 0x6e4440` emits: "Out of range." and "Target too close".
pub(super) const ERR_OUT_OF_RANGE: u8 = 0x59;
pub(super) const ERR_TOO_CLOSE: u8 = 0x76;

/// The range refusal, before `ArmCast`/`SendCast`, so an out-of-range press never runs the commit
/// tail (the ranged sheath snap `0x6e5930` included): squared 3D distance against
/// [`benilla_formats::min_max_range`], beyond max² out of range, inside a nonzero min² too close.
/// `target` is the bound unit, the target `IsTargetInRange 0x6e47b0` hands `GetMinMaxRange`
/// (`0x6e47ca`): its reach pads a ranged row and its motion, with the caster's, earns the moving
/// bonus. The function looks the auto-attack target up only when that target is no unit, which a
/// bound unit never is, so none is passed. No range row or no distance passes; the server judges.
pub(super) fn cast_range_refusal(
    spell: &SpellDisplay,
    row: Option<&SpellRange>,
    caster: RangeUnit,
    target: Option<RangeUnit>,
    dist_sq: Option<f32>,
    mods: &SpellModifiers,
) -> Option<u8> {
    let targets = benilla_formats::RangeTargets {
        target,
        attack_target: None,
    };
    let (min, max) = mods.min_max_range(spell, row, caster, targets)?;
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
/// [`caster_refusal`] runs it for a living caster only (`0x60980d`). Only `TryCast` calls this, so
/// stun, silence and pacify refuse on the press without greying a button; the greying walk's own
/// copy of these helpers (fear, confuse, charm) is not built. SILENCED reads `test ah,0x20`,
/// invisible to a dword-immediate scan.
///
/// Each arm first scans the caster's `UNIT_FIELD_AURA[0..47]` for auras of its own types (charm
/// `{6, 177, 2}`, stun `{12}`, silence `{27, 12, 60}`, pacify `{25, 12, 60}`, fear `{7}`, confuse
/// `{5}`), `scan` answering for one type ([`benilla_formats::cc_exemption`], `0x6e9ca0`) and
/// [`cc_exemption_over`] joining them: an exemption lifts the refusal; a named blocking mechanic
/// turns the reason into `0x8d`.
pub(crate) fn cast_cc_refusal(
    unit_flags: u32,
    charmed_by_other: bool,
    spell: Option<&SpellDisplay>,
    scan: &mut dyn FnMut(u32) -> CcExemption,
) -> Option<Refusal> {
    use crate::player::UNIT_FLAG_STUNNED;
    /// `UNIT_FIELD_FLAGS` bits 13, 17, 22 and 23 (vmangos `UnitDefines.h`).
    const UNIT_FLAG_SILENCED: u32 = 0x0000_2000;
    const UNIT_FLAG_PACIFIED: u32 = 0x0002_0000;
    const UNIT_FLAG_CONFUSED: u32 = 0x0040_0000;
    const UNIT_FLAG_FLEEING: u32 = 0x0080_0000;

    let prevention = spell.map_or(0, |d| d.prevention_type);
    // An exempt scan skips the arm; otherwise `0x8d` when it named a mechanic, else the arm's own
    // reason.
    let mut arm = |aura_types: &[u32], own_reason: u8| -> Option<Refusal> {
        let over = cc_exemption_over(aura_types, scan);
        if over.exempt {
            return None;
        }
        Some(if over.mechanic != 0 {
            // The mechanic is the message's `%s`, the one local refusal with an argument word.
            Refusal {
                reason: REASON_PREVENTED_BY_MECHANIC,
                arg: Some(over.mechanic),
            }
        } else {
            Refusal::of(own_reason)
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

/// The wrappers over the scan `0x6e9ca0` (`0x6e9c80`, `0x6e9e60`, `0x6e9ec0`, `0x6e9f20`,
/// `0x6e9f40`, `0x6e9f60`): one scan per aura type in order, exempt as soon as any scan exempts
/// (`0x6e9e84`, `0x6e9e95`, `0x6e9ea6`). The mechanic word is cleared once at entry (`0x6e9e72`)
/// and each refusing scan writes its nonzero mechanic over it (`0x6e9d4c`, `0x6e9d5f`), so the
/// last one names the refusal.
pub(crate) fn cc_exemption_over(
    aura_types: &[u32],
    scan: &mut dyn FnMut(u32) -> CcExemption,
) -> CcExemption {
    let mut mechanic = 0;
    for &aura_type in aura_types {
        let one = scan(aura_type);
        if one.exempt {
            return CcExemption {
                exempt: true,
                mechanic,
            };
        }
        if one.mechanic != 0 {
            mechanic = one.mechanic;
        }
    }
    CcExemption {
        exempt: false,
        mechanic,
    }
}

/// `SPELL_FAILED_PREVENTED_BY_MECHANIC`, "Can't do that while %s" with the blocking aura's
/// `SpellMechanic.dbc` name.
const REASON_PREVENTED_BY_MECHANIC: u8 = 0x8d;

/// The mounted refusal, `0x6094f0`'s mounted block (`0x609c6c`): a
/// `UNIT_FIELD_MOUNTDISPLAYID` above 0 (`0x609c84 jle`) refuses with `0x39` "You are mounted"
/// unless Attributes bit 24 (`0x01000000`, vmangos `SPELL_ATTR_ALLOW_WHILE_MOUNTED`, tested at
/// `0x609c6f`) is set. A spell with no record has no exemption.
pub(crate) fn cast_mounted_refusal(mounted: bool, spell: Option<&SpellDisplay>) -> bool {
    mounted && spell.is_none_or(|d| d.attributes & 0x0100_0000 == 0)
}

/// Whether a spell's aura or channel carries an interrupt bit, as the validator's stance and water
/// rungs read it: arm A in `AuraInterruptFlags` (`+0x58`) always, arm B in
/// `ChannelInterruptFlags` (`+0x5c`) only for a channeled spell ([`ATTR_EX_CHANNELED`]).
fn interrupt_flag_requires(d: &SpellDisplay, bit: u32) -> bool {
    d.aura_interrupt_flags & bit != 0
        || (d.attributes_ex & ATTR_EX_CHANNELED != 0 && d.channel_interrupt_flags & bit != 0)
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
    if !swimming && interrupt_flag_requires(d, INTERRUPT_ABOVE_WATER) {
        return Some(ERR_ONLY_UNDERWATER);
    }
    if swimming && interrupt_flag_requires(d, INTERRUPT_UNDER_WATER) {
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

/// One refusal of the validator: the reason `ReportCastFailure 0x6e1a00` is passed, and the
/// argument word it pushes first, which only the crowd-control rename carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) reason: u8,
    pub(crate) arg: Option<u32>,
}

impl Refusal {
    const fn of(reason: u8) -> Self {
        Self { reason, arg: None }
    }
}

/// The caster as the validator's rungs past its power test read it: the active player, or the
/// pet bar's unit the attempt passes (`0x6e4f3b`).
pub(crate) struct CasterState<'a> {
    /// Its descriptor.
    pub(crate) fields: &'a ObjectFields,
    /// The active player's guid (`0x60994d` asks `0x468550`), which the charm arm compares the
    /// caster's charmer with (`0x60995e`).
    pub(crate) guid: Option<u64>,
    /// `GetStandState`, the caster's vtable slot 41 (`0x609cad`): the active player's local state
    /// (`0x5ed570`), a unit's descriptor byte (`0x60be50`).
    pub(crate) stand_state: u8,
    /// The sheathe state `[unit+0xd40]`: 0 away, 1 melee, 2 ranged (`0x609ba9`).
    pub(crate) sheathe_state: u8,
    /// The mover's flags `[[unit+0x118]+0x40]`, which the water and moving rungs read.
    pub(crate) move_flags: u32,
    /// `IsDaytime 0x60a050` on the game clock.
    pub(crate) daytime: bool,
    /// `0x612cc0`: the entity outdoors test `0x6706f0` on the caster's world node.
    pub(crate) outdoors: bool,
}

/// What the rungs read beyond the record and the caster: the cast time the moving rung asks
/// `0x6e3340` for, whether the caster's form counts as a stance (`0x612480`), and the player's
/// standing with a faction (`GetFactionStanding 0x4d6370`).
pub(crate) struct SpellInputs<'a> {
    pub(crate) cast_time_ms: u32,
    pub(crate) form_is_stance: bool,
    pub(crate) standing: &'a dyn Fn(u32) -> i32,
}

/// `AURA_INTERRUPT_DISMOUNT_CANCELS` and `AURA_INTERRUPT_SHEATHING_CANCELS` (vmangos
/// `SpellDefines.h:545`, `:548`): an aura that ends on a dismount needs a mount, one that ends on
/// sheathing a drawn weapon.
const INTERRUPT_DISMOUNT: u32 = 0x40;
const INTERRUPT_SHEATHING: u32 = 0x200;

/// The `Attributes` bits the rungs read (vmangos `SpellDefines.h:804-820`).
const ATTR_DAYTIME_ONLY: u32 = 0x0000_1000;
const ATTR_NIGHT_ONLY: u32 = 0x0000_2000;
const ATTR_ONLY_INDOORS: u32 = 0x0000_4000;
const ATTR_ONLY_OUTDOORS: u32 = 0x0000_8000;
const ATTR_ONLY_STEALTHED: u32 = 0x0002_0000;
const ATTR_ALLOW_WHILE_SITTING: u32 = 0x0800_0000;
const ATTR_NOT_IN_COMBAT_ONLY_PEACEFUL: u32 = 0x1000_0000;
/// `SPELL_ATTR_EX_DISMISS_PET_FIRST` (vmangos `SpellDefines.h:832`): a summon or charm a held pet
/// does not refuse (`0x60972c`).
const ATTR_EX_DISMISS_PET_FIRST: u32 = 0x1;

/// The summoning effects the loop tests (`0x6096f6`-`0x609706`): `SPELL_EFFECT_SUMMON` 28,
/// `_TAMECREATURE` 55 and `_SUMMON_PET` 56 (vmangos `SpellDefines.h:239`, `:266`, `:267`), and the
/// aura types `SPELL_AURA_MOD_CHARM` 6 and `_MOD_POSSESS` 2 (`SpellAuraDefines.h:64`, `:47`), the
/// aura column read whatever the effect (`0x609708`-`0x609713`).
const SUMMON_EFFECTS: [u32; 3] = [28, 55, 56];
const CHARM_AURAS: [u32; 2] = [6, 2];
/// `TARGET_PET`, the implicit target the pet test matches in either column (`0x60973e`).
const TARGET_PET: u32 = 5;

/// The rungs of `0x6094f0` after its cooldown and power tests, in its order, the first that fails
/// answering. `scan` is the crowd-control scan `0x6e9ca0` for one aura type. A spell with no record
/// takes no rung that needs one of its bits to apply.
///
/// | rung | reason | test |
/// |---|---|---|
/// | each effect (`0x6096c3`-`0x6097fc`) | `4`/`5` | a summon or charm with `UNIT_FIELD_CHARM`, else `_SUMMON`, held, unless [`ATTR_EX_DISMISS_PET_FIRST`] |
/// | | `0x4c` | an effect aimed at the pet with neither held |
/// | living caster (`0x60980d`) | `0x61` | a player logging out, `PLAYER_FIELD_BYTES` bit `0x4` (`0x60982b`) |
/// | | crowd control | [`cast_cc_refusal`] |
/// | `0x609ba1` | `0x41` | [`INTERRUPT_SHEATHING`] with the weapon away, `[unit+0xd40] == 0` |
/// | `0x609c05` | `0x53` | [`INTERRUPT_DISMOUNT`] with `UNIT_FIELD_MOUNTDISPLAYID` at 0 or below |
/// | `0x609c6c` | `0x39` | [`cast_mounted_refusal`] |
/// | `0x609ca2` | `0x3e` | not standing (`0x609cad`), unless [`ATTR_ALLOW_WHILE_SITTING`] |
/// | `0x609cd3` | `0x51` | [`ATTR_DAYTIME_ONLY`] at night (`IsDaytime 0x60a050`) |
/// | `0x609d03` | `0x54` | [`ATTR_NIGHT_ONLY`] by day |
/// | `0x609d33` | `0x58`/`0x50` | [`cast_water_refusal`] |
/// | `0x609de3` | `0x2e` | [`cast_moving_refusal`] |
/// | `0x609e50` | the form's | [`SpellDisplay::form_refusal`] |
/// | `0x609e76` | `0x57` | [`ATTR_ONLY_STEALTHED`] without `UNIT_FIELD_BYTES_1` byte 3 bit `0x2` |
/// | `0x609eae` | `0` | [`ATTR_NOT_IN_COMBAT_ONLY_PEACEFUL`] in combat |
/// | `0x609ee4` | `0x12` | `CasterAuraState` n without `UNIT_FIELD_AURASTATE` bit n - 1 |
/// | `0x609f1f` | `0x8f` | `MinFactionId` standing below `MinReputation`'s floor (`0x80928c`) |
/// | `0x609f59` | `0x52` | [`ATTR_ONLY_INDOORS`] outdoors (`0x612cc0`) |
/// | | `0x4b`/`0x55` | [`ATTR_ONLY_OUTDOORS`] indoors: `0x4b` when an effect applies the mount aura |
///
/// Not built: the learn rungs, which show a line of their own through `0x496720` (`0x6098a2`,
/// `0x609920`) or ask a pet's knowledge (`0x609904`); and the tame rung `0x17` (`0x609fd7`), which
/// shows its pet tame failure through `0x6e6a20` first.
pub(crate) fn caster_refusal(
    spell: Option<&SpellDisplay>,
    caster: &CasterState,
    inputs: &SpellInputs,
    scan: &mut dyn FnMut(u32) -> CcExemption,
) -> Option<Refusal> {
    let fields = caster.fields;
    if let Some(d) = spell {
        for i in 0..3 {
            let summons = SUMMON_EFFECTS.contains(&d.effects[i])
                || CHARM_AURAS.contains(&d.effect_apply_aura[i]);
            if summons
                && fields.unit_pet_guid().is_some()
                && d.attributes_ex & ATTR_EX_DISMISS_PET_FIRST == 0
            {
                // 4 "You already control a charmed creature" while charming, else 5 (`0x6098ce`).
                return Some(Refusal::of(if fields.unit_charm().is_some() {
                    4
                } else {
                    5
                }));
            }
            let at_pet = d.effect_implicit_target_a[i] == TARGET_PET
                || d.effect_implicit_target_b[i] == TARGET_PET;
            if at_pet && fields.unit_pet_guid().is_none() {
                return Some(Refusal::of(0x4c));
            }
        }
    }
    // A caster with health 0 or below skips the control block (`0x60980d jle`); no health field
    // yet is not death.
    let alive = fields.unit_health().is_none_or(|h| h as i32 > 0);
    if alive {
        if fields.player_logging_out() {
            return Some(Refusal::of(0x61));
        }
        // The charm arm refuses only when someone else holds the charm (`0x60995e`).
        let charmed_by_other = fields
            .unit_charmed_by()
            .is_some_and(|charmer| Some(charmer) != caster.guid);
        if let Some(r) = cast_cc_refusal(fields.unit_flags(), charmed_by_other, spell, scan) {
            return Some(r);
        }
    }
    let attributes = spell.map_or(0, |d| d.attributes);
    let mount_display = fields.unit_mount_display_id() as i32;
    if let Some(d) = spell {
        if interrupt_flag_requires(d, INTERRUPT_SHEATHING) && caster.sheathe_state == 0 {
            return Some(Refusal::of(0x41));
        }
        // `0x609c18 jg`: a signed display id.
        if interrupt_flag_requires(d, INTERRUPT_DISMOUNT) && mount_display <= 0 {
            return Some(Refusal::of(0x53));
        }
    }
    if cast_mounted_refusal(mount_display > 0, spell) {
        return Some(Refusal::of(0x39));
    }
    if attributes & ATTR_ALLOW_WHILE_SITTING == 0 && caster.stand_state != 0 {
        return Some(Refusal::of(0x3e));
    }
    if attributes & ATTR_DAYTIME_ONLY != 0 && !caster.daytime {
        return Some(Refusal::of(0x51));
    }
    if attributes & ATTR_NIGHT_ONLY != 0 && caster.daytime {
        return Some(Refusal::of(0x54));
    }
    if let Some(reason) = cast_water_refusal(caster.move_flags, spell) {
        return Some(Refusal::of(reason));
    }
    if cast_moving_refusal(caster.move_flags, inputs.cast_time_ms, spell) {
        return Some(Refusal::of(0x2e));
    }
    let d = spell?;
    if let Some(refusal) = d.form_refusal(fields.unit_shapeshift_form(), inputs.form_is_stance) {
        return Some(Refusal::of(refusal.reason()));
    }
    if attributes & ATTR_ONLY_STEALTHED != 0 && !fields.unit_is_stealthed() {
        return Some(Refusal::of(0x57));
    }
    if attributes & ATTR_NOT_IN_COMBAT_ONLY_PEACEFUL != 0
        && fields.unit_flags() & crate::player::UNIT_FLAG_IN_COMBAT != 0
    {
        // `SPELL_FAILED_AFFECTING_COMBAT`, reason 0 (`0x609ed0 xor dl,dl`).
        return Some(Refusal::of(0));
    }
    // `shl edx,cl` (`0x609ef9`) takes the count mod 32.
    if d.caster_aura_state != 0
        && fields.unit_aura_state() & 1u32.wrapping_shl(d.caster_aura_state - 1) == 0
    {
        return Some(Refusal::of(0x12));
    }
    if d.min_faction_id != 0 {
        // A signed compare against the rank's floor (`0x609f3b jge`); Spell.dbc ranks run 0 to 7.
        let floor = crate::ui_reputation::RANK_BOUNDS
            .get(d.min_reputation as usize)
            .copied()
            .unwrap_or(i32::MAX);
        if (inputs.standing)(d.min_faction_id) < floor {
            return Some(Refusal::of(0x8f));
        }
    }
    // One question for either bit (`0x609f5c`, `0x609f63`).
    if attributes & (ATTR_ONLY_INDOORS | ATTR_ONLY_OUTDOORS) != 0 {
        if caster.outdoors {
            if attributes & ATTR_ONLY_INDOORS != 0 {
                return Some(Refusal::of(0x52));
            }
        } else if attributes & ATTR_ONLY_OUTDOORS != 0 {
            // `SPELL_FAILED_NO_MOUNTS_ALLOWED` for an effect applying `SPELL_AURA_MOUNTED` (6 and
            // 78 in the same effect, `0x609fa1`, `0x609fa6`), else `ONLY_OUTDOORS`.
            let mounts = (0..3).any(|i| d.effects[i] == 6 && d.effect_apply_aura[i] == 78);
            return Some(Refusal::of(if mounts { 0x4b } else { 0x55 }));
        }
    }
    None
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
        let mods = SpellModifiers::default();
        let d = spell_with_range(114, 0);
        let auto_shot = SpellRange {
            min: 8.0,
            max: 35.0,
            flags: 0,
        };
        let me = RangeUnit::still(1.5);
        let reach = Some(RangeUnit::still(1.5));
        // Both bounds carry the bare reach pad (self 1.5 + target 1.5): min = 11, max = 38.
        let refuse = |d2: f32| cast_range_refusal(&d, Some(&auto_shot), me, reach, Some(d2), &mods);
        assert_eq!(refuse(3.0 * 3.0), Some(ERR_TOO_CLOSE));
        assert_eq!(refuse(20.0 * 20.0), None);
        assert_eq!(refuse(60.0 * 60.0), Some(ERR_OUT_OF_RANGE));

        // A min-0 ranged row (Fireball) passes point-blank: its min takes no reach pad.
        let fireball = SpellRange {
            min: 0.0,
            max: 35.0,
            flags: 0,
        };
        let refuse = |d2: f32| cast_range_refusal(&d, Some(&fireball), me, reach, Some(d2), &mods);
        assert_eq!(refuse(0.1), None);
        assert_eq!(refuse(60.0 * 60.0), Some(ERR_OUT_OF_RANGE));
        // The bound target is the pad's second reach (`0x6e47ca`): 35 + 1.5 + 1.5 = 38.
        assert_eq!(refuse(37.0 * 37.0), None);
        assert_eq!(refuse(39.0 * 39.0), Some(ERR_OUT_OF_RANGE));
        // With no unit bound the row stands as it is: 35.
        assert_eq!(
            cast_range_refusal(&d, Some(&fireball), me, None, Some(37.0 * 37.0), &mods),
            Some(ERR_OUT_OF_RANGE)
        );

        // A melee row has min 0: never too close.
        let melee = SpellRange {
            min: 0.0,
            max: 5.0,
            flags: 1,
        };
        let melee_spell = spell_with_range(2, 0);
        assert_eq!(
            cast_range_refusal(&melee_spell, Some(&melee), me, reach, Some(0.1), &mods),
            None
        );
        assert_eq!(
            cast_range_refusal(
                &melee_spell,
                Some(&melee),
                me,
                reach,
                Some(15.0 * 15.0),
                &mods
            ),
            Some(ERR_OUT_OF_RANGE)
        );

        // No row or no distance passes.
        assert_eq!(
            cast_range_refusal(&d, None, me, reach, Some(1.0), &mods),
            None
        );
        assert_eq!(
            cast_range_refusal(&d, Some(&auto_shot), me, reach, None, &mods),
            None
        );
    }

    /// A warrior running after a running mob presses Hamstring (the melee row) at 7 yards:
    /// `GetMinMaxRange` adds the moving bonus to the 5.0 floor (`0x6e3648`-`0x6e36a2`), so the
    /// press goes out to 7.667, and a standing pair is refused at the same distance.
    #[test]
    fn cast_range_refusal_lets_two_running_units_reach_the_moving_bonus() {
        let mods = SpellModifiers::default();
        let hamstring = spell_with_range(2, 0);
        let melee = SpellRange {
            min: 0.0,
            max: 5.0,
            flags: 1,
        };
        let running = RangeUnit {
            motion: benilla_formats::UnitMotion {
                flags: 1,
                speed: 7.0,
                walk_speed: 2.5,
            },
            ..RangeUnit::still(1.5)
        };
        let press = |caster, target, yards: f32| {
            cast_range_refusal(
                &hamstring,
                Some(&melee),
                caster,
                Some(target),
                Some(yards * yards),
                &mods,
            )
        };
        let standing = RangeUnit::still(1.5);
        assert_eq!(press(running, running, 7.0), None);
        assert_eq!(press(running, running, 7.5), None);
        assert_eq!(press(running, running, 8.0), Some(ERR_OUT_OF_RANGE));
        assert_eq!(press(standing, standing, 7.0), Some(ERR_OUT_OF_RANGE));
        assert_eq!(press(running, standing, 7.0), Some(ERR_OUT_OF_RANGE));
        assert_eq!(press(standing, running, 7.0), Some(ERR_OUT_OF_RANGE));
        assert_eq!(press(standing, standing, 5.0), None);
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
            "on land, Aquatic Form refuses: it is underwater-only"
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
#[path = "validator_tests.rs"]
mod caster_tests;
