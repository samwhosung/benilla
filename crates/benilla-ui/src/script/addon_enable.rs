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

/// Where a character's `AddOns.txt` lives and how it is read and written, the host's: the loader
/// `0x51ebe0` opens `WTF\Account\<account>\<realm>\<character>\AddOns.txt` and the writer
/// `0x51ef20` creates it afresh.
pub trait AddonStateFiles {
    /// The rows of `realm`/`character`'s file in file order, `None` when it cannot be opened
    /// (`0x51ec9d` falls to the legacy path, then leaves the node empty).
    fn read(&self, realm: &str, character: &str) -> Option<Vec<(String, bool)>>;
    /// The file written from `rows`, a line each.
    fn write(&self, realm: &str, character: &str, rows: &[(String, bool)]);
}

/// The reference's `ADDONSTATELIST` (`0xbe1bd0`): a node per character in `SMSG_CHAR_ENUM` order,
/// rebuilt whole (`0x51f0b0` clears it, then callback `0x472300` fills a node per record through
/// `AddOnList_LoadCharacter 0x51ebe0`), each holding that character's enable hash. A character
/// with no file is an empty node, whose enable bit is answered from the others.
#[derive(Default)]
pub struct EnableStore {
    /// The realm half of every node's file.
    realm: String,
    /// `(character, enable hash)` in character-list order.
    nodes: Vec<(String, EnableHash)>,
}

impl EnableStore {
    /// A node per character, each read through `files`; a character with no file still gets one
    /// (`0x51ee69`, the node is made before the file is opened).
    pub fn load(realm: &str, characters: &[String], files: &dyn AddonStateFiles) -> Self {
        let nodes = characters
            .iter()
            .map(|character| {
                let rows = files.read(realm, character).unwrap_or_default();
                (character.clone(), EnableHash::from_rows(rows))
            })
            .collect();
        Self {
            realm: realm.to_string(),
            nodes,
        }
    }

    /// A node per character, none with a row, as for characters with no file.
    pub fn blank(realm: &str, characters: &[String]) -> Self {
        Self {
            realm: realm.to_string(),
            nodes: characters
                .iter()
                .map(|c| (c.clone(), EnableHash::default()))
                .collect(),
        }
    }

    /// The characters with a node, in order.
    pub fn characters(&self) -> impl Iterator<Item = &str> {
        self.nodes.iter().map(|(name, _)| name.as_str())
    }

    /// `0x51e470(addon, NULL, useDefault = 0)`, the explicit-only aggregate over the nodes with a
    /// row (`0x51e5df je 0x51e60a`): `Some(v)` when all say `v` (the reference's 2 or 0), `None`
    /// when they are mixed or there are none.
    fn aggregate(&self, addon: &str) -> Option<bool> {
        let mut total = 0usize;
        let mut on = 0usize;
        for (_, hash) in &self.nodes {
            if let Some(v) = hash.get(addon) {
                total += 1;
                on += usize::from(v);
            }
        }
        match (total, on) {
            (0, _) => None,
            (_, 0) => Some(false),
            (t, o) if o == t => Some(true),
            _ => None,
        }
    }

    /// The query `0x51e470(addon, character, useDefault = 1)`: 0 disabled, 1 enabled for some of
    /// the nodes it counted, 2 enabled for all of them. `character` `None` is the NULL name
    /// (`0x51e547`), which counts every node; a name counts its one node and stops (`0x51e60f`),
    /// so it answers 0 or 2 only.
    /// * a node with an explicit row says what the row says;
    /// * a node with none takes [`Self::aggregate`] (the self-recursion `0x51e5f0`), then
    ///   `default_state` (`## DefaultState`, `[rec+0x2b]`) where that is undecided (`0x51e5fd`);
    /// * no node counted (a name no node answers to, or no nodes) is `default_state ? 2 : 0`
    ///   (`0x51e623`): a miss walks the list out (`0x51e55a jne 0x51e611`) with `total` still 0.
    pub fn state(&self, addon: &str, default_state: bool, character: Option<&str>) -> u8 {
        let (mut total, mut on) = (0usize, 0usize);
        for (name, hash) in &self.nodes {
            if character.is_some_and(|c| !name.eq_ignore_ascii_case(c)) {
                continue;
            }
            total += 1;
            on += usize::from(
                hash.get(addon)
                    .unwrap_or_else(|| self.aggregate(addon).unwrap_or(default_state)),
            );
            if character.is_some() {
                break;
            }
        }
        match (total, on) {
            (0, _) => u8::from(default_state) * 2,
            (_, 0) => 0,
            (t, o) if o < t => 1,
            _ => 2,
        }
    }

    /// [`Self::state`] as the load gate reads it: `AddOn_CanLoad`'s check 3 (`0x51e829`) takes
    /// any nonzero answer as enabled.
    pub fn enabled_for(&self, addon: &str, default_state: bool, character: Option<&str>) -> bool {
        self.state(addon, default_state, character) != 0
    }

    /// The setter `0x51ea20`: `None` is every node (the glue's All), a name its one node; a name
    /// with no node is a no-op, as the setter never creates one.
    pub fn set(&mut self, addon: &str, character: Option<&str>, on: bool) {
        for (name, hash) in &mut self.nodes {
            if character.is_none_or(|c| name.eq_ignore_ascii_case(c)) {
                hash.set(addon, on);
            }
        }
    }

    /// The writer `0x51ef20`, glue `SaveAddOns` (`0x46d990`): each dirty node's file, rewritten
    /// from its hash; a clean node is skipped (`0x51ef59`) and its file left as it is.
    pub fn save(&mut self, files: &dyn AddonStateFiles) {
        for (name, hash) in &mut self.nodes {
            if let Some(rows) = hash.take_dirty() {
                files.write(&self.realm, name, &rows);
            }
        }
    }

