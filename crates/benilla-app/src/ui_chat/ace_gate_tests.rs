//! **Ace2's initialisation gate, opened end to end** — the reason 0288 §1's chat-event fire and
//! 0288 phase 6's zone auto-join were worth doing together.
//!
//! ## The failure this closes
//!
//! `AceEvent:IsFullyInitialized()` is called by 85 of the 218 addons in the vanilla corpus, and
//! under benilla it answered **false forever** — with no Lua error anywhere, which is why it went
//! unnoticed. The consequences are all silent: RosterLib never scans, BigWigs and oRA2 are
//! complete no-ops, Jostle never repositions, AceComm refuses to join its channels, and FuBar's
//! bar stays permanently blank across ~75 `FuBar_*` plugins.
//!
//! The gate is `AceEvent-2.0.lua:913-947`. On a fresh load `self.postInit` is false, and exactly
//! three things can set it:
//!
//! | armed by | what it schedules |
//! |---|---|
//! | `CHAT_MSG_CHANNEL_NOTICE` | `func` after 0.05 s |
//! | `MEETINGSTONE_CHANGED` | `func` after 1 s |
//! | `LANGUAGE_LIST_CHANGED` → `MINIMAP_ZONE_CHANGED` | `func` after 1 s |
//!
//! `func` sets `postInit = true` and fires `AceEvent_FullyInitialized`. Of the three arming
//! events, `CHAT_MSG_CHANNEL_NOTICE` is the one that can honestly fire at login on a 1.12 server —
//! it is the server's answer to joining a channel. benilla fired **no** `CHAT_MSG_*` at all, and
//! joined **no** channels, so none of the three ever arrived.
//!
//! ## What this file proves, and how
//!
//! Against the corpus's own Ace2 chain, loaded in FuBar's `.toc` order into a VM with our whole
//! FrameXML under it — not a stub, because the thing under test is third-party library code and a
//! stub would only ever prove that our stub works.
//!
//! **Nothing from the corpus is committed**, and the tests skip cleanly on a machine without it
//! (see [`corpus`]) — a checkout with no addon folder must never go red here.

use std::path::Path;

use benilla_ui::script::UiScript;

use super::event::{ChatEvent, ChatEventKind as K};
use super::frames::{route, ChatWindows};

/// The corpus, or a skip — `benilla_formats::addon_corpus_or_skip!`, the one resolver (its doc
/// carries the incident: five copies of a ladder that looked beside the checkout, and thirty
/// tests that skipped at every land once the pool moved).
use benilla_formats::addon_corpus_or_skip as corpus_or_skip;

/// FuBar's `.toc` load order for the four files the gate lives in (`FuBar/FuBar.toc:16-19`).
/// AceEvent is last because it needs all three under it — AceLibrary to register into, Compost for
/// its table recycling, AceOO for the class it derives from.
const ACE_CHAIN: &[&str] = &[
    "FuBar/libs/AceLibrary/AceLibrary.lua",
    "FuBar/libs/Compost-2.0/Compost-2.0.lua",
    "FuBar/libs/AceOO-2.0/AceOO-2.0.lua",
    "FuBar/libs/AceEvent-2.0/AceEvent-2.0.lua",
];

/// A VM with our whole FrameXML and the corpus's Ace2 chain in it, at the point a real session
/// reaches `PLAYER_LOGIN` — which is where `AceEvent` starts waiting.
fn ace_vm(root: &Path) -> UiScript {
    let mut script = UiScript::new().expect("VM");
    script.set_screen_size(1024.0, 768.0);
    // Our interface, exactly as the app loads it — Ace calls `CreateFrame`, `GetTime` and
    // `ChatTypeInfo` as readily as it calls anything of its own.
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
    // The UI-init tail the app runs once every file has loaded (`ui_script::finish_ui_load`'s
    // second half): AceEvent registers PLAYER_LOGIN in `activate` and flips `playerLogin` on it.
    script.fire_event("PLAYER_LOGIN", vec![]);
    script
}

/// Whether `AceEvent:IsFullyInitialized()` answers true.
fn gate_open(s: &UiScript) -> bool {
    s.eval::<bool>("return AceLibrary('AceEvent-2.0'):IsFullyInitialized() and true or false")
        .expect("AceEvent-2.0 is registered")
}

/// **The headline.** A zone-channel join's `YOU_JOINED` notice opens Ace2's gate.
///
/// The event goes in through the real router, so what is being tested is benilla's own chat path
/// end to end — not a hand-rolled `fire_event`.
#[test]
fn a_you_joined_notice_opens_ace2s_initialisation_gate() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = ace_vm(&root);
    let mut windows = ChatWindows::default();

    // An addon's-eye view of the gate opening: AceEvent fires `AceEvent_FullyInitialized` at its
    // own registry, which is how every FuBar plugin learns it may start.
    // The library-level registration AceEvent documents for a bare function (`:87-91` — called on
    // AceEvent itself, `self` becomes the handler). `AceEvent_FullyInitialized` is in its
    // `eventsWhichHappenOnce` table (`:79-84`), so it is a once-only registration by construction:
    // the count below can only ever be 0 or 1, which is what makes 1 mean something.
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

    // AceEvent does not open on the event itself — it schedules `func` 0.05 s out and lets its own
    // OnUpdate drive it (`AceEvent-2.0.lua:938`, `:471`). So the clock has to move.
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

/// The control, and the reason the headline is not circular: **without the notice the gate stays
/// shut**, no matter how much time passes. If this ever goes green, something else is arming the
/// gate and the headline is measuring the wrong thing.
#[test]
fn without_the_notice_the_gate_stays_shut() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = ace_vm(&root);
    for _ in 0..40 {
        s.tick(0.1); // four seconds — well past both the 0.05 s and the 1 s schedules
    }
    assert!(
        !gate_open(&s),
        "nothing but CHAT_MSG_CHANNEL_NOTICE can honestly arm this gate on a 1.12 server"
    );
}

/// The other half: the walk that makes the notice happen at all. Composed against the corpus's
/// own expectations is not possible here (the corpus does not know our zone), so this asserts the
/// names the auto-join sends for a real zone and a real capital — the strings the server matches
/// to `ChatChannels.dbc` rows, and therefore the strings that decide whether we are in the same
/// channel as everyone else.
#[test]
fn the_auto_join_walk_names_the_channels_the_server_resolves() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = benilla_formats::load_chat_channels_catalog(&mut chain).expect("ChatChannels.dbc");

    // The city word is DBC data, so read it the way the walk does rather than spelling it here —
    // that this resolves to "City" at all is the finding (`AreaTable.dbc` row 3459,
    // `Flags & 0x200`, the row `0xb4e4f0` caches).
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
    // Every one of them resolves back to a built-in row — i.e. vmangos will treat them as the
    // constant channels, not as custom ones (`GetChannelEntryFor`, DBCStores.cpp:531).
    for name in out_in_the_world.iter().chain(in_a_capital.iter()) {
        assert_ne!(
            cat.zone_channel_id(name),
            0,
            "{name} must resolve to a ChatChannels.dbc row"
        );
    }
}
