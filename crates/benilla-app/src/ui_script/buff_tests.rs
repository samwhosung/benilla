//! The player buff bar (stock `Interface\FrameXML\BuffFrame.xml`) against its
//! reference behaviour. The XML/Lua is the unit under test; the app-side feed (`crate::ui_aura`) is
//! stubbed by pushing an [`AuraState`] list straight through [`UiScript::set_auras`] and firing
//! `PLAYER_AURAS_CHANGED`, so these exercise the *button* handlers — the row/filter wiring, the
//! dispel-tinted border, the stack count, the countdown, the warning flash, and the right-click
//! cancel — the way the reference's own `BuffButton_*` do.
//!
//! **The window runs on the 1.12 verbs and the reference's index space** (b2ede294 landed the
//! `GetPlayerBuff` family): a button's `id` is a 0-based ordinal within its own filter,
//! `GetPlayerBuff` turns it into a CACHE POSITION, and every read below — texture, dispel class,
//! stacks, time left, cancel, tooltip — consumes that position. The tests drive it the way the
//! corpus does, including the `while GetPlayerBuff(i) >= 0 do` walk and a cancel by position.
//!
//! State is read back through the widget (`IsShown`/`GetText`/`GetAlpha`) and the paint through the
//! [`draw list`](UiScript::extract) (icon path + border tint), the same two-lens split the cast-bar
//! tests use — Lua-visible state is blind to what actually renders.

use benilla_ui::script::{AuraState, ExtractedQuad, QuadContent, UiScript};

use super::test_ui::load_ui as load_xml;

fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The one-letter duration strings (DAY/HOUR/MINUTE/SECOND_ONELETTER_ABBR) the bar prints
    // under a timed aura. Our copy carried them as `X = X or "%d s"` fallbacks; the reference's
    // file formats them straight and `format(nil, …)` raises.
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml"); // NORMAL/HIGHLIGHT_FONT_COLOR + the FontStrings' faces
                                                    // `GameTooltip`, which the reference's BuffButton_Update indexes on EVERY repaint to ask
                                                    // `IsOwned(this)` (BuffFrame.lua l.104) — not just on hover. Ours guarded it; the reference
                                                    // does not, so a session without the tooltip loses the whole repaint.
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    // `SecondsToTimeAbbrev`, which 1.12 keeps in UIParent.lua and so do we since window 18.
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
    load_xml(&s, "Interface\\FrameXML\\BonusActionBarFrame.xml"); // BENILLA_FALLBACK_ICON (the unknown-icon fallback)
                                                                  // The timer switch, PLANTED ON — not the shipped value. 1.12 declares it in
                                                                  // UIOptionsFrame.lua (default "0"); we have no counterpart to that file, so it lives with the
                                                                  // row that drives it (OptionsFrame.xml), where it shipped "1" from 0255/1139 until 1804 put
                                                                  // it back on the reference's "0". These tests are about the timer text and the geometry it
                                                                  // buys, so the harness turns it on the way the Interface page's row does. Order matters: the
                                                                  // reference's `BuffFrame_OnLoad` calls `BuffButtons_UpdatePositions`, which seats the debuff
                                                                  // row 20px differently depending on this value, so setting it afterwards leaves the bar laid
                                                                  // out for the wrong one.
    s.run("SHOW_BUFF_DURATIONS = \"1\"").unwrap();
    load_xml(&s, "Interface\\FrameXML\\BuffFrame.xml");
    // …and APPLIED, the way the app applies it (`manifest::apply_buff_durations`).
    // `BuffFrame_OnLoad` does not call `BuffButtons_UpdatePositions`: in 1.12 that is
    // `UIOptionsFrame.lua`'s job and we have no counterpart to that file. A session that skips it
    // is laid out for durations-OFF while the setting says on.
    super::manifest::apply_buff_durations(&s).unwrap();
    s
}

