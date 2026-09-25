use super::event::{default_color, ChatEvent, ChatEventKind as K};
use super::input::{emote_send_eligible, emote_target, EmoteGate, ParsedChat};

thread_local! {
    /// The shipped `GlobalStrings.lua`, run in a VM once per test thread; built lazily, so a
    /// caller's `wow_data_or_skip!()` runs first.
    static GLOBAL_STRINGS: benilla_ui::script::UiScript = {
        let s = benilla_ui::script::UiScript::new().expect("VM");
        crate::ui_script::test_ui::load_ui(&s, "Interface\\FrameXML\\GlobalStrings.lua");
        s
    };
}

/// Run `f` with a key lookup into the shipped string table.
fn with_strings<T>(f: impl FnOnce(&dyn Fn(&str) -> Option<String>) -> T) -> T {
    GLOBAL_STRINGS.with(|s| f(&|key: &str| s.lua().globals().get::<String>(key).ok()))
}

/// [`super::frames::compose`] against the shipped table, for this file and [`super::broadcast`].
pub(super) fn compose(event: &ChatEvent, kind: K, default_language: &str) -> Option<String> {
    GLOBAL_STRINGS.with(|s| {
        super::frames::compose(event, kind, default_language, &|key| {
            s.lua().globals().get::<String>(key).ok()
        })
    })
}

/// A player-line event, as the wire bridge builds it.
fn ev(kind: K, text: &str, sender: &str) -> ChatEvent {
    ChatEvent {
        kind: Some(kind),
        text: text.into(),
        sender: sender.into(),
        ..Default::default()
    }
}

#[test]
fn player_lines_link_the_name_except_emote() {
    let _data = benilla_formats::wow_data_or_skip!();
    // The `|Hplayer` link of `ChatFrame.lua:1451`.
    assert_eq!(
        compose(&ev(K::Say, "hi there", "Bob"), K::Say, "Common").unwrap(),
        "|Hplayer:Bob|h[Bob]|h says: hi there"
    );
    assert_eq!(
        compose(
            &ev(K::WhisperInform, "hey", "Bob"),
            K::WhisperInform,
            "Common"
        )
        .unwrap(),
        "To |Hplayer:Bob|h[Bob]|h: hey"
    );
    // EMOTE takes the bare name (`ChatFrame.lua:1450`).
    assert_eq!(
        compose(&ev(K::Emote, "dances.", "Bob"), K::Emote, "Common").unwrap(),
        "Bob dances."
    );
}

#[test]
fn group_prefixed_kinds_wear_their_brackets() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        compose(&ev(K::Party, "inc 3", "Ann"), K::Party, "Common").unwrap(),
        "[Party] |Hplayer:Ann|h[Ann]|h: inc 3"
    );
    assert_eq!(
        compose(&ev(K::Guild, "gz", "Ann"), K::Guild, "Common").unwrap(),
        "[Guild] |Hplayer:Ann|h[Ann]|h: gz"
    );
    assert_eq!(
        compose(&ev(K::RaidWarning, "move", "Ann"), K::RaidWarning, "Common").unwrap(),
        "[Raid Warning] |Hplayer:Ann|h[Ann]|h: move"
    );
}

#[test]
fn flags_prefix_the_name_and_afk_uses_its_get() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut e = ev(K::Say, "brb", "Bob");
    e.flag = "GM".into();
    assert_eq!(
        compose(&e, K::Say, "Common").unwrap(),
        "<GM>|Hplayer:Bob|h[Bob]|h says: brb"
    );
    assert_eq!(
        compose(&ev(K::Afk, "farming", "Bob"), K::Afk, "Common").unwrap(),
        "|Hplayer:Bob|h[Bob]|h is Away From Keyboard: farming"
    );
}

#[test]
fn language_header_rides_non_default_tongues() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut e = ev(K::Say, "throm-ka", "Grunk");
    e.language = "Orcish".into();
    assert_eq!(
        compose(&e, K::Say, "Common").unwrap(),
        "|Hplayer:Grunk|h[Grunk]|h says: [Orcish] throm-ka"
    );
    // The frame's default tongue renders no header.
    e.language = "Common".into();
    assert_eq!(
        compose(&e, K::Say, "Common").unwrap(),
        "|Hplayer:Grunk|h[Grunk]|h says: throm-ka"
    );
}

#[test]
fn system_and_loot_lines_are_verbatim() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        compose(
            &ChatEvent::text_only(K::System, "Additem: Wool Cloth added.".into()),
            K::System,
            "Common"
        )
        .unwrap(),
        "Additem: Wool Cloth added."
    );
    // A LOOT line arrives composed, item link and all; its escapes pass through untouched.
    assert_eq!(
        compose(
            &ChatEvent::text_only(
                K::Loot,
                "You receive loot: |cffffffff|Hitem:117:0:0:0|h[Tough Jerky]|h|r.".into()
            ),
            K::Loot,
            "Common"
        )
        .unwrap(),
        "You receive loot: |cffffffff|Hitem:117:0:0:0|h[Tough Jerky]|h|r."
    );
}

/// The performer rides in `sender` (arg2, for addons) without the composer bracketing it on.
#[test]
fn text_emote_lines_are_verbatim_and_never_wear_the_senders_name() {
    let _data = benilla_formats::wow_data_or_skip!();
    let e = ev(K::TextEmote, "Bob waves at you.", "Bob");
    assert_eq!(
        compose(&e, K::TextEmote, "Common").unwrap(),
        "Bob waves at you."
    );
    // The control: a SAY does get the bracketed link.
    assert!(compose(&ev(K::Say, "hi", "Bob"), K::Say, "Common")
        .unwrap()
        .contains("[Bob]"));
}

/// `DoEmote` sends a self-target as guid 0 (`0x5ef611`), so 1.12 has no self-emote sentence.
#[test]
fn emoting_at_your_own_selection_sends_an_untargeted_emote() {
    let _data = benilla_formats::wow_data_or_skip!();
    use crate::target::Selection;
    use bevy::prelude::Entity;

    let me = Entity::from_raw_u32(7).unwrap();
    let them = Entity::from_raw_u32(9).unwrap();

    let sel = Selection {
        target: Some(me),
        guid: Some(0xdead_beef),
    };
    assert_eq!(emote_target(&sel, Some(me)), 0);

    let sel = Selection {
        target: Some(them),
        guid: Some(0xdead_beef),
    };
    assert_eq!(emote_target(&sel, Some(me)), 0xdead_beef);

    // No selection is untargeted, and with no self entity yet a selection goes out untouched.
    assert_eq!(emote_target(&Selection::default(), Some(me)), 0);
    let sel = Selection {
        target: Some(them),
        guid: Some(0xdead_beef),
    };
    assert_eq!(emote_target(&sel, None), 0xdead_beef);
}

#[test]
fn a_received_text_emote_composes_its_sentence_and_names_the_performer() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = benilla_formats::load_emote_text_catalog(&mut chain).expect("emote text catalog");
    const WAVE: u32 = 101;
    const SIT: u32 = 86;

    fn them(target: &'static str) -> benilla_formats::EmoteLine<'static> {
        benilla_formats::EmoteLine {
            performer: "Bob",
            performer_is_you: false,
            performer_female: false,
            target: if target == "-" { "" } else { target },
            your_name: "Me",
        }
    }
    fn mine(target: &'static str) -> benilla_formats::EmoteLine<'static> {
        benilla_formats::EmoteLine {
            performer: "Me",
            performer_is_you: true,
            ..them(target)
        }
    }
    let line = |text_id, l| super::feed::text_emote_event(&cat, text_id, &l);

    for (l, expected) in [
        (them("Jane"), "Bob waves at Jane."),
        (them("Me"), "Bob waves at you."),
        (them("-"), "Bob waves."),
    ] {
        let e = line(WAVE, l).expect("a sentence");
        assert_eq!(e.text, expected);
        assert_eq!(e.kind, Some(K::TextEmote));
        // arg2 is the performer, not the target (`0x49b47c`).
        assert_eq!(e.sender, "Bob");
    }
    for (l, expected) in [
        (mine("Jane"), "You wave at Jane."),
        (mine("-"), "You wave."),
    ] {
        let e = line(WAVE, l).expect("a sentence");
        assert_eq!(e.text, expected);
        assert_eq!(e.sender, "Me");
    }
    // SIT's EmotesTextData rows ship blank: no line, not an empty one.
    assert!(line(SIT, them("-")).is_none(), "/sit prints nothing");
}

/// `COMBATLOG_HONORAWARD`, `COMBATLOG_HONORGAIN` and `COMBATLOG_DISHONORGAIN`
/// (`GlobalStrings.lua:785-787`).
#[test]
fn honor_gain_lines_pick_the_reference_form() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        with_strings(|g| super::feed::honor_gain_line(None, None, 42, g)).as_deref(),
        Some("You have been awarded 42 honor points.")
    );
    assert_eq!(
        with_strings(|g| super::feed::honor_gain_line(Some("Grimtusk"), Some("Sergeant"), 137, g))
            .as_deref(),
        Some("Grimtusk dies, honorable kill Rank: Sergeant (Estimated Honor Points: 137)")
    );
    // A dishonorable kill: vmangos sends a negative honor (`HonorMgr.cpp:807`).
    assert_eq!(
        with_strings(|g| super::feed::honor_gain_line(Some("Innkeeper Renee"), None, -37, g))
            .as_deref(),
        Some("Innkeeper Renee dies, dishonorable kill.")
    );
    // The fork is `honor <= 0` (`0x625270`): zero honor is dishonorable.
    assert_eq!(
        with_strings(|g| super::feed::honor_gain_line(Some("Grimtusk"), Some("Sergeant"), 0, g))
            .as_deref(),
        Some("Grimtusk dies, dishonorable kill.")
    );
    // No rank title: the clause stays, empty.
    assert_eq!(
        with_strings(|g| super::feed::honor_gain_line(Some("Grimtusk"), None, 5, g)).as_deref(),
        Some("Grimtusk dies, honorable kill Rank:  (Estimated Honor Points: 5)")
    );
}

#[test]
fn xp_gain_lines_pick_the_reference_form() {
    let _data = benilla_formats::wow_data_or_skip!();
    // `COMBATLOG_XPGAIN_FIRSTPERSON`, `COMBATLOG_XPGAIN_EXHAUSTION1` (rested) and
    // `COMBATLOG_XPGAIN_FIRSTPERSON_UNNAMED` (`GlobalStrings.lua:801`, `:789`, `:804`).
    assert_eq!(
        with_strings(|g| super::feed::xp_gain_line(Some("Kobold Vermin"), 35, 0, g)).as_deref(),
        Some("Kobold Vermin dies, you gain 35 experience.")
    );
    assert_eq!(
        with_strings(|g| super::feed::xp_gain_line(Some("Kobold Vermin"), 52, 17, g)).as_deref(),
        Some("Kobold Vermin dies, you gain 52 experience. (+17 exp Rested bonus)")
    );
    assert_eq!(
        with_strings(|g| super::feed::xp_gain_line(None, 120, 0, g)).as_deref(),
        Some("You gain 120 experience.")
    );
    // The shipped lavender, 0x6F6FFF, chat-cache row 46.
    assert_eq!(default_color(K::CombatXpGain), [111, 111, 255]);
}

