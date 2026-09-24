//! The reference's own `Blizzard_MacroUI` addon — the macro editor + its name/icon popup (decision
//! 0983), driven against the engine's own macro table.
//!
//! What these guard, end to end through the real file: the window loads clean in its real
//! neighbourhood; NEW → pick an icon → OKAY actually creates a macro on the right tab; typing in
//! the body and closing the window commits it (the `MacroFrame_SaveMacro` line that makes the
//! whole editor work); the two tabs address the two index ranges; DELETE removes and re-selects;
//! and a macro button is a drag source that loads the cursor with the macro payload.
//!
//! The **saving** half is deliberately exercised through the window rather than the bindings: the
//! bindings' own round trip is `benilla_ui::script::macros`' unit tests, and what can only break
//! here is the wiring — a tab that forgets to save, an OKAY that creates on the wrong tab.

use benilla_ui::script::{CursorPayload, UiScript};

/// The window's real neighbourhood, in the manifest's own order.
/// Click the first macro in the list.
///
/// The reference selects **nothing** when the window opens: `MacroFrame_OnShow` calls only
/// `MacroFrame_Update`, and that function merely *highlights* whichever macro is already selected —
/// it never assigns `MacroFrame.selectedMacro`. Our retired file added a benilla-only
/// `BenillaMacroFrame_EnsureSelection` that picked the first one, and every test here leaned on it
/// without saying so (decision 1848). A player clicks; so do these.
fn select_first(s: &UiScript) {
    s.run("MacroButton1:Click()").unwrap();
}

/// [`harness`] with a chosen player name.
///
/// The character tab's label is built in its own `OnLoad` — `format(CHARACTER_SPECIFIC_MACROS,
/// UnitName("player"))` — so the name has to be in place BEFORE the load, not after it. That is
/// also why the name is how a test chooses the label's width (decision 1848).
fn harness_named(player: &str) -> UiScript {
    harness_with(player)
}

fn harness() -> UiScript {
    harness_with("Probefour")
}

fn harness_with(player: &str) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The global strings the file reads by name (the app runs the real `GlobalStrings.lua`; here
    // only the handful this window formats need to exist).
    s.run(
        r#"
        CREATE_MACROS = "Create Macros"
        GENERAL_MACROS = "General Macros"
        CHARACTER_SPECIFIC_MACROS = "%s Specific Macros"
        ENTER_MACRO_LABEL = "Enter Macro Commands:"
        MACROFRAME_CHAR_LIMIT = "%d/255 Characters Used"
        MACRO_POPUP_TEXT = "Enter Macro Name (Max 16 Characters):"
        MACRO_POPUP_CHOOSE_ICON = "Choose an Icon:"
        CHANGE_MACRO_NAME_ICON = "Change Name/Icon"
        DELETE = "Delete"
        NEW = "New"
        EXIT = "Exit"
        CANCEL = "Cancel"
        OKAY = "Okay"
        MACROS = "Macros"
        -- The tooltip plate colours the body's backdrop reads (the app gets these from
        -- UIParent.lua's own globals; the window only needs them to exist).
        TOOLTIP_DEFAULT_COLOR = { r = 1.0, g = 1.0, b = 1.0 }
        TOOLTIP_DEFAULT_BACKGROUND_COLOR = { r = 0.09, g = 0.09, b = 0.19 }
        "#,
    )
    .unwrap();
    // **A synchronous measurer, because the app has one.** `PanelTemplates_TabResize` sizes a tab
    // from its label's laid-out width at OnLoad, and `measured_wh` answers that inline only when a
    // host font engine is installed — which `ui_script::extract` does (`AtlasMeasurer`). Without
    // one the measure is pending, the width reads 0, and the stock file never re-runs TabResize
    // because the reference's own measure is synchronous. This harness used to model the async
    // round trip and lean on our retired file's OnUpdate re-check (decision 1848).
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));

    // `CHARACTER_SPECIFIC_MACROS` is a `%s` template the character tab formats
    // `UnitName("player")` into, in its OnLoad — so the player exists before the load, not after.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some(player.into()),
            level: 60,
            ..Default::default()
        }),
    );
    // Through the shared loader, because the window is off the chain now and a private disk-only
    // reader cannot name it (decision 1848).
    for file in [
        r"Interface\FrameXML\GlobalStrings.lua",
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\BasicControls.xml", // `TEXT`
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        // The chain's `PanelTemplates_SelectTab` reaches for `GameTooltip` unguarded.
        "Interface\\FrameXML\\GameTooltip.xml",
        // **ScrollTemplates BEFORE UIPanelTemplates, the manifest's own order.** Ours still
        // carries dead `FauxScrollFrame_*` copies the chain overrides by loading after (1846's
        // step 3, deliberately not done); the other way round OUR copies win — the silent
        // drift that record names.
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        // `ClassTrainerListScrollFrameTemplate` — the icon chooser's scroll frame inherits it.
        r"Interface\FrameXML\ClassTrainerFrameTemplates.xml",
        r"Interface\FrameXML\MainMenuBarMicroButtons.xml", // stock `MacroFrame_OnShow`/`_OnHide` drive the micro button
    ] {
        super::test_ui::load_ui(&s, file);
    }
    // The window is a LoadOnDemand addon, reached the way the app reaches it: seated off the
    // chain as a registry row (1957) and loaded by the reference's own `MacroFrame_LoadUI`
    // (UIParent.xml; 1967).
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_MacroUI");
    s.run("MacroFrame_LoadUI()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    // The icon chooser's list is the app's push; three entries is enough to index into.
    s.set_macro_icons(vec![
        "Interface\\Icons\\Ability_Ambush".into(),
        "Interface\\Icons\\Ability_BackStab".into(),
        "Interface\\Icons\\Spell_Fire_FlameBolt".into(),
    ]);
    s
}

