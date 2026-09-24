//! Character-creation source data: the per-race body displayIds (ChrRaces), the
//! race/class combos (CharBaseInfo), and the five appearance-dial *ranges* per (race, sex) derived
//! from CharSections + CharHairGeosets + CharacterFacialHairStyles.
//!
//! **The ranges are data-derived, never hardcoded** (the client's CharSections grid, built at
//! `0x476020`). Each dial cycles index `0..count-1`; the counts come from:
//! - **skin color** = distinct `ColorIndex` of CharSections `SECTION_SKIN` (variation 0) for (race, sex);
//! - **face** = distinct `VariationIndex` of `SECTION_FACE`;
//! - **hair style** = distinct `VariationID` of **CharHairGeosets** for (race, sex), `max(1, …)` (the
//!   client's `0x478540` count fetcher — bald is style 0);
//! - **hair color** = distinct `ColorIndex` of `SECTION_HAIR`;
//! - **facial hair** = distinct `VariationID` of **CharacterFacialHairStyles** for (race, sex).
//!
//! By construction every `(skin, face, hairStyle, hairColor, facialHair)` tuple in the product of a
//! (race, sex)'s ranges passes vmangos's `Player::ValidateAppearance` — the server checks, in order,
//! that CharSections rows exist for `SKIN(0, skin)`, `FACE(face, skin)` (face is keyed by *skinColor*),
//! `HAIR(hairStyle, hairColor)`, and — unless the (race, sex) is facial-hair-excluded (Tauren, or a
//! female that isn't Night Elf / Undead) — `FACIAL_HAIR(facialHair, hairColor)`, plus a
//! CharacterFacialHairStyles row for `facialHair`. The real 5875 grids are *complete* over each
//! coupled pair, so the independent per-dial counts never produce an invalid tuple — asserted
//! exhaustively against the transcribed predicate in the real-data test.

use std::collections::{HashMap, HashSet};

use anyhow::{bail, Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const CHR_RACES: &str = "DBFilesClient\\ChrRaces.dbc";
const CHAR_BASE_INFO: &str = "DBFilesClient\\CharBaseInfo.dbc";
const CHAR_START_OUTFIT: &str = "DBFilesClient\\CharStartOutfit.dbc";
const CHAR_HAIR_GEOSETS: &str = "DBFilesClient\\CharHairGeosets.dbc";
const CHAR_FACIAL_HAIR_STYLES: &str = "DBFilesClient\\CharacterFacialHairStyles.dbc";
const CHAR_SECTIONS: &str = "DBFilesClient\\CharSections.dbc";

// CharSections `SectionType` values the range derivation reads (the hair apply `0x4784c0` pushes
// 3; the same constants `sections.rs` uses — kept local so the two concerns don't couple).
// `SECTION_FACIAL_HAIR` (2) isn't here: the facial-hair *count* comes from
// CharacterFacialHairStyles, not CharSections (the test's ValidateAppearance transcription defines
// it there, where it's used).
const SECTION_SKIN: u8 = 0;
const SECTION_FACE: u8 = 1;
const SECTION_HAIR: u8 = 3;

/// A CharSections availability key: `(race, sex, sectionType, variation, color)`.
type SectionKey = (u8, u8, u8, u8, u8);

/// The 8 playable races (ChrRaces ids 1–8); higher rows (Goblin = 9) aren't character-creatable.
const PLAYABLE_RACES: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

/// The 8 playable races' ChrRaces client fileStrings (col 15), a **frozen fact** of the build — the
/// load-time misparse guard for the string-column positions in [`load_races`] (if col 15 drifted,
/// these wouldn't match), and the GlueStrings key stems (`RACE_INFO_SCOURGE`, not `…_UNDEAD`).
const KNOWN_FILES: [(u8, &str); 8] = [
    (1, "Human"),
    (2, "Orc"),
    (3, "Dwarf"),
    (4, "NightElf"),
    (5, "Scourge"),
    (6, "Tauren"),
    (7, "Gnome"),
    (8, "Troll"),
];

/// The **raw** 1.12 `CharBaseInfo` content, race → classes (a frozen fact of the file; class ids:
/// Warrior 1, Paladin 2, Hunter 3, Rogue 4, Priest 5, Shaman 7, Mage 8, Warlock 9, Druid 11). Used
/// only as the load-time misparse guard (proves the 2-byte parse landed) — note it is NOT the
/// playable set: the file carries one dead pair, Dwarf-Mage, which the real client never offers
/// (see [`UNUSED_COMBOS`]).
const KNOWN_COMBOS: [(u8, &[u8]); 8] = [
    (1, &[1, 2, 4, 5, 8, 9]), // Human
    (2, &[1, 3, 4, 7, 9]),    // Orc
    (3, &[1, 2, 3, 4, 5, 8]), // Dwarf — 8 (Mage) is the dead row, stripped after the guard
    (4, &[1, 3, 4, 5, 11]),   // Night Elf
    (5, &[1, 4, 5, 8, 9]),    // Undead
    (6, &[1, 3, 7, 11]),      // Tauren
    (7, &[1, 4, 8, 9]),       // Gnome
    (8, &[1, 3, 4, 5, 7, 8]), // Troll
];

/// The `CharBaseInfo` pairs the real client never offers on the create screen — leftovers in the
/// data, not creatable content. Dwarf-Mage (3, 8) sits fully populated in both CharBaseInfo *and*
/// CharStartOutfit, yet the real client hardcodes the skip: its class-list builder (`0x4706b0`)
/// iterates CharBaseInfo and drops rows matching the literal `race == 3 && class == 8` — two x86
/// immediates, the only such pair binary-wide. Stripped here right after the
/// raw-parse guard — the same mechanism, one layer down — so `allows`/`classes_for_race` answer
/// what the real client offers.
const UNUSED_COMBOS: [(u8, u8); 1] = [(3, 8)];

/// One renderable item of a class's level-1 starting outfit (CharStartOutfit.dbc):
/// the item's **ItemDisplayInfo** id — the same kind of id the char-enum equipment carries, so it
/// feeds the equipment pipeline directly with no Item.dbc/template hop — and its InventoryType (the
/// worn slot). Non-worn entries (bags, food, hearthstone — InventoryType 0) are dropped at load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartOutfitItem {
    pub display_id: u32,
    pub inv_type: u8,
}

