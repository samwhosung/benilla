//! The friend list through the whole client's host, read by the Lua on the line or the feed after
//! each packet: `SMSG_FRIEND_LIST` (`0x5ae350`) names a slot only from the cache and signals only
//! with no name pending, the sort (`0x5ae850`) puts online friends first, `SMSG_FRIEND_STATUS`
//! (`0x5acab0`) acts through its table (`0x5ace1c`), the name callback (`0x5ace70`) finishes what
//! a miss left, `GetFriendInfo` (`0x5ad060`) answers every index, and the friend and ignore verbs
//! act inside their calls.

use benilla_protocol::messages::FriendStatusUpdate;
use benilla_protocol::messages::{friend_result, friend_status, FriendEntry, FriendOnline};
use benilla_protocol::SessionEvent;
use bevy::ecs::system::SystemId;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use benilla_ui::script::UiScript;

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::script_calls::with_hosted_vm;
use crate::ui_action::UiErrorKeys;

/// A zone and a subzone under it, for the area lookup's one parent hop (`0x5ad127`-`0x5ad13f`).
const ELWYNN: u32 = 12;
const GOLDSHIRE: u32 = 87;

struct Social {
    app: App,
    rx: Receiver<ClientCommand>,
    feed: SystemId,
}

/// The whole client, headless, with the stock class names, two areas, and a VM that counts
/// `FRIENDLIST_UPDATE` in `UPDATES` and defines the strings `GetFriendInfo` reads. The first feed,
/// which stands for every list's packet, has run.
fn client() -> Social {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    crate::ui_unit::TestNames::english().insert(&mut app);
    let row = |zone_id, name: &str| benilla_formats::AreaTableRow {
        map_id: 0,
        zone_id,
        explore_flag: 0,
        flags: 0,
        faction_group_mask: 0,
        exploration_level: 0,
        name: name.to_string(),
    };
    app.insert_resource(crate::area::AreaTableRes(
        benilla_formats::AreaTableCatalog::from_rows(vec![
            (ELWYNN, row(0, "Elwynn Forest")),
            (GOLDSHIRE, row(ELWYNN, "Goldshire")),
        ]),
    ));
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"UNKNOWN = "Unknown"
            CHAT_FLAG_AFK = "<AFK>"
            CHAT_FLAG_DND = "<DND>"
            UPDATES = 0
            local f = CreateFrame("Frame")
            f:RegisterEvent("FRIENDLIST_UPDATE")
            f:SetScript("OnEvent", function() UPDATES = UPDATES + 1 end)"#,
        )
        .expect("the recorder installs");
    app.world_mut().insert_non_send_resource(script);
    let feed = app.world_mut().register_system(super::feed::feed_social);
    let mut s = Social { app, rx, feed };
    feed_once(&mut s);
    run(&mut s, "UPDATES = 0");
    s
}

fn packets(s: &mut Social, events: Vec<SessionEvent>) {
    crate::net::handlers::dispatch(s.app.world_mut(), events);
}

fn feed_once(s: &mut Social) {
    s.app
        .world_mut()
        .run_system(s.feed)
        .expect("the social feed runs");
}

fn run(s: &mut Social, lua: &str) {
    try_run(s, lua).expect("the chunk runs");
}

fn try_run(s: &mut Social, lua: &str) -> Result<(), String> {
    with_hosted_vm(s.app.world_mut(), |vm| vm.expect("a VM").run(lua)).map_err(|e| e.to_string())
}

fn eval<T: benilla_ui::mlua::FromLuaMulti>(s: &mut Social, lua: &str) -> T {
    with_hosted_vm(s.app.world_mut(), |vm| vm.expect("a VM").eval::<T>(lua))
        .expect("the chunk runs")
}

fn updates(s: &mut Social) -> i64 {
    let n = eval(s, "return UPDATES");
    run(s, "UPDATES = 0");
    n
}

/// A friend or ignore packet, as the tests compare them.
#[derive(Debug, PartialEq, Eq)]
enum Sent {
    AddFriend(String),
    DelFriend(u64),
    FriendList,
    AddIgnore(String),
    DelIgnore(u64),
}

