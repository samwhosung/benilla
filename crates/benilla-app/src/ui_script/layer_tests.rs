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
    let before = format!("{DEFINE_LOG}\n{ADDON_LOADED_LOG}");
    production_load_with(tag, stock_ui, &before, |root| {
        let dir = root.join("ZZOrder");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ZZOrder.toc"), "## Interface: 11200\nprobe.lua\n").unwrap();
        std::fs::write(
            dir.join("probe.lua"),
            "ZZORDER_SAW_LAYER = BenillaOptionsFrame ~= nil\n",
        )
        .unwrap();
    })
}

/// The production in-game load, the layer on unless `stock_ui`, over a hermetic AddOns root that
/// `addons` fills; `before` runs ahead of the load. The caller holds [`ENV_LOCK`]. Returns the VM
/// and the load's failures.
pub(super) fn production_load_with(
    tag: &str,
    stock_ui: bool,
    before: &str,
    addons: impl FnOnce(&std::path::Path),
) -> (UiScript, Vec<String>) {
    let tmp = std::env::temp_dir().join(format!("benilla-layer-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let home = tmp.join("benilla-config");
    let root = home.join("AddOns");
    std::fs::create_dir_all(&root).unwrap();
    addons(&root);
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
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            ..Default::default()
        }),
    );
    // A reply that hid nothing, so `GetNumAddOns` counts the registry.
    s.note_addon_info_reply(&[]);
    s.register_cvars(crate::cvars::registered_pairs());
    s.run(before).unwrap();
    // The layer passed in, not set through `WOW_STOCK_UI`: every test in this process reads it.
    let mut failures = Vec::new();
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

    // The core's last row that defines a global (RaidWarning.xml; ClassTrainerFrameTemplates.xml
    // declares templates alone), the layer's first file (ScrollTemplates.xml) and last
    // (FrameXMLFixes.xml is all redefinitions, so ScriptLogFrame.xml), then the addon.
    let order = [
        "RaidWarningFrame_OnLoad",
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
    let late: Vec<&String> = log[at[1]..]
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

/// Booted without the layer, as `WOW_STOCK_UI=1` does: no layer file loads and the stock files
/// raise nothing.
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
        "BenillaKeyBindings_OnKeyDown",
        "BenillaOptionsFrame_SelectCategory",
        "BenillaGameMenuButtonEditMode",
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
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Types `line` into the chat box and sends it through the stock `ChatEdit_ParseText`
/// (`ChatFrame.lua:2087`), counting the `HELP_TEXT_SIMPLE` lines it prints.
fn type_line(s: &UiScript, line: &str) -> i64 {
    s.run(&format!(
        r#"HELPS = 0
           local box = ChatFrameEditBox
           box.chatFrame = box.chatFrame or DEFAULT_CHAT_FRAME
           local frame = box.chatFrame
           local add = frame.AddMessage
           frame.AddMessage = function(f, text, r, g, b, id)
               if text == HELP_TEXT_SIMPLE then HELPS = HELPS + 1 end
               return add(f, text, r, g, b, id)
           end
           box:SetText({line:?})
           ChatEdit_ParseText(box, 1)
           frame.AddMessage = add"#
    ))
    .unwrap_or_else(|e| panic!("{line}: {e}"));
    s.eval("return HELPS").unwrap()
}

/// The layer's player commands are plain `SlashCmdList` rows: `/reload` runs `ReloadUI()`,
/// `/errors` (`/err`) toggles the log and `/errors clear` empties it. 1.12 has no `/convertraid`
/// (the Raid tab's button converts, `RaidFrame.lua:63-71`), so it answers `HELP_TEXT_SIMPLE`, and
/// the forwarding hook `SubmitChatInput` is gone.
#[test]
fn the_layers_commands_are_slash_rows_and_convertraid_is_unknown() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, failures) = production_load("slash", false);
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    assert!(s.eval::<bool>("return SubmitChatInput == nil").unwrap());

    s.take_session_requests();
    assert_eq!(type_line(&s, "/reload"), 0);
    assert_eq!(
        s.take_session_requests(),
        [benilla_ui::script::SessionRequest::ReloadUi],
        "/reload is ReloadUI()"
    );
    assert!(
        s.take_chat_input().is_empty(),
        "nothing is handed to the host"
    );

    assert_eq!(type_line(&s, "/errors"), 0);
    assert!(s
        .eval::<bool>("return BenillaScriptLogFrame:IsVisible()")
        .unwrap());
    assert_eq!(type_line(&s, "/err"), 0);
    assert!(!s
        .eval::<bool>("return BenillaScriptLogFrame:IsVisible()")
        .unwrap());
    s.run("BenillaScriptLog_Record('kept')").unwrap();
    assert_eq!(type_line(&s, "/errors  clear "), 0);
    assert_eq!(
        s.eval::<i64>("return table.getn(BenillaScriptLog.rows)")
            .unwrap(),
        0,
        "/errors clear empties the log"
    );

    assert_eq!(type_line(&s, "/convertraid"), 1, "the unknown-command line");
    assert!(s.take_chat_input().is_empty());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// In a player build the dev instruments are no `SlashCmdList` row, so a typed one answers
