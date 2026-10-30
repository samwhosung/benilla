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
/// next line has heard both, in that order, once.
#[test]
fn a_selection_call_signals_its_target_change_inside_the_call() {
    let mut f = frame(true);
    record_events(&mut f);
    run(
        &mut f,
        r#"TargetUnit("party1") SEEN = table.concat(EVENTS, ",")"#,
    );
    assert_eq!(selected(&f), Some(NEW));
    assert_eq!(seen(&mut f), "SPELL_UPDATE_USABLE,PLAYER_TARGET_CHANGED");
    assert_eq!(
        events(&mut f),
        ["SPELL_UPDATE_USABLE", "PLAYER_TARGET_CHANGED"],
        "heard once, in the call"
    );
    run(&mut f, r#"ClearTarget() SEEN = table.concat(EVENTS, ",")"#);
    assert_eq!(selected(&f), None);
    assert_eq!(seen(&mut f), "SPELL_UPDATE_USABLE,PLAYER_TARGET_CHANGED");
    assert_eq!(events(&mut f).len(), 2, "the release's, once");
}

/// The release `0x493910` calls a possessed unit off (`0x4bd650` at `0x493a18`) behind
/// `0x5ee5a0` (`0x493a0f`-`0x493a16`), after `StopAttack` (`0x493a08`) and before
/// `CMSG_SET_SELECTION` 0 (`0x493a2a`); `PetStopAttack` lowers the latch and signals
/// `PET_BAR_UPDATE` there (`0x4bd6ae`, `0x4bd6b4`), ahead of the release's own events.
#[test]
fn a_deselect_calls_a_possessed_unit_off_before_it_deselects() {
    let mut f = frame(true);
    with_pet_attacking(&mut f, true);
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
    f.rx.try_iter().for_each(drop);
    run(&mut f, "ClearTarget()");
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
pub(super) fn swinging_at_old(f: &mut Frame) {
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

/// We swing at the selected `OLD`, now friendly, and `party1` targets it, with `assistAttack`
/// on: `AssistUnit("party1")`'s select is the setter's dedup, so its StartAttack's refused target
/// (`0x5ecc2e`) makes the call's first StopAttack (`0x5ecc48` → `0x5ecac0`). A listener of
/// `event` rolls.
fn assist_a_friend_while_swinging(f: &mut Frame, event: &str) {
    with_factions(f, FRIEND, &[]);
    let world = f.app.world_mut();
    world.resource_mut::<crate::target::AssistAttack>().0 = true;
    let new = world.resource::<GuidIndex>().0[&NEW];
    world
        .get_mut::<ObjectStore>(new)
        .unwrap()
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[
            (16, OLD as u32),
            (17, (OLD >> 32) as u32),
        ]));
    swinging_at_old(f);
    run(
        f,
        &format!(
            r#"local r = CreateFrame("Frame")
            r:RegisterEvent("{event}")
            r:SetScript("OnEvent", function() RandomRoll(1, 2) end)"#
        ),
    );
    f.rx.try_iter().for_each(drop);
}

/// The attack stops, cancels and rolls sent, in order.
fn stop_wire(f: &Frame) -> Vec<String> {
    f.rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::AttackStop => Some("AttackStop".to_string()),
            ClientCommand::CancelCast { spell_id } => Some(format!("CancelCast {spell_id}")),
            ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
            ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
            ClientCommand::AttackSwing { guid } => Some(format!("AttackSwing {guid:#x}")),
            _ => None,
        })
        .collect()
}

