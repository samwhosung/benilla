use benilla_ui::script::{ActionSlot, QuadContent, ScriptValue, UiScript};

/// The queued action ids alone — `take_action_uses` carries `UseAction`'s self-cast modifier
/// beside the id since 1745, and every assertion in this file is about the id.
fn action_ids(s: &mut UiScript) -> Vec<u32> {
    s.take_action_uses().into_iter().map(|u| u.action).collect()
}

/// Load the stock `Interface\FrameXML\ActionBarFrame.xml` into a bare engine and
/// drive it with a synthetic action snapshot — the slice-1 chain minus Bevy: template
/// expansion over 12 instances, the vanilla bonus-page formula, icon paint on events, empty
/// slots drawing no icon, and a physical click queuing the right UseAction id.
#[test]
fn shipped_action_bar_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Cooldown.xml");
    // `load_ui` returns the same `report.frames` the disk reader asserted on, so this
    // count is the one that always stood here — moved, not re-derived.
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

    // `ExhaustionTick_Update` indexes `ReputationWatchBar` UNGUARDED — the reference's own code,
    // safe there because `ReputationFrame.xml` is always loaded and always declares the bar. Since
    // 1875 that file is the reference's own, so a harness that drives the XP bar has to load it too
    // or the tick raises on its first event.
    // In manifest order: the fonts its check-box labels colour from, the panel templates those
    // boxes inherit through, then the pane.
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\OptionsFrameTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\ReputationFrame.xml");

    // A warrior in battle stance: offset 1 ⇒ the bar shows actions 73..84.
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
    // The app fires this on the offset's edge (ui_action/feed.rs); the stock bonus frame shows on
    // it and slides up over the main bar for BONUSACTIONBAR_SLIDETIME (0.15 s) — one OnUpdate
    // paints the start, the next lands it.
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    s.tick(10.0);
    s.tick(0.2);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The stance page is painted by BonusActionButton1/2, not by the main bar (1897/1938). The
    // 1024-wide bar centers at BOTTOM of the 1024-wide screen ⇒ bar left edge = 0; the bonus
    // frame lands at the bar's BOTTOMLEFT + (BONUSACTIONBAR_XPOS 4, BONUSACTIONBAR_YPOS 43), is
    // 43 high, and its button 1 sits at its own BOTTOMLEFT + (5, 4), 36×36 ⇒ x[9,45] y[4,40] —
    // one pixel right of the main bar's (8, 4). The reference's own geometry, off
    // BonusActionBarFrame.xml:54-96; the chain stride is 36 + 6 = 42.
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
                                     // Only an OCCUPIED button is shown — an empty one hides while showgrid == 0
                                     // (ActionButton.lua:69-70) — so two rings, two icons.
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

    // A physical click at (26, 22) lands on BonusActionButton1 (9..45 × 4..40, two frame levels
    // above the hidden main slot) and queues UseAction(73) — the stance page's id, not 1.
    s.mouse_button(26.0, 22.0, "LeftButton", true);
    s.mouse_button(26.0, 22.0, "LeftButton", false);
    assert_eq!(action_ids(&mut s), vec![73]);

    // The keybinding entry (the app's key feed runs `ActionButtonDown/Up(i)` on the two
    // key edges — the ref's ACTIONBUTTONn binding, ActionButton.lua:15-45): UP fires UseAction
    // directly with no checkCursor (a keybind never places) — but ONLY from
    // the PUSHED state a DOWN set, so a stray release with no press is the ref's own no-op.
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

    // Stance drops (offset 0): the bar re-pages to actions 1..12 — all empty here, icons clear.
    s.set_bonus_bar_offset(0);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    // The overlay descends carrying the old form's page (`lastBonusBar`) and hides at the end
    // of its slide — one OnUpdate more than BONUSACTIONBAR_SLIDETIME (paint, then advance).
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

    // `ExhaustionTick_Update` indexes `ReputationWatchBar` UNGUARDED — the reference's own code,
    // safe there because `ReputationFrame.xml` is always loaded and always declares the bar. Since
    // 1875 that file is the reference's own, so a harness that drives the XP bar has to load it too
    // or the tick raises on its first event.
    // In manifest order: the fonts its check-box labels colour from, the panel templates those
    // boxes inherit through, then the pane.
    super::test_ui::load_ui(s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(s, r"Interface\FrameXML\UIPanelTemplates.lua");
    super::test_ui::load_ui(s, r"Interface\FrameXML\UIPanelTemplates.xml");
    super::test_ui::load_ui(s, r"Interface\FrameXML\OptionsFrameTemplates.xml");
    super::test_ui::load_ui(s, r"Interface\FrameXML\ReputationFrame.xml");
    // The dialog engine — the keybindings page registers its two confirms into its table (1960).
    super::test_ui::load_ui(s, r"Interface\FrameXML\BasicControls.xml"); // `TEXT`
    super::test_ui::load_ui(s, r"Interface\FrameXML\LocaleProperties.lua"); // `GetText`
    super::test_ui::load_ui(s, r"Interface\FrameXML\StaticPopup.xml");
    super::test_ui::load_ui(s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    // `LOCK_ACTIONBAR` and `ALWAYS_SHOW_MULTIBARS` are declared by `UIOptionsFrame_Init` — the
    // reference's own home for them, and off the chain since 2115 (they were our
    // `OptionsFrame.xml`'s from 1938 until then). The manifest loads this before the bars and
    // before our window, whose Action Bars rows capture the value as their Defaults; so does this.
    super::test_ui::load_ui(s, r"Interface\FrameXML\OptionsFrame.lua");
    super::test_ui::load_ui(s, r"Interface\FrameXML\UIOptionsFrame.xml");
    super::test_ui::load_ui(s, "ScrollTemplates.xml");
    super::test_ui::load_ui(s, "KeyBindingsPage.xml");
    super::test_ui::load_ui(s, "OptionsFrame.xml");
}

/// The state/feedback layer (decision 0137 phase 4) through the REAL shipped XML: a pushed
/// cooldown + `ACTIONBAR_UPDATE_COOLDOWN` arms the button's Cooldown widget, `IsCurrentAction` +
/// `ACTIONBAR_UPDATE_STATE` checks the ring, and `IsUsableAction`'s OOM pair blue-tints the icon.
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

    // A running 10 s cooldown with 6 s left + the update event: the widget shows mid-sweep.
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            cooldown: Some((6_000, 10_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    // The stock machine: `CooldownFrame_SetTimer` arms sequence 0 and shows the
    // pane; the next paint's `OnUpdateModel` scrubs it to `(GetTime() − start) / duration`.
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

    // The checked ring on the current action (the transcribed UpdateState).
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

    // The OOM blue tint (the transcribed UpdateUsable): usable=false + notEnoughMana=true.
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

    // The PLAIN grey — `usable = false` with `notEnoughMana = false`, which is what an unusable
    // item reads (food in combat, the whole point of the ITEM arm's spell walk). Stock
    // `ActionButton_UpdateUsable`'s `else` dims the ICON to 0.4 and leaves the normal texture
    // at full strength, so the ring keeps its colour under a dead icon.
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

/// **Why a restarted `GetTime` clock reads on the bar as "no cooldown at all"**,
/// through the shipped `Cooldown.lua`: `CooldownFrame_SetTimer`'s only gate is
/// `start > 0 and duration > 0 and enable > 0`, and its `else` branch is `this:Hide()`. So the
/// SAME running cooldown draws or vanishes purely on which clock its start was converted against.
///
/// This is the observable half of the relog bug: the store held the cooldown (the press was still
/// refused), the feed pushed a triple every frame, and the button showed nothing — because the VM
/// had been rebuilt and its clock had gone back to zero, putting every already-running cooldown's
/// start behind the new epoch.
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
    s.tick(10.0); // GetTime == 10 — a clock that restarted ten seconds ago

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

    // The same cooldown on a clock that never restarted: GetTime 100, armed at 70. The sweep is
    // exactly where it belongs.
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

/// The pie paints OVER its button's art. The Cooldown child is born at button-level+1, and the
/// draw key's LEVEL term outranks 0884's bucket-wide layer term — so the sweep quad must sort
/// after the icon (BACKGROUND) and after the button's own special textures. A regression here is
/// invisible to every store/feed instrument (the triple still pushes; only the pixels vanish
/// under the icon), which is exactly why the order is pinned end-to-end through the real XML.
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

/// **A cooldown-count addon's hook leaves the sweep exactly where it was.**
///
/// `!OmniCC` 6.8.30 — the cooldown addon on the director's screen — is a single wrap of the
/// FrameXML global: it captures `CooldownFrame_SetTimer` in an upvalue, replaces the global with
/// a function that calls through, and hangs a `Frame` + `FontString` off the button for its own
/// countdown, keeping the handle on a field of the cooldown widget itself (`cd.textFrame`). The
/// shape is transcribed here — the widget verbs it uses, not its source — because that shape
/// touches everything the pie's paint depends on: the global the bar calls, the widget's own
/// Lua fields (`start`/`duration`/`stopping`, which the stock `Cooldown.lua` writes and its
/// `OnUpdateModel` reads back), and the button's frame-level stack.
///
/// It came in as "with `!OmniCC` installed the pie and the GCD sweep are gone", and this is the
/// half that answers whether the addon's hook itself is what breaks them. It is not: the engine
/// reports the same paint list, the same armed sequence and the same scrub with the wrap in
/// place as without it. (What did break them is the tile renderer's per-VM state — enabling an
/// addon costs a logout and a login, and the pane's model facts did not survive that; see
/// `ui_models`' own tests.)
#[test]
fn a_cooldown_count_addons_hook_leaves_the_sweep_running() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    // The addon loads after FrameXML (`!` sorts it first among addons, all of which run after the
    // interface), so the global it captures is the stock one.
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
    // And it goes cold the reference's way when the cooldown is cleared.
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

/// `!OmniCC` 6.8.30's hook, as shape: it captures `CooldownFrame_SetTimer` in an upvalue, replaces
/// the global with a function that calls through, and hangs its countdown off the BUTTON — a
/// `CreateFrame` parented there lands at `button + 1`, then its own `+ 1` puts it at `button + 2`,
/// one over where it trusts the cooldown to sit. The handle rides a field of the cooldown widget
/// itself (`cd.textFrame`).
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

/// **On the bonus bar the countdown draws over the sweep, as it does on every other bar**.
/// It came in as "the cooldown counter on action bar 1 is hidden behind the pie";
/// a warrior in a stance — a druid in a form, a rogue in stealth — sees the bonus bar there.
///
/// Stock `BonusActionButtonTemplate`'s `OnLoad` raises the button `+2` and then its cooldown `+2`
/// **by hand**, which is only one level of separation because a script level change carries no
/// children (`0x774560` → `set_frame_level(…, propagate=0)`). Our binding carried them, so the
/// cooldown came out at `button + 3` — over the count text the hook hangs at `button + 2`.
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
    // The addon writes its digits from its OnUpdate; the transcription's is a no-op.
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

/// An action button is a TWO-button button (director's report B200: "I can't right
/// click food on my bar to eat it or right click spells"). The ref's `ActionButton_OnLoad`
/// registers `("LeftButtonUp", "RightButtonUp")` (ActionButton.lua:109) and its OnClick body reads
/// no `arg1` — so either button runs the same fork and right-click USES the action. The widget
/// default is `{"LeftButtonUp"}` (`benilla_ui::script::button::wants_click`), so without the
/// explicit registration the input path silently swallowed every right-click on the bar. Driven
/// through the real shipped XML and the real `mouse_button` path, which is the only place the
/// registration set is consulted.
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
            kind: 0x80, // an ITEM action — the food/mount case the report is about
            action: 4540,
            count: 5,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    // Button 1's center (geometry as in the tests above): (26, 22).
    s.mouse_button(26.0, 22.0, "RightButton", true);
    s.mouse_button(26.0, 22.0, "RightButton", false);
    assert_eq!(
        action_ids(&mut s),
        vec![1],
        "right-click queues the same UseAction a left-click does"
    );

    // The middle button is registered by neither the ref nor us: it stays swallowed, which is what
    // proves the assertion above is the REGISTRATION and not the gate having been removed.
    s.mouse_button(26.0, 22.0, "MiddleButton", true);
    s.mouse_button(26.0, 22.0, "MiddleButton", false);
    assert!(
        s.take_action_uses().is_empty(),
        "an unregistered button still reaches nothing"
    );

    // Shift+right-click picks up, exactly as shift+left does — the OnClick fork is button-blind.
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

/// Decision 0216 §7 (byte-verified 0218 §4) driven through the REAL shipped XML, not the engine
/// unit tests directly — the modifier-key mirror gating `PickupAction` vs `UseAction`, end to end.
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

    // Button 1's center (unchanged geometry from the test above): (26, 22).
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

    // A PLAIN click while holding routes through checkCursor=1 to a place — closes the loop back
    // onto the same (now empty) slot: no shift needed once something is already held.
    s.mouse_button(26.0, 22.0, "LeftButton", true);
    s.mouse_button(26.0, 22.0, "LeftButton", false);
    assert!(s.take_action_uses().is_empty(), "routed to place, not use");
    assert!(s.cursor_payload().is_none(), "empty destination clears");
    assert!(s.eval::<bool>("return HasAction(1)").unwrap());
    assert_eq!(s.take_action_sets(), vec![(1, 111)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Lock ActionBars** — the `LOCK_ACTIONBAR` uvar the Options window's Action Bars
/// row and the `TOGGLEACTIONBARLOCK` binding both write, guarding the two drag ends the way the
/// reference does (ActionBarFrame.xml:23-38).
///
/// The teeth are the second half: the reference leaves the shift-click pick-up in `OnClick`
/// UNGUARDED (l.12-22), so a locked bar still yields to the deliberate gesture. Guarding it too
/// would be a "sensible" tightening that silently diverges — and would leave a locked bar with no
/// way to rearrange it at all.
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

    // Shift-click still picks up — the reference's unguarded fork, and the way out of a locked bar.
    s.set_modifiers(true, false, false);
    s.mouse_button(26.0, 22.0, "LeftButton", true);
    s.mouse_button(26.0, 22.0, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(
        s.cursor_payload().is_some(),
        "shift-click is not what the lock stops"
    );

    // The receiving end is guarded too: the held action cannot be dropped back by a drag…
    s.run("this = ActionButton1 ActionButton1:GetScript(\"OnReceiveDrag\")()")
        .unwrap();
    assert!(
        s.cursor_payload().is_some(),
        "a locked slot refuses the drop"
    );
    // …and unlocking makes both ends live again.
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

/// A physical drag from button 1 onto OCCUPIED button 2: the byte-verified action-bar hop (0218
/// §4) — the displaced action lands on the cursor, TWO independent `action_sets` entries across
/// the one gesture ("a drag-swap is two sends, never atomic").
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

    // Button 1 center (26, 22), button 2 center (68, 22) — same geometry as the end-to-end test.
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

/// The Count fontstring, on the reference's own gate (`ActionButton_UpdateCount`, ref
/// ActionButton.lua:285-292): **`IsConsumableAction`**, never "count > 0". The director's report —
/// a mount on the bar wearing a stack number "1" — is the non-consumable case: a mount holds an
/// on-use spell with zero charges and `InventoryType` 0, so `IsConsumableAction 0x4e5250` answers
/// false and the ref paints nothing at all. It repaints on `ACTIONBAR_SLOT_CHANGED` alongside the
/// icon (the same event the identity resolve fires).
///
/// **The gate rides the SLOT, not the state map**. It used to be pushed through
/// `set_action_state`, and this test set that up *before* the repaint — the opposite of the
/// runtime order, where the identity feed fires `ACTIONBAR_SLOT_CHANGED` a whole system before the
/// state feed writes anything. That inversion is why a passing test sat over a fresh character
/// whose food showed no stack number at all. Every push here is now one `set_action`, which is
/// the only order the runtime can produce.
#[test]
fn count_fontstring_follows_is_consumable_action_not_the_bag_count() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);

    // Multi-digit counts, deliberately: the static HotKey labels are single characters
    // ("1".."9","0","-","="), so a single-digit count could false-positive match an unrelated
    // button's hotkey text rather than the Count fontstring actually under test.
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_A".into()),
            kind: 0x00, // SPELL
            action: 111,
            count: 42, // the app never actually sets this for a spell — proves the XML, not the feed
            consumable: false,
        }),
    );
    s.set_action(
        2,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            kind: 0x80, // ITEM — a stack of food
            action: 117,
            count: 15,
            consumable: true,
        }),
    );
    // The report's own shape: an ITEM action the player holds eleven of, which is NOT consumable
    // (a mount). The count is fed all the same; the gate is what must suppress it.
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

    // Read the fontstrings by name, not by scanning painted text: a single-character count
    // ("0") is indistinguishable from a neighbouring button's static HotKey label by content.
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
    // …and it really is painted, not just set (the multi-digit value is unambiguous in the quads).
    assert!(
        s.extract()
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "15")),
        "the consumable's count reaches the screen"
    );

    // Eat the stack down to nothing. A *consumable* keeps its fontstring and reads a literal "0"
    // — the ref's `SetText(GetActionCount(...))` is unconditional inside the gate, and 0216 §7's
    // `count > 0` blank was ours, not the reference's.
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

