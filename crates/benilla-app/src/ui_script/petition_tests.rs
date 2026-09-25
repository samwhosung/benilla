//! The stock guild registrar and charter windows over a Lua stand-in for the engine's petition API,
//! so only the windows are under test. The stand-in keeps the API's shapes: booleans are `1`/`nil`,
//! and `GetPetitionInfo`'s fourth return is the wire's signature requirement.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui_strict as load_xml;

/// The petition API in Lua, seeded as a signer's view with two signatures. Tests mutate
/// `BenillaPetitionFixture`'s fields, never the table, which the closures hold as an upvalue.
const PETITION_FIXTURE: &str = r#"
BenillaPetitionFixture = {
    petitionType = "charter",
    title = "Legacy of Steel",
    bodyText = "",
    -- The WIRE's requirement, deliberately not 9, so a test can tell it apart from
    -- MAX_PETITION_SIGNATURES.
    maxSignatures = 4,
    originator = "Tigole",
    isOriginator = nil,
    canSign = 1,
    signers = { "Furor", "Kaplan" },
    charterCost = 1000,
    calls = {},
}

local F = BenillaPetitionFixture

function BenillaPetitionCalls()
    local out = table.concat(F.calls, "|")
    F.calls = {}
    return out
end

local function record(call)
    table.insert(F.calls, call)
end

function GetPetitionInfo()
    return F.petitionType, F.title, F.bodyText, F.maxSignatures, F.originator, F.isOriginator
end

function GetNumPetitionNames() return table.getn(F.signers) end
function GetPetitionNameInfo(i) return F.signers[i] end
function CanSignPetition() return F.canSign end
function GetGuildCharterCost() return F.charterCost end

function SignPetition() record("SignPetition") end
function OfferPetition() record("OfferPetition") end
function ClosePetition() record("ClosePetition") end
function CloseGuildRegistrar() record("CloseGuildRegistrar") end
function TurnInGuildCharter() record("TurnInGuildCharter") end
function BuyGuildCharter(name) record("BuyGuildCharter:" .. name) end
function RenamePetition(name) record("RenamePetition:" .. name) end
"#;

/// The windows' manifest slice in `benilla.toc` order, the fixture first; `MoneyFrame.xml` comes
/// before the registrar, whose price row inherits `MoneyFrameTemplate` at load.
fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(PETITION_FIXTURE).unwrap();
    // Stock `PetitionFrame_Update` formats `GlobalStrings` entries with no fallback.
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    // `QuestFrame.xml` includes `QuestTitleButtonTemplate`, which the registrar's rows inherit.
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");
    // `ChatFrameEditBox`, which the stock purchase button focuses unguarded
    // (`GuildRegistrarFrame.xml:271`).
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml"); // TOOLTIP_DEFAULT_COLOR, for dropdowns
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml"); // ChatFrame's dropdowns inherit it
    load_xml(&s, "Interface\\FrameXML\\UIMenu.xml"); // the kit the chat menus build from
    load_xml(&s, "Interface\\FrameXML\\ChatFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\FloatingChatFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GuildRegistrarFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\PetitionFrame.xml");
    s
}

fn text(s: &UiScript, expr: &str) -> String {
    s.eval::<String>(&format!("return {expr}:GetText() or \"\""))
        .unwrap_or_else(|e| panic!("{expr}:GetText() — {e}"))
}

fn visible(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!("return {frame}:IsVisible()"))
        .unwrap_or_else(|e| panic!("{frame}:IsVisible() — {e}"))
}

fn calls(s: &UiScript) -> String {
    s.eval::<String>("return BenillaPetitionCalls()").unwrap()
}

fn show_petition(s: &mut UiScript) {
    s.fire_event("PETITION_SHOW", vec![]);
    assert!(s.errors().is_empty(), "PETITION_SHOW: {:?}", s.errors());
}

