//! Drives the stock `Interface\FrameXML\MailFrame.xml` through the engine (decision 0544 P1/P2) —
//! the mail twin of `tradeskill_frame.rs`: it loads the same file chain the app does (cut to the
//! mail window's dependency prefix), pushes a synthetic inbox, opens the window with the app's own
//! `MAIL_SHOW`/`MAIL_INBOX_UPDATE` events, and asserts the transcribed Lua actually paints — the
//! named regions exist, the rows populate from a fed `MailState`, the paging math is right, and the
//! unread/read row state tracks the wire `wasRead` flag.

mod common;

use benilla_ui::script::{MailInboxRow, MailInvoice, MailState, UiScript};

/// The mail window's load prefix — the app's own order (`ui_script/mod.rs`), members only.
/// MerchantFrame.xml rides along because MailFrame.xml reuses its global `BenillaMoney_*` coin
/// helpers (postage display), so a load error in either fails here.
const FILES: &[&str] = &[
    "Interface\\FrameXML\\Fonts.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    // The send tab's money entry comes off the chain since 1882 — `MoneyInputFrameTemplate` and
    // the `MoneyInputFrame_*` verbs. Seated straight after MoneyFrame.xml, benilla.toc's order.
    r"Interface\FrameXML\MoneyInputFrame.lua",
    r"Interface\FrameXML\MoneyInputFrame.xml",
    "Interface\\FrameXML\\GlobalStrings.lua",
    r"Interface\FrameXML\UIParent.xml",
    "ScrollTemplates.xml", // our scroll kit + the placeholder icon
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    "Interface\\FrameXML\\BasicControls.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\StaticPopup.xml", // the dialog engine (1960)
    "Interface\\FrameXML\\GameTooltip.xml",
    r"Interface\FrameXML\ItemButtonTemplate.xml", // the send tab's attachment slot inherits it
    // The stock tabs inherit `FriendsFrameTabTemplate`, and `inherits=` resolves at load — so the
    // social window and the kit it needs come first, as the reference's toc has them (1970).
    "Interface\\FrameXML\\UIDropDownMenu.xml",
    "Interface\\FrameXML\\CharacterFrameTemplates.xml",
    "Interface\\FrameXML\\FriendsFrame.xml",
    "Interface\\FrameXML\\MailFrame.xml",
];

fn load_ui(script: &UiScript) {
    for file in FILES {
        common::load_ui(script, file);
    }
}

/// One inbox row with the fields the window paints.
fn row(sender: &str, subject: &str, was_read: bool, item_id: u32, cod: u32) -> MailInboxRow {
    MailInboxRow {
        package_icon: (item_id != 0).then(|| "Interface\\Icons\\INV_Misc_Bag_08".to_string()),
        stationery_icon: Some("Interface\\Icons\\INV_Misc_Note_01".to_string()),
        sender: Some(sender.to_string()),
        subject: subject.to_string(),
        money: 0,
        cod,
        days_left: 29.0,
        item_count: if item_id != 0 { 2 } else { 0 },
        was_read,
        was_returned: false,
        text_created: false,
        can_reply: true,
        is_gm: false,
        body: Some("body".into()),
        stationery_texture: "STATIONERYTEST".into(),
        is_invoice: false,
        invoice: None,
        has_body: true,
        item_id,
        item_name: (item_id != 0).then(|| "Linen Cloth".to_string()),
        item_texture: (item_id != 0).then(|| "Interface\\Icons\\INV_Fabric_Linen_01".to_string()),
        item_quality: (item_id != 0).then_some(1),
        can_delete: false,
        item_random_property_id: 0,
    }
}

/// One Linen Cloth in the backpack — the send tab's attachment fixture. `ClickSendMailItemButton`
/// reads the cursor, and `GetSendMailItem` answers off the bag slot it came from, so the row needs
/// a real name and texture for the stock tab to title the letter with.
fn one_linen_backpack() -> benilla_ui::script::ContainerState {
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        benilla_ui::script::ContainerSlot {
            item_id: 2589,
            count: 20,
            quality: Some(1),
            texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
            link: Some("|cffffffff|Hitem:2589|h[Linen Cloth]|h|r".into()),
            bar_placeable: true,
            ..Default::default()
        },
    );
    benilla_ui::script::ContainerState {
        name: Some("Backpack".into()),
        num_slots: 16,
        slots,
    }
}