/// The macro-name line (ref `ActionButton_Update:236-238`, "Update Macro Text") through the REAL
/// shipped XML: a MACRO slot's button reads its macro's name under the icon, a SPELL slot's reads
/// nothing, and a macro slot that empties loses the name through the same unconditional write.
/// B340: the template declared `$parentName` and nothing ever set it, so every
/// macro on the bar was nameless.
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

    // Read the fontstrings by name (the count test's reason: painted text is ambiguous).
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

    // The slot empties: the ref's write is unconditional, so a nil clears the line.
    s.set_action(1, None);
    s.fire_event("ACTIONBAR_SLOT_CHANGED", vec![ScriptValue::Int(1)]);
    s.resolve();
    assert_eq!(name_of(&s, 1), "", "an emptied slot loses the name");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The **bag BAR** — stock `Interface\FrameXML\MainMenuBarBagButtons.xml` — materialized frame
/// for frame, and then driven end to end: the bar's own backpack toggle opens the backpack
/// window, the fed stack paints in its slot's well, the slot's clicks queue the right intents,
/// and the toggle shuts it again. It lives in this file because the bar seats on
/// `MainMenuBarArtFrame` (`ActionBarFrame.xml`) — the toggle's anchor arithmetic below is the
/// reason.
///
/// **What decision 1751 changed.** This asserted `report.frames == 259` over a breakdown that
/// counted five bag WINDOWS and a keyring window (37 + 4×42 + 42, plus the bar's own handful).
/// Those windows are gone: the live ones are the reference's `ContainerFrame1..12`, executed off
/// the player's own patch chain, and the bar is the reference's own
/// `MainMenuBarBagButtons.xml` (1783). So the count is recounted from what that file declares,
/// and the drive reaches the reference's window through the bar's own button rather than showing
/// one of ours by name.
#[test]
fn shipped_bag_frame_drives_end_to_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    use super::test_ui::{bag_open, bag_slot_button, centre_of, load_ui, BAG_UI};
    use benilla_ui::script::{ContainerSlot, ContainerState};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // [`BAG_UI`] is `benilla.toc`'s own order for everything a bag window needs. Three files join
    // it at the positions the manifest gives them:
    //   * ActionBar.xml straight after Cooldown.xml — the bag bar is anchored INTO
    //     MainMenuBarArtFrame, so the bar must exist before the bag bar loads or the toggle's
    //     cross-file `relativeTo` silently falls back to the screen root (which would land in the
    //     right place here anyway, by the 1024-wide coincidence: screen BOTTOMRIGHT == the
    //     full-width bar's art-frame BOTTOMRIGHT — so the failure would be invisible);
    //   * StackSplit.xml and MerchantFrame.xml after the bags — the reference's
    //     `ContainerFrameItemButton_OnClick` reads `StackSplitFrame` on both arms and
    //     `MerchantFrame:IsShown()` on the right one, so a slot click raises without them. That
    //     dependency is the reference's, not ours.
    let mut bar_frames = 0;
    for file in BAG_UI {
        let frames = load_ui(&s, file);
        if *file == "Interface\\FrameXML\\Cooldown.xml" {
            load_ui(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
            load_ui(&s, "Interface\\FrameXML\\TextStatusBar.lua");
            load_ui(&s, "Interface\\FrameXML\\TextStatusBar.xml");
            load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
            load_ui(&s, r"Interface\FrameXML\UIParent.xml");
            load_ui(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
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

    // Nothing open at load: no jerky on screen.
    s.resolve();
    let jerky_visible = |quads: &[benilla_ui::script::ExtractedQuad]| {
        quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("INV_Misc_Food_16"))
        })
    };
    assert!(!jerky_visible(&s.extract()), "no bag window at load");

    // Click the toggle → the backpack window opens and slot 1 paints the jerky. The toggle seats
    // on the bar's art frame BOTTOMRIGHT +(-6,2), 37×37: art frame BOTTOMRIGHT is the bar's
    // (full-width, bottom-anchored) corner (1024,0) ⇒ toggle x[981,1018] y[2,39], center
    // (999.5,20.5). That arithmetic is THIS file's — the button is `MainMenuBarBagButtons.xml`'s
    // and its seat is `ActionBarFrame.xml`'s — so the click stays at literal coordinates rather
    // than going through `centre_of`: hitting them is part of what is being tested.
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
    // WHERE inside the window that well sits is the reference's arithmetic, not ours
    // (`ContainerFrame_GenerateFrame` + `updateContainerFrameAnchors`), so it is asked of the
    // button rather than pinned to numbers this tree no longer owns — and asked by GetID, since
    // the reference numbers its buttons backwards (`…Item1` is the bag's LAST slot). The property
    // is what it always was: the fed stack paints in the well that says it is game slot 1.
    let button = bag_slot_button(&s, 0, 1);
    let (bx, by) = centre_of(&mut s, &button);
    assert!(
        icon.left <= bx && bx <= icon.right && icon.bottom <= by && by <= icon.top,
        "the jerky icon {icon:?} is not painted on slot 1's button ({button} at {bx},{by})"
    );
    // The stack count renders as text.
    assert!(
        quads
            .iter()
            .any(|q| { matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "5") }),
        "stack count shows"
    );

    // LEFT-click picks the item up onto the cursor — a local drag, no wire use queued until a
    // place (ref ContainerFrameItemButton_OnClick's left arm: PickupContainerItem).
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

    // RIGHT-click while holding. **This assertion inverted with 1751, and the inversion is the
    // point of recording it.** Our own `BenillaBagSlot_OnClick` had a cursor-cancel arm — a
    // benilla divergence — so this used to assert that the pickup was cancelled and nothing sent.
    // The reference's right arm has no cursor test at all: it falls through to
    // `UseContainerItem(bag, slot)` like any other right-click, and the cursor keeps its payload.
    // Pinned as what it now IS rather than dropped, so the swap is visible here. Whether the host
    // should treat a use-while-holding as a place is a fidelity question for the bag arc, not
    // this file's to answer.
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

    // RIGHT-click with an empty cursor uses slot 1 (UseContainerItem → the app's use/equip fork).
    s.mouse_button(bx, by, "RightButton", true);
    s.mouse_button(bx, by, "RightButton", false);
    assert_eq!(s.take_container_uses(), vec![(0, 1)]);

    // Toggle again → shut.
    s.mouse_button(999.0, 20.0, "LeftButton", true);
    s.mouse_button(999.0, 20.0, "LeftButton", false);
    s.resolve();
    assert!(!bag_open(&s, 0), "toggle closes the window");
    assert!(!jerky_visible(&s.extract()), "…and its slots with it");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The empty-wells regression (director-reported, 2026-07-10): the bar-level event fan ran
