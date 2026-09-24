//! The shipped **character window** driven end-to-end, engine-only (no Bevy) — and since decision
//! 1751's character swap, "shipped" means the **reference's own**
//! `Interface\FrameXML\CharacterFrame.xml`, `PaperDollFrame.xml` and `PetPaperDollFrame.xml`, read
//! off the player's patch chain. Our `assets/ui/CharacterFrame.xml` and `PetPaperDollFrame.xml` are
//! deleted; [`super::test_ui::CHARACTER_UI`] is the load list, in `benilla.toc`'s own order, and
//! every test here opens with `wow_data_or_skip!()` because a chain entry needs the install.
//!
//! The fixtures are unchanged (a synthetic player snapshot + combat stats + one equipped item), but
//! the behaviour under them is the stock file's, not ours. Where the two differed the divergence is
//! named at the test that used to pin it, with the stock file and line that replaces it — the
//! per-window re-examination 1751 asks for, done in the open rather than silently.
//!
//! **Handlers are driven through the mouse, never called by name.** The reference's slot handlers
//! read `this` (`PaperDollItemSlotButton_OnClick(button, ignoreModifiers)` takes the MOUSE button
//! and gets the frame from `this`, `PaperDollFrame.lua:647`), and only the engine sets `this`.

use benilla_ui::script::{
    ExtractedQuad, InvSlotView, InventorySlots, QuadContent, ScriptValue, SoundRequest, UiScript,
    UnitCombatStats, UnitState,
};

use super::test_ui::load_ui as load_xml;

/// A Night Elf Warrior, level 12 — the fixture every test below shares for the level/name line.
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

/// A minimal combat-stats snapshot: STR effective 15 (a +2 pos buff, so the stat line shows green),
/// 120 armor (school 0), 3 arcane resistance (school 6), a mainhand-only melee weapon (2.6s speed,
/// 10-15 damage, 80 AP) and NO ranged weapon equipped (exercises the "N/A" ranged fallback).
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

/// One item in the head slot (inventory slot id 1, per `GetInventorySlotInfo("HeadSlot")`) —
/// everything else stays empty (exercises the empty-slot art + slot-name tooltip fallback).
/// `equip_slots: vec![1]` — fits ONLY the head slot, decision 0208 phase 1b's fit rule.
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

/// A one-item backpack whose slot 1 item fits ONLY the head slot (`equip_slots: vec![1]`) — the
/// bag-side half of the doll-interaction tests below.
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

/// The loader itself: every file the window depends on parses and materializes with no errors, and
/// the two chain files this module owns each land their own frame count.
///
/// **A count is a fingerprint of a file, not a target** (decision 1800's closing note). These two
/// are the reference's — `CharacterFrame.xml`'s container + close button + name frame + five tabs,
/// and `PaperDollFrame.xml`'s 19 slots + ammo + their cooldown children + the stat/resistance rows
/// + the model pane and its two rotate buttons. They change only when the player's own file does.
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

