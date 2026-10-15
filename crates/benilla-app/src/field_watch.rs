//! The field watches on units and the active player whose callbacks signal interface events. The
//! object packets write one [`FieldNotice`] per callback run, in wire order, and the unit feed
//! fires each (`crate::ui_unit`), so an event follows the field change that ran its watch and
//! nothing else: not a login, not a `/reload`, not a value read for the first time.
//!
//! A values block runs each watch whose bytes now differ from the copy taken before the block was
//! written, once, at the first sent field of its range (`0x465570` sets the record's countdown to
//! 1; `repz cmpsb` at `0x4655bb`), so the notices of one block come out in field order. The
//! notify pass `0x465330` has two callers, a values block (`0x465277`) and a create of an object
//! that already exists (`0x465d8e`); a first create runs none (`0x465d80 je 0x465d98`), and a
//! `/reload` receives no update at all.
//!
//! The watches here: the unit-field bridge `0x51bbb0` on every unit with token flags, the active
//! player's `0x5de4b0` and rest-state watches registered in `0x5dd8a0`, its skill watches
//! `0x5de180` and `0x5de470`, and the weapon arms of the visible-item watch `0x5e2810`.

use benilla_protocol::messages::ObjectType;
use benilla_protocol::ObjectFields;
use bevy::prelude::*;

/// One run of a reference field watch whose callback signals interface events, in the order the
/// packets ran them.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldNotice {
    /// The unit-field bridge `0x51bd50`: event `id`, the watched field's byte offset in the unit
    /// section over 4 (`0x51bd55`), through `0x515e50` once per token naming `guid`.
    Unit { guid: u64, id: u16 },
    /// `0x5de4b0` on the active player: the event its registration pushes as the argument
    /// (`0x5de4b3`), with `"player"` (`0x5de4c1`).
    Player { id: u16 },
    /// `0x5de4e0` on `PLAYER_BYTES_2` byte 3: `UPDATE_EXHAUSTION` bare (`0x5de4e8`), then the
    /// rest-state line for the new state.
    RestState { old: u8, new: u8 },
    /// `0x5ed990`: `UNIT_ATTACK`, `UNIT_DEFENSE` and `UNIT_RANGED_ATTACK_POWER`, each with
    /// `"player"`, from the skill value watch `0x5de180` (`0x5de214`) or the temporary-bonus watch
    /// `0x5de470` (`0x5de495`).
    CombatSkill,
    /// The weapon arms of `0x5dee30` for the active player (`0x5deeae`): main hand (15) and off
    /// hand (16) signal `UNIT_ATTACK` through `0x5ed970`, ranged (17) `UNIT_RANGED_ATTACK_POWER`
    /// (`0x5deecb`).
    Weapon { slot: u8 },
}

/// The first field of the unit and the player sections: their byte offsets in the field array,
/// 0x18 and 0x2f0 (`0x465690`), over 4.
const UNIT_SECTION: u16 = 6;
const PLAYER_SECTION: u16 = 188;

/// The bridge ids whose events fire through [`FieldNotice::Unit`]: the attack times, damage,
/// stat, resistance and attack-power fields. `0x51bbb0` registers each as a 4-byte watch (its
/// length selector `0x51bc98` gives 4 for all of them); the other named ids of the event table
/// still fire from the unit feed's snapshot diff.
pub(crate) const BRIDGE_IDS: [u16; 27] = [
    120, 121, 122, 128, 129, 130, 131, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155,
    159, 160, 161, 162, 163, 164, 165, 166,
];

/// `0x5de4b0`'s registrations in `0x5dd8a0`, in registration order: the byte offset in the player
/// section, the length, and the event id pushed as the argument.
const PLAYER_EVENT_WATCHES: [(u16, u16, u16); 11] = [
    // `UPDATE_EXHAUSTION`, the rest pool (`0x5ddb24`).
    (0xf6c, 4, 406),
    // `PLAYER_XP_UPDATE`, the XP and the next level's (`0x5ddb4b`, `0x5ddb72`).
    (0x840, 4, 189),
    (0x844, 4, 189),
    // `UNIT_RESISTANCES`, both resistance buff-mod arrays as one watch (`0x5ddb99`).
    (0xf9c, 0x38, 149),
    // `PLAYER_DAMAGE_DONE_MODS`, the positive, negative and percent arrays (`0x5ddbc0`,
    // `0x5ddbe7`, `0x5ddc0e`).
    (0xfd4, 0x1c, 201),
    (0xff0, 0x1c, 201),
    (0x100c, 0x1c, 201),
    // `PLAYER_PVP_KILLS_CHANGED`, the session kills (`0x5ddc35`).
    (0x1098, 4, 523),
    // `PLAYER_PVP_RANK_CHANGED`, the whole `PLAYER_BYTES_3` and `PLAYER_FIELD_BYTES2` byte 0
    // (`0x5ddc5c`, `0x5ddc83`).
    (0x1c, 4, 524),
    (0x10c0, 1, 524),
    // `UPDATE_FACTION`, the watched faction index (`0x5ddcce`).
    (0x10c4, 4, 372),
];

