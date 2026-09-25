use benilla_ui::script::{ActionSlot, QuadContent, ScriptValue, UiScript};

/// The queued `UseAction` ids, without the self-cast flag `take_action_uses` carries beside them.
fn action_ids(s: &mut UiScript) -> Vec<u32> {
    s.take_action_uses().into_iter().map(|u| u.action).collect()
}

/// The stock main bar without Bevy: a stance's bonus page, its icons, a click and a key.
#[test]
fn shipped_action_bar_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Cooldown.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIParent.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\MoneyFrame.lua");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\MoneyFrame.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\GameTooltip.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    let frames = super::test_ui::load_ui(&s, "Interface\\FrameXML\\MainMenuBar.xml")
        + super::test_ui::load_ui(&s, "Interface\\FrameXML\\ActionBarFrame.xml")
        + super::test_ui::load_ui(&s, "Interface\\FrameXML\\BonusActionBarFrame.xml");
    assert_eq!(
        frames, 80,
        "what the three stock files declare (1938): MainMenuBar.xml's 8 — the bar, the XP StatusBar, \
         the overlay frame, the max-level rail, the art frame, the performance bar and its button, \
         the exhaustion tick; ActionBarFrame.xml's 14 — 12 ActionButtons and the 2 page arrows; \
         BonusActionBarFrame.xml's 24 — the bonus frame with its 12 buttons and the shapeshift frame \
         with its 10; plus one $parentCooldown per action, bonus and shapeshift button (12 + 12 + 10). \
         Ours built 59 for the same seats: no shapeshift bar in the file (StanceBar.xml's), no \
         overlay frame, no performance-bar button"
    );

    // `ExhaustionTick_Update` reads `ReputationWatchBar`, which ReputationFrame.xml declares.
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\OptionsFrameTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\ReputationFrame.xml");

    // A warrior in Battle Stance: bonus offset 1, actions 73..84.
    s.set_bonus_bar_offset(1);
    s.set_action(
        73,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_SteelMelee".into()),
            kind: 0x00,
            action: 100,
            count: 0,
            consumable: false,
        }),
    );
    s.set_action(
        74,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_Rogue_Ambush".into()),
            kind: 0x00,
            action: 101,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    // Fired on the offset's edge; the bonus frame's slide takes two OnUpdates: start, then land.
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    s.tick(10.0);
    s.tick(0.2);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The stance page is painted by `BonusActionButton1/2`. The 1024-wide bar's left edge is 0;
    // the 43-high bonus frame's TOPLEFT lands at the bar's BOTTOMLEFT + (4, 43), and button 1 at
    // (5, 4), 36x36: x[9,45] y[4,40], stride 36 + 6 (BonusActionBarFrame.xml:54-105).
    s.resolve();
    let quads = s.extract();
    let icon = |path: &str| {
        quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path))
            .and_then(|q| q.rect)
    };
    let r = icon("Interface\\Icons\\Ability_SteelMelee").expect("bonus button 1 icon");
    assert_eq!((r.left, r.bottom, r.right, r.top), (9.0, 4.0, 45.0, 40.0));
    let r2 = icon("Interface\\Icons\\Ability_Rogue_Ambush").expect("bonus button 2 icon");
    assert_eq!(r2.left, 9.0 + 42.0); // bonus button 2 left = 51
    let rings = quads
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("UI-Quickslot2"))
        })
        .count();
    assert_eq!(
        rings, 2,
        "the two occupied bonus buttons; every empty main and bonus slot is hidden"
    );
    let icons = quads
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("Icons"))
        })
        .count();
    assert_eq!(icons, 2, "empty slots draw no icon quad");

    // A click at (26, 22) lands on `BonusActionButton1` and queues its paged id, 73.
    s.mouse_button(26.0, 22.0, "LeftButton", true);
    s.mouse_button(26.0, 22.0, "LeftButton", false);
    assert_eq!(action_ids(&mut s), vec![73]);

    // `ACTIONBUTTONn` runs `ActionButtonDown/Up(n)` on the key's two edges (Bindings.xml:121-127).
    s.run("ActionButtonUp(2)").unwrap();
    assert!(
        s.take_action_uses().is_empty(),
        "an Up without a Down is a no-op (the PUSHED gate)"
    );
    let depressed = |s: &UiScript| {
        s.extract()
            .iter()
            .filter(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                        if p.contains("UI-Quickslot-Depress"))
            })
            .count()
    };
    s.run("ActionButtonDown(2)").unwrap();
    assert_eq!(depressed(&s), 1, "key DOWN shows the pushed texture");
    assert_eq!(
        s.eval::<String>("return BonusActionButton2:GetButtonState()")
            .unwrap(),
        "PUSHED",
        "with the overlay up a key drives the BONUS button (ActionButton.lua:15-22)"
    );
    s.run("ActionButtonUp(2)").unwrap();
    assert_eq!(action_ids(&mut s), vec![74], "key '2' fires action 74");
    assert_eq!(depressed(&s), 0, "key UP restores the normal state");

    s.set_bonus_bar_offset(0);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    // The overlay descends and hides one OnUpdate past the slide time.
    s.tick(0.2);
    s.tick(0.01);
    s.resolve();
    let icons_after = s
        .extract()
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("Icons"))
        })
        .count();
    assert_eq!(icons_after, 0, "re-page to an empty page clears the icons");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