/// A refused StartAttack's stop (`0x5ecc48`) triggers its tutorials (`0x5ecb17`, `0x5ecb54`),
/// whose listeners run before its `CMSG_ATTACKSTOP` (`0x5ecb5b`).
#[test]
fn a_refused_assists_stop_tutorial_listener_sends_ahead_of_its_attack_stop() {
    let mut f = frame(true);
    f.app
        .world()
        .resource::<crate::tutorial::Tutorials>()
        .lock()
        .apply_flags(&[0u8; 32]);
    assist_a_friend_while_swinging(&mut f, "TUTORIAL_TRIGGER");
    run(&mut f, r#"AssistUnit("party1")"#);
    assert_eq!(selected(&f), Some(OLD));
    assert_eq!(stop_wire(&f), ["RandomRoll", "AttackStop"]);
}

/// The refused StartAttack's stop cancels the slot's strike (`0x6e6f68`), whose listeners run
/// before the save's strike goes on the wire (`0x6e6ff3`): Cleave over a queued Heroic Strike.
#[test]
fn a_refused_assists_slot_cancel_listener_sends_ahead_of_the_saves_cancel() {
    const CLEAVE: u32 = 845;
    let mut f = frame(true);
    queue_strike(&mut f);
    f.app
        .world_mut()
        .resource_mut::<crate::spell::QueuedMeleeSpell>()
        .hold(CLEAVE, 78);
    assist_a_friend_while_swinging(&mut f, "SPELLCAST_STOP");
    run(&mut f, r#"AssistUnit("party1")"#);
    assert_eq!(
        stop_wire(&f),
        [
            "AttackStop",
            "CancelCast 845",
            "RandomRoll",
            "CancelCast 78"
        ]
    );
}

/// A sit's leave (`0x5f0790`) closes the loot on the dead `MOB` we swing at and have selected
/// (`0x48f200` at `0x5f07d5`), whose last step deselects it (`0x48f369` → `0x493910`): that
/// teardown's stop, its send and its `PLAYER_TARGET_CHANGED` listener's packet go ahead of the
/// leave's own attack stop (`0x5f07e7`).
#[test]
fn a_sits_dead_loot_source_deselect_signals_ahead_of_its_attack_stop() {
    let mut f = frame(false);
    spawn_mob(&mut f, &[(2, 0x9), (HEALTH, 0)]);
    let world = f.app.world_mut();
    let mob = world.resource::<GuidIndex>().0[&MOB];
    *world.resource_mut::<Selection>() = Selection {
        target: Some(mob),
        guid: Some(MOB),
        ..Default::default()
    };
    let me = world.resource::<GuidIndex>().0[&ME];
    world
        .entity_mut(me)
        .insert(crate::creature_anim::Engaged(MOB));
    world.resource_mut::<crate::ui_loot::LootState>().open(
        MOB,
        benilla_protocol::messages::loot_type::CORPSE,
        0,
        Vec::new(),
    );
    on_target_changed(&mut f, "RandomRoll(1, 2)");
    f.rx.try_iter().for_each(drop);
    run(&mut f, "SitOrStand()");
    assert_eq!(selected(&f), None);
    let wire: Vec<String> =
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::LootRelease { guid } => Some(format!("LootRelease {guid:#x}")),
                ClientCommand::AttackStop => Some("AttackStop".to_string()),
                ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
                ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
                _ => None,
            })
            .collect();
    assert_eq!(
        wire,
        [
            "LootRelease 0x31",
            "AttackStop",
            "SetSelection 0x0",
            "RandomRoll",
            "AttackStop"
        ]
    );
}

/// `SetSelection`'s tutorial ladder (`0x4937be`-`0x49383c`) signals ahead of its send
/// (`0x49388d`): Combat Mode for an attackable unit, its `TUTORIAL_TRIGGER` listener's packet
/// before `CMSG_SET_SELECTION`, with nothing selected before and past a swing's teardown, whose
/// stop triggers Drink first (`0x5ecb54`).
#[test]
fn a_selections_ladder_listener_sends_ahead_of_its_send() {
    for swinging in [false, true] {
        let mut f = frame(swinging);
        if swinging {
            swinging_at_old(&mut f);
        } else {
            as_creature(f.app.world_mut(), NEW);
        }
        f.app
            .world()
            .resource::<crate::tutorial::Tutorials>()
            .lock()
            .apply_flags(&[0u8; 32]);
        run(
            &mut f,
            r#"local r = CreateFrame("Frame")
            r:RegisterEvent("TUTORIAL_TRIGGER")
            r:SetScript("OnEvent", function() RandomRoll(1, 2) end)"#,
        );
        f.rx.try_iter().for_each(drop);
        run(&mut f, r#"TargetUnit("party1")"#);
        let wire: Vec<String> =
            f.rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::AttackStop => Some("AttackStop".to_string()),
                    ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
                    ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
                    _ => None,
                })
                .collect();
        let expected: &[&str] = if swinging {
            &[
                "RandomRoll",
                "AttackStop",
                "RandomRoll",
                "SetSelection 0x22",
            ]
        } else {
            &["RandomRoll", "SetSelection 0x22"]
        };
        assert_eq!(wire, expected, "swinging: {swinging}");
    }
}

