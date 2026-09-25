//! Quiver, a third-party hunter addon, in a hunter's session. Its `VARIABLES_LOADED` handler
//! builds its modules and then publishes its global functions (`Quiver.CastPetAction` and the
//! rest), so any raise in that handler loses them all. The modules run only for a hunter, so the
//! fixture seats one. Nothing from the corpus is committed; the tests skip without it.

use std::path::{Path, PathBuf};

use benilla_ui::script::{ScriptValue, UiScript, UnitState};
use benilla_ui::toc::Toc;

/// The corpus root holding a Quiver, or `None` for a skip. Quiver ships a generated bundle, so a
/// machine that wants these tests builds it into the corpus once.
fn quiver_root() -> Option<PathBuf> {
    benilla_formats::addon_corpus_candidates()
        .into_iter()
        .find(|c| c.join("Quiver").join("Quiver.toc").is_file())
}

macro_rules! quiver_or_skip {
    () => {
        match quiver_root() {
            Some(root) => root,
            None => {
                benilla_formats::skipped(
                    "no Quiver in the addon corpus (set $BENILLA_ADDON_CORPUS; the folder needs \
                     Quiver.toc + Quiver.bundle.lua)",
                    &benilla_formats::addon_corpus_candidates(),
                );
                return;
            }
        }
    };
}

fn read_toc(root: &Path, name: &str) -> Toc {
    let path = root.join(name).join(format!("{name}.toc"));
    Toc::parse(&benilla_ui::source::decode(
        &std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    ))
}

/// One addon's `.toc` files through the same two arms the real loader uses.
fn load_addon_files(script: &UiScript, root: &Path, name: &str) -> Vec<String> {
    let toc = read_toc(root, name);
    let provider = |req: &str| -> Option<Vec<u8>> { std::fs::read(root.join(req)).ok() };
    let mut errors = Vec::new();
    for file in &toc.files {
        let path = benilla_ui::loader::join_ref(name, file);
        let Ok(bytes) = std::fs::read(root.join(&path)) else {
            errors.push(format!("{file}: not found"));
            continue;
        };
        if file.to_ascii_lowercase().ends_with(".lua") {
            if let Err(e) =
                script.run_chunk_named(&bytes, &benilla_ui::script::addon_chunk_name(name, file))
            {
                errors.push(format!("{file}: {e}"));
            }
            continue;
        }
        match benilla_ui::framexml::parse(&benilla_ui::source::decode(&bytes)) {
            Ok(doc) => {
                let report = benilla_ui::loader::load_in(script, &doc, &path, &provider);
                errors.extend(report.errors.into_iter().map(|e| format!("{file}: {e}")));
            }
            Err(e) => errors.push(format!("{file}: {e}")),
        }
    }
    errors
}

/// Our whole interface with a hunter at the keyboard: named, on a named realm, with a faction
/// group, as every real session has.
fn seat_a_hunter(root: &Path) -> UiScript {
    let mut s = UiScript::new().expect("VM");
    s.set_screen_size(1024.0, 768.0);
    s.register_cvars(crate::cvars::registered_pairs());
    s.set_realm_name("Harness");
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Harness".into()),
            health: 100,
            max_health: 100,
            level: 60,
            power_type: 0,
            power: 100,
            max_power: 100,
            race: Some("Night Elf".into()),
            race_file: Some("NightElf".into()),
            class: Some("Hunter".into()),
            class_file: Some("HUNTER".into()),
            sex: 2,
            is_player: true,
            player_controlled: true,
            faction_group: Some("Alliance".into()),
            ..Default::default()
        }),
    );
    // A spellbook: Quiver's `FindSpellIndex` does arithmetic on `GetSpellTabInfo`'s offset, which
    // is nil for an empty book in the reference too, so without one every tick raises.
    {
        use benilla_ui::script::{SpellBookState, SpellSlotView, SpellTabView};
        let slot = |spell_id: u32, name: &str, rank: Option<&str>| SpellSlotView {
            spell_id,
            name: name.to_string(),
            rank: rank.map(str::to_string),
            texture: Some("Interface\\Icons\\INV_Misc_QuestionMark".into()),
            ..Default::default()
        };
        s.set_spellbook(SpellBookState {
            tabs: vec![
                SpellTabView {
                    name: "General".into(),
                    texture: Some("Interface\\Icons\\INV_Misc_QuestionMark".into()),
                    offset: 0,
                    num_spells: 2,
                },
                SpellTabView {
                    name: "Marksmanship".into(),
                    texture: Some("Interface\\Icons\\Ability_Marksmanship".into()),
                    offset: 2,
                    num_spells: 2,
                },
            ],
            slots: vec![
                slot(6603, "Attack", None),
                slot(75, "Auto Shot", None),
                slot(2973, "Raptor Strike", Some("Rank 1")),
                slot(1978, "Serpent Sting", Some("Rank 1")),
            ],
        });
    }

    let info = super::addons::info_from_toc("Quiver", &read_toc(root, "Quiver"));
    s.register_addons(vec![info], Some(root.to_path_buf()), None, None);
    // The in-game UI loads on world entry, so a player always exists by then.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(
        failures.is_empty(),
        "our own FrameXML failed to load: {failures:#?}"
    );
    s
}

