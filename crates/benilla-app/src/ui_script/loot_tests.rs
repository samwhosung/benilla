use benilla_ui::script::{
    DressUpIntent, ExtractedQuad, LootRow, LootState, MerchantItem, MerchantState, QuadContent,
    SoundRequest, UiScript,
};

use super::test_ui::load_ui as load_xml;

/// The rect of the `QuadContent::Frame` quad sized `w` by `h`: every frame emits one.
fn frame_rect(quads: &[ExtractedQuad], w: f32, h: f32) -> benilla_ui::layout::Rect {
    quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Frame => q
                .rect
                .filter(|r| (r.width() - w).abs() < 0.5 && (r.height() - h).abs() < 0.5),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no bare-frame quad sized {w}x{h}"))
}

fn icon_center(quads: &[ExtractedQuad], needle: &str) -> (f32, f32) {
    let r = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
        })
        .and_then(|q| q.rect)
        .unwrap_or_else(|| panic!("no icon quad for {needle}"));
    ((r.left + r.right) * 0.5, (r.bottom + r.top) * 0.5)
}

fn text_color(quads: &[ExtractedQuad], t: &str) -> Option<[f32; 4]> {
    quads.iter().find_map(|q| match &q.content {
        QuadContent::Text {
            text: Some(x),
            color,
            ..
        } if x == t => Some(*color),
        _ => None,
    })?
}

fn coin_and_two_items() -> LootState {
    LootState {
        fishing: false,
        master_candidates: Vec::new(),
        rows: vec![
            Some(LootRow {
                item_id: 0,
                name: Some("1g 23s 45c".into()),
                texture: Some("Interface\\Icons\\INV_Misc_Coin_01".into()),
                quantity: 1,
                quality: Some(1),
                is_coin: true,
                link: None,
                random_property_id: 0,
            }),
            Some(LootRow {
                item_id: 0,
                name: Some("Wool Cloth".into()),
                texture: Some("Interface\\Icons\\INV_Fabric_Wool_01".into()),
                quantity: 3,
                quality: Some(2), // uncommon → green text
                is_coin: false,
                link: None,
                random_property_id: 0,
            }),
            Some(LootRow {
                item_id: 0,
                name: Some("Linen Cloth".into()),
                texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
                quantity: 1,
                quality: Some(1), // common → white text
                is_coin: false,
                link: None,
                random_property_id: 0,
            }),
        ],
    }
}

