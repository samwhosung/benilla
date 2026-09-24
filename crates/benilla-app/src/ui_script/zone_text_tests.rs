//! The reference's own `ZoneText.xml` — executed off the player's chain since 1751's eighth
//! window — driven engine-only: the zone splash shows on the
//! right events with the right strings/colors, fades on the reference 0.5/1.0/2.0 timeline, and
//! honors the subtle law — a plain `ZONE_CHANGED` re-caches the zone name *silently*, so a later
//! `ZONE_CHANGED_NEW_AREA` that lands on the already-cached name never re-splashes.
//!
//! The test plays the app's role by hand: write the zone host globals (what
//! `crate::area::feed_zone_events` pushes), fire the event, tick the clock.

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

/// Push the host globals the app writes for an area transition. Long-bracket Lua strings, so a
/// name with an apostrophe ("Lion's Pride Inn") survives.
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
    // The player's own strings: the splash formats CONTESTED_TERRITORY,
    // FACTION_CONTROLLED_TERRITORY, FREE_FOR_ALL_TERRITORY and the AUTOFOLLOW pair straight out of
    // GlobalStrings. Our deleted copy carried `X = X or "…"` fallbacks for a harness with no
    // chain; the reference's file has none, and it should not.
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
    // The fading kit, its own manifest entry since window 8 — as it is in the reference's TOC.
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

    // Cross into Westfall proper (Alliance-owned): the big name + the territory line show.
    set_area(&s, "Westfall", "", "friendly", "Alliance");
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    assert!(visible(&s, "ZoneTextFrame"), "zone splash shows");
    assert_eq!(text_of(&s, "ZoneTextString"), "Westfall");
    assert_eq!(text_of(&s, "PVPInfoTextString"), "Alliance Territory");
    assert_eq!(text_of(&s, "SubZoneTextString"), "");

    // The fade timeline (ref: in 0.5, hold 1.0, out 2.0). Mid-fade-in: alpha ≈ elapsed/0.5.
    s.tick(0.25);
    let alpha: f32 = s.eval("return ZoneTextFrame:GetAlpha()").unwrap();
    assert!((alpha - 0.5).abs() < 0.05, "mid-fade-in alpha, got {alpha}");
    // Into the hold plateau.
    s.tick(0.5);
    let alpha: f32 = s.eval("return ZoneTextFrame:GetAlpha()").unwrap();
    assert!((alpha - 1.0).abs() < 0.01, "hold alpha, got {alpha}");
    // Past in+hold+out: hidden again.
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

    // Goldshire: same zone, new subzone → the subzone frame alone.
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

    // The reference law: a plain ZONE_CHANGED updates ZoneTextFrame.zoneText and returns —
    // no splash even though the zone text changed…
    set_area(&s, "Westfall", "The Jansen Stead", "friendly", "Alliance");
    s.fire_event("ZONE_CHANGED", vec![]);
    assert!(
        !visible(&s, "ZoneTextFrame"),
        "plain ZONE_CHANGED never splashes the zone name"
    );
    // …and a NEW_AREA landing on the now-cached name stays silent too.
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    assert!(
        !visible(&s, "ZoneTextFrame"),
        "NEW_AREA on the already-cached zone text must not re-splash"
    );

    // A genuinely new zone splashes again.
    set_area(&s, "Duskwood", "", "contested", "");
    s.fire_event("ZONE_CHANGED_NEW_AREA", vec![]);
    assert!(visible(&s, "ZoneTextFrame"));
    assert_eq!(text_of(&s, "PVPInfoTextString"), "Contested Territory");
}

/// The director's abbey repro, outdoor half: "Northshire Abbey" is a real outdoor SUBZONE
/// (AreaTable row 24, parent Elwynn 12) — walking onto the abbey grounds is a plain subzone hop.
/// The reference shows ONLY the small subzone line: ZoneTextFrame never shows on a plain
/// ZONE_CHANGED, so the territory line (its child region) cannot render — even though
/// SubZoneTextFrame's handler calls SetZoneText(1), which SETS the (hidden) PVP string.
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