/// The five appearance-dial counts for one (race, sex); each dial cycles index `0..count-1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialRanges {
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
}

/// Character-creation source data: race body displayIds, race/class combos, and per-(race, sex)
/// appearance-dial ranges. Loaded once from the DBCs ([`Self::load`]).
pub struct CharCreateCatalog {
    /// race → (male displayId, female displayId), from ChrRaces cols 4/5.
    displays: HashMap<u8, (u32, u32)>,
    /// race → the ChrRaces client fileString (col 15: `"Human"`, `"Scourge"`, …) — the stem the glue
    /// screens key GlueStrings entries on (`RACE_INFO_<FILE>`, `ABILITY_INFO_<FILE><n>`).
    files: HashMap<u8, String>,
    /// race → (facial-hair customization token per sex [male, female] — cols 26/27 — and the hair
    /// customization token — col 28). The glue tokens behind the per-race dial labels
    /// (`HAIR_<tok>_STYLE`, `FACIAL_HAIR_<tok>`); a facial token of `"NONE"` hides that dial.
    custom_tokens: HashMap<u8, ([String; 2], String)>,
    /// The (race, class) combos CharBaseInfo permits.
    combos: HashSet<(u8, u8)>,
    /// (race, sex) → the five dial counts.
    ranges: HashMap<(u8, u8), DialRanges>,
    /// (race, class, sex) → the level-1 starting-outfit items the create preview dresses in
    /// (CharStartOutfit) — worn slots only, DisplayId + InventoryType.
    start_outfits: HashMap<(u8, u8, u8), Vec<StartOutfitItem>>,
}

impl CharCreateCatalog {
    /// The body model displayId for a (race, sex) — resolved through the ordinary CreatureDisplayInfo
    /// chain, the same as any streamed unit. `None` for a non-playable race.
    pub fn body_display(&self, race: u8, sex: u8) -> Option<u32> {
        self.displays
            .get(&race)
            .map(|&(m, f)| if sex == 0 { m } else { f })
    }

    /// Whether a race may be created as a class (CharBaseInfo).
    pub fn allows(&self, race: u8, class: u8) -> bool {
        self.combos.contains(&(race, class))
    }

