//! The FrameXML loader: builds live frames from a parsed document ([`crate::framexml`]) by driving
//! the Lua object model ([`crate::script`]) as an addon does, never the arena directly.
//!
//! The caller's `files` provider supplies bytes, so the loader does no IO; a `<Script file=>`
//! chunk reaches Lua as the bytes on disk, as `luaL_loadbuffer` receives them. The loader joins a
//! relative path to its file's directory ([`join_ref`]); the provider decides what it may reach.
//!
//! Lua handles are never accumulated breadth-wise (MAXCSTACK): a frame's wrapper and `OnLoad`
//! live only across its own subtree build, and everything durable lives Lua-side.

use std::collections::HashSet;

use mlua::{Function, ObjectLike, Table, Value};

use crate::framexml::{self, Element, ParsedDocument, ScriptRef, TopLevel};
use crate::justify;
use crate::layout::Len;
use crate::script::{Outline, UiScript};
use crate::status;

mod backdrop;
mod geometry;
mod regions;
mod scripts;
mod widgets;

impl LoadReport {
    /// A load fault that is no script raise: the handler hears the row's own text.
    pub(super) fn fault(&mut self, text: String) {
        self.handler_errors.push(text.clone());
        self.errors.push(text);
    }

    /// A script raise: the row is `text`, the handler hears the bare Lua message of `e`.
    pub(super) fn raise(&mut self, text: String, e: &mlua::Error) {
        self.handler_errors.push(crate::script::lua_message(e));
        self.errors.push(text);
    }

    /// Each of [`Self::errors`] with what the Lua error handler is called with for it.
    pub fn raised(&self) -> impl Iterator<Item = (&str, &str)> {
        debug_assert_eq!(
            self.errors.len(),
            self.handler_errors.len(),
            "every error row is written through `fault` or `raise`"
        );
        self.errors
            .iter()
            .map(String::as_str)
            .zip(self.handler_errors.iter().map(String::as_str))
    }
}

/// The outcome of a [`load`]; nothing aborts a load, as the reference logs and continues.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// Issues that dropped nothing, and the document layer's parse warnings. An unknown frame type
    /// drops its node yet lands here: the reference logs it and goes on (`0x6ee356`).
    pub warnings: Vec<String>,
    /// Failures the reference raises on: a handler that fails to compile, a method call that
    /// errors, a malformed included document.
    pub errors: Vec<String>,
    /// What the Lua error handler is called with for each of [`Self::errors`], row for row
    /// ([`Self::raised`]): the bare Lua message of a raise, as `lua_pcall` leaves it for the
    /// reference's error function (`0x703b40`), and the row's own text for a fault that is no
    /// script raise.
    handler_errors: Vec<String>,
    /// `<Include file=>` and `<Script file=>` paths the provider lacks. The reference logs
    /// `"Couldn't open %s"` (`0x6edaa0`, `0x846ff4`) or `"Error loading %s"` (`0x872e50`) and goes
    /// on (`0x6ee00d`); kept apart from `warnings` so a load that resolved nothing is not clean.
    pub missing_files: Vec<String>,
    /// Frame instances `CreateFrame` built.
    pub frames: usize,
    /// Trace lines, only while `FrameXML_Debug` is on: the reference's flag (`[0xceea30]`) boots 0
    /// and its loader traces gate on `flag > 0` (`0x6ee298`). A trace is severity 0, not a warning.
    pub traces: Vec<String>,
    /// The document's own load status, the lines `0x6ede10` reports into its record for
    /// `Logs\FrameXML.log`: the traces, each `<Include>` or `<Script file=>` that did not open or
    /// parse, and each included document's own record under its banner. The caller closes it
    /// under the document's banner ([`crate::status::Status::close_into`]).
    pub status: crate::status::Status,
}

