//! The charter getters as a listener reads them, and the charter's events at their packets. The
//! getters read the window's signers, its record and its owner's name at the call (`0x4f43d0`,
//! `0x4f44e0`, `0x4f4510`, `0x4f45e0`); the show signals `PETITION_SHOW` inside its handler once
//! every name and the record are held (`0x4f41b4`), a missed name's answer inside its own
//! (`0x4f433f`), and our signature's success and the turn-in's theirs (`0x5ef037`, `0x5ef16b`).

use benilla_protocol::messages::{
    petition_result, PetitionQueryResponse, PetitionShowSignatures, PetitionSignResults,
    PetitionSignature,
};
use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::ui_petition::PetitionLane;

const ME: u64 = 0x10;
const OWNER: u64 = 0xaa;
const SIGNER: u64 = 0xbb;
const CHARTER: u64 = 0x4000_0000_0000_0099;

/// The client with us and the owner known, and a VM whose own copy holds no charter: its spy notes
/// each charter event.
fn client() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    // In the world with the interface up, so the dispatch fires each packet's lanes.
    app.insert_resource(bevy::prelude::State::new(
        crate::client_state::ClientState::InWorld,
    ));
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    app.insert_resource(crate::net::SelfGuid(Some(ME)));
    app.world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(OWNER, "Owner".into(), None);
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"PETITION_SHOW", "PETITION_CLOSED", "GUILD_REGISTRAR_CLOSED"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function() table.insert(Seen, event) end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
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

/// `packets` through their handlers: each event the charter lane takes fires inside the dispatch, at
/// its packet's place, as a handler signals inside itself (`0x703e50`) ahead of the next packet's
/// (`0x537ae5`), so the drain finds nothing; what the spy heard.
fn handled(app: &mut App, packets: Vec<SessionEvent>) -> Vec<String> {
    crate::net::handlers::dispatch(app.world_mut(), packets);
    app.world_mut().flush();
    assert!(
        app.world().resource::<PetitionLane>().stamped().is_empty(),
        "fired inside the dispatch"
    );
    eval(app, false, "local s = Seen Seen = {} return s")
}

fn offered() -> SessionEvent {
    SessionEvent::PetitionShowSignatures(PetitionShowSignatures {
        item: CHARTER,
        owner: OWNER,
        petition_id: 7,
        signatures: vec![PetitionSignature {
            signer: SIGNER,
            unknown: 0,
        }],
    })
}

fn record() -> SessionEvent {
    SessionEvent::PetitionQueryResponse(PetitionQueryResponse {
        petition_id: 7,
        owner: OWNER,
        name: "Legacy".into(),
        flags: 1,
        min_signatures: 9,
        ..Default::default()
    })
}

fn signer_name() -> SessionEvent {
    SessionEvent::PlayerName {
        guid: SIGNER,
        name: "Bee".into(),
        race: 1,
        gender: 0,
        class: 1,
    }
}

/// With the host lent the getters read the charter as the handlers left it, where the VM's copy
/// holds none.
#[test]
fn the_getters_read_the_charter_through_the_host() {
    let mut app = client();
    crate::net::handlers::dispatch(app.world_mut(), vec![offered(), record(), signer_name()]);
    app.world_mut().flush();
    let read = "local _, title, _, max, owner = GetPetitionInfo() \
                return title, max, owner, GetNumPetitionNames(), GetPetitionNameInfo(1), \
                CanSignPetition()";
    type Read = (
        Option<String>,
        i64,
        Option<String>,
        i64,
        Option<String>,
        Option<i64>,
    );
    assert_eq!(
        eval::<Read>(&mut app, false, read),
        (None, 0, None, 0, None, None),
        "the copy"
    );
    assert_eq!(
        eval::<Read>(&mut app, true, read),
        (
            Some("Legacy".into()),
            9,
            Some("Owner".into()),
            1,
            Some("Bee".into()),
            Some(1)
        ),
        "live"
    );
}

/// The show waits on the record's and the signer name's answers, and `PETITION_SHOW` fires inside
/// the last one's dispatch; our signature's chat line (the catalog sends the success ids to chat)
/// and `PETITION_CLOSED`, and the turn-in's
/// `GUILD_REGISTRAR_CLOSED`, inside theirs.
#[test]
fn the_show_and_the_results_fire_inside_their_dispatch() {
    let mut app = client();
    eval::<()>(
        &mut app,
        false,
        r#"ERR_PETITION_SIGNED = "Signed"
        local g = CreateFrame("Frame")
        g:RegisterEvent("CHAT_MSG_SYSTEM")
        g:SetScript("OnEvent", function() table.insert(Seen, "line " .. arg1) end)"#,
    );
    assert!(handled(&mut app, vec![offered()]).is_empty());
    assert!(handled(&mut app, vec![record()]).is_empty());
    assert_eq!(handled(&mut app, vec![signer_name()]), ["PETITION_SHOW"]);
    let signed = SessionEvent::PetitionSignResults(PetitionSignResults {
        item: CHARTER,
        player: ME,
        result: petition_result::OK,
    });
    assert_eq!(
        handled(&mut app, vec![signed]),
        ["line Signed", "PETITION_CLOSED"]
    );
    assert_eq!(
        handled(
            &mut app,
            vec![SessionEvent::TurnInPetitionResults {
                result: petition_result::OK
            }]
        ),
        ["GUILD_REGISTRAR_CLOSED"]
    );
}
