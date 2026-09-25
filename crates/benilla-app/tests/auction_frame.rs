//! Drives the stock `Blizzard_AuctionUI` addon off the player's chain with a synthetic
//! `AuctionState` and the app's own show and list events, and asserts what it paints and queues.

use benilla_ui::script::{
    AuctionCategory, AuctionItemRow, AuctionListState, AuctionState, AuctionSubCategory, UiScript,
    UnitState, BIDDER, LIST, OWNER,
};

mod common;

/// The auction window's load prefix, in the app's order (`assets/ui/benilla.toc`): UIParent for
/// `ShowUIPanel`, UIPanelTemplates for the tab kit and widget templates, StaticPopup for the
/// dialogs, MoneyFrame for `SmallMoneyFrameTemplate` and `MoneyTypeInfo`, MoneyInputFrame for
/// the price entry, UIDropDownMenu for the rarity menu, ScrollTemplates for the faux lists.
const FILES: &[&str] = &[
    "Interface\\FrameXML\\Fonts.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    // `MoneyInputFrameTemplate` and the `MoneyInputFrame_*` verbs.
    r"Interface\FrameXML\MoneyInputFrame.lua",
    r"Interface\FrameXML\MoneyInputFrame.xml",
    r"Interface\FrameXML\UIParent.xml",
    // UIParent declares `ITEM_QUALITY_COLORS` (`UIParent.lua:65`), which colours each row name.
    "Interface\\FrameXML\\GameTooltip.xml",
    "Interface\\FrameXML\\UIDropDownMenu.xml",
    "ScrollTemplates.xml",
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    // `AuctionTabTemplate` inherits `CharacterFrameTabButtonTemplate`; `inherits=` resolves at
    // load, so it comes first.
    r"Interface\FrameXML\CharacterFrameTemplates.xml",
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\BasicControls.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\StaticPopup.xml", // the dialog engine
    // The dress-up pane's OnLoad calls `DressUpTexturePath` (`DressUpFrame.lua`).
    "Interface\\FrameXML\\DressUpFrame.xml",
    // The addon in its toc order, loaded as chain files: a test has no addon registry.
    "Interface\\AddOns\\Blizzard_AuctionUI\\Blizzard_AuctionUI.xml",
    "Interface\\AddOns\\Blizzard_AuctionUI\\Blizzard_AuctionDressUp.xml",
];

/// [`load_ui`] with the class tree seated first: `AuctionFrameBrowse_OnLoad` reads
/// `GetAuctionItemClasses()` at load, before any session.
fn load_ui_with_classes(s: &mut UiScript) {
    s.set_auction_item_classes(vec![AuctionCategory {
        class_id: 4,
        name: "Armor".into(),
        has_subclass_filter: true,
        subclasses: vec![AuctionSubCategory {
            sub_id: 1,
            name: "Cloth".into(),
            has_inv_types: true,
        }],
    }]);
    load_ui(s);
}

fn load_ui(script: &UiScript) {
    // `common::load_ui`: an entry with a path separator is a stock file read off the player's
    // chain, not from `assets/ui`.
    for file in FILES {
        common::load_ui(script, file);
    }
}

/// One browse row with the fields the window paints.
fn row(name: &str, min_bid: u32, buyout: u32, bid: u32, owner: &str) -> AuctionItemRow {
    AuctionItemRow {
        auction_id: 1,
        item_id: 2589,
        name: Some(name.to_string()),
        texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
        count: 5,
        quality: Some(1),
        level: 10,
        min_bid,
        min_increment: if bid > 0 { 100 } else { 0 },
        buyout_price: buyout,
        bid_amount: bid,
        high_bidder: false,
        owner: Some(owner.to_string()),
        time_left: 4,
        link: Some("|cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r".into()),
        random_property_id: 0,
    }
}

/// A session snapshot: `rows` on the Browse list, nothing on the other two.
/// Each row gets a distinct `auction_id`: the engine remembers a selection by wire id, not row.
fn state(mut rows: Vec<AuctionItemRow>) -> AuctionState {
    for (i, r) in rows.iter_mut().enumerate() {
        r.auction_id = i as u32 + 1;
    }
    let mut lists: [AuctionListState; 3] = Default::default();
    let total = rows.len() as u32;
    lists[LIST] = AuctionListState {
        rows,
        total,
        sort: vec![("bid".into(), true)],
    };
    lists[BIDDER] = AuctionListState::default();
    lists[OWNER] = AuctionListState::default();
    AuctionState {
        lists,
        deposit_percent: 5,
    }
}

