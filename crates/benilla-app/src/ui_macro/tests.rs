//! The macro wiring: persistence under `benilla-config/macros/`, the runner's route through the
//! stock chat frame, and the seed and dirty contract the plugin's systems rely on.

use benilla_ui::script::{MacroState, MacroView, UiScript};

use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

fn macro_view(name: &str, body: &str) -> MacroView {
    MacroView {
        name: name.into(),
        texture: Some("Interface\\Icons\\Ability_Ambush".into()),
        body: body.into(),
        ..Default::default()
    }
}

/// A save writes the reference's format under `benilla-config/macros/`, and a load reads it back.
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

    let state = MacroState::from_scopes(
        vec![macro_view("Ambush", "/cast Ambush\n/say pew")],
        vec![macro_view("Charge", "/cast Charge")],
    );
    crate::local_state::write_atomic(
        &account,
        &super::store::write(state.saved(false).into_iter()),
    )
    .unwrap();
    crate::local_state::write_atomic(
        &character,
        &super::store::write(state.saved(true).into_iter()),
    )
    .unwrap();

    // The reference's format, with `\n` line ends.
    assert_eq!(
        std::fs::read_to_string(&account).unwrap(),
        "MACRO 1 \"Ambush\" Ability_Ambush\n/cast Ambush\n/say pew\nEND\n"
    );
    // A character macro keeps its full id, the scope's base and one (`0x4eff90`).
    assert_eq!(
        std::fs::read_to_string(&character).unwrap(),
        "MACRO 16777217 \"Charge\" Ability_Ambush\n/cast Charge\nEND\n"
    );

    let back = MacroState::from_scopes(
        super::store::parse(&std::fs::read_to_string(&account).unwrap()),
        super::store::parse(&std::fs::read_to_string(&character).unwrap()),
    );
    assert_eq!(back, state);
    std::fs::remove_dir_all(&tmp).ok();
}

/// A capture resolves both paths to `None`, so its macros are session-only.
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

/// The real UI, so the whole route is under test: `run_macro` fires `EXECUTE_CHAT_LINE`, and the
/// stock `ChatFrame1` sends the line through its edit box.
fn ui() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, so a player exists: the stock macro window's OnLoad
    // formats `UnitName("player")` into its character tab.
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

/// One `EXECUTE_CHAT_LINE` per non-empty line, in order (`0x4f14e0`), which `ChatFrame_OnEvent`
/// sends through the edit box (`ChatFrame.lua:1343-1347`): a macro line lands where a typed one
/// does.
#[test]
fn running_a_macro_runs_its_lines_through_the_references_edit_box() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui();
    s.set_macros(MacroState::from_scopes(
        vec![macro_view("Greet", "/wave\n\n  /say pew  \n/roll 1 100")],
        Vec::new(),
    ));

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
    s.set_macros(MacroState::from_scopes(
        vec![macro_view("Blank", "   \n\n")],
        Vec::new(),
    ));
    assert!(!super::run_macro(&mut s, 1));
    assert!(!super::run_macro(&mut s, 7));
    assert!(s.take_chat_sends().is_empty());
}

/// A macro slot holds the id, and the first per-character macro's is `0x1000001` (`0x4eff90`):
/// its position, 19, names nothing to the runner (`0x4f1460` looks the record up by id).
#[test]
fn a_character_macro_runs_by_its_own_id() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui();
    s.set_macros(MacroState::from_scopes(
        Vec::new(),
        vec![macro_view("Charge", "/say Charge")],
    ));
    assert!(!super::run_macro(&mut s, 19));
    assert!(super::run_macro(&mut s, 0x100_0001));
    assert_eq!(
        s.take_chat_sends()
            .iter()
            .map(|c| c.text.clone())
            .collect::<Vec<_>>(),
        vec!["Charge".to_string()]
    );
}

