//! The talent engine seam: the Era binding tuples over a pushed snapshot, the
//! learn-click queue, and the `SetTalent` tooltip — the spell builder with the talent
//! interleave (rank line white, req lines red, next-rank block, learn hint green).

use super::common::script;
use crate::script::*;

fn one_tab_state() -> TalentUiState {
    TalentUiState {
        tabs: vec![
            TalentTabView {
                name: "Fire".into(),
                background: "MageFire".into(),
                points_spent: 7,
            },
            TalentTabView {
                name: "Frost".into(),
                background: "MageFrost".into(),
                points_spent: 0,
            },
        ],
        talents: vec![
            vec![
                TalentView {
                    name: "Improved Fireball".into(),
                    texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
                    tier: 1,
                    column: 1,
                    rank: 3,
                    max_rank: 5,
                    exceptional: false,
                    meets_prereq: true,
                    prereqs: Vec::new(),
                    display_spell: 11070,
                    next_spell: 11071,
                    req_lines: Vec::new(),
                    learnable: true,
                },
                TalentView {
                    name: "Ignite".into(),
                    texture: None,
                    tier: 2,
                    column: 2,
                    rank: 0,
                    max_rank: 5,
                    exceptional: false,
                    meets_prereq: true,
                    prereqs: vec![TalentPrereqView {
                        tier: 1,
                        column: 1,
                        learnable: false,
                    }],
                    display_spell: 11119,
                    next_spell: 0,
                    req_lines: vec!["Requires 5 points in Improved Fireball".into()],
                    learnable: false,
                },
            ],
            Vec::new(),
        ],
        points: (2, 1),
    }
}

/// The Era tuples read back exactly what the app pushed — including the 1-based grid seats,
/// the flat prereq triplets, and the points pair.
#[test]
fn bindings_read_the_pushed_snapshot() {
    let mut s = script();
    s.set_talents(one_tab_state());
    s.run(
        r#"
        assert(GetNumTalentTabs() == 2)
        local name, texture, points, file = GetTalentTabInfo(1)
        assert(name == "Fire" and texture == nil and points == 7 and file == "MageFire")
        assert(GetTalentTabInfo(3) == nil, "out of range is nil")
        assert(GetNumTalents(1) == 2 and GetNumTalents(2) == 0 and GetNumTalents(9) == 0)

        local n, icon, tier, col, rank, max, exc, meets = GetTalentInfo(1, 1)
        assert(n == "Improved Fireball" and tier == 1 and col == 1)
        assert(rank == 3 and max == 5 and exc == 0 and meets == true)
        assert(icon == "Interface\\Icons\\Spell_Fire_FlameBolt")
        assert(GetTalentInfo(1, 9) == nil)

        -- The prereq triplets, flat (the reference walks arg[5], arg[6], arg[7], ...).
        local pt, pc, pl = GetTalentPrereqs(1, 2)
        assert(pt == 1 and pc == 1 and pl == false)
        assert(GetTalentPrereqs(1, 1) == nil, "no prereqs is empty")

        local cp1, cp2 = UnitCharacterPoints("player")
        assert(cp1 == 2 and cp2 == 1)
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// LearnTalent queues (tab, index) verbatim for the app's wire drain.
#[test]
fn learn_talent_queues_for_the_app_drain() {
    let mut s = script();
    s.set_talents(one_tab_state());
    s.run("LearnTalent(1, 1); LearnTalent(1, 2)").unwrap();
    assert_eq!(s.take_talent_learns(), vec![(1, 1), (1, 2)]);
    assert!(s.take_talent_learns().is_empty(), "drain empties the queue");
    assert!(s.take_errors().is_empty());
}

/// A stand-in string table for the talent tail's three keys — **deliberately not the shipped
/// wording**, because what is under test is which key each line reaches and what fills it, never
/// what the sentence says.
fn seed_talent_strings(s: &mut UiScript) {
    s.run(
        r#"
        TOOLTIP_TALENT_RANK      = "[RANK %d/%d]"
        TOOLTIP_TALENT_NEXT_RANK = "[NEXT_RANK]"
        TOOLTIP_TALENT_LEARN     = "[LEARN]"
    "#,
    )
    .unwrap();
}

/// SetTalent = the spell builder + the talent interleave: name, TOOLTIP_TALENT_RANK white, cost
/// line, gold description, TOOLTIP_TALENT_NEXT_RANK + the next rank's gold description, green
/// TOOLTIP_TALENT_LEARN.
#[test]
fn set_talent_renders_the_interleaved_tooltip() {
    let mut s = script();
    seed_talent_strings(&mut s);
    s.set_talents(one_tab_state());
    s.set_spell_tooltip(
        11070,
        SpellTooltipView {
            name: "Improved Fireball".into(),
            cast_time: Some("Instant".into()),
            description: "Reduces the casting time of your Fireball spell by 0.3 sec.".into(),
            ..Default::default()
        },
    );
    s.set_spell_tooltip(
        11071,
        SpellTooltipView {
            name: "Improved Fireball".into(),
            description: "Reduces the casting time of your Fireball spell by 0.4 sec.".into(),
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "TB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetTalent(1, 1)
        assert(tt:IsShown(), "SetTalent shows")
        -- name, rank, Instant, desc, next-rank header, next desc, learn hint = 7 lines.
        assert(tt:NumLines() == 7, "got " .. tt:NumLines())
        assert(TTTextLeft1:GetText() == "Improved Fireball")
        assert(TTTextLeft2:GetText() == "[RANK 3/5]", "got " .. TTTextLeft2:GetText())
        assert(TTTextLeft3:GetText() == "Instant")
        assert(TTTextLeft5:GetText() == "[NEXT_RANK]")
        assert(TTTextLeft7:GetText() == "[LEARN]")
    "#,
    )
    .unwrap();
    // The learn hint wears the tooltip green; the requirement red is exercised below.
    s.resolve();
    let quads = s.extract();
    let green = quads.iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t == "[LEARN]" && c[0] < 1e-6 && (c[1] - 1.0).abs() < 1e-6)
    });
    assert!(green, "the learn hint is green");
    assert!(s.take_errors().is_empty());
}

