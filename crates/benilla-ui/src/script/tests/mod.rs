//! Rust-driven tests of the Lua host: the object model, layout+size reads, show/hide + event + tick
//! firing (both `0x704d50`/`0x704f10` conventions), the WoW stdlib (positional `format`, the
//! aliases, `getglobal`), the sandbox holes, and an end-to-end extract in ZKey order.
//!
//! Split by subject; the shared `script()` fixture lives in [`common`].

mod addon_argument_abi;
mod addon_index_space;
mod anchors;
mod backdrop;
mod button;
mod channel;
mod common;
mod cooldown;
mod create_frame_template;
mod dispatch_bench;
mod end_to_end;
mod events;
mod font_object;
mod frame_api;
mod generic_for;
mod gm_ticket;
mod handler_prof;
mod input;
mod keyboard;
mod layout_gate;
mod measure;
mod minimap;
mod model_clock;
mod modelframe;
mod movable;
mod name_targets;
mod nameplate;
mod numeric_shape_gate;
mod object_model;
mod reference_surface;
mod region_map;
mod regions;
mod resize_bounds;
mod scrollframe;
mod setparent;
mod simplehtml;
mod size_changed;
mod slider;
mod statusbar;
mod stdlib;
mod structure_queries;
mod talent;
mod taxi;
mod texcoord_font;
mod tooltip;
mod tooltip_item;
mod tooltip_spell;
mod tooltip_unit;
mod toplevel;
mod visibility;
mod widget_surface;
mod worldframe;
mod worldmap;

/// **A bounded chunk reports instead of hanging** — the guard decision 1247's hang called for.
///
/// Without it, a `while true do end` inside an addon takes the whole process with it, and every
/// instrument downstream produces nothing at all: no roster, no columns, no error row. The bound
/// turns that into an ordinary Lua error, which every caller already knows how to report.
#[test]
fn an_instruction_budget_turns_a_runaway_loop_into_an_error() {
    let s = crate::script::UiScript::new().unwrap();
    s.set_instruction_budget(5_000_000);
    let err = s
        .run("local i = 0 while true do i = i + 1 end")
        .expect_err("an unbounded loop must raise rather than run forever")
        .to_string();
    assert!(
        err.contains("instruction budget exhausted"),
        "the raise names itself so a report can tell it from an addon's own error: {err}"
    );
    assert!(
        s.instructions_used() >= 5_000_000,
        "the counter reports what was spent: {}",
        s.instructions_used()
    );

    // **The VM survives it.** The bound must not poison the interpreter — a survey has 217 more
    // addons to run after the one that looped, and each gets its own VM only because this one
    // returned at all.
    assert_eq!(
        s.eval::<i64>("return 2 + 3").unwrap(),
        5,
        "the VM still evaluates after a budget raise"
    );

    // An ordinary chunk is untouched by a budget it never approaches.
    let t = crate::script::UiScript::new().unwrap();
    t.set_instruction_budget(200_000_000);
    t.run("BudgetOk = 0 for i = 1, 1000 do BudgetOk = BudgetOk + i end")
        .unwrap();
    assert_eq!(t.eval::<i64>("return BudgetOk").unwrap(), 500_500);
}

