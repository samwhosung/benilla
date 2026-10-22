//! The quest log's order and the selected quest's detail getters at the bytes: the rebuild's two
//! `qsort` passes, the abandon list, the reward money's level bonus and the reward rows' quality
//! and usability.

use super::fixture::{collect, record, seat, slot};
use super::QuestRecordView;
use crate::script::{ItemTemplateView, PlayerReqState, UiScript, UnitState};

/// The log's rows by title, top to bottom.
fn titles(s: &UiScript) -> Vec<String> {
    s.eval::<Vec<String>>(
        "local t = {} for i = 1, GetNumQuestLogEntries() do t[i] = GetQuestLogTitle(i) end \
         return t",
    )
    .unwrap()
}

/// `0x4de510` fills the list in slot order, each quest ahead of its group's header (`0x4de68e`,
/// `0x4de713`), and `0x4dea50` sorts it with the CRT `qsort` (`0x73f727`) over `0x4deac0`, which
/// never reads the slot: two quests of one level and one title, case aside (`0x4decfb` through
/// `0x64a4c0`), land where `shortsort` leaves them, the later slot first.
#[test]
fn a_full_tie_lists_where_qsort_leaves_it_not_in_slot_order() {
    let mut s = UiScript::new().unwrap();
    seat(
        &mut s,
        &[
            (
                10,
                record("Wolf Pelts", 5, 12, "Elwynn Forest"),
                slot(10, 0, 0),
            ),
            (
                11,
                record("WOLF PELTS", 5, 12, "Elwynn Forest"),
                slot(11, 0, 0),
            ),
        ],
    );
    assert_eq!(titles(&s), ["Elwynn Forest", "WOLF PELTS", "Wolf Pelts"]);
}

/// The groups' ids, first seen first (`0x4de6f6`), go through the same `qsort` with `0x4de8f0`
/// (`0x4de751`), which ties two names that differ in case alone (`0x4dea44`): the later group
/// heads the list.
#[test]
fn two_groups_whose_names_tie_list_where_qsort_leaves_them() {
    let mut s = UiScript::new().unwrap();
    seat(
        &mut s,
        &[
            (
                10,
                record("Wolf Pelts", 5, 12, "Elwynn Forest"),
                slot(10, 0, 0),
            ),
            (
                11,
                record("Bear Pelts", 5, 99, "ELWYNN FOREST"),
                slot(11, 0, 0),
            ),
        ],
    );
    assert_eq!(
        titles(&s),
        ["ELWYNN FOREST", "Bear Pelts", "Elwynn Forest", "Wolf Pelts"]
    );
}

const TUSK: u32 = 1000;
const FANG: u32 = 1001;
const PELT: u32 = 1002;

/// Item `id`, named `name`, with `bonding`.
fn item(name: &str, bonding: u32) -> ItemTemplateView {
    ItemTemplateView {
        name: name.into(),
        quality: 1,
        bonding,
        ..Default::default()
    }
}

/// Quest 10 needs tusks and fangs (both quest items) and pelts (not), quest 11 needs fangs too;
/// the player carries all three. Quest 10 is selected and marked.
fn abandon_log() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level: 10,
            ..Default::default()
        }),
    );
    s.set_item_template(TUSK, item("Tusk", 4));
    s.set_item_template(FANG, item("Fang", 5));
    s.set_item_template(PELT, item("Pelt", 1));
    let mut boars = record("Boars", 5, 12, "Elwynn Forest");
    boars.objectives[0] = collect(TUSK, 5);
    boars.objectives[1] = collect(FANG, 5);
    boars.objectives[2] = collect(PELT, 5);
    let mut wolves = record("Wolves", 6, 12, "Elwynn Forest");
    wolves.objectives[0] = collect(FANG, 5);
    seat(
        &mut s,
        &[(10, boars, slot(10, 0, 0)), (11, wolves, slot(11, 0, 0))],
    );
    s.set_quest_item_counts([(TUSK, 2), (FANG, 1), (PELT, 3)].into_iter().collect());
    s.run("SelectQuestLogEntry(2); SetAbandonQuest()").unwrap();
    s
}

