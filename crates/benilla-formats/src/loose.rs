//! The install's loose files: the index Storm keeps of the files on disk under the install, which
//! an open that sets the loose flag asks before the archive chain (`0x647e60`; benilla-re
//! `storm/mount`, "Loose files" to "Which opens ask the index").

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, RwLock};

use crate::wow_data;

/// What an open asks of the index: the flag bits `0x647e60(name, flags)` tests (`0x647ec1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LooseOpen {
    /// Flags 0: the archives alone, as the DBC tables, terrain, the minimap's default textures and
    /// the texture cache's world textures open (`0x47ac30`).
    Archives,
    /// Flag bit 0: the index by the whole name, then the archives, as interface files, interface
    /// textures (`0x770281`), cursors, portraits and every sound file (`0x7a8860`) open.
    Index,
    /// Flag bits 0 and 1: the part after the last backslash first (`0x647ed7`), then the whole
    /// name, as a font face opens (`0x5d0060`, flags 3 at `0x5d0121`).
    BareNameFirst,
}

/// The index: a file's path below the folder walked, compared without ASCII case (`SStrCmpI`), to
/// what opens it; a key's first entry stays (`0x646d66`).
#[derive(Debug, Default)]
pub struct LooseIndex {
    files: HashMap<String, LooseFile>,
}

/// What an index entry opens, by the kind `0x646910` stores (`+0x1c`) and `SFile::OpenEx`
/// (`0x6477c0`) switches on (`0x64783d`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LooseFile {
    /// Kind 0: the file itself, opened with `fopen(path, "rb")` (`0x647844`).
    Plain(PathBuf),
    /// Kind 2: a `<name>.MPQ`, keyed without the suffix, opened as an archive from which the
    /// name's last component is read (`0x647a30`).
    Archive {
        /// The `.MPQ` on disk.
        archive: PathBuf,
        /// The file read from it: the key's part after its last backslash.
        member: String,
    },
}

impl LooseFile {
    /// The file on disk the entry opens.
    pub fn path(&self) -> &Path {
        match self {
            LooseFile::Plain(path) | LooseFile::Archive { archive: path, .. } => path,
        }
    }

    /// The entry's bytes, or `None` when the file, the archive or its member will not open: the
    /// open fails then, with no turn to the archive chain (`0x647865`, `0x647a63`, `0x647a94`).
    pub fn read(&self) -> Option<Vec<u8>> {
        match self {
            LooseFile::Plain(path) => std::fs::read(path).ok(),
            LooseFile::Archive { archive, member } => benilla_mpq::Archive::open(archive)
                .ok()?
                .read_file(member)
                .ok(),
        }
    }
}

/// The data folder the second walk takes, `0xc520c8` (`Data\`); the walk skips a folder whose
/// path below the walked folder is a prefix of it, compared with case (`0x646a2f`).
const DATA_DIR: &str = "Data\\";

/// The folders benilla leaves out of the index in either walk, compared without case: the
/// install's `Interface\AddOns\`, for which benilla's own AddOns folder stands by its written
/// deviation (`benilla-app`'s `ui_script::addons::root`).
const NEVER_INDEXED: &[&str] = &["Interface\\AddOns"];

/// The folders left out of the base walk only: the glue build moves a loose `Interface\GlueXML`
/// and `Interface\FrameXML` below the install folder aside to `%s.old` before it reads anything
/// (`0x51fb50`, at `0x46a8eb`) and empties the index (`0x51fbe3`), so their files never answer.
/// benilla never writes into the install, so it leaves them unread instead.
const MOVED_ASIDE: &[&str] = &["Interface\\GlueXML", "Interface\\FrameXML"];

/// The key a name is looked up and entered by: ASCII case folded, as `SStrCmpI` compares
/// (`0x647f37`). A `/` stays: the keys hold `\`, so a name written with `/` never matches, though
/// the bucket hash reads it as `\` (`0x64b3f0`).
fn key(name: &str) -> String {
    name.to_ascii_lowercase()
}

