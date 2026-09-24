//! The bank bindings: the app pushes the open bank's snapshot ([`UiScript::set_bank`]), and
//! `PurchaseSlot` and `CloseBankFrame` queue intents it drains.
//!
//! The vault is container `-1` (24 slots) and containers 5..=10 (the six bags), the reference's
//! own ids (`BankFrame.lua:1-4`), so the container verbs work on it unchanged. The bag items
//! themselves are inventory slots 64..=69, read through `GetInventoryItem*` as the reference's bank
//! reads them (`BankFrame.lua:28`); they stream whether or not a banker is open.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// The open bank's purchase row, pushed whole by the app.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BankState {
    /// Purchased bag slots, 0..=6: `PLAYER_BYTES_2` byte 2.
    pub num_purchased: u32,
    /// The next slot's price in copper, `BankBagSlotPrices.dbc` row `num_purchased + 1`; the rows
    /// past 6 hold 999999999, and 0 means the row is absent.
    pub next_cost: u32,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open bank's snapshot.
    pub fn set_bank(&mut self, state: Option<BankState>) {
        self.model_mut().bank = state;
    }

    /// Whether `PurchaseSlot()` was called since the last drain; the app sends
    /// `CMSG_BUY_BANK_SLOT`. Success has no reply packet, only the `PLAYER_BYTES_2` byte-2 change
    /// (`ItemHandler.cpp:934`).
    pub fn take_bank_purchase(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().bank_purchase)
    }

    /// Whether `CloseBankFrame()` was called since the last drain; no packet exists for it.
    pub fn take_bank_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().bank_close)
    }
}

