//! The character window, engine-only: the reference's own `CharacterFrame.xml`,
//! `PaperDollFrame.xml` and `PetPaperDollFrame.xml` off the player's patch chain, loaded through
//! [`super::test_ui::CHARACTER_UI`]. Slot handlers are driven through the mouse: they read the
//! frame from `this` (`PaperDollFrame.lua:647`), and only the engine sets `this`.

use benilla_ui::script::{
    ExtractedQuad, InvSlotView, InventorySlots, QuadContent, ScriptValue, SoundRequest, UiScript,
    UnitCombatStats, UnitState,
};

use super::test_ui::load_ui as load_xml;

fn player_unit() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Benilla".into()),
        health: 100,
        max_health: 100,
        level: 12,
        power_type: 0,
        power: 50,
        max_power: 50,
        dead: false,
        reaction: 0,
        race: Some("Night Elf".into()),
        race_file: Some("NightElf".into()),
        class: Some("Warrior".into()),
        class_file: Some("WARRIOR".into()),
        sex: 2,
        is_player: true,
        player_controlled: true,
        ..Default::default()
    }
}

/// Strength 15 with a +2 buff (a green line), 120 armor, 3 arcane (school 6), no ranged weapon.
fn combat_stats() -> UnitCombatStats {
    UnitCombatStats {
        stats: [15, 12, 20, 8, 9],
        stat_pos: [2, 0, 0, 0, 0],
        resistances: [120, 5, 0, 0, 0, 0, 3],
        min_damage: 10.0,
        max_damage: 15.0,
        attack_power: 80,
        main_attack_time_ms: 2600,
        main_weapon_skill: (300, 5),
        ..Default::default()
    }
}

/// A helm in the head slot (inventory slot 1, `GetInventorySlotInfo("HeadSlot")`), the rest empty.
fn inventory_with_head_item() -> InventorySlots {
    let mut slots: InventorySlots = Default::default();
    slots[1] = Some(InvSlotView {
        duration_ms: None,
        already_bound: false,
        bar_placeable: true,
        durability: None,
        flags: 0,
        item_id: 1234,
        icon: Some("Interface\\Icons\\INV_Helmet_01".into()),
        count: 1,
        contents_count: None,
        quality: 2,
        name: Some("Test Helm".into()),
        link: Some("|cff1eff00|Hitem:1234:0:0:0|h[Test Helm]|h|r".into()),
        locked: false,
        equip_slots: vec![1],
        creator: None,
        enchants: Vec::new(),
    });
    slots
}

/// A backpack whose slot 1 holds a helm that fits only the head slot.
fn backpack_with_fitting_helm() -> benilla_ui::script::ContainerState {
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        benilla_ui::script::ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Helmet_02".into()),
            count: 1,
            quality: Some(3),
            item_id: 2000,
            link: Some("|cff0070dd|Hitem:2000:0:0:0|h[Another Helm]|h|r".into()),
            locked: false,
            equip_slots: vec![1],
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    benilla_ui::script::ContainerState {
        name: Some("Backpack".into()),
        num_slots: 16,
        slots,
    }
}

/// A frame count fingerprints the player's own file, not a target: it moves only when that does.
#[test]
fn shipped_character_frame_loads_clean() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    let mut counts = std::collections::HashMap::new();
    for f in super::test_ui::CHARACTER_UI {
        counts.insert(*f, super::test_ui::load_ui_strict(&s, f));
    }
    assert!(s.errors().is_empty(), "load errors: {:?}", s.errors());
    assert_eq!(
        counts.get("Interface\\FrameXML\\CharacterFrame.xml"),
        Some(&8),
        "the reference's own CharacterFrame.xml"
    );
    assert_eq!(
        counts.get("Interface\\FrameXML\\PaperDollFrame.xml"),
        Some(&75),
        "the reference's own PaperDollFrame.xml"
    );
}

