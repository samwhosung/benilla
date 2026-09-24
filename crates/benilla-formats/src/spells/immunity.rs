//! The crowd-control **exemption** — `0x6e9ca0`'s aura scan and `0x6e9d70`'s immunity matcher.
//!
//! Each of the six crowd-control arms in the cast validator asks this before it refuses: *does one
//! of the caster's own auras grant immunity to the thing blocking me?* The answer is not a flag
//! lookup — it is a join between two `Spell.dbc` records, the spell **being cast** and the aura
//! **doing the blocking**, and the question it really asks is "is the spell I am casting an
//! immunity that covers this aura?" — Ice Block cast while stunned, and its kin.
//!
//! **Its ordinary answer is "no", and that is the point.** The matcher's first gate is
//! `AttributesEx` bit 15 on the *cast*, which is clear on essentially every ordinary spell — so a
//! normal cast falls out immediately and the arm refuses. What the scan changes is *which message*
//! the refusal carries, and the rare real exemption.

use super::SpellDisplay;

/// `SPELL_EFFECT_APPLY_AURA` — the only effect kind the matcher's loop considers (`6e9da0`).
const SPELL_EFFECT_APPLY_AURA: u32 = 6;

/// `AttributesEx` bit 15 on the CAST spell, which must be **set** for any immunity to be
/// considered (`6e9d81`). INFERRED `SPELL_ATTR_EX_DISPEL_AURAS_ON_IMMUNITY`.
const ATTR_EX_DISPELS_ON_IMMUNITY: u32 = 0x0000_8000;

/// `Attributes` bit 29 on the BLOCKING aura, which must be **clear** (`6e9d89`). INFERRED
/// `SPELL_ATTR_UNAFFECTED_BY_INVULNERABILITY` — an aura that says "no immunity touches me".
const ATTR_UNAFFECTED_BY_INVULNERABILITY: u32 = 0x2000_0000;

/// `AttributesEx2` bit 26 on the BLOCKING aura — the school arm's own veto (`6e9dcd`).
const ATTR_EX2_NO_SCHOOL_IMMUNITY: u32 = 0x0400_0000;

/// The four immunity aura types the matcher switches on, all on the **cast** spell's own effects.
/// Its window is 38..=77 (`6e9dac`'s `sub 0x26 ; cmp 0x27`); every value inside it that is not one
/// of these four falls through to the next effect.
const AURA_STATE_IMMUNITY: u32 = 38;
const AURA_SCHOOL_IMMUNITY: u32 = 39;
const AURA_DISPEL_IMMUNITY: u32 = 41;
const AURA_MECHANIC_IMMUNITY: u32 = 77;

/// **The matcher** (`0x6e9d70`) — does `cast` grant immunity to effect `aura_effect` of `aura`?
///
/// Two head gates, then a four-way switch over the cast's own `APPLY_AURA` effects:
///
/// | cast `EffectApplyAuraName[j]` | accepts when |
/// |---|---|
/// | 38 STATE | `cast.EffectMiscValue[j] == aura.EffectApplyAuraName[i]` |
/// | 39 SCHOOL | the aura lacks `AttributesEx2` bit 26 **and** `cast.EffectMiscValue[j] & (1 << aura.School)` |
/// | 41 DISPEL | `cast.EffectMiscValue[j] == aura.Dispel` |
/// | 77 MECHANIC | `cast.EffectMiscValue[j] == aura.Mechanic` **or** `== aura.EffectMechanic[i]` |
///
/// Note the asymmetry the school arm carries: `School` is an **index** and `EffectMiscValue` a
/// **mask**, so it shifts before testing. Getting that backwards silently makes every school
/// immunity match school 0 and nothing else.
pub fn grants_immunity(cast: &SpellDisplay, aura: &SpellDisplay, aura_effect: usize) -> bool {
    if cast.attributes_ex & ATTR_EX_DISPELS_ON_IMMUNITY == 0 {
        return false;
    }
    if aura.attributes & ATTR_UNAFFECTED_BY_INVULNERABILITY != 0 {
        return false;
    }
    let i = aura_effect.min(2);
    (0..3).any(|j| {
        if cast.effects[j] != SPELL_EFFECT_APPLY_AURA {
            return false;
        }
        // `EffectMiscValue` is signed in the DBC; every comparison here is against an unsigned id
        // or a mask, so a negative value simply matches nothing.
        let misc = cast.effect_misc_value[j];
        let misc_u = u32::try_from(misc).unwrap_or(u32::MAX);
        match cast.effect_apply_aura[j] {
            AURA_STATE_IMMUNITY => misc_u == aura.effect_apply_aura[i],
            AURA_SCHOOL_IMMUNITY => {
                aura.attributes_ex2 & ATTR_EX2_NO_SCHOOL_IMMUNITY == 0
                    && aura.school < 32
                    && misc_u & (1 << aura.school) != 0
            }
            AURA_DISPEL_IMMUNITY => misc_u == aura.dispel,
            AURA_MECHANIC_IMMUNITY => misc_u == aura.mechanic || misc_u == aura.effect_mechanic[i],
            _ => false,
        }
    })
}