/// The player: a name (the bid gate compares it to the owner), a level and a purse.
fn seat_player(s: &mut UiScript, money: u64) {
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Buyer".into()),
            level: 60,
            ..UnitState::default()
        }),
    );
    s.set_money(money);
}

#[test]
fn auction_frame_loads_and_key_regions_exist() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);
    for name in [
        // The window, its three panes and its three tabs.
        "AuctionFrame",
        "AuctionFrameBrowse",
        "AuctionFrameBid",
        "AuctionFrameAuctions",
        "AuctionFrameTab1",
        "AuctionFrameTab2",
        "AuctionFrameTab3",
        // One row of each list, and the parts the repaint addresses by name.
        "BrowseButton1",
        "BrowseButton1Name",
        "BrowseButton1MoneyFrameGoldButton",
        "BrowseButton1MoneyFrameCopperButton",
        "BidButton1",
        "AuctionsButton1",
        // The create form: the sell slot and both money inputs.
        "AuctionsItemButton",
        "StartPrice",
        "StartPriceGold",
        "BuyoutPrice",
        "BuyoutPriceGold",
        // The filter column and the action buttons the gates drive.
        "AuctionFilterButton1",
        "AuctionFilterButton15",
        "BrowseBidButton",
        "BrowseBuyoutButton",
        "AuctionsCreateAuctionButton",
    ] {
        assert!(
            s.eval::<bool>(&format!("return getglobal('{name}') ~= nil"))
                .unwrap(),
            "region {name} should exist"
        );
    }
    // The window is hidden until its show event.
    assert!(!s.eval::<bool>("return AuctionFrame:IsShown()").unwrap());
    assert!(s.errors().is_empty(), "load errors: {:?}", s.errors());
}

/// `doublewide` is the only such `UIPanelWindows` row in 1.12 (`Blizzard_AuctionUI.lua:14`).
#[test]
fn the_window_is_registered_doublewide() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);
    assert_eq!(
        s.eval::<String>("return UIPanelWindows['AuctionFrame'].area")
            .unwrap(),
        "doublewide"
    );
}

#[test]
fn auction_house_show_opens_the_window_on_the_browse_tab() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);
    seat_player(&mut s, 500_000);
    s.set_auction(Some(state(vec![row(
        "Linen Cloth",
        1000,
        5000,
        0,
        "Seller",
    )])));

    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    assert!(
        s.eval::<bool>("return AuctionFrame:IsShown()").unwrap(),
        "the window opens on AUCTION_HOUSE_SHOW"
    );
    assert!(
        s.eval::<bool>("return AuctionFrameBrowse:IsShown()")
            .unwrap(),
        "and lands on the Browse tab"
    );
    assert!(!s.eval::<bool>("return AuctionFrameBid:IsShown()").unwrap());
    assert!(!s
        .eval::<bool>("return AuctionFrameAuctions:IsShown()")
        .unwrap());
    // The skin followed the tab.
    assert_eq!(
        s.eval::<String>("return AuctionFrameTopLeft:GetTexture()")
            .unwrap()
            .to_ascii_lowercase(),
        "interface\\auctionframe\\ui-auctionframe-browse-topleft"
    );

    // Tab 3 re-skins and swaps the pane.
    s.run("AuctionFrameTab_OnClick(3)").unwrap();
    assert!(s
        .eval::<bool>("return AuctionFrameAuctions:IsShown()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return AuctionFrameTopLeft:GetTexture()")
            .unwrap()
            .to_ascii_lowercase(),
        "interface\\auctionframe\\ui-auctionframe-auction-topleft"
    );
    // Opening the Auctions tab asks the server for the owned list, once per window session.
    assert_eq!(
        s.take_auction_owner_query(),
        Some(0),
        "the Auctions pane fetches the owned list on its first show"
    );

    assert!(s.errors().is_empty(), "clean open: {:?}", s.errors());
}

