//! `/who` and the looking-for-group verbs through the whole client's host: each verb acts inside
//! its call, as `SendWho` (`0x5ad3b0` → `0x5aebb0`, the send at `0x5af168`), `SortWho`
//! (`0x5ad890`), `SetWhoToUI` (`0x5ad870`), `LFGQuery` (`0x4e9760`, the send at `0x4e9840`) and
//! `SetLookingForGroup` (`0x4e96b0` → `0x4e88c0`) do; the answer's rows read `UNKNOWN` for an id
//! with no row (`GetWhoInfo`, `0x5ad6e0`); and the sort chain is seeded again at each world entry
//! (`0x5adc50`, only while the list `0xc28168` is null).

use benilla_protocol::messages::{WhoEntry, WhoResults};
use benilla_protocol::SessionEvent;
use bevy::ecs::system::SystemId;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use benilla_ui::script::{UiScript, WhoSortChain};

use super::SocialState;
use crate::net::{ClientCommand, NetCommands};
use crate::script_calls::with_hosted_vm;

const ELWYNN: u32 = 12;

struct Who {
    app: App,
    rx: Receiver<ClientCommand>,
    feed: SystemId,
}

/// The whole client, headless, with the stock class and race names, one zone, and a VM that
/// defines the enUS tags and `UNKNOWN`.
fn client() -> Who {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    crate::ui_unit::TestNames::english().insert(&mut app);
    app.insert_resource(crate::area::AreaTableRes(
        benilla_formats::AreaTableCatalog::from_rows(vec![(
            ELWYNN,
            benilla_formats::AreaTableRow {
                map_id: 0,
                zone_id: 0,
                explore_flag: 0,
                flags: 0,
                faction_group_mask: 0,
                exploration_level: 0,
                name: "Elwynn Forest".to_string(),
            },
        )]),
    ));
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"UNKNOWN = "Unknown"
            WHO_TAG_NAME = "n-" WHO_TAG_GUILD = "g-" WHO_TAG_ZONE = "z-"
            WHO_TAG_RACE = "r-" WHO_TAG_CLASS = "c-""#,
        )
        .expect("the strings install");
    app.world_mut().insert_non_send_resource(script);
    let feed = app.world_mut().register_system(super::feed::feed_social);
    let mut w = Who { app, rx, feed };
    w.app
        .world_mut()
        .run_system(w.feed)
        .expect("the social feed runs");
    w.rx.try_iter().for_each(drop);
    w
}

fn try_run(w: &mut Who, lua: &str) -> Result<(), String> {
    with_hosted_vm(w.app.world_mut(), |vm| vm.expect("a VM").run(lua)).map_err(|e| e.to_string())
}

fn run(w: &mut Who, lua: &str) {
    try_run(w, lua).expect("the chunk runs");
}

fn eval<T: benilla_ui::mlua::FromLuaMulti>(w: &mut Who, lua: &str) -> T {
    with_hosted_vm(w.app.world_mut(), |vm| vm.expect("a VM").eval::<T>(lua))
        .expect("the chunk runs")
}

/// What the chunks sent so far, in order, with nothing else run.
fn sent(w: &Who) -> Vec<ClientCommand> {
    w.rx.try_iter().collect()
}

fn social(w: &Who) -> &SocialState {
    w.app.world().resource::<SocialState>()
}

/// The `CMSG_WHO` body for levels `min..max` and the name `name`, every other field its default.
fn who_body(min: u32, max: u32, name: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(min.to_le_bytes());
    b.extend(max.to_le_bytes());
    b.extend(name);
    b.extend([0, 0]);
    b.extend([0xff; 8]);
    b.extend(0u32.to_le_bytes());
    b.extend(0u32.to_le_bytes());
    b
}

