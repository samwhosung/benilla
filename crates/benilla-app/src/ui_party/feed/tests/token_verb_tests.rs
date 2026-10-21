//! `SetRaidTarget` and `InviteToParty` resolve any unit token, as `UninviteFromParty` does.

use super::*;

/// A creature, and a player outside the group.
const CREATURE: u64 = 0xF130_0000_0000_0031;
const STRANGER: u64 = 0x77;

/// The marks among the commands sent, `(icon, guid)`.
fn marks(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<(u8, u64)> {
    rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::SetRaidTarget { icon, guid } => Some((icon, guid)),
            _ => None,
        })
        .collect()
}

/// The invites among the commands sent, by name.
fn invites(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::GroupInvite { name } => Some(name),
            _ => None,
        })
        .collect()
}

/// The resolver's guids: us, `mouseover` and `target` on `at`, held, and the party slots.
fn resolve(app: &mut App, at: u64, party: [u64; 4]) {
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_unit_guids(&benilla_ui::script::UnitGuids {
            player: ME,
            target: at,
            mouseover: at,
            party,
            held: [(ME, 0), (at, 0)].into(),
            ..Default::default()
        });
    // The app's selection too, which no verb here reads.
    let world = app.world_mut();
    world.init_resource::<crate::target::Selection>();
    world.resource_mut::<crate::target::Selection>().guid = Some(at);
}

fn lua(app: &mut App, code: &str) {
    app.world_mut()
        .non_send_resource::<UiScript>()
        .run(code)
        .unwrap();
    frame(app);
}

fn errors(app: &App) -> Vec<crate::ui_action::UiError> {
    app.world()
        .resource::<crate::ui_action::UiErrorKeys>()
        .take()
}

/// `SetRaidTarget` turns any token into its guid (`0x515970` at `0x4bbeff`) and marks a unit that
/// is no player once the player's object and the unit's are held (`0x4bbf85`, `0x4bbfa4`,
/// `0x4bbfc1`), with `MSG_RAID_TARGET_UPDATE`'s slot the index less one (`0x4bbf29`).
#[test]
fn set_raid_target_marks_any_unit_the_token_names() {
    let (mut app, rx) = drain_app();
    app.init_resource::<crate::ui_action::UiErrorKeys>();
    seat_self(&mut app);
    stream(&mut app, CREATURE, &[(2, 0x9), (22, 100), (28, 100)]);
    resolve(&mut app, CREATURE, [0; 4]);
    frame(&mut app);
    let _ = marks(&rx);
    lua(&mut app, r#"SetRaidTarget("mouseover", 8)"#);
    assert_eq!(marks(&rx), [(7, CREATURE)]);
    assert!(errors(&app).is_empty());
}

/// A mark needs a raid or party member or pet (`0x4bafd0`, `0x4e8fa0`) or a held unit
/// (`0x4bbfa9`-`0x4bbfb4`): a guid with neither shows `ERR_INVALID_RAID_TARGET` (`0x4bbfd2`) and
/// sends nothing, while a party member out of range is marked.
#[test]
fn set_raid_target_refuses_a_guid_no_roster_or_object_holds() {
    let (mut app, rx) = drain_app();
    app.init_resource::<crate::ui_action::UiErrorKeys>();
    seat_self(&mut app);
    party(&mut app, 1, true);
    resolve(&mut app, STRANGER, [member(1), 0, 0, 0]);
    frame(&mut app);
    let _ = marks(&rx);
    lua(&mut app, r#"SetRaidTarget("target", 1)"#);
    assert_eq!(marks(&rx), []);
    assert_eq!(
        errors(&app),
        [crate::ui_action::UiError::key("ERR_INVALID_RAID_TARGET")]
    );
    lua(&mut app, r#"SetRaidTarget("party1", 2)"#);
    assert_eq!(marks(&rx), [(1, member(1))]);
}

/// An index whose low byte is not 1 to 8 clears: the slot the unit holds (`0x4bb190` at
/// `0x4bbf36`) is sent with guid 0, and a unit holding none sends nothing (`0x4bbfed`).
#[test]
fn set_raid_target_past_eight_clears_the_units_icon() {
    let (mut app, rx) = drain_app();
    app.init_resource::<crate::ui_action::UiErrorKeys>();
    seat_self(&mut app);
    stream(&mut app, CREATURE, &[(2, 0x9), (22, 100), (28, 100)]);
    resolve(&mut app, CREATURE, [0; 4]);
    app.world_mut()
        .resource_mut::<GroupState>()
        .apply_raid_target(3, CREATURE);
    frame(&mut app);
    let _ = marks(&rx);
    lua(&mut app, r#"SetRaidTarget("target", 9)"#);
    assert_eq!(marks(&rx), [(3, 0)], "the unit's own slot, cleared");
    app.world_mut()
        .resource_mut::<GroupState>()
        .apply_raid_target(3, 0);
    lua(&mut app, r#"SetRaidTarget("target", 0)"#);
    assert_eq!(marks(&rx), [], "no icon to clear");
}

/// `InviteToParty` turns any token into its guid (`0x515970` at `0x48a3ed`), and `InviteByGuid`
/// (`0x5e9440`) invites a player the object manager holds (`0x5e945d`) by its `GetUnitName`
/// (`0x609210`); a guid with no player object, a party member out of range, is not invited.
#[test]
fn invite_to_party_invites_a_held_player_any_token_names() {
    let (mut app, rx) = drain_app();
    seat_self(&mut app);
    party(&mut app, 1, true);
    stream(&mut app, STRANGER, &PLAYER_TYPE);
    cache_names(&mut app, &[(STRANGER, "Zed"), (member(1), "M1")]);
    resolve(&mut app, STRANGER, [member(1), 0, 0, 0]);
    frame(&mut app);
    let _ = invites(&rx);
    lua(&mut app, r#"InviteToParty("mouseover")"#);
    assert_eq!(invites(&rx), ["Zed"]);
    lua(&mut app, r#"InviteToParty("party1")"#);
    assert_eq!(invites(&rx), Vec::<String>::new(), "no object, no invite");
}
