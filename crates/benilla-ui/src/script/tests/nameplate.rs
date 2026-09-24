//! The nameplate ABI, as the four vanilla addons actually read it (decision 2148).
//!
//! These are not "does the pool work" tests. Each is a transcription of a real corpus idiom — the
//! same walk, the same destructuring, the same identification test — so a change that would break
//! a shipped addon fails **here** rather than on the director's screen.
//!
//! Sources, all `## Interface: 11200`: `pfUI/modules/nameplates.lua` (+ `compat/vanilla.lua`),
//! `ShaguTweaks/libs/libnameplate.lua`, `CustomNameplates/CustomNameplates.lua`,
//! `_Nameplates/_Nameplates.lua`. The byte side is read off the reference binary.

use crate::script::{PlateGeometry, PlateState, UiScript};

/// A VM with a `WorldFrame` to hang plates off — the only thing [`UiScript::sync_nameplates`]
/// requires of a session.
fn vm() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(r#"WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()"#)
        .unwrap();
    s.resolve();
    s
}

/// The plate size at the capture window (`vplates` scenario: one gx unit = 1280 px, so the
/// 0.1 × 0.025 plate is 128 × 32 px, and at seam scale 1.0 that is 128 × 32 units).
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
        // The unit's GUID stands in as a hash of its name: what matters to these tests is that the
        // same unit keeps the same key across frames and two units never share one.
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

/// pfUI + ShaguTweaks share one lineage: `GetNumChildren` grew, so walk the tail.
///
/// `initialized` is never reset in either addon, so a client whose child list SHRANK or REORDERED
/// would make them miss every plate created after the old high-water mark — permanently, and with
/// no error. This is the test that a retired plate keeps its slot.
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

    // One unit leaves: the plate is HIDDEN, and the child list does not shrink.
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

    // A third unit arrives: it takes the pooled plate back — the count goes to 2, not 3.
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

/// pfUI's `IsNamePlate` (`nameplates.lua:70`) and ShaguTweaks' (`libnameplate.lua:12`), verbatim in
/// shape: the object type is `"Button"`, and region #1 is a `Texture` whose `GetTexture()` is
/// exactly `Interface\Tooltips\Nameplate-Border`.
///
/// `CustomNameplates` runs the texture half against **every** WorldFrame child
/// (`CustomNameplates.lua:93`), so `GetRegions()` must also be safe to call on a frame with none.
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

/// The two tuples, positionally — the ABI decision 2148 §3 records.
///
/// `CustomNameplates.lua:212` binds `Border, Glow, Name, Level, Boss, RaidTargetIcon`; pfUI and
/// ShaguTweaks bind the same six by the `NAMEPLATE_OBJECTORDER` table. A **seventh** region is a
/// bug (pfUI blanks any extra with `SetTexture("")`), and a **second** child is a bug too (pfUI
/// destructures `healthbar, castbar` and would blank ours as a TBC cast bar).
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
    // ShaguTweaks `libnameplate.lua:39`: `plate.healthbar = plate:GetChildren()` — one child, and
    // it is the StatusBar. Its own single region is the fill `GetStatusBarTexture()` hands back.
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

/// `_Nameplates.Nameplate.Initialize` (`_Nameplates.lua:163`) is the strictest consumer: it
/// content-addresses the regions and **validates the two FontString anchors**, rejecting any plate
/// whose name is not `BOTTOM`→`CENTER` and whose level is not `CENTER`→`BOTTOMRIGHT`. Those are the
/// reference's own (ctor `0x7cb456`, bind `0x7cb7f5`).
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

/// The bar reports **raw** health, not a fraction — `GetValue 0x78f5d0` → `[bar+0x320]`, and
/// `GetMinMaxValues` → `(0, maxHealth)`. pfUI (`:595`) and CustomNameplates (`:471`) both divide,
/// so a fraction here would read as full health on every plate.
///
/// The internal fill *fraction* `0x783380` is a distinct value, easily confused for the Lua getter
/// above.
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

