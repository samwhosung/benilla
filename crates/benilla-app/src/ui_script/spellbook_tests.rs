//! The stock spellbook window (`SpellBookFrame.xml`) over a small synthetic book, engine only.

use benilla_ui::script::{SpellBookState, SpellSlotView, SpellTabView, UiScript};

use super::test_ui::load_ui as load_xml;

/// Fire (two spells) and Frost (one); `offset` is a tab's 0-based start in the flat `slots`.
fn book() -> SpellBookState {
    SpellBookState {
        tabs: vec![
            SpellTabView {
                name: "Fire".into(),
                texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
                offset: 0,
                num_spells: 2,
            },
            SpellTabView {
                name: "Frost".into(),
                texture: Some("Interface\\Icons\\Spell_Frost_FrostBolt02".into()),
                offset: 2,
                num_spells: 1,
            },
        ],
        slots: vec![
            SpellSlotView {
                spell_id: 133,
                name: "Fireball".into(),
                rank: Some("Rank 1".into()),
                texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
                passive: false,
                current: false,
                cooldown: None,
                ..Default::default()
            },
            SpellSlotView {
                spell_id: 2136,
                name: "Fire Blast".into(),
                rank: Some("Rank 1".into()),
                texture: Some("Interface\\Icons\\Spell_Fire_FireBolt02".into()),
                passive: false,
                current: false,
                cooldown: None,
                ..Default::default()
            },
            SpellSlotView {
                spell_id: 168,
                name: "Frost Armor".into(),
                rank: Some("Rank 1".into()),
                texture: Some("Interface\\Icons\\Spell_Frost_FrostArmor02".into()),
                passive: false,
                current: false,
                cooldown: None,
                ..Default::default()
            },
        ],
    }
}

/// The centre of a laid-out frame, for a click through the hit test.
fn center(s: &UiScript, name: &str) -> (f32, f32) {
    let l: f32 = s.eval(&format!("return {name}:GetLeft()")).unwrap();
    let r: f32 = s.eval(&format!("return {name}:GetRight()")).unwrap();
    let t: f32 = s.eval(&format!("return {name}:GetTop()")).unwrap();
    let b: f32 = s.eval(&format!("return {name}:GetBottom()")).unwrap();
    ((l + r) * 0.5, (t + b) * 0.5)
}

/// One full press/release of `button` on the named frame.
fn click(s: &mut UiScript, name: &str, button: &str) {
    s.resolve();
    let (x, y) = center(s, name);
    s.mouse_button(x, y, button, true);
    s.mouse_button(x, y, button, false);
}

/// The stock spellbook in manifest order, over the multibar grids and `UpdateMicroButtons` its show
/// and hide call (`SpellBookFrame.lua:94-104`, `:186-203`).
pub(super) fn spellbook_ui(w: f32, h: f32) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(w, h);
    for f in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\MainMenuBar.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        r"Interface\FrameXML\ReputationFrame.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "ScrollTemplates.xml",
        "KeyBindingsPage.xml",
        "OptionsFrame.xml",
        "Interface\\FrameXML\\MultiActionBars.xml",
        r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
        "Interface\\FrameXML\\SpellBookFrame.xml",
        "SpellBookAdapters.xml",
    ] {
        load_xml(&s, f);
    }
    s
}

#[test]
fn shipped_spellbook_loads_clean() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    let frames = load_xml(&s, "Interface\\FrameXML\\SpellBookFrame.xml");
    load_xml(&s, "SpellBookAdapters.xml");
    assert!(s.errors().is_empty(), "loader errors: {:?}", s.errors());
    assert_eq!(
        frames, 52,
        "window + close + prev/next + 12 spell buttons (each with its Cooldown and AutoCast \
         Model children) + 8 skill-line tabs + the 3 Spell/Pet toggle tabs + the tab flash frame"
    );
    for name in ["SpellBookFrame", "SpellButton12", "SpellButton1AutoCast"] {
        assert!(
            s.eval::<bool>(&format!("return {name} ~= nil")).unwrap(),
            "{name} exists"
        );
    }
}

