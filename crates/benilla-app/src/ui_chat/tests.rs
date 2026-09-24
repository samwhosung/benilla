use super::event::{default_color, ChatEvent, ChatEventKind as K};
use super::input::{emote_send_eligible, emote_target, EmoteGate, ParsedChat};

thread_local! {
    /// The **shipped** string table, in a VM, once per test thread — `GlobalStrings.lua` run as
    /// the chunk it is, so `\32` and every other escape is Lua's own doing rather than a parser
    /// of ours.
    ///
    /// The composer resolves `CHAT_<TYPE>_GET` / `CHAT_<X>_NOTICE` off the player's chain now
    /// (decision 2045), so an assertion below is only worth making against the real table: a test
    /// that graded a rendered line against a stub would pass on sentences the running client
    /// never shows, which is the trap decision 2052 named when it moved the glue tests onto the
    /// loader's own assembly.
    ///
    /// Built lazily, so a caller's `wow_data_or_skip!()` runs first.
    static GLOBAL_STRINGS: benilla_ui::script::UiScript = {
        let s = benilla_ui::script::UiScript::new().expect("VM");
        crate::ui_script::test_ui::load_ui(&s, "Interface\\FrameXML\\GlobalStrings.lua");
        s
    };
}

/// Run something that needs the shipped string table — the feed's line producers resolve their
/// keys through one of these now (decision 2045), and every assertion below wants the REAL table
/// under it rather than a stub with our own idea of the wording in it.
fn with_strings<T>(f: impl FnOnce(&dyn Fn(&str) -> Option<String>) -> T) -> T {
    GLOBAL_STRINGS.with(|s| f(&|key: &str| s.lua().globals().get::<String>(key).ok()))
}

/// [`super::frames::compose`] against the shipped table — the shim that keeps every assertion in
/// this file (and [`super::broadcast`]'s own) reading as a rendered line rather than as a lookup.
pub(super) fn compose(event: &ChatEvent, kind: K, default_language: &str) -> Option<String> {
    GLOBAL_STRINGS.with(|s| {
        super::frames::compose(event, kind, default_language, &|key| {
            s.lua().globals().get::<String>(key).ok()
        })
    })
}

/// A player-line event (the wire bridge's output shape) — sender resolved, optional flag.
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
    // The composer emits the REAL |Hplayer link now (ref ChatFrame.lua l.1451); the renderer
    // strips the markers and spans the [Name] (the P2 markup law).
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
    // EMOTE uses the bare name (l.1450 `type ~= "EMOTE"`).
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
    // A received AFK auto-reply: CHAT_AFK_GET (whisper-pink family).
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
    // Common (our default) and Universal (empty) render no header.
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
    // A LOOT line arrives already composed by `ui_loot::receive_line`, item link and all; compose
    // must pass the escapes through untouched (the quality colour is the link's, not the line's).
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

/// B156's visible half, in one assertion: a TEXT_EMOTE line renders **verbatim**, and setting the
/// performer in `sender` (arg2, for addons) must not make the composer bracket a name onto it the
/// way it does for SAY. If this ever starts reading "[Bob] Bob waves.", the sender slot has leaked
/// into the render.
#[test]
fn text_emote_lines_are_verbatim_and_never_wear_the_senders_name() {
    let _data = benilla_formats::wow_data_or_skip!();
    let e = ev(K::TextEmote, "Bob waves at you.", "Bob");
    assert_eq!(
        compose(&e, K::TextEmote, "Common").unwrap(),
        "Bob waves at you."
    );
    // The control: the same event as a SAY *does* get the bracketed link, so the assertion above
    // is about the TEXT_EMOTE arm and not about `compose` having stopped decorating anything.
    assert!(compose(&ev(K::Say, "hi", "Bob"), K::Say, "Common")
        .unwrap()
        .contains("[Bob]"));
}

/// **A self-target goes out as guid 0** — `DoEmote`'s last act before it builds the packet
/// (`0x5ef611`), and the reason vanilla has no self-emote sentence (decision 1282, correcting
/// 1274's claim that you would read "You wave at ⟨YourName⟩.").
///
/// Without this the server echoes your own name back as the emote's target and the *whole zone*
/// reads "Sam waves at Sam." — so the control below (a selection that is someone else survives
/// intact) is what makes this a gate and not a mute button.
#[test]
fn emoting_at_your_own_selection_sends_an_untargeted_emote() {
    let _data = benilla_formats::wow_data_or_skip!();
    use crate::target::Selection;
    use bevy::prelude::Entity;

    let me = Entity::from_raw_u32(7).unwrap();
    let them = Entity::from_raw_u32(9).unwrap();

    // Myself selected: the guid is dropped on the floor, exactly as `mov [ebp+0xc],ebx` does.
    let sel = Selection {
        target: Some(me),
        guid: Some(0xdead_beef),
    };
    assert_eq!(emote_target(&sel, Some(me)), 0);

    // The control — someone else selected: the guid goes out untouched.
    let sel = Selection {
        target: Some(them),
        guid: Some(0xdead_beef),
    };
    assert_eq!(emote_target(&sel, Some(me)), 0xdead_beef);

    // No selection at all is already untargeted, and a not-yet-streamed self entity must not make
    // an empty selection look like a self-target (the `me.is_some()` guard).
    assert_eq!(emote_target(&Selection::default(), Some(me)), 0);
    let sel = Selection {
        target: Some(them),
        guid: Some(0xdead_beef),
    };
    assert_eq!(emote_target(&sel, None), 0xdead_beef);
}

/// The receive half of B156 on the real tables (decision 1274): the five reachable sentence forms,
/// the performer in arg2, and the three silent rows. The composition law itself is pinned in
/// `benilla_formats::emote_text`; what this covers is the seam — that the app hands the composer
/// the right facts and puts the result in the right slots. Skips without client data.
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
        // arg2 is the performer, not the target — the reference pushes the performer's NameCache
        // record (`0x49b47c`).
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
    // SIT's columns point at EmotesTextData rows that ship blank: no line, not an empty one.
    assert!(line(SIT, them("-")).is_none(), "/sit prints nothing");
}

/// The three honor forms (COMBATLOG_HONORAWARD / COMBATLOG_HONORGAIN / COMBATLOG_DISHONORGAIN,
/// GlobalStrings :786/:787/:785) and the fork between them, decision 1512.
///
/// The empty-rank case is asserted deliberately: it is what the server's floor-at-5 exists to
/// prevent, so a change that silently starts hiding the clause instead would pass unnoticed here
/// without it.
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
    // A dishonorable kill: vmangos sends the same packet with a negative honor
    // (`HonorMgr.cpp:807`), and the client's fork is on the sign.
    assert_eq!(
        with_strings(|g| super::feed::honor_gain_line(Some("Innkeeper Renee"), None, -37, g))
            .as_deref(),
        Some("Innkeeper Renee dies, dishonorable kill.")
    );
    // The BOUNDARY, byte-verified at `0x625270`: the test is `honor <= 0`, so a zero-honor kill
    // takes the dishonorable arm. The pre-verdict reading had `< 0` and put this one on the
    // honorable side, where it would have printed "Rank:  (Estimated Honor Points: 0)".
    assert_eq!(
        with_strings(|g| super::feed::honor_gain_line(Some("Grimtusk"), Some("Sergeant"), 0, g))
            .as_deref(),
        Some("Grimtusk dies, dishonorable kill.")
    );
    // No rank title: the clause stays, empty — the reference's own shape.
    assert_eq!(
        with_strings(|g| super::feed::honor_gain_line(Some("Grimtusk"), None, 5, g)).as_deref(),
        Some("Grimtusk dies, honorable kill Rank:  (Estimated Honor Points: 5)")
    );
}

