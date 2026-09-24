//! The **merchant window** driven end-to-end, engine-only (no Bevy): the stock
//! `Interface\FrameXML\MerchantFrame.xml` (+ `GameTooltip.xml` for the hover chain) loaded behind
//! `UIParent.xml` and fed synthetic stock — the phase-4 vendor arc's machine checks (decision
//! 0081/0084). Split from `panel_tests` (which keeps gossip + the slot manager itself) along the
//! folder's one-file-per-window convention.

use benilla_ui::script::{
    ContainerState, DressUpIntent, ExtractedQuad, ItemStatsHead, MerchantItem, MerchantState,
    QuadContent, ScriptValue, SoundRequest, UiScript,
};

use super::test_ui::{bag_open, load_ui as load_xml, BAG_UI};

/// Put a bag carrying `num_slots` empty slots in bag `id` — the fixture the three bag tests below
/// need, and one they did NOT need before 1751's swap. The reference's `OpenBag` opens nothing for
/// a container whose `GetContainerNumSlots` is 0 (ContainerFrame.lua l.144-165); our five
/// hand-authored windows showed either way, so these tests used to name no container at all.
fn equip_bag(s: &mut UiScript, id: i64, name: &str, num_slots: u32) {
    s.set_container(
        id,
        Some(ContainerState {
            name: Some(name.into()),
            num_slots,
            slots: std::collections::HashMap::new(),
        }),
    );
}

/// Find a bare frame's own rect via its `QuadContent::Frame` entry (every frame emits one, at its
/// resolved rect, whether or not it paints anything itself — `UiScript::extract`'s doc).
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

