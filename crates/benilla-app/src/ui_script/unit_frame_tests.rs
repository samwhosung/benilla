use benilla_ui::script::{
    QuadContent, ScriptValue, SelectionRequest, SoundRequest, UiScript, UnitState,
};

use super::test_ui::{hover, load_ui as load_xml, unhover};

/// The unit frames' production load prefix (ui_script/mod.rs order): fonts + UIParent +
/// tooltip, then the dropdown kit + unit popups the frames' DropDown children initialize into.
fn load_unit_frames(s: &UiScript) {
    // The app runs the real `Interface\FrameXML\GlobalStrings.lua` off the player's chain BEFORE
    // any XML (`load_global_strings`), so the fixture names it too. This used to be a single
    // hand-set `DEAD = "Dead"`, which was enough while the frames were OUR transcription: ours
    // carried `X = X or "…"` fallbacks and hard literals for everything else. The reference's own
    // files carry none, and they resolve GlobalStrings at LOAD in three separate places —
    // `CombatFeedback.lua` l.7-17 builds the whole `CombatFeedbackText` table out of them
    // (`TEXT(ABSORB)`, `TEXT(MISS)`, …), `UnitFrame.lua` l.1-6 builds `ManaBarColor`'s prefixes
    // the same way, and `UnitFrame_OnEnter` l.60/63 passes `PARTY_OPTIONS_LABEL` /
    // `PLAYER_OPTIONS_LABEL` straight into `GameTooltip:SetText`, which raises on nil rather than
    // drawing an empty plate. Hand-setting the union of those is a second copy of the reference's
    // own file; naming the file is the only version that cannot drift. (`DEAD` is l.898 of it.)
    load_xml(s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(s, r"Interface\FrameXML\UIParent.xml");
    // The bars' numerals machinery, which the manifest loads immediately ahead of
    // UnitFrames.xml and which every bar's OnLoad wires into since 1143.
    load_xml(s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(s, "Interface\\FrameXML\\GameTooltip.xml");
    // `FACTION_BAR_COLORS` for the stock `GameTooltip_UnitColor` — ReputationFrame.lua's (1968).
    load_xml(s, r"Interface\FrameXML\ReputationFrame.lua");
    load_xml(s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(s, "Interface\\FrameXML\\BasicControls.xml"); // `TEXT`, which UnitPopup.lua reads at file scope
    load_xml(s, "Interface\\FrameXML\\UnitPopup.xml");
    load_xml(s, "Interface\\FrameXML\\BuffFrame.xml");
    load_xml(s, "Interface\\FrameXML\\UnitFrame.xml");
    load_xml(s, "Interface\\FrameXML\\CombatFeedback.xml");
    load_xml(s, "Interface\\FrameXML\\PlayerFrame.xml");
    load_xml(s, "Interface\\FrameXML\\PartyFrame.xml");
    load_xml(s, "Interface\\FrameXML\\TargetFrame.xml");
    load_xml(s, "Interface\\FrameXML\\PetFrame.xml");
}

/// Load the real `assets/ui/UnitFrames.xml` (the shipped default UI) into a bare engine and
/// drive it with synthetic snapshots — the whole slice-1 chain minus Bevy: template expansion,
/// StatusBar fill, the Era event set, the target frame's hide/show lifecycle, and the async
/// name arriving via UNIT_NAME_UPDATE.
#[test]
fn shipped_unit_frames_drive_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // **Only the TARGET frame hides.** Our deleted transcription hid both; the reference hides
    // exactly one. `PlayerFrame` is a plain top-level `<Button>` with no `hidden=` attribute
    // (ref PlayerFrame.xml l.4) and `PlayerFrame_Update` (ref PlayerFrame.lua l.29-37) wraps its
    // whole body in `if UnitExists("player")` without an else — nothing in the file ever calls
    // `PlayerFrame:Hide()`. That is the real client's behaviour: the player plate is up from the
    // moment the UI loads, empty until the player object arrives. `TargetFrame_Update`
    // (ref TargetFrame.lua l.38-55) is the one with the `else this:Hide()`.
    let shape: bool = s
        .eval("return PlayerFrame:IsVisible() and not TargetFrame:IsVisible()")
        .unwrap();
    assert!(
        shape,
        "the player plate is always up; only the target frame hides while its unit is absent"
    );

    // The player appears (name still unresolved), at 72/100 health, 45/80 mana.
    //
    // `PLAYER_ENTERING_WORLD` is the event, not `UNIT_HEALTH`, and the reference is why: the
    // stock bars each register their OWN events and repaint only themselves
    // (`UnitFrameHealthBar_Initialize` takes UNIT_HEALTH/UNIT_MAXHEALTH, ref UnitFrame.lua
    // l.150-151; `UnitFrameManaBar_Initialize` takes the ten UNIT_MANA/RAGE/… ones, l.189-199),
    // so UNIT_HEALTH alone would leave the mana bar at its load-time 0/0. The one handler that
    // repaints name + portrait + both bars together is `PlayerFrame_OnEvent`'s
    // PLAYER_ENTERING_WORLD arm → `UnitFrame_Update()` (ref PlayerFrame.lua l.96-100), which is
    // also what really fires when the player object arrives.
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: None,
            health: 72,
            max_health: 100,
            level: 12,
            power_type: 0,
            power: 45,
            max_power: 80,
            dead: false,
            reaction: 0,
            // Not decoration: `UnitFrameManaBar_Update`'s disconnect leg (ref UnitFrame.lua
            // l.209-212) pins a disconnected unit's power bar to MAX and greys it, and
            // `UnitState::default()` leaves this `false`.
            is_connected: true,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    assert!(s.eval::<bool>("return PlayerFrame:IsVisible()").unwrap());
    let ok: bool = s
        .eval(
            r#"
            local hb, pb = PlayerFrameHealthBar, PlayerFrameManaBar
            local _, hmax = hb:GetMinMaxValues()
            local _, pmax = pb:GetMinMaxValues()
            local r, g, b = pb:GetStatusBarColor()
            return hb:GetValue() == 72 and hmax == 100
               and pb:GetValue() == 45 and pmax == 80 and pb:IsVisible()
               and b == 1 and r == 0 -- mana blue
        "#,
        )
        .unwrap();
    assert!(ok, "player frame painted from the snapshot");

    // …and the name plate is BLANK while the name is unresolved, not the word "Unknown". That
    // word was our transcription's, twice over — a `text="Unknown"` literal on the FontString
    // (deleted UnitFrames.xml l.1364) and a `UnitName(unit) or "Unknown"` fallback (l.803). The
    // reference's `PlayerName` carries no `text=` at all (ref PlayerFrame.xml l.58) and
    // `GetUnitName` returns `UnitName`'s result unchanged (ref UnitFrame.lua l.226-236), so what
    // shows is whatever the engine's `UnitName` returns for a nameless unit — nil here.
    assert_eq!(
        s.eval::<Option<String>>("return PlayerName:GetText()")
            .unwrap(),
        None,
        "no name yet: the stock file has no \"Unknown\" literal to fall back on"
    );

    // The name-query answer lands: UNIT_NAME_UPDATE repaints the name.
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Benilla".into()),
            health: 72,
            max_health: 100,
            level: 12,
            power_type: 0,
            power: 45,
            max_power: 80,
            dead: false,
            reaction: 0,
            is_connected: true,
            ..UnitState::default()
        }),
    );
    s.fire_event("UNIT_NAME_UPDATE", vec![ScriptValue::Str("player".into())]);
    assert_eq!(
        s.eval::<String>("return PlayerName:GetText()").unwrap(),
        "Benilla"
    );

    // A powerless wolf gets targeted: frame shows, power bar runs EMPTY, health fills 30/50.
    //
    // "Empty", not hidden — and the reason is a reference quirk worth knowing. The only thing in
    // 1.12 that hides a StatusBar for having no track is `TextStatusBar_UpdateTextString`'s
    // `else textStatusBar:Hide()` (ref TextStatusBar.lua l.55-57), and that whole body sits
    // inside `if(string)` — `string` being `bar.TextString`. The reference **never declares**
    // `TargetFrameHealthBarText` / `TargetFrameManaBarText`: TargetFrame.xml l.486-487 passes
    // both names into `UnitFrame_Initialize` and neither exists anywhere in FrameXML (only the
    // player's and the pet's do — PlayerFrame.xml l.79/88, PetFrame.xml l.87/96). So
    // `SetTextStatusBarText` early-returns on the nil (ref TextStatusBar.lua l.7-10), the
    // target's bars carry no `TextString`, and the hide can never fire. It doesn't show: these
    // bars have a `<BarTexture>` and no background, so a 0/0 bar draws nothing either way.
    //
    // Our transcription hid it, because 1146 §3 *declared* the two text regions the reference
    // leaves dangling. Those are gone with the file.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Young Wolf".into()),
            health: 30,
            max_health: 50,
            level: 3,
            power_type: 0,
            power: 0,
            max_power: 0,
            dead: false,
            reaction: 4, // neutral
            is_connected: true,
            ..UnitState::default()
        }),
    );
    s.take_sounds(); // drain anything earlier; the target select/deselect pair is under test below
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let ok: bool = s
        .eval(
            r#"
            local _, pmax = TargetFrameManaBar:GetMinMaxValues()
            return TargetFrame:IsVisible()
               and TargetFrameManaBar:GetValue() == 0 and pmax == 0
               and TargetFrameManaBar.TextString == nil -- the reference declares no text region
               and TargetName:GetText() == "Young Wolf"
               and TargetLevelText:GetText() == "3"
               and not TargetDeadText:IsShown() -- living target: no dead word
        "#,
        )
        .unwrap();
    assert!(
        ok,
        "target frame painted; a powerless unit's power bar runs empty over an empty track"
    );
    // The select sound rides the frame's OnShow (ref TargetFrame_OnShow): a neutral (4) wolf is
    // neither UnitIsEnemy (≤2) nor UnitIsFriend (≥5) → the neutral kit.
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCreatureNeutralSelect".into())],
        "neutral target select kit"
    );

    // Neutral reaction (4) tints the name plate yellow — UnitReactionColor[4] = (1,1,0), the
    // faithful TargetFrame_CheckFaction path (the plate was untinted before). Assert on the extracted
    // quad's vertex color (same style as the bar-fill check below).
    s.resolve();
    let plate = s
        .extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Texture {
                path: Some(p),
                color: Some(c),
                ..
            } if p.contains("LevelBackground") => Some(c),
            _ => None,
        })
        .expect("target name-plate quad present");
    assert!(
        (plate[0] - 1.0).abs() < 1e-6 && (plate[1] - 1.0).abs() < 1e-6 && plate[2].abs() < 1e-6,
        "neutral name plate is yellow, got {plate:?}"
    );

    // The target's health-bar fill quad is 60% of the bar width (30/50 of the real 119px bar,
    // ref-TargetFrame.xml l.253-255).
    s.resolve();
    let quads = s.extract();
    let bar_rect = quads
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("UI-StatusBar"))
        })
        .filter_map(|q| q.rect)
        .find(|r| (r.width() - 119.0 * 0.6).abs() < 0.01)
        .expect("target health fill at 60% of 119px");
    assert!((bar_rect.width() - 71.4).abs() < 0.01);

    // Deselect: the target frame hides again, playing the lost-target kit (ref TargetFrame_OnHide).
    s.set_unit("target", None);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(!s.eval::<bool>("return TargetFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName(
            "INTERFACESOUND_LOSTTARGETUNIT".into()
        )],
        "deselect plays the lost-target kit"
    );

    // A hostile (reaction 2) target takes the UnitIsEnemy branch — the aggro kit.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Kobold Vermin".into()),
            health: 40,
            max_health: 40,
            level: 1,
            reaction: 2,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(s
        .eval::<bool>("return UnitIsEnemy(\"target\", \"player\") == 1")
        .unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCreatureAggroSelect".into())],
        "hostile target select kit"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The hit indicator's drawn height via the engine extract: the `SetTextHeight` override on the
/// Text quad whose text matches (the two-regime split, decision 0582 — GetFont keeps reporting
/// the font object's own 30, like the real API).
fn extracted_text_height(s: &mut UiScript, text: &str) -> Option<f32> {
    s.resolve();
    s.extract().into_iter().find_map(|q| match q.content {
        QuadContent::Text {
            text: Some(t),
            text_height,
            ..
        } if t == text => Some(text_height),
        _ => None,
    })?
}

/// The portrait hit indicator: `UNIT_COMBAT` over `"player"` drives the
/// transcribed CombatFeedback — a physical wound paints the amount white at the base height 30,
/// a spell crit paints yellow at ×1.5, a full absorb paints the word at ×0.75 — and the fade
/// envelope (0.2 s in, 0.7 s hold, 0.3 s out) ends in a Hide. A `"target"`-token event never
/// touches it (only the player frame registers UNIT_COMBAT in 1.12).
#[test]
fn unit_combat_drives_the_player_hit_indicator() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            health: 72,
            max_health: 100,
            ..UnitState::default()
        }),
    );
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);

    let ev = |unit: &str, action: &str, flags: &str, amount: i64, school: i64| {
        vec![
            ScriptValue::Str(unit.into()),
            ScriptValue::Str(action.into()),
            ScriptValue::Str(flags.into()),
            ScriptValue::Int(amount),
            ScriptValue::Int(school),
        ]
    };

    // A physical wound: the amount, white, base height.
    s.fire_event("UNIT_COMBAT", ev("player", "WOUND", "", 17, 0));
    let ok: bool = s
        .eval(
            r#"
            local ind = PlayerHitIndicator
            return ind:IsShown() ~= nil and tostring(ind:GetText()) == "17"
        "#,
        )
        .unwrap();
    assert!(ok, "physical wound paints the amount ({:?})", s.errors());
    assert_eq!(
        extracted_text_height(&mut s, "17"),
        Some(30.0),
        "base height 30 (the SetTextHeight regime)"
    );

    // A spell crit: ×1.5 height (the CRITICAL arm), and the type>0 yellow.
    s.fire_event("UNIT_COMBAT", ev("player", "WOUND", "CRITICAL", 64, 4));
    let ok: bool = s
        .eval("return tostring(PlayerHitIndicator:GetText()) == \"64\"")
        .unwrap();
    assert!(ok, "spell crit paints ({:?})", s.errors());
    assert_eq!(
        extracted_text_height(&mut s, "64"),
        Some(45.0),
        "×1.5 crit height, UNCAPPED past 32 (decision 0582's regime split)"
    );

    // A full absorb: the word at ×0.75.
    s.fire_event("UNIT_COMBAT", ev("player", "WOUND", "ABSORB", 0, 0));
    let ok: bool = s
        .eval("return PlayerHitIndicator:GetText() == \"Absorb\"")
        .unwrap();
    assert!(ok, "full absorb paints the word ({:?})", s.errors());
    assert_eq!(
        extracted_text_height(&mut s, "Absorb"),
        Some(22.5),
        "the word at ×0.75"
    );

    // The envelope: mid-hold the text is fully opaque; past 1.2 s it hides.
    s.tick(0.5); // 0.2 fade-in + into the hold
    let ok: bool = s
        .eval(
            r#"
            local ind = PlayerHitIndicator
            return ind:IsShown() ~= nil and ind:GetAlpha() == 1.0
        "#,
        )
        .unwrap();
    assert!(ok, "mid-hold: opaque ({:?})", s.errors());
    s.tick(0.8); // past fade-in + hold + fade-out (1.2 s total)
    assert!(
        s.eval::<bool>("return PlayerHitIndicator:IsShown() == nil")
            .unwrap(),
        "the envelope ends in a Hide ({:?})",
        s.errors()
    );

    // A target-token event leaves the (hidden) indicator alone.
    s.fire_event("UNIT_COMBAT", ev("target", "WOUND", "", 99, 0));
    assert!(
        s.eval::<bool>("return PlayerHitIndicator:IsShown() == nil")
            .unwrap(),
        "a target event never touches the player indicator"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Left-clicking the player unit frame targets yourself — the faithful self-target path (ref
/// `PlayerFrame_OnClick` → `TargetUnit("player")`). Drives the real hit-test path (a press + release
/// on the frame's centre fires its `OnClick`) against the shipped `UnitFrames.xml`, and asserts the
/// `TargetUnit` request the app drains and commits. A right-click opens the SELF unit popup
/// (decision 0434 phase 5) — nothing solo (only CANCEL survives the gates, the ref shows no
/// menu), the full leader set once a party is pushed.
#[test]
fn left_clicking_the_player_frame_targets_self() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The SELF-menu strings the popup rows bake at UnitPopup.xml load arrive with
    // `load_unit_frames`' `GlobalStrings.lua`; they used to be hand-set here, and one of the
    // hand-set values was wrong (see the row assertion below).
    load_unit_frames(&s);

    // The player must exist for the frame to be shown and mouse-hittable.
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            health: 72,
            max_health: 100,
            ..UnitState::default()
        }),
    );
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    s.resolve();
    assert!(
        s.take_selection_requests().is_empty(),
        "no request before any click"
    );

    // Click the frame's centre through the real hit-test path (press + release on the same frame).
    let (cx, cy) = s
        .eval::<(f64, f64)>("return PlayerFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    assert!(
        s.take_selection_requests().is_empty(),
        "right-click queues no target"
    );
    // Solo, every SELF row is gated off (only CANCEL survives) — the ref opens no menu. 1.12 has
    // no PvP row here and neither do we (decision 0652 took 0646's added row back out).
    assert!(
        s.eval::<bool>("return not DropDownList1:IsVisible()")
            .unwrap(),
        "no SELF menu while solo"
    );

    s.mouse_button(cx as f32, cy as f32, "LeftButton", true);
    s.mouse_button(cx as f32, cy as f32, "LeftButton", false);
    assert_eq!(
        s.take_selection_requests(),
        vec![SelectionRequest::Unit("player".into())],
        "left-click queues a self-target"
    );

    // Grouped and leading: the same right-click opens the SELF popup — title (our name),
    // Loot Method + Loot Threshold (nested), Leave Party, Raid Target Icon (nested), Cancel.
    s.set_party(benilla_ui::script::PartyState {
        members: vec![benilla_ui::script::PartyMemberInfo {
            name: "Alice".into(),
            guid: 0xA11CE,
        }],
        leader_index: 0, // we lead
        // The player's own guid, which this fixture leaves unset — spelled out because a bare 0
        // is also the reference's "ungrouped" sentinel and this party has a member.
        leader_guid: 0,
        own_guid: 0,
        raid: Vec::new(),
        loot_method: "group".into(),
        master_looter: None,
        loot_threshold: 2,
    });
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "the SELF menu opens for a party leader"
    );
    assert_eq!(
        s.eval::<i64>("return DropDownList1.numButtons").unwrap(),
        6,
        "title + Loot Method + Loot Threshold + Leave Party + Raid Target Icon + Cancel"
    );
    // `PARTY_LEAVE`, verbatim. The reference's own GlobalStrings.lua l.2991 is
    // `PARTY_LEAVE = "Leave party"` — lower-case "party". The hand-set fixture this test used to
    // open with title-cased it, so the row read back the fixture's own typo rather than the
    // client's string; naming the real file is what exposed it.
    assert_eq!(
        s.eval::<String>("return DropDownList1Button4:GetText()")
            .unwrap(),
        "Leave party"
    );
    // The nested rows carry the expand arrow (the level-2 gate for a leader).
    assert!(
        s.eval::<bool>("return DropDownList1Button2ExpandArrow:IsVisible()")
            .unwrap(),
        "Loot Method is nested for the leader"
    );
    // Clicking Leave Party through the real hit path fires UnitPopup_OnClick → LeaveParty()
    // and closes the list (not keepShownOnClick).
    s.resolve();
    let (rx, ry) = s
        .eval::<(f64, f64)>("return DropDownList1Button4:GetCenter()")
        .unwrap();
    s.mouse_button(rx as f32, ry as f32, "LeftButton", true);
    s.mouse_button(rx as f32, ry as f32, "LeftButton", false);
    assert_eq!(
        s.take_party_requests(),
        vec![benilla_ui::script::PartyRequest::Leave],
        "Leave Party queues the leave intent"
    );
    assert!(
        s.eval::<bool>("return not DropDownList1:IsVisible()")
            .unwrap(),
        "the click closes the list"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The nested level-2 list end-to-end (the 6a suspect path): a leader right-clicks the player
/// frame, hovers Raid Target Icon — a `hasArrow` row's OnEnter is what opens `DropDownList2` —
/// and clicks Skull through the real hit path. The mark intent must queue against the menu's
/// unit. The level-1 click was pinned above; this pins the level the marks actually live on.
#[test]
fn raid_mark_clicks_through_the_nested_level() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The menu strings arrive with `load_unit_frames`' own `GlobalStrings.lua`
    // (`RAID_TARGET_ICON` l.3288, `RAID_TARGET_1..8` l.3280-3287, `NONE` l.2795), which is where
    // production reads them; they used to be hand-set here.
    load_unit_frames(&s);

    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            health: 72,
            max_health: 100,
            ..UnitState::default()
        }),
    );
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    s.set_party(benilla_ui::script::PartyState {
        members: vec![benilla_ui::script::PartyMemberInfo {
            name: "Alice".into(),
            guid: 0xA11CE,
        }],
        leader_index: 0, // we lead — the mark rows are leader-gated
        leader_guid: 0,  // the player's own guid; this fixture leaves it unset
        own_guid: 0,
        raid: Vec::new(),
        loot_method: "group".into(),
        master_looter: None,
        loot_threshold: 2,
    });
    s.resolve();

    let (cx, cy) = s
        .eval::<(f64, f64)>("return PlayerFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "the SELF menu opens"
    );
    s.resolve();

    // Hover the nested row through the real pointer path.
    assert_eq!(
        s.eval::<String>("return DropDownList1Button5:GetText()")
            .unwrap(),
        "Raid Target Icon"
    );
    let (rx, ry) = s
        .eval::<(f64, f64)>("return DropDownList1Button5:GetCenter()")
        .unwrap();
    s.mouse_move(rx as f32, ry as f32);
    assert!(
        s.eval::<bool>("return DropDownList2:IsVisible()").unwrap(),
        "hovering the nested row opens level 2"
    );
    assert_eq!(
        s.eval::<i64>("return DropDownList2.numButtons").unwrap(),
        9,
        "the eight marks + None"
    );
    s.resolve();

    // Click Skull (row 8) through the real hit path.
    assert_eq!(
        s.eval::<String>("return DropDownList2Button8:GetText()")
            .unwrap(),
        "Skull"
    );
    let (sx, sy) = s
        .eval::<(f64, f64)>("return DropDownList2Button8:GetCenter()")
        .unwrap();
    s.mouse_button(sx as f32, sy as f32, "LeftButton", true);
    s.mouse_button(sx as f32, sy as f32, "LeftButton", false);
    // `UnitPopup.xml`'s row calls `SetRaidTargetIcon(menu.unit, mark)`; the definition of that
    // used to be ours, and since the migration it is the reference's own — `TargetFrame.lua`
    // l.486-492 — whose whole body is `SetRaidTarget(unit, 0 or index)`. That is the ENGINE
    // verb, built by 1820 (`benilla-ui` `script/party.rs`), never the wrapper's name on our
    // body: this asserts the intent it queues, and `:638` asserts nothing raised.
    assert_eq!(
        s.take_party_requests(),
        vec![benilla_ui::script::PartyRequest::SetRaidTarget {
            unit: "player".into(),
            index: 8
        }],
        "Skull queues the mark intent for the menu's unit"
    );
    // Ref law: the click hides only the row's OWN list (UIDropDownMenuButton_OnClick's
    // `this:GetParent():Hide()`; DropDownList1's OnHide closes level 2, never the reverse).
    // Level 1 lingers and dies by the 2s show-timer once the pointer leaves the chain.
    assert!(
        s.eval::<bool>("return not DropDownList2:IsVisible() and DropDownList1:IsVisible()")
            .unwrap(),
        "the click closes its own level; level 1 lingers (ref)"
    );
    // Two ticks: the ref's OnUpdate hides only on the frame AFTER the timer crosses zero.
    s.mouse_move(5.0, 5.0);
    s.tick(2.1);
    s.tick(0.1);
    assert!(
        s.eval::<bool>("return not DropDownList1:IsVisible()")
            .unwrap(),
        "level 1 times out 2s after the pointer leaves the chain"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The level slot's transcribed CheckLevel law (ref-TargetFrame.lua l.119-142) end to end over
/// the REAL `GetDifficultyColor` (ref-QuestLogFrame.lua l.14-20 + l.584-599, loaded from the
/// shipped QuestLogFrame.xml — its ref home) and `UnitLevel`'s −1 return: an attackable target
/// difficulty-colors its number; a hostile 10+ levels up (UnitLevel −1) or a corpse swaps the
/// number for the HighLevelTexture skull; the green→grey boundary rides the real
/// GetQuestGreenRange binding.
#[test]
fn shipped_target_frame_runs_the_level_law() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::PlayerReqState;
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    // GetDifficultyColor's own load chain (the quest log window, its ref home).
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\CharacterFrameTemplates.xml"); // the window tab (1993)
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");
    // The player at level 3, both feeds (the snapshot UnitLevel("player") reads; the req state
    // the −1 gate and GetQuestGreenRange read) — the app keeps the two in step.
    s.set_player_req_state(PlayerReqState {
        level: 3,
        ..Default::default()
    });
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level: 3,
            is_player: true,
            player_controlled: true,
            health: 50,
            max_health: 50,
            ..UnitState::default()
        }),
    );

    // An attackable boar 5 up (the screenshot's case at the director's level): the number shows,
    // impossible-red via the real table; no skull.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Elder Mottled Boar".into()),
            health: 40,
            max_health: 40,
            level: 8,
            reaction: 4,
            can_attack: true,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let ok: bool = s
        .eval(
            r#"
            local lvl = getglobal("TargetLevelText")
            local skull = getglobal("TargetHighLevelTexture")
            local c = GetDifficultyColor(8)
            return lvl:IsShown() ~= nil and skull:IsShown() == nil
               and tostring(lvl:GetText()) == "8"
               and c.r == 1.00 and c.g == 0.10 and c.b == 0.10
        "#,
        )
        .unwrap();
    assert!(ok, "attackable +5: red number, no skull ({:?})", s.errors());

    // A hostile 10+ levels up: UnitLevel reads −1 → the skull replaces the number.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Guard".into()),
            health: 400,
            max_health: 400,
            level: 13,
            reaction: 2,
            can_attack: true,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let ok: bool = s
        .eval(
            r#"
            local lvl = getglobal("TargetLevelText")
            local skull = getglobal("TargetHighLevelTexture")
            return UnitLevel("target") == -1
               and skull:IsShown() ~= nil and lvl:IsShown() == nil
        "#,
        )
        .unwrap();
    assert!(ok, "hostile +10: the skull shows ({:?})", s.errors());

    // A DEAD mob is NOT a corpse (UnitIsCorpse `0x5161c0` is a pure TYPEID_CORPSE object check,
    // and UnitLevel `0x517fc0` has no health test) — the ref shows a dead mob's NUMBER, not the
    // skull.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Elder Mottled Boar".into()),
            health: 0,
            max_health: 40,
            level: 8,
            reaction: 4,
            dead: true,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let ok: bool = s
        .eval(
            r#"
            local lvl = getglobal("TargetLevelText")
            local skull = getglobal("TargetHighLevelTexture")
            return UnitIsCorpse("target") == nil
               and lvl:IsShown() ~= nil and skull:IsShown() == nil
               and tostring(lvl:GetText()) == "8"
        "#,
        )
        .unwrap();
    assert!(
        ok,
        "dead mob: the number shows, no skull ({:?})",
        s.errors()
    );

    // A resolved CORPSE world object: the ref's first branch — the skull, whatever the level.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Corpse of Somebody".into()),
            level: 8,
            corpse_object: true,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let ok: bool = s
        .eval(
            r#"
            local skull = getglobal("TargetHighLevelTexture")
            return UnitIsCorpse("target") == 1 and skull:IsShown() ~= nil
        "#,
        )
        .unwrap();
    assert!(ok, "corpse object: the skull shows ({:?})", s.errors());

    // The green→grey boundary rides GetQuestGreenRange: at player 30 the band is 7 —
    // 7 below (23) still standard-green, 8 below (22) trivial-grey.
    s.set_player_req_state(PlayerReqState {
        level: 30,
        ..Default::default()
    });
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level: 30,
            is_player: true,
            player_controlled: true,
            health: 50,
            max_health: 50,
            ..UnitState::default()
        }),
    );
    let ok: bool = s
        .eval(
            r#"
            local g, t = GetDifficultyColor(23), GetDifficultyColor(22)
            return GetQuestGreenRange() == 7
               and g.r == 0.25 and g.g == 0.75 and g.b == 0.25
               and t.r == 0.50 and t.g == 0.50 and t.b == 0.50
        "#,
        )
        .unwrap();
    assert!(ok, "green range boundary at level 30 ({:?})", s.errors());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The PvP flag icon on the player and target frames: the reference's
