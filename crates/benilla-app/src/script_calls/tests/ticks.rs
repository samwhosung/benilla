//! A movement verb's tick inside its call: `SetCommands` (`0x515090`) ends by running the tick
//! (`0x5151a0` → `0x514640`), whose emitters stand the player up (`0x514e0c` → `0x60be30`), clear
//! away (`0x514e23` → `0x5eb830`) and, on a start, stop the auto-move and close the loot
//! (`0x60dc6a` → `0x60e990`), all before the verb returns. So the script's next line reads each,
//! and its send follows theirs on the wire.

use benilla_protocol::field::{FIELD_UNIT_BYTES_1, FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::ObjectFields;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::systems::{client, eval, listen};
use crate::net::{ClientCommand, Embodied, Guid, GuidIndex, ObjectStore, SelfGuid, SelfPlayer};

const ME: u64 = 0x10;

/// Run `chunk` as a key's body runs, the hardware event raised for the movement gate
/// (`0x494a50(0)`), and answer the global `R`.
fn pressed(app: &mut App, chunk: &str) -> Option<i64> {
    crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
        vm.expect("a VM").run_binding(&format!("R = nil {chunk}"))
    })
    .unwrap_or_else(|e| panic!("{chunk}: {e}"));
    eval(app, "return R")
}

/// What went out, in order, of the kinds the tick and the chunks send.
fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|c| {
            Some(match c {
                ClientCommand::RandomRoll { min, .. } => format!("roll {min}"),
                ClientCommand::StandStateChange { state } => format!("stand {state}"),
                ClientCommand::Chat {
                    kind: crate::net::ChatKind::Afk,
                    text,
                    ..
                } => format!("afk '{text}'"),
                _ => return None,
            })
        })
        .collect()
}

/// Our player, a player object (`OBJECT_FIELD_TYPE` 0x19) driving its own body, in control, with
/// `extra` fields on its descriptor.
fn drive(app: &mut App, extra: &[(u16, u32)]) {
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    let mut pairs = vec![
        (2, 0x19),
        (FIELD_UNIT_HEALTH, 100),
        (FIELD_UNIT_MAXHEALTH, 100),
    ];
    pairs.extend_from_slice(extra);
    let me = world
        .spawn((
            Guid(ME),
            SelfPlayer,
            Embodied,
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&pairs)),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(ME, me);
    in_control(world);
}

/// Our player's movement flags.
fn flags(app: &mut App, flags: u32) {
    let world = app.world_mut();
    world.insert_resource(crate::player::Player::with_move_flags(flags));
    world.resource_mut::<crate::player::Player>().active = true;
}

/// The rig and our player in control, as the setup seats them at startup.
fn in_control(world: &mut World) {
    world.init_resource::<crate::player::CameraControl>();
    world.init_resource::<crate::player::FarSight>();
    world.init_resource::<crate::player::Player>();
    world.resource_mut::<crate::player::Player>().active = true;
}

/// Seated and away, `MoveForwardStart()` stands the player and clears away inside its call: the
/// forward emitter's total is nonzero, so `0x60be30(0)` sends `CMSG_STANDSTATECHANGE` (`0x514e0c`)
/// and `ClearAFK(0)` shows its line and sends the empty away message (`0x514e23`), both ahead of the
/// next line's roll, whose listener count reads the line. Main ran both at frame time, after it.
#[test]
fn a_movement_call_stands_and_clears_away_before_its_next_line() {
    let (mut app, rx) = client();
    drive(&mut app, &[(FIELD_UNIT_BYTES_1, 1)]);
    app.world_mut()
        .insert_resource(crate::ui_chat::AfkMirror(1));
    listen(&mut app, &["CHAT_MSG_SYSTEM"], "");
    let shown = pressed(&mut app, "MoveForwardStart() R = N RandomRoll(1, 2)");
    assert_eq!(shown, Some(1), "the away line fired inside the call");
    assert_eq!(sent(&rx), ["stand 0", "afk ''", "roll 1"]);
    assert!(!app.world().resource::<crate::ui_chat::AfkMirror>().is_afk());

    // A turn while forward is held ticks again: the emitter's total is still nonzero, but the
    // player stands and is no longer away, so nothing goes out twice.
    pressed(&mut app, "TurnLeftStart() RandomRoll(1, 2)");
    assert_eq!(sent(&rx), ["roll 1"]);
}