/// The plate is anonymous. `_Nameplates.lua:164` rejects any WorldFrame child with a name, and
/// pfUI's bubble filter does the same — a named plate is invisible to both.
#[test]
fn a_plate_is_anonymous() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    assert!(s
        .eval::<bool>("return WorldFrame:GetChildren():GetName() == nil")
        .unwrap());
}

/// The pooling contract: a retired plate comes back as the **same Lua object**. Every addon keys
/// its per-plate state off exactly that identity — pfUI's `registry[plate]` (`:342`),
/// `_Nameplates.Frames[Nameplate]` (`:205`) — and re-runs `OnCreate` for an object it has not seen.
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

/// **A plate belongs to its UNIT, not to this frame's sort order.**
///
/// The driver hands plates over in distance-sorted order — the seat priority the reference solves
/// overlap in — and that order changes whenever two units cross. If the pool were indexed by it,
/// widget #1 would swap units mid-session, and every addon that caches state on the frame would be
/// reading the other unit's: pfUI's `nameplate.original.name`/`level`, its `registry[plate]`
/// bookkeeping, `_Nameplates`' per-plate button. So the pool is keyed by the unit's GUID, and this
/// is the test that says so.
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

    // The two units cross: the driver now sorts Bear first. Nothing about either plate's identity
    // may move with it.
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

/// `plate:GetAlpha() == 1` is how pfUI (`:879`) and CustomNameplates (`:287`) detect the TARGET
/// plate: with a target up, every other plate is dimmed. So alpha is a contract, not a look.
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

/// pfUI hooks the bar's `OnValueChanged` (`:393`) and reads `this:GetParent()` inside the handler
/// to find the plate. The driver moves that value from Rust, so the handler has to fire from there
/// too — the reference's own bar is driven by a GUID-watch callback and fires it the same way.
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

/// **An addon takes the plate over.** pfUI blanks all six regions (`DisableObject` →
/// `SetTexture("")`) and paints its own art; ShaguTweaks reparents the raid icon. A driver that
/// re-stamped every property every frame would undo that at 60 Hz — so static properties are
/// written once, at creation, and never again.
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
    // Several frames of ordinary driving — health moving, the unit renamed, the plate re-seated.
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

