//! Whole-directory guards over the shipped `assets/ui/*.xml` — the coverage the per-window test
//! modules structurally cannot give, since each of those loads only the file it is about.

/// EVERY shipped `assets/ui/*.xml` parses — not just the ones a window test happens to load.
///
/// The per-window tests each load their own file, which left real holes: `CraftFrame.xml` and
/// `TradeSkillFrame.xml` ship in [`super::load_default_ui`]'s manifest and had NO parse coverage at
/// all, because the manifest is an inline array no test walks. A malformed comment in either (XML
/// forbids `--` inside `<!-- -->`, which a prose edit hits easily) would have reached a real run
/// untouched by a green suite.
///
/// Deliberately parse-only, not `loader::load`: loading one file out of manifest order reports
/// legitimate errors for templates its predecessors define, so a load-sweep would have to duplicate
/// the load order to say anything. Parsing is order-free and catches the whole well-formedness
/// class on its own.
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
    // Never let the sweep pass by finding nothing — a moved assets dir would otherwise turn this
    // into a test that guards zero files while staying green.
    assert!(
        // A sanity floor for the walk, not a census: `assets/ui` retires file by file (1751),
        // so the floor sits well under the count rather than one step above it (1956). Nine
        // files stand after 2014; the glue screens and the dev frames alone are more than six.
        checked >= 6,
        "only {checked} xml files swept — sweep broke"
    );
}

/// The WHOLE manifest LOADS, in its real order, with zero loader errors — the check the parse
/// sweep above structurally cannot give, and the one the app itself never made.
///
/// [`super::load_default_ui`] only ever *logged* its errors, so a broken manifest entry reached a
/// real run behind a log line nobody greps: a mistyped file name, a frame name colliding with a
/// later window's, a template used before its definer, an `<Include>` that resolves to nothing.
/// Nor could a capture run have caught it — captures skip the manifest entirely unless
/// `WOW_CAPTURE_UI=1`. This is that assertion, over the array the app really walks, so a new
/// window's entry is covered the moment it is added rather than when someone remembers to test it.
#[test]
fn the_whole_shipped_manifest_loads_without_errors() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
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

/// **Loading the UI makes no sound.** Materializing the shipped manifest is bookkeeping — nothing
/// has opened, so nothing may be heard.
///
/// The defect this pins: a dropdown's `OnLoad` calls `UIDropDownMenu_Initialize`, which —
/// faithfully, ref `UIDropDownMenu.lua` l.49-52 — *calls the init function immediately*. For the
/// unit popups that init function reaches `UnitPopup_ShowMenu`, whose last line is
/// `PlaySound("igMainMenuOpen")`. The ref never gets there at load: `UnitPopup_HideButtons` leaves
/// nothing but CANCEL shown for a unit that does not exist, tripping the "only one item, don't show
/// the menu" early-out. Ours was missing that hide for FOLLOW and INSPECT (ref l.304-307/316-319),
/// so all four party dropdowns rang on startup — four copies of the menu-open tack stacked in one
/// frame, on the login screen.
///
/// Deliberately asserted over the WHOLE manifest rather than the popup: any future window that
/// plays a sound from a load-time handler is the same bug, and this is where it gets caught. Sound
/// is on by default now, so a load-time sound is something the director hears on
/// every single launch.
#[test]
fn loading_the_shipped_ui_queues_no_sounds() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
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
         UnitPopup_HideButtons / UIDropDownMenu_Initialize (decision 1033)"
    );
}

/// **Every texture path the shipped UI names RESOLVES in the real archives** — the tripwire for a
/// mis-typed `file=`, which the renderer reports by drawing a plain WHITE QUAD and nothing else.
///
/// A sprite that fails to resolve is `None` all the way to `ui_pass`, where a texture-less quad
/// samples the shared 1×1 white image (that fallback is what makes flat-shaded quads batch, so it
/// cannot itself be made loud). The bug it hid (1046): six `file=` attributes in
/// `SpellBookFrame.xml` were written with **doubled** separators — `Interface\\SpellBook\\…`, the
/// Lua escaping, in an XML attribute where a backslash is already literal. `normalize_path` only
/// folds case and slashes, so the doubled key missed the archive hash and the pet book shipped with
/// a white slab over its autocast ring and another under its tab row. Every gate was green.
///
/// The shape half runs everywhere; the resolve half needs client data and skips without it (a
/// The reference's pet bar, up with the hunter fixture (Claw autocasting), settled past its
/// slide — the pet half of the two shine pins below.
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

/// The autocast **corner brackets** (`UI-AutoCastableOverlay`), per template — the sibling pin to
/// the shine's, and the same invariant: not a magic size, but the **bracket square against its
/// button**, which is the thing an eye actually reads.
///
/// The art fills only the middle `33/64` of the texture, so the drawn size decides where the
/// brackets land. The reference gets this right on the pet button (58 x 0.5156 = 29.9 on 30) and
/// loose on the spell book (60 x 0.5156 = 30.9 on 37 — three units inside each edge). Decision
/// 1393 carries the pet button's ratio across; this test is what keeps it carried, and what will
/// catch a third template added with a borrowed number.
#[test]
fn the_autocast_brackets_reach_each_buttons_corners() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::framexml::{Element, TopLevel};

    /// The fraction of `UI-AutoCastableOverlay.blp` the bracket art actually covers, measured off
    /// the shipped BLP (`benilla-extract blp`): art bbox 15..47 of 64.
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
    // The spell book's overlay is the reference's own template resized by SpellBookAdapters.xml's
    // script (1952/2014), so its number is measured off a VM, not read off an XML.
    let s = super::spellbook_tests::spellbook_ui(1024.0, 768.0);
    let (overlay, button): (f32, f32) = s
        .eval("return SpellButton1AutoCastable:GetWidth(), SpellButton1:GetWidth()")
        .unwrap();
    found.push(("SpellButton1AutoCastable".into(), overlay * ART, button));
    // The pet button's overlay is the reference's own template (1953) — measured the same way.
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
             pet button's is 0.997x, which is what puts brackets IN the corners (decision 1393)",
            brackets / button
        );
    }
}

