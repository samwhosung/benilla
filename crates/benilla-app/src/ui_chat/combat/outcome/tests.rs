use super::*;
use crate::ui_chat::combat::WordText;

fn swing(hit_info: u32, victim_state: u32, damage: u32, school: u8) -> AttackerState {
    AttackerState {
        attacker: 1,
        victim: 2,
        hit_info,
        damage,
        victim_state,
        absorb: 0,
        resist: 0,
        blocked: 0,
        melee_spell_id: 0,
        school,
    }
}

fn stem(s: &AttackerState) -> Option<&'static str> {
    melee_outcome(s).map(|(f, _)| f.stem)
}

fn word(s: &AttackerState) -> Option<LineWord> {
    melee_outcome(s).map(|(_, w)| w)
}

/// `0x629b60`'s arm order: a blocked swing with damage is `VSBLOCK`, and a fully absorbed one
/// `VSABSORB`, not `MISSED`.
#[test]
fn the_melee_dispatcher_follows_the_reference_order() {
    let f = |h, v, d, s| stem(&swing(h, v, d, s)).expect("a family");
    // A plain landed swing, and its crit and school variants.
    assert_eq!(f(0, 1, 120, 0), "COMBATHIT");
    assert_eq!(f(0x80, 1, 120, 0), "COMBATHITCRIT");
    assert_eq!(f(0, 1, 120, 2), "COMBATHITSCHOOL");
    assert_eq!(f(0x80, 1, 120, 2), "COMBATHITCRITSCHOOL");
    // MISS wins over everything, including a VictimState that would say otherwise.
    assert_eq!(f(0x10, 2, 0, 0), "MISSED");
    // BLOCKS wins over the damage test: a partial block still lands damage.
    assert_eq!(f(0, 5, 90, 0), "VSBLOCK");
    // Zero damage + the absorb/resist bits.
    assert_eq!(f(0x20, 1, 0, 0), "VSABSORB");
    assert_eq!(f(0x40, 1, 0, 0), "VSRESIST");
    // ...but the same bits with damage through are a landed hit, not a full absorb.
    assert_eq!(f(0x20, 1, 90, 0), "COMBATHIT");
    // The VictimState words.
    assert_eq!(f(0, 2, 0, 0), "VSDODGE");
    assert_eq!(f(0, 3, 0, 0), "VSPARRY");
    assert_eq!(f(0, 6, 0, 0), "VSEVADE");
    assert_eq!(f(0, 7, 0, 0), "VSIMMUNE");
    assert_eq!(f(0, 8, 0, 0), "VSDEFLECT");
}

/// `0x62a710` prints nothing for a VictimState whose byte in `0x8628f8`, `[0,0,1,1,0,1,1,1,1,0]`,
/// is 0. State 1 words a hit only with damage through, in arm 5.
#[test]
fn the_silent_victim_states_emit_no_melee_line() {
    for state in [0, 1, 4, 9] {
        assert!(
            melee_outcome(&swing(0, state, 0, 0)).is_none(),
            "VictimState {state} must emit no line"
        );
    }
    // The same states still word normally when an earlier arm claims them: the MISS bit, and
    // state 1 with damage through.
    assert!(stem(&swing(0x10, 0, 0, 0)).is_some(), "the MISS bit wins");
    assert!(stem(&swing(0, 1, 120, 0)).is_some(), "a landed hit wins");
    // Index 5 of the table is a 1, and unreachable: arm 2 claims a block.
    assert_eq!(stem(&swing(0, 5, 0, 0)), Some("VSBLOCK"));
}

/// The word is the arm's own (`0x62a26d`, `0x62a45b`, `0x62a57b`, `0x62a69b`, the table at
/// `0x62a908`): the MISS bit words `MISS` whatever the state, and a state with no line, an
/// interrupt or a damageless hit, has no word either.
#[test]
fn each_melee_arm_raises_its_own_word() {
    let w = |h, v, d| word(&swing(h, v, d, 0)).map(|w| w.message_type);
    assert_eq!(w(0x10, 2, 0), Some("MISS"), "the MISS bit, not DODGE");
    assert_eq!(w(0x10, 7, 0), Some("MISS"), "the MISS bit, not IMMUNE");
    assert_eq!(w(0, 5, 0), Some("BLOCK"));
    assert_eq!(w(0x20, 1, 0), Some("ABSORB"));
    assert_eq!(w(0x40, 1, 0), Some("RESIST"));
    assert_eq!(w(0, 2, 0), Some("DODGE"));
    assert_eq!(w(0, 3, 0), Some("PARRY"));
    assert_eq!(w(0, 6, 0), Some("EVADE"));
    assert_eq!(w(0, 7, 0), Some("IMMUNE"));
    assert_eq!(w(0, 8, 0), Some("DEFLECT"));
    assert_eq!(w(0, 1, 0), None, "a damageless hit: no line, no MISS");
    assert_eq!(w(0, 4, 0), None, "an interrupt");
    // The bare arms carry no figures.
    assert_eq!(word(&swing(0, 5, 0, 0)), Some(LineWord::bare("BLOCK")));
}

