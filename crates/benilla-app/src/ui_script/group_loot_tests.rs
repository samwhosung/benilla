//! The group-loot roll popups (stock `Interface\FrameXML\GroupLootFrame.xml`): the
//! four stacked `GroupLootFrame`s that answer `START_LOOT_ROLL`/`CANCEL_LOOT_ROLL` off a pushed
//! [`LootRollsState`] snapshot (the `loot_roll.rs` seam's own harness idiom, mirrored here the way
//! `loot_tests.rs` mirrors it for `set_loot`/`LootState`).

use benilla_ui::script::{
    DressUpIntent, ExtractedQuad, LootRollEntry, LootRollsState, QuadContent, ScriptValue, UiScript,
};

use super::test_ui::{load_ui as load_xml, load_ui_no_warnings as load_xml_no_warnings};

/// Like [`load_xml`], but also demands zero loader WARNINGS — the bar this file's own assignment
/// set for `GroupLootFrame.xml` itself (a stale "unknown template" warning, e.g. the kind
/// LootFrame.xml's now-obsolete `GameFontNormalSmall` caveat would have produced, is exactly what
/// this catches). Not used for the prerequisite files below: Fonts/UiPanels/GameTooltip aren't this
/// assignment's to police and may carry warnings of their own.
/// The centre of the first texture quad whose path contains `needle` (a button's art), for clicking
/// it — the `loot_tests.rs` `icon_center` idiom.
fn quad_center(quads: &[ExtractedQuad], needle: &str) -> (f32, f32) {
    let r = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
        })
        .and_then(|q| q.rect)
        .unwrap_or_else(|| panic!("no quad for {needle}"));
    ((r.left + r.right) * 0.5, (r.bottom + r.top) * 0.5)
}

/// The colour of the first text quad whose text equals `t` (the `loot_tests.rs` `text_color` idiom).
fn text_color(quads: &[ExtractedQuad], t: &str) -> Option<[f32; 4]> {
    quads.iter().find_map(|q| match &q.content {
        QuadContent::Text {
            text: Some(x),
            color,
            ..
        } if x == t => Some(*color),
        _ => None,
    })?
}

/// The four roll popups, off the player's chain.
///
/// They are declared in stock `LootFrame.xml` — `GroupLootFrameTemplate` plus `GroupLootFrame1..4`
/// — and that file has been on the manifest since the loot window migrated. Our own
/// `GroupLootFrame.xml` re-declared all five names on top of it and won by load order, so what
/// these tests exercised was our copy shadowing the chain's.
///
/// `UIParent.xml` comes with it because the `START_LOOT_ROLL` router lives there now — the
/// reference's own slot for it — where our file used to carry a dedicated hidden driver frame.
fn load_group_loot(s: &UiScript) {
    // `UIParent.xml` is loaded by `setup()` above — the stock main bar needs it first (1938).
    // `LootFrame.xml` brings the whole loot window, and its `GroupLootDropDown` calls
    // `UIDropDownMenu_Initialize` from its own OnLoad — the dropdown kit is on the chain too.
    load_xml(s, r"Interface\FrameXML\UIDropDownMenu.xml");
    // `MAX_PARTY_MEMBERS` — `GroupLootDropDown_OnLoad` counts party rows, and the constant lives
    // in stock `PartyMemberFrame.lua`. A real cross-file dependency the reference has too (its own
    // toc loads PartyFrame well before LootFrame); the `.lua` alone is the minimal way to satisfy
    // it here, because its whole top level is four constants and some function definitions — no
    // frames, no side effects — where `PartyFrame.xml` would drag in the entire unit-frame cluster
    // for one number.
    load_xml(s, r"Interface\FrameXML\PartyMemberFrame.lua");
    load_xml(s, r"Interface\FrameXML\LootFrame.xml");
}

fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The loot window's own labels (`ITEMS`, `PREV`, `NEXT`) are GlobalStrings keys, and the
    // loader warns on a key with no global behind it rather than failing — which is exactly the
    // kind of warning `load_ui_no_warnings` is here to catch.
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml"); // ITEM_QUALITY_COLORS + GameFontNormalSmall
                                                    // The loot window's slots inherit it — the same dependency the inspect window needed (1832).
    load_xml(&s, r"Interface\FrameXML\ItemButtonTemplate.xml");
    // `UIPanelCloseButton`, which the loot window's four close buttons inherit.
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml"); // PASS/NEED/GREED + item hovers
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\MainMenuBar.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionBarFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BonusActionBarFrame.xml"); // BENILLA_FALLBACK_ICON (the in-flight icon fallback) —
                                                                  // buff_tests.rs's own load-order precedent for this same global.
    s
}

