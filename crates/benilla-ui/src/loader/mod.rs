//! The FrameXML loader: builds live frames from a parsed document ([`crate::framexml`]) by driving
//! the Lua object model ([`crate::script`]) as an addon does, never the arena directly.
//!
//! The caller's `files` provider supplies bytes, so the loader does no IO; a `<Script file=>`
//! chunk reaches Lua as the bytes on disk, as `luaL_loadbuffer` receives them. The loader joins a
//! relative path to its file's directory ([`join_ref`]); the provider decides what it may reach.
//!
//! Lua handles are never accumulated breadth-wise (MAXCSTACK): a frame's wrapper and `OnLoad`
//! live only across its own subtree build, and everything durable lives Lua-side.

use std::collections::{HashMap, HashSet};

use mlua::{Function, ObjectLike, Table, Value};

use crate::framexml::{self, Element, ParsedDocument, ScriptRef, TopLevel};
use crate::script::{FontObject, JustifyH, JustifyV, Outline, UiScript};

mod backdrop;
mod geometry;
mod regions;
mod scripts;
mod widgets;

/// The outcome of a [`load`]; nothing aborts a load, as the reference logs and continues.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// Issues that dropped nothing, and the document layer's parse warnings. An unknown frame type
    /// drops its node yet lands here: the reference logs it and goes on (`0x6ee356`).
    pub warnings: Vec<String>,
    /// Failures the reference raises on: a handler that fails to compile, a method call that
    /// errors, a malformed included document.
    pub errors: Vec<String>,
    /// `<Include file=>` and `<Script file=>` paths the provider lacks. The reference logs
    /// `"Couldn't open %s"` (`0x6edaa0`, `0x846ff4`) or `"Error loading %s"` (`0x872e50`) and goes
    /// on (`0x6ee00d`); kept apart from `warnings` so a load that resolved nothing is not clean.
    pub missing_files: Vec<String>,
    /// Frame instances `CreateFrame` built.
    pub frames: usize,
    /// Trace lines, only while `FrameXML_Debug` is on: the reference's flag (`[0xceea30]`) boots 0
    /// and its loader traces gate on `flag > 0` (`0x6ee298`). A trace is severity 0, not a warning.
    pub traces: Vec<String>,
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
    let mut loader = Loader {
        lua,
        files,
        path: path.to_string(),
        report: LoadReport::default(),
        warned: HashSet::new(),
        deferred_anchors: Vec::new(),
    };
    loader.report.warnings.extend(doc.warnings.iter().cloned());
    loader.load_doc(doc);
    loader.report
}

