//! **"`Quiver.CastPetAction` is nil when the addon's own macro runs"** — bug B267, reproduced end
//! to end and then closed.
//!
//! ## Why this file exists
//!
//! The report is a `/run Quiver.CastPetAction("Furious Howl")` macro raising
//! `attempt to call field 'CastPetAction' (a nil value)`. The field is published on the addon's
//! **last** line of `VARIABLES_LOADED`:
//!
//! ```lua
//! if event == "VARIABLES_LOADED" then
//!     LoadLocale() Migrations() savedVariablesRestore()
//!     initSlashCommandsAndModules()      -- builds the whole config UI, HUNTERS ONLY
//!     RegisterGlobalFunctions()          -- <- Quiver.CastPetAction = … lives here
//! ```
//!
//! so anything that raises in that handler takes every global function with it. Three separate
//! walls did, one behind the other, and each is a real gap of ours:
//!
//! | | what it does | who found it |
//! |---|---|---|
//! | the Region method map was two implementations, not one | `Api._Height = WorldFrame.GetHeight` applied to a Texture raised `stale or invalid frame handle` | this bug |
//! | `Button:SetFontString` missing | every dropdown option row died on its first line | behind wall 1 |
//! | `Frame:SetMinResize`/`SetMaxResize` missing | `SideEffectMakeMoveable` died on every module frame | behind wall 2 |
//!
//! **The addon survey scored Quiver `loaded, session=ok, probe=ok` through all three.** It seats a
//! WARRIOR, and every one of these is behind `if cl == "HUNTER"` — the same blind spot shape as
//! Bagnon's (`ui_script::bagnon_render_tests`), one axis over: there the columns asked "did it
//! raise" and never "did it draw"; here they ask both, of a session the addon declines to run in.
//! So the fixture below seats a **hunter**, and that is the load-bearing part of it.
//!
//! Nothing from the corpus is committed, and the test skips cleanly on a machine without it — the
//! `ui_chat::ace_gate_tests` rule.

use std::path::{Path, PathBuf};

use benilla_ui::script::{ScriptValue, UiScript, UnitState};
use benilla_ui::toc::Toc;

/// The corpus root **and** a Quiver in it, or `None` — a skip, never a failure. Quiver is a live
/// third-party addon (`github.com/SabineWren/Quiver`) whose shipped file is a generated bundle, so
/// a machine that wants this test builds it into the corpus once. The corpus itself resolves
/// through `benilla_formats::addon_corpus` — the one resolver, whose doc carries the incident.
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

/// A VM shaped like the reporter's session: our whole interface, and a **hunter** at the keyboard.
///
/// The class is the point (see the module doc). Everything else is the addon-survey seat's own
/// minimum — a named player on a named realm, with a faction group, because a character in a real
/// session always has all three and a nil one is a failure mode no player can produce.
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
    // A HUNTER'S SPELLBOOK. Quiver's `Api.Spell.FindSpellIndex` opens
    // `GetSpellTabInfo(GetNumSpellTabs())` and adds `offset + numSpells`, so an EMPTY book is
    // `attempt to perform arithmetic on local 'tabOffset'` every tick — the reference answers nil
    // there too, so that is the addon's own defect on a spell-less character and not a gap of
    // ours. Seating a book is what lets this test TICK, which is where the module OnUpdates live.
    // Two tabs and four spells is the addon-survey fixture's own shape, with a hunter's names.
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
    let failures = super::load_default_ui(&s);
    assert!(
        failures.is_empty(),
        "our own FrameXML failed to load: {failures:#?}"
    );
    s
}

/// **The reported symptom.** Load Quiver into a hunter's session, drive the session start the
/// client drives, and ask the question the macro asked.
///
/// The assertion is deliberately the addon's own field and not "no errors": B267 was filed as a
/// nil field, and a future wall inside `initSlashCommandsAndModules` would take this field out
/// again even if it left a different error behind.
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

    // The macro the report ran: `/run Quiver.CastPetAction("Furious Howl"); …`
    assert_eq!(
        s.eval::<String>("return type(Quiver.CastPetAction)")
            .unwrap(),
        "function",
        "B267: the addon's own field is nil because its VARIABLES_LOADED handler died \
         before publishing it — errors so far: {:#?}",
        s.errors()
    );
    // The whole of `RegisterGlobalFunctions`, not just the one the report named — they publish
    // together, so any of them missing means the same handler died at the same place.
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

/// The session start itself raises **nothing** — the three walls, stated as the thing they were.
///
/// Separate from the field assertion above because it fails differently and more usefully: this is
/// the test that names a *new* wall the moment one appears, instead of reporting a nil field.
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
    // …and then a second of frames, because Quiver's modules are OnUpdate-driven (the auto-shot
    // timer, the range indicator, the aspect tracker) and a handler that only raises once it is
    // *running* is invisible to the event burst alone.
    for _ in 0..10 {
        s.tick(0.1);
    }
    let raised = s.take_errors();
    assert!(
        raised.is_empty(),
        "Quiver raised at session start: {raised:#?}"
    );
}