/// A one-page inbox (2 mails).
fn small_inbox() -> MailState {
    MailState {
        inbox: vec![
            row("Thrall", "Warchief's orders", false, 2589, 0),
            row("Jaina", "A letter", true, 0, 0),
        ],
    }
}

#[test]
fn mail_frame_loads_and_key_regions_exist() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_ui(&s);
    // The window, both tabs' bodies, a row, the open-letter toplevel, and the send-tab widgets.
    for name in [
        "MailFrame",
        "InboxFrame",
        "MailItem1",
        "MailItem1Button",
        "MailItem7",
        "SendMailFrame",
        "SendMailNameEditBox",
        "SendMailMailButton",
        "OpenMailFrame",
        "OpenMailReplyButton",
    ] {
        assert!(
            s.eval::<bool>(&format!("return getglobal('{name}') ~= nil"))
                .unwrap(),
            "region {name} should exist"
        );
    }
    // The window is hidden until MAIL_SHOW.
    assert!(!s.eval::<bool>("return MailFrame:IsShown()").unwrap());
}

#[test]
fn mail_show_opens_and_inbox_populates() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_mail(Some(small_inbox()));

    s.fire_event("MAIL_SHOW", vec![]);
    assert!(
        s.eval::<bool>("return MailFrame:IsShown()").unwrap(),
        "the window opens on MAIL_SHOW"
    );
    // MAIL_SHOW's CheckInbox() queued the inbox-refresh intent.
    assert!(s.take_mail_check_inbox(), "MAIL_SHOW fires CheckInbox");

    s.fire_event("MAIL_INBOX_UPDATE", vec![]);
    assert_eq!(s.eval::<i64>("return GetInboxNumItems()").unwrap(), 2);
    // Row 1 painted its sender + subject from the fed state.
    assert_eq!(
        s.eval::<String>("return MailItem1Sender:GetText()")
            .unwrap(),
        "Thrall"
    );
    assert_eq!(
        s.eval::<String>("return MailItem1Subject:GetText()")
            .unwrap(),
        "Warchief's orders"
    );
    // The row's clickable child button is shown for a populated row, hidden past the 2 mails (the
    // reference row FRAME stays shown — only $parentButton toggles; ref InboxFrame_Update l.120/180).
    assert!(s.eval::<bool>("return MailItem1Button:IsShown()").unwrap());
    assert!(!s.eval::<bool>("return MailItem3Button:IsShown()").unwrap());

    // No script errors escaped the event-driven repaints.
    assert!(s.take_errors().is_empty(), "clean repaint");
}

#[test]
fn paging_math_enables_next_only_when_overflowing() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    // 9 mails → 2 pages of 7.
    let mut inbox = Vec::new();
    for i in 0..9 {
        inbox.push(row(&format!("S{i}"), &format!("subj{i}"), false, 0, 0));
    }
    s.set_mail(Some(MailState { inbox }));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);

    // Page 1: prev disabled, next enabled.
    assert!(!s
        .eval::<bool>("return InboxPrevPageButton:IsEnabled() ~= 0")
        .unwrap());
    assert!(s
        .eval::<bool>("return InboxNextPageButton:IsEnabled() ~= 0")
        .unwrap());
    // Turn the page: prev enabled, next disabled (only 2 mails on page 2).
    s.run("InboxNextPage()").unwrap();
    assert!(s
        .eval::<bool>("return InboxPrevPageButton:IsEnabled() ~= 0")
        .unwrap());
    assert!(!s
        .eval::<bool>("return InboxNextPageButton:IsEnabled() ~= 0")
        .unwrap());
}

/// The unread/read row *coloring* (SetTextColor / SetVertexColor) has no engine getter to assert
/// against, so the harness verifies the other observable per-row state the same Update paints: the
/// COD tag shows on a COD mail and hides on a plain one, and the plain row's money field is nil.
#[test]
fn cod_tag_shows_on_a_cod_mail() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_mail(Some(MailState {
        inbox: vec![
            row("Auctioneer", "COD parcel", false, 2589, 5000), // COD 50s
            row("Jaina", "A letter", true, 0, 0),               // no COD
        ],
    }));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);

    assert!(
        s.eval::<bool>("return MailItem1ButtonCOD:IsShown()")
            .unwrap(),
        "the COD mail shows its coin tag"
    );
    assert!(
        !s.eval::<bool>("return MailItem2ButtonCOD:IsShown()")
            .unwrap(),
        "the plain mail hides the COD tag"
    );
    assert!(s.take_errors().is_empty());
}

