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
    assert_eq!(show(&s, "TT:SetSpell(1, 5)"), "1:nil:nil");
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
            bag: 0,
            slot: 1,
            item_id: 2589,
            texture: None,
            link: None,
            count: None,
            quality: Some(1),
            equip_slots: Vec::new(),
            bar_placeable: false,
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
