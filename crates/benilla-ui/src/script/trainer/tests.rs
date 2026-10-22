//! Tests for the trainer tree: the per-type row orders, the filter, the collapse and the intents.

use super::*;
use crate::script::UiScript;

fn svc(
    spell_id: u32,
    name: &str,
    cat: TrainerServiceCategory,
    group_key: u32,
    group_name: &str,
) -> TrainerService {
    TrainerService {
        spell_id,
        tooltip: TrainerTooltip::Spell {
            spell_id,
            alt_caster: false,
        },
        name: Some(name.into()),
        subtext: Some("Rank 1".into()),
        texture: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
        icon_wait: None,
        description: Some(format!("Teaches {name}.")),
        description_wait: None,
        cost: 100,
        prof_first_rank: false,
        category: cat,
        level_req: 10,
        skill_req: None,
        ability_reqs: vec![],
        is_trade_skill: false,
        learn_spell: Some(false),
        group_key,
        group_name: group_name.into(),
        skill_step: false,
        step_increase: None,
        line_name: Some(group_name.into()),
    }
}

/// A class trainer whose tree is `[H:Arms, Heroic Strike, Cleave, H:Fury, Bloodrage]`, the three
/// services available, used and unavailable.
fn trainer() -> TrainerState {
    let mut hs = svc(
        78,
        "Heroic Strike",
        TrainerServiceCategory::Available,
        26,
        "Arms",
    );
    hs.level_req = 1;
    let mut cl = svc(284, "Cleave", TrainerServiceCategory::Used, 26, "Arms");
    cl.level_req = 20;
    let br = svc(
        285,
        "Bloodrage",
        TrainerServiceCategory::Unavailable,
        256,
        "Fury",
    );
    TrainerState {
        guid: 0x42,
        greeting: "What would you like to learn?".into(),
        trainer_type: 0,
        groups: Vec::new(),
        services: vec![hs, cl, br],
    }
}

/// Every visible row as `(name, serviceType)`.
fn visible(s: &mut UiScript) -> Vec<(String, String)> {
    let n = s.eval::<i64>("return GetNumTrainerServices()").unwrap();
    (1..=n)
        .map(|i| {
            s.eval::<(String, String)>(&format!(
                "local n,_,t = GetTrainerServiceInfo({i}) return n,t"
            ))
            .unwrap()
        })
        .collect()
}

#[test]
fn tree_interleaves_headers_and_ordered_services() {
    let mut s = UiScript::new().unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 0);
    assert!(s
        .eval::<bool>("return GetTrainerServiceInfo(1) == nil")
        .unwrap());

    s.set_trainer(Some(trainer()));
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 5);

    let (hn, hsub, ht, hexp) = s
        .eval::<(String, Option<String>, String, Option<i64>)>(
            "local n,s,t,e = GetTrainerServiceInfo(1) return n,s,t,e",
        )
        .unwrap();
    assert_eq!((hn.as_str(), ht.as_str()), ("Arms", "header"));
    // The header's subText is the empty string (`0x4d8b50`), not nil.
    assert_eq!((hsub.as_deref(), hexp), (Some(""), Some(1)));

    let info = |s: &mut UiScript, i: i64| {
        s.eval::<(String, String)>(&format!(
            "local n,_,t = GetTrainerServiceInfo({i}) return n,t"
        ))
        .unwrap()
    };
    assert_eq!(
        info(&mut s, 2),
        ("Heroic Strike".into(), "available".into())
    );
    assert_eq!(info(&mut s, 3), ("Cleave".into(), "used".into()));
    assert_eq!(info(&mut s, 4), ("Fury".into(), "header".into()));
    assert_eq!(info(&mut s, 5), ("Bloodrage".into(), "unavailable".into()));
}

/// A service row whose spell has no rank answers the empty string for its subText (the Spell
/// record's `NameSubtext`, `0x4d8b84`), never nil: a client's FrameXML may `format` it before its
/// nil guard, which throws on nil. Without a Spell record `0x4d8b50` returns NULL (`0x4d8b58`),
/// which `0x6f3890` pushes as nil.
#[test]
fn a_rankless_service_answers_an_empty_string_for_its_subtext() {
    let mut s = UiScript::new().unwrap();
    let mut t = trainer();
    t.services[0].subtext = Some(String::new()); // Heroic Strike, row 2
    s.set_trainer(Some(t));

    let (name, sub, ty) = s
        .eval::<(String, mlua::Value, String)>(
            "local n,s,t = GetTrainerServiceInfo(2) return n,s,t",
        )
        .unwrap();
    assert_eq!((name.as_str(), ty.as_str()), ("Heroic Strike", "available"));
    match sub {
        mlua::Value::String(v) => assert_eq!(v.to_str().unwrap(), ""),
        other => panic!("the rankless subText must be a string, got {other:?}"),
    }
    // A `format` over it must not throw.
    assert_eq!(
        s.eval::<String>("local _,sub = GetTrainerServiceInfo(2) return format(\"(%s)\", sub)")
            .unwrap(),
        "()"
    );
    // A ranked service is unchanged.
    assert_eq!(
        s.eval::<String>("local _,sub = GetTrainerServiceInfo(3) return sub")
            .unwrap(),
        "Rank 1"
    );
}

/// A service with no Spell record has no subText: nil (`0x4d8b58` -> `0x6f3890`).
#[test]
fn a_service_without_a_spell_record_answers_nil_for_its_subtext() {
    let mut s = UiScript::new().unwrap();
    let mut t = trainer();
    t.services[0].subtext = None; // Heroic Strike, row 2
    s.set_trainer(Some(t));
    assert!(s
        .eval::<bool>("local _,sub = GetTrainerServiceInfo(2) return sub == nil")
        .unwrap());
}

/// The fourth value of a service row is 1 (`0x4d8e42`: the header test `0x4d8c30` answers 0 for a
/// row that is no header, which pushes 1), not nil; a collapsed header's is nil.
#[test]
fn a_service_row_answers_one_for_its_fourth_value() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    let fourth = |s: &mut UiScript, row: u32| -> (i64, Option<i64>) {
        s.eval(&format!(
            "local n = function(...) return arg.n, arg[4] end return n(GetTrainerServiceInfo({row}))"
        ))
        .unwrap()
    };
    // Rows: Arms header, Heroic Strike, Cleave, Fury header, Bloodrage.
    assert_eq!(fourth(&mut s, 1), (4, Some(1)));
    for service in [2, 3, 5] {
        assert_eq!(fourth(&mut s, service), (4, Some(1)), "row {service}");
    }
    s.run("CollapseTrainerSkillLine(1)").unwrap();
    assert_eq!(fourth(&mut s, 1), (4, None), "a collapsed header");
}

