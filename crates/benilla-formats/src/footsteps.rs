//! Footsteps: ground texture → `GroundEffectTexture.TerrainType` → `TerrainType.SoundID`, joined
//! with `CreatureSoundData.FootstepID` in `FootstepTerrainLookup` to a dry and a splash
//! `SoundEntries` kit. `TerrainType.dbc` is `ID`, `Desc`, `FootstepSprayRun`, `FootstepSprayWalk`,
//! `SoundID`, `Flags`: 11 rows, `SoundID` 1 to 9 (DustyGrass shares Grass's 6) and 0 for None.
//!
//! Class 7 is the character class (its rows are the `CharacterMediumLarge*` kits), reached through
//! the display's sound data, never a code default. Class 0 means no footsteps: the reference bails
//! before any lookup (`0x6233ec`). A position with no ground-effect layer is silent: its sentinel
//! is -1, which the kit lookup's signed bounds check rejects (`0x458450`).

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

/// The joined footstep tables, resolvable from `(footstep class, ground effect id)`.
pub struct FootstepCatalog {
    /// `GroundEffectTexture` id → `TerrainType` id (every row, including doodad-less ones).
    effect_terrain: HashMap<u32, u32>,
    /// `TerrainType` id → its `SoundID` (the lookup's `TerrainSoundID` axis).
    terrain_sound: HashMap<u32, u32>,
    /// `TerrainType` id → its `Flags`; bit 0, the footprint bit, is set on Snow (3) and Sand (7).
    terrain_flags: HashMap<u32, u32>,
    /// `(CreatureFootstepID, TerrainSoundID)` → `(dry kit, splash kit)`.
    lookup: HashMap<(u32, u32), (u32, u32)>,
}

impl FootstepCatalog {
    /// The `(dry, splash)` `SoundEntries` kits for a footstep class on a ground-effect layer;
    /// `None`, silence, for no layer or no row.
    pub fn resolve(&self, footstep_class: u32, effect_id: Option<u32>) -> Option<(u32, u32)> {
        self.resolve_terrain(
            footstep_class,
            self.effect_terrain.get(&effect_id?).copied()?,
        )
    }

    /// The same kits from a `TerrainType` id: the reference's down-ray reaches one through the
    /// ground-effect layer on terrain and reads it off the surface in a WMO.
    pub fn resolve_terrain(&self, footstep_class: u32, terrain: u32) -> Option<(u32, u32)> {
        let sound_class = self.terrain_sound.get(&terrain).copied()?;
        self.lookup.get(&(footstep_class, sound_class)).copied()
    }

    /// Whether the ground under an effect layer takes footprints: `TerrainType.Flags` bit 0, the
    /// bit the reference's footprint gate tests (`0x699e8e`). No layer, no prints.
    pub fn leaves_footprints(&self, effect_id: Option<u32>) -> bool {
        effect_id
            .and_then(|e| self.effect_terrain.get(&e))
            .is_some_and(|&t| self.terrain_leaves_footprints(t))
    }

    /// The footprint bit from a terrain id; the WMO default, 10 "None", is clear, so a building's
    /// floor takes no prints.
    pub fn terrain_leaves_footprints(&self, terrain: u32) -> bool {
        self.terrain_flags
            .get(&terrain)
            .is_some_and(|flags| flags & 1 != 0)
    }

    /// The `TerrainType` id under a ground-effect layer, the chain's first hop.
    pub fn terrain_of(&self, effect_id: u32) -> Option<u32> {
        self.effect_terrain.get(&effect_id).copied()
    }

    /// A `TerrainType`'s `SoundID`, the `FootstepTerrainLookup` axis.
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

/// `FootprintTextures.dbc`, id → extensionless texture path (`textures\Footsteps\BaseFootprint`),
/// the table `CreatureModelData.FootprintTextureID` indexes.
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

    /// The chain on 5875 data: class 8 on Metallic, and the character class on dirt and grass.
    #[test]
    fn real_footstep_chain_resolves() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_footstep_catalog(&mut chain).expect("load footstep catalog");
        assert_eq!(cat.len(), 179, "all lookup rows load");

        // An effect over Metallic (`TerrainType` 1, sound 2).
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
        // No ground-effect layer, or an unknown class, is silent.
        assert_eq!(cat.resolve(7, None), None);
        assert_eq!(cat.resolve(9999, None), None);
    }

    /// The footprint bit on 5875 data, Snow and Sand only, and the six `FootprintTextures` rows.
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
