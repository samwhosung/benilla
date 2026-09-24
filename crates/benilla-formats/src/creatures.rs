//! Creature display resolution: `displayId` → M2 model + skin textures + scale.
//!
//! An NPC's `UNIT_FIELD_DISPLAYID` indexes **CreatureDisplayInfo.dbc**, which gives a `ModelID`
//! (into **CreatureModelData.dbc**, the `.mdx` path), a per-display scale, and up to three skin
//! texture names. Creature M2s leave their `Monster1/2/3` texture slots blank and pull the skin
//! from these names — the texture lives **in the same directory as the model** (wowdev.wiki). The
//! effective render scale is `CreatureModelData.modelScale * CreatureDisplayInfo.creatureModelScale`.
//!
//! **Character-model NPCs.** A humanoid NPC (guard, questgiver, townsfolk) uses a `Character\…` body
//! M2 — the same model a player wears — but its appearance is *not* on the wire (as a player's is);
//! it lives in **CreatureDisplayInfoExtra.dbc**, reached via `CreatureDisplayInfo.ExtendedDisplayInfoID`
//! (0 for a plain beast). That row supplies race/sex + the customization selectors and, in field 18, a
//! **bake name** — a pre-composited body atlas the client ships under `Textures\BakedNpcTextures\` and
//! loads directly (rather than compositing live like the local player). We surface it as
//! [`NpcAppearance`]; the beast skin path ([`CreatureModel::textures`]) is untouched.
//!
//! Layouts verified against build 5875 (field counts from the file header, cross-checked with
//! wowdev.wiki + vmangos `DBCStructure.h`): CreatureModelData = 16 fields (ID@0, ModelName@2,
//! ModelScale@4, CollisionHeight@15); CreatureDisplayInfo = 12 fields (ID@0, ModelID@1, ExtendedDisplayInfoID@3, Scale@4,
//! TextureVariation@6/7/8); CreatureDisplayInfoExtra = 19 fields (ID@0, Race@1, Sex@2, Skin@3, Face@4,
//! HairStyle@5, HairColor@6, FacialHair@7, Equipment@8..17, BakeName@18).

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, str_at, u32_at};

const CREATURE_MODEL_DATA: &str = "DBFilesClient\\CreatureModelData.dbc";
const CREATURE_DISPLAY_INFO: &str = "DBFilesClient\\CreatureDisplayInfo.dbc";
const CREATURE_DISPLAY_INFO_EXTRA: &str = "DBFilesClient\\CreatureDisplayInfoExtra.dbc";

/// A resolved creature: model path + effective scale + its (up to three) skin texture names, plus —
/// for a character-model NPC — the [`NpcAppearance`] that skins its body.
#[derive(Debug, Clone)]
pub struct CreatureModel {
    /// `.mdx` path (the M2 loader normalizes to `.m2`), e.g. `Creature\Basilisk\Basilisk.mdx`.
    pub model_path: String,
    /// `CreatureModelData.modelScale * CreatureDisplayInfo.creatureModelScale`.
    pub scale: f32,
    /// `textureVariation[0..2]` — bare names (no dir/extension); `None` where empty. The renderer
    /// resolves a used one to `<dir-of-model_path>\<name>.blp` for the model's `Monster1/2/3` slots.
    /// For a character-model NPC the body skin comes from [`Self::npc_appearance`]'s baked atlas
    /// regardless; these slots are empty on ~98% of such rows and unused for the body on the rest
    /// (the body build `0x5fb200` never reads them — a `Monster`-slot binding on a character M2 is
    /// a separate, untraced mechanism).
    pub textures: [Option<String>; 3],
    /// A character-model NPC's body appearance (from CreatureDisplayInfoExtra, via the display's
    /// `ExtendedDisplayInfoID`). `None` for a plain beast/monster (ExtendedDisplayInfoID 0) — those
    /// skin from [`Self::textures`], not here.
    pub npc_appearance: Option<NpcAppearance>,
    /// `CreatureDisplayInfo.BloodLevel` (+0x28) — **tier 1** of the reference's UnitBloodLevels
    /// row resolve. Not a resolved key: the three tiers need the table to know which candidate
    /// lands, so they live in [`crate::BloodCatalog::level_key`], which is what a consumer calls.
    pub blood_display: i32,
    /// `CreatureModelData.BloodID` (+0x14) — **tier 2** of the same resolve. `−1` in 122 of the
    /// 430 shipped models; that is *not* "bloodless", it is a tier-2 miss that falls through to
    /// tier 3 (see [`crate::BloodCatalog::level_key`]).
    pub blood_model: i32,
    /// `CreatureModelData.collisionHeight` — the unit's collision box height in **raw model units**
    /// (multiply by the unit's render scale for world yards; see
    /// [`CreatureCatalog::collision_height`], which is the accessor every consumer should use).
    pub collision_height: f32,
}

