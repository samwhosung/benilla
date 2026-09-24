//! The **macro icon chooser's catalog** — the list `GetNumMacroIcons`/`GetMacroIconInfo` serve.
//!
//! Byte-derived from the reference client's `BuildMacroIconList` (`0x4f0090`). The headline: **the
//! chooser does not read `SpellIcon.dbc`**. It enumerates the files that actually exist under
//! `Interface\Icons\`.

use anyhow::Result;

use crate::Chain;

/// The directory the chooser enumerates — and, at `GetMacroIconInfo`, the prefix it splices back on
/// (`0x84c988`, spliced by `SStrPrintf("%s%s", …)` at `0x4f1a8d`). 16 characters, which is the
/// literal count the client strips when it stores an entry.
const ICON_DIR: &str = "Interface\\Icons\\";

/// The **macro icon chooser** list — every `Interface\Icons\` file whose name begins `Spell_` or
/// `Ability_`, case-insensitively sorted and deduplicated, each as a full texture path.
///
/// This is the *enumeration*, not a DBC scan, because that is what the client does
/// (`BuildMacroIconList 0x4f0090`). It unions three sources: the `(listfile)` of every open
/// `patch*.MPQ` plus `interface.MPQ` (name-table index 6, `0x82e12c`), read via `0x648fb0`; a disk
/// scan of `SFileGetBasePath() + "Interface\Icons\"`; and a CWD-relative `Interface\Icons\` scan.
/// We serve the archive half through [`Chain::list`] — a stock install has no loose `Interface\Icons`
/// directory, so sources 2 and 3 contribute nothing and are not implemented. (They are how the
/// reference lets a user drop a custom icon in; if we ever want that, it belongs here.)
///
/// Per entry the client keeps the name **after the 16-char prefix, with the extension stripped**,
/// accepting only a last-dot extension in `{.blp, .tga}` and rejecting directories. It then
/// `qsort`s case-insensitively (`0x4f0128`, comparator `0x4f05e0` = `SStrCmpI`) and adjacent-dedups
/// walking down (`0x4f0140`–`0x4f01a4`) — so the dedup key is the whole stored name, folded for
/// case, and `Foo.blp` beside `Foo.tga` collapses to one entry. **Sorted, not DBC row order**: the
/// order the player scrolls is alphabetical.
///
/// We store the reassembled full path rather than the bare name. `GetMacroIconInfo` splices
/// [`ICON_DIR`] back on anyway, so what Lua sees is identical, and every consumer here (the popup's
/// `SetTexture`, `CreateMacro`'s saved texture) wants the whole path.
///
/// **Why this replaces a `SpellIcon.dbc` scan** (bug B221): the DBC names art that
/// does not ship. Five of its chooser-eligible rows have no file under any name (`Ability_Temp`,
/// `Spell_Holy_Invulnerable`, `Spell_Misc_Food_08`, `Spell_Orc_Omniscience`,
/// `Ability_Hunter_BeastSooth`), and two more are malformed for a texture lookup
/// (`Spells\Icon\Spell_Fire_Fire`, a different directory root; `Ability_Druid_Mangle.tga`, carrying
/// a literal extension) — so the picker showed solid white cells where those landed. Enumerating
/// the archive cannot produce a name with no file behind it: the defect class is gone by
/// construction, not by an allow-list. On a stock 5875 install this yields **517** icons (the
/// reference's rules count the same 517 off `patch.MPQ` 77 + `interface.MPQ` 443 = 520 raw, less 3
/// duplicate names).
pub fn load_macro_icons(chain: &mut Chain) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    for entry in chain.list()? {
        let path = entry.name.replace('/', "\\");
        // The prefix test is case-insensitive (`SStrCmpI`), as is everything else here: archive
        // listfiles are not consistently cased.
        if path.len() <= ICON_DIR.len()
            || !path.as_bytes()[..ICON_DIR.len()].eq_ignore_ascii_case(ICON_DIR.as_bytes())
        {
            continue;
        }
        let file = &path[ICON_DIR.len()..];
        // Directories are rejected on `flags & 0x10`; the listfile's equivalent is a name that is
        // still a path, i.e. anything in a SUBdirectory of Interface\Icons.
        if file.contains('\\') {
            continue;
        }
        // Extension = the LAST dot, and only `.blp`/`.tga` are icons. `Foo.tga.blp` therefore
        // stores as `Foo.tga` — which is exactly the entry that makes the reference's own picker
        // carry an `Ability_Druid_Mangle.tga` beside `Ability_Druid_Mangle`… except that both
        // resolve to the same art, so the CI dedup below cannot collapse them and both are shown.
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
    // `qsort` by `SStrCmpI`, then adjacent-dedup walking down — the client's own two steps, in its
    // own order (sorting first is what makes a single adjacent pass a complete dedup).
    names.sort_by_key(|n| n.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    Ok(names
        .into_iter()
        .map(|n| format!("{ICON_DIR}{n}"))
        .collect())
}
