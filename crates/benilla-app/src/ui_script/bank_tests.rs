//! The **bank window** driven end-to-end, engine-only (no Bevy): the reference's own
//! `Interface\FrameXML\BankFrame.xml`, executed off the player's patch chain behind the bag
//! chain — decision 0604 phase 4's machine checks, re-pointed at the real file by 1751 (window 2).
//! Split from `merchant_tests`/`bag_tests` along the folder's one-file-per-window convention.
//!
//! **What decision 1751 changed in here, in two passes.** First the bank lost six popout windows
//! of its own — `BenillaBankBagFrame1..6`, six copies of `BagFrame.xml`'s window template — because
//! a bank bag is an ordinary `ContainerFrame` at container id `NUM_BAG_SLOTS + slot` (5..10), which
//! is what the real client always did and what 0604 already said it did. Then the window itself
//! went, and every name below is the reference's: `BankFrameItem1..24`, `BankFrameBag1..6`,
//! `BankFramePurchaseButton`, `BankFrameDetailMoneyFrame`.
//!
//! Two consequences worth stating, because both are places a test could quietly assert our old
//! behaviour instead of the client's:
//!
//! · **The bag buttons' `GetID()` is the CONTAINER id, 5..10** — not a bag number. That is what
//!   `ButtonInventorySlot` hands `BankButtonIDToInvSlotID`, and what `ToggleBag` takes.
//! · **Opening the bank does NOT open your bags.** Decision 0561 (a window that opens your bags
//!   opens every equipped one) is ours and stays for the vendor; the bank goes faithful, and the
//!   reference's `BankFrame` OnShow plays one sound and nothing else. The director's call.
//!
//! The knock-on for the fixtures: the reference generates a window only for a container that
//! exists (`OpenBag`'s `size > 0` gate), where our own `BenillaBagFrame*` were static frames that
//! `Show()` regardless. So a test that wants a bank bag open has to feed a bag at id 5 — which is
//! the honest fixture anyway: an empty bank-bag slot has no window in the real client either.

use benilla_ui::script::{
    BankState, ContainerSlot, ContainerState, ExtractedQuad, QuadContent, ScriptValue,
    SoundRequest, UiScript,
};

use super::test_ui::{bag_open, load_ui as load_xml, BAG_UI};

/// A container fixture with no items in it — the shape every "is this window open" test wants.
fn empty_bag(name: &str, num_slots: u32) -> Option<ContainerState> {
    Some(ContainerState {
        name: Some(name.into()),
        num_slots,
        slots: std::collections::HashMap::new(),
    })
}

/// The bank's own dependency chain, in `benilla.toc` order: [`BAG_UI`] carries it whole — Fonts
/// before anything with a font `inherits`, the four templates stock `ContainerFrame.xml` inherits,
/// UiPanels before any UIPanel/StaticPopup use, GameTooltip before any slot's hover, the
/// reference's own container file, our bag bar, and the reference's own `BankFrame.xml` (which
/// BAG_UI already needs, because `updateContainerFrameAnchors` measures every open bag against
/// `BankFrame:GetRight()`).
///
/// `MerchantFrame.xml` used to load here too, for the `BenillaMoney_*` coin rig our own bank
/// window's purse and cost rows called. The reference's rows are `SmallMoneyFrameTemplate`
/// instances off `MoneyFrame.xml`, which BAG_UI already carries — so the merchant is not a bank
/// dependency any more.
///
/// **Every caller needs client data**: `BAG_UI` names a chain entry, so each test below opens with
/// `wow_data_or_skip!`.
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

