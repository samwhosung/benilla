//! A selection call's `SetSelection` (`0x493540`) inside the call: its teardown's pet stop
//! (`0x493910`), its usable refresh and `PLAYER_TARGET_CHANGED`, all made before it returns, so a
//! chunk's next line reads what they did.

use super::*;

/// A possessed unit, the bar's: charmed by us and flagged possessed, as `0x5ee5a0` answers it.
const PET: u64 = 0xF140_0000_0000_0099;
/// `PLAYER_FARSIGHT`, the view `0x5ee5a0` reads the possessed unit from (`0x5ee5e9`).
const FARSIGHT: u16 = 712;
/// `UNIT_FIELD_CHARMEDBY`.
const CHARMEDBY: u16 = 10;

/// Record the selection's events on `EVENTS`, in order.
fn record_events(f: &mut Frame) {
    run(
        f,
        r#"EVENTS = {}
        local r = CreateFrame("Frame")
        for _, e in ipairs({"PLAYER_TARGET_CHANGED", "SPELL_UPDATE_USABLE", "PET_BAR_UPDATE"}) do
            r:RegisterEvent(e)
        end
        r:SetScript("OnEvent", function() table.insert(EVENTS, event) end)"#,
    );
}

/// The events [`record_events`] heard since the last read.
fn events(f: &mut Frame) -> Vec<String> {
    eval(f, "local e = EVENTS EVENTS = {} return e")
}

/// What the chunk's line saw, `SEEN`.
fn seen(f: &mut Frame) -> String {
    eval(f, "return SEEN")
}

/// The feeds that signal what a change no call made owes: the unit events and the action events.
fn feeds(f: &mut Frame) {
    settle(f);
    crate::ui_action::run_action_events_once(f.app.world_mut());
}

/// The pet stops and the selection sends, in order.
fn stops_and_selections(f: &Frame) -> Vec<String> {
    f.rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::PetStopAttack { .. } => Some("PetStopAttack".to_string()),
            ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
            _ => None,
        })
        .collect()
}

/// The bar's attack latch up on `PET`; `possessed` makes it `0x5ee5a0`'s answer. The world is
/// entered first, as the bar comes after it: the entry's pet reset drops a bar (`0x4bc6f0`).
fn with_pet_attacking(f: &mut Frame, possessed: bool) {
    settle(f);
    let world = f.app.world_mut();
    let pet = world
        .spawn((
            Guid(PET),
            Transform::default(),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (22, 100),
                (28, 100),
                (CHARMEDBY, ME as u32),
                (FLAGS, crate::target::UNIT_FLAG_POSSESSED),
            ])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(PET, pet);
    if possessed {
        let me = world.resource::<GuidIndex>().0[&ME];
        world.get_mut::<ObjectStore>(me).unwrap().0.merge(
            benilla_protocol::ObjectFields::from_pairs(&[
                (FARSIGHT, PET as u32),
                (FARSIGHT + 1, (PET >> 32) as u32),
            ]),
        );
    }
    world
        .resource_mut::<crate::ui_pet::PetBar>()
        .spells
        .pet_guid = PET;
    world.resource_mut::<crate::ui_pet::PetAttackLatch>().0 = Some(PET);
}

