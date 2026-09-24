//! The nameplate ABI as four vanilla addons read it, one idiom per test:
//! `pfUI/modules/nameplates.lua`, `ShaguTweaks/libs/libnameplate.lua`,
//! `CustomNameplates/CustomNameplates.lua` and `_Nameplates/_Nameplates.lua`.

use crate::script::{PlateGeometry, PlateState, UiScript};

/// A VM with a `WorldFrame` to hang plates off, all [`UiScript::sync_nameplates`] requires.
fn vm() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(r#"WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()"#)
        .unwrap();
    s.resolve();
    s
}

/// A 1280 px window, where one gx unit is 1280 px, so the 0.1 × 0.025 plate is 128 × 32.
fn geometry() -> PlateGeometry {
    PlateGeometry {
        width: 128.0,
        height: 32.0,
        bar_off_x: 4.0,
        bar_off_y: 4.0,
        bar_width: 103.0,
        bar_height: 9.0,
        level_off_x: 11.8,
        level_off_y: 9.1,
        skull_size: 12.8,
        raid_size: 25.6,
        name_height: 12.8,
        level_height: 11.0,
        shadow_offset: 1.0,
    }
}

fn plate(name: &str, health: f32, max: f32) -> PlateState {
    PlateState {
        // A hash of the name stands in for the GUID: stable per unit, distinct between units.
        key: name.bytes().map(u64::from).sum(),
        top_centre: (500.0, 400.0),
        health,
        max_health: max,
        bar_colour: [1.0, 0.0, 0.0],
        name: name.to_string(),
        level: Some(12),
        skull: false,
        level_colour: [1.0, 1.0, 0.0],
        raid_icon: None,
        alpha: 1.0,
        lit: false,
        hovered: false,
    }
}

/// Drive N plates and settle the layout, the way one app frame does.
fn drive(s: &mut UiScript, states: &[PlateState]) {
    s.sync_nameplates(geometry(), states);
    s.resolve();
}

/// pfUI and ShaguTweaks walk only the children past a count they never reset, so the list must not
/// shrink or reorder: a retired plate keeps its slot.
#[test]
fn the_worldframe_child_list_only_grows_and_keeps_its_order() {
    let mut s = vm();
    assert_eq!(
        s.eval::<i64>("return WorldFrame:GetNumChildren()").unwrap(),
        0
    );

    drive(
        &mut s,
        &[plate("Wolf", 30.0, 40.0), plate("Bear", 10.0, 90.0)],
    );
    assert_eq!(
        s.eval::<i64>("return WorldFrame:GetNumChildren()").unwrap(),
        2
    );

    // One unit leaves: its plate hides, and the list keeps its length.
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    assert_eq!(
        s.eval::<i64>("return WorldFrame:GetNumChildren()").unwrap(),
        2
    );
    assert!(s
        .eval::<bool>(
            "local a, b = WorldFrame:GetChildren() return a:IsShown() == 1 and b:IsShown() == nil"
        )
        .unwrap());

    // A new unit takes the pooled plate back: the count stays 2.
    drive(
        &mut s,
        &[plate("Wolf", 30.0, 40.0), plate("Boar", 5.0, 50.0)],
    );
    assert_eq!(
        s.eval::<i64>("return WorldFrame:GetNumChildren()").unwrap(),
        2
    );
    assert_eq!(s.nameplate_pool_len(), 2);
}

/// pfUI's `IsNamePlate` (`nameplates.lua:70`) and ShaguTweaks' (`libnameplate.lua:12`): a `Button`
/// whose first region is a `Texture` reading `Interface\Tooltips\Nameplate-Border`.
#[test]
fn a_plate_identifies_by_button_plus_the_border_texture() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    assert!(s
        .eval::<bool>(
            r#"
            local plate = WorldFrame:GetChildren()
            if plate:GetObjectType() ~= "Button" then return false end
            local region = plate:GetRegions()
            if not region or not region.GetTexture then return false end
            if region:GetObjectType() ~= "Texture" then return false end
            return region:GetTexture() == "Interface\\Tooltips\\Nameplate-Border"
        "#
        )
        .unwrap());
}