/// A locked talent shows its red requirement line; a missing spell view falls back to the rank
/// line alone and records the ask for the app resolver (the shared ask-once channel).
#[test]
fn set_talent_locked_reqs_and_the_ask_once_miss() {
    let mut s = script();
    seed_talent_strings(&mut s);
    s.set_talents(one_tab_state());
    // No spell view pushed for Ignite (11119): the render falls back, the ask is recorded.
    s.run(
        r#"
        local a = CreateFrame("Button", "TB2"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT2")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetTalent(1, 2)
        assert(tt:IsShown())
        assert(TT2TextLeft1:GetText() == "[RANK 0/5]", "fallback shows the rank head")
    "#,
    )
    .unwrap();
    let asks = s.take_spell_tooltip_asks();
    assert!(
        asks.contains(&11119),
        "the display spell was asked: {asks:?}"
    );

    // With the view landed, the full render carries the red requirement line.
    s.set_spell_tooltip(
        11119,
        SpellTooltipView {
            name: "Ignite".into(),
            description: "Your critical strikes ignite the target.".into(),
            ..Default::default()
        },
    );
    s.run(
        r#"
        local tt = getglobal("TT2")
        tt:SetOwner(getglobal("TB2"), "ANCHOR_RIGHT")
        tt:SetTalent(1, 2)
        -- name, rank, req(red), desc = 4 lines; rank 0 has no next block, locked has no hint.
        assert(tt:NumLines() == 4, "got " .. tt:NumLines())
        assert(TT2TextLeft3:GetText() == "Requires 5 points in Improved Fireball")
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    let red = quads.iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t.starts_with("Requires 5") && (c[0] - 1.0).abs() < 1e-6 && c[1] < 0.2)
    });
    assert!(red, "the requirement line is red");
    assert!(s.take_errors().is_empty());
}
