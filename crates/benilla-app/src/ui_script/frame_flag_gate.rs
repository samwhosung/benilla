//! The reference's frame flags as a test oracle: every frame both UIs name must carry the
//! reference's `toplevel`, effective mouse enable, `id` and parent.
//!
//! Our side is read off the loaded engine (`IsToplevel`, `IsMouseEnabled`, `GetID`, `GetParent`),
//! never our XML: a renamed template hides from a name-keyed diff, and a mouse handler in
//! `<Scripts>` reaches the same enable as `enableMouse=` (`0x76af00(2,-1)`). The reference side is
//! its XML off the player's chain, so a `SetID` or `EnableMouse` it makes from Lua reads as absent,
//! which can only under-report.
//!
//! A parent comes from `parent=`, on the element or a template it inherits, or from nesting in
//! another frame's `<Frames>`. It decides whether a frame can show over a fullscreen panel:
//! `SetFullScreenFrame` hides `UIParent` and all below it (`UIParent.lua:852-861`). Templates are
//! compared through their instances, so a [`KNOWN`] entry naming one reads as stale.
//!
//! `movable`, `frameStrata`, `setAllPoints` and `hidden` are out of scope: each has an equivalent
//! spelling or is not mechanical.

use std::collections::{HashMap, HashSet};

use benilla_ui::framexml::{self, Element, TopLevel};

/// The frame tags; a region carries a name too and must not shadow a frame of that name.
const FRAME_TAGS: &[&str] = &[
    "Frame",
    "Button",
    "CheckButton",
    "EditBox",
    "ScrollFrame",
    "Slider",
    "StatusBar",
    "MessageFrame",
    "ScrollingMessageFrame",
    "Model",
    "PlayerModel",
    "DressUpModel",
    "TabardModel",
    "ColorSelect",
    "SimpleHTML",
    "GameTooltip",
    "Minimap",
    "MovieFrame",
    "WorldFrame",
];

/// The `<Scripts>` handlers that auto-enable the mouse (the kind-2 OR-chain,
/// `0x769fb7`..`0x76a022`); `OnDragStop` and `OnReceiveDrag` enable nothing.
const MOUSE_HANDLERS: &[&str] = &[
    "OnEnter",
    "OnLeave",
    "OnMouseDown",
    "OnMouseUp",
    "OnDragStart",
];

/// Whether the constructor mouse-enables this widget kind; asked of the engine, never a copy.
fn tag_mouse_enabled_by_ctor(tag: &str) -> bool {
    benilla_ui::script::frame_kind_from_tag(tag)
        .is_some_and(benilla_ui::widget::mouse_enabled_by_ctor)
}

/// One accepted difference: benilla's frame name, the flag, and why the difference is right.
struct Known {
    frame: &'static str,
    flag: Flag,
    why: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Flag {
    Toplevel,
    Mouse,
    Id,
    Parent,
}

impl std::fmt::Display for Flag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Flag::Toplevel => "toplevel",
            Flag::Mouse => "mouse",
            Flag::Id => "id",
            Flag::Parent => "parent",
        })
    }
}

/// The accepted differences, each with its reason; empty, so any new divergence fails the gate.
const KNOWN: &[Known] = &[
    // A parent entry is acceptable only inside `UIParent`'s tree: a seat crossing between it and
    // the top level is a defect, since `SetFullScreenFrame` hides `UIParent` and all below it.
];

/// Every stock FrameXML document off the player's chain, in `FrameXML.toc` order with each
/// `<Include>`, once each; `None` without an install.
fn reference_documents() -> Option<Vec<framexml::ParsedDocument>> {
    let data = benilla_formats::wow_data()?;
    let chain = benilla_formats::open_chain(&data).ok()?;
    let toc = chain.read("Interface\\FrameXML\\FrameXML.toc").ok()?;
    let toc = benilla_ui::toc::Toc::parse(&benilla_ui::source::decode(&toc));
    let mut queue: Vec<String> = toc
        .files
        .iter()
        .filter(|f| f.to_ascii_lowercase().ends_with(".xml"))
        .map(|f| format!("Interface\\FrameXML\\{f}"))
        .collect();
    let mut seen = HashSet::new();
    let mut docs = Vec::new();
    while !queue.is_empty() {
        let path = queue.remove(0);
        if !seen.insert(path.to_ascii_lowercase()) {
            continue;
        }
        let Ok(bytes) = chain.read(&path) else {
            continue;
        };
        // Some files carry a UTF-8 BOM and stray high bytes in comments: read lossily.
        let text = String::from_utf8_lossy(&bytes);
        let Ok(doc) = framexml::parse(text.trim_start_matches('\u{feff}')) else {
            continue;
        };
        let dir = path.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
        for item in &doc.items {
            if let TopLevel::Include(file) = item {
                queue.push(format!("{dir}\\{file}"));
            }
        }
        docs.push(doc);
    }
    Some(docs)
}

