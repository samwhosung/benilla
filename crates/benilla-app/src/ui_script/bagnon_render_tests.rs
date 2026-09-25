//! Bagnon against the addon corpus, driven as a player drives it: every live bag slot must be
//! drawn (an empty window raises nothing), and every gesture must run without raising.

use std::path::Path;

use benilla_formats::addon_corpus_or_skip as corpus_or_skip;
use benilla_ui::script::{AddOnInfo, ContainerSlot, ContainerState, QuadContent, UiScript};
use benilla_ui::toc::Toc;

/// The corpus and a client install, for tests of a stock global sourced off the player's chain
/// ([`super::reference_ui`]), which is nil without an install.
macro_rules! corpus_and_install_or_skip {
    () => {{
        let _data = benilla_formats::wow_data_or_skip!();
        corpus_or_skip!()
    }};
}

fn read_toc(root: &Path, name: &str) -> Toc {
    let path = root.join(name).join(format!("{name}.toc"));
    Toc::parse(&benilla_ui::source::decode(
        &std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    ))
}

/// One addon's `.toc` files as the loader runs them: `.lua` as a chunk, anything else as FrameXML.
fn load_addon_files(script: &UiScript, root: &Path, name: &str) -> Vec<String> {
    let toc = read_toc(root, name);
    let provider = |req: &str| -> Option<Vec<u8>> { std::fs::read(root.join(req)).ok() };
    let mut errors = Vec::new();
    for file in &toc.files {
        let path = benilla_ui::loader::join_ref(name, file);
        let Ok(bytes) = std::fs::read(root.join(&path)) else {
            errors.push(format!("{file}: not found"));
            continue;
        };
        if file.to_ascii_lowercase().ends_with(".lua") {
            // Named as the client names it: the corpus parses its own tracebacks.
            if let Err(e) =
                script.run_chunk_named(&bytes, &benilla_ui::script::addon_chunk_name(name, file))
            {
                errors.push(format!("{file}: {e}"));
            }
            continue;
        }
        match benilla_ui::framexml::parse(&benilla_ui::source::decode(&bytes)) {
            Ok(doc) => {
                let report = benilla_ui::loader::load_in(script, &doc, &path, &provider);
                errors.extend(report.errors.into_iter().map(|e| format!("{file}: {e}")));
            }
            Err(e) => errors.push(format!("{file}: {e}")),
        }
    }
    errors
}

/// The installed set. Bagnon and Bagnon_Options are `LoadOnDemand`: the startup walk skips them and
/// `LoadAddOn` loads them, which is why the registry carries a real root.
const INSTALLED: &[&str] = &[
    "!OmniCC",
    "Bagnon",
    "Bagnon_Core",
    "Bagnon_Forever",
    "Bagnon_Options",
    "MapCoords",
];

/// Whether the VM has its `"player"` ([`super::seat_from_roster`]) before the addons load.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Seat {
    /// The app's order: the roster row goes in ahead of `load_ingame_ui`.
    BeforeAddons,
    /// Nothing writes `"player"` until `ui_unit::feed_units` sees the self descriptor.
    AfterAddons,
}

/// A roster with a pending pick, for [`super::seat_from_roster`] itself to seat.
fn roster() -> crate::char_select::Roster {
    let row = benilla_protocol::Character {
        guid: 7,
        name: "Harness".into(),
        race: 1,  // Human → Alliance
        class: 1, // Warrior
        gender: 0,
        level: 60,
        skin: 0,
        face: 0,
        hair_style: 0,
        hair_color: 0,
        facial_hair: 0,
        zone: 0,
        map: 0,
        position: benilla_protocol::wire::Vector3d {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        flags: 0,
        equipment: [benilla_protocol::CharEnumItem::default(); 19],
        pet_display_id: 0,
        pet_level: 0,
        pet_family: 0,
    };
    crate::char_select::Roster::with_pending_pick(vec![row], 7)
}

/// The `"player"` snapshot the self descriptor builds, as `ui_unit::feed_units` pushes it: the
/// roster seat plus what only the descriptor says (the unit exists, its level, health, a live
/// object). The seat leaves those nil and 0, as the reference answers before the object exists.
fn descriptor_snapshot(seat: &benilla_ui::script::UnitState) -> benilla_ui::script::UnitState {
    benilla_ui::script::UnitState {
        exists: true,
        has_object: true,
        is_connected: true,
        guid: 7,
        level: 60,
        health: 4_000,
        max_health: 4_000,
        power_type: 1, // a Warrior's rage
        max_power: 1_000,
        ..seat.clone()
    }
}

/// A session's VM: the whole interface, `INSTALLED` walked in dependency order, a real backpack.
fn seat(root: &Path, seat: Seat) -> UiScript {
    let mut s = UiScript::new().expect("VM");
    s.set_screen_size(1024.0, 768.0);

    let mut registry: Vec<AddOnInfo> = INSTALLED
        .iter()
        .map(|n| super::addons::info_from_toc(n, &read_toc(root, n)))
        .collect();
    s.register_addons(registry.clone(), Some(root.to_path_buf()), None, None);
    s.set_realm_name("Harness");
    let player = super::seat_from_roster(&roster()).expect("a pending pick seats a player");
    if seat == Seat::BeforeAddons {
        // A `"player"` push with a name also seeds the player record, as the entry load does, so
        // `UnitName("player")` answers at addon file scope and Bagnon reads the live bags.
        s.set_unit("player", Some(player.clone()));
    }
    let failures = super::load_default_ui(&s);
    assert!(
        failures.is_empty(),
        "our own FrameXML failed to load: {failures:#?}"
    );

    // A 16-slot backpack with one item, carrying an id and a link: `GameTooltip:SetBagItem` and
    // `PickupContainerItem` need both, and an icon alone makes every hover and click a no-op.
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            count: 1,
            item_id: 4496,
            quality: Some(1),
            link: Some("|cffffffff|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
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

    // The startup walk: dependencies first, LoadOnDemand skipped, one `ADDON_LOADED` each.
    let mut done: Vec<String> = Vec::new();
    for name in INSTALLED {
        walk(&mut s, root, name, &mut done, &mut registry);
    }
    // Re-registered with the startup set marked loaded, so `LoadAddOn` finds its dependencies met.
    s.register_addons(registry, Some(root.to_path_buf()), None, None);
    for event in ["VARIABLES_LOADED", "PLAYER_LOGIN"] {
        s.fire_event(event, Vec::new());
    }
    // The self descriptor lands in both arms: stock `GetKeyRingSize` reads `UnitLevel("player")`
    // (`ContainerFrame.lua:773-786`), 12 slots at level 60 and 4 before the object exists.
    s.set_unit("player", Some(descriptor_snapshot(&player)));
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::new());
    for _ in 0..10 {
        s.tick(0.1);
    }
    s
}

fn walk(s: &mut UiScript, root: &Path, name: &str, done: &mut Vec<String>, reg: &mut [AddOnInfo]) {
    if done.iter().any(|d| d.eq_ignore_ascii_case(name)) {
        return;
    }
    let toc = read_toc(root, name);
    if toc.load_on_demand() {
        return; // the reference's `0x51f600` loads only records whose LoadOnDemand byte is 0
    }
    done.push(name.to_string());
    for dep in toc.dependencies() {
        if INSTALLED.iter().any(|n| n.eq_ignore_ascii_case(dep)) {
            walk(s, root, dep, done, reg);
        }
    }
    load_addon_files(s, root, name);
    if let Some(i) = reg.iter().position(|a| a.name.eq_ignore_ascii_case(name)) {
        reg[i].loaded = true;
    }
    s.fire_event(
        "ADDON_LOADED",
        vec![benilla_ui::script::ScriptValue::Str(name.to_string())],
    );
}

/// The item-slot quads Bagnon's own buttons drew: the `UI-Quickslot2` ring, which even an empty
/// slot paints. Stock item and action buttons paint it too, so each quad is charged to its nearest
/// named frame ([`UiScript::target_owner_name`]) and only `BagnonItem*` counts.
fn bagnon_slot_quads(s: &mut UiScript) -> usize {
    s.resolve();
    s.extract()
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                     if p.contains("UI-Quickslot2"))
        })
        .filter(|q| {
            s.target_owner_name(q.target)
                .is_some_and(|n| n.starts_with("BagnonItem"))
        })
        .count()
}

