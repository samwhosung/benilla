//! A `Bindings.xml`, its key-binding declarations, as the one loader both uses (`0x4b6f70`) reads
//! it: the core's `Interface\FrameXML\Bindings.xml` off the player's chain, which `UI_Init`
//! (`0x48fbf0`) loads at `0x490018` after the `FrameXML.toc` walk (`0x48ffed`), and each addon's,
//! which `AddOn_Load` (`0x51f240`) loads at `0x51f443` after the addon's `.toc` files and before
//! its saved variables. The stock file has 219 commands (94 `runOnUp`, 13 `header`, 3 `hidden`, 5
//! `platform`) once its nine `debug` rows are skipped; the six `MOVEVIEW*` ones sit in an XML
//! comment.
//!
//! A body is one Lua chunk; with `runOnUp="true"` it runs on press and again on release, the
//! global `keystate` set to `"down"` or `"up"`. `header="X"` opens the section
//! `BINDING_HEADER_X` that following header-less bindings join, in a flat list with `HEADER`
//! pseudo-entries (`Blizzard_BindingUI.lua:87`). `hidden="true"` keeps a binding bindable but out
//! of the Key Bindings window. `platform="mac"` marks the `ITUNES_REMOTE` rows; the loader skips
//! another platform's row (`0x4b70c3`-`0x4b70e5`), here at registration.

use std::fmt;

/// One `<Binding>` element as the file states it; the section carry-forward and the
/// `BINDING_HEADER_` prefix are applied at registration ([`crate::script::UiScript::register_bindings`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The command name `SetBinding` and `GetBindingKey` speak in.
    pub name: String,
    /// The raw `header` suffix (`MOVEMENT` of `BINDING_HEADER_MOVEMENT`), opening a section.
    pub header: Option<String>,
    /// `runOnUp="true"`: the body runs on press and on release.
    pub run_on_up: bool,
    /// `hidden="true"`: bindable, but not listed in the Key Bindings window.
    pub hidden: bool,
    /// `platform=`, lower-cased: the one build the row belongs to.
    pub platform: Option<String>,
    /// The element's own text, one Lua chunk, entities decoded (`&lt;` arrives as `<`).
    pub body: String,
}

/// Bytes that are not XML. A nameless element, an unknown attribute or an unexpected root is
/// tolerated, as the reference's loader tolerates them.
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

/// Parse `Bindings.xml` text into its bindings, in document order, with the loader's own skips.
/// It walks the root's children only (`0x4b701f`, then the sibling link at `0x4b745b`), so a
/// nested `<Binding>` is never read; a child that is not a `<Binding>` (the tag matched
/// case-insensitively, `0x64a4c0`) is skipped (`0x4b704c`). A row with no `name`
/// (`0x4b7084`-`0x4b708d`), an empty body (`0x4b7093`-`0x4b709e`) or `debug="true"`
/// (`0x4b70a4`-`0x4b70bd`) is skipped too, so the stock file's nine debug rows are not commands.
pub fn parse(text: &str) -> Result<Vec<Binding>, Error> {
    // The reference's expat has no namespace processing, so an undeclared prefix is bound and the
    // text re-read rather than costing the file, as in `framexml::parse`.
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
    for node in doc.root_element().children() {
        if !node.is_element() || !node.tag_name().name().eq_ignore_ascii_case("Binding") {
            continue;
        }
        let Some(name) = attr_ci(node, "name").filter(|n| !n.is_empty()) else {
            continue;
        };
        let body = direct_text(node);
        if body.is_empty() || attr_bool(node, "debug") {
            continue;
        }
        out.push(Binding {
            name,
            header: attr_ci(node, "header").filter(|h| !h.is_empty()),
            run_on_up: attr_bool(node, "runOnUp"),
            hidden: attr_bool(node, "hidden"),
            platform: attr_ci(node, "platform")
                .filter(|p| !p.is_empty())
                .map(|p| p.to_ascii_lowercase()),
            body,
        });
    }
    Ok(out)
}

