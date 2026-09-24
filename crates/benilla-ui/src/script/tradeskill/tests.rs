use super::*;
use crate::script::UiScript;

/// A single-reagent, single-tool recipe fixture.
fn recipe(
    spell_id: u32,
    name: &str,
    difficulty: TradeSkillDifficulty,
    num_available: u32,
    group: Option<(u32, u32, &str)>,
) -> TradeSkillRecipe {
    TradeSkillRecipe {
        group: group.map(|(class, subclass, name)| (class, subclass, name.to_string())),
        spell_id,
        name: name.into(),
        difficulty,
        num_available,
        icon: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
        min_made: 1,
        max_made: 1,
        cooldown_secs: None,
        product_item: spell_id + 10_000,
        product_inv_type: 20, // Robe: folds to the Chest slot bit (4)
        // 0 everywhere, so ties fall through to the name; the ItemLevel test sets it.
        product_item_level: 0,
        reagents: vec![TradeSkillReagent {
            item: 2589,
            name: Some("Linen Cloth".into()),
            icon: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
            need: 2,
            have: 10,
        }],
        tools: vec![("Anvil".into(), true)],
    }
}

/// One "Cloth" group: row 1 the header, then Simple Robe (tier 0) and Bolt of Linen Cloth (tier 3).
fn two_recipe_state() -> TradeSkillState {
    TradeSkillState {
        line: 197,
        line_name: "Tailoring".into(),
        rank: 57,
        max_rank: 75,
        recipes: vec![
            recipe(
                2963,
                "Bolt of Linen Cloth",
                TradeSkillDifficulty::Trivial,
                5,
                Some((2, 1, "Cloth")),
            ),
            recipe(
                3919,
                "Simple Robe",
                TradeSkillDifficulty::Optimal,
                0,
                Some((2, 1, "Cloth")),
            ),
        ],
        repeat_count: 3,
    }
}

/// Four groups: "Bolts" (class 1, a name tie within a tier); "Armor Kit" and "Zephyr Cloak"
/// (class 2, ordered by name against their subclass ids 5 and 1); a recipe with no group yet.
fn state() -> TradeSkillState {
    TradeSkillState {
        line: 197,
        line_name: "Tailoring".into(),
        rank: 57,
        max_rank: 75,
        recipes: vec![
            recipe(
                1,
                "Beta Bolt",
                TradeSkillDifficulty::Optimal,
                5,
                Some((1, 2, "Bolts")),
            ),
            recipe(
                2,
                "Alpha Bolt",
                TradeSkillDifficulty::Optimal,
                5,
                Some((1, 2, "Bolts")),
            ),
            recipe(
                3,
                "Zinc Chain",
                TradeSkillDifficulty::Trivial,
                5,
                Some((2, 5, "Armor Kit")),
            ),
            recipe(
                4,
                "Alpha Plate",
                TradeSkillDifficulty::Optimal,
                0,
                Some((2, 5, "Armor Kit")),
            ),
            recipe(
                5,
                "Wind Cloak",
                TradeSkillDifficulty::Medium,
                3,
                Some((2, 1, "Zephyr Cloak")),
            ),
            recipe(6, "Mystery Item", TradeSkillDifficulty::Easy, 1, None),
        ],
        repeat_count: 3,
    }
}

/// Read `(name, type)` at a visible index.
fn row_kind(s: &mut UiScript, i: i64) -> (String, String) {
    s.eval::<(String, String)>(&format!("local n,t = GetTradeSkillInfo({i}) return n,t"))
        .unwrap()
}

#[test]
fn grouped_visible_rows_ordered_by_class_then_name_tie_then_tier_then_name() {
    let mut s = UiScript::new().unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 0);

    s.set_trade_skill(Some(state()));

    // 4 headers + 6 recipes = 10 visible rows.
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 10);

    let expect = [
        ("Bolts", "header"),
        ("Alpha Bolt", "optimal"), // tier tie -> name
        ("Beta Bolt", "optimal"),
        ("Armor Kit", "header"), // class tie -> name, not subclass id (5 > 1)
        ("Alpha Plate", "optimal"), // tier 0 before...
        ("Zinc Chain", "trivial"), // ...tier 3
        ("Zephyr Cloak", "header"),
        ("Wind Cloak", "medium"),
        ("", "header"), // the pending bucket sorts last, empty name
        ("Mystery Item", "easy"),
    ];
    for (i, (name, kind)) in expect.iter().enumerate() {
        assert_eq!(
            row_kind(&mut s, (i + 1) as i64),
            (name.to_string(), kind.to_string()),
            "row {}",
            i + 1
        );
    }
}