/// The two shipped autocast-shine panes, and the rim/viewport ratio each one asks for — the one
/// place the widget's geometry is decided, so the one place to pin it. Read off the live panes
/// (the stock `$parentAutoCast` Models the tile renderer draws, 2013/2014), never an XML.
///
/// The pet button is the REFERENCE's own numbers (`setAllPoints` on 30x30 at `scale="1.2"`), and
/// its ratio is why that button reads as a rim: the rim square is 1.024x its viewport, so it runs
/// ON the edge and the tile's cell — the widget's own scissor — halves every star (1387/1391).
///
/// The spell book is **one deliberate deviation**. The reference writes
/// `scale="1.22"` into a 36-unit viewport — a 0.87x rim that floats clear of the edge, is never
/// clipped, and washes the icon; the real 1.12 client looks the same way (director-checked), so
/// this is taste, not fidelity. We re-seat the stock Model to 1.48 on a 37-unit rim (1393's
/// numbers, SpellBookAdapters.xml) to borrow the pet button's ratio. This test is what stops that
/// drifting, or being "corrected" back to 1.22 by someone who only read the ref.
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
             as a wash rather than a rim (decisions 1387/1392)",
            rim / view
        );
    }
}

/// hand-kept list of real file names would rot into agreeing with itself — the `text=` sweep's
/// argument below). Resolution goes through the renderer's own `sprite_candidates`, not a copy of it —
/// including its `.blp`/`.tga` fallback, so the sweep accepts exactly what the renderer accepts.
#[test]
fn every_shipped_texture_path_resolves_in_the_client_archives() {
    use benilla_ui::framexml::{Element, TopLevel};

    // `file=` on a texture element is an ARCHIVE path; `<Script file=>`/`<Include file=>` name our
    // own source files and are top-level items, not elements, so walking elements can't see them.
    fn walk(el: &Element, file: &str, out: &mut Vec<(String, String, String)>) {
        // `<Model file=>` names a **model** (`.mdx`), not a texture: it never reaches
        // `sprite_candidates` and it cannot draw a white quad, because this engine renders no
        // FrameXML models at all. The one shipped case is the reference's `CooldownFrameTemplate`,
        // transcribed with its own attributes so an addon's `inherits=` resolves, while the sweep
        // it drives lives in our native `<Cooldown>` widget instead.
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
    // Never let the sweep pass by matching nothing — the shipped UI still names over a hundred
    // textures (173 after 1971 retired the auction transcription, fewer again after 1973 the
    // crafting pair, 64 after 1980 the world map; the count falls with every window that
    // migrates, and the floor follows it).
    assert!(refs.len() >= 50, "only {} texture paths swept", refs.len());

    // The shape half: a doubled separator is the Lua escaping written into XML, and it resolves to
    // nothing. Checked without the client so a data-less machine still catches this exact class.
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

/// **Every archive path a shipped LUA chunk names survives its own escaping** — the mirror image
/// of the doubled-separator check above, and the tripwire for the defect that left the raid tab
/// with no window art at all (the `FriendsFrame_Update` arm was written
/// `"Interface\PaperDollInfoFrame\…"`, one backslash, so Lua ate both separators and
/// `SetTexture` was handed `InterfacePaperDollInfoFrameUI-Character-General-TopLeft`).
///
/// The two halves of the same trap, and they pull in OPPOSITE directions, which is exactly why
/// neither check can stand in for the other:
///
/// - an **XML attribute** is not a Lua string, so `file="Interface\Buttons\X"` is right and a
///   doubled separator there is the bug (the sweep above);
/// - a **Lua literal** is, so `SetTexture("Interface\\Buttons\\X")` is right and a single
///   separator here is the bug — and it fails SILENTLY: `\P` is not an escape Lua rejects, it is
///   one Lua drops the backslash from, so the string is well-formed, the call succeeds, and the
///   only symptom is a texture that never appears.
///
/// The shape half runs without client data; the resolution half needs the archives and skips
/// without them, the same posture as its sibling.
#[test]
fn every_archive_path_a_shipped_lua_chunk_names_survives_its_own_escaping() {
    use benilla_ui::framexml::{Element, ScriptRef, TopLevel};

    // The archive roots a path literal can start with. A prefix list rather than "has a
    // backslash", because Lua strings legitimately carry `\n`/`\124` and those are not paths.
    const ROOTS: [&str; 7] = [
        "interface",
        "textures",
        "world",
        "sound",
        "character",
        "item",
        "spells",
    ];

    /// Every `"…"`/`'…'` literal in one Lua chunk, as its RAW source text (escapes uninterpreted —
    /// what this sweep is looking for is an escape Lua would have eaten, so interpreting them
    /// first would destroy the evidence).
    ///
    /// **Comments and long strings are skipped, not scanned.** A `--` line quoting a path (this
    /// house writes plenty: "the file is `Interface\\Foo\\Bar`") is prose, and a prose backslash
    /// is nobody's bug — flagging one would train the next session to weaken this test rather
    /// than read it.
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
                // A newline before the closing quote means this was never a literal (an
                // apostrophe in a comment, most often) — drop it and resume at the next char.
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
                // Is this the WHOLE path, or a fragment? A literal with `..` against either side
                // is being concatenated onto, and one carrying a `%` spec is a `format` template —
                // in both cases the string in the source is a prefix and resolving it would fail
                // by construction. Read off the SITE rather than guessed from the text (a
                // "ends with a dash" rule would be a list to feed forever), so a fragment that
                // later becomes whole starts being resolved on its own.
                let before = src[..i].iter().rposition(|c| !c.is_whitespace());
                let after = src[j + 1..].iter().position(|c| !c.is_whitespace());
                let joined = before.is_some_and(|k| k >= 1 && src[k] == '.' && src[k - 1] == '.')
                    || after.is_some_and(|k| {
                        src.get(j + 1 + k) == Some(&'.') && src.get(j + 2 + k) == Some(&'.')
                    });
                // …and the two fragments a `..` cannot see, because they are assigned to a
                // variable first and joined somewhere else: a literal ending in the SEPARATOR is a
                // directory, and one ending in a DASH is a family prefix (`"…\\MageFire-"` .. rank).
                // Neither is a file name — no texture in the chain ends in either character — so
                // this excludes fragments without excluding anything real.
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

    // Every Lua chunk in the file: the top-level `<Script>` blocks and every element body (the
    // `<OnLoad>`-family handlers). Through the PARSER, not a text scan — an attribute value is
    // also inside double quotes, and a text scan cannot tell the two apart, which is the whole
    // distinction this test exists to make.
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
    // Never let the sweep pass by matching nothing; the floor follows the census down as windows
    // migrate.
    assert!(
        paths.len() >= 3,
        "only {} archive paths swept out of the shipped Lua",
        paths.len()
    );

    // The shape half: after collapsing every `\\` pair, no backslash may remain — one that does
    // is a separator Lua is about to eat.
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
        // `sprite_candidates` answers for TEXTURES; a sound or a model path is a different
        // resolver's business, so only the two texture roots take the resolution half — and only
        // a WHOLE path, never a concatenation fragment.
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

/// **Every `text=` in the shipped UI is answerable against the REAL `GlobalStrings.lua`** — the
/// tripwire for the defect that put "CREATE_MACROS" across the macro window's title bar.
///
/// `text=` is a global-string LOOKUP, not a literal (`FrameScript_GetText 0x703bf0`). The loader
/// didn't do the lookup at all, so **23 key-shaped values across six windows** were rendering as
/// their own key names — and nothing anywhere said so. Two halves, both needed:
///
/// - a **key-shaped** value (`SCREAMING_SNAKE`) must resolve to a non-empty string, or it reaches a
///   real screen as its own key;
/// - a **literal** value ("Send Mail") must NOT collide with a real global, because the loader's
///   deliberate literal-fallback divergence means a collision would silently swap the words on
///   screen for a localized string nobody asked for.
///
/// Skips without client data (the real string table is the whole point — a hand-kept copy of the
/// keys would rot into agreeing with itself).
#[test]
fn every_shipped_text_attribute_answers_against_the_real_global_strings() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let s = benilla_ui::script::UiScript::new().expect("VM");
    s.run(&String::from_utf8_lossy(&src)).expect("runs clean");

    // The loader's own shape test, restated here so the two can't drift apart silently.
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
    // Never let the sweep pass by matching nothing: 23 key-shaped values across six windows was
    // the floor as of 0991; ONE is left after 1971, and a regex that stops matching is exactly how
    // this guard would retire — so the floor is one, and the guard retires with the last file of
    // ours that writes a key-shaped `text=`.
    assert!(keys >= 1, "only {keys} key-shaped text= values swept");
}

/// **No shipped script hands a GlobalStrings KEY to a text sink as if it were the string.**
///
/// The defect this pins, twice now: `TargetFrame_CheckDead` did `deadText:SetText("DEAD")`, and
/// `DEAD` is not the word — it is `GlobalStrings.lua` l.898's key for `"Dead"`. So the target
/// frame wore **DEAD** in caps over every corpse, in every locale, until the director looked at
/// an Onyxia and said so. The same class shipped once before with `"CREATE_MACROS"` written
/// across the macro window's title bar (0983 → 0991).
///
/// The XML half of this class is already guarded twice: `loader::resolve_text` warns at load
/// whenever a **key-shaped** `text=` attribute resolves to nothing, and the sweep directly above
/// answers every one of them against the real `GlobalStrings.lua`. The **Lua** half had no guard
/// at all — `SetText` takes a string and cannot tell a key from a word — and that is the
/// half both escapes came through. This is that missing guard, as a shape test: a literal that is
/// all `[A-Z0-9_]` with an uppercase in it is `is_global_string_key`'s own predicate, and nothing
/// we would ever want a player to read.
///
/// Swept over the whole directory rather than the file that broke, because the next one will be a
/// different window. If a genuinely uppercase word ever needs to reach a sink, it goes through the
/// house's `gs(key, fallback)` helper (AuctionFrame/HelpFrame/MailFrame) or the XML `text=`
/// attribute — both resolve the global first, and both are what "faithful" means here.
#[test]
fn no_shipped_script_sets_a_global_string_key_as_display_text() {
    /// `loader::is_global_string_key`'s predicate, restated here because it is private: at least
    /// two characters, uppercase/digit/underscore only, and at least one uppercase.
    fn key_shaped(s: &str) -> bool {
        s.len() >= 2
            && s.chars().any(|c| c.is_ascii_uppercase())
            && s.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    }

    // The literal after a text sink's open paren, if the call opens on this line at all. Prose is
    // the noise here — the house documents its own history in comments, this very record included
    // — so XML comment blocks are tracked across lines and Lua `--` lines are dropped.
    fn sink_literals(line: &str) -> Vec<&str> {
        let mut out = Vec::new();
        // `:SetFormattedText(` used to sit between these two and never could have matched —
        // it is not a 1.12 verb, so no shipped script writes it (2142 retired ours).
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
    // The sweep must never pass by finding nothing to sweep.
    // The same walk floor as above (1956).
    assert!(swept >= 6, "only {swept} xml files swept — sweep broke");
}

/// **The `$parentTextureFrame` idiom's contract, over the whole shipped UI**: a frame whose art is
/// meant to cap a unit frame must sit at a strictly HIGHER frame level than that frame's status
/// bars — because frame level is the only key term above the draw layer, and the layer would
/// otherwise lift an ARTWORK bar fill straight over BACKGROUND art.
///
/// Decision 0884 pinned the layer as bucket-wide and above the frame. Two files had been getting
/// this right for the wrong reason — our `UnitFrames.xml` and `PartyFrame.xml` both *declared* the
/// TextureFrame after the bars and leaned on the retired key's insertion-order tie-break — so both
/// inverted the instant the key was corrected, and both reached the director's screen. (Both files
/// are retired now, 1751; the lesson is kept because it is about the KEY, not about them.) The
/// reference spends a real frame level on this in both of its own spellings (`TargetFrame.lua`
/// l.32-34's explicit `SetFrameLevel(textureFrame-1)`, `PlayerFrame.xml` l.50-52's two anonymous
/// nesting frames); benilla now uses the first, everywhere.
///
/// Name-driven on purpose, so a *new* copy of the idiom is covered the day it is written rather
/// than when someone remembers to test it. A geometric sweep was run once over this same UI —
/// every substantially-overlapping cross-frame quad pair at one `(strata, level)`, i.e. every pair
/// the draw layer alone orders — and found no third instance; the surviving 45 pairs were all
/// benign (action-bar and bag-slot chrome adjacency, text over art, and a bar fill over its own
/// frame's trough, which is the intended order). It is not kept as a gate: frozen, that list would
/// churn on every action-bar edit, and a noisy gate is a gate nobody reads.
#[test]
fn every_texture_frame_outranks_its_status_bars() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::order::unpack;

    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
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
    // Every unit-frame family painting at once: the frames hide themselves without a unit, and a
    // hidden frame emits no quads to read a level from.
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
    // The target-of-target frame ships OFF (`SHOW_TARGET_OF_TARGET = "0"`, the reference's own
    // default, declared in OptionsFrame.xml which the manifest above has just loaded). Turn it on
    // here: it is the SECOND of the two named TextureFrames the reference spells this way, and a
    // gate that only ever sees the first covers half of what it claims to.
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
        for suffix in ["HealthBar", "ManaBar", "ManaBar"] {
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
    // **The predicted reduction, arrived.** This asserted 12 while the unit frames were ours,
    // because our transcription spelled the idiom with a named `$parentTextureFrame` on EVERY
    // family. The reference does not: it has exactly TWO named ones, `TargetFrameTextureFrame` and
    // `TargetofTargetTextureFrame`, both in TargetFrame.xml. `PlayerFrame`, `PetFrame` and the
    // party rows reach the same order the other way — the art inside ANONYMOUS nested
    // `<Frame setAllPoints="true">` wrappers, outranking the bars by nesting depth rather than by
    // a level on a name. There is nothing for a name sweep to look up, so those families move from
    // this assertion's cover to the loader's nesting rule.
    //
    // The note above this line predicted the drop when PartyFrame went to the chain and called it
    // "a real reduction rather than a rename". That was right, and it under-counted: the whole kit
    // went at once, so player and pet left by the same door. Two families × (HealthBar, ManaBar,
    // ManaBar) = 6 is the reference's own coverage, and it is the floor now. Still never zero —
    // matching nothing is exactly how a renamed frame would silently retire the check.
    assert!(
        checked >= 6,
        "only {checked} texture-frame/bar pairs checked — the name sweep found nothing"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The boot phase is inert.** `Fonts.xml` — the only file loaded at `Startup` since 1051 — is a
/// pure registry: it materializes no frames, so nothing of the in-game UI can draw, tick or ring
/// before a character is in the world. (1033's tack was a load-time handler firing on the login
/// screen; this is the structural half of that fix.)
#[test]
fn the_boot_phase_materializes_no_frames() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The font registry is manifest entry 0 and comes off the chain since 1888, so this reads
    // through `test_ui::load_ui` rather than joining `assets/ui` — the file is not ours any more.
    // `load_ui` returns the same `report.frames` this asserted on and fails on any loader error.
    let frames = super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    assert_eq!(
        frames, 0,
        "the boot-phase load materialized {frames} frame(s) — the login screen is meant to carry none"
    );
}

/// **The font registry alone covers the WHOLE glyph-atlas bake plan** — the property that makes
/// 1051's split safe, asserted rather than reasoned.
///
/// Our native glue screens share the one atlas, and that atlas bakes **once**, on the first
/// `Update`, from `script.font_objects()` (`ui_text/atlas.rs` — the face size-list extension *and*
/// the outlined-cell census). So the registry must be complete at boot even though the other 55
/// manifest files now load at world entry. It is: three font objects live outside `Fonts.xml`
/// (`GameFontNormalMed1` 13, `OptionsFontHighlightMedium` 14, `OptionsFontHighlightHuge` 20), all
/// un-outlined, all at heights `Fonts.xml` already declares.
///
/// If this fails, someone added a font object to a non-`Fonts.xml` file with a new height or an
/// outline. In-game text would silently lose that variant for the whole session — the bake has
/// already happened by the time the file loads. Either move it into `Fonts.xml` or make the atlas
/// rebakeable.
#[test]
fn the_font_registry_alone_covers_the_whole_bake_plan() {
    benilla_formats::wow_data_or_skip!();
    let plan = |whole: bool| -> std::collections::BTreeSet<(String, String, String)> {
        let mut s = benilla_ui::script::UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        if whole {
            // The in-game UI materializes on world entry (1051), so a player always exists by the time the
            // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
            // into its label inside its own OnLoad. A manifest load with no player is a state the client
            // never reaches.
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

/// The whole shipped UI survives `VARIABLES_LOADED` — the event the saved-variables
/// load fires at every launch, and which nothing fired before this arc existed.
///
/// Today no shipped file registers it, so this asserts a clean no-op; the moment one does (the
/// combat-text option family and `TwentyFourHourTime` are next on 1128's list) this is what catches a
/// handler that errors on the one event that runs before any window has been shown.
#[test]
fn the_shipped_ui_takes_variables_loaded_without_a_script_error() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
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
    let _ = s.errors(); // drain anything the load itself logged; this test is about the event
    s.fire_event("VARIABLES_LOADED", vec![]);
    assert!(
        s.errors().is_empty(),
        "VARIABLES_LOADED script errors: {:?}",
        s.errors()
    );
}

/// **Every `<Font name=…>` the shipped manifest declares is a real Lua global** — a `Font` object
/// answering the FontInstance getters, not a bare style record with no name.
///
/// The per-fragment test in `benilla-ui` proves the mechanism on a two-font document; this proves
/// it over the 54 fonts our real `Fonts.xml` (and the windows after it) actually declare, in
/// manifest order, which is the only place a name collision with a *frame* of the same name — the
/// one way publication can silently not happen, since `publish_global` never overwrites — could
/// show up.
///
/// The named spot-checks are the corpus's four most-wanted font objects: `GameFontNormal` (98
/// addons), `GameTooltipText` (89), `GameFontHighlightSmall` (69), `GameTooltipHeaderText` (the
/// `Tablet-2.0.lua:289` header-size probe, 268 read sites).
#[test]
fn every_shipped_font_object_is_published_as_a_lua_global() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
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

    // Collect the declared names straight out of the MANIFEST's own entries, so the sweep cannot
    // go stale — and so it follows a file onto the chain. It swept `assets/ui` until 1888 put the
    // font registry on the chain and the count fell from 51 to 3, which is the failure this shape
    // prevents: the sweep's subject is what the manifest declares, not what happens to be ours.
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

    // The four the corpus wants most actually carry a face and a size, not just a name.
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

/// **Two reference templates addons INHERIT, and the silence that hid them.**
///
/// `TargetBuffButtonTemplate` (ref `TargetFrame.xml`) and `MainMenuBarMicroButton` (ref
/// `MainMenuBarMicroButtons.xml`) are declared by the reference and were not by us. An unresolved
/// `inherits=` is **silent** — no load error, no session error, nothing in any harness column — so
/// a consumer just gets a button with no size, no hit-rect and no scripts, and nothing anywhere
/// says so. They surfaced only in the report's missing-TEMPLATES ranking.
///
/// Asserted through what a consumer actually gets: inherit the template, then read back the
/// geometry and the script the reference confers.
#[test]
fn the_inheritable_reference_templates_confer_their_shape() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
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

    // `CT_UnitFrames/CT_TargetFrame.xml:4` builds its own virtual template on top of this one, so
    // the inherit has to work through a second hop as well as directly.
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

/// **`CursorUpdate` / `CursorOnUpdate` — the inspect-cursor pair, transcribed from FrameXML.**
///
/// Both are `framexml` origin in `reference/1.12-globals.tsv` (while `ResetCursor` and
/// `ShowInspectCursor` beside them are `engine`), so they are ours to write rather than to bind.
/// Two corpus addons call `CursorUpdate` and one calls `CursorOnUpdate`; it is also one of the
/// blockers on two reference templates addons inherit, and the sourced `ContainerFrame.lua` calls
/// it from its keyring fork.
#[test]
fn the_inspect_cursor_pair_takes_both_arms() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
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

    // Both exist as functions — the shape a caller checks before hooking.
    assert_eq!(
        s.eval::<(String, String)>("return type(CursorUpdate), type(CursorOnUpdate)")
            .unwrap(),
        ("function".into(), "function".into())
    );

    // **The no-ctrl arm, which is the one that runs in practice.** `this.hasItem` is read as a
    // plain truthy field, so a frame that never sets it simply takes `ResetCursor` — this is why
    // the pair is safe to ship before anything sets that field.
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

    // `CursorOnUpdate` gates on tooltip ownership, so with the tooltip unowned it must do nothing
    // at all rather than reach through to the cursor.
    s.run("this = CursorProbe CursorOnUpdate() this = nil")
        .unwrap();
    assert!(
        s.errors().is_empty(),
        "the unowned-tooltip gate short-circuits: {:?}",
        s.errors()
    );
}

/// **Nothing of the interface is left on screen during a cinematic** — swept over the whole
/// shipped manifest, not one file at a time.
///
/// This is the test the director's eye had to stand in for. Decision 1734 restored 72 dropped
/// `parent=` declarations and a per-window test proved the cascade worked; the chat still drew
/// over the fly-by, because benilla carries the reference's `FloatingChatFrameTemplate` and
/// `ChatTabTemplate` under its own names (`BenillaChatFrameTemplate`, `BenillaChatTabTemplate`)
/// and the gap analysis that found the 72 skipped `virtual="true"` templates entirely. A
/// name-for-name comparison against the reference could not see it. **Sweeping what is actually
/// visible can**, which is why this is written against the observable and not against a list.
///
/// The two survivors are each required to survive (a third, `WorldFrame`, was one while our
/// `UIParent.xml` declared it; since decision 1983 it is the reference's own file off the chain,
/// which this sweep over the SHIPPED tree does not walk — it stays up through a fly-by exactly as
/// before, top-level and never hidden):
///
/// - `CinematicFrame` — the frame being *shown*. The reference declares it with no parent for
///   exactly this reason, and `SetFullScreenFrame` shows it in the same breath as hiding UIParent.
/// - (Until 1988 a `BenillaFadeDriver` survived beside it — our fade kit's tick frame. The stock
///   `UIFrameFadeUpdate` runs from UIParent's own OnUpdate, which a cinematic's `UIParent:Hide()`
///   stops exactly as the reference's does.)
#[test]
fn a_cinematic_leaves_nothing_of_the_interface_on_screen() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
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

    // The sweep's subjects: the frames OUR files declare — plus the reference's own HUD, because
    // `assets/ui` is nearly empty now (1988 retired the last of the glue) and a sweep over what is
    // left would pass without ever looking at the interface the cascade actually hides.
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
            // The frame being SHOWN — the sweep's one expected survivor.
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

    // The sweep is only worth anything if there was something to hide in the first place.
    let before = names.iter().filter(|n| visible(&s, n)).count();
    assert!(
        // 50 until 1938 took the three bar files stock, 20 until 1974 took the minimap cluster
        // (its 22 named frames were most of what this sweep counted), 12 until 1987 took the
        // micro row and 1988 the glue. The floor is over the named HUD above now, not over our
        // files' census, so it stops following the migration down.
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

/// **Every `parent=` the shipped tree declares REALLY attaches** — the tripwire for 1734's trap.
///
/// A parent name is resolved at LOAD, and a name that names nothing is **not an error**: the loader
/// warns ("names no frame — falling back to the enclosing one") and silently reparents to whatever
/// element encloses the declaration. So a typo, a renamed window, or a file that loads earlier in
/// the manifest than its parent's leaves the frame attached to the wrong thing with a green suite
/// and one log line nobody greps — which is how 1734's 41 harness failures were the *lucky*
/// outcome and a silent half-fix was the unlucky one.
///
/// Asserted on the LOADED tree, not on the text: `GetParent():GetName()` is what the frame ended up
/// attached to, which is the only form of this claim worth making. It is also the standing answer
/// to "does this engine read a cross-file `parent=`?" — several file headers still said it does
/// not, years after it did, and five pages carry a hand-written substitute for `setAllPoints`
/// because of it.
#[test]
fn every_declared_parent_really_attaches() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
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
        // A sanity floor for the scan, not a census — the declarations retire with the files
        // that carry them (1751); 67 before 1938, 30 after 1956, 14 after 1970, 2 after 1987
        // (the micro row's eight were most of what was left).
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

/// `(named non-virtual frame, the `parent=` it declares)` for every such declaration in the shipped
/// tree — the population of [`every_declared_parent_really_attaches`].
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

/// Every named, non-virtual frame the shipped tree declares — the sweep's population.
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

/// **The icon picker opens, against the SHIPPED manifest.** Not a harness list — the manifest, the
/// way the client loads it.
///
/// The distinction is the whole point. `macro_tests` stands this window up from its own file list
/// and passed throughout, because a harness names the dependencies it needs; the manifest had not
/// listed `ClassTrainerFrameTemplates.xml`, so in the client `MacroPopupScrollFrame` inherited a
/// template nothing had loaded, came up bare, and `MacroPopupFrame_Update` multiplied a nil offset.
/// A window is only as loaded as the manifest says.
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
    // One macro, so the edit path has something selected — the director's own flow was a saved
    // macro and the "Change Name/Icon" button beside it.
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

    // The window is a LoadOnDemand addon the app seats at setup (1957) and `ShowMacroFrame`
    // loads through the reference's own `MacroFrame_LoadUI` (1967).
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

/// **The reference's own Interface Options window is on the manifest, and it stays HIDDEN**
/// — the whole shape of that record, asserted in one place.
///
/// Four claims, and each one has a way to fail that nothing else here would catch:
///
/// 1. **`UIOptionsFrame` is the reference's frame, not an alias onto ours.** The alias
///    (`UIOptionsFrame = OptionsFrame`, when that was our window's name) is what this replaces, and under it every one of these
///    reads would have succeeded while pointing at the window the player opens — pfUI's skin does
///    `UIOptionsFrame:SetWidth(1024)`. So the test asks for the reference's own CHILDREN, which an
///    alias could never grow.
/// 2. **It is never shown.** `hidden="true"` is the stock file's own attribute and nothing of ours
///    may `Show()` it.
/// 3. **`UIOptionsFrameCheckButtons` carries the five rows `MultiActionBars.lua:10` writes** —
///    which IS the load-order proof. That file writes them at its own load, under the reference's
///    own comment *"Hack to get around load order dependencies"*, so the rows are there only if
///    the options window's manifest row sits above the bars', as `FrameXML.toc` l.21 vs l.39 has
///    it. Reorder the manifest and this is what goes red.
/// 4. **Ours is still the player's.** `GameMenuButtonOptions` opens `BenillaOptionsFrame`.
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

    // 1 · the reference's own frame, by the children only it declares.
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

    // 2 · …and hidden, by its own file's attribute.
    assert!(
        !s.eval::<bool>("return UIOptionsFrame:IsShown()").unwrap(),
        "the stock window must never be on screen"
    );

    // 3 · the order proof.
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

    // 4 · ours is still the window the ESC menu opens.
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

/// **The reference's own Sound Options window is on the manifest, HIDDEN, and the alias is gone**
/// — 2115's argument applied to the window its §9 handed over.
///
/// Four claims, each with its own way to fail:
///
/// 1. **`SoundOptionsFrame` is the reference's frame, not an alias onto ours.** This one is not
///    the interface window's story repeated: our `OptionsFrame.xml` loads *after* the stock file,
///    so `SoundOptionsFrame = OptionsFrame` — the alias as it was written, when that was our
///    window's name — would not have sat beside the real frame; it would
///    have **clobbered** it, and pfUI's `options-sound.lua` would then have stripped textures off
///    and re-anchored the window the player opens. The check is the reference's own CHILDREN,
///    which an alias could never grow, plus the identity itself.
/// 2. **It is never shown** — `hidden="true"` at the stock file's own xml l.18.
/// 3. **`SoundOptionsFrame_Load()` runs clean**, which is the Sound window's counterpart to the
///    interface window's `cameraYawMoveSpeed` hole: its `_Load` reaches
///    `slider:SetValue(value.initialValue)` for four sliders, and all four of their CVars are
///    registered. The three check-button CVars that were missing are built now too, so the walk
///    has nothing to trip on.
/// 4. **Ours is still the player's** — the ESC menu's Options button opens `BenillaOptionsFrame`, and
///    neither stock window.
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

    // 1 · the reference's own frame, by the children only it declares — and this list IS the set
    // pfUI's `skins/blizzard/options-sound.lua` walks (there is deliberately no CheckButton3 in
    // the stock file; pfUI's own `if btn then` guard handles the hole).
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

    // 2 · …and hidden, by its own file's attribute.
    assert!(
        !s.eval::<bool>("return SoundOptionsFrame:IsShown()")
            .unwrap(),
        "the stock Sound window must never be on screen"
    );

    // 3 · `_Load` runs clean, and every CVar its two tables name answers.
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

    // 4 · ours is still the window the ESC menu opens.
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

    // The two stock consumers still answer correctly with every alias retired. They read the
    // reference's three window names unguarded in one `or` chain, and since 2177 all three are
    // real hidden frames — so that chain is now false whatever our window is doing, and the
    // wrappers `GameMenuFrame.xml` installs are the whole reason these still say yes.
    assert!(
        s.eval::<bool>("return IsOptionFrameOpen() and true or false")
            .unwrap(),
        "UIParent.lua:997 must still see an open options window with the alias retired"
    );
}

/// **The reference's own VIDEO options window is on the manifest, HIDDEN, it owns the
/// `OptionsFrame` name again, and ours is still the player's**.
///
/// The last of the reference's three options windows, and the only one whose arrival took a name
/// off a file of ours — which is what makes this test different from its two siblings above.
///
/// Six claims:
///
/// 1. **`OptionsFrame` is the stock video window**, by children only that file declares. The list
///    is exactly what pfUI's `skins/blizzard/options-video.lua` walks — `OptionsFrameHeader`, the
///    five box frames, `OptionsFrameSlider1..9`, `OptionsFrameCheckButton1..18`, the three
///    dropdowns and the three buttons. Under the old arrangement `CreateBackdrop(OptionsFrame)`
///    and `OptionsFrameHeader:SetTexture("")` landed on the window the player opens.
/// 2. **It is never shown** — `hidden="true"` at the stock file's own xml l.5.
/// 3. **Ours is a different frame with a different name, and it is still the player's**: the ESC
///    menu's Options button opens `BenillaOptionsFrame` and no stock window, and it holds the
///    native-centre panel slot, which is the `UIPanelWindows` row that had to be restated.
/// 4. **The three stock "is an options window open?" consumers still answer correctly.** They
///    name the reference's three windows literally, and all three are hidden frames now, so the
///    honest chain is false — the wrappers in `GameMenuFrame.xml` are the whole reason
///    `IsOptionFrameOpen` says yes while ours is up, and the ESC ladder still closes it.
/// 5. **`OptionsFrameSliders` is the reference's nine rows**, not our three, and our Graphics page
///    reads its bounds off them.
/// 6. **The verbs answer with the shapes their consumers demand** — `"WxH"` with no decoration,
///    a `GetCurrentResolution` that really indexes the list `CT_Viewport.lua:105` reads it against,
///    the single `0` that makes `OptionsFrame_GetRefreshRates` grey its own dropdown, and seven
///    caps whose flags are `1`/nil (the one type assignment that satisfies both `not hasPixel\
///    Shaders` and `hasTripleBuffering == 1` in the same function).
#[test]
fn the_stock_video_options_window_loads_hidden_and_owns_its_own_name() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The display facts the host pushes in a real run. 1600x900 is deliberately NOT one of the
    // monitor's modes: the windowed size a player runs at usually is not, and the setter's promise
    // that `GetCurrentResolution` indexes a row that exists is exactly what that case tests.
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

    // 1 · the reference's own frame, by the children only it declares — and this list IS what
    // pfUI's `skins/blizzard/options-video.lua` walks.
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

    // 2 · …and hidden, by its own file's attribute.
    assert!(
        !s.eval::<bool>("return OptionsFrame:IsShown()").unwrap(),
        "the stock Video window must never be on screen"
    );

    // 5 · the reference's own nine slider rows, and our Graphics page standing on them.
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

    // 6 · the verbs, at the shapes their consumers demand.
    let res: Vec<String> = s
        .eval("return { GetScreenResolutions() }")
        .expect("the resolution list");
    assert_eq!(
        res,
        ["1280x720", "1600x900", "1920x1080"],
        "`WxH`, ascending by pixel AREA (the reference's own key), with the live windowed size \
         folded in — 1600×900 is not a monitor mode and is exactly the case CT_Viewport needs"
    );
    // `CT_Viewport.lua:105` is literally `arg[GetCurrentResolution()]` over these varargs, and
    // `:107` then re-parses it with `string.find(currRes, \"(%d+)x(%d+)\")`. Both, verbatim.
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
    // **Zero return values**, which is the reference's own "nothing to offer" answer
    // (`0x48c136 xor eax,eax`). It was written here as a single `0` first, from
    // `OptionsFrame_GetRefreshRates`'s `arg.n == 1 and arg[1] == 0` opening; the binary has no
    // `push 0` path at all, and that FrameXML branch is reachable only when the OS reports a `0`
    // refresh rate for every matching mode.
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
    // The optional index argument is tolerated, which is the shape `SetScreenResolution` shares.
    assert!(
        s.eval::<bool>("return table.getn({ GetRefreshRates(2) }) == 0")
            .unwrap(),
        "the argument is optional AND ignored here; it must not raise"
    );
    // The caps, at the two types the one consuming function demands of them.
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
    // The stock defect this reproduces, asserted as a defect: `OptionsFrame_Load` tests
    // `not hasTripleBuffering` twice and `hasTripleBuffering == 1` once. Slot 5 is never nil, and
    // `0` is truthy in Lua — so the two `not` clauses are dead in the reference too, and only the
    // `== 1` one decides anything. Answering nil here would REVIVE a branch the binary cannot reach.
    assert!(
        s.eval::<bool>(
            "local a, b, c, d, hasTriple = GetVideoCaps() \
             return (not (not hasTriple)) and hasTriple ~= 1"
        )
        .unwrap(),
        "`not hasTripleBuffering` must stay FALSE (the dead clause) while `== 1` is also false"
    );

    // `SetScreenResolution`'s three argument behaviours in the reference, and the one deliberate
    // divergence.
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
    // …and now the CVar says 1920x1080 while the live window is still 1600x900, which is exactly
    // the case `GetCurrentResolution` must answer from the WINDOW rather than from the CVar.
    assert_eq!(
        s.eval::<f64>("return GetCurrentResolution()").unwrap(),
        2.0,
        "the live size is still entry 2; a pick stages `gxResolution` and takes effect on apply"
    );

    // 3 · ours is still the window the ESC menu opens, in the centre slot.
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

    // 4 · the three stock consumers, which now hear about our window only through the wrappers.
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

/// **The display-brightness pair** — `GetGamma`/`SetGamma` and the Graphics page
/// row that drives them, held to the reference's own behaviour.
///
/// The claim that needs an assertion rather than a comment is the **unit**: `0x4891d0` is FSUBR,
/// so `GetGamma()` is `1.0 − gamma` and `SetGamma(v)` writes `gamma := 1.0 − v`. Read as
/// "gamma in, gamma out" the pair still composes to the identity and still *looks* right in a
/// round-trip test — but the panel would then put `gamma = 0` (`pow(x, 0) = 1`, a white screen)
/// at the top of a slider whose top is supposed to be the brightest legible picture. So this
/// checks the two ENDS of the reference's own `[-0.5, 0.5]` against the CVar text, not just that
/// get and set agree with each other.
///
/// And the second claim: **there is no clamp**, anywhere, in the reference (the positive control
/// is `baseMip`'s validating callback `0x689090`). `SetGamma(5)` writes `"-4.000000"` and the
/// store keeps it — benilla's clamp is at the render consumer, where it cannot lie to `GetCVar`.
#[test]
fn the_display_brightness_pair_speaks_the_reference_slider_unit() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");

    // The reference registers `gamma = "1.0"` (our table spells it `"1.000000"`, the spelling
    // `SetGamma` itself writes — see its row in `crate::cvars`), and 0.0 is what that reads as:
    // the exact centre of the stock slider.
    assert_eq!(
        s.eval::<f64>("return GetGamma()").unwrap(),
        0.0,
        "a fresh client sits at the centre of the stock slider's [-0.5, 0.5]"
    );
    // Both ends, in the CVar's own text — `SStrPrintf(buf, 0x10, "%f", 1.0 - v)`.
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
    // Zero return values, not nil (`eax = 0` at every `ret`). `select` is not in this VM (2171),
    // so the count is read the way 1.12 Lua reads one: a multiple assignment.
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
    // …and it REQUIRES its argument (`0x4891fe`, raising through `0x6f4940`, which never returns).
    let err = s.eval::<()>("SetGamma()").unwrap_err().to_string();
    assert!(
        err.contains("Usage: SetGamma(value)"),
        "the reference's own usage string, verbatim: {err}"
    );

    // The row: our Graphics page drives the PAIR, not the CVar, which is 1.12's own arrangement
    // for this one slider — and it reads its bounds off the reference's `OptionsFrameSliders[6]`.
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
    // The step is an f32 on the widget, so it comes back as 0.100000001…; the two bounds and the
    // numeric flag are exact.
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
    // The readout is the thumb's share of the groove (0..100 with the default at 50), never the
    // stored offset — "0%" on a brightness control doing nothing wrong is exactly backwards. It is
    // written by the page's own refresh, so the window has to be up for there to be one.
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

/// **The ten names the video window's slider walk must NOT find**.
///
/// `OptionsFrame_Load:110` and `_Save:208-209` do `getglobal("Get"..value.func)` /
/// `("Set"..value.func)` over the nine `OptionsFrameSliders` rows and **branch on the result**: a
/// hit is called, a miss falls through to `GetCVar`/`SetCVar`. Of the eighteen composed names, six
/// resolve to real bindings in the reference — `Get/SetWorldDetail`, `Get/SetTerrainMip`,
/// `Get/SetBaseMip` — and ten must resolve to nil. Defining any of the ten changes this window's
/// behaviour **without erroring**, which is why it needs an assertion rather than a comment.
///
/// The trap that makes it worth pinning: `GetFarclip 0x488f00` and `SetFarclip 0x488f30` really do
/// exist in the reference, with a capital F, while `value.func` is the lowercase `"farclip"`. The
/// stock client takes the CVar path for far clip **only because `getglobal` is case-sensitive** — so
/// this asserts that too, against a name we DO define.
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
    // Case-sensitivity, against a name that IS defined (2163's Environment Detail pair).
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

/// **pfUI's `UIOptionsFrame_Save()` path runs clean, and `_Load()` stops at exactly one thing**.
///
/// pfUI's `modules/gui.lua` l.146-148 wraps a GVAR checkbox's write in
/// `UIOptionsFrame_Load()` … `UIOptionsFrame_Save()`, so both are reached at runtime by a real
/// addon and both had to be more than nil.
///
/// **Both run clean now.** 2115 shipped this test asserting that `_Load` *raised* — pinning the
/// gap rather than hiding it, and saying in as many words that "the day something registers it,
/// THIS assertion goes red and gets deleted". That day is this change: `UIOptionsFrameSliders`
/// row 3 is `cameraYawMoveSpeed`, `_Load` does `slider:SetValue(GetCVar(value.cvar))`, and
/// `Slider:SetValue` is a shape-A binding (`0x790980`) that
/// raises on a nil in the reference too. All four slider CVars are registered, so the walk reaches
/// its end — and `_SetDefaults`, which does the same through `GetCVarDefault`, with it.
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
            // **The player has a CLASS, because a real one always does** — and `_Load`'s tail
            // needs it: `UIOptionsFrame_UpdateDependencies` (lua l.759-763) does
            // `local temp, class = UnitClass("player"); class = strupper(class)` to decide whether
            // the combo-point combat-text box applies, and `strupper(nil)` raises. It could not be
            // reached before, because `_Load` died at slider 3 forty lines earlier — so this is
            // 2115 §10's lesson a second time, in the same window: a probe VM that runs the stock
            // interface has to be a VM the stock interface's own assumptions hold in. Warrior
            // rather than Rogue/Druid so the DISABLE branch runs, which is the one with a call in
            // it (`OptionsFrame_DisableCheckBox`).
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            ..Default::default()
        }),
    );
    assert!(super::load_default_ui(&s).is_empty());

    // The Okay path, whole: 69 check buttons, four sliders, four dropdowns, the multibar toggles
    // and the combat-text tail. Driven through the reference's OWN caller — `UIOptionsFrameOkay`'s
    // `<OnClick>` (xml l.1205-1209) — rather than as a bare call, and that is not ceremony: the
    // `SHOW_PARTY_PETS` arm reaches `RefreshBuffs`, whose first act is `this.hasDispellable = nil`
    // (`BuffFrame.lua:266`), so the reference's ambient handler global has to be set. It is, at
    // every real call site: the Okay button here, and pfUI's own checkbox on its GVAR path.
    // **`_Load` first, because that is the window's own order** — the reference loads on show and
    // saves on Okay, and driving Okay over a window nothing had loaded is what made an earlier cut
    // of this test read a slider floor back out of the CVar. It also makes the Okay run below
    // exercise a *populated* window, which is strictly the better coverage.
    s.run("this = UIOptionsFrameOkay UIOptionsFrame_Load()")
        .expect("_Load runs to completion — every slider CVar it reads is registered");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The four slider CVars, by the file's own table rather than by a list retyped here: every one
    // must answer, because a single nil among them puts the raise back.
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

    // …and the walk really did reach slider 3 rather than stopping short of it: the Mouse Look
    // Speed slider is sitting on `cameraYawMoveSpeed`'s registered 180. Slider 3 is the one that
    // matters — it is the row whose nil raised, forty lines into a walk of 69 check buttons.
    let slider3: f64 = s
        .eval("return UIOptionsFrameSlider3:GetValue()")
        .expect("the Mouse Look Speed slider's value");
    assert!(
        (slider3 - 180.0).abs() < 0.001,
        "slider 3 should carry cameraYawMoveSpeed's registered 180, got {slider3}"
    );

    // `_SetDefaults` is the same walk through `GetCVarDefault`, and it had the same raise.
    s.run("this = UIOptionsFrameDefaults UIOptionsFrame_SetDefaults()")
        .expect("_SetDefaults runs to completion too");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The Okay path, over the window `_Load` just populated. Driven through the reference's OWN
    // caller — `UIOptionsFrameOkay`'s `<OnClick>` (xml l.1205-1209) — rather than as a bare call,
    // and that is not ceremony: the `SHOW_PARTY_PETS` arm reaches `RefreshBuffs`, whose first act
    // is `this.hasDispellable = nil` (`BuffFrame.lua:266`), so the reference's ambient handler
    // global has to be set. It is, at every real call site: the Okay button here, and pfUI's own
    // checkbox on its GVAR path.
    s.run("UIOptionsFrameOkay:Click()")
        .expect("the stock Okay button's own handler");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return UIOptionsFrame:IsShown()").unwrap(),
        "…and the window it hides on the way out was hidden to begin with"
    );

    // **`_Save` writes the two move-speed CVars for real now**, which is the other half of the
    // wiring and was a silent no-op before they were registered: it writes the yaw slider's value
    // and, beside it, `cameraPitchMoveSpeed = value / 2` (lua l.355-356) — the relation the
    // reference's own registered defaults confirm, 180 and 90.
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

/// `assets/ui` does not grow. The interface is the stock 1.12 FrameXML executed off the player's
/// own patch chain; what is left here is the glue and the shims, named below. A new file means a
/// window was authored instead of migrated: point `benilla.toc` at
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
