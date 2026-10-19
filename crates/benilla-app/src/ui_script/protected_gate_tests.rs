//! The stock UI through the protected-action gate `0x494a50`. The stock FrameXML is context 0, so
//! tags 2, 4 and 10 pass it wherever it runs; these tests drive the real files (the action bar,
//! the key bindings, the chat edit box, the spellbook, the Escape chain) through the production
//! load and assert each still casts, and that the same VM refuses a tainted call, so a pass here
//! is the gate in force and not a gate that was never asked.

use benilla_ui::script::{
    ActionSlot, ScriptCall, SpellBookState, SpellSlotView, SpellTabView, UiScript,
};

use crate::local_state::test_env::ENV_LOCK;

/// Fireball, the one spell the book holds.
const FIREBALL: u32 = 133;

/// The production in-game UI over a book with Fireball, an action bar with it on slot 1, a cast in
/// flight and a targeting cursor up, after `PLAYER_ENTERING_WORLD`.
fn stock_ui(tag: &str) -> UiScript {
    let (mut s, failures) = super::layer_tests::production_load_with(tag, false, "", |_| {});
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    s.set_spellbook(SpellBookState {
        tabs: vec![SpellTabView {
            name: "Fire".into(),
            texture: None,
            offset: 0,
            num_spells: 1,
        }],
        slots: vec![SpellSlotView {
            spell_id: FIREBALL,
            name: "Fireball".into(),
            rank: Some("Rank 1".into()),
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            ..Default::default()
        }],
    });
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            kind: 0x00,
            action: FIREBALL,
            count: 0,
            consumable: false,
            use_spell: None,
        }),
    );
    s.set_cast_view(benilla_ui::script::CastView {
        slot: Some(FIREBALL),
        ..Default::default()
    });
    s.set_spell_targeting(true);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.tick(0.1);
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// The centre of a laid-out frame, for a click through the hit test.
fn center(s: &UiScript, name: &str) -> (f32, f32) {
    let edge = |f: &str| s.eval::<f32>(&format!("return {name}:Get{f}()")).unwrap();
    (
        (edge("Left") + edge("Right")) / 2.0,
        (edge("Top") + edge("Bottom")) / 2.0,
    )
}

fn click(s: &mut UiScript, name: &str) {
    s.resolve();
    let (x, y) = center(s, name);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
}

/// Every call the stock path queued, as `(call, may_cast)`.
fn calls(s: &mut UiScript) -> Vec<(String, bool)> {
    s.take_script_calls()
        .into_iter()
        .map(|c| (format!("{c:?}"), c.may_cast()))
        .collect()
}

