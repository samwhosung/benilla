//! Slider (per-kind behavior; LoadXML `0x789580`, decision 0250). The value/step/orientation
//! contract, the VERTICAL ctor default, the no-swap divergence from StatusBar, and the change-gate
//! that keeps the real scrollbar wiring from recursing.

use super::common::script;
use crate::script::QuadContent;

/// Find the thumb quad (the texture whose path carries `needle`) and return its
/// `(left, right, bottom, top)` rect. Panics if absent — the render must emit it.
fn thumb_rect(quads: &[crate::script::ExtractedQuad], needle: &str) -> (f32, f32, f32, f32) {
    let q = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
        })
        .expect("thumb quad");
    let r = q.rect.expect("thumb rect resolved");
    (r.left, r.right, r.bottom, r.top)
}

#[test]
fn slider_thumb_draws_at_value_fraction_along_the_track() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        local sl = CreateFrame("Slider", "SlRender")
        -- A vertical scrollbar: 16 wide, 100 tall, bottom-left at (100, 100) -> track y in [100, 200].
        sl:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        sl:SetWidth(16); sl:SetHeight(100)
        sl:SetThumbTexture("Interface\\Buttons\\UI-ScrollBar-Knob")
        local t = sl:GetThumbTexture()
        t:SetWidth(16); t:SetHeight(16)
        sl:SetMinMaxValues(0, 100)
        sl:SetValue(0)
    "#,
    )
    .unwrap();
    s.resolve();
    // value=min: the thumb sits flush at the track TOP (a scrollbar at 0 is scrolled to the top).
    // travel = 100 - 16 = 84; thumb centered on cx=108 -> [100,116], top edge at track.top=200.
    assert_eq!(
        thumb_rect(&s.extract(), "Knob"),
        (100.0, 116.0, 184.0, 200.0),
        "value=min: thumb flush at track top, centered on the cross-axis"
    );

    // value=max: the thumb sits flush at the track BOTTOM.
    s.run(r#"SlRender:SetValue(100)"#).unwrap();
    assert_eq!(
        thumb_rect(&s.extract(), "Knob"),
        (100.0, 116.0, 100.0, 116.0),
        "value=max: thumb flush at track bottom"
    );

    // Halfway: thumb centered in the track (midpoint 150).
    s.run(r#"SlRender:SetValue(50)"#).unwrap();
    let (_, _, bottom, top) = thumb_rect(&s.extract(), "Knob");
    assert_eq!(
        (bottom, top),
        (142.0, 158.0),
        "value=mid: thumb centered in the track"
    );
}

#[test]
fn slider_thumb_drag_maps_cursor_to_value() {
    // The same vertical scrollbar as the render test: track y in [100, 200], 16x16 thumb. At value=0
    // the thumb spans y[184,200] (its top edge at the track top), centered on x=108.
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        bar = CreateFrame("Slider", "SlDrag")
        bar:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        bar:SetWidth(16); bar:SetHeight(100)
        bar:SetThumbTexture("Interface\\Buttons\\UI-ScrollBar-Knob")
        local thumb = bar:GetThumbTexture(); thumb:SetWidth(16); thumb:SetHeight(16)
        bar:SetMinMaxValues(0, 100)
        bar:SetValue(0)
    "#,
    )
    .unwrap();
    s.resolve();

    // Grab the thumb at its center (108, 192) and drag down the track. travel = 84; the grab keeps
    // the same thumb point under the cursor, so the value tracks the cursor absolutely.
    s.mouse_button(108.0, 192.0, "LeftButton", true);
    s.mouse_move(108.0, 150.0);
    let v: f32 = s.eval("return SlDrag:GetValue()").unwrap();
    assert_eq!(v, 50.0, "cursor halfway down the track -> value 50");
    s.mouse_move(108.0, 108.0);
    let v: f32 = s.eval("return SlDrag:GetValue()").unwrap();
    assert_eq!(
        v, 100.0,
        "cursor at the track bottom -> value 100 (clamped)"
    );

    // Release ends the capture: a later move must not move the value.
    s.mouse_button(108.0, 108.0, "LeftButton", false);
    s.mouse_move(108.0, 192.0);
    let v: f32 = s.eval("return SlDrag:GetValue()").unwrap();
    assert_eq!(v, 100.0, "no capture after release");
}