#[test]
fn shipped_spellbook_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = spellbook_ui(1024.0, 768.0);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);

    s.set_spellbook(book());

    assert!(!s.eval::<bool>("return SpellBookFrame:IsVisible()").unwrap());
    s.run("ToggleSpellBook(BOOKTYPE_SPELL)").unwrap();
    assert!(s.errors().is_empty(), "open errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return SpellBookFrame:IsVisible()").unwrap());

    // Button ids run column-major (`SpellBookFrame.xml:387-486`): SpellButton3 is id 2, and
    // SpellButton2 is id 7, past the two-spell Fire tab that `OnLoad` selects.
    assert_eq!(
        s.eval::<String>("return SpellButton1SpellName:GetText()")
            .unwrap(),
        "Fireball"
    );
    assert_eq!(
        s.eval::<String>("return SpellButton1SubSpellName:GetText()")
            .unwrap(),
        "Rank 1"
    );
    assert_eq!(
        s.eval::<String>("return SpellButton3SpellName:GetText()")
            .unwrap(),
        "Fire Blast"
    );
    assert!(
        !s.eval::<bool>("return SpellButton2:IsEnabled() ~= 0")
            .unwrap(),
        "book id 7 is past the 2-spell Fire tab — disabled"
    );

    s.resolve();
    let (x1, y1) = center(&s, "SpellButton1");

    s.mouse_button(x1, y1, "LeftButton", true);
    s.mouse_button(x1, y1, "LeftButton", false);
    assert!(s.errors().is_empty(), "click errors: {:?}", s.errors());
    assert_eq!(s.take_spell_casts(), vec![133]);
    assert!(s.cursor_payload().is_none(), "a cast never picks up");

    s.set_modifiers(true, false, false);
    s.mouse_button(x1, y1, "LeftButton", true);
    s.mouse_button(x1, y1, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(s.take_spell_casts().is_empty(), "shift-click never casts");
    let (kind, _book_id, book_type, spell_id) = s
        .eval::<(String, i64, String, i64)>(
            "local k, slot, book, id = GetCursorInfo() return k, slot, book, id",
        )
        .unwrap();
    assert_eq!(
        (kind.as_str(), book_type.as_str(), spell_id),
        ("spell", "spell", 133)
    );

    // An empty slot is hidden (`ActionButton.lua:214-215`) until a held payload opens the grid;
    // the engine fires `ACTIONBAR_SHOWGRID` on the next tick.
    s.tick(0.016);
    assert!(
        s.eval::<bool>("return ActionButton1:IsVisible()").unwrap(),
        "the held spell opened the empty well"
    );
    let (ax, ay) = center(&s, "ActionButton1");
    s.mouse_button(ax, ay, "LeftButton", true);
    s.mouse_button(ax, ay, "LeftButton", false);
    assert!(s.errors().is_empty(), "place errors: {:?}", s.errors());
    assert!(s.cursor_payload().is_none(), "placed — cursor clears");
    assert!(s.eval::<bool>("return HasAction(1)").unwrap());
    assert_eq!(s.take_action_sets(), vec![(1, 133)]); // 0x00<<24 | 133
}

/// A running cooldown arms the button's sweep (`SpellBookFrame.lua:359-360`); an on-hold one
/// (enable 0, as Stealth until `SMSG_COOLDOWN_EVENT`) draws none and dims the icon (`:361-365`).
#[test]
fn shipped_spellbook_shows_the_cooldown_pie() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::QuadContent;

    let mut s = spellbook_ui(1024.0, 768.0);
    s.set_spellbook(book());
    s.run("ToggleSpellBook(BOOKTYPE_SPELL)").unwrap();
    s.tick(10.0); // GetTime = 10

    // A 10 s cooldown started at t=6, 6 s left.
    let mut b = book();
    b.slots[0].cooldown = Some((6_000, 10_000, true));
    s.set_spellbook(b);
    s.fire_event("SPELL_UPDATE_COOLDOWN", vec![]);
    // `CooldownFrame_SetTimer` arms sequence 0; the next paint's `OnUpdateModel` scrubs it.
    super::test_ui::cooldown_facts(&mut s);
    s.tick(0.0);
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    let play = super::test_ui::cooldown_play(&s, "SpellButton1Cooldown")
        .expect("SpellButton1's cooldown pane is showing");
    assert_eq!(
        play,
        (0, 400),
        "4 s elapsed of 10 ⇒ the sweep sits at 40 %: sequence 0 at 400 ms"
    );

    // On hold: `CooldownFrame_SetTimer`'s enable gate draws no sweep, and the icon dims to 0.4.
    let mut b = book();
    b.slots[0].cooldown = Some((6_000, 10_000, false));
    s.set_spellbook(b);
    s.fire_event("SPELL_UPDATE_COOLDOWN", vec![]);
    s.resolve();
    let quads = s.extract();
    assert!(
        !quads
            .iter()
            .any(|q| { s.quad_owner_name(q.target).as_deref() == Some("SpellButton1Cooldown") }),
        "an on-hold cooldown draws no sweep"
    );
    let icon_color = quads.iter().find_map(|q| {
        if s.quad_owner_name(q.target).as_deref() != Some("SpellButton1") {
            return None;
        }
        match &q.content {
            QuadContent::Texture {
                path: Some(p),
                color,
                ..
            } if p.contains("Spell_Fire_FlameBolt") => Some(*color),
            _ => None,
        }
    });
    let c = icon_color.expect("icon quad").expect("vertex color set");
    assert_eq!((c[0], c[1], c[2]), (0.4, 0.4, 0.4), "the on-hold 40% dim");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// An empty slot is a disabled button that keeps its `UI-Quickslot2` ring (whitened at
