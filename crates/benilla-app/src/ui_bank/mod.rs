//! The bank window's app side: the open banker, the purchase row, the bank events and the Lua
//! intents. The vault itself is descriptor fields that [`crate::ui_items`] feeds as containers
//! `-1` and `5..=10`, and moves items in and out of on a right-click.

use benilla_protocol::messages::bank_slot_result;
use bevy::prelude::*;

use benilla_ui::script::{BankState, UiScript};

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, npc_switched, NpcSession};

/// `BankBagSlotPrices.dbc`, the client's own slot prices; absent, the purchase row shows 0 and
/// the server still charges its own price.
#[derive(Resource)]
pub(crate) struct BankPrices(pub(crate) benilla_formats::BankBagSlotPrices);

/// The open bank: the banker from `SMSG_SHOW_BANK`, until a close or a disconnect.
#[derive(Resource, Default)]
pub(crate) struct BankOpen {
    pub(crate) banker: Option<u64>,
}

impl BankOpen {
    pub(crate) fn open(&mut self, banker: u64) {
        self.banker = Some(banker);
    }

    pub(crate) fn is_open(&self) -> bool {
        self.banker.is_some()
    }

    /// A client-side close: there is no close packet.
    pub(crate) fn clear(&mut self) {
        self.banker = None;
    }

    /// The disconnect clear.
    pub(crate) fn clear_session(&mut self) {
        self.clear();
    }
}

/// The range guard closes the bank, as `CloseBankFrame` does, when the banker is out of range or
/// gone.
impl NpcSession for BankOpen {
    fn npc(&self) -> Option<u64> {
        self.banker
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// `SMSG_BUY_BANK_SLOT_RESULT` codes waiting for the error line; vmangos sends one only on failure.
#[derive(Resource, Default)]
pub(crate) struct BankErrors(pub Vec<u32>);

mod net;

pub(crate) struct UiBankPlugin;

impl Plugin for UiBankPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<BankOpen>()
            .init_resource::<BankErrors>()
            .add_systems(
                Update,
                (
                    // Range-close first so the clear fires `BANKFRAME_CLOSED` the same frame.
                    close_npc_session_out_of_range::<BankOpen>.before(feed_bank),
                    feed_bank.in_set(UiFeed),
                    drain_bank.after(UiInput),
                ),
            );
    }
}

/// The `ERR_BANKSLOT_*` line for a `SMSG_BUY_BANK_SLOT_RESULT` code. The reference's handler
/// (`0x5e3f8d`) ignores any code `>= 3`, `OK` included (`0x5e3f9c`), and maps the rest through the
/// three-entry table at `0x80af14`, message ids `0x100..=0x102`.
fn bank_slot_error_key(result: u32) -> Option<&'static str> {
    Some(match result {
        bank_slot_result::FAILED_TOO_MANY => "ERR_BANKSLOT_FAILED_TOO_MANY", // 0x100
        bank_slot_result::INSUFFICIENT_FUNDS => "ERR_BANKSLOT_INSUFFICIENT_FUNDS", // 0x101
        bank_slot_result::NOTBANKER => "ERR_BANKSLOT_NOTBANKER",             // 0x102
        _ => return None,
    })
}

/// Push the purchase row and fire the bank events. `BANKFRAME_OPENED` has no arguments: the
/// reference fires it bare (`0x4f8522`), and stock `BankFrame.lua:127` titles with
/// `UnitName("npc")`.
fn feed_bank(
    script: Option<NonSendMut<UiScript>>,
    open: Res<BankOpen>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    prices: Option<Res<BankPrices>>,
    mut errors: ResMut<BankErrors>,
    mut last: Local<crate::ui_script::VmMemo<Option<BankState>>>,
    mut last_banker: Local<crate::ui_script::VmMemo<Option<u64>>>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_banker = last_banker.get(&script);
    // Refusals show, and speak, as their message rows say: insufficient funds is speech `0x16`.
    let lines: Vec<_> = errors
        .0
        .drain(..)
        .filter_map(|result| crate::ui_action::keyed_line(&script, bank_slot_error_key(result)?))
        .collect();
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_bank", lines);
    let store = self_q.iter().next();
    let purchased = store
        .and_then(|s| s.0.player_bank_bag_slots_purchased())
        .unwrap_or(0);
    let fresh = open.banker.map(|_| BankState {
        num_purchased: u32::from(purchased),
        next_cost: prices
            .as_ref()
            .and_then(|p| p.0.next_slot_price(purchased))
            .unwrap_or(0),
    });
    // A new banker while open is a close then an open; the close intent OnHide queues is
    // consumed so the drain keeps the new session.
    let switched = npc_switched(*last_banker, open.banker);
    if fresh == *last && !switched {
        return;
    }
    script.set_bank(fresh.clone());
    if switched {
        script.fire_event("BANKFRAME_CLOSED", vec![]);
        script.fire_event("BANKFRAME_OPENED", vec![]);
        let _ = script.take_bank_close();
    } else {
        match (&*last, &fresh) {
            (None, Some(_)) => script.fire_event("BANKFRAME_OPENED", vec![]),
            // The bought count moved: a bought slot has no reply packet, so this confirms it.
            (Some(_), Some(_)) => script.fire_event("PLAYERBANKBAGSLOTS_CHANGED", vec![]),
            (Some(_), None) => script.fire_event("BANKFRAME_CLOSED", vec![]),
            (None, None) => {}
        }
    }
    *last = fresh;
    *last_banker = open.banker;
}

/// Drain the Lua intents: `PurchaseSlot()` sends the buy, `CloseBankFrame()` clears locally.
fn drain_bank(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<BankOpen>,
    net: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    if script.take_bank_purchase() {
        if let Some(banker) = open.banker {
            debug!("bank: purchase next bag slot at {banker:#x}");
            let _ = net.0.send(ClientCommand::BuyBankSlot { guid: banker });
        }
    }
    if script.take_bank_close() && open.is_open() {
        debug!("bank: client-side close");
        open.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_slot_error_keys() {
        assert_eq!(
            bank_slot_error_key(bank_slot_result::FAILED_TOO_MANY),
            Some("ERR_BANKSLOT_FAILED_TOO_MANY")
        );
        assert_eq!(
            bank_slot_error_key(bank_slot_result::INSUFFICIENT_FUNDS),
            Some("ERR_BANKSLOT_INSUFFICIENT_FUNDS")
        );
        assert_eq!(
            bank_slot_error_key(bank_slot_result::NOTBANKER),
            Some("ERR_BANKSLOT_NOTBANKER")
        );
        assert_eq!(bank_slot_error_key(bank_slot_result::OK), None);
        assert_eq!(
            bank_slot_error_key(99),
            None,
            "past the reference's own bound"
        );
        // The ids the reference's table at `0x80af14` holds; insufficient funds also speaks.
        for (key, id) in [
            ("ERR_BANKSLOT_FAILED_TOO_MANY", 0x100u16),
            ("ERR_BANKSLOT_INSUFFICIENT_FUNDS", 0x101),
            ("ERR_BANKSLOT_NOTBANKER", 0x102),
        ] {
            let r = benilla_ui::messages::by_key(key).expect("catalog row");
            assert_eq!(r.id, id, "{key}");
            assert_eq!(r.kind, benilla_ui::messages::MsgKind::Error, "{key}");
        }
        assert_eq!(
            benilla_ui::messages::by_key("ERR_BANKSLOT_INSUFFICIENT_FUNDS")
                .unwrap()
                .type_tag,
            0x16
        );
    }
}