/// The whole contract in one end-to-end drive: `ToggleCharacter` (the 'C' binding's entry point)
/// opens through `ShowUIPanel`, the level/name lines read the player snapshot, the stat/armor/
/// resistance lines read the combat-stats snapshot (with the ref's own buff-coloring), the ranged
/// block falls back to N/A with no ranged weapon, an equipped item's icon renders once
/// `UNIT_INVENTORY_CHANGED` fires, rotating the model pane advances the booth yaw + plays the kit,
/// and a second toggle closes it again — each transition its own sound.
#[test]
fn shipped_character_frame_drives_end_to_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The stock tooltip declares no size: it sizes from its lines through the font engine, as
    // the client's does (1968) — a harness that reads its rect needs one; the fixed-width
    // font is that engine here.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }

    s.set_unit("player", Some(player_unit()));
    s.set_player_combat_stats(Some(combat_stats()));
    s.set_inventory_slots(inventory_with_head_item());

    // Hidden at load: no sound queued (never transitions on startup).
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
    // STR effective 15, posBuff 2 > 0 → green (ref PaperDollFrame_SetStats).
    assert_eq!(
        s.eval::<String>("return CharacterStatFrame1StatText:GetText()")
            .unwrap(),
        "|cff20ff2015|r"
    );
    // The labels are the TEMPLATE's own $parentLabel, filled from Lua exactly like the ref
    // (PaperDollFrame_SetStats l.140 for the attributes, _OnLoad l.7-13 for the fixed rows) —
    // never a re-declared region in an instance, which this engine renders as a SECOND
    // anchorless default-font FontString (the white/overlapping-label regression).
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
    // Geometry resolves below (GetLeft is nil until the first resolve).
    s.resolve();
    // The template's label sits LEFT-anchored at its row's left edge.
    assert_eq!(
        s.eval::<f32>("return CharacterStatFrame1Label:GetLeft()")
            .unwrap(),
        s.eval::<f32>("return CharacterStatFrame1:GetLeft()")
            .unwrap(),
    );
    // Exactly ONE region carries the label text, in the small gold font (GameFontNormalSmall:
    // 10px, 1.0/0.82/0) — a duplicate or a default-font fallback both fail here.
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
    // Armor (school 0) reads straight off UnitArmor's base.
    assert_eq!(
        s.eval::<String>("return CharacterArmorFrameStatText:GetText()")
            .unwrap(),
        "120"
    );
    // MagicResFrame1 = id 6 (arcane) — resistances[6] = 3.
    assert_eq!(
        s.eval::<String>("return MagicResText1:GetText()").unwrap(),
        "3"
    );
    // No ranged weapon in this fixture: the N/A fallback (ref PaperDollFrame_SetRangedAttack).
    assert_eq!(
        s.eval::<String>("return CharacterRangedAttackFrameStatText:GetText()")
            .unwrap(),
        "N/A"
    );

    // The equipped head item's icon shows once UNIT_INVENTORY_CHANGED fires (the real app's own
    // sequencing: a snapshot push is followed by the event, never inferred from the push alone).
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

    // Hovering an EMPTY slot (Neck, never populated) shows the slot-name tooltip fallback.
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

    // Hovering a resistance icon WRAPS its long subtext at the tooltip wrap column — the ref
    // passes wrap=1 on every stat/resistance subtext AddLine (ref-PaperDollFrame.xml:115 and
    // siblings); a transcription that drops the flag renders the ~380px sentence as one
    // unwrapped line (the director's tooltip max-width report).
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

    // Rotating the model pane advances the booth yaw from the ref's default 0.61 by +0.03/click and
    // plays the rotate kit. The pane's own rotate button calls `Model_RotateRight(this:GetParent())`
    // (stock `PaperDollFrame.xml:265`), the reference's shared turntable out of `UIParent.lua:1421`,
    // declared in our `UIParent.xml` — our `BenillaPaperDollModel_*` pair is gone with the file
    // that used it.
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

    // ToggleCharacter() again closes it through HideUIPanel, playing the close kit.
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    assert!(s.errors().is_empty(), "close errors: {:?}", s.errors());
    assert!(!s.eval::<bool>("return CharacterFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoClose".into())],
        "closing plays igCharacterInfoClose"
    );
}

/// Regression — the close button's z-order (the reported "missing red X"): the character window
/// carries no border art of its own; its whole frame-and-background IS `PaperDollFrame`'s
/// BACKGROUND layer, a full-window page created AFTER the close button. Every frame defaults to
/// level 0, so within the window draw order is creation order (`order.rs`) — the page's art painted
/// over the earlier-created button and it vanished. The button's `OnLoad` raises its frame level
/// (the same idiom `CharacterNameFrame` uses), so its art must paint AFTER every one of the
/// page's own quads. Checked on the real extracted draw order, not merely the level integer.
#[test]
fn close_button_draws_above_the_paper_doll_page() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit("player", Some(player_unit()));
    // The window must be SHOWN for extract() to emit quads (hidden="true" by default).
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    s.resolve();

    // extract() returns quads already sorted ascending by draw order, so index == paint order.
    let quads = s.extract();
    let owner = |q: &ExtractedQuad| s.quad_owner_name(q.target);

    // The window's page art draws first; its LAST quad is the top-most page pixel.
    let page_last = quads
        .iter()
        .rposition(|q| owner(q).as_deref() == Some("PaperDollFrame"))
        .expect("the paper-doll page renders its background art");

    // The close button's red-X normal texture must render at all …
    let close_x = quads
        .iter()
        .position(|q| {
            owner(q).as_deref() == Some("CharacterFrameCloseButton")
                && matches!(&q.content,
                    QuadContent::Texture { path: Some(p), .. } if p.contains("MinimizeButton-Up"))
        })
        .expect("the close button renders its normal (red-X) texture");

    // … and it must paint AFTER the page — otherwise it's buried, the reported bug.
    assert!(
        close_x > page_last,
        "close button (draw #{close_x}) must paint after the paper-doll page (last at #{page_last})"
    );
}