/// Materialize a parsed FrameXML document into live frames in `script`, in document order
/// (`0x6ede10`). `files` returns a path's bytes; a `None` is a [`LoadReport::missing_files`] row.
pub fn load(
    script: &UiScript,
    doc: &ParsedDocument,
    files: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> LoadReport {
    load_in(script, doc, "", files)
}

/// [`load`] for a document at `path` in the provider's path space (`/`-separated, `""` for none),
/// which its relative references resolve against and its inline `<Script>` chunks are named after.
pub fn load_in(
    script: &UiScript,
    doc: &ParsedDocument,
    path: &str,
    files: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> LoadReport {
    load_into(script.lua(), doc, path, files)
}

/// [`load_in`] on the VM directly, for a Lua binding (`LoadAddOn`), which holds only `&Lua`.
pub fn load_into(
    lua: &mlua::Lua,
    doc: &ParsedDocument,
    path: &str,
    files: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> LoadReport {
    load_into_under(lua, doc, path, files, "")
}

/// [`load_into`] for a provider whose path space sits under `status_root` in the install's, so
/// the status lines name the install path (`LoadAddOn` reads under `Interface\AddOns\`).
pub fn load_into_under(
    lua: &mlua::Lua,
    doc: &ParsedDocument,
    path: &str,
    files: &dyn Fn(&str) -> Option<Vec<u8>>,
    status_root: &str,
) -> LoadReport {
    let mut loader = Loader {
        lua,
        files,
        path: path.to_string(),
        status_root,
        report: LoadReport::default(),
        warned: HashSet::new(),
        deferred_anchors: Vec::new(),
    };
    loader.report.warnings.extend(doc.warnings.iter().cloned());
    loader.load_doc(doc);
    loader.report
}

/// Apply a template to an existing frame, `CreateFrame`'s fourth argument. Returns the report:
/// its errors are script errors for the error handler, as the reference's `CreateFrame` builds the
/// template through the XML instantiator `0x6ee280`, whose OnLoad runs under the error function
/// (`0x704d50`). The frame always survives an error, and only virtual elements are registered; an
/// unknown name never gets here, since the binding raises first. The frame's own name is the `$parent`
/// base (`MyButton` makes `$parentText` `MyButtonText`). Runs inside the `CreateFrame` binding, so
/// it enters at [`Loader::decorate`]: [`Loader::materialize`] would recurse.
pub fn apply_template(lua: &mlua::Lua, wrapper: &Table, kind: &str, template: &str) -> LoadReport {
    let template = template.trim();
    if template.is_empty() {
        return LoadReport::default();
    }
    // A template never reads a file: `<Include>` and `<Script file=>` are top-level only.
    let no_files = |_: &str| -> Option<Vec<u8>> { None };
    let mut loader = Loader {
        lua,
        files: &no_files,
        path: String::new(),
        status_root: "",
        report: LoadReport::default(),
        warned: HashSet::new(),
        deferred_anchors: Vec::new(),
    };

    let (own_name, parent_name) = loader.frame_names(wrapper);
    let self_name = own_name.clone().unwrap_or_else(|| parent_name.clone());
    let dbg = format!(
        "CreateFrame(\"{kind}\", \"{}\", inherits=\"{template}\")",
        own_name.as_deref().unwrap_or("<unnamed>")
    );

    // Diagnostic only: `expand` does not say what kind a template was declared as.
    let mut notes = Vec::new();
    let mut any_resolved = false;
    {
        let model = loader.model();
        let templates = model.framexml_templates.borrow();
        for name in [template].into_iter().filter(|s| !s.is_empty()) {
            let Some(el) = templates.get(name) else {
                continue;
            };
            any_resolved = true;
            let tag = &el.tag;
            if tag.eq_ignore_ascii_case("Texture") || tag.eq_ignore_ascii_case("FontString") {
                notes.push(format!(
                    "{dbg}: '{name}' is a <{tag}> REGION template, not a frame template; its \
                     frame-shaped content still applies, its region-only content cannot"
                ));
            } else if !tag.eq_ignore_ascii_case(kind) {
                notes.push(format!(
                    "{dbg}: '{name}' is a <{tag}>, but a {kind} was created; the frame keeps the \
                     kind CreateFrame was given, so the <{tag}>-only parts of the template do not \
                     apply"
                ));
            }
        }
    }
    if !any_resolved {
        notes.push(format!(
            "{dbg}: no template of that name is registered — the frame exists and is usable, but \
             it is bare (only a `virtual=\"true\"` element is ever registered as a template)"
        ));
    }
    loader.report.warnings.extend(notes);

    // The XML `inherits=` expansion; its warnings get the `CreateFrame` call as a prefix.
    let node = framexml::inherits_node(kind, template);
    let first = loader.report.warnings.len();
    let expanded = loader.expand(&node);
    for w in &mut loader.report.warnings[first..] {
        *w = format!("{dbg}: {w}");
    }
    let levels = loader.levels(&node);

    loader.decorate(&expanded, &levels, wrapper, &self_name, &parent_name, &dbg);

    loader.report
}

/// Join a FrameXML reference to its file's directory, lexically: `\` and `/` both separate, `..`
/// walks up. A `..` above the root is kept, not clamped, so the provider can refuse the escape.
pub fn join_ref(base: &str, path: &str) -> String {
    let path = path.replace('\\', "/");
    let combined = if path.starts_with('/') || base.is_empty() {
        path.trim_start_matches('/').to_string()
    } else {
        format!("{base}/{path}")
    };
    let mut out: Vec<&str> = Vec::new();
    for seg in combined.split('/') {
        match seg {
            "" | "." => {}
            ".." => match out.last() {
                Some(&"..") | None => out.push(".."),
                _ => {
                    out.pop();
                }
            },
            s => out.push(s),
        }
    }
    out.join("/")
}

/// A `CSimpleModel` kind, whose `LoadXML` (`0x76cac0`) reads `scale=` as the model's scale;
/// `ModelFFX` shares it (its second vtable's slot 2).
pub(super) fn model_kind_tag(tag: &str) -> bool {
    crate::script::frame_kind_from_tag(tag).is_some_and(crate::widget::FrameKind::is_model)
}

impl Loader<'_> {
    /// A provider path as a status line names it: under [`Loader::status_root`], `\`-separated.
    fn status_path(&self, path: &str) -> String {
        status::install_path(&format!("{}{path}", self.status_root))
    }

    /// Mark the loader's `CreateFrame` call, the builder's creation half (`Model::xml_creating`).
    fn set_xml_creating(&self, on: bool) {
        self.model().xml_creating = on;
    }

    /// `FrameXML_Debug` above 0, read where the reference reads it, at each close.
    fn debug(&self) -> bool {
        self.model().framexml_debug.get() > 0
    }
}

/// The directory part of a provider path, `""` for a bare name.
fn dir_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

struct Loader<'a> {
    /// The VM, not a `UiScript`: a Lua binding such as `LoadAddOn` holds only `&Lua`, and loads
    /// an addon synchronously from inside the binding, as the reference does.
    pub(super) lua: &'a mlua::Lua,
    pub(super) files: &'a dyn Fn(&str) -> Option<Vec<u8>>,
    /// The current document's path: its directory resolves relative references, the whole path
    /// names inline `<Script>` chunks. Swapped for each nested include and restored after.
    pub(super) path: String,
    /// What the provider's path space sits under in the install's, for the status lines.
    pub(super) status_root: &'a str,
    // The template and font registries live on the `Model` and persist across loads: the
    // reference's template table is global (`0x6ee500`), so `MerchantFrame.xml` inherits a template
    // `CharacterFrameTemplates.xml` registered. Fonts are their own namespace.
    pub(super) report: LoadReport,
    /// Warn-once keys.
    pub(super) warned: HashSet<String>,
    /// Anchors whose named `relativeTo` did not exist yet, applied once the enclosing frame's
    /// subtree is built: the reference resolves anchors at layout, this loader at `SetPoint`, so a
    /// target built later waits here (`ClassTrainerFrameTemplates.xml:67` hangs the label off the
    /// highlight). Each `decorate` drains its own.
    pub(super) deferred_anchors: Vec<DeferredAnchor>,
}

/// The nodes an element's `LoadXML` reads, in the order it reads them: the reference merges no
/// template into an instance, but re-enters `LoadXML` on each `inherits` node first (`0x76985c`
/// for a frame, `0x76fe5b` for a texture, `0x770f9e` for a FontString), then reads the node's own
/// attributes and its own first child of each kind (`0x6f2c50`). So an attribute whose rule is
/// local to a node (`setAllPoints` beside the node's own `<Anchors>`, a `font=` with its own
/// `<FontHeight>`) is read level by level, the template first and the element last;
/// [`Loader::expand`]'s merged element serves the rest. The templates are shared with the
/// registry, not copied.
pub(super) struct Levels<'a> {
    inherited: Vec<std::rc::Rc<Element>>,
    own: &'a Element,
}

type LevelIter<'l> = std::iter::Chain<
    std::iter::Map<
        std::slice::Iter<'l, std::rc::Rc<Element>>,
        fn(&std::rc::Rc<Element>) -> &Element,
    >,
    std::iter::Once<&'l Element>,
>;

impl Levels<'_> {
    pub(super) fn iter(&self) -> LevelIter<'_> {
        let shared: fn(&std::rc::Rc<Element>) -> &Element = |e| e;
        self.inherited
            .iter()
            .map(shared)
            .chain(std::iter::once(self.own))
    }
}

impl<'l> IntoIterator for &'l Levels<'_> {
    type Item = &'l Element;
    type IntoIter = LevelIter<'l>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// One `SetPoint` held until its target exists.
