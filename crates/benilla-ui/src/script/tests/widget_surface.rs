//! **The widget-method surface gate** — the sibling of [`super::reference_surface`], for the API
//! an addon reaches through a *widget* rather than through `_G`.
//!
//! `our_globals_stay_inside_the_1_12_surface` has guarded the global namespace since 1189, and the
//! shape gate (1842/1843/2118) guards the arity and return kinds of the methods we *have*. Between
//! them sat the question neither asks: **is the set of method NAMES on each widget class the set
//! 1.12 registers?** A method the reference has and we do not is invisible to the shape gate,
//! which skips what does not exist; a method we have and the reference does not is invisible to
//! everything, and is the superset `api-coverage.sh`'s header calls "not free" — an addon that
//! feature-detects (`if region.GetBlendMode then`) takes a path we cannot honour, and the failure
//! surfaces far from the cause.
//!
//! A corpus census over 110 top-20 and 219 vanilla addons found both directions live at once
//! (2142): `ShaguTweaks` probes `GetBlendMode`, which we were missing, and silently recolours
//! textures the reference leaves alone; `pfUI` and `_Nameplates` call `GameTooltip:GetAnchorType`
//! bare and raise. The demand is in the corpus, which is not in this repo — so this gate does not
//! measure demand. It measures the SURFACE, and every entry in the two declared lists below is
//! where the reading of the demand got written down.
//!
//! When re-reading that demand: those corpora hold addons that *run on* 1.12, not addons written
//! only for it — most of the big ones are multi-client and pick a code path at load, and they come
//! in several versions of themselves. `scripts/api-coverage.sh`'s header has the detail; decision
//! 2146 is the case that made it matter.
//!
//! ## The reference side is a CHAIN, not 23 flat surfaces
//!
//! The reference's 23 widget registrar tables are mutually **disjoint** — a class's real surface is
//! its own table plus the tables it misses into. Comparing a live `Frame` against `0x878ec0` alone
//! reports 87 of its 88 methods as beyond-1.12, which is how you get a gate nobody believes.
//! [`WIDGET_CHAINS`] is that inheritance, and its two non-obvious rows are byte-settled, not
//! guessed: `0x87c9b8` is the base `Region` map (its 19 names are exactly [`super::super`]'s
//! `REGION_MAP_METHODS`), and `0x847ce4` is `LootButton`, whose whole surface of its own is
//! `SetSlot` (read off the registrar's own `mov edx,1`).
use std::collections::{BTreeMap, BTreeSet};

use crate::script::{widget_method_census, UiScript};

/// `(class, its own registrar table, the tables it inherits)`.
///
/// `WorldFrame` is a `Frame` to Lua (1983/1984) and `TitleRegion` a bare `Region`, so neither has a
/// table of its own among the 23. `TaxiRouteFrame` is absent for the same reason (1828).
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
    // The font OBJECT (`CreateFont`) carries its own `GetName`/`GetObjectType`/`IsObjectType` and
    // inherits nothing.
    ("Font", "0x87c7c8", &[]),
    ("TitleRegion", REGION, &[]),
];
const REGION: &str = "0x87c9b8";
const FRAME: &str = "0x878ec0";
const BUTTON: &str = "0x879d00";
const MODEL: &str = "0x878948";
const PLAYER_MODEL: &str = "0x84f1fc";
const FRAME_CHAIN: &[&str] = &[FRAME, REGION];

