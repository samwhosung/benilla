//! The glue screens' Lua surface (`reference/glue_surface.tsv`) held to its source, the shapes
//! table, and the glue VM ([`UiScript::new_glue`]) held to the surface: every function it puts in
//! `_G` is a glue-state one, and every glue-state function is there or on [`NOT_YET`]. Then the glue
//! state's frame types and event table.

use std::collections::{BTreeMap, BTreeSet};

use crate::script::{glue_events, UiScript};
use crate::widget::KindState;

/// A vendored `reference/` table's data rows, split on tabs, the header row dropped.
fn reference_rows(file: &str, header: &str) -> Vec<Vec<String>> {
    let path = format!("{}/../../reference/{file}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.split('\t').map(str::to_string).collect::<Vec<_>>())
        .filter(|f| f[0] != header)
        .collect()
}

/// One row of the glue surface table.
pub(super) struct GlueRow {
    pub scope: String,
    pub name: String,
    pub fn_va: String,
    pub table_va: String,
    pub arity: String,
}

/// `reference/glue_surface.tsv`.
pub(super) fn glue_surface() -> Vec<GlueRow> {
    let rows: Vec<GlueRow> = reference_rows("glue_surface.tsv", "name")
        .into_iter()
        .map(|f| GlueRow {
            name: f[0].clone(),
            scope: f[1].clone(),
            fn_va: f[3].clone(),
            table_va: f[4].clone(),
            arity: f[5].clone(),
        })
        .collect();
    assert!(
        rows.len() > 150,
        "the glue surface table looks truncated ({} rows)",
        rows.len()
    );
    rows
}

/// The tables the glue state installs, as the glue registrar `0x46abb0` and the Lua init
/// `0x7039e0` loop them: each `cmp` bound over the 8-byte pair stride.
const GLUE_TABLES: [(&str, usize); 8] = [
    ("0x811e28", 36), // the base array, 0x7039e0
    ("0x8503f4", 6),  // 0x51bb20
    ("0x872e74", 4),  // 0x706350
    ("0x8373b8", 60), // 0x46ddf0
    ("0x838578", 11), // 0x4736c0
    ("0x8380f8", 24), // 0x471a20
    ("0x837ea0", 10), // 0x46f480
    ("0x835a50", 4),  // 0x4587f0
];

/// Each registrar table's rows are exactly the shapes table's rows for that table, binding and
/// arity alike; a compat alias names a library binding the shapes table has.
#[test]
fn the_glue_surface_matches_the_shapes_table() {
    // (table_va, name) -> (fn, arity): a name is not a key in the shapes table.
    let shapes: BTreeMap<(String, String), (String, String)> =
        reference_rows("bindings.tsv", "name")
            .into_iter()
            .map(|f| ((f[3].clone(), f[0].clone()), (f[1].clone(), f[7].clone())))
            .collect();
    let surface = glue_surface();

    for (table_va, rows) in GLUE_TABLES {
        let in_shapes: BTreeSet<&str> = shapes
            .keys()
            .filter(|(t, _)| t == table_va)
            .map(|(_, n)| n.as_str())
            .collect();
        let in_surface: BTreeSet<&str> = surface
            .iter()
            .filter(|r| r.table_va == table_va)
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(
            in_shapes.len(),
            rows,
            "the shapes table's {table_va} has {} rows",
            in_shapes.len()
        );
        assert_eq!(
            in_surface, in_shapes,
            "table {table_va}: the glue surface and the shapes table differ"
        );
    }
    for r in &surface {
        if r.fn_va == "lua" {
            continue;
        }
        if r.table_va.is_empty() {
            // `__pow`, set by `luaopen_math` outside any table.
            assert_eq!(r.name, "__pow", "{} has no table", r.name);
            continue;
        }
        let shape = shapes
            .iter()
            .find(|((t, _), (f, _))| *t == r.table_va && *f == r.fn_va)
            .map(|(_, s)| s);
        assert_eq!(
            shape.map(|(f, a)| (f.as_str(), a.as_str())),
            Some((r.fn_va.as_str(), r.arity.as_str())),
            "{}: no shapes row in table {} with that binding and arity",
            r.name,
            r.table_va
        );
    }
    let names: BTreeSet<&str> = surface.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names.len(), surface.len(), "a name appears twice");
    // 36 base-array rows, the compat chunk's 39 functions and `__pow`; 6 + 4 + 4 shared; the four
    // glue tables' 105.
    for (scope, n) in [("base", 76), ("shared", 14), ("glue", 105)] {
        assert_eq!(
            surface.iter().filter(|r| r.scope == scope).count(),
            n,
            "{scope} rows"
        );
    }
}

