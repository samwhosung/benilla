//! Creature display resolution: a `displayId` to its M2, skin textures and scale through
//! `CreatureDisplayInfo` and `CreatureModelData`. A humanoid NPC's look is not on the wire: it
//! comes from `CreatureDisplayInfoExtra` and a baked atlas under `Textures\BakedNpcTextures\`.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, str_at, u32_at};

const CREATURE_MODEL_DATA: &str = "DBFilesClient\\CreatureModelData.dbc";
const CREATURE_DISPLAY_INFO: &str = "DBFilesClient\\CreatureDisplayInfo.dbc";
const CREATURE_DISPLAY_INFO_EXTRA: &str = "DBFilesClient\\CreatureDisplayInfoExtra.dbc";

/// A resolved creature display: model, scale, skins and, for a humanoid NPC, its appearance.
#[derive(Debug, Clone)]
pub struct CreatureModel {
    /// `.mdx` path (the M2 loader maps it to `.m2`), e.g. `Creature\Basilisk\Basilisk.mdx`.
    pub model_path: String,
    /// `CreatureModelData.modelScale * CreatureDisplayInfo.creatureModelScale`.
    pub scale: f32,
    /// `textureVariation[0..2]`, bare names found beside the model, for its `Monster1/2/3` slots;
    /// a humanoid NPC's body build (`0x5fb200`) never reads them.
    pub textures: [Option<String>; 3],
    /// A character-model NPC's appearance; `None` for a plain creature (`ExtendedDisplayInfoID` 0).
    pub npc_appearance: Option<NpcAppearance>,
    /// `CreatureDisplayInfo.BloodLevel` (+0x28), tier 1 of the reference's `UnitBloodLevels` row
    /// resolve, which [`crate::BloodCatalog::level_key`] performs.
    pub blood_display: i32,
    /// `CreatureModelData.BloodID` (+0x14), tier 2 of the same resolve: `−1` is a miss that falls
    /// through to tier 3, not bloodlessness.
    pub blood_model: i32,
    /// `CreatureModelData.collisionHeight`, in raw model units.
    pub collision_height: f32,
}

/// A character-model NPC's appearance, from `CreatureDisplayInfoExtra.dbc`. `skin` and `face` are
/// already in the baked atlas; they serve the live composite when a row has no bake name.
#[derive(Debug, Clone)]
pub struct NpcAppearance {
    pub race: u8,
    pub sex: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    /// Worn `ItemDisplayInfo` ids (fields 8..17), 0 = empty, by body slot: head, shoulder, shirt,
    /// chest, belt, pants, boots, wrist, gloves, tabard (no cloak column).
    pub equipment: [u32; 10],
    /// The baked body atlas under `Textures\BakedNpcTextures\`; `None` composites the body live.
    pub bake_name: Option<String>,
}

/// One CreatureDisplayInfo row (the parts we use).
#[derive(Debug, Clone)]
struct DisplayRow {
    model_id: u32,
    /// `ExtendedDisplayInfoID`: the `CreatureDisplayInfoExtra` key; 0 = none.
    extended_id: u32,
    scale: f32,
    textures: [Option<String>; 3],
    /// `BloodLevel` (field 10), a per-display `UnitBloodLevels` override; 0 names no row.
    blood_level: u32,
    /// `SizeClass` (field 9, +0x24), the display's override, read signed: `−1` defers to the model.
    size_class: i32,
    /// `CreatureModelAlpha` (field 5, +0x14), 0..=255: the `baseAlpha` of the reference's unit
    /// alpha product (`0x60d2d0`, × 1/255), which is a flat 1.0 for players.
    model_alpha: u32,
}

/// One CreatureModelData row (the parts we use).
#[derive(Debug, Clone)]
struct ModelRow {
    path: String,
    scale: f32,
    /// `Flags` (field 1); bit 0x2 is the no-breath flag.
    flags: u32,
    /// `SizeClass` (field 3, +0x0c), 0 Small to 4 Colossal, read signed like the display's.
    size_class: i32,
    /// `BloodID`, read signed: `−1` is a tier-2 miss, not bloodlessness.
    blood: i32,
    /// `FootprintTextureID` (field 6), read signed: `−1` leaves no prints.
    footprint_texture: i32,
    /// `FootprintTextureLength`/`Width` (fields 7/8), in inches (× 1/36 to yards, `0x607a00`).
    footprint_length: f32,
    footprint_width: f32,
    /// `collisionHeight` (field 15), raw model units.
    collision_height: f32,
    /// `FoleyMaterialID` (field 10): the reference reads `[unit+0xb3c]+0x28`, field 10 of 16.
    foley_material: u32,
    /// `FootstepShakeSize`/`DeathThudShakeSize` (fields 11/12), `CameraShakes.dbc` ids; 0 = none.
    footstep_shake: u32,
    death_thud_shake: u32,
}