/// `HELP_TEXT_SIMPLE` once and hands the host nothing to re-run; in a dev build each is a host row
/// that hands the host its line.
#[test]
fn the_dev_commands_are_host_rows_in_a_dev_build_alone() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let lines = [
        "/chattest",
        "/shot",
        "/liquid",
        "/castvis",
        "/partytest ping",
        "/reaction",
        "/react Probe",
    ];

    let (mut s, failures) = production_load("devcmds-player", false);
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    crate::ui_chat::commands::register_dev_commands(&s, false);
    for line in lines {
        assert_eq!(
            type_line(&s, line),
            1,
            "{line}: the unknown-command line, once"
        );
        assert!(
            s.take_chat_input().is_empty(),
            "{line}: nothing queued to loop"
        );
        let cmd = line[1..].split(' ').next().unwrap();
        assert!(!s.has_slash_command(cmd), "{cmd} is no SlashCmdList row");
    }
    drop(s);

    let (mut s, failures) = production_load("devcmds-dev", false);
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    crate::ui_chat::commands::register_dev_commands(&s, true);
    for line in lines {
        assert_eq!(type_line(&s, line), 0, "{line}");
        let queued = s.take_chat_input();
        assert_eq!(queued.len(), 1, "{line}: {queued:?}");
        // An alias queues under the command's first one, as the row's handler knows only that.
        let want = line.replacen("/react ", "/reaction ", 1);
        assert_eq!(queued[0].trim(), want, "{line}");
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The `/errors` log collects off the Lua error handler, ahead of the stock `_ERRORMESSAGE`
/// (`BasicControls.xml:16`), which still shows the `ScriptErrors` dialog: an addon's runtime error,
/// an error in one of the layer's own handlers, and one raised in the stock load before the layer
/// loaded (the engine hands caught errors to the handler after the load).
#[test]
fn the_error_log_collects_off_the_error_handler() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let before = r#"
        local pre = CreateFrame("Button", "ZZPreLayer")
        pre:SetScript("OnClick", function() error("before the layer") end)
        pre:Click()
    "#;
    let (mut s, failures) = production_load_with("errlog", false, before, |root| {
        let dir = root.join("ZZErr");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ZZErr.toc"), "## Interface: 11200\nerr.lua\n").unwrap();
        std::fs::write(
            dir.join("err.lua"),
            "ZZErrButton = CreateFrame('Button', 'ZZErrButton')\n\
             ZZErrButton:SetScript('OnClick', function() error('an addon boom') end)\n",
        )
        .unwrap();
    });
    assert!(
        failures.iter().all(|f| f.contains("before the layer")),
        "load failures: {failures:#?}"
    );
    s.run("ZZErrButton:Click()").unwrap();
    // A layer handler raising: the window's OnShow calls this.
    s.run("BenillaOptionsFrame_UpdateScale = function() error('a layer boom') end")
        .unwrap();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.dispatch_script_errors_to_handler();

    let rows: Vec<String> = s
        .eval(
            "local out = {} \
             for _, row in ipairs(BenillaScriptLog.rows) do table.insert(out, row.message) end \
             return out",
        )
        .unwrap();
    for want in ["before the layer", "an addon boom", "a layer boom"] {
        assert!(
            rows.iter().any(|r| r.contains(want)),
            "{want:?} is in the log: {rows:#?}"
        );
    }
    assert!(
        s.eval::<bool>("return ScriptErrors:IsVisible()").unwrap(),
        "the stock handler still runs after the log"
    );
    let shown: String = s.eval("return ScriptErrors_Message:GetText()").unwrap();
    assert!(
        shown.contains("before the layer"),
        "the first error shows: {shown}"
    );
    // A repeat is one row with a count.
    s.run("ZZErrButton:Click() ZZErrButton:Click()").unwrap();
    s.dispatch_script_errors_to_handler();
    assert_eq!(
        s.eval::<i64>(
            "for _, row in ipairs(BenillaScriptLog.rows) do \
                if strfind(row.message, 'an addon boom', 1, 1) then return row.count end \
             end"
        )
        .unwrap(),
        3
    );
}

/// Every global the layer defines takes the `Benilla` prefix (a slash alias its `SLASH_BENILLA_`
/// form, as `ChatEdit_ParseText` reads it), so no layer name meets a stock or an addon one; a stock
/// global the layer redefines keeps its name and is not a new definition here.
#[test]
fn every_global_the_layer_defines_takes_the_benilla_prefix() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (s, failures) = production_load("prefix", false);
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    let log: Vec<String> = s.eval("return DEFINE_LOG").unwrap();
    let start = defined_at(&log, "BenillaScrollBar_Step");
    let end = defined_at(&log, "ZZORDER_SAW_LAYER");
    let dispatch = |n: &str| {
        n == "this"
            || n == "event"
            || n.strip_prefix("arg")
                .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
    };
    let unprefixed: Vec<&String> = log[start..end]
        .iter()
        .filter(|n| {
            !(n.starts_with("Benilla")
                || n.starts_with("BENILLA_")
                || n.starts_with("SLASH_BENILLA_"))
                && !dispatch(n)
        })
        .collect();
    assert!(
        unprefixed.is_empty(),
        "layer globals without the Benilla prefix: {unprefixed:?}"
    );
}