/// Glue-state functions the glue VM does not answer yet, by registrar table, each with why. The
/// list only shrinks: a verb is absent until built, never stubbed.
const NOT_YET: &[(&str, &[&str])] = &[
    (
        "0x51bb20's file trio, each only `luaL_error(\"Access Denied\")`; unbuilt in the in-game \
         VM too, tracked by #338",
        &["ReadFile", "DeleteFile", "AppendToFile"],
    ),
    (
        "the glue core (0x46ddf0, table 0x8373b8): the movie's setter, the \
         patch and survey and the billing; each a binding of its own, the ones \
         with an in-game namesake included",
        &[
            "GetBillingPlan",
            "GetBillingTimeRemaining",
            "GetBillingTimeRested",
            "PINEntered",
            "PatchDownloadApply",
            "PatchDownloadCancel",
            "PatchDownloadProgress",
            "SetMovieSubtitles",
            "SurveyNotificationDone",
        ],
    ),
];

/// The glue VM's globals by Lua type name.
fn glue_globals() -> BTreeMap<String, String> {
    UiScript::new_glue()
        .expect("construct the glue VM")
        .eval::<Vec<Vec<String>>>(
            "local out = {} \
             for k, v in pairs(getfenv(0)) do \
               if type(k) == 'string' then table.insert(out, {k, type(v)}) end \
             end \
             return out",
        )
        .expect("dump _G")
        .into_iter()
        .map(|kv| (kv[0].clone(), kv[1].clone()))
        .collect()
}

