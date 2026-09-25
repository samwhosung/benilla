//! The bank window, engine-only: the reference's own `BankFrame.xml` off the player's patch chain.
//! A bank bag is an ordinary `ContainerFrame` at container id `NUM_BAG_SLOTS + slot` (5..10), its
//! bag button's `GetID()`; `OpenBag` opens only a container that exists, so tests feed one at 5.

use benilla_ui::script::{
    BankState, ContainerSlot, ContainerState, ExtractedQuad, QuadContent, ScriptValue,
    SoundRequest, UiScript,
};

use super::test_ui::{bag_open, load_ui as load_xml, BAG_UI};

fn empty_bag(name: &str, num_slots: u32) -> Option<ContainerState> {
    Some(ContainerState {
        name: Some(name.into()),
        num_slots,
        slots: std::collections::HashMap::new(),
    })
}

/// [`BAG_UI`] carries the bank's whole chain, `BankFrame.xml` included; it needs client data.
fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    s
}

fn has_icon(quads: &[ExtractedQuad], needle: &str) -> bool {
    quads.iter().any(
        |q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle)),
    )
}

/// The title is `UnitName("npc")`: `BankFrame_OnEvent` reads no event argument (BankFrame.lua:127).
/// The bank does not open your bags: its OnShow plays `igMainMenuOpen` and nothing else.
#[test]
fn bankframe_opened_shows_and_sets_the_title() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();

    // The reference gives `BankFrameTitleText` no `text=`: it is empty until the bank opens.
    assert_eq!(
        s.eval::<String>("return BankFrameTitleText:GetText() or \"\"")
            .unwrap(),
        ""
    );
    assert!(!s.eval::<bool>("return BankFrame:IsVisible()").unwrap());

    let _ = s.take_sounds();

    s.set_money(0);
    s.set_container(0, empty_bag("Backpack", 16));
    s.set_bank(Some(BankState::default()));
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            name: Some("Grumnus Steelshaper".into()),
            ..Default::default()
        }),
    );
    s.fire_event("BANKFRAME_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(s.eval::<bool>("return BankFrame:IsVisible()").unwrap());
    assert_eq!(
        s.eval::<String>("return BankFrameTitleText:GetText()")
            .unwrap(),
        "Grumnus Steelshaper"
    );
    assert!(
        !bag_open(&s, 0),
        "the reference's bank leaves your bags alone (opening them is the vendor's, not the bank's)"
    );
    assert!(
        s.eval::<bool>("return GetLeftFrame() == BankFrame")
            .unwrap(),
        "ShowUIPanel landed the bank at the left slot"
    );
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igMainMenuOpen".into())],
        "the bank's own OnShow kit, and only it"
    );
}

/// The reference's OnHide order: `CloseBankBagFrames()`, `CloseBankFrame()`, then the close sound
/// (BankFrame.xml:521-526). A "popout" is a bank bag's `ContainerFrame`.
#[test]
fn bankframe_closed_queues_close_closes_open_popouts_and_plays_the_close_kit() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_money(0);
    // The first bank bag slot: container 5, `BankFrameBag1`'s `GetID()`.
    s.set_container(5, empty_bag("Bank Bag", 8));
    s.set_bank(Some(BankState::default()));
    s.fire_event("BANKFRAME_OPENED", vec![ScriptValue::Str("Banker".into())]);
    let _ = s.take_sounds();
    let _ = s.take_bank_close();

    // A bank bag left open when the bank closes: what `CloseBankBagFrames` exists for.
    s.run("OpenBag(5)").unwrap();
    assert!(bag_open(&s, 5), "the bank bag's window is up");
    let _ = s.take_sounds();

    s.fire_event("BANKFRAME_CLOSED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(
        !s.eval::<bool>("return BankFrame:IsVisible()").unwrap(),
        "BANKFRAME_CLOSED hides the window"
    );
    assert!(
        !bag_open(&s, 5),
        "closing the bank closes the bank bag it left open (CloseBankBagFrames)"
    );
    assert!(
        s.take_bank_close(),
        "OnHide queued CloseBankFrame() — the client-side close intent (no wire opcode)"
    );
    assert!(
        s.eval::<bool>("return GetLeftFrame() == nil").unwrap(),
        "HideUIPanel vacated the left slot"
    );
    let sounds = s.take_sounds();
    assert!(
        sounds.contains(&SoundRequest::KitName("igMainMenuClose".into())),
        "the close kit plays: {sounds:?}"
    );
}

