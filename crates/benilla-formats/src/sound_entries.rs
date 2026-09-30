//! `SoundEntries.dbc`, the sound-kit table every audio trigger resolves through: up to 10
//! weighted variation files with volume, flags and distances. Kits play by id or by name:
//! `PlaySoundByName` (`0x458030`), behind Lua's `PlaySound`, hashes the name into this table.
//!
//! 29 fields and 116-byte records in 5875. The wowdev wiki's 30-column layout, with separate
//! `maxDistance` and `soundEntriesAdvancedID` columns, does not fit this build.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, str_at, u32_at};

const SOUND_ENTRIES: &str = "DBFilesClient\\SoundEntries.dbc";

/// `Flags` bits, copied raw into the runtime kit (`0x45c139`): `0x400` varies pitch (`0x458da0`),
/// `0x800` volume (`0x458c60`), though no 5875 kit sets it. `0x20` no-duplicates and `0x200`
/// looping are the wiki's 0.5.3 meanings, consistent with behaviour.
pub mod sound_kit_flags {
    pub const NO_DUPLICATES: u32 = 0x20;
    pub const LOOPING: u32 = 0x200;
    pub const VARY_PITCH: u32 = 0x400;
    pub const VARY_VOLUME: u32 = 0x800;
}

/// One sound kit: its resolved variations and playback parameters.
pub struct SoundKit {
    pub id: u32,
    /// The kit's category (1 spells, 2 UI, 3 footsteps, 28 zone music, 50 zone ambience), which
    /// picks the volume category.
    pub sound_type: u32,
    /// The `PlaySoundByName` key (`"igMainMenuOpen"`, `"LevelUp"`).
    pub name: String,
    /// Non-empty variations as `(MPQ path, Freq[i] weight)`, `DirectoryBase` and `File[i]` joined
    /// as the reference joins them.
    pub files: Vec<(String, u32)>,
    /// Base volume in `[0, 1]`, scaled by the per-shot variation (`0x458c60`).
    pub volume: f32,
    pub flags: u32,
    /// Full-volume radius fed to the backend's min/max rolloff (FMOD `Sample_SetMinMaxDistance`).
    pub min_distance: f32,
    /// The audibility radius, `d² < cutoff²`, also the per-frame virtualization cull (`0x45cdf0`,
    /// `0x7a5000`); 0 is non-positional.
    pub distance_cutoff: f32,
    /// The `SoundSamplePreferences.dbc` row for the channel's EAX send. 0 is dry, not a default:
    /// that table holds only 1 and 2, so the slot lookup (`0x45cdc0`) returns null and
    /// `FSOUND_Reverb_SetChannelProperties` (`0x7a5bf0`) skips. Every NPC voice kit is 0.
    pub eax_def: u32,
}

/// The names `0x4609b0` finds the two forced ambience beds by (`0x836444`, `0x836428`).
const GHOST_BED_NAME: &str = "Ghost (DONOTRENAME)";
const UNDERWATER_BED_NAME: &str = "Underwater (DONOTRENAME)";

/// All kits, resolvable by id or, ignoring case, by name.
pub struct SoundKitCatalog {
    kits: HashMap<u32, SoundKit>,
    /// Lowercased `Name` to id: the reference's name hash ignores case.
    by_name: HashMap<String, u32>,
    ghost_bed: Option<u32>,
    underwater_bed: Option<u32>,
}

impl SoundKitCatalog {
    pub fn get(&self, id: u32) -> Option<&SoundKit> {
        self.kits.get(&id)
    }

