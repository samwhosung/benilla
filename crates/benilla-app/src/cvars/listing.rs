//! The reference's CVar registrations, `reference/cvars.tsv`, compiled in: the order the registry
//! is listed and walked in.
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

/// Lowercased name to its first registration's line index.
static ORDER: LazyLock<HashMap<String, usize>> = LazyLock::new(|| {
    let mut order = HashMap::new();
    for (i, line) in TABLE.lines().skip(1).enumerate() {
        let name = line.split('\t').nth(1).expect("cvars.tsv row: site, name");
        order.entry(name.to_ascii_lowercase()).or_insert(i);
    }
    order
});

/// A name's place in the list: the table's rows by their first registration, then every other
/// name after them.
pub(crate) fn list_rank(name: &str) -> usize {
    ORDER
        .get(&name.to_ascii_lowercase())
        .copied()
        .unwrap_or(usize::MAX)
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
    }
}
