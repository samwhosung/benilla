//! A nameplate's click (`CGNamePlateFrame::OnClick 0x7cb910`) runs `ClickObjectByAction 0x4949f0`
//! on the plate's unit inside the click, ahead of the button's own click (`0x7cb92a`, then
//! `0x779540` at `0x7cb936`): the left button's `SetSelection` (`0x49280c` → `0x493540`) and the
//! right's, with the unit's interact after it (`0x492870`, `0x492881`), signal in place, so the
//! line after `Click()` reads what they did, a mouse press or a script's `Click()` alike.

use super::*;

/// A unit that was drawn a plate and left before its click.
const GONE: u64 = 0x23;

/// A plate on each unit of `keys`, the world frame's children in that order.
fn plates(f: &mut Frame, keys: &[u64]) {
    let mut script = f.app.world_mut().non_send_resource_mut::<UiScript>();
    crate::vplates::plates_for_test(&mut script, keys);
}

/// Record `PLAYER_TARGET_CHANGED` on `EVENTS`.
fn record_target_changes(f: &mut Frame) {
    run(
        f,
        r#"EVENTS = {}
        local r = CreateFrame("Frame")
        r:RegisterEvent("PLAYER_TARGET_CHANGED")
        r:SetScript("OnEvent", function() table.insert(EVENTS, event) end)"#,
    );
}

/// The selections sent since the last read, in order.
fn selections(f: &Frame) -> Vec<u64> {
    f.rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::SetSelection { guid } => Some(guid),
            _ => None,
        })
        .collect()
}

/// pfUI's click-through calls `plate:Click()` (`nameplates.lua:1228`), the slot a mouse release
/// runs (`0x7826c0` → slot `0x94`, `0x7cb910`): the select commits before `Click` returns.
#[test]
fn a_plates_click_selects_its_unit_before_the_chunks_next_line() {
    let mut f = frame(true);
    let world = f.app.world_mut();
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(NEW, "Red Whelp".into(), None);
    as_player(world, NEW);
    plates(&mut f, &[NEW]);
    let seen = eval::<Option<String>>(
        &mut f,
        "WorldFrame:GetChildren():Click() return UnitName('target')",
    );
    assert_eq!(seen.as_deref(), Some("Red Whelp"));
    assert_eq!(selected(&f), Some(NEW));
    assert_eq!(selections(&f), vec![NEW], "one select, sent in the click");
}

/// The select's `PLAYER_TARGET_CHANGED` runs its listeners inside the click, and ahead of the
/// plate's `OnClick`: `0x7cb92a` calls `0x4949f0` before `0x7cb936` runs the button's click.
#[test]
fn a_plates_select_signals_inside_its_click_ahead_of_its_on_click() {
    let mut f = frame(true);
    plates(&mut f, &[NEW]);
    record_target_changes(&mut f);
    run(
        &mut f,
        r#"WorldFrame:GetChildren():SetScript("OnClick", function()
            table.insert(EVENTS, "OnClick")
        end)"#,
    );
    let events = eval::<Vec<String>>(
        &mut f,
        r#"WorldFrame:GetChildren():Click() table.insert(EVENTS, "returned") return EVENTS"#,
    );
    assert_eq!(events, ["PLAYER_TARGET_CHANGED", "OnClick", "returned"]);
}

/// `0x4925d0` looks the plate's guid up (`0x492619`) and returns when no object answers: a unit
/// that left after its plate was drawn is neither greeted nor selected, and the next plate's click
/// in the chunk selects as ever.
#[test]
fn a_plate_whose_unit_left_clicks_nothing_and_the_next_plate_selects() {
    let mut f = frame(true);
    let gone = spawn_gone(&mut f);
    plates(&mut f, &[GONE, NEW]);
    // The unit leaves before the driver's next sync retires its plate.
    let world = f.app.world_mut();
    world.resource_mut::<GuidIndex>().0.remove(&GONE);
    world.despawn(gone);
    record_target_changes(&mut f);
    world_greetings(&mut f);
    let (after_gone, events) = eval::<(i64, Vec<String>)>(
        &mut f,
        r#"EVENTS = {}
        local gone, new = WorldFrame:GetChildren()
        gone:Click()
        local n = table.getn(EVENTS)
        new:Click()
        return n, EVENTS"#,
    );
    assert_eq!(after_gone, 0, "the gone unit's plate selects nothing");
    assert_eq!(events, ["PLAYER_TARGET_CHANGED"]);
    assert_eq!(selections(&f), vec![NEW]);
    assert_eq!(selected(&f), Some(NEW));
    let greeted = world_greetings(&mut f);
    let new = f.app.world().resource::<GuidIndex>().0[&NEW];
    assert_eq!(greeted, vec![new], "only the unit still there is greeted");
}

