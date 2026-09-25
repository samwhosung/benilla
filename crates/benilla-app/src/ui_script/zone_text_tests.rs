//! Stock `ZoneText.xml`, driven by hand as `crate::area::feed_zone_events` drives it: host globals
//! written, the event fired, the clock ticked. A plain `ZONE_CHANGED` re-caches the zone name
//! silently (`ZoneText.xml:92`), so a later `ZONE_CHANGED_NEW_AREA` on that name never splashes.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

fn visible(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!("return {frame}:IsVisible() and true or false"))
        .unwrap()
}

fn text_of(s: &UiScript, fontstring: &str) -> String {
    s.eval::<String>(&format!("return {fontstring}:GetText() or ''"))
        .unwrap()
}

/// The host globals the app writes on an area change, in long brackets so an apostrophe survives.
fn set_area(s: &UiScript, zone: &str, sub: &str, pvp: &str, faction: &str) {
    s.run(&format!(
        "__benilla_zone_name = [[{zone}]]; __benilla_subzone_name = [[{sub}]]; \
         __benilla_pvp_type = [[{pvp}]]; __benilla_pvp_faction = [[{faction}]]; \
         __benilla_pvp_arena = false"
    ))
    .unwrap();
}

fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The splash formats its territory and autofollow lines straight out of GlobalStrings.
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // `TEXT()`, which AutoFollowStatus_OnEvent puts its message through.
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    // The fading kit, its own `FrameXML.toc` entry.
    load_xml(&s, "Interface\\FrameXML\\FadingFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\ZoneText.xml");
    s
}

#[test]
fn new_area_splashes_zone_pvp_and_subzone_then_fades_out() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();

    // OnLoad hides both fading frames.
    assert!(!visible(&s, "ZoneTextFrame"));
    assert!(!visible(&s, "SubZoneTextFrame"));

    set_area(&s, "Westfall", "", "friendly", "Alliance");
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    assert!(visible(&s, "ZoneTextFrame"), "zone splash shows");
    assert_eq!(text_of(&s, "ZoneTextString"), "Westfall");
    assert_eq!(text_of(&s, "PVPInfoTextString"), "Alliance Territory");
    assert_eq!(text_of(&s, "SubZoneTextString"), "");

    // The stock timeline: in 0.5 s, hold 1.0 s, out 2.0 s (`ZoneText.xml:5`).
    s.tick(0.25);
    let alpha: f32 = s.eval("return ZoneTextFrame:GetAlpha()").unwrap();
    assert!((alpha - 0.5).abs() < 0.05, "mid-fade-in alpha, got {alpha}");
    s.tick(0.5);
    let alpha: f32 = s.eval("return ZoneTextFrame:GetAlpha()").unwrap();
    assert!((alpha - 1.0).abs() < 0.01, "hold alpha, got {alpha}");
    s.tick(3.0);
    assert!(!visible(&s, "ZoneTextFrame"), "fade-out ends in Hide()");
}

#[test]
fn subzone_hop_shows_only_the_small_line() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    set_area(&s, "Elwynn Forest", "", "friendly", "Alliance");
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    s.tick(4.0); // let the login splash finish
    assert!(!visible(&s, "ZoneTextFrame"));

    set_area(&s, "Elwynn Forest", "Goldshire", "friendly", "Alliance");
    s.fire_event("ZONE_CHANGED", vec![]);
    assert!(visible(&s, "SubZoneTextFrame"), "subzone splash shows");
    assert!(
        !visible(&s, "ZoneTextFrame"),
        "the big zone name stays hidden on a subzone hop"
    );
    assert_eq!(text_of(&s, "SubZoneTextString"), "Goldshire");
}

#[test]
fn plain_zone_changed_recaches_silently_so_new_area_wont_resplash() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    set_area(&s, "Elwynn Forest", "", "friendly", "Alliance");
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    s.tick(4.0);
    assert!(!visible(&s, "ZoneTextFrame"));

    set_area(&s, "Westfall", "The Jansen Stead", "friendly", "Alliance");
    s.fire_event("ZONE_CHANGED", vec![]);
    assert!(
        !visible(&s, "ZoneTextFrame"),
        "plain ZONE_CHANGED never splashes the zone name"
    );
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    assert!(
        !visible(&s, "ZoneTextFrame"),
        "NEW_AREA on the already-cached zone text must not re-splash"
    );

    set_area(&s, "Duskwood", "", "contested", "");
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    assert!(visible(&s, "ZoneTextFrame"));
    assert_eq!(text_of(&s, "PVPInfoTextString"), "Contested Territory");
}

