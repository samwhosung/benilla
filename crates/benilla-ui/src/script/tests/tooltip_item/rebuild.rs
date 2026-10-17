//! The rebuild a cache answer runs (`CGTooltip::OnCacheAnswer`, `0x52e530`): a shown item tooltip
//! whose build missed a cache builds itself again from the inputs it keeps when the answer lands,
//! with no re-hover; a clear drops the kept inputs, and a later build replaces them.

use std::collections::HashMap;

use super::script as item_script;
use crate::script::*;

const LOCKBOX: u32 = 4632;
const LOCK_ID: u32 = 7;
const KEY: u32 = 5060;
const BAR: u32 = 2840;

/// The tooltip `tt`'s left lines, in order.
fn lines(s: &mut UiScript, tt: &str) -> Vec<String> {
    let n: i64 = s.eval(&format!("return {tt}:NumLines()")).unwrap();
    (1..=n)
        .map(|i| {
            s.eval(&format!(
                "return getglobal('{tt}TextLeft{i}'):GetText() or ''"
            ))
            .unwrap()
        })
        .collect()
}

/// `TT` owned by `Slot`, counting its `OnShow` and `OnTooltipCleared`; the partner offers a
/// lockbox in trade slot 1 whose lock wants the key [`KEY`], which the item cache lacks; the
/// backpack's slot 1 holds a copper bar.
fn fixture() -> UiScript {
    let mut s = item_script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"LOCKED_WITH_ITEM = "[LOCKED_WITH_ITEM %s]"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0)
        a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        shows, cleared = 0, 0
        TT:SetScript("OnShow", function() shows = shows + 1 end)
        TT:SetScript("OnTooltipCleared", function() cleared = cleared + 1 end)
        "#,
    )
    .unwrap();
    s.set_item_template(
        LOCKBOX,
        ItemTemplateView {
            name: "Lockbox".into(),
            quality: 1,
            item_level: 20,
            ..Default::default()
        },
    );
    s.set_item_template(
        BAR,
        ItemTemplateView {
            name: "Copper Bar".into(),
            quality: 1,
            ..Default::default()
        },
    );
    s.set_lock_rows(HashMap::from([(
        LOCK_ID,
        vec![LockSlotView {
            key_type: 1,
            index: KEY,
            skill: 0,
        }],
    )]));
    s.set_lock_openers(LockOpeners::default());
    let mut trade = TradeState::default();
    trade.target.slots[0] = Some(TradeSlotItem {
        item_id: LOCKBOX,
        name: Some("Lockbox".into()),
        count: 1,
        lock_id: LOCK_ID,
        ..Default::default()
    });
    s.set_trade(Some(trade));
    s.set_container(0, Some(backpack((SMITH, None))));
    s
}

const SMITH: u64 = 0x5;
const CHARTER: u32 = 5863;

/// One backpack slot's item object, made by `creator` (a guid, and its name once resolved).
fn held(item_id: u32, name: &str, guid: u64, creator: (u64, Option<&str>)) -> ContainerSlot {
    ContainerSlot {
        item_id,
        guid,
        count: 1,
        quality: Some(1),
        link: Some(format!("|cffffffff|Hitem:{item_id}:0:0:0|h[{name}]|h|r")),
        creator_guid: creator.0,
        creator: creator.1.map(str::to_string),
        ..Default::default()
    }
}

/// The backpack: the copper bar in slot 1, made by `creator`, and a second, uncrafted bar in
/// slot 2.
fn backpack(creator: (u64, Option<&str>)) -> ContainerState {
    ContainerState {
        name: Some("Backpack".into()),
        num_slots: 16,
        slots: HashMap::from([
            (1, held(BAR, "Copper Bar", 0x4000_0000_0000_0001, creator)),
            (2, held(BAR, "Copper Bar", 0x4000_0000_0000_0002, (0, None))),
        ]),
    }
}

