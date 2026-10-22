//! The charter verbs through the whole client's host: each sends its packet, shows its line or
//! closes the window inside its call, as the reference's bindings do (`0x4f46d0`, `0x4f4790`,
//! `0x4f4930`, `0x4f5260`, `0x4f43c0` → `0x4f3f60`).

use benilla_protocol::messages::{
    PetitionQueryResponse, PetitionShowSignatures, PetitionSignature,
};
use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;
use bevy::ecs::system::SystemId;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::{ClientCommand, NetCommands};
use crate::script_calls::with_hosted_vm;

const ME: u64 = 0x10;
const OWNER: u64 = 0xaa;
const CHARTER: u64 = 0x4000_0000_0000_0099;

struct Charter {
    app: App,
    rx: Receiver<ClientCommand>,
    /// The charter feed and drain, registered once so their memories persist.
    feed: SystemId,
    drain: SystemId,
}

/// The whole client, headless, with a VM that records the charter's events and lines.
fn client() -> Charter {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    app.world_mut()
        .insert_resource(crate::net::SelfGuid(Some(ME)));
    app.world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(OWNER, "Owner".to_string(), None);
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"HEARD = ""
            ERR_GUILD_ENTER_NAME = "Enter a guild name."
            local f = CreateFrame("Frame")
            for _, e in ipairs({"PETITION_SHOW", "PETITION_CLOSED", "UI_ERROR_MESSAGE",
                    "CHAT_MSG_SYSTEM"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                HEARD = HEARD .. event .. "(" .. tostring(arg1) .. ") "
            end)"#,
        )
        .expect("the recorder installs");
    app.world_mut().insert_non_send_resource(script);
    let feed = app.world_mut().register_system(super::feed::feed_petition);
    let drain = app.world_mut().register_system(super::feed::drain_petition);
    Charter {
        app,
        rx,
        feed,
        drain,
    }
}

/// Another player's charter offered to us, its record landed, and the feed's show.
fn offered(c: &mut Charter) {
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![
            SessionEvent::PetitionShowSignatures(PetitionShowSignatures {
                item: CHARTER,
                owner: OWNER,
                petition_id: 7,
                signatures: Vec::<PetitionSignature>::new(),
            }),
            SessionEvent::PetitionQueryResponse(PetitionQueryResponse {
                petition_id: 7,
                owner: OWNER,
                name: "Legacy".into(),
                flags: 1,
                min_signatures: 9,
                ..Default::default()
            }),
        ],
    );
    frame(c);
    let _ = sent(c);
    assert_eq!(heard(c), "PETITION_SHOW(nil) ");
}

/// The drain and the feed, as a frame runs them.
fn frame(c: &mut Charter) {
    let (drain, feed) = (c.drain, c.feed);
    c.app.world_mut().flush();
    c.app.world_mut().run_system(drain).expect("the drain runs");
    c.app.world_mut().flush();
    c.app.world_mut().run_system(feed).expect("the feed runs");
}

fn run(c: &mut Charter, lua: &str) {
    with_hosted_vm(c.app.world_mut(), |vm| vm.expect("a VM").run(lua)).expect("the chunk runs");
}

fn eval<T: benilla_ui::mlua::FromLuaMulti>(c: &mut Charter, lua: &str) -> T {
    with_hosted_vm(c.app.world_mut(), |vm| vm.expect("a VM").eval::<T>(lua))
        .expect("the chunk runs")
}

fn heard(c: &mut Charter) -> String {
    eval(c, "local h = HEARD HEARD = \"\" return h")
}

/// The packets sent since the last read, the name and record queries left out.
fn sent(c: &Charter) -> Vec<String> {
    c.rx.try_iter()
        .map(|c| format!("{c:?}"))
        .filter(|c| !c.starts_with("NameQuery") && !c.starts_with("PetitionQuery"))
        .collect()
}

/// `RenamePetition` loads the open item's guid before it reads the name (`0x4f495e`), and with
/// none returns at once (`0x4f4970`): the name check `0x4f5160` never runs, so no line shows,
/// now or later.
#[test]
fn a_rename_with_no_petition_open_shows_nothing() {
    let mut c = client();
    run(&mut c, "RenamePetition('') RenamePetition(' Legacy')");
    frame(&mut c);
    assert_eq!(heard(&mut c), "");
    assert!(sent(&c).is_empty());
}

/// With a charter open the name check shows its refusal inside the call (`0x4f5160` →
/// `0x496720`), and a passing name sends `CMSG_PETITION_RENAME` there (`0x4f49d5`).
#[test]
fn a_rename_refuses_or_sends_inside_its_call() {
    let mut c = client();
    offered(&mut c);
    run(&mut c, "RenamePetition('')");
    assert_eq!(heard(&mut c), "UI_ERROR_MESSAGE(Enter a guild name.) ");
    run(&mut c, "RenamePetition('Legacy')");
    assert_eq!(
        sent(&c),
        [format!(
            "PetitionRename {{ item: {CHARTER}, name: \"Legacy\" }}"
        )]
    );
}

/// `BuyGuildCharter`'s name check shows its refusal inside the call (`0x4f5294` → `0x4f5160`),
/// before the verb returns nil.
#[test]
fn a_refused_charter_name_shows_inside_its_call() {
    let mut c = client();
    run(&mut c, "R = BuyGuildCharter('') SEEN = HEARD");
    assert_eq!(
        eval::<String>(&mut c, "return SEEN"),
        "UI_ERROR_MESSAGE(Enter a guild name.) "
    );
    assert_eq!(eval::<Option<i64>>(&mut c, "return R"), None);
}

