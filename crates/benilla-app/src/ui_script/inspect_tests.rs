//! The shipped **inspect window** driven end-to-end, engine-only (no Bevy): the real
//! The reference's own `Blizzard_InspectUI` addon loaded behind
//! `Fonts.xml`/`UIParent.xml`/`GameTooltip.xml` and fed a synthetic target snapshot + a foreign
//! equipment view. `character_tests.rs`'s harness, turned onto the other paper doll.
//!
//! What these are here to falsify, in order of how quietly it could have shipped broken:
//!
//! 1. **The range gate really gates.** `InspectUnit` out of range must not open the window. A
//!    `CanInspect` that always answered truthy would look perfectly fine in play — you would just
//!    be able to inspect across the zone — so the boundary is asserted at the verified thresholds,
//!    including the strict-vs-non-strict asymmetry between the two predicates.
//! 2. **The slots come from the inspected unit, not from us.** The whole slice is one router change
//!    (`Model::inv_slot`); if it fell back to the self feed, the window would show *your* gear on
//!    someone else's doll — plausible-looking and completely wrong. So the two sources are fed
//!    deliberately different items and the doll is checked against the foreign one.
//! 3. **The window's own lifecycle.** Open/close sounds, the level line off the target snapshot, the
//!    `UNIT_INVENTORY_CHANGED` repaint, the rotate buttons moving the booth yaw, and
//!    `ClearInspectPlayer` firing on hide (without which the app would keep resolving a closed
//!    window's target forever).

use std::collections::HashMap;

use benilla_ui::script::{
    InspectView, InvSlotView, InventorySlots, QuadContent, SoundRequest, UiScript, UnitReach,
    UnitState,
};

/// A reach entry for a live, inspectable unit at squared distance `d2` — the ordinary case. The
/// `inspectable` half is `CanInspect`'s alone (the app folds vmangos's non-distance refusals into
/// it); every test in this file is about distance, so it stays true.
fn reach(dist_sq: f64) -> UnitReach {
    UnitReach {
        dist_sq,
        inspectable: true,
    }
}

use super::test_ui::load_ui as load_xml;

/// The inspected player: a Dwarf Paladin, level 34.
fn target_unit() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Thargrim".into()),
        health: 900,
        max_health: 900,
        level: 34,
        race: Some("Dwarf".into()),
        race_file: Some("Dwarf".into()),
        class: Some("Paladin".into()),
        class_file: Some("PALADIN".into()),
        sex: 2,
        is_player: true,
        player_controlled: true,
        ..Default::default()
    }
}

/// A slot view as the inspect feed builds one: entry + icon + name + quality, and nothing an item
/// *object* would have supplied (no count>1, no durability, no lock) — see `ui_inspect`'s module doc.
fn foreign_slot(entry: u32, icon: &str, name: &str) -> InvSlotView {
    InvSlotView {
        item_id: entry,
        icon: Some(icon.into()),
        count: 1,
        quality: 3,
        name: Some(name.into()),
        link: Some(format!("|cff0070dd|Hitem:{entry}:0:0:0|h[{name}]|h|r")),
        ..Default::default()
    }
}

/// The inspected unit's equipment: a helm in the head slot (live-API id 1) and nothing else.
fn inspect_view(unit: &str) -> InspectView {
    let mut slots: InventorySlots = Default::default();
    slots[1] = Some(foreign_slot(
        7365,
        "Interface\\Icons\\INV_Helmet_09",
        "Mighty Helm",
    ));
    InspectView {
        unit: unit.into(),
        guid: 0x0000_0000_0000_0042,
        slots,
    }
}

/// OUR equipment, deliberately a different item in the same slot — the router's falsifier.
fn own_slots() -> InventorySlots {
    let mut slots: InventorySlots = Default::default();
    slots[1] = Some(foreign_slot(
        1234,
        "Interface\\Icons\\INV_Helmet_01",
        "My Own Helm",
    ));
    slots
}