fn cleared(s: &mut UiScript) -> i64 {
    s.eval("return cleared").unwrap()
}

fn key_arrives(s: &mut UiScript) {
    s.set_item_template(
        KEY,
        ItemTemplateView {
            name: "Skeleton Key".into(),
            quality: 1,
            ..Default::default()
        },
    );
    s.tick(0.1);
}

/// Stock re-runs no `OnEnter` for the partner's trade slot, so only the rebuild draws the line the
/// key's answer completes (`0x52bf48` asks with the tooltip as the cookie). The rebuild is a fresh
/// build: its clear runs `OnTooltipCleared`, and its show on a shown plate runs no `OnShow`.
#[test]
fn a_trade_targets_key_answer_adds_its_locked_with_line_without_a_rehover() {
    let mut s = fixture();
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetTradeTargetItem(1)")
        .unwrap();
    assert_eq!(lines(&mut s, "TT"), ["Lockbox", "[LOCKED]"]);
    let (shows, cleared): (i64, i64) = s.eval("return shows, cleared").unwrap();
    key_arrives(&mut s);
    assert_eq!(
        lines(&mut s, "TT"),
        ["Lockbox", "[LOCKED]", "[LOCKED_WITH_ITEM Skeleton Key]"]
    );
    let after: (i64, i64) = s.eval("return shows, cleared").unwrap();
    assert_eq!(after, (shows, cleared + 1));
    // The answer is taken once: a later tick builds nothing more.
    s.tick(0.1);
    let again: (i64, i64) = s.eval("return shows, cleared").unwrap();
    assert_eq!(again, after);
    assert!(s.take_errors().is_empty());
}

/// The maker's name answers the name cache (`0x52e1f4`): each build that misses it asks the app,
/// the bag slot's view carries it once the app re-pushes it, and the tooltip that lacked it
/// rebuilds with the line.
#[test]
fn a_made_by_name_that_arrives_adds_its_line() {
    let mut s = fixture();
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 1)")
        .unwrap();
    assert_eq!(lines(&mut s, "TT"), ["Copper Bar"]);
    assert_eq!(
        s.take_item_name_asks(),
        vec![SMITH],
        "the build asks the app"
    );
    s.run("TT:SetBagItem(0, 1)").unwrap();
    assert_eq!(s.take_item_name_asks(), vec![SMITH], "and so does the next");
    s.set_container(0, Some(backpack((SMITH, Some("Smith")))));
    s.tick(0.1);
    assert_eq!(
        lines(&mut s, "TT"),
        ["Copper Bar", "|cff00ff00[CREATED_BY Smith]|r"]
    );
    assert!(s.take_errors().is_empty());
}

/// `ItemRefTooltip:SetHyperlink` keeps its link's inputs like any build. A link carries no maker
/// (`0x5321ad` zeroes the creator), so what it waits on is the item-name cache of a set's members
/// (`0x52df4b`): a member named late gains its line in place.
#[test]
fn an_item_ref_links_set_member_named_late_gains_its_line() {
    let mut s = item_script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"CreateFrame("Frame", "Holder"):SetAllPoints()
        CreateFrame("GameTooltip", "ItemRefTooltip", Holder, "TestTooltipTemplate")"#,
    )
    .unwrap();
    s.set_item_template(
        6303,
        ItemTemplateView {
            name: "Defias Mark".into(),
            quality: 2,
            item_set: 161,
            ..Default::default()
        },
    );
    let set = |boots: Option<&str>| ItemSetView {
        name: "Defias Leather".into(),
        members: vec![
            (6303, Some("Defias Mark".into())),
            (6307, boots.map(str::to_string)),
        ],
        ..Default::default()
    };
    s.set_item_set(161, set(None));
    s.run(
        r#"ItemRefTooltip:SetOwner(Holder, "ANCHOR_PRESERVE")
        ItemRefTooltip:SetHyperlink("item:6303:0:0:0")"#,
    )
    .unwrap();
    let before = lines(&mut s, "ItemRefTooltip");
    assert!(before.contains(&"  Defias Mark".to_string()), "{before:?}");
    assert!(
        !before.contains(&"  Defias Boots".to_string()),
        "{before:?}"
    );
    s.set_item_set(161, set(Some("Defias Boots")));
    s.tick(0.1);
    let after = lines(&mut s, "ItemRefTooltip");
    assert!(after.contains(&"  Defias Boots".to_string()), "{after:?}");
    assert_eq!(after.len(), before.len() + 1, "{after:?}");
    assert!(s.take_errors().is_empty());
}