/// **A hovered plate wears no black box** — the defect the widget port shipped (director report,
/// 2026-09-10: "they look totally fucked when I hover or click them"), and the two halves that
/// keep it fixed.
///
/// `Nameplate-Glow.blp` is DXT1 with **no alpha channel** and 80.5% pure-black texels. The
/// reference blends it `ADD` (`push 3` @`0x7cb36a`); blitted with straight `BLEND` it is an opaque
/// rectangle over the entire plate — which is exactly what the ctor's default mode produced here.
/// 0184 is the director's call that benilla paints **no rim at all**, so the region has to be ADD
/// *and* emit nothing, while staying shown and textured for the addons that read it.
///
/// The border assert is not decoration: without it the test would pass on an extract that produced
/// no quads whatsoever.
#[test]
fn a_hovered_plate_emits_no_glow_quad() {
    use crate::script::QuadContent;
    let mut s = vm();
    let mut p = plate("Wolf", 30.0, 40.0);
    p.hovered = true;
    p.lit = true;
    drive(&mut s, &[p]);

    // The model tells the truth: shown, textured, and the reference's own blend mode.
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

/// **The plate's name and level seat off the UI pixel grid** (decision 2172).
///
/// The plate is a WorldFrame overlay: the driver snaps its rect to the DEVICE pixel grid
/// (`vplates::device_snap` — half a logical pixel at 2×) because it slides continuously over the
/// world, and everything drawn inside it has to be rigid to that. The renderer's UI seat snap
/// quantizes a text block's top on the coarser LOGICAL grid, so with both laws in force the name
/// and the level pop a whole pixel every second step the border takes.
///
/// The painter this port replaced never snapped them — it measured into degenerate rects, which
/// the renderer's carve-out skips — so this flag is what carries that seating law across decision
/// 2148's move into the frame system. It rides the extract, one field per Text quad, because the
/// renderer is the only thing that can act on it.
///
/// It is answered from the OWNER, which is why the third block here matters: pfUI and
/// ShaguTweaks blank the stock regions and hang their own FontStrings on the plate, and a rule
/// written on our six regions would have left an addon's strings jittering inside a plate whose
/// own text had stopped.
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

    // A FontString on an ordinary frame is interface, and interface is ON the grid — the flag has
    // to be the plate's claim, not something every string picked up.
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

    // An addon's own string on the plate — pfUI's whole vanilla branch — is drawn inside the same
    // sliding rect, so it gets the same seat.
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

/// The suppression is **the reference's own glow art on an ADD region**, and nothing wider: an
/// addon that re-textures the region gets its art painted like any other texture.
///
/// pfUI's vanilla branch does exactly this — it blanks the six regions and paints its own — so a
/// blanket "plate glow regions never draw" would silently eat an addon's work.
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

/// pfUI reads `glow:IsShown()` as the MOUSEOVER signal (`:601`, `:880`). benilla does not paint the
/// additive rim (0184 — it read as hard edge lines on our pipeline, and the director pinned a bar
/// brighten instead), but the region is structurally present and really shown, because the
/// deviation is about pixels and this is about data.
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

/// The level number and the skull share one seat and are mutually exclusive — `skull` is how the
/// driver says "world boss, or ten levels up". pfUI gates on `levelicon:IsShown()` and
/// CustomNameplates on `Boss:IsVisible()`, so both flags have to move.
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

/// **A unit whose level has not arrived is not a world boss.** `level: None` with no skull leaves
/// the seat EMPTY — the old painter's behaviour, which the widget port inverted by making one
/// `Option` carry both facts (audit, 2026-09-10).
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

/// **A plate the pool grows on any frame but the first is laid out too.** The lay-out used to ride
/// a per-frame "did the window move?" flag, so a unit that walked into range later got a plate with
/// no size and no anchors: invisible, permanently, and the number of plates on screen was capped at
/// however many were up when V was first pressed (audit, 2026-09-10).
#[test]
fn a_plate_created_after_the_first_frame_is_laid_out() {
    let mut s = vm();
    drive(&mut s, &[plate("Wolf", 30.0, 40.0)]);
    // A second unit arrives on a LATER frame, with no free slot and no geometry change.
    drive(
        &mut s,
        &[plate("Wolf", 30.0, 40.0), plate("Bear", 10.0, 90.0)],
    );
    let (w, h): (f32, f32) = s
        .eval("local _, b = WorldFrame:GetChildren() return b:GetWidth(), b:GetHeight()")
        .unwrap();
    assert_eq!((w, h), (128.0, 32.0), "the second plate has the plate size");
    // …and its regions are anchored to it, which is what `GetLeft` answering at all proves.
    assert!(s
        .eval::<bool>(
            r#"local _, b = WorldFrame:GetChildren()
               local border = b:GetRegions()
               return border:GetLeft() == b:GetLeft() and border:GetRight() == b:GetRight()"#
        )
        .unwrap());
}

/// An addon's own frame parented to `WorldFrame` shares the list with the plates —
/// `_NameplatesFrame` and `pfUICombatScreen` both are — which is why the identification test
/// (`0x7a1390`'s always-nil `GetName`) is load-bearing and why the count-grew heuristic re-scans.
/// It must not be mistaken for one.
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

/// **The plate takes the mouse, and hands its unit back.** It is a `Button` and born mouse-enabled
/// (`CSimpleButton`'s ctor writes `[+0xcc] = 0x4`), so hovering it makes it the mouse focus — which
/// on this engine means the UI pointer pass owns the cursor and the world pick stands down. What
/// keeps the mouseover alive is the seam: the widget layer answers WHICH unit's plate the pointer
/// is inside, exactly as the reference's OnEnter (`0x7cb850`) publishes `[0xb4e2c8]`.
#[test]
fn hovering_a_plate_names_its_unit() {
    let mut s = vm();
    let wolf = plate("Wolf", 30.0, 40.0);
    let key = wolf.key;
    drive(&mut s, &[wolf]);

    assert_eq!(s.hovered_nameplate(), None, "nothing hovered yet");
    // The plate's TOP-CENTRE is (500, 400) and it hangs 32 units below, so this is its middle.
    s.mouse_move(500.0, 384.0);
    assert_eq!(s.hovered_nameplate(), Some(key));
    // Off the plate: the WorldFrame takes the focus again, and no plate is named.
    s.mouse_move(50.0, 50.0);
    assert_eq!(s.hovered_nameplate(), None);
}

/// A completed click on a plate reaches the app — the reference's click slot (`0x7cb910`), whose
/// tail is the same `SetSelection` a click on the body runs.
///
/// **Mouse-UP only**: the plate registers `LeftButtonUp | RightButtonUp` (`RegisterForClicks(0x500)`
/// at `0x7cb637`), so a press alone selects nothing.
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

/// **The right button too** — `RegisterForClicks(0x500)` is LeftButtonUp | **RightButtonUp**
/// (`0x7cb637` → `[this+0x330]`), and the reference's slot forks on it: mask 1 → `0x4925d0`
/// select, mask 4 → `0x492820` select **and interact** (decision 2233).
///
/// The regression this pins: a plate is created as a plain `Button`, whose default registered set
/// is `{"LeftButtonUp"}` alone, so the release was refused before the click funnel — and with 2233
/// taking the camera off a press that lands on a plate, that left right-clicking a nameplate doing
/// nothing whatsoever.
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

/// A button the plate never registered fires nothing — the mask is exactly `0x500`, not "any
/// button".
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

/// pfUI's click-through calls `plate:Click("LeftButton")` (`nameplates.lua:1274`), and
/// CustomNameplates and `_Nameplates` do the same. A scripted click has to select the unit like a
/// physical one — in the reference both go through the button's one click slot, and here they go
/// through the one funnel that slot's twin lives on.
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

/// **The mouselook toggle** (`0x60f830`): entering freelook hands the mouse back on every plate,
/// leaving takes it again — the reference walks its own intrusive plate list doing exactly this,
/// from `0x483e80` (enter) and `0x483e70` (leave).
///
/// **What it is for is NOT "a drag that begins over a plate"** — that gesture never reaches
/// mouselook at all, because `0x7662c0` hands the mouse-down to the plate and stops the bus walk
/// before any binding runs (decision 2233, which reversed the inference this doc used to carry).
/// It is for a turn that started on the **world** and then dragged the pointer across a plate: the
/// plates must not take a pointer that is hidden and locked to the camera.
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

/// **The plate's own `+0x3c` veto** (`0x7cba30`) — a DIFFERENT mechanism from the freelook toggle
/// above, and the reason it is not folded into the same flag.
///
/// While a ground-targeted spell is armed the plate refuses the hit test *before* testing its rect
/// (`0x6e48a0() && 0x6e6320() && !0x6e6180()` → `xor eax,eax; ret 4`, never calling the base), so
/// the point falls through to the `WorldFrame` at (strata 0, level 0) behind it and the reticle can
/// be placed through a plate. Crucially it never touches `[+0xcc]`: `IsMouseEnabled()` still
/// answers **true** throughout, which is exactly what `0x60f830` does not do.
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
    // …and the press falls through with it, which is what lets the WorldFrame win the gesture and
    // the ground cast commit where the player clicked.
    s.mouse_button(500.0, 384.1, "LeftButton", true);
    s.mouse_button(500.0, 384.1, "LeftButton", false);
    assert!(
        s.take_nameplate_clicks().is_empty(),
        "a vetoed plate takes no click either"
    );
    // The bit the veto must NOT have touched — an addon asking is told the truth.
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
