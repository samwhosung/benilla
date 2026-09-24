use benilla_ui::script::{
    ContainerMove, ContainerSlot, ContainerState, ItemTemplateView, QuadContent, SoundRequest,
    UiScript,
};

use super::test_ui::{
    bag_slot_button, bag_window, centre_of, click, hover, load_ui as load_xml, unhover, BAG_UI,
};

/// Is bag `id`'s window open? `IsBagOpen` is the reference's own name for the scan over
/// `ContainerFrame1..12`, and it is what these tests ask instead of naming a frame — the windows
/// are RECYCLED (1751), so which `ContainerFrame` a bag lands in is not a property to assert on.
fn open(s: &UiScript, id: i64) -> bool {
    super::test_ui::bag_open(s, id)
}

/// The frame currently showing bag `id`, by name — for the tests that need to reach INTO the
/// window (its slots, its title, its money row). Panics if that bag is not open, which is the
/// right failure: every caller has just opened it.
fn window(s: &UiScript, id: i64) -> String {
    bag_window(s, id).unwrap_or_else(|| panic!("bag {id} is not open"))
}

/// The equipped-bag BAR icons must draw ABOVE the action-bar art, not under it. The bar buttons are
/// relocated onto `MainMenuBarArtFrame` but are top-level frames, so they default to a lower
/// frame level than the bar's own child-hierarchy art (the ExpBar dwarf notches + metal/well art) —
/// which would then paint over the centered icons, leaving the ring but no bag icon. The OnLoad
/// `BenillaActionBarArt_SeatAbove` seats them one level above the art (the action buttons' level).
/// This locks that: no bag-slot icon quad may be covered by a higher-z art texture at its center.
#[test]
fn bag_bar_icons_draw_above_the_action_bar_art() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    // The screen the client defaults to; the action bar centers here and the bag bar lands over its
    // right end, where the dwarf-notch strip overlaps — the exact geometry that reproduced the bug.
    s.set_screen_size(1600.0, 900.0);
    // `BAG_UI` carries ActionBar.xml — both the anchor target (MainMenuBarArtFrame) and the
    // occluder (the ExpBar dwarf art) — in its manifest position.
    for file in BAG_UI {
        load_xml(&s, file);
    }
    s.resolve();
    let quads = s.extract();

    // A bag-slot icon is occluded when any HIGHER-z textured quad (other than the button's own ring)
    // covers its center — i.e. the bar art draws on top of it.
    let occluded = quads
        .iter()
        .filter(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("UI-PaperDoll-Slot-Bag")))
        .filter(|icon| {
            let r = icon.rect.expect("a resolved icon rect");
            let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
            quads.iter().any(|q| {
                q.z > icon.z
                    && matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if !p.contains("UI-Quickslot2"))
                    && q.rect.is_some_and(|qr| qr.left <= cx && cx <= qr.right && qr.bottom <= cy && cy <= qr.top)
            })
        })
        .count();
    assert_eq!(
        occluded, 0,
        "a bag-slot icon is painted over by the action-bar art (the seat-above-the-bar fix regressed)"
    );
}

/// The backpack open/close kits (ContainerFrame.lua ContainerFrame_OnShow/OnHide, l.140 / l.120):
/// showing the window queues igBackPackOpen, hiding it queues igBackPackClose — and nothing queues
/// at load (the frame is authored hidden="true", so it never transitions on startup). Driven through
/// `BenillaBagToggle_OnClick` — the toggle body the bag button's click wrapper calls (the 'B'
/// binding runs the bare `ToggleBackpack()`, the same one hop deeper).
#[test]
fn backpack_toggle_plays_open_and_close_kits() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    s.set_money(0);
    // The backpack has to have slots before `ToggleBag` will open it — its `size > 0` guard.
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::new(),
        }),
    );

    // Hidden at load: no sound queued, every window starts hidden.
    assert!(
        s.take_sounds().is_empty(),
        "no sound at load (never transitions)"
    );
    assert!(!open(&s, 0));

    // Toggle open → OnShow → igBackPackOpen.
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    assert!(open(&s, 0));
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igBackPackOpen".into())],
        "opening the backpack plays igBackPackOpen"
    );

    // Toggle closed → OnHide → igBackPackClose.
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    assert!(!open(&s, 0));
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igBackPackClose".into())],
        "closing the backpack plays igBackPackClose"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **B / the backpack button open the BACKPACK ALONE** (ref ToggleBackpack, ContainerFrame.lua
/// l.67-82) — the director's report, and the fix in 1494. Until then both ran an all-bags toggle
/// and an equipped bag came up beside the backpack every time.
///
/// The reference's shape is deliberately asymmetric, and all three arms are pinned here:
///   * shut → `ToggleBag(0)`: bag 0 and nothing else, however many bags are equipped;
///   * bag 0 OPEN → hide every container window there is (the close arm is the all-bags one);
///   * bag 0 shut but ANOTHER bag open → the condition reads bag 0 specifically, so this is still
///     the open arm: the backpack joins the open bag rather than closing it.
///
/// ESC's CloseAllWindows must still sweep all of them.
#[test]
fn b_opens_the_backpack_alone_and_closes_every_bag() {
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

    // Backpack (16) + one equipped bag in slot 2 (6). Bags 1/3/4 are left unset → 0 slots.
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::new(),
        }),
    );
    s.set_container(
        2,
        Some(ContainerState {
            name: Some("Small Pouch".into()),
            num_slots: 6,
            slots: std::collections::HashMap::new(),
        }),
    );

    // Toggle open: the BACKPACK, and only the backpack — bag 2 is equipped and stays shut.
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    assert!(open(&s, 0), "backpack opens");
    assert!(
        !open(&s, 2),
        "the equipped bag stays shut — this is the backpack's own toggle, not open-all"
    );
    assert!(
        !open(&s, 1) && !open(&s, 3) && !open(&s, 4),
        "and the empty slots have no window to show either way"
    );

    // The close arm IS the all-bags one: with bag 2 also open by some other path, one toggle
    // takes the lot down.
    s.run("ToggleBag(2)").unwrap();
    assert!(open(&s, 2));
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    assert!(
        !open(&s, 0) && !open(&s, 2),
        "bag 0 open ⇒ the toggle hides every container window"
    );

    // Bag 0 shut, bag 2 open: the ref's condition reads bag 0, so this is the OPEN arm — the
    // backpack joins the bag already up instead of closing it.
    s.run("ToggleBag(2)").unwrap();
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    assert!(
        open(&s, 0) && open(&s, 2),
        "with only another bag open, B opens the backpack beside it"
    );

    // ESC's CloseAllWindows sweeps every open bag, not just the backpack.
    s.run("CloseAllWindows()").unwrap();
    assert!(
        !open(&s, 0) && !open(&s, 2),
        "CloseAllWindows hides every bag window"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **SHIFT-B / a shift-click on the backpack button open ALL of them** (ref OpenAllBags,
/// ContainerFrame.lua l.662-700) — the other half of the split 1494 restored. It is a TOGGLE, and
/// its arms are decided by a COUNT, not by "is anything open":
///   * anything less than everything open → open the lot (the backpack plus each equipped bag);
///   * everything already open → the counting pass IS the close;
///   * `forceOpen` → always the open arm (what a vendor window would pass).
///
/// The keyring is swept by the counting pass but never counted and never opened — the reference's
/// own `GetID() ~= KEYRING_CONTAINER` exclusion.
#[test]
fn shift_b_toggles_every_bag_at_once() {
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
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::new(),
        }),
    );
    s.set_container(
        2,
        Some(ContainerState {
            name: Some("Small Pouch".into()),
            num_slots: 6,
            slots: std::collections::HashMap::new(),
        }),
    );

    // From nothing open: the backpack AND the equipped bag; the empty slots have no window.
    s.run("OpenAllBags()").unwrap();
    let _ = s.take_sounds();
    assert!(open(&s, 0) && open(&s, 2));
    assert!(
        !open(&s, 1) && !open(&s, 3) && !open(&s, 4),
        "empty bag slots (no container) have no window to show"
    );
    assert!(
        !open(&s, -2),
        "the keyring is not one of your bags — open-all never opens it"
    );

    // Everything open ⇒ the next one closes the lot.
    s.run("OpenAllBags()").unwrap();
    let _ = s.take_sounds();
    assert!(!open(&s, 0) && !open(&s, 2));

    // PARTIAL is the open arm, not the close arm — the count is what decides. Backpack alone up
    // (1 of 2) ⇒ open-all still opens rather than closing what is there.
    s.run("ToggleBag(0)").unwrap();
    s.run("OpenAllBags()").unwrap();
    let _ = s.take_sounds();
    assert!(
        open(&s, 0) && open(&s, 2),
        "1 of 2 open is not 'all open': the lot opens"
    );

    // An open keyring rides the sweep down without ever counting toward "all open" — so this
    // still reads 2-of-2 and closes.
    s.run("ToggleKeyRing()").unwrap();
    assert!(open(&s, -2));
    s.run("OpenAllBags()").unwrap();
    let _ = s.take_sounds();
    assert!(
        !open(&s, 0) && !open(&s, 2),
        "the keyring does not count as an open bag"
    );
    assert!(!open(&s, -2), "…but it IS swept by the same pass");

    // forceOpen skips the close arm: from all-open, everything stays open.
    s.run("OpenAllBags(1)").unwrap();
    s.run("OpenAllBags(1)").unwrap();
    let _ = s.take_sounds();
    assert!(
        open(&s, 0) && open(&s, 2),
        "forceOpen means open, never toggle"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// An open bag LIGHTS its bar button (the CheckButton ring — ref ContainerFrame_OnShow/OnHide
/// SetChecked, l.124-131/84-95), and any close clears it: the windows are the source of truth,
/// so the ring tracks opens from every path (the all-toggle, a bar click, ESC's sweep).
#[test]
fn bag_bar_buttons_light_while_their_bag_is_open() {
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
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::new(),
        }),
    );
    s.set_container(
        2,
        Some(ContainerState {
            name: Some("Small Pouch".into()),
            num_slots: 6,
            slots: std::collections::HashMap::new(),
        }),
    );
    let checked = |s: &mut UiScript, name: &str| {
        s.eval::<bool>(&format!("return {name}:GetChecked() and true or false"))
            .unwrap()
    };

    // Open-all (SHIFT-B's knob since 1494 — the backpack button alone would light only its own
    // ring now): the backpack button and the equipped bag's slot light; the empty slots don't.
    s.run("OpenAllBags()").unwrap();
    assert!(
        checked(&mut s, "MainMenuBarBackpackButton"),
        "backpack ring lights"
    );
    assert!(checked(&mut s, "CharacterBag1Slot"), "bag 2's ring lights");
    assert!(
        !checked(&mut s, "CharacterBag0Slot"),
        "empty slot stays dark"
    );

    // ...and the rings actually EMIT (extract-level): exactly two CheckButtonHilight quads, the
    // toggle's owner-sized on the 37px button at the art frame's BOTTOMRIGHT −6,2 (art right
    // edge = 1024 at this screen ⇒ x[981,1018] y[2,39]).
    s.resolve();
    let rings: Vec<_> = s
        .extract()
        .into_iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("CheckButtonHilight"))
        })
        .collect();
    assert_eq!(rings.len(), 2, "toggle + bag 2 rings emit, nothing else");
    let toggle_ring = rings
        .iter()
        .find_map(|q| q.rect.filter(|r| r.left == 981.0))
        .expect("the toggle's ring at the art frame's corner");
    assert_eq!(
        (
            toggle_ring.left,
            toggle_ring.bottom,
            toggle_ring.right,
            toggle_ring.top
        ),
        (981.0, 2.0, 1018.0, 39.0)
    );

    // Closing ONE window (its close button / any Hide path) clears just its ring.
    s.run("CloseBag(2)").unwrap();
    assert!(
        !checked(&mut s, "CharacterBag1Slot"),
        "closing bag 2 clears its ring"
    );
    assert!(
        checked(&mut s, "MainMenuBarBackpackButton"),
        "the backpack ring stays"
    );

    // A bar-slot click reopens bag 2 and relights it (the click auto-toggle + the ref's
    // re-derive tail agree here). Driven as a RIGHT-click through the real input path: the ref's
    // BagSlotButtonTemplate inherits PaperDollItemSlotButtonTemplate, whose OnLoad registers
    // ("LeftButtonUp", "RightButtonUp") — PaperDollFrame.lua:86 — and BagSlotButton_OnClick reads
    // no button, so either one opens the bag. Ours registered LeftButtonUp only until 0908.
    let (cx, cy): (f64, f64) = s
        .eval(
            "return (CharacterBag1Slot:GetLeft() + CharacterBag1Slot:GetRight()) / 2, \
                    (CharacterBag1Slot:GetTop() + CharacterBag1Slot:GetBottom()) / 2",
        )
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    assert!(
        checked(&mut s, "CharacterBag1Slot"),
        "a RIGHT-click on the bar slot reopens bag 2, exactly as a left one does"
    );

    // ESC's sweep closes everything → every ring dark.
    s.run("CloseAllWindows()").unwrap();
    assert!(
        !checked(&mut s, "MainMenuBarBackpackButton") && !checked(&mut s, "CharacterBag1Slot"),
        "close-all clears every ring"
    );
    let _ = s.take_sounds();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A slot on the RIGHT half of the screen hangs its tooltip LEFT — the ref's own screen-edge
