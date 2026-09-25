//! Whole-tree guards over the shipped interface: `assets/ui` and the manifest it loads.

/// Every shipped `assets/ui/*.xml` parses (XML forbids `--` inside a comment). Parse-only: a file
/// loaded out of manifest order reports templates its predecessors define.
#[test]
fn every_shipped_ui_xml_parses() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "xml") {
            let text = std::fs::read_to_string(&path).expect("read");
            if let Err(e) = benilla_ui::framexml::parse(&text) {
                panic!("{}: {e}", path.display());
            }
            checked += 1;
        }
    }
    assert!(
        // A floor, not a census: a moved directory must not pass by sweeping nothing.
        checked >= 6,
        "only {checked} xml files swept — sweep broke"
    );
}

/// The whole manifest loads in its real order with zero loader errors, which the app only logs.
#[test]
fn the_whole_shipped_manifest_loads_without_errors() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, so a player always exists by then.
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
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Loading the UI makes no sound: a dropdown's OnLoad runs `UIDropDownMenu_Initialize`, which calls
/// its init at once (`UIDropDownMenu.lua:50`), and a unit popup's init reaches `UnitPopup_ShowMenu`
/// and its closing `PlaySound("igMainMenuOpen")` unless `UnitPopup_HideButtons` leaves only CANCEL
/// for a unit that does not exist (`UnitPopup.lua:115`).
#[test]
fn loading_the_shipped_ui_queues_no_sounds() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, so a player always exists by then.
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
    assert_eq!(
        s.take_sounds(),
        vec![],
        "loading the UI played a sound — a load-time handler is ringing; see \
         UnitPopup_HideButtons / UIDropDownMenu_Initialize"
    );
}

/// The stock pet bar with the hunter fixture (Claw autocasting), settled past its slide.
fn pet_bar_vm() -> benilla_ui::script::UiScript {
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::pet_bar_tests::load_pet_bar(&s);
    super::pet_bar_tests::declare_token_strings(&s);
    s.set_pet_actions(true, true, true, super::pet_bar_tests::hunter_slots());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    for _ in 0..3 {
        s.tick(0.05);
    }
    s.resolve();
    assert!(s.errors().is_empty(), "pet bar errors: {:?}", s.errors());
    s
}

/// The autocast brackets land on each button's corners: the art fills the middle 33/64 of
/// `UI-AutoCastableOverlay`, which puts the stock pet button's on its corners (58 × 33/64 = 29.9
/// on 30). Deviation: the spell book takes that ratio (`SpellBookAdapters.xml`), because its stock
/// brackets (60 × 33/64 = 30.9 on 37) sit three units inside each edge.
#[test]
fn the_autocast_brackets_reach_each_buttons_corners() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::framexml::{Element, TopLevel};

    /// The bracket art's share of `UI-AutoCastableOverlay.blp`, texels 15..47 of 64, measured with
    /// `benilla-extract blp`.
    const ART: f32 = 33.0 / 64.0;

    fn dim(el: &Element, tag: &str, axis: &str) -> Option<f32> {
        el.children
            .iter()
            .find(|c| c.tag.eq_ignore_ascii_case(tag))
            .and_then(|n| n.children.first())
            .and_then(|d| {
                d.attrs()
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(axis))
                    .map(|(_, v)| v.clone())
            })
            .and_then(|v| v.parse().ok())
    }

    /// Walk a template, carrying the nearest enclosing button size down to the overlay.
    fn walk(el: &Element, button: Option<f32>, out: &mut Vec<(String, f32, f32)>) {
        let button = dim(el, "Size", "x")
            .filter(|_| el.tag.ends_with("Button"))
            .or(button);
        let name = el
            .attrs()
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("name"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        if name.ends_with("AutoCastable") {
            if let (Some(size), Some(btn)) = (dim(el, "Size", "x"), button) {
                out.push((name.clone(), size * ART, btn));
            }
        }
        for child in &el.children {
            walk(child, button, out);
        }
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("assets/ui") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "xml") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read");
        for item in benilla_ui::framexml::parse(&src).expect("parse").items {
            match item {
                TopLevel::Font(el) | TopLevel::Template(el) | TopLevel::Instance(el) => {
                    walk(&el, None, &mut found)
                }
                TopLevel::Include(_) | TopLevel::Script(_) => {}
            }
        }
    }
    // The spell book's stock overlay is resized by a script, so it is measured off a VM.
    let s = super::spellbook_tests::spellbook_ui(1024.0, 768.0);
    let (overlay, button): (f32, f32) = s
        .eval("return SpellButton1AutoCastable:GetWidth(), SpellButton1:GetWidth()")
        .unwrap();
    found.push(("SpellButton1AutoCastable".into(), overlay * ART, button));
    // The pet button's is the stock template, measured the same way.
    let s = pet_bar_vm();
    let (overlay, button): (f32, f32) = s
        .eval("return PetActionButton1AutoCastable:GetWidth(), PetActionButton1:GetWidth()")
        .unwrap();
    found.push(("PetActionButton1AutoCastable".into(), overlay * ART, button));
    assert_eq!(found.len(), 2, "expected two autocast overlays: {found:?}");
    for (name, brackets, button) in &found {
        assert!(
            (brackets / button - 0.997).abs() < 0.01,
            "{name}: a {brackets:.1}-unit bracket square on a {button}-unit button is {:.3}x — the \
             pet button's is 0.997x, which is what puts brackets IN the corners",
            brackets / button
        );
    }
}

/// The autocast shine's rim runs on its viewport edge (1.024×), as the stock pet button's does
/// (`setAllPoints` on 30×30, `scale="1.2"`). Deviation: the spell book re-seats the stock model at
/// 1.48 on 37 units (`SpellBookAdapters.xml`), because its stock `scale="1.22"` in 36 units is a
/// 0.87× rim that floats clear of the edge and washes the icon.
#[test]
fn the_shine_panes_ask_for_the_rims_we_meant() {
    benilla_formats::wow_data_or_skip!();
    let mut found: Vec<(String, f32, f32)> = Vec::new();
    let mut s = super::spellbook_tests::spellbook_ui(1024.0, 768.0);
    s.run("ToggleSpellBook(BOOKTYPE_SPELL)").unwrap();
    s.tick(0.05);
    s.resolve();
    let (scale, view): (f32, f32) = s
        .eval("return SpellButton1AutoCast:GetModelScale(), SpellButton1AutoCast:GetWidth()")
        .unwrap();
    found.push(("SpellButton1AutoCast".into(), 0.02 * 1280.0 * scale, view));
    let s = pet_bar_vm();
    let (scale, view): (f32, f32) = s
        .eval(
            "return PetActionButton1AutoCast:GetModelScale(), PetActionButton1AutoCast:GetWidth()",
        )
        .unwrap();
    found.push((
        "PetActionButton1AutoCast".into(),
        0.02 * 1280.0 * scale,
        view,
    ));
    for (name, rim, view) in &found {
        assert!(
            (rim / view - 1.024).abs() < 1e-3,
            "{name}: a {rim}-unit rim in a {view}-unit viewport is {:.3}x, not the pet button's \
             1.024x — a rim that does not reach its own viewport edge is never clipped, and reads \
             as a wash rather than a rim",
            rim / view
        );
    }
}