/// Does a texture whose path contains `needle` actually get DRAWN this frame? Regions carry no Lua
/// `GetTexture` in this engine, so the icon assertions go through the render pass exactly as
/// `character_tests.rs`'s do — which also makes them a stronger claim (it renders, not merely
/// "the state says so").
fn drawn(s: &mut UiScript, needle: &str) -> bool {
    s.resolve();
    s.extract().iter().any(
        |q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle)),
    )
}

/// Everything the window needs to open on `"target"`, in range.
fn armed() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // `PLAYER_LEVEL`, the template the stock level line formats through — the reference keeps its
    // strings here, and our retired file had the sentence written into it (1832).
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // `TEXT`, which the stock `InspectPaperDollFrame_SetLevel` formats its level line through.
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    // The inspect window's four tabs (`Blizzard_InspectUI`) inherit it (1993).
    load_xml(&s, r"Interface\FrameXML\CharacterFrameTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    // `InspectUnit` lives here now — the reference's own home for that name, and no longer in the
    // window's own file. The manifest's order, and 49 other harnesses'.
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    // Before InspectFrame.xml, and required rather than tidy: this window's honor page inherits
    // HonorFrame.xml's five row templates and `inherits=` resolves at LOAD, so without it the
    // twelve honor rows materialize bare (the manifest states the same order).
    // The stock slot buttons inherit `ItemButtonTemplate` — where their `$parentIconTexture`
    // comes from, which `InspectPaperDollItemSlotButton_OnLoad` indexes unconditionally. Our own
    // slot template declared its icon inline and needed no such file; the manifest has carried
    // this entry far above the inspect window all along.
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\HonorFrame.xml");
    // The window is a LoadOnDemand addon, reached the way the app reaches it: seated off the
    // chain as a registry row (1957) and loaded by the reference's own `InspectFrame_LoadUI`
    // (UIParent.xml; 1967).
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_InspectUI");
    s.run("InspectFrame_LoadUI()").unwrap();
    s.set_unit("target", Some(target_unit()));
    s.set_inspect(Some(inspect_view("target")));
    // 4 yards away (d² = 16) — comfortably inside the verified 100.0.
    s.set_unit_reach(HashMap::from([("target".to_string(), reach(16.0))]));
    s
}

/// The loader itself: the window and its 19 slots + model pane materialize with no errors.
#[test]
fn shipped_inspect_frame_loads_clean() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    // `PLAYER_LEVEL`, the template the stock level line formats through — the reference keeps its
    // strings here, and our retired file had the sentence written into it (1832).
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // `TEXT`, which the stock `InspectPaperDollFrame_SetLevel` formats its level line through.
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    // The inspect window's four tabs (`Blizzard_InspectUI`) inherit it (1993).
    load_xml(&s, r"Interface\FrameXML\CharacterFrameTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    // `InspectUnit` lives here now — the reference's own home for that name, and no longer in the
    // window's own file. The manifest's order, and 49 other harnesses'.
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    // Before InspectFrame.xml, and required rather than tidy: this window's honor page inherits
    // HonorFrame.xml's five row templates and `inherits=` resolves at LOAD, so without it the
    // twelve honor rows materialize bare (the manifest states the same order).
    // The stock slot buttons inherit `ItemButtonTemplate` — where their `$parentIconTexture`
    // comes from, which `InspectPaperDollItemSlotButton_OnLoad` indexes unconditionally. Our own
    // slot template declared its icon inline and needed no such file; the manifest has carried
    // this entry far above the inspect window all along.
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\HonorFrame.xml");
    // The window is a LoadOnDemand addon, reached the way the app reaches it: seated off the
    // chain as a registry row (1957) and loaded by the reference's own `InspectFrame_LoadUI`
    // (UIParent.xml; 1967).
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_InspectUI");
    s.run("InspectFrame_LoadUI()").unwrap();
    // All 19 slots exist and carry their GetInventorySlotInfo id (1..=19, no ammo slot).
    // The stock `InspectPaperDollItemSlotButton_OnLoad` puts it on the frame's own ID
    // (`this:SetID(id)`); our retired file kept it in an `invSlotId` field of its own (1832).
    for (name, id) in [
        ("InspectHeadSlot", 1),
        ("InspectBackSlot", 15),
        ("InspectMainHandSlot", 16),
        ("InspectRangedSlot", 18),
        ("InspectTabardSlot", 19),
    ] {
        assert_eq!(
            s.eval::<i64>(&format!("return {name}:GetID()")).unwrap(),
            id,
            "{name} inventory slot id"
        );
    }
}

