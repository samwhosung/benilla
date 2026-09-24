//! `Lock.dbc`: the requirements of a lockable GameObject or item, up to 8 slots per lock. Opening
//! casts a known spell whose `SPELL_EFFECT_OPEN_LOCK` `EffectMiscValue` matches a skill slot's
//! `LockType` index, or uses an item slot's key. A `lockId` of 0, or a row of empty slots, is no
//! lock: the object opens by `CMSG_GAMEOBJ_USE` instead (the use handler `0x5f33e0`).
//!
//! 33 fields: `ID`, `Type[8]`, `Index[8]`, `Skill[8]`, `Action[8]` (vmangos `LockEntry`; the
//! reference reads `Index[0]` at `[lockRec+0x24]`, `0x5f84af`).

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};

const LOCK: &str = "DBFilesClient\\Lock.dbc";
/// The column count, which `benilla-dbc` checks against the header.
const LOCK_FIELDS: usize = 33;
/// A lock has up to 8 requirement slots (`MAX_LOCK_CASE`).
pub const MAX_LOCK_SLOTS: usize = 8;

/// `Type[i]`, a slot's key kind (vmangos `LockKeyType`): an empty slot.
pub const LOCK_KEY_NONE: u32 = 0;
/// A key-item slot; `LockSlot::index` is the item's entry.
pub const LOCK_KEY_ITEM: u32 = 1;
/// A skill slot; `LockSlot::index` is the `LockType.dbc` index the opening spell's
/// `EffectMiscValue` must match, `LockSlot::skill` the skill value it needs.
pub const LOCK_KEY_SKILL: u32 = 2;

/// One requirement slot; an all-zero slot is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LockSlot {
    /// `LOCK_KEY_NONE`, `LOCK_KEY_ITEM` or `LOCK_KEY_SKILL`.
    pub key_type: u32,
    /// Key item entry (item) or `LockType` index (skill); 0 when empty.
    pub index: u32,
    /// The skill value a skill slot needs. 0 is not free: the reference substitutes
    /// `GAMEOBJECT_LEVEL * 5` (`0x5f84be`).
    pub skill: u32,
    /// The slot's operation, which gates when it applies ([`LockSlot::available`]).
    pub action: u32,
}

/// `GAMEOBJECT_STATE` (vmangos `GOState`). The gates read the client's mirror at `go+0x27c`, not
/// the wire: a chest's lid opens client-side, so the two differ.
pub const GO_STATE_ACTIVE: u32 = 0;
pub const GO_STATE_READY: u32 = 1;
pub const GO_STATE_ACTIVE_ALTERNATIVE: u32 = 2;

impl LockSlot {
    /// Whether this slot applies to a GameObject now: the reference's per-slot gate `0x5f81d0`.
    /// Both legs of the lock resolver `0x5f83d0` skip a slot it refuses (`0x5f8450` for a skill
    /// slot, `0x5f8547` for a key), so that slot can neither satisfy the lock nor open it.
    /// `go_state` is the client's stored state (`go+0x27c`); `flag_locked` is `GO_FLAG_LOCKED`
    /// (`0x2`) in `GAMEOBJECT_FLAGS`.
    ///
    /// | Action | operation | applies when |
    /// |--------|-----------|--------------|
    /// | 0 | open | READY and `GO_FLAG_LOCKED` clear (`0x5f8212`-`0x5f8220`) |
    /// | 1 | unlock | READY and `GO_FLAG_LOCKED` set (`0x5f822d`-`0x5f823a`) |
    /// | 2 | close | ACTIVE (`0x5f8247`) |
    /// | 3 | | READY |
    /// | 4 | | ALTERNATIVE (`0x5f81ff`) |
    /// | other | | any state but ALTERNATIVE |
    ///
    /// ALTERNATIVE blocks every action but 4 (`0x5f81e1`). Without this gate a locked door opens
    /// on right-click: keyed doors carry a spare Quick Open skill slot (`LockType` 10, skill 0,
    /// Action 0) that spell 6247, Opening, which every character knows, satisfies.
    pub fn available(&self, go_state: u32, flag_locked: bool) -> bool {
        if self.action == 4 {
            return go_state == GO_STATE_ACTIVE_ALTERNATIVE;
        }
        if go_state == GO_STATE_ACTIVE_ALTERNATIVE {
            return false;
        }
        if matches!(self.action, 0 | 1 | 3) && go_state != GO_STATE_READY {
            return false;
        }
        match self.action {
            0 => !flag_locked,
            1 => flag_locked,
            2 => go_state == GO_STATE_ACTIVE,
            _ => true,
        }
    }
}