/// A footprint decal: the `FootprintTextures.dbc` key and the print size in yards (length along
/// the facing, width across), before the unit's scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FootprintParams {
    pub texture_id: u32,
    pub length: f32,
    pub width: f32,
}

/// The creature display tables; `Default` is empty, which consumers treat as a failed load.
#[derive(Default)]
pub struct CreatureCatalog {
    display: HashMap<u32, DisplayRow>,
    models: HashMap<u32, ModelRow>,
    extra: HashMap<u32, NpcAppearance>,
}

impl CreatureCatalog {
    /// A display's own `creatureModelScale`, the mount scale law: a mount renders at
    /// `OBJECT_FIELD_SCALE_X × creatureModelScale`, without `modelScale` (`0x613ef0`).
    pub fn display_scale(&self, display_id: u32) -> Option<f32> {
        self.display.get(&display_id).map(|r| r.scale)
    }

    /// A display's footstep camera shake, a `CameraShakes.dbc` id (`FootstepShakeSize`).
    pub fn footstep_shake(&self, display_id: u32) -> Option<u32> {
        let row = self.display.get(&display_id)?;
        let id = self.models.get(&row.model_id)?.footstep_shake;
        (id != 0).then_some(id)
    }

    /// A display's camera shake as the body lands (`DeathThudShakeSize`).
    pub fn death_thud_shake(&self, display_id: u32) -> Option<u32> {
        let row = self.display.get(&display_id)?;
        let id = self.models.get(&row.model_id)?.death_thud_shake;
        (id != 0).then_some(id)
    }

    /// A display's audible size class, 0 Small to 4 Colossal, the axis of
    /// [`crate::DeathThudCatalog`]. The display's `SizeClass` wins and only its `−1` defers to the
    /// model's (`0x625500`); `None` when both are `−1`, which the reference's unsigned `>= 5` gate
    /// (`0x623744`) also silences.
    pub fn size_class(&self, display_id: u32) -> Option<u32> {
        let row = self.display.get(&display_id)?;
        let class = match row.size_class {
            -1 => self.models.get(&row.model_id)?.size_class,
            c => c,
        };
        u32::try_from(class).ok()
    }