#[test]
fn exploration_lines_pick_the_reference_form() {
    let _data = benilla_formats::wow_data_or_skip!();
    // `ERR_ZONE_EXPLORED` (`GlobalStrings.lua:1925`) is the toast on every exploration packet;
    // `ERR_ZONE_EXPLORED_XP` (`:1926`) is the chat line, only when xp > 0 (`0x5e422f`).
    assert_eq!(
        with_strings(|g| super::feed::exploration_toast("Westfall", g)).as_deref(),
        Some("Discovered: Westfall")
    );
    assert_eq!(
        with_strings(|g| super::feed::exploration_line("Westfall", 85, g)).as_deref(),
        Some("Discovered Westfall: 85 experience gained")
    );
}

#[test]
fn monster_lines_use_the_bare_inline_name() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        compose(
            &ev(K::MonsterSay, "Intruders!", "Guard"),
            K::MonsterSay,
            "Common"
        )
        .unwrap(),
        "Guard says: Intruders!"
    );
    // MONSTER_EMOTE embeds %s where the name goes (CHAT_MONSTER_EMOTE_GET = "").
    assert_eq!(
        compose(
            &ev(K::MonsterEmote, "%s beckons you closer.", "Sentinel"),
            K::MonsterEmote,
            "Common"
        )
        .unwrap(),
        "Sentinel beckons you closer."
    );
}

#[test]
fn channel_line_prefixes_the_stripped_channel() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut e = ev(K::Channel, "wts boar livers", "Bob");
    e.channel = "General - Elwynn Forest".into();
    assert_eq!(
        compose(&e, K::Channel, "Common").unwrap(),
        "[General] |Hplayer:Bob|h[Bob]|h: wts boar livers"
    );
}

/// The notice arms print arg4 whole, zone tail and all: the strip at `ChatFrame.lua:1463` is the
/// speech arm's alone. The bridge builds each fixture, so each takes the arm its byte selects.
#[test]
fn channel_notices_compose_by_the_notice_law() {
    let _data = benilla_formats::wow_data_or_skip!();
    let notice = |byte: u8, channel: &str, a: Option<&str>, b: Option<&str>| {
        let e = super::feed::notice_event(
            byte,
            channel.to_string(),
            a.map(str::to_string),
            b.map(str::to_string),
        )
        .expect("the bridge builds an event for this notice");
        let kind = e.kind.expect("a built notice always carries its kind");
        compose(&e, kind, "Common")
    };
    assert_eq!(
        notice(0x02, "General - Elwynn Forest", None, None).unwrap(), // YOU_JOINED
        "Joined Channel: [General - Elwynn Forest]"
    );
    assert_eq!(
        notice(0x12, "World", Some("Ann"), Some("Mod")).unwrap(), // PLAYER_KICKED
        "[World] Player Ann kicked by Mod."
    );
    // A member join is a CHANNEL_JOIN event, linked like a player line.
    let mut join = ev(K::ChannelJoin, "", "Ann");
    join.channel = "World".into();
    assert_eq!(
        compose(&join, K::ChannelJoin, "Common").unwrap(),
        "[World] |Hplayer:Ann|h[Ann]|h joined channel."
    );
}

// ── the Lua face: the CHAT_MSG_* fire ───────────────────────────────────────────────────────────

/// A fresh VM with the chat stack the app loads, so `ChatFrame1` is the real window.
fn chat_vm() -> benilla_ui::script::UiScript {
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    // The chat tabs call the dropdown kit (`CloseDropDownMenus` on a click), which reads
    // `TOOLTIP_DEFAULT_COLOR`: both load ahead of ChatFrame.xml, as in `benilla.toc`.
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\UIMenu.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\ChatFrame.xml",
        "Interface\\FrameXML\\UIPanelTemplates.lua",
        "Interface\\FrameXML\\UIPanelTemplates.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\FloatingChatFrame.xml",
    ] {
        crate::ui_script::load_ui_for_test(&s, file);
    }
    crate::ui_script::fire_chat_login(&mut s);
    s.set_screen_size(1600.0, 900.0);
    s.resolve();
    s
}

/// An addon that records a `CHAT_MSG_*` fire: the count, the event and `arg1..arg10` joined with
/// `|`, which raises on a `nil` in any slot.
const SPY: &str = r#"
    SpyN, SpyEvent, SpyLine = 0, "", ""
    Spy = CreateFrame("Frame", "BenillaChatSpy")
    Spy:SetScript("OnEvent", function()
        SpyN = SpyN + 1
        SpyEvent = event
        SpyLine = arg1.."|"..arg2.."|"..arg3.."|"..arg4.."|"..arg5.."|"..arg6..
                  "|"..arg7.."|"..arg8.."|"..arg9.."|"..arg10
    end)
"#;

/// How many lines `ChatFrame1` is holding (`GetNumMessages`).
fn lines_in_window(s: &benilla_ui::script::UiScript) -> i64 {
    s.eval::<i64>("return ChatFrame1:GetNumMessages()").unwrap()
}

/// A routed line prints once, even after an addon registers `ChatFrame1` for its event again.
#[test]
fn an_addon_registering_our_own_chat_frame_does_not_double_print() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    s.run(SPY).unwrap();
    s.run(r#"BenillaChatSpy:RegisterEvent("CHAT_MSG_SAY")"#)
        .unwrap();

    assert_eq!(lines_in_window(&s), 0, "the window starts empty");
    super::frames::route(&mut s, &mut windows, &ev(K::Say, "hi there", "Bob"));
    assert_eq!(lines_in_window(&s), 1, "our window prints exactly once");
    assert_eq!(
        s.eval::<i64>("return SpyN").unwrap(),
        1,
        "the addon saw the fire — otherwise the count above proves nothing"
    );

    // An addon registers ChatFrame1 for the event again: the double-print case, if there is one.
    s.run(r#"ChatFrame1:RegisterEvent("CHAT_MSG_SAY")"#)
        .unwrap();
    super::frames::route(&mut s, &mut windows, &ev(K::Say, "hi again", "Bob"));
    assert_eq!(
        lines_in_window(&s),
        2,
        "one more line, not two — registering an event again never fires it twice"
    );
    assert_eq!(s.eval::<i64>("return SpyN").unwrap(), 2);
    assert!(s.errors().is_empty(), "handler errors: {:?}", s.errors());
}

/// Listeners run in registration order, as the reference's do, and `ChatFrame1` registered first.
#[test]
fn an_addons_handler_sees_the_line_already_in_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    s.run(
        r#"
        SeenAtFireTime = -1
        Spy = CreateFrame("Frame", "BenillaChatSpy")
        Spy:SetScript("OnEvent", function()
            SeenAtFireTime = ChatFrame1:GetNumMessages()
        end)
        BenillaChatSpy:RegisterEvent("CHAT_MSG_SAY")
    "#,
    )
    .unwrap();

    super::frames::route(&mut s, &mut windows, &ev(K::Say, "hi there", "Bob"));
    assert_eq!(
        s.eval::<i64>("return SeenAtFireTime").unwrap(),
        1,
        "the handler ran AFTER our window took the line, as registration order requires"
    );
}

/// arg7 and arg10 are numbers, zero for a non-channel line.
#[test]
fn a_say_line_fires_chat_msg_say_in_the_references_arg_positions() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    s.run(SPY).unwrap();
    s.run(r#"BenillaChatSpy:RegisterEvent("CHAT_MSG_SAY")"#)
        .unwrap();

    let mut e = ev(K::Say, "throm-ka", "Grunk");
    e.language = "Orcish".into();
    e.flag = "GM".into();
    super::frames::route(&mut s, &mut windows, &e);

    assert_eq!(s.eval::<String>("return SpyEvent").unwrap(), "CHAT_MSG_SAY");
    // arg1 is the raw body: the reference's Lua adds "%s says: " and the link.
    assert_eq!(
        s.eval::<String>("return SpyLine").unwrap(),
        "throm-ka|Grunk|Orcish|||GM|0|0||0"
    );
}

/// A channel notice fires its token in arg1 and the numbered name in arg4; the handler repeats
/// `ChatFrame_OnEvent`'s bare `arg7 > 0` and `arg10 > 0`, which raise on a `nil`.
#[test]
fn a_channel_notice_fires_its_token_and_the_reference_reads_arg7_and_arg10_bare() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    s.run(SPY).unwrap();
    s.run(
        r#"
        SpyZone, SpySuffix = nil, nil
        BenillaChatSpy:SetScript("OnEvent", function()
            SpyN = SpyN + 1
            SpyEvent = event
            SpyLine = arg1.."|"..arg2.."|"..arg3.."|"..arg4.."|"..arg5.."|"..arg6..
                      "|"..arg7.."|"..arg8.."|"..arg9.."|"..arg10
            -- ChatFrame_OnEvent l.1379 and l.1421, verbatim shape.
            if arg7 > 0 then SpyZone = arg7 end
            if arg10 > 0 then SpySuffix = arg10 end
        end)
        BenillaChatSpy:RegisterEvent("CHAT_MSG_CHANNEL_NOTICE")
    "#,
    )
    .unwrap();
    // The window prints only a channel it carries (`ChatFrame.lua:1374-1391`); `/join` adds it.
    s.run("ChatFrame_AddChannel(ChatFrame1, 'General - Elwynn Forest')")
        .unwrap();

    let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
    e.notice = "2".into(); // YOU_JOINED
    e.channel = "1. General - Elwynn Forest".into();
    e.channel_base = "General - Elwynn Forest".into();
    e.channel_number = 1;
    e.zone_channel_id = 1; // ChatChannels.dbc General
    super::frames::route(&mut s, &mut windows, &e);

    assert_eq!(s.eval::<i64>("return SpyN").unwrap(), 1);
    assert_eq!(
        s.eval::<String>("return SpyEvent").unwrap(),
        "CHAT_MSG_CHANNEL_NOTICE"
    );
    assert_eq!(
        s.eval::<String>("return SpyLine").unwrap(),
        "YOU_JOINED|||1. General - Elwynn Forest|||1|1|General - Elwynn Forest|0"
    );
    assert_eq!(s.eval::<i64>("return SpyZone").unwrap(), 1);
    assert!(s.errors().is_empty(), "handler errors: {:?}", s.errors());
    assert_eq!(lines_in_window(&s), 1);
}

/// The notice switch's MODE_CHANGE arm (`0x49c24d`) calls `0x49e910` and returns before the fire.
#[test]
fn a_mode_change_notice_never_becomes_an_event() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_protocol::messages::{channel_notice, ChannelNoticeTail};

    let mut log = super::feed::ChatLog::default();
    log.push_channel_notice(
        channel_notice::MODE_CHANGE,
        "World".into(),
        &ChannelNoticeTail::ModeChange {
            guid: 42,
            old_flags: 0,
            new_flags: 1,
        },
    );
    assert_eq!(
        log.pending_len(),
        0,
        "MODE_CHANGE is dropped at the feed — the reference's 0x0C arm fires nothing"
    );

    // The control: a notice that fires is queued.
    log.push_channel_notice(
        channel_notice::YOU_JOINED,
        "World".into(),
        &ChannelNoticeTail::YouJoined { flags: 0 },
    );
    assert_eq!(log.pending_len(), 1);
}

/// arg4, arg7, arg8 and arg9 are one channel record in the reference, defaulted together
/// (`0x49b12f`).
#[test]
fn a_channel_we_are_not_in_fires_the_bare_name_and_zeroes() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    s.run(SPY).unwrap();
    s.run(r#"BenillaChatSpy:RegisterEvent("CHAT_MSG_CHANNEL")"#)
        .unwrap();

    let channels = super::edit::ChannelState::default();
    let mut e = ev(K::Channel, "wts boar livers", "Bob");
    e.channel = "SomeoneElsesChannel".into();
    channels.stamp_channel(&mut e);
    super::frames::route(&mut s, &mut windows, &e);

    assert_eq!(
        s.eval::<String>("return SpyLine").unwrap(),
        "wts boar livers|Bob||SomeoneElsesChannel|||0|0||0",
        "arg4 keeps the bare INCOMING name (the miss leg still has one); arg7/8/9/10 are the \
         record we do not have, so 0/0/\"\"/0"
    );
}