/// After a hunter's session start `Quiver.CastPetAction` is a function. Asserted on the field
/// itself, since a raise anywhere in `initSlashCommandsAndModules` loses it.
#[test]
fn quiver_publishes_its_global_functions_for_a_hunter() {
    benilla_formats::wow_data_or_skip!();
    let root = quiver_or_skip!();
    let mut s = seat_a_hunter(&root);

    let load_errors = load_addon_files(&s, &root, "Quiver");
    assert!(load_errors.is_empty(), "Quiver's files: {load_errors:#?}");

    s.fire_event("ADDON_LOADED", vec![ScriptValue::Str("Quiver".into())]);
    for e in ["VARIABLES_LOADED", "PLAYER_LOGIN", "PLAYER_ENTERING_WORLD"] {
        s.fire_event(e, Vec::new());
    }

    // What a `/run Quiver.CastPetAction("Furious Howl")` macro calls.
    assert_eq!(
        s.eval::<String>("return type(Quiver.CastPetAction)")
            .unwrap(),
        "function",
        "the addon's own field is nil because its VARIABLES_LOADED handler died \
         before publishing it — errors so far: {:#?}",
        s.errors()
    );
    // Everything `RegisterGlobalFunctions` publishes; they go together.
    for name in [
        "CastNoClip",
        "CastPetAction",
        "FdPrepareTrap",
        "GetSecondsRemainingReload",
        "GetSecondsRemainingShoot",
        "PredMidShot",
        "TrinketSwap1",
        "TrinketSwap2",
    ] {
        assert_eq!(
            s.eval::<String>(&format!("return type(Quiver.{name})"))
                .unwrap(),
            "function",
            "Quiver.{name} was never published"
        );
    }
}

/// A hunter's session start and a second of frames after it raise nothing; a failure here names
/// the raise.
#[test]
fn quiver_survives_a_hunter_session_start_without_raising() {
    benilla_formats::wow_data_or_skip!();
    let root = quiver_or_skip!();
    let mut s = seat_a_hunter(&root);
    assert!(load_addon_files(&s, &root, "Quiver").is_empty());

    s.fire_event("ADDON_LOADED", vec![ScriptValue::Str("Quiver".into())]);
    for e in ["VARIABLES_LOADED", "PLAYER_LOGIN", "PLAYER_ENTERING_WORLD"] {
        s.fire_event(e, Vec::new());
    }
    // Then a second of frames: the modules (the auto-shot timer, the range indicator, the aspect
    // tracker) run on OnUpdate.
    for _ in 0..10 {
        s.tick(0.1);
    }
    let raised = s.take_errors();
    assert!(
        raised.is_empty(),
        "Quiver raised at session start: {raised:#?}"
    );
}

// ── The Auto Shot Timer's state machine ────────────────────────────────────────────────────────

/// A 2.8 s bow, so `UnitRangedDamage("player")` answers a speed; the addon's reload is
/// `speed - 0.5`.
fn seat_a_bow(s: &mut UiScript) {
    use benilla_ui::script::UnitCombatStats;
    s.set_player_combat_stats(Some(UnitCombatStats {
        ranged_attack_time_ms: 2800,
        ranged_min_damage: 31.0,
        ranged_max_damage: 47.0,
        damage_percent: 1.0,
        ..Default::default()
    }));
}

/// The event burst a real session start fires, in order.
fn start_session(s: &mut UiScript) {
    s.fire_event("ADDON_LOADED", vec![ScriptValue::Str("Quiver".into())]);
    for e in ["VARIABLES_LOADED", "PLAYER_LOGIN", "PLAYER_ENTERING_WORLD"] {
        s.fire_event(e, Vec::new());
    }
}

