//! The target-of-target frame, driven end to end — the **reference's own**
//! `TargetofTargetFrame`, declared in `Interface\FrameXML\TargetFrame.xml` (l.515-680) and
//! driven by `TargetFrame.lua`, off the player's patch chain. Decision 1751 retired our
//! `assets/ui/UnitFrames.xml` transcription, so what is under test here is the stock file over
//! synthetic `"targettarget"` snapshots.
//!
//! Three things here are not ordinary unit-frame plumbing and are what most of these test:
//!
//! * **The visibility law is six gates deep** — a switch, five display modes, and the four unit
//!   tests the reference wraps them in — and a frame that gets any of them wrong is either always
//!   there or never there. The mode leg leaks by construction: its solo arm calls neither `Show`
//!   nor `Hide` when you are in a raid (ref TargetFrame.lua l.513-520), so the frame keeps
//!   whatever state it had. Our transcription closed that; the stock file is the end state, so the
//!   leak is now the behaviour under test.
//! * **Nothing on this frame is event-driven.** Its bars carry no `OnEvent` at all (ref
//!   TargetFrame.xml l.593-625 — the health bar has only `OnValueChanged`, the mana bar no
//!   `<Scripts>` block), so the `UNIT_HEALTH`/`UNIT_MANA` registrations
//!   `UnitFrameHealthBar_Initialize`/`UnitFrameManaBar_Initialize` make (ref UnitFrame.lua
//!   l.150-151, l.190-200) are dead, and so is the frame's own `UNIT_AURA` registration (ref
//!   TargetFrame.xml l.668) — `UnitFrame_OnEvent` handles three events and that is not one of
//!   them. `TargetofTarget_OnUpdate` → `TargetofTarget_Update` is the only driver of the name, the
//!   bars, the dead word, the portrait tint and the aura row. **That is why nearly every step in
//!   this file is a `tick`, not a `fire_event`.**
//! * **A token going away is silent.** The feed clears `"targettarget"` without an event, by the
//!   same convention `"target"` uses, so the frame's own events cannot take it down — that is what
//!   `TargetFrame_OnUpdate`'s one-compare reconcile is for, and it has a test.

use benilla_ui::script::{
    AuraState, PartyMemberInfo, PartyState, QuadContent, RaidMemberInfo, ScriptValue,
    SelectionRequest, UiScript, UnitState,
};

use super::test_ui::load_ui as load_xml;

/// A unit snapshot with a guid — the identity `UnitIsUnit` compares, which the "your target is
/// you" gate reads.
fn unit(name: &str, guid: u64, health: u32) -> UnitState {
    UnitState {
        exists: true,
        is_connected: true,
        name: Some(name.into()),
        guid,
        health,
        max_health: 100,
        level: 60,
        power_type: 0,
        power: 50,
        max_power: 100,
        ..UnitState::default()
    }
}

/// The frame's production load prefix, plus a player, a target and a target's target — the state
/// in which everything but the switch itself already allows the frame.
///
/// The tail is the target-aura tests' set and is here for the same reasons: `BuffFrame` defines
/// `DebuffTypeColor`, which the debuff row's dispel tint indexes, and it will not TICK without
/// `ActionBar`'s `TOOLTIP_UPDATE_TIME` — which almost every test in this file needs, because the
/// stock frame is driven from its OnUpdate and nowhere else (see the module doc).
fn load_tot() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\UnitPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\MainMenuBar.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionBarFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BonusActionBarFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(&s, "Interface\\FrameXML\\BuffFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UnitFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\CombatFeedback.xml");
    load_xml(&s, "Interface\\FrameXML\\PlayerFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\PartyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\TargetFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\PetFrame.xml");
    // The target-of-target pair's home is `OptionsFrame.xml` (the reference keeps them in
    // `UIOptionsFrame.lua`'s uvar defaults block, l.116-119). That file sits at the BOTTOM of our
    // manifest where the reference's sits near the top, so a test that loads the unit kit alone
    // has to state them itself — the same thing this harness already does for `DEAD`.
    s.run(r#"SHOW_TARGET_OF_TARGET = "0" SHOW_TARGET_OF_TARGET_STATE = "5""#)
        .unwrap();
    s.set_unit("player", Some(unit("Tri", 0x100, 100)));
    s.set_unit("target", Some(unit("Kobold Miner", 0x200, 80)));
    s.set_unit("targettarget", Some(unit("Tri", 0x100, 100)));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s
}

