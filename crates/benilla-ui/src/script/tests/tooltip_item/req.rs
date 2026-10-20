//! The requirement and identity lines: the area and map of a zone-bound item, the known-recipe
//! line, and the honor rank and medal requirements.

use std::collections::HashSet;

use super::{lines_of, script};
use crate::script::*;

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const RED: [f32; 4] = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];

/// Render `id` into `TT` and answer its lines with their colours.
fn render(s: &mut UiScript, id: u32) -> Vec<(String, [f32; 4])> {
    s.run(&format!(
        r#"
        local a = ReqSlot or CreateFrame("Button", "ReqSlot")
        a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = TT or CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        SetItemByIdForTest(tt, {id})
    "#
    ))
    .unwrap();
    lines_of(s)
}

fn texts(lines: &[(String, [f32; 4])]) -> Vec<&str> {
    lines.iter().map(|(t, _)| t.as_str()).collect()
}

/// A zone-bound item names its area (`+0x1c8`, the `AreaTable` name at `+0x2c`) and its map
/// (`+0x1cc`, the `Map` name at `+0x10`) in white, right after `ITEM_SIGNABLE` and before
/// `ITEM_CONJURED` (`0x52bac9`..`0x52bb46`), and neither with `nameOnly` (`0x52bac3`).
#[test]
fn a_zone_bound_item_names_its_area_and_map() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        9500,
        ItemTemplateView {
            name: "Scarab Coffer Key".into(),
            quality: 1,
            flags: 0x2,
            area_name: Some("Ruins of Ahn'Qiraj".into()),
            map_name: Some("Ahn'Qiraj Temple".into()),
            ..Default::default()
        },
    );
    let lines = render(&mut s, 9500);
    assert_eq!(
        texts(&lines),
        [
            "Scarab Coffer Key",
            "Ruins of Ahn'Qiraj",
            "Ahn'Qiraj Temple",
            "[ITEM_CONJURED]"
        ]
    );
    assert_eq!((lines[1].1, lines[2].1), (WHITE, WHITE));
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// An item whose first spell teaches a spell shows `ITEM_SPELL_KNOWN` in red once the player knows
/// the taught spell or a higher rank of it (`0x52d54f`..`0x52d5de`), with any trigger, between the
/// skill line and the required-spell line. A pet-target teach asks the pet's book instead
/// (`0x52d56d`), and with no pet nothing shows.
#[test]
fn already_known_tests_the_taught_spell_and_its_higher_ranks() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let recipe = |pet| ItemTemplateView {
        name: "Recipe: Swiftness Potion".into(),
        quality: 1,
        required_skill: 171,
        required_skill_name: Some("Alchemy".into()),
        required_skill_rank: 60,
        required_spell: 7,
        required_spell_name: Some("Apprentice".into()),
        teaches: Some(TaughtSpell {
            ranks: vec![2335, 2336],
            pet,
        }),
        spell_lines: vec![ItemSpellLine {
            trigger: 0,
            spell_id: 2364,
            text: "Teaches you.".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    s.set_item_template(9501, recipe(false));
    s.set_item_template(9502, recipe(true));
    let known = |lines: &[(String, [f32; 4])]| {
        lines
            .iter()
            .position(|(t, _)| t == "[ITEM_SPELL_KNOWN]")
            .map(|i| (i, lines[i].1))
    };
    assert_eq!(known(&render(&mut s, 9501)), None, "not known yet");
    // The item's own spell in the book is not the taught one.
    s.set_known_spells(HashSet::from([2364]));
    assert_eq!(known(&render(&mut s, 9501)), None, "the item's own spell");
    s.set_known_spells(HashSet::from([2335]));
    let lines = render(&mut s, 9501);
    assert_eq!(
        texts(&lines),
        [
            "Recipe: Swiftness Potion",
            "[MIN_SKILL Alchemy 60]",
            "[ITEM_SPELL_KNOWN]",
            "[REQ_SKILL Apprentice]",
            "[ONUSE] Teaches you.",
        ]
    );
    assert_eq!(known(&lines), Some((2, RED)));
    s.set_known_spells(HashSet::from([2336]));
    assert!(known(&render(&mut s, 9501)).is_some(), "a higher rank");
    // The pet's book decides a pet teach; the player's does not.
    assert_eq!(known(&render(&mut s, 9502)), None, "no pet, no line");
    s.set_pet_book(PetBookState {
        token: Some("PET".into()),
        slots: vec![SpellSlotView {
            spell_id: 2335,
            ..Default::default()
        }],
    });
    assert!(known(&render(&mut s, 9502)).is_some(), "the pet knows it");
    s.set_known_spells(HashSet::new());
    assert_eq!(
        known(&render(&mut s, 9501)),
        None,
        "the pet's book is not the player's"
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// A required honor rank names the rank by the player's side and gender (`0x5efe60`,
/// `PVP_RANK_%d_%d`, `_FEMALE` first), white at or above it (`fields+0x102b`); a required medal
/// reads `PVP_MEDAL<n>`, white when bit `n-1` of `PLAYER_FIELD_PVP_MEDALS` is set (`fields+0x1034`).
/// Both are `ITEM_REQ_SKILL` lines between the required spell and the reputation
/// (`0x52d694`..`0x52d796`).
#[test]
fn a_required_rank_and_medal_name_themselves() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        PVP_RANK_11_1 = "Knight-Lieutenant"
        PVP_RANK_11_1_FEMALE = "Knight-Lieutenant (f)"
        PVP_RANK_11_0 = "Blood Guard"
        PVP_MEDAL1 = "Protector of Stormwind"
    "#,
    )
    .unwrap();
    s.set_item_template(
        9503,
        ItemTemplateView {
            name: "Lieutenant's Silk".into(),
            quality: 3,
            required_spell: 7,
            required_spell_name: Some("Apprentice".into()),
            required_honor_rank: 11,
            required_city_rank: 1,
            required_rep_line: Some("Requires Stormwind - Honored".into()),
            required_rep_faction: 72,
            required_rep_rank: 5,
            ..Default::default()
        },
    );
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            pvp_team: 1,
            sex: 2,
            ..Default::default()
        }),
    );
    s.set_player_req_state(PlayerReqState {
        level: 60,
        honor_rank: 10,
        ..Default::default()
    });
    let lines = render(&mut s, 9503);
    assert_eq!(
        texts(&lines),
        [
            "Lieutenant's Silk",
            "[REQ_SKILL Apprentice]",
            "[REQ_SKILL Knight-Lieutenant]",
            "[REQ_SKILL Protector of Stormwind]",
            "Requires Stormwind - Honored",
        ]
    );
    assert_eq!((lines[2].1, lines[3].1), (RED, RED), "rank 10, no medal");
    s.set_player_req_state(PlayerReqState {
        level: 60,
        honor_rank: 11,
        pvp_medals: 0x1,
        ..Default::default()
    });
    let lines = render(&mut s, 9503);
    assert_eq!((lines[2].1, lines[3].1), (WHITE, WHITE), "rank 11, medal 1");
    // A female player reads the `_FEMALE` title; the Horde side its own digit.
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            pvp_team: 1,
            sex: 3,
            ..Default::default()
        }),
    );
    assert_eq!(
        render(&mut s, 9503)[2].0,
        "[REQ_SKILL Knight-Lieutenant (f)]"
    );
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            pvp_team: 0,
            sex: 3,
            ..Default::default()
        }),
    );
    assert_eq!(render(&mut s, 9503)[2].0, "[REQ_SKILL Blood Guard]");
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The usable gate tests the same medal bit (`0x5eaa89`..`0x5eaaa0`): a player holding it may use
/// the item.
#[test]
fn the_medal_bit_passes_the_usable_gate() {
    let v = ItemTemplateView {
        allowable_class: -1,
        allowable_race: -1,
        required_city_rank: 3,
        ..Default::default()
    };
    let req = |pvp_medals| PlayerReqState {
        level: 60,
        class_id: 1,
        race_id: 1,
        pvp_medals,
        ..Default::default()
    };
    assert!(!item_usable(&v, &req(0x3), |_| false));
    assert!(item_usable(&v, &req(0x4), |_| false));
}