/// The keys of the refusals a handler queued for the action feed's lines.
fn error_keys(f: &Frame) -> Vec<String> {
    f.app
        .world()
        .resource::<crate::ui_action::UiErrorKeys>()
        .snapshot()
        .into_iter()
        .map(|e| e.key.to_string())
        .collect()
}

/// Backstab's `SMSG_SPELL_GO` from us at [`MOB`], whose start of the swing it defers
/// (`AttributesEx2` bit 20, `0x6e83c0`), with us not swinging (`[+0xc48]`, `0x6e83e7`).
fn backstab_lands(f: &mut Frame) {
    backstab_lands_at(f, vec![MOB], Some(MOB));
}

/// [`backstab_lands`] with `hits` and the packet's `target`.
fn backstab_lands_at(f: &mut Frame, hits: Vec<u64>, target: Option<u64>) {
    backstab_lands_after(f, Vec::new(), hits, target);
}

/// [`backstab_lands_at`] behind `before`'s packets, one dispatch.
fn backstab_lands_after(
    f: &mut Frame,
    before: Vec<benilla_protocol::SessionEvent>,
    hits: Vec<u64>,
    target: Option<u64>,
) {
    const BACKSTAB: u32 = 53;
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
    let go = benilla_protocol::SessionEvent::SpellGo {
        caster: ME,
        spell_id: BACKSTAB,
        cast_flags: 0,
        hits,
        misses: vec![],
        target,
        go_target: None,
        target_mask: 0x2,
        source: None,
        dest: None,
        ammo_display_id: None,
        item_caster: None,
        unit_caster: ME,
    };
    crate::net::handlers::dispatch(world, before.into_iter().chain([go]).collect());
}

/// A GO with no hit falls back to the packet's target (the `Deviation:` at `spell_go`), but only
/// one the validator keeps: a friend would drop to the nearest-enemy cycle (`0x6130b5`), which
/// the fallback never runs, so nothing is selected and no swing goes out.
#[test]
fn a_go_with_no_hit_and_a_friendly_target_starts_nothing() {
    let mut f = frame(true);
    with_factions(&mut f, FRIEND, &[]);
    spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
    swinging_at_old(&mut f);
    backstab_lands_at(&mut f, vec![], Some(OLD));
    assert_eq!(attack_wire(&f), Vec::<String>::new());
    assert_eq!(selected(&f), Some(OLD));
    assert!(error_keys(&f).is_empty());
}

/// The GO's deferred start is the player's Attack at its first hit (`0x6e8402` → `0x6131a0`),
/// whose validator (`0x612df0`) selects the hit whatever is selected (`0x613100`), and its
/// `PLAYER_TARGET_CHANGED` (`0x49389c`) runs its listeners before the swing at the selection the
/// validator re-read (`0x6130c1`).
#[test]
fn a_go_deferred_attack_selects_its_hit_before_it_swings() {
    let mut f = frame(true);
    spawn_mob(&mut f, &[]);
    swinging_at_old(&mut f);
    on_target_changed(&mut f, "RandomRoll(1, 100)");
    backstab_lands(&mut f);
    assert_eq!(selected(&f), Some(MOB));
    assert_eq!(
        attack_wire(&f),
        ["SetSelection 0x31", "RandomRoll", "AttackSwing 0x31"]
    );
}