    /// The ChrRaces client fileString for a race (`"Human"`, `"Scourge"`, …) — the GlueStrings key
    /// stem. `None` for a non-playable race.
    pub fn race_file(&self, race: u8) -> Option<&str> {
        self.files.get(&race).map(String::as_str)
    }

    /// The hair-customization glue token for a race (`"NORMAL"`, `"HORNS"`) — the dial-label key
    /// (`HAIR_<tok>_STYLE` / `HAIR_<tok>_COLOR`).
    pub fn hair_customization(&self, race: u8) -> Option<&str> {
        self.custom_tokens.get(&race).map(|(_, h)| h.as_str())
    }

    /// The facial-hair-customization glue token for a (race, sex) (`"NORMAL"`, `"MARKINGS"`, …) —
    /// the dial-label key (`FACIAL_HAIR_<tok>`); `"NONE"` means the dial is hidden (the ref's
    /// `GetFacialHairCustomization() == "NONE"` branch).
    pub fn facial_hair_customization(&self, race: u8, sex: u8) -> Option<&str> {
        self.custom_tokens
            .get(&race)
            .map(|(f, _)| f[(sex as usize).min(1)].as_str())
    }

    /// The classes a race may be created as, ascending by class id. The real client's grid order is
    /// CharBaseInfo *row* order (its builder `0x4706b0` walks the table) — which in
    /// the 5875 file is ascending class id for all 8 races (verified byte fact), so the sort here is
    /// order-equivalent on the real data.
    pub fn classes_for_race(&self, race: u8) -> Vec<u8> {
        let mut cs: Vec<u8> = self
            .combos
            .iter()
            .filter(|&&(r, _)| r == race)
            .map(|&(_, c)| c)
            .collect();
        cs.sort_unstable();
        cs
    }

    /// The five appearance-dial counts for a (race, sex). `None` for a non-playable race.
    pub fn ranges(&self, race: u8, sex: u8) -> Option<DialRanges> {
        self.ranges.get(&(race, sex)).copied()
    }

    /// The renderable starting-outfit items for a (race, class, sex) — the level-1 gear the create
    /// screen dresses the preview in. Empty for a combo with no CharStartOutfit row.
    pub fn start_outfit(&self, race: u8, class: u8, sex: u8) -> &[StartOutfitItem] {
        self.start_outfits
            .get(&(race, class, sex))
            .map_or(&[], Vec::as_slice)
    }

    /// Load the character-creation catalog from the patch chain (ChrRaces + CharBaseInfo +
    /// CharSections + CharHairGeosets + CharacterFacialHairStyles). Two load-time self-checks fail
    /// loudly on a misparse: the parsed combos must equal the frozen 1.12 table (guards the 2-byte
    /// CharBaseInfo layout), and every playable (race, sex) must have a body display + all five
    /// ranges nonzero.
    pub fn load(chain: &mut Chain) -> Result<Self> {
        let (displays, files, custom_tokens) = load_races(chain)?;
        let combos = load_combos(chain)?;
        let start_outfits = load_start_outfits(chain)?;
        let avail = load_available_sections(chain)?;
        let hair_geo = load_hair_geosets(chain)?;
        let facial = load_facial_hair_styles(chain)?;

        let mut ranges = HashMap::new();
        for race in PLAYABLE_RACES {
            for sex in [0u8, 1] {
                ranges.insert(
                    (race, sex),
                    derive_ranges(race, sex, &avail, &hair_geo, &facial),
                );
            }
        }

        let mut catalog = Self {
            displays,
            files,
            custom_tokens,
            combos,
            ranges,
            start_outfits,
        };
        catalog.self_check()?;
        for combo in UNUSED_COMBOS {
            catalog.combos.remove(&combo);
        }
        Ok(catalog)
    }