/// Every named frame element in the corpus, templates and instances, nested ones included;
/// `$parent` names are skipped, as they name nothing on their own.
fn reference_frames() -> Option<HashMap<String, Element>> {
    let mut out: HashMap<String, Element> = HashMap::new();
    for doc in reference_documents()? {
        for item in &doc.items {
            if let TopLevel::Template(el) | TopLevel::Instance(el) = item {
                collect_named(el, &mut out);
            }
        }
    }
    Some(out)
}

/// Publish every named frame under `el`; the first name wins, as in the client's auto-publish.
fn collect_named(el: &Element, out: &mut HashMap<String, Element>) {
    if FRAME_TAGS.iter().any(|t| t.eq_ignore_ascii_case(&el.tag)) {
        if let Some(name) = el.attr("name") {
            if !name.contains("$parent") {
                out.entry(name.to_string()).or_insert_with(|| el.clone());
            }
        }
    }
    for child in &el.children {
        collect_named(child, out);
    }
}

/// Each named frame's nearest named enclosing frame: the nesting half of its parent, where
/// [`resolved_attr`] reads `parent=` and wins. `ScreenshotStatus` has no `parent=` and is nested
/// in `WorldFrame` (`WorldFrame.xml:42`); `DropDownList1` has neither.
fn collect_nesting(
    el: &Element,
    enclosing: Option<&str>,
    out: &mut HashMap<String, Option<String>>,
) {
    let mut inner = enclosing;
    if FRAME_TAGS.iter().any(|t| t.eq_ignore_ascii_case(&el.tag)) {
        if let Some(name) = el.attr("name") {
            if !name.contains("$parent") {
                out.entry(name.to_string())
                    .or_insert_with(|| enclosing.map(str::to_string));
                inner = Some(name);
            }
        }
    }
    for child in &el.children {
        collect_nesting(child, inner, out);
    }
}

/// The reference's nesting table, built over the same corpus [`reference_frames`] reads.
fn reference_nesting() -> Option<HashMap<String, Option<String>>> {
    let mut out: HashMap<String, Option<String>> = HashMap::new();
    for doc in reference_documents()? {
        for item in &doc.items {
            if let TopLevel::Template(el) | TopLevel::Instance(el) = item {
                collect_nesting(el, None, &mut out);
            }
        }
    }
    Some(out)
}

/// The templates `el` inherits, last first: a later name overrides an earlier one.
fn inherits(el: &Element) -> impl Iterator<Item = &str> {
    el.attr("inherits")
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
}

/// `attr` on `name`, else on its templates; `depth` guards a malformed cycle.
fn resolved_attr<'a>(
    name: &str,
    frames: &'a HashMap<String, Element>,
    attr: &str,
    depth: u32,
) -> Option<&'a str> {
    if depth > 16 {
        return None;
    }
    let el = frames.get(name)?;
    if let Some(v) = el.attr(attr) {
        return Some(v);
    }
    inherits(el).find_map(|t| resolved_attr(t, frames, attr, depth + 1))
}

/// Whether any element in `name`'s inherits chain declares one of `handlers` inside `<Scripts>`.
fn declares_handler(
    name: &str,
    frames: &HashMap<String, Element>,
    handlers: &[&str],
    depth: u32,
) -> bool {
    if depth > 16 {
        return false;
    }
    let Some(el) = frames.get(name) else {
        return false;
    };
    let own = el
        .children
        .iter()
        .filter(|c| c.tag.eq_ignore_ascii_case("Scripts"))
        .flat_map(|s| s.children.iter())
        .any(|h| handlers.iter().any(|w| w.eq_ignore_ascii_case(&h.tag)));
    own || inherits(el).any(|t| declares_handler(t, frames, handlers, depth + 1))
}

/// Whether the reference frame takes the mouse once loaded: `0x76af00(2, -1)` is reached from its
/// constructor, `enableMouse`, or an auto-enabling handler.
fn reference_takes_mouse(name: &str, frames: &HashMap<String, Element>) -> bool {
    let Some(el) = frames.get(name) else {
        return false;
    };
    if tag_mouse_enabled_by_ctor(&el.tag) {
        return true;
    }
    if resolved_attr(name, frames, "enableMouse", 0).is_some_and(|v| v.eq_ignore_ascii_case("true"))
    {
        return true;
    }
    declares_handler(name, frames, MOUSE_HANDLERS, 0)
}

/// The reference name for one of ours: the same name, or the one behind a `Benilla` prefix.
fn reference_name<'a>(ours: &str, frames: &'a HashMap<String, Element>) -> Option<&'a str> {
    if let Some((k, _)) = frames.get_key_value(ours) {
        return Some(k);
    }
    let bare = ours.strip_prefix("Benilla")?;
    frames.get_key_value(bare).map(|(k, _)| k.as_str())
}

/// One divergence found by the sweep, rendered for the failure message.
fn describe(frame: &str, flag: Flag, ours: &str, theirs: &str) -> String {
    format!("  {frame} — {flag}: ours {ours}, reference {theirs}")
}

