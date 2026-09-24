use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::items::ItemDisplay;
use crate::{read_texture_mip_chain, BlpMipChain, Chain};

const CHAR_SECTIONS: &str = "DBFilesClient\\CharSections.dbc";

/// CharSections `sectionType` values (facial hair 2 at `0x478660`, hair 3 at `0x4784c0`).
const SECTION_SKIN: u8 = 0;
const SECTION_FACE: u8 = 1;
const SECTION_FACIAL_HAIR: u8 = 2;
const SECTION_HAIR: u8 = 3;
const SECTION_UNDERWEAR: u8 = 4;

/// The hair variation the type-6 binder falls back to: a literal 1 at `0x478445` and `0x4786f2`.
const HAIR_SUBSTITUTE_VARIATION: u8 = 1;

/// An atlas rect `(x, y, w, h)` in pixels.
type Tile = (u32, u32, u32, u32);

/// The head strip of the 256² partition `0x475c50` writes: g8 the upper band, g9 the lower.
const TILE_G8: Tile = (0, 160, 128, 32);
const TILE_G9: Tile = (0, 192, 128, 64);
// The underwear's torso and pelvis tiles are equipment tiles g3 and g5, named for the tests.
#[cfg(test)]
const TILE_G3: Tile = EQUIP_TILES[3];
#[cfg(test)]
const TILE_G5: Tile = EQUIP_TILES[5];

/// The underwear's blits, `(TextureName column, layer, cells tested)`: the tile's fallback, drawn
/// by `0x4772f0` (TorsoUpper, `cc+0x20c`) only with cells 0–2 empty and by `0x4773a0` (LegUpper,
/// `cc+0x208`) only with cells 0–1 empty; `0x478790` fills both from the sectionType-4 row.
const UNDERWEAR_TILES: [(usize, usize, i8); 2] = [
    (0, 5, 2), // pelvis into LegUpper: pants and chest tested, not the belt
    (1, 3, 3), // torso into TorsoUpper: shirt, chest and a guild tabard's background tested
];

/// The equipment tiles g0–g7 of `0x475c50`: ItemDisplayInfo texture column i is layer i, tile gi.
const EQUIP_TILES: [Tile; 8] = [
    (0, 0, 128, 64),     // g0 ArmUpper
    (0, 64, 128, 64),    // g1 ArmLower
    (0, 128, 128, 32),   // g2 Hand
    (128, 0, 128, 64),   // g3 TorsoUpper
    (128, 64, 128, 32),  // g4 TorsoLower
    (128, 96, 128, 64),  // g5 LegUpper
    (128, 160, 128, 64), // g6 LegLower
    (128, 224, 128, 32), // g7 Foot
];

/// The `Item\TextureComponents\` directory per layer, the eight the data ships.
const EQUIP_TEX_DIRS: [&str; 8] = [
    "ArmUpperTexture",
    "ArmLowerTexture",
    "HandTexture",
    "TorsoUpperTexture",
    "TorsoLowerTexture",
    "LegUpperTexture",
    "LegLowerTexture",
    "FootTexture",
];

/// The `[0x803bf8]` table: per bodyslot 2–9 and layer 0–7, the default cell of the slot's art,
/// `-1` for never; two layer choosers (`[0xb42424 + layer*4]`) overrule it ([`equip_column`]).
/// A row holds one record per cell, blitted by ascending cell.
const EQUIP_LAYER_COLUMN: [[i8; 8]; 8] = [
    [0, 0, -1, 0, 0, -1, -1, -1],    // shirt
    [1, 1, -1, 1, 1, 1, 1, -1],      // chest (robes reach the legs)
    [-1, -1, -1, -1, -1, 2, -1, -1], // belt
    [-1, -1, -1, -1, -1, 0, 0, -1],  // pants
    [-1, -1, -1, -1, -1, -1, 2, 0],  // boots
    [-1, 2, -1, -1, -1, -1, -1, -1], // wrist
    [-1, 3, 0, -1, -1, -1, -1, -1],  // gloves
    [-1, -1, -1, 4, 4, -1, -1, -1],  // tabard
];

/// Worn-slot indices in `equipment` order, bodyslot − 2.
const SLOT_CHEST: usize = 1;
const SLOT_PANTS: usize = 3;
const SLOT_BOOTS: usize = 4;
const SLOT_GLOVES: usize = 6;
/// The tabard's worn slot, whose display decides whether the guild emblem installs.
const SLOT_TABARD: usize = 7;

/// Where the guild tabard's art lives.
const GUILD_EMBLEM_DIR: &str = "Textures\\GuildEmblems";

/// The guild tabard's layers and name halves (`0x47a610`): TorsoUpper `_TU_`, TorsoLower `_TL_`.
const EMBLEM_LAYERS: [(usize, &str); 2] = [(3, "TU"), (4, "TL")];

fn emblem_half(layer: usize) -> Option<&'static str> {
    EMBLEM_LAYERS
        .iter()
        .find_map(|&(l, half)| (l == layer).then_some(half))
}

/// CharSections: (race, sex, sectionType, variation, colorIndex) → the row's `TextureName`s.
pub struct CharSections {
    sections: HashMap<(u8, u8, u8, u8, u8), [String; 3]>,
}

impl CharSections {
    /// The 256² base body skin for a skin colour (`sectionType 0`).
    pub fn skin_texture(&self, race: u8, sex: u8, skin_color: u8) -> Option<&str> {
        self.tex(race, sex, SECTION_SKIN, 0, skin_color, 0)
    }

    /// The extra skin (`sectionType 0`, `TextureName[1]`), bound uncomposited to M2 texture type 8;
    /// only the fur races author it (tauren).
    pub fn skin_extra_texture(&self, race: u8, sex: u8, skin_color: u8) -> Option<&str> {
        self.tex(race, sex, SECTION_SKIN, 0, skin_color, 1)
    }

    /// The hair row's `TextureName[0]`, colour baked in; meshes use [`Self::hair_mesh_texture`].
    pub fn hair_texture(&self, race: u8, sex: u8, hair_style: u8, hair_color: u8) -> Option<&str> {
        self.tex(race, sex, SECTION_HAIR, hair_style, hair_color, 0)
    }

    /// The sheet for M2 texture type 6, the hair and on several races the beard: the style's row,
    /// else variation 1 at the same colour. The client's binder `0x478220` ignores an empty name
    /// (`0x47827d`) and runs for variation 1 (`0x478445`, `0x4786f2`) and the style (`0x478450`);
    /// on the shipped data its last non-empty bind is always this.
    pub fn hair_mesh_texture(
        &self,
        race: u8,
        sex: u8,
        hair_style: u8,
        hair_color: u8,
    ) -> Option<&str> {
        self.hair_texture(race, sex, hair_style, hair_color)
            .or_else(|| self.hair_texture(race, sex, HAIR_SUBSTITUTE_VARIATION, hair_color))
    }

    /// One `TextureName` column of a row; an empty name is `None`.
    fn tex(&self, race: u8, sex: u8, ty: u8, var: u8, color: u8, col: usize) -> Option<&str> {
        self.sections
            .get(&(race, sex, ty, var, color))
            .map(|t| t[col].as_str())
            .filter(|s| !s.is_empty())
    }

