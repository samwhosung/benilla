//! Character-creation source data from ChrRaces, CharBaseInfo and CharStartOutfit, and the five
//! appearance-dial ranges per (race, sex), counted from the DBCs: skin colour, face and hair colour
//! off the CharSections rows the client groups into its grid (`0x476020`), hair and facial-hair
//! styles off CharHairGeosets and CharacterFacialHairStyles. Every tuple in a (race, sex)'s ranges
//! passes vmangos's `Player::ValidateAppearance`, since the shipped grids are complete over each
//! coupled pair.

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

// CharSections `SectionType` values the dial ranges read (hair is 3 at `0x4784c0`).
const SECTION_SKIN: u8 = 0;
const SECTION_FACE: u8 = 1;
const SECTION_HAIR: u8 = 3;

/// A CharSections availability key: `(race, sex, sectionType, variation, color)`.
type SectionKey = (u8, u8, u8, u8, u8);

/// The playable races, ChrRaces ids 1–8; Goblin (9) and above are not creatable.
const PLAYABLE_RACES: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

/// The shipped ChrRaces fileStrings (col 15), guarding [`load_races`]' string columns.
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

/// The shipped `CharBaseInfo` rows by race, the load-time guard on the 2-byte parse; not the
/// playable set, as it holds the dead Dwarf-Mage row ([`UNUSED_COMBOS`]).
const KNOWN_COMBOS: [(u8, &[u8]); 8] = [
    (1, &[1, 2, 4, 5, 8, 9]), // Human
    (2, &[1, 3, 4, 7, 9]),    // Orc
    (3, &[1, 2, 3, 4, 5, 8]), // Dwarf; 8 (Mage) is the dead row
    (4, &[1, 3, 4, 5, 11]),   // Night Elf
    (5, &[1, 4, 5, 8, 9]),    // Undead
    (6, &[1, 3, 7, 11]),      // Tauren
    (7, &[1, 4, 8, 9]),       // Gnome
    (8, &[1, 3, 4, 5, 7, 8]), // Troll
];

/// `CharBaseInfo` pairs the client never offers: its class-list builder `0x4706b0` skips the
/// literal `race == 3 && class == 8` (Dwarf-Mage), though the row and its outfit are populated.
const UNUSED_COMBOS: [(u8, u8); 1] = [(3, 8)];

/// One worn item of a CharStartOutfit row: its ItemDisplayInfo id and its InventoryType.
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

/// Character-creation source data, loaded once from the DBCs ([`Self::load`]).
pub struct CharCreateCatalog {
    /// race → (male displayId, female displayId), from ChrRaces cols 4/5.
    displays: HashMap<u8, (u32, u32)>,
    /// race → ChrRaces fileString (col 15), the GlueStrings key stem (`RACE_INFO_<FILE>`).
    files: HashMap<u8, String>,
    /// race → ([male, female] facial-hair token, cols 26/27; hair token, col 28).
    custom_tokens: HashMap<u8, ([String; 2], String)>,
    combos: HashSet<(u8, u8)>,
    ranges: HashMap<(u8, u8), DialRanges>,
    /// (race, class, sex) → the worn starting-outfit items.
    start_outfits: HashMap<(u8, u8, u8), Vec<StartOutfitItem>>,
}

impl CharCreateCatalog {
    /// The body displayId for a (race, sex), resolved through CreatureDisplayInfo like any unit.
    pub fn body_display(&self, race: u8, sex: u8) -> Option<u32> {
        self.displays
            .get(&race)
            .map(|&(m, f)| if sex == 0 { m } else { f })
    }

    /// Whether a race may be created as a class (CharBaseInfo less `UNUSED_COMBOS`).
    pub fn allows(&self, race: u8, class: u8) -> bool {
        self.combos.contains(&(race, class))
    }

    /// The ChrRaces fileString for a race (`"Human"`, `"Scourge"`), the GlueStrings key stem.
    pub fn race_file(&self, race: u8) -> Option<&str> {
        self.files.get(&race).map(String::as_str)
    }

    /// A race's hair glue token (`"NORMAL"`, `"HORNS"`), keying `HAIR_<tok>_STYLE`/`_COLOR`.
    pub fn hair_customization(&self, race: u8) -> Option<&str> {
        self.custom_tokens.get(&race).map(|(_, h)| h.as_str())
    }

