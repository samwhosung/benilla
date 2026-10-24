//! The NPC and profession windows' sends and engine writes inside their calls, beside their
//! closes: `PurchaseSlot`, `SortAuctionItems` and `DoTradeSkill`.

use benilla_protocol::ObjectFields;
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::systems::{client, eval, NPC};
use crate::net::{ClientCommand, Guid, GuidIndex, ObjectStore, SelfGuid, SelfPlayer};
use crate::ui_session::latch_tests::open;
use crate::ui_session::NpcWindow as W;

const ME: u64 = 0x10;

/// What went out, in order, of the kinds these chunks send.
fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|c| {
            Some(match c {
                ClientCommand::RandomRoll { min, .. } => format!("roll {min}"),
                ClientCommand::BuyBankSlot { guid } => format!("buy slot {guid:#x}"),
                ClientCommand::CastSpell { spell_id, .. } => format!("cast {spell_id}"),
                _ => return None,
            })
        })
        .collect()
}

/// Our player seated with `fields`, in the guid index.
fn seat(app: &mut App, fields: &[(u16, u32)]) {
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    let mut pairs = vec![(2, 0x19), (22, 100), (28, 100)];
    pairs.extend_from_slice(fields);
    let me = world
        .spawn((
            Guid(ME),
            SelfPlayer,
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&pairs)),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(ME, me);
}

/// `PurchaseSlot()` (`0x4f86b0`) sends `CMSG_BUY_BANK_SLOT` with the interaction target inside its
/// call (`0x4f8785`-`0x4f87a1`), ahead of the chunk's next verb.
#[test]
fn purchase_slot_sends_inside_its_call() {
    let (mut app, rx) = client();
    // `PLAYER_FIELD_COINAGE`: a purse over any price.
    seat(&mut app, &[(1176, 1_000_000)]);
    open(&mut app, W::Bank, NPC);
    eval::<()>(&mut app, "PurchaseSlot() RandomRoll(1, 2)");
    assert_eq!(sent(&rx), ["buy slot 0xf130000000000042", "roll 1"]);
}

/// `PurchaseSlot()`'s refusals inside its call (`0x4f86b0`): six slots bought returns silently
/// (`0x4f8703`); a purse below the next slot's price shows `ERR_BANKSLOT_INSUFFICIENT_FUNDS`
/// (`0x4f8743`, `0x4f8748`, row 257) before the chunk's next line, and neither sends.
#[test]
fn purchase_slot_refuses_inside_its_call() {
    // `PLAYER_FIELD_COINAGE`, and `PLAYER_BYTES_2` byte 2 the slots bought.
    const COINAGE: u16 = 1176;
    const BYTES_2: u16 = 194;
    for (bought, money, error) in [(6, 1_000_000, false), (0, 9, true)] {
        let (mut app, rx) = client();
        seat(&mut app, &[(COINAGE, money), (BYTES_2, bought << 16)]);
        app.insert_resource(crate::ui_bank::BankPrices(
            benilla_formats::BankBagSlotPrices::from_rows([(1, 10), (7, 999_999_999)]),
        ));
        open(&mut app, W::Bank, NPC);
        eval::<()>(
            &mut app,
            "ERR_BANKSLOT_INSUFFICIENT_FUNDS = 'Not enough money' SEEN = '' \
             local f = CreateFrame('Frame') f:RegisterEvent('UI_ERROR_MESSAGE') \
             f:SetScript('OnEvent', function() SEEN = SEEN .. arg1 .. ';' RandomRoll(9, 9) end)",
        );
        eval::<()>(&mut app, "PurchaseSlot() RandomRoll(1, 2)");
        let want: &[&str] = if error {
            &["roll 9", "roll 1"]
        } else {
            &["roll 1"]
        };
        assert_eq!(sent(&rx), want, "bought {bought}");
        let seen: String = eval(&mut app, "return SEEN");
        let want = if error { "Not enough money;" } else { "" };
        assert_eq!(seen, want, "bought {bought}");
    }
}

/// `SortAuctionItems(type, sort)` promotes the column in the engine's stack inside its call
/// (`0x4cfc00` → `0x4cd850`), with no auctioneer open: the stack is the one that outlives the
/// session and the VM.
#[test]
fn sort_auction_items_promotes_the_engine_stack_inside_its_call() {
    let (mut app, _rx) = client();
    eval::<()>(&mut app, "SortAuctionItems('list', 'name')");
    let stack = app
        .world()
        .resource::<crate::ui_auction::AuctionOpen>()
        .sorts
        .0[0];
    // "name" is the seventh key; the click moves it to the front unreversed.
    assert_eq!(stack.slots()[0], (6, false));
}

/// `DoTradeSkill(index, count)` casts the recipe, row 2 under its header, inside its call
/// (`0x500314` → `0x6e5a90`), ahead of the chunk's next verb.
#[test]
fn do_trade_skill_casts_inside_its_call() {
    const BOLT: u32 = 2963;
    let (mut app, rx) = client();
    seat(&mut app, &[]);
    app.world_mut().insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(
            [(
                BOLT,
                benilla_formats::SpellDisplay {
                    name: "Bolt of Linen Cloth".into(),
                    ..Default::default()
                },
            )]
            .into(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_trade_skill(Some(benilla_ui::script::TradeSkillState {
            line: 197,
            line_name: "Tailoring".into(),
            rank: 1,
            max_rank: 75,
            recipes: vec![benilla_ui::script::TradeSkillRecipe {
                group: None,
                spell_id: BOLT,
                name: "Bolt of Linen Cloth".into(),
                difficulty: benilla_ui::script::TradeSkillDifficulty::Trivial,
                num_available: 5,
                icon: None,
                min_made: 1,
                max_made: 1,
                cooldown: None,
                product_item: 2996,
                product_inv_type: 0,
                product_item_level: 0,
                reagents: vec![],
                tools: vec![],
                icon_pending: None,
                tools_pending: Vec::new(),
            }],
        }));
    eval::<()>(&mut app, "DoTradeSkill(2, 1) RandomRoll(1, 2)");
    assert_eq!(sent(&rx), ["cast 2963", "roll 1"]);
}
