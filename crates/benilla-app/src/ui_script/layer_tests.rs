//! benilla's layer ([`super::manifest::LAYER_MANIFEST`]) loads as more of the stock load: after
//! every core file, before the first third-party addon, with no `ADDON_LOADED` and no AddOn API
//! row, as 1.12.1 loads FrameXML (`UI_Init 0x48fbf0`: `FrameXML.toc` at `0x48ffed`, the addons at
//! `0x4900a3`, only the latter through `AddOn_Load`); and a dev build boots without it.

use benilla_ui::script::{EditAction, EditUnit, QuadContent, ScriptValue, UiScript};

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

/// A login as the reference runs one on `s`, over a saved-variables file in `home` holding
/// `saved`: the chunk and `VARIABLES_LOADED` inside the UI load, then the world entry's clear
/// (`0x401639`, after the load at `0x401602`), then `PLAYER_ENTERING_WORLD`; a `ReloadUI()`
/// (`entry` false) skips the clear. The bits start as `before`. Returns the app holding the VM and
/// the plate bits.
fn log_in(
    s: UiScript,
    home: &std::path::Path,
    saved: Option<&str>,
    entry: bool,
    before: crate::vplates::VPlateMode,
) -> bevy::app::App {
    use bevy::prelude::*;
    std::fs::create_dir_all(home).unwrap();
    if let Some(saved) = saved {
        std::fs::write(home.join("saved-variables.lua"), saved).unwrap();
    }
    let mut app = App::new();
    app.add_systems(Update, crate::vplates::apply_plate_verbs)
        .insert_resource(before)
        .insert_non_send_resource(s);
    crate::ui_saved::load_saved_variables(
        &mut app.world_mut().non_send_resource_mut::<UiScript>(),
        |_| {},
    );
    app.update();
    if entry {
        crate::vplates::clear_at_world_entry(app.world_mut());
    }
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .fire_event("PLAYER_ENTERING_WORLD", vec![]);
    app.update();
    app
}

fn plates(app: &bevy::app::App) -> (bool, bool) {
    let m = app.world().resource::<crate::vplates::VPlateMode>();
    (m.enemies, m.friends)
}

fn plate_globals(app: &bevy::app::App) -> (Option<i64>, Option<i64>) {
    app.world()
        .non_send_resource::<UiScript>()
        .eval("return NAMEPLATES_ON, FRIENDNAMEPLATES_ON")
        .unwrap()
}

/// On the stock UI alone benilla is 1.12.1: the stock `UpdateNameplates` shows the friendly
/// plates and hides them at once (`UIOptionsFrame.lua:775-776`), so they are off after the entry
/// though the saved variable says on, and the saved variable is left as it was.
#[test]
fn on_the_stock_ui_friendly_plates_are_off_after_entry_and_the_saved_value_stands() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (s, failures) = production_load_with("plates-stock", true, "", |_| {});
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    let home = std::env::temp_dir().join(format!("benilla-plates-stock-{}", std::process::id()));
    let _c = EnvGuard::unset("WOW_CAPTURE");
    let _h = EnvGuard::set("BENILLA_HOME", home.to_str().unwrap());

    let app = log_in(
        s,
        &home,
        Some("NAMEPLATES_ON = 1\nFRIENDNAMEPLATES_ON = 1\n"),
        true,
        Default::default(),
    );
    assert_eq!(plates(&app), (true, false), "enemy on, friendly off");
    assert_eq!(
        plate_globals(&app),
        (Some(1), Some(1)),
        "both saved variables untouched"
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// With the layer, plates the player turned on stay on: after the entry, through the logout's
/// save, and after a `ReloadUI()` on the saved file.
#[test]
fn with_the_layer_plates_turned_on_stay_on_after_entry_and_reload() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (s, failures) = production_load_with("plates-layer", false, "", |_| {});
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    let home = std::env::temp_dir().join(format!("benilla-plates-layer-{}", std::process::id()));
    let _c = EnvGuard::unset("WOW_CAPTURE");
    let _h = EnvGuard::set("BENILLA_HOME", home.to_str().unwrap());

    let mut app = log_in(
        s,
        &home,
        Some("NAMEPLATES_ON = 1\nFRIENDNAMEPLATES_ON = 1\n"),
        true,
        Default::default(),
    );
    assert_eq!(plates(&app), (true, true), "both on after the entry");

    // The session's end writes the file, the friendly global included.
    crate::ui_saved::save(&mut app.world_mut().non_send_resource_mut::<UiScript>());
    let file = std::fs::read_to_string(home.join("saved-variables.lua")).unwrap();
    for line in ["NAMEPLATES_ON = 1", "FRIENDNAMEPLATES_ON = 1"] {
        assert!(file.contains(line), "{line} saved: {file}");
    }

    // `ReloadUI()`: a new VM over the file just written, and no clear.
    let before = *app.world().resource::<crate::vplates::VPlateMode>();
    let (s, failures) = production_load_with("plates-reload", false, "", |_| {});
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    let app = log_in(s, &home, None, false, before);
    assert_eq!(plates(&app), (true, true), "both on after the reload");
    assert_eq!(plate_globals(&app), (Some(1), Some(1)));
    let _ = std::fs::remove_dir_all(&home);
}

