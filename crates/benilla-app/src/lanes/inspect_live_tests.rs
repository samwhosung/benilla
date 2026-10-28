//! The inspect and honor getters as they read through the lent host: another player's visible
//! items off its object (`0x4c8328` → `0x5f0d60`), the active player's honor block
//! (`0x51a4b0`) and the inspect-honor slot (`0x4c95e0`, `0x4c9620`), each as the getter runs. The
//! reply handler stores a reply for the inspect target and signals `INSPECT_HONOR_UPDATE` inside
//! itself (`0x4c6e40`, `0x4c6f56`), so the event fires at its packet's place.

use benilla_protocol::field::FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR;
use benilla_protocol::messages::InspectHonorStats;
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, created, frame, health, values};

const ME: u64 = 0x7;
/// Another player, a cooperative one 2 yards off.
const MATE: u64 = 0x21;
const HELM: u32 = 7366;
const CROWN: u32 = 7367;
/// `PLAYER_FIELD_SESSION_KILLS`.
const SESSION_KILLS: u16 = 1250;
/// The helm slot's entry, `PLAYER_VISIBLE_ITEM_1_0`.
const HELM_ENTRY: u16 = FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 2;

/// `chunk`'s answer with the call host lent, as a listener's or a key's Lua runs.
fn hosted<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, chunk: &str) -> T {
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| s.expect("a VM").eval(chunk))
        .expect("the chunk runs")
}

/// `chunk`'s answer with no host lent: the snapshots the VM holds.
fn unhosted<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, chunk: &str) -> T {
    crate::script_calls::eval_in_vm(app.world(), chunk)
}

/// [`client`] with [`MATE`] created wearing the helm and selected, cooperative and in range, and the
/// helm's and the crown's records cached.
fn inspecting() -> App {
    let mut app = mate_beside();
    let world = app.world_mut();
    let mate = world.resource::<crate::net::GuidIndex>().0[&MATE];
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(mate),
        guid: Some(MATE),
        ..Default::default()
    };
    app
}

/// [`client`] with [`MATE`] created wearing the helm, unselected, cooperative and in range, and the
/// helm's and the crown's records cached.
fn mate_beside() -> App {
    let mut app = client();
    frame(
        &mut app,
        vec![created(MATE, EntityKind::Player, &[(HELM_ENTRY, HELM)])],
    );
    let world = app.world_mut();
    let mut items = world.resource_mut::<crate::items::Items>();
    items.insert_template(HELM, Some(crate::items::test_template("Helm")));
    items.insert_template(CROWN, Some(crate::items::test_template("Crown")));
    // Both Human beside us, so the host's reach holds MATE cooperative (`0x4944e4`) and in range.
    use crate::target::relations::fixture::{factions, HUMAN};
    const TEMPLATE: u16 = 35;
    app.insert_resource(factions());
    write(&mut app, ME, &[(TEMPLATE, HUMAN)]);
    write(&mut app, MATE, &[(TEMPLATE, HUMAN)]);
    app
}

/// Write `pairs` into `guid`'s store, as a handler would, with nothing pushed to the VM.
fn write(app: &mut App, guid: u64, pairs: &[(u16, u32)]) {
    let world = app.world_mut();
    let e = world.resource::<crate::net::GuidIndex>().0[&guid];
    world
        .get_mut::<crate::net::ObjectStore>(e)
        .unwrap()
        .0
        .merge(ObjectFields::from_pairs(pairs));
}

/// A reply for `guid` with `session` honorable kills today.
fn reply(guid: u64, session: u32) -> SessionEvent {
    SessionEvent::InspectHonorStats(InspectHonorStats {
        player_guid: guid,
        highest_rank: 9,
        session_kills: session,
        yesterday_hk: 0,
        unknown_old1: 0,
        last_week_hk: 0,
        unknown_old2: 0,
        this_week_hk: 0,
        unknown_old3: 0,
        lifetime_hk: 0,
        lifetime_dhk: 0,
        yesterday_honor: 0,
        last_week_honor: 0,
        this_week_honor: 0,
        last_week_rank: 0,
        rank_bar: 0,
    })
}