/// three-branch law, driven through the real shipped XML. FFA outranks the faction flag; the
/// faction leg needs BOTH a side and the flag; and the player's `igPVPUpdate` sounds on the
/// UNIT_FACTION edge, not on every repaint.
#[test]
fn pvp_icon_follows_the_three_branch_law() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    let _ = s.take_sounds(); // the frames' own load-time kits (target hide) aren't ours

    let icon_shown = |s: &UiScript, unit: &str| -> bool {
        s.eval(&format!(
            "return {unit}PVPIcon:IsVisible() and true or false"
        ))
        .unwrap()
    };
    // The extracted quad path for a shown icon (nil while hidden).
    let icon_path = |s: &mut UiScript, needle: &str| -> bool {
        s.resolve();
        s.extract().into_iter().any(
            |q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle)),
        )
    };

    let alliance_player = |pvp: bool, ffa: bool| UnitState {
        exists: true,
        name: Some("Benilla".into()),
        health: 50,
        max_health: 50,
        level: 10,
        is_player: true,
        player_controlled: true,
        faction_group: Some("Alliance".into()),
        pvp,
        is_pvp_ffa: ffa,
        ..UnitState::default()
    };

    // Unflagged: no icon, no sound.
    s.set_unit("player", Some(alliance_player(false, false)));
    s.fire_event("UNIT_FACTION", vec![ScriptValue::Str("player".into())]);
    assert!(!icon_shown(&s, "Player"), "unflagged shows none");
    assert!(s.take_sounds().is_empty(), "no sound while unflagged");

    // Flagged: the Alliance icon, and one igPVPUpdate for the flag change.
    s.set_unit("player", Some(alliance_player(true, false)));
    s.fire_event("UNIT_FACTION", vec![ScriptValue::Str("player".into())]);
    assert!(icon_shown(&s, "Player"));
    assert!(icon_path(&mut s, "UI-PVP-Alliance"), "faction leg art");
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igPVPUpdate".into())],
        "the flag change sounds once"
    );

    // A repaint that is NOT a flag change (a health tick) must not re-sound.
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    assert!(s.take_sounds().is_empty(), "repaints don't re-sound");

    // FFA outranks the faction flag even when both are set — the reference's branch order.
    s.set_unit("player", Some(alliance_player(true, true)));
    s.fire_event("UNIT_FACTION", vec![ScriptValue::Str("player".into())]);
    assert!(icon_path(&mut s, "UI-PVP-FFA"), "FFA wins the branch");
    let _ = s.take_sounds();

    // No resolvable side (a Monster/neutral template) hides the icon however flagged it is —
    // the `factionGroup and UnitIsPVP` gate. A guard the app can't name a side for draws nothing
    // rather than reaching for a texture that doesn't ship.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Kobold Vermin".into()),
            health: 40,
            max_health: 40,
            level: 8,
            reaction: 2,
            pvp: true,
            faction_group: None,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(
        !icon_shown(&s, "Target"),
        "flagged but sideless draws no icon"
    );

    // A Horde target that IS flagged takes its own faction's art on the target frame.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Orgrimmar Grunt".into()),
            health: 40,
            max_health: 40,
            level: 8,
            reaction: 2,
            pvp: true,
            faction_group: Some("Horde".into()),
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(icon_shown(&s, "Target"));
    assert!(icon_path(&mut s, "UI-PVP-Horde"), "the target's own side");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The target name plate's two middle legs, unblocked by the PvP wire: a