/// A frame's rect from its window's top-left corner, y down: `(left, top, right, bottom)`.
fn window_rect(s: &UiScript, window: &str, frame: &str) -> (f32, f32, f32, f32) {
    s.eval(&format!(
        "local w, f = {window}, {frame} \
         return f:GetLeft() - w:GetLeft(), w:GetTop() - f:GetTop(), \
                f:GetRight() - w:GetLeft(), w:GetTop() - f:GetBottom()"
    ))
    .unwrap_or_else(|e| panic!("{frame} in {window}: {e}"))
}

/// The `/errors` window wears the ClassTrainer art, so its frames sit where the class trainer's own
/// do on it (`Blizzard_TrainerUI.xml`, resized by `ClassTrainer_SetToClassTrainer`,
/// `Blizzard_TrainerUI.lua:469-475`): the rows, the list, the detail under it, and the two buttons
/// in the art's sockets (`ClassTrainerTrainButton` and `ClassTrainerCancelButton`, 80x22 centred
/// at 224 and 305, -420). The detail ends above the buttons, as the trainer's does at -407.
#[test]
fn the_error_log_lays_out_on_the_trainer_art_as_the_class_trainer_does() {
    benilla_formats::wow_data_or_skip!();
    let mut s = super::trainer_tests::trainer_script();
    // The file registers its commands in the stock table, which `ChatFrame.lua` declares and this
    // kit stops short of.
    s.run("SlashCmdList = {}").unwrap();
    super::test_ui::load_ui_strict(&s, "ScriptLogFrame.xml");

    // The class trainer's layout, read off its own frames once it has laid itself out.
    s.set_money(50);
    s.set_trainer(Some(super::trainer_tests::menu()));
    s.fire_event(
        "TRAINER_SHOW",
        vec![ScriptValue::Str("Sana Winterhoof".into())],
    );
    s.resolve();
    let trainer = |s: &UiScript, frame: &str| window_rect(s, "ClassTrainerFrame", frame);
    let ref_rows = s
        .eval::<i64>("return CLASS_TRAINER_SKILLS_DISPLAYED")
        .unwrap();
    let ref_list = trainer(&s, "ClassTrainerListScrollFrame");
    let ref_detail = trainer(&s, "ClassTrainerDetailScrollFrame");
    let ref_buttons = [
        trainer(&s, "ClassTrainerTrainButton"),
        trainer(&s, "ClassTrainerCancelButton"),
    ];
    let ref_rows_rects: Vec<_> = (1..=ref_rows)
        .map(|i| trainer(&s, &format!("ClassTrainerSkill{i}")))
        .collect();
    // The reference's own numbers, so a drifted harness cannot pass this against itself.
    assert_eq!(ref_rows, 11);
    assert_eq!(ref_list, (21.0, 96.0, 317.0, 280.0));
    assert_eq!(ref_detail, (21.0, 288.0, 317.0, 407.0));
    assert_eq!(ref_buttons[0], (184.0, 409.0, 264.0, 431.0));
    assert_eq!(ref_buttons[1], (265.0, 409.0, 345.0, 431.0));

    s.set_trainer(None);
    s.fire_event("TRAINER_CLOSED", vec![]);
    s.run("ShowUIPanel(BenillaScriptLogFrame)").unwrap();
    // Enough rows to fill the list, so every row shows.
    s.run("for i = 1, 30 do BenillaScriptLog_Record('error ' .. i) end")
        .unwrap();
    for _ in 0..4 {
        s.resolve();
        s.tick(0.016);
    }
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    let ours = |s: &UiScript, frame: &str| window_rect(s, "BenillaScriptLogFrame", frame);

    let window = ours(&s, "BenillaScriptLogFrame");
    assert_eq!(window, (0.0, 0.0, 384.0, 512.0), "the standard panel size");
    let clear = ours(&s, "BenillaScriptLogClearButton");
    let close = ours(&s, "BenillaScriptLogCloseBottomButton");
    let detail = ours(&s, "BenillaScriptLogDetailScroll");
    let list = ours(&s, "BenillaScriptLogListScrollFrame");

    // The buttons take the trainer's two sockets, at its size, inside the window's rect.
    assert_eq!(clear, ref_buttons[0], "Clear sits where Train does");
    assert_eq!(close, ref_buttons[1], "Close sits where Exit does");
    for (name, b) in [("Clear", clear), ("Close", close)] {
        assert!(
            b.0 >= window.0 && b.1 >= window.1 && b.2 <= window.2 && b.3 <= window.3,
            "{name} {b:?} lies inside the {window:?} window"
        );
    }
    // The detail pane is the trainer's, and its bottom clears both buttons' tops.
    assert_eq!(
        detail, ref_detail,
        "the detail sits where the trainer's does"
    );
    assert!(
        detail.3 < clear.1 && detail.3 < close.1,
        "the detail's bottom {} is above the buttons' top {}",
        detail.3,
        clear.1.min(close.1)
    );
    // The list is the trainer's, with its rows.
    assert_eq!(list, ref_list, "the list sits where the trainer's does");
    assert_eq!(
        s.eval::<i64>("return BENILLA_SCRIPTLOG_ROWS").unwrap(),
        ref_rows,
        "as many rows as the trainer's list"
    );
    assert!(
        s.eval::<bool>(&format!(
            "return getglobal('BenillaScriptLogRow{}') == nil",
            ref_rows + 1
        ))
        .unwrap(),
        "no row beyond the list"
    );
    for (i, want) in ref_rows_rects.iter().enumerate() {
        let row = ours(&s, &format!("BenillaScriptLogRow{}", i + 1));
        // The trainer widens a row to 323 while its scroll bar is hidden and keeps the template's
        // 293 beside it (`Blizzard_TrainerUI.lua:133-137`); the log's rows are the 293.
        assert_eq!(
            (row.0, row.1, row.2, row.3),
            (want.0, want.1, want.0 + 293.0, want.3),
            "row {} sits where the trainer's does",
            i + 1
        );
        assert!(
            row.1 >= list.1 && row.3 <= list.3,
            "row {} {row:?} lies inside the list {list:?}",
            i + 1
        );
    }
}