/// A strafe start while following runs `OnMoveInput` (`0x60dcba` → `0x60e990`), whose
/// `StopAutoMove` (`0x60e9b5` → `0x60fb60`) signals `AUTOFOLLOW_END` (`0x60fb84`) and ends the
/// follow inside the call, so the next line reads the signal and no follow. A turn under no mouse
/// button starts a turn (`0x60dd55`); a pitch key starts nothing here. Main ended the follow at the
/// frame's input pass.
#[test]
fn a_movement_start_ends_a_follow_inside_its_call() {
    let (mut app, _rx) = client();
    drive(&mut app, &[]);
    listen(&mut app, &["AUTOFOLLOW_END"], "");
    app.world_mut()
        .resource_mut::<crate::player::FollowState>()
        .start(0x77, "Bob".into());
    let n = pressed(&mut app, "PitchUpStart() R = N");
    assert_eq!(n, Some(0), "no emitter starts");
    let n = pressed(&mut app, "StrafeLeftStart() R = N");
    assert_eq!(n, Some(1), "the next line reads the end");
    let follow = app.world().resource::<crate::player::FollowState>();
    assert!(follow.guid.is_none() && follow.owes_release());
}

/// A key's first `TurnOrActionStart` with a turn key held starts a strafe inside its call: the
/// press sets bit 0x1 ahead of the tick (`0x514840`, then `0x5151a0`), under which the strafe
/// emitter nets the turn bits (`0x514e98`-`0x514eb1`), and the start's `OnMoveInput`
/// (`0x60dcba` → `0x60e990`) ends the follow there.
#[test]
fn a_first_turn_or_action_start_over_a_held_turn_strafes_inside_its_call() {
    let (mut app, _rx) = client();
    drive(&mut app, &[]);
    listen(&mut app, &["AUTOFOLLOW_END"], "");
    pressed(&mut app, "TurnLeftStart()");
    app.world_mut()
        .resource_mut::<crate::player::FollowState>()
        .start(0x77, "Bob".into());
    let n = pressed(&mut app, "N = 0 TurnOrActionStart() R = N");
    assert_eq!(
        n,
        Some(1),
        "the strafe's start ended the follow in the call"
    );
    assert!(looking(&app));
}

/// Falling does not stop the tick: the translate predicate tests `0x1200` (`0x514560`), not the
/// fall's `0x2000`, so a press in the air clears away inside its call (`0x514e23`).
#[test]
fn a_movement_call_in_the_air_clears_away_inside_it() {
    let (mut app, rx) = client();
    drive(&mut app, &[]);
    flags(&mut app, crate::creature_anim::move_flags::FALLING);
    app.world_mut()
        .insert_resource(crate::ui_chat::AfkMirror(1));
    listen(&mut app, &["CHAT_MSG_SYSTEM"], "");
    assert_eq!(pressed(&mut app, "MoveForwardStart() R = N"), Some(1));
    assert_eq!(sent(&rx), ["afk ''"]);
}

/// `ClearAFK`'s line runs its listeners where it is added (`0x5eb878`), before the mirror's zero
/// and the empty AFK packet (`0x5eb885`, `0x5eb8d0`): a listener's roll goes out first.
#[test]
fn an_away_lines_listener_sends_ahead_of_the_afk_packet() {
    let (mut app, rx) = client();
    drive(&mut app, &[]);
    flags(&mut app, crate::creature_anim::move_flags::FALLING);
    app.world_mut()
        .insert_resource(crate::ui_chat::AfkMirror(1));
    listen(&mut app, &["CHAT_MSG_SYSTEM"], "RandomRoll(1, 2)");
    pressed(&mut app, "MoveForwardStart()");
    assert_eq!(sent(&rx), ["roll 1", "afk ''"]);
}