/// Open at the left slot, the coin row first, 1-based row picks, a looted row hidden in place.
#[test]
fn shipped_loot_frame_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    assert_eq!(
        load_xml(&s, "Interface\\FrameXML\\LootFrame.xml"),
        34,
        "the STOCK file's own shape: the window, its portrait overlay and Next/Prev art, \
         four LootButton rows each carrying ItemButtonTemplate's sub-frames, the two pagers, the \
         close button, and GroupLootDropDown with the dropdown template's own children. Our \
         transcription materialized 10 — a count is a fingerprint of a file, not a property of a \
         loot window, and this one is now the reference's"
    );

    s.resolve();
    let has_icon = |quads: &[ExtractedQuad], needle: &str| {
        quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
        })
    };
    assert!(
        !has_icon(&s.extract(), "INV_Misc_Coin_01"),
        "loot window starts hidden"
    );
    assert!(s.eval::<bool>("return GetLeftFrame() == nil").unwrap());

    s.set_loot(Some(coin_and_two_items()));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(s.eval::<bool>("return LootFrame:IsVisible()").unwrap());
    let vis: (bool, bool, bool, bool) = s
        .eval(
            "return LootButton1:IsVisible(), LootButton2:IsVisible(),\n\
                    LootButton3:IsVisible(), LootButton4:IsVisible()",
        )
        .unwrap();
    assert_eq!(vis, (true, true, true, false), "coin + 2 items, 4th hidden");
    assert!(!s
        .eval::<bool>("return LootFrameDownButton:IsVisible()")
        .unwrap());

    s.resolve();
    let quads = s.extract();

    // The left panel slot: top-left at (0, 664), the 768 screen less the slot's 104.
    let win = frame_rect(&quads, 256.0, 256.0);
    assert_eq!(
        (win.left, win.top),
        (0.0, 664.0),
        "loot window landed at the left slot (TOPLEFT UIParent, 0, -104)"
    );

    // `UI-LootPanel` is the window art (LootFrame.xml:88): unsized, it fills the 256x256 window.
    let panel = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("UI-LootPanel"))
        })
        .and_then(|q| q.rect)
        .expect("no UI-LootPanel art quad");
    assert!(
        (panel.width() - 256.0).abs() < 0.5 && (panel.height() - 256.0).abs() < 0.5,
        "loot panel fills the 256×256 window, got {}×{}",
        panel.width(),
        panel.height()
    );
    assert_eq!(
        (panel.left, panel.top),
        (win.left, win.top),
        "loot panel pinned to the window's TOPLEFT"
    );

    // Row text wears `ITEM_QUALITY_COLORS`: uncommon (0.12, 1, 0), common white.
    let has_text = |t: &str| {
        quads
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(x), .. } if x == t))
    };
    assert!(has_text("1g 23s 45c"), "coin row shows the money amount");
    assert!(has_text("Wool Cloth"), "item row shows the name");
    let green = text_color(&quads, "Wool Cloth").expect("Wool Cloth colour");
    assert!(
        (green[0] - 0.12).abs() < 0.02 && (green[1] - 1.0).abs() < 0.02 && green[2].abs() < 0.02,
        "uncommon item text is green, got {green:?}"
    );
    let white = text_color(&quads, "Linen Cloth").expect("Linen Cloth colour");
    assert!(
        (white[0] - 1.0).abs() < 0.02
            && (white[1] - 1.0).abs() < 0.02
            && (white[2] - 1.0).abs() < 0.02,
        "common item text is white, got {white:?}"
    );
    // The count sits at `ItemButtonTemplate`'s BOTTOMRIGHT (-5, 2) inset of the 37px icon.
    let count_rect = quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text { text: Some(x), .. } if x == "3" => q.rect,
            _ => None,
        })
        .expect("the x3 stack count renders");
    let icon_rect = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("INV_Fabric_Wool_01"))
        })
        .and_then(|q| q.rect)
        .expect("Wool Cloth icon quad");
    assert!(
        (count_rect.right - (icon_rect.right - 5.0)).abs() < 0.6,
        "count right edge pinned 5px inside the icon (ref ItemButtonTemplate), got {} vs icon right {}",
        count_rect.right,
        icon_rect.right
    );
    assert!(
        (count_rect.bottom - (icon_rect.bottom + 2.0)).abs() < 0.6,
        "count bottom pinned 2px above the icon bottom, got {} vs icon bottom {}",
        count_rect.bottom,
        icon_rect.bottom
    );

    let (cx, cy) = icon_center(&quads, "INV_Misc_Coin_01");
    s.mouse_button(cx, cy, "LeftButton", true);
    s.mouse_button(cx, cy, "LeftButton", false);
    let (ix, iy) = icon_center(&quads, "INV_Fabric_Wool_01");
    s.mouse_button(ix, iy, "LeftButton", true);
    s.mouse_button(ix, iy, "LeftButton", false);
    assert_eq!(
        s.take_loot_picks(),
        vec![1, 2],
        "coin row is pick 1, the first item row is pick 2"
    );
    assert!(!s.take_loot_close());

    // The looted coin row hides in place, off `LOOT_SLOT_CLEARED` alone, and the items keep their
    // rows.
    let mut coin_looted = coin_and_two_items();
    coin_looted.rows[0] = None;
    s.set_loot(Some(coin_looted));
    s.fire_event(
        "LOOT_SLOT_CLEARED",
        vec![benilla_ui::script::ScriptValue::Int(1)],
    );
    let vis2: (bool, bool, bool) = s
        .eval(
            "return LootButton1:IsVisible(), LootButton2:IsVisible(),\n\
                    LootButton3:IsVisible()",
        )
        .unwrap();
    assert_eq!(
        vis2,
        (false, true, true),
        "the looted coin row hides in place — the items do not shift up"
    );

    s.run("LootCloseButton:Click()").unwrap();
    assert!(
        s.take_loot_close(),
        "closing the window releases the loot (OnHide → CloseLoot)"
    );
    assert!(!s.eval::<bool>("return LootFrame:IsVisible()").unwrap());
    assert!(
        s.eval::<bool>("return GetLeftFrame() == nil").unwrap(),
        "HideUIPanel vacated the left slot"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Only an empty loot plays a Lua sound, `LOOTWINDOWOPENEMPTY` (LootFrame.lua:135-136).
#[test]
fn loot_empty_roll_plays_the_empty_open_kit() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");

    s.set_loot(Some(coin_and_two_items()));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.take_sounds().is_empty(),
        "a non-empty loot open is silent (its open kit is C-side)"
    );

    s.fire_event("LOOT_CLOSED", vec![]);
    assert!(s.take_sounds().is_empty(), "loot close is silent (C-side)");

    s.set_loot(Some(LootState {
        fishing: false,
        master_candidates: Vec::new(),
        rows: vec![],
    }));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("LOOTWINDOWOPENEMPTY".into())],
        "an empty loot roll plays the empty-open kit"
    );
}