/// Case-insensitive attribute lookup, as the reference's `GetAttribute 0x6f2cf0` folds case.
fn attr_ci(node: roxmltree::Node, name: &str) -> Option<String> {
    node.attributes()
        .find(|a| a.name().eq_ignore_ascii_case(name))
        .map(|a| a.value().to_string())
}

/// True only for a case-insensitive `"true"` (`0x6f1b30` has no `"false"` branch), so
/// `runOnUp="1"` is false.
fn attr_bool(node: roxmltree::Node, name: &str) -> bool {
    attr_ci(node, name).is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// The node's direct text and CDATA, concatenated: `roxmltree` splits `a &lt; b` into three text
/// nodes.
fn direct_text(node: roxmltree::Node) -> String {
    node.children()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    <Binding name="PROBEHIDDEN" hidden="true">
        Probe();
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

        // Section membership is resolved at registration, not here.
        assert_eq!(binds[1].name, "JUMP");
        assert_eq!(binds[1].header, None);
        assert!(!binds[1].run_on_up);

        assert!(binds[2].hidden, "hidden=\"true\" is recorded, not dropped");
        assert!(!binds[2].run_on_up);

        // Attribute names and values fold case; the entity decodes and the body stays whole.
        assert!(binds[3].run_on_up, "RUNONUP=\"TRUE\" is runOnUp");
        assert_eq!(binds[3].body.trim(), "if ( a < b ) then Probe(); end");

        assert!(
            !binds[4].run_on_up,
            "runOnUp=\"1\" is false: the client's bool compares against \"true\" alone"
        );
    }

    /// Registered under `""`, a nameless binding would shadow `GetBindingAction`'s empty answer
    /// for an unbound key; the loader skips it (`0x4b7084`-`0x4b708d`).
    #[test]
    fn nameless_bindings_are_skipped() {
        let binds = parse(
            r#"<Ui>
    <Binding>Orphan();</Binding>
    <Binding name="">AlsoOrphan();</Binding>
    <Binding name="PROBENAMED">Probe();</Binding>
</Ui>"#,
        )
        .expect("well-formed");
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].name, "PROBENAMED");
    }

    /// The loader's three skips beyond a missing name: a `<Binding>` below the root's children is
    /// never walked (`0x4b701f`, the sibling link `0x4b745b`), an empty body is refused
    /// (`0x4b7093`-`0x4b709e`), and a truthy `debug` skips the row (`0x4b70a4`-`0x4b70bd`), as
    /// the stock file's nine debug rows are skipped. A whitespace body is not empty.
    #[test]
    fn nested_empty_and_debug_rows_are_not_bindings() {
        let binds = parse(
            r#"<Bindings>
    <Binding name="PROBEKEPT">Probe();</Binding>
    <Bindings>
        <Binding name="PROBENESTED">Probe();</Binding>
    </Bindings>
    <Frame><Binding name="PROBEINFRAME">Probe();</Binding></Frame>
    <Binding name="PROBEEMPTY"></Binding>
    <Binding name="PROBESELFCLOSED"/>
    <Binding name="PROBEDEBUG" hidden="true" debug="true">ToggleStats();</Binding>
    <Binding name="PROBEDEBUGCASE" DEBUG="TRUE">Probe();</Binding>
    <Binding name="PROBENOTDEBUG" debug="1">Probe();</Binding>
    <Binding name="PROBEBLANK">  </Binding>
</Bindings>"#,
        )
        .expect("well-formed");
        let names: Vec<&str> = binds.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["PROBEKEPT", "PROBENOTDEBUG", "PROBEBLANK"]);
    }

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

    #[test]
    fn malformed_xml_is_an_error() {
        let e = parse("<Bindings><Binding name=\"X\"></Bindings>").expect_err("malformed");
        assert!(e.to_string().starts_with("malformed Bindings.xml:"));
    }
}
