//! `OnSizeChanged` — the base-map layout script (`0x76a0d0` `+0x120`), fired from the resolve pass.
//!
//! Two things are being pinned here, and they are different in kind. The **gate** is byte-verified:
//! `ApplyRect 0x76b580` fires iff `|Δwidth| ≥ ε ∨ |Δheight| ≥ ε` with `ε = _DAT_008029d4`
//! ([`crate::layout::SIZE_EPS`]), which is why a frame that merely MOVES fires nothing. The
//! **seam** is ours: the client applies rects one at a time and fires per application, benilla
//! solves the whole graph to a fixpoint, so we fire on the entry-vs-convergence diff — one fire per
//! resolve per frame whose size actually moved, never one per solver round.

use super::common::script;

/// The size moves ⇒ `OnSizeChanged(self, width, height)` fires with the NEW size; a pure move fires
/// nothing (the byte-verified gate is on width/height, not on the rect).
#[test]
fn a_resize_fires_on_size_changed_and_a_move_does_not() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        fires, w, h = 0, nil, nil
        Panel = CreateFrame("Frame", "SizedPanel")
        Panel:SetPoint("BOTTOMLEFT", 100, 100)
        Panel:SetWidth(200); Panel:SetHeight(50)
        Panel:SetScript("OnSizeChanged", function(self, aw, ah)
            fires = fires + 1 w = aw h = ah
        end)
    "#,
    )
    .unwrap();

    // The FIRST layout is a change too: the client's cached rect starts zeroed, so its first
    // ApplyRect is 0×0 → 200×50 and does fire.
    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 1);
    assert_eq!(s.eval::<f64>("return w").unwrap(), 200.0);
    assert_eq!(s.eval::<f64>("return h").unwrap(), 50.0);

    // A second resolve with nothing touched fires nothing — the size did not move.
    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 1);

    // A MOVE is not a resize.
    s.run(r#"Panel:SetPoint("BOTTOMLEFT", 400, 300)"#).unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<i64>("return fires").unwrap(),
        1,
        "the rect moved but width/height did not — ApplyRect's gate is on the size alone"
    );

    // A real resize does.
    s.run("Panel:SetHeight(120)").unwrap();
    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 2);
    assert_eq!(s.eval::<f64>("return w").unwrap(), 200.0);
    assert_eq!(s.eval::<f64>("return h").unwrap(), 120.0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// It fires for a size the frame did not set on itself — the case the corpus actually uses it for
/// (`MoveAnything`, `CleanMinimap`, `FuBar`, `Outfitter`, `AckisRecipeList` all watch a frame whose
/// size is decided by something else). Here the child is anchored to two edges of a parent that
/// resizes, so its own `LayoutInput` never changes at all and only the SOLVE knows it moved.
#[test]
fn an_anchor_driven_resize_fires_it_too() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        fires, w = 0, nil
        Parent = CreateFrame("Frame", "SizeParent")
        Parent:SetPoint("BOTTOMLEFT", 0, 0)
        Parent:SetWidth(300); Parent:SetHeight(200)
        Child = CreateFrame("Frame", "SizeChild", Parent)
        Child:SetPoint("BOTTOMLEFT", Parent, "BOTTOMLEFT", 0, 0)
        Child:SetPoint("TOPRIGHT", Parent, "TOPRIGHT", 0, 0)
        Child:SetScript("OnSizeChanged", function(self, aw, ah) fires = fires + 1 w = aw end)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(s.eval::<f64>("return w").unwrap(), 300.0);
    let first = s.eval::<i64>("return fires").unwrap();

    s.run("Parent:SetWidth(500)").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<i64>("return fires").unwrap(),
        first + 1,
        "the child's own inputs never changed — only the parent's rect did"
    );
    assert_eq!(s.eval::<f64>("return w").unwrap(), 500.0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// **The loop guard.** A handler that resizes its own frame is the classic `OnSizeChanged` hazard,
/// and it must not spin the engine. The drain takes ONE batch per resolve
/// ([`crate::script::event::fire_size_changes`]), so a handler that settles on a constant size
/// settles — one extra fire for the size it forced, then silence — and one that never settles costs
/// one fire per resolve rather than hanging inside a single call.
#[test]
fn a_handler_that_resizes_its_own_frame_settles_instead_of_spinning() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        fires = 0
        Sq = CreateFrame("Frame", "SquarePanel")
        Sq:SetPoint("BOTTOMLEFT", 0, 0)
        Sq:SetWidth(200); Sq:SetHeight(50)
        -- The idiom: "keep me square". It writes back into the very input that fired it.
        Sq:SetScript("OnSizeChanged", function(self, aw, ah)
            fires = fires + 1
            if ah ~= aw then self:SetHeight(aw) end
        end)
    "#,
    )
    .unwrap();

    // Resolve returns (does not hang) even though the handler dirties the layout from inside it.
    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 1);
    // The next resolve sees the handler's own 200×200 and fires once for it…
    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 2);
    assert_eq!(s.eval::<f64>("return Sq:GetHeight()").unwrap(), 200.0);
    // …and then it is stable: no further fires, however many passes run.
    for _ in 0..5 {
        s.resolve();
    }
    assert_eq!(
        s.eval::<i64>("return fires").unwrap(),
        2,
        "a settling handler settles — the fixpoint is reached, not re-entered forever"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The regression guard that makes the `SCRIPT_KINDS` row real: `SetScript("OnSizeChanged", …)`
/// stopped raising, and what replaced the error is a handler that actually runs — not a stored
/// closure nobody ever calls.
#[test]
fn set_script_on_size_changed_is_accepted_because_the_resolve_pass_fires_it() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        ran = false
        Accepted = CreateFrame("Frame", "SizeAccepted")
        Accepted:SetPoint("BOTTOMLEFT", 0, 0)
        Accepted:SetWidth(10); Accepted:SetHeight(10)
        Accepted:SetScript("OnSizeChanged", function() ran = true end)
    "#,
    )
    .unwrap();
    assert!(s
        .eval::<bool>(r#"return Accepted:GetScript("OnSizeChanged") ~= nil"#)
        .unwrap());
    s.resolve();
    assert!(s.eval::<bool>("return ran").unwrap(), "…and it FIRED");
}

