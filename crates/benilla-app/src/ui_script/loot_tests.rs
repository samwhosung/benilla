use benilla_ui::script::{
    DressUpIntent, ExtractedQuad, LootRow, LootState, MerchantItem, MerchantState, QuadContent,
    SoundRequest, UiScript,
};

use super::test_ui::load_ui as load_xml;

/// A bare frame's own rect via its `QuadContent::Frame` entry (every frame emits one at its resolved
/// rect). The re-skinned loot window has no solid-colour fill (the UI-LootPanel slab is opaque), so
/// it's found by its own frame quad rather than a background texture.
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

/// The centre of the first texture quad whose path contains `needle` (a row's icon), for clicking it.
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

/// The colour of the first text quad whose text equals `t`.
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

/// The whole loot chain minus Bevy (decision 0084): LOOT_OPENED lands the window at the left slot,
/// the coin row (first) + two item rows render with quality-coloured text + a stack count, a coin
/// click and an item click each queue the right 1-based row pick, a LOOT_UPDATE with the coin row
/// looted hides ITS button in place (the items keep their rows — the fixed slot layout), and the
/// close button releases through OnHide → CloseLoot.
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
        "the STOCK file's own shape (1751): the window, its portrait overlay and Next/Prev art, \
         four LootButton rows each carrying ItemButtonTemplate's sub-frames, the two pagers, the \
         close button, and GroupLootDropDown with the dropdown template's own children. Our \
         transcription materialized 10 — a count is a fingerprint of a file, not a property of a \
         loot window, and this one is now the reference's"
    );

    // Hidden by default: no coin icon on screen, left slot empty.
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

    // The app's feed: a coin pile + two items.
    s.set_loot(Some(coin_and_two_items()));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Shown at the left slot; three rows visible (coin + 2 items), row 4 hidden.
    assert!(s.eval::<bool>("return LootFrame:IsVisible()").unwrap());
    let vis: (bool, bool, bool, bool) = s
        .eval(
            "return LootButton1:IsVisible(), LootButton2:IsVisible(),\n\
                    LootButton3:IsVisible(), LootButton4:IsVisible()",
        )
        .unwrap();
    assert_eq!(vis, (true, true, true, false), "coin + 2 items, 4th hidden");
    // Only 3 items ⇒ no pager.
    assert!(!s
        .eval::<bool>("return LootFrameDownButton:IsVisible()")
        .unwrap());

    s.resolve();
    let quads = s.extract();

    // The slot anchor applied: top-left at (0, 664) — screen height 768 minus the left slot's 104.
    let win = frame_rect(&quads, 256.0, 256.0);
    assert_eq!(
        (win.left, win.top),
        (0.0, 664.0),
        "loot window landed at the left slot (TOPLEFT UIParent, 0, -104)"
    );

    // The single UI-LootPanel slab IS the window art (ref LootFrame.xml l.88): no size/anchors → it
    // fills the 256×256 window, sampling its whole texture (no TexCoords).
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

    // The coin row is FIRST: its money text renders, and the uncommon item's text is green while the
    // common item's is white (ITEM_QUALITY_COLORS).
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
    // The stack count "3" sits INSIDE the Wool Cloth row's icon, at the reference
    // ItemButtonTemplate inset (BOTTOMRIGHT -5,+2 of the 37px icon) — the director's report was
    // this count overshooting the icon's right border onto the name plate.
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

    // Click the coin row's icon → LootSlot(1); click the Wool Cloth row's icon → LootSlot(2).
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

    // LOOT_UPDATE with the coin row LOOTED: its slot becomes a gap (`None`) — its button hides in
    // place and the two items KEEP their rows (the reference's fixed slot layout; the director's
    // report was these rows sliding up to fill the coin's spot).
    let mut coin_looted = coin_and_two_items();
    coin_looted.rows[0] = None;
    s.set_loot(Some(coin_looted));
    // The app's own pair, in the app's own order (`ui_loot`): the per-slot event first — the
    // stock file hangs "hide the button in place" off it and only off it — then the summary
    // repaint. Firing LOOT_UPDATE alone leaves the looted row drawn, which is what a client that
    // sent only the summary would actually show.
    s.fire_event(
        "LOOT_SLOT_CLEARED",
        vec![benilla_ui::script::ScriptValue::Int(1)],
    );
    s.fire_event("LOOT_UPDATE", vec![]);
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

    // The close button hides the window → OnHide → CloseLoot() queues the release intent.
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