#[test]
fn xp_gain_lines_pick_the_reference_form() {
    let _data = benilla_formats::wow_data_or_skip!();
    // COMBATLOG_XPGAIN_FIRSTPERSON / its EXHAUSTION1 rested form / _UNNAMED (GlobalStrings
    // :801/:789/:804).
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
    // The XP kind wears the shipped lavender (chat-cache row 46, 0x6F6FFF).
    assert_eq!(default_color(K::CombatXpGain), [111, 111, 255]);
}

#[test]
fn exploration_lines_pick_the_reference_form() {
    let _data = benilla_formats::wow_data_or_skip!();
    // ERR_ZONE_EXPLORED (GlobalStrings :1925) — the toast, fired on EVERY exploration packet
    // (UIErrorsFrame); ERR_ZONE_EXPLORED_XP (:1926) — the chat system line that rides
    // additionally iff xp > 0 (byte-verified branch `0x5e422f`; decisions 0828/0829).
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

/// The notice arms print arg4 **whole** — zone tail and all (1275).
///
/// The pair to hold in view is [`channel_line_prefixes_the_stripped_channel`] directly above: the
/// same channel, the same arg4, and the reference renders them differently. `gsub(arg4,
/// "%s%-%s.*", "")` lives at l.1463, inside the speech `else` arm, *after* every notice arm has
/// returned — so speech says "[General]" and the join notice says "[General - Elwynn Forest]".
///
/// **The fixtures are built by the bridge**, not hand-stamped with a kind. CHANNEL_NOTICE and
/// CHANNEL_NOTICE_USER are two different arms of `ChatFrame_OnEvent` — one passes arg4 alone, the
/// other arg4/arg2/arg5 (l.1416/1424) — and which one a notice byte takes is
/// [`super::feed::notice_event`]'s call, not the test author's. Stamping it by hand is how this
/// test came to describe PLAYER_KICKED as a plain CHANNEL_NOTICE, which the bridge has never
/// produced; nothing could see it while the composer ignored the distinction.
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
    // A member join line is a CHANNEL_JOIN event, hyperlinked like any player line.
    let mut join = ev(K::ChannelJoin, "", "Ann");
    join.channel = "World".into();
    assert_eq!(
        compose(&join, K::ChannelJoin, "Common").unwrap(),
        "[World] |Hplayer:Ann|h[Ann]|h joined channel."
    );
}

// ── the Lua face: the real CHAT_MSG_* fire (0288 §1's addon-API phase) ────────────────────────
//
// These drive the REAL router into a REAL VM with our shipped ChatFrame.xml under it, because the
// question they exist to answer — "does anything print twice now?" — cannot be answered by
// reasoning about the composer in isolation. `route` both renders and fires; only a VM holding
// our actual window can show that the two do not add up to two lines.

/// A fresh VM with the shipped chat stack under it — the same files the app loads, so `ChatFrame1`
/// here is the real window carrying its real `<OnEvent>`.
fn chat_vm() -> benilla_ui::script::UiScript {
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    // GameTooltip.xml + UIDropDownMenu.xml are real RUNTIME dependencies of the chat tabs since
    // decision 1589: a left click closes any open menu (`CloseDropDownMenus`, the reference's own
    // first move) and a right click opens the window's options menu. `benilla.toc` already orders
    // both ahead of ChatFrame.xml (l.60/64 vs l.399); the harness says so too, rather than a guard
    // that would hide a real ordering fault. (The tooltip file is the dropdown kit's own
    // dependency — its MenuBackdrop reads `TOOLTIP_DEFAULT_COLOR`.)
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        // The UIMenu kit is the reference's own file since 1751 window 21, so this reads both
        // stores through the one loader that speaks them.
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

/// An "addon" that records what a `CHAT_MSG_*` fire actually delivered — the count, the event
/// name, and `arg1..arg10` joined with `|`. The concatenation is the point: a `nil` in any slot
/// raises in Lua, so a passing read is itself the proof that all ten args arrived.
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

/// **The double-print answer, proved rather than argued.**
///
/// In the reference, `CHAT_MSG_SAY` is what MAKES the line: C fires it, `ChatFrame_OnEvent` calls
/// `AddMessage`. benilla composes and adds in Rust instead (0288 §1) and now fires the event as
/// well — so the honest worry is that an addon registering the event on *our own* ChatFrame1 makes
/// the line land twice. It does not, and the mechanism is that our shipped `ChatFrame.xml`
/// `<OnEvent>` handles exactly one event (`EXECUTE_CHAT_LINE`) and ignores everything else.
///
/// The spy is a control, not decoration: without it a broken fire would pass this test.
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

    // Now the addon registers OUR window for the event, exactly as the reference's own
    // FloatingChatFrame does. This is the double-print case if there is one.
    s.run(r#"ChatFrame1:RegisterEvent("CHAT_MSG_SAY")"#)
        .unwrap();
    super::frames::route(&mut s, &mut windows, &ev(K::Say, "hi again", "Bob"));
    assert_eq!(
        lines_in_window(&s),
        2,
        "one more line, not two — ChatFrame1's own OnEvent does not render CHAT_MSG_*"
    );
    assert_eq!(s.eval::<i64>("return SpyN").unwrap(), 2);
    assert!(s.errors().is_empty(), "handler errors: {:?}", s.errors());
}

/// **The line is already in the window when an addon's handler runs.** The reference dispatches to
/// listeners in registration order and ChatFrame1 registers at FrameXML load, before any addon
/// exists — so an addon that reads `GetNumMessages()` (or re-reads the last line to recolour it)
/// from its own `CHAT_MSG_*` handler sees the line, not the gap before it. Our Rust composer stands
/// in for ChatFrame1's handler, so it has to run first for the same reason.
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

/// A player line fires `CHAT_MSG_SAY` with the reference's own arg positions — including the two
/// slots our doc comment used to omit (arg7, arg10), both numbers, both zero for a non-channel
/// line.
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
    // arg1 is the RAW body, not the composed line — the reference's Lua is what adds
    // "%s says: " and the |Hplayer link, so an addon must see what the wire sent.
    assert_eq!(
        s.eval::<String>("return SpyLine").unwrap(),
        "throm-ka|Grunk|Orcish|||GM|0|0||0"
    );
}

/// A channel notice fires the **token** in arg1 (not the rendered line), the numbered display form
/// in arg4, and the three numeric slots the reference reads bare.
///
/// The last assertion runs `ChatFrame_OnEvent`'s own two comparisons — `arg7 > 0` and
/// `arg10 > 0` — inside the handler. Under Lua 5.0 a `nil` there raises, so this is the test that
/// would have caught passing nine args instead of ten.
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
    // The reference prints a channel-family line only for a channel the WINDOW carries
    // (`ChatFrame_OnEvent` l.1374-1391 walks `this.channelList`; a miss returns) — the list
    // `/join`'s handler fills through `ChatFrame_AddChannel`.
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
    // ...and the window still shows the one composed line it always did.
    assert_eq!(lines_in_window(&s), 1);
}