/// A line's listeners run at its packet, before the next packet's handler (`0x537ae5`): a
/// monster's line, then a GO whose deferred attack selects its hit and signals
/// `PLAYER_TARGET_CHANGED` in place (`0x49389c`), are heard in their packets' order.
#[test]
fn a_lines_listeners_run_ahead_of_the_next_packets_in_place_signal() {
    let mut f = frame(true);
    f.app.insert_resource(bevy::prelude::State::new(
        crate::client_state::ClientState::InWorld,
    ));
    spawn_mob(&mut f, &[]);
    swinging_at_old(&mut f);
    run(
        &mut f,
        r#"HEARD = {}
        local r = CreateFrame("Frame")
        r:RegisterEvent("CHAT_MSG_MONSTER_SAY")
        r:RegisterEvent("PLAYER_TARGET_CHANGED")
        r:SetScript("OnEvent", function() table.insert(HEARD, event) end)"#,
    );
    let says = benilla_protocol::SessionEvent::Chat(benilla_protocol::messages::ChatMessage {
        chat_type: benilla_protocol::messages::CHAT_MSG_MONSTER_SAY,
        language: 0,
        sender_guid: 0xF130_0000_0045_0003,
        target_guid: 0,
        sender_name: Some("Boar".into()),
        channel: None,
        text: "a".into(),
        chat_tag: 0,
    });
    backstab_lands_after(&mut f, vec![says], vec![MOB], Some(MOB));
    crate::lanes::fire_in_order(f.app.world_mut());
    assert_eq!(selected(&f), Some(MOB));
    assert_eq!(
        eval::<Vec<String>>(&mut f, "return HEARD"),
        ["CHAT_MSG_MONSTER_SAY", "PLAYER_TARGET_CHANGED"]
    );
}

/// The validator's actor checks run first (`0x612e10`-`0x61305a`): a stunned player's GO shows
/// its line, selects nothing and sends no swing.
#[test]
fn a_stunned_casters_go_shows_its_line_and_does_not_swing() {
    let mut f = frame(true);
    spawn_mob(&mut f, &[]);
    swinging_at_old(&mut f);
    let world = f.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    let mut store = world.get_mut::<ObjectStore>(me).unwrap();
    let flags = store.0.unit_flags();
    store.0.merge(benilla_protocol::ObjectFields::from_pairs(&[(
        FLAGS,
        flags | 0x0004_0000,
    )]));
    backstab_lands(&mut f);
    assert_eq!(error_keys(&f), ["ERR_ATTACK_STUNNED"]);
    assert_eq!(attack_wire(&f), Vec::<String>::new());
    assert_eq!(selected(&f), Some(OLD));
}

/// With the targeting cursor up, the validator's select (`0x613100`) hands the GO's hit to the
/// cursor's cast (`0x4935d5` → `0x6e5b40`), which binds it inside the GO's packet and selects
/// nothing; the validator then re-reads the selection (`0x6130c1`), so the swing goes at the unit
/// selected before, after the cast.
#[test]
fn a_go_deferred_attack_with_the_cursor_up_binds_its_hit_and_swings_at_the_selection() {
    let mut f = frame(true);
    spawn_mob(&mut f, &[]);
    swinging_at_old(&mut f);
    with_the_cursor_up(&mut f);
    backstab_lands(&mut f);
    let sent: Vec<String> =
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::CastSpell { spell_id, target } => {
                    Some(format!("CastSpell {spell_id} {target:x?}"))
                }
                ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
                ClientCommand::AttackSwing { guid } => Some(format!("AttackSwing {guid:#x}")),
                _ => None,
            })
            .collect();
    assert_eq!(
        sent,
        [
            format!("CastSpell {HEAL} Some(31)"),
            "AttackSwing 0x21".into()
        ]
    );
    assert_eq!(selected(&f), Some(OLD));
    assert!(!cursor_is_up(&f));
}