/// `AuctionFrameAuctions_Update` does arithmetic on `AuctionFrameAuctions.page`, which only that
/// pane's `OnShow` assigns, so `AUCTION_OWNED_LIST_UPDATE` is safe only once the tab has shown.
/// The reference never sends it earlier: every `GetOwnerAuctionItems` caller lives in that pane.
#[test]
fn the_auctions_tab_cannot_repaint_before_it_has_been_shown() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);
    seat_player(&mut s, 500_000);
    s.set_auction(Some(state(vec![row(
        "Linen Cloth",
        1000,
        5000,
        0,
        "Seller",
    )])));

    // Open, and land a browse result: the whole of a search.
    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    s.fire_event("AUCTION_ITEM_LIST_UPDATE", vec![]);
    assert!(
        s.take_errors().is_empty(),
        "a browse result on the Browse tab is clean"
    );

    // The owned-list event before the Auctions tab has shown: its `page` is nil.
    s.fire_event("AUCTION_OWNED_LIST_UPDATE", vec![]);
    let errors = s.take_errors();
    assert!(
        errors.iter().any(|e| e.contains("page")),
        "the never-shown Auctions tab raises the nil-page error: {errors:?}"
    );

    // Harmless once the tab has shown, the only state the app sends it in.
    s.run("AuctionFrameTab_OnClick(3)").unwrap();
    assert!(s.take_errors().is_empty(), "opening the tab is clean");
    s.fire_event("AUCTION_OWNED_LIST_UPDATE", vec![]);
    assert!(
        s.take_errors().is_empty(),
        "and now the owned list may announce itself"
    );
}

/// Each pane assigns `page` only in its `OnShow`. `AuctionFrameBid` alone is declared
/// `hidden="false"` (`Blizzard_AuctionUI.xml:960`), so it shows with the window on the first
/// `AUCTION_HOUSE_SHOW` before tab 1 hides it; the Auctions pane has no `page` until tab 3.
#[test]
fn only_the_bids_pane_gets_its_page_without_being_opened() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);

    let page = |s: &UiScript, pane: &str| {
        s.eval::<String>(&format!("return tostring({pane}.page)"))
            .unwrap()
    };

    // At load, with no session: no pane has been shown, so none has a page.
    for pane in [
        "AuctionFrameBrowse",
        "AuctionFrameBid",
        "AuctionFrameAuctions",
    ] {
        assert_eq!(page(&s, pane), "nil", "{pane} before the window opens");
    }

    // Browse is shown by the tab click; Bid is shown with the window and hidden again, keeping
    // the page its OnShow assigned.
    seat_player(&mut s, 500_000);
    s.set_auction(Some(state(vec![row(
        "Linen Cloth",
        1000,
        5000,
        0,
        "Seller",
    )])));
    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    assert_eq!(page(&s, "AuctionFrameBrowse"), "0");
    assert_eq!(
        page(&s, "AuctionFrameBid"),
        "0",
        "hidden=\"false\": the Bids pane rode the window up and back down"
    );
    assert!(
        !s.eval::<bool>("return AuctionFrameBid:IsShown()").unwrap(),
        "and it is not the visible tab"
    );
    assert_eq!(
        page(&s, "AuctionFrameAuctions"),
        "nil",
        "the Auctions pane is the one an unowed event can still kill"
    );
    assert!(s.take_errors().is_empty());
}