/// **MODE_CHANGE produces no chat event at all** — not a silent one.
///
/// This test replaced an earlier one that asserted the opposite (that a notice the UI renders
/// silently still reaches Lua, using MODE_CHANGE as the example). The bytes settle it
/// the other way: `0x49c24d`, the `0x0C` arm of the notice jump table, calls `0x49e910` and
/// **returns** — it never reaches the fire. So the right
/// behaviour is what our feed already does: drop it before it becomes an event, which is what this
/// now asserts.
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

    // The control: a notice that DOES fire still gets queued, so the assertion above is about
    // MODE_CHANGE and not about `push_channel_notice` being broken.
    log.push_channel_notice(
        channel_notice::YOU_JOINED,
        "World".into(),
        &ChannelNoticeTail::YouJoined { flags: 0 },
    );
    assert_eq!(log.pending_len(), 1);
}

/// A channel line whose channel we are **not** in leaves all four channel slots empty — arg4 falls
/// back to the bare name and arg7/arg8/arg9 stay `0/0/""`. They are one record in the reference
/// (`slot+0x00/+0x04/+0x94/+0x98`, defaulted together at `0x49b12f`), so they are one record here.
#[test]
fn a_channel_we_are_not_in_fires_the_bare_name_and_zeroes() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    s.run(SPY).unwrap();
    s.run(r#"BenillaChatSpy:RegisterEvent("CHAT_MSG_CHANNEL")"#)
        .unwrap();

    // An empty joined list: nothing is in the local channel record array.
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

/// `stamp_channel` splits the wire's bare name into the reference's arg4/arg8/arg9 trio: the
/// display form gets the number prefix, arg9 never does.
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
    assert_eq!(e.channel_base, "General - Elwynn Forest"); // arg9, " - Zone" tail intact

    // A channel we are not in keeps its bare name in arg4 and leaves the whole rest of the record
    // empty — the reference's miss leg `0x49aa86`, where there is no local record to read
    // `slot+0x04/+0x94/+0x98` out of at all.
    let mut other = ev(K::Channel, "hi", "Bob");
    other.channel = "SomeoneElsesChannel".into();
    channels.stamp_channel(&mut other);
    assert_eq!(other.channel, "SomeoneElsesChannel"); // arg4: the bare incoming name
    assert_eq!(other.channel_number, 0); // arg8
    assert_eq!(other.channel_base, ""); // arg9 — NOT the name
    assert_eq!(other.zone_channel_id, 0); // arg7
}

/// **A channel notice renders in the CHANNEL row, not the CHANNEL_NOTICE row** (1275).
///
/// `ChatFrame_OnEvent` looks up `ChatTypeInfo[type]` and then overwrites it for the whole channel
/// family: `info = ChatTypeInfo["CHANNEL"..arg8]` (l.1381). So the grey C0C0C0 the CHANNEL_NOTICE
/// row carries is looked up and thrown away, and the join line comes out the channel's FFC0C0 —
/// which is what the director's eye caught: our notices read white-grey where the client's read
/// warm. Driven through the real router into the real window and read back off the extracted
/// quad, because the color that matters is the one that reaches the screen.
#[test]
fn a_channel_notice_renders_in_the_channels_color_not_the_notice_row() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::QuadContent;

    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let mut channels = super::edit::ChannelState::default();
    channels.claim_slot("General - Elwynn Forest");

    // The window has to carry the channel for the reference's handler to print the line at all
    // (`ChatFrame_OnEvent` l.1374-1391) — `/join`'s own bookkeeping, done here by hand.
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

/// **Crossing a zone border must not deregister the channel it renames** (decision 2130).
///
/// The walk sends `LEAVE(General - Elwynn Forest)` then `JOIN(General - Westfall)` — one DBC row,
/// renamed, and the retail 1.8.1 Winterspring sniff shows exactly that
/// pair on the wire. The server answers each with a notice, and the stock `ChatFrame_OnEvent`'s
/// `YOU_LEFT` arm **deletes the window's registration for whatever it matched**
/// (`ChatFrame.lua` l.1382-1384):
///
/// ```lua
/// this.channelList[index] = nil;
/// this.zoneChannelList[index] = nil;
/// ```
///
/// Nothing in stock FrameXML ever re-adds one on `YOU_JOINED` — `ChatFrame_AddChannel` is reachable
/// only from the `/join` popup and the chat-tab dropdown. So if our `YOU_LEFT` still resolves to a
/// slot, the window loses General for the rest of the session: the replacement join notice is
/// dropped unprinted, and so is every General line spoken in the new zone. That is the director's
/// *"sometimes I get no channel stuff"*, and this test is the observable.
///
/// The window is registered here the way the chat cache registers it at login — the row's
/// **Shortcut** against its **ChannelID**, which is the id-match at `ChatFrame.lua:1379` — because
/// that is the registration the reference's own `chat-cache.txt` produces (`ZONECHANNELS` bits, not
/// names).
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

    // `ChatFrame_RegisterForChannels(GetChatWindowChannels(1))`, by hand: it does exactly this pair
    // of writes, and calling it needs the `this` the event dispatch supplies.
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

    // The border crossing, in the order the walk actually produces it: `CMSG_LEAVE_CHANNEL(old)`
    // goes out, **the slot is renamed in place before the answer can arrive**
    // ([`super::edit::ChannelState::rename_slot`], the reference's `0x49bc50` at pass 1 step 6),
    // `CMSG_JOIN_CHANNEL(new)` goes out, and only then do the two notices land.
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

    // …and the observable that actually matters: speech from the NEW zone still lands.
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

/// **Walking out of a capital SUSPENDS Trade — it does not free it** (decision 2130).
///
/// The zone walk's other leave: a row that stops applying entirely, which in the 1.12 data means
/// exactly `Trade` when you step out of a city (the city gate `0x49a3b8`). The
/// `CMSG_LEAVE_CHANNEL` still goes out, but the client marks its own slot state 3 (`0x49bcf0`) and
/// keeps the record — so the notice comes back as the `SUSPENDED` token, the stock handler's
/// `YOU_LEFT` arm never runs, and the window keeps its registration. Walking back in re-joins
/// through the state-3 bypass onto the same slot, with the same number.
///
/// Freeing it — which is what we did — cost Trade its registration on the way out and left the
/// re-join notice unprintable on the way back in. Same bug as the border crossing, one row over.
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

    // The line still PRINTS — `CHAT_SUSPENDED_NOTICE` is "Left Channel: [%s]", the same text as
    // `CHAT_YOU_LEFT_NOTICE`. Only arg1 differs, and arg1 is what the stock handler branches on.
    // Asserted because a missing string would make `compose_notice` answer `None` and the line
    // would vanish silently — the failure this whole area is prone to.
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

    // Walking back in: the same slot, the same number, and the notice prints again.
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

