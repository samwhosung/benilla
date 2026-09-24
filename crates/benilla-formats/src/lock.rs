//! Lock.dbc — the requirements a lockable GameObject (or item) carries. A
//! GameObject's template `lockId` (a type-specific slot of its query `data[]`) indexes this table;
//! each lock has up to 8 requirement **slots**. Interacting with a locked object casts a *known*
//! spell whose `SPELL_EFFECT_OPEN_LOCK` `EffectMiscValue` matches a **skill** slot's `LockType`
//! index (mining / herbalism / lockpicking), or consumes the **item** slot's key. A `lockId` of 0,
//! or a row whose every slot is empty, means "no lock" — the object opens by `CMSG_GAMEOBJ_USE`
//! instead of a cast (the split in the use handler `0x5f33e0`).
//!
//! Layout verified against build 5875 (mangos `LockEntry`, `DBCStructure.h`, and the reference's
//! `[lockRec+0x24]` = `Index[0]` = column 9 at `0x5f84af`): **33 fields** — `ID@0`,
//! `Type[8]@1..8`, `Index[8]@9..16`, `Skill[8]@17..24`, `Action[8]@25..32`.
//!
//! **`Action` is a gate, not a label**. Before the client's lock resolver
//! (`0x5f83d0`) will even *consider* a slot, it asks `0x5f81d0(gameObject, Action[i])` — a
//! predicate over the GameObject's own **state** and its `GO_FLAG_LOCKED` wire bit. See
//! [`LockSlot::available`]. Skipping it is why "any locked door opens on right-click": nearly every
//! keyed door in 5875 carries a spare `Quick Open` slot with `Action = 0`, and Action 0 means
//! *"only when the object is NOT flagged locked"*.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};

const LOCK: &str = "DBFilesClient\\Lock.dbc";
/// The file's column count (must equal the DBC header `field_count` — `benilla-dbc` enforces it).
const LOCK_FIELDS: usize = 33;
/// A lock has up to 8 requirement slots (`MAX_LOCK_CASE`).
pub const MAX_LOCK_SLOTS: usize = 8;

/// `Lock.dbc` `Type[i]` — a slot's key kind (mangos `LockKeyType`).
pub const LOCK_KEY_NONE: u32 = 0;
/// The slot is opened by holding a **key item**; `LockSlot::index` is that item's entry.
pub const LOCK_KEY_ITEM: u32 = 1;
/// The slot is opened by a **skill** (mining / herbalism / lockpicking); `LockSlot::index` is the
/// `LockType.dbc` index the opener spell's `EffectMiscValue` must match, `LockSlot::skill` the
/// required skill value.
pub const LOCK_KEY_SKILL: u32 = 2;

/// One of a lock's up-to-8 requirement slots. An all-zero slot is empty (unused).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LockSlot {
    /// `LOCK_KEY_NONE` / `LOCK_KEY_ITEM` / `LOCK_KEY_SKILL`.
    pub key_type: u32,
    /// Key item entry (ITEM) or `LockType` index (SKILL); `0` when empty.
    pub index: u32,
    /// Required skill value (SKILL slots); `0` otherwise — and `0` does **not** mean "free": the
    /// client substitutes `GAMEOBJECT_LEVEL × 5` for it (`0x5f84be`).
    pub skill: u32,
    /// `Action[i]` — which *operation* this slot performs, and the gate on when it applies.
    /// See [`LockSlot::available`].
    pub action: u32,
}

/// `GAMEOBJECT_STATE` (vmangos `GOState`) — the client mirrors it at `go+0x27c` and every gate
/// below reads that mirror, not the wire (a chest's lid is opened client-side, so the two differ).
pub const GO_STATE_ACTIVE: u32 = 0;
pub const GO_STATE_READY: u32 = 1;
pub const GO_STATE_ACTIVE_ALTERNATIVE: u32 = 2;

