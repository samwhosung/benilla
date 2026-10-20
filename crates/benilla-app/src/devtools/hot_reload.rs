//! Hot reload: an edited `.lua` file of a loaded addon, or of benilla's own layer, takes effect in
//! the running UI, its functions swapped into the live closures (`benilla_lua::reload`), state and
//! registrations kept; an edited handler body in one of their `.xml` files is swapped into the
//! frames running it. Each file is checked about twice a second, a slice of them each frame; each
//! reload is reported in the chat frame.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use benilla_ui::mlua::benilla_lua::reload::{top_level_changed, Report};
use benilla_ui::mlua::{ObjectLike, Table};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

/// How often each file is checked, and the VM's list of loaded files read again.
const POLL_SECS: f32 = 0.5;
/// Reloads the profiler panel lists.
pub(super) const LOG_LINES: usize = 6;

#[derive(Resource, Default)]
pub(crate) struct HotReload {
    /// The VM session the files below were read from.
    session: u64,
    /// Each source id's file on disk, for a chunk the loader read from the addon folder, as of
    /// source epoch `epoch` (`Heap::sources_epoch`): a freed slot may hold another chunk later.
    files: Vec<Option<PathBuf>>,
    epoch: u64,
    /// Each chunk name's file, resolved once.
    resolved: HashMap<Box<[u8]>, Option<PathBuf>>,
    /// Every watched file, in the order the slices check them: a `.lua` file by the source id the
    /// VM loaded it as, or an `.xml` file.
    watch: Vec<(Option<usize>, PathBuf)>,
    /// Where the next frame's slice of `watch` starts.
    cursor: usize,
    mtimes: HashMap<PathBuf, SystemTime>,
    /// Each watched `.lua` file's text when first seen, the one its top level ran: an edit of the
    /// top level is told apart from it.
    ran: HashMap<PathBuf, Vec<u8>>,
    /// Each watched `.xml` file's addon and its text as last read: an edited handler's old body
    /// is what finds the frames running it.
    xml: HashMap<PathBuf, (String, String)>,
    /// The folders whose `.xml` files are watched, each walked once a VM session.
    walked: HashSet<PathBuf>,
    next_poll: f32,
    /// The latest reloads, newest last: the time, the line and whether it applied.
    pub(super) log: VecDeque<(f32, String, bool)>,
}

pub(super) fn poll(
    time: Res<Time>,
    mut hosted_vm: crate::script_calls::HostedVm,
    mut hr: ResMut<HotReload>,
) {
    hosted_vm.run(|script| {
        let (Some(root), Some(script)) = (crate::ui_script::addons::root(), script) else {
            return;
        };
        let hr = &mut *hr;
        if hr.session != script.session() {
            hr.session = script.session();
            hr.files.clear();
            hr.epoch = u64::MAX;
            hr.watch.clear();
            hr.mtimes.clear();
            hr.ran.clear();
            hr.xml.clear();
            hr.walked.clear();
            hr.next_poll = 0.0;
        }
        let now = time.elapsed_secs();
        if now >= hr.next_poll {
            hr.next_poll = now + POLL_SECS;
            refresh(hr, script, &root);
        }
        let n = hr.watch.len();
        if n == 0 {
            return;
        }
        let mut changed = Vec::new();
        for _ in 0..slice_len(n, time.delta_secs()) {
            hr.cursor = (hr.cursor + 1) % n;
            let (id, path) = &hr.watch[hr.cursor];
            let Ok(t) = std::fs::metadata(path).and_then(|m| m.modified()) else {
                continue;
            };
            match hr.mtimes.insert(path.clone(), t) {
                // A file first seen is recorded, not reloaded.
                None if id.is_some() => {
                    if let Ok(bytes) = std::fs::read(path) {
                        let text = benilla_ui::source::chunk(&bytes).to_vec();
                        hr.ran.insert(path.clone(), text);
                    }
                }
                Some(old) if old != t => changed.push((*id, path.clone())),
                _ => {}
            }
        }
        for (id, path) in changed {
            let (line, ok) = match id {
                Some(id) => reload(script, &root, id, &path, hr.ran.get(&path)),
                None => reload_xml(script, &root, &path, &mut hr.xml),
            };
            info!("devtools: {line}");
            chat(script, &line, ok);
            hr.log.push_back((now, line, ok));
            while hr.log.len() > LOG_LINES {
                hr.log.pop_front();
            }
        }
    })
}

