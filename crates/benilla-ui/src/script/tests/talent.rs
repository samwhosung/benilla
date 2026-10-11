//! The talent bindings over a pushed snapshot, the learn queue, and `SetTalent`: the talent builder
//! for a passive talent, the spell tooltip with the talent lines interleaved for an exceptional one.

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
        assert(rank == 3 and max == 5 and exc == nil and meets == 1)
        assert(icon == "Interface\\Icons\\Spell_Fire_FlameBolt")
        assert(GetTalentInfo(1, 9) == nil)

        -- The prereq triplets, flat (the reference walks arg[5], arg[6], arg[7], ...).
        local pt, pc, pl = GetTalentPrereqs(1, 2)
        assert(pt == 1 and pc == 1 and pl == nil)
        assert(GetTalentPrereqs(1, 1) == nil, "no prereqs is empty")

        local cp1, cp2 = UnitCharacterPoints("player")
        assert(cp1 == 2 and cp2 == 1)
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// `isExceptional` is `1.0` when `[rec+0x4c] & 1` and nil otherwise (`0x4f33e8`): never `0`, which
/// Lua reads as true, and `meetsPrereq` and each prereq's `isLearnable` are 1 or nil too
/// (`0x4f3403`-`0x4f3443`, `0x4f3676`, `0x4f3644`). `GetTalentPrereqs` answers three values per edge.
#[test]
fn talent_flags_are_one_or_nil() {
    let mut s = script();
    let mut state = one_tab_state();
    state.talents[0][0].exceptional = true;
    state.talents[0][1].meets_prereq = false;
    state.talents[0][1].prereqs.push(TalentPrereqView {
        tier: 1,
        column: 2,
        learnable: true,
    });
    s.set_talents(state);
    s.run(
        r#"
        local _, _, _, _, _, _, exc, meets = GetTalentInfo(1, 1)
        assert(exc == 1 and type(exc) == "number" and meets == 1)
        local _, _, _, _, _, _, exc2, meets2 = GetTalentInfo(1, 2)
        assert(exc2 == nil, "an ordinary talent is not exceptional")
        assert(meets2 == nil, "an unmet prerequisite is nil, not false")

        local t1, c1, l1, t2, c2, l2 = GetTalentPrereqs(1, 2)
        assert(t1 == 1 and c1 == 1 and l1 == nil)
        assert(t2 == 1 and c2 == 2 and l2 == 1 and type(l2) == "number")
    "#,
    )
    .unwrap();
    assert_eq!(s.arity("GetTalentInfo(1, 2)").unwrap(), 8);
    assert_eq!(s.arity("GetTalentPrereqs(1, 2)").unwrap(), 6);
    assert!(s.take_errors().is_empty());
}

#[test]
fn learn_talent_queues_for_the_app_drain() {
    let mut s = script();
    s.set_talents(one_tab_state());
    s.run("LearnTalent(1, 1); LearnTalent(1, 2)").unwrap();
    assert_eq!(s.take_talent_learns(), vec![(1, 1), (1, 2)]);
    assert!(s.take_talent_learns().is_empty(), "drain empties the queue");
    assert!(s.take_errors().is_empty());
}

/// Stand-in values for the three talent keys, not the shipped wording: the tests check which key
/// each line reaches.
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

/// A passive talent's full spell view, every body line filled, so a test sees which ones the talent
/// builder drops.
fn passive_body(description: &str) -> SpellTooltipView {
    SpellTooltipView {
        name: "Improved Fireball".into(),
        cost: Some("35 Mana".into()),
        range: Some("30 yd range".into()),
        cast_time: Some("Instant".into()),
        cooldown: Some("6 sec cooldown".into()),
        requires_item: Some("Requires One-Handed Axes".into()),
        requires_form: Some("Requires Battle Stance".into()),
        reagents: Some("Reagents: Light Feather".into()),
        chance: Some("2.62% chance to dodge".into()),
        description: description.into(),
        ..Default::default()
    }
}