/// UpdateUsable on EMPTY buttons, whose `IsUsableAction` answers (nil, nil) — the 0.4 grey
/// `SetVertexColor` landed on the texture-less icon region and drew a solid grey plate over every
/// empty well. The fix is the ref's own HasAction gate on the fan (ActionButton.lua registers the
/// state handlers only while the button has an action). This drives the exact failing sequence —
/// a usable/cooldown event with empties on the bar — and asserts no empty well gains a solid.
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
    // The failing edges: the fanned usable/cooldown/state events with empties on the bar.
    s.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    s.fire_event("ACTIONBAR_UPDATE_STATE", vec![]);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.resolve();

    // No texture-less colored quad anywhere on the button row (the icon regions of empty wells
    // must stay path-None + color-None); the occupied icon still carries its OOM blue.
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

/// The white-buttons regression (director-reported 2026-08-07): a slot that was
/// OCCUPIED — UpdateUsable painted its icon's 1/1/1 usable tint — then goes EMPTY (the feed's
/// character-switch diff: `set_action(None)` + `ACTIONBAR_SLOT_CHANGED`) kept the tint on the
/// now-artless icon region and drew it as a solid WHITE square. Two laws close it, both asserted
/// here: the empty arm HIDES the icon (ref ActionButton.lua l.168), and the engine emits nothing
/// for a texture-less region whatever its surviving tint (`0x7706e0` — the draw gate is `+0xcc`,
/// never the colour).
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
    // The usable pass paints the occupied icon's 1/1/1 usable tint — the tint that survives.
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
                // Well-sized only, as in the sibling test: page-wide solids are legitimate.
                panic!("the emptied slot draws its surviving tint as a solid plate: {c:?}")
            }
            _ => {}
        }
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The bonus action bar exists and stays hidden** — the same posture 1219 gave the vertical
/// multibars, and the largest single session-start row in the corpus.
///
/// `ref-BonusActionBarFrame.xml` l.54 instantiates `BonusActionBarFrame` as a real
/// `parent="MainMenuBar"` frame carrying `hidden="true"`, with `BonusActionButton1..12` inside.
/// benilla models the bonus page by re-paging the MAIN bar, so we never show this one — but four
/// addons died at `CT_BarMod\CT_BarModOptions.lua:154`,
/// `getglobal("BonusActionButton" .. i):ClearAllPoints()`, which is pure layout and needs only
/// that the buttons be there.
///
/// The last assertion is the one that keeps it honest: declaring a hidden bar must not change what
/// the visible bar shows.
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

    // A hidden bar changes nothing about the visible one. (An EMPTY main-bar button is itself
    // hidden under the reference — `ActionButton_Update` hides a slot with no action while
    // `showgrid == 0`, ActionButton.lua:69-70 — so slot 1 is occupied first.)
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

