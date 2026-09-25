//! Binding persistence: `benilla-config/bindings/account.txt` and
//! `benilla-config/bindings/<Realm>-<Char>.txt`, through [`crate::local_state`] like every resident.
//!
//! Format is **command-centric diff-vs-defaults** — one line per command whose keys moved:
//!
//! ```text
//! # benilla key bindings
//! bind JUMP F
//! bind MOVEFORWARD W
//! bind TOGGLESHEATH
//! ```
//!
//! `bind <COMMAND> [key...]` replaces that command's whole key list; no tokens = deliberately
//! unbound; a command absent from the file = its registered defaults. This diverges from the
//! reference's full-snapshot `bindings-cache.wtf` deliberately (recorded in 0997): benilla grows
//! commands weekly, and a wholesale snapshot would ship every new command unbound to anyone with
//! a saved file. Tokens are the canonical chord strings, which never contain spaces, so
//! whitespace splitting is safe (`-` and `=` are key tokens, hence no sentinel for "empty" —
//! absence of tokens is the sentinel).

use std::collections::HashMap;

use super::commands::SPECS;

/// Serialize the live table as the diff file. `snapshot` is `(command, keys)` in registry order
/// ([`benilla_ui::script::UiScript::keybind_snapshot`]'s shape).
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

/// Parse a diff file into `(command, keys)` overrides — the shape
/// [`benilla_ui::script::UiScript::seed_binding_set`] takes. Unknown commands are kept (a file
/// from a newer build stays intact through load/save); malformed lines are skipped.
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

/// Resolve a diff into the **full** per-command key table (defaults with the overrides applied,
/// steal law honored file-order) — what a stored set seeds. The steal pass matters: a file that
/// binds `W` to JUMP must also strip `W` from MOVEFORWARD even if MOVEFORWARD has no line (older
/// file, new default collision) — one key, one command, always.
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
            // **A name that is not in SPECS is an ADDON's command, and it is kept** (decision
            // 1201). Dropping it here is what made an addon binding forget its key every restart:
            // the row was written by `to_diff`, read back by `from_diff`, had its chord stolen
            // from whoever else held it by the pass above — and was then thrown away, so the
            // binding registered at world entry with nothing to restore. `Bindings.txt` is a flat
            // list in the reference too; an addon's row is a row like any other.
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
        // All-defaults snapshot → header-only file.
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
        // A file that binds W to JUMP: MOVEFORWARD keeps UP but loses W, with no MOVEFORWARD
        // line in the file at all.
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
        // **A command the registry does not know is KEPT**. It used to be
        // dropped after its keys were stolen, which was right for a genuinely bogus line and
        // catastrophic for the case that actually occurs: an ADDON's command, whose
        // `Bindings.xml` registers at world entry, hours after this file was read. Dropping it
        // meant the binding registered with nothing to restore and forgot the player's chord
        // every restart (1192 §4). The steal still happens either way, so the file's intent
        // ("Q belongs to that command, not STRAFELEFT") holds for the commands we do have.
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
