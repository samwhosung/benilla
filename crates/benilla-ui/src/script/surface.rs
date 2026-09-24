//! **benilla's widget METHOD surface, asked of a running VM** — the shared measurement behind
//! the `dump_widget_methods` example and the widget-surface gate
//! (`script::tests::widget_surface`).
//!
//! It exists for the same reason `dump_globals` does: **it is a run, not a
//! grep**. `_G` is not the whole surface an addon can tell apart — most of what an addon touches
//! is reached through a *widget*, and an addon's test for "does this client have X" is
//! `if frame.SetBackdrop then`, i.e. what the `__index` chain answers on a live instance. A regex
//! over `m.set("Name", …)` cannot answer that: it cannot see which registry table a kind actually
//! chains to, it cannot see the Texture/FontString leaf split ([`super::region`]), and it reads a
//! name registered under a `format!` family as one method.
//!
//! So every row is one live object being asked `type(obj.Name) == "function"`, exactly as an addon
//! asks it. The only thing **not** asked of the VM is the *candidate name list* — a metatable whose
//! `__index` is a Rust dispatcher is not enumerable from Lua, so the candidates are the union of
//! the keys of the VM's own method tables ([`METHOD_TABLES`]), read out of the Lua registry at
//! runtime rather than out of the source. A key that no longer resolves to a table is a hard error
//! rather than a silent loss of candidates: a quietly-missing candidate source would under-report
//! the surface, which is the exact failure this measurement exists to prevent.
use std::collections::BTreeSet;

use mlua::{Table, Value};

use super::UiScript;

/// The named-registry method tables the VM installs — the **candidate name source**, read from the
/// live registry.
///
/// This is not the class chain (the chain is `object::kind_method_registries`' business and this
/// instrument deliberately does not model it — it asks the instance instead). It is only the
/// universe of names worth probing: every name benilla registers on any widget appears as a key in
/// one of these, so the union is a superset of our surface and the per-instance probe decides
/// membership.
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

/// Every widget/region/font class benilla can hand an addon, and one Lua expression that makes one.
///
/// **Ordered, and the order is load-bearing**: the region and font rows are made *by* the plain
/// frame the first row creates, so `Frame` comes first. Each instance is published as the global
/// `DW_<Class>`; the frame kinds also publish their own `CreateFrame` name, which is harmless.
///
/// `TaxiRouteFrame` is absent on purpose — it is a registered `CreateFrame` type that *is* a
/// `Frame` and nothing else, so it would be the `Frame` row twice.
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

/// Every `(class, method)` pair this VM answers — one live instance per class, probed with the
/// membership test an addon writes.
///
/// **A class that cannot be instantiated is REPORTED, never skipped**: its row is
/// `(class, "!NOT-INSTANTIABLE")`. A census that silently drops a class reads as "that class has
/// no methods", which is the wrong answer to a different question.
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
