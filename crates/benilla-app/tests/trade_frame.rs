//! Drives the stock `TradeFrame.xml` through the engine: pushes a synthetic two-sided offer, opens
//! the window with `TRADE_SHOW`, and asserts the Lua paints both columns, the money and the accept
//! glows that follow `TRADE_ACCEPT_UPDATE(my, his)`.

mod common;

use benilla_ui::script::{ScriptValue, TradeSideState, TradeSlotItem, TradeState, UiScript};

/// The trade window's load prefix, in `assets/ui/benilla.toc` order.
const FILES: &[&str] = &[
    "Interface\\FrameXML\\Fonts.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    // `MoneyInputFrameTemplate` and the `MoneyInputFrame_*` functions the window calls.
    r"Interface\FrameXML\MoneyInputFrame.lua",
    r"Interface\FrameXML\MoneyInputFrame.xml",
    r"Interface\FrameXML\UIParent.xml",
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\BasicControls.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\StaticPopup.xml", // the dialog engine
    "Interface\\FrameXML\\GameTooltip.xml",
    // The slot updates go through `ItemButtonTemplate.lua`'s `SetItemButton*` helpers.
    r"Interface\FrameXML\ItemButtonTemplate.xml",
    "Interface\\FrameXML\\TradeFrame.xml",
];

fn load_ui(script: &UiScript) {
    for file in FILES {
        common::load_ui(script, file);
    }
}

fn item(item_id: u32, name: &str, count: u32, quality: u32) -> TradeSlotItem {
    TradeSlotItem {
        item_id,
        name: Some(name.to_string()),
        texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".to_string()),
        count,
        quality: Some(quality),
        enchantment: None,
        link: Some(format!("|cffffffff|Hitem:{item_id}:0:0:0|h[{name}]|h|r")),
    }
}

/// We offer Linen Cloth ×5 and 1g 23s 45c; the partner offers Silk Cloth and 5s.
fn state() -> TradeState {
    let mut player = TradeSideState {
        gold: 12_345,
        ..Default::default()
    };
    player.slots[0] = Some(item(2589, "Linen Cloth", 5, 1));
    let mut target = TradeSideState {
        gold: 500,
        ..Default::default()
    };
    target.slots[0] = Some(item(4306, "Silk Cloth", 1, 2));
    TradeState {
        player,
        target,
        partner_name: Some("Thrall".into()),
    }
}

#[test]
fn trade_frame_loads_and_key_regions_exist() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_ui(&s);
    for name in [
        "TradeFrame",
        "TradePlayerItem1",
        "TradePlayerItem1ItemButton",
        "TradePlayerItem7", // the enchant slot
        "TradeRecipientItem1",
        "TradeRecipientItem7",
        "TradeFrameTradeButton",
        "TradeFrameCancelButton",
        "TradePlayerInputMoneyFrameGold", // our gold is the editable input
        "TradeRecipientMoneyFrameCopperButton",
        "TradeHighlightPlayer",
        "TradeHighlightRecipientEnchant",
    ] {
        assert!(
            s.eval::<bool>(&format!("return getglobal('{name}') ~= nil"))
                .unwrap(),
            "region {name} should exist"
        );
    }
    // The window is hidden until TRADE_SHOW.
    assert!(!s.eval::<bool>("return TradeFrame:IsShown()").unwrap());
}