/// Assert no script error has been collected, naming the step that produced it.
fn no_errors(s: &UiScript, step: &str) {
    assert!(s.errors().is_empty(), "{step}: {:?}", s.errors());
}

/// The whole creation flow through the window's own buttons: NEW opens the popup, an icon click
/// selects, OKAY enables and creates. This is the path a player takes, and every step of it is
/// wiring this file owns.
#[test]
fn new_then_pick_an_icon_then_okay_creates_the_macro() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);
    no_errors(&s, "show");

    // OKAY starts disabled: no name, no icon.
    s.run("MacroNewButton:Click()").unwrap();
    assert!(s
        .eval::<bool>("return MacroPopupFrame:IsVisible()")
        .unwrap());
    assert!(
        !s.eval::<bool>("return MacroPopupOkayButton:IsEnabled() ~= 0")
            .unwrap(),
        "a nameless, iconless macro cannot be created"
    );

    s.run(r#"MacroPopupEditBox:SetText("Ambush")"#).unwrap();
    s.run("MacroPopupButton1:Click()").unwrap();
    assert!(
        s.eval::<bool>("return MacroPopupOkayButton:IsEnabled() ~= 0")
            .unwrap(),
        "a name and an icon enable OKAY"
    );

    s.run("MacroPopupOkayButton:Click()").unwrap();
    no_errors(&s, "okay");
    assert_eq!(
        s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
            .unwrap(),
        (1, 0),
        "created on the ACCOUNT tab (macroBase 0)"
    );
    let (name, tex) = s
        .eval::<(String, String)>("local n, t = GetMacroInfo(1) return n, t")
        .unwrap();
    assert_eq!(name, "Ambush");
    assert_eq!(tex, "Interface\\Icons\\Ability_Ambush");
    assert!(
        !s.eval::<bool>("return MacroPopupFrame:IsVisible()")
            .unwrap(),
        "OKAY closes the popup"
    );
    // …and the new macro is selected and shown in the detail pane.
    assert_eq!(
        s.eval::<String>("return MacroFrameSelectedMacroName:GetText()")
            .unwrap(),
        "Ambush"
    );
}

