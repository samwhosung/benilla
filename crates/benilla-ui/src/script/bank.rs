//! The bank bindings (decision 0604 phase 4) — the Era-shaped bank surface, the same two-way seam
//! as [`super::merchant`]: the app pushes a **bank snapshot** ([`UiScript::set_bank`] — the
//! purchased-slot count off the descriptor's `PLAYER_BYTES_2` byte 2 plus the next slot's
//! `BankBagSlotPrices.dbc` cost), and the Lua `PurchaseSlot`/`CloseBankFrame` calls queue outbound
//! **intents** the app drains ([`UiScript::take_bank_purchase`] / [`UiScript::take_bank_close`]).
//!
//! The bank's *contents* never pass through here: bank slots are player-array slots the container
//! seam already carries — the app feeds them as container `-1` (`BANK_CONTAINER`, the 24 generic
//! slots) and containers `5..=10` (the six bank bags), the reference client's own id space
//! (`BankFrame.lua:1-4`), so the container verbs, the cursor drag-drop, and the stack split all
//! work on bank slots with no bank-specific surface.
//!
//! Nor do the six bank BAGS — the bag items themselves, as opposed to what is inside them. Those
//! are inventory slots at live ids 64..=69, fed beside the paper doll's
//! ([`super::char_stats::BankBagSlots`]) and read through the ordinary `GetInventoryItem*` /
//! `PickupBagFromSlot` surface, which is exactly how the reference's own bank reads them
//! (`BankFrame.lua:28`, `ButtonInventorySlot`). They stream in the player descriptor whether or
//! not a banker is open, so they are not part of the window's snapshot.
//!
//! ## The 5875 API shape (the reference `BankFrame.lua`, read as behavior spec this session)
//!
//! - `GetNumBankSlots()` → `numSlots, full` — purchased count 0..6, `full` as `1`/`nil`
//!   (`UpdateBagSlotStatus` destructures exactly this pair; `full` hides the purchase frame).
//! - `GetBankSlotCost(numSlots)` → the NEXT slot's cost in copper. The real binding reads
//!   `BankBagSlotPrices.dbc` — whose rows 7+ hold a 999999999 sentinel, so the call answers even
//!   when the bank is full (the purchase frame is already hidden then). The argument is ignored
//!   here as it is there: the cost of "the next slot" is a fact of the pushed state.
//! - `PurchaseSlot()` — the confirm popup's accept (`StaticPopup.lua` `CONFIRM_BUY_BANK_SLOT`):
//!   queue the buy intent; the app sends `CMSG_BUY_BANK_SLOT`. No packet on success — the
//!   descriptor's byte-2 delta is the confirmation (`PLAYERBANKBAGSLOTS_CHANGED`).
//! - `CloseBankFrame()` — client-side close, **no packet exists** for it (decision 0604): flag the
//!   app to clear its session, the merchant/gossip pattern.
//! - `BankButtonIDToInvSlotID(id, isBag)` — the pure button→live-inventory-slot map: item button
//!   `i` (1..24) → live `39 + i` (wire 39..62 + 1), bag button — whose id is the **container id**
//!   5..10, not a bag number — → live `59 + id` (wire 63..68 + 1); the same "live id − 1 = wire
//!   slot" law as the doll (`crate::script::container`'s `EQUIPMENT_BAG` space). See the binding
//!   for the four places the reference's own file pins the bag arm's numbering.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// The open bank window's snapshot: what the purchase row and the six bag buttons need. Pushed
/// whole by the app while the bank session is open; `None` = no bank open (the window is closed).
/// The bank's item contents ride the container seam (module doc), not this.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BankState {
    /// Purchased bank-bag slots (0..=6) — the descriptor's `PLAYER_BYTES_2` byte 2.
    pub num_purchased: u32,
    /// The NEXT slot's price in copper (`BankBagSlotPrices.dbc` row `num_purchased + 1`; the DBC's
    /// own 999999999 sentinel past slot 6). 0 only if the DBC row is genuinely absent.
    pub next_cost: u32,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open bank's snapshot.
    pub fn set_bank(&mut self, state: Option<BankState>) {
        self.model_mut().bank = state;
    }

    /// Whether `PurchaseSlot()` was called since the last drain (and clear the flag). The app
    /// sends `CMSG_BUY_BANK_SLOT` to the open session's banker.
    pub fn take_bank_purchase(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().bank_purchase)
    }

    /// Whether `CloseBankFrame()` was called since the last drain (and clear the flag). No packet
    /// — the app just clears its local bank session (the merchant pattern).
    pub fn take_bank_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().bank_close)
    }
}

