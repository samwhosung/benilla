//! Shipped end-to-end tests for the chat window + input (the chat arc): the real
//! `Interface\FrameXML\ChatFrame.xml` loaded through the engine loader, driven exactly as the app
//! drives it — `add_chat_message` → `resolve`/`extract` (lines render in the pinned colors), the
//! wheel → the Lua `OnMouseWheel` handler → `ScrollUp` (which freezes the fade), and the input
//! EditBox's ENTER → `OnEnterPressed` → `SubmitChatInput` → `take_chat_input` (the app then parses
//! it — see `crate::ui_chat`'s `parse_line` tests for the `/say`//`/yell`//`/w` mapping).

use benilla_ui::script::{ExtractedQuad, QuadContent, UiScript};

use super::test_ui::load_ui as load_xml;

/// The full ChatFrame stack (fonts first, so the FontString's `inherits="ChatFontNormal"` resolves).
fn chat_frame() -> UiScript {
    let mut s = UiScript::new().unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // GameTooltip.xml + UIDropDownMenu.xml are real RUNTIME dependencies of the chat tabs since
    // decision 1589: a left click closes any open menu (`CloseDropDownMenus`, the reference's own
    // first move in `FCF_Tab_OnClick`) and a right click opens the window's options menu.
    // `benilla.toc` already orders both ahead of ChatFrame.xml (l.60/64 vs l.399); the harness
    // says so too, rather than a guard that would hide a real ordering fault. (The tooltip file is
    // the dropdown kit's own dependency — its MenuBackdrop reads `TOOLTIP_DEFAULT_COLOR`.)
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\UIMenu.xml"); // the kit the chat menus build from
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    // The UIParent slice (RaiseFrameLevel, MouseIsOver, the fade and flash kits) ahead of the
    // bar and the chat files; `FCF_OnUpdate` — the dock's hover, fade and drag driver — runs
    // from UIParent's OnUpdate, as in the reference.
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    // `FCF_ValidateChatFramePosition` (a tab-drag stop) reads `MainMenuBar:GetHeight()`; the bar's
    // own load-time chain precedes it, as in the action-bar harness.
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(&s, "Interface\\FrameXML\\MainMenuBar.xml");
    load_xml(&s, "Interface\\FrameXML\\ChatFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\FloatingChatFrame.xml");
    super::fire_chat_login(&mut s);
    s.set_screen_size(1600.0, 900.0);
    s.resolve();
    s
}

/// The color+alpha of the first Text quad whose text equals `t`.
fn text_color(quads: &[ExtractedQuad], t: &str) -> Option<[f32; 4]> {
    quads.iter().find_map(|q| match &q.content {
        QuadContent::Text {
            text: Some(x),
            color: Some(c),
            ..
        } if x == t => Some(*c),
        _ => None,
    })
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn injected_lines_render_in_the_pinned_colors() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_frame();
    // The app's feed passes 0..1 floats from the pinned type→color table; the seam quantizes them.
    // SAY white (FFFFFF), SYSTEM yellow (FFFF00 — the GM-feedback color), LOOT green (00AA00).
    s.add_chat_message("ChatFrame1", "[Tri] says: hi", 1.0, 1.0, 1.0);
    s.add_chat_message("ChatFrame1", "You give 500 copper.", 1.0, 1.0, 0.0);
    s.add_chat_message(
        "ChatFrame1",
        "You receive loot: [Tough Jerky].",
        0.0,
        170.0 / 255.0,
        0.0,
    );
    s.resolve();
    let quads = s.extract();

    let say = text_color(&quads, "[Tri] says: hi").expect("say line rendered");
    assert!(
        close(say[0], 1.0) && close(say[1], 1.0) && close(say[2], 1.0),
        "say white: {say:?}"
    );
    assert!(close(say[3], 1.0), "a fresh line is fully opaque");

    let sys = text_color(&quads, "You give 500 copper.").expect("system line rendered");
    assert!(
        close(sys[0], 1.0) && close(sys[1], 1.0) && close(sys[2], 0.0),
        "system yellow: {sys:?}"
    );

    let loot = text_color(&quads, "You receive loot: [Tough Jerky].").expect("loot line rendered");
    assert!(
        close(loot[0], 0.0) && close(loot[1], 170.0 / 255.0) && close(loot[2], 0.0),
        "loot green: {loot:?}"
    );
}

#[test]
fn newest_line_sits_at_the_bottom() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_frame();
    s.add_chat_message("ChatFrame1", "older", 1.0, 1.0, 1.0);
    s.add_chat_message("ChatFrame1", "newer", 1.0, 1.0, 1.0);
    s.resolve();
    let quads = s.extract();
    let y = |t: &str| {
        quads
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Text { text: Some(x), .. } if x == t => q.rect.map(|r| r.bottom),
                _ => None,
            })
            .unwrap()
    };
    // y-up: the newest line's band is lower than the older one's.
    assert!(y("newer") < y("older"), "newest renders at the bottom");
}

/// The whole fade round trip as it reaches a real chat window: a line ramps down, a scroll brings
/// it back to full, the scrolled-up view then holds it there, and returning to the bottom lets it
/// ramp again. The re-arm half: every scroll entry reaches
/// `0x788b80` or the relayout's `0x788af0`; before it, a faded-out chat could not be recovered by
/// any input the client offers (director-reported, 2026-08-29).
#[test]
fn wheel_scroll_re_arms_the_fade_then_freezes_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_frame();
    s.run("ChatFrame1:SetTimeVisible(0); ChatFrame1:SetFadeDuration(4)")
        .unwrap();
    for t in ["L0", "L1", "L2"] {
        s.add_chat_message("ChatFrame1", t, 1.0, 1.0, 1.0);
    }
    s.resolve();
    s.tick(1.0);
    s.resolve();
    let a1 = text_color(&s.extract(), "L1").expect("L1 visible")[3];
    assert!(a1 < 1.0 && a1 > 0.0, "the line faded partway: {a1}");
    // The scroll verb is what the reference's scroll buttons call; the 1.12 chat frame takes
    // no wheel of its own (`enableMouse="false"`, no OnMouseWheel).
    s.run("ChatFrame1:ScrollUp()").unwrap();
    s.resolve();
    let a2 = text_color(&s.extract(), "L1").expect("L1 still visible")[3];
    assert!(close(a2, 1.0), "the scroll brought the line back: {a2}");
    s.tick(2.0);
    s.resolve();
    let a3 = text_color(&s.extract(), "L1").expect("L1 still visible")[3];
    assert!(close(a3, 1.0), "frozen while scrolled up: {a3}");
    s.run("ChatFrame1:ScrollToBottom()").unwrap();
    s.tick(1.0);
    s.resolve();
    let a4 = text_color(&s.extract(), "L1").expect("L1 visible")[3];
    assert!(a4 < 1.0 && a4 > 0.0, "the fade resumed at the bottom: {a4}");
    assert!(close(a4, a1), "and from a full re-arm: {a4} vs {a1}");
}

