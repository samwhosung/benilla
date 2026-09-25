//! The stock buff bar (`BuffFrame.xml`) under a stubbed aura feed. A button's `id` is an ordinal
//! within its filter; `GetPlayerBuff` maps it to the cache position every other buff verb takes.

use benilla_ui::script::{AuraState, ExtractedQuad, QuadContent, UiScript};

use super::test_ui::load_ui as load_xml;

fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The one-letter duration strings, which `SecondsToTimeAbbrev` formats unguarded.
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // `BuffButton_Update` asks `GameTooltip:IsOwned(this)` unguarded (`BuffFrame.lua:104`).
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    // `SecondsToTimeAbbrev` comes with `UIParent.xml` (`UIParent.lua:1034`).
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(&s, "Interface\\FrameXML\\MainMenuBar.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionBarFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BonusActionBarFrame.xml");
    // Timers on for these tests; the 1.12 default is "0" (`UIOptionsFrame.lua:104`).
    s.run("SHOW_BUFF_DURATIONS = \"1\"").unwrap();
    load_xml(&s, "Interface\\FrameXML\\BuffFrame.xml");
    // As the app does: `BuffFrame_OnLoad` never calls `BuffButtons_UpdatePositions`, which 1.12
    // runs on `VARIABLES_LOADED` (`UIOptionsFrame.lua:206`).
    super::manifest::apply_buff_durations(&s).unwrap();
    s
}

/// An [`AuraState`]: `expiration_time` 0 is permanent, and the harness's `GetTime()` starts at 0.
fn aura(
    spell_id: u32,
    name: &str,
    icon: &str,
    helpful: bool,
    count: u8,
    debuff_type: Option<&str>,
    expiration_time: f64,
    cancelable: bool,
) -> AuraState {
    AuraState {
        spell_id,
        name: Some(name.into()),
        icon: Some(icon.into()),
        count,
        debuff_type: debuff_type.map(Into::into),
        duration: expiration_time,
        expiration_time,
        helpful,
        cancelable,
        // The feed takes this from the spell (`ui_aura::until_cancelled`); here, from the expiry.
        until_cancelled: expiration_time == 0.0,
        channeled: false,
    }
}

/// Push the list and fire `PLAYER_AURAS_CHANGED`, as the app's feed does (`BuffFrame.lua:113`).
fn push(s: &mut UiScript, auras: Vec<AuraState>) {
    s.set_auras("player", Some(auras));
    s.fire_event("PLAYER_AURAS_CHANGED", vec![]);
}

/// One tick of the app's real order (OnUpdate → resolve → draw list), returning the quads.
fn frame(s: &mut UiScript, dt: f32) -> Vec<ExtractedQuad> {
    s.tick(dt);
    s.resolve();
    s.extract()
}

fn shown(s: &UiScript, name: &str) -> bool {
    s.eval::<bool>(&format!("return {name}:IsShown()")).unwrap()
}

fn text(s: &UiScript, name: &str) -> String {
    s.eval::<String>(&format!("return tostring(({name}:GetText()) or \"\")"))
        .unwrap()
}

fn alpha(s: &UiScript, name: &str) -> f64 {
    s.eval::<f64>(&format!("return {name}:GetAlpha()")).unwrap()
}

/// The first texture quad whose path ends in `leaf`.
fn tex_quad<'a>(quads: &'a [ExtractedQuad], leaf: &str) -> Option<&'a ExtractedQuad> {
    quads.iter().find(|q| match &q.content {
        QuadContent::Texture { path: Some(p), .. } => p.ends_with(leaf),
        _ => false,
    })
}