/// friendly player who is PvP-flagged reads GREEN, an unflagged one stays blue. Before the wire
/// both collapsed into blue.
#[test]
fn flagged_friendly_player_plate_is_green() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    let plate_color = |s: &mut UiScript| -> [f32; 4] {
        s.resolve();
        s.extract()
            .into_iter()
            .find_map(|q| match q.content {
                QuadContent::Texture {
                    path: Some(p),
                    color: Some(c),
                    ..
                } if p.contains("LevelBackground") => Some(c),
                _ => None,
            })
            .expect("target name-plate quad present")
    };
    let friendly_player = |pvp: bool| UnitState {
        exists: true,
        name: Some("Guildmate".into()),
        health: 50,
        max_health: 50,
        level: 20,
        is_player: true,
        player_controlled: true,
        reaction: 5,
        can_attack: false,
        faction_group: Some("Alliance".into()),
        pvp,
        ..UnitState::default()
    };

    s.set_unit("target", Some(friendly_player(false)));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let blue = plate_color(&mut s);
    assert!(
        blue[0].abs() < 1e-6 && blue[1].abs() < 1e-6 && (blue[2] - 1.0).abs() < 1e-6,
        "an unflagged friendly player is blue, got {blue:?}"
    );

    s.set_unit("target", Some(friendly_player(true)));
    s.fire_event("UNIT_FACTION", vec![ScriptValue::Str("target".into())]);
    let green = plate_color(&mut s);
    assert!(
        green[0].abs() < 1e-6 && (green[1] - 1.0).abs() < 1e-6 && green[2].abs() < 1e-6,
        "a PvP-flagged friendly player is green (UnitReactionColor[6]), got {green:?}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The classification border law (ref-TargetFrame.lua l.205-218) end to end: the
/// gated rank on the snapshot → `UnitClassification` → which of the three shipped border textures
/// actually reaches the draw list. Asserting the *extracted quad* rather than a Lua getter is the
/// point — it is the pixels, and it catches a swap that sets the path on the wrong region.
///
/// The two facts worth a test rather than a comment: three of the five classifications share the
/// Elite art (1.12 ships no rare-elite border at all), and the border must repaint on
/// UNIT_CLASSIFICATION_CHANGED alone, with no re-target.
#[test]
fn target_frame_border_follows_the_classification_law() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    // Every TargetingFrame border path in the draw list. The player frame contributes the plain
    // art on every frame, so the assertions below are about which *extra* border appears.
    let borders = |s: &mut UiScript| -> Vec<String> {
        s.resolve();
        let mut v: Vec<String> = s
            .extract()
            .into_iter()
            .filter_map(|q| match &q.content {
                QuadContent::Texture { path: Some(p), .. }
                    if p.contains("UI-TargetingFrame") && !p.contains("LevelBackground") =>
                {
                    Some(p.clone())
                }
                _ => None,
            })
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let has = |v: &[String], suffix: &str| v.iter().any(|p| p.ends_with(suffix));

    let mob = |rank: u32| UnitState {
        exists: true,
        name: Some("Ol' Sooty".into()),
        health: 400,
        max_health: 400,
        level: 26,
        reaction: 2,
        can_attack: true,
        rank,
        ..UnitState::default()
    };

    // rank 0 — a plain mob: no elite and no rare art anywhere.
    s.set_unit("target", Some(mob(0)));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let v = borders(&mut s);
    assert_eq!(
        s.eval::<String>(r#"return UnitClassification("target")"#)
            .unwrap(),
        "normal"
    );
    assert!(
        !has(&v, "UI-TargetingFrame-Elite") && !has(&v, "UI-TargetingFrame-Rare"),
        "rank 0 wears the plain border, got {v:?}"
    );

    // ranks 1/2/3 — elite, rare-elite and world boss ALL take the one Elite texture. 1.12 ships no
    // UI-TargetingFrame-Rare-Elite (absent from the patch chain), which is why 2 lands here.
    for (rank, word) in [(1, "elite"), (2, "rareelite"), (3, "worldboss")] {
        s.set_unit("target", Some(mob(rank)));
        s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
        let v = borders(&mut s);
        assert_eq!(
            s.eval::<String>(r#"return UnitClassification("target")"#)
                .unwrap(),
            word
        );
        assert!(
            has(&v, "UI-TargetingFrame-Elite") && !has(&v, "UI-TargetingFrame-Rare"),
            "rank {rank} ({word}) takes the Elite border, got {v:?}"
        );
    }

    // rank 4 — rare (the silver dragon), the only classification with art of its own.
    s.set_unit("target", Some(mob(4)));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let v = borders(&mut s);
    assert_eq!(
        s.eval::<String>(r#"return UnitClassification("target")"#)
            .unwrap(),
        "rare"
    );
    assert!(
        has(&v, "UI-TargetingFrame-Rare") && !has(&v, "UI-TargetingFrame-Elite"),
        "rank 4 takes the Rare border, got {v:?}"
    );

    // The repaint wire: the creature query landing on an already-targeted mob raises its rank, and
    // UNIT_CLASSIFICATION_CHANGED alone must swap the border — no PLAYER_TARGET_CHANGED.
    s.set_unit("target", Some(mob(0)));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(
        !has(&borders(&mut s), "UI-TargetingFrame-Elite"),
        "precondition: plain border before the query lands"
    );
    s.set_unit("target", Some(mob(1)));
    s.fire_event(
        "UNIT_CLASSIFICATION_CHANGED",
        vec![ScriptValue::Str("target".into())],
    );
    assert!(
        has(&borders(&mut s), "UI-TargetingFrame-Elite"),
        "the event alone repaints the border"
    );

    // The player frame never reclassifies: its own art stays plain with an elite target up.
    let plain_on_player: bool = s
        .eval(
            r#"
            local p = getglobal("PlayerFrameTexture")
            return p ~= nil and PlayerFrame.frameTexture == nil
        "#,
        )
        .unwrap();
    assert!(
        plain_on_player,
        "the player frame has the region but caches no handle, so nothing can swap it"
    );

    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The recessed-bar look, asserted in the draw list: the ring art's metal well-edges must paint
/// **over** the health/power fills, and the name/level text over both.
///
/// This is the shape decision 0884 broke. Our transcription used to get the ring over the bars by
/// *declaring* `$parentTextureFrame` after the StatusBars, leaning on the old draw key's
/// insertion-order tie-break between siblings at one `(strata, level)`. 0884 pinned the real law —
/// the draw **layer** is bucket-wide and outranks the frame — so the bars' ARTWORK fills started
/// painting over the TextureFrame's BACKGROUND ring art and the frames read as pasted-on slabs.
/// The fix is the reference's own mechanism (`TargetFrame.lua` l.32-34): push the bars one frame
/// level *below* the texture frame, where no layer can lift them back over it.
#[test]
fn the_ring_art_paints_over_the_bars() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Onemage".into()),
            health: 60,
            max_health: 100,
            level: 60,
            power_type: 0,
            power: 60,
            max_power: 100,
            is_connected: true, // else the power bar takes the disconnect leg's grey max fill
            ..UnitState::default()
        }),
    );
    // PLAYER_ENTERING_WORLD, not UNIT_HEALTH: the reference repaints the NAME only from
    // `UnitFrame_Update` (ref UnitFrame.lua l.23-28) and on UNIT_NAME_UPDATE (l.31-34) — the
    // health bar's own event touches its bar and nothing else. `PlayerFrame_OnEvent`'s
    // PLAYER_ENTERING_WORLD arm (ref PlayerFrame.lua l.96-100) is the one that runs the full
    // repaint, and the name quad is half of what this test measures.
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();
    let quads = s.extract();

    // The ring art: `UI-TargetingFrame` exactly — not the `-LevelBackground` / `-Elite` siblings.
    let ring = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.ends_with("UI-TargetingFrame"))
        })
        .expect("the player frame's ring art");
    // The bar fills (both bars share the texture; take the highest z of the two).
    let fill = quads
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.ends_with("UI-StatusBar"))
        })
        .map(|q| q.z)
        .max()
        .expect("a bar fill");
    let name = quads
        .iter()
        .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "Onemage"))
        .expect("the name text");

    assert!(
        ring.z > fill,
        "the ring art must paint OVER the bar fills (ring z={:#x}, fill z={fill:#x})",
        ring.z
    );
    assert!(
        name.z > fill,
        "the name text must paint OVER the bar fills (name z={:#x}, fill z={fill:#x})",
        name.z
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The same recessed-bar law for the PARTY member frames, which carry an independent copy of the
/// idiom (`PartyFrame.xml`'s `$parentTextureFrame` over `$parentHealthBar`/`$parentManaBar`).
///
/// Split from the player/target test on purpose: the two files reach the same look through separate
/// OnLoads, so one green assertion says nothing about the other. This is the copy 0884 broke in
/// silence — nobody was in a party when the director reported the player frame.
#[test]
fn the_party_art_paints_over_the_bars() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The loot test's prefix (`loot_tests.rs`): PartyFrame's inline <Script> reads
    // StaticPopupDialogs, which the chain's `StaticPopup.xml` defines, and its per-member dropdown
    // OnLoad walks the whole popup kit.
    // GlobalStrings first, for the same reason `load_unit_frames` names it — the stock unit-frame
    // files resolve it at LOAD (`CombatFeedback.lua` l.7-17, `UnitFrame.lua` l.1-6).
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\ReputationFrame.lua"); // FACTION_BAR_COLORS (1968)
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    // Before UnitPopup: that file reads ITEM_QUALITY_COLORS at FILE SCOPE and its
    // declarer is UIParent (ref UIParent.lua:65) since 1888.
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\UnitPopup.xml");
    // The reference's own kit, in the manifest's order. `UIParent.xml` is not decoration here:
    // `RaiseFrameLevel`/`LowerFrameLevel` live in it (ref UIParent.lua l.1890-1896) and stock
    // `TargetofTargetTextureFrame`'s OnLoad calls one of them.
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(&s, "Interface\\FrameXML\\BuffFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UnitFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\CombatFeedback.xml");
    load_xml(&s, "Interface\\FrameXML\\PlayerFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\PartyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\TargetFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\PetFrame.xml");

    s.set_unit(
        "party1",
        Some(UnitState {
            exists: true,
            name: Some("Onepriest".into()),
            health: 60,
            max_health: 100,
            level: 60,
            power_type: 0,
            power: 60,
            max_power: 100,
            is_connected: true,
            ..UnitState::default()
        }),
    );
    // **The ROSTER, not the unit snapshot, is what shows a party row.** Stock
    // `PartyMemberFrame_UpdateMember` gates on `GetPartyMember(id)` (ref PartyMemberFrame.lua
    // l.41-57) — `UnitExists("party1")` is never consulted — and its else arm is `this:Hide()`.
    // Our deleted `PartyFrame.xml` keyed the row off the unit token, so setting `party1` alone
    // used to be enough; against the reference's file it paints nothing at all.
    s.set_party(benilla_ui::script::PartyState {
        members: vec![benilla_ui::script::PartyMemberInfo {
            name: "Onepriest".into(),
            guid: 0x0_0B12,
        }],
        leader_index: 0,
        leader_guid: 0,
        own_guid: 0,
        raid: Vec::new(),
        loot_method: "group".into(),
        master_looter: None,
        loot_threshold: 2,
    });
    s.fire_event("PARTY_MEMBERS_CHANGED", vec![]);
    s.resolve();
    let quads = s.extract();

    // Scoped to member frame 1 by owner name, so a stray UI-StatusBar from any other frame in the
    // manifest can never stand in for the bar under test.
    let mine = |q: &benilla_ui::script::ExtractedQuad| {
        s.quad_owner_name(q.target)
            .is_some_and(|n| n.starts_with("PartyMemberFrame1"))
    };
    // The ART cannot be scoped that way, and the reason is the reference's own declaration:
    // `PartyMemberFrameTemplate` hangs `$parentTexture` two levels down inside a pair of
    // **anonymous** `<Frame setAllPoints="true">` wrappers (ref PartyFrameTemplates.xml
    // l.230-240). The region itself still resolves to `PartyMemberFrame1Texture` — `$parent`
    // walks to the nearest named ancestor — but the frame that OWNS the quad has no name at all,
    // so an owner-name filter can never see it. (Our deleted PartyFrame.xml gave that wrapper a
    // name, `$parentTextureFrame`, which is why this used to work.) Scoped by geometry instead:
    // rows 2-4 are hidden with no member, so exactly one UI-PartyFrame quad is drawn, and it is
    // asserted to sit on member 1's rect.
    let art_quads: Vec<_> = quads
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.ends_with("UI-PartyFrame"))
        })
        .collect();
    assert_eq!(
        art_quads.len(),
        1,
        "one member in the party, one row of art drawn"
    );
    let art = art_quads[0];
    let row: Vec<f32> = s
        .eval(
            "return { PartyMemberFrame1:GetLeft(), PartyMemberFrame1:GetBottom(), \
                      PartyMemberFrame1:GetRight(), PartyMemberFrame1:GetTop() }",
        )
        .unwrap();
    let r = art.rect.expect("the art quad has a resolved rect");
    assert!(
        r.left >= row[0] - 1.0 && r.right <= row[2] + 1.0 && r.top <= row[3] + 3.0,
        "the art quad sits on PartyMemberFrame1 (art {r:?}, row {row:?})"
    );
    let fill = quads
        .iter()
        .filter(|q| {
            mine(q)
                && matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.ends_with("UI-StatusBar"))
        })
        .map(|q| q.z)
        .max()
        .expect("a bar fill");
    assert!(
        art.z > fill,
        "the party art must paint OVER the bar fills (art z={:#x}, fill z={fill:#x})",
        art.z
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **What a feigning hunter looks like on the frames** — the end of the chain the
/// snapshot starts: `UNIT_DYNFLAG_DEAD` zeroes `UnitHealth`/`UnitMana` while the maxima stay real
/// (`UnitHealth 0x5174d0` gates, `UnitHealthMax 0x5175b0` does not), so both bars run **empty over
/// a full-size track** rather than collapsing to a 0/0 nothing, and the target frame's DEAD text
/// lights on the same `UnitHealth(unit) <= 0` test a corpse trips.
#[test]
fn a_feigning_target_paints_empty_bars_and_the_dead_text() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    let hunter = |health: u32, power: u32, dead: bool| {
        Some(UnitState {
            exists: true,
            is_connected: true, // CheckDead's second term — a feign is not a link-drop
            name: Some("Corvane".into()),
            health,
            max_health: 1500,
            level: 60,
            power_type: 0,
            power,
            max_power: 900,
            dead,
            reaction: 2, // hostile — the frame we would be watching him through
            ..UnitState::default()
        })
    };

    s.set_unit("target", hunter(1200, 300, false));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let alive: bool = s
        .eval(
            r#"
            local hb, pb = TargetFrameHealthBar, TargetFrameManaBar
            return hb:GetValue() == 1200 and pb:GetValue() == 300
               and not TargetDeadText:IsShown()
        "#,
        )
        .unwrap();
    assert!(alive, "the control: a live hunter reads live");

    // He feigns. Only the flag moved on the wire — the snapshot turns it into these three.
    //
    // BOTH events, because the reference's bars are independent listeners: the health bar takes
    // UNIT_HEALTH/UNIT_MAXHEALTH (ref UnitFrame.lua l.150-151) and the mana bar takes the ten
    // UNIT_MANA/RAGE/FOCUS/… ones (l.189-199) — there is no shared "repaint the frame" event, so
    // UNIT_HEALTH alone leaves the power bar showing the pre-feign number. Our deleted
    // transcription drove both bars off one update, which is why this test used to fire one event.
    // The server sends both when the dynflag lands.
    s.set_unit("target", hunter(0, 0, true));
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("target".into())]);
    s.fire_event("UNIT_MANA", vec![ScriptValue::Str("target".into())]);
    let (hp, hmax, mana, mmax): (f64, f64, f64, f64) = (
        s.eval("return TargetFrameHealthBar:GetValue()").unwrap(),
        s.eval("local _, m = TargetFrameHealthBar:GetMinMaxValues() return m")
            .unwrap(),
        s.eval("return TargetFrameManaBar:GetValue()").unwrap(),
        s.eval("local _, m = TargetFrameManaBar:GetMinMaxValues() return m")
            .unwrap(),
    );
    assert_eq!((hp, hmax), (0.0, 1500.0), "empty health bar, real track");
    assert_eq!((mana, mmax), (0.0, 900.0), "empty mana bar, real track");
    assert!(
        s.eval::<bool>("return TargetFrameManaBar:IsVisible()")
            .unwrap(),
        "the mana bar empties, it does not disappear — UnitManaMax 0x5177e0 is ungated"
    );
    assert!(
        s.eval::<bool>("return TargetDeadText:IsShown() and true or false")
            .unwrap(),
        "TargetFrame_CheckDead's UnitHealth(unit) <= 0 test, tripped by the flag"
    );
    assert_eq!(
        s.eval::<String>("return TargetDeadText:GetText()").unwrap(),
        "Dead",
        "the WORD is the GlobalString `DEAD` (l.898), never the key: a literal \"DEAD\" here \
         is the caps bug the director caught on Onyxia"
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsDead("target")"#).unwrap(),
        1,
        "UnitIsDead 0x517ac0's dynflag leg reaches the API too — as the number 1 (2043)"
    );

    // He stands back up: the flag clears, and nothing about the body needed restoring.
    s.set_unit("target", hunter(1200, 300, false));
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("target".into())]);
    s.fire_event("UNIT_MANA", vec![ScriptValue::Str("target".into())]);
    let up: bool = s
        .eval(
            r#"
            return TargetFrameHealthBar:GetValue() == 1200
               and TargetFrameManaBar:GetValue() == 300
               and not TargetDeadText:IsShown()
               and not UnitIsDead("target")
        "#,
        )
        .unwrap();
    assert!(up, "the feign ends and the frame reads live again");
}