/// **The reference's action-bar constants are real globals, not comments.**
///
/// `ActionButton.lua:1-9` defines them; this file and `MultiBars.xml` cited them in comments and
/// defined none. An addon reading one got nil — `zBar.lua:40` is the shape,
/// `to = to or value.max or NUM_ACTIONBAR_BUTTONS` feeding a numeric `for`, which raises
/// `'for' limit must be a number`. Only the use-probe could find it: nothing else touches anything.
///
/// `CURRENT_ACTIONBAR_PAGE` is asserted ABSENT on purpose. It is the reference's mutable page
/// cursor and benilla does not page the main bar that way; a frozen 1 would be silently wrong
/// forever, where nil fails loudly. Pinned so a later "completeness" pass cannot quietly add it.
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

    // zBar's exact expression, which raised before these existed.
    assert_eq!(
        s.eval::<i64>("local to = nil or nil or NUM_ACTIONBAR_BUTTONS local n = 0 for i = 1, to do n = n + 1 end return n")
            .unwrap(),
        12,
        "zBar.lua:40's numeric for must have a limit"
    );

    // `CURRENT_ACTIONBAR_PAGE` was asserted ABSENT here, on the grounds that a frozen 1 lies where
    // nil fails loudly. That objection is discharged, not overruled: the bar pages now, so the
    // global is live state the paged-id formula reads rather than a frozen number. It is therefore
    // asserted as state — present, and MOVING — instead of as one of the constants above.
    assert_eq!(s.eval::<i64>("return CURRENT_ACTIONBAR_PAGE").unwrap(), 1);
    s.run("ActionBar_PageUp()").unwrap();
    assert_eq!(
        s.eval::<i64>("return CURRENT_ACTIONBAR_PAGE").unwrap(),
        2,
        "a frozen 1 would still be a lie — this one has to move"
    );
    s.run("ActionBar_PageDown()").unwrap();
}