/// `SpellBookFrame.lua:337`): created enabled (`0x7786a0` ends in `SetState(NORMAL)`), and
/// `SetState` (`0x779790`) has no step that takes the ring off on `Disable()`. `SetChecked(0)`
/// unchecks, so there is no `CheckButtonHilight` glow.
#[test]
fn shipped_spellbook_empty_slot_draws_its_background_and_socket_ring() {
    benilla_formats::wow_data_or_skip!();
    let mut s = spellbook_ui(640.0, 700.0);
    // One spell: SpellButton5, id 3, takes the `id > offset + numSpells` disable path.
    let mut b = book();
    b.tabs.truncate(1);
    b.tabs[0].num_spells = 1;
    b.slots.truncate(1);
    s.set_spellbook(b);
    s.run("ToggleSpellBook(BOOKTYPE_SPELL)").unwrap();
    s.tick(0.05);
    s.resolve();
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());

    let mut slot5_paths: Vec<String> = Vec::new();
    for eq in s.extract() {
        if s.quad_owner_name(eq.target).as_deref() == Some("SpellButton5") {
            if let benilla_ui::script::QuadContent::Texture { path: Some(p), .. } = &eq.content {
                slot5_paths.push(p.clone());
            }
        }
    }
    assert_eq!(
        slot5_paths,
        vec![
            "Interface\\Spellbook\\UI-Spellbook-SpellBackground".to_string(),
            "Interface\\Buttons\\UI-Quickslot2".to_string(),
        ],
        "an empty slot keeps its socket ring and gains no checked glow"
    );
    // `SetChecked(0)` unchecks: a number is coerced, not tested for Lua truthiness.
    assert!(!s
        .eval::<bool>("return SpellButton5:GetChecked() and true or false")
        .unwrap());
}

/// `SetChecked`'s coercion over the arguments the stock call sites pass
/// (`SpellBookFrame.lua:132`, `:134`, `:268`, `:296-303`, `:336`).
#[test]
fn set_checked_uses_blizzard_bool_coercion() {
    benilla_formats::wow_data_or_skip!();
    let s = spellbook_ui(1024.0, 768.0);
    for (arg, want) in [
        ("1", true),
        ("0", false),
        ("nil", false),
        ("true", true),
        ("false", false),
        ("\"true\"", true),
        ("\"false\"", false),
        ("\"1\"", true),
        ("\"0\"", false),
        ("\"junk\"", false),
    ] {
        let got = s
            .eval::<bool>(&format!(
                "SpellButton1:SetChecked({arg}) \
                 return SpellButton1:GetChecked() and true or false"
            ))
            .unwrap();
        assert_eq!(got, want, "SetChecked({arg})");
    }
}