/// The body editor's commit path — the single line that makes the window an editor at all
/// (`MacroFrame_SaveMacro`, called from the tab switch, the list click, and the window's OnHide).
/// A regression here silently discards everything the player typed.
#[test]
fn typing_a_body_and_closing_the_window_commits_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run(r#"CreateMacro("Ambush", 1, "")"#).unwrap();
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);
    s.run(r#"MacroFrameText:SetText("/cast Ambush\n/say pew")"#)
        .unwrap();
    // The window's dirty flag comes from the box's `OnTextChanged`, which the drain owes until the
    // next frame (decision 1831).
    s.tick(0.0);
    no_errors(&s, "type");
    assert!(
        s.eval::<bool>("return MacroFrame.textChanged == 1")
            .unwrap(),
        "OnTextChanged marks the window dirty"
    );
    // The character counter is the ref's own MACROFRAME_CHAR_LIMIT fill.
    assert_eq!(
        s.eval::<String>("return MacroFrameCharLimitText:GetText()")
            .unwrap(),
        "21/255 Characters Used"
    );

    s.run(r#"HideUIPanel(MacroFrame)"#).unwrap();
    no_errors(&s, "hide");
    assert_eq!(
        s.eval::<String>("local _, _, b = GetMacroInfo(1) return b")
            .unwrap(),
        "/cast Ambush\n/say pew",
        "closing the window commits the body"
    );
}

/// The two tabs address the two index ranges, and switching tabs saves first. The 19 is the whole
/// point: `MacroFrame.macroBase` is 0 or MAX_MACROS, and every binding takes `macroBase + i`.
#[test]
fn the_character_tab_creates_in_the_second_index_range_and_switching_saves() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run(r#"CreateMacro("Acct", 1, "")"#).unwrap();
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);

    // Type into the account macro, then switch tabs WITHOUT any explicit save.
    s.run(r#"MacroFrameText:SetText("/say account")"#).unwrap();
    s.tick(0.0); // as above — the edit marks, the drain notifies (1831)
    s.run("MacroFrameTab2:Click()").unwrap();
    no_errors(&s, "tab 2");
    assert_eq!(
        s.eval::<String>("local _, _, b = GetMacroInfo(1) return b")
            .unwrap(),
        "/say account",
        "the tab switch saved the body first"
    );
    assert_eq!(s.eval::<i64>("return MacroFrame.macroBase").unwrap(), 18);

    // Creating on this tab lands at 19 (the character range's base + 1).
    s.run("MacroNewButton:Click()").unwrap();
    s.run(r#"MacroPopupEditBox:SetText("Char")"#).unwrap();
    s.run("MacroPopupButton2:Click()").unwrap();
    s.run("MacroPopupOkayButton:Click()").unwrap();
    no_errors(&s, "create on tab 2");
    assert_eq!(
        s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
            .unwrap(),
        (1, 1)
    );
    assert_eq!(
        s.eval::<String>("return GetMacroInfo(19)").unwrap(),
        "Char",
        "the character tab's first slot is index 19"
    );

    // …and back to tab 1 shows the account macro again.
    s.run("MacroFrameTab1:Click()").unwrap();
    assert_eq!(s.eval::<i64>("return MacroFrame.macroBase").unwrap(), 0);
    assert_eq!(
        s.eval::<String>("return MacroFrameSelectedMacroName:GetText()")
            .unwrap(),
        "Acct"
    );
}

/// DELETE removes the macro and leaves the window on a sane selection — the ref re-runs its own
/// OnLoad, which re-selects the first macro (or clears the detail pane when none is left).
#[test]
fn delete_removes_the_macro_and_re_selects() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run(r#"CreateMacro("One", 1, "/say one")"#).unwrap();
    s.run(r#"CreateMacro("Two", 2, "/say two")"#).unwrap();
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);

    s.run("MacroDeleteButton:Click()").unwrap();
    no_errors(&s, "delete");
    assert_eq!(
        s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
            .unwrap(),
        (1, 0)
    );
    // The list closed its gap, so slot 1 is now the survivor and it is what's selected.
    assert_eq!(s.eval::<String>("return GetMacroInfo(1)").unwrap(), "Two");
    assert_eq!(
        s.eval::<String>("return MacroFrameSelectedMacroName:GetText()")
            .unwrap(),
        "Two"
    );

    // Deleting the last one clears the detail pane rather than leaving a stale selection.
    s.run("MacroDeleteButton:Click()").unwrap();
    no_errors(&s, "delete last");
    assert_eq!(
        s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
            .unwrap(),
        (0, 0)
    );
    assert!(
        !s.eval::<bool>("return MacroFrameSelectedMacroButton:IsVisible()")
            .unwrap(),
        "no selection, no detail pane"
    );
}

/// A macro button is a DRAG SOURCE: `OnDragStart` loads the cursor with the macro payload, which
/// `PlaceAction` then packs onto a bar slot under the MACRO tag. This is the only route a macro
/// reaches the action bar.
#[test]
fn dragging_a_macro_button_loads_the_cursor_with_the_macro_payload() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run(r#"CreateMacro("Ambush", 1, "/cast Ambush")"#)
        .unwrap();
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);

    // A REAL drag gesture — press, then move — the way `bag_tests` drives one. Calling
    // `GetScript("OnDragStart")(button)` leaves `this` nil, and the stock handler reads it
    // (decision 1848); our retired file's took the button as an argument.
    s.resolve();
    let (bx, by): (f32, f32) = s.eval("return MacroButton1:GetCenter()").unwrap();
    s.mouse_button(bx, by, "LeftButton", true);
    s.mouse_move(bx + 60.0, by + 60.0);
    no_errors(&s, "drag");
    let payload = s.cursor_payload();
    assert!(
        matches!(&payload, Some(CursorPayload::Macro(m)) if m.index == 1),
        "the macro payload, carrying its index: {payload:?}"
    );
    assert_eq!(
        s.eval::<(String, i64)>("local k, i = GetCursorInfo() return k, i")
            .unwrap(),
        ("macro".to_string(), 1)
    );

    // Placing it on a bar slot packs the MACRO tag (0x40 << 24) with the macro index.
    let mut s = s;
    s.run("PlaceAction(1)").unwrap();
    assert_eq!(s.take_action_sets(), vec![(1, 0x4000_0000 | 1)]);
}

/// The icon chooser's grid: 20 buttons over the app-pushed list, the tail hidden rather than
/// blank, and a click marking the selection.
#[test]
fn the_icon_chooser_shows_the_pushed_list_and_hides_its_tail() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);
    s.run("MacroNewButton:Click()").unwrap();
    no_errors(&s, "popup");

    assert_eq!(s.eval::<i64>("return GetNumMacroIcons()").unwrap(), 3);
    for i in 1..=3 {
        assert!(
            s.eval::<bool>(&format!("return MacroPopupButton{i}:IsVisible()"))
                .unwrap(),
            "button {i} shows an icon"
        );
    }
    assert!(
        !s.eval::<bool>("return MacroPopupButton4:IsVisible()")
            .unwrap(),
        "past the end of the list the button hides — not a blank square"
    );

    s.run("MacroPopupButton3:Click()").unwrap();
    assert_eq!(
        s.eval::<i64>("return MacroPopupFrame.selectedIcon")
            .unwrap(),
        3
    );
    assert!(
        s.eval::<bool>("return MacroPopupButton3:GetChecked()")
            .unwrap(),
        "the picked icon is checked"
    );
}