/// Test 1 — BANKFRAME_OPENED shows the frame (through ShowUIPanel, landing at the left slot — the
/// same NPC-session chain Merchant/Trainer use) and sets the title to the event arg.
///
/// **It does NOT open your bags, and that is the change.** Decision 0561 — a window that opens
/// your bags opens every equipped one — is this client's, and the vendor keeps it; the bank went
/// faithful with 1751's swap (the director's call), and the reference's `BankFrame` OnShow plays
/// `igMainMenuOpen` and nothing else. So the one sound is the whole sound list, which is a
/// stronger assertion than it looks: a stray `OpenBackpack` anywhere on this path would put
/// `igBackPackOpen` in front of it.
///
/// **And the title comes from `UnitName("npc")`, not from the event.** The reference's
/// `BankFrame_OnEvent` reads no `arg1` (BankFrame.lua:125), so the banker's name has to arrive on
/// the interaction token `crate::ui_session` points at the banker — which is why this test seats
/// an `"npc"` unit and why `feed_bank` stopped firing a name argument the reference never fires.
#[test]
fn bankframe_opened_shows_and_sets_the_title() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();

    // The reference gives `BankFrameTitleText` no `text=` — it is empty until an event names the
    // banker (our own file used to author a "Banker" placeholder).
    assert_eq!(
        s.eval::<String>("return BankFrameTitleText:GetText() or \"\"")
            .unwrap(),
        ""
    );
    assert!(!s.eval::<bool>("return BankFrame:IsVisible()").unwrap());

    let _ = s.take_sounds(); // ignore anything from load (every frame is hidden; nothing should fire)

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
        "the reference's bank leaves your bags alone (0561 is the vendor's, not the bank's)"
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

/// Test 2 — Hiding the frame (BANKFRAME_CLOSED, routed through HideUIPanel) queues the CloseBankFrame
/// intent (`take_bank_close()`), closes any open bank bag with it, and plays the close
/// sound — the reference's own OnHide order (CloseBankBagFrames(); CloseBankFrame(); PlaySound).
///
/// **"Popout" is this test's own history, not a widget any more** (1751). It was
/// `BenillaBankBagFrame1`, one of six copies of `BagFrame.xml`'s window template that our own bank
/// file owned, and the assertion was `:Show()` it then check `:IsShown()`. A bank bag is an
/// ordinary container now — id `NUM_BAG_SLOTS + 1` = 5 — so it is OPENED with the reference's own
/// `OpenBag` and asked for with `IsBagOpen`, and `CloseBankBagFrames` is now the reference's own
/// `CloseBag(5..10)` loop rather than a transcription of it. The property is unchanged and is the
/// point of the test: closing the bank must close the bank BAGS, not merely play a sound.
#[test]
fn bankframe_closed_queues_close_closes_open_popouts_and_plays_the_close_kit() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_money(0);
    // A bag in the FIRST bank bag slot — container 5, which is `BankFrameBag1`'s own `GetID()`.
    s.set_container(5, empty_bag("Bank Bag", 8));
    s.set_bank(Some(BankState::default()));
    s.fire_event("BANKFRAME_OPENED", vec![ScriptValue::Str("Banker".into())]);
    let _ = s.take_sounds();
    let _ = s.take_bank_close(); // nothing queued yet — the drain would otherwise see a stale flag

    // A bank bag open when the main window closes (ref CloseBankBagFrames' own reason to exist) —
    // opened through the reference's own verb, which is what the bag button calls.
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
        "OnHide queued CloseBankFrame() — the client-side close intent (no wire opcode, decision 0604)"
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

/// Test 3 — A pushed container −1 with an item in slot 3 paints that slot button's icon;
/// PLAYERBANKSLOTS_CHANGED(3) repaints it after a later change.
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

    // A later change to the same slot (a stack count bump) repaints again.
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

/// Test 4 — Bag buttons: with `set_bank` num_purchased=2, buttons 1-2 read normal (white) and 3-6
/// read the red tint (1.0, 0.1, 0.1 — the reference's exact tint, `UpdateBagSlotStatus`
/// BankFrame.lua:88-95); the bank-bag inventory band drives button 1's icon.
#[test]
fn bag_buttons_tint_by_purchase_count_and_texture_from_the_bank_bag_band() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_money(0);
    s.set_bank(Some(BankState {
        num_purchased: 2,
        next_cost: 100_000,
    }));
    // Bank bag slot 1 holds a bag: an inventory slot at live id 64, which is where the
    // reference's `BankFrameItemButton_OnUpdate` reads its icon from
    // (`GetInventoryItemTexture("player", ButtonInventorySlot())`).
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

    // BagFrame.xml's own bag-bar slots (CharacterBag0Slot..4) fall back to this SAME empty-slot
    // texture, so the search must be scoped to each named bank button's own rect, not the path
    // alone — get each button's center via eval, then read the color off the quad sitting there.
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

    // Button 1: the live inventory slot's own bag icon, purchased -> white.
    let (x1, y1) = center("BankFrameBag1");
    let c1 = color_at(x1, y1, "INV_Misc_Bag_08").expect("button 1 shows the banked bag's icon");
    assert!(
        near(c1, 1.0, 1.0, 1.0),
        "button 1 is purchased: white {c1:?}"
    );

    // Button 2: purchased but empty -> the paper-doll empty bag art, still white. The art comes
    // from `GetInventorySlotInfo(strsub("BankFrameBag2", 10))` — the DBC's own `"Bag2"` row.
    let (x2, y2) = center("BankFrameBag2");
    let c2 = color_at(x2, y2, "UI-PaperDoll-Slot-Bag").expect("button 2 shows the empty-slot art");
    assert!(
        near(c2, 1.0, 1.0, 1.0),
        "button 2 is purchased: white {c2:?}"
    );

    // Buttons 3..6: unpurchased -> the reference's red tint.
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