/// A release can start a direction: with W and S held the forward total is 0, and S's release
/// makes it 1, a start (`0x514e6e` → `0x60dc60` → `0x60e990`), so the follow ends inside the
/// call. A swimmer's pitch key starts the pitch emitter (`0x60dda0`), and its release stops it
/// (`0x60ddd0`), each through the same guard (`0x60ddaa`, `0x60ddda`).
#[test]
fn a_start_by_release_or_pitch_ends_a_follow_inside_its_call() {
    let (mut app, _rx) = client();
    drive(&mut app, &[]);
    listen(&mut app, &["AUTOFOLLOW_END"], "");
    let follow = |app: &mut App| {
        app.world_mut()
            .resource_mut::<crate::player::FollowState>()
            .start(0x77, "Bob".into());
    };
    pressed(&mut app, "MoveForwardStart() MoveBackwardStart()");
    follow(&mut app);
    assert_eq!(pressed(&mut app, "N = 0 MoveBackwardStop() R = N"), Some(1));

    pressed(&mut app, "MoveForwardStop()");
    flags(&mut app, crate::creature_anim::move_flags::SWIMMING);
    for (chunk, what) in [
        ("PitchUpStart()", "the start"),
        ("PitchUpStop()", "the stop"),
    ] {
        follow(&mut app);
        let n = pressed(&mut app, &format!("N = 0 {chunk} R = N"));
        assert_eq!(n, Some(1), "{what} ends the follow");
    }
}