fn show_registrar(s: &mut UiScript) {
    s.fire_event("GUILD_REGISTRAR_SHOW", vec![]);
    assert!(
        s.errors().is_empty(),
        "GUILD_REGISTRAR_SHOW: {:?}",
        s.errors()
    );
}

/// Rows 1-2 carry the signers and 3-9 read `<not yet signed>`: `GetPetitionNameInfo` is 1-based.
#[test]
fn a_signers_charter_shows_the_sign_face_and_nine_rows() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    show_petition(&mut s);

    assert!(visible(&s, "PetitionFrame"));
    assert_eq!(
        text(&s, "PetitionFrameNpcNameText"),
        "Legacy of Steel Guild Charter",
        "GUILD_CHARTER_TEMPLATE filled with the title"
    );
    assert_eq!(text(&s, "PetitionFrameCharterName"), "Legacy of Steel");
    assert_eq!(text(&s, "PetitionFrameMasterName"), "Tigole");
    // The three static labels come from `text=` attributes; no script paints them.
    assert_eq!(text(&s, "PetitionFrameCharterTitle"), "Guild Name");
    assert_eq!(text(&s, "PetitionFrameMasterTitle"), "Guild Master");
    assert_eq!(text(&s, "PetitionFrameMemberTitle"), "Members");

    assert_eq!(text(&s, "PetitionFrameMemberName1"), "Furor");
    assert_eq!(text(&s, "PetitionFrameMemberName2"), "Kaplan");
    for i in 3..=9 {
        assert_eq!(
            text(&s, &format!("PetitionFrameMemberName{i}")),
            "<not yet signed>",
            "row {i} is an empty seat"
        );
    }

    // Every part draws, not merely holds text.
    for part in [
        "PetitionFrameCharterTitle",
        "PetitionFrameCharterName",
        "PetitionFrameMasterTitle",
        "PetitionFrameMasterName",
        "PetitionFrameMemberTitle",
        "PetitionFrameMemberName1",
        "PetitionFrameMemberName9",
        "PetitionFrameInstructions",
        "PetitionFrameCancelButton",
        "PetitionFrameCloseButton",
    ] {
        assert!(visible(&s, part), "{part} is on screen");
    }
    assert!(visible(&s, "PetitionFrameSignButton"));
    assert!(!visible(&s, "PetitionFrameRequestButton"));
    assert!(
        !visible(&s, "PetitionFrameRenameButton"),
        "only the charter's owner may rename it"
    );
    assert_eq!(
        text(&s, "PetitionFrameInstructions"),
        "Click the <Sign Charter> button to become a charter member of this guild."
    );
}

/// Request and Rename replace Sign; the buttons share one anchor, so both halves are asserted.
#[test]
fn the_owners_charter_swaps_sign_for_request_and_rename() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("BenillaPetitionFixture.isOriginator = 1").unwrap();
    show_petition(&mut s);

    assert!(visible(&s, "PetitionFrameRequestButton"));
    assert!(visible(&s, "PetitionFrameRenameButton"));
    assert!(!visible(&s, "PetitionFrameSignButton"));
    assert_eq!(
        text(&s, "PetitionFrameInstructions"),
        "Select a player you wish to invite and click <request signature>.   To create this \
         guild, turn it in to the guild registrar when you have filled the charter."
    );
}

/// Request disables at the wire's requirement, 4 here, not at the nine rows
/// (`PetitionFrame.lua:38`).
#[test]
fn request_signature_disables_at_the_wires_requirement_not_at_nine() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run(
        r#"
        BenillaPetitionFixture.isOriginator = 1
        BenillaPetitionFixture.signers = { "A", "B", "C" }
    "#,
    )
    .unwrap();
    show_petition(&mut s);
    assert!(
        s.eval::<bool>("return PetitionFrameRequestButton:IsEnabled() ~= 0")
            .unwrap(),
        "three of four signatures — still asking"
    );

    s.run(r#"BenillaPetitionFixture.signers = { "A", "B", "C", "D" }"#)
        .unwrap();
    show_petition(&mut s);
    assert!(
        !s.eval::<bool>("return PetitionFrameRequestButton:IsEnabled() ~= 0")
            .unwrap(),
        "four of four — the charter is full, though only four of nine rows are used"
    );
    assert_eq!(
        text(&s, "PetitionFrameMemberName5"),
        "<not yet signed>",
        "the nine rows are unaffected by the requirement"
    );
}