#[test]
fn collapse_hides_a_groups_entries_and_remaps_indices_incl_collapse_all() {
    let mut s = UiScript::new().unwrap();
    s.set_trade_skill(Some(state()));

    // "Armor Kit" is header 4: its two recipes go, 10 -> 8, and it reports isExpanded nil.
    s.run("CollapseTradeSkillSubClass(4)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 8);
    let (name, _, _, expanded) = s
        .eval::<(String, String, i64, Option<i64>)>(
            "local n,t,a,e = GetTradeSkillInfo(4) return n,t,a,e",
        )
        .unwrap();
    assert_eq!((name.as_str(), expanded), ("Armor Kit", None));
    // "Zephyr Cloak" moves up from row 7 to row 5.
    assert_eq!(
        row_kind(&mut s, 5),
        ("Zephyr Cloak".into(), "header".into())
    );

    s.run("ExpandTradeSkillSubClass(4)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 10);

    // Id 0 folds every group: the 4 headers remain.
    s.run("CollapseTradeSkillSubClass(0)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 4);
    s.run("ExpandTradeSkillSubClass(0)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 10);
}

#[test]
fn pending_recipes_bucket_trailing_under_an_empty_header() {
    let mut s = UiScript::new().unwrap();
    s.set_trade_skill(Some(state()));

    // Row 9 is the pending group's header.
    let (name, kind, avail, expanded) = s
        .eval::<(String, String, i64, Option<i64>)>(
            "local n,t,a,e = GetTradeSkillInfo(9) return n,t,a,e",
        )
        .unwrap();
    assert_eq!(
        (name.as_str(), kind.as_str(), avail, expanded),
        ("", "header", 0, Some(1))
    );
    assert_eq!(row_kind(&mut s, 10), ("Mystery Item".into(), "easy".into()));

    s.run("CollapseTradeSkillSubClass(9)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 9);
}

#[test]
fn header_and_entry_tuple_shapes() {
    let mut s = UiScript::new().unwrap();
    s.set_trade_skill(Some(state()));

    let (name, kind, avail, expanded) = s
        .eval::<(String, String, i64, Option<i64>)>(
            "local n,t,a,e = GetTradeSkillInfo(1) return n,t,a,e",
        )
        .unwrap();
    assert_eq!(
        (name.as_str(), kind.as_str(), avail, expanded),
        ("Bolts", "header", 0, Some(1))
    );

    let (name, kind, avail, expanded) = s
        .eval::<(String, String, i64, Option<i64>)>(
            "local n,t,a,e = GetTradeSkillInfo(2) return n,t,a,e",
        )
        .unwrap();
    assert_eq!(
        (name.as_str(), kind.as_str(), avail, expanded),
        ("Alpha Bolt", "optimal", 5, None)
    );
}

#[test]
fn do_trade_skill_and_getters_no_op_on_a_header_index() {
    let mut s = UiScript::new().unwrap();
    s.set_trade_skill(Some(state()));

    // Row 1 is a header: every per-recipe getter reads nil, zero or empty.
    assert!(s
        .eval::<bool>("return GetTradeSkillIcon(1) == nil")
        .unwrap());
    assert_eq!(
        s.eval::<(i64, i64)>("return GetTradeSkillNumMade(1)")
            .unwrap(),
        (0, 0)
    );
    assert!(s
        .eval::<bool>("return GetTradeSkillCooldown(1) == nil")
        .unwrap());
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillNumReagents(1)").unwrap(),
        0
    );
    assert!(s
        .eval::<bool>("return GetTradeSkillReagentInfo(1, 1) == nil")
        .unwrap());
    assert_eq!(s.arity("GetTradeSkillTools(1)").unwrap(), 0);

    s.run("DoTradeSkill(1, 5)").unwrap();
    assert!(
        s.take_trade_skill_dos().is_empty(),
        "a header index queues no craft"
    );

    // A header index leaves the selection as it was.
    s.run("SelectTradeSkill(2)").unwrap(); // "Alpha Bolt", row 2
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        2
    );
    s.run("SelectTradeSkill(1)").unwrap(); // row 1 is a header
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        2,
        "a header index never clears or changes the selection"
    );
}

#[test]
fn get_trade_skill_sub_classes_returns_group_names_in_order() {
    let mut s = UiScript::new().unwrap();
    assert_eq!(
        s.arity("GetTradeSkillSubClasses()").unwrap(),
        0,
        "no window open, no groups"
    );

    s.set_trade_skill(Some(state()));
    assert_eq!(
        s.eval::<(String, String, String, String)>("return GetTradeSkillSubClasses()")
            .unwrap(),
        (
            "Bolts".into(),
            "Armor Kit".into(),
            "Zephyr Cloak".into(),
            "".into()
        )
    );
}

#[test]
fn selection_persists_across_collapse_and_a_regroup() {
    let mut s = UiScript::new().unwrap();
    s.set_trade_skill(Some(state()));

    // Select "Wind Cloak" ("Zephyr Cloak"'s only recipe, row 8).
    s.run("SelectTradeSkill(8)").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        8
    );

    // Folding "Bolts" above it moves Wind Cloak up two rows, and the selection follows.
    s.run("CollapseTradeSkillSubClass(1)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 8);
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        6
    );
    s.run("ExpandTradeSkillSubClass(1)").unwrap();

    // Folding its own group (row 7) hides it: the index reads 0, but the selection stays...
    s.run("CollapseTradeSkillSubClass(7)").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        0
    );
    // ...and comes back at row 8.
    s.run("ExpandTradeSkillSubClass(7)").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        8
    );

    // A re-push (a reagent-count tick) keeps the same recipe selected, tracked by spell id.
    let mut ticked = state();
    ticked.recipes[4].num_available = 9; // Wind Cloak, spell 5
    s.set_trade_skill(Some(ticked));
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        8
    );

    // A re-push without the recipe clears the selection.
    let mut without_wind_cloak = state();
    without_wind_cloak.recipes.remove(4);
    s.set_trade_skill(Some(without_wind_cloak));
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        0
    );
}