#[test]
fn item_slot_paints_from_container_minus_one_and_repaints_on_playerbankslots_changed() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_money(0);
    s.set_bank(Some(BankState::default()));
    s.fire_event("BANKFRAME_OPENED", vec![]);
    s.resolve();
    assert!(
        !has_icon(&s.extract(), "INV_Misc_Gem_01"),
        "nothing painted before any container push"
    );

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        3,
        ContainerSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Gem_01".into()),
            count: 1,
            ..Default::default()
        },
    );
    s.set_container(
        -1,
        Some(ContainerState {
            name: Some("Bank".into()),
            num_slots: 24,
            slots,
        }),
    );
    s.fire_event("PLAYERBANKSLOTS_CHANGED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    assert!(
        has_icon(&s.extract(), "INV_Misc_Gem_01"),
        "slot 3's icon painted after PLAYERBANKSLOTS_CHANGED(3)"
    );

    // A stack-count change on the same slot repaints too.
    let mut slots2 = std::collections::HashMap::new();
    slots2.insert(
        3,
        ContainerSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Gem_01".into()),
            count: 5,
            ..Default::default()
        },
    );
    s.set_container(
        -1,
        Some(ContainerState {
            name: Some("Bank".into()),
            num_slots: 24,
            slots: slots2,
        }),
    );
    s.fire_event("PLAYERBANKSLOTS_CHANGED", vec![]);
    s.resolve();
    let quads = s.extract();
    assert!(
        quads
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "5")),
        "the repaint picks up the new stack count"
    );
}

/// `UpdateBagSlotStatus` tints an unpurchased bag button (1.0, 0.1, 0.1) (BankFrame.lua:96).
#[test]
fn bag_buttons_tint_by_purchase_count_and_texture_from_the_bank_bag_band() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_money(0);
    s.set_bank(Some(BankState {
        num_purchased: 2,
        next_cost: 100_000,
    }));
    // Bank bag slot 1 holds a bag: inventory id 64, where `BankFrameItemButton_OnUpdate` reads
    // the icon (`GetInventoryItemTexture`).
    let mut bags: benilla_ui::script::BankBagSlots = Default::default();
    bags[0] = Some(benilla_ui::script::InvSlotView {
        item_id: 4500,
        icon: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
        count: 1,
        equip_slots: vec![20, 21, 22, 23],
        ..Default::default()
    });
    s.set_bank_bag_slots(bags);
    s.fire_event("BANKFRAME_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    let quads = s.extract();

    // The bag bar's slots show the same empty-slot art, so each lookup is scoped to one button.
    let center = |name: &str| -> (f32, f32) {
        let (x, y): (f64, f64) = s
            .eval(&format!(
                "return ({name}:GetLeft() + {name}:GetRight()) / 2, \
                        ({name}:GetTop() + {name}:GetBottom()) / 2"
            ))
            .unwrap();
        (x as f32, y as f32)
    };
    let color_at = |cx: f32, cy: f32, path_needle: &str| -> Option<[f32; 4]> {
        quads.iter().find_map(|q| match &q.content {
            QuadContent::Texture {
                path: Some(p),
                color,
                ..
            } if p.contains(path_needle) => q
                .rect
                .filter(|r| r.left <= cx && cx <= r.right && r.bottom <= cy && cy <= r.top)
                .map(|_| color.unwrap_or([1.0; 4])),
            _ => None,
        })
    };
    let near = |c: [f32; 4], r: f32, g: f32, b: f32| {
        (c[0] - r).abs() < 0.01 && (c[1] - g).abs() < 0.01 && (c[2] - b).abs() < 0.15
    };

    let (x1, y1) = center("BankFrameBag1");
    let c1 = color_at(x1, y1, "INV_Misc_Bag_08").expect("button 1 shows the banked bag's icon");
    assert!(
        near(c1, 1.0, 1.0, 1.0),
        "button 1 is purchased: white {c1:?}"
    );

    // Button 2 is purchased and empty: `GetInventorySlotInfo("Bag2")`'s art, from
    // `PaperDollItemFrame.dbc`.
    let (x2, y2) = center("BankFrameBag2");
    let c2 = color_at(x2, y2, "UI-PaperDoll-Slot-Bag").expect("button 2 shows the empty-slot art");
    assert!(
        near(c2, 1.0, 1.0, 1.0),
        "button 2 is purchased: white {c2:?}"
    );

    for i in 3..=6 {
        let (x, y) = center(&format!("BankFrameBag{i}"));
        let c = color_at(x, y, "UI-PaperDoll-Slot-Bag")
            .unwrap_or_else(|| panic!("button {i} shows the empty-slot art"));
        assert!(
            near(c, 1.0, 0.1, 0.1),
            "button {i} is unpurchased: red {c:?}"
        );
    }
}

