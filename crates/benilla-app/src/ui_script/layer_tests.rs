//! benilla's layer ([`super::manifest::LAYER_MANIFEST`]) loads as more of the stock load: after
//! every core file, before the first third-party addon, with no `ADDON_LOADED` and no AddOn API
//! row, as 1.12.1 loads FrameXML (`UI_Init 0x48fbf0`: `FrameXML.toc` at `0x48ffed`, the addons at
//! `0x4900a3`, only the latter through `AddOn_Load`); and a dev build boots without it.

use benilla_ui::script::UiScript;

use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

/// Records each global's first definition in order: `__newindex` fires only for a new key, so a
/// file that redefines a stock function is not logged, and one that loads stock code is.
const DEFINE_LOG: &str = r#"
DEFINE_LOG = {}
setmetatable(getfenv(0), { __newindex = function(t, k, v)
    table.insert(DEFINE_LOG, k)
    rawset(t, k, v)
end })
"#;

/// Records every `ADDON_LOADED` from before the load.
const ADDON_LOADED_LOG: &str = r#"
ADDON_LOADED_LOG = {}
local f = CreateFrame("Frame")
f:RegisterEvent("ADDON_LOADED")
f:SetScript("OnEvent", function() table.insert(ADDON_LOADED_LOG, arg1) end)
"#;

/// A VM after the production in-game load, with one third-party addon, `ZZOrder`, in a hermetic
/// AddOns root; the layer on unless `stock_ui`. Returns the VM and the load's failures.
fn production_load(tag: &str, stock_ui: bool) -> (UiScript, Vec<String>) {
    let tmp = std::env::temp_dir().join(format!("benilla-layer-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let home = tmp.join("benilla-config");
    let dir = home.join("AddOns").join("ZZOrder");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ZZOrder.toc"), "## Interface: 11200\nprobe.lua\n").unwrap();
    std::fs::write(
        dir.join("probe.lua"),
        "ZZORDER_SAW_LAYER = BenillaOptionsFrame ~= nil\n",
    )
    .unwrap();
    let _c = EnvGuard::unset("WOW_CAPTURE");
    let _h = EnvGuard::set("BENILLA_HOME", home.to_str().unwrap());

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probesix".into()),
            level: 60,
            ..Default::default()
        }),
    );
    // A reply that hid nothing, so `GetNumAddOns` counts the registry.
    s.note_addon_info_reply(&[]);
    s.register_cvars(crate::cvars::registered_pairs());
    let mut failures = super::load_font_registry(&s);
    s.run(DEFINE_LOG).unwrap();
    s.run(ADDON_LOADED_LOG).unwrap();
    // The layer passed in, not set through `WOW_STOCK_UI`: every test in this process reads it.
    failures.extend(super::manifest::load_ingame_ui_with(
        &mut s,
        None,
        &[],
        true,
        !stock_ui,
    ));
    failures.extend(s.errors());
    let _ = std::fs::remove_dir_all(&tmp);
    (s, failures)
}

/// Where `name` was first defined in [`DEFINE_LOG`].
fn defined_at(log: &[String], name: &str) -> usize {
    log.iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("{name} was never defined in the load"))
}

/// The stock FrameXML's own globals, from the captured 1.12.1 `_G` (`reference/1.12-globals.tsv`):
/// the `framexml` rows a FrameXML file defines at load, LoadOnDemand ones aside.
fn stock_framexml_globals() -> std::collections::HashSet<String> {
    let tsv = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/1.12-globals.tsv"
    ));
    tsv.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut cols = l.split('\t');
            let (name, kind, origin) = (cols.next()?, cols.next()?, cols.next()?);
            (origin == "framexml" && kind != "lod").then(|| name.to_string())
        })
        .collect()
}