/// **The level/race/class line, and the one event that repaints it.** `PaperDollFrame_SetLevel`
/// formats `PLAYER_LEVEL` ("Level %d %s %s", `GlobalStrings.lua:3075`) from `UnitLevel`/`UnitRace`/
/// `UnitClass` and writes it to BOTH `CharacterLevelText` and the honor page's `HonorLevelText`
/// "while we at it" (stock `PaperDollFrame.lua:100-103`); `UNIT_LEVEL` for `"player"` is the only
/// event that calls it again (`PaperDollFrame.lua:48-49`), and only while the page is visible
/// (`:42-44`).
///
/// **Retired divergence (decision 1751's character swap).** This test used to be
/// `level_line_survives_no_player_snapshot_yet`: it opened the window with NO player snapshot and
/// pinned `"Level 0 ? ?"`, because our deleted `assets/ui/CharacterFrame.xml` carried `or "?"`
/// placeholders around the two nils. The reference has no such guard — `PaperDollFrame.lua:101`
/// passes `UnitRace("player")` straight into `format`, which raises `bad argument #3 to 'format'`
/// on a nil — so on the stock file that state is not "survived", it is a hard error, and pinning
/// our softening would have been pinning the thing the swap deleted.
///
/// It is also **unreachable in the app**, which is why nothing replaces the guard rather than the
/// expectation moving to "it raises": `ui_unit::feed_units` pushes `set_unit("player", …)` earlier
/// in the same system than it fires `PLAYER_ENTERING_WORLD`, and that fire is gated on our avatar's
/// descriptor existing — while the window can only be opened in-world at all.
/// So the state under test is the one below: a snapshot is always there first.
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
    // The honor page's twin, written by the same call — the reason `HonorFrame.xml` is in
    // CHARACTER_UI at all (its absence loads clean and raises on the first SHOW).
    assert_eq!(
        s.eval::<String>("return HonorLevelText:GetText()").unwrap(),
        "Level 12 Night Elf Warrior"
    );

    // Ding: a new snapshot plus `UNIT_LEVEL` for the player, the app's own sequencing.
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

    // …and it is really on screen, not merely in the FontString.
    s.resolve();
    let quads: Vec<ExtractedQuad> = s.extract();
    assert!(
        quads.iter().any(|q| matches!(&q.content,
            QuadContent::Text { text: Some(t), .. } if t == "Level 13 Night Elf Warrior")),
        "the level line renders"
    );
}