#[test]
fn stamping_a_channel_splits_the_display_form_from_the_base_name() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut channels = super::edit::ChannelState::default();
    channels.claim_slot("World");
    channels.claim_slot("General - Elwynn Forest");

    let mut e = ev(K::Channel, "wts boar livers", "Bob");
    e.channel = "General - Elwynn Forest".into();
    channels.stamp_channel(&mut e);
    assert_eq!(e.channel, "2. General - Elwynn Forest"); // arg4
    assert_eq!(e.channel_number, 2); // arg8
    assert_eq!(e.channel_base, "General - Elwynn Forest"); // arg9, zone tail intact

    // A channel we are not in: its bare name in arg4, the rest empty (the miss leg `0x49aa86`).
    let mut other = ev(K::Channel, "hi", "Bob");
    other.channel = "SomeoneElsesChannel".into();
    channels.stamp_channel(&mut other);
    assert_eq!(other.channel, "SomeoneElsesChannel"); // arg4: the bare incoming name
    assert_eq!(other.channel_number, 0); // arg8
    assert_eq!(other.channel_base, ""); // arg9, not the name
    assert_eq!(other.zone_channel_id, 0); // arg7
}

/// The channel family's colour is `ChatTypeInfo["CHANNEL"..arg8]` (`ChatFrame.lua:1381`).
#[test]
fn a_channel_notice_renders_in_the_channels_color_not_the_notice_row() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::QuadContent;

    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let mut channels = super::edit::ChannelState::default();
    channels.claim_slot("General - Elwynn Forest");

    // The window prints only a channel it carries (`ChatFrame.lua:1374-1391`).
    s.run("ChatFrame_AddChannel(ChatFrame1, 'General - Elwynn Forest')")
        .unwrap();
    let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
    e.channel = "General - Elwynn Forest".into();
    e.notice = "2".into(); // YOU_JOINED
    super::feed::deliver(&mut s, &mut windows, &mut channels, &mut e);
    s.resolve();

    let line = "Joined Channel: [1. General - Elwynn Forest]";
    let color = s
        .extract()
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text {
                text: Some(t),
                color: Some(c),
                ..
            } if t == line => Some(*c),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the notice line {line:?} rendered"));
    let near = |a: f32, b: f32| (a - b).abs() < 0.01;
    assert!(
        near(color[0], 1.0) && near(color[1], 192.0 / 255.0) && near(color[2], 192.0 / 255.0),
        "FFC0C0 (the CHANNEL1 row), not C0C0C0 (the CHANNEL_NOTICE row): {color:?}"
    );
}

/// The stock `YOU_LEFT` arm deletes the window's registration for the channel it matches
/// (`ChatFrame.lua:1382-1384`) and no join re-adds it, so the leave of a renamed slot must match
/// nothing. The window registers by `ChannelID`, as the chat cache does (`ChatFrame.lua:1379`).
#[test]
fn a_zone_change_must_not_deregister_the_channel_it_renames() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let mut channels = super::edit::ChannelState {
        channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
            benilla_formats::ChatChannelRow {
                id: 1,
                flags: 0x0_0003,
                pattern: "General - %s".into(),
                shortcut: "General".into(),
            },
        ]),
        ..Default::default()
    };
    channels.claim_slot("General - Elwynn Forest");

    // `ChatFrame_RegisterForChannels(GetChatWindowChannels(1))`'s two writes, by hand.
    s.run("ChatFrame1.channelList[1] = 'General' ChatFrame1.zoneChannelList[1] = 1")
        .unwrap();

    let channel_line = |name: &str| {
        let mut e = ChatEvent::text_only(K::Channel, "anybody out here".into());
        e.sender = "Bob".into();
        e.channel = name.into();
        e
    };
    let notice = |name: &str, byte: &str| {
        let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
        e.channel = name.into();
        e.notice = byte.into();
        e
    };

    // The control: registered by id, a General line reaches the window.
    let before = lines_in_window(&s);
    super::feed::deliver(
        &mut s,
        &mut windows,
        &mut channels,
        &mut channel_line("General - Elwynn Forest"),
    );
    assert_eq!(
        lines_in_window(&s),
        before + 1,
        "the control must print — otherwise the assertion below proves nothing"
    );

    // The border crossing: the leave goes out, the slot is renamed in place (`0x49bc50`), the join
    // goes out, and only then do the two notices land.
    let renamed = channels.rename_slot("General - Elwynn Forest", "General - Westfall");
    assert_eq!(
        renamed,
        Some(1),
        "renamed in place — the slot number does not move"
    );

    let before = lines_in_window(&s);
    super::feed::deliver(
        &mut s,
        &mut windows,
        &mut channels,
        &mut notice("General - Elwynn Forest", "3"), // YOU_LEFT
    );
    assert_eq!(
        lines_in_window(&s),
        before,
        "the leave prints NOTHING: no slot carries the old name any more, so arg7/arg8/arg9 come \
         out defaulted and the stock handler returns at `found == 0` — which is also why it never \
         reaches the arm that would deregister the channel"
    );

    super::feed::deliver(
        &mut s,
        &mut windows,
        &mut channels,
        &mut notice("General - Westfall", "2"), // YOU_JOINED
    );

    assert_eq!(
        s.eval::<Option<i64>>("return ChatFrame1.zoneChannelList[1]")
            .unwrap(),
        Some(1),
        "the window must still be registered for ChannelID 1 after the rename — a nil here is \
         General going silent for the rest of the session"
    );

    // Speech from the new zone still lands.
    let before = lines_in_window(&s);
    super::feed::deliver(
        &mut s,
        &mut windows,
        &mut channels,
        &mut channel_line("General - Westfall"),
    );
    assert_eq!(
        lines_in_window(&s),
        before + 1,
        "a General line in the new zone must reach the window"
    );
}

/// Leaving a city (`0x49a3b8`) sends the leave but keeps Trade's slot in state 3 (`0x49bcf0`): the
/// notice comes back as `SUSPENDED`, the stock `YOU_LEFT` arm never runs, and a re-join takes the
/// same slot.
#[test]
fn leaving_a_capital_suspends_trade_rather_than_deregistering_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let mut channels = super::edit::ChannelState {
        channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
            benilla_formats::ChatChannelRow {
                id: 2,
                flags: 0x0_003B,
                pattern: "Trade - %s".into(),
                shortcut: "Trade".into(),
            },
        ]),
        ..Default::default()
    };
    channels.claim_slot("Trade - City");
    s.run("ChatFrame1.channelList[1] = 'Trade' ChatFrame1.zoneChannelList[1] = 2")
        .unwrap();

    let notice = |name: &str, byte: &str| {
        let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
        e.channel = name.into();
        e.notice = byte.into();
        e
    };

    // The walk: LEAVE goes out, then the eligibility test suspends the slot.
    assert_eq!(channels.suspend_slot("Trade - City"), Some(1));
    let before = lines_in_window(&s);
    super::feed::deliver(
        &mut s,
        &mut windows,
        &mut channels,
        &mut notice("Trade - City", "3"), // YOU_LEFT
    );
    s.resolve();

    // `CHAT_SUSPENDED_NOTICE` reads as `CHAT_YOU_LEFT_NOTICE` does; only arg1, which the stock
    // handler branches on, differs.
    assert_eq!(lines_in_window(&s), before + 1);
    assert!(
        s.extract().iter().any(|q| matches!(
            &q.content,
            benilla_ui::script::QuadContent::Text { text: Some(t), .. }
                if t == "Left Channel: [1. Trade - City]"
        )),
        "the suspended leave renders the same text as an ordinary one"
    );

    assert_eq!(
        channels.number_of("Trade - City"),
        Some(1),
        "the record and its number survive — `/1` still addresses Trade, and the state-3 bypass \
         needs the slot to be there to bypass onto"
    );
    assert_eq!(
        s.eval::<Option<i64>>("return ChatFrame1.zoneChannelList[1]")
            .unwrap(),
        Some(2),
        "and the window is still registered for it: the notice carried the SUSPENDED token, so \
         the stock handler never reached the arm that deletes the registration"
    );

    // Walking back in: the same slot and number, and the notice prints.
    let before = lines_in_window(&s);
    super::feed::deliver(
        &mut s,
        &mut windows,
        &mut channels,
        &mut notice("Trade - City", "2"), // YOU_JOINED
    );
    assert_eq!(
        lines_in_window(&s),
        before + 1,
        "the re-join prints — it could not have, with the registration gone"
    );
    assert_eq!(channels.number_of("Trade - City"), Some(1));
    assert_eq!(
        channels.slot_state("Trade - City"),
        Some(super::edit::SlotState::Joined),
        "and the slot is back to plain joined"
    );
}

/// A renamed slot's join notice reads as `YOU_CHANGED`: the `0x02` arm splits on `rec+0x9c == 2`.
#[test]
fn a_renamed_zone_channel_confirms_as_changed_not_joined() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let mut channels = super::edit::ChannelState {
        channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
            benilla_formats::ChatChannelRow {
                id: 1,
                flags: 0x0_0003,
                pattern: "General - %s".into(),
                shortcut: "General".into(),
            },
        ]),
        ..Default::default()
    };
    channels.claim_slot("General - Elwynn Forest");
    s.run("ChatFrame1.channelList[1] = 'General' ChatFrame1.zoneChannelList[1] = 1")
        .unwrap();
    channels.rename_slot("General - Elwynn Forest", "General - Westfall");

    let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
    e.channel = "General - Westfall".into();
    e.notice = "2".into(); // YOU_JOINED
    super::feed::deliver(&mut s, &mut windows, &mut channels, &mut e);
    s.resolve();

    let want = "Changed Channel: [1. General - Westfall]";
    assert!(
        s.extract().iter().any(|q| matches!(
            &q.content,
            benilla_ui::script::QuadContent::Text { text: Some(t), .. } if t == want
        )),
        "expected {want:?} in the window"
    );
    assert_eq!(
        channels.slot_state("General - Westfall"),
        Some(super::edit::SlotState::Joined),
        "and the confirming notice resolves the state — a second crossing must read as a rename \
         of its own, not as a leftover"
    );
}

/// The `YOU_LEFT` arm tears the record down after the fire (`0x49c5b0`, then `0x49bbd0` at
/// `0x49c5c2`), so [`super::feed::deliver`] stamps the line before it frees the slot.
#[test]
fn a_leave_notice_keeps_its_number_because_the_record_dies_after_the_line() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let mut channels = super::edit::ChannelState::default();
    channels.claim_slot("World");
    channels.claim_slot("General - Elwynn Forest");

    let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
    e.channel = "General - Elwynn Forest".into();
    e.notice = "3".into(); // YOU_LEFT
    super::feed::deliver(&mut s, &mut windows, &mut channels, &mut e);

    assert_eq!(e.channel, "2. General - Elwynn Forest", "arg4 was stamped");
    assert_eq!(
        e.channel_number, 2,
        "arg8 — what the color resolves through"
    );
    assert_eq!(
        compose(&e, K::ChannelNotice, "Common").unwrap(),
        "Left Channel: [2. General - Elwynn Forest]"
    );
    assert_eq!(
        channels.names(),
        [Some("World".to_string()), None],
        "and only THEN is the record gone — as a HOLE at slot 2, not a shortened list"
    );
}