/// The same with nothing selected: the bind leaves the selection empty, so the re-read finds
/// nothing and the validator shows "There is nothing to attack." (`0x6130d9`), with no swing.
#[test]
fn a_go_deferred_attack_the_cursor_took_with_nothing_selected_shows_no_target() {
    let mut f = frame(false);
    spawn_mob(&mut f, &[]);
    swinging_at_old(&mut f);
    with_the_cursor_up(&mut f);
    backstab_lands(&mut f);
    assert_eq!(error_keys(&f), ["ERR_NO_ATTACK_TARGET"]);
    let sent: Vec<_> = f.rx.try_iter().collect();
    assert!(
        sent.iter().any(|c| matches!(
            c,
            ClientCommand::CastSpell {
                spell_id: HEAL,
                target: Some(MOB)
            }
        )),
        "{sent:?}"
    );
    assert!(
        !sent.iter().any(|c| matches!(
            c,
            ClientCommand::AttackSwing { .. } | ClientCommand::SetSelection { .. }
        )),
        "{sent:?}"
    );
    assert_eq!(selected(&f), None);
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

/// `MOB` is named in the name cache, which the getters read at the call off a held unit.
fn with_named_mob(f: &mut Frame) {
    let world = f.app.world_mut();
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(MOB, "Red Affinity".into(), None);
    as_creature(world, MOB);
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
            .resource_mut::<crate::ui_action::ActionSlots>()
            .buttons
            .insert(
                0,
                ActionButton {
                    slot: 0,
                    action: crate::ui_action::SPELL_ATTACK,
                    kind: ACTION_KIND_SPELL,
                },
            );
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
/// packet goes out between the pick's selection and the order, and it reads the Attack button lit
/// through the lent host.
#[test]
fn a_possessed_attacks_latch_signals_its_bar_before_the_order() {
    let attack = benilla_protocol::messages::PET_COMMAND_ATTACK
        | (u32::from(benilla_protocol::messages::PET_ACT_COMMAND) << 24);
    let mut f = frame(true);
    with_factions(&mut f, FRIEND, &[]);
    spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
    with_pet(&mut f, true, attack);
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
}

/// With the targeting cursor up, the pet bar's Attack validator (`0x4bd40d`) hands its kept unit
/// to the cast at its select (`0x613100` → `0x4935d5`), inside the validator, so the cast goes out
/// ahead of the latch's `PET_BAR_UPDATE` (`0x4bd438`), whose listener's packet follows, and the
/// order at the unit last (`0x4bd4a4`).
#[test]
fn a_possessed_attacks_bind_goes_ahead_of_its_bar_update() {
    let attack = benilla_protocol::messages::PET_COMMAND_ATTACK
        | (u32::from(benilla_protocol::messages::PET_ACT_COMMAND) << 24);
    let mut f = frame(true);
    with_factions(&mut f, FOE, &[]);
    with_pet(&mut f, true, attack);
    f.rx.try_iter().for_each(drop);
    run(
        &mut f,
        r#"local r = CreateFrame("Frame")
        r:RegisterEvent("PET_BAR_UPDATE")
        r:SetScript("OnEvent", function() RandomRoll(1, 100) end)"#,
    );
    with_the_cursor_up(&mut f);
    run_pressed(&mut f, "PetAttack()");
    let wire: Vec<String> =
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::CastSpell { spell_id, target } => {
                    Some(format!("CastSpell {spell_id} {target:x?}"))
                }
                ClientCommand::RandomRoll { .. } => Some("RandomRoll".into()),
                ClientCommand::PetAction { target_guid, .. } => {
                    Some(format!("PetAction {target_guid:#x}"))
                }
                ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
                _ => None,
            })
            .collect();
    assert_eq!(
        wire,
        [
            format!("CastSpell {HEAL} Some(21)"),
            "RandomRoll".into(),
            "PetAction 0x21".into()
        ]
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

/// A listener of `CURRENT_SPELL_CAST_CHANGED` that rolls each time it hears it.
fn roll_on_cast_changes(f: &mut Frame) {
    run(
        f,
        r#"local r = CreateFrame("Frame")
        r:RegisterEvent("CURRENT_SPELL_CAST_CHANGED")
        r:SetScript("OnEvent", function() RandomRoll(1, 2) end)"#,
    );
}

/// The send signals once more past its packet (`0x6e589d`), and its listeners run before
/// `TryCast`'s attack tail swings (`0x6e51de` → `0x6131a0`): a roll between the strike's cast and
/// its `CMSG_ATTACKSWING`.
#[test]
fn a_sends_own_signal_listener_sends_ahead_of_its_swing() {
    let mut f = frame(false);
    spawn_mob(&mut f, &[]);
    with_named_mob(&mut f);
    with_strike(&mut f);
    roll_on_cast_changes(&mut f);
    f.rx.try_iter().for_each(drop);
    run(&mut f, "UseAction(1)");
    let wire = pick_wire(&f);
    let cast = wire
        .iter()
        .position(|w| w == "CastSpell 0x31")
        .expect("the strike goes out");
    assert_eq!(
        wire[cast + 1..cast + 3],
        ["RandomRoll", "AttackSwing 0x31"],
        "{wire:?}"
    );
}

/// A unit caster's attempt runs its attack tail (`0x6e51de` → the bar's Attack, `0x4bd630`) before
/// it signals as it returns (`0x6e51e3`): the last roll follows the pet's order.
#[test]
fn a_unit_casters_return_signals_after_its_attack_tail() {
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
                    attributes_ex: 0x200,
                    ..Default::default()
                },
            )]
            .into(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    roll_on_cast_changes(&mut f);
    f.rx.try_iter().for_each(drop);
    run_pressed(&mut f, "CastPetAction(1)");
    let wire = pet_wire(&f);
    assert_eq!(
        wire[wire.len() - 2..],
        ["PetAction 0x31", "RandomRoll"],
        "{wire:?}"
    );
}

