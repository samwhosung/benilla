//! The validator's rungs after power (`0x6096b3`-`0x60a02d`) and its crowd-control joins, each
//! pinned against the reference's order: which reason wins when two rungs fail.

use super::*;
use benilla_formats::CcExemption;
use benilla_protocol::ObjectFields;

/// Descriptor indices: `UNIT_FIELD_CHARM`, `_SUMMON`, `_CHARMEDBY`, `_HEALTH`, `_FLAGS`,
/// `_AURASTATE`, `_MOUNTDISPLAYID`, `_BYTES_1` and `PLAYER_FIELD_BYTES`.
const CHARM: u16 = 6;
const SUMMON: u16 = 8;
const CHARMED_BY: u16 = 10;
const HEALTH: u16 = 22;
const FLAGS: u16 = 46;
const AURA_STATE: u16 = 125;
const MOUNT_DISPLAY: u16 = 133;
const BYTES_1: u16 = 138;
const PLAYER_BYTES: u16 = 1222;

const STUNNED: u32 = 0x0004_0000;
const SILENCED: u32 = 0x0000_2000;
const PACIFIED: u32 = 0x0002_0000;
const CONFUSED: u32 = 0x0040_0000;
const FLEEING: u32 = 0x0080_0000;
const IN_COMBAT: u32 = 0x0008_0000;

/// Our guid.
const ME: u64 = 7;

fn spell(prevention_type: u32) -> SpellDisplay {
    SpellDisplay {
        prevention_type,
        ..Default::default()
    }
}

/// A scan that finds no aura of any type: every arm answers its own reason.
fn no_auras() -> impl FnMut(u32) -> CcExemption {
    |_: u32| CcExemption::default()
}

/// A scan answering per aura type, as `0x6e9ca0` does for each.
fn scans(by_type: &'static [(u32, bool, u32)]) -> impl FnMut(u32) -> CcExemption {
    move |ty: u32| {
        by_type.iter().find(|(t, _, _)| *t == ty).map_or_else(
            CcExemption::default,
            |&(_, exempt, mechanic)| CcExemption { exempt, mechanic },
        )
    }
}

/// The arms alone, nobody else at the reins, no auras.
fn cc(flags: u32, d: &SpellDisplay) -> Option<u8> {
    cast_cc_refusal(flags, false, Some(d), &mut no_auras()).map(|r| r.reason)
}

/// The caster the rungs read: health 100 unless set, standing, weapons away, still, outdoors, at
/// day.
#[derive(Clone)]
struct Me {
    pairs: Vec<(u16, u32)>,
    stand: u8,
    sheathe: u8,
    move_flags: u32,
    daytime: bool,
    outdoors: bool,
    standing: i32,
    cast_ms: u32,
}

impl Default for Me {
    fn default() -> Self {
        Self {
            pairs: vec![(HEALTH, 100)],
            stand: 0,
            sheathe: 0,
            move_flags: 0,
            daytime: true,
            outdoors: true,
            standing: 0,
            cast_ms: 0,
        }
    }
}

impl Me {
    fn with(mut self, index: u16, value: u32) -> Self {
        self.pairs.retain(|&(i, _)| i != index);
        self.pairs.push((index, value));
        self
    }

    fn without(mut self, index: u16) -> Self {
        self.pairs.retain(|&(i, _)| i != index);
        self
    }

    fn refusal_with(
        &self,
        d: Option<&SpellDisplay>,
        scan: &mut dyn FnMut(u32) -> CcExemption,
    ) -> Option<Refusal> {
        let fields = ObjectFields::from_pairs(&self.pairs);
        let caster = CasterState {
            fields: &fields,
            guid: Some(ME),
            stand_state: self.stand,
            sheathe_state: self.sheathe,
            move_flags: self.move_flags,
            daytime: self.daytime,
            outdoors: self.outdoors,
        };
        let standing = |_: u32| self.standing;
        let inputs = SpellInputs {
            cast_time_ms: self.cast_ms,
            form_is_stance: false,
            standing: &standing,
        };
        caster_refusal(d, &caster, &inputs, scan)
    }