/// `lockId` to its 8 requirement slots.
pub struct LockCatalog {
    locks: HashMap<u32, [LockSlot; MAX_LOCK_SLOTS]>,
}

impl LockCatalog {
    /// A catalog from explicit rows, for tests and tools.
    pub fn from_rows(rows: impl IntoIterator<Item = (u32, [LockSlot; MAX_LOCK_SLOTS])>) -> Self {
        Self {
            locks: rows.into_iter().collect(),
        }
    }

    /// A `lockId`'s slots; an absent id is no lock, and a row may still be all empty:
    /// [`LockCatalog::is_locked`] is the must-cast test.
    pub fn slots(&self, lock_id: u32) -> Option<&[LockSlot; MAX_LOCK_SLOTS]> {
        self.locks.get(&lock_id)
    }

    /// Whether the object opens by a cast rather than `CMSG_GAMEOBJ_USE`: a present row with at
    /// least one non-empty slot.
    pub fn is_locked(&self, lock_id: u32) -> bool {
        lock_id != 0
            && self
                .locks
                .get(&lock_id)
                .is_some_and(|s| s.iter().any(|slot| slot.key_type != LOCK_KEY_NONE))
    }

    pub fn len(&self) -> usize {
        self.locks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.locks.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("Lock");
    for i in 0..LOCK_FIELDS {
        s.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32));
    }
    s
}