#[test]
fn opening_a_letter_shows_the_open_frame_and_queues_the_body() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_mail(Some(small_inbox()));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);
    let _ = s.take_mail_opens();

    // Click row 1: the check button toggles on → open the letter.
    // A programmatic click toggles the check button on, then fires OnClick (this = the button).
    s.run("MailItem1Button:Click()").unwrap();
    assert!(
        s.eval::<bool>("return OpenMailFrame:IsShown()").unwrap(),
        "the open-letter frame shows"
    );
    // The sender/subject/body painted; GetInboxText queued the open (mark-read + body ask).
    assert_eq!(
        s.eval::<String>("return OpenMailSender:GetText()").unwrap(),
        "Thrall"
    );
    assert!(s.take_mail_opens().contains(&1), "opening queued the row");
    assert!(s.take_errors().is_empty());
}

/// **The letter and the centre seat exclude each other, in BOTH arrival orders** (
/// director-reported and ref-checked): with the mailbox at the left slot and the character sheet
/// pushed to centre beside it (its pushable=2 row), clicking a mail item must EVICT the sheet —
/// the letter's OnShow is the ref's own (`if GetCenterFrame() then HideUIPanel(...)`, ref
/// MailFrame.xml l.1903-1907) — not open underneath it, which is what a bare Show did. The
/// reverse order is 1507's child-window loop: a frame arriving at centre puts the letter away.
/// Whichever comes second wins the space; they can never stack. A bare stand-in carries the
/// CharacterFrame row (the real file isn't in this harness's chain).
#[test]
fn a_letter_and_the_centre_occupant_evict_each_other() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_mail(Some(small_inbox()));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);
    let _ = s.take_mail_opens();

    // The director's setup: mailbox left, character sheet pushed to centre beside it.
    s.run(
        r#"local c = CreateFrame("Frame", "CharacterFrame") c:SetWidth(50); c:SetHeight(50) c:Hide()
           ShowUIPanel(CharacterFrame)"#,
    )
    .unwrap();
    assert!(
        s.eval::<bool>(
            "return GetLeftFrame():GetName() == 'MailFrame' \
             and GetCenterFrame():GetName() == 'CharacterFrame'"
        )
        .unwrap(),
        "mail holds left, the sheet was pushed to centre"
    );

    // Click a mail item: the letter opens AND the sheet is evicted — one thing beside the mailbox.
    s.run("MailItem1Button:Click()").unwrap();
    assert!(s.take_errors().is_empty());
    assert!(
        s.eval::<bool>("return OpenMailFrame:IsShown()").unwrap(),
        "the letter opened"
    );
    assert!(
        !s.eval::<bool>("return CharacterFrame:IsShown()").unwrap(),
        "the centre occupant was evicted, not covered (the reported bug)"
    );
    assert!(
        s.eval::<bool>("return GetCenterFrame() == nil").unwrap(),
        "the eviction is a plain vacate — nothing slides"
    );
    assert!(
        s.eval::<bool>("return MailFrame:IsShown() and GetLeftFrame():GetName() == 'MailFrame'")
            .unwrap(),
        "the mailbox itself is untouched"
    );

    // The reverse arrival: re-opening the sheet over the open letter puts the LETTER away
    // (1507's child-window loop) — the same exclusion, other direction.
    s.run("ShowUIPanel(CharacterFrame)").unwrap();
    assert!(s.take_errors().is_empty());
    assert!(
        s.eval::<bool>("return CharacterFrame:IsShown()").unwrap()
            && !s.eval::<bool>("return OpenMailFrame:IsShown()").unwrap(),
        "a frame arriving at centre hides the letter"
    );
}

#[test]
fn reply_switches_to_send_tab_prefilled() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_mail(Some(small_inbox()));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);
    // A programmatic click toggles the check button on, then fires OnClick (this = the button).
    s.run("MailItem1Button:Click()").unwrap();

    s.run("OpenMail_Reply()").unwrap();
    // The send tab is now shown, the recipient prefilled, the subject "RE: "-prefixed.
    assert!(s.eval::<bool>("return SendMailFrame:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return SendMailNameEditBox:GetText()")
            .unwrap(),
        "Thrall"
    );
    assert_eq!(
        s.eval::<String>("return SendMailSubjectEditBox:GetText()")
            .unwrap(),
        "RE: Warchief's orders"
    );
}