/// Every texture path the shipped UI names resolves in the archives, through the renderer's own
/// `sprite_candidates` and its `.blp`/`.tga` fallback: a miss draws as a plain white quad. The
/// shape half runs without client data.
#[test]
fn every_shipped_texture_path_resolves_in_the_client_archives() {
    use benilla_ui::framexml::{Element, TopLevel};

    // `file=` on an element is an archive path; `<Script file=>` and `<Include file=>` are
    // top-level items, not elements.
    fn walk(el: &Element, file: &str, out: &mut Vec<(String, String, String)>) {
        // `<Model file=>` names a model, never drawn as a texture.
        if el.tag.eq_ignore_ascii_case("Model") {
            return;
        }
        for (key, value) in el.attrs() {
            let archive_path = ["file", "bgfile", "edgefile"]
                .contains(&key.to_ascii_lowercase().as_str())
                && !value.is_empty();
            if archive_path {
                out.push((file.to_string(), el.tag.clone(), value.clone()));
            }
        }
        for child in &el.children {
            walk(child, file, out);
        }
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut refs = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "xml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        let doc = benilla_ui::framexml::parse(&text).expect("parses");
        for item in &doc.items {
            match item {
                TopLevel::Font(el) | TopLevel::Template(el) | TopLevel::Instance(el) => {
                    walk(el, &file, &mut refs);
                }
                TopLevel::Include(_) | TopLevel::Script(_) => {}
            }
        }
    }
    // A floor, so the sweep cannot pass by matching nothing.
    assert!(refs.len() >= 50, "only {} texture paths swept", refs.len());

    // The shape half: a doubled separator is Lua escaping written into XML.
    for (file, tag, path) in &refs {
        assert!(
            !path.contains("\\\\"),
            "{file}: <{tag} file=\"{path}\"> has DOUBLED separators — XML attributes are not Lua \
             strings, so the backslashes stay doubled, the archive lookup misses, and the widget \
             draws as a white quad"
        );
    }

    let data = benilla_formats::wow_data_or_skip!();
    let chain = benilla_formats::open_chain(&data).expect("open chain");
    let missing: Vec<String> = refs
        .iter()
        .filter(|(_, _, path)| {
            !benilla_assets::sprite_candidates(path)
                .iter()
                .any(|c| chain.contains(c))
        })
        .map(|(file, tag, path)| format!("{file}: <{tag} file=\"{path}\">"))
        .collect();
    assert!(
        missing.is_empty(),
        "texture paths that resolve to nothing (each draws as a white quad): {missing:#?}"
    );
}

/// Every archive path a shipped Lua chunk names survives its own escaping: Lua silently drops the
/// backslash of an unknown escape, so `"Interface\P…"` arrives as `InterfaceP…` and the texture
/// never appears. The mirror of the XML check above, where a doubled separator is the bug.
#[test]
fn every_archive_path_a_shipped_lua_chunk_names_survives_its_own_escaping() {
    use benilla_ui::framexml::{Element, ScriptRef, TopLevel};

    // Archive roots a path literal starts with; other strings carry `\n` or `\124`, not paths.
    const ROOTS: [&str; 7] = [
        "interface",
        "textures",
        "world",
        "sound",
        "character",
        "item",
        "spells",
    ];

    /// Every quoted literal in one Lua chunk as raw source text, escapes uninterpreted; comments
    /// and long strings are skipped.
    fn literals(chunk: &str) -> Vec<(String, bool)> {
        let src: Vec<char> = chunk.chars().collect();
        let at = |i: usize, s: &str| src[i..].starts_with(&s.chars().collect::<Vec<_>>()[..]);
        let mut out = Vec::new();
        let mut i = 0;
        while i < src.len() {
            if at(i, "--") {
                i = if at(i + 2, "[[") {
                    src[i..]
                        .windows(2)
                        .position(|w| w == [']', ']'])
                        .map_or(src.len(), |k| i + k + 2)
                } else {
                    src[i..]
                        .iter()
                        .position(|&c| c == '\n')
                        .map_or(src.len(), |k| i + k + 1)
                };
                continue;
            }
            if at(i, "[[") {
                i = src[i + 2..]
                    .windows(2)
                    .position(|w| w == [']', ']'])
                    .map_or(src.len(), |k| i + 2 + k + 2);
                continue;
            }
            let quote = src[i];
            if quote != '"' && quote != '\'' {
                i += 1;
                continue;
            }
            let (mut raw, mut j, mut closed) = (String::new(), i + 1, false);
            while j < src.len() {
                let c = src[j];
                if c == '\\' && j + 1 < src.len() {
                    raw.push(c);
                    raw.push(src[j + 1]);
                    j += 2;
                    continue;
                }
                // A newline before the closing quote: not a literal (often an apostrophe).
                if c == '\n' {
                    break;
                }
                if c == quote {
                    closed = true;
                    break;
                }
                raw.push(c);
                j += 1;
            }
            if closed {
                // Joined by `..` on either side, or a `%` template: a fragment, not a whole path.
                let before = src[..i].iter().rposition(|c| !c.is_whitespace());
                let after = src[j + 1..].iter().position(|c| !c.is_whitespace());
                let joined = before.is_some_and(|k| k >= 1 && src[k] == '.' && src[k - 1] == '.')
                    || after.is_some_and(|k| {
                        src.get(j + 1 + k) == Some(&'.') && src.get(j + 2 + k) == Some(&'.')
                    });
                // A literal ending in a separator (a directory) or a dash (a family prefix,
                // `"…\\MageFire-" .. rank`) is joined elsewhere; no texture name ends in either.
                let fragment = raw.ends_with('\\') || raw.ends_with('-');
                let whole = !joined && !fragment && !raw.contains('%');
                out.push((raw, whole));
                i = j + 1;
            } else {
                i += 1;
            }
        }
        out
    }

    // Every Lua chunk (top-level `<Script>` blocks and handler bodies), through the parser: an
    // attribute value is also double-quoted, and a text scan cannot tell the two apart.
    fn bodies(el: &Element, out: &mut Vec<String>) {
        if !el.body.trim().is_empty() {
            out.push(el.body.clone());
        }
        for child in &el.children {
            bodies(child, out);
        }
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut paths: Vec<(String, String, bool)> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "xml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        let mut chunks = Vec::new();
        for item in &benilla_ui::framexml::parse(&text).expect("parses").items {
            match item {
                TopLevel::Script(ScriptRef::Inline { body, .. }) => chunks.push(body.clone()),
                TopLevel::Font(el) | TopLevel::Template(el) | TopLevel::Instance(el) => {
                    bodies(el, &mut chunks);
                }
                TopLevel::Script(ScriptRef::File(_)) | TopLevel::Include(_) => {}
            }
        }
        for (raw, whole) in chunks.iter().flat_map(|c| literals(c)) {
            let lower = raw.to_ascii_lowercase();
            if ROOTS.iter().any(|r| lower.starts_with(r)) && raw.contains('\\') {
                paths.push((file.to_string(), raw, whole));
            }
        }
    }
    // A floor, so the sweep cannot pass by matching nothing.
    assert!(
        paths.len() >= 3,
        "only {} archive paths swept out of the shipped Lua",
        paths.len()
    );

    // The shape half: with every `\\` pair collapsed, no backslash may remain.
    for (file, raw, _) in &paths {
        assert!(
            !raw.replace("\\\\", "").contains('\\'),
            "{file}: the Lua literal \"{raw}\" has SINGLE separators — Lua drops the backslash \
             from every one of them, so the path arrives with its folders run together, the \
             archive lookup misses, and the texture silently never appears. Double them."
        );
    }

    let data = benilla_formats::wow_data_or_skip!();
    let chain = benilla_formats::open_chain(&data).expect("open chain");
    let missing: Vec<String> = paths
        .iter()
        // Only whole paths under the two texture roots resolve through `sprite_candidates`.
        .filter(|(_, raw, whole)| {
            let lower = raw.to_ascii_lowercase();
            *whole && (lower.starts_with("interface") || lower.starts_with("textures"))
        })
        .filter(|(_, raw, _)| {
            let real = raw.replace("\\\\", "\\");
            !benilla_assets::sprite_candidates(&real)
                .iter()
                .any(|c| chain.contains(c))
        })
        .map(|(file, raw, _)| format!("{file}: \"{raw}\""))
        .collect();
    assert!(
        missing.is_empty(),
        "archive paths a shipped Lua chunk names that resolve to nothing: {missing:#?}"
    );
}

