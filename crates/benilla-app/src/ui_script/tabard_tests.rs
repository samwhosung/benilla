//! The stock guild tabard designer (`TabardFrame.xml`): the open event seeds and shows it, the rows
//! cycle the design, Save follows `CanSaveTabardNow`, and closing calls `CloseTabardCreation`.

use benilla_ui::script::{
    emblem_mask_path, TabardHost, TabardIntent, UiScript, UnitGuild, UnitState,
};

/// The player in a guild at `rank`, 0 the master; `GetGuildInfo("player")` reads the unit's guild.
fn harness(rank: u32) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for (token, name, guild) in [
        (
            "player",
            "Probefour",
            Some(UnitGuild {
                name: "Probe Guild".into(),
                rank_name: if rank == 0 { "Guild Master" } else { "Member" }.into(),
                rank_index: rank,
            }),
        ),
        ("npc", "Tabard Vendor", None),
    ] {
        s.set_unit(
            token,
            Some(UnitState {
                exists: true,
                name: Some(name.into()),
                level: 60,
                guild,
                ..Default::default()
            }),
        );
    }
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        "ScrollTemplates.xml", // ours: the scroll kits
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        r"Interface\FrameXML\TabardFrame.xml",
    ] {
        super::test_ui::load_ui(&s, file);
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// `MSG_TABARDVENDOR_ACTIVATE` fires `OPEN_TABARD_FRAME`; the handler seeds the design from the
/// guild record and shows the panel (`TabardFrame.lua:25`).
#[test]
fn the_open_event_seeds_the_design_paints_the_cells_and_shows_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness(0);
    s.set_tabard_host(TabardHost {
        guild_record: Some([12, 4, 1, 2, 9]),
        save_pending: false,
    });
    s.fire_event("OPEN_TABARD_FRAME", vec![]);
    s.resolve();
    assert!(s.eval::<bool>("return TabardFrame:IsVisible()").unwrap());
    assert_eq!(s.tabard_design(), Some([12, 4, 1, 2, 9]));
    assert_eq!(
        s.eval::<String>("return TabardFrameNameText:GetText()")
            .unwrap(),
        "Tabard Vendor"
    );
    let cell = s
        .eval::<String>("return TabardFrameEmblemTopLeft:GetTexture()")
        .unwrap();
    assert_eq!(
        emblem_mask_path(&cell),
        Some(r"Textures\GuildEmblems\Emblem_12_04_TU_U")
    );
    let cell = s
        .eval::<String>("return TabardFrameEmblemBottomRight:GetTexture()")
        .unwrap();
    assert!(cell.ends_with("Emblem_12_04_TL_U"), "{cell}");
    assert!(
        s.eval::<bool>("return TabardFrameAcceptButton:IsEnabled() == 1")
            .unwrap(),
        "a guild master with a cached record can save"
    );
    assert_eq!(
        s.eval::<String>("return TabardFrameGreetingText:GetText()")
            .unwrap(),
        s.eval::<String>("return TABARDVENDORGREETING").unwrap()
    );

    s.run("TabardCustomization_Right(1)").unwrap();
    assert_eq!(s.tabard_design(), Some([13, 4, 1, 2, 9]));
    let cell = s
        .eval::<String>("return TabardFrameEmblemTopRight:GetTexture()")
        .unwrap();
    assert!(cell.ends_with("Emblem_13_04_TU_U"), "{cell}");
    s.run("TabardCustomization_Left(5) TabardCustomization_Left(5)")
        .unwrap();
    assert_eq!(s.tabard_design().unwrap()[4], 7);

    // Save sends the five values; the pending latch greys the button (`TabardFrame.lua:38`).
    s.run("TabardFrameAcceptButton:Click()").unwrap();
    assert_eq!(
        s.take_tabard_intents(),
        vec![TabardIntent::Save([13, 4, 1, 2, 7])]
    );
    s.set_tabard_host(TabardHost {
        guild_record: Some([12, 4, 1, 2, 9]),
        save_pending: true,
    });
    s.fire_event("TABARD_SAVE_PENDING", vec![]);
    assert!(
        !s.eval::<bool>("return TabardFrameAcceptButton:IsEnabled() == 1")
            .unwrap(),
        "the latch greys the button"
    );

    // Cancel hides the panel, whose OnHide calls `CloseTabardCreation` (`TabardFrame.xml:625`).
    s.run("TabardFrameCancelButton:Click()").unwrap();
    s.resolve();
    assert!(!s.eval::<bool>("return TabardFrame:IsVisible()").unwrap());
    assert_eq!(s.take_tabard_intents(), vec![TabardIntent::Close]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// No guild or a rank above 0: the no-guild greeting and no Save (`TabardFrame.lua:97`).
#[test]
fn a_non_master_gets_the_no_guild_greeting_and_no_save() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness(3);
    s.set_tabard_host(TabardHost {
        guild_record: Some([-1; 5]),
        save_pending: false,
    });
    s.fire_event("OPEN_TABARD_FRAME", vec![]);
    assert!(!s
        .eval::<bool>("return TabardFrameAcceptButton:IsEnabled() == 1")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return TabardFrameGreetingText:GetText()")
            .unwrap(),
        s.eval::<String>("return TABARDVENDORNOGUILDGREETING")
            .unwrap()
    );
    s.fire_event("CLOSE_TABARD_FRAME", vec![]);
    s.resolve();
    assert!(!s.eval::<bool>("return TabardFrame:IsVisible()").unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