/// The resting status flash (ref `PlayerFrame_UpdateStatus` + `_OnUpdate`): while
/// resting the player frame wears the gold status ring, the zzz state icon and its glow, all
/// pulsing on a 0.5 s alpha wave; auto-attack (PLAYER_ENTER_COMBAT) swaps them for the red
/// ring/swords/disc — resting still wins when both hold — and leaving both states clears the lot.
#[test]
fn the_player_frame_flashes_zzz_while_resting() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Prober".into()),
            health: 100,
            max_health: 100,
            level: 12,
            power_type: 0,
            power: 80,
            max_power: 80,
            dead: false,
            reaction: 0,
            ..UnitState::default()
        }),
    );

    // Into the inn: the resting flag lands and PLAYER_UPDATE_RESTING repaints.
    s.set_rest_state(1, 500, true);
    s.fire_event("PLAYER_UPDATE_RESTING", vec![]);
    let resting: bool = s
        .eval(
            r#"
            local u = "Player"
            return getglobal(u .. "StatusTexture"):IsShown()
               and getglobal(u .. "RestIcon"):IsShown()
               and not getglobal(u .. "AttackIcon"):IsShown()
               and PlayerStatusGlow:IsShown()
               and PlayerRestGlow:IsShown()
               and not PlayerAttackGlow:IsShown()
               and not getglobal(u .. "AttackBackground"):IsShown()
        "#,
        )
        .unwrap();
    assert!(
        resting,
        "resting shows the gold ring + zzz + glow, nothing red"
    );

    // The pulse: two OnUpdate ticks move the ring's alpha (the 0.5 s triangle wave).
    let a0: f64 = s.eval("return PlayerStatusTexture:GetAlpha()").unwrap();
    s.run("this = PlayerFrame; PlayerFrame_OnUpdate(0.25)")
        .unwrap();
    let a1: f64 = s.eval("return PlayerStatusTexture:GetAlpha()").unwrap();
    assert!(
        (a0 - a1).abs() > 0.1,
        "the flash moves the status alpha ({a0} → {a1})"
    );

    // Swinging while resting: resting still wins (the ref's branch order).
    s.fire_event("PLAYER_ENTER_COMBAT", vec![]);
    assert!(
        s.eval::<bool>("return PlayerRestIcon:IsShown()").unwrap(),
        "resting outranks auto-attack"
    );

    // Out of the inn mid-swing: the red attack set takes over.
    s.set_rest_state(2, 0, false);
    s.fire_event("PLAYER_UPDATE_RESTING", vec![]);
    let attacking: bool = s
        .eval(
            r#"
            local u = "Player"
            return getglobal(u .. "StatusTexture"):IsShown()
               and getglobal(u .. "AttackIcon"):IsShown()
               and not getglobal(u .. "RestIcon"):IsShown()
               and PlayerAttackGlow:IsShown()
               and getglobal(u .. "AttackBackground"):IsShown()
        "#,
        )
        .unwrap();
    assert!(attacking, "auto-attack shows the red ring + swords + disc");

    // Swords down: everything clears.
    s.fire_event("PLAYER_LEAVE_COMBAT", vec![]);
    let clear: bool = s
        .eval(
            r#"
            local u = "Player"
            return not getglobal(u .. "StatusTexture"):IsShown()
               and not getglobal(u .. "RestIcon"):IsShown()
               and not getglobal(u .. "AttackIcon"):IsShown()
               and not PlayerStatusGlow:IsShown()
               and not getglobal(u .. "AttackBackground"):IsShown()
        "#,
        )
        .unwrap();
    assert!(clear, "neither state → no status dressing at all");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The zzz badge paints OVER the level number: the ref keeps the number in the