/// The LOOTWINDOWOPENEMPTY branch (LootFrame.lua LootFrame_OnShow, l.135-136): an empty loot roll
/// queues the empty-open kit on show; a normal (non-empty) loot open queues NO sound (that kit is
/// C-side, server-driven). DR 0086 masks the lootable flag for empty rolls so live empty windows are
/// rare — but the branch must exist, and this proves it fires the right kit and only then.
#[test]
fn loot_empty_roll_plays_the_empty_open_kit() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");

    // A normal, non-empty loot open queues no sound (the normal open kit is C-side).
    s.set_loot(Some(coin_and_two_items()));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.take_sounds().is_empty(),
        "a non-empty loot open is silent (its open kit is C-side)"
    );

    // Close it (also silent — the loot-close kit is C-side).
    s.fire_event("LOOT_CLOSED", vec![]);
    assert!(s.take_sounds().is_empty(), "loot close is silent (C-side)");

    // Re-open with an EMPTY roll: OnShow's numItems==0 fork queues exactly LOOTWINDOWOPENEMPTY.
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

/// The fishing fork (LootFrame.lua LootFrame_OnShow l.137-140; decision 1086): a fishing loot open
/// plays "FISHING REEL IN" (SoundEntries "Fishing Reel in", kit 3407 — the name lookup is
/// case-insensitive on both sides) and swaps the portrait ring's skull for the FishingLoot-Icon;
/// the next ordinary open resets the skull (the ref re-stamps TargetDead at every show, l.133).
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

    // Close, then an ordinary corpse loot: silent again, and the skull is back.
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

/// Paging (LootFrame.lua l.70-73,105-118): 5 items ⇒ 3 rows + a Down pager on page 1, 2 rows + an Up
/// pager on page 2 — the real shipped XML driving the render, not just the arithmetic.
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

    // Page 1: 3 rows (the pager spends the 4th slot), Down shown, Up hidden.
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

    // Page down → page 2: 2 rows, Up shown, Down hidden.
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

    // Back on page 1, loot all three of its rows: their buttons hide in place, and the emptied
    // page advances to page 2 on its own — the ref's LOOT_SLOT_CLEARED tail (LootFrame.lua
    // l.38-50: every button hidden + Down visible → PageDown).
    //
    // The events are the app's own, in the app's own order: one LOOT_SLOT_CLEARED per row that
    // went away (`ui_loot`'s emitter), then the summary LOOT_UPDATE. The auto-advance hangs off
    // the per-slot event and ONLY off it, which is why the app fires both.
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
    s.fire_event("LOOT_UPDATE", vec![]);
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

/// Pin §4/§5: loot (pushable=7) open, then merchant (pushable=0) open → loot is pushed to the CENTER
/// slot rather than replaced, and merchant takes the left slot loot vacated — the real windows, not
/// the synthetic stand-in the merchant panel test uses.
#[test]
fn shipped_loot_pushed_to_center_by_merchant() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");

    // Loot opens onto the empty left slot.
    s.set_loot(Some(coin_and_two_items()));
    s.fire_event("LOOT_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GetLeftFrame():GetName() == \"LootFrame\"")
            .unwrap(),
        "loot took the empty left slot"
    );

    // Merchant (pushable=0) opens: loot's pushable=7 outranks it, so loot is promoted to center and
    // merchant takes the left spot.
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
    // Both windows still visible, at their slots.
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

