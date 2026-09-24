//! The four optional extra bars (`MultiBars.xml`) + the stance bar (`StanceBar.xml`) driven end to
//! end through the REAL shipped XML — split out of `action_bar_tests.rs` (one file per bar
//! family; self-contained loader per the `bag_tests` precedent).
//!
//! Since decision 1500 all four bars are player options that ship OFF, so most of what is here
//! raises the bar it is about first — through the same two globals the Options rows write.

use benilla_ui::script::{ActionSlot, QuadContent, ScriptValue, SpellTooltipView, UiScript};

use super::test_ui::load_ui as load_xml;

/// Load the shipped `UIParent.xml` (UIParent_ManageFramePositions — the stance bar's
/// OnShow/OnHide calls it, decision 0272; the runtime loads it before every bar) +
/// `Fonts.xml`/`GameTooltip.xml` (the buttons' OnEnter/OnLeave reach GameTooltip, and since the
/// hover-hide law a bar hiding under the cursor fires the hovered button's OnLeave — a harness
/// without the tooltip turns that faithful fire into a nil-index error) +
/// `Cooldown.xml` (CooldownFrame_SetTimer — same before-every-consumer posture) +
/// `ActionBar.xml` (the anchor target + shared globals both new bars need).
fn load_action_bar(s: &UiScript) {
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        // The reference declares the reputation WATCH BAR in `ReputationFrame.xml`, and
        // `ExhaustionTick_Update` reads `ReputationWatchBar:IsShown()` twice — the reference's own
        // coupling of MainMenuBar to that pane. So an action-bar harness loads it, and with it the
        // two template files its check boxes inherit through (1875).
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        r"Interface\FrameXML\ReputationFrame.xml",
        // The stock multibar file's OnLoad writes `UIOptionsFrameCheckButtons`, whose home is the
        // reference's own hidden Interface Options window (2115) — the reference's own load order
        // (UIOptionsFrame.xml l.21 before MultiActionBars.xml l.39), and the manifest's since 1938.
        // That window is also where `ALWAYS_SHOW_MULTIBARS` is declared and where the load arm this
        // file exercises lives.
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        r"Interface\FrameXML\OptionsFrame.lua",
        r"Interface\FrameXML\UIOptionsFrame.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "ScrollTemplates.xml",
        "KeyBindingsPage.xml",
        "OptionsFrame.xml",
    ] {
        load_xml(s, file);
    }
}

/// Raise exactly the bars named, through the globals the Options rows write.
fn show_bars(s: &UiScript, bars: &[u32]) {
    let mut lua = String::new();
    for bar in 1..=4u32 {
        lua.push_str(&format!(
            "SHOW_MULTI_ACTIONBAR_{bar} = {} ",
            if bars.contains(&bar) { "1" } else { "nil" }
        ));
    }
    // …then the manage pass, which the options window runs after a toggle (stock
    // UIOptionsFrame.xml:1192, and our rows the same); stock MultiActionBar_Update does not.
    lua.push_str("MultiActionBar_Update() UIParent_ManageFramePositions()");
    s.run(&lua).unwrap();
}

