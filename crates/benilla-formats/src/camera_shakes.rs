//! `CameraShakes.dbc` — the shipped camera-shake presets.
//!
//! A shake is **not** authored per model or per animation: the model names a preset id, and this
//! 24-row table holds the shape. Two id spaces reach it (decisions 1540, 1849):
//!
//! - **The big-creature footstep/death thud** names a preset **directly** —
//!   `CreatureModelData.FootstepShakeSize` (field 11) and `.DeathThudShakeSize` (field 12), read
//!   here through [`CreatureCatalog::footstep_shake`](crate::CreatureCatalog::footstep_shake).
//! - **Everything else names a GROUP** — `SpellEffectCameraShakes.dbc`, the 9-row indirection read
//!   here too ([`SpellShakeGroup`]): up to three `CameraShakes` ids fired at one point. Both
//!   `SpellVisualKit` field 14 and the `$SHK` animation event speak this id space and never the
//!   preset one (`[0xc0d814]`, bound `0xc0d818`).
//!
//! **Layout — VERIFIED** against build 5875 (header + row decode, 2026-08-22): `24 × 8 × 32 B`,
//! string block empty. The **column names** are the conventional map (wowdev.wiki + vmangos
//! `DBCStructure.h`), and the shipped values corroborate them structurally rather than by
//! assertion: rows 4/5/6, 7/8/9, 13/14/15, 16/17/18, 36/37/38 and 76/77/78 are **direction
//! triples** — identical `Duration` within a triple, `Direction` running 0·1·2 — which is what a
//! per-axis column looks like and nothing else does.
//!
//! **The two families are visibly distinct**, which is the check that the creature columns really
//! do index this table:
//!
//! | family | rows | `Type`/`Direction` | `Duration` | `Phase` | `Coefficient` |
//! |---|---|---|---|---|---|
//! | creature (footstep + thud) | 1, 2, 10, 11, 12 | `1` / `2` | 0.40–0.65 s | **nonzero** | 1.0–2.0 |
//! | spell (the direction triples) | 4–9, 13–18, 36–38, 76–78 | 0 or 1 / 0·1·2 | 0.6–20 s | **0.0** | 0.4–3.0 |
//!
//! and within the creature family the amplitude ranks by mass exactly as it should: the Ancient
//! Protector and the kodo take row 1 (`amplitude 2.0`), the Ancient of Lore/War, the giants and
//! the dragons take row 2 (`amplitude 7.0`), and the death thuds (rows 11/12) are longer and
//! stronger than the footsteps.
//!
//! **The semantics** — what `ShakeType` and `Direction` select, and the distance attenuation the
//! evaluator applies — were settled against the reference in decision 1540 and live beside the
//! evaluator, in `benilla-app`'s `camera_shake`. This module stays the shipped data and nothing
//! more.
//!
//! ## The group table — `SpellEffectCameraShakes.dbc` (9 rows × 4 fields, recsize 16)
//!
//! Every **spell-side** producer names a row of *this* table, never a `CameraShakes` id: the
//! reference's shake spawner is reached through a group that fires **up to three presets at one
//! point**. Layout verified against build 5875 the same way ([`SpellShakeGroup`]): `ID` plus three
//! `CameraShakes` ids at `+0x4/+0x8/+0xc`, zeros skipped, string block empty. The shipped rows:
//!
//! | group | slots | group | slots |
//! |---|---|---|---|
//! | 1 | 4 · 5 · 6 | 6 | 4 · 5 · 6 |
//! | 2 | 7 · 8 · 9 | 7 | 18 · 17 · 18 |
//! | 3 | 1 | 26 | 36 · 37 · 38 |
//! | 4 | 15 · 14 · 15 | 66 | 76 · 77 · 78 |
//! | 5 | 11 | | |
//!
//! **The three slots are slots, not axes.** Group 4 lists `15` twice and group 7 lists `18` twice —
//! a per-axis reading cannot survive that. The *intent* is clearly a per-axis triple (the ids named
//! are the `direction` 0·1·2 members of a `CameraShakes` family), but the duplicate simply loses the
//! evaluator's strict-`>` tie-break and contributes nothing.
//!
//! **Reachability.** The creature columns name `CameraShakes` {1, 2, 10} (footstep) and
//! {10, 11, 12, 38} (thud); the nine groups name {1, 4, 5, 6, 7, 8, 9, 11, 14, 15, 17, 18, 36, 37,
//! 38, 76, 77, 78}. Their union is 21 of the 24 shipped rows — **13, 16 and 56 are named by neither
//! table**. All three are `direction 0` rows; 13 and 16 are the missing member of a family whose
//! other two *are* named (14/15 by group 4, 17/18 by group 7), and 56 stands alone. See
//! `benilla-extract shakecensus`, which prints the whole map — and keeps "named by a table" apart
//! from "has a live producer behind it", which are the same 21 rows here only because every group
//! turns out to have a producer.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};
use crate::Chain;

