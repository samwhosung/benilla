//! Every `ERR_*` key this workspace names must be a real row of the client's message catalog: an
//! unresolvable key shows nothing, and [`benilla_ui::messages`] reads the display surface from the
//! row. Source text is scanned, not a registry, so an unregistered key is still caught.

use std::collections::BTreeSet;
use std::path::Path;

/// Fixtures for the unknown-key fallback (`ui_petition::lines` and `benilla_ui::messages`'
/// `kind_of`), which need a key the client does not have.
const NOT_A_MESSAGE: &[&str] = &["ERR_SOMETHING_UNCARVED", "ERR_NOT_A_REAL_MESSAGE"];

/// The generated catalog, skipped: walking it would make every key trivially present.
const GENERATED: &str = "catalog.rs";

fn walk(dir: &Path, into: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            walk(&path, into);
        } else if path.extension().is_some_and(|e| e == "rs")
            && path.file_name().is_some_and(|n| n != GENERATED)
        {
            into.push(std::fs::read_to_string(&path).expect("read source"));
        }
    }
}

/// Every `"ERR_…"` string literal in a source file.
fn err_keys(src: &str) -> impl Iterator<Item = String> + '_ {
    src.match_indices("\"ERR_").filter_map(|(i, _)| {
        let rest = &src[i + 1..];
        let end = rest.find('"')?;
        let key = &rest[..end];
        key.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            .then(|| key.to_string())
    })
}

#[test]
fn every_error_key_in_the_source_is_a_catalog_row() {
    let mut sources = Vec::new();
    walk(Path::new("src"), &mut sources);
    walk(Path::new("../benilla-ui/src"), &mut sources);
    assert!(sources.len() > 100, "the walk found almost nothing to read");

    let keys: BTreeSet<String> = sources.iter().flat_map(|s| err_keys(s)).collect();
    // A floor proving the walk found the vocabulary, not a count to keep updated.
    assert!(
        keys.len() > 150,
        "expected the hand-written `ERR_*` vocabulary, found {}",
        keys.len()
    );

    let strays: Vec<&String> = keys
        .iter()
        .filter(|k| !NOT_A_MESSAGE.contains(&k.as_str()))
        .filter(|k| benilla_ui::messages::by_key(k).is_none())
        .collect();
    assert!(
        strays.is_empty(),
        "these keys are not rows of the 5875 message catalog, so the client would show nothing \
         for them: {strays:?}"
    );
}

/// A named key must also resolve to text: a row's key is a `GlobalStrings.lua` lookup, and the
/// shipped file has no entry for many rows, which the reference then never shows.
#[test]
fn every_error_key_in_the_source_resolves_to_real_text() {
    /// Keys the shipped `GlobalStrings.lua` has no string for, so the reference shows nothing:
    /// raised by `ui_items::equip_error` (errorId 362), `ui_action::cast_fail` (pet happiness)
    /// and `ui_pet::net` (errorId 337). `PET_SPELL_NOPATH` exists but is not row 337's key and
    /// nothing in the client raises it.
    const SILENT_IN_5875: &[&str] = &[
        "ERR_CANT_BE_DISENCHANTED",
        "ERR_NOT_HAPPY_ENOUGH",
        "ERR_PET_SPELL_NOPATH",
    ];

    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let vm = benilla_ui::script::UiScript::new().expect("VM");
    vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");

    let mut sources = Vec::new();
    walk(Path::new("src"), &mut sources);
    walk(Path::new("../benilla-ui/src"), &mut sources);
    let keys: BTreeSet<String> = sources.iter().flat_map(|s| err_keys(s)).collect();

    let mut resolved = 0;
    let silent: Vec<&String> = keys
        .iter()
        .filter(|k| !NOT_A_MESSAGE.contains(&k.as_str()))
        .filter(|k| !SILENT_IN_5875.contains(&k.as_str()))
        .filter(|k| {
            let text: String = vm.lua().globals().get(k.as_str()).unwrap_or_default();
            resolved += usize::from(!text.is_empty());
            text.is_empty()
        })
        .collect();
    assert!(
        silent.is_empty(),
        "these keys are catalog rows but resolve to NOTHING in the shipped GlobalStrings.lua, so \
         every line raised through them is invisible: {silent:?}"
    );
    // A floor proving the VM loaded the file.
    assert!(
        resolved > 150,
        "only {resolved} keys resolved — did GlobalStrings load?"
    );
}