fn load_action_bar(s: &UiScript) {
    super::test_ui::load_ui(s, "Interface\\FrameXML\\Cooldown.xml");
    super::test_ui::load_ui(s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    super::test_ui::load_ui(s, "Interface\\FrameXML\\TextStatusBar.lua");
    super::test_ui::load_ui(s, "Interface\\FrameXML\\TextStatusBar.xml");
    super::test_ui::load_ui(s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(s, r"Interface\FrameXML\UIParent.xml");
    super::test_ui::load_ui(s, "Interface\\FrameXML\\GlobalStrings.lua");
    super::test_ui::load_ui(s, "Interface\\FrameXML\\MainMenuBar.xml");
    super::test_ui::load_ui(s, r"Interface\FrameXML\MoneyFrame.lua");
    super::test_ui::load_ui(s, r"Interface\FrameXML\MoneyFrame.xml");
    super::test_ui::load_ui(s, "Interface\\FrameXML\\GameTooltip.xml");
    super::test_ui::load_ui(s, "Interface\\FrameXML\\ActionBarFrame.xml");
    super::test_ui::load_ui(s, "Interface\\FrameXML\\BonusActionBarFrame.xml");

    // `ExhaustionTick_Update` reads `ReputationWatchBar`, which ReputationFrame.xml declares.
    super::test_ui::load_ui(s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(s, r"Interface\FrameXML\UIPanelTemplates.lua");
    super::test_ui::load_ui(s, r"Interface\FrameXML\UIPanelTemplates.xml");
    super::test_ui::load_ui(s, r"Interface\FrameXML\OptionsFrameTemplates.xml");
    super::test_ui::load_ui(s, r"Interface\FrameXML\ReputationFrame.xml");
    // StaticPopup.xml: the keybindings page adds its two confirm dialogs to `StaticPopupDialogs`.
    super::test_ui::load_ui(s, r"Interface\FrameXML\BasicControls.xml"); // `TEXT`
    super::test_ui::load_ui(s, r"Interface\FrameXML\LocaleProperties.lua"); // `GetText`
    super::test_ui::load_ui(s, r"Interface\FrameXML\StaticPopup.xml");
    super::test_ui::load_ui(s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    // `UIOptionsFrame_Init` declares `LOCK_ACTIONBAR` and `ALWAYS_SHOW_MULTIBARS`
    // (UIOptionsFrame.lua:93-107), before our window, whose rows take their defaults from them.
    super::test_ui::load_ui(s, r"Interface\FrameXML\OptionsFrame.lua");
    super::test_ui::load_ui(s, r"Interface\FrameXML\UIOptionsFrame.xml");
    super::test_ui::load_ui(s, "ScrollTemplates.xml");
    super::test_ui::load_ui(s, "KeyBindingsPage.xml");
    super::test_ui::load_ui(s, "OptionsFrame.xml");
}

/// The state events through the stock bar: the cooldown, the checked ring and the usable tint.
#[test]
fn state_feedback_drives_cooldown_checked_and_usable_through_the_xml() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            kind: 0x00,
            action: 133,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.tick(10.0); // a nonzero GetTime epoch

    // A 10 s cooldown with 6 s left: the pane shows mid-sweep.
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            cooldown: Some((6_000, 10_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    // `CooldownFrame_SetTimer` arms sequence 0; the next paint's `OnUpdateModel` scrubs it.
    super::test_ui::cooldown_facts(&mut s);
    s.tick(0.0);
    s.resolve();
    let play = super::test_ui::cooldown_play(&s, "ActionButton1Cooldown")
        .expect("the button's cooldown pane is showing, sequence 0 armed");
    assert_eq!(
        play,
        (0, 400),
        "6 s of 10 s left ⇒ the sweep sits at 40 %: sequence 0 at 400 ms"
    );

    // The checked ring on the current action (`ActionButton_UpdateState`).
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            current: true,
            cooldown: Some((6_000, 10_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_STATE", vec![]);
    assert!(s.eval::<bool>("return ActionButton1:GetChecked()").unwrap());

    // Out of power (not usable, not enough mana): the blue tint.
    s.set_action_state(
        1,
        Some(ActionState {
            usable: false,
            not_enough_mana: true,
            cooldown: Some((6_000, 10_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
    s.resolve();
    let icon_color = s.extract().into_iter().find_map(|q| match &q.content {
        QuadContent::Texture {
            path: Some(p),
            color,
            ..
        } if p.contains("Spell_Fire_FlameBolt") => Some(*color),
        _ => None,
    });
    let c = icon_color.expect("icon quad").expect("vertex color set");
    assert_eq!(
        (c[0], c[1], c[2]),
        (0.5, 0.5, 1.0),
        "the ref's out-of-power blue-grey"
    );

    // Unusable for another reason (food in combat): the `else` arm greys the icon to 0.4 and
    // leaves the ring at full strength (ActionButton.lua:279-281).
    s.set_action_state(
        1,
        Some(ActionState {
            usable: false,
            not_enough_mana: false,
            cooldown: Some((6_000, 10_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
    s.resolve();
    let c = s
        .extract()
        .into_iter()
        .find_map(|q| match &q.content {
            QuadContent::Texture {
                path: Some(p),
                color,
                ..
            } if p.contains("Spell_Fire_FlameBolt") => Some(*color),
            _ => None,
        })
        .expect("icon quad")
        .expect("vertex color set");
    assert_eq!(
        (c[0], c[1], c[2]),
        (0.4, 0.4, 0.4),
        "the ref's unusable grey — the icon the director sees on food in combat"
    );

    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `CooldownFrame_SetTimer` hides the pane unless `start > 0 and duration > 0 and enable > 0`
/// (Cooldown.lua:3): a running cooldown whose start predates a restarted `GetTime` never draws.
#[test]
fn a_start_behind_the_clocks_epoch_hides_the_stock_sweep() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            kind: 0x00,
            action: 133,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.tick(10.0); // GetTime is 10: a clock that restarted ten seconds ago

    // A 10-minute cooldown armed 30 s ago, converted against that restarted clock: start = −20 s.
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            cooldown: Some((-20_000, 600_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    super::test_ui::cooldown_facts(&mut s);
    s.tick(0.0);
    s.resolve();
    assert_eq!(
        super::test_ui::cooldown_play(&s, "ActionButton1Cooldown"),
        None,
        "the stock `start > 0` guard hides the pane outright — 9.5 minutes still to run and the \
         button shows nothing"
    );

    // The same cooldown on a clock that never restarted: GetTime 100, armed at 70.
    s.tick(90.0);
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            cooldown: Some((70_000, 600_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    super::test_ui::cooldown_facts(&mut s);
    s.tick(0.0);
    s.resolve();
    assert_eq!(
        super::test_ui::cooldown_play(&s, "ActionButton1Cooldown"),
        Some((0, 50)),
        "30 s of 600 s elapsed ⇒ sequence 0 scrubbed to 5 %: 50 ms of the 1000 ms sweep"
    );

    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The cooldown child sits at its button's level + 1 and frame level outranks draw layer, so the
/// sweep paints over the button's icon and ring.
#[test]
fn the_cooldown_sweep_paints_over_the_buttons_icon_and_ring() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            kind: 0x00,
            action: 133,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.tick(10.0);
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            cooldown: Some((6_000, 10_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    s.resolve();

    let quads = s.extract();
    let pos = |pred: &dyn Fn(&QuadContent) -> bool| quads.iter().position(|q| pred(&q.content));
    let icon = pos(&|c| {
        matches!(c, QuadContent::Texture { path: Some(p), .. } if p.contains("Spell_Fire_FlameBolt"))
    })
    .expect("the icon texture quad");
    let ring = pos(
        &|c| matches!(c, QuadContent::Texture { path: Some(p), .. } if p.contains("UI-Quickslot2")),
    )
    .expect("the NormalTexture ring quad");
    let sweep = pos(&|c| {
        matches!(c, QuadContent::ModelPane { model: Some(m), .. }
            if m.eq_ignore_ascii_case(super::test_ui::COOLDOWN_MODEL))
    })
    .expect("the sweep pane's quad");
    assert!(
        icon < sweep,
        "the sweep (index {sweep}) must paint over the icon (index {icon})"
    );
    assert!(
        ring < sweep,
        "the sweep (index {sweep}) must paint over the button ring (index {ring})"
    );
}

/// A cooldown-count addon's wrap of `CooldownFrame_SetTimer` leaves the sweep as it was.
#[test]
fn a_cooldown_count_addons_hook_leaves_the_sweep_running() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    // Addons load after FrameXML, so the hook captures the stock global.
    s.run(COOLDOWN_COUNT_HOOK)
        .expect("the addon's hook installs");

    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            kind: 0x00,
            action: 133,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.tick(10.0);
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            cooldown: Some((6_000, 10_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    super::test_ui::cooldown_facts(&mut s);
    s.tick(0.0);
    s.resolve();

    assert!(
        s.eval::<i64>("return seen").unwrap() > 0,
        "the bar must reach the addon's replacement, not a captured original"
    );
    assert!(
        s.eval::<bool>("return ActionButton1Cooldown.textFrame ~= nil")
            .unwrap(),
        "the addon hangs its countdown off the cooldown widget — a field the widget must accept"
    );
    assert_eq!(
        super::test_ui::cooldown_play(&s, "ActionButton1Cooldown"),
        Some((0, 400)),
        "the wrap calls through, so the pane is on the paint list with sequence 0 at 40 %"
    );
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    s.tick(0.0);
    s.resolve();
    assert_eq!(
        super::test_ui::cooldown_play(&s, "ActionButton1Cooldown"),
        None,
        "an elapsed cooldown hides the pane through the wrap exactly as it does without it"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `!OmniCC` 6.8.30's hook in shape: it wraps `CooldownFrame_SetTimer` and hangs its countdown
/// off the button at `button + 2`, one level over the cooldown, kept in `cd.textFrame`.
const COOLDOWN_COUNT_HOOK: &str = r#"
    local original = CooldownFrame_SetTimer
    seen = 0
    CooldownFrame_SetTimer = function(cd, start, duration, enable)
        seen = seen + 1
        original(cd, start, duration, enable)
        if start > 0 and duration > 3 and enable > 0 then
            local count = cd.textFrame
            if not count then
                local icon = getglobal(cd:GetParent():GetName() .. "Icon")
                if icon then
                    count = CreateFrame("Frame", nil, cd:GetParent())
                    count:SetAllPoints(cd:GetParent())
                    count:SetFrameLevel(count:GetFrameLevel() + 1)
                    count.text = count:CreateFontString(nil, "OVERLAY")
                    count.text:SetFontObject(GameFontNormal)
                    count.text:SetPoint("CENTER", count, "CENTER", 0, 1)
                    count.icon = icon
                    count:SetScript("OnUpdate", function() end)
                    cd.textFrame = count
                end
            end
            if count then
                count.start = start
                count.duration = duration
                count:Show()
            end
        elseif cd.textFrame then
            cd.textFrame:Hide()
        end
    end
"#;

/// `BonusActionButtonTemplate`'s OnLoad raises the button by 2, then its cooldown by 2
/// (BonusActionBarFrame.xml:13-15); a script level change carries no children (`0x774560`), so
/// the cooldown sits one level over its button, under the countdown.
#[test]
fn a_cooldown_count_draws_over_the_bonus_bars_sweep() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    s.run(COOLDOWN_COUNT_HOOK)
        .expect("the addon's hook installs");
    let level = |s: &UiScript, frame: &str| {
        s.eval::<i64>(&format!("return {frame}:GetFrameLevel()"))
            .unwrap()
    };
    assert_eq!(
        level(&s, "BonusActionButton1Cooldown"),
        level(&s, "BonusActionButton1") + 1,
        "the template's two hand raises leave the sweep ONE level over its button"
    );

    // Bonus page 1 (a warrior's Battle Stance): button 1 is action 73.
    s.set_action(
        73,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_Racial_BloodRage".into()),
            kind: 0x00,
            action: 2687,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.run("BonusActionBarFrame:Show()").unwrap();
    s.tick(10.0);
    s.set_action_state(
        73,
        Some(ActionState {
            usable: true,
            cooldown: Some((6_000, 60_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    super::test_ui::cooldown_facts(&mut s);
    // The real addon writes its digits from OnUpdate; this shape's OnUpdate is empty.
    s.run(r#"BonusActionButton1Cooldown.textFrame.text:SetText("27")"#)
        .expect("the hook hung its countdown off the bonus button");
    s.tick(0.0);
    s.resolve();

    let quads = s.extract();
    let sweep = quads
        .iter()
        .position(|q| {
            matches!(q.content, QuadContent::ModelPane { .. })
                && s.quad_owner_name(q.target).as_deref() == Some("BonusActionButton1Cooldown")
        })
        .expect("the bonus button's sweep is on the paint list");
    let count = quads
        .iter()
        .position(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "27"))
        .expect("the countdown is on the paint list");
    assert!(
        sweep < count,
        "the countdown (index {count}) must paint over the bonus button's sweep (index {sweep})"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `ActionButton_OnLoad` registers left and right clicks (ActionButton.lua:109) over a button's
/// default of left only, and the OnClick reads no `arg1`: a right-click uses the action too.
#[test]
fn a_right_click_on_an_action_button_uses_the_action() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);

    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Food_11".into()),
            kind: 0x80, // an ITEM action: food
            action: 4540,
            count: 5,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    s.mouse_button(26.0, 22.0, "RightButton", true);
    s.mouse_button(26.0, 22.0, "RightButton", false);
    assert_eq!(
        action_ids(&mut s),
        vec![1],
        "right-click queues the same UseAction a left-click does"
    );

    // The middle button is not registered, so the click gate is still in force.
    s.mouse_button(26.0, 22.0, "MiddleButton", true);
    s.mouse_button(26.0, 22.0, "MiddleButton", false);
    assert!(
        s.take_action_uses().is_empty(),
        "an unregistered button still reaches nothing"
    );

    // Shift+right-click picks up like shift+left: the OnClick fork ignores the button.
    s.set_modifiers(true, false, false);
    s.mouse_button(26.0, 22.0, "RightButton", true);
    s.mouse_button(26.0, 22.0, "RightButton", false);
    s.set_modifiers(false, false, false);
    assert!(s.take_action_uses().is_empty());
    assert!(
        s.cursor_payload().is_some(),
        "shift+right-click carries the action, like shift+left"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Shift-click runs `PickupAction`, a plain click `UseAction` (ActionBarFrame.xml:12-22).
#[test]
fn shift_click_picks_up_not_uses() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);

    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_A".into()),
            kind: 0x00,
            action: 111,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    s.set_modifiers(true, false, false); // IsShiftKeyDown() true
    s.mouse_button(26.0, 22.0, "LeftButton", true);
    s.mouse_button(26.0, 22.0, "LeftButton", false);
    s.set_modifiers(false, false, false);

    assert!(
        s.take_action_uses().is_empty(),
        "shift-click PICKS UP, never queues a use"
    );
    assert!(s.cursor_payload().is_some(), "action 1 is on the cursor");
    assert!(
        !s.eval::<bool>("return HasAction(1)").unwrap(),
        "slot cleared"
    );
    assert_eq!(
        s.take_action_sets(),
        vec![(1, 0)],
        "picking up queues the clear-the-slot send"
    );

    // A plain click while holding places (`UseAction(id, 1)`, ActionBarFrame.xml:19).
    s.mouse_button(26.0, 22.0, "LeftButton", true);
    s.mouse_button(26.0, 22.0, "LeftButton", false);
    assert!(s.take_action_uses().is_empty(), "routed to place, not use");
    assert!(s.cursor_payload().is_none(), "empty destination clears");
    assert!(s.eval::<bool>("return HasAction(1)").unwrap());
    assert_eq!(s.take_action_sets(), vec![(1, 111)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `LOCK_ACTIONBAR` guards both drag ends (ActionBarFrame.xml:23-38) but not the shift-click
/// pickup in OnClick (l.12-22), so a locked bar still yields to shift-click.
#[test]
fn the_action_bar_lock_stops_the_drag_and_leaves_shift_click_alone() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    assert_eq!(
        s.eval::<String>("return LOCK_ACTIONBAR").unwrap(),
        "0",
        "the bar ships unlocked, the reference's own default"
    );

    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_A".into()),
            kind: 0x00,
            action: 111,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    s.run(r#"LOCK_ACTIONBAR = "1""#).unwrap();
    s.run("this = ActionButton1 ActionButton1:GetScript(\"OnDragStart\")()")
        .unwrap();
    assert!(
        s.cursor_payload().is_none(),
        "a locked bar does not give the action up to a drag"
    );
    assert!(
        s.eval::<bool>("return HasAction(1)").unwrap(),
        "slot intact"
    );
    assert!(
        s.take_action_sets().is_empty(),
        "and nothing is sent to the server"
    );

    s.set_modifiers(true, false, false);
    s.mouse_button(26.0, 22.0, "LeftButton", true);
    s.mouse_button(26.0, 22.0, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(
        s.cursor_payload().is_some(),
        "shift-click is not what the lock stops"
    );

    s.run("this = ActionButton1 ActionButton1:GetScript(\"OnReceiveDrag\")()")
        .unwrap();
    assert!(
        s.cursor_payload().is_some(),
        "a locked slot refuses the drop"
    );
    s.run(r#"LOCK_ACTIONBAR = "0""#).unwrap();
    s.run("this = ActionButton1 ActionButton1:GetScript(\"OnReceiveDrag\")()")
        .unwrap();
    assert!(s.cursor_payload().is_none(), "unlocked, the drop lands");
    assert_eq!(
        s.take_action_sets(),
        vec![(1, 0), (1, 111)],
        "the shift-pickup's clear and the unlocked drop's set — and nothing from the two \
         refused gestures between them"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A drag onto an occupied button puts the displaced action on the cursor, in two sends.
#[test]
fn drag_drop_onto_another_button_hops_the_displaced_action() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);

    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_A".into()),
            kind: 0x00,
            action: 111,
            count: 0,
            consumable: false,
        }),
    );
    s.set_action(
        2,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_B".into()),
            kind: 0x00,
            action: 222,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    s.mouse_button(26.0, 22.0, "LeftButton", true);
    s.mouse_move(40.0, 22.0); // past the 4px drag-start threshold
    let consumed = s.mouse_button(68.0, 22.0, "LeftButton", false);
    assert!(consumed, "OnReceiveDrag consumed the release");

    assert!(
        !s.eval::<bool>("return HasAction(1)").unwrap(),
        "slot 1 emptied"
    );
    assert!(s.eval::<bool>("return HasAction(2)").unwrap());
    assert_eq!(
        s.eval::<String>("return GetActionTexture(2)").unwrap(),
        "Interface\\Icons\\Spell_A",
        "slot 2 now shows the placed action"
    );
    let (kind, src) = s
        .eval::<(String, i64)>("local k, slot = GetCursorInfo() return k, slot")
        .unwrap();
    assert_eq!(
        (kind.as_str(), src),
        ("action", 2),
        "the displaced action hopped on, sourced from slot 2"
    );

    // Two independent sends across the one gesture: the pickup's clear, then the place's write.
    assert_eq!(s.take_action_sets(), vec![(1, 0), (2, 111)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The count shows only for `IsConsumableAction` (ActionButton.lua:285-292), false for a mount's
/// item: zero charges, `InventoryType` 0 (`0x4e5250`). The flag rides the slot, not the state
/// map: `ACTIONBAR_SLOT_CHANGED` repaints before the state feed writes.
#[test]
fn count_fontstring_follows_is_consumable_action_not_the_bag_count() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);

    // Multi-digit counts: a single digit could match a button's one-character HotKey label.
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_A".into()),
            kind: 0x00, // SPELL
            action: 111,
            count: 42, // the app never sets this for a spell; the XML must ignore it
            consumable: false,
        }),
    );
    s.set_action(
        2,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            kind: 0x80, // ITEM: a stack of food
            action: 117,
            count: 15,
            consumable: true,
        }),
    );
    // A mount: an ITEM held eleven times that is not consumable.
    s.set_action(
        3,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_Mount_Undeadhorse".into()),
            kind: 0x80,
            action: 13332,
            count: 11,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    // Read the fontstrings by name: a one-character count reads like a HotKey label.
    let count_of = |s: &UiScript, n: u32| {
        s.eval::<String>(&format!("return ActionButton{n}Count:GetText() or \"\""))
            .unwrap()
    };
    assert_eq!(
        count_of(&s, 1),
        "",
        "SPELL kind never shows a count, however GetActionCount answers"
    );
    assert_eq!(
        count_of(&s, 2),
        "15",
        "a consumable ITEM shows its bag count"
    );
    assert_eq!(
        count_of(&s, 3),
        "",
        "B201: a NON-consumable ITEM (a mount) shows no stack number, whatever the count says"
    );
    assert!(
        s.extract()
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "15")),
        "the consumable's count reaches the screen"
    );

    // A spent consumable reads "0": no count test guards the `SetText` (ActionButton.lua:288).
    s.set_action(
        2,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            kind: 0x80,
            action: 117,
            count: 0,
            consumable: true,
        }),
    );
    s.fire_event("ACTIONBAR_SLOT_CHANGED", vec![ScriptValue::Int(2)]);
    s.resolve();
    assert_eq!(
        count_of(&s, 2),
        "0",
        "a spent consumable reads 0, it does not go blank"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `ActionButton_Update` writes `GetActionText` into the `$parentName` line unconditionally
/// (ActionButton.lua:236-238): a macro slot shows its name, a spell slot or an emptied one none.
#[test]
fn macro_name_line_follows_get_action_text_through_the_xml() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::{MacroState, MacroView};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    s.set_macros(MacroState {
        account: vec![MacroView {
            name: "spawn".into(),
            texture: Some("Interface\\Icons\\Ability_Racial_Cannibalize".into()),
            body: ".spawn 16032".into(),
            local_only: false,
        }],
        character: Vec::new(),
    });
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_Racial_Cannibalize".into()),
            kind: 0x40, // MACRO
            action: 1,  // macro index 1
            count: 0,
            consumable: false,
        }),
    );
    s.set_action(
        2,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_A".into()),
            kind: 0x00, // SPELL
            action: 111,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    // Read the fontstrings by name: painted text is ambiguous.
    let name_of = |s: &UiScript, n: u32| {
        s.eval::<String>(&format!("return ActionButton{n}Name:GetText() or \"\""))
            .unwrap()
    };
    assert_eq!(
        name_of(&s, 1),
        "spawn",
        "a MACRO slot wears its macro's name"
    );
    assert_eq!(name_of(&s, 2), "", "a SPELL slot has no name line");
    assert!(
        s.extract()
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "spawn")),
        "the name reaches the screen"
    );

    s.set_action(1, None);
    s.fire_event("ACTIONBAR_SLOT_CHANGED", vec![ScriptValue::Int(1)]);
    s.resolve();
    assert_eq!(name_of(&s, 1), "", "an emptied slot loses the name");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The stock bag bar (`MainMenuBarBagButtons.xml`), which seats on `MainMenuBarArtFrame`: its
/// toggle opens the backpack, the stack paints in its slot, and the slot's clicks queue intents.
#[test]
fn shipped_bag_frame_drives_end_to_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    use super::test_ui::{bag_open, bag_slot_button, centre_of, load_ui, BAG_UI};
    use benilla_ui::script::{ContainerSlot, ContainerState};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // `BAG_UI` is `benilla.toc`'s order for a bag window. The bag buttons are parented to
    // `MainMenuBarArtFrame`, so the main bar's files follow Cooldown.xml, and
    // `ContainerFrameItemButton_OnClick` reads `StackSplitFrame` and `MerchantFrame`
    // (ContainerFrame.lua:581, 586), so both load after the bags.
    let mut bar_frames = 0;
    for file in BAG_UI {
        let frames = load_ui(&s, file);
        if *file == "Interface\\FrameXML\\Cooldown.xml" {
            load_ui(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
            load_ui(&s, "Interface\\FrameXML\\TextStatusBar.lua");
            load_ui(&s, "Interface\\FrameXML\\TextStatusBar.xml");
            load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
            load_ui(&s, r"Interface\FrameXML\UIParent.xml");
            load_ui(&s, "ScrollTemplates.xml"); // our scroll kits
            load_ui(&s, "Interface\\FrameXML\\GlobalStrings.lua");
            load_ui(&s, "Interface\\FrameXML\\MainMenuBar.xml");
            load_ui(&s, "Interface\\FrameXML\\ActionBarFrame.xml");
            load_ui(&s, "Interface\\FrameXML\\BonusActionBarFrame.xml");
        }
        if *file == "Interface\\FrameXML\\MainMenuBarBagButtons.xml" {
            bar_frames = frames;
        }
    }
    load_ui(&s, "Interface\\FrameXML\\StackSplitFrame.xml");
    load_ui(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_ui(&s, "Interface\\FrameXML\\MerchantFrame.xml");

    assert_eq!(
        bar_frames, 16,
        "the bag bar is six CheckButtons — MainMenuBarBackpackButton, CharacterBag0..3Slot, \
         KeyRingButton — each carrying one $parentItemAnim Model (6 + 6), plus a $parentCooldown \
         Model on each of the four slots that inherit PaperDollItemSlotButtonTemplate (+4). The \
         deleted BagFrame.xml built 12: it mirrored the six buttons and their push cards but had \
         no cooldown on a bag-bar slot at all, which is the reference's own and is what the swap \
         to Interface\\FrameXML\\MainMenuBarBagButtons.xml brought with it (1751 window 3)"
    );

    // The app's feed: a backpack with Tough Jerky ×5 in slot 1.
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
            link: None,
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
    s.fire_event("BAG_UPDATE", vec![ScriptValue::Int(0)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    s.resolve();
    let jerky_visible = |quads: &[benilla_ui::script::ExtractedQuad]| {
        quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("INV_Misc_Food_16"))
        })
    };
    assert!(!jerky_visible(&s.extract()), "no bag window at load");

    // The toggle seats at the art frame's BOTTOMRIGHT + (-6, 2), 37x37
    // (MainMenuBarBagButtons.xml:54-64), and that corner is the bar's (1024, 0): x[981,1018]
    // y[2,39]. The click stays at literal coordinates because hitting them is under test.
    s.mouse_button(999.0, 20.0, "LeftButton", true);
    s.mouse_button(999.0, 20.0, "LeftButton", false);
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(bag_open(&s, 0), "the bar's toggle opened the backpack");
    let quads = s.extract();
    let icon = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("INV_Misc_Food_16"))
        })
        .and_then(|q| q.rect)
        .expect("slot 1 icon visible after toggle");
    // Ask the slot's button by GetID: the buttons are numbered backwards, `…Item1` is the bag's
    // last slot (ContainerFrame.lua:425-428).
    let button = bag_slot_button(&s, 0, 1);
    let (bx, by) = centre_of(&mut s, &button);
    assert!(
        icon.left <= bx && bx <= icon.right && icon.bottom <= by && by <= icon.top,
        "the jerky icon {icon:?} is not painted on slot 1's button ({button} at {bx},{by})"
    );
    assert!(
        quads
            .iter()
            .any(|q| { matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "5") }),
        "stack count shows"
    );

    // Left-click picks the item up (`PickupContainerItem`, ContainerFrame.lua:580).
    s.mouse_button(bx, by, "LeftButton", true);
    s.mouse_button(bx, by, "LeftButton", false);
    assert!(
        s.take_container_uses().is_empty(),
        "left-click is a pickup, not a use"
    );
    assert!(
        s.cursor_item().is_some(),
        "left-click put the item on the cursor"
    );
    assert!(
        s.take_container_moves().is_empty(),
        "a pickup alone queues no move"
    );

    // Right-click while holding: the right arm has no cursor test (ContainerFrame.lua:583-599).
    // What the reference client does with a use while the cursor holds an item is untraced.
    s.mouse_button(bx, by, "RightButton", true);
    s.mouse_button(bx, by, "RightButton", false);
    assert_eq!(
        s.take_container_uses(),
        vec![(0, 1)],
        "the reference's right arm uses the slot even with a full cursor"
    );
    assert!(
        s.cursor_item().is_some(),
        "…and leaves the held item where it was"
    );
    s.run("ClearCursor()").unwrap();

    s.mouse_button(bx, by, "RightButton", true);
    s.mouse_button(bx, by, "RightButton", false);
    assert_eq!(s.take_container_uses(), vec![(0, 1)]);

    s.mouse_button(999.0, 20.0, "LeftButton", true);
    s.mouse_button(999.0, 20.0, "LeftButton", false);
    s.resolve();
    assert!(!bag_open(&s, 0), "toggle closes the window");
    assert!(!jerky_visible(&s.extract()), "…and its slots with it");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `ActionButton_Update` registers the state events only while the button has an action
/// (ActionButton.lua:175-212), so an empty well's texture-less icon never takes a tint.
#[test]
fn state_events_leave_empty_wells_untinted() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    // One occupied slot; 2..12 empty.
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            kind: 0x00,
            action: 133,
            count: 0,
            consumable: false,
        }),
    );
    s.set_action_state(
        1,
        Some(ActionState {
            usable: false,
            not_enough_mana: true,
            ..Default::default()
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    s.fire_event("ACTIONBAR_UPDATE_STATE", vec![]);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.resolve();

    // No well-sized texture-less coloured quad; the occupied icon keeps its out-of-power blue.
    let mut oom_icon = None;
    for q in s.extract() {
        match &q.content {
            QuadContent::Texture {
                path: None,
                color: Some(c),
                ..
            } if q.rect.is_some_and(|r| r.right - r.left <= 40.0) => {
                // Well-sized only: the XP bar's 1024-wide black backdrop is a legitimate solid.
                panic!("an empty well gained a solid color quad: {c:?}")
            }
            QuadContent::Texture {
                path: Some(p),
                color,
                ..
            } if p.contains("Spell_Fire_FlameBolt") => oom_icon = *color,
            _ => {}
        }
    }
    assert_eq!(
        oom_icon,
        Some([0.5, 0.5, 1.0, 1.0]),
        "the occupied button still tints OOM blue"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A slot going empty draws no plate: the empty arm hides the icon (ActionButton.lua:168), and a
/// texture-less region never draws, whatever its tint (`0x7706e0` gates on `+0xcc`).
#[test]
fn an_occupied_slot_going_empty_leaves_no_white_plate() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    s.set_action(
        3,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Nature_HealingTouch".into()),
            kind: 0x00,
            action: 5185,
            count: 0,
            consumable: false,
        }),
    );
    s.set_action_state(
        3,
        Some(ActionState {
            usable: true,
            ..Default::default()
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    // The usable pass tints the occupied icon 1/1/1, a tint that outlives the emptying.
    s.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
    s.resolve();
    assert!(
        s.extract().iter().any(|q| matches!(&q.content,
            QuadContent::Texture { path: Some(p), .. } if p.contains("Spell_Nature_HealingTouch"))),
        "the occupied slot draws its icon"
    );

    // The character switch: the new character's table has nothing in slot 3.
    s.set_action(3, None);
    s.set_action_state(3, None);
    s.fire_event("ACTIONBAR_SLOT_CHANGED", vec![ScriptValue::Int(3)]);
    s.resolve();
    for q in s.extract() {
        match &q.content {
            QuadContent::Texture { path: Some(p), .. }
                if p.contains("Spell_Nature_HealingTouch") =>
            {
                panic!("the emptied slot still draws the old icon")
            }
            QuadContent::Texture {
                path: None,
                color: Some(c),
                ..
            } if q.rect.is_some_and(|r| r.right - r.left <= 40.0) => {
                // Well-sized only: page-wide solids are legitimate.
                panic!("the emptied slot draws its surviving tint as a solid plate: {c:?}")
            }
            _ => {}
        }
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `BonusActionBarFrame` is declared hidden (BonusActionBarFrame.xml:54); addons lay out its
/// buttons before it ever shows (`CT_BarModOptions.lua:154`).
#[test]
fn the_bonus_action_bar_exists_hidden_and_takes_layout_calls() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);

    assert!(
        s.eval::<bool>("return BonusActionBarFrame ~= nil").unwrap(),
        "5 corpus addons index BonusActionBarFrame by name"
    );
    assert!(
        !s.eval::<bool>("return BonusActionBarFrame:IsShown()")
            .unwrap(),
        "it must ship HIDDEN — benilla re-pages the main bar instead of showing this one"
    );

    for i in [1, 12] {
        assert!(
            s.eval::<bool>(&format!("return BonusActionButton{i} ~= nil"))
                .unwrap(),
            "BonusActionButton{i} must exist"
        );
        // CT_BarModOptions.lua:154's exact pair, on a bar that has never been shown.
        s.run(&format!("BonusActionButton{i}:ClearAllPoints()"))
            .unwrap();
        s.run(&format!(
            "BonusActionButton{i}:SetPoint(\"TOP\", \"ActionButton1\", \"BOTTOM\", 0, -4)"
        ))
        .unwrap();
    }

    // The hidden bar leaves the main bar alone. An empty main-bar button hides while
    // `showgrid == 0` (ActionButton.lua:214-215), so slot 1 is occupied first.
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
    s.fire_event("ACTIONBAR_SLOT_CHANGED", vec![ScriptValue::Int(1)]);
    assert!(
        s.eval::<bool>("return ActionButton1:IsShown()").unwrap(),
        "the main bar is untouched"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// ActionButton.lua:1-9's constants are globals (`zBar.lua:40` loops to `NUM_ACTIONBAR_BUTTONS`).
#[test]
fn the_reference_action_bar_constants_are_defined() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_action_bar(&s);

    for (name, want) in [
        ("NUM_ACTIONBAR_PAGES", 6),
        ("NUM_ACTIONBAR_BUTTONS", 12),
        ("BOTTOMLEFT_ACTIONBAR_PAGE", 6),
        ("BOTTOMRIGHT_ACTIONBAR_PAGE", 5),
        ("LEFT_ACTIONBAR_PAGE", 4),
        ("RIGHT_ACTIONBAR_PAGE", 3),
    ] {
        assert_eq!(
            s.eval::<i64>(&format!("return {name}")).unwrap(),
            want,
            "{name} must be the reference's value"
        );
    }

    // zBar's expression.
    assert_eq!(
        s.eval::<i64>("local to = nil or nil or NUM_ACTIONBAR_BUTTONS local n = 0 for i = 1, to do n = n + 1 end return n")
            .unwrap(),
        12,
        "zBar.lua:40's numeric for must have a limit"
    );

    // `CURRENT_ACTIONBAR_PAGE` is live state that `ActionButton_GetPagedID` reads, so it must move.
    assert_eq!(s.eval::<i64>("return CURRENT_ACTIONBAR_PAGE").unwrap(), 1);
    s.run("ActionBar_PageUp()").unwrap();
    assert_eq!(
        s.eval::<i64>("return CURRENT_ACTIONBAR_PAGE").unwrap(),
        2,
        "a frozen 1 would still be a lie — this one has to move"
    );
    s.run("ActionBar_PageDown()").unwrap();
}

/// `ActionButtonTemplate` is regions only (ActionButtonTemplate.xml:3); `ActionBarButtonTemplate`
/// adds the handlers (ActionBarFrame.xml:4), and `zBar.xml:7` inherits it.
#[test]
fn both_reference_action_button_templates_are_inheritable() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_action_bar(&s);

    // zBar's exact shape: inherit the bar template, supply your own OnLoad.
    let doc = benilla_ui::framexml::parse(
        r#"<Ui>
            <CheckButton name="ZLikeButton" inherits="ActionBarButtonTemplate" parent="UIParent" id="1">
                <Anchors><Anchor point="CENTER"/></Anchors>
            </CheckButton>
            <CheckButton name="BareLikeButton" inherits="ActionButtonTemplate" id="1">
                <Anchors><Anchor point="TOPLEFT"/></Anchors>
            </CheckButton>
        </Ui>"#,
    )
    .unwrap();
    let report = benilla_ui::loader::load(&s, &doc, &|_| None);
    assert!(
        report.errors.is_empty(),
        "loader errors: {:?}",
        report.errors
    );
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.contains("unknown template")),
        "both names must resolve: {:?}",
        report.warnings
    );

    // The derived name zBar reads, on a button built from each half.
    for owner in ["ZLikeButton", "BareLikeButton"] {
        assert!(
            s.eval::<bool>(&format!("return {owner}NormalTexture ~= nil"))
                .unwrap(),
            "{owner}NormalTexture — zBar.lua:88's read"
        );
        assert!(
            s.eval::<bool>(&format!(
                "return {owner}Icon ~= nil and {owner}Cooldown ~= nil"
            ))
            .unwrap(),
            "{owner} must carry the template's regions"
        );
    }

    // The base half carries no handlers: an addon inheriting it wires its own.
    assert!(
        !s.eval::<bool>("return BareLikeButton:GetScript(\"OnClick\") ~= nil")
            .unwrap(),
        "ActionButtonTemplate is regions only; handlers belong to the bar half"
    );
    assert!(
        s.eval::<bool>("return ZLikeButton:GetScript(\"OnClick\") ~= nil")
            .unwrap(),
        "ActionBarButtonTemplate carries the handler set"
    );

    // ...and the stock bar's own buttons carry both.
    assert!(
        s.eval::<bool>("return ActionButton1NormalTexture ~= nil and ActionButton1:GetScript(\"OnClick\") ~= nil")
            .unwrap(),
        "BenillaActionButtonTemplate's 48 inherits= sites must be untouched by the split"
    );
}

/// Main-bar paging (ActionButton.lua:63-101): page-up past the last viewable page wraps to 1,
/// page-down below 1 rescans for the last viewable one.
#[test]
fn the_main_bar_pages_and_a_bonus_page_still_outranks_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in [
        // Fonts first: the reputation pane's check boxes read `RED_FONT_COLOR` in their OnLoad.
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        // `ExhaustionTick_Update` reads `ReputationWatchBar`, which ReputationFrame.xml declares.
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
        // `UIOptionsFrame_Init`'s uvars and `UIOptionsFrameCheckButtons`, which
        // `MultiActionBarFrame_OnLoad` writes (MultiActionBars.lua:8-22), so ahead of the bars.
        r"Interface\FrameXML\OptionsFrame.lua",
        r"Interface\FrameXML\UIOptionsFrame.xml",
        "OptionsFrame.xml",
        "Interface\\FrameXML\\MultiActionBars.xml",
    ] {
        super::test_ui::load_ui(&s, file);
    }

    assert_eq!(s.eval::<i64>("return CURRENT_ACTIONBAR_PAGE").unwrap(), 1);
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        1,
        "page 1 button 1 is action 1"
    );
    assert!(
        s.eval::<bool>(
            "for i = 1, NUM_ACTIONBAR_PAGES do \
               if not VIEWABLE_ACTION_BAR_PAGES[i] then return false end \
             end return true"
        )
        .unwrap(),
        "all six pages are viewable at rest — every extra bar ships off (1500)"
    );
    // The page numeral: `ActionBarUpButton` writes it at load and on `ACTIONBAR_PAGE_CHANGED`
    // (ActionBarFrame.xml:175, 180).
    let page_text = |s: &UiScript| {
        s.eval::<String>("return MainMenuBarPageNumber:GetText()")
            .unwrap()
    };
    assert_eq!(page_text(&s), "1", "the load seed is the current page");

    // Raising both bottom bars takes pages 5 and 6 out of the cycle (MultiActionBars.lua:53, 61).
    s.run("SHOW_MULTI_ACTIONBAR_1 = 1 SHOW_MULTI_ACTIONBAR_2 = 1 MultiActionBar_Update()")
        .unwrap();

    // Page up walks to 2, so button 1 shows action 13.
    s.run("ActionBar_PageUp()").unwrap();
    assert_eq!(s.eval::<i64>("return CURRENT_ACTIONBAR_PAGE").unwrap(), 2);
    assert_eq!(page_text(&s), "2", "the numeral follows the page up");
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        13
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton12)")
            .unwrap(),
        24
    );

    // Down again; below page 1 it rescans for the last viewable page.
    s.run("ActionBar_PageDown()").unwrap();
    assert_eq!(s.eval::<i64>("return CURRENT_ACTIONBAR_PAGE").unwrap(), 1);
    s.run("ActionBar_PageDown()").unwrap();
    assert_eq!(
        s.eval::<i64>("return CURRENT_ACTIONBAR_PAGE").unwrap(),
        4,
        "page-down off the bottom rescans for the last VIEWABLE page — 4, not 6, because the \
         two raised bottom multibars own pages 5 and 6"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        37
    );
    assert_eq!(
        page_text(&s),
        "4",
        "the numeral follows a wrap, not just a step"
    );
    // The bottom bars' pages are unreachable from the main bar.
    assert!(s
        .eval::<bool>(
            "return VIEWABLE_ACTION_BAR_PAGES[5] == nil and VIEWABLE_ACTION_BAR_PAGES[6] == nil"
        )
        .unwrap());
    s.run("CURRENT_ACTIONBAR_PAGE = 4 ActionBar_PageUp()")
        .unwrap();
    assert_eq!(
        s.eval::<i64>("return CURRENT_ACTIONBAR_PAGE").unwrap(),
        1,
        "walking up from the last viewable page skips 5 and 6 and wraps to the LITERAL 1 — the \
         reference's own asymmetry with page-down, which rescans instead"
    );

    // The bonus branch needs `isBonus` and page 1 (ActionButton.lua:447); a main-bar button has
    // no `isBonus`, so slot 1 on page 3 is action 25 whatever the offset.
    s.run("CURRENT_ACTIONBAR_PAGE = 3").unwrap();
    s.set_bonus_bar_offset(1);
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        25,
        "a main-bar button follows its page, never the bonus offset (ActionButton.lua:447-456)"
    );
    s.run("CURRENT_ACTIONBAR_PAGE = 1").unwrap();
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(BonusActionButton1)")
            .unwrap(),
        73,
        "the bonus frame's own button, on page 1 with offset 1, is 72 + 1"
    );
    s.run("CURRENT_ACTIONBAR_PAGE = 3").unwrap();
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(BonusActionButton1)")
            .unwrap(),
        25,
        "and off page 1 even the bonus button follows the page: the conjunct 1897 found missing"
    );
    s.set_bonus_bar_offset(0);
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        25,
        "and the page comes back when the form drops"
    );
}

/// The form swap (BonusActionBarFrame.lua:1-98): the bonus frame slides up and sounds on landing,
/// stays while the form holds, and slides down silently with the old page.
#[test]
fn bonus_bar_slides_up_with_sound_and_down_without() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Cooldown.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIParent.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\MainMenuBar.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\MoneyFrame.lua");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\MoneyFrame.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\GameTooltip.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\ActionBarFrame.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\BonusActionBarFrame.xml");

    // `ExhaustionTick_Update` reads `ReputationWatchBar`, which ReputationFrame.xml declares.
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\OptionsFrameTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\ReputationFrame.xml");
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.tick(10.0); // a nonzero clock epoch
    let _ = s.take_sounds();
    assert!(
        !s.eval::<bool>("return BonusActionBarFrame:IsShown()")
            .unwrap(),
        "no form, no overlay"
    );

    // ── Enter a form: offset 0 to 1, fired on the app's offset edge ──
    s.set_bonus_bar_offset(1);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return BonusActionBarFrame:IsShown()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return BonusActionBarFrame.mode").unwrap(),
        "show"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        1,
        "the main bar holds the OLD page under the rising overlay"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(BonusActionButton1)")
            .unwrap(),
        73,
        "the overlay paints the bonus page from the first slide frame"
    );
    assert!(s.take_sounds().is_empty(), "no sound until the bar lands");

    // Keys route to the overlay at once: `ActionButtonDown/Up` fork on
    // `BonusActionBarFrame:IsShown()` (ActionButton.lua:16, 31).
    s.run("ActionButtonDown(1) ActionButtonUp(1)").unwrap();
    assert_eq!(
        action_ids(&mut s),
        vec![73],
        "a key pressed mid-slide already drives the bonus page"
    );

    // …and the self-cast bindings, which differ only in `ActionButtonUp(n, 1)`
    // (Bindings.xml:205-211).
    s.run("ActionButtonDown(1) ActionButtonUp(1, 1)").unwrap();
    assert_eq!(
        s.take_action_uses()
            .into_iter()
            .map(|u| (u.action, u.on_self))
            .collect::<Vec<_>>(),
        vec![(73, false)],
        "the BONUS branch of stock ActionButtonUp passes a literal 0 for onSelf (ActionButton.lua:38) \
         — in a stance, ALT-1 drives the bonus page without self-cast; the reference's own rule, \
         which 1745's expectation of (73, true) had smoothed over"
    );
    s.run("BonusActionBarFrame:Hide() ActionButtonDown(1) ActionButtonUp(1, 1)")
        .unwrap();
    assert_eq!(
        s.take_action_uses()
            .into_iter()
            .map(|u| (u.action, u.on_self))
            .collect::<Vec<_>>(),
        vec![(1, true)],
        "on the main-bar branch onSelf reaches UseAction's third argument (ActionButton.lua:51)"
    );
    s.run("BonusActionBarFrame:Show()").unwrap();

    // `BonusActionBar_OnUpdate` paints, then advances (BonusActionBarFrame.lua:37-52): top 0, then
    // 0.5 * 43, then the landing.
    s.tick(0.075);
    let top = s
        .eval::<f64>("return BonusActionBarFrame:GetTop()")
        .unwrap();
    assert!(top.abs() < 0.01, "first frame top = {top}, want 0");
    assert!(s.take_sounds().is_empty());
    s.tick(0.075);
    let top = s
        .eval::<f64>("return BonusActionBarFrame:GetTop()")
        .unwrap();
    assert!(
        (top - 21.5).abs() < 0.6,
        "half-slide top = {top}, want ~21.5"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        1
    );
    assert!(s.take_sounds().is_empty());

    // Landing snaps to 43 and plays the sound (BonusActionBarFrame.lua:61-64); the main bar keeps
    // its page, having no `isBonus` (ActionButton.lua:447).
    s.tick(0.01);
    assert_eq!(
        s.take_sounds(),
        vec![benilla_ui::script::SoundRequest::KitName(
            "igBonusBarOpen".into()
        )]
    );
    let top = s
        .eval::<f64>("return BonusActionBarFrame:GetTop()")
        .unwrap();
    assert!((top - 43.0).abs() < 0.01, "landed top = {top}, want 43");
    assert_eq!(
        s.eval::<String>("return BonusActionBarFrame.mode").unwrap(),
        "none"
    );
    assert!(
        s.eval::<bool>("return BonusActionBarFrame:IsShown()")
            .unwrap(),
        "the overlay stays up while the form holds — the ref-visible state"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        1,
        "the main bar keeps its page under the landed overlay"
    );
    s.tick(0.5);
    assert!(s.take_sounds().is_empty(), "a landed bar never re-sounds");

    // ── A form to form swap: repaint, no slide, no sound ──
    s.set_bonus_bar_offset(3);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    assert_eq!(
        s.eval::<String>("return BonusActionBarFrame.mode").unwrap(),
        "none"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(BonusActionButton1)")
            .unwrap(),
        97
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        1,
        "the main bar stays on its page through a form swap"
    );
    s.tick(0.2);
    assert!(
        s.take_sounds().is_empty(),
        "form→form never re-slides or re-sounds"
    );

    // ── Drop the form: the overlay descends with the old page, silently ──
    s.set_bonus_bar_offset(0);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    assert_eq!(
        s.eval::<String>("return BonusActionBarFrame.mode").unwrap(),
        "hide"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        1,
        "the main bar returns to the page immediately — it is being revealed"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(BonusActionButton1)")
            .unwrap(),
        97,
        "the descending overlay carries the OLD form's page (lastBonusBar)"
    );
    // A key mid-descent drives the old page: `ActionButton_GetPagedID` falls back to
    // `lastBonusBar` (ActionButton.lua:449-451).
    s.run("ActionButtonDown(1) ActionButtonUp(1)").unwrap();
    assert_eq!(action_ids(&mut s), vec![97]);
    // The frame that finds the timer past the slide time hides it (BonusActionBarFrame.lua:53-68),
    // one OnUpdate past the time.
    s.tick(0.2);
    s.tick(0.01);
    assert!(
        !s.eval::<bool>("return BonusActionBarFrame:IsShown()")
            .unwrap(),
        "the descent ends hidden"
    );
    assert!(
        s.take_sounds().is_empty(),
        "the down-slide is silent — the ref plays only on open"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A form dropped mid-rise: `HideBonusActionBar` keeps the running timer
/// (BonusActionBarFrame.lua:90-92), and the hide arm paints `(1 - timer / 0.15) * 43` (l.48).
#[test]
fn bonus_bar_turnaround_continues_from_position() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Cooldown.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIParent.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\MainMenuBar.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\MoneyFrame.lua");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\MoneyFrame.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\GameTooltip.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\ActionBarFrame.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\BonusActionBarFrame.xml");

    // `ExhaustionTick_Update` reads `ReputationWatchBar`, which ReputationFrame.xml declares.
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\OptionsFrameTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\ReputationFrame.xml");
    s.tick(10.0);
    let _ = s.take_sounds();

    // The form drops with the rise's timer at half.
    s.set_bonus_bar_offset(1);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    s.tick(0.075);
    s.set_bonus_bar_offset(0);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    assert_eq!(
        s.eval::<String>("return BonusActionBarFrame.mode").unwrap(),
        "hide"
    );
    // The rise's one OnUpdate left the timer at 0.075, so the hide arm paints
    // (1 - 0.075/0.15) * 43 = 21.5, then 12.9 a frame later: paint, then advance.
    s.tick(0.03);
    let top = s
        .eval::<f64>("return BonusActionBarFrame:GetTop()")
        .unwrap();
    assert!(
        (top - 21.5).abs() < 0.6,
        "turnaround descends from 21.5, top = {top}"
    );
    s.tick(0.03);
    let top = s
        .eval::<f64>("return BonusActionBarFrame:GetTop()")
        .unwrap();
    assert!(
        (top - 12.9).abs() < 0.6,
        "next frame top = {top}, want ~12.9"
    );
    s.tick(0.2);
    s.tick(0.01);
    assert!(!s
        .eval::<bool>("return BonusActionBarFrame:IsShown()")
        .unwrap());
    assert!(
        s.take_sounds().is_empty(),
        "an aborted rise never lands, so it never sounds"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The page arrows are 32x32 squares 20 px apart, overlapping by 12 px; their `<HitRectInsets>`
/// (6 each side, 7 top and bottom, ActionBarFrame.xml:170-172) shrink each hit rect to 20x18,
/// which separates them.
#[test]
fn the_page_arrows_do_not_steal_each_other_s_clicks() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Cooldown.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIParent.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\MainMenuBar.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\MoneyFrame.lua");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\MoneyFrame.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\GameTooltip.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\ActionBarFrame.xml");
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\BonusActionBarFrame.xml");

    // `ExhaustionTick_Update` reads `ReputationWatchBar`, which ReputationFrame.xml declares.
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\OptionsFrameTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\ReputationFrame.xml");
    // The rested marker is declared shown in DIALOG strata over the arrows (MainMenuBar.xml:415);
    // `ExhaustionTick_Update` hides it on PLAYER_ENTERING_WORLD when `GetXPExhaustion()` is nil
    // (MainMenuBar.lua:29-31), or it eats every click.
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    let centre = |s: &UiScript, name: &str| {
        s.eval::<(f64, f64)>(&format!(
            "return ({name}:GetLeft() + {name}:GetRight()) / 2, \
                    ({name}:GetBottom() + {name}:GetTop()) / 2"
        ))
        .unwrap()
    };
    for name in ["ActionBarUpButton", "ActionBarDownButton"] {
        let (x, y) = centre(&s, name);
        assert_eq!(
            s.hit_test_name(x as f32, y as f32).as_deref(),
            Some(name),
            "{name} must eat the click at its own centre"
        );
    }

    // 8 px above the up button's bottom edge is inside the down button's raw square but only the
    // up button's hit rect.
    let (x, up_bottom) = s
        .eval::<(f64, f64)>(
            "return (ActionBarUpButton:GetLeft() + ActionBarUpButton:GetRight()) / 2, \
                    ActionBarUpButton:GetBottom()",
        )
        .unwrap();
    assert_eq!(
        s.hit_test_name(x as f32, up_bottom as f32 + 8.0).as_deref(),
        Some("ActionBarUpButton"),
        "the lower third of the visible UP arrow must page up, not down"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