/// A fishing open plays "FISHING REEL IN" (kit 3407, "Fishing Reel in": the lookup ignores case)
/// and swaps the skull for `FishingLoot-Icon`; every show resets it (LootFrame.lua:134-140).
#[test]
fn fishing_loot_open_plays_the_reel_and_swaps_the_portrait() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");
    let has_icon = |quads: &[ExtractedQuad], needle: &str| {
        quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
        })
    };

    let mut fished = coin_and_two_items();
    fished.fishing = true;
    s.set_loot(Some(fished));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("FISHING REEL IN".into())],
        "a fishing loot open plays the reel-in kit"
    );
    let quads = s.extract();
    assert!(
        has_icon(&quads, "FishingLoot-Icon"),
        "the portrait ring shows the fishing icon"
    );
    assert!(
        !has_icon(&quads, "TargetDead"),
        "…instead of the dead-target skull"
    );

    s.fire_event("LOOT_CLOSED", vec![]);
    let _ = s.take_sounds();
    s.set_loot(Some(coin_and_two_items()));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.take_sounds().is_empty(),
        "a non-fishing loot open is silent (its open kit is C-side)"
    );
    let quads = s.extract();
    assert!(has_icon(&quads, "TargetDead"), "the skull is restored");
    assert!(!has_icon(&quads, "FishingLoot-Icon"));
}

/// Past four rows the pager takes a button: five items page as 3 + 2 (LootFrame.lua:70-73,105-118).
#[test]
fn shipped_loot_frame_pages_five_items() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");

    let rows: Vec<Option<LootRow>> = (0..5)
        .map(|i| {
            Some(LootRow {
                item_id: 0,
                name: Some(format!("Item {i}")),
                texture: Some(format!("Interface\\Icons\\Item_{i}")),
                quantity: 1,
                quality: Some(1),
                is_coin: false,
                link: None,
                random_property_id: 0,
            })
        })
        .collect();
    s.set_loot(Some(LootState {
        fishing: false,
        master_candidates: Vec::new(),
        rows: rows.clone(),
    }));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let page1: (bool, bool, bool, bool) = s
        .eval(
            "return LootButton1:IsVisible(), LootButton2:IsVisible(),\n\
                    LootButton3:IsVisible(), LootButton4:IsVisible()",
        )
        .unwrap();
    assert_eq!(page1, (true, true, true, false), "page 1 shows 3 rows");
    let pager1: (bool, bool) = s
        .eval("return LootFrameUpButton:IsVisible(), LootFrameDownButton:IsVisible()")
        .unwrap();
    assert_eq!(pager1, (false, true), "page 1: Up hidden, Down shown");

    s.run("LootFrame_PageDown()").unwrap();
    let page2: (bool, bool, bool) = s
        .eval(
            "return LootButton1:IsVisible(), LootButton2:IsVisible(),\n\
                    LootButton3:IsVisible()",
        )
        .unwrap();
    assert_eq!(page2, (true, true, false), "page 2 shows the last 2 rows");
    let pager2: (bool, bool) = s
        .eval("return LootFrameUpButton:IsVisible(), LootFrameDownButton:IsVisible()")
        .unwrap();
    assert_eq!(pager2, (true, false), "page 2: Up shown, Down hidden");

    // Loot out page 1: it pages down by itself, off `LOOT_SLOT_CLEARED` alone
    // (LootFrame.lua:38-50).
    s.run("LootFrame_PageUp()").unwrap();
    let mut cleared = rows;
    cleared[0] = None;
    cleared[1] = None;
    cleared[2] = None;
    s.set_loot(Some(LootState {
        fishing: false,
        master_candidates: Vec::new(),
        rows: cleared,
    }));
    for row in 1..=3 {
        s.fire_event(
            "LOOT_SLOT_CLEARED",
            vec![benilla_ui::script::ScriptValue::Int(row)],
        );
    }
    assert_eq!(
        s.eval::<i64>("return LootFrame.page").unwrap(),
        2,
        "an emptied page auto-advances to the next"
    );
    let auto: (bool, bool) = s
        .eval("return LootButton1:IsVisible(), LootButton2:IsVisible()")
        .unwrap();
    assert_eq!(auto, (true, true), "…showing the two remaining rows");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Loot, then merchant (pushable 7 and 0, UIParent.lua:22,26): loot is pushed to the centre slot.