/// The abbey repro, indoor half — the byte-corrected feed (`0x67e670`):
/// crossing the abbey threshold, the zone-name override SKIPS (the whole-WMO name
/// "Northshire Abbey" equals the yard subzone), so the ZONE slot stays "Elwynn Forest" and the
/// group row's name re-populates the SUBZONE ("Main Hall"). ZoneTextFrame's text never changes ⇒
/// it stays hidden ⇒ no territory line (its child region) can render — the director's reference
/// truth, produced by the engine feed rather than by handler-order luck.
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

    // Step inside: zone slot UNCHANGED, subzone = the group row's name.
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

/// The INN case — the override FIRES (the whole-WMO name differs from the street subzone):
/// zone slot = the inn's name, subzone nulls (unnamed group rows). The big splash shows the inn
/// name, and — the FIFO dispatch law (`RegisterEvent 0x702140` tail-appends: SubZoneTextFrame
/// registered second fires LAST, its SetZoneText(1) is the last writer) — the territory line
/// shows under it, exactly as on a NEW_AREA splash.
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

/// Leaving the interior: the subzone reverts to the leaf name (a TEXT change, outdoors ⇒ plain
/// ZONE_CHANGED ⇒ ZoneTextFrame only re-caches, silently) — the small line splashes alone,
/// exactly like the entry hop.
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

/// Room→room inside one building (the corrected feed: the SUBZONE slot hops "Main Hall" →
/// "Library Wing", the zone slot stays the DBC zone): the room name splashes in the small line;
/// the big frame never shows, so no territory line.
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

/// An FFA pit (GetZonePVPInfo's isArena — leaf Flags bit 0x80): the subzone line splashes with
/// the "PvP Area" arena string under it, red per the quote's PVPArenaTextString color.
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

/// The subzone seat law: with PvP info showing (a friendly NEW_AREA), SubZoneTextString's TOP
/// anchors to PVPInfoTextString's BOTTOM — the three-line stack the director's screenshot shows.
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

/// `AutoFollowStatus` — the third frame in `ZoneText.xml`, and the only thing `ref-ZoneText.lua`
/// actually contains. The app fires `AUTOFOLLOW_BEGIN` with the followee's name and
/// `AUTOFOLLOW_END` with nothing; the frame does the rest.
///
/// The load-bearing assertion is that **END reuses the name BEGIN latched**. That is why the app
/// gets to fire END with no argument at all, and why [`crate::player::FollowState`] latches the
/// name at start rather than re-reading it — by the time a follow ends, the followee is often gone.
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

    // A follow that runs for a while must NOT fade — the fade is armed only by END, and there is
    // no hold timer to expire (this is not the FadingFrame kit).
    s.tick(10.0);
    assert!(visible(&s, "AutoFollowStatus"), "no fade while following");
    let alpha: f32 = s.eval("return AutoFollowStatus:GetAlpha()").unwrap();
    assert!((alpha - 1.0).abs() < 0.01, "still opaque after 10 s");

    // END carries no argument: the line reuses the latched name.
    s.fire_event("AUTOFOLLOW_END", vec![]);
    s.resolve();
    assert_eq!(
        text_of(&s, "AutoFollowStatusText"),
        "You stop following Probeone.",
        "END has no argument of its own — the name comes from what BEGIN latched"
    );
    // A linear 4 s fade, no hold: half gone at 2 s, hidden past 4.
    s.tick(2.0);
    let alpha: f32 = s.eval("return AutoFollowStatus:GetAlpha()").unwrap();
    assert!((alpha - 0.5).abs() < 0.05, "mid-fade alpha, got {alpha}");
    s.tick(2.5);
    assert!(!visible(&s, "AutoFollowStatus"), "gone after the 4 s fade");
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// A follow that switches subject fires `BEGIN` alone (see `crate::ui_follow`'s header). The
/// reference's own handler treats BEGIN as a full reset — it clears `fadeTime`, restores alpha and
/// re-shows — so a switch made *during* the end-fade must come back to a clean, opaque line rather
/// than inheriting the dying one's alpha.
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