/// Every shipped `text=` answers against the real `GlobalStrings.lua`: `text=` is a lookup
/// (`FrameScript_GetText`, `0x703bf0`) that shows the raw attribute only on a miss (`0x778c07`).
/// A key-shaped value must resolve, or it shows its own key; a literal must not collide with a
/// global, or the lookup swaps in that string.
#[test]
fn every_shipped_text_attribute_answers_against_the_real_global_strings() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let s = benilla_ui::script::UiScript::new().expect("VM");
    s.run(&String::from_utf8_lossy(&src)).expect("runs clean");

    // The loader's key-shape predicate, restated because it is private.
    let key_shaped = |v: &str| {
        v.len() >= 2
            && v.chars().any(|c| c.is_ascii_uppercase())
            && v.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    };

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut keys = 0;
    for entry in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "xml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        for value in text
            .match_indices("text=\"")
            .filter_map(|(i, m)| text[i + m.len()..].split('"').next())
        {
            let resolved = s.lua().globals().get::<String>(value).ok();
            if key_shaped(value) {
                assert!(
                    resolved.is_some_and(|t| !t.is_empty()),
                    "{file}: text=\"{value}\" is shaped like a GlobalStrings key but the real \
                     GlobalStrings.lua has no such string — it would render as its own key name"
                );
                keys += 1;
            } else {
                assert!(
                    resolved.is_none(),
                    "{file}: the literal text=\"{value}\" collides with a real GlobalStrings key — \
                     the loader would silently show that string's value instead of these words"
                );
            }
        }
    }
    // A floor, so the sweep cannot pass by matching nothing.
    assert!(keys >= 1, "only {keys} key-shaped text= values swept");
}

/// No shipped script hands a GlobalStrings key to a text sink as its words: `SetText` cannot tell
/// a key from a word, so `SetText("DEAD")` shows the key where the word is its value, "Dead"
/// (`GlobalStrings.lua:898`). The `text=` half is the sweep above.
#[test]
fn no_shipped_script_sets_a_global_string_key_as_display_text() {
    /// `loader::is_global_string_key`'s predicate, restated because it is private.
    fn key_shaped(s: &str) -> bool {
        s.len() >= 2
            && s.chars().any(|c| c.is_ascii_uppercase())
            && s.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    }

    // The string literal after each text sink's open paren on one line.
    fn sink_literals(line: &str) -> Vec<&str> {
        let mut out = Vec::new();
        // No `:SetFormattedText(`: it is not a 1.12 verb.
        for sink in [":SetText(", ":SetButtonText("] {
            let mut rest = line;
            while let Some(at) = rest.find(sink) {
                rest = &rest[at + sink.len()..];
                let arg = rest.trim_start();
                if let Some(body) = arg.strip_prefix('"') {
                    if let Some(end) = body.find('"') {
                        out.push(&body[..end]);
                    }
                }
            }
        }
        out
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut offenders: Vec<String> = Vec::new();
    let mut swept = 0;
    for entry in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "xml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let mut in_comment = false;
        for (n, raw) in text.lines().enumerate() {
            // Strip whatever of this line sits inside an XML comment, carrying the state over.
            let mut code = String::new();
            let mut cur = raw;
            loop {
                if in_comment {
                    match cur.find("-->") {
                        Some(at) => {
                            in_comment = false;
                            cur = &cur[at + 3..];
                        }
                        None => break,
                    }
                } else {
                    match cur.find("<!--") {
                        Some(at) => {
                            code.push_str(&cur[..at]);
                            in_comment = true;
                            cur = &cur[at + 4..];
                        }
                        None => {
                            code.push_str(cur);
                            break;
                        }
                    }
                }
            }
            if code.trim_start().starts_with("--") {
                continue; // a Lua comment line
            }
            for lit in sink_literals(&code) {
                if key_shaped(lit) {
                    offenders.push(format!(
                        "{}:{}: SetText(\"{lit}\") — a GlobalStrings KEY, not the word",
                        path.file_name().unwrap().to_string_lossy(),
                        n + 1
                    ));
                }
            }
        }
        swept += 1;
    }
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
    // A floor, so the sweep cannot pass by finding nothing.
    assert!(swept >= 6, "only {swept} xml files swept — sweep broke");
}

/// Every `$parentTextureFrame` outranks its unit frame's status bars by frame level: level is the
/// only draw-key term above the layer, which would otherwise lift an ARTWORK bar fill over
/// BACKGROUND art. The stock frames spend a level on it (`TargetFrame.lua:32-34`) or nest the art
/// in anonymous frames (`PlayerFrame.xml:50-52`).
#[test]
fn every_texture_frame_outranks_its_status_bars() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::order::unpack;

    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, so a player always exists by then.
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
    // Every unit frame shown: a hidden frame emits no quads to read a level from.
    for unit in [
        "player",
        "target",
        "targettarget",
        "party1",
        "party2",
        "party3",
        "party4",
    ] {
        s.set_unit(
            unit,
            Some(benilla_ui::script::UnitState {
                exists: true,
                name: Some("Someone".into()),
                health: 60,
                max_health: 100,
                level: 60,
                power_type: 0,
                power: 60,
                max_power: 100,
                ..benilla_ui::script::UnitState::default()
            }),
        );
    }
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.fire_event("PARTY_MEMBERS_CHANGED", vec![]);
    // Target-of-target ships off (`SHOW_TARGET_OF_TARGET = "0"`, `UIOptionsFrame.lua:116`); it
    // holds the second of the reference's two named TextureFrames.
    s.run(r#"SHOW_TARGET_OF_TARGET = "1""#).unwrap();
    s.run("this = TargetofTargetFrame TargetofTarget_Update() this = nil")
        .unwrap();
    s.resolve();

    // owner frame name → its (strata, level), read off the packed draw key the renderer sorts by.
    let mut level_of: std::collections::BTreeMap<String, (u8, u16)> =
        std::collections::BTreeMap::new();
    for q in s.extract() {
        if let Some(name) = s.quad_owner_name(q.target) {
            let p = unpack(q.z);
            level_of.insert(name, (p.strata, p.level));
        }
    }

    let mut checked = 0;
    for (texture_frame, &(tf_strata, tf_level)) in &level_of {
        let Some(base) = texture_frame.strip_suffix("TextureFrame") else {
            continue;
        };
        for suffix in ["HealthBar", "ManaBar"] {
            let bar = format!("{base}{suffix}");
            let Some(&(bar_strata, bar_level)) = level_of.get(&bar) else {
                continue;
            };
            assert_eq!(
                bar_strata, tf_strata,
                "{bar} and {texture_frame} must share a strata for the level to decide"
            );
            assert!(
                tf_level > bar_level,
                "{texture_frame} (level {tf_level}) must outrank {bar} (level {bar_level}): \
                 tied, the draw layer lifts the bar's ARTWORK fill over the frame's BACKGROUND art"
            );
            checked += 1;
        }
    }
    // The reference names two TextureFrames, `TargetFrameTextureFrame` and
    // `TargetofTargetTextureFrame` (TargetFrame.xml); the player, pet and party frames nest their
    // art in anonymous frames instead: two families with two bars each.
    assert!(
        checked >= 4,
        "only {checked} texture-frame/bar pairs checked — the name sweep found nothing"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The boot phase is inert: `Fonts.xml`, the only file loaded at `Startup`, is a pure registry and
/// materializes no frames.
#[test]
fn the_boot_phase_materializes_no_frames() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // Manifest entry 0, off the chain; `load_ui` fails on any loader error.
    let frames = super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    assert_eq!(
        frames, 0,
        "the boot-phase load materialized {frames} frame(s) — the login screen is meant to carry none"
    );
}

/// The boot-time font registry alone declares every (font, height, outline) the whole manifest
/// does; a failure is a font object outside `Fonts.xml` with a new height or an outline.
#[test]
fn the_font_registry_alone_covers_the_whole_bake_plan() {
    benilla_formats::wow_data_or_skip!();
    let plan = |whole: bool| -> std::collections::BTreeSet<(String, String, String)> {
        let mut s = benilla_ui::script::UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        if whole {
            // The in-game UI loads on world entry, so a player always exists by then.
            s.set_unit(
                "player",
                Some(benilla_ui::script::UnitState {
                    exists: true,
                    name: Some("Probefour".into()),
                    level: 60,
                    ..Default::default()
                }),
            );
            let _ = super::load_default_ui(&s);
        } else {
            let _ = super::load_font_registry(&s);
        }
        s.font_objects()
            .iter()
            .map(|f| {
                (
                    f.font.clone().unwrap_or_default().to_ascii_lowercase(),
                    format!("{:?}", f.height),
                    format!("{:?}", f.outline),
                )
            })
            .collect()
    };
    let whole = plan(true);
    let registry_only = plan(false);
    let missing: Vec<_> = whole.difference(&registry_only).collect();
    assert!(
        missing.is_empty(),
        "these (font, height, outline) combinations exist in the full manifest but NOT in the \
         boot-time font registry, so the atlas would never bake them: {missing:#?}"
    );
    // Never let this pass by finding nothing on both sides.
    assert!(
        registry_only.len() >= 19,
        "only {} combinations swept — the registry sweep broke",
        registry_only.len()
    );
}

/// The shipped UI takes `VARIABLES_LOADED`, which the saved-variables load fires at every launch
/// before any window has shown, without a script error.
#[test]
fn the_shipped_ui_takes_variables_loaded_without_a_script_error() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, so a player always exists by then.
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
    assert!(failures.is_empty(), "loader errors: {failures:?}");
    let _ = s.errors(); // drain the load's own errors
    s.fire_event("VARIABLES_LOADED", vec![]);
    assert!(
        s.errors().is_empty(),
        "VARIABLES_LOADED script errors: {:?}",
        s.errors()
    );
}