/// The friend and ignore packets sent since the last read, in order.
fn sent(s: &Social) -> Vec<Sent> {
    s.rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::AddFriend { name } => Some(Sent::AddFriend(name)),
            ClientCommand::DelFriend { guid } => Some(Sent::DelFriend(guid)),
            ClientCommand::FriendListRequest => Some(Sent::FriendList),
            ClientCommand::AddIgnore { name } => Some(Sent::AddIgnore(name)),
            ClientCommand::DelIgnore { guid } => Some(Sent::DelIgnore(guid)),
            _ => None,
        })
        .collect()
}

/// The keys of the lines queued since the last read, in order.
fn lines(s: &Social) -> Vec<&'static str> {
    s.app
        .world()
        .resource::<UiErrorKeys>()
        .take()
        .into_iter()
        .map(|e| e.key)
        .collect()
}

fn cache(s: &mut Social, guid: u64, name: &str) {
    s.app
        .world_mut()
        .resource_mut::<NameCache>()
        .insert_player(guid, name.to_string(), None);
}

/// `SMSG_NAME_QUERY_RESPONSE`; an empty name is the server's refusal.
fn answer(s: &mut Social, guid: u64, name: &str) {
    packets(
        s,
        vec![SessionEvent::PlayerName {
            guid,
            name: name.to_string(),
            race: 1,
            gender: 0,
            class: 1,
        }],
    );
}

fn offline(guid: u64) -> FriendEntry {
    FriendEntry {
        guid,
        ..Default::default()
    }
}

fn online(guid: u64, area: u32, level: u32, class: u32) -> FriendEntry {
    FriendEntry {
        guid,
        status: friend_status::ONLINE,
        area,
        level,
        class,
    }
}

fn list(s: &mut Social, friends: Vec<FriendEntry>) {
    packets(s, vec![SessionEvent::FriendList { friends }]);
}

fn status(s: &mut Social, result: u8, guid: u64, tail: Option<FriendOnline>) {
    packets(
        s,
        vec![SessionEvent::FriendStatus(FriendStatusUpdate {
            result,
            guid,
            online: tail,
        })],
    );
}

/// The names of rows 1 to `n`, `"nil"` for a slot with none.
fn names(s: &mut Social, n: u32) -> Vec<String> {
    (1..=n)
        .map(|i| eval::<Option<String>>(s, &format!("return GetFriendInfo({i})")))
        .map(|n| n.unwrap_or_else(|| "nil".into()))
        .collect()
}

/// `GetFriendInfo(index)`'s six answers, each as text, `"nil"` for nil.
fn info(s: &mut Social, index: i64) -> [String; 6] {
    let lua = format!(
        "local a, b, c, d, e, f = GetFriendInfo({index})
        return tostring(a), tostring(b), tostring(c), tostring(d), tostring(e), tostring(f)"
    );
    let (a, b, c, d, e, f): (String, String, String, String, String, String) = eval(s, &lua);
    [a, b, c, d, e, f]
}

/// The list frees every slot's name and fills one only from a cached name (`0x5ae35e`-`0x5ae388`,
/// `0x5ae438`-`0x5ae451`); `GetFriendInfo` pushes the null name as nil (`0x5ad0c2`).
#[test]
fn a_slot_whose_name_is_not_cached_answers_nil() {
    let mut s = client();
    list(&mut s, vec![offline(7)]);
    feed_once(&mut s);
    assert_eq!(eval::<i64>(&mut s, "return GetNumFriends()"), 1);
    assert_eq!(names(&mut s, 1), ["nil"], "not the empty string");
}

/// `FRIENDLIST_UPDATE` waits for the last pending name (`0x5ae475`-`0x5ae481`), whose callback
/// stores it and signals (`0x5aeac0`, `0x5aeb70` → `0x5aeb85`).
#[test]
fn the_list_signals_once_its_last_name_arrives() {
    let mut s = client();
    cache(&mut s, 8, "Bbb");
    list(&mut s, vec![offline(7), offline(8)]);
    feed_once(&mut s);
    assert_eq!(updates(&mut s), 0, "7's name is still pending");

    answer(&mut s, 7, "Aaa");
    feed_once(&mut s);
    assert_eq!(updates(&mut s), 1, "the answer signals the sorted list");
    assert_eq!(names(&mut s, 2), ["Aaa", "Bbb"]);
}

