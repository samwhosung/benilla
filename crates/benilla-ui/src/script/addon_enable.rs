//! One character's addon enable hash, the table an `ADDONSTATELIST` node (`0xbe1bd0`) holds at
//! `+0x10`, with the node's dirty byte (`+0xc`). Both the glue AddOns screen and the in-game verbs
//! write through its setter, and the writer `0x51ef20` saves a node only while it is dirty, one
//! line per entry, so a character that toggled nothing writes nothing and one that never set an
//! addon keeps inheriting what the realm's other characters chose (`0x51e470`).

use std::collections::HashMap;

/// A character's explicit enable rows: those `AddOnList_LoadCharacter 0x51ebe0` read from its
/// `AddOns.txt`, whether or not that addon is installed now, plus those the setter `0x51ea20` put
/// there since. An addon with no row here is not the character's choice, and the query answers it
/// from the other characters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnableHash {
    /// In insertion order, each name in the spelling that first put it here.
    rows: Vec<(String, bool)>,
    /// Lowercased name to its row: every compare in the reference is `SStrCmpI` (`0x64a4c0`).
    index: HashMap<String, usize>,
    /// The node's `+0xc`: set when a setter call changes the hash (`0x51ebbc`), cleared by a
    /// reload (`0x51ec59`) and by the writer as it saves (`0x51ef68`).
    dirty: bool,
}

impl EnableHash {
    /// The loader's hash, clean. A name on two lines keeps its first spelling and takes the last
    /// value, as the reload looks each one up and overwrites its byte (`0x51ee31`, `0x51eeef`).
    pub fn from_rows(rows: impl IntoIterator<Item = (String, bool)>) -> Self {
        let mut hash = Self::default();
        for (name, on) in rows {
            hash.put(name, on);
        }
        hash
    }

    /// The row for `name`, if this character has one.
    pub fn get(&self, name: &str) -> Option<bool> {
        self.index
            .get(&name.to_ascii_lowercase())
            .map(|&i| self.rows[i].1)
    }

    /// The setter `0x51ea20` on this node: a row with the same value is left alone
    /// (`0x51ebab`-`0x51ebb1`); a changed row is stored and a missing one inserted, and either
    /// dirties the node (`0x51ebb3`-`0x51ebbc`, `0x51eb62`).
    pub fn set(&mut self, name: &str, on: bool) {
        if self.get(name) == Some(on) {
            return;
        }
        self.put(name.to_string(), on);
        self.dirty = true;
    }

    /// Whether the writer would save this node.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Every row, in order.
    pub fn rows(&self) -> &[(String, bool)] {
        &self.rows
    }

    /// The writer's view of the node (`0x51ef59`-`0x51ef68`): every row when dirty, clearing the
    /// flag; `None` when clean, which writes nothing.
    pub fn take_dirty(&mut self) -> Option<Vec<(String, bool)>> {
        std::mem::take(&mut self.dirty).then(|| self.rows.clone())
    }

    fn put(&mut self, name: String, on: bool) {
        match self.index.get(&name.to_ascii_lowercase()) {
            Some(&i) => self.rows[i].1 = on,
            None => {
                self.index
                    .insert(name.to_ascii_lowercase(), self.rows.len());
                self.rows.push((name, on));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EnableHash;

    fn loaded() -> EnableHash {
        EnableHash::from_rows([("Kept".to_string(), true), ("Off".to_string(), false)])
    }

    #[test]
    fn a_loaded_hash_is_clean_and_keeps_the_first_spelling_with_the_last_value() {
        let h = EnableHash::from_rows([
            ("MyAddon".to_string(), true),
            ("myaddon".to_string(), false),
        ]);
        assert!(!h.is_dirty());
        assert_eq!(h.rows(), &[("MyAddon".to_string(), false)]);
        assert_eq!(h.get("MYADDON"), Some(false));
    }

    #[test]
    fn setting_a_row_to_its_own_value_does_not_dirty() {
        let mut h = loaded();
        h.set("kept", true);
        h.set("OFF", false);
        assert!(!h.is_dirty());
        assert_eq!(h.clone().take_dirty(), None, "a clean node writes nothing");
    }

    #[test]
    fn a_changed_row_dirties_and_the_writer_takes_every_row_once() {
        let mut h = loaded();
        h.set("Kept", false);
        assert!(h.is_dirty());
        assert_eq!(
            h.take_dirty(),
            Some(vec![
                ("Kept".to_string(), false),
                ("Off".to_string(), false)
            ])
        );
        assert!(!h.is_dirty(), "the writer clears the flag (`0x51ef68`)");
        assert_eq!(h.take_dirty(), None);
    }

    /// A miss inserts even when the value matches what the character inherited: the setter has
    /// no view of the aggregate, only of this node's table.
    #[test]
    fn a_missing_row_is_inserted_and_dirties() {
        let mut h = loaded();
        h.set("New", true);
        assert!(h.is_dirty());
        assert_eq!(h.get("new"), Some(true));
        assert_eq!(h.rows().len(), 3);
    }
}
