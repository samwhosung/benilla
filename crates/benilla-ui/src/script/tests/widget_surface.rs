//! The widget-method surface gate, the widget sibling of [`super::reference_surface`]. The
//! reference's 23 widget registrar tables are disjoint, so a class's surface is its own table plus
//! those it inherits ([`WIDGET_CHAINS`]): `0x87c9b8` is the base `Region` map (the 19
//! `REGION_MAP_METHODS`), and `LootButton`'s own `0x847ce4` holds only `SetSlot`.
use std::collections::{BTreeMap, BTreeSet};

use crate::script::{widget_method_census, UiScript};

/// `(class, its own registrar table, the tables it inherits)`. `WorldFrame` is a `Frame` to Lua
/// and `TitleRegion` a bare `Region`; like `TaxiRouteFrame`, neither has a table of its own.
const WIDGET_CHAINS: &[(&str, &str, &[&str])] = &[
    ("Frame", FRAME, &[REGION]),
    ("WorldFrame", FRAME, &[REGION]),
    ("Button", BUTTON, FRAME_CHAIN),
    ("LootButton", "0x847ce4", &[BUTTON, FRAME, REGION]),
    ("CheckButton", "0x87bf74", &[BUTTON, FRAME, REGION]),
    ("EditBox", "0x87bb68", FRAME_CHAIN),
    ("StatusBar", "0x87b010", FRAME_CHAIN),
    ("Slider", "0x87b260", FRAME_CHAIN),
    ("ScrollFrame", "0x87b3c0", FRAME_CHAIN),
    ("Model", MODEL, FRAME_CHAIN),
    ("PlayerModel", PLAYER_MODEL, &[MODEL, FRAME, REGION]),
    (
        "DressUpModel",
        "0x84f190",
        &[PLAYER_MODEL, MODEL, FRAME, REGION],
    ),
    (
        "TabardModel",
        "0x84ee40",
        &[PLAYER_MODEL, MODEL, FRAME, REGION],
    ),
    ("MessageFrame", "0x87b960", FRAME_CHAIN),
    ("ScrollingMessageFrame", "0x87b5c0", FRAME_CHAIN),
    ("ColorSelect", "0x87abb0", FRAME_CHAIN),
    ("SimpleHTML", "0x87ba80", FRAME_CHAIN),
    ("MovieFrame", "0x87ab4c", FRAME_CHAIN),
    ("GameTooltip", "0x854198", FRAME_CHAIN),
    ("Minimap", "0x84c538", FRAME_CHAIN),
    ("Texture", "0x87c128", &[REGION]),
    ("FontString", "0x87c1d8", &[REGION]),
    // The font object (`CreateFont`) inherits nothing.
    ("Font", "0x87c7c8", &[]),
    ("TitleRegion", REGION, &[]),
];
const REGION: &str = "0x87c9b8";
const FRAME: &str = "0x878ec0";
const BUTTON: &str = "0x879d00";
const MODEL: &str = "0x878948";
const PLAYER_MODEL: &str = "0x84f1fc";
const FRAME_CHAIN: &[&str] = &[FRAME, REGION];

