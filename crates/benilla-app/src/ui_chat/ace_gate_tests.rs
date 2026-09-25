//! Ace2's initialisation gate, `AceEvent:IsFullyInitialized()` (`AceEvent-2.0.lua:913-947`),
//! opened by a zone-channel join. Of its three arming events, `CHAT_MSG_CHANNEL_NOTICE` (0.05 s),
//! `MEETINGSTONE_CHANGED` (1 s) and `LANGUAGE_LIST_CHANGED` then `MINIMAP_ZONE_CHANGED` (1 s),
//! only the notice fires at login on a 1.12 server. Run against the addon corpus's own Ace2 chain
//! over our whole FrameXML; skipped where the corpus is absent.

use std::path::Path;

use benilla_ui::script::UiScript;

use super::event::{ChatEvent, ChatEventKind as K};
use super::frames::{route, ChatWindows};

/// The addon corpus, or a skip where it is absent.
use benilla_formats::addon_corpus_or_skip as corpus_or_skip;

/// FuBar's `.toc` load order for the gate's four files (`FuBar/FuBar.toc:16-19`).
const ACE_CHAIN: &[&str] = &[
    "FuBar/libs/AceLibrary/AceLibrary.lua",
    "FuBar/libs/Compost-2.0/Compost-2.0.lua",
    "FuBar/libs/AceOO-2.0/AceOO-2.0.lua",
    "FuBar/libs/AceEvent-2.0/AceEvent-2.0.lua",
];

/// A VM with our FrameXML and the corpus's Ace2 chain, past `PLAYER_LOGIN`.
fn ace_vm(root: &Path) -> UiScript {
    let mut script = UiScript::new().expect("VM");
    script.set_screen_size(1024.0, 768.0);
    // Ace calls `CreateFrame`, `GetTime` and `ChatTypeInfo`, so the whole interface loads first.
    let failures = crate::ui_script::load_default_ui(&script);
    assert!(
        failures.is_empty(),
        "our own FrameXML failed to load: {failures:#?}"
    );
    for rel in ACE_CHAIN {
        let path = root.join(rel);
        let src = benilla_ui::source::decode(&std::fs::read(&path).unwrap_or_else(|e| {
            panic!("{}: {e}", path.display());
        }))
        .into_owned();
        script.run(&src).unwrap_or_else(|e| panic!("{rel}: {e}"))
    }
    // AceEvent registers `PLAYER_LOGIN` in `activate` and sets `playerLogin` on it.
    script.fire_event("PLAYER_LOGIN", vec![]);
    script
}

/// Whether `AceEvent:IsFullyInitialized()` answers true.
fn gate_open(s: &UiScript) -> bool {
    s.eval::<bool>("return AceLibrary('AceEvent-2.0'):IsFullyInitialized() and true or false")
        .expect("AceEvent-2.0 is registered")
}

/// A zone-channel join's `YOU_JOINED` notice, sent through the real router, opens Ace2's gate.
#[test]
fn a_you_joined_notice_opens_ace2s_initialisation_gate() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = ace_vm(&root);
    let mut windows = ChatWindows::default();

    // A once-only event (`eventsWhichHappenOnce`, `AceEvent-2.0.lua:79-84`): the count is 0 or 1.
    s.run(
        r#"
        AceGateFired = 0
        AceLibrary("AceEvent-2.0"):RegisterEvent("AceEvent_FullyInitialized", function()
            AceGateFired = AceGateFired + 1
        end)
    "#,
    )
    .unwrap();

    assert!(
        !gate_open(&s),
        "before any notice the gate is shut — that IS the bug this closes"
    );

    // The notice our zone auto-join provokes: the server's YOU_JOINED for "General - <zone>".
    let mut joined = ChatEvent::text_only(K::ChannelNotice, String::new());
    joined.notice = "2".into(); // channel_notice::YOU_JOINED
    joined.channel = "General - Elwynn Forest".into();
    joined.channel_base = "General - Elwynn Forest".into();
    joined.channel_number = 1;
    joined.zone_channel_id = 1; // ChatChannels.dbc General
    route(&mut s, &mut windows, &joined);

    // The open runs 0.05 s later, from AceEvent's OnUpdate (`AceEvent-2.0.lua:938`, `:471`).
    assert!(!gate_open(&s), "still shut inside the 0.05 s delay");
    s.tick(0.02);
    assert!(!gate_open(&s), "0.02 s is not 0.05 s");
    s.tick(0.05);

    assert!(
        gate_open(&s),
        "AceEvent:IsFullyInitialized() must be true once the notice's delay elapses"
    );
    assert_eq!(
        s.eval::<i64>("return AceGateFired").unwrap(),
        1,
        "AceEvent_FullyInitialized fired exactly once"
    );
    assert!(s.errors().is_empty(), "Lua errors: {:?}", s.errors());
}

/// The control for the test above: with no notice, nothing else arms the gate.
#[test]
fn without_the_notice_the_gate_stays_shut() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = ace_vm(&root);
    for _ in 0..40 {
        s.tick(0.1); // 4 s in all, past both the 0.05 s and the 1 s schedules
    }
    assert!(
        !gate_open(&s),
        "nothing but CHAT_MSG_CHANNEL_NOTICE can honestly arm this gate on a 1.12 server"
    );
}

/// The auto-join's names for a zone and a capital, which the server matches to its channel rows.
#[test]
fn the_auto_join_walk_names_the_channels_the_server_resolves() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = benilla_formats::load_chat_channels_catalog(&mut chain).expect("ChatChannels.dbc");

    // The city word: `AreaTable.dbc` row 3459, `Flags & 0x200`, cached at `0xb4e4f0`.
    let areas = benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable.dbc");
    let city = super::channels::city_word(&areas);
    assert_eq!(
        city,
        Some("City"),
        "the 0x200 sentinel row names the channel"
    );

    let out_in_the_world =
        super::channels::wanted_channels(&cat, 0x0020_0003, "Elwynn Forest", false, city);
    assert_eq!(
        out_in_the_world,
        vec!["General - Elwynn Forest", "LocalDefense - Elwynn Forest"]
    );
    let in_a_capital =
        super::channels::wanted_channels(&cat, 0x0020_0003, "Stormwind City", true, city);
    assert_eq!(
        in_a_capital,
        vec![
            "General - Stormwind City",
            "Trade - City",
            "LocalDefense - Stormwind City",
        ]
    );
    // Each resolves to a built-in row, so vmangos treats them as the constant channels, not as
    // custom ones (`GetChannelEntryFor`, `DBCStores.cpp:531`).
    for name in out_in_the_world.iter().chain(in_a_capital.iter()) {
        assert_ne!(
            cat.zone_channel_id(name),
            0,
            "{name} must resolve to a ChatChannels.dbc row"
        );
    }
}