/// A landed swing words the first of resist, block and absorb with `(damage, partial)`, ahead of
/// the crit (`0x629efe`, `0x629f16`, `0x629f31`, `0x629f4b`).
#[test]
fn a_landed_swing_words_its_first_partial_then_the_crit() {
    let hit = |hit_info, absorb, resist, blocked| {
        word(&AttackerState {
            absorb,
            resist,
            blocked,
            ..swing(hit_info, 1, 300, 0)
        })
        .expect("a landed hit")
    };
    assert_eq!(hit(0, 50, 0, 40), LineWord::pair("BLOCK", 300, 40));
    assert_eq!(hit(0, 50, 20, 0), LineWord::pair("RESIST", 300, 20));
    assert_eq!(hit(0x80, 50, 20, 40), LineWord::pair("RESIST", 300, 20));
    assert_eq!(hit(0x80, 50, 0, 0), LineWord::pair("ABSORB", 300, 50));
    assert_eq!(hit(0x80, 0, 0, 0), LineWord::with("DAMAGE_CRIT", 300));
    assert_eq!(hit(0, 0, 0, 0), LineWord::with("DAMAGE", 300));
}

/// A direct spell hit with partials takes the first of resisted, blocked and absorbed
/// (`0x62d048`, `0x62d062`, `0x62d07b`); blocked carries its amount alone.
#[test]
fn a_direct_spell_hit_words_its_first_partial() {
    assert_eq!(
        spell_hit_word(300, 50, 100, 40),
        LineWord::pair("SPELL_RESISTED", 300, 100)
    );
    assert_eq!(
        spell_hit_word(300, 50, 0, 40),
        LineWord::with("SPELL_BLOCKED", 40)
    );
    assert_eq!(
        spell_hit_word(300, 50, 0, 0),
        LineWord::pair("SPELL_ABSORBED", 300, 50)
    );
    assert_eq!(
        spell_hit_word(300, 0, 0, 0),
        LineWord::with("SPELL_DAMAGE", 300)
    );
}

/// A periodic tick takes the melee types, resisted before absorbed before the damage (`0x628354`,
/// `0x62836c`, `0x62837f`).
#[test]
fn a_periodic_tick_words_resist_absorb_damage() {
    assert_eq!(
        periodic_hit_word(300, 50, 100),
        LineWord::pair("RESIST", 300, 100)
    );
    assert_eq!(
        periodic_hit_word(300, 50, 0),
        LineWord::pair("ABSORB", 300, 50)
    );
    assert_eq!(periodic_hit_word(300, 0, 0), LineWord::with("DAMAGE", 300));
}

/// The spell-miss word per code, as the jump table `0x62be10`, carrying the spell's name: absorb
/// (10) and every code past the table are `SPELL_MISSED`.
#[test]
fn a_spell_miss_words_its_code_with_the_spell_name() {
    let got: Vec<_> = (1..=13)
        .map(|code| miss_word(code, "Frostbolt").message_type)
        .collect();
    assert_eq!(
        got,
        vec![
            "SPELL_MISSED",
            "SPELL_RESISTED",
            "SPELL_DODGED",
            "SPELL_PARRIED",
            "SPELL_BLOCKED",
            "SPELL_EVADED",
            "SPELL_IMMUNE",
            "SPELL_IMMUNE",
            "SPELL_DEFLECTED",
            "SPELL_MISSED",
            "SPELL_REFLECTED",
            "SPELL_MISSED",
            "SPELL_MISSED",
        ]
    );
    assert_eq!(miss_word(255, "Frostbolt").message_type, "SPELL_MISSED");
    assert_eq!(
        miss_word(7, "Frostbolt").data,
        WordText::Text(Some("Frostbolt".into()))
    );
}

/// `HAPPINESS_POINTS` is no type of `0x862920`: a happiness gain has a line and no word.
#[test]
fn a_happiness_gain_has_no_word() {
    assert_eq!(power_gain_word(0, 300), Some(LineWord::with("MANA", 300)));
    assert_eq!(power_gain_word(3, 20), Some(LineWord::with("ENERGY", 20)));
    assert_eq!(power_gain_word(4, 1), None);
}
