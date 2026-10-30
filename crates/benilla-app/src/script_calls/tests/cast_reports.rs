//! What a cast attempt reports, read on the script's next line: 1.12.1 makes every report inside
//! the call (`TryCastSpell 0x6e4b60`, reached through direct calls from `CastSpellByName 0x4b4ab0`
//! and `UseAction 0x4e5ee0`), so the line after a cast reads the state it left.

use super::*;
use benilla_protocol::messages::UseItemTarget;

/// The `CMSG_CAST_SPELL`s and `CMSG_CANCEL_CAST`s sent, in order.
fn casts_and_cancels(f: &Frame) -> Vec<(&'static str, u32)> {
    f.rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::CastSpell { spell_id, .. } => Some(("cast", spell_id)),
            ClientCommand::CancelCast { spell_id } => Some(("cancel", spell_id)),
            _ => None,
        })
        .collect()
}

/// A cast that goes out is set in flight inside the attempt (`0x6e5026` → `SetInFlightSpell
/// 0x6e4ad0`), so on the next line `SpellStopCasting()` (`0x6e6e80`) finds it through
/// `IsCastInFlight 0x6e3d30`, aborts it with `CMSG_CANCEL_CAST` (`0x6e6ec3` → `0x6e4970`) and
/// answers 1.
#[test]
fn spell_stop_casting_on_the_next_line_stops_the_cast_just_sent() {
    let mut f = frame(true);
    run_hosted(
        &mut f,
        r#"BEFORE = SpellStopCasting()
        CastSpellByName("Heal")
        STOPPED = SpellStopCasting()"#,
    );
    assert_eq!(global(&mut f, "BEFORE"), None, "nothing in flight yet");
    assert_eq!(global(&mut f, "STOPPED"), Some(1));
    assert_eq!(
        casts_and_cancels(&f),
        vec![("cast", HEAL), ("cancel", HEAL)]
    );
}

/// `IsCurrentAction` (`0x4e72f0`) reads the slot's live state (`ActionSlotIsCurrent 0x4e53a0`
/// compares the in-flight spell at `0x4e54c1`), so stock `ActionButtonUp`, which asks it straight
/// after `UseAction` to `SetChecked` the button (`ActionButton.lua:54-55`), sees the cast just
/// sent, with no frame between.
#[test]
fn is_current_action_on_the_next_line_reads_the_use_just_made() {
    let mut f = frame(false);
    with_book_spell(
        &mut f,
        HEAL,
        "Heal",
        benilla_formats::SpellDisplay::default(),
    );
    let world = f.app.world_mut();
    world
        .resource_mut::<crate::ui_action::ActionSlots>()
        .buttons
        .insert(
            0,
            ActionButton {
                slot: 0,
                action: HEAL,
                kind: ACTION_KIND_SPELL,
            },
        );
    run_hosted(
        &mut f,
        "BEFORE = IsCurrentAction(1) UseAction(1) AFTER = IsCurrentAction(1)",
    );
    assert_eq!(global(&mut f, "BEFORE"), None);
    assert_eq!(global(&mut f, "AFTER"), Some(1), "checked on the press");
    assert_eq!(casts(&f), vec![(HEAL, None)]);
}

/// What `REPORTS` held when the script set `SEEN` to it, joined with commas.
fn seen(f: &mut Frame) -> String {
    f.app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .eval::<Option<String>>("return SEEN")
        .expect("the global reads")
        .unwrap_or_default()
}

/// The `REPORTS` snapshot a chunk takes on its next line.
const SNAPSHOT: &str = r#"SEEN = table.concat(REPORTS, ",")"#;

/// Us dead, a corpse: TryCast's dead rung (`0x6e4c49`) refuses every cast with 0x13.
fn dead(f: &mut Frame) {
    let world = f.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world
        .entity_mut(me)
        .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x19),
            (HEALTH, 0),
            (28, 100),
            (FLAGS, 0x8),
        ])));
}