#[test]
fn input_editbox_enter_drains_the_typed_line() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_frame();
    assert!(s.focus_editbox("ChatFrameEditBox"), "the edit box focuses");
    assert!(s.has_keyboard_focus(), "focus gates the world's keys");
    s.char_input("/yell hi");
    assert!(s.key_input("ENTER"), "the box consumes ENTER");
    // The reference's own path: `ChatEdit_OnEnterPressed` → `ChatEdit_SendText` →
    // `ChatEdit_ParseText` → `SendChatMessage("hi", "YELL", …)`.
    let sends = s.take_chat_sends();
    assert_eq!(
        sends
            .iter()
            .map(|c| (c.text.as_str(), c.chat_type.as_str()))
            .collect::<Vec<_>>(),
        vec![("hi", "YELL")]
    );
    assert!(
        !s.has_keyboard_focus(),
        "submit closes the box (ChatEdit_OnEscapePressed: ClearFocus + Hide)"
    );
    assert!(s.take_chat_sends().is_empty(), "drained");
}

#[test]
fn input_escape_closes_without_submitting() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_frame();
    assert!(s.focus_editbox("ChatFrameEditBox"));
    s.char_input("hello");
    assert!(s.key_input("ESCAPE"), "the box consumes ESCAPE");
    assert!(s.take_chat_input().is_empty(), "escape submits nothing");
    assert!(!s.has_keyboard_focus(), "escape closes the box");
}

/// The shipped chat box takes plain arrows and history recall — end-to-end over the real XML.
/// Guards the exact in-game gap the director hit: the ref template's `ignoreArrows="true"` (which
/// benilla deliberately drops — plain arrows edit; the divergence note in ChatFrame.xml) would
/// leave LEFT/RIGHT consumed-but-dead here, and an unpushed history would leave Up/Down empty.
#[test]
fn chat_box_arrows_edit_and_history_recalls() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::{EditAction, EditUnit};
    let mut s = chat_frame();
    assert!(s.focus_editbox("ChatFrameEditBox"));
    s.char_input("ab");
    // The reference's ChatFrameEditBoxTemplate is `ignoreArrows="true"` — the engine's
    // AltArrowKeyMode, which the app's key routing reads to hand a plain arrow to the bindings
    // (the character turns while you type) rather than to the box. The box itself still edits.
    assert!(
        s.editbox_alt_arrow_mode(),
        "the stock template's ignoreArrows landed as AltArrowKeyMode on the focused box"
    );
    s.editbox_action(EditAction::Delete {
        unit: EditUnit::Char,
        back: true,
    });
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        "a"
    );
    s.run("ChatFrameEditBox:SetText('')").unwrap();
    s.char_input("/yell hi");
    assert!(s.key_input("ENTER"));
    // `ChatEdit_SendText` → `ChatEdit_ParseText` → the YELL type's send, and the reference's
    // `AddHistoryLine` on the way.
    let sends = s.take_chat_sends();
    assert_eq!(
        sends
            .iter()
            .map(|c| (c.text.as_str(), c.chat_type.as_str()))
            .collect::<Vec<_>>(),
        vec![("hi", "YELL")]
    );
    assert!(s.focus_editbox("ChatFrameEditBox"));
    s.char_input("dra");
    s.editbox_action(EditAction::HistoryPrev);
    // `ChatEdit_AddHistory` filed `SLASH_YELL1 .. " " .. text` = "/y hi" (ChatFrame.lua
    // l.1913-1937); the recall lands it in the box, and the box's own `OnTextChanged` live parse
    // (`ChatEdit_ParseText(this, 0)`) turns it straight into the YELL type with "hi" — which is
    // what the player sees: a "Yell:" header and the text.
    assert!(
        s.eval::<bool>(
            "return ChatFrameEditBox.chatType == 'YELL' and ChatFrameEditBox:GetText() == 'hi'"
        )
        .unwrap(),
        "Up recalls the filed line, live-parsed: {:?} {:?}",
        s.eval::<String>("return ChatFrameEditBox.chatType")
            .unwrap(),
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap()
    );
    s.editbox_action(EditAction::HistoryNext);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        "dra",
        "Down past the newest restores the in-progress draft"
    );
}