/// Every clear zeroes the kept entry (`0x5300dd`), so the answer finds nothing to rebuild
/// (`0x52e556`): a plate whose lines were cleared stays empty, and a hidden one stays down.
#[test]
fn a_tooltip_cleared_before_the_answer_stays_cleared() {
    let mut s = fixture();
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetTradeTargetItem(1) TT:ClearLines()")
        .unwrap();
    let cleared: i64 = s.eval("return cleared").unwrap();
    key_arrives(&mut s);
    assert_eq!(lines(&mut s, "TT"), Vec::<String>::new());
    assert_eq!(s.eval::<i64>("return cleared").unwrap(), cleared);

    let mut s = fixture();
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetTradeTargetItem(1) TT:Hide()")
        .unwrap();
    key_arrives(&mut s);
    assert!(!s.eval::<bool>("return TT:IsShown() == 1").unwrap());
    assert_eq!(lines(&mut s, "TT"), Vec::<String>::new());
    assert!(s.take_errors().is_empty());
}

/// The answer's cookie is the tooltip, not the build: a later build overwrote the kept inputs, so
/// the key's late answer rebuilds the copper bar the plate shows by then, not the lockbox. The
/// rebuild is from scratch, so a line an addon added after the build is gone.
#[test]
fn a_tooltip_showing_another_item_by_then_rebuilds_that_item() {
    let mut s = fixture();
    s.run(
        "TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetTradeTargetItem(1)
         TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 1) TT:AddLine('addon line')",
    )
    .unwrap();
    assert_eq!(lines(&mut s, "TT"), ["Copper Bar", "addon line"]);
    let cleared: i64 = s.eval("return cleared").unwrap();
    key_arrives(&mut s);
    assert_eq!(lines(&mut s, "TT"), ["Copper Bar"]);
    assert_eq!(s.eval::<i64>("return cleared").unwrap(), cleared + 1);
    assert!(s.take_errors().is_empty());
}

/// The name cache's answer outlives the build that asked, as every ask does: a plate showing
/// another item by then rebuilds that item, from scratch.
#[test]
fn a_late_name_rebuilds_the_item_the_plate_shows_by_then() {
    let mut s = fixture();
    s.run(
        "TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 1)
         TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 2) TT:AddLine('addon line')",
    )
    .unwrap();
    let before = cleared(&mut s);
    s.set_container(0, Some(backpack((SMITH, Some("Smith")))));
    s.tick(0.1);
    assert_eq!(lines(&mut s, "TT"), ["Copper Bar"]);
    assert_eq!(cleared(&mut s), before + 1);
    assert!(s.take_errors().is_empty());
}

/// An item with no creator asks the name cache nothing (`0x52e1eb`), so the same entry turning up
/// crafted later rebuilds nothing.
#[test]
fn an_item_with_no_creator_waits_for_no_name() {
    let mut s = fixture();
    s.set_container(0, Some(backpack((0, None))));
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 1)")
        .unwrap();
    let before = cleared(&mut s);
    s.set_container(0, Some(backpack((SMITH, Some("Smith")))));
    s.tick(0.1);
    assert_eq!(lines(&mut s, "TT"), ["Copper Bar"]);
    assert_eq!(cleared(&mut s), before);
    assert!(s.take_errors().is_empty());
}