/// **The watch list is exactly the frames carrying the script.** The resolve's "before" snapshot
/// reads `SetScript`'s maintained `on_size_changed_frames` rather than filtering the whole scripts
/// map, so the list itself is now load-bearing: `SetScript(…, nil)` must remove
/// the frame, and re-registering must not enrol it twice.
///
/// Asserted on the LIST, not on the fire count, and that is the point. A stale entry fires nothing
/// — `fire` looks the handler up in Lua and finds nil — so behaviour hides the bug completely and
/// the only symptom is the snapshot silently growing forever, one dead frame at a time. (Written
/// as a fire-count test first; it passed with the `retain` deleted.)
#[test]
fn the_on_size_changed_watch_list_is_exactly_the_frames_carrying_the_script() {
    use crate::script::model::Model;

    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        fires = 0
        Watched = CreateFrame("Frame", "WatchedPanel")
        Watched:SetPoint("BOTTOMLEFT", 0, 0)
        Watched:SetWidth(10); Watched:SetHeight(10)
        Other = CreateFrame("Frame", "UnwatchedPanel")
        Other:SetScript("OnShow", function() end)      -- another kind never enrols
        local bump = function() fires = fires + 1 end
        Watched:SetScript("OnSizeChanged", bump)
        Watched:SetScript("OnSizeChanged", bump)       -- re-registering is not a second enrolment
    "#,
    )
    .unwrap();
    let watched = |s: &crate::script::UiScript| {
        s.lua()
            .app_data_ref::<Model>()
            .expect("model app_data")
            .on_size_changed_frames
            .len()
    };
    assert_eq!(watched(&s), 1, "one frame carries the script");

    s.resolve();
    s.run("Watched:SetWidth(40); Watched:SetHeight(40)")
        .unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<i64>("return fires").unwrap(),
        2,
        "one fire per resolve whose size moved — the 0×0→10×10 birth, then the resize"
    );

    s.run(r#"Watched:SetScript("OnSizeChanged", nil)"#).unwrap();
    assert_eq!(watched(&s), 0, "clearing the script un-watches the frame");
}