/// Load the stock `Interface\FrameXML\MerchantFrame.xml` behind `UIParent.xml`
/// into a bare engine and drive it with a synthetic 2-item stock + a purse — the whole phase-4
/// chain minus Bevy, now over the UIPanel slot manager: the
/// hidden→shown lifecycle on MERCHANT_SHOW goes through ShowUIPanel (landing at the left slot),
/// both rows' icons + a price + the money line rendering, a row click queuing the right buy
/// intent, and MERCHANT_CLOSED hiding it through HideUIPanel, vacating the left slot.
#[test]
fn shipped_merchant_frame_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The named virtual Font objects the re-skinned rows/title inherit through — loaded first at
    // runtime (ui_script's shipped list) so `inherits="GameFontNormalSmall"` resolves here too.
    for f in super::test_ui::MERCHANT_UI {
        load_xml(&s, f);
    }
    // The STOCK file's frame census (1751). Ours materialized 63 — window, twelve rows, close, the
    // three-slot coin kit's 39 slots, the buyback slot, and the repair/page/tab pairs. The reference's
    // shape is different in kind, not just in count: every row is an inert container PLUS a
    // `$parentItemButton` that carries `ItemButtonTemplate`'s own children, and the prices are
    // `SmallMoneyFrameTemplate` instances rather than our `<prefix>Coin1..3` chain. A count is a
    // fingerprint of a file; this one is the reference's.
    assert_eq!(
        load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml"),
        90,
        "the stock file's own shape — see the census note above"
    );

    // Hidden by default: no vendor icon on screen.
    s.resolve();
    let has_icon = |quads: &[ExtractedQuad], needle: &str| {
        quads.iter().any(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
            })
    };
    assert!(
        !has_icon(&s.extract(), "INV_Drink_18"),
        "merchant window starts hidden"
    );

    // The app's feed: a purse + a two-item stock (one unlimited, one finite).
    s.set_money(12_345); // 1g 23s 45c
    s.set_merchant(Some(MerchantState {
        items: vec![
            MerchantItem {
                name: Some("Refreshing Spring Water".into()),
                texture: Some("Interface\\Icons\\INV_Drink_18".into()),
                price: 25,
                quantity: 1,
                num_available: -1,
                item_id: 159,
                stats: None,
                link: None,
                max_stack: Some(1),
            },
            MerchantItem {
                name: Some("Linen Bandage".into()),
                texture: Some("Interface\\Icons\\INV_Misc_Bandage_01".into()),
                price: 100,
                quantity: 1,
                num_available: 5,
                item_id: 1251,
                stats: None,
                link: None,
                max_stack: Some(1),
            },
        ],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The window is shown (ShowUIPanel put it on the left slot), both row icons rendered, a price
    // + the purse line painted.
    assert!(s.eval::<bool>("return MerchantFrame:IsVisible()").unwrap());
    s.resolve();
    let quads = s.extract();
    assert!(has_icon(&quads, "INV_Drink_18"), "row 1 icon visible");
    assert!(
        has_icon(&quads, "INV_Misc_Bandage_01"),
        "row 2 icon visible"
    );
    let has_text = |t: &str| {
        quads
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(x), .. } if x == t))
    };
    // Prices + purse now render as coin icons (Interface\MoneyFrame\UI-MoneyIcons) + numbers, the
    // real SmallMoneyFrame look, not "Xg Ys Zc" text. Row 1 costs 25c → the number "25" + a copper
    // coin; the purse (12345 = 1g 23s 45c) shows "1"/"23"/"45" over gold/silver/copper coins.
    assert!(has_icon(&quads, "UI-MoneyIcons"), "coin icons render");
    assert!(has_text("25"), "row 1 price shows the copper count '25'");
    assert!(
        has_text("23") && has_text("45"),
        "purse shows its silver/copper counts"
    );

    // Item 4: every one of the 10 slots always renders its plate art — the empty-slot socket + the
    // dark label plate — whether the row is filled or not. The 2 filled rows keep the socket
    // full-bright; the 8 empty rows dim it to 0.4 (ref MerchantFrame.lua l.108/115).
    let socket_colors: Vec<[f32; 4]> = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture {
                path: Some(p),
                color,
                ..
            } if p.contains("UI-EmptySlot") => Some(color.unwrap_or([1.0; 4])),
            _ => None,
        })
        .collect();
    // 10 merchant rows + the buyback slot (rows 11/12 are hidden on the merchant tab).
    assert_eq!(
        socket_colors.len(),
        11,
        "10 merchant rows + the buyback slot render their socket plate"
    );
    // THREE, not two, and the third is the buyback slot. Our file dimmed an empty buyback socket
    // with the empty rows; the reference's `MerchantFrame_UpdateBuybackInfo` does not run the
    // empty-row arm on it at all, so it keeps the bright plate whether or not a sold item is
    // there. Same census, one socket moved sides.
    assert_eq!(
        socket_colors.iter().filter(|c| c[0] > 0.9).count(),
        3,
        "the 2 filled rows and the buyback slot keep the socket full-bright"
    );
    assert_eq!(
        socket_colors
            .iter()
            .filter(|c| (c[0] - 0.4).abs() < 0.01)
            .count(),
        8,
        "the 8 empty rows dim the socket to 0.4 (ref MerchantFrame.lua l.115)"
    );
    assert_eq!(
        quads
            .iter()
            .filter(
                |q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("UI-Merchant-LabelSlots"))
            )
            .count(),
        11,
        "10 row label plates + the buyback slot's render"
    );

    // The slot anchor actually applied: the window's rect top-left sits at (0, 664) — screen
    // height 768 minus the left slot's 104px drop (pin §4's extract-rect assertion). The re-skinned
    // window has no solid-colour fill (the real quadrant art is opaque), so it's found by its own
    // 384×512 frame quad rather than a background texture.
    let win = frame_rect(&quads, 384.0, 512.0);
    assert_eq!(
        (win.left, win.top),
        (0.0, 664.0),
        "merchant window landed at the left slot (TOPLEFT UIParent, 0, -104)"
    );

    // The four quadrant slabs ARE the window art (ref-MerchantFrame.xml l.146-176): 256-wide left
    // halves, 128-wide right halves, each 256 tall, pinned to their corner, each sampling its whole
    // texture (no TexCoords — the ref uses those only on the bottom-border/repair art we omit).
    let quad_of = |needle: &str| {
        quads
            .iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                        if p.contains(needle))
            })
            .unwrap_or_else(|| panic!("no texture quad for {needle}"))
    };
    let quad_rect = |needle: &str, w: f32, h: f32| {
        let q = quad_of(needle);
        assert!(
            matches!(
                &q.content,
                QuadContent::Texture {
                    tex_coords: None,
                    ..
                }
            ),
            "{needle} quadrant samples its whole texture (no TexCoords)"
        );
        let r = q.rect.unwrap_or_else(|| panic!("no rect for {needle}"));
        assert!(
            (r.width() - w).abs() < 0.5 && (r.height() - h).abs() < 0.5,
            "{needle} is {w}×{h}, got {}×{}",
            r.width(),
            r.height()
        );
        r
    };
    let tl = quad_rect("UI-Merchant-TopLeft", 256.0, 256.0);
    assert_eq!(
        (tl.left, tl.top),
        (win.left, win.top),
        "TopLeft quadrant pinned to the window's TOPLEFT"
    );
    let tr = quad_rect("UI-Merchant-TopRight", 128.0, 256.0);
    assert_eq!(
        (tr.right, tr.top),
        (win.right, win.top),
        "TopRight quadrant pinned to the window's TOPRIGHT"
    );
    let bl = quad_rect("UI-Merchant-BotLeft", 256.0, 256.0);
    assert_eq!(
        (bl.left, bl.bottom),
        (win.left, win.bottom),
        "BotLeft quadrant pinned to the window's BOTTOMLEFT"
    );
    let br = quad_rect("UI-Merchant-BotRight", 128.0, 256.0);
    assert_eq!(
        (br.right, br.bottom),
        (win.right, win.bottom),
        "BotRight quadrant pinned to the window's BOTTOMRIGHT"
    );

    let tex_rect = |needle: &str| {
        quads
            .iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                        if p.contains(needle))
            })
            .and_then(|q| q.rect)
            .unwrap_or_else(|| panic!("no texture quad for {needle}"))
    };
    let text_rect = |t: &str| {
        quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Text { text: Some(x), .. } if x == t))
            .and_then(|q| q.rect)
            .unwrap_or_else(|| panic!("no text quad for {t:?}"))
    };

    // The icon sits at the row-left as a ~37px square — NOT stretched across the whole row (the
    // smeared-merchant regression this whole change fixes).
    let icon_rect = tex_rect("INV_Drink_18");
    assert!(
        (icon_rect.width() - 37.0).abs() < 0.5 && (icon_rect.height() - 37.0).abs() < 0.5,
        "row-left icon is a 37px square, got {}×{}",
        icon_rect.width(),
        icon_rect.height()
    );

    // Name (upper) and price coins (lower) stack the real name-over-money way (ref l.34/99-106): the
    // name FontString's box sits above the price number, so their rects don't overlap. (The engine
    // quad carries the full name string — word wrapping into lines is the app-side ui_text pass.)
    let name_rect = text_rect("Refreshing Spring Water");
    let price_rect = text_rect("25");
    // Rects are y-up (top > bottom): the name's interval sits entirely above the price's.
    let overlap = name_rect.left < price_rect.right
        && price_rect.left < name_rect.right
        && name_rect.bottom < price_rect.top
        && price_rect.bottom < name_rect.top;
    assert!(
        !overlap,
        "name {name_rect:?} must not overlap price {price_rect:?}"
    );
    assert!(
        name_rect.bottom >= price_rect.top,
        "name {name_rect:?} sits above price {price_rect:?}"
    );

    // RIGHT-click row 1 → BuyMerchantItem(1); LEFT-click does NOT buy (pickup pending the cursor
    // arc). The icon center lies inside the row button.
    let (cx, cy) = (
        (icon_rect.left + icon_rect.right) * 0.5,
        (icon_rect.bottom + icon_rect.top) * 0.5,
    );
    s.mouse_button(cx, cy, "LeftButton", true);
    s.mouse_button(cx, cy, "LeftButton", false);
    assert!(
        s.take_merchant_buys().is_empty(),
        "left-click does not buy (pickup pending the cursor arc)"
    );
    s.mouse_button(cx, cy, "RightButton", true);
    s.mouse_button(cx, cy, "RightButton", false);
    assert_eq!(s.take_merchant_buys(), vec![(1, 1)]);

    // MERCHANT_CLOSED hides the window through HideUIPanel — the icons go away and the left slot
    // vacates.
    s.fire_event("MERCHANT_CLOSED", vec![]);
    s.resolve();
    assert!(
        !has_icon(&s.extract(), "INV_Drink_18"),
        "MERCHANT_CLOSED hides the window"
    );
    assert!(
        s.eval::<bool>("return GetLeftFrame() == nil").unwrap(),
        "HideUIPanel vacated the left slot"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The vendor window's open/close kits — the window-sound convention. The real
/// MerchantFrame.xml frame Scripts play igCharacterInfoOpen on OnShow (l.721) and igCharacterInfoClose
/// on OnHide (l.714); MERCHANT_SHOW → ShowUIPanel → Show() fires OnShow, MERCHANT_CLOSED → HideUIPanel
/// → Hide() fires OnHide. Nothing queues at load (the frame is authored hidden="true").
#[test]
fn merchant_show_hide_plays_open_and_close_kits() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::MERCHANT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");

    // Hidden at load: no open sound (never transitions on startup).
    assert!(
        s.take_sounds().is_empty(),
        "no sound at load (never transitions)"
    );

    s.set_money(0);
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoOpen".into())],
        "opening the vendor window plays igCharacterInfoOpen"
    );

    s.fire_event("MERCHANT_CLOSED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoClose".into())],
        "closing the vendor window plays igCharacterInfoClose"
    );
}