    /// Every model row naming a shake: `(model id, path, footstep id, death-thud id)`.
    pub fn shaking_models(&self) -> impl Iterator<Item = (u32, &str, u32, u32)> + '_ {
        self.models
            .iter()
            .filter(|(_, m)| m.footstep_shake != 0 || m.death_thud_shake != 0)
            .map(|(id, m)| (*id, m.path.as_str(), m.footstep_shake, m.death_thud_shake))
    }

    /// Every model path: being a creature model decides whether an M2's animation events reach
    /// `CGUnit_C::HandleAnimEvent`.
    pub fn model_paths(&self) -> impl Iterator<Item = &str> + '_ {
        self.models.values().map(|m| m.path.as_str())
    }

    /// Every model row as `(model id, path, SizeClass)`; most displays override the column.
    pub fn sized_models(&self) -> impl Iterator<Item = (u32, &str, i32)> + '_ {
        self.models
            .iter()
            .map(|(id, m)| (*id, m.path.as_str(), m.size_class))
    }

    /// Every display as `(display id, model id)`.
    pub fn display_models(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.display.iter().map(|(id, r)| (*id, r.model_id))
    }

    /// `modelScale × creatureModelScale`. The server folds it into `OBJECT_FIELD_SCALE_X`, which
    /// alone scales a unit (`0x613ef0`), so only the glue screens read this: the reference sizes
    /// the serverless character-select pet by it (`0x472dc6`).
    pub fn model_scale(&self, display_id: u32) -> Option<f32> {
        let row = self.display.get(&display_id)?;
        let model = self.models.get(&row.model_id)?;
        Some(model.scale * row.scale)
    }

    /// `CreatureModelAlpha / 255`, the first factor of the unit alpha product.
    pub fn display_base_alpha(&self, display_id: u32) -> Option<f32> {
        self.display
            .get(&display_id)
            .map(|r| f32::from(r.model_alpha.min(255) as u8) / 255.0)
    }

    /// `CreatureModelData.collisionHeight`, the `h` of every depth line (swim `0.75·h`, splash
    /// `0.4·h`, foam gate `2·h`, read against `CMovement+0xb4`). It is the M2's collision-box Z
    /// extent, pre-scale: the prism is this × `max(OBJECT_FIELD_SCALE_X, creatureModelScale)`
    /// (`0x60b312` → `0x617501`). A miss takes the constructor default, `2.0277777` (`0x616fd8`).
    pub fn collision_height(&self, display_id: u32) -> Option<f32> {
        let row = self.display.get(&display_id)?;
        Some(self.models.get(&row.model_id)?.collision_height)
    }

    /// A display's foley material (`Material.dbc`), the creature half of the footfall rustle:
    /// `0x623610` hands `[[unit+0xb3c]+0x28]` to `0x4584e0`, while a player reads its chest
    /// (`0x62fa30`). Every shipped row is 0, so an NPC's silent armor is the data, not a bug.
    pub fn foley_material(&self, display_id: u32) -> Option<u32> {
        let row = self.display.get(&display_id)?;
        Some(self.models.get(&row.model_id)?.foley_material)
    }

    /// Whether the model may wear the `$BTH` breath effects: `CreatureModelData.Flags & 0x2`
    /// suppresses them (`0x600003`, the row at `[unit+0xb3c]`); an unknown display breathes.
    pub fn breathes(&self, display_id: u32) -> bool {
        self.display
            .get(&display_id)
            .and_then(|row| self.models.get(&row.model_id))
            .is_none_or(|m| m.flags & 0x2 == 0)
    }

    /// A display's footprint decal, fields 6..=8, in yards (inches × 1/36, the client's cache at
    /// `0x607a00` for the getter `0x607920`); `None` for a `−1` texture or a 0×0 print.
    pub fn footprint(&self, display_id: u32) -> Option<FootprintParams> {
        let row = self.display.get(&display_id)?;
        let model = self.models.get(&row.model_id)?;
        let texture_id = u32::try_from(model.footprint_texture).ok()?;
        let (length, width) = (model.footprint_length / 36.0, model.footprint_width / 36.0);
        (length > 0.0 && width > 0.0).then_some(FootprintParams {
            texture_id,
            length,
            width,
        })
    }

    /// Resolve an NPC display id to its model.
    pub fn model(&self, display_id: u32) -> Option<CreatureModel> {
        let row = self.display.get(&display_id)?;
        let model = self.models.get(&row.model_id)?;
        // A non-zero `ExtendedDisplayInfoID` with an extra row marks a character-model NPC.
        let npc_appearance = (row.extended_id != 0)
            .then(|| self.extra.get(&row.extended_id).cloned())
            .flatten();
        Some(CreatureModel {
            model_path: model.path.clone(),
            scale: model.scale * row.scale,
            textures: row.textures.clone(),
            npc_appearance,
            blood_display: row.blood_level as i32,
            blood_model: model.blood,
            collision_height: model.collision_height,
        })
    }

    /// Number of display entries (for logging/diagnostics).
    pub fn len(&self) -> usize {
        self.display.len()
    }

    /// Whether the catalog has no display entries.
    pub fn is_empty(&self) -> bool {
        self.display.is_empty()
    }

    /// Number of appearance rows; 0 means `CreatureDisplayInfoExtra` failed to load.
    pub fn extra_len(&self) -> usize {
        self.extra.len()
    }
}