/// Apply a template to an existing frame, `CreateFrame`'s fourth argument. Returns the warnings,
/// then the errors. The frame always survives: an unknown name leaves it bare, as in the
/// reference, and only virtual elements are registered. The frame's own name is the `$parent`
/// base (`MyButton` makes `$parentText` `MyButtonText`). Runs inside the `CreateFrame` binding, so
/// it enters at [`Loader::decorate`]: [`Loader::materialize`] would recurse.
pub fn apply_template(lua: &mlua::Lua, wrapper: &Table, kind: &str, template: &str) -> Vec<String> {
    let template = template.trim();
    if template.is_empty() {
        return Vec::new();
    }
    // A template never reads a file: `<Include>` and `<Script file=>` are top-level only.
    let no_files = |_: &str| -> Option<Vec<u8>> { None };
    let mut loader = Loader {
        lua,
        files: &no_files,
        path: String::new(),
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
        // Looked up as `framexml::expand` looks it up (exact, then case-folded); keep them in step.
        for name in [template].into_iter().filter(|s| !s.is_empty()) {
            let Some(el) = templates.get(name).or_else(|| {
                templates
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v)
            }) else {
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
    let first = loader.report.warnings.len();
    let expanded = loader.expand(&framexml::inherits_node(kind, template));
    for w in &mut loader.report.warnings[first..] {
        *w = format!("{dbg}: {w}");
    }

    loader.decorate(&expanded, wrapper, &self_name, &parent_name, &dbg);

    let mut out = loader.report.warnings;
    out.extend(loader.report.errors);
    out
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

/// A `CSimpleModel` kind, whose `LoadXML` (`0x76cac0`) reads `scale=` as the model's scale.
pub(super) fn model_kind_tag(tag: &str) -> bool {
    ["Model", "PlayerModel", "DressUpModel", "TabardModel"]
        .iter()
        .any(|k| k.eq_ignore_ascii_case(tag))
}

/// The `.lua` test `0x6ede10` opens with: the last `.` anywhere in the resolved path, not the
/// basename's (`strrchr`), compared case-insensitively to `".lua"` (`0x8710c8`, via `0x64a4c0`).
fn has_lua_suffix(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("lua"))
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
    // The template and font registries live on the `Model` and persist across loads: the
    // reference's template table is global (`0x6ee500`), so `MerchantFrame.xml` inherits a template
    // `CharacterFrameTemplates.xml` registered. Fonts are their own namespace, stored flattened.
    pub(super) report: LoadReport,
    /// Warn-once keys.
    pub(super) warned: HashSet<String>,
    /// Anchors whose named `relativeTo` did not exist yet, applied once the enclosing frame's
    /// subtree is built: the reference resolves anchors at layout, this loader at `SetPoint`, so a
    /// target built later waits here (`ClassTrainerFrameTemplates.xml:67` hangs the label off the
    /// highlight). Each `decorate` drains its own.
    pub(super) deferred_anchors: Vec<DeferredAnchor>,
}

/// One `SetPoint` held until its target exists.
pub(super) struct DeferredAnchor {
    pub(super) wrapper: Table,
    /// A region wrapper (`call_region`) rather than a frame's (`call`).
    pub(super) region: bool,
    pub(super) args: (String, Option<String>, String, f32, f32),
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
        // `luaO_chunkid 0x6f5c40`), with `\` separators as in `crate::script::addon_chunk_name`.
        self.run_named(chunk, &format!("@{}", path.replace('/', "\\")))
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
        crate::script::event::invoke_with_globals(self.lua, wrapper.clone(), func, None, Vec::new())
    }

    pub(super) fn warn_once(&mut self, key: &str, msg: impl Into<String>) {
        if self.warned.insert(key.to_string()) {
            self.report.warnings.push(msg.into());
        }
    }

    /// Resolve `text=` as a global-string key (`FrameScript_GetText 0x703bf0`), so
    /// `GlobalStrings.lua` must run before any XML. A miss comes back empty (`0x882748`) and all
    /// three `text=` readers then show the raw value (`0x778c07`, `0x771006`, `0x7292a6`). The
    /// warning on a key-shaped miss is benilla's own.
    pub(super) fn resolve_text(&mut self, raw: &str, dbg: &str) -> String {
        if let Ok(s) = self.lua().globals().get::<String>(raw) {
            return s;
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
                                    .errors
                                    .push(format!("<Script file=\"{path}\">: {e}"));
                            }
                        }
                        None => self.report.missing_files.push(format!(
                            "<Script file=\"{path}\">: no provider hit for \"{joined}\"; \
                             every handler in it is missing"
                        )),
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
                    let name = format!("{}:<Scripts>", self.path.replace('/', "\\"));
                    if let Err(e) = self.run_named(&chunk, &name) {
                        self.report.errors.push(format!("inline <Script>: {e}"));
                    }
                }
                TopLevel::Font(el) => self.do_font(el),
                TopLevel::Template(el) => {
                    // Registered raw; `expand` resolves its own `inherits` at instantiation. An
                    // unnamed one was warned at parse time.
                    if let Some(name) = el.name() {
                        self.model()
                            .framexml_templates
                            .borrow_mut()
                            .insert(name.to_string(), el.clone());
                    }
                }
                TopLevel::Instance(el) => {
                    let expanded = self.expand(el);
                    self.materialize(&expanded, None, framexml::DEFAULT_PARENT_NAME);
                }
            }
        }
    }

    pub(super) fn do_include(&mut self, path: &str) {
        let joined = join_ref(self.base(), path);
        let Some(bytes) = (self.files)(&joined) else {
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
        if has_lua_suffix(&joined) {
            if let Err(e) = self.run(crate::source::chunk(&bytes), &joined) {
                self.report
                    .errors
                    .push(format!("<Include file=\"{path}\">: {e}"));
            }
            return;
        }
        // Only an included document is decoded to text: roxmltree parses `&str`.
        match framexml::parse(&crate::source::decode(&bytes)) {
            Ok(sub) => {
                self.report.warnings.extend(sub.warnings.iter().cloned());
                // The included document resolves and names against its own path until it is done.
                let outer = std::mem::replace(&mut self.path, joined.clone());
                self.load_doc(&sub);
                self.path = outer;
            }
            Err(e) => self
                .report
                .errors
                .push(format!("<Include file=\"{path}\">: {e}")),
        }
    }

    /// Resolve `inherits=` against the persistent template registry (`framexml::expand`).
    pub(super) fn expand(&mut self, el: &Element) -> Element {
        let model = self.model();
        let templates = model.framexml_templates.borrow();
        let view: HashMap<&str, &Element> =
            templates.iter().map(|(k, v)| (k.as_str(), v)).collect();
        // A font object in an inherit chain is skipped, not warned as an unknown template.
        let fonts = model.framexml_fonts.borrow();
        let font_names: std::collections::HashSet<&str> =
            fonts.keys().map(|k| k.as_str()).collect();
        let mut warns = Vec::new();
        let out = framexml::expand_known(el, &view, &font_names, &mut warns);
        drop(font_names);
        drop(fonts);
        drop(view);
        drop(templates);
        drop(model);
        self.report.warnings.extend(warns);
        out
    }

    /// [`Loader::expand`] for a region, only when `inherits=` names a registered element template:
    /// a FontString's `inherits=` usually names a font object, which passes through unwarned.
    pub(super) fn expand_region(&mut self, el: &Element) -> Element {
        let hit = el.attr("inherits").is_some_and(|names| {
            let model = self.model();
            let templates = model.framexml_templates.borrow();
            names
                .split(',')
                .map(str::trim)
                .any(|n| templates.contains_key(n))
        });
        if hit {
            self.expand(el)
        } else {
            el.clone()
        }
    }

    /// A top-level `<Font name=…>`: flatten its `inherits=` chain (fonts inherit only fonts),
    /// register the [`FontObject`] and store the flattened element for later fonts to inherit.
    pub(super) fn do_font(&mut self, el: &Element) {
        let Some(name) = el.name().map(str::to_string) else {
            return;
        };
        let merged = {
            let model = self.model();
            let fonts = model.framexml_fonts.borrow();
            let view: HashMap<&str, &Element> =
                fonts.iter().map(|(k, v)| (k.as_str(), v)).collect();
            let mut warns = Vec::new();
            let merged = framexml::expand(el, &view, &mut warns);
            drop(view);
            drop(fonts);
            drop(model);
            self.report.warnings.extend(warns);
            merged
        };

        let font = font_object_from_element(&merged);
        {
            let mut model = self.model();
            model
                .font_objects_by_lower
                .insert(name.to_ascii_lowercase(), font);
            model
                .framexml_fonts
                .borrow_mut()
                .insert(name.clone(), merged);
        }
        // In 1.12 a named `<Font>` is also a Lua global, the object `SetFontObject` takes.
        if let Err(e) = crate::script::font::publish(self.lua(), &name) {
            self.report
                .warnings
                .push(format!("<Font name=\"{name}\">: {e}"));
        }
    }

    /// Build one template-expanded frame element, its nested `<Frames>`, then its `OnLoad`
    /// (bottom-up, `0x76a060`). `parent` is the lexically enclosing frame, which `parent=`
    /// overrides; `None` when the frame type is unknown, which skips the subtree.
    pub(super) fn materialize(
        &mut self,
        el: &Element,
        parent: Option<&Table>,
        parent_name: &str,
    ) -> Option<Table> {
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
                    .errors
                    .push(format!("CreateFrame global missing: {e}"));
                return None;
            }
        };
        let wrapper: Table =
            match create.call((el.tag.clone(), resolved_name.clone(), parent.cloned())) {
                Ok(w) => w,
                Err(e) => {
                    self.report.errors.push(format!(
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
        if self.model().framexml_debug.get() > 0 {
            self.report
                .traces
                .push(format!("-- Creating {} named {dbg_name}", el.tag));
        }

        // `$parent` for its contents; a nameless frame passes its nearest named ancestor on.
        let self_name = resolved_name.as_deref().unwrap_or(parent_name).to_string();

        self.decorate(el, &wrapper, &self_name, parent_name, &dbg_name);
        Some(wrapper)
    }

    /// Everything a frame element does to an existing frame, from `LoadXML` attributes to its
    /// `OnLoad` (last, bottom-up, `0x76a060`); shared by [`Self::materialize`] and
    /// [`apply_template`]. `$parent` is `self_name` in this frame's contents and `parent_name` in
    /// its own anchors (`0x76c5b0`).
    pub(super) fn decorate(
        &mut self,
        el: &Element,
        wrapper: &Table,
        self_name: &str,
        parent_name: &str,
        dbg_name: &str,
    ) {
        // This frame's deferred anchors drain before its OnLoad; a nested frame drains its own.
        let deferred_mark = self.deferred_anchors.len();
        // 2 · LoadXML attributes (`0x769820`).
        self.apply_attrs(el, wrapper, dbg_name);
        // 3 · <Size> and 4 · <Anchors> (the CLayoutFrame geometry base, `0x767800`).
        self.apply_size(el, wrapper, dbg_name);
        self.apply_anchors(el, wrapper, parent_name, dbg_name);
        // 5 · <Layers> regions (`0x769d70`); `$parent` in a region name is this frame.
        self.apply_layers(el, wrapper, self_name, dbg_name);
        // 5·b · a direct-child `<FontString>`: the widget's own text font (EditBox, message frame).
        self.apply_special_fontstrings(el, wrapper, self_name, dbg_name);
        // 5a · <Backdrop> plate (LoadXML `0x77e6c0`): the tiled bg + 8-piece border.
        self.apply_backdrop(el, wrapper, dbg_name);
        // 5a' · <TitleRegion>: the drag handle, through the API's own CreateTitleRegion.
        self.apply_title_region(el, wrapper, self_name, dbg_name);
        // 5b · per-kind LoadXML extras (EditBox `0x779fb0`), each gated on the element's own tag,
        //      so a Frame wearing a <Button> template skips the Button steps.
        self.apply_statusbar(el, wrapper, dbg_name);
        self.apply_slider(el, wrapper, self_name, dbg_name);
        self.apply_colorselect(el, wrapper, self_name, dbg_name);
        self.apply_button(el, wrapper, self_name, dbg_name);
        self.apply_editbox(el, wrapper, dbg_name);
        self.apply_messageframe(el, wrapper, dbg_name);
        self.apply_simplehtml(el, wrapper, dbg_name);
        self.apply_minimap(el, wrapper, dbg_name);
        // 6 · <Scripts> handlers (`0x769ef0`); OnLoad is captured to fire bottom-up below.
        let onload = self.apply_scripts(el, wrapper, dbg_name);

        // 7 · nested <Frames>, whose OnLoads fire before ours (`0x76a060`).
        for frames_el in children_named(el, "Frames") {
            for child in &frames_el.children {
                let expanded = self.expand(child);
                self.materialize(&expanded, Some(wrapper), self_name);
            }
        }

        // 7b · <ScrollChild>: its one child is built through `Instantiate 0x6ee280` like
        //      `<Frames>`, then stored at `+0x318` with `+0x314 = 1`, which is `SetScrollChild`
        //      and gives the ScrollFrame its scroll range. An empty one errors in the reference.
        for sc in children_named(el, "ScrollChild") {
            let Some(child) = sc.children.first() else {
                self.report.errors.push(format!(
                    "{dbg_name}: <ScrollChild> is empty — the reference errors here, and a \
                     ScrollFrame with no child has no scroll range at all"
                ));
                continue;
            };
            let expanded = self.expand(child);
            let Some(child_wrapper) = self.materialize(&expanded, Some(wrapper), self_name) else {
                continue; // materialize already reported why
            };
            if let Err(e) = wrapper.call_method::<()>("SetScrollChild", child_wrapper) {
                self.report
                    .errors
                    .push(format!("{dbg_name}: <ScrollChild>: {e}"));
            }
        }

        // 7c · deferred anchors: every child exists now, as at the reference's layout pass.
        self.drain_deferred_anchors(deferred_mark);
        // 8 · this frame's OnLoad, now that its subtree is complete (bottom-up).
        if let Some(func) = onload {
            self.fire_onload(wrapper, &func, dbg_name);
        }
    }

    /// Apply one anchor, deferring it (when `may_defer`) while its named `relativeTo` is unbuilt.
    pub(super) fn apply_anchor(&mut self, d: DeferredAnchor, may_defer: bool) {
        let rel: Value = match d.args.1.as_deref() {
            Some(name) => {
                match crate::script::object::anchor_args::resolve_xml_relative_to(self.lua(), name)
                {
                    Some(t) => Value::Table(t),
                    // Not built yet: retry once the subtree is complete; a miss then is real.
                    None if may_defer => {
                        self.deferred_anchors.push(d);
                        return;
                    }
                    None => {
                        self.report
                            .warnings
                            .push(format!("{}: Couldn't find relative frame: {name}", d.dbg));
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
            args: (point, _, rel_point, x, y),
            dbg,
        } = d;
        let args = (point, rel, rel_point, x, y);
        if region {
            self.call_region(&wrapper, "SetPoint", args, &dbg);
        } else {
            self.call(&wrapper, "SetPoint", args, &dbg);
        }
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

/// An `x`/`y` pair from a `<Size>`/`<Offset>`: its `<AbsDimension>` child, else its own attributes.
pub(super) fn abs_dim(el: &Element) -> (Option<f32>, Option<f32>) {
    let src = children_named(el, "AbsDimension").next().unwrap_or(el);
    let x = src.attr("x").and_then(|v| v.trim().parse::<f32>().ok());
    let y = src.attr("y").and_then(|v| v.trim().parse::<f32>().ok());
    (x, y)
}

/// Read an RGBA colour from a `<Color r= g= b= a=/>` element (`a` defaults to 1.0).
pub(super) fn color_of(el: &Element) -> [f32; 4] {
    let c = |k: &str, d: f32| {
        el.attr(k)
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(d)
    };
    [c("r", 0.0), c("g", 0.0), c("b", 0.0), c("a", 1.0)]
}

/// A `<TexCoords>` child's UV rect `(l, r, t, b)`; one missing an edge is ignored (full UVs).
pub(super) fn tex_coords_of(el: &Element) -> Option<(f32, f32, f32, f32)> {
    let tc = children_named(el, "TexCoords").next()?;
    let edge = |k: &str| tc.attr(k).and_then(|v| v.trim().parse::<f32>().ok());
    Some((edge("left")?, edge("right")?, edge("top")?, edge("bottom")?))
}

/// A `<FontHeight>`-style scalar: its `<AbsValue val=>` child, else its own `val=`.
pub(super) fn abs_value(el: &Element) -> Option<f32> {
    let src = children_named(el, "AbsValue").next().unwrap_or(el);
    src.attr("val").and_then(|v| v.trim().parse::<f32>().ok())
}

/// A [`FontObject`] from a flattened `<Font>`. The merge appends inherited nodes first, so the last
/// `<FontHeight>`, `<Color>` and `<Shadow>` win.
pub(super) fn font_object_from_element(el: &Element) -> FontObject {
    let justify_h = el
        .attr("justifyH")
        .map(|j| match j.to_ascii_uppercase().as_str() {
            "LEFT" => JustifyH::Left,
            "RIGHT" => JustifyH::Right,
            _ => JustifyH::Center,
        });
    let justify_v = el
        .attr("justifyV")
        .map(|j| match j.to_ascii_uppercase().as_str() {
            "TOP" => JustifyV::Top,
            "BOTTOM" => JustifyV::Bottom,
            _ => JustifyV::Middle,
        });
    // `<Shadow>` inherits like any child: `MasterFont`'s (1,-1) black reaches every `GameFont*`
    // (`Fonts.xml:55`).
    let shadow = children_named(el, "Shadow").last().map(|sh| {
        let offset = children_named(sh, "Offset")
            .next()
            .map(abs_dim)
            .map(|(x, y)| [x.unwrap_or(0.0), y.unwrap_or(0.0)])
            .unwrap_or([0.0, 0.0]);
        let color = children_named(sh, "Color")
            .next()
            .map(color_of)
            .unwrap_or([0.0, 0.0, 0.0, 1.0]);
        crate::script::FontShadow { offset, color }
    });
    FontObject {
        font: el.attr("font").map(str::to_string),
        height: children_named(el, "FontHeight").last().and_then(abs_value),
        color: children_named(el, "Color").last().map(color_of),
        outline: el.attr("outline").map(Outline::parse).unwrap_or_default(),
        justify_h,
        justify_v,
        shadow,
    }
}

#[cfg(test)]
mod tests;
