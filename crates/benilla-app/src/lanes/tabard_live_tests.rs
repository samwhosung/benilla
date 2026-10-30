//! The designer's getters as a listener reads them, and its events at their packets. The getters
//! read the guild record and the save latch at the call (`0x5f0800`); the open signals inside its
//! handler (`0x4f588d`), and the save reply its line and, past a failure, `TABARD_SAVE_PENDING`
//! (`0x5e711b`, `0x5e7172`).

use benilla_protocol::SessionEvent;
use benilla_ui::script::{TabardHost, UiScript, UnitState};
use bevy::prelude::*;

use crate::ui_tabard::{TabardLane, TabardOpen};

const VENDOR: u64 = 0xF130_0000_0000_0042;
/// `PLAYER_GUILDID`, absolute.
const GUILD_ID: u16 = 191;

/// The client with our player in guild 5, whose record is cached, and a VM whose own copy says no
/// record is: its spy notes each designer event and message line.
fn client() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    // In the world with the interface up, so the dispatch fires each packet's lanes.
    app.insert_resource(bevy::prelude::State::new(
        crate::client_state::ClientState::InWorld,
    ));
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let world = app.world_mut();
    world.spawn((
        crate::net::SelfPlayer,
        crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(GUILD_ID, 5)])),
    ));
    {
        let mut guild = world.resource_mut::<crate::ui_guild::GuildState>();
        guild.join_for_test(5);
        guild.land_for_test(5, "Probe Guild");
    }
    let mut script = UiScript::new().expect("a VM");
    script.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            ..Default::default()
        }),
    );
    script.set_tabard_host(TabardHost::default());
    script
        .run(
            r#"
            Seen = {}
            ERR_GUILDEMBLEM_NOTGUILDMASTER = "not master"
            TM = CreateFrame("TabardModel", "TM")
            local f = CreateFrame("Frame")
            for _, e in ipairs({"OPEN_TABARD_FRAME", "TABARD_SAVE_PENDING", "UI_ERROR_MESSAGE",
                    "UI_INFO_MESSAGE"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. tostring(arg1))
            end)
            "#,
        )
        .unwrap();
    world.insert_non_send_resource(script);
    app
}

/// `chunk`'s answer through the lent host when `live`, else off the VM's copy alone.
fn eval<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, live: bool, chunk: &str) -> T {
    if live {
        crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
            s.expect("a VM").eval::<T>(chunk).expect("runs")
        })
    } else {
        crate::script_calls::eval_in_vm(app.world(), chunk)
    }
}

/// `CanSaveTabardNow` with the host lent reads the guild record and the latch as the world holds
/// them, where the VM's copy holds neither; each write shows at the next call, with no push.
#[test]
fn can_save_reads_the_record_and_the_latch_through_the_host() {
    let mut app = client();
    let can = "return TM:CanSaveTabardNow()";
    assert_eq!(eval::<Option<i64>>(&mut app, false, can), None, "the copy");
    assert_eq!(eval::<Option<i64>>(&mut app, true, can), Some(1), "live");
    app.world_mut()
        .resource_mut::<crate::ui_guild::GuildState>()
        .evict_own_identity();
    assert_eq!(
        eval::<Option<i64>>(&mut app, true, can),
        None,
        "the record dropped"
    );
}

/// The activate's `OPEN_TABARD_FRAME` and the failed save reply's line and `TABARD_SAVE_PENDING`
/// fire inside their packet's dispatch, as a handler signals inside itself (`0x703e50`) ahead of
/// the next packet's (`0x537ae5`).
#[test]
fn the_open_and_the_reply_fire_inside_their_dispatch() {
    let mut app = client();
    let handled = |app: &mut App, ev: SessionEvent| {
        crate::net::handlers::dispatch(app.world_mut(), vec![ev]);
        app.world_mut().flush();
        assert!(
            app.world().resource::<TabardLane>().stamped().is_empty(),
            "fired inside the dispatch"
        );
        eval::<Vec<String>>(app, false, "local s = Seen Seen = {} return s")
    };
    let heard = handled(&mut app, SessionEvent::TabardVendorActivate(VENDOR));
    assert_eq!(heard, ["OPEN_TABARD_FRAME:nil"]);
    assert_eq!(app.world().resource::<TabardOpen>().npc(), Some(VENDOR));

    let heard = handled(&mut app, SessionEvent::SaveGuildEmblemResult(3));
    assert_eq!(
        heard,
        ["UI_ERROR_MESSAGE:not master", "TABARD_SAVE_PENDING:nil"]
    );
}

/// A close in the open's own frame runs after the open: the open's listener starts the preview
/// (`TabardFrame.lua`, `InitializeTabardColors`) and the close core's undo (`0x4f58ce` →
/// `0x5e0830`) comes after it, ahead of `CLOSE_TABARD_FRAME` (`0x4f58f8`), so no preview is left
/// on the body with no designer open.
#[test]
fn a_close_after_the_open_ends_the_preview_the_open_started() {
    let mut app = client();
    eval::<()>(
        &mut app,
        false,
        r#"
        Order = {}
        local g = CreateFrame("Frame")
        g:RegisterEvent("OPEN_TABARD_FRAME")
        g:RegisterEvent("CLOSE_TABARD_FRAME")
        g:SetScript("OnEvent", function()
            table.insert(Order, event)
            if event == "OPEN_TABARD_FRAME" then TM:InitializeTabardColors() end
        end)
        "#,
    );
    let preview = |app: &App| app.world().non_send_resource::<UiScript>().tabard_design();
    crate::net::handlers::dispatch(
        app.world_mut(),
        vec![SessionEvent::TabardVendorActivate(VENDOR)],
    );
    app.world_mut().flush();
    // Another window's open takes the latch in the same frame, which closes the designer
    // (`0x493310`).
    crate::ui_session::register(app.world_mut(), 0xF130_0000_0000_0043);
    app.world_mut().flush();
    assert_eq!(app.world().resource::<TabardOpen>().npc(), None, "closed");
    assert_eq!(preview(&app), None, "the close ended the open's preview");
    crate::lanes::fire_in_order(app.world_mut());
    let order: Vec<String> = eval(&mut app, false, "return Order");
    assert_eq!(order, ["OPEN_TABARD_FRAME", "CLOSE_TABARD_FRAME"]);
    assert_eq!(
        preview(&app),
        None,
        "the open's preview ended with the close"
    );
}