/// A character-model NPC's appearance, from **CreatureDisplayInfoExtra.dbc**. A humanoid NPC wears a
/// `Character\…` body M2 whose skin is not on the wire; this carries what the client needs to render
/// it: race/sex + the customization selectors (for the hair mesh + the geoset selection), the ten worn
/// **equipment** display ids (the armor geosets + the helm/shoulder attach models), and a `bake_name`
/// — the pre-composited body atlas the client ships under `Textures\BakedNpcTextures\` and loads
/// directly. `skin`/`face` are already baked into that atlas; they're kept for completeness and for the
/// live-composite fallback when a row carries no bake name.
#[derive(Debug, Clone)]
pub struct NpcAppearance {
    pub race: u8,
    pub sex: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    /// The ten worn-equipment `ItemDisplayInfo` display ids (fields 8..17), **bodyslot-indexed**:
    /// `0` head · `1` shoulder · `2` shirt · `3` chest · `4` belt · `5` pants · `6` boots · `7` wrist ·
    /// `8` gloves · `9` tabard (no cloak column — the row stops at bodyslot 9). `0` = the slot is
    /// empty. Direct display ids (not item entries — no template round-trip): the head/shoulder ids
    /// drive the helm/pauldron attach sub-models and the shirt..tabard ids drive the equipment geosets,
    /// through the same `ItemDisplayInfo` catalog + geoset machinery the player wire path uses.
    pub equipment: [u32; 10],
    /// The pre-baked body-atlas file name (bare, no dir) under `Textures\BakedNpcTextures\`; `None`
    /// when field 18 is empty (then the body composites live from the fields above, like a player).
    pub bake_name: Option<String>,
}

/// One CreatureDisplayInfo row (the parts we use).
#[derive(Debug, Clone)]
struct DisplayRow {
    model_id: u32,
    /// `ExtendedDisplayInfoID` — the CreatureDisplayInfoExtra key for a character-model NPC; 0 = none.
    extended_id: u32,
    scale: f32,
    textures: [Option<String>; 3],
    /// `BloodLevel` (field 10) — a per-display UnitBloodLevels override. `0` in 10498 of the
    /// 10534 shipped displays, and `0` is not a row of that table, so it falls through to the
    /// model's `BloodID` (see [`CreatureModel::blood_display`]).
    blood_level: u32,
    /// `SizeClass` (field 9, @+0x24) — the display's **size-class override**, read signed:
    /// `−1` (1 791 of the 10 534 shipped rows) defers to the model's own
    /// [`ModelRow::size_class`]. See [`CreatureCatalog::size_class`].
    size_class: i32,
    /// `CreatureModelAlpha` (field 5, @+0x14) — the display's **base render opacity**, 0..=255.
    /// This is the `baseAlpha` of the reference's per-unit alpha product (`0x60d2d0`, the CGUnit
    /// vtbl+0x6c getter: `CreatureDisplayInfo+0x14 × (1/255)`) — an authored translucency 445 of
    /// the 10534 shipped displays carry (wisps, spirits,
    /// ghosts; the modal non-opaque value is 128). Players are not on this chain (the getter
    /// returns a flat 1.0 for them).
    model_alpha: u32,
}

/// One CreatureModelData row (the parts we use).
#[derive(Debug, Clone)]
struct ModelRow {
    path: String,
    scale: f32,
    /// `Flags` (field 1) — see [`CreatureCatalog::breathes`] for the one bit we read.
    flags: u32,
    /// `SizeClass` (field 3, @+0x0c) — the model's own audible size, `0 Small · 1 Medium ·
    /// 2 Large · 3 Giant · 4 Colossal`, and the fallback arm of [`CreatureCatalog::size_class`].
    /// Reads signed for symmetry with the display override; every one of the 430 shipped rows is
    /// in `0..=4`, so the fallback always answers.
    size_class: i32,
    /// `BloodID` — see [`CreatureModel::blood_model`]. Reads signed: `−1` in 122 of the 430
    /// shipped rows, which the reference treats as a tier-2 miss, not as bloodlessness.
    blood: i32,
    /// `FootprintTextureID` (field 6) — the `FootprintTextures.dbc` key. Reads signed: `−1`
    /// (133 of 430 shipped rows) marks a model that leaves no prints.
    footprint_texture: i32,
    /// `FootprintTextureLength`/`Width` (fields 7/8), authored in **inches** — the client caches
    /// them ×(1/36) into yards (`0x607a00`).
    footprint_length: f32,
    footprint_width: f32,
    /// `collisionHeight` (field 15), raw model units — see [`CreatureCatalog::collision_height`].
    collision_height: f32,
    /// `FoleyMaterialID` (field 10) — a `Material.dbc` id, and the whole of a *creature's* armor
    /// foley (see [`CreatureCatalog::foley_material`]). `[unit+0xb3c]` is this row, and the
    /// reference reads it at `+0x28`, which is field 10 on the 16-field 5875 record.
    foley_material: u32,
    /// `FootstepShakeSize` (field 11) and `DeathThudShakeSize` (field 12) — **`CameraShakes.dbc`
    /// row ids**, 0 on a model that shakes nothing. Only 25 of the 430 shipped rows carry a
    /// footstep shake, and the set is exactly the thumping-giant list (Ancients, kodos, sea and
    /// mountain giants, titans, dragons, Anubisath, stone keeper, fel beast, Nian, Lord Kezzak,
    /// bear) — see [`crate::CameraShakeCatalog`] and.
    footstep_shake: u32,
    death_thud_shake: u32,
}