/// answer (ContainerFrameItemButton_OnEnter, ContainerFrame.lua:602-612 side-pick), which is what
/// keeps a bag tooltip from running off the right edge (the bag lives at the bottom-right).
#[test]
fn bag_tooltip_hangs_left_when_the_slot_sits_in_the_right_half() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The stock tooltip declares no size: it sizes from its lines through the font engine, as
    // the client's does (1968) — a harness that reads its rect needs one; the fixed-width
    // font is that engine here.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_ThrowingKnife_02".into()),
            count: 200,
            quality: Some(1),
            item_id: 2947,
            link: Some("|cffffffff|Hitem:2947|h[Small Throwing Knife]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    s.take_sounds();
    s.resolve();

    // The engine speaks 1.12: GetScreenWidth serves the host-set root width.
    assert_eq!(s.eval::<f64>("return GetScreenWidth()").unwrap(), 1024.0);
    // The bag window anchors bottom-right, so every slot button is in the right half. The bag
    // numbers its buttons visually (reversed from container slots) — find the button showing
    // container slot 1, where the fixture item lives.
    let btn = bag_slot_button(&s, 0, 1);
    let ok: bool = s
        .eval(&format!("return {btn}:GetRight() >= GetScreenWidth() / 2"))
        .unwrap();
    assert!(ok, "fixture: the slot must sit in the right half");

    hover(&mut s, &btn);
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());

    assert!(s.eval::<bool>("return GameTooltip:IsVisible()").unwrap());
    s.resolve();
    // ANCHOR_LEFT seats the tooltip's BOTTOMRIGHT on the slot's TOPLEFT: the whole tooltip stays
    // left of the slot, i.e. on-screen — never past the right edge.
    let ok: bool = s
        .eval(&format!(
            "return GameTooltip:GetRight() <= {btn}:GetLeft() \
               and GameTooltip:GetRight() <= GetScreenWidth()"
        ))
        .unwrap();
    assert!(ok, "tooltip hangs LEFT of a right-half slot");
}

