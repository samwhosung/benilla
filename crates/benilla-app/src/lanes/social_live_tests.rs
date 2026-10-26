//! The friend, ignore and `/who` getters as a listener reads them, and their events at their
//! packets. The getters read the friend list and the `/who` array at the call: `GetNumFriends`
//! (`0x5ad000`), `GetFriendInfo` (`0x5ad060` through `0x5ae4b0`), `GetIgnoreName` (`0x5ad460`,
//! its name through `0x55f080`), `GetNumWhoResults` (`0x5ad690`) and `GetWhoInfo` (`0x5ad6e0`).
//! The events fire inside the handler that raises them: the sorts' `FRIENDLIST_UPDATE`
//! (`0x5ae867`), `HandleResult`'s line (`0x5acde6`) and `SMSG_WHO`'s `WHO_LIST_UPDATE`
//! (`0x5adfca`).

use benilla_protocol::messages::{
    friend_result, friend_status, FriendEntry, FriendOnline, FriendStatusUpdate, WhoEntry,
    WhoResults,
};
use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use crate::net::ClientCommand;

/// Two friends and an ignored player the name cache holds.
const ANN: u64 = 0x61;
const BOB: u64 = 0x62;
const CAL: u64 = 0x63;

/// The whole client, headless, in the world with its frame run.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    _sent: Receiver<ClientCommand>,
}

impl Client {
    /// One frame of the client's, with `packets` in the inbound channel.
    fn frame(&mut self, packets: Vec<SessionEvent>) {
        for ev in packets {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// `packets` through their handlers alone, with no frame: the world as the handlers leave it,
    /// which no feed has pushed.
    fn handle(&mut self, packets: Vec<SessionEvent>) {
        crate::net::handlers::dispatch(self.app.world_mut(), packets);
    }

    /// `chunk`'s answer through the lent host when `live`, else off the VM's copies alone.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, live: bool, chunk: &str) -> T {
        if live {
            crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
                s.expect("a VM").eval::<T>(chunk).expect("runs")
            })
        } else {
            crate::script_calls::eval_in_vm(self.app.world(), chunk)
        }
    }

    /// What the spy heard since the last call.
    fn seen(&mut self) -> Vec<String> {
        self.eval(false, "local s = Seen Seen = {} return s")
    }
}

/// The client in the world, the three names cached, Ann on the friend list at level 10, and a spy
/// that notes each social event, each system line and the quest log's `QUEST_LOG_UPDATE`.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            UNKNOWN = "Unknown"
            ERR_FRIEND_ADDED_S = "%s added"
            WHO_LIST_FORMAT = "%s %s %d %s %s - %s"
            WHO_NUM_RESULTS = "%d player total"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"FRIENDLIST_UPDATE", "IGNORELIST_UPDATE", "WHO_LIST_UPDATE",
                    "CHAT_MSG_SYSTEM", "QUEST_LOG_UPDATE"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. (event == "CHAT_MSG_SYSTEM" and (":" .. arg1) or ""))
                if event == "FRIENDLIST_UPDATE" and TO_UI ~= nil then SetWhoToUI(TO_UI) end
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    let mut names = app.world_mut().resource_mut::<crate::names::NameCache>();
    for (guid, name) in [(ANN, "Ann"), (BOB, "Bob"), (CAL, "Cal")] {
        names.insert_player(guid, name.into(), None);
    }
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    c.frame(vec![SessionEvent::FriendList {
        friends: vec![online(ANN, 10)],
    }]);
    c.frame(vec![]);
    c.seen();
    c
}

fn online(guid: u64, level: u32) -> FriendEntry {
    FriendEntry {
        guid,
        status: friend_status::ONLINE,
        area: 0,
        level,
        class: 1,
    }
}

fn who(name: &str) -> WhoEntry {
    WhoEntry {
        name: name.into(),
        guild: String::new(),
        level: 60,
        class: 1,
        race: 1,
        zone: 0,
    }
}