/// **The two tabs fit inside the 384-wide window** — the director's own report (2026-08-05: "tab is
/// overflowing"), and the check 0983 never had.
///
/// 0983 dropped the reference's `PanelTemplates_TabResize(-15, …)` tuning as redundant with the
/// template's own +0 fit. It isn't: at +0 a tab is `text + 40`, and two of those plus the 65-unit
/// left inset run off a 384-wide plate — the more so on tab 2, whose label carries a player name and
/// so has no width anyone can know when the window is written. The reference's answer is the −15
/// padding on both and a 150 cap on tab 2; this pins both arms, including the cap actually engaging.
///
/// **Driven through a SYNCHRONOUS measurer, and that is a correction** (decision 1848). This used
/// to feed widths through the async round trip and lean on our retired file's OnUpdate re-check.
/// The stock window has no such re-check: its tabs call `PanelTemplates_TabResize` once, in their
/// own OnLoad, and the reference's measure is inline — so a client whose measure is pending at that
/// moment can never size its tabs at all. The app installs `AtlasMeasurer`, so this harness
/// installs one too; modelling the async path here was modelling a configuration the app does not
/// have.
///
/// The label widths therefore come from the text rather than being chosen, so the two arms are a
/// SHORT character name and a LONG one, and the cap is asserted against the label the measurer
/// actually produced.
#[test]
fn the_two_tabs_fit_inside_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    /// `sideWidths` — 2 × the tab's 16-unit end slice (TabButtonTemplate, UiPanels.xml).
    const SIDES: f64 = 32.0;
    /// The reference's own padding for this window's tabs (Blizzard_MacroUI.xml l.488/511).
    const PAD: f64 = -15.0;
    /// …and its cap on tab 2 (l.511).
    const CAP: f64 = 150.0;

    // A short name and a long one. `CHARACTER_SPECIFIC_MACROS` formats the name into tab 2's
    // label, so the name is how the label's width is chosen.
    for (name, cap_should_bind) in [("Probe", false), ("Bartholomewthelongnamed", true)] {
        let mut s = harness_named(name);
        s.run("ShowMacroFrame()").unwrap();
        select_first(&s);
        s.resolve();

        let (w1, w2, left1, label1, label2): (f64, f64, f64, f64, f64) = s
            .eval(
                "return MacroFrameTab1:GetWidth(), MacroFrameTab2:GetWidth(), \
                 MacroFrameTab1:GetLeft() - MacroFrame:GetLeft(), \
                 MacroFrameTab1Text:GetStringWidth(), MacroFrameTab2Text:GetStringWidth()",
            )
            .unwrap();

        assert!(label1 > 0.0 && label2 > 0.0, "both labels measured");
        assert_eq!(
            w1,
            label1 + PAD + SIDES,
            "tab 1 is text − 15 + the end slices"
        );
        let uncapped = label2 + PAD + SIDES;
        if cap_should_bind {
            assert_eq!(
                w2,
                CAP + PAD + SIDES,
                "the reference's 150 cap binds on a long name"
            );
            assert!(
                uncapped > CAP + PAD + SIDES,
                "…and the name really was over it"
            );
        } else {
            assert_eq!(w2, uncapped, "a short name is under the cap");
        }
        assert!(
            w2 <= CAP + PAD + SIDES,
            "the clamp may tighten the reference's cap, never loosen it"
        );

        // The property the clamp exists for, checked outright: the tab ends inside the plate the
        // player can actually see, not merely inside the 384-unit frame rect.
        let tab2_right: f64 = s
            .eval("return MacroFrameTab2:GetLeft() + MacroFrameTab2:GetWidth()")
            .unwrap();
        assert!(
            tab2_right <= 349.0,
            "tab 2 ends at {tab2_right}, past the drawn plate's last opaque column (349)"
        );
        assert!(
            left1 + w1 + w2 <= 384.0,
            "the tab row ({left1} + {w1} + {w2}) runs off the 384-wide window"
        );
        no_errors(&s, "tab fit");
    }
}