/// `SetSelection` refreshes the usable states (`0x493892`) and signals `PLAYER_TARGET_CHANGED`
/// (`0x49389c`) before it returns, and so does its release (`0x493a69`, `0x493a73`): the chunk's
/// next line has heard both, in that order, and the feeds signal the change no second time.
#[test]
fn a_selection_call_signals_its_target_change_inside_the_call() {
    let mut f = frame(true);
    feeds(&mut f);
    record_events(&mut f);
    run(
        &mut f,
        r#"TargetUnit("party1") SEEN = table.concat(EVENTS, ",")"#,
    );
    assert_eq!(selected(&f), Some(NEW));
    assert_eq!(seen(&mut f), "SPELL_UPDATE_USABLE,PLAYER_TARGET_CHANGED");
    feeds(&mut f);
    assert_eq!(
        events(&mut f),
        ["SPELL_UPDATE_USABLE", "PLAYER_TARGET_CHANGED"],
        "heard once, in the call"
    );
    run(&mut f, r#"ClearTarget() SEEN = table.concat(EVENTS, ",")"#);
    assert_eq!(selected(&f), None);
    assert_eq!(seen(&mut f), "SPELL_UPDATE_USABLE,PLAYER_TARGET_CHANGED");
    feeds(&mut f);
    assert_eq!(events(&mut f).len(), 2, "the release's, once");
    // A change no call made is the feeds' to signal, each once.
    f.app.world_mut().resource_mut::<Selection>().guid = Some(NEW);
    feeds(&mut f);
    let mut heard = events(&mut f);
    heard.sort();
    assert_eq!(heard, ["PLAYER_TARGET_CHANGED", "SPELL_UPDATE_USABLE"]);
}

/// The release `0x493910` calls a possessed unit off (`0x4bd650` at `0x493a18`) behind
/// `0x5ee5a0` (`0x493a0f`-`0x493a16`), after `StopAttack` (`0x493a08`) and before
/// `CMSG_SET_SELECTION` 0 (`0x493a2a`); `PetStopAttack` lowers the latch and signals
/// `PET_BAR_UPDATE` there (`0x4bd6ae`, `0x4bd6b4`), ahead of the release's own events.
#[test]
fn a_deselect_calls_a_possessed_unit_off_before_it_deselects() {
    let mut f = frame(true);
    with_pet_attacking(&mut f, true);
    feeds(&mut f);
    f.rx.try_iter().for_each(drop);
    record_events(&mut f);
    run(&mut f, r#"ClearTarget() SEEN = table.concat(EVENTS, ",")"#);
    assert_eq!(
        stops_and_selections(&f),
        ["PetStopAttack", "SetSelection 0x0"],
        "the stop goes out first"
    );
    assert_eq!(
        seen(&mut f),
        "PET_BAR_UPDATE,SPELL_UPDATE_USABLE,PLAYER_TARGET_CHANGED"
    );
    assert!(!f
        .app
        .world()
        .resource::<crate::ui_pet::PetAttackLatch>()
        .up());
}

/// Without `0x5ee5a0`'s answer the release asks nothing of the pet: the latch stays up and no
/// stop is sent.
#[test]
fn a_deselect_without_a_possession_leaves_the_pets_attack() {
    let mut f = frame(true);
    with_pet_attacking(&mut f, false);
    feeds(&mut f);
    f.rx.try_iter().for_each(drop);
    run(&mut f, "ClearTarget()");
    feeds(&mut f);
    assert_eq!(stops_and_selections(&f), ["SetSelection 0x0"]);
    assert!(f
        .app
        .world()
        .resource::<crate::ui_pet::PetAttackLatch>()
        .up());
}

/// We swing at `OLD`, the selection, as the server echoed it. `OLD`, `NEW` and `MOB` stream as
/// units (`OBJECT_FIELD_TYPE` 0x9), which the re-swing's latch looks the outgoing selection up as
/// (`0x49365f`).
fn swinging_at_old(f: &mut Frame) {
    let world = f.app.world_mut();
    for guid in [OLD, NEW, MOB] {
        let Some(&unit) = world.resource::<GuidIndex>().0.get(&guid) else {
            continue;
        };
        world
            .get_mut::<ObjectStore>(unit)
            .unwrap()
            .0
            .merge(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x9)]));
    }
    let me = world.resource::<GuidIndex>().0[&ME];
    world
        .entity_mut(me)
        .insert(crate::creature_anim::Engaged(OLD));
    feeds(f);
    f.rx.try_iter().for_each(drop);
}

/// A frame whose `PLAYER_TARGET_CHANGED` handler runs `body`.
fn on_target_changed(f: &mut Frame, body: &str) {
    run(
        f,
        &format!(
            r#"local r = CreateFrame("Frame")
            r:RegisterEvent("PLAYER_TARGET_CHANGED")
            r:SetScript("OnEvent", function() {body} end)"#
        ),
    );
}

/// The attack stops, selections, rolls and swings sent, in order.
fn attack_wire(f: &Frame) -> Vec<String> {
    f.rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::AttackStop => Some("AttackStop".to_string()),
            ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
            ClientCommand::AttackSwing { guid } => Some(format!("AttackSwing {guid:#x}")),
            ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
            _ => None,
        })
        .collect()
}