/// With the bags loaded, opening the vendor also opens them (the real MerchantFrame_OnShow →
/// OpenBackpack, decision 0095; ALL equipped bags since decision 0561 — here only the backpack is
/// carried), so the backpack kit plays ALONGSIDE the panel kit — the "two sounds go together" the
/// director heard. On close, CloseBackpack hides the bag it opened, so both close kits play. The
/// bag window's own OnShow/OnHide fire first (OpenBackpack shows it before the panel sound), so the
/// backpack kit leads each pair.
///
/// **What 1751's swap changed, and what it did not.** The behaviour is the same and the sound
/// pairs are unchanged; the window is now the reference's own recycled `ContainerFrame` rather than
/// `BenillaBagFrame`, so the open assertion asks `IsBagOpen(0)` instead of naming a frame, and the
/// two bag kits come from `ContainerFrame_OnShow`/`_OnHide`'s own `PlaySound` calls
/// (ContainerFrame.lua l.140 / l.120). The seeded backpack is new and load-bearing — see
/// [`equip_bag`].
#[test]
fn vendor_open_opens_the_backpack_and_layers_the_sound() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // `BAG_UI` already carries GameTooltip.xml, so the app's tooltip-before-merchant order holds.
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);
    equip_bag(&mut s, 0, "Backpack", 16);
    let _ = s.take_sounds(); // ignore anything from load (frames are hidden; nothing should)

    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![
            SoundRequest::KitName("igBackPackOpen".into()),
            SoundRequest::KitName("igCharacterInfoOpen".into()),
        ],
        "vendor open opens the backpack (bag kit) then plays its own panel kit"
    );
    assert!(bag_open(&s, 0), "the backpack is open alongside the vendor");

    s.fire_event("MERCHANT_CLOSED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![
            SoundRequest::KitName("igBackPackClose".into()),
            SoundRequest::KitName("igCharacterInfoClose".into()),
        ],
        "vendor close closes the backpack it opened, both close kits play"
    );
    assert!(!bag_open(&s, 0), "…and the window goes with it");
}

/// A backpack the player already had open is NOT closed when the vendor closes: opening the vendor
/// over an already-open bag plays only the panel kit, and closing plays only the panel close kit —
/// the bag stays.
///
/// The guard is a **was-open memory**, and 1751 left two spellings of it in the tree: the
/// reference's own single `ContainerFrame1.backpackWasOpen` flag (ContainerFrame.lua l.187-202) and
/// `UiPanels.xml`'s bag-id-keyed `BENILLA_BAG_WAS_OPEN`, which 0561's all-bags `OpenBackpack` needs
/// because the windows are recycled now (frame N is a different bag every time it opens). Whichever
/// body the manifest leaves live, they spell bag 0's case identically — so this test pins the
/// PROPERTY and stays honest either way. Which one is actually live is the next test's subject,
/// not this one's.
#[test]
fn vendor_leaves_an_already_open_backpack_alone() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);
    equip_bag(&mut s, 0, "Backpack", 16);

    // Open the bag first (the 'B' toggle), then the vendor over it.
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    assert!(bag_open(&s, 0), "fixture: the player's own bag is up first");
    let _ = s.take_sounds();
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoOpen".into())],
        "bag already open → only the panel kit on vendor open"
    );

    s.fire_event("MERCHANT_CLOSED", vec![]);
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoClose".into())],
        "the pre-opened bag is left open → only the panel close kit"
    );
    assert!(
        bag_open(&s, 0),
        "the player's own open bag survives the vendor closing"
    );
}