pub(super) struct DeferredAnchor {
    pub(super) wrapper: Table,
    /// A region wrapper (`call_region`) rather than a frame's (`call`).
    pub(super) region: bool,
    /// `(point, relativeTo, relativePoint)`, `relativeTo` with `$parent` expanded.
    pub(super) args: (String, Option<String>, String),
    /// The anchor's first `<Offset>`, read only once the target passes (`0x767992`).
    pub(super) offset: Option<Element>,
    /// `relativeTo` as spelled, before `$parent` expands: what the anchor lines print (`0x76796f`,
    /// `0x767984`).
    pub(super) rel_raw: String,
    pub(super) dbg: String,
}

impl Loader<'_> {
    pub(super) fn lua(&self) -> &mlua::Lua {
        self.lua
    }

    /// The current document's directory, which its relative references resolve against.
    fn base(&self) -> &str {
        dir_of(&self.path)
    }

    /// Run a file chunk (bytes, as on disk) in the one global state, named after its path. The
    /// name matters: unnamed, mlua's `#[track_caller]` `load` names it after this Rust line.
    fn run(&self, chunk: &[u8], path: &str) -> mlua::Result<()> {
        // The reference's file chunk name, `"@%s"` (`0x8716e0`, `0x704bc0`; a file name to
        // `luaO_chunkid 0x6f5c40`), with `\` separators as in `crate::script::addon_chunk_name`,
        // over the install path: an addon's files sit under `Interface\AddOns\`.
        self.run_named(chunk, &format!("@{}", self.status_path(path)))
    }

    /// Run a chunk under a complete chunk name, for a producer whose name is not a path.
    fn run_named(&self, chunk: &[u8], name: &str) -> mlua::Result<()> {
        self.lua
            .load(chunk)
            .set_name(name)
            .set_mode(mlua::ChunkMode::Text)
            .exec()
    }

    fn model(&self) -> mlua::AppDataRefMut<'_, crate::script::Model> {
        self.lua
            .app_data_mut::<crate::script::Model>()
            .expect("model app_data")
    }

    /// Run a widget handler with the client's calling convention, from `&Lua`.
    pub(super) fn invoke_handler(
        &self,
        wrapper: &mlua::Table,
        func: &mlua::Function,
    ) -> mlua::Result<()> {
        crate::script::event::invoke_with_globals(self.lua, wrapper.clone(), func, Vec::new())
    }

    /// An object's name as the reference's load lines print it: its own name (vtable `+4`), else
    /// `<unnamed>` (`0x84c7f0`).
    pub(super) fn object_name(&self, wrapper: &Table) -> String {
        crate::script::binding_abi::name_of(self.lua, wrapper)
    }

    /// A severity-1 line into the document's record, the status object's `+0xc` slot every
    /// `LoadXML` logs through, retained as a warning too.
    pub(super) fn status_warning(&mut self, line: String) {
        self.report.status.report(status::WARNING, line.clone());
        self.report.warnings.push(line);
    }

    pub(super) fn warn_once(&mut self, key: &str, msg: impl Into<String>) {
        if self.warned.insert(key.to_string()) {
            self.report.warnings.push(msg.into());
        }
    }

    /// Resolve `text=` as a global-string key (`FrameScript_GetText 0x703bf0`), so
    /// `GlobalStrings.lua` must run before any XML. A miss comes back empty (`0x882748`), and all
    /// three `text=` readers show the raw value for an empty answer (`0x771032`, `0x778c31`,
    /// `0x7292a6`), so a global holding `""` shows its key. The warning on a key-shaped miss is
    /// benilla's own.
    pub(super) fn resolve_text(&mut self, raw: &str, dbg: &str) -> String {
        if let Ok(s) = self.lua().globals().get::<String>(raw) {
            return if s.is_empty() { raw.to_string() } else { s };
        }
        if is_global_string_key(raw) {
            self.warn_once(
                &format!("gs:{raw}"),
                format!(
                    "{dbg}: text=\"{raw}\" is shaped like a GlobalStrings key but no such string \
                     global exists — showing the key"
                ),
            );
        }
        raw.to_string()
    }

    /// Walk one document's top-level items in order (`0x6ede10`).
    pub(super) fn load_doc(&mut self, doc: &ParsedDocument) {
        for item in &doc.items {
            match item {
                TopLevel::Include(path) => self.do_include(path),
                // Deviation: a `<Script file=>` value with a separator joins the including
                // document's directory, like a bare name. The reference uses it verbatim
                // (`0x6ee070`-`0x6ee079`) against the install directory, the root `0x646ea0`
                // scans from Storm's base path `0xc52418` (the basename retry at `0x647ed3` is
                // off), where a stock install has no such file, so joining breaks nothing.
                TopLevel::Script(ScriptRef::File(path)) => {
                    let joined = join_ref(self.base(), path);
                    match (self.files)(&joined) {
                        Some(bytes) => {
                            if let Err(e) = self.run(crate::source::chunk(&bytes), &joined) {
                                self.report
                                    .raise(format!("<Script file=\"{path}\">: {e}"), &e);
                            }
                        }
                        None => {
                            // The chunk arm (`0x704bc0`), whatever the extension.
                            let line = status::missing(&self.status_path(&joined), true);
                            self.report.status.report(status::FAILURE, line);
                            self.report.missing_files.push(format!(
                                "<Script file=\"{path}\">: no provider hit for \"{joined}\"; \
                                 every handler in it is missing"
                            ))
                        }
                    }
                }
                TopLevel::Script(ScriptRef::Inline { body, line }) => {
                    // Deviation: padded to the block's line, so a raise reports a line a reader can
                    // find in the file; the reference counts from the body's first line.
                    let mut chunk = "\n".repeat(line.saturating_sub(1) as usize).into_bytes();
                    chunk.extend_from_slice(body.as_bytes());
                    // The reference names an inline body `"%s:<Scripts>"` (`0x871074`, formatted at
                    // `0x6ee0ff`) over the document's path with no `@`, so a raise reads
                    // `[string "Interface\FrameXML\Foo.xml:<Scripts>"]`, not a file path.
                    let name = format!("{}:<Scripts>", self.status_path(&self.path));
                    if let Err(e) = self.run_named(&chunk, &name) {
                        self.report.raise(format!("inline <Script>: {e}"), &e);
                    }
                }
                TopLevel::Font(el) => self.do_font(el),
                TopLevel::Template(el) => self.register_template(el),
                TopLevel::Instance(el) => {
                    self.materialize(el, None, framexml::DEFAULT_PARENT_NAME);
                }
            }
        }
    }

    /// A top-level `virtual` element, registered raw (`0x6ee500`, called at `0x6ee1bd` under the
    /// live context word): `expand` resolves its own `inherits` at instantiation.
    fn register_template(&mut self, el: &Element) {
        // No name, or an empty one (`0x6ee1a9` tests its first byte): `0x871020` at severity 1
        // into the document's record (`0x6ee1c4`-`0x6ee1cf`), and nothing registers.
        let Some(name) = el.name().filter(|n| !n.is_empty()) else {
            let line = "Unnamed virtual node at top level";
            self.report.status.report(status::WARNING, line);
            self.report.warnings.push(line.to_string());
            return;
        };
        let model = self.model();
        let context = model.exec_context.borrow().clone();
        let registered =
            model
                .framexml_templates
                .borrow_mut()
                .register(name, el.clone(), context.as_deref());
        drop(model);
        match registered {
            framexml::Registration::Refused => {
                // `0x871188` over the refused spelling, at severity 1 into the document's record.
                let line = format!("Virtual object named {name} already exists");
                self.report.status.report(status::WARNING, line.clone());
                self.report.warnings.push(line);
            }
            // Either way the entry now holds this node, traced at `0x6ee6dd` (`0x87116c`).
            framexml::Registration::Added | framexml::Registration::Replaced => {
                if self.debug() {
                    let trace = format!("-- Added virtual frame {name}");
                    self.report.status.report(status::TRACE, trace.clone());
                    self.report.traces.push(trace);
                }
            }
        }
    }

    pub(super) fn do_include(&mut self, path: &str) {
        let joined = join_ref(self.base(), path);
        let Some(bytes) = (self.files)(&joined) else {
            // Reported into this document's record, before the included one has its own.
            let line = status::missing(&self.status_path(&joined), status::runs_as_lua(&joined));
            self.report.status.report(status::FAILURE, line);
            self.report.missing_files.push(format!(
                "<Include file=\"{path}\">: no provider hit for \"{joined}\"; the whole \
                 document it names is missing"
            ));
            return;
        };
        // `<Include>` recurses (`0x6ee00d`) into the reference's load-one-file routine `0x6ede10`,
        // which dispatches on the resolved path's extension, never its content: a `.lua` suffix
        // (`0x6edee6`-`0x6edf0f`) runs the file as a chunk (`0x704bc0`), anything else parses as
        // XML. A `.toc` line goes through the same routine.
        if status::runs_as_lua(&joined) {
            if let Err(e) = self.run(crate::source::chunk(&bytes), &joined) {
                self.report
                    .raise(format!("<Include file=\"{path}\">: {e}"), &e);
            }
            return;
        }
        // Only an included document is decoded to text: roxmltree parses `&str`.
        match framexml::parse(&crate::source::decode(&bytes)) {
            Ok(sub) => {
                self.report.warnings.extend(sub.warnings.iter().cloned());
                // The included document resolves and names against its own path until it is done,
                // and reports into a record of its own, closed under its banner (`0x6ee21c`).
                let outer = std::mem::replace(&mut self.path, joined.clone());
                let outer_status = std::mem::take(&mut self.report.status);
                self.load_doc(&sub);
                self.path = outer;
                let own = std::mem::replace(&mut self.report.status, outer_status);
                let banner = status::file_banner(&self.status_path(&joined));
                let debug = self.debug();
                own.close_into(&mut self.report.status, debug, banner);
            }
            Err(e) => {
                let line = status::unparsed(&self.status_path(&joined));
                self.report.status.report(status::FAILURE, line);
                self.report.fault(format!("<Include file=\"{path}\">: {e}"))
            }
        }
    }

    /// Resolve `inherits=` against the persistent template registry (`framexml::expand`).
    pub(super) fn expand(&mut self, el: &Element) -> Element {
        let model = self.model();
        let templates = model.framexml_templates.borrow();
        // A font object in an inherit chain is skipped, not warned as an unknown template.
        let is_font = |name: &str| model.fonts.id(name).is_some();
        let mut warns = Vec::new();
        let out = framexml::expand_known(el, &*templates, &is_font, &mut warns);
        drop(templates);
        drop(model);
        self.report.warnings.extend(warns);
        out
    }

    /// The nodes `raw`'s `LoadXML` reads ([`Levels`]).
    pub(super) fn levels<'a>(&self, raw: &'a Element) -> Levels<'a> {
        let inherited = self
            .model()
            .framexml_templates
            .borrow()
            .inherited_levels(raw);
        Levels {
            inherited,
            own: raw,
        }
    }

    /// The nodes a FontString's `LoadXML` reads: [`Self::levels`], up to the first `inherits` that
    /// names a font, which the string links instead of loading a template (`0x770f69`).
    pub(super) fn fontstring_levels<'a>(&self, raw: &'a Element) -> Levels<'a> {
        let model = self.model();
        let inherited = model
            .framexml_templates
            .borrow()
            .inherited_levels_until(raw, |name| model.fonts.id(name).is_some());
        Levels {
            inherited,
            own: raw,
        }
    }

    /// A top-level `<Font name=…>`: the named object, made if new and kept with its followers if
    /// re-declared (`0x783870`, `0x7839ab`), loads the node ([`Self::load_font_node`]) and is
    /// published whether or not its own font was refused.
    pub(super) fn do_font(&mut self, el: &Element) {
        let Some(name) = el.name().map(str::to_string) else {
            return;
        };
        let id = self.model().fonts.named(&name);
        self.load_font_node(el, id, &name);
        // In 1.12 a named `<Font>` is also a Lua global, the object `SetFontObject` takes.
        if let Err(e) = crate::script::font::publish(self.lua(), id, &name) {
            self.report
                .warnings
                .push(format!("<Font name=\"{name}\">: {e}"));
        }
    }

    /// `CSimpleFont::LoadXML 0x783c30` on font `id`, `name` naming it in the warnings: `inherits=`
    /// links it to that font (`0x783ce6`; a miss is logged, `0x87a51c`), a `font=` naming a font
    /// links it there instead (`0x783d22`), and any other `font=` is a file that needs the node's
    /// own first `<FontHeight>` above 0 and sets the flags word whole from `outline=` and
    /// `monochrome=` (`0x783db1`-`0x783ea3`) when it loads; without the height the load stops
    /// (`0x783dae`). Then `spacing=` with `SStrToFloat` (`0x783ef6`), `justifyV`, `justifyH` and each
    /// `<Color>` and `<Shadow>` child, the last of each winning. Each attribute is a setter's write,
    /// clearing its inherit bit (`0x783ea9`-`0x784100`) and pushing to what already follows the
    /// font.
    pub(super) fn load_font_node(
        &mut self,
        el: &Element,
        id: crate::script::font::FontId,
        name: &str,
    ) {
        use crate::script::{font, FontMask};
        if let Some(parent) = el.attr("inherits").filter(|n| !n.is_empty()) {
            let to = self.model().fonts.id(parent);
            match to {
                Some(to) => self.link_font_node(id, to, name),
                None => self.status_warning(format!("Couldn't find inherited font: {parent}")),
            }
        }
        match el.attr("font").filter(|f| !f.is_empty()) {
            Some(f) if self.model().fonts.id(f).is_some() => {
                let to = self.model().fonts.id(f).expect("tested");
                self.link_font_node(id, to, name);
            }
            Some(path) => {
                // The first `<FontHeight>`, read whatever it answers (`0x783d65`), above 0.
                let height = children_named(el, "FontHeight")
                    .next()
                    .map(|h| self.value(h))
                    .filter(|h| h.raw() > 0.0);
                let Some(height) = height else {
                    self.report_missing_font_height("Font", name, &el.tag);
                    return;
                };
                let outline = el.attr("outline").map(Outline::parse).unwrap_or_default();
                // Any non-empty `monochrome=` sets the flag (`0x783e01`-`0x783e14`), even "false".
                let monochrome = el.attr("monochrome").is_some_and(|v| !v.is_empty());
                // The font the node already holds (the path ignoring case, the height within 2⁻²²,
                // the same flags) is kept as spelled and still clears the bit (`0x783e17`-`0x783e5d`
                // → `0x783ea9`); any other loads first (`0x783e7c`). A file that fails is logged
                // (`0x87a4c8`) and the node goes on to `spacing`, keeping what it follows.
                let mut model = self.model();
                let u = model.layout_units();
                let fo = &model.fonts.get(id).block;
                let held = fo
                    .font
                    .as_deref()
                    .is_some_and(|p| p.eq_ignore_ascii_case(path))
                    && fo.height.is_some_and(|h| {
                        (h.script(u) - height.script(u)).abs() < crate::script::SAME_FONT_HEIGHT
                    })
                    && (fo.outline, fo.monochrome) == (outline, monochrome);
                if held || crate::script::face_loads(&model, path) {
                    font::set(&mut model, id, FontMask::FONT, |fo| {
                        if !held {
                            fo.font = Some(path.to_string());
                            fo.height = Some(height);
                            fo.outline = outline;
                            fo.monochrome = monochrome;
                        }
                    });
                } else {
                    drop(model);
                    self.status_warning(format!("Font {name}: Unable to load font file {path}"));
                }
            }
            None => {}
        }
        let mut model = self.model();
        if let Some(sp) = el.attr("spacing").filter(|v| !v.is_empty()) {
            let sp = xml_float(sp);
            font::set(&mut model, id, FontMask::SPACING, |fo| {
                fo.spacing = Some(sp)
            });
        }
        // Each axis matches its token as `0x6f1990` does and writes its own bits of the one justify
        // word, none for the other axis's token (`0x783f75`-`0x783f7c`, `0x783fbd`-`0x783fc4`); a
        // token that matches nothing is skipped and clears no bit (`0x783f6d`, `0x783fb5`).
        for (attr, mask) in [("justifyV", justify::V_MASK), ("justifyH", justify::H_MASK)] {
            let Some(bits) = el.attr(attr).and_then(justify::parse_bits) else {
                continue;
            };
            font::set(&mut model, id, FontMask::JUSTIFY, |fo| {
                let word = fo.justify.get_or_insert_with(Default::default);
                word.0 = justify::set_axis(word.0, mask, bits);
            });
        }
        drop(model);
        for child in &el.children {
            if child.tag.eq_ignore_ascii_case("Color") {
                let c = color_of(child);
                font::set(&mut self.model(), id, FontMask::COLOR, |fo| {
                    fo.color = Some(c)
                });
            } else if child.tag.eq_ignore_ascii_case("Shadow") {
                let sh = self.shadow_of(child);
                font::set(&mut self.model(), id, FontMask::SHADOW, |fo| {
                    fo.shadow = Some(sh)
                });
            }
        }
    }

    /// Link font `id` to `to` for an XML attribute. Deviation: a link that would close a loop is
    /// refused and reported; the reference's XML legs (`0x783ce6`, `0x783d22`) check nothing, and
    /// the next push down such a loop would never end.
    fn link_font_node(
        &mut self,
        id: crate::script::font::FontId,
        to: crate::script::font::FontId,
        name: &str,
    ) {
        use crate::script::font;
        let mut model = self.model();
        if font::would_loop(&model, id, to) {
            drop(model);
            self.status_warning(format!("Font {name}: can't create a font object loop"));
            return;
        }
        font::link_font(&mut model, id, Some(to));
    }

    /// Build one frame element, template-expanded, its nested `<Frames>`, then its `OnLoad`
    /// (bottom-up, `0x76a060`). `parent` is the lexically enclosing frame, which `parent=`
    /// overrides; `None` when the frame type is unknown, which skips the subtree.
    pub(super) fn materialize(
        &mut self,
        raw: &Element,
        parent: Option<&Table>,
        parent_name: &str,
    ) -> Option<Table> {
        let expanded = self.expand(raw);
        let el = &expanded;
        // Only creation lives here: everything after `CreateFrame` is `decorate`, which
        // `apply_template` enters for a frame that already exists.

        // 0 · The type lookup. `Instantiate 0x6ee280` checks the tag before reading `parent=`; a
        //     miss logs `"Unknown frame type: %s"` (`0x871124`, at `0x6ee356`), builds nothing for
        //     the node and goes on to the next sibling. Only the Lua door raises.
        if crate::script::object::registered_frame_kind(self.lua(), &el.tag).is_none() {
            self.report
                .warnings
                .push(format!("Unknown frame type: {}", el.tag));
            return None;
        }

        // 1a · `parent=` wins over the lexical parent, unexpanded: `0x6ee280` hands the raw value
        //      to the by-name lookup `0x76c760` (at `0x6ee3e8`). A miss is logged and stores 0 over
        //      the default parent (`0x6ee3ef`), so the frame is built parentless (`0x6ee408`); an
        //      empty `parent=""` keeps the default parent silently (`0x6ee3c7`).
        //      `None`: no attribute, or an empty one. `Some(None)`: a miss, parentless and logged.
        let attr_parent: Option<Option<Table>> =
            el.attr("parent")
                .filter(|name| !name.is_empty())
                .map(|name| {
                    // Only a frame counts: `0x76c760` reads the frame registry, so a non-frame
                    // global of that name is a miss. A frame wrapper is the one table carrying
                    // lightuserdata at `[0]` (`0x6f3ea0`).
                    let hit =
                        self.lua().globals().get::<Table>(name).ok().filter(|t| {
                            matches!(t.raw_get::<Value>(0), Ok(Value::LightUserData(_)))
                        });
                    if hit.is_none() {
                        // The reference's wording (`0x8710f0`).
                        self.report
                            .warnings
                            .push(format!("Couldn't find frame parent: {name}"));
                    }
                    hit
                });
        let parent = match &attr_parent {
            Some(resolved) => resolved.as_ref(),
            None => parent,
        };

        // 1b · The name resolves after the parent, against it. `Instantiate 0x6ee280` passes the
        //      parent to the constructor (`0x6ee408`) before it applies `name=` (`0x6ee4d6`), and
        //      `SetName 0x76c650` expands `$parent` by walking the real parent chain to the nearest
        //      name, seeded `"Top"` (`0x76c5b0`). The base is the parent frame's own name, not the
        //      attribute text; a nulled parent leaves the `"Top"` seed.
        let attr_parent_name: Option<String> =
            attr_parent.as_ref().map(|resolved| match resolved {
                Some(p) => {
                    let (own, ancestor) = self.frame_names(p);
                    own.filter(|n| !n.is_empty()).unwrap_or(ancestor)
                }
                None => framexml::DEFAULT_PARENT_NAME.to_string(),
            });
        let parent_name = attr_parent_name.as_deref().unwrap_or(parent_name);
        let resolved_name: Option<String> = el
            .name()
            .map(|raw| framexml::resolve_name(raw, parent_name));

        // 1 · `CreateFrame(tag, name, parent)`; the type is known, so an error is a real raise.
        let create: Function = match self.lua().globals().get("CreateFrame") {
            Ok(f) => f,
            Err(e) => {
                self.report
                    .fault(format!("CreateFrame global missing: {e}"));
                return None;
            }
        };
        self.set_xml_creating(true);
        let made = create.call::<Table>((el.tag.clone(), resolved_name.clone(), parent.cloned()));
        self.set_xml_creating(false);
        let wrapper: Table = match made {
            Ok(w) => w,
            Err(e) => {
                self.report.fault(format!(
                    "CreateFrame(<{}>{}): {e}",
                    el.tag,
                    resolved_name
                        .as_deref()
                        .map(|n| format!(" name=\"{n}\""))
                        .unwrap_or_default()
                ));
                return None;
            }
        };
        self.report.frames += 1;

        let dbg_name = resolved_name
            .clone()
            .unwrap_or_else(|| format!("<{}>", el.tag));

        // One of the reference's five loader traces: `Instantiate 0x6ee280`'s `"-- Creating %s
        // named %s"` (`0x871154`), gated `flag > 0` at `0x6ee298`; the first `%s` is the tag.
        if self.debug() {
            let trace = format!("-- Creating {} named {dbg_name}", el.tag);
            self.report.status.report(status::TRACE, trace.clone());
            self.report.traces.push(trace);
        }

        // `$parent` for its contents; a nameless frame passes its nearest named ancestor on.
        let self_name = resolved_name.as_deref().unwrap_or(parent_name).to_string();

        let levels = self.levels(raw);
        self.decorate(el, &levels, &wrapper, &self_name, parent_name, &dbg_name);
        Some(wrapper)
    }

    /// Everything a frame element does to an existing frame, from `LoadXML` attributes to its
    /// `OnLoad` (bottom-up, `0x76a060`) and the post-load OnShow; shared by [`Self::materialize`] and
    /// [`apply_template`]. `el` is the template-expanded element and `levels` the nodes it merges
    /// ([`Loader::levels`]). `$parent` is `self_name` in this frame's contents and `parent_name` in
    /// its own anchors (`0x76c5b0`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn decorate(
        &mut self,
        el: &Element,
        levels: &Levels,
        wrapper: &Table,
        self_name: &str,
        parent_name: &str,
        dbg_name: &str,
    ) {
        // This frame's deferred anchors drain before its OnLoad; a nested frame drains its own.
        let deferred_mark = self.deferred_anchors.len();
        // 1c · the pre-load hook opens the frame's load window (`0x7697c2`): until the post-load
        //      hook, its own OnShow and OnHide fire nothing.
        crate::script::visibility::set_load_window(self.lua(), wrapper, true);
        // 2 · LoadXML attributes (`0x769820`).
        self.apply_attrs(el, levels, wrapper, dbg_name);
        // 3 · <Size> and 4 · <Anchors>, each level's own (the CLayoutFrame geometry base,
        //     `0x767800`).
        for level in levels {
            self.apply_geometry(level, wrapper, false, parent_name, dbg_name);
        }
        // 5 · <Layers> regions (`0x769d70`) and the <Backdrop> plate (`0x77e6c0`), whose nine
        //     pieces are regions too, in document order: both are arms of the one child loop
        //     (`0x769b1c`), so the order they link into `GetRegions` is the file's. A template's
        //     children come first, as its LoadXML runs first. `$parent` in a region name is this
        //     frame.
        for child in &el.children {
            if child.tag.eq_ignore_ascii_case("Layers") {
                self.apply_layers(child, wrapper, self_name, dbg_name);
            } else if child.tag.eq_ignore_ascii_case("Backdrop") {
                self.apply_backdrop(child, wrapper, dbg_name);
            }
        }
        // 5·b · a text frame's `font=`, then its `<FontString>` children, which load into the same
        //       font (`0x779fcb` before `0x77a220`, `0x78592b` before `0x785af5`, `0x787b3b`
        //       before `0x787cfe`). Any other frame's direct `<FontString>` builds nothing: the base
        //       loop (`0x769b27`-`0x769d22`) has no arm for it, and only SimpleHTML's reads one.
        if ["EditBox", "MessageFrame", "ScrollingMessageFrame"]
            .iter()
            .any(|t| el.tag.eq_ignore_ascii_case(t))
        {
            self.apply_frame_font(el, wrapper, dbg_name);
            self.apply_text_font_children(el, wrapper, dbg_name);
        }
        // 5a' · <TitleRegion>: the drag handle, through the API's own CreateTitleRegion.
        self.apply_title_region(el, wrapper, self_name, dbg_name);
        // 5b · per-kind LoadXML extras (EditBox `0x779fb0`), each gated on the element's own tag,
        //      so a Frame wearing a <Button> template skips the Button steps.
        self.apply_statusbar(el, levels, wrapper, self_name, dbg_name);
        self.apply_slider(el, levels, wrapper, self_name, dbg_name);
        self.apply_colorselect(el, wrapper, self_name, dbg_name);
        self.apply_button(el, levels, wrapper, self_name, dbg_name);
        self.apply_editbox(el, wrapper, dbg_name);
        self.apply_messageframe(el, wrapper, dbg_name);
        self.apply_simplehtml(el, wrapper, dbg_name);
        // 6 · <Scripts> handlers (`0x769ef0`); OnLoad is captured to fire bottom-up below.
        let onload = self.apply_scripts(el, wrapper, dbg_name);
        // 6b · a ScrollFrame's <ScrollChild> (`0x786bc0`, after the base load `0x769820`): still
        //      inside the load window, so ahead of every <Frames> child and of our own OnLoad.
        self.apply_scrollframe(el, wrapper, self_name, dbg_name);

        // 7 · the post-load hook (`0x76a2f0`): close the window (`0x76a2ff`), then the nested
        //     <Frames>, whose OnLoads fire before ours (`0x76a060`).
        crate::script::visibility::set_load_window(self.lua(), wrapper, false);
        crate::script::tooltip_post_load(self.lua(), wrapper);
        for frames_el in children_named(el, "Frames") {
            for child in &frames_el.children {
                self.materialize(child, Some(wrapper), self_name);
            }
        }

        // 7c · deferred anchors: every child exists now, as at the reference's layout pass.
        self.drain_deferred_anchors(deferred_mark);
        // 8 · this frame's OnLoad, now that its subtree is complete (bottom-up).
        if let Some(func) = onload {
            self.fire_onload(wrapper, &func, dbg_name);
        }
        // 9 · and its OnShow, if it is visible now (`0x76a3b3`).
        crate::script::visibility::post_load_show(self.lua(), wrapper);
        // 10 · a Minimap's models, which its post-load override sets after chaining to this one
        //      (`0x4ee2b0` calls `0x76a2f0` first).
        self.apply_minimap(el, wrapper, dbg_name);
    }

    /// Apply one anchor, deferring it (when `may_defer`) while its named `relativeTo` is unbuilt.
    /// A target still missing (`0x878440`) or the object itself (`0x878420`, `0x76797d`) is logged
    /// and the anchor skipped.
    pub(super) fn apply_anchor(&mut self, d: DeferredAnchor, may_defer: bool) {
        let rel: Value = match d.args.1.as_deref() {
            Some(name) => {
                match crate::script::object::anchor_args::resolve_xml_relative_to(self.lua(), name)
                {
                    Some(t) => {
                        let id = crate::script::object::decode_id;
                        if id(&t).ok().is_some_and(|i| id(&d.wrapper).ok() == Some(i)) {
                            let raw = &d.rel_raw;
                            self.status_warning(format!("Frame anchored to itself: {raw}"));
                            return;
                        }
                        Value::Table(t)
                    }
                    // Not built yet: retry once the subtree is complete; a miss then is real.
                    None if may_defer => {
                        self.deferred_anchors.push(d);
                        return;
                    }
                    None => {
                        let raw = &d.rel_raw;
                        self.status_warning(format!("Couldn't find relative frame: {raw}"));
                        return;
                    }
                }
            }
            // No `relativeTo=`: the layout parent (`0x76c6e0`, called at `0x76785b`), or the
            // screen for a parentless frame, which `nil` means here.
            None => match d
                .wrapper
                .call_method::<Option<Table>>("GetParent", ())
                .ok()
                .flatten()
            {
                Some(t) => Value::Table(t),
                None => Value::Nil,
            },
        };
        let DeferredAnchor {
            wrapper,
            region,
            args: (point, _, rel_point),
            offset,
            dbg,
            ..
        } = d;
        // The first `<Offset>`, read whatever it answers, after the target's miss and self tests
        // (`0x767946`-`0x7679ad`), so a skipped anchor reads none.
        let v = offset.map_or([Len::Script(0.0); 2], |o| self.dimension(&o).v);
        let [x, y] = v.map(|l| l.script(self.model().layout_units()));
        let args = (point.clone(), rel, rel_point, x, y);
        if region {
            self.call_region(&wrapper, "SetPoint", args, &dbg);
        } else {
            self.call(&wrapper, "SetPoint", args, &dbg);
        }
        crate::script::xml_layout::offset(self.lua, &wrapper, region, &point, v);
    }

    /// Apply the anchors deferred since `mark`, in order; a target still missing now warns.
    pub(super) fn drain_deferred_anchors(&mut self, mark: usize) {
        let pending: Vec<DeferredAnchor> = self.deferred_anchors.drain(mark..).collect();
        for d in pending {
            self.apply_anchor(d, false);
        }
    }

    fn frame_names(&self, wrapper: &Table) -> (Option<String>, String) {
        let own = wrapper
            .call_method::<Option<String>>("GetName", ())
            .ok()
            .flatten();
        let mut cur = wrapper.clone();
        // The frame's own name and its nearest named ancestor's, else `"Top"` (`0x76c5b0`'s
        // walk), read through `GetName`/`GetParent`. Bounded, so a malformed chain cannot hang.
        for _ in 0..64 {
            let Ok(Some(parent)) = cur.call_method::<Option<Table>>("GetParent", ()) else {
                break;
            };
            if let Ok(Some(name)) = parent.call_method::<Option<String>>("GetName", ()) {
                return (own, name);
            }
            cur = parent;
        }
        (own, framexml::DEFAULT_PARENT_NAME.to_string())
    }
}