/// A queued on-next-swing strike in the in-flight slot, Heroic Strike's.
pub(super) fn queue_strike(f: &mut Frame) {
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

/// The teardown's strike cancel reports its stop inside StopAttack (`0x493a08`), after the pair
/// is zeroed (`0x4939d3`) and before `CMSG_SET_SELECTION` (`0x493a64` for a clear; `0x49388d` for
/// a switch, behind the store `0x4936e2`): its listeners read no target, and their packets go out
/// ahead of the selection's.
#[test]
fn a_teardowns_strike_cancel_listeners_run_ahead_of_the_store_and_the_send() {
    for (lua, wire) in [
        (
            "ClearTarget()",
            vec!["AttackStop", "RandomRoll", "SetSelection 0x0"],
        ),
        (
            r#"TargetUnit("party1")"#,
            vec![
                "AttackStop",
                "RandomRoll",
                "SetSelection 0x22",
                "AttackSwing 0x22",
            ],
        ),
    ] {
        let mut f = frame(true);
        swinging_at_old(&mut f);
        queue_strike(&mut f);
        run(
            &mut f,
            r#"local r = CreateFrame("Frame")
            r:RegisterEvent("SPELLCAST_STOP")
            r:SetScript("OnEvent", function()
                SEEN = UnitExists("target") and "target" or "none"
                RandomRoll(1, 2)
            end)"#,
        );
        run(&mut f, lua);
        assert_eq!(attack_wire(&f), wire, "{lua}");
        assert_eq!(seen(&mut f), "none", "{lua}");
    }
}

/// The release's pet leg (`0x493a0f`-`0x493a18`) runs after its StopAttack (`0x493a08`) returned,
/// so the strike cancel's listeners send ahead of `PetStopAttack`'s packet.
#[test]
fn a_teardowns_strike_cancel_listeners_send_ahead_of_the_pet_stop() {
    let mut f = frame(true);
    with_pet_attacking(&mut f, true);
    swinging_at_old(&mut f);
    queue_strike(&mut f);
    run(
        &mut f,
        r#"local r = CreateFrame("Frame")
        r:RegisterEvent("SPELLCAST_STOP")
        r:SetScript("OnEvent", function() RandomRoll(1, 2) end)"#,
    );
    run(&mut f, "ClearTarget()");
    let wire: Vec<String> =
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::AttackStop => Some("AttackStop".to_string()),
                ClientCommand::PetStopAttack { .. } => Some("PetStopAttack".to_string()),
                ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
                ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
                _ => None,
            })
            .collect();
    assert_eq!(
        wire,
        [
            "AttackStop",
            "RandomRoll",
            "PetStopAttack",
            "SetSelection 0x0"
        ]
    );
}