/// The purse is a `SmallMoneyFrameTemplate` (BankFrame.xml:501): a large amount must not truncate.
#[test]
fn the_purse_row_splits_a_large_amount_across_three_coin_buttons() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_bank(Some(BankState::default()));
    s.fire_event("BANKFRAME_OPENED", vec![]);
    // 98765g 43s 21c.
    s.set_money(987_654_321);
    s.fire_event("PLAYER_MONEY", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    for (coin, want) in [("Gold", "98765"), ("Silver", "43"), ("Copper", "21")] {
        assert_eq!(
            s.eval::<String>(&format!(
                "return BankFrameMoneyFrame{coin}ButtonText:GetText()"
            ))
            .unwrap(),
            want,
            "the {coin} button's digits"
        );
    }
}

/// `BankFramePurchaseButton` is declared `virtual="true"` inside `<Frames>` (BankFrame.xml:466),
/// and only a top-level element is a template, so the button exists and clicks. The row hides once
/// `GetNumBankSlots()` reports `full`.
#[test]
fn purchase_flow_shows_popup_queues_the_intent_and_the_row_hides_when_full() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_money(1_000_000);
    s.set_bank(Some(BankState {
        num_purchased: 2,
        next_cost: 100_000,
    }));
    s.fire_event("BANKFRAME_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return BankFramePurchaseInfo:IsShown()")
            .unwrap(),
        "the purchase row shows while unfilled"
    );
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "no popup before the click"
    );

    s.run("BankFramePurchaseButton:Click()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the click shows the confirm popup"
    );
    // `CONFIRM_BUY_BANK_SLOT`'s OnShow prints `BankFrame.nextSlotCost` (StaticPopup.lua:54-56),
    // which `UpdateBagSlotStatus` sets from `GetBankSlotCost`.
    assert!(
        s.eval::<bool>("return StaticPopup1MoneyFrame:IsShown()")
            .unwrap(),
        "the confirm shows the coin row"
    );
    assert_eq!(
        s.eval::<i64>("return BankFrame.nextSlotCost").unwrap(),
        100_000
    );
    assert!(!s.take_bank_purchase(), "nothing queued until accept");

    s.run("StaticPopup1Button1:Click()").unwrap(); // Yes
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.take_bank_purchase(),
        "accepting the popup queued PurchaseSlot()'s intent"
    );
    assert!(!s.take_bank_purchase(), "drained");

    // Six is full. A successful buy has no reply packet: `PLAYERBANKBAGSLOTS_CHANGED` repaints.
    s.set_bank(Some(BankState {
        num_purchased: 6,
        next_cost: 999_999_999,
    }));
    s.fire_event("PLAYERBANKBAGSLOTS_CHANGED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return BankFramePurchaseInfo:IsShown()")
            .unwrap(),
        "the purchase row hides once full (6 purchased)"
    );
}

