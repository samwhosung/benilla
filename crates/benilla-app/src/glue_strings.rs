//! The 1.12 glue string table, read off the MPQ chain at startup — the localized text the glue
//! screens quote: faction/race/class description paragraphs (`FACTION_INFO_*`, `RACE_INFO_*`,
//! `ABILITY_INFO_*`, `CLASS_*`), the per-race customization dial labels (`HAIR_<tok>_STYLE`,
//! `FACIAL_HAIR_<tok>`), the login refusals, and the button captions.
//!
//! **It is two files, in the reference's own order**. `GlueXML.toc` sources
//! `GlueStrings.lua` at line 1 and `GlueLocalization.xml` at line 3, and that XML exists only to
//! load its script and call `Localize()` — a function of nothing but assignments, which *overwrite*
//! the base table. In 1.12 enGB it rewrites 32 keys, among them every long login refusal
//! (`AUTH_BANNED`, `LOGIN_UNKNOWN_ACCOUNT`, …) and the realm-type suffixes. Reading only the base
//! file showed the player sentences their own client never shows. The in-game side already loads
//! its exact twin — `Interface\FrameXML\Localization.xml`, same two-part shape, sourced from
//! `benilla.toc` — so the glue side was the half that had been left behind.
//!
//! Runtime-read, never embedded: the paragraphs are Blizzard content, so they load from the
//! player's own client data like every other asset (the repo's never-commit rule) — which is also
//! what makes the locale *theirs* rather than ours to guess. Both files are plain `KEY = "value";`
//! Lua assignments, one per line — a full Lua VM would be theater here; [`parse_glue_strings`]
//! handles exactly that shape (and skips everything else).

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_assets::{LockRecover, WorldAssets};

const GLUE_STRINGS: &str = "Interface\\GlueXML\\GlueStrings.lua";
const GLUE_LOCALIZATION: &str = "Interface\\GlueXML\\GlueLocalization.lua";

/// The glue string table. Present but possibly empty (missing client data — the graceful-absence
/// posture; callers fall back to their built-in captions).
#[derive(Resource, Default)]
pub(crate) struct GlueStrings(HashMap<String, String>);

impl GlueStrings {
    /// Build a table straight from parsed pairs — the test seam for code that has to resolve
    /// against the *real* shipped file rather than a stub.
    #[cfg(test)]
    pub(crate) fn from_map(map: HashMap<String, String>) -> Self {
        Self(map)
    }

    /// The pairs back out, for a test that has to alter one key and re-resolve.
    #[cfg(test)]
    pub(crate) fn into_map(self) -> HashMap<String, String> {
        self.0
    }

    /// The string for a key, or `None` (unknown key / no data).
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// The string for a key, falling back to a built-in caption.
    pub(crate) fn text<'a>(&'a self, key: &str, fallback: &'a str) -> &'a str {
        self.get(key).unwrap_or(fallback)
    }
}

/// Startup: read + parse the glue string table off the chain (after the chain exists).
///
/// Two reads, and the **order is the whole point**: the base file, then the locale patch over it,
/// exactly as `GlueXML.toc` sources them. A chain missing either one degrades on its own — no
/// base means built-in captions, no patch means the base text.
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

/// Parse the `KEY = "value";` assignments (one per line; `\n`/`\t`/`\"`/`\\` escapes unfolded).
/// Anything else — comments, code, multi-line constructs — is skipped.
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

/// The assignments inside `GlueLocalization.lua`'s **`Localize()`** — the locale patch the
/// reference lays over `GlueStrings.lua`.
///
/// Scoped to that one function deliberately, because the file defines two and only one of them is
/// a string table:
///
/// - **`Localize()`** is run by `GlueLocalization.xml`'s own inline `<Script>` (l.6) the moment the
///   file loads, which in `GlueXML.toc` is line 3 — after `GlueStrings.lua` at line 1. Its body is
///   nothing but `KEY = "value";`, and it is what this reads.
/// - **`LocalizeFrames()`** is called much later, from `GlueParent_OnEvent`'s `FRAMES_LOADED` arm
///   (`GlueParent.lua` l.95), and is frame surgery rather than text: in 1.12 enGB, a texture swap
///   and reposition of the login screen's `WorldOfWarcraftRating` logo — a frame benilla's native
///   glue does not have at all. Nothing there to apply, so nothing here reads it.
///
/// The body runs from `function Localize()` to the first **unindented** `end`, which is Blizzard's
/// own formatting: nested blocks are indented, a function's own terminator is not. A file whose
/// shape doesn't match yields no overrides rather than the wrong ones — the loader logs the count,
/// so a zero is visible rather than silent.
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
    // No terminator (or no `Localize()` at all): an unrecognised file patches nothing.
    HashMap::new()
}

/// Build the table the way [`load_glue_strings`] does — base file, then the locale patch over it.
/// Every test that resolves against the *real* chain goes through here, so no test can assert a
/// sentence the running client would not show.
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

    /// `Localize()`'s body, and **only** its body: the second function in the file is frame
    /// surgery that the reference calls from somewhere else entirely, so an assignment there must
    /// not leak into the string table.
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

    /// A file that is not the shape we understand patches **nothing** — never a partial or a
    /// guessed table. The two ways it can fail: no `Localize()` at all, and a body with no
    /// terminator to stop at.
    #[test]
    fn an_unrecognised_file_patches_nothing() {
        assert!(parse_localize_overrides("KEY = \"value\";\n").is_empty());
        assert!(parse_localize_overrides("function Localize()\n\tK = \"v\";\n").is_empty());
    }

    /// **The real chain's patch, applied in the reference's own order.** Reading only the base
    /// file is what decision 2052 fixed, and this is the regression: three keys whose base text
    /// and patched text differ, asserted from both ends so neither a lost patch nor a lost base
    /// can pass. Skips without client data.
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

        // The suffix is the one a player can read at a glance: the base file parenthesises it,
        // the enGB patch does not — `CharSelectRealmName` says "Realm PVP", not "Realm (PVP)".
        assert_eq!(
            base.get("PVP_PARENTHESES").map(String::as_str),
            Some("(PVP)")
        );
        assert_eq!(table.get("PVP_PARENTHESES"), Some("PVP"));

        // Every key the base file never had still resolves, and every key it had is still there:
        // the patch overwrites, it does not replace the table.
        assert!(table.get("FACTION_INFO_HORDE").is_some(), "base lost");
        assert!(base.len() > 100 && table.0.len() >= base.len());
    }
}