/// Closing an ordinary letter (no money, no item, textCreated=false) must NOT delete it — the
/// reference OnHide rule (MailFrame.lua l.256-272) only purges a fully-taken husk.
#[test]
fn closing_a_plain_letter_does_not_delete_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_mail(Some(MailState {
        inbox: vec![row("One", "test", false, 0, 0)],
    }));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);
    let _ = s.take_mail_opens();
    let _ = s.take_mail_deletes();

    s.run("MailItem1Button:Click()").unwrap();
    assert!(s.eval::<bool>("return OpenMailFrame:IsShown()").unwrap());
    s.run("OpenMailCancelButton:Click()").unwrap();
    assert!(s.take_errors().is_empty(), "no Lua errors on close");
    let deletes = s.take_mail_deletes();
    assert!(
        deletes.is_empty(),
        "closing a plain read letter must NOT delete it, got deletes: {deletes:?}"
    );
}

/// Closing a fully-taken husk (no money, no item, textCreated=TRUE) deletes it — the reference
/// OnHide purge. This is also the live "mail vanished on close" moment against vmangos: the server
/// stamps an EMPTY-BODY player mail MAIL_CHECK_MASK_COPIED (`MailHandler.cpp` l.421,
/// `req->body.empty() ? MAIL_CHECK_MASK_COPIED : MAIL_CHECK_MASK_HAS_BODY`), which IS the wire's
/// textCreated bit — so a subject-only letter with nothing attached auto-purges when closed, in
/// the real 1.12 client exactly as here.
#[test]
fn closing_a_taken_husk_deletes_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    let mut husk = row("One", "test", false, 0, 0);
    husk.text_created = true;
    s.set_mail(Some(MailState { inbox: vec![husk] }));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);
    let _ = s.take_mail_opens();
    let _ = s.take_mail_deletes();

    s.run("MailItem1Button:Click()").unwrap();
    s.run("OpenMailCancelButton:Click()").unwrap();
    assert!(s.take_errors().is_empty(), "no Lua errors on close");
    assert_eq!(
        s.take_mail_deletes(),
        vec![1],
        "the husk purges on close (reference OnHide, MailFrame.lua l.256-272)"
    );
}

/// The expiry text pluralizes like the reference (`GetText("DAYS_ABBR", nil, n)`: "Day"/"Days") —
/// and carries the reference's own trailing space before the colour close (MailFrame.lua l.144).
#[test]
fn expiry_text_pluralizes_days() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    let mut one_day = row("Two", "b", false, 0, 0);
    one_day.days_left = 1.7;
    s.set_mail(Some(MailState {
        inbox: vec![row("One", "a", false, 0, 0), one_day],
    }));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);
    assert_eq!(
        s.eval::<String>("return MailItem1ExpireTime:GetText()")
            .unwrap(),
        "|cff20ff2029 Days |r"
    );
    assert_eq!(
        s.eval::<String>("return MailItem2ExpireTime:GetText()")
            .unwrap(),
        "|cff20ff201 Day |r"
    );
    assert!(s.take_errors().is_empty());
}

/// The letter button — "make a permanent copy" (ref OpenMail_Update l.364-376): a mail whose body
/// is takeable (`item_text_id != 0`) and not yet copied shows it; clicking queues the
/// `TakeInboxTextItem` intent (→ `CMSG_MAIL_CREATE_TEXT_ITEM`). With money enclosed too, both
/// buttons show and the caption reads "Take Attachments:".
#[test]
fn letter_button_shows_for_a_body_letter_and_click_queues_the_copy() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    let mut mail = row("One", "asd", false, 0, 0);
    mail.money = 10000;
    s.set_mail(Some(MailState { inbox: vec![mail] }));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);

    s.run("MailItem1Button:Click()").unwrap();
    assert!(
        s.eval::<bool>("return OpenMailLetterButton:IsShown()")
            .unwrap(),
        "a takeable, not-yet-copied body shows the letter button"
    );
    assert!(s
        .eval::<bool>("return OpenMailMoneyButton:IsShown()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return OpenMailAttachmentText:GetText()")
            .unwrap(),
        "Take Attachments:"
    );
    s.run("OpenMailLetterButton:Click()").unwrap();
    assert_eq!(
        s.take_mail_take_texts(),
        vec![1],
        "the click queues the permanent-copy intent"
    );
    assert!(s.take_errors().is_empty());
}