    /// A kit by its `Name`, ignoring case, as `PlaySoundByName` finds it.
    pub fn by_name(&self, name: &str) -> Option<&SoundKit> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .and_then(|id| self.kits.get(id))
    }

    /// The ambience bed a ghost hears, the row named `"Ghost (DONOTRENAME)"`: `[0xb06d48]`.
    pub fn ghost_bed(&self) -> Option<u32> {
        self.ghost_bed
    }

    /// The ambience bed under water, the row named `"Underwater (DONOTRENAME)"`: `[0xb06d4c]`.
    pub fn underwater_bed(&self) -> Option<u32> {
        self.underwater_bed
    }

    pub fn len(&self) -> usize {
        self.kits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kits.is_empty()
    }

    /// An empty catalog for consumers' unit tests.
    pub fn empty_for_tests() -> Self {
        Self::from_kits(Vec::new())
    }

    /// A catalog of file-less kits, `(id, name)` in row order, for consumers' unit tests.
    pub fn named_for_tests(rows: &[(u32, &str)]) -> Self {
        Self::from_kits(
            rows.iter()
                .map(|&(id, name)| SoundKit {
                    id,
                    sound_type: 0,
                    name: name.to_owned(),
                    files: Vec::new(),
                    volume: 1.0,
                    flags: 0,
                    min_distance: 0.0,
                    distance_cutoff: 0.0,
                    eax_def: 0,
                })
                .collect(),
        )
    }

    /// The catalog of `rows` in file order, with the two forced beds resolved once, as the
    /// reference does at init.
    fn from_kits(rows: Vec<SoundKit>) -> Self {
        let (ghost_bed, underwater_bed) = resolve_forced_beds(&rows);
        let mut kits = HashMap::with_capacity(rows.len());
        let mut by_name = HashMap::with_capacity(rows.len());
        for kit in rows {
            if !kit.name.is_empty() {
                by_name.insert(kit.name.to_ascii_lowercase(), kit.id);
            }
            kits.insert(kit.id, kit);
        }
        Self {
            kits,
            by_name,
            ghost_bed,
            underwater_bed,
        }
    }
}

/// `0x4609b0`: the ghost and underwater ambience rows, `(ghost, underwater)`. It walks the table
/// from the last row down, comparing each name with `0x64a480` at length `0x7fffffff`, which is
/// `strncmp` (`0x40de80`): exact, case-sensitive, the whole string with its terminator, so neither
/// a case variant nor a prefix matches (the ignore-case `0x64a4c0` is not used here). A match
/// stores the row's id, later rows down overwriting, and the walk ends when both are set.
fn resolve_forced_beds(rows: &[SoundKit]) -> (Option<u32>, Option<u32>) {
    let (mut ghost, mut underwater) = (None, None);
    for kit in rows.iter().rev() {
        if ghost.is_some() && underwater.is_some() {
            break;
        }
        if kit.name == GHOST_BED_NAME {
            ghost = Some(kit.id);
        } else if kit.name == UNDERWATER_BED_NAME {
            underwater = Some(kit.id);
        }
    }
    (ghost, underwater)
}

fn sound_entries_schema() -> Schema {
    let mut s = Schema::new("SoundEntries");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("SoundType", FieldType::UInt32));
    s.add_field(SchemaField::new("Name", FieldType::String));
    for i in 0..10 {
        s.add_field(SchemaField::new(format!("File{i}"), FieldType::String));
    }
    for i in 0..10 {
        s.add_field(SchemaField::new(format!("Freq{i}"), FieldType::UInt32));
    }
    s.add_field(SchemaField::new("DirectoryBase", FieldType::String));
    s.add_field(SchemaField::new("Volume", FieldType::Float32));
    s.add_field(SchemaField::new("Flags", FieldType::UInt32));
    s.add_field(SchemaField::new("MinDistance", FieldType::Float32));
    s.add_field(SchemaField::new("DistanceCutoff", FieldType::Float32));
    s.add_field(SchemaField::new("EAXDef", FieldType::UInt32));
    s
}