/// A pet book: Growl (autocast on, cooling down), Claw (autocast off), Avoidance (passive).
fn pet_book() -> benilla_ui::script::PetBookState {
    benilla_ui::script::PetBookState {
        token: Some("PET".into()),
        slots: vec![
            SpellSlotView {
                spell_id: 2649,
                name: "Growl".into(),
                rank: Some("Rank 1".into()),
                texture: Some("Interface\\Icons\\Ability_Physical_Taunt".into()),
                cooldown: Some((9400, 5000, true)),
                autocast: Some((true, true)),
                packed: 0xC100_0000 | 2649,
                ..Default::default()
            },
            SpellSlotView {
                spell_id: 16827,
                name: "Claw".into(),
                rank: Some("Rank 1".into()),
                texture: Some("Interface\\Icons\\Ability_Druid_Rake".into()),
                autocast: Some((true, false)),
                packed: 0x8100_0000 | 16827,
                ..Default::default()
            },
            SpellSlotView {
                spell_id: 3025,
                name: "Avoidance".into(),
                texture: Some("Interface\\Icons\\Spell_Nature_SpiritArmor".into()),
                passive: true,
                autocast: Some((false, false)),
                packed: 0x0100_0000 | 3025,
                ..Default::default()
            },
        ],
    }
}