/// texture frame's BACKGROUND layer and the state icons up in OVERLAY, so the opaque badge covers
/// it while resting. The layer split is the only mechanism that CAN hide it — a fontstring never
/// ducks under a texture of its own layer (0884's bucket-wide quads-then-text law) — which is
/// exactly how 1082's transcription slipped: it put the number in OVERLAY beside the icons, and
/// the number rode on the badge (director report, 2026-08-07).
#[test]
fn the_rest_badge_covers_the_level_number() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Prober".into()),
            health: 100,
            max_health: 100,
            level: 12,
            power_type: 0,
            power: 80,
            max_power: 80,
            dead: false,
            reaction: 0,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.set_rest_state(1, 500, true);
    s.fire_event("PLAYER_UPDATE_RESTING", vec![]);
    s.resolve();
    let quads = s.extract();

    // Every UI-StateIcon quad (the badge AND its ADD glow — the glow rides a later frame and
    // sits higher still): even the LOWEST must clear the number.
    let badge = quads
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.ends_with("UI-StateIcon"))
        })
        .map(|q| q.z)
        .min()
        .expect("the zzz badge");
    let level = quads
        .iter()
        .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "12"))
        .expect("the level text");
    assert!(
        badge > level.z,
        "the badge must cover the level number (badge z={badge:#x}, level z={:#x})",
        level.z
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The on-bar numerals** — the half 1140 promised and did not deliver. The Status
/// Bar Text switch landed with only the XP bar wired to it, so turning it on changed nothing on the
/// frames people actually watch (director report). Now the player's health and power bars carry
/// "value / max", and the switch pins them.
///
/// Which bars it reaches is the REFERENCE's own split, and running the reference's own files made
/// it narrower than 1146 believed. `textLockable` is set on the player's two bars, the pet's two
/// and the XP bar and nothing else — but the TARGET frame goes further than "not lockable": it has
/// **no text regions at all**. `TargetFrame.xml` l.486-487 hands `TargetFrameHealthBarText` /
/// `TargetFrameManaBarText` to `UnitFrame_Initialize` and neither name is declared anywhere in
/// FrameXML (only `PlayerFrame.xml` l.79/88 and `PetFrame.xml` l.87/96 declare theirs), so
/// `SetTextStatusBarText` early-returns on the nil (ref TextStatusBar.lua l.7-10) and those bars
/// never get a `TextString`. 1146 §3 *added* the two regions to our transcription so a hover could
/// reveal them; they went with the file. What 1.12 actually gives you when you hover a target bar
/// is the unit TOOLTIP (ref TextStatusBar.xml l.16-26), asserted below.
///
/// The power bar's numerals also carry the resource's LABEL again — "Rage 45 / 80" — because
/// `UnitFrame_UpdateManaType` sets the prefix on every update (ref UnitFrame.lua l.129) and that
/// call is inside the stock file. 1147 §1 cut ours on the director's look call; the stock file has
/// no such cut.
#[test]
fn status_bar_text_paints_the_player_numerals_but_not_the_targets() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    let alive = |health: u32, power: u32, power_type: u8| UnitState {
        exists: true,
        name: Some("Somebody".into()),
        health,
        max_health: 100,
        level: 12,
        power_type,
        power,
        max_power: 80,
        dead: false,
        reaction: 4,
        // Without this `UnitFrameManaBar_Update` takes its disconnect leg (ref UnitFrame.lua
        // l.209-212) — the power bar pins to MAX and never reaches `UnitFrame_UpdateManaType`,
        // so it would read "80 / 80" with no prefix.
        is_connected: true,
        ..UnitState::default()
    };
    s.set_unit("player", Some(alive(72, 45, 1))); // power_type 1 = RAGE
    s.set_unit("target", Some(alive(50, 20, 0)));
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);

    let text = |s: &UiScript, name: &str| -> String {
        s.eval::<String>(&format!("return tostring(({name}:GetText()) or \"\")"))
            .unwrap()
    };
    let shown = |s: &UiScript, name: &str| -> bool {
        s.eval::<bool>(&format!("return {name}:IsShown()")).unwrap()
    };

    // Off (the shipped default): the strings carry the numbers, and nothing paints.
    assert!(!shown(&s, "PlayerFrameHealthBarText"));
    assert!(!shown(&s, "PlayerFrameManaBarText"));

    // On, through the switch's own event — no repaint, no damage taken.
    s.register_cvars([("statusBarText", "0")]);
    s.run("SetCVar(\"statusBarText\", \"1\", \"STATUS_BAR_TEXT\")")
        .unwrap();
    s.tick(0.0);
    assert!(
        shown(&s, "PlayerFrameHealthBarText"),
        "your own health numerals pin on"
    );
    // Bare here, and only because this fixture stops short of the character window: the
    // reference's "Health" prefix is set by `CharacterFrame_OnLoad` (ref CharacterFrame.lua l.55),
    // not by the unit frames, and the manifest loads `Interface\FrameXML\CharacterFrame.xml` far
    // below these. In a full run the player's health bar reads "Health 72 / 100".
    assert_eq!(
        text(&s, "PlayerFrameHealthBarText"),
        "72 / 100",
        "no prefix without the character window, which is what sets HEALTH"
    );
    // The power bar's LABEL is back, and it is the unit frames' own:
    // `UnitFrame_UpdateManaType` re-sets the prefix from `ManaBarColor[UnitPowerType(unit)].prefix`
    // on every mana-bar update (ref UnitFrame.lua l.122-129), and
    // `TextStatusBar_UpdateTextString` renders `prefix .. " " .. value .. " / " .. max`
    // (ref TextStatusBar.lua l.42-46). 1147 §1 cut the three prefix calls out of OUR files on the
    // director's look call; this one lives inside the stock file, so it came back with it.
    assert_eq!(
        s.eval::<String>("return PlayerFrameManaBar.prefix")
            .unwrap(),
        "Rage",
        "the prefix follows the resource — this player runs on rage"
    );
    // The STRING, though, is still the one rendered before the switch existed. That is the
    // reference's own ordering, not a gap: `TextStatusBar_OnEvent`'s CVAR_UPDATE arm only calls
    // `TextString:Show()` (ref TextStatusBar.lua l.14-24) — it never re-renders — and the last
    // render ran from `SetValue`'s OnValueChanged during PLAYER_ENTERING_WORLD, before
    // `UnitFrame_UpdateManaType` had set the prefix (ref UnitFrame.lua l.208-215 sets the value
    // first). Its own re-render is gated on `GetCVar("statusBarText") == "1"` (l.130-132), which
    // was not yet true. So the label lands on the next repaint — asserted at the end of this test.
    assert_eq!(
        text(&s, "PlayerFrameManaBarText"),
        "45 / 80",
        "flipping the switch shows the string, it does not re-render it"
    );

    // The target's bars carry NO text region, so there is nothing for the switch to pin and
    // nothing for a hover to reveal. Asserted on the globals themselves: the reference leaves both
    // names undeclared (ref TargetFrame.xml l.486-487 passes them anyway).
    assert!(
        s.eval::<bool>(
            "return TargetFrameHealthBarText == nil and TargetFrameManaBarText == nil \
             and TargetFrameHealthBar.TextString == nil and TargetFrameManaBar.TextString == nil"
        )
        .unwrap(),
        "the reference declares no numerals on the target frame, at any switch setting"
    );

    // Hovering a target bar pops the unit TOOLTIP instead — the `elseif this:GetParent() ==
    // TargetFrame` arm of the template's own OnEnter (ref TextStatusBar.xml l.16-26). Driven
    // through the real pointer path, because that arm reads `this`.
    hover(&mut s, "TargetFrameHealthBar");
    assert!(
        s.eval::<bool>(
            "return GameTooltip:IsShown() and GameTooltip:IsOwned(TargetFrameHealthBar)"
        )
        .unwrap(),
        "the target's bar hover is a tooltip, not numerals ({:?})",
        s.errors()
    );
    assert!(
        s.eval::<String>("return tostring(GameTooltipTextLeft1:GetText())")
            .unwrap()
            .contains("Somebody"),
        "and it is the unit's own plate"
    );
    unhover(&mut s);
    assert!(
        s.eval::<bool>("return not GameTooltip:IsShown()").unwrap(),
        "the template's OnLeave hides it (ref TextStatusBar.xml l.28-31)"
    );

    // The hover on YOUR bar with the switch OFF is the reveal that still exists: the numbers are
    // there when you go looking, without living on the bar. `ShowTextStatusBarText` /
    // `HideTextStatusBarText` and their `lockShow` refcount (ref TextStatusBar.lua l.77-102) are
    // the mechanism, reached through the template's OnEnter/OnLeave.
    s.run("SetCVar(\"statusBarText\", \"0\", \"STATUS_BAR_TEXT\")")
        .unwrap();
    s.tick(0.0);
    assert!(!shown(&s, "PlayerFrameHealthBarText"));
    hover(&mut s, "PlayerFrameHealthBar");
    assert!(
        shown(&s, "PlayerFrameHealthBarText"),
        "hover shows them even with the option off ({:?})",
        s.errors()
    );
    unhover(&mut s);
    assert!(
        !shown(&s, "PlayerFrameHealthBarText"),
        "and go away again — the lockShow refcount balances"
    );
    s.run("SetCVar(\"statusBarText\", \"1\", \"STATUS_BAR_TEXT\")")
        .unwrap();
    s.tick(0.0);

    // A health change repaints through the bar's own OnValueChanged, like the reference's.
    s.set_unit("player", Some(alive(31, 45, 1)));
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    assert_eq!(text(&s, "PlayerFrameHealthBarText"), "31 / 100");

    // A power-type change repaints AND re-labels — `UnitFrame_UpdateManaType` runs off
    // UNIT_DISPLAYPOWER (ref UnitFrame.lua l.36-40) and the prefix follows the new resource.
    s.set_unit("player", Some(alive(31, 60, 3)));
    s.fire_event("UNIT_DISPLAYPOWER", vec![ScriptValue::Str("player".into())]);
    assert_eq!(text(&s, "PlayerFrameManaBarText"), "Energy 60 / 80");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The numerals do not collide** — the pin for a bug the director caught on