/// [`GONE`], 4 yd off, alive.
fn spawn_gone(f: &mut Frame) -> Entity {
    let world = f.app.world_mut();
    let gone = world
        .spawn((
            Guid(GONE),
            Transform::from_xyz(4.0, 0.0, 0.0),
            GlobalTransform::from_translation(Vec3::new(4.0, 0.0, 0.0)),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (HEALTH, 100),
                (28, 100),
            ])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(GONE, gone);
    gone
}

/// The driver's sync to the units of `keys`: a plate whose unit is not among them retires.
fn sync_plates(f: &mut Frame, keys: &[u64]) {
    let mut script = f.app.world_mut().non_send_resource_mut::<UiScript>();
    crate::vplates::sync_plates_for_test(&mut script, keys);
}

/// `NEW` named "Red Whelp" and `MOB` "Red Affinity", for the chunk's `UnitName("target")`.
fn name_new_and_mob(f: &mut Frame) {
    spawn_mob(f, &[]);
    let world = f.app.world_mut();
    let mut names = world.resource_mut::<crate::names::NameCache>();
    names.insert_player(NEW, "Red Whelp".into(), None);
    names.insert_player(MOB, "Red Affinity".into(), None);
    as_player(world, NEW);
    as_creature(world, MOB);
}

/// The retire (`0x608a10`) hides a plate and returns it to the pool without writing its guid
/// (`+0x4e8`, written only by the constructor `0x7cb2a6` and the bind `0x7cb6e0`), so the click
/// slot still hands `0x4949f0` its last unit (`0x7cb91c`, `0x7cb92a`): a script's `Click()` on the
/// hidden plate selects the unit that walked out of plate range.
#[test]
fn a_retired_plates_click_selects_its_last_unit() {
    let mut f = frame(true);
    name_new_and_mob(&mut f);
    plates(&mut f, &[NEW]);
    sync_plates(&mut f, &[]);
    let (shown, seen) = eval::<(bool, Option<String>)>(
        &mut f,
        "local p = WorldFrame:GetChildren() p:Click() return p:IsShown() == 1, UnitName('target')",
    );
    assert!(!shown, "the plate is retired");
    assert_eq!(seen.as_deref(), Some("Red Whelp"));
    assert_eq!(selections(&f), vec![NEW]);
}

/// A retired plate bound again to another unit carries the new unit's guid from its bind
/// (`0x7cb6e0`): its click selects the new unit, where the hidden plate's selected the last.
#[test]
fn a_rebound_plates_click_selects_its_new_unit() {
    let mut f = frame(true);
    name_new_and_mob(&mut f);
    plates(&mut f, &[NEW]);
    sync_plates(&mut f, &[]);
    let hidden = eval::<Option<String>>(
        &mut f,
        "WorldFrame:GetChildren():Click() return UnitName('target')",
    );
    assert_eq!(
        hidden.as_deref(),
        Some("Red Whelp"),
        "the retired plate's last unit"
    );
    sync_plates(&mut f, &[MOB]);
    let (count, seen) = eval::<(i64, Option<String>)>(
        &mut f,
        "WorldFrame:GetChildren():Click() return WorldFrame:GetNumChildren(), UnitName('target')",
    );
    assert_eq!(count, 1, "the same plate, bound again");
    assert_eq!(seen.as_deref(), Some("Red Affinity"));
    assert_eq!(selections(&f), vec![NEW, MOB]);
}

/// A retired plate's last unit gone from the object manager: `0x4925d0`'s lookup returns
/// (`0x492623`), so its click selects nothing, while another retired plate's selects its own.
#[test]
fn a_retired_plate_whose_unit_is_gone_clicks_nothing() {
    let mut f = frame(true);
    let gone = spawn_gone(&mut f);
    plates(&mut f, &[GONE, NEW]);
    sync_plates(&mut f, &[]);
    let world = f.app.world_mut();
    world.resource_mut::<GuidIndex>().0.remove(&GONE);
    world.despawn(gone);
    record_target_changes(&mut f);
    let (after_gone, events) = eval::<(i64, Vec<String>)>(
        &mut f,
        r#"local gone, new = WorldFrame:GetChildren()
        gone:Click()
        local n = table.getn(EVENTS)
        new:Click()
        return n, EVENTS"#,
    );
    assert_eq!(after_gone, 0, "the gone unit's plate selects nothing");
    assert_eq!(events, ["PLAYER_TARGET_CHANGED"]);
    assert_eq!(selections(&f), vec![NEW]);
}

/// The greetings asked since the last read.
fn world_greetings(f: &mut Frame) -> Vec<Entity> {
    f.app
        .world_mut()
        .resource_mut::<Messages<crate::sound::NpcGreetingRequest>>()
        .drain()
        .map(|g| g.npc)
        .collect()
}