impl LooseIndex {
    /// The index of `base` and then `data`, built as `0x646ea0` builds it from the base path
    /// (`0xc52418`) and that path's `Data\`: both walked depth first, the base folder's entries
    /// entered before `data`'s.
    pub fn build(base: Option<&Path>, data: &Path) -> LooseIndex {
        let mut index = LooseIndex::default();
        let data_canon = std::fs::canonicalize(data).ok();
        if let Some(base) = base {
            let mut seen = HashSet::new();
            index.walk(base, "", &mut seen, &|rel, canon| {
                // The data folder is the second walk's, whatever `$WOW_DATA` names it.
                MOVED_ASIDE.iter().any(|m| m.eq_ignore_ascii_case(rel))
                    || data_canon.as_deref().is_some_and(|d| d == canon)
            });
        }
        let mut seen = HashSet::new();
        index.walk(data, "", &mut seen, &|_, _| false);
        index
    }

    /// `0x646910(root, prefix)`: every entry of `root\prefix` but `.` and `..`, in name order. A
    /// folder is skipped when its path below the root is a prefix of [`DATA_DIR`], is one of
    /// [`NEVER_INDEXED`] or `skip` says so, and walked otherwise, once per walk even through a
    /// link back up the tree; a regular file is entered.
    fn walk(
        &mut self,
        root: &Path,
        prefix: &str,
        seen: &mut HashSet<PathBuf>,
        skip: &dyn Fn(&str, &Path) -> bool,
    ) {
        let dir = if prefix.is_empty() {
            root.to_path_buf()
        } else {
            root.join(prefix.replace('\\', std::path::MAIN_SEPARATOR_STR))
        };
        let Ok(canon) = std::fs::canonicalize(&dir) else {
            return;
        };
        if !seen.insert(canon) {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .collect();
        // Listing order decides only between names equal but for case; sorted, it is the same on
        // every host.
        names.sort_by_cached_key(|n| (n.to_ascii_lowercase(), n.clone()));
        for name in names {
            if name == "." || name == ".." {
                continue;
            }
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}\\{name}")
            };
            let full = dir.join(&name);
            // `stat`, which follows a link, as `0x646a04` does.
            let Ok(meta) = std::fs::metadata(&full) else {
                continue;
            };
            if meta.is_dir() {
                let canon = std::fs::canonicalize(&full).unwrap_or_else(|_| full.clone());
                if DATA_DIR.starts_with(rel.as_str())
                    || NEVER_INDEXED.iter().any(|n| n.eq_ignore_ascii_case(&rel))
                    || skip(&rel, &canon)
                {
                    continue;
                }
                self.walk(root, &rel, seen, skip);
            } else if meta.is_file() {
                self.insert(&rel, full);
            }
        }
    }

    /// Enter one file by its path below the walked folder. A last extension `.MPQ` (any case) is
    /// cut from the key and makes kind 2 (`0x646bc7`, `0x646bd4`); any other file is kind 0.
    fn insert(&mut self, rel: &str, full: PathBuf) {
        let (key_rel, file) = match rel.rsplit_once('.') {
            // Kind 1, a `.bz` file opened past a `BZ00` header (`0x646aab`, `0x647894`): the
            // reference does not say how its body decodes, so it is left out of the index.
            Some((_, ext)) if ext.eq_ignore_ascii_case("bz") => return,
            Some((stem, ext)) if ext.eq_ignore_ascii_case("mpq") => {
                let member = stem.rsplit('\\').next().unwrap_or(stem).to_string();
                let archive = full;
                (stem, LooseFile::Archive { archive, member })
            }
            _ => (rel, LooseFile::Plain(full)),
        };
        self.files.entry(key(key_rel)).or_insert(file);
    }

    /// The entry whose key equals `name` without ASCII case.
    pub fn get(&self, name: &str) -> Option<&LooseFile> {
        self.files.get(&key(name))
    }

    /// The entry that answers an open of `name` with `open`, `0x647e60`'s probes: with
    /// [`LooseOpen::BareNameFirst`] the part after the last backslash, then for either loose open
    /// the whole name. `None` sends the open to the archives; a hit is the open's one answer,
    /// and a file that then fails to read is a miss with no turn to the archives (`0x647865`).
    pub fn resolve(&self, name: &str, open: LooseOpen) -> Option<&LooseFile> {
        match open {
            LooseOpen::Archives => None,
            LooseOpen::Index => self.get(name),
            LooseOpen::BareNameFirst => {
                let bare = name.rsplit('\\').next().unwrap_or(name);
                self.get(bare).or_else(|| self.get(name))
            }
        }
    }

    /// The number of keys.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether no file was entered.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// An index with its lifetime: built by the first open that asks (`0x647ebc` calls `0x646ea0`