/// The reference frees a slot in place and refills the first free one, so no other number moves.
#[test]
fn a_freed_slot_is_reused_and_the_others_keep_their_numbers() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut c = super::edit::ChannelState::default();
    assert_eq!(c.claim_slot("General - Teldrassil"), Some(1));
    assert_eq!(c.claim_slot("Trade - City"), Some(2));
    assert_eq!(c.claim_slot("LocalDefense - Teldrassil"), Some(3));

    // Cross a zone border: General and LocalDefense rename, Trade is untouched.
    assert_eq!(c.free_slot("General - Teldrassil"), Some(1));
    assert_eq!(
        c.claim_slot("General - The Barrens"),
        Some(1),
        "the freed slot is reused — the client scans for a zeroed record before growing"
    );
    assert_eq!(c.free_slot("LocalDefense - Teldrassil"), Some(3));
    assert_eq!(c.claim_slot("LocalDefense - The Barrens"), Some(3));
    assert_eq!(
        c.number_of("Trade - City"),
        Some(2),
        "Trade never moved: /2 still reaches it, which is the whole complaint"
    );

    // Leaving the city drops Trade; the hole it leaves is what the next join takes.
    assert_eq!(c.free_slot("Trade - City"), Some(2));
    assert!(c.joined[1].is_none(), "a hole answers 'not joined'");
    assert_eq!(c.number_of("General - The Barrens"), Some(1), "still 1");
    assert_eq!(
        c.claim_slot("Trade - City"),
        Some(2),
        "and back into slot 2"
    );

    // The ceiling is ten slots (`0x49b9c0`).
    for i in 4..=super::edit::MAX_CHANNELS {
        assert_eq!(c.claim_slot(&format!("Custom{i}")), Some(i as u32));
    }
    assert_eq!(c.claim_slot("OneTooMany"), None);
    assert_eq!(c.free_slot("Custom7"), Some(7));
    assert_eq!(
        c.claim_slot("OneTooMany"),
        Some(7),
        "full means no free slot, not a permanent ceiling"
    );
}

/// The reference destroys its Lua state at logout, so the next character's window is empty.
#[test]
fn a_session_end_empties_the_window_and_the_boxs_memory() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let mut log = super::ChatLog::default();

    super::frames::route(&mut s, &mut windows, &ev(K::Say, "hi there", "Bob"));
    super::frames::route(&mut s, &mut windows, &ev(K::Whisper, "psst", "Ann"));
    log.push_event(ChatEvent::text_only(K::System, "queued".into()));
    assert_eq!(
        lines_in_window(&s),
        2,
        "the window has this session's lines"
    );

    super::end_chat_session(Some(&mut s), &mut log);

    assert_eq!(
        lines_in_window(&s),
        0,
        "and the next character starts clean"
    );
}

/// Each token names a `CHAT_<X>_NOTICE` string (`ChatFrame.lua:1416`, `:1424`); the tokenless bytes
/// are MODE_CHANGE (`0x0C`), which fires nothing, and the member lines JOINED and LEFT.
#[test]
fn every_notice_token_resolves_and_the_tokenless_bytes_stay_silent() {
    let _data = benilla_formats::wow_data_or_skip!();
    for byte in 0x00u8..=0x21 {
        let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
        e.channel = "World".into();
        e.notice = byte.to_string();
        let rendered = GLOBAL_STRINGS.with(|s| {
            super::frames::compose_notice(&e, K::ChannelNotice, &|key| {
                s.lua().globals().get::<String>(key).ok()
            })
        });
        match super::event::notice_token(byte, None) {
            Some(token) => assert!(
                rendered.is_some(),
                "notice {byte:#04x} has token {token} but CHAT_{token}_NOTICE resolves to nothing"
            ),
            None => assert_eq!(
                rendered, None,
                "notice {byte:#04x} has no token and must render nothing"
            ),
        }
    }
}

/// `CHAT_INVITE_NOTICE` is `"%2$s has invited you to join the channel '%1$s'."`, filled from the
/// fixed `(arg4, arg2)` list (`ChatFrame.lua:1418`).
#[test]
fn the_invite_notice_reorders_its_two_names() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut e = ChatEvent::text_only(K::ChannelNoticeUser, String::new());
    e.channel = "2. Trade - City".into();
    e.sender = "Ann".into();
    e.notice = benilla_protocol::messages::channel_notice::INVITE.to_string();
    let line = GLOBAL_STRINGS.with(|s| {
        super::frames::compose_notice(&e, K::ChannelNoticeUser, &|key| {
            s.lua().globals().get::<String>(key).ok()
        })
    });
    assert_eq!(
        line.as_deref(),
        Some("Ann has invited you to join the channel '2. Trade - City'."),
        "the inviter fills %2$s and the channel — arg4, zone tail and all — fills %1$s"
    );
}

/// A new kind breaks [`super::event::event_name`]'s exhaustive match; this makes it join `ALL`.
#[test]
fn every_kind_is_in_all() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut seen: Vec<&str> = K::ALL
        .iter()
        .map(|&k| super::event::event_name(k))
        .collect();
    let before = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), before, "a kind is listed twice in ALL");
    assert_eq!(before, 93, "93 kinds — update this when the kind set grows");
}

#[test]
fn colors_match_the_shipped_table() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(default_color(K::Say), [255, 255, 255]);
    assert_eq!(default_color(K::System), [255, 255, 0]);
    assert_eq!(default_color(K::Yell), [255, 64, 64]);
    assert_eq!(default_color(K::Emote), [255, 128, 64]);
    assert_eq!(default_color(K::MonsterSay), [255, 255, 159]);
    assert_eq!(default_color(K::Loot), [0, 170, 0]);
    assert_eq!(default_color(K::Money), [255, 255, 0]);
    assert_eq!(default_color(K::ChannelNotice), [192, 192, 192]);
    assert_eq!(default_color(K::RaidWarning), [255, 219, 183]);
    assert_eq!(default_color(K::BgSystemAlliance), [0, 174, 239]);
}

// ── the submitted-line grammar: type switches and action commands ──────────────────────────

/// A command table from a stub of the shipped `SLASH_<INDEX><n>` and `EMOTE<i>_CMD<j>` strings.
fn stub_table() -> super::commands::SlashCommands {
    const STRINGS: &[(&str, &str)] = &[
        ("SLASH_JOIN1", "/join"),
        ("SLASH_LEAVE1", "/leave"),
        ("SLASH_LIST_CHANNEL1", "/chatlist"),
        ("SLASH_CHAT_AFK1", "/afk"),
        ("SLASH_CHAT_DND1", "/dnd"),
        ("SLASH_RANDOM1", "/random"),
        ("SLASH_RANDOM2", "/roll"),
        ("SLASH_PLAYED1", "/played"),
        ("SLASH_HELP1", "/help"),
        ("SLASH_PVP1", "/pvp"),
        ("SLASH_REPLY1", "/r"),
        ("SLASH_LOGOUT1", "/logout"),
        ("SLASH_LOGOUT2", "/camp"),
        ("SLASH_QUIT1", "/quit"),
        ("SLASH_TRADE1", "/trade"),
        ("SLASH_SCRIPT1", "/script"),
        // One emote index, in the two-table shape: the alias, and the token it resolves through.
        ("EMOTE1_CMD1", "/wave"),
        ("EMOTE1_CMD2", "/hello"), // an alias that is not the token
        ("EMOTE1_TOKEN", "WAVE"),
    ];
    super::commands::SlashCommands::build(
        |key| {
            STRINGS
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        },
        |token| (token == "WAVE").then_some(101),
    )
}

#[test]
fn action_commands_parse() {
    let _data = benilla_formats::wow_data_or_skip!();
    let t = stub_table();
    let parse_line = |line: &str| super::input::parse_line(&t, line);
    assert_eq!(
        parse_line("/join world secret"),
        ParsedChat::Join {
            name: "world".into(),
            password: "secret".into(),
        }
    );
    assert_eq!(
        parse_line("/leave world"),
        ParsedChat::Leave {
            name: "world".into()
        }
    );
    assert_eq!(
        parse_line("/chatlist world"),
        ParsedChat::ChatList {
            name: "world".into()
        }
    );
    // `/afk` and `/dnd` run the stock `SlashCmdList` bodies, with the argument whole.
    assert_eq!(
        parse_line("/afk farming"),
        ParsedChat::Lua {
            body: "SlashCmdList[\"CHAT_AFK\"](\"farming\")".into()
        }
    );
    // Bare is the toggle: the empty string survives to the call.
    assert_eq!(
        parse_line("/dnd"),
        ParsedChat::Lua {
            body: "SlashCmdList[\"CHAT_DND\"](\"\")".into()
        }
    );
    assert_eq!(parse_line("/roll"), ParsedChat::Random { min: 1, max: 100 });
    assert_eq!(
        parse_line("/random 50"),
        ParsedChat::Random { min: 1, max: 50 }
    );
    assert_eq!(
        parse_line("/random 2 8"),
        ParsedChat::Random { min: 2, max: 8 }
    );
    assert_eq!(parse_line("/played"), ParsedChat::Played);
    assert_eq!(parse_line("/help"), ParsedChat::Help);
    // /pvp takes no argument; a trailing word is ignored.
    assert_eq!(parse_line("/pvp"), ParsedChat::Pvp);
    assert_eq!(parse_line("/pvp on"), ParsedChat::Pvp);
    // /r rides its own arm: the reply state lives on ChatEditState.
    assert_eq!(
        parse_line("/r hey"),
        ParsedChat::Reply { text: "hey".into() }
    );
}

#[test]
fn emote_aliases_resolve_through_the_table() {
    let _data = benilla_formats::wow_data_or_skip!();
    let t = stub_table();
    let parse_line = |line: &str| super::input::parse_line(&t, line);
    assert_eq!(parse_line("/wave"), ParsedChat::TextEmote(101));
    // An alias that is not its token's `EmotesText` name resolves too, as `/lol` (LAUGH) does.
    assert_eq!(parse_line("/hello"), ParsedChat::TextEmote(101));
    // An emote takes an argument (`DoEmote(token, msg)`): the command is the first word only.
    assert_eq!(parse_line("/wave Bob"), ParsedChat::TextEmote(101));
    assert_eq!(parse_line("/nosuch"), ParsedChat::Unknown);
}

#[test]
fn logout_and_camp_parse() {
    let _data = benilla_formats::wow_data_or_skip!();
    let t = stub_table();
    let parse_line = |line: &str| super::input::parse_line(&t, line);
    for line in ["/logout", "/camp", "/LOGOUT", "/logout now"] {
        assert_eq!(parse_line(line), ParsedChat::Logout);
    }
    assert_eq!(parse_line("/quit"), ParsedChat::Quit);
}

#[test]
fn one_line_reference_bodies_run_in_the_vm() {
    let _data = benilla_formats::wow_data_or_skip!();
    let t = stub_table();
    let parse_line = |line: &str| super::input::parse_line(&t, line);
    // `/trade` is the stock `InitiateTrade("target")`.
    assert_eq!(
        parse_line("/trade"),
        ParsedChat::Lua {
            body: "InitiateTrade(\"target\")".into()
        }
    );
    // `/script` runs the typed text as the chunk (stock `RunScript(msg)`); bare is a no-op.
    assert_eq!(
        parse_line("/script Print(\"hi\")"),
        ParsedChat::Lua {
            body: "Print(\"hi\")".into()
        }
    );
    assert_eq!(parse_line("/script"), ParsedChat::Unknown);
}