/// **The gate really gates.** Out of range, `InspectUnit` sends nothing and opens nothing; the same
/// call in range does both. A `CanInspect` stuck truthy passes every other test in this file.
#[test]
fn inspect_unit_refuses_out_of_range() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = armed();
    // 11 yards (d² = 121) — past the verified 100.0 threshold.
    s.set_unit_reach(HashMap::from([("target".to_string(), reach(121.0))]));

    s.run(r#"InspectUnit("target")"#).unwrap();
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return InspectFrame:IsVisible()").unwrap(),
        "out of range: the window must not open"
    );
    assert!(
        s.take_inspect_notifies().is_empty(),
        "out of range: no CMSG_INSPECT request is queued"
    );

    // Step inside the threshold: the same call now opens and requests.
    s.set_unit_reach(HashMap::from([("target".to_string(), reach(99.9))]));
    s.run(r#"InspectUnit("target")"#).unwrap();
    assert!(
        s.eval::<bool>("return InspectFrame:IsVisible()").unwrap(),
        "in range: the window opens"
    );
    assert_eq!(
        s.take_inspect_notifies(),
        vec!["target".to_string()],
        "in range: NotifyInspect queued the token for the app to resolve"
    );
}

/// The two verified thresholds and their **different operators** — `CanInspect` refuses only on a
/// strict `100.0 < d²` (so exactly 100.0 is IN range), while `CheckInteractDistance` admits only on
/// a strict `d² < table[type-1]` (so exactly 100.0 is OUT). That asymmetry is the binary's own
/// (`test ah,0x41; jne` at `0x48a28b` vs `test ah,0x5; jp` at `0x48bb0a`), and it is the kind of
/// detail a rewrite silently normalizes — hence a test on the boundary itself.
#[test]
fn range_predicates_transcribe_the_verified_thresholds() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = armed();
    for (d2, can_inspect, interact1) in [
        (99.9_f64, true, true),
        (100.0, true, false),
        (100.1, false, false),
    ] {
        s.set_unit_reach(HashMap::from([("target".to_string(), reach(d2))]));
        assert_eq!(
            s.eval::<bool>(r#"return CanInspect("target") ~= nil"#)
                .unwrap(),
            can_inspect,
            "CanInspect at d²={d2}"
        );
        assert_eq!(
            s.eval::<bool>(r#"return CheckInteractDistance("target", 1) ~= nil"#)
                .unwrap(),
            interact1,
            "CheckInteractDistance(type 1) at d²={d2}"
        );
    }
    // Type 4 is the 30-yard row (900.0) — the table is indexed, not hardcoded to one distance.
    s.set_unit_reach(HashMap::from([("target".to_string(), reach(899.0))]));
    assert!(s
        .eval::<bool>(r#"return CheckInteractDistance("target", 4) ~= nil"#)
        .unwrap());
    assert!(
        !s.eval::<bool>(r#"return CheckInteractDistance("target", 1) ~= nil"#)
            .unwrap(),
        "the same distance is out of range for the 10-yard type"
    );
    // **A token the object manager holds no unit for answers nil — the null-object arm** (report
    // B316, `0x48babe`). This is the party member outside the local
    // area: the roster wire gives them a GUID, so the token resolves, and the object lookup then
    // misses silently. It used to answer 1, which lit every distance row for exactly the member
    // who was furthest away.
    assert!(
        s.eval::<bool>(r#"return CheckInteractDistance("party3", 1) == nil"#)
            .unwrap(),
        "a token with no live unit is out of range, not in"
    );
    assert!(
        s.eval::<bool>(r#"return CanInspect("party3") == nil"#)
            .unwrap(),
        "…and CanInspect agrees, through its own null-`this` tail"
    );

    // The `type` argument's three degenerate arms, each the binary's own answer.
    s.set_unit_reach(HashMap::from([("target".to_string(), reach(1.0))]));
    for bad in ["0", "5", "-1", "0.5"] {
        assert!(
            s.eval::<bool>(&format!(
                r#"return CheckInteractDistance("target", {bad}) == nil"#
            ))
            .unwrap(),
            "type {bad} is outside the table (unsigned compare on trunc(type) − 1)"
        );
    }
    // …but a fractional type INSIDE the range truncates toward zero and answers its row.
    assert!(
        s.eval::<bool>(r#"return CheckInteractDistance("target", 1.9) ~= nil"#)
            .unwrap(),
        "1.9 chops to 1"
    );
    // A missing `type` is a usage ERROR, not nil — the reference's `luaL_error`, which longjmps.
    assert!(
        s.eval::<bool>(r#"return CheckInteractDistance("target") ~= nil"#)
            .is_err(),
        "no distIndex is a script error"
    );

    // The token is CASE-FOLDED, like every compare in the resolver both predicates reach their
    // unit through (`_strnicmp`, 1247) — the same fold `UnitName("Target")` already gets. An
    // addon that capitalises must not be told the unit is out of range.
    assert!(
        s.eval::<bool>(r#"return CheckInteractDistance("TARGET", 1) ~= nil"#)
            .unwrap(),
        "an upper-case token names the same unit"
    );
    assert!(
        s.eval::<bool>(r#"return CanInspect("Target") ~= nil"#)
            .unwrap(),
        "…for both predicates"
    );
}

/// **The doll shows THEIR gear, not ours.** Both sources are fed, with different items in the same
/// slot; the inspected token must read the foreign one and `"player"` must still read ours.
#[test]
fn inventory_bindings_route_by_unit_token() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = armed();
    s.set_inventory_slots(own_slots());

    assert_eq!(
        s.eval::<String>(r#"return GetInventoryItemTexture("target", 1)"#)
            .unwrap(),
        "Interface\\Icons\\INV_Helmet_09",
        "the inspected token reads the foreign visible-item view"
    );
    assert_eq!(
        s.eval::<String>(r#"return GetInventoryItemTexture("player", 1)"#)
            .unwrap(),
        "Interface\\Icons\\INV_Helmet_01",
        "\"player\" still reads our own equipped feed"
    );
    // A token nobody is inspecting has no equipment source at all.
    assert!(s
        .eval::<bool>(r#"return GetInventoryItemTexture("party2", 1) == nil"#)
        .unwrap());
    // The inspect view is keyed on the token it was built for: if the window rebinds to another
    // token, the stale view must not answer for it.
    s.set_inspect(Some(inspect_view("party1")));
    assert!(
        s.eval::<bool>(r#"return GetInventoryItemTexture("target", 1) == nil"#)
            .unwrap(),
        "a view built for \"party1\" must not answer for \"target\""
    );
}

/// The whole window contract in one drive: open through `ShowUIPanel` with the ref's sound, the
/// name/level lines off the target snapshot, the slot icon painted from the foreign view, an empty
/// slot falling back to its paper-doll art, the `UNIT_INVENTORY_CHANGED` repaint, the rotate buttons
/// moving the booth yaw, and closing firing `ClearInspectPlayer`.
#[test]
fn shipped_inspect_frame_drives_end_to_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = armed();

    // Hidden at load, and no sound queued (never transitions on startup).
    assert!(!s.eval::<bool>("return InspectFrame:IsVisible()").unwrap());
    assert!(s.take_sounds().is_empty());
    // The pane's OnLoad set the ref's own default facing (InspectModelFrame_OnLoad, 0.61).
    assert!(
        (s.model_pane_facing("InspectModelFrame") - 0.61).abs() < 0.0001,
        "default facing 0.61, got {}",
        s.model_pane_facing("InspectModelFrame")
    );

    s.run(r#"InspectUnit("target")"#).unwrap();
    assert!(s.errors().is_empty(), "open errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return InspectFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoOpen".into())],
        "opening plays igCharacterInfoOpen"
    );

    assert_eq!(
        s.eval::<String>("return InspectNameText:GetText()")
            .unwrap(),
        "Thargrim",
        "the name line reads UnitName of the inspected token"
    );
    assert_eq!(
        s.eval::<String>("return InspectLevelText:GetText()")
            .unwrap(),
        "Level 34 Dwarf Paladin",
        "the level line reads the inspected unit, not the player"
    );
    assert!(
        drawn(&mut s, "INV_Helmet_09"),
        "the head slot paints the inspected item's icon"
    );
    assert!(
        drawn(&mut s, "UI-PaperDoll-Slot-Chest"),
        "an empty slot falls back to its own paper-doll art"
    );

    // The repaint path: the feed pushes a changed view and fires the event the ref's slot buttons
    // listen for (filtered on arg1 == the inspected unit).
    let mut swapped = inspect_view("target");
    swapped.slots[1] = Some(foreign_slot(
        7366,
        "Interface\\Icons\\INV_Helmet_22",
        "Better Helm",
    ));
    s.set_inspect(Some(swapped));
    s.fire_event(
        "UNIT_INVENTORY_CHANGED",
        vec![benilla_ui::script::ScriptValue::Str("target".into())],
    );
    assert!(
        drawn(&mut s, "INV_Helmet_22"),
        "UNIT_INVENTORY_CHANGED for the inspected unit repaints the slot"
    );

    // An event for somebody else must NOT repaint (the ref's arg1 filter).
    let mut other = inspect_view("target");
    other.slots[1] = Some(foreign_slot(1, "Interface\\Icons\\INV_Helmet_ZZ", "Nope"));
    s.set_inspect(Some(other));
    s.fire_event(
        "UNIT_INVENTORY_CHANGED",
        vec![benilla_ui::script::ScriptValue::Str("party4".into())],
    );
    assert!(
        drawn(&mut s, "INV_Helmet_22") && !drawn(&mut s, "INV_Helmet_ZZ"),
        "an event for another unit leaves the doll alone"
    );

    // Rotate: the ref's sign convention (left subtracts, right adds 0.03) onto the booth yaw, with
    // the kit each click.
    s.run("InspectModelRotateLeftButton:Click()").unwrap();
    assert!(
        (s.model_pane_facing("InspectModelFrame") - 0.58).abs() < 0.0001,
        "rotate-left subtracts 0.03, got {}",
        s.model_pane_facing("InspectModelFrame")
    );
    s.run("InspectModelRotateRightButton:Click()").unwrap();
    s.run("InspectModelRotateRightButton:Click()").unwrap();
    assert!(
        (s.model_pane_facing("InspectModelFrame") - 0.64).abs() < 0.0001,
        "rotate-right adds 0.03, got {}",
        s.model_pane_facing("InspectModelFrame")
    );
    assert_eq!(
        s.take_sounds(),
        vec![
            SoundRequest::KitName("igInventoryRotateCharacter".into()),
            SoundRequest::KitName("igInventoryRotateCharacter".into()),
            SoundRequest::KitName("igInventoryRotateCharacter".into()),
        ],
        "each rotate click plays the kit"
    );

    // Closing plays the close sound AND tells the app to stop inspecting — without the latter the
    // feed would keep resolving a closed window's target forever.
    assert!(
        !s.take_inspect_clear(),
        "ClearInspectPlayer not called while open"
    );
    s.run(r#"HideUIPanel(InspectFrame)"#).unwrap();
    assert!(!s.eval::<bool>("return InspectFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoClose".into())],
        "closing plays igCharacterInfoClose"
    );
    assert!(
        s.take_inspect_clear(),
        "closing called ClearInspectPlayer (the ref's InspectFrame_OnHide)"
    );
    assert!(s.errors().is_empty(), "close errors: {:?}", s.errors());
}

/// The tab chrome, and where the ref's "toggle the showing page shut" branch actually lives.
///
/// `PanelTemplates_SelectTab` **disables** the selected tab button (`UiPanels.xml`, the ref's own
/// UIPanelTemplates), so the SELECTED tab is inert from the moment it opens — clicking it
/// cannot close anything. `ToggleInspect`'s close-when-already-showing branch
/// (`Blizzard_InspectUI.lua:67-83`) is therefore reachable only by calling `ToggleInspect` directly,
/// which is how a keybinding would. Both halves asserted, because the first one is what makes the
/// second one non-obvious.
#[test]
fn the_active_tab_is_inert_and_toggle_inspect_closes() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = armed();
    s.run(r#"InspectUnit("target")"#).unwrap();
    let _ = s.take_sounds();

    // NB this engine's `IsEnabled` returns a plain bool, not the live API's 1/nil.
    assert!(
        !s.eval::<bool>("return InspectFrameTab1:IsEnabled() ~= 0")
            .unwrap(),
        "the selected tab is disabled (PanelTemplates_SelectTab), so its click is inert"
    );
    s.run("InspectFrameTab1:Click()").unwrap();
    assert!(
        s.eval::<bool>("return InspectFrame:IsVisible()").unwrap(),
        "clicking the disabled active tab changes nothing"
    );

    // The reachable path: ToggleInspect on the page already showing closes the window.
    s.run(r#"ToggleInspect("InspectPaperDollFrame")"#).unwrap();
    assert!(
        !s.eval::<bool>("return InspectFrame:IsVisible()").unwrap(),
        "ToggleInspect on the showing page closes the window (ref ToggleInspect)"
    );
    assert!(s.errors().is_empty(), "tab errors: {:?}", s.errors());
}

/// The other branch of `ToggleInspect` — re-opening after a close — **errors, and that is the
/// reference's own behaviour** (decision 1834 settled what 1832 had to leave open).
///
/// `InspectFrame_OnHide` sets `this.unit = nil`, so a re-open runs `InspectFrame_OnShow` with no
/// token. In the reference, `SetPortraitTexture 0x519ef0` gates its unit
/// argument at `0x519fb4` with `lua_isstring` and raises `Usage: SetPortraitTexture(texture,
/// "unit")`, and `luaL_error` does not return — so the reference raises here too, on its own
/// shipped code. The widget clear sits six instructions PAST the gate, so the portrait is not even
/// blanked; it is left exactly as it was.
///
/// In the reference the raise unwinds to the handler's `lua_pcall`, abandons the rest of `OnShow`,
/// and pops the ScriptErrors dialog through `_ERRORMESSAGE`. Ours collects it into `errors` and
/// abandons the handler the same way. So: the window shows, the portrait and name do not update,
/// and there is exactly one error — which is what this now asserts.
///
/// One correction worth keeping: stock 1.12 ships **no** inspect keybind (0 of 234
/// `Binding name=` entries) and no caller of `ToggleInspect` outside the addon's own tab buttons,
/// which are children of the hidden `InspectFrame`. So on the stock UI's own paths this is
/// unreachable — only an addon or a macro gets here. It is still ours to reproduce.
#[test]
fn toggle_inspect_reopens_after_a_close_and_raises_like_the_reference() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = armed();
    s.run(r#"InspectUnit("target")"#).unwrap();
    s.run(r#"ToggleInspect("InspectPaperDollFrame")"#).unwrap();
    assert!(!s.eval::<bool>("return InspectFrame:IsVisible()").unwrap());
    let _ = s.errors();

    s.run(r#"ToggleInspect("InspectPaperDollFrame")"#).unwrap();
    assert!(
        s.eval::<bool>("return InspectFrame:IsVisible()").unwrap(),
        "the window still shows — Show() runs before OnShow raises"
    );
    // TWO raises, not one: `InspectFrame` and `InspectPaperDollFrame` each have their own OnShow,
    // each fired under its own pcall, so each abandons independently — the first at the portrait's
    // gate, the second at `UnitLevel`'s. Both carry the reference's own `Usage:` text.
    let errors = s.errors();
    assert_eq!(errors.len(), 2, "one raise per OnShow handler: {errors:?}");
    assert!(
        errors[0].contains(r#"Usage: SetPortraitTexture(texture, "unit")"#),
        "the portrait's gate, before the name line is ever reached: {errors:?}"
    );
    assert!(
        errors[1].contains(r#"Usage: UnitLevel("unit")"#),
        "and the paper doll's own handler, dying at its first getter: {errors:?}"
    );
}