/// What one of the reference's XML length readers answers (`XmlReadDimension 0x6f1eb0`,
/// `XmlReadValue 0x6f1d60`, `XmlReadInset 0x6f20a0`): the values, zeroed and then written where
/// read, and whether the read succeeded. A failure keeps what was written before it, which a
/// caller that ignores the answer reads (`0x7679ad`, `0x77142d`, `0x771146`).
pub(super) struct XmlLength<const N: usize> {
    pub(super) v: [Len; N],
    pub(super) ok: bool,
}

impl<const N: usize> XmlLength<N> {
    /// The values as script numbers, for a cell benilla keeps as one: a layout length through `G`.
    pub(super) fn script(&self, units: crate::layout::LayoutUnits) -> [f32; N] {
        self.v.map(|l| l.script(units))
    }
}

impl Loader<'_> {
    /// The readers' one node rule: the node's own non-empty `keys` through `SStrToFloat`, then its
    /// first child only. With none, the read succeeds when `gate` is present, else it logs
    /// `missing` with the node's tag and fails (`0x6f1f44`-`0x6f1f55`). The node's own and an
    /// `abs` child's non-empty `keys` are script numbers the reader stores by `S` (`0x6f1ee3`-
    /// `0x6f1f02`, `0x6f1f9f`-`0x6f1fc1`), the child's overwriting; a `rel` child's are layout
    /// units, stored as parsed (`0x6f2037`-`0x6f2060`); any other child logs `0x871464` and fails
    /// (`0x6f2070`).
    fn xml_length<const N: usize>(
        &mut self,
        el: &Element,
        keys: [&str; N],
        gate: &str,
        child: (&str, &str),
        missing: &str,
    ) -> XmlLength<N> {
        let mut v = [Len::Script(0.0); N];
        let read = |src: &Element, v: &mut [Len; N], unit: fn(f32) -> Len| {
            for (slot, key) in v.iter_mut().zip(keys) {
                if let Some(x) = xml_float_attr(src, key) {
                    *slot = unit(x);
                }
            }
        };
        read(el, &mut v, Len::Script);
        let ok = match el.children.first() {
            None if el.attr(gate).is_some() => true,
            None => {
                self.status_warning(format!("{missing}{}", el.tag));
                false
            }
            Some(c) if c.tag.eq_ignore_ascii_case(child.0) => {
                read(c, &mut v, Len::Script);
                true
            }
            Some(c) if c.tag.eq_ignore_ascii_case(child.1) => {
                read(c, &mut v, Len::Layout);
                true
            }
            Some(c) => {
                let line = format!("Unknown child node in {} element: {}", el.tag, c.tag);
                self.status_warning(line);
                false
            }
        };
        XmlLength { v, ok }
    }

    /// `XmlReadDimension` (`0x6f1eb0`): `x` and `y` of a `<Size>`, an `<Offset>`, a
    /// `<PushedTextOffset>` or a resize bound, gated on `y` (`0x8714e8`).
    pub(super) fn dimension(&mut self, el: &Element) -> XmlLength<2> {
        let missing = r#"No "x" or "y" attributes in element: "#;
        self.xml_length(
            el,
            ["x", "y"],
            "y",
            ("AbsDimension", "RelDimension"),
            missing,
        )
    }

    /// `XmlReadValue` (`0x6f1d60`): the `val` of a `<FontHeight>`, `<EdgeSize>` or `<TileSize>`
    /// (`0x8714a4`).
    /// Whatever the read answers, its unit kept, for a caller that ignores the answer.
    pub(super) fn value(&mut self, el: &Element) -> Len {
        let missing = r#"No "val" attribute in element: "#;
        self.xml_length(el, ["val"], "val", ("AbsValue", "RelValue"), missing)
            .v[0]
    }

    /// [`Self::value`] with its answer and its unit, for a caller that checks it (`0x77e779`,
    /// `0x77e7ae`).
    pub(super) fn value_read(&mut self, el: &Element) -> Option<Len> {
        let missing = r#"No "val" attribute in element: "#;
        let r = self.xml_length(el, ["val"], "val", ("AbsValue", "RelValue"), missing);
        r.ok.then_some(r.v[0])
    }

    /// `XmlReadInset` (`0x6f20a0`): `left`, `right`, `top` and `bottom` of an insets element,
    /// gated on `bottom` alone (`0x6f21bc`, `0x871530`).
    pub(super) fn inset(&mut self, el: &Element) -> Option<[Len; 4]> {
        let missing = r#"No "right", "left", "top", or "bottom" attributes in element: "#;
        let keys = ["left", "right", "top", "bottom"];
        let r = self.xml_length(el, keys, "bottom", ("AbsInset", "RelInset"), missing);
        r.ok.then_some(r.v)
    }

    /// Script numbers for a verb, a layout length through `G`.
    pub(super) fn script_of<const N: usize>(&self, v: [Len; N]) -> [f32; N] {
        let units = self.model().layout_units();
        v.map(|l| l.script(units))
    }

    /// A `<Shadow>` element (`0x7713ca`, `0x784058`): its first `<Color>` over opaque black
    /// (`0xff000000`), and its first `<Offset>` read whatever it answers (`0x77142d`, `0x7840a8`),
    /// script numbers the reader stores by `S` (`0x6f1ee3`-`0x6f1efd`), else (0.001, -0.001)
    /// layout units, stored raw (`0x3a83126f`, `0xba83126f`), which new cells read as the reload
    /// they stand for would ([`crate::layout::Len::Layout`]).
    pub(super) fn shadow_of(&mut self, sh: &Element) -> crate::script::FontShadow {
        let offset = match children_named(sh, "Offset").next() {
            Some(o) => self.dimension(o).v,
            None => crate::script::FontShadow::DEFAULT_OFFSET,
        };
        let color = children_named(sh, "Color")
            .next()
            .map_or(crate::color::Rgba8::BLACK, color_of);
        crate::script::FontShadow { offset, color }
    }
}