/// Decision 0208 phase 1b, end to end: clicking an OCCUPIED doll slot picks the item up onto the
/// cursor (the SAME payload `GetCursorInfo`/`CursorHasItem` read) and dims the slot's icon
/// (`IsInventoryItemLocked` — the held-here derivation, no server round-trip). A second click on
/// the SAME slot cancels: the cursor empties and the dim clears.
///
/// **Driven through the mouse since decision 1751's character swap**, not `s.run("Handler(frame)")`:
/// the stock handler is `PaperDollItemSlotButton_OnClick(button, ignoreModifiers)`
/// (`PaperDollFrame.lua:647`), which takes the MOUSE button as its first argument and reads the
/// frame off `this` — and only the engine sets `this`. That also puts the template's own
/// `RegisterForClicks("LeftButtonUp", "RightButtonUp")` (`PaperDollFrame.lua:86`) and its
/// `<OnClick>PaperDollItemSlotButton_OnClick(arg1)</OnClick>` (`PaperDollFrame.xml:15-17`) under
/// test, which the old by-name call skipped.
///
/// **Retired divergence:** the dim used to be asserted as "darker than 0.5". The reference's exact
/// value is 0.5 — `PaperDollItemSlotButton_UpdateLock` calls
/// `SetItemButtonDesaturated(this, 1, 0.5, 0.5, 0.5)` (`PaperDollFrame.lua:729-737`), and
/// `ItemButtonTemplate.lua:61-82` both desaturates the icon and vertex-colours it with exactly
/// those numbers. So the assertion is the pair, `desaturated` included.
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
    // The window must be SHOWN for `extract()` to emit its quads below (hidden="true" by
    // default) — the icon-dim check needs a real render pass, not just the engine-side state.
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    s.take_sounds();
    s.resolve();

    assert!(!s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap());
    assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());

    super::test_ui::click(&mut s, "CharacterHeadSlot", "LeftButton");
    // The repaint is one frame behind the pickup, and that is the reference's shape, not a
    // convenience: `PickupInventoryItem` locks the slot and QUEUES `ITEM_LOCK_CHANGED(-1, id)`
    // (`benilla-ui/src/script/cursor/doll.rs:78`, the one transition seam of decision 0216), and
    // that event is what runs `PaperDollItemSlotButton_UpdateLock` (stock
    // `PaperDollFrame.lua:601-604`). The app ticks every frame; a test has to say so.
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
    // The icon desaturates and vertex-dims to the reference's own 0.5 triple.
    s.resolve();
    let head_icon_dim = s.extract().iter().any(|q| {
        matches!(&q.content, QuadContent::Texture { path: Some(p), color: Some(c), desaturated, .. }
            if p.contains("INV_Helmet_01") && *desaturated && c[0] == 0.5 && c[1] == 0.5 && c[2] == 0.5)
    });
    assert!(head_icon_dim, "the picked slot's icon dims");

    // Clicking the SAME slot again cancels — mirrors the bag's own same-slot-cancel contract.
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

/// Decision 0208 phase 1b's `CURSOR_UPDATE` highlight: holding a BAG item that fits ONLY the
/// head slot lights the head slot's highlight ring and leaves every other slot dark (the neck slot
/// checked here). Stock `PaperDollItemSlotButton_OnEvent`'s `CURSOR_UPDATE` arm is
/// `if ( CursorCanGoInSlot(this:GetID()) ) then this:LockHighlight() else this:UnlockHighlight()`
/// (`PaperDollFrame.lua:609-616`).
///
/// **Retired divergence (decision 1751's character swap).** This used to spy on a pair of our own
/// `BenillaPaperDollSlot_LockHighlight`/`_UnlockHighlight` wrappers, which vertex-tinted the
/// Quickslot2 ring because the engine had no `LockHighlight`. It has both verbs now
/// (`benilla-ui/src/script/button.rs`), the stock file calls them, and the emulation is gone with
/// the file that held it — so the assertion is the real locked-highlight state, read where a player
/// reads it: `ItemButtonTemplate`'s own `HighlightTexture`
/// (`Interface\Buttons\ButtonHilight-Square`, `ItemButtonTemplate.xml:44`) is emitted for a locked
/// button and not for an unlocked, unhovered one (`ButtonState::region_visible`). The mouse is
/// parked clear of the doll first so nothing is hovered and the lock is the only thing that can
/// light a ring.
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
    // The window must be SHOWN for extract() to emit the slots' quads at all.
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    super::test_ui::unhover(&mut s);
    s.resolve();

    /// Is `slot`'s highlight ring in the render list? (`None` for "no such owner drew anything".)
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
    s.tick(0.0); // dispatches the queued CURSOR_UPDATE to every registered doll slot
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    // The engine's own predicate agrees with the fixture's `equip_slots: vec![1]` — the input the
    // reference's arm branches on, pinned beside the effect so a wrong answer here can't read as a
    // wiring failure there.
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

/// The model pane's click-with-payload path (ref `CharacterModelFrame_OnMouseUp`): a left-click
/// release on the model pane while holding a fitting BAG item auto-equips it — the queued
/// `(bag, slot)` source, cursor cleared.
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