/// `GetTrainerServiceDescription` (`0x4d9b40`) pushes one value, nil (`0x4d9d90`) where there is
/// no text to show: no row at the index (`0x4d9ba8` to `0x4d9d93`), a service with no spell record
/// (`0x4d9bd7`), or a service whose spell has none (`0x4d9d44`, `0x4d9d49`). A header row has no service either.
#[test]
fn service_description_is_nil_without_text() {
    let mut s = UiScript::new().unwrap();
    let mut t = trainer();
    t.services[1].description = None;
    s.set_trainer(Some(t));

    // `arg.n` counts the returns (`select` is 5.1's, not a 1.12 global).
    let shape = |s: &mut UiScript, i: i64| {
        s.eval::<String>(&format!(
            "local function c(...) return arg.n .. ':' .. type(arg[1]) end \
             return c(GetTrainerServiceDescription({i}))"
        ))
        .unwrap()
    };
    assert_eq!(shape(&mut s, 2), "1:string");
    assert_eq!(shape(&mut s, 3), "1:nil", "a service with no text");
    assert_eq!(shape(&mut s, 4), "1:nil", "a header row");
    assert_eq!(shape(&mut s, 99), "1:nil", "past the last row");
    assert_eq!(shape(&mut s, 0), "1:nil", "before the first");
}

#[test]
fn service_getters_read_the_row_at_a_visible_index() {
    let mut s = UiScript::new().unwrap();
    let mut t = trainer();
    // Heroic Strike, row 2, gets a full gate set.
    t.services[0].skill_req = Some(TrainerSkillReq {
        name: "Blacksmithing".into(),
        rank: 100,
        met: true,
    });
    t.services[0].ability_reqs = vec![TrainerAbilityReq {
        spell_id: 2,
        name: Some("Apprentice".into()),
        met: false,
    }];
    s.set_trainer(Some(t));

    assert_eq!(
        s.eval::<(i64, i64, i64)>("return GetTrainerServiceCost(2)")
            .unwrap(),
        (100, 0, 0)
    );
    assert_eq!(
        s.eval::<i64>("return GetTrainerServiceLevelReq(2)")
            .unwrap(),
        1
    );
    assert_eq!(
        s.eval::<String>("return GetTrainerServiceDescription(2)")
            .unwrap(),
        "Teaches Heroic Strike."
    );
    let (skill, rank, has) = s
        .eval::<(String, i64, i64)>("return GetTrainerServiceSkillReq(2)")
        .unwrap();
    assert_eq!((skill.as_str(), rank, has), ("Blacksmithing", 100, 1));
    assert!(s
        .eval::<bool>(
            "local n,h = GetTrainerServiceAbilityReq(2,1) return n=='Apprentice' and h==nil"
        )
        .unwrap());
    assert!(s
        .eval::<bool>("local l,p = IsTrainerServiceLearnSpell(2) return l==1 and p==nil")
        .unwrap());

    // Row 1 is a header, so the getters answer their defaults.
    assert_eq!(
        s.eval::<i64>("return GetTrainerServiceLevelReq(1)")
            .unwrap(),
        0
    );
    assert!(s
        .eval::<bool>("return GetTrainerServiceIcon(1) == nil")
        .unwrap());
    assert!(s
        .eval::<bool>("return GetTrainerServiceSkillReq(1) == nil")
        .unwrap());
}

/// The reference's filter hide exempts no header (`0x4d8528`, `0x4d8535`).
#[test]
fn state_filter_takes_a_groups_header_with_its_last_service() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 5);

    s.run("SetTrainerServiceTypeFilter('used', 0)").unwrap();
    assert_eq!(
        visible(&mut s)
            .into_iter()
            .map(|(_, t)| t)
            .collect::<Vec<_>>(),
        ["header", "available", "header", "unavailable"]
    );

    s.run("SetTrainerServiceTypeFilter('available', 0)")
        .unwrap();
    assert_eq!(
        visible(&mut s),
        [
            ("Fury".to_string(), "header".to_string()),
            ("Bloodrage".to_string(), "unavailable".to_string()),
        ]
    );

    s.run("SetTrainerServiceTypeFilter('unavailable', 0)")
        .unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 0);

    // The rows remain: the getters bound on the total `ds:0xb73a10` (`0x4d89b0`), not the visible
    // count `ds:0xb73a18`.
    let mut names: Vec<String> = (1..=5)
        .map(|i| {
            s.eval::<String>(&format!("return (GetTrainerServiceInfo({i}))"))
                .unwrap()
        })
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["Arms", "Bloodrage", "Cleave", "Fury", "Heroic Strike"],
        "every row is still in the hidden tail"
    );
    assert!(s
        .eval::<bool>("return GetTrainerServiceInfo(6) == nil")
        .unwrap());
    assert_eq!(
        s.eval::<i64>("return GetTrainerSelectionIndex()").unwrap(),
        0,
        "and nothing is selected, which is a different question from what row 1 holds"
    );
}

/// The reference's collapse test exempts headers (`0x4d853d`), where the filter's does not.
#[test]
fn collapse_keeps_the_header_the_filter_would_remove() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    s.run("CollapseTrainerSkillLine(1)").unwrap(); // fold Arms
    assert_eq!(
        visible(&mut s),
        [
            ("Arms".to_string(), "header".to_string()),
            ("Fury".to_string(), "header".to_string()),
            ("Bloodrage".to_string(), "unavailable".to_string()),
        ]
    );
    s.run("CollapseTrainerSkillLine(0)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 2);
}

#[test]
fn collapse_by_header_index_and_collapse_all() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));

    s.run("CollapseTrainerSkillLine(1)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 3);
    assert!(s
        .eval::<bool>("local _,_,t,e = GetTrainerServiceInfo(1) return t=='header' and e==nil")
        .unwrap());
    assert_eq!(
        s.eval::<String>("local n = GetTrainerServiceInfo(2) return n")
            .unwrap(),
        "Fury",
        "Arms' services are folded; Fury's header is now row 2"
    );

    s.run("ExpandTrainerSkillLine(1)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 5);

    s.run("CollapseTrainerSkillLine(0)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 2);
    s.run("ExpandTrainerSkillLine(0)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 5);
}

#[test]
fn collapse_survives_a_content_update_and_resets_on_close() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    s.run("CollapseTrainerSkillLine(1)").unwrap(); // fold Arms
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 3);

    // A content update over the same groups keeps the fold.
    s.set_trainer(Some(trainer()));
    assert_eq!(
        s.eval::<i64>("return GetNumTrainerServices()").unwrap(),
        3,
        "Arms stays collapsed across a content update"
    );

    s.set_trainer(None);
    s.set_trainer(Some(trainer()));
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 5);
}