/// Opens with `ToggleCharacter`, the `C` binding's entry point, reads the snapshot lines, hovers,
/// rotates the model and closes again.
#[test]
fn shipped_character_frame_drives_end_to_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The stock tooltip sizes from its measured lines; a fixed-width font stands in for the atlas.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }

    s.set_unit("player", Some(player_unit()));
    s.set_player_combat_stats(Some(combat_stats()));
    s.set_inventory_slots(inventory_with_head_item());

    assert!(
        s.take_sounds().is_empty(),
        "no sound at load (never transitions)"
    );
    assert!(!s.eval::<bool>("return CharacterFrame:IsVisible()").unwrap());

    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    assert!(s.errors().is_empty(), "open errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return CharacterFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoOpen".into())],
        "opening plays igCharacterInfoOpen"
    );

    assert_eq!(
        s.eval::<String>("return CharacterNameText:GetText()")
            .unwrap(),
        "Benilla"
    );
    assert_eq!(
        s.eval::<String>("return CharacterLevelText:GetText()")
            .unwrap(),
        "Level 12 Night Elf Warrior"
    );
    // `PaperDollFrame_SetStats` colours a buffed stat green.
    assert_eq!(
        s.eval::<String>("return CharacterStatFrame1StatText:GetText()")
            .unwrap(),
        "|cff20ff2015|r"
    );
    // Labels are the template's `$parentLabel`, set in Lua (`PaperDollFrame.lua:7-13`, `:140`).
    assert_eq!(
        s.eval::<String>("return CharacterStatFrame1Label:GetText()")
            .unwrap(),
        "Strength:"
    );
    assert_eq!(
        s.eval::<String>("return CharacterAttackFrameLabel:GetText()")
            .unwrap(),
        "Melee Attack"
    );
    // GetLeft is nil until the first resolve.
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return CharacterStatFrame1Label:GetLeft()")
            .unwrap(),
        s.eval::<f32>("return CharacterStatFrame1:GetLeft()")
            .unwrap(),
    );
    // One region, in GameFontNormalSmall: 10 px, 1.0/0.82/0 (`Fonts.xml:97-102`).
    let quads = s.extract();
    let strength_labels: Vec<_> = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Text {
                text: Some(t),
                color,
                font_height,
                ..
            } if t == "Strength:" => Some((*color, *font_height)),
            _ => None,
        })
        .collect();
    assert_eq!(
        strength_labels.len(),
        1,
        "exactly one Strength: label region, got {}",
        strength_labels.len()
    );
    let (color, height) = strength_labels[0];
    assert_eq!(height, Some(10.0), "GameFontNormalSmall is 10px");
    let c = color.expect("the label carries the font object's color");
    assert!(
        (c[0] - 1.0).abs() < 1e-3 && (c[1] - 0.82).abs() < 1e-3 && c[2].abs() < 1e-3,
        "gold NORMAL_FONT_COLOR, got {c:?}"
    );
    assert_eq!(
        s.eval::<String>("return CharacterArmorFrameStatText:GetText()")
            .unwrap(),
        "120"
    );
    // MagicResFrame1 is id 6, arcane (`PaperDollFrame.xml:654`).
    assert_eq!(
        s.eval::<String>("return MagicResText1:GetText()").unwrap(),
        "3"
    );
    // No ranged weapon: `PaperDollFrame_SetRangedAttack` shows N/A.
    assert_eq!(
        s.eval::<String>("return CharacterRangedAttackFrameStatText:GetText()")
            .unwrap(),
        "N/A"
    );

    // The app follows a snapshot push with this event; the slots repaint on the event.
    s.fire_event(
        "UNIT_INVENTORY_CHANGED",
        vec![ScriptValue::Str("player".into())],
    );
    assert!(
        s.errors().is_empty(),
        "inventory refresh errors: {:?}",
        s.errors()
    );
    s.resolve();
    let head_icon = s.extract().iter().any(|q| {
        matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("INV_Helmet_01"))
    });
    assert!(head_icon, "the head slot renders the equipped item's icon");

    // An empty slot's tooltip names the slot.
    let neck_center = {
        let l: f32 = s.eval("return CharacterNeckSlot:GetLeft()").unwrap();
        let r: f32 = s.eval("return CharacterNeckSlot:GetRight()").unwrap();
        let t: f32 = s.eval("return CharacterNeckSlot:GetTop()").unwrap();
        let b: f32 = s.eval("return CharacterNeckSlot:GetBottom()").unwrap();
        ((l + r) * 0.5, (t + b) * 0.5)
    };
    s.mouse_move(neck_center.0, neck_center.1);
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "the empty Neck slot shows a tooltip"
    );

    // The resistance subtext wraps: its `AddLine` passes wrap = 1 (`PaperDollFrame.xml:115`).
    let res_center = {
        let l: f32 = s.eval("return MagicResFrame1:GetLeft()").unwrap();
        let r: f32 = s.eval("return MagicResFrame1:GetRight()").unwrap();
        let t: f32 = s.eval("return MagicResFrame1:GetTop()").unwrap();
        let b: f32 = s.eval("return MagicResFrame1:GetBottom()").unwrap();
        ((l + r) * 0.5, (t + b) * 0.5)
    };
    s.mouse_move(res_center.0, res_center.1);
    assert!(s.errors().is_empty(), "res hover errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "the resistance icon shows a tooltip"
    );
    s.resolve();
    let quads = s.extract();
    let sub_rect = quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text { text: Some(t), .. }
                if t.starts_with("Increases the ability to resist") =>
            {
                q.rect
            }
            _ => None,
        })
        .expect("the resistance subtext line renders");
    let w = sub_rect.right - sub_rect.left;
    assert!(
        w <= benilla_ui::widget::TOOLTIP_WRAP_WIDTH + 0.5,
        "the subtext line wraps at the tooltip wrap column, got width {w}"
    );

    // `Model_RotateRight` adds 0.03 to `Model_OnLoad`'s 0.61 and plays the rotate kit
    // (`UIParent.lua:1421-1442`); the pane's button calls it (`PaperDollFrame.xml:265`).
    s.run("Model_RotateRight(CharacterModelFrame)").unwrap();
    assert!(
        (s.model_pane_facing("CharacterModelFrame") - 0.64).abs() < 0.001,
        "yaw = {}",
        s.model_pane_facing("CharacterModelFrame")
    );
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igInventoryRotateCharacter".into())]
    );

    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    assert!(s.errors().is_empty(), "close errors: {:?}", s.errors());
    assert!(!s.eval::<bool>("return CharacterFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoClose".into())],
        "closing plays igCharacterInfoClose"
    );
}

