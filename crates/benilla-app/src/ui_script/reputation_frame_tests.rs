//! The stock **Reputation tab** (`Interface\FrameXML\ReputationFrame.xml`) and the **reputation
//! watch bar** (`Interface\FrameXML\ActionBarFrame.xml`) driven end-to-end, engine-only (no Bevy) —
//! the per-window test module the skills/spellbook/bank files already establish.
//!
//! What it pins is the PAINT law, which is the half `benilla-ui`'s own
//! `script::reputation::tests` structurally cannot reach: that module drives the twelve globals and
//! asserts the tuples, and stops at the seam. Everything below is about what the tuples become on
//! screen — which of the 15 fixed row slots a visible index lands in, whether a header slot draws
//! the plus or the minus art, the `FACTION_BAR_COLORS[standingID]` fill, the scroll offset
//! re-binding the slots, the popup's three boxes, and the watch bar's own text and posture.
//!
//! The fixture deliberately mirrors the engine module's own (Alliance over Ironforge + Stormwind,
//! Steamwheedle over Booty Bay, the parentless bucket last) so a failure here reads against a shape
//! that is already pinned one layer down, and the two can be compared line for line.
//!
//! **All of it is the reference's own now** — the window since 1751 (`CharacterFrame.xml`,
//! `PaperDollFrame.xml`), the page itself (`ReputationFrame.xml`) with it, and the watch bar with
//! `ActionBar.xml`'s retirement into `ActionBarFrame.xml`. So every test here opens with
//! `wow_data_or_skip!()` and loads [`super::test_ui::CHARACTER_UI`], and what it pins is our
//! ENGINE under stock XML rather than XML of ours.

use benilla_ui::script::{FactionEntry, QuadContent, ReputationState, UiScript, UnitState};

/// An ordinary bar row: visible, not a header, `standing_id` 5 ("Friendly") sitting 1000 into a
/// 6000-wide rank window. The same numbers `benilla-ui`'s own fixture uses.
fn entry(faction_id: u32, rep_list_id: u32, parent_id: u32, name: &str) -> FactionEntry {
    FactionEntry {
        faction_id,
        rep_list_id,
        parent_id,
        name: name.into(),
        description: format!("About the {name}."),
        standing: 4000,
        standing_id: 5,
        bar_min: 3000,
        bar_max: 9000,
        visible: true,
        is_header: false,
        at_war: false,
        can_toggle_at_war: true,
        inactive: false,
    }
}

/// A header row as the wire really delivers one: flag `0x08` set and NOT visible (the Steamwheedle
/// Cartel's actual byte). Its own visibility never gates its group.
fn header(faction_id: u32, rep_list_id: u32, name: &str) -> FactionEntry {
    FactionEntry {
        visible: false,
        is_header: true,
        ..entry(faction_id, rep_list_id, 0, name)
    }
}

/// The engine module's own fixture, pushed out of order so the engine's sort is what shows. The
/// visible rows it produces, in order:
///
/// | slot | row |
/// |------|-----|
/// | 1 | header `Alliance` |
/// | 2 | `Ironforge` |
/// | 3 | `Stormwind` |
/// | 4 | header `Steamwheedle Cartel` |
/// | 5 | `Booty Bay` |
/// | 6 | header `Other` |
/// | 7 | `Argent Dawn` |
/// | 8 | `Bloodsail Buccaneers` |
fn state() -> ReputationState {
    ReputationState {
        entries: vec![
            entry(72, 19, 469, "Stormwind"),
            entry(529, 13, 0, "Argent Dawn"),
            header(469, 11, "Alliance"),
            entry(21, 1, 169, "Booty Bay"),
            entry(47, 20, 469, "Ironforge"),
            header(169, 10, "Steamwheedle Cartel"),
            entry(87, 0, 0, "Bloodsail Buccaneers"),
        ],
        watched: None,
    }
}

/// The character window, whose third tab this page is — [`super::test_ui::CHARACTER_UI`], which
/// already carries `ReputationFrame.xml` (stock `CharacterFrame_ShowSubFrame` hides all five pages
/// by name, unguarded) and `ActionBar.xml` (the watch bar, and the show/hide pair's
/// `ShowWatchedReputationBarText`). This page adds nothing of its own.
///
/// It was a hand-copied manifest prefix until the window became the reference's:
/// stock `CharacterFrame_OnLoad` reaches into four other files before it does anything else, so the
/// list stopped being short enough for a per-module copy to stay honest.
fn load_page(s: &UiScript) {
    for file in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(s, file);
    }
}