/// **A renamed row's confirming notice is `YOU_CHANGED`, and it renders "Changed Channel:"**
/// (decision 2130).
///
/// `CHAT_YOU_CHANGED_NOTICE = "Changed Channel: [%s]"` is a string 1.12 ships and we had never
/// printed, because we modelled no per-slot state to select it with (`0x02`'s arm splits on
/// `rec+0x9c == 2`). It is what a zone-border crossing actually looks like in the reference: one
/// line, not a leave and a join.
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

/// **The leave line still knows its number, because the record dies after the line** (1275).
///
/// [`super::feed::deliver`] is the ordering under test: we used to drop the channel from the joined
/// list before composing, so `stamp_channel` missed and the line came out "Left Channel: [General]"
/// — unnumbered, and (with the color override above) resolved against arg8 = 0. The reference's
/// YOU_LEFT arm flags the teardown and runs it *after* the fire (`0x49c5b0` fire, `0x49c5c2 call
/// 0x49bbd0`), so the line is numbered and an addon's handler still sees the channel.
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
        "and only THEN is the record gone — as a HOLE at slot 2, not a shortened list (1286)"
    );
}

/// **A channel that leaves does not renumber the ones that stay** (1286).
///
/// The director's teleport tour: `Left Channel: [1. General - Teldrassil]` /
/// `Joined Channel: [2. General - The Barrens]` / `Left Channel: [1. LocalDefense - Teldrassil]`,
/// with Trade shuffling 1 → 2 → 3 across the same few seconds — every number in the window moving
/// because the list closed each hole. The reference frees the slot in place and refills the first
/// free one, so a zone hop *renames* a channel and leaves its number alone.
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

    // The ceiling is the reference's ten (`0x49b9c0: cmp ecx,0xa`), counted in SLOTS.
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

/// **The next character does not inherit this one's chat window** (1288).
///
/// The reference ends a session by destroying its Lua state, so the window that comes back is
/// empty. We keep the VM (`ui_script::IngameUiLoaded` is the latch standing in for that teardown),
/// so the director saw the previous character's `Joined Channel:` lines still sitting under the
/// new character's. Everything the module remembers across a box open goes with the lines.
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

/// **Every notice token names a string the shipped client actually has.** The composer splices
/// the token into `CHAT_<X>_NOTICE` the way `ChatFrame_OnEvent` does (l.1416/1424), so the token
/// table IS the render table — there is nothing left to cross-check between two lists of ours.
/// What can still be wrong is a token that resolves to nothing, which would silently print no
/// line at all; that is what this asserts, against the player's own `GlobalStrings.lua`.
///
/// The bytes with no token render nothing, and must keep rendering nothing: MODE_CHANGE (`0x0C`)
/// fires no chat event in the reference at all, and JOINED/LEFT (`0x00`/`0x01`) are the member
/// lines rather than notices.
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

/// The one notice whose two names are **not** in the order it prints them:
/// `CHAT_INVITE_NOTICE = "%2$s has invited you to join the channel '%1$s'."`, filled from the
/// reference's own fixed `(arg4, arg2)` list (l.1418). This is decision 2045's whole argument in
/// one assertion — hand-typing the English bakes in one locale's word order, and only a
/// positional fill off the shipped string can put the inviter first while the channel is
/// argument one.
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

/// The `ALL` sweep list really is every variant. A new kind fails [`super::event::event_name`]'s
/// exhaustive match at compile time; this is what makes you add it to `ALL` as well.
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
    // 93 since 2077: `CHAT_MSG_FILTERED` (`0x5B`) is a real server line, not the never-wire value
    // this tree had it filed as — the server's "your message was filtered" notice.
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

// ── the submitted-line grammar (0288 P5): type switches + action commands ──────────────────

/// The grammar fixture: a command table built from a *stub* of the reference's alias strings, in
/// the same `SLASH_<INDEX><n>` / `EMOTE<i>_CMD<j>` shape the shipped `GlobalStrings.lua` has. The
/// aliases here are fixture data for the ARGUMENT grammar; that the real ones all resolve is
/// [`real_alias_table_resolves_the_shipped_commands`]'s job, on the real files.
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
        ("EMOTE1_CMD2", "/hello"), // an alias that is NOT the token — the 0881 class of bug
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

/// The Enter-path type switch (send path — no trailing-space requirement).

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
    // `/afk` and `/dnd` run the reference's OWN `SlashCmdList` bodies rather than building a bare
    // send (2088). They used to be a `ParsedChat::AfkDnd` that went straight to the wire — correct
    // until the away law landed, and a second implementation the moment it did: no echo, no
    // client-side default substitution, no mirror write. The argument still rides WHOLE, which is
    // what this row has always pinned.
    assert_eq!(
        parse_line("/afk farming"),
        ParsedChat::Lua {
            body: "SlashCmdList[\"CHAT_AFK\"](\"farming\")".into()
        }
    );
    // Bare, because that is the toggle — and the empty string has to survive to the send.
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
    // /pvp takes no argument (decision 0646 §3): the binding has no state form, so a trailing
    // word is ignored rather than read as a target.
    assert_eq!(parse_line("/pvp"), ParsedChat::Pvp);
    assert_eq!(parse_line("/pvp on"), ParsedChat::Pvp);
    // /r rides its own arm (the reply state lives on ChatEditState).
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
    // The 0881 fix in one line: an alias that is NOT its token's `EmotesText` name resolves too.
    // `/hello` (token WAVE) is the shape `/lol` (token LAUGH) has in the shipped table — before
    // the table, matching on the DBC name alone left 61 such commands unresolvable.
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
    // `/trade` is the ref's `InitiateTrade("target")`, verbatim.
    assert_eq!(
        parse_line("/trade"),
        ParsedChat::Lua {
            body: "InitiateTrade(\"target\")".into()
        }
    );
    // `/script` runs the typed text AS the chunk (the ref's `RunScript(msg)`); bare is a no-op.
    assert_eq!(
        parse_line("/script Print(\"hi\")"),
        ParsedChat::Lua {
            body: "Print(\"hi\")".into()
        }
    );
    assert_eq!(parse_line("/script"), ParsedChat::Unknown);
}

/// `/castvis` is one of benilla's own instruments, so 1179 gates the whole dev alias table behind
/// `run_mode::dev_affordances()` — in a player build the alias is never claimed and the line falls
/// through to the reference's "unknown command". This test therefore asserts the grammar in a dev
/// build and the *absence* of the grammar in a player one, rather than assuming the configuration
/// it happens to run in. (It assumed, until 1180's `player-tests` gate ran it the other way.)
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
    // `ground` is a GO too — the pure-dest shape, the only one that flies the location fallback.
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
    // The regression this grammar exists to fix: `/yell` used to literally SAY "/yell hello" —
    // any unresolved slash-line must never fall through to plain chat.
    assert_eq!(parse_line("/dancemove"), ParsedChat::Unknown);
    assert_eq!(parse_line("/frobnicate"), ParsedChat::Unknown);
}