    fn refuse(&self, d: &SpellDisplay) -> Option<u8> {
        self.refusal_with(Some(d), &mut no_auras())
            .map(|r| r.reason)
    }
}

// ── The crowd-control arms (`0x609945`-`0x609b9e`) ──

/// Stun, fear, charm and confuse refuse every spell; silence and pacify only the rows whose
/// `PreventionType` names them.
#[test]
fn crowd_control_refuses_by_prevention_type_except_the_unconditional_arms() {
    // Fireball's shape (silence), Heroic Strike's (pacify), auto-attack's (neither).
    let cast = spell(1);
    let melee = spell(2);
    let neither = spell(0);

    for (flags, reason) in [(STUNNED, 0x64), (FLEEING, 0x1e), (CONFUSED, 0x16)] {
        for d in [&cast, &melee, &neither] {
            assert_eq!(cc(flags, d), Some(reason), "flags {flags:#x}");
        }
    }
    // Charm: somebody else holds the reins.
    assert_eq!(
        cast_cc_refusal(0, true, Some(&neither), &mut no_auras()).map(|r| r.reason),
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
        cast_cc_refusal(SILENCED, false, None, &mut no_auras()),
        None
    );
    assert_eq!(
        cast_cc_refusal(STUNNED, false, None, &mut no_auras()).map(|r| r.reason),
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
        cast_cc_refusal(STUNNED | SILENCED, true, Some(&cast), &mut no_auras()).map(|r| r.reason),
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

/// The exemption's three outcomes at an arm: its own reason, `0x8d` with the mechanic, skipped.
#[test]
fn the_exemption_skips_an_arm_or_renames_its_refusal() {
    let cast = spell(1);
    let arm = |scan: &mut dyn FnMut(u32) -> CcExemption| {
        cast_cc_refusal(STUNNED, false, Some(&cast), scan)
    };
    assert_eq!(arm(&mut no_auras()), Some(Refusal::of(0x64)));
    assert_eq!(
        arm(&mut scans(&[(12, false, 12)])),
        Some(Refusal {
            reason: 0x8d,
            arg: Some(12)
        }),
        "a blocking aura the cast does not counter"
    );
    assert_eq!(
        arm(&mut scans(&[(12, true, 0)])),
        None,
        "the cast grants immunity (Ice Block while stunned)"
    );
    // The silence arm scans the stun type too (`0x6e9e8a`), so the same exemption lifts it.
    assert_eq!(
        cast_cc_refusal(
            STUNNED | SILENCED,
            false,
            Some(&cast),
            &mut scans(&[(12, true, 0)])
        ),
        None
    );
}

/// A dead caster skips the control block (`0x60980d`'s `jle` on `UNIT_FIELD_HEALTH`); no health
/// field yet is not death.
#[test]
fn a_dead_caster_takes_no_crowd_control_refusal() {
    let cast = spell(1);
    let stunned = Me::default().with(FLAGS, STUNNED);
    assert_eq!(stunned.clone().with(HEALTH, 0).refuse(&cast), None);
    assert_eq!(
        Me::default()
            .with(HEALTH, 0)
            .with(CHARMED_BY, 0x99)
            .refuse(&cast),
        None
    );
    assert_eq!(stunned.clone().with(HEALTH, 1).refuse(&cast), Some(0x64));
    assert_eq!(stunned.without(HEALTH).refuse(&cast), Some(0x64));
    // Our own charm is no charm (`0x60995e`).
    assert_eq!(
        Me::default().with(CHARMED_BY, ME as u32).refuse(&cast),
        None
    );
}

// ── The wrappers over the scan (`0x6e9e60`, `0x6e9ec0`, `0x6e9f60`) ──

/// The wrapper returns 1 as soon as any scan does (`0x6e9e84`, `0x6e9e95`, `0x6e9ea6`): a silence
/// aura the cast does not counter is lifted by a later type's exemption, and so for the charm arm.
#[test]
fn a_later_aura_type_that_exempts_lifts_the_arm() {
    let cast = spell(1);
    assert_eq!(
        cast_cc_refusal(
            SILENCED,
            false,
            Some(&cast),
            &mut scans(&[(27, false, 15), (12, true, 0)])
        ),
        None,
        "silence {{27, 12, 60}}: 27 refuses, 12 exempts"
    );
    assert_eq!(
        cast_cc_refusal(
            0,
            true,
            Some(&cast),
            &mut scans(&[(6, false, 3), (2, true, 0)])
        ),
        None,
        "charm {{6, 177, 2}}: 6 refuses, 2 exempts (`0x6e9fa9`)"
    );
}

/// The out word is cleared once at entry (`0x6e9e72`) and each refusing scan overwrites it with a
/// nonzero mechanic (`0x6e9d4c`, `0x6e9d5f`): the last one names the line, and a refusal with no
/// mechanic leaves the one before.
#[test]
fn the_last_refusing_mechanic_names_the_line() {
    let cast = spell(1);
    let silenced = |scan: &mut dyn FnMut(u32) -> CcExemption| {
        cast_cc_refusal(SILENCED, false, Some(&cast), scan)
    };
    assert_eq!(
        silenced(&mut scans(&[(27, false, 15), (12, false, 12)])),
        Some(Refusal {
            reason: 0x8d,
            arg: Some(12)
        })
    );
    assert_eq!(
        silenced(&mut scans(&[(27, false, 15), (12, false, 0)])),
        Some(Refusal {
            reason: 0x8d,
            arg: Some(15)
        })
    );
    assert_eq!(
        silenced(&mut scans(&[(60, false, 9)])),
        Some(Refusal {
            reason: 0x8d,
            arg: Some(9)
        }),
        "the last type alone"
    );
}

// ── The three-effect loop (`0x6096b3`-`0x6097fc`) ──

/// A summon or charm effect (28, 55, 56, or aura 6 or 2) refuses with 4 while a charm is held,
/// else 5 while a summon is, unless `AttributesEx` bit 0 (`0x609715`-`0x6098d6`).
#[test]
fn a_summon_refuses_while_a_charm_or_a_summon_is_held() {
    let call_pet = SpellDisplay {
        effects: [28, 0, 0],
        ..Default::default()
    };
    let with_summon = Me::default().with(SUMMON, 0x99);
    let with_charm = Me::default().with(CHARM, 0x98);
    assert_eq!(with_summon.refuse(&call_pet), Some(5));
    assert_eq!(with_charm.refuse(&call_pet), Some(4));
    assert_eq!(
        with_charm.clone().with(SUMMON, 0x99).refuse(&call_pet),
        Some(4),
        "the charm is tested first"
    );
    assert_eq!(Me::default().refuse(&call_pet), None);
    for effect in [55, 56] {
        let d = SpellDisplay {
            effects: [0, effect, 0],
            ..Default::default()
        };
        assert_eq!(with_summon.refuse(&d), Some(5), "effect {effect}");
    }
    for aura in [6, 2] {
        let d = SpellDisplay {
            effects: [6, 0, 0],
            effect_apply_aura: [aura, 0, 0],
            ..Default::default()
        };
        assert_eq!(with_summon.refuse(&d), Some(5), "aura {aura}");
    }
    let dismissing = SpellDisplay {
        attributes_ex: 0x1,
        ..call_pet
    };
    assert_eq!(with_summon.refuse(&dismissing), None);
}

/// An effect aimed at the pet (implicit target 5, A or B) refuses with `0x4c` "You do not have a
/// pet" when neither a charm nor a summon is held (`0x60973e`-`0x609761`).
#[test]
fn a_pet_spell_refuses_without_a_pet() {
    let mend_pet = SpellDisplay {
        effect_implicit_target_a: [5, 0, 0],
        ..Default::default()
    };
    assert_eq!(Me::default().refuse(&mend_pet), Some(0x4c));
    assert_eq!(Me::default().with(SUMMON, 0x99).refuse(&mend_pet), None);
    assert_eq!(Me::default().with(CHARM, 0x98).refuse(&mend_pet), None);
    let by_b = SpellDisplay {
        effect_implicit_target_b: [0, 0, 5],
        ..Default::default()
    };
    assert_eq!(Me::default().refuse(&by_b), Some(0x4c));
}

/// The loop runs effect by effect, the summon test before the pet test, and all of it before the
/// control block: a stunned caster calling a second pet is told about the pet.
#[test]
fn the_effect_loop_runs_before_the_control_block() {
    let summon_then_pet = SpellDisplay {
        effects: [56, 0, 0],
        effect_implicit_target_a: [0, 5, 0],
        ..Default::default()
    };
    assert_eq!(Me::default().refuse(&summon_then_pet), Some(0x4c));
    assert_eq!(
        Me::default().with(CHARM, 0x98).refuse(&summon_then_pet),
        Some(4)
    );
    assert_eq!(
        Me::default()
            .with(SUMMON, 0x99)
            .with(FLAGS, STUNNED)
            .refuse(&summon_then_pet),
        Some(5)
    );
    assert_eq!(
        Me::default().with(FLAGS, STUNNED).refuse(&summon_then_pet),
        Some(0x4c)
    );
}

// ── The control block's own rung (`0x609813`-`0x609842`) ──

/// A living player logging out (`PLAYER_FIELD_BYTES` byte 0 bit `0x4`) is refused `0x61` ahead of
/// the crowd-control arms; a dead one skips it with them.
#[test]
fn a_player_logging_out_is_refused_ahead_of_the_crowd_control() {
    let cast = spell(1);
    let logging_out = Me::default().with(PLAYER_BYTES, 0x4);
    assert_eq!(logging_out.refuse(&cast), Some(0x61));
    assert_eq!(
        logging_out.clone().with(FLAGS, STUNNED).refuse(&cast),
        Some(0x61)
    );
    assert_eq!(logging_out.with(HEALTH, 0).refuse(&cast), None);
    assert_eq!(
        Me::default().with(PLAYER_BYTES, 0x1).refuse(&cast),
        None,
        "another bit of the byte"
    );
}

// ── The interrupt-flag stance rungs and the posture rungs (`0x609ba1`-`0x609cbd`) ──

/// `0x41` "You must be unsheathed" for interrupt flag `0x200` with the weapon away, arm A
/// on the aura flags, arm B on the channel flags of a channelled spell (`0x609ba1`-`0x609bf3`).
#[test]
fn an_unsheathed_only_spell_needs_a_drawn_weapon() {
    let by_aura = SpellDisplay {
        aura_interrupt_flags: 0x200,
        ..Default::default()
    };
    let by_channel = SpellDisplay {
        channel_interrupt_flags: 0x200,
        attributes_ex: 0x4,
        ..Default::default()
    };
    let unchannelled = SpellDisplay {
        channel_interrupt_flags: 0x200,
        ..Default::default()
    };
    let away = Me::default();
    let drawn = Me {
        sheathe: 1,
        ..Me::default()
    };
    assert_eq!(away.refuse(&by_aura), Some(0x41));
    assert_eq!(away.refuse(&by_channel), Some(0x41));
    assert_eq!(away.refuse(&unchannelled), None);
    assert_eq!(drawn.refuse(&by_aura), None);
    assert_eq!(
        Me {
            sheathe: 2,
            ..Me::default()
        }
        .refuse(&by_channel),
        None
    );
}

/// `0x53` for interrupt flag `0x40` without a mount, by both arms (`0x609c05`-`0x609c5a`); a
/// mounted caster then meets the mounted rung `0x39` unless `Attributes` bit 24.
#[test]
fn a_mounted_only_spell_needs_a_mount() {
    let gallop = SpellDisplay {
        aura_interrupt_flags: 0x40,
        attributes: 0x0100_0000,
        ..Default::default()
    };
    let by_channel = SpellDisplay {
        channel_interrupt_flags: 0x40,
        attributes_ex: 0x40,
        attributes: 0x0100_0000,
        ..Default::default()
    };
    let mounted = Me::default().with(MOUNT_DISPLAY, 0x100);
    assert_eq!(Me::default().refuse(&gallop), Some(0x53));
    assert_eq!(Me::default().refuse(&by_channel), Some(0x53));
    assert_eq!(mounted.refuse(&gallop), None);
    let not_while_mounted = SpellDisplay {
        attributes: 0,
        ..gallop
    };
    assert_eq!(mounted.refuse(&not_while_mounted), Some(0x39));
}

/// The rungs' order: unsheathed `0x41`, only mounted `0x53`, mounted `0x39`, standing `0x3e`.
#[test]
fn the_stance_rungs_precede_the_mount_and_the_mount_the_posture() {
    let both = SpellDisplay {
        aura_interrupt_flags: 0x240,
        ..Default::default()
    };
    assert_eq!(Me::default().refuse(&both), Some(0x41));
    let sitting = Me {
        stand: 1,
        ..Me::default()
    };
    let plain = SpellDisplay::default();
    assert_eq!(sitting.refuse(&plain), Some(0x3e));
    assert_eq!(
        sitting.clone().with(MOUNT_DISPLAY, 0x100).refuse(&plain),
        Some(0x39)
    );
    let sitting_cast = SpellDisplay {
        attributes: 0x0800_0000,
        ..Default::default()
    };
    assert_eq!(sitting.refuse(&sitting_cast), None, "Attributes bit 27");
    assert_eq!(
        sitting.refusal_with(None, &mut no_auras()),
        Some(Refusal::of(0x3e)),
        "no record, no exemption"
    );
    for state in [3, 8] {
        assert_eq!(
            Me {
                stand: state,
                ..Me::default()
            }
            .refuse(&plain),
            Some(0x3e),
            "stand state {state}"
        );
    }
}

/// `Attributes` bit 12 refuses `0x51` at night, bit 13 `0x54` by day (`0x609cd3`-`0x609d1e`),
/// after the posture rung and before the water rungs.
#[test]
fn the_day_and_night_rungs_read_the_clock() {
    let by_day = SpellDisplay {
        attributes: 0x1000,
        ..Default::default()
    };
    let by_night = SpellDisplay {
        attributes: 0x2000,
        ..Default::default()
    };
    let night = Me {
        daytime: false,
        ..Me::default()
    };
    assert_eq!(night.refuse(&by_day), Some(0x51));
    assert_eq!(Me::default().refuse(&by_day), None);
    assert_eq!(Me::default().refuse(&by_night), Some(0x54));
    assert_eq!(night.refuse(&by_night), None);
    assert_eq!(
        Me {
            stand: 1,
            ..night.clone()
        }
        .refuse(&by_day),
        Some(0x3e)
    );
    // Aquatic Form's water bit on land, by day only.
    let aquatic_by_day = SpellDisplay {
        attributes: 0x1000,
        aura_interrupt_flags: 0x100,
        ..Default::default()
    };
    assert_eq!(night.refuse(&aquatic_by_day), Some(0x51));
    assert_eq!(Me::default().refuse(&aquatic_by_day), Some(0x58));
}

// ── The tail (`0x609e76`-`0x609f47`) ──

/// `0x57` for a stealth-only spell out of stealth (`UNIT_FIELD_BYTES_1` byte 3 bit 2), reason 0
/// `AFFECTING_COMBAT` for a peaceful-only spell in combat, `0x12` for a missing caster aura state,
/// `0x8f` below the required rank: in that order, after the moving rung.
#[test]
fn the_stealth_combat_aura_state_and_reputation_rungs_in_order() {
    let ambush = SpellDisplay {
        attributes: 0x0002_0000,
        ..Default::default()
    };
    let peaceful = SpellDisplay {
        attributes: 0x1000_0000,
        ..Default::default()
    };
    let overpower = SpellDisplay {
        caster_aura_state: 1,
        ..Default::default()
    };
    let feinting = SpellDisplay {
        min_faction_id: 47,
        min_reputation: 4,
        ..Default::default()
    };
    let me = Me::default();
    assert_eq!(me.refuse(&ambush), Some(0x57));
    assert_eq!(me.clone().with(BYTES_1, 0x0200_0000).refuse(&ambush), None);
    assert_eq!(me.clone().with(FLAGS, IN_COMBAT).refuse(&peaceful), Some(0));
    assert_eq!(me.refuse(&peaceful), None);
    assert_eq!(me.refuse(&overpower), Some(0x12));
    assert_eq!(me.clone().with(AURA_STATE, 0x1).refuse(&overpower), None);
    assert_eq!(
        me.clone().with(AURA_STATE, 0x2).refuse(&overpower),
        Some(0x12),
        "state n is bit n - 1"
    );
    let friendly = |standing| Me {
        standing,
        ..Me::default()
    };
    assert_eq!(friendly(2999).refuse(&feinting), Some(0x8f));
    assert_eq!(friendly(3000).refuse(&feinting), None, "Friendly's floor");
    assert_eq!(friendly(-50_000).refuse(&SpellDisplay::default()), None);

    // Every one of them failing: stealth first, then combat, then the aura state.
    let all = SpellDisplay {
        attributes: 0x1002_0000,
        caster_aura_state: 1,
        min_faction_id: 47,
        min_reputation: 4,
        ..Default::default()
    };
    let in_combat = Me::default().with(FLAGS, IN_COMBAT);
    assert_eq!(in_combat.refuse(&all), Some(0x57));
    let stealthed = in_combat.with(BYTES_1, 0x0200_0000);
    assert_eq!(stealthed.refuse(&all), Some(0));
    let out = stealthed.with(FLAGS, 0);
    assert_eq!(out.refuse(&all), Some(0x12));
    assert_eq!(
        Me {
            standing: 0,
            ..out.with(AURA_STATE, 1)
        }
        .refuse(&all),
        Some(0x8f)
    );
    // The moving rung comes first.
    let moving_ambush = SpellDisplay {
        interrupt_flags: 0x1,
        ..ambush
    };
    assert_eq!(
        Me {
            move_flags: 0x1,
            cast_ms: 1500,
            ..Me::default()
        }
        .refuse(&moving_ambush),
        Some(0x2e)
    );
}

// ── The place rungs (`0x609f59`-`0x609fd0`) ──

/// `Attributes` bit 14 refuses `0x52` outdoors and bit 15 indoors, the one answer of `0x612cc0`:
/// `0x4b` for a spell whose effect applies the mounted aura (`Effect` 6 with aura 78, the same
/// effect), else `0x55`. A spell with both bits is refused on either answer. They follow the
/// reputation rung.
#[test]
fn the_place_rungs_ask_the_outdoors_test_after_reputation() {
    let place = |attributes: u32| SpellDisplay {
        attributes,
        ..Default::default()
    };
    let outside = Me::default();
    let inside = Me {
        outdoors: false,
        ..Me::default()
    };
    assert_eq!(outside.refuse(&place(0x4000)), Some(0x52));
    assert_eq!(inside.refuse(&place(0x4000)), None);
    assert_eq!(inside.refuse(&place(0x8000)), Some(0x55));
    assert_eq!(outside.refuse(&place(0x8000)), None);
    assert_eq!(outside.refuse(&place(0xc000)), Some(0x52));
    assert_eq!(inside.refuse(&place(0xc000)), Some(0x55));
    assert_eq!(inside.refuse(&place(0)), None, "neither bit asks nothing");

    let mount = |effects: [u32; 3], auras: [u32; 3]| SpellDisplay {
        attributes: 0x8000,
        effects,
        effect_apply_aura: auras,
        ..Default::default()
    };
    assert_eq!(inside.refuse(&mount([6, 0, 0], [78, 0, 0])), Some(0x4b));
    assert_eq!(
        inside.refuse(&mount([0, 0, 6], [0, 0, 78])),
        Some(0x4b),
        "any of the three"
    );
    assert_eq!(
        inside.refuse(&mount([6, 0, 0], [0, 78, 0])),
        Some(0x55),
        "the aura of the same effect"
    );
    assert_eq!(inside.refuse(&mount([0, 0, 0], [78, 0, 0])), Some(0x55));

    let unknown_rank = SpellDisplay {
        attributes: 0x4000,
        min_faction_id: 47,
        min_reputation: 4,
        ..Default::default()
    };
    assert_eq!(
        outside.refuse(&unknown_rank),
        Some(0x8f),
        "reputation first"
    );
}
