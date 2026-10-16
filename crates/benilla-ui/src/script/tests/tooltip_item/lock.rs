//! The colour of an item tooltip's `LOCKED` line (`0x52bdde`..`0x52be3d`): red unless the lock
//! check `0x5da5e0` finds an opener, the skill-margin ramp `0x529fa0` for a known open-lock spell,
//! the green `0xc0d420` for a held key or a lock with nothing to open.

use std::collections::{HashMap, HashSet};

use super::{lines_of, script as item_script};
use crate::script::*;

const RED: [f32; 4] = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];
const ORANGE: [f32; 4] = [1.0, 128.0 / 255.0, 64.0 / 255.0, 1.0];
const YELLOW: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
/// `0xff40c040`: the ramp's green, not the tooltip's `0xff00ff00`.
const LOCK_GREEN: [f32; 4] = [64.0 / 255.0, 192.0 / 255.0, 64.0 / 255.0, 1.0];
const GRAY: [f32; 4] = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];

const LOCKBOX: u32 = 4632;
const LOCK_ID: u32 = 7;
const KEY: u32 = 5060;

fn skill_slot(lock_type: u32, skill: u32) -> LockSlotView {
    LockSlotView {
        key_type: 2,
        index: lock_type,
        skill,
    }
}

fn key_slot(entry: u32) -> LockSlotView {
    LockSlotView {
        key_type: 1,
        index: entry,
        skill: 0,
    }
}

/// A locked box (item level 20) in the backpack, whose lock is `slots`, with the player's means
/// of opening it `openers`.
fn fixture(slots: Vec<LockSlotView>, openers: Option<LockOpeners>) -> UiScript {
    fixture_rows(LOCK_ID, vec![(LOCK_ID, slots)], openers)
}

/// [`fixture`] with the box's template naming lock `template_lock` and the store holding `rows`.
fn fixture_rows(
    template_lock: u32,
    rows: Vec<(u32, Vec<LockSlotView>)>,
    openers: Option<LockOpeners>,
) -> UiScript {
    let mut s = item_script();
    s.run(
        r#"LOCKED_WITH_ITEM = "[LOCKED_WITH_ITEM %s]"
           LOCKED_WITH_SPELL_KNOWN = "[LOCKED_WITH_SPELL_KNOWN %s]""#,
    )
    .unwrap();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0)
        a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
    "#,
    )
    .unwrap();
    s.set_item_template(
        LOCKBOX,
        ItemTemplateView {
            name: "Lockbox".into(),
            quality: 1,
            item_level: 20,
            lock_id: template_lock,
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: HashMap::from([(
                1,
                ContainerSlot {
                    item_id: LOCKBOX,
                    count: 1,
                    quality: Some(1),
                    link: Some(format!("|cffffffff|Hitem:{LOCKBOX}:0:0:0|h[Lockbox]|h|r")),
                    ..Default::default()
                },
            )]),
        }),
    );
    s.set_lock_rows(rows.into_iter().collect());
    s.set_lock_type_names(HashMap::from([(1, "Pick Lock".to_string())]));
    if let Some(o) = openers {
        s.set_lock_openers(o);
    }
    s
}

/// The `LOCKED` line's colour on a bag hover.
fn locked_color(s: &mut UiScript) -> [f32; 4] {
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 1)")
        .unwrap();
    let lines = lines_of(s);
    let (_, color) = lines
        .iter()
        .find(|(t, _)| t == "[LOCKED]")
        .unwrap_or_else(|| panic!("no LOCKED line in {lines:?}"));
    *color
}

fn picker(points: i32) -> LockOpeners {
    LockOpeners {
        spells: vec![LockOpener {
            lock_type: 1,
            points,
        }],
        held: HashSet::new(),
    }
}

/// A lock the player cannot open is red, whichever way it fails: no opener, too little skill, the
/// key not carried, or a lockpicking spell for another lock type.
#[test]
fn a_lock_the_player_cannot_open_is_red() {
    let mut s = fixture(vec![skill_slot(1, 25)], Some(LockOpeners::default()));
    assert_eq!(locked_color(&mut s), RED, "no opener");
    let mut s = fixture(vec![skill_slot(1, 25)], Some(picker(24)));
    assert_eq!(locked_color(&mut s), RED, "a point short");
    let mut s = fixture(vec![key_slot(KEY)], Some(picker(300)));
    assert_eq!(locked_color(&mut s), RED, "a key lock, no key");
    let mut s = fixture(vec![skill_slot(2, 1)], Some(picker(300)));
    assert_eq!(locked_color(&mut s), RED, "another lock type");
}