/// The comparator `0x5ae870`: empty slots last, online before offline (`0x5ae8b5`, `0x5ae8f4`),
/// then the names without case (`0x5ae8ce`).
#[test]
fn online_friends_sort_ahead_of_offline_ones() {
    let mut s = client();
    cache(&mut s, 7, "Aaa");
    cache(&mut s, 8, "bbb");
    cache(&mut s, 9, "Ccc");
    list(
        &mut s,
        vec![
            offline(7),
            online(9, ELWYNN, 10, 1),
            online(8, ELWYNN, 10, 1),
        ],
    );
    feed_once(&mut s);
    assert_eq!(updates(&mut s), 1);
    assert_eq!(names(&mut s, 3), ["bbb", "Ccc", "Aaa"]);
}

/// A status whose name misses counts it pending and returns unsignalled (`0x5acba6`-`0x5acbc4`);
/// the callback stores the name, signals and shows the line (`0x5acf0a`, `0x5acf15`, `0x5acf27`).
#[test]
fn an_add_whose_name_is_pending_waits_for_it() {
    let mut s = client();
    cache(&mut s, 7, "Aaa");
    list(&mut s, vec![offline(7)]);
    feed_once(&mut s);
    assert_eq!(updates(&mut s), 1);
    let _ = lines(&s);

    status(&mut s, friend_result::ADDED_OFFLINE, 8, None);
    feed_once(&mut s);
    assert_eq!(updates(&mut s), 0, "8's name is pending");
    assert!(lines(&s).is_empty(), "the line waits for the name");

    answer(&mut s, 8, "Bbb");
    feed_once(&mut s);
    assert_eq!(updates(&mut s), 1);
    assert_eq!(lines(&s), ["ERR_FRIEND_ADDED_S"]);
    assert_eq!(names(&mut s, 2), ["Aaa", "Bbb"]);
}

/// Result 8 claims a slot for the guid (`0x5acca0` → `0x5aea60`) and joins the add path. A cached
/// name is stored only for results 6 and 7 (`0x5acdbd`), so this row has none.
#[test]
fn already_your_friend_claims_a_slot() {
    let mut s = client();
    cache(&mut s, 7, "Bob");
    status(&mut s, friend_result::ALREADY, 7, None);
    feed_once(&mut s);
    assert_eq!(eval::<i64>(&mut s, "return GetNumFriends()"), 1);
    assert_eq!(
        updates(&mut s),
        1,
        "the add path sorts and signals (`0x5acdde`)"
    );
    assert_eq!(lines(&s), ["ERR_FRIEND_ALREADY_S"]);
    assert_eq!(names(&mut s, 1), ["nil"]);
}

/// The table at `0x5ace1c`: result 0 shares result 4's `ERR_FRIEND_NOT_FOUND` (`0x5acad6`), and
/// every result past 17 shows `ERR_FRIEND_ERROR` (`0x5acab9` → `0x5acd85`).
#[test]
fn the_result_table_maps_zero_and_the_codes_past_seventeen() {
    let mut s = client();
    for (result, key) in [
        (0x00, "ERR_FRIEND_NOT_FOUND"),
        (0x01, "ERR_FRIEND_LIST_FULL"),
        (0x12, "ERR_FRIEND_ERROR"),
        (0x1a, "ERR_FRIEND_ERROR"),
        (0xff, "ERR_FRIEND_ERROR"),
    ] {
        status(&mut s, result, 7, None);
        feed_once(&mut s);
        assert_eq!(lines(&s), [key], "result {result:#04x}");
    }
}

/// `0x5ae4b0` bounds `index − 1` unsigned at 50: outside it the tuple is `UNKNOWN`, 1, `UNKNOWN`,
/// `UNKNOWN`, nil, `""` (`0x5ad1c8`-`0x5ad1fd`); inside it an empty slot answers its zeros.
#[test]
fn get_friend_info_answers_past_the_count() {
    let mut s = client();
    cache(&mut s, 7, "Aaa");
    list(&mut s, vec![online(7, ELWYNN, 60, 4)]);
    feed_once(&mut s);
    let outside = ["Unknown", "1", "Unknown", "Unknown", "nil", ""].map(String::from);
    assert_eq!(info(&mut s, 0), outside);
    assert_eq!(info(&mut s, 52), outside);
    assert_eq!(info(&mut s, -3), outside);
    assert_eq!(
        info(&mut s, 2),
        ["nil", "0", "Unknown", "Unknown", "nil", ""].map(String::from),
        "an empty slot"
    );
    assert_eq!(
        info(&mut s, 1),
        ["Aaa", "60", "Rogue", "Elwynn Forest", "1", ""].map(String::from)
    );
}