/// Once the body is copied (`textCreated`, the wire COPIED bit) the letter button hides — the same
/// flag whose OnHide purge then deletes the husk on close.
#[test]
fn letter_button_hides_once_copied() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    let mut mail = row("One", "asd", false, 0, 0);
    mail.money = 10000;
    mail.text_created = true;
    s.set_mail(Some(MailState { inbox: vec![mail] }));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);

    s.run("MailItem1Button:Click()").unwrap();
    assert!(!s
        .eval::<bool>("return OpenMailLetterButton:IsShown()")
        .unwrap());
    assert!(s
        .eval::<bool>("return OpenMailMoneyButton:IsShown()")
        .unwrap());
    assert!(s.take_errors().is_empty());
}

/// Hovering the coins shows the plain money tooltip (ref OpenMailMoneyButton OnEnter l.1823-1829).
#[test]
fn money_button_hover_shows_the_amount_tooltip() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    let mut mail = row("One", "asd", false, 0, 0);
    mail.money = 10000;
    s.set_mail(Some(MailState { inbox: vec![mail] }));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);
    s.run("MailItem1Button:Click()").unwrap();

    // The hover is the button's own inline handler (stock MailFrame.xml), reading `this`.
    s.run("this = OpenMailMoneyButton OpenMailMoneyButton:GetScript(\"OnEnter\")()")
        .unwrap();
    assert!(
        s.eval::<bool>("return GameTooltip:IsShown()").unwrap(),
        "the money tooltip shows on hover"
    );
    assert!(
        s.eval::<bool>("return GameTooltipMoneyFrame:IsShown()")
            .unwrap(),
        "the coin row rendered (SetTooltipMoney path)"
    );
    assert!(s.take_errors().is_empty());
}

/// The inbox page label stays EMPTY: the ref XML declares InboxCurrentPage (l.329) but no
/// reference Lua ever writes it — the real 1.12 window shows nothing there ("Page 1" was our
/// invention, now removed).
#[test]
fn inbox_page_label_stays_empty_like_the_reference() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_mail(Some(small_inbox()));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);
    let text = s
        .eval::<Option<String>>("return InboxCurrentPage:GetText()")
        .unwrap();
    assert!(
        text.as_deref().unwrap_or("").is_empty(),
        "InboxCurrentPage must stay unwritten (got {text:?})"
    );
}

/// A runtime-shown child `<Frame>` renders its own `<Layers>` FontStrings.
///
/// This pins a fact three window files spent months asserting the opposite of. `SendMailFrame`
/// ships `hidden="true"` and is shown by the tab click; its title lives in its OWN Layers, not on
/// the window root. The "flat layout" those windows adopted was a workaround for a constraint that
/// never existed — and it produced no symptom precisely because it was always obeyed, which is why
/// the claim needed a test rather than a header comment.
#[test]
fn a_runtime_shown_pane_renders_its_own_layers() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_ui(&s);
    s.eval::<()>("MailFrame:Show() SendMailFrame:Show()")
        .unwrap();
    assert!(
        s.eval::<bool>("return SendMailFrame:IsVisible()").unwrap(),
        "the pane itself"
    );
    assert!(
        s.eval::<bool>("return SendMailTitleText:IsVisible()")
            .unwrap(),
        "a FontString declared inside a runtime-shown child frame's own <Layers>"
    );
}

