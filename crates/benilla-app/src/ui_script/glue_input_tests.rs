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
    s.key_up(name);
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

/// The login screen's Cinematics button shows `MovieFrame`; no movie plays here, so
/// `StartMovie` answers nil and the stock `MovieFrame_PlayMovie` hides the frame, whose `OnHide`
/// puts the login screen back and the pointer with it (`MovieFrame.lua:18-23,40-44`).
#[test]
fn the_cinematics_button_comes_back_to_the_login_screen() {
    let _ = benilla_formats::wow_data_or_skip!();
    let mut s = glue_on(GlueScreen::Login);
    s.register_cvars([("movieSubtitle", "1"), ("gxResolution", "1024x768")]);
    s.run("AccountLoginCinematicsButton:Click()").unwrap();
    s.resolve();
    let (movie, login, cursor_hidden) = (
        s.eval::<bool>("return MovieFrame:IsShown() == 1").unwrap(),
        s.eval::<bool>("return AccountLoginUI:IsVisible() == 1")
            .unwrap(),
        s.glue_cursor_hidden(),
    );
    assert_eq!(
        (movie, login, cursor_hidden),
        (false, true, false),
        "the movie frame is gone, the login boxes are back and the pointer shows"
    );
    assert_eq!(s.take_errors(), Vec::<String>::new());
}

/// A frame whose only key script is `OnKeyUp` takes the key-down and runs its `OnKeyUp` on the
/// release (`0x76b7d0`, `0x765fd0`, `0x76bba0`): Escape on a playing movie hides the frame
/// (`MovieFrame_OnKeyUp`), which returns to login.
#[test]
fn escape_on_a_playing_movie_returns_to_login_on_the_key_up() {
    let _ = benilla_formats::wow_data_or_skip!();
    let mut s = glue_on(GlueScreen::Login);
    s.register_cvars([("movieSubtitle", "0"), ("gxResolution", "1024x768")]);
    // A movie that starts, as the reference's player would.
    s.run("MovieFrame.StartMovie = function() return 1 end SetGlueScreen('movie')")
        .unwrap();
    s.resolve();
    assert!(s.eval::<bool>("return MovieFrame:IsShown() == 1").unwrap());
    assert!(s.glue_cursor_hidden(), "a playing movie hides the pointer");

    assert!(
        s.frame_key_input("SPACE"),
        "the movie swallows the key-down"
    );
    assert!(
        s.eval::<bool>("return MovieFrame:IsShown() == 1").unwrap(),
        "…and nothing happens until the key is up"
    );
    assert!(s.key_input("ESCAPE"));
    assert!(s.eval::<bool>("return MovieFrame:IsShown() == 1").unwrap());
    s.key_up("ESCAPE");
    s.resolve();
    assert!(!s.eval::<bool>("return MovieFrame:IsShown() == 1").unwrap());
    assert!(s
        .eval::<bool>("return AccountLoginUI:IsVisible() == 1")
        .unwrap());
    assert!(!s.glue_cursor_hidden());
    assert_eq!(s.take_errors(), Vec::<String>::new());
}

/// The Realmlist dialog widens the stock edit box, whose border is two 75-pixel caps that meet at
/// its own 130 pixels: while it is wide the stretched middle fills the gap between the caps, as the
/// chat box's does (`ChatFrame.xml:41-55`), and the stock box comes back untouched.
#[test]
fn the_widened_realmlist_box_has_a_border_between_its_caps() {
    let _ = benilla_formats::wow_data_or_skip!();
    let mut s = glue_on(GlueScreen::Login);
    s.run("BENILLA_REALMLIST = 'logon.example.org' BenillaRealmlistButton:Click()")
        .unwrap();
    s.resolve();
    let span = |s: &UiScript| {
        s.eval::<(f64, f64, f64, f64)>(
            "return GlueDialogEditBox:GetLeft(), GlueDialogEditBox:GetRight(), \
             BenillaRealmlist_Middle:GetLeft(), BenillaRealmlist_Middle:GetRight()",
        )
        .unwrap()
    };
    let (box_left, box_right, mid_left, mid_right) = span(&s);
    benilla_ui::assert_near!(box_right - box_left, 300.0);
    benilla_ui::assert_near!(
        (mid_left - box_left, box_right - mid_right),
        (65.0, 65.0),
        "the middle starts where the left cap ends and ends where the right one starts"
    );
    assert!(s
        .eval::<bool>("return BenillaRealmlist_Middle:IsShown() == 1")
        .unwrap());

    s.run("GlueDialogButton2:Click()").unwrap();
    s.resolve();
    assert!(!s
        .eval::<bool>("return BenillaRealmlist_Middle:IsShown() == 1")
        .unwrap());
    benilla_ui::assert_near!(
        s.eval::<f64>("return GlueDialogEditBox:GetWidth()")
            .unwrap(),
        130.0
    );
    benilla_ui::assert_near!(s.take_errors(), Vec::<String>::new());
}