/// `0x5de4e0`'s range, `PLAYER_BYTES_2` byte 3 (`0x5ddb0b`).
const REST_STATE: u16 = 0x1b;

/// The skill block: entry `k` at `0x848 + 12k` of the player section, 128 entries (`0x5dda54` to
/// `0x5ddaf0`). The value watch covers the value word at `+4` and the temporary-bonus watch the
/// word at `+8`, two bytes each.
const SKILL_VALUE: u16 = 0x84c;
const SKILL_TEMP_BONUS: u16 = 0x850;
const SKILL_STRIDE: u16 = 12;
const SKILLS: u16 = 128;

/// The visible-item watch's ranges, `0x118 + 0x30·slot` of the player section, 0x30 bytes
/// (`0x5e2541`), for the three slots `0x5dee30` signals on.
const VISIBLE_ITEM: u16 = 0x118;
const VISIBLE_STRIDE: u16 = 0x30;
const WEAPON_SLOTS: [u8; 3] = [15, 16, 17];

/// One values block's changed dwords, `(index, old, new)` ascending, as the store's merge reports
/// them; an absent dword did not change.
struct Block<'a>(&'a [(u16, u32, u32)]);

impl Block<'_> {
    /// The dword `index` before and after the block, when it changed.
    fn edge(&self, index: u16) -> Option<(u32, u32)> {
        self.0
            .binary_search_by_key(&index, |&(i, _, _)| i)
            .ok()
            .map(|at| (self.0[at].1, self.0[at].2))
    }

    /// Whether any byte of `len` bytes at byte `offset` of the section starting at field `base`
    /// differs from the copy taken before the block: the watch's `repz cmpsb` (`0x4655bb`).
    fn moved(&self, base: u16, offset: u16, len: u16) -> bool {
        let (start, end) = (u32::from(offset), u32::from(offset) + u32::from(len));
        let below = |n: u32| {
            if n >= 4 {
                u32::MAX
            } else {
                (1u32 << (8 * n)) - 1
            }
        };
        let mut at = start & !3;
        while at < end {
            if let Some((old, new)) = self.edge(base + (at / 4) as u16) {
                let mask = below(end.min(at + 4) - at) & !below(start.max(at) - at);
                if (old ^ new) & mask != 0 {
                    return true;
                }
            }
            at += 4;
        }
        false
    }
}

/// The notices of one values block on `guid`, created as `kind`: `fields` is the store after the
/// merge and `edges` the changed dwords as `(index, old, new)`, ascending. `me` is the active
/// player's guid. Sorted by the first field of each watch's range, which for these disjoint
/// ranges is the notify pass's order; a stable sort keeps registration order within one field.
pub(crate) fn values(
    me: Option<u64>,
    guid: u64,
    kind: ObjectType,
    fields: &ObjectFields,
    edges: &[(u16, u32, u32)],
) -> Vec<FieldNotice> {
    let block = Block(edges);
    if edges.is_empty() || !matches!(kind, ObjectType::Unit | ObjectType::Player) {
        return Vec::new();
    }
    let mut out: Vec<(u16, FieldNotice)> = Vec::new();
    for id in BRIDGE_IDS {
        if block.moved(UNIT_SECTION, 4 * id, 4) {
            out.push((UNIT_SECTION + id, FieldNotice::Unit { guid, id }));
        }
    }
    if kind == ObjectType::Player && me == Some(guid) {
        let first = |offset: u16| PLAYER_SECTION + offset / 4;
        if let Some((old, new)) = block.edge(first(REST_STATE)) {
            let byte = |v: u32| (v >> (8 * (REST_STATE % 4))) as u8;
            if byte(old) != byte(new) {
                out.push((
                    first(REST_STATE),
                    FieldNotice::RestState {
                        old: byte(old),
                        new: byte(new),
                    },
                ));
            }
        }
        for (offset, len, id) in PLAYER_EVENT_WATCHES {
            if block.moved(PLAYER_SECTION, offset, len) {
                out.push((first(offset), FieldNotice::Player { id }));
            }
        }
        for k in 0..SKILLS {
            let value = SKILL_VALUE + SKILL_STRIDE * k;
            if let Some((old, _)) = block.edge(first(value)) {
                if block.moved(PLAYER_SECTION, value, 2) && skill_value_signals(fields, k, old) {
                    out.push((first(value), FieldNotice::CombatSkill));
                }
            }
            let bonus = SKILL_TEMP_BONUS + SKILL_STRIDE * k;
            if block.moved(PLAYER_SECTION, bonus, 2) {
                out.push((first(bonus), FieldNotice::CombatSkill));
            }
        }
        for slot in WEAPON_SLOTS {
            let offset = VISIBLE_ITEM + VISIBLE_STRIDE * u16::from(slot);
            if block.moved(PLAYER_SECTION, offset, VISIBLE_STRIDE) {
                out.push((first(offset), FieldNotice::Weapon { slot }));
            }
        }
    }
    out.sort_by_key(|&(at, _)| at);
    out.into_iter().map(|(_, n)| n).collect()
}

