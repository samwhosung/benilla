//! Footsteps: the terrain-type chain (decision 0070 slice 3) —
//! `ground texture → GroundEffectTexture.TerrainType → TerrainType.SoundID ×
//! CreatureSoundData.FootstepID → FootstepTerrainLookup → SoundEntries (dry | splash)`.
//!
//! Layouts — VERIFIED against build 5875 (headers + row decodes, 2026-07-02):
//! - `TerrainType` **11 × 6 × 24 B**: `ID, Desc(str), FootstepSprayRun, FootstepSprayWalk,
//!   SoundID, Flags` — the full domain decoded: Dirt→1, Metallic→2, Stone→3, Snow→4, Wood→5,
//!   Grass→6, Leaves→7, Sand→8, Soggy→9, DustyGrass→6, None→0.
//! - `FootstepTerrainLookup` **179 × 5 × 20 B**: `ID, CreatureFootstepID, TerrainSoundID,
//!   SoundID(dry), SoundIDSplash`. Spot-checks: class 8 × terrain-sound 2 (Metallic) →
//!   650/1063; class 8 × 3 (Stone) → 653/1057.
//! - `GroundEffectTexture` field 6 is the `TerrainType` FK (the clutter catalog reads the same
//!   table for doodads and rightly skips doodad-less rows; footsteps need every row, so this
//!   module re-reads it into its own map).
//!
//! Class semantics (data-verified 2026-07-02; class-0 gate at `0x6233ec`): **class 7 is the
//! humanoid/character class** — its ten rows
//! are exactly the `CharacterMediumLarge*` kits; characters reach it through the ordinary
//! display→sound data chain (`creature_sound`, the model fallback), never a code default.
//! **Class 0 means "no footstep sounds"** — the client bails before any lookup; the lookup's
//! class-0 rows are the Ancient Protector's stomps (kit 661), reached only by a *nonzero* class
//! on its own row. A position with **no ground-effect layer is silent** — the client's sentinel
//! is −1 and the kit lookup's signed bounds check rejects it (`0x458450`; the
//! audible fingerprint is vanilla's famously quiet dirt roads).

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

/// The joined footstep tables, resolvable from `(footstep class, ground effect id)`.
pub struct FootstepCatalog {
    /// `GroundEffectTexture` id → `TerrainType` id (every row, including doodad-less ones).
    effect_terrain: HashMap<u32, u32>,
    /// `TerrainType` id → its `SoundID` class (the lookup's `TerrainSoundID` axis).
    terrain_sound: HashMap<u32, u32>,
    /// `TerrainType` id → its `Flags` word. In the shipped 5875 data bit 0 is set on exactly
    /// Snow (3) and Sand (7) — the leaves-footprints surfaces (the reporter-visible pair).
    terrain_flags: HashMap<u32, u32>,
    /// `(CreatureFootstepID, TerrainSoundID)` → `(dry kit, splash kit)`.
    lookup: HashMap<(u32, u32), (u32, u32)>,
}

impl FootstepCatalog {
    /// The `(dry, splash)` SoundEntries kits for a creature footstep class standing on the given
    /// ground-effect layer. `None` = silence: no/unknown effect layer (the client's −1 sentinel,
    /// module docs), or no lookup row for `(class, terrain)`.
    pub fn resolve(&self, footstep_class: u32, effect_id: Option<u32>) -> Option<(u32, u32)> {
        self.resolve_terrain(
            footstep_class,
            self.effect_terrain.get(&effect_id?).copied()?,
        )
    }

    /// The same `(dry, splash)` answer from a `TerrainType` id **directly**, skipping the
    /// `GroundEffectTexture` hop. This is the tail both legs of the client's down-ray share: the
    /// ADT leg reaches a terrain id through the ground-effect layer, the WMO leg carries one in
    /// the surface itself.
    pub fn resolve_terrain(&self, footstep_class: u32, terrain: u32) -> Option<(u32, u32)> {
        let sound_class = self.terrain_sound.get(&terrain).copied()?;
        self.lookup.get(&(footstep_class, sound_class)).copied()
    }

    /// Does the ground under this effect layer take footprint decals? `TerrainType.Flags` bit 0
    /// through the same effect→terrain chain the sounds ride (INTERIM reading, decision 1006:
    /// bit 0 fits the shipped data — set on exactly Snow and Sand — with the client's own gate
    /// unconfirmed). No/unknown effect layer = no prints.
    pub fn leaves_footprints(&self, effect_id: Option<u32>) -> bool {
        effect_id
            .and_then(|e| self.effect_terrain.get(&e))
            .is_some_and(|&t| self.terrain_leaves_footprints(t))
    }