/// The reference's show cascade `0x76ae10` is post-order: a frame marks itself visible
/// (`0x76ae7b`), walks its children re-reading live links, and fires its own `OnShow` last
/// (`0x76aef5`). benilla's is pre-order, a gap in the engine; for this window both orders reach
/// the same state, so only the `ShowOrder` assertions are benilla's order.
#[test]
fn the_show_cascade_notifies_the_bids_pane_once_and_keeps_its_page() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);
    seat_player(&mut s, 500_000);
    s.set_auction(Some(state(vec![row(
        "Linen Cloth",
        1000,
        5000,
        0,
        "Seller",
    )])));

    // Both `<OnShow>`s call their handler by global name, so wrapping the globals records the
    // order.
    s.run(
        "ShowOrder = {}          local pane, window = AuctionFrameBid_OnShow, AuctionFrame_OnShow          AuctionFrameBid_OnShow = function() table.insert(ShowOrder, \"pane\") pane() end          AuctionFrame_OnShow = function() table.insert(ShowOrder, \"window\") window() end",
    )
    .unwrap();
    let order = |s: &UiScript| {
        s.eval::<String>("return table.concat(ShowOrder, \",\")")
            .unwrap()
    };
    let page = |s: &UiScript| {
        s.eval::<String>("return tostring(AuctionFrameBid.page)")
            .unwrap()
    };

    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    assert_eq!(
        order(&s),
        "window,pane",
        "OURS, and a known deviation: the reference's cascade is post-order, \"pane,window\""
    );
    assert_eq!(
        page(&s),
        "0",
        "the pane the XML leaves shown takes its page riding the window up — this is what keeps a \
         mis-aimed AUCTION_BIDDER_LIST_UPDATE off the nil-page path"
    );
    assert_eq!(
        s.take_auction_bidder_query(),
        Some(0),
        "and that OnShow is what asks for the bids list"
    );

    // Reopen: the tab click left the pane's shown flag clear, so it is skipped and keeps its page.
    s.run("HideUIPanel(AuctionFrame)").unwrap();
    let _ = s.take_auction_close();
    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    assert_eq!(
        order(&s),
        "window,pane,window",
        "the reopen notifies the window and not the pane it left hidden"
    );
    assert_eq!(page(&s), "0", "kept from the first open, not re-assigned");
    assert_eq!(
        s.take_auction_bidder_query(),
        None,
        "and the reopen puts no second bids query on the wire"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A fed snapshot paints the Browse rows.
#[test]
fn the_browse_list_populates_from_the_fed_snapshot() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);
    seat_player(&mut s, 500_000);
    s.set_auction(Some(state(vec![
        // No bids yet: the row shows the seller's opening price as the current bid.
        row("Linen Cloth", 1000, 5000, 0, "Seller"),
        // Bid on: the row shows the live bid instead.
        row("Wool Cloth", 1000, 0, 2500, "Someone"),
    ])));

    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    s.fire_event("AUCTION_ITEM_LIST_UPDATE", vec![]);

    assert_eq!(
        s.eval::<(i64, i64)>("return GetNumAuctionItems('list')")
            .unwrap(),
        (2, 2)
    );
    assert_eq!(
        s.eval::<String>("return BrowseButton1Name:GetText()")
            .unwrap(),
        "Linen Cloth"
    );
    assert_eq!(
        s.eval::<String>("return BrowseButton2Name:GetText()")
            .unwrap(),
        "Wool Cloth"
    );
    // The `HighBidder` region shows the owner.
    assert_eq!(
        s.eval::<String>("return BrowseButton1HighBidder:GetText()")
            .unwrap(),
        "Seller"
    );
    // Row 1: no bids, so the money line reads the 10s opening price.
    assert_eq!(
        s.eval::<String>("return tostring(BrowseButton1MoneyFrameSilverButton:GetText())")
            .unwrap(),
        "10",
        "1000c with no bids paints the minimum bid"
    );
    // Row 2: bid on, so the money line reads the live 25s bid instead.
    assert_eq!(
        s.eval::<String>("return tostring(BrowseButton2MoneyFrameSilverButton:GetText())")
            .unwrap(),
        "25",
        "a bid-on row paints the live bid, not the minimum"
    );
    // `MoneyTypeInfo["AUCTION"]` sets `showSmallerCoins`: only leading zero coins collapse.
    assert!(
        !s.eval::<bool>("return BrowseButton1MoneyFrameGoldButton:IsShown()")
            .unwrap(),
        "no gold in 1000c, and a LEADING zero is the one thing that does collapse"
    );
    assert!(
        s.eval::<bool>("return BrowseButton1MoneyFrameCopperButton:IsShown()")
            .unwrap(),
        "the trailing zero copper stays on under showSmallerCoins"
    );
    assert_eq!(
        s.eval::<String>("return tostring(BrowseButton1MoneyFrameCopperButton:GetText())")
            .unwrap(),
        "0"
    );
    // Row 1 has a buyout, so its buyout line shows; row 2 has none, so it hides.
    assert!(s
        .eval::<bool>("return BrowseButton1BuyoutMoneyFrame:IsShown()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return BrowseButton2BuyoutMoneyFrame:IsShown()")
        .unwrap());
    // The stack count shows for a stack of 5.
    assert!(s
        .eval::<bool>("return BrowseButton1ItemCount:IsShown()")
        .unwrap());
    // The closing-time bucket sets a hover tooltip (its GlobalStrings text may be blank here).
    assert!(s
        .eval::<bool>("return BrowseButton1ClosingTime.tooltip ~= nil")
        .unwrap());

    // A row past the batch is hidden.
    assert!(s.eval::<bool>("return BrowseButton2:IsShown()").unwrap());
    assert!(
        !s.eval::<bool>("return BrowseButton3:IsShown()").unwrap(),
        "row 3 is past the 2-row batch and must be hidden"
    );

    assert!(s.errors().is_empty(), "clean repaint: {:?}", s.errors());
}

