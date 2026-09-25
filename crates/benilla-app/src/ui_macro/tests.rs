//! The app half of the macro system: persistence under `benilla-config/macros/`, the
//! runner's route into the chat drain, and the seed/dirty contract the plugin's systems rely on.
//!
//! The file FORMAT has its own tests in [`super::store`] (including the director's real 1.12
//! `macros-cache.txt`); the API's own round trip is `benilla_ui::script::macros`'; the window's is
//! `crate::ui_script::macro_tests`. What is only testable here is the wiring.

use benilla_ui::script::{MacroState, MacroView, UiScript};

use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

fn macro_view(name: &str, body: &str) -> MacroView {
    MacroView {
        name: name.into(),
        texture: Some("Interface\\Icons\\Ability_Ambush".into()),
        body: body.into(),
        local_only: false,
    }
}

/// A save writes the reference's own format under `benilla-config/macros/`, and a load brings the same
/// macros back — the whole persistence loop over the real `local_state` law.
#[test]
fn a_saved_macro_table_round_trips_through_benilla_macros() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = std::env::temp_dir().join(format!("benilla-macros-{}", std::process::id()));
    std::fs::remove_dir_all(&tmp).ok();
    let _c = EnvGuard::unset("WOW_CAPTURE");
    let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());

    let account = crate::local_state::macros_account_path().unwrap();
    let character = crate::local_state::macros_character_path("Test Realm", "Probeone").unwrap();

    let state = MacroState {
        account: vec![macro_view("Ambush", "/cast Ambush\n/say pew")],
        character: vec![macro_view("Charge", "/cast Charge")],
    };
    crate::local_state::write_atomic(&account, &super::store::write(&state.account)).unwrap();
    crate::local_state::write_atomic(&character, &super::store::write(&state.character)).unwrap();

    // The file on disk is the reference's own shape — readable, hand-editable, and the exact
    // format a vanilla `macros-cache.txt` already has.
    assert_eq!(
        std::fs::read_to_string(&account).unwrap(),
        "MACRO 1 \"Ambush\" Ability_Ambush\n/cast Ambush\n/say pew\nEND\n"
    );

    let back = MacroState {
        account: super::store::parse(&std::fs::read_to_string(&account).unwrap()),
        character: super::store::parse(&std::fs::read_to_string(&character).unwrap()),
    };
    assert_eq!(back, state);
    std::fs::remove_dir_all(&tmp).ok();
}

/// A capture run is hermetic: both paths resolve to `None`, so a macro edit during
/// a capture is session-only and nothing is written under anyone's install.
#[test]
fn a_capture_run_persists_nothing() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _h = EnvGuard::set("BENILLA_HOME", "/tmp/benilla-should-not-exist");
    let _c = EnvGuard::set("WOW_CAPTURE", "ui-macro");
    assert_eq!(crate::local_state::macros_account_path(), None);
    assert_eq!(crate::local_state::macros_character_path("R", "C"), None);
}

/// The real UI, so the WHOLE route is under test: `run_macro` fires `EXECUTE_CHAT_LINE`, the
/// shipped ChatFrame1 is registered for it, and its handler calls `SubmitChatInput`. A bare VM
/// would pass this by accident under the old direct-push shape and silently prove nothing under
/// this one.
fn ui() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = crate::ui_script::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    s
}