/// The wire's marks of a right-click's steps, in the order they went out.
fn steps(f: &Frame) -> Vec<&'static str> {
    f.rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::CastSpell {
                target: Some(_), ..
            } => Some("cast"),
            ClientCommand::SetSelection { .. } => Some("select"),
            ClientCommand::AttackSwing { .. } => Some("swing"),
            _ => None,
        })
        .collect()
}

/// A script's right-click on a hostile unit's plate (`0x4949f0` action 4 → `0x492820`): the
/// `SelectTarget` (`0x492870`), then the unit's interact (`0x492881`), whose attack leg selects
/// through StartAttack (`0x5eccaa`) and swings, all inside the click. With the targeting cursor up
/// the select binds the cast to the unit and returns (`0x4935d5`), and the interact runs after the
/// bind has emptied the cursor's word, so it selects and swings after the cast.
#[test]
fn a_plates_right_click_interacts_inside_the_click_with_and_without_the_cursor() {
    for (cursor, want) in [
        (false, vec!["select", "swing"]),
        (true, vec!["cast", "select", "swing"]),
    ] {
        let mut f = frame(true);
        spawn_mob(&mut f, &[]);
        f.app.world_mut().init_resource::<crate::player::Player>();
        if cursor {
            f.app
                .world_mut()
                .resource_mut::<crate::spell::SpellTargeting>()
                .enter(HEAL, crate::spell::CastCommit::Spell, 0x0002);
        }
        plates(&mut f, &[MOB]);
        // A key's body, an input event, so the bind's tag 2 passes (`0x6e5ba5`).
        run_pressed(
            &mut f,
            r#"WorldFrame:GetChildren():Click("RightButton") SEEN = SpellIsTargeting()"#,
        );
        let what = format!("cursor up {cursor}");
        assert_eq!(steps(&f), want, "{what}");
        assert_eq!(selected(&f), Some(MOB), "{what}");
        let targeting = eval::<Option<bool>>(&mut f, "return SEEN");
        assert_ne!(
            targeting,
            Some(true),
            "{what}: the cursor is down by the line after"
        );
    }
}

/// Attacking the old target, a plate's select stops and re-swings at the new unit (`0x493a08`,
/// `0x4938c8` → `Attack 0x5ecb70`) when Attack's own legs pass on it: alive and `CanAttack`
/// (`0x5ecc2e`). The press and its cursor play no part, so a press under a cursor that is not
/// Attack re-swings at a hostile unit, a script's `Click()` does too, and a press under the Attack
/// cursor on a friendly unit only stops.
#[test]
fn a_plate_select_mid_combat_reswings_by_the_units_own_verdict() {
    use crate::target::cursor_mode::CursorKind;
    // (a mouse press, the cursor the press showed, the new unit's template, the swings sent)
    for (pressed, kind, template, want) in [
        (true, CursorKind::Speak, FOE, vec![OLD, MOB]),
        (false, CursorKind::Attack, FOE, vec![OLD, MOB]),
        (true, CursorKind::Attack, FRIEND, vec![OLD]),
    ] {
        let mut f = frame(true);
        // Creatures (`OBJECT_FIELD_TYPE` 0x9), which the latch (`0x60ecb0`) and Attack look up.
        with_factions(&mut f, FOE, &[(2, 0x9)]);
        spawn_mob(&mut f, &[(2, 0x9), (TEMPLATE, template)]);
        plates(&mut f, &[MOB]);
        let world = f.app.world_mut();
        let mob = world.resource::<GuidIndex>().0[&MOB];
        // The press the walk latched on the plate's unit (`world_steps`), as the cursor classed it.
        *world.resource_mut::<crate::target::PressPick>() = crate::target::PressPick {
            hovered: crate::target::Hovered {
                target: Some(mob),
                guid: Some(MOB),
                distance: 3.0,
                ..Default::default()
            },
            cursor: crate::target::cursor_mode::WorldCursor {
                kind,
                unable: false,
            },
            ..Default::default()
        };
        with_hosted_vm(world, |s| {
            let s = s.expect("a VM");
            s.run("AttackTarget()").expect("the attack runs");
            if pressed {
                s.mouse_move(500.0, 384.0);
                s.mouse_button(500.0, 384.0, "LeftButton", true);
                s.mouse_button(500.0, 384.0, "LeftButton", false);
            } else {
                s.run("WorldFrame:GetChildren():Click()")
                    .expect("the click runs");
            }
        });
        let what = format!("pressed {pressed}, cursor {kind:?}, template {template}");
        assert_eq!(swings(&f), (want, true), "{what}");
        assert_eq!(selected(&f), Some(MOB), "{what}");
    }
}