/// Every `<Font name=…>` the manifest declares is a `Font` global answering the FontInstance
/// getters, in manifest order: `publish_global` never overwrites, so a same-named frame loaded
/// first would keep a font unpublished.
#[test]
fn every_shipped_font_object_is_published_as_a_lua_global() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, so a player always exists by then.
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
    assert!(failures.is_empty(), "loader errors: {failures:?}");

    // The names come from the manifest's own entries, chain files included.
    let mut names: Vec<String> = Vec::new();
    for entry in &super::addons::Addon::builtin().toc.files {
        if !entry.to_ascii_lowercase().ends_with(".xml") {
            continue;
        }
        let Some(bytes) = super::test_ui::read(&entry.replace('\\', "/")) else {
            continue;
        };
        let text = benilla_ui::source::decode(&bytes);
        for chunk in text.split("<Font ").skip(1) {
            if let Some(rest) = chunk.split_once("name=\"") {
                if let Some((name, _)) = rest.1.split_once('"') {
                    names.push(name.to_string());
                }
            }
        }
    }
    names.sort();
    names.dedup();
    assert!(
        names.len() >= 50,
        "only {} <Font name=> declarations found — the sweep broke",
        names.len()
    );

    let mut unpublished: Vec<&str> = Vec::new();
    for name in &names {
        match s.eval::<String>(&format!("return {name}:GetObjectType()")) {
            Ok(t) if t == "Font" => {}
            _ => unpublished.push(name),
        }
    }
    assert!(
        unpublished.is_empty(),
        "declared <Font name=> that is not a Font global: {unpublished:?}"
    );

    // The four font objects addons use most carry a face and a size, not just a name.
    for name in [
        "GameFontNormal",
        "GameTooltipText",
        "GameFontHighlightSmall",
        "GameTooltipHeaderText",
    ] {
        let (face, height) = s
            .eval::<(String, f32)>(&format!("return {name}:GetFont()"))
            .unwrap_or_else(|e| panic!("{name}:GetFont() — {e}"));
        assert!(
            face.to_ascii_uppercase().ends_with(".TTF"),
            "{name}: {face}"
        );
        assert!(height > 0.0, "{name}: height {height}");
    }
}

/// `TargetBuffButtonTemplate` (`TargetFrame.xml`, 21×21) and `MainMenuBarMicroButton`
/// (`MainMenuBarMicroButtons.xml`, 29×58) confer their size, regions and scripts, read back as a
/// consumer gets them: an unresolved `inherits=` fails silently.
#[test]
fn the_inheritable_reference_templates_confer_their_shape() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, so a player always exists by then.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    assert!(super::load_default_ui(&s).is_empty());

    s.run(
        r#"
        BuffProbe = CreateFrame("Button", "BuffProbe", UIParent, "TargetBuffButtonTemplate")
        MicroProbe = CreateFrame("Button", "MicroProbe", UIParent, "MainMenuBarMicroButton")
        "#,
    )
    .unwrap();
    s.resolve();

    assert_eq!(
        s.eval::<(f64, f64)>("return BuffProbe:GetWidth(), BuffProbe:GetHeight()")
            .unwrap(),
        (21.0, 21.0),
        "the buff button carries the reference's 21x21"
    );
    assert!(
        s.eval::<bool>("return BuffProbeIcon ~= nil").unwrap(),
        "$parentIcon is the region a consumer getglobals to set the texture"
    );
    assert!(
        s.eval::<bool>(r#"return BuffProbe:GetScript("OnEnter") ~= nil"#)
            .unwrap(),
        "the tooltip script comes with the template"
    );

    assert_eq!(
        s.eval::<(f64, f64)>("return MicroProbe:GetWidth(), MicroProbe:GetHeight()")
            .unwrap(),
        (29.0, 58.0)
    );
    assert!(s
        .eval::<bool>(r#"return MicroProbe:GetScript("OnEnter") ~= nil"#)
        .unwrap());
    assert!(s.errors().is_empty(), "no script errors: {:?}", s.errors());
}

/// The stock inspect-cursor pair, `CursorUpdate` and `CursorOnUpdate` (`UIParent.lua:1805-1817`),
/// runs clean on the arms a frame without an item takes.
#[test]
fn the_inspect_cursor_pair_takes_both_arms() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, so a player always exists by then.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    assert!(super::load_default_ui(&s).is_empty());

    // Both exist as functions, the shape a caller checks before hooking.
    assert_eq!(
        s.eval::<(String, String)>("return type(CursorUpdate), type(CursorOnUpdate)")
            .unwrap(),
        ("function".into(), "function".into())
    );

    // A frame without `this.hasItem` takes the `ResetCursor` arm.
    s.run(
        r#"
        CursorProbe = CreateFrame("Frame", "CursorProbe", UIParent)
        this = CursorProbe
        CursorUpdate()
        this = nil
        "#,
    )
    .unwrap();
    assert!(
        s.errors().is_empty(),
        "the ResetCursor arm runs clean: {:?}",
        s.errors()
    );

    // `CursorOnUpdate` does nothing unless the frame owns the tooltip.
    s.run("this = CursorProbe CursorOnUpdate() this = nil")
        .unwrap();
    assert!(
        s.errors().is_empty(),
        "the unowned-tooltip gate short-circuits: {:?}",
        s.errors()
    );
}

/// Nothing of the interface stays visible through a cinematic, swept over what is visible rather
/// than a list. The one survivor is `CinematicFrame`, which the reference declares with no parent
/// and `SetFullScreenFrame` shows as it hides `UIParent` (`UIParent.lua:852`).
#[test]
fn a_cinematic_leaves_nothing_of_the_interface_on_screen() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, so a player always exists by then.
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

    // Our files' frames plus the stock HUD, which is most of what the cascade hides.
    let mut names = shipped_frame_names();
    names.extend(
        [
            "MainMenuBar",
            "ChatFrame1",
            "PlayerFrame",
            "MinimapCluster",
            "BuffFrame",
            "MainMenuBarBackpackButton",
            "CharacterMicroButton",
            "UIErrorsFrame",
            // The frame being shown, the one expected survivor.
            "CinematicFrame",
        ]
        .into_iter()
        .map(String::from),
    );
    let visible = |s: &benilla_ui::script::UiScript, n: &str| -> bool {
        s.eval::<i64>(&format!(
            "local f = getglobal(\"{n}\") \
             if not f or not f.IsVisible then return 0 end \
             return f:IsVisible() and 1 or 0"
        ))
        .unwrap_or(0)
            == 1
    };

    // The sweep is worth something only if there was something to hide.
    let before = names.iter().filter(|n| visible(&s, n)).count();
    assert!(
        // A floor over the named HUD, not over our files' census.
        before >= 6,
        "only {before} frames visible before the cinematic — the sweep found no interface to \
         hide, so it would pass no matter what the cascade did"
    );

    s.set_in_cinematic(true);
    s.fire_event("CINEMATIC_START", vec![]);
    s.resolve();

    let mut after: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|n| visible(&s, n))
        .collect();
    after.sort_unstable();
    assert_eq!(
        after,
        ["CinematicFrame"],
        "something is drawing over the fly-by (see this test's header for why exactly these \
         two are allowed to survive)"
    );

    // …and the player gets it all back.
    s.set_in_cinematic(false);
    s.fire_event("CINEMATIC_STOP", vec![]);
    s.resolve();
    let restored = names.iter().filter(|n| visible(&s, n)).count();
    assert_eq!(
        restored, before,
        "the interface comes back exactly as it was"
    );
}