    /// The body-skin atlas as one mip pyramid: the 256² base skin, the head overlays, the
    /// underwear, the equipment by bodyslot − 2 and the guild emblem ([`equip_blits`]), each at its
    /// tile per authored mip level (`0x475c50`, `0x4770f0`); `Ok(None)` without a base skin row.
    ///
    /// Deviation: blends in 8-bit RGBA, not the client's RGB565 with 2-bit coverage, because that
    /// format only saves texture memory and loses precision.
    pub fn composite_body(
        &self,
        chain: &mut Chain,
        race: u8,
        sex: u8,
        skin: u8,
        face: u8,
        facial_hair: u8,
        hair_style: u8,
        hair_color: u8,
        equipment: [Option<&ItemDisplay>; 8],
        emblem: Option<GuildEmblem>,
        tabard_preview: bool,
    ) -> Result<Option<BlpMipChain>> {
        let Some(base_path) = self.skin_texture(race, sex, skin) else {
            return Ok(None);
        };
        let mut atlas = read_texture_mip_chain(chain, base_path)
            .with_context(|| format!("reading base skin '{base_path}'"))?;
        // The head overlays `(sectionType, variation, color, column, tile)` of `0x4782e0`, in order
        // face, facial hair, hair; face and facial hair use columns 0/1, hair 1/2.
        let overlays: [(u8, u8, u8, usize, Tile); 6] = [
            (SECTION_FACE, face, skin, 0, TILE_G9),
            (SECTION_FACE, face, skin, 1, TILE_G8),
            (SECTION_FACIAL_HAIR, facial_hair, hair_color, 0, TILE_G9),
            (SECTION_FACIAL_HAIR, facial_hair, hair_color, 1, TILE_G8),
            (SECTION_HAIR, hair_style, hair_color, 1, TILE_G9),
            (SECTION_HAIR, hair_style, hair_color, 2, TILE_G8),
        ];
        for (ty, var, color, col, tile) in overlays {
            let Some(path) = self.tex(race, sex, ty, var, color, col) else {
                continue;
            };
            if let Ok(overlay) = read_texture_mip_chain(chain, path) {
                blit_over(&mut atlas, &overlay, tile);
            }
        }
        // One plan gates the underwear and drives the equipment blits.
        let plan = equip_blits(&equipment, emblem, tabard_preview);
        // The underwear, skipped when a tested cell is taken (`UNDERWEAR_TILES`); drawn first, so
        // the untested cells (belt, tabard) stack on top.
        for (col, layer, tested) in UNDERWEAR_TILES {
            if plan.iter().any(|s| s.layer == layer && s.column < tested) {
                continue;
            }
            let Some(path) = self.tex(race, sex, SECTION_UNDERWEAR, 0, skin, col) else {
                continue;
            };
            if let Ok(overlay) = read_texture_mip_chain(chain, path) {
                blit_over(&mut atlas, &overlay, EQUIP_TILES[layer]);
            }
        }
        // The equipment and emblem layers in plan order, each from its first decodable candidate.
        for step in &plan {
            if let Some(overlay) = step
                .candidates(sex)
                .iter()
                .find_map(|path| read_texture_mip_chain(chain, path).ok())
            {
                blit_over(&mut atlas, &overlay, EQUIP_TILES[step.layer]);
            }
        }
        Ok(Some(atlas))
    }

    /// Load CharSections.dbc from the patch chain.
    pub fn load(chain: &mut Chain) -> Result<Self> {
        let bytes = chain
            .read_file(CHAR_SECTIONS)
            .with_context(|| format!("reading {CHAR_SECTIONS}"))?;
        let rs = parse(&bytes, char_sections_schema(), "CharSections")?;
        let mut sections = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            // fields: ID, Race, Sex, SectionType, Variation, Color, Tex0, Tex1, Tex2, Flags.
            if let (Some(race), Some(sex), Some(ty), Some(var), Some(color)) = (
                u32_at(r, 1),
                u32_at(r, 2),
                u32_at(r, 3),
                u32_at(r, 4),
                u32_at(r, 5),
            ) {
                let texs = [
                    str_at(&rs, r, 6).unwrap_or_default(),
                    str_at(&rs, r, 7).unwrap_or_default(),
                    str_at(&rs, r, 8).unwrap_or_default(),
                ];
                let key = (race as u8, sex as u8, ty as u8, var as u8, color as u8);
                // A row flagged `0x1` can share its key with a standard row (Human male skin 0 has
                // `…Skin00_00` and `…_100`); the standard row wins in either order.
                let extra = u32_at(r, 9).unwrap_or(0) & 0x1 != 0;
                if extra {
                    sections.entry(key).or_insert(texs);
                } else {
                    sections.insert(key, texs);
                }
            }
        }
        Ok(Self { sections })
    }
}

/// One step of the equipment plan [`CharSections::composite_body`] runs, public so
/// `benilla-extract charatlas` reports the same plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EquipBlit<'a> {
    /// The compositor layer: ItemDisplayInfo texture column i, atlas tile gi.
    pub layer: usize,
    /// The cell in the layer's row ([`equip_column`], or the emblem's 2/3/4); higher blits later.
    pub column: i8,
    /// Where the art comes from.
    pub source: BlitSource<'a>,
}

/// What an [`EquipBlit`] paints: a garment's `Item\TextureComponents\<dir>\<name>_<U|M|F>.blp`,
/// or a guild-tabard layer's `Textures\GuildEmblems\<…>_<TU|TL>_U.blp`, which is never gendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlitSource<'a> {
    /// A worn garment: its slot (bodyslot − 2) and its bare region-texture name for this layer.
    Worn { slot: usize, texture: &'a str },
    /// One of the wearer's guild-tabard layers.
    Emblem {
        part: EmblemLayer,
        emblem: GuildEmblem,
    },
}

impl EquipBlit<'_> {
    /// The chain paths to try, first hit wins: `_U` then the gender letter for a garment
    /// (`0x476e20`), the one ungendered name for an emblem layer.
    pub fn candidates(&self, sex: u8) -> Vec<String> {
        match self.source {
            BlitSource::Worn { texture, .. } => {
                equip_region_candidates(self.layer, texture, sex).into()
            }
            BlitSource::Emblem { part, emblem } => emblem_half(self.layer)
                .map(|half| vec![part.path(&emblem, half)])
                .unwrap_or_default(),
        }
    }
}

/// A guild's tabard: the five `SMSG_GUILD_QUERY_RESPONSE` indices, each naming a BLP under
/// `Textures\GuildEmblems\` (the designer's counts `[0x808220]` = {170, 17, 6, 17, 51}); an index
/// with no file paints nothing. Signed, never clamped: a guild with no tabard sends `-1` in all
/// five (vmangos `Guild/Guild.cpp:86`, `int32`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GuildEmblem {
    /// The symbol, `Emblem_<style>_<color>_…`.
    pub emblem_style: i32,
    /// The symbol's colour.
    pub emblem_color: i32,
    /// The border, `Border_<style>_<color>_…`; the designer never offers the shipped styles 6–9.
    pub border_style: i32,
    /// The border's colour.
    pub border_color: i32,
    /// The background; 29 ships as `BACKGROUND_29_TU_U.blp`, so the lookup must ignore case.
    pub background_color: i32,
}

impl GuildEmblem {
    /// Whether none of the five is `-1`, the client's guard `0x6d6d20` (`0x6d6d73`–`0x6d6d93`).
    /// Otherwise `0x47a610` is never entered, so its clear of cells 4→2 (`0x47a616 push 4;
    /// 0x47a61a call 0x47a5c0`) never runs and the tabard keeps its own art.
    pub fn is_designed(&self) -> bool {
        [
            self.emblem_style,
            self.emblem_color,
            self.border_style,
            self.border_color,
            self.background_color,
        ]
        .iter()
        .all(|&i| i != NO_TABARD)
    }
}

