//! The 1.12 glue string table, read off the MPQ chain at startup: the glue screens' localized
//! text (race and class paragraphs, customization labels, login refusals, button captions).
//!
//! Two files, in `GlueXML.toc`'s order: `GlueStrings.lua`, then `GlueLocalization.xml`, whose
//! script's `Localize()` overwrites keys of the base table (32 in enGB, among them the long login
//! refusals and the realm-type suffixes). Both are plain `KEY = "value";` lines, parsed without a
//! Lua VM; the text is read from the player's install, never embedded.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_assets::{LockRecover, WorldAssets};

const GLUE_STRINGS: &str = "Interface\\GlueXML\\GlueStrings.lua";
const GLUE_LOCALIZATION: &str = "Interface\\GlueXML\\GlueLocalization.lua";

/// The glue string table; empty without client data, and callers fall back to built-in captions.
#[derive(Resource, Default)]
pub(crate) struct GlueStrings(HashMap<String, String>);

impl GlueStrings {
    /// A table from parsed pairs, for tests that resolve against the real file.
    #[cfg(test)]
    pub(crate) fn from_map(map: HashMap<String, String>) -> Self {
        Self(map)
    }

    /// The pairs back out, for a test that has to alter one key and re-resolve.
    #[cfg(test)]
    pub(crate) fn into_map(self) -> HashMap<String, String> {
        self.0
    }

    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// The string for a key, falling back to a built-in caption.
    pub(crate) fn text<'a>(&'a self, key: &str, fallback: &'a str) -> &'a str {
        self.get(key).unwrap_or(fallback)
    }
}

/// Startup: reads the base file, then lays the locale patch over it, in `GlueXML.toc`'s order.
/// Either may be missing on its own.
pub(crate) fn load_glue_strings(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let mut table = GlueStrings::default();
    if let Some(assets) = assets {
        let mut chain = assets.chain.lock_recover();
        match chain.read_file(GLUE_STRINGS) {
            Ok(bytes) => {
                table.0 = parse_glue_strings(&String::from_utf8_lossy(&bytes));
                info!("glue strings: {} entries", table.0.len());
            }
            Err(e) => warn!("glue strings unavailable ({e:#}) — built-in captions only"),
        }
        match chain.read_file(GLUE_LOCALIZATION) {
            Ok(bytes) => {
                let patch = parse_localize_overrides(&String::from_utf8_lossy(&bytes));
                info!("glue strings: {} locale overrides", patch.len());
                table.0.extend(patch);
            }
            Err(e) => warn!("glue localization unavailable ({e:#}) — unpatched glue strings"),
        }
    }
    commands.insert_resource(table);
}

/// Parses the `KEY = "value";` lines, unfolding `\n`/`\t`/`\"`/`\\` escapes; anything else is
/// skipped.
fn parse_glue_strings(src: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in src.lines() {
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('"') else {
            continue;
        };
        // Unescape up to the closing quote; a line whose quote never closes is skipped.
        let mut value = String::new();
        let mut chars = rest.chars();
        let mut closed = false;
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    closed = true;
                    break;
                }
                '\\' => match chars.next() {
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some('r') => {}
                    Some(other) => value.push(other),
                    None => break,
                },
                other => value.push(other),
            }
        }
        if closed {
            out.insert(key.to_string(), value);
        }
    }
    out
}

/// The assignments inside `GlueLocalization.lua`'s `Localize()`, which `GlueLocalization.xml` runs
/// as it loads. The file's other function, `LocalizeFrames()`, moves the `WorldOfWarcraftRating`
/// logo (called on `FRAMES_LOADED`), a frame benilla's glue does not have, so it is not read.
///
/// The body ends at the first unindented `end`, as the stock file is formatted; a file of another
/// shape yields no overrides.
fn parse_localize_overrides(src: &str) -> HashMap<String, String> {
    let mut body = String::new();
    let mut inside = false;
    for line in src.lines() {
        if !inside {
            inside = line.trim_start().starts_with("function Localize()");
            continue;
        }
        if line.trim_end() == "end" {
            return parse_glue_strings(&body);
        }
        body.push_str(line);
        body.push('\n');
    }
    HashMap::new()
}