/// Build an [`AuraState`] as the feed would push it. `expiration_time` 0 = permanent (no wire
/// duration — the reference's "until cancelled"); the `GetTime()` clock starts at 0 in the harness.
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
        // What the feed derives for a permanent aura: no finite duration to display. Here the two
        // agree by construction, but they are different questions in the real feed — the flag is
        // DBC-derived and correct on the frame the aura appears, before any duration packet lands
        // (`benilla::ui_aura::until_cancelled`).
        until_cancelled: expiration_time == 0.0,
        channeled: false,
    }
}

/// Push the list, then fire the discrete-change event the app's feed fires — the buttons repaint.
///
/// `PLAYER_AURAS_CHANGED`, no args: the reference's own event, which `ui_aura` fires beside the
/// Era-shaped `UNIT_AURA` on the same rebuild. It is what the buttons register for now, and it is
/// what every corpus aura addon registers for.
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

/// The first quad drawn from `Interface\...\<leaf>` — texture regions keyed by their art.
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

    // The buff takes the first helpful button, the debuff the first harmful one; the rest hide.
    assert!(shown(&s, "BuffButton0"), "first buff -> BuffButton0");
    assert!(!shown(&s, "BuffButton1"), "no second buff");
    assert!(shown(&s, "BuffButton16"), "first debuff -> BuffButton16");
    assert!(!shown(&s, "BuffButton17"), "no second debuff");

    // Both icons drew.
    assert!(
        tex_quad(&quads, "Spell_Nature_Regeneration").is_some(),
        "buff icon drew"
    );
    assert!(
        tex_quad(&quads, "Spell_Shadow_ShadowWordPain").is_some(),
        "debuff icon drew"
    );

    // The debuff border wears the Magic tint (DebuffTypeColor["Magic"] = 0.20, 0.60, 1.00).
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
            // The reference's overlay-quadrant crop rode through unchanged.
            let uv = uv.edges();
            assert!(
                (uv[0] - 0.296875).abs() < 1e-6 && (uv[3] - 0.515625).abs() < 1e-6,
                "debuff overlay tex-coords, got {uv:?}"
            );
        }
        other => panic!("border is not a tinted, cropped texture: {other:?}"),
    }

    // Stack count shows only above 1: the 3-stack debuff shows "3", the single buff shows nothing.
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
            // Permanent (a stance): no wire duration -> expiration 0 -> no timer.
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
            // Timed: 120s out. After a 0.1s tick GetTime()=0.1, so 119.9s -> SecondsToTimeAbbrev -> "2 m".
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
            // 5 min out — never inside the 31s warning window, so it stays at full alpha.
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
            // 20s out — inside the window the whole run, so its button pulses.
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
    // The RightButtonUp handler -> CancelPlayerBuff(this.buffIndex) -> the app drains one
    // CMSG_CANCEL_AURA by spell id.
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

    // The aura drops: the feed pushes an empty list and re-fires. The button hides, and so does its
    // timer — a HIDDEN region since 1139, where it used to be blanked, so the text it last drew is
    // stale and beside the point; what the bar promises is that nothing paints.
    push(&mut s, vec![]);
    frame(&mut s, 0.1);
    assert!(!shown(&s, "BuffButton0"), "hidden once the aura is gone");
    assert!(!shown(&s, "BuffButton0Duration"));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Decision 0846's second defect, pinned.** The bar's countdown is a per-frame POLL, not a value