/// **The icon chooser's scroll bar sits ON the popup's plate**, not off its right edge over the
/// world. 0983 sized the faux scroll frame to the icon grid (252×152) instead of transcribing the
/// reference's rect, which pushed the bar — anchored 6 units right of the frame — to popup x
/// 282..298, i.e. straddling the 297-wide plate's edge.
///
/// The frame is invisible, so its rect has exactly one observable consequence and this is it.
#[test]
fn the_icon_choosers_scroll_bar_sits_on_the_popup_plate() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run("ShowMacroFrame() MacroNewButton:Click()").unwrap();
    s.resolve();
    let (bar_left, bar_right, plate_left, plate_right): (f64, f64, f64, f64) = s
        .eval(
            "local b = MacroPopupScrollFrameScrollBar local p = MacroPopupFrame \
             return b:GetLeft(), b:GetRight(), p:GetLeft(), p:GetRight()",
        )
        .unwrap();
    assert!(
        bar_left >= plate_left && bar_right <= plate_right,
        "scroll bar {bar_left}..{bar_right} is outside the popup plate {plate_left}..{plate_right}"
    );
    // The reference's own seat: 6 units right of a scroll frame whose right edge is 39 in from the
    // popup's — so 264..280 of 297. Pinned as a number so a future re-anchor has to mean it.
    assert_eq!(
        (bar_left - plate_left, bar_right - plate_left),
        (264.0, 280.0)
    );
    no_errors(&s, "popup scroll bar");
}

// `pump_measures` stood here: it answered the measure round-trip every frame so a fit that
// OSCILLATED would show. Nothing oscillates now and nothing can — this harness installs a
// synchronous measurer, because the app does (`AtlasMeasurer`), and the stock tabs size themselves
// once in their own OnLoad with no re-check to converge. Decision 1848.

