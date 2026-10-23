//! The roll lines' templates: each key the reference reads, filled with the pieces it pushes.

use benilla_protocol::messages::{roll_vote, LootAllPassed, LootRoll, LootRollWon};

use super::{format_line, RollItem, RollLine};

/// The pieces of [`LINK`]: the roll's `+0x24` word is the link's fourth number (`0x61c16e`).
fn item() -> RollItem {
    RollItem {
        quality: 4,
        item_id: 17182,
        random_property_id: 862,
        suffix_factor: 1234,
        name: "Sulfuras of the Bear".into(),
    }
}

const LINK: &str = "|cffa335ee|Hitem:17182:0:862:1234|h[Sulfuras of the Bear]|h|r";

/// A string table whose every template names its key and lays out the holes in the order the
/// reference pushes them: the roller's name first for a third-person line (`0x61c266`), the
/// sign-extended number first for a dice line (`0x61c33d`) and the name last, and the positional
/// `_NO_SPAM_` forms.
fn strings(key: &str) -> Option<String> {
    let link = "%s|Hitem:%d:%d:%d:%d|h[%s]|h%s";
    Some(match key {
        "LOOT_ROLL_NEED" | "LOOT_ROLL_GREED" | "LOOT_ROLL_PASSED" | "LOOT_ROLL_WON" => {
            format!("{key} <%s> {link}")
        }
        "LOOT_ROLL_NEED_SELF"
        | "LOOT_ROLL_GREED_SELF"
        | "LOOT_ROLL_PASSED_SELF"
        | "LOOT_ROLL_YOU_WON"
        | "LOOT_ROLL_ALL_PASSED" => format!("{key} {link}"),
        "LOOT_ROLL_ROLLED_NEED" | "LOOT_ROLL_ROLLED_GREED" => format!("{key} %d {link} <%s>"),
        "LOOT_ROLL_WON_NO_SPAM_NEED" | "LOOT_ROLL_WON_NO_SPAM_GREED" => {
            format!("{key} <%1$s> %3$s|Hitem:%4$d:%5$d:%6$d:%7$d|h[%8$s]|h%9$s (%2$d)")
        }
        "LOOT_ROLL_YOU_WON_NO_SPAM_NEED" | "LOOT_ROLL_YOU_WON_NO_SPAM_GREED" => {
            format!("{key} %2$s|Hitem:%3$d:%4$d:%5$d:%6$d|h[%7$s]|h%8$s (%1$d)")
        }
        _ => return None,
    })
}

fn line(l: RollLine, name: Option<&str>, is_self: bool, detailed: bool) -> Option<String> {
    format_line(&l, name, is_self, &item(), detailed, &strings)
}

fn announce(roll_number: u8, roll_type: u8) -> RollLine {
    RollLine::Announce(LootRoll {
        looted_target: 0xAA,
        item_slot: 0,
        roller: 1,
        item_id: 17182,
        random_property_id: 862,
        suffix_factor: 0,
        roll_number,
        roll_type,
    })
}

fn won(roll_number: u8, roll_type: u8) -> RollLine {
    RollLine::Won(LootRollWon {
        looted_target: 0xAA,
        item_slot: 0,
        item_id: 17182,
        random_property_id: 862,
        suffix_factor: 0,
        winner: 1,
        roll_number,
        roll_type,
    })
}

/// `0x61c0b0` reads its key through `FrameScript_GetText` (`0x61c18f` and its siblings), choosing
/// by the roll number as a signed byte and the type: a negative number with type 2 is a Greed vote,
/// any other negative one a pass, a non-negative one with type 0 a Need vote, and anything else a
/// dice result, `ROLLED_GREED` for type 2 and `ROLLED_NEED` otherwise (`0x61c320`).
#[test]
fn a_roll_announcement_reads_its_key_and_fills_the_pieces() {
    let bob = Some("Bob");
    let cases: [(RollLine, bool, String); 10] = [
        (
            announce(0, 0),
            false,
            format!("LOOT_ROLL_NEED <Bob> {LINK}"),
        ),
        (announce(0, 0), true, format!("LOOT_ROLL_NEED_SELF {LINK}")),
        (
            announce(128, roll_vote::GREED),
            false,
            format!("LOOT_ROLL_GREED <Bob> {LINK}"),
        ),
        (
            announce(128, roll_vote::GREED),
            true,
            format!("LOOT_ROLL_GREED_SELF {LINK}"),
        ),
        (
            announce(128, 128),
            false,
            format!("LOOT_ROLL_PASSED <Bob> {LINK}"),
        ),
        (
            announce(200, 0),
            true,
            format!("LOOT_ROLL_PASSED_SELF {LINK}"),
        ),
        (
            announce(57, roll_vote::NEED),
            false,
            format!("LOOT_ROLL_ROLLED_NEED 57 {LINK} <Bob>"),
        ),
        (
            announce(57, roll_vote::GREED),
            true,
            format!("LOOT_ROLL_ROLLED_GREED 57 {LINK} <Bob>"),
        ),
        (
            announce(0, 1),
            false,
            format!("LOOT_ROLL_ROLLED_NEED 0 {LINK} <Bob>"),
        ),
        (
            announce(101, 0),
            false,
            format!("LOOT_ROLL_NEED <Bob> {LINK}"),
        ),
    ];
    for (l, is_self, expect) in cases {
        assert_eq!(line(l, bob, is_self, true), Some(expect));
    }
}

/// `0x61b9e0`: with `showLootSpam` on, `LOOT_ROLL_YOU_WON` or `LOOT_ROLL_WON`; off, the
/// `_NO_SPAM_NEED` form for type 1 (`0x61ba94`, `0x61bb52`), else `_NO_SPAM_GREED`, which take the
/// sign-extended number second for another's line and first for yours (`0x61baa2`, `0x61bb6a`).
/// `0x61b640` writes `LOOT_ROLL_ALL_PASSED` with the pieces alone.
#[test]
fn a_result_line_reads_its_key_and_fills_the_pieces() {
    let bob = Some("Bob");
    assert_eq!(
        line(won(84, 1), bob, false, true),
        Some(format!("LOOT_ROLL_WON <Bob> {LINK}"))
    );
    assert_eq!(
        line(won(84, 1), None, true, true),
        Some(format!("LOOT_ROLL_YOU_WON {LINK}"))
    );
    assert_eq!(
        line(won(84, 1), bob, false, false),
        Some(format!("LOOT_ROLL_WON_NO_SPAM_NEED <Bob> {LINK} (84)"))
    );
    assert_eq!(
        line(won(84, 2), None, true, false),
        Some(format!("LOOT_ROLL_YOU_WON_NO_SPAM_GREED {LINK} (84)"))
    );
    let passed = RollLine::AllPassed(LootAllPassed {
        looted_target: 0xAA,
        item_slot: 0,
        item_id: 17182,
        random_property_id: 862,
        suffix_factor: 0,
    });
    assert_eq!(
        line(passed, None, false, false),
        Some(format!("LOOT_ROLL_ALL_PASSED {LINK}"))
    );
}

/// A key the strings lack prints no line, as `FrameScript_GetText` gives the empty string
/// (`0x882748`) for one.
#[test]
fn a_missing_template_prints_nothing() {
    assert_eq!(
        format_line(&announce(0, 0), Some("Bob"), false, &item(), true, &|_| {
            None
        }),
        None
    );
}