/// Bagnon's window open over the fixture's backpack; its `ToggleBackpack` demand-loads the addon.
fn open_bagnon(root: &Path) -> UiScript {
    let mut s = seat(root, Seat::BeforeAddons);
    s.run("ToggleBackpack()").expect("ToggleBackpack");
    for _ in 0..3 {
        s.tick(0.1);
    }
    assert!(
        s.eval::<bool>("return Bagnon and Bagnon:IsVisible() or false")
            .unwrap(),
        "the window must be on screen before anything is clicked"
    );
    s.resolve();
    s
}

/// A named frame's centre, in the y-up UI space the mouse calls take.
fn centre_of(s: &mut UiScript, name: &str) -> (f32, f32) {
    s.resolve();
    let r: Vec<f32> = s
        .eval(&format!(
            "local f = getglobal(\"{name}\") \
             return {{ f:GetLeft(), f:GetBottom(), f:GetWidth(), f:GetHeight() }}"
        ))
        .unwrap_or_else(|e| panic!("{name}: no resolved rect: {e}"));
    assert_eq!(r.len(), 4, "{name}: unresolved rect {r:?}");
    (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}

/// The Bagnon item button showing `bag`/`slot`, found by ID: Bagnon orders its buttons itself.
fn bagnon_button_for_slot(s: &UiScript, bag: i64, slot: u32) -> String {
    s.eval::<String>(&format!(
        "for i = 1, 36 do \
           local b = getglobal(\"BagnonItem\"..i) \
           if b and b:GetID() == {slot} and b:GetParent():GetID() == {bag} then \
             return \"BagnonItem\"..i \
           end \
         end \
         return \"\""
    ))
    .inspect(|n| assert!(!n.is_empty(), "no BagnonItem* is bag {bag} slot {slot}"))
    .expect("the item-button scan")
}

/// Press and release `button` over `name` through the engine's whole mouse path, as a player does.
fn click(s: &mut UiScript, name: &str, button: &str) {
    let (x, y) = centre_of(s, name);
    s.mouse_move(x, y);
    s.mouse_button(x, y, button, true);
    s.mouse_button(x, y, button, false);
}

/// Opening the bags with Bagnon installed draws every live slot. The count is the check: a window
/// with no slots raises nothing.
#[test]
fn bagnon_draws_a_slot_for_every_bag_slot() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = seat(&root, Seat::BeforeAddons);

    // Bagnon's `ToggleBackpack` demand-loads the `Bagnon` addon on its first call.
    s.run("ToggleBackpack()").expect("ToggleBackpack");
    for _ in 0..3 {
        s.tick(0.1);
    }

    assert!(
        s.eval::<bool>("return Bagnon and Bagnon:IsVisible() or false")
            .unwrap(),
        "the window must be on screen"
    );
    // 28: the backpack's 16 and the keyring's 12. Bagnon's bag set includes the keyring
    // (`Bagnon.lua:25`), sized by stock `GetKeyRingSize` (`ContainerFrame.lua:773-786`): 4, 8 at
    // 40, 12 at 50, 16 only above 60. The four bag slots are empty.
    assert_eq!(
        s.eval::<i64>("return Bagnon.size").unwrap(),
        28,
        "Bagnon must size itself off the LIVE bags (GetContainerNumSlots(0) == 16 plus \
         GetKeyRingSize() == 12 at level 60), not off Bagnon_Forever's empty offline cache — nil \
         UnitName('player') is what sent it there"
    );
    assert_eq!(
        bagnon_slot_quads(&mut s),
        28,
        "every live slot must actually be DRAWN (16 backpack + 12 keyring) — the director saw a \
         title and a gold line and nothing else, while every column of the addon survey called \
         this addon fine"
    );
    assert!(
        s.errors().is_empty(),
        "and nothing may raise on the way: {:#?}",
        s.errors()
    );
}