/// **The tab row is stable and inside the window, frame after frame** — the director's repro
/// (2026-08-05: a 9-character character name, "tab is still slightly overlapping"), and the check
/// the first fix did not have.
///
/// `the_two_tabs_fit_inside_the_window` feeds one measure and ticks once, which cannot catch this:
/// the defect is a FEEDBACK LOOP. The fit sets a width on the label; that changed the label's box;
/// and `GetStringWidth` used to report the box rather than the string, so the next frame refit off
/// a smaller number, uncapped, un-set the box, re-measured wider, capped again — a tab that changed
/// width every single frame and could be photographed at any point in the cycle.
#[test]
fn the_tab_row_settles_and_stays_inside_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    // The name is chosen at construction, because the stock tab builds its label in its own
    // OnLoad; and there is no measure round trip to pump any more (decision 1848). The loop stays:
    // "it does not change every frame" is still the property, it is just satisfied on the first
    // pass now rather than after a convergence.
    let mut s = harness_named("Onehunter");
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);

    let mut widths = Vec::new();
    for _ in 0..12 {
        s.tick(0.016);
        s.resolve();
        widths.push(
            s.eval::<(f64, f64, f64)>(
                "return MacroFrameTab1:GetWidth(), MacroFrameTab2:GetWidth(), \
                 MacroFrameTab2:GetRight() - MacroFrame:GetLeft()",
            )
            .unwrap(),
        );
    }

    let last = *widths.last().unwrap();
    let settled = &widths[widths.len() - 4..];
    assert!(
        settled.iter().all(|w| *w == last),
        "the tab fit never settles — it changes every frame: {widths:?}"
    );
    // 344, not 384: this window's art stops 40 units short of its frame rect, and the frame
    // rect is not what the player sees. It holds off the reference's own numbers — the −15
    // padding and the 150 cap the two tabs pass `TabResize` — with no clamp of ours in the way;
    // 1002's was retired with our tab template (1993) and never reached these tabs anyway, which
    // are `TabButtonTemplate`'s.
    assert!(
        last.2 <= 344.0,
        "tab 2 ends at {} of a window whose plate stops at 344: {widths:?}",
        last.2
    );
    no_errors(&s, "tab settle");
}

/// **No character name can push the tab row off the window** — checked against a 64-character
/// one, because `CHARACTER_SPECIFIC_MACROS` formats the player's name into tab 2's label and no
/// window author can know its width.
///
/// What holds it is the reference's own `150` cap, passed to `TabResize` by tab 2's own OnLoad.
/// 1002 added a structural clamp on top of that — a tab may never grow past its window's drawn
/// right edge — and 1993 retired it with our tab template; it never reached these tabs anyway
/// (they are `TabButtonTemplate`'s, and the clamp rode our `CharacterFrameTabButtonTemplate`).
/// So this is the reference's guarantee, asserted at the drawn edge.
#[test]
fn no_character_name_can_push_the_tab_row_off_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    // The `capped = false` arm is gone with decision 1848: it stripped a benilla-only max-width
    // field, and the reference has no such switch — its cap is the literal `150` passed
    // to `PanelTemplates_TabResize` in the tab's OnLoad, which nothing can turn off. So the
    // guarantee is now just the guarantee, checked against four names including a 64-character one.
    for name in ["Ai", "Onehunter", "Bartholomewthethird", &"W".repeat(64)] {
        let mut s = harness_named(name);
        s.run("ShowMacroFrame()").unwrap();
        select_first(&s);
        s.tick(0.016);
        s.resolve();
        let (right, w2): (f64, f64) = s
            .eval(
                "return MacroFrameTab2:GetRight() - MacroFrame:GetLeft(), \
                 MacroFrameTab2:GetWidth()",
            )
            .unwrap();
        assert!(
            right <= 344.0,
            "{name:?}: the tab row ends at {right}, past the drawn plate's edge at 344 \
             (tab 2 is {w2} wide)"
        );
        no_errors(&s, name);
    }
}