impl LockSlot {
    /// Whether this slot applies to a GameObject right now — the client's per-slot gate
    /// **`0x5f81d0(this = GO, Action[i])`**, byte-transcribed. Both legs of the
    /// lock resolver `0x5f83d0` call it (`0x5f8450` for a SKILL slot, `0x5f8547` for a KEY slot)
    /// and **skip the slot** when it answers false, so a gated-out slot can neither satisfy the
    /// lock nor be opened.
    ///
    /// `go_state` is the client's stored state (`go+0x27c`, our `GoAnim::state`); `flag_locked` is
    /// `GAMEOBJECT_FLAGS & GO_FLAG_LOCKED (0x2)`. The `Action` values are operations:
    ///
    /// | Action | operation | applies when |
    /// |--------|-----------|--------------|
    /// | 0 | open | state READY **and** `GO_FLAG_LOCKED` **clear** (`0x5f8212..0x5f8220`) |
    /// | 1 | unlock | state READY **and** `GO_FLAG_LOCKED` **set** (`0x5f822d..0x5f823a`) |
    /// | 2 | close | state ACTIVE (`0x5f8247`) |
    /// | 3 | (state-only) | state READY |
    /// | 4 | (alt-state) | state ALTERNATIVE (`0x5f81ff`) |
    /// | other | — | any state but ALTERNATIVE |
    ///
    /// ALTERNATIVE (2) blocks every action but 4 (`0x5f81e1`).
    ///
    /// **This is the whole of the "any locked door opens on right-click" report.** The keyed doors
    /// (Scholomance 1159, Shadowforge 680, Stratholme 879, SM 299, Deadmines 202 …) each carry a
    /// spare `Quick Open` SKILL slot — `LockType 10`, `Skill 0`, **`Action 0`** — and *every*
    /// character knows spell 6247 "Opening", which opens LockType 10 with an effect value of 100.
    /// Without this gate that slot satisfies the lock and the door opens. With it, Action 0 is
    /// refused the moment `GO_FLAG_LOCKED` is set — which every one of those doors sets. The
    /// Searing Gorge gate (lock 84) was the reporter's counter-example precisely because it has no
    /// `Action 0` slot at all: only the key (Action 1) and Pick Lock (Action 1).
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

/// `lockId → its 8 requirement slots`, from Lock.dbc.
pub struct LockCatalog {
    locks: HashMap<u32, [LockSlot; MAX_LOCK_SLOTS]>,
}

impl LockCatalog {
    /// Build a catalog from explicit rows — tests and tools; the game path is
    /// [`load_lock_catalog`].
    pub fn from_rows(rows: impl IntoIterator<Item = (u32, [LockSlot; MAX_LOCK_SLOTS])>) -> Self {
        Self {
            locks: rows.into_iter().collect(),
        }
    }

    /// The 8 requirement slots for a `lockId`, or `None` if the id isn't in the table (treat as no
    /// lock). A returned row may still be all-empty — [`LockCatalog::is_locked`] is the "must cast"
    /// test.
    pub fn slots(&self, lock_id: u32) -> Option<&[LockSlot; MAX_LOCK_SLOTS]> {
        self.locks.get(&lock_id)
    }

    /// Whether a `lockId` names a real lock (present, with at least one non-empty slot) — i.e. the
    /// object opens by a cast, not `CMSG_GAMEOBJ_USE`. A `0` id or an absent/all-empty row is "no
    /// lock".
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

/// Load Lock.dbc from the patch chain into a [`LockCatalog`].
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

    /// The Type/Index/Skill column offsets on the real build-5875 `Lock.dbc` — a column slip fails
    /// loudly. Anchors: Copper Vein (lockId 38, a Mining node) and Silverleaf (lockId 29, an
    /// Herbalism node), both skill 1 — the lowest of their profession. Skips without client data.
    #[test]
    fn real_lock_catalog_reads_skill_slots() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_lock_catalog(&mut chain).expect("load Lock.dbc");
        assert!(!cat.is_empty(), "Lock.dbc parsed empty");

        // Copper Vein (gameobject_template 1731, chest.data0 = lockId 38): a MINING skill lock. The
        // real row is `{Type[0]=2 SKILL, Index[0]=3 (Mining LockType), Skill[0]=0}` — `Type[0]` at
        // col 1, `Index[0]` at col 9, `Skill[0]` at col 17; a column slip lands elsewhere. (Lock.dbc
        // stores 0 in `Skill` for gathering nodes — the profession *spell* is the gate, not a value
        // here; the server enforces the node's grey level.)
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

        // Silverleaf (gameobject_template 1617, lockId 29): an HERBALISM skill lock — LockType index
        // 2, distinct from mining's 3.
        let herb = cat.slots(29).expect("lockId 29 (Silverleaf)");
        assert_eq!(
            herb[0].key_type, LOCK_KEY_SKILL,
            "silverleaf is a skill lock"
        );
        assert_eq!(herb[0].index, 2, "Herbalism is LockType index 2");
        assert_ne!(herb[0].index, vein[0].index, "herbalism ≠ mining LockType");