/// The glue VM's functions are the glue surface less [`NOT_YET`], exactly.
#[test]
fn the_glue_vm_answers_the_glue_surface() {
    let surface: BTreeSet<String> = glue_surface().into_iter().map(|r| r.name).collect();
    let not_yet: BTreeSet<&str> = NOT_YET
        .iter()
        .flat_map(|(_, n)| n.iter().copied())
        .collect();
    let ours: BTreeSet<String> = glue_globals()
        .into_iter()
        .filter(|(_, t)| t == "function")
        .map(|(k, _)| k)
        .collect();

    let beyond: Vec<&String> = ours.iter().filter(|n| !surface.contains(*n)).collect();
    assert!(
        beyond.is_empty(),
        "the glue VM exposes {} function(s) the glue state does not have: {beyond:?}. The \
         in-game surface stays out of it (`UiScript::new` installs that).",
        beyond.len()
    );
    let missing: Vec<&String> = surface
        .iter()
        .filter(|n| !ours.contains(*n) && !not_yet.contains(n.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "glue-state functions the glue VM lacks and NOT_YET does not name: {missing:?}"
    );
    let stale: Vec<&&str> = not_yet
        .iter()
        .filter(|n| ours.contains(**n) || !surface.contains(**n))
        .collect();
    assert!(
        stale.is_empty(),
        "NOT_YET names that are built, or that are not glue-state functions; take them off: \
         {stale:?}"
    );
    assert_eq!(
        not_yet.len(),
        NOT_YET.iter().map(|(_, n)| n.len()).sum::<usize>(),
        "a NOT_YET name is listed twice"
    );
}

/// Past its functions the glue `_G` holds the four library tables, the frame metatable
/// `0x7039e0` publishes, and the compat chunk's `PI`; `_G` itself is benilla's, as in-game
/// (`reference_surface::BEYOND_1_12`).
#[test]
fn the_glue_vm_holds_no_other_global() {
    let others: BTreeMap<String, String> = glue_globals()
        .into_iter()
        .filter(|(_, t)| t != "function")
        .collect();
    let want: BTreeMap<String, String> = [
        ("PI", "number"),
        ("__framescript_meta", "table"),
        ("_G", "table"),
        ("bit", "table"),
        ("math", "table"),
        ("string", "table"),
        ("table", "table"),
    ]
    .into_iter()
    .map(|(k, t)| (k.to_string(), t.to_string()))
    .collect();
    assert_eq!(others, want);
}

/// The glue event table the VM matches against is `reference/glue_events.tsv`, by id.
#[test]
fn the_glue_event_table_matches_the_reference() {
    let rows = reference_rows("glue_events.tsv", "id");
    // 1.12.1's glue screens name 26 events; a PIN bump that changes the count is read here.
    assert_eq!(glue_events().len(), 26);
    assert_eq!(rows.len(), glue_events().len());
    for (id, row) in rows.iter().enumerate() {
        assert_eq!(row[0], id.to_string(), "ids run 0..26 in order");
        assert_eq!(row[1], glue_events()[id], "event {id}");
    }
}

/// `RegisterEvent` matches a name against the glue table case-insensitively (`0x702140` with
/// `0x64a4c0`) and drops one the table lacks without raising; `UnregisterEvent` matches the same
/// way (`0x702280`). An in-game event fired at the glue VM reaches nothing, an all-events frame
/// included, since the reference fires by id.
#[test]
fn glue_events_register_by_the_glue_table() {
    let mut s = UiScript::new_glue().unwrap();
    s.run(
        r#"
        heard = {}
        f = CreateFrame("Frame")
        f:SetScript("OnEvent", function() table.insert(heard, "f:" .. event) end)
        f:RegisterEvent("set_glue_screen")
        f:RegisterEvent("PLAYER_LOGIN")
        all = CreateFrame("Frame")
        all:SetScript("OnEvent", function() table.insert(heard, "all:" .. event) end)
        all:RegisterAllEvents()
        "#,
    )
    .expect("an event off the glue table raises nothing");
    s.fire_event("SET_GLUE_SCREEN", vec![]);
    s.queue_event("PLAYER_LOGIN", vec![]);
    // The per-frame advance runs over the glue VM, its queued events included.
    s.tick(0.016);
    assert_eq!(
        s.eval::<Vec<String>>("return heard").unwrap(),
        ["f:SET_GLUE_SCREEN", "all:SET_GLUE_SCREEN"],
        "the glue event reaches both frames under the table's spelling; the in-game one neither"
    );
    s.run(r#"heard = {} f:UnregisterEvent("Set_Glue_Screen")"#)
        .unwrap();
    s.fire_event("SET_GLUE_SCREEN", vec![]);
    assert_eq!(
        s.eval::<Vec<String>>("return heard").unwrap(),
        ["all:SET_GLUE_SCREEN"]
    );
}

/// `Screenshot` (`0x46d3d0`, table `0x8373b8`) arms a capture, and its callback `0x46c520` answers
/// with glue event 14 on success and 15 on failure, the glue table's own names
/// (`reference/glue_events.tsv`); the in-game pair would reach no glue frame.
#[test]
fn the_glue_screenshot_arms_a_capture_and_is_answered_with_the_glue_events() {
    let mut s = UiScript::new_glue().unwrap();
    assert_eq!(glue_events()[14], "GLUE_SCREENSHOT_SUCCEEDED");
    assert_eq!(glue_events()[15], "GLUE_SCREENSHOT_FAILED");
    s.run(
        r#"
        heard = {}
        f = CreateFrame("Frame")
        f:SetScript("OnEvent", function() table.insert(heard, event) end)
        f:RegisterEvent("GLUE_SCREENSHOT_SUCCEEDED")
        f:RegisterEvent("GLUE_SCREENSHOT_FAILED")
        f:RegisterEvent("SCREENSHOT_SUCCEEDED")
        Screenshot()
        "#,
    )
    .expect("the glue VM binds Screenshot");
    assert_eq!(s.take_screenshot_asks(), 1, "one call, one capture");
    assert_eq!(s.take_screenshot_asks(), 0, "and the ask is spent");
    assert!(
        s.eval::<Vec<String>>("return heard").unwrap().is_empty(),
        "the call itself answers nothing"
    );
    let e = s.screenshot_event(true);
    s.fire_event(e, vec![]);
    let e = s.screenshot_event(false);
    s.fire_event(e, vec![]);
    s.fire_event("SCREENSHOT_SUCCEEDED", vec![]);
    assert_eq!(
        s.eval::<Vec<String>>("return heard").unwrap(),
        ["GLUE_SCREENSHOT_SUCCEEDED", "GLUE_SCREENSHOT_FAILED"],
        "the glue names, and neither in-game one"
    );
}

/// The in-game state answers with its own names.
#[test]
fn the_in_game_screenshot_is_answered_with_the_in_game_events() {
    let mut s = UiScript::new().unwrap();
    s.run(
        r#"
        heard = {}
        f = CreateFrame("Frame")
        f:SetScript("OnEvent", function() table.insert(heard, event) end)
        f:RegisterEvent("SCREENSHOT_SUCCEEDED")
        f:RegisterEvent("SCREENSHOT_FAILED")
        "#,
    )
    .unwrap();
    let e = s.screenshot_event(true);
    s.fire_event(e, vec![]);
    let e = s.screenshot_event(false);
    s.fire_event(e, vec![]);
    assert_eq!(
        s.eval::<Vec<String>>("return heard").unwrap(),
        ["SCREENSHOT_SUCCEEDED", "SCREENSHOT_FAILED"]
    );
}

/// The in-game VM registers by its own event table (`0x48feab`): an in-game event registers and
/// fires there.
#[test]
fn the_in_game_vm_keeps_its_events() {
    let mut s = UiScript::new().unwrap();
    s.run(
        r#"heard = 0 f = CreateFrame("Frame") f:RegisterEvent("PLAYER_LOGIN")
           f:SetScript("OnEvent", function() heard = heard + 1 end)"#,
    )
    .unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert_eq!(s.eval::<i64>("return heard").unwrap(), 1);
}

/// The frame types: the 13 registered at startup (`0x6ef070`) and `ModelFFX` (`0x46a4fe`) in both
/// states; `UI_Init`'s eight (`0x495940`) only in-game.
#[test]
fn the_glue_vm_makes_the_startup_frame_types_only() {
    const STARTUP: [&str; 14] = [
        "Button",
        "CheckButton",
        "EditBox",
        "Frame",
        "MessageFrame",
        "Model",
        "ScrollFrame",
        "ScrollingMessageFrame",
        "Slider",
        "SimpleHTML",
        "StatusBar",
        "ColorSelect",
        "MovieFrame",
        "ModelFFX",
    ];
    const IN_GAME: [&str; 8] = [
        "WorldFrame",
        "GameTooltip",
        "Minimap",
        "PlayerModel",
        "DressUpModel",
        "TabardModel",
        "LootButton",
        "TaxiRouteFrame",
    ];
    let glue = UiScript::new_glue().unwrap();
    let world = UiScript::new().unwrap();
    for t in STARTUP {
        for (vm, s) in [("glue", &glue), ("in-game", &world)] {
            s.run(&format!(r#"CreateFrame("{t}")"#))
                .unwrap_or_else(|e| panic!("{vm} CreateFrame(\"{t}\"): {e}"));
        }
    }
    for t in IN_GAME {
        let e = glue
            .run(&format!(r#"CreateFrame("{t}")"#))
            .expect_err(t)
            .to_string();
        assert!(e.contains(&format!("unknown frame type '{t}'")), "{t}: {e}");
        world.run(&format!(r#"CreateFrame("{t}")"#)).unwrap();
    }
}

/// `ModelFFX` is a `Model` to Lua (`0x470030`, `0x46ffd0`) with `Model`'s methods, flagged FFX for
/// the renderer; its XML is `Model`'s (`0x76cac0`), `file=` and `scale=` included.
#[test]
fn model_ffx_is_a_model_flagged_ffx() {
    let s = UiScript::new_glue().unwrap();
    s.run(
        r#"
        a = CreateFrame("ModelFFX", "FfxPane")
        b = CreateFrame("Model", "PlainPane")
        a:SetModel("Test/Scene.mdx")
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(String, bool, bool, String)>(
            r#"return a:GetObjectType(), a:IsObjectType("Model") == 1,
                      a:IsObjectType("ModelFFX") == 1, a:GetModel()"#
        )
        .unwrap(),
        (
            "Model".to_string(),
            true,
            false,
            "Test/Scene.mdx".to_string()
        )
    );
    let ffx = |name: &str| {
        let model = s.model_ref();
        let h = model.arena.lookup(name).unwrap();
        match &model.arena.frame(h).unwrap().kind_state {
            KindState::Model(m) => m.ffx,
            other => panic!("{name}: {other:?}"),
        }
    };
    assert!(ffx("FfxPane"));
    assert!(!ffx("PlainPane"));

    let doc = crate::framexml::parse(
        r#"<Ui>
            <ModelFFX name="XmlFfx" file="Test/Xml.mdx" scale="2"/>
            <PlayerModel name="XmlPlayer"/>
        </Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w == "Unknown frame type: PlayerModel"),
        "the glue state has no PlayerModel factory: {:?}",
        report.warnings
    );
    assert!(ffx("XmlFfx"));
    assert_eq!(
        s.eval::<(String, f64)>("return XmlFfx:GetModel(), XmlFfx:GetModelScale()")
            .unwrap(),
        ("Test/Xml.mdx".to_string(), 2.0)
    );
}

/// The glue's screen verbs answer the screen root with no UI scale (`0x46dd30`, `0x46dd80`): the
/// in-game pair divides by `UIParent`'s scale (`0x494590`), the glue pair has none to divide by,
/// even when a frame is named `UIParent`. The cursor is the root's (`0x46dad0`).
#[test]
fn the_glue_screen_verbs_answer_the_root_unscaled() {
    let root = (768.0_f32 * 16.0 / 9.0, 768.0_f32);
    let read = |mut s: UiScript| {
        s.set_screen_size(root.0, root.1);
        s.run(r#"local p = CreateFrame("Frame", "UIParent") p:SetScale(0.5)"#)
            .unwrap();
        s.mouse_move(100.0, 200.0);
        s.eval::<(f32, f32, f32, f32)>(
            "local x, y = GetCursorPosition() return GetScreenWidth(), GetScreenHeight(), x, y",
        )
        .unwrap()
    };
    assert_eq!(
        read(UiScript::new_glue().unwrap()),
        (root.0, root.1, 100.0, 200.0)
    );
    assert_eq!(
        read(UiScript::new().unwrap()),
        (root.0 * 2.0, root.1 * 2.0, 100.0, 200.0),
        "the in-game pair answers in UIParent's units"
    );
}