/// The runner delivers every body line through the reference's own door — one
/// `EXECUTE_CHAT_LINE` event per non-empty line, in order (`0x4f14e0`) — and the
/// reference's own `ChatFrame_OnEvent` arm does the rest: `SetText(arg1)`,
/// `ChatEdit_SendText`, `ChatEdit_OnEscapePressed` (ChatFrame.lua l.1343-1347). So a macro line
/// lands wherever a typed line lands — a chat type in the send queue, an emote in the emote
/// queue, a roll in the roll queue — without the runner knowing any of them.
#[test]
fn running_a_macro_runs_its_lines_through_the_references_edit_box() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui();
    s.set_macros(MacroState {
        account: vec![macro_view("Greet", "/wave\n\n  /say pew  \n/roll 1 100")],
        character: Vec::new(),
    });

    assert!(super::run_macro(&mut s, 1));
    let sends = s.take_chat_sends();
    assert_eq!(
        sends
            .iter()
            .map(|c| (c.text.as_str(), c.chat_type.as_str()))
            .collect::<Vec<_>>(),
        vec![("pew", "SAY")],
        "the line was trimmed before the parse, and sent as the type its slash named"
    );
    assert_eq!(
        s.take_emote_requests()
            .iter()
            .map(|e| e.token.as_str())
            .collect::<Vec<_>>(),
        vec!["WAVE"],
        "the emote line reached DoEmote through the reference's emote table"
    );
    assert_eq!(s.take_roll_requests(), vec![(1, 100)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // An empty macro and an empty slot both run nothing and queue nothing.
    s.set_macros(MacroState {
        account: vec![macro_view("Blank", "   \n\n")],
        character: Vec::new(),
    });
    assert!(!super::run_macro(&mut s, 1));
    assert!(!super::run_macro(&mut s, 7));
    assert!(s.take_chat_sends().is_empty());
}

/// A CHARACTER-range macro runs by its own index — the second half of the space is not a special
/// case anywhere in the runner.
#[test]
fn a_character_macro_runs_by_its_own_index() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui();
    s.set_macros(MacroState {
        account: Vec::new(),
        character: vec![macro_view("Charge", "/say Charge")],
    });
    assert!(super::run_macro(&mut s, 19));
    assert_eq!(
        s.take_chat_sends()
            .iter()
            .map(|c| c.text.clone())
            .collect::<Vec<_>>(),
        vec!["Charge".to_string()]
    );
}