/// The window's art is `PaperDollFrame`'s BACKGROUND layer, created after the close button; the
/// button's `OnLoad` raises its level by 4 (`CharacterFrame.xml:75`), so its red X paints above.
#[test]
fn close_button_draws_above_the_paper_doll_page() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit("player", Some(player_unit()));
    // extract() emits only shown frames, and the window is authored hidden.
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    s.resolve();

    // extract() sorts by draw order, so a quad's index is its paint position.
    let quads = s.extract();
    let owner = |q: &ExtractedQuad| s.quad_owner_name(q.target);

    let page_last = quads
        .iter()
        .rposition(|q| owner(q).as_deref() == Some("PaperDollFrame"))
        .expect("the paper-doll page renders its background art");

    let close_x = quads
        .iter()
        .position(|q| {
            owner(q).as_deref() == Some("CharacterFrameCloseButton")
                && matches!(&q.content,
                    QuadContent::Texture { path: Some(p), .. } if p.contains("MinimizeButton-Up"))
        })
        .expect("the close button renders its normal (red-X) texture");

    assert!(
        close_x > page_last,
        "close button (draw #{close_x}) must paint after the paper-doll page (last at #{page_last})"
    );
}

/// `PaperDollFrame_SetLevel` writes `PLAYER_LEVEL` to `CharacterLevelText` and `HonorLevelText`
/// (`PaperDollFrame.lua:100-103`); `UNIT_LEVEL` repaints it while the page shows (`:42-49`). A nil
/// race raises in its `format`, and `ui_unit::feed_units` always pushes the player snapshot before
/// `PLAYER_ENTERING_WORLD`, so the test seats one first.
#[test]
fn level_line_reads_the_snapshot_and_repaints_on_unit_level() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit("player", Some(player_unit()));
    s.set_player_combat_stats(Some(combat_stats()));

    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    assert!(s.errors().is_empty(), "open errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<String>("return CharacterLevelText:GetText()")
            .unwrap(),
        "Level 12 Night Elf Warrior"
    );
    // Written by the same call, which is why `HonorFrame.xml` is in the load list.
    assert_eq!(
        s.eval::<String>("return HonorLevelText:GetText()").unwrap(),
        "Level 12 Night Elf Warrior"
    );

    // A level-up as the app sends it: a new snapshot, then `UNIT_LEVEL` for the player.
    let mut leveled = player_unit();
    leveled.level = 13;
    s.set_unit("player", Some(leveled));
    s.fire_event("UNIT_LEVEL", vec![ScriptValue::Str("player".into())]);
    assert!(s.errors().is_empty(), "level-up errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<String>("return CharacterLevelText:GetText()")
            .unwrap(),
        "Level 13 Night Elf Warrior",
        "UNIT_LEVEL repaints the line (PaperDollFrame.lua:48-49)"
    );

    s.resolve();
    let quads: Vec<ExtractedQuad> = s.extract();
    assert!(
        quads.iter().any(|q| matches!(&q.content,
            QuadContent::Text { text: Some(t), .. } if t == "Level 13 Night Elf Warrior")),
        "the level line renders"
    );
}

/// A click picks the item up and locks the slot, and a second click on it cancels. A locked slot's
/// icon is desaturated and tinted 0.5 (`PaperDollFrame.lua:729-737`,
/// `ItemButtonTemplate.lua:61-82`).
#[test]
fn clicking_an_occupied_doll_slot_picks_it_up_and_locks_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit("player", Some(player_unit()));
    s.set_player_combat_stats(Some(combat_stats()));
    s.set_inventory_slots(inventory_with_head_item());
    // Shown, so extract() emits the slot quads.
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    s.take_sounds();
    s.resolve();

    assert!(!s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap());
    assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());

    super::test_ui::click(&mut s, "CharacterHeadSlot", "LeftButton");
    // `PickupInventoryItem` queues `ITEM_LOCK_CHANGED`, whose arm repaints the lock
    // (`PaperDollFrame.lua:601-604`); the tick delivers it.
    s.tick(0.0);
    assert!(s.errors().is_empty(), "click errors: {:?}", s.errors());

    assert!(s.eval::<bool>("return CursorHasItem()").unwrap());
    let (kind, id) = s
        .eval::<(String, i64)>("local k, id = GetCursorInfo() return k, id")
        .unwrap();
    assert_eq!((kind.as_str(), id), ("item", 1234));
    assert!(
        s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap(),
        "the picked slot locks"
    );
    s.resolve();
    let head_icon_dim = s.extract().iter().any(|q| {
        matches!(&q.content, QuadContent::Texture { path: Some(p), color: Some(c), desaturated, .. }
            if p.contains("INV_Helmet_01") && *desaturated && c[0] == 0.5 && c[1] == 0.5 && c[2] == 0.5)
    });
    assert!(head_icon_dim, "the picked slot's icon dims");

    super::test_ui::click(&mut s, "CharacterHeadSlot", "LeftButton");
    s.tick(0.0);
    assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());
    assert!(!s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap());
    s.resolve();
    let head_icon_lit = s.extract().iter().any(|q| {
        matches!(&q.content, QuadContent::Texture { path: Some(p), color: Some(c), desaturated, .. }
            if p.contains("INV_Helmet_01") && !*desaturated && c[0] == 1.0)
    });
    assert!(head_icon_lit, "cancelling clears the dim");
}