/// The control: seated after the addons load, Bagnon shows its title and money but no slot, and
/// raises nothing. If this starts drawing slots, the test above has lost its subject.
#[test]
fn without_a_player_at_addon_load_bagnon_draws_an_empty_window() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = seat(&root, Seat::AfterAddons);
    s.run("ToggleBackpack()").expect("ToggleBackpack");
    for _ in 0..3 {
        s.tick(0.1);
    }

    assert!(
        s.eval::<bool>("return Bagnon:IsVisible()").unwrap(),
        "the window is up"
    );
    assert_eq!(
        s.eval::<String>("return BagnonTitle:GetText()").unwrap(),
        "Harness's Inventory",
        "…with its header"
    );
    assert_eq!(
        bagnon_slot_quads(&mut s),
        0,
        "no slot is drawn — this is the report, reproduced"
    );
    assert!(
        s.eval::<String>("return tostring(BagnonItem1)").unwrap() == "nil",
        "not one item button was even created"
    );
}

/// The stock `SetItemButton*` family (`ItemButtonTemplate.lua`), which ends every Bagnon slot
/// update: a raise there aborts `BagnonFrame_AddBag`'s slot loop and the window never shows.
#[test]
fn the_item_button_helpers_paint_a_slots_icon_and_count() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().expect("VM");
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads at world entry, so a player exists before the manifest loads.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "our own FrameXML: {failures:#?}");

    // A button with the children the family finds by name, built as an addon builds it.
    s.run(
        r#"
        local b = CreateFrame("Button", "ProbeItemButton", UIParent)
        b:SetWidth(37) b:SetHeight(37)
        b:SetPoint("CENTER", UIParent, "CENTER", 0, 0)
        local icon = b:CreateTexture("ProbeItemButtonIconTexture", "BORDER")
        icon:SetAllPoints(b)
        local count = b:CreateFontString("ProbeItemButtonCount", "OVERLAY", "NumberFontNormal")
        count:SetPoint("BOTTOMRIGHT", b, "BOTTOMRIGHT", -5, 2)
        count:Hide()
    "#,
    )
    .expect("probe button");

    s.run(r#"SetItemButtonTexture(ProbeItemButton, "Interface\\Icons\\INV_Misc_Bag_08")"#)
        .expect("SetItemButtonTexture");
    s.run("SetItemButtonCount(ProbeItemButton, 5)")
        .expect("SetItemButtonCount");
    s.run("SetItemButtonDesaturated(ProbeItemButton, 1, 0.5, 0.5, 0.5)")
        .expect("SetItemButtonDesaturated");

    // Scoped to the probe button: the action-bar hotkeys also paint "1" to "0".
    let count_text = |s: &UiScript, want: &str| {
        s.extract().iter().any(|q| {
            matches!(&q.content, QuadContent::Text { text, .. } if text.as_deref() == Some(want))
                && s.target_owner_name(q.target).as_deref() == Some("ProbeItemButton")
        })
    };

    s.resolve();
    assert!(
        s.extract().iter().any(|q| matches!(&q.content,
            QuadContent::Texture { path: Some(p), .. } if p.contains("INV_Misc_Bag_08"))),
        "the icon the addon asked for must be on the region"
    );
    assert!(
        count_text(&s, "5"),
        "a stack of 5 shows its count (>1 is the reference's own gate)"
    );
    // `SetDesaturated` answers shader support, so the icon greys and keeps the caller's 0.5 tint.
    let tint = s
        .eval::<Vec<f32>>("return {ProbeItemButtonIconTexture:GetVertexColor()}")
        .expect("GetVertexColor");
    assert_eq!(
        &tint[..3],
        &[0.5, 0.5, 0.5],
        "a locked item greys out (ref: `elseif not r or not shaderSupported`)"
    );

    // A count of 1 hides the label again (`ItemButtonTemplate.lua:12`).
    s.run("SetItemButtonCount(ProbeItemButton, 1)").unwrap();
    s.resolve();
    assert!(
        !count_text(&s, "1") && !count_text(&s, "5"),
        "a single item shows no count"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// What the roster seat puts in the VM, and `None` with no pick, so a capture loads unchanged.
#[test]
fn the_roster_seat_names_the_character_the_addons_will_meet() {
    let seat = super::seat_from_roster(&roster()).expect("a pending pick seats a player");
    assert_eq!(seat.name.as_deref(), Some("Harness"));
    // 0, not 60: `UnitLevel` (`0x517fc0`) has no `"player"` fast path, so with no object it pushes
    // the number 0 (`0x51813e`). The level arrives with the descriptor.
    assert_eq!(seat.level, 0);
    assert_eq!(seat.race_file.as_deref(), Some("Human"));
    assert_eq!(seat.class_file.as_deref(), Some("WARRIOR"));
    assert_eq!(seat.sex, 2, "the wire's 0 is UnitSex's 2");
    assert_eq!(
        seat.faction_group.as_deref(),
        Some("Alliance"),
        "nil here is 24 corpus addons stopping on AceDB-2.0's file-scope concatenation"
    );
    // `UnitExists` (`0x515fb0`) takes the GUID off the object (`0x515994`); with none, the roster
    // fallback (`0x491900`) bails on the zero GUID (`0x4e80aa`) and it pushes nil (`0x516001`).
    assert!(!seat.exists);
    assert!(seat.is_player);
    // Left for the descriptor to say.
    assert_eq!((seat.health, seat.max_health), (0, 0));

    assert!(
        super::seat_from_roster(&crate::char_select::Roster::default()).is_none(),
        "no pick, no seat — a capture must load exactly as it did before"
    );
}

// ── Interaction: hover, click and drag through the engine's mouse path ──

/// `Bagnon_Core/core/Item.lua:136` ends a hover in stock `ContainerFrameItemButton_OnEnter`.
#[test]
fn hovering_a_bagnon_slot_shows_the_items_tooltip() {
    let root = corpus_and_install_or_skip!();
    let mut s = open_bagnon(&root);
    let occupied = bagnon_button_for_slot(&s, 0, 1);

    let (x, y) = centre_of(&mut s, &occupied);
    s.mouse_move(x, y);

    assert!(
        s.errors().is_empty(),
        "a hover must not raise: {:#?}",
        s.errors()
    );
    assert!(
        s.eval::<bool>(&format!("return GameTooltip:IsOwned({occupied})"))
            .unwrap(),
        "the tooltip must belong to the hovered slot ({occupied})"
    );
    assert!(
        s.eval::<bool>("return GameTooltip:IsShown()").unwrap(),
        "…and be on screen"
    );

    let (ex, ey) = (x, y + 400.0);
    s.mouse_move(ex, ey);
    assert!(
        !s.eval::<bool>("return GameTooltip:IsShown()").unwrap(),
        "moving off the slot hides the tooltip"
    );
    assert!(s.errors().is_empty(), "…without raising: {:#?}", s.errors());
}

/// `Item.lua:105` routes a plain click to the stock `ContainerFrameItemButton_OnClick`, whose left
/// arm is `PickupContainerItem(this:GetParent():GetID(), this:GetID())`.
#[test]
fn left_clicking_a_bagnon_slot_picks_the_item_up() {
    let root = corpus_and_install_or_skip!();
    let mut s = open_bagnon(&root);
    let occupied = bagnon_button_for_slot(&s, 0, 1);

    assert!(
        !s.eval::<bool>("return CursorHasItem()").unwrap(),
        "nothing is held before the click"
    );
    click(&mut s, &occupied, "LeftButton");

    assert!(
        s.errors().is_empty(),
        "a left click must not raise: {:#?}",
        s.errors()
    );
    assert!(
        s.eval::<bool>("return CursorHasItem()").unwrap(),
        "the item is now on the cursor"
    );

    let empty = bagnon_button_for_slot(&s, 0, 2);
    click(&mut s, &empty, "LeftButton");
    assert_eq!(
        s.take_container_moves(),
        vec![benilla_ui::script::ContainerMove {
            src_bag: 0,
            src_slot: 1,
            dst_bag: 0,
            dst_slot: 2,
            count: None,
        }],
        "the place half of the same gesture"
    );
    assert!(s.errors().is_empty(), "…nor the place: {:#?}", s.errors());
}

/// With no merchant open, the stock right-click arm is `UseContainerItem(bag, slot)`.
#[test]
fn right_clicking_a_bagnon_slot_uses_the_item() {
    let root = corpus_and_install_or_skip!();
    let mut s = open_bagnon(&root);
    let occupied = bagnon_button_for_slot(&s, 0, 1);

    click(&mut s, &occupied, "RightButton");

    assert!(
        s.errors().is_empty(),
        "a right click must not raise: {:#?}",
        s.errors()
    );
    assert_eq!(
        s.take_container_uses(),
        vec![(0, 1)],
        "right-click uses the slot"
    );
}

/// Bagnon routes `OnDragStart` and `OnReceiveDrag` to `ContainerFrameItemButton_OnClick`'s
/// `ignoreModifiers` arm; driven as press, move past the threshold, release.
#[test]
fn dragging_a_bagnon_slot_picks_the_item_up() {
    let root = corpus_and_install_or_skip!();
    let mut s = open_bagnon(&root);
    let occupied = bagnon_button_for_slot(&s, 0, 1);
    let empty = bagnon_button_for_slot(&s, 0, 2);

    let (sx, sy) = centre_of(&mut s, &occupied);
    let (dx, dy) = centre_of(&mut s, &empty);
    s.mouse_move(sx, sy);
    s.mouse_button(sx, sy, "LeftButton", true);
    s.mouse_move(sx + 12.0, sy + 12.0); // past DRAG_START_THRESHOLD ⇒ OnDragStart
    s.mouse_move(dx, dy);
    s.mouse_button(dx, dy, "LeftButton", false);

    assert!(
        s.errors().is_empty(),
        "a drag must not raise: {:#?}",
        s.errors()
    );
    assert_eq!(
        s.take_container_moves(),
        vec![benilla_ui::script::ContainerMove {
            src_bag: 0,
            src_slot: 1,
            dst_bag: 0,
            dst_slot: 2,
            count: None,
        }],
        "drag-and-drop moved the item"
    );
}

/// Bagnon calls the backpack button's captured `OnEnter` back with no arguments
/// (`Bagnon.lua:87`): an XML script body reads its frame off `this`, never an argument.
#[test]
fn the_backpack_button_still_hovers_with_bagnon_holding_its_script() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon(&root);

    // Bagnon must hold the script, or this proves nothing.
    assert!(
        s.eval::<bool>(
            "return MainMenuBarBackpackButton:GetScript(\"OnEnter\") == BagnonBlizMainBag_OnEnter"
        )
        .unwrap(),
        "Bagnon_AddBagHooks must have replaced the backpack button's OnEnter"
    );

    let (x, y) = centre_of(&mut s, "MainMenuBarBackpackButton");
    s.mouse_move(x, y);

    assert!(
        s.errors().is_empty(),
        "the hooked hover must not raise: {:#?}",
        s.errors()
    );
    assert!(
        s.eval::<bool>("return GameTooltip:IsOwned(MainMenuBarBackpackButton)")
            .unwrap(),
        "our own tooltip still opens, owned by the button"
    );
}

/// `Bagnon_Forever/database/ui.lua:61` calls `GetTextWidth` on a CheckButton, a 1.12 Button method
/// (`0x782290`); Button has no `GetStringWidth`.
#[test]
fn a_button_reports_its_own_label_width() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon(&root);

    s.run(
        r#"
        local b = CreateFrame("CheckButton", "ProbeWidthButton", UIParent, "BagnonDBUINameBox")
        b:SetPoint("CENTER", UIParent, "CENTER", 0, 0)
        b:SetText("ProbeLabelText")
    "#,
    )
    .expect("the probe button");
    s.resolve();

    // Answer the measure for this label alone, so the value read back proves the button forwards
    // to its label; a headless VM otherwise measures 0.
    let req = s
        .fontstrings_needing_measure()
        .into_iter()
        .find(|r| r.text == "ProbeLabelText")
        .expect("the label asks to be measured");
    s.set_measured_text_unwrapped(&[(req.id, 61.0, 12.0, req.key)]);

    assert_eq!(
        s.eval::<f64>("return ProbeWidthButton:GetTextWidth()")
            .expect("Button:GetTextWidth"),
        61.0,
        "the Button reports its LABEL's natural extent"
    );
    assert_eq!(
        s.eval::<f64>("return ProbeWidthButton:GetTextHeight()")
            .expect("Button:GetTextHeight"),
        12.0
    );
    // A Button with no label answers 0; the reference reads a FontString pointer that a bare
    // `CreateFrame("Button")` leaves null, and what it does then is untraced.
    s.run(r#"CreateFrame("Button", "ProbeLabellessButton", UIParent)"#)
        .unwrap();
    assert_eq!(
        s.eval::<f64>("return ProbeLabellessButton:GetTextWidth()")
            .unwrap(),
        0.0
    );

    s.run("BagnonDBUI_ShowCharacterList(Bagnon)")
        .expect("BagnonDBUI_ShowCharacterList");
    assert!(
        s.errors().is_empty(),
        "the character list must not raise: {:#?}",
        s.errors()
    );
}

/// `Bagnon_Core/core/Item.xml:35` gives every item button a stock `CooldownFrameTemplate` child.
#[test]
fn the_reference_cooldown_template_resolves() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let s = open_bagnon(&root);
    let occupied = bagnon_button_for_slot(&s, 0, 1);

    assert!(
        s.eval::<bool>(&format!("return getglobal(\"{occupied}Cooldown\") ~= nil"))
            .unwrap(),
        "the template's $parentCooldown child must exist"
    );
    s.run(&format!(
        "CooldownFrame_SetTimer(getglobal(\"{occupied}Cooldown\"), GetTime(), 30, 1)"
    ))
    .expect("CooldownFrame_SetTimer on an addon's cooldown child");
    assert!(s.errors().is_empty(), "{:#?}", s.errors());
}

