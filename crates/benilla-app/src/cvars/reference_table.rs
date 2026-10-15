//! The CVar table held to the reference's registrations (`reference/cvars.tsv`, one row per
//! `CVar::Register` call, `0x63db90`): a row's [`Reference`] claim is checked against the string
//! the client registers, as text. Names match ASCII case-insensitively, as `CVar::Lookup`
//! (`0x63de30`) does; a default the client computes at registration (`?` in the table) has no
//! string to compare.

use std::collections::BTreeMap;

use super::table::{all_rows, Reference};

/// `same` rows whose shipped default is not yet the registered string, each with the issue that
/// fixes it. The test fails on any other, and on a listed row that has come right.
const KNOWN: &[(&str, &str)] = &[
    ("MasterVolume", "#1278"),
    ("SoundVolume", "#1278"),
    ("cameraYawMoveSpeed", "#1278"),
    ("cameraPitchMoveSpeed", "#1278"),
    ("cameraDistanceMaxFactor", "#1278"),
    ("cameraYawSmoothSpeed", "#1278"),
    ("cameraPivotDYMin", "#1278"),
    ("cameraTargetSmoothSpeed", "#1278"),
    ("cameraTerrainTiltTimeMin", "#1278"),
    ("cameraTerrainTiltTimeMax", "#1278"),
    ("cameraBobbingLRAmplitude", "#1278"),
    ("cameraBobbingUDAmplitude", "#1278"),
    ("WorldDetail", "#1279"),
    ("Sound_EnableSoundWhenGameIsInBG", "#545"),
    ("gamma", "#1281"),
];

/// Every registered name, lower-cased, with the defaults the client registers it with (a
/// conditional default lists each arm).
fn registered() -> BTreeMap<String, Vec<String>> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../reference/cvars.tsv");
    let text = std::fs::read_to_string(path).expect("reference/cvars.tsv");
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        // site name name_address default default_address flags …
        assert!(f.len() >= 5, "malformed cvars.tsv row: {line:?}");
        if f[1].is_empty() {
            continue;
        }
        out.entry(f[1].to_ascii_lowercase())
            .or_default()
            .extend(f[3].split('|').map(str::to_string));
    }
    assert!(out.len() > 350, "cvars.tsv gave only {} names", out.len());
    out
}

/// A `same` row ships the string the reference registers, character for character; an
/// `overridden` or `deviates` row quotes it; no `same`, `overridden` or `deviates` row names a
/// CVar the reference does not register.
#[test]
fn the_reference_columns_hold_against_the_registered_cvars() {
    let reg = registered();
    let mut absent = Vec::new();
    let mut differs = Vec::new();
    let mut known_hit = Vec::new();
    for row in all_rows() {
        let name = row.name;
        let known = KNOWN.iter().find(|(n, _)| *n == name);
        let Some(defaults) = reg.get(&name.to_ascii_lowercase()) else {
            if !matches!(row.reference, Reference::Ours(_)) && known.is_none() {
                absent.push(format!("{name}: no CVar::Register names it"));
            }
            // An `ours` row is what the fix of an absent name makes it: its KNOWN entry has come right.
            if !matches!(row.reference, Reference::Ours(_)) {
                known_hit.extend(known.map(|_| name));
            }
            continue;
        };
        if defaults.iter().any(|d| d == "?") {
            continue;
        }
        // What an `overridden` row says the client registers, and what a `deviates` row says it
        // ships, is the registered string too.
        if let Reference::Overridden {
            registered: said, ..
        }
        | Reference::Deviates { value: said, .. } = &row.reference
        {
            if !defaults.iter().any(|d| d == said) {
                differs.push(format!(
                    "{name}: the row says the reference registers {said:?}, cvars.tsv has {defaults:?}"
                ));
            }
        }
        // A locale-dependent default is the reference's, arm for arm: the strings its register
        // site chooses between are exactly the base and the exceptions'.
        if let Reference::SameByLocale { except } = &row.reference {
            let mut ours: Vec<&str> = std::iter::once(row.default)
                .chain(except.iter().map(|(_, v)| *v))
                .collect();
            let mut theirs: Vec<&str> = defaults.iter().map(String::as_str).collect();
            ours.sort_unstable();
            ours.dedup();
            theirs.sort_unstable();
            theirs.dedup();
            if ours != theirs {
                differs.push(format!(
                    "{name}: ships {ours:?} across the locales, the reference registers {theirs:?}"
                ));
            }
        }
        if let Reference::Same(_) = row.reference {
            let right = defaults.iter().any(|d| d == row.default);
            match (right, known) {
                (false, None) => differs.push(format!(
                    "{name}: ships {:?}, the reference registers {defaults:?}",
                    row.default
                )),
                (true, Some((_, issue))) => differs.push(format!(
                    "{name}: listed under {issue} but now ships the registered string; take it off KNOWN"
                )),
                (false, Some(_)) => known_hit.push(name),
                (true, None) => {}
            }
        }
    }
    for (name, issue) in KNOWN {
        assert!(
            known_hit.contains(name),
            "KNOWN lists {name} ({issue}), which no longer fails; take it off"
        );
    }
    let mut all = absent;
    all.extend(differs);
    assert!(
        all.is_empty(),
        "{} row(s) against reference/cvars.tsv:\n  {}",
        all.len(),
        all.join("\n  ")
    );
}

/// Every script-visible `CVar::Register` of the reference is registered here: Lua's lookup
/// (`0x63dec0`) finds only a record with flag bit 31, and `GetCVar`, `SetCVar` and `GetCVarDefault`
/// raise on a name it misses (`0x488be7`, `0x488c53`, `0x488d37`). The `script_hidden` column is 1
/// for a variable Lua cannot see; `?` rows are the three sites whose arguments sit in a register
/// the generator does not follow, read from the image to push `esi` = 0 (`0x6039b5`-`0x6039ee`), or
/// registered elsewhere too (`showGameTips`, `realmName`).
#[test]
fn every_script_visible_reference_row_is_registered() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../reference/cvars.tsv");
    let text = std::fs::read_to_string(path).expect("reference/cvars.tsv");
    let mut visible = BTreeMap::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        let (name, hidden) = (f[1], f[8]);
        // `?` names are the `RegisterCVar` verb's own call (`0x488b8a`) and the console `set`
        // command's (`0x63d582`), which make a record per call.
        if name == "?" || hidden == "1" {
            continue;
        }
        visible.insert(name.to_ascii_lowercase(), name);
    }
    let ours: std::collections::BTreeSet<String> =
        all_rows().map(|r| r.name.to_ascii_lowercase()).collect();
    let missing: Vec<&str> = visible
        .iter()
        .filter(|(key, _)| !ours.contains(*key))
        .map(|(_, name)| *name)
        .collect();
    assert!(
        missing.is_empty(),
        "{} script-visible reference CVars are unregistered, so `GetCVar` and `SetCVar` raise on \
         them: {missing:?}",
        missing.len()
    );
    // And nothing is registered that the reference lacks, unless it is a documented `ours` row.
    let undocumented: Vec<&str> = all_rows()
        .filter(|r| !visible.contains_key(&r.name.to_ascii_lowercase()))
        .filter(|r| !matches!(r.reference, Reference::Ours(_)))
        .filter(|r| !KNOWN.iter().any(|(n, _)| *n == r.name))
        .map(|r| r.name)
        .collect();
    assert!(
        undocumented.is_empty(),
        "registered but not in reference/cvars.tsv, and neither an `ours` row nor on KNOWN: \
         {undocumented:?}"
    );
}
