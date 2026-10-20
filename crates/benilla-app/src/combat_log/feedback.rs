//! `UNIT_COMBAT`, the portrait hit indicator, as the packet handlers raise it through `0x494600`:
//! `0x4946b0` a wound, `0x494720` a miss, `0x494740` a heal. Each is queued in the chat drain
//! ([`ChatLog::push_unit_combat`]) at the point its handler calls it, before or after that
//! handler's line, so Lua sees the two in the handler's order.

use crate::ui_chat::ChatLog;
use crate::ui_unit::UnitCombatFeedback;

/// `UNIT_COMBAT`'s descriptor, as its one emitter `0x494600` picks it from the packet's flags:
/// with an amount, `CRITICAL` (`0x80`), `GLANCING` (`0x4000`), `CRUSHING` (`0x8000`); without one,
/// `ABSORB` (`0x20`), `BLOCK` (`0x800`), `RESIST` (`0x40`); else empty, which the stock portrait
/// shows as "Miss" (`CombatFeedback.lua:57-58`).
pub(crate) fn wound_descriptor(amount: u32, flags: u32) -> &'static str {
    // The amount is tested signed (`0x49460a`, `jle`).
    let tests: [(u32, &str); 3] = if amount as i32 > 0 {
        [
            (0x80, "CRITICAL"),
            (0x4000, "GLANCING"),
            (0x8000, "CRUSHING"),
        ]
    } else {
        [(0x20, "ABSORB"), (0x800, "BLOCK"), (0x40, "RESIST")]
    };
    tests
        .into_iter()
        .find(|&(bit, _)| flags & bit != 0)
        .map_or("", |(_, word)| word)
}

/// The flags a spell hit hands `0x494600`: crit `0x80`, and each partial nonzero, absorbed `0x20`,
/// blocked `0x800`, resisted `0x40` (`0x5e87fa`-`0x5e8825`).
pub(super) fn hit_flags(crit: bool, absorb: u32, resist: i32, blocked: u32) -> u32 {
    let bit = |set: bool, bit: u32| if set { bit } else { 0 };
    bit(crit, 0x80) | bit(absorb != 0, 0x20) | bit(blocked != 0, 0x800) | bit(resist != 0, 0x40)
}

/// `0x4946b0(guid, amount, school, flags)`: `WOUND`, its descriptor off the flags.
pub(super) fn wound(log: &mut ChatLog, guid: u64, amount: u32, flags: u32, school: u32) {
    log.push_unit_combat(UnitCombatFeedback {
        guid,
        action: "WOUND",
        flags: wound_descriptor(amount, flags),
        amount,
        school,
    });
}

/// `0x494740(guid, amount, crit)`: `HEAL`, `CRITICAL` for a crit (mark `0x80`).
pub(super) fn heal(log: &mut ChatLog, guid: u64, amount: u32, crit: bool) {
    log.push_unit_combat(UnitCombatFeedback {
        guid,
        action: "HEAL",
        flags: wound_descriptor(amount, if crit { 0x80 } else { 0 }),
        amount,
        school: 0,
    });
}

/// Miss code (vmangos `SpellMissInfo`) to the `UNIT_COMBAT` action word, the table `0x86582c` that
/// `0x494720` indexes for a code from 0 to 11, `NONE` first; any other code fires nothing.
fn miss_action(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => "NONE",
        1 => "MISS",
        2 => "RESIST",
        3 => "DODGE",
        4 => "PARRY",
        5 => "BLOCK",
        6 => "EVADE",
        7 | 8 => "IMMUNE",
        9 => "DEFLECT",
        10 => "ABSORB",
        11 => "REFLECT",
        _ => return None,
    })
}

/// `0x494720(guid, code)`: a miss's `UNIT_COMBAT`, amount, school and flags all 0.
pub(super) fn miss(log: &mut ChatLog, guid: u64, code: u8) {
    if let Some(action) = miss_action(code) {
        log.push_unit_combat(UnitCombatFeedback {
            guid,
            action,
            flags: "",
            amount: 0,
            school: 0,
        });
    }
}