/// The RUNTIME leg on the real data (the `every_mount_key_resolves…` pattern): build the table the
/// way boot does — the shipped `GlobalStrings.lua` and `ChatFrame.lua`'s token table executed into
/// a real VM, joined to the real `EmotesText.dbc` — and assert the commands 0881 was opened for.
/// Skips without client data.
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
        // GlobalStrings runs whole (it is only assignments); ChatFrame contributes its token
        // table alone, through the production filter.
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

    // The reported symptom: `/sit` resolves to the SIT text emote (EmotesText id 86), whose
    // `Emotes.dbc` row (13, STATE_SIT) is the posture emote that sets stand state 1.
    assert_eq!(parse_line("/sit"), ParsedChat::TextEmote(86));
    assert_eq!(
        cat.text_emote(86).and_then(|e| cat.posture_state(e)),
        Some(1)
    );
    // Every posture command the shipped tables expose, with the state it sets.
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
    // The dead-alias class 0881 found: an alias that differs from its token's DBC name. These all
    // returned "Type '/help'…" before the table.
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
    // …and the three names the reference has NO command for, which the DBC-name match used to
    // accept as emotes.
    for line in ["/joke", "/puzzle", "/attackmytarget"] {
        assert_eq!(parse_line(line), ParsedChat::Unknown, "{line}");
    }
    // `/follow` was the sharp one in that class: the DBC-name match fired a text emote where the
    // real client's `SlashCmdList["FOLLOW"]` follows your target. 0881 made it honestly unknown;
    // 0890 makes it the real command, over all three shipped aliases (SLASH_FOLLOW1-6 → `/f`,
    // `/follow`, `/fol`).
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
    // The by-name selection pair (decision 0886) — every shipped alias, and the whole-argument
    // grammar that makes a multi-word creature name ONE name. `/tar` and `/a` are the short forms
    // the shipped strings carry (SLASH_TARGET2/4, SLASH_ASSIST1/3).
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
    // The macro family (decision 0983): `/cast` (with its `/spell` alias — SLASH_CAST1-4 spell two
    // distinct strings across four slots) runs the ref's own one-line body `CastSpellByName(msg)`;
    // `/macro`/`/m` open the window; `/macrohelp` prints the shipped five lines.
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
    // `/console` from a line that skipped the stock edit box (a probe rig) forwards to the verb
    // the stock handler calls, so a CVar write lands like a typed one (2008). The long-bracket
    // quoting steps its level past anything the text could close.
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
    // **The quoting is a SHORT string, because 1.12's lexer has no long-string levels** (2136).
    // The old long-bracket form stepped its `=` level past whatever the payload could close, which
    // is a construct the reference cannot compile at all — see `lua_quoted_string`.
    assert_eq!(super::input::lua_quoted_string("a]]b"), "\"a]]b\"");
    assert_eq!(super::input::lua_quoted_string("a]]b]=]c"), "\"a]]b]=]c\"");
    assert_eq!(
        super::input::lua_quoted_string("say \"hi\"\\n"),
        "\"say \\\"hi\\\"\\\\n\""
    );
    // …and the generated literal ROUND-TRIPS through a real VM, which is the assertion that
    // actually pins the grammar: the old form compiled here and would not have on the reference,
    // so a string check alone could not have caught it.
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
    // The whole shipped surface, so a table that half-loaded fails loudly: **225 distinct emote
    // commands** over the 169 `EmotesText` names (the strings repeat — `EMOTE87_CMD1` and `_CMD2`
    // are both "/sit" — and EMOTE27 "UNUSED" has no row, so it contributes none), and **68 distinct
    // aliases** across the 36 registered `SlashCmdList` indices (0886 added TARGET's `/target`
    // `/tar` and ASSIST's `/assist` `/a` to 0881's 55; 0890 added FOLLOW's `/f` `/follow` `/fol`;
    // 0983 added CAST's `/cast` `/spell`, MACRO's `/macro` `/m`, and MACROHELP's `/macrohelp`;
    // 1291 added CONSOLE's `/console` — one distinct alias, SLASH_CONSOLE1 and 2 are both the
    // same string).
    //
    // The third number is benilla's own player-facing additions: `/reload` (1291), `/errors`
    // `/err` (1495, the script error log) and `/convertraid` (the raid conversion trigger the
    // unbuilt RaidFrame tab would otherwise carry) — 4 aliases over 3 commands. Present in
    // every build, deliberately counted apart from the shipped surface so the seam stays visible.
    // The error log is player-facing on purpose and NOT an instrument: gating it on
    // `dev_affordances()` would leave exactly the reporters who asked for it unable to type it.
    //
    // The fourth is the instrument **seam** (decision 1179): benilla's own instrument commands
    // (`/castvis` `/chattest` `/partytest` `/shot` `/liquid` `/reaction` `/react` — 7 aliases over 6
    // commands) are registered only when `run_mode::dev_affordances()`, so a player build claims
    // none of them and `/partytest` falls through to the reference's "unknown command". Asserted
    // against the predicate rather than a literal, so the row states the rule in both builds.
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

/// **The `0x4000` "requires standing still" arm** (decision 1904) — the one gate arm that does not
/// suppress silently. It reports [`EmoteGate::Moving`] and the CALLER turns that into a red
/// `ERR_NOEMOTEWHILERUNNING`, but only while the caster is self-controlled.
///
/// The mask is the reference's own `0x20ff` — [`move_flags::INTEGRATED`] — and what it leaves out
/// matters as much as what it holds: **SWIMMING is not in it**, so a swimmer standing still in the
/// water emotes fine. That negative is why this arm has nothing to do with the swim suppression
/// tested above.
#[test]
fn the_standing_still_arm_reports_moving_and_ignores_swimming() {
    use super::input::EmoteGate;
    use crate::creature_anim::move_flags;

    const STILL: u32 = 0x4000;

    // Not moving: it sends.
    assert_eq!(emote_send_eligible(STILL, 0, false, 0), EmoteGate::Send);
    // Any INTEGRATED bit trips it — a direction, a keyboard turn, or a fall.
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
    // SWIMMING is NOT in `0x20ff`: a still swimmer is still.
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
    // An unconditionally-suppressed emote never reaches this arm — it stays SILENT, which is why
    // only 20 of the 33 rows carrying `0x4000` can actually raise the line.
    assert_eq!(
        emote_send_eligible(0x4400, 0, false, move_flags::FORWARD),
        EmoteGate::Suppressed
    );
}

/// **`IsSelfControlled`'s polarity** (`0x5fa550`, decision 1904) — the half that decides whether
/// the moving refusal is heard. It is `true` for an ordinary player and `false` while confused,
/// fleeing or move-disabled, so the red line fires in the NORMAL case and a feared player emotes
/// away. STUNNED is deliberately absent from the mask.
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

// ── The send-side posture-eligibility gate (`emote_send_eligible`) — the director-verified rows
// against `CheckEmoteEligible 0x47db40`, real `Emotes.dbc` `EmoteFlags` values.
const BOW: u32 = 0x4801;
const RUDE: u32 = 0x0001;
const APPLAUD: u32 = 0x0000;
const CHEER: u32 = 0x0800;
const SALUTE: u32 = 0x0800;
const LAUGH: u32 = 0x0980;