/// How many of `n` watched files a frame `dt` long checks, so that each is checked about every
/// [`POLL_SECS`] and no frame stats them all: a few thousand addon files take milliseconds.
fn slice_len(n: usize, dt: f32) -> usize {
    ((n as f32 * dt / POLL_SECS).ceil() as usize).clamp(1, n)
}

/// Read the VM's list of loaded chunks again when it moved, and with it the watch list.
fn refresh(hr: &mut HotReload, script: &UiScript, root: &Path) {
    let layer = crate::run_mode::dev_source_dir().map(|d| d.join("assets/ui"));
    let moved = script.lua().with_vm(|vm| {
        let heap = &vm.heap;
        if hr.epoch == heap.sources_epoch() {
            return false;
        }
        hr.epoch = heap.sources_epoch();
        let resolved = &mut hr.resolved;
        hr.files = heap
            .sources
            .iter()
            .map(|name| {
                let name = name.as_ref()?;
                if let Some(p) = resolved.get(&name[..]) {
                    return p.clone();
                }
                let p = file_of(root, layer.as_deref(), &String::from_utf8_lossy(name));
                resolved.insert(Box::from(&name[..]), p.clone());
                p
            })
            .collect();
        true
    });
    if !moved {
        return;
    }
    watch_xml(hr, root, layer.as_deref());
    let lua = hr
        .files
        .iter()
        .enumerate()
        .filter_map(|(id, p)| Some((Some(id), p.clone()?)));
    let xml = hr.xml.keys().map(|p| (None, p.clone()));
    hr.watch = lua.chain(xml).collect();
}

/// The file a chunk was read from: the loader names it `@Interface\AddOns\<Folder>\<file>`, in
/// the `.toc`'s spelling, matched on disk case-insensitively as the loader matched it. Failing
/// that, a file of benilla's layer (`@Interface\AddOns\benilla\<file>`), which a dev build reads
/// from `layer`, the source tree's `assets/ui`.
fn file_of(root: &Path, layer: Option<&Path>, chunk: &str) -> Option<PathBuf> {
    let rel = chunk.strip_prefix('@')?;
    if let Some(p) = benilla_assets::loose_addon_file(root, &benilla_assets::normalize_path(rel)) {
        return Some(p);
    }
    let file = rel.strip_prefix("Interface\\AddOns\\benilla\\")?;
    let p = layer?.join(file.replace('\\', "/"));
    p.is_file().then_some(p)
}

/// Watch every `.xml` file under the folder of each addon a watched `.lua` file is in, and under
/// the layer's, each folder walked once: its text is read now, the old text of a later edit.
fn watch_xml(hr: &mut HotReload, root: &Path, layer: Option<&Path>) {
    let mut dirs: Vec<(String, PathBuf)> = Vec::new();
    for path in hr.files.iter().flatten() {
        let dir = if let Some(l) = layer.filter(|l| path.starts_with(l)) {
            ("benilla".to_string(), l.to_path_buf())
        } else if let Ok(rel) = path.strip_prefix(root) {
            let Some(folder) = rel.components().next() else {
                continue;
            };
            let folder = folder.as_os_str().to_string_lossy().into_owned();
            (folder.clone(), root.join(folder))
        } else {
            continue;
        };
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    for (addon, dir) in dirs {
        if !hr.walked.insert(dir.clone()) {
            continue;
        }
        let mut found = Vec::new();
        walk_xml(&dir, &mut found);
        for path in found {
            if hr.xml.contains_key(&path) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let text = benilla_ui::source::decode(&bytes).into_owned();
            if let Ok(t) = std::fs::metadata(&path).and_then(|m| m.modified()) {
                hr.mtimes.insert(path.clone(), t);
            }
            hr.xml.insert(path, (addon.clone(), text));
        }
    }
}

fn walk_xml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_xml(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml")) {
            out.push(p);
        }
    }
}

/// One handler in an XML file, in document order: its tag, its frame's chunk name when the file
/// alone names the frame (a named instance, no `$parent`), and its body.
#[derive(Debug, PartialEq)]
struct Handler {
    tag: String,
    chunk: Option<String>,
    body: String,
}

fn handlers(text: &str) -> Option<Vec<Handler>> {
    use benilla_ui::framexml::{Element, TopLevel};
    fn walk(el: &Element, concrete: bool, out: &mut Vec<Handler>) {
        let name = el.name().filter(|n| !n.is_empty() && !n.contains('$'));
        for child in &el.children {
            if child.tag.eq_ignore_ascii_case("Scripts") {
                for h in &child.children {
                    out.push(Handler {
                        tag: h.tag.clone(),
                        chunk: name.filter(|_| concrete).map(|n| format!("{n}:{}", h.tag)),
                        body: h.body.clone(),
                    });
                }
            } else {
                walk(child, concrete, out);
            }
        }
    }
    let doc = benilla_ui::framexml::parse(text).ok()?;
    let mut out = Vec::new();
    for item in &doc.items {
        match item {
            TopLevel::Instance(el) => walk(el, true, &mut out),
            TopLevel::Template(el) => walk(el, false, &mut out),
            _ => {}
        }
    }
    Some(out)
}