/// Six regions in `CustomNameplates.lua:212`'s order (`Border, Glow, Name, Level, Boss,
/// RaidTargetIcon`) and one child; pfUI blanks a seventh region, and a second child as a cast bar.
#[test]
fn the_region_and_child_tuples_are_six_and_one() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);

    assert_eq!(
        s.eval::<i64>("local p = WorldFrame:GetChildren() return p:GetNumRegions()")
            .unwrap(),
        6
    );
    assert_eq!(
        s.eval::<i64>("local p = WorldFrame:GetChildren() return p:GetNumChildren()")
            .unwrap(),
        1
    );
    assert!(s
        .eval::<bool>(
            r#"
            local p = WorldFrame:GetChildren()
            local border, glow, name, level, levelicon, raidicon = p:GetRegions()
            return border:GetTexture()   == "Interface\\Tooltips\\Nameplate-Border"
               and glow:GetTexture()     == "Interface\\Tooltips\\Nameplate-Glow"
               and name:GetObjectType()  == "FontString"
               and level:GetObjectType() == "FontString"
               and levelicon:GetTexture() == "Interface\\TargetingFrame\\UI-TargetingFrame-Skull"
               and raidicon:GetTexture() == "Interface\\TargetingFrame\\UI-RaidTargetingIcons"
        "#
        )
        .unwrap());
    // `libnameplate.lua:39` takes the one child as the health bar: a StatusBar whose only region
    // is its fill.
    assert!(s
        .eval::<bool>(
            r#"
            local p = WorldFrame:GetChildren()
            local healthbar = p:GetChildren()
            if healthbar:GetObjectType() ~= "StatusBar" then return false end
            if healthbar:GetNumRegions() ~= 1 then return false end
            local fill = healthbar:GetStatusBarTexture()
            return fill:GetTexture() == "Interface\\TargetingFrame\\UI-TargetingFrame-BarFill"
        "#
        )
        .unwrap());
}

/// `_Nameplates.lua:163` rejects a plate unless its name is anchored `BOTTOM` to `CENTER` and its
/// level `CENTER` to `BOTTOMRIGHT`, the reference's own anchors (`0x7cb456`, `0x7cb7f5`).
#[test]
fn the_two_fontstring_anchors_are_what_nameplates_validates() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    assert!(s
        .eval::<bool>(
            r#"
            local plate = WorldFrame:GetChildren()
            local Name, Level
            for _, region in ipairs({ plate:GetRegions() }) do
                if region:GetObjectType() == "FontString" then
                    local point, _, relativePoint = region:GetPoint()
                    if point == "BOTTOM" and relativePoint == "CENTER" then Name = region end
                    if point == "CENTER" and relativePoint == "BOTTOMRIGHT" then Level = region end
                end
            end
            return Name ~= nil and Level ~= nil
        "#
        )
        .unwrap());
}

/// `GetValue` (`0x78f5d0`) is raw health, `[bar+0x320]`, not the fill fraction (`0x783380`), and
/// `GetMinMaxValues` is `(0, maxHealth)`; pfUI (`:595`) and CustomNameplates (`:471`) divide them.
#[test]
fn the_healthbar_reports_raw_health_and_its_max() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    assert!(s
        .eval::<bool>(
            r#"
            local healthbar = WorldFrame:GetChildren():GetChildren()
            local min, max = healthbar:GetMinMaxValues()
            return healthbar:GetValue() == 30 and min == 0 and max == 40
        "#
        )
        .unwrap());
}

/// `_Nameplates.lua:164` and pfUI's bubble filter skip any WorldFrame child with a name.
#[test]
fn a_plate_is_anonymous() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    assert!(s
        .eval::<bool>("return WorldFrame:GetChildren():GetName() == nil")
        .unwrap());
}

/// Addons key per-plate state on the plate object: pfUI's `registry[plate]` (`:342`) and
/// `_Nameplates.Frames[Nameplate]` (`:205`).
#[test]
fn a_retired_plate_returns_as_the_same_lua_object() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    s.run("FirstPlate = WorldFrame:GetChildren()").unwrap();
    drive(&mut s, &[]);
    drive(&mut s, &[plate("Bear", 10.0, 90.0)]);
    assert!(s
        .eval::<bool>("return WorldFrame:GetChildren() == FirstPlate")
        .unwrap());
}