/// What one arm's scan concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CcExemption {
    /// **The arm is skipped and the cast proceeds.** True only when at least one aura of the
    /// wanted type was found *and* every matching effect the scan reached was accepted.
    pub exempt: bool,
    /// The blocking aura's mechanic, when one was rejected — `EffectMechanic[i]` if non-zero, else
    /// the aura's `Mechanic`. `0` means nothing was written, and the arm refuses with its **own**
    /// reason rather than the generic "Can't do that while %s".
    pub mechanic: u32,
}

/// **The scanner** (`0x6e9ca0`) — walk the caster's aura slots for an aura of `wanted_aura_type`
/// and ask [`grants_immunity`] about each matching effect.
///
/// Three outcomes, and the middle one is the whole reason the arms carry two reason codes:
///
/// - **exempt** — a matching aura existed and every matching effect was accepted;
/// - **not exempt, mechanic written** — a matching aura was *rejected*; the refusal names its
///   mechanic (the `0x8d` line). The scan **stops at the first rejection**;
/// - **not exempt, mechanic 0** — no aura of that type at all.
///
/// `aura_ids` must be the **raw** `UNIT_FIELD_AURA` slot ids: the reference does not consult
/// `UNIT_FIELD_AURAFLAGS` here, does not skip an "inactive" slot, and reads no duration, stack or
/// caster state. Any non-zero, in-range id counts — which is why this takes ids rather than the
/// filtered slot view the buff bar uses.
pub fn cc_exemption<'a>(
    cast: &SpellDisplay,
    aura_ids: impl IntoIterator<Item = u32>,
    wanted_aura_type: u32,
    spell: impl Fn(u32) -> Option<&'a SpellDisplay>,
) -> CcExemption {
    let mut found = false;
    for id in aura_ids {
        if id == 0 {
            continue;
        }
        let Some(aura) = spell(id) else {
            continue; // out of range for the id table — the reference's own bound test
        };
        for i in 0..3 {
            if aura.effect_apply_aura[i] != wanted_aura_type {
                continue;
            }
            found = true;
            if !grants_immunity(cast, aura, i) {
                let mechanic = if aura.effect_mechanic[i] != 0 {
                    aura.effect_mechanic[i]
                } else {
                    aura.mechanic
                };
                return CcExemption {
                    exempt: false,
                    mechanic,
                };
            }
        }
    }
    CcExemption {
        exempt: found,
        mechanic: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blocking aura: one effect of `aura_type`, with a mechanic and a school.
    fn aura(aura_type: u32, mechanic: u32, effect_mechanic: u32, school: u32) -> SpellDisplay {
        let mut d = SpellDisplay {
            mechanic,
            school,
            ..Default::default()
        };
        d.effect_apply_aura[0] = aura_type;
        d.effect_mechanic[0] = effect_mechanic;
        d
    }

    /// An immunity spell: one `APPLY_AURA` effect of `immunity_type` carrying `misc`.
    fn immunity(immunity_type: u32, misc: i32) -> SpellDisplay {
        let mut d = SpellDisplay {
            attributes_ex: ATTR_EX_DISPELS_ON_IMMUNITY,
            ..Default::default()
        };
        d.effects[0] = SPELL_EFFECT_APPLY_AURA;
        d.effect_apply_aura[0] = immunity_type;
        d.effect_misc_value[0] = misc;
        d
    }

    /// **The two head gates**, which are why an ordinary cast is never exempt: the CAST must carry
    /// `AttributesEx` bit 15, and the blocking AURA must not carry `Attributes` bit 29.
    #[test]
    fn an_ordinary_cast_grants_no_immunity() {
        let stun = aura(12, 12, 0, 0);
        // A mechanic immunity that matches — but with bit 15 clear it never even looks.
        let mut ordinary = immunity(AURA_MECHANIC_IMMUNITY, 12);
        ordinary.attributes_ex = 0;
        assert!(!grants_immunity(&ordinary, &stun, 0));

        // With the bit set it matches…
        let real = immunity(AURA_MECHANIC_IMMUNITY, 12);
        assert!(grants_immunity(&real, &stun, 0));

        // …unless the aura declares itself unaffected by invulnerability.
        let stubborn = SpellDisplay {
            attributes: ATTR_UNAFFECTED_BY_INVULNERABILITY,
            ..aura(12, 12, 0, 0)
        };
        assert!(!grants_immunity(&real, &stubborn, 0));
    }

    /// The four arms, each on its own field of the blocking aura.
    #[test]
    fn each_immunity_arm_reads_its_own_field() {
        // 77 MECHANIC — matches the aura's `Mechanic` OR its per-effect `EffectMechanic[i]`.
        assert!(grants_immunity(
            &immunity(AURA_MECHANIC_IMMUNITY, 12),
            &aura(12, 12, 0, 0),
            0
        ));
        assert!(grants_immunity(
            &immunity(AURA_MECHANIC_IMMUNITY, 7),
            &aura(12, 0, 7, 0),
            0
        ));
        assert!(!grants_immunity(
            &immunity(AURA_MECHANIC_IMMUNITY, 5),
            &aura(12, 12, 7, 0),
            0
        ));

        // 41 DISPEL — the aura's `Dispel`.
        let magic = SpellDisplay {
            dispel: 1,
            ..aura(12, 12, 0, 0)
        };
        assert!(grants_immunity(
            &immunity(AURA_DISPEL_IMMUNITY, 1),
            &magic,
            0
        ));
        assert!(!grants_immunity(
            &immunity(AURA_DISPEL_IMMUNITY, 2),
            &magic,
            0
        ));

        // 38 STATE — the aura's own `EffectApplyAuraName[i]`.
        assert!(grants_immunity(
            &immunity(AURA_STATE_IMMUNITY, 12),
            &aura(12, 0, 0, 0),
            0
        ));

        // 39 SCHOOL — **`School` is an index and `EffectMiscValue` a mask**, so the arm shifts.
        // A frost aura (school 4) is covered by a mask with bit 4 set, not by the value 4.
        let frost = aura(12, 12, 0, 4);
        assert!(grants_immunity(
            &immunity(AURA_SCHOOL_IMMUNITY, 1 << 4),
            &frost,
            0
        ));
        assert!(
            !grants_immunity(&immunity(AURA_SCHOOL_IMMUNITY, 4), &frost, 0),
            "the raw index must not match — that is the bug this shift exists to avoid"
        );
        // …and the aura can veto the school arm specifically.
        let unschooled = SpellDisplay {
            attributes_ex2: ATTR_EX2_NO_SCHOOL_IMMUNITY,
            ..frost
        };
        assert!(!grants_immunity(
            &immunity(AURA_SCHOOL_IMMUNITY, 1 << 4),
            &unschooled,
            0
        ));
    }

    /// **The scanner's three outcomes** — and the middle one is why every arm carries two reasons.
    #[test]
    fn the_scan_reports_exempt_rejected_or_absent() {
        let stun = aura(12, 0, 9, 0); // EffectMechanic 9, so a rejection names 9
        let plain = SpellDisplay::default();
        let ice_block = immunity(AURA_MECHANIC_IMMUNITY, 9);
        let lookup = |id: u32| match id {
            100 => Some(&stun),
            _ => None,
        };

        // No aura of that type at all: not exempt, nothing named — the arm uses its OWN reason.
        assert_eq!(
            cc_exemption(&plain, [0, 0, 0], 12, lookup),
            CcExemption {
                exempt: false,
                mechanic: 0
            }
        );

        // A matching aura, rejected: the arm reports the blocking MECHANIC, which turns its
        // message into "Can't do that while %s".
        assert_eq!(
            cc_exemption(&plain, [100], 12, lookup),
            CcExemption {
                exempt: false,
                mechanic: 9
            }
        );

        // A matching aura, accepted: EXEMPT — the arm is skipped and the cast goes out.
        assert_eq!(
            cc_exemption(&ice_block, [100], 12, lookup),
            CcExemption {
                exempt: true,
                mechanic: 0
            }
        );

        // An id the catalog does not know is skipped, exactly as the reference's bound test does.
        assert_eq!(
            cc_exemption(&ice_block, [999], 12, lookup),
            CcExemption::default()
        );
    }

    /// The mechanic the scan names prefers the per-effect column and falls back to the spell's.
    #[test]
    fn the_named_mechanic_prefers_the_per_effect_column() {
        let plain = SpellDisplay::default();
        let both = aura(12, 5, 9, 0);
        let only_spell = aura(12, 5, 0, 0);
        assert_eq!(
            cc_exemption(&plain, [1], 12, |_| Some(&both)).mechanic,
            9,
            "EffectMechanic[i] wins"
        );
        assert_eq!(
            cc_exemption(&plain, [1], 12, |_| Some(&only_spell)).mechanic,
            5,
            "…and Mechanic is the fallback"
        );
    }
}