/// A tooltip opened while the item's template is still in flight repaints itself the moment the
/// stats land — no re-hover. The refresh loop is the ref's own (ContainerFrameItemButton_OnUpdate,
/// ContainerFrame.lua:645-660: re-run OnEnter every frame while `GameTooltip:IsOwned(this)`), and
/// hiding the tooltip drops ownership so the loop can never resurrect it.
#[test]
fn hovered_bag_tooltip_fills_itself_when_the_stats_land() {
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

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Sword_04".into()),
            count: 1,
            quality: Some(1),
            item_id: 25,
            link: Some("|cffffffff|Hitem:25|h[Worn Shortsword]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    s.resolve();
    let btn = bag_slot_button(&s, 0, 1);

    // Hover with the stats store empty: the fallback one-line tooltip, and the miss recorded.
    hover(&mut s, &btn);
    assert!(s.eval::<bool>("return GameTooltip:IsVisible()").unwrap());
    assert_eq!(
        s.eval::<i64>("return GameTooltip:NumLines()").unwrap(),
        1,
        "in-flight template: the name-only fallback line"
    );
    assert_eq!(s.take_item_stat_asks(), vec![25], "the miss asks the app");

    // The template lands (the app's arrival-driven push) → the very next frame's OnUpdate
    // re-enter repaints the OPEN tooltip with the full stat head.
    s.set_item_template(
        25,
        ItemTemplateView {
            name: "Worn Shortsword".into(),
            quality: 1,
            inventory_type: 21,
            class: 2,
            subclass: 7,
            damages: vec![(1.0, 3.0, 0)],
            delay_ms: 1900,
            ..Default::default()
        },
    );
    s.tick(0.016);
    assert!(
        s.eval::<i64>("return GameTooltip:NumLines()").unwrap() > 1,
        "the stats landing repainted the open tooltip"
    );
    let has_damage: bool = s
        .eval(
            "for i = 1, GameTooltip:NumLines() do \
               local fs = getglobal(\"GameTooltipTextLeft\" .. i) \
               if fs and string.find(fs:GetText() or \"\", \"Damage\") then return true end \
             end return false",
        )
        .unwrap();
    assert!(
        has_damage,
        "the repaint carries the stat head's damage line"
    );

    // Leaving drops ownership: the loop must not resurrect the hidden tooltip.
    unhover(&mut s);
    s.tick(0.016);
    assert!(
        !s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "hide drops ownership; OnUpdate never resurrects"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// At a vendor, a bag hover shows the engine-truth sell-price money row (SellPrice × stack,
/// 0x52b650@0x52e376) — or the ITEM_UNSELLABLE "No sell price" line — and arms
/// the pouch cursor (ShowContainerSellCursor 0x4fa460 → Buy over a Point base);
/// leaving resets it.
#[test]
fn vendor_bag_hover_shows_sell_price_and_arms_the_pouch_cursor() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::{MerchantState, ScriptValue, UiCursorMode};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Misc_Pelt_Wolf_01".into()),
            count: 4,
            quality: Some(1),
            item_id: 2318,
            link: Some("|cffffffff|Hitem:2318|h[Light Leather]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    slots.insert(
        2,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Misc_Key_03".into()),
            count: 1,
            quality: Some(1),
            item_id: 9999,
            link: Some("|cffffffff|Hitem:9999|h[Shadowforge Key]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );
    // Sellable stack: 13c each × 4 = 52c. Unsellable: SellPrice 0.
    s.set_item_template(
        2318,
        ItemTemplateView {
            name: "Light Leather".into(),
            quality: 1,
            sell_price: 13,
            ..Default::default()
        },
    );
    s.set_item_template(
        9999,
        ItemTemplateView {
            name: "Shadowforge Key".into(),
            quality: 1,
            sell_price: 0,
            ..Default::default()
        },
    );
    s.set_merchant(Some(MerchantState::default()));
    // MERCHANT_SHOW already opens the bags (the window's OnShow calls `OpenBackpack`, decision
    // 0561), so this needs no toggle of its own — and must not have one: with bag 0 already up,
    // `ToggleBackpack` takes its close-all arm and the window this test hovers in would go away.
    s.fire_event("MERCHANT_SHOW", vec![ScriptValue::Str("Vendor".into())]);
    s.take_sounds();
    s.resolve();
    let b1 = bag_slot_button(&s, 0, 1);
    let b2 = bag_slot_button(&s, 0, 2);

    // The sellable stack: a money row (52c → the copper coin slot shows "52") + the pouch armed.
    hover(&mut s, &b1);
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>(
            "return GameTooltipMoneyFrame:IsShown() \
             and GameTooltipMoneyFrameCopperButton:GetText() == '52'",
        )
        .unwrap());
    assert_eq!(
        s.ui_cursor(),
        Some(UiCursorMode::Buy),
        "the pouch cursor is armed over a sellable item"
    );

    // Leaving resets the cursor and the money row dies with the tooltip.
    unhover(&mut s);
    assert_eq!(s.ui_cursor(), None, "ResetCursor on leave");
    assert!(s
        .eval::<bool>("return not GameTooltipMoneyFrame:IsShown()")
        .unwrap());

    // The unsellable item: the ITEM_UNSELLABLE line, no coins.
    hover(&mut s, &b2);
    let has_line: bool = s
        .eval(
            "for i = 1, GameTooltip:NumLines() do \
               if (getglobal('GameTooltipTextLeft' .. i):GetText() or '') == 'No sell price' \
                 then return true end \
             end return false",
        )
        .unwrap();
    assert!(has_line, "SellPrice 0 shows the ITEM_UNSELLABLE line");
    assert!(s
        .eval::<bool>("return not GameTooltipMoneyFrame:IsShown()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A readable bag item (a mail permanent copy — the instance carries item text) shows the
/// Inspect magnifier on hover (ref ContainerFrameItemButton_OnEnter, ContainerFrame.lua l.638:
/// `this.readable → ShowInspectCursor()`); a plain item leaves the base cursor; leaving resets.
#[test]
fn readable_letter_hover_shows_the_inspect_magnifier() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::UiCursorMode;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Note_01".into()),
            count: 1,
            quality: Some(1),
            item_id: 8383,
            link: Some("|cffffffff|Hitem:8383|h[Plain Letter]|h|r".into()),
            readable: true,
            ..Default::default()
        },
    );
    slots.insert(
        2,
        ContainerSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            count: 5,
            quality: Some(1),
            item_id: 117,
            link: Some("|cffffffff|Hitem:117|h[Tough Jerky]|h|r".into()),
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    s.take_sounds();
    s.resolve();
    let b1 = bag_slot_button(&s, 0, 1);
    let b2 = bag_slot_button(&s, 0, 2);

    hover(&mut s, &b1);
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    assert_eq!(
        s.ui_cursor(),
        Some(UiCursorMode::Inspect),
        "the magnifier over the letter"
    );
    unhover(&mut s);
    assert_eq!(s.ui_cursor(), None, "ResetCursor on leave");

    hover(&mut s, &b2);
    assert_eq!(s.ui_cursor(), None, "no magnifier over the jerky");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The drag trio: a real press-drag-release across two slot buttons routes
/// through the SAME `ContainerFrameItemButton_OnClick("LeftButton")` path a two-click pickup/place
/// does —
/// unlike every other bag test here, which calls the Lua click handler directly, this one drives
/// actual `mouse_button`/`mouse_move` so the `RegisterForDrag`/`OnDragStart`/`OnReceiveDrag` XML
/// wiring itself is under test, not just the handler body.
#[test]
fn drag_across_two_slots_queues_the_same_move_a_click_pickup_would() {
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

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            count: 5,
            quality: Some(3),
            item_id: 117,
            link: Some("|cffffffff|Hitem:117|h[Tough Jerky]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    s.take_sounds();
    s.resolve();

    let b1 = bag_slot_button(&s, 0, 1);
    let b5 = bag_slot_button(&s, 0, 5);
    let (x1, y1): (f64, f64) = s
        .eval(&format!(
            "return ({b1}:GetLeft() + {b1}:GetRight()) / 2, \
                        ({b1}:GetTop() + {b1}:GetBottom()) / 2"
        ))
        .unwrap();
    let (x5, y5): (f64, f64) = s
        .eval(&format!(
            "return ({b5}:GetLeft() + {b5}:GetRight()) / 2, \
                        ({b5}:GetTop() + {b5}:GetBottom()) / 2"
        ))
        .unwrap();

    // Press on slot 1 (picks up), drag past the threshold onto slot 5, release there.
    s.mouse_button(x1 as f32, y1 as f32, "LeftButton", true);
    s.mouse_move(x5 as f32, y5 as f32);
    let consumed = s.mouse_button(x5 as f32, y5 as f32, "LeftButton", false);
    assert!(consumed, "the drag release lands on a mouse-enabled frame");

    assert!(s.cursor_item().is_none(), "placed onto the empty slot 5");
    assert_eq!(
        s.take_container_moves(),
        vec![ContainerMove {
            src_bag: 0,
            src_slot: 1,
            dst_bag: 0,
            dst_slot: 5,
            count: None,
        }]
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A second bag window (decision 0216 slice 2): bag 1's snapshot feeds through the SAME
/// container/BenillaBagWindow_Update plumbing the backpack uses, opened via the bag-bar path
/// (`BenillaBagBarSlot_OnClick`, not the backpack toggle) and painting its own slot 1 icon.
#[test]
fn a_second_bag_window_feeds_and_paints_via_the_bag_bar() {
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

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Misc_Gem_01".into()),
            count: 1,
            quality: Some(2),
            item_id: 200,
            link: Some("|cffffffff|Hitem:200|h[Shiny Gem]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    s.set_container(
        1,
        Some(ContainerState {
            name: Some("Small Pouch".into()),
            num_slots: 6,
            slots,
        }),
    );

    assert!(!open(&s, 1), "hidden by default");
    // CharacterBag0Slot == bag id 1: the stock bar's handlers carry no `bagId` field and
    // recompute `this:GetID() - CharacterBag0Slot:GetID() + 1` (see `:2119` below).
    s.run("CharacterBag0Slot:Click()").unwrap();
    let _ = s.take_sounds();
    assert!(open(&s, 1), "the bag-bar click opened bag 1's window");
    let w = window(&s, 1);
    assert_eq!(
        s.eval::<String>(&format!("return {w}Name:GetText()"))
            .unwrap(),
        "Small Pouch",
        "the title reads the live GetBagName"
    );

    s.resolve();
    let painted = s.extract().iter().any(|q| {
        matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("INV_Misc_Gem_01"))
    });
    assert!(painted, "bag 1's slot 1 icon is on screen");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// An equipped bag's window SNUG-FITS its row count (BenillaBagWindow_FitBackground) instead of the
/// backpack's fixed 5-row/260 slab. The heights are the real client's, from
/// `ContainerFrame_GenerateFrame` in the shipped `Interface\FrameXML\ContainerFrame.lua`:
/// `height = topH + ((rows-1)*41 - 9) + 10`, with `topH` = 72 for a size%4==2 bag (its own plus-two
/// top band), 86 for a single full row, else 94. The `-9` is the reference's `firstRowPixelOffset`
/// and the `10` its fixed bottom rim; both are load-bearing — dropping the offset slides the rim a
/// row-fraction low and bleeds the next row's wells in above it. This locks that arithmetic AND the
/// core fix: a small bag is far shorter than the old fixed height.
#[test]
fn equipped_bag_window_snug_fits_its_row_count() {
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

    // (bag id, slot count, expected window height). 6 → 2 rows plus-two-top (72+32+10); 8 → 2 rows
    // full-top (94+32+10); 10 → 3 rows plus-two-top (72+73+10); 20 → 5 rows full-top (94+155+10).
    // The last two exercise the no-middle fork: 4 → one full row (86+0+10); 2 → one plus-two row
    // (72+0+10). Bag 1 stays at 6 so the h6 assertion below still reads the pouch.
    for (bag, size, expected) in [
        (1, 6, 114.0),
        (2, 8, 136.0),
        (3, 10, 155.0),
        (4, 20, 259.0),
        (2, 4, 96.0),
        (3, 2, 82.0),
    ] {
        s.set_container(
            bag,
            Some(ContainerState {
                name: Some(format!("Bag {bag}")),
                num_slots: size,
                slots: std::collections::HashMap::new(),
            }),
        );
        // The window is SIZED at generation, so re-open it after each resize: `ToggleBag` twice
        // is the reference's own way of regenerating a frame for a bag whose size changed.
        if open(&s, bag) {
            s.run(&format!("ToggleBag({bag})")).unwrap();
        }
        s.run(&format!("ToggleBag({bag})")).unwrap();
        let _ = s.take_sounds();
        let frame = window(&s, bag);
        let h = s
            .eval::<f64>(&format!("return {frame}:GetHeight()"))
            .unwrap();
        assert!(
            (h - expected).abs() < 0.5,
            "bag {bag} ({size} slots): height {h}, expected {expected}"
        );
        s.run(&format!("ToggleBag({bag})")).unwrap();
        let _ = s.take_sounds();
    }
    // The core regression: a 6-slot bag is much shorter than the old fixed 260-tall slab.
    s.run("ToggleBag(1)").unwrap();
    let _ = s.take_sounds();
    let w1 = window(&s, 1);
    let h6 = s.eval::<f64>(&format!("return {w1}:GetHeight()")).unwrap();
    assert!(
        h6 < 200.0,
        "a 6-slot bag must not fill the old 260 slab, got {h6}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A fixture backpack with a 5-stack of Tough Jerky in slot 1, the bag opened — shared setup for
/// the shift-click/split tests below. Returns the slot-1 button's screen center.
fn open_backpack_with_a_five_stack(s: &mut UiScript) -> (f32, f32) {
    for file in BAG_UI {
        load_xml(s, file);
    }
    load_xml(s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(s, "Interface\\FrameXML\\StackSplitFrame.xml");
    s.set_money(0);

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            count: 5,
            quality: Some(1),
            item_id: 117,
            link: Some("|cffffffff|Hitem:117|h[Tough Jerky]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    s.resolve();

    let btn = bag_slot_button(s, 0, 1);
    centre_of(s, &btn)
}

/// The stack-split trigger — SHIFT + left-click on an unlocked stack of ≥2, the reference fork
/// verbatim (ContainerFrame.lua:567-577), driven through the `set_modifiers` mirror the cursor
/// arc landed. Nothing is picked up: the spinner opens against the still-seated stack.
#[test]
fn shift_click_on_a_stack_opens_the_split_frame() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let (x, y) = open_backpack_with_a_five_stack(&mut s);

    assert!(!s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap());

    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(
        s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "shift-click opened the split frame"
    );
    assert!(
        s.cursor_item().is_none(),
        "the shift fork never picks the stack up"
    );
    assert_eq!(s.eval::<i64>("return StackSplitFrame.maxStack").unwrap(), 5);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// B180 — the split dialog's parchment plate fills the whole 172×96 frame. The reference authors
/// the plate 256×32 with NO anchors, a vestigial size the real client never renders (the TexCoords
/// crop exactly 172×96 out of the 256×128 art — the frame's own size, a complete panel drawn 1:1
/// over it). 1308 first dodged this by dropping the size; 1310 then landed the byte-verified law
/// (an anchor-less region gets an implicit SetAllPoints at creation, size unread under the two
/// corners) and restored the ref's own text — this pins the render through the real mechanism.
#[test]
fn the_split_frame_plate_fills_the_dialog() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let (x, y) = open_backpack_with_a_five_stack(&mut s);

    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    s.resolve();

    let (left, right, top, bottom) = s
        .eval::<(f64, f64, f64, f64)>(
            "return StackSplitFrame:GetLeft(), StackSplitFrame:GetRight(), \
                    StackSplitFrame:GetTop(), StackSplitFrame:GetBottom()",
        )
        .unwrap();
    assert!(
        (right - left - 172.0).abs() < 0.5 && (top - bottom - 96.0).abs() < 0.5,
        "the dialog frame is 172×96, got {}×{}",
        right - left,
        top - bottom
    );
    let plate = s
        .extract()
        .into_iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("UI-MoneyFrame"))
        })
        .expect("the plate is on screen");
    let r = plate.rect.expect("the plate has a rect");
    for (edge, got, want) in [
        ("left", r.left, left as f32),
        ("right", r.right, right as f32),
        ("top", r.top, top as f32),
        ("bottom", r.bottom, bottom as f32),
    ] {
        assert!(
            (got - want).abs() < 0.5,
            "plate {edge} = {got}, frame {edge} = {want} — the plate must fill the dialog"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// B180's Bagnon follow-on (director, 08-14): the dialog opened UNDER an addon bag window —
/// Bagnon's windows are `frameStrata="HIGH"`, the dialog's own stratum, and our re-expression had
/// dropped the reference's `toplevel="true"`, so the dialog's Show raised nothing and lost the
/// level tie to the later-shown window (its child buttons at level+1 poked through; the plate did
/// not). With the ref's attrs restored, Show runs the verified raise (toplevel.rs: compact, then
/// top-occupied-plus-one) and the whole dialog lands above. The synthetic window stands in for
/// Bagnon: same stratum, shown after load, overlapping the dialog.
#[test]
fn the_split_frame_raises_over_a_same_stratum_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let (x, y) = open_backpack_with_a_five_stack(&mut s);

    s.run(
        r#"
        local w = CreateFrame("Frame", "FakeBagnon", UIParent)
        w:SetFrameStrata("HIGH")
        w:SetPoint("BOTTOMLEFT", 0, 0)
        w:SetWidth(1024); w:SetHeight(768)
        local bg = w:CreateTexture(nil, "BACKGROUND")
        bg:SetTexture("Interface\\FakeBagnonBG")
        bg:SetAllPoints()
        w:Show()
    "#,
    )
    .unwrap();
    s.resolve();

    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    s.resolve();

    assert!(
        s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "the spinner opened"
    );
    let (dialog, bagnon) = s
        .eval::<(i64, i64)>("return StackSplitFrame:GetFrameLevel(), FakeBagnon:GetFrameLevel()")
        .unwrap();
    assert!(
        dialog > bagnon,
        "Show must raise the toplevel dialog over the same-stratum window \
         (dialog level {dialog}, window level {bagnon})"
    );
    // The symptom itself: the plate paints AFTER the window's background in draw order.
    let order: Vec<String> = s
        .extract()
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. }
                if p.contains("FakeBagnonBG") || p.contains("UI-MoneyFrame") =>
            {
                Some(p.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(order.len(), 2, "both textures on screen: {order:?}");
    assert!(
        order[0].contains("FakeBagnonBG"),
        "the plate must draw over the window, not under it: {order:?}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Typed-digit entry in the split spinner — the director's ask, and the deferral
/// this file's header carried since 0216. Pins the whole chain in one go: the dialog is in the
/// keyboard walk (`enableKeyboard`), a digit reaches its `OnChar`, the first digit REPLACES the
/// seeded 1 while later digits append, an over-max entry clamps instead of being rejected,
/// BACKSPACE drops a digit, and ENTER commits exactly as Okay does.
#[test]
fn typing_a_number_into_the_split_spinner_sets_the_count() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let (x, y) = open_backpack_with_a_five_stack(&mut s);

    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    s.resolve();
    assert_eq!(
        s.eval::<i64>("return StackSplitFrame.split").unwrap(),
        1,
        "opens seeded at 1"
    );

    // A digit is consumed by the dialog — which is also what stops it firing action button 3.
    assert!(s.char_input("3"), "the spinner consumed the digit");
    assert_eq!(
        s.eval::<i64>("return StackSplitFrame.split").unwrap(),
        3,
        "the first digit REPLACES the seed rather than appending to it"
    );
    assert_eq!(
        s.eval::<String>("return StackSplitText:GetText()").unwrap(),
        "3",
        "and the label follows"
    );

    // A second digit that would overflow the stack is **IGNORED**, not clamped — the reference's
    // `StackSplitFrame_OnChar` writes `this.split` only inside `if split <= this.maxStack`
    // (StackSplitFrame.lua l.72-85), so 37 against a 5-stack leaves 3 standing. Our own copy
    // clamped to 5 instead, and this assertion was written against that; the window is the
    // reference's since 1751 window 17, so the ignore is the behaviour.
    assert!(s.char_input("7"));
    assert_eq!(
        s.eval::<i64>("return StackSplitFrame.split").unwrap(),
        3,
        "37 against a 5-stack keeps the 3 — the ref ignores the digit, it does not clamp"
    );
    // BACKSPACE drops a digit off it.
    assert!(s.frame_key_input("BACKSPACE"), "the dialog took BACKSPACE");
    assert_eq!(s.eval::<i64>("return StackSplitFrame.split").unwrap(), 1);

    // Type a real value and commit with ENTER — the Okay path, so the carry is picked up.
    assert!(s.char_input("4"));
    assert_eq!(s.eval::<i64>("return StackSplitFrame.split").unwrap(), 4);
    assert!(s.key_input("ENTER"), "ENTER is consumed by the dialog");
    assert!(
        !s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "ENTER commits and closes, like Okay"
    );
    let held = s.cursor_item().expect("ENTER committed the split");
    assert_eq!(held.count, Some(4), "the typed count is what got picked up");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Okay in the split spinner only picks the split carry up (ref/cursor.rs `SplitContainerItem` —
/// a pickup, not a self-contained move); a SUBSEQUENT placement is what actually queues the
/// `ContainerMove` with `count: Some(n)`, drained the same way any other container move is.
#[test]
fn split_okay_then_a_placement_queues_the_split_move() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let (x, y) = open_backpack_with_a_five_stack(&mut s);
    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap());

    // Bump the spinner from 1 to 3, then Okay — the carry lands on the cursor.
    click(&mut s, "StackSplitRightButton", "LeftButton");
    click(&mut s, "StackSplitRightButton", "LeftButton");
    assert_eq!(s.eval::<i64>("return StackSplitFrame.split").unwrap(), 3);
    click(&mut s, "StackSplitOkayButton", "LeftButton");
    assert!(
        !s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "Okay hides the spinner"
    );
    let held = s.cursor_item().expect("Okay picked up the split carry");
    assert_eq!((held.bag, held.slot, held.count), (0, 1, Some(3)));
    assert!(
        s.take_container_moves().is_empty(),
        "no move yet — only a pickup"
    );

    // Place the carry on slot 5 (empty) — NOW the move queues, carrying the split count.
    let b5 = bag_slot_button(&s, 0, 5);
    click(&mut s, &b5, "LeftButton");
    assert!(s.cursor_item().is_none());
    assert_eq!(
        s.take_container_moves(),
        vec![ContainerMove {
            src_bag: 0,
            src_slot: 1,
            dst_bag: 0,
            dst_slot: 5,
            count: Some(3),
        }]
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A plain click hides any open split frame (ref ContainerFrame.lua:581) — even a click on an
/// unrelated, empty slot.
#[test]
fn a_plain_click_hides_an_open_split_frame() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let (x, y) = open_backpack_with_a_five_stack(&mut s);
    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap());

    let b9 = bag_slot_button(&s, 0, 9);
    click(&mut s, &b9, "LeftButton");
    assert!(
        !s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "the plain click on an unrelated slot hid the spinner"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Bag-slot item cooldowns through the shipped XML (decision 0263's deferral): a potion mid-
/// cooldown pushes its triple with the container snapshot; opening the bag runs the ref's
/// occupied-slot fork (`ContainerFrame_UpdateCooldown` → `GetContainerItemCooldown` →
/// `CooldownFrame_SetTimer`) and the slot grows a live sweep; a `BAG_UPDATE_COOLDOWN` refresh
/// with the cooldown gone hides it again.
#[test]
fn bag_slot_cooldown_sweeps_through_the_xml() {
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
    s.tick(100.0); // a nonzero clock epoch, like the engine cooldown tests

    let potion = |cooldown| ContainerSlot {
        durability: None,
        texture: Some("Interface\\Icons\\INV_Potion_49".into()),
        count: 3,
        quality: Some(1),
        item_id: 118,
        cooldown,
        ..Default::default()
    };
    let backpack = |cooldown| {
        let mut slots = std::collections::HashMap::new();
        slots.insert(1, potion(cooldown));
        ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }
    };
    // 12 s remain of the potion category's 60 s: started at GetTime 52 (absolute-start triple).
    s.set_container(0, Some(backpack(Some((52_000, 60_000, true)))));
    s.run("OpenAllBags()").unwrap();
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(0)]);
    // The stock machine: the slot's cooldown pane, sequence 0 scrubbed by the
    // next paint's `OnUpdateModel` to the elapsed fraction.
    super::test_ui::cooldown_facts(&mut s);
    s.tick(0.0);
    s.resolve();

    let sweep = |s: &UiScript| super::test_ui::cooldown_play_any(s);
    let play = sweep(&s).expect("the bag slot sweeps");
    assert_eq!(play, (0, 800), "48 of 60 s elapsed: sequence 0 at 800 ms");

    // The cooldown clears (a CLEAR_COOLDOWN, or it simply ran out before the re-push): the
    // refresh event re-reads the now-cold triple and hides the widget.
    s.set_container(0, Some(backpack(None)));
    s.fire_event("BAG_UPDATE_COOLDOWN", vec![]);
    assert_eq!(sweep(&s), None, "cold again after the refresh");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The bar's five bag buttons had NO hover at all — the director's "the bags are missing their
/// simple tooltips". These are the ref's plain `SetText` plates (MainMenuBarBagButtons.xml l.91-99
/// for the backpack, MainMenuBarBagButtons.lua l.86-96 for the four slots), NOT the two-line
/// newbie kind the micro buttons next to them use — so what's pinned here is the label, the
/// empty-slot fallback, and that they seat BESIDE the button rather than at the screen corner.
///
/// Those two bodies are the reference's OWN now (1751 window 3), which changes one thing this test
/// records: **the binding-key suffix appears on the four slots too.** The reference appends
/// `GetBindingKey("TOGGLEBAG"..(4 - (this:GetID() - CharacterBag0Slot:GetID())))` — its numbering
/// runs the bar right-to-left, so the slot next to the backpack is `TOGGLEBAG4`, our F11
/// (`bindings/commands.rs`, quoting ref Bindings.xml l.564-575). Our own handler never did this,
/// so the suffix is new here and is the reference's behaviour, not a regression. The backpack's
/// key is the reference's `TOGGLEBACKPACK` rather than the `OPENALLBAGS` ours read; both default
/// to B, so the string is unchanged and only the reason is.
#[test]
fn the_bar_bag_buttons_name_themselves_on_hover() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    // The suffix reads GetBindingKey live since 0997 — register the real command set the way
    // the app's seed does, so the pin below is OPENALLBAGS's actual default.
    s.register_bindings(&crate::bindings::registry_commands());
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.resolve();

    // The backpack: its label plus OPENALLBAGS's live key (default B; 0997's divergence note).
    hover(&mut s, "MainMenuBarBackpackButton");
    let line = s
        .eval::<String>("return GameTooltipTextLeft1:GetText()")
        .unwrap();
    assert!(
        line.starts_with("Backpack") && line.contains("(B)"),
        "the backpack names itself and its TOGGLEBACKPACK key: {line:?}"
    );
    // Beside the button, not the default corner — the ref's ANCHOR_LEFT.
    assert!(
        s.eval::<bool>("return GameTooltip.default == nil").unwrap(),
        "a bag button's plate is owner-anchored, never the default corner"
    );
    assert!(
        s.eval::<bool>("return GameTooltip:IsOwned(MainMenuBarBackpackButton)")
            .unwrap(),
        "…owned by the button it opened from"
    );

    // An empty bag slot falls back to the ref's EQUIP_CONTAINER rather than showing nothing.
    hover(&mut s, "CharacterBag0Slot");
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Equip Container",
        "an empty slot says what belongs in it"
    );

    // With a bag actually equipped there, the ref shows that BAG's own item tooltip instead — the
    // SetInventoryItem arm. Bar slot 1 is inventory slot 20 (Bag0Slot).
    let mut inv: benilla_ui::script::InventorySlots = Default::default();
    inv[20] = Some(benilla_ui::script::InvSlotView {
        duration_ms: None,
        already_bound: false,
        bar_placeable: true,
        durability: None,
        flags: 0,
        item_id: 4496,
        icon: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
        count: 1,
        contents_count: Some(0), // an equipped bag, whose subclass row carries no count gate
        quality: 1,
        name: Some("Small Brown Pouch".into()),
        link: Some("|cffffffff|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
        locked: false,
        equip_slots: vec![20],
        creator: None,
        enchants: Vec::new(),
    });
    s.set_inventory_slots(inv);
    hover(&mut s, "CharacterBag0Slot");
    let line = s
        .eval::<String>("return GameTooltipTextLeft1:GetText()")
        .unwrap();
    assert!(
        line.starts_with("Small Brown Pouch"),
        "an equipped slot shows that bag, not the empty-slot fallback: {line:?}"
    );
    assert!(
        line.contains("(F11)"),
        "…and the reference appends this slot's own TOGGLEBAG4 key: {line:?}"
    );

    unhover(&mut s);
    assert!(
        !s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "leaving hides the plate"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// One keyring container fed as the app feeds it: container −2 sized by the level ladder, with a
/// key in the first slot. `size` is the count `keyring_size(level)` would give.
fn keyring(size: u32, occupied: bool) -> ContainerState {
    let mut slots = std::collections::HashMap::new();
    if occupied {
        slots.insert(
            1,
            ContainerSlot {
                // The Scarlet Key — the item the director's own character carries in keyring slot 1.
                item_id: 7146,
                count: 1,
                quality: Some(1),
                texture: Some("Interface\\Icons\\INV_Misc_Key_07".into()),
                link: Some("|cffffffff|Hitem:7146:0:0:0|h[The Scarlet Key]|h|r".into()),
                ..Default::default()
            },
        );
    }
    ContainerState {
        name: Some("Keyring".into()),
        num_slots: size,
        slots,
    }
}

/// Seat a player at `level`. **The keyring's size is the REFERENCE's own level ladder now** —
/// `GetKeyRingSize` (ContainerFrame.lua l.773-786: 4, then 8 at 40, 12 at 50, 16 above 60) reads
/// `UnitLevel("player")`, where ours read the container's fed `num_slots` back.
/// Both derive from the same ladder the server enforces (`Player::GetMaxKeyringSize`), so this is
/// the swap becoming FAITHFUL rather than a behaviour change — but the level is now the input, and
/// a fixture that leaves it 0 gets a 4-slot ring.
fn seat_player_at_level(s: &mut UiScript, level: u32) {
    let mut player = benilla_ui::script::UnitState {
        level,
        ..Default::default()
    };
    player.name = Some("Tester".into());
    s.set_unit("player", Some(player));
}

/// How many drawn quads carry a texture path containing `needle`. The engine exposes no
/// `GetTexture`, so "which art is this frame wearing" is asked of the resolved draw list — which is
/// the stronger question anyway (it answers what would actually be on screen).
fn drawn_with(s: &mut UiScript, needle: &str) -> usize {
    s.resolve();
    s.extract()
        .iter()
        .filter(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle)))
        .count()
}

/// Load the whole bar+bag surface the keyring spans: its button seats on the action bar (BAG_UI
/// carries ActionBar.xml) and its window is an ordinary `ContainerFrame` at `KEYRING_CONTAINER`
/// since decision 1751, so the reference's own file is load-bearing too. `ErrorsFrame.xml` is the
/// extra: `PutKeyInKeyRing` reports a full ring through `UIErrorsFrame`.
fn keyring_surface(s: &UiScript) {
    for file in BAG_UI {
        load_xml(s, file);
    }
    load_xml(s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(s, "Interface\\FrameXML\\UIErrorsFrame.xml");
}

/// **The gate**: no key ⇒ no keyring anywhere on the bar — the button is hidden and
/// the bar's two right-hand strips wear the ordinary dwarf plate. The first key flips all of it:
/// the button appears, both strips swap to the keyring plate with the reference's own TexCoords,
/// and the performance meter slides from −227 to −235 to clear the new socket
/// (ref MainMenuBar_UpdateKeyRing, MainMenuBar.lua l.174-183).
///
/// This is the director-reported symptom itself: a character holding a key saw no keyring at all.
#[test]
fn the_first_key_puts_the_keyring_on_the_bar() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    keyring_surface(&s);
    s.set_money(0);

    // Keyless: the button is hidden and the plate is the dwarf one.
    s.set_has_key(false);
    s.run("MainMenuBar_UpdateKeyRing()").unwrap();
    assert!(
        !s.eval::<bool>("return KeyRingButton:IsShown()").unwrap(),
        "no key ⇒ no keyring button"
    );
    assert_eq!(
        drawn_with(&mut s, "UI-MainMenuBar-KeyRing"),
        0,
        "no key ⇒ every bar strip wears the ordinary dwarf plate"
    );

    // A key lands. The app pushes HasKey and the wire's BAG_UPDATE reaches the button's OnEvent —
    // the exact runtime path, not a direct call.
    s.set_has_key(true);
    s.set_container(-2, Some(keyring(8, true)));
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(-2)]);
    assert!(
        s.eval::<bool>("return KeyRingButton:IsShown()").unwrap(),
        "the first key reveals the keyring button"
    );
    assert_eq!(
        drawn_with(&mut s, "UI-MainMenuBar-KeyRing"),
        2,
        "exactly the two RIGHT-hand strips swap to the keyring plate"
    );
    for (strip, top, bottom) in [
        ("MainMenuBarTexture2", 0.6640625, 1.0),
        ("MainMenuBarTexture3", 0.1640625, 0.5),
    ] {
        let (t, b) = s
            .eval::<(f64, f64)>(&format!(
                "local _, top, _, bottom = {strip}:GetTexCoord() return top, bottom"
            ))
            .unwrap();
        assert!(
            (t - top).abs() < 1e-9 && (b - bottom).abs() < 1e-9,
            "{strip} keeps the reference's own band — got {t}..{b}, want {top}..{bottom}"
        );
    }
    // Geometry cross-check against the reference's own chain. The ref seats its backpack button at
    // BOTTOMRIGHT (-6, 2) and steps 37px buttons left with -5 gaps, putting KeyRingButton's left
    // edge at -234 from the bar's right; ours steps 36px buttons with -6 gaps and lands at -235.
    // Within a pixel of the real bar — which is the check that matters, because the socket the
    // button sits in is PAINTED INTO the keyring plate and cannot be nudged to meet it.
    let bar_right = s.eval::<f64>("return MainMenuBar:GetRight()").unwrap();
    let button_left = s.eval::<f64>("return KeyRingButton:GetLeft()").unwrap();
    assert!(
        (bar_right - button_left - 235.0).abs() < 1.5,
        "the keyring button must land in the plate's painted socket — the ref's own -234, got {}",
        bar_right - button_left
    );

    // The performance meter clears the new socket (ref l.180: -227 → -235).
    let perf_right = s
        .eval::<f64>("return MainMenuBarPerformanceBarFrame:GetRight()")
        .unwrap();
    let bar_right = s.eval::<f64>("return MainMenuBar:GetRight()").unwrap();
    assert!(
        (bar_right - perf_right - 235.0).abs() < 0.01,
        "the meter slid to -235 with the keyring up (got {})",
        bar_right - perf_right
    );

    // And it does NOT revert: the reference's latch is one-way — `MainMenuBar_UpdateKeyRing`
    // only ever Shows, and SHOW_KEYRING is a saved variable (stock MainMenuBar.lua:174-183,
    // MainMenuBar.xml:323-336). Our file used to take the button away with the last key; that
    // divergence went with the file (1938).
    s.set_has_key(false);
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(-2)]);
    assert!(
        s.eval::<bool>("return KeyRingButton:IsShown()").unwrap(),
        "losing the last key leaves the button on the bar — the reference's one-way latch"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The window itself: clicking the button opens a container titled "Keyring", stitched from the
/// `-Keyring` plate, showing exactly the level-gated slot count — and it jingles rather than
/// rustling (KeyRingOpen/KeyRingClose, ref ContainerFrame.lua l.116-138).
#[test]
fn the_keyring_button_opens_a_keyring_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    keyring_surface(&s);
    s.set_money(0);
    s.set_has_key(true);
    // A level-44 character: 8 usable slots (the ladder's 40..49 rung). The LEVEL is what decides
    // now — see `seat_player_at_level` — and the feed carries the same 8 so the two agree.
    seat_player_at_level(&mut s, 44);
    s.set_container(-2, Some(keyring(8, true)));
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(-2)]);
    let _ = s.take_sounds();

    assert!(!open(&s, -2));
    s.run("KeyRingButton:Click()").unwrap();
    assert!(open(&s, -2), "the button opens the keyring");
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("KeyRingOpen".into())],
        "the keyring has its own open kit, not the backpack's"
    );
    let w = window(&s, -2);
    assert_eq!(
        s.eval::<String>(&format!("return {w}Name:GetText()"))
            .unwrap(),
        "Keyring",
        "titled from the KEYRING string, not a bag item's name"
    );
    assert!(
        drawn_with(&mut s, "UI-Bag-Components-Keyring") > 0,
        "stitched from the keyring plate, not the ordinary bag sheet"
    );
    // 8 usable slots: the window is GENERATED at the keyring's own size, so exactly 8 of the
    // template's 36 item buttons are shown — the reference's own
    // `for j = size + 1, MAX_CONTAINER_ITEMS do …:Hide() end` tail, no keyring-specific code.
    let shown = s
        .eval::<i64>(&format!(
            "local n = 0 for i = 1, MAX_CONTAINER_ITEMS do \
             if getglobal('{w}Item' .. i):IsShown() then n = n + 1 end end return n"
        ))
        .unwrap();
    assert_eq!(shown, 8, "only the level-unlocked keyring slots are drawn");
    // The key is in the window (slot 1 of 8 is the LAST chain button — size - physIndex + 1).
    assert_eq!(
        s.eval::<i64>("return BenillaGetContainerItemID(KEYRING_CONTAINER, 1)")
            .unwrap(),
        7146
    );

    s.run("KeyRingButton:Click()").unwrap();
    assert!(!open(&s, -2));
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("KeyRingClose".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Dropping a held key on the button files it in the first free slot (ref PutKeyInKeyRing), and a
/// full keyring refuses out loud — where the reference prints nothing at all, having passed a
/// GlobalStrings name that was never defined.
#[test]
fn a_key_dropped_on_the_button_files_itself() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    keyring_surface(&s);
    s.set_money(0);
    s.set_has_key(true);

    // A 4-slot keyring with slot 1 taken, and the key on the cursor (picked out of the backpack).
    s.set_container(-2, Some(keyring(4, true)));
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::from([(
                3,
                ContainerSlot {
                    item_id: 7146,
                    count: 1,
                    ..Default::default()
                },
            )]),
        }),
    );
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(-2)]);
    s.run("PickupContainerItem(0, 3)").unwrap();
    assert!(s.eval::<bool>("return CursorHasItem()").unwrap());

    s.run("KeyRingButton:Click()").unwrap();
    assert_eq!(
        s.take_container_moves(),
        vec![ContainerMove {
            src_bag: 0,
            src_slot: 3,
            dst_bag: -2,
            dst_slot: 2,
            count: None,
        }],
        "filed into the FIRST FREE keyring slot (1 is taken), not the backpack"
    );

    // Now full: the click refuses with the error line instead of queueing anything.
    // Every slot needs a TEXTURE, not just an item id: the reference's `PutKeyInKeyRing` decides
    // "is this slot free" from `GetContainerItemInfo`'s first return, which is the icon path
    // (ContainerFrame.lua l.744-746). A slot carrying an id and no texture reads EMPTY.
    let mut full = keyring(4, true);
    for slot in 2..=4 {
        full.slots.insert(
            slot,
            ContainerSlot {
                item_id: 7146,
                count: 1,
                texture: Some("Interface\\Icons\\INV_Misc_Key_07".into()),
                ..Default::default()
            },
        );
    }
    s.set_container(-2, Some(full));
    s.run("PickupContainerItem(0, 3)").unwrap();
    // The refusal is the REFERENCE's own `PutKeyInKeyRing`, reporting through the name its own
    // line asks for — `NO_EMPTY_KEYRING_SLOTS`, which 1.12 never defines. So the drop is refused
    // and **nothing is printed**: a real 1.12 bug, and faithful here on purpose.
    //
    // This client un-quirked it once, by assigning the string the reference plainly meant. Window
    // 1 of 1751 retired that (ContainerFrameAdapters.xml's header records it), but the assignment
    // itself outlived the decision in `BagFrame.xml` for a window and a half. Window 3 deleted that
    // file, so the stated posture and the behaviour agree again. Restoring the repair is one line
    // in the adapters if it is ever wanted; it is not silent either way.
    s.run("KeyRingButton:Click()").unwrap();
    assert!(
        s.take_container_moves().is_empty(),
        "a full keyring queues no move"
    );
    s.resolve();
    assert!(
        !s.extract().iter().any(|q| matches!(
            &q.content,
            QuadContent::Text { text: Some(t), .. } if t.contains("keyring is full")
        )),
        "the reference's own dangling string name prints nothing — un-quirking it is a decision, \
         not a default"
    );
    // And it does not RAISE either, which is the half this test could not assert until the bytes
    // came back. `AddMessage`'s text is fetched through `lua_isstring 0x6f3510` whose failure edge
    // is a silent jump to the function's own epilogue (`0x79562c`) — no line, no error. Ours took
    // an mlua `String` and raised, which put a script-error dialog on an ordinary player gesture;
    // the fix is in `messageframe/mod.rs::message_text` and its own test carries the full law.
    //
    // So a full keyring is a DEAD CLICK on a real 1.12 client, and now here.
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The item-push drop animation is the reference's own `<Model>` now:
/// `ITEM_PUSH(container, icon)` reaches the stock `ItemAnim_OnEvent`, which puts the icon on the
/// pane whose parent button owns that inventory slot (`ReplaceIconTexture`), arms sequence 0 at
/// 0 and shows it; the pane's clock runs the file's 1000 ms clamp; `ItemAnim_OnAnimFinished`
/// hides it. The pane authored no size, so its rect is the file's own bounding box in layout
/// units — the 42.41 × 124.72 the 0887 card measured by hand at 16:9.
#[test]
fn an_item_push_drops_its_icon_into_the_bag_that_took_it() {
    use benilla_ui::widget::{ModelFileFacts, SequenceFacts};

    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    // `ForcedBackpackItem.m2` (`m2seq`/`m2batch`): one sequence, id 0, 1000 ms, clamp; the
    // header box spans the card's whole travel band — 0.02707 × 0.07962 model units, which at
    // 16:9 (a layout unit is 768·√(a²+1) = 1566.4 FrameXML units) is the 42.41 × 124.72 rect
    // 0887's card measured.
    const CARD: &str = r"Interface\ItemAnimations\ForcedBackpackItem.mdx";
    s.set_model_facts(
        CARD,
        ModelFileFacts {
            sequences: vec![SequenceFacts {
                anim_id: 0,
                duration_ms: 1000,
                looping: false,
            }],
            bbox: ([0.0, 0.0, 0.0], [0.02707, 0.07962, 0.0]),
            cameras: 0,
        },
    );
    s.resolve();

    let shown = |s: &UiScript, name: &str| {
        s.eval::<bool>(&format!("return {name}ItemAnim:IsShown()"))
            .unwrap()
    };
    // The shown card panes, as the renderer sees them: `(icon, play head)` per visible pane.
    type Card = (Option<String>, Option<(u16, u32)>);
    let cards = |s: &mut UiScript| -> Vec<Card> {
        let heads = s.visible_model_panes();
        s.extract()
            .into_iter()
            .filter_map(|q| match q.content {
                QuadContent::ModelPane {
                    handle,
                    model: Some(m),
                    icon,
                    ..
                } if m == CARD => Some((
                    icon,
                    heads
                        .iter()
                        .find(|p| p.handle == handle)
                        .and_then(|p| p.play.map(|ph| (ph.anim_id, ph.cursor_ms))),
                )),
                _ => None,
            })
            .collect()
    };

    // Nothing is animating until a push arrives.
    for b in [
        "MainMenuBarBackpackButton",
        "CharacterBag1Slot",
        "KeyRingButton",
    ] {
        assert!(!shown(&s, b), "{b}'s card starts hidden");
    }
    assert!(cards(&mut s).is_empty());

    // A push into equipped bag 2. `arg1` is the reference's own vocabulary since 1751 window 3 —
    // the button's INVENTORY-slot id, which is what `ItemAnim_OnEvent` compares against — so bag 2
    // is `CharacterBag1Slot`'s 21.
    s.fire_event(
        "ITEM_PUSH",
        vec![
            benilla_ui::script::ScriptValue::Int(21),
            benilla_ui::script::ScriptValue::Str("Interface\\Icons\\INV_Misc_Bag_08".into()),
        ],
    );
    assert!(
        shown(&s, "CharacterBag1Slot"),
        "the card that took it plays"
    );
    assert!(
        !shown(&s, "MainMenuBarBackpackButton") && !shown(&s, "KeyRingButton"),
        "…and nobody else's"
    );
    s.resolve();
    let live = cards(&mut s);
    assert_eq!(
        live,
        vec![(
            Some(r"Interface\Icons\INV_Misc_Bag_08".to_string()),
            Some((0, 0))
        )],
        "one pane, the pushed icon on it, sequence 0 at 0"
    );
    // The implicit rect: the file's box in layout units, hung off the button's BOTTOMRIGHT
    // (−10, 0) as the stock template anchors it.
    let (w, h): (f32, f32) = s
        .eval("return CharacterBag1SlotItemAnim:GetWidth(), CharacterBag1SlotItemAnim:GetHeight()")
        .unwrap();
    assert!(
        (w - 42.41).abs() < 0.05 && (h - 124.72).abs() < 0.05,
        "the card's rect is the file's box: {w} × {h}"
    );
    let (dx, dy): (f32, f32) = s
        .eval(
            "local a, b = CharacterBag1SlotItemAnim, CharacterBag1Slot \
             return a:GetRight() - b:GetRight(), a:GetBottom() - b:GetBottom()",
        )
        .unwrap();
    assert!(
        (dx + 10.0).abs() < 0.01 && dy.abs() < 0.01,
        "BOTTOMRIGHT (−10, 0) off the button: ({dx}, {dy})"
    );

    // The clock runs the file's clamp: half a second in, the play head is at 500 ms…
    s.tick(0.5);
    assert_eq!(cards(&mut s)[0].1, Some((0, 500)));
    // …and past 1000 ms the completion fires `ItemAnim_OnAnimFinished` → `Hide()`.
    s.tick(0.55);
    assert!(
        !shown(&s, "CharacterBag1Slot"),
        "one play, then gone (the stock OnAnimFinished hides it)"
    );
    assert!(cards(&mut s).is_empty());

    // A second push RESTARTS the card (SetSequenceTime(0, 0) on a fresh arm).
    s.fire_event(
        "ITEM_PUSH",
        vec![
            benilla_ui::script::ScriptValue::Int(21),
            benilla_ui::script::ScriptValue::Str("Interface\\Icons\\INV_Misc_Bag_09".into()),
        ],
    );
    s.resolve();
    assert_eq!(
        cards(&mut s),
        vec![(
            Some(r"Interface\Icons\INV_Misc_Bag_09".to_string()),
            Some((0, 0))
        )]
    );

    // The keyring is a real destination, not a rounding of the backpack.
    s.fire_event(
        "ITEM_PUSH",
        vec![
            benilla_ui::script::ScriptValue::Int(-2),
            benilla_ui::script::ScriptValue::Str("Interface\\Icons\\INV_Misc_Key_03".into()),
        ],
    );
    assert!(shown(&s, "KeyRingButton"), "the keyring card plays");
    assert!(
        !shown(&s, "MainMenuBarBackpackButton"),
        "and the backpack's does not"
    );

    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **An addon that replaces `ToggleBackpack` gets the bag button's click.**
///
/// This is how a 1.12 addon customises anything — replace a global, expect the client to call your
/// replacement. `Bagnon_Core/core/Overrides.lua` is the canonical shape:
///
/// ```lua
/// local bToggleBackpack = ToggleBackpack
/// ToggleBackpack = function() … end
/// ```
///
/// It was inert here. The 'B' binding and the bag button called `BenillaBagToggle_OnClick`
/// directly, so nothing ever looked up `ToggleBackpack`; Bagnon loaded, showed up in the AddOns
/// list, replaced the globals, and the player still got the stock bags. **The director saw that and
/// no instrument here could** — the corpus survey loads addons and fires events, but never clicks.
///
/// The class is much wider than one addon: every hook-a-global addon is silently inert wherever our
/// UI calls a benilla-named equivalent. This test pins the bag path; the rule is general.
#[test]
fn an_addon_that_hooks_toggle_backpack_receives_the_click() {
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

    // Bagnon's exact idiom: capture the original, replace the global.
    s.run(
        r#"
        HOOK_RAN = 0
        local original = ToggleBackpack
        ToggleBackpack = function() HOOK_RAN = HOOK_RAN + 1 end
    "#,
    )
    .unwrap();

    // The button's own OnClick path — what the B binding and a plain backpack-button click run.
    s.run("MainMenuBarBackpackButton:Click()").unwrap();

    assert_eq!(
        s.eval::<i64>("return HOOK_RAN").unwrap(),
        1,
        "the click must reach the addon's replacement, not our own function"
    );
    // And the addon's override SUPPRESSED the stock behaviour, which is the point of hooking.
    assert!(
        !open(&s, 0),
        "the stock bag must not open when an addon has taken the verb over"
    );

    // The reference's other two names exist and are callable by an addon that wants the original.
    assert!(
        s.eval::<bool>("return type(OpenAllBags) == 'function' and type(ToggleBag) == 'function'")
            .unwrap(),
        "OpenAllBags and ToggleBag are the ref's names and addons hook them too"
    );
}

/// **The equipped-bag slots carry the REFERENCE's names, and so do their icons.**
///
/// Ours were `BenillaBagBarSlot1..4`; the reference's are `CharacterBag0Slot..3` — 0-based, so
/// `CharacterBag0Slot` IS bag 1. Eight corpus addons index those names and found nil, including
/// **Bagnon**, which does `getglobal("CharacterBag0Slot"):GetScript("OnClick")`.
///
/// The same class as the `ToggleBackpack` bug the director found: our own name where the reference
/// has one, so every addon reaching for it is silently inert.
///
/// A Lua alias would not have been enough. `Bartender2/Alias.lua:352` takes
/// `CharacterBag0SlotIconTexture` — a name DERIVED from the frame's own, which only a real rename
/// produces. The icon is `$parentIconTexture` now, matching the reference's
/// `PaperDollItemSlotButtonTemplate`.
#[test]
fn the_bag_slots_carry_the_references_names_and_icon_names() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");

    for i in 0..4 {
        assert!(
            s.eval::<bool>(&format!("return CharacterBag{i}Slot ~= nil"))
                .unwrap(),
            "CharacterBag{i}Slot must exist — 8 corpus addons index these"
        );
        assert!(
            s.eval::<bool>(&format!(
                "return getglobal('CharacterBag{i}SlotIconTexture') ~= nil"
            ))
            .unwrap(),
            "and its icon under the derived name Bartender2 aliases"
        );
    }

    // 0-based: CharacterBag0Slot is bag 1, and the off-by-one is the whole trap. Asked the way the
    // reference asks it, which is the only way there is now: the bar is stock
    // `MainMenuBarBagButtons.xml` since 1751's third window, its buttons carry no `bagId` field,
    // and every one of its handlers recomputes `this:GetID() - CharacterBag0Slot:GetID() + 1` at
    // the call site. The ids themselves come from `GetInventorySlotInfo("Bag0Slot")` = 20 through
    // the sourced `PaperDollItemSlotButton_OnLoad`, which is what makes the arithmetic work.
    assert_eq!(
        s.eval::<i64>("return CharacterBag0Slot:GetID()").unwrap(),
        20,
        "CharacterBag0Slot's id is its INVENTORY slot — 8 corpus addons compute `GetID() - 19`"
    );
    assert_eq!(
        s.eval::<i64>("return CharacterBag3Slot:GetID() - CharacterBag0Slot:GetID() + 1")
            .unwrap(),
        4,
        "the reference's own translation: the fourth button is bag 4"
    );

    // Bagnon's exact reach.
    assert!(
        s.eval::<bool>("return getglobal('CharacterBag0Slot'):GetScript('OnClick') ~= nil")
            .unwrap(),
        "Bagnon captures this OnClick to replace the bag behaviour"
    );
}

/// The backpack button's checked law after the stated divergence (BenillaBagToggle_OnClick's
/// comment): checked belongs to the WINDOWS' OnShow/OnHide writes, and the button's XML click
/// wrapper only undoes the CheckButton widget's pre-handler flip.
///
/// Driven through REAL input (the widget flip fires only on real clicks) over the case 1494
/// changed: backpack shut with another bag's window open. That used to be the close-all arm —
/// nothing transitioned the backpack and only the undo kept the button from ending lit-while-shut.
/// It is now the OPEN arm, so the pin runs the whole cycle: the click opens bag 0 beside bag 1 and
/// its OnShow lights the button, the next click closes everything and the OnHide clears it.
#[test]
fn the_backpack_buttons_ring_follows_its_own_window_through_real_clicks() {
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
    // The reference's `ToggleBag` opens nothing for a container of size 0, so the backpack has to
    // be fed — our own windows existed statically and opened regardless.
    s.set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::new(),
        }),
    );
    s.set_container(
        1,
        Some(benilla_ui::script::ContainerState {
            name: Some("Pouch".into()),
            num_slots: 6,
            slots: std::collections::HashMap::new(),
        }),
    );
    let checked = |s: &mut UiScript| {
        s.eval::<bool>("return MainMenuBarBackpackButton:GetChecked() and true or false")
            .unwrap()
    };
    // Bag 1's window alone is open; the backpack window and its button are dark.
    s.run("ToggleBag(1)").unwrap();
    assert!(!open(&s, 0));
    assert!(!checked(&mut s));

    // A REAL click on the backpack button (the widget flip fires only on real input): close-all.
    s.resolve();
    let r: Vec<f32> = s
        .eval(
            "local f = MainMenuBarBackpackButton \
             return { f:GetLeft() + f:GetWidth() / 2, f:GetBottom() + f:GetHeight() / 2 }",
        )
        .unwrap();
    s.mouse_move(r[0], r[1]);
    s.mouse_button(r[0], r[1], "LeftButton", true);
    s.mouse_button(r[0], r[1], "LeftButton", false);
    assert!(
        open(&s, 0),
        "the click opens the backpack (ToggleBackpack's open arm reads bag 0, not 'anything open')"
    );
    assert!(open(&s, 1), "…and leaves the bag that was already up alone");
    assert!(
        checked(&mut s),
        "open ⇒ lit, written by the window's OnShow over the widget flip"
    );

    // The second real click: bag 0 is open now, so this is the close-all arm — both windows go,
    // and the OnHide write clears the ring.
    s.mouse_button(r[0], r[1], "LeftButton", true);
    s.mouse_button(r[0], r[1], "LeftButton", false);
    assert!(!open(&s, 0));
    assert!(!open(&s, 1), "the close arm takes every container down");
    assert!(
        !checked(&mut s),
        "shut ⇒ dark, written by the window's OnHide"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