/// `SendWho` builds and sends its query before it returns, so a chunk's packets leave in call
/// order: the who query, then the roll after it.
#[test]
fn send_who_sends_its_query_inside_the_call() {
    let mut w = client();
    run(&mut w, r#"SendWho("n-bob 60") RandomRoll(1, 100)"#);
    let first = sent(&w);
    assert_eq!(first.len(), 2, "{first:?}");
    let ClientCommand::Who { request } = &first[0] else {
        panic!("the who query first: {first:?}");
    };
    assert_eq!(
        benilla_protocol::messages::who(request),
        who_body(60, 60, b"bob")
    );
    assert!(
        matches!(first[1], ClientCommand::RandomRoll { min: 1, max: 100 }),
        "{first:?}"
    );
    // `lua_tostring` of a number (`0x5ad3e3`): its text, a level.
    run(&mut w, "SendWho(5)");
    let second = sent(&w);
    let [ClientCommand::Who { request }] = second.as_slice() else {
        panic!("{second:?}");
    };
    assert_eq!(
        benilla_protocol::messages::who(request),
        who_body(5, 5, b"")
    );
}

/// `lua_isstring` on the filter, else the usage (`0x85daac`, `0x5ad3c7`), and nothing is sent.
#[test]
fn send_who_without_a_filter_raises_its_usage() {
    let mut w = client();
    for call in ["SendWho()", "SendWho(nil)", "SendWho({})"] {
        let err = try_run(&mut w, call).expect_err(call);
        assert!(err.contains(r#"Usage: SendWho("filter")"#), "{call}: {err}");
    }
    assert!(sent(&w).is_empty());
}

/// `SortWho` promotes the engine's chain (`0x5ad980`) before it returns.
#[test]
fn sort_who_promotes_the_engines_chain_inside_its_call() {
    let mut w = client();
    run(&mut w, r#"SortWho("level")"#);
    let mut level = WhoSortChain::default();
    level.promote("level");
    assert_eq!(social(&w).who_sort, level);
}

/// `SetWhoToUI(1)` sends the next answer, three rows or fewer, to `WHO_LIST_UPDATE` and not to
/// chat (`0x5adf9c`-`0x5adfce`); `SetWhoToUI(0)` sends it back to chat.
#[test]
fn set_who_to_ui_routes_the_next_answer() {
    let mut w = client();
    run(
        &mut w,
        r#"UPDATES = 0
        local f = CreateFrame("Frame")
        f:RegisterEvent("WHO_LIST_UPDATE")
        f:SetScript("OnEvent", function() UPDATES = UPDATES + 1 end)"#,
    );
    let answer = |w: &mut Who| {
        crate::net::handlers::dispatch(
            w.app.world_mut(),
            vec![SessionEvent::WhoResults(WhoResults {
                displayed: 1,
                total: 1,
                entries: vec![entry("Solo", 1, 1, ELWYNN)],
            })],
        );
        w.app
            .world_mut()
            .run_system(w.feed)
            .expect("the social feed runs");
        let n: i64 = eval(w, "return UPDATES");
        run(w, "UPDATES = 0");
        n
    };
    run(&mut w, "SetWhoToUI(1)");
    assert_eq!(answer(&mut w), 1, "the frame's");
    run(&mut w, "SetWhoToUI(0)");
    assert_eq!(answer(&mut w), 0, "chat's");
}

/// `LFGQuery` sends `MSG_LOOKING_FOR_GROUP` in the call (`0x4e9840`), `SetLookingForGroup` its
/// slots and comment (`0x4e88c0`).
#[test]
fn the_lfg_verbs_send_inside_their_call() {
    let mut w = client();
    {
        let mut script = w.app.world_mut().non_send_resource_mut::<UiScript>();
        script.set_lfg_dungeons(vec![benilla_ui::script::LfgDungeon {
            name: "Deadmines".into(),
            min_level: 15,
            max_level: 28,
            kind: 1,
            side: -1,
        }]);
        script.set_unit(
            "player",
            Some(benilla_ui::script::UnitState {
                exists: true,
                level: 20,
                ..Default::default()
            }),
        );
    }
    run(&mut w, "LFGQuery(1, 0, 7)");
    let sent_now = sent(&w);
    assert!(
        matches!(
            sent_now.as_slice(),
            [ClientCommand::LfgQuery {
                criterion: 0,
                class: 7
            }]
        ),
        "{sent_now:?}"
    );
    run(&mut w, r#"SetLookingForGroup(1, 3, 3, 12, 5, 0, "LF2M")"#);
    let sent_now = sent(&w);
    let [ClientCommand::SetLookingForGroup { slots, comment }] = sent_now.as_slice() else {
        panic!("{sent_now:?}");
    };
    assert_eq!((*slots, comment.as_str()), ([0; 3], "LF2M"));
}

fn entry(name: &str, race: u32, class: u32, zone: u32) -> WhoEntry {
    WhoEntry {
        name: name.to_string(),
        guild: String::new(),
        level: 60,
        class,
        race,
        zone,
    }
}

/// `GetWhoInfo` reads the localized name of each id, or `UNKNOWN` for one with no row
/// (`0x5ad782`, `0x5ad7c5`, `0x5ad816`).
#[test]
fn get_who_info_answers_unknown_for_an_id_with_no_row() {
    let mut w = client();
    let entries = vec![
        entry("Known", 1, 1, ELWYNN),
        entry("Lost", 200, 200, 999_999),
        entry("Third", 1, 1, ELWYNN),
        entry("Fourth", 1, 1, ELWYNN),
    ];
    crate::net::handlers::dispatch(
        w.app.world_mut(),
        vec![SessionEvent::WhoResults(WhoResults {
            displayed: 4,
            total: 4,
            entries,
        })],
    );
    w.app
        .world_mut()
        .run_system(w.feed)
        .expect("the social feed runs");
    let row = |w: &mut Who, name: &str| -> (String, String, String) {
        eval(
            w,
            &format!(
                r#"for i = 1, GetNumWhoResults() do
                    local n, _, _, race, class, zone = GetWhoInfo(i)
                    if n == "{name}" then return race, class, zone end
                end"#
            ),
        )
    };
    assert_eq!(
        row(&mut w, "Lost"),
        ("Unknown".into(), "Unknown".into(), "Unknown".into())
    );
    assert_eq!(
        row(&mut w, "Known"),
        ("Human".into(), "Warrior".into(), "Elwynn Forest".into())
    );
}

/// The world teardown frees the list (`0x5ae2d0` at `0x401fc6`), so the next world entry seeds the
/// chain again (`0x5add04`).
#[test]
fn a_logout_seeds_the_sort_chain_again() {
    let mut w = client();
    w.app
        .world_mut()
        .resource_mut::<SocialState>()
        .who_sort
        .promote("level");
    assert_ne!(social(&w).who_sort, WhoSortChain::default());
    crate::net::handlers::dispatch(w.app.world_mut(), vec![SessionEvent::LoggedOut]);
    assert_eq!(social(&w).who_sort, WhoSortChain::default());
}

/// One `SMSG_WHO` answer through its handler, then the feed.
fn answer(w: &mut Who, displayed: u32, total: u32, entries: Vec<WhoEntry>) {
    crate::net::handlers::dispatch(
        w.app.world_mut(),
        vec![SessionEvent::WhoResults(WhoResults {
            displayed,
            total,
            entries,
        })],
    );
    w.app
        .world_mut()
        .run_system(w.feed)
        .expect("the social feed runs");
}

fn levelled(name: &str, level: u32) -> WhoEntry {
    WhoEntry {
        level,
        ..entry(name, 1, 1, ELWYNN)
    }
}

type WhoRow = (
    i64,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// `GetWhoInfo(index)`'s values, their count first.
fn who_info(w: &mut Who, index: i64) -> WhoRow {
    let call = format!("GetWhoInfo({index})");
    let count = with_hosted_vm(w.app.world_mut(), |vm| vm.expect("a VM").arity(&call))
        .expect("the call runs") as i64;
    let (name, guild, level, race, class, zone) = eval(w, &format!("return {call}"));
    (count, name, guild, level, race, class, zone)
}

fn miss() -> WhoRow {
    (6, None, None, Some(0), None, None, None)
}

/// The parser keeps the count capped at 50 (`0x5adf87`-`0x5adf92`) and stores the first 50
/// records (`0x5ae045`); `GetNumWhoResults` answers that count and the total.
#[test]
fn an_answer_keeps_at_most_fifty_rows() {
    let mut w = client();
    let entries = (0..60).map(|i| levelled(&format!("P{i:02}"), 60)).collect();
    answer(&mut w, 60, 60, entries);
    assert_eq!(
        eval::<(i64, i64)>(&mut w, "return GetNumWhoResults()"),
        (50, 60)
    );
    // Index 51 is the slot after the array, which the reference reads past its end.
    assert_eq!(who_info(&mut w, 51), miss());
}

/// The count line takes the answer's first dword, the raw count, both as the number and as the
/// plural's selector (`0x5ae0f1`-`0x5ae0fb`), never the server's total.
#[test]
fn the_count_line_prints_the_raw_count() {
    let mut w = client();
    crate::net::handlers::dispatch(
        w.app.world_mut(),
        vec![SessionEvent::WhoResults(WhoResults {
            displayed: 1,
            total: 7,
            entries: vec![levelled("Solo", 1)],
        })],
    );
    let get = |key: &str| match key {
        "WHO_LIST_FORMAT" => Some("%s %s %d %s %s - %s".to_string()),
        "WHO_NUM_RESULTS" => Some("%d player total".to_string()),
        "WHO_NUM_RESULTS_P1" => Some("%d players total".to_string()),
        "UNKNOWN" => Some("Unknown".to_string()),
        _ => None,
    };
    let names = crate::ui_unit::TestNames::english();
    let lines: Vec<String> = super::feed::printed_lines(social(&w), None, names.names(), &get)
        .iter()
        .map(|s| s.text().to_string())
        .collect();
    assert_eq!(lines.last().map(String::as_str), Some("1 player total"));
}

/// `GetWhoInfo` takes `index - 1` unsigned against the kept count with `ja` (`0x5ad71a`-`0x5ad71f`),
/// so 0, a negative index and anything past the count plus one miss with `nil, nil, 0, nil, nil,
/// nil` (`0x5ad836`-`0x5ad862`).
#[test]
fn get_who_info_misses_with_a_zero_level() {
    let mut w = client();
    answer(&mut w, 1, 1, vec![levelled("Solo", 1)]);
    for index in [0, -1, 3, 99] {
        assert_eq!(who_info(&mut w, index), miss(), "GetWhoInfo({index})");
    }
}

/// The index one past the count reads the array's next slot (`0x5ad726`): zeros before any answer
/// filled it (an empty name, level 0 and ids with no row), else what an earlier, longer answer
/// left there in its sorted order. The array outlives the session; only the count is reset
/// (`0x5add18`).
#[test]
fn the_index_past_the_count_reads_the_next_slot() {
    let mut w = client();
    let unknown = || Some("Unknown".to_string());
    assert_eq!(
        who_info(&mut w, 1),
        (
            6,
            Some(String::new()),
            Some(String::new()),
            Some(0),
            unknown(),
            unknown(),
            unknown()
        )
    );
    let name = |w: &mut Who, index: i64| who_info(w, index).1;
    let three = || vec![levelled("Aa", 30), levelled("Bb", 10), levelled("Cc", 20)];
    // Sorted zone, then level: Bb, Cc, Aa. One row over it leaves Cc and Aa behind it.
    answer(&mut w, 3, 3, three());
    answer(&mut w, 1, 1, vec![levelled("Zz", 5)]);
    assert_eq!(name(&mut w, 2).as_deref(), Some("Cc"));
    assert_eq!(who_info(&mut w, 3), miss());
    // A click re-sorts the array itself (`0x5ad9d6`): by name, Aa, Bb, Cc.
    answer(&mut w, 3, 3, three());
    run(&mut w, r#"SortWho("name")"#);
    answer(&mut w, 1, 1, vec![levelled("Zz", 5)]);
    assert_eq!(name(&mut w, 2).as_deref(), Some("Bb"));
    // After a logout the count is 0 and the array's first slot still holds Zz.
    crate::net::handlers::dispatch(w.app.world_mut(), vec![SessionEvent::LoggedOut]);
    w.app
        .world_mut()
        .run_system(w.feed)
        .expect("the social feed runs");
    assert_eq!(
        eval::<(i64, i64)>(&mut w, "return GetNumWhoResults()"),
        (0, 0)
    );
    assert_eq!(name(&mut w, 1).as_deref(), Some("Zz"));
}