/// The slot getters read another player's visible block off its object as they run, and the
/// self getters the active player's honor block, each write showing with nothing pushed; with no
/// host lent, the snapshots, which hold nothing.
#[test]
fn hosted_getters_read_the_visible_items_and_the_honor_block_as_written() {
    let mut app = inspecting();
    const LINK: &str = r#"return GetInventoryItemLink("target", 1)"#;
    let link = hosted::<String>(&mut app, LINK);
    assert!(link.contains("[Helm]"), "{link}");
    write(&mut app, MATE, &[(HELM_ENTRY, CROWN)]);
    let link = hosted::<String>(&mut app, LINK);
    assert!(
        link.contains("[Crown]"),
        "the write, no push between: {link}"
    );
    assert_eq!(unhosted::<Option<String>>(&mut app, LINK), None, "no host");
    // An empty slot reads nil, the object's own.
    assert_eq!(
        hosted::<Option<String>>(&mut app, r#"return GetInventoryItemLink("target", 5)"#),
        None
    );

    const KILLS: &str = "return GetPVPSessionStats()";
    assert_eq!(hosted::<(u32, u32)>(&mut app, KILLS), (0, 0));
    write(&mut app, ME, &[(SESSION_KILLS, 3 | (2 << 16))]);
    assert_eq!(hosted::<(u32, u32)>(&mut app, KILLS), (3, 2));
    assert_eq!(unhosted::<(u32, u32)>(&mut app, KILLS), (0, 0), "no host");
}

/// What the spy heard since the last take, taken.
fn heard(app: &mut App) -> Vec<String> {
    hosted(app, "local seen = Seen; Seen = {}; return seen")
}

/// `INSPECT_HONOR_UPDATE` fires at its reply's packet, between the two health packets around it,
/// and only for a reply naming the target `NotifyInspect` set (`0x4c6e58`); its listener reads the
/// slot the reply filled. A new target drops the held reply and signals, inside the call that set
/// it, and `ClearInspectPlayer` with nothing held signals nothing (the slot's `Deviation:`).
#[test]
fn the_honor_reply_fires_at_its_packet_for_the_target_alone() {
    let mut app = inspecting();
    hosted::<()>(
        &mut app,
        r#"
        NotifyInspect("target")
        local f = CreateFrame("Frame")
        f:RegisterEvent("INSPECT_HONOR_UPDATE")
        f:SetScript("OnEvent", function()
            local kills = GetInspectHonorData()
            table.insert(Seen, event .. ":" .. tostring(HasInspectHonorData()) .. ":" .. kills)
        end)
        "#,
    );
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(ME, &health(70)),
                reply(0x99, 4),
                reply(MATE, 5),
                values(ME, &health(60)),
            ]
        ),
        [
            "UNIT_HEALTH:player:70",
            "INSPECT_HONOR_UPDATE:1:5",
            "UNIT_HEALTH:player:60"
        ]
    );
    const READ: &str = "return HasInspectHonorData() == 1, (GetInspectHonorData())";
    assert_eq!(hosted::<(bool, u32)>(&mut app, READ), (true, 5));
    // The same target again keeps the reply; another nonzero guid drops it and signals at once.
    hosted::<()>(&mut app, r#"NotifyInspect("target")"#);
    assert!(heard(&mut app).is_empty(), "the same target");
    hosted::<()>(&mut app, r#"NotifyInspect("player")"#);
    assert_eq!(
        heard(&mut app),
        ["INSPECT_HONOR_UPDATE:nil:0"],
        "re-targeted"
    );
    assert_eq!(hosted::<(bool, u32)>(&mut app, READ), (false, 0));
    // A reply for the old target is dropped and signals nothing; with nothing held, a clear is
    // quiet.
    assert!(frame(&mut app, vec![reply(MATE, 6)]).is_empty());
    hosted::<()>(&mut app, "ClearInspectPlayer()");
    assert!(heard(&mut app).is_empty(), "nothing held");
    assert_eq!(hosted::<(bool, u32)>(&mut app, READ), (false, 0));
}

/// A player hovered and then targeted for the first time: the slot getters inside its
/// `UPDATE_MOUSEOVER_UNIT` (`0x4929b3`) and its `PLAYER_TARGET_CHANGED` (`0x493a73`) read the
/// object each token names as they run (`0x4c8328`), with no view pushed.
#[test]
fn the_slot_getters_read_the_object_inside_the_hover_and_target_events() {
    use bevy::ecs::system::RunSystemOnce;
    let mut app = mate_beside();
    {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        crate::ui_script::test_ui::load_tooltip_template(&script);
        script
            .run(
                r#"
                UIParent = CreateFrame("Frame", "UIParent"); UIParent:SetAllPoints()
                CreateFrame("GameTooltip", "GameTooltip", UIParent, "TestTooltipTemplate"):Hide()
                local f = CreateFrame("Frame")
                f:RegisterEvent("UPDATE_MOUSEOVER_UNIT")
                f:RegisterEvent("PLAYER_TARGET_CHANGED")
                f:SetScript("OnEvent", function()
                    local unit = event == "PLAYER_TARGET_CHANGED" and "target" or "mouseover"
                    table.insert(Seen, event .. ":" .. tostring(CanInspect(unit)) .. ":"
                        .. tostring((GetInventoryItemLink(unit, 1))))
                end)
                "#,
            )
            .unwrap();
    }
    // Both Human, so the hover's reach holds MATE cooperative (`0x4944e4`).
    use crate::target::relations::fixture::{factions, HUMAN};
    const TEMPLATE: u16 = 35;
    app.insert_resource(factions());
    write(&mut app, ME, &[(TEMPLATE, HUMAN)]);
    write(&mut app, MATE, &[(TEMPLATE, HUMAN)]);
    let world = app.world_mut();
    let mate = world.resource::<crate::net::GuidIndex>().0[&MATE];
    *world.resource_mut::<crate::target::Hovered>() = crate::target::Hovered {
        target: Some(mate),
        guid: Some(MATE),
        distance: 2.0,
        ..Default::default()
    };
    world
        .run_system_once(crate::ui_tooltip::drive_mouseover_tooltip)
        .unwrap();
    hosted::<()>(&mut app, r#"TargetUnit("mouseover")"#);
    let seen = frame(&mut app, vec![]);
    for event in ["UPDATE_MOUSEOVER_UNIT", "PLAYER_TARGET_CHANGED"] {
        assert!(
            seen.iter()
                .any(|l| l.starts_with(&format!("{event}:1:")) && l.contains("[Helm]")),
            "{event}: {seen:?}"
        );
    }
}

/// `PLAYER_ENTERING_WORLD`'s handler reads the honor block off the active player as it runs
/// (`0x51a4b0`): `HonorFrame_Update(1)` reads the weekly values only then
/// (`HonorFrame.lua:10-11`). A reload's VM hears the cascade over a block written with no push.
#[test]
fn the_world_enter_listener_reads_the_honor_block() {
    let mut app = client();
    write(&mut app, ME, &[(SESSION_KILLS, 3)]);
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_ENTERING_WORLD")
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. GetPVPSessionStats())
            end)
            "#,
        )
        .unwrap();
    app.world_mut().insert_non_send_resource(script);
    // A `/reload`'s cascade (`0x490168`), as the reload queues it on the arrival lane.
    app.world_mut()
        .resource_mut::<crate::ui_world_enter::ArrivalLane>()
        .push(crate::ui_world_enter::Entry::Reload);
    let seen = frame(&mut app, vec![]);
    assert!(
        seen.iter().any(|l| l == "PLAYER_ENTERING_WORLD:3"),
        "{seen:?}"
    );
}