/// The bid and buyout gates open only for the selected row, and only when the purse covers it.
#[test]
fn the_bid_and_buyout_gates_read_the_purse() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);
    // 2 gold. Row 1 costs 10s to bid and 50s to buy out; row 2 wants 5 gold.
    seat_player(&mut s, 20_000);
    s.set_auction(Some(state(vec![
        row("Linen Cloth", 1000, 5000, 0, "Seller"),
        row("Arcanite Bar", 50_000, 60_000, 0, "Seller"),
    ])));
    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    s.fire_event("AUCTION_ITEM_LIST_UPDATE", vec![]);

    // Nothing selected: both shut.
    assert!(!s
        .eval::<bool>("return BrowseBidButton:IsEnabled() ~= 0")
        .unwrap());
    assert!(!s
        .eval::<bool>("return BrowseBuyoutButton:IsEnabled() ~= 0")
        .unwrap());

    // Select the affordable row: both open, and the bid box is seated at the required bid.
    s.run("BrowseButton1:Click()").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetSelectedAuctionItem('list')")
            .unwrap(),
        1
    );
    assert!(
        s.eval::<bool>("return BrowseBidButton:IsEnabled() ~= 0")
            .unwrap(),
        "10s is affordable on 2g"
    );
    assert!(
        s.eval::<bool>("return BrowseBuyoutButton:IsEnabled() ~= 0")
            .unwrap(),
        "50s is affordable on 2g"
    );
    assert_eq!(
        s.eval::<i64>("return MoneyInputFrame_GetCopper(BrowseBidPrice)")
            .unwrap(),
        1000,
        "with no bids the required bid IS the minimum bid"
    );

    // Select the unaffordable row: both shut again.
    s.run("BrowseButton2:Click()").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetSelectedAuctionItem('list')")
            .unwrap(),
        2
    );
    assert!(
        !s.eval::<bool>("return BrowseBidButton:IsEnabled() ~= 0")
            .unwrap(),
        "5g is not affordable on 2g"
    );
    assert!(
        !s.eval::<bool>("return BrowseBuyoutButton:IsEnabled() ~= 0")
            .unwrap(),
        "6g is not affordable on 2g"
    );

    // Back to row 1 and press Bid: the intent reaches the app with the seated amount.
    s.run("BrowseButton1:Click()").unwrap();
    let _ = s.take_auction_bids();
    s.run("BrowseBidButton:Click()").unwrap();
    let bids = s.take_auction_bids();
    assert_eq!(bids.len(), 1, "one bid queued, got {bids:?}");
    assert_eq!(bids[0].list, LIST);
    assert_eq!(bids[0].index, 1);
    assert_eq!(bids[0].amount, 1000);
    assert!(
        !s.eval::<bool>("return BrowseBidButton:IsEnabled() ~= 0")
            .unwrap(),
        "the button disables itself so a second click cannot outrun the answer"
    );

    assert!(s.errors().is_empty(), "clean gating: {:?}", s.errors());
}