/// The all-bags divergence (director's call): the vendor opens EVERY equipped bag
/// with it — not only the backpack like the ref's OpenBackpack — and closes every bag it opened.
/// Backpack + one equipped bag in slot 2 (bags 1/3/4 unequipped → no window): MERCHANT_SHOW opens
/// both windows, MERCHANT_CLOSED closes both.
///
/// This is also the test that decides WHOSE `OpenBackpack` is live. 1751 keeps 0561 by shadowing:
/// `UiPanels.xml` redefines `OpenBackpack`/`CloseBackpack`/`CloseAllBags` over the sourced
/// `ContainerFrame.lua`'s, and its body now loops the reference's `OpenBag`/`CloseBag` rather than
/// five windows by name — so the "unequipped slot" arm below is the reference's own `size > 0` test
/// rather than ours. A shadow only wins if it loads LATER (`reference_ui`'s rule: order decides,
/// the manifest is the order), so this test is exactly the falsifier for that ordering: with the
/// reference's own body live, bag 2 never opens and only the backpack does.
#[test]
fn vendor_opens_and_closes_all_equipped_bags() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);
    equip_bag(&mut s, 0, "Backpack", 16);
    equip_bag(&mut s, 2, "Small Pouch", 6);

    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(bag_open(&s, 0), "backpack opens");
    assert!(bag_open(&s, 2), "the equipped bag opens with the vendor");
    assert!(
        !bag_open(&s, 1) && !bag_open(&s, 3) && !bag_open(&s, 4),
        "an unequipped slot has no container, so nothing opens for it"
    );

    s.fire_event("MERCHANT_CLOSED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !bag_open(&s, 0) && !bag_open(&s, 2),
        "every bag the vendor opened closes with it"
    );
}

/// Switching vendors (right-click vendor B while vendor A's window is open) is a real close+open —
/// the client's ShowUIPanel early-returns when the frame is visible, so the open kit only re-plays
/// after a hide. The feed fires MERCHANT_CLOSED then MERCHANT_SHOW; this drives that
/// exact sequence over the shipped XML and asserts BOTH the close-then-open kit order AND that the
/// MERCHANT_CLOSED's OnHide queued a close intent — the intent the feed then consumes so the drain
/// doesn't clear the vendor it just re-opened to.
#[test]
fn merchant_switch_plays_close_then_open_and_queues_the_consumable_close() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::MERCHANT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");

    // Vendor A open.
    s.set_money(0);
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    let _ = s.take_sounds();
    let _ = s.take_merchant_close();

    // The switch to vendor B — the feed's close+open sequence.
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_CLOSED", vec![]);
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![
            SoundRequest::KitName("igCharacterInfoClose".into()),
            SoundRequest::KitName("igCharacterInfoOpen".into()),
        ],
        "switching vendors plays the close then the open kit"
    );
    assert!(
        s.take_merchant_close(),
        "the switch's MERCHANT_CLOSED queued a CloseMerchant intent — the feed must consume it so \
         the re-opened vendor is not cleared by the drain"
    );
}