/// A known open-lock spell whose skill covers the lock colours it by the margin: orange from the
/// requirement, yellow 25 over, green 50 over, gray 100 over (`0x529fa0`).
#[test]
fn a_lock_a_known_spell_opens_takes_the_margin_ramp() {
    for (points, want, name) in [
        (25, ORANGE, "at the requirement"),
        (49, ORANGE, "24 over"),
        (50, YELLOW, "25 over"),
        (74, YELLOW, "49 over"),
        (75, LOCK_GREEN, "50 over"),
        (124, LOCK_GREEN, "99 over"),
        (125, GRAY, "100 over"),
    ] {
        let mut s = fixture(vec![skill_slot(1, 25)], Some(picker(points)));
        assert_eq!(locked_color(&mut s), want, "{name}");
    }
}

/// A slot whose skill is 0 asks five times the item's level (`0x5da6ae`): a level 20 box, 100.
#[test]
fn a_zero_skill_slot_asks_five_times_the_item_level() {
    let mut s = fixture(vec![skill_slot(1, 0)], Some(picker(99)));
    assert_eq!(locked_color(&mut s), RED);
    let mut s = fixture(vec![skill_slot(1, 0)], Some(picker(100)));
    assert_eq!(locked_color(&mut s), ORANGE);
}

/// A held key, and a lock with no key or skill slot, are green; so is a lock read with no active
/// player (the walk returns 0 before it reads a slot, `0x5da60b`).
#[test]
fn a_held_key_or_a_lock_with_nothing_to_open_is_green() {
    let mut carrying = picker(0);
    carrying.held.insert(KEY);
    let mut s = fixture(vec![key_slot(KEY)], Some(carrying));
    assert_eq!(locked_color(&mut s), LOCK_GREEN, "the key is carried");
    let mut s = fixture(Vec::new(), Some(LockOpeners::default()));
    assert_eq!(locked_color(&mut s), LOCK_GREEN, "no key or skill entries");
    let mut s = fixture(vec![skill_slot(1, 300)], None);
    assert_eq!(locked_color(&mut s), LOCK_GREEN, "no active player");
}

/// A skill that falls short does not close a lock a held key opens, and a lock a spell opens takes
/// the ramp even when a key slot precedes it.
#[test]
fn the_first_slot_to_open_the_lock_decides_the_colour() {
    let mut weak = picker(10);
    weak.held.insert(KEY);
    let mut s = fixture(vec![skill_slot(1, 25), key_slot(KEY)], Some(weak));
    assert_eq!(
        locked_color(&mut s),
        LOCK_GREEN,
        "the key after a failed skill"
    );
    let mut strong = picker(25);
    strong.held.insert(KEY);
    let mut s = fixture(vec![skill_slot(1, 25), key_slot(KEY)], Some(strong));
    assert_eq!(locked_color(&mut s), ORANGE, "the skill is first");
}

/// The tooltip's lines under a bag hover.
fn bag_lines(s: &mut UiScript) -> Vec<(String, [f32; 4])> {
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 1)")
        .unwrap();
    lines_of(s)
}

fn line<'a>(lines: &'a [(String, [f32; 4])], text: &str) -> Option<&'a (String, [f32; 4])> {
    lines.iter().find(|(t, _)| t == text)
}

fn key_template(s: &mut UiScript) {
    s.set_item_template(
        KEY,
        ItemTemplateView {
            name: "Skeleton Key".into(),
            quality: 1,
            ..Default::default()
        },
    );
}

/// A key slot (`0x52bf30`) names the key from the item cache in white (`0x52bf93`), under LOCKED,
/// whether or not the player holds it.
#[test]
fn a_key_lock_names_its_key_in_white_under_locked() {
    for held in [false, true] {
        let mut openers = LockOpeners::default();
        if held {
            openers.held.insert(KEY);
        }
        let mut s = fixture(vec![key_slot(KEY)], Some(openers));
        key_template(&mut s);
        let lines = bag_lines(&mut s);
        let at = |t: &str| lines.iter().position(|(l, _)| l == t);
        assert_eq!(
            at("[LOCKED_WITH_ITEM Skeleton Key]"),
            at("[LOCKED]").map(|i| i + 1),
            "{lines:?}"
        );
        assert_eq!(
            line(&lines, "[LOCKED_WITH_ITEM Skeleton Key]").unwrap().1,
            [1.0; 4]
        );
    }
}