/// The one leg of the Browse bid gate that is not about money.
#[test]
fn you_cannot_bid_on_your_own_auction() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);
    seat_player(&mut s, 10_000_000);
    s.set_auction(Some(state(vec![row(
        "Linen Cloth",
        1000,
        5000,
        0,
        "Buyer",
    )])));
    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    s.fire_event("AUCTION_ITEM_LIST_UPDATE", vec![]);

    s.run("BrowseButton1:Click()").unwrap();
    assert!(
        !s.eval::<bool>("return BrowseBidButton:IsEnabled() ~= 0")
            .unwrap(),
        "the row's owner is the player"
    );
    // The buyout gate has no such leg: the reference lets you buy out your own listing.
    assert!(s
        .eval::<bool>("return BrowseBuyoutButton:IsEnabled() ~= 0")
        .unwrap());
    assert!(s.errors().is_empty());
}

/// A search reads every filter at once; nothing before Search queues a query.
#[test]
fn search_reads_the_filters_and_nothing_queries_before_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);
    seat_player(&mut s, 0);
    s.set_auction(Some(state(vec![])));
    s.set_auction_can_query(true);
    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    let _ = s.take_auction_query();

    // The class tree paints from the pushed categories.
    s.run("AuctionFrameFilters_Update()").unwrap();
    assert_eq!(
        s.eval::<String>("return AuctionFilterButton1:GetText()")
            .unwrap(),
        "Armor"
    );
    // Clicking it expands the subclass beneath it and queries nothing.
    s.run("AuctionFilterButton1:Click()").unwrap();
    assert!(
        s.eval::<String>("return AuctionFilterButton2:GetText()")
            .unwrap()
            .contains("Cloth"),
        "the selected class expands its subclasses in place"
    );
    assert!(
        s.take_auction_query().is_none(),
        "a filter click must not query — the selection is read when Search is pressed"
    );

    s.run("BrowseName:SetText('linen')").unwrap();
    s.run("BrowseMinLevel:SetText('10')").unwrap();
    s.run("BrowseMaxLevel:SetText('20')").unwrap();
    s.run("IsUsableCheckButton:SetChecked(1)").unwrap();
    assert!(s.take_auction_query().is_none(), "still nothing queued");

    s.run("AuctionFrameBrowse_Search()").unwrap();
    let query = s.take_auction_query().expect("Search queues one query");
    assert_eq!(query.name, "linen");
    assert_eq!(query.min_level, 10);
    assert_eq!(query.max_level, 20);
    // The stock Lua hands over the class row's position (1); the wire carries its item class id.
    assert_eq!(
        query.class,
        Some(4),
        "Armor is item class 4, not menu row 1"
    );
    assert!(query.usable_only);
    assert_eq!(query.page, 0);
    assert!(s.errors().is_empty(), "clean search: {:?}", s.errors());
}