/// The pool is keyed by the unit's GUID, not by the driver's distance order, which changes when two
/// units cross; addons cache per-unit state on the plate.
#[test]
fn a_plate_belongs_to_its_unit_across_a_sort_order_swap() {
    let mut s = vm();
    drive(
        &mut s,
        &[plate("Wolf", 30.0, 40.0), plate("Bear", 10.0, 90.0)],
    );
    s.run(
        r#"
        WolfPlate = nil
        for _, p in ipairs({ WorldFrame:GetChildren() }) do
            local _, _, name = p:GetRegions()
            if name:GetText() == "Wolf" then WolfPlate = p end
        end
    "#,
    )
    .unwrap();
    assert!(s.eval::<bool>("return WolfPlate ~= nil").unwrap());

    // The units cross: the driver now sorts Bear first.
    drive(
        &mut s,
        &[plate("Bear", 10.0, 90.0), plate("Wolf", 30.0, 40.0)],
    );
    assert!(s
        .eval::<bool>(
            r#"
            local _, _, name = WolfPlate:GetRegions()
            return name:GetText() == "Wolf"
        "#
        )
        .unwrap());
}

/// pfUI (`:879`) and CustomNameplates (`:287`) find the target plate as the one at alpha 1; with a
/// target up, every other plate is dimmed.
#[test]
fn the_target_plate_is_the_opaque_one() {
    let mut s = vm();
    let mut target = plate("Wolf", 30.0, 40.0);
    let mut other = plate("Bear", 10.0, 90.0);
    target.alpha = 1.0;
    other.alpha = 0.5;
    drive(&mut s, &[target, other]);
    assert!(s
        .eval::<bool>(
            "local a, b = WorldFrame:GetChildren() return a:GetAlpha() == 1 and b:GetAlpha() < 1"
        )
        .unwrap());
}

/// pfUI's bar `OnValueChanged` (`:393`) finds the plate by `this:GetParent()`; the reference sets
/// the bar from a GUID-watch callback that fires the handler, as the driver's write does here.
#[test]
fn an_engine_health_write_fires_onvaluechanged_with_the_plate_as_parent() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    s.run(
        r#"
        Fired = 0
        ParentIsPlate = false
        local plate = WorldFrame:GetChildren()
        local healthbar = plate:GetChildren()
        healthbar:SetScript("OnValueChanged", function()
            Fired = Fired + 1
            ParentIsPlate = (this:GetParent() == plate)
        end)
    "#,
    )
    .unwrap();
    drive(&mut s, &[plate("Wolf", 12.0, 40.0)]);
    assert_eq!(s.eval::<i64>("return Fired").unwrap(), 1);
    assert!(s.eval::<bool>("return ParentIsPlate").unwrap());
    // Unmoved health fires nothing: the driver writes only what changed.
    drive(&mut s, &[plate("Wolf", 12.0, 40.0)]);
    assert_eq!(s.eval::<i64>("return Fired").unwrap(), 1);
}

/// The driver writes a plate's static properties once, at creation, so an addon's takeover (pfUI
/// blanks all six regions, ShaguTweaks reparents the raid icon) survives.
#[test]
fn the_driver_does_not_overwrite_an_addons_takeover() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    s.run(
        r#"
        local plate = WorldFrame:GetChildren()
        for _, region in ipairs({ plate:GetRegions() }) do
            if region.SetTexture then region:SetTexture("") end
        end
    "#,
    )
    .unwrap();
    // Several frames of driving: health moves and the plate re-seats.
    for hp in [29.0, 28.0, 27.0] {
        let mut state = plate("Wolf", hp, 40.0);
        state.top_centre = (500.0 + hp, 400.0);
        drive(&mut s, &[state]);
    }
    assert!(s
        .eval::<bool>(
            r#"
            local border = WorldFrame:GetChildren():GetRegions()
            return border:GetTexture() == "" or border:GetTexture() == nil
        "#
        )
        .unwrap());
}

/// The glow art has no alpha and is mostly black, so the reference blends it ADD (`0x7cb36a`); as
/// BLEND it is an opaque box. Deviation: we paint no rim, which reads as hard edges in our
/// pipeline, but the region stays shown, ADD and textured. The border check fails an empty extract.
#[test]
fn a_hovered_plate_emits_no_glow_quad() {
    use crate::script::QuadContent;
    let mut s = vm();
    let mut p = plate("Wolf", 30.0, 40.0);
    p.hovered = true;
    p.lit = true;
    drive(&mut s, &[p]);

    assert!(s
        .eval::<bool>(
            r#"local _, glow = WorldFrame:GetChildren():GetRegions()
               return glow:IsShown() == 1
                  and glow:GetTexture() == "Interface\\Tooltips\\Nameplate-Glow"
                  and glow:GetBlendMode() == "ADD""#
        )
        .unwrap());

    let paths: Vec<String> = s
        .extract()
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture { path, .. } => path.clone(),
            _ => None,
        })
        .collect();
    assert!(
        !paths.iter().any(|p| p.contains("Nameplate-Glow")),
        "the hovered plate's glow must not be painted (0184); quads were {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.contains("Nameplate-Border")),
        "the plate's border must still paint; quads were {paths:?}"
    );
}