/// Every gesture a player makes in Bagnon's window, in one VM, with no error at the end: a scan
/// finds only what it knows to look for, this finds whatever is missing.
#[test]
fn the_whole_bagnon_window_survives_being_used() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon(&root);

    // 1 · Every slot, occupied and empty, hovered and left: OnEnter, OnUpdate and OnLeave.
    let size = s.eval::<i64>("return Bagnon.size").expect("Bagnon.size");
    for i in 1..=size {
        let name = format!("BagnonItem{i}");
        if s.eval::<bool>(&format!("return getglobal(\"{name}\") ~= nil"))
            .unwrap_or(false)
        {
            let (x, y) = centre_of(&mut s, &name);
            s.mouse_move(x, y);
            s.tick(0.4); // Bagnon's own 0.3s OnUpdate tooltip refresh
            s.mouse_move(x, y + 400.0);
        }
    }

    // 2 · The window's own furniture: the title (a tooltip), the bag row toggle, the close button.
    for name in ["BagnonTitle", "BagnonShowBags", "BagnonCloseButton"] {
        if s.eval::<bool>(&format!("return getglobal(\"{name}\") ~= nil"))
            .unwrap_or(false)
        {
            let (x, y) = centre_of(&mut s, name);
            s.mouse_move(x, y);
        }
    }
    click(&mut s, "BagnonShowBags", "LeftButton"); // show the bag row

    // 3 · Bagnon's own bag buttons, `BagnonBags0..4` and the keyring's `BagnonBags-2`: hover,
    //     click and drag each (`PutItemInBackpack`, `PutItemInBag`, `PickupBagFromSlot`).
    for id in ["0", "1", "2", "3", "4", "-2"] {
        let name = format!("BagnonBags{id}");
        if !s
            .eval::<bool>(&format!(
                "local b = getglobal(\"{name}\") return b ~= nil and b:IsVisible()"
            ))
            .unwrap_or(false)
        {
            continue;
        }
        let (x, y) = centre_of(&mut s, &name);
        s.mouse_move(x, y);
        s.mouse_button(x, y, "LeftButton", true);
        s.mouse_button(x, y, "LeftButton", false);
        s.mouse_button(x, y, "LeftButton", true);
        s.mouse_move(x + 12.0, y + 12.0);
        s.mouse_button(x + 12.0, y + 12.0, "LeftButton", false);
    }

    // 4 · The three coin buttons: `BagnonFrameMoney_OnClick` (`Frame.lua:537-543`) calls
    //     `OpenCoinPickupFrame` unguarded, as the stock `MoneyFrame.xml` coin buttons do.
    for coin in ["Gold", "Silver", "Copper"] {
        let name = format!("BagnonMoneyFrame{coin}Button");
        if s.eval::<bool>(&format!(
            "local b = getglobal(\"{name}\") return b ~= nil and b:IsVisible()"
        ))
        .unwrap_or(false)
        {
            click(&mut s, &name, "LeftButton");
        }
    }

    // 5 · The bag-bar buttons Bagnon hooks, and the keyring button: hover and click each.
    for name in [
        "MainMenuBarBackpackButton",
        "CharacterBag0Slot",
        "CharacterBag1Slot",
        "KeyRingButton",
    ] {
        if !s
            .eval::<bool>(&format!(
                "local b = getglobal(\"{name}\") return b ~= nil and b:IsVisible()"
            ))
            .unwrap_or(false)
        {
            continue;
        }
        let (x, y) = centre_of(&mut s, name);
        s.mouse_move(x, y);
        s.mouse_button(x, y, "LeftButton", true);
        s.mouse_button(x, y, "LeftButton", false);
        s.mouse_move(x, y + 400.0);
    }

    // A closed list of known gaps, `(name, where)`: an unlisted error fails, and so does a listed
    // one that no longer raises. Each entry states its reason; empty is the list's best state.
    const KNOWN_GAPS: &[(&str, &str)] = &[];

    let raised = s.errors();
    let unexplained: Vec<&String> = raised
        .iter()
        .filter(|e| !KNOWN_GAPS.iter().any(|(name, _)| e.contains(name)))
        .collect();
    assert!(
        unexplained.is_empty(),
        "using Bagnon raised {} error(s) that are NOT on the known-gap list:\n{:#?}",
        unexplained.len(),
        unexplained
    );
    for (name, site) in KNOWN_GAPS {
        assert!(
            raised.iter().any(|e| e.contains(name)),
            "`{name}` is listed as a known gap ({site}) but nothing raised on it — if it is \
             built now, delete the entry"
        );
    }
}