/// A display's footprint-decal parameters (see [`CreatureCatalog::footprint`]): the
/// `FootprintTextures.dbc` key + the print rectangle in **yards** (length along the facing,
/// width across), pre-scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FootprintParams {
    pub texture_id: u32,
    pub length: f32,
    pub width: f32,
}

/// Display/model tables loaded from the DBCs, resolving `displayId` → [`CreatureModel`].
///
/// `Default` is the **empty** catalog — the same thing "the DBC failed to load" already means to
/// every consumer (every lookup misses and the caller takes its documented fallback).
#[derive(Default)]
pub struct CreatureCatalog {
    /// CreatureDisplayInfo: displayId → row.
    display: HashMap<u32, DisplayRow>,
    /// CreatureModelData: modelId → row.
    models: HashMap<u32, ModelRow>,
    /// CreatureDisplayInfoExtra: extendedDisplayInfoId → character-model NPC appearance.
    extra: HashMap<u32, NpcAppearance>,
}

impl CreatureCatalog {
    /// A display's own `creatureModelScale` column alone — the MOUNT scale law (`0x613ef0`: a
    /// rendered mount = `OBJECT_FIELD_SCALE_X ×
    /// CreatureDisplayInfo.creatureModelScale`; `CreatureModelData.modelScale` does NOT multiply
    /// in, unlike [`CreatureModel::scale`]'s spawned-creature product). `None` when the display
    /// id misses.
    pub fn display_scale(&self, display_id: u32) -> Option<f32> {
        self.display.get(&display_id).map(|r| r.scale)
    }

    /// A display's **footstep camera-shake preset** — `CreatureModelData.FootstepShakeSize`, a
    /// `CameraShakes.dbc` row id fired on each footfall of a heavy enough creature. `None` when
    /// the display id misses or the model shakes nothing (405 of the 430 shipped models).
    ///
    /// The trigger, the evaluator and the distance falloff are decision 1540's; this is the
    /// authored id and nothing more.
    pub fn footstep_shake(&self, display_id: u32) -> Option<u32> {
        let row = self.display.get(&display_id)?;
        let id = self.models.get(&row.model_id)?.footstep_shake;
        (id != 0).then_some(id)
    }

    /// A display's **death-thud camera-shake preset** — `CreatureModelData.DeathThudShakeSize`,
    /// the one-off shake as the body lands. Same conventions as [`Self::footstep_shake`].
    pub fn death_thud_shake(&self, display_id: u32) -> Option<u32> {
        let row = self.display.get(&display_id)?;
        let id = self.models.get(&row.model_id)?.death_thud_shake;
        (id != 0).then_some(id)
    }

    /// A display's **audible size class** — `0 Small · 1 Medium · 2 Large · 3 Giant ·
    /// 4 Colossal`, the axis [`crate::DeathThudCatalog`] picks the body-fall sample on.
    ///
    /// The reference's own two-step (`0x625500`): the **display's** `SizeClass` column wins, and
    /// only its `−1` sentinel defers to the **model's**. That order matters — 8 743 of the 10 534
    /// shipped displays override their model, so reading the model alone would mis-size most of
    /// the world (every Small display sharing a Medium model, for one).
    ///
    /// `None` when either lookup misses, or when **both** rows read `−1`. No shipped row does, so
    /// that arm exists to keep a broken/absent table silent rather than Small — the same thing
    /// the reference's own unsigned `sizeClass >= 5` gate (`0x623744`) does with a `−1`.
    pub fn size_class(&self, display_id: u32) -> Option<u32> {
        let row = self.display.get(&display_id)?;
        let class = match row.size_class {
            -1 => self.models.get(&row.model_id)?.size_class,
            c => c,
        };
        u32::try_from(class).ok()
    }

