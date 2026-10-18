//! The macro icon chooser's catalog, the list `GetNumMacroIcons`/`GetMacroIconInfo` serve. The
//! reference's `BuildMacroIconList` (`0x4f0090`) enumerates the files under `Interface\Icons\`; it
//! does not read `SpellIcon.dbc`.

use std::path::{Path, PathBuf};

use crate::loose::child_dir_ignoring_case;
use crate::Chain;

/// The directory the chooser enumerates and the prefix `GetMacroIconInfo` splices back on
/// (`0x84c988`, at `0x4f1a8d`); the reference strips its 16 characters from each stored entry.
const ICON_DIR: &str = "Interface\\Icons\\";

/// The chooser's list, sorted and deduplicated without case, each entry as the full path
/// `GetMacroIconInfo` returns. `chain` is the mounted archives and `data` the install's `Data`
/// folder, whose parent is the install folder; `None` leaves out that source.
///
/// The reference reads three sources in turn. First the archives' listed names under the prefix
/// ([`Chain::enumerate_archive_files`], `0x401470(6, prefix, 0x4f0220)` at `0x4f00b8`), the
/// callback skipping the prefix's 16 characters unchecked and keeping a name that begins `Spell_`
/// or `Ability_` and ends `.blp` or `.tga` (`0x4f0220`); it has no folder test, so a name below a
/// subfolder is kept with its `\`. Then the two loose scans of `loose_names`. Each entry is the
/// name without its last-dot extension; then a `qsort` with `SStrCmpI` (`0x4f0128`, comparator
/// `0x4f05e0`) and the drop of adjacent duplicates (`0x4f0140`-`0x4f01a4`): the order is
/// alphabetical, and a stock 5875 install yields 517 icons.
pub fn load_macro_icons(chain: Option<&Chain>, data: Option<&Path>) -> Vec<String> {
    let mut names = chain.map_or_else(Vec::new, |c| {
        archive_names(c.enumerate_archive_files(ICON_DIR))
    });
    if let Some(data) = data {
        loose_names(data, &mut names);
    }
    chooser_list(names)
}

/// `0x4f0220` over the listed names, each beginning with the prefix: the part after it, cut at its
/// last dot, when it passes the name and extension rules.
fn archive_names(listed: Vec<String>) -> Vec<String> {
    listed
        .iter()
        .filter_map(|name| {
            let file = &name[ICON_DIR.len()..];
            icon_stem(file)
                .filter(|_| spell_or_ability(file))
                .map(str::to_string)
        })
        .collect()
}

/// The two loose scans, through `0x42ad10`, which lists a folder with `FindFirstFileW` relative to
/// the working directory, the install folder (`0x40224f`): the data path `Data\` (`0x648990`, set
/// at `0x40228c`) joined to the prefix, kept by [`LooseRule::SpellOrAbility`] (`0x4f00f6`), then the
/// prefix itself, kept by [`LooseRule::AnyName`] (`0x4f010f`). benilla's `Data` folder is `data`,
/// and the install folder its parent.
fn loose_names(data: &Path, out: &mut Vec<String>) {
    if let Some(dir) = icons_folder(data) {
        loose_icons(&dir, LooseRule::SpellOrAbility, out);
    }
    if let Some(dir) = data.parent().and_then(icons_folder) {
        loose_icons(&dir, LooseRule::AnyName, out);
    }
}

/// Sort without case, then drop adjacent duplicates, the reference's two steps in its order, and
/// put the prefix back on. Deviation: of names equal but for case, benilla keeps the first in
/// source order, where the reference's survivor is whichever its CRT `qsort` (`0x73f727`, not
/// stable) leaves last, as the dedup frees the earlier of each equal pair (`0x4f0163`-`0x4f0171`).
///
/// An empty name, a file named only `.blp` or `.tga`, stays an empty entry: `GetMacroIconInfo`
/// answers "" for it (`0x4f1a88`) rather than the bare prefix.
fn chooser_list(mut names: Vec<String>) -> Vec<String> {
    names.sort_by_key(|n| n.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
        .into_iter()
        .map(|n| {
            if n.is_empty() {
                n
            } else {
                format!("{ICON_DIR}{n}")
            }
        })
        .collect()
}

/// The name rule both the archive callback and the first loose scan apply: `Ability_` or `Spell_`
/// at the start, without case (`0x84cb38`, `0x84cb30`).
fn spell_or_ability(name: &str) -> bool {
    ["Ability_", "Spell_"].iter().any(|p| {
        name.len() >= p.len() && name.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes())
    })
}

/// The name cut at its last dot, when what follows is `.blp` or `.tga` without case (`0x8354b8`,
/// `0x8354c0`). The last dot starts the extension, so `Foo.tga.blp` stores as `Foo.tga`: the
/// reference's picker shows `Ability_Druid_Mangle.tga` beside `Ability_Druid_Mangle`.
fn icon_stem(name: &str) -> Option<&str> {
    let (stem, ext) = name.rsplit_once('.')?;
    (ext.eq_ignore_ascii_case("blp") || ext.eq_ignore_ascii_case("tga")).then_some(stem)
}

/// `dir`'s `Interface\Icons` folder, each component found without case, as on Windows.
fn icons_folder(dir: &Path) -> Option<PathBuf> {
    child_dir_ignoring_case(&child_dir_ignoring_case(dir, "Interface")?, "Icons")
}

/// Which of the two loose callbacks a scan runs.
#[derive(Clone, Copy)]
enum LooseRule {
    /// `0x4f0350`: the `Spell_`/`Ability_` rule, then a trailing `.bz` cut (`0x84cb44`).
    SpellOrAbility,
    /// `0x4f04f0`: any name.
    AnyName,
}