/// A signer's view can still be unsignable: already signed, already guilded, or full.
#[test]
fn the_sign_button_follows_can_sign_petition() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    show_petition(&mut s);
    assert!(s
        .eval::<bool>("return PetitionFrameSignButton:IsEnabled() ~= 0")
        .unwrap());

    s.run("BenillaPetitionFixture.canSign = nil").unwrap();
    show_petition(&mut s);
    assert!(
        !s.eval::<bool>("return PetitionFrameSignButton:IsEnabled() ~= 0")
            .unwrap(),
        "nil, the era false, disables it"
    );
    assert!(
        visible(&s, "PetitionFrameSignButton"),
        "disabled, not hidden — hiding would expose the Request button beneath it"
    );
}

/// `ClosePetition` rides `OnHide`, so it fires however the window closes
/// (`PetitionFrame.xml:391-394`).
#[test]
fn the_charter_buttons_reach_their_verbs_and_the_close_clears_the_session() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    show_petition(&mut s);
    let _ = calls(&s);

    s.run("PetitionFrameSignButton:Click()").unwrap();
    assert_eq!(calls(&s), "SignPetition");

    s.run("BenillaPetitionFixture.isOriginator = 1").unwrap();
    show_petition(&mut s);
    let _ = calls(&s);
    s.run("PetitionFrameRequestButton:Click()").unwrap();
    assert_eq!(calls(&s), "OfferPetition");

    s.run("HideUIPanel(PetitionFrame)").unwrap();
    assert!(!visible(&s, "PetitionFrame"));
    assert_eq!(calls(&s), "ClosePetition", "OnHide clears the session");
}

/// Stock `RENAME_GUILD` caps the name at 24 (`StaticPopup.lua:138`), the server's limit too.
#[test]
fn rename_guild_sends_the_box_text_and_caps_at_twenty_four() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("BenillaPetitionFixture.isOriginator = 1").unwrap();
    show_petition(&mut s);
    let _ = calls(&s);

    s.run("PetitionFrameRenameButton:Click()").unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert_eq!(
        s.eval::<i64>("return StaticPopupDialogs[\"RENAME_GUILD\"].maxLetters")
            .unwrap(),
        24,
        "the server's MAX_CHARTER_NAME"
    );
    assert!(
        visible(&s, "StaticPopup1"),
        "the popup engine raised the dialog"
    );
    s.run(
        r#"
        StaticPopup1EditBox:SetText("Second Legacy")
        StaticPopup1Button1:Click()
    "#,
    )
    .unwrap();
    assert_eq!(calls(&s), "RenamePetition:Second Legacy");
}

/// Purchase only swaps panels; the buy waits for a typed name.
#[test]
fn the_registrar_opens_on_services_and_purchase_is_a_local_panel_swap() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    show_registrar(&mut s);

    assert!(visible(&s, "GuildRegistrarFrame"));
    assert!(visible(&s, "GuildRegistrarGreetingFrame"));
    assert!(!visible(&s, "GuildRegistrarPurchaseFrame"));
    // Visible, not merely labelled: a hidden button still answers `GetText()`.
    for row in ["GuildRegistrarButton1", "GuildRegistrarButton2"] {
        assert!(visible(&s, row), "{row} is on screen, not just loaded");
    }
    assert_eq!(
        text(&s, "GuildRegistrarButton1"),
        "Purchase a Guild Charter"
    );
    assert_eq!(
        text(&s, "GuildRegistrarButton2"),
        "Register a Guild Charter"
    );
    let _ = calls(&s);

    s.run("GuildRegistrarButton1:Click()").unwrap();
    assert!(visible(&s, "GuildRegistrarPurchaseFrame"));
    assert!(!visible(&s, "GuildRegistrarGreetingFrame"));
    for part in [
        "GuildRegistrarPurchaseText",
        "GuildRegistrarCostLabel",
        "GuildRegistrarMoneyFrame",
        "GuildRegistrarFrameEditBox",
        "GuildRegistrarFramePurchaseButton",
        "GuildRegistrarFrameCancelButton",
    ] {
        assert!(visible(&s, part), "{part} is on screen");
    }
    assert_eq!(calls(&s), "", "the swap sends nothing");
}