/// CreatureModelData.dbc: 16 fields in build 5875 (no `mountHeight`).
pub(crate) fn creature_model_data_schema() -> Schema {
    let mut s = Schema::new("CreatureModelData");
    for (name, ty) in [
        ("ID", FieldType::UInt32),
        ("Flags", FieldType::UInt32),
        ("ModelName", FieldType::String),
        ("SizeClass", FieldType::UInt32),
        ("ModelScale", FieldType::Float32),
        ("BloodID", FieldType::UInt32),
        ("FootprintTextureID", FieldType::UInt32),
        ("FootprintTextureLength", FieldType::Float32),
        ("FootprintTextureWidth", FieldType::Float32),
        ("FootprintParticleScale", FieldType::Float32),
        ("FoleyMaterialID", FieldType::UInt32),
        ("FootstepShakeSize", FieldType::UInt32),
        ("DeathThudShakeSize", FieldType::UInt32),
        ("SoundID", FieldType::UInt32),
        ("CollisionWidth", FieldType::Float32),
        ("CollisionHeight", FieldType::Float32),
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    s
}

/// CreatureDisplayInfo.dbc: 12 fields in build 5875.
pub(crate) fn creature_display_info_schema() -> Schema {
    let mut s = Schema::new("CreatureDisplayInfo");
    for (name, ty) in [
        ("ID", FieldType::UInt32),
        ("ModelID", FieldType::UInt32),
        ("SoundID", FieldType::UInt32),
        ("ExtendedDisplayInfoID", FieldType::UInt32),
        ("CreatureModelScale", FieldType::Float32),
        ("CreatureModelAlpha", FieldType::UInt32),
        ("TextureVariation0", FieldType::String),
        ("TextureVariation1", FieldType::String),
        ("TextureVariation2", FieldType::String),
        // Not `PortraitTextureName` (absent in 5875): a signed size class, tested for -1 at
        // `0x625509`.
        ("SizeClass", FieldType::UInt32),
        ("BloodLevel", FieldType::UInt32),
        // Some maps label this BloodID, but its 5875 values (33..188) are NPC sound ids.
        ("NPCSoundID", FieldType::UInt32),
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    s
}

/// CreatureDisplayInfoExtra.dbc: 19 fields, 76-byte records in 5875 (vmangos `DBCStructure.h`).
pub(crate) fn creature_display_info_extra_schema() -> Schema {
    let mut s = Schema::new("CreatureDisplayInfoExtra");
    for (name, ty) in [
        ("ID", FieldType::UInt32),
        ("Race", FieldType::UInt32),
        ("Sex", FieldType::UInt32),
        ("SkinColor", FieldType::UInt32),
        ("FaceType", FieldType::UInt32),
        ("HairStyle", FieldType::UInt32),
        ("HairColor", FieldType::UInt32),
        ("FacialHair", FieldType::UInt32),
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    for i in 0..10 {
        s.add_field(SchemaField::new(format!("Equipment{i}"), FieldType::UInt32));
    }
    s.add_field(SchemaField::new("BakeName", FieldType::String));
    s
}

/// Load the creature DBCs from the patch chain into a [`CreatureCatalog`].
pub fn load_creature_catalog(chain: &mut Chain) -> Result<CreatureCatalog> {
    let models = {
        let bytes = chain
            .read_file(CREATURE_MODEL_DATA)
            .with_context(|| format!("reading {CREATURE_MODEL_DATA}"))?;
        let rs = parse(&bytes, creature_model_data_schema(), "CreatureModelData")?;
        let mut m = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            if let (Some(id), Some(name)) = (u32_at(r, 0), str_at(&rs, r, 2)) {
                m.insert(
                    id,
                    ModelRow {
                        path: name,
                        scale: f32_at(r, 4).unwrap_or(1.0),
                        flags: u32_at(r, 1).unwrap_or(0),
                        size_class: u32_at(r, 3).map_or(-1, |v| v as i32),
                        blood: u32_at(r, 5).map_or(0, |v| v as i32),
                        footprint_texture: u32_at(r, 6).map_or(-1, |v| v as i32),
                        footprint_length: f32_at(r, 7).unwrap_or(0.0),
                        footprint_width: f32_at(r, 8).unwrap_or(0.0),
                        collision_height: f32_at(r, 15).unwrap_or(0.0),
                        foley_material: u32_at(r, 10).unwrap_or(0),
                        footstep_shake: u32_at(r, 11).unwrap_or(0),
                        death_thud_shake: u32_at(r, 12).unwrap_or(0),
                    },
                );
            }
        }
        m
    };

    let display = {
        let bytes = chain
            .read_file(CREATURE_DISPLAY_INFO)
            .with_context(|| format!("reading {CREATURE_DISPLAY_INFO}"))?;
        let rs = parse(
            &bytes,
            creature_display_info_schema(),
            "CreatureDisplayInfo",
        )?;
        let mut d = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            if let (Some(id), Some(model_id)) = (u32_at(r, 0), u32_at(r, 1)) {
                d.insert(
                    id,
                    DisplayRow {
                        model_id,
                        extended_id: u32_at(r, 3).unwrap_or(0),
                        scale: f32_at(r, 4).unwrap_or(1.0),
                        textures: [str_at(&rs, r, 6), str_at(&rs, r, 7), str_at(&rs, r, 8)],
                        blood_level: u32_at(r, 10).unwrap_or(0),
                        size_class: u32_at(r, 9).map_or(-1, |v| v as i32),
                        model_alpha: u32_at(r, 5).unwrap_or(255),
                    },
                );
            }
        }
        d
    };

    // Best-effort: without it only humanoid NPCs lose their skins; the caller logs `extra_len()`.
    let extra = load_creature_display_info_extra(chain).unwrap_or_default();

    Ok(CreatureCatalog {
        display,
        models,
        extra,
    })
}

/// Load CreatureDisplayInfoExtra.dbc: `extendedDisplayInfoId` → [`NpcAppearance`].
fn load_creature_display_info_extra(chain: &mut Chain) -> Result<HashMap<u32, NpcAppearance>> {
    let bytes = chain
        .read_file(CREATURE_DISPLAY_INFO_EXTRA)
        .with_context(|| format!("reading {CREATURE_DISPLAY_INFO_EXTRA}"))?;
    let rs = parse(
        &bytes,
        creature_display_info_extra_schema(),
        "CreatureDisplayInfoExtra",
    )?;
    let mut e = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let Some(id) = u32_at(r, 0) {
            e.insert(
                id,
                NpcAppearance {
                    race: u32_at(r, 1).unwrap_or(0) as u8,
                    sex: u32_at(r, 2).unwrap_or(0) as u8,
                    skin: u32_at(r, 3).unwrap_or(0) as u8,
                    face: u32_at(r, 4).unwrap_or(0) as u8,
                    hair_style: u32_at(r, 5).unwrap_or(0) as u8,
                    hair_color: u32_at(r, 6).unwrap_or(0) as u8,
                    facial_hair: u32_at(r, 7).unwrap_or(0) as u8,
                    equipment: std::array::from_fn(|i| u32_at(r, 8 + i).unwrap_or(0)),
                    bake_name: str_at(&rs, r, 18),
                },
            );
        }
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `BloodID = −1` is a tier-2 miss, so 595 displays reach tier 3 and bleed red.
    #[test]
    fn blood_row_tiers_over_the_shipped_displays() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");
        let blood = crate::load_blood_catalog(&mut chain).expect("blood tables");

        // A tier resolves iff `level_key` finds its id with the other tier forced to miss.
        let resolves = |v: i32| {
            u32::try_from(v)
                .ok()
                .is_some_and(|k| blood.level_key(v, i32::MIN) == Some(k))
        };
        let mut tiers = [0usize; 3];
        for &display_id in cat.display.keys() {
            let Some(m) = cat.model(display_id) else {
                continue;
            };
            tiers[if resolves(m.blood_display) {
                0
            } else if resolves(m.blood_model) {
                1
            } else {
                2
            }] += 1;
        }
        assert_eq!(
            tiers,
            [36, 9903, 595],
            "tier 1 / tier 2 / tier 3 over the 10534 shipped displays"
        );
        assert_eq!(tiers.iter().sum::<usize>(), cat.display.len());
    }

    /// Tier 3 is ordinary fauna, so its records base cannot mean no blood.
    #[test]
    fn the_tier_three_fallback_bleeds_red_on_ordinary_creatures() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");
        let blood = crate::load_blood_catalog(&mut chain).expect("blood tables");

        // Each has `BloodID = −1` and no display override: tier 3 alone resolves it.
        for path in [
            r"Creature\Quillboar\QuillBoar.mdx",
            r"Creature\Crocodile\Crocodile.mdx",
            r"Creature\GnollMelee\GnollMelee.mdx",
            r"Creature\MountainGiant\MountainGiant.mdx",
            r"Creature\NagaFemale\Siren.mdx",
            r"Creature\Troll\TrollMelee.mdx",
        ] {
            let mut seen = 0usize;
            for &display_id in cat.display.keys() {
                let Some(m) = cat.model(display_id) else {
                    continue;
                };
                if !m.model_path.eq_ignore_ascii_case(path) {
                    continue;
                }
                seen += 1;
                assert_eq!(
                    (m.blood_display, m.blood_model),
                    (0, -1),
                    "{path} display {display_id} is no longer a pure tier-3 case"
                );
                assert_eq!(
                    blood.level_key(m.blood_display, m.blood_model),
                    Some(1),
                    "{path} display {display_id} must fall through to the records base (RED)"
                );
            }
            assert!(seen > 0, "{path} is missing from the shipped display table");
        }

        // The records-base row draws, both facings and both sizes.
        for (front, large) in [(true, false), (true, true), (false, false), (false, true)] {
            assert!(
                blood.effect_id(1, 2, front, large).is_some(),
                "the records-base row draws nothing (front {front}, large {large})"
            );
        }
    }

    /// Nine models have two `CreatureModelData` rows that disagree about blood, eight with one side
    /// `−1`: the value is unfilled, which tier 3 handles.
    #[test]
    fn the_minus_one_blood_id_is_unfilled_data() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");

        let mut by_path: HashMap<String, Vec<i32>> = HashMap::new();
        for m in cat.models.values() {
            by_path
                .entry(m.path.to_ascii_lowercase())
                .or_default()
                .push(m.blood);
        }
        let mut split: Vec<&str> = by_path
            .iter()
            .filter(|(_, ids)| ids.contains(&-1) && ids.iter().any(|&v| v > 0))
            .map(|(p, _)| p.as_str())
            .collect();
        split.sort_unstable();
        assert_eq!(
            split.len(),
            8,
            "models whose duplicate rows disagree about blood, one of them −1: {split:?}"
        );
        assert!(
            split.iter().any(|p| p.ends_with(r"murloc\babymurloc.mdx")),
            "the Baby Murloc pair is the clearest case and must be among them: {split:?}"
        );

        // Within one creature family, one model says −1 and its siblings name a colour.
        let blood_of = |needle: &str| {
            cat.models
                .values()
                .find(|m| m.path.to_ascii_lowercase().ends_with(needle))
                .unwrap_or_else(|| panic!("{needle} is not in CreatureModelData"))
                .blood
        };
        for (unspecified, sibling) in [
            (
                r"quillboar\quillboar.mdx",
                r"quillboar\quillboarwarrior.mdx",
            ),
            (r"gnollmelee\gnollmelee.mdx", r"gnollcaster\gnollcaster.mdx"),
            (r"troll\trollmelee.mdx", r"troll\troll.mdx"),
            (r"nagafemale\siren.mdx", r"nagamale\nagamale.mdx"),
        ] {
            assert_eq!(
                blood_of(unspecified),
                -1,
                "{unspecified} should be the unfilled one"
            );
            assert!(
                blood_of(sibling) > 0,
                "{sibling} should name a real colour, splitting its own family"
            );
        }
    }

    /// HumanMale's Base boot print (id 1) is 12×10 inches, 1/3 × 5/18 yd; a `−1` model has none.
    #[test]
    fn footprints_resolve_on_real_data() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");
        let human = cat
            .display
            .iter()
            .find(|(_, r)| {
                cat.models.get(&r.model_id).is_some_and(|m| {
                    m.path
                        .eq_ignore_ascii_case(r"Character\Human\Male\HumanMale.mdx")
                })
            })
            .map(|(&d, _)| d)
            .expect("some display wears the HumanMale body");
        let p = cat.footprint(human).expect("HumanMale leaves prints");
        assert_eq!(p.texture_id, 1, "the Base boot print");
        assert!((p.length - 12.0 / 36.0).abs() < 1e-6, "length {}", p.length);
        assert!((p.width - 10.0 / 36.0).abs() < 1e-6, "width {}", p.width);
        let printless = cat
            .display
            .iter()
            .find(|(_, r)| {
                cat.models
                    .get(&r.model_id)
                    .is_some_and(|m| m.footprint_texture == -1)
            })
            .map(|(&d, _)| d)
            .expect("some display over a printless model");
        assert_eq!(cat.footprint(printless), None);
    }

    #[test]
    fn character_model_npcs_resolve_a_shipped_baked_atlas() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");
        assert!(
            cat.extra_len() > 1000,
            "CreatureDisplayInfoExtra loaded ({} rows)",
            cat.extra_len()
        );

        let mut verified = 0;
        for (&disp, row) in &cat.display {
            if row.extended_id == 0 {
                continue;
            }
            let Some(m) = cat.model(disp) else { continue };
            let Some(bake) = m.npc_appearance.as_ref().and_then(|a| a.bake_name.as_ref()) else {
                continue;
            };
            assert!(
                m.model_path.to_ascii_lowercase().starts_with("character\\"),
                "an extended-display NPC wears a Character\\ body, got {}",
                m.model_path
            );
            let path = format!("Textures\\BakedNpcTextures\\{bake}");
            assert!(
                chain.read_file(&path).is_ok(),
                "the baked body atlas ships: {path}"
            );
            verified += 1;
            if verified >= 20 {
                break;
            }
        }
        assert!(
            verified >= 5,
            "found + verified several baked character-model NPCs (got {verified})"
        );
    }

    /// The three arms of `0x625500`: an override, a `−1` deferral and an agreeing row.
    #[test]
    fn the_size_class_column_resolves_its_three_arms() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");

        assert_eq!(
            cat.size_class(792),
            Some(3),
            "Gorilla display overrides to Giant"
        );
        assert_eq!(
            cat.size_class(170),
            Some(4),
            "Sea Giant falls back to Colossal"
        );
        assert_eq!(
            cat.size_class(1921),
            Some(3),
            "Ancient Protector agrees at Giant"
        );
        assert_eq!(cat.size_class(49), Some(1), "HumanMale is Medium");
        assert_eq!(cat.size_class(0), None, "no such display");

        // Every display lands in `0..=4`, so the reference's unsigned `>= 5` gate never fires here.
        let mut n = 0;
        for &id in cat.display.keys() {
            let class = cat
                .size_class(id)
                .expect("every display resolves a size class");
            assert!(
                class <= 4,
                "display {id} resolves to {class}, past Colossal"
            );
            n += 1;
        }
        assert_eq!(n, 10_534, "the whole shipped display table");
    }

    /// Fields 11 and 12 are `CameraShakes.dbc` ids; 25 models, all heavy, carry a footstep shake.
    #[test]
    fn the_footstep_shake_columns_are_the_thumping_giants() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");

        // Display 1921 is the plain Ancient Protector (model 67): footstep row 1, thud row 11.
        assert_eq!(
            cat.footstep_shake(1921),
            Some(1),
            "Ancient Protector footstep"
        );
        assert_eq!(
            cat.death_thud_shake(1921),
            Some(11),
            "Ancient Protector thud"
        );
        // Display 1460 is Onu's Ancient of Lore (model 187), the heavier row 2.
        assert_eq!(
            cat.footstep_shake(1460),
            Some(2),
            "Ancient of Lore footstep"
        );

        // The control: a player body shakes nothing in either column.
        assert_eq!(cat.footstep_shake(49), None, "HumanMale leaves no thump");
        assert_eq!(cat.death_thud_shake(49), None, "nor a thud");

        // Every id a creature names lands on a row of the 24-row table.
        let shakes = crate::load_camera_shakes(&mut chain).expect("load CameraShakes.dbc");
        let mut footstep = 0;
        for (_, path, foot, thud) in cat.shaking_models() {
            if foot != 0 {
                footstep += 1;
                assert!(shakes.get(foot).is_some(), "{path} footstep {foot} dangles");
            }
            if thud != 0 {
                assert!(shakes.get(thud).is_some(), "{path} thud {thud} dangles");
            }
        }
        assert_eq!(footstep, 25, "the shipped footstep-shake census");
    }

    #[test]
    fn stormwind_guard_equipment_columns_decode_in_bodyslot_order() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");
        let guard = cat
            .model(3167)
            .expect("Stormwind City Guard display 3167 resolves");
        let npc = guard
            .npc_appearance
            .expect("display 3167 is a character-model NPC with an appearance row");
        // Body-slot order: head, shoulder, shirt, chest, belt, pants, boots, wrist, gloves, tabard.
        assert_eq!(
            npc.equipment,
            [14964, 7541, 7223, 0, 7224, 7225, 7255, 0, 7698, 6255],
            "SW Guard worn-equipment ids in bodyslot order"
        );
    }

    #[test]
    fn the_breathless_models_are_the_ones_with_no_breath() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");

        assert_eq!(
            cat.models.values().filter(|m| m.flags & 0x2 != 0).count(),
            99,
            "the shipped census: 99 of {} models suppress breath",
            cat.models.len()
        );
        for (display_id, label) in [
            (49, "HumanMale"),
            (50, "HumanFemale"),
            (53, "DwarfMale"),
            (59, "TaurenMale"),
            (1564, "GnomeFemale"),
        ] {
            assert!(cat.breathes(display_id), "{label} breathes");
        }
        for (display_id, label) in [
            (158, "Skeleton"),
            (110, "WaterElemental"),
            (169, "Infernal"),
        ] {
            assert!(!cat.breathes(display_id), "{label} has no breath to see");
        }
        assert!(
            cat.breathes(0),
            "an unknown display falls to the common case"
        );
    }

    /// Every shipped model has `FoleyMaterialID` 0; a data file that grows one fails here.
    #[test]
    fn no_shipped_model_carries_a_foley() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("creature catalog");

        assert!(!cat.models.is_empty(), "models loaded");
        let foleyed: Vec<u32> = cat
            .models
            .iter()
            .filter(|(_, m)| m.foley_material != 0)
            .map(|(&id, _)| id)
            .collect();
        assert!(
            foleyed.is_empty(),
            "shipped data grew a creature foley material: models {foleyed:?}"
        );

        // Field 11 is not all zero, so the zero at field 10 is the data, not a slid schema.
        let shakers = cat
            .models
            .values()
            .filter(|m| m.footstep_shake != 0)
            .count();
        assert_eq!(shakers, 25, "footstep-shake rows (schema alignment guard)");
    }

    #[test]
    fn collision_height_is_the_m2_collision_box() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");

        // (display id, label, expected column); ids from ChrRaces Male/FemaleDisplayId.
        let races: &[(u32, &str, f32)] = &[
            (49, "HumanMale", 2.031),
            (50, "HumanFemale", 1.913),
            (51, "OrcMale", 2.361),
            (52, "OrcFemale", 2.051),
            (53, "DwarfMale", 1.667),
            (54, "DwarfFemale", 1.528),
            (55, "NightElfMale", 2.438),
            (56, "NightElfFemale", 2.250),
            (57, "ScourgeMale", 1.861),
            (58, "ScourgeFemale", 1.844),
            (59, "TaurenMale", 1.653),
            (60, "TaurenFemale", 2.111),
            (1563, "GnomeMale", 1.056),
            (1564, "GnomeFemale", 1.000),
            (1478, "TrollMale", 2.083),
            (1479, "TrollFemale", 1.839),
        ];
        for &(display_id, label, expect) in races {
            let m = cat
                .model(display_id)
                .unwrap_or_else(|| panic!("{label}: display {display_id} resolves"));
            let h = cat.collision_height(display_id).expect("collision height");
            assert!(
                (h - expect).abs() < 5e-4,
                "{label}: column is {h}, expected {expect}"
            );
            assert_eq!(h, m.collision_height, "{label}: accessor vs CreatureModel");

            let bytes = chain
                .read_file(&crate::models::model_path(&m.model_path))
                .unwrap_or_else(|e| panic!("{label}: read {}: {e:#}", m.model_path));
            let fmt = benilla_m2::parse_m2(&mut std::io::Cursor::new(&bytes[..]))
                .unwrap_or_else(|e| panic!("{label}: parse M2: {e:#}"));
            let hdr = &fmt.model().header;
            let box_z = (hdr.collision_box_max[2] - hdr.collision_box_min[2]).abs();
            assert!(
                (box_z - h).abs() < 5e-4,
                "{label}: MD20 collision box Z extent {box_z} != column {h}"
            );
        }

        // Not vacuous: the races differ.
        let gnome = cat.collision_height(1564).unwrap();
        let nelf = cat.collision_height(55).unwrap();
        assert!(
            nelf > gnome * 2.0,
            "a night elf is over twice a gnome ({nelf} vs {gnome}) — the whole point of the plumb"
        );
    }

    /// vmangos folds both scales into `SCALE_X`, so the prism's `max(SCALE_X, display scale)` floor
    /// bites only where `modelScale < 1`, and no shipped row is below 1.
    #[test]
    fn no_shipped_model_scales_below_one_so_the_prism_floor_is_inert_at_rest() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");

        let under: Vec<_> = cat
            .models
            .iter()
            .filter(|(_, m)| m.scale < 1.0)
            .map(|(id, m)| (*id, m.scale))
            .collect();
        assert!(
            under.is_empty(),
            "a sub-1 modelScale would make the floor bite at rest: {under:?}"
        );
        // Not vacuous: the column is read, and varies.
        assert!(
            cat.models.values().any(|m| m.scale > 1.0),
            "some row scales above 1.0, or this is asserting on a zeroed column"
        );
    }

    /// The prism comes from `UNIT_FIELD_NATIVEDISPLAYID`, so a druid in form keeps the druid's
    /// depth lines, where the form's row would move the swim line up to 0.72 yd. A form scales
    /// `SCALE_X` by 1.0 or 0.80 (vmangos `GetShapeshiftDisplayInfo`).
    #[test]
    fn a_shapeshift_moves_the_collision_prism_and_the_native_row_is_what_stops_it() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");

        let h = |display: u32, scale_x: f32| {
            let col = cat.collision_height(display).expect("collision height");
            let s = cat.display_scale(display).expect("display scale");
            col * scale_x.max(s)
        };

        // (label, native display, form display, native SCALE_X, the form's scale factor)
        let cases: &[(&str, u32, u32, f32, f32)] = &[
            ("NElf M → cat", 55, 892, 1.0, 0.80),
            ("NElf M → bear", 55, 2281, 1.0, 1.0),
            ("Tauren M → moonkin", 59, 15375, 1.35, 1.0),
            ("Tauren M → bear", 59, 2289, 1.35, 1.0),
        ];
        let mut worst: f32 = 0.0;
        for &(label, native, form, native_scale_x, factor) in cases {
            let scale_x = native_scale_x * factor;
            let (reference, ours_before) = (h(native, scale_x), h(form, scale_x));
            let swim_delta = 0.75 * (ours_before - reference);
            assert!(
                swim_delta.abs() > 0.05,
                "{label}: the two readings must actually differ, else this test asserts nothing \
                 (reference {reference}, form-derived {ours_before})"
            );
            worst = worst.max(swim_delta.abs());
        }
        assert!(
            (worst - 0.72).abs() < 0.02,
            "worst swim-line divergence is {worst} yd, the doc says 0.72"
        );

        // The sign flips with the form, so this is no constant offset.
        assert!(h(892, 0.80) < h(55, 0.80), "NElf cat: form row is shorter");
        assert!(
            h(15375, 1.35) > h(59, 1.35),
            "Tauren moonkin: form row is taller"
        );

        // The tauren bear value seen live: h = 2.083 × 1.35.
        assert!(
            (h(2289, 1.35) - 2.083 * 1.35).abs() < 5e-3,
            "tauren bear form-derived h should reproduce the 2.812 seen live"
        );
    }

    /// The Shore Strider (display 4945, model 35): column 2.083 × display scale 1.75 = 3.645 yd
    /// with or without the floor, since its `modelScale` is 1.0.
    #[test]
    fn the_shore_strider_prism_is_the_same_under_both_readings() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");

        let m = cat.model(4945).expect("display 4945 resolves");
        assert_eq!(m.model_path, "Creature\\SeaGiant\\SeaGiant.mdx");
        let column = cat.collision_height(4945).expect("collision height");
        let display_scale = cat.display_scale(4945).expect("display scale");
        assert!((column - 2.083).abs() < 5e-4, "column is {column}");
        assert!(
            (display_scale - 1.75).abs() < 5e-4,
            "CreatureDisplayInfo.scale is {display_scale}"
        );
        // vmangos ships `creature_template.display_scale = 0` for entry 5359, so SCALE_X is the
        // folded `modelScale × displayScale` = 1.0 × 1.75.
        let scale_x = m.scale;
        assert!((scale_x - 1.75).abs() < 5e-4, "folded SCALE_X is {scale_x}");
        assert_eq!(
            column * scale_x,
            column * scale_x.max(display_scale),
            "the floor is inert on this row — the height was not the bug"
        );
    }
}