/// Every `parent=` the shipped tree declares attaches, read off the loaded tree: a parent name that
/// names nothing only warns, and the loader falls back to the enclosing element.
#[test]
fn every_declared_parent_really_attaches() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, so a player always exists by then.
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

    let declared = shipped_frame_parents();
    assert!(
        // A sanity floor for the scan, not a census.
        declared.len() >= 2,
        "only {} parent declarations found — the scan broke",
        declared.len()
    );
    let mut wrong: Vec<String> = Vec::new();
    for (child, parent) in &declared {
        let got = s
            .eval::<String>(&format!(
                "local f = getglobal(\"{child}\") \
                 if not f or not f.GetParent then return \"<not a frame>\" end \
                 local p = f:GetParent() \
                 return p and (p:GetName() or \"<anonymous>\") or \"<none>\""
            ))
            .unwrap_or_else(|_| "<error>".to_string());
        if got != *parent {
            wrong.push(format!(
                "  {child}: declared parent={parent}, attached to {got}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} frame(s) did not attach to the parent they declare:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// Every named, non-virtual frame in the shipped tree that declares a `parent=`, with that parent.
fn shipped_frame_parents() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut out: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e == "xml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        for chunk in text.split('<').skip(1) {
            let head = &chunk[..chunk.find('>').unwrap_or(chunk.len())];
            if head.contains("virtual=\"true\"") {
                continue;
            }
            let attr = |key: &str| -> Option<String> {
                let i = head.find(key)?;
                let rest = &head[i + key.len()..];
                let j = rest.find('"')?;
                Some(rest[..j].to_string())
            };
            let (Some(name), Some(parent)) = (attr("name=\""), attr("parent=\"")) else {
                continue;
            };
            if !name.contains('$') && !parent.contains('$') {
                out.push((name, parent));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every named, non-virtual frame the shipped tree declares.
pub(super) fn shipped_frame_names() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e == "xml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        for chunk in text.split('<').skip(1) {
            let head = &chunk[..chunk.find('>').unwrap_or(chunk.len())];
            if head.contains("virtual=\"true\"") {
                continue;
            }
            let Some(i) = head.find("name=\"") else {
                continue;
            };
            let rest = &head[i + 6..];
            let Some(j) = rest.find('"') else { continue };
            let n = &rest[..j];
            // `$parent`-templated names are not globals; nothing can look them up by name.
            if !n.is_empty() && !n.contains('$') {
                names.push(n.to_string());
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// The macro icon picker opens off the shipped manifest, not a harness list: the on-demand macro
/// window's `MacroPopupScrollFrame` inherits a template only the manifest's
/// `ClassTrainerFrameTemplates.xml` row brings.
#[test]
fn the_shipped_manifest_opens_the_macro_icon_picker() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefive".into()),
            level: 60,
            ..Default::default()
        }),
    );
    // One saved macro, so "Change Name/Icon" has something selected.
    s.set_macros(benilla_ui::script::MacroState {
        account: vec![benilla_ui::script::MacroView {
            name: "die".into(),
            texture: Some(r"Interface\Icons\Ability_Ambush".into()),
            body: ".die".into(),
            ..Default::default()
        }],
        character: Vec::new(),
    });
    s.set_macro_icons(vec![
        r"Interface\Icons\Ability_Ambush".into(),
        r"Interface\Icons\Ability_Backstab".into(),
    ]);
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    s.resolve();
    let _ = s.errors();

    // A LoadOnDemand addon, which `ShowMacroFrame` loads through the stock `MacroFrame_LoadUI`.
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_MacroUI");
    s.run("ShowMacroFrame()").unwrap();
    s.run("MacroButton1:Click()").unwrap();
    s.run("MacroEditButton:Click()").unwrap();
    assert!(
        s.errors().is_empty(),
        "opening the icon picker off the shipped manifest must not raise: {:?}",
        s.errors()
    );
    assert!(
        s.eval::<bool>("return MacroPopupFrame:IsShown() and true or false")
            .unwrap(),
        "the picker is up"
    );
}

/// The stock Interface Options window loads hidden as its own frame, and ours is still the one the
/// ESC menu opens. It loads before `MultiActionBars.lua:10` sets fields on five of its check-button
/// rows, as in `FrameXML.toc` (lines 21 and 39).
#[test]
fn the_stock_interface_options_window_loads_hidden_and_ours_is_still_the_players() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefive".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");

    // The stock frame, by the children only it declares.
    for name in [
        "UIOptionsFrame",
        "UIOptionsFrameTitle",
        "UIOptionsFrameTab1",
        "UIOptionsFrameTab2",
        "UIOptionsFrameOkay",
        "UIOptionsFrameCancel",
        "UIOptionsFrameDefaults",
        "UIOptionsFrameResetTutorials",
        "UIOptionsFrameCheckButton1",
        "UIOptionsFrameCheckButton69",
        "UIOptionsFrameSlider1",
        "UIOptionsFrameSlider4",
        "UIOptionsFrameClickCameraDropDown",
        "UIOptionsFrameCameraDropDown",
        "UIOptionsFrameTargetofTargetDropDown",
        "UIOptionsFrameCombatTextDropDown",
        "BasicOptions",
        "BasicOptionsGeneral",
        "BasicOptionsDisplay",
        "BasicOptionsCamera",
        "BasicOptionsHelp",
        "AdvancedOptions",
        "AdvancedOptionsActionBars",
        "AdvancedOptionsChat",
        "AdvancedOptionsRaid",
        "AdvancedOptionsCombatText",
    ] {
        assert!(
            s.eval::<bool>(&format!("return getglobal({name:?}) ~= nil"))
                .unwrap(),
            "{name} — pfUI's options-interface skin walks every one of these"
        );
    }
    assert!(
        s.eval::<bool>("return UIOptionsFrame ~= BenillaOptionsFrame")
            .unwrap(),
        "the alias is gone: the stock frame is its own frame, not ours under a second name"
    );

    // …and hidden, by its own file's attribute.
    assert!(
        !s.eval::<bool>("return UIOptionsFrame:IsShown()").unwrap(),
        "the stock window must never be on screen"
    );

    // The load-order proof: the five rows `MultiActionBars.lua:10` sets fields on.
    let rows: Vec<String> = s
        .eval::<Vec<String>>(
            "local out = {} \
             for _, k in ipairs({ \"SHOW_MULTIBAR1_TEXT\", \"SHOW_MULTIBAR2_TEXT\", \
                 \"SHOW_MULTIBAR3_TEXT\", \"SHOW_MULTIBAR4_TEXT\", \"ALWAYS_SHOW_MULTIBARS_TEXT\" }) do \
                 local row = UIOptionsFrameCheckButtons[k] \
                 if row and row.func and row.setFunc then table.insert(out, k) end \
             end \
             return out",
        )
        .expect("UIOptionsFrameCheckButtons is a table with rows");
    assert_eq!(
        rows.len(),
        5,
        "MultiActionBars.lua:10 writes five rows into UIOptionsFrameCheckButtons at ITS load, so \
         the options window's manifest row must sit above the bars' — got {rows:?}"
    );

    // Ours is still the window the ESC menu opens.
    assert_eq!(
        s.eval::<String>(
            "return GameMenuButtonOptions:GetScript(\"OnClick\") and \"bound\" or \"\""
        )
        .unwrap(),
        "bound"
    );
    s.run("GameMenuButtonOptions:Click()").unwrap();
    assert!(
        s.eval::<bool>("return BenillaOptionsFrame:IsShown()")
            .unwrap(),
        "the player's Options button opens OUR window"
    );
    assert!(
        !s.eval::<bool>("return UIOptionsFrame:IsShown()").unwrap(),
        "…and never the stock one"
    );
}