/// The class and area lookups run whatever the online byte (`0x5ad0da`-`0x5ad151`, the byte
/// first read at `0x5ad15c`): an id with no row is `UNKNOWN`, and an area names its parent when the
/// parent has a row (`0x5ad127`-`0x5ad13f`).
#[test]
fn a_class_or_area_with_no_name_is_unknown() {
    let mut s = client();
    for (guid, name) in [(7, "Aaa"), (8, "Bbb"), (9, "Ccc")] {
        cache(&mut s, guid, name);
    }
    list(
        &mut s,
        vec![
            online(7, GOLDSHIRE, 60, 4),
            online(8, 999_999, 30, 200),
            offline(9),
        ],
    );
    feed_once(&mut s);
    assert_eq!(
        info(&mut s, 1),
        ["Aaa", "60", "Rogue", "Elwynn Forest", "1", ""].map(String::from),
        "Goldshire names its zone"
    );
    assert_eq!(
        info(&mut s, 2),
        ["Bbb", "30", "Unknown", "Unknown", "1", ""].map(String::from)
    );
    assert_eq!(
        info(&mut s, 3),
        ["Ccc", "0", "Unknown", "Unknown", "nil", ""].map(String::from),
        "the list stores zeros for an offline friend (`0x5ae415`)"
    );
}

/// Result 3 writes the online byte alone (`0x5acc5c` → `0x5aeb30`): the level, class and area stay,
/// and the away flag reads only for an online slot (`0x5ad176`).
#[test]
fn an_offline_broadcast_keeps_the_rest_of_the_slot() {
    let mut s = client();
    cache(&mut s, 7, "Aaa");
    list(&mut s, vec![offline(7)]);
    status(
        &mut s,
        friend_result::ONLINE,
        7,
        Some(FriendOnline {
            status: friend_status::AFK,
            area: ELWYNN,
            level: 42,
            class: 8,
        }),
    );
    feed_once(&mut s);
    assert_eq!(
        info(&mut s, 1),
        ["Aaa", "42", "Mage", "Elwynn Forest", "1", "<AFK>"].map(String::from)
    );
    status(&mut s, friend_result::OFFLINE, 7, None);
    feed_once(&mut s);
    assert_eq!(
        info(&mut s, 1),
        ["Aaa", "42", "Mage", "Elwynn Forest", "nil", ""].map(String::from)
    );
}

/// The name callback's refusal (`0x5ace94` → `0x5ace96`): the line unless the lookup was the
/// list's (`0x1d1`), the pending count down (`0x5aceb6`), and `CMSG_DEL_FRIEND` (`0x5acec8`).
#[test]
fn a_name_the_server_refuses_removes_the_friend() {
    let mut s = client();
    list(&mut s, vec![offline(7)]);
    feed_once(&mut s);
    let _ = sent(&s);
    answer(&mut s, 7, "");
    feed_once(&mut s);
    assert_eq!(sent(&s), [Sent::DelFriend(7)]);
    assert!(lines(&s).is_empty(), "the list's lookup shows nothing");
    assert_eq!(updates(&mut s), 1, "the last pending name signals");

    status(&mut s, friend_result::ADDED_OFFLINE, 8, None);
    answer(&mut s, 8, "");
    feed_once(&mut s);
    assert_eq!(sent(&s), [Sent::DelFriend(8)]);
    assert_eq!(lines(&s), ["ERR_FRIEND_ERROR"]);
}

