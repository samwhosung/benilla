//! The crowd-control exemption each of the cast validator's six crowd-control arms checks before
//! refusing: does the spell being cast grant immunity to the aura blocking it (Ice Block while
//! stunned)? The scan is `0x6e9ca0`, the matcher `0x6e9d70`, whose first gate, `AttributesEx` bit
//! 15 on the cast, is clear on almost every spell, so the scan mostly picks the refusal's message.

use super::SpellDisplay;

/// The only effect kind the matcher's loop considers (`0x6e9da0`).
const SPELL_EFFECT_APPLY_AURA: u32 = 6;

/// `AttributesEx` bit 15, which the cast must carry for any immunity to count (`0x6e9d81`);
/// inferred as `SPELL_ATTR_EX_DISPEL_AURAS_ON_IMMUNITY`.
const ATTR_EX_DISPELS_ON_IMMUNITY: u32 = 0x0000_8000;

/// `Attributes` bit 29, which the blocking aura must not carry (`0x6e9d89`); inferred as
/// `SPELL_ATTR_UNAFFECTED_BY_INVULNERABILITY`.
const ATTR_UNAFFECTED_BY_INVULNERABILITY: u32 = 0x2000_0000;

/// `AttributesEx2` bit 26 on the blocking aura vetoes the school arm (`0x6e9dcd`).
const ATTR_EX2_NO_SCHOOL_IMMUNITY: u32 = 0x0400_0000;

/// The four immunity aura types the matcher switches on, read off the cast's own effects
/// (`0x6e9dac`); any other value moves on to the next effect.
const AURA_STATE_IMMUNITY: u32 = 38;
const AURA_SCHOOL_IMMUNITY: u32 = 39;
const AURA_DISPEL_IMMUNITY: u32 = 41;
const AURA_MECHANIC_IMMUNITY: u32 = 77;

/// The matcher (`0x6e9d70`): does `cast` grant immunity to effect `aura_effect` of `aura`? The
/// school arm shifts because `School` is an index and `EffectMiscValue` a mask.
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
        // `EffectMiscValue` is signed: a negative one reads as `u32::MAX`, which equals no id but
        // sets every school bit.
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
    /// The arm is skipped: an aura of the wanted type was found and every match accepted.
    pub exempt: bool,
    /// The rejected aura's mechanic (`EffectMechanic[i]`, else `Mechanic`), which makes the
    /// refusal "Can't do that while %s"; 0 means the arm refuses with its own reason.
    pub mechanic: u32,
}

/// The scanner (`0x6e9ca0`): asks [`grants_immunity`] about each `wanted_aura_type` effect on the
/// caster's auras and stops at the first rejection, whose mechanic names the refusal (`0x8d`,
/// `PREVENTED_BY_MECHANIC`). `aura_ids` are the raw `UNIT_FIELD_AURA` slots: the reference reads
/// no aura flags, duration, stacks or caster, so any nonzero known id counts.
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
            continue; // out of the id table's range: the reference's own bound test
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

    #[test]
    fn an_ordinary_cast_grants_no_immunity() {
        let stun = aura(12, 12, 0, 0);
        // A matching mechanic immunity, but with bit 15 clear it never looks.
        let mut ordinary = immunity(AURA_MECHANIC_IMMUNITY, 12);
        ordinary.attributes_ex = 0;
        assert!(!grants_immunity(&ordinary, &stun, 0));

        let real = immunity(AURA_MECHANIC_IMMUNITY, 12);
        assert!(grants_immunity(&real, &stun, 0));

        let stubborn = SpellDisplay {
            attributes: ATTR_UNAFFECTED_BY_INVULNERABILITY,
            ..aura(12, 12, 0, 0)
        };
        assert!(!grants_immunity(&real, &stubborn, 0));
    }

    #[test]
    fn each_immunity_arm_reads_its_own_field() {
        // 77 MECHANIC: the aura's `Mechanic` or its `EffectMechanic[i]`.
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

        // 41 DISPEL: the aura's `Dispel`.
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

        // 38 STATE: the aura's `EffectApplyAuraName[i]`.
        assert!(grants_immunity(
            &immunity(AURA_STATE_IMMUNITY, 12),
            &aura(12, 0, 0, 0),
            0
        ));

        // 39 SCHOOL: a frost aura (school 4) is covered by mask bit 4, not by the value 4.
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

    #[test]
    fn the_scan_reports_exempt_rejected_or_absent() {
        let stun = aura(12, 0, 9, 0); // EffectMechanic 9, so a rejection names 9
        let plain = SpellDisplay::default();
        let ice_block = immunity(AURA_MECHANIC_IMMUNITY, 9);
        let lookup = |id: u32| match id {
            100 => Some(&stun),
            _ => None,
        };

        // No aura of that type: nothing named, so the arm uses its own reason.
        assert_eq!(
            cc_exemption(&plain, [0, 0, 0], 12, lookup),
            CcExemption {
                exempt: false,
                mechanic: 0
            }
        );

        // A matching aura, rejected: its mechanic makes the message "Can't do that while %s".
        assert_eq!(
            cc_exemption(&plain, [100], 12, lookup),
            CcExemption {
                exempt: false,
                mechanic: 9
            }
        );

        assert_eq!(
            cc_exemption(&ice_block, [100], 12, lookup),
            CcExemption {
                exempt: true,
                mechanic: 0
            }
        );

        // An unknown id is skipped, as the reference's bound test does.
        assert_eq!(
            cc_exemption(&ice_block, [999], 12, lookup),
            CcExemption::default()
        );
    }

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
