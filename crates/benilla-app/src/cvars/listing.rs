//! The reference's CVar registrations, `reference/cvars.tsv`, compiled in: the order the registry
//! is listed and walked in, and each CVar's console category and help, which its registration
//! hands its command (`0x63ddc9`).
//!
//! Deviation: the reference's list runs in creation order, the records `Config.wtf` created first
//! in file order (`0x63d8eb`), then each registration as start-up reaches it (`containers`), an
//! order benilla-re has not traced. benilla lists in the table's order, which is register-site
//! address order, then the rows the table lacks (benilla's own, an addon's) as they registered.

use std::collections::HashMap;
use std::sync::LazyLock;

/// `reference/cvars.tsv`: a header, then one registration per line, by site address.
const TABLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../reference/cvars.tsv"
));

/// One name's first registration.
struct Registration {
    /// Its line index, the list rank.
    rank: usize,
    /// The `category` column.
    category: u8,
    /// The `help` column; empty for none.
    help: &'static str,
}

/// Lowercased name to its first registration.
static REGISTRATIONS: LazyLock<HashMap<String, Registration>> = LazyLock::new(|| {
    let mut out = HashMap::new();
    for (rank, line) in TABLE.lines().skip(1).enumerate() {
        let f: Vec<&'static str> = line.split('\t').collect();
        assert!(f.len() >= 12, "malformed cvars.tsv row: {line:?}");
        out.entry(f[1].to_ascii_lowercase())
            .or_insert_with(|| Registration {
                rank,
                category: f[7].parse().expect("cvars.tsv category"),
                help: f[11],
            });
    }
    out
});

/// A name's place in the list: the table's rows by their first registration, then every other
/// name after them.
pub(crate) fn list_rank(name: &str) -> usize {
    REGISTRATIONS
        .get(&name.to_ascii_lowercase())
        .map_or(usize::MAX, |r| r.rank)
}

/// A name's console category and help, as its registration passes them; a name the table lacks
/// (benilla's own, an addon's) has category 9, which `help` names none of (`0x488b81`, an addon's
/// `RegisterCVar`), and no help.
pub(crate) fn reference_help(name: &str) -> (u8, Option<&'static str>) {
    REGISTRATIONS
        .get(&name.to_ascii_lowercase())
        .map_or((9, None), |r| {
            (r.category, Some(r.help).filter(|h| !h.is_empty()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table's first rows lead, a later site ranks after an earlier one, and a name the table
    /// lacks ranks last.
    #[test]
    fn the_list_follows_the_tables_order() {
        assert_eq!(list_rank("disableOptionalSpeech"), 0);
        assert!(list_rank("M2UseZFill") < list_rank("accountName"));
        assert_eq!(list_rank("ACCOUNTNAME"), list_rank("accountName"));
        assert_eq!(list_rank("noSuchCvarAnywhere"), usize::MAX);
        assert_eq!(
            reference_help("accountName"),
            (4, Some("Saved account name"))
        );
        assert_eq!(reference_help("noSuchCvarAnywhere"), (9, None));
    }
}