/// The auction house's mail is a RECEIPT, not a letter. Before this the window
/// showed exactly what the server wrote — `From: Unknown / Subject: 5529:0:2` over a body of
/// `6C:10000:10000:25:500` — because nothing parsed it. The subject rewrite is the engine's
/// (`ui_mail::invoice`); this is the pane, and it comes in two shapes off one set of seven values.
///
/// (`From: Unknown` is NOT part of the bug and is not fixed here: an auction mail carries no
/// player sender guid, so the reference's own `if ( not sender ) then sender = UNKNOWN` is what
/// puts that word there — MailFrame.lua l.286-288.)
///
/// The body text assertion is the 1527 fold-back: `GetInboxText` returns **nil** for an invoice by
/// an explicit carve-out in the reference, so `SetText(nil)` leaves the page genuinely empty.
#[test]
fn an_auction_invoice_renders_as_a_receipt() {
    let _data = benilla_formats::wow_data_or_skip!();
    /// The pane reads the player's own GlobalStrings, which a bare-XML harness has none of. Stand
    /// in synthetic ones: what is under test is that each reaches the right region, never their text.
    fn strings(s: &UiScript) {
        s.run(concat!(
            "ITEM_SOLD_COLON = 'SOLD:' PURCHASED_BY_COLON = 'BY:' AMOUNT_RECEIVED_COLON = 'GOT:' ",
            "ITEM_PURCHASED_COLON = 'BOUGHT:' SOLD_BY_COLON = 'FROM:' AMOUNT_PAID_COLON = 'PAID:' ",
            "BUYOUT = 'Buyout' HIGH_BIDDER = 'High Bidder'",
        ))
        .unwrap();
    }
    fn open_with(invoice: MailInvoice) -> UiScript {
        let mut s = UiScript::new().unwrap();
        load_ui(&s);
        strings(&s);
        let mut inbox = small_inbox();
        inbox.inbox[0].is_invoice = true;
        inbox.inbox[0].invoice = Some(invoice);
        s.set_mail(Some(inbox));
        s.fire_event("MAIL_SHOW", vec![]);
        s.fire_event("MAIL_INBOX_UPDATE", vec![]);
        s.run("MailItem1Button:Click()").unwrap();
        s
    }
    let text = |s: &UiScript, region: &str| {
        s.eval::<String>(&format!("return tostring({region}:GetText())"))
            .unwrap()
    };
    let money = |s: &UiScript, frame: &str| {
        s.eval::<String>(&format!("return tostring({frame}.staticMoney)"))
            .unwrap()
    };

    // ── The seller's: the full sum. 1g sale + 25c deposit back − 5c the house takes. ──────────
    let mut s = open_with(MailInvoice {
        seller: true,
        item_name: "Linen Cloth".into(),
        player_name: "Twowarrior".into(),
        bid: 10_000,
        buyout: 10_000,
        deposit: 25,
        consignment: 500,
    });
    assert!(
        s.eval::<bool>("return OpenMailInvoiceFrame:IsShown()")
            .unwrap(),
        "a sold-auction mail shows the receipt"
    );
    assert_eq!(text(&s, "OpenMailInvoiceItemLabel"), "SOLD: Linen Cloth");
    assert_eq!(text(&s, "OpenMailInvoicePurchaser"), "BY: Twowarrior");
    // bid == buyout, so it was bought outright rather than won on a bid.
    assert_eq!(text(&s, "OpenMailInvoiceBuyMode"), "(Buyout)");
    assert_eq!(money(&s, "OpenMailSalePriceMoneyFrame"), "10000");
    assert_eq!(money(&s, "OpenMailDepositMoneyFrame"), "25");
    assert_eq!(money(&s, "OpenMailHouseCutMoneyFrame"), "500");
    assert_eq!(
        money(&s, "OpenMailTransactionAmountMoneyFrame"),
        "9525",
        "sale + deposit - cut, which is the whole point of the four lines"
    );
    assert!(s
        .eval::<bool>("return OpenMailInvoiceHouseCut:IsShown()")
        .unwrap());
    assert_eq!(
        text(&s, "OpenMailBodyText"),
        "nil",
        "an invoice has no letter body at all — `GetInboxText` nils it (1527), so the receipt has \
         nothing to sit on top of"
    );
    assert!(s.take_errors().is_empty());

    // ── The buyer's: one line, and the seller-only rows gone. Won on a bid, not bought out. ───
    let mut s = open_with(MailInvoice {
        seller: false,
        item_name: "Small Blue Pouch".into(),
        player_name: "Onewarrior".into(),
        bid: 9_000,
        buyout: 10_000,
        deposit: 0,
        consignment: 0,
    });
    assert!(s
        .eval::<bool>("return OpenMailInvoiceFrame:IsShown()")
        .unwrap());
    assert_eq!(
        text(&s, "OpenMailInvoiceItemLabel"),
        "BOUGHT: Small Blue Pouch  (High Bidder)",
        "the buy mode rides the item line here, not the purchaser line"
    );
    assert_eq!(text(&s, "OpenMailInvoicePurchaser"), "FROM: Onewarrior");
    assert_eq!(
        text(&s, "OpenMailInvoiceBuyMode"),
        "nil",
        "the buy-mode line is blank on a bid win, and a blank FontString reads back nil — \
         `FontString:GetText 0x79d690` substitutes (decision 2110); this helper `tostring`s it"
    );
    assert_eq!(money(&s, "OpenMailTransactionAmountMoneyFrame"), "9000");
    for gone in [
        "OpenMailInvoiceSalePrice",
        "OpenMailInvoiceDeposit",
        "OpenMailInvoiceHouseCut",
        "OpenMailSalePriceMoneyFrame",
        "OpenMailDepositMoneyFrame",
        "OpenMailHouseCutMoneyFrame",
    ] {
        assert!(
            !s.eval::<bool>(&format!("return {gone}:IsShown()")).unwrap(),
            "{gone} is a seller-only line"
        );
    }
    assert!(s.take_errors().is_empty());
}

