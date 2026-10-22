//! The skill list's folds and rebuilds as `SkillInfo.cpp` holds them: the rows are one sorted
//! array, the visible ones first (`0x4d3070`), which every index verb bounds by the whole row count
//! (`0xb731b8`); collapse and expand fold the category of the row an index names and re-run the
//! finalize `0x4d2ed0`, which signals `SKILL_LINES_CHANGED` (`0x4d2fec`); and only a rebuild
//! (`0x4d2cb0`) resets the folds (`0x4d2ce2`).

use crate::script::{SkillEntry, SkillsState, UiScript};

fn entry(skill_id: u32, name: &str, value: u32, category_id: u32, category: &str) -> SkillEntry {
    SkillEntry {
        skill_id,
        name: name.into(),
        value,
        max: 300,
        mono: false,
        temp_bonus: 0,
        perm_bonus: 0,
        min_level: 0,
        cost_index: 0,
        category_id,
        category_name: category.into(),
        category_order: category_id,
        description: String::new(),
        abandonable: false,
        step: 0,
        flags: 0,
        tier_id: 0,
    }
}

/// Rows: 1 Weapon Skills, 2 Defense, 3 Swords, 4 Professions, 5 First Aid, 6 Fishing.
fn state() -> SkillsState {
    SkillsState {
        entries: vec![
            entry(43, "Swords", 200, 1, "Weapon Skills"),
            entry(95, "Defense", 180, 1, "Weapon Skills"),
            entry(129, "First Aid", 57, 2, "Professions"),
            entry(356, "Fishing", 1, 2, "Professions"),
        ],
    }
}

fn seated() -> UiScript {
    let mut s = UiScript::new().expect("VM");
    s.set_skills(state());
    s.run(
        "Heard = {} local f = CreateFrame('Frame') f:RegisterEvent('SKILL_LINES_CHANGED') \
         f:SetScript('OnEvent', function() table.insert(Heard, GetNumSkillLines()) end)",
    )
    .unwrap();
    s
}

/// The visible count each `SKILL_LINES_CHANGED` handler read since the last call.
fn heard(s: &UiScript) -> Vec<i64> {
    let out = s.eval::<Vec<i64>>("return Heard").unwrap();
    s.run("Heard = {}").unwrap();
    out
}

fn count(s: &UiScript) -> i64 {
    s.eval::<i64>("return GetNumSkillLines()").unwrap()
}

/// `0x4d3210` and `0x4d3270` read the category of the row at the index (`+0x4`, `0x4d322f`,
/// `0x4d328f`) and test no header bit, so a skill row's index folds its own group.
#[test]
fn a_skill_rows_index_folds_and_unfolds_its_own_group() {
    let s = seated();
    s.run("CollapseSkillHeader(5)").unwrap(); // First Aid
    assert_eq!(count(&s), 4, "Professions folded by its skill row");
    s.run("CollapseSkillHeader(2)").unwrap(); // Defense
    assert_eq!(count(&s), 2, "Weapon Skills folded by its skill row");
    s.run("ExpandSkillHeader(1)").unwrap();
    assert_eq!(count(&s), 4);
    assert!(s.errors().is_empty(), "{:#?}", s.errors());
}

/// Both verbs end in the finalize `0x4d2ed0` (the tail jumps at `0x4d3260` and `0x4d32c0`, the
/// calls at `0x4d3bb1` and `0x4d3c11`), so `SKILL_LINES_CHANGED` fires inside the call, over the
/// folded list; an index past the last row returns before it (`0x4d3218`).
#[test]
fn collapse_and_expand_signal_skill_lines_changed_inside_the_call() {
    let s = seated();
    s.run("CollapseSkillHeader(1) InCall = GetNumSkillLines()")
        .unwrap();
    assert_eq!(heard(&s), [4], "the handler read the folded list");
    assert_eq!(s.eval::<i64>("return InCall").unwrap(), 4);
    s.run("ExpandSkillHeader(0)").unwrap();
    assert_eq!(heard(&s), [6]);
    s.run("CollapseSkillHeader(0)").unwrap();
    assert_eq!(heard(&s), [2]);
    s.run("ExpandSkillHeader(99) CollapseSkillHeader(99)")
        .unwrap();
    assert!(
        heard(&s).is_empty(),
        "past the last row: no fold, no signal"
    );
    assert!(s.errors().is_empty(), "{:#?}", s.errors());
}

/// The folded rows sort after the visible ones (`0x4d3095`) and stay in the row array, which the
/// index verbs bound by the whole count (`0x4d3210`, `0x4d3676`, `0x4d405d`): an index past the
/// visible count reaches them.
#[test]
fn an_index_past_the_visible_rows_reaches_the_folded_ones() {
    let s = seated();
    s.run("CollapseSkillHeader(1)").unwrap();
    assert_eq!(count(&s), 4);
    assert_eq!(
        s.eval::<String>("return (GetSkillLineInfo(5))").unwrap(),
        "Defense",
        "row 5 is the first folded row"
    );
    s.run("ExpandSkillHeader(5)").unwrap();
    assert_eq!(count(&s), 6, "its index unfolds its group");
    assert!(s.errors().is_empty(), "{:#?}", s.errors());
}

/// A push of new skill values is no rebuild: the folds stay (`0x4d2ce2` is the rebuild's alone),
/// and the selection, a skill line id no rebuild clears (`0xb731c4`), comes back with its skill.
#[test]
fn a_push_keeps_the_folds_and_the_selection() {
    let mut s = seated();
    s.run("SetSelectedSkill(3) CollapseSkillHeader(4)").unwrap();
    let mut ticked = state();
    ticked.entries[0].value = 201;
    s.set_skills(ticked);
    assert_eq!(count(&s), 4, "Professions stays folded");
    assert_eq!(
        s.eval::<(String, i64)>("local n,_,_,r = GetSkillLineInfo(3) return n,r")
            .unwrap(),
        ("Swords".into(), 201),
        "the row reads the pushed value"
    );
    assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 3);
    assert!(s.errors().is_empty(), "{:#?}", s.errors());
}

/// The world-enter cascade rebuilds the list (`0x4909f1` → `0x4d2c90` → `0x4d2cb0`): every
/// category opens and the finalize signals `SKILL_LINES_CHANGED`.
#[test]
fn the_world_enter_cascade_rebuilds_the_list() {
    let mut s = seated();
    s.run("CollapseSkillHeader(0)").unwrap();
    let _ = heard(&s);
    s.fire_world_enter();
    assert_eq!(heard(&s), [6]);
    assert!(s.errors().is_empty(), "{:#?}", s.errors());
}