#[test]
fn slider_track_press_seats_the_thumb_and_a_mouseless_slider_ignores_it() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        bar = CreateFrame("Slider", "SlTrack")
        bar:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        bar:SetWidth(16); bar:SetHeight(100)
        bar:SetThumbTexture("Interface\\Buttons\\UI-ScrollBar-Knob")
        local thumb = bar:GetThumbTexture(); thumb:SetWidth(16); thumb:SetHeight(16)
        bar:SetMinMaxValues(0, 100)
        bar:SetValue(0)
        fired = {}
        bar:SetScript("OnValueChanged", function() table.insert(fired, arg1) end)
    "#,
    )
    .unwrap();
    s.resolve();

    // A press on the TRACK below the thumb (108, 120) — inside the slider, not on the thumb
    // [184,200] — SEATS the thumb's center under the cursor and fires OnValueChanged from the
    // press itself (0989's track-press law; 0250 §5 covered only the thumb grab). Track
    // y in [100, 200], travel 84: cursor 120 → thumb top 128 → fraction (200−128)/84 = 72/84.
    s.mouse_button(108.0, 120.0, "LeftButton", true);
    let v: f32 = s.eval("return SlTrack:GetValue()").unwrap();
    assert!(
        (v - 100.0 * (72.0 / 84.0)).abs() < 1e-3,
        "the press seats the thumb center at the cursor (got {v})"
    );
    let n: usize = s.eval("return table.getn(fired)").unwrap();
    assert_eq!(n, 1, "the jump fired OnValueChanged once, from the press");

    // The SAME press keeps dragging — the capture began at the track press.
    s.mouse_move(108.0, 108.0);
    let v: f32 = s.eval("return SlTrack:GetValue()").unwrap();
    assert_eq!(v, 100.0, "the gesture drags on without re-grabbing");
    s.mouse_button(108.0, 108.0, "LeftButton", false);

    // A slider the mouse cannot reach ignores a press anywhere — thumb and track alike. This was
    // `SlTrack:Disable()` until the Button-only `Enable`/`Disable`/`IsEnabled` trio came off the
    // Slider table (1.12 registers them on `0x879d00` alone, and the Slider's LoadXML has no
    // `enabled` attribute either). `EnableMouse(false)` is how the reference keeps a slider off a
    // press, and it gates upstream in the hit test rather than inside `begin_drag`.
    s.run(r#"SlTrack:SetValue(0); SlTrack:EnableMouse(false); fired = {}"#)
        .unwrap();
    for y in [192.0, 150.0] {
        s.mouse_button(108.0, y, "LeftButton", true);
        s.mouse_move(108.0, 130.0);
        let v: f32 = s.eval("return SlTrack:GetValue()").unwrap();
        assert_eq!(
            v, 0.0,
            "mouse-disabled slider does not move (press at y={y})"
        );
        s.mouse_button(108.0, 130.0, "LeftButton", false);
    }
    let n: usize = s.eval("return table.getn(fired)").unwrap();
    assert_eq!(n, 0, "mouse disabled: no OnValueChanged at all");
}

#[test]
fn slider_methods_exist_only_on_sliders() {
    let s = script();
    let ok: bool = s
        .eval(
            r#"
        local sl = CreateFrame("Slider", "SlDuck")
        local plain = CreateFrame("Frame")
        -- Duck-typing: addons branch on `if frame.SetValueStep then` — a plain frame must say nil.
        return (type(sl.SetValue) == "function") and (plain.SetValue == nil)
            and (type(sl.SetValueStep) == "function") and (plain.SetValueStep == nil)
            and (type(sl.SetThumbTexture) == "function") and (plain.SetThumbTexture == nil)
            and (type(sl.Show) == "function") -- base methods still reachable through the fallback
    "#,
        )
        .unwrap();
    assert!(ok);
}

#[test]
fn slider_default_orientation_is_vertical() {
    // The load-bearing fact of decision 0250: every scrollbar omits `orientation` and is vertical,
    // so the CSimpleSlider ctor default is VERTICAL — the opposite of StatusBar's HORIZONTAL.
    let s = script();
    s.run(
        r#"
        local sl = CreateFrame("Slider", "SlOrient")
        assert(sl:GetOrientation() == "VERTICAL", "ctor default is VERTICAL")
        sl:SetOrientation("HORIZONTAL")
        assert(sl:GetOrientation() == "HORIZONTAL", "explicit horizontal takes")
    "#,
    )
    .unwrap();
}