/// `GetAbandonQuestItems` (`0x4dfbe0`) keeps the carried items of bonding 4 or 5 the marked
/// quest needs (`0x4dfc8a`, `0x4dfcb1`), drops those another row's quest needs (`0x4dfd42`), and
/// answers one string of their names (`0x4dfde3`).
#[test]
fn the_abandon_list_names_the_carried_quest_items_no_other_quest_needs() {
    let s = abandon_log();
    assert_eq!(
        s.eval::<String>("return GetAbandonQuestItems()").unwrap(),
        "Tusk"
    );
}

/// Two kept names join with the list separator (`0x84480c`, pushed at `0x4dfdb9`).
#[test]
fn two_abandoned_items_join_with_the_list_separator() {
    let mut s = abandon_log();
    s.set_quest_item_counts([(TUSK, 2), (FANG, 1)].into_iter().collect());
    // Quest 11 leaves the log, so nothing else needs the fangs.
    let mut slots = [Default::default(); super::QUEST_LOG_SLOTS];
    slots[0] = slot(10, 0, 0);
    s.quest_log_changed(slots);
    assert_eq!(
        s.eval::<String>("return GetAbandonQuestItems()").unwrap(),
        "Tusk, Fang"
    );
}

/// Nothing kept, no player, or no record for the mark: no value at all (`0x4dfd72`, `0x4dfc22`,
/// `0x4dfc54`).
#[test]
fn an_empty_abandon_list_answers_nothing() {
    let mut s = abandon_log();
    s.set_quest_item_counts([(PELT, 3)].into_iter().collect());
    assert_eq!(s.arity("GetAbandonQuestItems()").unwrap(), 0);
    let mut s = abandon_log();
    s.set_quest_log_player(false);
    assert_eq!(s.arity("GetAbandonQuestItems()").unwrap(), 0);
    let s = UiScript::new().unwrap();
    s.run("SetAbandonQuest()").unwrap();
    assert_eq!(s.arity("GetAbandonQuestItems()").unwrap(), 0);
}

/// A miss pushes nil, never `""`: `GetAbandonQuestName` one (`0x4dfbc3`), `GetQuestLogQuestText`
/// two (`0x4dffdb`).
#[test]
fn the_abandon_name_and_the_quest_text_miss_as_nil() {
    let s = UiScript::new().unwrap();
    s.run("SetAbandonQuest()").unwrap();
    assert!(s
        .eval::<bool>("return GetAbandonQuestName() == nil")
        .unwrap());
    assert!(s
        .eval::<bool>("local d, o = GetQuestLogQuestText() return d == nil and o == nil")
        .unwrap());
    assert_eq!(s.arity("GetQuestLogQuestText()").unwrap(), 2);
}

/// One quest selected, its base money `money` and its level-60 money `bonus`, the player at
/// `level`.
fn money_log(money: i32, bonus: i32, level: u32) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level,
            ..Default::default()
        }),
    );
    let mut r: QuestRecordView = record("Bounty", 60, 12, "Elwynn Forest");
    r.reward_money = money.max(0) as u32;
    r.required_money = if money < 0 { money.unsigned_abs() } else { 0 };
    r.reward_money_max_level = bonus;
    seat(&mut s, &[(10, r, slot(10, 0, 0))]);
    s.run("SelectQuestLogEntry(2)").unwrap();
    s
}

/// `GetQuestLogRewardMoney` (`0x4e1070`): the base money when not negative (`0x4e10f7`), plus the
/// template's `+0x2c` at level 60 and above (`0x4e10b2` `jl`, `0x4e1102`), a negative base
/// included.
#[test]
fn the_reward_money_adds_the_level_sixty_money_at_sixty() {
    let money = |s: &UiScript| s.eval::<i64>("return GetQuestLogRewardMoney()").unwrap();
    assert_eq!(money(&money_log(100, 50, 59)), 100);
    assert_eq!(money(&money_log(100, 50, 60)), 150);
    assert_eq!(money(&money_log(100, 50, 61)), 150);
    assert_eq!(money(&money_log(-100, 50, 60)), 50);
    assert_eq!(money(&money_log(-100, 50, 59)), 0);
}