    /// `0x51ebe0(NULL)`, glue `ResetAddOns` (`0x46d9a0`): every node already in the list read
    /// afresh from its file, clean (`0x51ec59`), whatever the toggles since changed.
    pub fn reload(&mut self, files: &dyn AddonStateFiles) {
        for (name, hash) in &mut self.nodes {
            *hash = EnableHash::from_rows(files.read(&self.realm, name).unwrap_or_default());
        }
    }

    pub fn node(&self, character: &str) -> Option<&EnableHash> {
        self.nodes
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(character))
            .map(|(_, hash)| hash)
    }
}

#[cfg(test)]
mod tests {
    use super::{AddonStateFiles, EnableHash, EnableStore};
    use std::cell::RefCell;
    use std::collections::HashMap;

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

    type Rows = Vec<(String, bool)>;

    /// Files in memory: `(realm, character)` to rows, with every write kept.
    #[derive(Default)]
    struct Mem {
        files: RefCell<HashMap<(String, String), Rows>>,
        writes: RefCell<Vec<(String, String, Rows)>>,
    }

    impl Mem {
        fn with(rows: &[(&str, &[(&str, bool)])]) -> Self {
            let m = Mem::default();
            for (c, r) in rows {
                m.files.borrow_mut().insert(
                    ("R".to_string(), (*c).to_string()),
                    r.iter().map(|(n, v)| ((*n).to_string(), *v)).collect(),
                );
            }
            m
        }
    }

    impl AddonStateFiles for Mem {
        fn read(&self, realm: &str, character: &str) -> Option<Vec<(String, bool)>> {
            self.files
                .borrow()
                .get(&(realm.to_string(), character.to_string()))
                .cloned()
        }
        fn write(&self, realm: &str, character: &str, rows: &[(String, bool)]) {
            self.writes
                .borrow_mut()
                .push((realm.into(), character.into(), rows.to_vec()));
            self.files
                .borrow_mut()
                .insert((realm.into(), character.into()), rows.to_vec());
        }
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| (*s).to_string()).collect()
    }

    /// The glue query's tri-state (`0x51e470`): the NULL name counts every node, a name its one.
    #[test]
    fn the_query_is_a_tri_state_over_the_counted_nodes() {
        let files = Mem::with(&[
            ("A", &[("X", true), ("Y", false)]),
            ("B", &[("X", false), ("Y", false)]),
            ("C", &[("Z", true)]),
        ]);
        let s = EnableStore::load("R", &names(&["A", "B", "C"]), &files);
        // X: on for A, off for B, and C has no row, so it takes the mixed aggregate's default.
        assert_eq!(
            s.state("X", true, None),
            1,
            "A on, B off, C by default: some"
        );
        assert_eq!(
            s.state("X", false, None),
            1,
            "…C off by default: still some"
        );
        // Y: every row says off, and C inherits that.
        assert_eq!(s.state("Y", true, None), 0);
        // Z: only C says, on, so A and B inherit it.
        assert_eq!(s.state("Z", false, None), 2);
        // A name counts its one node: 0 or 2.
        assert_eq!(s.state("X", true, Some("A")), 2);
        assert_eq!(s.state("X", true, Some("b")), 0);
        assert_eq!(
            s.state("X", true, Some("C")),
            2,
            "no row: the aggregate is mixed, so the default"
        );
        assert_eq!(s.state("X", false, Some("C")), 0);
    }

    /// A name no node answers to walks the list out with nothing counted: `DefaultState ? 2 : 0`.
    #[test]
    fn a_missing_node_or_an_empty_store_answers_the_default() {
        let s = EnableStore::blank("R", &names(&["A"]));
        assert_eq!(s.state("X", true, Some("Nobody")), 2);
        assert_eq!(s.state("X", false, Some("Nobody")), 0);
        let none = EnableStore::default();
        assert_eq!(none.state("X", true, None), 2);
        assert_eq!(none.state("X", false, None), 0);
    }

    /// The setter with no name reaches every node, with one only its own, and with a name no node
    /// has nobody (`0x51ea65`, `0x51ebc3`).
    #[test]
    fn the_setter_reaches_the_nodes_it_names() {
        let mut s = EnableStore::blank("R", &names(&["A", "B"]));
        s.set("X", None, false);
        assert_eq!(s.state("X", true, None), 0);
        s.set("X", Some("b"), true);
        assert_eq!(s.state("X", true, None), 1);
        assert_eq!(s.state("X", true, Some("A")), 0);
        s.set("X", Some("Nobody"), true);
        assert_eq!(s.characters().count(), 2);
        assert!(s.node("Nobody").is_none());
    }

    /// The writer saves a dirty node and no other; a reload throws the toggles away and reads the
    /// files again.
    #[test]
    fn save_writes_only_the_dirty_nodes_and_reload_reverts() {
        let files = Mem::with(&[("A", &[("X", true)]), ("B", &[("X", true)])]);
        let mut s = EnableStore::load("R", &names(&["A", "B"]), &files);
        s.set("X", Some("A"), false);
        s.reload(&files);
        assert_eq!(s.state("X", false, Some("A")), 2, "reset reads the file");
        s.set("X", Some("A"), false);
        s.save(&files);
        let writes = files.writes.borrow().clone();
        assert_eq!(
            writes,
            vec![("R".into(), "A".into(), vec![("X".to_string(), false)])]
        );
        s.save(&files);
        assert_eq!(files.writes.borrow().len(), 1, "saved nodes are clean");
    }
}
