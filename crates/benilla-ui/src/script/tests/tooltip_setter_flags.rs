//! The `Set*` bindings that answer the tooltip builder's return value as 1 or nil: `SetAction`,
//! `SetSpell`, `SetShapeshift`, `SetPetAction` (`0x5325a9`, `0x532e9b`, `0x5328ae`) and the mail
//! and auction item setters (`0x5356c7`, `0x53549c`). The stock buttons arm `updateTooltip` from
//! it (`ActionButton.lua:377-381`, `SpellBookFrame.lua:239-243`). Each answer is read as the
//! count, type and value of everything the call returned.

use super::common::script;
use crate::script::*;

/// `count:type:value` of the first return, with `arg.n` counting them.
const SHOW: &str =
    "function PGShow(...) return arg.n .. ':' .. type(arg[1]) .. ':' .. tostring(arg[1]) end";

const RUNNING_30_MIN: Option<(i64, u32, bool)> = Some((9_400, 1_800_000, true));

fn view(recovery_ms: u32, chance: Option<&str>) -> SpellTooltipView {
    SpellTooltipView {
        name: "Shield Wall".into(),
        cast_time: Some("Instant".into()),
        recovery_ms,
        chance: chance.map(str::to_string),
        ..Default::default()
    }
}

fn show(s: &UiScript, call: &str) -> String {
    s.eval::<String>(&format!(
        "TT:SetOwner(FL, 'ANCHOR_RIGHT') return PGShow({call})"
    ))
    .unwrap()
}

fn fixture() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(SHOW).unwrap();
    s.run(
        r#"
        CreateFrame("Button", "FL"):SetPoint("CENTER", 0, 0)
        FL:SetWidth(10) FL:SetHeight(10)
        CreateFrame("GameTooltip", "TT")
    "#,
    )
    .unwrap();
    // A nonzero GetTime epoch, so the cooldowns above are running at 10 s.
    s.tick(10.0);
    s
}

fn slot(spell_id: u32, cooldown: Option<(i64, u32, bool)>) -> SpellSlotView {
    SpellSlotView {
        spell_id,
        name: "Shield Wall".into(),
        cooldown,
        ..Default::default()
    }
}