/// The reference's buy refuses any state byte but 0 (`0x4d89d0`, `0x4d89e5`).
#[test]
fn buy_calls_with_an_available_services_spell_id_and_refuses_every_other_row() {
    let mut s = UiScript::new().unwrap();
    s.keep_calls();
    s.set_trainer(Some(trainer()));
    s.run("BuyTrainerService(2)").unwrap();
    assert_eq!(
        s.take_trainer_calls(),
        vec![TrainerCall::Buy {
            trainer: 0x42,
            spell_id: 78
        }]
    );

    s.run("BuyTrainerService(1)").unwrap();
    assert!(s.take_trainer_calls().is_empty(), "a header is not buyable");

    s.run("BuyTrainerService(3)").unwrap();
    s.run("BuyTrainerService(5)").unwrap();
    assert!(
        s.take_trainer_calls().is_empty(),
        "only state byte 0 — available — reaches the wire"
    );
}

#[test]
fn selection_and_close_intents() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    assert_eq!(
        s.eval::<i64>("return GetTrainerSelectionIndex()").unwrap(),
        0
    );
    s.run("SelectTrainerService(2)").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetTrainerSelectionIndex()").unwrap(),
        2
    );
    s.run("SelectTrainerService(9)").unwrap(); // past the 5 rows: clears
    assert_eq!(
        s.eval::<i64>("return GetTrainerSelectionIndex()").unwrap(),
        0
    );

    assert!(!s.take_trainer_close());
    s.run("CloseTrainer()").unwrap();
    assert!(s.take_trainer_close());
    assert!(!s.take_trainer_close(), "drained");
}

/// `IsTradeskillTrainer` tests type 2 (`0x4d8ea0`); `IsTalentTrainer` answers 1 with no trainer
/// open (`0x4d8edb`, the zero guid) or at type 1 (`0x4d8ee4`).
#[test]
fn tradeskill_and_talent_flags() {
    let mut s = UiScript::new().unwrap();
    assert!(s
        .eval::<bool>("return IsTradeskillTrainer() == nil and IsTalentTrainer() == 1")
        .unwrap());

    s.set_trainer(Some(trainer()));
    assert!(s
        .eval::<bool>("return IsTradeskillTrainer() == nil and IsTalentTrainer() == nil")
        .unwrap());

    // One "Recipes" group, so its service is row 2.
    let mut recipe = svc(
        2743,
        "Copper Chain Pants",
        TrainerServiceCategory::Unavailable,
        2,
        "Recipes",
    );
    recipe.is_trade_skill = true;
    s.set_trainer(Some(TrainerState {
        guid: 0,
        greeting: String::new(),
        trainer_type: 2,
        groups: Vec::new(),
        services: vec![recipe],
    }));
    assert!(s
        .eval::<bool>("return IsTradeskillTrainer() == 1 and IsTalentTrainer() == nil")
        .unwrap());
    assert!(s
        .eval::<bool>("return IsTrainerServiceTradeSkill(2) == 1")
        .unwrap());

    s.set_trainer(Some(TrainerState {
        guid: 0,
        greeting: String::new(),
        trainer_type: 1,
        groups: Vec::new(),
        services: vec![svc(
            33,
            "Riding",
            TrainerServiceCategory::Available,
            762,
            "Riding",
        )],
    }));
    assert!(s
        .eval::<bool>("return IsTalentTrainer() == 1 and IsTradeskillTrainer() == nil")
        .unwrap());

    // A class trainer answers nil, and the close answers 1 again.
    s.set_trainer(Some(trainer()));
    assert!(s.eval::<bool>("return IsTalentTrainer() == nil").unwrap());
    s.set_trainer(None);
    assert!(s.eval::<bool>("return IsTalentTrainer() == 1").unwrap());
}

/// The row comparators compare names and ranks with `SStrCmp` (`0x64a480` → `_strncmp`), byte for
/// byte (`0x4d86db`, `0x4d86fe`): at a warlock trainer "Curse of Weakness" sorts before "Curse of
/// the Elements", `W` (0x57) below `t` (0x74). The headers keep `SStrCmpI` (`0x4d7c12`), so
/// "affliction" sorts before "Demonology".
#[test]
fn row_names_compare_byte_for_byte_and_headers_ignore_case() {
    let curse = |id: u32, name: &str, rank: &str| {
        let mut s = svc(
            id,
            name,
            TrainerServiceCategory::Available,
            355,
            "affliction",
        );
        s.level_req = 32;
        s.subtext = Some(rank.into());
        s
    };
    let services = vec![
        svc(
            706,
            "Demon Armor",
            TrainerServiceCategory::Available,
            354,
            "Demonology",
        ),
        curse(1490, "Curse of the Elements", "Rank 1"),
        curse(7646, "Curse of Weakness", "Rank 4"),
    ];
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(TrainerState {
        guid: 0,
        greeting: String::new(),
        trainer_type: 0,
        groups: Vec::new(),
        services,
    }));
    let got: Vec<String> = visible(&mut s).into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        got,
        [
            "affliction",
            "Curse of Weakness",
            "Curse of the Elements",
            "Demonology",
            "Demon Armor",
        ]
    );
}

#[test]
fn unresolved_skill_line_is_dropped() {
    let mut s = UiScript::new().unwrap();
    let mut t = trainer();
    t.services.push(svc(
        999,
        "Orphan Spell",
        TrainerServiceCategory::Available,
        0,
        "",
    ));
    s.set_trainer(Some(t));
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 5);
}

/// The reference stores the selection as a spell id (`0x4d74f0`) and scans the whole array for it
/// (`0x4d7520`), so a hidden selection reads past the visible count.
#[test]
fn the_selection_follows_its_service_and_lands_in_the_tail_when_it_is_hidden() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    let sel = |s: &mut UiScript| s.eval::<i64>("return GetTrainerSelectionIndex()").unwrap();
    let shown = |s: &mut UiScript| s.eval::<i64>("return GetNumTrainerServices()").unwrap();
    let at = |s: &mut UiScript, i: i64| {
        s.eval::<String>(&format!("return (GetTrainerServiceInfo({i})) or ''"))
            .unwrap()
    };

    s.run("SelectTrainerService(5)").unwrap();
    assert_eq!(sel(&mut s), 5);
    assert_eq!(at(&mut s, 5), "Bloodrage");

    // Hiding Cleave moves Bloodrage up to row 4.
    s.run("SetTrainerServiceTypeFilter('used', 0)").unwrap();
    assert_eq!(at(&mut s, 4), "Bloodrage");
    assert_eq!(
        sel(&mut s),
        4,
        "it followed its service, it did not stay put"
    );

    // Row 3 is now the Fury header.
    s.run("CollapseTrainerSkillLine(3)").unwrap();
    let folded = sel(&mut s);
    assert!(
        folded > shown(&mut s),
        "a folded-away selection reads past the visible count, not as row {folded} of {}",
        shown(&mut s)
    );
    s.run("ExpandTrainerSkillLine(3)").unwrap();
    assert_eq!(sel(&mut s), 4, "and back on screen when the group unfolds");

    // Learned, it comes back gray and the filter hides it.
    let mut learned = trainer();
    learned.services[2].category = TrainerServiceCategory::Used;
    s.set_trainer(Some(learned));
    assert!(
        sel(&mut s) > shown(&mut s),
        "the learned spell is off screen, not at some live row"
    );

    let mut shorter = trainer();
    shorter.services.remove(2);
    s.set_trainer(Some(shorter));
    assert_eq!(sel(&mut s), 0);

    s.set_trainer(Some(trainer()));
    s.run("SelectTrainerService(1)").unwrap();
    assert_eq!(sel(&mut s), 0, "row 1 is the Arms header");
}