/// cached when the aura event fires — exactly as the reference does it (`BuffButton_OnUpdate` →
/// `GetPlayerBuffTimeLeft`, extracted 1.12 `BuffFrame.lua` l.128-130, which caches only the buff
/// *index* on `PLAYER_AURAS_CHANGED`).
///
/// This is what makes a *refresh* work. A recast, a reapplied DoT, cast pushback and the server's
/// replay of every duration at world entry all change an aura's expiry while changing nothing
/// discrete about it — same spell, same stacks, same dispel class — so the feed's change key does
/// not trip and **no aura event fires**. The push below therefore deliberately does NOT fire one.
/// Against the old event-cached bar the button kept counting the stale expiry down, reached zero
/// and sat at `"0 s"` for as long as the buff lived (live-measured: `app left 59.9s` beside
/// `lua left -9.9s`).
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
    // 20s out. After the tick GetTime()=0.1 -> 19.9s -> "19 s" (SecondsToTimeAbbrev floors via %d).
    push(&mut s, vec![mark(20.0)]);
    frame(&mut s, 0.1);
    assert_eq!(text(&s, "BuffButton0Duration"), "19 s");

    // The refresh: a new expiry, everything discrete unchanged — so the feed pushes the list and
    // fires NOTHING. The button must still pick the new expiry up on its next frame.
    s.set_auras("player", Some(vec![mark(300.0)]));
    frame(&mut s, 0.1);
    assert_eq!(
        text(&s, "BuffButton0Duration"),
        "5 m",
        "the countdown re-reads the aura every frame; it does not wait for an event"
    );
    // ...and the flash follows it back out of the warning window (20s pulsed, 300s is solid).
    assert!(
        (alpha(&s, "BuffButton0") - 1.0).abs() < 1e-6,
        "no longer inside the 31s warning window, so full alpha"
    );

    // The same poll is what shows a timer that arrived AFTER the icon did — the fresh-apply case,
    // where the duration packet's stamp lands a frame later than the descriptor delta.
    //
    // **What the icon-first frame looks like MOVED when the bar came onto the 1.12 verbs.** The
    // permanence test used to be `expirationTime > 0`, which cannot tell "no duration, ever" from
    // "the duration has not landed yet" and so drew nothing for both. It is now `untilCancelled` —
    // GetPlayerBuff's second return, derived from the SPELL (`ui_aura::until_cancelled`) — so a
    // timed aura awaiting its stamp is known to be timed, and the reference counts it from `0 s`
    // and pulses it for the frame or two until the packet joins. `mark` is a timed spell, so that
    // is what it does here; the permanent case (a stance) is
    // [`a_timed_aura_counts_down_and_a_permanent_one_shows_no_timer`], and it still shows nothing.
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

/// **`SHOW_BUFF_DURATIONS`** — 0255 shipped the durations-shown geometry with no
/// switch because there was no panel to hang one on, and said the other branch was already there
/// waiting. This is it: the global — the reference's "0" since 1804, planted "1" by this file's
/// harness — hides the timer text and closes the 15px gutter each row leaves for it, down to the
/// 5px the columns use. What it does NOT touch is the warning flash — with the numbers gone, the
/// pulse is the only thing left saying an aura is about to drop, and the reference pulses from
/// `BuffButton_OnUpdate` regardless of the setting.
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
            20.0, // inside the 31s warning window, so the flash is live either way
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

    // The switch, applied the way the options row applies it.
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

    // And back: the text is re-written from the poll, not from anything it kept.
    s.run("SHOW_BUFF_DURATIONS = \"1\"; BuffButtons_UpdatePositions()")
        .unwrap();
    frame(&mut s, 0.1);
    assert!(shown(&s, "BuffButton0Duration"));
    assert_eq!(text(&s, "BuffButton0Duration"), "19 s");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A mixed bar: three buffs and two debuffs in one insertion-ordered cache.