    /// Every `CreatureModelData` row that names a camera-shake preset: `(model id, path,
    /// footstep id, death-thud id)`. The **census** view — the runtime reads
    /// [`Self::footstep_shake`] by display id instead, because that is what a unit carries.
    pub fn shaking_models(&self) -> impl Iterator<Item = (u32, &str, u32, u32)> + '_ {
        self.models
            .iter()
            .filter(|(_, m)| m.footstep_shake != 0 || m.death_thud_shake != 0)
            .map(|(id, m)| (*id, m.path.as_str(), m.footstep_shake, m.death_thud_shake))
    }

    /// Every `CreatureModelData` model path, unordered — the census surface for "is this M2 a
    /// creature model?", which is what decides whether an animation event on it reaches
    /// `CGUnit_C::HandleAnimEvent` at all.
    pub fn model_paths(&self) -> impl Iterator<Item = &str> + '_ {
        self.models.values().map(|m| m.path.as_str())
    }

    /// Every `CreatureModelData` row as `(model id, path, its own SizeClass)` — the **census**
    /// view of the death-thud size axis. The runtime reads [`Self::size_class`] by *display* id
    /// instead, because most displays override this column.
    pub fn sized_models(&self) -> impl Iterator<Item = (u32, &str, i32)> + '_ {
        self.models
            .iter()
            .map(|(id, m)| (*id, m.path.as_str(), m.size_class))
    }

    /// Every display as `(display id, its model id)` — the census hop that lets an instrument roll
    /// a per-display answer (a resolved size class, say) up onto the model that carries the M2.
    pub fn display_models(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.display.iter().map(|(id, r)| (*id, r.model_id))
    }

    /// A display's **spawned-creature render scale** — the product
    /// `CreatureModelData.modelScale × CreatureDisplayInfo.creatureModelScale`, the same number
    /// [`CreatureModel::scale`] carries, without the row's string clones.
    ///
    /// Almost nothing in the world reads this: the server folds it into `OBJECT_FIELD_SCALE_X` and
    /// the client renders a unit at that field alone, so multiplying it again would square it
    /// (`crate::entities::attach`'s note, `0x613ef0`). The **glue
    /// screens are the exception** — the character-select pet has no wire object and therefore no
    /// server scale, and the reference sizes it with exactly this product (`0x472dc6`
    /// `fld [x+0x10]; fmul [y+0x10]` → a uniform `diag(S,S,S,1)`). `None` when either DBC lookup
    /// misses.
    pub fn model_scale(&self, display_id: u32) -> Option<f32> {
        let row = self.display.get(&display_id)?;
        let model = self.models.get(&row.model_id)?;
        Some(model.scale * row.scale)
    }

    /// A display's **base render alpha** in `0.0..=1.0` — `CreatureDisplayInfo.CreatureModelAlpha`
    /// / 255 ([`DisplayRow::model_alpha`]). The first factor of the unit alpha product the aura
    /// CharProc nodes multiply into (`crate::aura_visual`). `None` for an unknown display; a known
    /// display with no authored translucency reads `1.0`.
    pub fn display_base_alpha(&self, display_id: u32) -> Option<f32> {
        self.display
            .get(&display_id)
            .map(|r| f32::from(r.model_alpha.min(255) as u8) / 255.0)
    }

    /// A display's **collision height** in raw model units — `CreatureModelData.collisionHeight`,
    /// the per-unit `h` every depth line in the client is a fraction of (swim at `0.75·h`, splash at
    /// `0.4·h`, the foam gate at `2·h`; the reference reads each against `CMovement+0xb4`). World
    /// yards = this × the unit's render scale (`OBJECT_FIELD_SCALE_X`, which the server has already
    /// folded the DBC scales into) — the caller multiplies, because only it knows the live scale.
    ///
    /// The column is **exactly the model's own MD20 collision-box Z extent**, verified against the
    /// shipped client for all thirteen character models (`tests::collision_height_is_the_m2_box`) —
    /// which is what settles the space: it is authored pre-scale, like the geometry it bounds.
    /// `None` when either DBC lookup misses; callers fall back to the client's own ctor default
    /// (`2.0277777`, `0x616fd8`).
    pub fn collision_height(&self, display_id: u32) -> Option<f32> {
        let row = self.display.get(&display_id)?;
        Some(self.models.get(&row.model_id)?.collision_height)
    }

    /// The display's **foley material** — a `Material.dbc` id, the creature half of the footfall
    /// rustle (`0x623610`: `[[unit+0xb3c]+0x28]` handed straight to `0x4584e0`). A *player* does
    /// not come through here at all: its own override reads the equipped chest instead
    /// (`0x62fa30`), so this is the answer for creatures — and, for a player wearing a
    /// non-character display, the body it is actually wearing.
    ///
    /// `None` when either DBC lookup misses. `Some(0)` is the real "no material" the data
    /// carries, and resolves to silence at [`crate::MaterialCatalog::foley_kit`].
    ///
    /// **In shipped 5875 data this column is 0 in every one of the 430 rows**, in both the base
    /// archive's 333-row copy and the patched 430 (`tests::no_shipped_model_carries_a_foley`).
    /// The creature branch of the foley is therefore inert against the real client's own files:
    /// the armor rustle you hear is the *player* override's, and no NPC has one. Kept because it
    /// is the reference's own path and one map lookup, and because a server shipping patched
    /// DBCs would light it up — not because it does anything today. A future reader finding this
    /// silent has found the data, not a bug.
    pub fn foley_material(&self, display_id: u32) -> Option<u32> {
        let row = self.display.get(&display_id)?;
        Some(self.models.get(&row.model_id)?.foley_material)
    }

    /// Does this display's model **breathe** — i.e. may it wear the `$BTH` hardcoded effects
    /// (cold vapour, underwater bubbles, inebriated bubbles)?
    ///
    /// `CreatureModelData.Flags & 0x2` suppresses the whole family (`0x600003`, the row at
    /// `[unit+0xb3c]`): 99 of the 430 shipped
    /// rows carry it — skeletons, ghosts, ghouls, zombies, banshees, every elemental, golems,
    /// slimes, infernals, voidwalkers, succubi, spiders, frogs, crocodiles, turtles, totems. The
    /// things that have no breath to see. Every player row is `0x4`, so players pass.
    ///
    /// An unknown display breathes — this catalog's degrade shape is "fall back to the common
    /// case", and the common case is 331 of the 430 rows.
    pub fn breathes(&self, display_id: u32) -> bool {
        self.display
            .get(&display_id)
            .and_then(|row| self.models.get(&row.model_id))
            .is_none_or(|m| m.flags & 0x2 == 0)
    }

    /// A display's **footprint decal** parameters — `CreatureModelData` fields 6..=8 through the
    /// display→model chain, sizes converted to **yards** (the client's own ×(1/36) inches→yards
    /// cache, byte-verified at `0x607a00` for the mounted getter `0x607920`). `None` when either
    /// lookup misses, the model authors `FootprintTextureID = −1` (no prints — 133 of 430 shipped
    /// rows), or the print rectangle is degenerate (40 rows carry an id over a 0×0 size). World
    /// yards = these × the unit's render scale — the caller multiplies, like `collision_height`.
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

    /// Resolve an NPC display id to its model, or `None` if either DBC lookup misses.
    pub fn model(&self, display_id: u32) -> Option<CreatureModel> {
        let row = self.display.get(&display_id)?;
        let model = self.models.get(&row.model_id)?;
        // A non-zero ExtendedDisplayInfoID that resolves to an extra row ⇒ a character-model NPC.
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

    /// Number of character-model NPC appearance rows loaded (for logging/diagnostics). Zero here means
    /// CreatureDisplayInfoExtra failed to load ⇒ humanoid NPCs stay untextured.
    pub fn extra_len(&self) -> usize {
        self.extra.len()
    }
}