/// `DirectoryBase` and `File[i]` joined as the reference does (`0x45be10`, called only from
/// `0x45c167` in the `SOUNDDEFINITION` loader): `"%s%s%s"` over dir, separator and file, with no
/// separator when the dir is empty or already ends in `\`. Nothing below normalizes further: the
/// archive hash (`0x6549a0`) folds case and maps `/` to `\` but keeps leading and doubled
/// separators, and there is no loose-file fallback for a single leading `\`.
///
/// So the reference cannot play 27 of the 8961 shipped variations, and neither does this: 17 are
/// absent from the archives, and 10 are kit 8940 `Ashbringer`, whose `DirectoryBase` starts with
/// `\`. Stripping that separator would be a deviation, not a fix.
fn join_variation(dir: &str, file: &str) -> String {
    if dir.is_empty() || dir.ends_with('\\') {
        format!("{dir}{file}")
    } else {
        format!("{dir}\\{file}")
    }
}

/// Read `SoundEntries.dbc` off the patch chain.
pub fn load_sound_kit_catalog(chain: &mut Chain) -> Result<SoundKitCatalog> {
    let bytes = chain
        .read_file(SOUND_ENTRIES)
        .with_context(|| format!("reading {SOUND_ENTRIES}"))?;
    let rs = parse(&bytes, sound_entries_schema(), "SoundEntries")?;
    let mut rows = Vec::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let name = str_at(&rs, r, 2).unwrap_or_default();
        let dir = str_at(&rs, r, 23).unwrap_or_default();
        let mut files = Vec::new();
        for i in 0..10 {
            let Some(file) = str_at(&rs, r, 3 + i).filter(|f| !f.is_empty()) else {
                continue;
            };
            let weight = u32_at(r, 13 + i).unwrap_or(0);
            let path = join_variation(&dir, &file);
            files.push((path, weight));
        }
        rows.push(SoundKit {
            id,
            sound_type: u32_at(r, 1).unwrap_or(0),
            name,
            files,
            volume: f32_at(r, 24).unwrap_or(1.0),
            flags: u32_at(r, 25).unwrap_or(0),
            min_distance: f32_at(r, 26).unwrap_or(0.0),
            distance_cutoff: f32_at(r, 27).unwrap_or(0.0),
            eax_def: u32_at(r, 28).unwrap_or(0),
        });
    }
    Ok(SoundKitCatalog::from_kits(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_sound_entries_parse_and_resolve() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_sound_kit_catalog(&mut chain).expect("load sound kits");
        assert_eq!(cat.len(), 4623, "all 5875 SoundEntries rows load");

        // Kit 3, decoded by hand from the file's bytes.
        let kit = cat.get(3).expect("kit 3 exists");
        assert_eq!(kit.name, "Invisibility Impact");
        assert_eq!(kit.sound_type, 1);
        assert_eq!(kit.files.len(), 1);
        assert_eq!(
            kit.files[0],
            ("Sound\\Spells\\Dispel_Low_Base.wav".into(), 1)
        );
        assert_eq!(kit.volume, 1.0);
        assert_eq!(kit.flags, 0);
        assert_eq!(kit.min_distance, 8.0);
        assert_eq!(kit.distance_cutoff, 45.0);
        assert_eq!(kit.eax_def, 2);

        // The name lookup ignores case.
        let ui = cat.by_name("IGMINIMAPZOOMIN").expect("UI kit by name");
        assert_eq!(ui.id, 823);
        assert_eq!(ui.sound_type, 2, "type 2 = UI");

        let repair = cat.by_name("ITEM_REPAIR").expect("repair UI kit by name");
        assert_eq!(repair.id, 7994);
        assert_eq!(repair.flags & sound_kit_flags::NO_DUPLICATES, 0x20);

        // The joined path of a UI kit resolves to real bytes on the chain.
        let (path, _) = &ui.files[0];
        let bytes = chain.read(path).expect("kit file readable off the chain");
        assert!(
            bytes.len() > 1000,
            "{path} is a real WAV ({} B)",
            bytes.len()
        );
    }

    /// The two forced ambience rows, by the exact names `0x4609b0` compares: row 4160 and row 4209,
    /// where no row is named plain `Ghost` and 4123 `UnderWaterLoop` is a different row of the
    /// same file.
    #[test]
    fn real_sound_entries_resolve_the_forced_ambience_beds() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_sound_kit_catalog(&mut chain).expect("load sound kits");

        assert_eq!(cat.ghost_bed(), Some(4160));
        assert_eq!(cat.underwater_bed(), Some(4209));
        let file = |id: u32| cat.get(id).expect("row").files[0].0.clone();
        assert!(file(4160).ends_with("GhostState.wav"));
        assert!(file(4209).ends_with("UndwaterLoop.wav"));
        assert_eq!(cat.get(4123).expect("row").name, "UnderWaterLoop");
        assert_eq!(file(4123), file(4209), "the same file, its own row");

        // One row of each name, and none named plain `Ghost`.
        let count = |name: &str| cat.kits.values().filter(|k| k.name == name).count();
        assert_eq!(count("Ghost (DONOTRENAME)"), 1);
        assert_eq!(count("Underwater (DONOTRENAME)"), 1);
        assert!(cat.by_name("Ghost").is_none());
    }

    /// `0x64a480` at length `0x7fffffff` is `strncmp` over the whole string: a case variant, a
    /// prefix or an extension of the name is another row, and the ignore-case name registry does
    /// not decide it.
    #[test]
    fn the_forced_beds_match_the_exact_name_only() {
        let cat = SoundKitCatalog::named_for_tests(&[
            (1, "ghost (donotrename)"),
            (2, "Ghost (DONOTRENAME) "),
            (3, "Ghost"),
            (4, "Ghost (DONOTRENAM"),
            (5, "UNDERWATER (DONOTRENAME)"),
            (6, "Underwater (DONOTRENAME)x"),
            (7, ""),
        ]);
        assert_eq!((cat.ghost_bed(), cat.underwater_bed()), (None, None));
        assert_eq!(
            cat.by_name("GHOST").map(|k| k.id),
            Some(3),
            "PlaySoundByName folds case"
        );

        let cat = SoundKitCatalog::named_for_tests(&[
            (1, "ghost (donotrename)"),
            (9, "Ghost (DONOTRENAME)"),
            (10, "Underwater (DONOTRENAME)"),
        ]);
        assert_eq!((cat.ghost_bed(), cat.underwater_bed()), (Some(9), Some(10)));
    }

    /// The walk runs from the last row down and stops once both are set: a duplicate name is
    /// overwritten by earlier rows only while the other bed is still unfound.
    #[test]
    fn the_forced_bed_walk_runs_from_the_last_row_down() {
        let cat = SoundKitCatalog::named_for_tests(&[
            (1, "Ghost (DONOTRENAME)"),
            (2, "Underwater (DONOTRENAME)"),
            (3, "Ghost (DONOTRENAME)"),
            (4, "Underwater (DONOTRENAME)"),
        ]);
        assert_eq!(
            (cat.ghost_bed(), cat.underwater_bed()),
            (Some(3), Some(4)),
            "both set at rows 4 and 3, the walk ends"
        );

        let cat = SoundKitCatalog::named_for_tests(&[
            (1, "Underwater (DONOTRENAME)"),
            (2, "Ghost (DONOTRENAME)"),
            (3, "Ghost (DONOTRENAME)"),
        ]);
        assert_eq!(
            (cat.ghost_bed(), cat.underwater_bed()),
            (Some(2), Some(1)),
            "row 2 overwrites row 3 while the underwater bed is still unfound"
        );
    }

    /// The `EAXDef` census the reverb send is gated on: 0 is the reference's null slot, a channel
    /// that never gets reverb.
    #[test]
    fn real_sound_entries_eaxdef_census() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_sound_kit_catalog(&mut chain).expect("load sound kits");

        let mut n = [0usize; 3];
        for k in cat.kits.values() {
            assert!(k.eax_def <= 2, "kit {} has EAXDef {}", k.id, k.eax_def);
            n[k.eax_def as usize] += 1;
        }
        assert_eq!(
            (n[0], n[1], n[2]),
            (2072, 2, 2549),
            "the 5875 EAXDef census"
        );

        // NPC voice lines (`SoundType` 17, which `NPCSounds.dbc` names) are all dry, so NPCs in a
        // reverberant interior carry no echo.
        let voices: Vec<_> = cat.kits.values().filter(|k| k.sound_type == 17).collect();
        assert_eq!(voices.len(), 275, "the type-17 NPC voice rows");
        assert!(
            voices.iter().all(|k| k.eax_def == 0),
            "every NPC voice kit is authored dry"
        );

        // The control: creature barks mix wet and dry.
        let barks: Vec<_> = cat.kits.values().filter(|k| k.sound_type == 10).collect();
        assert!(
            barks.iter().any(|k| k.eax_def != 0) && barks.iter().any(|k| k.eax_def == 0),
            "creature barks split wet/dry"
        );
    }

    /// An empty directory emits the file alone (26 shipped rows, 1103 `WyvernWingFlap` among them,
    /// whose variations are full paths); a leading separator is kept, as the reference keeps it.
    #[test]
    fn the_variation_join_is_the_reference_s_separator_rule() {
        assert_eq!(
            join_variation("Sound\\Spells", "Dispel_Low_Base.wav"),
            "Sound\\Spells\\Dispel_Low_Base.wav"
        );
        assert_eq!(
            join_variation("Sound\\interface\\", "igNewTaxiNodeDiscovered.wav"),
            "Sound\\interface\\igNewTaxiNodeDiscovered.wav"
        );
        assert_eq!(
            join_variation("", "Sound\\Creature\\Wyvern\\WyvernWingFlap1.wav"),
            "Sound\\Creature\\Wyvern\\WyvernWingFlap1.wav"
        );
        assert_eq!(
            join_variation("\\Sound\\Creature\\Ashbringer\\", "ASH_SPEAK_01.wav"),
            "\\Sound\\Creature\\Ashbringer\\ASH_SPEAK_01.wav",
            "the leading separator survives — the reference emits it and misses too"
        );
    }

    /// Every kit path asked of the real chain: the reference's join resolves 8934 of 8961
    /// variations, and the 27 it misses are dead in the reference too. An unconditional separator
    /// would also silence kits 1519, 2988 and 3412, each one variation under a `DirectoryBase`
    /// that ends in `\`.
    #[test]
    fn real_sound_entries_paths_resolve_exactly_as_the_reference_s_do() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_sound_kit_catalog(&mut chain).expect("load sound kits");

        let mut total = 0usize;
        let mut dead: Vec<(u32, &str, &str)> = Vec::new();
        for kit in cat.kits.values() {
            for (path, _) in &kit.files {
                total += 1;
                if !chain.contains(path) {
                    dead.push((kit.id, &kit.name, path));
                }
            }
        }
        dead.sort_unstable();
        assert_eq!(total, 8961, "non-empty variation cells in the 5875 table");
        assert_eq!(
            total - dead.len(),
            8934,
            "variations that resolve — the binary's own count; {dead:?}"
        );

        // Ten belong to kit 8940, silent in the reference too.
        let ashbringer: Vec<_> = dead.iter().filter(|(id, ..)| *id == 8940).collect();
        assert_eq!(ashbringer.len(), 10, "every ASH_SPEAK line misses");
        assert!(
            chain.contains("Sound\\Creature\\Ashbringer\\ASH_SPEAK_01.wav"),
            "the asset ships — it is the authored path that cannot reach it"
        );

        // The other 17 are absent assets, in kits that mostly still play.
        assert_eq!(dead.len() - ashbringer.len(), 17, "absent assets: {dead:?}");

        // Only 8588, whose one variation is absent, and 8940 have nothing playable.
        let silent: Vec<u32> = cat
            .kits
            .values()
            .filter(|k| !k.files.is_empty() && !k.files.iter().any(|(p, _)| chain.contains(p)))
            .map(|k| k.id)
            .collect();
        let mut silent = silent;
        silent.sort_unstable();
        assert_eq!(
            silent,
            vec![8588, 8940],
            "the only kits with no playable variation at all"
        );
    }
}
