//! `AnimationData.dbc` — the per-animation policy row behind the sheath reconcile (decision 0080)
//! and, later, missing-clip fallback resolution.
//!
//! Layout — VERIFIED against build 5875 (header + full 208-row decode with name joins,
//! 2026-07-03): **208 × 7 × 28 B**: `ID(0), Name(str, 1), WeaponFlags(2), BodyFlags(3), _col4(4),
//! _col5(5), Fallback(6)`.
//!
//! - **WeaponFlags (col 2)** — the sheath-reconcile bits the client tests on every `PlayAnimation`
//!   (`0x5fdf80`). The identification is the binary's: the reconcile indexes the
//!   `WowClientDB<AnimationData>` cache (`0xc0e070`, loader-asserted 7 columns × 28 B against
//!   `DBFilesClient\AnimationData.dbc`) by the current sequence's AnimationData id, and all three
//!   flag tests read row offset `+0x8` = column 2.
//!   The bits: `4` force-stow (casts, Swim 42–45, Mount 91,
//!   SitChair 102–104, Loot 50), `0x10` force-stow (every Emote, AttackUnarmed 16/117,
//!   SitGround 96–98, Kneel 114–116, AttackBow/Rifle/Thrown), `0x20` force-draw-melee (armed
//!   attacks 17–19/85–88, Ready1H/2H/2HL 26–28, specials, FishingCast/Loop 133–134). The column's
//!   exact 5875 value set is `{0, 4, 16, 20, 32}` — precisely the tested bit triple.
//! - **Fallback (col 6)** — identified from the data (the chains are unambiguous:
//!   Attack2H→Attack1H→AttackUnarmed, Sprint→Run, JumpLandRun→Run, Sleep→SleepDown, Drowned→Drown,
//!   WalkBackwards→Walk, StealthWalk→Walk; col 5 read as a fallback yields nonsense — Attack2H→Stop,
//!   Sleep→SpellCast). **Not the primary runtime missing-clip mechanism** (decision 0082, amending
//!   this file's earlier reading): the real client's per-id fallback for ids `< 203` is a single
//!   indexed read of the M2's own baked `PlayableAnimationLookup` (a per-model precomputed cache of
//!   *this* column's walk against that model's actual sequence set — see
//!   `benilla_formats::PlayableAnim` / `benilla_assets::ModelAnimations::resolve`). This column is
//!   PATH 2 of that resolver (ids ≥ 203 only, `AnimationData.dbc`'s 208 rows) and the *source* the
//!   baked tables were computed from. `0` = fall back to Stand.
//! - Cols 4/5 are unidentified here (4 reads priority-like — 500 on Death/Dead/Drown, 10 on
//!   locomotion; 5 flag-like) and are not exposed.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

/// One animation's policy row.
#[derive(Clone, Copy)]
pub struct AnimEntry {
    /// The sheath-reconcile bits (see the module doc). `0` for most rows.
    pub weapon_flags: u32,
    /// The substitute animation id when a model lacks this clip (`0` = Stand).
    pub fallback: u16,
}

/// `AnimationData.dbc` id → its policy row.
pub struct AnimDataCatalog {
    rows: HashMap<u16, AnimEntry>,
    /// id → the row's Name string (col 1, `"Attack1H"`) — the human face for debug readouts
    /// (the inspector's anim line). Beside [`AnimEntry`] so the policy row stays `Copy`.
    names: HashMap<u16, String>,
}

impl AnimDataCatalog {
    /// Build a catalog directly from rows — the synthetic-table entry point for tests exercising
    /// missing-clip resolution's PATH 2 walk (decision 0082, `benilla_assets::ModelAnimations::resolve`)
    /// without a real `AnimationData.dbc`.
    pub fn from_rows(rows: impl IntoIterator<Item = (u16, AnimEntry)>) -> Self {
        Self {
            rows: rows.into_iter().collect(),
            names: HashMap::new(),
        }
    }

    /// The sheath-reconcile WeaponFlags for an animation id (`0` for an unknown id — no policy).
    pub fn weapon_flags(&self, id: u16) -> u32 {
        self.rows.get(&id).map_or(0, |r| r.weapon_flags)
    }

