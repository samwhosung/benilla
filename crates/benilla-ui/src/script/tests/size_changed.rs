//! `OnSizeChanged`, the base-map layout script (`0x76a0d0` `+0x120`). `ApplyRect 0x76b580` fires
//! it only when width or height moves by `ε = _DAT_008029d4` ([`crate::layout::SIZE_EPS`]).
//! Deviation: the client fires per rect application; we fire once per resolve on the size before
//! and after, because our solver settles the whole graph to a fixpoint.

use super::common::script;
use crate::script::UiScript;

/// A fired size, in layout units, over one root unit's store: the root-unit size it carries.
fn root(s: &UiScript, layout: f64) -> f64 {
    layout / f64::from(s.model_ref().layout_units().store(1.0))
}

/// `0x76b623`–`0x76b63e` hand slot `+0x48` the layout rect's own `right − left` and `top − bottom`,
/// rounded to f32, which `0x76b660` pushes as doubles: the stored size, with no getter steps and
/// no divide by the frame's scale. At 4:3 the store is `v · 0.8 / 1024`.
#[test]
fn on_size_changed_passes_the_layout_rects_own_size() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Plain = CreateFrame("Frame", "PlainPanel")
        Plain:SetPoint("BOTTOMLEFT", 0, 0)
        Plain:SetWidth(100); Plain:SetHeight(40)
        Plain:SetScript("OnSizeChanged", function() PW, PH = arg1, arg2 end)
        Big = CreateFrame("Frame", "ScaledPanel")
        Big:SetScale(2)
        Big:SetPoint("BOTTOMLEFT", 0, 0)
        Big:SetWidth(100); Big:SetHeight(40)
        Big:SetScript("OnSizeChanged", function() BW, BH = arg1, arg2 end)
    "#,
    )
    .unwrap();
    s.resolve();
    let fired: (f64, f64, f64, f64) = s.eval("return PW, PH, BW, BH").unwrap();
    assert_eq!(fired, (0.078125, 0.03125, 0.15625, 0.0625));
    crate::assert_near!(s.eval::<f64>("return Big:GetWidth()").unwrap(), 100.0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `OnSizeChanged(self, width, height)` carries the new size.
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
        Panel:SetScript("OnSizeChanged", function() local self, aw, ah = this, arg1, arg2
            fires = fires + 1 w = aw h = ah
        end)
    "#,
    )
    .unwrap();

    // The client's cached rect starts zeroed, so the first layout fires too.
    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 1);
    crate::assert_near!(root(&s, s.eval::<f64>("return w").unwrap()), 200.0);
    crate::assert_near!(root(&s, s.eval::<f64>("return h").unwrap()), 50.0);

    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 1);

    s.run(r#"Panel:SetPoint("BOTTOMLEFT", 400, 300)"#).unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<i64>("return fires").unwrap(),
        1,
        "the rect moved but width/height did not — ApplyRect's gate is on the size alone"
    );

    s.run("Panel:SetHeight(120)").unwrap();
    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 2);
    crate::assert_near!(root(&s, s.eval::<f64>("return w").unwrap()), 200.0);
    crate::assert_near!(root(&s, s.eval::<f64>("return h").unwrap()), 120.0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Addons watch frames sized by their anchors; only the solve knows this child moved.
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
        Child:SetScript("OnSizeChanged", function() local self, aw, ah = this, arg1, arg2 fires = fires + 1 w = aw end)
    "#,
    )
    .unwrap();
    s.resolve();
    crate::assert_near!(root(&s, s.eval::<f64>("return w").unwrap()), 300.0);
    let first = s.eval::<i64>("return fires").unwrap();

    s.run("Parent:SetWidth(500)").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<i64>("return fires").unwrap(),
        first + 1,
        "the child's own inputs never changed — only the parent's rect did"
    );
    crate::assert_near!(root(&s, s.eval::<f64>("return w").unwrap()), 500.0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The drain takes one batch per resolve ([`crate::script::event::fire_size_changes`]), so a
/// handler that resizes its own frame costs one fire per resolve instead of hanging.
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
        -- "Keep me square": it writes back into the very input that fired it.
        Sq:SetScript("OnSizeChanged", function() local self, aw, ah = this, arg1, arg2
            fires = fires + 1
            if ah ~= aw then self:SetHeight(200) end
        end)
    "#,
    )
    .unwrap();

    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 1);
    // The next resolve fires once for the handler's own 200×200.
    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 2);
    crate::assert_near!(s.eval::<f64>("return Sq:GetHeight()").unwrap(), 200.0);
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