/// Reference methods this client does not answer yet: one row per name, its classes and why. The
/// list only shrinks; a name with live callers belongs implemented.
const NOT_YET_ANSWERED: &[(&str, &str, &str)] = &[
    // ── The MovieFrame trio ──
    // The stock glue `MovieFrame.lua` calls all three; benilla plays no movie yet.
    (
        "StartMovie",
        "MovieFrame",
        "the intro-movie screen's own verb; benilla plays no movie yet",
    ),
    ("StopMovie", "MovieFrame", "the same, on the skip path"),
    (
        "EnableSubtitles",
        "MovieFrame",
        "the same — `MovieFrame.lua:35` passes `GetMovieSubtitles()`",
    ),
    // ── The Minimap's four content setters ──
    // The app draws the minimap's blips and arrow itself, never through Lua.
    (
        "SetArrowModel",
        "Minimap",
        "app-side art; no caller in the stock UI or either corpus",
    ),
    ("SetBlipTexture", "Minimap", "the same"),
    ("SetIconTexture", "Minimap", "the same"),
    ("SetPlayerModel", "Minimap", "the same"),
    // ── The Button font-object and text-colour family ──
    // `button.rs` keeps this font state but publishes none of it; nothing calls these.
    (
        "GetDisabledFontObject",
        "Button CheckButton LootButton",
        "the button font-state family; no caller anywhere measured",
    ),
    (
        "GetDisabledTextColor",
        "Button CheckButton LootButton",
        "the same family",
    ),
    (
        "GetHighlightFontObject",
        "Button CheckButton LootButton",
        "the same family",
    ),
    (
        "GetHighlightTextColor",
        "Button CheckButton LootButton",
        "the same family",
    ),
    (
        "GetTextFontObject",
        "Button CheckButton LootButton",
        "the same family",
    ),
    (
        "GetPushedTextOffset",
        "Button CheckButton LootButton",
        "the same family",
    ),
    (
        "SetPushedTextOffset",
        "Button CheckButton LootButton",
        "the same family",
    ),
    // ── Line spacing, on the five classes that carry text ──
    (
        "GetSpacing",
        "EditBox Font FontString MessageFrame ScrollingMessageFrame",
        "line spacing is not modelled by `ui_text`'s layout; no caller anywhere measured",
    ),
    (
        "SetSpacing",
        "EditBox Font FontString MessageFrame ScrollingMessageFrame",
        "the same — the setter is a layout change, not a stored value",
    ),
    // ── EditBox mode read-backs ──
    (
        "IsAutoFocus",
        "EditBox",
        "the mode is set and not readable back; no caller anywhere measured",
    ),
    ("IsMultiLine", "EditBox", "the same"),
    ("IsNumeric", "EditBox", "the same"),
    ("IsPassword", "EditBox", "the same"),
    // ── ScrollingMessageFrame's line-position surface ──
    (
        "GetCurrentLine",
        "ScrollingMessageFrame",
        "the buffer's position is internal; no caller measured",
    ),
    ("GetCurrentScroll", "ScrollingMessageFrame", "the same"),
    ("GetNumLinesDisplayed", "ScrollingMessageFrame", "the same"),
    ("SetScrollFromBottom", "ScrollingMessageFrame", "the same"),
    // ── The colour picker's HSV pair ──
    (
        "GetColorHSV",
        "ColorSelect",
        "the RGB pair is published; nothing measured asks for HSV",
    ),
    ("SetColorHSV", "ColorSelect", "the same"),
    // ── One texture read-back ──
    (
        "IsDesaturated",
        "Texture",
        "`SetDesaturated` is here and not readable back; no caller measured",
    ),
];

/// Methods answered here that 1.12 does not register on that class, each with why it stays.
const ALLOWED_BEYOND: &[(&str, &str, &str)] = &[];

/// The reference's widget rows, `table_va -> {name}`.
fn reference_tables() -> BTreeMap<String, BTreeSet<String>> {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/1.12-shapes.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/1.12-shapes.tsv");
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('#') || line.starts_with("name\t") {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 5 || f[4] != "widget" {
            continue;
        }
        out.entry(f[3].to_string())
            .or_default()
            .insert(f[0].to_string());
    }
    out
}

/// `class -> the effective reference surface` (its own table and everything it inherits).
fn reference_surface_by_class() -> BTreeMap<&'static str, BTreeSet<String>> {
    let tables = reference_tables();
    assert!(
        tables.len() == 23,
        "the shapes table gave {} widget registrar tables, not the reference's 23 — it changed \
         shape, and every count below is against the wrong denominator",
        tables.len()
    );
    let mut out = BTreeMap::new();
    for (class, own, bases) in WIDGET_CHAINS {
        let mut set = tables.get(*own).cloned().unwrap_or_default();
        for base in *bases {
            set.extend(tables.get(*base).cloned().unwrap_or_default());
        }
        assert!(!set.is_empty(), "{class}: empty reference surface");
        out.insert(*class, set);
    }
    out
}

/// `class -> what this VM answers`, asked of a live instance of each.
fn ours_by_class() -> BTreeMap<String, BTreeSet<String>> {
    let script = UiScript::new().expect("VM");
    let rows = widget_method_census(&script).expect("census");
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (class, method) in rows {
        assert_ne!(
            method, "!NOT-INSTANTIABLE",
            "{class} could not be instantiated — the census cannot speak for a class it never made"
        );
        out.entry(class).or_default().insert(method);
    }
    out
}

