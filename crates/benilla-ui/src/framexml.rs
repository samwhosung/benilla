//! The FrameXML **document** layer (decision 0068): turns a `.xml` file's text into an owned,
//! order-preserving tree, resolves `virtual`/`inherits` template expansion, and rewrites `$parent`
//! name tokens. This is XML-tree semantics only — no widget instantiation (frame factories, anchor
//! resolution, script compilation) and no Lua; those live in the layer above, which walks the tree
//! this module produces.
//!
//! Ground truth is the real client's loader (`0x6edc00–0x6f3000`):
//! - the file loader `0x6ede10` — the schema → frame-op interpretation, the full top-level +
//!   per-element op map
//! - `LoadChildFrames 0x76a060` — `<Frames>` / load-order (widget-layer concern; cited
//!   here only for the "spliced before the instance's own nodes" ordering that also governs template
//!   `<Frames>` children, which this module's generic child-concat already gets right)
//! - `0x76c5b0` — the `$parent` substitution rule
//!
//! XML *parsing* is delegated plumbing in the reference too (an embedded expat); the schema→op map
//! is its own — we use `roxmltree` rather than reproduce a tokenizer; only the schema→op
//! interpretation below is transcribed from the reference.

use std::collections::{HashMap, HashSet};
use std::fmt;

/// An owned XML element: tag, attributes (order preserved — Blizzard XML attribute order is
/// sometimes meaningful for authoring tools, and always useful for round-tripping/debugging), and
/// element children. Case-insensitive attribute lookup: "attrs via `GetAttribute 0x6f2cf0`" and
/// element-name compares are case-insensitive in the real loader (`0x64a4c0`), and Blizzard's own
/// shipped XML is inconsistent about attribute casing (e.g. both `relativeTo` and stray
/// all-lowercase variants appear in the wild), so lookups here fold case rather than assume a
/// canonical spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub tag: String,
    attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    /// The node's own direct text content (the XML node struct's `body-text @ +0xc`). Populated for
    /// text-bearing elements such as `<Scripts>` handler bodies (`<OnLoad>lua…</OnLoad>`) and
    /// inline `<Script>` bodies. Empty for purely structural elements.
    pub body: String,
}

impl Element {
    /// Case-insensitive attribute lookup (see struct docs).
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// A boolean attribute (`hidden="true"`, `virtual="true"`, …): true iff present and equal to
    /// the literal `"true"`, case-insensitively (`0x6f1b30`'s true-cmp).
    /// Absent or any other value is false — matching the client (there is no explicit `"false"`
    /// branch in the reference; only a `true` match flips the flag).
    pub fn attr_bool(&self, name: &str) -> bool {
        self.attr(name)
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }

    /// The same, but **presence-aware** — `None` when the attribute is absent.
    ///
    /// [`attr_bool`](Self::attr_bool) folds "absent" and `="false"` together, which is fine for a
    /// flag whose default is off and *silently wrong* for one whose default is on: it turns an
    /// explicit opt-out into a no-op. `<EditBox autoFocus="false">` is exactly that flag, and the
    /// shipped UI writes it ten times.
    pub fn attr_bool_opt(&self, name: &str) -> Option<bool> {
        self.attr(name).map(|v| v.eq_ignore_ascii_case("true"))
    }

    /// The `name` attribute, if any.
    pub fn name(&self) -> Option<&str> {
        self.attr("name")
    }

    /// All attributes, in document order, as written.
    pub fn attrs(&self) -> &[(String, String)] {
        &self.attrs
    }
}

/// A `<Script>` reference: an external file (`<Script file="…"/>`) or an inline body
/// (`<Script>lua…</Script>`). Both run in document order relative to everything else at the top
/// level (`0x704bc0` / `0x704cd0`, from `0x6ede10`'s walk — this is how XML-referenced FrameXML Lua
/// loads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptRef {
    File(String),
    Inline {
        body: String,
        /// The 1-based line **of this XML file** that `body`'s first character sits on.
        ///
        /// Carried so the loader can pad the chunk out to that offset, making Lua's own error
        /// lines the file's lines. Without it every inline block starts at line 1 and a raise
        /// reports a number belonging to no file — worse than no number, because it looks like one
        /// you could go and read (decision 1214).
        line: u32,
    },
}

/// One top-level item of a FrameXML document, in document order. Modeled as an order-preserving
/// `Vec<TopLevel>` rather than a split `{ scripts, fonts, templates, instances }` struct because the
/// real loader executes `<Include>`/`<Script>` interleaved with frame element definitions **in
/// document order** (`0x6ede10`) — a split-by-kind struct would lose that
/// order and misrepresent Lua load sequencing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopLevel {
    /// `<Include file="…"/>` — recurse-load another file at this point in the sequence
    /// (`0x8710c0`). File resolution/recursion is the loader
    /// layer's job; this only preserves *where* the include happens relative to everything else.
    Include(String),
    Script(ScriptRef),
    /// `<Font …>` — a named font definition (`0x87106c`).
    Font(Element),
    /// A frame/region element with `virtual="true"`: registered as a template keyed by `name`, not
    /// instantiated (`0x6ee500`).
    Template(Element),
    /// A frame/region element without `virtual="true"`: instantiated in place.
    Instance(Element),
}