/// The two resolved rolls' item links, exactly as `ui_loot_roll.rs` builds them (`item_link`:
/// quality colour + the four-field `|Hitem:` payload) — what the icon button's ctrl/shift arms read.
const STAFF_LINK: &str = "|cffa335ee|Hitem:17182:0:0:0|h[Staff of Jordan]|h|r";
const SWORD_LINK: &str = "|cffffffff|Hitem:25:0:0:0|h[Worn Shortsword]|h|r";

/// A resolved Epic (BoP) roll and an in-flight one (item template not landed yet) — the same two
/// shapes `loot_roll.rs`'s own test module fixtures, reused here against the real shipped XML.
fn rolls() -> LootRollsState {
    LootRollsState {
        rolls: vec![
            LootRollEntry {
                roll_id: 7,
                name: Some("Staff of Jordan".into()),
                texture: Some("Interface\\Icons\\INV_Staff_12".into()),
                quantity: 1,
                quality: Some(4), // Epic -> purple
                bind_on_pickup: true,
                time_left_ms: 42_000,
                item_id: 17182,
                // The link lands with the name — one template answer fills both.
                link: Some(STAFF_LINK.into()),
                random_property_id: 0,
            },
            LootRollEntry {
                roll_id: 8,
                name: Some("Worn Shortsword".into()),
                texture: Some("Interface\\Icons\\INV_Sword_04".into()),
                quantity: 1,
                quality: Some(1), // Common -> white
                bind_on_pickup: false,
                time_left_ms: 60_000,
                item_id: 25,
                link: Some(SWORD_LINK.into()),
                random_property_id: 0,
            },
            // The item-template query hasn't landed: name/texture/quality all nil (loot_roll.rs) —
            // and no link either: it embeds the name, so it cannot exist before the name does.
            LootRollEntry {
                roll_id: 9,
                name: None,
                texture: None,
                quantity: 1,
                quality: None,
                bind_on_pickup: false,
                time_left_ms: 55_000,
                item_id: 4306,
                link: None,
                random_property_id: 0,
            },
        ],
    }
}

/// The XML loads with zero loader warnings/errors, and all four `GroupLootFrame1..4` exist
/// and start hidden.
#[test]
fn shipped_group_loot_frame_loads_clean_and_starts_hidden() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIDropDownMenu.xml");
    load_xml(&s, r"Interface\FrameXML\PartyMemberFrame.lua");
    // The four roll popups arrive INSIDE the chain's loot window, so there is no exact frame count
    // to assert any more: this used to read `4 * 6 + 1` — six regions per instance plus our own
    // `BenillaGroupLootFrameDriver` — and both halves of that number were ours.
    // What survives is the assignment that mattered: the file loads with **no warning of any
    // kind**, which is what `load_ui_no_warnings` is for.
    assert!(
        load_xml_no_warnings(&s, r"Interface\FrameXML\LootFrame.xml") > 4,
        "the loot window brought its frames"
    );

    for i in 1..=4 {
        let name = format!("GroupLootFrame{i}");
        assert!(
            !s.eval::<bool>(&format!("return {name}:IsVisible()"))
                .unwrap(),
            "{name} starts hidden"
        );
    }
}