/// Bagnon's window over a stack of 200: `SetItemButtonCount` paints only above 1.
fn open_bagnon_stacked(root: &Path) -> UiScript {
    let mut s = seat(root, Seat::BeforeAddons);
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            count: 200,
            item_id: 4496,
            quality: Some(1),
            link: Some("|cffffffff|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
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
    s.run("ToggleBackpack()").expect("ToggleBackpack");
    for _ in 0..3 {
        s.tick(0.1);
    }
    s.resolve();
    s
}

/// The text quad owned by the frame named `owner`.
fn text_quad(s: &mut UiScript, owner: &str) -> QuadContent {
    s.resolve();
    s.extract()
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Text { text: Some(_), .. })
                && s.target_owner_name(q.target).as_deref() == Some(owner)
        })
        .map(|q| q.content.clone())
        .unwrap_or_else(|| panic!("no text quad owned by {owner}"))
}

/// A FontString's `font=` (`Bagnon_Core/core/Item.xml:14`) names a font object before a file: the
/// reference asks the registry first (`0x783d15` calls `0x783870`). A stack count's dark edge is
/// `NumberFontNormal`'s outline; it has no `<Shadow>` (`Fonts.xml:150`).
#[test]
fn a_stack_count_wears_the_font_object_its_font_attr_names() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon_stacked(&root);

    let QuadContent::Text {
        text,
        font,
        font_height,
        outline,
        shadow,
        color,
        ..
    } = text_quad(&mut s, "BagnonItem1")
    else {
        unreachable!("filtered to Text above")
    };

    assert_eq!(text.as_deref(), Some("200"), "the stack count is on screen");
    // The face proves the registry was consulted; the outline is the part that shows.
    assert_eq!(
        font.as_deref(),
        Some("Fonts\\ARIALN.TTF"),
        "`font=\"NumberFontNormal\"` must resolve the font OBJECT, not become a font path"
    );
    assert_eq!(font_height, Some(14.0), "NumberFontNormal's height");
    assert_eq!(
        outline,
        benilla_ui::script::Outline::Normal,
        "the black ring around a stack count IS `outline=\"NORMAL\"` — this is the reported symptom"
    );
    assert_eq!(
        color,
        Some([1.0, 1.0, 1.0, 1.0]),
        "NumberFontNormal's white"
    );
    assert_eq!(
        shadow, None,
        "and NO drop shadow: NumberFontNormal has no <Shadow> in 1.12 either (Fonts.xml:226). \
         The readability is the outline; adding a shadow here would be a divergence"
    );
}