/// A strike cancel's listener that calls the pet off (`PetStopAttack`, `0x4bd650`) lowers the latch
/// and signals `PET_BAR_UPDATE` (`0x4bd6b4`) there, so the release's pet leg (`0x493a0f`-`0x493a18`)
/// and its change find the latch down and signal nothing more: one `PET_BAR_UPDATE`, the
/// listener's, with a possessed unit or a pet of the bar's own.
#[test]
fn a_strike_cancel_listeners_pet_stop_signals_the_pet_bar_once() {
    for possessed in [false, true] {
        let mut f = frame(true);
        with_pet_attacking(&mut f, possessed);
        swinging_at_old(&mut f);
        queue_strike(&mut f);
        run(
            &mut f,
            r#"BARS = 0
            local r = CreateFrame("Frame")
            r:RegisterEvent("SPELLCAST_STOP")
            r:RegisterEvent("PET_BAR_UPDATE")
            r:SetScript("OnEvent", function()
                if event == "SPELLCAST_STOP" then PetStopAttack() else BARS = BARS + 1 end
            end)"#,
        );
        f.rx.try_iter().for_each(drop);
        run(&mut f, "ClearTarget()");
        assert_eq!(
            eval::<i64>(&mut f, "return BARS"),
            1,
            "possessed {possessed}"
        );
        let stops =
            f.rx.try_iter()
                .filter(|c| matches!(c, ClientCommand::PetStopAttack { .. }))
                .count();
        assert_eq!(stops, 1, "possessed {possessed}");
    }
}

/// StopAttack's Drink tutorial (`0x5ecb54`, for a mana user with no maximum) signals before its
/// `CMSG_ATTACKSTOP` (`0x5ecb5b`), so its listener's packet goes first: in a teardown's stop
/// (`0x493a08`) and in the Attack toggle's (`0x6131ce`).
#[test]
fn a_stop_attacks_tutorial_listener_sends_ahead_of_its_attack_stop() {
    for lua in ["ClearTarget()", "AttackTarget()"] {
        let mut f = frame(true);
        swinging_at_old(&mut f);
        f.app
            .world()
            .resource::<crate::tutorial::Tutorials>()
            .lock()
            .apply_flags(&[0u8; 32]);
        run(
            &mut f,
            r#"local r = CreateFrame("Frame")
            r:RegisterEvent("TUTORIAL_TRIGGER")
            r:SetScript("OnEvent", function() RandomRoll(1, 2) end)"#,
        );
        f.rx.try_iter().for_each(drop);
        run(&mut f, lua);
        let wire: Vec<&str> =
            f.rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::AttackStop => Some("AttackStop"),
                    ClientCommand::RandomRoll { .. } => Some("RandomRoll"),
                    _ => None,
                })
                .collect();
        assert_eq!(wire, ["RandomRoll", "AttackStop"], "{lua}");
    }
}

/// The teardown's StopAttack cancels the slot's strike (`0x6e6f68`), whose listeners run before
/// the save is read and its strike goes on the wire (`0x6e6f6d`, `0x6e6ff3`): Cleave cast over a
/// queued Heroic Strike holds the slot over the save.
#[test]
fn a_teardowns_slot_cancel_listeners_send_ahead_of_the_saves_cancel() {
    const CLEAVE: u32 = 845;
    let mut f = frame(true);
    swinging_at_old(&mut f);
    queue_strike(&mut f);
    f.app
        .world_mut()
        .resource_mut::<crate::spell::QueuedMeleeSpell>()
        .hold(CLEAVE, 78);
    run(
        &mut f,
        r#"local r = CreateFrame("Frame")
        r:RegisterEvent("SPELLCAST_STOP")
        r:SetScript("OnEvent", function() RandomRoll(1, 2) end)"#,
    );
    f.rx.try_iter().for_each(drop);
    run(&mut f, "ClearTarget()");
    let wire: Vec<String> =
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::AttackStop => Some("AttackStop".to_string()),
                ClientCommand::CancelCast { spell_id } => Some(format!("CancelCast {spell_id}")),
                ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
                ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
                _ => None,
            })
            .collect();
    assert_eq!(
        wire,
        [
            "AttackStop",
            "CancelCast 845",
            "RandomRoll",
            "CancelCast 78",
            "SetSelection 0x0"
        ]
    );
}