/// The required spell's line and the usable gate both ask whether the player knows the spell
/// (`0x60c740`: every spell the player knows, not only the spellbook's) or a later rank of it
/// (`0x60c8d0`), at `0x52d614`..`0x52d638` and `0x5eaa42`..`0x5eaa51`.
#[test]
fn a_required_spell_is_met_by_any_known_rank() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        9504,
        ItemTemplateView {
            name: "Elixir".into(),
            quality: 1,
            allowable_class: -1,
            allowable_race: -1,
            required_spell: 7,
            required_spell_name: Some("Apprentice".into()),
            required_spell_ranks: vec![7, 8],
            ..Default::default()
        },
    );
    s.set_player_req_state(PlayerReqState {
        level: 60,
        class_id: 1,
        race_id: 1,
        ..Default::default()
    });
    s.set_merchant(Some(MerchantState {
        items: vec![MerchantItem {
            item_id: 9504,
            ..Default::default()
        }],
        ..Default::default()
    }));
    let usable = |s: &mut UiScript| -> bool {
        s.eval::<bool>("local _, _, _, _, _, usable = GetMerchantItemInfo(1) return usable == 1")
            .unwrap()
    };
    let line = |s: &mut UiScript| render(s, 9504)[1].clone();
    assert_eq!(line(&mut s), ("[REQ_SKILL Apprentice]".to_string(), RED));
    assert!(!usable(&mut s));
    s.set_known_spells(HashSet::from([7]));
    assert_eq!(line(&mut s).1, WHITE, "a known spell outside the spellbook");
    assert!(usable(&mut s));
    s.set_known_spells(HashSet::from([8]));
    assert_eq!(line(&mut s).1, WHITE, "a later rank");
    assert!(usable(&mut s));
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}