/// A traceback long enough to wrap the detail box and to pass any small `letters` cap, with a
/// newline and a lone `|` in it, which the box must hold byte for byte.
const LONG_ERROR: &str = "Interface/AddOns/Boom/Boom.lua:12: bad argument | #1\\nStack:\\n";

/// The `/errors` window open on the trainer's kit with `LONG_ERROR` logged and its row selected,
/// settled a few frames. Returns the script and the message as Lua holds it.
fn error_log_selected() -> (UiScript, String) {
    let mut s = super::trainer_tests::trainer_script();
    s.run("SlashCmdList = {}").unwrap();
    super::test_ui::load_ui_strict(&s, "ScriptLogFrame.xml");
    s.run("ShowUIPanel(BenillaScriptLogFrame)").unwrap();
    s.run(&format!(
        "MSG = \"{LONG_ERROR}\" .. string.rep(\"  [C]: in function `Boom'\\n\", 60)"
    ))
    .unwrap();
    s.run("BenillaScriptLog_Record(MSG)").unwrap();
    settle(&mut s);
    s.run("BenillaScriptLogRow1:Click()").unwrap();
    settle(&mut s);
    let msg = s.eval::<String>("return MSG").unwrap();
    assert!(msg.len() > 1500, "a message past any small letters cap");
    (s, msg)
}