const SWORD: u32 = 2000;

/// The selected quest offers the sword as its one reward and its one choice.
fn reward_log(sword: ItemTemplateView, req: PlayerReqState) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_player_req_state(req);
    s.set_item_template(SWORD, sword);
    let mut r = record("Sword", 5, 12, "Elwynn Forest");
    r.rewards = vec![(SWORD, 1)];
    r.choices = vec![(SWORD, 1)];
    seat(&mut s, &[(10, r, slot(10, 0, 0))]);
    s.run("SelectQuestLogEntry(2)").unwrap();
    s
}

fn warrior(level: u32) -> PlayerReqState {
    PlayerReqState {
        level,
        class_id: 1,
        race_id: 1,
        ..Default::default()
    }
}

fn sword(required_level: u32, inventory_type: u32) -> ItemTemplateView {
    ItemTemplateView {
        name: "Sword".into(),
        quality: 2,
        inventory_type,
        required_level,
        allowable_class: -1,
        allowable_race: -1,
        ..Default::default()
    }
}

/// Both log getters answer `isUsable` through the item-usable predicate `0x5ea930` (`0x4e0c83`,
/// `0x4e0e73`): 1 for an item the player can use, nil for one the gate refuses.
#[test]
fn the_log_reward_rows_answer_the_usable_gate() {
    for getter in ["GetQuestLogRewardInfo", "GetQuestLogChoiceInfo"] {
        let usable = |s: &UiScript| {
            s.eval::<bool>(&format!("local _, _, _, _, u = {getter}(1) return u == 1"))
                .unwrap()
        };
        let nil = |s: &UiScript| {
            s.eval::<bool>(&format!(
                "local _, _, _, _, u = {getter}(1) return u == nil"
            ))
            .unwrap()
        };
        assert!(usable(&reward_log(sword(5, 13), warrior(10))), "{getter}");
        assert!(nil(&reward_log(sword(20, 13), warrior(10))), "{getter}");
    }
}

/// Both log getters answer quality -1 for an item whose inventory type (`+0x2c`) is 0, the
/// template's quality (`+0x1c`) otherwise (`0x4e0c3c`).
#[test]
fn a_log_reward_of_inventory_type_zero_reads_quality_minus_one() {
    for getter in ["GetQuestLogRewardInfo", "GetQuestLogChoiceInfo"] {
        let quality = |s: &UiScript| {
            s.eval::<i64>(&format!("local _, _, _, q = {getter}(1) return q"))
                .unwrap()
        };
        assert_eq!(
            quality(&reward_log(sword(5, 0), warrior(10))),
            -1,
            "{getter}"
        );
        assert_eq!(
            quality(&reward_log(sword(5, 13), warrior(10))),
            2,
            "{getter}"
        );
    }
}

/// The headers by title, each with whether it reads collapsed.
fn headers(s: &UiScript) -> Vec<(String, bool)> {
    s.eval::<Vec<String>>(
        "local t = {} for i = 1, GetNumQuestLogEntries() do \
           local title, _, _, header, collapsed = GetQuestLogTitle(i) \
           if header then table.insert(t, (collapsed == 1 and '+' or '-') .. title) end \
         end return t",
    )
    .unwrap()
    .into_iter()
    .map(|h| (h[1..].to_string(), h.starts_with('+')))
    .collect()
}

/// The same slots again, which rebuilds the list (`0x4de510`).
fn rebuild(s: &mut UiScript, ids: &[u32]) {
    let mut slots = [Default::default(); super::QUEST_LOG_SLOTS];
    for (i, &id) in ids.iter().enumerate() {
        slots[i] = slot(id, 0, 0);
    }
    s.quest_log_changed(slots);
}

