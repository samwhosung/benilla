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

use benilla_ui::script::keybind::{normalize_binding_key, parse_bindings_wtf, KeyBinding};

/// A key list grouped by command, in first-appearance order; names compare case-insensitively.
fn by_command(list: &[KeyBinding]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (key, command) in list {
        match out
            .iter_mut()
            .find(|(c, _)| c.eq_ignore_ascii_case(command))
        {
            Some((_, keys)) => keys.push(key.clone()),
            None => out.push((command.clone(), vec![key.clone()])),
        }
    }
    out
}

/// Serialize a `(command, keys)` table (`stored_binding_snapshot`) as the diff file against the
/// defaults (`WTF\DefaultBindings.wtf`): a line for each command whose keys differ, including a
/// defaulted command the live table left unbound.
pub(crate) fn to_diff(snapshot: &[(String, Vec<String>)], defaults: &[KeyBinding]) -> String {
    let defaults = by_command(defaults);
    let default_of = |name: &str| {
        defaults
            .iter()
            .find(|(c, _)| c.eq_ignore_ascii_case(name))
            .map_or(&[][..], |(_, k)| k.as_slice())
    };
    let mut out = String::from("# benilla key bindings — diff vs defaults\n");
    let unlisted = defaults
        .iter()
        .filter(|(c, _)| !snapshot.iter().any(|(n, _)| n.eq_ignore_ascii_case(c)))
        .map(|(c, _)| (c.clone(), Vec::new()));
    for (name, keys) in snapshot.iter().cloned().chain(unlisted) {
        if default_of(&name) == keys.as_slice() {
            continue;
        }
        out.push_str("bind ");
        out.push_str(&name);
        for k in keys {
            out.push(' ');
            out.push_str(&k);
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

/// Resolve a diff into the full key table: the defaults, then each override in file order
/// replacing its command's keys and stealing each key from every other command, so one key always
/// maps to one command.
pub(crate) fn resolve(diff: &[(String, Vec<String>)], defaults: &[KeyBinding]) -> Vec<KeyBinding> {
    let mut table = defaults.to_vec();
    for (name, keys) in diff {
        table.retain(|(k, c)| !c.eq_ignore_ascii_case(name) && !keys.contains(k));
        table.extend(keys.iter().map(|k| (k.clone(), name.clone())));
    }
    table
}

/// `pairs` applied in order as the `.wtf` loader applies them (`0x4b6140` through `SetBinding`'s
/// `0x4b7490`): a key the validator refuses is dropped, a key bound again moves, and an empty
/// command unbinds it.
pub(crate) fn key_table(pairs: impl IntoIterator<Item = KeyBinding>) -> Vec<KeyBinding> {
    let mut table: Vec<KeyBinding> = Vec::new();
    for (key, command) in pairs {
        let Some(key) = normalize_binding_key(&key) else {
            continue;
        };
        table.retain(|(k, _)| k != &key);
        if !command.is_empty() {
            table.push((key, command));
        }
    }
    table
}

/// A stored set as the reference's `bindings-cache.wtf` holds it, the account-data section that
/// carries it: the whole key table, `bind <key> <command>` and CR LF per key, in the table's order
/// (`0x4b6520`, the format at `0x846e5c`).
pub(crate) fn to_cache(diff: &[(String, Vec<String>)], defaults: &[KeyBinding]) -> String {
    let mut out = String::new();
    for (key, command) in resolve(diff, defaults) {
        out.push_str("bind ");
        out.push_str(&key);
        out.push(' ');
        out.push_str(&command);
        out.push_str("\r\n");
    }
    out
}

/// A `bindings-cache.wtf` from the server as the diff file a set is kept in here, or `None` when it
/// binds no key: read as the reference's loader reads it ([`parse_bindings_wtf`], [`key_table`]),
/// grouped by command, then held against the defaults, so a command the file leaves unbound is
/// unbound here too.
pub(crate) fn from_cache(text: &str, defaults: &[KeyBinding]) -> Option<String> {
    let table = key_table(parse_bindings_wtf(text));
    if table.is_empty() {
        return None;
    }
    Some(to_diff(&by_command(&table), defaults))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Vec<KeyBinding> {
        [
            ("W", "MOVEFORWARD"),
            ("UP", "MOVEFORWARD"),
            ("Q", "STRAFELEFT"),
            ("SPACE", "JUMP"),
            ("NUMPAD0", "JUMP"),
            ("Z", "TOGGLESHEATH"),
        ]
        .into_iter()
        .map(|(k, c)| (k.to_string(), c.to_string()))
        .collect()
    }

    fn keys(table: &[KeyBinding], command: &str) -> Vec<String> {
        table
            .iter()
            .filter(|(_, c)| c == command)
            .map(|(k, _)| k.clone())
            .collect()
    }

    #[test]
    fn the_diff_round_trips_and_defaults_stay_silent() {
        // All defaults: a header-only file.
        let snapshot = by_command(&defaults());
        let text = to_diff(&snapshot, &defaults());
        assert_eq!(text.lines().count(), 1, "defaults produce no bind lines");

        // Move JUMP to F (unbinding SPACE/NUMPAD0), unbind TOGGLESHEATH entirely, and leave
        // STRAFELEFT out of the snapshot with no keys (a defaulted command no file declared).
        let mut moved: Vec<(String, Vec<String>)> = snapshot
            .into_iter()
            .filter(|(n, _)| n != "STRAFELEFT")
            .collect();
        moved.iter_mut().find(|(n, _)| n == "JUMP").unwrap().1 = vec!["F".into()];
        moved
            .iter_mut()
            .find(|(n, _)| n == "TOGGLESHEATH")
            .unwrap()
            .1 = vec![];
        let text = to_diff(&moved, &defaults());
        assert!(text.contains("bind JUMP F\n"));
        assert!(
            text.contains("bind TOGGLESHEATH\n"),
            "unbound = a bare line"
        );
        assert!(
            text.contains("bind STRAFELEFT\n"),
            "a defaulted command the snapshot lacks is unbound too"
        );
        assert!(
            !text.contains("MOVEFORWARD"),
            "untouched commands stay absent"
        );

        let resolved = resolve(&from_diff(&text), &defaults());
        assert_eq!(keys(&resolved, "JUMP"), ["F"]);
        assert!(keys(&resolved, "TOGGLESHEATH").is_empty());
        assert_eq!(keys(&resolved, "MOVEFORWARD"), ["W", "UP"]);
    }

    #[test]
    fn resolve_steals_across_commands_even_without_a_line_for_the_victim() {
        let resolved = resolve(&[("JUMP".to_string(), vec!["W".to_string()])], &defaults());
        assert_eq!(keys(&resolved, "MOVEFORWARD"), ["UP"]);
        assert_eq!(keys(&resolved, "JUMP"), ["W"]);
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
        let resolved = resolve(&parsed, &defaults());
        assert_eq!(keys(&resolved, "BOGUSCMD"), ["Q"]);
        assert!(
            keys(&resolved, "STRAFELEFT").is_empty(),
            "Q was stolen by the unknown command's line"
        );
    }

    /// The section is the whole table in the reference's line form: every default key not moved,
    /// then the overrides, each `bind <key> <command>` with CR LF (`0x846e5c`).
    #[test]
    fn a_set_goes_to_the_server_as_the_whole_table_in_the_cache_form() {
        let diff = from_diff("bind JUMP F\nbind TOGGLESHEATH\n");
        assert_eq!(
            to_cache(&diff, &defaults()),
            "bind W MOVEFORWARD\r\nbind UP MOVEFORWARD\r\nbind Q STRAFELEFT\r\nbind F JUMP\r\n"
        );
        assert_eq!(
            to_cache(&[], &defaults()).lines().count(),
            6,
            "an empty diff is the defaults, every one written"
        );
    }

    /// A cache from another client: its table read as the loader reads it, so a key bound twice
    /// keeps its last command, a refused key is dropped, and what it leaves unbound is unbound.
    #[test]
    fn a_cache_from_the_server_becomes_the_diff_of_its_whole_table() {
        let cache =
            "bind W MOVEFORWARD\r\nbind SPACE JUMP\r\nbind F JUMP\r\nbind F TOGGLESHEATH\r\n\
                     bind mousewheelup JUMP\r\nnot a line\r\n";
        let diff = from_cache(cache, &defaults()).expect("keys bound");
        let resolved = resolve(&from_diff(&diff), &defaults());
        assert_eq!(
            keys(&resolved, "MOVEFORWARD"),
            ["W"],
            "UP is unbound by its absence"
        );
        assert_eq!(
            keys(&resolved, "JUMP"),
            ["SPACE"],
            "F moved on to the later line"
        );
        assert_eq!(keys(&resolved, "TOGGLESHEATH"), ["F"]);
        assert!(keys(&resolved, "STRAFELEFT").is_empty());
        assert_eq!(
            to_cache(&from_diff(&diff), &defaults()),
            "bind W MOVEFORWARD\r\nbind SPACE JUMP\r\nbind F TOGGLESHEATH\r\n",
            "the same table goes back up"
        );
        assert_eq!(from_cache("", &defaults()), None);
        assert_eq!(from_cache("junk\r\n", &defaults()), None, "no key bound");
    }
}
