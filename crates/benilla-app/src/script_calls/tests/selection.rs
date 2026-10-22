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

/// The bar's attack latch up on `PET`; `possessed` makes it `0x5ee5a0`'s answer.
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
    let mut bar = world.resource_mut::<crate::ui_pet::PetBar>();
    bar.spells.pet_guid = PET;
    bar.attacking = true;
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
    // A change no call made is the feeds' to signal.
    f.app.world_mut().resource_mut::<Selection>().guid = Some(NEW);
    feeds(&mut f);
    assert_eq!(
        events(&mut f),
        ["SPELL_UPDATE_USABLE", "PLAYER_TARGET_CHANGED"]
    );
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
    assert!(!f.app.world().resource::<crate::ui_pet::PetBar>().attacking);
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
    assert!(f.app.world().resource::<crate::ui_pet::PetBar>().attacking);
}