/// CreatureModelData.dbc — 16 fields in build 5875 (no `mountHeight`). We read ID, ModelName, scale.
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

/// CreatureDisplayInfo.dbc — 12 fields in build 5875. We read ID, ModelID, scale, 3 skin textures.
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
        // **NOT `PortraitTextureName`** — 5875 has no such column, and reading field 9 as a
        // string would resolve a size class into the string block. The client reads it as a
        // signed int at `+0x24` and compares it to `-1` (`0x625509`); the shipped column takes
        // exactly `{-1, 0, 1, 2, 3, 4}` across all 10 534 rows, `-1` on 1 791 of them.
        ("SizeClass", FieldType::UInt32),
        ("BloodLevel", FieldType::UInt32),
        // Labeled BloodID in some third-party maps, but the 5875 values (33..188, dense) are the
        // NPC sound-kit range, not blood ids — the blood override is BloodLevel above.
        ("NPCSoundID", FieldType::UInt32),
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    s
}

/// CreatureDisplayInfoExtra.dbc — 19 fields in build 5875 (`19 × 4 == 76`-byte records; field map
/// cross-checked with vmangos `DBCStructure.h`). We read the appearance selectors, the 10 equipment
/// columns (8..17 — `ItemDisplayInfo` display ids for the worn armor geosets + helm/shoulder attach),
/// and the bake name.
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
                        // BloodID (field 5) reads signed: −1 in 122 of the 430 shipped rows.
                        // That is a tier-2 MISS, not bloodlessness — the resolve falls through
                        // to the records base (1850). Kept signed so the miss is visible.
                        blood: u32_at(r, 5).map_or(0, |v| v as i32),
                        // FootprintTextureID reads signed too: −1 = no prints (see ModelRow docs).
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
                        // Signed: −1 is "no override", not a size class (see the field docs).
                        size_class: u32_at(r, 9).map_or(-1, |v| v as i32),
                        model_alpha: u32_at(r, 5).unwrap_or(255),
                    },
                );
            }
        }
        d
    };

    // CreatureDisplayInfoExtra — the character-model NPC appearance table. Best-effort: a plain beast
    // catalog is still useful without it (only humanoid NPCs need it), so a load failure degrades to an
    // empty map (humanoid NPCs stay untextured) rather than sinking the whole catalog. The caller logs
    // `extra_len()` so a `0` (unexpected — it ships in patch.MPQ) is visible.
    let extra = load_creature_display_info_extra(chain).unwrap_or_default();

    Ok(CreatureCatalog {
        display,
        models,
        extra,
    })
}