/// Push the icon names a loose folder holds. `0x42ad10` passes every entry but a hidden one
/// (`0x42addd`-`0x42adef`, its flag argument 0), and each callback skips a folder (`flags & 0x10`,
/// `0x4f0359`, `0x4f04f0`). Listed in name order, which decides only between names equal but for
/// case.
fn loose_icons(dir: &Path, rule: LooseRule, out: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<String> = read
        .flatten()
        .filter(|e| !e.path().is_dir() && !hidden(e))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    files.sort();
    for name in files {
        let name = match rule {
            LooseRule::SpellOrAbility => {
                if !spell_or_ability(&name) {
                    continue;
                }
                // `0x4f03b1`-`0x4f03ef`: the dot whose remainder is `.bz`, the last one, ends it.
                match name.rsplit_once('.') {
                    Some((head, ext)) if ext.eq_ignore_ascii_case("bz") => head.to_string(),
                    _ => name,
                }
            }
            LooseRule::AnyName => name,
        };
        if let Some(stem) = icon_stem(&name) {
            out.push(stem.to_string());
        }
    }
}

/// `FILE_ATTRIBUTE_HIDDEN`, the bit `0x42addd` tests in the find data's attributes.
#[cfg(windows)]
fn hidden(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    entry
        .metadata()
        .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
}

/// A name beginning with `.`, the hidden file of a host with no hidden attribute.
#[cfg(not(windows))]
fn hidden(entry: &std::fs::DirEntry) -> bool {
    entry.file_name().as_encoded_bytes().starts_with(b".")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh install folder under the temp dir, removed when dropped.
    struct Install(PathBuf);

    impl Install {
        fn new(tag: &str) -> Install {
            let dir = std::env::temp_dir().join(format!(
                "benilla-macro-icons-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Install(dir)
        }

        fn touch(&self, rel: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
    }

    impl Drop for Install {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scan(dir: &Path, rule: LooseRule) -> Vec<String> {
        let mut out = Vec::new();
        loose_icons(dir, rule, &mut out);
        out
    }

    /// A loose `.blp` stores an empty name, which the list keeps as an empty entry, not the bare
    /// prefix, since `GetMacroIconInfo` answers "" for it (`0x4f1a88`).
    #[test]
    fn an_empty_icon_name_is_an_empty_entry() {
        assert_eq!(
            chooser_list(vec!["Spell_A".into(), String::new()]),
            ["", "Interface\\Icons\\Spell_A"]
        );
    }

    /// `0x4f0220` keeps a `Spell_`/`Ability_` name with a `.blp` or `.tga` last extension, a
    /// name below a subfolder with it, as it tests no folder.
    #[test]
    fn the_archive_callback_keeps_names_by_their_text_alone() {
        let listed = [
            "Interface\\Icons\\Spell_Fire_A.blp",
            "INTERFACE\\icons\\ability_b.TGA",
            "Interface\\Icons\\Spell_Sub\\C.blp",
            "Interface\\Icons\\Ability_D.tga.blp",
            "Interface\\Icons\\Other.blp",
            "Interface\\Icons\\Spell_E",
            "Interface\\Icons\\",
        ];
        assert_eq!(
            archive_names(listed.iter().map(|n| n.to_string()).collect()),
            ["Spell_Fire_A", "ability_b", "Spell_Sub\\C", "Ability_D.tga"]
        );
    }

    /// `0x4f0350` keeps `Ability_`/`Spell_` names, any case, with a `.bz` cut before the
    /// extension test; `0x4f04f0` keeps any `.blp` or `.tga`. Neither keeps a folder or a hidden
    /// file.
    #[test]
    fn the_two_loose_scans_keep_their_own_names() {
        let tree = Install::new("rules");
        for f in [
            "Spell_Fire_X.blp",
            "ability_y.TGA",
            "Spell_Z.blp.bz",
            "Spell_W.bz",
            "Spell_V.mdx",
            "Other.blp",
            "Ability_Q",
            #[cfg(not(windows))]
            ".Spell_Hidden.blp",
        ] {
            tree.touch(&format!("Icons/{f}"));
        }
        std::fs::create_dir_all(tree.0.join("Icons/Spell_Folder.blp")).unwrap();
        let dir = tree.0.join("Icons");
        assert_eq!(
            scan(&dir, LooseRule::SpellOrAbility),
            ["Spell_Fire_X", "Spell_Z", "ability_y"]
        );
        assert_eq!(
            scan(&dir, LooseRule::AnyName),
            ["Other", "Spell_Fire_X", "ability_y"]
        );
    }

    /// The first scan reads `Data\Interface\Icons\` with the name rule, the second
    /// `Interface\Icons\` beside `Data` with none, each folder found without case; the union with
    /// the archives' names sorts without case and drops a name equal but for case.
    #[test]
    fn the_loose_icons_join_the_list_from_both_folders() {
        let tree = Install::new("union");
        tree.touch("Data/interface/ICONS/Spell_Loose_Data.blp");
        tree.touch("Data/interface/ICONS/Custom_Data.blp");
        tree.touch("Interface/Icons/Custom_Base.tga");
        tree.touch("Interface/Icons/spell_loose_data.blp");
        tree.touch("Interface/Icons/Ability_Ambush.blp");
        let mut names = vec!["Ability_Ambush".to_string(), "Spell_Zap".to_string()];
        loose_names(&tree.0.join("Data"), &mut names);
        assert_eq!(
            chooser_list(names),
            [
                "Interface\\Icons\\Ability_Ambush",
                "Interface\\Icons\\Custom_Base",
                "Interface\\Icons\\Spell_Loose_Data",
                "Interface\\Icons\\Spell_Zap",
            ]
        );
    }
}