        // A lockId of 0 is never a lock (opens by USE, not a cast).
        assert!(!cat.is_locked(0));
    }

    /// The keyless-chest path: lockId 43 (Sunken Chest, Storage Chest, Worn Wooden Chest, …) is a
    /// single SKILL slot naming LockType **13** — the one spell 6478 "Opening" opens, and "Opening"
    /// is a default-known player spell, so any character can loot these. Confirms the routing matches
    /// keyless chests (not just gathering nodes) to a spell the player already has. Skips without data.
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
        // The requirement sits in slot 1 here (slot 0 empty) — the routing must scan all 8 slots,
        // not just slot 0. LockType 13 = the "Opening" the default spell 6478 provides.
        assert_eq!(chest[1].key_type, LOCK_KEY_SKILL);
        assert_eq!(
            chest[1].index, 13,
            "the keyless-chest LockType spell 6478 opens"
        );
        // …and it is an `Action 0` (open) slot, so it applies only to an unflagged object: the
        // chest opens because chests do NOT carry GO_FLAG_LOCKED, not because Action is ignored.
        assert_eq!(chest[1].action, 0);
        assert!(chest[1].available(GO_STATE_READY, false));
        assert!(!chest[1].available(GO_STATE_READY, true));
    }

    /// The `Action` column on the real 5875 `Lock.dbc`, on the two rows the "any locked door opens"
    /// report turns on. A column slip here re-opens the bug silently, so both rows
    /// are pinned by value. Skips without client data.
    #[test]
    fn real_lock_catalog_reads_the_action_column() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_lock_catalog(&mut chain).expect("load Lock.dbc");

        // Scholomance Door (gameobject_template 174626, door.data1 = lockId 1159; wire flags 34 =
        // GO_FLAG_LOCKED|NODESPAWN). Five slots: the Skeleton Key, Pick Lock 280, and the three
        // spares — `Quick Open`/`Quick Close`/`Blasting`.
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
        // The gate: on a GO_FLAG_LOCKED door, the Quick Open slot does not apply — so the
        // universally-known "Opening" (6247) never gets to satisfy it. The key and Pick Lock legs
        // do apply, which is exactly the pair the reference asks the player for.
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

        // The Searing Gorge gate (gameobject_template 150137/150138, lockId 84) — the reporter's
        // counter-example, and the reason the bug looked like "almost all doors". It carries no
        // Action-0 slot at all, so it refused even before the gate existed.
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
        // Action 0 (open): READY + not flagged locked.
        assert!(slot(0).available(GO_STATE_READY, false));
        assert!(!slot(0).available(GO_STATE_READY, true));
        assert!(!slot(0).available(GO_STATE_ACTIVE, false));
        // Action 1 (unlock): READY + flagged locked — the exact mirror.
        assert!(slot(1).available(GO_STATE_READY, true));
        assert!(!slot(1).available(GO_STATE_READY, false));
        assert!(!slot(1).available(GO_STATE_ACTIVE, true));
        // Action 2 (close): only while ACTIVE, flag irrelevant.
        assert!(slot(2).available(GO_STATE_ACTIVE, false));
        assert!(slot(2).available(GO_STATE_ACTIVE, true));
        assert!(!slot(2).available(GO_STATE_READY, false));
        // Action 3: READY, flag irrelevant.
        assert!(slot(3).available(GO_STATE_READY, true));
        assert!(!slot(3).available(GO_STATE_ACTIVE, false));
        // Action 4 is the ONLY action an ALTERNATIVE-state object admits, and it admits nothing else.
        assert!(slot(4).available(GO_STATE_ACTIVE_ALTERNATIVE, false));
        assert!(!slot(4).available(GO_STATE_READY, false));
        for action in [0, 1, 2, 3, 5, 19] {
            assert!(
                !slot(action).available(GO_STATE_ACTIVE_ALTERNATIVE, false),
                "action {action} must not apply in the ALTERNATIVE state"
            );
        }
        // An unmodelled action (≥5) falls through to "applies", in any state but ALTERNATIVE.
        assert!(slot(5).available(GO_STATE_ACTIVE, false));
        assert!(slot(5).available(GO_STATE_READY, true));
    }
}