/// The stock Sound Options window loads hidden (`SoundOptionsFrame.xml:18`) as its own frame; ours
/// loads later, so an alias would replace the real frame. `SoundOptionsFrame_Load()` runs clean
/// with every CVar its tables name, and the ESC menu still opens ours.
#[test]
fn the_stock_sound_options_window_loads_hidden_and_the_alias_is_gone() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefive".into()),
            level: 60,
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");

    // The stock frame, by the children only it declares; the stock file has no CheckButton3.
    for name in [
        "SoundOptionsFrame",
        "SoundOptionsFrameHeader",
        "SoundOptionsFrameOkay",
        "SoundOptionsFrameCancel",
        "SoundOptionsFrameDefaults",
        "SoundOptionsFrameCheckButton1",
        "SoundOptionsFrameCheckButton2",
        "SoundOptionsFrameCheckButton4",
        "SoundOptionsFrameCheckButton8",
        "SoundOptionsFrameSlider1",
        "SoundOptionsFrameSlider4",
    ] {
        assert!(
            s.eval::<bool>(&format!("return getglobal({name:?}) ~= nil"))
                .unwrap(),
            "{name} — pfUI's options-sound skin walks every one of these"
        );
    }
    assert!(
        s.eval::<bool>("return SoundOptionsFrame ~= BenillaOptionsFrame")
            .unwrap(),
        "the alias is gone — and because OUR file loads later, an alias would have CLOBBERED the \
         real frame rather than merely shadowed it"
    );
    assert!(
        s.eval::<bool>("return SoundOptionsFrameCheckButton3 == nil")
            .unwrap(),
        "the stock file declares 1,2,4..8 — a CheckButton3 here means this is not that file"
    );

    // …and hidden, by its own file's attribute.
    assert!(
        !s.eval::<bool>("return SoundOptionsFrame:IsShown()")
            .unwrap(),
        "the stock Sound window must never be on screen"
    );

    // `_Load` runs clean, and every CVar its two tables name answers.
    s.run("this = SoundOptionsFrameOkay SoundOptionsFrame_Load()")
        .expect("_Load runs to completion");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    let unbacked: Vec<String> = s
        .eval(
            "local out = {} \
             for _, v in ipairs(SoundOptionsFrameSliders) do \
                 if GetCVar(v.cvar) == nil then table.insert(out, v.cvar) end \
             end \
             for _, v in ipairs(SoundOptionsFrameCheckButtons) do \
                 if v.cvar and GetCVar(v.cvar) == nil then table.insert(out, v.cvar) end \
             end \
             return out",
        )
        .expect("read the Sound window's own two tables");
    assert!(
        unbacked.is_empty(),
        "these Sound-window CVars answer nil: {unbacked:?} — a SLIDER among them is a live raise"
    );

    // Ours is still the window the ESC menu opens.
    s.run("GameMenuButtonOptions:Click()").unwrap();
    assert!(
        s.eval::<bool>("return BenillaOptionsFrame:IsShown()")
            .unwrap(),
        "the player's Options button opens OUR window"
    );
    assert!(
        !s.eval::<bool>("return SoundOptionsFrame:IsShown()")
            .unwrap(),
        "…and never the stock Sound one"
    );

    // `IsOptionFrameOpen` names the stock options windows, all hidden; it sees ours through the
    // wrapper `GameMenuFrame.xml` installs.
    assert!(
        s.eval::<bool>("return IsOptionFrameOpen() and true or false")
            .unwrap(),
        "UIParent.lua:997 must still see an open options window with the alias retired"
    );
}

