//! The macro icon chooser's catalog, the list `GetNumMacroIcons`/`GetMacroIconInfo` serve. The
//! reference's `BuildMacroIconList` (`0x4f0090`) enumerates the files under `Interface\Icons\`; it
//! does not read `SpellIcon.dbc`.

use anyhow::Result;

use crate::Chain;

/// The directory the chooser enumerates and the prefix `GetMacroIconInfo` splices back on
/// (`0x84c988`, at `0x4f1a8d`); the reference strips its 16 characters from each stored entry.
const ICON_DIR: &str = "Interface\\Icons\\";

/// The chooser's list: every `Interface\Icons\` file whose name begins `Spell_` or `Ability_`,
/// sorted and deduplicated without case, each as the full path `GetMacroIconInfo` returns.
///
/// The reference unions the `(listfile)` of every open `patch*.MPQ` and `interface.MPQ` (name table
/// index 6, `0x82e12c`, read via `0x648fb0`) with two loose-file scans of `Interface\Icons\`; only
/// the archive half is built, as a stock install has no loose icons. Per entry it keeps the name
/// after the prefix without a last-dot `.blp` or `.tga`, skipping directories, then `qsort`s with
/// `SStrCmpI` (`0x4f0128`, comparator `0x4f05e0`) and drops adjacent duplicates
/// (`0x4f0140`-`0x4f01a4`): the order is alphabetical, and a stock 5875 install yields 517 icons.
pub fn load_macro_icons(chain: &mut Chain) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    for entry in chain.list()? {
        let path = entry.name.replace('/', "\\");
        // Case-insensitive (`SStrCmpI`), as listfiles are not consistently cased.
        if path.len() <= ICON_DIR.len()
            || !path.as_bytes()[..ICON_DIR.len()].eq_ignore_ascii_case(ICON_DIR.as_bytes())
        {
            continue;
        }
        let file = &path[ICON_DIR.len()..];
        // The reference skips directories (`flags & 0x10`): here, a name in a subdirectory.
        if file.contains('\\') {
            continue;
        }
        // The last dot starts the extension, so `Foo.tga.blp` stores as `Foo.tga`: the reference's
        // picker shows `Ability_Druid_Mangle.tga` beside `Ability_Druid_Mangle`.
        let Some((stem, ext)) = file.rsplit_once('.') else {
            continue;
        };
        if !ext.eq_ignore_ascii_case("blp") && !ext.eq_ignore_ascii_case("tga") {
            continue;
        }
        let keeps = ["Spell_", "Ability_"].iter().any(|p| {
            stem.len() >= p.len() && stem.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes())
        });
        if keeps {
            names.push(stem.to_string());
        }
    }
    // Sort without case, then drop adjacent duplicates: the reference's two steps, in its order.
    names.sort_by_key(|n| n.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    Ok(names
        .into_iter()
        .map(|n| format!("{ICON_DIR}{n}"))
        .collect())
}