#[test]
fn trade_show_opens_and_both_columns_populate() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_trade(Some(state()));
    // The stock header reads `UnitName("NPC")` (`TradeFrame.lua:43`); the app points the "npc"
    // unit at the partner.
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Thrall".into()),
            ..Default::default()
        }),
    );

    s.fire_event("TRADE_SHOW", vec![]);
    assert!(
        s.eval::<bool>("return TradeFrame:IsShown()").unwrap(),
        "the window opens on TRADE_SHOW"
    );
    // Slotted into the left panel by its `UIPanelWindows` row (`UIParent.lua:27`); unregistered,
    // `ShowUIPanel` only calls `Show()` and the window never lands on screen.
    assert_eq!(
        s.eval::<String>("return GetLeftFrame() and GetLeftFrame():GetName() or ''")
            .unwrap(),
        "TradeFrame",
        "TRADE_SHOW slots the window into the left panel"
    );

    assert_eq!(
        s.eval::<String>("return TradePlayerItem1Name:GetText()")
            .unwrap(),
        "Linen Cloth"
    );
    assert_eq!(
        s.eval::<String>("return TradeRecipientItem1Name:GetText()")
            .unwrap(),
        "Silk Cloth"
    );
    assert!(
        s.eval::<bool>("return TradePlayerItem1ItemButtonIconTexture:IsShown()")
            .unwrap(),
        "a filled slot shows its icon"
    );
    // An empty slot clears its name (`SetText(nil)`) and hides its icon.
    assert!(s
        .eval::<Option<String>>("return TradePlayerItem2Name:GetText()")
        .unwrap()
        .unwrap_or_default()
        .is_empty());
    assert!(!s
        .eval::<bool>("return TradePlayerItem2ItemButtonIconTexture:IsShown()")
        .unwrap());

    // The partner's name paints from `UnitName("NPC")`, set up above.
    assert_eq!(
        s.eval::<String>("return TradeFrameRecipientNameText:GetText()")
            .unwrap(),
        "Thrall"
    );

    // `TARGET_TRADE` collapses (`MoneyFrame.lua:69`), so 5s shows the silver coin alone.
    assert!(s
        .eval::<bool>(
            "return TradeRecipientMoneyFrameSilverButton:IsShown() \
             and not TradeRecipientMoneyFrameGoldButton:IsShown() \
             and not TradeRecipientMoneyFrameCopperButton:IsShown()"
        )
        .unwrap());

    assert!(s.take_errors().is_empty(), "clean repaint");
}

#[test]
fn enchant_slot_shows_the_not_traded_note() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    let mut st = state();
    // Our enchant slot: index 6, slot 7.
    st.player.slots[6] = Some(item(6217, "Copper Rod", 1, 1));
    s.set_trade(Some(st));
    s.fire_event("TRADE_SHOW", vec![]);
    assert_eq!(
        s.eval::<String>("return TradePlayerItem7Name:GetText()")
            .unwrap(),
        // `TRADEFRAME_NOT_MODIFIED_TEXT` (`TradeFrame.lua:64`).
        "|cffffffffItem not yet modified|r"
    );
    assert!(s.take_errors().is_empty());
}

#[test]
fn accept_update_drives_the_column_glows() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_trade(Some(state()));
    s.fire_event("TRADE_SHOW", vec![]);
    // `TradeFrame_Update` hides all four highlights.
    assert!(!s
        .eval::<bool>("return TradeHighlightRecipient:IsShown()")
        .unwrap());

    // The partner accepts: their column and enchant glow show, ours stay hidden.
    s.fire_event(
        "TRADE_ACCEPT_UPDATE",
        vec![ScriptValue::Int(0), ScriptValue::Int(1)],
    );
    assert!(s
        .eval::<bool>("return TradeHighlightRecipient:IsShown()")
        .unwrap());
    assert!(s
        .eval::<bool>("return TradeHighlightRecipientEnchant:IsShown()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return TradeHighlightPlayer:IsShown()")
        .unwrap());

    // We accept: our glow shows and the Trade button disables.
    s.fire_event(
        "TRADE_ACCEPT_UPDATE",
        vec![ScriptValue::Int(1), ScriptValue::Int(0)],
    );
    assert!(s
        .eval::<bool>("return TradeHighlightPlayer:IsShown()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return TradeFrameTradeButton:IsEnabled() ~= 0")
        .unwrap());
    assert!(s.take_errors().is_empty());
}