/// A key the item cache lacks adds nothing (`0x52bf67`) and is asked for, so the hover's repaint
/// draws it on arrival.
#[test]
fn a_key_the_cache_lacks_adds_no_line_and_is_asked_for() {
    let mut s = fixture(vec![key_slot(KEY)], Some(LockOpeners::default()));
    let lines = bag_lines(&mut s);
    assert!(line(&lines, "[LOCKED]").is_some());
    assert!(
        lines.iter().all(|(t, _)| !t.starts_with("[LOCKED_WITH")),
        "{lines:?}"
    );
    assert!(s.take_item_stat_asks().contains(&KEY));
}

/// A skill slot names the lock type, in the ramp colour of the spell the walk tried last
/// (`0x52beae`), the same colour `LOCKED` takes when the spell reaches the requirement.
#[test]
fn a_skill_lock_names_its_lock_type_in_the_ramp_colour() {
    for (points, want) in [(25, ORANGE), (50, YELLOW), (75, LOCK_GREEN), (125, GRAY)] {
        let mut s = fixture(vec![skill_slot(1, 25)], Some(picker(points)));
        let lines = bag_lines(&mut s);
        let sub = line(&lines, "[LOCKED_WITH_SPELL_KNOWN Pick Lock]")
            .unwrap_or_else(|| panic!("no sub-line at {points}: {lines:?}"));
        assert_eq!(sub.1, want, "{points} points");
        assert_eq!(line(&lines, "[LOCKED]").unwrap().1, want);
    }
}

/// The line shows for any known opener of the lock type, even one whose skill falls short
/// (`0x5da6cc` records it before the compare): red, under a red `LOCKED`.
#[test]
fn a_known_opener_below_the_skill_still_names_the_lock_type_in_red() {
    let mut s = fixture(vec![skill_slot(1, 25)], Some(picker(24)));
    let lines = bag_lines(&mut s);
    assert_eq!(line(&lines, "[LOCKED]").unwrap().1, RED);
    assert_eq!(
        line(&lines, "[LOCKED_WITH_SPELL_KNOWN Pick Lock]")
            .unwrap()
            .1,
        RED
    );
}

/// No known spell of the lock type, or no active player (the walk returns before it reads a
/// slot, `0x5da60b`), adds nothing (`0x52bea2`).
#[test]
fn a_skill_lock_with_no_known_opener_adds_no_line() {
    for openers in [
        Some(LockOpeners::default()),
        // A herbalism spell against a lockpicking lock.
        Some(LockOpeners {
            spells: vec![LockOpener {
                lock_type: 2,
                points: 300,
            }],
            held: HashSet::new(),
        }),
        None,
    ] {
        let mut s = fixture(vec![skill_slot(1, 25)], openers.clone());
        let lines = bag_lines(&mut s);
        assert!(line(&lines, "[LOCKED]").is_some());
        assert!(
            lines.iter().all(|(t, _)| !t.starts_with("[LOCKED_WITH")),
            "{openers:?}: {lines:?}"
        );
    }
}

/// A lock type the store lacks reads `UNKNOWN` (`0x52bf00`); the line is the first slot's alone,
/// so a lock whose first slot is empty adds none (`0x52be5d`).
#[test]
fn the_sub_line_reads_the_first_slot_and_names_a_missing_type_unknown() {
    let mut s = fixture(
        vec![skill_slot(9, 25)],
        Some(LockOpeners {
            spells: vec![LockOpener {
                lock_type: 9,
                points: 25,
            }],
            held: HashSet::new(),
        }),
    );
    let lines = bag_lines(&mut s);
    assert!(
        line(&lines, "[LOCKED_WITH_SPELL_KNOWN UNKNOWN]").is_some(),
        "{lines:?}"
    );
    let mut s = fixture(
        vec![LockSlotView::default(), skill_slot(1, 25)],
        Some(picker(300)),
    );
    let lines = bag_lines(&mut s);
    assert!(line(&lines, "[LOCKED]").is_some());
    assert!(
        lines.iter().all(|(t, _)| !t.starts_with("[LOCKED_WITH")),
        "{lines:?}"
    );
}

