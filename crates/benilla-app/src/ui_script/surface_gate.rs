//! The 1.12 surface of the production load as a gate: every global and widget method an addon
//! reaches once the core and benilla's layer have loaded is one the 1.12.1 client has
//! (`reference/1.12-globals.tsv`), one the engine exposes, or a `Benilla`-prefixed name the layer
//! defines. The engine's own surface is gated on a bare VM in `benilla-ui`, name by name against
//! the same table (`script::tests::reference_surface`, whose commented `BEYOND_1_12` is the one
//! allowlist) and method by method against each class's registrar tables
//! (`script::tests::widget_surface`); so here the load itself may add nothing else.

use std::collections::{BTreeSet, HashSet};

use benilla_ui::script::{widget_method_census, UiScript};

use crate::local_state::test_env::ENV_LOCK;

/// `reference/1.12-globals.tsv`'s names.
fn reference_globals() -> HashSet<&'static str> {
    let tsv = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/1.12-globals.tsv"
    ));
    let names: HashSet<&str> = tsv
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split('\t').next())
        .collect();
    assert!(
        names.len() > 19_000,
        "the reference table looks truncated ({} names)",
        names.len()
    );
    names
}

/// The string keys of `_G`.
fn globals_of(s: &UiScript) -> BTreeSet<String> {
    s.eval::<Vec<String>>(
        "local out = {} for k in pairs(getfenv(0)) do \
         if type(k) == 'string' then table.insert(out, k) end end return out",
    )
    .expect("dump the globals")
    .into_iter()
    .collect()
}

/// Every `(class, method)` a VM answers, as `Class:Method`. The WorldFrame type is one-shot
/// (`0x6ee439`), spent once the stock `WorldFrame` exists, so a loaded VM cannot make a second;
/// its methods are a Frame's, which the census asks of a Frame.
fn methods_of(s: &UiScript) -> BTreeSet<String> {
    widget_method_census(s)
        .expect("the widget census")
        .into_iter()
        .filter_map(|(class, method)| {
            if method == "!NOT-INSTANTIABLE" {
                assert_eq!(class, "WorldFrame", "{class} could not be made");
                return None;
            }
            Some(format!("{class}:{method}"))
        })
        .collect()
}

/// A name the layer may define: its prefix, or a slash alias's `SLASH_BENILLA_` form.
fn layer_prefixed(n: &str) -> bool {
    n.starts_with("Benilla") || n.starts_with("BENILLA_") || n.starts_with("SLASH_BENILLA_")
}

/// What the production load adds past 1.12 and past the engine, and the layer's own names: the
/// globals (`Benilla`-prefixed ones the engine does not also define passing) and the methods.
fn production_beyond(s: &UiScript, engine: &UiScript) -> (Vec<String>, Vec<String>) {
    let reference = reference_globals();
    let engine_globals = globals_of(engine);
    let globals = globals_of(s)
        .into_iter()
        .filter(|n| !reference.contains(n.as_str()) && !engine_globals.contains(n))
        .filter(|n| !layer_prefixed(n))
        .collect();
    let engine_methods = methods_of(engine);
    let methods = methods_of(s)
        .into_iter()
        .filter(|m| !engine_methods.contains(m))
        .collect();
    (globals, methods)
}

/// The production load, core and layer, adds no global past 1.12 but the layer's own
/// `Benilla`-prefixed names, and no widget method at all.
#[test]
fn the_production_load_stays_inside_the_1_12_surface() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (s, failures) = super::layer_tests::production_load_with("surface", false, "", |_| {});
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    let engine = UiScript::new().unwrap();
    // The layer's globals are there to be checked: a load that defined none proves nothing.
    assert!(
        globals_of(&s)
            .iter()
            .any(|n| layer_prefixed(n) && !globals_of(&engine).contains(n)),
        "the layer defined no global: it did not load"
    );

    let (globals, methods) = production_beyond(&s, &engine);
    assert!(
        globals.is_empty(),
        "the production load exposes {} global(s) the 1.12.1 client does not:\n    {}\n\n\
         A layer global takes the `Benilla` prefix; anything else gets its 1.12 spelling or goes.",
        globals.len(),
        globals.join(" ")
    );
    assert!(
        methods.is_empty(),
        "the production load adds {} widget method(s) the engine does not register:\n    {}",
        methods.len(),
        methods.join(" ")
    );
}

/// The gate bites: a global past 1.12 put back into the loaded VM, an unprefixed layer-style one,
/// an engine-only prefixed one and a method on a class's table are each caught.
#[test]
fn the_production_gate_catches_a_name_put_back() {
    let s = UiScript::new().unwrap();
    let engine = UiScript::new().unwrap();
    assert_eq!(
        production_beyond(&s, &engine),
        (Vec::new(), Vec::new()),
        "two bare VMs differ"
    );
    s.run(
        "UnitAura = function() end \
         BenillaLayerThing = 1 \
         getmetatable(CreateFrame('GameTooltip')).__index.BenillaSetItemById = function() end",
    )
    .unwrap();
    let (globals, methods) = production_beyond(&s, &engine);
    assert_eq!(globals, vec!["UnitAura".to_string()]);
    assert!(
        methods.contains(&"GameTooltip:BenillaSetItemById".to_string()),
        "{methods:?}"
    );
}
