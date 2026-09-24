//! **`Bindings.xml`** — an addon's key-binding declaration (decision 1188 phase 4).
//!
//! The reference loads one per addon, at a verified position inside `AddOn_Load 0x51f240`:
//! **after** that addon's `.toc`-listed files (`0x51f3fa`) and **before** its saved-variables
//! files (`0x51f400` → `0x51f4b5`). Both of benilla's load paths
//! attach there — the startup walk (`benilla_app::ui_script::addons`) and the demand load
//! ([`crate::script::addon`]) — and what they parse here feeds the same table the Key Bindings
//! window edits ([`crate::script::keybind`]).
//!
//! ## The format, read off the client's own file, not remembered
//!
//! `Interface\FrameXML\Bindings.xml` (extracted 1.12.1) is **228 live `<Binding>` elements**,
//! every one named, and the whole schema it uses is six attributes: `name` (228), `runOnUp` (94),
//! `header` (13), `hidden` (12), `debug` (9), `platform` (5).
//!
//! *Live*, because a text search says 234: six more sit inside an XML comment — Blizzard commented
//! out the whole `MOVEVIEW*` family in place. A parser that finds bindings by scanning text (and
//! any count taken by `grep`) registers those six as real, which is why the counts here are the
//! ones this module's own parse returns and why `a_commented_out_binding_is_not_a_binding` exists.
//! The counts are pinned against the install itself by
//! `benilla_app::bindings::commands::tests::the_installs_bindings_xml_reads_as_the_parsers_header_says`
//! — there, because there is where the patch chain is (this crate reads no install).
//!
//! ```xml
//! <Bindings>
//!     <Binding name="MOVEFORWARD" runOnUp="true" header="MOVEMENT">
//!         if ( keystate == "down" ) then MoveForwardStart(); else MoveForwardStop(); end
//!     </Binding>
//! </Bindings>
//! ```
//!
//! - **The body is ONE Lua chunk, run once or twice.** A `runOnUp="true"` binding runs on the
//!   press *and again* on the release, with the global `keystate` set to `"down"` / `"up"` — which
//!   is why every shipped `runOnUp` body is an `if ( keystate == "down" )` fork. A binding without
//!   it runs once, on the press. (Our own registry says the same thing a second way:
//!   `benilla_app::bindings::commands::Kind::EdgeUpDown` holds the two halves as two strings,
//!   because a host command's halves are ours to write. An addon's is one string and a global.)
//! - **`header` is a GLOBAL-STRING KEY, and it opens a SECTION.** `header="MOVEMENT"` means the
//!   category `BINDING_HEADER_MOVEMENT`, and every following binding *without* a `header` belongs
//!   to it: 13 headers over 228 bindings is 13 sections, and the client's own list is flat with
//!   `HEADER_*` pseudo-entries in it (`Blizzard_BindingUI.lua:87` tests
//!   `strsub(commandName, 1, 6) == "HEADER"`). The carry-forward is applied at *registration*, not
//!   here, so this type stays a faithful record of what the file says — see
//!   [`crate::script::UiScript::register_addon_bindings`].
//! - **`hidden="true"` keeps a binding out of the Key Bindings window** while leaving it bindable
//!   and dispatchable — 1.12 uses it for the debug toggles and for the three mouselook bindings
//!   (`TURNORACTION`, `CAMERAORSELECTORMOVE`, …) that the mouse owns rather than the player.
//! - **`platform="mac"` is the file's own OS gate**, and its five uses are the whole of it: the
//!   `ITUNES_REMOTE` block, which remotes the *system* music player and therefore exists only in
//!   the Mac build. It is carried on [`AddonBinding::platform`] and acted on at registration
//!   ([`crate::script::UiScript::register_addon_bindings`]) — a row for another platform is not
//!   registered at all, so it never reaches the Key Bindings window, a save file, or a chord.
//!   Registering it instead would list a command whose body calls functions this build does not
//!   have, which is a row that can only ever error on its first press.
//! - `debug="true"` is **read and not acted on**: it appears only on the client's own nine
//!   hidden dev toggles, always together with `hidden="true"` (which IS acted on and is what
//!   keeps them out of the window), and no addon in the wild uses it. Recording that it exists is
//!   the point; inventing a second meaning for it would be a guess.
//!
//! ## Why this lives beside `toc` and `framexml` rather than under `script/`
//!
//! It is the third of the client's **file formats**, and the crate's top level is where a format
//! becomes owned data with no VM in sight: [`crate::toc`] turns manifest text into a load list,
//! [`crate::framexml`] turns document text into a tree, and this turns binding text into
//! [`AddonBinding`]s. `script/` is the other side of that line — the live Lua-facing runtime,
//! which *consumes* what these three produce. A parser under `script/keybind` would also be
//! unreachable from the host's own load path without going through the VM, which is exactly the
//! shape 1186 spent a decision untangling.
//!
//! XML *parsing* is delegated plumbing here for the same reason it is in [`crate::framexml`] —
//! `roxmltree` reads the bytes, and only the schema→meaning map below is ours.