/// The stock Video Options window loads hidden (`OptionsFrame.xml:5`) and owns the `OptionsFrame`
/// name; ours is `BenillaOptionsFrame`, which the ESC menu opens and the stock "options window
/// open?" consumers see only through the `GameMenuFrame.xml` wrappers.
#[test]
fn the_stock_video_options_window_loads_hidden_and_owns_its_own_name() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // 1600x900 is not one of the monitor's modes, as a player's windowed size usually is not;
    // `GetCurrentResolution` must still index a row that exists.
    s.set_screen_resolutions(
        vec![
            benilla_ui::script::ScreenResolution {
                width: 1280,
                height: 720,
            },
            benilla_ui::script::ScreenResolution {
                width: 1920,
                height: 1080,
            },
        ],
        Some(benilla_ui::script::ScreenResolution {
            width: 1600,
            height: 900,
        }),
    );
    s.set_video_caps(benilla_ui::script::VideoCaps {
        anisotropic: true,
        pixel_shaders: true,
        vertex_shaders: true,
        trilinear: true,
        triple_buffering: false,
        max_anisotropy: 16,
        hardware_cursor: true,
    });
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefive".into()),
            level: 60,
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");

    // The stock frame, by the children only it declares.
    for name in [
        "OptionsFrameHeader",
        "OptionsFrameDisplay",
        "OptionsFrameWorldAppearance",
        "OptionsFrameBrightness",
        "OptionsFramePixelShaders",
        "OptionsFrameMiscellaneous",
        "OptionsFrameResolutionDropDown",
        "OptionsFrameRefreshDropDown",
        "OptionsFrameMultiSampleDropDown",
        "OptionsFrameOkay",
        "OptionsFrameCancel",
        "OptionsFrameDefaults",
        "OptionsFrameSlider1",
        "OptionsFrameSlider9",
        "OptionsFrameCheckButton1",
        "OptionsFrameCheckButton18",
    ] {
        assert!(
            s.eval::<bool>(&format!("return getglobal({name:?}) ~= nil"))
                .unwrap(),
            "{name} — pfUI's options-video skin walks every one of these"
        );
    }
    assert!(
        s.eval::<bool>("return OptionsFrame ~= BenillaOptionsFrame")
            .unwrap(),
        "the video window's name is the video window's; ours answers to its own"
    );
    assert!(
        s.eval::<bool>("return OptionsFrameCancel:GetParent() == OptionsFrame")
            .unwrap(),
        "`OptionsFrameCancel` is the stock window's own button now — the alias that pointed it at \
         our Close button would have clobbered it, ours loading later"
    );

    // …and hidden, by its own file's attribute.
    assert!(
        !s.eval::<bool>("return OptionsFrame:IsShown()").unwrap(),
        "the stock Video window must never be on screen"
    );

    // The stock nine slider rows, and our Graphics page standing on them.
    assert_eq!(
        s.eval::<f64>("return table.getn(OptionsFrameSliders)")
            .unwrap() as i32,
        9,
        "our three-row overwrite is gone — this is the reference's table"
    );
    assert_eq!(
        s.eval::<String>("return OptionsFrameSliders[3].func")
            .unwrap(),
        "WorldDetail",
        "pfUI's hdgraphic writes index 3 and must still land on Environment Detail"
    );
    let (lo, hi): (f64, f64) = s
        .eval(
            "local sl = BenillaOptionsFrameContainerBodyGraphicsRowFarclipControlSlider \
             return sl:GetMinMaxValues()",
        )
        .expect("our Terrain Distance row's bounds");
    assert_eq!(
        (lo, hi),
        (
            s.eval::<f64>("return OptionsFrameSliders[2].minValue")
                .unwrap(),
            s.eval::<f64>("return OptionsFrameSliders[2].maxValue")
                .unwrap()
        ),
        "our row is built from the REFERENCE's row 2, not from a transcription of it"
    );

    // The display verbs, at the shapes their consumers read.
    let res: Vec<String> = s
        .eval("return { GetScreenResolutions() }")
        .expect("the resolution list");
    assert_eq!(
        res,
        ["1280x720", "1600x900", "1920x1080"],
        "`WxH`, ascending by pixel AREA (the reference's own key), with the live windowed size \
         folded in — 1600×900 is not a monitor mode and is exactly the case CT_Viewport needs"
    );
    // `CT_Viewport.lua:105-107`, verbatim: index the varargs, then parse `WxH`.
    let (w, h): (f64, f64) = s
        .eval(
            "local function pick(...) local r = arg[GetCurrentResolution()] \
             local _, _, x, y = string.find(r, \"(%d+)x(%d+)\") return tonumber(x), tonumber(y) end \
             return pick(GetScreenResolutions())",
        )
        .expect("CT_Viewport's own read");
    assert_eq!(
        (w, h),
        (1600.0, 900.0),
        "CT_Viewport must find the live size"
    );
    // No return values, the reference's "nothing to offer" (`0x48c136 xor eax,eax`); the stock
    // lone-`0` branch of `OptionsFrame_GetRefreshRates` needs the OS to report 0 Hz for every mode.
    let rates: Vec<f64> = s.eval("return { GetRefreshRates() }").expect("the rates");
    assert!(
        rates.is_empty(),
        "the reference answers ZERO values, never a lone 0: {rates:?}"
    );
    assert!(
        s.eval::<bool>("return OptionsFrameRefreshDropDownButton:IsEnabled() == 1")
            .unwrap(),
        "…so the dropdown is left empty but ENABLED — the greying branch is the driver-quirk one, \
         and inventing a 0 to reach it would be inventing a value the binary never produces \
         (`IsEnabled` answers a NUMBER — `0x7800b0`)"
    );
    // The optional index argument is tolerated.
    assert!(
        s.eval::<bool>("return table.getn({ GetRefreshRates(2) }) == 0")
            .unwrap(),
        "the argument is optional AND ignored here; it must not raise"
    );
    // The caps, at the types `OptionsFrame_Load` reads them as.
    let caps: Vec<String> = s
        .eval(
            "local out = {} \
             local t = { GetVideoCaps() } \
             for i = 1, 7 do table.insert(out, tostring(t[i])) end \
             return out",
        )
        .expect("the seven caps");
    assert_eq!(
        caps,
        ["1", "1", "1", "1", "0", "16", "1"],
        "THREE shapes: four flags as 1/nil, slot 5 an unconditional NUMBER, slot 6 raw"
    );
    // Slot 5 is never nil and `0` is truthy, so the stock `not hasTripleBuffering` tests
    // (`OptionsFrame.lua:87`, `:386`) are dead in the reference too; only `== 1` (`:159`) decides.
    assert!(
        s.eval::<bool>(
            "local a, b, c, d, hasTriple = GetVideoCaps() \
             return (not (not hasTriple)) and hasTriple ~= 1"
        )
        .unwrap(),
        "`not hasTripleBuffering` must stay FALSE (the dead clause) while `== 1` is also false"
    );

    // `SetScreenResolution`: no argument picks entry one and a fraction truncates, as in the
    // reference. Deviation: an index past the end clamps to the last entry, because the reference
    // reads one past the end of its list.
    assert!(
        s.eval::<bool>("SetScreenResolution() return GetCVar(\"gxResolution\") == \"1280x720\"")
            .unwrap(),
        "the tolerant family: a missing argument selects entry ONE, it does not raise"
    );
    assert!(
        s.eval::<bool>("SetScreenResolution(2.7) return GetCVar(\"gxResolution\") == \"1600x900\"")
            .unwrap(),
        "truncated toward zero — 2.7 is entry 2"
    );
    assert!(
        s.eval::<bool>("SetScreenResolution(99) return GetCVar(\"gxResolution\") == \"1920x1080\"")
            .unwrap(),
        "out of range CLAMPS to the last entry — the reference reads `list[count]`, one past the \
         end, and reproducing an out-of-bounds read to apply an uninitialised size is not fidelity"
    );
    // The CVar now says 1920x1080 and the window 1600x900: `GetCurrentResolution` reads the window.
    assert_eq!(
        s.eval::<f64>("return GetCurrentResolution()").unwrap(),
        2.0,
        "the live size is still entry 2; a pick stages `gxResolution` and takes effect on apply"
    );

    // Ours is still the window the ESC menu opens, in the centre slot.
    s.run("GameMenuButtonOptions:Click()").unwrap();
    assert!(
        s.eval::<bool>("return BenillaOptionsFrame:IsShown()")
            .unwrap(),
        "the player's Options button opens OUR window"
    );
    assert!(
        !s.eval::<bool>("return OptionsFrame:IsShown()").unwrap(),
        "…and never the stock Video one"
    );
    assert_eq!(
        s.eval::<String>("return GetCenterFrame():GetName()")
            .unwrap(),
        "BenillaOptionsFrame",
        "the restated `UIPanelWindows` row — without it `ShowUIPanel` places nothing"
    );

    // The three stock consumers, which hear about our window only through the wrappers.
    assert!(
        s.eval::<bool>("return IsOptionFrameOpen() and true or false")
            .unwrap(),
        "UIParent.lua:996 must see an open options window"
    );
    assert!(
        s.eval::<bool>("return MainMenuMicroButton:GetButtonState() == \"PUSHED\"")
            .unwrap(),
        "MainMenuBarMicroButtons.lua:47-58 must show the micro button pushed"
    );
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        !s.eval::<bool>("return BenillaOptionsFrame:IsShown()")
            .unwrap(),
        "the ESC ladder's options rung must close OUR window, in the reference's own order"
    );
    assert!(
        !s.eval::<bool>("return GameMenuFrame:IsShown()").unwrap(),
        "…and take that press, rather than falling through to the game menu"
    );
    assert!(
        !s.eval::<bool>("return IsOptionFrameOpen() and true or false")
            .unwrap(),
        "with ours closed and all three stock windows hidden, the honest answer is no"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `GetGamma`/`SetGamma` speak the stock slider's unit: `0x4891d0` is FSUBR, so `GetGamma()` is
/// `1.0 − gamma` and `SetGamma(v)` writes `1.0 − v`. The reference has no clamp (unlike `baseMip`'s
/// validating callback `0x689090`), so `SetGamma(5)` stores `"-4.000000"`; benilla clamps only at
/// the render consumer.
#[test]
fn the_display_brightness_pair_speaks_the_reference_slider_unit() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");

    // The registered `gamma = "1.0"` reads as 0.0, the centre of the stock slider.
    assert_eq!(
        s.eval::<f64>("return GetGamma()").unwrap(),
        0.0,
        "a fresh client sits at the centre of the stock slider's [-0.5, 0.5]"
    );
    // Both ends, in the CVar's own text: `SStrPrintf(buf, 0x10, "%f", 1.0 - v)`.
    for (slider, cvar) in [
        (0.5, "0.500000"),
        (-0.5, "1.500000"),
        (0.0, "1.000000"),
        // No clamp: the reference accepts this and writes a negative gamma.
        (5.0, "-4.000000"),
    ] {
        s.eval::<()>(&format!("SetGamma({slider})")).unwrap();
        assert_eq!(
            s.eval::<String>("return GetCVar(\"gamma\")").unwrap(),
            cvar,
            "SetGamma({slider}) writes 1 - v with six decimals"
        );
        assert_eq!(
            s.eval::<f64>("return GetGamma()").unwrap(),
            slider,
            "…and the getter is its exact inverse"
        );
    }
    // No return values (`eax = 0` at every `ret`), counted by assignment: this VM has no `select`.
    assert_eq!(
        s.eval::<i64>(
            "local a, b = SetGamma(0) \
             if a ~= nil or b ~= nil then return 1 end \
             return 0"
        )
        .unwrap(),
        0,
        "SetGamma pushes nothing"
    );
    // …and it requires its argument (`0x4891fe`, raising through `0x6f4940`, which never returns).
    let err = s.eval::<()>("SetGamma()").unwrap_err().to_string();
    assert!(
        err.contains("Usage: SetGamma(value)"),
        "the reference's own usage string, verbatim: {err}"
    );

    // Our Graphics row drives the pair, as the stock slider does, with the bounds of the stock
    // `OptionsFrameSliders[6]`.
    s.eval::<()>("SetGamma(0)").unwrap();
    let row = "BenillaOptionsFrameContainerBodyGraphicsRowBrightness";
    let bounds = s
        .eval::<Vec<f64>>(&format!(
            "local r = getglobal({row:?}) \
             local sl = getglobal({row:?} .. \"ControlSlider\") \
             local lo, hi = sl:GetMinMaxValues() \
             return {{ lo, hi, sl:GetValueStep(), r.numeric }}"
        ))
        .unwrap();
    // The step is an f32 on the widget (0.100000001…); the bounds and the numeric flag are exact.
    assert_eq!(
        (bounds[0], bounds[1], bounds[3]),
        (-0.5, 0.5, 1.0),
        "the reference's slider-6 bounds, on a numeric api row"
    );
    assert!(
        (bounds[2] - 0.1).abs() < 1e-6,
        "…and its step: {}",
        bounds[2]
    );
    // The readout is the thumb's share of the groove (default 50%), not the stored offset; the
    // page's refresh writes it, so the window must be up.
    s.eval::<()>(
        "ShowUIPanel(BenillaOptionsFrame) \
         BenillaOptionsFrameCategoryListRowGraphics:Click()",
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>(&format!(
            "return getglobal({row:?} .. \"ControlValue\"):GetText()"
        ))
        .unwrap(),
        "50%",
    );
}