/// **`GetBindLocation()` — the hearth location's name, `""` before the packet lands.**
///
/// Three corpus addons read it (FuBar_TransporterFu `TransporterFu.lua:456`, Necrosis
/// `Necrosis.lua:1089`, _LazyPig `LazyPig.lua:623`), and the reference's own hearth confirmation
/// formats it straight into the dialog (`StaticPopup.lua:1742`).
#[test]
fn get_bind_location_answers_the_pushed_name_and_never_nil() {
    let mut s = crate::script::UiScript::new().unwrap();
    // Before the app pushes anything: the empty string, NOT nil. Necrosis concatenates the result,
    // so nil would be a raise rather than a blank — the same choice `GetRealmName` beside it makes.
    assert_eq!(s.eval::<String>("return GetBindLocation()").unwrap(), "");
    assert_eq!(
        s.eval::<String>("return type(GetBindLocation())").unwrap(),
        "string",
        "never nil — a consumer concatenates it"
    );

    s.set_bind_location("Stormwind City");
    assert_eq!(
        s.eval::<String>("return GetBindLocation()").unwrap(),
        "Stormwind City"
    );
    // Concatenation is the idiom that matters, so it is the one asserted.
    assert_eq!(
        s.eval::<String>(r#"return "Bound: " .. GetBindLocation()"#)
            .unwrap(),
        "Bound: Stormwind City"
    );
    // A re-bind replaces it rather than accumulating.
    s.set_bind_location("Ironforge");
    assert_eq!(
        s.eval::<String>("return GetBindLocation()").unwrap(),
        "Ironforge"
    );
}

/// **`RequestTimePlayed()` queues the ask; `TIME_PLAYED_MSG` carries the answer.**
///
/// The two halves are one feature: `QuestHistory` calls the verb and then waits on the event, so
/// shipping the request alone would be 1203 — an addon waiting forever is quieter and worse than
/// the `attempt to call global` it got before.
#[test]
fn request_time_played_queues_an_ask_and_the_answer_arrives_as_an_event() {
    let mut s = crate::script::UiScript::new().unwrap();

    // The verb returns NOTHING — the answer is an event, never a return value.
    assert_eq!(s.arity("RequestTimePlayed()").unwrap(), 0);
    // …and that call queued one ask. A COUNT, not a payload: the packet is empty, so two asks in a
    // frame are two sends rather than one collapsed intent (the pvp queue's rule).
    s.run("RequestTimePlayed() RequestTimePlayed()").unwrap();
    assert_eq!(
        s.take_played_time_asks(),
        3,
        "the eval above plus two more — each ask is its own CMSG_PLAYED_TIME"
    );
    assert_eq!(s.take_played_time_asks(), 0, "the drain empties the queue");

    // The answer half: the app fires the event with (total, level) in seconds.
    s.run(
        r#"
        TPSeen = nil
        local f = CreateFrame("Frame", "TPWatcher")
        f:RegisterEvent("TIME_PLAYED_MSG")
        f:SetScript("OnEvent", function() TPSeen = { arg1, arg2 } end)
        "#,
    )
    .unwrap();
    s.fire_event(
        "TIME_PLAYED_MSG",
        vec![
            crate::script::ScriptValue::Int(360_000),
            crate::script::ScriptValue::Int(7_200),
        ],
    );
    assert_eq!(
        s.eval::<(i64, i64)>("return TPSeen[1], TPSeen[2]").unwrap(),
        (360_000, 7_200),
        "total seconds played and seconds since the last level-up, in that order"
    );
}

/// **The engine hands caught script errors to the CHOSEN Lua error handler** (decision 1305) —
/// the reference's `seterrorhandler` contract, which is how `_ERRORMESSAGE`'s dialog (or an
/// addon's own ImprovedErrorFrame-style handler) ever hears about a failure the engine caught.
#[test]
fn a_chosen_error_handler_hears_engine_caught_errors_and_the_default_does_not_duplicate() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        "local f = CreateFrame('Frame') \
         f:RegisterEvent('B271_PROBE') \
         f:SetScript('OnEvent', function() error('boom from a handler') end)",
    )
    .unwrap();

    // Nobody chose a handler: the error lands on the host channel ONCE, and the dispatch —
    // recognising the stdlib default by identity — adds nothing.
    s.fire_event("B271_PROBE", vec![]);
    s.dispatch_script_errors_to_handler();
    let errors = s.take_errors();
    assert_eq!(
        errors.iter().filter(|e| e.contains("boom")).count(),
        1,
        "one error, once — the default handler must not double-report: {errors:?}"
    );

    // An addon chose one: the dispatch fires it with the message, and the host channel STILL
    // records the error (the dual channel is what keeps the harness's instruments sighted).
    s.run("caught = {} seterrorhandler(function(msg) table.insert(caught, msg) end)")
        .unwrap();
    s.fire_event("B271_PROBE", vec![]);
    s.dispatch_script_errors_to_handler();
    assert!(
        s.eval::<String>("return caught[1]")
            .unwrap()
            .contains("boom from a handler"),
        "the chosen handler received the engine-caught error"
    );
    assert_eq!(
        s.take_errors()
            .iter()
            .filter(|e| e.contains("boom"))
            .count(),
        1,
        "and the host channel recorded it too"
    );
}