/// `CURSOR_UPDATE` locks the highlight on each slot `CursorCanGoInSlot` accepts
/// (`PaperDollFrame.lua:609-616`); the lit ring is the template's `HighlightTexture`
/// (`ItemButtonTemplate.xml:44`). The mouse is parked off the doll, so only a lock lights a ring.
#[test]
fn cursor_update_highlights_fitting_doll_slots_while_holding_a_bag_item() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit("player", Some(player_unit()));
    s.set_player_combat_stats(Some(combat_stats()));
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    super::test_ui::unhover(&mut s);
    s.resolve();

    /// Whether `slot`'s highlight ring is in the render list.
    fn ring_lit(s: &mut UiScript, slot: &str) -> bool {
        s.resolve();
        let quads = s.extract();
        quads.iter().any(|q| {
            s.quad_owner_name(q.target).as_deref() == Some(slot)
                && matches!(&q.content,
                    QuadContent::Texture { path: Some(p), .. } if p.contains("ButtonHilight-Square"))
        })
    }

    assert!(
        !ring_lit(&mut s, "CharacterHeadSlot"),
        "nothing is held yet, so no ring is lit"
    );

    s.set_container(0, Some(backpack_with_fitting_helm()));
    s.run("PickupContainerItem(0, 1)").unwrap();
    s.tick(0.0); // delivers the queued CURSOR_UPDATE
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    // The predicate the arm branches on, pinned beside the effect.
    assert!(s.eval::<bool>("return CursorCanGoInSlot(1)").unwrap());
    assert!(!s.eval::<bool>("return CursorCanGoInSlot(2)").unwrap());

    assert!(
        ring_lit(&mut s, "CharacterHeadSlot"),
        "the fitting slot locks its highlight"
    );
    assert!(
        !ring_lit(&mut s, "CharacterNeckSlot"),
        "a non-fitting slot stays unhighlighted"
    );
}

/// A left-button release on the model pane auto-equips the held item (`PaperDollFrame.lua:31-35`).
#[test]
fn model_pane_click_auto_equips_a_held_bag_item() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit("player", Some(player_unit()));
    s.set_player_combat_stats(Some(combat_stats()));
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    s.take_sounds();
    s.resolve();

    s.set_container(0, Some(backpack_with_fitting_helm()));
    s.run("PickupContainerItem(0, 1)").unwrap();
    assert!(s.eval::<bool>("return CursorHasItem()").unwrap());

    let center = {
        let l: f32 = s.eval("return CharacterModelFrame:GetLeft()").unwrap();
        let r: f32 = s.eval("return CharacterModelFrame:GetRight()").unwrap();
        let t: f32 = s.eval("return CharacterModelFrame:GetTop()").unwrap();
        let b: f32 = s.eval("return CharacterModelFrame:GetBottom()").unwrap();
        ((l + r) * 0.5, (t + b) * 0.5)
    };
    s.mouse_button(center.0, center.1, "LeftButton", true);
    s.mouse_button(center.0, center.1, "LeftButton", false);
    assert!(s.errors().is_empty(), "click errors: {:?}", s.errors());

    assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());
    assert_eq!(s.take_container_autoequips(), vec![(0, 1)]);
}