/// The reference's list builder re-selects a header (`0x4d7560`, `0x4d7b42`), so a re-opened
/// trainer never keeps the last visit's selection.
#[test]
fn a_new_list_packet_clears_the_selection() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    s.run("SelectTrainerService(5)").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetTrainerSelectionIndex()").unwrap(),
        5
    );
    s.reset_trainer_list_state(0);
    s.set_trainer(Some(trainer()));
    assert_eq!(
        s.eval::<i64>("return GetTrainerSelectionIndex()").unwrap(),
        0,
        "the same spell is still in the list, and it is still not selected"
    );
}

#[test]
fn clearing_empties_and_resets_selection() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    s.run("SelectTrainerService(2)").unwrap();
    s.set_trainer(None);
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 0);
    assert_eq!(
        s.eval::<i64>("return GetTrainerSelectionIndex()").unwrap(),
        0
    );
}

/// Creature 957's 19 vmangos `npc_trainer` rows, in the order the reference's builder `0x4d7560`
/// and finalizer `0x4d8410` give them at type 2: the `SKILL_STEP` learn row first under its own
/// header, `reqLevel` inert, the recipes by `reqSkillValue` then name.
#[test]
fn tradeskill_trainer_matches_the_emulated_reference_order() {
    // (wire spell, name, reqSkillValue), reversed below so the order must come from the sort.
    let recipes: &[(u32, &str, u32)] = &[
        (2743, "Copper Chain Pants", 1),
        (2754, "Copper Mace", 15),
        (2755, "Copper Axe", 20),
        (3340, "Copper Chain Boots", 20),
        (2756, "Copper Shortsword", 25),
        (3341, "Rough Grinding Stone", 25),
        (9984, "Copper Claymore", 30),
        (8881, "Copper Dagger", 30),
        (2744, "Copper Battle Axe", 35),
        (3299, "Copper Chain Belt", 35),
        (3342, "Runed Copper Gauntlets", 40),
        (3343, "Runed Copper Pants", 45),
        (2746, "Coarse Sharpening Stone", 65),
        (7409, "Coarse Weightstone", 65),
        (3118, "Heavy Copper Maul", 65),
        (3300, "Runed Copper Belt", 70),
        (2747, "Thick War Axe", 70),
        (3344, "Coarse Grinding Stone", 75),
    ];
    let mut services: Vec<TrainerService> = recipes
        .iter()
        .map(|&(id, name, req)| {
            // Group key 2, `TRADESKILL_SERVICE_LEARN`: the wire spell has no `SKILL_STEP` effect.
            let mut s = svc(id, name, TrainerServiceCategory::Unavailable, 2, "Recipes");
            s.subtext = None;
            s.level_req = 0;
            s.skill_req = Some(TrainerSkillReq {
                name: "Blacksmithing".into(),
                rank: req,
                met: false,
            });
            s
        })
        .collect();
    services.reverse();
    // Group key 1, `TRADESKILL_SERVICE_STEP`: spell 2020 has effect 44, `SKILL_STEP`.
    let mut learn = svc(
        2020,
        "Apprentice Blacksmith",
        TrainerServiceCategory::Available,
        1,
        "Development Skills",
    );
    learn.subtext = None;
    learn.level_req = 5; // inert at type 2
    learn.prof_first_rank = true;
    services.insert(services.len() / 2, learn);

    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(TrainerState {
        guid: 0,
        greeting: "Care to learn how to turn the ore that you find into weapons?".into(),
        trainer_type: 2,
        groups: Vec::new(),
        services,
    }));

    let rows = visible(&mut s);
    let expected: Vec<(&str, &str)> = vec![
        ("Development Skills", "header"),
        ("Apprentice Blacksmith", "available"),
        ("Recipes", "header"),
        ("Copper Chain Pants", "unavailable"),
        ("Copper Mace", "unavailable"),
        ("Copper Axe", "unavailable"),
        ("Copper Chain Boots", "unavailable"),
        ("Copper Shortsword", "unavailable"),
        ("Rough Grinding Stone", "unavailable"),
        ("Copper Claymore", "unavailable"),
        ("Copper Dagger", "unavailable"),
        ("Copper Battle Axe", "unavailable"),
        ("Copper Chain Belt", "unavailable"),
        ("Runed Copper Gauntlets", "unavailable"),
        ("Runed Copper Pants", "unavailable"),
        ("Coarse Sharpening Stone", "unavailable"),
        ("Coarse Weightstone", "unavailable"),
        ("Heavy Copper Maul", "unavailable"),
        ("Runed Copper Belt", "unavailable"),
        ("Thick War Axe", "unavailable"),
        ("Coarse Grinding Stone", "unavailable"),
    ];
    let got: Vec<(&str, &str)> = rows.iter().map(|(n, t)| (n.as_str(), t.as_str())).collect();
    assert_eq!(got, expected);
}