/// The result of parsing one FrameXML document: its top-level items in order, plus any tolerable
/// issues found along the way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedDocument {
    pub items: Vec<TopLevel>,
    /// Tolerable issues: unregistered (unnamed) virtual templates, missing `Include`/`Font`
    /// attributes, a non-`<Ui>` root, and similar — the real client logs and continues rather than
    /// aborting the whole file for these, so parsing does too (see [`parse`]'s docs).
    pub warnings: Vec<String>,
}

impl ParsedDocument {
    /// All registered templates, keyed by name, for use with [`expand`]. A [`TopLevel::Template`]
    /// with no `name` (already flagged in [`ParsedDocument::warnings`] at parse time — an
    /// "Unnamed virtual node" load error in the real client, `0x6ee500`) is simply
    /// absent here, matching the fact that nothing can `inherits=` it.
    pub fn templates(&self) -> HashMap<&str, &Element> {
        self.items
            .iter()
            .filter_map(|item| match item {
                TopLevel::Template(el) => el.name().map(|name| (name, el)),
                _ => None,
            })
            .collect()
    }
}

/// A document parse error: malformed XML. Tolerable issues (unknown tags, missing optional
/// attributes, an unregistrable template) are not errors — see [`ParsedDocument::warnings`].
#[derive(Debug)]
pub enum Error {
    Xml(roxmltree::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Xml(e) => write!(f, "malformed FrameXML document: {e}"),
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

/// Parse FrameXML document text into an owned, order-preserving tree.
///
/// Top-level semantics (`0x6ede10`): the root's children are walked in
/// order; `<Include file=>` and `<Script file=>`/inline `<Script>` are preserved as-is (their
/// execution is the loader layer's job — this only fixes their position); `<Font>` becomes a font
/// definition; any other element with `virtual="true"` registers as a template (by `name`); anything
/// else is an instance. Unknown top-level tags (not one of the 13 known frame-element types) are
/// kept as generic [`Element`]s and classified the same way (template if `virtual="true"`, else
/// instance) — the real client's loader tolerates them the same way (widget-type validation happens
/// later, at instantiation, which is out of this module's scope).
///
/// A root element that isn't (case-insensitively) `<Ui>` is tolerated with a warning, not a hard
/// error: nothing establishes that the loader checks the root's own tag name, only that it walks
/// its children (`0x6ede10`).
pub fn parse(text: &str) -> Result<ParsedDocument, Error> {
    // **The reference has no XML Namespaces, and our stand-in parser does** (decision 2155).
    //
    // The client's document tree is `XMLTree.cpp`'s, built by embedded **expat 1.95.5** created
    // through `XML_ParserCreate 0x7e6690` — *not* `XML_ParserCreateNS` — so namespace processing
    // is off: a prefix is opaque bytes inside the attribute's name, an `xmlns:` declaration is an
    // ordinary attribute, and an *undeclared* prefix cannot be an error because nothing is
    // resolving one. The node struct the FrameXML loader walks is that same tree (name `@+0x8`,
    // the `{char*, char*}` attribute array `@+0x14`, looked up by `GetAttribute 0x6f2cf0`'s linear
    // case-insensitive scan).
    //
    // `roxmltree` is namespace-aware and has no switch for it, so it refuses
    // `<Ui xsi:schemaLocation="…">` with `UnknownNamespace` when the file never declares
    // `xmlns:xsi` — and takes the **whole document** with it. That is not a rare authoring slip:
    // Auctioneer writes exactly that line at the top of all five of its UI documents and
    // BeanCounter includes two of them, so the Auction House replacement lost its entire interface
    // to a concept the client it targets does not implement.
    //
    // So a namespace error is answered by giving the parser the binding it wants and re-reading —
    // driven by the prefix the error itself names, never by scanning the text for colons (which
    // would fire inside comments, CDATA and attribute values). The document's own bytes are
    // otherwise untouched, and [`element_from_node`] drops every prefixed attribute, so nothing
    // downstream can see the difference between a repaired document and one that never had a
    // prefix.
    let repaired;
    let doc = match roxmltree::Document::parse(text) {
        Ok(doc) => doc,
        Err(roxmltree::Error::UnknownNamespace(..)) => {
            repaired = bind_undeclared_prefixes(text);
            roxmltree::Document::parse(&repaired).map_err(Error::Xml)?
        }
        Err(e) => return Err(Error::Xml(e)),
    };
    let root = doc.root_element();

    let mut warnings = Vec::new();
    if !root.tag_name().name().eq_ignore_ascii_case("Ui") {
        warnings.push(format!(
            "root element is <{}>, not <Ui>; tolerated (the reference's file loader walks \
             the root's children regardless of the root's own tag)",
            root.tag_name().name()
        ));
    }

    let mut items = Vec::new();
    for child in root.children().filter(|n| n.is_element()) {
        let tag = child.tag_name().name();
        if tag.eq_ignore_ascii_case("Include") {
            match attr_ci(child, "file") {
                Some(file) => items.push(TopLevel::Include(file)),
                None => warnings.push("<Include> without a file attribute; skipped".to_string()),
            }
        } else if tag.eq_ignore_ascii_case("Script") {
            match attr_ci(child, "file") {
                Some(file) => items.push(TopLevel::Script(ScriptRef::File(file))),
                None => items.push(TopLevel::Script(ScriptRef::Inline {
                    body: direct_text(child),
                    line: inline_start_line(&doc, child),
                })),
            }
        } else if tag.eq_ignore_ascii_case("Font") {
            let el = element_from_node(child);
            if el.name().is_none() {
                warnings.push("<Font> without a name attribute".to_string());
            }
            items.push(TopLevel::Font(el));
        } else {
            let el = element_from_node(child);
            if el.attr_bool("virtual") {
                if el.name().is_none() {
                    warnings.push(format!(
                        "virtual <{}> without a name — a load error in the real client \
                         (\"Unnamed virtual node\"); kept but unregistered, \
                         so nothing can inherit it",
                        el.tag
                    ));
                }
                items.push(TopLevel::Template(el));
            } else {
                items.push(TopLevel::Instance(el));
            }
        }
    }

    Ok(ParsedDocument { items, warnings })
}

/// Declare every prefix the document uses but never binds, so a namespace-aware parser stops
/// having an opinion the reference's parser cannot have (decision 2155; the *why* is in
/// [`parse`]).
///
/// **Driven by the parser's own error, not by a scan.** `roxmltree` names the offending prefix in
/// `UnknownNamespace`, so each round adds exactly that one binding to the ROOT element's start tag
/// — where a declaration is in scope for the whole document, including the root's own name — and
/// re-parses. A text scan for `name:` would fire inside comments, CDATA and attribute values
/// (`Interface\AddOns\…` and `$Id:` lines are everywhere in this corpus), and would have to
/// re-implement the tokenizer to avoid it.
///
/// Bounded, and honest when it gives up: a document that keeps naming new prefixes stops after
/// [`MAX_BOUND_PREFIXES`] rounds and the text is returned as it stands, so [`parse`] reports the
/// parser's real error rather than looping. A document whose error is something else on the retry
/// (a genuinely malformed file that also has a stray prefix) reports that error, which is the one
/// worth reading.
pub(crate) fn bind_undeclared_prefixes(text: &str) -> String {
    let mut out = text.to_string();
    for _ in 0..MAX_BOUND_PREFIXES {
        let Err(roxmltree::Error::UnknownNamespace(prefix, _)) = roxmltree::Document::parse(&out)
        else {
            return out;
        };
        let Some(at) = root_attr_insertion_point(&out) else {
            return out;
        };
        // The URI is inert and never read: nothing downstream resolves a namespace, and
        // `element_from_node` drops prefixed attributes outright. It only has to be *a* URI, and
        // distinct per prefix so two stray prefixes are not silently the same namespace.
        out.insert_str(
            at,
            &format!(" xmlns:{prefix}=\"urn:benilla:undeclared:{prefix}\""),
        );
    }
    out
}

/// How many distinct undeclared prefixes [`bind_undeclared_prefixes`] will bind before it stops.
///
/// Generous against the real cause (one file, one stray `xsi:`) and small enough that a
/// pathological document cannot turn a failed parse into a long loop.
const MAX_BOUND_PREFIXES: usize = 8;

/// The byte offset inside the ROOT element's start tag at which a new attribute may be inserted —
/// just before the `>` or `/>` that closes it.
///
/// Walks the prolog by hand rather than parsing, because the document by definition does not parse
/// at this point: `<?…?>`, `<!--…-->` and `<!…>` are skipped, and the first remaining `<` opens the
/// root. Inside that tag, `'` and `"` runs are stepped over whole, so a `>` living in an attribute
/// value (`hyperlinkFormat="|H%s|h[%s]|h"` is the shape that exists in this corpus) cannot be
/// mistaken for the tag's end.
fn root_attr_insertion_point(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        if rest.starts_with("<?") {
            i += rest.find("?>").map(|n| n + 2)?;
        } else if rest.starts_with("<!--") {
            i += rest.find("-->").map(|n| n + 3)?;
        } else if rest.starts_with("<!") {
            i += rest.find('>').map(|n| n + 1)?;
        } else {
            // The root's start tag. Step over quoted runs so a `>` inside a value is not its end.
            let mut j = i + 1;
            while j < b.len() {
                match b[j] {
                    q @ (b'\'' | b'"') => {
                        j += 1;
                        while j < b.len() && b[j] != q {
                            j += 1;
                        }
                    }
                    b'>' => {
                        // Insert before a `/>`'s slash, not between the slash and the `>`.
                        let at = if j > 0 && b[j - 1] == b'/' { j - 1 } else { j };
                        return Some(at);
                    }
                    _ => {}
                }
                j += 1;
            }
            return None;
        }
    }
    None
}

fn attr_ci(node: roxmltree::Node, name: &str) -> Option<String> {
    node.attributes()
        .find(|a| a.name().eq_ignore_ascii_case(name))
        .map(|a| a.value().to_string())
}

/// Concatenates a node's own direct text/CDATA children (not descending into child elements) —
/// the XML node struct's `body-text @ +0xc`.
/// The 1-based line of the first text byte inside a `<Script>` element.
///
/// Taken from the first text child's own range rather than the element's, because a CDATA section
/// puts `<![CDATA[` between the two and every FrameXML script block in this project uses one — the
/// element's start line would be off by however much of the opening tag precedes the body.
/// Falls back to the element's line for a `<Script></Script>` with no text at all.
fn inline_start_line(doc: &roxmltree::Document, node: roxmltree::Node) -> u32 {
    let at = node
        .children()
        .find(|n| n.is_text())
        .unwrap_or(node)
        .range()
        .start;
    doc.text_pos_at(at).row
}

fn direct_text(node: roxmltree::Node) -> String {
    node.children()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect()
}

fn element_from_node(node: roxmltree::Node) -> Element {
    Element {
        tag: node.tag_name().name().to_string(),
        attrs: node
            .attributes()
            // **A prefixed attribute is unreachable in the reference, so it is not an attribute
            // here** (decision 2155). `GetAttribute 0x6f2cf0` compares a lookup key against the
            // stored name whole, and no FrameXML attribute name contains a colon — so
            // `xsi:schemaLocation` can never match any key the loader asks for. Dropping it is
            // what keeps a *declared* prefix (stock FrameXML's own `<Ui xmlns:xsi=… xsi:schema
            // Location=…>`) and an *undeclared* one (Auctioneer's) behave identically, and stops
            // `roxmltree`'s local-name view from making `xsi:name` answer a lookup for `name`.
            .filter(|a| a.namespace().is_none())
            .map(|a| (a.name().to_string(), a.value().to_string()))
            .collect(),
        children: node
            .children()
            .filter(|n| n.is_element())
            .map(element_from_node)
            .collect(),
        body: direct_text(node),
    }
}

/// Resolve `inherits="A, B"` template references into a fully materialized [`Element`]: a
/// structural merge with inherited content first, the element's own content last.
///
/// **Splice order** (in the reference, a matched template's stored attribute/child nodes are
/// spliced in first, then the instance's own nodes are read on top; `LoadChildFrames 0x76a060`
/// keeps the same "inherited-first" ordering for a template's own `<Frames>` children) — so:
/// - **children**: the (fully expanded) template's children first, in order, then the element's own
///   children appended after.
/// - **attributes**: the template's attributes, with the element's own attributes overriding any
///   attribute of the same name (case-insensitively) and adding any the template didn't have. This
///   is the literal reading of "spliced in first, then the instance's own" applied to attributes —
///   note this means an element that omits `name` entirely while inheriting a *named* template would
///   inherit that template's registration name, and `virtual="true"`/`inherits="…"` themselves are
///   ordinary attributes that splice through too (harmlessly inert post-expansion: nothing reads
///   them again after the one-time top-level virtual/instance routing decision, which already
///   happened on the *outer* node before expansion ever runs). Real FrameXML avoids ever hitting the
///   `name` case (instances either supply their own name or stay anonymous), and no
///   `name`/`virtual` exception is established for the reference, so this implementation does not
///   special-case them — flagged as an unverified edge case, not a silent guess.
///
/// **Multiple `inherits`** (`"A, B"`, comma-separated, left-to-right): each named template is
/// resolved and merged in order, so `B`'s content lands on top of `A`'s where they overlap, and the
/// element's own content lands on top of both. Multi-template precedence is not established from
/// the reference; left-to-right precedence (later name wins ties) is the natural reading
/// and is what this function implements — flagged, not guessed silently, since it isn't
/// byte-verified for the multi-name case specifically.
///
/// **Chains** (a template inheriting another template) recurse: each named template is itself fully
/// expanded (its own `inherits` resolved) before being merged in.
///
/// **Cycles** are guarded: if expanding a chain would re-enter a template already being expanded,
/// that reference is skipped and a warning is appended instead of infinite-recursing.
pub fn expand(
    element: &Element,
    templates: &HashMap<&str, &Element>,
    warnings: &mut Vec<String>,
) -> Element {
    expand_known(element, templates, &HashSet::new(), warnings)
}

/// [`expand`], plus the names that are known to be **font objects** rather than element templates.
///
/// A `<FontString inherits="GameFontNormalSmall">` names a font, not a template, and the font is
/// applied later by its own path (`apply_fontstring_font`) — so the name must be skipped here
/// silently rather than warned as unknown. `Loader::expand_region` already made that distinction
/// for an INSTANCE; it could not make it one level down, and stock `BuffFrame.xml` is exactly that
/// case: `BuffButtonDurationTemplate` is a virtual `<FontString>` that inherits a font object, so
/// every instance of it warned twenty-six times over (decision 1874).
pub fn expand_known(
    element: &Element,
    templates: &HashMap<&str, &Element>,
    fonts: &HashSet<&str>,
    warnings: &mut Vec<String>,
) -> Element {
    let mut active = HashSet::new();
    expand_inner(element, templates, fonts, warnings, &mut active)
}

fn expand_inner(
    element: &Element,
    templates: &HashMap<&str, &Element>,
    fonts: &HashSet<&str>,
    warnings: &mut Vec<String>,
    active: &mut HashSet<String>,
) -> Element {
    let Some(inherits) = element.attr("inherits") else {
        return element.clone();
    };

    // ── ONE name, matched case-INSENSITIVELY (template-name lookup `0x6ee6f0`) ─────────────────
    //
    // **No comma splitting.** 1.12's registry lookup `0x6ee6f0` has no splitter at all:
    // `inherits="A, B"` is ONE literal name, misses, and applies neither template. Supporting the
    // list is what LATER clients do, so splitting here made us a superset of 1.12 (1189). Nothing
    // uses it — 0 comma lists in our own assets, 0 across the 218-addon corpus, and 0 in stock
    // Blizzard XML's 249 distinct `inherits=` values.
    //
    // **Case-insensitive.** The compare is `SStrCmpI` → `_strnicmp` at `0x6ee747`, and the trap
    // that decides it is the BUCKET HASH: `SStrHash 0x64b3f0` uppercases before mixing, so a
    // mis-cased name lands in the same bucket and the stored-hash pre-check passes rather than
    // short-circuiting. `Recap`'s `inherits="UIDropdownMenuTemplate"` (lowercase d) resolves on the
    // real client. ASCII-only, like every other fold in this engine — bytes >= 0x80 are untouched.
    //
    // The fold is done as a scan on miss rather than by re-keying the view, because this same
    // function resolves the FONT registry too and both are name→element maps of the same shape.
    // Verbatim — not trimmed. `GetAttribute 0x6f2cf0` returns the parsed value as expat stored it
    // (`SStrDup`, no trim, no fold), and the empty test is a single `cmp [esi],0` on the first
    // byte: `inherits=""` is a silent skip, `inherits=" "` is a lookup that misses.
    let mut base: Option<Element> = None;
    for name in [inherits].into_iter().filter(|s| !s.is_empty()) {
        if !active.insert(name.to_string()) {
            warnings.push(format!(
                "inheritance cycle detected at template '{name}'; skipping this reference"
            ));
            continue;
        }
        let hit = templates.get(name).copied().or_else(|| {
            templates
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| *v)
        });
        let Some(template) = hit else {
            // A FONT OBJECT is not an unknown template: it is a different namespace, applied by
            // `apply_fontstring_font` after this. Skip it without a word (1874).
            if !fonts.contains(name) {
                warnings.push(format!(
                    "unknown template '{name}' referenced by inherits; skipping"
                ));
            }
            active.remove(name);
            continue;
        };
        let expanded_template = expand_inner(template, templates, fonts, warnings, active);
        active.remove(name);
        base = Some(match base {
            None => expanded_template,
            Some(acc) => merge(&acc, &expanded_template),
        });
    }

