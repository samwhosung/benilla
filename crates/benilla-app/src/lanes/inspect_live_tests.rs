//! The inspect and honor getters as they read through the lent host: another player's visible
//! items off its object (`0x4c8328` → `0x5f0d60`), the active player's honor block
//! (`0x51a4b0`) and the inspect-honor slot (`0x4c95e0`, `0x4c9620`), each as the getter runs. The
//! reply handler stores a reply for the inspect target and signals `INSPECT_HONOR_UPDATE` inside
//! itself (`0x4c6e40`, `0x4c6f56`), so the event fires at its packet's place.

use benilla_protocol::field::FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR;
use benilla_protocol::messages::InspectHonorStats;
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::{UiScript, UnitReach};
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

/// [`client`] with [`MATE`] created wearing the helm and selected, its reach in the VM, and the
/// helm's and the crown's records cached.
fn inspecting() -> App {
    let mut app = client();
    frame(
        &mut app,
        vec![created(MATE, EntityKind::Player, &[(HELM_ENTRY, HELM)])],
    );
    let world = app.world_mut();
    let mut items = world.resource_mut::<crate::items::Items>();
    items.insert_template(HELM, Some(crate::items::test_template("Helm")));
    items.insert_template(CROWN, Some(crate::items::test_template("Crown")));
    let mate = world.resource::<crate::net::GuidIndex>().0[&MATE];
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(mate),
        guid: Some(MATE),
        ..Default::default()
    };
    world.non_send_resource_mut::<UiScript>().set_unit_reach_of(
        MATE,
        UnitReach {
            dist_sq: 4.0,
            inspectable: true,
        },
    );
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

/// `INSPECT_HONOR_UPDATE` fires at its reply's packet, between the two health packets around it,
/// and only for a reply naming the target `NotifyInspect` set (`0x4c6e58`); its listener reads the
/// slot the reply filled. `ClearInspectPlayer` clears the flags and keeps the figures
/// (`0x4c6f70`), so `GetInspectHonorData` still answers them (`0x4c9620`).
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
    hosted::<()>(&mut app, "ClearInspectPlayer()");
    assert_eq!(hosted::<(bool, u32)>(&mut app, READ), (false, 5));
    // With no target, a reply is dropped and signals nothing.
    assert!(frame(&mut app, vec![reply(MATE, 6)]).is_empty());
    assert_eq!(hosted::<(bool, u32)>(&mut app, READ), (false, 5));
}