/// A refused cast is reported inside `TryCastSpell` (`0x6e4c66` → `ReportCastFailure 0x6e1a00`):
/// the line first (`DisplayError`, `0x6e21dd`), then, with nothing in flight, `SPELLCAST_FAILED`
/// (`0x6e2228`-`0x6e224a`), so the script's next line reads both, and nothing goes out.
#[test]
fn a_refused_cast_shows_its_line_and_fails_inside_the_call() {
    let mut f = frame(true);
    dead(&mut f);
    run_hosted(&mut f, &format!(r#"CastSpellByName("Heal") {SNAPSHOT}"#));
    assert_eq!(seen(&mut f), "SPELL_FAILED_CASTER_DEAD,SPELLCAST_FAILED");
    assert_eq!(casts_and_cancels(&f), vec![]);
    assert_eq!(reports(&mut f).len(), 2);
    f.app
        .world_mut()
        .run_system_once(crate::ui_cast::signal_reports)
        .unwrap();
    assert_eq!(
        reports(&mut f),
        Vec::<String>::new(),
        "and the frame's feed signals nothing again"
    );
}

/// The refusal's combat-log line is written inside the report as well: after `DisplayError`
/// (`0x6e21dd`), `0x6e21fc` calls `LogCastFailure 0x62c360`, whose `AddCombatLogLine 0x626850`
/// fires `CHAT_MSG_SPELL_FAILED_LOCALPLAYER` at once (`0x49b1b0` → `0x49b0b0`), ahead of the
/// report's `SPELLCAST_FAILED` (`0x6e224a`). So the script's next line reads all three, and the
/// chat feed is left only the line's tee to the log files.
#[test]
fn a_refused_casts_log_line_fires_inside_the_call_between_its_line_and_its_failure() {
    let mut f = frame(true);
    with_book_spell(
        &mut f,
        HEAL,
        "Heal",
        benilla_formats::SpellDisplay {
            id: HEAL,
            name: "Heal".into(),
            ..Default::default()
        },
    );
    dead(&mut f);
    run(
        &mut f,
        r#"SPELLFAILCASTSELF = "You fail to cast %s: %s."
        local c = CreateFrame("Frame")
        c:RegisterEvent("CHAT_MSG_SPELL_FAILED_LOCALPLAYER")
        c:SetScript("OnEvent", function() table.insert(REPORTS, event .. ":" .. arg1) end)"#,
    );
    run_hosted(&mut f, &format!(r#"CastSpellByName("Heal") {SNAPSHOT}"#));
    assert_eq!(
        seen(&mut f),
        "SPELL_FAILED_CASTER_DEAD,\
         CHAT_MSG_SPELL_FAILED_LOCALPLAYER:You fail to cast Heal: SPELL_FAILED_CASTER_DEAD.,\
         SPELLCAST_FAILED"
    );
    assert_eq!(
        f.app
            .world()
            .resource::<crate::ui_chat::ChatLog>()
            .pending_kinds(),
        ["tee"],
        "no event waits for the chat feed"
    );
}

/// Heal and Mark of the Wild in the book, neither in the catalog.
fn with_heal_and_motw(f: &mut Frame) {
    let slot = |spell_id: u32, name: &str| SpellSlotView {
        spell_id,
        name: name.into(),
        ..Default::default()
    };
    f.app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_spellbook(SpellBookState {
            tabs: vec![SpellTabView {
                name: "Book".into(),
                texture: None,
                offset: 0,
                num_spells: 2,
            }],
            slots: vec![slot(HEAL, "Heal"), slot(MOTW, "Mark of the Wild")],
        });
}

/// The tail fires nothing while another spell is in flight (`0x6e2228` → `0x6e224f`): a second
/// spell pressed over the cast just sent shows "Another action is in progress" (0x61, `0x6e4d97`)
/// and no event; the same spell pressed again is the same-spell bail (`0x6e4d43`), silent.
#[test]
fn a_refusal_over_a_cast_in_flight_fires_no_event() {
    let mut f = frame(true);
    with_heal_and_motw(&mut f);
    run_hosted(
        &mut f,
        &format!(
            r#"CastSpellByName("Heal") CastSpellByName("Heal")
            CastSpellByName("Mark of the Wild") {SNAPSHOT}"#
        ),
    );
    assert_eq!(seen(&mut f), "SPELL_FAILED_SPELL_IN_PROGRESS");
    assert_eq!(casts_and_cancels(&f), vec![("cast", HEAL)]);
}

/// Every cancel of the cursor is `StopTargeting 0x6e4900`, the cancel with its failed flag
/// (`0x6e490b`, `0x6e4940(1, 1, 0x1c)`): the word drops with no packet and the record's caster
/// fires `SPELLCAST_FAILED`, inside the call. Its two script entries: `SpellStopTargeting()`
/// (`0x6e6e4a`) and a new cast's supersede (`TryCast`, `0x6e4d71`), which fires it ahead of the
/// cast that goes on.
#[test]
fn a_cancelled_cursor_fails_inside_the_call() {
    let mut f = frame(true);
    with_the_cursor_up(&mut f);
    run_hosted(&mut f, &format!("SpellStopTargeting() {SNAPSHOT}"));
    assert_eq!(seen(&mut f), "SPELLCAST_FAILED");
    assert!(!cursor_is_up(&f));
    assert_eq!(
        casts_and_cancels(&f),
        vec![],
        "a cursor's cancel sends nothing"
    );

    let mut f = frame(true);
    with_heal_and_motw(&mut f);
    with_the_cursor_up(&mut f);
    run_hosted(
        &mut f,
        &format!(r#"CastSpellByName("Mark of the Wild") {SNAPSHOT}"#),
    );
    assert_eq!(seen(&mut f), "SPELLCAST_FAILED", "the supersede's event");
    assert!(!cursor_is_up(&f));
    assert_eq!(
        casts_and_cancels(&f),
        vec![("cast", MOTW)],
        "and the cast goes on"
    );
}

/// The pair `0x4b3250` signals, in its order, and the cast's events, as a script sees them: `A`
/// for `ACTIONBAR_UPDATE_STATE` (`0x4e5bf0`), `C` for `CURRENT_SPELL_CAST_CHANGED` (`0x4b325a`),
/// the rest by name.
fn record_changes(f: &mut Frame) {
    f.app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"CHANGES = {}
            local c = CreateFrame("Frame")
            for _, e in ipairs({"ACTIONBAR_UPDATE_STATE", "CURRENT_SPELL_CAST_CHANGED",
                    "SPELLCAST_FAILED", "SPELLCAST_STOP", "UI_ERROR_MESSAGE"}) do
                c:RegisterEvent(e)
            end
            c:SetScript("OnEvent", function()
                if event == "ACTIONBAR_UPDATE_STATE" then table.insert(CHANGES, "A")
                elseif event == "CURRENT_SPELL_CAST_CHANGED" then table.insert(CHANGES, "C")
                elseif event == "UI_ERROR_MESSAGE" then table.insert(CHANGES, arg1)
                else table.insert(CHANGES, event) end
            end)"#,
        )
        .unwrap();
}

/// `CHANGES` as the chunk's last line saw it.
const CHANGES: &str = r#"SEEN = table.concat(CHANGES, ",") CHANGES = {}"#;

/// A spell with no target word sends from the arm (`0x6e5338` → `0x6e5349`), and the attempt
/// signals the pair four times inside the call: the push (`0x6e5026`), the arm (`0x6e5333`), the
/// send (`0x6e589d`) and its end (`0x6e51e3`). The spell is on no action bar slot, and still
/// signals: the pair is the cast's, not the bar's.
#[test]
fn a_cast_from_no_bar_slot_signals_its_four_changes_inside_the_call() {
    let mut f = frame(false);
    with_book_spell(
        &mut f,
        HEAL,
        "Heal",
        benilla_formats::SpellDisplay::default(),
    );
    record_changes(&mut f);
    run_hosted(&mut f, &format!(r#"CastSpellByName("Heal") {CHANGES}"#));
    assert_eq!(seen(&mut f), "A,C,A,C,A,C,A,C");
    assert_eq!(casts(&f), vec![(HEAL, None)]);
}

/// A cast bound to the selection goes through `BindTarget`'s merge, which signals once more before
/// the send (`0x6e60cb`): five pairs.
#[test]
fn a_cast_bound_to_the_selection_signals_five_changes() {
    let mut f = frame(true);
    // A friendly player (`OBJECT_FIELD_TYPE` 0x19, `UNIT_FLAG_PLAYER_CONTROLLED`), whom
    // `CanAssist` passes.
    with_factions(&mut f, FRIEND, &[(2, 0x19), (FLAGS, 0x8)]);
    with_book_spell(
        &mut f,
        MOTW,
        "Mark of the Wild",
        benilla_formats::SpellDisplay {
            implicit_target_a1: 21,
            ..Default::default()
        },
    );
    record_changes(&mut f);
    run_hosted(
        &mut f,
        &format!(r#"CastSpellByName("Mark of the Wild") {CHANGES}"#),
    );
    assert_eq!(seen(&mut f), "A,C,A,C,A,C,A,C,A,C");
    assert_eq!(casts(&f), vec![(MOTW, Some(OLD))]);
}

/// A refusal ahead of the push signals no change, only its line and event; a cursor cancelled
/// signals its word's clear (`0x6e495c`) and the slot's pop (`0x6e49d6`) ahead of its
/// `SPELLCAST_FAILED` (`0x6e4a50`).
#[test]
fn a_refusal_signals_no_change_and_a_cancel_signals_two() {
    let mut f = frame(true);
    dead(&mut f);
    record_changes(&mut f);
    run_hosted(&mut f, &format!(r#"CastSpellByName("Heal") {CHANGES}"#));
    assert_eq!(seen(&mut f), "SPELL_FAILED_CASTER_DEAD,SPELLCAST_FAILED");

    let mut f = frame(true);
    with_the_cursor_up(&mut f);
    record_changes(&mut f);
    run_hosted(&mut f, &format!("SpellStopTargeting() {CHANGES}"));
    assert_eq!(seen(&mut f), "A,C,A,C,SPELLCAST_FAILED");
}

/// Attack toggled off is StopAttack (`0x6131a0` → `0x5ecac0`): `CMSG_ATTACKSTOP`, then
/// `CancelQueuedCast` (`0x6e6f30`), which cancels the strike in the slot as `SpellStopCasting`
/// does (`0x6e6f68`): its `CMSG_CANCEL_CAST`, its pop's pair and `SPELLCAST_STOP`, inside the
/// call, so the stock bar unchecks the strike's button with the swing.
#[test]
fn an_attack_toggled_off_cancels_its_queued_strike_with_a_pair_then_stop() {
    const HEROIC_STRIKE: u32 = 78;
    let mut f = frame(true);
    with_factions(&mut f, FOE, &[(HEALTH, 100)]);
    let world = f.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world
        .entity_mut(me)
        .insert(crate::creature_anim::Engaged(OLD));
    world
        .resource_mut::<crate::spell::QueuedMeleeSpell>()
        .arm(HEROIC_STRIKE);
    world
        .resource_mut::<crate::spell::PendingCast>()
        .fill_record();
    record_changes(&mut f);
    run_hosted(&mut f, &format!("AttackTarget() {CHANGES}"));
    assert_eq!(seen(&mut f), "A,C,SPELLCAST_STOP");
    assert_eq!(casts_and_cancels(&f), vec![("cancel", HEROIC_STRIKE)]);
    assert_eq!(
        f.app
            .world()
            .resource::<crate::spell::QueuedMeleeSpell>()
            .current(),
        None
    );
}

/// A refusal whose report leaves before its line and its tail (`report_reaches_pop`): a form the
/// spell needs with no name to show, "Must be in %s" (0x56, `0x6e20c6`), refused by the attempt
/// itself, shows nothing and fires nothing; with its form named the line shows under errorId
/// `0xd6` (`0x6e1be9`), whose string the recorder defines as its own key, and the tail, nothing
/// in flight, fires `SPELLCAST_FAILED`.
#[test]
fn a_local_refusal_that_skips_its_text_skips_its_tail() {
    for (named, expected) in [
        (false, ""),
        (true, "ERR_SPELL_FAILED_SHAPESHIFT_FORM_S,SPELLCAST_FAILED"),
    ] {
        let mut f = frame(true);
        with_book_spell(
            &mut f,
            HEAL,
            "Heal",
            benilla_formats::SpellDisplay {
                stances: 1,
                ..Default::default()
            },
        );
        if named {
            f.app
                .world_mut()
                .resource_mut::<crate::ui_action::Spells>()
                .forms
                .insert(
                    1,
                    benilla_formats::ShapeshiftForm {
                        name: "Cat Form".into(),
                        ..Default::default()
                    },
                );
        }
        run_hosted(&mut f, &format!(r#"CastSpellByName("Heal") {SNAPSHOT}"#));
        assert_eq!(seen(&mut f), expected, "named {named}");
        assert_eq!(casts_and_cancels(&f), vec![], "named {named}");
    }
}

/// The auto-repeat toggle is `UseAction`'s alone (`0x4e60d1`-`0x4e60da`, `0x6e9fd0` against the
/// slot's spell): `CastSpellByName` of the running repeat spell goes through `TryCast`, which never
/// reads the key, so it casts again and the repeat runs on; the bar's press stops it (`0x6ea080`).
#[test]
fn only_the_bars_press_toggles_a_running_repeat_off() {
    const SHOOT: u32 = 5019;
    let mut f = frame(true);
    with_book_spell(
        &mut f,
        SHOOT,
        "Shoot",
        benilla_formats::SpellDisplay {
            targets: 0x2,
            attributes_ex2: 0x20,
            ..Default::default()
        },
    );
    f.app
        .world_mut()
        .resource_mut::<crate::ui_action::ActionSlots>()
        .buttons
        .insert(
            0,
            ActionButton {
                slot: 0,
                action: SHOOT,
                kind: ACTION_KIND_SPELL,
            },
        );
    let key = |f: &Frame| f.app.world().resource::<crate::spell::AutoRepeatActive>().0;
    run_hosted(&mut f, r#"CastSpellByName("Shoot")"#);
    assert_eq!(key(&f), Some(SHOOT), "the first cast starts the repeat");
    let _ = f.rx.try_iter().count();

    run_hosted(&mut f, r#"CastSpellByName("Shoot")"#);
    let sent: Vec<ClientCommand> = f.rx.try_iter().collect();
    assert!(
        !sent
            .iter()
            .any(|c| matches!(c, ClientCommand::CancelAutoRepeat)),
        "{sent:?}"
    );
    assert!(sent.iter().any(|c| matches!(
        c,
        ClientCommand::CastSpell {
            spell_id: SHOOT,
            ..
        }
    )));
    assert_eq!(key(&f), Some(SHOOT), "the repeat runs on");

    run_hosted(&mut f, "UseAction(1)");
    assert!(f
        .rx
        .try_iter()
        .any(|c| matches!(c, ClientCommand::CancelAutoRepeat)));
    assert_eq!(key(&f), None, "the bar's press stops it");
}

/// `MoveForwardStart()` mid-cast is `SetCommands` (`0x515090`) inside the call: the bit set
/// (`0x5150a7` → `0x514840`), then for a press of the mask `0x10f0` the cancel of a cast whose
/// `InterruptFlags` has bit 0 (`0x51511e` → `0x51512a` → `0x6e4940`), its `CMSG_CANCEL_CAST` sent
/// and its stop signalled before the verb returns. So the script's next line reads the stop and the
/// held bit, and its send follows the cancel on the wire. The turn keys cancel nothing (`0x5150ce`).
#[test]
fn move_forward_start_mid_cast_cancels_inside_the_call() {
    let mut f = frame(false);
    with_book_spell(
        &mut f,
        HEAL,
        "Heal",
        benilla_formats::SpellDisplay {
            interrupt_flags: 0xf,
            ..Default::default()
        },
    );
    let world = f.app.world_mut();
    world.init_resource::<crate::player::Player>();
    world.resource_mut::<crate::player::Player>().active = true;
    world
        .resource_mut::<crate::bindings::BindingsState>()
        .open_for_test();
    run(&mut f, r#"CastSpellByName("Heal")"#);
    assert_eq!(casts_and_cancels(&f), vec![("cast", HEAL)]);
    let _ = reports(&mut f);

    run_pressed(&mut f, &format!("TurnLeftStart() {SNAPSHOT}"));
    assert_eq!(seen(&mut f), "", "a turn spares the cast");
    run_pressed(
        &mut f,
        &format!(r#"MoveForwardStart() {SNAPSHOT} SendChatMessage("hi", "SAY")"#),
    );
    assert_eq!(
        seen(&mut f),
        "SPELLCAST_STOP",
        "the next line reads the stop"
    );
    let world = f.app.world();
    let binds = world.resource::<crate::bindings::BindingsState>();
    assert!(
        binds.pressed(crate::bindings::Input::MoveForward)
            && binds.just_pressed(crate::bindings::Input::MoveForward),
        "the bit is held as the call returns"
    );
    let sent: Vec<_> =
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::CancelCast { spell_id } => Some(format!("cancel {spell_id}")),
                ClientCommand::Chat { .. } => Some("chat".to_string()),
                _ => None,
            })
            .collect();
    assert_eq!(sent, [format!("cancel {HEAL}"), "chat".to_string()]);
}

/// The cancel's stop runs its listeners inside `SetCommands` (`0x51512a` → `0x6e4940`), before
/// its tick (`0x5151a0`) writes the commands' state: no write of the tick follows the report's
/// listeners ([`crate::script_calls::census::writes_after`]).
#[test]
fn a_movement_starts_cancel_report_runs_ahead_of_its_tick() {
    let mut f = frame(false);
    with_book_spell(
        &mut f,
        HEAL,
        "Heal",
        benilla_formats::SpellDisplay {
            interrupt_flags: 0xf,
            ..Default::default()
        },
    );
    let world = f.app.world_mut();
    world.init_resource::<crate::player::Player>();
    world.resource_mut::<crate::player::Player>().active = true;
    world
        .resource_mut::<crate::bindings::BindingsState>()
        .open_for_test();
    run(&mut f, r#"CastSpellByName("Heal")"#);
    let _ = reports(&mut f);
    let _watch = benilla_ui::script::census::watch_writes();
    benilla_ui::script::census::take_writes();
    run_pressed(&mut f, "MoveForwardStart()");
    assert_eq!(reports(&mut f), ["SPELLCAST_STOP"]);
    assert_eq!(
        crate::script_calls::census::writes_after(),
        Vec::<String>::new()
    );
}

/// A movement start's cast cancel signals (`0x51512a` → `0x6e4940`) ahead of its channel cancel
/// (`0x515193` → `0x6e9b70`), so the stop's listener sends before `CMSG_CANCEL_CHANNELLING`.
#[test]
fn a_movement_starts_stop_listener_sends_ahead_of_its_channel_cancel() {
    const CHANNEL: u32 = 689;
    let mut f = frame(false);
    let world = f.app.world_mut();
    let display =
        |name: &str, interrupt_flags, channel_interrupt_flags| benilla_formats::SpellDisplay {
            name: name.into(),
            interrupt_flags,
            channel_interrupt_flags,
            ..Default::default()
        };
    world.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(
            [
                (HEAL, display("Heal", 0xf, 0)),
                (CHANNEL, display("Drain Life", 0, 0x8)),
            ]
            .into(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    world
        .resource_mut::<crate::ui_action::PlayerActions>()
        .spells
        .insert(HEAL);
    world.init_resource::<crate::player::Player>();
    world.resource_mut::<crate::player::Player>().active = true;
    world
        .resource_mut::<crate::bindings::BindingsState>()
        .open_for_test();
    let me = world
        .query_filtered::<Entity, With<crate::net::SelfPlayer>>()
        .single(world)
        .expect("our player");
    world.entity_mut(me).insert(crate::net::ActiveMover);
    world.resource_mut::<crate::spell::ActiveChannel>().start(
        CHANNEL,
        10_000,
        std::time::Instant::now(),
    );
    run(&mut f, r#"CastSpellByName("Heal")"#);
    run(
        &mut f,
        r#"local r = CreateFrame("Frame")
        r:RegisterEvent("SPELLCAST_STOP")
        r:SetScript("OnEvent", function() RandomRoll(1, 2) end)"#,
    );
    let _ = f.rx.try_iter().count();
    run_pressed(&mut f, "MoveForwardStart()");
    let wire: Vec<&str> =
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::CancelCast { .. } => Some("cancel"),
                ClientCommand::RandomRoll { .. } => Some("roll"),
                ClientCommand::CancelChannelling { .. } => Some("channel"),
                _ => None,
            })
            .collect();
    assert_eq!(wire, ["cancel", "roll", "channel"]);
}

/// `SetCommands` ticks at its tail (`0x5151a0`), past its cast cancel's listeners (`0x51512a`): a
/// seated caster's forward start stands up there (`0x514e0c` → `0x60be30(0)`), with the cancel's
/// abort clearing the precast its gate reads, so the stop's listener sends ahead of
/// `CMSG_STANDSTATECHANGE`.
#[test]
fn a_movement_starts_stand_up_follows_its_stop_listeners() {
    let mut f = frame(false);
    with_book_spell(
        &mut f,
        HEAL,
        "Heal",
        benilla_formats::SpellDisplay {
            interrupt_flags: 0xf,
            ..Default::default()
        },
    );
    let world = f.app.world_mut();
    world.init_resource::<crate::player::CameraControl>();
    world.init_resource::<crate::player::FarSight>();
    world.init_resource::<crate::player::Player>();
    world.resource_mut::<crate::player::Player>().active = true;
    world
        .resource_mut::<crate::bindings::BindingsState>()
        .open_for_test();
    run(&mut f, r#"CastSpellByName("Heal")"#);
    let world = f.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world.entity_mut(me).insert(crate::net::Embodied);
    world
        .get_mut::<ObjectStore>(me)
        .expect("our descriptor")
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_UNIT_BYTES_1,
            1,
        )]));
    run(
        &mut f,
        r#"local r = CreateFrame("Frame")
        r:RegisterEvent("SPELLCAST_STOP")
        r:SetScript("OnEvent", function() RandomRoll(1, 2) end)"#,
    );
    let _ = f.rx.try_iter().count();
    run_pressed(&mut f, "MoveForwardStart()");
    let wire: Vec<String> =
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::CancelCast { .. } => Some("cancel".to_string()),
                ClientCommand::RandomRoll { .. } => Some("roll".to_string()),
                ClientCommand::StandStateChange { state } => Some(format!("stand {state}")),
                _ => None,
            })
            .collect();
    assert_eq!(wire, ["cancel", "roll", "stand 0"]);
}

/// `MouselookStart()` under a left drag mid-cast is `SetCommands(1, 1, …)` (`0x514231` →
/// `0x515090`) inside the call: the press completes both mouse bits (`0x5150d6`-`0x5150ec`) and
/// cancels the cast there (`0x51512a` → `0x6e4940`), so the next line reads the stop and its send
/// follows the cancel. `MouselookStop()` releases the bit and cancels nothing. Main applied it at
/// the frame's latch, after the chunk.
#[test]
fn mouselook_start_under_a_left_drag_cancels_inside_the_call() {
    let mut f = frame(false);
    with_book_spell(
        &mut f,
        HEAL,
        "Heal",
        benilla_formats::SpellDisplay {
            interrupt_flags: 0xf,
            ..Default::default()
        },
    );
    let world = f.app.world_mut();
    world.init_resource::<crate::player::Player>();
    world.resource_mut::<crate::player::Player>().active = true;
    world.init_resource::<crate::player::CameraControl>();
    world
        .resource_mut::<crate::bindings::BindingsState>()
        .apply_unhosted(benilla_ui::script::BindingInput::Start(
            benilla_ui::script::HeldInput::CameraOrSelectOrMove,
        ));
    run(&mut f, r#"CastSpellByName("Heal")"#);
    assert_eq!(casts_and_cancels(&f), vec![("cast", HEAL)]);
    let _ = reports(&mut f);

    run(
        &mut f,
        &format!(r#"MouselookStart() {SNAPSHOT} SendChatMessage("hi", "SAY")"#),
    );
    assert_eq!(
        seen(&mut f),
        "SPELLCAST_STOP",
        "the next line reads the stop"
    );
    assert!(f
        .app
        .world()
        .resource::<crate::bindings::BindingsState>()
        .pressed(crate::bindings::Input::TurnOrAction));
    let sent: Vec<_> =
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::CancelCast { spell_id } => Some(format!("cancel {spell_id}")),
                ClientCommand::Chat { .. } => Some("chat".to_string()),
                _ => None,
            })
            .collect();
    assert_eq!(sent, [format!("cancel {HEAL}"), "chat".to_string()]);
}

/// The casts, attack stops and rolls sent, in order.
fn cast_wire(f: &Frame) -> Vec<String> {
    f.rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::CastSpell { spell_id, .. } => Some(format!("CastSpell {spell_id}")),
            ClientCommand::AttackStop => Some("AttackStop".to_string()),
            ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
            _ => None,
        })
        .collect()
}

/// A listener of `event` that rolls the first time it hears it.
fn roll_once_on(f: &mut Frame, event: &str) {
    run(
        f,
        &format!(
            r#"local r = CreateFrame("Frame")
            r:RegisterEvent("{event}")
            r:SetScript("OnEvent", function()
                if not ROLLED then ROLLED = 1 RandomRoll(1, 2) end
            end)"#
        ),
    );
}

/// The attempt's push and arm signal (`0x6e5026`, `0x6e5333`) ahead of `SendCast` (`0x6e5349`),
/// which runs their listeners there (`0x703e50`): a `CURRENT_SPELL_CAST_CHANGED` listener's packet
/// goes before the cast's.
#[test]
fn an_attempts_signal_listener_sends_ahead_of_its_cast() {
    let mut f = frame(false);
    with_book_spell(
        &mut f,
        HEAL,
        "Heal",
        benilla_formats::SpellDisplay::default(),
    );
    roll_once_on(&mut f, "CURRENT_SPELL_CAST_CHANGED");
    f.rx.try_iter().for_each(drop);
    run_hosted(&mut f, r#"CastSpellByName("Heal")"#);
    assert_eq!(cast_wire(&f), ["RandomRoll", "CastSpell 2050"]);
}

/// A cursor's bind merges and signals (`0x6e60cb`) ahead of its send (`0x6e60d7`), so a
/// `CURRENT_SPELL_CAST_CHANGED` listener's packet goes before the cast's.
#[test]
fn a_cursor_binds_signal_listener_sends_ahead_of_its_cast() {
    let mut f = frame(true);
    with_factions(&mut f, FOE, &[(FLAGS, 0x8)]);
    with_friendly_party1(&mut f, true);
    with_book_spell(
        &mut f,
        MOTW,
        "Mark of the Wild",
        benilla_formats::SpellDisplay {
            implicit_target_a1: 21,
            ..Default::default()
        },
    );
    run_hosted(&mut f, r#"CastSpellByName("Mark of the Wild")"#);
    assert!(cursor_is_up(&f));
    roll_once_on(&mut f, "CURRENT_SPELL_CAST_CHANGED");
    f.rx.try_iter().for_each(drop);
    run_hosted(&mut f, r#"SpellTargetUnit("party1")"#);
    assert_eq!(cast_wire(&f), ["RandomRoll", "CastSpell 1126"]);
}

/// A repeat's send signals `START_AUTOREPEAT_SPELL` (`0x6e5952`), whose listeners run before its
/// StopAttack sends `CMSG_ATTACKSTOP` (`0x6e5976` → `0x5ecac0`).
#[test]
fn a_repeats_start_listener_sends_ahead_of_its_attack_stop() {
    const AUTO_SHOT: u32 = 75;
    let mut f = frame(true);
    super::selection::swinging_at_old(&mut f);
    with_book_spell(
        &mut f,
        AUTO_SHOT,
        "Auto Shot",
        benilla_formats::SpellDisplay {
            targets: 0x2,
            attributes_ex2: 0x20,
            ..Default::default()
        },
    );
    roll_once_on(&mut f, "START_AUTOREPEAT_SPELL");
    f.rx.try_iter().for_each(drop);
    run_hosted(&mut f, r#"CastSpellByName("Auto Shot")"#);
    assert_eq!(cast_wire(&f), ["CastSpell 75", "RandomRoll", "AttackStop"]);
}

/// A listener's `SpellStopCasting` at the attempt's push signal cancels the cast in flight, which
/// zeroes the record's caster (`0x6e4a5a`), so `SendCast` finds no caster and leaves unsent
/// (`0x6e5572` → `0x6e59b0`): the wire holds the cancel alone.
#[test]
fn a_cancel_at_the_push_signal_leaves_the_cast_unsent() {
    let mut f = frame(false);
    with_book_spell(
        &mut f,
        HEAL,
        "Heal",
        benilla_formats::SpellDisplay::default(),
    );
    run(
        &mut f,
        r#"local r = CreateFrame("Frame")
        r:RegisterEvent("CURRENT_SPELL_CAST_CHANGED")
        r:SetScript("OnEvent", function()
            if not STOPPED then STOPPED = 1 SpellStopCasting() end
        end)"#,
    );
    f.rx.try_iter().for_each(drop);
    run_hosted(&mut f, r#"CastSpellByName("Heal")"#);
    assert_eq!(casts_and_cancels(&f), vec![("cancel", HEAL)]);
}

/// `spells` in the catalog and the book together.
fn seat_spells(f: &mut Frame, spells: Vec<(u32, benilla_formats::SpellDisplay)>) {
    let world = f.app.world_mut();
    let ids: Vec<u32> = spells.iter().map(|(id, _)| *id).collect();
    world.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(spells.into_iter().collect()),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    let mut actions = world.resource_mut::<crate::ui_action::PlayerActions>();
    actions.spells.extend(ids);
}

/// The potion's on-use spell.
const QUAFF: u32 = 4321;
/// Power Word: Fortitude's word, an assist unit (implicit target 21).
const FORTITUDE: u32 = 1243;

/// Backpack slot 1 holds a potion casting [`QUAFF`], an item with no `ITEM_FLAG_PLAYERCAST`, so
/// its attempt pushes nothing (`0x6e500b`-`0x6e501f`) and a listener's cast is not refused as in
/// progress; `quaff` is the spell's record. `OLD` is selected, of template `old`.
fn with_quaff(f: &mut Frame, old: u32, quaff: benilla_formats::SpellDisplay) {
    let mut t = crate::items::test_template("Potion");
    t.spells = vec![benilla_protocol::messages::ItemSpellEntry {
        index: 0,
        spell_id: QUAFF,
        trigger: 0,
        charges: 0,
        cooldown_ms: -1,
        category: 0,
        category_cooldown_ms: -1,
    }];
    t.use_spell = Some(benilla_protocol::messages::ItemUseSpell {
        spell_id: QUAFF,
        cooldown_ms: -1,
        category: 0,
        category_cooldown_ms: -1,
    });
    with_potion(f, t);
    with_factions(f, old, &[(2, 0x19), (FLAGS, 0x8)]);
    // The faction fixture's descriptor for us, with the backpack slot the potion sits in.
    typed(
        f.app.world_mut(),
        ME,
        &[(532, POTION as u32), (533, (POTION >> 32) as u32)],
    );
    seat_spells(
        f,
        vec![
            (
                QUAFF,
                benilla_formats::SpellDisplay {
                    name: "Quaff".into(),
                    ..quaff
                },
            ),
            (
                FORTITUDE,
                benilla_formats::SpellDisplay {
                    name: "Fortitude".into(),
                    implicit_target_a1: 21,
                    ..Default::default()
                },
            ),
        ],
    );
}

/// The targets of the `CMSG_USE_ITEM`s sent.
fn item_targets(f: &Frame) -> Vec<UseItemTarget> {
    f.rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::UseItem { target, .. } => Some(target),
            _ => None,
        })
        .collect()
}

/// A listener of `CURRENT_SPELL_CAST_CHANGED` that runs `lua` the first time it hears it.
fn once_on_change(f: &mut Frame, lua: &str) {
    run(
        f,
        &format!(
            r#"local r = CreateFrame("Frame")
            r:RegisterEvent("CURRENT_SPELL_CAST_CHANGED")
            r:SetScript("OnEvent", function()
                if not ONCE then ONCE = 1 {lua} end
            end)"#
        ),
    );
    f.rx.try_iter().for_each(drop);
}

/// `ArmCast` reads its candidate past the attempt's push and arm signals (`0x6e5333` →
/// `0x6e539f`): a listener there that targets another unit binds the cast to that unit.
#[test]
fn a_push_listener_that_retargets_binds_the_cast_to_its_target() {
    let mut f = frame(true);
    with_factions(&mut f, FRIEND, &[(2, 0x19), (FLAGS, 0x8)]);
    with_friendly_party1(&mut f, true);
    with_book_spell(
        &mut f,
        MOTW,
        "Mark of the Wild",
        benilla_formats::SpellDisplay {
            implicit_target_a1: 21,
            ..Default::default()
        },
    );
    once_on_change(&mut f, r#"TargetUnit("party1")"#);
    run_hosted(&mut f, r#"CastSpellByName("Mark of the Wild")"#);
    assert_eq!(casts(&f), vec![(MOTW, Some(NEW))]);
    assert_eq!(selected(&f), Some(NEW));
}

/// A cursor a listener raises at the arm's signal stands in the word the arm tests (`0x6e5338`),
/// so the arm takes its bind leg on that cursor's record (`0x6e5359`): its own candidate, the
/// player its use was aimed at (`0x6e5393`), binds the listener's spell (`0x6e53c3`), which goes
/// out at the player (`0x6e60d7`).
#[test]
fn a_listener_that_raises_a_cursor_takes_the_bind_leg() {
    let mut f = frame(true);
    with_quaff(&mut f, FOE, Default::default());
    once_on_change(&mut f, r#"CastSpellByName("Fortitude")"#);
    run_hosted(&mut f, "UseContainerItem(0, 1, 1)");
    let wire: Vec<String> = f.rx.try_iter().map(|c| format!("{c:?}")).collect();
    let fortitude = ClientCommand::CastSpell {
        spell_id: FORTITUDE,
        target: Some(ME),
    };
    assert_eq!(
        wire,
        [format!("{fortitude:?}")],
        "the potion never goes out"
    );
    assert!(!cursor_is_up(&f), "the bind emptied the word");
}

/// A listener's attempt that refills the record and is refused at its own arm's tail empties the
/// word (`0x6e5098`) and, never pushed, keeps the record's caster (`0x6e2228`), so the arm finds
/// the word empty and sends the record as the listener left it (`0x6e5338` → `0x6e5349`): the
/// potion goes out with no target, and the attempt's own refusal never comes.
#[test]
fn a_listener_that_refills_the_record_without_sending_has_it_sent() {
    let mut f = frame(true);
    with_quaff(
        &mut f,
        FRIEND,
        benilla_formats::SpellDisplay {
            implicit_target_a1: 6,
            ..Default::default()
        },
    );
    record_changes(&mut f);
    once_on_change(&mut f, "UseContainerItem(0, 1)");
    run_hosted(&mut f, &format!("UseContainerItem(0, 1) {CHANGES}"));
    assert_eq!(item_targets(&f), vec![UseItemTarget::SelfImplicit]);
    let failures = seen(&mut f).matches("SPELL_FAILED_BAD_TARGETS").count();
    assert_eq!(failures, 1, "the listener's refusal alone");
}

/// An item use passes the selection, as its verb read it, as `TryCast`'s guid (`0x4fa163`,
/// `0x5d9249` → `0x6e5a90`), which the arm takes (`0x6e5393`) without reading the selection again:
/// a listener's retarget at the arm's signal leaves the bind where the use aimed it.
#[test]
fn an_item_use_binds_the_unit_selected_at_its_call() {
    let mut f = frame(true);
    with_quaff(
        &mut f,
        FRIEND,
        benilla_formats::SpellDisplay {
            implicit_target_a1: 21,
            ..Default::default()
        },
    );
    with_friendly_party1(&mut f, true);
    once_on_change(&mut f, r#"TargetUnit("party1")"#);
    run_hosted(&mut f, "UseContainerItem(0, 1)");
    assert_eq!(item_targets(&f), vec![UseItemTarget::Unit(OLD)]);
    assert_eq!(selected(&f), Some(NEW));
}

/// `BindTarget`'s merge refuses a unit out of range (`0x6e6063` → `0x6e6089`), and the arm goes on
/// at `autoSelfCast` (`0x6e53d7`). The report cancels only a spell in flight (`0x6e2201`), so a
/// potion never pushed keeps its record's caster, and the player binds. The bar's item use passes
/// no guid (`0x4e606f`), so the arm reads the selection a listener moved (`0x6e539f`).
#[test]
fn a_bind_refused_out_of_range_goes_on_to_auto_self_cast() {
    let mut f = frame(true);
    with_quaff(
        &mut f,
        FRIEND,
        benilla_formats::SpellDisplay {
            implicit_target_a1: 21,
            range_index: 5,
            ..Default::default()
        },
    );
    with_friendly_party1(&mut f, true);
    let world = f.app.world_mut();
    world.resource_mut::<crate::ui_action::Spells>().ranges =
        benilla_formats::SpellRangeCatalog::from_rows(
            [(
                5,
                benilla_formats::SpellRange {
                    min: 0.0,
                    max: 30.0,
                    flags: 0,
                },
            )]
            .into(),
        );
    let new = world.resource::<GuidIndex>().0[&NEW];
    world.entity_mut(new).insert((
        Transform::from_xyz(100.0, 0.0, 0.0),
        GlobalTransform::from_translation(Vec3::new(100.0, 0.0, 0.0)),
    ));
    world.resource_mut::<crate::spell::AutoSelfCast>().0 = true;
    once_on_change(&mut f, r#"TargetUnit("party1")"#);
    run_hosted(&mut f, "UseAction(1)");
    assert_eq!(item_targets(&f), vec![UseItemTarget::Unit(ME)]);
}

/// `TryCast`'s tail picks its refusal of a standing enemy word by whether anything is selected
/// (`0x6e5084`), not by the candidate: cast on the player with nothing selected, it is "You have no
/// target".
#[test]
fn the_tails_refusal_reads_the_selection_not_the_candidate() {
    const BOLT: u32 = 133;
    let mut f = frame(false);
    with_book_spell(
        &mut f,
        BOLT,
        "Bolt",
        benilla_formats::SpellDisplay {
            implicit_target_a1: 6,
            ..Default::default()
        },
    );
    record_changes(&mut f);
    run_hosted(&mut f, &format!(r#"CastSpellByName("Bolt", 1) {CHANGES}"#));
    let seen = seen(&mut f);
    assert!(seen.contains("ERR_GENERIC_NO_TARGET"), "{seen}");
    assert!(!seen.contains("SPELL_FAILED_BAD_TARGETS"), "{seen}");
}