    match base {
        Some(base) => merge(&base, element),
        None => element.clone(),
    }
}

/// The structural merge documented on [`expand`]: `base`'s children first, `over`'s appended after;
/// `over`'s attributes override `base`'s of the same name (case-insensitive), and are added if
/// `base` didn't have them. `over`'s tag and body win (an instance's own tag/handler body, if any,
/// is what actually runs).
fn merge(base: &Element, over: &Element) -> Element {
    let mut attrs = base.attrs.clone();
    for (k, v) in &over.attrs {
        if let Some(slot) = attrs.iter_mut().find(|(ek, _)| ek.eq_ignore_ascii_case(k)) {
            slot.1 = v.clone();
        } else {
            attrs.push((k.clone(), v.clone()));
        }
    }

    let mut children = base.children.clone();
    children.extend(over.children.iter().cloned());

    Element {
        tag: over.tag.clone(),
        attrs,
        children,
        body: if over.body.is_empty() {
            base.body.clone()
        } else {
            over.body.clone()
        },
    }
}

/// The synthetic element a **runtime** `CreateFrame(kind, name, parent, "A, B")` expands: a bare
/// node of the caller's own `tag` carrying nothing but the `inherits=` list.
///
/// Handing this to [`expand`] is what makes the Lua path and the XML path *the same* path — one
/// comma-split, one chain resolution, one cycle guard, one unknown-template warning, one splice
/// order. Nothing about `inherits=` is written twice.
///
/// And because [`merge`] takes the **overriding** node's tag, the expanded result carries `tag` —
/// the kind `CreateFrame` was actually given — whatever the templates were declared as. That is
/// the honest answer to `CreateFrame("Frame", n, p, "SomeButtonTemplate")`: the frame already
/// exists as a Frame, so the template's `<Button>` tag cannot retype it, and the loader's per-kind
/// passes (which every one of them gate on the element tag) skip the parts that could not apply
/// anyway.
pub fn inherits_node(tag: &str, inherits: &str) -> Element {
    Element {
        tag: tag.to_string(),
        attrs: vec![("inherits".to_string(), inherits.to_string())],
        children: Vec::new(),
        body: String::new(),
    }
}

