//! `QuestInfo.dbc`, the quest tag vocabulary: seven rows, `ID` at column 0 and `Name` at column 1
//! of a 10-column loc-block layout.

use benilla_formats::{load_quest_tag_names, open_chain};

#[test]
fn quest_tag_names_are_the_whole_5875_vocabulary() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let tags = load_quest_tag_names(&mut chain).expect("load quest tag names");

    // The whole table. The ids are sparse, so lookup is by id, never by row index.
    let want = [
        (1, "Elite"),
        (21, "Life"),
        (41, "PvP"),
        (62, "Raid"),
        (81, "Dungeon"),
        (82, "World Event"),
        (83, "Legendary"),
    ];
    // The chain resolves this path to `patch-2.MPQ`; the base `dbc.MPQ` copy has only four rows.
    assert_eq!(tags.len(), want.len(), "QuestInfo.dbc row count");
    for (id, name) in want {
        assert_eq!(tags.resolve(id), Some(name), "QuestInfo id {id}");
    }

    // 1.12 says "Elite" where later expansions say "Group".
    assert_eq!(tags.resolve(1), Some("Elite"));

    // Type 0 is the untagged majority.
    assert_eq!(tags.resolve(0), None);
    assert_eq!(tags.resolve(84), None);
}