/// Every frame both UIs name carries the reference's `toplevel`, mouse enable, `id` and parent,
/// read off the loaded engine; [`KNOWN`] holds the only accepted differences.
#[test]
fn the_shipped_frames_carry_the_references_flags() {
    let _data = benilla_formats::wow_data_or_skip!();
    let reference = reference_frames().expect("the reference FrameXML off the player's chain");
    let nesting = reference_nesting().expect("the same corpus the frames came from");
    assert!(
        reference.len() > 500,
        "only {} reference frames parsed — the corpus scan broke, and a sweep over nothing \
         passes whatever we did",
        reference.len()
    );

    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The UI loads on world entry, so a player always exists by then.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    s.resolve();

    // `GetParent()` by name, so nesting and `parent=` read alike; "" is a top-level frame.
    let parent_of = |n: &str| -> Option<String> {
        s.eval::<String>(&format!(
            "local f = getglobal(\"{n}\") if not f or not f.GetParent then return \"\" end \
             local p = f:GetParent() return (p and p:GetName()) or \"\""
        ))
        .ok()
        .filter(|v| !v.is_empty())
    };

    let flags = |n: &str| -> Option<(bool, bool, i64)> {
        s.eval::<i64>(&format!(
            "local f = getglobal(\"{n}\") \
             if not f or not f.IsToplevel then return -1 end \
             return (f:IsToplevel() and 1 or 0) + (f:IsMouseEnabled() and 2 or 0)"
        ))
        .ok()
        .filter(|v| *v >= 0)
        .map(|v| {
            let id = s
                .eval::<i64>(&format!("return getglobal(\"{n}\"):GetID()"))
                .unwrap_or(0);
            (v & 1 == 1, v & 2 == 2, id)
        })
    };

    let mut divergences: Vec<String> = Vec::new();
    let mut compared = 0usize;
    // A divergence claims its KNOWN entry; one left unclaimed at the end is stale.
    let mut unused: Vec<&Known> = KNOWN.iter().collect();

    let mut ours: Vec<String> = super::shipped_xml_tests::shipped_frame_names();
    ours.sort();
    for name in &ours {
        let Some(theirs) = reference_name(name, &reference) else {
            continue;
        };
        let Some((toplevel, mouse, id)) = flags(name) else {
            continue; // not a frame in the loaded tree (a template, or a region name)
        };
        compared += 1;

        let want_toplevel = resolved_attr(theirs, &reference, "toplevel", 0)
            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
        let want_mouse = reference_takes_mouse(theirs, &reference);
        let want_id: i64 = resolved_attr(theirs, &reference, "id", 0)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        // `parent=` wins over nesting and follows `inherits=`: the chat frames take theirs from
        // `FloatingChatFrameTemplate` and `ChatTabTemplate` (`FloatingChatFrame.xml:211`, `:36`).
        // A parent name goes through the same `Benilla` prefix map as a frame's.
        let want_parent = resolved_attr(theirs, &reference, "parent", 0)
            .map(str::to_string)
            .or_else(|| nesting.get(theirs).cloned().flatten());
        let parent = parent_of(name);
        let parent_matches = match (&parent, &want_parent) {
            (None, None) => true,
            (Some(p), Some(w)) => p == w || p.strip_prefix("Benilla") == Some(w.as_str()),
            _ => false,
        };
        let show = |p: &Option<String>| p.clone().unwrap_or_else(|| "(top-level)".into());

        for (flag, differs, ours_s, theirs_s) in [
            (
                Flag::Toplevel,
                toplevel != want_toplevel,
                toplevel.to_string(),
                want_toplevel.to_string(),
            ),
            (
                Flag::Mouse,
                mouse != want_mouse,
                mouse.to_string(),
                want_mouse.to_string(),
            ),
            (Flag::Id, id != want_id, id.to_string(), want_id.to_string()),
            (
                Flag::Parent,
                !parent_matches,
                show(&parent),
                show(&want_parent),
            ),
        ] {
            if !differs {
                continue;
            }
            if KNOWN.iter().any(|k| k.frame == name && k.flag == flag) {
                unused.retain(|u| !(u.frame == name && u.flag == flag));
                continue;
            }
            divergences.push(describe(name, flag, &ours_s, &theirs_s));
        }
    }

    assert!(
        // The floor guards the pairing, not the census: the paired count falls as each of our
        // files declaring a reference-named frame retires.
        compared > 5,
        "only {compared} frames compared — the pairing broke, and the sweep guards nothing"
    );
    assert!(
        divergences.is_empty(),
        "{} frame flag(s) diverge from the reference. Each is either a defect to fix or an entry \
         to add to KNOWN with the reason it is right — never a tolerance:\n{}",
        divergences.len(),
        divergences.join("\n")
    );
    let stale: Vec<String> = unused
        .iter()
        .map(|k| format!("  {} ({}) — claimed: {}", k.frame, k.flag, k.why))
        .collect();
    assert!(
        stale.is_empty(),
        "{} KNOWN entr(y/ies) no longer describe a real difference — delete them, an accepted \
         divergence that has been fixed is documentation claiming a defect we do not have:\n{}",
        stale.len(),
        stale.join("\n")
    );
}