/// A broken item tints its slot's ring (`UI-Quickslot2`) 0.9,0,0 (`PaperDollFrame.lua:670-672`).
/// The icon is tinted too, but the same update ends in `UpdateLock` (`:714`), whose unlocked
/// `SetItemButtonDesaturated(this, nil)` resets it to white (`ItemButtonTemplate.lua:71-81`).
#[test]
fn broken_equipped_item_tints_its_doll_slot_red() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit("player", Some(player_unit()));
    s.set_player_combat_stats(Some(combat_stats()));

    let mut inv = inventory_with_head_item();
    inv[1].as_mut().unwrap().durability = Some((0, 40));
    s.set_inventory_slots(inv);
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    s.fire_event(
        "UNIT_INVENTORY_CHANGED",
        vec![ScriptValue::Str("player".into())],
    );
    assert!(s.errors().is_empty(), "update errors: {:?}", s.errors());
    s.resolve();
    /// The colour of `slot`'s own quad with `needle` in its path (every slot shares the ring art).
    fn slot_color(s: &mut UiScript, slot: &str, needle: &str) -> Option<[f32; 4]> {
        s.resolve();
        let quads = s.extract();
        quads.iter().find_map(|q| match &q.content {
            QuadContent::Texture {
                path: Some(p),
                color,
                ..
            } if p.contains(needle) && s.quad_owner_name(q.target).as_deref() == Some(slot) => {
                Some(color.unwrap_or([1.0, 1.0, 1.0, 1.0]))
            }
            _ => None,
        })
    }
    let red = [0.9, 0.0, 0.0, 1.0];
    let white = [1.0, 1.0, 1.0, 1.0];
    assert_eq!(
        slot_color(&mut s, "CharacterHeadSlot", "Quickslot2"),
        Some(red),
        "the broken slot's ring paints red"
    );
    assert_eq!(
        slot_color(&mut s, "CharacterNeckSlot", "Quickslot2"),
        Some(white),
        "…and only that slot's — its neighbours share the art and rest white"
    );
    assert_eq!(
        slot_color(&mut s, "CharacterHeadSlot", "INV_Helmet_01"),
        Some(white),
        "the icon's red is overwritten by UpdateLock's SetItemButtonDesaturated(this, nil)"
    );

    let mut inv = inventory_with_head_item();
    inv[1].as_mut().unwrap().durability = Some((40, 40));
    s.set_inventory_slots(inv);
    s.fire_event(
        "UNIT_INVENTORY_CHANGED",
        vec![ScriptValue::Str("player".into())],
    );
    s.resolve();
    assert_eq!(
        slot_color(&mut s, "CharacterHeadSlot", "Quickslot2"),
        Some(white),
        "repair restores the ring"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// Skills and back to Character twice, every click by screen point through the pointer path, with
/// a skill row selected on the way: the Character tab must switch back while Skills is up.
#[test]
fn tab_round_trip_with_a_selected_skill_by_point() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::{SkillEntry, SkillsState};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit("player", Some(player_unit()));
    s.set_skills(SkillsState {
        entries: vec![
            SkillEntry {
                skill_id: 95,
                name: "Defense".into(),
                value: 12,
                max: 60,
                temp_bonus: 0,
                perm_bonus: 0,
                min_level: 0,
                cost_index: 0,
                category_id: 6,
                category_name: "Weapon Skills".into(),
                category_order: 1,
                description: "Defensive expertise.".into(),
                abandonable: false,
                mono: false,
            },
            SkillEntry {
                skill_id: 164,
                name: "Blacksmithing".into(),
                value: 62,
                max: 75,
                temp_bonus: 0,
                perm_bonus: 0,
                min_level: 0,
                cost_index: 0,
                category_id: 11,
                category_name: "Professions".into(),
                category_order: 2,
                description: "Working with metals.".into(),
                // A primary profession: SkillRaceClassInfo flag 0x20, `SKILL_FLAG_UNLEARNABLE`.
                abandonable: true,
                mono: false,
            },
        ],
    });

    // The centre of the visible text quad `text`, after a resolve.
    fn text_center(s: &mut UiScript, text: &str) -> (f32, f32) {
        s.resolve();
        let quads = s.extract();
        let rect = quads
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Text { text: Some(t), .. } if t == text => q.rect,
                _ => None,
            })
            .unwrap_or_else(|| {
                let visible: Vec<String> = quads
                    .iter()
                    .filter_map(|q| match &q.content {
                        QuadContent::Text { text: Some(t), .. } => Some(t.clone()),
                        _ => None,
                    })
                    .collect();
                panic!("no visible text quad {text:?}; visible texts: {visible:?}");
            });
        (
            (rect.left + rect.right) * 0.5,
            (rect.bottom + rect.top) * 0.5,
        )
    }
    fn click(s: &mut UiScript, (x, y): (f32, f32)) {
        s.mouse_move(x, y);
        s.mouse_button(x, y, "LeftButton", true);
        s.mouse_button(x, y, "LeftButton", false);
    }
    let shown = |s: &mut UiScript, name: &str| {
        s.eval::<bool>(&format!("return {name}:IsVisible()"))
            .unwrap()
    };

    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    assert!(shown(&mut s, "PaperDollFrame"), "opens on the doll page");

    let skills_tab = text_center(&mut s, "Skills");
    click(&mut s, skills_tab);
    assert!(shown(&mut s, "SkillFrame"), "Skills tab shows the page");
    assert!(!shown(&mut s, "PaperDollFrame"), "doll page yields");

    // Selecting a row arms the detail pane.
    let row = text_center(&mut s, "Blacksmithing");
    click(&mut s, row);
    assert!(
        s.eval::<i64>("return GetSelectedSkill()").unwrap() > 0,
        "the row click selects"
    );
    assert!(shown(&mut s, "SkillDetailStatusBar"), "detail bar arms");
    // SKILL_DESCRIPTION, with a skillType that is always "" (`SkillFrame.lua:245`).
    assert_eq!(
        s.eval::<String>("return SkillDetailDescriptionText:GetText()")
            .unwrap(),
        "|cffffffff|r Working with metals.",
        "the detail description renders via SKILL_DESCRIPTION"
    );

    // Accepting the unlearn confirm queues CMSG_UNLEARN_SKILL by skill id; the server removes it.
    assert!(
        shown(&mut s, "SkillDetailStatusBarUnlearnButton"),
        "the unlearn button shows for a profession"
    );
    s.run("SkillDetailStatusBarUnlearnButton:Click()").unwrap();
    assert!(
        shown(&mut s, "StaticPopup1"),
        "the UNLEARN_SKILL confirm opens"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Do you want to unlearn Blacksmithing?"
    );
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(
        s.take_skill_abandons(),
        vec![164],
        "accept queues the skill id"
    );
    assert!(!shown(&mut s, "StaticPopup1"), "accept closes the confirm");
    assert_eq!(
        s.eval::<i64>("return GetNumSkillLines()").unwrap(),
        4,
        "nothing is removed locally"
    );

    let defense = text_center(&mut s, "Defense");
    click(&mut s, defense);
    assert!(
        !shown(&mut s, "SkillDetailStatusBarUnlearnButton"),
        "no unlearn button for Defense"
    );
    let row = text_center(&mut s, "Blacksmithing");
    click(&mut s, row);

    let character_tab = text_center(&mut s, "Character");
    assert_eq!(
        s.hit_test_name(character_tab.0, character_tab.1).as_deref(),
        Some("CharacterFrameTab1"),
        "the Character tab OWNS its point while Skills is up (the wheel catcher must not)"
    );
    click(&mut s, character_tab);
    assert!(
        shown(&mut s, "PaperDollFrame"),
        "the Character tab switches back"
    );
    assert!(!shown(&mut s, "SkillFrame"), "Skills page yields");

    let skills_tab = text_center(&mut s, "Skills");
    click(&mut s, skills_tab);
    assert!(shown(&mut s, "SkillFrame"), "second trip to Skills");
    let character_tab = text_center(&mut s, "Character");
    click(&mut s, character_tab);
    assert!(shown(&mut s, "PaperDollFrame"), "second trip back");

    assert!(
        s.errors().is_empty(),
        "zero handler errors across the whole trip: {:?}",
        s.errors()
    );
}