/// A BROKEN equipped item (durability 0) paints its doll slot's **ring** red — the reference's own
/// 0.9,0,0 on that slot's `NormalTexture` (`Interface\Buttons\UI-Quickslot2`,
/// `ItemButtonTemplate.xml:31`). Repairing it restores the ring to white, and no neighbouring slot
/// is ever tinted. Director-directed with the armor guy.
///
/// **Retired divergence (decision 1751's character swap): the ICON does not go red, and in the
/// reference it never did.** Stock `PaperDollItemSlotButton_Update` really does call BOTH setters
/// on a broken item — `SetItemButtonTextureVertexColor(this, 0.9, 0, 0)` and
/// `SetItemButtonNormalTextureVertexColor(this, 0.9, 0, 0)` (`PaperDollFrame.lua:670-672`) — and
/// `ItemButtonTemplate.lua` paints `$parentIconTexture` from the first and `$parentNormalTexture`
/// from the second (`:53-59` / `:84-90`). But the same `Update` ends with
/// `PaperDollItemSlotButton_UpdateLock()` (`:714`), whose unlocked arm is
/// `SetItemButtonDesaturated(this, nil)` (`:735`), and that function's `if ( not desaturated )`
/// branch overwrites `r,g,b` with 1,1,1 and re-`SetVertexColor`s **the icon**
/// (`ItemButtonTemplate.lua:71-81`). Forty-four lines later in the same call, the icon's red is
/// gone; only the ring survives. Our deleted `assets/ui/CharacterFrame.xml` painted both and made
/// them stick, which is why this test asserted the icon — that half is the transcription's, not the
/// reference's, and it goes with the file. Verified on a real run: the head slot's `INV_Helmet_01`
/// quad extracts white while its `UI-Quickslot2` quad extracts [0.9, 0, 0, 1].
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
    /// The colour of `slot`'s own quad whose texture path contains `needle` — asked per OWNER, so
    /// the nineteen slots that share the ring art can be told apart.
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
    // The icon stays white: see the retired divergence in this test's doc comment.
    assert_eq!(
        slot_color(&mut s, "CharacterHeadSlot", "INV_Helmet_01"),
        Some(white),
        "the icon's red is overwritten by UpdateLock's SetItemButtonDesaturated(this, nil)"
    );

    // Repaired: the ring restores to white.
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

/// The director's own gesture, by screen point (2026-07-17 report: "tab switching flaky, can't
/// switch back to Character from Skills"): open the window, click the Skills tab, click a skill
/// row (arming the detail pane), click the Character tab, and round-trip once more — every click
/// routed through the pointer pipeline (`mouse_move`/`mouse_button`) exactly as the app does, so
/// a frame silently eating the tab row's clicks (or a handler error breaking the chain) fails
/// HERE, not on the director's screen. Zero collected handler errors allowed anywhere.
#[test]
fn tab_round_trip_with_a_selected_skill_by_point() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::{SkillEntry, SkillsState};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    // `ScrollTemplates.xml` and `SkillFrame.xml` used to be loaded again here, because the old
    // per-file preamble did not carry them and a MISSING template is a loader *warning*, not an
    // error (`SkillDetailScrollFrame` inherits `UIPanelScrollFrameTemplate`, so it would have
    // loaded clean with no trough and failed later on geometry that silently never got built).
    // `CHARACTER_UI` carries both now — `CharacterFrame_ShowSubFrame` hides all five pages by name
    // on the first `ToggleCharacter` — and `load_ui_strict` fails on the warning, so a second load
    // would only re-declare frames the list already built.
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
                // The real 5875 split: primary professions carry SkillRaceClassInfo 0x20.
                abandonable: true,
                mono: false,
            },
        ],
    });

    // The center of the unique visible Text quad `text` — resolves layout first, so it's always
    // the CURRENT rect. Panics with the visible-text inventory if absent (a paint regression).
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

    // Tab to Skills — by point, the director's gesture.
    let skills_tab = text_center(&mut s, "Skills");
    click(&mut s, skills_tab);
    assert!(shown(&mut s, "SkillFrame"), "Skills tab shows the page");
    assert!(!shown(&mut s, "PaperDollFrame"), "doll page yields");

    // Select a skill row — arms the detail pane (the report's precondition).
    let row = text_center(&mut s, "Blacksmithing");
    click(&mut s, row);
    assert!(
        s.eval::<i64>("return GetSelectedSkill()").unwrap() > 0,
        "the row click selects"
    );
    assert!(shown(&mut s, "SkillDetailStatusBar"), "detail bar arms");
    // The description body renders through the SKILL_DESCRIPTION format (skillType is "" in every
    // reachable 1.12 branch) — the nil-global half of the report, pinned.
    assert_eq!(
        s.eval::<String>("return SkillDetailDescriptionText:GetText()")
            .unwrap(),
        "|cffffffff|r Working with metals.",
        "the detail description renders via SKILL_DESCRIPTION"
    );

    // The abandon slice: a primary profession offers the unlearn button; its confirm formats the
    // name; accepting queues the CMSG_UNLEARN_SKILL intent BY SKILL ID and removes nothing
    // locally (the server's SetSkill(id,0,0) round trip owns the removal).
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

    // A weapon line never offers it.
    let defense = text_center(&mut s, "Defense");
    click(&mut s, defense);
    assert!(
        !shown(&mut s, "SkillDetailStatusBarUnlearnButton"),
        "no unlearn button for Defense"
    );
    // Restore the profession selection so the round trip below leaves familiar state.
    let row = text_center(&mut s, "Blacksmithing");
    click(&mut s, row);

    // Tab BACK to Character — the reported failure.
    let character_tab = text_center(&mut s, "Character");
    assert_eq!(
        s.hit_test_name(character_tab.0, character_tab.1).as_deref(),
        Some("CharacterFrameTab1"),
        "the Character tab OWNS its point while Skills is up (the wheel catcher must not)"
    );
    click(&mut s, character_tab);
    assert!(
        shown(&mut s, "PaperDollFrame"),
        "the Character tab switches back (the 2026-07-17 report)"
    );
    assert!(!shown(&mut s, "SkillFrame"), "Skills page yields");

    // Once more around — "flaky" only shows up on repetition.
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