/// A stock action button's click: `ActionButton_OnClick` (`ActionButton.lua`) calls `UseAction`
/// from the button's `OnClick`, an input event of the client's own code, and its press carries the
/// gate's answer, passed.
#[test]
fn a_stock_action_button_click_casts() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = stock_ui("gate-click");
    click(&mut s, "ActionButton1");
    let uses = s.take_action_uses();
    assert_eq!(
        uses.iter()
            .map(|u| (u.action, u.may_cast))
            .collect::<Vec<_>>(),
        [(1, true)],
        "the click queued UseAction(1), cast allowed"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    // The same press from a macro's `/script` line outside an input event is refused in this VM.
    s.run(r#"RunScript("UseAction(1)")"#).unwrap();
    assert_eq!(
        s.take_action_uses()
            .iter()
            .map(|u| u.may_cast)
            .collect::<Vec<_>>(),
        [false]
    );
}

/// A key binding: the stock `ACTIONBUTTON1` body (`Bindings.xml`) runs `ActionButtonDown(1)` on the
/// press and `ActionButtonUp(1)` on the release, which calls `UseAction`. A binding body is the
/// client's own code, and an input event as well.
#[test]
fn a_stock_key_binding_casts() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = stock_ui("gate-binding");
    assert!(s.execute_binding("ACTIONBUTTON1", true).unwrap());
    assert!(s.execute_binding("ACTIONBUTTON1", false).unwrap());
    assert_eq!(
        s.take_action_uses()
            .iter()
            .map(|u| (u.action, u.may_cast))
            .collect::<Vec<_>>(),
        [(1, true)]
    );

    // The stock Escape body (`TOGGLEGAMEMENU`) runs the tag-2 verbs in order: it stops a cast, else
    // the targeting cursor.
    assert!(s.execute_binding("TOGGLEGAMEMENU", true).unwrap());
    assert_eq!(s.take_script_calls(), [ScriptCall::SpellStopCasting]);
    s.set_cast_view(Default::default());
    assert!(s.execute_binding("TOGGLEGAMEMENU", true).unwrap());
    assert_eq!(s.take_script_calls(), [ScriptCall::SpellStopTargeting]);
    // The stock TAB body targets through the tag-4 verb.
    assert!(s.execute_binding("TARGETNEARESTENEMY", true).unwrap());
    assert!(
        matches!(
            s.take_script_calls().as_slice(),
            [ScriptCall::TargetNearest { reverse: false, .. }]
        ),
        "TAB picked"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A typed `/cast`: the chat edit box's `OnEnterPressed` (`ChatFrame.lua`) parses the line and runs
/// `SlashCmdList["CAST"]`, which calls `CastSpellByName`. The cast is the client's own, queued
/// whole; `/script` runs the line as a macro (`RunScript`), whose cast passes only because the
/// enter script is an input event.
#[test]
fn a_typed_cast_casts() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = stock_ui("gate-typed");
    for line in [
        "/cast Fireball",
        r#"/script CastSpellByName("Fireball")"#,
        // The tag-4 and tag-10 verbs, typed.
        "/script TargetNearestEnemy()",
    ] {
        assert!(s.focus_editbox("ChatFrameEditBox"));
        s.run(&format!("ChatFrameEditBox:SetText({line:?})"))
            .unwrap();
        assert!(s.key_input("ENTER"), "{line}");
        let queued = calls(&mut s);
        assert!(
            !queued.is_empty() && queued.iter().all(|(_, may)| *may),
            "{line}: {queued:?}"
        );
        assert!(
            queued
                .iter()
                .any(|(c, _)| c.contains("CastSpell") || c.contains("TargetNearest")),
            "{line}: {queued:?}"
        );
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    // A `/script` line run outside the enter script (a timer, an event) casts nothing.
    s.run(r#"RunScript('CastSpellByName("Fireball")')"#)
        .unwrap();
    assert!(
        matches!(calls(&mut s).as_slice(), [(_, false)]),
        "queued with the gate's no, for the book's cancels; the cast is refused"
    );
}

/// The stock spellbook: `SpellButton_OnClick` calls `CastSpell(id, book)` from the button's
/// `OnClick`.
#[test]
fn a_stock_spellbook_click_casts() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = stock_ui("gate-spellbook");
    s.run("ToggleSpellBook(BOOKTYPE_SPELL)").unwrap();
    s.resolve();
    click(&mut s, "SpellButton1");
    assert_eq!(s.take_spell_casts(), vec![(FIREBALL, false)]);
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    // The same call from a macro outside an input event is refused here.
    s.run(r#"RunScript("CastSpell(1, BOOKTYPE_SPELL)")"#)
        .unwrap();
    assert!(matches!(calls(&mut s).as_slice(), [(_, false)]));
}

/// A stock macro button whose body is a `/script` line: the click is the input event, and the
/// macro's line (a `RunScript` chunk, whose cast the gate lets through only inside an input event)
/// runs after the click has returned, where the reference runs it inside. The app replays the
/// events the press was made under for the line ([`UiScript::with_input_events`]); a line run with
/// none is refused here, so this is the replay in force and not a gate that never asked.
#[test]
fn a_stock_macro_buttons_script_line_casts() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = stock_ui("gate-macro");
    let index: u32 = s
        .eval(r#"return CreateMacro("fb", 1, '/script CastSpellByName("Fireball")', 1, nil)"#)
        .unwrap();
    s.set_action(
        2,
        Some(ActionSlot {
            texture: None,
            kind: 0x40,
            action: index,
            count: 0,
            consumable: false,
            use_spell: None,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.tick(0.1);
    // The press is applied as the app applies it, in call order, which raises the events the click
    // ran under around the macro's lines; the same lines run with none are refused.
    click(&mut s, "ActionButton2");
    let mut ran = false;
    let mut casts = Vec::new();
    crate::script_calls::in_call_order(&mut s, |s, call| match call {
        ScriptCall::UseAction(press) => {
            assert_eq!(press.action, 2);
            ran = crate::ui_macro::run_macro(s, index);
        }
        other => casts.push(other),
    });
    assert!(ran, "the macro has a line");
    assert!(
        matches!(casts.as_slice(), [c @ ScriptCall::CastSpell { .. }] if c.may_cast()),
        "the line's cast passed the gate inside the click's events: {casts:?}"
    );

    click(&mut s, "ActionButton2");
    s.take_script_calls();
    assert!(crate::ui_macro::run_macro(&mut s, index));
    assert!(
        matches!(s.take_script_calls().as_slice(), [c @ ScriptCall::CastSpell { .. }] if !c.may_cast()),
        "with no input event up the cast is refused"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