#[test]
fn snapshot_feeds_the_api_tuples_through_the_visible_mapping() {
    let mut s = UiScript::new().unwrap();
    s.set_trade_skill(Some(two_recipe_state()));

    assert_eq!(
        s.eval::<(String, i64, i64)>("return GetTradeSkillLine()")
            .unwrap(),
        ("Tailoring".into(), 57, 75)
    );
    // 1 header + 2 recipes; the first recipe row is 2 (Simple Robe, tier 0).
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 3);
    assert_eq!(s.eval::<i64>("return GetFirstTradeSkill()").unwrap(), 2);

    let (name, kind, avail, expanded) = s
        .eval::<(String, String, i64, Option<i64>)>(
            "local n,t,a,e = GetTradeSkillInfo(3) return n,t,a,e",
        )
        .unwrap();
    assert_eq!(
        (name.as_str(), kind.as_str(), avail, expanded),
        ("Bolt of Linen Cloth", "trivial", 5, None)
    );

    assert_eq!(
        s.eval::<String>("return GetTradeSkillIcon(3)").unwrap(),
        "Interface\\Icons\\Spell_2963"
    );
    assert_eq!(
        s.eval::<(i64, i64)>("return GetTradeSkillNumMade(3)")
            .unwrap(),
        (1, 1)
    );
    assert!(s
        .eval::<bool>("return GetTradeSkillCooldown(3) == nil")
        .unwrap());
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillNumReagents(3)").unwrap(),
        1
    );
    let (rname, ricon, need, have) = s
        .eval::<(String, String, i64, i64)>("return GetTradeSkillReagentInfo(3, 1)")
        .unwrap();
    assert_eq!(
        (rname.as_str(), ricon.as_str(), need, have),
        (
            "Linen Cloth",
            "Interface\\Icons\\INV_Fabric_Linen_01",
            2,
            10
        )
    );
    assert_eq!(
        s.eval::<i64>("return GetTradeskillRepeatCount()").unwrap(),
        3
    );
}

