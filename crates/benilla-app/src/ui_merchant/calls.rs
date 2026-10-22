//! The merchant verbs' engine half, which the call host applies inside each call
//! ([`crate::script_calls::ScriptCallHost`]): each refuses or sends with the vendor cell as it
//! stands (`[0xbddfa0]`) before the verb returns, as the reference's bindings send in their own
//! bodies (`0x4fb850`, `0x4fb760`, `0x4fb950`, `0x4fbfe0`).

use benilla_ui::script::{MerchantCall, UiScript};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{buyback_order, MerchantOpen, RepairPurse, BUYBACK_SLOT_FIRST};
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};

/// The open vendor, the player, and what the verbs send through.
#[derive(SystemParam)]
pub(crate) struct MerchantSteps<'w, 's> {
    /// Absent in a world with no merchant window, which drops the calls.
    open: Option<Res<'w, MerchantOpen>>,
    net: Res<'w, NetCommands>,
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    purse: RepairPurse<'w, 's>,
}

impl MerchantSteps<'_, '_> {
    /// Apply one merchant call.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: MerchantCall) {
        let Self {
            open,
            net,
            self_q,
            purse,
        } = self;
        let Some(open) = open.as_deref() else {
            debug!("ui_merchant: no merchant window in this world; {call:?} dropped");
            return;
        };
        let me = self_q.iter().next();
        match call {
            // With a player (`0x4fb88e`), the row in range (`0x4fb904`-`0x4fb90e`), a vendor cell
            // (`0x4fb91d`) and a row slot (`0x4fb92e`): the row's item, `CMSG_BUY_ITEM`
            // (`0x4fb93c` → `0x5e1e90`), the quantity a byte.
            MerchantCall::Buy { row, quantity } => {
                let (Some(vendor), Some(_)) = (open.vendor, me) else {
                    return;
                };
                let Some(item) = open.row_at(row).filter(|it| it.slot != 0) else {
                    debug!("ui_merchant: BuyMerchantItem({row}) out of range — ignored");
                    return;
                };
                let count = quantity.clamp(1, u32::from(u8::MAX)) as u8;
                debug!("ui_merchant: buy row {row} (entry {} ×{count})", item.entry);
                let _ = net.0.send(ClientCommand::BuyItem {
                    vendor,
                    entry: item.entry,
                    count,
                });
            }
            // The vendor cell as it stands, never tested, so 0 with none (`0x4fb799`).
            MerchantCall::Sell { item_guid, count } => {
                let vendor = open.vendor.unwrap_or(0);
                debug!("ui_merchant: cursor sell of {item_guid:#x} ×{count} to {vendor:#x}");
                let _ = net.0.send(ClientCommand::SellItem {
                    vendor,
                    item_guid,
                    count,
                });
            }
            // With a vendor, a player and an item in the slot the list names (`0x4fb9b5`,
            // `0x4fb9fc`): the absolute slot, 69 to 80 (`0x4fba02`).
            MerchantCall::Buyback(index) => {
                let (Some(vendor), Some(store)) = (open.vendor, me) else {
                    return;
                };
                let order = buyback_order(&store.0);
                match index.checked_sub(1).and_then(|i| order.get(i as usize)) {
                    Some(&idx) => {
                        let slot = BUYBACK_SLOT_FIRST + u32::from(idx);
                        debug!("ui_merchant: buyback list index {index} → absolute slot {slot}");
                        let _ = net.0.send(ClientCommand::BuybackItem { vendor, slot });
                    }
                    None => debug!("ui_merchant: BuybackItem({index}) out of range — ignored"),
                }
            }
            // With a player, the purse test over the equipped slots 0-18 only (`0x4fc026`-
            // `0x4fc063`), not the bags `GetRepairAllCost` also sweeps; then `CMSG_REPAIR_ITEM`
            // with guid 0 (`0x4fc0d5`).
            MerchantCall::RepairAll => {
                let (Some(vendor), Some(me)) = (open.vendor, me) else {
                    return;
                };
                let worn = (0..19u8).filter_map(|i| me.0.player_inv_slot(i));
                if !purse.refuses(script, open, me, worn) {
                    debug!("ui_merchant: repair all");
                    let _ = net.0.send(ClientCommand::RepairItem {
                        vendor,
                        item_guid: 0,
                    });
                }
            }
        }
    }
}