/// Turn the frame on the way the option row does — the write plus its `applyFunc`.
fn switch_on(s: &mut UiScript) {
    s.run(r#"SHOW_TARGET_OF_TARGET = "1" this = TargetofTargetFrame TargetofTarget_Update() this = nil"#)
        .unwrap();
}

fn shown(s: &mut UiScript) -> bool {
    s.eval::<bool>("return TargetofTargetFrame:IsShown() and true or false")
        .unwrap()
}

/// Every texture path the UI actually draws this frame (the pet-frame tests' render probe).
fn drawn(s: &mut UiScript) -> Vec<String> {
    s.resolve();
    s.extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Texture { path: Some(p), .. } => Some(p),
            _ => None,
        })
        .collect()
}

fn debuff(spell_id: u32, name: &str, debuff_type: Option<&str>) -> AuraState {
    AuraState {
        spell_id,
        name: Some(name.into()),
        icon: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
        count: 1,
        debuff_type: debuff_type.map(str::to_string),
        // No unit but yourself carries a duration on the 1.12 wire (decision 0257 B6).
        duration: 0.0,
        expiration_time: 0.0,
        helpful: false,
        cancelable: false,
        until_cancelled: false,
        channeled: false,
    }
}

/// A party of `n` others; `GetNumPartyMembers` is that list's length.
fn party(n: usize) -> PartyState {
    PartyState {
        members: (0..n)
            .map(|i| PartyMemberInfo {
                name: format!("Mate{i}"),
                guid: 0x300 + i as u64,
            })
            .collect(),
        ..PartyState::default()
    }
}

/// A raid of `n` (the roster INCLUDES the player, per `GetRaidRosterInfo`'s array).
fn raid(n: usize) -> PartyState {
    PartyState {
        raid: (0..n)
            .map(|i| RaidMemberInfo {
                name: format!("Raider{i}"),
                guid: 0x400 + i as u64,
                ..RaidMemberInfo::default()
            })
            .collect(),
        ..PartyState::default()
    }
}

/// The switch is the outer gate, and it ships OFF — 1.12's own default (`SHOW_TARGET_OF_TARGET`
/// = `"0"`, ref UIOptionsFrame.lua l.116). Everything else about the state already allows the
/// frame, so this is the switch alone.
#[test]
fn the_frame_ships_off_and_the_switch_is_what_shows_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<String>("return SHOW_TARGET_OF_TARGET").unwrap(),
        "0",
        "the shipped default is the reference's: off"
    );
    assert!(
        !shown(&mut s),
        "off means hidden with everything else ready"
    );

    switch_on(&mut s);
    assert!(shown(&mut s), "the switch alone brings it up");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The four unit gates the reference wraps the switch in (ref TargetofTarget_Update l.504): a