/// The two bottom multibars (MultiBars.xml) through the REAL shipped XML, RAISED: the fixed
/// page bases (BottomLeft = actions 61..72, BottomRight = 49..60 — ref ActionButton_GetPagedID's
/// parent-name fork), the vanilla anchor chain (BottomLeft's BOTTOMLEFT on ActionButton1's
/// TOPLEFT +17, BottomRight 10 to its right), empty wells HIDDEN except while a payload is held
/// (the ref's own multibar default, unlike the main bar's always-visible wells), a click queuing
/// the multibar id, and the bonus-bar page flip leaving multibar ids untouched.
///
/// The raise is the first thing this does since 1500 — the bars ship off, and everything below is
/// about what a bar looks like once the player has asked for it.
#[test]
fn shipped_multibars_drive_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    let frames = super::test_ui::load_ui(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    assert_eq!(
        frames, 100,
        "what stock MultiActionBars.xml declares (1938): the four bar frames and their 48 buttons, \
         each with a $parentCooldown — the same 100 ours built for the same seats"
    );

    // Occupy main slot 1, BottomLeft slot 1 (action 61), BottomRight slot 1 (action 49).
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Main".into()),
            kind: 0x00,
            action: 100,
            count: 0,
            consumable: false,
        }),
    );
    s.set_action(
        61,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_BL".into()),
            kind: 0x00,
            action: 200,
            count: 0,
            consumable: false,
        }),
    );
    s.set_action(
        49,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_BR".into()),
            kind: 0x00,
            action: 300,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    show_bars(&s, &[1, 2]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    let quads = s.extract();
    let icon = |path: &str| {
        quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path))
            .and_then(|q| q.rect)
    };

    // Geometry: the 1024-wide bar's left edge sits at x=0 on the 1024-wide screen; main button 1
    // spans x[8,44] y[4,40] (the end-to-end test above). BottomLeft's BOTTOMLEFT = button 1's
    // TOPLEFT +(0,17) = (8,57), its button 1 at the frame's BOTTOMLEFT ⇒ x[8,44] y[57,93].
    // BottomRight's LEFT = BottomLeft's (500-wide) RIGHT +(10,0) ⇒ frame left 518, same y band.
    let bl = icon("Interface\\Icons\\Spell_BL").expect("BottomLeft button 1 icon");
    assert_eq!(
        (bl.left, bl.bottom, bl.right, bl.top),
        (8.0, 57.0, 44.0, 93.0)
    );
    let br = icon("Interface\\Icons\\Spell_BR").expect("BottomRight button 1 icon");
    assert_eq!(
        (br.left, br.bottom, br.right, br.top),
        (518.0, 57.0, 554.0, 93.0)
    );

    // Empty multibar wells are HIDDEN (the ref default): rings = 12 main wells (always drawn)
    // + the 2 occupied multibar buttons only.
    let rings = |quads: &[benilla_ui::script::ExtractedQuad], path: &str| {
        quads
            .iter()
            .filter(
                |q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path),
            )
            .count()
    };
    assert_eq!(
        rings(&quads, "Interface\\Buttons\\UI-Quickslot2"),
        3,
        "the occupied main slot + the 2 occupied multibar buttons; every empty well is hidden, \
         the main bar's included (ActionButton.lua:69-70)"
    );

    // A click on BottomLeft button 1 (center (26,75)) queues the FIXED id 61 — and stays 61 when
    // a bonus page is active (the ref's fork: only the main bar re-pages).
    s.mouse_button(26.0, 75.0, "LeftButton", true);
    s.mouse_button(26.0, 75.0, "LeftButton", false);
    assert_eq!(
        s.take_action_uses()
            .into_iter()
            .map(|u| u.action)
            .collect::<Vec<_>>(),
        vec![61]
    );
    s.set_bonus_bar_offset(1);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    s.mouse_button(26.0, 75.0, "LeftButton", true);
    s.mouse_button(26.0, 75.0, "LeftButton", false);
    assert_eq!(
        s.take_action_uses()
            .into_iter()
            .map(|u| u.action)
            .collect::<Vec<_>>(),
        vec![61],
        "a bonus page never re-pages a multibar"
    );
    s.set_bonus_bar_offset(0);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    // The offset edges above showed the bonus overlay (1524) and its descent takes 0.15s of
    // frame time this event-only test never used to pass; without it the 12 overlay wells are
    // still on screen and correctly answer the SHOWGRID below as 12 extra drop targets. The
    // stock slide paints, THEN advances, so the frame that hides it is one OnUpdate past the time.
    s.tick(0.2);
    s.tick(0.01);

    // While a payload is held (SHOWGRID), the hidden empty wells appear as drop-target rings
    // (UI-Quickslot, the "no action" ring): 11 empty main wells swap texture + 22 multibar wells
    // show ⇒ 33; HIDEGRID hides the multibar ones again.
    s.fire_event("ACTIONBAR_SHOWGRID", vec![]);
    s.resolve();
    assert_eq!(
        rings(&s.extract(), "Interface\\Buttons\\UI-Quickslot"),
        33,
        "grid shows every empty well as a drop target"
    );
    s.fire_event("ACTIONBAR_HIDEGRID", vec![]);
    s.resolve();
    assert_eq!(rings(&s.extract(), "Interface\\Buttons\\UI-Quickslot"), 0);
    assert_eq!(
        rings(&s.extract(), "Interface\\Buttons\\UI-Quickslot2"),
        3,
        "letting go hides every empty well again, the main bar's included; the three occupied \
         buttons keep their rings"
    );

    // The unbound multibar hotkey corner carries the ref's RANGE_INDICATOR dot: out of range paints
    // the red dot, back in range clears it (the main bar's labels tint instead). Two stock
    // mechanisms, in order: `ActionButton_UpdateHotkeys` decides whether the corner's TEXT is the
    // dot at all — only while `IsActionInRange` answers non-nil, i.e. with a target — and it runs
    // on PLAYER_TARGET_CHANGED (ActionButton.lua:121-145, 333-334); every `ActionButton_OnUpdate`
    // then shows or hides that text by range (l.389-395) and tints it red past the 0.2 s recheck.
    use benilla_ui::script::ActionState;
    s.set_action_state(
        61,
        Some(ActionState {
            usable: true,
            in_range: Some(false),
            has_range: true,
            ..Default::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.tick(0.5); // past the 0.2 s range recheck
    s.resolve();
    let dot_shown = |quads: &[benilla_ui::script::ExtractedQuad]| {
        quads
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "●"))
    };
    assert!(dot_shown(&s.extract()), "out of range shows the red dot");
    s.set_action_state(
        61,
        Some(ActionState {
            usable: true,
            in_range: Some(true),
            has_range: true,
            ..Default::default()
        }),
    );
    s.tick(0.5);
    s.resolve();
    assert!(!dot_shown(&s.extract()), "back in range clears the dot");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The stance bar (StanceBar.xml) through the REAL shipped XML: hidden at zero forms, sized to
/// the pushed list (buttons past numForms hide), the checked ring on the active form, the 0.4
/// grey on a not-castable one, a click queuing the form's spell id, and an emptied push hiding
/// the whole frame again — the reference's form list (`0xb71100`) and its four bindings driven
/// end to end.
#[test]
fn shipped_stance_bar_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ShapeshiftFormView;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // Cooldown.xml + ActionBar.xml first: StanceBar.xml anchors to MainMenuBar and calls
    // CooldownFrame_SetTimer / BENILLA_FALLBACK_ICON (the runtime load order).
    load_action_bar(&s);
    // MultiBars.xml too, and it is load-bearing for the geometry below: the stance bar's OnShow
    // re-fires UIParent_ManageFramePositions, whose `ShapeshiftBarFrame` row computes the y as
    // baseY 0 + bottomLeft 45 — the same +45 the XML anchor carries statically, but only while a
    // bottom multibar is UP to raise the flag. Since 1500 that is a player option rather than a
    // given, so this test raises it; `the_stance_bar_sits_where_the_pass_puts_it` below owns the
    // other state.
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    show_bars(&s, &[1]);
    // The shapeshift bar is declared by stock BonusActionBarFrame.xml (ShapeshiftBarFrame and
    // ShapeshiftButton1..10 beside the bonus bar), which load_action_bar loaded above (1938).
    assert!(
        s.eval::<bool>("return ShapeshiftBarFrame ~= nil and ShapeshiftButton10 ~= nil")
            .unwrap(),
        "the stance bar's frames come with the stock bonus-bar file"
    );

    // No forms pushed (a mage): the frame is hidden.
    assert!(!s
        .eval::<bool>("return ShapeshiftBarFrame:IsShown()")
        .unwrap());

    // A warrior's two stances: battle active, defensive known but not castable.
    s.set_shapeshift_forms(vec![
        ShapeshiftFormView {
            spell_id: 2457,
            texture: Some("Interface\\Icons\\Stance_A".into()),
            name: "Battle Stance".into(),
            active: true,
            castable: true,
            cooldown: None,
        },
        ShapeshiftFormView {
            spell_id: 71,
            texture: Some("Interface\\Icons\\Stance_B".into()),
            name: "Defensive Stance".into(),
            active: false,
            castable: false,
            cooldown: None,
        },
    ]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    let quads = s.extract();

    // Geometry: the stance frame's BOTTOMLEFT = MainMenuBar (1024×53, screen-bottom
    // centered ⇒ left edge 0) TOPLEFT +(30,45) = (30,98); button 1 at frame BOTTOMLEFT +(11,3),
    // 30×30 ⇒ x[41,71] y[101,131]; button 2 chains +7 ⇒ left 78.
    let icon = |quads: &[benilla_ui::script::ExtractedQuad], path: &str| {
        quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path))
            .and_then(|q| q.rect)
    };
    let a = icon(&quads, "Interface\\Icons\\Stance_A").expect("stance button 1 icon");
    assert_eq!(
        (a.left, a.bottom, a.right, a.top),
        (41.0, 101.0, 71.0, 131.0)
    );
    let b = icon(&quads, "Interface\\Icons\\Stance_B").expect("stance button 2 icon");
    assert_eq!(b.left, 78.0);

    // The active form's checked ring; the not-castable grey on button 2's icon.
    assert!(s
        .eval::<bool>("return ShapeshiftButton1:GetChecked()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return ShapeshiftButton2:GetChecked()")
        .unwrap());
    let grey = quads.iter().find_map(|q| match &q.content {
        QuadContent::Texture {
            path: Some(p),
            color: Some(c),
            ..
        } if p.contains("Stance_B") => Some(*c),
        _ => None,
    });
    assert_eq!(
        grey,
        Some([0.4, 0.4, 0.4, 1.0]),
        "not-castable greys the icon"
    );
    // Buttons past numForms stay hidden: exactly two stance icons drew.
    let stance_icons = quads
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("Stance_"))
        })
        .count();
    assert_eq!(stance_icons, 2);

    // A click on button 2 (center (93,116)) queues the form's SPELL id — cast-vs-cancel is the
    // app drain's call, not the XML's.
    s.mouse_button(93.0, 116.0, "LeftButton", true);
    s.mouse_button(93.0, 116.0, "LeftButton", false);
    assert_eq!(s.take_shapeshift_casts(), vec![71]);

    // The checked ring never follows the click — the OnClick reverts the CheckButton's own
    // pre-OnClick toggle (ref ShapeshiftButtonTemplate), leaving checked to the isActive
    // repaint: button 2 stays unchecked until the server actually shifts us…
    assert!(
        !s.eval::<bool>("return ShapeshiftButton2:GetChecked()")
            .unwrap(),
        "a clicked non-active form must stay unchecked until the form byte confirms"
    );
    assert!(s
        .eval::<bool>("return ShapeshiftButton1:GetChecked()")
        .unwrap());

    // …and clicking the ACTIVE form (the director's warrior bug: Battle Stance must not
    // untoggle) still queues the spell — the app drain decides it is a silent no-op — while the
    // ring stays lit with no repaint needed.
    s.mouse_button(56.0, 116.0, "LeftButton", true);
    s.mouse_button(56.0, 116.0, "LeftButton", false);
    assert_eq!(s.take_shapeshift_casts(), vec![2457]);
    assert!(
        s.eval::<bool>("return ShapeshiftButton1:GetChecked()")
            .unwrap(),
        "clicking the active stance must not untoggle its checked ring"
    );

    // RIGHT-CLICK IS DEAD ON THIS BAR — no flash, no cast — and that is the reference (decisions
    // 1023 + 1030). `ShapeshiftButtonTemplate` inherits `ActionButtonTemplate` and then overrides
    // <OnLoad> with a body that only scales the cooldown, dropping the
    // `RegisterForClicks("LeftButtonUp","RightButtonUp")` that `ActionButton_OnLoad` gives every
    // other action-style button (ref ActionButton.lua:109). The default {LeftButtonUp} stands, and
    // `0x77924b` gates the PushedTexture on that same mask. 1027 diverged and 1030 reverted it, so
    // this asserts the quirk on purpose: the left press below proves the flash works at all here.
    let depressed = |s: &UiScript| {
        s.extract().iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("Quickslot-Depress"))
        })
    };
    s.mouse_move(56.0, 116.0);
    s.mouse_button(56.0, 116.0, "RightButton", true);
    assert!(
        !depressed(&s),
        "a right-press must NOT flash — unregistered"
    );
    s.mouse_button(56.0, 116.0, "RightButton", false);
    assert!(
        s.take_shapeshift_casts().is_empty(),
        "and must not cast — the ref never routes a right-click here at all"
    );
    s.mouse_button(56.0, 116.0, "LeftButton", true);
    assert!(depressed(&s), "the LEFT press does flash — the mask has it");
    s.mouse_button(56.0, 116.0, "LeftButton", false);
    let _ = s.take_shapeshift_casts();
    assert!(
        s.eval::<bool>("return ShapeshiftButton1:GetChecked()")
            .unwrap(),
        "and the active form stays lit through either"
    );

    // An emptied push hides the whole frame (the formless class path, live: shapeshift unlearned).
    s.set_shapeshift_forms(vec![]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    assert!(!s
        .eval::<bool>("return ShapeshiftBarFrame:IsShown()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The hover reads the button's OWN paged action, not the main bar's slot of the same index.
///
/// The director's report (2026-07-21): hovering a multibar spell showed nothing, or the tooltip
/// of the spell *below* it on the main bar. `BenillaActionButton_OnEnter` called
/// `BenillaActionBar_ActionFor(button.index)` — the MAIN bar's page formula — instead of
/// `BenillaActionButton_Action(button)`, which carries the ref's parent-name fork as the
/// per-button `base`. Index 1 on BottomLeft therefore rendered action 1, not 61: the button
/// directly below it. Every other button path already used the fork; only the tooltip reached
/// past it, so nothing else showed the bug.
#[test]
fn multibar_hover_renders_the_buttons_own_action() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        // The reference declares the reputation WATCH BAR in `ReputationFrame.xml`, and
        // `ExhaustionTick_Update` reads `ReputationWatchBar:IsShown()` twice — the reference's own
        // coupling of MainMenuBar to that pane. So an action-bar harness loads it, and with it the
        // two template files its check boxes inherit through (1875).
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        r"Interface\FrameXML\ReputationFrame.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "KeyBindingsPage.xml",
        "OptionsFrame.xml",
        "Interface\\FrameXML\\MultiActionBars.xml",
    ] {
        load_xml(&s, file);
    }

    // Main slot 1 → spell 100, BottomLeft slot 1 (action 61) → 200, BottomRight slot 1 (49) → 300.
    // Main slot 2 is left EMPTY while BottomLeft slot 2 (62) → 400: the "no tooltip at all" half
    // of the report — the wrong id resolved to an empty slot, and SetAction renders nothing.
    for (slot, spell) in [(1u32, 100u32), (61, 200), (49, 300), (62, 400)] {
        s.set_action(
            slot,
            Some(ActionSlot {
                texture: Some(format!("Interface\\Icons\\Spell_{spell}")),
                kind: 0x00,
                action: spell,
                count: 0,
                consumable: false,
            }),
        );
        s.set_spell_tooltip(
            spell,
            SpellTooltipView {
                name: format!("Spell {spell}"),
                description: "does a thing".into(),
                ..Default::default()
            },
        );
    }
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    show_bars(&s, &[1, 2]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let hover_name = |s: &UiScript, button: &str| -> Option<String> {
        s.run(&format!(
            "GameTooltip:Hide() this = {button} ActionButton_SetTooltip()"
        ))
        .unwrap();
        s.eval::<Option<String>>(
            "if not GameTooltip:IsShown() then return nil end return GameTooltipTextLeft1:GetText()",
        )
        .unwrap()
    };

    assert_eq!(
        hover_name(&s, "ActionButton1").as_deref(),
        Some("Spell 100"),
        "the main bar still reads its own slot"
    );
    assert_eq!(
        hover_name(&s, "MultiBarBottomLeftButton1").as_deref(),
        Some("Spell 200"),
        "BottomLeft button 1 is action 61 — not main slot 1 (the spell below it)"
    );
    assert_eq!(
        hover_name(&s, "MultiBarBottomRightButton1").as_deref(),
        Some("Spell 300"),
        "BottomRight button 1 is action 49"
    );
    assert_eq!(
        hover_name(&s, "MultiBarBottomLeftButton2").as_deref(),
        Some("Spell 400"),
        "an occupied multibar slot over an EMPTY main slot still renders (the no-tooltip half)"
    );

    // A bonus page re-pages ONLY the main bar — the multibar hover is untouched by it.
    s.set_bonus_bar_offset(1);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    assert_eq!(
        hover_name(&s, "MultiBarBottomLeftButton1").as_deref(),
        Some("Spell 200"),
        "a bonus page never re-pages a multibar hover"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The two vertical multibars exist and start hidden** — the reference's own posture, and the
/// reason three corpus addons stopped dying at session start.
///
/// `MultiActionBars.xml` l.515/523 instantiates `MultiBarRight`/`MultiBarLeft` as real
/// `parent="UIParent"` frames; their templates (VerticalMultiBar3/4, l.266/381) carry
/// `hidden="true"`. Atlas (`Atlas.lua:387`, `MultiBarLeft:SetFrameStrata`) and Bartender2
/// (`Bartender2.lua:74`, `MultiBarLeft:ClearAllPoints`) only need them to BE there — neither shows
/// one. So all three claims below are the fix: present, hidden, and on the reference's pages.
/// Since 1500 "hidden" is where they START rather than where they stay — see the toggle tests.
#[test]
fn the_vertical_multibars_exist_hidden_on_the_reference_pages() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");

    for bar in ["MultiBarRight", "MultiBarLeft"] {
        assert!(
            s.eval::<bool>(&format!("return {bar} ~= nil")).unwrap(),
            "{bar} must exist — Atlas and Bartender2 index it by name at session start"
        );
        assert!(
            !s.eval::<bool>(&format!("return {bar}:IsShown()")).unwrap(),
            "{bar} must ship HIDDEN, exactly as VerticalMultiBar3/4 do"
        );
        // The two calls the corpus actually makes, verbatim in shape.
        s.run(&format!("{bar}:SetFrameStrata(\"MEDIUM\")")).unwrap();
        s.run(&format!("{bar}:ClearAllPoints()")).unwrap();
    }

    // ref ActionButton.lua:8-9 — RIGHT_ACTIONBAR_PAGE = 3 (actions 25..36), LEFT = 4 (37..48).
    for (bar, first, last) in [("MultiBarRight", 25, 36), ("MultiBarLeft", 37, 48)] {
        assert_eq!(
            s.eval::<i64>(&format!("return ActionButton_GetPagedID({bar}Button1)"))
                .unwrap(),
            first,
            "{bar}'s first slot"
        );
        assert_eq!(
            s.eval::<i64>(&format!("return ActionButton_GetPagedID({bar}Button12)"))
                .unwrap(),
            last,
            "{bar}'s last slot"
        );
    }
}

/// **Every extra bar is down until its own toggle says otherwise, and `MultiBarLeft` needs two.**
///
/// The four bits of `PLAYER_FIELD_BYTES` byte 2 map to bars 1-4 at the FrameXML layer (the binary
/// is bar-agnostic — `SetActionBarToggles 0x4e76e0` packs four unnamed bits), and a fresh
/// character's byte is 0, which is the whole of "off by default": nothing here fakes a default,
/// the bars simply have nothing telling them to show.
///
/// The conjunction on bar 4 is the reference's own (`MultiActionBars.lua` l.73,
/// `if ( SHOW_MULTI_ACTIONBAR_3 and SHOW_MULTI_ACTIONBAR_4 )`) and it is not cosmetic:
/// `MultiBarLeft` anchors its TOPRIGHT to `MultiBarRight`'s TOPLEFT, so alone it would be a column
/// hanging off a bar that is not on screen.
#[test]
fn every_extra_bar_stays_down_until_its_own_toggle_is_set() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");

    const BARS: [&str; 4] = [
        "MultiBarBottomLeft",
        "MultiBarBottomRight",
        "MultiBarRight",
        "MultiBarLeft",
    ];
    let shown =
        |s: &UiScript, bar: &str| s.eval::<bool>(&format!("return {bar}:IsShown()")).unwrap();

    // The zero byte, applied through the same path a login takes.
    s.run("MultiActionBar_Update()").unwrap();
    for bar in BARS {
        assert!(!shown(&s, bar), "{bar} must be down at a zero toggle byte");
    }

    // Each of the first three raises ITS bar and only its bar.
    for (flag, want) in [(1u32, 0usize), (2, 1), (3, 2)] {
        show_bars(&s, &[flag]);
        for (i, bar) in BARS.iter().enumerate() {
            assert_eq!(
                shown(&s, bar),
                i == want,
                "bar {flag} alone: {bar} shown {}, expected {}",
                shown(&s, bar),
                i == want
            );
        }
    }

    // Bar 4 alone does nothing at all — not even to itself.
    show_bars(&s, &[4]);
    for bar in BARS {
        assert!(
            !shown(&s, bar),
            "{bar}: SHOW_MULTI_ACTIONBAR_4 without 3 raises nothing"
        );
    }

    // 3 and 4 together bring up both vertical bars.
    show_bars(&s, &[3, 4]);
    assert!(shown(&s, "MultiBarRight"));
    assert!(shown(&s, "MultiBarLeft"), "MultiBarLeft rides on bar 3");
    assert!(!shown(&s, "MultiBarBottomLeft"));
    assert!(!shown(&s, "MultiBarBottomRight"));

    // All four on, then all four off again — the toggle is a toggle, not a one-way door.
    show_bars(&s, &[1, 2, 3, 4]);
    for bar in BARS {
        assert!(shown(&s, bar), "{bar} up with the full byte");
    }
    show_bars(&s, &[]);
    for bar in BARS {
        assert!(!shown(&s, bar), "{bar} down again");
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Raising a bottom bar moves everything that shares the bottom band with it.**
///
/// `MultiActionBar_Update` ends by running `UIParent_ManageFramePositions`, and that is the whole
/// reason a bar toggle is safe to expose: the cast bar, the chat frames and the bag stack's corner
/// all seat themselves off that pass. Pinned on the two the pass expresses differently — a VAR row
/// (`CONTAINER_OFFSET_Y`, the number the bag stack reads: 70 with the band clear, +27 with either
/// bottom bar up) and a FRAME row (`CastingBarFrame`, baseY 60 +40 on `bottomEither`).
///
/// The regression it locks is the pass never firing at all, which would leave every one of those
/// frames drawing straight through a bar the player just asked for (decision 1499's screenshot).
#[test]
fn raising_a_bottom_bar_moves_the_managed_bottom_stack() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    load_xml(&s, "Interface\\FrameXML\\CastingBarFrame.xml");

    // The pass writes the y of a frame row into its anchor; read it back off the anchor rather
    // than off a resolved rect, so a hidden cast bar answers the same as a visible one.
    let cast_y = |s: &UiScript| {
        s.eval::<f64>("local _, _, _, _, y = CastingBarFrame:GetPoint() return y")
            .unwrap()
    };
    let offset_y = |s: &UiScript| s.eval::<f64>("return CONTAINER_OFFSET_Y").unwrap();

    show_bars(&s, &[]);
    assert_eq!(offset_y(&s), 70.0, "band clear: the row's base");
    let low = cast_y(&s);
    assert_eq!(low, 60.0, "CastingBarFrame's baseY");

    show_bars(&s, &[1]);
    assert_eq!(
        offset_y(&s),
        97.0,
        "bottom-left up: 70 + the row's bottomEither 27"
    );
    assert_eq!(cast_y(&s), 100.0, "the cast bar rises with it (60 + 40)");

    // The RIGHT bottom bar raises the same flag — either one, not both.
    show_bars(&s, &[2]);
    assert_eq!(offset_y(&s), 97.0, "bottomEither is either");
    assert_eq!(cast_y(&s), 100.0);

    // Both up is still one step for these two rows (`bottomEither` is paid once); the bag corner's
    // own `bottomRight` delta is 0, which is why this number does not move again.
    show_bars(&s, &[1, 2]);
    assert_eq!(offset_y(&s), 97.0);

    // And down again — the pass is re-derived, never accumulated.
    show_bars(&s, &[]);
    assert_eq!(offset_y(&s), 70.0);
    assert_eq!(cast_y(&s), low);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **A raised bar takes its page out of the main bar's cycle, and a lowered one gives it back.**
///
/// `ActionBar.xml` now declares all six pages viewable, because all four extra bars ship off; the
/// arithmetic lives in `MultiActionBar_Update`. Before 1500 two rows were blanked at DECLARATION,
/// which was only ever right because visibility was static.
#[test]
fn viewable_action_bar_pages_follow_the_bar_toggles() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");

    let viewable = |s: &UiScript| {
        s.eval::<String>(
            "local out = {} \
             for i = 1, NUM_ACTIONBAR_PAGES do \
               if VIEWABLE_ACTION_BAR_PAGES[i] then table.insert(out, i) end \
             end \
             return table.concat(out, \",\")",
        )
        .unwrap()
    };

    show_bars(&s, &[]);
    assert_eq!(viewable(&s), "1,2,3,4,5,6", "no bar up, no page claimed");

    // ref ActionButton.lua:6-9 — BottomLeft 6, BottomRight 5, Right 3, Left 4.
    show_bars(&s, &[1]);
    assert_eq!(viewable(&s), "1,2,3,4,5", "BottomLeft owns page 6");
    show_bars(&s, &[2]);
    assert_eq!(viewable(&s), "1,2,3,4,6", "BottomRight owns page 5");
    show_bars(&s, &[3]);
    assert_eq!(viewable(&s), "1,2,4,5,6", "MultiBarRight owns page 3");
    show_bars(&s, &[3, 4]);
    assert_eq!(
        viewable(&s),
        "1,2,5,6",
        "…and MultiBarLeft page 4 beside it"
    );

    // Bar 4 without bar 3 shows nothing, so it claims nothing either — the page arithmetic reads
    // the same conjunction the Show/Hide does, not the raw flag.
    show_bars(&s, &[4]);
    assert_eq!(viewable(&s), "1,2,3,4,5,6");

    show_bars(&s, &[1, 2, 3, 4]);
    assert_eq!(viewable(&s), "1,2", "all four up: only the main pages left");
    show_bars(&s, &[]);
    assert_eq!(viewable(&s), "1,2,3,4,5,6", "and every page comes back");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Always Show ActionBars holds every extra bar's empty wells open — and lets go without closing
/// what a held payload is still holding.**
///
/// That last clause is why `button.showgrid` is a COUNT rather than a flag (`ActionBar.xml`'s SCOPE
/// note): the option and the cursor payload are two independent askers, and 0216's "no two shows
/// can ever nest" stopped being true the moment this switch existed.
#[test]
fn the_grid_option_holds_the_extra_bars_empty_wells_open() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    show_bars(&s, &[1]);

    let well = |s: &UiScript| {
        s.eval::<bool>("return MultiBarBottomLeftButton5:IsShown()")
            .unwrap()
    };
    assert!(!well(&s), "an empty multibar well is hidden by default");

    s.run("ALWAYS_SHOW_MULTIBARS = \"1\" MultiActionBar_UpdateGridVisibility()")
        .unwrap();
    assert!(well(&s), "the option opens it with nothing in hand");
    // Every one of the four bars, not just the raised one — the reference's ShowAllGrids names all
    // four, and a bar the player turns on later must already be holding its wells open.
    for bar in ["MultiBarBottomRight", "MultiBarRight", "MultiBarLeft"] {
        assert!(
            s.eval::<bool>(&format!("return {bar}Button5:IsShown()"))
                .unwrap(),
            "{bar}'s wells are held open too"
        );
    }

    // A payload arrives on top of the option, then the option lets go: the well stays open because
    // the payload is still asking for it. A boolean would have closed it here.
    s.fire_event("ACTIONBAR_SHOWGRID", vec![]);
    assert!(well(&s));
    s.run("ALWAYS_SHOW_MULTIBARS = \"0\" MultiActionBar_UpdateGridVisibility()")
        .unwrap();
    assert!(well(&s), "the payload still holds the well open");
    s.fire_event("ACTIONBAR_HIDEGRID", vec![]);
    assert!(!well(&s), "and it closes when the last asker lets go");

    // Re-applying an "off" that is already off must not owe anything: with a payload in hand the
    // wells stay open through it (the reference's own idempotence bug, closed by the latch).
    // The reference's hide is a COUNTED decrement on every well (`ActionButton_HideGrid`), never
    // a no-op: with the option off, one more apply closes a payload-held well. That is why our
    // row runs `MultiActionBar_UpdateGridVisibility` only on a CLICK (stock UIOptionsFrame.xml:683)
    // and its load arm only ever SHOWS (UIOptionsFrame.lua:218-220) — a load-time hide would push
    // counts nobody raised below zero. (Ours used to guard the no-op; the guard went with the
    // file, 1938.)
    // The load arm, both ways: off touches no count, on opens the wells.
    // The LOAD arm is the reference's own VARIABLES_LOADED arm now (`UIOptionsFrame.lua`
    // l.218-220), off the chain since 2115 — not our retired `OptionsFrame_ApplySavedSettings`.
    s.run("ALWAYS_SHOW_MULTIBARS = \"0\"").unwrap();
    s.fire_event("VARIABLES_LOADED", vec![]);
    s.fire_event("ACTIONBAR_SHOWGRID", vec![]);
    assert!(well(&s));
    s.fire_event("ACTIONBAR_HIDEGRID", vec![]);
    assert!(!well(&s), "a load with the option off left the count alone");
    s.run("ALWAYS_SHOW_MULTIBARS = \"1\"").unwrap();
    s.fire_event("VARIABLES_LOADED", vec![]);
    assert!(well(&s), "a load with the option on opens the wells");
    s.run("ALWAYS_SHOW_MULTIBARS = \"0\" MultiActionBar_UpdateGridVisibility()")
        .unwrap();
    assert!(!well(&s));

    // …and the click arm against a held payload: one apply closes the well the payload opened.
    // (The payload's own HIDEGRID that follows takes the count to −1 in the reference too — its
    // quirk, not modelled here.)
    s.fire_event("ACTIONBAR_SHOWGRID", vec![]);
    assert!(well(&s));
    s.run("MultiActionBar_UpdateGridVisibility()").unwrap();
    assert!(
        !well(&s),
        "the option's hide counts against the payload's open"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **A held payload GHOSTS the wells it opens — the reference's `SetVertexColor(1, 1, 1, 0.5)` on
/// a shown grid ring (`ActionButton.lua:247`), which ours skipped from 0216 to 1781.**
///
/// `UI-Quickslot` is not a second ring. Measured off the shipped BLPs: `UI-Quickslot2`'s interior
/// is fully transparent (alpha 0) — a hollow border — while `UI-Quickslot`'s is solid black at
/// alpha 0.6 across its whole middle. So the grid swap does not *outline* an empty well, it
/// *fills* it, and at full alpha picking up a spell dropped a 60%-black square into every empty
/// slot on every raised bar at once. That is the director's report. Half alpha takes the well to
/// 30% black, where it reads as a drop target instead of a hole.
///
/// The occupied rings stay opaque, and letting go of the payload restores the wells — both
/// DIVERGENCES, chosen in 1782. In stock 1.12.1 the dim is fanned to every button and then never
/// cleared by anything: `SetNormalTexture(path)` reuses the region and writes only its texture
/// handle, and a 3-argument `SetVertexColor` is `SetVertexColor(r, g, b, currentAlpha)` — so the
/// first spell anyone picks up leaves every border at alpha 0x80 for the session (the reuse leg
/// `0x778f10`, the alpha-keeping `SetVertexColor` `0x79abd0`). That is a stuck state keyed to
/// nothing the player can see. We take the look the dim was written for and let go of it with the
/// payload, which is what the last two assertions here pin.
#[test]
fn a_held_payload_ghosts_the_empty_wells_it_opens() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_SteelMelee".into()),
            kind: 0x00,
            action: 100,
            count: 0,
            consumable: false,
        }),
    );
    // The raise comes AFTER the seed: `UIParent.xml`'s PLAYER_ENTERING_WORLD arm reads the
    // server's bar-toggle byte once and would put an early raise straight back down.
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    show_bars(&s, &[1]);
    s.resolve();

    /// The drawn alpha of every ring quad wearing `path` — `None` colour is untinted white.
    fn ring_alphas(s: &UiScript, path: &str) -> Vec<f32> {
        s.extract()
            .iter()
            .filter_map(|q| match &q.content {
                QuadContent::Texture {
                    path: Some(p),
                    color,
                    ..
                } if p == path => Some(color.map_or(1.0, |c| c[3])),
                _ => None,
            })
            .collect()
    }
    let quickslot = "Interface\\Buttons\\UI-Quickslot";
    let quickslot2 = "Interface\\Buttons\\UI-Quickslot2";

    // Nothing in hand: the main bar's 12 always-visible wells wear the hollow ring, all opaque.
    assert_eq!(ring_alphas(&s, quickslot), Vec::<f32>::new());
    let resting = ring_alphas(&s, quickslot2);
    assert_eq!(
        resting.len(),
        1,
        "only the occupied main slot draws a ring: every empty well, main bar included, is \
         hidden while nothing is held (ActionButton.lua:69-70)"
    );
    assert!(
        resting.iter().all(|a| *a == 1.0),
        "a resting ring is opaque: {resting:?}"
    );

    // Payload up. Every well the grid opened is the FILLED plate at HALF alpha.
    s.fire_event("ACTIONBAR_SHOWGRID", vec![]);
    s.resolve();
    let ghosts = ring_alphas(&s, quickslot);
    assert_eq!(
        ghosts.len(),
        23,
        "11 empty main wells swap art + MultiBarBottomLeft's 12 appear"
    );
    assert!(
        ghosts.iter().all(|a| *a == 0.5),
        "every grid ring is the ref's half-alpha ghost, not an opaque plate: {ghosts:?}"
    );

    // …and the one OCCUPIED button ghosts with it: `ActionButton_ShowGrid` sets the half alpha
    // (ActionButton.lua:246) before anything asks whether the slot holds an action, and
    // `HideGrid` touches no colour at all. What restores an occupied ring is its next
    // `ActionButton_UpdateUsable` (l.271-283), whose `SetVertexColor(1, 1, 1)` carries the default
    // alpha — from Update, and on SPELL_UPDATE_USABLE. Ours skipped occupied rings; the stock
    // file does not (1938).
    let occupied = ring_alphas(&s, quickslot2);
    assert_eq!(occupied, vec![0.5], "the occupied ring ghosts too");

    // A well that was ghosted and is then FILLED comes back opaque. This is the assertion that
    // outlives the engine's own `SetVertexColor` divergence (1782): the day an absent alpha stops
    // meaning 1.0 and starts meaning "whatever is already there", a `SetRing` that let the fourth
    // argument default would carry the 0.5 straight into the occupied ring, and only this catches
    // it — the HIDEGRID check below would not, because that path re-derives an EMPTY well.
    s.set_action(
        2,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_Rogue_Ambush".into()),
            kind: 0x00,
            action: 101,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("ACTIONBAR_SLOT_CHANGED", vec![ScriptValue::Number(2.0)]);
    s.resolve();
    assert_eq!(
        ring_alphas(&s, quickslot2),
        vec![0.5, 1.0],
        "the well that got an action is opaque again (its Update ran UpdateUsable); the other \
         occupied ring keeps its ghost, payload still held"
    );

    // Letting go takes the empty wells away; the colour it leaves alone.
    s.fire_event("ACTIONBAR_HIDEGRID", vec![]);
    s.resolve();
    assert_eq!(ring_alphas(&s, quickslot), Vec::<f32>::new());
    let after = ring_alphas(&s, quickslot2);
    assert_eq!(
        after,
        vec![0.5, 1.0],
        "HideGrid touches no colour: a ring the grid ghosted stays dim past the payload — the \
         reference's own rule (ours used to clear it here)"
    );
    // What happens to that ring NEXT is decision 1782's open question, deliberately not asserted:
    // the reference's `SetVertexColor(1, 1, 1)` in `ActionButton_UpdateUsable` keeps the alpha
    // already on the region (`SetVertexColor` `0x79abd0` — the "stuck dim" a 1.12 player sees
    // after a drag), while this engine's three-argument call still resets it to 1.0 until the
    // director calls 1782. A test that pinned either outcome would be enshrining a divergence as
    // fidelity.
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The byte the row sends is the four globals, and the seed brings that byte back.**
///
/// The whole round trip, over the REAL bindings (`benilla_ui::script::action_bar_toggles`): the
/// row's setter writes a global, re-derives the bars and posts `CMSG_SET_ACTIONBAR_TOGGLES` with
/// the WHOLE byte — bits `0x01/0x02/0x04/0x08` (`SetActionBarToggles 0x4e76e0`). Coming back,
/// the server's descriptor push is the only thing that moves the getter, and `UIParent.xml`'s
/// `PLAYER_ENTERING_WORLD` arm reads it exactly once as the seed.
///
/// Every `Set` is one packet, deliberately (the binding gates nothing), which is why the drain is a
/// list and each step below checks the packet it just caused.
#[test]
fn a_bar_toggle_sends_the_byte_its_globals_pack_to() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    let _ = s.take_action_bar_toggle_sends();

    // The row's own setter, with the row's own "1"/"0" strings.
    s.run("SHOW_MULTI_ACTIONBAR_1 = 1 MultiActionBar_Update() SetActionBarToggles(SHOW_MULTI_ACTIONBAR_1, SHOW_MULTI_ACTIONBAR_2, SHOW_MULTI_ACTIONBAR_3, SHOW_MULTI_ACTIONBAR_4)")
        .unwrap();
    assert_eq!(s.take_action_bar_toggle_sends(), vec![0x01]);
    s.run("SHOW_MULTI_ACTIONBAR_3 = 1 MultiActionBar_Update() SetActionBarToggles(SHOW_MULTI_ACTIONBAR_1, SHOW_MULTI_ACTIONBAR_2, SHOW_MULTI_ACTIONBAR_3, SHOW_MULTI_ACTIONBAR_4)")
        .unwrap();
    assert_eq!(
        s.take_action_bar_toggle_sends(),
        vec![0x05],
        "the WHOLE byte re-sent, not a delta"
    );
    s.run("SHOW_MULTI_ACTIONBAR_4 = 1 MultiActionBar_Update() SetActionBarToggles(SHOW_MULTI_ACTIONBAR_1, SHOW_MULTI_ACTIONBAR_2, SHOW_MULTI_ACTIONBAR_3, SHOW_MULTI_ACTIONBAR_4)")
        .unwrap();
    assert_eq!(s.take_action_bar_toggle_sends(), vec![0x0d]);
    s.run("SHOW_MULTI_ACTIONBAR_1 = nil MultiActionBar_Update() SetActionBarToggles(SHOW_MULTI_ACTIONBAR_1, SHOW_MULTI_ACTIONBAR_2, SHOW_MULTI_ACTIONBAR_3, SHOW_MULTI_ACTIONBAR_4)")
        .unwrap();
    assert_eq!(
        s.take_action_bar_toggle_sends(),
        vec![0x0c],
        "the row turns its \"0\" into nil before the send, and nil is off"
    );

    // What is stored is 1 or nil and nothing else — GetActionBarToggles' own shape, and what the
    // corpus reads back (Bartender2 writes 1s; CT_BarMod tests `not SHOW_MULTI_ACTIONBAR_n`).
    assert_eq!(
        s.eval::<String>(
            "return type(SHOW_MULTI_ACTIONBAR_3) .. \"/\" .. type(SHOW_MULTI_ACTIONBAR_1)"
        )
        .unwrap(),
        "number/nil"
    );

    // The way back in. Wipe the live state, push the byte the server would hold, and run the seed
    // `UIParent.xml`'s PLAYER_ENTERING_WORLD arm runs — nothing else re-reads that field, so this
    // one read is the whole restore.
    show_bars(&s, &[]);
    let _ = s.take_action_bar_toggle_sends();
    s.set_action_bar_toggles(0x0c);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    assert!(
        s.eval::<bool>("return MultiBarRight:IsShown() and MultiBarLeft:IsShown()")
            .unwrap(),
        "the seed brings back exactly the byte that was sent"
    );
    assert!(!s
        .eval::<bool>("return MultiBarBottomLeft:IsShown()")
        .unwrap());
    assert!(
        s.take_action_bar_toggle_sends().is_empty(),
        "the seed READS — a login must not post the byte back at the server"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The shipped setter passes exactly FOUR arguments** — the one claim the real binding cannot be
/// asked, which is why this is the one place that shadows it.
///
/// `SetActionBarToggles` loops `i = 0..3` (`0x4e770e cmp esi,4`) and never fetches a fifth, so a
/// call with five is *behaviourally* identical to one with four — the difference is invisible from
/// outside and only a spy on the Lua side can see it. The reference's own panel does pass five
/// (`UIOptionsFrame_Save` l.363, with `ALWAYS_SHOW_MULTIBARS` on the end) and that fifth is the
/// reason its grid option looks server-backed and is not. Ours passes four and this holds it there,
/// so the day someone "restores fidelity" by appending the fifth, they are told what it costs.
///
/// A later reader: the shadowing here is deliberate and is NOT a leftover shim. Everything else in
/// this file drives the real binding.
#[test]
fn the_shipped_setter_passes_exactly_four_arguments() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");

    // 5.0's `arg.n`, not `select("#", ...)`: `...` as a value is not in this VM's grammar, because
    // it is not in the 1.12 client's.
    s.run(
        r#"
        BENILLA_TEST_TOGGLE_ARGC = nil
        function SetActionBarToggles(...)
            BENILLA_TEST_TOGGLE_ARGC = arg.n
        end
        "#,
    )
    .unwrap();
    // The shipped setter is the Action Bars row's own closure (OptionsFrame.xml), which is
    // stock's three lines — assign the global, MultiActionBar_Update(), SetActionBarToggles(…) —
    // plus the manage pass.
    s.run("BenillaOptionsFrameContainerBodyActionBarsRowMultiBar2Check:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<i64>("return BENILLA_TEST_TOGGLE_ARGC").unwrap(),
        4,
        "four — never the reference's five, which the binding drops on the floor"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The stance bar's seat is the manage pass's, in BOTH bottom-bar states.**
///
/// `StanceBar.xml`'s XML anchor carries the RAISED value (`MainMenuBar` TOPLEFT +(30,45)) so the
/// frame is somewhere sane before the first pass; the truth is the `ShapeshiftBarFrame` row
/// (baseY 0, bottomLeft 45). That distinction did not matter while the bottom bars were always on
/// — the two agreed by construction — and 1500 makes the unraised state reachable, so it is pinned
/// here rather than assumed.
#[test]
fn the_stance_bar_sits_where_the_pass_puts_it() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ShapeshiftFormView;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    s.set_shapeshift_forms(vec![ShapeshiftFormView {
        spell_id: 2457,
        texture: Some("Interface\\Icons\\Stance_A".into()),
        name: "Battle Stance".into(),
        active: true,
        castable: true,
        cooldown: None,
    }]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);

    let seat = |s: &UiScript| {
        s.eval::<f64>("local _, _, _, _, y = ShapeshiftBarFrame:GetPoint() return y")
            .unwrap()
    };

    show_bars(&s, &[]);
    assert_eq!(
        seat(&s),
        0.0,
        "no bottom bar: the row's baseY, on the main bar"
    );
    show_bars(&s, &[1]);
    assert_eq!(seat(&s), 45.0, "bottom-left up: +45, the ref's raised seat");
    show_bars(&s, &[2]);
    assert_eq!(
        seat(&s),
        0.0,
        "the RIGHT bottom bar is not under it — the row's flag is bottomLeft, not bottomEither"
    );
    show_bars(&s, &[]);
    assert_eq!(seat(&s), 0.0);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The stance bar's SHELF ART follows the same pass as its seat** (ref
/// `ShapeshiftBar_UpdatePosition`, BonusActionBarFrame.lua l.229-251).
///
/// Sitting on the main bar, the shelf IS the bar's border: the two end caps show, the middle strip
/// shows only past two forms, and each button's ring grows to 64. Raised a row over the bottom-left
/// multibar the strips would draw across whatever is underneath, so all three hide and the rings
/// drop to 50.
///
/// This was dead code until 1500. With the bottom bars always on only the raised branch was
/// reachable; now the unraised bar is what every form class sees on a fresh character, which makes
/// the missing border the DEFAULT look rather than an edge case.
///
/// The last block is the point of the whole test: one `MultiActionBar_Update()` flips the seat AND
/// the art, which is what proves the art rides the managed-position pass rather than having been
/// set once at load.
#[test]
fn the_stance_shelf_follows_the_bottom_left_bar() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ShapeshiftFormView;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");

    let form = |id: u32| ShapeshiftFormView {
        spell_id: id,
        texture: Some(format!("Interface\\Icons\\Stance_{id}")),
        name: format!("Form {id}"),
        active: false,
        castable: true,
        cooldown: None,
    };
    let shown = |s: &UiScript, region: &str| {
        s.eval::<bool>(&format!("return {region}:IsShown()"))
            .unwrap()
    };
    let ring = |s: &UiScript| {
        s.eval::<f64>("return ShapeshiftButton1NormalTexture:GetWidth()")
            .unwrap()
    };

    // A three-form warrior, no bottom bar: the whole shelf, and the big ring.
    s.set_shapeshift_forms(vec![form(2457), form(71), form(2458)]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    show_bars(&s, &[]);
    assert!(shown(&s, "ShapeshiftBarLeft"), "the left end cap");
    assert!(shown(&s, "ShapeshiftBarRight"), "the right end cap");
    assert!(
        shown(&s, "ShapeshiftBarMiddle"),
        "3 forms: the middle strip"
    );
    assert_eq!(ring(&s), 64.0, "unraised rings are the ref's 64");

    // Two forms drop the middle strip and nothing else — the ref's own `> 2`.
    s.set_shapeshift_forms(vec![form(2457), form(71)]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    assert!(shown(&s, "ShapeshiftBarLeft"));
    assert!(shown(&s, "ShapeshiftBarRight"));
    assert!(
        !shown(&s, "ShapeshiftBarMiddle"),
        "exactly 2 forms: end caps only"
    );
    assert_eq!(ring(&s), 64.0);

    // Raising the bottom-left bar flips the SEAT and the ART in the one pass — the listener seam.
    show_bars(&s, &[1]);
    assert_eq!(
        s.eval::<f64>("local _, _, _, _, y = ShapeshiftBarFrame:GetPoint() return y")
            .unwrap(),
        45.0,
        "the seat rose"
    );
    for region in [
        "ShapeshiftBarLeft",
        "ShapeshiftBarMiddle",
        "ShapeshiftBarRight",
    ] {
        assert!(
            !shown(&s, region),
            "{region} must not draw across the row below"
        );
    }
    assert_eq!(ring(&s), 50.0, "raised rings are the ref's 50");

    // Even at three forms, raised keeps the middle strip down — the fork is on the BAR, not the
    // form count; the count only decides the middle strip within the unraised branch.
    // A third form learned while the bar is up: the reference's `ShapeshiftBar_Update` shows the
    // middle strip for three forms regardless of the bar (BonusActionBarFrame.lua:198-206), and
    // nothing runs the pass on that event — the strip draws across the row until something
    // moves. The pass, not Update, owns the raised look (UIParent.lua:1705-1720).
    s.set_shapeshift_forms(vec![form(2457), form(71), form(2458)]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    assert!(
        shown(&s, "ShapeshiftBarMiddle"),
        "Update shows the strip; only the pass takes it down"
    );
    s.run("UIParent_ManageFramePositions()").unwrap();
    assert!(!shown(&s, "ShapeshiftBarMiddle"));
    assert_eq!(ring(&s), 50.0);

    // …and back down again, in one call.
    show_bars(&s, &[]);
    assert!(shown(&s, "ShapeshiftBarMiddle"));
    assert_eq!(ring(&s), 64.0);
    assert_eq!(
        s.eval::<f64>("local _, _, _, _, y = ShapeshiftBarFrame:GetPoint() return y")
            .unwrap(),
        0.0
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The shelf's LENGTH is the form count** — the half of the ref's `ShapeshiftBar_Update`
/// (l.161-175) that decides where the right end cap lands.
///
/// The three strips are DECLARED as the >2-form chain (Left → Middle → Right), and hiding the
/// middle strip does not close its 38px hole: a hidden region still resolves its rect, here and in
/// the reference. So a one-form class — a rogue, whose only "form" is Stealth — drew its Stealth
/// button with the 42x50 end cap stranded a slot's width to its right: a second, permanently empty
/// recess beside the icon (director, 2026-08-22). The shipped test beside this one only ever
/// exercised 2 and 3 forms and only ever read `IsShown()`, which is exactly why it shipped.
#[test]
fn the_stance_shelf_is_as_long_as_the_form_count() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ShapeshiftFormView;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    show_bars(&s, &[]); // unraised: the shelf art is the state under test

    let form = |id: u32| ShapeshiftFormView {
        spell_id: id,
        texture: Some(format!("Interface\\Icons\\Stance_{id}")),
        name: format!("Form {id}"),
        active: false,
        castable: true,
        cooldown: None,
    };
    let set_forms = |s: &mut UiScript, n: u32| {
        s.set_shapeshift_forms((0..n).map(|i| form(2450 + i)).collect());
        s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
        s.resolve();
    };
    // The cap's left edge relative to the LEFT cap's — the whole shelf in one number, independent
    // of where the managed pass seats the bar.
    let cap = |s: &UiScript| {
        s.eval::<f64>("return ShapeshiftBarRight:GetLeft() - ShapeshiftBarLeft:GetLeft()")
            .unwrap()
    };
    // Button 1 sits at +11 and is 30 wide, so its own span is 11..41 in the same space.
    const BUTTON1_RIGHT: f64 = 41.0;

    // ONE form (the rogue): the cap is pulled back INSIDE the left cap (+12) — a 54px shelf that
    // wraps the single button and stops there. The regression put it at 83.
    set_forms(&mut s, 1);
    assert_eq!(
        cap(&s),
        12.0,
        "one form: the cap sits 12px into the left cap"
    );
    assert!(
        cap(&s) + 42.0 > BUTTON1_RIGHT,
        "the one-form shelf still covers its button"
    );
    assert!(
        !s.eval::<bool>("return ShapeshiftBarMiddle:IsShown()")
            .unwrap(),
        "one form: no middle strip"
    );

    // TWO forms: the caps butt together, 45 + 42 = 87px of shelf, still no middle strip.
    set_forms(&mut s, 2);
    assert_eq!(cap(&s), 45.0, "two forms: cap on the left cap's RIGHT edge");

    // PAST two, the middle strip carries one 38px slot per extra form and the cap chains off it.
    set_forms(&mut s, 3);
    assert_eq!(
        s.eval::<f64>("return ShapeshiftBarMiddle:GetWidth()")
            .unwrap(),
        38.0,
        "3 forms: one slot of middle"
    );
    assert_eq!(cap(&s), 83.0);

    set_forms(&mut s, 5);
    assert_eq!(
        s.eval::<f64>("return ShapeshiftBarMiddle:GetWidth()")
            .unwrap(),
        114.0,
        "5 forms: three slots of middle — the width the ref computes, not the declared 38"
    );
    assert_eq!(cap(&s), 159.0);
    // Button 5 spans 159..189 (11 + 4*37), so the cap must clear it — at the declared width it
    // landed at 83 and drew straight across buttons 3, 4 and 5.
    assert!(
        cap(&s) >= 11.0 + 4.0 * 37.0,
        "the cap clears the last button"
    );

    // …and back down to one form re-points it, so the shelf is not a one-way ratchet.
    set_forms(&mut s, 1);
    assert_eq!(cap(&s), 12.0);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **B348 — an extra bar's EMPTY well keeps the label of the key bound to it.**
///
/// The reported symptom is a flicker between two states of the same bar: the occupied slot always
/// wore its `Q`, while the bound-but-empty slots beside it showed `E`/`R`/`C` in one screenshot
/// and nothing in the next. The cause is a single line left behind by 0270's multibars, which
/// carried NO binding commands at all and so had no label to lose: `BenillaActionButton_Update`'s
/// empty arm cleared the HotKey corner outright. 1008 gave those wells real
/// `MULTIACTIONBAR1/2BUTTONn` bindings and that clear became a wipe of a live label — repainted by
/// the next `UPDATE_BINDINGS` and wiped again by the next thing that repaints a button (a world
/// enter, a payload picked up over the bars, the slot next door changing), which is exactly
/// "sometimes yes, sometimes not".
///
/// The reference never touches the text in `ActionButton_Update` (`ActionButton.lua:167-172` sets
/// only the vertex colour); `ActionButton_UpdateHotkeys` is the sole writer, and its bound leg —
/// `hotkey:SetText(GetBindingText(GetBindingKey(action), "KEY_", 1))` — runs whether or not the
/// slot has an action.
#[test]
fn an_extra_bars_empty_well_keeps_its_bound_hotkey_label() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The real command set, so MULTIACTIONBAR1BUTTONn is bindable at all (it ships unbound —
    // the player binds it, as the reporter had).
    s.register_bindings(&crate::bindings::registry_commands());
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    show_bars(&s, &[1]);
    s.set_action(
        61,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_BL".into()),
            kind: 0x00,
            action: 200,
            count: 0,
            consumable: false,
        }),
    );
    // The reporter's shape: an occupied first well and two empty ones beside it, all three bound,
    // and the wells held open so the empty ones are on screen at all.
    s.run(
        r#"SetBinding("Q", "MULTIACTIONBAR1BUTTON1")
           SetBinding("E", "MULTIACTIONBAR1BUTTON2")
           SetBinding("R", "MULTIACTIONBAR1BUTTON3")
           ALWAYS_SHOW_MULTIBARS = "1" MultiActionBar_UpdateGridVisibility()"#,
    )
    .unwrap();
    s.fire_event("UPDATE_BINDINGS", vec![]);

    let label = |s: &UiScript, button: &str| {
        s.eval::<Option<String>>(&format!("return {button}HotKey:GetText()"))
            .unwrap()
            .unwrap_or_default()
    };
    let drawn = |s: &mut UiScript, want: &str| {
        s.resolve();
        s.extract()
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == want))
    };
    assert_eq!(label(&s, "MultiBarBottomLeftButton1"), "Q");
    assert_eq!(label(&s, "MultiBarBottomLeftButton2"), "E");
    assert_eq!(label(&s, "MultiBarBottomLeftButton3"), "R");
    assert!(drawn(&mut s, "E"), "the empty well's label is on screen");

    // Anything that repaints the buttons. Each of these reached the empty arm and wiped the label
    // before the fix; the occupied well never did, which is why its Q survived every time.
    for (event, what) in [
        ("PLAYER_ENTERING_WORLD", "a world enter"),
        ("ACTIONBAR_SHOWGRID", "a payload picked up over the bars"),
        ("ACTIONBAR_HIDEGRID", "and put down again"),
    ] {
        s.fire_event(event, vec![]);
        assert_eq!(label(&s, "MultiBarBottomLeftButton1"), "Q", "{what}");
        assert_eq!(
            label(&s, "MultiBarBottomLeftButton2"),
            "E",
            "the empty well keeps its bound label through {what}"
        );
        assert_eq!(label(&s, "MultiBarBottomLeftButton3"), "R", "{what}");
    }
    // The world enter above also re-seeds the four bar toggles from the server byte, which the
    // harness does not send — so bar 1 went back down with it. Raise it again: the label is still
    // on the well, not merely still in the FontString.
    show_bars(&s, &[1]);
    assert!(drawn(&mut s, "E"), "…and it is still on screen");

    // The control: an UNBOUND empty well stays blank throughout — the clear this line was written
    // for in 0270 is still done, by the repaint that knows there is no binding.
    assert_eq!(label(&s, "MultiBarBottomLeftButton4"), "");
    // And unbinding one takes its label away, rather than freezing the last text painted.
    s.run(r#"SetBinding("E", nil)"#).unwrap();
    s.fire_event("UPDATE_BINDINGS", vec![]);
    assert_eq!(label(&s, "MultiBarBottomLeftButton2"), "");
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    assert_eq!(label(&s, "MultiBarBottomLeftButton2"), "");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Decision 2000 — the shelf's middle strip tiles along its LENGTH only.**
///
/// Past two forms `ShapeshiftBar_Update` maps the middle strip `SetTexCoord(0, n-2, 0, 1)`
/// (BonusActionBarFrame.lua l.198-206): one slot of art per extra form, repeated along u — the
/// reference's tiling idiom — while v spans exactly the texture. The renderer serves such a strip
/// from a repeat-sampled image, and until 2000 that image wrapped BOTH axes: bilinear filtering
/// at the strip's top edge weighed in the texture's LAST row (`ShapeshiftBarMiddle.blp` row 31 —
/// opaque grey; rows 0-7 transparent), so every four-form bar wore a one-device-px grey hairline
/// along the top of its middle piece, over the world: 0.81-0.84× the world's luma over the strip
/// against 1.00 over the clamp-sampled end caps, in a live four-form shot.
///
/// The seam this pins is the ask itself: the wrap the shelf's three pieces request of the
/// renderer, derived from the UV mapping the VM extracts — u alone for the middle strip past two
/// forms, nothing at three forms (`SetTexCoord(0, 1, 0, 1)` is the whole texture), and never for
/// the end caps (atlas crops of `ShapeshiftBarEnds`).
#[test]
fn the_middle_strip_tiles_along_its_length_only() {
    benilla_formats::wow_data_or_skip!();
    use super::extract::tiling_axes;
    use crate::ui_pass::UvRect;
    use benilla_ui::script::{ShapeshiftFormView, TexCoords};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    show_bars(&s, &[]); // bars down: the shelf is the state under test

    let form = |id: u32| ShapeshiftFormView {
        spell_id: id,
        texture: Some(format!("Interface\\Icons\\Stance_{id}")),
        name: format!("Form {id}"),
        active: id == 2457,
        castable: true,
        cooldown: None,
    };
    // Every drawn piece whose texture path ends in `piece`, as the wrap it asks the renderer for.
    let wraps_of = |s: &UiScript, piece: &str| -> Vec<(bool, bool)> {
        s.extract()
            .into_iter()
            .filter_map(|q| match q.content {
                QuadContent::Texture {
                    path: Some(p),
                    tex_coords,
                    ..
                } if p.ends_with(piece) => Some(tiling_axes(&match tex_coords {
                    Some(TexCoords::Rect(e)) => UvRect::from_tex_coords(e),
                    Some(TexCoords::Corners(c)) => UvRect::from_corners(c),
                    None => UvRect::FULL,
                })),
                _ => None,
            })
            .collect()
    };

    // Four forms (a druid; a GM-learned warrior): the middle strip carries two slots and wraps
    // along u ONLY. Both end caps stay clamped on both axes.
    s.set_shapeshift_forms(vec![form(2457), form(71), form(768), form(2458)]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    s.resolve();
    assert_eq!(
        wraps_of(&s, "ShapeshiftBarMiddle"),
        vec![(true, false)],
        "four forms: the strip tiles along its length and clamps across it"
    );
    assert_eq!(
        wraps_of(&s, "ShapeshiftBarEnds"),
        vec![(false, false), (false, false)],
        "the end caps are atlas crops and never tile"
    );

    // Three forms (the plain warrior): one slot, the whole texture — nothing tiles.
    s.set_shapeshift_forms(vec![form(2457), form(71), form(2458)]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    s.resolve();
    assert_eq!(wraps_of(&s, "ShapeshiftBarMiddle"), vec![(false, false)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The director's picture (2026-09-05)**: a three-stance warrior with the
/// bottom-left bar up, and a silver plate around Defensive Stance — `ShapeshiftBarMiddle` drawn
/// alone, both end caps down.
///
/// The reference fires `UPDATE_SHAPESHIFT_FORMS` for exactly one thing: the form LIST changed
/// (learn / unlearn / rank — the fires at `0x4b28ff`/`0x4b2e43`).
/// Our feed fired it for any change in the pushed view — a stance switch, a castable flip, the
/// shared 1 s category cooldown arming and then EXPIRING. Every fire runs the stock
/// `ShapeshiftBar_Update` (BonusActionBarFrame.lua l.170): `ShapeshiftBarMiddle:Show()`
/// unconditionally past two forms, then `ShapeshiftBarFrame:Show()` on a frame already shown —
/// no OnShow, so no manage pass, and the strip stays up over the raised bar (UIParent.lua
/// l.1706-1712 is the only thing that takes it down). 2000 and 2001 each fixed a real thing in
/// this band and neither touched this, which is the director's "no change from before".
///
/// Driven through the feed's own diff ([`crate::ui_shapeshift::push_forms`]), so what fails here
/// is the feed's rule, not a hand-fired event. A form's STATE rides the reference's state events,
/// which the feeds owning those transitions fire (`PLAYER_AURAS_CHANGED` for the form aura's
/// slot, `SPELL_UPDATE_COOLDOWN` for the store's generation edge) — checked below by firing the
/// one the switch really carries and reading the checked ring.
#[test]
fn a_forms_state_change_leaves_the_shelf_down_over_the_raised_bar() {
    benilla_formats::wow_data_or_skip!();
    use crate::ui_shapeshift::{push_forms, FormsEdge, StanceMemory};
    use benilla_ui::script::ShapeshiftFormView;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    // A recorder for the two events a push may announce.
    s.run(
        "STANCE_LOG = {} local f = CreateFrame('Frame') \
         f:RegisterEvent('UPDATE_SHAPESHIFT_FORMS') f:RegisterEvent('SPELL_UPDATE_USABLE') \
         f:SetScript('OnEvent', function() table.insert(STANCE_LOG, event) end)",
    )
    .unwrap();
    let log = |s: &UiScript| {
        s.eval::<String>("local l = table.concat(STANCE_LOG, ',') STANCE_LOG = {} return l")
            .unwrap()
    };
    let shown = |s: &UiScript, region: &str| {
        s.eval::<bool>(&format!("return {region}:IsShown()"))
            .unwrap()
    };
    let shelf = [
        "ShapeshiftBarLeft",
        "ShapeshiftBarMiddle",
        "ShapeshiftBarRight",
    ];
    let ring = |s: &UiScript| {
        s.eval::<f64>("return ShapeshiftButton1NormalTexture:GetWidth()")
            .unwrap()
    };
    let checked = |s: &UiScript, i: u32| {
        s.eval::<bool>(&format!("return ShapeshiftButton{i}:GetChecked() == 1"))
            .unwrap()
    };
    let stance = |id: u32, active: bool, castable: bool, cooldown| ShapeshiftFormView {
        spell_id: id,
        texture: Some(format!("Interface\\Icons\\Stance_{id}")),
        name: format!("Stance {id}"),
        active,
        castable,
        cooldown,
    };
    let mut memory = StanceMemory::default();

    // Login: the list arrives — the reference's learn edge — with Battle Stance active.
    assert_eq!(
        push_forms(
            &mut s,
            &mut memory,
            vec![
                stance(2457, true, true, None),
                stance(71, false, true, None),
                stance(2458, false, true, None),
            ],
        ),
        FormsEdge::List
    );
    assert_eq!(log(&s), "UPDATE_SHAPESHIFT_FORMS");
    assert!(checked(&s, 1) && !checked(&s, 2));
    // The bottom-left bar up, the way the Options row raises it: the pass seats the stance bar a
    // row higher and takes the whole shelf down (UIParent.lua l.1706-1716).
    show_bars(&s, &[1]);
    for region in shelf {
        assert!(!shown(&s, region), "{region} is down under a raised bar");
    }
    assert_eq!(ring(&s), 50.0);

    // A stance switch: the form byte flips to Defensive and category 47 arms for a second on all
    // three. State, not list — the reference announces it through the aura and cooldown events.
    let cd = Some((0, 1000, true));
    let edge = push_forms(
        &mut s,
        &mut memory,
        vec![
            stance(2457, false, true, cd),
            stance(71, true, true, cd),
            stance(2458, false, true, cd),
        ],
    );
    for region in shelf {
        assert!(
            !shown(&s, region),
            "{region}: the director's plate — the shelf re-shown over the raised bar"
        );
    }
    assert_eq!(ring(&s), 50.0);
    assert_eq!(edge, FormsEdge::Silent);
    assert_eq!(log(&s), "", "a state move fires no list edge");
    // …and the checked ring follows on the event the switch really carries.
    assert!(
        checked(&s, 1) && !checked(&s, 2),
        "no repaint before the state event"
    );
    s.fire_event("PLAYER_AURAS_CHANGED", vec![]);
    assert!(
        !checked(&s, 1) && checked(&s, 2),
        "the aura event repaints the ring"
    );
    for region in shelf {
        assert!(
            !shown(&s, region),
            "{region}: a state repaint never touches the shelf"
        );
    }

    // The cooldown running out is the same silent edge — the widget hides itself, the reference's
    // store fires nothing at expiry, and neither do we.
    assert_eq!(
        push_forms(
            &mut s,
            &mut memory,
            vec![
                stance(2457, false, true, None),
                stance(71, true, true, None),
                stance(2458, false, true, None),
            ],
        ),
        FormsEdge::Silent
    );
    assert_eq!(log(&s), "");
    assert!(!shown(&s, "ShapeshiftBarMiddle"));

    // A castable flip is a usability move — the reference's own SPELL_UPDATE_USABLE — and the
    // stock bar greys the icon on it without touching the shelf.
    assert_eq!(
        push_forms(
            &mut s,
            &mut memory,
            vec![
                stance(2457, false, false, None),
                stance(71, true, true, None),
                stance(2458, false, true, None),
            ],
        ),
        FormsEdge::Usable
    );
    assert_eq!(log(&s), "SPELL_UPDATE_USABLE");
    let grey = s
        .eval::<f64>("local r = ShapeshiftButton1Icon:GetVertexColor() return r")
        .unwrap();
    assert!(
        (grey - 0.4).abs() < 1e-6,
        "not castable: the 0.4 grey, got {grey}"
    );
    assert!(!shown(&s, "ShapeshiftBarMiddle"));

    // Nothing moved: nothing pushed, nothing fired.
    assert_eq!(
        push_forms(
            &mut s,
            &mut memory,
            vec![
                stance(2457, false, false, None),
                stance(71, true, true, None),
                stance(2458, false, true, None),
            ],
        ),
        FormsEdge::Unchanged
    );
    assert_eq!(log(&s), "");

    // The control — a fourth stance LEARNED is the list edge, and the reference's own Update
    // shows the strip across the raised bar until the next pass (the sibling test pins that).
    assert_eq!(
        push_forms(
            &mut s,
            &mut memory,
            vec![
                stance(2457, false, false, None),
                stance(71, true, true, None),
                stance(2458, false, true, None),
                stance(768, false, true, None),
            ],
        ),
        FormsEdge::List
    );
    assert_eq!(log(&s), "UPDATE_SHAPESHIFT_FORMS");
    assert!(
        shown(&s, "ShapeshiftBarMiddle"),
        "the learn edge: Update shows the strip"
    );
    s.run("UIParent_ManageFramePositions()").unwrap();
    assert!(!shown(&s, "ShapeshiftBarMiddle"));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