/// A charter's petition record answers the petition cache (`0x52b92f`): its title and owner lines
/// appear in place.
#[test]
fn a_charters_petition_answer_adds_its_lines() {
    let mut s = fixture();
    s.set_item_template(
        CHARTER,
        ItemTemplateView {
            name: "Charter".into(),
            quality: 1,
            flags: 0x2000,
            ..Default::default()
        },
    );
    let charter = |petition: Option<PetitionSlotView>| ContainerState {
        name: Some("Backpack".into()),
        num_slots: 16,
        slots: HashMap::from([(
            1,
            ContainerSlot {
                petition,
                ..held(CHARTER, "Charter", 0x4000_0000_0000_0009, (0, None))
            },
        )]),
    };
    s.set_container(0, Some(charter(None)));
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 1)")
        .unwrap();
    assert_eq!(lines(&mut s, "TT"), ["Charter", "[ITEM_SIGNABLE]"]);
    assert_eq!(
        s.take_petition_asks(),
        vec![0x4000_0000_0000_0009],
        "the build asks the app"
    );
    s.set_container(
        0,
        Some(charter(Some(PetitionSlotView {
            is_charter: false,
            title: "Pact".into(),
            owner: Some("Bob".into()),
        }))),
    );
    s.tick(0.1);
    assert_eq!(
        lines(&mut s, "TT"),
        [
            "Charter",
            "[PETITION_TITLE Pact]",
            "[PETITION_CREATOR Bob]",
            "[ITEM_SIGNABLE]"
        ]
    );
    assert!(
        s.take_petition_asks().is_empty(),
        "an answered build asks nothing"
    );
    assert!(s.take_errors().is_empty());
}

/// A compare's ask takes the tooltip as its cookie with no dedupe (`0x5361d7` pushes 0), so two
/// asks are two callback nodes: the answer rebuilds the item the plate keeps twice, each with its
/// own clear.
#[test]
fn each_compare_ask_rebuilds_the_kept_item_on_the_answer() {
    const OFFER: u32 = 7777;
    let mut s = fixture();
    s.set_merchant(Some(MerchantState {
        items: vec![MerchantItem {
            item_id: OFFER,
            ..Default::default()
        }],
        ..Default::default()
    }));
    s.run(
        "TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 2) TT:AddLine('addon line')
         assert(TT:SetMerchantCompareItem(1) == nil)
         assert(TT:SetMerchantCompareItem(1) == nil)",
    )
    .unwrap();
    let before = cleared(&mut s);
    s.set_item_template(
        OFFER,
        ItemTemplateView {
            name: "Offer".into(),
            quality: 1,
            ..Default::default()
        },
    );
    s.tick(0.1);
    // The vendor is open: the bar's price line is the builder's own (`0x52e4a3`).
    assert_eq!(lines(&mut s, "TT"), ["Copper Bar", "[ITEM_UNSELLABLE]"]);
    assert_eq!(cleared(&mut s), before + 2);
    assert!(s.take_errors().is_empty());
}

/// `SetAction`'s item leg keeps the player's own object like any build (`0x5324d5`): its maker's
/// late name rebuilds it with the line.
#[test]
fn an_action_items_late_maker_name_adds_its_line() {
    let mut s = fixture();
    s.set_action(
        1,
        Some(ActionSlot {
            kind: 0x80,
            action: BAR,
            ..Default::default()
        }),
    );
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetAction(1)")
        .unwrap();
    assert_eq!(lines(&mut s, "TT"), ["Copper Bar"]);
    s.set_container(0, Some(backpack((SMITH, Some("Smith")))));
    s.tick(0.1);
    assert_eq!(
        lines(&mut s, "TT"),
        ["Copper Bar", "|cff00ff00[CREATED_BY Smith]|r"]
    );
    assert!(s.take_errors().is_empty());
}