/// Northshire Abbey is an outdoor subzone of Elwynn (`AreaTable` row 24, parent 12), so entering
/// it is a plain `ZONE_CHANGED`; the territory line is `ZoneTextFrame`'s child, which stays hidden.
#[test]
fn abbey_grounds_subzone_hop_shows_no_territory_line() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    set_area(
        &s,
        "Elwynn Forest",
        "Northshire Valley",
        "friendly",
        "Alliance",
    );
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    s.tick(4.0); // both splashes fully faded

    set_area(
        &s,
        "Elwynn Forest",
        "Northshire Abbey",
        "friendly",
        "Alliance",
    );
    s.fire_event("ZONE_CHANGED", vec![]);
    assert!(visible(&s, "SubZoneTextFrame"), "the subzone line splashes");
    assert_eq!(text_of(&s, "SubZoneTextString"), "Northshire Abbey");
    assert!(
        !visible(&s, "ZoneTextFrame"),
        "ZoneTextFrame stays hidden on a plain subzone hop — no territory line can show"
    );
}

/// Indoors, the zone-name override skips when the whole-WMO name equals the subzone (`0x67e670`),
/// so the zone stays and the group's name becomes the subzone: no big splash, no territory line.
#[test]
fn abbey_interior_shows_the_room_in_the_small_line_alone() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    set_area(
        &s,
        "Elwynn Forest",
        "Northshire Abbey",
        "friendly",
        "Alliance",
    );
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    s.tick(4.0);

    set_area(&s, "Elwynn Forest", "Main Hall", "friendly", "Alliance");
    s.fire_event("ZONE_CHANGED_INDOORS", vec![]);
    assert!(
        !visible(&s, "ZoneTextFrame"),
        "the zone text did not change — no big splash, so no territory line"
    );
    assert!(
        visible(&s, "SubZoneTextFrame"),
        "the room name splashes small"
    );
    assert_eq!(text_of(&s, "SubZoneTextString"), "Main Hall");
}

/// An inn's WMO name differs from the street's, so the override makes it the zone. Handlers run in
/// registration order (`0x702140` appends), so `SubZoneTextFrame`'s `SetZoneText(1)` writes last.
#[test]
fn inn_entry_splashes_the_inn_name_with_territory_line() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    set_area(&s, "Elwynn Forest", "Goldshire", "friendly", "Alliance");
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    s.tick(4.0);

    set_area(&s, "Lion's Pride Inn", "", "friendly", "Alliance");
    s.fire_event("ZONE_CHANGED_INDOORS", vec![]);
    assert!(visible(&s, "ZoneTextFrame"), "the inn name splashes big");
    assert_eq!(text_of(&s, "ZoneTextString"), "Lion's Pride Inn");
    assert_eq!(
        text_of(&s, "PVPInfoTextString"),
        "Alliance Territory",
        "SubZoneTextFrame fires last (FIFO) — its SetZoneText(1) leaves the territory line set"
    );
}

#[test]
fn indoor_exit_returns_the_subzone_line_alone() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    set_area(
        &s,
        "Elwynn Forest",
        "Northshire Abbey",
        "friendly",
        "Alliance",
    );
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    s.run("__benilla_subzone_name = 'Main Hall'").unwrap();
    s.fire_event("ZONE_CHANGED_INDOORS", vec![]);
    s.tick(4.0); // everything faded

    set_area(
        &s,
        "Elwynn Forest",
        "Northshire Abbey",
        "friendly",
        "Alliance",
    );
    s.fire_event("ZONE_CHANGED", vec![]);
    assert!(visible(&s, "SubZoneTextFrame"));
    assert_eq!(text_of(&s, "SubZoneTextString"), "Northshire Abbey");
    assert!(
        !visible(&s, "ZoneTextFrame"),
        "nothing changed the zone text — cache-only, no splash"
    );
}

#[test]
fn room_to_room_hop_splashes_the_room_name_alone() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    set_area(&s, "Elwynn Forest", "Main Hall", "friendly", "Alliance");
    s.fire_event("ZONE_CHANGED_INDOORS", vec![]);
    s.tick(4.0);

    set_area(&s, "Elwynn Forest", "Library Wing", "friendly", "Alliance");
    s.fire_event("ZONE_CHANGED_INDOORS", vec![]);
    assert!(
        !visible(&s, "ZoneTextFrame"),
        "no zone-text change, no big splash"
    );
    assert!(visible(&s, "SubZoneTextFrame"));
    assert_eq!(text_of(&s, "SubZoneTextString"), "Library Wing");
}