/// `/castvis` is a dev instrument: a player build never claims it (`run_mode::dev_affordances()`).
#[test]
fn castvis_parses_id_and_phase() {
    let _data = benilla_formats::wow_data_or_skip!();
    use crate::creature_anim::CastEventKind;
    let t = stub_table();
    let parse_line = |line: &str| super::input::parse_line(&t, line);
    if !crate::run_mode::dev_affordances() {
        assert_eq!(
            parse_line("/castvis 133"),
            ParsedChat::Unknown,
            "a player build must not claim an instrument's alias"
        );
        return;
    }
    assert_eq!(
        parse_line("/castvis 133"),
        ParsedChat::CastVis {
            spell_id: 133,
            kind: CastEventKind::Start,
            ground: false
        }
    );
    assert_eq!(
        parse_line("/castvis 133 go"),
        ParsedChat::CastVis {
            spell_id: 133,
            kind: CastEventKind::Go,
            ground: false
        }
    );
    // `ground` is a GO too, the pure-destination shape.
    assert_eq!(
        parse_line("/castvis 1543 GROUND"),
        ParsedChat::CastVis {
            spell_id: 1543,
            kind: CastEventKind::Go,
            ground: true
        }
    );
    assert_eq!(
        parse_line("/castvis 689 FAIL"),
        ParsedChat::CastVis {
            spell_id: 689,
            kind: CastEventKind::Fail,
            ground: false
        }
    );
    assert_eq!(parse_line("/castvis"), ParsedChat::Unknown);
    assert_eq!(parse_line("/castvis abc"), ParsedChat::Unknown);
    assert_eq!(parse_line("/castvis 133 nope"), ParsedChat::Unknown);
}

#[test]
fn unknown_slash_command_is_dropped_not_said_aloud() {
    let _data = benilla_formats::wow_data_or_skip!();
    let t = stub_table();
    let parse_line = |line: &str| super::input::parse_line(&t, line);
    assert_eq!(parse_line("/dancemove"), ParsedChat::Unknown);
    assert_eq!(parse_line("/frobnicate"), ParsedChat::Unknown);
}

/// The command table built as boot builds it: `GlobalStrings.lua` and `ChatFrame.lua`'s token
/// table run in a VM, joined to `EmotesText.dbc`.
#[test]
fn real_alias_table_resolves_the_shipped_commands() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let s = benilla_ui::script::UiScript::new().expect("VM");
    for file in ["GlobalStrings.lua", "ChatFrame.lua"] {
        let src = chain
            .read_file(&format!("Interface\\FrameXML\\{file}"))
            .expect("FrameXML file in the chain");
        let src = String::from_utf8_lossy(&src).into_owned();
        // GlobalStrings runs whole; ChatFrame contributes its token table alone.
        let src = if file == "ChatFrame.lua" {
            src.lines()
                .map(str::trim)
                .filter(|l| crate::ui_script::is_emote_token_line(l))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            src
        };
        s.run(&src).expect("runs clean");
    }
    let cat = benilla_formats::load_emote_sound_catalog(&mut chain).expect("emote catalog");
    let globals = s.lua().globals();
    let table = super::commands::SlashCommands::build(
        |name| globals.get::<String>(name).ok().filter(|v| !v.is_empty()),
        |token| cat.text_id(token),
    );
    let parse_line = |line: &str| super::input::parse_line(&table, line);

    // `/sit` is EmotesText 86, whose `Emotes.dbc` row 13 (STATE_SIT) sets stand state 1.
    assert_eq!(parse_line("/sit"), ParsedChat::TextEmote(86));
    assert_eq!(
        cat.text_emote(86).and_then(|e| cat.posture_state(e)),
        Some(1)
    );
    for (line, state) in [
        ("/stand", 0),
        ("/sit", 1),
        ("/sleep", 3),
        ("/liedown", 3),
        ("/kneel", 8),
    ] {
        let ParsedChat::TextEmote(text_id) = parse_line(line) else {
            panic!("{line} is an emote");
        };
        let posture = cat.text_emote(text_id).and_then(|e| cat.posture_state(e));
        assert_eq!(posture, Some(state), "{line} sets stand state {state}");
    }
    // Aliases that differ from their token's DBC name.
    for line in [
        "/lol",
        "/hi",
        "/ty",
        "/thanks",
        "/congrats",
        "/sorry",
        "/yes",
        "/bravo",
        "/weep",
        "/goodbye",
        "/pizza",
        "/strong",
    ] {
        assert!(
            matches!(parse_line(line), ParsedChat::TextEmote(_)),
            "{line} resolves to an emote"
        );
    }
    // DBC emote names the reference has no command for.
    for line in ["/joke", "/puzzle", "/attackmytarget"] {
        assert_eq!(parse_line(line), ParsedChat::Unknown, "{line}");
    }
    // `SlashCmdList["FOLLOW"]`, over the three distinct aliases of SLASH_FOLLOW1-6.
    for line in ["/follow", "/f", "/fol"] {
        assert_eq!(
            parse_line(line),
            ParsedChat::Follow { name: None },
            "{line}"
        );
    }
    assert_eq!(
        parse_line("/follow Probeone"),
        ParsedChat::Follow {
            name: Some("Probeone".into())
        }
    );
    // A command whose handler benilla does not register answers like any unknown command.
    assert_eq!(parse_line("/ginvite"), ParsedChat::Unknown);
    // /target and /assist take the whole argument as one name; `/tar` and `/a` are shipped aliases.
    assert_eq!(
        parse_line("/target Kobold Vermin"),
        ParsedChat::Target {
            name: Some("Kobold Vermin".into())
        },
        "the argument is trimmed WHOLE — `GetSlashCmdTarget`'s gsub, not a first-word split"
    );
    assert_eq!(
        parse_line("/tar   Hogger  "),
        ParsedChat::Target {
            name: Some("Hogger".into())
        }
    );
    assert_eq!(parse_line("/target"), ParsedChat::Target { name: None });
    assert_eq!(
        parse_line("/a Bob"),
        ParsedChat::Assist {
            name: Some("Bob".into())
        }
    );
    assert_eq!(parse_line("/assist"), ParsedChat::Assist { name: None });
    // `/cast` and `/spell` run the stock `CastSpellByName(msg)`; `/macro` and `/m` open the window.
    assert_eq!(
        parse_line("/cast Fireball(Rank 1)"),
        ParsedChat::Lua {
            body: "CastSpellByName(\"Fireball(Rank 1)\")".into()
        }
    );
    assert_eq!(
        parse_line("/spell Frostbolt"),
        ParsedChat::Lua {
            body: "CastSpellByName(\"Frostbolt\")".into()
        }
    );
    assert_eq!(
        parse_line("/cast"),
        ParsedChat::Unknown,
        "a bare /cast is the ref's own no-op (`if msg ~= \"\"`)"
    );
    for line in ["/macro", "/m"] {
        assert_eq!(
            parse_line(line),
            ParsedChat::Lua {
                body: "ShowMacroFrame()".into()
            },
            "{line}"
        );
    }
    assert_eq!(parse_line("/macrohelp"), ParsedChat::MacroHelp);
    assert_eq!(parse_line("/convertraid"), ParsedChat::ConvertRaid);
    // `/console` from a line that skipped the stock edit box forwards to the stock handler's verb.
    assert_eq!(
        parse_line("/console fpsJournal 1"),
        ParsedChat::Lua {
            body: "ConsoleExec(\"fpsJournal 1\")".into()
        }
    );
    assert_eq!(
        parse_line("/console reloadUI"),
        ParsedChat::Lua {
            body: "ConsoleExec(\"reloadUI\")".into()
        }
    );
    // The quoting is a short string: 1.12's Lua lexer has no long-string levels.
    assert_eq!(super::input::lua_quoted_string("a]]b"), "\"a]]b\"");
    assert_eq!(super::input::lua_quoted_string("a]]b]=]c"), "\"a]]b]=]c\"");
    assert_eq!(
        super::input::lua_quoted_string("say \"hi\"\\n"),
        "\"say \\\"hi\\\"\\\\n\""
    );
    // The literal round-trips through a real VM.
    {
        let vm = benilla_ui::script::UiScript::new().expect("VM");
        for payload in [
            "fpsJournal 1",
            "a]]b",
            "a]]b]=]c",
            "quote \" and backslash \\",
            "tab\there",
        ] {
            let lit = super::input::lua_quoted_string(payload);
            let got: String = vm
                .eval(&format!("return {lit}"))
                .unwrap_or_else(|e| panic!("{lit} must compile on a 1.12-grammar VM: {e}"));
            assert_eq!(got, payload, "and must carry the text unchanged");
        }
    }
    // The shipped surface: 68 distinct aliases over 36 `SlashCmdList` indices and 225 emote
    // commands over 169 `EmotesText` names (aliases repeat; EMOTE27 "UNUSED" has no row). Then
    // benilla's own `/reload`, `/errors`, `/err` and `/convertraid`, which are not 1.12 commands,
    // in every build, and 7 instrument aliases, in dev builds only.
    let instruments = if crate::run_mode::dev_affordances() {
        7
    } else {
        0
    };
    assert_eq!(
        table.counts(),
        (68, 225, 4, instruments),
        "(slash, emote, benilla addition, instrument) aliases"
    );
}

/// The `0x4000` "stand still" flag reports [`EmoteGate::Moving`] on any bit of the reference's
/// `0x20ff` mask (`move_flags::INTEGRATED`), which holds no SWIMMING.
#[test]
fn the_standing_still_arm_reports_moving_and_ignores_swimming() {
    use super::input::EmoteGate;
    use crate::creature_anim::move_flags;

    const STILL: u32 = 0x4000;

    assert_eq!(emote_send_eligible(STILL, 0, false, 0), EmoteGate::Send);
    // Any INTEGRATED bit trips it: a direction, a turn or a fall.
    for f in [
        move_flags::FORWARD,
        move_flags::BACKWARD,
        move_flags::STRAFE_LEFT,
        move_flags::TURN_RIGHT,
        move_flags::FALLING,
    ] {
        assert_eq!(
            emote_send_eligible(STILL, 0, false, f),
            EmoteGate::Moving,
            "move flag {f:#x}"
        );
    }
    // SWIMMING is not in `0x20ff`: a still swimmer is still.
    assert_eq!(
        emote_send_eligible(STILL, 0, false, move_flags::SWIMMING),
        EmoteGate::Send,
        "the mover-integration mask carries no SWIMMING"
    );
    // WALK_MODE and LEVITATING are mode bits, not motion.
    assert_eq!(
        emote_send_eligible(STILL, 0, false, move_flags::WALK_MODE),
        EmoteGate::Send
    );
    // Without the flag, movement is irrelevant.
    assert_eq!(
        emote_send_eligible(0, 0, false, move_flags::FORWARD),
        EmoteGate::Send
    );
    // An unconditionally suppressed emote (`0x0400`) never reaches this arm: it stays silent.
    assert_eq!(
        emote_send_eligible(0x4400, 0, false, move_flags::FORWARD),
        EmoteGate::Suppressed
    );
}