    /// Load-time misparse guards (see [`Self::load`]).
    fn self_check(&self) -> Result<()> {
        for (race, classes) in KNOWN_COMBOS {
            let got = self.classes_for_race(race);
            if got != classes {
                bail!(
                    "CharBaseInfo misparse: race {race} classes {got:?} != known {classes:?} \
                     (check the 2-byte race/class layout)"
                );
            }
        }
        for (race, file) in KNOWN_FILES {
            if self.race_file(race) != Some(file) {
                bail!(
                    "ChrRaces misparse: race {race} fileString {:?} != known {file:?} \
                     (check the string-column positions in load_races)",
                    self.race_file(race)
                );
            }
        }
        // CharStartOutfit byte-layout guard (like the CharBaseInfo one): Human(1) Warrior(1) male(0)
        // wears the recruit set — shirt disp 9891 (inv 4/BODY), pants 9892 (7/LEGS), boots 10141
        // (8/FEET), plus the worn shortsword 1542 (21/MAINHAND) and small shield 18730 (14/SHIELD).
        // If the packed race/class/gender head or the DisplayId/InventoryType array offsets drifted,
        // these (display, inv) pairs wouldn't line up. Bags/food (InventoryType 0) are dropped, so
        // the count is exactly these five worn items.
        let hw = self.start_outfit(1, 1, 0);
        for &(disp, inv) in &[
            (9891u32, 4u8),
            (9892, 7),
            (10141, 8),
            (1542, 21),
            (18730, 14),
        ] {
            if !hw
                .iter()
                .any(|it| it.display_id == disp && it.inv_type == inv)
            {
                bail!(
                    "CharStartOutfit misparse: Human Warrior male missing display {disp} inv {inv} \
                     (got {hw:?}) — check the 41-field / 152-byte packed layout"
                );
            }
        }
        for race in PLAYABLE_RACES {
            for sex in [0u8, 1] {
                if self.body_display(race, sex).unwrap_or(0) == 0 {
                    bail!("ChrRaces: race {race} sex {sex} has no body displayId");
                }
                if self.facial_hair_customization(race, sex).is_none()
                    || self.hair_customization(race).is_none()
                {
                    bail!("ChrRaces: race {race} has no customization tokens");
                }
                let r = self
                    .ranges(race, sex)
                    .with_context(|| format!("no ranges for race {race} sex {sex}"))?;
                if r.skin == 0
                    || r.face == 0
                    || r.hair_style == 0
                    || r.hair_color == 0
                    || r.facial_hair == 0
                {
                    bail!("customization ranges: race {race} sex {sex} has a zero dial ({r:?})");
                }
            }
        }
        Ok(())
    }
}

/// ChrRaces → per playable race: the (male, female) displayIds, the client fileString, and the glue
/// customization tokens. 29 fields in build 5875 (record size 116); we read cols 0 (RaceID),
/// 4 (MaleDisplayId), 5 (FemaleDisplayId), 15 (ClientFileString), 26/27 (FacialHairCustomization
/// male/female) and 28 (HairCustomization) — the rest (name strings + spell/sound ids) are declared
/// `UInt32` (a string column is a 4-byte offset, read and ignored). The string-column positions are
/// guarded by [`CharCreateCatalog::self_check`]'s frozen fileString table.
#[allow(clippy::type_complexity)] // one pass over one DBC → the catalog's three race-keyed maps
fn load_races(
    chain: &mut Chain,
) -> Result<(
    HashMap<u8, (u32, u32)>,
    HashMap<u8, String>,
    HashMap<u8, ([String; 2], String)>,
)> {
    let bytes = chain
        .read_file(CHR_RACES)
        .with_context(|| format!("reading {CHR_RACES}"))?;
    let mut schema = Schema::new("ChrRaces");
    for i in 0..29 {
        let ty = match i {
            15 | 26 | 27 | 28 => FieldType::String,
            _ => FieldType::UInt32,
        };
        schema.add_field(SchemaField::new(format!("f{i}"), ty));
    }
    let rs = parse(&bytes, schema, "ChrRaces")?;
    let mut displays = HashMap::new();
    let mut files = HashMap::new();
    let mut tokens = HashMap::new();
    for r in rs.records() {
        let Some(race) = u32_at(r, 0) else { continue };
        let race = race as u8;
        if !PLAYABLE_RACES.contains(&race) {
            continue;
        }
        if let (Some(male), Some(female)) = (u32_at(r, 4), u32_at(r, 5)) {
            displays.insert(race, (male, female));
        }
        if let Some(file) = str_at(&rs, r, 15) {
            files.insert(race, file);
        }
        if let (Some(fm), Some(ff), Some(hair)) =
            (str_at(&rs, r, 26), str_at(&rs, r, 27), str_at(&rs, r, 28))
        {
            tokens.insert(race, ([fm, ff], hair));
        }
    }
    Ok((displays, files, tokens))
}