/// The addon's shot state, read through the three functions it publishes for macros, which the
/// bar draws from.
fn shot_state(s: &mut UiScript) -> (bool, bool, f64, f64) {
    let mid = s.eval::<bool>("return Quiver.PredMidShot()").unwrap();
    let (reloading, reload_left) = s
        .eval::<(bool, f64)>("return Quiver.GetSecondsRemainingReload()")
        .unwrap();
    let (_, shoot_left) = s
        .eval::<(bool, f64)>("return Quiver.GetSecondsRemainingShoot()")
        .unwrap();
    (mid, reloading, reload_left, shoot_left)
}

/// Quiver detects a fired auto shot only by `ITEM_LOCK_CHANGED`, which a spent arrow's stack
/// write fires; with no such event its 0.5 s aim saturates and the bar stays full.
#[test]
fn auto_shot_bar_saturates_when_no_ammo_lock_event_ever_arrives() {
    benilla_formats::wow_data_or_skip!();
    let root = quiver_or_skip!();
    let mut s = seat_a_hunter(&root);
    seat_a_bow(&mut s);
    assert!(load_addon_files(&s, &root, "Quiver").is_empty());
    start_session(&mut s);

    // The player presses the ranged-attack key, then stands still and shoots for two seconds.
    s.fire_event("START_AUTOREPEAT_SPELL", Vec::new());
    for _ in 0..20 {
        s.tick(0.1);
    }

    let (mid, reloading, _, shoot_left) = shot_state(&mut s);
    assert!(mid, "the addon does believe it is shooting");
    assert!(
        !reloading,
        "THE BUG: two seconds into a 2.8s weapon cycle and the reload phase never began, \
         because nothing told the addon a shot went off"
    );
    assert!(
        shoot_left <= 0.0,
        "THE SYMPTOM: the 0.5s aim bar saturated {shoot_left:.2}s ago and has nowhere to go — \
         this is 'it just stays full'"
    );
}

/// The same session plus one `ITEM_LOCK_CHANGED`: the reload phase starts at once.
#[test]
fn auto_shot_bar_drains_the_moment_an_ammo_lock_event_arrives() {
    benilla_formats::wow_data_or_skip!();
    let root = quiver_or_skip!();
    let mut s = seat_a_hunter(&root);
    seat_a_bow(&mut s);
    assert!(load_addon_files(&s, &root, "Quiver").is_empty());
    start_session(&mut s);

    s.fire_event("START_AUTOREPEAT_SPELL", Vec::new());
    s.tick(0.1);
    // The arrow leaves the quiver.
    s.fire_event("ITEM_LOCK_CHANGED", Vec::new());
    s.tick(0.1);

    let (_, reloading, reload_left, _) = shot_state(&mut s);
    assert!(
        reloading,
        "one ITEM_LOCK_CHANGED is the whole difference between a dead bar and a live one"
    );
    // reloadTime = UnitRangedDamage speed (2.8) - the addon's 0.5s aiming constant, less the tick.
    assert!(
        (2.0..=2.3).contains(&reload_left),
        "the reload should be draining from ~2.3s, got {reload_left:.2}"
    );
}

// ── The Aspect Tracker ─────────────────────────────────────────────────────────────────────────

/// Seat one active player buff by name and announce it the way the app's aura feed does.
fn seat_a_buff(s: &mut UiScript, spell_id: u32, name: &str, icon: &str) {
    use benilla_ui::script::AuraState;
    s.set_auras(
        "player",
        Some(vec![AuraState {
            spell_id,
            name: Some(name.into()),
            icon: Some(icon.into()),
            count: 0,
            debuff_type: None,
            duration: 0.0,
            expiration_time: 0.0,
            helpful: true,
            cancelable: true,
            until_cancelled: true,
            channeled: false,
        }]),
    );
    s.fire_event("PLAYER_AURAS_CHANGED", vec![]);
}

/// Whether a visible quad draws this art. The buff bar is hidden first: an active aura's own
/// `BuffButton` icon carries the same art as the tracker's.
fn draws(s: &mut UiScript, leaf: &str) -> bool {
    use benilla_ui::script::QuadContent;
    s.eval::<()>("if BuffFrame then BuffFrame:Hide() end")
        .unwrap();
    for i in 1..=24 {
        let _ = s.eval::<()>(&format!("if BuffButton{i} then BuffButton{i}:Hide() end"));
    }
    s.tick(0.1);
    s.resolve();
    s.extract().iter().any(|q| {
        q.alpha > 0.0
            && q.rect.is_some()
            && matches!(&q.content,
                QuadContent::Texture { path: Some(p), .. }
                    if p.to_ascii_lowercase().ends_with(&leaf.to_ascii_lowercase()))
    })
}