/// A record whose lock id has no row falls back to the block's row (`0x52bd76`): the trade
/// target's lock names the row, and `LOCKED` and its sub-line follow that row. With no row in
/// either, there is no lock to draw (`0x52bda9`).
#[test]
fn a_missing_record_row_falls_back_to_the_blocks_row() {
    const MISSING: u32 = 99;
    let mut s = fixture_rows(
        MISSING,
        vec![(LOCK_ID, vec![skill_slot(1, 25)])],
        Some(picker(25)),
    );
    let trade = |lock_id: u32| {
        let mut st = TradeState::default();
        st.target.slots[0] = Some(TradeSlotItem {
            item_id: LOCKBOX,
            name: Some("Lockbox".into()),
            count: 1,
            lock_id,
            ..Default::default()
        });
        st
    };
    s.set_trade(Some(trade(LOCK_ID)));
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetTradeTargetItem(1)")
        .unwrap();
    let lines = lines_of(&mut s);
    assert_eq!(line(&lines, "[LOCKED]").unwrap().1, ORANGE, "{lines:?}");
    assert!(line(&lines, "[LOCKED_WITH_SPELL_KNOWN Pick Lock]").is_some());

    // Neither row exists: no LOCKED, no sub-line.
    s.set_trade(Some(trade(MISSING)));
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetTradeTargetItem(1)")
        .unwrap();
    let lines = lines_of(&mut s);
    assert!(line(&lines, "[LOCKED]").is_none(), "{lines:?}");

    // A bag item (no block) whose record names a missing row is not locked either.
    let lines = bag_lines(&mut s);
    assert!(line(&lines, "[LOCKED]").is_none(), "{lines:?}");
}

/// A lock type whose name is blank is a row, not a missing one: only a missing row reads `UNKNOWN`
/// (`0x52bede`..`0x52bef2`), a blank name fills the line as it is (`0x52bef4`).
#[test]
fn a_blank_lock_type_name_fills_the_line_and_is_not_unknown() {
    let mut s = fixture(vec![skill_slot(5, 25)], Some(picker_for(5, 25)));
    s.set_lock_type_names(HashMap::from([(5, String::new())]));
    let lines = bag_lines(&mut s);
    assert!(
        line(&lines, "[LOCKED_WITH_SPELL_KNOWN ]").is_some(),
        "{lines:?}"
    );
    assert!(line(&lines, "[LOCKED_WITH_SPELL_KNOWN UNKNOWN]").is_none());
}

fn picker_for(lock_type: u32, points: i32) -> LockOpeners {
    LockOpeners {
        spells: vec![LockOpener { lock_type, points }],
        held: HashSet::new(),
    }
}

/// OPENABLE reads the record's raw lock id: a nonzero one needs the object UNLOCKED (`0x52e2fc`),
/// with no row check, so a lock id without a Lock row (no LOCKED line to draw) still withholds it.
#[test]
fn openable_waits_for_unlocked_on_a_lock_id_with_no_row() {
    const MISSING: u32 = 99;
    let mut s = fixture_rows(MISSING, Vec::new(), None);
    s.set_item_template(
        LOCKBOX,
        ItemTemplateView {
            name: "Lockbox".into(),
            quality: 1,
            flags: 0x4,
            lock_id: MISSING,
            ..Default::default()
        },
    );
    let slot = |flags| ContainerSlot {
        item_id: LOCKBOX,
        count: 1,
        quality: Some(1),
        flags,
        ..Default::default()
    };
    let hover = |s: &mut UiScript, flags| {
        s.set_container(
            0,
            Some(ContainerState {
                name: Some("Backpack".into()),
                num_slots: 16,
                slots: HashMap::from([(1, slot(flags))]),
            }),
        );
        bag_lines(s)
    };
    let lines = hover(&mut s, 0);
    assert!(
        line(&lines, "[LOCKED]").is_none(),
        "no row, no lock: {lines:?}"
    );
    assert!(
        line(&lines, "[ITEM_OPENABLE]").is_none(),
        "locked by id, not unlocked: {lines:?}"
    );
    let lines = hover(&mut s, 0x4);
    assert!(
        line(&lines, "[ITEM_OPENABLE]").is_some(),
        "UNLOCKED: {lines:?}"
    );
}
