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

use crate::ui_petition::{PetitionEvent, PetitionLane};

const ME: u64 = 0x10;
const OWNER: u64 = 0xaa;
const SIGNER: u64 = 0xbb;
const CHARTER: u64 = 0x4000_0000_0000_0099;

/// The client with us and the owner known, and a VM whose own copy holds no charter: its spy notes
/// each charter event.
fn client() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
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

/// `packets` through their handlers, each item they put on the charter lane stamped inside them,
/// none heard before the drain; then the drain, and what the spy heard.
fn handled(app: &mut App, packets: Vec<SessionEvent>) -> (Vec<PetitionEvent>, Vec<String>) {
    let before = crate::lanes::stamp();
    crate::net::handlers::dispatch(app.world_mut(), packets);
    app.world_mut().flush();
    let after = crate::lanes::stamp();
    let stamped = app.world().resource::<PetitionLane>().stamped();
    assert!(
        stamped.iter().all(|(at, _)| before < *at && *at < after),
        "stamped inside the handler"
    );
    let heard: Vec<String> = eval(app, false, "return Seen");
    assert!(heard.is_empty(), "on the lane: {heard:?}");
    crate::lanes::fire_in_order(app.world_mut());
    let heard = eval(app, false, "local s = Seen Seen = {} return s");
    (stamped.into_iter().map(|(_, e)| e).collect(), heard)
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

/// The show waits on the record's and the signer name's answers, and `PETITION_SHOW` sits on the
/// lane at the last one's handler; our signature's line and `PETITION_CLOSED`, and the turn-in's
/// `GUILD_REGISTRAR_CLOSED`, sit at theirs, and each fires there in the drain.
#[test]
fn the_show_and_the_results_fire_on_the_lane_at_their_handlers() {
    let mut app = client();
    assert_eq!(handled(&mut app, vec![offered()]), (vec![], vec![]));
    assert_eq!(handled(&mut app, vec![record()]), (vec![], vec![]));
    assert_eq!(
        handled(&mut app, vec![signer_name()]),
        (vec![PetitionEvent::Show], vec!["PETITION_SHOW".into()])
    );
    let signed = SessionEvent::PetitionSignResults(PetitionSignResults {
        item: CHARTER,
        player: ME,
        result: petition_result::OK,
    });
    let (stamped, heard) = handled(&mut app, vec![signed]);
    assert!(
        matches!(
            stamped.as_slice(),
            [PetitionEvent::Line(_), PetitionEvent::Closed]
        ),
        "{stamped:?}"
    );
    assert_eq!(heard, ["PETITION_CLOSED"]);
    assert_eq!(
        handled(
            &mut app,
            vec![SessionEvent::TurnInPetitionResults {
                result: petition_result::OK
            }]
        ),
        (
            vec![PetitionEvent::RegistrarClosed],
            vec!["GUILD_REGISTRAR_CLOSED".into()]
        )
    );
}