/// With Aspect of the Cheetah up the tracker draws its icon, found through a scanning tooltip:
/// `SetPlayerBuff`, then the named `TextLeft1` line compared with the spell name.
#[test]
fn aspect_tracker_draws_the_icon_for_an_active_aspect() {
    benilla_formats::wow_data_or_skip!();
    let root = quiver_or_skip!();
    let mut s = seat_a_hunter(&root);
    assert!(load_addon_files(&s, &root, "Quiver").is_empty());
    start_session(&mut s);

    seat_a_buff(
        &mut s,
        5118,
        "Aspect of the Cheetah",
        "Interface\\Icons\\Ability_Mount_JungleTiger",
    );
    assert!(
        draws(&mut s, "Ability_Mount_JungleTiger"),
        "the aspect tracker drew nothing for an active Aspect of the Cheetah — \
         errors: {:#?}",
        s.errors()
    );
}

/// `chooseIconTexture` shows the Hawk icon when `(learned and not active) or not IsLockedFrames`:
/// a reminder while Hawk is missing, and forced on while frames are unlocked, the fresh-profile
/// default, so the frame can be dragged.
#[test]
fn the_hawk_reminder_is_suppressed_only_once_frames_are_locked() {
    benilla_formats::wow_data_or_skip!();
    let root = quiver_or_skip!();
    let mut s = seat_a_hunter(&root);
    assert!(load_addon_files(&s, &root, "Quiver").is_empty());
    start_session(&mut s);

    seat_a_buff(
        &mut s,
        13165,
        "Aspect of the Hawk",
        "Interface\\Icons\\Spell_Nature_RavenForm",
    );

    // Unlocked: the reminder shows even though Hawk is up.
    s.eval::<()>("Quiver_Store.IsLockedFrames = false").unwrap();
    s.fire_event("PLAYER_AURAS_CHANGED", vec![]);
    assert!(
        draws(&mut s, "Spell_Nature_RavenForm"),
        "unlocked frames force the icon on so it can be dragged — errors: {:#?}",
        s.errors()
    );

    // Locked: the first clause governs, and Hawk being up suppresses its own reminder.
    s.eval::<()>("Quiver_Store.IsLockedFrames = true").unwrap();
    s.fire_event("PLAYER_AURAS_CHANGED", vec![]);
    assert!(
        !draws(&mut s, "Spell_Nature_RavenForm"),
        "locked + Hawk up must be blank: the reminder has nothing to remind you of"
    );
}

/// An arrow stack one lighter, pushed through [`crate::ui_items::feed::apply_container_source`]
/// as a server update is, fires `ITEM_LOCK_CHANGED` and starts Quiver's reload.
#[test]
fn spending_an_arrow_starts_quivers_reload_through_the_real_item_feed() {
    benilla_formats::wow_data_or_skip!();
    use crate::ui_items::feed::{apply_container_source, FeedMemory};
    use benilla_ui::script::{ContainerSlot, ContainerState};
    use std::collections::HashMap;

    let root = quiver_or_skip!();
    let mut s = seat_a_hunter(&root);
    seat_a_bow(&mut s);
    assert!(load_addon_files(&s, &root, "Quiver").is_empty());
    start_session(&mut s);

    let quiver = |count: u32| {
        Some(HashMap::from([(
            0i64,
            ContainerState {
                name: Some("Backpack".into()),
                num_slots: 16,
                slots: HashMap::from([(
                    1u32,
                    ContainerSlot {
                        item_id: 2512, // Rough Arrow
                        count,
                        ..Default::default()
                    },
                )]),
            },
        )]))
    };
    let mut memory = FeedMemory::default();
    apply_container_source(
        &mut s,
        &mut memory,
        quiver(200),
        Default::default(),
        Vec::new(),
        Vec::new(),
    );

    s.fire_event("START_AUTOREPEAT_SPELL", Vec::new());
    s.tick(0.1);
    let (_, reloading_before, _, _) = shot_state(&mut s);
    assert!(!reloading_before, "no shot has landed yet");

    // The server's stack count drops by one: a fired shot.
    apply_container_source(
        &mut s,
        &mut memory,
        quiver(199),
        Default::default(),
        Vec::new(),
        Vec::new(),
    );
    s.tick(0.1);

    let (_, reloading, reload_left, _) = shot_state(&mut s);
    assert!(
        reloading,
        "spending an arrow must start the reload drain — errors: {:#?}",
        s.errors()
    );
    assert!(
        (2.0..=2.3).contains(&reload_left),
        "draining from ~2.3s (2.8s bow - 0.5s aim), got {reload_left:.2}"
    );
}