/// The bag buttons register both mouse buttons (`BankFrameBaseButton_OnLoad`, BankFrame.lua:12),
/// and `BankFrameItemButtonBag_OnClick` reads no button. It opens the bag before it plays
/// `BAGMENUBUTTONPRESS` (BankFrame.lua:193-194), so `igBackPackOpen` leads.
#[test]
fn a_bank_bag_button_answers_the_right_button_too() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    // Bank bag slot 1: container 5, `BankFrameBag1:GetID()`.
    s.set_container(5, empty_bag("Bank Bag", 8));
    s.set_bank(Some(BankState::default()));
    s.fire_event("BANKFRAME_OPENED", vec![]);
    s.resolve();
    let _ = s.take_sounds();

    let (cx, cy): (f64, f64) = s
        .eval(
            "return (BankFrameBag1:GetLeft() + BankFrameBag1:GetRight()) / 2, \
                    (BankFrameBag1:GetTop() + BankFrameBag1:GetBottom()) / 2",
        )
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    let consumed = s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    assert!(consumed, "the right-click lands on the button");
    assert_eq!(
        s.take_sounds(),
        vec![
            SoundRequest::KitName("igBackPackOpen".into()),
            SoundRequest::KitName("BAGMENUBUTTONPRESS".into()),
        ],
        "a right-click runs the same OnClick a left-click does — the window ToggleBag opened, \
         then the button's own press kit"
    );
    assert!(bag_open(&s, 5), "the right-click opened the bank bag");
    assert!(
        s.eval::<bool>("return BankFrameBag1HighlightFrameTexture:IsShown()")
            .unwrap(),
        "…and lit the button (UpdateBagButtonHighlight's own texture)"
    );
    assert!(s.errors().is_empty(), "click errors: {:?}", s.errors());
}

/// Driven through `mouse_button`, not `run()`, so the XML's `OnClick` and `RegisterForClicks`
/// wiring is under test.
#[test]
fn clicking_item_slot_one_with_an_empty_cursor_queues_the_pickup() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_money(0);

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Gem_01".into()),
            count: 1,
            item_id: 774,
            ..Default::default()
        },
    );
    s.set_container(
        -1,
        Some(ContainerState {
            name: Some("Bank".into()),
            num_slots: 24,
            slots,
        }),
    );
    s.set_bank(Some(BankState::default()));
    s.fire_event("BANKFRAME_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();

    assert!(s.cursor_item().is_none(), "nothing on the cursor yet");

    let (cx, cy): (f64, f64) = s
        .eval(
            "return (BankFrameItem1:GetLeft() + BankFrameItem1:GetRight()) / 2, \
                    (BankFrameItem1:GetTop() + BankFrameItem1:GetBottom()) / 2",
        )
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "LeftButton", true);
    let consumed = s.mouse_button(cx as f32, cy as f32, "LeftButton", false);
    assert!(consumed, "the click lands on a mouse-enabled frame");
    assert!(s.errors().is_empty(), "click errors: {:?}", s.errors());

    let held = s
        .cursor_item()
        .expect("the click picked the item up onto the cursor");
    assert_eq!(
        (held.bag, held.slot),
        (-1, 1),
        "PickupContainerItem(-1, 1) — bank container, slot 1"
    );
    assert!(
        s.take_container_moves().is_empty(),
        "a bare pickup (nothing to swap into) queues no move yet"
    );
}

fn icon_quad<'a>(quads: &'a [ExtractedQuad], needle: &str) -> Option<&'a ExtractedQuad> {
    quads.iter().find(
        |q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle)),
    )
}

/// Whether the icon is drawn desaturated (`SetItemButtonDesaturated`).
fn is_greyed(quads: &[ExtractedQuad], needle: &str) -> bool {
    match icon_quad(quads, needle).map(|q| &q.content) {
        Some(QuadContent::Texture { desaturated, .. }) => *desaturated,
        _ => panic!("no icon quad matching {needle}"),
    }
}