/// `lua` run with a listener of `event` that selects `party1` the first time it hears it: the
/// pick signals its change itself, nested (`0x493540` → `0x49389c`), and the call's own end tells
/// it no more. The `PLAYER_TARGET_CHANGED`s heard.
fn changes_for_a_pick_at(f: &mut Frame, event: &str, lua: &str) -> i64 {
    run(
        f,
        &format!(
            r#"CHANGES = 0
            local p = CreateFrame("Frame")
            p:RegisterEvent("PLAYER_TARGET_CHANGED")
            p:SetScript("OnEvent", function() CHANGES = CHANGES + 1 end)
            local r = CreateFrame("Frame")
            r:RegisterEvent("{event}")
            r:SetScript("OnEvent", function()
                if not PICKED then PICKED = 1 TargetUnit("party1") end
            end)"#
        ),
    );
    run(f, lua);
    assert_eq!(selected(f), Some(NEW));
    eval::<i64>(f, "return CHANGES")
}

/// A pick at the Attack toggle's StopAttack tutorial (`0x5ecb54`) signals once.
#[test]
fn a_pick_at_the_stops_tutorial_signals_its_target_change_once() {
    let mut f = frame(true);
    swinging_at_old(&mut f);
    f.app
        .world()
        .resource::<crate::tutorial::Tutorials>()
        .lock()
        .apply_flags(&[0u8; 32]);
    assert_eq!(
        changes_for_a_pick_at(&mut f, "TUTORIAL_TRIGGER", "AttackTarget()"),
        1
    );
}

/// A pick at a gossip close core's `GOSSIP_CLOSED` (`0x4931c0` → the core's signal) signals once.
#[test]
fn a_pick_at_a_close_cores_signal_signals_its_target_change_once() {
    let mut f = frame(true);
    crate::ui_session::latch_tests::open(
        &mut f.app,
        crate::ui_session::NpcWindow::Gossip,
        super::systems::NPC,
    );
    assert_eq!(
        changes_for_a_pick_at(&mut f, "GOSSIP_CLOSED", "CloseGossip()"),
        1
    );
}

/// `UnitCreatureFamily("target")` (`0x51a310`) reads the held unit's cached creature record
/// (`0x6055e0`: `[[unit+0xb30]+0x1c]`) through `CreatureFamily.dbc` inside its call, though no
/// snapshot carries the word; a unit whose record the cache lacks answers nil.
#[test]
fn a_creature_family_is_read_off_the_held_units_record() {
    const OBJECT_FIELD_TYPE: u16 = 2;
    const OBJECT_FIELD_ENTRY: u16 = 3;
    const BOAR: u32 = 1_126;
    let mut f = frame(false);
    // A unit object (`TYPEMASK_OBJECT | TYPEMASK_UNIT`) of the boar's template.
    spawn_mob(
        &mut f,
        &[(OBJECT_FIELD_TYPE, 0x9), (OBJECT_FIELD_ENTRY, BOAR)],
    );
    f.app.world_mut().resource_mut::<Selection>().guid = Some(MOB);
    let family =
        |f: &mut Frame| eval::<Option<String>>(f, r#"return UnitCreatureFamily("target")"#);
    assert_eq!(family(&mut f), None, "no record cached yet");

    let world = f.app.world_mut();
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_creature(
            BOAR,
            Some(crate::names::CreatureRecord {
                name: "Stonetusk Boar".into(),
                subname: None,
                creature_type: 1,
                pet_family: 5,
                rank: 0,
                type_flags: 0,
                pet_spell_list: 0,
                civilian: false,
                racial_leader: false,
                display_id: 607,
            }),
        );
    world.insert_resource(crate::ui_pet_stats::PetFamilyTables {
        families: benilla_formats::CreatureFamilies::from_rows([(
            5,
            benilla_formats::CreatureFamily {
                name: "Boar".into(),
                icon: "Interface\\Icons\\Ability_Hunter_Pet_Boar".into(),
                pet_food_mask: 0,
                min_scale: 1.0,
                min_scale_level: 0,
                max_scale: 1.0,
                max_scale_level: 0,
                skill_lines: [0, 0],
            },
        )]),
        foods: Default::default(),
    });
    assert_eq!(family(&mut f).as_deref(), Some("Boar"));
}
