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
    s.set_container(0, Some(backpack(None)));
    s
}

/// The backpack with the copper bar in slot 1, made by `creator`.
fn backpack(creator: Option<&str>) -> ContainerState {
    ContainerState {
        name: Some("Backpack".into()),
        num_slots: 16,
        slots: HashMap::from([(
            1,
            ContainerSlot {
                item_id: BAR,
                count: 1,
                quality: Some(1),
                link: Some(format!("|cffffffff|Hitem:{BAR}:0:0:0|h[Copper Bar]|h|r")),
                creator: creator.map(str::to_string),
                ..Default::default()
            },
        )]),
    }
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

/// The maker's name answers the name cache (`0x52e1f4`): the bag slot's view carries it once the
/// app re-pushes it, and the tooltip that lacked it rebuilds with the line.
#[test]
fn a_made_by_name_that_arrives_adds_its_line() {
    let mut s = fixture();
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 1)")
        .unwrap();
    assert_eq!(lines(&mut s, "TT"), ["Copper Bar"]);
    s.set_container(0, Some(backpack(Some("Smith"))));
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