/// At a mount trainer the known group's header sorts first (`0x4d7b90`), the rest by name.
#[test]
fn mount_trainer_folds_known_services_into_my_talents() {
    let known = |id: u32, name: &str| {
        svc(
            id,
            name,
            TrainerServiceCategory::Used,
            TRAINER_GROUP_KNOWN,
            "My Talents",
        )
    };
    let services = vec![
        svc(
            33,
            "Riding",
            TrainerServiceCategory::Available,
            762,
            "Riding",
        ),
        known(6648, "Tiger Riding"),
        svc(
            824,
            "Horse Riding",
            TrainerServiceCategory::Unavailable,
            148,
            "Horse Riding",
        ),
        svc(
            8394,
            "Ram Riding",
            TrainerServiceCategory::Available,
            152,
            "Ram Riding",
        ),
    ];
    let mut s = UiScript::new().unwrap();
    // The header reads its text by name (`0x4d8b11`), not off a service.
    s.run("KNOWN_TALENTS_HEADER = 'My Talents'").unwrap();
    s.set_trainer(Some(TrainerState {
        guid: 0,
        greeting: "Ready to ride?".into(),
        trainer_type: 1,
        groups: Vec::new(),
        services,
    }));

    let got: Vec<String> = visible(&mut s).into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        got,
        [
            "My Talents",
            "Tiger Riding",
            "Horse Riding",
            "Horse Riding",
            "Ram Riding",
            "Ram Riding",
            "Riding",
            "Riding",
        ],
        "the -1 group leads; every other header is name-ordered"
    );
    assert!(s.eval::<bool>("return IsTalentTrainer() == 1").unwrap());
    assert!(s
        .eval::<bool>("return IsTradeskillTrainer() == nil")
        .unwrap());
}

/// Only at type 1 is the state byte a sort key (`0x4d8850`).
#[test]
fn talent_order_sorts_on_state_within_a_group() {
    let one = |id: u32, name: &str, cat: TrainerServiceCategory| svc(id, name, cat, 762, "Riding");
    let services = vec![
        one(3, "Cee", TrainerServiceCategory::Available),
        one(1, "Aye", TrainerServiceCategory::Used),
        one(2, "Bee", TrainerServiceCategory::Unavailable),
    ];
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(TrainerState {
        guid: 0,
        greeting: String::new(),
        trainer_type: 1,
        groups: Vec::new(),
        services: services.clone(),
    }));
    let got: Vec<String> = visible(&mut s).into_iter().map(|(n, _)| n).collect();
    assert_eq!(got, ["Riding", "Cee", "Bee", "Aye"]);

    // At a class trainer the same three sort by name: equal levels, no skill gates.
    s.set_trainer(Some(TrainerState {
        guid: 0,
        greeting: String::new(),
        trainer_type: 0,
        groups: Vec::new(),
        services,
    }));
    let got: Vec<String> = visible(&mut s).into_iter().map(|(n, _)| n).collect();
    assert_eq!(got, ["Riding", "Aye", "Bee", "Cee"]);
}

/// The mask commits fire `TRAINER_UPDATE` inside the verb, after the re-filter (`0x4d8410`, then
/// `0x4d8ca0`/`0x4d8ce0`): a handler reads the filtered tree before the next Lua line runs.
#[test]
fn the_filter_and_collapse_verbs_fire_trainer_update_in_place() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    crate::script::tick::heard::listen(&s, &["TRAINER_UPDATE"], Some("GetNumTrainerServices()"));
    let seen: Vec<String> = s
        .eval(
            "SetTrainerServiceTypeFilter('used', 0) local h = HEARD HEARD = {} \
             CollapseTrainerSkillLine(1) table.insert(h, HEARD[1]) return h",
        )
        .unwrap();
    assert_eq!(seen, ["TRAINER_UPDATE 4", "TRAINER_UPDATE 3"]);
}

/// `OpenTrainer` (`0x4d8cf0`) asks for the player's own list: it hands the host its call, which
/// holds the player, and takes no argument.
#[test]
fn open_trainer_calls_for_the_own_list() {
    let mut s = UiScript::new().unwrap();
    s.keep_calls();
    s.run("OpenTrainer()").unwrap();
    assert_eq!(s.take_trainer_calls(), vec![TrainerCall::OpenOwn]);
    assert!(
        !s.take_trainer_close(),
        "the close waits on the player, which the host sees"
    );
}

/// `IsTrainerServiceSkillStep` (`0x4d9dd0`) and `GetTrainerServiceStepIncrease` (`0x4da030`).
#[test]
fn skill_step_rows_answer_their_flag_and_their_increase_text() {
    let mut s = UiScript::new().unwrap();
    for (call, usage) in [
        (
            "IsTrainerServiceSkillStep()",
            "Usage: IsTrainerServiceSkillStep(index)",
        ),
        (
            "IsTrainerServiceSkillStep('x')",
            "Usage: IsTrainerServiceSkillStep(index)",
        ),
        (
            "GetTrainerServiceStepIncrease(nil)",
            "Usage: GetTrainerServiceStepIncrease(index)",
        ),
        (
            "GetTrainerServiceStepIncrease({})",
            "Usage: GetTrainerServiceStepIncrease(index)",
        ),
    ] {
        let err = s.run(call).expect_err(call).to_string();
        assert!(err.contains(usage), "{call}: {err}");
    }
    let mut t = trainer();
    // Rows: 1 H:Arms, 2 Heroic Strike (available), 3 Cleave (used), 4 H:Fury, 5 Bloodrage.
    t.services[0].skill_step = true;
    t.services[0].step_increase = Some("Increases potential in Arms by 75".into());
    t.services[1].skill_step = true;
    t.services[1].step_increase = Some("Increases potential in Arms by 75".into());
    s.set_trainer(Some(t));
    let nil =
        |s: &mut UiScript, call: &str| s.eval::<bool>(&format!("return {call} == nil")).unwrap();
    assert_eq!(
        s.eval::<i64>("return IsTrainerServiceSkillStep(2)")
            .unwrap(),
        1
    );
    assert_eq!(
        s.eval::<String>("return type(IsTrainerServiceSkillStep(2))")
            .unwrap(),
        "number"
    );
    // A used row still is one; a row without the effect, a header and a miss are not.
    assert_eq!(
        s.eval::<i64>("return IsTrainerServiceSkillStep(3)")
            .unwrap(),
        1
    );
    for row in [5, 1, 4, 0, -1, 99] {
        assert!(
            nil(&mut s, &format!("IsTrainerServiceSkillStep({row})")),
            "{row}"
        );
    }
    assert_eq!(
        s.eval::<String>("return GetTrainerServiceStepIncrease(2)")
            .unwrap(),
        "Increases potential in Arms by 75"
    );
    // A used row (state 2) refuses; so do the rest.
    for row in [3, 5, 1, 0, 99] {
        assert!(
            nil(&mut s, &format!("GetTrainerServiceStepIncrease({row})")),
            "{row}"
        );
    }
}