#[test]
fn selection_persists_across_a_repush_by_spell_id() {
    let mut s = UiScript::new().unwrap();
    s.set_trade_skill(Some(two_recipe_state()));

    // Bolt of Linen Cloth (spell 2963) is row 3.
    s.run("SelectTradeSkill(3)").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        3
    );

    // A reordered re-push: the engine re-sorts, and the selection follows the spell id to row 3.
    let mut reordered = two_recipe_state();
    reordered.recipes.swap(0, 1);
    s.set_trade_skill(Some(reordered));
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        3
    );

    // A re-push without the recipe clears the selection.
    let mut without_2963 = two_recipe_state();
    without_2963.recipes.remove(0);
    s.set_trade_skill(Some(without_2963));
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        0
    );
}

#[test]
fn do_trade_skill_drains_spell_id_and_count() {
    let mut s = UiScript::new().unwrap();
    s.set_trade_skill(Some(two_recipe_state()));

    // Row 3 = Bolt of Linen Cloth (spell 2963).
    s.run("DoTradeSkill(3, 5)").unwrap();
    assert_eq!(s.take_trade_skill_dos(), vec![(2963, 5)]);
    assert!(s.take_trade_skill_dos().is_empty(), "drained");

    // Row 2 = Simple Robe (spell 3919, numAvailable 0): a missing or zero count is 1 (`0x500280`).
    s.run("DoTradeSkill(2) DoTradeSkill(2, 0)").unwrap();
    assert_eq!(s.take_trade_skill_dos(), vec![(3919, 1), (3919, 1)]);

    s.run("DoTradeSkill(99)").unwrap();
    assert!(s.take_trade_skill_dos().is_empty());

    // Row 1 is the "Cloth" header.
    s.run("DoTradeSkill(1, 5)").unwrap();
    assert!(s.take_trade_skill_dos().is_empty());
}

#[test]
fn no_snapshot_shapes_unknown_line_and_nil_info() {
    let mut s = UiScript::new().unwrap();
    assert_eq!(
        s.eval::<(String, i64, i64)>("return GetTradeSkillLine()")
            .unwrap(),
        ("UNKNOWN".into(), 0, 0)
    );
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 0);
    assert_eq!(s.eval::<i64>("return GetFirstTradeSkill()").unwrap(), 0);
    assert!(s
        .eval::<bool>("return GetTradeSkillInfo(1) == nil")
        .unwrap());
    assert!(s
        .eval::<bool>("return GetTradeSkillIcon(1) == nil")
        .unwrap());
    assert_eq!(
        s.eval::<(i64, i64)>("return GetTradeSkillNumMade(1)")
            .unwrap(),
        (0, 0)
    );
    assert!(s
        .eval::<bool>("return GetTradeSkillCooldown(1) == nil")
        .unwrap());
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillNumReagents(1)").unwrap(),
        0
    );
    assert!(s
        .eval::<bool>("return GetTradeSkillReagentInfo(1, 1) == nil")
        .unwrap());
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        0
    );

    s.run("DoTradeSkill(1, 1)").unwrap();
    assert!(s.take_trade_skill_dos().is_empty(), "no window, no intent");

    s.run("CollapseTradeSkillSubClass(0) ExpandTradeSkillSubClass(1) SelectTradeSkill(1)")
        .unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 0);
}

#[test]
fn get_trade_skill_tools_multivalue_shape() {
    let mut s = UiScript::new().unwrap();
    let mut t = two_recipe_state();
    t.recipes[0].tools = vec![("Anvil".into(), true), ("Mining Pick".into(), false)];
    s.set_trade_skill(Some(t));

    // Row 3 = Bolt of Linen Cloth (recipes[0]).
    let (a, b, c, d) = s
        .eval::<(String, Option<i64>, String, Option<i64>)>(
            "local a,b,c,d = GetTradeSkillTools(3) return a,b,c,d",
        )
        .unwrap();
    assert_eq!(
        (a.as_str(), b, c.as_str(), d),
        ("Anvil", Some(1), "Mining Pick", None)
    );

    // No tools: zero values, not one nil.
    let mut t2 = two_recipe_state();
    t2.recipes[0].tools.clear();
    s.set_trade_skill(Some(t2));
    assert_eq!(s.arity("GetTradeSkillTools(3)").unwrap(), 0);

    // A header row: zero values too.
    assert_eq!(s.arity("GetTradeSkillTools(1)").unwrap(), 0);
}