#[test]
fn slider_value_clamps_and_does_not_swap_minmax() {
    let s = script();
    s.run(
        r#"
        local sl = CreateFrame("Slider", "SlClamp")
        sl:SetMinMaxValues(0, 100)
        sl:SetValueStep(5)
        assert(sl:GetValueStep() == 5, "step round-trips")
        sl:SetValue(250)
        assert(sl:GetValue() == 100, "clamped to max")
        sl:SetValue(-5)
        assert(sl:GetValue() == 0, "clamped to min")
        -- Unlike StatusBar, a reversed pair is NOT swapped (LoadXML `0x789580` stores min + range).
        sl:SetMinMaxValues(80, 20)
        local mn, mx = sl:GetMinMaxValues()
        assert(mn == 80 and mx == 20, "reversed pair kept as given, not swapped")
    "#,
    )
    .unwrap();
}

#[test]
fn slider_setvalue_fires_onvaluechanged_only_on_change() {
    let s = script();
    s.run(
        r#"
        local sl = CreateFrame("Slider", "SlEvt")
        sl:SetMinMaxValues(0, 10)
        seen = {}
        sl:SetScript("OnValueChanged", function(self, value)
            table.insert(seen, value)
            assert(self == sl and arg1 == value, "handler-firing conventions carry the value")
        end)
        sl:SetValue(4)
        sl:SetValue(4)          -- no change, no fire
        sl:SetMinMaxValues(0, 3) -- re-clamp 4 -> 3: a value change, fires
        assert(table.getn(seen) == 2 and seen[1] == 4 and seen[2] == 3,
               "fired once per actual change: " .. table.getn(seen))
    "#,
    )
    .unwrap();
}

/// `Enable`/`Disable`/`IsEnabled` are a **Button** trio in 1.12 (table `0x879d00`), and a Slider
/// must not answer them: a duck-typing addon that branches on `if widget.IsEnabled then` reads a
/// superset as the wrong class (1189/1250 §5). This asserts the removal *and* its control — the
/// same three names still present on a Button, so a regression that emptied the Button table would
/// fail here rather than pass by accident.
#[test]
fn a_slider_does_not_answer_the_buttons_enable_trio() {
    let s = script();
    s.run(
        r#"
        local sl = CreateFrame("Slider", "SlEnable")
        for _, name in ipairs({ "Enable", "Disable", "IsEnabled" }) do
            assert(sl[name] == nil, "Slider must not answer " .. name)
        end
        local b = CreateFrame("Button", "SlEnableControl")
        for _, name in ipairs({ "Enable", "Disable", "IsEnabled" }) do
            assert(type(b[name]) == "function", "Button still answers " .. name)
        end
        -- The Button's own predicate is the NUMBER 1 / the NUMBER 0, never a Lua boolean — its
        -- false leg is 0 rather than nil, settled per body at `0x7800b0`'s `setne`+`fild`
        -- (decision 2118's `binding_abi::flag` doc).
        assert(b:IsEnabled() == 1, "1, not true")
        b:Disable()
        assert(b:IsEnabled() == 0, "0, not false and not nil")
    "#,
    )
    .unwrap();
}

#[test]
fn slider_get_thumb_texture_returns_a_stable_region() {
    let s = script();
    s.run(
        r#"
        local sl = CreateFrame("Slider", "SlThumb")
        assert(sl:GetThumbTexture() == nil, "no thumb before one is set")
        sl:SetThumbTexture("Interface\\Buttons\\UI-ScrollBar-Knob")
        local t = sl:GetThumbTexture()
        assert(t ~= nil and t == sl:GetThumbTexture(), "stable region wrapper")
    "#,
    )
    .unwrap();
}

#[test]
fn slider_scrollbar_wiring_does_not_recurse() {
    // The real FauxScrollFrame wiring (UIPanelTemplates.xml): the scrollbar Slider's OnValueChanged
    // drives the ScrollFrame's SetVerticalScroll, whose OnVerticalScroll drives the Slider's
    // SetValue back the other way. A fire-always SetValue would recurse forever; the change-gate
    // (SliderState::store_value returns Some only on an actual change) breaks the loop after one hop.
    // This test *is* the recursion — if the gate regressed it would stack-overflow, not assert-fail.
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        local sf = CreateFrame("ScrollFrame", "SlSF")
        sf:SetPoint("TOPLEFT", nil, "TOPLEFT", 0, 0)
        sf:SetWidth(100); sf:SetHeight(100)
        local child = CreateFrame("Frame", "SlSFChild", sf)
        child:SetWidth(100); child:SetHeight(300)  -- 200px taller than the frame -> scroll range 200
        sf:SetScrollChild(child)

        bar = CreateFrame("Slider", "SlSFBar")
        bar:SetMinMaxValues(0, 200)
        bar:SetScript("OnValueChanged", function() sf:SetVerticalScroll(arg1) end)
        sf:SetScript("OnVerticalScroll", function() bar:SetValue(arg1) end)
    "#,
    )
    .unwrap();
    // A resolve populates the rects SetVerticalScroll's live range clamps against.
    s.resolve();
    s.run(
        r#"
        bar:SetValue(50)
        assert(bar:GetValue() == 50, "slider settled at 50")
        assert(SlSF:GetVerticalScroll() == 50, "scroll followed the slider")
    "#,
    )
    .unwrap();
}