/// `START_LOOT_ROLL` claims the first free frame and shows it; a second `START_LOOT_ROLL` claims
/// the second frame, the first staying up. Also exercises the resolved-roll paint (name text +
/// quality colour + the BoP gold decoration swap + the Timer's primed max) and the Need/Greed/Pass
/// click wiring end-to-end through `RollOnLoot`.
#[test]
fn start_loot_roll_claims_frames_in_order_and_paints_the_roll() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    load_group_loot(&s);
    s.set_loot_rolls(rolls());

    // Roll 7 (BoP, Epic) claims frame 1.
    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(7), ScriptValue::Int(42_000)],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return GroupLootFrame1:IsVisible()")
        .unwrap());
    assert_eq!(s.eval::<i64>("return GroupLootFrame1.rollID").unwrap(), 7);
    // The Timer's max was primed to the roll's countdown before Show() (GroupLootFrame_OpenNewFrame).
    assert_eq!(
        s.eval::<(f64, f64)>("return GroupLootFrame1Timer:GetMinMaxValues()")
            .unwrap(),
        (0.0, 42_000.0)
    );
    // BoP swaps in the gold decoration; frame 2 doesn't exist yet to compare against.
    assert!(s
        .eval::<bool>("return GroupLootFrame1Decoration:IsShown()")
        .unwrap());

    // Roll 8 (BoE, Common) claims frame 2 — frame 1 stays up, untouched.
    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(8), ScriptValue::Int(60_000)],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return GroupLootFrame1:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return GroupLootFrame2:IsVisible()")
        .unwrap());
    assert_eq!(s.eval::<i64>("return GroupLootFrame2.rollID").unwrap(), 8);
    // A BoE roll keeps the plain decoration hidden.
    assert!(!s
        .eval::<bool>("return GroupLootFrame2Decoration:IsShown()")
        .unwrap());

    // The names painted, quality-coloured (frame 1 purple/Epic, frame 2 white/Common).
    s.resolve();
    let quads = s.extract();
    assert_eq!(
        s.eval::<String>("return GroupLootFrame1Name:GetText()")
            .unwrap(),
        "Staff of Jordan"
    );
    let purple = text_color(&quads, "Staff of Jordan").expect("Staff of Jordan colour");
    assert!(
        (purple[0] - 0.64).abs() < 0.02 && (purple[1] - 0.21).abs() < 0.02,
        "Epic item text is purple, got {purple:?}"
    );
    let white = text_color(&quads, "Worn Shortsword").expect("Worn Shortsword colour");
    assert!(
        (white[0] - 1.0).abs() < 0.02 && (white[1] - 1.0).abs() < 0.02,
        "Common item text is white, got {white:?}"
    );

    // Clicking Need on frame 1's roll button reaches RollOnLoot(7, 1) — the same click-through
    // proof loot_tests.rs runs for LootSlot. Roll 7 is BIND-ON-PICKUP, so the seam's gate holds
    // the vote back and asks instead: nothing on the wire until the popup is
    // accepted.
    //
    // Taken BY NAME, not by painter order. This used to be
    // `quad_center(&quads, "UI-GroupLoot-Dice-Up")` — the first dice quad — which silently assumed
    // declaration order is draw order among the four roll frames. `toplevel="true"` (decision
    // 1739) ends that: the frame a roll claims raises above its siblings on Show, so the first
    // dice belonged to frame 2 and the click landed on the BoE roll, which has no confirm gate and
    // went straight to the wire. The proxy was the fixture, never the assertion.
    let (nx, ny) = s
        .eval::<(f64, f64)>(
            "return (GroupLootFrame1RollButton:GetLeft() + GroupLootFrame1RollButton:GetRight()) \
             / 2, (GroupLootFrame1RollButton:GetBottom() + GroupLootFrame1RollButton:GetTop()) / 2",
        )
        .unwrap();
    let (nx, ny) = (nx as f32, ny as f32);
    assert_eq!(
        s.hit_test_name(nx, ny).as_deref(),
        Some("GroupLootFrame1RollButton"),
        "the click has to land on frame 1's Need button for the rest of this to mean anything"
    );
    s.mouse_button(nx, ny, "LeftButton", true);
    s.mouse_button(nx, ny, "LeftButton", false);
    assert!(
        s.take_loot_roll_votes().is_empty(),
        "a BoP Need must not reach the wire before the confirm"
    );
    assert_eq!(s.take_loot_roll_confirms(), vec![(7, 1)], "Need on roll 7");
}