#[test]
fn seated_stand_required_emotes_are_suppressed() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(emote_send_eligible(BOW, 1, false, 0), EmoteGate::Suppressed); // 0x4801 has 0x1 (requires STAND)
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
    assert_eq!(emote_send_eligible(0, 3, false, 0), EmoteGate::Suppressed); // SLEEP without the allow bit
    assert_eq!(emote_send_eligible(0, 7, false, 0), EmoteGate::Suppressed); // DEAD without the allow bit
    assert_eq!(emote_send_eligible(0x0200, 3, false, 0), EmoteGate::Send); // "allowed while asleep/dead"
}

// ── The open-the-box law, shared by the ENTER key and an addon's ChatFrame_OpenChat ──────────

/// **The inbound addon split, and the direction a reimplementation gets backwards.**
///
/// `CHAT_MSG_ADDON` (event 227) carries `(prefix, message, distribution, sender)`. The text divides
/// on its **FIRST** tab (`0x49a8d0`) — and with **no tab at all the whole text is the PREFIX** with
/// an empty message, not the reverse. That direction is the counter-intuitive one; this test is
/// where it is pinned.
///
/// `distribution` is the remap at `0x49aff4`: only the four lanes have names, and anything else
/// reports `"UNKNOWN"` rather than being dropped — the reference hands the addon a string it can
/// branch on either way.
#[test]
fn an_inbound_addon_line_splits_on_the_first_tab_only() {
    let _data = benilla_formats::wow_data_or_skip!();
    // **Imported, not hand-copied.** These read `0x03`/`0x04`/`0x18` for RAID/GUILD/BATTLEGROUND,
    // which are all wrong — and because the test carried the SAME wrong bytes as the code under
    // test, it agreed with the defect instead of catching it. A test that restates the value it is
    // checking cannot fail on that value; taking it from the protocol crate is what makes it a
    // check rather than an echo.
    use benilla_protocol::messages as m;
    let party = m::CHAT_TYPE_PARTY as u8;
    let raid = m::CHAT_TYPE_RAID as u8;
    let guild = m::CHAT_TYPE_GUILD as u8;
    let battleground = m::CHAT_TYPE_BATTLEGROUND as u8;
    let say = m::CHAT_TYPE_SAY as u8;
    #[allow(non_snake_case)]
    let (PARTY, RAID, GUILD, BATTLEGROUND, SAY) = (party, raid, guild, battleground, say);

    let mut log = super::feed::ChatLog::default();
    // The ordinary shape.
    log.push_addon("oRA\tSYNC:1", PARTY, 7);
    // A message that itself contains tabs: only the FIRST one divides.
    log.push_addon("CTRA\tA\tB\tC", RAID, 7);
    // NO TAB — the whole text is the prefix, the message is empty.
    log.push_addon("BareTag", GUILD, 7);
    // An empty message after a trailing tab is still an empty message, not a missing one.
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

/// **`CHAT_MSG_ADDON` reaches Lua with the reference's four arguments, in the reference's order.**
///
/// The split test above covers the parse; this covers the FIRE, which is the half that can be
/// silently wrong — an addon reading `arg3` as the sender instead of the distribution gets a string
/// either way and misbehaves without erroring.
///
/// The reference fires `SignalEvent2(227, "%s%s%s%s", prefix, message, distribution, sender)`
/// (`0x49a95f`); `BigWigs` self-delivers the identical order by hand. The handler below records all
/// four positionally, so a reordering fails on the values rather than on a count.
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

/// **The two halves of the addon lane, against each other.**
///
/// Send (1235/1236) and receive (7bd5567f) landed in different sessions, and the agent that built
/// the send half flagged the gap honestly: they pass together but *"I have not independently
/// exercised the two together."* A two-account live loopback is still the only thing that proves
/// the round trip on the wire — this proves the halves agree with each OTHER, which is the part
/// that can drift without either side looking wrong on its own.
///
/// The composition and the split are separate transcriptions of the same byte law (`0x49f9b3`
/// composes on a tab, `0x49a8d0` splits on the first one), written by different sessions from the
/// same note. If one had picked a different separator, or split last-tab instead of first, every
/// test on both sides would still pass.
///
/// A message CONTAINING tabs is the case that discriminates: compose glues one tab, the split takes
/// only the first, so the payload must come back with its own tabs intact.
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

    // Now the wire turns around: the same text arrives as an ordinary PARTY line carrying
    // LANG_ADDON, and the receive half parses it.
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

/// **The `[Language]` header keys off the frame's DEFAULT tongue, not off "Common"** (B262).
///
/// `ChatFrame.lua`'s test is `strlen(arg3) > 0 and arg3 ~= "Universal" and arg3 ~= this.defaultLanguage`,
/// and `GetDefaultLanguage()` answers the **faction** language — Common for every Alliance race,
/// Orcish for every Horde one (`0x5ec890`, and benilla's own
/// `ChrRaces` field-8 join). The composer hardcoded `"Common"`, which is right for half the game
/// and exactly backwards for the other half: a Horde character saw `[Orcish]` on every ordinary
/// line of their own faction's chat, and no tag at all on the Common they cannot read.
///
/// The condition is about the default language, never about whether the listener understands it —
/// so a fully-understood foreign line still carries its tag.
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

    // A Horde body (default Orcish): exactly the mirror. This is the assertion that fails against
    // the hardcoded "Common".
    assert_eq!(
        compose(&orcish, K::Say, "Orcish").unwrap(),
        "|Hplayer:Grom|h[Grom]|h says: lok'tar"
    );
    assert_eq!(
        compose(&common, K::Say, "Orcish").unwrap(),
        "|Hplayer:Ann|h[Ann]|h says: [Common] hello"
    );

    // Language 0 arrives as an empty arg3 and is never tagged, whatever the default is — which is
    // also how a GM and the narration chat types come through, since all three force the field to 0.
    let universal = ev(K::Say, "system", "Ann");
    assert_eq!(
        compose(&universal, K::Say, "Orcish").unwrap(),
        "|Hplayer:Ann|h[Ann]|h says: system"
    );

    // A language the listener fully understands still carries its tag: the test is about the
    // default tongue, not about comprehension. A dwarf reading Dwarvish sees the header.
    let dwarvish = ChatEvent {
        language: "Dwarvish".into(),
        ..ev(K::Say, "here we go", "Bran")
    };
    assert_eq!(
        compose(&dwarvish, K::Say, "Common").unwrap(),
        "|Hplayer:Bran|h[Bran]|h says: [Dwarvish] here we go"
    );
}