/// `SignPetition` sends `CMSG_PETITION_SIGN` with the item and the byte inside the call
/// (`0x4f4749`), in call order with `RandomRoll`'s.
#[test]
fn a_signature_sends_inside_its_call() {
    let mut c = client();
    offered(&mut c);
    run(&mut c, "SignPetition() RandomRoll(1, 2)");
    assert_eq!(
        sent(&c),
        [
            format!("PetitionSign {{ item: {CHARTER}, byte: 1 }}"),
            "RandomRoll { min: 1, max: 2 }".to_string(),
        ]
    );
}

/// `ClosePetition` (`0x4f43c0` → `0x4f3f60`) sends the decline for another's charter with a
/// record cached (`0x4f3ff3`) and signals `PETITION_CLOSED` before it returns (`0x4f402c`), with
/// the item's guid already zeroed (`0x4f4020`), so the handler's own `ClosePetition` finds
/// nothing; the feed announces nothing more.
#[test]
fn a_close_declines_and_signals_inside_its_call() {
    let mut c = client();
    offered(&mut c);
    run(
        &mut c,
        "local f = CreateFrame('Frame') f:RegisterEvent('PETITION_CLOSED') \
         f:SetScript('OnEvent', function() ClosePetition() end) \
         ClosePetition() SEEN = HEARD",
    );
    assert_eq!(
        eval::<String>(&mut c, "return SEEN"),
        "PETITION_CLOSED(nil) "
    );
    assert_eq!(sent(&c), [format!("PetitionDecline {{ item: {CHARTER} }}")]);
    let _ = heard(&mut c);
    frame(&mut c);
    assert_eq!(heard(&mut c), "");
    assert!(sent(&c).is_empty());
}

/// A handler that notes the title `GetPetitionInfo` answers as `PETITION_CLOSED` runs, and closes
/// the window as the stock frame's `OnHide` does.
fn note_closed_title(c: &mut Charter) {
    run(
        c,
        "CLOSED_SAW = 'unset' local f = CreateFrame('Frame') f:RegisterEvent('PETITION_CLOSED') \
         f:SetScript('OnEvent', function() local _, t = GetPetitionInfo() CLOSED_SAW = t \
         ClosePetition() end)",
    );
}

fn closed_saw(c: &mut Charter) -> Option<String> {
    eval(c, "return CLOSED_SAW")
}

fn title(c: &mut Charter) -> Option<String> {
    eval(c, "local _, t = GetPetitionInfo() return t")
}

/// `ClosePetition`'s `0x4f3f60` signals `PETITION_CLOSED` (`0x4f402c`) before it drops the
/// record (`0x4f4078`), so a handler still reads the charter's title; it is gone after the call.
#[test]
fn a_close_signals_over_the_record_and_drops_it_after() {
    let mut c = client();
    offered(&mut c);
    note_closed_title(&mut c);
    run(&mut c, "ClosePetition()");
    assert_eq!(closed_saw(&mut c).as_deref(), Some("Legacy"));
    assert_eq!(title(&mut c), None);
}

/// Another charter offered while one is shown; `record` says whether its record lands with it.
fn offered_again(c: &mut Charter, record: bool) {
    let mut events = vec![SessionEvent::PetitionShowSignatures(
        PetitionShowSignatures {
            item: SECOND,
            owner: OWNER,
            petition_id: 8,
            signatures: Vec::<PetitionSignature>::new(),
        },
    )];
    if record {
        events.push(SessionEvent::PetitionQueryResponse(PetitionQueryResponse {
            petition_id: 8,
            owner: OWNER,
            name: "Second".into(),
            flags: 1,
            min_signatures: 9,
            ..Default::default()
        }));
    }
    crate::net::handlers::dispatch(c.app.world_mut(), events);
    frame(c);
}

const SECOND: u64 = 0x4000_0000_0000_009a;

/// The replace leg of `0x4f3f60`: the old charter's `PETITION_CLOSED` signals over its own record
/// with the item's guid zeroed (`0x4f4020`-`0x4f402c`), and only then is the next charter's record
/// looked up (`0x4f4061`) and shown.
#[test]
fn a_replaced_charter_closes_over_its_own_record_first() {
    let mut c = client();
    offered(&mut c);
    note_closed_title(&mut c);
    offered_again(&mut c, true);
    assert_eq!(closed_saw(&mut c).as_deref(), Some("Legacy"));
    assert_eq!(heard(&mut c), "PETITION_CLOSED(nil) PETITION_SHOW(nil) ");
    assert_eq!(title(&mut c).as_deref(), Some("Second"));
}

/// With the next charter's record still to come the old one closes at once, over its own record,
/// and the handler's `ClosePetition()` finds no item (`0x4f4020`), so the next charter is neither
/// closed nor declined.
#[test]
fn a_replaced_charter_closes_first_while_the_next_resolves() {
    let mut c = client();
    offered(&mut c);
    note_closed_title(&mut c);
    offered_again(&mut c, false);
    assert_eq!(closed_saw(&mut c).as_deref(), Some("Legacy"));
    assert_eq!(heard(&mut c), "PETITION_CLOSED(nil) ");
    assert!(
        !sent(&c).iter().any(|s| s.contains(&SECOND.to_string())),
        "nothing sent for the next charter"
    );
}