/// `AddFriend` (`0x5ad290` → `0x5ae670`), `ShowFriends` (`0x5ad340`) and `RemoveFriend`
/// (`0x5ad2d0`) send inside their calls; a name `RemoveFriend` finds no slot for shows
/// `ERR_FRIEND_NOT_FOUND` (`0x5ae722`). `AddFriend` sends even an empty name.
#[test]
fn the_friend_verbs_send_inside_their_calls() {
    let mut s = client();
    cache(&mut s, 7, "Aaa");
    cache(&mut s, 8, "Bbb");
    list(
        &mut s,
        vec![online(7, ELWYNN, 1, 1), online(8, ELWYNN, 1, 1)],
    );
    feed_once(&mut s);
    let _ = sent(&s);

    run(&mut s, r#"AddFriend("Ccc") AddFriend("")"#);
    assert_eq!(
        sent(&s),
        [Sent::AddFriend("Ccc".into()), Sent::AddFriend("".into()),]
    );
    run(&mut s, "ShowFriends()");
    assert_eq!(sent(&s), [Sent::FriendList]);
    run(
        &mut s,
        r#"RemoveFriend(2) RemoveFriend("aAA") RemoveFriend("3")"#,
    );
    assert_eq!(
        sent(&s),
        [Sent::DelFriend(8), Sent::DelFriend(7),],
        "a numeric string is an index, and slot 3 is empty"
    );
    run(&mut s, r#"RemoveFriend("Nobody")"#);
    assert!(sent(&s).is_empty());
    assert_eq!(lines(&s), ["ERR_FRIEND_NOT_FOUND"]);

    let err = try_run(&mut s, "RemoveFriend({})").unwrap_err();
    assert!(
        err.contains(r#"Usage: RemoveFriend(["name"] or [index])"#),
        "{err}"
    );
    let err = try_run(&mut s, "AddFriend()").unwrap_err();
    assert!(err.contains(r#"Usage: AddFriend("name")"#), "{err}");
}

/// `SetSelectedFriend` stores the guid of slot `index − 1` up to 50 inclusive (`0x5ae4d6`), so
/// index 51 reads the selection itself back and keeps it, and past it clears; `GetSelectedFriend`
/// finds the slot holding the guid (`0x5ae510`).
#[test]
fn the_selection_is_a_guid_the_slots_are_searched_for() {
    let mut s = client();
    cache(&mut s, 7, "Aaa");
    cache(&mut s, 8, "Bbb");
    list(
        &mut s,
        vec![online(7, ELWYNN, 1, 1), online(8, ELWYNN, 1, 1)],
    );
    feed_once(&mut s);
    assert_eq!(
        eval::<i64>(&mut s, "SetSelectedFriend(2) return GetSelectedFriend()"),
        2
    );
    assert_eq!(
        eval::<i64>(&mut s, "SetSelectedFriend(51) return GetSelectedFriend()"),
        2,
        "slot 50 is the selection's own field"
    );
    assert_eq!(
        eval::<i64>(&mut s, "SetSelectedFriend(52) return GetSelectedFriend()"),
        0
    );
}

/// The ignore verbs match a listed guid's cached name (`0x5af1ef`, `0x5af345`, `0x5af447`):
/// `AddOrDelIgnore` removes a match or adds the name (`0x5af1a0`), `AddIgnore` refuses a match with
/// `ERR_IGNORE_ALREADY_S` (`0x5af3df`), and `DelIgnore` shows `ERR_IGNORE_NOT_FOUND` for none
/// (`0x5af471`); each inside its call.
#[test]
fn the_ignore_verbs_act_inside_their_calls() {
    let mut s = client();
    cache(&mut s, 9, "Spam");
    packets(&mut s, vec![SessionEvent::IgnoreList { guids: vec![9] }]);
    feed_once(&mut s);
    let _ = sent(&s);
    let _ = lines(&s);

    run(&mut s, r#"AddOrDelIgnore("sPAM") AddOrDelIgnore("Other")"#);
    assert_eq!(
        sent(&s),
        [Sent::DelIgnore(9), Sent::AddIgnore("Other".into()),]
    );
    run(&mut s, r#"AddIgnore("Spam") DelIgnore("Nope")"#);
    assert!(sent(&s).is_empty());
    assert_eq!(lines(&s), ["ERR_IGNORE_ALREADY_S", "ERR_IGNORE_NOT_FOUND"]);
    run(&mut s, r#"DelIgnore("spam") AddIgnore("Other")"#);
    assert_eq!(
        sent(&s),
        [Sent::DelIgnore(9), Sent::AddIgnore("Other".into()),]
    );
}