/// Register the bank globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumBankSlots() → numSlots, full: `full` is 1 from six slots up, else nil (`0x4f85b0`:
    // `cmp esi,6; jl`).
    g.set(
        "GetNumBankSlots",
        lua.create_function(|lua, ()| {
            let n = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.bank.as_ref().map_or(0, |b| b.num_purchased)
            };
            let full = if n >= 6 {
                Value::Integer(1)
            } else {
                Value::Nil
            };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(n)),
                full,
            ]))
        })?,
    )?;

    // GetBankSlotCost(numSlots): the argument is unread; the pushed state names the next slot.
    g.set(
        "GetBankSlotCost",
        lua.create_function(|lua, _n: Option<u32>| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.bank.as_ref().map_or(0, |b| b.next_cost)))
        })?,
    )?;

    // PurchaseSlot(): the `CONFIRM_BUY_BANK_SLOT` popup's accept (`StaticPopup.lua:52`).
    g.set(
        "PurchaseSlot",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.bank_purchase = true;
            Ok(())
        })?,
    )?;

    g.set(
        "CloseBankFrame",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.bank_close = true;
            Ok(())
        })?,
    )?;

    // BankButtonIDToInvSlotID(id, isBag) (`0x4f8530`): `id + 39` for an item button, `id + 59`
    // for a bag button, whose id is the container id 5..=10 (`BankFrame.xml:372`), the same
    // arithmetic as `ContainerIDToInventoryID`'s bank-bag arm (`0x4f94e0`). There is no range
    // check. `isBag` is `lua_isnumber(L, 2)` (`0x4f8576`), not truthiness: `true` takes the item
    // arm, `0` and `"1"` the bag arm; stock sets `this.isBag = 1` (`BankFrame.lua:24`). Argument 1
    // truncates toward zero (`0x40a2b0`), and its usage string names only `buttonID`.
    g.set(
        "BankButtonIDToInvSlotID",
        lua.create_function(|lua, (id, is_bag): (Value, Option<Value>)| {
            let n = super::binding_abi::number_arg(
                lua,
                id,
                "Usage: BankButtonIDToInvSlotID(buttonID)",
            )?;
            let is_bag = is_bag
                .and_then(|v| lua.coerce_number(v).ok().flatten())
                .is_some();
            let step = if is_bag { 59 } else { 39 };
            Ok(i64::from(n.wrapping_add(step)))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::BankState;
    use crate::script::UiScript;

    #[test]
    fn bank_snapshot_reads() {
        let mut s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("local n, full = GetNumBankSlots()\nreturn n == 0 and full == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetBankSlotCost(0)").unwrap(), 0);

        s.set_bank(Some(BankState {
            num_purchased: 2,
            next_cost: 100_000,
        }));
        assert!(s
            .eval::<bool>("local n, full = GetNumBankSlots()\nreturn n == 2 and full == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetBankSlotCost(2)").unwrap(), 100_000);

        // Six purchased: full = 1, and 999999999 is the DBC's sentinel row.
        s.set_bank(Some(BankState {
            num_purchased: 6,
            next_cost: 999_999_999,
        }));
        assert!(s
            .eval::<bool>("local n, full = GetNumBankSlots()\nreturn n == 6 and full == 1")
            .unwrap());

        s.set_bank(None);
        assert!(s
            .eval::<bool>("local n, full = GetNumBankSlots()\nreturn n == 0 and full == nil")
            .unwrap());
    }

    /// The bank bags are inventory slots 64..=69, as the reference's bank reads them
    /// (`BankFrame.lua:28`).
    #[test]
    fn bank_bag_slots_read_through_the_inventory_api() {
        let mut s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("return GetInventoryItemTexture(\"player\", 64) == nil")
            .unwrap());

        let mut bags: crate::script::BankBagSlots = Default::default();
        bags[0] = Some(crate::script::InvSlotView {
            item_id: 4500,
            icon: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            count: 1,
            link: Some("|cffffffff|Hitem:4500:0:0:0|h[Traveler\'s Backpack]|h|r".into()),
            equip_slots: vec![20, 21, 22, 23],
            ..Default::default()
        });
        s.set_bank_bag_slots(bags);

        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(5, 1)")
                .unwrap(),
            64,
            "BankFrameBag1's own id is 5 — the container id, not a bag number"
        );
        assert_eq!(
            s.eval::<String>("return GetInventoryItemTexture(\"player\", 64)")
                .unwrap(),
            "Interface\\Icons\\INV_Misc_Bag_08"
        );
        assert_eq!(
            s.eval::<i64>("return GetInventoryItemID(\"player\", 64)")
                .unwrap(),
            4500
        );
        // Slot 65 is empty and 70 is past the band; neither falls through to the doll.
        assert!(s
            .eval::<bool>("return GetInventoryItemTexture(\"player\", 65) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetInventoryItemTexture(\"player\", 70) == nil")
            .unwrap());
    }

    #[test]
    fn purchase_and_close_flag_the_intents() {
        let mut s = UiScript::new().unwrap();
        assert!(!s.take_bank_purchase());
        s.run("PurchaseSlot()").unwrap();
        assert!(s.take_bank_purchase());
        assert!(!s.take_bank_purchase(), "drained");

        assert!(!s.take_bank_close());
        s.run("CloseBankFrame()").unwrap();
        assert!(s.take_bank_close());
        assert!(!s.take_bank_close(), "drained");
    }

    /// The reference paints the vault through the inventory API (`BankFrame.lua:35`) while the app
    /// feeds it as container `-1`; if the two disagree the bank draws empty.
    #[test]
    fn the_bank_band_reads_the_vault_through_the_inventory_api() {
        let mut s = UiScript::new().unwrap();
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            3,
            super::super::ContainerSlot {
                item_id: 4496,
                count: 7,
                texture: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
                quality: Some(2),
                link: Some("|cff1eff00|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
                ..Default::default()
            },
        );
        s.set_container(
            -1,
            Some(super::super::ContainerState {
                name: Some("Bank".into()),
                num_slots: 24,
                slots,
            }),
        );

        // Vault slot 3 is inventory id 42.
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(3)").unwrap(),
            42
        );
        assert_eq!(
            s.eval::<String>("return GetInventoryItemTexture(\"player\", 42)")
                .unwrap(),
            "Interface\\Icons\\INV_Misc_Bag_08"
        );
        assert_eq!(
            s.eval::<i64>("return GetInventoryItemCount(\"player\", 42)")
                .unwrap(),
            7
        );
        assert_eq!(
            s.eval::<i64>("return GetInventoryItemID(\"player\", 42)")
                .unwrap(),
            4496
        );
        // An empty vault slot answers nil rather than falling through to `inventory_slots`.
        assert!(s
            .eval::<bool>("return GetInventoryItemTexture(\"player\", 43) == nil")
            .unwrap());
        // The doll is untouched either side of the band.
        assert!(s
            .eval::<bool>("return GetInventoryItemTexture(\"player\", 16) == nil")
            .unwrap());
    }

    /// Item buttons 1..=24 map to 40..=63 and bag buttons, whose ids are the container ids 5..=10,
    /// to 64..=69; the wire slot is one less.
    #[test]
    fn bank_button_to_inv_slot() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(1)").unwrap(),
            40
        );
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(24)").unwrap(),
            63
        );
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(5, 1)")
                .unwrap(),
            64,
            "BankFrameBag1 carries id 5"
        );
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(10, 1)")
                .unwrap(),
            69,
            "BankFrameBag6 carries id 10"
        );
        // The same map as `ContainerIDToInventoryID` for a bank bag; a drift would put the bag row
        // and the bag windows on different slots.
        for id in 5..=10 {
            assert_eq!(
                s.eval::<i64>(&format!("return BankButtonIDToInvSlotID({id}, 1)"))
                    .unwrap(),
                s.eval::<i64>(&format!("return ContainerIDToInventoryID({id})"))
                    .unwrap()
            );
        }
    }

    #[test]
    fn bank_button_to_inv_slot_follows_the_carved_abi() {
        let s = UiScript::new().unwrap();

        // `isBag` is `lua_isnumber(L, 2)`: `true` takes the item arm as nil and false do, while
        // `0` and `"1"` take the bag arm.
        for (call, want) in [
            ("BankButtonIDToInvSlotID(5, true)", 44),
            ("BankButtonIDToInvSlotID(5, false)", 44),
            ("BankButtonIDToInvSlotID(5, nil)", 44),
            ("BankButtonIDToInvSlotID(5, {})", 44),
            ("BankButtonIDToInvSlotID(5)", 44),
            ("BankButtonIDToInvSlotID(5, 0)", 64),
            ("BankButtonIDToInvSlotID(5, \"1\")", 64),
        ] {
            assert_eq!(
                s.eval::<i64>(&format!("return {call}")).unwrap(),
                want,
                "{call}"
            );
        }

        // No range check: the only compares in `0x4f8530` test `lua_isnumber`'s result.
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(25)").unwrap(),
            64
        );
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(-100)")
                .unwrap(),
            -61
        );

        // Argument 1 truncates toward zero (`0x40a2b0`), and the usage string names only
        // `buttonID`.
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(-2.9)")
                .unwrap(),
            37,
            "trunc toward zero gives -2, not -3"
        );
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(\"7\")")
                .unwrap(),
            46,
            "a numeric string passes the gate"
        );
        let err = s
            .eval::<i64>("return BankButtonIDToInvSlotID({})")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: BankButtonIDToInvSlotID(buttonID)"),
            "the reference's own usage string, verbatim: {err}"
        );
    }
}