/// The bind-on-pickup confirm, end to end through the real shipped XML: the
/// driver turns `CONFIRM_LOOT_ROLL` into the popup carrying `(rollID, rollType)`, and the popup's
/// OK re-enters `ConfirmLootRoll` to land the vote the gate withheld.
///
/// 0591 shipped this wrong — a Need on a BoP roll went straight to the wire with no prompt, which
/// binds an epic to you on a single click. The five-line path below is the whole correction.
#[test]
fn the_bop_confirm_popup_lands_the_withheld_vote() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    load_group_loot(&s);
    s.set_loot_rolls(rolls());

    // The app fires this after draining the seam's confirm queue (`ui_loot_roll::drain_loot_rolls`).
    s.fire_event(
        "CONFIRM_LOOT_ROLL",
        vec![ScriptValue::Int(7), ScriptValue::Int(1)],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The popup is up and carries BOTH halves of the identity — data2 is the rollType, which the
    // reference threads through precisely so OnAccept can replay the original click.
    assert!(
        s.eval::<bool>("return StaticPopup_FindVisible(\"CONFIRM_LOOT_ROLL\") ~= nil")
            .unwrap(),
        "the confirm popup should be visible"
    );
    let (data, data2) = s
        .eval::<(i64, i64)>(
            "local d = StaticPopup_FindVisible(\"CONFIRM_LOOT_ROLL\")\nreturn d.data, d.data2",
        )
        .unwrap();
    assert_eq!((data, data2), (7, 1), "rollID and rollType ride the dialog");

    // The real 1.12 LOOT_NO_DROP/OKAY/CANCEL text, the same bar delete_item_tests holds its own
    // popup to — these are quoted locals precisely so they render under test too.
    let dialog = s
        .eval::<String>("return StaticPopup_FindVisible(\"CONFIRM_LOOT_ROLL\"):GetName()")
        .unwrap();
    assert_eq!(
        s.eval::<String>(&format!("return {dialog}Text:GetText()"))
            .unwrap(),
        "Looting this item will bind it to you."
    );
    assert_eq!(
        s.eval::<String>(&format!("return {dialog}Button1:GetText()"))
            .unwrap(),
        "Okay"
    );

    // Still nothing on the wire while the question is open.
    assert!(s.take_loot_roll_votes().is_empty());

    // Accepting re-enters ConfirmLootRoll, which bypasses the bind gate and queues the real vote.
    s.eval::<bool>(
        "local d = StaticPopup_FindVisible(\"CONFIRM_LOOT_ROLL\")\n\
         StaticPopupDialogs[\"CONFIRM_LOOT_ROLL\"].OnAccept(d.data, d.data2)\nreturn true",
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_loot_roll_votes(),
        vec![(7, 1)],
        "OK on the confirm sends the Need the gate withheld"
    );
}