/// screen: the pet frame's two numeral strings overlapped each other and its name.
///
/// The cause was a bad transcription, not a bad seat. 1.12's pet bars are 70×8 at `(47,-22)` and
/// `(47,-29)` — a **7 px pitch** for strings taller than that — so the reference does not centre the
/// second one: it drops it clear, to `(82,-38)`, just under a bar that ends at −37. 1143 replaced
/// those literals with "anchor each string to its own bar's centre", on a comparison that read the
/// reference's TEXT offsets against our BAR offsets and concluded the geometry differed. It does
/// not: our bars are byte-identical to the reference's, so its literals apply unchanged.
///
/// This asserts the PROPERTY, not the numbers, so it survives a future nudge (1145 moved the
/// player's pair the day before this). Text metrics come from the harness's own measurer — a
/// FontString with a single CENTER anchor and no measured size cannot pin either edge, so it falls
/// back to its owner's rect and every string would appear to sit in the same box.
#[test]
fn no_two_numeral_strings_overlap_on_any_frame() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    let alive = |health: u32, power: u32| UnitState {
        exists: true,
        name: Some("Onehunter".into()),
        health,
        max_health: health,
        level: 40,
        power_type: 0,
        power,
        max_power: power,
        dead: false,
        reaction: 4,
        ..UnitState::default()
    };
    s.set_unit("player", Some(alive(4122, 3300)));
    s.set_unit("target", Some(alive(4122, 3300)));
    s.set_unit("pet", Some(alive(256, 100)));
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    s.register_cvars([("statusBarText", "0")]);
    s.run("SetCVar(\"statusBarText\", \"1\", \"STATUS_BAR_TEXT\")")
        .unwrap();
    s.tick(0.0);

    // The host's half of the measure round-trip, at the numerals' real font size: NumberFontNormal
    // is ~14 px tall, and a digit runs ~7 px wide. The round-trip answers a frame late, so settle.
    for _ in 0..4 {
        let answers: Vec<(u32, f32, f32, u64)> = s
            .fontstrings_needing_measure()
            .into_iter()
            .map(|r| (r.id, r.text.chars().count() as f32 * 7.0, 14.0, r.key))
            .collect();
        s.set_measured_text_unwrapped(&answers);
        s.tick(0.05);
        s.resolve();
    }

    // The pin is the SEPARATION between the two numeral seats, which is the thing that broke and
    // the thing the reference authored deliberately. Not "the line boxes must not overlap": at a
    // 14 px NumberFontNormal the reference's own player seats are 12 px apart, so the boxes DO
    // overlap there while the ink (a digit's ~10 px cap height, centred) does not. Only a seat
    // pitch well under the font is a real collision — the pet's was 7 px.
    for (frame, upper, lower, want) in [
        (
            "player",
            "PlayerFrameHealthBarText",
            "PlayerFrameManaBarText",
            12.0,
        ),
        ("pet", "PetFrameHealthBarText", "PetFrameManaBarText", 11.0),
    ] {
        let mid = |s: &UiScript, n: &str| -> f64 {
            s.eval::<f64>(&format!("return ({n}:GetTop() + {n}:GetBottom()) / 2"))
                .unwrap()
        };
        let apart = mid(&s, upper) - mid(&s, lower);
        assert!(
            (apart - want).abs() < 0.51,
            "{frame}: the numerals sit {apart:.1} px apart, the reference authors {want:.0}"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **`UnitFrame_OnEnter`/`OnLeave` exist under the REFERENCE's names, `this`-shaped, so an addon
/// that hooks them is reached.**
///
/// `TipBuddy.lua:2770-2773` is the exact idiom, and it is the 1.12 customisation model in general:
///
/// ```lua
/// originalUnitFrame_OnEnter = UnitFrame_OnEnter
/// function UnitFrame_OnEnter() originalUnitFrame_OnEnter() … end
/// ```
///
/// Against a missing global that captured nil and raised on the first hover. Our own adapter
/// (`BenillaUnitFrame_OnEnter(frame)`, which took the frame explicitly) went with the file 1751's
/// twenty-second window deleted: what runs here is the reference's own `this`-shaped body.
///
/// The same class as the Bagnon bag bug, and equally invisible to the corpus survey: it loads
/// addons and fires events, but never hovers anything.
#[test]
fn the_unit_frame_hover_hooks_carry_the_references_names() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_unit_frames(&s);

    assert!(
        s.eval::<bool>(
            "return type(UnitFrame_OnEnter) == 'function' \
             and type(UnitFrame_OnLeave) == 'function'"
        )
        .unwrap(),
        "the reference's names must exist for an addon to capture"
    );

    // TipBuddy's idiom, run for real against a live unit frame.
    s.run(
        r#"
        HOVERS = 0
        local original = UnitFrame_OnEnter
        function UnitFrame_OnEnter()
            HOVERS = HOVERS + 1
            original()
        end
        this = PlayerFrame
        UnitFrame_OnEnter()
    "#,
    )
    .unwrap();

    assert_eq!(
        s.eval::<i64>("return HOVERS").unwrap(),
        1,
        "the addon's replacement must run"
    );
    assert!(
        s.errors().is_empty(),
        "and calling through to the original must not raise: {:?}",
        s.errors()
    );
}

/// **`SetRaidTargetIconTexture` lands each mark on its own cell of the 4×4 sheet.**
///
/// The helper an addon uses to draw a raid mark it got from `GetRaidTargetIndex` — three corpus
/// addons in two codebases (CustomNameplates; Optional/oRA2's shared `MainTank.lua:1216` and
/// `PlayerTarget.lua:1472`). It owns no frame and reads no unit: the texture is the caller's.
///
/// The coordinates are the point, so all eight are checked rather than one. Star is the top-left
/// cell and skull the bottom-right of the first two rows — which is also what pins the reference's
/// own `/ ROWS` row derivation as harmless here: with 4 rows and 4 columns the wrap lands where the
/// sheet's mark order says it should.
#[test]
fn the_raid_mark_helper_maps_each_index_to_its_cell() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_unit_frames(&s);
    s.run(r#"RTMark = UIParent:CreateTexture("RTMark", "OVERLAY")"#)
        .unwrap();

    // (index, left, right, top, bottom) — 0.25 per cell, four across then down a row.
    let cells = [
        (1, 0.00, 0.25, 0.00, 0.25), // star
        (2, 0.25, 0.50, 0.00, 0.25), // circle
        (3, 0.50, 0.75, 0.00, 0.25), // diamond
        (4, 0.75, 1.00, 0.00, 0.25), // triangle
        (5, 0.00, 0.25, 0.25, 0.50), // moon — the wrap onto row 2
        (6, 0.25, 0.50, 0.25, 0.50), // square
        (7, 0.50, 0.75, 0.25, 0.50), // cross
        (8, 0.75, 1.00, 0.25, 0.50), // skull
    ];
    for (i, l, r, t, b) in cells {
        s.run(&format!("SetRaidTargetIconTexture(RTMark, {i})"))
            .unwrap();
        // `GetTexCoord` answers EIGHT (UL, LL, UR, LR as x,y pairs) since 1840; the old
        // `(l, r, t, b)` rect is `ULx, URx, ULy, LLy` — positions 1, 5, 2, 4.
        let (gl, gt, _, gb, gr, ..): (f64, f64, f64, f64, f64, f64, f64, f64) =
            s.eval("return RTMark:GetTexCoord()").unwrap();
        assert_eq!(
            (gl, gr, gt, gb),
            (l, r, t, b),
            "mark {i} must sample the cell at ({l}, {r}, {t}, {b})"
        );
    }
    assert!(s.errors().is_empty(), "no errors: {:?}", s.errors());
}

/// B317 — the player's OWN frame wears the leader and master-looter icons. Every other frame that
/// can wear them has since 0434 phase 2; the player frame predated the party wire and listed them
/// OUT, so the one person who could not see who was leading was the leader.
///
/// The two predicates differ, and the asymmetry is the reference's (`PlayerFrame_UpdatePartyLeader`):
/// the leader icon asks `IsPartyLeader()`, the master icon asks whether `GetLootMethod`'s party
/// index is `0` — the player's own seat on that scale — *and* that we are grouped at all.
#[test]
fn the_player_frame_wears_the_leader_and_master_looter_icons() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::{PartyMemberInfo, PartyState};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Aldwyn".into()),
            health: 100,
            max_health: 100,
            level: 60,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);

    let shown = |s: &UiScript, region: &str| -> bool {
        s.eval::<bool>(&format!("return {region}:IsVisible()"))
            .unwrap()
    };
    let leader = "PlayerLeaderIcon";
    let master = "PlayerMasterIcon";

    // Solo: neither. A solo player "leads" nothing — `IsPartyLeader()` is nil without a group.
    assert!(!shown(&s, leader), "solo: no leader icon");
    assert!(!shown(&s, master), "solo: no master-looter icon");

    let party = |leader_index: u32, master_looter: Option<u32>, method: &str| PartyState {
        members: vec![PartyMemberInfo {
            name: "Brisca".into(),
            guid: 0x7A17,
        }],
        leader_index,
        // Follows `leader_index`: 0 = the player (unset here), else the member who leads.
        leader_guid: if leader_index == 0 { 0 } else { 0x7A17 },
        own_guid: 0,
        raid: Vec::new(),
        loot_method: method.into(),
        master_looter,
        loot_threshold: 2,
    };

    // Grouped, we lead, group loot: leader icon only.
    s.set_party(party(0, None, "group"));
    s.fire_event("PARTY_LEADER_CHANGED", vec![]);
    assert!(shown(&s, leader), "we lead: the leader icon shows");
    assert!(!shown(&s, master), "group loot: no master-looter icon");

    // Master loot, and we are the master (index 0 — the player's own seat).
    s.set_party(party(0, Some(0), "master"));
    s.fire_event("PARTY_LOOT_METHOD_CHANGED", vec![]);
    assert!(shown(&s, master), "we are master looter: the icon shows");

    // The master looter is party1 instead: the icon is theirs, not ours.
    s.set_party(party(0, Some(1), "master"));
    s.fire_event("PARTY_LOOT_METHOD_CHANGED", vec![]);
    assert!(!shown(&s, master), "somebody else masters: our icon hides");
    assert!(shown(&s, leader), "…and we still lead");

    // Leadership passes to party1: the crown goes with it.
    s.set_party(party(1, Some(1), "master"));
    s.fire_event("PARTY_LEADER_CHANGED", vec![]);
    assert!(!shown(&s, leader), "we no longer lead: the crown hides");

    // Leaving the group clears both, through PARTY_MEMBERS_CHANGED alone.
    s.set_party(PartyState::default());
    s.fire_event("PARTY_MEMBERS_CHANGED", vec![]);
    assert!(
        !shown(&s, leader) && !shown(&s, master),
        "ungrouped: both hide"
    );

    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Every unit-frame global the reference declares** — the whole block, per.