/// Reload one edited `.xml` file's handler bodies: each changed body goes into the live handlers
/// running the old one, the frame's own when the file names it, else every one of its tag the
/// file's addon owns. Anything else an XML edit changes takes `/reload`.
fn reload_xml(
    script: &UiScript,
    root: &Path,
    path: &Path,
    xml: &mut HashMap<PathBuf, (String, String)>,
) -> (String, bool) {
    let shown = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();
    let (Some((addon, old_text)), Ok(bytes)) = (xml.get_mut(path), std::fs::read(path)) else {
        return (format!("{shown}: unreadable"), false);
    };
    let new_text = benilla_ui::source::decode(&bytes).into_owned();
    let old_text = std::mem::replace(old_text, new_text.clone());
    let (Some(old), Some(new)) = (handlers(&old_text), handlers(&new_text)) else {
        return (format!("{shown}: not well-formed FrameXML"), false);
    };
    let same_shape = old.len() == new.len()
        && old
            .iter()
            .zip(&new)
            .all(|(a, b)| a.tag.eq_ignore_ascii_case(&b.tag) && a.chunk == b.chunk);
    if !same_shape {
        return (
            format!("{shown}: handlers added, removed or moved, which takes /reload"),
            false,
        );
    }
    let (mut bodies, mut closures) = (0, 0);
    let (mut idle, mut shared) = (Vec::new(), Vec::new());
    for (i, (a, b)) in old.iter().zip(&new).enumerate() {
        if a.body == b.body {
            continue;
        }
        let label = a.chunk.clone().unwrap_or_else(|| a.tag.clone());
        // Without its frame's name, a body is found by its code: one the file has twice under
        // the same tag names no one handler.
        if a.chunk.is_none()
            && old.iter().enumerate().any(|(j, o)| {
                j != i
                    && o.chunk.is_none()
                    && o.tag.eq_ignore_ascii_case(&a.tag)
                    && o.body == a.body
            })
        {
            shared.push(label);
            continue;
        }
        let suffix = format!(":{}", a.tag);
        let r = script.lua().with_vm(|vm| {
            let owners = super::owners::owners_of(&vm.heap);
            vm.hot_reload_mains(a.body.as_bytes(), b.body.as_bytes(), |p| match &a.chunk {
                Some(c) => &*p.source == c.as_bytes(),
                None => {
                    p.source.ends_with(suffix.as_bytes())
                        && owners.get(p.source_id as usize).is_some_and(|o| o == addon)
                }
            })
        });
        match r {
            Ok(0) => idle.push(label),
            Ok(n) => (bodies, closures) = (bodies + 1, closures + n),
            Err(e) => return (format!("{shown}: {label}: {e}"), false),
        }
    }
    if bodies + idle.len() + shared.len() == 0 {
        return (
            format!("{shown}: no handler body changed; the rest of an XML edit takes /reload"),
            false,
        );
    }
    let mut line = format!("{shown}: {bodies} handler(s) swapped into {closures} closure(s)");
    if !idle.is_empty() {
        line += &format!("; no live frame runs the old body of: {}", idle.join(", "));
    }
    if !shared.is_empty() {
        line += &format!(
            "; kept, the old body is shared by several handlers: {}",
            shared.join(", ")
        );
    }
    (line, idle.is_empty() && shared.is_empty())
}

/// Reload one file into the chunk the loader compiled it as, source `id`, whose top level ran as
/// `ran`; the report line and whether it applied.
fn reload(
    script: &UiScript,
    root: &Path,
    id: usize,
    path: &Path,
    ran: Option<&Vec<u8>>,
) -> (String, bool) {
    let shown = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();
    let Ok(bytes) = std::fs::read(path) else {
        return (format!("{shown}: unreadable"), false);
    };
    let src = benilla_ui::source::chunk(&bytes);
    let r = script.lua().with_vm(|vm| {
        let name = vm.heap.source_name(id as u32).cloned()?;
        Some(vm.hot_reload(&name, src))
    });
    let Some(r) = r else {
        return (format!("{shown}: no longer loaded"), false);
    };
    match r {
        Ok(r) => {
            let top = ran.is_some_and(|old| top_level_changed(old, src));
            report_line(&shown, &r, top)
        }
        Err(e) => (format!("{shown}: {e}"), false),
    }
}

