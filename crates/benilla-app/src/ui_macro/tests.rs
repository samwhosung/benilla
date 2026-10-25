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

    // Bodies as the reader builds them, each line followed by a newline (`0x4f0a2b`).
    let state = MacroState::from_scopes(
        vec![macro_view("Ambush", "/cast Ambush\n/say pew\n")],
        vec![macro_view("Charge", "/cast Charge\n")],
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
        "MACRO 1 \"Ambush\" Ability_Ambush\r\n/cast Ambush\r\n/say pew\r\nEND\r\n"
    );
    // A character macro keeps its full id, the scope's base and one (`0x4eff90`).
    assert_eq!(
        std::fs::read_to_string(&character).unwrap(),
        "MACRO 16777217 \"Charge\" Ability_Ambush\r\n/cast Charge\r\nEND\r\n"
    );

    let back = MacroState::from_scopes(
        super::store::parse(&std::fs::read_to_string(&account).unwrap(), false),
        super::store::parse(&std::fs::read_to_string(&character).unwrap(), true),
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
    // No engine behind it: the calls its lines make are kept for the `take_*` readers.
    s.keep_calls();
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

/// A mini app around [`super::rebind_macro_spells`]: the VM with the shipped `/cast` alias, which
/// the derivation reads from the globals (`0x703bf0`), and an empty bar.
fn rebind_app(script: UiScript) -> bevy::prelude::App {
    use bevy::prelude::*;
    script.run(r#"SLASH_CAST1 = "/cast""#).unwrap();
    let mut app = App::new();
    app.init_resource::<crate::ui_action::PlayerActions>()
        .init_resource::<crate::ui_action::ActionSlots>()
        .init_resource::<super::MacroBoundSpells>()
        .init_resource::<crate::ui_pet::PetBar>()
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

/// One `SMSG_PET_SPELLS`'s sort of the pet book (`0x4bdadb`), which the derivation follows.
fn pet_spell_list(app: &mut bevy::prelude::App) {
    let mut bar = app.world_mut().resource_mut::<crate::ui_pet::PetBar>();
    bar.book_sorts = bar.book_sorts.wrapping_add(1);
}

/// `0x4f0ce0`, from each pet spell message (`0x4bdae0`), derives only the records whose cached
/// spell is below 0 or whose book selector is the pet's (`0x4f0cfb`-`0x4f0d11`), and a miss leaves
/// the selector (`0x4eff81` stores the spell alone). So a `/cast` that missed binds when the pet's
/// spell arrives; a `CastSpellByName` that was bound to it and lost it binds again when the pet
/// returns; and one made while the pet was away keeps its 0 through the pet's list alone. A book
/// pushed with no message derives nothing. The engine's copy carries the book, which
/// `GetActionAutocast` reads.
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

    // The pet's book empties with no spell message: nothing is derived again.
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_pet_book(PetBookState::default());
    app.update();
    assert_eq!(
        (bound(&app, 1), bound(&app, 2)),
        (pet_spell, pet_spell),
        "no spell message, no derivation"
    );

    // The pet is dismissed. The `/cast` is a miss (-1) and the quoted call stores 0; both keep the
    // pet's book. A quoted call made now never found the spell.
    pet_spell_list(&mut app);
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
    pet_spell_list(&mut app);
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
    let loaded = MacroState::from_scopes(super::store::parse(&text, false), Vec::new());
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
    let mut slots = crate::ui_action::ActionSlots::default();
    for (slot, word) in placed {
        slots.buttons.insert(
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
    app.insert_resource(PlayerActions::default())
        .insert_resource(slots)
        .insert_resource(crate::net::NetCommands(tx))
        .insert_resource(crate::glue_session::roster::Roster::with_pending_pick(
            vec![crate::glue_session::roster::test_character(7, "Probeone")],
            7,
        ))
        .init_resource::<super::MacroFiles>()
        .insert_non_send_resource(UiScript::new().unwrap())
        .add_systems(Update, super::load_macros);
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
        let b = app
            .world()
            .resource::<crate::ui_action::ActionSlots>()
            .buttons
            .get(&slot)?;
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
        .resource_mut::<crate::ui_action::ActionSlots>()
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

/// The bound spell of macro `id` in a [`rebind_app`].
fn bound_of(app: &bevy::prelude::App, id: u32) -> super::run::BoundSpell {
    app.world().resource::<super::MacroBoundSpells>().0[&id].spell
}

fn learn(app: &mut bevy::prelude::App, slots: Vec<benilla_ui::script::SpellSlotView>) {
    let ids: Vec<u32> = slots.iter().map(|s| s.spell_id).collect();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_spellbook(benilla_ui::script::SpellBookState {
            tabs: Vec::new(),
            slots,
        });
    let mut actions = app
        .world_mut()
        .resource_mut::<crate::ui_action::PlayerActions>();
    actions.spells.extend(ids);
    actions.dirty = true;
}

/// `0x4efe00` has four callers (`0x4f0cbd`, `0x4f0d11`, `0x4f123c`, `0x4f1313`): a learned spell
/// derives nothing, and a create or an edit derives the one record it touches. So a `/cast` of a
/// spell not yet known stays -1 until that macro is edited, whatever else changes.
#[test]
fn only_a_create_or_an_edit_of_that_macro_derives_its_spell_again() {
    use super::run::BoundSpell;
    let mut script = UiScript::new().unwrap();
    script.set_macros(MacroState::from_scopes(
        vec![
            macro_view("Pyro", "/cast Pyroblast"),
            macro_view("Say", "/say hi"),
        ],
        Vec::new(),
    ));
    let mut app = rebind_app(script);
    app.update();
    assert_eq!(bound_of(&app, 1), BoundSpell::Unresolved, "not yet known");

    learn(&mut app, vec![book_slot(11366, "Pyroblast")]);
    app.update();
    assert_eq!(
        bound_of(&app, 1),
        BoundSpell::Unresolved,
        "a learned spell derives nothing"
    );

    app.world_mut()
        .non_send_resource::<UiScript>()
        .run("EditMacro(GetMacroIndexByName('Say'), nil, nil, '/say yo')")
        .unwrap();
    app.update();
    assert_eq!(
        bound_of(&app, 1),
        BoundSpell::Unresolved,
        "an edit of another macro derives only that one"
    );

    app.world_mut()
        .non_send_resource::<UiScript>()
        .run(r#"CreateMacro("Again", 1, "/cast Pyroblast")"#)
        .unwrap();
    app.update();
    assert_eq!(
        (bound_of(&app, 1), bound_of(&app, 3)),
        (BoundSpell::Unresolved, BoundSpell::Spell(11366)),
        "a create derives the new record alone"
    );

    app.world_mut()
        .non_send_resource::<UiScript>()
        .run("EditMacro(GetMacroIndexByName('Pyro'), 'Pyro')")
        .unwrap();
    app.update();
    assert_eq!(
        bound_of(&app, 1),
        BoundSpell::Spell(11366),
        "its own edit does"
    );
}

/// The active player's arrival learns the initial spells, then derives every record (`0x5deacc`
/// into `0x4f0ca0`).
#[test]
fn the_players_arrival_derives_every_macro() {
    use super::run::BoundSpell;
    let mut script = UiScript::new().unwrap();
    script.set_macros(MacroState::from_scopes(
        vec![macro_view("Pyro", "/cast Pyroblast")],
        Vec::new(),
    ));
    let mut app = rebind_app(script);
    app.update();
    assert_eq!(bound_of(&app, 1), BoundSpell::Unresolved);
    // The book the arrival's learns built, with no bar or macro change beside it.
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_spellbook(benilla_ui::script::SpellBookState {
            tabs: Vec::new(),
            slots: vec![book_slot(11366, "Pyroblast")],
        });
    app.update();
    assert_eq!(
        bound_of(&app, 1),
        BoundSpell::Unresolved,
        "a book alone derives nothing"
    );
    app.world_mut().spawn(crate::net::SelfPlayer);
    app.update();
    assert_eq!(bound_of(&app, 1), BoundSpell::Spell(11366));
}

/// `SMSG_SUPERCEDED_SPELL` (`0x5e6356` into `0x4e5e60`) walks the 120 slots: a macro slot whose
/// spell (`0x4e5a50`) is the old rank gets the new one in its record's cached spell (`0x4e5ec4`),
/// derived from nothing. A macro on no slot keeps the old rank.
#[test]
fn a_superseded_rank_rewrites_the_cached_spell_of_a_macro_on_the_bar() {
    use super::run::BoundSpell;
    use benilla_protocol::messages::{ActionButton, ACTION_KIND_MACRO};
    let mut script = UiScript::new().unwrap();
    script.set_spellbook(benilla_ui::script::SpellBookState {
        tabs: Vec::new(),
        slots: vec![book_slot(116, "Frostbolt")],
    });
    script.set_macros(MacroState::from_scopes(
        vec![
            macro_view("Bar", "/cast Frostbolt"),
            macro_view("Off", "/cast Frostbolt"),
        ],
        Vec::new(),
    ));
    let mut app = rebind_app(script);
    app.world_mut()
        .resource_mut::<crate::ui_action::ActionSlots>()
        .buttons
        .insert(
            4,
            ActionButton {
                slot: 4,
                action: 1,
                kind: ACTION_KIND_MACRO,
            },
        );
    app.update();
    assert_eq!(
        (bound_of(&app, 1), bound_of(&app, 2)),
        (BoundSpell::Spell(116), BoundSpell::Spell(116))
    );

    learn(&mut app, vec![book_slot(205, "Frostbolt")]);
    app.world_mut()
        .resource_mut::<crate::ui_action::PlayerActions>()
        .superseded
        .push((116, 205));
    app.update();
    assert_eq!(
        (bound_of(&app, 1), bound_of(&app, 2)),
        (BoundSpell::Spell(205), BoundSpell::Spell(116))
    );
}

/// A supersede waits for the VM: the pairs are applied by the first pass that runs, not dropped by
/// the passes that cannot.
#[test]
fn a_supersede_with_no_vm_yet_is_kept_for_the_first_pass() {
    use super::run::BoundSpell;
    use benilla_protocol::messages::{ActionButton, ACTION_KIND_MACRO};
    let mut script = UiScript::new().unwrap();
    script.set_spellbook(benilla_ui::script::SpellBookState {
        tabs: Vec::new(),
        slots: vec![book_slot(116, "Frostbolt")],
    });
    script.set_macros(MacroState::from_scopes(
        vec![macro_view("Bar", "/cast Frostbolt")],
        Vec::new(),
    ));
    let mut app = rebind_app(script);
    app.world_mut()
        .resource_mut::<crate::ui_action::ActionSlots>()
        .buttons
        .insert(
            4,
            ActionButton {
                slot: 4,
                action: 1,
                kind: ACTION_KIND_MACRO,
            },
        );
    app.update();
    assert_eq!(bound_of(&app, 1), BoundSpell::Spell(116));

    let script = app
        .world_mut()
        .remove_non_send_resource::<UiScript>()
        .unwrap();
    app.world_mut()
        .resource_mut::<crate::ui_action::PlayerActions>()
        .superseded
        .push((116, 205));
    app.update();
    app.world_mut().insert_non_send_resource(script);
    app.update();
    assert_eq!(bound_of(&app, 1), BoundSpell::Spell(205));
}

/// `EditMacroRecord` derives the record inside the call (`0x4f1313`), against the book as it
/// stands then: a `/cast Frostbolt(Rank 1)` edited while rank 1 is known binds it, and a
/// supersede arriving after the call moves it to rank 2 (`0x4e5ec4`). Derived a frame late,
/// against the book the supersede left, it would find no rank 1.
#[test]
fn an_edit_derives_against_the_book_at_its_call() {
    use super::run::BoundSpell;
    use benilla_protocol::messages::{ActionButton, ACTION_KIND_MACRO};
    let ranked = |id: u32, rank: &str| benilla_ui::script::SpellSlotView {
        rank: Some(rank.into()),
        ..book_slot(id, "Frostbolt")
    };
    let mut script = UiScript::new().unwrap();
    script.set_spellbook(benilla_ui::script::SpellBookState {
        tabs: Vec::new(),
        slots: vec![ranked(116, "Rank 1")],
    });
    script.set_macros(MacroState::from_scopes(
        vec![macro_view("Bar", "/say hi")],
        Vec::new(),
    ));
    let mut app = rebind_app(script);
    app.world_mut()
        .resource_mut::<crate::ui_action::ActionSlots>()
        .buttons
        .insert(
            4,
            ActionButton {
                slot: 4,
                action: 1,
                kind: ACTION_KIND_MACRO,
            },
        );
    app.update();
    assert_eq!(bound_of(&app, 1), BoundSpell::None);

    app.world_mut()
        .non_send_resource::<UiScript>()
        .run("EditMacro(1, nil, nil, '/cast Frostbolt(Rank 1)')")
        .unwrap();
    // The next packet: rank 2 supersedes rank 1.
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_spellbook(benilla_ui::script::SpellBookState {
            tabs: Vec::new(),
            slots: vec![ranked(205, "Rank 2")],
        });
    app.world_mut()
        .resource_mut::<crate::ui_action::PlayerActions>()
        .superseded
        .push((116, 205));
    app.update();
    assert_eq!(bound_of(&app, 1), BoundSpell::Spell(205));
}

/// A load and our player's arrival in one pass derive every record once: the next pass, with
/// neither, derives nothing, so a spell learned after them leaves a `/cast` at -1.
#[test]
fn a_load_and_an_arrival_in_one_pass_derive_once() {
    use super::run::BoundSpell;
    let mut script = UiScript::new().unwrap();
    script.set_macros(MacroState::from_scopes(
        vec![macro_view("Pyro", "/cast Pyroblast")],
        Vec::new(),
    ));
    let mut app = rebind_app(script);
    app.world_mut().spawn(crate::net::SelfPlayer);
    app.update();
    assert_eq!(bound_of(&app, 1), BoundSpell::Unresolved);
    learn(&mut app, vec![book_slot(11366, "Pyroblast")]);
    // A change that runs the pass: the macro table's generation moves with no derive owed.
    app.world_mut()
        .non_send_resource::<UiScript>()
        .run("CreateMacro('Other', 1, '/say hi')")
        .unwrap();
    app.update();
    assert_eq!(
        bound_of(&app, 1),
        BoundSpell::Unresolved,
        "the load's flag was taken with the arrival's"
    );
}

/// The macro load's harness over a state folder of its own: the loader and the saver, with the
/// account-data state they talk to, every section settled.
fn file_app(script: UiScript) -> bevy::prelude::App {
    use bevy::prelude::*;
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.insert_resource(crate::ui_action::PlayerActions::default())
        .init_resource::<crate::ui_action::ActionSlots>()
        .insert_resource(crate::net::NetCommands(tx))
        .insert_resource(crate::glue_session::roster::Roster::with_pending_pick(
            vec![crate::glue_session::roster::test_character(7, "Probeone")],
            7,
        ))
        .init_resource::<super::MacroFiles>()
        .init_resource::<crate::account_data::AccountData>()
        .insert_non_send_resource(script)
        .add_systems(Update, (super::load_macros, super::save_dirty_macros));
    app.world()
        .resource::<crate::account_data::AccountData>()
        .settle_for_test(Some(("Realm".into(), "Probeone".into())));
    app
}

fn macro_names(app: &bevy::prelude::App) -> Vec<String> {
    app.world()
        .non_send_resource::<UiScript>()
        .macros()
        .views()
        .map(|m| m.name.clone())
        .collect()
}

/// A macro section the server's file replaced is read again into the VM that loaded the old
/// file, as the reference's reader runs once the section lands (`0x5afd19`).
#[test]
fn a_macro_section_from_the_server_is_read_again() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = std::env::temp_dir().join(format!("benilla-macros-reread-{}", std::process::id()));
    std::fs::remove_dir_all(&tmp).ok();
    let _c = EnvGuard::unset("WOW_CAPTURE");
    let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
    let account = crate::local_state::macros_account_path().unwrap();
    crate::local_state::write_atomic(&account, "MACRO 1 \"Old\" Ability_Ambush\nEND\n").unwrap();
    let mut app = file_app(UiScript::new().unwrap());
    app.update();
    assert_eq!(macro_names(&app), ["Old"]);

    crate::local_state::write_atomic(&account, "MACRO 1 \"New\" Ability_Ambush\nEND\n").unwrap();
    app.update();
    assert_eq!(macro_names(&app), ["Old"], "loaded once per VM");
    app.world()
        .resource::<crate::account_data::AccountData>()
        .owe_reader_for_test(4);
    app.update();
    assert_eq!(macro_names(&app), ["New"], "the server's file read again");
    std::fs::remove_dir_all(&tmp).ok();
}

/// A save writes both macro files, so both sections are marked for the interface's shutdown,
/// where the reference writes them (`0x490c5a` → `0x4f0c7f`, `0x4f0c90`); the save arms nothing.
#[test]
fn a_macro_save_marks_both_sections_for_the_shutdown() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = std::env::temp_dir().join(format!("benilla-macros-arm-{}", std::process::id()));
    std::fs::remove_dir_all(&tmp).ok();
    let _c = EnvGuard::unset("WOW_CAPTURE");
    let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
    let mut script = UiScript::new().unwrap();
    script.set_macro_icons(vec!["Interface\\Icons\\Ability_Ambush".into()]);
    let mut app = file_app(script);
    app.update();
    let armed = |app: &bevy::prelude::App, section: usize| {
        let data = app.world().resource::<crate::account_data::AccountData>();
        assert!(!data.armed_for_test(section), "a save arms nothing");
        data.saved_for_test(section)
    };
    assert!(!armed(&app, 4) && !armed(&app, 5));
    app.world()
        .non_send_resource::<UiScript>()
        .run(r#"CreateMacro("Hi", 1, "/say hi")"#)
        .unwrap();
    app.update();
    assert!(armed(&app, 4) && armed(&app, 5));
    std::fs::remove_dir_all(&tmp).ok();
}