/// A few frames: the layout resolves after Lua runs, and an edit's `OnTextChanged` fires on the tick.
fn settle(s: &mut UiScript) {
    for _ in 0..5 {
        s.resolve();
        s.tick(0.016);
    }
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A left click near the detail box's top left, inside its pane whatever the text's height, which
/// focuses it as the game's mouse does.
fn click_detail(s: &mut UiScript) {
    let (x, y) = s
        .eval::<(f64, f64)>(
            "return BenillaScriptLogDetailText:GetLeft() + 20, BenillaScriptLogDetailText:GetTop() - 8",
        )
        .unwrap();
    s.mouse_button(x as f32, y as f32, "LeftButton", true);
    s.mouse_button(x as f32, y as f32, "LeftButton", false);
    settle(s);
    assert_eq!(
        s.focused_editbox_name().as_deref(),
        Some("BenillaScriptLogDetailText"),
        "the click focused the detail box"
    );
}

fn detail_text(s: &UiScript) -> String {
    s.eval("return BenillaScriptLogDetailText:GetText()")
        .unwrap()
}

/// The kit's stand-in font (`trainer_script`'s `FixedWidthFont(7.0)`): every character this wide,
/// every wrapped line this tall.
const CHAR_W: f64 = 7.0;
const LINE_H: f64 = 12.0;
/// The detail box's `<TextInsets>` (left, right, top, bottom) and width, from the XML.
const DETAIL_INSETS: (f64, f64, f64, f64) = (6.0, 20.0, 6.0, 20.0);
const DETAIL_WIDTH: f64 = 296.0;

/// The height the reference gives a multi-line box (`0x77d4d0` @`0x77d8ad`): its insets plus its
/// text, wrapped at the box's width less its side insets, one line when short or empty.
fn detail_box_height(chars: usize) -> f64 {
    let wrap = DETAIL_WIDTH - (DETAIL_INSETS.0 + DETAIL_INSETS.1);
    let natural = chars as f64 * CHAR_W;
    let lines = if natural > wrap {
        (natural / wrap).ceil()
    } else {
        1.0
    };
    DETAIL_INSETS.2 + DETAIL_INSETS.3 + lines * LINE_H
}

fn detail_geometry(s: &UiScript) -> (f64, f64, f64, f64, f64) {
    s.eval(
        "return BenillaScriptLogDetailText:GetWidth(), BenillaScriptLogDetailText:GetHeight(), \
                BenillaScriptLogDetailScrollChild:GetHeight(), \
                BenillaScriptLogDetailScroll:GetHeight(), \
                BenillaScriptLogDetailScroll:GetVerticalScrollRange()",
    )
    .unwrap()
}

/// The selected error's text is in a multi-line EditBox, whole: no `letters` cap, no change to a
/// newline or a `|`. Opening the window takes no focus, so the movement keys keep working until a
/// click (the stock mail body's `autoFocus="false"`, `MailFrame.xml:616`). The box is as tall as
/// its insets and its text, so the scroll frame ranges over the whole traceback and shows its bar;
/// the text draws once, where the FontString drew it.
#[test]
fn the_error_logs_detail_is_a_box_holding_the_whole_error() {
    benilla_formats::wow_data_or_skip!();
    let (mut s, msg) = error_log_selected();
    assert_eq!(
        s.eval::<String>("return BenillaScriptLogDetailText:GetObjectType()")
            .unwrap(),
        "EditBox"
    );
    assert_eq!(detail_text(&s), msg, "the full message, traceback and all");
    assert_eq!(
        s.eval::<i64>("return BenillaScriptLogDetailText:GetMaxLetters()")
            .unwrap(),
        0,
        "no letters cap"
    );

    // Nothing took the keyboard on its own: a key with no box focused is not consumed.
    assert!(!s.has_keyboard_focus());
    assert!(!s.char_input("w"), "no autoFocus box waits for a key");
    assert!(!s.has_keyboard_focus());

    // The box sizes itself: insets plus the wrapped text, which is taller than the pane.
    let want = detail_box_height(msg.chars().count());
    let (width, height, child, pane, range) = detail_geometry(&s);
    assert_eq!(width, DETAIL_WIDTH);
    assert!(want > pane, "sanity: the text is taller than the pane");
    assert_eq!(height, want, "insets + lines * line height");
    // The scroll child stays the pane's size; the range is the box's overhang, 20 below the text.
    assert_eq!(child, pane, "the child is not sized to the text");
    assert_eq!(range, want - pane, "the range covers the whole traceback");
    assert!(
        s.eval::<bool>("return BenillaScriptLogDetailScrollBar:IsShown()")
            .unwrap(),
        "the bar shows for a range"
    );

    // The text draws once, at the 6-in, 6-down seat the FontString had.
    let (left, top) = s
        .eval::<(f64, f64)>(
            "return BenillaScriptLogDetailScroll:GetLeft(), BenillaScriptLogDetailScroll:GetTop()",
        )
        .unwrap();
    let drawn: Vec<_> = s
        .extract()
        .into_iter()
        .filter(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if *t == msg))
        .collect();
    assert_eq!(drawn.len(), 1, "the message draws once: {drawn:#?}");
    let rect = drawn[0].rect.expect("the drawn text has a rect");
    assert_eq!(
        (f64::from(rect.left), f64::from(rect.top)),
        (left + DETAIL_INSETS.0, top - DETAIL_INSETS.2),
        "6 in and 6 down from the pane's corner"
    );
    assert_eq!(
        f64::from(rect.right - rect.left),
        DETAIL_WIDTH - (DETAIL_INSETS.0 + DETAIL_INSETS.1),
        "wrapped at 270"
    );

    // The wheel over the box scrolls the pane, a step a notch.
    let (x, y) = (left + 40.0, top - 40.0);
    s.mouse_wheel(x as f32, y as f32, -1.0);
    assert_eq!(
        s.eval::<f64>("return BenillaScriptLogDetailScroll:GetVerticalScroll()")
            .unwrap(),
        20.0
    );

    // Another error's text resizes it: one short line, no range, no bar.
    s.run("BenillaScriptLog_Record('short') BenillaScriptLogRow2:Click()")
        .unwrap();
    settle(&mut s);
    assert_eq!(detail_text(&s), "short");
    let (_, height, child, pane, range) = detail_geometry(&s);
    assert_eq!(height, detail_box_height(5));
    assert_eq!(height, 38.0, "sanity: 6 + 12 + 20");
    assert_eq!((child, range), (pane, 0.0));
    assert!(
        !s.eval::<bool>("return BenillaScriptLogDetailScrollBar:IsShown()")
            .unwrap(),
        "no range, no bar"
    );
}