/// The rotate arrows, driven by the pointer exactly as a hand does (the director's report:
/// "the 2 arrows for turning barely move it at all"). The ref's feel is TWO mechanisms, and the
/// pane used to ship only the smaller one:
///
/// - a **tap** fires `OnClick` on BOTH edges (`RegisterForClicks("LeftButtonDown","LeftButtonUp")`,
///   ref PaperDollFrame.xml l.241-243) — 2 × 0.03 rad, ~3.4°, which is all a tap is meant to be;
/// - **holding** spins at `ROTATIONS_PER_SECOND` = half a turn per second through the pane's
///   `OnUpdate` (ref `Model_OnUpdate`, UIParent.lua:1444-1462) — 100× the tap per second, and the
///   only reason the arrows feel like anything.
///
/// The ref's held branches also run OPPOSITE to its click helpers (held-LEFT adds where
/// click-LEFT subtracts); that quirk is quoted, so it is asserted here rather than "corrected".
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

    // The left arrow's centre, in screen points.
    let btn = "CharacterModelFrameRotateLeftButton";
    let (bx, by) = {
        let l: f32 = s.eval(&format!("return {btn}:GetLeft()")).unwrap();
        let r: f32 = s.eval(&format!("return {btn}:GetRight()")).unwrap();
        let t: f32 = s.eval(&format!("return {btn}:GetTop()")).unwrap();
        let b: f32 = s.eval(&format!("return {btn}:GetBottom()")).unwrap();
        ((l + r) * 0.5, (t + b) * 0.5)
    };

    // Press and HOLD. The press edge is itself a click (−0.03 off the 0.61 default).
    s.mouse_move(bx, by);
    s.mouse_button(bx, by, "LeftButton", true);
    let pressed = s.model_pane_facing("CharacterModelFrame");
    assert!(
        (pressed - 0.58).abs() < 1e-4,
        "the press edge nudges once: {pressed}"
    );

    // Held for half a second: half of half a turn = +π/2 (the held-LEFT branch ADDS).
    s.tick(0.5);
    assert!(s.errors().is_empty(), "OnUpdate errors: {:?}", s.errors());
    let spun = s.model_pane_facing("CharacterModelFrame");
    assert!(
        (spun - (pressed + std::f32::consts::FRAC_PI_2)).abs() < 1e-3,
        "half a second held spins half of half a turn: {pressed} → {spun}"
    );

    // Release: the second click edge (another −0.03), and the spin stops dead.
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

    // The right arrow spins the other way (held-RIGHT subtracts).
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