#[test]
fn slider_value_bits_gate_the_first_fire_and_the_range_reclamp() {
    // The client's `+0x314` bit1 (has a range) and bit2 (has a value), byte-read off
    // `SetValue 0x789930` / `SetMinMaxValues 0x7898f0`:
    // rangeless SetValue is a no-op; the FIRST SetValue always fires, even at the zero-init value;
    // SetMinMaxValues re-clamps through SetValue only once a value exists. Decision 2095 — Atlas's
    // option sliders raised from their own <OnLoad> because our SetMinMaxValues clamped the
    // zero-init value into (0.25, 1) and ran a handler the addon had not armed yet.
    let s = script();
    s.run(
        r#"
        seen = {}
        local sl = CreateFrame("Slider", "SlBits")
        sl:SetScript("OnValueChanged", function() table.insert(seen, arg1) end)

        sl:SetValue(7)                       -- bit1 clear: a complete no-op
        assert(sl:GetValue() == 0, "rangeless SetValue stores nothing: " .. sl:GetValue())
        assert(table.getn(seen) == 0, "rangeless SetValue fires nothing")

        sl:SetMinMaxValues(0.25, 1)          -- bit2 clear: the range excludes 0, still no fire
        assert(table.getn(seen) == 0, "SetMinMaxValues on a fresh slider fires nothing")
        assert(sl:GetValue() == 0, "and does not clamp a value that does not exist")

        sl:SetMinMaxValues(0, 10)
        sl:SetValue(0)                       -- first-ever value == zero-init: fires anyway
        assert(table.getn(seen) == 1 and seen[1] == 0, "the first SetValue always fires")
        sl:SetValue(0)                       -- now the change-gate holds
        assert(table.getn(seen) == 1, "an equal value after the first does not fire")

        sl:SetValue(8)
        sl:SetMinMaxValues(0, 5)             -- bit2 set: re-clamp 8 -> 5 fires
        assert(table.getn(seen) == 3 and seen[3] == 5, "a held value re-clamps and fires")
        sl:SetMinMaxValues(0, 20)            -- 5 stays 5: no fire
        assert(table.getn(seen) == 3, "a re-range that moves nothing fires nothing")
    "#,
    )
    .unwrap();
}