    /// The fallback animation for a model lacking clip `id` — `None` when the row has none
    /// (fallback 0 = Stand, left to the caller's own id-list fallbacks) or the id is unknown.
    pub fn fallback(&self, id: u16) -> Option<u16> {
        self.rows
            .get(&id)
            .map(|r| r.fallback)
            .filter(|&f| f != 0 && f != id)
    }

    /// The row's Name string (`"Run"`, `"Attack1H"`) — `None` for an unknown id or a
    /// synthetic-table catalog ([`Self::from_rows`] keeps no names).
    pub fn name(&self, id: u16) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("AnimationData");
    for i in 0..7 {
        let ty = if i == 1 {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("f{i}"), ty));
    }
    s
}

/// Read `AnimationData.dbc` off the patch chain.
pub fn load_anim_data_catalog(chain: &mut Chain) -> Result<AnimDataCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\AnimationData.dbc")
        .context("reading AnimationData.dbc")?;
    let rs = parse(&bytes, schema(), "AnimationData")?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    let mut names = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        rows.insert(
            id as u16,
            AnimEntry {
                weapon_flags: u32_at(r, 2).unwrap_or(0),
                fallback: u32_at(r, 6).unwrap_or(0) as u16,
            },
        );
        if let Some(name) = str_at(&rs, r, 1).filter(|n| !n.is_empty()) {
            names.insert(id as u16, name);
        }
    }
    Ok(AnimDataCatalog { rows, names })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real 5875 table: the WeaponFlags value set is exactly the reconcile's tested bit
    /// triple, the decision-0080 spot rows carry the recorded bits, and the fallback chains
    /// resolve (Attack2H→Attack1H, Sprint→Run). Skips without client data.
    #[test]
    fn real_animation_data_decodes() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_anim_data_catalog(&mut chain).expect("load AnimationData");
        assert_eq!(cat.len(), 208);

        // The whole column stays within the tested bit triple {0, 4, 0x10, 0x14, 0x20}.
        for (&id, row) in &cat.rows {
            assert!(
                matches!(row.weapon_flags, 0 | 4 | 0x10 | 0x14 | 0x20),
                "anim {id}: unexpected WeaponFlags {:#x}",
                row.weapon_flags
            );
        }

        // Decision 0080's transcribed highlights.
        assert_eq!(cat.weapon_flags(42), 4, "Swim force-stows");
        assert_eq!(cat.weapon_flags(91), 4, "Mount force-stows");
        assert_eq!(cat.weapon_flags(102), 4, "SitChairLow force-stows");
        assert_eq!(cat.weapon_flags(60), 0x10, "EmoteTalk force-stows");
        assert_eq!(
            cat.weapon_flags(16),
            0x10,
            "AttackUnarmed needs empty hands"
        );
        assert_eq!(cat.weapon_flags(96), 0x10, "SitGroundDown force-stows");
        assert_eq!(cat.weapon_flags(17), 0x20, "Attack1H force-draws");
        assert_eq!(cat.weapon_flags(26), 0x20, "Ready1H force-draws");
        assert_eq!(cat.weapon_flags(133), 0x20, "FishingCast force-draws");
        assert_eq!(cat.weapon_flags(0), 0, "Stand carries no policy");

        // The ranged-handling trio the reconcile exempts (0x69/0x6a/0x70).
        for id in [105, 106, 112] {
            assert_eq!(cat.weapon_flags(id), 0, "Load* anims carry no policy");
        }

        // The name column (the inspector's anim line).
        assert_eq!(cat.name(0), Some("Stand"));
        assert_eq!(cat.name(5), Some("Run"));
        assert_eq!(cat.name(17), Some("Attack1H"));
        assert_eq!(cat.name(255), None, "unknown id has no name");

        // The fallback chains.
        assert_eq!(cat.fallback(18), Some(17), "Attack2H → Attack1H");
        assert_eq!(cat.fallback(17), Some(16), "Attack1H → AttackUnarmed");
        assert_eq!(cat.fallback(143), Some(5), "Sprint → Run");
        assert_eq!(cat.fallback(187), Some(5), "JumpLandRun → Run");
        assert_eq!(cat.fallback(100), Some(99), "Sleep → SleepDown");
        assert_eq!(cat.fallback(4), None, "Walk falls back to Stand (0 → None)");
    }
}