const CAMERA_SHAKES: &str = "DBFilesClient\\CameraShakes.dbc";
const SPELL_EFFECT_CAMERA_SHAKES: &str = "DBFilesClient\\SpellEffectCameraShakes.dbc";

/// One `CameraShakes.dbc` row — the authored shape of a shake, in the table's own units.
///
/// Every field below `id` is data as shipped; see the module doc for what is verified (the layout
/// and the family split) and what still awaits the reference (the evaluator's reading of them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraShake {
    pub id: u32,
    /// `ShakeType` — 0 or 1 across the shipped table; every creature row is `1`.
    pub shake_type: u32,
    /// `Direction` — 0·1·2, running consistently across the spell rows' triples; every creature
    /// row is `2`.
    pub direction: u32,
    pub amplitude: f32,
    pub frequency: f32,
    /// Seconds.
    pub duration: f32,
    pub phase: f32,
    pub coefficient: f32,
}

/// One `SpellEffectCameraShakes.dbc` row — **up to three [`CameraShake`] ids fired at one point**.
///
/// This is the id space every *spell-side* producer speaks: `SpellVisualKit` field 14 and the
/// `$SHK` animation event both name a **group**, never a preset. Zero is the empty slot; the
/// reference walks the three in order and skips them (`edi = 3`, `0x6ecb40`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellShakeGroup {
    pub id: u32,
    /// The three slots as shipped, zeros included — iterate with [`Self::shakes`].
    pub slots: [u32; 3],
}

impl SpellShakeGroup {
    /// The populated slots, in the reference's walk order. A group can name the same preset twice
    /// (group 4 is `15 · 14 · 15`); the duplicate is kept here and loses the evaluator's tie-break.
    pub fn shakes(&self) -> impl Iterator<Item = u32> + '_ {
        self.slots.iter().copied().filter(|&id| id != 0)
    }
}

/// The camera-shake tables: `CameraShakes.dbc` keyed by row id, and the
/// `SpellEffectCameraShakes.dbc` groups that index it — **two id spaces, never conflated**
/// (the same per-table split [`crate::SpellVisualCatalog`] keeps).
///
/// `Default` is the **empty** catalog — the same thing "the DBC failed to load" already means to
/// every consumer (every lookup misses and the caller takes its documented fallback).
#[derive(Default)]
pub struct CameraShakeCatalog {
    rows: HashMap<u32, CameraShake>,
    groups: HashMap<u32, SpellShakeGroup>,
}

impl CameraShakeCatalog {
    /// One preset by id. `None` for id 0 (the "no shake" value both creature columns use) and for
    /// any id the shipped table does not carry.
    pub fn get(&self, id: u32) -> Option<&CameraShake> {
        self.rows.get(&id)
    }