/// **The party frame drew through the loot window** — the director's report, and the reason
/// decision 0597 exists.
///
/// Both own the top-left corner: `ShowUIPanel` plants the loot window at `UIParent TOPLEFT
/// (0,-104)` and `PartyMemberFrame1` sits just above it. Benilla had transcribed the party template
/// without the reference's `frameStrata="LOW"` (`PartyFrameTemplates.xml` l.194), so it inherited
/// the default MEDIUM — the panels' own stratum. Within one stratum the draw key is
/// `level`-then-insertion, and the party frame's art rides a nested `$parentTextureFrame` CHILD
/// (level 1) while the loot window's `UI-LootPanel` slab is a REGION of the window itself (level
/// 0). Level outranks insertion, so being shown later could never have lifted the window above it.
///
/// Asserted on the packed draw key itself, and it pins both halves of the fix: that the template's
/// strata reaches an `inherits=` instance at all, and that it cascades down to the nested child
/// frames (`SetFrameStrata 0x76a470` is a whole-subtree cascade).
#[test]
fn the_loot_window_draws_over_the_party_frames() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");

    // The party frame up FIRST, the window second — the order that cannot be what saves it.
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
    // The window's own background slab (a level-0 region) against the party art (a level-1 child
    // frame's region) — the exact pair that inverted.
    let loot_panel = z_of("UI-LootPanel");
    let party_art = z_of("UI-PartyFrame");
    assert!(
        loot_panel > party_art,
        "the loot window's background must draw OVER the party frame art: \
         panel {loot_panel:#x} vs party {party_art:#x}"
    );
    // And the whole party frame is below the whole window, not just that one pair.
    const STRATUM_SHIFT: u32 = 60;
    assert!(
        (party_art >> STRATUM_SHIFT) < (loot_panel >> STRATUM_SHIFT),
        "they must differ by STRATUM, not by luck within one"
    );
}