/// The skill-line filter: `GetTrainerSkillLines` (`0x4da660`), `GetTrainerSkillLineFilter`
/// (`0x4da590`) and `SetTrainerSkillLineFilter` (`0x4da3f0`), over the header groups in order.
#[test]
fn the_skill_line_filter_hides_whole_groups_and_reports_them() {
    let mut s = UiScript::new().unwrap();
    // No trainer: no groups, no values, and "every group is shown".
    assert_eq!(
        s.eval::<i64>("local n = function(...) return arg.n end return n(GetTrainerSkillLines())",)
            .unwrap(),
        0
    );
    assert_eq!(
        s.eval::<i64>("return GetTrainerSkillLineFilter(0)")
            .unwrap(),
        1
    );
    let mut t = trainer();
    // Fury's line has no `SkillLine.dbc` row.
    t.services[2].line_name = None;
    s.set_trainer(Some(t));
    // Groups, in header order: Arms, Fury. One value per group, nil where the line has no row.
    let (n, first, second): (i64, Option<String>, Option<String>) = s
        .eval(
            "local n = function(...) return arg.n, arg[1], arg[2] end \
             return n(GetTrainerSkillLines())",
        )
        .unwrap();
    assert_eq!((n, first.as_deref(), second), (2, Some("Arms"), None));

    let shown = |s: &mut UiScript, n: i64| -> bool {
        s.eval::<Option<i64>>(&format!("return GetTrainerSkillLineFilter({n})"))
            .unwrap()
            .is_some()
    };
    for n in [0, -3, 1, 2] {
        assert!(shown(&mut s, n), "{n} on a fresh list");
    }
    assert_eq!(visible(&mut s).len(), 5);

    // Clearing group 2 hides Fury and its row, header included.
    s.run("SetTrainerSkillLineFilter(2, 0)").unwrap();
    assert!(shown(&mut s, 1) && !shown(&mut s, 2));
    assert!(!shown(&mut s, 0), "not every group is shown");
    assert_eq!(
        visible(&mut s)
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["Arms", "Heroic Strike", "Cleave"]
    );
    // Adding it back with a nonzero `on`.
    s.run("SetTrainerSkillLineFilter(2, 1)").unwrap();
    assert!(shown(&mut s, 0));
    assert_eq!(visible(&mut s).len(), 5);
    // Exclusive: only the named group.
    s.run("SetTrainerSkillLineFilter(2, 1, 1)").unwrap();
    assert!(!shown(&mut s, 1) && shown(&mut s, 2));
    assert_eq!(
        visible(&mut s)
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["Fury", "Bloodrage"]
    );
    // An index below 1 shows every group, and needs no `on`.
    s.run("SetTrainerSkillLineFilter(0)").unwrap();
    assert!(shown(&mut s, 0));
    assert_eq!(visible(&mut s).len(), 5);
    // A new list shows every group again.
    s.run("SetTrainerSkillLineFilter(1, 0)").unwrap();
    s.reset_trainer_list_state(0);
    assert!(shown(&mut s, 0));
}

/// The three functions' errors, in the exe's order and words.
#[test]
fn the_skill_line_filter_raises_the_exes_errors() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    let raises = |s: &mut UiScript, call: &str, text: &str| {
        let err = s.run(call).expect_err(call).to_string();
        assert!(err.contains(text), "{call}: {err}");
    };
    let set_usage = r"Usage: SetTrainerSkillLineFilter(index [, on\off, exclusive])";
    raises(&mut s, "SetTrainerSkillLineFilter()", set_usage);
    raises(&mut s, "SetTrainerSkillLineFilter('x', 1)", set_usage);
    raises(&mut s, "SetTrainerSkillLineFilter({})", set_usage);
    // Past the group count (2 groups), before the `on` check.
    raises(
        &mut s,
        "SetTrainerSkillLineFilter(3)",
        "Bad skill line in SetTrainerSkillLineFilter",
    );
    raises(
        &mut s,
        "SetTrainerSkillLineFilter(3, 1)",
        "Bad skill line in SetTrainerSkillLineFilter",
    );
    raises(
        &mut s,
        "SetTrainerSkillLineFilter(1)",
        "Missing on//off parameter in SetTrainerSkillLineFilter",
    );
    raises(
        &mut s,
        "SetTrainerSkillLineFilter(1, 'x')",
        "Missing on//off parameter in SetTrainerSkillLineFilter",
    );
    raises(
        &mut s,
        "GetTrainerSkillLineFilter()",
        "Usage: GetTrainerSkillLineFilter(index)",
    );
    raises(
        &mut s,
        "GetTrainerSkillLineFilter(nil)",
        "Usage: GetTrainerSkillLineFilter(index)",
    );
    raises(
        &mut s,
        "GetTrainerSkillLineFilter(3)",
        "Bad skill line in GetTrainerSkillLineFilter",
    );
    // Nothing changed: the refused calls left the mask alone.
    assert_eq!(
        s.eval::<i64>("return GetTrainerSkillLineFilter(0)")
            .unwrap(),
        1
    );
}

/// Each change to the skill-line filter signals `TRAINER_UPDATE` through the store routine
/// (`0x4d8cb0`), also when the mask did not move.
#[test]
fn setting_the_skill_line_filter_signals_trainer_update() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    s.run(
        "n = 0 local f = CreateFrame('Frame') f:RegisterEvent('TRAINER_UPDATE') \
         f:SetScript('OnEvent', function() n = n + 1 end)",
    )
    .unwrap();
    s.run("SetTrainerSkillLineFilter(1, 1)").unwrap();
    assert_eq!(s.eval::<i64>("return n").unwrap(), 1);
    s.run("SetTrainerSkillLineFilter(0)").unwrap();
    assert_eq!(s.eval::<i64>("return n").unwrap(), 2);
    let _ = s.run("SetTrainerSkillLineFilter(9, 1)");
    assert_eq!(
        s.eval::<i64>("return n").unwrap(),
        2,
        "a refusal signals nothing"
    );
}

/// The whole row array as `(visible count, rows)`: each service by its spell id, a header as -1.
fn array(s: &mut UiScript) -> (i64, Vec<i64>) {
    let visible = s.eval::<i64>("return GetNumTrainerServices()").unwrap();
    let rows = s
        .eval::<Vec<i64>>(
            r#"
            local out, i = {}, 1
            while GetTrainerServiceInfo(i) ~= nil or GetTrainerServiceIcon(i) ~= nil do
                local icon = GetTrainerServiceIcon(i)
                table.insert(out, icon and tonumber((string.gsub(icon, ".*_", ""))) or -1)
                i = i + 1
            end
            return out
            "#,
        )
        .unwrap();
    (visible, rows)
}