/// An FFA pit: `GetZonePVPInfo`'s isArena, the leaf area's flag `0x80`.
#[test]
fn arena_pit_shows_the_ffa_line() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    set_area(&s, "Stranglethorn Vale", "", "contested", "");
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    s.tick(4.0);

    s.run(
        "__benilla_subzone_name = 'Gurubashi Arena'; __benilla_zone_text = 'Gurubashi Arena'; \
         __benilla_pvp_arena = true",
    )
    .unwrap();
    s.fire_event("ZONE_CHANGED", vec![]);
    assert!(visible(&s, "SubZoneTextFrame"));
    assert_eq!(text_of(&s, "SubZoneTextString"), "Gurubashi Arena");
    assert_eq!(text_of(&s, "PVPArenaTextString"), "PvP Area");
}

/// With PvP info showing, `SubZoneTextString`'s `TOP` anchors to `PVPInfoTextString`'s `BOTTOM`.
#[test]
fn subzone_seat_hangs_under_the_territory_line_on_new_area() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    set_area(
        &s,
        "Stormwind City",
        "Valley of Heroes",
        "friendly",
        "Alliance",
    );
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    s.resolve();
    let ok: bool = s
        .eval(
            "return SubZoneTextString:GetTop() == PVPInfoTextString:GetBottom() \
               and PVPInfoTextString:GetTop() == ZoneTextString:GetBottom()",
        )
        .unwrap();
    assert!(
        ok,
        "zone → territory → subzone, each seated at the previous line's bottom"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// `AutoFollowStatus`, all of `ZoneText.lua`: END reuses the name BEGIN latched, so the app fires
/// END with no argument.
#[test]
fn the_autofollow_status_line_names_the_followee_and_fades_on_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    assert!(
        !visible(&s, "AutoFollowStatus"),
        "hidden until a follow begins"
    );

    s.fire_event(
        "AUTOFOLLOW_BEGIN",
        vec![benilla_ui::script::ScriptValue::Str("Probeone".into())],
    );
    s.resolve();
    assert!(visible(&s, "AutoFollowStatus"));
    assert_eq!(text_of(&s, "AutoFollowStatusText"), "Following Probeone.");
    let alpha: f32 = s.eval("return AutoFollowStatus:GetAlpha()").unwrap();
    assert!((alpha - 1.0).abs() < 0.01, "full alpha while following");

    // Only END arms the fade; there is no hold timer (`ZoneText.lua:19`).
    s.tick(10.0);
    assert!(visible(&s, "AutoFollowStatus"), "no fade while following");
    let alpha: f32 = s.eval("return AutoFollowStatus:GetAlpha()").unwrap();
    assert!((alpha - 1.0).abs() < 0.01, "still opaque after 10 s");

    s.fire_event("AUTOFOLLOW_END", vec![]);
    s.resolve();
    assert_eq!(
        text_of(&s, "AutoFollowStatusText"),
        "You stop following Probeone.",
        "END has no argument of its own — the name comes from what BEGIN latched"
    );
    // `AUTOFOLLOW_STATUS_FADETIME`: a linear 4 s fade.
    s.tick(2.0);
    let alpha: f32 = s.eval("return AutoFollowStatus:GetAlpha()").unwrap();
    assert!((alpha - 0.5).abs() < 0.05, "mid-fade alpha, got {alpha}");
    s.tick(2.5);
    assert!(!visible(&s, "AutoFollowStatus"), "gone after the 4 s fade");
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// A switch of subject fires BEGIN alone, and the stock BEGIN arm clears `fadeTime` and restores
/// alpha (`ZoneText.lua:11`).
#[test]
fn a_begin_during_the_end_fade_resets_the_line() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event(
        "AUTOFOLLOW_BEGIN",
        vec![benilla_ui::script::ScriptValue::Str("Probeone".into())],
    );
    s.fire_event("AUTOFOLLOW_END", vec![]);
    s.tick(3.0); // most of the way through the fade
    let faded: f32 = s.eval("return AutoFollowStatus:GetAlpha()").unwrap();
    assert!(faded < 0.4, "mid-fade, got {faded}");

    s.fire_event(
        "AUTOFOLLOW_BEGIN",
        vec![benilla_ui::script::ScriptValue::Str("Probetwo".into())],
    );
    s.resolve();
    assert_eq!(text_of(&s, "AutoFollowStatusText"), "Following Probetwo.");
    let alpha: f32 = s.eval("return AutoFollowStatus:GetAlpha()").unwrap();
    assert!(
        (alpha - 1.0).abs() < 0.01,
        "BEGIN restores alpha, got {alpha}"
    );
    s.tick(10.0);
    assert!(
        visible(&s, "AutoFollowStatus"),
        "and clears the pending fade rather than letting it finish"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}