/// `SetSelection` sends `CMSG_SET_SELECTION` (`0x493871`), signals `PLAYER_TARGET_CHANGED`
/// (`0x49389c`), whose listeners run before the signal returns, then re-swings (`0x4938c8`): a
/// handler's packet goes out between the selection and the swing.
#[test]
fn a_switch_while_swinging_sends_the_handlers_packet_before_the_swing() {
    let mut f = frame(true);
    swinging_at_old(&mut f);
    on_target_changed(&mut f, "RandomRoll(1, 100)");
    run(&mut f, r#"TargetUnit("party1")"#);
    assert_eq!(
        attack_wire(&f),
        [
            "AttackStop",
            "SetSelection 0x22",
            "RandomRoll",
            "AttackSwing 0x22"
        ]
    );
}

/// A listener that retargets runs a whole `SetSelection` inside the first one's signal. The first
/// one's re-swing then goes at the selection as it stands: `0x4938c3` pushes the selection's
/// address, `0xb4e2d8`, which `StartAttack` reads after the listeners ran (`0x5ecbcd`). So both
/// swings go at the unit the listener selected, and each selection signals its change once.
#[test]
fn a_retargeting_listener_nests_and_the_first_swing_goes_at_its_pick() {
    let mut f = frame(true);
    spawn_mob(&mut f, &[]);
    swinging_at_old(&mut f);
    record_events(&mut f);
    on_target_changed(
        &mut f,
        "if not AGAIN then AGAIN = 1 TargetNearestEnemy() end",
    );
    run(&mut f, r#"TargetUnit("party1")"#);
    assert_eq!(selected(&f), Some(MOB));
    assert_eq!(
        attack_wire(&f),
        [
            "AttackStop",
            "SetSelection 0x22",
            "AttackStop",
            "SetSelection 0x31",
            "AttackSwing 0x31",
            "AttackSwing 0x31"
        ]
    );
    let heard = events(&mut f);
    assert_eq!(
        heard
            .iter()
            .filter(|e| *e == "PLAYER_TARGET_CHANGED")
            .count(),
        2,
        "{heard:?}"
    );
}

/// With `assistAttack` on, the assist tail swings at the unit it selected (`0x489bfc`-`0x489c02`)
/// after `SetSelection` returned, so after its `PLAYER_TARGET_CHANGED` listeners ran: a handler's
/// packet goes out between the selection and the swing.
#[test]
fn an_assists_swing_follows_its_target_change_handlers() {
    let mut f = frame(true);
    let world = f.app.world_mut();
    world.resource_mut::<crate::target::AssistAttack>().0 = true;
    let old = world.resource::<GuidIndex>().0[&OLD];
    // `UNIT_FIELD_TARGET`, the basis's target, is `NEW`, a unit StartAttack's lookup finds
    // (`0x5ecbe6`, typemask 8).
    world
        .get_mut::<ObjectStore>(old)
        .unwrap()
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[
            (16, NEW as u32),
            (17, (NEW >> 32) as u32),
        ]));
    let new = world.resource::<GuidIndex>().0[&NEW];
    world
        .get_mut::<ObjectStore>(new)
        .unwrap()
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x9)]));
    feeds(&mut f);
    f.rx.try_iter().for_each(drop);
    on_target_changed(&mut f, "RandomRoll(1, 100)");
    run(&mut f, r#"AssistUnit("target")"#);
    assert_eq!(selected(&f), Some(NEW));
    assert_eq!(
        attack_wire(&f),
        ["SetSelection 0x22", "RandomRoll", "AttackSwing 0x22"]
    );
}

/// The release's `PetStopAttack` signals `PET_BAR_UPDATE` (`0x4bd6b4`) inside the teardown
/// (`0x493a18`), ahead of `CMSG_SET_SELECTION` (`0x493871` for a switch, `0x493a2a` for a clear),
/// with the selection zeroed (`0x4939d3`): its listeners run there, so their packets go out ahead
/// of the selection's.
#[test]
fn a_called_off_pets_bar_update_handlers_send_ahead_of_the_selection() {
    for (lua, selected) in [
        (r#"TargetUnit("party1")"#, "SetSelection 0x22"),
        ("ClearTarget()", "SetSelection 0x0"),
    ] {
        let mut f = frame(true);
        with_pet_attacking(&mut f, true);
        feeds(&mut f);
        f.rx.try_iter().for_each(drop);
        run(
            &mut f,
            r#"local r = CreateFrame("Frame")
            r:RegisterEvent("PET_BAR_UPDATE")
            r:SetScript("OnEvent", function() RandomRoll(1, 100) end)"#,
        );
        run(&mut f, lua);
        let wire: Vec<String> =
            f.rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::PetStopAttack { .. } => Some("PetStopAttack".to_string()),
                    ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
                    ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
                    _ => None,
                })
                .collect();
        assert_eq!(wire, ["PetStopAttack", "RandomRoll", selected], "{lua}");
    }
}