/// Text on a plate skips the UI grid snap: the plate's rect snaps to the finer device grid as it
/// slides, and a text top snapped to the logical grid would jump a pixel against it. The flag is
/// the owning frame's, so an addon's own string on the plate gets it too.
#[test]
fn the_plates_texts_carry_the_world_seat_and_its_textures_do_not() {
    use crate::script::QuadContent;
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);

    let seats: Vec<(String, bool)> = s
        .extract()
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Text {
                text: Some(t),
                world_seat,
                ..
            } => Some((t.clone(), *world_seat)),
            _ => None,
        })
        .collect();
    assert_eq!(
        seats,
        vec![("Wolf".to_string(), true), ("12".to_string(), true)],
        "the plate draws exactly its name and level, both seated off the UI grid"
    );

    // A FontString on an ordinary frame stays on the grid.
    s.run(
        r#"local f = CreateFrame("Frame", "Chrome", UIParent) f:SetAllPoints()
           local t = f:CreateFontString("ChromeText") t:SetAllPoints() t:SetText("Chrome")"#,
    )
    .unwrap();
    s.resolve();
    assert!(s.extract().iter().any(|q| matches!(
        &q.content,
        QuadContent::Text { text: Some(t), world_seat: false, .. } if t == "Chrome"
    )));

    // An addon's own string on the plate, as pfUI draws its text, gets the same seat.
    s.run(
        r#"local plate = WorldFrame:GetChildren()
           local t = plate:CreateFontString() t:SetAllPoints() t:SetText("pfName")"#,
    )
    .unwrap();
    s.resolve();
    assert!(
        s.extract().iter().any(|q| matches!(
            &q.content,
            QuadContent::Text { text: Some(t), world_seat: true, .. } if t == "pfName"
        )),
        "an addon's FontString on the plate seats off the UI grid too"
    );
}

/// Only the stock glow art on an ADD region goes unpainted; an addon's own glow texture paints.
#[test]
fn an_addon_that_retextures_the_glow_gets_its_art_painted() {
    use crate::script::QuadContent;
    let mut s = vm();
    let mut p = plate("Wolf", 30.0, 40.0);
    p.hovered = true;
    drive(&mut s, &[p]);
    s.run(
        r#"local _, glow = WorldFrame:GetChildren():GetRegions()
           glow:SetTexture("Interface\\AddOns\\pfUI\\img\\glow")"#,
    )
    .unwrap();
    s.resolve();

    assert!(s
        .extract()
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture { path, .. } => path.clone(),
            _ => None,
        })
        .any(|p| p.contains("pfUI")));
}

/// pfUI reads `glow:IsShown()` as the mouseover signal (`:601`, `:880`), so the glow shows on hover
/// though it paints nothing.
#[test]
fn the_glow_region_is_the_mouseover_signal() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    assert!(s
        .eval::<bool>(
            "local _, glow = WorldFrame:GetChildren():GetRegions() return glow:IsShown() == nil"
        )
        .unwrap());
    let mut hovered = plate("Wolf", 30.0, 40.0);
    hovered.hovered = true;
    drive(&mut s, &[hovered]);
    assert!(s
        .eval::<bool>(
            "local _, glow = WorldFrame:GetChildren():GetRegions() return glow:IsShown() == 1"
        )
        .unwrap());
}

/// The level and the skull (a world boss, or ten levels up) share one seat; pfUI reads
/// `levelicon:IsShown()` and CustomNameplates `Boss:IsVisible()`.
#[test]
fn the_skull_replaces_the_level_number() {
    let mut s = vm();
    let mut skulled = plate("Wolf", 30.0, 40.0);
    skulled.skull = true;
    drive(&mut s, &[skulled]);
    assert!(s
        .eval::<bool>(
            r#"
            local _, _, _, level, levelicon = WorldFrame:GetChildren():GetRegions()
            return level:IsShown() == nil and levelicon:IsShown() == 1
                   and levelicon:IsVisible() == 1
        "#
        )
        .unwrap());
}