/// What a reload did, in a line, and whether all of it took effect: `top` says the file's top
/// level differs from the one that ran.
fn report_line(shown: &str, r: &Report, top: bool) -> (String, bool) {
    // A new function nested in one that was swapped exists from that one's next call; any other
    // is made by code that already ran.
    let (live, later): (Vec<&String>, Vec<&String>) = r.added.iter().partition(|k| {
        k.rsplit_once('/')
            .is_some_and(|(parent, _)| r.swapped.iter().any(|s| s == parent))
    });
    let mut line = format!(
        "{shown}: {} function(s) swapped into {} closure(s)",
        r.swapped.len(),
        r.closures
    );
    if !r.reshaped.is_empty() {
        line += &format!(
            "; kept old code for {} (captures changed): {}",
            r.reshaped.len(),
            r.reshaped.join(", ")
        );
    }
    if !live.is_empty() {
        line += &format!("; {} new, live on their parent's next call", live.len());
    }
    if !later.is_empty() {
        line += &format!("; {} new, live after /reload", later.len());
    }
    if !r.removed.is_empty() {
        line += &format!(
            "; {} removed, still on the old code where held: {}",
            r.removed.len(),
            r.removed.join(", ")
        );
    }
    if top {
        line += "; top-level code changed, which runs again only on /reload";
    }
    let complete = r.reshaped.is_empty() && later.is_empty() && !top;
    (line, complete)
}

/// The line in the default chat frame: green when it applied, red when it did not.
fn chat(script: &UiScript, line: &str, ok: bool) {
    let lua = script.lua();
    let Ok(frame) = lua.globals().get::<Table>("DEFAULT_CHAT_FRAME") else {
        return;
    };
    let (r, g, b) = if ok { (0.4, 1.0, 0.6) } else { (1.0, 0.4, 0.4) };
    let _ = frame.call_method::<()>("AddMessage", (format!("hot reload: {line}"), r, g, b));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_file_is_checked_every_poll_and_no_frame_checks_them_all() {
        for dt in [1.0 / 30.0, 1.0 / 60.0, 1.0 / 240.0] {
            let n = 2200;
            let frames = (POLL_SECS / dt).round() as usize;
            assert!(slice_len(n, dt) * frames >= n, "{dt}: all within a poll");
            assert!(slice_len(n, dt) < n / 10, "{dt}: a slice");
        }
        assert_eq!(slice_len(3, 0.0), 1, "the first frame still checks one");
        assert_eq!(slice_len(3, 5.0), 3, "a long frame checks each once");
    }

    #[test]
    fn handlers_are_listed_with_their_frames_chunk_names() {
        let xml = r#"<Ui>
  <Frame name="Named"><Scripts><OnShow>a()</OnShow></Scripts>
    <Frames><Button name="$parentChild"><Scripts><OnClick>b()</OnClick></Scripts></Button></Frames>
  </Frame>
  <Frame name="Tmpl" virtual="true"><Scripts><OnLoad>c()</OnLoad></Scripts></Frame>
</Ui>"#;
        let h = handlers(xml).unwrap();
        let got: Vec<(&str, Option<&str>, &str)> = h
            .iter()
            .map(|h| (h.tag.as_str(), h.chunk.as_deref(), h.body.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("OnShow", Some("Named:OnShow"), "a()"),
                ("OnClick", None, "b()"),
                ("OnLoad", None, "c()"),
            ]
        );
    }

    #[test]
    fn the_line_says_what_did_not_take_effect() {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let r = Report {
            closures: 2,
            swapped: s(&["Outer", "F:SetScript"]),
            added: s(&["Outer/inner", "NewGlobal"]),
            removed: s(&["Old"]),
            ..Default::default()
        };
        let (line, complete) = report_line("A.lua", &r, true);
        assert_eq!(
            line,
            "A.lua: 2 function(s) swapped into 2 closure(s); 1 new, live on their parent's next \
             call; 1 new, live after /reload; 1 removed, still on the old code where held: Old; \
             top-level code changed, which runs again only on /reload"
        );
        assert!(!complete);
        let r = Report {
            closures: 1,
            swapped: s(&["Outer"]),
            added: s(&["Outer/inner"]),
            ..Default::default()
        };
        assert!(
            report_line("A.lua", &r, false).1,
            "a nested addition is live at once"
        );
    }
}