/// The re-swing's latch tests the outgoing selection (`0x49365f`-`0x4936a3`): it resolves as a
/// unit, it is alive, and `CanAttack` (`0x606980`) allows it. In a selection a listener makes at
/// the first one's signal, the outgoing selection is the first one's pick: a friendly `party1`
/// latches no swing there, so only the first selection's re-swing goes out, at the listener's pick.
#[test]
fn a_nested_switch_off_a_friend_latches_no_swing_of_its_own() {
    let mut f = frame(true);
    with_factions(&mut f, FOE, &[]);
    let world = f.app.world_mut();
    let new = world.resource::<GuidIndex>().0[&NEW];
    world
        .get_mut::<ObjectStore>(new)
        .unwrap()
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[(
            TEMPLATE, FRIEND,
        )]));
    spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
    swinging_at_old(&mut f);
    on_target_changed(
        &mut f,
        "if not AGAIN then AGAIN = 1 TargetNearestEnemy() end",
    );
    run(&mut f, r#"TargetUnit("party1")"#);
    assert_eq!(selected(&f), Some(MOB));
    assert_eq!(
        attack_wire(&f),
        [
            "AttackStop",
            "SetSelection 0x22",
            "AttackStop",
            "SetSelection 0x31",
            "AttackSwing 0x31"
        ]
    );
}

/// A clear's send reads the selection as it stands after the teardown's `PET_BAR_UPDATE`
/// (`0x493a49`-`0x493a4f`, `0xb4e2d8`): a listener that selects there leaves that unit selected,
/// and the clear sends it again.
#[test]
fn a_clear_sends_the_unit_a_pet_bar_update_listener_selected() {
    let mut f = frame(true);
    with_pet_attacking(&mut f, true);
    feeds(&mut f);
    f.rx.try_iter().for_each(drop);
    run(
        &mut f,
        r#"local r = CreateFrame("Frame")
        r:RegisterEvent("PET_BAR_UPDATE")
        r:SetScript("OnEvent", function()
            if not ONCE then ONCE = 1 TargetUnit("party1") end
        end)"#,
    );
    run(&mut f, "ClearTarget()");
    assert_eq!(selected(&f), Some(NEW));
    assert_eq!(
        stops_and_selections(&f),
        ["PetStopAttack", "SetSelection 0x22", "SetSelection 0x22"]
    );
}

/// Selections nested at their signals through the whole host, each a level of applies on the
/// Rust stack, reach the bound of eight (`MAX_NESTING`) on a 2 MB thread, and the ninth raises.
#[test]
fn eight_nested_switches_fit_a_two_megabyte_stack() {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            let mut f = frame(true);
            spawn_mob(&mut f, &[]);
            swinging_at_old(&mut f);
            on_target_changed(
                &mut f,
                r#"ODD = not ODD
                if ODD then TargetNearestEnemy() else TargetUnit("party1") end"#,
            );
            run(&mut f, r#"TargetUnit("party1")"#);
            let selections =
                f.rx.try_iter()
                    .filter(|c| matches!(c, ClientCommand::SetSelection { .. }))
                    .count();
            assert_eq!(selections, 8);
            let errors = f
                .app
                .world_mut()
                .non_send_resource_mut::<UiScript>()
                .take_errors();
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(errors[0].contains("nested too deep"), "{errors:?}");
        })
        .expect("the thread starts")
        .join()
        .expect("eight levels fit");
}