/// A level-`level` player behind the window — enough of one that opening it is quiet.
///
/// **Race and class carry both halves of their pairs**, which is not decoration since 1751:
/// `UnitRace`/`UnitClass` answer `(localized, file)` or `nil, nil` — the binding `zip`s the two —
/// and stock `PaperDollFrame_SetLevel` formats level, race and class into `CharacterLevelText`
/// unguarded (`PaperDollFrame.lua:100-104`), on every show of the window this page is a tab of.
/// A `UnitState::default()` player therefore raises the moment `ToggleCharacter` runs.
fn player(level: u32) -> UnitState {
    UnitState {
        exists: true,
        level,
        race: Some("Human".into()),
        race_file: Some("Human".into()),
        class: Some("Warrior".into()),
        class_file: Some("WARRIOR".into()),
        ..UnitState::default()
    }
}

/// The Reputation page, open on its tab, with [`state`] pushed and a level-40 player behind it.
fn shown_reputation_page() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_page(&s);
    s.set_unit("player", Some(player(40)));
    s.set_reputation(state());
    s.run(r#"ToggleCharacter("ReputationFrame")"#).unwrap();
    s.resolve();
    s
}

/// Click the centre of a named frame through the real pointer pipeline, the way the app does — so
/// a frame silently eating the row's clicks fails here rather than on the director's screen.
fn click_center(s: &mut UiScript, name: &str) {
    s.resolve();
    let (x, y) = s
        .eval::<(f32, f32)>(&format!(
            "return ({name}:GetLeft() + {name}:GetRight()) / 2, \
                    ({name}:GetTop() + {name}:GetBottom()) / 2"
        ))
        .unwrap();
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
}

fn text_of(s: &mut UiScript, expr: &str) -> String {
    s.eval::<String>(&format!("return {expr}:GetText() or \"\""))
        .unwrap()
}

/// **`IsVisible`, not `IsShown`, is the right question for the scroll bar since 1860.** The
/// reference's `FauxScrollFrame_Update` hides the scroll FRAME when the list fits
/// (`scrollBar:SetValue(0); frame:Hide()`); our deleted kit hid the BAR itself through
/// `BenillaScrollBar_SetShown`. So the bar's own `IsShown` stays true under the reference and only
/// its hidden ancestor takes it off screen — which `IsVisible` reports and `IsShown` cannot.
fn visible(s: &mut UiScript, name: &str) -> bool {
    s.eval::<bool>(&format!("return {name}:IsVisible() and true or false"))
        .unwrap_or(false)
}

fn shown(s: &mut UiScript, name: &str) -> bool {
    s.eval::<bool>(&format!("return {name}:IsShown() and true or false"))
        .unwrap()
}