/// A click anywhere in the pane focuses the box and selects all of it, as a click on the text
/// does: below a short text the click lands on the scroll child, which hands focus to the box
/// (`SendMailScrollChildFrame`'s `OnMouseUp`, `MailFrame.xml:651`).
#[test]
fn a_click_below_the_detail_text_focuses_the_box_and_selects_it_all() {
    benilla_formats::wow_data_or_skip!();
    let (mut s, _) = error_log_selected();
    s.run("BenillaScriptLog_Record('short') BenillaScriptLogRow2:Click()")
        .unwrap();
    settle(&mut s);
    assert_eq!(detail_text(&s), "short");
    assert_eq!(
        detail_geometry(&s).1,
        38.0,
        "a box far shorter than the pane"
    );
    // The pane's foot, 6 up from its bottom edge: well below the box.
    let (x, y) = s
        .eval::<(f64, f64)>(
            "return BenillaScriptLogDetailScroll:GetLeft() + 40, BenillaScriptLogDetailScroll:GetBottom() + 6",
        )
        .unwrap();
    assert!(
        y < s
            .eval::<f64>("return BenillaScriptLogDetailText:GetBottom()")
            .unwrap(),
        "sanity: the click is below the box"
    );
    assert!(!s.has_keyboard_focus());
    s.mouse_button(x as f32, y as f32, "LeftButton", true);
    s.mouse_button(x as f32, y as f32, "LeftButton", false);
    settle(&mut s);
    assert_eq!(
        s.focused_editbox_name().as_deref(),
        Some("BenillaScriptLogDetailText"),
        "the pane's foot focused the box"
    );
    assert_eq!(
        s.editbox_copy().as_deref(),
        Some("short"),
        "all of it selected"
    );
}