/// A spell's tooltip holds a changing line when its recovery is a minute or more and the cooldown
/// runs (the remaining line, `0x52f581`), or when it carries the chance line (`0x52f68c`); the
/// book setter answers that as 1, else nil, always one value.
#[test]
fn set_spell_answers_one_while_the_tooltip_holds_a_changing_line() {
    let mut s = fixture();
    s.set_spell_tooltip(871, view(1_800_000, None));
    s.set_spell_tooltip(872, view(30_000, None));
    s.set_spell_tooltip(873, view(0, Some("5.00% chance to parry")));
    s.set_spellbook(SpellBookState {
        tabs: Vec::new(),
        slots: vec![
            slot(871, RUNNING_30_MIN),
            // No cooldown, a shorter recovery, a passive's chance line, a cooldown on hold
            // (enable 0), and one that elapsed at 3 s.
            slot(871, None),
            slot(872, Some((9_400, 30_000, true))),
            slot(873, None),
            slot(871, Some((9_400, 1_800_000, false))),
            slot(871, Some((1_000, 2_000, true))),
        ],
    });
    for (index, want) in [
        (1, "1:number:1"),
        (2, "1:nil:nil"),
        (3, "1:nil:nil"),
        (4, "1:number:1"),
        (5, "1:nil:nil"),
        (6, "1:nil:nil"),
        // Past the book, and a non-string book type, miss with one nil.
        (99, "1:nil:nil"),
    ] {
        assert_eq!(
            show(&s, &format!("TT:SetSpell({index}, 'spell')")),
            want,
            "slot {index}"
        );
    }
    // A number book type is stringified, the player's book (`0x6f3510`); anything else raises.
    assert_eq!(show(&s, "TT:SetSpell(1, 5)"), "1:number:1");
    assert!(s
        .eval::<bool>(
            "local ok, err = pcall(TT.SetSpell, TT, 1, nil) \
             return not ok and string.find(tostring(err), 'Invalid spell slot in SetSpell', 1, true) ~= nil"
        )
        .unwrap());
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

#[test]
fn set_shapeshift_and_set_pet_action_answer_the_same_flag() {
    let mut s = fixture();
    s.set_spell_tooltip(871, view(1_800_000, None));
    s.set_pet_spell_tooltip(871, view(1_800_000, None));
    // A chance line is a changing line too, but these setters read the builder's value only with
    // a cooldown running (`0x532a89`, `0x5328aa`).
    s.set_spell_tooltip(873, view(0, Some("5.00% chance to parry")));
    s.set_pet_spell_tooltip(873, view(0, Some("5.00% chance to parry")));
    s.set_shapeshift_forms(vec![
        ShapeshiftFormView {
            spell_id: 871,
            name: "Battle Stance".into(),
            cooldown: RUNNING_30_MIN,
            ..Default::default()
        },
        ShapeshiftFormView {
            spell_id: 871,
            name: "Defensive Stance".into(),
            ..Default::default()
        },
        ShapeshiftFormView {
            spell_id: 873,
            name: "Parry".into(),
            ..Default::default()
        },
        ShapeshiftFormView {
            spell_id: 873,
            name: "Parry".into(),
            cooldown: RUNNING_30_MIN,
            ..Default::default()
        },
    ]);
    let pet = |cooldown| PetActionView {
        name: Some("Shield Wall".into()),
        spell_id: Some(871),
        cooldown,
        ..Default::default()
    };
    let chance_pet = |cooldown| PetActionView {
        spell_id: Some(873),
        ..pet(cooldown)
    };
    s.set_pet_actions(
        true,
        true,
        true,
        vec![
            pet(RUNNING_30_MIN),
            pet(None),
            chance_pet(None),
            chance_pet(RUNNING_30_MIN),
        ],
    );
    for (call, want) in [
        ("TT:SetShapeshift(1)", "1:number:1"),
        ("TT:SetShapeshift(2)", "1:nil:nil"),
        ("TT:SetShapeshift(3)", "1:nil:nil"),
        ("TT:SetShapeshift(4)", "1:number:1"),
        ("TT:SetShapeshift(9)", "1:nil:nil"),
        ("TT:SetPetAction(1)", "1:number:1"),
        ("TT:SetPetAction(2)", "1:nil:nil"),
        ("TT:SetPetAction(3)", "1:nil:nil"),
        ("TT:SetPetAction(4)", "1:number:1"),
        ("TT:SetPetAction(9)", "1:nil:nil"),
    ] {
        assert_eq!(show(&s, call), want, "{call}");
    }
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// `SetAction` answers the spell builder's flag for a spell slot, the item builder's for an item
/// (a running cooldown reaches its remaining line, `0x52e140`), and nil for a macro, an empty slot
/// and a kind it does not build.
#[test]
fn set_action_answers_the_builders_flag_by_kind() {
    let mut s = fixture();
    s.set_spell_tooltip(871, view(1_800_000, None));
    s.set_item_template(
        6948,
        ItemTemplateView {
            name: "Hearthstone".into(),
            quality: 1,
            ..Default::default()
        },
    );
    s.set_macros(MacroState {
        account: vec![MacroView {
            name: "spawn".into(),
            ..Default::default()
        }],
        character: Vec::new(),
    });
    let put = |s: &mut UiScript, id: u32, kind: u8, action: u32, cooldown| {
        s.set_action(
            id,
            Some(ActionSlot {
                kind,
                action,
                ..Default::default()
            }),
        );
        s.set_action_state(
            id,
            Some(ActionState {
                usable: true,
                cooldown,
                ..Default::default()
            }),
        );
    };
    put(&mut s, 1, 0x00, 871, RUNNING_30_MIN);
    put(&mut s, 2, 0x00, 871, None);
    // The player owns one Hearthstone (the action word resolves by entry, `0x532490`).
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::from([(
                1,
                ContainerSlot {
                    item_id: 6948,
                    count: 1,
                    ..Default::default()
                },
            )]),
        }),
    );
    put(&mut s, 3, 0x80, 6948, RUNNING_30_MIN);
    put(&mut s, 4, 0x80, 6948, None);
    put(&mut s, 5, 0x40, 1, RUNNING_30_MIN);
    put(&mut s, 6, 0xC0, 1, RUNNING_30_MIN);
    // Attack, Parry, Block and Dodge carry the chance line: nil without a running cooldown.
    s.set_spell_tooltip(873, view(0, Some("5.00% chance to parry")));
    put(&mut s, 8, 0x00, 873, None);
    put(&mut s, 9, 0x00, 873, RUNNING_30_MIN);
    for (id, want) in [
        (1, "1:number:1"),
        (2, "1:nil:nil"),
        (3, "1:number:1"),
        (4, "1:nil:nil"),
        (5, "1:nil:nil"),
        (6, "1:nil:nil"),
        (7, "1:nil:nil"),
        (8, "1:nil:nil"),
        (9, "1:number:1"),
    ] {
        assert_eq!(
            show(&s, &format!("TT:SetAction({id})")),
            want,
            "action {id}"
        );
    }
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The four mail and auction item setters answer one value: nil with no item, nil for a hover that
/// writes no changing line. Their builder reaches the changing lines (the item's expiry and a
/// temporary enchant's countdown) only through an item object or an instance block carrying
/// them, which these rows do not.
#[test]
fn the_mail_and_auction_item_setters_answer_one_value() {
    let mut s = fixture();
    s.set_item_template(
        2589,
        ItemTemplateView {
            name: "Linen Cloth".into(),
            ..Default::default()
        },
    );
    let call = |s: &UiScript, c: &str| show(s, c);
    // Nothing attached or listed: nil, one value.
    for c in [
        "TT:SetInboxItem(1)",
        "TT:SetAuctionItem('list', 1)",
        "TT:SetAuctionSellItem()",
        "TT:SetSendMailItem()",
    ] {
        assert_eq!(call(&s, c), "1:nil:nil", "{c} empty");
    }
    s.set_mail(Some(MailState {
        inbox: vec![MailInboxRow {
            item_id: 2589,
            item_name: Some("Linen Cloth".into()),
            ..Default::default()
        }],
        open: true,
    }));
    let mut auction = AuctionState::default();
    auction.lists[0].rows.push(AuctionItemRow {
        item_id: 2589,
        name: Some("Linen Cloth".into()),
        ..Default::default()
    });
    s.set_auction(Some(auction));
    {
        let mut model = s.model_mut();
        let item = CursorItem {
            item_class: None,
            guid: 0x120,
            bag: 0,
            slot: 1,
            item_id: 2589,
            texture: None,
            link: None,
            count: None,
            quality: Some(1),
            equip_slots: Vec::new(),
            bar_placeable: false,
            consumable: false,
        };
        model.mail_send_item = Some(item.clone());
        model.auction_sell_item = Some(item);
    }
    for c in [
        "TT:SetInboxItem(1)",
        "TT:SetAuctionItem('list', 1)",
        "TT:SetAuctionSellItem()",
        "TT:SetSendMailItem()",
    ] {
        assert_eq!(call(&s, c), "1:nil:nil", "{c} filled");
        assert!(s.eval::<bool>("return TT:IsShown()").unwrap(), "{c} drew");
    }
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The sell and send-mail slots hold a live item object, so its expiry, temporary enchants and
/// LOCKED state reach the flag; the trade target's block names a lock of its own.
#[test]
fn live_objects_and_blocks_reach_the_flag() {
    let mut s = fixture();
    s.run(r#"ITEM_DURATION_HOURS = "[DURATION %d h]" LOCKED = "[LOCKED]""#)
        .unwrap();
    s.set_item_template(
        8300,
        ItemTemplateView {
            name: "Strongbox".into(),
            lock_id: 7,
            ..Default::default()
        },
    );
    s.set_item_template(
        8301,
        ItemTemplateView {
            name: "Plain".into(),
            ..Default::default()
        },
    );
    let held = |item_id, flags, duration_ms| ContainerSlot {
        item_id,
        count: 1,
        flags,
        duration_ms,
        ..Default::default()
    };
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::from([
                (1, held(8300, 0, None)),
                (2, held(8300, 0x4, None)),
                (3, held(8301, 0, Some(3_600_000))),
                (4, held(8301, 0, None)),
            ]),
        }),
    );
    let cursor = |slot| CursorItem {
        item_class: None,
        guid: 0x121,
        bag: 0,
        slot,
        item_id: if slot <= 2 { 8300 } else { 8301 },
        texture: None,
        link: None,
        count: None,
        quality: Some(1),
        equip_slots: Vec::new(),
        bar_placeable: false,
        consumable: false,
    };
    for (slot, want) in [
        (1, "1:number:1"),
        (2, "1:nil:nil"),
        (3, "1:number:1"),
        (4, "1:nil:nil"),
    ] {
        {
            let mut model = s.model_mut();
            model.mail_send_item = Some(cursor(slot));
            model.auction_sell_item = Some(cursor(slot));
        }
        assert_eq!(show(&s, "TT:SetSendMailItem()"), want, "mail slot {slot}");
        assert_eq!(
            show(&s, "TT:SetAuctionSellItem()"),
            want,
            "sell slot {slot}"
        );
    }

    // The trade target's block carries the partner's lock id: LOCKED with no object, flag set.
    // The action bar's item word resolves to the player's own item by entry (`0x532490`): the
    // locked strongbox in the bag sets the flag with no cooldown, a plain one does not.
    for (id, item) in [(1, 8300), (2, 8301)] {
        s.set_action(
            id,
            Some(ActionSlot {
                kind: 0x80,
                action: item,
                ..Default::default()
            }),
        );
    }
    assert_eq!(show(&s, "TT:SetAction(1)"), "1:number:1");
    assert_eq!(show(&s, "TT:SetAction(2)"), "1:number:1", "slot 3 expires");
    {
        let mut model = s.model_mut();
        model.containers.get_mut(&0).unwrap().slots.remove(&3);
    }
    assert_eq!(show(&s, "TT:SetAction(2)"), "1:nil:nil");

    let mut st = TradeState::default();
    for (i, lock_id) in [(0, 9), (1, 0)] {
        st.target.slots[i] = Some(TradeSlotItem {
            item_id: 8301,
            name: Some("Plain".into()),
            count: 1,
            lock_id,
            ..Default::default()
        });
    }
    // The player's slots are live item objects: the flags decide LOCKED (`0x5342ed`), and the
    // block's lock id is zeroed (`0x534341`).
    for (i, flags) in [(0, Some(0)), (1, Some(0x4)), (2, None)] {
        st.player.slots[i] = Some(TradeSlotItem {
            item_id: 8300,
            name: Some("Strongbox".into()),
            count: 1,
            object_flags: flags,
            ..Default::default()
        });
    }
    s.set_trade(Some(st));
    let locked = |s: &UiScript, call: &str| {
        s.eval::<bool>(&format!(
            "TT:SetOwner(FL, 'ANCHOR_RIGHT') {call} \
             for i = 1, TT:NumLines() do \
               if getglobal('TTTextLeft' .. i):GetText() == '[LOCKED]' then return true end \
             end return false"
        ))
        .unwrap()
    };
    assert!(locked(&s, "TT:SetTradeTargetItem(1)"));
    assert!(!locked(&s, "TT:SetTradeTargetItem(2)"));
    assert!(
        locked(&s, "TT:SetTradePlayerItem(1)"),
        "a locked object in the player's slot"
    );
    assert!(!locked(&s, "TT:SetTradePlayerItem(2)"), "UNLOCKED");
    assert!(
        !locked(&s, "TT:SetTradePlayerItem(3)"),
        "no object, no block lock"
    );
}