/// A level not yet known, with no skull, leaves the seat empty.
#[test]
fn a_unit_with_no_level_yet_wears_no_skull() {
    let mut s = vm();
    let mut unknown = plate("Wolf", 30.0, 40.0);
    unknown.level = None;
    drive(&mut s, &[unknown]);
    assert!(s
        .eval::<bool>(
            r#"
            local _, _, _, level, levelicon = WorldFrame:GetChildren():GetRegions()
            return level:IsShown() == nil and levelicon:IsShown() == nil
        "#
        )
        .unwrap());
}

#[test]
fn a_plate_created_after_the_first_frame_is_laid_out() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    // A second unit on a later frame, with no free slot and no geometry change.
    drive(
        &mut s,
        &[plate("Wolf", 30.0, 40.0), plate("Bear", 10.0, 90.0)],
    );
    let (w, h): (f32, f32) = s
        .eval("local _, b = WorldFrame:GetChildren() return b:GetWidth(), b:GetHeight()")
        .unwrap();
    assert_eq!((w, h), (128.0, 32.0), "the second plate has the plate size");
    // Its regions are anchored to it, or `GetLeft` would not answer.
    assert!(s
        .eval::<bool>(
            r#"local _, b = WorldFrame:GetChildren()
               local border = b:GetRegions()
               return border:GetLeft() == b:GetLeft() and border:GetRight() == b:GetRight()"#
        )
        .unwrap());
}

/// An addon's frame under `WorldFrame` (`pfUICombatScreen`, `_NameplatesFrame`) shares the list
/// with the plates, whose `GetName` is always nil (`0x7a1390`).
#[test]
fn an_addon_frame_under_the_worldframe_is_not_a_plate() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    s.run(r#"CreateFrame("Frame", "pfUICombatScreen", WorldFrame)"#)
        .unwrap();
    assert_eq!(
        s.eval::<i64>("return WorldFrame:GetNumChildren()").unwrap(),
        2
    );
    assert_eq!(
        s.eval::<i64>(
            r#"
            local found = 0
            for _, frame in ipairs({ WorldFrame:GetChildren() }) do
                local region = frame:GetRegions()
                if frame:GetObjectType() == "Button" and region and region.GetTexture
                   and region:GetTexture() == "Interface\\Tooltips\\Nameplate-Border" then
                    found = found + 1
                end
            end
            return found
        "#
        )
        .unwrap(),
        1
    );
}

/// A plate is a mouse-enabled `Button` (`CSimpleButton`'s constructor writes `[+0xcc] = 0x4`), so
/// hovering it takes the mouse focus from the world; the widget layer then names the plate's unit,
/// as the reference's OnEnter (`0x7cb850`) publishes `[0xb4e2c8]`.
#[test]
fn hovering_a_plate_names_its_unit() {
    let mut s = vm();
    let wolf = plate("Wolf", 30.0, 40.0);
    let key = wolf.key;
    drive(&mut s, &[wolf]);

    assert_eq!(s.hovered_nameplate(), None, "nothing hovered yet");
    // The plate hangs 32 units below its top centre (500, 400), so this is its middle.
    s.mouse_move(500.0, 384.0);
    assert_eq!(s.hovered_nameplate(), Some(key));
    // Off the plate the WorldFrame takes the focus again.
    s.mouse_move(50.0, 50.0);
    assert_eq!(s.hovered_nameplate(), None);
}

/// The reference's click slot (`0x7cb910`) ends in the same `SetSelection` as a click on the body,
/// and fires on the up edge only: `RegisterForClicks(0x500)` at `0x7cb637`.
#[test]
fn a_completed_click_on_a_plate_reaches_the_app() {
    let mut s = vm();
    let wolf = plate("Wolf", 30.0, 40.0);
    let key = wolf.key;
    drive(&mut s, &[wolf]);
    s.mouse_move(500.0, 384.0);

    s.mouse_button(500.0, 384.0, "LeftButton", true);
    assert!(
        s.take_nameplate_clicks().is_empty(),
        "a press is not a click: the plate fires on the UP edge only"
    );
    s.mouse_button(500.0, 384.0, "LeftButton", false);
    let clicks = s.take_nameplate_clicks();
    assert_eq!(clicks.len(), 1);
    assert_eq!(clicks[0].key, key);
    assert_eq!(clicks[0].button, "LeftButton");
    assert!(
        s.take_nameplate_clicks().is_empty(),
        "drained, not repeated"
    );
}