/// target, a target OF it, a target that is not you, and a target that is alive.
#[test]
fn the_four_unit_gates_each_take_the_frame_down() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    assert!(shown(&mut s));

    // No target of target.
    s.set_unit("targettarget", None);
    s.run("this = TargetofTargetFrame TargetofTarget_Update() this = nil")
        .unwrap();
    assert!(!shown(&mut s), "nothing to show");
    s.set_unit("targettarget", Some(unit("Tri", 0x100, 100)));

    // No target at all.
    s.set_unit("target", None);
    s.run("this = TargetofTargetFrame TargetofTarget_Update() this = nil")
        .unwrap();
    assert!(!shown(&mut s), "no target, no target's target");
    s.set_unit("target", Some(unit("Kobold Miner", 0x200, 80)));

    // The target is YOU: its target is your own target, and the frame would restate the frame it
    // hangs off.
    s.set_unit("target", Some(unit("Tri", 0x100, 100)));
    s.run("this = TargetofTargetFrame TargetofTarget_Update() this = nil")
        .unwrap();
    assert!(!shown(&mut s), "self-target: nothing to add");
    s.set_unit("target", Some(unit("Kobold Miner", 0x200, 80)));

    // A dead target.
    s.set_unit("target", Some(unit("Kobold Miner", 0x200, 0)));
    s.run("this = TargetofTargetFrame TargetofTarget_Update() this = nil")
        .unwrap();
    assert!(!shown(&mut s), "a corpse is fighting nobody");

    s.set_unit("target", Some(unit("Kobold Miner", 0x200, 80)));
    s.run("this = TargetofTargetFrame TargetofTarget_Update() this = nil")
        .unwrap();
    assert!(shown(&mut s), "and back");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The five display modes, each against solo / party / raid.