#[test]
fn shipped_loot_pushed_to_center_by_merchant() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");

    s.set_loot(Some(coin_and_two_items()));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GetLeftFrame():GetName() == \"LootFrame\"")
            .unwrap(),
        "loot took the empty left slot"
    );

    s.set_merchant(Some(MerchantState {
        items: vec![MerchantItem {
            name: Some("Refreshing Spring Water".into()),
            texture: Some("Interface\\Icons\\INV_Drink_18".into()),
            price: 25,
            quantity: 1,
            num_available: -1,
            item_id: 159,
            stats: None,
            link: None,
            max_stack: Some(1),
        }],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(
        s.eval::<bool>("return GetCenterFrame():GetName() == \"LootFrame\"")
            .unwrap(),
        "loot was pushed to center, not replaced"
    );
    assert!(
        s.eval::<bool>("return GetLeftFrame():GetName() == \"MerchantFrame\"")
            .unwrap(),
        "merchant took the left slot loot vacated"
    );
    s.resolve();
    let quads = s.extract();
    let loot_center = frame_rect(&quads, 256.0, 256.0);
    assert_eq!(
        (loot_center.left, loot_center.top),
        (384.0, 664.0),
        "loot moved to the center slot (TOPLEFT UIParent, 384, -104)"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The party template's `frameStrata="LOW"` (PartyFrameTemplates.xml:194) must reach an instance
/// and its nested art frame (`SetFrameStrata` `0x76a470` cascades over the subtree): in one
/// stratum that level-1 child draws over the window's level-0 art, whatever the show order.
#[test]
fn the_loot_window_draws_over_the_party_frames() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");

    // The party frame first: showing the window later is not what lifts it.
    s.eval::<()>("PartyMemberFrame1:Show()").unwrap();
    s.set_loot(Some(coin_and_two_items()));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    s.resolve();
    let quads = s.extract();
    let z_of = |needle: &str| {
        quads
            .iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains(needle))
            })
            .unwrap_or_else(|| panic!("no quad for {needle}"))
            .z
    };
    let loot_panel = z_of("UI-LootPanel");
    let party_art = z_of("UI-PartyFrame");
    assert!(
        loot_panel > party_art,
        "the loot window's background must draw OVER the party frame art: \
         panel {loot_panel:#x} vs party {party_art:#x}"
    );
    // The stratum is the draw key's top bits: in sync with `benilla_ui::order`'s `STRATUM_SHIFT`.
    const STRATUM_SHIFT: u32 = 60;
    assert!(
        (party_art >> STRATUM_SHIFT) < (loot_panel >> STRATUM_SHIFT),
        "they must differ by STRATUM, not by luck within one"
    );
}