/// The vendor hover chain over the shipped XML (MerchantFrame + GameTooltip): the row highlight is
/// scoped to the 37px icon (the real ItemButtonTemplate's own-button glow — never the whole 153px
/// row), the tooltip anchors at the icon's TOPRIGHT (the real client's owner is the 37px
/// ItemButton; ours walks the ANCHOR_RIGHT offset back), and `SetMerchantItem` renders the real
/// item-tooltip stat head — quality-coloured name, slot|type, damage|speed + dps, armor, block —
/// with no buy-price line (the price is on the row). Leaving the row hides it all.
#[test]
fn shipped_merchant_hover_scopes_highlight_and_anchors_item_tooltip() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The stock tooltip declares no size: it sizes from its lines through the font engine, as
    // the client's does (1968) — a harness that reads its rect needs one; the fixed-width
    // font is that engine here.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    for f in super::test_ui::MERCHANT_UI {
        load_xml(&s, f);
    }
    // The quality→colour table is `ITEM_QUALITY_COLORS`, and Fonts.xml is its home — this used
    // to load the whole loot window for `BENILLA_LOOT_QUALITY_COLORS`, an alias our own
    // LootFrame.xml set to that same table. 1751 took that window to the chain and the alias went
    // with it (GameTooltip.xml reads the real name now), so the dependency is gone. At runtime
    // every FrameXML file loads before any hover fires, so load it here too — with the dropdown
    // kit its GroupLootDropDown initializes against at load (benilla.toc l.64 vs 383).
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");

    s.set_merchant(Some(MerchantState {
        items: vec![
            MerchantItem {
                name: Some("Vendor Blade".into()),
                texture: Some("Interface\\Icons\\INV_Sword_04".into()),
                price: 8000,
                quantity: 1,
                num_available: -1,
                item_id: 2131,
                // An uncommon main-hand sword: 5.0–9.0 physical at 2600ms.
                stats: Some(ItemStatsHead {
                    quality: 2,
                    inventory_type: 21,
                    class: 2,
                    subclass: 7,
                    dmg_min: 5.0,
                    dmg_max: 9.0,
                    dmg_type: 0,
                    delay_ms: 2600,
                    armor: 0,
                    block: 0,
                    sell_price: 0,
                }),
                link: None,
                max_stack: Some(1),
            },
            MerchantItem {
                name: Some("Chipped Buckler".into()),
                texture: Some("Interface\\Icons\\INV_Shield_09".into()),
                price: 124,
                quantity: 1,
                num_available: -1,
                item_id: 2129,
                // A common shield: armor + block, no damage.
                stats: Some(ItemStatsHead {
                    quality: 1,
                    inventory_type: 14,
                    class: 4,
                    subclass: 6,
                    dmg_min: 0.0,
                    dmg_max: 0.0,
                    dmg_type: 0,
                    delay_ms: 0,
                    armor: 85,
                    block: 1,
                    sell_price: 0,
                }),
                link: None,
                max_stack: Some(1),
            },
        ],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();

    let rect_of_tex = |quads: &[ExtractedQuad], needle: &str| {
        quads
            .iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
            })
            .and_then(|q| q.rect)
            .unwrap_or_else(|| panic!("no texture quad for {needle}"))
    };
    let has_text = |quads: &[ExtractedQuad], t: &str| {
        quads
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(x), .. } if x == t))
    };

    // Before any hover: no highlight, no tooltip lines.
    let quads = s.extract();
    assert!(
        !quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("ButtonHilight-Square"))
        }),
        "no highlight before hover"
    );
    assert!(!has_text(&quads, "Main Hand"), "no tooltip before hover");
    let sword_icon = rect_of_tex(&quads, "INV_Sword_04");

    // Hover the ICON, and that is the whole change 1751 made here. Our `MerchantFrame.xml` made
    // the WHOLE ROW take the mouse — this hovered 60px right of the icon, over the name plate,
    // and expected the tooltip. The reference splits every row into an inert container plus a
    // `$parentItemButton` the size of the icon, which takes the mouse on its behalf; the twelve
    // `MerchantItem<N>` entries in the flag gate's KNOWN list said exactly that, and they retired
    // with our file.
    //
    // So on the reference's window, hovering a vendor row's NAME shows nothing. That is not a
    // regression to fix — it is what the client does.
    let (hx, hy) = (
        (sword_icon.left + sword_icon.right) * 0.5,
        (sword_icon.bottom + sword_icon.top) * 0.5,
    );
    s.mouse_move(hx, hy);
    assert!(s.errors().is_empty(), "OnEnter errors: {:?}", s.errors());
    s.resolve();
    let quads = s.extract();
    let hl = rect_of_tex(&quads, "ButtonHilight-Square");
    assert!(
        (hl.width() - 37.0).abs() < 0.5 && (hl.height() - 37.0).abs() < 0.5,
        "highlight is the 37px icon square, got {}×{}",
        hl.width(),
        hl.height()
    );
    assert_eq!(
        (hl.left, hl.top),
        (sword_icon.left, sword_icon.top),
        "highlight sits on the icon, not the row"
    );

    // The tooltip: the real item stat head, name coloured by quality (uncommon green — the
    // BENILLA_LOOT_QUALITY_COLORS[2] the loot window shares).
    for line in [
        "Vendor Blade",
        "Main Hand",
        "5 - 9 Damage",
        "Speed 2.60",
        "(2.7 damage per second)",
    ] {
        assert!(has_text(&quads, line), "tooltip line {line:?} missing");
    }
    // The type cell ("Sword") is deliberately NOT here. This row renders through the stat-head
    // FALLBACK — the shape a hover takes only while the full template view is still in flight —
    // and the type word is `ItemSubClass.dbc`'s DisplayName, which only the app can resolve. The
    // head carries `class`/`subclass` numbers and no name, so the cell stands empty rather than
    // being composed from a table in the engine (the renderer's hand-typed copy of that DBC is
    // gone — decision 2080's shape). A hover whose template HAS landed takes `view_of` and prints
    // it; that path is pinned in `benilla-ui`'s own item-law tests.
    assert!(!has_text(&quads, "Sword"), "the head carries no type word");
    // Two "Vendor Blade" quads exist — the merchant row's own gold name and the tooltip's header
    // line; the tooltip's is the quality-green one.
    let name_colors: Vec<[f32; 4]> = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Text {
                text: Some(t),
                color: Some(c),
                ..
            } if t == "Vendor Blade" => Some(*c),
            _ => None,
        })
        .collect();
    assert!(
        name_colors
            .iter()
            .any(|c| (c[0] - 0.12).abs() < 0.01 && (c[1] - 1.0).abs() < 0.01 && c[2].abs() < 0.01),
        "the tooltip name line is quality-green, got {name_colors:?}"
    );
    assert!(
        !quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Text { text: Some(t), .. }
                if t.starts_with("Buy Price"))
        }),
        "the tooltip carries no buy-price line"
    );

    // Anchored at the icon: the tooltip's BOTTOMLEFT sits on the icon's TOPRIGHT (the −116 offset
    // walks ANCHOR_RIGHT from the 153px row back to the 37px icon). The corner (an anchor-law
    // fact, independent of the frame's own size) is what's under test — not the size itself, so
    // read it straight off the real global by name (GameTooltip is the frame's actual Lua name
    // since decision 0274) instead of hunting quads by a guessed frame size.
    let (tip_left, tip_bottom): (f32, f32) = s
        .eval("return GameTooltip:GetLeft(), GameTooltip:GetBottom()")
        .unwrap();
    assert_eq!(
        (tip_left, tip_bottom),
        (sword_icon.right, sword_icon.top),
        "tooltip BOTTOMLEFT sits on the icon's TOPRIGHT"
    );

    // Hover row 2 (the shield): armor/block lines, the slot|type pair, row 1's lines gone.
    let shield_icon = rect_of_tex(&quads, "INV_Shield_09");
    s.mouse_move(
        (shield_icon.left + shield_icon.right) * 0.5,
        (shield_icon.bottom + shield_icon.top) * 0.5,
    );
    assert!(
        s.errors().is_empty(),
        "row-2 OnEnter errors: {:?}",
        s.errors()
    );
    s.resolve();
    let quads = s.extract();
    for line in ["Chipped Buckler", "Off Hand", "85 Armor", "1 Block"] {
        assert!(
            has_text(&quads, line),
            "shield tooltip line {line:?} missing"
        );
    }
    assert!(!has_text(&quads, "Shield"), "the head carries no type word");
    assert!(!has_text(&quads, "Main Hand"), "row 1's tooltip cleared");

    // Leave the window entirely: tooltip + highlight gone.
    s.mouse_move(1000.0, 10.0);
    s.resolve();
    let quads = s.extract();
    assert!(!has_text(&quads, "Off Hand"), "tooltip hidden on leave");
    assert!(
        !quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("ButtonHilight-Square"))
        }),
        "highlight gone on leave"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── `money_display_shrinks_to_content_and_stays_flush` RETIRED (1751) ────────────────────────
