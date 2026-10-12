//! Keys on the stock glue screens, end to end: the host's key and char events into the stock
//! GlueXML in the glue VM.

use benilla_ui::script::{GlueNotice, NoticeState, UiScript};

use super::glue::{load_glue, set_glue_screen, GlueScreen};

/// The stock glue VM on `screen` with both agreements accepted, as a returning player's is.
fn glue_on(screen: GlueScreen) -> UiScript {
    let mut s = UiScript::new_glue().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let accepted = NoticeState {
        accepted: true,
        show_notice: false,
    };
    for notice in [GlueNotice::Eula, GlueNotice::Tos] {
        s.set_glue_notice(notice, accepted);
    }
    let _ = load_glue(&mut s);
    set_glue_screen(&mut s, screen);
    s.resolve();
    let _ = s.take_errors();
    s
}

/// One typed key as the host feeds it (`ui_script::input`): the key-down by name, then its char.
fn type_key(s: &mut UiScript, name: &str, text: &str) {
    s.frame_key_input(name);
    s.char_input(text);
}

fn text(s: &UiScript, frame: &str) -> String {
    s.eval::<String>(&format!("return {frame}:GetText() or ''"))
        .unwrap()
}

/// The login boxes take the keys typed into them. `AccountLogin` is keyboard-enabled with an
/// `OnChar` and an `OnKeyDown`, and walks ahead of any box that is not itself in the walk; the
/// EditBox ctor enables key input (`0x779ced`, `0x779cf8`) so the focused box answers at its own
/// level above its parent.
#[test]
fn the_stock_login_boxes_take_typed_keys() {
    let _ = benilla_formats::wow_data_or_skip!();
    let mut s = glue_on(GlueScreen::Login);
    assert!(
        s.eval::<bool>("return AccountLoginUI:IsShown() == 1")
            .unwrap(),
        "accepted agreements put the login boxes up"
    );
    s.run("AccountLoginAccountEdit:SetFocus()").unwrap();
    for (name, c) in [("P", "p"), ("R", "r"), ("O", "o"), ("B", "b"), ("E", "e")] {
        type_key(&mut s, name, c);
    }
    assert_eq!(text(&s, "AccountLoginAccountEdit"), "probe");

    // Tab moves to the password box (`AccountLoginAccountEdit`'s `OnTabPressed`), which then types.
    assert!(s.key_input("TAB"));
    assert_eq!(
        s.focused_editbox_name().as_deref(),
        Some("AccountLoginPasswordEdit")
    );
    for (name, c) in [("S", "s"), ("E", "e"), ("C", "c")] {
        type_key(&mut s, name, c);
    }
    assert_eq!(text(&s, "AccountLoginPasswordEdit"), "sec");
    assert_eq!(text(&s, "AccountLoginAccountEdit"), "probe");
    assert!(s.take_errors().is_empty());
}
