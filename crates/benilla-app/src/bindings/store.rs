//! Binding persistence in `benilla-config/bindings/account.txt` and `<Realm>-<Char>.txt`: one
//! line per command whose keys differ from its defaults.
//!
//! ```text
//! # benilla key bindings
//! bind JUMP F
//! bind MOVEFORWARD W
//! bind TOGGLESHEATH
//! ```
//!
//! `bind <COMMAND> [key...]` replaces the command's key list; no tokens means unbound, an absent
//! command keeps its defaults. Tokens are canonical chord strings and never contain spaces.
//! Deviation: the reference saves a full snapshot (`bindings-cache.wtf`); a diff is stored
//! because a full snapshot would leave every command added later unbound for a saved file.

use std::collections::HashMap;

use super::commands::SPECS;

/// Serialize the live `(command, keys)` table, in registry order, as the diff file.
pub(crate) fn to_diff(snapshot: &[(String, Vec<String>)]) -> String {
    let defaults: HashMap<&str, Vec<&str>> = SPECS
        .iter()
        .map(|s| (s.name, [s.d1, s.d2].into_iter().flatten().collect()))
        .collect();
    let mut out = String::from("# benilla key bindings (decision 0997) — diff vs defaults\n");
    for (name, keys) in snapshot {
        let is_default = defaults
            .get(name.as_str())
            .is_some_and(|d| d.iter().copied().eq(keys.iter().map(String::as_str)));
        if is_default {
            continue;
        }
        out.push_str("bind ");
        out.push_str(name);
        for k in keys {
            out.push(' ');
            out.push_str(k);
        }
        out.push('\n');
    }
    out
}

/// Parse a diff file into `(command, keys)` overrides; unknown commands are kept, malformed lines
/// skipped.
pub(crate) fn from_diff(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        if it.next() != Some("bind") {
            continue;
        }
        let Some(name) = it.next() else { continue };
        out.push((
            name.to_ascii_uppercase(),
            it.map(|t| t.to_ascii_uppercase()).collect(),
        ));
    }
    out
}

/// Resolve a diff into the full key table: defaults, then each override in file order stealing
/// its keys from every other command, so one key always maps to one command.
pub(crate) fn resolve(diff: &[(String, Vec<String>)]) -> Vec<(String, Vec<String>)> {
    let mut table: Vec<(String, Vec<String>)> = SPECS
        .iter()
        .map(|s| {
            (
                s.name.to_string(),
                [s.d1, s.d2]
                    .into_iter()
                    .flatten()
                    .map(str::to_owned)
                    .collect(),
            )
        })
        .collect();
    for (name, keys) in diff {
        // Steal each key from wherever it currently sits, then install the command's list.
        for k in keys {
            for (_, held) in table.iter_mut() {
                held.retain(|h| h != k);
            }
        }
        match table.iter_mut().find(|(n, _)| n == name) {
            Some((_, slot)) => *slot = keys.clone(),
            // A name not in SPECS is an addon's command, registered later at world entry: keep
            // its row so the binding has its keys to restore.
            None => table.push((name.clone(), keys.clone())),
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_diff_round_trips_and_defaults_stay_silent() {
        // All defaults: a header-only file.
        let snapshot: Vec<(String, Vec<String>)> = SPECS
            .iter()
            .map(|s| {
                (
                    s.name.to_string(),
                    [s.d1, s.d2]
                        .into_iter()
                        .flatten()
                        .map(str::to_owned)
                        .collect(),
                )
            })
            .collect();
        let text = to_diff(&snapshot);
        assert_eq!(text.lines().count(), 1, "defaults produce no bind lines");

        // Move JUMP to F (unbinding SPACE/NUMPAD0), unbind TOGGLESHEATH entirely.
        let mut moved = snapshot.clone();
        moved.iter_mut().find(|(n, _)| n == "JUMP").unwrap().1 = vec!["F".into()];
        moved
            .iter_mut()
            .find(|(n, _)| n == "TOGGLESHEATH")
            .unwrap()
            .1 = vec![];
        let text = to_diff(&moved);
        assert!(text.contains("bind JUMP F\n"));
        assert!(
            text.contains("bind TOGGLESHEATH\n"),
            "unbound = a bare line"
        );
        assert!(
            !text.contains("MOVEFORWARD"),
            "untouched commands stay absent"
        );

        let parsed = from_diff(&text);
        let resolved = resolve(&parsed);
        let get = |n: &str| {
            resolved
                .iter()
                .find(|(name, _)| name == n)
                .map(|(_, k)| k.clone())
                .unwrap()
        };
        assert_eq!(get("JUMP"), vec!["F".to_string()]);
        assert!(get("TOGGLESHEATH").is_empty());
        assert_eq!(get("MOVEFORWARD"), vec!["W".to_string(), "UP".to_string()]);
    }

    #[test]
    fn resolve_steals_across_commands_even_without_a_line_for_the_victim() {
        let resolved = resolve(&[("JUMP".to_string(), vec!["W".to_string()])]);
        let fwd = &resolved.iter().find(|(n, _)| n == "MOVEFORWARD").unwrap().1;
        assert_eq!(fwd, &vec!["UP".to_string()]);
        let jump = &resolved.iter().find(|(n, _)| n == "JUMP").unwrap().1;
        assert_eq!(jump, &vec!["W".to_string()]);
    }

    #[test]
    fn comments_junk_and_case_survive_parsing() {
        let parsed = from_diff("# header\n\nbind jump f\nnot-a-line\nbind BOGUSCMD Q\n");
        assert_eq!(
            parsed,
            vec![
                ("JUMP".to_string(), vec!["F".to_string()]),
                ("BOGUSCMD".to_string(), vec!["Q".to_string()]),
            ]
        );
        // An unknown command (an addon's, registered later) is kept, and still steals its key.
        let resolved = resolve(&parsed);
        let bogus = &resolved.iter().find(|(n, _)| n == "BOGUSCMD").unwrap().1;
        assert_eq!(bogus, &vec!["Q".to_string()]);
        let strafe = &resolved.iter().find(|(n, _)| n == "STRAFELEFT").unwrap().1;
        assert!(
            strafe.is_empty(),
            "Q was stolen by the unknown command's line"
        );
    }
}