/// Methods the reference registers that this client does not answer — **a list that may only
/// shrink**, one row per name, naming every class it is missing on and why it has not been built.
///
/// The corpus demand behind each is 2142's census. A name with real, live callers does not belong
/// here: it belongs implemented.
const NOT_YET_ANSWERED: &[(&str, &str, &str)] = &[
    // ── The MovieFrame trio: zero addon demand, and the stock GLUE screen calls all three ──────
    // `_extracted_gluexml/MovieFrame.lua:23/30/35/47/71`. The glue screens stay ours permanently,
    // so the intro-movie screen is owed these however little the ecosystem wants them —
    // which makes this the one group here that is a real backlog item rather than a name nobody
    // has ever typed. benilla has no movie playback at all, so it is a feature, not a verb.
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
    // ── The Minimap's four content setters ────────────────────────────────────────────────────
    // benilla draws the minimap's blips, arrow and mask from the app side (`minimap/blips`), never
    // through Lua; the stock UI does not call these either. Wiring them means routing the art
    // choice back through the VM, which nothing has asked for.
    (
        "SetArrowModel",
        "Minimap",
        "app-side art; no caller in the stock UI or either corpus",
    ),
    ("SetBlipTexture", "Minimap", "the same"),
    ("SetIconTexture", "Minimap", "the same"),
    ("SetPlayerModel", "Minimap", "the same"),
    // ── The Button font-object and text-colour family ─────────────────────────────────────────
    // Seven getters and one setter over a button's disabled/highlight/pushed font state. benilla
    // keeps that state (`button.rs`'s `Slot` triple) but publishes none of it back. **Zero call
    // sites in 329 addon folders and zero in the stock UI** — the whole family is completeness.
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
    // ── Line spacing, on the five classes that carry text ─────────────────────────────────────
    // A real layout property benilla's text layout does not model — a setter here is not a store,
    // it is a change to how `ui_text` lays out wrapped lines. No caller anywhere measured.
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
    // ── EditBox mode read-backs ───────────────────────────────────────────────────────────────
    // The four modes are all SET here (`editbox`'s `SetAutoFocus`/`SetMultiLine`/`SetNumeric`/
    // `SetPassword`) and none is readable. Cheap, and nothing has ever asked.
    (
        "IsAutoFocus",
        "EditBox",
        "the mode is set and not readable back; no caller anywhere measured",
    ),
    ("IsMultiLine", "EditBox", "the same"),
    ("IsNumeric", "EditBox", "the same"),
    ("IsPassword", "EditBox", "the same"),
    // ── ScrollingMessageFrame's line-position surface ─────────────────────────────────────────
    // The scroll POSITION verbs (`ScrollUp`/`ScrollDown`/`ScrollToTop`…) are all here; these four
    // report or set where that position sits, which our message buffer tracks internally and does
    // not publish. No caller anywhere measured.
    (
        "GetCurrentLine",
        "ScrollingMessageFrame",
        "the buffer's position is internal; no caller measured",
    ),
    ("GetCurrentScroll", "ScrollingMessageFrame", "the same"),
    ("GetNumLinesDisplayed", "ScrollingMessageFrame", "the same"),
    ("SetScrollFromBottom", "ScrollingMessageFrame", "the same"),
    // ── The colour picker's HSV pair ──────────────────────────────────────────────────────────
    // `ColorSelect` publishes the RGB pair; the HSV twin would need the conversion the reference
    // does in C. The stock `ColorPickerFrame` uses RGB only, and so does every corpus caller.
    (
        "GetColorHSV",
        "ColorSelect",
        "the RGB pair is published; nothing measured asks for HSV",
    ),
    ("SetColorHSV", "ColorSelect", "the same"),
    // ── One texture read-back ─────────────────────────────────────────────────────────────────
    (
        "IsDesaturated",
        "Texture",
        "`SetDesaturated` is here and not readable back; no caller measured",
    ),
];

/// Methods this client answers that 1.12 does not register on that class — **each a deliberate,
/// reasoned exception, never a quiet superset**. Same contract as
/// [`super::reference_surface::allowed_beyond_1_12`]: a name here must say why it stays.
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

/// `class -> the effective reference surface` (own table + everything it inherits).
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

/// Render a `class -> names` map as the declared-list rows it would need, so a failure hands you
/// the entry to paste rather than a set difference to transcribe.
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

    // A listed name that came back fails, so the list cannot outlive its fix.
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
            // Our host bridge and the VM's own internals declare themselves ours by prefix, the
            // same discipline `reference_surface` uses for globals.
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