/// `0x5de180`'s gate ahead of its `0x5ed990` call: the new shown value, the value plus the
/// permanent bonus when the value is above 0 (`0x5de1f8`, `0x5de207`), against the old value word
/// the watch passes (`0x5de18f`), equal skipping to the end (`0x5de20c`). `old` is the value's
/// whole dword before the block.
fn skill_value_signals(fields: &ObjectFields, k: u16, old: u32) -> bool {
    let Some(slot) = fields.player_skill(k as u8) else {
        return true;
    };
    let value = u32::from(slot.value);
    let shown = if value > 0 {
        value + u32::from(slot.perm_bonus as u16)
    } else {
        0
    };
    shown != old & 0xffff
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u64 = 0x07;
    const MOB: u64 = 0xF130_0000_0000_0001;

    /// Merge `delta` into `store` and run the watches on what moved, as the object handlers do.
    fn block(
        store: &mut ObjectFields,
        me: Option<u64>,
        guid: u64,
        delta: &[(u16, u32)],
    ) -> Vec<FieldNotice> {
        let mut edges = Vec::new();
        store.merge_diff(ObjectFields::from_pairs(delta), |i, o, n| {
            edges.push((i, o, n))
        });
        let kind = store.created_as().expect("created");
        values(me, guid, kind, store, &edges)
    }

    fn player() -> ObjectFields {
        ObjectFields::from_pairs(&[]).into_created(ObjectType::Player)
    }

    /// The field of player-section byte `offset`.
    fn pf(offset: u16) -> u16 {
        PLAYER_SECTION + offset / 4
    }

    /// The bridge's event id names the field `id + 6`; each moved field is one notice, in field
    /// order, and an equal rewrite runs nothing.
    #[test]
    fn each_bridge_field_is_one_watch_in_field_order() {
        let mut mob = ObjectFields::from_pairs(&[]).into_created(ObjectType::Unit);
        // All five stats and one resistance in one block.
        let delta: Vec<(u16, u32)> = (150..156).map(|f| (f, 7)).collect();
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &delta),
            (144..150)
                .map(|id| FieldNotice::Unit { guid: MOB, id })
                .collect::<Vec<_>>()
        );
        assert!(
            block(&mut mob, Some(ME), MOB, &delta).is_empty(),
            "nothing moved"
        );
        // The attack power multipliers are watched (161, 164); the stat bonus arrays are not.
        assert_eq!(
            block(
                &mut mob,
                Some(ME),
                MOB,
                &[(167, 1.1f32.to_bits()), (170, 1.2f32.to_bits())]
            ),
            [
                FieldNotice::Unit { guid: MOB, id: 161 },
                FieldNotice::Unit { guid: MOB, id: 164 }
            ]
        );
        // `UNIT_FIELD_HEALTH` (id 16) fires from the feed's diff, not here.
        assert!(block(&mut mob, Some(ME), MOB, &[(22, 90)]).is_empty());
    }

    /// The active player's own watches run for it alone, and in field order across sections.
    #[test]
    fn the_player_watches_run_for_the_active_player_alone() {
        let mut me = player();
        let mut other = player();
        let xp = pf(0x840);
        let delta = [(xp, 100), (pf(0x10c4), 3)];
        assert_eq!(
            block(&mut me, Some(ME), ME, &delta),
            [
                FieldNotice::Player { id: 189 },
                FieldNotice::Player { id: 372 }
            ]
        );
        assert!(block(&mut other, Some(ME), 0x08, &delta).is_empty());
        // A level-up moves both XP fields: two watches.
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(xp, 0), (xp + 1, 400)]),
            [FieldNotice::Player { id: 189 }; 2]
        );
        // The last rested kill moves the rest byte (194), the XP (716) and the pool (1175): field
        // order, not the registration order that has the pool ahead of the XP.
        let (rest, pool) = (pf(0x1b), pf(0xf6c));
        block(&mut me, Some(ME), ME, &[(rest, 1 << 24), (pool, 30)]);
        assert_eq!(
            block(
                &mut me,
                Some(ME),
                ME,
                &[(rest, 2 << 24), (xp, 60), (pool, 0)]
            ),
            [
                FieldNotice::RestState { old: 1, new: 2 },
                FieldNotice::Player { id: 189 },
                FieldNotice::Player { id: 406 }
            ]
        );
        // A stat (unit section) before the XP (player section) in one block.
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(150, 20), (xp, 10)]),
            [
                FieldNotice::Unit { guid: ME, id: 144 },
                FieldNotice::Player { id: 189 }
            ]
        );
    }

    /// The rank bar watch is one byte of `PLAYER_FIELD_BYTES2`, the rest-state watch one byte of
    /// `PLAYER_BYTES_2`: the other bytes of their dwords run neither.
    #[test]
    fn a_byte_watch_reads_its_own_byte_only() {
        let mut me = player();
        let bytes2 = pf(0x10c0);
        assert!(block(&mut me, Some(ME), ME, &[(bytes2, 0x0100)]).is_empty());
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(bytes2, 0x0105)]),
            [FieldNotice::Player { id: 524 }]
        );
        let rest = pf(0x1b);
        assert!(block(&mut me, Some(ME), ME, &[(rest, 0x00ff_ffff)]).is_empty());
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(rest, 0x02ff_ffff)]),
            [FieldNotice::RestState { old: 0, new: 2 }]
        );
        // The whole `PLAYER_BYTES_3` is the rank watch: its gender byte runs it.
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(pf(0x1c), 1)]),
            [FieldNotice::Player { id: 524 }]
        );
    }

    /// A multi-dword watch runs once however many of its dwords moved; the stat bonus arrays
    /// between the coinage and the resistance mods have no watch.
    #[test]
    fn a_wide_watch_runs_once_per_block() {
        let mut me = player();
        let mods = pf(0xf9c);
        assert_eq!(
            block(
                &mut me,
                Some(ME),
                ME,
                &[(mods, 5), (mods + 3, 9), (mods + 13, 2)]
            ),
            [FieldNotice::Player { id: 149 }]
        );
        // `PLAYER_FIELD_POSSTAT0` .. `NEGSTAT4`, fields 1177-1186.
        let bonus: Vec<(u16, u32)> = (1177..1187).map(|f| (f, 3)).collect();
        assert!(block(&mut me, Some(ME), ME, &bonus).is_empty());
    }

    /// The value watch signals when the shown value differs from the old value word; the
    /// temporary-bonus watch on any change; the maximum and permanent bonus run neither.
    #[test]
    fn the_skill_watches_reach_the_combat_signals() {
        let mut me = player();
        let value = pf(SKILL_VALUE);
        let bonus = pf(SKILL_TEMP_BONUS);
        // Skill 95 (Defense) in entry 0 at 1/300.
        block(
            &mut me,
            Some(ME),
            ME,
            &[(value - 1, 95), (value, (300 << 16) | 1)],
        );
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(value, (300 << 16) | 2)]),
            [FieldNotice::CombatSkill]
        );
        // The maximum alone (`0x5de450`), and the permanent bonus alone, signal nothing.
        assert!(block(&mut me, Some(ME), ME, &[(value, (305 << 16) | 2)]).is_empty());
        assert!(block(&mut me, Some(ME), ME, &[(bonus, 5 << 16)]).is_empty());
        // The temporary bonus, in entry 1.
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(bonus + 3, 10)]),
            [FieldNotice::CombatSkill]
        );
        // With a permanent bonus of 5: a value of 2 → 7 shows 12 against the old word 2, and a
        // drop of exactly the bonus, 7 → 2, shows 7, the old word itself, so the gate skips it.
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(value, (305 << 16) | 7)]),
            [FieldNotice::CombatSkill]
        );
        assert!(block(&mut me, Some(ME), ME, &[(value, (305 << 16) | 2)]).is_empty());
    }

    /// The visible-item watch signals on the three weapon slots only, on any of their 12 dwords.
    #[test]
    fn the_weapon_slots_signal_on_any_dword() {
        let mut me = player();
        let slot = |n: u16| pf(VISIBLE_ITEM + VISIBLE_STRIDE * n);
        assert_eq!(
            block(
                &mut me,
                Some(ME),
                ME,
                &[(slot(15) + 2, 2504), (slot(16) + 5, 1), (slot(17) + 11, 3)]
            ),
            [
                FieldNotice::Weapon { slot: 15 },
                FieldNotice::Weapon { slot: 16 },
                FieldNotice::Weapon { slot: 17 }
            ]
        );
        // The chest (slot 4) and a ring (10) do not reach it.
        assert!(block(
            &mut me,
            Some(ME),
            ME,
            &[(slot(4) + 2, 1), (slot(10) + 2, 1)]
        )
        .is_empty());
    }
}