#[test]
fn slider_onload_range_does_not_run_an_unarmed_onvaluechanged() {
    // Atlas's exact shape (AtlasOptions.xml): the slider's <OnLoad> sets a range whose low end is
    // above zero, and its <OnValueChanged> indexes a saved-variables global that only exists after
    // ADDON_LOADED. In the reference the load raises nothing; before 2095 ours raised
    // "attempt to index global 'AtlasOptions' (a nil value)" out of the OnLoad.
    let mut s = script();
    let doc = crate::framexml::parse(
        r#"<Ui>
          <Slider name="SlAtlasAlpha">
            <Scripts>
              <OnLoad>this:SetMinMaxValues(0.25, 1); this:SetValueStep(0.05)</OnLoad>
              <OnValueChanged>SlAtlasOpts.alpha = this:GetValue()</OnValueChanged>
            </Scripts>
          </Slider>
        </Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(
        report.errors.is_empty() && s.take_errors().is_empty(),
        "an OnLoad range on a fresh slider must not run OnValueChanged: {:?}",
        report.errors
    );
    s.run(
        r#"
        SlAtlasOpts = {}
        SlAtlasAlpha:SetValue(0.5)
        assert(SlAtlasOpts.alpha == 0.5, "the first real SetValue reaches the handler")
    "#,
    )
    .unwrap();
}

#[test]
fn a_thumb_with_no_authored_size_takes_its_arts_texel_span_and_still_drags() {
    // Dewdrop-2.0's `OpenSlider` (Cartographer 2.02 → Look 'n' Feel → Overlay transparency), in
    // its own construction order — the shape that has no `<ThumbTexture><Size>` anywhere:
    // `SetThumbTexture(path)` from Lua and nothing else. The reference asks the thumb for its own
    // `GetWidth`/`GetHeight` (`0x789ba0` → `[thumb_vt+0x1c]/[+0x20]` = `CSimpleTexture::0x770720`
    // /`0x770790`), which falls back to the art's native texel span — 32×32 for
    // `UI-SliderBar-Button-Vertical` — so the thumb is a 32-unit knob on a 128-unit track with
    // 96 units of travel. Ours read the *authored* size only and, finding none, sized the thumb to
    // the whole track: zero travel, a thumb smeared over the bar, and a drag that cannot move.
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.set_texture_size_probe(Box::new(|p| {
        p.contains("UI-SliderBar-Button-Vertical")
            .then_some((32, 32))
    }));
    s.run(
        r#"
        -- The popout is a parentless FULLSCREEN_DIALOG frame that takes the mouse itself, with
        -- the slider one frame level above it — so this also pins that the press resolves to the
        -- Slider and not to the mouse-enabled host sitting under it.
        host = CreateFrame("Frame", "DdHost", nil)
        host:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        host:SetWidth(80); host:SetHeight(170)
        host:SetFrameStrata("FULLSCREEN_DIALOG")
        host:EnableMouse(true)
        bar = CreateFrame("Slider", "DdSlider", host)
        bar:SetFrameLevel(host:GetFrameLevel() + 1)
        bar:SetOrientation("VERTICAL")
        bar:SetMinMaxValues(0, 1)
        bar:SetValueStep(0.01)
        bar:SetValue(0.5)
        bar:SetWidth(16)
        bar:SetHeight(128)
        bar:SetPoint("LEFT", host, "LEFT", 15, 0)
        bar:SetThumbTexture("Interface\\Buttons\\UI-SliderBar-Button-Vertical")
        -- Dewdrop then seats the open value: overlayAlpha is 100 % of a 25 %..100 % range, and
        -- the popout inverts it (`1 - (value-min)/(max-min)`), so the engine value is 0 = the
        -- TOP of a vertical track.
        bar:SetValue(0)
    "#,
    )
    .unwrap();
    s.resolve();

    // host y ∈ [100, 270] ⇒ the bar is centred on y=185, 128 tall ⇒ track y ∈ [121, 249];
    // x: host left 100 + 15 ⇒ [115, 131], so the thumb centres on x=123.
    assert_eq!(
        thumb_rect(&s.extract(), "UI-SliderBar-Button-Vertical"),
        (107.0, 139.0, 217.0, 249.0),
        "a sizeless thumb is its art's 32×32, flush at the track top — not the whole 16×128 track"
    );

    // And it drags: grab the thumb at its centre and pull to the bottom of the track.
    s.mouse_button(123.0, 233.0, "LeftButton", true);
    s.mouse_move(123.0, 137.0);
    let v: f32 = s.eval("return DdSlider:GetValue()").unwrap();
    assert!(
        (v - 1.0).abs() < 1e-3,
        "travel is 128−32 = 96, so a 96-unit pull runs the value min→max (got {v})"
    );
}

#[test]
fn slider_setvalue_quantises_onto_the_min_anchored_lattice() {
    // `SetValue 0x789930` rounds the clamped value onto `min + n·step` (round-half-away-from-zero,
    // truncated by `__ftol`) BEFORE it stores — so `GetValue` on a stepped slider can only ever
    // answer a lattice point, and the lattice is anchored at `min`, not at zero. Decision 2095
    // named this divergence and left it; this is it closed.
    let s = script();
    s.run(
        r#"
        q = CreateFrame("Slider", "SlQuant")
        q:SetMinMaxValues(0.2, 1.2)
        q:SetValueStep(0.5)
    "#,
    )
    .unwrap();
    // The lattice is 0.2 / 0.7 / 1.2 — 0.5 and 1.0 are NOT on it, which is the whole point of
    // anchoring at `min`.
    for (set, want) in [
        (0.6, 0.7),
        (0.95, 0.7),
        (0.4, 0.2),
        (1.2, 1.2),
        (0.2, 0.2),
        (0.0, 0.2), // clamped to min first, then quantised
    ] {
        s.run(&format!("SlQuant:SetValue({set})")).unwrap();
        let v: f32 = s.eval("return SlQuant:GetValue()").unwrap();
        assert!(
            (v - want).abs() < 1e-5,
            "SetValue({set}) settles on {want}, got {v}"
        );
    }
}

#[test]
fn a_zero_step_leaves_the_value_continuous() {
    // The gate that keeps every scrollbar working: `UIPanelScrollBarTemplate` declares no
    // `valueStep`, so `[+0x324]` stays at the ctor's 0.0 and `0x78999c`'s `fcomp(step, 0.0)`
    // skips the quantiser outright. A stepless slider stores exactly what it was handed.
    let s = script();
    s.run(
        r#"
        c = CreateFrame("Slider", "SlCont")
        c:SetMinMaxValues(0, 1)
        c:SetValue(0.375)
    "#,
    )
    .unwrap();
    let v: f32 = s.eval("return SlCont:GetValue()").unwrap();
    assert_eq!(v, 0.375, "no step means no lattice");
    let step: f32 = s.eval("return SlCont:GetValueStep()").unwrap();
    assert_eq!(step, 0.0, "the ctor's step is 0.0, and nothing set one");
}

#[test]
fn the_quantised_value_is_not_re_clamped_and_may_pass_max() {
    // The clamp runs BEFORE the quantiser and there is no second clamp after it (`0x789a06`
    // stores whatever `fild n; fmul step; fadd min` produced). So a range that is not a whole
    // number of steps rounds *past* its own max at the top of the travel. This looks like a bug
    // and is the client's arithmetic; it is unreachable in the shipped UI because every stepped
    // slider the reference ships is an exact multiple of its step.
    let s = script();
    s.run(
        r#"
        o = CreateFrame("Slider", "SlOver")
        o:SetMinMaxValues(0, 2.5)
        o:SetValueStep(1)
        o:SetValue(2.5)
    "#,
    )
    .unwrap();
    let v: f32 = s.eval("return SlOver:GetValue()").unwrap();
    assert_eq!(v, 3.0, "2.5 clamps to 2.5, then rounds half-away to 3·step");
    let (min, max): (f32, f32) = s.eval("return SlOver:GetMinMaxValues()").unwrap();
    assert_eq!(
        (min, max),
        (0.0, 2.5),
        "and the range itself is untouched — the overshoot is in the value alone"
    );
}

#[test]
fn a_faux_scrollframes_rows_snap_and_its_bottom_is_exact() {
    // The falsifier decision 2095 named for this change, run on the shape it names.
    // `FauxScrollFrame_Update` builds its range as `(numItems − numToDisplay) · valueStep`
    // (`UIPanelTemplates.lua:180`) and then sets that same step, so the range IS a whole number
    // of steps: every drag lands on a row boundary and the bottom of the travel is exactly max.
    // 20 items, 10 on screen, 16 units a row ⇒ range 0..160, step 16.
    let s = script();
    s.run(
        r#"
        f = CreateFrame("Slider", "SlFaux")
        f:SetMinMaxValues(0, 160)
        f:SetValueStep(16)
    "#,
    )
    .unwrap();
    for (set, want) in [
        (23.9, 16.0), // just under the half-step boundary: the row below
        (24.1, 32.0), // just over it: the row above
        (37.0, 32.0),
        (160.0, 160.0), // the bottom is exact — no overshoot, the range is 10 steps
        (0.0, 0.0),
    ] {
        s.run(&format!("SlFaux:SetValue({set})")).unwrap();
        let v: f32 = s.eval("return SlFaux:GetValue()").unwrap();
        assert_eq!(v, want, "SetValue({set}) snaps to the row at {want}");
        // FauxScrollFrame_GetOffset's own arithmetic, which is what the row list reads.
        let off: f32 = s.eval("return floor(SlFaux:GetValue() / 16)").unwrap();
        assert_eq!(off, (want / 16.0).floor(), "and the row offset follows it");
    }
}

#[test]
fn a_move_inside_one_step_fires_nothing() {
    // The quantiser sits INSIDE the change-gate (`0x789a0b`'s `fcom` is against the stored value,
    // and what is stored is already quantised), so a drag that stays within one step's band
    // resolves to the same lattice point and never reaches `OnValueChanged`. Before this, every
    // pixel of a stepped drag fired.
    let s = script();
    s.run(
        r#"
        fires = 0
        g = CreateFrame("Slider", "SlGate")
        g:SetScript("OnValueChanged", function() fires = fires + 1 end)
        g:SetMinMaxValues(0, 160)
        g:SetValueStep(16)
        g:SetValue(32)      -- first-ever value: always fires
    "#,
    )
    .unwrap();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 1);
    for v in [30.0, 34.0, 39.9, 24.1] {
        s.run(&format!("SlGate:SetValue({v})")).unwrap();
    }
    assert_eq!(
        s.eval::<i64>("return fires").unwrap(),
        1,
        "four moves inside the 24..40 band are one lattice point: no further fire"
    );
    s.run("SlGate:SetValue(40.1)").unwrap();
    assert_eq!(
        s.eval::<i64>("return fires").unwrap(),
        2,
        "crossing into the next band fires once"
    );
}