/// The no-tabard sentinel a new guild carries (`Guild/Guild.cpp:86`), tested by `0x6d6d20`.
const NO_TABARD: i32 = -1;

/// A guild tabard's three layers, in the blit order of the cells `0x47a610` refills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmblemLayer {
    /// Cell 2, the field colour: inside TorsoUpper's tested cells, so a guild tabard hides the bra,
    /// and its cutout shows the base skin (`0x477070`) at the sides.
    Background,
    /// Cell 3, the trim around the field.
    Border,
    /// Cell 4, the symbol, in the cell the tabard's own art would take.
    Symbol,
}

impl EmblemLayer {
    /// The three layers in blit order.
    pub const ALL: [EmblemLayer; 3] = [
        EmblemLayer::Background,
        EmblemLayer::Border,
        EmblemLayer::Symbol,
    ];

    /// The layer's fixed cell in its row (`0x47a610`).
    pub fn column(self) -> i8 {
        match self {
            EmblemLayer::Background => 2,
            EmblemLayer::Border => 3,
            EmblemLayer::Symbol => 4,
        }
    }

    /// This layer's chain path for one tabard half (`"TU"`/`"TL"`): the client's `%02d` format
    /// (`0x838dd8`) plus the `.blp` its loader appends (`0x449641`); `%02d` pads, never truncates.
    pub fn path(self, emblem: &GuildEmblem, half: &str) -> String {
        match self {
            EmblemLayer::Background => format!(
                "{GUILD_EMBLEM_DIR}\\Background_{:02}_{half}_U.blp",
                emblem.background_color
            ),
            EmblemLayer::Border => format!(
                "{GUILD_EMBLEM_DIR}\\Border_{:02}_{:02}_{half}_U.blp",
                emblem.border_style, emblem.border_color
            ),
            EmblemLayer::Symbol => format!(
                "{GUILD_EMBLEM_DIR}\\Emblem_{:02}_{:02}_{half}_U.blp",
                emblem.emblem_style, emblem.emblem_color
            ),
        }
    }
}

/// The cell a worn slot takes in `layer`'s row: the `[0x803bf8]` default, unless the ArmLower
/// (`0x479210`) or LegLower (`0x4793f0`) chooser overrules it from a `geosetGroup`.
pub fn equip_column(equipment: &[Option<&ItemDisplay>; 8], slot: usize, layer: usize) -> i8 {
    let group = |s: usize, j: usize| equipment[s].is_some_and(|d| d.geoset_groups[j] != 0);
    match (layer, slot) {
        // ArmLower (`0x479210`, via `0x4774f0(rec, 0)`): geoset gloves to 6, a sleeved chest to 5.
        (1, SLOT_GLOVES) if group(slot, 0) => 6,
        (1, SLOT_CHEST) if group(slot, 0) => 5,
        // LegLower (`0x4793f0`): a boot geoset lifts boots 2 → 3, the robe bit lifts a chest 1 → 4
        // over them, and robe-trousers take 3 under a robe chest, else 4.
        (6, SLOT_BOOTS) if group(slot, 0) => 3,
        (6, SLOT_CHEST) if group(slot, 2) => 4,
        (6, SLOT_PANTS) if group(slot, 2) => {
            if group(SLOT_CHEST, 2) {
                3
            } else {
                4
            }
        }
        _ => EQUIP_LAYER_COLUMN[slot][layer],
    }
}

/// The equipment plan: per layer, each worn slot with a table cell and a non-empty name
/// (`0x478ad0` tests the first byte) in its [`equip_column`] cell, by ascending cell. A cell holds
/// one record: the later slot wins here, the later-equipped item in the client (`0x478900`). The
/// only slots that meet in a cell are robe-trousers under a robe chest and geoset boots, at
/// LegLower's 3.
///
/// `emblem` installs over a flagged tabard or in the designer preview: `0x47a610` clears cells 4→2
/// of layers 3 and 4 (`0x47a616`, `0x47a61a`) and refills them, replacing the tabard's own art.
pub fn equip_blits<'a>(
    equipment: &[Option<&'a ItemDisplay>; 8],
    emblem: Option<GuildEmblem>,
    tabard_preview: bool,
) -> Vec<EquipBlit<'a>> {
    // In the designer preview `0x47a610` installs with no ItemDisplayInfo test, over an empty slot.
    let emblem = emblem.filter(|_| {
        tabard_preview
            || equipment[SLOT_TABARD].is_some_and(|d: &ItemDisplay| d.takes_guild_emblem())
    });
    let mut plan = Vec::new();
    for (layer, _tile) in EQUIP_TILES.iter().enumerate() {
        let mut row: [Option<EquipBlit<'a>>; 8] = [None; 8];
        for (slot, display) in equipment.iter().enumerate() {
            let Some(display) = display else { continue };
            if EQUIP_LAYER_COLUMN[slot][layer] < 0 {
                continue;
            }
            let Some(texture) = display.region_textures[layer]
                .as_deref()
                .filter(|name| !name.is_empty())
            else {
                continue;
            };
            let column = equip_column(equipment, slot, layer);
            if let Some(cell) = row.get_mut(column as usize) {
                *cell = Some(EquipBlit {
                    layer,
                    column,
                    source: BlitSource::Worn { slot, texture },
                });
            }
        }
        if let Some(emblem) = emblem.filter(|_| emblem_half(layer).is_some()) {
            for part in EmblemLayer::ALL {
                let column = part.column();
                if let Some(cell) = row.get_mut(column as usize) {
                    *cell = Some(EquipBlit {
                        layer,
                        column,
                        source: BlitSource::Emblem { part, emblem },
                    });
                }
            }
        }
        plan.extend(row.into_iter().flatten());
    }
    plan
}

const LAYER_ARM_LOWER: usize = 1;

/// Whether anything but the shirt dresses the forearm, ArmLower cells 1–6 (`cc+0x26c..cc+0x280`):
/// branch B3's gate on the shirt's cuff (`0x4775bd`). 533 of 637 shipped chests leave the tile
/// empty, 566 of 570 bracers fill it; the emblem never reaches this layer.
pub fn forearm_dressed(equipment: &[Option<&ItemDisplay>; 8]) -> bool {
    equip_blits(equipment, None, false)
        .iter()
        .any(|b| b.layer == LAYER_ARM_LOWER && (1..=6).contains(&b.column))
}

/// A layer's atlas rect `(x, y, w, h)` in the 256² body atlas (`0x475c50`).
pub fn equip_tile(layer: usize) -> Option<(u32, u32, u32, u32)> {
    EQUIP_TILES.get(layer).copied()
}

/// A layer's `Item\TextureComponents\` subdirectory.
pub fn equip_tex_dir(layer: usize) -> Option<&'static str> {
    EQUIP_TEX_DIRS.get(layer).copied()
}

/// A region texture's chain paths in the order the composite tries them: `_U`, then the gender
/// letter. `0x476e20` probes the `_U` path (`0x648a10`) and patches in the gender letter only on a
/// miss, so `_U` wins on the 43 shipped basenames that have both.
pub fn equip_region_candidates(layer: usize, name: &str, sex: u8) -> [String; 2] {
    let dir = EQUIP_TEX_DIRS[layer];
    let letter = if sex == 1 { 'F' } else { 'M' };
    ['U', letter].map(|c| format!("Item\\TextureComponents\\{dir}\\{name}_{c}.blp"))
}