use std::fmt;

/// One `<Binding>` element, exactly as the file states it.
///
/// Deliberately a *record of the file*, not of the registered binding: `header` is the raw
/// attribute (`None` when the element carries none, which is 221 of the reference's 234), and the
/// section carry-forward and the `BINDING_HEADER_` prefixing happen where the addon's identity is
/// known — [`crate::script::UiScript::register_addon_bindings`]. (`None` for 215 of the
/// reference's 228 rows, which is what makes the carry-forward the whole story rather than a
/// detail.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonBinding {
    /// The `name` attribute — the command name `SetBinding`/`GetBindingKey` speak in.
    pub name: String,
    /// The `header` attribute's global-string *suffix* (`MOVEMENT` → `BINDING_HEADER_MOVEMENT`),
    /// when this element opens a section.
    pub header: Option<String>,
    /// `runOnUp="true"` — the body runs on the press **and** on the release (see the module doc).
    pub run_on_up: bool,
    /// `hidden="true"` — bindable and dispatchable, but not listed in the Key Bindings window.
    pub hidden: bool,
    /// `platform="mac"` — the row belongs to one build only, lower-cased. `None` on every row
    /// that carries no such attribute, which is every row but five.
    pub platform: Option<String>,
    /// The element's own text: one Lua chunk, verbatim (entities already decoded, so a body that
    /// wrote `&lt;` arrives as `<`).
    pub body: String,
}

/// A malformed document. Everything softer — an element with no `name`, an unknown attribute, an
/// unexpected root — is tolerated the way the real loader tolerates it; only bytes that are not
/// XML stop the file.
#[derive(Debug)]
pub enum Error {
    Xml(roxmltree::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Xml(e) => write!(f, "malformed Bindings.xml: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Xml(e) => Some(e),
        }
    }
}

/// Parse `Bindings.xml` text into its bindings, in document order.
///
/// **Every `<Binding>` under the root**, at any depth and matched case-insensitively like every
/// other element compare in this crate (`0x64a4c0`, see [`crate::framexml`]'s header). The shipped
/// root is `<Bindings>` and the elements are its direct children, but a descendant walk costs
/// nothing and cannot over-collect — a `<Binding>` never nests — while it does keep an addon that
/// wraps its bindings in a `<Ui>` root (or ships them inside a `<Bindings>` inside one) working.
///
/// **A `<Binding>` with no `name` is skipped**: there is nothing for `SetBinding` to name and
/// nothing for the reference's record to key on. Defensive rather than observed — all 228 of the
/// client's own are named.
pub fn parse(text: &str) -> Result<Vec<AddonBinding>, Error> {
    // Namespace-oblivious, like the FrameXML document layer beside it and for the same reason
    // (`framexml::parse`'s comment is the mechanism): a `Bindings.xml` is read by
    // the same `XMLTree.cpp` tree the client builds with expat's `XML_ParserCreate`, which has no
    // namespace processing at all — so an undeclared prefix cannot be an error there, and must not
    // cost the whole file here.
    let repaired;
    let doc = match roxmltree::Document::parse(text) {
        Ok(doc) => doc,
        Err(roxmltree::Error::UnknownNamespace(..)) => {
            repaired = crate::framexml::bind_undeclared_prefixes(text);
            roxmltree::Document::parse(&repaired).map_err(Error::Xml)?
        }
        Err(e) => return Err(Error::Xml(e)),
    };
    let mut out = Vec::new();
    for node in doc.root_element().descendants() {
        if !node.is_element() || !node.tag_name().name().eq_ignore_ascii_case("Binding") {
            continue;
        }
        let Some(name) = attr_ci(node, "name").filter(|n| !n.is_empty()) else {
            continue;
        };
        out.push(AddonBinding {
            name,
            header: attr_ci(node, "header").filter(|h| !h.is_empty()),
            run_on_up: attr_bool(node, "runOnUp"),
            hidden: attr_bool(node, "hidden"),
            platform: attr_ci(node, "platform")
                .filter(|p| !p.is_empty())
                .map(|p| p.to_ascii_lowercase()),
            body: direct_text(node),
        });
    }
    Ok(out)
}