/// A unit under the pointer, published as the world's pick publishes it (`0x492890`).
fn hover_a_unit(app: &mut App) {
    let world = app.world_mut();
    let unit = world
        .spawn((
            Guid(0x77),
            Transform::default(),
            ObjectStore(
                ObjectFields::from_pairs(&[
                    (2, 0x9),
                    (FIELD_UNIT_HEALTH, 100),
                    (FIELD_UNIT_MAXHEALTH, 100),
                ])
                .into_created(benilla_protocol::messages::ObjectType::Unit),
            ),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(0x77, unit);
    let mut hovered = world.resource_mut::<crate::target::Hovered>();
    hovered.target = Some(unit);
    hovered.guid = Some(0x77);
    let mut script = world.non_send_resource_mut::<benilla_ui::script::UiScript>();
    script.publish_world_mouseover(Some(0x77));
    script.seat_world_mouseover_unit(0x77, 0);
    script.set_unit(
        "mouseover",
        Some(benilla_ui::script::UnitState {
            exists: true,
            has_object: true,
            guid: 0x77,
            ..Default::default()
        }),
    );
}

/// Whether the camera is in freelook.
fn looking(app: &App) -> bool {
    app.world()
        .resource::<crate::player::CameraControl>()
        .is_looking()
}

/// `MouselookStart` enters freelook inside its call, as the first primary's press does
/// (`0x514231` → `0x514840` → `0x51491f` → `0x50fe30`), and the entry clears the published
/// mouseover (`0x483e80` → `0x483ec3`): the next line's `UnitExists("mouseover")` is nil.
/// `MouselookStop` leaves it there (`0x514c48` → `0x50f940`), and `ToggleMouseMove` flips it
/// (`0x50f910`). The frame's hover driver, after the call, publishes nobody in freelook.
#[test]
fn freelook_enters_and_leaves_inside_the_call() {
    let (mut app, _rx) = client();
    app.world_mut()
        .init_resource::<crate::player::CameraControl>();
    hover_a_unit(&mut app);
    let (before, after): (Option<i64>, Option<i64>) = eval(
        &mut app,
        "local a = UnitExists('mouseover') MouselookStart() return a, UnitExists('mouseover')",
    );
    assert_eq!((before, after), (Some(1), None), "the entry cleared it");
    assert!(looking(&app), "in freelook as the call returns");
    // The frame's hover driver, still holding the pick taken before the call, publishes nobody.
    {
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut()
            .run_system_once(crate::ui_tooltip::drive_mouseover_tooltip)
            .unwrap();
    }
    let after: Option<i64> = eval(&mut app, "return UnitExists('mouseover')");
    assert_eq!(after, None, "the stale pick is not published again");
    eval::<()>(&mut app, "MouselookStop()");
    assert!(!looking(&app), "left inside the stop");
    eval::<()>(&mut app, "ToggleMouseMove()");
    assert!(looking(&app));
    eval::<()>(&mut app, "ToggleMouseMove()");
    assert!(!looking(&app));
}

/// A bound mouse command's press enters freelook inside its call when it sets the first primary
/// bit (`0x51413e`/`0x5141ae` → `0x515090` → `0x514840` → `0x51491f` → `0x50f930`), the left one
/// orbiting: a key's `TurnOrActionStart()` has cleared the mouseover on its next line, and its stop
/// leaves (`0x514c48` → `0x50f940`). A second command's press finds a primary held and enters
/// nothing, so the first one's release leaves nothing while the second holds its bit; a stop that
/// clears no bit leaves nothing either.
#[test]
fn a_bound_mouse_command_enters_and_leaves_freelook_inside_its_call() {
    let (mut app, _rx) = client();
    app.world_mut()
        .init_resource::<crate::player::CameraControl>();
    hover_a_unit(&mut app);
    let cleared = pressed(
        &mut app,
        "local a = UnitExists('mouseover') TurnOrActionStart() \
         R = (a == 1 and UnitExists('mouseover') == nil) and 1 or 0",
    );
    assert_eq!(cleared, Some(1), "the entry cleared the mouseover");
    assert!(looking(&app), "in freelook as the call returns");
    pressed(&mut app, "TurnOrActionStop()");
    assert!(!looking(&app), "left inside the stop");

    pressed(&mut app, "CameraOrSelectOrMoveStart()");
    assert!(looking(&app), "the left command orbits");
    pressed(&mut app, "TurnOrActionStart() CameraOrSelectOrMoveStop()");
    assert!(looking(&app), "the right command's bit holds it");
    pressed(&mut app, "TurnOrActionStop()");
    assert!(!looking(&app));

    // A toggled freelook holds no bit: a stop clears none and leaves nothing (`0x514b7d`).
    pressed(&mut app, "ToggleMouseMove() TurnOrActionStop()");
    assert!(looking(&app), "a stop with no bit held leaves nothing");
    pressed(&mut app, "TurnOrActionStart() TurnOrActionStop()");
    assert!(
        !looking(&app),
        "a press under it makes the release the one that leaves"
    );
}

/// `IsMouselooking` (`0x514270`) tests bit `0x1` of the input control's word (`0x514278`), which a
/// key's `TurnOrActionStart` sets inside its call (`0x51414c` → `0x515090`): the next line reads
/// 1, and a key's stop of the bit a `MouselookStart` set reads nil, in the same frame.
#[test]
fn is_mouselooking_reads_the_bit_a_key_moved_in_the_same_frame() {
    let (mut app, _rx) = client();
    app.world_mut()
        .init_resource::<crate::player::CameraControl>();
    assert_eq!(pressed(&mut app, "R = IsMouselooking()"), None);
    assert_eq!(
        pressed(&mut app, "TurnOrActionStart() R = IsMouselooking()"),
        Some(1)
    );
    assert_eq!(
        pressed(&mut app, "TurnOrActionStop() R = IsMouselooking()"),
        None
    );
    assert_eq!(
        pressed(
            &mut app,
            "MouselookStart() TurnOrActionStop() R = IsMouselooking()"
        ),
        None,
        "the key's stop clears the bit the call set"
    );
}

/// A `WorldFrame` over the screen and a Button `B` (x 100..300, y 200..400) the cursor hovers,
/// logging its scripts to `log`.
fn hover_a_button(app: &mut App) {
    let mut script = app
        .world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>();
    script.set_screen_size(1024.0, 768.0);
    script
        .run(
            r#"log = {}
            WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()
            B = CreateFrame("Button", "B")
            B:SetPoint("BOTTOMLEFT", 100, 200) B:SetWidth(200) B:SetHeight(200) B:EnableMouse(true)
            local function note(what) table.insert(log, what) end
            B:SetScript("OnEnter", function() note("enter") end)
            B:SetScript("OnLeave", function() note("leave") end)
            B:SetScript("OnMouseUp", function() note("up:"..arg1) end)
            B:SetScript("OnClick", function() note("click") end)"#,
        )
        .unwrap();
    script.resolve();
    script.mouse_move(200.0, 300.0);
}

/// Freelook's entry moves the mouse focus to the world frame inside the call (`0x493085` →
/// `0x764dc0`): the hovered frame's `OnLeave` has run when `MouselookStart` returns, and the next
/// line's `GetMouseFocus()` is the `WorldFrame`. The leave has the hover picked again
/// (`0x4930a0`), so the next update gives it back to the frame under the cursor.
#[test]
fn freelook_entry_moves_the_mouse_focus_to_the_world_frame_inside_the_call() {
    let (mut app, _rx) = client();
    app.world_mut()
        .init_resource::<crate::player::CameraControl>();
    hover_a_button(&mut app);
    let (before, log, after): (String, String, String) = eval(
        &mut app,
        "local a = GetMouseFocus():GetName() MouselookStart() \
         return a, table.concat(log, ' '), GetMouseFocus():GetName()",
    );
    assert_eq!(before, "B");
    assert_eq!(
        log, "enter leave",
        "the hovered frame's OnLeave ran inside the call"
    );
    assert_eq!(after, "WorldFrame");
    let focus: String = eval(&mut app, "MouselookStop() return GetMouseFocus():GetName()");
    assert_eq!(focus, "WorldFrame", "the re-pick waits for the update");
    app.world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>()
        .tick(0.0);
    let (focus, log): (String, String) = eval(
        &mut app,
        "return GetMouseFocus():GetName(), table.concat(log, ' ')",
    );
    assert_eq!((focus.as_str(), log.as_str()), ("B", "enter leave enter"));
}

/// A key's `TurnOrActionStart` entering freelook while a frame holds the left button releases it
/// inside the call (`0x764e98`): its `OnMouseUp` runs once, with the press's button, at the zeroed
/// point (`0x764e87`, `0x764e8e`), which is outside the button, so no click; the capture goes
/// (`0x764e9b`), so the real release reaches no frame.
#[test]
fn freelook_entry_releases_a_held_frame_once_at_the_origin() {
    let (mut app, _rx) = client();
    app.world_mut()
        .init_resource::<crate::player::CameraControl>();
    hover_a_button(&mut app);
    let click = |app: &mut App, down: bool| {
        app.world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>()
            .mouse_button(200.0, 300.0, "LeftButton", down);
    };
    click(&mut app, true);
    pressed(&mut app, "TurnOrActionStart()");
    assert!(looking(&app));
    let log: String = eval(&mut app, "return table.concat(log, ' ')");
    assert_eq!(log, "enter leave up:LeftButton", "released inside the call");
    click(&mut app, false);
    let log: String = eval(&mut app, "return table.concat(log, ' ')");
    assert_eq!(
        log, "enter leave up:LeftButton",
        "the real release reaches no frame"
    );
}

/// Freelook's entry turns every plate's mouse off inside the call (`0x483e9a` → `0x60f830`), and
/// its leave turns it back on (`0x50fdd0` → `0x483e70`): a plate's `IsMouseEnabled()` answers the
/// change on the script's next line.
#[test]
fn freelook_turns_the_plates_mouse_off_and_back_inside_the_call() {
    use benilla_ui::script::{PlateGeometry, PlateState};
    let (mut app, _rx) = client();
    app.world_mut()
        .init_resource::<crate::player::CameraControl>();
    {
        let mut script = app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>();
        script.set_screen_size(1024.0, 768.0);
        script
            .run(
                r#"NAMEPLATE_FONT = "Fonts\\FRIZQT__.TTF"
                WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()"#,
            )
            .unwrap();
        script.resolve();
        let geometry = PlateGeometry {
            width: 128.0,
            height: 32.0,
            bar_off_x: 4.0,
            bar_off_y: 4.0,
            bar_width: 103.0,
            bar_height: 9.0,
            level_off_x: 11.8,
            level_off_y: 9.1,
            skull_size: 12.8,
            raid_size: 25.6,
            name_height: 12.8,
            level_height: 11.0,
            shadow_offset: 1.0,
        };
        let wolf = PlateState {
            key: 0x77,
            top_centre: (500.0, 400.0),
            health: 30.0,
            max_health: 40.0,
            bar_colour: [1.0, 0.0, 0.0],
            name: "Wolf".into(),
            level: Some(12),
            skull: false,
            level_colour: [1.0, 1.0, 0.0],
            raid_icon: None,
            alpha: 255,
            lit: false,
            hovered: false,
        };
        script.sync_nameplates(geometry, &[wolf]);
        script.resolve();
    }
    let (before, after): (Option<i64>, Option<i64>) = eval(
        &mut app,
        "local p = WorldFrame:GetChildren() local a = p:IsMouseEnabled() MouselookStart() \
         return a, p:IsMouseEnabled()",
    );
    assert_eq!((before, after), (Some(1), None), "off inside the entry");
    let back: Option<i64> = eval(
        &mut app,
        "MouselookStop() return WorldFrame:GetChildren():IsMouseEnabled()",
    );
    assert_eq!(back, Some(1), "back on inside the leave");
}

/// `MouselookStart` and `TurnOrActionStart` set one bit (`0x515090(1, …)`), so either's stop clears
/// it and leaves freelook inside its call (`0x514b70` → `0x514c48`): a right drag's
/// `TurnOrActionStart()`, as `BUTTON2`'s body runs it, ends at a `MouselookStop()`, and a
/// `MouselookStart()` at a key's `TurnOrActionStop()`.
#[test]
fn either_call_on_the_turn_or_action_bit_ends_the_others_freelook() {
    let (mut app, _rx) = client();
    app.world_mut()
        .init_resource::<crate::player::CameraControl>();
    pressed(&mut app, "TurnOrActionStart()");
    assert!(looking(&app), "the drag entered");
    let held: Option<i64> = eval(&mut app, "MouselookStop() return IsMouselooking()");
    assert_eq!(held, None, "the stop cleared the bit");
    assert!(!looking(&app), "and left inside its call");

    eval::<()>(&mut app, "MouselookStart()");
    assert!(looking(&app));
    pressed(&mut app, "TurnOrActionStop()");
    assert!(!looking(&app), "the key's stop cleared the mouselook's bit");
}

/// The pitch emitter runs against its latch, behind the TurnOrAction bit's gate (`0x515019`): a
/// pitch key held under the bit starts nothing, and the release of the bit, `TurnOrActionStop` or
/// `MouselookStop`, lets it start one in the release's tick (`0x515054` → `0x60ddaa` →
/// `0x60e990`), so the follow ends inside that call.
#[test]
fn a_release_that_lets_a_held_pitch_start_ends_a_follow_inside_its_call() {
    for (start, stop) in [
        ("TurnOrActionStart()", "TurnOrActionStop()"),
        ("MouselookStart()", "MouselookStop()"),
    ] {
        let (mut app, _rx) = client();
        drive(&mut app, &[]);
        flags(&mut app, crate::creature_anim::move_flags::SWIMMING);
        listen(&mut app, &["AUTOFOLLOW_END"], "");
        app.world_mut()
            .resource_mut::<crate::player::FollowState>()
            .start(0x77, "Bob".into());
        let n = pressed(&mut app, &format!("N = 0 {start} PitchUpStart() R = N"));
        assert_eq!(n, Some(0), "{start}: the bit holds the pitch back");
        let n = pressed(&mut app, &format!("N = 0 {stop} R = N"));
        assert_eq!(n, Some(1), "{stop}: the release's tick starts the pitch");
    }
}