/// Ctrl previews and shift posts the link (LootFrame.lua:147-154), and neither loots: the take is
/// the `LootButton`'s own click, armed by `SetSlot` (LootFrame.lua:94) and off under any modifier.
#[test]
fn ctrl_and_shift_on_a_loot_row_preview_and_post_without_looting() {
    benilla_formats::wow_data_or_skip!();
    const WOOL_LINK: &str = "|cffffffff|Hitem:2589:0:0:0|h[Wool Cloth]|h|r";
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    for file in [
        r"Interface\FrameXML\UIParent.xml", // UIParent and UIParent.lua
        "Interface\\FrameXML\\LootFrame.xml",
        "Interface\\FrameXML\\DressUpFrame.xml",
        "Interface\\FrameXML\\UIMenu.xml", // the kit ChatMenu/EmoteMenu/VoiceMacroMenu build from
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\ChatFrame.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\UIPanelTemplates.lua",
        "Interface\\FrameXML\\UIPanelTemplates.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\FloatingChatFrame.xml",
    ] {
        load_xml(&s, file);
    }

    s.set_loot(Some(LootState {
        fishing: false,
        master_candidates: Vec::new(),
        rows: vec![
            Some(LootRow {
                item_id: 0,
                name: Some("1g 23s 45c".into()),
                texture: Some("Interface\\Icons\\INV_Misc_Coin_01".into()),
                quantity: 1,
                quality: Some(1),
                is_coin: true,
                link: None, // synthesized row, no item behind it
                random_property_id: 0,
            }),
            Some(LootRow {
                item_id: 2589,
                name: Some("Wool Cloth".into()),
                texture: Some("Interface\\Icons\\INV_Fabric_Wool_01".into()),
                quantity: 3,
                quality: Some(1),
                is_coin: false,
                link: Some(WOOL_LINK.into()),
                random_property_id: 0,
            }),
        ],
    }));
    s.fire_event("LOOT_OPENED", vec![]);
    s.resolve();
    let quads = s.extract();
    let (x, y) = icon_center(&quads, "INV_Fabric_Wool_01");
    let (coin_x, coin_y) = icon_center(&quads, "INV_Misc_Coin_01");

    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    assert_eq!(
        s.take_loot_picks(),
        vec![2],
        "an unmodified click still loots the row"
    );

    assert!(s.focus_editbox("ChatFrameEditBox"));
    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        WOOL_LINK,
        "the row's full escaped link landed in the chat box"
    );
    assert!(
        s.take_loot_picks().is_empty(),
        "a shift-click must not also loot the row"
    );

    // The linkless coin row: `Insert(nil)` is a no-op, as the reference's `lua_tostring` makes it.
    s.set_modifiers(true, false, false);
    s.mouse_button(coin_x, coin_y, "LeftButton", true);
    s.mouse_button(coin_x, coin_y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(
        s.errors().is_empty(),
        "shift-clicking the linkless coin row must not raise: {:?}",
        s.errors()
    );
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        WOOL_LINK,
        "and it must not disturb what is already typed"
    );
    assert!(
        s.take_loot_picks().is_empty(),
        "a shift-click on the coin row must not loot the money either"
    );

    // Alt: nothing, and no loot. No FrameXML binds alt on a loot row; the suppression is
    // `CLootButton::OnClick`'s third modifier gate (`0x41f8f0(2)` at `0x4c1856`).
    s.set_modifiers(false, false, true);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(
        s.take_loot_picks().is_empty(),
        "an alt-click must not loot the row (the C gate's third modifier)"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Ctrl: the dressing room wearing it, and no loot. Last, because the room takes the left panel
    // slot and moves the loot window (pushable 2 against 7).
    s.set_modifiers(false, true, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.take_dressup_intents(),
        vec![DressUpIntent::Dress, DressUpIntent::TryOn(2589)],
        "re-dress first, then try the looted item on"
    );
    assert!(
        s.take_loot_picks().is_empty(),
        "a ctrl-click must not also loot the row"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Master loot: an item at or above `MASTER_LOOT_THREHOLD` (4, epic; the misspelling is the
/// reference's, LootFrame.lua:3) asks `CONFIRM_LOOT_DISTRIBUTION` before it is given.
#[test]
fn shipped_loot_frame_hands_a_master_row_to_a_candidate() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");

    let row = |name: &str, quality: u32| {
        Some(LootRow {
            item_id: 17182,
            name: Some(name.into()),
            texture: Some("Interface\\Icons\\INV_Sword_39".into()),
            quantity: 1,
            quality: Some(quality),
            is_coin: false,
            link: None,
            random_property_id: 0,
        })
    };
    s.set_loot(Some(LootState {
        fishing: false,
        master_candidates: vec![Some("Thrall".into()), Some("Cairne".into())],
        rows: vec![row("Wool Cloth", 2), row("Thunderfury", 4)],
    }));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    let _ = s.take_loot_picks();

    // ── The row click stashes the selection the dropdown will read ────────────────────────────
    let click = |s: &mut UiScript, frame: &str| {
        let (x, y) = s
            .eval::<(f64, f64)>(&format!("return {frame}:GetCenter()"))
            .unwrap();
        s.mouse_button(x as f32, y as f32, "LeftButton", true);
        s.mouse_button(x as f32, y as f32, "LeftButton", false);
        s.resolve();
    };
    click(&mut s, "LootButton1");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(s.eval::<i64>("return LootFrame.selectedSlot").unwrap(), 1);
    assert_eq!(
        s.eval::<i64>("return LootFrame.selectedQuality").unwrap(),
        2
    );
    assert_eq!(
        s.eval::<String>("return LootFrame.selectedItemName")
            .unwrap(),
        "Wool Cloth"
    );
    assert_eq!(
        s.eval::<String>("return LootFrame.selectedLootButton")
            .unwrap(),
        "LootButton1",
        "the dropdown anchors on the clicked row, not the window"
    );
    // The pick still queues: the app chooses take or menu off the wire slot type, as 1.12 does.
    assert_eq!(s.take_loot_picks(), vec![1]);

    // ── The app answers a MASTER row with the event; the menu lists the candidates ─────────────
    s.fire_event("OPEN_MASTER_LOOT_LIST", vec![]);
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "the candidate menu is up"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button1:GetText()")
            .unwrap(),
        "Thrall"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button2:GetText()")
            .unwrap(),
        "Cairne"
    );

    // ── Below the threshold: straight out, no confirmation ────────────────────────────────────
    click(&mut s, "DropDownList1Button2");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_loot_master_gives(),
        vec![(1, 2)],
        "row 1 to candidate 2"
    );
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "an uncommon item is under MASTER_LOOT_THREHOLD — no confirmation"
    );

    // ── At the threshold: the confirmation stands between the click and the send ──────────────
    click(&mut s, "LootButton2");
    assert_eq!(
        s.eval::<i64>("return LootFrame.selectedQuality").unwrap(),
        4
    );
    let _ = s.take_loot_picks();
    s.fire_event("OPEN_MASTER_LOOT_LIST", vec![]);
    s.resolve();
    click(&mut s, "DropDownList1Button1");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.take_loot_master_gives().is_empty(),
        "an epic asks first — nothing is sent on the dropdown click"
    );
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "CONFIRM_LOOT_DISTRIBUTION is up"
    );
    s.run("StaticPopup1Button1:Click()").unwrap(); // Yes
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_loot_master_gives(),
        vec![(2, 1)],
        "row 2 to candidate 1, only after the accept"
    );

    // A row click hides a standing confirmation (LootFrame.lua:156).
    s.fire_event("OPEN_MASTER_LOOT_LIST", vec![]);
    s.resolve();
    click(&mut s, "DropDownList1Button1");
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    click(&mut s, "LootButton1");
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "clicking another row closes the standing confirmation"
    );
}