/// `IsSelfControlled` (`0x5fa550`) is false only while confused, fleeing or move-disabled, the
/// `0xc00004` mask, which leaves STUNNED out.
#[test]
fn self_controlled_is_true_for_an_ordinary_player() {
    use crate::player::self_controlled;

    assert!(self_controlled(0));
    assert!(self_controlled(1 << 3), "an ordinary PvP-attackable player");
    assert!(!self_controlled(0x0040_0000), "confused");
    assert!(!self_controlled(0x0080_0000), "fleeing");
    assert!(!self_controlled(0x0000_0004), "move-disabled");
    assert!(
        self_controlled(0x0004_0000),
        "STUNNED is not in the 0xc00004 mask — it has its own gate"
    );
}

// ── the send-side emote gate, `CheckEmoteEligible` (`0x47db40`), on real `EmoteFlags` values ──
const BOW: u32 = 0x4801;
const RUDE: u32 = 0x0001;
const APPLAUD: u32 = 0x0000;
const CHEER: u32 = 0x0800;
const SALUTE: u32 = 0x0800;
const LAUGH: u32 = 0x0980;

#[test]
fn seated_stand_required_emotes_are_suppressed() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(emote_send_eligible(BOW, 1, false, 0), EmoteGate::Suppressed); // 0x1 needs STAND
    assert_eq!(
        emote_send_eligible(RUDE, 1, false, 0),
        EmoteGate::Suppressed
    );
}

#[test]
fn seated_non_stand_emotes_pass() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(emote_send_eligible(APPLAUD, 1, false, 0), EmoteGate::Send);
    assert_eq!(emote_send_eligible(CHEER, 1, false, 0), EmoteGate::Send);
    assert_eq!(emote_send_eligible(LAUGH, 1, false, 0), EmoteGate::Send);
    assert_eq!(emote_send_eligible(SALUTE, 1, false, 0), EmoteGate::Send);
}

#[test]
fn swimming_suppresses_only_the_0x80_emotes() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        emote_send_eligible(LAUGH, 0, true, 0),
        EmoteGate::Suppressed
    ); // 0x0980 has 0x80
    assert_eq!(emote_send_eligible(CHEER, 0, true, 0), EmoteGate::Send);
}

#[test]
fn standing_and_dry_everyone_is_eligible() {
    let _data = benilla_formats::wow_data_or_skip!();
    for flags in [BOW, RUDE, APPLAUD, CHEER, SALUTE, LAUGH] {
        assert_eq!(
            emote_send_eligible(flags, 0, false, 0),
            EmoteGate::Send,
            "flags {flags:#x}"
        );
    }
}

#[test]
fn unconditional_and_sleep_dead_rules() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        emote_send_eligible(0x0400, 0, false, 0),
        EmoteGate::Suppressed
    ); // unconditional suppress
    assert_eq!(emote_send_eligible(0, 3, false, 0), EmoteGate::Suppressed); // SLEEP, no allow bit
    assert_eq!(emote_send_eligible(0, 7, false, 0), EmoteGate::Suppressed); // DEAD, no allow bit
    assert_eq!(emote_send_eligible(0x0200, 3, false, 0), EmoteGate::Send); // 0x0200 allows it
}

// ── received lines: the addon lane, the language header, the talk gesture ──────────────────

/// The text divides on its first tab (`0x49a8d0`), and with no tab it is all prefix; a lane
/// without a name reports `"UNKNOWN"` (`0x49aff4`).
#[test]
fn an_inbound_addon_line_splits_on_the_first_tab_only() {
    let _data = benilla_formats::wow_data_or_skip!();
    // The lane bytes come from the protocol crate, never copied by hand.
    use benilla_protocol::messages as m;
    let party = m::CHAT_TYPE_PARTY as u8;
    let raid = m::CHAT_TYPE_RAID as u8;
    let guild = m::CHAT_TYPE_GUILD as u8;
    let battleground = m::CHAT_TYPE_BATTLEGROUND as u8;
    let say = m::CHAT_TYPE_SAY as u8;
    #[allow(non_snake_case)]
    let (PARTY, RAID, GUILD, BATTLEGROUND, SAY) = (party, raid, guild, battleground, say);

    let mut log = super::feed::ChatLog::default();
    log.push_addon("oRA\tSYNC:1", PARTY, 7);
    // Tabs in the message: only the first divides.
    log.push_addon("CTRA\tA\tB\tC", RAID, 7);
    // No tab: all prefix, an empty message.
    log.push_addon("BareTag", GUILD, 7);
    // A trailing tab: an empty message.
    log.push_addon("Tag\t", BATTLEGROUND, 7);
    // A lane with no name still arrives, labelled.
    log.push_addon("X\ty", SAY, 7);

    assert_eq!(
        log.pending_addons(),
        vec![
            ("oRA".into(), "SYNC:1".into(), "PARTY".into()),
            ("CTRA".into(), "A\tB\tC".into(), "RAID".into()),
            ("BareTag".into(), String::new(), "GUILD".into()),
            ("Tag".into(), String::new(), "BATTLEGROUND".into()),
            ("X".into(), "y".into(), "UNKNOWN".into()),
        ]
    );
}

/// `CHAT_MSG_ADDON` carries prefix, message, distribution, sender, in that order (`0x49a95f`).
#[test]
fn the_addon_event_reaches_lua_with_four_arguments_in_order() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.run(
        r#"
        seen = nil
        f = CreateFrame("Frame", "AddonSink")
        f:RegisterEvent("CHAT_MSG_ADDON")
        f:SetScript("OnEvent", function()
            seen = { arg1, arg2, arg3, arg4 }
        end)
        "#,
    )
    .unwrap();

    super::feed::fire_addon_message(
        &mut s,
        "oRA".into(),
        "SYNC:1".into(),
        "PARTY".into(),
        "Someone".into(),
    );

    assert!(
        s.errors().is_empty(),
        "the fire must not raise: {:?}",
        s.errors()
    );
    assert_eq!(
        s.eval::<String>("return seen[1]").unwrap(),
        "oRA",
        "arg1 is the PREFIX"
    );
    assert_eq!(
        s.eval::<String>("return seen[2]").unwrap(),
        "SYNC:1",
        "arg2 is the MESSAGE"
    );
    assert_eq!(
        s.eval::<String>("return seen[3]").unwrap(),
        "PARTY",
        "arg3 is the DISTRIBUTION, not the sender"
    );
    assert_eq!(
        s.eval::<String>("return seen[4]").unwrap(),
        "Someone",
        "arg4 is the SENDER, and a name rather than a guid"
    );
}

/// The send joins on a tab (`0x49f9b3`) and the receive splits on the first (`0x49a8d0`), so a
/// payload's own tabs come back intact.
#[test]
fn an_addon_message_survives_its_own_send_and_receive() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.run(r#"SendAddonMessage("oRA", "SYNC\t1\t2", "PARTY")"#)
        .unwrap();
    assert!(s.errors().is_empty(), "send raised: {:?}", s.errors());

    let sends = s.take_addon_sends();
    assert_eq!(sends.len(), 1, "one broadcast queued");
    let sent = &sends[0];
    assert_eq!(sent.distribution.token(), "PARTY");

    // The same text arrives as a PARTY line in LANG_ADDON.
    let mut log = super::feed::ChatLog::default();
    log.push_addon(&sent.text, 0x01, 7);

    assert_eq!(
        log.pending_addons(),
        vec![(
            "oRA".to_string(),
            "SYNC\t1\t2".to_string(),
            "PARTY".to_string()
        )],
        "what one half composed, the other must recover — tabs in the payload included"
    );
}

/// The header shows unless arg3 is empty, "Universal" or `this.defaultLanguage`
/// (`ChatFrame.lua:1442`), the faction's language (`GetDefaultLanguage`, `0x5ec890`).
#[test]
fn the_language_header_suppresses_only_the_frames_own_default_tongue() {
    let _data = benilla_formats::wow_data_or_skip!();
    let orcish = ChatEvent {
        language: "Orcish".into(),
        ..ev(K::Say, "lok'tar", "Grom")
    };
    let common = ChatEvent {
        language: "Common".into(),
        ..ev(K::Say, "hello", "Ann")
    };

    // An Alliance body (default Common): Orcish is tagged, Common is not.
    assert_eq!(
        compose(&orcish, K::Say, "Common").unwrap(),
        "|Hplayer:Grom|h[Grom]|h says: [Orcish] lok'tar"
    );
    assert_eq!(
        compose(&common, K::Say, "Common").unwrap(),
        "|Hplayer:Ann|h[Ann]|h says: hello"
    );

    // A Horde body (default Orcish): the mirror.
    assert_eq!(
        compose(&orcish, K::Say, "Orcish").unwrap(),
        "|Hplayer:Grom|h[Grom]|h says: lok'tar"
    );
    assert_eq!(
        compose(&common, K::Say, "Orcish").unwrap(),
        "|Hplayer:Ann|h[Ann]|h says: [Common] hello"
    );

    // Language 0 arrives as an empty arg3 and is never tagged.
    let universal = ev(K::Say, "system", "Ann");
    assert_eq!(
        compose(&universal, K::Say, "Orcish").unwrap(),
        "|Hplayer:Ann|h[Ann]|h says: system"
    );

    // Comprehension does not matter: an understood foreign tongue is still tagged.
    let dwarvish = ChatEvent {
        language: "Dwarvish".into(),
        ..ev(K::Say, "here we go", "Bran")
    };
    assert_eq!(
        compose(&dwarvish, K::Say, "Common").unwrap(),
        "|Hplayer:Bran|h[Bran]|h says: [Dwarvish] here we go"
    );
}

/// The gesture selector in the parser `0x49d560` (`0x49d820`-`0x49d8ae`) matches the plaintext that
/// `0x49dbc2` then hands to the display path `0x49a870`, so a laugh is language-independent.
#[test]
fn the_talk_gesture_reads_the_plaintext_not_the_garbled_line() {
    let _data = benilla_formats::wow_data_or_skip!();
    use crate::creature_anim::{select_gesture, Gesture};
    use benilla_protocol::messages::CHAT_MSG_SAY;

    let Some(data) = benilla_formats::wow_data() else {
        return; // no client data
    };
    let mut chain = benilla_formats::Chain::open(&data).expect("open patch chain");
    let words = benilla_formats::load_language_words(&mut chain).expect("load word pools");

    // An Orcish `lol` heard by someone with no Orcish at all.
    let garbled = benilla_formats::garble_chat(&words, 1, 0, "lol");
    assert_ne!(garbled, "lol", "the two inputs must actually differ");

    let laugh_words = |n: u32| (n == 1).then(|| "lol".to_string());
    assert_eq!(
        select_gesture(CHAT_MSG_SAY, "lol", laugh_words),
        Some(Gesture::Laugh),
        "the plaintext laughs"
    );
    assert_eq!(
        select_gesture(CHAT_MSG_SAY, &garbled, laugh_words),
        Some(Gesture::Talk),
        "the garbled form would NOT laugh — which is why the feed must pass the plaintext"
    );
}

// ── the combat log reaches addons ────────────────────────────────────────────────────────────