    /// The same `TerrainType.Flags` bit 0 gate from a terrain id **directly** — the form both legs
    /// of the down-ray share, since the WMO leg carries a terrain id rather than an effect layer.
    /// Set on exactly Snow (3) and Sand (7) in the shipped data; the unauthored WMO default
    /// `10 "None"` is clear, which is why a building's floor takes no prints.
    pub fn terrain_leaves_footprints(&self, terrain: u32) -> bool {
        self.terrain_flags
            .get(&terrain)
            .is_some_and(|flags| flags & 1 != 0)
    }

    /// The `TerrainType` id under a ground-effect layer — the chain's first hop on its own.
    /// Exposed for the `surface_here` probe, which has to show *where* the chain lands, and where
    /// it falls off, not just the kit it ends at.
    pub fn terrain_of(&self, effect_id: u32) -> Option<u32> {
        self.effect_terrain.get(&effect_id).copied()
    }

    /// A `TerrainType`'s `SoundID` — the `FootstepTerrainLookup` axis the kit is chosen on.
    /// Companion to [`Self::terrain_of`]; the same map [`Self::resolve`] walks.
    pub fn sound_class_of(&self, terrain: u32) -> Option<u32> {
        self.terrain_sound.get(&terrain).copied()
    }

    pub fn len(&self) -> usize {
        self.lookup.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lookup.is_empty()
    }
}

fn n_u32_schema(name: &str, n: usize, string_fields: &[usize]) -> Schema {
    let mut s = Schema::new(name);
    for i in 0..n {
        let ty = if string_fields.contains(&i) {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("f{i}"), ty));
    }
    s
}

/// Read the three tables off the patch chain.
pub fn load_footstep_catalog(chain: &mut Chain) -> Result<FootstepCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\GroundEffectTexture.dbc")
        .context("reading GroundEffectTexture.dbc")?;
    let rs = parse(
        &bytes,
        n_u32_schema("GroundEffectTexture", 7, &[]),
        "GroundEffectTexture",
    )?;
    let mut effect_terrain = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let (Some(id), Some(tt)) = (u32_at(r, 0), u32_at(r, 6)) {
            effect_terrain.insert(id, tt);
        }
    }

    let bytes = chain
        .read_file("DBFilesClient\\TerrainType.dbc")
        .context("reading TerrainType.dbc")?;
    let rs = parse(&bytes, n_u32_schema("TerrainType", 6, &[1]), "TerrainType")?;
    let mut terrain_sound = HashMap::with_capacity(rs.records().len());
    let mut terrain_flags = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let (Some(id), Some(class)) = (u32_at(r, 0), u32_at(r, 4)) {
            terrain_sound.insert(id, class);
        }
        if let (Some(id), Some(flags)) = (u32_at(r, 0), u32_at(r, 5)) {
            terrain_flags.insert(id, flags);
        }
    }

    let bytes = chain
        .read_file("DBFilesClient\\FootstepTerrainLookup.dbc")
        .context("reading FootstepTerrainLookup.dbc")?;
    let rs = parse(
        &bytes,
        n_u32_schema("FootstepTerrainLookup", 5, &[]),
        "FootstepTerrainLookup",
    )?;
    let mut lookup = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(class), Some(ts)) = (u32_at(r, 1), u32_at(r, 2)) else {
            continue;
        };
        lookup.insert(
            (class, ts),
            (u32_at(r, 3).unwrap_or(0), u32_at(r, 4).unwrap_or(0)),
        );
    }

    Ok(FootstepCatalog {
        effect_terrain,
        terrain_sound,
        terrain_flags,
        lookup,
    })
}