/// Decision 0843 — the chat body as a dismiss surface, the director's stuck-spell gesture end to
/// end through the shipped XML: a spell dragged out of the spellbook and RELEASED over the chat
/// keeps carrying (a drag release is never a click — 0218's byte-verified trigger), the follow-up
/// completed LEFT CLICK on the chat body dismisses it, and the same click leaves an ITEM payload
/// untouched (a silent item dismissal would be a destroy — only the world-drop popup offers that).
#[test]
fn chat_click_dismisses_a_stuck_spell_but_not_an_item() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::{
        ContainerSlot, ContainerState, SpellBookState, SpellSlotView, SpellTabView,
    };
    // The reference's spellbook wants the action-bar chain beneath it (1952); the chat window
    // loads after it, as in the manifest.
    let mut s = super::spellbook_tests::spellbook_ui(1024.0, 768.0);
    for f in [
        "Interface\\FrameXML\\UIMenu.xml", // the kit ChatMenu/EmoteMenu/VoiceMacroMenu build from
        "Interface\\FrameXML\\ChatFrame.xml",
        "Interface\\FrameXML\\FloatingChatFrame.xml",
    ] {
        load_xml(&s, f);
    }
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.set_spellbook(SpellBookState {
        tabs: vec![SpellTabView {
            name: "Fire".into(),
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            offset: 0,
            num_spells: 1,
        }],
        slots: vec![SpellSlotView {
            spell_id: 133,
            name: "Fireball".into(),
            rank: Some("Rank 1".into()),
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            passive: false,
            current: false,
            cooldown: None,
            ..Default::default()
        }],
    });
    s.run("ToggleSpellBook(BOOKTYPE_SPELL)").unwrap();
    s.resolve();

    // Drag Fireball off its book button (press → past the 4px threshold → the payload is up).
    let (l, r, t, b) = (
        s.eval::<f32>("return SpellButton1:GetLeft()").unwrap(),
        s.eval::<f32>("return SpellButton1:GetRight()").unwrap(),
        s.eval::<f32>("return SpellButton1:GetTop()").unwrap(),
        s.eval::<f32>("return SpellButton1:GetBottom()").unwrap(),
    );
    let (x1, y1) = ((l + r) * 0.5, (t + b) * 0.5);
    s.mouse_button(x1, y1, "LeftButton", true);
    s.mouse_move(x1 + 20.0, y1);
    assert!(
        s.cursor_payload().is_some(),
        "OnDragStart picked the spell up"
    );

    // Release the DRAG over the chat body: keeps carrying (OnClick never fires on a drag).
    let (cx, cy) = (200.0, 150.0); // inside ChatFrame1 (BOTTOMLEFT 32,85 + 430×120)
    s.mouse_move(cx, cy);
    s.mouse_button(cx, cy, "LeftButton", false);
    assert!(
        s.cursor_payload().is_some(),
        "a drag release over the chat keeps carrying"
    );

    // The completed click on the chat body dismisses the stuck spell.
    s.mouse_button(cx, cy, "LeftButton", true);
    s.mouse_button(cx, cy, "LeftButton", false);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert!(
        s.cursor_payload().is_none(),
        "a chat click dismisses a spell payload"
    );

    // An ITEM payload survives the same click untouched.
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            count: 5,
            quality: Some(1),
            item_id: 117,
            link: None,
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );
    s.run("PickupContainerItem(0, 1)").unwrap();
    assert!(s.cursor_item().is_some(), "fixture: the item is held");
    s.mouse_button(cx, cy, "LeftButton", true);
    s.mouse_button(cx, cy, "LeftButton", false);
    assert!(
        s.cursor_item().is_some(),
        "a chat click never touches an item payload"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

// ── ChatTypeInfo: the addon-facing color table ────────────────────────────────────────────────

/// `ChatTypeInfo` carries the shipped default chat colors twice: once in
/// `Interface\FrameXML\ChatFrame.xml` for addons to read, and once in
/// [`crate::ui_chat::default_color`] for our own feed to render. Both are the same reference byte
/// table (the static registry at `.rdata 0x804710`) — so this is the gate
/// that makes the duplication safe: every kind we model must agree to the byte, and the table's shape (`sticky`/`id`) must be the reference's.
#[test]
fn chat_type_info_matches_the_host_color_table() {
    benilla_formats::wow_data_or_skip!();
    use crate::ui_chat::{default_color, ChatEventKind as K};

    /// Each modeled kind and its `ChatTypeInfo` key — the reference's own spellings.
    const PAIRS: &[(&str, K, i64)] = &[
        ("SAY", K::Say, 1),
        ("PARTY", K::Party, 2),
        ("RAID", K::Raid, 3),
        ("GUILD", K::Guild, 4),
        ("OFFICER", K::Officer, 5),
        ("YELL", K::Yell, 6),
        ("WHISPER", K::Whisper, 7),
        ("WHISPER_INFORM", K::WhisperInform, 8),
        ("EMOTE", K::Emote, 9),
        ("TEXT_EMOTE", K::TextEmote, 10),
        ("SYSTEM", K::System, 11),
        ("MONSTER_SAY", K::MonsterSay, 12),
        ("MONSTER_YELL", K::MonsterYell, 13),
        ("MONSTER_EMOTE", K::MonsterEmote, 14),
        ("MONSTER_WHISPER", K::MonsterWhisper, 27),
        ("CHANNEL", K::Channel, 15),
        ("CHANNEL_JOIN", K::ChannelJoin, 16),
        ("CHANNEL_LEAVE", K::ChannelLeave, 17),
        ("CHANNEL_NOTICE", K::ChannelNotice, 19),
        ("CHANNEL_NOTICE_USER", K::ChannelNoticeUser, 20),
        ("CHANNEL_LIST", K::ChannelList, 18),
        ("AFK", K::Afk, 21),
        ("DND", K::Dnd, 22),
        ("IGNORED", K::Ignored, 23),
        ("SKILL", K::Skill, 24),
        ("LOOT", K::Loot, 25),
        ("MONEY", K::Money, 87),
        ("COMBAT_XP_GAIN", K::CombatXpGain, 46),
        ("RAID_LEADER", K::RaidLeader, 88),
        ("RAID_WARNING", K::RaidWarning, 89),
        ("RAID_BOSS_EMOTE", K::RaidBossEmote, 91),
        ("BATTLEGROUND", K::Battleground, 93),
        ("BATTLEGROUND_LEADER", K::BattlegroundLeader, 94),
        ("BG_SYSTEM_NEUTRAL", K::BgSystemNeutral, 83),
        ("BG_SYSTEM_ALLIANCE", K::BgSystemAlliance, 84),
        ("BG_SYSTEM_HORDE", K::BgSystemHorde, 85),
    ];

    let s = chat_frame();
    for (name, kind, want_id) in PAIRS {
        let (r, g, b, id): (f64, f64, f64, i64) = s
            .eval(&format!(
                r#"local i = ChatTypeInfo["{name}"] return i.r, i.g, i.b, i.id"#
            ))
            .unwrap_or_else(|e| panic!(r#"ChatTypeInfo["{name}"]: {e}"#));
        let got = [
            (r * 255.0).round() as u8,
            (g * 255.0).round() as u8,
            (b * 255.0).round() as u8,
        ];
        assert_eq!(got, default_color(*kind), r#"ChatTypeInfo["{name}"] color"#);
        // `id` is GetChatTypeIndex's 1-based registry slot, asserted EXACTLY rather than as a
        // range: four of these ship the same FFDBB7 and a range check would let a transposition
        // inside that family through with both colors still matching.
        assert_eq!(id, *want_id, r#"ChatTypeInfo["{name}"].id"#);
    }
}

/// The table's *shape*, transcribed rather than derived: the reference's 105 keys, its five sticky
/// types (the set `ui_chat::edit`'s `SendType::sticky` already quotes), the two keys FrameXML
/// declares that the engine's color registry does not contain, and the ten boot-seeded channel
/// extras. An addon reads `.sticky` and `.id` as often as it reads the color.
#[test]
fn chat_type_info_has_the_references_shape() {
    benilla_formats::wow_data_or_skip!();
    let s = chat_frame();

    let count: i64 = s
        .eval("local n = 0 for _ in pairs(ChatTypeInfo) do n = n + 1 end return n")
        .unwrap();
    assert_eq!(count, 105, "the reference declares 105 keys");

    let sticky: String = s
        .eval(
            "local t = {} for k, v in pairs(ChatTypeInfo) do if v.sticky == 1 then \
             table.insert(t, k) end end table.sort(t) return table.concat(t, \" \")",
        )
        .unwrap();
    assert_eq!(sticky, "BATTLEGROUND GUILD PARTY RAID SAY");

    // REPLY and COMBAT_ERROR are declared by FrameXML and absent from the engine's 94-entry
    // registry, so GetChatTypeIndex answers 0 for both — but their COLORS differ, which is the
    // trap this asserts. Nothing ever writes COMBAT_ERROR. REPLY is written by hand inside the
    // UPDATE_CHAT_COLOR handler, which mirrors WHISPER into it (ChatFrame.lua l.1357-1365), so
    // its end state is WHISPER's FF80FF. Seeding both white reads as symmetric and is wrong.
    for (name, want) in [
        ("REPLY", [255u8, 128, 255]),
        ("COMBAT_ERROR", [255, 255, 255]),
    ] {
        let (id, r, g, b): (i64, f64, f64, f64) = s
            .eval(&format!(
                r#"local i = ChatTypeInfo["{name}"] return i.id, i.r, i.g, i.b"#
            ))
            .unwrap();
        assert_eq!(id, 0, r#""{name}".id — not in the engine's registry"#);
        let got = [
            (r * 255.0).round() as u8,
            (g * 255.0).round() as u8,
            (b * 255.0).round() as u8,
        ];
        assert_eq!(got, want, r#""{name}" color"#);
    }

    // The extras: CHANNEL1..CHANNEL10, indices 95..104, each the live CHANNEL entry's FFC0C0.
    for n in 1..=10 {
        let (id, r, g, b): (i64, f64, f64, f64) = s
            .eval(&format!(
                r#"local i = ChatTypeInfo["CHANNEL{n}"] return i.id, i.r, i.g, i.b"#
            ))
            .unwrap();
        assert_eq!(id, 94 + n, "CHANNEL{n}.id");
        let rgb = [
            (r * 255.0).round() as u8,
            (g * 255.0).round() as u8,
            (b * 255.0).round() as u8,
        ];
        assert_eq!(rgb, [255, 192, 192], "CHANNEL{n} color");
    }
}

/// **Every event name we fire is a key the reference's own `ChatTypeInfo` carries.**
/// `ChatFrame_OnEvent` recovers the type with `strsub(event, 10)` and indexes
/// `ChatTypeInfo[type]` with it — a name that misses that table is a name whose colour, id and
/// sticky flag an addon cannot look up, so this is the check that our `CHAT_MSG_*` spellings are
/// the reference's and not ours.
///
/// Swept over `ChatEventKind::ALL` rather than a second hand-written list, so a kind added without
/// a matching table key fails here instead of shipping a name nothing can resolve.
#[test]
fn fired_event_names_are_all_chat_type_info_keys() {
    benilla_formats::wow_data_or_skip!();
    use crate::ui_chat::{event_name, ChatEventKind as K};

    let s = chat_frame();
    for &kind in K::ALL {
        let name = event_name(kind);
        let key = name
            .strip_prefix("CHAT_MSG_")
            .unwrap_or_else(|| panic!("{name}: every fired chat event is CHAT_MSG_-prefixed"));
        let present: bool = s
            .eval(&format!(r#"return ChatTypeInfo["{key}"] ~= nil"#))
            .unwrap();
        assert!(present, r#"{name} → ChatTypeInfo["{key}"] is missing"#);
    }
}

// ── The seven chat windows (NUM_CHAT_WINDOWS) ────────────────────────────────────────────────
//
// benilla shipped two windows against a constant the reference sets to seven. Everything below is
// a claim about the five that were missing, checked against the shipped `ChatFrame.xml` itself
// rather than against the loader's idea of it.

/// `NUM_CHAT_WINDOWS = 7` (ref ChatFrame.lua l.5) and every index it promises resolves to a real
/// `ScrollingMessageFrame`. _LazyPig walks exactly this loop and indexes the result with no nil
/// guard (`LazyPig.lua:1992`), so the constant without the frames is worse than neither.
#[test]
fn every_window_num_chat_windows_promises_is_a_real_frame() {
    benilla_formats::wow_data_or_skip!();
    let s = chat_frame();
    assert_eq!(s.eval::<i64>("return NUM_CHAT_WINDOWS").unwrap(), 7);
    for i in 1..=7 {
        let ok: bool = s
            .eval(&format!(
                "local f = getglobal('ChatFrame{i}') \
                 return f ~= nil and f.AddMessage ~= nil and f:GetID() == {i}"
            ))
            .unwrap();
        assert!(ok, "ChatFrame{i} is a real message frame carrying its id");
    }
}

/// The corpus walk itself, verbatim from `_LazyPig/LazyPig.lua:1992` down to the unguarded
/// `getglobal(...):IsVisible()` that used to die at i=3.
#[test]
fn the_lazypig_window_walk_survives_all_seven_indices() {
    benilla_formats::wow_data_or_skip!();
    let s = chat_frame();
    let visible: i64 = s
        .eval(
            "local n = 0\n\
             for i = 1, NUM_CHAT_WINDOWS do\n\
               local ChatFrame = getglobal('ChatFrame'..i)\n\
               if ChatFrame:IsVisible() then n = n + 1 end\n\
             end\n\
             return n",
        )
        .unwrap();
    assert_eq!(visible, 1, "only the selected dock window is visible");
}

/// ChatFrame3..7 ship hidden and with no `isDocked`, the reference's own chat-cache state
/// (`DOCKED 0 / SHOWN 0`). This is not cosmetic: `Outfitter.lua:3099` reaches an UNGUARDED
/// `getglobal('ChatFrame'..i..'Tab'):GetText()` for any window that is visible *or* docked, and we
/// build no tabs past ChatFrame2Tab.
///
/// **ChatFrame1 and ChatFrame2 DO carry it**, and that is the half this test gained with the
/// move/resize arc: the reference's own gates read `chatFrame.isDocked` — `isDocked and chatFrame
/// ~= DEFAULT_CHAT_FRAME` is what stops the Combat Log being dragged out of the dock it is
/// anchored into — so the flag has a consumer now rather than being an advertisement. Both have
/// tabs (1575), so the Outfitter walk below is still safe.
#[test]
fn the_undocked_windows_are_hidden_and_carry_no_is_docked() {
    benilla_formats::wow_data_or_skip!();
    let s = chat_frame();
    for i in 1..=2 {
        let docked: bool = s
            .eval(&format!("return ChatFrame{i}.isDocked ~= nil"))
            .unwrap();
        assert!(docked, "ChatFrame{i} is docked and says so");
    }
    for i in 2..=7 {
        let shown: bool = s.eval(&format!("return ChatFrame{i}:IsShown()")).unwrap();
        assert!(!shown, "ChatFrame{i} ships hidden");
    }
    for i in 3..=7 {
        let docked: bool = s
            .eval(&format!("return ChatFrame{i}.isDocked ~= nil"))
            .unwrap();
        assert!(!docked, "ChatFrame{i} carries no isDocked");
    }
    // And therefore the Outfitter walk never reaches a tab that does not exist.
    let ok: bool = s
        .eval(
            "for i = 1, NUM_CHAT_WINDOWS do\n\
               local f = getglobal('ChatFrame'..i)\n\
               if f and (f:IsVisible() or f.isDocked) then\n\
                 local tab = getglobal('ChatFrame'..i..'Tab')\n\
                 if not tab then return false end\n\
                 local _ = tab:GetText()\n\
               end\n\
             end\n\
             return true",
        )
        .unwrap();
    assert!(ok, "the Outfitter tab walk never touches a missing tab");
}

/// `GetChatWindowInfo`'s `shown` is not an independent opinion — it must agree with the frame the
/// shipped XML actually built, or an addon that trusts the getter and an addon that trusts the
/// frame will disagree about the same window. The drift guard between the Rust table and the XML.
#[test]
fn get_chat_window_info_shown_matches_the_shipped_frames() {
    benilla_formats::wow_data_or_skip!();
    let s = chat_frame();
    for i in 1..=7 {
        let agrees: bool = s
            .eval(&format!(
                // Both sides are normalised to a boolean before comparing: `IsShown` answers the
                // NUMBER 1 or nil (1830), so comparing it straight against `shown ~= nil` compares
                // a number with a boolean and is false for every window. This test is about the two
                // AGREEING, not about either one's shape.
                "local _, _, _, _, _, _, shown = GetChatWindowInfo({i})\n\
                 return (shown ~= nil) == (ChatFrame{i}:IsShown() ~= nil)"
            ))
            .unwrap();
        assert!(
            agrees,
            "window {i}: GetChatWindowInfo disagrees with the frame"
        );
    }
}

/// A named debug sink is a real ring of its own: `ChatFrame3:AddMessage` (IgniteStatus does this
/// seven times, TipBuddy once, Radar guarded, and AceDebug's `debugFrame` stores the frame) lands
/// in ChatFrame3 and nowhere near the window the player is reading.
#[test]
fn a_line_added_to_chat_frame3_lands_in_chat_frame3_only() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_frame();
    s.add_chat_message("ChatFrame1", "a real line", 1.0, 1.0, 1.0);
    s.run("ChatFrame3:AddMessage('Radar: debug', 1, 1, 0)")
        .unwrap();
    assert_eq!(
        s.eval::<i64>("return ChatFrame3:GetNumMessages()").unwrap(),
        1
    );
    assert_eq!(
        s.eval::<i64>("return ChatFrame1:GetNumMessages()").unwrap(),
        1
    );
    // Hidden, so nothing of it reaches the screen.
    s.resolve();
    assert!(
        text_color(&s.extract(), "Radar: debug").is_none(),
        "a hidden window renders nothing"
    );
}

/// `FCF_SelectDockFrame(frame)` — the corpus idiom (CustomNameplates.lua:69, Roid-Macros
/// Utility.lua:40) is "un-hide the default chat frame before printing into it". It takes a FRAME,
/// where `BenillaFCF_SelectDock` takes a dock id.
///
/// **It used to RAISE on an undocked window and no longer does** (1714). The raise was right for a
/// client whose dock was the literal `for i = 1, 2`: "select ChatFrame5" had no meaning, and
/// answering with silence would have left the caller's next `AddMessage` in a still-hidden window.
/// With a real `DOCKED_CHAT_FRAMES` the reference's own answer applies — the assignment, then
/// `FCF_DockUpdate` hiding every docked window (none of them is the selection) and leaving the
/// undocked one exactly as it was. Not a nice answer, but the client's; and neither corpus caller
/// reaches it, since both pass `DEFAULT_CHAT_FRAME`.
#[test]
fn fcf_select_dock_frame_selects_by_frame_and_leaves_an_undocked_one_alone() {
    benilla_formats::wow_data_or_skip!();
    let s = chat_frame();
    s.run("FCF_SelectDockFrame(ChatFrame2)").unwrap();
    let (one, two): (bool, bool) = (
        s.eval("return ChatFrame1:IsShown()").unwrap(),
        s.eval("return ChatFrame2:IsShown()").unwrap(),
    );
    assert!(!one && two, "selecting the Combat Log swaps the dock");

    s.run("if not DEFAULT_CHAT_FRAME:IsVisible() then FCF_SelectDockFrame(DEFAULT_CHAT_FRAME) end")
        .unwrap();
    assert!(
        s.eval::<bool>("return ChatFrame1:IsShown()").unwrap(),
        "the corpus guard brings the default frame back"
    );

    // An undocked window: no raise, and the reference's consequence — the dock's members all
    // hide, because none of them is the selection, and ChatFrame5 is untouched (still hidden,
    // since nothing showed it).
    let shown5_before: bool = s
        .eval("return ChatFrame5:IsShown() and true or false")
        .unwrap();
    s.run("FCF_SelectDockFrame(ChatFrame5)").unwrap();
    assert_eq!(
        s.eval::<String>("return SELECTED_DOCK_FRAME:GetName()")
            .unwrap(),
        "ChatFrame5"
    );
    assert!(
        !s.eval::<bool>("return ChatFrame1:IsShown()").unwrap()
            && !s.eval::<bool>("return ChatFrame2:IsShown()").unwrap(),
        "FCF_DockUpdate hides every docked window when the selection is not one of them"
    );
    assert_eq!(
        s.eval::<bool>("return ChatFrame5:IsShown() and true or false")
            .unwrap(),
        shown5_before,
        "an undocked window is not in the loop, so nothing shows or hides it"
    );
}

/// **The idle dock writes nothing**. `FCF_OnUpdate`'s apply block used to run
/// unconditionally — ~24 `getglobal`s, two table builds and ~20 `SetAlpha` calls every frame of
/// every session, writing the values that were already there, at 36 µs/frame (1395 measured it as
/// the largest single handler in the client).
///
/// The probe is a sentinel the applier can never write: it only ever writes `{0, 0.5, 1.0} × reveal`,
/// and `reveal` is pinned at 0 with the cursor away from the dock. If the gate is removed, the very
/// next tick overwrites 0.42 with 0 and this goes red.
#[test]
fn an_idle_dock_stops_rewriting_the_tab_alpha_every_frame() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_frame();
    s.mouse_move(1500.0, 850.0); // far from the dock: no hover, so `reveal` stays 0
    for _ in 0..8 {
        s.tick(0.016); // let the label measurements land and the applier reach its resting write
        s.resolve();
    }

    s.run("ChatFrame1Tab:SetAlpha(0.42)").unwrap();
    for _ in 0..10 {
        s.tick(0.016);
        s.resolve();
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    let alpha: f64 = s.eval("return ChatFrame1Tab:GetAlpha()").unwrap();
    assert!(
        (alpha - 0.42).abs() < 1e-6,
        "a settled dock must not rewrite its tab alpha — got {alpha}"
    );
}

/// The control for the guard above: the gate must not cost the reveal it is gating. A stationary
/// cursor over the dock for longer than `CHAT_TAB_SHOW_DELAY` fades the selected tab to full.
#[test]
fn hovering_the_dock_still_reveals_the_tabs() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_frame();
    s.mouse_move(1500.0, 850.0);
    for _ in 0..4 {
        s.tick(0.016);
        s.resolve();
    }
    assert!(
        !s.eval::<bool>("return ChatFrame1Tab:IsVisible()").unwrap(),
        "the dock starts concealed — the tab template ships hidden"
    );
    let (x, y): (f32, f32) = s
        .eval(
            "return (ChatFrame1:GetLeft() + ChatFrame1:GetRight()) / 2, \
             (ChatFrame1:GetBottom() + ChatFrame1:GetTop()) / 2",
        )
        .unwrap();
    s.mouse_move(x, y);
    for _ in 0..45 {
        s.tick(0.016);
        s.resolve();
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    let alpha: f64 = s.eval("return ChatFrame1Tab:GetAlpha()").unwrap();
    assert!(
        s.eval::<bool>("return ChatFrame1Tab:IsVisible()").unwrap() && (alpha - 1.0).abs() < 1e-6,
        "a stationary hover past CHAT_TAB_SHOW_DELAY reveals the selected tab at full alpha — got {alpha}"
    );
}

/// **A view at the bottom writes nothing** — `ChatFrame_OnUpdate`'s at-bottom branch used to call
/// `flash:Hide()` and zero `flashTimer` every frame; the 1.12 reference gates both behind
/// `flash:IsVisible()` (1396's residual item 3, now restored — our `flashTimer = 0` rides inside
/// the reference's gate, so a return-to-bottom during a hidden blink phase keeps the residual
/// phase, a divergence that costs nothing at rest).
///
/// The probe is a sentinel the gated branch can only zero: `flashTimer = 0.42` with the flash
/// hidden survives ten ticks. The controls: scrolled up, the bottom-button blink still runs its
/// 0.5s toggle; and scrolling back while the flash is LIT still hides it and zeroes the timer.
#[test]
fn a_chat_view_at_the_bottom_stops_rewriting_the_flash() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_frame();
    for t in ["L0", "L1", "L2"] {
        s.add_chat_message("ChatFrame1", t, 1.0, 1.0, 1.0);
    }
    for _ in 0..3 {
        s.tick(0.016);
        s.resolve();
    }
    s.run("ChatFrame1.flashTimer = 0.42").unwrap();
    for _ in 0..10 {
        s.tick(0.016);
        s.resolve();
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    let timer: f64 = s.eval("return ChatFrame1.flashTimer").unwrap();
    assert!(
        (timer - 0.42).abs() < 1e-6,
        "a settled view must not rewrite flashTimer every frame — got {timer}"
    );
    assert!(
        !s.eval::<bool>("return ChatFrame1BottomButtonFlash:IsVisible()")
            .unwrap(),
        "the flash stays hidden at rest"
    );
    // The reference's ChatFrameTemplate takes no mouse and has no wheel script (1.12 scrolls its
    // chat by the buttons), so the scroll is the frame's own verb.
    s.run("ChatFrame1:ScrollUp()").unwrap();
    s.tick(0.3); // 0.42 + 0.3 = 0.72 >= CHAT_BUTTON_FLASH_TIME -> toggle on
    s.resolve();
    assert!(
        s.eval::<bool>("return ChatFrame1BottomButtonFlash:IsVisible()")
            .unwrap(),
        "scrolled up, the bottom-button blink lights"
    );
    s.run("ChatFrame1:ScrollToBottom()").unwrap();
    s.tick(0.016);
    s.resolve();
    assert!(
        !s.eval::<bool>("return ChatFrame1BottomButtonFlash:IsVisible()")
            .unwrap(),
        "returning to the bottom hides a lit flash"
    );
    // …and leaves the residual phase where it was: the reference's at-bottom arm hides the flash
    // and returns, it never zeroes the timer (ChatFrame.lua `ChatFrame_OnUpdate`).
    let timer: f64 = s.eval("return ChatFrame1.flashTimer").unwrap();
    assert!(
        (timer - 0.22).abs() < 1e-6,
        "the residual phase stands: {timer}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Selecting the Combat Log must not stop the dock.** The falsification for.
///
/// `FCF_OnUpdate` is the dock's whole driver — the hover reveal, both tabs' alphas, the label
/// settle, the whisper blink — and it used to ride `ChatFrame1`'s own `OnUpdate`.
/// `BenillaFCF_SelectDock(2)` hides ChatFrame1, and a hidden frame gets no `OnUpdate`
/// ([`UiScript::tick`] runs only effectively-visible frames, which is the reference's rule), so the
/// driver died at the exact moment the feature it drives was first used: the tabs froze mid-write,
/// still showing General as the selected one. That is what the director saw and reported, and every
/// test we had passed straight through it — including this file's two dock tests, because both only
/// ever exercise window 1.
///
/// The trap this walks around: calling `FCF_OnUpdate()` by hand — which the first live probe did,
/// and which every earlier test does — bypasses the visibility gate and reports healthy alphas on a
/// dead driver. So this drives the clock and nothing else.
#[test]
fn selecting_the_combat_log_keeps_the_dock_driver_running() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_frame();
    let (x, y): (f32, f32) = s
        .eval(
            "return (ChatFrame1:GetLeft() + ChatFrame1:GetRight()) / 2, \
             (ChatFrame1:GetBottom() + ChatFrame1:GetTop()) / 2",
        )
        .unwrap();
    s.mouse_move(x, y);
    for _ in 0..45 {
        s.tick(0.016);
        s.resolve();
    }
    assert!(
        close(
            s.eval::<f32>("return ChatFrame1Tab:GetAlpha()").unwrap(),
            1.0
        ) && close(
            s.eval::<f32>("return ChatFrame2Tab:GetAlpha()").unwrap(),
            0.5
        ),
        "the hovered dock starts with General selected — this test must not pass vacuously"
    );
    s.run("FCF_SelectDockFrame(ChatFrame2)").unwrap();
    for _ in 0..10 {
        s.tick(0.016);
        s.resolve();
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert!(
        s.eval::<bool>("return ChatFrame2:IsShown() and not ChatFrame1:IsShown()")
            .unwrap(),
        "the selection swapped the windows"
    );
    let (a1, a2): (f32, f32) = s
        .eval("return ChatFrame1Tab:GetAlpha(), ChatFrame2Tab:GetAlpha()")
        .unwrap();
    assert!(
        close(a1, 0.5) && close(a2, 1.0),
        "the selected tab is the one at full alpha — got General {a1}, Combat Log {a2}"
    );
}

/// The other half of the same freeze, and the one a screenshot cannot show: with the driver dead,
/// `reveal` never decayed either, so the chat box and both tabs stayed lit after the cursor left.
#[test]
fn the_dock_still_fades_out_with_the_combat_log_selected() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_frame();
    let (x, y): (f32, f32) = s
        .eval(
            "return (ChatFrame1:GetLeft() + ChatFrame1:GetRight()) / 2, \
             (ChatFrame1:GetBottom() + ChatFrame1:GetTop()) / 2",
        )
        .unwrap();
    s.mouse_move(x, y);
    for _ in 0..45 {
        s.tick(0.016);
        s.resolve();
    }
    s.run("FCF_SelectDockFrame(ChatFrame2)").unwrap();
    for _ in 0..10 {
        s.tick(0.016);
        s.resolve();
    }
    assert!(
        close(
            s.eval::<f32>("return ChatFrame2Tab:GetAlpha()").unwrap(),
            1.0
        ),
        "revealed before the cursor leaves"
    );
    s.mouse_move(1500.0, 850.0); // away from the dock
    for _ in 0..45 {
        s.tick(0.016);
        s.resolve();
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    // The reference fades the tab out and `FCF_ChatTabFadeFinished` hides it.
    assert!(
        !s.eval::<bool>("return ChatFrame2Tab:IsVisible()").unwrap(),
        "the dock conceals itself again on leave"
    );
}

/// The Combat Log window has the scroll column too — the third part of window 2 that was simply
/// never authored (after the tab, 1575, and the border art, 1579). It is a behavioural check, not
/// a name check: scroll up and the bottom button's flash must start blinking on window 2's own
/// button, which only happens if window 2 both HAS the button and runs `ChatFrame_OnUpdate`.
#[test]
fn the_combat_log_window_runs_its_own_bottom_button_blink() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_frame();
    s.run("FCF_SelectDockFrame(ChatFrame2)").unwrap();
    for i in 0..40 {
        s.add_chat_message("ChatFrame2", &format!("line {i}"), 1.0, 1.0, 1.0);
    }
    s.run("ChatFrame2:ScrollUp()").unwrap();
    assert!(
        !s.eval::<bool>("return ChatFrame2:AtBottom()").unwrap(),
        "scrolled off the bottom — this test must not pass vacuously"
    );

    // CHAT_BUTTON_FLASH_TIME is 0.5 s; 40 frames of 16 ms crosses it.
    for _ in 0..40 {
        s.tick(0.016);
        s.resolve();
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert!(
        s.eval::<bool>("return ChatFrame2BottomButtonFlash:IsVisible()")
            .unwrap(),
        "window 2's bottom-button flash blinks while it is scrolled up"
    );
}

/// **The chat menu runs on the REFERENCE's UIMenu kit** — `Interface\FrameXML\UIMenu.xml` and its
/// own `UIMenu.lua`, not a transcription of them.
///
/// Ours kept the reference's ten names in `ChatFrame.xml`, which loads at manifest 939 against the
/// chain's 194, so our copies overwrote the chain's from the day that entry landed — the reverse of
/// 1855's direction, and nothing drove this menu in a test, so nothing said so.
#[test]
fn the_chat_menu_builds_its_rows_on_the_references_kit() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        r"Interface\FrameXML\UIMenu.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\UIMenu.xml",
        "Interface\\FrameXML\\ChatFrame.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\UIPanelTemplates.lua",
        "Interface\\FrameXML\\UIPanelTemplates.xml",
        "Interface\\FrameXML\\FloatingChatFrame.xml",
    ] {
        load_xml(&s, f);
    }
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    let _ = s.errors();

    // `ChatMenu_OnLoad` calls `UIMenu_Initialize()` bare, relying on `this` — the reference's own
    // idiom — then `UIMenu_AddButton` per row. If the kit were missing, the rows would not exist.
    s.run("ChatMenu:Show()").unwrap();
    assert!(s.errors().is_empty(), "opening it raises: {:?}", s.errors());
    assert!(
        s.eval::<i64>("return ChatMenu.numButtons or 0").unwrap() >= 7,
        "the reference's `UIMenu_AddButton` counted the rows this window adds"
    );
    assert_eq!(
        s.eval::<String>("return ChatMenuButton1:GetText()")
            .unwrap(),
        "Say",
        // The reference labels a row with `button:SetText(text)` — the Button's own text, not a
        // named FontString, so there is no `…Button1Text` to read.
        "row 1 is the Say row, built by the chain's kit"
    );

    // A row click runs the owner's func and closes the menu — `UIMenuButton_OnClick`'s contract.
    s.run("ChatMenuButton1:Click()").unwrap();
    assert!(
        s.errors().is_empty(),
        "a row click raises: {:?}",
        s.errors()
    );
    assert!(
        !s.eval::<bool>("return ChatMenu:IsShown() and true or false")
            .unwrap(),
        "the reference's row click hides the menu"
    );
    // `ChatMenu_Say` → `ChatMenu_SetChatType(chatFrame, "SAY")`: the box opens and its type is
    // set directly — no slash is typed into it (ChatFrame.lua l.2245-2255).
    assert!(
        s.eval::<bool>(
            "return ChatFrameEditBox:IsVisible() and ChatFrameEditBox.chatType == 'SAY'"
        )
        .unwrap(),
        "and it opened the edit box as SAY"
    );
}

/// **A glass window's plate fades in on every hover, not only the first.** A restored window
/// (the cache's `COLOR 0 0 0 0`, the stock row) keeps its nine plate textures at alpha 0 and
/// relies on `FCF_OnUpdate`'s hover arm to lift them to `DEFAULT_CHATFRAME_ALPHA` and drop them
/// back (FloatingChatFrame.lua l.873-877, l.913-916). The director's report (2026-09-05): hovering
/// the chat window shows the tabs and the buttons but no plate.
#[test]
fn a_glass_windows_plate_fades_in_on_every_hover() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_frame();
    s.mouse_move(1500.0, 850.0);
    for _ in 0..4 {
        s.tick(0.016);
        s.resolve();
    }
    let bg =
        |s: &mut UiScript| -> f64 { s.eval("return ChatFrame1Background:GetAlpha()").unwrap() };
    assert_eq!(bg(&mut s), 0.0, "a glass window rests at alpha 0");
    let (x, y): (f32, f32) = s
        .eval(
            "return (ChatFrame1:GetLeft() + ChatFrame1:GetRight()) / 2, \
             (ChatFrame1:GetBottom() + ChatFrame1:GetTop()) / 2",
        )
        .unwrap();
    for round in 1..=3 {
        s.mouse_move(x, y);
        for _ in 0..45 {
            s.tick(0.016);
            s.resolve();
        }
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        let a = bg(&mut s);
        assert!(
            (a - 0.25).abs() < 1e-6,
            "hover {round}: the plate fades to DEFAULT_CHATFRAME_ALPHA — got {a} (oldAlpha={:?})",
            s.eval::<Option<f64>>("return ChatFrame1.oldAlpha").unwrap()
        );
        s.mouse_move(1500.0, 850.0);
        for _ in 0..45 {
            s.tick(0.016);
            s.resolve();
        }
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        let a = bg(&mut s);
        assert!(
            a.abs() < 1e-6,
            "leave {round}: the plate fades back to its saved alpha — got {a}"
        );
    }
}

/// The same law under the WHOLE shipped manifest, driven the way the app drives it.
#[test]
fn a_glass_windows_plate_fades_in_on_every_hover_under_the_full_manifest() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "{failures:?}");
    s.resolve();
    s.mouse_move(1500.0, 850.0);
    for _ in 0..4 {
        s.tick(0.016);
        s.resolve();
    }
    super::fire_chat_login(&mut s);
    s.resolve();
    let bg =
        |s: &mut UiScript| -> f64 { s.eval("return ChatFrame1Background:GetAlpha()").unwrap() };
    assert_eq!(bg(&mut s), 0.0, "a glass window rests at alpha 0");
    let (x, y): (f32, f32) = s
        .eval(
            "return (ChatFrame1:GetLeft() + ChatFrame1:GetRight()) / 2, \
             (ChatFrame1:GetBottom() + ChatFrame1:GetTop()) / 2",
        )
        .unwrap();
    for round in 1..=3 {
        s.mouse_move(x, y);
        for _ in 0..45 {
            s.tick(0.016);
            s.resolve();
        }
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        // The plate as the renderer receives it: one Background quad over ChatFrame1's rect at
        // the faded alpha, black.
        let plates: Vec<(Option<benilla_ui::layout::Rect>, f32, Option<[f32; 4]>)> = s
            .extract()
            .iter()
            .filter_map(|q| match &q.content {
                QuadContent::Texture {
                    path: Some(p),
                    color,
                    ..
                } if p.to_ascii_lowercase().contains("chatframebackground") => {
                    Some((q.rect, q.alpha, *color))
                }
                _ => None,
            })
            .collect();
        let (left, bottom): (f32, f32) = s
            .eval("return ChatFrame1:GetLeft(), ChatFrame1:GetBottom()")
            .unwrap();
        // The texture's own anchors: TOPLEFT (-2, 3) and BOTTOMLEFT (-2, -6) off the frame.
        let over_frame1 = plates.iter().find(|(r, _, _)| {
            r.is_some_and(|r| {
                (r.left - (left - 2.0)).abs() < 1.0 && (r.bottom - (bottom - 6.0)).abs() < 1.0
            })
        });
        assert!(
            over_frame1.is_some_and(|(_, a, c)| {
                (a - 0.25).abs() < 1e-3 && c.is_some_and(|c| c[0] == 0.0 && c[1] == 0.0 && c[2] == 0.0)
            }),
            "hover {round}: the extracted plate quad over ChatFrame1 ({left}, {bottom}) — {plates:?}"
        );
        let a = bg(&mut s);
        assert!(
            (a - 0.25).abs() < 1e-6,
            "hover {round}: the plate fades to DEFAULT_CHATFRAME_ALPHA — got {a} (oldAlpha={:?}, hover={:?}, hasBeenFaded={:?}, init={:?})",
            s.eval::<Option<f64>>("return ChatFrame1.oldAlpha").unwrap(),
            s.eval::<Option<f64>>("return ChatFrame1.hover").unwrap(),
            s.eval::<Option<f64>>("return ChatFrame1.hasBeenFaded").unwrap(),
            s.eval::<Option<f64>>("return ChatFrame1.init").unwrap(),
        );
        s.mouse_move(1500.0, 850.0);
        for _ in 0..45 {
            s.tick(0.016);
            s.resolve();
        }
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        let a = bg(&mut s);
        assert!(
            a.abs() < 1e-6,
            "leave {round}: the plate fades back to its saved alpha — got {a}"
        );
    }
}

/// **The plate comes back after a quick exit and re-entry** (the director's
/// report: an existing window shows tabs but no plate on hover; a new one shows the plate; the
/// tab menu's opacity slider, used once, makes the hover work again).
///
/// The stock `FCF_OnUpdate` traps itself: leave the window (the tabs' fade-out is queued with
/// `FCF_ChatTabFadeFinished` as its finished callback), re-enter inside `CHAT_FRAME_FADE_TIME`,
/// and the callback fires `chatFrame.oldAlpha = nil` under the live hover — after which the
/// plate arm (`oldAlpha < DEFAULT_CHATFRAME_ALPHA`) never passes and no leave clears `hover`, so
/// the hover-start re-read never runs either. Two VMs prove the guard is what makes the
/// difference: the bare chat stack (the reference's Lua alone) traps, the shipped manifest (the
/// guard installed by `bootstrap_positions`) does not. The control keeps this from passing
/// vacuously — if the stock Lua or the engine ever stop trapping, it says so, and the guard is
/// then a repair of nothing.
#[test]
fn a_quick_exit_and_reentry_keeps_the_plates_hover_fade() {
    benilla_formats::wow_data_or_skip!();
    fn drive(s: &mut UiScript) -> (f64, f64, f64) {
        let (x, y): (f32, f32) = s
            .eval(
                "return (ChatFrame1:GetLeft() + ChatFrame1:GetRight()) / 2, \
                 (ChatFrame1:GetBottom() + ChatFrame1:GetTop()) / 2",
            )
            .unwrap();
        let bg =
            |s: &mut UiScript| -> f64 { s.eval("return ChatFrame1Background:GetAlpha()").unwrap() };
        s.mouse_move(1500.0, 850.0);
        for _ in 0..4 {
            s.tick(0.016);
            s.resolve();
        }
        // A stationary hover past CHAT_TAB_SHOW_DELAY and the ramp: the plate is up.
        s.mouse_move(x, y);
        for _ in 0..45 {
            s.tick(0.016);
            s.resolve();
        }
        let first = bg(s);
        // Out for ~50 ms — inside the 0.15 s fade-out — and back, then stationary.
        s.mouse_move(1500.0, 850.0);
        for _ in 0..3 {
            s.tick(0.016);
            s.resolve();
        }
        s.mouse_move(x, y);
        for _ in 0..45 {
            s.tick(0.016);
            s.resolve();
        }
        let reentry = bg(s);
        // Away for a full second, then a fresh hover: whatever the re-entry left behind stays.
        s.mouse_move(1500.0, 850.0);
        for _ in 0..60 {
            s.tick(0.016);
            s.resolve();
        }
        s.mouse_move(x, y);
        for _ in 0..45 {
            s.tick(0.016);
            s.resolve();
        }
        let later = bg(s);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        (first, reentry, later)
    }

    // The control: the reference's Lua alone.
    let mut bare = chat_frame();
    let (first, reentry, later) = drive(&mut bare);
    assert!((first - 0.25).abs() < 1e-6, "control, first hover: {first}");
    assert!(
        reentry.abs() < 1e-6 && later.abs() < 1e-6,
        "the control must trap — the reference's Lua nils oldAlpha under a live hover \
         (re-entry {reentry}, later {later}); if it no longer does, the guard is a repair of nothing"
    );
    assert!(
        bare.eval::<bool>("return ChatFrame1Tab:IsVisible() and ChatFrame1.oldAlpha == nil and ChatFrame1.hover == 1")
            .unwrap(),
        "the trapped state the director described: tab up, oldAlpha nil, hover stuck"
    );

    // The shipped interface, with the guard the manifest load installs.
    let mut shipped = UiScript::new().unwrap();
    shipped.set_screen_size(1600.0, 900.0);
    super::test_ui::load_ui(&shipped, "Interface\\FrameXML\\GlobalStrings.lua");
    let failures = super::load_default_ui(&shipped);
    assert!(failures.is_empty(), "{failures:?}");
    shipped.resolve();
    super::fire_chat_login(&mut shipped);
    shipped.resolve();
    let (first, reentry, later) = drive(&mut shipped);
    assert!((first - 0.25).abs() < 1e-6, "shipped, first hover: {first}");
    assert!(
        (reentry - 0.25).abs() < 1e-6,
        "shipped: the plate comes back on the quick re-entry — got {reentry}"
    );
    assert!(
        (later - 0.25).abs() < 1e-6,
        "shipped: and on every hover after — got {later}"
    );
}

/// **`/afk` and `/dnd`, through the reference's own parser and out the far side as a SEND.**
///
/// The director's report: typing either printed `Unknown chat type "AFK".` and nothing happened —
/// no away flag, so no `<AFK>` over the head and no `<AFK>` on their chat lines, because both of
/// those surfaces were already built and were waiting on a flag that never got set.
///
/// The break was a seam, which is why this test spans it rather than testing either side. benilla's
/// own slash grammar has always had a working `/afk` (`ui_chat::input::parse`'s `S::ChatAfk`), but
/// migrating the chat window (1948) put the stock `ChatFrame.lua` on the chain and **its** parser
/// claims a slash line first — so the live path became
/// `ChatEdit_ParseText` → `SlashCmdList["CHAT_AFK"](msg)` → `SendChatMessage(msg, "AFK")` →
/// `SendType::from_token`, which answered `None` because a doc comment there asserted AFK/DND
/// "set a flag rather than sending a line". They do not: they are `CMSG_MESSAGECHAT` `0x14`/`0x15`
/// and the SERVER toggles the bit.
///
/// So the assertion runs the two halves in series: the stock file really does emit the token, and
/// the token really does resolve to a wire kind. Testing either alone passes while the seam is
/// broken — which is exactly how this shipped.
#[test]
fn the_stock_afk_and_dnd_commands_reach_the_wire() {
    benilla_formats::wow_data_or_skip!();
    use crate::net::ChatKind;
    use crate::ui_chat::edit::SendType;

    let typed = |line: &str| -> (String, String) {
        let mut s = chat_frame();
        assert!(s.focus_editbox("ChatFrameEditBox"));
        s.char_input(line);
        assert!(s.key_input("ENTER"), "the box consumes ENTER");
        let sends = s.take_chat_sends();
        assert_eq!(sends.len(), 1, "{line:?} produced one send");
        (sends[0].text.clone(), sends[0].chat_type.clone())
    };

    // Half one — the stock `SlashCmdList` body, verbatim: `SendChatMessage(msg, "AFK")`.
    let (text, token) = typed("/afk Away from Keyboard");
    assert_eq!(
        (text.as_str(), token.as_str()),
        ("Away from Keyboard", "AFK")
    );
    // Half two — the seam that was cut. `None` here is the reported bug.
    assert_eq!(
        SendType::from_token(&token).map(SendType::wire),
        Some(ChatKind::Afk),
        "the token the stock file sends must resolve to a wire kind"
    );

    let (text, token) = typed("/dnd Do not Disturb");
    assert_eq!((text.as_str(), token.as_str()), ("Do not Disturb", "DND"));
    assert_eq!(
        SendType::from_token(&token).map(SendType::wire),
        Some(ChatKind::Dnd)
    );

    // **A BARE `/afk` still sends** — with an empty body, which is the toggle. The stock body
    // passes `msg` straight through, and vmangos's `CHAT_MSG_AFK` arm reads an empty message as
    // "toggle" and a non-empty one as "set, and store this as the auto-reply".
    let (text, token) = typed("/afk");
    assert_eq!((text.as_str(), token.as_str()), ("", "AFK"));
    assert_eq!(
        SendType::from_token(&token).map(SendType::wire),
        Some(ChatKind::Afk),
        "the bare toggle is a send too, not a no-op"
    );

    // The control: a token the reference has no send for still answers None, so the arms above
    // are two specific additions and not a blanket "anything goes" that would let a typo through.
    assert!(SendType::from_token("NOT_A_CHAT_TYPE").is_none());
    assert!(
        SendType::from_token("TEXT_EMOTE").is_none(),
        "a real ChatTypeInfo key that is nonetheless not sendable stays None"
    );
}