#[test]
fn closing_the_window_queues_the_cancel() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_trade(Some(state()));
    s.fire_event("TRADE_SHOW", vec![]);
    let _ = s.take_trade_close();

    // The X hides the window, and its OnHide's `CloseTrade` queues the close.
    s.run("TradeFrameCloseButton:Click()").unwrap();
    assert!(!s.eval::<bool>("return TradeFrame:IsShown()").unwrap());
    assert!(s.take_trade_close(), "closing queued the CloseTrade intent");
    assert!(s.take_errors().is_empty());

    // A server-driven `TRADE_CLOSED` also hides it.
    s.fire_event("TRADE_SHOW", vec![]);
    assert!(s.eval::<bool>("return TradeFrame:IsShown()").unwrap());
    s.fire_event("TRADE_CLOSED", vec![]);
    assert!(!s.eval::<bool>("return TradeFrame:IsShown()").unwrap());
}

#[test]
fn trade_button_click_queues_accept() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_trade(Some(state()));
    s.fire_event("TRADE_SHOW", vec![]);
    let _ = s.take_trade_accept();

    s.run("TradeFrameTradeButton:Click()").unwrap();
    assert!(s.take_trade_accept(), "the Trade button queues AcceptTrade");
    assert!(s.take_errors().is_empty());
}

/// `PLAYER_TRADE_MONEY` fills the three boxes from `GetPlayerTradeMoney()` without re-offering,
/// and a keystroke offers the copper total through `SetTradeMoney`. vmangos never echoes our own
/// gold (it sends it to the partner only, `TradeData.cpp:118`); the app sets it client-side and
/// fires the event.
#[test]
fn player_money_input_reflects_then_offers() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_money(1_000_000); // SetTradeMoney is purse-gated
    load_ui(&s);
    s.set_trade(Some(state())); // state().player.gold == 12345 (1g 23s 45c)
    s.fire_event("TRADE_SHOW", vec![]);

    // The three numeric boxes exist and start empty.
    for box_ in ["Gold", "Silver", "Copper"] {
        assert!(
            s.eval::<bool>(&format!(
                "return getglobal('TradePlayerInputMoneyFrame{box_}') ~= nil"
            ))
            .unwrap(),
            "money box {box_} exists"
        );
    }

    // The programmatic fill must not re-offer.
    let _ = s.take_trade_money();
    s.fire_event("PLAYER_TRADE_MONEY", vec![]);
    assert_eq!(
        s.eval::<(String, String, String)>(
            "return TradePlayerInputMoneyFrameGold:GetText(), \
             TradePlayerInputMoneyFrameSilver:GetText(), \
             TradePlayerInputMoneyFrameCopper:GetText()"
        )
        .unwrap(),
        ("1".into(), "23".into(), "45".into()),
        "the event fills 1g 23s 45c into the boxes"
    );
    assert_eq!(
        s.take_trade_money(),
        None,
        "the guarded reflect does not re-offer"
    );

    // Lift the affordability clamp (`TradeFrame.lua:164`) for a typed offer.
    s.run("GetMoney = function() return 100000000 end").unwrap();
    s.run("TradePlayerInputMoneyFrameGold:SetText('2')")
        .unwrap();
    s.tick(0.0); // the deferred OnTextChanged drains here
    assert_eq!(
        s.take_trade_money(),
        Some(2 * 10000 + 23 * 100 + 45),
        "typing offers the copper total via SetTradeMoney"
    );
    assert!(s.take_errors().is_empty());
}

/// With an empty cursor, clicking our filled slot clears it (`ClickTradeButton`), while the
/// partner's column calls `ClickTargetTradeButton`, which queues nothing (`TradeFrame.xml:101`).
#[test]
fn slot_click_routes_player_to_clear_and_recipient_to_inert() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_trade(Some(state())); // both slot 1s are filled
    s.fire_event("TRADE_SHOW", vec![]);
    let _ = s.take_trade_clear_items();

    s.run("TradePlayerItem1ItemButton:Click()").unwrap();
    assert_eq!(
        s.take_trade_clear_items(),
        vec![1],
        "clicking our filled slot clears it (ClickTradeButton)"
    );

    s.run("TradeRecipientItem1ItemButton:Click()").unwrap();
    assert!(
        s.take_trade_clear_items().is_empty() && s.take_trade_set_items().is_empty(),
        "the partner slot is inert (ClickTargetTradeButton)"
    );
    assert!(s.take_errors().is_empty());
}