/// Iterate an element's direct children whose tag matches `tag` (case-insensitively).
pub(super) fn children_named<'a>(
    el: &'a Element,
    tag: &'a str,
) -> impl Iterator<Item = &'a Element> {
    el.children
        .iter()
        .filter(move |c| c.tag.eq_ignore_ascii_case(tag))
}

/// Iterate an element's direct children whose tag matches any of `tags` (case-insensitively), in
/// document order. One walk, not one per tag: a Button's label and state fonts each answer to two
/// tags (`CSimpleButton::LoadXML 0x7788c0`) and each slot is last-wins.
pub(super) fn children_named_any<'a>(
    el: &'a Element,
    tags: &'a [&'a str],
) -> impl Iterator<Item = &'a Element> {
    el.children
        .iter()
        .filter(move |c| tags.iter().any(|t| c.tag.eq_ignore_ascii_case(t)))
}

/// Whether a `text=` value is shaped like a GlobalStrings key: `SCREAMING_SNAKE` with a letter, two
/// characters or more (a one-character `text="X"` is a glyph).
fn is_global_string_key(s: &str) -> bool {
    s.len() >= 2
        && s.chars().any(|c| c.is_ascii_uppercase())
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// An XML number as every loader reads one, `SStrToFloat` (`0x64aaa0`): a prefix parse with no
/// whitespace skip and no leading `+`, narrowed to the `f32` the reference stores.
pub(super) fn xml_float(s: &str) -> f32 {
    crate::script::binding_abi::s_str_to_float(s) as f32
}

/// [`xml_float`] of a present, non-empty attribute, as the readers take one (`0x6f1ed9`-`0x6f1ee0`).
fn xml_float_attr(el: &Element, key: &str) -> Option<f32> {
    el.attr(key).filter(|v| !v.is_empty()).map(xml_float)
}

/// A `<Color r= g= b= a=/>` element as `0x6f23d0` packs it: each attribute that is present and
/// non-empty through `SStrToFloat` (`0x64aaa0`, so text with no number is 0), else 0 for r, g and b
/// and 1 for a (`0x6f23e2`-`0x6f23f7`), then clamped and rounded as every colour setter does.
pub(super) fn color_of(el: &Element) -> crate::color::Rgba8 {
    let c = |k: &str, d: f64| {
        el.attr(k)
            .filter(|v| !v.is_empty())
            .map_or(d, crate::script::binding_abi::s_str_to_float)
    };
    crate::color::Rgba8::pack(c("r", 0.0), c("g", 0.0), c("b", 0.0), c("a", 1.0))
}

/// A `<TexCoords>` element's edges `(l, r, t, b)`: each starts at 0, 1, 0, 1 and is read with
/// `SStrToFloat` only when present and non-empty (`0x76fef5`-`0x76ff70`).
pub(super) fn tex_coords(tc: &Element) -> (f32, f32, f32, f32) {
    let edge = |k: &str, default: f32| xml_float_attr(tc, k).unwrap_or(default);
    (
        edge("left", 0.0),
        edge("right", 1.0),
        edge("top", 0.0),
        edge("bottom", 1.0),
    )
}

#[cfg(test)]
mod level_tests;
#[cfg(test)]
mod template_name_tests;
#[cfg(test)]
mod template_registry_tests;
#[cfg(test)]
mod tests;