/// A mail that is NOT an auction invoice keeps its letter: the pane stays hidden and the body text
/// survives. The blanking above is aimed at one kind of mail and must not reach any other.
#[test]
fn a_plain_letter_keeps_its_body_and_shows_no_receipt() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_mail(Some(small_inbox()));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);
    s.run("MailItem1Button:Click()").unwrap();
    assert!(!s
        .eval::<bool>("return OpenMailInvoiceFrame:IsShown()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return OpenMailBodyText:GetText()")
            .unwrap(),
        "body"
    );
    assert!(s.take_errors().is_empty());
}

/// The open letter's ring holds an ordinary **item** icon (`INV_Misc_Note_01`), so it has to go
/// through the PORTRAIT verb and not a bare `SetTexture` — which is exactly what the reference
/// reaches for here, and nowhere else in the file (`MailFrame.lua` l.174).
///
/// Set raw, the icon's square dark border shows through the ring's transparent corners as four
/// little squares around the circle (director's report, 2026-08-22). The INBOX window's ring is the
/// control: it holds `Mail-Icon`, purpose-drawn art that needs no mask, and the reference leaves
/// that one at its `file=` (ref l.258) — so a blanket "mask every ring" would be wrong too.
#[test]
fn the_open_letters_ring_icon_is_masked_but_the_inboxs_is_not() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::QuadContent;

    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_screen_size(1024.0, 768.0);
    s.set_mail(Some(small_inbox()));
    s.fire_event("MAIL_SHOW", vec![]);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);
    s.run("MailItem1Button:Click()").unwrap();
    s.resolve();

    let masked = |needle: &str| -> Vec<bool> {
        s.extract()
            .into_iter()
            .filter_map(|q| match q.content {
                QuadContent::Texture {
                    path: Some(p),
                    circular,
                    ..
                } if p.contains(needle) => Some(circular),
                _ => None,
            })
            .collect()
    };

    let stationery = masked("INV_Misc_Note_01");
    assert!(
        !stationery.is_empty(),
        "the stationery icon should be drawn somewhere"
    );
    assert!(
        stationery.contains(&true),
        "the open letter's ring icon draws masked to its inscribed circle; got {stationery:?}"
    );

    let mail_icon = masked("Mail-Icon");
    assert!(
        !mail_icon.contains(&true),
        "the inbox window's own ring is purpose-drawn art and stays raw, as in the reference; \
         got {mail_icon:?}"
    );
}