#[test]
fn buffs_and_debuffs_fill_their_own_rows_with_counts_and_the_dispel_tint() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(
        &mut s,
        vec![
            aura(
                1126,
                "Mark of the Wild",
                "Interface\\Icons\\Spell_Nature_Regeneration",
                true,
                1,
                None,
                120.0,
                true,
            ),
            aura(
                589,
                "Shadow Word: Pain",
                "Interface\\Icons\\Spell_Shadow_ShadowWordPain",
                false,
                3,
                Some("Magic"),
                18.0,
                false,
            ),
        ],
    );
    let quads = frame(&mut s, 0.1);

    assert!(shown(&s, "BuffButton0"), "first buff -> BuffButton0");
    assert!(!shown(&s, "BuffButton1"), "no second buff");
    assert!(shown(&s, "BuffButton16"), "first debuff -> BuffButton16");
    assert!(!shown(&s, "BuffButton17"), "no second debuff");

    assert!(
        tex_quad(&quads, "Spell_Nature_Regeneration").is_some(),
        "buff icon drew"
    );
    assert!(
        tex_quad(&quads, "Spell_Shadow_ShadowWordPain").is_some(),
        "debuff icon drew"
    );

    // `DebuffTypeColor["Magic"]` is 0.20, 0.60, 1.00 (`BuffFrame.lua:11`).
    let border = tex_quad(&quads, "UI-Debuff-Overlays").expect("debuff border drew");
    match &border.content {
        QuadContent::Texture {
            color: Some(c),
            tex_coords: Some(uv),
            ..
        } => {
            assert!(
                (c[0] - 0.20).abs() < 1e-3
                    && (c[1] - 0.60).abs() < 1e-3
                    && (c[2] - 1.00).abs() < 1e-3,
                "Magic tint, got {c:?}"
            );
            // The overlay crop of `BuffFrame.xml:72`.
            let uv = uv.edges();
            assert!(
                (uv[0] - 0.296875).abs() < 1e-6 && (uv[3] - 0.515625).abs() < 1e-6,
                "debuff overlay tex-coords, got {uv:?}"
            );
        }
        other => panic!("border is not a tinted, cropped texture: {other:?}"),
    }

    // The count shows only above 1 (`BuffFrame.lua:97`).
    assert_eq!(text(&s, "BuffButton16Count"), "3");
    assert_eq!(text(&s, "BuffButton0Count"), "");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn a_timed_aura_counts_down_and_a_permanent_one_shows_no_timer() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(
        &mut s,
        vec![
            aura(
                2457,
                "Battle Stance",
                "Interface\\Icons\\Ability_Warrior_OffensiveStance",
                true,
                1,
                None,
                0.0,
                false,
            ),
            // Timed, 120 s out: 119.9 s left after the tick, which rounds up to "2 m".
            aura(
                1126,
                "Mark of the Wild",
                "Interface\\Icons\\Spell_Nature_Regeneration",
                true,
                1,
                None,
                120.0,
                true,
            ),
        ],
    );
    frame(&mut s, 0.1);

    assert!(
        !shown(&s, "BuffButton0Duration"),
        "a permanent aura shows no timer (our 'until cancelled')"
    );
    assert!(shown(&s, "BuffButton1Duration"));
    assert_eq!(
        text(&s, "BuffButton1Duration"),
        "2 m",
        "120s remaining abbreviates to minutes"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn the_warning_flash_pulses_a_low_aura_but_leaves_a_long_one_solid() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(
        &mut s,
        vec![
            // 5 min out: never inside the 31 s `BUFF_WARNING_TIME` window, so full alpha.
            aura(
                1,
                "Long",
                "Interface\\Icons\\INV_Misc_QuestionMark",
                true,
                1,
                None,
                300.0,
                true,
            ),
            aura(
                2,
                "Short",
                "Interface\\Icons\\INV_Misc_QuestionMark",
                true,
                1,
                None,
                20.0,
                true,
            ),
        ],
    );

    let (mut short_min, mut long_min) = (2.0_f64, 2.0_f64);
    for _ in 0..20 {
        s.tick(0.1);
        short_min = short_min.min(alpha(&s, "BuffButton1"));
        long_min = long_min.min(alpha(&s, "BuffButton0"));
    }
    assert!(
        short_min < 0.9,
        "the 20s aura's icon pulses toward BUFF_MIN_ALPHA (min seen {short_min})"
    );
    assert!(
        (long_min - 1.0).abs() < 1e-9,
        "the 5min aura's icon stays solid (min seen {long_min})"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn right_clicking_a_cancelable_buff_queues_its_spell_cancel() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(
        &mut s,
        vec![aura(
            1126,
            "Mark of the Wild",
            "Interface\\Icons\\Spell_Nature_Regeneration",
            true,
            1,
            None,
            120.0,
            true,
        )],
    );
    frame(&mut s, 0.1);

    assert!(
        s.take_cancel_aura_requests().is_empty(),
        "nothing queued yet"
    );
    // `CancelPlayerBuff(this.buffIndex)`; the app sends `CMSG_CANCEL_AURA` by spell id.
    s.eval::<()>("this = BuffButton0; BuffButton_OnClick()")
        .unwrap();
    assert_eq!(s.take_cancel_aura_requests(), vec![1126]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn an_emptied_bar_hides_every_button() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(
        &mut s,
        vec![aura(
            1126,
            "Mark of the Wild",
            "Interface\\Icons\\Spell_Nature_Regeneration",
            true,
            1,
            None,
            120.0,
            true,
        )],
    );
    frame(&mut s, 0.1);
    assert!(shown(&s, "BuffButton0"), "shown while the aura is up");

    // An empty list and a re-fire hide the button and its timer (`BuffFrame.lua:64-67`).
    push(&mut s, vec![]);
    frame(&mut s, 0.1);
    assert!(!shown(&s, "BuffButton0"), "hidden once the aura is gone");
    assert!(!shown(&s, "BuffButton0Duration"));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The countdown polls `GetPlayerBuffTimeLeft` every frame (`BuffFrame.lua:129-130`), so a refresh
/// (recast, pushback, the replay at world entry), which fires no aura event, still reaches the bar.
#[test]
fn a_refreshed_duration_reaches_the_bar_with_no_aura_event() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    let mark = |expiry: f64| {
        aura(
            1126,
            "Mark of the Wild",
            "Interface\\Icons\\Spell_Nature_Regeneration",
            true,
            1,
            None,
            expiry,
            true,
        )
    };
    // 20 s out: 19.9 s after the tick, which `%d` truncates to "19 s".
    push(&mut s, vec![mark(20.0)]);
    frame(&mut s, 0.1);
    assert_eq!(text(&s, "BuffButton0Duration"), "19 s");

    // The refresh: a new expiry and no event.
    s.set_auras("player", Some(vec![mark(300.0)]));
    frame(&mut s, 0.1);
    assert_eq!(
        text(&s, "BuffButton0Duration"),
        "5 m",
        "the countdown re-reads the aura every frame; it does not wait for an event"
    );
    assert!(
        (alpha(&s, "BuffButton0") - 1.0).abs() < 1e-6,
        "no longer inside the 31s warning window, so full alpha"
    );

    // A fresh apply, stamp a frame behind the icon: `untilCancelled` comes from the spell, so a
    // timed aura with no stamp yet shows "0 s" and pulses until the stamp joins.
    let mut pending = mark(0.0);
    pending.until_cancelled = false; // a timed spell, stamp not yet arrived
    s.set_auras("player", Some(vec![pending]));
    frame(&mut s, 0.1);
    assert_eq!(
        text(&s, "BuffButton0Duration"),
        "0 s",
        "the reference draws the floor of a timed aura it has no stamp for, not a blank"
    );
    s.set_auras("player", Some(vec![mark(60.0)])); // the stamp joins, still no event
    frame(&mut s, 0.1);
    assert_eq!(
        text(&s, "BuffButton0Duration"),
        "59 s",
        "a duration that lands after the icon still reaches the bar"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `SHOW_BUFF_DURATIONS` "0" hides the timers and closes each row's 15px gutter to 5px
/// (`BuffFrame.lua:152-160`); the warning pulse runs regardless of it (`BuffFrame.lua:131-135`).
#[test]
fn the_duration_switch_hides_the_timers_and_closes_their_gutter() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(
        &mut s,
        vec![aura(
            1126,
            "Mark of the Wild",
            "Interface\\Icons\\Spell_Nature_Regeneration",
            true,
            1,
            None,
            20.0, // inside the 31 s warning window, so the flash runs either way
            true,
        )],
    );
    frame(&mut s, 0.1);
    assert!(
        shown(&s, "BuffButton0Duration"),
        "timers on — the harness planted the switch"
    );
    assert_eq!(text(&s, "BuffButton0Duration"), "19 s");
    s.resolve();
    let shown_gap = s
        .eval::<f64>("return BuffButton0:GetBottom() - BuffButton8:GetTop()")
        .unwrap();
    assert!((shown_gap - 15.0).abs() < 1e-3, "gutter: {shown_gap}");

    s.run("SHOW_BUFF_DURATIONS = \"0\"; BuffButtons_UpdatePositions()")
        .unwrap();
    frame(&mut s, 0.1);
    assert!(!shown(&s, "BuffButton0Duration"), "no timer drawn");
    assert!(shown(&s, "BuffButton0"), "the icon stays");
    s.resolve();
    let hidden_gap = s
        .eval::<f64>("return BuffButton0:GetBottom() - BuffButton8:GetTop()")
        .unwrap();
    assert!((hidden_gap - 5.0).abs() < 1e-3, "gutter: {hidden_gap}");
    assert!(
        alpha(&s, "BuffButton0") < 1.0,
        "the last-31s pulse is not gated on the timers"
    );

    s.run("SHOW_BUFF_DURATIONS = \"1\"; BuffButtons_UpdatePositions()")
        .unwrap();
    frame(&mut s, 0.1);
    assert!(shown(&s, "BuffButton0Duration"));
    assert_eq!(text(&s, "BuffButton0Duration"), "19 s");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Positions 0 to 4 of one cache: buff, debuff (Magic, 3), buff, debuff (Poison, 5), a stance.
fn mixed_bar() -> Vec<AuraState> {
    vec![
        aura(
            1126,
            "Mark of the Wild",
            "Interface\\Icons\\A",
            true,
            1,
            None,
            120.0,
            true,
        ),
        aura(
            589,
            "Shadow Word: Pain",
            "Interface\\Icons\\B",
            false,
            3,
            Some("Magic"),
            18.0,
            false,
        ),
        aura(
            1459,
            "Arcane Intellect",
            "Interface\\Icons\\C",
            true,
            1,
            None,
            600.0,
            true,
        ),
        aura(
            2818,
            "Deadly Poison",
            "Interface\\Icons\\D",
            false,
            5,
            Some("Poison"),
            12.0,
            false,
        ),
        aura(
            2457,
            "Battle Stance",
            "Interface\\Icons\\E",
            true,
            1,
            None,
            0.0,
            false,
        ),
    ]
}

/// A button's `id` is an ordinal within its filter and `GetPlayerBuff` maps it to a position
/// shared across filters. The addon walk `while GetPlayerBuff(i) >= 0 do` stops on `-1`, not nil,
/// and under the default `HELPFUL|HARMFUL` filter it sees the debuffs too.
#[test]
fn the_buttons_and_the_corpus_walk_agree_on_the_cache_positions() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(&mut s, mixed_bar());
    frame(&mut s, 0.1);

    // Unfiltered, the walk visits all five, each position equal to its ordinal.
    let visited = s
        .eval::<i64>(
            r#"local i = 0
               while GetPlayerBuff(i) >= 0 do
                   assert((GetPlayerBuff(i)) == i, "unfiltered walk is position-identical")
                   i = i + 1
                   assert(i < 100, "GetPlayerBuff walk did not terminate")
               end
               return i"#,
        )
        .unwrap();
    assert_eq!(visited, 5, "three buffs and two debuffs, one cache");

    // Each button caches the position, not its ordinal.
    for (button, position) in [
        ("BuffButton0", 0),  // helpful ordinal 0 -> position 0
        ("BuffButton1", 2),  // helpful ordinal 1 -> position 2 (the debuff at 1 is skipped)
        ("BuffButton2", 4),  // helpful ordinal 2 -> position 4
        ("BuffButton16", 1), // harmful ordinal 0 -> position 1
        ("BuffButton17", 3), // harmful ordinal 1 -> position 3
    ] {
        assert!(shown(&s, button), "{button} draws its aura");
        assert_eq!(
            s.eval::<i64>(&format!("return {button}.buffIndex"))
                .unwrap(),
            position,
            "{button} caches the CACHE POSITION, not its per-filter ordinal"
        );
    }
    assert!(!shown(&s, "BuffButton3"), "only three buffs");
    assert!(!shown(&s, "BuffButton18"), "only two debuffs");
    assert_eq!(
        s.eval::<i64>("return BuffButton3.buffIndex").unwrap(),
        -1,
        "a miss is -1, the sentinel the corpus terminates on — never nil"
    );

    assert_eq!(text(&s, "BuffButton16Count"), "3");
    assert_eq!(text(&s, "BuffButton17Count"), "5");
    assert_eq!(
        text(&s, "BuffButton0Count"),
        "",
        "a single stack shows nothing"
    );
    let quads = frame(&mut s, 0.1);
    let poison = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture {
                path: Some(p),
                color: Some(c),
                ..
            } if p.ends_with("UI-Debuff-Overlays") => Some(*c),
            _ => None,
        })
        // `DebuffTypeColor["Poison"]` is 0.00, 0.60, 0.00 (`BuffFrame.lua:14`).
        .any(|c| c[0].abs() < 1e-3 && (c[1] - 0.60).abs() < 1e-3 && c[2].abs() < 1e-3);
    assert!(poison, "the second debuff button wears the Poison tint");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `BuffButton_OnClick` is `CancelPlayerBuff(this.buffIndex)` (`BuffFrame.lua:149`): it cancels by
/// position, and a stance or a debuff queues nothing.
#[test]
fn right_clicking_cancels_the_aura_under_the_cursor_by_its_cache_position() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(&mut s, mixed_bar());
    frame(&mut s, 0.1);
    assert!(s.take_cancel_aura_requests().is_empty());

    // Ordinal 1 is position 2 (spell 1459); by ordinal it would cancel 589, the debuff at 1.
    s.eval::<()>("this = BuffButton1; BuffButton_OnClick()")
        .unwrap();
    assert_eq!(
        s.take_cancel_aura_requests(),
        vec![1459],
        "the aura under the cursor, not the record at the ordinal"
    );

    for button in ["BuffButton2", "BuffButton16", "BuffButton17", "BuffButton7"] {
        s.eval::<()>(&format!("this = {button}; BuffButton_OnClick()"))
            .unwrap();
    }
    assert!(
        s.take_cancel_aura_requests().is_empty(),
        "a stance, a debuff and an empty button are all no-ops"
    );

    s.eval::<()>("this = BuffButton0; BuffButton_OnClick()")
        .unwrap();
    assert_eq!(s.take_cancel_aura_requests(), vec![1126]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The temporary-enchant row (`BuffFrame.lua:162-233`): idle, it hides both slots and parks the
/// bar; enchanted, it counts down from milliseconds and slides the top buff row left.
#[test]
fn the_temporary_enchant_row_shows_a_weapon_enchant_and_moves_the_top_row_aside() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    // Auras up, so one leaking into an enchant slot would show.
    push(&mut s, mixed_bar());
    frame(&mut s, 0.1);

    assert!(
        !shown(&s, "TempEnchant1"),
        "the enchant slots are not buff buttons: no aura may reach them"
    );
    assert!(!shown(&s, "TempEnchant2"));
    // `BuffFrame.xml:101-109` blanks three inherited handlers with whitespace bodies, which 1.12
    // compiles to empty functions, not nil: `0x7025c0` clears only a NULL or empty body.
    for handler in ["OnLoad", "OnEvent", "OnClick"] {
        assert!(
            s.eval::<bool>(&format!(
                "return type(TempEnchant1:GetScript(\"{handler}\")) == \"function\""
            ))
            .unwrap(),
            "an enchant slot's {handler} is the reference's compiled no-op, not nil"
        );
    }
    // Nor the inherited body, which would paint the first buff into the slot and arm its cancel.
    assert!(
        s.eval::<bool>(
            "return TempEnchant1:GetScript(\"OnLoad\") ~= BuffButton0:GetScript(\"OnLoad\")"
        )
        .unwrap(),
        "the blank displaces the inherited handler rather than sharing it"
    );
    s.resolve();
    let resting = s.eval::<f64>("return BuffFrame:GetRight()").unwrap();
    let row2 = s.eval::<f64>("return BuffButton16:GetRight()").unwrap();
    assert!(
        (resting - (1024.0 - 175.0)).abs() < 1e-3,
        "UIParent TOPRIGHT -175: {resting}"
    );

    s.set_weapon_enchants(
        Some(benilla_ui::script::WeaponEnchant {
            remaining_ms: Some(480_000),
            charges: 0,
        }),
        None,
    );
    frame(&mut s, 0.1);
    assert!(
        shown(&s, "TempEnchant1"),
        "the enchanted weapon takes slot 1"
    );
    assert!(!shown(&s, "TempEnchant2"));
    assert_eq!(
        s.eval::<i64>("return TempEnchant1:GetID()").unwrap(),
        16,
        "the live-API MainHandSlot id, which is what its item tooltip hover needs"
    );
    assert_eq!(
        text(&s, "TempEnchant1Duration"),
        "8 m",
        "480000 ms / 1000 -> 480 s -> SecondsToTimeAbbrev"
    );

    // The debuff row stays only because `BuffButtons_UpdatePositions` re-anchored it to
    // `TemporaryEnchantFrame`; its XML anchor is inside `BuffFrame`, which moves.
    s.resolve();
    let shifted = s.eval::<f64>("return BuffFrame:GetRight()").unwrap();
    assert!(
        (resting - shifted - 37.0).abs() < 1e-3,
        "one 32px enchant column + the 5px gutter: {resting} -> {shifted}"
    );
    assert!(
        (s.eval::<f64>("return BuffButton16:GetRight()").unwrap() - row2).abs() < 1e-3,
        "the debuff row hangs off TemporaryEnchantFrame and stays put: {row2} -> {}",
        s.eval::<f64>("return BuffButton16:GetRight()").unwrap()
    );

    s.set_weapon_enchants(None, None);
    frame(&mut s, 0.1);
    assert!(!shown(&s, "TempEnchant1"));
    s.resolve();
    assert!((s.eval::<f64>("return BuffFrame:GetRight()").unwrap() - resting).abs() < 1e-3);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The idle enchant branch hides both slots and re-parks the bar every frame, unconditionally
/// (`BuffFrame.lua:166-173`), so the sentinels below are overwritten.
#[test]
fn an_idle_enchant_row_rewrites_the_bar_as_the_reference_does() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(&mut s, mixed_bar());
    frame(&mut s, 0.1);
    frame(&mut s, 0.1);
    s.resolve();
    let resting = s.eval::<f64>("return BuffFrame:GetRight()").unwrap();

    // Sentinels the idle branch must overwrite.
    s.run(
        r#"BuffFrame:SetPoint("TOPRIGHT", "TemporaryEnchantFrame", "TOPRIGHT", -41, 0)
           TempEnchant1:Show()"#,
    )
    .unwrap();
    for _ in 0..10 {
        frame(&mut s, 0.1);
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert!(
        !shown(&s, "TempEnchant1"),
        "the reference's idle branch re-hides the slot on every tick"
    );
    let displaced = s.eval::<f64>("return BuffFrame:GetRight()").unwrap();
    assert!(
        (displaced - resting).abs() < 1e-3,
        "…and re-parks the bar with it: got {displaced}, resting {resting}"
    );

    // Both hands: off hand in slot 1, main hand in slot 2 to its left (`BuffFrame.lua:179-223`).
    s.set_weapon_enchants(
        Some(benilla_ui::script::WeaponEnchant {
            remaining_ms: Some(480_000),
            charges: 0,
        }),
        Some(benilla_ui::script::WeaponEnchant {
            remaining_ms: Some(120_000),
            charges: 0,
        }),
    );
    frame(&mut s, 0.1);
    assert!(shown(&s, "TempEnchant1") && shown(&s, "TempEnchant2"));
    assert_eq!(
        s.eval::<i64>("return TempEnchant1:GetID()").unwrap(),
        17,
        "off hand first: slot 1 is the OffHandSlot id"
    );
    assert_eq!(s.eval::<i64>("return TempEnchant2:GetID()").unwrap(), 16);
    assert_eq!(text(&s, "TempEnchant1Duration"), "2 m");
    assert_eq!(text(&s, "TempEnchant2Duration"), "8 m");
    s.resolve();
    assert_eq!(
        s.eval::<f64>("return TemporaryEnchantFrame:GetWidth()")
            .unwrap(),
        64.0,
        "two 32px columns"
    );
    let both = s.eval::<f64>("return BuffFrame:GetRight()").unwrap();
    assert!(
        (resting - both - 69.0).abs() < 1e-3,
        "the bar clears both columns + the gutter: {resting} -> {both}"
    );

    s.set_weapon_enchants(None, None);
    frame(&mut s, 0.1);
    assert!(!shown(&s, "TempEnchant1") && !shown(&s, "TempEnchant2"));
    s.resolve();
    let parked = s.eval::<f64>("return BuffFrame:GetRight()").unwrap();
    assert!(
        (parked - resting).abs() < 1e-3,
        "the drop-to-empty pass re-parks the bar: {parked} vs {resting}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `BuffButton_OnUpdate` writes the alpha and the duration every tick, unconditionally
/// (`BuffFrame.lua:130-138`), so the sentinels (alpha 0.42, text "X") are overwritten.
#[test]
fn a_settled_buff_button_rewrites_alpha_and_duration_as_the_reference_does() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(
        &mut s,
        vec![aura(
            1126,
            "Mark of the Wild",
            "Interface\\Icons\\Spell_Nature_Regeneration",
            true,
            1,
            None,
            300.0,
            true,
        )],
    );
    frame(&mut s, 0.1); // the event repaint and the poll's first write ("5 m", alpha 1.0)
    frame(&mut s, 0.1);
    assert_eq!(text(&s, "BuffButton0Duration"), "5 m");

    s.run(r#"BuffButton0:SetAlpha(0.42); BuffButton0Duration:SetText("X")"#)
        .unwrap();
    for _ in 0..10 {
        frame(&mut s, 0.016);
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert!(
        (alpha(&s, "BuffButton0") - 1.0).abs() < 1e-6,
        "the reference's OnUpdate writes a fresh alpha every tick — got {}",
        alpha(&s, "BuffButton0")
    );
    assert_eq!(
        text(&s, "BuffButton0Duration"),
        "5 m",
        "…and re-writes the duration with it, unchanged or not"
    );

    s.tick(60.0);
    assert_eq!(
        text(&s, "BuffButton0Duration"),
        "4 m",
        "the minute rollover overwrites the sentinel: the gate passes real changes"
    );

    // In the last 31 s the pulse ramps each tick; under a minute the number is
    // `HIGHLIGHT_FONT_COLOR` white (`BuffFrame.lua:251-252`).
    s.tick(210.0); // ~29.5 s left
    s.resolve();
    let a1 = alpha(&s, "BuffButton0");
    s.tick(0.2);
    let a2 = alpha(&s, "BuffButton0");
    s.tick(0.3);
    let a3 = alpha(&s, "BuffButton0");
    assert!(
        a1 != a2 && a2 != a3,
        "the warning pulse flows through the gate per tick: {a1} {a2} {a3}"
    );
    s.resolve();
    let seconds_text = text(&s, "BuffButton0Duration");
    assert!(
        seconds_text.ends_with(" s"),
        "inside the last minute the timer counts seconds: {seconds_text}"
    );
    let white = s.extract().iter().any(|q| match &q.content {
        QuadContent::Text {
            text: Some(t),
            color: Some(c),
            ..
        } => {
            *t == seconds_text
                && (c[0] - 1.0).abs() < 1e-3
                && (c[1] - 1.0).abs() < 1e-3
                && (c[2] - 1.0).abs() < 1e-3
        }
        _ => false,
    });
    assert!(
        white,
        "the sub-minute band rewrote the vertex color to HIGHLIGHT white"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The aura tooltip's duration line over the install's own `GlobalStrings.lua`, formatted by
/// `tooltip::duration_text` on the reference's ladder (`0x52fa50`).
#[test]
fn the_duration_line_reads_the_real_global_strings() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
    s.set_spell_tooltip(
        1459,
        benilla_ui::script::SpellTooltipView {
            name: "Arcane Intellect".into(),
            aura_description: "Intellect increased by 2.".into(),
            ..Default::default()
        },
    );
    s.tick(10.0); // GetTime = 10
    s.run(
        r#"
        BENILLA_ANCHOR = CreateFrame("Button", "BF9")
        BENILLA_ANCHOR:SetPoint("CENTER", 0, 0); BENILLA_ANCHOR:SetWidth(10); BENILLA_ANCHOR:SetHeight(10)
        BENILLA_TIP = CreateFrame("GameTooltip", "TT9")
    "#,
    )
    .unwrap();

    // `secs_left` seconds remaining at GetTime = 10, read back as the tooltip's last line.
    let mut line = |secs_left: f64| -> String {
        s.set_auras(
            "player",
            Some(vec![AuraState {
                spell_id: 1459,
                name: Some("Arcane Intellect".into()),
                duration: 86_400.0,
                expiration_time: 10.0 + secs_left,
                helpful: true,
                ..Default::default()
            }]),
        );
        s.run(
            r#"
            BENILLA_TIP:SetOwner(BENILLA_ANCHOR, "ANCHOR_RIGHT")
            BENILLA_TIP:SetPlayerBuff(0)
            BENILLA_LAST = TT9TextLeft3:GetText()
        "#,
        )
        .unwrap();
        s.eval::<String>("return BENILLA_LAST").unwrap()
    };

    assert_eq!(line(7_200.0), "2 hours remaining", "was '120 minutes'");
    assert_eq!(line(3_600.0), "1 hour remaining", "the hour edge, singular");
    assert_eq!(
        line(3_599.999),
        "60 minutes remaining",
        "one ms under the hour stays in minutes — no '1 hour' until it is whole"
    );
    assert_eq!(line(60.0), "1 minute remaining", "was '1 minutes'");
    assert_eq!(line(61.0), "2 minutes remaining", "the minute arm ceils");
    // The seconds arm truncates where every arm above it ceils.
    assert_eq!(line(5.4), "5 seconds remaining", "was '6 seconds'");
    assert_eq!(line(1.0), "1 second remaining", "singular at exactly one");
    assert_eq!(
        line(-0.4),
        "0 seconds remaining",
        "the lapsing read, plural at zero"
    );
    assert_eq!(line(129_600.0), "2 days remaining", "was '2160 minutes'");

    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}