/// Persistence keyed by skill line (`0x4fc910`, `0xbde064`).
#[test]
fn close_reopen_keeps_state_for_the_same_line_and_resets_on_a_switch() {
    let mut s = UiScript::new().unwrap();
    s.set_trade_skill(Some(two_recipe_state()));
    s.run("SelectTradeSkill(2)").unwrap(); // Simple Robe, the tier-0 row
    s.run("CollapseTradeSkillSubClass(1)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 1);

    assert!(!s.take_trade_skill_close());
    s.run("CloseTradeSkill()").unwrap();
    assert!(s.take_trade_skill_close());
    assert!(!s.take_trade_skill_close(), "drained");

    s.set_trade_skill(None);
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 0);
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        0
    );

    // The same line: the fold survives...
    s.set_trade_skill(Some(two_recipe_state()));
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 1);
    // ...and so does the selection, by spell id.
    s.run("ExpandTradeSkillSubClass(1)").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        2
    );

    // A different line resets everything.
    s.run("CollapseTradeSkillSubClass(1)").unwrap();
    let mut other = two_recipe_state();
    other.line = 164; // Blacksmithing
    s.set_trade_skill(Some(other));
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 3);
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        0
    );
}

/// The subclass filter as the stock menu drives it (`Blizzard_TradeSkillUI.lua:406-409`).
#[test]
fn subclass_filter_exclusive_narrows_list_but_not_vocabulary() {
    let mut s = UiScript::new().unwrap();
    s.set_trade_skill(Some(state()));
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 10);
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSubClassFilter(0)")
            .unwrap(),
        1
    );

    // Exclusive to "Armor Kit" (group order index 2): its header + two recipes remain.
    s.run("SetTradeSkillSubClassFilter(2, 1, 1)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 3);
    assert_eq!(row_kind(&mut s, 1), ("Armor Kit".into(), "header".into()));
    assert_eq!(
        s.arity("GetTradeSkillSubClasses()").unwrap(),
        4,
        "the dropdown vocabulary stays full under a filter"
    );
    assert!(s
        .eval::<Option<i64>>("return GetTradeSkillSubClassFilter(0)")
        .unwrap()
        .is_none());
    assert!(s
        .eval::<Option<i64>>("return GetTradeSkillSubClassFilter(1)")
        .unwrap()
        .is_none());
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSubClassFilter(2)")
            .unwrap(),
        1
    );

    // The "All Subclasses" row's call restores everything.
    s.run("SetTradeSkillSubClassFilter(0, 1, 1)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 10);
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSubClassFilter(0)")
            .unwrap(),
        1
    );
}

/// Stand-in slot strings named after their tokens (`0x84dd70`): `SECONDARYHANDSLOT`,
/// `INVTYPE_SHIELD` and `INVTYPE_WEAPONOFFHAND` all read "Off Hand" in enUS, so only the token
/// shows which one a bit reaches.
fn seed_slot_tokens(s: &mut UiScript) {
    s.run(
        r#"
        HEADSLOT = "<HEADSLOT>";       NECKSLOT = "<NECKSLOT>"
        SHOULDERSLOT = "<SHOULDERSLOT>"; SHIRTSLOT = "<SHIRTSLOT>"
        CHESTSLOT = "<CHESTSLOT>";     WAISTSLOT = "<WAISTSLOT>"
        LEGSSLOT = "<LEGSSLOT>";       FEETSLOT = "<FEETSLOT>"
        WRISTSLOT = "<WRISTSLOT>";     HANDSSLOT = "<HANDSSLOT>"
        FINGER0SLOT = "<FINGER0SLOT>"; FINGER1SLOT = "<FINGER1SLOT>"
        TRINKET0SLOT = "<TRINKET0SLOT>"; TRINKET1SLOT = "<TRINKET1SLOT>"
        BACKSLOT = "<BACKSLOT>";       MAINHANDSLOT = "<MAINHANDSLOT>"
        SECONDARYHANDSLOT = "<SECONDARYHANDSLOT>"; RANGEDSLOT = "<RANGEDSLOT>"
        TABARDSLOT = "<TABARDSLOT>";   BAGSLOT = "<BAGSLOT>"
        NONEQUIPSLOT = "<NONEQUIPSLOT>"
    "#,
    )
    .unwrap();
}