///
/// benilla named the player's and target's power bar `…PowerBar`, which is the LATER client's
/// vocabulary: 1.12 has no `*PowerBar` anywhere. Every one is `ManaBar` — `PlayerFrameManaBar`,
/// `TargetFrameManaBar`, `PetFrameManaBar`, plus the `ManaBarColor` table and the `frame.manabar`
/// field `UnitFrame_Initialize` sets (`UnitFrame.lua:14`). So we published four names the
/// reference lacks and were missing four it has, which is decision 1189's error in both
/// directions at once. ShaguTweaks reads `TargetFrameManaBar` unguarded (`health-numbers.lua:22`)
/// and died on it; our own party frames already used the right name, so the tree disagreed with
/// itself as well.
///
/// The guard is the reference's own list, not the names that turned up missing — 1718's rule,
/// which exists because the two previous guards of this class each enumerated the instances that
/// had already broken and so could not fire on the next one. Deliberate ADDITIONS are fine and are
/// not asserted against; what is asserted is that nothing the reference declares is absent.
#[test]
fn the_unit_frames_publish_every_name_the_reference_declares() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_unit_frames(&s);

    // ref PlayerFrame.xml / TargetFrame.xml / PetFrame.xml — every `name=` those three files
    // declare, minus the `virtual="true"` templates (which are not globals) and the buff/debuff
    // button instances (a separate arc: TargetFrameBuff1..5, TargetFrameDebuff1..16).
    //
    // The list is taken from the reference files, not written from what turned up missing — and
    // the first draft of THIS test still got that wrong, by filtering the reference's own list to
    // the names starting `PlayerFrame`/`TargetFrame`/`PetFrame` and so dropping `PlayerName`,
    // `PetPortrait`, `PetAttackModeTexture` and two dozen more. 1718's rule is easy to state and
    // easy to re-break one level down.
    //
    // Collected rather than asserted one at a time: the point of a whole-block guard is to report
    // the whole gap, and a first-failure assert would have hidden everything after it.
    let mut missing = Vec::new();
    for name in [
        // PlayerFrame.xml
        "PlayerAttackBackground",
        "PlayerAttackGlow",
        "PlayerAttackIcon",
        "PlayerFrame",
        "PlayerFrameBackground",
        "PlayerFrameGroupIndicator",
        "PlayerFrameGroupIndicatorLeft",
        "PlayerFrameGroupIndicatorMiddle",
        "PlayerFrameGroupIndicatorRight",
        "PlayerFrameGroupIndicatorText",
        "PlayerFrameHealthBar",
        "PlayerFrameHealthBarText",
        "PlayerFrameManaBar",
        "PlayerFrameManaBarText",
        "PlayerFrameTexture",
        "PlayerHitIndicator",
        "PlayerLeaderIcon",
        "PlayerLevelText",
        "PlayerMasterIcon",
        "PlayerName",
        "PlayerPortrait",
        "PlayerPVPIcon",
        "PlayerRestGlow",
        "PlayerRestIcon",
        "PlayerStatusGlow",
        "PlayerStatusTexture",
        // TargetFrame.xml
        "TargetDeadText",
        "TargetFrame",
        "TargetFrameBackground",
        "TargetFrameHealthBar",
        "TargetFrameManaBar",
        "TargetFrameNameBackground",
        "TargetFrameTexture",
        "TargetFrameTextureFrame",
        "TargetHighLevelTexture",
        "TargetLevelText",
        "TargetName",
        "TargetPortrait",
        "TargetPVPIcon",
        // PetFrame.xml
        "PetAttackModeTexture",
        "PetFrame",
        "PetFrameHappiness",
        "PetFrameHappinessTexture",
        "PetFrameHealthBar",
        "PetFrameHealthBarText",
        "PetFrameManaBar",
        "PetFrameManaBarText",
        "PetFrameTexture",
        "PetName",
        "PetPortrait",
    ] {
        if !s
            .eval::<bool>(&format!("return getglobal('{name}') ~= nil"))
            .unwrap()
        {
            missing.push(name);
        }
    }
    // The one genuine GAP, stated rather than quietly dropped from the list (1718's whole point
    // is that the block stays visible): `PlayerFrameGroupIndicator` and its four children are the
    // raid "Group N" tab above the player frame, driven by `PlayerFrame_UpdateGroupIndicator`
    // (ref PlayerFrame.lua:214-229). We build no region, no texture and no handler for it — this
    // is a feature we have not written, not a name we got wrong, and inventing five regions to
    // satisfy a list would be the worse error. Zero corpus readers.
    let unbuilt = [
        "PlayerFrameGroupIndicator",
        "PlayerFrameGroupIndicatorLeft",
        "PlayerFrameGroupIndicatorMiddle",
        "PlayerFrameGroupIndicatorRight",
        "PlayerFrameGroupIndicatorText",
    ];
    missing.retain(|m| !unbuilt.contains(m));
    assert!(
        missing.is_empty(),
        "the reference declares these and we do not publish them — an addon reading any of them \
         by name finds nil: {missing:?}"
    );

    // The colour table is `ManaBarColor` (ref UnitFrame.lua:2), not the later `PowerBarColor`,
    // and addons index it directly for the power-type prefix and tint.
    assert!(s.eval::<bool>("return ManaBarColor ~= nil").unwrap());
    assert!(
        s.eval::<bool>("return PowerBarColor == nil").unwrap(),
        "PowerBarColor is the later client's name — 1.12 has no such global"
    );

    // And the frame FIELD `UnitFrame_Initialize` sets (ref UnitFrame.lua:13-14), which unit-frame
    // addons read off the frame rather than by global name.
    assert!(
        s.eval::<bool>("return PlayerFrame.manabar ~= nil and PlayerFrame.healthbar ~= nil")
            .unwrap(),
        "the reference's field names are `manabar`/`healthbar`, both lowercase"
    );

    // ShaguTweaks' own line, in shape (health-numbers.lua:22).
    s.run("TargetFrameManaBar:SetStatusBarColor(0, 0, 1)")
        .unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// **The reported symptom, on the stock file that has it: the target's leader crown went stale.**