/// StartAttack's refusal arm (`0x5ecc37` `IsAttacking` → `0x5ecc48` StopAttack `0x5ecac0`): a
/// switch from a swing to a friendly unit re-swings into a refused attack test (`0x606980` at
/// `0x5ecc2e`), which stops again, so the wire reads stop, select, stop.
#[test]
fn a_switch_to_a_friend_while_swinging_stops_again() {
    let mut f = frame(true);
    with_factions(&mut f, FOE, &[]);
    let world = f.app.world_mut();
    let new = world.resource::<GuidIndex>().0[&NEW];
    world
        .get_mut::<ObjectStore>(new)
        .unwrap()
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[(
            TEMPLATE, FRIEND,
        )]));
    swinging_at_old(&mut f);
    run(&mut f, r#"TargetUnit("party1")"#);
    assert_eq!(selected(&f), Some(NEW));
    assert_eq!(
        attack_wire(&f),
        ["AttackStop", "SetSelection 0x22", "AttackStop"]
    );
}

/// StartAttack selects its target (`0x5ecca0`-`0x5eccaa` → `0x493540`) after the stand and before
/// the swing: the attack a stealth opener defers to its `SMSG_SPELL_GO` (`0x6e8402` → `0x6131a0`
/// → `0x5ecb70`) selects the GO's first hit, whatever is selected, and its `PLAYER_TARGET_CHANGED`
/// (`0x49389c`) runs its listeners before the swing (`0x5ecd3c`).
#[test]
fn a_go_deferred_attack_selects_its_hit_before_it_swings() {
    const BACKSTAB: u32 = 53;
    let mut f = frame(true);
    spawn_mob(&mut f, &[]);
    swinging_at_old(&mut f);
    on_target_changed(&mut f, "RandomRoll(1, 100)");
    let world = f.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world
        .entity_mut(me)
        .remove::<crate::creature_anim::Engaged>();
    world.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(
            [(
                BACKSTAB,
                benilla_formats::SpellDisplay {
                    name: "Backstab".into(),
                    // `AttributesEx2` bit 20, the GO-deferred start's test (`0x6e83c0`).
                    attributes_ex2: 0x0010_0000,
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
        ),
        forms: Default::default(),
        ranges: Default::default(),
        cast_times: Default::default(),
        durations: Default::default(),
        radii: Default::default(),
    });
    crate::net::handlers::dispatch(
        world,
        vec![benilla_protocol::SessionEvent::SpellGo {
            caster: ME,
            spell_id: BACKSTAB,
            cast_flags: 0,
            hits: vec![MOB],
            misses: vec![],
            target: Some(MOB),
            go_target: None,
            target_mask: 0x2,
            source: None,
            dest: None,
            ammo_display_id: None,
            item_caster: None,
            unit_caster: ME,
        }],
    );
    assert_eq!(selected(&f), Some(MOB));
    assert_eq!(
        attack_wire(&f),
        ["SetSelection 0x31", "RandomRoll", "AttackSwing 0x31"]
    );
}

/// The wire a pick and what follows it send, in order: selections, rolls, Sinister Strike's casts
/// and swings.
fn pick_wire(f: &Frame) -> Vec<String> {
    f.rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
            ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
            ClientCommand::CastSpell {
                spell_id: SINISTER_STRIKE,
                target,
            } => Some(format!("CastSpell {:#x}", target.unwrap_or(0))),
            ClientCommand::AttackSwing { guid } => Some(format!("AttackSwing {guid:#x}")),
            _ => None,
        })
        .collect()
}

/// `MOB` is named, as the unit feed names it to the VM.
fn with_named_mob(f: &mut Frame) {
    let world = f.app.world_mut();
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(MOB, "Red Affinity".into(), None);
    world
        .run_system_once(crate::ui_unit::feed_name_candidates)
        .expect("the candidate feed runs as a one-shot system");
}

/// TryCast's attack pick (`0x6e4efb` → `0x612df0` → `0x493f60`) commits through `SetSelection`,
/// which signals `PLAYER_TARGET_CHANGED` (`0x49389c`) before the attempt goes on to its send
/// (`0x6e585a`): the listener reads the picked unit as `"target"`, finds the slot's cast not yet
/// current (`IsCurrentAction`, `0x4e53a0`), and its packet goes out between the selection and the
/// cast.
#[test]
fn a_strikes_pick_signals_its_target_change_before_the_cast() {
    let mut f = frame(false);
    spawn_mob(&mut f, &[]);
    with_named_mob(&mut f);
    with_strike(&mut f);
    feeds(&mut f);
    f.rx.try_iter().for_each(drop);
    on_target_changed(
        &mut f,
        r#"SEEN = UnitName("target") .. "," .. tostring(IsCurrentAction(1)) RandomRoll(1, 100)"#,
    );
    run(&mut f, "UseAction(1)");
    assert_eq!(
        pick_wire(&f),
        [
            "SetSelection 0x31",
            "RandomRoll",
            "CastSpell 0x31",
            "AttackSwing 0x31"
        ]
    );
    assert_eq!(seen(&mut f), "Red Affinity,nil");
}