/// Load CreatureDisplayInfoExtra.dbc → `extendedDisplayInfoId` → [`NpcAppearance`].
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
                    // fields 8..17 — the ten worn-equipment ItemDisplayInfo display ids, bodyslot-indexed
                    // (0 head · 1 shoulder · 2 shirt … 9 tabard); `0` = the slot is empty.
                    equipment: std::array::from_fn(|i| u32_at(r, 8 + i).unwrap_or(0)),
                    // field 18 — the baked body-atlas name.
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

    /// **The blood-row tier populations** over the shipped tables — the measurement behind 1850.
    /// `CreatureModelData.BloodID = −1` (122 of 430 models) is a tier-2 *miss*, not a bloodless
    /// marker, so those displays fall through to the reference's tier-3 records base and bleed
    /// RED. benilla read `−1` as bloodless and dropped the spurt on all 595 of them.
    #[test]
    fn blood_row_tiers_over_the_shipped_displays() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");
        let blood = crate::load_blood_catalog(&mut chain).expect("blood tables");

        // A tier resolves iff its id names a real UnitBloodLevels row — which is exactly what
        // `level_key` reports when the *other* tier is forced to miss.
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

    /// **Tier 3 is ordinary fauna, not an exotic tail** — the content proof behind 1859, and the
    /// reason the records-base fallback cannot mean "no blood".
    ///
    /// Read the tier-3 population by its oddest members — elementals, skeletons, mecha-striders —
    /// and the natural conclusion is that `BloodID = −1` marks a bloodless creature and the
    /// fallback is a misread of the disassembly. The population says otherwise: it is *headed* by
    /// Quilboar (42 displays), Mountain Giants (30), Crocolisks (27), Gnolls (25), Nagas (24) and
    /// Trolls (17). A fallback that resolved to "no blood" would leave Razorfen, every gnoll camp
    /// and every Stranglethorn troll bloodless — which is not the game anyone played. That is a
    /// proof from shipped content, independent of any instruction decode, so the creatures are
    /// named here: a future round that "simplifies" `level_key` back to two tiers fails saying
    /// *which creature stopped bleeding*, not merely that a count moved.
    #[test]
    fn the_tier_three_fallback_bleeds_red_on_ordinary_creatures() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_creature_catalog(&mut chain).expect("load creature catalog");
        let blood = crate::load_blood_catalog(&mut chain).expect("blood tables");

        // Each authors `BloodID = −1` on a single, uniquely-pathed model row, and none of their
        // displays carries a `BloodLevel` override — so every one reaches the resolve by tier 3
        // alone, with no other tier able to account for the result.
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

        // …and the row it lands on really draws — red, both facings, both sizes.
        for (front, large) in [(true, false), (true, true), (false, false), (false, true)] {
            assert!(
                blood.effect_id(1, 2, front, large).is_some(),
                "the records-base row draws nothing (front {front}, large {large})"
            );
        }
    }

    /// **`BloodID = −1` is unfilled data, not a "bloodless" marker** — the evidence that closes
    /// the question 1850 left open (1859).
    ///
    /// Nine shipped models appear under **two** `CreatureModelData` rows for the same art and the
    /// twins *disagree* about blood; in eight of the nine, one side of the disagreement is `−1`
    /// (the ninth, FelBat, splits 1 vs 2). A Baby Murloc is a Baby
    /// Murloc. If `−1` meant "this creature does not bleed", the same creature would bleed or not
    /// depending on which of its two rows a display happened to name — so `−1` is an unspecified
    /// value, and the reference's tier-3 records base is precisely the handler for one.
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

        // The same unfilled field, one level up: within a single creature family one model says
        // −1 and its siblings name a colour. A "bloodless" reading would put bleeding quilboar
        // warriors next to bloodless quilboar in the same Razorfen room.
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

    /// The footprint accessor on the **real** build-5875 DBCs: a display wearing the HumanMale
    /// body resolves the Base boot print (`FootprintTextures` id 1) at the authored 12×10 inches
    /// → 1/3 × 5/18 yards (the client's ×1/36 cache conversion, byte-verified at `0x607a00`);
    /// a display whose model authors `FootprintTextureID = −1` resolves `None`. Guards the
    /// schema columns (a shifted field would misread every print) and the inches→yards space.
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
        // A −1 model (133 shipped rows) yields no print params through any display over it.
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

    /// End-to-end on the **real** build-5875 DBCs: the `ExtendedDisplayInfoID` chain resolves
    /// character-model NPCs (guards/townsfolk) to a CreatureDisplayInfoExtra appearance whose body is a
    /// `Character\` M2 skinned by a pre-baked atlas that actually ships under `Textures\BakedNpcTextures\`.
    /// Guards the extra schema (a shifted column would misread the bake name), the display→extra join, and
    /// the baked-texture-path convention. Skips when the client data isn't present.
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

        // Every display that resolves both an appearance and a bake name is definitively a character
        // model — confirm the body path + that its baked atlas is a real, readable file.
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

    /// The worn-equipment columns (fields 8..17) decode in bodyslot order, anchored on live
    /// server-truth: the **Stormwind City Guard** (display 3167) carries a plate helm (slot 0), a
    /// pauldron pair (slot 1), and boot/glove/tabard geosets (slots 6/8/9), with empty chest/wrist
    /// (slots 3/7). These ids are the real `CreatureDisplayInfoExtra` values read off the build-5875
    /// DBC; a shifted column or a wrong field offset would misread them. Skips without the client data.
    /// **The size-class column, pinned against the shipped client.** The death-thud sound
    /// (`benilla-app`'s `sound::death_thud`) picks its sample on this axis alone, so a shifted
    /// field would re-size the whole world's body-falls silently.
    ///
    /// The three arms of `0x625500`, each on a row that exercises it:
    /// - the **display override** wins — display 792 is a Gorilla (model 136, class `0` Small)
    ///   overridden to `3` Giant, which is the whole reason the override exists;
    /// - `−1` **defers to the model** — display 170 is a Sea Giant with no override and the
    ///   model's `4` Colossal;
    /// - and an ordinary agreeing row (the Ancient Protector, `3` Giant on both).
    ///
    /// The control is the human male at `1` Medium: a schema shift onto a neighbouring column
    /// would have to keep *all four* of these, and the string-block column that used to be
    /// misread here (field 9 was labelled `PortraitTextureName`) cannot.
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

        // Every shipped display resolves, and inside the reference's own `0..=4` bound — so its
        // unsigned `>= 5` gate never fires on this data, and no body-fall is silent for want of
        // a size.
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

    /// **The shake columns, pinned against the shipped client**. Fields 11
    /// and 12 are `CameraShakes.dbc` row ids, and the evidence that the map is right is not that
    /// the names look plausible — it is that the census is *semantic*: 25 of 430 rows carry a
    /// footstep shake and every one of them is a creature heavy enough to shake a camera, the
    /// amplitude ranks by mass, and nothing dangles.
    ///
    /// The Ancient Protector is the reported row (Dolanaar's tree guardians); the human male
    /// is the control that must stay zero, since a schema shift would smear a neighbouring column
    /// into these and give *everything* a shake.
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
        // Display 1460 is Onu's Ancient of Lore (model 187) — the heavier row 2.
        assert_eq!(
            cat.footstep_shake(1460),
            Some(2),
            "Ancient of Lore footstep"
        );

        // The control: a player body shakes nothing, at either column. If a schema shift walked
        // these indices onto a neighbour, this is the assert that catches it.
        assert_eq!(cat.footstep_shake(49), None, "HumanMale leaves no thump");
        assert_eq!(cat.death_thud_shake(49), None, "nor a thud");

        // The census, and the property that licenses the whole column map: every id a creature
        // names must land on a real row of the 24-row table.
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
        // 0 head · 1 shoulder · 2 shirt · 3 chest · 4 belt · 5 pants · 6 boots · 7 wrist · 8 gloves · 9 tabard.
        assert_eq!(
            npc.equipment,
            [14964, 7541, 7223, 0, 7224, 7225, 7255, 0, 7698, 6255],
            "SW Guard worn-equipment ids in bodyslot order"
        );
    }

    /// **The column's space, pinned.** `CreatureModelData.collisionHeight` is not a derived or
    /// hand-authored number: it is a verbatim copy of the model's own MD20 **collision-box** Z
    /// extent, in raw model units. Asserted for all thirteen player-race body models against the
    /// shipped client — which is what licenses [`CreatureCatalog::collision_height`]'s contract
    /// that world yards = column × render scale (it is pre-scale, exactly like the geometry it
    /// bounds), and pins field index 15 so a schema shift can't silently return garbage.
    ///
    /// Tauren Female is the load-bearing row: `modelScale` 1.25 with a 2.111 column. If the column
    /// were authored post-`modelScale` the box would read 2.111/1.25 = 1.689, so this row alone
    /// refutes the "divide the model scale out" reading (which is what vmangos's server-side
    /// `Unit::UpdateModelData` does).
    /// `CreatureModelData.Flags & 0x2` — the `$BTH` suppression. The census
    /// on the shipped table is 99 of 430 rows, and the split is semantic, not arbitrary: the
    /// things with no breath to see. **Every player row passes** (they carry `0x4`), which is what
    /// makes the flag safe to gate the reported case on.
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
        // Every playable race's own display breathes — the reported case.
        for (display_id, label) in [
            (49, "HumanMale"),
            (50, "HumanFemale"),
            (53, "DwarfMale"),
            (59, "TaurenMale"),
            (1564, "GnomeFemale"),
        ] {
            assert!(cat.breathes(display_id), "{label} breathes");
        }
        // …and the breathless: a skeleton, a water elemental, an infernal.
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

    /// **The creature foley is dead data in 5875.** Every `CreatureModelData` row ships
    /// `FoleyMaterialID = 0`, so `0x623610`'s branch resolves to silence for every NPC in the
    /// game and the armor rustle is the player override's alone. Pinned as a test rather than a
    /// comment because it is a *negative* that a future reader will otherwise re-derive by
    /// wondering why NPCs are quiet — and because a shipped file that ever grows a nonzero here
    /// should make this fail loudly rather than change the soundscape silently.
    ///
    /// The row alignment this rests on is checked in the same pass: field 11 (footstep shake)
    /// is nonzero on exactly the 25 thumping-giant rows [`ModelRow`] documents, which would not
    /// hold if the schema had slipped a column. Skips without client data.
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

        // The alignment guard: the neighbouring column is NOT uniformly zero, so a zero at
        // field 10 is the data's own answer and not a schema that slid.
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

        // (display id, label, the column's expected value). Display ids are the ChrRaces
        // Male/FemaleDisplayId columns; Goblin shares one display across both sexes.
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

        // Not vacuous: the races must actually differ, or "one constant for everyone" would pass.
        let gnome = cat.collision_height(1564).unwrap();
        let nelf = cat.collision_height(55).unwrap();
        assert!(
            nelf > gnome * 2.0,
            "a night elf is over twice a gnome ({nelf} vs {gnome}) — the whole point of the plumb"
        );
    }

    /// **Why the collision-prism FLOOR is invisible on shipped data** — and therefore why its
    /// absence hid until a server override went looking for it (B311's triage).
    ///
    /// The real client's prism height is `CollisionHeight × max(SCALE_X, CreatureDisplayInfo.scale)`
    /// (`0x60b312` → `0x617501`). vmangos folds `modelScale × displayScale` into `SCALE_X`, so the
    /// floor can only bite where `modelScale < 1` would drag the product under the display column —
    /// and **no shipped row scales below 1.0**. So on stock data `max` always picks `SCALE_X`, our
    /// old `× SCALE_X` was bit-identical, and only a `creature_template.display_scale` override or
    /// a shrink aura can separate the two.
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
        // Not vacuous: the column is really read, and really varies.
        assert!(
            cat.models.values().any(|m| m.scale > 1.0),
            "some row scales above 1.0, or this is asserting on a zeroed column"
        );
    }

    /// **The shapeshift divergence, pinned in numbers**. The reference derives the
    /// collision prism from `UNIT_FIELD_NATIVEDISPLAYID`, so a druid in a form keeps the druid's
    /// depth lines. This asserts the two readings really differ on shipped data, and by how much —
    /// a doc claiming "up to 0.72 yd" is worth nothing if the DBC rows drift under it.
    ///
    /// `h = collisionHeight × max(SCALE_X, CreatureDisplayInfo.scale)` on the row named. Player
    /// `SCALE_X` starts at `modelScale × CDI.scale` and a shapeshift multiplies it by the form's
    /// own factor (vmangos `GetShapeshiftDisplayInfo`: 1.0 for bear/moonkin/tree, 0.80 for
    /// cat/travel/aquatic) — both live-confirmed by decision 0695's own probe (tauren bear
    /// `h = 2.083 × 1.35`, tauren cat `SCALE_X 1.35 → 1.08`).
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
                "{label}: the two readings must actually differ, else this test asserts nothing                  (reference {reference}, form-derived {ours_before})"
            );
            worst = worst.max(swim_delta.abs());
        }
        assert!(
            (worst - 0.72).abs() < 0.02,
            "worst swim-line divergence is {worst} yd, the doc says 0.72"
        );

        // The direction flips with the form, which is why this can't be waved off as a constant
        // offset: a night elf cat swims too EARLY, a tauren moonkin far too LATE.
        assert!(h(892, 0.80) < h(55, 0.80), "NElf cat: form row is shorter");
        assert!(
            h(15375, 1.35) > h(59, 1.35),
            "Tauren moonkin: form row is taller"
        );

        // 0695's own live probe, reproduced from the DBCs: tauren bear h = 2.083 × 1.35.
        assert!(
            (h(2289, 1.35) - 2.083 * 1.35).abs() < 5e-3,
            "tauren bear form-derived h should reproduce 0695's observed 2.812"
        );
    }

    /// **The Shore Strider, pinned**. The reported giant's own chain and
    /// numbers, recorded so nobody re-suspects the height: display 4945 → `CreatureModelData` 35,
    /// `Creature\SeaGiant\SeaGiant.mdx`, column 2.083 over a display scale of 1.75 and a
    /// `modelScale` of 1.0. Its prism is `2.083 × 1.75 = 3.645` yd under **both** the old
    /// `× SCALE_X` reading and the corrected `× max(SCALE_X, displayScale)` — identical to the
    /// float — so the height was never why it glided. The cause was the missing
    /// `UNIT_FIELD_FLAGS` enter gate; this row is the control that says so.
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