/// A tap clicks on both edges (`PaperDollFrame.xml:243`), 0.03 each; holding spins half a turn a
/// second (`Model_OnUpdate`, `UIParent.lua:1444-1462`). The held branches turn opposite to the
/// click helpers, as in the reference.
#[test]
fn rotate_arrows_tap_twice_and_spin_while_held() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit("player", Some(player_unit()));
    s.set_player_combat_stats(Some(combat_stats()));
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    s.resolve();

    let btn = "CharacterModelFrameRotateLeftButton";
    let (bx, by) = {
        let l: f32 = s.eval(&format!("return {btn}:GetLeft()")).unwrap();
        let r: f32 = s.eval(&format!("return {btn}:GetRight()")).unwrap();
        let t: f32 = s.eval(&format!("return {btn}:GetTop()")).unwrap();
        let b: f32 = s.eval(&format!("return {btn}:GetBottom()")).unwrap();
        ((l + r) * 0.5, (t + b) * 0.5)
    };

    // The press edge is a click: 0.61 - 0.03.
    s.mouse_move(bx, by);
    s.mouse_button(bx, by, "LeftButton", true);
    let pressed = s.model_pane_facing("CharacterModelFrame");
    assert!(
        (pressed - 0.58).abs() < 1e-4,
        "the press edge nudges once: {pressed}"
    );

    // Half a second held is a quarter turn, which the held-left branch adds.
    s.tick(0.5);
    assert!(s.errors().is_empty(), "OnUpdate errors: {:?}", s.errors());
    let spun = s.model_pane_facing("CharacterModelFrame");
    assert!(
        (spun - (pressed + std::f32::consts::FRAC_PI_2)).abs() < 1e-3,
        "half a second held spins half of half a turn: {pressed} → {spun}"
    );

    // The release edge is the second click, and the spin stops.
    s.mouse_button(bx, by, "LeftButton", false);
    let released = s.model_pane_facing("CharacterModelFrame");
    assert!(
        (released - (spun - 0.03)).abs() < 1e-4,
        "the release edge nudges again: {spun} → {released}"
    );
    s.tick(0.5);
    assert!(
        (s.model_pane_facing("CharacterModelFrame") - released).abs() < 1e-6,
        "a released button does not keep spinning"
    );

    // Held right subtracts.
    let rbtn = "CharacterModelFrameRotateRightButton";
    let (rx, ry) = {
        let l: f32 = s.eval(&format!("return {rbtn}:GetLeft()")).unwrap();
        let r: f32 = s.eval(&format!("return {rbtn}:GetRight()")).unwrap();
        let t: f32 = s.eval(&format!("return {rbtn}:GetTop()")).unwrap();
        let b: f32 = s.eval(&format!("return {rbtn}:GetBottom()")).unwrap();
        ((l + r) * 0.5, (t + b) * 0.5)
    };
    s.mouse_move(rx, ry);
    s.mouse_button(rx, ry, "LeftButton", true);
    let before = s.model_pane_facing("CharacterModelFrame");
    s.tick(0.25);
    let after = s.model_pane_facing("CharacterModelFrame");
    assert!(
        (after - (before - std::f32::consts::FRAC_PI_4)).abs() < 1e-3,
        "the right arrow spins the other way: {before} → {after}"
    );
    s.mouse_button(rx, ry, "LeftButton", false);
    assert!(s.errors().is_empty(), "no handler errors: {:?}", s.errors());
}