    /// A (race, sex)'s facial-hair glue token, keying `FACIAL_HAIR_<tok>`; the glue hides the dial
    /// when `GetFacialHairCustomization()` is `"NONE"`.
    pub fn facial_hair_customization(&self, race: u8, sex: u8) -> Option<&str> {
        self.custom_tokens
            .get(&race)
            .map(|(f, _)| f[(sex as usize).min(1)].as_str())
    }

    /// The classes a race may be created as, ascending by id: the client lists them in CharBaseInfo
    /// row order (`0x4706b0`), which is ascending class id in the shipped file.
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

    /// The five appearance-dial counts for a (race, sex).
    pub fn ranges(&self, race: u8, sex: u8) -> Option<DialRanges> {
        self.ranges.get(&(race, sex)).copied()
    }

    /// The worn level-1 gear the create preview dresses a (race, class, sex) in.
    pub fn start_outfit(&self, race: u8, class: u8, sex: u8) -> &[StartOutfitItem] {
        self.start_outfits
            .get(&(race, class, sex))
            .map_or(&[], Vec::as_slice)
    }

    /// Load the catalog from the patch chain. The misparse guards compare the raw combos, so they
    /// run before `UNUSED_COMBOS` are stripped.
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
        // CharStartOutfit layout guard: the shipped Human Warrior male set, (display,
        // InventoryType) for the shirt, pants, boots, shortsword and shield.
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

/// ChrRaces (29 fields, 116-byte records) → per playable race: displayIds (cols 4/5), fileString
/// (15), facial-hair (26/27) and hair (28) tokens; other string columns are read as ignored `u32`s.
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

/// CharBaseInfo → (race, class) pairs: two one-byte fields, parsed by hand off the record block.
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

/// CharStartOutfit → (race, class, sex) → worn items, parsed by hand (41 fields, 152 bytes): `ID`
/// u32 @0, race/class/gender u8 @4/5/6, `ItemId[12]` @8, `DisplayId[12]` @56, `InventoryType[12]`
/// @104. A slot is kept when both are ≥ 1, dropping the `-1` tail and the InventoryType-0 bags.
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

/// CharSections → the selectable keys, rows without flag `0x1` (`SECTION_FLAG_UNAVAILABLE`): the
/// domain vmangos's `GetCharSectionEntry` searches.
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

/// CharHairGeosets → (race, sex) → hair-style variations, keyed on cols 1/2/3 as `0x478540` does.
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

/// CharacterFacialHairStyles → (race, sex) → variations, keyed on cols 0/1/2 as `0x478740` does.
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
        // Hair styles are CharHairGeosets variations, at least 1 (bald), not CharSections HAIR
        // variations: Orc female has one HAIR variation with no geoset.
        hair_style: hair_geo
            .get(&(race, sex))
            .map_or(1, |v| v.len().max(1) as u8),
        hair_color: section_axis_count(avail, race, sex, SECTION_HAIR, false),
        facial_hair: facial.get(&(race, sex)).map_or(0, |v| v.len() as u8),
    }
}

/// `n` `UInt32` fields for a DBC read by index; the parser's field-count check guards the layout.
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

    /// CharSections `SECTION_TYPE_FACIAL_HAIR`, needed only by the transcribed check.
    const SECTION_FACIAL_HAIR: u8 = 2;

    /// vmangos's `Player::ValidateAppearance` (`Player.cpp:326`), run on `CMSG_CHAR_CREATE`;
    /// `avail` already excludes the `0x1`-flagged rows, and face is keyed by skin colour.
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
        // Tauren and all females but Night Elf (4) and Undead (5) have no FACIAL_HAIR sections.
        let excluded = race == 6 || (sex == 1 && race != 4 && race != 5);
        if !excluded && !has(SECTION_FACIAL_HAIR, facial_hair, hair_color) {
            return false;
        }
        facial
            .get(&(race, sex))
            .is_some_and(|vs| vs.contains(&facial_hair))
    }

    #[test]
    fn char_create_catalog_matches_the_5875_dbcs() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = CharCreateCatalog::load(&mut chain).expect("load char-create catalog");
        let creatures = crate::load_creature_catalog(&mut chain).expect("load creatures");

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

        // The playable sets are the raw table less the dead pairs.
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

        // The glue tokens against the shipped rows, pinning cols 26/27's male/female order; no
        // shipped row is "NONE".
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

        // Every creatable combo wears at least one item; the Human Mage wears a robe (inv 20).
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

        // Every tuple in the dial ranges passes ValidateAppearance.
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