/// In a raid each candidate sits in its subgroup's five-slot block, and the menu keeps a "Group N"
/// row only for an occupied block (LootFrame.lua:197-213).
#[test]
fn the_master_loot_menu_groups_raid_candidates_by_subgroup() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::{PartyState, RaidMemberInfo};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");

    // A raid: `GetNumRaidMembers() > 0` is the nested arm's whole gate (LootFrame.lua:189).
    s.set_party(PartyState {
        raid: vec![RaidMemberInfo::default(); 7],
        loot_method: "master".into(),
        master_looter: Some(0),
        loot_threshold: 2,
        ..PartyState::default()
    });

    // Subgroup 1 holds slots 1-2 and subgroup 3 slot 11, the rest holes, as the client files them.
    let mut candidates = vec![None; 12];
    candidates[0] = Some("Thrall".to_string());
    candidates[1] = Some("Cairne".to_string());
    candidates[10] = Some("Sylvanas".to_string());
    s.set_loot(Some(LootState {
        fishing: false,
        master_candidates: candidates,
        rows: vec![Some(LootRow {
            item_id: 17182,
            name: Some("Thunderfury".into()),
            texture: Some("Interface\\Icons\\INV_Sword_39".into()),
            quantity: 1,
            quality: Some(2),
            is_coin: false,
            link: None,
            random_property_id: 0,
        })],
    }));
    s.fire_event("LOOT_OPENED", vec![]);
    let (x, y) = s
        .eval::<(f64, f64)>("return LootButton1:GetCenter()")
        .unwrap();
    s.mouse_button(x as f32, y as f32, "LeftButton", true);
    s.mouse_button(x as f32, y as f32, "LeftButton", false);
    s.resolve();
    let _ = s.take_loot_picks();
    s.fire_event("OPEN_MASTER_LOOT_LIST", vec![]);
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert_eq!(
        s.eval::<String>("return DropDownList1Button1:GetText()")
            .unwrap(),
        "Give Loot To:",
        "the raid arm opens with the title row the party arm has no such thing for"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button2:GetText()")
            .unwrap(),
        "Group 1"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button3:GetText()")
            .unwrap(),
        "Group 3",
        "the empty group 2 block contributes no row — and group 3 is NOT relabelled 2"
    );
    assert_eq!(
        s.eval::<i64>("return DropDownList1.numButtons").unwrap(),
        3,
        "title + two occupied groups, out of eight blocks"
    );
}