/// 56 catalog rows carry an error-speech id at `+0x0c` instead of a cue name, spoken in the
/// player's race and sex through `type_tag`, `VocalUISounds.dbc(race, line)` and
/// `SoundEntries(sex)`. Every voiced key raised here must resolve audio for every playable race
/// and both sexes, except the gaps in the shipped data listed below.
#[test]
fn every_voiced_key_the_source_raises_has_audio_for_every_playable_race() {
    use benilla_formats::VOCAL_UI_LINES;

    /// Lines with no audio in any race, `0x07` (`ERR_FOOD_COOLDOWN`) and `0x20`
    /// (`ERR_LOOT_BAD_FACING`): every race's `VocalUISounds` row has kit id -1.
    const NO_AUDIO_IN_5875: &[u8] = &[0x07, 0x20];
    /// Single `(line, race, sex)` gaps in the shipped file.
    const RACE_SEX_GAPS: &[(u8, u32, u32)] = &[
        (0x21, 3, 0), // ERR_LOOT_LOCKED, Dwarf male
        (0x21, 4, 0), // ERR_LOOT_LOCKED, Night Elf male
        (0x31, 2, 0), // ERR_MUST_EQUIP_ITEM, Orc male
    ];

    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let vocal = benilla_formats::load_vocal_ui_sounds(&mut chain).expect("VocalUISounds.dbc");
    let kits = benilla_formats::load_sound_kit_catalog(&mut chain).expect("SoundEntries.dbc");

    let mut sources = Vec::new();
    walk(Path::new("src"), &mut sources);
    walk(Path::new("../benilla-ui/src"), &mut sources);
    let keys: BTreeSet<String> = sources.iter().flat_map(|s| err_keys(s)).collect();

    let mut voiced = 0;
    for key in &keys {
        let Some(record) = benilla_ui::messages::by_key(key) else {
            continue; // the walk above is what polices this
        };
        let tag = record.type_tag;
        if usize::from(tag) >= VOCAL_UI_LINES {
            continue; // a cue row, not a spoken one
        }
        voiced += 1;
        if NO_AUDIO_IN_5875.contains(&tag) {
            continue;
        }
        for race in 1..=8u32 {
            let row = vocal
                .rows()
                .iter()
                .find(|r| r.race == race && r.line == u32::from(tag))
                .unwrap_or_else(|| panic!("{key} (line {tag:#04x}) has no row for race {race}"));
            for sex in 0..2u32 {
                if RACE_SEX_GAPS.contains(&(tag, race, sex)) {
                    continue;
                }
                let kit = row
                    .normal_kit(sex)
                    .and_then(|k| kits.get(k))
                    .unwrap_or_else(|| {
                        panic!("{key} (line {tag:#04x}) is silent for race {race} sex {sex}")
                    });
                assert!(
                    !kit.files.is_empty(),
                    "{key}: kit {} has no files for race {race} sex {sex}",
                    kit.id
                );
            }
        }
    }
    // A floor over the 45 distinct voiced lines, so raise sites that stop naming them fail here.
    assert!(
        voiced >= 30,
        "only {voiced} voice-tagged keys reached the catalog — the raise sites stopped naming them"
    );
}

/// `CGGameUI::DisplayError` (`0x496720`) sounds (`0x49673d`) before it guards on the key
/// (`0x4967bd`/`0x4967c5`) and the text (`0x4945b4`); benilla drops an empty line before it
/// sounds, which matches only while every sounding row has text. If this fails,
/// `sound::message` must sound independently of the display.
#[test]
fn every_sounding_catalog_row_also_has_text_to_show() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let vm = benilla_ui::script::UiScript::new().expect("VM");
    vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");

    let mut sounding = 0;
    for r in benilla_ui::messages::CATALOG {
        let sounds = usize::from(r.type_tag) < benilla_formats::VOCAL_UI_LINES || r.sound.is_some();
        if !sounds {
            continue;
        }
        sounding += 1;
        let text: String = vm.lua().globals().get(r.key).unwrap_or_default();
        assert!(
            !text.is_empty(),
            "{} sounds (tag {:#04x}, cue {:?}) but has no 1.12 string — benilla would be silent \
             where the reference is not",
            r.key,
            r.type_tag,
            r.sound
        );
    }
    assert_eq!(sounding, 86, "56 voice lines + 30 named cues");
}

/// Every `PETTAME_*` key `pet_tame_failure_key` returns, the fill for `ERR_TAME_FAILED`, is a 1.12
/// global, checked against the in-tree `reference/1.12-globals.tsv`.
#[test]
fn every_pettame_key_is_a_real_1_12_global() {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/1.12-globals.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("the reference surface");
    let globals: BTreeSet<&str> = text
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split('\t').next())
        .collect();

    // 1..=11 index the table; everything else is the default arm.
    let reachable: BTreeSet<&str> = (0..=u8::MAX)
        .map(benilla_protocol::messages::pet_tame_failure_key)
        .collect();
    assert_eq!(reachable.len(), 12, "eleven arms plus the default");
    for key in &reachable {
        assert!(globals.contains(key), "{key} is not a 1.12 global");
    }
    // The message the fill goes into, and the two bodiless arms beside it.
    for key in ["ERR_TAME_FAILED", "ERR_INVALID_PETNAME", "ERR_PET_BROKEN"] {
        assert!(globals.contains(key), "{key} is not a 1.12 global");
    }
}
