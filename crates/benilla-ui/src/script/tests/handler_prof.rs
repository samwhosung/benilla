//! Per-handler cost attribution. The tests assert names, call counts and the self/total relation,
//! never a duration; `self == total` with nothing nested is the control for `self < total`.

use super::common::script;
use crate::script::{HandlerRow, ScriptValue, UiScript};

fn row(rows: &[HandlerRow], frame: &str, script: &str) -> Option<HandlerRow> {
    rows.iter()
        .find(|r| r.frame == frame && r.script == script)
        .cloned()
}

/// Recording, with no periodic report: the tests read the window directly.
fn armed() -> UiScript {
    let s = script();
    s.profile_handlers(true, 0.0);
    s
}

#[test]
fn every_fired_handler_is_attributed_to_its_own_frame_and_script() {
    let mut s = armed();
    s.run(
        r#"
        local a = CreateFrame("Frame", "Alpha")
        a:SetScript("OnUpdate", function() end)
        local b = CreateFrame("Frame", "Beta")
        b:SetScript("OnUpdate", function() end)
        b:RegisterEvent("UNIT_HEALTH")
        b:SetScript("OnEvent", function() end)
        local quiet = CreateFrame("Frame", "Quiet")
        "#,
    )
    .unwrap();

    s.tick(0.016);
    s.tick(0.016);
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    let rows = s.handler_profile();
    assert_eq!(
        s.handler_profile_frames(),
        2,
        "two ticks is two frames of denominator"
    );
    assert_eq!(
        row(&rows, "Alpha", "OnUpdate").map(|r| r.calls),
        Some(2),
        "one OnUpdate call per tick: {rows:#?}"
    );
    assert_eq!(row(&rows, "Beta", "OnUpdate").map(|r| r.calls), Some(2));
    assert_eq!(
        row(&rows, "Beta", "OnEvent").map(|r| r.calls),
        Some(1),
        "the event fan-out is attributed too, and separately from the tick"
    );
    assert!(
        rows.iter().all(|r| r.frame != "Quiet"),
        "a frame with no handler must not appear at all: {rows:#?}"
    );
}

#[test]
fn a_handler_with_nothing_nested_inside_it_is_all_self_time() {
    // The control for the nesting test: nothing nested subtracts zero, so the two are identical.
    let mut s = armed();
    s.run(
        r#"
        local f = CreateFrame("Frame", "Flat")
        f:SetScript("OnUpdate", function()
            local n = 0
            for i = 1, 20000 do n = n + i end
        end)
        "#,
    )
    .unwrap();
    s.tick(0.016);

    let flat =
        row(&s.handler_profile(), "Flat", "OnUpdate").expect("the flat handler is attributed");
    assert_eq!(
        flat.self_us, flat.total_us,
        "nothing nested ⇒ self is total, exactly"
    );
}

#[test]
fn a_handler_that_fires_another_is_not_charged_for_its_child() {
    let mut s = armed();
    s.run(
        r#"
        inner = CreateFrame("Frame", "Inner")
        inner:Hide()
        inner:SetScript("OnShow", function()
            local n = 0
            for i = 1, 200000 do n = n + i end
        end)
        local outer = CreateFrame("Frame", "Outer")
        outer:SetScript("OnUpdate", function()
            inner:Hide()
            inner:Show()   -- fires Inner's OnShow from inside Outer's OnUpdate
        end)
        "#,
    )
    .unwrap();
    s.tick(0.016);
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    let rows = s.handler_profile();
    let outer = row(&rows, "Outer", "OnUpdate").expect("the outer handler is attributed");
    let inner = row(&rows, "Inner", "OnShow").expect("the nested handler gets its own row");
    assert_eq!(inner.calls, 1);
    assert!(
        outer.self_us < outer.total_us,
        "the child's cost must come off the parent's self time: {outer:#?}"
    );
    assert!(
        inner.self_us > 0.0,
        "the child's own cost lands on the child: {inner:#?}"
    );

    // A second tick behaves like the first: a leaked stack level would charge the child's time to
    // a phantom parent.
    s.tick(0.016);
    let rows = s.handler_profile();
    let outer = row(&rows, "Outer", "OnUpdate").expect("outer");
    assert_eq!(outer.calls, 2);
    assert_eq!(row(&rows, "Inner", "OnShow").map(|r| r.calls), Some(2));
    assert!(outer.self_us < outer.total_us, "{outer:#?}");
}

#[test]
fn an_anonymous_frames_handler_is_labelled_by_where_it_was_written() {
    // An unnamed frame's OnUpdate is the addon timer idiom, and `#4127` would be a useless label.
    let mut s = armed();
    s.run(
        r#"
        local f = CreateFrame("Frame")
        f:SetScript("OnUpdate", function() end)
        "#,
    )
    .unwrap();
    s.tick(0.016);

    let rows = s.handler_profile();
    let [only] = &rows[..] else {
        panic!("exactly one handler fired: {rows:#?}");
    };
    assert!(
        !only.frame.starts_with('#') && only.frame.contains(':'),
        "an unnamed frame is labelled by its handler's chunk and line, not by its id: {only:#?}"
    );
}

#[test]
fn an_unarmed_vm_records_nothing() {
    // An unarmed VM records nothing while another records: the gate (`handler_prof::ARMED`) is
    // process-wide, the state per VM.
    let _recording = armed();
    let mut s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame", "Silent")
        f:SetScript("OnUpdate", function() end)
        "#,
    )
    .unwrap();
    s.tick(0.016);

    assert!(
        s.handler_profile().is_empty(),
        "an unarmed VM records nothing: {:#?}",
        s.handler_profile()
    );
    assert_eq!(s.handler_profile_frames(), 0);
}

#[test]
fn disarming_clears_the_window() {
    let mut s = armed();
    s.run(
        r#"
        local f = CreateFrame("Frame", "Cleared")
        f:SetScript("OnUpdate", function() end)
        "#,
    )
    .unwrap();
    s.tick(0.016);
    assert!(!s.handler_profile().is_empty());

    s.profile_handlers(false, 0.0);
    assert!(s.handler_profile().is_empty(), "disarming drops the window");
    s.tick(0.016);
    assert!(
        s.handler_profile().is_empty(),
        "and a disarmed VM keeps recording nothing"
    );
}