/// The purse row at the bottom of the window, and the director's "the gold at bottom you see
/// 97…" report.
///
/// Our own window painted the purse with `BenillaMoney_Set` into a single FontString, which is
/// what could run out of room. The reference's `BankFrameMoneyFrame` is an ordinary
/// `SmallMoneyFrameTemplate` — the same three-button gold/silver/copper kit every other window in
/// this client already uses — so a large amount splits across three buttons instead of
/// overflowing one string. Asserted on the DIGITS in each button, not on the picture: whether it
/// reads right on screen is the director's call, but "98765 / 43 / 21 in three separate buttons"
/// is a fact a test can hold.
#[test]
fn the_purse_row_splits_a_large_amount_across_three_coin_buttons() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_bank(Some(BankState::default()));
    s.fire_event("BANKFRAME_OPENED", vec![]);
    // 98765g 43s 21c — wider than any single-string purse could hold.
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

/// Test 5 — The purchase flow: the Purchase button's click shows the `CONFIRM_BUY_BANK_SLOT` popup;
/// the popup's accept queues `take_bank_purchase()`; the whole row hides once `num_purchased`
/// reaches 6 (`GetNumBankSlots()`'s `full`).
///
/// Two things this pins that only the swap made checkable. `BankFramePurchaseButton` is declared
/// `virtual="true"` INSIDE `<Frames>` — a reference quirk Classic Era still carries — and only a
/// TOP-LEVEL element is ever a template, so the button really exists and really clicks. And the
/// dialog itself lives in `UiPanels.xml` now: the reference keeps it in `StaticPopup.lua`, which
/// this client replaces, so without the re-home the button would be a silent no-op.
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
    // The dialog carries the cost, which is why 1580's `hasMoneyFrame` had to be live before the
    // reference's own StaticPopup entry could be re-homed into UiPanels.xml: its OnShow reads
    // `BankFrame.nextSlotCost`, set by `UpdateBagSlotStatus` off `GetBankSlotCost`.
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

    // Six purchased: full -> the whole row hides (PLAYERBANKBAGSLOTS_CHANGED is the no-packet
    // buy's own repaint trigger).
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

/// The bank's bag buttons take BOTH mouse buttons: the ref's
/// `BankItemButtonBagTemplate` OnLoad runs `BankFrameBagButton_OnLoad` →
/// `BankFrameBaseButton_OnLoad`, which registers `("LeftButtonUp","RightButtonUp")`
/// (BankFrame.lua:12), and `BankFrameItemButtonBag_OnClick` reads no button. Ours registered
/// nothing, so the widget default (`{"LeftButtonUp"}`) swallowed every right-click.
///
/// It was asserted on the click sound alone, because "the one observable that does not need a bag
/// object fed into the slot" was all there was while the popout was a `BenillaBankBagFrame` our
/// own file drove by hand. The reference's handler has a real tail — `ToggleBag(this:GetID())` —
/// so a bag IS fed here now and the right-click is followed all the way to the window it opens.
///
/// **The sound ORDER is the reference's, and it is the reverse of ours.** Its handler opens the
/// bag FIRST and plays `BAGMENUBUTTONPRESS` after (BankFrame.lua:190-197), so the window's own
/// `igBackPackOpen` leads. Ours played the press kit first. Nothing depends on it; it is here
/// because a transcription that gets the order wrong is exactly the kind of thing the swap is
/// meant to stop having to notice.
#[test]
fn a_bank_bag_button_answers_the_right_button_too() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    // A bag in bank slot 1 ⇒ container id 5, which IS `BankFrameBag1:GetID()`.
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