/// CharBaseInfo → the (race, class) combos. A **special** DBC: 2 columns of **one byte each**
/// (record size 2, not 8), so the generic 4-byte-field reader can't touch it — parsed by hand off the
/// raw record block. Byte 0 = RaceID, byte 1 = ClassID (verified against the 5875 file + the known
/// combo table).
fn load_combos(chain: &mut Chain) -> Result<HashSet<(u8, u8)>> {
    let bytes = chain
        .read_file(CHAR_BASE_INFO)
        .with_context(|| format!("reading {CHAR_BASE_INFO}"))?;
    if bytes.len() < 20 || &bytes[0..4] != b"WDBC" {
        bail!("CharBaseInfo: not a WDBC file");
    }
    let record_count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let field_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let record_size = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    if field_count != 2 || record_size != 2 {
        bail!("CharBaseInfo: unexpected layout (fields {field_count}, record size {record_size}; expected 2/2)");
    }
    let data = &bytes[20..];
    let mut combos = HashSet::new();
    for i in 0..record_count {
        let off = i * record_size;
        if off + 2 > data.len() {
            bail!("CharBaseInfo: record {i} runs past the file");
        }
        combos.insert((data[off], data[off + 1]));
    }
    Ok(combos)
}

/// CharStartOutfit → (race, class, sex) → the worn starting items. A **special** DBC (like
/// CharBaseInfo): 41 columns / 152 bytes with a byte-packed race/class/gender/unused head, so the
/// generic 4-byte-field reader can't touch it — parsed by hand off the raw record block. Layout,
/// byte-verified against the 5875 file: `ID` u32 @0; `Race`/`Class`/`Gender`/unused u8 @4/5/6/7;
/// `ItemId[12]` i32 @8 (dropped — we render by display); **`DisplayId[12]` i32 @56**;
/// **`InventoryType[12]` i32 @104**. Each row keeps the (DisplayId, InventoryType) pair of every
/// **worn** slot (both ≥ 1 — this drops the empty `-1` tail and the InventoryType-0 bag/food/hearth
/// entries, which never render). DisplayId is already an ItemDisplayInfo id (the char-enum
/// equipment convention), so it feeds the equipment pipeline with no template hop.
fn load_start_outfits(chain: &mut Chain) -> Result<HashMap<(u8, u8, u8), Vec<StartOutfitItem>>> {
    let bytes = chain
        .read_file(CHAR_START_OUTFIT)
        .with_context(|| format!("reading {CHAR_START_OUTFIT}"))?;
    if bytes.len() < 20 || &bytes[0..4] != b"WDBC" {
        bail!("CharStartOutfit: not a WDBC file");
    }
    let record_count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let field_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let record_size = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    if field_count != 41 || record_size != 152 {
        bail!(
            "CharStartOutfit: unexpected layout (fields {field_count}, record size {record_size}; \
             expected 41/152)"
        );
    }
    let data = &bytes[20..];
    let i32_at = |rec: &[u8], off: usize| i32::from_le_bytes(rec[off..off + 4].try_into().unwrap());
    let mut map: HashMap<(u8, u8, u8), Vec<StartOutfitItem>> = HashMap::new();
    for i in 0..record_count {
        let off = i * record_size;
        if off + record_size > data.len() {
            bail!("CharStartOutfit: record {i} runs past the file");
        }
        let rec = &data[off..off + record_size];
        let (race, class, sex) = (rec[4], rec[5], rec[6]);
        let mut items = Vec::new();
        for slot in 0..12 {
            let display = i32_at(rec, 56 + slot * 4);
            let inv = i32_at(rec, 104 + slot * 4);
            if display >= 1 && inv >= 1 {
                items.push(StartOutfitItem {
                    display_id: display as u32,
                    inv_type: inv as u8,
                });
            }
        }
        map.insert((race, class, sex), items);
    }
    Ok(map)
}