//
// It pinned `BenillaMoney_Set`'s geometry inside OUR `MerchantFrame.xml`: price groups left-flush
// at `rowLeft + 46`, the purse right-flush at window-right −53, denominations packed with the real
// 4px `MONEY_BUTTON_SPACING`. The 46 was a director call — 2px of breathing room off the plate
// border where the reference quotes 44 — and it was a fact about a file we no longer ship.
//
// The reference's vendor rows use `SmallMoneyFrameTemplate` and `MoneyFrame_Update`, a different
// kit with its own geometry. **The rules the test protected are not lost**: the shrink, the
// collapse and the per-type `showSmallerCoins`/`fixedWidth` behaviour are `money_frame_tests.rs`'s
// subject, against the kit that now draws these rows. What is gone is the merchant-specific pixel
// geometry, which had nothing left to describe.
//
// **The vendor window's coin columns therefore look different now** — the reference's spacing, not
// our approved 2px indent. Said here rather than left inside a changed constant.

/// The merchant/buyback tab pair drives the two pages (ref MerchantFrame_Update l.57-64 +
/// UpdateMerchantInfo/UpdateBuybackInfo): the merchant page shows the single most-recent buyback
/// slot + the repair pair (enabled iff there's damage to pay for); the buyback tab retitles the
/// window, fills the rows from GetBuybackItemInfo, and hides every merchant-only piece. Clicks
/// queue the BuybackItem/RepairAllItems intents the app drains.
#[test]
fn merchant_tabs_drive_buyback_page_and_repair_pair() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::MERCHANT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(500);

    let buyback_item = |name: &str, price: u32| MerchantItem {
        name: Some(name.into()),
        texture: Some("Interface\\Icons\\INV_Misc_Cape_01".into()),
        price,
        quantity: 1,
        stats: Some(ItemStatsHead {
            quality: 1,
            ..Default::default()
        }),
        ..Default::default()
    };
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
        buyback: vec![
            buyback_item("Bandit Cloak", 116),
            buyback_item("Cracked Sword", 20), // most recent sale — the merchant page's slot
        ],
        can_repair: true,
        repair_all_cost: 76,
    }));
    s.fire_event(
        "MERCHANT_SHOW",
        vec![ScriptValue::Str("Kurdram Stonehammer".into())],
    );
    assert!(s.errors().is_empty(), "show errors: {:?}", s.errors());

    // Merchant page: the buyback slot shows the MOST RECENT sale; the repair pair is up and the
    // all-button enabled (cost 76 > 0); the buyback page art is down.
    assert!(s
        .eval::<bool>("return MerchantBuyBackItemName:GetText() == 'Cracked Sword'")
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return MerchantRepairAllButton:IsShown() and \
             MerchantRepairAllButton:IsEnabled() ~= 0"
        )
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return not MerchantItem11:IsShown() and BuybackFrameTopLeft:IsShown() == nil"
        )
        .unwrap());

    // The slot click buys back the most recent sale (slot 2 of 2).
    // The stock row is a container plus a `$parentItemButton` that takes the mouse on its
    // behalf — the split the retired flag-gate entries used to record as our divergence.
    s.run("MerchantBuyBackItemItemButton:Click()").unwrap();
    assert_eq!(s.take_merchant_buybacks(), vec![2]);

    // Repair-all queues its intent.
    s.run("MerchantRepairAllButton:Click()").unwrap();
    assert!(s.take_repair_all());
    s.take_sounds();

    // Tab 2: the buyback page — retitled, rows off GetBuybackItemInfo, merchant-only pieces down,
    // buyback art up, tab 2 selected (its Active slices showing).
    s.run("MerchantFrameTab2:Click()").unwrap();
    assert!(s.errors().is_empty(), "tab errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return MerchantNameText:GetText() == 'Merchant Buyback'")
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return MerchantItem1Name:GetText() == 'Bandit Cloak' \
             and MerchantItem2Name:GetText() == 'Cracked Sword'",
        )
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return BuybackFrameTopLeft:IsShown() == 1 \
             and not MerchantBuyBackItem:IsShown() \
             and not MerchantRepairAllButton:IsShown()",
        )
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return MerchantFrameTab2LeftDisabled:IsShown() == 1 \
             and MerchantFrameTab1LeftDisabled:IsShown() == nil",
        )
        .unwrap());

    // A row click on the buyback page buys that slot back.
    s.run("MerchantItem1ItemButton:Click(\"LeftButton\")")
        .unwrap();
    assert_eq!(s.take_merchant_buybacks(), vec![1]);

    // Back to tab 1: the merchant row returns, the buyback art drops.
    s.run("MerchantFrameTab1:Click()").unwrap();
    assert!(s
        .eval::<bool>(
            "return MerchantItem1Name:GetText() == 'Refreshing Spring Water' \
             and BuybackFrameTopLeft:IsShown() == nil",
        )
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The two tabs fit their labels, from the reference's own `<OnShow>` — `PanelTemplates_TabResize(0)`
/// on `CharacterFrameTabButtonTemplate`: tab width = text + the two 20px end slices, middle slices
/// stretched to the text. Fixed 115px tabs looked wrong against the ref (director pass 2026-07-05):
/// too wide, and the −16 overlap lost its nestle gap.
///
/// **Driven through a SYNCHRONOUS measurer, with no round trip to pump** — the same correction 1848
/// made to the macro harness, arriving here with the template. This used to feed
/// two widths through `set_measured_text_unwrapped` and then tick, because our own template re-fit
/// from `OnUpdate` until the measure settled. The reference's file fits once, in `OnShow`; a client
/// whose measure is still pending at that moment could never size its tabs at all, and the app is
/// not such a client — it installs `AtlasMeasurer`. Modelling the async path here was modelling a
/// configuration the app does not have.
#[test]
fn merchant_tabs_fit_their_labels() {
    benilla_formats::wow_data_or_skip!();
    /// `2 * $parentLeft:GetWidth()` — the template's two 20-unit end slices.
    const SIDES: f64 = 40.0;
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    for f in super::test_ui::MERCHANT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![ScriptValue::Str("Vendor".into())]);
    s.resolve();

    // Tab width = text + 2×20 end slices; the middle slices carry exactly the text width.
    let (l1, l2, w1, w2, m1, m2): (f64, f64, f64, f64, f64, f64) = s
        .eval(
            "return MerchantFrameTab1Text:GetStringWidth(), MerchantFrameTab2Text:GetStringWidth(), \
             MerchantFrameTab1:GetWidth(), MerchantFrameTab2:GetWidth(), \
             MerchantFrameTab1MiddleDisabled:GetWidth(), MerchantFrameTab2Middle:GetWidth()",
        )
        .unwrap();
    assert!(l1 > 0.0 && l2 > 0.0, "both labels measured synchronously");
    assert_eq!(
        (w1, w2),
        (l1 + SIDES, l2 + SIDES),
        "text + 40, not the fixed 115"
    );
    assert_eq!((m1, m2), (l1, l2), "middle slices stretch to the text");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Hovering a buy-tab row arms the vendor **Buy** cursor (the grayed **UnableBuy** when you can't
/// afford the row), swaps to **Inspect** while Ctrl is held, and clears on leave — the whole reason
/// this window exists to a first-time visitor. Drives the stock row button's own `<OnEnter>` +
/// the frame's `OnUpdate` (a `tick`) exactly as the app does, since the coin is re-armed per frame.
#[test]
fn shipped_merchant_frame_arms_the_buy_cursor_on_hover() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::UiCursorMode;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::MERCHANT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");

    // A purse of 50c: row 1 (25c) is affordable, row 2 (100c) is not.
    s.set_money(50);
    s.set_merchant(Some(MerchantState {
        items: vec![
            MerchantItem {
                name: Some("Refreshing Spring Water".into()),
                texture: Some("Interface\\Icons\\INV_Drink_18".into()),
                price: 25,
                quantity: 1,
                num_available: -1,
                item_id: 159,
                stats: None,
                link: None,
                max_stack: Some(1),
            },
            MerchantItem {
                name: Some("Linen Bandage".into()),
                texture: Some("Interface\\Icons\\INV_Misc_Bandage_01".into()),
                price: 100,
                quantity: 1,
                num_available: 5,
                item_id: 1251,
                stats: None,
                link: None,
                max_stack: Some(1),
            },
        ],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    s.resolve();
    assert_eq!(s.ui_cursor(), None, "no override before any hover");

    // Row 1 hover: OnEnter remembers the row, the frame's OnUpdate arms the coin — affordable → Buy.
    // The stock `<OnEnter>` is inline on the row button, so it is fired the way the engine
    // fires it: `this` bound, then the compiled handler.
    s.run("this = MerchantItem1ItemButton MerchantItem1ItemButton:GetScript(\"OnEnter\")() this = nil")
        .unwrap();
    s.tick(0.016);
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    assert_eq!(
        s.ui_cursor(),
        Some(UiCursorMode::Buy),
        "the coin arms over an affordable vendor item"
    );

    // Ctrl held over the same row → the Inspect magnifier (the OnUpdate's Ctrl leg).
    s.set_modifiers(false, true, false);
    s.tick(0.016);
    assert_eq!(
        s.ui_cursor(),
        Some(UiCursorMode::Inspect),
        "Ctrl-hover shows the inspect cursor"
    );
    s.set_modifiers(false, false, false);

    // Leaving clears the override (OnLeave ResetCursor + the itemHover poll stops).
    s.run("this = MerchantItem1ItemButton MerchantItem1ItemButton:GetScript(\"OnLeave\")() this = nil")
        .unwrap();
    s.tick(0.016);
    assert_eq!(s.ui_cursor(), None, "leaving the row resets the cursor");

    // Row 2 hover: 100c against a 50c purse → the grayed UnableBuy.
    s.run("this = MerchantItem2ItemButton MerchantItem2ItemButton:GetScript(\"OnEnter\")() this = nil")
        .unwrap();
    s.tick(0.016);
    assert_eq!(
        s.ui_cursor(),
        Some(UiCursorMode::UnableBuy),
        "an unaffordable vendor item grays the coin"
    );

    // Closing the window drops the override even without an OnLeave (walked out of range, panel
    // displaced): the frame OnHide ResetCursors.
    s.fire_event("MERCHANT_CLOSED", vec![]);
    s.tick(0.016);
    assert_eq!(s.ui_cursor(), None, "closing the window resets the cursor");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Reproduce the director's report (2026-07-22): the trade RECIPIENT (read-only, partner) gold shows
/// "..." even for a single digit, while the byte-identical merchant purse renders fine. Drives the real
/// TradeFrame recipient coin trio through extract() and asserts the number quad is the digit, not the
/// ellipsis. If this FAILS, the bug is structural and reproduced here; if it PASSES, it is live-only.
#[test]
fn trade_recipient_money_renders_the_digit_not_ellipsis() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::MERCHANT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml"); // MerchantFrame:IsShown(), read on the slot click
                                                            // TradeFrame's money entry is the chain's own since 1882 — its OnLoad calls MoneyInputFrame_*.
    load_xml(&s, "Interface\\FrameXML\\MoneyInputFrame.lua");
    load_xml(&s, "Interface\\FrameXML\\MoneyInputFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\TradeFrame.xml");
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));

    let target = benilla_ui::script::TradeSideState {
        gold: 5, // a partner offering 5 copper → the recipient trio should show "5"
        ..Default::default()
    };
    s.set_trade(Some(benilla_ui::script::TradeState {
        target,
        ..Default::default()
    }));
    s.fire_event("TRADE_SHOW", vec![]);
    s.resolve();
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
    let quads = s.extract();

    let has = |t: &str| {
        quads
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(x), .. } if x == t))
    };
    let texts: Vec<&str> = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Text { text: Some(x), .. } => Some(x.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !has("..."),
        "recipient money truncated to '...' (the bug). all text quads: {texts:?}"
    );
    assert!(
        has("5"),
        "recipient money shows the digit. all text quads: {texts:?}"
    );
}