/// The video window's slider walk finds none of these ten names: `OptionsFrame.lua:110` (`_Load`)
/// and `:208-209` (`_Save`) call `getglobal("Get"..value.func)` and its `Set` twin, falling back to
/// the CVar on a miss; eight of the eighteen composed names are real. The reference's `GetFarclip`
/// (`0x488f00`) and `SetFarclip` (`0x488f30`) miss the row's `"farclip"` only because `getglobal`
/// is case-sensitive.
#[test]
fn the_video_windows_ten_composed_names_stay_nil() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");

    for name in [
        "Getuiscale",
        "Setuiscale",
        "Getfarclip",
        "Setfarclip",
        "Getanisotropic",
        "Setanisotropic",
        "GetspellEffectLevel",
        "SetspellEffectLevel",
        "GetweatherDensity",
        "SetweatherDensity",
    ] {
        assert!(
            s.eval::<bool>(&format!("return getglobal({name:?}) == nil"))
                .unwrap(),
            "{name} must not exist — `OptionsFrame_Load` branches on it and would stop using the \
             CVar path for that slider"
        );
    }
    // Case-sensitivity, against a name that is defined (Environment Detail's getter).
    assert!(
        s.eval::<bool>("return getglobal(\"GetWorldDetail\") ~= nil")
            .unwrap(),
        "the control: this one is real"
    );
    assert!(
        s.eval::<bool>("return getglobal(\"Getworlddetail\") == nil")
            .unwrap(),
        "`getglobal` must stay case-sensitive — case-folding it would resolve `Getfarclip` to the \
         reference's `GetFarclip` and silently change which path the far-clip slider takes"
    );
}

/// `UIOptionsFrame_Load()` and `_Save()` run clean, as pfUI's `modules/gui.lua:146-148` calls them
/// around a checkbox write: every slider CVar `_Load` reads is registered, and `Slider:SetValue`
/// (`0x790980`) raises on nil in the reference too.
#[test]
fn the_stock_options_windows_load_and_save_are_reachable_for_addons() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefive".into()),
            level: 60,
            // `_Load`'s tail upper-cases `UnitClass("player")` (`UIOptionsFrame.lua:759-763`); a
            // warrior takes the branch that disables the combo-point box.
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            ..Default::default()
        }),
    );
    assert!(super::load_default_ui(&s).is_empty());

    // `_Load` first, the window's own order: the reference loads on show and saves on Okay.
    s.run("this = UIOptionsFrameOkay UIOptionsFrame_Load()")
        .expect("_Load runs to completion — every slider CVar it reads is registered");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Every slider CVar in the stock table must answer, or `_Load` raises.
    let unbacked: Vec<String> = s
        .eval(
            "local out = {} \
             for _, v in ipairs(UIOptionsFrameSliders) do \
                 if GetCVar(v.cvar) == nil then table.insert(out, v.cvar) end \
             end \
             return out",
        )
        .expect("read UIOptionsFrameSliders");
    assert!(
        unbacked.is_empty(),
        "these slider CVars answer nil, and `_Load` raises on the first of them: {unbacked:?}"
    );

    // The walk reached slider 3, Mouse Look Speed, on `cameraYawMoveSpeed`'s registered 180
    // (`UIOptionsFrame.lua:89`).
    let slider3: f64 = s
        .eval("return UIOptionsFrameSlider3:GetValue()")
        .expect("the Mouse Look Speed slider's value");
    assert!(
        (slider3 - 180.0).abs() < 0.001,
        "slider 3 should carry cameraYawMoveSpeed's registered 180, got {slider3}"
    );

    // `_SetDefaults` is the same walk through `GetCVarDefault`.
    s.run("this = UIOptionsFrameDefaults UIOptionsFrame_SetDefaults()")
        .expect("_SetDefaults runs to completion too");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Through the Okay button's own `<OnClick>` (`UIOptionsFrame.xml:1205-1209`), which sets
    // `this`: the `SHOW_PARTY_PETS` arm reaches `RefreshBuffs`, whose first act is
    // `this.hasDispellable = nil` (`BuffFrame.lua:266`).
    s.run("UIOptionsFrameOkay:Click()")
        .expect("the stock Okay button's own handler");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return UIOptionsFrame:IsShown()").unwrap(),
        "…and the window it hides on the way out was hidden to begin with"
    );

    // `_Save` writes the yaw slider's value and `cameraPitchMoveSpeed = value / 2`
    // (`UIOptionsFrame.lua:355-356`), as the registered defaults, 180 and 90, agree.
    let (yaw, pitch): (f64, f64) = (
        s.eval(r#"return tonumber(GetCVar("cameraYawMoveSpeed"))"#)
            .expect("yaw move speed"),
        s.eval(r#"return tonumber(GetCVar("cameraPitchMoveSpeed"))"#)
            .expect("pitch move speed"),
    );
    assert!(
        (yaw - 180.0).abs() < 0.001 && (pitch - 90.0).abs() < 0.001,
        "_Save should write the slider's 180 and its half; got yaw={yaw} pitch={pitch}"
    );
}

/// `assets/ui` does not grow: the interface is the stock 1.12 FrameXML off the player's own patch
/// chain, and a new file means a window was authored rather than migrated. Point `benilla.toc` at
/// `Interface\FrameXML\<Window>.xml`, delete ours, and build the engine verbs the stock file calls.
#[test]
fn assets_ui_does_not_grow() {
    const SHIPPED: &[&str] = &[
        "ContainerFrameAdapters.xml",
        "GameMenuFrame.xml",
        "KeyBindingsPage.xml",
        "OptionsFrame.xml",
        "ScriptLogFrame.xml",
        "ScrollTemplates.xml",
        "SpellBookAdapters.xml",
        "benilla.toc",
    ];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut found: Vec<String> = std::fs::read_dir(&dir)
        .expect("assets/ui")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    found.sort();
    let mut shipped: Vec<String> = SHIPPED.iter().map(|s| s.to_string()).collect();
    shipped.sort();
    assert_eq!(
        found, shipped,
        "assets/ui changed. It does not grow: a window is migrated, not authored (docs/METHOD.md). \
         A file that retired comes off this list; a new one needs a reason this list can name."
    );
}