/// The action bar's item word finds the player's item as `0x622420` walks with mask `0x47`: the
/// worn items, then each equipped bag and its contents, then the backpack, then the keyring; the
/// bank is out of the mask; a broken item or one with flag `0x10` is skipped; a miss pushes nil
/// whatever the builder returned (`0x5325bc`). A timed copy answers 1, a plain one nil, so the
/// answer says which copy was found.
#[test]
fn the_action_bar_item_word_walks_the_references_order() {
    let mut s = fixture();
    s.run("ITEM_DURATION_HOURS = '[DURATION %d h]'").unwrap();
    s.set_item_template(
        9000,
        ItemTemplateView {
            name: "Charm".into(),
            ..Default::default()
        },
    );
    let timed = |flags, durability| ContainerSlot {
        item_id: 9000,
        count: 1,
        flags,
        durability,
        duration_ms: Some(3_600_000),
        ..Default::default()
    };
    let plain = ContainerSlot {
        item_id: 9000,
        count: 1,
        ..Default::default()
    };
    let bag = |slots: Vec<(u32, ContainerSlot)>| {
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots: slots.into_iter().collect(),
        })
    };
    let ask = |s: &mut UiScript| {
        s.set_action(
            1,
            Some(ActionSlot {
                kind: 0x80,
                action: 9000,
                ..Default::default()
            }),
        );
        show(s, "TT:SetAction(1)")
    };
    // Not carried at all.
    assert_eq!(ask(&mut s), "1:nil:nil");
    // Only in the bank (container -1): outside the mask, so a miss.
    s.set_container(-1, bag(vec![(1, timed(0, None))]));
    assert_eq!(ask(&mut s), "1:nil:nil", "the bank is not walked");
    // A broken timed copy and a flag-0x10 timed copy in the backpack are skipped; the plain
    // copy behind them is found, and says nil.
    s.set_container(
        0,
        bag(vec![
            (1, timed(0, Some((0, 100)))),
            (2, timed(0x10, None)),
            (3, plain.clone()),
        ]),
    );
    assert_eq!(
        ask(&mut s),
        "1:nil:nil",
        "skipped copies, then the plain one"
    );
    // A wrapped broken copy is not skipped (`0x6224f7`).
    s.set_container(0, bag(vec![(1, timed(0x8, Some((0, 100))))]));
    assert_eq!(ask(&mut s), "1:number:1");
    // An equipped bag's contents come before the backpack's.
    s.set_container(0, bag(vec![(1, plain.clone())]));
    s.set_container(1, bag(vec![(1, timed(0, None))]));
    assert_eq!(ask(&mut s), "1:number:1", "bag 1 before the backpack");
    // The worn item comes before the bags.
    let mut inv: InventorySlots = Default::default();
    inv[5] = Some(InvSlotView {
        item_id: 9000,
        ..Default::default()
    });
    s.set_inventory_slots(inv);
    assert_eq!(
        ask(&mut s),
        "1:nil:nil",
        "the worn copy is plain and found first"
    );
    // The keyring comes last.
    let mut inv: InventorySlots = Default::default();
    inv[5] = None;
    s.set_inventory_slots(inv);
    s.set_container(1, None);
    s.set_container(0, None);
    s.set_container(-2, bag(vec![(1, timed(0, None))]));
    assert_eq!(ask(&mut s), "1:number:1", "the keyring is walked");
}