/// **The talk/laugh gesture reads the PLAINTEXT, not the garbled line** — so it is
/// language-independent, and a Horde player yelling `lol` laughs for every observer, Alliance
/// included.
///
/// This corrects an inference we had already wired: the consumer census of the display path
/// `0x49a870` found the chat line, the Lua `arg1` and the bubble all sharing the rewritten buffer,
/// and we concluded the gesture did too. It does not — the selector is not on that path at all. It
/// lives in the **parser** `0x49d560` at `0x49d820`-`0x49d8ae`, matching against `[ebp-0x10]`, which
/// is the very buffer `0x49dbc2` then hands to `0x49a870` as its `src`. The garbled buffer is a
/// local of a frame that does not exist yet, so the census could never have found this consumer.
///
/// The two inputs are observably different, which is the whole point of the test: feed the garbled
/// text here and the laugh silently becomes a plain talk.
#[test]
fn the_talk_gesture_reads_the_plaintext_not_the_garbled_line() {
    let _data = benilla_formats::wow_data_or_skip!();
    use crate::creature_anim::{select_gesture, Gesture};
    use benilla_protocol::messages::CHAT_MSG_SAY;

    let Some(data) = benilla_formats::wow_data() else {
        return; // no client data — the same skip every data-gated test here takes
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

// ── B297: the combat log reaches addons ──────────────────────────────────────────────────────

/// **The bug, stated as a test.** B297 is "benilla emits no combat-log chat events at all", and the
/// consumer named in the report is Quiver's TranqAnnouncer, whose *only* detector is
/// `CHAT_MSG_SPELL_SELF_DAMAGE`. So the test is an addon registering exactly that event and
/// receiving exactly that sentence.
///
/// `arg1` carrying the whole formatted line is the load-bearing half: every 1.12 damage meter and
/// announcer parses `arg1` with a Lua pattern built from its own copy of the GlobalStrings, so a
/// fire with an empty or differently-shaped arg1 would pass a "does it fire" check and still be
/// useless. This asserts the text.
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

/// **Every combat-log kind reaches an addon that registers it**, not just the one B297 named.
///
/// [`an_addon_sees_the_combat_log_line_it_registers_for`] pins the shape on `SPELL_SELF_DAMAGE`;
/// this sweeps the whole block, which is what makes 1703's eleven new types a *fact* rather than a
/// hope — a kind whose name is misspelled in `event_name`, or that the router drops, fires nothing
/// and would otherwise be found by a player's damage meter months later.
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

/// The combat block is **verbatim**: the composer adds `arg1` and nothing else — no `[Name]` link,
/// no `<AFK>` prefix, no language header. The reference says so by prefix
/// (`ChatFrame_OnEvent` l.1397/1399, two arms that only `AddMessage(arg1, …)`), and the failure
/// this guards against is the player/monster branch's decorations leaking onto a combat line.
#[test]
fn a_combat_log_line_renders_verbatim() {
    let _data = benilla_formats::wow_data_or_skip!();
    let default_language = String::from("Common");
    for kind in K::ALL.iter().copied().filter(|k| k.is_combat_log()) {
        let mut e = ChatEvent::text_only(kind, "You hit Kobold Vermin for 5.".into());
        // Deliberately populated: a combat line never carries these, and if the composer ever fell
        // through to the player branch it would splice them in.
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

/// **Both dock tabs exist, and clicking one selects its window.**
///
/// This is the test that was missing when 1571 shipped the combat log's chat lines: the lines
/// routed correctly into ChatFrame2 and no player could ever see them, because `ChatFrame2Tab` had
/// never been authored. Every piece of machinery around it *did* exist and was written for two
/// tabs — `BenillaFCF`'s fade/resize/flash loops all run `for i = 1, 2` — but each carries an
/// `if tab then` guard, so the absence was swallowed silently for as long as it existed. The
/// director found it by looking at the screen.
///
/// So the assertion is deliberately about the tab BUTTON and the selection it drives, not about
/// routing (which `the_combat_log_lands_in_window_two_only` already covers and which was never
/// the broken half).
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

    // Clicking the Combat Log tab swaps them — the path a player takes to read the combat log.
    s.run("FCF_SelectDockFrame(ChatFrame2)").unwrap();
    assert_eq!(
        s.eval::<i64>("return SELECTED_DOCK_FRAME:GetID()").unwrap(),
        2
    );
    assert!(s.eval::<bool>("return ChatFrame2:IsShown()").unwrap());
    assert!(!s.eval::<bool>("return ChatFrame1:IsShown()").unwrap());
    assert!(s.errors().is_empty(), "handler errors: {:?}", s.errors());
}

/// The two dock tabs are labelled from the install's own `GlobalStrings.lua` (`GENERAL`,
/// `COMBAT_LOG`) rather than from words quoted into our XML — the loader's ALL-CAPS key rule, and
/// the same shape the combat log's format strings use (1571). Skips without client data.
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

/// **The Combat Log window has a real rect, and it is the dock's.**
///
/// This is the test whose absence let 1575 ship a tab onto a window that rendered nothing:
/// `ChatFrame2` carries no `<Size>` and derives its whole rect from two anchors onto ChatFrame1,
/// and every check we had asked only whether lines *routed* into it. They did — 21 of them — into a
/// frame measuring 0×0, so `GetNumMessages()` was 21 and the screen was empty.
///
/// Asserting the rect (not the size attribute — `GetWidth` reports the explicit field, which is 0
/// here by design and told us nothing) is what makes "the window exists" mean "the window has
/// pixels".
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
    // **Three points, and which three is the assertion.** The XML authors two (TOPLEFT and
    // BOTTOMRIGHT onto ChatFrame1) and `FCF_DockUpdate` then clears them and applies the
    // reference's own three for a non-first dock member — TOPLEFT / BOTTOMLEFT / BOTTOMRIGHT,
    // FloatingChatFrame.lua l.1059-1063. So `3` here means the dock seeding ran and this window is
    // in `DOCKED_CHAT_FRAMES`; `2` would mean it never did and the rect is standing on the XML
    // alone. Either way one alone cannot size the frame, which is what 1575 shipped.
    assert_eq!(
        s.eval::<i64>("return ChatFrame2:GetNumPoints()").unwrap(),
        3,
        "FCF_DockUpdate's three points — a 2 means the dock never seeded this window"
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

/// Both dock windows carry the SAME chrome. `BenillaFCF_Textures(id)` looks each piece up by
/// `ChatFrame<id><suffix>`, which is the reference's own per-frame `CHAT_FRAME_TEXTURES` shape —
/// and ChatFrame2 had only a background, no borders. Selecting it therefore hid ChatFrame1's
/// chrome (its textures are ChatFrame1's children) and put nothing in its place: the background
/// and border vanished, which is what the director saw.
///
/// The ring is the eight **resize grips** since the move/resize arc, so the same check now covers
/// both halves of each piece: the texture the fade paints and the button that grabs it.
#[test]
fn both_dock_windows_carry_the_same_chrome() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = chat_vm();
    // The reference's own `CHAT_FRAME_TEXTURES`, which is also the list the file declares — read
    // out of the VM rather than restated here, so a piece that leaves the list cannot leave this
    // check with it.
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
    // The grips themselves, not only their art: each texture's owning Button has to exist too, or
    // the ring is painted and nothing can grab it.
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

/// **A docked chat window survives the managed-position pass.**
///
/// The pass owns a frame's whole seat — it `ClearAllPoints()` first, by design (decision 1499) —
/// and the reference's own table carries a `ChatFrame2` row. That row cost B297 a visible fix
/// while the pass was ours, and our answer then was to drop the row from our copy. The stock pass
/// is the one that runs now (1988), row and all, and it ends by calling `FCF_DockUpdate()` —
/// which re-anchors every docked window onto `DEFAULT_CHAT_FRAME` in the same breath. That is the
/// reference's answer to its own row, and this is the behavioural check that it holds: after the
/// pass, the docked window is still exactly on the dock.
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

/// **What does NOT gate a `/sit` underwater** — the negative that sends B155's refusal to the
/// stand-state setter instead of here (`player::state::stand_state_refused`).
///
/// [`emote_send_eligible`] carries the client's own swim suppression (`EmoteFlags & 0x0080` at
/// `0x47db7d`), so it is the obvious suspect for "the reference won't let me `/sit` in water" — and
/// it is the wrong one. Read off the shipped `Emotes.dbc`: every posture emote (`/sit`, `/sleep`,
/// `/kneel`, `/stand`) carries `0x6202`, and `0x0080` is clear in it, so the emote layer passes them
/// straight through while swimming. This asserts that on the real data, because it is the fact that
/// decides *where* the fix belongs: if a later data read made these rows carry `0x0080`, the two
/// gates would double up and this test is what says so. Skips without client data.
#[test]
fn the_posture_emotes_carry_no_swim_suppression_flag() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = benilla_formats::load_emote_sound_catalog(&mut chain).expect("emote catalog");
    // Every posture row in the shipped table, found by scanning rather than by hardcoded id.
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
        // STATE_DEAD (7, emote 65, `0x6602`) is the one posture row the client kills outright —
        // `0x0400`, unconditional suppress, in water or out. It is not reachable from the slash
        // grammar and it is not what B155 is about; every posture a player can actually ask for
        // passes the emote gate mid-swim, which is the point.
        if flags & 0x0400 != 0 {
            assert_eq!(state, 7, "only STATE_DEAD is unconditionally suppressed");
            continue;
        }
        // …so the emote gate lets it through mid-swim. (`stand_state` here is the performer's
        // CURRENT state; a standing swimmer pressing `/sit` is the reported case.)
        assert_eq!(
            super::input::emote_send_eligible(flags, 0, true, 0),
            super::input::EmoteGate::Send,
            "posture emote {id} passes the emote gate while swimming"
        );
    }
}

/// **The ding's gains reach `PLAYER_LEVEL_UP` matched BY LEVEL, and a miss is zeros, not absence.**
///
/// The net layer has no `UiScript`, so `SMSG_LEVELUP_INFO`'s tuple is parked on `ChatLog` and the
/// feed that owns the level edge picks it up (decision 1884). Matching on the level is what keeps
/// a stale entry from attaching to a later ding — the trigger is a descriptor diff and the gains
/// are packet-borne, so the two are only coincidentally in step.
///
/// The miss case is the load-bearing half: a GM demotion writes the descriptor with no packet, and
/// the event must still carry nine arguments. `ChatFrame.lua` guards every one with
/// `if ( argN > 0 )`, so a zero reads as "no line" while a nil raises.
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

/// **The ding block is printed once, by the reference's own window.**
///
/// `benilla.toc` sources `Interface\FrameXML\ChatFrame.xml` off the player's chain, and stock
/// `ChatFrame_OnEvent` composes the whole five-line level-up block itself from `PLAYER_LEVEL_UP`
/// (`ChatFrame.lua` l.1283-1324) — `LEVEL_UP`, the health/mana pair, `LEVEL_UP_CHAR_POINTS`, and a
/// `LEVEL_UP_STAT` per positive gain. benilla fires that event with the reference's nine arguments
/// (decision 1884), and for a while it *also* routed its own Rust copy of the same five lines,
/// under a comment saying they would stay "until that window migrates". The window migrated; the
/// copy did not go. Every ding printed twice, and nothing could see it: both halves were correct
/// on their own, and the composer's own tests only ever checked the text it produced.
///
/// So this counts what lands in the real window. The event is the whole of the ding now — which
/// also means the count below is the reference's own composition, not ours.
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
    // The whole of what the packet's apply does now: park the gains, print nothing.
    log.push_level_up_gains(&info, 1);
    let before = lines_in_window(&s);
    assert_eq!(
        lines_in_window(&s) - before,
        0,
        "the app composes no ding line of its own"
    );

    // Tap the window's own `AddMessage` so the assertion can be about the BLOCK and not just its
    // length — this is where the deleted Rust composer's test went. The subject moved to the
    // reference's Lua; the knowledge did not.
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

    // `ChatFrame.lua` l.1283-1324's exact order and forms, off the shipped GlobalStrings: the
    // singular `LEVEL_UP_CHAR_POINTS` at one point (`GetText`'s plural pick), and one
    // `LEVEL_UP_STAT` for the single positive gain, named through `SPELL_STAT0_NAME`.
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

/// **The free-professions line, same question as the ding.** Stock `ChatFrame_OnEvent` handles
/// `CHARACTER_POINTS_CHANGED` too (`ChatFrame.lua` l.1324-1334): on `arg2 > 0` it reads
/// `UnitCharacterPoints("player")` and prints `GetText("LEVEL_UP_SKILL_POINTS", nil, cp2)`.
/// `ui_talent` fires that event *and* composes the same line. This asks the window which of them
/// lands.
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

// ───────────── The chat cache restores INSIDE the login, not after it (decision 2119) ─────────

/// **The login order, asserted at the two places that broke.**
///
/// `UPDATE_CHAT_WINDOWS` is the only thing that registers a chat frame for any `CHAT_MSG_*`
/// (`ChatFrame_OnEvent`'s arm calls `ChatFrame_RegisterForMessages(GetChatWindowMessages(id))`),
/// and the `UPDATE_CHAT_COLOR` burst mirrors `WHISPER` into `ChatTypeInfo["REPLY"]`, whose `.id`
/// is 0 — the same id every `AddMessage` with no explicit colour carries — so the burst repaints
/// them. Both events come from the chat-cache restore, so the restore has to be finished before
/// `PLAYER_LOGIN`: before it, an addon's `Print` gets repainted whisper-pink, and any chat routed
/// in that window lands on a frame registered for nothing and is dropped in silence (1784).
///
/// Pre-2119 the restore was an `Update` system and this probe saw `windows = nil`,
/// `colors = nil`, `registered = ""` at `PLAYER_LOGIN`.
///
/// The probe is planted as a real loose ADDON, the way `world_entry_tests` plants its own. It
/// cannot be a frame created on the boot VM beforehand: since 2226 the entry load BUILDS the VM it
/// runs on, so anything seated on the character screen's VM is gone before the first event fires.
/// An addon's file scope runs inside the load — after the XML, before `VARIABLES_LOADED` — which
/// is exactly the vantage point this probe wants, and the one the reference gives an addon too.
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
    // The reference's own login order (decision 2125): addons and their `ADDON_LOADED`
    // (`0x4900a3`), then `VARIABLES_LOADED`
    // (`0x4900b2`), then the chat-cache reader's burst (`0x4900d6`), then `PLAYER_LOGIN`
    // (`0x490959`). 2119 put the burst ahead of `VARIABLES_LOADED`, one step too early.
    assert_eq!(
        read("ChatOrderProbe.order"),
        "VARIABLES_LOADED UPDATE_CHAT_WINDOWS UPDATE_CHAT_COLOR PLAYER_LOGIN ",
        "the reference fires the chat-cache burst BETWEEN VARIABLES_LOADED and PLAYER_LOGIN"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}
