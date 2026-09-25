//! Every `ERR_*` key this workspace names must be a real row of the client's message catalog.
//!
//! benilla writes these keys as string literals at ~160 call sites, and each one used to arrive by
//! hand-tracing a `push <id>; call CGGameUI::DisplayError` in the binary. A typo, a key from a
//! later expansion, or a plausible-looking invention would all have been invisible: the string
//! simply would not resolve in `GlobalStrings.lua`, and the reference's own data-suppression rule
//! means benilla shows *nothing* for an unresolvable key. Silence is the failure mode, which is the
//! worst kind — nobody files a bug about a toast they never saw.
//!
//! Since decision 1770 the surface those keys are shown on is read from
//! [`benilla_ui::messages`] rather than carried beside them, so a key that is not in the catalog
//! also silently takes the fallback. This walk is what keeps that fallback unreachable: it is the
//! tripwire that turns "we believe these are message ids" into a checked claim.
//!
//! It reads source text rather than any registry the code keeps, deliberately — a registry could
//! only contain what somebody remembered to register.

use std::collections::BTreeSet;
use std::path::Path;

/// Not message ids, and not meant to be — both are fixtures for the unknown-key *fallback*, which
/// needs a key the client does not have in order to be tested at all: `ui_petition::lines`'
/// degrade-to-generic arm, and `benilla_ui::messages`' own proof that `kind_of` answers for a key
/// it has never heard of. Anything else appearing here is a defect, not an entry to add.
const NOT_A_MESSAGE: &[&str] = &["ERR_SOMETHING_UNCARVED", "ERR_NOT_A_REAL_MESSAGE"];

/// The generated table itself, which is the answer and not a question — walking it would make
/// every key trivially present and the check vacuous.
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

/// Pull every `"ERR_…"` string literal out of a source file, the way the call sites write them.
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
    // 163 at the time of writing — the point is only that the walk found the vocabulary and not
    // a handful of files, so this is a floor and not a ledger to keep updated.
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

/// **The claim this file's own doc has always made, finally checked**: a key the
/// source names must not only be a catalog row, it must **resolve to text**.
///
/// The two are not the same, and the difference is the failure this file exists to catch. A row's
/// `key` is a `GlobalStrings.lua` *lookup*, and the shipped file has no entry for a good many of
/// them — that absence is the reference's own data-suppression face, and a raise site that picks
/// one of those rows is silent for good. The walk above would pass it.
///
/// Worth closing now because 1821 moved four more windows' refusals onto keys (the vendor's two
/// tables, the banker's, the mailbox's), where a superseded or mistyped key costs a whole window's
/// worth of lines rather than one toast. Skips without client data.
#[test]
fn every_error_key_in_the_source_resolves_to_real_text() {
    /// Keys whose row the shipped `GlobalStrings.lua` has **no string for**, so the reference
    /// itself shows nothing when it raises them. Each is documented where it is raised —
    /// `ui_items::equip_error` (errorId 362), `ui_action::cast_fail` (the pet-happiness arm), and
    /// `ui_pet::net` (errorId 337, both pet handlers' NOPATH arm). A fourth entry here is a
    /// defect until someone proves otherwise.
    ///
    /// `ERR_PET_SPELL_NOPATH` is the one worth reading twice, because it is a trap: 5875 *does*
    /// ship a `PET_SPELL_NOPATH` ("No path available for your pet"), one letter-group away and
    /// perfectly plausible on screen — but it is not row 337's key and no code path in the client
    /// raises it. benilla named it for months and showed the wrong string for the wrong byte.
    /// Silence here is the reference's behaviour, not a gap.
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
    // A floor, not a ledger: the point is that the VM really loaded the file, so an empty `vm`
    // cannot make the sweep above vacuously green.
    assert!(
        resolved > 150,
        "only {resolved} keys resolved — did GlobalStrings load?"
    );
}

/// **The other half of the same claim, on real data: a key that carries a VOICE line can actually
/// be spoken**.
///
/// 56 catalog rows put an error-speech id in `+0x0c` instead of a cue name, and the client says
/// those aloud in the player's own race and gender. The join runs
/// `key → MessageRecord::type_tag → VocalUISounds.dbc(race, line) → SoundEntries(sex)`, across
/// four tables and two crates, and every link is a lookup that can come back empty *silently* —
/// the same failure mode the walk above exists for, one layer deeper.
///
/// So: for every voice-tagged key this workspace raises, every playable race must have a row, and
/// that row must resolve real audio for both sexes. The two exceptions are exceptions **in the
/// shipped data**, named here so a regression cannot hide behind them. Skips without client data.
#[test]
fn every_voiced_key_the_source_raises_has_audio_for_every_playable_race() {
    use benilla_formats::VOCAL_UI_LINES;

    /// Lines the 5875 data ships no audio for, in any race: `0x07` (`ERR_FOOD_COOLDOWN`) and
    /// `0x20` (`ERR_LOOT_BAD_FACING`). Both have a `VocalUISounds` row per race whose kit ids are
    /// `-1` — the file's own "no line recorded" — so the client is silent for them by data.
    const NO_AUDIO_IN_5875: &[u8] = &[0x07, 0x20];
    /// Single `(race, sex)` gaps in the shipped file — one voice actor's line that never got
    /// recorded. Listed rather than tolerated wholesale, so a *new* hole fails the test.
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
    // benilla raises a good share of the 45 distinct voiced lines — a floor, so a refactor that
    // quietly stopped routing refusals through the catalog fails here instead of going silent.
    assert!(
        voiced >= 30,
        "only {voiced} voice-tagged keys reached the catalog — the raise sites stopped naming them"
    );
}

/// **Why benilla may hang a message's sound off its display, even though the reference does not.**
///
/// `CGGameUI::DisplayError 0x496720` sounds first (`0x49673d`) and only then guards on the row's
/// key (`0x4967bd`/`0x4967c5`) and, in the sink, on the resolved text (`0x4945b4`) — so *there*, a
/// message with no GlobalStrings string still makes its noise. benilla drops an empty line before
/// it reaches `show_messages`, so it would not. That is a divergence with **no case in 5875's
/// data**, and this is the measurement that says so: every row that sounds also has text.
///
/// If this ever fails, `sound::message` has to sound the row independently of the display rather
/// than alongside it. Skips without client data.
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

/// **The non-`ERR_` half of the same tripwire, for the one family that has one**:
/// every string `pet_tame_failure_key` can return must be a real global of the 1.12 client.
///
/// The walk above collects `"ERR_…"` literals, which is every catalog row — and is therefore
/// blind to a raise site that names something else. `SMSG_PET_TAME_FAILURE` is precisely that
/// shape: the message it raises is `ERR_TAME_FAILED`, but the *text* comes from a second lookup
/// of a `PETTAME_*` key, and an invented one there would fail exactly the way this file's header
/// describes — silently, with no line shown and nobody to file it.
///
/// Checked against `reference/1.12-globals.tsv`, the running client's own `_G`,
/// rather than against the install: it is in-tree, so this holds on a machine with no client.
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

    // The whole reachable image of the table: 1..=11 index it, everything else is the default
    // arm. `u8` is small enough to just walk.
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