/// `LOOT_BIND_CONFIRM` raises `LOOT_BIND` with the row on `dialog.data` (UIParent.lua:317-323);
/// Okay hands it back through `LootSlot`, the only verb that completes a deferred take
/// (StaticPopup.lua:601-611), and `LOOT_CLOSED` hides the dialog (LootFrame.lua:53-54).
#[test]
fn the_loot_bind_confirm_raises_the_dialog_and_okay_calls_loot_slot() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");
    s.set_loot(Some(coin_and_two_items()));
    s.fire_event("LOOT_OPENED", vec![]);
    let _ = s.take_loot_picks();

    // The app defers row 2 and says so.
    s.fire_event(
        "LOOT_BIND_CONFIRM",
        vec![benilla_ui::script::ScriptValue::Int(2)],
    );
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "LOOT_BIND_CONFIRM raises the dialog"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1.which").unwrap(),
        "LOOT_BIND"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Looting this item will bind it to you.",
        "the real GlobalStrings LOOT_NO_DROP, which carries no format specifier"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Button1:GetText()")
            .unwrap(),
        "Okay"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Button2:GetText()")
            .unwrap(),
        "Cancel"
    );
    assert_eq!(
        s.eval::<i64>("return StaticPopup1.data").unwrap(),
        2,
        "the row rides on dialog.data, not in the text"
    );

    // Button 2, Cancel: nothing reaches either queue.
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert!(
        s.take_loot_confirms().is_empty(),
        "Cancel completes nothing"
    );
    assert!(s.take_loot_picks().is_empty(), "and takes nothing either");

    // Button 1, Okay: the row goes back out through `LootSlot`.
    s.fire_event(
        "LOOT_BIND_CONFIRM",
        vec![benilla_ui::script::ScriptValue::Int(2)],
    );
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert_eq!(s.take_loot_confirms(), vec![2]);
    assert!(
        s.take_loot_picks().is_empty(),
        "the accept is a continuation, never a fresh take"
    );

    // Closing the window takes the dialog with it: its row would name a slot in the next corpse.
    s.fire_event(
        "LOOT_BIND_CONFIRM",
        vec![benilla_ui::script::ScriptValue::Int(2)],
    );
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    s.fire_event("LOOT_CLOSED", vec![]);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "LOOT_CLOSED hides the confirm"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A row click is a take; `LootSlot` is only the bind-confirm continuation.
#[test]
fn a_row_click_takes_rather_than_continues() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");
    s.set_loot(Some(coin_and_two_items()));
    s.fire_event("LOOT_OPENED", vec![]);
    let quads = s.extract();

    let (ix, iy) = icon_center(&quads, "INV_Fabric_Wool_01");
    s.mouse_button(ix, iy, "LeftButton", true);
    s.mouse_button(ix, iy, "LeftButton", false);
    assert_eq!(s.take_loot_picks(), vec![2], "the click is a TAKE");
    assert!(
        s.take_loot_confirms().is_empty(),
        "and never a continuation"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A row awaiting its template: the app never opens on one (`ui_loot` waits), but stock
/// `LootFrame_Update` indexes `ITEM_QUALITY_COLORS[quality]` unguarded (LootFrame.lua:82-85).
#[test]
fn loot_row_awaiting_its_template_opens_clean() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");

    // One row in flight: the wire gives the icon (by display id) and the count, nothing else.
    s.set_loot(Some(LootState {
        fishing: false,
        master_candidates: Vec::new(),
        rows: vec![Some(LootRow {
            item_id: 2582,
            name: None,
            texture: Some("Interface\\Icons\\INV_Gauntlets_17".into()),
            quantity: 1,
            quality: None,
            is_coin: false,
            link: None,
            random_property_id: 0,
        })],
    }));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(
        s.errors().is_empty(),
        "an in-flight loot row must not raise in LootFrame_Update: {:?}",
        s.errors()
    );
    assert!(
        s.eval::<bool>("return LootFrame:IsVisible() and LootButton1:IsVisible()")
            .unwrap(),
        "the window opened and drew the row"
    );

    // The reference's sentinels, not nils; -1 is a row of `ITEM_QUALITY_COLORS` (UIParent.lua:66).
    let (item, quantity, quality) = s
        .eval::<(String, i64, i64)>("local _, i, n, q = GetLootSlotInfo(1)\nreturn i, n, q")
        .unwrap();
    assert_eq!((item.as_str(), quantity, quality), ("", 1, -1));
    assert!(
        s.eval::<bool>(
            "local _, _, _, quality = GetLootSlotInfo(1) \
             return ITEM_QUALITY_COLORS[quality] ~= nil"
        )
        .unwrap(),
        "the cache-miss quality must be a real row of ITEM_QUALITY_COLORS"
    );

    // Line 85 ran: the row text wears `ITEM_QUALITY_COLORS[-1]`, Common white, because
    // `GetItemQualityColor`'s clamp is unsigned and -1 takes the 7-and-up branch.
    let painted: (f64, f64, f64) = s
        .eval("local r, g, b = LootButton1Text:GetTextColor()\nreturn r, g, b")
        .unwrap();
    let miss: (f64, f64, f64) = s
        .eval("local r, g, b = GetItemQualityColor(-1)\nreturn r, g, b")
        .unwrap();
    assert_eq!(miss, (1.0, 1.0, 1.0), "the cache-miss row is Common/white");
    // f32 region storage against the binding's f64: compare within a tolerance.
    assert!(
        (painted.0 - miss.0).abs() < 1e-6
            && (painted.1 - miss.1).abs() < 1e-6
            && (painted.2 - miss.2).abs() < 1e-6,
        "LootFrame_Update:85 must paint the -1 colour, got {painted:?} want {miss:?}"
    );

    // The template lands and the window opens only now: the reference's item-cache callback
    // (`0x4c2ac0`) fires `LOOT_OPENED` as the pending count falls to zero.
    s.fire_event("LOOT_CLOSED", vec![]);
    s.set_loot(Some(LootState {
        fishing: false,
        master_candidates: Vec::new(),
        rows: vec![Some(LootRow {
            item_id: 2582,
            name: Some("Thin Cloth Gloves".into()),
            texture: Some("Interface\\Icons\\INV_Gauntlets_17".into()),
            quantity: 1,
            quality: Some(1),
            is_coin: false,
            link: None,
            random_property_id: 0,
        })],
    }));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    let white = text_color(&s.extract(), "Thin Cloth Gloves").expect("the resolved name renders");
    assert!(
        (white[0] - 1.0).abs() < 0.02
            && (white[1] - 1.0).abs() < 0.02
            && (white[2] - 1.0).abs() < 0.02,
        "the resolved row is white, got {white:?}"
    );
}