#[test]
fn invslot_filter_drops_recipes_and_emptied_groups() {
    let mut s = UiScript::new().unwrap();
    seed_slot_tokens(&mut s);
    let mut st = state();
    // Wind Cloak is the one Back product (16 → bit 14); everything else stays Robe/Chest.
    st.recipes
        .iter_mut()
        .find(|r| r.name == "Wind Cloak")
        .unwrap()
        .product_inv_type = 16;
    s.set_trade_skill(Some(st));

    assert_eq!(
        s.eval::<(String, String)>("return GetTradeSkillInvSlots()")
            .unwrap(),
        ("<CHESTSLOT>".to_string(), "<BACKSLOT>".to_string()),
        "ascending slot-bit order (4 before 14)"
    );
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillInvSlotFilter(0)")
            .unwrap(),
        1
    );

    // Exclusive to "Back" (list index 2): only Zephyr Cloak's header + Wind Cloak survive.
    s.run("SetTradeSkillInvSlotFilter(2, 1, 1)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 2);
    assert_eq!(
        row_kind(&mut s, 1),
        ("Zephyr Cloak".into(), "header".into())
    );
    assert_eq!(row_kind(&mut s, 2), ("Wind Cloak".into(), "medium".into()));

    s.run("SetTradeSkillInvSlotFilter(0, 1, 1)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 10);
}

/// A one-hand weapon (InventoryType 13, `0x18000`) lists both hand slots and passes either.
#[test]
fn one_hand_weapon_spans_both_hand_slots() {
    let mut s = UiScript::new().unwrap();
    seed_slot_tokens(&mut s);
    let mut st = two_recipe_state();
    st.recipes[0].product_inv_type = 13; // Bolt of Linen Cloth becomes a one-hand weapon
    s.set_trade_skill(Some(st));

    assert_eq!(
        s.eval::<(String, String, String)>("return GetTradeSkillInvSlots()")
            .unwrap(),
        (
            "<CHESTSLOT>".to_string(),
            "<MAINHANDSLOT>".to_string(),
            // Not `INVTYPE_WEAPONOFFHAND`: this dropdown is the paper-doll family.
            "<SECONDARYHANDSLOT>".to_string()
        )
    );
    // Exclusive "Off Hand" (index 3): the weapon row survives, the robe row hides.
    s.run("SetTradeSkillInvSlotFilter(3, 1, 1)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 2);
    assert_eq!(
        row_kind(&mut s, 2),
        ("Bolt of Linen Cloth".into(), "trivial".into())
    );
}

/// Each filter or fold raises the flag the app answers with TRADE_SKILL_UPDATE (`0x4fd710`,
/// `0x4fd750`).
#[test]
fn filter_and_fold_mutators_raise_the_touched_flag() {
    let mut s = UiScript::new().unwrap();
    s.set_trade_skill(Some(state()));
    assert!(!s.take_trade_skill_touched());

    s.run("SetTradeSkillSubClassFilter(2, 1, 1)").unwrap();
    assert!(s.take_trade_skill_touched());
    assert!(!s.take_trade_skill_touched(), "drained");

    s.run("SetTradeSkillInvSlotFilter(0, 1, 1)").unwrap();
    assert!(s.take_trade_skill_touched());

    s.run("CollapseTradeSkillSubClass(0)").unwrap();
    assert!(s.take_trade_skill_touched());
    s.run("ExpandTradeSkillSubClass(0)").unwrap();
    assert!(s.take_trade_skill_touched());
}

/// ItemLevel (`record+0x14`) sorts between tier and name.
#[test]
fn same_tier_recipes_order_by_product_item_level_before_name() {
    let mut s = UiScript::new().unwrap();
    let mut aaa = recipe(
        100,
        "Aaa Robe",
        TradeSkillDifficulty::Medium,
        1,
        Some((2, 1, "Cloth")),
    );
    aaa.product_item_level = 30;
    let mut zzz = recipe(
        101,
        "Zzz Robe",
        TradeSkillDifficulty::Medium,
        1,
        Some((2, 1, "Cloth")),
    );
    zzz.product_item_level = 10;
    let mut mid = recipe(
        102,
        "Mmm Robe",
        TradeSkillDifficulty::Medium,
        1,
        Some((2, 1, "Cloth")),
    );
    mid.product_item_level = 10;
    s.set_trade_skill(Some(TradeSkillState {
        line: 197,
        line_name: "Tailoring".into(),
        rank: 57,
        max_rank: 75,
        recipes: vec![aaa, zzz, mid],
        repeat_count: 0,
    }));
    // Header, then: Mmm(10) before Zzz(10) by name, both before Aaa(30) despite the alphabet.
    let names: Vec<String> = (1..=4)
        .map(|i| {
            s.eval::<String>(&format!("local n = GetTradeSkillInfo({i}) return n"))
                .unwrap()
        })
        .collect();
    assert_eq!(names, ["Cloth", "Mmm Robe", "Zzz Robe", "Aaa Robe"]);
}

/// The two link verbs' shapes (`0x4ff410`, `0x4ff800`).
#[test]
fn the_link_verbs_answer_the_clients_shapes() {
    let mut s = UiScript::new().unwrap();
    s.set_trade_skill(Some(state()));
    // Row 2 is the first recipe, found by name since the visible order is the sorted one.
    let st = state();
    let row2 = s.eval::<String>("return (GetTradeSkillInfo(2))").unwrap();
    let r = st
        .recipes
        .iter()
        .find(|r| r.name == row2)
        .expect("row 2 is a pushed recipe");
    s.set_item_template(
        r.product_item,
        crate::script::ItemTemplateView {
            name: "Copper Chain Belt".into(),
            quality: 2,
            ..Default::default()
        },
    );
    assert_eq!(
        s.arity("GetTradeSkillItemLink(1)").unwrap(),
        0,
        "a header row answers zero values"
    );
    let link = s
        .eval::<String>("return (GetTradeSkillItemLink(2))")
        .unwrap();
    assert_eq!(
        link,
        format!(
            "|cff1eff00|Hitem:{}:0:0:0|h[Copper Chain Belt]|h|r",
            r.product_item
        )
    );
    assert!(
        s.eval::<bool>("return GetTradeSkillReagentItemLink(2, 1) == nil")
            .unwrap(),
        "an uncached reagent template is nil, and nothing is queried"
    );
    assert!(s.take_item_stat_asks().is_empty());
    s.set_item_template(
        r.reagents[0].item,
        crate::script::ItemTemplateView {
            name: "Copper Bar".into(),
            quality: 1,
            ..Default::default()
        },
    );
    assert_eq!(
        s.eval::<String>("return GetTradeSkillReagentItemLink(2, 1)")
            .unwrap(),
        format!(
            "|cffffffff|Hitem:{}:0:0:0|h[Copper Bar]|h|r",
            r.reagents[0].item
        )
    );
    assert_eq!(
        s.arity("GetTradeSkillReagentItemLink(2, 9)").unwrap(),
        1,
        "past the reagents: still exactly one value"
    );
    assert!(
        s.eval::<bool>("return GetTradeSkillReagentItemLink(2, 9) == nil")
            .unwrap(),
        "past the reagents: that one value is nil"
    );
    let err = s
        .run("GetTradeSkillReagentItemLink(2, nil)")
        .expect_err("non-number")
        .to_string();
    assert!(
        err.contains("Usage: GetTradeReagentSkillItemLink("),
        "{err}"
    );
    assert!(s.run("GetTradeSkillItemLink('x')").is_err());
}
