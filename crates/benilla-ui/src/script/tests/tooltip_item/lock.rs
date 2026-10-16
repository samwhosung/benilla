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
    let mut s = item_script();
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
            lock_id: LOCK_ID,
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
    s.set_lock_rows(HashMap::from([(LOCK_ID, slots)]));
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
