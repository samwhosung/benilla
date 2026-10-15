//! The CVar table held to the reference's registrations (`reference/cvars.tsv`, one row per
//! `CVar::Register` call, `0x63db90`): a row's [`Reference`] claim is checked against the string
//! the client registers, as text. Names match ASCII case-insensitively, as `CVar::Lookup`
//! (`0x63de30`) does; a default the client computes at registration (`?` in the table) has no
//! string to compare.

use std::collections::BTreeMap;

use super::table::{Reference, REGISTERED};

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
    for row in REGISTERED {
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