/// The stock purchase button closes the window after the buy (`GuildRegistrarFrame.xml:269-270`).
#[test]
fn purchase_sends_the_typed_name_and_register_turns_the_charter_in() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    show_registrar(&mut s);
    s.run("GuildRegistrarButton1:Click()").unwrap();
    let _ = calls(&s);

    s.run(
        r#"
        GuildRegistrarFrameEditBox:SetText("Legacy of Steel")
        GuildRegistrarFramePurchaseButton:Click()
    "#,
    )
    .unwrap();
    assert_eq!(
        calls(&s),
        "BuyGuildCharter:Legacy of Steel|CloseGuildRegistrar",
        "the buy, then the window's own OnHide close"
    );
    assert!(!visible(&s, "GuildRegistrarFrame"));

    show_registrar(&mut s);
    let _ = calls(&s);
    s.run("GuildRegistrarButton2:Click()").unwrap();
    assert_eq!(
        calls(&s),
        "TurnInGuildCharter",
        "no argument — the engine finds the charter in the bags"
    );
}

/// Stock `GuildRegistrar_ShowPurchaseFrame` reads `GetGuildCharterCost()`, in copper, at the swap
/// (`GuildRegistrarFrame.lua:8-11`).
#[test]
fn the_charter_price_is_read_when_the_purchase_panel_opens() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    show_registrar(&mut s);
    s.run("GuildRegistrarButton1:Click()").unwrap();
    // 1000 copper = 10 silver: the gold slot is empty, the silver slot reads 10.
    assert_eq!(text(&s, "GuildRegistrarMoneyFrameSilverButtonText"), "10");
    assert_eq!(text(&s, "GuildRegistrarCostLabel"), "Cost:");
}

/// Stock `GuildRegistrar_OnShow` shows the greeting and hides the purchase panel
/// (`GuildRegistrarFrame.lua:1-3`).
#[test]
fn reopening_the_registrar_returns_to_the_services_list() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    show_registrar(&mut s);
    s.run("GuildRegistrarButton1:Click()").unwrap();
    assert!(visible(&s, "GuildRegistrarPurchaseFrame"));

    s.fire_event("GUILD_REGISTRAR_CLOSED", vec![]);
    assert!(!visible(&s, "GuildRegistrarFrame"));
    show_registrar(&mut s);
    assert!(visible(&s, "GuildRegistrarGreetingFrame"));
    assert!(!visible(&s, "GuildRegistrarPurchaseFrame"));
}

/// Without a `UIPanelWindows` row, `ShowUIPanel` is a bare `Show()` and the windows overlap
/// (`UIParent.lua:33-34`).
#[test]
fn both_charter_windows_are_registered_left_slot_panels() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    for frame in ["GuildRegistrarFrame", "PetitionFrame"] {
        assert_eq!(
            s.eval::<String>(&format!("return UIPanelWindows[\"{frame}\"].area"))
                .unwrap(),
            "left",
            "{frame} must hold a panel row"
        );
    }
    // Both left with `pushable = 0`: one replaces the other.
    show_registrar(&mut s);
    show_petition(&mut s);
    assert!(visible(&s, "PetitionFrame"));
    assert!(
        !visible(&s, "GuildRegistrarFrame"),
        "one left slot, both pushable 0"
    );
}