/// `ToggleCharacter` selects the tab itself (`CharacterFrame.lua:10`), so a keybind's page switch
/// moves the tab row; driven by a bare `ToggleCharacter(page)`, as the `C` binding calls it.
#[test]
fn a_keybind_page_switch_moves_the_tab_row_with_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit("player", Some(player_unit()));

    // `ToggleCharacter` selects the tab by the page's id, so each page's id is the tab that shows
    // it (`CharacterFrame.lua:35-48`). `CHARACTERFRAME_SUBFRAMES` is a set, not the tab order:
    // Skills is its third entry and tab 4 (`CharacterFrame.lua:1`, `CharacterFrame.xml:79-168`).
    let row: [(i64, &str); 5] = [
        (1, "PaperDollFrame"),
        (2, "PetPaperDollFrame"),
        (3, "ReputationFrame"),
        (4, "SkillFrame"),
        (5, "HonorFrame"),
    ];
    for (id, page) in row {
        assert_eq!(
            s.eval::<i64>(&format!("return {page}:GetID()")).unwrap(),
            id,
            "{page} is the page CharacterFrameTab{id} toggles, so it must carry id={id}"
        );
        assert_eq!(
            s.eval::<i64>(&format!("return CharacterFrameTab{id}:GetID()"))
                .unwrap(),
            id,
        );
        // In the list `CharacterFrame_ShowSubFrame` sweeps (`CharacterFrame.lua:25-33`).
        assert!(
            s.eval::<bool>(&format!(
                "for _, v in CHARACTERFRAME_SUBFRAMES do if v == \"{page}\" then return true end \
                 end return false"
            ))
            .unwrap(),
            "{page} must be in CHARACTERFRAME_SUBFRAMES"
        );
    }
    assert_eq!(
        s.eval::<i64>("return getn(CHARACTERFRAME_SUBFRAMES)")
            .unwrap(),
        5,
        "…and there is nothing past the end for a sixth page to arrive into unnoticed"
    );

    let selected = |s: &mut UiScript| {
        s.eval::<i64>("return PanelTemplates_GetSelectedTab(CharacterFrame)")
            .unwrap()
    };
    // A selected tab shows its Disabled art, the active look (`UIPanelTemplates.lua:119-128`).
    let wearing_active_art = |s: &mut UiScript, tab: u32| {
        s.eval::<bool>(&format!(
            "return CharacterFrameTab{tab}MiddleDisabled:IsVisible()"
        ))
        .unwrap()
    };
    let shown = |s: &mut UiScript, name: &str| {
        s.eval::<bool>(&format!("return {name}:IsVisible()"))
            .unwrap()
    };

    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    assert_eq!(selected(&mut s), 1);
    s.run(r#"ToggleCharacter("SkillFrame")"#).unwrap();
    assert!(shown(&mut s, "SkillFrame"));
    assert_eq!(selected(&mut s), 4, "the row follows a keybind to Skills");
    assert!(wearing_active_art(&mut s, 4));
    assert!(!wearing_active_art(&mut s, 1));

    // `C` from the Skills page: the page and the row both go back to Character.
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    assert!(shown(&mut s, "PaperDollFrame"));
    assert!(!shown(&mut s, "SkillFrame"));
    assert_eq!(
        selected(&mut s),
        1,
        "the tab row followed the keybind back to Character"
    );
    assert!(wearing_active_art(&mut s, 1));
    assert!(!wearing_active_art(&mut s, 4));

    // `PanelTemplates_SelectTab` disables the selected tab (`UIPanelTemplates.lua:125`), so a click
    // on the showing page's tab cannot close the window. The disable is asserted before the click:
    // a click whose OnClick never ran would pass the checks after it.
    assert_eq!(
        s.eval::<i64>("return CharacterFrameTab1:IsEnabled()")
            .unwrap(),
        0,
        "the selected tab is DISABLED — that is what makes the click below inert"
    );
    assert_eq!(
        s.eval::<i64>("return CharacterFrameTab4:IsEnabled()")
            .unwrap(),
        1,
        "…while an unselected one stays clickable"
    );
    s.run("CharacterFrameTab1:Click()").unwrap();
    assert!(
        shown(&mut s, "CharacterFrame"),
        "the selected tab is disabled — clicking it cannot close the window"
    );
    assert!(shown(&mut s, "PaperDollFrame"), "…or change the page");
    assert!(s.errors().is_empty(), "no handler errors: {:?}", s.errors());
}

