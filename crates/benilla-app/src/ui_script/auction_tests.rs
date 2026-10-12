//! The stock `Blizzard_AuctionUI` window, engine-only: loaded behind its FrameXML dependencies
//! in `FrameXML.toc` order.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// The stock auction window's sell button: `ClickAuctionSellItemButton` signals
/// `NEW_AUCTION_UPDATE` in place (`0x4ce576`), so `AuctionsFrame`'s handler has painted the
/// attached item's name by the click's next line, with no feed or tick between.
#[test]
fn the_auction_sell_button_paints_the_attached_item_in_place() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(7.0)));
    for f in [
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\MoneyInputFrame.lua",
        r"Interface\FrameXML\MoneyInputFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        "Interface\\FrameXML\\CharacterFrameTemplates.xml",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\AddOns\\Blizzard_AuctionUI\\Blizzard_AuctionUI.xml",
    ] {
        load_xml(&s, f);
    }
    s.set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: [(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: 2589,
                    count: 5,
                    quality: Some(1),
                    texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
                    link: Some("|cffffffff|Hitem:2589|h[Linen Cloth]|h|r".into()),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
        }),
    );
    let name: Option<String> = s
        .eval(
            "PickupContainerItem(0, 1) ClickAuctionSellItemButton() \
             return AuctionsItemButtonName:GetText()",
        )
        .unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert_eq!(name.as_deref(), Some("Linen Cloth"));
}
