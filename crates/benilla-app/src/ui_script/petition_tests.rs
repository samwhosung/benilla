//! The two guild-charter windows: the guild registrar's two panels, and the
//! charter itself with its two faces.
//!
//! What these guard that the Rust-side unit tests structurally cannot: the windows are Lua over an
//! engine snapshot, so a `GetPetitionInfo` destructured in the wrong order, a name row wired to the
//! wrong index, the leader/signer swap tested the wrong way round, or a button pointed at the wrong
//! verb are all invisible to `script::petition`'s own tests and green in the parse sweep. Each test
//! below fails on exactly one of those.
//!
//! **The engine API is STOOD IN FOR here, deliberately** — `guild_tests`' convention, and for its
//! reasons: what is under test is the window and nothing else, the tests need no app feed seated,
//! and a change to the engine's plumbing cannot quietly turn one green. Two fixture shapes are the
//! ones `script::petition` promises and are easy to get wrong:
//!
//! - **era booleans are `1`/`nil`, never `true`/`false`** — `isOriginator` and `CanSignPetition`;
//! - **`GetPetitionInfo` returns SIX values in a fixed order**
//!   (`petitionType, title, bodyText, maxSignatures, originatorName, isOriginator`), and the
//!   fourth is the wire's *requirement*, not `MAX_PETITION_SIGNATURES`.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui_strict as load_xml;

/// The charter engine API, stood in for in Lua.
///
/// One mutable table, `BenillaPetitionFixture`, is the whole model; every verb appends to `.calls`,
/// which a test drains with `BenillaPetitionCalls()`. Tests mutate the table's FIELDS — never
/// replace the table, since the closures below hold it as an upvalue.
///
/// Seeded as a **signer's** view of a charter with two of nine signatures: the case where every
/// distinction this window makes is live at once (the Sign face showing, rows both filled and
/// empty, Request still enabled).
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

/// The windows' manifest slice, in `benilla.toc` order, with the fixture seated first.
///
/// `MoneyFrame.xml` is load-bearing rather than incidental: `GuildRegistrarMoneyFrame` declares
/// `inherits="MoneyFrameTemplate"`, which resolves at LOAD, so without it the
/// registrar's price row would be a bare frame and the unknown-template guard above would fire.
fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(PETITION_FIXTURE).unwrap();
    // The player's own strings: the reference's `PetitionFrame_Update` formats
    // GUILD_CHARTER_TEMPLATE and reads GUILD_PETITION_*_INSTRUCTIONS and NOT_YET_SIGNED straight
    // out of GlobalStrings, with no fallback of its own — `format(nil, …)` raises, and the window
    // never paints.
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
    // `QuestTitleButtonTemplate`, which the reference's registrar inherits for its two service
    // rows — 1.12 declares it in QuestFrameTemplates.xml, an `<Include>` of QuestFrame.xml, and
    // ours declares it in QuestFrame.xml directly. An unknown template is a loader WARNING, so
    // without this the rows build with no art at all and nothing goes red — which is exactly the
    // failure `load_ui_strict` exists to turn into a red test.
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");
    // `ChatFrameEditBox`, which the reference's own purchase button indexes on every click to
    // decide where focus goes after the name box closes — a nil there raises before the charter is
    // bought. Ours guarded it; the reference does not.
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml"); // TOOLTIP_DEFAULT_COLOR, read by the dropdown backdrops
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml"); // ChatFrame's seven dropdowns inherit its template
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

/// Open the charter window the way the engine does.
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

/// A signer's charter paints the header, the signed rows, the unsigned rows, and the member
/// instructions — and shows Sign, not Request/Rename.
///
/// The row assertion is the load-bearing half: rows 1..2 carry the two signers and rows 3..9 must
/// read `<not yet signed>`. A `GetPetitionNameInfo` bound 0-based would put "Kaplan" in row 1 and
/// blank row 2 while every other assertion here still passed.
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
    // The three static labels come from GlobalStrings via `text=` and are never painted.
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

    // The charter's own static furniture — the three labels, the nine rows and the instructions
    // all draw, not merely hold text.
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

/// The owner's charter is the same window inside out: Request + Rename replace Sign, and the
/// instructions change.
///
/// Both halves are asserted because the two buttons share one anchor — a Show/Hide written the
/// wrong way round stacks them and the top one wins, which looks correct from a screenshot of
/// either view alone.
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

/// Request Signature disables against the **wire's** requirement, never against the nine rows.
///
/// The fixture requires 4. At three signatures the button is live; at four it is not — and nine
/// rows are still painted either way. A window that compared against `MAX_PETITION_SIGNATURES`
/// would leave Request enabled on a full charter, and every other assertion in this file would
/// still pass.
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

/// `CanSignPetition` gates the Sign button independently of which face is showing: a charter can be
/// a signer's view and still be unsignable (already signed, already guilded, full).
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

/// Each button reaches its own verb, and closing the window closes the engine's session.
///
/// The `ClosePetition` half is the one worth pinning: it rides `OnHide`, so it fires however the
/// window closes, and without it the engine keeps a charter session for a window nobody can see.
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

/// The rename dialog is registered, takes the box's text, and is capped at the server's own
/// 24-character charter-name limit.
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

/// The registrar opens on its services list, and Purchase swaps panels **locally** — no verb, no
/// packet. Getting that wrong would send a buy on the first click of "Purchase a Guild Charter",
/// before the player has typed a name.
#[test]
fn the_registrar_opens_on_services_and_purchase_is_a_local_panel_swap() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    show_registrar(&mut s);

    assert!(visible(&s, "GuildRegistrarFrame"));
    assert!(visible(&s, "GuildRegistrarGreetingFrame"));
    assert!(!visible(&s, "GuildRegistrarPurchaseFrame"));
    // **The two service rows must be VISIBLE, not merely labelled.** This assertion is here
    // because its absence shipped the bug: the rows inherited `hidden="true"` from a template
    // modelled on the quest list's POOLED rows, which are shown one at a time by code. These are
    // static and nothing ever calls `:Show()` on them, so both were permanently invisible — the
    // window came up as a bare "Available Services" heading over blank parchment — while every
    // `GetText()` assertion below passed, because a hidden button still knows its own label.
    // Decision 0672's lesson in another key: a frame that loads is not a frame that draws.
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
    // The purchase panel's own furniture, for the reason the services rows above are checked: a
    // window whose parts load but do not draw passes every text assertion.
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

/// Buying sends the box's text and closes the window; registering sends the no-argument turn-in.
///
/// The close-after-buy is the reference's own behaviour and is asserted because it looks like a bug
/// otherwise: a buy is one-shot, and its answer is an item arriving rather than anything this
/// window will be told.
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

/// The price row is filled at panel-swap time from `GetGuildCharterCost()`, in copper.
///
/// Reading it at *show* time instead would paint a 0: the cost belongs to the open registrar's
/// charter list, and the greeting panel never displays one.
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

/// Re-opening the registrar always lands on the services list, never on a half-filled purchase
/// panel left over from last time.
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

/// Both windows are registered UIPanels — without a row, `ShowUIPanel` degrades to a bare `Show()`
/// in no slot and two left-slot windows paint over each other (B288's shape).
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
    // And they are rivals, not neighbours: opening one seats it where the other was.
    show_registrar(&mut s);
    show_petition(&mut s);
    assert!(visible(&s, "PetitionFrame"));
    assert!(
        !visible(&s, "GuildRegistrarFrame"),
        "one left slot, both pushable 0"
    );
}