/// CharSections → the set of *available* `(race, sex, sectionType, variation, color)` keys — the rows
/// a player may select (`Flags & 0x1 == 0`; the `0x1`-flagged rows are `SECTION_FLAG_UNAVAILABLE`,
/// the late `…_100` extras the compositor already skips). This is exactly the domain vmangos's
/// `GetCharSectionEntry` searches, so range derivation + validation read the same predicate.
fn load_available_sections(chain: &mut Chain) -> Result<HashSet<SectionKey>> {
    let bytes = chain
        .read_file(CHAR_SECTIONS)
        .with_context(|| format!("reading {CHAR_SECTIONS}"))?;
    // 10 fields (ID, Race, Sex, SectionType, Variation, Color, Tex0-2, Flags); we read the ints.
    let rs = parse(&bytes, all_u32_schema("CharSections", 10), "CharSections")?;
    let mut avail = HashSet::new();
    for r in rs.records() {
        if let (Some(race), Some(sex), Some(ty), Some(var), Some(color), Some(flags)) = (
            u32_at(r, 1),
            u32_at(r, 2),
            u32_at(r, 3),
            u32_at(r, 4),
            u32_at(r, 5),
            u32_at(r, 9),
        ) {
            if flags & 0x1 == 0 {
                avail.insert((race as u8, sex as u8, ty as u8, var as u8, color as u8));
            }
        }
    }
    Ok(avail)
}

/// CharHairGeosets → (race, sex) → the set of hair-style `VariationID`s. 6 fields (ID, Race, Sex,
/// Variation, GeosetID, unused); we key on cols 1/2/3, as the client's `0x478540` does.
fn load_hair_geosets(chain: &mut Chain) -> Result<HashMap<(u8, u8), HashSet<u8>>> {
    let bytes = chain
        .read_file(CHAR_HAIR_GEOSETS)
        .with_context(|| format!("reading {CHAR_HAIR_GEOSETS}"))?;
    let rs = parse(
        &bytes,
        all_u32_schema("CharHairGeosets", 6),
        "CharHairGeosets",
    )?;
    let mut map: HashMap<(u8, u8), HashSet<u8>> = HashMap::new();
    for r in rs.records() {
        if let (Some(race), Some(sex), Some(var)) = (u32_at(r, 1), u32_at(r, 2), u32_at(r, 3)) {
            map.entry((race as u8, sex as u8))
                .or_default()
                .insert(var as u8);
        }
    }
    Ok(map)
}

/// CharacterFacialHairStyles → (race, sex) → the set of facial-hair `VariationID`s. 9 fields; the
/// keys are cols 0/1/2 (Race, Sex, Variation, as `0x478740` keys them — note col 0 is Race here,
/// not an ID).
fn load_facial_hair_styles(chain: &mut Chain) -> Result<HashMap<(u8, u8), HashSet<u8>>> {
    let bytes = chain
        .read_file(CHAR_FACIAL_HAIR_STYLES)
        .with_context(|| format!("reading {CHAR_FACIAL_HAIR_STYLES}"))?;
    let rs = parse(
        &bytes,
        all_u32_schema("CharacterFacialHairStyles", 9),
        "CharacterFacialHairStyles",
    )?;
    let mut map: HashMap<(u8, u8), HashSet<u8>> = HashMap::new();
    for r in rs.records() {
        if let (Some(race), Some(sex), Some(var)) = (u32_at(r, 0), u32_at(r, 1), u32_at(r, 2)) {
            map.entry((race as u8, sex as u8))
                .or_default()
                .insert(var as u8);
        }
    }
    Ok(map)
}

/// Count distinct values on one axis of a (race, sex, sectionType) group in `avail`.
fn section_axis_count(
    avail: &HashSet<SectionKey>,
    race: u8,
    sex: u8,
    ty: u8,
    variation_axis: bool,
) -> u8 {
    avail
        .iter()
        .filter(|&&(r, s, t, _, _)| r == race && s == sex && t == ty)
        .map(|&(_, _, _, v, c)| if variation_axis { v } else { c })
        .collect::<HashSet<u8>>()
        .len() as u8
}