/// What the handlers write is what the getters read at the call, with no push between: a status
/// that levels Ann, an add, an ignore list and a `/who` answer, each read before any frame runs.
/// With no host lent the getters read the VM's snapshot, which nothing in the game pushes.
#[test]
fn the_social_getters_read_the_world_at_the_call() {
    let mut c = client();
    const CHUNK: &str = "local _, level = GetFriendInfo(1) \
        local n, total = GetNumWhoResults() \
        return GetNumFriends(), level, GetIgnoreName(1), n, total, (GetWhoInfo(1))";
    type Answer = (i64, i64, String, i64, i64, Option<String>);
    let read: Answer = c.eval(true, CHUNK);
    // `GetWhoInfo(1)` with none kept reads the array's first slot, as no answer left it.
    assert_eq!(
        read,
        (1, 10, "Unknown".into(), 0, 0, Some(String::new())),
        "as seated"
    );
    c.handle(vec![
        SessionEvent::FriendStatus(FriendStatusUpdate {
            result: friend_result::ONLINE,
            guid: ANN,
            online: Some(FriendOnline {
                status: friend_status::ONLINE,
                area: 0,
                level: 60,
                class: 1,
            }),
        }),
        SessionEvent::FriendStatus(FriendStatusUpdate {
            result: friend_result::ADDED_OFFLINE,
            guid: BOB,
            online: None,
        }),
        SessionEvent::IgnoreList { guids: vec![CAL] },
        SessionEvent::WhoResults(WhoResults {
            displayed: 1,
            total: 7,
            entries: vec![who("Dee")],
        }),
    ]);
    let read: Answer = c.eval(true, CHUNK);
    assert_eq!(
        read,
        (2, 60, "Cal".into(), 1, 7, Some("Dee".into())),
        "the world, not the frame's push"
    );
    let unlent: Answer = c.eval(false, CHUNK);
    assert_eq!(
        unlent,
        (0, 0, "Unknown".into(), 0, 0, None),
        "no host lent: the empty snapshot"
    );
}

/// Each social event fires at its packet's place among the other lanes' items: the friend list's
/// sort, the add's sort and line, and the `/who` answer's event interleave with the time
/// responses' `QUEST_LOG_UPDATE` (`0x4def1b`) as the packets came, not ahead of every lane.
#[test]
fn the_social_events_fire_at_their_packets_places() {
    let mut c = client();
    let sync = |t| SessionEvent::ServerUnixTime { unix_time: t };
    c.frame(vec![
        SessionEvent::FriendList {
            friends: vec![online(ANN, 10)],
        },
        sync(1_000_000),
        SessionEvent::FriendStatus(FriendStatusUpdate {
            result: friend_result::ADDED_OFFLINE,
            guid: BOB,
            online: None,
        }),
        sync(1_000_001),
        SessionEvent::WhoResults(WhoResults {
            displayed: 4,
            total: 4,
            entries: ["Dee", "Eve", "Fay", "Gus"].map(who).to_vec(),
        }),
        sync(1_000_002),
    ]);
    assert_eq!(
        c.seen(),
        [
            "FRIENDLIST_UPDATE",
            "QUEST_LOG_UPDATE",
            // The cached add sorts, then shows its line (`0x5acdde` → `0x5acde6`).
            "FRIENDLIST_UPDATE",
            "CHAT_MSG_SYSTEM:Bob added",
            "QUEST_LOG_UPDATE",
            // Four rows go to the frame (`0x5adfca`).
            "WHO_LIST_UPDATE",
            "QUEST_LOG_UPDATE",
        ]
    );
}

/// `SMSG_WHO` fixes its exit in the handler, off `SetWhoToUI`'s flag as the listeners stamped
/// ahead of it left it (`0x5adf9c`): a friend list's listener that claims the answers for the
/// frame ahead of a one-record answer sends it to `WHO_LIST_UPDATE`, and one behind it leaves it
/// to chat.
#[test]
fn the_who_answer_routes_by_the_flag_its_stamp_finds() {
    let mut c = client();
    let list = || SessionEvent::FriendList {
        friends: vec![online(ANN, 10)],
    };
    let answer = || {
        SessionEvent::WhoResults(WhoResults {
            displayed: 1,
            total: 1,
            entries: vec![who("Dee")],
        })
    };
    let _: () = c.eval(true, "TO_UI = 1");
    c.frame(vec![list(), answer()]);
    assert_eq!(c.seen(), ["FRIENDLIST_UPDATE", "WHO_LIST_UPDATE"], "ahead");
    let _: () = c.eval(true, "SetWhoToUI(0)");
    c.frame(vec![answer(), list()]);
    let seen = c.seen();
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(
        seen[0].starts_with("CHAT_MSG_SYSTEM:Dee Dee 60"),
        "{seen:?}"
    );
    assert_eq!(seen[1], "CHAT_MSG_SYSTEM:1 player total");
    assert_eq!(seen[2], "FRIENDLIST_UPDATE", "behind");
}
