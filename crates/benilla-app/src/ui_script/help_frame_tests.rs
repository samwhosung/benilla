//! The GM help window (HelpFrame.xml): the category list the DBC feeds it, the two
//! faces of `UPDATE_TICKET`, the queue gate, the ticket toast, and the three dialogs.
//!
//! Written as the **falsification** pass over the transcription rather than a demonstration of it:
//! every test is named after one claim the window makes, and each was checked to fail when the
//! claim is broken. The load-bearing one is
//! [`clicking_a_category_files_a_ticket_under_that_categorys_dbc_id`] — the id travels from
//! `GMTicketCategory.dbc` through a button, a page, and the editor onto the wire, and a break
//! anywhere in that chain files every ticket under the wrong heading with nothing on screen to
//! show for it.

use benilla_ui::script::{GmTicketIntent, ScriptValue, UiScript};

use super::test_ui::load_ui as load_xml;

/// The window, its dependencies, and the catalog the app pushes — the real ten `GMTicketCategory`
/// rows, so a test that walks the list is walking the shipped data.
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
        // Before UiPanels.xml: the shared StaticPopup carries a `SmallMoneyFrameTemplate` coin
        // row, whose OnLoad calls `SmallMoneyFrame_OnLoad` — the TOC's own order (1580's
        // talent-wipe fixture hit this first).
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        // The stock file's `HelpFrame_OnShow` calls `UpdateMicroButtons()` before
        // `GetGMStatus()`, so without the micro row the OnShow raises and the status ask never
        // happens. Ours never called it. The row needs the bar's button kit under it.
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
        // `TicketStatusFrame_OnEvent` re-anchors TemporaryEnchantFrame before it arms the repoll
        // timer, so without BuffFrame the handler raises and `refreshTime` is never set. This is
        // the one ordering requirement benilla.toc already calls out for this window.
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

/// The `UPDATE_TICKET` argument list the app's feed builds for an open ticket — category first,
/// text second, exactly as `ui_gm_ticket::update_ticket_args` orders it. Kept in sync by being
/// written the same way in both places; if they ever disagree, this file's tests are what notices.
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

/// **`UPDATE_TICKET`'s two faces.** With a ticket the editor becomes an editor (Save Changes /
/// Exit); with the bare `arg1 = 0` it goes back to being a form (Submit / Cancel). The zero leg is
/// the one that would silently rot: it is the ordinary answer, so a window stuck in edit mode
/// looks fine until you try to file a second ticket.
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

    // And now the ordinary answer.
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

/// The toast follows the ticket: up while one is open, gone when it is not. It is the only thing
/// on screen that says a ticket exists at all once the window is closed.
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

/// The toast's own poll is what keeps a long wait honest: `TicketStatus_OnUpdate` re-asks the
/// server every `GMTICKET_CHECK_INTERVAL`, and not before. This is the reason the app counts
/// answers instead of diffing them, so it is worth a test on this side too.
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

/// `ToggleHelpFrame` is the micro button's whole wiring, and opening the window asks the server
/// for the queue status — without that ask the gate above would run on a stale assumption for the
/// life of the session.
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
    // This is the one test that asserts the OnShow traffic itself; the others drain it away first.
    assert!(
        s.eval::<bool>("return HelpFrameHome:IsVisible()").unwrap(),
        "and it opens on Home"
    );

    s.run("ToggleHelpFrame()").unwrap();
    assert!(!s.eval::<bool>("return HelpFrame:IsVisible()").unwrap());
}