// ───────────────────────────────────────────────────────────────────────────────────────────────
// The Auto Shot Timer's state machine — why the bar "just stays full".
// ───────────────────────────────────────────────────────────────────────────────────────────────

/// A ranged weapon on the character sheet, so `UnitRangedDamage("player")` answers a real speed.
///
/// 2.8s is a vanilla hunter bow. The addon computes `reloadTime = speed - 0.5`, so this is what
/// makes the reload phase measurable rather than a divide by zero.
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

/// The addon's own state machine, read through the three globals it publishes for macros.
/// Reading these beats measuring the bar's pixels: they ARE what the bar draws from.
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

/// **The reported symptom, reproduced.** *"the shot timer doesn't seem to work properly when
/// standing still and shooting, it just stays full."*
///
/// Quiver's ONLY detector for "an auto shot actually fired" is `ITEM_LOCK_CHANGED` — in the real
/// client, spending an arrow toggles the ammo slot's lock, and the addon's own comment says so:
/// *"Inventory event, such as using ammo or drinking a potion. This is how we detect auto shots."*
///
/// benilla fires `ITEM_LOCK_CHANGED` only from bag / cursor / mail / loot paths. Nothing fires it
/// for ammo spent on a ranged attack. So the addon starts its 0.5s aim, saturates it, and waits
/// forever for a shot it is never told about — which is a bar pinned at 100%.
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

/// **The mechanism, proven.** The same session, plus the one event benilla never sends: the bar
/// immediately behaves. This is what pins the diagnosis on the missing ammo lock rather than on
/// the addon, on `SetWidth`, or on the standing-still check.
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
    // The arrow leaves the quiver. This is the event the real client fires and we do not.
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

// ───────────────────────────────────────────────────────────────────────────────────────────────
// The Aspect Tracker — what it actually draws, and the one aspect it deliberately does not.
// ───────────────────────────────────────────────────────────────────────────────────────────────

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

/// Is any quad drawing this art, visibly?
///
/// **The buff bar is hidden first, and that is load-bearing.** An active aura paints its own icon
/// through `BuffButton*`, so while Aspect of the Hawk is up TWO quads carry
/// `Spell_Nature_RavenForm` — the tracker's, and the player's buff bar. An earlier cut of this
/// helper counted both and reported the tracker as drawing when it had correctly hidden itself.
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

/// **The Aspect Tracker draws.** Cheetah up ⇒ the Cheetah icon is on screen.
///
/// This is the module end to end on our stack: the scanning tooltip
/// (`CreateFrame("GameTooltip", …, "GameTooltipTemplate")` → `SetPlayerBuff` → the *named*
/// `…TextLeft1` font string → a string compare against the localized spell name), then
/// `Texture:SetTexture` and the backdrop frame it lives in. Every one of those is a real
/// dependency of ours, and a break in any of them shows up here as a missing quad.
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

/// **The Hawk arm is gated on the frame LOCK, and getting that wrong looks exactly like a bug.**
///
/// `chooseIconTexture` tests seven aspects by name (Beast, Cheetah, Fox, Monkey, Viper, Wild,
/// Wolf) and then handles Hawk differently:
///
/// ```lua
/// elseif Api.Spell.PredSpellLearned(Hawk) and not Api.Aura.PredBuffActive(Hawk)
///     or not Quiver_Store.IsLockedFrames
/// ```
///
/// `and` binds tighter than `or`, so this is `(learned and NOT active) or (UNLOCKED)`. Two
/// clauses, and the second is the one that catches you out:
///
/// - the Hawk icon is a **missing-aspect reminder**, not a status light — with Hawk up it is
///   suppressed by the first clause;
/// - but **unlocked frames force it on regardless**, so you can see the frame you are dragging.
///
/// A fresh profile starts UNLOCKED, so the honest default is *the reminder shows even with Hawk
/// up*. This test asserts both halves, because an earlier cut of it asserted only "blank while
/// Hawk is up" and failed — the addon was right and the test was wrong.
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

    // Unlocked (the fresh-profile default): the reminder shows even though Hawk IS up.
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

/// **B267's second half, end to end through the real feed.** Not a hand-fired event this time:
/// a container snapshot whose arrow stack ticks down by one, pushed through
/// [`crate::ui_items::feed::apply_container_source`] exactly as a server object update does.
///
/// This is the test that would have caught the bug in the first place. The A/B above proves the
/// addon reacts to `ITEM_LOCK_CHANGED`; this proves **we actually send one when an arrow is
/// spent**, which is the half that was missing.
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

    // The server tells us the stack is one lighter. That is a fired shot, and the only way the
    // addon can ever know it.
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