/// The vendor row's LEFT-button modifier fork (ref `MerchantItemButton_OnClick`,
/// MerchantFrame.lua l.301-306): CTRL previews the item in the dressing room, SHIFT
/// posts its link into an open chat edit box — both over `GetMerchantItemLink`, the
/// binding this arc added. Neither may buy: this window's click *is* a purchase, so a modified click
/// that fell through would spend the player's money.
///
/// The controls that must not change: a plain RIGHT-click still buys, and (the reference's own
/// right-button guard, l.332-333) a CTRL-held right-click buys nothing at all.
#[test]
fn ctrl_and_shift_on_a_vendor_row_preview_and_post_without_buying() {
    benilla_formats::wow_data_or_skip!();
    const WATER_LINK: &str = "|cffffffff|Hitem:159:0:0:0|h[Refreshing Spring Water]|h|r";
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::MERCHANT_UI {
        load_xml(&s, f);
    }
    for file in [
        r"Interface\FrameXML\UIParent.xml", // UIParent + UIParent.lua, the reference's own (1988)
        "ScrollTemplates.xml",              // our scroll kit + the placeholder icon
        "Interface\\FrameXML\\MerchantFrame.xml",
        "Interface\\FrameXML\\DressUpFrame.xml",
        "Interface\\FrameXML\\UIMenu.xml", // the kit ChatMenu/EmoteMenu/VoiceMacroMenu build from
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\ChatFrame.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        // (the panel templates, UiPanels and the dialog engine are MERCHANT_UI's already — a
        // second load of the stock money kit's frames trips its own global-named update)
        "Interface\\FrameXML\\FloatingChatFrame.xml",
    ] {
        load_xml(&s, file);
    }

    s.set_money(12_345);
    s.set_merchant(Some(MerchantState {
        items: vec![MerchantItem {
            name: Some("Refreshing Spring Water".into()),
            texture: Some("Interface\\Icons\\INV_Drink_18".into()),
            price: 25,
            quantity: 1,
            num_available: -1,
            item_id: 159,
            stats: None,
            // Fed exactly as `ui_merchant` builds it off the row's template answer.
            link: Some(WATER_LINK.into()),
            max_stack: Some(1),
        }],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    let quads = s.extract();
    let icon = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("INV_Drink_18"))
        })
        .and_then(|q| q.rect)
        .expect("no row icon quad");
    let (x, y) = (
        (icon.left + icon.right) * 0.5,
        (icon.bottom + icon.top) * 0.5,
    );

    // The control first: an unmodified right-click still buys row 1.
    s.mouse_button(x, y, "RightButton", true);
    s.mouse_button(x, y, "RightButton", false);
    assert_eq!(
        s.take_merchant_buys(),
        vec![(1, 1)],
        "an unmodified right-click still buys"
    );

    // The reference's right-button guard (l.332-333): CTRL-held, a right-click does nothing.
    s.set_modifiers(false, true, false);
    s.mouse_button(x, y, "RightButton", true);
    s.mouse_button(x, y, "RightButton", false);
    s.set_modifiers(false, false, false);
    assert!(
        s.take_merchant_buys().is_empty(),
        "a ctrl-held right-click must not buy (ref l.332-333)"
    );

    // SHIFT + LEFT with the chat edit box open → the link, no purchase.
    assert!(s.focus_editbox("ChatFrameEditBox"));
    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        WATER_LINK,
        "the vendor row's full escaped link landed in the chat box"
    );
    assert!(
        s.take_merchant_buys().is_empty(),
        "a shift-click must not also buy"
    );

    // CTRL + LEFT → the dressing room wearing it, no purchase. Last: opening the room takes a
    // UIPanel slot and can move the vendor window.
    s.set_modifiers(false, true, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.take_dressup_intents(),
        vec![DressUpIntent::Dress, DressUpIntent::TryOn(159)],
        "re-dress first, then try the vendor's item on"
    );
    assert!(
        s.take_merchant_buys().is_empty(),
        "a ctrl-click must not also buy"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