/// `BagnonPopupFrame` (`Bagnon_Core/core/Frame.xml:23-36`) tints white `ChatFrameBackground` art
/// with a `<Gradient>`; untinted, the dropdown is a white box. A `<Gradient>` survives beside
/// `file=` where `<Color>` does not: it writes the vertex colours (`+0xb8`, by `0x77f910`),
/// `<Color>` the texture (`+0xcc`, by `0x770360`).
#[test]
fn a_texture_gradient_tints_the_art_it_sits_on() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon_stacked(&root);
    s.run("BagnonDBUI_ShowCharacterList(Bagnon)")
        .expect("BagnonDBUI_ShowCharacterList");
    for _ in 0..3 {
        s.tick(0.1);
    }
    s.resolve();

    let bg = s
        .extract()
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                     if p.contains("ChatFrameBackground"))
                && s.target_owner_name(q.target).as_deref() == Some("BagnonDBUICharacterList")
        })
        .map(|q| q.content.clone())
        .expect("the popup's background art reaches the render list");

    let QuadContent::Texture { color, .. } = bg else {
        unreachable!("filtered to Texture above")
    };
    // The stops are (0,0,0,0.9) and (0.2,0.2,0.2,0.9). A quad carries one tint, so the paint shows
    // their midpoint, 0.1 grey; the reference shades per vertex.
    assert_eq!(
        color,
        Some([0.1, 0.1, 0.1, 0.9]),
        "the <Gradient> must tint the art; untinted this is the reported white box"
    );
}

/// Stock `MoneyFrame_Update` sets each coin's text, then its width from `GetTextWidth()`
/// (`MoneyFrame.lua:201-208`); the font measurer answers inside that call, so the first open's
/// digits are not cramped.
#[test]
fn a_money_frame_is_the_right_width_on_the_first_open() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = seat(&root, Seat::BeforeAddons);

    // 12345g 67s 89c: three digit counts, so a width that ignores the digits cannot pass.
    s.set_money(123_456_789);
    // Set before any window opens, as the app does; a flat 8px advance keeps the widths exact.
    s.set_text_measurer(Box::new(super::FixedWidthFont(8.0)));

    s.run("ToggleBackpack()").expect("ToggleBackpack");
    for _ in 0..3 {
        s.tick(0.1);
    }
    s.resolve();

    let widths: Vec<f32> = s
        .eval(
            "local o = {} \
             for _, n in ipairs({\"Gold\", \"Silver\", \"Copper\"}) do \
               local b = getglobal(\"BagnonMoneyFrame\" .. n .. \"Button\") \
               table.insert(o, b and b:GetWidth() or -1) \
             end \
             return o",
        )
        .expect("the three coin buttons");

    // `MONEY_ICON_WIDTH_SMALL` is 13 (`MoneyFrame.lua:3`) and a digit 8px: 5, 2 and 2 digits.
    assert_eq!(
        widths,
        vec![5.0 * 8.0 + 13.0, 2.0 * 8.0 + 13.0, 2.0 * 8.0 + 13.0],
        "each coin must be its digits PLUS its icon on the first open. All three coming back at \
         13.0 — the bare icon width — is the reported cramping: it means the width was taken from \
         a text measure that had not landed yet"
    );
}

/// The font a FontString/Button label actually resolved: `(path, height)`.
fn resolved_font(s: &UiScript, lua_expr: &str) -> (String, String) {
    let r: Vec<String> = s
        .eval(&format!(
            "local fs = {lua_expr} \
             if not fs then return {{ \"MISSING\", \"MISSING\" }} end \
             local p, h = fs:GetFont() \
             return {{ tostring(p), tostring(h) }}"
        ))
        .expect("GetFont");
    (r[0].clone(), r[1].clone())
}

