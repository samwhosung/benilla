//! The watchers on a unit's fields run in the order they were linked onto its list (`0x467f00`,
//! `0x467f65`), on the whole client headless: the unit's own, linked while its token flags
//! `+0xc90` are nonzero (`0x51bbb0`), and the unit tooltip's, linked by each build that shows it
//! (`0x52a0a8`). A `UNIT_HEALTH` listener reads the tooltip's bar where it runs.

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::{EntityKind, SessionEvent};
use bevy::prelude::*;

use super::unit_view_tests::{client, created, frame, health, values, wire_packets};

/// The client's own guid.
const ME: u64 = 0x7;
/// The target [`client`] seats.
const WOLF: u64 = 0xF130_0000_0044_0001;
/// `UNIT_FIELD_SUMMON`, a unit's summoned pet.
const SUMMON: u16 = 8;

/// `lua` with the host lent, as a key's body or a `/script` line runs it.
fn hosted(app: &mut App, lua: &str) {
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| s.expect("a VM").run(lua))
        .expect("the script runs");
}

/// A unit tooltip `Tip` with its `TipStatusBar`, and the spy's `Look` reading the bar: the
/// listener's view of the tooltip as it fires. A VM the client already spies on keeps its spy;
/// a new one gets it.
fn tip(app: &mut App) {
    crate::ui_script::test_ui::load_tooltip_template(
        app.world()
            .non_send_resource::<benilla_ui::script::UiScript>(),
    );
    crate::script_calls::eval_in_vm::<()>(
        app.world(),
        r#"
        UIParent = UIParent or CreateFrame("Frame", "UIParent")
        local tip = CreateFrame("GameTooltip", "Tip", nil, "TestTooltipTemplate")
        local bar = CreateFrame("StatusBar", "TipStatusBar", tip)
        bar:SetWidth(100); bar:SetHeight(8)
        function Look(event, unit) return TipStatusBar:GetValue() end
        if not Seen then
            Seen = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_HEALTH")
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. arg1 .. ":" .. tostring(Look(event, arg1)))
            end)
        end
        "#,
    );
}

/// [`frame`] after a reload, which loads the stock files where the install is: this client's
/// world does not feed them whole, so their own errors are left out; any other fails the test.
fn reloaded_frame(app: &mut App, packets: Vec<SessionEvent>) -> Vec<String> {
    let world = app.world_mut();
    crate::net::handlers::dispatch(
        world,
        wire_packets(packets.into_iter().map(|p| vec![p]).collect()),
    );
    crate::lanes::fire_in_order(world);
    let mut s = world.non_send_resource_mut::<benilla_ui::script::UiScript>();
    let out = s.eval::<Vec<String>>("return Seen").unwrap();
    s.run("Seen = {}").unwrap();
    let errors: Vec<String> = s
        .take_errors()
        .into_iter()
        .filter(|e| !e.contains("Interface\\FrameXML\\"))
        .collect();
    assert!(errors.is_empty(), "script errors: {errors:?}");
    out
}

/// The tooltip shows `token`'s unit, built with the host lent.
fn show(app: &mut App, token: &str) {
    hosted(
        app,
        &format!(r#"Tip:SetOwner(UIParent, "ANCHOR_NONE") Tip:SetUnit("{token}")"#),
    );
}

/// The tooltip's watcher runs at its link among the unit's own, inside the packet that moves the
/// health: built after the target's store linked the unit-field watchers (`0x49372a` →
/// `0x60c4a0`), it runs behind them, so the `UNIT_HEALTH("target")` listener reads the bar's old
/// value; a deselect unlinks them (`0x4939cc` → `0x60c50c`) and a reselect links them again at the
/// tail, behind the tooltip's, and the listener reads the packet's.
#[test]
fn the_tooltips_watcher_runs_at_its_link_among_the_units_own() {
    let mut app = client();
    tip(&mut app);
    show(&mut app, "target");
    assert_eq!(
        frame(&mut app, vec![values(WOLF, &health(70))]),
        ["UNIT_HEALTH:target:100"],
        "the target's watchers first"
    );
    hosted(&mut app, "ClearTarget()");
    hosted(&mut app, "TargetLastTarget()");
    assert_eq!(
        frame(&mut app, vec![values(WOLF, &health(60))]),
        ["UNIT_HEALTH:target:60"],
        "the tooltip's first"
    );
}

/// A `/reload` keeps the units' token flags and their unit-field watchers' links, which live on
/// the unit objects: its one token write is its deselect's (`0x490bdf` → `0x4939cc`), the group
/// writers run only in the group packets' handlers, and it destroys the tooltip, whose destructor
/// unlinks its watcher (`0x490c9b`, `0x529480` → `0x5294d5`). So a party pet keeps its watchers
/// ahead of a tooltip the new interface builds; a target and a deselect add and remove a flag
/// beside the one it holds and move nothing; and a reseat of the party, the list's clear then
/// its seat (`0x4e8530`, `0x4e83d6`), links them again behind the tooltip's.
#[test]
fn a_reload_keeps_a_units_token_flags_and_its_watchers_place() {
    use benilla_protocol::messages::{member_status, GroupMemberEntry};
    const MATE: u64 = 0x22;
    const PET: u64 = 0xF140_0000_0046_0003;
    let list = || SessionEvent::GroupList {
        group_type: 0,
        own_flags: 0,
        members: vec![GroupMemberEntry {
            name: String::new(),
            guid: MATE,
            status: member_status::ONLINE,
            flags: 0,
        }],
        leader: ME,
        loot: None,
    };
    let mut app = client();
    let _ = frame(
        &mut app,
        vec![
            created(
                PET,
                EntityKind::Unit,
                &[(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)],
            ),
            created(
                MATE,
                EntityKind::Player,
                &[
                    (FIELD_UNIT_HEALTH, 100),
                    (FIELD_UNIT_MAXHEALTH, 100),
                    (SUMMON, PET as u32),
                    (SUMMON + 1, (PET >> 32) as u32),
                ],
            ),
            list(),
        ],
    );
    hosted(&mut app, r#"TargetUnit("partypet1")"#);
    tip(&mut app);
    show(&mut app, "partypet1");

    let world = app.world_mut();
    world.resource_mut::<crate::ui_script::ReloadUiPending>().0 = true;
    crate::ui_script::run_pending_reload(world);
    tip(&mut app);
    show(&mut app, "partypet1");
    let hurt = |app: &mut App, n| reloaded_frame(app, vec![values(PET, &health(n))]);
    assert_eq!(
        hurt(&mut app, 60),
        ["UNIT_HEALTH:partypet1:100"],
        "the reload's deselect, and the pet's watchers ahead of the new tooltip's"
    );
    hosted(&mut app, r#"TargetUnit("partypet1")"#);
    assert_eq!(
        hurt(&mut app, 40),
        ["UNIT_HEALTH:target:60", "UNIT_HEALTH:partypet1:60"],
        "a flag beside the pet's moves nothing"
    );
    hosted(&mut app, "ClearTarget()");
    assert_eq!(
        hurt(&mut app, 30),
        ["UNIT_HEALTH:partypet1:40"],
        "nor does its removal"
    );
    let _ = reloaded_frame(&mut app, vec![list()]);
    assert_eq!(
        hurt(&mut app, 20),
        ["UNIT_HEALTH:partypet1:20"],
        "the reseat links them behind the tooltip's"
    );
}
