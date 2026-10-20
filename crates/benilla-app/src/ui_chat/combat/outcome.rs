//! The reference's outcome switches: which family a packet's figures select, and the
//! `COMBAT_TEXT_UPDATE` word (a type of the table `0x862920`) its formatter raises once that line
//! is written. Each word here is raised only when the unit its line is about is you, class 0, and
//! the emitters in [`crate::combat_log::chat`] make that test. Not every formatter's word is so
//! gated: the reputation line raises `FACTION` with no class test (`0x62c5f0`, `0x62c667`).

use benilla_protocol::messages::AttackerState;

use super::families::*;
use super::{Family, LineWord};

/// The family that words a `SpellMissInfo` byte (vmangos `SpellDefines.h:160-174`), as the
/// reference's miss switch `0x62bb50` maps it.
pub(crate) fn miss_family(miss_info: u8) -> Family {
    match miss_info {
        2 => SPELLRESIST,
        3 => SPELLDODGED,
        4 => SPELLPARRIED,
        5 => SPELLBLOCKED,
        6 => SPELLEVADED,
        // 7 IMMUNE and 8 IMMUNE2 are one outcome with two server-side spellings.
        7 | 8 => SPELLIMMUNE,
        9 => SPELLDEFLECTED,
        11 => SPELLREFLECT,
        // 10 ABSORB, 0, 1 and anything out of range take the default arm `0x62c0e0`, not
        // `SPELLLOGABSORB`, which belongs to the direct-damage path.
        _ => SPELLMISS,
    }
}

/// The word a spell-miss line raises, with the spell's name: the jump table `0x62be10`, indexed by
/// code - 2. Absorb (10) and every code past the table take the default `SPELL_MISSED`.
pub(crate) fn miss_word(miss_info: u8, spell: &str) -> LineWord {
    LineWord::with(
        match miss_info {
            2 => "SPELL_RESISTED",
            3 => "SPELL_DODGED",
            4 => "SPELL_PARRIED",
            5 => "SPELL_BLOCKED",
            6 => "SPELL_EVADED",
            7 | 8 => "SPELL_IMMUNE",
            9 => "SPELL_DEFLECTED",
            11 => "SPELL_REFLECTED",
            _ => "SPELL_MISSED",
        },
        spell,
    )
}

/// A swing's family and word, the dispatcher `0x629b60` arm for arm; the order matters because
/// the tests overlap. The bits are vmangos's `HitInfo` in its `> 1.9.4` branch
/// (`Objects/UnitDefines.h:250-268`) and `VictimState` (`:237-248`).
pub(crate) fn melee_outcome(s: &AttackerState) -> Option<(Family, LineWord)> {
    const MISS: u32 = 0x10;
    const ABSORB: u32 = 0x20;
    const RESIST: u32 = 0x40;
    const CRIT: u32 = 0x80;
    // Each arm's own formatter words it bare: `0x62a26d`, `0x62a45b`, `0x62a57b`, `0x62a69b`.
    if s.hit_info & MISS != 0 {
        return Some((MISSED, LineWord::bare("MISS")));
    }
    if s.victim_state == 5 {
        return Some((VSBLOCK, LineWord::bare("BLOCK")));
    }
    if s.damage == 0 {
        if s.hit_info & ABSORB != 0 {
            return Some((VSABSORB, LineWord::bare("ABSORB")));
        }
        if s.hit_info & RESIST != 0 {
            return Some((VSRESIST, LineWord::bare("RESIST")));
        }
    }
    if s.victim_state == 1 && s.damage > 0 {
        let family = match (s.hit_info & CRIT != 0, s.school != 0) {
            (false, false) => COMBATHIT,
            (true, false) => COMBATHITCRIT,
            (false, true) => COMBATHITSCHOOL,
            (true, true) => COMBATHITCRITSCHOOL,
        };
        // `0x629d30`: the first of the partials, then the crit (`0x629efe`-`0x629f5e`).
        let word = if s.resist > 0 {
            LineWord::pair("RESIST", s.damage, s.resist)
        } else if s.blocked != 0 {
            LineWord::pair("BLOCK", s.damage, s.blocked)
        } else if s.absorb != 0 {
            LineWord::pair("ABSORB", s.damage, s.absorb)
        } else if s.hit_info & CRIT != 0 {
            LineWord::with("DAMAGE_CRIT", s.damage)
        } else {
            LineWord::with("DAMAGE", s.damage)
        };
        return Some((family, word));
    }
    // `0x62a710` words only a state its flag table `0x8628f8` marks (`0x62a720`) and its jump
    // table `0x62a8ec` covers: 0, 1 without damage, 4, 9 and 10 or more print nothing. Its word
    // table `0x62a908` gives each the matching type.
    let (family, word) = match s.victim_state {
        2 => (VSDODGE, "DODGE"),
        3 => (VSPARRY, "PARRY"),
        6 => (VSEVADE, "EVADE"),
        7 => (VSIMMUNE, "IMMUNE"),
        8 => (VSDEFLECT, "DEFLECT"),
        _ => return None,
    };
    Some((family, LineWord::bare(word)))
}

/// A direct spell hit's word, once damage got through (`0x62cd80`): the first of resisted
/// (damage and resisted, `0x62d048`), blocked (the blocked amount alone, `0x62d062`) and absorbed
/// (damage and absorbed, `0x62d07b`), else the damage (`0x62d098`). A crit has no word of its own.
pub(crate) fn spell_hit_word(damage: u32, absorbed: u32, resisted: i32, blocked: u32) -> LineWord {
    if resisted > 0 {
        LineWord::pair("SPELL_RESISTED", damage, resisted)
    } else if blocked != 0 {
        LineWord::with("SPELL_BLOCKED", blocked)
    } else if absorbed != 0 {
        LineWord::pair("SPELL_ABSORBED", damage, absorbed)
    } else {
        LineWord::with("SPELL_DAMAGE", damage)
    }
}

/// A periodic damage tick's word, once damage got through (`0x628100`), in the melee types: the
/// first of resisted (`0x628354`) and absorbed (`0x62836c`), else the damage (`0x62837f`).
pub(crate) fn periodic_hit_word(damage: u32, absorbed: u32, resisted: i32) -> LineWord {
    if resisted > 0 {
        LineWord::pair("RESIST", damage, resisted)
    } else if absorbed != 0 {
        LineWord::pair("ABSORB", damage, absorbed)
    } else {
        LineWord::with("DAMAGE", damage)
    }
}

/// A power gain's word (`0x627520`): the power's noun key (table `0x85645c`) compared against
/// `MANA_POINTS`, `RAGE_POINTS`, `FOCUS_POINTS` and `ENERGY_POINTS` (`0x6276f0`-`0x62779d`), each
/// its own type; `HAPPINESS_POINTS` matches none and has no word.
pub(crate) fn power_gain_word(power: u32, amount: i64) -> Option<LineWord> {
    let message_type = match power {
        0 => "MANA",
        1 => "RAGE",
        2 => "FOCUS",
        3 => "ENERGY",
        _ => return None,
    };
    Some(LineWord::with(message_type, amount))
}

#[cfg(test)]
mod tests;