/// Source-over blit of an overlay's mip pyramid at `tile`, level by level, from the overlay's own
/// origin (`0x4770f0`: src `(0,0)`, extent the tile); an opaque texel copies, the client's REPLACE.
fn blit_over(dst: &mut BlpMipChain, src: &BlpMipChain, tile: Tile) {
    // Both chains must be decoded RGBA: DXT blocks would blend into garbage without failing.
    debug_assert!(
        dst.is_rgba8() && src.is_rgba8(),
        "character-skin compositing needs decoded chains on both sides"
    );
    let (tx, ty, tw, th) = tile;
    let levels = dst.mips.len().min(src.mips.len());
    for i in 0..levels {
        let dw = (dst.width >> i).max(1) as usize;
        let dh = (dst.height >> i).max(1) as usize;
        let sw = (src.width >> i).max(1) as usize;
        let sh = (src.height >> i).max(1) as usize;
        let (ox, oy) = ((tx >> i) as usize, (ty >> i) as usize);
        let cw = ((tw >> i).max(1) as usize)
            .min(sw)
            .min(dw.saturating_sub(ox));
        let ch = ((th >> i).max(1) as usize)
            .min(sh)
            .min(dh.saturating_sub(oy));
        let (d, s) = (&mut dst.mips[i], &src.mips[i]);
        for row in 0..ch {
            for col in 0..cw {
                let si = (row * sw + col) * 4;
                let di = ((oy + row) * dw + (ox + col)) * 4;
                if si + 4 > s.len() || di + 4 > d.len() {
                    continue;
                }
                let a = s[si + 3] as u32;
                if a == 0 {
                    continue;
                }
                if a == 255 {
                    d[di..di + 4].copy_from_slice(&s[si..si + 4]);
                    continue;
                }
                let ia = 255 - a;
                for c in 0..3 {
                    d[di + c] = ((s[si + c] as u32 * a + d[di + c] as u32 * ia + 127) / 255) as u8;
                }
                d[di + 3] = (a + (d[di + 3] as u32 * ia + 127) / 255).min(255) as u8;
            }
        }
    }
}