#[test]
fn the_pet_tab_switches_books_and_renders_the_pets_spells() {
    benilla_formats::wow_data_or_skip!();
    let mut s = spellbook_ui(1024.0, 768.0);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.set_spellbook(book());

    // ── No pet: no toggle row, and no pet book (`SpellBookFrame.lua:11-14`) ──────────────────
    s.run("ToggleSpellBook(BOOKTYPE_SPELL)").unwrap();
    assert!(s.errors().is_empty(), "open errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return SpellBookFrameTabButton1:IsVisible()")
            .unwrap(),
        "with no pet spells the ref hides the whole toggle row"
    );
    s.run("ToggleSpellBook(BOOKTYPE_PET)").unwrap();
    assert!(
        s.eval::<bool>("return SpellBookFrame.bookType == BOOKTYPE_SPELL")
            .unwrap(),
        "asking for a pet book you have not got does nothing — not even close the window"
    );
    assert!(s.eval::<bool>("return SpellBookFrame:IsVisible()").unwrap());

    // ── A pet arrives: the row appears on the next repaint ────────────────────────────────────
    s.set_pet_book(pet_book());
    s.fire_event("SPELLS_CHANGED", vec![]);
    assert!(s.errors().is_empty(), "repaint errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return SpellBookFrameTabButton1:IsVisible()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return SpellBookFrameTabButton2:GetText()")
            .unwrap(),
        "Pet",
        "the label is PET_TYPE_<token>, not the token"
    );

    // ── Click the pet tab ─────────────────────────────────────────────────────────────────────
    click(&mut s, "SpellBookFrameTabButton2", "LeftButton");
    assert!(s.errors().is_empty(), "tab errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return SpellBookFrame.bookType == BOOKTYPE_PET")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return SpellBookTitleText:GetText()")
            .unwrap(),
        "Pet",
        "the window title becomes the pet's, not SPELLBOOK"
    );
    assert!(
        !s.eval::<bool>("return SpellBookSkillLineTab1:IsVisible()")
            .unwrap(),
        "the pet book has no skill lines — the whole strip hides (ref l.124)"
    );

    // The pet book's ids are the button ids (`SpellBookFrame.lua:460-461`): SpellButton3 is Claw.
    assert_eq!(
        s.eval::<String>("return SpellButton1SpellName:GetText()")
            .unwrap(),
        "Growl"
    );
    assert_eq!(
        s.eval::<String>("return SpellButton3SpellName:GetText()")
            .unwrap(),
        "Claw"
    );
    // The overlay follows `GetSpellAutocast`'s first return (can it), the shine model its second
    // (is it on) (`SpellBookFrame.lua:367-377`).
    assert!(s
        .eval::<bool>("return SpellButton1AutoCastable:IsVisible()")
        .unwrap());
    assert!(
        !s.eval::<bool>("return SpellButton5AutoCastable:IsVisible()")
            .unwrap(),
        "a passive is not autocastable"
    );
    assert!(s
        .eval::<bool>("return SpellButton1AutoCast:IsVisible()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return SpellButton3AutoCast:IsVisible()")
        .unwrap());
    // Deviation (SpellBookAdapters.xml): the brackets are 71.53 square and the shine 37x37 at
    // scale 1.48, both centred on the button, so the two share a centre; the reference's are 60,
    // and 36x36 at CENTER (1,1) and 1.22 (`SpellBookFrame.xml:121-147`), which reads off-centre.
    let br: Vec<f32> = [
        "GetWidth()",
        "GetHeight()",
        "GetLeft() + SpellButton1AutoCastable:GetWidth() / 2 - (SpellButton1:GetLeft() + SpellButton1:GetWidth() / 2)",
        "GetBottom() + SpellButton1AutoCastable:GetHeight() / 2 - (SpellButton1:GetBottom() + SpellButton1:GetHeight() / 2)",
    ]
    .iter()
    .map(|e| {
        s.eval::<f32>(&format!("return SpellButton1AutoCastable:{e}"))
            .unwrap()
    })
    .collect();
    assert!(
        (br[0] - 71.53).abs() < 0.01 && (br[1] - 71.53).abs() < 0.01,
        "brackets are {}x{}, 1393 draws them at 71.53 so the art reaches this button's corners",
        br[0],
        br[1]
    );
    assert!(
        br[2].abs() < 0.01 && br[3].abs() < 0.01,
        "brackets sit ({}, {}) off the button's centre; the ref centres them exactly",
        br[2],
        br[3]
    );

    let geom: Vec<f32> = ["GetWidth", "GetHeight", "GetModelScale"]
        .iter()
        .map(|m| {
            s.eval::<f32>(&format!("return SpellButton1AutoCast:{m}()"))
                .unwrap()
        })
        .collect();
    assert!(
        (geom[0] - 37.0).abs() < 0.01 && (geom[1] - 37.0).abs() < 0.01,
        "shine pane is {geom:?}, expected 37x37 (1393 squares it on the button)"
    );
    assert!(
        (geom[2] - 1.48).abs() < 0.001,
        "shine pane's model scale is {}, expected 1393's 1.48 (the pet button's rim ratio)",
        geom[2]
    );
    let dx = s
        .eval::<f32>("return SpellButton1AutoCast:GetLeft() - SpellButton1:GetLeft()")
        .unwrap();
    let dy = s
        .eval::<f32>("return SpellButton1AutoCast:GetBottom() - SpellButton1:GetBottom()")
        .unwrap();
    assert!(
        dx.abs() < 0.01 && dy.abs() < 0.01,
        "shine pane sits at ({dx}, {dy}) inside the button; 1393 squares it on the button so it \
         is concentric with the brackets — the ref's +1,+1 is what read as a top/right bias"
    );

    // ── The clicks ────────────────────────────────────────────────────────────────────────────
    click(&mut s, "SpellButton1", "LeftButton");
    assert!(s.errors().is_empty(), "click errors: {:?}", s.errors());
    assert_eq!(s.take_pet_spell_casts(), vec![2649]);
    assert!(s.take_spell_casts().is_empty());
    assert!(s.take_pet_spell_autocasts().is_empty());

    // Right: autocast, never a cast (`SpellBookFrame.lua:284-285`).
    click(&mut s, "SpellButton3", "RightButton");
    assert!(
        s.errors().is_empty(),
        "right-click errors: {:?}",
        s.errors()
    );
    assert_eq!(s.take_pet_spell_autocasts(), vec![16827]);
    assert!(
        s.take_pet_spell_casts().is_empty(),
        "a right-click on the pet page must not also cast"
    );

    // ── Two reference quirks, pinned ───────────────────────────────────────────────────────────
    // The pet page ignores its page number: `SpellBook_GetSpellID`'s pet arm is a bare `return id`
    // (`SpellBookFrame.lua:460-461`).
    assert_eq!(s.eval::<i64>("return SpellBook_GetSpellID(1)").unwrap(), 1);
    s.run(r#"SPELLBOOK_PAGENUMBERS["pet"] = 2"#).unwrap();
    assert_eq!(
        s.eval::<i64>("return SpellBook_GetSpellID(1)").unwrap(),
        1,
        "DO NOT FIX: the ref's pet arm has no page term (decision 1032)"
    );
    // …and Next writes `SPELLBOOK_PAGENUMBERS[selectedSkillLine]` on both books
    // (`SpellBookFrame.lua:423`), so the pet page stays put.
    s.run(r#"SPELLBOOK_PAGENUMBERS["pet"] = 1"#).unwrap();
    click(&mut s, "SpellBookNextPageButton", "LeftButton");
    assert!(s.errors().is_empty(), "page errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<i64>(r#"return SPELLBOOK_PAGENUMBERS["pet"]"#)
            .unwrap(),
        1,
        "DO NOT FIX: the ref writes SPELLBOOK_PAGENUMBERS[selectedSkillLine] on both books"
    );
    assert_eq!(
        s.eval::<String>("return SpellButton1SpellName:GetText()")
            .unwrap(),
        "Growl",
        "so the page does not turn"
    );

    // ── The pet leaves: the window closes and reverts (`SpellBookFrame.lua:147-151`) ─────────
    s.set_pet_book(benilla_ui::script::PetBookState::default());
    s.fire_event("SPELLS_CHANGED", vec![]);
    assert!(s.errors().is_empty(), "teardown errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return SpellBookFrame:IsVisible()").unwrap(),
        "a pet book with no pet closes rather than showing an empty page"
    );
    assert!(s
        .eval::<bool>("return SpellBookFrame.bookType == BOOKTYPE_SPELL")
        .unwrap());
}