///
/// `PLAYER_FLAGS_CHANGED` was one of the events a migrated window registered and nothing here
/// produced. `TargetFrame.lua:88-95` is its only 1.12 consumer and it re-runs the
/// **party-leader icon** — *not* an AFK/DND badge, which 1.12 has nowhere on a unit frame and has
/// no `UnitIsAFK`/`UnitIsDND` binding to draw from. With no producer, `TargetFrame_Update` was the
/// crown's only writer, so it moved on a re-target and at no other moment: leadership passing to
/// the player you were already holding did nothing on screen.
///
/// This drives the real stock file, so it is the symptom itself and not a proxy for it.
#[test]
fn the_target_leader_crown_follows_player_flags_changed() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    let mate = |leader: bool| {
        Some(UnitState {
            exists: true,
            has_object: true,
            name: Some("Groupmate".into()),
            health: 100,
            max_health: 100,
            level: 30,
            power_type: 0,
            power: 50,
            max_power: 50,
            // `UnitIsPartyLeader`'s descriptor leg — PLAYER_FLAGS bit 0x1 — and the raw dword the
            // event fires on, moved together the way the wire moves them.
            group_leader: leader,
            player_flags: u32::from(leader),
            // **A NON-ZERO guid, or this test cannot fail.** `UnitIsPartyLeader` is two legs ORed
            // and the second is a raw compare against the group's leader guid, with no zero guard
            // — verified 1.12 behaviour, which is why `UnitIsPartyLeader(nil)` answers 1 while
            // solo (`unit::bindings`). A default `UnitState` carries `guid: 0`, which equals the
            // zeroed leader of an empty group, so the crown is up from the first frame and every
            // assertion below passes for the wrong reason. Naming a guid puts the descriptor leg
            // in sole charge, which is the leg this event exists to refresh.
            guid: 0x4000_0000_0000_0009,
            ..UnitState::default()
        })
    };
    let crown =
        |s: &UiScript| -> bool { s.eval::<bool>("return TargetLeaderIcon:IsShown()").unwrap() };

    // Target an ordinary group member: no crown.
    s.set_unit("target", mate(false));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(!crown(&s), "not the leader: no crown");

    // **Leadership passes to them while you hold the target.** This is the whole report.
    s.set_unit("target", mate(true));
    // The control FIRST: the same event for a different token must change nothing, because the
    // stock handler is gated on `arg1 == "target"`. A producer that fired argless — or that fired
    // the wrong token — would light the crown here and the assertion below would prove nothing.
    s.fire_event(
        "PLAYER_FLAGS_CHANGED",
        vec![ScriptValue::Str("player".into())],
    );
    assert!(
        !crown(&s),
        "arg1 = \"player\" is not this frame's unit — the token gate is real"
    );

    s.fire_event(
        "PLAYER_FLAGS_CHANGED",
        vec![ScriptValue::Str("target".into())],
    );
    assert!(crown(&s), "the crown appears without a re-target");

    // And back down — the reference fires on the XOR-diff, so the clear edge is the same event.
    s.set_unit("target", mate(false));
    s.fire_event(
        "PLAYER_FLAGS_CHANGED",
        vec![ScriptValue::Str("target".into())],
    );
    assert!(!crown(&s), "leadership leaves and the crown goes with it");

    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