/// The Attack validator's pick (`0x6130b5` → `0x493f60` → `SetSelection`) signals
/// `PLAYER_TARGET_CHANGED` (`0x49389c`) inside the validator, which re-reads the selection after it
/// (`0x6130c1`) before `0x6131a0` swings (`0x5ecb70`): the listener's packet goes out between the
/// selection and `CMSG_ATTACKSWING`, from `AttackTarget` and from the Attack slot alike.
#[test]
fn an_attacks_pick_signals_its_target_change_before_the_swing() {
    for lua in ["AttackTarget()", "UseAction(1)"] {
        let mut f = frame(true);
        with_factions(&mut f, FRIEND, &[]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        f.app
            .world_mut()
            .resource_mut::<crate::ui_action::PlayerActions>()
            .buttons
            .insert(
                0,
                ActionButton {
                    slot: 0,
                    action: crate::ui_action::SPELL_ATTACK,
                    kind: ACTION_KIND_SPELL,
                },
            );
        feeds(&mut f);
        f.rx.try_iter().for_each(drop);
        record_events(&mut f);
        on_target_changed(&mut f, "RandomRoll(1, 100)");
        run(&mut f, &format!("{lua} SEEN = table.concat(EVENTS, \",\")"));
        assert_eq!(
            pick_wire(&f),
            ["SetSelection 0x31", "RandomRoll", "AttackSwing 0x31"],
            "{lua}"
        );
        assert_eq!(
            seen(&mut f),
            "SPELL_UPDATE_USABLE,PLAYER_TARGET_CHANGED",
            "{lua}"
        );
        feeds(&mut f);
        assert_eq!(events(&mut f).len(), 2, "{lua}: heard once, in the call");
    }
}

/// The assist tail's StartAttack (`0x489c02` → `0x5ecb70`) selects its unit (`0x5eccaa` →
/// `0x493540`): when a `PLAYER_TARGET_CHANGED` listener moved the selection off it, that select
/// is a whole `SetSelection`, whose own signal (`0x49389c`) runs its listeners before the swing
/// (`0x5eccfd`), inside the call, and the feeds signal it no second time.
#[test]
fn an_assists_own_select_signals_inside_the_call_after_a_listener_retargets() {
    let mut f = frame(true);
    let world = f.app.world_mut();
    world.resource_mut::<crate::target::AssistAttack>().0 = true;
    let old = world.resource::<GuidIndex>().0[&OLD];
    // `OLD` targets `NEW`, and both are units StartAttack's lookup finds (`0x5ecbe6`).
    world
        .get_mut::<ObjectStore>(old)
        .unwrap()
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x9),
            (16, NEW as u32),
            (17, (NEW >> 32) as u32),
        ]));
    let new = world.resource::<GuidIndex>().0[&NEW];
    world
        .get_mut::<ObjectStore>(new)
        .unwrap()
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x9)]));
    feeds(&mut f);
    f.rx.try_iter().for_each(drop);
    record_events(&mut f);
    on_target_changed(
        &mut f,
        "RandomRoll(1, 100) if not AGAIN then AGAIN = 1 TargetLastTarget() end",
    );
    run(
        &mut f,
        r#"AssistUnit("target") SEEN = table.concat(EVENTS, ",")"#,
    );
    assert_eq!(selected(&f), Some(NEW));
    assert_eq!(
        pick_wire(&f),
        [
            "SetSelection 0x22",
            "RandomRoll",
            "SetSelection 0x21",
            "RandomRoll",
            "SetSelection 0x22",
            "RandomRoll",
            "AttackSwing 0x22"
        ]
    );
    let heard = seen(&mut f);
    assert_eq!(heard.matches("PLAYER_TARGET_CHANGED").count(), 3, "{heard}");
    feeds(&mut f);
    let again = events(&mut f);
    assert_eq!(
        again
            .iter()
            .filter(|e| *e == "PLAYER_TARGET_CHANGED")
            .count(),
        3,
        "heard in the call alone: {again:?}"
    );
}

