//! The escort-quest confirm (`QuestShareFrame.xml`): the Lua wiring between the
//! `QUEST_ACCEPT_CONFIRM` event `ui_quest_share`'s feed fires and the shared StaticPopup engine,
//! driven exactly as that feed drives it.
//!
//! The asymmetry these tests pin is the reference's: **Yes sends, No does not**. There is no
//! decline packet for an escort confirm, so a test that only checked "the popup closes" would pass
//! against a version that silently answered No on the wire.

use benilla_ui::script::{ScriptValue, UiScript};

use super::test_ui::load_ui as load_xml;

fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    s
}

fn confirm(s: &mut UiScript, who: &str, quest: &str) {
    s.fire_event(
        "QUEST_ACCEPT_CONFIRM",
        vec![
            ScriptValue::Str(who.to_string()),
            ScriptValue::Str(quest.to_string()),
        ],
    );
}

/// The event raises the popup with BOTH args filled, in the reference's order: the player first,
/// the quest title second (`QUEST_ACCEPT = "%s is starting %s\nWould you like to as well?"`). A
/// swap would read "Escort Duty is starting Thrall", which is exactly the kind of wrong that looks
/// right at a glance.
#[test]
fn the_confirm_names_the_player_first_and_the_quest_second() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    confirm(&mut s, "Thrall", "Escort Duty");
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "QUEST_ACCEPT_CONFIRM shows the confirm popup"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Thrall is starting Escort Duty\nWould you like to as well?"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Yes queues exactly one `ConfirmAcceptQuest()` and closes the popup.
#[test]
fn yes_answers_once() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    confirm(&mut s, "Thrall", "Escort Duty");
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_quest_confirms(), 1);
    assert_eq!(s.take_quest_confirms(), 0, "drained");
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "answering closes it"
    );
}

/// **No sends nothing.** The reference's `QUEST_ACCEPT` entry has an `OnAccept` and no `OnCancel`
/// at all (`StaticPopup.lua:727-737`), because there is no decline packet — the server's pending
/// latch is cleared by the next thing that touches it. Same for ESC.
#[test]
fn no_and_escape_send_nothing() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    confirm(&mut s, "Thrall", "Escort Duty");
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert_eq!(s.take_quest_confirms(), 0, "No must not answer the server");
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());

    confirm(&mut s, "Thrall", "Escort Duty");
    s.run("StaticPopup_EscapePressed()").unwrap();
    assert_eq!(s.take_quest_confirms(), 0, "ESC must not answer the server");
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
}

/// A second confirm replaces the first rather than stacking — the server keeps ONE share latch per
/// player, so an older question is already dead by the time a newer one arrives, and answering the
/// stale popup would send the wrong quest id.
#[test]
fn a_second_confirm_replaces_the_first() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    confirm(&mut s, "Thrall", "Escort Duty");
    confirm(&mut s, "Jaina", "Deeper Still");
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Jaina is starting Deeper Still\nWould you like to as well?"
    );
    assert!(
        !s.eval::<bool>("return StaticPopup2:IsVisible()").unwrap(),
        "the confirm is exclusive — it reuses the one slot"
    );
}