    /// Every row, unordered.
    pub fn iter(&self) -> impl Iterator<Item = &CameraShake> {
        self.rows.values()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// One `SpellEffectCameraShakes.dbc` group by id — the spell side's whole lookup. `None` for
    /// id 0 (the "no shake" value the kit column uses) and for any id the 9-row table lacks.
    pub fn group(&self, id: u32) -> Option<&SpellShakeGroup> {
        self.groups.get(&id)
    }

    /// Every group, unordered.
    pub fn groups(&self) -> impl Iterator<Item = &SpellShakeGroup> {
        self.groups.values()
    }

    pub fn group_len(&self) -> usize {
        self.groups.len()
    }

    /// Seed one preset — for fixtures exercising the evaluator without a client install (the same
    /// builder shape [`crate::SpellVisualCatalog::with_chain_effect`] uses). The live path is
    /// [`load_camera_shakes`].
    #[must_use]
    pub fn with_row(mut self, row: CameraShake) -> Self {
        self.rows.insert(row.id, row);
        self
    }

    /// Seed one group. See [`Self::with_row`].
    #[must_use]
    pub fn with_group(mut self, group: SpellShakeGroup) -> Self {
        self.groups.insert(group.id, group);
        self
    }
}

fn camera_shakes_schema() -> Schema {
    let mut s = Schema::new("CameraShakes");
    for (name, ty) in [
        ("ID", FieldType::UInt32),
        ("ShakeType", FieldType::UInt32),
        ("Direction", FieldType::UInt32),
        ("Amplitude", FieldType::Float32),
        ("Frequency", FieldType::Float32),
        ("Duration", FieldType::Float32),
        ("Phase", FieldType::Float32),
        ("Coefficient", FieldType::Float32),
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    s
}

fn spell_effect_camera_shakes_schema() -> Schema {
    let mut s = Schema::new("SpellEffectCameraShakes");
    for name in ["ID", "CameraShake1", "CameraShake2", "CameraShake3"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Read both camera-shake tables off the patch chain.
///
/// `SpellEffectCameraShakes.dbc` is loaded beside `CameraShakes.dbc` rather than on demand: it is
/// nine rows, it ships in the same archive, and every spell-side producer needs both to resolve a
/// single shake. A missing or malformed table is a hard error like every other DBC (1300) — the
/// caller's fallback is to run with no shake system at all, which is what an empty catalog means.
pub fn load_camera_shakes(chain: &mut Chain) -> Result<CameraShakeCatalog> {
    let bytes = chain
        .read_file(CAMERA_SHAKES)
        .with_context(|| format!("reading {CAMERA_SHAKES}"))?;
    let rs = parse(&bytes, camera_shakes_schema(), "CameraShakes")?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        rows.insert(
            id,
            CameraShake {
                id,
                shake_type: u32_at(r, 1).unwrap_or(0),
                direction: u32_at(r, 2).unwrap_or(0),
                amplitude: f32_at(r, 3).unwrap_or(0.0),
                frequency: f32_at(r, 4).unwrap_or(0.0),
                duration: f32_at(r, 5).unwrap_or(0.0),
                phase: f32_at(r, 6).unwrap_or(0.0),
                coefficient: f32_at(r, 7).unwrap_or(0.0),
            },
        );
    }

    let bytes = chain
        .read_file(SPELL_EFFECT_CAMERA_SHAKES)
        .with_context(|| format!("reading {SPELL_EFFECT_CAMERA_SHAKES}"))?;
    let gs = parse(
        &bytes,
        spell_effect_camera_shakes_schema(),
        "SpellEffectCameraShakes",
    )?;
    let mut groups = HashMap::with_capacity(gs.records().len());
    for r in gs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let mut slots = [0; 3];
        for (i, slot) in slots.iter_mut().enumerate() {
            *slot = u32_at(r, 1 + i).unwrap_or(0);
        }
        groups.insert(id, SpellShakeGroup { id, slots });
    }

    Ok(CameraShakeCatalog { rows, groups })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    /// The nine shipped `SpellEffectCameraShakes.dbc` groups, verbatim — the id space every
    /// spell-side producer speaks. Sparse ids (`…7, 26, 66`) and duplicate slots (group 4 lists
    /// `15` twice, group 7 lists `18` twice) are both load-bearing: the sparseness is what makes a
    /// wrong column map impossible to miss, and the duplicate is what proves the three slots are
    /// **slots, not axes**.
    #[test]
    fn the_spell_shake_groups_decode_as_shipped() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = super::load_camera_shakes(&mut chain).expect("load the camera-shake tables");

        assert_eq!(cat.group_len(), 9, "the shipped group table");
        for (id, slots) in [
            (1, [4, 5, 6]),
            (2, [7, 8, 9]),
            (3, [1, 0, 0]),
            (4, [15, 14, 15]),
            (5, [11, 0, 0]),
            (6, [4, 5, 6]),
            (7, [18, 17, 18]),
            (26, [36, 37, 38]),
            (66, [76, 77, 78]),
        ] {
            let g = cat
                .group(id)
                .unwrap_or_else(|| panic!("group {id} missing"));
            assert_eq!(g.slots, slots, "group {id}");
        }
        assert!(cat.group(0).is_none(), "0 is the no-shake value, not a row");
        assert!(cat.group(8).is_none(), "the id space is sparse, not dense");

        // Zeros are skipped, duplicates are not: group 3 fires one shake, group 4 fires three.
        assert_eq!(cat.group(3).unwrap().shakes().count(), 1);
        assert_eq!(
            cat.group(4).unwrap().shakes().collect::<Vec<_>>(),
            vec![15, 14, 15],
            "a duplicate slot survives the walk and loses the evaluator's tie-break"
        );
    }

    /// **The property that licenses the column map**: every id the group table names must land on a
    /// real row of the 24-row preset table. A merely plausible map would dangle.
    #[test]
    fn every_group_slot_resolves_to_a_preset() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = super::load_camera_shakes(&mut chain).expect("load the camera-shake tables");

        assert_eq!(cat.len(), 24, "the shipped preset table");
        let named: BTreeSet<u32> = cat.groups().flat_map(|g| g.shakes()).collect();
        for id in &named {
            assert!(cat.get(*id).is_some(), "group slot {id} dangles");
        }
        assert_eq!(
            named.iter().copied().collect::<Vec<_>>(),
            vec![1, 4, 5, 6, 7, 8, 9, 11, 14, 15, 17, 18, 36, 37, 38, 76, 77, 78],
            "the 18 presets the spell side reaches"
        );

        // The census the module doc states: with the creature columns' {1,2,10} and
        // {10,11,12,38}, three shipped rows are named by NEITHER table.
        let creature: BTreeSet<u32> = [1, 2, 10, 11, 12, 38].into_iter().collect();
        let all: BTreeSet<u32> = cat.iter().map(|r| r.id).collect();
        let unreached: Vec<u32> = all.difference(&(&named | &creature)).copied().collect();
        assert_eq!(
            unreached,
            vec![13, 16, 56],
            "shipped presets no table names — each the direction-0 member of a named family"
        );
    }
}