/// Test 6 — Clicking item slot 1 with an empty cursor queues a `PickupContainerItem(-1, 1)` — read off
/// the cursor's own resulting payload (`bag_tests`' pattern: drive the real click through
/// `mouse_button`, not a bare `run()`, so the XML's `OnClick`/`RegisterForClicks` wiring is
/// actually under test).
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

/// The icon-texture quad whose path contains `needle`, for the desaturation assertions below.
fn icon_quad<'a>(quads: &'a [ExtractedQuad], needle: &str) -> Option<&'a ExtractedQuad> {
    quads.iter().find(
        |q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle)),
    )
}

/// Whether that icon is drawn desaturated (`SetItemButtonDesaturated`'s shader arm).
fn is_greyed(quads: &[ExtractedQuad], needle: &str) -> bool {
    match icon_quad(quads, needle).map(|q| &q.content) {
        Some(QuadContent::Texture { desaturated, .. }) => *desaturated,
        _ => panic!("no icon quad matching {needle}"),
    }
}

/// A bag view for one bank BAG slot — the drop the director made. `contents_count: Some(_)` is
/// what makes it a CONTAINER to `GetInventoryItemCount`, and the value is deliberately non-zero:
/// the binding's `0x16` short-circuit has to be what zeroes it, not an already-zero sum.
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

/// **The bug of 1771, both halves.** A bag dropped into a bank bag slot came up greyed and stayed
/// greyed until the window was reopened, and a bag moved between two bag slots stayed drawn in the
/// one it left.
///
/// Both are the same defect: the six bag buttons repaint from `BankFrameItemButton_OnUpdate`,
/// which is reached ONLY from `BankFrameItemButton_OnEvent` on `PLAYERBANKSLOTS_CHANGED` or
/// `BANKFRAME_OPENED` — despite the name, it is not an `OnUpdate` handler
/// (`BankItemButtonTemplate`'s `<OnUpdate>` is `CursorOnUpdate()`). benilla fired that event only
/// for the vault's 24 slots, so the bag buttons showed whatever had been true at the last
/// *unrelated* vault change, and `ITEM_LOCK_CHANGED` — the one event that did reach them — repaints
/// the desaturation only, never the icon.
///
/// This test drives the reference's own file through the exact sequence and asserts what the
/// player sees at each step.
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
    // The event carries NO arguments (`0x703e50`, `__fastcall(ecx = id)`, no vararg push) — it is
    // a broadcast, and every bank button repaints from its own `GetInventorySlot()`.
    s.fire_event("PLAYERBANKSLOTS_CHANGED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        is_greyed(&s.extract(), "INV_Misc_Bag_08"),
        "in flight, the button greys — BankFrameItemButton_UpdateLock's desaturate arm"
    );
    // **And no digit.** `BankFrameBagButton_OnLoad` sets `isBag = 1`, so `SetItemButtonCount`'s
    // `isBag and count > 0` arm would print any positive number in the corner — the "1" the
    // director saw. `GetInventoryItemCount 0x4c8680` short-circuits every container past 0-based
    // slot 0x16 to a literal 0, and the bank's six bag slots are all in that band.
    assert!(
        !s.eval::<bool>("return BankFrameBag1Count:IsShown()")
            .unwrap(),
        "a bank bag counts nothing — the count fontstring stays hidden"
    );

    // The lock lets go. `ITEM_LOCK_CHANGED` is all the button gets, and it is enough — PROVIDED
    // the snapshot it reads has already been corrected, which is what moving the resolving clear
    // ahead of both feeds buys (`ui_items::feed::resolve_item_locks`).
    bags[0] = Some(a_bag(false));
    s.set_bank_bag_slots(bags.clone());
    s.fire_event("ITEM_LOCK_CHANGED", vec![]);
    assert!(
        !is_greyed(&s.extract(), "INV_Misc_Bag_08"),
        "the stuck grey: unlocked, the button must come back to full colour"
    );

    // Moved to the next bag slot: the icon has to LEAVE the first button, which only the repaint
    // event can do — the lock event never touches the texture.
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