/// Each header's fold is its own bit of `0xbb748c`, by its place among the sorted ids
/// (`0x4ded8e`): two groups that share a name fold apart.
#[test]
fn two_groups_of_one_name_fold_apart() {
    let mut s = UiScript::new().unwrap();
    seat(
        &mut s,
        &[
            (10, record("Wolves", 5, 12, "Elwynn Forest"), slot(10, 0, 0)),
            (11, record("Bears", 5, 99, "Elwynn Forest"), slot(11, 0, 0)),
        ],
    );
    s.run("CollapseQuestHeader(1)").unwrap();
    assert_eq!(
        headers(&s),
        [
            ("Elwynn Forest".to_string(), true),
            ("Elwynn Forest".to_string(), false)
        ]
    );
}

/// A rebuild snapshots each old id as it is when folded and negated when open (`0x4de5bc`,
/// `0x4de5c4`), opens every header (`0x4de608`), and folds each new id equal to a snapshot
/// entry (`0x4de777`, `0x4de78d`): an open group 0 matches its own snapshot and folds.
#[test]
fn a_rebuild_folds_the_group_of_id_zero() {
    let mut s = UiScript::new().unwrap();
    seat(
        &mut s,
        &[
            (10, record("Wolves", 5, 0, "Misc"), slot(10, 0, 0)),
            (11, record("Bears", 5, 12, "Elwynn Forest"), slot(11, 0, 0)),
        ],
    );
    assert_eq!(
        headers(&s),
        [
            ("Misc".to_string(), false),
            ("Elwynn Forest".to_string(), false)
        ]
    );
    rebuild(&mut s, &[10, 11]);
    assert_eq!(
        headers(&s),
        [
            ("Misc".to_string(), true),
            ("Elwynn Forest".to_string(), false)
        ]
    );
}

/// The negated snapshot of an open group `N` is `-N`, which a group of id `-N` matches: after a
/// rebuild, open zone 12 and open sort -12 both fold, and a folded group stays folded.
#[test]
fn a_rebuild_folds_by_id_through_the_negated_snapshot() {
    let mut s = UiScript::new().unwrap();
    seat(
        &mut s,
        &[
            (10, record("Wolves", 5, 12, "Elwynn Forest"), slot(10, 0, 0)),
            (11, record("Bears", 5, -12, "Seasonal"), slot(11, 0, 0)),
            (12, record("Boars", 5, 40, "Westfall"), slot(12, 0, 0)),
        ],
    );
    // Westfall, the third header, folds.
    s.run("CollapseQuestHeader(5)").unwrap();
    rebuild(&mut s, &[10, 11, 12]);
    assert_eq!(
        headers(&s),
        [
            ("Elwynn Forest".to_string(), true),
            ("Seasonal".to_string(), true),
            ("Westfall".to_string(), true)
        ]
    );
}

/// The mask and the header ids are engine globals a `/reload` keeps: a new VM seated from the
/// old one's carry folds by the old snapshot at its first rebuild.
#[test]
fn the_folds_outlive_the_vm_by_id() {
    let mut s = UiScript::new().unwrap();
    seat(
        &mut s,
        &[
            (10, record("Wolves", 5, 12, "Elwynn Forest"), slot(10, 0, 0)),
            (11, record("Bears", 5, 99, "Elwynn Forest"), slot(11, 0, 0)),
        ],
    );
    s.run("CollapseQuestHeader(1)").unwrap();
    let carry = s.quest_log_carry();
    let mut t = UiScript::new().unwrap();
    t.seed_quest_log_carry(carry);
    seat(
        &mut t,
        &[
            (10, record("Wolves", 5, 12, "Elwynn Forest"), slot(10, 0, 0)),
            (11, record("Bears", 5, 99, "Elwynn Forest"), slot(11, 0, 0)),
        ],
    );
    assert_eq!(
        headers(&t),
        [
            ("Elwynn Forest".to_string(), true),
            ("Elwynn Forest".to_string(), false)
        ]
    );
}