/// Builds the table as [`load_glue_strings`] does, for tests against the real chain.
#[cfg(test)]
pub(crate) fn table_from_chain(chain: &mut benilla_formats::Chain) -> GlueStrings {
    let base = chain.read_file(GLUE_STRINGS).expect("GlueStrings.lua");
    let mut map = parse_glue_strings(&String::from_utf8_lossy(&base));
    let patch = chain
        .read_file(GLUE_LOCALIZATION)
        .expect("GlueLocalization.lua");
    map.extend(parse_localize_overrides(&String::from_utf8_lossy(&patch)));
    GlueStrings::from_map(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_assignments_and_skips_the_rest() {
        let src = r#"
MALE = "Male";
FACTION_INFO_HORDE = "Four races\ncomprise the Horde";
QUOTED = "a \"b\" c";
-- a comment = "not a string";
CODE = getglobal("nope");
BROKEN = "no close
"#;
        let t = parse_glue_strings(src);
        assert_eq!(t.get("MALE").map(String::as_str), Some("Male"));
        assert_eq!(
            t.get("FACTION_INFO_HORDE").map(String::as_str),
            Some("Four races\ncomprise the Horde")
        );
        assert_eq!(t.get("QUOTED").map(String::as_str), Some(r#"a "b" c"#));
        assert!(!t.contains_key("CODE"));
        assert!(!t.contains_key("BROKEN"));
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn the_locale_patch_reads_localize_and_stops_at_its_end() {
        let src = r#"function Localize()
	-- Put all locale specific string adjustments here
	AUTH_REJECT = "Login unavailable - contact support";
	PVP_PARENTHESES = "PVP";
	--SHOW_CONTEST_AGREEMENT = 1;
end

function LocalizeFrames()
	NOT_A_STRING_TABLE = "must not leak";
	WorldOfWarcraftRating:SetTexture("Interface\\Glues\\Login\\Glues-FrenchRating");
end
"#;
        let t = parse_localize_overrides(src);
        assert_eq!(
            t.get("AUTH_REJECT").map(String::as_str),
            Some("Login unavailable - contact support")
        );
        assert_eq!(t.get("PVP_PARENTHESES").map(String::as_str), Some("PVP"));
        assert!(
            !t.contains_key("NOT_A_STRING_TABLE"),
            "LocalizeFrames leaked"
        );
        assert!(!t.contains_key("SHOW_CONTEST_AGREEMENT"), "comment parsed");
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn an_unrecognised_file_patches_nothing() {
        assert!(parse_localize_overrides("KEY = \"value\";\n").is_empty());
        assert!(parse_localize_overrides("function Localize()\n\tK = \"v\";\n").is_empty());
    }

    /// Three keys whose base and patched text differ. Skips without client data.
    #[test]
    fn the_real_chain_patches_the_login_refusals_and_the_realm_suffixes() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");

        let base = chain.read_file(GLUE_STRINGS).expect("GlueStrings.lua");
        let base = parse_glue_strings(&String::from_utf8_lossy(&base));
        let table = table_from_chain(&mut chain);

        for key in ["AUTH_BANNED", "LOGIN_UNKNOWN_ACCOUNT", "PVP_PARENTHESES"] {
            let before = base.get(key).expect("key in the base file").as_str();
            let after = table.get(key).expect("key in the patched table");
            assert_ne!(before, after, "{key} was not patched by Localize()");
        }

        // The enGB patch drops the parentheses: `CharSelectRealmName` reads "Realm PVP".
        assert_eq!(
            base.get("PVP_PARENTHESES").map(String::as_str),
            Some("(PVP)")
        );
        assert_eq!(table.get("PVP_PARENTHESES"), Some("PVP"));

        // The patch overwrites keys; it does not replace the table.
        assert!(table.get("FACTION_INFO_HORDE").is_some(), "base lost");
        assert!(base.len() > 100 && table.0.len() >= base.len());
    }
}