/// Combat-log addons parse arg1 with patterns built from the GlobalStrings, so arg1 is the whole
/// sentence.
#[test]
fn an_addon_sees_the_combat_log_line_it_registers_for() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    s.run(SPY).unwrap();
    s.run(r#"BenillaChatSpy:RegisterEvent("CHAT_MSG_SPELL_SELF_DAMAGE")"#)
        .unwrap();

    let line = "Your Fireball hits Kobold Vermin for 120 fire damage.";
    super::frames::route(
        &mut s,
        &mut windows,
        &ChatEvent::text_only(K::SpellSelfDamage, line.into()),
    );

    assert_eq!(
        s.eval::<String>("return SpyEvent").unwrap(),
        "CHAT_MSG_SPELL_SELF_DAMAGE",
        "the event an addon registered is the event that fired"
    );
    assert_eq!(s.eval::<i64>("return SpyN").unwrap(), 1);
    let seen: String = s.eval("return SpyLine").unwrap();
    assert!(
        seen.starts_with(&format!("{line}|")),
        "arg1 must be the whole sentence, not a fragment — got {seen:?}"
    );
    assert!(s.errors().is_empty(), "handler errors: {:?}", s.errors());
}

#[test]
fn every_combat_log_kind_reaches_an_addon() {
    let _data = benilla_formats::wow_data_or_skip!();
    for kind in K::ALL.iter().copied().filter(|k| k.is_combat_log()) {
        let name = super::event::event_name(kind);
        let mut s = chat_vm();
        let mut windows = super::frames::ChatWindows::default();
        s.run(SPY).unwrap();
        s.run(&format!(r#"BenillaChatSpy:RegisterEvent("{name}")"#))
            .unwrap();

        let line = format!("a line for {name}");
        super::frames::route(
            &mut s,
            &mut windows,
            &ChatEvent::text_only(kind, line.clone()),
        );

        assert_eq!(
            s.eval::<String>("return SpyEvent").unwrap(),
            name,
            "{name} did not fire under its own name"
        );
        assert_eq!(
            s.eval::<i64>("return SpyN").unwrap(),
            1,
            "{name} fired more than once"
        );
        let seen: String = s.eval("return SpyLine").unwrap();
        assert!(
            seen.starts_with(&format!("{line}|")),
            "{name}: arg1 must be the whole sentence — got {seen:?}"
        );
        assert!(
            s.errors().is_empty(),
            "{name} handler errors: {:?}",
            s.errors()
        );
    }
}

/// The `COMBAT_` and `SPELL_` arms only `AddMessage(arg1)` (`ChatFrame.lua:1397`, `:1399`).
#[test]
fn a_combat_log_line_renders_verbatim() {
    let _data = benilla_formats::wow_data_or_skip!();
    let default_language = String::from("Common");
    for kind in K::ALL.iter().copied().filter(|k| k.is_combat_log()) {
        let mut e = ChatEvent::text_only(kind, "You hit Kobold Vermin for 5.".into());
        // Populated so a fall-through to the player branch would show.
        e.sender = "Somebody".into();
        e.flag = "AFK".into();
        e.language = "Orcish".into();
        assert_eq!(
            compose(&e, kind, &default_language).as_deref(),
            Some("You hit Kobold Vermin for 5."),
            "{} must render verbatim",
            super::event::event_name(kind)
        );
    }
}

#[test]
fn both_dock_tabs_exist_and_select_their_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = chat_vm();
    for id in [1, 2] {
        assert!(
            s.eval::<bool>(&format!("return ChatFrame{id}Tab ~= nil"))
                .unwrap(),
            "ChatFrame{id}Tab is missing — its window is unreachable from the screen"
        );
    }
    // The default dock: General selected and shown, Combat Log hidden behind its tab.
    assert_eq!(
        s.eval::<i64>("return SELECTED_DOCK_FRAME:GetID()").unwrap(),
        1
    );
    assert!(s.eval::<bool>("return ChatFrame1:IsShown()").unwrap());
    assert!(!s.eval::<bool>("return ChatFrame2:IsShown()").unwrap());

    // Clicking the Combat Log tab swaps them.
    s.run("FCF_SelectDockFrame(ChatFrame2)").unwrap();
    assert_eq!(
        s.eval::<i64>("return SELECTED_DOCK_FRAME:GetID()").unwrap(),
        2
    );
    assert!(s.eval::<bool>("return ChatFrame2:IsShown()").unwrap());
    assert!(!s.eval::<bool>("return ChatFrame1:IsShown()").unwrap());
    assert!(s.errors().is_empty(), "handler errors: {:?}", s.errors());
}

/// The stock tabs default to the `GENERAL` and `COMBAT_LOG` GlobalStrings
/// (`FloatingChatFrame.lua:684-686`).
#[test]
fn the_dock_tab_labels_come_from_the_install() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let s = benilla_ui::script::UiScript::new().expect("VM");
    s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
    for key in ["GENERAL", "COMBAT_LOG"] {
        let text: String = s.lua().globals().get(key).unwrap_or_default();
        assert!(!text.is_empty(), "{key} is not a GlobalString");
    }
}

#[test]
fn the_combat_log_window_has_the_docks_rect() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = chat_vm();
    let edge = |frame: &str, get: &str| {
        s.eval::<Option<f64>>(&format!("return {frame}:{get}()"))
            .unwrap()
    };
    for get in ["GetLeft", "GetRight", "GetTop", "GetBottom"] {
        let one = edge("ChatFrame1", get);
        let two = edge("ChatFrame2", get);
        assert!(
            one.is_some(),
            "ChatFrame1:{get}() is nil — the dock has no rect"
        );
        assert_eq!(
            two, one,
            "ChatFrame2:{get}() must equal ChatFrame1's — a docked window shares the dock's rect"
        );
    }
    // `FCF_DockUpdate` anchors a docked window by three points (`FloatingChatFrame.lua:1059-1063`),
    // so 3 means the dock seeding ran.
    assert_eq!(
        s.eval::<i64>("return ChatFrame2:GetNumPoints()").unwrap(),
        3,
        "FCF_DockUpdate's three points — any other count means the dock never seeded this window"
    );
    assert_eq!(
        s.eval::<i64>("return table.getn(DOCKED_CHAT_FRAMES)")
            .unwrap(),
        2,
        "the dock is ChatFrame1 + ChatFrame2, seeded from GetChatWindowInfo's stored positions"
    );
    assert_eq!(
        s.eval::<String>("return SELECTED_DOCK_FRAME:GetName()")
            .unwrap(),
        "ChatFrame1"
    );
    assert!(
        s.eval::<bool>(
            "return ChatFrame1.isDocked and ChatFrame2.isDocked and not ChatFrame3.isDocked"
        )
        .unwrap(),
        "FCF_DockFrame set the flag on the dock's two and nothing else"
    );
}

/// The stock fade and tint walk `CHAT_FRAME_TEXTURES` by frame name (`FloatingChatFrame.lua:14`),
/// so each dock window carries all nine pieces.
#[test]
fn both_dock_windows_carry_the_same_chrome() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = chat_vm();
    // Read out of the VM, so the check follows the list.
    let suffixes: Vec<String> = (1..=9)
        .map(|i| {
            s.eval::<String>(&format!("return CHAT_FRAME_TEXTURES[{i}]"))
                .unwrap()
        })
        .collect();
    assert_eq!(
        suffixes.len(),
        9,
        "the background plus the eight resize grips' textures"
    );
    for suffix in &suffixes {
        for id in [1, 2] {
            assert!(
                s.eval::<bool>(&format!("return ChatFrame{id}{suffix} ~= nil"))
                    .unwrap(),
                "ChatFrame{id}{suffix} is missing — the dock's chrome dies when window {id} is up"
            );
        }
    }
    // Each texture's owning grip Button exists too.
    for grip in [
        "TopLeft",
        "TopRight",
        "BottomLeft",
        "BottomRight",
        "Top",
        "Bottom",
        "Left",
        "Right",
    ] {
        for id in [1, 2] {
            assert!(
                s.eval::<bool>(&format!("return ChatFrame{id}Resize{grip} ~= nil"))
                    .unwrap(),
                "ChatFrame{id}Resize{grip} is missing — the window has art where a handle should be"
            );
        }
    }
}

/// The stock pass anchors its `ChatFrame2` row (`UIParent.lua:1582`), then `FCF_DockUpdate()`
/// re-anchors every docked window onto `DEFAULT_CHAT_FRAME`.
#[test]
fn a_docked_chat_window_survives_the_managed_position_pass() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().expect("VM");
    s.set_screen_size(1024.0, 768.0);
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    assert!(crate::ui_script::load_default_ui(&s).is_empty());
    s.resolve();
    // Dock the combat log the way the reference's own default layout does, then run the pass.
    s.run("FCF_DockFrame(ChatFrame2, 2, nil) UIParent_ManageFramePositions()")
        .unwrap();
    s.resolve();
    let rect = |name: &str| {
        s.eval::<(f64, f64, f64, f64)>(&format!(
            "return {name}:GetLeft(), {name}:GetBottom(), {name}:GetRight(), {name}:GetTop()"
        ))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
    };
    assert_eq!(
        rect("ChatFrame2"),
        rect("ChatFrame1"),
        "the docked window still sits exactly on the dock"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The swim bit (`EmoteFlags & 0x0080`, `0x47db7d`) is clear on every posture emote, so the
/// refusal to sit in water is the stand-state setter's (`player::state::stand_state_refused`).
#[test]
fn the_posture_emotes_carry_no_swim_suppression_flag() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = benilla_formats::load_emote_sound_catalog(&mut chain).expect("emote catalog");
    // Every posture row, found by scanning.
    let posture: Vec<(u32, u32, u32)> = (0..600u32)
        .filter_map(|id| Some((id, cat.posture_state(id)?, cat.emote_flags(id)?)))
        .collect();
    let states: Vec<u32> = posture.iter().map(|&(_, s, _)| s).collect();
    for state in [0u32, 1, 3, 8] {
        assert!(
            states.contains(&state),
            "the shipped table has a posture emote for stand state {state}; found {states:?}"
        );
    }
    for (id, state, flags) in posture {
        assert_eq!(
            flags & 0x0080,
            0,
            "posture emote {id} (state {state}) carries the swim-suppress bit: {flags:#x}"
        );
        // STATE_DEAD (emote 65, `0x6602`) carries the unconditional suppress bit `0x0400`.
        if flags & 0x0400 != 0 {
            assert_eq!(state, 7, "only STATE_DEAD is unconditionally suppressed");
            continue;
        }
        // A standing swimmer's posture emote passes the gate.
        assert_eq!(
            super::input::emote_send_eligible(flags, 0, true, 0),
            super::input::EmoteGate::Send,
            "posture emote {id} passes the emote gate while swimming"
        );
    }
}

/// `SMSG_LEVELUP_INFO`'s gains park on `ChatLog` until the level edge that matches them.
#[test]
fn level_up_gains_are_matched_by_level_and_a_miss_is_not_an_absence() {
    use benilla_protocol::messages::LevelUpInfo;

    let mut log = super::feed::ChatLog::default();
    let ding = LevelUpInfo {
        level: 10,
        health: 22,
        powers: [15, 0, 0, 0, 0],
        stats: [0, 1, 2, 3, 0],
    };
    log.push_level_up_gains(&ding, 1);

    // A level edge that is not this ding's leaves the entry parked...
    assert!(log.take_level_up_gains(11).is_none(), "wrong level matched");
    // ...and the right one takes it, exactly once.
    let (got, talent_points) = log.take_level_up_gains(10).expect("parked for level 10");
    assert_eq!(got, ding);
    assert_eq!(talent_points, 1);
    assert!(
        log.take_level_up_gains(10).is_none(),
        "the entry was taken, not copied"
    );
}