/// The resolve snapshots `on_size_changed_frames`, which `SetScript` maintains. Asserted on the
/// list itself: a stale entry fires nothing, so only the list would show it.
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

/// The pump drains the layout before its walk (`0x7656ba` → `0x768ed0`), and the drain's
/// `ApplyRect` fires `OnSizeChanged`: a resize made in an event handler fires it ahead of that
/// tick's `OnUpdate`.
#[test]
fn a_resize_from_an_event_fires_on_size_changed_before_the_walk() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        LOG = {}
        P = CreateFrame("Frame", "DrainPanel")
        P:SetPoint("BOTTOMLEFT", 0, 0)
        P:SetWidth(10); P:SetHeight(10)
        P:RegisterEvent("PLAYER_ENTERING_WORLD")
        P:SetScript("OnEvent", function() this:SetWidth(50) end)
        P:SetScript("OnSizeChanged", function() table.insert(LOG, "size") W = arg1 end)
        P:SetScript("OnUpdate", function() table.insert(LOG, "update") end)
    "#,
    )
    .unwrap();
    s.resolve();
    s.tick(0.016);
    s.run("LOG = {}").unwrap();
    s.queue_event("PLAYER_ENTERING_WORLD", Vec::new());
    s.tick(0.016);
    assert_eq!(
        s.eval::<String>("return table.concat(LOG, ' ')").unwrap(),
        "size update"
    );
    crate::assert_near!(root(&s, s.eval::<f64>("return W").unwrap()), 50.0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The pump drains again after its walk (`0x765799`), so a resize made in an `OnUpdate` fires
/// `OnSizeChanged` inside the same tick, before the host reads the frame.
#[test]
fn a_resize_from_on_update_fires_on_size_changed_in_the_same_tick() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        LOG = {}
        P = CreateFrame("Frame", "DrainPanel")
        P:SetPoint("BOTTOMLEFT", 0, 0)
        P:SetWidth(10); P:SetHeight(10)
        P:SetScript("OnSizeChanged", function() table.insert(LOG, "size") W = arg1 end)
        P:SetScript("OnUpdate", function()
            table.insert(LOG, "update")
            if GROW then GROW = nil; this:SetWidth(70) end
        end)
    "#,
    )
    .unwrap();
    s.resolve();
    s.tick(0.016);
    s.run("LOG = {} GROW = true").unwrap();
    s.tick(0.016);
    assert_eq!(
        s.eval::<String>("return table.concat(LOG, ' ')").unwrap(),
        "update size"
    );
    crate::assert_near!(root(&s, s.eval::<f64>("return W").unwrap()), 70.0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `ApplyRect` compares the size move against 2⁻²² (`0x76b5e7`, `0x76b602`, `[0x8029d4]`) in the
/// layout units its rects are in, where the screen diagonal is 1: about 3.05e-4 of a root unit at
/// 4:3. A jitter under it fires nothing; a real move fires.
#[test]
fn a_sub_threshold_jitter_fires_nothing_and_a_real_move_fires() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        fires = 0
        P = CreateFrame("Frame", "JitterPanel")
        P:SetPoint("BOTTOMLEFT", 0, 0)
        P:SetWidth(100); P:SetHeight(40)
        P:SetScript("OnSizeChanged", function() fires = fires + 1 end)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 1, "the birth");
    // 1e-4 is a thousand times 2⁻²² but a third of the reference's threshold here.
    s.run("P:SetWidth(100.0001); P:SetHeight(40.0001)").unwrap();
    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 1, "a jitter");
    s.run("P:SetWidth(101)").unwrap();
    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 2, "a real move");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