/// The finalizer `0x4d8410` sorts the one persistent row array with the CRT `qsort` (`0x4d857e`)
/// from the order it left, every group's rows and the hidden ones together, so the rows its
/// comparator ties (a service with no spell record, `0x4d868d`; two hidden rows, `0x4d85e0`)
/// land where that history puts them. Each expected array is the client's own: `0x4d8410` run in
/// the emulator over the same list, from the builder's layout (the services in list order, then
/// the header rows), under the same filters in turn.
#[test]
fn every_finalize_sorts_the_whole_array_from_its_last_order() {
    for trainer_type in [0, 2] {
        let row = |id: u32, name: Option<&str>, cat, key: u32, group: &str| {
            let mut s = svc(id, "", cat, key, group);
            s.name = name.map(Into::into);
            s.subtext = name.map(|_| "Rank 1".into());
            s.level_req = 1;
            s
        };
        use TrainerServiceCategory::{Available as A, Unavailable as U, Used};
        let services = vec![
            row(100, Some("Zeta"), A, 26, "Arms"),
            row(101, None, A, 26, "Arms"),
            row(102, Some("Alpha"), A, 26, "Arms"),
            row(103, None, A, 26, "Arms"),
            row(106, Some("Alpha"), A, 26, "Arms"),
            row(104, Some("Beta"), Used, 256, "Fury"),
            row(105, Some("Gamma"), U, 256, "Fury"),
        ];
        let mut s = UiScript::new().unwrap();
        s.reset_trainer_list_state(trainer_type);
        s.set_trainer(Some(TrainerState {
            guid: 0,
            greeting: String::new(),
            trainer_type,
            groups: Vec::new(),
            services,
        }));
        let mut seen = vec![array(&mut s)];
        for lua in [
            "SetTrainerServiceTypeFilter('unavailable', 0)",
            "SetTrainerServiceTypeFilter('used', 1)",
            "SetTrainerServiceTypeFilter('unavailable', 1)",
            "CollapseTrainerSkillLine(1)",
            "ExpandTrainerSkillLine(0)",
        ] {
            s.run(lua).unwrap();
            seen.push(array(&mut s));
        }
        assert_eq!(
            seen,
            [
                (8, vec![-1, 102, 103, 101, 106, 100, -1, 105, 104]),
                (6, vec![-1, 103, 101, 102, 106, 100, 105, 104, -1]),
                (8, vec![-1, 101, 102, 103, 106, 100, -1, 104, 105]),
                (9, vec![-1, 102, 103, 101, 106, 100, -1, 104, 105]),
                (4, vec![-1, -1, 104, 105, 100, 101, 103, 102, 106]),
                (9, vec![-1, 102, 106, 100, 101, 103, -1, 104, 105]),
            ],
            "trainer type {trainer_type}"
        );
    }
}

/// `GetTrainerServiceNumAbilityReq` (`0x4d9600`) counts the slots whose id is above 0
/// (`0x4d9653`); `GetTrainerServiceAbilityReq(index, i)` (`0x4d96e0`) reads slot `i - 1` in place
/// (`0x4d97a5`), so a gap answers nil and a slot past the count still answers, and a requirement
/// with no `Spell.dbc` record names nothing (`0x4d97c8`), its flag left at 1 (`0x4d9761`).
#[test]
fn ability_reqs_read_their_slots_in_place() {
    let mut s = UiScript::new().unwrap();
    let mut t = trainer();
    t.services[0].ability_reqs = vec![
        TrainerAbilityReq {
            spell_id: 0,
            name: None,
            met: true,
        },
        TrainerAbilityReq {
            spell_id: 78,
            name: Some("Heroic Strike (Rank 1)".into()),
            met: false,
        },
        TrainerAbilityReq {
            spell_id: 99999,
            name: None,
            met: true,
        },
    ];
    s.set_trainer(Some(t));
    let req = |s: &mut UiScript, i: i64| {
        s.eval::<(Option<String>, Option<i64>)>(&format!(
            "return GetTrainerServiceAbilityReq(2, {i})"
        ))
        .unwrap()
    };
    assert_eq!(
        s.eval::<i64>("return GetTrainerServiceNumAbilityReq(2)")
            .unwrap(),
        2
    );
    assert_eq!(req(&mut s, 1), (None, Some(1)));
    assert_eq!(
        req(&mut s, 2),
        (Some("Heroic Strike (Rank 1)".into()), None)
    );
    assert_eq!(req(&mut s, 3), (None, Some(1)));
    assert_eq!(req(&mut s, 4), (None, Some(1)));
}

/// The state filter as `(available, unavailable, used)`.
fn type_filter(s: &mut UiScript) -> (bool, bool, bool) {
    let on = |s: &mut UiScript, kind: &str| {
        s.eval::<Option<i64>>(&format!("return GetTrainerServiceTypeFilter('{kind}')"))
            .unwrap()
            .is_some()
    };
    (on(s, "available"), on(s, "unavailable"), on(s, "used"))
}

/// `SetTrainerServiceTypeFilter("type", on [, exclusive])` (`0x4da260`): `"all"` sets the mask to
/// 7 and reads no second argument (`0x4da2a9`); a nonzero number third makes the one bit the whole
/// mask (`0x4da359`-`0x4da364`); `on` and `exclusive` are truncated (`0x40a2b0`), so 0.5 is off;
/// both names compare without case (`0x64a4c0`, `0x4da390`). Every path commits through
/// `0x4d8c90`, which fires `TRAINER_UPDATE`.
#[test]
fn the_type_filter_takes_all_the_exclusive_mode_and_a_truncated_on() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    s.reset_trainer_list_state(0);
    s.run(
        "n = 0 local f = CreateFrame('Frame') f:RegisterEvent('TRAINER_UPDATE') \
         f:SetScript('OnEvent', function() n = n + 1 end)",
    )
    .unwrap();
    assert_eq!(
        type_filter(&mut s),
        (true, true, false),
        "the list's mask 3"
    );

    s.run("SetTrainerServiceTypeFilter('all')").unwrap();
    assert_eq!(type_filter(&mut s), (true, true, true));
    assert_eq!(visible(&mut s).len(), 5, "every row shows");
    assert_eq!(s.eval::<i64>("return n").unwrap(), 1, "the commit fires");

    s.run("SetTrainerServiceTypeFilter('used', 1, 1)").unwrap();
    assert_eq!(type_filter(&mut s), (false, false, true), "exclusive");
    s.run("SetTrainerServiceTypeFilter('ALL', 0)").unwrap();
    assert_eq!(
        type_filter(&mut s),
        (true, true, true),
        "\"all\" without case, its second argument never read"
    );

    s.run("SetTrainerServiceTypeFilter('Available', 0.5)")
        .unwrap();
    assert_eq!(
        type_filter(&mut s),
        (false, true, true),
        "0.5 truncates to 0"
    );
    s.run("SetTrainerServiceTypeFilter('available', '1', 0.5)")
        .unwrap();
    assert_eq!(
        type_filter(&mut s),
        (true, true, true),
        "a numeric string is a number, and 0.5 exclusive truncates to 0: added"
    );
    s.run("SetTrainerServiceTypeFilter('unavailable', -0.9)")
        .unwrap();
    assert_eq!(
        type_filter(&mut s),
        (true, false, true),
        "-0.9 truncates to 0"
    );
    s.run("SetTrainerServiceTypeFilter('unavailable', 2, true)")
        .unwrap();
    assert_eq!(
        type_filter(&mut s),
        (true, true, true),
        "a boolean third argument is no number: added"
    );
    s.run("SetTrainerServiceTypeFilter('used', 1, '1')")
        .unwrap();
    assert_eq!(type_filter(&mut s), (false, false, true));
    assert_eq!(s.eval::<i64>("return n").unwrap(), 8);
}