/// `FootprintTextures.dbc` — id → texture path (extensionless, e.g.
/// `textures\Footsteps\BaseFootprint`), the table `CreatureModelData.FootprintTextureID`
/// indexes. Shipped 5875 data: 6 rows (1 Base, 3 Cloven, 4 Bare, 5 Claw, 6 Hoof, 7 Paw), all
/// 32×32 pure-black-RGB BLPs under a soft alpha — the print decal's ink.
pub fn load_footprint_textures(chain: &mut Chain) -> Result<HashMap<u32, String>> {
    let bytes = chain
        .read_file("DBFilesClient\\FootprintTextures.dbc")
        .context("reading FootprintTextures.dbc")?;
    let rs = parse(
        &bytes,
        n_u32_schema("FootprintTextures", 2, &[1]),
        "FootprintTextures",
    )?;
    let mut map = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let (Some(id), Some(path)) = (u32_at(r, 0), str_at(&rs, r, 1)) {
            map.insert(id, path);
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full chain resolves on real 5875 data: a Metallic-terrain effect under footstep
    /// class 8 yields the byte-decoded kits (650 dry / 1063 splash); the humanoid class 7
    /// resolves the `CharacterMediumLarge*` kits (560 Dirt with no effect, 562 Grass on a
    /// grass-terrain effect — the character-in-Elwynn case); an unknown class stays silent.
    #[test]
    fn real_footstep_chain_resolves() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_footstep_catalog(&mut chain).expect("load footstep catalog");
        assert_eq!(cat.len(), 179, "all lookup rows load");

        // Some effect whose terrain is Metallic (TerrainType 1 → sound class 2).
        let metallic = cat
            .effect_terrain
            .iter()
            .find(|(_, &tt)| tt == 1)
            .map(|(&e, _)| e);
        if let Some(e) = metallic {
            assert_eq!(
                cat.resolve(8, Some(e)),
                Some((650, 1063)),
                "class 8 on metallic → the byte-decoded row"
            );
        }
        // The humanoid class (7): the dirt kit on a dirt-terrain effect, the grass kit on a
        // grass one (class 0 is the Ancient Protector, module docs).
        let effect_with_sound_class = |sc: u32| {
            cat.effect_terrain
                .iter()
                .find(|(_, tt)| cat.terrain_sound.get(tt) == Some(&sc))
                .map(|(&e, _)| e)
        };
        if let Some(e) = effect_with_sound_class(1) {
            assert_eq!(
                cat.resolve(7, Some(e)).map(|(dry, _)| dry),
                Some(560),
                "class 7 on dirt → CharacterMediumLargeDirt"
            );
        }
        if let Some(e) = effect_with_sound_class(6) {
            assert_eq!(
                cat.resolve(7, Some(e)).map(|(dry, _)| dry),
                Some(562),
                "class 7 on grass → CharacterMediumLargeGrass"
            );
        }
        // No ground-effect layer = silence (the −1 sentinel, B5); unknown class too.
        assert_eq!(cat.resolve(7, None), None);
        assert_eq!(cat.resolve(9999, None), None);
    }

    /// The footprint gate on the real data: `TerrainType.Flags` bit 0 is set on exactly Snow (3)
    /// and Sand (7) — an effect layer over either takes prints, every other terrain (and the
    /// no-layer sentinel) doesn't. And `FootprintTextures.dbc` decodes its six shipped rows to
    /// the `textures\Footsteps\*` ink paths.
    #[test]
    fn footprint_gate_and_textures_on_real_data() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_footstep_catalog(&mut chain).expect("load footstep catalog");
        let printing: std::collections::BTreeSet<u32> = cat
            .terrain_flags
            .iter()
            .filter(|(_, &f)| f & 1 != 0)
            .map(|(&t, _)| t)
            .collect();
        assert_eq!(
            printing,
            [3, 7].into(),
            "exactly Snow and Sand carry the flag"
        );
        let effect_on = |terrain: u32| {
            cat.effect_terrain
                .iter()
                .find(|(_, &tt)| tt == terrain)
                .map(|(&e, _)| e)
        };
        if let Some(e) = effect_on(3) {
            assert!(cat.leaves_footprints(Some(e)), "snow effect layer prints");
        }
        if let Some(e) = effect_on(5) {
            assert!(!cat.leaves_footprints(Some(e)), "grass doesn't");
        }
        assert!(!cat.leaves_footprints(None), "no layer, no prints");

        let inks = load_footprint_textures(&mut chain).expect("load FootprintTextures.dbc");
        assert_eq!(inks.len(), 6);
        assert_eq!(
            inks.get(&1).map(String::as_str),
            Some(r"textures\Footsteps\BaseFootprint")
        );
        assert_eq!(
            inks.get(&7).map(String::as_str),
            Some(r"textures\Footsteps\PawFootprint")
        );
    }
}