/// **The reference's two-level action-button split, both halves inheritable by name.**
///
/// `ActionButtonTemplate` (ref ActionButtonTemplate.xml:3) is regions only and carries NO scripts;
/// `ActionBarButtonTemplate` (ref ActionBarFrame.xml:4) inherits it and adds the handlers. Ours
/// conflated them under one `Benilla*` name, so an addon inheriting either reference name got a
/// bare frame — no art, no regions, and no error (1203's silent shape).
///
/// `zBar.xml:7` is the corpus shape: it inherits `ActionBarButtonTemplate`, wires its own OnLoad,
/// and then reads `getglobal(button:GetName().."NormalTexture")` — a derived name it only has
/// because the template declares `$parentNormalTexture`. That read is where it died.
///
/// The last assertion is the one that keeps our own bars safe: the alias must still resolve to the
/// full thing, or 48 `inherits=` sites across four files silently lose their handlers.
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

    // The base half carries NO handlers, exactly as the reference's does — an addon inheriting it
    // wires its own, and must not silently receive ours.
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

    // ...and our own alias still resolves to the full thing.
    assert!(
        s.eval::<bool>("return ActionButton1NormalTexture ~= nil and ActionButton1:GetScript(\"OnClick\") ~= nil")
            .unwrap(),
        "BenillaActionButtonTemplate's 48 inherits= sites must be untouched by the split"
    );
}

