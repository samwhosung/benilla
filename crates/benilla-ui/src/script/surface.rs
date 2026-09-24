//! benilla's widget method surface asked of a running VM, shared by the `dump_widget_methods`
//! example and the widget-surface gate (`script::tests::widget_surface`). Each row is a live
//! instance asked `type(obj.Name) == "function"`, as an addon asks it; only the candidate names
//! come from elsewhere, the keys of the VM's method tables ([`METHOD_TABLES`]), since a Rust
//! `__index` dispatcher cannot be enumerated from Lua.
use std::collections::BTreeSet;

use mlua::{Table, Value};

use super::UiScript;

/// The VM's named-registry method tables: every widget method benilla registers is a key in one,
/// and the per-instance probe decides which a class answers.
const METHOD_TABLES: &[&str] = &[
    "__benilla_frame_methods",
    "__benilla_region_methods",
    "__benilla_texture_methods",
    "__benilla_fontstring_methods",
    "__benilla_title_methods",
    "__benilla_font_methods",
    "__benilla_button_methods",
    "__benilla_checkbutton_methods",
    "__benilla_lootbutton_methods",
    "__benilla_editbox_methods",
    "__benilla_statusbar_methods",
    "__benilla_slider_methods",
    "__benilla_scrollframe_methods",
    "__benilla_simplehtml_methods",
    "__benilla_colorselect_methods",
    "__benilla_model_methods",
    "__benilla_playermodel_methods",
    "__benilla_dressupmodel_methods",
    "__benilla_tabardmodel_methods",
    "__benilla_plain_messageframe_methods",
    "__benilla_scrollingmessageframe_methods",
    "__benilla_minimap_methods",
    "__benilla_tooltip_methods",
];

/// Every class benilla can hand an addon, with a Lua expression making one, published as
/// `DW_<Class>`. `Frame` must come first: the region and font rows are made by it.
/// `TaxiRouteFrame` is left out, being a plain `Frame` to Lua.
const CLASSES: &[(&str, &str)] = &[
    ("Frame", r#"CreateFrame("Frame", "DWFrameN", UIParent)"#),
    (
        "WorldFrame",
        r#"CreateFrame("WorldFrame", "DWWorldN", UIParent)"#,
    ),
    ("Button", r#"CreateFrame("Button", "DWButtonN", UIParent)"#),
    (
        "LootButton",
        r#"CreateFrame("LootButton", "DWLootN", UIParent)"#,
    ),
    (
        "CheckButton",
        r#"CreateFrame("CheckButton", "DWCheckN", UIParent)"#,
    ),
    ("EditBox", r#"CreateFrame("EditBox", "DWEditN", UIParent)"#),
    (
        "StatusBar",
        r#"CreateFrame("StatusBar", "DWStatusN", UIParent)"#,
    ),
    ("Slider", r#"CreateFrame("Slider", "DWSliderN", UIParent)"#),
    (
        "ScrollFrame",
        r#"CreateFrame("ScrollFrame", "DWScrollN", UIParent)"#,
    ),
    ("Model", r#"CreateFrame("Model", "DWModelN", UIParent)"#),
    (
        "PlayerModel",
        r#"CreateFrame("PlayerModel", "DWPlayerModelN", UIParent)"#,
    ),
    (
        "DressUpModel",
        r#"CreateFrame("DressUpModel", "DWDressN", UIParent)"#,
    ),
    (
        "TabardModel",
        r#"CreateFrame("TabardModel", "DWTabardN", UIParent)"#,
    ),
    (
        "MessageFrame",
        r#"CreateFrame("MessageFrame", "DWMessageN", UIParent)"#,
    ),
    (
        "ScrollingMessageFrame",
        r#"CreateFrame("ScrollingMessageFrame", "DWScrollMsgN", UIParent)"#,
    ),
    (
        "ColorSelect",
        r#"CreateFrame("ColorSelect", "DWColorN", UIParent)"#,
    ),
    (
        "SimpleHTML",
        r#"CreateFrame("SimpleHTML", "DWHtmlN", UIParent)"#,
    ),
    (
        "MovieFrame",
        r#"CreateFrame("MovieFrame", "DWMovieN", UIParent)"#,
    ),
    (
        "GameTooltip",
        r#"CreateFrame("GameTooltip", "DWTooltipN", UIParent)"#,
    ),
    (
        "Minimap",
        r#"CreateFrame("Minimap", "DWMinimapN", UIParent)"#,
    ),
    ("Texture", r#"DW_Frame:CreateTexture("DWTexN")"#),
    ("FontString", r#"DW_Frame:CreateFontString("DWFSN")"#),
    ("TitleRegion", r#"DW_Frame:CreateTitleRegion()"#),
    ("Font", r#"CreateFont("DWFontN")"#),
];

/// Every `(class, method)` pair this VM answers, one live instance per class. A class that cannot
/// be made reports `(class, "!NOT-INSTANTIABLE")` rather than dropping out.
pub fn widget_method_census(script: &UiScript) -> mlua::Result<Vec<(String, String)>> {
    let mut candidates: BTreeSet<String> = BTreeSet::new();
    for key in METHOD_TABLES {
        let table: Table = script.lua().named_registry_value(key).map_err(|e| {
            mlua::Error::runtime(format!(
                "candidate source '{key}' is not a table in this VM ({e}) — a renamed or retired \
                 method table would silently shrink this census; fix the list, do not ignore it"
            ))
        })?;
        for pair in table.pairs::<Value, Value>() {
            let (k, _) = pair?;
            if let Value::String(s) = k {
                candidates.insert(s.to_str()?.to_string());
            }
        }
    }
    // Hand the names to the VM once; the per-class probe is then one chunk, not one per name.
    let names = script.lua().create_table()?;
    for (i, n) in candidates.iter().enumerate() {
        names.set(i + 1, n.as_str())?;
    }
    script.lua().globals().set("DW_NAMES", names)?;

    let mut rows: Vec<(String, String)> = Vec::new();
    for (class, expr) in CLASSES {
        if let Err(e) = script.run(&format!("DW_{class} = {expr}")) {
            eprintln!("{class}: could not be instantiated: {e}");
            rows.push(((*class).to_string(), "!NOT-INSTANTIABLE".to_string()));
            continue;
        }
        let found: Vec<String> = script.eval(&format!(
            "local o = DW_{class} \
             local out = {{}} \
             for i = 1, table.getn(DW_NAMES) do \
               local n = DW_NAMES[i] \
               if type(o[n]) == 'function' then table.insert(out, n) end \
             end \
             return out"
        ))?;
        for name in found {
            rows.push(((*class).to_string(), name));
        }
    }
    rows.sort();
    Ok(rows)
}