/// Stock `ChatFrame_OnEvent` prints the whole level-up block from `PLAYER_LEVEL_UP`
/// (`ChatFrame.lua:1283-1323`); the app composes none of it.
#[test]
fn the_ding_block_is_printed_once() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_protocol::messages::LevelUpInfo;
    let mut s = chat_vm();
    let mut log = super::ChatLog::default();

    let info = LevelUpInfo {
        level: 10,
        health: 22,
        powers: [15, 0, 0, 0, 0],
        stats: [1, 0, 0, 0, 0],
    };
    // The packet's apply parks the gains and prints nothing.
    log.push_level_up_gains(&info, 1);
    let before = lines_in_window(&s);
    assert_eq!(
        lines_in_window(&s) - before,
        0,
        "the app composes no ding line of its own"
    );

    // Tap the window's `AddMessage` to read the block itself.
    s.run(
        r#"
        DingLines = {}
        local add = ChatFrame1.AddMessage
        ChatFrame1.AddMessage = function(self, text, ...)
            table.insert(DingLines, text)
            return add(self, text, unpack(arg))
        end
        "#,
    )
    .unwrap();

    // The event `ui_unit` fires, with the reference's nine arguments.
    let args: Vec<benilla_ui::script::ScriptValue> = [10i64, 22, 15, 1, 1, 0, 0, 0, 0]
        .into_iter()
        .map(benilla_ui::script::ScriptValue::Int)
        .collect();
    s.fire_event("PLAYER_LEVEL_UP", args);
    assert!(s.errors().is_empty(), "handler errors: {:?}", s.errors());
    assert_eq!(
        lines_in_window(&s) - before,
        4,
        "LEVEL_UP, the health/mana pair, CHAR_POINTS, and one STAT — once each"
    );

    // The singular `LEVEL_UP_CHAR_POINTS` (`GetText`'s plural pick) and one `LEVEL_UP_STAT`.
    let lines: Vec<String> = (1..=4)
        .map(|i| s.eval::<String>(&format!("return DingLines[{i}]")).unwrap())
        .collect();
    assert_eq!(
        lines,
        [
            "Congratulations, you have reached level 10!",
            "You have gained 22 hit points and 15 mana.",
            "You have gained 1 talent point.",
            "Your Strength increases by 1.",
        ]
    );
}

/// On `CHARACTER_POINTS_CHANGED` with `arg2 > 0`, stock `ChatFrame_OnEvent` prints
/// `LEVEL_UP_SKILL_POINTS` (`ChatFrame.lua:1324-1334`).
#[test]
fn the_free_professions_line_is_printed_once() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    s.set_talents(benilla_ui::script::TalentUiState {
        points: (0, 2),
        ..Default::default()
    });
    s.run(
        r#"
        SkillLines = {}
        local add = ChatFrame1.AddMessage
        ChatFrame1.AddMessage = function(self, text, ...)
            table.insert(SkillLines, text)
            return add(self, text, unpack(arg))
        end
        "#,
    )
    .unwrap();
    let before = lines_in_window(&s);
    // `ui_talent`'s fire: arg1 = the talent delta, arg2 = the profession delta.
    s.fire_event(
        "CHARACTER_POINTS_CHANGED",
        vec![
            benilla_ui::script::ScriptValue::Int(0),
            benilla_ui::script::ScriptValue::Int(1),
        ],
    );
    assert!(s.errors().is_empty(), "handler errors: {:?}", s.errors());
    assert_eq!(
        lines_in_window(&s) - before,
        1,
        "the stock frame prints the professions line from the event"
    );
    assert_eq!(
        s.eval::<String>("return SkillLines[1]").unwrap(),
        "You now have 2 free professions.",
        "LEVEL_UP_SKILL_POINTS_P1, plural-picked by GetText on cp2"
    );
}

// ───────────── the chat cache restores inside the login ─────────

/// `UPDATE_CHAT_WINDOWS` registers the chat frames for `CHAT_MSG_*`, and the `UPDATE_CHAT_COLOR`
/// burst repaints id-0 lines through `ChatTypeInfo["REPLY"]`, so both come before `PLAYER_LOGIN`.
/// The probe is a loose addon, because the entry load builds the VM it runs on.
#[test]
fn the_chat_cache_restore_is_finished_before_player_login() {
    let _data = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = std::env::temp_dir().join(format!("benilla-chat-order-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let home = tmp.join("benilla-config");
    let probe = home.join("AddOns").join("ChatOrderProbe");
    std::fs::create_dir_all(&probe).expect("hermetic home + probe addon dir");
    std::fs::write(
        probe.join("ChatOrderProbe.toc"),
        "## Interface: 11200\nChatOrderProbe.lua\n",
    )
    .expect("probe toc");
    std::fs::write(
        probe.join("ChatOrderProbe.lua"),
        r#"
        ChatOrderProbe = { order = "" }
        local f = CreateFrame("Frame")
        f:RegisterEvent("VARIABLES_LOADED")
        f:RegisterEvent("UPDATE_CHAT_WINDOWS")
        f:RegisterEvent("UPDATE_CHAT_COLOR")
        f:RegisterEvent("PLAYER_LOGIN")
        f:SetScript("OnEvent", function()
            -- The burst is 100+ events; record it once so the order string stays readable.
            if not string.find(ChatOrderProbe.order, event, 1, 1) then
                ChatOrderProbe.order = ChatOrderProbe.order .. event .. " "
            end
            if event == "UPDATE_CHAT_WINDOWS" then
                ChatOrderProbe.windows = (ChatOrderProbe.windows or 0) + 1
            elseif event == "UPDATE_CHAT_COLOR" then
                ChatOrderProbe.colors = (ChatOrderProbe.colors or 0) + 1
            elseif event == "PLAYER_LOGIN" then
                ChatOrderProbe.loginWindows = ChatOrderProbe.windows or 0
                ChatOrderProbe.loginColors = ChatOrderProbe.colors or 0
                ChatOrderProbe.loginRegistered =
                    (ChatFrame1 and ChatFrame1.messageTypeList
                        and table.concat(ChatFrame1.messageTypeList, ",")) or ""
            end
        end)
        "#,
    )
    .expect("probe lua");
    let _capture = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
    let _home =
        crate::local_state::test_env::EnvGuard::set("BENILLA_HOME", home.to_str().expect("utf-8"));

    let mut world = bevy::prelude::World::new();
    world.init_resource::<crate::ui_script::AddOnIdentity>();
    world.init_resource::<crate::minimap::MinimapZoom>();
    world.init_resource::<crate::ui_script::ReloadUiPending>();
    world.init_resource::<super::edit::ChannelState>();
    world.init_resource::<super::settings::ChatWindowFile>();
    crate::ui_script::setup_script(&mut world);

    world.insert_resource(crate::char_select::Roster::with_pending_pick(
        vec![benilla_protocol::Character {
            guid: 1,
            name: "Probeorder".into(),
            race: 1,  // Human → Alliance
            class: 1, // Warrior
            gender: 0,
            level: 60,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
            zone: 0,
            map: 0,
            position: benilla_protocol::wire::Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            flags: 0,
            equipment: [benilla_protocol::CharEnumItem::default(); 19],
            pet_display_id: 0,
            pet_level: 0,
            pet_family: 0,
        }],
        1,
    ));
    crate::ui_script::load_ingame_ui_on_world_entry(&mut world);

    let read = |expr: &str| -> String {
        world
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval::<Option<String>>(&format!("return tostring({expr})"))
            .ok()
            .flatten()
            .unwrap_or_default()
    };
    assert_eq!(
        read("ChatOrderProbe.loginWindows"),
        "1",
        "UPDATE_CHAT_WINDOWS must have fired before PLAYER_LOGIN — it is the only thing that \
         registers a chat frame for CHAT_MSG_*, so a line routed before it is dropped in silence"
    );
    assert_ne!(
        read("ChatOrderProbe.loginColors"),
        "0",
        "the UPDATE_CHAT_COLOR burst must precede PLAYER_LOGIN — after it, its WHISPER→REPLY \
         mirror repaints every already-printed AceConsole line whisper-pink"
    );
    assert!(
        read("ChatOrderProbe.loginRegistered").contains("SYSTEM"),
        "ChatFrame1 must carry the SYSTEM message group at PLAYER_LOGIN, not {:?}",
        read("ChatOrderProbe.loginRegistered")
    );
    // The reference's login order: `ADDON_LOADED` (`0x4900a3`), `VARIABLES_LOADED` (`0x4900b2`),
    // the chat-cache burst (`0x4900d6`), `PLAYER_LOGIN` (`0x490959`).
    assert_eq!(
        read("ChatOrderProbe.order"),
        "VARIABLES_LOADED UPDATE_CHAT_WINDOWS UPDATE_CHAT_COLOR PLAYER_LOGIN ",
        "the reference fires the chat-cache burst BETWEEN VARIABLES_LOADED and PLAYER_LOGIN"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// `SendChatMessage`'s `CHANNEL` target through the real drain: `SStrToInt` into `0x49be50`
/// (`0x49f4d9`-`0x49f4ea`), so the packet carries the numbered slot's name, and a number naming
/// no confirmed slot, or a name, sends nothing at all.
#[test]
fn a_channel_send_carries_the_numbered_slots_name() {
    use super::edit::{ChannelSlot, ChannelState, SlotState};
    use crate::net::{ChatKind, ClientCommand, NetCommands};
    use bevy::ecs::system::RunSystemOnce;

    let mut world = bevy::prelude::World::new();
    world.insert_non_send_resource(benilla_ui::script::UiScript::new().expect("VM"));
    let (tx, rx) = crossbeam_channel::unbounded();
    world.insert_resource(NetCommands(tx));
    world.init_resource::<super::feed::ChatLog>();
    world.init_resource::<super::away::AfkMirror>();
    world.init_resource::<crate::cvars::Cvars>();
    world.insert_resource(ChannelState {
        joined: vec![
            Some(ChannelSlot::joined("General - Elwynn Forest")),
            Some(ChannelSlot::joined("Trade - City")),
            None,
            Some(ChannelSlot {
                name: "LocalDefense - Elwynn Forest".into(),
                state: SlotState::Suspended,
            }),
        ],
        ..Default::default()
    });
    world
        .non_send_resource::<benilla_ui::script::UiScript>()
        .run(
            r#"
            SendChatMessage("lf1m", "CHANNEL", nil, 2)
            SendChatMessage("hello", "CHANNEL", nil, "1")
            SendChatMessage("float", "CHANNEL", nil, 2.7)
            SendChatMessage("hole", "CHANNEL", nil, 3)
            SendChatMessage("suspended", "CHANNEL", nil, 4)
            SendChatMessage("past the end", "CHANNEL", nil, 9)
            SendChatMessage("by name", "CHANNEL", nil, "General - Elwynn Forest")
            SendChatMessage("zero", "CHANNEL", nil, "0")
            SendChatMessage("none", "CHANNEL")
            SendChatMessage("tell", "WHISPER", nil, "2")
            "#,
        )
        .expect("lua");
    world
        .run_system_once(super::input::drain_addon_chat_sends)
        .expect("drain");

    let sent: Vec<(ChatKind, Option<String>, String)> = rx
        .try_iter()
        .map(|c| match c {
            ClientCommand::Chat { kind, target, text } => (kind, target, text),
            other => panic!("unexpected command {other:?}"),
        })
        .collect();
    let chan = |target: &str, text: &str| {
        (
            ChatKind::Channel,
            Some(target.to_string()),
            text.to_string(),
        )
    };
    assert_eq!(
        sent,
        vec![
            chan("Trade - City", "lf1m"),
            chan("General - Elwynn Forest", "hello"),
            chan("Trade - City", "float"),
            (ChatKind::Whisper, Some("2".into()), "tell".into()),
        ]
    );
}