/// `0x500` includes RightButtonUp (`0x7cb637` writes `[this+0x330]`), where a plain `Button`
/// registers LeftButtonUp alone; the slot forks on the mask: 1 selects (`0x4925d0`), 4 selects and
/// interacts (`0x492820`).
#[test]
fn a_physical_right_click_on_a_plate_reaches_the_app() {
    let mut s = vm();
    let wolf = plate("Wolf", 30.0, 40.0);
    let key = wolf.key;
    drive(&mut s, &[wolf]);
    s.mouse_move(500.0, 384.0);

    s.mouse_button(500.0, 384.0, "RightButton", true);
    assert!(
        s.take_nameplate_clicks().is_empty(),
        "the UP lane only: `0x500` has no ButtonDown bit"
    );
    s.mouse_button(500.0, 384.0, "RightButton", false);
    let clicks = s.take_nameplate_clicks();
    assert_eq!(clicks.len(), 1);
    assert_eq!(clicks[0].key, key);
    assert_eq!(clicks[0].button, "RightButton");
}

#[test]
fn a_middle_click_on_a_plate_fires_nothing() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    s.mouse_move(500.0, 384.0);
    s.mouse_button(500.0, 384.0, "MiddleButton", true);
    s.mouse_button(500.0, 384.0, "MiddleButton", false);
    assert!(
        s.take_nameplate_clicks().is_empty(),
        "`0x500` is Left|Right on the up edge and nothing else"
    );
}

/// pfUI's click-through (`nameplates.lua:1274`) calls `plate:Click`; in the reference a scripted
/// and a physical click go through the button's one click slot.
#[test]
fn a_scripted_click_selects_too() {
    let mut s = vm();
    let wolf = plate("Wolf", 30.0, 40.0);
    let key = wolf.key;
    drive(&mut s, &[wolf]);
    s.run(r#"WorldFrame:GetChildren():Click("RightButton")"#)
        .unwrap();
    let clicks = s.take_nameplate_clicks();
    assert_eq!(clicks.len(), 1);
    assert_eq!(clicks[0].key, key);
    assert_eq!(clicks[0].button, "RightButton");
}

/// Entering mouselook turns the mouse off on every plate and leaving turns it back on (`0x60f830`,
/// from `0x483e80` and `0x483e70`), for a turn begun on the world that drags across a plate; a
/// press on a plate never reaches mouselook (`0x7662c0`).
#[test]
fn freelook_hands_the_mouse_back() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    s.mouse_move(500.0, 384.0);
    assert!(s.hovered_nameplate().is_some());

    s.set_nameplate_mouse(false);
    s.mouse_move(500.0, 384.1);
    assert_eq!(
        s.hovered_nameplate(),
        None,
        "a plate must not hold the pointer while the camera does"
    );

    s.set_nameplate_mouse(true);
    s.mouse_move(500.0, 384.0);
    assert!(
        s.hovered_nameplate().is_some(),
        "and it takes it back on leave"
    );
}

/// The plate's `+0x3c` hit test (`0x7cba30`) refuses the point while a ground-targeted spell is
/// armed (`0x6e48a0() && 0x6e6320() && !0x6e6180()`), so it falls through to the `WorldFrame`.
/// It never touches `[+0xcc]`: `IsMouseEnabled` stays true, unlike under mouselook (`0x60f830`).
#[test]
fn a_ground_target_veto_refuses_the_hit_without_disabling_the_mouse() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    s.mouse_move(500.0, 384.0);
    assert!(s.hovered_nameplate().is_some(), "the plate takes the mouse");

    s.set_nameplate_hit_test_veto(true);
    s.mouse_move(500.0, 384.1);
    assert_eq!(
        s.hovered_nameplate(),
        None,
        "the reticle must be placeable through a plate"
    );
    // The press falls through too, so the WorldFrame takes the ground cast.
    s.mouse_button(500.0, 384.1, "LeftButton", true);
    s.mouse_button(500.0, 384.1, "LeftButton", false);
    assert!(
        s.take_nameplate_clicks().is_empty(),
        "a vetoed plate takes no click either"
    );
    assert_eq!(
        s.eval::<i64>("local p = WorldFrame:GetChildren() return p:IsMouseEnabled()")
            .unwrap(),
        1,
        "the veto refuses the hit test, it does not disable the mouse"
    );

    s.set_nameplate_hit_test_veto(false);
    s.mouse_move(500.0, 384.0);
    assert!(
        s.hovered_nameplate().is_some(),
        "and the plate takes it back when the cast is gone"
    );
}