/// **"I can't even type anything in the mailbox window"** — the director's report, at its cause.
///
/// The send tab has five edit boxes, and exactly one of them — `SendMailNameEditBox` — carries an
/// XML `<OnChar>` (`SendMailFrame_SendeeAutocomplete`). That handler auto-enables the box for the
/// keyboard walk, and the box is the first-registered keyboard frame in the window, so on every
/// keystroke the walk reached it first and the **base** `CSimpleFrame::OnChar` gate consumed:
/// script present → fire → stop. Nothing downstream of it ever saw a character. The reference
/// cannot do that — `CSimpleEditBox` replaces slot `+0x5c` with `0x77a900`, which asks about focus
/// and declines (`0x77a956`) when another box owns it, and never chains to the base gate at all
/// ([`benilla_ui`]'s `script::keyboard::is_editbox`).
///
/// The one box that DID work is the assertion's control: the autocomplete box types, once per
/// character, and the four that were dead now type too.
#[test]
fn every_send_tab_box_takes_a_keystroke_not_just_the_one_with_an_onchar() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_ui(&s);
    s.fire_event("MAIL_SHOW", vec![]);
    s.run("MailFrameTab_OnClick(2)").unwrap();
    s.resolve();
    assert!(s.eval::<bool>("return SendMailFrame:IsVisible()").unwrap());

    // The box whose handler was eating everyone else's keys: it still types, and its OnChar still
    // fires exactly once per character (the insert path's fire — never the walk's).
    s.run("SendMailNameEditBox:SetText('') SendMailNameEditBox:SetFocus()")
        .unwrap();
    s.run("SendMailChars = 0 \
           SendMailNameEditBox:SetScript('OnChar', function() SendMailChars = SendMailChars + 1 end)")
        .unwrap();
    for c in ["T", "h", "r"] {
        assert!(s.char_input(c), "the focused box consumes");
    }
    assert_eq!(
        s.eval::<String>("return SendMailNameEditBox:GetText()")
            .unwrap(),
        "Thr"
    );
    assert_eq!(
        s.eval::<i64>("return SendMailChars").unwrap(),
        3,
        "OnChar fires once per character, from the insert — not a second time from the walk"
    );

    // …and the four that took nothing at all. The money boxes are `numeric`, so they get digits.
    for (box_name, typed, expect) in [
        ("SendMailSubjectEditBox", ["H", "e", "y"], "Hey"),
        ("SendMailBodyEditBox", ["o", "d", "d"], "odd"),
        ("SendMailMoneyGold", ["1", "2", "3"], "123"),
        ("SendMailMoneySilver", ["4", "5", "5"], "45"), // letters="2"
    ] {
        s.run(&format!("{box_name}:SetText('') {box_name}:SetFocus()"))
            .unwrap();
        for c in typed {
            assert!(s.char_input(c), "{box_name} consumes the keystroke");
        }
        assert_eq!(
            s.eval::<String>(&format!("return {box_name}:GetText()"))
                .unwrap(),
            expect,
            "{box_name} takes what was typed into it"
        );
    }
    assert!(s.take_errors().is_empty(), "and nothing raised on the way");
}

/// **"Sending an item still leaves the subject and the item image in the input"** — the director's
/// second report, at its cause: the compose-tab reset used to fire `MAIL_SEND_SUCCESS` *before* it
/// dropped the attachment, so the stock `SendMailFrame_Reset`'s own tail
/// (`SendMailFrame_Update` → `GetSendMailItem`) painted the just-sent item straight back into the
/// form it had blanked one line earlier. `UiScript::reset_compose_tab` is now `0x4acdc0(1)` whole —
/// zero the globals, THEN tail-fire the three events — so there is no window in which the two
/// disagree.
#[test]
fn the_compose_reset_clears_the_subject_and_the_attachment_together() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_ui(&s);
    s.set_container(0, Some(one_linen_backpack()));
    s.fire_event("MAIL_SHOW", vec![]);
    s.run("MailFrameTab_OnClick(2)").unwrap();

    // Attach the item the way the player does: pick it up, click the send slot. The stock tab
    // names the letter after it, which is the text that used to survive the send.
    s.run("PickupContainerItem(0, 1) ClickSendMailItemButton()")
        .unwrap();
    s.fire_event("MAIL_SEND_INFO_UPDATE", vec![]);
    assert_eq!(
        s.eval::<String>("return SendMailSubjectEditBox:GetText()")
            .unwrap(),
        "Linen Cloth (20)",
        "the stock tab titles the letter after its attachment"
    );

    // The send lands. This is the whole edge: one call, and the form is clean when the FrameXML
    // handler reads it.
    s.reset_compose_tab();
    assert_eq!(
        s.eval::<String>("return SendMailSubjectEditBox:GetText()")
            .unwrap(),
        "",
        "the subject does not come back from the attachment that just left"
    );
    assert!(
        s.eval::<bool>("return GetSendMailItem() == nil").unwrap(),
        "and the attachment is gone with it"
    );
}