/// Register the bank globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumBankSlots() → numSlots, full (1/nil — the client's boolean shape; the reference
    // destructures `local numSlots, full = GetNumBankSlots()`). 0, nil with no bank open.
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

    // GetBankSlotCost(numSlots) → the next slot's cost in copper (module doc: the argument is
    // decorative — the pushed state already names the next slot). 0 with no bank open.
    g.set(
        "GetBankSlotCost",
        lua.create_function(|lua, _n: Option<u32>| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.bank.as_ref().map_or(0, |b| b.next_cost)))
        })?,
    )?;

    // BenillaGetBankBagTexture(i) → the bank bag slot's held-bag icon path | nil (empty slot, or
    // no bank open) — benilla-named (module doc: the snapshot carries what the reference read
    // through the inventory-item API).
    // PurchaseSlot() — queue the bank-slot buy intent (the CONFIRM_BUY_BANK_SLOT popup's accept).
    g.set(
        "PurchaseSlot",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.bank_purchase = true;
            Ok(())
        })?,
    )?;

    // CloseBankFrame() — client-side close (no packet exists, decision 0604): flag the app.
    g.set(
        "CloseBankFrame",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.bank_close = true;
            Ok(())
        })?,
    )?;

    // BankButtonIDToInvSlotID(id, isBag) — the pure button→live-slot map (module doc).
    //
    // `0x4f8530` (118 bytes). Three findings, and two of them corrected
    // this binding:
    //
    // 1 · **The arithmetic is `+59` / `+39`, and the bag arm takes the CONTAINER id 5..10** — not
    //   a bag number 1..6, which is what this used to compute. `0x4f8575 dec esi` runs BEFORE the
    //   branch and `0x4f8587 inc esi` AFTER it, so the pair cancels and the constants stand as
    //   written (`0x4f857f add esi,0x3b` / `0x4f8584 add esi,0x27`). The bag arm is numerically
    //   identical to `ContainerIDToInventoryID 0x4f94e0`'s own `id >= 5` arm — two functions, two
    //   instruction sequences, one constant. The reference's file pins the same numbering four
    //   times over: `BankFrame.xml` gives `BankFrameBag1` `id="5"`, `ButtonInventorySlot` hands
    //   `this:GetID()` straight in, and `UpdateBagButtonHighlight`/`BankFrameItemButton_UpdateLock`
    //   both subtract 4 from it to get back to 1..6.
    //
    // 2 · **`isBag` is a TYPE test, not a truthiness test.** `0x4f8576 call 0x6f34d0` is
    //   `lua_isnumber(L, 2)`, which accepts tag 3 or a tag-4 string `luaO_str2d` fully consumes,
    //   and refuses every other tag at `0x6f7c35` without ever loading the value. So **`true`
    //   takes the same arm as `nil`, `false` and a missing argument**, while `0` and `"1"` take
    //   the BAG arm. `BankFrame.lua:24` writes `this.isBag = 1` — a NUMBER — so the stock UI is
    //   correct either way; a client that models the flag as a boolean diverges the moment an
    //   addon passes `true`, and would then resolve the six bank bags to 44..49, colliding with
    //   `BankFrameItem5..10`.
    //
    // 3 · **Total, with no range check of any kind.** The only `test`/`cmp` in all 118 bytes are
    //   on `lua_isnumber`'s return value: no clamp, no mask, no `nil`. This used to answer 0
    //   outside 1..24 / 5..10 as a "wired wrong" tell; that was our invention, and the reference
    //   simply returns the arithmetic. Its immediate neighbour `GetNumBankSlots 0x4f85b0` carries
    //   a real `cmp esi,6; jl`, so a bound would have been visible here if one existed.
    //
    // Argument 1 is shape A ([`binding_abi::number_arg`]): `lua_isnumber` gated, truncated toward
    // zero by the `0x40a2b0` ftol, low dword only, and raising the `.data` usage string verbatim
    // — which names only `buttonID`, the reference's own omission of the second parameter.
    g.set(
        "BankButtonIDToInvSlotID",
        lua.create_function(|lua, (id, is_bag): (Value, Option<Value>)| {
            let n = super::binding_abi::number_arg(
                lua,
                id,
                "Usage: BankButtonIDToInvSlotID(buttonID)",
            )?;
            // Finding 2: `lua_isnumber(L, 2)`, asked of the VALUE's type — never its truthiness.
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

    /// The purchase-row reads: closed → (0, nil)/0; open → the pushed count + next cost; six
    /// purchased → `full` = 1 (the reference hides the purchase frame on it).
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

        // Six purchased: full = 1; the cost read still answers (the DBC's own sentinel row).
        s.set_bank(Some(BankState {
            num_purchased: 6,
            next_cost: 999_999_999,
        }));
        assert!(s
            .eval::<bool>("local n, full = GetNumBankSlots()\nreturn n == 6 and full == 1")
            .unwrap());

        // Clearing empties it.
        s.set_bank(None);
        assert!(s
            .eval::<bool>("local n, full = GetNumBankSlots()\nreturn n == 0 and full == nil")
            .unwrap());
    }

    /// The six bank BAGS are inventory slots at live ids 64..=69, read through the ordinary
    /// inventory API — the band the reference's own bank uses (`ButtonInventorySlot` →
    /// `BankButtonIDToInvSlotID(id, this.isBag)`, BankFrame.lua:28). Empty slots answer nil, and
    /// the band does not bleed into the equipment array below it.
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
        // Bag slot 2 is empty, and slot 70 is past the band — neither falls through to the doll.
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

    /// **The bank's inventory BAND answers from the container feed** — the map decision 1751's
    /// bank swap turns on. The reference's bank paints every slot through the inventory API
    /// (`BankFrameItemButton_OnUpdate` → `GetInventoryItemTexture("player", BankButtonIDToInv
    /// SlotID(id))`, BankFrame.lua:35) while benilla feeds the same items as container `-1`. If
    /// those two ever disagree the bank draws empty, which is exactly the failure this pins.
    ///
    /// Asserted through the live API rather than the model, because the API is what the
    /// reference's file calls, and the tooltip must agree with the icon under it.
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

        // Vault slot 3 is live-API inventory id 42 (BankButtonIDToInvSlotID(3)).
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
        // …and an empty vault slot answers nil, not the equipment slot that shares no numbering
        // with it — the band must not fall through to `inventory_slots`.
        assert!(s
            .eval::<bool>("return GetInventoryItemTexture(\"player\", 43) == nil")
            .unwrap());
        // The doll is untouched either side of the band.
        assert!(s
            .eval::<bool>("return GetInventoryItemTexture(\"player\", 16) == nil")
            .unwrap());
    }

    /// The button→live-slot map: item buttons 1..24 → 40..63; bag buttons — **whose ids are the
    /// container ids 5..10, not bag numbers** — → 64..69 (live id − 1 = the wire slot: bank items
    /// 39..62, bank bags 63..68).
    ///
    /// The bag arm's numbering is checked here because getting it wrong reads six slots off the
    /// end of the band and draws an empty bag row — which is what this binding did before
    /// `0x4f8530` was read.
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
        // It is the SAME map `ContainerIDToInventoryID` computes for a bank bag — one arithmetic
        // under two names, and a drift between them would put the bag row and the bag windows on
        // different slots.
        for id in 5..=10 {
            assert_eq!(
                s.eval::<i64>(&format!("return BankButtonIDToInvSlotID({id}, 1)"))
                    .unwrap(),
                s.eval::<i64>(&format!("return ContainerIDToInventoryID({id})"))
                    .unwrap()
            );
        }
    }

    /// The three things `0x4f8530` corrected, each of which this binding had wrong or invented.
    #[test]
    fn bank_button_to_inv_slot_follows_the_carved_abi() {
        let s = UiScript::new().unwrap();

        // **`isBag` is a TYPE test** (`lua_isnumber(L,2)`), not a truthiness test. `true` is not a
        // number, so it takes the ITEM arm exactly as nil and false do; `0` and `"1"` are numbers
        // and take the BAG arm. The stock UI is safe either way — `BankFrame.lua:24` writes
        // `this.isBag = 1` — but an addon passing `true` would otherwise land the six bank bags on
        // 44..49, on top of `BankFrameItem5..10`.
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

        // **Total — no range check of any kind.** Answering 0 outside the button ranges was ours,
        // not the reference's; the only compares in the body are on `lua_isnumber`'s result.
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(25)").unwrap(),
            64
        );
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(-100)")
                .unwrap(),
            -61
        );

        // Argument 1 is shape A: `lua_isnumber` gated, truncated toward ZERO by the `0x40a2b0`
        // ftol (not floored), and raising the `.data` usage string — which names only `buttonID`,
        // the reference's own omission of the second parameter.
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
