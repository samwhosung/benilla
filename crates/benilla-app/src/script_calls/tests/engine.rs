//! The verbs that change engine state or signal, each through the whole client's host: each does
//! its work inside its call, so its signal's listeners run before the chunk's next verb
//! (`RandomRoll`, `0x48c832`), and the engine holds what the call wrote when the chunk returns.

use benilla_protocol::ObjectFields;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::systems::{client, eval, listen};
use crate::net::{ClientCommand, Guid, GuidIndex, SelfGuid, SelfPlayer};

/// Our own guid in these chunks.
const ME: u64 = 0x10;
/// Another player, followed.
const THEM: u64 = 0x77;

/// What went out, in order, of the kinds these chunks send.
fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|c| {
            Some(match c {
                ClientCommand::RandomRoll { min, .. } => format!("roll {min}"),
                ClientCommand::OpeningCinematic => "opening".into(),
                ClientCommand::ToggleHelm => "togglehelm".into(),
                ClientCommand::Whois { text } => format!("whois {text}"),
                _ => return None,
            })
        })
        .collect()
}

/// `OpeningCinematic()` (`0x48c8c0`) sends its empty `CMSG_OPENING_CINEMATIC` inside the call
/// (`0x48c8f6`), ahead of the chunk's next verb.
#[test]
fn opening_cinematic_sends_inside_its_call() {
    let (mut app, rx) = client();
    eval::<()>(
        &mut app,
        "OpeningCinematic() RandomRoll(1, 2) OpeningCinematic(1)",
    );
    assert_eq!(sent(&rx), ["opening", "roll 1", "opening"]);
    // The opcode the reference pushes at `0x48c8c9`.
    assert_eq!(
        benilla_protocol::messages::opcode::CMSG_OPENING_CINEMATIC,
        0xf9
    );
}

/// The plate bits as the engine holds them: enemy, then friendly.
fn plates(app: &App) -> (bool, bool) {
    let mode = app.world().resource::<crate::vplates::VPlateMode>();
    (mode.enemies, mode.friends)
}

/// The four plate verbs (`0x489450`-`0x489480`) write their bit of `[0xc4da34]` inside the call,
/// in call order, so the engine holds the chunk's last word on each bit as the chunk returns.
#[test]
fn the_plate_verbs_write_their_bits_inside_their_calls() {
    let (mut app, _rx) = client();
    eval::<()>(&mut app, "ShowNameplates() ShowFriendNameplates()");
    assert_eq!(plates(&app), (true, true));
    eval::<()>(
        &mut app,
        "HideNameplates() ShowNameplates() HideFriendNameplates()",
    );
    assert_eq!(plates(&app), (true, false));
}

/// Us, the active mover, and `THEM` beside us, both Human players at full health, `THEM`
/// selected and named.
fn follow_world(app: &mut App) {
    use crate::target::relations::fixture::{factions, player, store_of, HUMAN};
    let alive = |guid| {
        let mut store = store_of(&player(guid, HUMAN));
        store
            .0
            .merge(ObjectFields::from_pairs(&[(22, 100), (28, 100)]));
        store
    };
    let world = app.world_mut();
    world.insert_resource(factions());
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    let me = world
        .spawn((
            Guid(ME),
            SelfPlayer,
            crate::net::ActiveMover,
            Transform::default(),
            GlobalTransform::default(),
            alive(ME),
        ))
        .id();
    let them = world
        .spawn((
            Guid(THEM),
            Transform::from_translation(Vec3::X),
            GlobalTransform::from_translation(Vec3::X),
            alive(THEM),
        ))
        .id();
    let mut index = world.resource_mut::<GuidIndex>();
    index.0.insert(ME, me);
    index.0.insert(THEM, them);
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(THEM, "Them".into(), None);
    let mut selection = world.resource_mut::<crate::target::Selection>();
    selection.guid = Some(THEM);
    selection.target = Some(them);
}

/// `FollowUnit(unit)` (`0x489e00` → `0x60fed0(guid, 3)`) arms the follow and signals
/// `AUTOFOLLOW_BEGIN` (`0x610071`) inside its call, so the listener runs ahead of the chunk's next
/// verb, and the follow is set as the chunk returns; a second follow ends the first through the
/// arm's stop (`0x611130` → `0x60fb60`, `AUTOFOLLOW_END` at `0x60fb84`) before it begins.
#[test]
fn follow_unit_begins_inside_its_call() {
    let (mut app, rx) = client();
    follow_world(&mut app);
    listen(
        &mut app,
        &["AUTOFOLLOW_BEGIN", "AUTOFOLLOW_END"],
        "RandomRoll(9, 10)",
    );
    let seq: String = eval(
        &mut app,
        r#"FollowUnit("target") local seq = SEQ RandomRoll(1, 2) return seq"#,
    );
    assert_eq!(seq, "AUTOFOLLOW_BEGIN ");
    assert_eq!(sent(&rx), ["roll 9", "roll 1"]);
    assert_eq!(
        app.world().resource::<crate::player::FollowState>().guid,
        Some(THEM),
        "the follow is armed as the chunk returns"
    );
    let seq: String = eval(&mut app, r#"FollowByName("Them", 1) return SEQ"#);
    assert_eq!(
        seq, "AUTOFOLLOW_BEGIN AUTOFOLLOW_END AUTOFOLLOW_BEGIN ",
        "the arm ends the follow it replaces first"
    );
}

/// A follow the gate refuses shows its line inside the call (`0x489e8f`, `0x496720`), before the
/// chunk's next verb, and arms nothing.
#[test]
fn a_refused_follow_shows_its_line_inside_its_call() {
    let (mut app, rx) = client();
    follow_world(&mut app);
    *app.world_mut().resource_mut::<crate::target::Selection>() = Default::default();
    eval::<()>(&mut app, r#"ERR_GENERIC_NO_TARGET = "You have no target.""#);
    listen(&mut app, &["UI_ERROR_MESSAGE"], "RandomRoll(9, 10)");
    eval::<()>(&mut app, r#"FollowUnit("target") RandomRoll(1, 2)"#);
    assert_eq!(sent(&rx), ["roll 9", "roll 1"]);
    assert_eq!(
        app.world().resource::<crate::player::FollowState>().guid,
        None
    );
}

/// `ConsoleExec(line)` (`0x515f10`) runs its command through `ConsoleCommandExecute` inside the
/// call (`0x515f43` → `0x63ce00`): a command that sends sends ahead of the chunk's next verb.
#[test]
fn console_exec_runs_its_command_inside_its_call() {
    let (mut app, rx) = client();
    eval::<()>(
        &mut app,
        r#"ConsoleExec("togglehelm") RandomRoll(1, 2) ConsoleExec("whois Bob")"#,
    );
    assert_eq!(sent(&rx), ["togglehelm", "roll 1", "whois Bob"]);
}

/// A console command that writes engine state no script reads runs against the world at the
/// call's next sync point, here the end of the host's lend: `showCull` (`0x673250`) flips its
/// world flag.
#[test]
fn a_console_command_on_the_world_runs_at_the_calls_sync_point() {
    use benilla_world::world_flags::WorldFlags;
    let (mut app, _rx) = client();
    let culled = |app: &App| app.world().resource::<WorldFlags>().has(WorldFlags::CULL);
    let before = culled(&app);
    eval::<()>(&mut app, r#"ConsoleExec("showCull")"#);
    assert_eq!(culled(&app), !before);
}