/// The template's `<OnShow>` fits a tab once with `PanelTemplates_TabResize(0)`: its text width
/// plus the two end slices (`CharacterFrameTemplates.xml:78`, `UIPanelTemplates.lua:31-88`), from
/// an authored 115.
#[test]
fn the_five_tabs_fit_their_labels_on_the_first_show() {
    let _data = benilla_formats::wow_data_or_skip!();
    /// `2 * CharacterFrameTab1Left:GetWidth()`: the two 20-unit end slices.
    const SIDES: f64 = 40.0;
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The app always installs a measurer (`AtlasMeasurer`); a bare VM has none.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit("player", Some(player_unit()));

    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    s.resolve();

    let widths = |s: &mut UiScript| -> Vec<(f64, f64)> {
        (1..=5)
            .map(|i| {
                s.eval::<(f64, f64)>(&format!(
                    "return CharacterFrameTab{i}Text:GetStringWidth(), CharacterFrameTab{i}:GetWidth()"
                ))
                .unwrap()
            })
            .collect()
    };
    // `PetTab_Update` hides the pet tab without a pet (`PetPaperDollFrame.lua:189-197`), and a
    // hidden tab gets no OnShow, so it keeps its authored width.
    assert!(
        !s.eval::<bool>("return CharacterFrameTab2:IsVisible()")
            .unwrap(),
        "the pet tab is off with no pet — the rest of this test rests on it"
    );

    let first = widths(&mut s);
    let fitted = |label: f64| label + SIDES;
    for i in [0, 2, 3, 4] {
        let (label, width) = first[i];
        assert!(label > 0.0, "tab {} measured its label", i + 1);
        assert_eq!(
            width,
            fitted(label),
            "tab {} is its text plus the two end slices, from OnShow alone",
            i + 1
        );
        assert_ne!(
            width,
            115.0,
            "tab {} is still at the template's authored pre-fit",
            i + 1
        );
    }
    assert_eq!(
        first[1].1, 115.0,
        "…and the hidden pet tab is not yet fitted"
    );

    // Shown as a pet shows it, it fits on its own first show.
    s.run("CharacterFrameTab2:Show()").unwrap();
    s.resolve();
    let (pet_label, pet_width) = widths(&mut s)[1];
    assert_eq!(
        pet_width,
        fitted(pet_label),
        "the pet tab fits when it is first shown"
    );

    for _ in 0..3 {
        s.tick(0.016);
    }
    s.resolve();
    let settled = widths(&mut s);
    assert_eq!(
        settled[0], first[0],
        "the fit is once, in OnShow — nothing re-fits per frame"
    );
    assert_eq!(&settled[2..], &first[2..], "…for the whole row");
    assert!(s.errors().is_empty(), "no handler errors: {:?}", s.errors());
}

/// `PanelTemplates_Tab_OnClick(frame)` selects `this:GetID()` on `frame`
/// (`UIPanelTemplates.lua:2-4`): `this` is the clicked tab and the owner an argument, as an addon
/// calls it from a tab's `<OnClick>` on a row of its own.
#[test]
fn an_addons_tab_click_selects_through_the_generic_entry_point() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    // `PanelTemplates_SelectTab` asks `GameTooltip:IsOwned(tab)` (`UIPanelTemplates.lua:130`).
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\CharacterFrameTemplates.xml"); // the window tab
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");

    // Tabs named `<frame>Tab1..N`, as `PanelTemplates_UpdateTabs` looks them up, each with its id.
    s.run(
        r#"
        TabKitFrame = CreateFrame("Frame", "TabKitFrame", UIParent)
        PanelTemplates_SetNumTabs(TabKitFrame, 2)
        for i = 1, 2 do
            local t = CreateFrame("Button", "TabKitFrameTab" .. i, TabKitFrame,
                                  "CharacterFrameTabButtonTemplate")
            t:SetID(i)
        end
        PanelTemplates_SetTab(TabKitFrame, 1)
        "#,
    )
    .unwrap();
    let selected = |s: &mut UiScript| {
        s.eval::<i64>("return PanelTemplates_GetSelectedTab(TabKitFrame)")
            .unwrap()
    };
    assert_eq!(selected(&mut s), 1);

    s.run("this = TabKitFrameTab2; PanelTemplates_Tab_OnClick(TabKitFrame); this = nil")
        .unwrap();
    assert_eq!(
        selected(&mut s),
        2,
        "the kit takes the tab's OWN id from `this`, not from the frame it was handed"
    );
    // The row repaints, not just the number.
    assert!(
        s.eval::<bool>("return TabKitFrameTab2MiddleDisabled:IsVisible()")
            .unwrap(),
        "the newly selected tab wears the Active art"
    );
    assert!(s.errors().is_empty(), "no handler errors: {:?}", s.errors());
}