/// The macro-editor fork (`SpellBookFrame.lua:271-283`, `Blizzard_MacroUI.lua:122-126`): only a
/// shift-click writes, appending a whole `/cast <name>[(<rank>)]` line with no separator. The
/// `/cast` line is 1.12's; the bare-name insert is a later client's.
#[test]
fn the_macro_editor_takes_a_shift_click_and_only_a_shift_click() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        CREATE_MACROS = "Create Macros"
        GENERAL_MACROS = "General Macros"
        CHARACTER_SPECIFIC_MACROS = "%s Specific Macros"
        ENTER_MACRO_LABEL = "Enter Macro Commands:"
        MACROFRAME_CHAR_LIMIT = "%d/255 Characters Used"
        MACRO_POPUP_TEXT = "Enter Macro Name (Max 16 Characters):"
        MACRO_POPUP_CHOOSE_ICON = "Choose an Icon:"
        CHANGE_MACRO_NAME_ICON = "Change Name/Icon"
        DELETE = "Delete" NEW = "New" EXIT = "Exit" CANCEL = "Cancel" OKAY = "Okay"
        MACROS = "Macros"
        TOOLTIP_DEFAULT_COLOR = { r = 1.0, g = 1.0, b = 1.0 }
        TOOLTIP_DEFAULT_BACKGROUND_COLOR = { r = 0.09, g = 0.09, b = 0.19 }
        "#,
    )
    .unwrap();
    // The macro window's character tab formats `UnitName("player")` in its OnLoad.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    for file in [
        r"Interface\FrameXML\GlobalStrings.lua",
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\BasicControls.xml", // `TEXT`
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        // The icon chooser's scroll frame inherits `ClassTrainerListScrollFrameTemplate`.
        r"Interface\FrameXML\ClassTrainerFrameTemplates.xml",
        r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        // `ExhaustionTick_Update` reads `ReputationWatchBar` (`MainMenuBar.lua:52`, `:69`), which
        // `ReputationFrame.xml` declares, after the templates its check boxes inherit.
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        r"Interface\FrameXML\ReputationFrame.xml",
        // The multibar grids the spellbook toggles, whose file wants the options window's uvars.
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "KeyBindingsPage.xml",
        "OptionsFrame.xml",
        "Interface\\FrameXML\\MultiActionBars.xml",
        "Interface\\FrameXML\\SpellBookFrame.xml",
        "SpellBookAdapters.xml",
    ] {
        load_xml(&s, file);
    }
    // A LoadOnDemand addon, seated off the chain and loaded by stock `MacroFrame_LoadUI`
    // (`UIParent.lua:178`), as the app does.
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_MacroUI");
    s.run("MacroFrame_LoadUI()").unwrap();
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);

    // [`book`] plus a passive third Fire spell: id 3, on SpellButton5 in the column-major grid.
    let mut b = book();
    b.tabs[0].num_spells = 3;
    b.tabs[1].offset = 3;
    b.slots.insert(
        2,
        SpellSlotView {
            spell_id: 168,
            name: "Frost Warding".into(),
            texture: Some("Interface\\Icons\\Spell_Frost_FrostWard".into()),
            passive: true,
            ..Default::default()
        },
    );
    s.set_spellbook(b);

    s.run(r#"CreateMacro("Ambush", 1, "")"#).unwrap();
    s.run("ShowMacroFrame()").unwrap();
    // Nothing is selected on open (`MacroFrame_Update` never assigns a selection), so the details
    // pane waits for a click.
    s.run("MacroButton1:Click()").unwrap();
    s.run("ToggleSpellBook(BOOKTYPE_SPELL)").unwrap();
    assert!(s.errors().is_empty(), "open errors: {:?}", s.errors());
    let body =
        |s: &UiScript| -> String { s.eval::<String>("return MacroFrameText:GetText()").unwrap() };
    assert!(
        s.eval::<bool>("return MacroFrameText:IsVisible()").unwrap(),
        "the editor's body box is up — the ref's own `MacroFrame_AddMacroLine` gate"
    );
    assert_eq!(body(&s), "");

    // ── A plain right click still casts and writes nothing ─────────────────────────────────────
    click(&mut s, "SpellButton1", "RightButton");
    assert!(
        s.errors().is_empty(),
        "right-click errors: {:?}",
        s.errors()
    );
    assert_eq!(s.take_spell_casts(), vec![133], "a right-click casts");
    assert_eq!(
        body(&s),
        "",
        "B248: an UNSHIFTED click must never reach the macro editor (ref l.271 tests shift first)"
    );
    assert!(s.cursor_payload().is_none());

    click(&mut s, "SpellButton1", "LeftButton");
    assert_eq!(s.take_spell_casts(), vec![133]);
    assert_eq!(body(&s), "");

    // ── Shift-click appends a whole `/cast` line and never casts ──────────────────────────────
    s.set_modifiers(true, false, false);
    click(&mut s, "SpellButton1", "LeftButton");
    assert!(
        s.errors().is_empty(),
        "shift-click errors: {:?}",
        s.errors()
    );
    assert_eq!(
        body(&s),
        "/cast Fireball(Rank 1)",
        "ref l.276: SLASH_CAST1 + name + the rank in parens"
    );
    assert!(
        s.take_spell_casts().is_empty(),
        "the macro arm replaces the cast, it does not add to it"
    );
    assert!(
        s.cursor_payload().is_none(),
        "with the editor open a shift-click writes instead of picking up"
    );
    // The box's `OnTextChanged` sets the dirty flag, deferred to the next drain.
    s.tick(0.0);
    assert!(
        s.eval::<bool>("return MacroFrame.textChanged == 1")
            .unwrap(),
        "the write goes through the box's own OnTextChanged, so the window is dirty"
    );

    // A second line runs straight on: the reference appends with no separator (`GetText()..line`,
    // `Blizzard_MacroUI.lua:124`).
    click(&mut s, "SpellButton3", "RightButton"); // shift outranks the button
    assert_eq!(
        body(&s),
        "/cast Fireball(Rank 1)/cast Fire Blast(Rank 1)",
        "ref MacroFrame.lua:95 — appended, unseparated"
    );
    assert!(s.take_spell_casts().is_empty());

    // A passive writes nothing and does not pick up: its guard sits inside the macro arm
    // (`SpellBookFrame.lua:274`).
    let before = body(&s);
    click(&mut s, "SpellButton5", "LeftButton");
    assert_eq!(body(&s), before, "a passive is not a castable line");
    assert!(s.cursor_payload().is_none());

    // The name/icon popup hides the body box, which `MacroFrame_AddMacroLine` checks.
    s.run("MacroNewButton_OnClick()").unwrap();
    assert!(!s.eval::<bool>("return MacroFrameText:IsVisible()").unwrap());
    click(&mut s, "SpellButton1", "LeftButton");
    assert!(s.errors().is_empty(), "popup-open errors: {:?}", s.errors());
    assert!(s.cursor_payload().is_none(), "still not a pickup");
    s.run("MacroPopupFrame:Hide()").unwrap();
    s.run("MacroFrame_Update()").unwrap();
    assert_eq!(body(&s), before, "nothing landed while the popup was up");

    // ── Editor hidden: shift-click picks up again (`SpellBookFrame.lua:281-282`) ───────────────
    s.run("HideUIPanel(MacroFrame)").unwrap();
    click(&mut s, "SpellButton1", "LeftButton");
    s.set_modifiers(false, false, false);
    assert!(s.errors().is_empty(), "pickup errors: {:?}", s.errors());
    let (kind, spell_id) = s
        .eval::<(String, i64)>("local k, _, _, id = GetCursorInfo() return k, id")
        .unwrap();
    assert_eq!((kind.as_str(), spell_id), ("spell", 133));
    assert!(s.take_spell_casts().is_empty());
}