/// The literal fallback base name: when a `$parent`-prefixed name has no named ancestor to
/// substitute (e.g. a top-level element), the real client seeds the result with the literal string
/// `"Top"` (VA `0x8788ac`) rather than leaving the token literal, erroring, or producing an empty
/// name.
pub const DEFAULT_PARENT_NAME: &str = "Top";

/// `$parent` name-token substitution (`0x76c5b0`).
///
/// A `name` attribute beginning, **case-insensitively**, with the literal `"$parent"` (the
/// compare folds `A`–`Z`; `$Parent`/`$PARENT` all match — the *only* such token, there is no
/// `$parentKey`/`$parentN` variant) has that 7-character prefix replaced with
/// `parent_name`, and the remainder of `raw` appended verbatim (`SStrCat`). A `name` not starting
/// with the token is copied through unchanged.
///
/// `parent_name` must be the caller's **already-resolved** name for the nearest ancestor that has
/// one — i.e. the caller walks the ancestor chain (`this+0x9c`) for the first ancestor
/// whose own (already-substituted) name is non-empty, and passes [`DEFAULT_PARENT_NAME`] (`"Top"`)
/// if there is none. Passing the already-resolved name (not the raw ancestor `name=` attribute
/// text) is what makes nested `$parent` chains compose: a child of a
/// `$parent`-named parent inherits the parent's fully-resolved name, e.g. parent `PlayerFrame` +
/// `"$parentHealthBar"` → `"PlayerFrameHealthBar"`, and a grandchild's `"$parentBar"` resolves
/// against `"PlayerFrameHealthBar"`, not against the literal text `"$parentHealthBar"`.
///
/// Applies only to the `name` attribute — `parent=` (which *resolves* the enclosing
/// frame) is never `$parent`-substituted, and no other attribute is either.
pub fn resolve_name(raw: &str, parent_name: &str) -> String {
    const TOKEN: &str = "$parent";
    match raw.get(..TOKEN.len()) {
        Some(prefix) if prefix.eq_ignore_ascii_case(TOKEN) => {
            format!("{parent_name}{}", &raw[TOKEN.len()..])
        }
        _ => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **An undeclared namespace prefix is not an error, because the reference has no namespaces**
    /// (decision 2155). The client's tree is `XMLTree.cpp`'s, built by expat 1.95.5 through
    /// `XML_ParserCreate 0x7e6690` — never `XML_ParserCreateNS` — so nothing is resolving a prefix
    /// and nothing can find one unbound.
    ///
    /// The literal line is Auctioneer's, at the top of all five of its UI documents (and two of
    /// them are `<Include>`d by BeanCounter): a `roxmltree` `UnknownNamespace` there cost the
    /// Auction House replacement its entire interface, an element at a time, to a concept the
    /// client it targets does not implement.
    #[test]
    fn an_undeclared_namespace_prefix_does_not_cost_the_document() {
        let doc = parse(
            r#"<Ui xsi:schemaLocation="http://www.blizzard.com/wow/ui/">
                <Frame name="Survived"/>
            </Ui>"#,
        )
        .expect("the reference parses this, so we must");
        assert_eq!(doc.items.len(), 1);
        let TopLevel::Instance(el) = &doc.items[0] else {
            panic!("{:?}", doc.items[0])
        };
        assert_eq!(el.name(), Some("Survived"));
    }

    /// Two stray prefixes, and one of them on an ELEMENT rather than an attribute — the binding
    /// goes on the root, where it is in scope for the whole document including the root's own
    /// name, and the retry loop is driven by the parser naming each prefix in turn.
    #[test]
    fn several_undeclared_prefixes_are_all_bound() {
        let doc = parse(
            r#"<Ui xsi:schemaLocation="x" foo:bar="y">
                <Frame name="A"/>
                <bar:Frame name="B"/>
            </Ui>"#,
        )
        .expect("every stray prefix is bound, not just the first");
        assert_eq!(doc.items.len(), 2);
    }

    /// **A prefixed attribute is not an attribute** — `GetAttribute 0x6f2cf0` compares a lookup key
    /// against the stored name whole, and no FrameXML key contains a colon, so `xsi:name` can never
    /// answer a lookup for `name`. Asserted on a *declared* prefix as well, because stock FrameXML
    /// writes one on every `<Ui>` and the two spellings must behave identically.
    #[test]
    fn a_prefixed_attribute_never_answers_an_unprefixed_lookup() {
        for text in [
            r#"<Ui><Frame xsi:name="Ghost" text="real"/></Ui>"#,
            r#"<Ui xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
                 <Frame xsi:name="Ghost" text="real"/>
               </Ui>"#,
        ] {
            let doc = parse(text).unwrap();
            let TopLevel::Instance(el) = &doc.items[0] else {
                panic!()
            };
            assert_eq!(el.name(), None, "a prefixed name= is unreachable: {text}");
            assert_eq!(el.attr("text"), Some("real"), "{text}");
        }
    }

    /// A genuinely malformed document still fails — the repair answers `UnknownNamespace` and
    /// nothing else, so a mismatched tag is still the error a reader gets.
    #[test]
    fn a_malformed_document_still_fails_with_its_own_error() {
        assert!(parse(r#"<Ui xsi:a="b"><Frame></Ui>"#).is_err());
    }

    /// The insertion point is found in the ROOT's start tag, past a prolog and past a `>` that
    /// lives inside an attribute value — the shape `hyperlinkFormat="|H%s|h[%s]|h"` has in this
    /// corpus, and the reason this walks quotes rather than searching for the first `>`.
    #[test]
    fn the_root_tag_is_found_past_a_prolog_and_a_quoted_angle_bracket() {
        let doc = parse(
            "<?xml version=\"1.0\"?>\n<!-- a > in a comment -->\n\
             <Ui xsi:a=\"b\" note=\"a &gt; b\"><Frame name=\"Ok\"/></Ui>",
        )
        .unwrap();
        assert_eq!(doc.items.len(), 1);
        // …and a self-closing root: the attribute goes before the slash, not between it and `>`.
        assert!(parse(r#"<Ui xsi:a="b"/>"#).is_ok());
    }

    #[test]
    fn top_level_order_preserves_script_and_element_interleave() {
        let doc = parse(
            r#"<Ui>
                <Script file="A.lua"/>
                <Frame name="First"/>
                <Script>print("inline")</Script>
                <Include file="Other.xml"/>
                <Frame name="Second"/>
            </Ui>"#,
        )
        .unwrap();

        let kinds: Vec<&str> = doc
            .items
            .iter()
            .map(|item| match item {
                TopLevel::Include(_) => "include",
                TopLevel::Script(ScriptRef::File(_)) => "script-file",
                TopLevel::Script(ScriptRef::Inline { .. }) => "script-inline",
                TopLevel::Font(_) => "font",
                TopLevel::Template(_) => "template",
                TopLevel::Instance(_) => "instance",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "script-file",
                "instance",
                "script-inline",
                "include",
                "instance"
            ],
            "document order must be preserved, not grouped by kind"
        );

        match &doc.items[0] {
            TopLevel::Script(ScriptRef::File(f)) => assert_eq!(f, "A.lua"),
            other => panic!("expected file script, got {other:?}"),
        }
        assert!(doc.warnings.is_empty());
    }

    #[test]
    fn inline_vs_file_script() {
        let doc = parse(
            r#"<Ui>
                <Script file="Foo.lua"/>
                <Script>local x = 1
print(x)</Script>
            </Ui>"#,
        )
        .unwrap();
        assert_eq!(
            doc.items[0],
            TopLevel::Script(ScriptRef::File("Foo.lua".to_string()))
        );
        match &doc.items[1] {
            TopLevel::Script(ScriptRef::Inline { body, line }) => {
                assert!(body.contains("local x = 1"));
                assert!(body.contains("print(x)"));
                // Line 3 of the literal above: `<Ui>`, `<Script file=…>`, then this one.
                assert_eq!(*line, 3, "the inline body's own line in the file");
            }
            other => panic!("expected inline script, got {other:?}"),
        }
    }

    #[test]
    fn virtual_registers_as_template_non_virtual_as_instance() {
        let doc = parse(
            r#"<Ui>
                <Frame name="MyTemplate" virtual="true"/>
                <Frame name="MyInstance"/>
                <Frame VIRTUAL="TRUE" name="CasedTemplate"/>
            </Ui>"#,
        )
        .unwrap();
        assert!(matches!(doc.items[0], TopLevel::Template(_)));
        assert!(matches!(doc.items[1], TopLevel::Instance(_)));
        assert!(
            matches!(doc.items[2], TopLevel::Template(_)),
            "virtual attribute name/value compare is case-insensitive"
        );
        let templates = doc.templates();
        assert!(templates.contains_key("MyTemplate"));
        assert!(templates.contains_key("CasedTemplate"));
        assert_eq!(templates.len(), 2);
    }

    #[test]
    fn unnamed_virtual_warns_but_does_not_fail() {
        let doc = parse(r#"<Ui><Frame virtual="true"/></Ui>"#).unwrap();
        assert!(matches!(doc.items[0], TopLevel::Template(_)));
        assert_eq!(doc.warnings.len(), 1);
        assert!(
            doc.warnings[0].contains("Unnamed virtual node")
                || doc.warnings[0].contains("without a name")
        );
        assert!(
            doc.templates().is_empty(),
            "an unnamed template cannot be inherited"
        );
    }

    #[test]
    fn unknown_top_level_tag_is_tolerated_as_generic_element() {
        let doc = parse(r#"<Ui><TotallyMadeUpTag name="Whatever" foo="bar"/></Ui>"#).unwrap();
        match &doc.items[0] {
            TopLevel::Instance(el) => {
                assert_eq!(el.tag, "TotallyMadeUpTag");
                assert_eq!(el.attr("foo"), Some("bar"));
            }
            other => panic!("expected a generic instance, got {other:?}"),
        }
        assert!(doc.warnings.is_empty());
    }

    /// `inherits="A, B"` is **one name**, not a list — and it misses.
    ///
    /// This test used to assert the opposite (that both templates merged, children concatenated in
    /// order). That was a superset of 1.12 borrowed from later clients: there is no splitter
    /// anywhere in the loader — all 8 call sites hand `0x6ee6f0` the pointer `GetAttribute`
    /// returned, one or two instructions later, and no comma is examined in `0x6ed000–0x6f6000` at
    /// all. So the lookup runs **once**, for the literal name `"A, B"`, misses, warns at
    /// severity 1, and the element loads with **neither** template applied.
    ///
    /// Whitespace is never trimmed either, which is why the second half asserts `" A "` misses a
    /// template that genuinely exists.
    #[test]
    fn a_comma_list_is_one_literal_name_that_misses_and_applies_neither_template() {
        let doc = parse(
            r#"<Ui>
                <Frame name="A" virtual="true">
                    <Layers><Layer level="ARTWORK"><Texture name="FromA"/></Layer></Layers>
                </Frame>
                <Frame name="B" virtual="true" alpha="0.5">
                    <Layers><Layer level="ARTWORK"><Texture name="FromB"/></Layer></Layers>
                </Frame>
                <Frame name="Inst" inherits="A, B" alpha="1.0">
                    <Layers><Layer level="ARTWORK"><Texture name="FromInst"/></Layer></Layers>
                </Frame>
                <Frame name="Padded" inherits=" A "/>
            </Ui>"#,
        )
        .unwrap();
        let templates = doc.templates();
        let TopLevel::Instance(inst) = &doc.items[2] else {
            panic!("expected instance")
        };
        let mut warnings = Vec::new();
        let expanded = expand(inst, &templates, &mut warnings);

        // Warned, naming the whole unsplit string — not split, not silently ignored.
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("A, B"),
            "the warning must name the literal it looked up: {warnings:?}"
        );

        // The element still loads, with all of its OWN content and none of the templates'.
        assert_eq!(expanded.attr("alpha"), Some("1.0"));
        assert_eq!(expanded.children.len(), 1);
        fn texture_name(el: &Element) -> &str {
            el.children[0].children[0].attr("name").unwrap()
        }
        assert_eq!(texture_name(&expanded.children[0]), "FromInst");

        // And a padded name misses a template that exists — the value is used verbatim.
        let TopLevel::Instance(padded) = &doc.items[3] else {
            panic!("expected instance")
        };
        let mut w2 = Vec::new();
        let expanded = expand(padded, &templates, &mut w2);
        assert_eq!(w2.len(), 1, "{w2:?}");
        assert!(
            expanded.children.is_empty(),
            "picked up A despite the spaces"
        );
    }

    #[test]
    fn inheritance_chain_recurses() {
        let doc = parse(
            r#"<Ui>
                <Frame name="Root" virtual="true" frameStrata="LOW">
                    <Layers><Layer level="ARTWORK"><Texture name="RootTex"/></Layer></Layers>
                </Frame>
                <Frame name="Mid" virtual="true" inherits="Root" frameStrata="MEDIUM">
                    <Layers><Layer level="ARTWORK"><Texture name="MidTex"/></Layer></Layers>
                </Frame>
                <Frame name="Leaf" inherits="Mid"/>
            </Ui>"#,
        )
        .unwrap();
        let templates = doc.templates();
        let TopLevel::Instance(leaf) = &doc.items[2] else {
            panic!("expected instance")
        };
        let mut warnings = Vec::new();
        let expanded = expand(leaf, &templates, &mut warnings);
        assert!(warnings.is_empty());
        // Mid's own frameStrata (it overrode Root's) wins, since chain-expanding Mid resolves Root
        // first and Mid's own attrs on top, before Leaf (which sets nothing) merges on top of that.
        assert_eq!(expanded.attr("frameStrata"), Some("MEDIUM"));
        assert_eq!(
            expanded.children.len(),
            2,
            "Root's then Mid's <Layers>, in order"
        );
    }

    #[test]
    fn inheritance_cycle_warns_and_terminates() {
        let doc = parse(
            r#"<Ui>
                <Frame name="A" virtual="true" inherits="B"/>
                <Frame name="B" virtual="true" inherits="A"/>
                <Frame name="Inst" inherits="A"/>
            </Ui>"#,
        )
        .unwrap();
        let templates = doc.templates();
        let TopLevel::Instance(inst) = &doc.items[2] else {
            panic!("expected instance")
        };
        let mut warnings = Vec::new();
        // Must terminate (not infinitely recurse) and report the cycle.
        let _expanded = expand(inst, &templates, &mut warnings);
        assert!(
            warnings.iter().any(|w| w.contains("cycle")),
            "expected a cycle warning, got {warnings:?}"
        );
    }

    #[test]
    fn parent_name_substitution_basic_and_case_insensitive() {
        assert_eq!(
            resolve_name("$parentHealthBar", "PlayerFrame"),
            "PlayerFrameHealthBar"
        );
        assert_eq!(
            resolve_name("$PARENThealthbar", "PlayerFrame"),
            "PlayerFramehealthbar"
        );
        assert_eq!(resolve_name("$Parent", "PlayerFrame"), "PlayerFrame");
        assert_eq!(
            resolve_name("NotAParentName", "PlayerFrame"),
            "NotAParentName"
        );
        // No named ancestor: caller passes the literal "Top" fallback.
        assert_eq!(resolve_name("$parentFoo", DEFAULT_PARENT_NAME), "TopFoo");
    }

    #[test]
    fn parent_name_substitution_composes_through_nested_children() {
        // A named parent, a child using $parent, and a grandchild using $parent against the
        // child's *resolved* name — not the child's raw literal "$parent..." text.
        let parent_name = "PlayerFrame";
        let child_name = resolve_name("$parentHealthBar", parent_name);
        assert_eq!(child_name, "PlayerFrameHealthBar");
        let grandchild_name = resolve_name("$parentText", &child_name);
        assert_eq!(grandchild_name, "PlayerFrameHealthBarText");
    }
}
