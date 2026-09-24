//! Asset-gated fixture: `QuestInfo.dbc` against the real 5875 data — pins the whole 7-row tag
//! vocabulary and the `ID(0)/Name(1)` columns of its 10-column loc-block layout, so a schema drift
//! or column slip fails loudly instead of quietly blanking every quest-log tag. Skips (passes)
//! without `<repo>/WoW/Data`.

use benilla_formats::{load_quest_tag_names, open_chain};

#[test]
fn quest_tag_names_are_the_whole_5875_vocabulary() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let tags = load_quest_tag_names(&mut chain).expect("load quest tag names");

    // The complete table — 5875 ships exactly these seven, and the ids are sparse (so the lookup
    // must be by id, never by row index).
    let want = [
        (1, "Elite"),
        (21, "Life"),
        (41, "PvP"),
        (62, "Raid"),
        (81, "Dungeon"),
        (82, "World Event"),
        (83, "Legendary"),
    ];
    // The row count is the load-bearing assert, not a tidiness one. The client resolves this
    // path to **patch-2.MPQ**; the base `dbc.MPQ` copy has only FOUR rows (PvP, Life, Elite,
    // Raid, unordered, maxId 62), so a chain that stopped short of the patch would answer `None`
    // for exactly Dungeon / World Event / Legendary and look perfectly correct on everything
    // else.
    assert_eq!(tags.len(), want.len(), "QuestInfo.dbc row count");
    for (id, name) in want {
        assert_eq!(tags.resolve(id), Some(name), "QuestInfo id {id}");
    }

    // 1.12 says "Elite" where later expansions say "Group" — the string a group quest's row shows.
    assert_eq!(tags.resolve(1), Some("Elite"));

    // Type 0 is the untagged majority, and it must resolve to nothing rather than a row.
    assert_eq!(tags.resolve(0), None);
    // An id past the table (a later expansion's Escort/Heroic/Dungeon-tier ids) names nothing.
    assert_eq!(tags.resolve(84), None);
}