/// `SetTalent` on a passive talent is the talent builder `0x52b0a0`: name, `TOOLTIP_TALENT_RANK`
/// (white), the gold description, a `" "` spacer, `TOOLTIP_TALENT_NEXT_RANK` and the next rank's
/// gold description, green `TOOLTIP_TALENT_LEARN`; no cost, range, cast, requirement, reagent or
/// chance line.
#[test]
fn set_talent_on_a_passive_talent_is_the_talent_builder() {
    let mut s = script();
    seed_talent_strings(&mut s);
    s.set_talents(one_tab_state());
    s.set_spell_tooltip(
        11070,
        passive_body("Reduces the casting time of your Fireball spell by 0.3 sec."),
    );
    s.set_spell_tooltip(
        11071,
        passive_body("Reduces the casting time of your Fireball spell by 0.4 sec."),
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "TB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetTalent(1, 1)
        assert(tt:IsShown(), "SetTalent shows")
        -- name, rank, desc, spacer, next-rank header, next desc, learn hint = 7 lines.
        assert(tt:NumLines() == 7, "got " .. tt:NumLines())
        assert(TTTextLeft1:GetText() == "Improved Fireball")
        assert(TTTextLeft2:GetText() == "[RANK 3/5]", "got " .. TTTextLeft2:GetText())
        assert(TTTextLeft3:GetText() == "Reduces the casting time of your Fireball spell by 0.3 sec.")
        assert(TTTextLeft4:GetText() == " ", "the spacer, got " .. tostring(TTTextLeft4:GetText()))
        assert(TTTextLeft5:GetText() == "[NEXT_RANK]")
        assert(TTTextLeft6:GetText() == "Reduces the casting time of your Fireball spell by 0.4 sec.")
        assert(TTTextLeft7:GetText() == "[LEARN]")
        for i = 1, 7 do
            assert(getglobal("TTTextRight" .. i):GetText() == nil, "no right column on line " .. i)
        end
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    let green = quads.iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t == "[LEARN]" && c[0] < 1e-6 && (c[1] - 1.0).abs() < 1e-6)
    });
    assert!(green, "the learn hint is green");
    // The spacer is `0x530380`'s gold, not `0x5303b0`'s caller colour.
    let gold_spacer = quads.iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t == " " && (c[0] - 1.0).abs() < 1e-6 && (c[1] - 210.0 / 255.0).abs() < 1e-3 && c[2] < 1e-6)
    });
    assert!(gold_spacer, "the spacer is gold");
    assert!(s.take_errors().is_empty());
}

/// An exceptional talent (`Talent.dbc` Flags bit 0) is the spell builder `0x52e610` instead, whose
/// cost, cast and required-item lines stay: Holy Shield's "Requires Shields".
#[test]
fn set_talent_on_an_exceptional_talent_keeps_the_spell_body() {
    let mut s = script();
    seed_talent_strings(&mut s);
    let mut state = one_tab_state();
    let holy_shield = &mut state.talents[0][1];
    holy_shield.name = "Holy Shield".into();
    holy_shield.exceptional = true;
    holy_shield.req_lines = vec!["Requires 30 points in Protection Talents".into()];
    s.set_talents(state);
    s.set_spell_tooltip(
        11119,
        SpellTooltipView {
            name: "Holy Shield".into(),
            cost: Some("150 Mana".into()),
            cast_time: Some("Instant cast".into()),
            requires_item: Some("Requires Shields".into()),
            description: "Increases chance to block by 30% for 10 sec.".into(),
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "TB3"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT3")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetTalent(1, 2)
        local want = {
            "Holy Shield", "[RANK 0/5]", "Requires 30 points in Protection Talents", "150 Mana",
            "Instant cast", "Requires Shields", "Increases chance to block by 30% for 10 sec.",
        }
        assert(tt:NumLines() == table.getn(want), "got " .. tt:NumLines())
        for i, text in ipairs(want) do
            local got = getglobal("TT3TextLeft" .. i):GetText()
            assert(got == text, "line " .. i .. ": " .. tostring(got))
        end
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// A missing spell view renders the rank line alone and asks for the view once.
#[test]
fn set_talent_locked_reqs_and_the_ask_once_miss() {
    let mut s = script();
    seed_talent_strings(&mut s);
    s.set_talents(one_tab_state());
    // No spell view pushed for Ignite (11119).
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