/// A bag in a bank bag slot. `contents_count: Some(_)` makes it a container to
/// `GetInventoryItemCount`, non-zero so that the binding's slot-`0x16` cutoff does the zeroing.
fn a_bag(locked: bool) -> benilla_ui::script::InvSlotView {
    benilla_ui::script::InvSlotView {
        item_id: 4500,
        icon: Some(r"Interface\Icons\INV_Misc_Bag_08".into()),
        count: 1,
        contents_count: Some(3),
        name: Some("Traveler's Backpack".into()),
        locked,
        ..Default::default()
    }
}

/// Despite its name, `BankFrameItemButton_OnUpdate` is no `OnUpdate` handler: the bag buttons
/// repaint only on `PLAYERBANKSLOTS_CHANGED` or `BANKFRAME_OPENED` (BankFrame.lua:206-212), and
/// `ITEM_LOCK_CHANGED` repaints the desaturation, never the icon.
#[test]
fn a_bank_bag_button_paints_the_drop_and_lets_go_of_the_lock() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_money(0);
    s.set_container(0, empty_bag("Backpack", 16));
    s.set_bank(Some(BankState {
        num_purchased: 2,
        next_cost: 10_000,
    }));
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            name: Some("Grumnus Steelshaper".into()),
            ..Default::default()
        }),
    );
    s.fire_event("BANKFRAME_OPENED", vec![]);
    assert!(
        icon_quad(&s.extract(), "INV_Misc_Bag_08").is_none(),
        "no bag in the slot yet"
    );

    // The drop lands and the server has not answered: the send locks both ends.
    let mut bags: benilla_ui::script::BankBagSlots = Default::default();
    bags[0] = Some(a_bag(true));
    s.set_bank_bag_slots(bags.clone());
    // The event carries no arguments (`0x703e50`): every bank button repaints its own slot.
    s.fire_event("PLAYERBANKSLOTS_CHANGED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        is_greyed(&s.extract(), "INV_Misc_Bag_08"),
        "in flight, the button greys — BankFrameItemButton_UpdateLock's desaturate arm"
    );
    // No count: a bag button's `isBag` makes `SetItemButtonCount` print any count above 0, but
    // `GetInventoryItemCount` (`0x4c8680`) returns 0 for a container past 0-based slot 0x16.
    assert!(
        !s.eval::<bool>("return BankFrameBag1Count:IsShown()")
            .unwrap(),
        "a bank bag counts nothing — the count fontstring stays hidden"
    );

    // The lock lets go: `ITEM_LOCK_CHANGED` alone must ungrey it, which holds only because
    // `ui_items::feed::resolve_item_locks` corrects the snapshot ahead of both feeds.
    bags[0] = Some(a_bag(false));
    s.set_bank_bag_slots(bags.clone());
    s.fire_event("ITEM_LOCK_CHANGED", vec![]);
    assert!(
        !is_greyed(&s.extract(), "INV_Misc_Bag_08"),
        "the stuck grey: unlocked, the button must come back to full colour"
    );

    // Moved to the next bag slot: only the repaint event takes the icon off the first button.
    bags[0] = None;
    bags[1] = Some(a_bag(false));
    s.set_bank_bag_slots(bags);
    s.fire_event("PLAYERBANKSLOTS_CHANGED", vec![]);
    s.fire_event("PLAYERBANKSLOTS_CHANGED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    let quads = s.extract();
    let bag = icon_quad(&quads, "INV_Misc_Bag_08").expect("the bag is still drawn, once");
    let drawn = bag.rect.expect("the icon resolved a rect").left;
    let bag2_left = s.eval::<f32>("return BankFrameBag2:GetLeft()").unwrap();
    let bag1_left = s.eval::<f32>("return BankFrameBag1:GetLeft()").unwrap();
    assert!(
        (drawn - bag2_left).abs() < (drawn - bag1_left).abs(),
        "the bag is drawn on button 2, not the one it left ({drawn} vs {bag1_left}/{bag2_left})"
    );
}