/// **The page is the character window's tab 3, and it comes up on it.** The whole renumber rides on
/// this: `ToggleCharacter` selects the row from the page's own `id=`
/// (ref `CharacterFrame.lua:11`), so a page seated at the wrong id shows the right pane under the
/// wrong tab. Skills moved to 4 in the same breath and is asserted here for the same reason.
#[test]
fn the_reputation_page_opens_on_the_windows_third_tab() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_reputation_page();
    assert!(
        s.eval::<bool>("return ReputationFrame:IsVisible()")
            .unwrap(),
        "the Reputation page is up"
    );
    assert_eq!(
        s.eval::<i64>("return PanelTemplates_GetSelectedTab(CharacterFrame)")
            .unwrap(),
        3,
        "and the tab row selects 3 — the reference's own slot for Reputation"
    );
    // **The id IS the tab slot**, and that is the whole coupling: `ToggleCharacter` selects the tab
    // with `PanelTemplates_SetTab(CharacterFrame, subFrame:GetID())`
    // (`CharacterFrame.lua:10`), while `CharacterFrameTab_OnClick` hardcodes which page each tab
    // opens (`:41-43` — tab 3 → Reputation, tab 4 → Skills). A page whose `id=` disagreed would
    // open on click and light the wrong tab.
    assert_eq!(
        s.eval::<i64>("return ReputationFrame:GetID()").unwrap(),
        3,
        "the id IS the tab slot the reference's tab 3 opens"
    );
    assert_eq!(
        s.eval::<i64>("return SkillFrame:GetID()").unwrap(),
        4,
        "and Skills is the reference's 4 beside it"
    );
    // **`CHARACTERFRAME_SUBFRAMES` is the hide-all list, not the tab map** — re-pointed here at
    // 1751, because our deleted `CharacterFrame.xml` wrote the two as one thing and the
    // reference's does not. Stock line 1 is
    // `{ "PaperDollFrame", "PetPaperDollFrame", "SkillFrame", "ReputationFrame", "HonorFrame" }` —
    // Skills THIRD and Reputation FOURTH, the opposite of their tab slots — and it is only ever
    // walked by `CharacterFrame_ShowSubFrame`, which shows the one it was named and hides the rest
    // (`CharacterFrame.lua:25-33`). So what this page needs from the list is membership: a page
    // missing from it is a page nothing ever hides, which is the "one page at a time" assertion at
    // the end of this test.
    assert!(
        s.eval::<bool>(
            "for _, v in CHARACTERFRAME_SUBFRAMES do if v == \"ReputationFrame\" then return true end end return false"
        )
        .unwrap(),
        "the hide-all list names this page"
    );

    // Eight rows in fifteen slots: the tail goes dark on BOTH twins, and a list that fits raises
    // neither the scroll bar nor its trough (the kit shows and hides the pair together).
    assert_eq!(s.eval::<i64>("return GetNumFactions()").unwrap(), 8);
    assert!(
        !shown(&mut s, "ReputationBar9"),
        "slot 9 has nothing to hold"
    );
    assert!(!shown(&mut s, "ReputationHeader9"), "neither twin shows");
    // No trough line here: it was ours, and the stock pane declares none (1875). Asserting a frame
    // that cannot exist would pass for the wrong reason.
    assert!(!visible(&mut s, "ReputationListScrollFrameScrollBar"));

    // Switching away hides it again through the same one-page-at-a-time switch.
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    assert!(!shown(&mut s, "ReputationFrame"), "one page at a time");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **A header slot draws the fold icon its state calls for, and a click folds the group.**
///
/// Two laws in one: the minus/plus art is chosen from `isCollapsed` (ref `ReputationFrame.lua`
/// l.54-58), and the click has to REPAINT — `CollapseFactionHeader` is a pure model write in this
/// engine where the client's C binding raises `UPDATE_FACTION`, so a handler that only calls the
/// global leaves the folded rows on screen (ReputationFrame.xml's deviation 3).
#[test]
fn a_header_row_paints_its_fold_icon_and_folds_on_click() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_reputation_page();

    assert_eq!(text_of(&mut s, "ReputationHeader1"), "Alliance");
    assert!(shown(&mut s, "ReputationHeader1"), "slot 1 is a header");
    assert!(!shown(&mut s, "ReputationBar1"), "so its bar is down");
    let icon = |s: &mut UiScript| {
        s.eval::<String>("return ReputationHeader1:GetNormalTexture():GetTexture() or \"\"")
            .unwrap()
    };
    assert!(
        icon(&mut s).contains("UI-MinusButton-Up"),
        "an expanded header wears the MINUS: {}",
        icon(&mut s)
    );
    assert_eq!(
        text_of(&mut s, "ReputationBar2FactionName"),
        "Ironforge",
        "its two children follow it"
    );

    click_center(&mut s, "ReputationHeader1");

    assert!(
        icon(&mut s).contains("UI-PlusButton-Up"),
        "a collapsed header wears the PLUS: {}",
        icon(&mut s)
    );
    assert_eq!(
        text_of(&mut s, "ReputationHeader2"),
        "Steamwheedle Cartel",
        "and slot 2 has re-bound to the next group — the repaint the click owes"
    );
    assert!(
        !shown(&mut s, "ReputationBar2"),
        "Ironforge is not on screen at all any more"
    );
    assert_eq!(
        s.eval::<i64>("return GetNumFactions()").unwrap(),
        6,
        "the two folded children leave the visible list"
    );

    // …and unfolding puts them straight back, through the same handler.
    click_center(&mut s, "ReputationHeader1");
    assert_eq!(text_of(&mut s, "ReputationBar2FactionName"), "Ironforge");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **A bar row paints its name, its standing word and its `FACTION_BAR_COLORS[standingID]` fill**,