/// A `<LootButton>` loads as a `Button` (`CLootButton` keeps `CSimpleButton`'s `LoadXML`,
/// `0x7788c0`), so it wears `ItemButtonTemplate`'s state textures and lights under the cursor.
#[test]
fn stock_loot_rows_wear_the_item_button_art_and_light_under_the_cursor() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");
    s.set_loot(Some(coin_and_two_items()));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    s.resolve();
    let border = |quads: &[ExtractedQuad]| {
        quads
            .iter()
            .filter(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("UI-Quickslot2"))
            })
            .count()
    };
    assert_eq!(
        border(&s.extract()),
        3,
        "ItemButtonTemplate's <NormalTexture> is the Quickslot border on each of the three rows"
    );

    let hilite = |quads: &[ExtractedQuad]| {
        quads
            .iter()
            .filter_map(|q| match &q.content {
                QuadContent::Texture { path: Some(p), .. }
                    if p.contains("ButtonHilight-Square") =>
                {
                    q.rect
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert!(
        hilite(&s.extract()).is_empty(),
        "no row is lit with the mouse away"
    );

    let icon = |quads: &[ExtractedQuad], needle: &str| {
        quads
            .iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
            })
            .and_then(|q| q.rect)
            .unwrap_or_else(|| panic!("no icon quad for {needle}"))
    };
    let wool = icon(&s.extract(), "INV_Fabric_Wool_01");
    super::test_ui::hover(&mut s, "LootButton2");
    s.resolve();
    let lit = hilite(&s.extract());
    assert_eq!(lit.len(), 1, "one row lights, not three: {lit:?}");
    assert_eq!(
        lit[0], wool,
        "the highlight covers the hovered row's icon square"
    );

    super::test_ui::unhover(&mut s);
    s.resolve();
    assert!(
        hilite(&s.extract()).is_empty(),
        "the highlight is not latched: it goes out when the cursor leaves"
    );
}