/// **Every handler path reaches the chosen handler, not only `OnEvent`.** The widget scripts —
/// `OnClick`, `OnValueChanged`, the edit box's and keyboard's, the tooltip's — pushed their errors
/// onto the host channel alone, so an addon's broken click handler was a terminal line and never
/// the dialog, while the same error from `OnEnter` did pop it (decision 1305's "every
/// engine-caught script error", which these paths had never joined).
#[test]
fn a_widget_handler_error_reaches_the_chosen_error_handler() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        "caught = {} seterrorhandler(function(m) table.insert(caught, m) end) \
         local b = CreateFrame('Button') \
         b:SetScript('OnClick', function() error('click boom') end) \
         b:Click() \
         local sl = CreateFrame('Slider') \
         sl:SetMinMaxValues(0, 10) \
         sl:SetScript('OnValueChanged', function() error('slide boom') end) \
         sl:SetValue(5)",
    )
    .unwrap();
    s.dispatch_script_errors_to_handler();
    let caught = s
        .eval::<String>("return table.concat(caught, ' | ')")
        .unwrap();
    assert!(caught.contains("click boom"), "OnClick: {caught}");
    assert!(caught.contains("slide boom"), "OnValueChanged: {caught}");
}

/// **A listener that unregisters itself does not rob the next one** — on the engine-internal
/// dispatch too (`fire_global`: `ADDON_LOADED` for a `LoadAddOn`, `UPDATE_FACTION` from the
/// faction-header verbs). That walk stepped an index over the live list, so the hook-once idiom
/// (`this:UnregisterEvent(event)` inside the handler) shifted the successor into the slot just
/// visited and it never heard the event — the 1324 bug, fixed in `fire_event` and not here.
#[test]
fn a_self_unregistering_listener_does_not_rob_its_successor_on_the_internal_dispatch() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        r#"log = ""
        local a = CreateFrame("Frame") a:RegisterEvent("UPDATE_FACTION")
        a:SetScript("OnEvent", function() log = log .. "a" this:UnregisterEvent("UPDATE_FACTION") end)
        local b = CreateFrame("Frame") b:RegisterEvent("UPDATE_FACTION")
        b:SetScript("OnEvent", function() log = log .. "b" end)
        ExpandFactionHeader(0)"#,
    )
    .unwrap();
    assert_eq!(s.eval::<String>("return log").unwrap(), "ab");
}

/// **A handler that itself raises cannot recurse the error path** — its failure is recorded on
/// the host channel only, never re-queued, and the batch stops on the first handler failure.
#[test]
fn an_error_handler_that_errors_does_not_recurse() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        "seterrorhandler(function() error('the handler is broken too') end) \
         local f = CreateFrame('Frame') \
         f:RegisterEvent('B271_PROBE') \
         f:SetScript('OnEvent', function() error('original fault') end)",
    )
    .unwrap();
    s.fire_event("B271_PROBE", vec![]);
    s.dispatch_script_errors_to_handler();
    let errors = s.take_errors();
    assert!(
        errors.iter().any(|e| e.contains("original fault")),
        "the original fault is on the host channel: {errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("error handler itself failed")),
        "and so is the handler's own failure, named as such: {errors:?}"
    );
    // A second dispatch finds an EMPTY queue — the handler's failure was never re-queued.
    s.dispatch_script_errors_to_handler();
    assert!(
        s.take_errors().is_empty(),
        "nothing recurses: the queue drained and the handler failure did not re-enter it"
    );
}