/// The create form's gate and the deposit, which the client computes from the stack's vendor
/// value and the run time, never the asking price.
#[test]
fn the_create_gate_and_the_deposit() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);
    seat_player(&mut s, 100_000);
    s.set_auction(Some(state(vec![])));
    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    s.run("AuctionFrameTab_OnClick(3)").unwrap();

    // Empty slot: shut, whatever the prices say.
    assert!(
        !s.eval::<bool>("return AuctionsCreateAuctionButton:IsEnabled() ~= 0")
            .unwrap(),
        "no item in the sell slot"
    );
    // The default run time is the middle one, 8 hours.
    assert_eq!(
        s.eval::<i64>("return AuctionFrameAuctions.duration")
            .unwrap(),
        480
    );
    s.run("AuctionsShortAuctionButton:Click()").unwrap();
    assert_eq!(
        s.eval::<i64>("return AuctionFrameAuctions.duration")
            .unwrap(),
        120
    );
    s.run("AuctionsLongAuctionButton:Click()").unwrap();
    assert_eq!(
        s.eval::<i64>("return AuctionFrameAuctions.duration")
            .unwrap(),
        1440
    );

    // With no item the form never reaches the price checks: the buyout error stays hidden.
    s.run("MoneyInputFrame_SetCopper(StartPrice, 10000)")
        .unwrap();
    s.run("MoneyInputFrame_SetCopper(BuyoutPrice, 5000)")
        .unwrap();
    s.run("AuctionsFrameAuctions_ValidateAuction()").unwrap();
    assert!(!s
        .eval::<bool>("return AuctionsCreateAuctionButton:IsEnabled() ~= 0")
        .unwrap());
    assert!(!s
        .eval::<bool>("return AuctionsBuyoutErrorText:IsShown()")
        .unwrap());

    // Fill the slot through the cursor: pick a bag item up and click the slot
    // (`ClickAuctionSellItemButton`, as a drop would).
    s.set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::from([(
                1,
                benilla_ui::script::ContainerSlot {
                    texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
                    count: 4,
                    quality: Some(1),
                    item_id: 2589,
                    link: Some("|cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r".into()),
                    ..Default::default()
                },
            )]),
        }),
    );
    s.set_item_template(
        2589,
        benilla_ui::script::ItemTemplateView {
            name: "Linen Cloth".into(),
            sell_price: 25,
            ..Default::default()
        },
    );
    // A split pickup: after a whole-stack pickup (`count: None`) `GetAuctionSellItemInfo`
    // answers a count of 1, an engine gap in `script/auction.rs`'s `sell_item_info`.
    s.run("SplitContainerItem(0, 1, 2)").unwrap();
    s.run("AuctionsItemButton:Click()").unwrap();
    // The create pane paints on NEW_AUCTION_UPDATE, which the app fires when the slot changes.
    s.fire_event("NEW_AUCTION_UPDATE", vec![]);
    assert_eq!(
        s.eval::<String>("return AuctionsItemButtonName:GetText()")
            .unwrap(),
        "Linen Cloth",
        "the slot paints the item it took from the cursor"
    );
    assert_eq!(
        s.eval::<String>("return AuctionsItemButtonCount:GetText()")
            .unwrap(),
        "2"
    );
    assert!(s
        .eval::<bool>("return AuctionsItemButtonCount:IsShown()")
        .unwrap());

    // A drop resets the form (`AuctionSellItemButton_OnEvent` seeds the start price and zeroes
    // the buyout), so the prices are typed after it.
    s.run("MoneyInputFrame_SetCopper(StartPrice, 10000)")
        .unwrap();
    s.run("MoneyInputFrame_SetCopper(BuyoutPrice, 5000)")
        .unwrap();
    // With an item in the slot, a buyout under the start price shows its error.
    s.run("AuctionsFrameAuctions_ValidateAuction()").unwrap();
    assert!(
        s.eval::<bool>("return AuctionsBuyoutErrorText:IsShown()")
            .unwrap(),
        "a 50s buyout under a 1g start price is an error, and it is shown"
    );
    assert!(!s
        .eval::<bool>("return AuctionsCreateAuctionButton:IsEnabled() ~= 0")
        .unwrap());

    // Clear the buyout and the form opens; pressing Create sends exactly what is on screen.
    s.run("MoneyInputFrame_SetCopper(BuyoutPrice, 0)").unwrap();
    s.run("AuctionsFrameAuctions_ValidateAuction()").unwrap();
    assert!(
        s.eval::<bool>("return AuctionsCreateAuctionButton:IsEnabled() ~= 0")
            .unwrap(),
        "an item, a 1g start price and no buyout is a valid auction"
    );
    // 5% of the stack's vendor value (2 x 25c = 50c) floors to 2c, per two-hour unit: 24h is 24c.
    assert_eq!(
        s.eval::<i64>("return CalculateAuctionDeposit(1440)")
            .unwrap(),
        24
    );
    assert_eq!(
        s.eval::<i64>("return CalculateAuctionDeposit(120)")
            .unwrap(),
        2
    );
    let _ = s.take_auction_start();
    s.run("AuctionsCreateAuctionButton:Click()").unwrap();
    let start = s.take_auction_start().expect("Create queues one auction");
    assert_eq!(start.min_bid, 10000);
    assert_eq!(start.buyout, 0);
    assert_eq!(start.duration, 1440, "the long run time is still selected");

    assert!(s.errors().is_empty(), "clean create form: {:?}", s.errors());
}