/// **The keybind and the tab row must agree** (the director's 2026-08-06 report: *"go on skills tab
/// and press c, it goes back to char tab but without switching the actual tab ui below properly,
/// then if I click on char tab, it closes the whole window"*).
///
/// The cause was a dropped line: the reference selects the tab inside `ToggleCharacter` itself
/// (`PanelTemplates_SetTab(CharacterFrame, subFrame:GetID())`, ref `CharacterFrame.lua:11`), on
/// EVERY path. Ours only did it in the tab button's own `OnClick`, so the two entry points into the
/// window diverged: a keybind switched the page and left the row selecting the old tab, and the
/// next click on the *now-showing* page's tab hit `ToggleCharacter`'s "this page is already
/// visible" branch and shut the window instead of switching back.
///
/// Driven through the same entry point the `C` / `SHIFT-P` bindings use — a bare
/// `ToggleCharacter(page)` call, never the button — because that is precisely the path the row used
/// to miss.
#[test]
fn a_keybind_page_switch_moves_the_tab_row_with_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit("player", Some(player_unit()));

    // THE INVARIANT the fix rests on: a page's `id=` is the slot in this window's tab row that the
    // tab row's own dispatch sends to that page. `ToggleCharacter` selects the row with
    // `PanelTemplates_SetTab(CharacterFrame, subFrame:GetID())` (stock `CharacterFrame.lua:10`), so
    // if a page's id and its tab's id ever disagree, a keybind lights the wrong tab.
    //
    // **Retired divergence (decision 1751's character swap).** This used to loop over
    // `BENILLA_CHARACTERFRAME_SUBFRAMES` asserting `page i must carry id=i`, because our deleted
    // `assets/ui/CharacterFrame.xml` listed the pages in tab order. The reference does not:
    // `CHARACTERFRAME_SUBFRAMES` is `{PaperDoll, PetPaperDoll, Skill, Reputation, Honor}`
    // (`CharacterFrame.lua:1`) while the tab row is Character/Pet/**Reputation**/**Skills**/Honor
    // (`CharacterFrame.xml:79-168`) — index 3 is SkillFrame with id 4, index 4 is ReputationFrame
    // with id 3. That list is a set to iterate for show/hide, not an ordering, so the ordering
    // assertion moves to where the reference really states it: `CharacterFrameTab_OnClick`'s five
    // name branches (`CharacterFrame.lua:35-48`).
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
        // …and it is one of the five the show/hide sweep knows about, so a page cannot arrive with
        // a tab and stay invisible to `CharacterFrame_ShowSubFrame` (`CharacterFrame.lua:25-33`).
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
    // A tab wears its "Active" (=Disabled) art exactly when it is the selected one — the visible
    // half of the same fact, so a SetTab that updated the number without repainting fails here too.
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

    // Open on Character (the `C` binding), then move to Skills the same way.
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    assert_eq!(selected(&mut s), 1);
    s.run(r#"ToggleCharacter("SkillFrame")"#).unwrap();
    assert!(shown(&mut s, "SkillFrame"));
    assert_eq!(selected(&mut s), 4, "the row follows a keybind to Skills");
    assert!(wearing_active_art(&mut s, 4));
    assert!(!wearing_active_art(&mut s, 1));

    // THE REPORT: `C` from the Skills page. The page goes back to Character — and so must the row.
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

    // …and the second half of the report — *"then if I click on char tab, it closes the whole
    // window"* — falls out of the same fix rather than needing its own. `PanelTemplates_SelectTab`
    // DISABLES the tab it selects (a selected tab is not re-clickable), so with the row in step the
    // Character tab is inert while the Character page is up. The window only closed because the row
    // still said Skills: tab 1 was left enabled, the click reached `ToggleCharacter`, and the page
    // it named was already visible — the HideUIPanel arm.
    //
    // The MECHANISM is asserted first and the click second, deliberately: a tab whose OnClick never
    // ran would sail through the click assertions below for the wrong reason, and this is the one
    // fact that cannot.
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

/// **The five tabs fit their labels on the first show, from the reference's own `<OnShow>`** —
/// the whole point of retiring benilla's own copy of `CharacterFrameTabButtonTemplate`.
///
/// The reference fits a tab once, in the template's `<OnShow>`: `PanelTemplates_TabResize(0)`, so
/// `tab = tabText:GetWidth() + 2 * $parentLeft:GetWidth()`. That needs a `GetStringWidth` that
/// answers inside the Lua call that asked; ours landed a frame late when our template was written,
/// which is the only reason that template ever carried an `OnUpdate` settle. The engine has a
/// synchronous measurer now, so the falsifier is a row of tabs still wearing the template's
/// authored 115 after the window opens — and a second one, a width that keeps moving per frame.
#[test]
fn the_five_tabs_fit_their_labels_on_the_first_show() {
    let _data = benilla_formats::wow_data_or_skip!();
    /// `2 * CharacterFrameTab1Left:GetWidth()` — the template's two 20-unit end slices.
    const SIDES: f64 = 40.0;
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The app installs `AtlasMeasurer`; a bare VM installs none, and an OnShow fit against a
    // pending measure is a configuration the app does not have (the same correction 1848 made to
    // the macro harness).
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
    // Tab 2 is the PET tab, and `PetTab_Update` keeps it hidden while the player has no pet
    // (`CharacterFrame.lua`) — a hidden tab never gets an OnShow, so it is still at its authored
    // width until a pet turns it on. That is the reference's own arrangement, and it is asserted
    // below rather than skipped.
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

    // Turn it on the way a pet does, and it fits on ITS first show — same handler, no settle.
    s.run("CharacterFrameTab2:Show()").unwrap();
    s.resolve();
    let (pet_label, pet_width) = widths(&mut s)[1];
    assert_eq!(
        pet_width,
        fitted(pet_label),
        "the pet tab fits when it is first shown"
    );

    // …and it does not move afterwards: the reference fits once, and nothing re-fits per frame.
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

/// **The tab kit's XML-facing entry point, driven the way an addon drives it.**
///
/// Four corpus addons (Enchantrix, Outfitter, SimpleActionSets, TheoryCraft) put
/// `PanelTemplates_Tab_OnClick(<frame>)` straight into a tab's `<OnClick>` and let the kit do the
/// rest; this client had every other member of the kit and not that one, because our own windows
/// wire their tabs to their own handlers and never reached for the generic entry point.
///
/// **It is built on a row of this test's own, and that is deliberate.** When it was written our
/// character row carried no `id=` at all (it was id-based through a `BenillaCharacterFrameTab_OnClick(id)`
/// closure), so driving it through the generic entry point would have selected tab 0. Decision
/// 1751's swap fixed that from the other end — the row is the reference's own now and every tab
/// carries its id (`CharacterFrame.xml:79-168`) — but the standalone row stays: this test is about
/// the KIT, and an addon's window is exactly what it builds, so it must not need a shipped window
/// loaded to run.
///
/// The `this`/frame split is the whole reason the function exists: `this` is the clicked TAB and
/// the owning FRAME is a separate argument, which is the shape `SimpleActionSets.xml:342` relies on
/// when it passes `this:GetParent()`.
#[test]
fn an_addons_tab_click_selects_through_the_generic_entry_point() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    // The reference's `PanelTemplates_SelectTab` ends with `if GameTooltip:IsOwned(tab)` —
    // an arm our deleted copy omitted ("our tabs set no tooltip"), so selecting a tab now needs
    // the tooltip to exist (1860).
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\CharacterFrameTemplates.xml"); // the window tab (1993)
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");

    // A conforming row: tabs named `<frame>Tab1..N` (what `PanelTemplates_UpdateTabs` getglobals)
    // and each carrying its own id, exactly as an addon's XML declares them.
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

    // The addon idiom, verbatim: `this` is the tab, the frame is the argument.
    s.run("this = TabKitFrameTab2; PanelTemplates_Tab_OnClick(TabKitFrame); this = nil")
        .unwrap();
    assert_eq!(
        selected(&mut s),
        2,
        "the kit takes the tab's OWN id from `this`, not from the frame it was handed"
    );
    // The row repainted, not just the number — the same visible half the keybind test asserts.
    assert!(
        s.eval::<bool>("return TabKitFrameTab2MiddleDisabled:IsVisible()")
            .unwrap(),
        "the newly selected tab wears the Active art"
    );
    assert!(s.errors().is_empty(), "no handler errors: {:?}", s.errors());
}