/// Load `Lock.dbc` off the patch chain.
pub fn load_lock_catalog(chain: &mut Chain) -> Result<LockCatalog> {
    let bytes = chain
        .read_file(LOCK)
        .with_context(|| format!("reading {LOCK}"))?;
    let rs = parse(&bytes, schema(), "Lock.dbc")?;
    let mut locks = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let mut slots = [LockSlot::default(); MAX_LOCK_SLOTS];
        for (i, slot) in slots.iter_mut().enumerate() {
            slot.key_type = u32_at(r, 1 + i).unwrap_or(0);
            slot.index = u32_at(r, 9 + i).unwrap_or(0);
            slot.skill = u32_at(r, 17 + i).unwrap_or(0);
            slot.action = u32_at(r, 25 + i).unwrap_or(0);
        }
        locks.insert(id, slots);
    }
    Ok(LockCatalog { locks })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_lock_catalog_reads_skill_slots() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_lock_catalog(&mut chain).expect("load Lock.dbc");
        assert!(!cat.is_empty(), "Lock.dbc parsed empty");

        // Copper Vein (gameobject 1731, lock 38), Mining: `Type[0]` 2 at column 1, `Index[0]` 3 at
        // column 9, and `Skill[0]` 0 at column 17, as on every gathering node.
        let vein = cat.slots(38).expect("lockId 38 (Copper Vein)");
        assert_eq!(
            vein[0].key_type, LOCK_KEY_SKILL,
            "copper vein is a skill lock"
        );
        assert_eq!(vein[0].index, 3, "Mining is LockType index 3");
        assert_eq!(
            vein[0].skill, 0,
            "gathering nodes carry no Skill value in Lock.dbc"
        );
        assert!(
            vein[1..].iter().all(|s| s.key_type == LOCK_KEY_NONE),
            "one slot only"
        );
        assert!(
            cat.is_locked(38),
            "a mining vein is a real lock (must cast, not USE)"
        );

        // Silverleaf (gameobject 1617, lock 29), a Herbalism lock: `LockType` index 2.
        let herb = cat.slots(29).expect("lockId 29 (Silverleaf)");
        assert_eq!(
            herb[0].key_type, LOCK_KEY_SKILL,
            "silverleaf is a skill lock"
        );
        assert_eq!(herb[0].index, 2, "Herbalism is LockType index 2");
        assert_ne!(herb[0].index, vein[0].index, "herbalism ≠ mining LockType");

        assert!(!cat.is_locked(0));
    }

    /// Lock 43, the keyless chests: one skill slot naming `LockType` 13, which spell 6478, Opening,
    /// known to every character, opens.
    #[test]
    fn real_lock_catalog_reads_keyless_chest() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_lock_catalog(&mut chain).expect("load Lock.dbc");

        let chest = cat.slots(43).expect("lockId 43 (simple chest)");
        assert!(
            cat.is_locked(43),
            "even a keyless chest is a real lock (cast, not USE)"
        );
        // The requirement sits in slot 1, slot 0 empty: the routing must scan all 8 slots.
        assert_eq!(chest[1].key_type, LOCK_KEY_SKILL);
        assert_eq!(
            chest[1].index, 13,
            "the keyless-chest LockType spell 6478 opens"
        );
        // An Action 0 slot: the chest opens because chests carry no `GO_FLAG_LOCKED`.
        assert_eq!(chest[1].action, 0);
        assert!(chest[1].available(GO_STATE_READY, false));
        assert!(!chest[1].available(GO_STATE_READY, true));
    }

    /// The `Action` column pinned by value on two door rows: a slip would let locked doors open.
    #[test]
    fn real_lock_catalog_reads_the_action_column() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_lock_catalog(&mut chain).expect("load Lock.dbc");

        // Scholomance Door (gameobject 174626, lock 1159, flags 34: `GO_FLAG_LOCKED | NODESPAWN`):
        // the Skeleton Key, Pick Lock 280, and the spares Quick Open, Quick Close and Blasting.
        let scholo = cat.slots(1159).expect("lockId 1159 (Scholomance Door)");
        assert_eq!(
            (scholo[0].key_type, scholo[0].index, scholo[0].action),
            (LOCK_KEY_ITEM, 13704, 1),
            "slot 0 = Skeleton Key, Action 1 (unlock)"
        );
        assert_eq!(
            (
                scholo[1].key_type,
                scholo[1].index,
                scholo[1].skill,
                scholo[1].action
            ),
            (LOCK_KEY_SKILL, 1, 280, 1),
            "slot 1 = Pick Lock 280, Action 1 (unlock)"
        );
        assert_eq!(
            (
                scholo[2].key_type,
                scholo[2].index,
                scholo[2].skill,
                scholo[2].action
            ),
            (LOCK_KEY_SKILL, 10, 0, 0),
            "slot 2 = Quick Open, no skill, Action 0 (open) — THE bug's slot"
        );
        // On a locked door Quick Open does not apply, so spell 6247 cannot satisfy it; the key and
        // Pick Lock do.
        assert!(
            !scholo[2].available(GO_STATE_READY, true),
            "Quick Open is gated out by the flag"
        );
        assert!(
            scholo[0].available(GO_STATE_READY, true),
            "the key still applies"
        );
        assert!(
            scholo[1].available(GO_STATE_READY, true),
            "Pick Lock still applies"
        );

        // The Searing Gorge gate (gameobject 150137 and 150138, lock 84) has no Action 0 slot.
        let gorge = cat.slots(84).expect("lockId 84 (Searing Gorge gate)");
        assert_eq!(
            (gorge[0].key_type, gorge[0].index, gorge[0].action),
            (LOCK_KEY_ITEM, 5396, 1),
            "slot 0 = Key to the Searing Gorge"
        );
        assert_eq!(
            (
                gorge[1].key_type,
                gorge[1].index,
                gorge[1].skill,
                gorge[1].action
            ),
            (LOCK_KEY_SKILL, 1, 225, 1),
            "slot 1 = Pick Lock 225"
        );
        assert!(
            gorge[2..].iter().all(|s| s.key_type == LOCK_KEY_NONE),
            "no third slot — no Quick Open spare"
        );
    }

    /// [`LockSlot::available`] against `0x5f81d0`'s branch table, one row per arm.
    #[test]
    fn action_gate_matches_the_reference_branch_table() {
        let slot = |action| LockSlot {
            key_type: LOCK_KEY_SKILL,
            index: 1,
            skill: 0,
            action,
        };
        assert!(slot(0).available(GO_STATE_READY, false));
        assert!(!slot(0).available(GO_STATE_READY, true));
        assert!(!slot(0).available(GO_STATE_ACTIVE, false));
        assert!(slot(1).available(GO_STATE_READY, true));
        assert!(!slot(1).available(GO_STATE_READY, false));
        assert!(!slot(1).available(GO_STATE_ACTIVE, true));
        assert!(slot(2).available(GO_STATE_ACTIVE, false));
        assert!(slot(2).available(GO_STATE_ACTIVE, true));
        assert!(!slot(2).available(GO_STATE_READY, false));
        assert!(slot(3).available(GO_STATE_READY, true));
        assert!(!slot(3).available(GO_STATE_ACTIVE, false));
        assert!(slot(4).available(GO_STATE_ACTIVE_ALTERNATIVE, false));
        assert!(!slot(4).available(GO_STATE_READY, false));
        for action in [0, 1, 2, 3, 5, 19] {
            assert!(
                !slot(action).available(GO_STATE_ACTIVE_ALTERNATIVE, false),
                "action {action} must not apply in the ALTERNATIVE state"
            );
        }
        assert!(slot(5).available(GO_STATE_ACTIVE, false));
        assert!(slot(5).available(GO_STATE_READY, true));
    }
}
