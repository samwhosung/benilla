//! `UNIT_COMBAT`, the portrait hit indicator, as the handlers raise it through `0x494600`:
//! `0x4946b0` a wound, `0x4946d0` a swing, `0x494720` a miss, `0x494740` a heal. Each is queued in
//! the chat drain ([`ChatLog::push_unit_combat`]) at the point its handler calls it, before or after
//! that handler's line, so Lua sees the two in the handler's order.

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
/// blocked `0x800`, resisted `0x40` (`0x5e87fa`-`0x5e8825`); an environmental hit marks the two it
/// has the same way (`0x624f8c`-`0x624fa1`).
pub(super) fn hit_flags(crit: bool, absorb: u32, resist: i32, blocked: u32) -> u32 {
    let bit = |set: bool, bit: u32| if set { bit } else { 0 };
    bit(crit, 0x80) | bit(absorb != 0, 0x20) | bit(blocked != 0, 0x800) | bit(resist != 0, 0x40)
}

/// `0x4946b0(guid, amount, school, flags)`: `WOUND`, its descriptor off the flags.
pub(super) fn wound(log: &ChatLog, guid: u64, amount: u32, flags: u32, school: u32) {
    log.push_unit_combat(UnitCombatFeedback {
        guid,
        action: "WOUND",
        flags: wound_descriptor(amount, flags),
        amount,
        school,
    });
}

/// `0x494740(guid, amount, crit)`: `HEAL`, `CRITICAL` for a crit (mark `0x80`).
pub(super) fn heal(log: &ChatLog, guid: u64, amount: u32, crit: bool) {
    log.push_unit_combat(UnitCombatFeedback {
        guid,
        action: "HEAL",
        flags: wound_descriptor(amount, if crit { 0x80 } else { 0 }),
        amount,
        school: 0,
    });
}

/// A swing's `UNIT_COMBAT` action by its victim state, the nine-word table `0x83de28`.
const SWING_ACTIONS: [&str; 9] = [
    "MISS",
    "WOUND",
    "DODGE",
    "PARRY",
    "INTERRUPT",
    "BLOCK",
    "EVADE",
    "IMMUNE",
    "DEFLECT",
];

/// `0x4946d0(record)`: a swing's `UNIT_COMBAT`, its action the victim state's word (`0x494706`), its
/// descriptor off the damage and hit info like every wrapper's. The table index has no bound: a
/// state past it, which no server sends (vmangos `VictimState` ends at 8,
/// `Objects/UnitDefines.h:247`), signals nothing here.
pub(crate) fn swing(log: &ChatLog, guid: u64, victim_state: u32, hit_info: u32, damage: u32) {
    let Some(&action) = SWING_ACTIONS.get(victim_state as usize) else {
        return;
    };
    log.push_unit_combat(UnitCombatFeedback {
        guid,
        action,
        flags: wound_descriptor(damage, hit_info),
        amount: damage,
        // The reference passes sub-damage 0's school (`0x4946fc`), which the swing does not carry.
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
pub(super) fn miss(log: &ChatLog, guid: u64, code: u8) {
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

#[cfg(test)]
mod tests {
    use crate::ui_chat::ChatLog;

    /// A displayed swing on a held player, as `(action, descriptor)` of its `UNIT_COMBAT`.
    fn swing(victim_state: u32, hit_info: u32, damage: u32) -> Vec<(&'static str, &'static str)> {
        let log = ChatLog::default();
        let s = crate::creature_anim::SwingMessage {
            attacker: bevy::prelude::Entity::PLACEHOLDER,
            victim: None,
            hit_info,
            victim_state,
            damage,
            displayed: true,
            seq: 0,
        };
        let player =
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x19)]));
        crate::ui_unit::melee_unit_combat(&log, &s, Some((1, &player)));
        log.pending_unit_combat()
            .into_iter()
            .map(|f| (f.action, f.flags))
            .collect()
    }

    /// `0x4946d0` takes its action from the victim state through the nine-word table `0x83de28`
    /// (`0x494706`) and its descriptor from the amount and hit info alone (`0x494600`): an interrupt
    /// is `INTERRUPT`, state 0 `MISS` whatever the damage, state 1 `WOUND` with none, and a dodge
    /// keeps an absorb mark.
    #[test]
    fn a_swings_action_is_its_victim_states_word() {
        assert_eq!(swing(4, 0, 30), [("INTERRUPT", "")]);
        assert_eq!(swing(0, 0x80, 30), [("MISS", "CRITICAL")]);
        assert_eq!(swing(0, 0x10, 0), [("MISS", "")]);
        assert_eq!(swing(1, 0, 0), [("WOUND", "")]);
        assert_eq!(swing(1, 0x4000, 30), [("WOUND", "GLANCING")]);
        assert_eq!(swing(1, 0x40, 0), [("WOUND", "RESIST")]);
        assert_eq!(swing(2, 0x20, 0), [("DODGE", "ABSORB")]);
        assert_eq!(swing(3, 0, 0), [("PARRY", "")]);
        assert_eq!(swing(5, 0x800, 0), [("BLOCK", "BLOCK")]);
        assert_eq!(swing(6, 0, 0), [("EVADE", "")]);
        assert_eq!(swing(7, 0, 0), [("IMMUNE", "")]);
        assert_eq!(swing(8, 0, 0), [("DEFLECT", "")]);
        // Past the table, which `0x494706` indexes unbounded and no server sends.
        assert!(swing(9, 0, 0).is_empty());
    }
}