/// `0x4da260`'s three refusals, in its order and words (`0x84ab60`, `0x84ab2c`, `0x84aaf0`): the
/// name must be a string or a number (`0x6f3510`), then one the mapper `0x4da390` knows, then `on`
/// must be a number (`0x6f34d0`). A refusal commits nothing and fires nothing.
#[test]
fn the_type_filter_raises_the_exes_errors() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    s.reset_trainer_list_state(0);
    s.run(
        "n = 0 local f = CreateFrame('Frame') f:RegisterEvent('TRAINER_UPDATE') \
         f:SetScript('OnEvent', function() n = n + 1 end)",
    )
    .unwrap();
    let raises = |s: &mut UiScript, call: &str, text: &str| {
        let err = s.run(call).expect_err(call).to_string();
        assert!(err.contains(text), "{call}: {err}");
    };
    let usage = r#"Usage: SetTrainerServiceTypeFilter("type" [, on\off, exclusive])"#;
    let bad = "Bad service type in SetTrainerServiceTypeFilter";
    let missing = "Missing on//off parameter in SetTrainerServiceTypeFilter";
    raises(&mut s, "SetTrainerServiceTypeFilter()", usage);
    raises(&mut s, "SetTrainerServiceTypeFilter(nil, 1)", usage);
    raises(&mut s, "SetTrainerServiceTypeFilter({}, 1)", usage);
    raises(&mut s, "SetTrainerServiceTypeFilter('bogus', 1)", bad);
    raises(&mut s, "SetTrainerServiceTypeFilter('bogus')", bad);
    raises(&mut s, "SetTrainerServiceTypeFilter(1, 1)", bad);
    raises(&mut s, "SetTrainerServiceTypeFilter('used')", missing);
    raises(&mut s, "SetTrainerServiceTypeFilter('used', true)", missing);
    raises(&mut s, "SetTrainerServiceTypeFilter('used', 'x')", missing);
    assert_eq!(type_filter(&mut s), (true, true, false), "nothing moved");
    assert_eq!(s.eval::<i64>("return n").unwrap(), 0, "nothing fired");
}

/// At a mount trainer the list builds the known group first, header 0 with key -1
/// (`0x4d7609`-`0x4d7697`), whether or not a service is known: with no member its header is
/// hidden (`0x4d8447`), but it is a skill line, the first, named by `KNOWN_TALENTS_HEADER`
/// (`0x4d8b11`) and with no `SkillLine.dbc` row.
#[test]
fn a_mount_trainer_always_builds_the_known_group_first() {
    let mut s = UiScript::new().unwrap();
    s.run("KNOWN_TALENTS_HEADER = 'My Talents'").unwrap();
    s.set_trainer(Some(TrainerState {
        guid: 0,
        greeting: String::new(),
        trainer_type: 1,
        groups: Vec::new(),
        services: vec![
            svc(
                33,
                "Riding",
                TrainerServiceCategory::Available,
                762,
                "Riding",
            ),
            svc(
                8394,
                "Ram Riding",
                TrainerServiceCategory::Available,
                152,
                "Ram Riding",
            ),
        ],
    }));
    let (n, first, second, third): (i64, Option<String>, Option<String>, Option<String>) = s
        .eval(
            "local n = function(...) return arg.n, arg[1], arg[2], arg[3] end \
             return n(GetTrainerSkillLines())",
        )
        .unwrap();
    assert_eq!(
        (n, first, second.as_deref(), third.as_deref()),
        (3, None, Some("Ram Riding"), Some("Riding"))
    );
    assert_eq!(
        visible(&mut s)
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["Ram Riding", "Ram Riding", "Riding", "Riding"],
        "the empty known group shows no header"
    );
    // The hidden header sits in the array's tail, past the visible count.
    let (name, kind): (String, String) = s
        .eval("local n, _, t = GetTrainerServiceInfo(5) return n, t")
        .unwrap();
    assert_eq!((name.as_str(), kind.as_str()), ("My Talents", "header"));
    // The skill-line indices count it: group 2 is Ram Riding.
    s.run("SetTrainerSkillLineFilter(2, 1, 1)").unwrap();
    assert_eq!(
        visible(&mut s)
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["Ram Riding", "Ram Riding"]
    );
    assert!(s
        .eval::<Option<i64>>("return GetTrainerSkillLineFilter(3)")
        .unwrap()
        .is_none());

    // A class trainer builds no such group.
    s.set_trainer(Some(trainer()));
    assert_eq!(
        s.eval::<i64>("local n = function(...) return arg.n end return n(GetTrainerSkillLines())")
            .unwrap(),
        2
    );
}

/// `GetTrainerServiceTypeFilter("type")` (`0x4da510`) maps the name through `0x4da390`, without
/// case, and raises its two errors: a non-string first (`0x84ac78`), an unknown name (`0x84ac48`).
#[test]
fn the_type_filter_getter_reads_any_case_and_raises_the_exes_errors() {
    let mut s = UiScript::new().unwrap();
    s.set_trainer(Some(trainer()));
    s.reset_trainer_list_state(0);
    assert_eq!(
        s.eval::<(Option<i64>, Option<i64>)>(
            "return GetTrainerServiceTypeFilter('AVAILABLE'), GetTrainerServiceTypeFilter('Used')"
        )
        .unwrap(),
        (Some(1), None)
    );
    let raises = |s: &mut UiScript, call: &str, text: &str| {
        let err = s.run(call).expect_err(call).to_string();
        assert!(err.contains(text), "{call}: {err}");
    };
    let usage = r#"Usage: GetTrainerServiceTypeFilter("type")"#;
    let bad = "Bad service type in GetTrainerServiceTypeFilter";
    raises(&mut s, "GetTrainerServiceTypeFilter()", usage);
    raises(&mut s, "GetTrainerServiceTypeFilter({})", usage);
    raises(&mut s, "GetTrainerServiceTypeFilter('all')", bad);
    raises(&mut s, "GetTrainerServiceTypeFilter(1)", bad);
}