///
/// Mode 3 in a raid is not a rule the ladder states — it is the hole in it. The
/// `SHOW_TARGET_OF_TARGET_STATE == "3"` arm acts only while `GetNumRaidMembers() == 0` (ref
/// TargetFrame.lua l.513-520), so in a raid it calls neither Show nor Hide and the frame keeps
/// whatever it had. It reads `false` here because the preceding `party` step left it hidden, and
/// that sequencing is the only reason. The leak itself is
/// [`the_solo_mode_keeps_the_frame_when_a_raid_forms`], which shows the frame first.
#[test]
fn the_five_modes_answer_solo_party_and_raid() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);

    let states = [
        ("solo", PartyState::default()),
        ("party", party(2)),
        ("raid", raid(10)),
    ];
    // mode -> (solo, party, raid)
    let expected = [
        ("1", [false, false, true]), // raid only
        ("2", [false, true, false]), // party, and not while raiding
        ("3", [true, false, false]), // solo only
        ("4", [false, true, true]),  // grouped at all
        ("5", [true, true, true]),   // always
    ];
    for (mode, want) in expected {
        for (i, (label, state)) in states.iter().enumerate() {
            s.set_party(state.clone());
            s.run(&format!(
                r#"SHOW_TARGET_OF_TARGET_STATE = "{mode}" this = TargetofTargetFrame TargetofTarget_Update() this = nil"#
            ))
            .unwrap();
            assert_eq!(
                shown(&mut s),
                want[i],
                "mode {mode} while {label}: expected shown={}",
                want[i]
            );
        }
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Mode 3 in a raid, as a STATE question rather than a fresh evaluation — the leaked branch, which
/// can only be seen by showing the frame first.
///
/// **This assertion flipped with 1751.** Our deleted `UnitFrames.xml` answered the question the
/// option asks and took the frame down; the stock file does not. `TargetofTarget_Update`'s
/// `SHOW_TARGET_OF_TARGET_STATE == "3"` arm is `if ( GetNumRaidMembers() == 0 ) then … end` with
/// no `else` (ref TargetFrame.lua l.513-520), so in a raid it reaches neither `Show` nor `Hide`
/// and the frame keeps whatever it had. The stock file is the end state, so the reference's answer
/// is the assertion.
///
/// Driven by a `tick`, which is the strong form: `TargetofTarget_OnUpdate` re-runs
/// `TargetofTarget_Update` on **every** frame (ref TargetFrame.xml l.673-675 → TargetFrame.lua
/// l.500), so this is not one stale evaluation left standing — the reference re-asks the question
/// sixty times a second and still never takes the frame down.
#[test]
fn the_solo_mode_keeps_the_frame_when_a_raid_forms() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    s.run(r#"SHOW_TARGET_OF_TARGET_STATE = "3" this = TargetofTargetFrame TargetofTarget_Update() this = nil"#)
        .unwrap();
    assert!(shown(&mut s), "solo, in solo mode");

    s.set_party(raid(10));
    s.tick(0.016);
    assert!(
        shown(&mut s),
        "the raid forms and the reference's solo arm never fires — the frame stays"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The paint: name, health, and the powerless unit's mana rail — which the reference **never
/// hides**.
///
/// **Two things flipped with 1751**, and both are the stock file being the end state:
///
/// * **The name arrives on a tick, not on the switch.** `TargetofTarget_Update` (ref
///   TargetFrame.lua l.503-540) touches the bars, the dead word and the aura rows and never the
///   name; the `SetText` lives in `TargetofTarget_OnUpdate`'s `CURRENT_TARGETTARGET` compare
///   (l.494-501), off the frame's own OnUpdate. Turning the switch on alone leaves it nil.
/// * **A powerless unit still gets a rail.** `UnitFrameManaBar_Update` (ref UnitFrame.lua
///   l.203-224) sets 0..0 and calls neither `Show` nor `Hide` — there is no hide leg anywhere in
///   the reference's mana path. Our deleted `UnitFrames.xml` hid the bar and this asserted that;
///   the stock answer is an empty 0/0 rail, still drawn, so that is what is asserted now.
///
/// The powerless step is a `tick` for the same reason as everything else here: 1.12 has no
/// `UNIT_MAXPOWER` event at all (the reference registers `UNIT_MAXMANA`/`RAGE`/`FOCUS`/`ENERGY`/
/// `HAPPINESS`, ref UnitFrame.lua l.195-199), and this frame's mana bar carries no `OnEvent` to
/// answer one with if it did (ref TargetFrame.xml l.612-625).
#[test]
fn the_frame_paints_its_unit() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    s.tick(0.016);

    assert_eq!(
        s.eval::<String>("return TargetofTargetName:GetText()")
            .unwrap(),
        "Tri"
    );
    let (value, max) = s
        .eval::<(f64, f64)>(
            "local _, m = TargetofTargetHealthBar:GetMinMaxValues() \
             return TargetofTargetHealthBar:GetValue(), m",
        )
        .unwrap();
    assert_eq!((value, max), (100.0, 100.0));
    assert!(
        s.eval::<bool>("return TargetofTargetManaBar:IsShown() and true or false")
            .unwrap(),
        "a unit with mana shows its rail"
    );

    let mut powerless = unit("Skeleton", 0x100, 100);
    powerless.max_power = 0;
    s.set_unit("targettarget", Some(powerless));
    s.tick(0.016);
    assert!(
        s.eval::<bool>("return TargetofTargetManaBar:IsShown() and true or false")
            .unwrap(),
        "no power, and the rail is STILL drawn — the reference has no hide leg"
    );
    assert_eq!(
        s.eval::<(f64, f64)>(
            "local _, m = TargetofTargetManaBar:GetMinMaxValues() \
             return TargetofTargetManaBar:GetValue(), m",
        )
        .unwrap(),
        (0.0, 0.0),
        "it reads 0/0"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// DEAD over a dimmed trough — and the reference's connected test, which is what tells a corpse
/// from a linkdead player (both read zero health). Ref `TargetofTarget_CheckDead`, TargetFrame.lua
/// l.559-567.
///
/// Every step is a `tick` rather than a `UNIT_HEALTH`: `TargetofTargetHealthBar` registers that
/// event through `UnitFrameHealthBar_Initialize` (ref UnitFrame.lua l.150) but declares **no**
/// `OnEvent` to answer it with (ref TargetFrame.xml l.593-611 — `OnValueChanged` and nothing
/// else), so the registration is dead and `TargetofTarget_Update` off the frame's OnUpdate is the
/// only thing that ever calls `TargetofTarget_CheckDead`.
#[test]
fn the_dead_word_needs_a_connected_corpse() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    s.tick(0.016);
    assert!(
        !s.eval::<bool>("return TargetofTargetDeadText:IsShown() and true or false")
            .unwrap(),
        "alive: no word"
    );

    let mut corpse = unit("Tri", 0x100, 0);
    corpse.dead = true;
    s.set_unit("targettarget", Some(corpse.clone()));
    s.tick(0.016);
    assert!(
        s.eval::<bool>("return TargetofTargetDeadText:IsShown() and true or false")
            .unwrap(),
        "dead: the word"
    );

    let mut linkdead = corpse;
    linkdead.is_connected = false;
    s.set_unit("targettarget", Some(linkdead));
    s.tick(0.016);
    assert!(
        !s.eval::<bool>("return TargetofTargetDeadText:IsShown() and true or false")
            .unwrap(),
        "disconnected, not dead"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The portrait tint (ref `TargetofTargetHealthCheck`, TargetFrame.lua l.569-585), and its
/// PLAYERS-ONLY gate: a creature's portrait is never tinted, which is why the check has to run off
/// the bar's own value rather than off the snapshot.
///
/// Ticks, not `UNIT_HEALTH`, and here the chain is one link longer than the dead word's: the check
/// hangs off the health bar's `OnValueChanged` (ref TargetFrame.xml l.604-608), and the only thing
/// that moves that value is `UnitFrameHealthBar_Update` inside `TargetofTarget_Update` — which is
/// reached from the frame's OnUpdate and nowhere else.
#[test]
fn the_portrait_tints_with_a_players_state_and_never_a_creatures() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    s.tick(0.016);

    let mut hurt = unit("Tri", 0x100, 15);
    hurt.is_player = true;
    s.set_unit("targettarget", Some(hurt));
    s.tick(0.016);
    let (r, g, b) = s
        .eval::<(f64, f64, f64)>("return TargetofTargetPortrait:GetVertexColor()")
        .unwrap();
    assert_eq!((r, g, b), (1.0, 0.0, 0.0), "a player under a fifth: red");

    let mut healthy = unit("Tri", 0x100, 90);
    healthy.is_player = true;
    s.set_unit("targettarget", Some(healthy));
    s.tick(0.016);
    let (r, g, b) = s
        .eval::<(f64, f64, f64)>("return TargetofTargetPortrait:GetVertexColor()")
        .unwrap();
    assert_eq!((r, g, b), (1.0, 1.0, 1.0), "and white again");

    // A creature at the same 15%: the tint must not move (the check returns before touching it).
    s.set_unit("targettarget", Some(unit("Kobold", 0x500, 15)));
    s.tick(0.016);
    let (r, g, b) = s
        .eval::<(f64, f64, f64)>("return TargetofTargetPortrait:GetVertexColor()")
        .unwrap();
    assert_eq!((r, g, b), (1.0, 1.0, 1.0), "creatures carry no reading");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The left click selects — the one leg the target frame's own click does not have, and the whole
/// reason the frame is clickable.
#[test]
fn the_left_click_targets_the_unit() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    s.run(r#"TargetofTarget_OnClick("LeftButton")"#).unwrap();
    assert_eq!(
        s.take_selection_requests(),
        vec![SelectionRequest::Unit("targettarget".into())]
    );

    // A right click is not a menu here: the reference gives this frame none.
    s.run(r#"TargetofTarget_OnClick("RightButton")"#).unwrap();
    assert!(s.take_selection_requests().is_empty());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The four debuff buttons, dispel-tinted (ref `RefreshBuffs` over `MAX_PARTY_DEBUFFS`,
/// BuffFrame.lua l.262-313, called from `TargetofTarget_Update` l.538).
///
/// The `UNIT_AURA` fire is what the feed really does and is kept for that reason, but it is inert
/// here: the frame registers the event in its own OnLoad (ref TargetFrame.xml l.668) and answers
/// it with `UnitFrame_OnEvent`, which handles `UNIT_NAME_UPDATE`, `UNIT_PORTRAIT_UPDATE` and
/// `UNIT_DISPLAYPOWER` and nothing else (ref UnitFrame.lua l.31-45). A dead registration in the
/// stock file — the row is redrawn by `RefreshBuffs` off the OnUpdate, so the `tick` is what makes
/// it appear.
#[test]
fn the_debuff_row_draws_what_the_unit_carries() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    s.set_auras(
        "targettarget",
        Some(vec![
            debuff(1000, "Rend", None),
            debuff(1001, "Curse of Agony", Some("Curse")),
        ]),
    );
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("targettarget".into())]);
    s.tick(0.016);

    let paths = drawn(&mut s);
    assert!(
        paths.iter().any(|p| p == "Interface\\Icons\\Spell_1000"),
        "the first debuff's icon draws: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p == "Interface\\Icons\\Spell_1001"),
        "and the second"
    );
    assert!(
        !s.eval::<bool>("return TargetofTargetFrameDebuff3:IsShown() and true or false")
            .unwrap(),
        "the empty slots stay down"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The target's OWN aura rows re-wrap around this frame: 5 to a row instead of 6, and the buffs
/// open under the second debuff row instead of the seventh icon (ref `TargetDebuffButton_Update`,
/// TargetFrame.lua l.312-338 for the wrap and l.370-385 for the row anchors).
///
/// Both directions, and they are not symmetric: the reference re-runs the rows on the way **in**
/// only. `TargetDebuffButton_Update` is called from inside `TargetofTarget_Update`'s
/// `if ( TargetofTargetFrame:IsShown() )` (l.532-537), so turning the frame off leaves the rows
/// wrapped short behind it.
#[test]
fn the_target_rows_wrap_short_while_the_frame_stands_beside_them() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    // A hostile target, so the debuffs lead and the buffs hang off them.
    s.set_auras("target", Some(vec![debuff(2000, "Sunder", None)]));
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("target".into())]);

    let anchor = |s: &mut UiScript, frame: &str| -> String {
        s.eval::<String>(&format!(
            "local _, rel = {frame}:GetPoint(1) return rel:GetName()"
        ))
        .unwrap()
    };
    assert_eq!(
        anchor(&mut s, "TargetFrameBuff1"),
        "TargetFrameDebuff7",
        "hidden: the sixth icon closes the first row"
    );

    switch_on(&mut s);
    assert!(shown(&mut s));
    assert_eq!(
        anchor(&mut s, "TargetFrameBuff1"),
        "TargetFrameDebuff6",
        "shown: the row wraps a slot early"
    );
    assert_eq!(
        anchor(&mut s, "TargetFrameDebuff6"),
        "TargetFrameDebuff1",
        "and the sixth icon starts the second row"
    );

    // Back off — and the rows STAY wrapped short. **This assertion flipped with 1751.**
    // `TargetofTarget_Update` calls `TargetDebuffButton_Update` from inside
    // `if ( TargetofTargetFrame:IsShown() )` (ref TargetFrame.lua l.532-537), so the pass that
    // hides the frame is exactly the pass that does not re-lay the target's rows. They keep the
    // 5-wide wrap until something else runs the update — the next target change or `UNIT_AURA`.
    // Our deleted `UnitFrames.xml` re-laid them on the way out; the stock file is the end state.
    s.run(r#"SHOW_TARGET_OF_TARGET = "0" this = TargetofTargetFrame TargetofTarget_Update() this = nil"#)
        .unwrap();
    assert_eq!(
        anchor(&mut s, "TargetFrameBuff1"),
        "TargetFrameDebuff6",
        "the frame goes and the rows are left wrapped short behind it"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The reconcile. A token going away is CLEARED, not announced — no UNIT_* event carries it — so
/// the frame's own registrations cannot take it down. `TargetFrame_OnUpdate` is what does, and it
/// runs only while you have a target.
#[test]
fn the_reconcile_takes_the_frame_down_when_the_token_goes_silent() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    assert!(shown(&mut s));

    // Exactly what the feed does: clear the token, fire nothing.
    s.set_unit("targettarget", None);
    assert!(shown(&mut s), "no event, so nothing has told the frame yet");

    s.tick(0.016);
    assert!(!shown(&mut s), "the reconcile notices within a frame");

    // And the other direction: a token appearing DOES fire, but the reconcile covers it anyway.
    s.set_unit("targettarget", Some(unit("Tri", 0x100, 100)));
    s.tick(0.016);
    assert!(shown(&mut s), "and brings it back");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