fn mixed_bar() -> Vec<AuraState> {
    vec![
        // pos 0 — buff, cancelable, timed
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
        // pos 1 — debuff, Magic, 3 stacks
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
        // pos 2 — buff, cancelable
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
        // pos 3 — debuff, Poison
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
        // pos 4 — buff, NOT cancelable (a stance): right-click must be refused
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

/// **The two index spaces, end to end.** The button `id` is a 0-based ordinal within its own
/// filter; `GetPlayerBuff` turns it into a CACHE POSITION that is absolute across filters. The bar
/// is only correct if the two agree: button N of a row must draw the aura the corpus's own walk
/// finds at that ordinal, and the position it cached must be the one an unfiltered walk reports.
///
/// The walk is spelled the way the corpus spells it — `while GetPlayerBuff(i) >= 0 do`
/// (`_LazyPig/LazyPig.lua:1174`, `Zorlen/Zorlen.lua:2797`) — because `-1`, not nil, is the
/// terminator, and because the default filter is `HELPFUL|HARMFUL`: the walk must see the debuffs
/// too or the whole thing silently halves.
#[test]
fn the_buttons_and_the_corpus_walk_agree_on_the_cache_positions() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(&mut s, mixed_bar());
    frame(&mut s, 0.1);

    // The corpus walk: five auras, buffs AND debuffs, positions identical to the ordinals under
    // the default filter — and it terminates.
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

    // Each row filled from its own filter, in cache order — and each button's CACHED position is
    // the absolute one, not its ordinal.
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
    // The rows stop where the filter runs out; nothing bleeds across.
    assert!(!shown(&s, "BuffButton3"), "only three buffs");
    assert!(!shown(&s, "BuffButton18"), "only two debuffs");
    assert_eq!(
        s.eval::<i64>("return BuffButton3.buffIndex").unwrap(),
        -1,
        "a miss is -1, the sentinel the corpus terminates on — never nil"
    );

    // The reads that hang off the position land on the right aura: the 3-stack Magic debuff and
    // the 5-stack Poison one are in the right buttons, tinted from their own dispel classes.
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
        // DebuffTypeColor["Poison"] = 0.00, 0.60, 0.00
        .any(|c| c[0].abs() < 1e-3 && (c[1] - 0.60).abs() < 1e-3 && c[2].abs() < 1e-3);
    assert!(poison, "the second debuff button wears the Poison tint");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Right-click cancels by cache position.** `BuffButton_OnClick` is
/// `CancelPlayerBuff(this.buffIndex)` — the reference's own line — so the button that gets
/// cancelled is the one under the cursor even when its ordinal and its position differ, which is
/// the case for every buff after the first debuff.
///
/// The gate is the app's, unchanged: a cancelable buff queues its SPELL id, a stance and a debuff
/// are silent no-ops. That last part is what a per-filter id could not guarantee on its own — here
/// it is structural, because a harmful button's handle simply names a harmful record.
#[test]
fn right_clicking_cancels_the_aura_under_the_cursor_by_its_cache_position() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(&mut s, mixed_bar());
    frame(&mut s, 0.1);
    assert!(s.take_cancel_aura_requests().is_empty());

    // The SECOND buff button: helpful ordinal 1, cache position 2, spell 1459. A bar that cancelled
    // by ordinal would send 589 — the debuff sitting at position 1.
    s.eval::<()>("this = BuffButton1; BuffButton_OnClick()")
        .unwrap();
    assert_eq!(
        s.take_cancel_aura_requests(),
        vec![1459],
        "the aura under the cursor, not the record at the ordinal"
    );

    // A stance (helpful, not cancelable), both debuff buttons, and an empty slot: all silent.
    for button in ["BuffButton2", "BuffButton16", "BuffButton17", "BuffButton7"] {
        s.eval::<()>(&format!("this = {button}; BuffButton_OnClick()"))
            .unwrap();
    }
    assert!(
        s.take_cancel_aura_requests().is_empty(),
        "a stance, a debuff and an empty button are all no-ops"
    );

    // The first buff still cancels normally.
    s.eval::<()>("this = BuffButton0; BuffButton_OnClick()")
        .unwrap();
    assert_eq!(s.take_cancel_aura_requests(), vec![1126]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The temporary-enchant row** (ref `BuffFrame_Enchant_OnUpdate`, l.162-233). With nothing
/// enchanted it hides both slots and parks the bar on its resting point; with a weapon enchanted it
/// shows the icon, counts the expiry down from MILLISECONDS, and slides the top buff row left to
/// clear it — while rows 2 and 3, which hang off TempEnchant1 and TemporaryEnchantFrame, stay put.
#[test]
fn the_temporary_enchant_row_shows_a_weapon_enchant_and_moves_the_top_row_aside() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    // A LOADED bar, deliberately: the enchant slots must be driven by GetWeaponEnchantInfo and by
    // nothing else. The reference builds them from `BuffButtonTemplate` and blanks the four aura
    // handlers with empty `<OnLoad/>`/`<OnEvent/>`/`<OnClick/>` elements; our loader treats an
    // empty handler body as "none given" and so KEEPS the inherited one (`compile_handler` returns
    // None and `apply_scripts` never calls SetScript). Under that idiom TempEnchant1 would take
    // `BuffButton_OnLoad`, register PLAYER_AURAS_CHANGED, and paint the first buff in the corner —
    // and a right-click on it would cancel that aura. Hence the standalone template, and hence a
    // populated bar here: an empty aura list cannot tell the two apart.
    push(&mut s, mixed_bar());
    frame(&mut s, 0.1);

    // Nothing enchanted: both slots hidden, and the bar sits where it has always sat.
    assert!(
        !shown(&s, "TempEnchant1"),
        "the enchant slots are not buff buttons: no aura may reach them"
    );
    assert!(!shown(&s, "TempEnchant2"));
    // The direct probe, and its exact shape is the point. `BuffButtonTempEnchant` inherits
    // `BuffButtonTemplate` and blanks its three aura handlers with `<OnLoad>`↵`</OnLoad>` — a
    // WHITESPACE body, which 1.12 compiles into a valid empty function rather than storing nil
    // (`SetScript 0x7025c0` tests `text[0]`, and nothing trims). So each handler is still a
    // FUNCTION here, and it is the
    // reference's own no-op rather than `BuffButton_OnLoad`.
    //
    // Asserting `== nil` instead would be asserting a bug: an addon reading
    // `TempEnchant1:GetScript("OnLoad")` gets a function on a real client.
    for handler in ["OnLoad", "OnEvent", "OnClick"] {
        assert!(
            s.eval::<bool>(&format!(
                "return type(TempEnchant1:GetScript(\"{handler}\")) == \"function\""
            ))
            .unwrap(),
            "an enchant slot's {handler} is the reference's compiled no-op, not nil"
        );
    }
    // …and it is NOT the buff-button body: an aura event reaching it would paint the first buff
    // into the corner and arm a right-click cancel, which is what a naive "empty means absent"
    // loader produces.
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

    // A main-hand enchant, 8 minutes out. The app feeds it the way `ui_char` does.
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

    // The top buff row slid left by one icon plus the 5px gutter; the debuff row did not move.
    //
    // That is true only because the harness has APPLIED the durations setting, and the
    // qualification is worth keeping: `BuffButton16`'s XML anchor is `TOPRIGHT` to `BuffButton8`'s
    // `BOTTOM` — inside BuffFrame, so it would travel with it — and the only thing that re-anchors
    // it to `TemporaryEnchantFrame` is `BuffButtons_UpdatePositions`, which `BuffFrame_OnLoad`
    // does NOT call. A bar that nobody has applied the setting to therefore moves BOTH rows. The
    // window's own file cannot reach that state in production (our Interface page applies it), and
    // finding it was what showed the harness had to apply it too.
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

    // And back: the enchant drops, the row hides and the bar returns to its resting point.
    s.set_weapon_enchants(None, None);
    frame(&mut s, 0.1);
    assert!(!shown(&s, "TempEnchant1"));
    s.resolve();
    assert!((s.eval::<f64>("return BuffFrame:GetRight()").unwrap() - resting).abs() < 1e-3);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The idle enchant row rewrites the bar every frame, because the reference's does** — and this
/// pins that rather than the write-gating it replaced.
///
/// Our own `BuffFrame.xml` latched the no-enchant branch on a `benillaCleared` flag after its first
/// pass; decision 1396 measured that branch at ~14 µs/frame and took the gate as its post-fix lead.
/// 1751's eighteenth window made the bar `Interface\FrameXML\BuffFrame.xml`, whose
/// `BuffFrame_Enchant_OnUpdate` opens with an unconditional `TempEnchant1:Hide()` … `BuffFrame:
/// SetPoint(…)` and returns. So the writes are back, they are the reference's, and 1751 §2 takes
/// them: an optimisation is not a divergence we get to keep silently once the file is theirs.
///
/// **1396's finding is not withdrawn** — the cost is real and the same measurement would find it
/// again. If it is ever worth paying down, the fix is an adapter over this one handler with a
/// record beside it, not a re-transcription of the window.
///
/// The sentinels below are inverted from what they were: the branch overwrites both, every frame.
/// The rest of the test is unchanged and is the part that always mattered — the row DRIVEN with
/// both hands keeps the reference's off-hand-first packing order, and the drop back to empty
/// re-parks the bar.
#[test]
fn an_idle_enchant_row_rewrites_the_bar_as_the_reference_does() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(&mut s, mixed_bar());
    frame(&mut s, 0.1); // first pass: the clear + park write once and latch
    frame(&mut s, 0.1);
    s.resolve();
    let resting = s.eval::<f64>("return BuffFrame:GetRight()").unwrap();

    // Sentinels the no-enchant branch can never leave standing if it still writes.
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

    // The control: both hands enchant. Off hand packs slot 1 (the reference's order — main hand
    // sits to its LEFT in slot 2), the row is 64 wide, the bar starts one 5px gutter left of it.
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

    // And back to empty: the transition still writes (the latch re-arms, not re-fires).
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

/// **A settled buff button rewrites its alpha and its duration every frame, because the
/// reference's does** — the sibling of the enchant-row test above, and the same trade.
///
/// Our copy gated both writes on what was last written (the 1396 class, one row down); the
/// reference's `BuffButton_OnUpdate` polls `GetPlayerBuffTimeLeft` and then writes
/// `SetAlpha` and `BuffFrame_UpdateDuration` unconditionally, every tick. So the sentinels below —
/// alpha 0.42 and the text "X", neither of which the handler can produce — are overwritten within
/// one frame, and that is what is pinned.
///
/// **The poll itself was never the divergence** (`GetPlayerBuffTimeLeft` every frame is
/// load-bearing) and the controls below are unchanged, because they are the part
/// that describes the WINDOW rather than our implementation of it: the minute rollover drops "5 m"
/// to "4 m", the warning band turns the number white, and inside the last 31s the pulse ramps the
/// alpha. Those three are the reference's behaviour and they still hold.
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
    frame(&mut s, 0.1); // the event repaint + the poll's first write ("5 m", alpha 1.0)
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

    // Control 1: a REAL text change flows — the abbreviation drops to "4 m" past the boundary.
    s.tick(60.0);
    assert_eq!(
        text(&s, "BuffButton0Duration"),
        "4 m",
        "the minute rollover overwrites the sentinel: the gate passes real changes"
    );

    // Control 2: inside the last 31s the pulse writes a fresh ramp value every tick, and the
    // number wears the sub-minute HIGHLIGHT white (the band is part of the write tuple).
    s.tick(210.0); // ~29.5s left
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

/// The duration line against the **real shipped strings**, end to end — the leg the engine's own
/// synthetic-template tests cannot reach.
///
/// The engine formats through `tooltip::duration_text` (`0x52fa50`'s ladder) over whatever
/// `GlobalStrings.lua` the player's install carries; this runs
/// that file into a real VM exactly as the boot's `load_global_strings` does, then asserts the
/// wording that comes back. Every expectation below is a reading the PREVIOUS two-arm formatter got
/// wrong, so this is the whole delta in one place. Skips without client data.
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

    // The headline divergence: anything past an hour was reading in MINUTES.
    assert_eq!(line(7_200.0), "2 hours remaining", "was '120 minutes'");
    assert_eq!(line(3_600.0), "1 hour remaining", "the hour edge, singular");
    assert_eq!(
        line(3_599.999),
        "60 minutes remaining",
        "one ms under the hour stays in minutes — no '1 hour' until it is whole"
    );
    // The singular half: the shipped pair really is "%d minute" / "%d minutes".
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
    // And the top of the ladder, which had no arm at all.
    assert_eq!(line(129_600.0), "2 days remaining", "was '2160 minutes'");

    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}