/// Case-insensitive attribute lookup — Blizzard's own XML is inconsistent about attribute casing
/// and the real loader's `GetAttribute 0x6f2cf0` folds case ([`crate::framexml`]).
fn attr_ci(node: roxmltree::Node, name: &str) -> Option<String> {
    node.attributes()
        .find(|a| a.name().eq_ignore_ascii_case(name))
        .map(|a| a.value().to_string())
}

/// A boolean attribute: true **iff** present and equal to the literal `"true"`, case-insensitively
/// — the client's own rule (`0x6f1b30`'s true-cmp; there is no `"false"` branch, so `runOnUp="1"`
/// is false exactly as it is in the real loader).
fn attr_bool(node: roxmltree::Node, name: &str) -> bool {
    attr_ci(node, name).is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// The node's own direct text/CDATA children, concatenated — the binding's Lua chunk. Does not
/// descend into child elements, and so drops the `<!-- … -->` comments the shipped file is full
/// of. Concatenation (rather than the first text node) is what keeps a body containing an entity
/// whole: `a &lt; b` arrives from `roxmltree` as three text nodes.
fn direct_text(node: roxmltree::Node) -> String {
    node.children()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The reference's own shape**, element for element: the `runOnUp` + `header` opener with a
    /// multi-line `keystate` fork, a plain one-shot binding, a `hidden` debug toggle, and — the
    /// one case a hand-written addon file actually hits — an entity inside a body.
    ///
    /// What this catches: a parser that reads only the first text node (the `&lt;` body comes back
    /// truncated at the entity), one that treats `runOnUp="1"`/absent as true, one that swallows
    /// the newlines a Lua chunk needs to stay a Lua chunk, and one that hands back the `header`
    /// already prefixed (the prefixing is registration's job — a doubled
    /// `BINDING_HEADER_BINDING_HEADER_MOVEMENT` is the failure that would follow).
    #[test]
    fn the_reference_shape_parses_attribute_for_attribute() {
        let binds = parse(
            r#"<Bindings>
    <!-- User interface key bindings -->
    <Binding name="MOVEFORWARD" runOnUp="true" header="MOVEMENT">
        if ( keystate == "down" ) then
            MoveForwardStart();
        else
            MoveForwardStop();
        end
    </Binding>
    <Binding name="JUMP">
        Jump();
    </Binding>
    <Binding name="TOGGLESTATS" hidden="true" debug="true">
        ToggleStats();
    </Binding>
    <Binding name="PROBECOMPARE" RUNONUP="TRUE">
        if ( a &lt; b ) then Probe(); end
    </Binding>
    <Binding name="PROBELOOSE" runOnUp="1">
        Probe();
    </Binding>
    <Binding name="ITUNES_PLAYPAUSE" header="ITUNES_REMOTE" platform="mac">
        MusicPlayer_PlayPause();
    </Binding>
</Bindings>"#,
        )
        .expect("well-formed");

        assert_eq!(
            binds.len(),
            6,
            "one per <Binding>, comments are not bindings"
        );

        let fwd = &binds[0];
        assert_eq!(fwd.name, "MOVEFORWARD");
        assert_eq!(
            fwd.header.as_deref(),
            Some("MOVEMENT"),
            "the RAW attribute — BINDING_HEADER_ is registration's prefix to add, not ours"
        );
        assert!(fwd.run_on_up && !fwd.hidden);
        assert_eq!(fwd.platform, None, "no platform= is every row but five");
        let itunes = &binds[5];
        assert_eq!(itunes.name, "ITUNES_PLAYPAUSE");
        assert_eq!(
            itunes.platform.as_deref(),
            Some("mac"),
            "platform= is the file's own OS gate; registration acts on it"
        );
        assert!(fwd.body.contains("MoveForwardStart();"));
        assert!(fwd.body.contains("MoveForwardStop();"));
        assert!(
            fwd.body.contains('\n'),
            "the body is a Lua chunk — its newlines are load-bearing"
        );

        // A binding with no header of its own carries none: the section it belongs to is a
        // property of the LIST, resolved at registration.
        assert_eq!(binds[1].name, "JUMP");
        assert_eq!(binds[1].header, None);
        assert!(!binds[1].run_on_up);

        assert!(binds[2].hidden, "hidden=\"true\" is recorded, not dropped");
        assert!(!binds[2].run_on_up);

        // Attribute name AND value fold case (the loader's own compares do); the entity is
        // decoded and the body around it survives whole.
        assert!(binds[3].run_on_up, "RUNONUP=\"TRUE\" is runOnUp");
        assert_eq!(binds[3].body.trim(), "if ( a < b ) then Probe(); end");

        // ...but only the literal `true` is true — there is no `"false"` branch in the client's
        // parse, so `"1"` is simply not it.
        assert!(
            !binds[4].run_on_up,
            "runOnUp=\"1\" is false: the client's bool compares against \"true\" alone"
        );
    }

    /// A nameless `<Binding>` is skipped rather than registered under `""` — where it would eat
    /// `GetBindingAction`'s "no command is bound to this key" answer, which IS the empty string.
    /// A `<Binding>` nested under a wrapper element still counts, so an addon that wraps its
    /// bindings does not silently lose them.
    #[test]
    fn nameless_bindings_are_skipped_and_nesting_is_tolerated() {
        let binds = parse(
            r#"<Ui>
    <Binding>Orphan();</Binding>
    <Binding name="">AlsoOrphan();</Binding>
    <Bindings>
        <Binding name="PROBEWRAPPED">Probe();</Binding>
    </Bindings>
</Ui>"#,
        )
        .expect("well-formed");
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].name, "PROBEWRAPPED");
    }

    /// **A commented-out `<Binding>` is not a binding** — and this is not a hypothetical: the
    /// client's own file carries the entire `MOVEVIEW*` family (six bindings) inside one XML
    /// comment, which is the whole gap between the 234 elements a text search finds and the 228
    /// the loader sees. Registering them would put six commands in the Key Bindings window that
    /// the client does not have, each one a key a player could bind to nothing at all.
    #[test]
    fn a_commented_out_binding_is_not_a_binding() {
        let binds = parse(
            r#"<Bindings>
    <Binding name="PROBELIVE">Probe();</Binding>
    <!--
    <Binding name="MOVEVIEWIN" runOnUp="true">
        if ( keystate == "down" ) then MoveViewInStart(); else MoveViewInStop(); end
    </Binding>
    -->
</Bindings>"#,
        )
        .expect("well-formed");
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].name, "PROBELIVE");
        assert_eq!(
            binds[0].body.trim(),
            "Probe();",
            "a comment between elements is not body text either"
        );
    }

    /// Bytes that are not XML stop the file — the one hard error. The addon's other files have
    /// already run by then (`Bindings.xml` loads after them, `0x51f400`), so the load path reports
    /// this and carries on; what it must not do is silently register nothing.
    #[test]
    fn malformed_xml_is_an_error() {
        let e = parse("<Bindings><Binding name=\"X\"></Bindings>").expect_err("malformed");
        assert!(e.to_string().starts_with("malformed Bindings.xml:"));
    }
}