/// A Button's `<NormalFont>`, `<HighlightFont>` and `<DisabledFont>` take `font=` too: they are
/// `<Font>` elements, routed by `CSimpleButton::LoadXML` (`0x7788c0`, at `0x778bf4`) into the
/// `0x783c30` a top-level `<Font>` uses, so `font=` resolves registry-first (`0x783d15`, then
/// `0x783d22` calls `0x770c60`). 1.12.1 has no `style=` attribute.
#[test]
fn a_button_state_font_takes_the_font_attribute_not_just_inherits() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon_stacked(&root);
    s.run("BagnonDBUI_ShowCharacterList(Bagnon)")
        .expect("BagnonDBUI_ShowCharacterList");
    for _ in 0..3 {
        s.tick(0.1);
    }

    assert_eq!(
        resolved_font(
            &s,
            "getglobal(\"BagnonDBUICharacterList1\"):GetFontString()"
        ),
        ("Fonts\\FRIZQT__.TTF".into(), "16".into()),
        "`<NormalFont font=\"GameFontNormalLarge\"/>` must link the font object — 16px, not the \
         12px default, and emphatically not the nil this returned while only `inherits=` was read"
    );
}

/// `CSimpleFontString::LoadXML` reads `<FontHeight>`, `outline=` and `monochrome=` only on the leg
/// where `font=` names a file (`[0x77111e, 0x771254)`); with no `font=`, `0x7710f1`/`0x7710fa`
/// jump to `0x771254`. Stock `AutoFollowStatusText` (`ZoneText.xml:172-183`) inherits
/// `GameFontNormal` (12px) beside a dead `<FontHeight val="20">`.
#[test]
fn a_fontheight_with_no_font_attr_beside_it_is_never_read() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let s = open_bagnon_stacked(&root);
    assert_eq!(
        resolved_font(&s, "AutoFollowStatusText"),
        ("Fonts\\FRIZQT__.TTF".into(), "12".into()),
        "AutoFollowStatusText inherits GameFontNormal (12) and declares <FontHeight val=20> with \
         no font= — the 20 is dead XML in the reference, so 20 here means we are reading an \
         attribute the client never reaches"
    );
}

/// A stand-in font engine: every character `PER_CHAR` wide, one 12px line tall.
struct BlockFont;

const PER_CHAR: f32 = 7.0;

impl benilla_ui::script::TextMeasure for BlockFont {
    fn measure(&mut self, req: &benilla_ui::script::MeasureRequest) -> (f32, f32, f32) {
        let w = req.text.chars().count() as f32 * PER_CHAR;
        (w, 12.0, w)
    }
}

/// `Bagnon_Forever/database/ui.lua:56-62` sets each row's text and reads `GetTextWidth() + 40` back
/// in the same tick; a measure that answers a tick late leaves the list 40px wide.
#[test]
fn the_character_dropdown_is_as_wide_as_the_names_in_it() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon(&root);
    s.set_text_measurer(Box::new(BlockFont));
    // Three saved characters on this realm, the longest 10 letters, in Bagnon_Forever's store.
    s.run(
        "local realm = GetRealmName() \
         BagnonForeverData[realm][\"Onewarrior\"] = { g = 100 } \
         BagnonForeverData[realm][\"Onerogue\"] = { g = 200 }",
    )
    .expect("seed the character store");
    s.run("BagnonDBUI_ShowCharacterList(Bagnon)")
        .expect("BagnonDBUI_ShowCharacterList");
    for _ in 0..3 {
        s.tick(0.1);
    }
    s.resolve();

    let widest = "Onewarrior".len() as f32 * PER_CHAR + 40.0;
    assert_eq!(
        s.eval::<f32>("return BagnonDBUICharacterList:GetWidth()")
            .unwrap(),
        widest,
        "the list sizes itself from its rows' own text width, read in the tick that set it"
    );
    // A row's label starts 24px in (`Bagnon_Forever/database/ui.xml:36`) and must end inside.
    let (list_r, row_r): (f32, f32) = s
        .eval::<Vec<f32>>(
            "local l = BagnonDBUICharacterList \
             return { l:GetLeft() + l:GetWidth(), BagnonDBUICharacterList1:GetLeft() + \
             BagnonDBUICharacterList1:GetTextWidth() + 24 }",
        )
        .map(|v| (v[0], v[1]))
        .unwrap();
    assert!(
        row_r <= list_r,
        "a row's text must end inside the list: text right edge {row_r} > list right edge {list_r}"
    );
}

/// Bagnon_Forever scans every bag only at a character's first login, then trusts `BAG_UPDATE`, and
/// `SaveBagData` deletes a bag's record when `GetContainerNumSlots` reads 0: the logout despawn
/// frame, with no self store, must not erase the records before the saved-variables write.
#[test]
fn bagnon_forevers_records_survive_the_logout_boundary() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = seat(&root, Seat::BeforeAddons);

    // The first-login scan (PLAYER_LOGIN, in `seat`) recorded the backpack. The 1 in `16,1,` is
    // the reference's: `ContainerIDToInventoryID(0)` gives slot 19, the tabard's (`0x4f94e0`),
    // and `GetInventoryItemCount` (`0x4c8680`) answers 1 for it, empty or worn (`0x4c8797`).
    let record = "local r = BagnonForeverData[GetRealmName()][UnitName('player')] \
                  return tostring(r[0] and r[0].s), tostring(r[0] and r[0][1])";
    let (size, item) = s
        .eval::<(String, String)>(record)
        .expect("the record reads");
    assert_eq!(
        size, "16,1,",
        "the login scan records the backpack's size row"
    );
    assert_eq!(
        item, "4496",
        "the login scan records the occupied slot's short link"
    );

    // An equipped bag: the inventory slot (20) first, then the container and its `BAG_UPDATE`,
    // the order `feed_char.before(feed_containers)` holds. The size row reads the bag item's own
    // count and link, so the other order records the bag without its link.
    let mut inv: benilla_ui::script::InventorySlots = Default::default();
    inv[20] = Some(benilla_ui::script::InvSlotView {
        item_id: 4497,
        count: 1,
        quality: 1,
        name: Some("Small Green Pouch".into()),
        link: Some("|cffffffff|Hitem:4497:0:0:0|h[Small Green Pouch]|h|r".into()),
        ..Default::default()
    });
    s.set_inventory_slots(inv);
    s.set_container(
        1,
        Some(ContainerState {
            name: Some("Small Green Pouch".into()),
            num_slots: 6,
            slots: Default::default(),
        }),
    );
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(1)]);
    let bag_row = s
        .eval::<String>(
            "return tostring(BagnonForeverData[GetRealmName()][UnitName('player')][1].s)",
        )
        .expect("the bag row reads");
    assert_eq!(
        bag_row, "6,1,4497",
        "an equipped bag's size row carries its own count and short link"
    );

    // The logout boundary: the despawn frame (no self store), then the shutdown events.
    let mut memory = crate::ui_items::feed::FeedMemory::default();
    crate::ui_items::feed::apply_container_source(
        &mut s,
        &mut memory,
        None,
        Default::default(),
        Vec::new(),
        Vec::new(),
    );
    s.fire_event("PLAYER_LEAVING_WORLD", Vec::new());
    s.fire_event("PLAYER_LOGOUT", Vec::new());
    let (size, item) = s
        .eval::<(String, String)>(record)
        .expect("the record reads");
    assert_eq!(
        size, "16,1,",
        "the logout boundary must not erase the size row"
    );
    assert_eq!(item, "4496", "the logout boundary must not erase the slot");
}

