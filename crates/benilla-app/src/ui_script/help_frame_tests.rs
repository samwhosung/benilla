//! The stock GM help window (`HelpFrame.xml`): the two faces of `UPDATE_TICKET`, the ticket
//! toast and its repoll, and the status ask on show.

use benilla_ui::script::{GmTicketIntent, ScriptValue, UiScript};

use super::test_ui::load_ui as load_xml;

/// The window and its dependencies, with the ten `GMTicketCategory.dbc` rows the app pushes.
fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "ScrollTemplates.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        // Before StaticPopup.xml, whose coin row inherits `SmallMoneyFrameTemplate`.
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        // `HelpFrame_OnShow` calls `UpdateMicroButtons()` before `GetGMStatus()`
        // (`HelpFrame.lua:178`), so the micro row and the bar's button kit load too.
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
        // `TicketStatusFrame_OnEvent` re-anchors `TemporaryEnchantFrame` before it arms the
        // repoll (`HelpFrame.lua:494`).
        "Interface\\FrameXML\\BuffFrame.xml",
        "Interface\\FrameXML\\HelpFrame.xml",
    ] {
        load_xml(&s, file);
    }
    s.set_gm_ticket_categories(vec![
        (1, "Stuck".into()),
        (2, "Behavior/Harassment".into()),
        (3, "Guild".into()),
        (4, "Item".into()),
        (5, "Environmental".into()),
        (6, "Non-Quest/Creep".into()),
        (7, "Quest/Quest NPC".into()),
        (8, "Technical".into()),
        (9, "Account/Billing".into()),
        (10, "Character".into()),
    ]);
    s
}

/// `UPDATE_TICKET`'s args for an open ticket; must match `ui_gm_ticket::update_ticket_args`.
fn open_ticket_args(
    category: i64,
    text: &str,
    age: f64,
    oldest: f64,
    update: f64,
) -> Vec<ScriptValue> {
    vec![
        ScriptValue::Int(category),
        ScriptValue::Str(text.into()),
        ScriptValue::Number(age),
        ScriptValue::Number(oldest),
        ScriptValue::Number(update),
        ScriptValue::Int(0),
        ScriptValue::Int(0),
    ]
}

/// With a ticket the form is an editor (Save Changes, Exit); a bare `arg1 = 0` makes it a form.
#[test]
fn an_open_ticket_turns_the_form_into_an_editor_and_a_zero_turns_it_back() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ShowUIPanel(HelpFrame) HelpFrame_ShowFrame(\"OpenTicket\")")
        .unwrap();

    s.fire_event(
        "UPDATE_TICKET",
        open_ticket_args(7, "Where is this NPC?", 0.25, 2.5, 0.01),
    );
    assert_eq!(
        s.eval::<String>("return HelpFrameOpenTicketText:GetText()")
            .unwrap(),
        "Where is this NPC?",
        "arg2 is the description"
    );
    assert_eq!(
        s.eval::<i64>("return HelpFrameOpenTicket.ticketType")
            .unwrap(),
        7,
        "arg1 is the category"
    );
    assert_eq!(
        s.eval::<i64>("return HelpFrameOpenTicket.hasTicket")
            .unwrap(),
        1
    );

    // No ticket: a bare 0.
    s.fire_event("UPDATE_TICKET", vec![ScriptValue::Int(0)]);
    assert_eq!(
        s.eval::<String>("return HelpFrameOpenTicketText:GetText()")
            .unwrap(),
        "",
        "the editor empties"
    );
    assert!(
        s.eval::<bool>("return HelpFrameOpenTicket.hasTicket == nil")
            .unwrap(),
        "and stops believing it has a ticket"
    );
}

#[test]
fn the_ticket_toast_follows_the_ticket() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event(
        "UPDATE_TICKET",
        open_ticket_args(1, "Stuck.", 0.1, 0.2, 0.01),
    );
    assert!(
        s.eval::<bool>("return TicketStatusFrame:IsVisible()")
            .unwrap(),
        "a ticket raises the toast"
    );
    s.fire_event("UPDATE_TICKET", vec![ScriptValue::Int(0)]);
    assert!(
        !s.eval::<bool>("return TicketStatusFrame:IsVisible()")
            .unwrap(),
        "and abandoning it takes the toast away"
    );
}

/// `TicketStatus_OnUpdate` re-asks every `GMTICKET_CHECK_INTERVAL`, 600 s (`HelpFrame.lua:162`).
#[test]
fn the_toast_repolls_the_server_only_after_the_full_interval() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event(
        "UPDATE_TICKET",
        open_ticket_args(1, "Stuck.", 0.1, 0.2, 0.01),
    );
    let _ = s.take_gm_ticket_intents();

    s.run("TicketStatus_OnUpdate(599)").unwrap();
    assert!(s.take_gm_ticket_intents().is_empty(), "not yet");
    s.run("TicketStatus_OnUpdate(2)").unwrap();
    assert_eq!(
        s.take_gm_ticket_intents(),
        vec![GmTicketIntent::Ask],
        "600s elapsed — re-ask"
    );
    s.run("TicketStatus_OnUpdate(1)").unwrap();
    assert!(
        s.take_gm_ticket_intents().is_empty(),
        "and the clock restarts"
    );
}

/// `HelpFrame_OnShow` asks the server for the queue status (`HelpFrame.lua:180`).
#[test]
fn toggling_the_window_opens_it_and_asks_for_the_queue_status() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ToggleHelpFrame()").unwrap();
    assert!(s.eval::<bool>("return HelpFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_gm_ticket_intents(),
        vec![GmTicketIntent::AskStatus],
        "OnShow calls GetGMStatus — the gate must not run on an assumption"
    );
    assert!(
        s.eval::<bool>("return HelpFrameHome:IsVisible()").unwrap(),
        "and it opens on Home"
    );

    s.run("ToggleHelpFrame()").unwrap();
    assert!(!s.eval::<bool>("return HelpFrame:IsVisible()").unwrap());
}
