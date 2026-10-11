//! A Button's state and checked textures have one shown bit each (`+0xc4`), the region's own:
//! `Show`/`Hide` (`0x79b770`/`0x79b830`) and the state switch (`0x7797ea`, `0x785538`) write the
//! same bit, `IsShown` (`0x79b9b0`) reads it and the draw follows it.

use super::common::script;
use crate::script::*;

fn drawn(s: &UiScript) -> Vec<String> {
    s.extract()
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. } => Some(p.clone()),
            _ => None,
        })
        .collect()
}

fn button(s: &mut UiScript, kind: &str) {
    s.set_screen_size(800.0, 600.0);
    s.run(&format!(
        r#"
        b = CreateFrame("{kind}", "ShownBitBtn")
        b:SetPoint("CENTER", 0, 0) b:SetWidth(50) b:SetHeight(50)
        b:SetNormalTexture("n") b:SetPushedTexture("p") b:SetDisabledTexture("d")
        n, p, d = b:GetNormalTexture(), b:GetPushedTexture(), b:GetDisabledTexture()
    "#
    ))
    .unwrap();
}

/// `Show()` on the pushed texture of a NORMAL button writes its bit, which is all the draw reads
/// (`0x76fea5`..`0x77fcb0`); the next state switch, `0x7797c0`, clears it again.
#[test]
fn show_on_a_state_texture_draws_it_until_the_next_state_switch() {
    let mut s = script();
    button(&mut s, "Button");
    s.run(r#"assert(n:IsShown() and not p:IsShown() and not d:IsShown())"#)
        .unwrap();
    s.resolve();
    assert_eq!(drawn(&s), ["n"], "only the current state draws");

    s.run("p:Show() assert(p:IsShown())").unwrap();
    s.resolve();
    let d = drawn(&s);
    assert!(
        d.contains(&"p".to_string()) && d.contains(&"n".to_string()),
        "Show() on the pushed texture draws it beside the normal one: {d:?}"
    );

    s.run(r#"b:SetButtonState("PUSHED")"#).unwrap();
    s.resolve();
    assert_eq!(
        drawn(&s),
        ["p"],
        "the switch hides the shown one, shows the pushed one"
    );
    s.run(r#"b:SetButtonState("NORMAL") assert(not p:IsShown() and n:IsShown())"#)
        .unwrap();
    s.resolve();
    assert_eq!(
        drawn(&s),
        ["n"],
        "the switch back clears the bit Show() wrote"
    );
}

/// `Hide()` on the current state's texture clears the bit it draws by, and the switch away and
/// back writes it again (`0x7797ea`).
#[test]
fn hide_on_the_current_state_texture_lasts_until_its_state_comes_again() {
    let mut s = script();
    button(&mut s, "Button");
    s.run("n:Hide() assert(not n:IsShown())").unwrap();
    s.resolve();
    assert!(drawn(&s).is_empty(), "{:?}", drawn(&s));
    s.run(r#"b:SetButtonState("PUSHED") b:SetButtonState("NORMAL") assert(n:IsShown())"#)
        .unwrap();
    s.resolve();
    assert_eq!(drawn(&s), ["n"]);
}

/// The disabled state's switch (`Disable()` → `0x779790`) shows the disabled texture whatever
/// `Hide()` did to it before.
#[test]
fn disable_shows_the_disabled_texture_a_script_hid() {
    let mut s = script();
    button(&mut s, "Button");
    s.run("d:Show() d:Hide() b:Disable() assert(d:IsShown() and not n:IsShown())")
        .unwrap();
    s.resolve();
    assert_eq!(drawn(&s), ["d"]);
}

/// The checked pair's bits are written by the refresh `0x7854c0` alone: `Show()` on the checked
/// texture of an unchecked button draws it, `SetChecked` clears both and shows the right one
/// (`0x785538`), and a state switch that is not `Enable`/`Disable` leaves them (`0x779790` never
/// calls the refresh).
#[test]
fn the_checked_pair_is_two_bits_the_refresh_writes() {
    let mut s = script();
    button(&mut s, "CheckButton");
    s.run(
        r#"
        b:SetCheckedTexture("c") b:SetDisabledCheckedTexture("dc")
        c, dc = b:GetCheckedTexture(), b:GetDisabledCheckedTexture()
        assert(not c:IsShown() and not dc:IsShown())
        c:Show()
    "#,
    )
    .unwrap();
    s.resolve();
    assert!(drawn(&s).contains(&"c".to_string()), "{:?}", drawn(&s));

    s.run(r#"b:SetChecked(1) assert(c:IsShown() and not dc:IsShown())"#)
        .unwrap();
    s.run(r#"b:SetButtonState("DISABLED") assert(c:IsShown() and not dc:IsShown(), "no refresh")"#)
        .unwrap();
    s.run(r#"b:Enable() assert(c:IsShown() and not dc:IsShown())"#)
        .unwrap();
    s.run(r#"b:Disable() assert(dc:IsShown() and not c:IsShown(), "Disable refreshes")"#)
        .unwrap();
    s.resolve();
    assert!(drawn(&s).contains(&"dc".to_string()), "{:?}", drawn(&s));
    s.run(r#"b:SetChecked(nil) assert(not c:IsShown() and not dc:IsShown())"#)
        .unwrap();
}

/// The highlight is no shown-bit state: it links shown (`0x779139`) and the HIGHLIGHT layer's
/// enable decides, so `Hide()` on it stays hidden through any state switch.
#[test]
fn the_highlight_keeps_its_own_bit_through_state_switches() {
    let mut s = script();
    button(&mut s, "Button");
    s.run(
        r#"
        b:SetHighlightTexture("h") h = b:GetHighlightTexture()
        assert(h:IsShown())
        h:Hide()
        b:SetButtonState("PUSHED") b:SetButtonState("NORMAL") b:Disable() b:Enable()
        assert(not h:IsShown())
        h:Show() assert(h:IsShown())
    "#,
    )
    .unwrap();
}