/// The seed→dirty→save contract the plugin's two systems rest on: the app's own load must not look
/// like a change (or every login would rewrite the file), and every script mutation must.
#[test]
fn the_dirty_edge_distinguishes_a_load_from_an_edit() {
    let mut s = UiScript::new().unwrap();
    s.set_macros(MacroState {
        account: vec![macro_view("Ambush", "/cast Ambush")],
        character: Vec::new(),
    });
    assert!(
        !s.take_macros_dirty(),
        "loading from disk is not an edit — a save here would be a write-back loop"
    );

    s.run(r#"EditMacro(1, nil, nil, "/cast Backstab")"#)
        .unwrap();
    assert!(s.take_macros_dirty());
    assert_eq!(s.macros().account[0].body, "/cast Backstab");
}

/// The generation counter is the per-frame consumers' gate (the action bar's identity feed): it
/// moves on a seed AND on every mutation, and is never consumed by reading it.
#[test]
fn the_generation_moves_on_every_write_and_is_not_drained() {
    let mut s = UiScript::new().unwrap();
    let at_start = s.macros_generation();
    assert_eq!(s.macros_generation(), at_start, "reading never drains it");

    s.set_macros(MacroState {
        account: vec![macro_view("Ambush", "/cast Ambush")],
        character: Vec::new(),
    });
    let after_seed = s.macros_generation();
    assert_ne!(after_seed, at_start, "a seed changes the bar's icons too");

    s.run(r#"EditMacro(1, "Renamed", 1)"#).unwrap();
    assert_ne!(s.macros_generation(), after_seed);
}

/// **An addon that registers `EXECUTE_CHAT_LINE` sees macro lines** — the behaviour benilla gained
/// by firing the reference's event instead of calling its own drain. In 1.12 this is not a
/// courtesy: the event is the entire mechanism, and ChatFrame1's registration is just the default
/// UI's use of it.
#[test]
fn a_registered_frame_sees_every_macro_line_as_an_event() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui();
    s.run(
        r#"MacroSpy = CreateFrame("Frame")
           MacroSpy.seen = {}
           MacroSpy:RegisterEvent("EXECUTE_CHAT_LINE")
           MacroSpy:SetScript("OnEvent", function()
               table.insert(MacroSpy.seen, arg1)
           end)"#,
    )
    .unwrap();
    s.set_macros(MacroState {
        account: vec![macro_view("Pull", "/wave\n/say Incoming!")],
        character: Vec::new(),
    });

    assert!(super::run_macro(&mut s, 1));
    assert_eq!(
        s.eval::<(String, String, i64)>(
            "return MacroSpy.seen[1], MacroSpy.seen[2], table.getn(MacroSpy.seen)"
        )
        .unwrap(),
        ("/wave".into(), "/say Incoming!".into(), 2),
        "each line arrives as arg1 of its own event, in body order"
    );
    // …and the same lines still reach the chat grammar: the spy is an observer, not a diversion.
    assert_eq!(
        s.take_chat_sends()
            .iter()
            .map(|c| c.text.clone())
            .collect::<Vec<_>>(),
        vec!["Incoming!".to_string()]
    );
    assert_eq!(s.take_emote_requests().len(), 1);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The reference's tokenizer takes `"\r\n"` as a delimiter SET — either character splits a line
/// (`0x64ae50`). A body carrying lone `\r`s (an old-Mac hand edit, or a file round-tripped through
/// one) is therefore three lines, not one long one.
#[test]
fn either_line_ending_splits_a_body() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui();
    s.set_macros(MacroState {
        account: vec![macro_view("Mixed", "/say Charge\r/say a\r\n/say b")],
        character: Vec::new(),
    });
    assert!(super::run_macro(&mut s, 1));
    assert_eq!(
        s.take_chat_sends()
            .iter()
            .map(|c| c.text.clone())
            .collect::<Vec<_>>(),
        vec!["Charge".to_string(), "a".to_string(), "b".to_string()],
        "\r alone splits; \r\n does not leave an empty token"
    );
}

/// **Every icon the chooser offers RESOLVES to real art, and the catalog is the archive's** — the
/// tripwire for B221, where four pages of the picker each showed a solid WHITE cell.
///
/// This is `ui_script::shipped_xml_tests`' resolve sweep for the paths that sweep structurally
/// cannot see. That one walks static `file=` attributes in our own XML; a macro icon never appears
/// in XML — it arrives at runtime as `SetTexture(GetMacroIconInfo(i))`. Nothing checked that those
/// resolve, and an unresolvable path drew as an opaque white rectangle, so the picker shipped with
/// white squares in it and every gate green.
///
/// The catalog is now the archive enumeration the reference itself does
/// ([`benilla_formats::load_macro_icons`]), not a `SpellIcon.dbc` scan, so a name with no file
/// behind it can no longer enter the list at all — what this guards is the other half: that the
/// **resolution rule** still finds every enumerated name. `Ability_Druid_Mangle.tga` is the entry
/// that matters (the chooser stores names extension-stripped, and `…Mangle.tga.blp` ships): it only
/// resolves via the reference's second `.blp` candidate, and the old rule had no second candidate.
///
/// Resolution goes through the renderer's own [`benilla_assets::sprite_candidates`], never a copy of
/// it: a sweep re-implementing the rule could agree with itself while disagreeing with what draws.
///
/// Needs client data; skips without it, like the XML sweep.
#[test]
fn every_macro_chooser_icon_resolves_in_the_client_archives() {
    /// Icons on a stock 5875 install: `patch.MPQ` 77 + `interface.MPQ` 443 = 520 raw names under
    /// `Interface\Icons\` matching `Spell_`/`Ability_`, less 3 that differ only by case or
    /// extension — independently counted off the binary's own enumeration
    /// (`BuildMacroIconList 0x4f0090`). The DBC scan this replaced served 521, a
    /// different set: it included five names with no file and missed art the archive has.
    const CHOOSER_ICONS_5875: usize = 517;

    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let icons = benilla_formats::load_macro_icons(&mut chain).expect("load chooser catalog");
    assert_eq!(
        icons.len(),
        CHOOSER_ICONS_5875,
        "chooser catalog size moved — the enumeration or its filter changed"
    );

    let missing: Vec<String> = icons
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            !benilla_assets::sprite_candidates(p)
                .iter()
                .any(|c| chain.contains(c))
        })
        .map(|(i, p)| format!("#{} {p}", i + 1))
        .collect();
    assert!(
        missing.is_empty(),
        "chooser icons that resolve to nothing (each draws as a white square): {missing:#?}"
    );

    // Sorted, not archive order: the reference `qsort`s case-insensitively before deduping, so the
    // order the player scrolls is alphabetical.
    let mut sorted = icons.clone();
    sorted.sort_by_key(|p| p.to_ascii_lowercase());
    assert_eq!(
        icons, sorted,
        "the chooser list must be case-insensitively sorted"
    );
}