/// A declared list, `name -> (classes, reason)`.
fn declared(list: &[(&str, &str, &str)]) -> BTreeMap<String, BTreeSet<String>> {
    list.iter()
        .map(|(name, classes, _)| {
            (
                (*name).to_string(),
                classes.split_whitespace().map(str::to_string).collect(),
            )
        })
        .collect()
}

/// Render a `class -> names` map as declared-list rows, ready to paste in.
fn as_rows(diff: &BTreeMap<String, BTreeSet<String>>) -> String {
    let mut by_name: BTreeMap<&String, Vec<&String>> = BTreeMap::new();
    for (class, names) in diff {
        for n in names {
            by_name.entry(n).or_default().push(class);
        }
    }
    by_name
        .iter()
        .map(|(name, classes)| {
            format!(
                "        (\"{name}\", \"{}\", \"WHY\"),",
                classes
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every widget method the 1.12 client registers on a class is answered by ours, or listed.
#[test]
fn every_1_12_widget_method_is_answered() {
    let reference = reference_surface_by_class();
    let ours = ours_by_class();
    let listed = declared(NOT_YET_ANSWERED);

    let mut missing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut total = 0usize;
    for (class, want) in &reference {
        let have = ours.get(*class).cloned().unwrap_or_default();
        total += want.len();
        let gap: BTreeSet<String> = want
            .iter()
            .filter(|n| !have.contains(*n))
            .filter(|n| !listed.get(*n).is_some_and(|c| c.contains(*class)))
            .cloned()
            .collect();
        if !gap.is_empty() {
            missing.insert((*class).to_string(), gap);
        }
    }
    assert!(
        total > 2_000,
        "only {total} class/method pairs on the reference side — the chain stopped resolving"
    );
    assert!(
        missing.is_empty(),
        "the 1.12 client registers these on a class benilla does not answer them on, and they are \
         not listed:\n{}\n\nImplement it, or add the row to NOT_YET_ANSWERED with the reason it \
         has not been built.",
        as_rows(&missing)
    );

    // A listed name that is answered now fails, so the list cannot outlive its fix.
    let mut healed: Vec<String> = Vec::new();
    for (name, classes, _) in NOT_YET_ANSWERED {
        for class in classes.split_whitespace() {
            if ours.get(class).is_some_and(|h| h.contains(*name)) {
                healed.push(format!("{class}:{name}"));
            }
        }
    }
    assert!(
        healed.is_empty(),
        "these are answered now — take them out of NOT_YET_ANSWERED so the list keeps meaning what \
         it says: {healed:?}"
    );
}

/// Every widget method benilla answers is one 1.12 registers on that class, or a listed exception.
#[test]
fn our_widget_methods_stay_inside_the_1_12_surface() {
    let reference = reference_surface_by_class();
    let ours = ours_by_class();
    let listed = declared(ALLOWED_BEYOND);

    let mut beyond: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (class, have) in &ours {
        let Some(want) = reference.get(class.as_str()) else {
            panic!("{class} is instantiable here but has no row in WIDGET_CHAINS");
        };
        let extra: BTreeSet<String> = have
            .iter()
            .filter(|n| !want.contains(*n))
            // Our host bridge and VM internals are marked by prefix, as `reference_surface` does.
            .filter(|n| !n.starts_with("Benilla") && !n.starts_with("__benilla_"))
            .filter(|n| !listed.get(*n).is_some_and(|c| c.contains(class)))
            .cloned()
            .collect();
        if !extra.is_empty() {
            beyond.insert(class.clone(), extra);
        }
    }
    assert!(
        beyond.is_empty(),
        "benilla answers these on a class the 1.12 client does not register them on:\n{}\n\n\
         1.12 is the target (decision 1188). Either remove it, give it its 1.12 spelling, or add \
         the row to ALLOWED_BEYOND WITH the reason it has to stay — an unexplained superset is \
         what 1189 had to roll back, and an addon that feature-detects one takes a path we cannot \
         honour.",
        as_rows(&beyond)
    );

    let mut gone: Vec<String> = Vec::new();
    for (name, classes, _) in ALLOWED_BEYOND {
        for class in classes.split_whitespace() {
            if !ours.get(class).is_some_and(|h| h.contains(*name)) {
                gone.push(format!("{class}:{name}"));
            }
        }
    }
    assert!(
        gone.is_empty(),
        "these are no longer exposed — take them out of ALLOWED_BEYOND: {gone:?}"
    );
}