/// `UNIT_FIELD_SUMMONEDBY`.
const SUMMONEDBY: u16 = 12;

/// The bar's unit `PET`, alive, ours (summoned by us) or, `possessed`, possessed with our view on
/// it, with `slot1` the bar's first word; the VM shows the bar, as its feed pushes it. The world
/// is entered first, as the bar comes after it: the entry's pet reset drops a bar (`0x4bc6f0`).
fn with_pet(f: &mut Frame, possessed: bool, slot1: u32) {
    settle(f);
    let world = f.app.world_mut();
    let mut fields = vec![(22, 100), (28, 100)];
    if possessed {
        fields.extend([
            (CHARMEDBY, ME as u32),
            (FLAGS, crate::target::UNIT_FLAG_POSSESSED),
        ]);
    } else {
        fields.push((SUMMONEDBY, ME as u32));
    }
    let pet = world
        .spawn((
            Guid(PET),
            Transform::default(),
            GlobalTransform::default(),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&fields)),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(PET, pet);
    if possessed {
        let me = world.resource::<GuidIndex>().0[&ME];
        world.get_mut::<ObjectStore>(me).unwrap().0.merge(
            benilla_protocol::ObjectFields::from_pairs(&[
                (FARSIGHT, PET as u32),
                (FARSIGHT + 1, (PET >> 32) as u32),
            ]),
        );
    }
    let mut bar = world.resource_mut::<crate::ui_pet::PetBar>();
    bar.spells.pet_guid = PET;
    bar.spells.bar[0] = benilla_protocol::messages::PetActionEntry::from(slot1);
    world.non_send_resource_mut::<UiScript>().set_pet_actions(
        true,
        true,
        false,
        vec![
            benilla_ui::script::PetActionView {
                packed: slot1,
                ..Default::default()
            };
            10
        ],
    );
}

/// The selections, rolls and the pet's casts and orders sent, in order.
fn pet_wire(f: &Frame) -> Vec<String> {
    f.rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
            ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
            ClientCommand::PetCastSpell { spell_id, .. } => {
                Some(format!("PetCastSpell {spell_id}"))
            }
            ClientCommand::PetAction { target_guid, .. } => {
                Some(format!("PetAction {target_guid:#x}"))
            }
            _ => None,
        })
        .collect()
}

/// The pet bar's Attack (`0x4bd1d0`) runs the validator with the pet as its actor (`0x4bd40d`),
/// whose pick commits through `SetSelection` and signals `PLAYER_TARGET_CHANGED` (`0x49389c`)
/// inside it, ahead of `CMSG_PET_ACTION` (`0x4bd4a4`): the listener's packet goes out between the
/// selection and the order, which goes at the pick.
#[test]
fn a_pet_attacks_pick_signals_its_target_change_before_the_order() {
    let attack = benilla_protocol::messages::PET_COMMAND_ATTACK
        | (u32::from(benilla_protocol::messages::PET_ACT_COMMAND) << 24);
    let mut f = frame(true);
    with_factions(&mut f, FRIEND, &[]);
    spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
    with_pet(&mut f, false, attack);
    feeds(&mut f);
    f.rx.try_iter().for_each(drop);
    on_target_changed(&mut f, "RandomRoll(1, 100)");
    run_pressed(&mut f, "PetAttack()");
    assert_eq!(
        pet_wire(&f),
        ["SetSelection 0x31", "RandomRoll", "PetAction 0x31"]
    );
}