/// A click selects the whole error, so Ctrl+C (`UiScript::editbox_copy`) copies all of it; an
/// error arriving meanwhile leaves that selection alone, and another row takes the keyboard off
/// the box with the new text.
#[test]
fn a_click_on_the_detail_selects_all_and_copy_returns_the_whole_error() {
    benilla_formats::wow_data_or_skip!();
    let (mut s, msg) = error_log_selected();
    assert_eq!(s.editbox_copy(), None, "nothing to copy before a click");
    click_detail(&mut s);
    assert_eq!(s.editbox_copy().as_deref(), Some(msg.as_str()));

    // The window's update repaints on every new error, and must not collapse the selection.
    s.run("BenillaScriptLog_Record('another error')").unwrap();
    settle(&mut s);
    assert_eq!(s.editbox_copy().as_deref(), Some(msg.as_str()));

    // Another row: the text changes and the box lets go of the keyboard.
    s.run("BenillaScriptLogRow2:Click()").unwrap();
    settle(&mut s);
    assert_eq!(detail_text(&s), "another error");
    assert!(!s.has_keyboard_focus());
    click_detail(&mut s);
    assert_eq!(s.editbox_copy().as_deref(), Some("another error"));
}

/// Typing, pasting, Enter, Backspace and cut leave the text as it was, the selection is put back
/// so a copy still takes it all, and the restore's own change stops after one more round.
#[test]
fn an_edit_of_the_detail_box_is_undone() {
    benilla_formats::wow_data_or_skip!();
    let (mut s, msg) = error_log_selected();
    click_detail(&mut s);
    s.run(
        "CHANGES = 0 \
         local handler = BenillaScriptLogDetail_OnTextChanged \
         BenillaScriptLogDetail_OnTextChanged = function() CHANGES = CHANGES + 1 handler() end",
    )
    .unwrap();

    // With everything selected, a typed letter replaces the whole text until the tick restores it.
    assert!(s.char_input("x"));
    assert_eq!(
        detail_text(&s),
        "x",
        "the edit lands first, as in the reference"
    );
    settle(&mut s);
    assert_eq!(detail_text(&s), msg, "typing");
    assert_eq!(
        s.eval::<i64>("return CHANGES").unwrap(),
        2,
        "the edit, then its restore"
    );
    assert_eq!(
        s.editbox_copy().as_deref(),
        Some(msg.as_str()),
        "still all selected"
    );

    assert!(s.paste("pasted\nlines"));
    settle(&mut s);
    assert_eq!(detail_text(&s), msg, "pasting");

    assert!(s.key_input("ENTER"));
    settle(&mut s);
    assert_eq!(detail_text(&s), msg, "Enter");

    s.editbox_action(EditAction::Delete {
        unit: EditUnit::Char,
        back: true,
    });
    settle(&mut s);
    assert_eq!(detail_text(&s), msg, "Backspace");

    // A cut still copies, then the box takes the text back.
    assert_eq!(s.editbox_cut().as_deref(), Some(msg.as_str()));
    settle(&mut s);
    assert_eq!(detail_text(&s), msg, "cut");
    assert_eq!(s.editbox_copy().as_deref(), Some(msg.as_str()));
}

/// Escape clears the box's focus (`MailFrame.xml:644`), and leaves its text.
#[test]
fn escape_clears_the_detail_boxs_focus() {
    benilla_formats::wow_data_or_skip!();
    let (mut s, msg) = error_log_selected();
    click_detail(&mut s);
    assert!(s.key_input("ESCAPE"));
    settle(&mut s);
    assert!(!s.has_keyboard_focus(), "Escape let go of the keyboard");
    assert_eq!(detail_text(&s), msg);
}