/// The row click's modifier fork (ref `LootFrameItem_OnClick`, LootFrame.lua l.147-154): CTRL
/// previews the row's item in the dressing room (decision 1060), SHIFT posts its link into an open
/// chat edit box (decision 1059) — and **neither loots**.
///
/// That last clause is the whole point of the test, and it is the WIDGET's to get right: the loot
/// itself is the `LootButton` kind's click behaviour (`benilla-ui` `script/button.rs`, decision
/// 1799 — `l.94`'s `button:SetSlot(slot)` is what arms it), gated on no shift/ctrl/alt, which is
/// why the stock `LootFrameItem_OnClick` never calls a take itself and needs no `return`. The
/// unmodified click still loots — the regression that would otherwise ship silently.
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
        r"Interface\FrameXML\UIParent.xml", // UIParent + UIParent.lua, the reference's own (1988)
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

    // A coin row + one resolved item: the item's link is fed exactly as `ui_loot.rs` builds it.
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

    // The control first, while nothing else has been opened: a plain click still loots row 2.
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    assert_eq!(
        s.take_loot_picks(),
        vec![2],
        "an unmodified click still loots the row"
    );

    // SHIFT with the chat edit box open → the link, and no loot.
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

    // The COIN row has no link at all, and a modified click on it must be inert rather than an
    // error: our `EditBox:Insert` binding is typed `String` and raises on a nil, so the handler
    // guards it (the reference's C Insert tolerates the nil its own callers hand it).
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

    // ALT → nothing at all, and in particular NOT a loot (decision 1067). No FrameXML file binds
    // alt on a loot row; the suppression is the C `CLootButton::OnClick`'s own third gate
    // (`0x41f8f0(2)` @ `0x4c1841`, VERIFIED at the bytes), which our Lua-side take has to
    // reproduce. This is the arm nothing else in the suite would catch: before 1067, alt looted.
    s.set_modifiers(false, false, true);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(
        s.take_loot_picks().is_empty(),
        "an alt-click must not loot the row (the C gate's third modifier)"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // CTRL → the dressing room wearing it, still no loot. Last, because opening the room takes the
    // left UIPanel slot and moves the loot window (pushable 2 vs 7).
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

/// **Master loot, end to end through the shipped XML** (decision 1675).
///
/// The whole interaction in one pass: a row click stashes the four `LootFrame.selected*` fields
/// the dropdown reads, `OPEN_MASTER_LOOT_LIST` raises the menu anchored on that row, a candidate
/// row hands the item straight out when the item is below `MASTER_LOOT_THREHOLD`, and an at-or-
/// above-threshold item raises `CONFIRM_LOOT_DISTRIBUTION` first and only sends on accept.
///
/// The two thresholds matter: `MASTER_LOOT_THREHOLD` is 4 (epic), so an uncommon goes straight
/// out and an epic asks. The misspelling is the reference's own (`LootFrame.lua:3`).
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
    // The click still queues the pick — deciding whether it is a take or a master-loot open is
    // the app's, off the wire slot type, exactly as the real client decides it in C.
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

    // A fresh row click hides a standing confirmation, so an accept can never land on a row the
    // player has since clicked away from (the ref's own reason for the StaticPopup_Hide).
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

/// **The raid arm of the master-loot menu** (decision 1675).
///
/// In a raid the candidate array is not the wire order: the client files each candidate into its
/// own subgroup's five-slot block, leaving holes. `GroupLootDropDown_Initialize` reads those holes
/// as raid-group membership — it walks `1..40` in blocks of five and keeps a "Group N" submenu only
/// where the block has an occupant. This drives the shipped XML with a hole-shaped list and asserts
/// the menu that comes out, which a densely-packed list could not produce.
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

    // A raid, so the dropdown takes its nested arm (GetNumRaidMembers() > 0 is the whole gate).
    s.set_party(PartyState {
        raid: vec![RaidMemberInfo::default(); 7],
        loot_method: "master".into(),
        master_looter: Some(0),
        loot_threshold: 2,
        ..PartyState::default()
    });

    // Subgroup 1 holds slots 1-2, subgroup 3 holds slot 11 — everything else is a hole. That is
    // the placement `SMSG_LOOT_MASTER_LIST`'s handler produces for a raid.
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

    // A title row, then ONE row per occupied block — not one per candidate, and labelled by the
    // block's own number rather than by position in the list.
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

/// **The soulbind confirm's Lua half** (decision 1744, ref `StaticPopup.lua:601-611` +
/// `UIParent.lua:317-323` + `LootFrame.lua:53-54`). `LOOT_BIND_CONFIRM` raises `LOOT_BIND` with the
/// real `LOOT_NO_DROP` text and Okay/Cancel; the row rides on `dialog.data`, and Okay hands it back
/// through `LootSlot` — the continuation verb, which is the only thing that can complete a deferred
/// take. Cancel sends nothing at all, and `LOOT_CLOSED` takes the dialog down with the window.
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

    // Cancel: no continuation, and nothing reaches either queue.
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert!(
        s.take_loot_confirms().is_empty(),
        "Cancel completes nothing"
    );
    assert!(s.take_loot_picks().is_empty(), "and takes nothing either");

    // Okay: the row goes back out through LootSlot, the continuation verb.
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

    // The window closing takes an open dialog with it (ref LootFrame.lua l.53-54) — its row number
    // would name a slot in the next corpse.
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

/// The row click is `BenillaTakeLootSlot`, not `LootSlot` — a real mouse click on a row must land
/// in the TAKE queue, or every loot would be a continuation nothing is pending for and the window
/// would sit there doing nothing (decision 1744).
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

/// **The state every loot on a cold item cache opens in** — the rows are on the wire, the item
/// templates are not back yet — driven through the STOCK `LootFrame.lua`. Decision 1805.
///
/// The hole this closes: every loot fixture in this file (and in `benilla-ui`'s own) handed the
/// window fully-resolved rows, so nothing in the suite ever ran `LootFrame_Update` over a row whose
/// template was still in flight. Our deleted `LootFrame.xml` could not have noticed either — it
/// carried an `ITEM_QUALITY_COLORS[quality] or ITEM_QUALITY_COLORS[1]` guard, the way
/// `GroupLootFrame.xml` and `AuctionFrame.xml` still do. The stock file has no guard: it does
/// `color = ITEM_QUALITY_COLORS[quality]` (`LootFrame.lua:82`) and dereferences `color.r` on the
/// next line, so a nil quality raised
/// `LootFrame.lua:85: attempt to index local 'color' (a nil value)` out of `LootFrame_OnShow` —
/// on the director's screen, on an ordinary corpse.
///
/// The window must therefore come up clean, paint the row, and paint it in the cache-miss colour —
/// `ITEM_QUALITY_COLORS[-1]`, the row `UIParent.lua`'s `for i = -1, 6` exists to provide.
#[test]
fn loot_row_awaiting_its_template_opens_clean() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");

    // One row, in flight: the wire gave us the icon (it rides the display id) and the stack size,
    // and nothing else yet.
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

    // The binding answered the reference's sentinels rather than nils, and the quality one indexes
    // `ITEM_QUALITY_COLORS` — which is the whole of what `LootFrame.lua:82` needs to be true.
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

    // And line 85 — the line that raised — actually ran: the row text wears the cache-miss colour,
    // read back off the FontString rather than off the table. `ITEM_QUALITY_COLORS[-1]` is Common
    // (`GetItemQualityColor`'s clamp is unsigned, so -1 takes the same branch as 7-and-up), not the
    // Poor it used to be here — 1805 corrects 1199 on that.
    let painted: (f64, f64, f64) = s
        .eval("local r, g, b = LootButton1Text:GetTextColor()\nreturn r, g, b")
        .unwrap();
    let miss: (f64, f64, f64) = s
        .eval("local r, g, b = GetItemQualityColor(-1)\nreturn r, g, b")
        .unwrap();
    assert_eq!(miss, (1.0, 1.0, 1.0), "the cache-miss row is Common/white");
    // (f32 region storage vs the f64 the binding computes — compare, don't equate.)
    assert!(
        (painted.0 - miss.0).abs() < 1e-6
            && (painted.1 - miss.1).abs() < 1e-6
            && (painted.2 - miss.2).abs() < 1e-6,
        "LootFrame_Update:85 must paint the -1 colour, got {painted:?} want {miss:?}"
    );

    // The template lands and the window is reopened: the row reads its name in its own quality's
    // colour, so the sentinel state poisons nothing downstream.
    //
    // It is a REOPEN and not a repaint on purpose. The app fires `LOOT_UPDATE` when an open
    // window's content changes, and `LOOT_UPDATE` is not a 1.12 event — it appears nowhere in the
    // reference's FrameXML and `LootFrame_OnLoad` does not register it — so nothing in the stock
    // file listens. What the real client does when a template arrives mid-window is still open
    // (the arrival callback `0x4c2ac0` is recorded as clearing the pending flag and nothing else);
    // this test deliberately asserts only what is settled.
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

/// **A `<LootButton>` wears `ItemButtonTemplate`'s three state textures, and the highlight tracks
/// the cursor.** The loader's Button leg used to gate on the two tags `Button`/`CheckButton`, so
/// the stock rows — whose tag is `LootButton` — were built with no `<NormalTexture>`, no
/// `<PushedTexture>` and no `<HighlightTexture>` at all: no Quickslot border on the icons, and
/// nothing to light under the mouse. The gate is wrong about the reference: `CLootButton`'s
/// geometry vtable differs from `CSimpleButton`'s in exactly one slot — the destructor thunk — and
/// `LoadXML` is not it, so `0x7788c0` parses a `<LootButton>` element verbatim.
///
/// Asserted through the ENGINE's hover path rather than off the state: what a player sees is the
/// emitted quad, and the quad is what was missing.
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

    // The inherited template's own art reached every visible row.
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

    // The highlight is not on screen until the cursor is on a row.
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

    // Hover the second row: exactly one highlight, over THAT row's icon.
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

    // And it leaves with the cursor.
    super::test_ui::unhover(&mut s);
    s.resolve();
    assert!(
        hilite(&s.extract()).is_empty(),
        "the highlight is not latched: it goes out when the cursor leaves"
    );
}