/// `CANCEL_LOOT_ROLL` for a rollID hides that frame, and ONLY that one.
#[test]
fn cancel_loot_roll_hides_only_that_frame() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    load_group_loot(&s);
    s.set_loot_rolls(rolls());

    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(7), ScriptValue::Int(42_000)],
    );
    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(8), ScriptValue::Int(60_000)],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Cancel roll 7 (frame 1): only frame 1 hides, frame 2 (roll 8) is untouched.
    s.fire_event("CANCEL_LOOT_ROLL", vec![ScriptValue::Int(7)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(!s
        .eval::<bool>("return GroupLootFrame1:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return GroupLootFrame2:IsVisible()")
        .unwrap());

    // A cancel for a rollID nobody holds any more (or never held) is a harmless no-op.
    s.fire_event("CANCEL_LOOT_ROLL", vec![ScriptValue::Int(7)]);
    s.fire_event("CANCEL_LOOT_ROLL", vec![ScriptValue::Int(999)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return GroupLootFrame2:IsVisible()")
        .unwrap());

    // Cancel roll 8 (frame 2): the last one standing hides too.
    s.fire_event("CANCEL_LOOT_ROLL", vec![ScriptValue::Int(8)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(!s
        .eval::<bool>("return GroupLootFrame2:IsVisible()")
        .unwrap());
}

/// A roll whose `GetLootRollItemInfo` answers all-nil (the item template still in flight) opens its
/// frame without erroring, and paints the fallback icon / blank name / common-quality colour rather
/// than crashing on the nils.
#[test]
fn in_flight_roll_does_not_error_and_falls_back() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    load_group_loot(&s);
    s.set_loot_rolls(rolls());

    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(9), ScriptValue::Int(55_000)],
    );
    assert!(
        s.errors().is_empty(),
        "an in-flight roll (all-nil GetLootRollItemInfo) must not error: {:?}",
        s.errors()
    );
    assert!(s
        .eval::<bool>("return GroupLootFrame1:IsVisible()")
        .unwrap());
    assert_eq!(s.eval::<i64>("return GroupLootFrame1.rollID").unwrap(), 9);
    // Blank name (not an error, not the literal "nil").
    // The chain's FontString has NO text until something paints it, where our retired copy
    // defaulted to `""` — so this reads Option and expects None.
    assert_eq!(
        s.eval::<Option<String>>("return GroupLootFrame1Name:GetText()")
            .unwrap(),
        None,
        ""
    );
    // No BoP decoration for an unresolved (default-false bindOnPickUp) roll.
    assert!(!s
        .eval::<bool>("return GroupLootFrame1Decoration:IsShown()")
        .unwrap());

    // **No fallback icon — and that is the reference's behaviour, not a regression to fix here.**
    // Our retired file painted `BENILLA_FALLBACK_ICON` (a question mark) whenever the texture was
    // nil. The chain's `GroupLootFrame_OnShow` just calls `SetTexture(texture)` with whatever
    // `GetLootRollItemInfo` gave it, so an unresolved item leaves the icon empty. The migration
    // reverts our embellishment; whether that reads acceptably is a look call, recorded in 1838
    // rather than re-authored back in.
    s.resolve();
    let has_fallback_icon = s.extract().iter().any(|q| {
        matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("INV_Misc_QuestionMark"))
    });
    assert!(
        !has_fallback_icon,
        "the reference paints no placeholder for an unresolved item"
    );
    // What DOES still hold: no error, and the dialog is up and usable.
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The chain frame paints once and never again — which is exactly why the app holds the event.**
///
/// This is the Lua half of decision 2010's fix (bug B371). It drives the sequence the app used to
/// produce — `START_LOOT_ROLL` first, against a model that does not carry the roll yet — and pins
/// that *nothing repairs it*: the snapshot landing afterwards paints no name, no icon and no
/// decoration, for the life of the dialog. Our retired frame did repair it, from a benilla-only
/// `UPDATE_LOOT_ROLL(rollID)`; 1838 migrated to the stock frame, which has no such seam and never
/// listened, and 1883 removed the event.
///
/// The reference is in the same position and answers it upstream, in C: `0x61b310` fires
/// `START_LOOT_ROLL` from the item-template cache's *arrival callback* (`0x61b460`) when the record
/// is not resident. So `feed_loot_rolls` holds the event until the template is in — pinned
/// app-side in `ui_loot_roll.rs`'s `the_roll_waits_for_its_item_template`, which is the test that
/// fails if the hold is lost. This one is its premise: it says what that hold is protecting
/// against, and it must keep reading blank — a repaint appearing here would mean a seam the 1.12
/// client does not have had grown back.
#[test]
fn nothing_repaints_a_frame_that_opened_before_its_snapshot() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    load_group_loot(&s);

    // The order the app must never produce: the event first, against a model with no such roll.
    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(7), ScriptValue::Int(42_000)],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return GroupLootFrame1:IsVisible()")
        .unwrap());
    // The chain's FontString has NO text until something paints it, where our retired copy
    // defaulted to `""` — so this reads Option and expects None.
    assert_eq!(
        s.eval::<Option<String>>("return GroupLootFrame1Name:GetText()")
            .unwrap(),
        None,
        "nothing to paint yet — this is the state B371 reported"
    );

    // ...and then the snapshot carrying it lands.
    s.set_loot_rolls(rolls());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // **The repaint does not happen — there is no path for it.** Our retired file split the paint
    // out of `OnShow` precisely so an `UPDATE_LOOT_ROLL` could re-enter it; that is an event the
    // 1.12 client does not have, and 1883 stopped firing it once the stock frame proved nothing
    // listened. An adapter was tried before the hold and does not work either (1838): calling
    // `GroupLootFrame_OnShow()` from inside a handler repaints nothing, and the `Hide()`/`Show()`
    // round trip that does repaint only does so from a plain chunk.
    //
    // So the ordering is the app's to get right, and it now is: `feed_loot_rolls` holds
    // `START_LOOT_ROLL` until `Items::template` answers, which is the reference's own gate.
    assert_eq!(
        s.eval::<Option<String>>("return GroupLootFrame1Name:GetText()")
            .unwrap(),
        None,
        "the name stays blank: the reference has no repaint path"
    );
    assert!(
        !s.eval::<bool>("return GroupLootFrame1Decoration:IsShown()")
            .unwrap(),
        "and so does the BoP decoration"
    );
    s.resolve();
    let quads = s.extract();
    assert!(
        !quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("INV_Staff_12"))
        }),
        "and no icon arrives either — same missing repaint, same one cause"
    );
    // A snapshot naming a roll this frame does not hold still leaves it alone and still does not
    // error — the half of this test that survives the migration intact.
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<Option<String>>("return GroupLootFrame1Name:GetText()")
            .unwrap(),
        None
    );
}