/// Closing the window ends the session client-side and takes both confirmations with it.
#[test]
fn hiding_the_window_closes_the_session() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);
    seat_player(&mut s, 0);
    s.set_auction(Some(state(vec![])));
    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    let _ = s.take_auction_close();

    s.run("HideUIPanel(AuctionFrame)").unwrap();
    assert!(!s.eval::<bool>("return AuctionFrame:IsShown()").unwrap());
    assert!(
        s.take_auction_close(),
        "OnHide queues CloseAuctionHouse (the session is client-side only)"
    );
    assert!(s.errors().is_empty());

    // And the server-side close hides it again from the other direction.
    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    assert!(s.eval::<bool>("return AuctionFrame:IsShown()").unwrap());
    s.fire_event("AUCTION_HOUSE_CLOSED", vec![]);
    assert!(!s.eval::<bool>("return AuctionFrame:IsShown()").unwrap());
    assert!(s.errors().is_empty(), "clean close: {:?}", s.errors());
}

/// With more matches than the 50-row page, the turners appear only with the list scrolled to the
/// bottom; the list is fed one extra row so the bar can get there.
#[test]
fn paging_shows_the_turners_only_at_the_end_of_the_list() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);
    seat_player(&mut s, 0);
    let mut rows = Vec::new();
    for i in 0..50 {
        rows.push(row(&format!("Item {i}"), 100, 0, 0, "Seller"));
    }
    let mut st = state(rows);
    st.lists[LIST].total = 120;
    s.set_auction(Some(st));
    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    s.fire_event("AUCTION_ITEM_LIST_UPDATE", vec![]);

    assert!(
        !s.eval::<bool>("return BrowseNextPageButton:IsShown()")
            .unwrap(),
        "at the top of a full page the turners stay out of the way"
    );

    // 51 rows over 8 slots: offset 43 leaves slot 8 empty, which reveals the turners.
    s.run("FauxScrollFrame_SetOffset(BrowseScrollFrame, 43)")
        .unwrap();
    s.run("AuctionFrameBrowse_Update()").unwrap();
    assert!(
        !s.eval::<bool>("return BrowseButton8:IsShown()").unwrap(),
        "the extra row slot is empty at the bottom — that is what reveals the turners"
    );
    assert!(
        s.eval::<bool>("return BrowseNextPageButton:IsShown()")
            .unwrap(),
        "120 matches over a 50-row page, scrolled to the end: the turners appear"
    );
    assert!(s
        .eval::<bool>("return BrowseSearchCountText:IsShown()")
        .unwrap());

    // Turning the page re-searches at the next offset rather than scrolling.
    s.set_auction_can_query(true);
    let _ = s.take_auction_query();
    s.run("BrowseNextPageButton:Click()").unwrap();
    let query = s.take_auction_query().expect("the turner re-queries");
    assert_eq!(query.page, 1, "the next page, 0-based");

    assert!(s.errors().is_empty(), "clean paging: {:?}", s.errors());
}

/// The row and sell-slot hovers call `GameTooltip:SetAuctionItem` and `SetAuctionSellItem`, so an
/// addon hooking either sees the call.
#[test]
fn a_row_hover_goes_through_the_reference_tooltip_verb() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui_with_classes(&mut s);
    s.set_auction(Some(state(vec![row("Copper Bar", 100, 500, 0, "Someone")])));
    s.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    s.fire_event("AUCTION_ITEM_LIST_UPDATE", vec![]);

    // Both verbs are tooltip methods.
    assert!(
        s.eval::<bool>("return type(GameTooltip.SetAuctionItem) == 'function'")
            .unwrap(),
        "SetAuctionItem is bound"
    );
    assert!(
        s.eval::<bool>("return type(GameTooltip.SetAuctionSellItem) == 'function'")
            .unwrap(),
        "SetAuctionSellItem is bound"
    );

    // The row hover runs clean; the harness has no item templates, so this checks the call path.
    // A direct call binds `this` as the dispatcher would, or `SetOwner(this, ...)` gets nil.
    s.run("this = BrowseButton1Item AuctionFrameItem_OnEnter('list', 1)")
        .unwrap();
    assert!(
        s.take_errors().is_empty(),
        "the row hover raised no script error"
    );

    // An empty sell slot is a no-op, not an error.
    s.run("GameTooltip:SetAuctionSellItem()").unwrap();
    assert!(s.take_errors().is_empty());
}