/// Bagnon overrides the global `ToggleBag` (`Bagnon_Core/core/Overrides.lua`) and wraps each bag
/// slot's `OnClick`, calling the original back. Stock `BagSlotButton_OnClick`
/// (`MainMenuBarBagButtons.lua:3-21`) calls the global `ToggleBag`, then lights the button only for
/// a shown native container frame, so with Bagnon a slot click toggles Bagnon and stays unlit.
#[test]
fn a_bag_slot_click_toggles_bagnon_not_the_native_window() {
    // The install too: stock `IsBagOpen` (`ContainerFrame.lua:177`) answers whether a native
    // window shows; the windows are recycled, so none can be named.
    let root = corpus_and_install_or_skip!();
    let mut s = open_bagnon(&root);

    // An equipped bag behind `CharacterBag1Slot` (bag 2, inventory slot 21), item then container.
    let mut inv: benilla_ui::script::InventorySlots = Default::default();
    inv[21] = Some(benilla_ui::script::InvSlotView {
        item_id: 4497,
        count: 1,
        quality: 1,
        name: Some("Small Green Pouch".into()),
        link: Some("|cffffffff|Hitem:4497:0:0:0|h[Small Green Pouch]|h|r".into()),
        icon: Some("Interface\\Icons\\INV_Misc_Bag_10_Green".into()),
        ..Default::default()
    });
    s.set_inventory_slots(inv);
    s.set_container(
        2,
        Some(ContainerState {
            name: Some("Small Green Pouch".into()),
            num_slots: 6,
            slots: Default::default(),
        }),
    );
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(2)]);
    s.resolve();

    // Bagnon is open. A click must close it through both layers: the `OnClick` wrap, calling the
    // captured original back with no arguments, and the `ToggleBag` override the original calls.
    assert!(
        s.eval::<bool>("return CharacterBag1Slot:GetScript('OnClick') == BagnonBlizBag_OnClick")
            .unwrap_or(false),
        "Bagnon's SetScript OnClick wrap must be in place on the slot button"
    );
    let (x, y) = centre_of(&mut s, "CharacterBag1Slot");
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    assert!(
        !s.eval::<bool>("return Bagnon:IsVisible() or false")
            .unwrap(),
        "the slot click must reach Bagnon's ToggleBag override and close its window"
    );
    assert!(
        !super::test_ui::bag_open(&s, 2),
        "the native bag window must NOT open — the click belongs to the addon's override"
    );

    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    for _ in 0..2 {
        s.tick(0.1);
    }
    assert!(
        s.eval::<bool>("return Bagnon:IsVisible() or false")
            .unwrap(),
        "the second click re-opens Bagnon through the same override"
    );
    assert!(
        !super::test_ui::bag_open(&s, 2),
        "the native window stays shut on the re-open too"
    );
    assert!(
        !s.eval::<bool>("return CharacterBag1Slot:GetChecked() and true or false")
            .unwrap(),
        "the slot button stays unlit with an addon holding the bags (the ref scan is native-only)"
    );
    let raised = s.errors();
    assert!(
        raised.is_empty(),
        "the click round-trip raised: {raised:#?}"
    );
}

/// Bagnon lights `MainMenuBarBackpackButton` from its OnShow and OnHide (`Bagnon.lua:33`, `:39`),
/// and OnShow runs synchronously (`0x775750` → `0x76ae10`), so the last write wins. The `B`
/// binding's bare `ToggleBackpack()` ends lit; a click ends unlit either way, because stock
/// `BackpackButton_OnClick` (`MainMenuBarBagButtons.lua:55-68`) rescans the native frames last.
#[test]
fn the_backpack_button_lit_state_with_bagnon_holding_the_bags() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon(&root);

    // The fixture opened Bagnon through the bare global, so OnShow's `SetChecked(1)` stands.
    assert!(
        s.eval::<bool>("return MainMenuBarBackpackButton:GetChecked() and true or false")
            .unwrap(),
        "the bare ToggleBackpack() path (the B binding) must leave the button lit"
    );

    let (x, y) = centre_of(&mut s, "MainMenuBarBackpackButton");
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    assert!(
        !s.eval::<bool>("return Bagnon:IsVisible() or false")
            .unwrap(),
        "the click must toggle Bagnon closed through its override"
    );
    assert!(
        !s.eval::<bool>("return MainMenuBarBackpackButton:GetChecked() and true or false")
            .unwrap(),
        "closed ⇒ unlit (Bagnon's OnHide write is the last word)"
    );

    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    assert!(
        s.eval::<bool>("return Bagnon:IsVisible() or false")
            .unwrap(),
        "the second click re-opens Bagnon"
    );
    assert!(
        !s.eval::<bool>("return MainMenuBarBackpackButton:GetChecked() and true or false")
            .unwrap(),
        "open by click ⇒ unlit: the stock BackpackButton_OnClick tail's native scan is the last \
         word, as on the reference (Bug 7's divergence retired with the stock bag bar, 1783)"
    );
}