/// over a bar whose range is the rank window NORMALIZED — `barMax - barMin` and `barValue - barMin`
/// (ref `ReputationFrame.lua` l.79-82), which is the entire reason the engine reports those three
/// absolute.
#[test]
fn a_bar_row_paints_name_standing_and_the_faction_bar_colour() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_reputation_page();

    assert_eq!(text_of(&mut s, "ReputationBar2FactionName"), "Ironforge");
    assert_eq!(
        text_of(&mut s, "ReputationBar2FactionStanding"),
        "Friendly",
        "standingID 5 is FACTION_STANDING_LABEL5"
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return ReputationBar2:GetMinMaxValues()")
            .unwrap(),
        (0.0, 6000.0),
        "the 3000..9000 rank window, normalized to 0..6000"
    );
    assert_eq!(
        s.eval::<f64>("return ReputationBar2:GetValue()").unwrap(),
        1000.0,
        "and 4000 standing is 1000 into it"
    );
    let (r, g, b) = s
        .eval::<(f32, f32, f32)>("return ReputationBar2:GetStatusBarColor()")
        .unwrap();
    assert_eq!(
        (r, g, b),
        (0.0, 0.6, 0.1),
        "FACTION_BAR_COLORS[5] — the green every friendly-and-better rank shares"
    );

    // The hover swaps the standing word for the raw progress and lights the glow pair; leaving puts
    // the word back (ref ReputationFrame.xml l.189-203).
    assert!(
        !shown(&mut s, "ReputationBar2Highlight1"),
        "glow starts down"
    );
    let (x, y) = s
        .eval::<(f32, f32)>(
            "return (ReputationBar2:GetLeft() + ReputationBar2:GetRight()) / 2, \
                    (ReputationBar2:GetTop() + ReputationBar2:GetBottom()) / 2",
        )
        .unwrap();
    s.mouse_move(x, y);
    assert_eq!(
        text_of(&mut s, "ReputationBar2FactionStanding"),
        "|cffffffff 1000 / 6000|r",
        "hovering shows the numbers, in the reference's own colour-coded form"
    );
    assert!(shown(&mut s, "ReputationBar2Highlight1"));
    assert!(shown(&mut s, "ReputationBar2Highlight2"));
    s.mouse_move(0.0, 0.0);
    assert_eq!(
        text_of(&mut s, "ReputationBar2FactionStanding"),
        "Friendly",
        "and leaving restores the word"
    );
    assert!(
        !shown(&mut s, "ReputationBar2Highlight1"),
        "the glow goes with it — this row is not the selected one"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The 15 row slots are a window onto the list, and the scroll offset is what moves it.** The
/// faux kit re-binds fixed slots to a moving data offset rather than scrolling anything (decision
/// 0251), so this is the assertion that the slots really are re-bound and not just clipped.
#[test]
fn the_scroll_offset_rebinds_the_fixed_row_slots() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_page(&s);
    s.set_unit("player", Some(player(40)));

    // One header over twenty children: 21 visible rows against 15 slots.
    let mut entries = vec![header(469, 11, "Alliance")];
    for i in 1..=20u32 {
        entries.push(entry(1000 + i, i, 469, &format!("Faction {i:02}")));
    }
    s.set_reputation(ReputationState {
        entries,
        watched: None,
    });
    s.run(r#"ToggleCharacter("ReputationFrame")"#).unwrap();
    s.resolve();

    assert_eq!(s.eval::<i64>("return GetNumFactions()").unwrap(), 21);
    assert_eq!(text_of(&mut s, "ReputationHeader1"), "Alliance");
    assert_eq!(text_of(&mut s, "ReputationBar1FactionName"), "");
    assert_eq!(text_of(&mut s, "ReputationBar15FactionName"), "Faction 14");
    assert!(
        visible(&mut s, "ReputationListScrollFrameScrollBar"),
        "21 rows in 15 slots raises the scroll bar"
    );
    // **The trough is GONE with our pane.** `ReputationListScrollFrameScrollBarTrough` was the one
    // name in this window that was never the reference's (1844) — it belongs to our own scroll
    // templates, and stock `ReputationFrame.xml` declares no such thing. B224's law (the 21/20/8
    // hang that drops each arrow into its socket) is unchanged and still asserted, by the
    // keybindings page's own trough, which is a window we still own (1875).
    // Scroll three rows down: every slot re-binds, and slot 1 stops being a header.
    s.run("FauxScrollFrame_SetOffset(ReputationListScrollFrame, 3) ReputationFrame_Update()")
        .unwrap();
    assert!(
        !shown(&mut s, "ReputationHeader1"),
        "slot 1 now holds a bar, so its header twin is down"
    );
    assert_eq!(text_of(&mut s, "ReputationBar1FactionName"), "Faction 03");
    assert_eq!(text_of(&mut s, "ReputationBar15FactionName"), "Faction 17");

    // Past the end the REFERENCE does not clamp the offset — it guards the BINDING.
    // `FauxScrollFrame_SetOffset` is two lines (`frame.offset = offset`), and the reference's own
    // `ReputationFrame_Update` walks its slots under `if ( factionIndex <= numFactions )`, hiding
    // the ones that fall off the end (ReputationFrame.lua). Our deleted kit clamped the offset
    // itself, which is why this used to read back 6. A player cannot reach this state — the bar's
    // range bounds a real scroll — so it is only ever an out-of-range programmatic ask (1860).
    s.run("FauxScrollFrame_SetOffset(ReputationListScrollFrame, 9) ReputationFrame_Update()")
        .unwrap();
    assert_eq!(
        s.eval::<i64>("return FauxScrollFrame_GetOffset(ReputationListScrollFrame)")
            .unwrap(),
        9,
        "the offset is stored as asked; the reference guards the binding, not the offset"
    );
    assert_eq!(text_of(&mut s, "ReputationBar1FactionName"), "Faction 09");
    assert!(
        !shown(&mut s, "ReputationBar15"),
        "slot 15 would want faction 24 of 21, so it stays hidden rather than binding past the end"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Clicking a bar opens the detail popup on that faction**, with its name, its description and
/// its three switches in the states the row's own flags call for — including the disabled, greyed
/// At War box a peace-forced faction gets (ref `ReputationFrame.lua` l.115-121). Clicking the same
/// bar again closes it, which is the reference's own toggle (l.152-153).
#[test]
fn clicking_a_bar_opens_the_detail_popup_on_that_faction() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_page(&s);
    s.set_unit("player", Some(player(40)));
    // Ironforge at war and peace-forced — the two flags the popup reads back off the row.
    let mut st = state();
    for e in &mut st.entries {
        if e.name == "Ironforge" {
            e.at_war = true;
            e.can_toggle_at_war = false;
        }
    }
    s.set_reputation(st);
    s.run(r#"ToggleCharacter("ReputationFrame")"#).unwrap();
    s.resolve();

    assert!(!shown(&mut s, "ReputationDetailFrame"), "closed to start");
    assert!(
        shown(&mut s, "ReputationBar2AtWarCheck"),
        "the at-war pennant flies on the row itself"
    );

    click_center(&mut s, "ReputationBar2");

    assert!(shown(&mut s, "ReputationDetailFrame"), "the popup opened");
    assert_eq!(
        s.eval::<i64>("return GetSelectedFaction()").unwrap(),
        2,
        "on Ironforge's visible row"
    );
    assert_eq!(text_of(&mut s, "ReputationDetailFactionName"), "Ironforge");
    assert_eq!(
        text_of(&mut s, "ReputationDetailFactionDescription"),
        "About the Ironforge."
    );
    assert!(
        shown(&mut s, "ReputationBar2Highlight1"),
        "and the selected row keeps its glow with the mouse away"
    );

    let checked = |s: &mut UiScript, box_name: &str| {
        s.eval::<bool>(&format!("return {box_name}:GetChecked() and true or false"))
            .unwrap()
    };
    assert!(
        checked(&mut s, "ReputationDetailAtWarCheckBox"),
        "at war, so the box is ticked"
    );
    assert!(
        !s.eval::<bool>("return ReputationDetailAtWarCheckBox:IsEnabled() ~= 0")
            .unwrap(),
        "peace-forced, so the box is dead"
    );
    let (r, g, b) = s
        .eval::<(f32, f32, f32)>("return ReputationDetailAtWarCheckBoxText:GetTextColor()")
        .unwrap();
    assert_eq!(
        (r, g, b),
        (0.5, 0.5, 0.5),
        "and its label greys with it (GRAY_FONT_COLOR)"
    );
    assert!(!checked(&mut s, "ReputationDetailInactiveCheckBox"));
    assert!(!checked(&mut s, "ReputationDetailMainScreenCheckBox"));

    // The reference's toggle: the same bar again closes the popup rather than re-opening it.
    click_center(&mut s, "ReputationBar2");
    assert!(!shown(&mut s, "ReputationDetailFrame"), "clicked shut");

    // **The whole ROW is the click target, not just the 137px bar.** The template's
    // `HitRectInsets left="-126"` (ref `ReputationFrame.xml` l.61-63) is a NEGATIVE inset, which
    // WIDENS the mouse rect back over the faction name — so clicking "Ironforge" selects it exactly
    // as clicking its bar does. Driven from the name's own centre, which is outside the bar's rect.
    let name_x = s
        .eval::<f32>(
            "return (ReputationBar2FactionName:GetLeft() + ReputationBar2FactionName:GetRight()) / 2",
        )
        .unwrap();
    let bar_left = s.eval::<f32>("return ReputationBar2:GetLeft()").unwrap();
    assert!(
        name_x < bar_left,
        "the name really does sit left of the bar ({name_x} < {bar_left})"
    );
    let y = s
        .eval::<f32>("return (ReputationBar2:GetTop() + ReputationBar2:GetBottom()) / 2")
        .unwrap();
    assert_eq!(
        s.hit_test_name(name_x, y).as_deref(),
        Some("ReputationBar2"),
        "the widened hit rect reaches the name"
    );
    s.mouse_move(name_x, y);
    s.mouse_button(name_x, y, "LeftButton", true);
    s.mouse_button(name_x, y, "LeftButton", false);
    assert!(
        shown(&mut s, "ReputationDetailFrame"),
        "and a click there opens the popup, the same as a click on the bar"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **B369 — a disabled box is still a box.** The reported symptom, at the quad level.
///
/// `ReputationDetailAtWarCheckBox` (stock `ReputationFrame.xml`) carries a `<NormalTexture>` and
/// **no `<DisabledTexture>`**, and `ReputationFrame_Update` `Disable()`s it for every faction whose
/// war flag cannot be toggled (`ReputationFrame.lua` l.115-120). The client's `SetState 0x779790`
/// gates its hide-old step on the new state having a texture, so the `UI-CheckBox-Up` box stays up
/// and the row reads "a greyed **At War** beside an empty box". Ours resolved the shown texture as
/// a pure function of the state, hid it, and left a bare grey label with nothing beside it — which
/// is how the Ironforge row drew.
///
/// Three rows, because the tick is the half that made the report confusing: a peace-forced faction
/// that is NOT at war shows the empty box (the shot), a peace-forced faction that IS at war shows
/// the box plus its grey `DisabledCheckedTexture` (the tick still draws), and a
/// toggleable one is the control that must not move.
#[test]
fn the_at_war_box_keeps_its_art_while_it_is_disabled() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_page(&s);
    s.set_unit("player", Some(player(40)));
    let mut st = state();
    for e in &mut st.entries {
        match e.name.as_str() {
            // The report: peace-forced and at peace.
            "Ironforge" => e.can_toggle_at_war = false,
            // Peace-forced and at war — the same box, wearing the grey tick.
            "Stormwind" => {
                e.can_toggle_at_war = false;
                e.at_war = true;
            }
            _ => {}
        }
    }
    s.set_reputation(st);
    s.run(r#"ToggleCharacter("ReputationFrame")"#).unwrap();
    s.resolve();

    /// Every texture the At War box itself draws, in painter order.
    fn box_art(s: &UiScript) -> Vec<String> {
        s.extract()
            .iter()
            .filter(|q| {
                s.quad_owner_name(q.target).as_deref() == Some("ReputationDetailAtWarCheckBox")
            })
            .filter_map(|q| match &q.content {
                QuadContent::Texture { path: Some(p), .. } => Some(p.clone()),
                _ => None,
            })
            .collect()
    }
    let up = "Interface\\Buttons\\UI-CheckBox-Up".to_string();

    // Ironforge (row 2): peace-forced, at peace — the photographed case.
    click_center(&mut s, "ReputationBar2");
    assert_eq!(
        s.eval::<i64>("return ReputationDetailAtWarCheckBox:IsEnabled()")
            .unwrap(),
        0,
        "the box is disabled for a faction whose war flag is locked"
    );
    assert_eq!(
        box_art(&s),
        vec![up.clone()],
        "and it still draws its box — the sticky shown texture"
    );

    // Stormwind (row 3): peace-forced and at war — box plus the grey disabled tick.
    click_center(&mut s, "ReputationBar3");
    assert_eq!(
        box_art(&s),
        vec![
            up.clone(),
            "Interface\\Buttons\\UI-CheckBox-Check-Disabled".to_string()
        ],
        "a peace-forced faction at war keeps the box under its grey tick"
    );

    // Booty Bay (row 5): the control — toggleable, and unchanged by any of this.
    click_center(&mut s, "ReputationBar5");
    assert_eq!(
        s.eval::<i64>("return ReputationDetailAtWarCheckBox:IsEnabled()")
            .unwrap(),
        1
    );
    assert_eq!(box_art(&s), vec![up], "a live box is the same box");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The watch bar shows what is being watched, and swaps posture at max level.**
///
/// Below max level it STACKS above the XP bar, 8px tall, wearing the `UI-ReputationWatchBar` end
/// art; at max level it REPLACES the XP bar, 13px tall on MainMenuBar's own top, wearing the dwarf
/// art instead so the swap reads as the same bar (ref `ReputationFrame.lua` l.184-230). The rested
/// tick goes with the XP bar it rode, which is one of the two guards that were cut from
/// `ExhaustionTick_Update` as unreachable while this bar could never show.
#[test]
fn the_watch_bar_shows_the_watched_factions_progress_and_swaps_at_max_level() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_page(&s);
    s.set_unit("player", Some(player(40)));
    assert!(
        !shown(&mut s, "ReputationWatchBar"),
        "nothing watched: the bar stays down, as it has since it was a stub"
    );

    // Ironforge is rep slot 20; watching it is a server field, so it rides the push.
    let mut st = state();
    st.watched = Some(20);
    s.set_reputation(st);
    s.fire_event("UPDATE_FACTION", vec![]);
    s.resolve();

    assert!(shown(&mut s, "ReputationWatchBar"), "the bar came up");
    assert_eq!(
        text_of(&mut s, "ReputationWatchStatusBarText"),
        "Ironforge 1000 / 6000",
        "name plus the NORMALIZED progress, the reference's own string"
    );
    let (r, g, b) = s
        .eval::<(f32, f32, f32)>("return ReputationWatchStatusBar:GetStatusBarColor()")
        .unwrap();
    assert_eq!((r, g, b), (0.0, 0.6, 0.1), "FACTION_BAR_COLORS[5] again");
    assert_eq!(
        s.eval::<(f64, f64)>("return ReputationWatchStatusBar:GetMinMaxValues()")
            .unwrap(),
        (0.0, 6000.0)
    );
    assert!(
        shown(&mut s, "MainMenuExpBar"),
        "below 60 the strip STACKS on the XP bar rather than replacing it"
    );
    assert!(
        shown(&mut s, "ReputationWatchBarTexture0"),
        "rep end art up"
    );
    assert!(!shown(&mut s, "ReputationXPBarTexture0"), "dwarf art down");
    assert_eq!(
        s.eval::<f64>("return ReputationWatchStatusBar:GetHeight()")
            .unwrap(),
        8.0
    );

    // **`UIParent_ManageFramePositions`'s `reputation` branch fires for the first time.** It has
    // been live in UIParent.xml since decision 0272 and gated on
    // `ReputationWatchBar:IsShown() and MainMenuExpBar:IsShown()`, which that file's own header
    // notes "still cannot fire" — it can now, and this is the check that it does. `PETACTIONBAR_YPOS`
    // is the cheapest witness: an `isVar` row the pass writes as a plain global (baseY 97, plus the
    // row's own `reputation = 9`), so it needs no frame loaded to read back.
    assert_eq!(
        s.eval::<f64>("return PETACTIONBAR_YPOS").unwrap(),
        106.0,
        "the bottom stack lifts by the row's 9 while the stacked watch bar is up"
    );
    s.run("ReputationWatchBar:Hide() UIParent_ManageFramePositions()")
        .unwrap();
    assert_eq!(
        s.eval::<f64>("return PETACTIONBAR_YPOS").unwrap(),
        97.0,
        "and drops back to the base when it goes"
    );
    s.run("ReputationWatchBar:Show() UIParent_ManageFramePositions()")
        .unwrap();

    // Ding to 60: the reputation strip takes the XP bar's place, art and all.
    s.set_unit("player", Some(player(60)));
    s.fire_event(
        "PLAYER_LEVEL_UP",
        vec![benilla_ui::script::ScriptValue::Int(60)],
    );
    s.resolve();

    assert!(shown(&mut s, "ReputationWatchBar"), "still watching");
    assert!(
        !shown(&mut s, "MainMenuExpBar"),
        "at 60 the strip REPLACES the XP bar"
    );
    assert!(
        !shown(&mut s, "MainMenuBarMaxLevelBar"),
        "and the brass rail stays down — the watched bar is what fills that space"
    );
    assert!(
        !shown(&mut s, "ExhaustionTick"),
        "no XP strip, no rested tick"
    );
    assert!(shown(&mut s, "ReputationXPBarTexture0"), "dwarf art up");
    assert!(!shown(&mut s, "ReputationWatchBarTexture0"), "rep art down");
    assert_eq!(
        s.eval::<f64>("return ReputationWatchStatusBar:GetHeight()")
            .unwrap(),
        13.0
    );

    // Stop watching: at 60 with nothing watched the brass rail is what takes the XP bar's place.
    let mut st = state();
    st.watched = None;
    s.set_reputation(st);
    s.fire_event("UPDATE_FACTION", vec![]);
    s.resolve();
    assert!(!shown(&mut s, "ReputationWatchBar"));
    assert!(shown(&mut s, "MainMenuBarMaxLevelBar"), "the rail is back");
    assert!(!shown(&mut s, "MainMenuExpBar"));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **B368 — "Show as Experience Bar", the whole round trip.** The reported symptom is the one thing
/// no test here covered: every check above either drives `SetWatchedFactionIndex` from Lua or pushes
/// `watched` in by hand, so the box itself — the only thing a player touches — had no guard at all
/// between the pointer and the wire.
///
/// What the round trip is, and why it LOOKS broken when a link is missing: the box's `<OnClick>`
/// (stock `ReputationFrame.xml` l.839-848) calls `SetWatchedFactionIndex(GetSelectedFaction())` and
/// then `ReputationWatchBar_Update()` — and that update reads `GetWatchedFactionInfo()`, which is
/// still nil, because watching is **not optimistic** (`0x4d6b60`: the slot is
/// `PLAYER_FIELD_WATCHED_FACTION_INDEX`, a server field with no client mirror). So the click's own
/// frame moves NOTHING on screen by design; the bar comes up one round trip later, when the
/// descriptor update lands as a fresh push plus `UPDATE_FACTION`. A server that never answers the
/// opcode therefore reads exactly as "the box does nothing", and so does anything between the
/// pointer and the send.
///
/// Driven through the real pointer pipeline, so a frame eating the box's clicks fails here.
#[test]
fn the_show_as_experience_bar_box_sends_the_watch_and_the_server_brings_the_bar_up() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_reputation_page();
    // The row's checkmark is anchored past `factionName:GetStringWidth()`, so this test needs a
    // font: without a measurer installed every metric reads 0 (`script::measure`'s "absent by
    // default" state) and the mark would land on the name's first letter for that reason rather
    // than for a real one. 6 units a character, the file-wide stand-in.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    let _ = s.take_reputation_sends();

    // Ironforge (visible row 2, reputation slot 20) — select it and open the popup by clicking it.
    click_center(&mut s, "ReputationBar2");
    assert!(shown(&mut s, "ReputationDetailFrame"), "the popup opened");
    assert!(
        !s.eval::<bool>("return ReputationDetailMainScreenCheckBox:GetChecked() and true or false")
            .unwrap(),
        "nothing watched, so the box starts clear"
    );
    assert!(
        !shown(&mut s, "ReputationWatchBar"),
        "and the strip is down"
    );

    // THE CLICK. The engine ticks the box before the handler runs (the reference's own order), so
    // the handler takes the `GetChecked()` branch that watches rather than the one that clears.
    click_center(&mut s, "ReputationDetailMainScreenCheckBox");
    assert_eq!(
        s.take_reputation_sends(),
        [benilla_ui::script::ReputationSend::Watch(Some(20))],
        "one send, carrying Ironforge's reputation SLOT — not its visible row"
    );
    assert!(
        !shown(&mut s, "ReputationWatchBar"),
        "and nothing on screen moved: watching is not optimistic"
    );

    // The server's answer: the descriptor update arrives as a fresh push, and the event with it.
    let mut watched = state();
    watched.watched = Some(20);
    s.set_reputation(watched);
    s.fire_event("UPDATE_FACTION", vec![]);
    s.resolve();

    assert!(
        s.eval::<bool>("return ReputationDetailMainScreenCheckBox:GetChecked() and true or false")
            .unwrap(),
        "NOW the box reads ticked — off the server's field, not off the click"
    );
    assert!(shown(&mut s, "ReputationWatchBar"), "and the strip is up");
    assert_eq!(
        text_of(&mut s, "ReputationWatchStatusBarText"),
        "Ironforge 1000 / 6000"
    );
    // The list's own witness: the row grows a checkmark, re-anchored past the name's string width
    // (ref `ReputationFrame.lua` l.96-103) — the half a player sees without leaving the pane.
    assert!(
        shown(&mut s, "ReputationBar2Check"),
        "the watched row wears the checkmark"
    );
    assert!(
        !shown(&mut s, "ReputationBar3Check"),
        "and no other row does"
    );
    let (check_l, name_l) = s
        .eval::<(f32, f32)>(
            "return ReputationBar2Check:GetLeft(), ReputationBar2FactionName:GetLeft()",
        )
        .unwrap();
    // "Ironforge" is nine characters, so the stand-in font makes the offset exactly 54 — the
    // reference's `factionName:GetStringWidth()`, not a guess at one.
    assert_eq!(
        check_l - name_l,
        54.0,
        "the mark sits the name's own string width to its right"
    );

    // Untick: the same box, the same funnel, and `None` — NOT slot 0, which is a real faction.
    click_center(&mut s, "ReputationDetailMainScreenCheckBox");
    assert_eq!(
        s.take_reputation_sends(),
        [benilla_ui::script::ReputationSend::Watch(None)]
    );
    let mut cleared = state();
    cleared.watched = None;
    s.set_reputation(cleared);
    s.fire_event("UPDATE_FACTION", vec![]);
    s.resolve();
    assert!(!shown(&mut s, "ReputationWatchBar"), "the strip goes down");
    assert!(!shown(&mut s, "ReputationBar2Check"), "and the row's mark");
    assert!(!s
        .eval::<bool>("return ReputationDetailMainScreenCheckBox:GetChecked() and true or false")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