/// The layer loads after every stock file and before the first third-party addon: the core's last
/// rows define before the layer's first, the layer's last before the addon, and nothing first
/// defined from the layer's start on is a stock FrameXML global.
#[test]
fn the_layer_loads_after_every_stock_file_and_before_the_addons() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (s, failures) = production_load("order", false);
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    let log: Vec<String> = s.eval("return DEFINE_LOG").unwrap();

    // The core's last stock row (TutorialFrame.lua), the core's own last row (GameMenuFrame.xml),
    // the layer's first file (ScrollTemplates.xml) and last (FrameXMLFixes.xml is all
    // redefinitions, so ScriptLogFrame.xml), then the addon.
    let order = [
        "TutorialFrame_OnHide",
        "GameMenuButton_Pending",
        "BenillaScrollBar_Step",
        "BenillaScriptLog_Toggle",
        "ZZORDER_SAW_LAYER",
    ];
    let at: Vec<usize> = order.iter().map(|n| defined_at(&log, n)).collect();
    for w in 0..order.len() - 1 {
        assert!(
            at[w] < at[w + 1],
            "{} (#{}) must be defined before {} (#{})",
            order[w],
            at[w],
            order[w + 1],
            at[w + 1]
        );
    }
    let stock = stock_framexml_globals();
    assert!(stock.len() > 5000, "the reference's _G parse is broken");
    // The engine sets `this`, `event` and `arg1`-`arg9` around each handler it runs.
    let dispatch = |n: &str| {
        n == "this"
            || n == "event"
            || n.strip_prefix("arg")
                .is_some_and(|d| d.len() == 1 && d.as_bytes()[0].is_ascii_digit())
    };
    let late: Vec<&String> = log[at[2]..]
        .iter()
        .filter(|n| stock.contains(n.as_str()) && !dispatch(n))
        .collect();
    assert!(
        late.is_empty(),
        "stock FrameXML globals first defined after the layer began — a stock file loaded inside \
         or after the layer: {late:?}"
    );
    assert!(
        s.eval::<bool>("return ZZORDER_SAW_LAYER").unwrap(),
        "the addon's file scope sees the layer"
    );
}

/// The layer is no addon: no `ADDON_LOADED` of its own, no row in the AddOn API, and no row on the
/// character-select AddOns screen.
#[test]
fn the_layer_is_absent_from_the_addon_api() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (s, failures) = production_load("api", false);
    assert!(failures.is_empty(), "load failures: {failures:#?}");

    let loaded: Vec<String> = s.eval("return ADDON_LOADED_LOG").unwrap();
    assert_eq!(
        loaded,
        ["ZZOrder"],
        "only the third-party addon announces itself"
    );

    let names: Vec<String> = s
        .eval(
            "local out = {} \
             for i = 1, GetNumAddOns() do table.insert(out, (GetAddOnInfo(i))) end \
             return out",
        )
        .unwrap();
    assert!(
        names.iter().any(|n| n == "ZZOrder"),
        "the probe sees the registry: {names:?}"
    );
    for name in ["benilla", "layer"] {
        assert!(
            !names.iter().any(|n| n.eq_ignore_ascii_case(name)),
            "{name} is a row in the AddOn API: {names:?}"
        );
        assert!(
            !s.eval::<bool>(&format!(
                "return IsAddOnLoaded(\"{name}\") and true or false"
            ))
            .unwrap(),
            "IsAddOnLoaded(\"{name}\") answers"
        );
    }
    assert!(
        !super::addons::installed_rows()
            .iter()
            .any(|a| a.name.eq_ignore_ascii_case("benilla")),
        "the AddOns screen lists benilla"
    );
}

/// `WOW_STOCK_UI=1` drops the layer in a dev build alone; a player build always loads it.
#[test]
fn the_stock_ui_switch_is_a_dev_builds_alone() {
    use super::manifest::layer_enabled_by;
    assert!(
        !layer_enabled_by(true, Some("1")),
        "a dev build boots without it"
    );
    assert!(
        layer_enabled_by(false, Some("1")),
        "a player build ignores the switch"
    );
    assert!(layer_enabled_by(true, None) && layer_enabled_by(true, Some("0")));
}

/// Booted without the layer, as `WOW_STOCK_UI=1` does: no layer file loads, the stock files raise
/// nothing, and the host's calls into the layer are no-ops.
#[test]
fn the_stock_ui_switch_boots_the_core_without_the_layer() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (s, failures) = production_load("stock", true);
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    let log: Vec<String> = s.eval("return DEFINE_LOG").unwrap();
    // One global from each layer file that defines a new one.
    for name in [
        "BenillaScrollBar_Step",
        "KeyBindings_OnHostKey",
        "BenillaOptionsFrame_SelectCategory",
        "BENILLA_BAG_WAS_OPEN",
        "BenillaScriptLog_Toggle",
    ] {
        assert!(
            !log.iter().any(|n| n == name),
            "{name} is defined: a layer file loaded under WOW_STOCK_UI=1"
        );
    }
    assert!(!s.eval::<bool>("return BenillaOptionsFrame ~= nil").unwrap());
    assert!(!s.eval::<bool>("return ZZORDER_SAW_LAYER").unwrap());
    // The host's calls into the layer, and the game menu's Options rung.
    for body in [
        super::ERRORS_TOGGLE.to_string(),
        super::ERRORS_CLEAR.to_string(),
        super::host_key_capture("CTRL-J"),
        "GameMenuButtonOptions:Click()".to_string(),
    ] {
        s.run(&body).unwrap_or_else(|e| panic!("{body}: {e}"));
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