#[test]
fn set_value_step_re_quantises_the_held_value_and_can_fire() {
    // `SetValueStep 0x789a60` stores the step and then re-pushes the range through
    // `SetMinMaxValues`, which re-clamps the held value through `SetValue` — onto the NEW lattice
    // (decision 2143). A step handed to a slider that already holds a value moves it and fires.
    let s = script();
    s.run(
        r#"
        fires = 0
        v = CreateFrame("Slider", "SlStep")
        v:SetScript("OnValueChanged", function() fires = fires + 1 end)
        v:SetMinMaxValues(0, 100)
        v:SetValue(37)          -- first-ever value: always fires, no step yet, so raw
    "#,
    )
    .unwrap();
    assert_eq!(s.eval::<f32>("return SlStep:GetValue()").unwrap(), 37.0);
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 1);

    // A step arriving AFTER the value snaps it: 37 → 40 on a 10-lattice, and the move fires.
    s.run("SlStep:SetValueStep(10)").unwrap();
    assert_eq!(
        s.eval::<f32>("return SlStep:GetValue()").unwrap(),
        40.0,
        "the step re-quantised a value that was already stored"
    );
    assert_eq!(
        s.eval::<i64>("return fires").unwrap(),
        2,
        "…and the move fired OnValueChanged, exactly as a SetValue would"
    );

    // A step that moves nothing fires nothing — the same change gate, one level up.
    s.run("SlStep:SetValueStep(20)").unwrap();
    assert_eq!(s.eval::<f32>("return SlStep:GetValue()").unwrap(), 40.0);
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 2);
}

#[test]
fn a_step_before_any_range_is_the_whole_call() {
    // `0x789a6c test byte [+0x314],2` — bit1 clear ⇒ the step is stored and the function returns.
    // No range is pushed, nothing is clamped, nothing fires. It is what keeps an `<OnLoad>` that
    // sets a step before a range from running a handler the addon has not armed (2095's subject,
    // one verb over).
    let s = script();
    s.run(
        r#"
        fires = 0
        n = CreateFrame("Slider", "SlNoRange")
        n:SetScript("OnValueChanged", function() fires = fires + 1 end)
        n:SetValueStep(0.25)
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<f32>("return SlNoRange:GetValueStep()").unwrap(),
        0.25
    );
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 0);
    let (min, max): (f32, f32) = s.eval("return SlNoRange:GetMinMaxValues()").unwrap();
    assert_eq!((min, max), (0.0, 0.0), "no range was pushed");
}