/// CharSections.dbc: 10 fields, three of them texture names.
pub(crate) fn char_sections_schema() -> Schema {
    let mut s = Schema::new("CharSections");
    for (name, ty) in [
        ("ID", FieldType::UInt32),
        ("RaceID", FieldType::UInt32),
        ("SexID", FieldType::UInt32),
        ("SectionType", FieldType::UInt32),
        ("VariationIndex", FieldType::UInt32),
        ("ColorIndex", FieldType::UInt32),
        ("TextureName0", FieldType::String),
        ("TextureName1", FieldType::String),
        ("TextureName2", FieldType::String),
        ("Flags", FieldType::UInt32),
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    s
}

#[cfg(test)]
mod tests {
    /// The `(cell, texture)` of a worn contribution; these fixtures pass no emblem.
    fn worn_cell<'a>(step: &EquipBlit<'a>) -> (i8, &'a str) {
        match step.source {
            BlitSource::Worn { texture, .. } => (step.column, texture),
            BlitSource::Emblem { .. } => panic!("unexpected guild-emblem layer in this fixture"),
        }
    }
    use super::*;

    /// The value is the shipped Human male skinColor 3 row.
    #[test]
    fn base_skin_texture_resolves() {
        let mut sections = HashMap::new();
        sections.insert(
            (1, 0, SECTION_SKIN, 0, 3),
            [
                "Character\\Human\\Male\\HumanMaleSkin00_03.blp".to_string(),
                String::new(),
                String::new(),
            ],
        );
        let cs = CharSections { sections };
        assert_eq!(
            cs.skin_texture(1, 0, 3),
            Some("Character\\Human\\Male\\HumanMaleSkin00_03.blp")
        );
        assert_eq!(cs.skin_texture(1, 0, 99), None, "absent color → no texture");
    }

    fn worn(regions: [Option<&str>; 8], geoset_groups: [u32; 3]) -> ItemDisplay {
        ItemDisplay {
            region_textures: regions.map(|r| r.map(str::to_string)),
            geoset_groups,
            ..Default::default()
        }
    }

    /// Region textures for LegUpper and a named LegLower.
    fn leg(tex: &str) -> [Option<&str>; 8] {
        [None, None, None, None, None, Some("lu"), Some(tex), None]
    }

    /// The `[0x803bf8]` cell is a default the ArmLower chooser (`0x479210`) overrules.
    #[test]
    fn equip_blits_places_each_slot_in_its_chooser_cell() {
        let plain_chest = worn(
            [
                Some("au"),
                Some("al"),
                None,
                Some("tu"),
                None,
                None,
                None,
                None,
            ],
            [0, 0, 0],
        );
        let sleeved_chest = worn(
            [
                Some("au"),
                Some("al"),
                None,
                Some("tu"),
                None,
                None,
                None,
                None,
            ],
            [1, 0, 0],
        );
        let bracer = worn(
            [None, Some("bracer_al"), None, None, None, None, None, None],
            [0, 0, 0],
        );
        let plain_gloves = worn(
            [
                None,
                Some("glove_al"),
                Some("glove_ha"),
                None,
                None,
                None,
                None,
                None,
            ],
            [0, 0, 0],
        );
        let geoset_gloves = worn(
            [
                None,
                Some("glove_al"),
                Some("glove_ha"),
                None,
                None,
                None,
                None,
                None,
            ],
            [1, 0, 0],
        );

        // Plain items take the table's cells; geoset gloves go to 6, a sleeved chest to 5.
        let eq = [
            None,
            Some(&plain_chest),
            None,
            None,
            None,
            Some(&bracer),
            Some(&plain_gloves),
            None,
        ];
        let g1: Vec<_> = equip_blits(&eq, None, false)
            .iter()
            .filter(|s| s.layer == 1)
            .map(worn_cell)
            .collect();
        assert_eq!(g1, [(1, "al"), (2, "bracer_al"), (3, "glove_al")]);
        let eq = [
            None,
            Some(&sleeved_chest),
            None,
            None,
            None,
            Some(&bracer),
            Some(&geoset_gloves),
            None,
        ];
        let g1: Vec<_> = equip_blits(&eq, None, false)
            .iter()
            .filter(|s| s.layer == 1)
            .map(worn_cell)
            .collect();
        assert_eq!(
            g1,
            [(2, "bracer_al"), (5, "al"), (6, "glove_al")],
            "a sleeved chest paints over a bracer"
        );
    }

    /// The emblem installs only over a flagged tabard, taking cells 2/3/4 of layers 3 and 4.
    #[test]
    fn the_guild_emblem_replaces_the_tabard_it_is_worn_on() {
        let torso = [
            None,
            None,
            None,
            Some("tabard_tu"),
            Some("tabard_tl"),
            None,
            None,
            None,
        ];
        let guild_tabard = ItemDisplay {
            flags: 1,
            ..worn(torso, [1, 0, 0])
        };
        let plain_tabard = worn(torso, [1, 0, 0]);
        let shirt = worn(
            [
                Some("shirt_au"),
                Some("shirt_al"),
                None,
                Some("shirt_tu"),
                Some("shirt_tl"),
                None,
                None,
                None,
            ],
            [0, 0, 0],
        );
        let emblem = GuildEmblem {
            emblem_style: 42,
            emblem_color: 3,
            border_style: 1,
            border_color: 7,
            background_color: 12,
        };
        // The `(layer, cell)` pairs a plan fills, and what filled each.
        fn cells(plan: &[EquipBlit<'_>]) -> Vec<(usize, i8, String)> {
            plan.iter()
                .map(|s| {
                    let what = match s.source {
                        BlitSource::Worn { texture, .. } => texture.to_string(),
                        BlitSource::Emblem { part, .. } => format!("{part:?}"),
                    };
                    (s.layer, s.column, what)
                })
                .collect()
        }
        let want = |v: &[(usize, i8, &str)]| -> Vec<(usize, i8, String)> {
            v.iter().map(|(l, c, t)| (*l, *c, t.to_string())).collect()
        };

        let eq = [
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&guild_tabard),
        ];
        assert_eq!(
            cells(&equip_blits(&eq, Some(emblem), false)),
            want(&[
                (3, 2, "Background"),
                (3, 3, "Border"),
                (3, 4, "Symbol"),
                (4, 2, "Background"),
                (4, 3, "Border"),
                (4, 4, "Symbol"),
            ]),
            "the emblem owns cells 2/3/4 of the two torso layers, the tabard's own art none"
        );
        // No guild, or no `SMSG_GUILD_QUERY_RESPONSE` yet: the garment keeps its art.
        assert_eq!(
            cells(&equip_blits(&eq, None, false)),
            want(&[(3, 4, "tabard_tu"), (4, 4, "tabard_tl")])
        );
        // An unflagged tabard keeps its art for a guilded wearer.
        let plain = [
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&plain_tabard),
        ];
        assert_eq!(
            cells(&equip_blits(&plain, Some(emblem), false)),
            want(&[(3, 4, "tabard_tu"), (4, 4, "tabard_tl")])
        );
        assert!(equip_blits(&[None; 8], Some(emblem), false).is_empty());

        let eq = [
            Some(&shirt),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&guild_tabard),
        ];
        assert_eq!(
            cells(&equip_blits(&eq, Some(emblem), false)),
            want(&[
                (0, 0, "shirt_au"),
                (1, 0, "shirt_al"),
                (3, 0, "shirt_tu"),
                (3, 2, "Background"),
                (3, 3, "Border"),
                (3, 4, "Symbol"),
                (4, 0, "shirt_tl"),
                (4, 2, "Background"),
                (4, 3, "Border"),
                (4, 4, "Symbol"),
            ])
        );
    }

    /// The client's `%02d` names: padded, a three-digit style kept whole, and `-1` kept signed.
    #[test]
    fn emblem_layer_paths_are_the_clients_own_format() {
        let e = GuildEmblem {
            emblem_style: 7,
            emblem_color: 3,
            border_style: 1,
            border_color: 12,
            background_color: 5,
        };
        assert_eq!(
            EmblemLayer::ALL.map(|p| p.path(&e, "TU")),
            [
                "Textures\\GuildEmblems\\Background_05_TU_U.blp",
                "Textures\\GuildEmblems\\Border_01_12_TU_U.blp",
                "Textures\\GuildEmblems\\Emblem_07_03_TU_U.blp",
            ]
        );
        assert_eq!(
            EmblemLayer::Symbol.path(&e, "TL"),
            "Textures\\GuildEmblems\\Emblem_07_03_TL_U.blp",
            "the TorsoLower half is the same name with the other tag"
        );
        assert_eq!(
            EmblemLayer::Symbol.path(
                &GuildEmblem {
                    emblem_style: 169,
                    emblem_color: 16,
                    ..e
                },
                "TU"
            ),
            "Textures\\GuildEmblems\\Emblem_169_16_TU_U.blp"
        );
        let none = GuildEmblem {
            emblem_style: -1,
            emblem_color: -1,
            border_style: -1,
            border_color: -1,
            background_color: -1,
        };
        assert_eq!(
            EmblemLayer::ALL.map(|p| p.path(&none, "TU")),
            [
                "Textures\\GuildEmblems\\Background_-1_TU_U.blp",
                "Textures\\GuildEmblems\\Border_-1_-1_TU_U.blp",
                "Textures\\GuildEmblems\\Emblem_-1_-1_TU_U.blp",
            ],
            "the sentinel renders as the client's own `%02d` does, and names nothing"
        );
    }

    /// The `-1` guard (`0x6d6d20`): any one `-1` of the five means no crest.
    #[test]
    fn an_undesigned_guild_has_no_crest_to_paint() {
        let designed = GuildEmblem {
            emblem_style: 42,
            emblem_color: 3,
            border_style: 1,
            border_color: 7,
            background_color: 12,
        };
        assert!(designed.is_designed());
        // Index 0 ships in every field.
        assert!(GuildEmblem::default().is_designed());
        assert!(!GuildEmblem {
            emblem_style: -1,
            emblem_color: -1,
            border_style: -1,
            border_color: -1,
            background_color: -1,
        }
        .is_designed());
        for spoil in [
            GuildEmblem {
                emblem_style: -1,
                ..designed
            },
            GuildEmblem {
                emblem_color: -1,
                ..designed
            },
            GuildEmblem {
                border_style: -1,
                ..designed
            },
            GuildEmblem {
                border_color: -1,
                ..designed
            },
            GuildEmblem {
                background_color: -1,
                ..designed
            },
        ] {
            assert!(!spoil.is_designed(), "{spoil:?} still reads as designed");
        }
    }

    /// A guild tabard's background (cell 2) hides the bra, a plain tabard's cell 4 does not.
    #[test]
    fn only_a_guild_tabards_background_reaches_the_underwear_prefix() {
        let torso = [
            None,
            None,
            None,
            Some("tabard_tu"),
            Some("tabard_tl"),
            None,
            None,
            None,
        ];
        let guild_tabard = ItemDisplay {
            flags: 1,
            ..worn(torso, [1, 0, 0])
        };
        let plain_tabard = worn(torso, [1, 0, 0]);
        let emblem = GuildEmblem::default();
        let suppresses = |eq: &[Option<&ItemDisplay>; 8], em: Option<GuildEmblem>| {
            let (col, layer, tested) = UNDERWEAR_TILES[1];
            assert_eq!(
                (col, layer),
                (1, 3),
                "the bra is TextureName[1] into TorsoUpper"
            );
            equip_blits(eq, em, false)
                .iter()
                .any(|s| s.layer == layer && s.column < tested)
        };
        let guild = [
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&guild_tabard),
        ];
        let plain = [
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&plain_tabard),
        ];
        assert!(
            suppresses(&guild, Some(emblem)),
            "a guild tabard's background sits in cell 2, inside the tested prefix"
        );
        assert!(
            !suppresses(&guild, None),
            "with no guild the same tabard only reaches cell 4, past the prefix"
        );
        assert!(
            !suppresses(&plain, Some(emblem)),
            "a plain tabard never hides the bra"
        );
    }

    /// LegLower (`0x4793f0`): a robe takes cell 4 over boots, while a plain chest stays under them.
    #[test]
    fn a_robe_outranks_footwear_on_leglower() {
        let robe = worn(leg("robe_ll"), [1, 0, 1]);
        let plain_chest = worn(leg("chest_ll"), [0, 0, 0]);
        let trousers = worn(leg("pant_ll"), [0, 0, 0]);
        let robe_trousers = worn(leg("robetrouser_ll"), [0, 0, 1]);
        let shoes = worn(
            [
                None,
                None,
                None,
                None,
                None,
                None,
                Some("shoe_ll"),
                Some("shoe_fo"),
            ],
            [0, 0, 0],
        );
        let boots = worn(
            [
                None,
                None,
                None,
                None,
                None,
                None,
                Some("boot_ll"),
                Some("boot_fo"),
            ],
            [3, 0, 0],
        );

        fn g6(eq: &[Option<&ItemDisplay>; 8]) -> Vec<(i8, String)> {
            equip_blits(eq, None, false)
                .iter()
                .filter(|s| s.layer == 6)
                .map(|s| {
                    let (c, t) = worn_cell(s);
                    (c, t.to_string())
                })
                .collect()
        }
        fn cells(want: &[(i8, &str)]) -> Vec<(i8, String)> {
            want.iter().map(|(c, t)| (*c, t.to_string())).collect()
        }

        // A robe over sandals and over geoset boots: the robe is last either way.
        let eq = [
            None,
            Some(&robe),
            None,
            None,
            Some(&shoes),
            None,
            None,
            None,
        ];
        assert_eq!(
            g6(&eq),
            cells(&[(2, "shoe_ll"), (4, "robe_ll")]),
            "sandals under the robe"
        );
        let eq = [
            None,
            Some(&robe),
            None,
            None,
            Some(&boots),
            None,
            None,
            None,
        ];
        assert_eq!(
            g6(&eq),
            cells(&[(3, "boot_ll"), (4, "robe_ll")]),
            "boots under the robe too"
        );

        // A plain chest is not lifted, so boots still cover trousers.
        let eq = [
            None,
            Some(&plain_chest),
            None,
            Some(&trousers),
            Some(&boots),
            None,
            None,
            None,
        ];
        assert_eq!(
            g6(&eq),
            cells(&[(0, "pant_ll"), (1, "chest_ll"), (3, "boot_ll")]),
            "footwear still paints over trousers"
        );

        // Robe-trousers take 3 under a robe chest, else 4.
        let eq = [
            None,
            Some(&robe),
            None,
            Some(&robe_trousers),
            None,
            None,
            None,
            None,
        ];
        assert_eq!(g6(&eq), cells(&[(3, "robetrouser_ll"), (4, "robe_ll")]));
        let eq = [
            None,
            Some(&plain_chest),
            None,
            Some(&robe_trousers),
            None,
            None,
            None,
            None,
        ];
        assert_eq!(g6(&eq), cells(&[(1, "chest_ll"), (4, "robetrouser_ll")]));
    }

    /// `0x478ad0`'s gates: a `-1` cell and an empty name contribute nothing.
    #[test]
    fn equip_blits_drops_ungated_and_empty_contributions() {
        let odd = worn(
            [None, None, None, Some("boot_tu"), None, None, None, None],
            [0, 0, 0],
        );
        let eq = [None, None, None, None, Some(&odd), None, None, None];
        assert!(
            equip_blits(&eq, None, false).is_empty(),
            "a -1 cell drops the contribution entirely"
        );

        let blank = worn(
            [None, None, None, Some(""), None, None, None, None],
            [0, 0, 0],
        );
        let eq = [None, Some(&blank), None, None, None, None, None, None];
        assert!(
            equip_blits(&eq, None, false).is_empty(),
            "an empty texture name is not a contribution"
        );
    }

    /// `_U` comes before the gender letter (`0x476e20`); `Leather_A_02_Pant_LL` ships both.
    #[test]
    fn region_textures_resolve_unisex_before_the_gender_letter() {
        let female = equip_region_candidates(6, "Leather_A_02_Pant_LL", 1);
        assert_eq!(
            female,
            [
                "Item\\TextureComponents\\LegLowerTexture\\Leather_A_02_Pant_LL_U.blp",
                "Item\\TextureComponents\\LegLowerTexture\\Leather_A_02_Pant_LL_F.blp",
            ]
        );
        let male = equip_region_candidates(6, "Leather_A_02_Pant_LL", 0);
        assert!(male[0].ends_with("_U.blp") && male[1].ends_with("_M.blp"));
        // No third attempt: a bare name never resolves.
        assert_eq!(male.len(), 2);
    }

    fn chain(width: u32, height: u32, px: Vec<u8>) -> BlpMipChain {
        BlpMipChain {
            width,
            height,
            texels: crate::BlpTexels::Rgba8Unorm,
            mips: vec![px],
        }
    }

    #[test]
    fn blit_over_replaces_blends_and_clamps() {
        let mut dst = chain(2, 2, [128, 128, 128, 255].repeat(4));

        blit_over(&mut dst, &chain(1, 1, vec![255, 0, 0, 255]), (0, 0, 1, 1));
        assert_eq!(
            &dst.mips[0][0..4],
            &[255, 0, 0, 255],
            "opaque overlay replaces"
        );
        assert_eq!(
            &dst.mips[0][4..8],
            &[128, 128, 128, 255],
            "neighbour untouched"
        );

        // Half-alpha blue over grey: R≈64, B≈191.
        blit_over(&mut dst, &chain(1, 1, vec![0, 0, 255, 128]), (1, 1, 1, 1));
        let px = &dst.mips[0][12..16];
        assert!((px[0] as i32 - 64).abs() <= 1, "blended R ≈ 128·(1−a)");
        assert!(
            (px[2] as i32 - 191).abs() <= 1,
            "blended B ≈ 255·a + 128·(1−a)"
        );
        assert_eq!(px[3], 255, "over an opaque base the result stays opaque");

        let before = dst.mips[0].clone();
        blit_over(&mut dst, &chain(1, 1, vec![1, 2, 3, 0]), (0, 0, 1, 1));
        assert_eq!(dst.mips[0], before, "transparent texel is a no-op");
    }

    /// On the shipped files a Human male's head and pelvis tiles change, and the torso does not.
    #[test]
    fn composite_body_overlays_land_on_real_human_male() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cs = CharSections::load(&mut chain).expect("load CharSections");
        let base =
            read_texture_mip_chain(&mut chain, "Character\\Human\\Male\\HumanMaleSkin00_03.blp")
                .expect("read base skin");
        let comp = cs
            .composite_body(&mut chain, 1, 0, 3, 0, 1, 0, 0, [None; 8], None, false)
            .expect("composite ok")
            .expect("base skin row present");

        assert_eq!((comp.width, comp.height), (256, 256), "atlas is 256²");
        assert_eq!(comp.mips.len(), base.mips.len(), "keeps the base mip count");

        // Count mip-0 pixels in a tile that differ from the base atlas.
        let changed = |t: Tile| {
            let (x, y, w, h) = t;
            let (b, c) = (&base.mips[0], &comp.mips[0]);
            (y..y + h)
                .flat_map(|row| (x..x + w).map(move |col| (row, col)))
                .filter(|&(row, col)| {
                    let i = ((row * 256 + col) * 4) as usize;
                    b[i..i + 4] != c[i..i + 4]
                })
                .count()
        };
        assert!(changed(TILE_G9) > 4000, "face lower overlaid into g9");
        assert!(changed(TILE_G8) > 2000, "face upper overlaid into g8");
        assert!(changed(TILE_G5) > 4000, "pelvis overlaid into g5");
        // No male underwear row authors `TextureName[1]`, the torso.
        assert_eq!(
            changed(TILE_G3),
            0,
            "no male underwear row authors the naked-torso column"
        );

        let hair = cs
            .hair_texture(1, 0, 1, 0)
            .expect("hairStyle 1 has a hair texture");
        assert!(
            hair.contains("Hair"),
            "hair texture is a Hair BLP, got {hair:?}"
        );
        assert_eq!(
            cs.hair_texture(1, 0, 0, 0),
            None,
            "bald style has no hair texture"
        );

        // A bald orc or gnome male's beard is hair geometry, so bald still resolves a sheet.
        for (race, sex, name) in [(2u8, 0u8, "Orc"), (7, 0, "Gnome")] {
            let styled = cs
                .hair_mesh_texture(race, sex, 1, 0)
                .unwrap_or_else(|| panic!("{name} hairStyle 1 has a hair sheet"));
            let bald = cs
                .hair_mesh_texture(race, sex, 0, 0)
                .unwrap_or_else(|| panic!("{name} bald must still resolve a sheet for the beard"));
            assert_eq!(
                bald, styled,
                "{name} carries the style in the geometry — one sheet per colour"
            );
            assert!(
                bald.contains("Hair00_00"),
                "{name} sheet is Hair00_00, got {bald:?}"
            );
            // The fallback keeps the hair colour.
            let bald_c2 = cs
                .hair_mesh_texture(race, sex, 0, 2)
                .unwrap_or_else(|| panic!("{name} bald at colour 2 resolves"));
            assert!(
                bald_c2.contains("Hair00_02"),
                "{name} colour 2 sheet, got {bald_c2:?}"
            );
            assert_ne!(bald, bald_c2, "{name} colour must change the sheet");
        }

        // Human authors two sheets per colour, so the literal variation 1 matters.
        let human_bald = cs
            .hair_mesh_texture(1, 0, 0, 0)
            .expect("the substitute resolves variation 1 even where a race authors several sheets");
        assert_eq!(
            Some(human_bald),
            cs.hair_texture(1, 0, HAIR_SUBSTITUTE_VARIATION, 0),
            "the substitute is variation 1, not an arbitrary non-blank row"
        );
        assert!(
            human_bald.contains("Hair03_00"),
            "human male variation 1 is the Hair03 sheet, got {human_bald:?}"
        );
        assert_eq!(
            cs.hair_mesh_texture(1, 0, 1, 0),
            cs.hair_texture(1, 0, 1, 0)
        );

        // The extra skin, M2 type 8: tauren author it, humans do not.
        assert_eq!(
            cs.skin_extra_texture(6, 0, 0),
            Some("Character\\Tauren\\Male\\TaurenMaleSkin00_00_Extra.blp"),
            "tauren male extra skin resolves"
        );
        assert_eq!(
            cs.skin_extra_texture(1, 0, 0),
            None,
            "human male authors no extra skin"
        );

        // Skin colours 0 and 1 resolve the standard row, not the `0x1`-flagged `…_100`/`_101`.
        assert_eq!(
            cs.skin_texture(1, 0, 0),
            Some("Character\\Human\\Male\\HumanMaleSkin00_00.blp"),
            "skinColor 0 is the standard skin, not the EXTRA-flagged _100"
        );
        assert_eq!(
            cs.skin_texture(1, 0, 1),
            Some("Character\\Human\\Male\\HumanMaleSkin00_01.blp")
        );
    }

    /// The underwear's `TextureName[1]` is the female torso (g3), an opaque tile-sized sheet, so g3
    /// must equal it byte for byte.
    #[test]
    fn composite_body_dresses_the_female_torso_underwear() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cs = CharSections::load(&mut chain).expect("load CharSections");
        // Night elf female, skin colour 0.
        let (race, sex, skin) = (4u8, 1u8, 0u8);
        assert_eq!(
            cs.tex(race, sex, SECTION_UNDERWEAR, 0, skin, 1),
            Some("Character\\NightElf\\Female\\NightElfFemaleNakedTorsoSkin00_00.blp"),
            "the sectionType-4 row's second column is the naked torso"
        );
        let torso = read_texture_mip_chain(
            &mut chain,
            "Character\\NightElf\\Female\\NightElfFemaleNakedTorsoSkin00_00.blp",
        )
        .expect("read naked torso");
        assert_eq!(
            (torso.width, torso.height),
            (128, 64),
            "the sheet is authored exactly tile-sized"
        );
        let comp = cs
            .composite_body(
                &mut chain, race, sex, skin, 0, 0, 0, 0, [None; 8], None, false,
            )
            .expect("composite ok")
            .expect("base skin row present");

        let (tx, ty, tw, th) = TILE_G3;
        for row in 0..th {
            let d = ((ty + row) * 256 + tx) as usize * 4;
            let s = (row * tw) as usize * 4;
            assert_eq!(
                &comp.mips[0][d..d + tw as usize * 4],
                &torso.mips[0][s..s + tw as usize * 4],
                "g3 row {row} is the naked-torso sheet verbatim"
            );
        }
    }

    /// Only the tested cells suppress the underwear (`0x4772f0`, `0x4773a0`). Real art would paint
    /// over it either way, so the fixture's display takes a cell but names no shipped file.
    #[test]
    fn only_the_tested_equipment_columns_suppress_the_underwear() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cs = CharSections::load(&mut chain).expect("load CharSections");
        let (race, sex, skin) = (4u8, 1u8, 0u8); // night elf female, skinColor 0
        let dir = "Character\\NightElf\\Female\\NightElfFemale";
        let mut read = |n: &str| {
            read_texture_mip_chain(&mut chain, &format!("{dir}{n}00_00.blp")).expect("read sheet")
        };
        let (base, torso, pelvis) = (
            read("Skin"),
            read("NakedTorsoSkin"),
            read("NakedPelvisSkin"),
        );

        // The mip-0 pixels of a tile, out of a `stride`-wide image.
        let rect = |img: &BlpMipChain, (x, y, w, h): Tile, stride: u32| -> Vec<u8> {
            (0..h)
                .flat_map(|r| {
                    let o = (((y + r) * stride + x) * 4) as usize;
                    img.mips[0][o..o + (w * 4) as usize].to_vec()
                })
                .collect()
        };
        let sheet = |img: &BlpMipChain| rect(img, (0, 0, 128, 64), 128);
        let (torso_sheet, pelvis_sheet) = (sheet(&torso), sheet(&pelvis));
        let (base_g3, base_g5) = (rect(&base, TILE_G3, 256), rect(&base, TILE_G5, 256));

        // A display that takes the cell without painting it.
        let occupies = |layers: &[usize]| {
            let mut region_textures: [Option<String>; 8] = Default::default();
            for l in layers {
                region_textures[*l] = Some("benilla-no-such-region".into());
            }
            ItemDisplay {
                region_textures,
                ..Default::default()
            }
        };
        // Slots, bodyslot − 2: 0 shirt, 1 chest, 2 belt, 3 pants, 7 tabard.
        let (shirt, chest, belt, pants, tabard) = (
            occupies(&[3]),
            occupies(&[3, 5]), // a robe reaches both groups
            occupies(&[5]),
            occupies(&[5]),
            occupies(&[3]),
        );
        fn worn<'a>(slots: &[(usize, &'a ItemDisplay)]) -> [Option<&'a ItemDisplay>; 8] {
            let mut e: [Option<&ItemDisplay>; 8] = [None; 8];
            for (i, d) in slots {
                e[*i] = Some(d);
            }
            e
        }

        let differing = |got: &[u8], want: &[u8]| {
            got.as_chunks::<4>()
                .0
                .iter()
                .zip(want.as_chunks::<4>().0)
                .filter(|(a, b)| a != b)
                .count()
        };
        // (label, equipment, expected g3, expected g5)
        type Want<'a> = (&'a str, &'a Vec<u8>);
        let cases: [(&str, [Option<&ItemDisplay>; 8], Want, Want); 6] = [
            (
                "naked",
                worn(&[]),
                ("the bra", &torso_sheet),
                ("the panties", &pelvis_sheet),
            ),
            // The shirt is TorsoUpper cell 0, tested.
            (
                "shirt",
                worn(&[(0, &shirt)]),
                ("bare skin", &base_g3),
                ("the panties", &pelvis_sheet),
            ),
            // A tabard's cell 4 is past the tested cells.
            (
                "tabard",
                worn(&[(7, &tabard)]),
                ("the bra", &torso_sheet),
                ("the panties", &pelvis_sheet),
            ),
            // Pants are LegUpper cell 0, tested.
            (
                "pants",
                worn(&[(3, &pants)]),
                ("the bra", &torso_sheet),
                ("bare skin", &base_g5),
            ),
            // A belt is LegUpper cell 2, untested.
            (
                "belt",
                worn(&[(2, &belt)]),
                ("the bra", &torso_sheet),
                ("the panties", &pelvis_sheet),
            ),
            // A chest takes cell 1 of both.
            (
                "chest",
                worn(&[(1, &chest)]),
                ("bare skin", &base_g3),
                ("bare skin", &base_g5),
            ),
        ];
        for (label, equipment, (g3_want, g3), (g5_want, g5)) in cases {
            let comp = cs
                .composite_body(
                    &mut chain, race, sex, skin, 0, 0, 0, 0, equipment, None, false,
                )
                .expect("composite ok")
                .expect("base skin row present");
            for (tile, name, want_name, want) in [
                (TILE_G3, "torso", g3_want, g3),
                (TILE_G5, "pelvis", g5_want, g5),
            ] {
                let n = differing(&rect(&comp, tile, 256), want);
                assert_eq!(
                    n, 0,
                    "{label}: the {name} tile is not {want_name} ({n}/8192 texels)"
                );
            }
        }
        // The cases mean something only if the expectations differ.
        assert_ne!(torso_sheet, base_g3, "the bra differs from bare skin");
        assert_ne!(pelvis_sheet, base_g5, "the panties differ from bare skin");
    }

    /// On the shipped files display 20621 (item 5976, Guild Tabard) is flagged, its six emblem
    /// files are tile-sized (`0x4770f0`), and the emblem repaints only the torso tiles.
    #[test]
    fn composite_body_paints_the_guild_emblem_on_the_real_tabard() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cs = CharSections::load(&mut chain).expect("load CharSections");
        let items = crate::load_item_display_catalog(&mut chain).expect("load ItemDisplayInfo");
        let tabard = items.get(20621).expect("the Guild Tabard display");
        assert!(
            tabard.takes_guild_emblem(),
            "display 20621 is the guild-emblem tabard"
        );
        assert!(
            !items.get(9891).expect("shirt display").takes_guild_emblem(),
            "an ordinary garment is not"
        );

        let emblem = GuildEmblem {
            emblem_style: 42,
            emblem_color: 3,
            border_style: 1,
            border_color: 7,
            background_color: 12,
        };
        let equipment: [Option<&ItemDisplay>; 8] =
            [None, None, None, None, None, None, None, Some(tabard)];

        for step in equip_blits(&equipment, Some(emblem), false) {
            let path = step
                .candidates(0)
                .into_iter()
                .find(|p| read_texture_mip_chain(&mut chain, p).is_ok())
                .unwrap_or_else(|| panic!("{:?} resolves no file", step.source));
            let mips = read_texture_mip_chain(&mut chain, &path).expect("re-read");
            let (_, _, tw, th) = EQUIP_TILES[step.layer];
            assert_eq!(
                (mips.width, mips.height),
                (tw, th),
                "{path} is authored tile-sized"
            );
        }

        let mut compose = |em: Option<GuildEmblem>| {
            cs.composite_body(&mut chain, 1, 0, 3, 0, 1, 0, 0, equipment, em, false)
                .expect("composite ok")
                .expect("base skin row present")
        };
        let plain = compose(None);
        let crested = compose(Some(emblem));
        let in_torso = |i: usize| {
            let (x, y) = ((i % 256) as u32, (i / 256) as u32);
            [EQUIP_TILES[3], EQUIP_TILES[4]]
                .iter()
                .any(|(tx, ty, tw, th)| x >= *tx && x < tx + tw && y >= *ty && y < ty + th)
        };
        let moved: Vec<usize> = plain.mips[0]
            .as_chunks::<4>()
            .0
            .iter()
            .zip(crested.mips[0].as_chunks::<4>().0)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect();
        assert!(
            !moved.is_empty(),
            "the emblem must repaint something — a silent no-op is the bug this fixes"
        );
        let strays: Vec<usize> = moved.iter().copied().filter(|i| !in_torso(*i)).collect();
        assert!(
            strays.is_empty(),
            "the emblem repainted {} texel(s) outside the torso tiles, first at {:?}",
            strays.len(),
            strays.first().map(|i| (i % 256, i / 256))
        );

        let other = compose(Some(GuildEmblem {
            background_color: 30,
            ..emblem
        }));
        assert_ne!(
            crested.mips[0], other.mips[0],
            "the background index must reach the file name"
        );

        // Background 29 ships uppercase (`BACKGROUND_29_TU_U.blp`), so the lookup must ignore case.
        let odd = GuildEmblem {
            background_color: 29,
            ..emblem
        };
        for half in ["TU", "TL"] {
            let path = EmblemLayer::Background.path(&odd, half);
            assert!(
                read_texture_mip_chain(&mut chain, &path).is_ok(),
                "{path} must resolve despite the shipped name's casing"
            );
        }
    }

    /// The Human Warrior starter shirt, pants and boots (displays 9891, 9892, 10141) repaint their
    /// tiles, leave Hand alone, and the boots cover the pants on LegLower.
    #[test]
    fn composite_body_equipment_layers_land_on_real_human_male() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cs = CharSections::load(&mut chain).expect("load CharSections");
        let items = crate::load_item_display_catalog(&mut chain).expect("load ItemDisplayInfo");
        let (shirt, pants, boots) = (
            items.get(9891).expect("shirt display"),
            items.get(9892).expect("pants display"),
            items.get(10141).expect("boots display"),
        );
        let mut compose = |equipment: [Option<&ItemDisplay>; 8]| {
            cs.composite_body(&mut chain, 1, 0, 3, 0, 1, 0, 0, equipment, None, false)
                .expect("composite ok")
                .expect("base skin row present")
        };
        let naked = compose([None; 8]);
        // Indices are bodyslot − 2: shirt 0, pants 3, boots 4.
        let mut equipment: [Option<&ItemDisplay>; 8] = [None; 8];
        equipment[0] = Some(shirt);
        equipment[3] = Some(pants);
        let pants_only = compose(equipment);
        equipment[4] = Some(boots);
        let dressed = compose(equipment);

        let changed = |a: &BlpMipChain, b: &BlpMipChain, t: Tile| {
            let (x, y, w, h) = t;
            (y..y + h)
                .flat_map(|row| (x..x + w).map(move |col| (row, col)))
                .filter(|&(row, col)| {
                    let i = ((row * 256 + col) * 4) as usize;
                    a.mips[0][i..i + 4] != b.mips[0][i..i + 4]
                })
                .count()
        };
        assert!(
            changed(&naked, &dressed, EQUIP_TILES[3]) > 2000,
            "shirt repaints TorsoUpper (g3)"
        );
        assert!(
            changed(&naked, &dressed, EQUIP_TILES[5]) > 2000,
            "pants repaint LegUpper (g5)"
        );
        assert!(
            changed(&naked, &dressed, EQUIP_TILES[7]) > 1000,
            "boots repaint Foot (g7)"
        );
        assert_eq!(
            changed(&naked, &dressed, EQUIP_TILES[2]),
            0,
            "nothing touches Hand (g2)"
        );
        assert!(
            changed(&pants_only, &dressed, EQUIP_TILES[6]) > 1000,
            "boots stack over the pants' LegLower (g6)"
        );
    }
}