/// Derive the five dial counts for a (race, sex) from the section domain + the two geoset tables.
fn derive_ranges(
    race: u8,
    sex: u8,
    avail: &HashSet<SectionKey>,
    hair_geo: &HashMap<(u8, u8), HashSet<u8>>,
    facial: &HashMap<(u8, u8), HashSet<u8>>,
) -> DialRanges {
    DialRanges {
        skin: section_axis_count(avail, race, sex, SECTION_SKIN, false),
        face: section_axis_count(avail, race, sex, SECTION_FACE, true),
        // The hair-style count is the client's CharHairGeosets fetcher (`max(1, …)` — a race/sex with
        // no rows still has the bald style), NOT the CharSections HAIR variation count (Orc female has
        // one extra HAIR variation with no geoset; offering it would be a dead style).
        hair_style: hair_geo
            .get(&(race, sex))
            .map_or(1, |v| v.len().max(1) as u8),
        hair_color: section_axis_count(avail, race, sex, SECTION_HAIR, false),
        facial_hair: facial.get(&(race, sex)).map_or(0, |v| v.len() as u8),
    }
}

/// A schema of `n` `UInt32` fields named `f0..fn` — for the DBCs we read purely by index (the field
/// *names* are irrelevant; string columns are 4-byte offsets read as ints and ignored). The parser
/// requires the count to match the file header, which is the alignment guard.
fn all_u32_schema(name: &str, n: usize) -> Schema {
    let mut s = Schema::new(name);
    for i in 0..n {
        s.add_field(SchemaField::new(format!("f{i}"), FieldType::UInt32));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CharSections `SECTION_TYPE_FACIAL_HAIR` — used only by the predicate transcription (the runtime
    /// facial-hair count comes from CharacterFacialHairStyles, so the lib never references it).
    const SECTION_FACIAL_HAIR: u8 = 2;

    /// vmangos's `Player::ValidateAppearance`, transcribed verbatim (`Player.cpp:326`): the exact
    /// predicate the server runs on `CMSG_CHAR_CREATE`. `GetCharSectionEntry` is an available-row
    /// lookup (`avail` already excludes the `0x1`-flagged rows), and face is keyed by *skinColor*.
    fn validate_appearance(
        avail: &HashSet<SectionKey>,
        facial: &HashMap<(u8, u8), HashSet<u8>>,
        race: u8,
        sex: u8,
        skin: u8,
        face: u8,
        hair: u8,
        hair_color: u8,
        facial_hair: u8,
    ) -> bool {
        let has = |ty, var, color| avail.contains(&(race, sex, ty, var, color));
        if !has(SECTION_SKIN, 0, skin) {
            return false;
        }
        if !has(SECTION_FACE, face, skin) {
            return false;
        }
        if !has(SECTION_HAIR, hair, hair_color) {
            return false;
        }
        // Tauren, and any female that isn't Night Elf (4) or Undead (5), have no FACIAL_HAIR sections.
        let excluded = race == 6 || (sex == 1 && race != 4 && race != 5);
        if !excluded && !has(SECTION_FACIAL_HAIR, facial_hair, hair_color) {
            return false;
        }
        facial
            .get(&(race, sex))
            .is_some_and(|vs| vs.contains(&facial_hair))
    }

    /// The whole Phase-2 contract on the **real** 5875 DBCs: the 16 body displayIds resolve to a
    /// character body model; the parsed combos equal the frozen 1.12 table; the derived ranges are
    /// dense `0..n-1` on every dial; and — the load-bearing guarantee — every tuple in the product of
    /// a (race, sex)'s ranges passes the transcribed `ValidateAppearance`. Skips without client data.
    #[test]
    fn char_create_catalog_matches_the_5875_dbcs() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = CharCreateCatalog::load(&mut chain).expect("load char-create catalog");
        let creatures = crate::load_creature_catalog(&mut chain).expect("load creatures");

        // 1 · The 16 body displayIds resolve through the CreatureDisplayInfo chain to a body model.
        for race in PLAYABLE_RACES {
            for sex in [0u8, 1] {
                let display = cat.body_display(race, sex).expect("has display");
                let model = creatures.model(display).unwrap_or_else(|| {
                    panic!("display {display} (race {race} sex {sex}) resolves")
                });
                assert!(
                    model
                        .model_path
                        .to_ascii_lowercase()
                        .starts_with("character\\"),
                    "race {race} sex {sex} display {display} → {:?} (expected a Character body)",
                    model.model_path
                );
            }
        }

        // 2 · The catalog answers the *playable* sets: the frozen raw table (guarded at load)
        // minus the dead pairs the real client never offers (UNUSED_COMBOS — Dwarf-Mage).
        for (race, classes) in KNOWN_COMBOS {
            let playable: Vec<u8> = classes
                .iter()
                .copied()
                .filter(|&c| !UNUSED_COMBOS.contains(&(race, c)))
                .collect();
            assert_eq!(
                cat.classes_for_race(race),
                playable,
                "combos for race {race}"
            );
        }
        assert_eq!(
            cat.classes_for_race(3),
            vec![1, 2, 3, 4, 5],
            "Dwarf offers exactly Warrior/Paladin/Hunter/Rogue/Priest — no Mage (the dead \
             CharBaseInfo row must not reach the create screen)"
        );

        // 2b · The glue customization tokens land in the right columns (spot-checks pin both the
        // column positions and the male/female order of cols 26/27 against the dumped 5875 rows:
        // Tauren cycle horns, males normal facial hair, Human females piercings, Night Elf
        // females markings, trolls tusks both sexes). Note: no 5875 row is "NONE" — every
        // (race, sex) has a facial dial in this build.
        assert_eq!(cat.hair_customization(6), Some("HORNS"), "Tauren hair tok");
        assert_eq!(cat.hair_customization(1), Some("NORMAL"), "Human hair tok");
        assert_eq!(
            cat.facial_hair_customization(1, 0),
            Some("NORMAL"),
            "Human male facial tok"
        );
        assert_eq!(
            cat.facial_hair_customization(1, 1),
            Some("PIERCINGS"),
            "Human female facial tok"
        );
        assert_eq!(
            cat.facial_hair_customization(4, 1),
            Some("MARKINGS"),
            "Night Elf female facial tok"
        );
        assert_eq!(
            cat.facial_hair_customization(8, 0),
            Some("TUSKS"),
            "Troll male facial tok"
        );

        // 2c · CharStartOutfit: every creatable (race, class, sex) combo dresses
        // in ≥1 worn item, and the InventoryType semantics land — the Human Warrior recruit set
        // (guarded at load too) and the Human Mage's robe (InventoryType 20, chest column).
        // (Creatable = the playable sets; the dead Dwarf-Mage pair also has a populated outfit
        // row, but nothing can ever wear it.)
        for (race, classes) in KNOWN_COMBOS {
            for &class in classes
                .iter()
                .filter(|&&c| !UNUSED_COMBOS.contains(&(race, c)))
            {
                for sex in [0u8, 1] {
                    let out = cat.start_outfit(race, class, sex);
                    assert!(
                        !out.is_empty(),
                        "race {race} class {class} sex {sex} has no start outfit"
                    );
                    assert!(
                        out.iter().all(|it| it.display_id >= 1 && it.inv_type >= 1),
                        "race {race} class {class} sex {sex} outfit has a 0/empty entry: {out:?}"
                    );
                }
            }
        }
        let mage = cat.start_outfit(1, 8, 0);
        assert!(
            mage.iter()
                .any(|it| it.inv_type == 20 && it.display_id == 12647),
            "Human Mage male should wear a robe (inv 20, disp 12647): {mage:?}"
        );

        // 3 + 4 · Ranges dense; every producible tuple passes ValidateAppearance.
        let avail = load_available_sections(&mut chain).expect("sections");
        let facial = load_facial_hair_styles(&mut chain).expect("facial hair");
        for race in PLAYABLE_RACES {
            for sex in [0u8, 1] {
                let r = cat.ranges(race, sex).expect("ranges");
                for sk in 0..r.skin {
                    for fa in 0..r.face {
                        for hs in 0..r.hair_style {
                            for hc in 0..r.hair_color {
                                for fh in 0..r.facial_hair {
                                    assert!(
                                        validate_appearance(
                                            &avail, &facial, race, sex, sk, fa, hs, hc, fh
                                        ),
                                        "race {race} sex {sex} tuple \
                                         skin={sk} face={fa} hair={hs} hairColor={hc} facial={fh} \
                                         fails ValidateAppearance"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