/// **The hover highlight IS the tab, at every width and from the first frame** — the director's
/// report on the landed 1002 build: in one screenshot the glow stood proud of tab 1 and fell well
/// short of tab 2.
///
/// Both halves were the same defect, and it is 1002's own read-back trap in the one line 1002 did
/// not fix: the old kit sized the highlight from the label and then capped it at `tab:GetWidth()`,
/// which serves the last RESOLVED rect — inside a settle, the template's pre-fit 115. So every tab
/// in the client wore a 129-unit highlight regardless of its own width, and the settle's change-gate
/// latched it there. The fix is structural (the highlight anchors to the tab's two side edges), so
/// this test asserts the property that makes the whole class of bug impossible: **no frame, no
/// label, no clamp can put the glow at a width other than its tab's.**
#[test]
fn the_tab_highlight_is_exactly_its_tab() {
    let _data = benilla_formats::wow_data_or_skip!();
    // A name under the reference's cap, one over it, and a 40-character one — all three must
    // hold the same property.
    for name in ["Ai", "Onehunter", &"W".repeat(40)] {
        // The name is chosen at construction (the stock tab labels itself in its own OnLoad) and
        // there is no measure round trip to pump — decision 1848. The frame loop stays: the
        // highlight tracking its tab on EVERY frame is the property, not just on the first.
        let mut s = harness_named(name);
        s.run("ShowMacroFrame()").unwrap();
        select_first(&s);

        for frame in 0..12 {
            s.tick(0.016);
            s.resolve();
            for tab in ["MacroFrameTab1", "MacroFrameTab2"] {
                let (tl, tr, hl, hr): (f64, f64, f64, f64) = s
                    .eval(&format!(
                        "return {tab}:GetLeft(), {tab}:GetRight(), \
                         {tab}HighlightTexture:GetLeft(), {tab}HighlightTexture:GetRight()"
                    ))
                    .unwrap();
                // **The two tabs carry DIFFERENT highlight widths, and that is the reference's,
                // not a loader bug.** Exactly one `<OnLoad>` runs — the most-derived one; the
                // template's is installed and then *destroyed*, because `SetScript` releases the
                // slot's single ref before it even looks at the new body (`0x7025ec`). So a tab
                // gets whichever formula ITS OWN
                // body ends with:
                //
                //   * tab 2's body is just `PanelTemplates_TabResize(-15, nil, nil, 150)`, whose
                //     own last statement sets the highlight to `tabWidth` — so highlight == width;
                //   * tab 1's body RE-STATES `TabButtonTemplate`'s second line verbatim
                //     (`HighlightTexture:SetWidth(this:GetTextWidth() + 31)`) after calling
                //     TabResize — so its highlight is independent of its width, and 14 wider.
                //
                // That re-statement idiom is only necessary if the template's handler does not
                // run, and the reference uses it deliberately: `FriendsFrame.xml:610`/`:899` copy
                // both template lines while `:626`/`:881` declare none — a controlled pair in one
                // file. Our retired transcription made the two tabs agree; the migration reverts
                // that (decision 1848).
                let label: f64 = s
                    .eval(&format!("return {tab}Text:GetStringWidth()"))
                    .unwrap();
                let want = if tab == "MacroFrameTab1" {
                    label + 31.0
                } else {
                    tr - tl
                };
                assert_eq!(
                    hr - hl,
                    want,
                    "{tab} f{frame} ({name}): highlight {} wide, label {label}, tab {}",
                    hr - hl,
                    tr - tl
                );
                // …and seated where the reference seats it: `TabButtonTemplate` anchors the
                // highlight `BOTTOM` with a `(2, -8)` offset, so it is **CENTRED** on the tab and
                // nudged 2 right — not left-aligned at `tl + 2`, which is what this asserted while
                // the template was ours (decision 1848). With the overhang above, centring is what
                // keeps the extra 14 units split evenly rather than all trailing off one end.
                assert_eq!(
                    (hl + hr) / 2.0,
                    (tl + tr) / 2.0 + 2.0,
                    "{tab} f{frame} ({name}): highlight not centred on the tab"
                );
            }
        }
        no_errors(&s, "tab highlight");
    }
}

/// Clicking "Change Name/Icon" opens the icon picker.
///
/// **This harness cannot catch the defect that made it raise in-game**, and that is worth saying
/// out loud: a harness lists its own dependencies, so it had
/// `ClassTrainerFrameTemplates.xml` while the shipped manifest did not. The window worked here and
/// was broken in the client. What catches THAT is
/// `every_template_the_manifest_inherits_is_declared_by_the_manifest`, plus the end-to-end drive in
/// `shipped_xml_tests`. Decision 1862.
///
/// The raise it reproduced:
///
/// ```text
/// Blizzard_MacroUI.lua:233: attempt to perform arithmetic on local 'macroPopupOffset' (a nil value)
///   MacroPopupFrame_Update  <-  MacroPopupFrame_OnShow  <-  MacroEditButton_OnClick
/// ```
#[test]
fn the_icon_picker_opens_without_raising() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness();
    select_first(&s);
    let _ = s.errors();
    s.run("MacroEditButton:Click()").unwrap();
    assert!(
        s.errors().is_empty(),
        "opening the icon picker must not raise: {:?}",
        s.errors()
    );
    assert!(
        s.eval::<bool>("return MacroPopupFrame:IsShown() and true or false")
            .unwrap(),
        "the picker is up"
    );
}
