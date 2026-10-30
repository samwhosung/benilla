//! The GameObject builder's handler lines (`0x52aad4`) and lock lines (`0x52aae2`-`0x52ace7`).

use benilla_formats::{LockSlot, GO_STATE_READY, LOCK_KEY_ITEM, LOCK_KEY_SKILL, MAX_LOCK_SLOTS};
use benilla_ui::script::{TooltipText, TooltipTint};
use benilla_ui::strings::Strings;

use super::plate::{go_handler_lines, go_lock_lines, GoLock};
use crate::go_templates::MeetingStoneTemplate;
use crate::target::lock::{LockMatch, LockOutcome};

/// Marked stand-ins: every lock key reads "Requires %s" in enUS.
fn get(key: &str) -> Option<String> {
    Some(
        match key {
            "LOCKED" => "[LOCKED]",
            "LOCKED_WITH_ITEM" => "[ITEM %s]",
            "LOCKED_WITH_SPELL" => "[SPELL %s]",
            "LOCKED_WITH_SPELL_KNOWN" => "[KNOWN %s]",
            _ => return None,
        }
        .to_string(),
    )
}

/// Each line as the VM adds it, read through `get`: a template that is missing or fills to
/// nothing adds none.
fn read(
    lines: Vec<(TooltipText, TooltipTint)>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Vec<(String, TooltipTint)> {
    lines
        .into_iter()
        .filter_map(|(text, tint)| {
            let text = text.text(Strings::Host(get));
            (!text.is_empty()).then_some((text, tint))
        })
        .collect()
}

fn row(slot0: LockSlot) -> [LockSlot; MAX_LOCK_SLOTS] {
    let mut slots = [LockSlot::default(); MAX_LOCK_SLOTS];
    slots[0] = slot0;
    slots
}

/// A herb's lock: Herbalism (`LockType` 2), skill 100, opened while not flag-locked.
const HERB: LockSlot = LockSlot {
    key_type: LOCK_KEY_SKILL,
    index: 2,
    skill: 100,
    action: 0,
};

fn lines(
    slots: &[LockSlot; MAX_LOCK_SLOTS],
    flag_locked: bool,
    outcome: LockOutcome,
    matched: Option<LockMatch>,
) -> Vec<(String, TooltipTint)> {
    read(
        go_lock_lines(
            GoLock {
                slots: Some(slots),
                state: GO_STATE_READY,
                flag_locked,
                outcome: Some(outcome),
                matched,
            },
            &|entry| (entry == 5396).then(|| "Key to Searing Gorge".to_string()),
            &|index| (index == 2).then(|| "Herbalism".to_string()),
        ),
        &get,
    )
}

/// A herb a herbalist knows the spell for reads `LOCKED_WITH_SPELL_KNOWN` in the ramp of its value
/// against the requirement, a short skill too (the resolver's out-params hold the match it
/// tried, `0x5f84f8`-`0x5f850d`); with no such spell, red `LOCKED_WITH_SPELL`.
#[test]
fn a_gathering_nodes_requires_line_takes_the_skill_margin() {
    let herb = row(HERB);
    let herbalist = |points: i32| {
        Some(LockMatch {
            spell: 2366,
            points,
            required: 100,
        })
    };
    assert_eq!(
        lines(&herb, false, LockOutcome::OpenBySpell(2366), herbalist(150)),
        [(
            "[KNOWN Herbalism]".to_string(),
            TooltipTint::Skill {
                required: 100,
                value: 150
            }
        )]
    );
    assert_eq!(
        lines(&herb, false, LockOutcome::Unmet, herbalist(75)),
        [(
            "[KNOWN Herbalism]".to_string(),
            TooltipTint::Skill {
                required: 100,
                value: 75
            }
        )],
        "a short skill keeps the known line, red by the ramp"
    );
    assert_eq!(
        lines(&herb, false, LockOutcome::Unmet, None),
        [("[SPELL Herbalism]".to_string(), TooltipTint::Red)]
    );
    // A lock type with no row is the literal `UNKNOWN`.
    let odd = row(LockSlot { index: 77, ..HERB });
    assert_eq!(
        lines(&odd, false, LockOutcome::Unmet, None),
        [("[SPELL UNKNOWN]".to_string(), TooltipTint::Red)]
    );
}

/// A flag-locked object that a skill opens: the "Locked" line in the ramp's colour (`0x52ab2b`-
/// `0x52ab38`) and `LOCKED_WITH_SPELL_KNOWN` beside it; without the spell, a red "Locked" and no
/// skill line (`0x52abf7`). A key slot names its item, white.
#[test]
fn a_flag_locked_objects_lines_follow_the_opener() {
    let chest = row(LockSlot {
        key_type: LOCK_KEY_SKILL,
        index: 2,
        skill: 175,
        action: 1,
    });
    let rogue = Some(LockMatch {
        spell: 1804,
        points: 225,
        required: 175,
    });
    let ramp = TooltipTint::Skill {
        required: 175,
        value: 225,
    };
    assert_eq!(
        lines(&chest, true, LockOutcome::OpenBySpell(1804), rogue),
        [
            ("[LOCKED]".to_string(), ramp),
            ("[KNOWN Herbalism]".to_string(), ramp),
        ]
    );
    assert_eq!(
        lines(&chest, true, LockOutcome::Unmet, None),
        [("[LOCKED]".to_string(), TooltipTint::Red)]
    );
    let gate = row(LockSlot {
        key_type: LOCK_KEY_ITEM,
        index: 5396,
        skill: 0,
        action: 1,
    });
    assert_eq!(
        lines(&gate, true, LockOutcome::OpenByKey(5396), None),
        [
            ("[LOCKED]".to_string(), TooltipTint::LockOpen),
            (
                "[ITEM Key to Searing Gorge]".to_string(),
                TooltipTint::White
            ),
        ]
    );
}

/// A meeting stone's slot 33 (`0x5f6b10`): its area's `AreaTable` name (`+0x2c + 4·locale`,
/// `0x5f6b5c`, added at `0x5f6b68`), then `MEETINGSTONE_LEVEL` (`0x86095c`) of its minimum and
/// maximum levels (`0x5f6b8a`-`0x5f6bb2`, added at `0x5f6bdf`), both through `0x530380` in gold.
/// An area with no row adds no name line (`0x5f6b3f`-`0x5f6b54`); any other type adds nothing.
#[test]
fn a_meeting_stones_tooltip_adds_its_area_and_level_range() {
    let stone = MeetingStoneTemplate {
        min_level: 15,
        max_level: 25,
        area: 1581,
    };
    let area = |id: u32| (id == 1581).then(|| "The Deadmines".to_string());
    let get = |key: &str| (key == "MEETINGSTONE_LEVEL").then(|| "Level %d-%d".to_string());
    assert_eq!(
        read(go_handler_lines(Some(stone), &area), &get),
        [
            ("The Deadmines".to_string(), TooltipTint::Gold),
            ("Level 15-25".to_string(), TooltipTint::Gold),
        ]
    );
    let nowhere = MeetingStoneTemplate {
        area: 9999,
        ..stone
    };
    assert_eq!(
        read(go_handler_lines(Some(nowhere), &area), &get),
        [("Level 15-25".to_string(), TooltipTint::Gold)],
        "no area row, no name line"
    );
    assert!(go_handler_lines(None, &area).is_empty(), "not a stone");
}