/// A possessed unit's Attack raises its attack latch (`0x4bd42e`) and signals `PET_BAR_UPDATE`
/// (`0x4bd438`) past the validator and ahead of `CMSG_PET_ACTION` (`0x4bd4a4`): the listener's
/// packet goes out between the pick's selection and the order, it reads the Attack button lit,
/// and the bar's feed signals no second time.
#[test]
fn a_possessed_attacks_latch_signals_its_bar_before_the_order() {
    let attack = benilla_protocol::messages::PET_COMMAND_ATTACK
        | (u32::from(benilla_protocol::messages::PET_ACT_COMMAND) << 24);
    let mut f = frame(true);
    with_factions(&mut f, FRIEND, &[]);
    spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
    with_pet(&mut f, true, attack);
    feeds(&mut f);
    // The bar as its feed paints it, Attack a command token.
    let bar_feed = crate::ui_pet::pet_bar_feed(f.app.world_mut());
    f.app
        .world_mut()
        .run_system(bar_feed)
        .expect("the feed runs");
    f.rx.try_iter().for_each(drop);
    run(
        &mut f,
        r#"HEARD, LIT = 0, nil
        local r = CreateFrame("Frame")
        r:RegisterEvent("PET_BAR_UPDATE")
        r:SetScript("OnEvent", function()
            HEARD = HEARD + 1
            local _, _, _, _, active = GetPetActionInfo(1)
            LIT = active
            RandomRoll(1, 100)
        end)"#,
    );
    run_pressed(&mut f, "PetAttack()");
    assert_eq!(
        pet_wire(&f),
        ["SetSelection 0x31", "RandomRoll", "PetAction 0x31"]
    );
    assert_eq!(eval::<i64>(&mut f, "return HEARD"), 1, "in the press");
    assert!(
        eval::<bool>(&mut f, "return LIT == 1"),
        "the latch lit Attack"
    );
    f.app
        .world_mut()
        .run_system(bar_feed)
        .expect("the feed runs");
    assert_eq!(
        eval::<i64>(&mut f, "return HEARD"),
        1,
        "the feed signals no second time"
    );
}

/// A possessed unit's spell goes to the attempt with the unit as its caster (`0x4bd35e`), whose
/// attack pick (`0x6e4efb`, the unit as its actor at `0x6e4ef4`) signals its selection before the
/// send; TryCast's tail then presses the bar's Attack (`0x6e51de` → `0x6131a0` → `0x4bd630`) at
/// the guid the pick settled, after the cast.
#[test]
fn a_unit_casters_pick_signals_its_target_change_before_the_cast_and_the_order() {
    const PUMMEL: u32 = 10887;
    let mut f = frame(true);
    with_factions(&mut f, FRIEND, &[]);
    spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
    with_pet(
        &mut f,
        true,
        PUMMEL | (u32::from(benilla_protocol::messages::PET_ACT_ENABLED) << 24),
    );
    f.app.world_mut().insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(
            [(
                PUMMEL,
                benilla_formats::SpellDisplay {
                    name: "Pummel".into(),
                    // `AttributesEx 0x200`: predicate `0x6e5200`, and the send's melee start.
                    attributes_ex: 0x200,
                    ..Default::default()
                },
            )]
            .into(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    feeds(&mut f);
    f.rx.try_iter().for_each(drop);
    on_target_changed(&mut f, "RandomRoll(1, 100)");
    run_pressed(&mut f, "CastPetAction(1)");
    assert_eq!(
        pet_wire(&f),
        [
            "SetSelection 0x31",
            "RandomRoll",
            "PetCastSpell 10887",
            "PetAction 0x31"
        ]
    );
}

/// A queued on-next-swing strike in the in-flight slot, Heroic Strike's.
fn queue_strike(f: &mut Frame) {
    let world = f.app.world_mut();
    world
        .resource_mut::<crate::spell::QueuedMeleeSpell>()
        .arm(78);
    world
        .resource_mut::<crate::spell::PendingCast>()
        .fill_record();
}

/// A deselect while swinging runs StopAttack in its teardown (`0x493a08`), whose next-swing
/// cancel (`0x5ecb63` → `0x6e6f30`) reports the strike's stop there, ahead of the release's
/// `PLAYER_TARGET_CHANGED` (`0x493a73`).
#[test]
fn a_teardowns_strike_cancel_reports_before_the_target_change() {
    let mut f = frame(true);
    swinging_at_old(&mut f);
    queue_strike(&mut f);
    run(
        &mut f,
        r#"ORDER = {}
        local r = CreateFrame("Frame")
        r:RegisterEvent("SPELLCAST_STOP")
        r:RegisterEvent("PLAYER_TARGET_CHANGED")
        r:SetScript("OnEvent", function() table.insert(ORDER, event) end)"#,
    );
    run(&mut f, r#"ClearTarget() SEEN = table.concat(ORDER, ",")"#);
    assert_eq!(seen(&mut f), "SPELLCAST_STOP,PLAYER_TARGET_CHANGED");
}