/// The bare-name payoff (the header's "THE FOUR INSTANCES KEEP THE REFERENCE'S BARE NAMES"): the
/// ref's `UIPARENT_MANAGED_FRAME_POSITIONS["GroupLootFrame1"]` row actually engages, so the roll
/// dialogs ride the bottom-bar stack instead of sitting on top of the action bars.
///
/// This is the test that FALSIFIES the naming choice: under a `Benilla`-prefixed name
/// `UIParent_ManageFramePositions`'s literal `getglobal(name)` misses, the row no-ops through the
/// ref's own `if ( frame )` guard, and every assertion below collapses to the static XML 60.
/// Mirrors `cast_tests::managed_positions_track_the_bottom_bar_stack` (the same mechanism, the
/// same bar-visibility stubs); the arithmetic is the ref table's own row — baseY 60, bottomEither
/// 42, pet 42.
#[test]
fn managed_positions_engage_for_the_bare_frame_name() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    // UiPanels.xml before GroupLootFrame.xml, mirroring the shipped manifest order
    // (`ui_script::load_default_ui`): the roll file's CONFIRM_LOOT_ROLL entry indexes
    // `StaticPopupDialogs`, and indexing a nil there aborts the WHOLE inline <Script> chunk —
    // taking every BenillaGroupLootFrame_* function down with it, not just the popup.
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    // `TOOLTIP_DEFAULT_COLOR`, which the chain's dropdown backdrops read in their OnLoad — the
    // dropdown kit rides in with the loot window now (1838), so this bespoke setup needs it too.
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_group_loot(&s);

    let bottom = |s: &UiScript| s.eval::<f64>("return GroupLootFrame1:GetBottom()").unwrap();

    // The loader's post-load bootstrap with no bars in existence: the row's bare base, which
    // already differs from nothing here (the XML anchor is also 60) — the real proof is below.
    s.run("UIParent_ManageFramePositions()").unwrap();
    s.resolve();
    assert_eq!(bottom(&s), 60.0, "baseY");

    // The always-on bottom multibars appear.
    // The bar stubs carry a no-op SetPoint: `MultiBarBottomLeft` and `ShapeshiftBarFrame` are
    // themselves rows in UIPARENT_MANAGED_FRAME_POSITIONS, so since those frames wear their
    // reference names the pass positions them as well as reading their visibility.
    // **The bottom-bar flags come from the SAVED GLOBALS, not from the frames.** The stock pass
    // reads `SHOW_MULTI_ACTIONBAR_1`/`_2` (`UIParent.lua:1598-1606`) and never asks the bars
    // whether they are shown — our retired copy asked, which is why this drive used to fake a
    // frame with an `IsShown`. The fake also had no `IsObjectType`, which the pass calls on every
    // row it seats (1988).
    s.run("SHOW_MULTI_ACTIONBAR_1 = 1 SHOW_MULTI_ACTIONBAR_2 = 1 UIParent_ManageFramePositions()")
        .unwrap();
    s.resolve();
    assert_eq!(bottom(&s), 102.0, "60 + bottomEither 42 — the row engaged");

    // The stance bar shows on top of them.
    // The pass's shapeshift-appearance arm (the reference's own, UIParent.lua:1705-1732 — in ours
    // since 1938) touches the bar's three shelf textures by name, unguarded as the reference has
    // it; a stand-in frame needs stand-in textures.
    s.run(
        "local t = { Show = function() end, Hide = function() end } \
         ShapeshiftBarLeft, ShapeshiftBarMiddle, ShapeshiftBarRight = t, t, t",
    )
    .unwrap();
    s.run(
        "ShapeshiftBarFrame = ShapeshiftBarFrame or CreateFrame(\"Frame\", \"ShapeshiftBarFrame\") \
         ShapeshiftBarFrame:Show() UIParent_ManageFramePositions()",
    )
        .unwrap();
    s.resolve();
    assert_eq!(bottom(&s), 144.0, "60 + 42 + pet 42");

    // And it settles back when the stance bar hides.
    s.run("ShapeshiftBarFrame:Hide() UIParent_ManageFramePositions()")
        .unwrap();
    s.resolve();
    assert_eq!(bottom(&s), 102.0, "back to the multibar-only stack");
}