/// when the built byte `0xc5251c` is clear), emptied by [`Self::clear`] as `0x646fc0` empties it,
/// and built again by the next open, so a file added in between is found only after a clear.
pub struct LooseIndexCell {
    build: Box<dyn Fn() -> LooseIndex + Send + Sync>,
    slot: RwLock<Option<Arc<LooseIndex>>>,
    generation: AtomicU64,
}

impl LooseIndexCell {
    /// A cell whose index `build` makes, built on first use.
    pub fn new(build: impl Fn() -> LooseIndex + Send + Sync + 'static) -> Self {
        LooseIndexCell {
            build: Box::new(build),
            slot: RwLock::new(None),
            generation: AtomicU64::new(0),
        }
    }

    /// The index, built now if it is not.
    pub fn get(&self) -> Arc<LooseIndex> {
        if let Some(index) = self.slot.read().unwrap_or_else(|p| p.into_inner()).as_ref() {
            return index.clone();
        }
        let mut slot = self.slot.write().unwrap_or_else(|p| p.into_inner());
        slot.get_or_insert_with(|| Arc::new((self.build)())).clone()
    }

    /// Empty the index, `0x646fc0`, so the next open builds it again.
    pub fn clear(&self) {
        *self.slot.write().unwrap_or_else(|p| p.into_inner()) = None;
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// How many times the index has been emptied: a holder of what it answered compares this to
    /// know its answers may be stale.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

/// The player's install as a [`LooseIndexCell`], over [`wow_data`] and its parent folder; empty
/// without an install.
pub fn install_index() -> Arc<LooseIndexCell> {
    static CELL: LazyLock<Arc<LooseIndexCell>> = LazyLock::new(|| {
        Arc::new(LooseIndexCell::new(|| match wow_data() {
            Some(data) => LooseIndex::build(data.parent(), &data),
            None => LooseIndex::default(),
        }))
    });
    CELL.clone()
}

/// The install's loose `Interface\GlueXML` and `Interface\FrameXML` folders, any case, by those
/// names: the folders the glue build's sweep moves aside (`0x51fb50`).
pub fn install_loose_interface_folders() -> Vec<&'static str> {
    let Some(base) = wow_data().and_then(|d| d.parent().map(Path::to_path_buf)) else {
        return Vec::new();
    };
    let Some(interface) = child_dir_ignoring_case(&base, "Interface") else {
        return Vec::new();
    };
    MOVED_ASIDE
        .iter()
        .copied()
        .filter(|folder| {
            let name = folder.rsplit('\\').next().unwrap_or(folder);
            child_dir_ignoring_case(&interface, name).is_some()
        })
        .collect()
}

/// `dir`'s folder named `name` without regard to case, as `OsMoveFile` finds it on Windows.
fn child_dir_ignoring_case(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
                && e.path().is_dir()
        })
        .map(|e| e.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the temp dir, removed when dropped.
    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> Tree {
            let dir = std::env::temp_dir().join(format!(
                "benilla-loose-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Tree(dir)
        }

        fn file(&self, rel: &str, body: &str) -> PathBuf {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
            path
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn body(file: Option<&LooseFile>) -> Option<String> {
        file.map(|f| String::from_utf8(f.read().unwrap()).unwrap())
    }

    /// Both walks enter files by their path below the folder walked, the base folder's first, and
    /// a name matches without case, written with `\\` and not with `/`.
    #[test]
    fn the_base_folder_is_walked_before_data_and_keys_ignore_case() {
        let t = Tree::new("order");
        t.file("Fonts/FRIZQT__.TTF", "base font");
        t.file("Data/Fonts/FRIZQT__.TTF", "data font");
        t.file("Data/Interface/Glues/Common/Glues-WoW-Logo.blp", "logo");
        t.file("Data/eula.html", "eula");
        let ix = LooseIndex::build(Some(&t.0), &t.0.join("Data"));
        assert_eq!(
            body(ix.get("fonts\\frizqt__.ttf")).as_deref(),
            Some("base font")
        );
        assert_eq!(
            body(ix.get("Interface\\Glues\\Common\\Glues-WoW-Logo.BLP")).as_deref(),
            Some("logo")
        );
        // `SStrCmpI` does not read `/` as `\`, so a name written with it misses.
        assert_eq!(ix.get("Interface/Glues/Common/Glues-WoW-Logo.blp"), None);
        assert_eq!(body(ix.get("EULA.html")).as_deref(), Some("eula"));
        // The base walk skips `Data`, so a `Data\` file is never keyed by its `Data\` path.
        assert_eq!(ix.get("Data\\eula.html"), None);
        assert_eq!(ix.len(), 3);
    }

    /// The font open's bare-name probe finds a face loose in the install folder or `Data\` under
    /// any path, ahead of the whole name; the plain open asks only the whole name.
    #[test]
    fn a_font_open_asks_the_bare_name_first() {
        let t = Tree::new("bare");
        t.file("FRIZQT__.TTF", "root copy");
        t.file("Fonts/FRIZQT__.TTF", "fonts copy");
        let ix = LooseIndex::build(Some(&t.0), &t.0.join("Data"));
        let name = "Fonts\\FRIZQT__.TTF";
        assert_eq!(
            body(ix.resolve(name, LooseOpen::BareNameFirst)).as_deref(),
            Some("root copy")
        );
        assert_eq!(
            body(ix.resolve(name, LooseOpen::Index)).as_deref(),
            Some("fonts copy")
        );
        assert_eq!(ix.resolve(name, LooseOpen::Archives), None);
        assert_eq!(
            ix.resolve("Fonts\\MORPHEUS.TTF", LooseOpen::BareNameFirst),
            None
        );
        // `SStrChrR(name, '\\')` finds no backslash in a `/` name, whose whole self misses too.
        assert_eq!(
            ix.resolve("Fonts/FRIZQT__.TTF", LooseOpen::BareNameFirst),
            None
        );
    }

    /// The folders the reference moves aside and the AddOns folder benilla's own stands in for
    /// are not indexed; a `Data\` copy of the moved folders is, as the move is of the base
    /// folder's alone; a `.bz` file is not entered, and a `.MPQ` is, by its path less the suffix.
    #[test]
    fn moved_aside_addons_and_bz_files_stay_out() {
        let t = Tree::new("skip");
        t.file("Interface/FrameXML/UIParent.lua", "x");
        t.file("interface/gluexml/GlueParent.lua", "x");
        t.file("Interface/AddOns/Atlas/Atlas.toc", "x");
        t.file("Data/Interface/AddOns/Atlas/Atlas.toc", "x");
        t.file("Data/Interface/GlueXML/GlueStrings.lua", "data glue");
        t.file("Data/patch-3.MPQ", "x");
        t.file("Textures/Foo.blp.bz", "x");
        let ix = LooseIndex::build(Some(&t.0), &t.0.join("Data"));
        assert_eq!(ix.get("Interface\\FrameXML\\UIParent.lua"), None);
        assert_eq!(ix.get("Interface\\GlueXML\\GlueParent.lua"), None);
        assert_eq!(ix.get("Interface\\AddOns\\Atlas\\Atlas.toc"), None);
        assert_eq!(
            ix.get("patch-3"),
            Some(&LooseFile::Archive {
                archive: t.0.join("Data").join("patch-3.MPQ"),
                member: "patch-3".to_string(),
            })
        );
        assert_eq!(ix.get("Textures\\Foo.blp"), None);
        assert_eq!(
            body(ix.get("Interface\\GlueXML\\GlueStrings.lua")).as_deref(),
            Some("data glue")
        );
        assert_eq!(ix.len(), 2);
    }

    /// A link back up the tree is walked once, and a `$WOW_DATA` folder not named `Data` is the
    /// second walk's only.
    #[cfg(unix)]
    #[test]
    fn a_link_loop_ends_and_the_data_folder_is_walked_once() {
        let t = Tree::new("loop");
        t.file("WTF/Config.wtf", "cfg");
        t.file("MyData/Credits.html", "credits");
        std::os::unix::fs::symlink(&t.0, t.0.join("WoW")).unwrap();
        let ix = LooseIndex::build(Some(&t.0), &t.0.join("MyData"));
        assert_eq!(body(ix.get("WTF\\Config.wtf")).as_deref(), Some("cfg"));
        assert_eq!(body(ix.get("Credits.html")).as_deref(), Some("credits"));
        assert_eq!(ix.get("WoW\\WTF\\Config.wtf"), None);
        assert_eq!(ix.get("MyData\\Credits.html"), None);
        assert_eq!(ix.len(), 2);
    }

    /// A file added after the index is built is not found until the index is emptied, and is
    /// found by the next open after, which builds it again.
    #[test]
    fn a_file_added_after_the_build_is_found_only_after_a_clear() {
        let t = Tree::new("cell");
        t.file("Data/eula.html", "eula");
        let root = t.0.clone();
        let cell = LooseIndexCell::new(move || LooseIndex::build(Some(&root), &root.join("Data")));
        assert_eq!(cell.get().len(), 1);
        t.file("Fonts/FRIZQT__.TTF", "added");
        assert_eq!(
            cell.get().get("Fonts\\FRIZQT__.TTF"),
            None,
            "the built index stays"
        );
        let before = cell.generation();
        cell.clear();
        assert_ne!(cell.generation(), before);
        assert_eq!(
            body(cell.get().get("Fonts\\FRIZQT__.TTF")).as_deref(),
            Some("added")
        );
    }

    /// A kind-2 entry reads the name's last component out of the `.MPQ` on disk: here a real
    /// archive's `(listfile)`, linked as `Misc\(listfile).MPQ`.
    #[cfg(unix)]
    #[test]
    fn a_loose_mpq_answers_its_name_from_inside_the_archive() {
        let data = crate::wow_data_or_skip!();
        let real = data.join("patch.MPQ");
        let want = benilla_mpq::Archive::open(&real)
            .and_then(|a| a.read_file("(listfile)"))
            .expect("patch.MPQ carries a (listfile)");
        let t = Tree::new("mpq");
        std::fs::create_dir_all(t.0.join("Data/Misc")).unwrap();
        std::os::unix::fs::symlink(&real, t.0.join("Data/Misc/(listfile).MPQ")).unwrap();
        let ix = LooseIndex::build(Some(&t.0), &t.0.join("Data"));
        let hit = ix
            .resolve("Misc\\(LISTFILE)", LooseOpen::Index)
            .expect("keyed less .MPQ");
        assert_eq!(hit.read().as_deref(), Some(&want[..]));
        assert_eq!(ix.get("Misc\\(listfile).MPQ"), None);
    }
}