/// The app's own load is not an edit, or every login would rewrite the file; a script mutation is.
#[test]
fn the_dirty_edge_distinguishes_a_load_from_an_edit() {
    let mut s = UiScript::new().unwrap();
    s.set_macros(MacroState::from_scopes(
        vec![macro_view("Ambush", "/cast Ambush")],
        Vec::new(),
    ));
    assert!(
        !s.take_macros_dirty(),
        "loading from disk is not an edit — a save here would be a write-back loop"
    );

    s.run(r#"EditMacro(1, nil, nil, "/cast Backstab")"#)
        .unwrap();
    assert!(s.take_macros_dirty());
    assert_eq!(s.macros().by_id(1).unwrap().body, "/cast Backstab");
}

#[test]
fn the_generation_moves_on_every_write_and_is_not_drained() {
    let mut s = UiScript::new().unwrap();
    let at_start = s.macros_generation();
    assert_eq!(s.macros_generation(), at_start, "reading never drains it");

    s.set_macros(MacroState::from_scopes(
        vec![macro_view("Ambush", "/cast Ambush")],
        Vec::new(),
    ));
    let after_seed = s.macros_generation();
    assert_ne!(after_seed, at_start, "a seed changes the bar's icons too");

    s.run(r#"EditMacro(1, "Renamed", 1)"#).unwrap();
    assert_ne!(s.macros_generation(), after_seed);
}

/// In 1.12 the event is the whole mechanism; `ChatFrame1`'s registration is only the default UI's
/// use of it.
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
    s.set_macros(MacroState::from_scopes(
        vec![macro_view("Pull", "/wave\n/say Incoming!")],
        Vec::new(),
    ));

    assert!(super::run_macro(&mut s, 1));
    assert_eq!(
        s.eval::<(String, String, i64)>(
            "return MacroSpy.seen[1], MacroSpy.seen[2], table.getn(MacroSpy.seen)"
        )
        .unwrap(),
        ("/wave".into(), "/say Incoming!".into(), 2),
        "each line arrives as arg1 of its own event, in body order"
    );
    // The same lines still reach the chat frame.
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

/// The reference's tokenizer (`0x64ae50`) splits on either `\r` or `\n`.
#[test]
fn either_line_ending_splits_a_body() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui();
    s.set_macros(MacroState::from_scopes(
        vec![macro_view("Mixed", "/say Charge\r/say a\r\n/say b")],
        Vec::new(),
    ));
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

/// Every chooser icon resolves through the renderer's own [`benilla_assets::sprite_candidates`]:
/// one that does not draws as a white square. `Ability_Druid_Mangle.tga` resolves only through the
/// second `.blp` candidate, since `Ability_Druid_Mangle.tga.blp` is what ships.
#[test]
fn every_macro_chooser_icon_resolves_in_the_client_archives() {
    /// A stock 5875 install: `patch.MPQ` 77 + `interface.MPQ` 443 = 520 `Spell_`/`Ability_` names
    /// under `Interface\Icons\`, less 3 that differ only by case or extension
    /// (`BuildMacroIconList`, `0x4f0090`).
    const CHOOSER_ICONS_5875: usize = 517;

    let data = benilla_formats::wow_data_or_skip!();
    let chain = benilla_formats::open_chain(&data).expect("open chain");
    // The archives alone: a loose `Interface\Icons\` file is not in them to resolve.
    let icons = benilla_formats::load_macro_icons(Some(&chain), None);
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

    // Alphabetical: the reference `qsort`s without case before deduping.
    let mut sorted = icons.clone();
    sorted.sort_by_key(|p| p.to_ascii_lowercase());
    assert_eq!(
        icons, sorted,
        "the chooser list must be case-insensitively sorted"
    );
}

/// The world-entry VM's lister runs on the first `GetNumMacroIcons` (`0x4f19f0`), never at the
/// VM's creation, and `GetMacroIconInfo` before it answers "" (`0x4f1ac5`).
#[test]
fn the_entry_vms_icon_chooser_is_listed_on_first_use() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = benilla_formats::open_chain(&data).expect("open chain");
    let mut s = UiScript::new().expect("VM");
    s.set_macro_icon_source(super::icon_source(
        Some(std::sync::Arc::new(std::sync::Mutex::new(chain))),
        Some(data),
    ));
    assert_eq!(s.eval::<String>("return GetMacroIconInfo(1)").unwrap(), "");
    let count = s.eval::<i64>("return GetNumMacroIcons()").unwrap();
    assert!(count > 0, "the stock chain lists its icons");
    assert!(s
        .eval::<String>("return GetMacroIconInfo(1)")
        .unwrap()
        .starts_with("Interface\\Icons\\"));
}

/// A mini app around [`super::rebind_macro_spells`]: the VM, an empty bar and the `/cast` alias.
fn rebind_app(script: UiScript) -> bevy::prelude::App {
    use bevy::prelude::*;
    let mut app = App::new();
    app.init_resource::<crate::ui_action::PlayerActions>()
        .init_resource::<super::MacroBoundSpells>()
        .insert_resource(crate::ui_chat::commands::SlashCommands::build(
            |name| (name == "SLASH_CAST1").then(|| "/cast".to_string()),
            |_| None,
        ))
        .insert_non_send_resource(script)
        .add_systems(Update, super::rebind_macro_spells);
    app
}

fn book_slot(spell_id: u32, name: &str) -> benilla_ui::script::SpellSlotView {
    benilla_ui::script::SpellSlotView {
        spell_id,
        name: name.into(),
        ..Default::default()
    }
}

/// `0x4f0ce0`, from the pet spell message (`0x4bdae0`), derives only the records whose cached
/// spell is below 0 or whose book selector is the pet's (`0x4f0cfb`-`0x4f0d11`), and a miss leaves
/// the selector (`0x4eff81` stores the spell alone). So a `/cast` that missed binds when the pet's
/// spell arrives; a `CastSpellByName` that was bound to it and lost it binds again when the pet
/// returns; and one made while the pet was away keeps its 0 through the pet's list alone. The
/// engine's copy carries the book, which `GetActionAutocast` reads.
#[test]
fn the_pets_spell_list_rebinds_the_failed_and_the_pet_bound_macros_only() {
    use super::run::{Bound, BoundSpell};
    use benilla_ui::script::{ActionSlot, PetBookState};

    let mut script = UiScript::new().unwrap();
    script.set_macros(MacroState::from_scopes(
        vec![
            macro_view("Seduce", "/cast Seduction"),
            macro_view("Quoted", r#"/script CastSpellByName("Seduction")"#),
            macro_view("Bolt", "/cast Shadow Bolt"),
        ],
        Vec::new(),
    ));
    script.set_spellbook(benilla_ui::script::SpellBookState {
        tabs: Vec::new(),
        slots: vec![book_slot(686, "Shadow Bolt")],
    });
    let seduction = PetBookState {
        token: None,
        slots: vec![benilla_ui::script::SpellSlotView {
            autocast: Some((true, false)),
            ..book_slot(6358, "Seduction")
        }],
    };
    script.set_pet_book(seduction.clone());
    script.set_action(
        1,
        Some(ActionSlot {
            kind: 0x40,
            action: 1,
            ..Default::default()
        }),
    );
    let mut app = rebind_app(script);
    app.update();
    let bound = |app: &bevy::prelude::App, id: u32| {
        app.world().resource::<super::MacroBoundSpells>().0[&id]
    };
    let pet_spell = Bound {
        spell: BoundSpell::Spell(6358),
        pet_book: true,
    };
    let shadow_bolt = Bound::from(BoundSpell::Spell(686));
    assert_eq!(
        (bound(&app, 1), bound(&app, 2), bound(&app, 3)),
        (pet_spell, pet_spell, shadow_bolt),
        "the pet is out: both forms bind its spell, with its book"
    );
    let script = app.world().non_send_resource::<UiScript>();
    assert_eq!(
        script
            .eval::<(Option<i64>, Option<i64>)>("return GetActionAutocast(1)")
            .unwrap(),
        (Some(1), None),
        "the binding's book reaches GetActionAutocast"
    );

    // A cooldown or autocast change is not a spell message: nothing is derived again.
    let before = script.pet_book_generation();
    let mut ticking = seduction.clone();
    ticking.slots[0].cooldown = Some((1_000, 5_000, true));
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_pet_book(ticking);
    assert_eq!(
        app.world()
            .non_send_resource::<UiScript>()
            .pet_book_generation(),
        before
    );

    // The pet is dismissed. The `/cast` is a miss (-1) and the quoted call stores 0; both keep the
    // pet's book. A quoted call made now never found the spell.
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_pet_book(PetBookState::default());
    app.world_mut()
        .non_send_resource::<UiScript>()
        .run(r#"CreateMacro("Later", 1, 'CastSpellByName("Seduction")')"#)
        .unwrap();
    app.update();
    assert_eq!(
        (
            bound(&app, 1),
            bound(&app, 2),
            bound(&app, 3),
            bound(&app, 4)
        ),
        (
            Bound {
                spell: BoundSpell::Unresolved,
                pet_book: true
            },
            Bound {
                spell: BoundSpell::None,
                pet_book: true
            },
            shadow_bolt,
            Bound::default()
        )
    );

    // It returns: the two that kept the pet's book bind again, the one made without it does not.
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_pet_book(seduction);
    app.update();
    assert_eq!(
        (
            bound(&app, 1),
            bound(&app, 2),
            bound(&app, 3),
            bound(&app, 4)
        ),
        (pet_spell, pet_spell, shadow_bolt, Bound::default()),
        "a CastSpellByName that missed keeps its 0 through the pet's list alone"
    );
}

/// The save walks the sorted table, not the list (`0x4f0ad0`-`0x4f0aeb`), and the file loads as
/// the list, so a tie the sort left reversed is sorted again by the next load: `b` made before `B`
/// sits second, is written second, and is the first after the load.
#[test]
fn a_save_writes_the_sorted_table_and_a_load_sorts_it_again() {
    use bevy::prelude::*;

    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = std::env::temp_dir().join(format!("benilla-macros-order-{}", std::process::id()));
    std::fs::remove_dir_all(&tmp).ok();
    let _c = EnvGuard::unset("WOW_CAPTURE");
    let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
    let account = crate::local_state::macros_account_path().unwrap();

    let mut script = UiScript::new().unwrap();
    script.set_macro_icons(vec!["Interface\\Icons\\Ability_Ambush".into()]);
    script
        .run(r#"CreateMacro("b", 1, "/say b") CreateMacro("B", 1, "/say B")"#)
        .unwrap();
    assert_eq!(
        script.eval::<String>("return GetMacroInfo(1)").unwrap(),
        "B"
    );
    let mut app = App::new();
    app.insert_resource(super::MacroFiles {
        account: Some(account.clone()),
        ..Default::default()
    })
    .insert_non_send_resource(script)
    .add_systems(Update, super::save_dirty_macros);
    app.update();

    let text = std::fs::read_to_string(&account).unwrap();
    assert!(
        text.starts_with("MACRO 2 \"B\""),
        "the table's order: {text}"
    );
    let loaded = MacroState::from_scopes(super::store::parse(&text), Vec::new());
    assert_eq!(loaded.get(1).unwrap().name, "b");
    std::fs::remove_dir_all(&tmp).ok();
}

/// A character file of the builds that numbered by position, with buttons placed under those
/// numbers, moves once: its macros take their ids in the file, their buttons the words of those
/// ids, each sent as a placement is, and a load after it moves nothing, because a word from 19 to 36
/// is then an account macro's.
#[test]
fn a_position_numbered_character_bar_moves_to_macro_ids_once() {
    use crate::ui_action::PlayerActions;
    use benilla_protocol::messages::ActionButton;
    use bevy::prelude::*;

    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = std::env::temp_dir().join(format!("benilla-macros-move-{}", std::process::id()));
    std::fs::remove_dir_all(&tmp).ok();
    let _c = EnvGuard::unset("WOW_CAPTURE");
    let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
    let character = crate::local_state::macros_character_path("Realm", "Probeone").unwrap();

    // Written in the order they were made, ids 2, 1, 3: the old build read them back by id, so
    // Alpha (1) sat first, Bravo (2) second and Charlie (3) third, buttons 19, 20 and 21.
    crate::local_state::write_atomic(
        &character,
        "MACRO 2 \"Bravo\" Ability_Ambush\n/say b\nEND\n\
         MACRO 1 \"Alpha\" Ability_Ambush\n/say a\nEND\n\
         MACRO 3 \"Charlie\" Ability_Ambush\n/say c\nEND\n",
    )
    .unwrap();
    // `(wire slot, word)`: three character macros, one past the last (cleared), a spell, an
    // account macro, and a word past 36.
    let placed = [
        (0u8, 0x4000_0013u32),
        (1, 0x4000_0015),
        (2, 0x4000_0014),
        (3, 0x4000_0016),
        (4, 133),
        (5, 0x4000_0001),
        (6, 0x4000_0025),
    ];
    let mut actions = PlayerActions::default();
    for (slot, word) in placed {
        actions.buttons.insert(
            slot,
            ActionButton {
                slot,
                action: word & 0x00ff_ffff,
                kind: (word >> 24) as u8,
            },
        );
    }
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.insert_resource(actions)
        .insert_resource(crate::net::NetCommands(tx))
        .insert_resource(crate::glue_session::roster::Roster::with_pending_pick(
            vec![crate::glue_session::roster::test_character(7, "Probeone")],
            7,
        ))
        .init_resource::<super::MacroFiles>()
        .insert_non_send_resource(UiScript::new().unwrap())
        .add_systems(
            Update,
            (super::load_macros, crate::ui_action::drain_action_sets).chain(),
        );
    let sent = |rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>| {
        rx.try_iter()
            .map(|c| match c {
                crate::net::ClientCommand::SetActionButton { button, packed } => (button, packed),
                other => panic!("unexpected {other:?}"),
            })
            .collect::<Vec<_>>()
    };

    app.update();
    assert_eq!(
        sent(&rx),
        [(0, 0x4100_0001), (1, 0x4100_0003), (2, 0x4100_0002), (3, 0)],
        "Alpha, Charlie and Bravo by their ids, the button past the last macro cleared, each once"
    );
    let word = |app: &App, slot: u8| {
        let b = app.world().resource::<PlayerActions>().buttons.get(&slot)?;
        Some((u32::from(b.kind) << 24) | b.action)
    };
    let held: Vec<Option<u32>> = (0..7).map(|slot| word(&app, slot)).collect();
    assert_eq!(
        held,
        [
            Some(0x4100_0001),
            Some(0x4100_0003),
            Some(0x4100_0002),
            None,
            Some(133),
            Some(0x4000_0001),
            Some(0x4000_0025)
        ],
        "the local store follows; the spell, the account macro and the word past 36 stay"
    );
    let file = std::fs::read_to_string(&character).unwrap();
    assert!(
        file.starts_with("MACRO 16777217 \"Alpha\""),
        "the file is renumbered, in name order: {file}"
    );
    let state = app.world().non_send_resource::<UiScript>().macros();
    assert_eq!(state.by_id(0x100_0003).unwrap().name, "Charlie");

    // The next session: a fresh VM loads the renumbered file. A button at 0x40000013 is now the
    // account macro of that id, and nothing moves it.
    app.world_mut()
        .resource_mut::<PlayerActions>()
        .buttons
        .insert(
            9,
            ActionButton {
                slot: 9,
                action: 19,
                kind: 0x40,
            },
        );
    app.insert_non_send_resource(UiScript::new().unwrap());
    app.update();
    assert!(sent(&rx).is_empty(), "nothing is sent again");
    assert_eq!(word(&app, 9), Some(0x4000_0013));
    assert_eq!(
        app.world()
            .non_send_resource::<UiScript>()
            .macros()
            .count(true),
        3,
        "and the renumbered file loaded"
    );
    std::fs::remove_dir_all(&tmp).ok();
}