/// The roll popup's item icon (ref `$parentIconFrame` OnClick, LootFrame.xml l.353-361): CTRL
/// previews the rolled item in the dressing room, SHIFT posts its link into an open
/// chat edit box. Both read `GetLootRollItemLink(rollID)`, the binding this arc
/// added — so this pins that getter against the real shipped XML too.
///
/// The control that must not change: the Need/Greed/Pass buttons still vote. The icon button is new
/// click surface on a frame whose whole job is a three-button vote, and a stray vote from the icon
/// (or a swallowed one from the dice) is exactly the regression worth catching.
#[test]
fn ctrl_and_shift_on_the_roll_icon_preview_and_post_its_link() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    load_group_loot(&s);
    load_xml(&s, r"Interface\FrameXML\UIParent.xml"); // UIParent + UIParent.lua, the reference's own (1988)
    load_xml(&s, "Interface\\FrameXML\\DressUpFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UIMenu.xml"); // the kit the chat menus build from
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ChatFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "Interface\\FrameXML\\FloatingChatFrame.xml");
    s.set_loot_rolls(rolls());

    // Roll 8 (BoE Worn Shortsword) claims frame 1 — a non-BoP roll, so the dice below can land a
    // real vote rather than the bind-on-pickup confirm.
    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(8), ScriptValue::Int(60_000)],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    let quads = s.extract();
    let (x, y) = quad_center(&quads, "INV_Sword_04");

    // A plain click on the icon does nothing at all — the reference's handler has no third branch,
    // and the vote is the dice's job, not the icon's.
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    assert!(
        s.take_loot_roll_votes().is_empty() && s.take_loot_roll_confirms().is_empty(),
        "an unmodified icon click votes nothing"
    );

    // SHIFT with the chat edit box open → the link.
    assert!(s.focus_editbox("ChatFrameEditBox"));
    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        SWORD_LINK,
        "the rolled item's full escaped link landed in the chat box"
    );

    // CTRL → the dressing room wearing it.
    s.set_modifiers(false, true, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.take_dressup_intents(),
        vec![DressUpIntent::Dress, DressUpIntent::TryOn(25)],
        "re-dress first, then try the rolled item on"
    );
    assert!(
        s.take_loot_roll_votes().is_empty() && s.take_loot_roll_confirms().is_empty(),
        "no modified icon click may vote"
    );

    // The control: Need on the dice still reaches the wire (roll 8 is not bind-on-pickup).
    let (nx, ny) = quad_center(&quads, "UI-GroupLoot-Dice-Up");
    s.mouse_button(nx, ny, "LeftButton", true);
    s.mouse_button(nx, ny, "LeftButton", false);
    assert_eq!(
        s.take_loot_roll_votes(),
        vec![(8, 1)],
        "the Need button still votes"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The roll-frame template is published under **the reference's own name**, and an addon that
/// inherits it finds the parts it reaches for.
///
/// It shipped as `BenillaGroupLootFrameTemplate`. The `Benilla` prefix exists so a name WE invented
/// cannot collide with one an addon expects — and this template is the opposite of an invention: a
/// line-by-line transcription of 1.12's own `GroupLootFrameTemplate`, which addons inherit *by that
/// name*. `Bongos_RollBar/bar.lua:90` builds its four roll frames with
/// `CreateFrame("Frame", "BRollBarFrame"..i, bar, "GroupLootFrameTemplate")`; that resolves on the
/// real client and missed here for no reason but our prefix.
///
/// The child names are the actual contract, so they are what this asserts. `bar.lua` reaches for
/// `<name>IconFrameIcon`, `<name>Name`, `<name>Corner`, `<name>Decoration` and `<name>Timer` — and
/// `IconFrameIcon` is the one a flat template would get wrong, because it requires the icon texture
/// to be nested inside the `IconFrame` **button**, not hung off the template root.
#[test]
fn the_roll_template_carries_the_reference_name_and_the_parts_addons_reach_for() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    load_group_loot(&s);

    // The reference's name resolves — and, since 1253, a miss would RAISE rather than hand back a
    // bare frame, so this call failing is this test failing.
    s.run(r#"Probe = CreateFrame("Frame", "ProbeRoll", nil, "GroupLootFrameTemplate")"#)
        .expect("the reference-named template must resolve");

    for part in [
        "IconFrame",
        "IconFrameIcon",
        "Name",
        "Corner",
        "Decoration",
        "Timer",
    ] {
        assert!(
            s.eval::<bool>(&format!(r#"return getglobal("ProbeRoll{part}") ~= nil"#))
                .unwrap(),
            "an addon inheriting the template must find ProbeRoll{part}"
        );
    }

    // The timer is a StatusBar, not a plain frame: `bar.lua:33` calls SetMinMaxValues on it.
    s.run(r#"ProbeRollTimer:SetMinMaxValues(0, 60000)"#)
        .expect("the timer must answer StatusBar verbs");

    // And the old private name is gone, so nothing can quietly depend on it again.
    assert!(s
        .eval::<bool>(r#"return getglobal("BenillaGroupLootFrameTemplate") == nil"#)
        .unwrap());
}

/// The **stock** `GroupLootFrame_OnShow` over an in-flight roll — the same defect 1805 fixed one
/// window over, proven at the file that has not been migrated yet.
///
/// `GroupLootFrame1..4`, their template and `GroupLootFrame_OnShow` all live in the stock
/// `LootFrame.xml`/`.lua`, which has been on the player's chain since 1800; our `GroupLootFrame.xml`
/// loads after it and shadows the lot. So the stock body is dormant, not absent — and it does
/// `local color = ITEM_QUALITY_COLORS[quality]` (`LootFrame.lua:275`) then `color.r` (`:276`) off
/// `GetLootRollItemInfo`, with none of the `or ITEM_QUALITY_COLORS[1]` our copy carries.
///
/// This test loads the stock file WITHOUT ours, so the dormant body runs. It is the guard that stops
/// the eventual GroupLootFrame migration from shipping the loot bug a second time.
#[test]
fn the_stock_group_loot_frame_survives_an_in_flight_roll() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");
    s.set_loot_rolls(rolls());

    // Roll 9 is the in-flight one; roll 99 does not exist at all (a stale id from a torn-down
    // frame). Both take the reference's miss tail, so both must paint rather than raise.
    for roll in [9, 99] {
        s.run(&format!("GroupLootFrame_OpenNewFrame({roll}, 55000)"))
            .unwrap_or_else(|e| panic!("stock OpenNewFrame raised on roll {roll}: {e}"));
        assert!(
            s.errors().is_empty(),
            "stock GroupLootFrame_OnShow raised on roll {roll}: {:?}",
            s.errors()
        );
    }
    assert!(s
        .eval::<bool>("return GroupLootFrame1:IsVisible() and GroupLootFrame2:IsVisible()")
        .unwrap());

    // …and the colour it painted is the miss tail's Common, read back off the FontString that
    // `:276` set — not a nil, and not the Epic of the resolved roll sitting beside it.
    let painted: (f64, f64, f64) = s
        .eval("local r, g, b = GroupLootFrame1Name:GetTextColor()\nreturn r, g, b")
        .unwrap();
    let common: (f64, f64, f64) = s
        .eval("local r, g, b = GetItemQualityColor(1)\nreturn r, g, b")
        .unwrap();
    assert!(
        (painted.0 - common.0).abs() < 1e-6
            && (painted.1 - common.1).abs() < 1e-6
            && (painted.2 - common.2).abs() < 1e-6,
        "the stock roll popup paints the miss tail's Common, got {painted:?} want {common:?}"
    );
}