/// Main-bar paging — `CURRENT_ACTIONBAR_PAGE` and the three verbs around it.
///
/// The data was always there (the app owns all 120 action slots); only the selector was missing,
/// and its absence was visible on screen as page arrows with no `OnClick`. `Bartender2.lua:686`
/// died on the nil `ChangeActionBarPage` at session start.
///
/// Two things are asserted that a reconstruction would get wrong. **A bonus page outranks the
/// paged one** — the reference's own `ActionButton_GetPagedID` takes the bonus branch first, so
/// paging must be the `else` arm and not an addition. And **page-up wraps to the literal page 1**
/// while page-down rescans for the last viewable page: that asymmetry is the reference's, and it
/// is observable the moment a page is blanked from `VIEWABLE_ACTION_BAR_PAGES`.
///
/// Since 1500 the blanking is driven here the way the client drives it — by raising the two bottom
/// multibars — rather than read off a declaration. All six pages are viewable at rest now, because
/// every extra bar ships off and nothing has claimed a page yet.
#[test]
fn the_main_bar_pages_and_a_bonus_page_still_outranks_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in [
        // Fonts first: the pane's check-box labels colour from `RED_FONT_COLOR` in their own OnLoad.
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
        // `UIOptionsFrame_Init`'s uvars and `UIOptionsFrameCheckButtons`, which
        // `MultiActionBars.xml` below writes into at its load — the reference's own l.21 seat,
        // ahead of our window and ahead of the bars (2115).
        r"Interface\FrameXML\OptionsFrame.lua",
        r"Interface\FrameXML\UIOptionsFrame.xml",
        "OptionsFrame.xml",
        "Interface\\FrameXML\\MultiActionBars.xml",
    ] {
        // `test_ui::load_ui`, not a disk read: this list names chain entries now (the reputation
        // pane and the templates it inherits through), and `assets/ui` cannot answer for those.
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
    // The NUMERAL is the only output of paging the player can actually see on the bar itself, and
    // it went unwritten for as long as paging existed: the arrows worked, the twelve buttons
    // repainted, and the "1" beside them was a declared literal nothing ever touched — so clicking
    // them read as doing nothing at all (director, 2026-08-22). It seeds at the page's own value.
    let page_text = |s: &UiScript| {
        s.eval::<String>("return MainMenuBarPageNumber:GetText()")
            .unwrap()
    };
    assert_eq!(page_text(&s), "1", "the load seed is the current page");

    // Raise the two bottom bars the way the client does, which is what takes pages 6 and 5 out of
    // the cycle from here on.
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

    // Down again, and below page 1 it rescans to the LAST viewable page.
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
    // The pages the bottom bars already display are unreachable from the main bar — which is
    // exactly what MultiActionBar_Update did above. Without it, paging up lands on a duplicate of
    // the twelve actions already on screen below.
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

    // The bonus branch is GUARDED by the page (ActionButton.lua:447: `button.isBonus and
    // CURRENT_ACTIONBAR_PAGE == 1`) — and a MAIN-bar button has no isBonus at all, so it never
    // takes that branch: the bonus offset is BonusActionBarFrame's own twelve buttons' business
    // (1897, adopted with 1938). Main-bar slot 1 on page 3 is action 25 whatever the offset.
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

/// The form/stance/stealth swap transition (ref BonusActionBarFrame.lua:1-98).
/// Entering a form slides the BonusActionBarFrame replica up over 0.15s — the main bar keeps
/// painting the OLD page underneath until the landing, which is also the one moment the sound
/// (igBonusBarOpen) plays. The overlay then STAYS shown while the form holds (the ref-visible
/// state addons read), a direct form→form swap repaints without re-sliding or re-sounding, and
/// dropping the form slides it back down carrying the old form's page (lastBonusBar), silently.
/// Keys route to the overlay's buttons from the FIRST slide frame — the swap's feel half: a key
/// pressed the instant you enter stealth already drives the stealth page.
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

    // `ExhaustionTick_Update` indexes `ReputationWatchBar` UNGUARDED — the reference's own code,
    // safe there because `ReputationFrame.xml` is always loaded and always declares the bar. Since
    // 1875 that file is the reference's own, so a harness that drives the XP bar has to load it too
    // or the tick raises on its first event.
    // In manifest order: the fonts its check-box labels colour from, the panel templates those
    // boxes inherit through, then the pane.
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

    // ── Enter cat form: offset 0→1, the app's edge (ui_action/feed.rs) fires the event ─────────
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

    // Keys route to the overlay immediately (ref ActionButton.lua:15-45's IsShown fork).
    s.run("ActionButtonDown(1) ActionButtonUp(1)").unwrap();
    assert_eq!(
        action_ids(&mut s),
        vec![73],
        "a key pressed mid-slide already drives the bonus page"
    );

    // …and the same button with the SELF-CAST modifier. `SELFACTIONBUTTON1`-`12` (`ALT-1`…`ALT-=`)
    // are `ActionButtonUp(id, 1)` and nothing else, so this is the whole of what those twelve
    // bindings do that the plain twelve do not (1745).
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

    // The slide, frame by frame. Stock `BonusActionBar_OnUpdate` paints the position the timer
    // has REACHED and then advances it (BonusActionBarFrame.lua:37-49), so the first OnUpdate
    // after a Show paints the start (top = 0 over the bar's bottom edge at y=0), the second the
    // half-risen replica (0.5 * 43), and the one that finds the timer past BONUSACTIONBAR_SLIDETIME
    // lands it. Silent until then.
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

    // The landing edge: snap to 43 and THE sound. The main bar does NOT adopt the bonus page:
    // a main-bar button has no `isBonus`, so `ActionButton_GetPagedID` keeps it on its page and the
    // overlay IS the stance page (ActionButton.lua:447-456; 1897, adopted with 1938).
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

    // ── A direct form→form swap (stance dance, powershift): repaint, no slide, no sound ────────
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

    // ── Drop the form: the overlay descends carrying the OLD form's page, silently ─────────────
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
    // A key mid-descent still drives the old form's page — ref GetPagedID's lastBonusBar
    // stand-in while the frame is still shown.
    s.run("ActionButtonDown(1) ActionButtonUp(1)").unwrap();
    assert_eq!(action_ids(&mut s), vec![97]);
    // Paint-then-advance again: the frame that finds the timer past the slide time is the one
    // that hides (BonusActionBarFrame.lua:51-66), so the descent takes one OnUpdate more than
    // the time itself.
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

/// A form flip mid-slide turns the bar around from where it is (our progress fraction), rather
/// than the ref's timer arithmetic, which mirror-jumps the position — the mechanism, not the
/// quirk (1524).
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

    // `ExhaustionTick_Update` indexes `ReputationWatchBar` UNGUARDED — the reference's own code,
    // safe there because `ReputationFrame.xml` is always loaded and always declares the bar. Since
    // 1875 that file is the reference's own, so a harness that drives the XP bar has to load it too
    // or the tick raises on its first event.
    // In manifest order: the fonts its check-box labels colour from, the panel templates those
    // boxes inherit through, then the pane.
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\OptionsFrameTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\ReputationFrame.xml");
    s.tick(10.0);
    let _ = s.take_sounds();

    // Up to half height, then the form drops mid-slide.
    s.set_bonus_bar_offset(1);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    s.tick(0.075);
    s.set_bonus_bar_offset(0);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    assert_eq!(
        s.eval::<String>("return BonusActionBarFrame.mode").unwrap(),
        "hide"
    );
    // A third of the way back down from the turnaround point: 43 * (0.5 - 0.03/0.15).
    // `HideBonusActionBar` keeps a running timer (it resets it only when `completed`,
    // BonusActionBarFrame.lua:86-88), so the descent starts where the rise had got to: the rise's
    // one OnUpdate advanced the timer to 0.075 (half), and the hide arm paints (1 − 0.075/0.15) · 43
    // = 21.5 on its first frame, then 12.9 on the next — paint, then advance.
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

/// **The two page arrows must not sit on top of each other.**
///
/// They are 32x32 squares stacked only 20 px apart, so their raw frame rects overlap by 12 px —
/// and the hit-test walks the draw order in reverse, so the later-declared DOWN button owned that
/// band: the bottom third of the visible UP arrow paged the bar the wrong way. The reference's
/// `<HitRectInsets>` (±6 horizontal, ±7 vertical) shrink each square to the 20x18 arrow it
/// actually draws, which separates them — the insets are behaviour here, not decoration.
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

    // `ExhaustionTick_Update` indexes `ReputationWatchBar` UNGUARDED — the reference's own code,
    // safe there because `ReputationFrame.xml` is always loaded and always declares the bar. Since
    // 1875 that file is the reference's own, so a harness that drives the XP bar has to load it too
    // or the tick raises on its first event.
    // In manifest order: the fonts its check-box labels colour from, the panel templates those
    // boxes inherit through, then the pane.
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\OptionsFrameTemplates.xml");
    super::test_ui::load_ui(&s, r"Interface\FrameXML\ReputationFrame.xml");
    // The post-login state, which is what a player clicks into. Without it `ExhaustionTick_Update`
    // never runs, and the rested marker — DIALOG strata, declared CENTER on the XP strip, which is
    // exactly where the arrows are — sits unhidden over both and eats every click. That is the
    // reference's own declaration (`hidden="false"`, ref-MainMenuBar.xml l.415), hidden at runtime
    // when `GetXPExhaustion()` is nil, so firing the event is the harness's job, not a fix.
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

    // The contested band, measured rather than assumed: the two 32x32 squares are 20 px apart, so
    // raw they share the 12 px above the UP button's bottom edge. 8 px up from that edge is inside
    // the up arrow's own drawn art AND inside the down button's raw square — the exact pixel the
    // player aims at and the exact pixel the later-declared down button used to win.
    //
    // With the ref's ±7 vertical insets the two hit rects become disjoint (up keeps its top 18 px,
    // down its own), so this point resolves UP, which is what the arrow under the cursor says.
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
