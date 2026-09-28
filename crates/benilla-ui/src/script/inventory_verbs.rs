//! Four verbs of the reference's inventory registrar (`0x8484d0`) beside the paper-doll getters in
//! [`super::char_stats`]: the keyring's slot map, the doll's sell cursor, the doll portrait setter
//! and the alert recompute.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;
use super::char_stats::{inventory_slot_reader_accepts, recompute_inventory_alerts};
use super::container::UiCursorMode;
use super::region::{clear_texture, this_texture};
use super::Model;

/// `0x4c8520`, the registrar's shared slot reader: `is-number`, `tonumber`, the truncating cast,
/// then `dec` into the 0-based slot and the whitelist; a failure of either raises `message`. The
/// 0-based slot, `-1` for Lua 0, the ammo leg.
fn inventory_slot_arg(lua: &Lua, v: Value, message: &'static str) -> mlua::Result<i32> {
    let slot0 = number_arg(lua, v, message)?.wrapping_sub(1);
    if !inventory_slot_reader_accepts(slot0) {
        return Err(mlua::Error::RuntimeError(message.into()));
    }
    Ok(slot0)
}

/// Register the four globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // KeyRingButtonIDToInvSlotID(buttonID) (`0x4c8150`): the truncated button id plus `0x51`
    // (`0x4c818b`), so key 1 is live-API slot 82, with no range check; a non-number raises.
    g.set(
        "KeyRingButtonIDToInvSlotID",
        lua.create_function(|lua, id: Value| {
            let id = number_arg(lua, id, "Usage: KeyRingButtonIDToInvSlotID(buttonID)")?;
            Ok(i64::from(id.wrapping_add(0x51)))
        })?,
    )?;

    // ShowInventorySellCursor(slot) (`0x4c90b0`): the Buy cursor over a worn item. The armed-spell
    // and base-cursor gates run before the argument is read (`0x4c90b7`, `0x4c90c9`), so under
    // either a bad slot does not raise; past them a slot outside the reader's whitelist raises,
    // and the ammo slot, an empty slot or a locked item leave the cursor as it was. Zero returns.
    // The stock doll's call is commented out (`PaperDollFrame.lua:754-756`).
    g.set(
        "ShowInventorySellCursor",
        lua.create_function(|lua, slot: Value| {
            {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                // `[0xbe2c4c] == 1`: repair mode (`0x4fbce3`) and the targeting arm move the base.
                if model.spell_targeting || model.repair_mode {
                    return Ok(());
                }
            }
            let slot0 = inventory_slot_arg(
                lua,
                slot,
                "Invalid inventory slot in ShowInventorySellCursor",
            )?;
            if slot0 == -1 {
                return Ok(());
            }
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let sellable = usize::try_from(slot0 + 1)
                .ok()
                .and_then(|slot| model.inv_slot("player", slot))
                .is_some_and(|v| v.item_id != 0 && !v.locked);
            if sellable {
                model.ui_cursor = Some(UiCursorMode::Buy);
                model.ui_cursor_dirty = true;
            }
            Ok(())
        })?,
    )?;

    // SetInventoryPortaitTexture(texture, unit, slot) (`0x4c9150`, the reference's spelling):
    // the texture is cleared (`0x4c91ed`) before arguments 2 and 3 are read, each of which raises
    // on a bad value. The unit is then read from argument 1 (`0x4c9247`), the texture itself,
    // which `lua_tostring` answers NULL for, so the token resolves no unit (`0x515940`) and the
    // item's icon is never painted: the call only blanks the texture. No stock caller.
    g.set(
        "SetInventoryPortaitTexture",
        lua.create_function(|lua, (texture, unit, slot): (Value, Value, Value)| {
            let rh = this_texture(lua, &texture)?;
            clear_texture(
                &mut lua.app_data_mut::<Model>().expect("model app_data"),
                rh,
            );
            if !matches!(
                unit,
                Value::String(_) | Value::Number(_) | Value::Integer(_)
            ) {
                return Err(mlua::Error::runtime(
                    "Usage: SetInventoryPortaitTexture(texure, unit, slot)",
                ));
            }
            inventory_slot_arg(
                lua,
                slot,
                "Invalid inventory slot in SetInventoryPortaitTexture",
            )?;
            Ok(())
        })?,
    )?;

    // UpdateInventoryAlertStatus() (`0x4c9560`): the alert recompute (`0x4c7ee0`), which fires
    // UPDATE_INVENTORY_ALERTS unconditionally; zero returns. Stock calls it when the durability
    // figure's enchant timer runs out (`DurabilityFrame.lua:81`).
    g.set(
        "UpdateInventoryAlertStatus",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            recompute_inventory_alerts(&mut model);
            model
                .pending_events
                .push(("UPDATE_INVENTORY_ALERTS".to_string(), Vec::new()));
            Ok(MultiValue::new())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::{ContainerSlot, ContainerState, InvSlotView, UiCursorMode, UiScript};

    fn keyring_with_key() -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            2,
            ContainerSlot {
                item_id: 5396,
                texture: Some("Interface\\Icons\\INV_Misc_Key_03".into()),
                count: 1,
                link: Some("|cffffffff|Hitem:5396:0:0:0|h[Key to Searing Gorge]|h|r".into()),
                ..Default::default()
            },
        );
        ContainerState {
            name: None,
            num_slots: 4,
            slots,
        }
    }

    #[test]
    fn keyring_button_ids_map_past_the_buyback_band() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("return KeyRingButtonIDToInvSlotID(1)")
                .unwrap(),
            82
        );
        assert_eq!(
            s.eval::<i64>("return KeyRingButtonIDToInvSlotID(32)")
                .unwrap(),
            113
        );
        // No range check, a truncating cast and a numeric string, as `0x4c8150` reads them.
        assert_eq!(
            s.eval::<i64>("return KeyRingButtonIDToInvSlotID(0)")
                .unwrap(),
            81
        );
        assert_eq!(
            s.eval::<i64>("return KeyRingButtonIDToInvSlotID(-2.9)")
                .unwrap(),
            79
        );
        assert_eq!(
            s.eval::<i64>("return KeyRingButtonIDToInvSlotID('3')")
                .unwrap(),
            84
        );
        let err = s
            .run("KeyRingButtonIDToInvSlotID()")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: KeyRingButtonIDToInvSlotID(buttonID)"),
            "{err}"
        );
    }

    /// The keyring band answers the inventory API from the keyring container, so the stock hover
    /// (`ContainerFrame.lua:617`) reads a key through `GetInventoryItem*` and the tooltip.
    #[test]
    fn a_key_reads_through_its_inventory_slot() {
        let mut s = UiScript::new().unwrap();
        s.set_container(-2, Some(keyring_with_key()));
        assert_eq!(
            s.eval::<Option<String>>(
                "return GetInventoryItemTexture('player', KeyRingButtonIDToInvSlotID(2))"
            )
            .unwrap()
            .as_deref(),
            Some("Interface\\Icons\\INV_Misc_Key_03")
        );
        assert_eq!(
            s.eval::<Option<String>>("return GetInventoryItemTexture('player', 82)")
                .unwrap(),
            None,
            "key slot 1 is empty"
        );
        assert_eq!(
            s.eval::<i64>("return GetInventoryItemCount('player', 83)")
                .unwrap(),
            1
        );
    }

    #[test]
    fn inventory_sell_cursor_buys_over_a_worn_item_only() {
        let mut s = UiScript::new().unwrap();
        let mut slots: crate::script::InventorySlots = Default::default();
        slots[16] = Some(InvSlotView {
            item_id: 2361,
            ..Default::default()
        });
        slots[17] = Some(InvSlotView {
            item_id: 2362,
            locked: true,
            ..Default::default()
        });
        s.set_inventory_slots(slots);

        for quiet in ["0", "1", "17"] {
            s.run(&format!("ShowInventorySellCursor({quiet})")).unwrap();
            assert_eq!(s.ui_cursor(), None, "slot {quiet}: ammo, empty or locked");
        }
        s.run("ShowInventorySellCursor(16)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));
        assert_eq!(
            s.eval::<i64>(
                "local n = function(...) return arg.n end return n(ShowInventorySellCursor(16))"
            )
            .unwrap(),
            0
        );

        for bad in ["24", "39", "70", "114", "-1", "'x'"] {
            let err = s
                .run(&format!("ShowInventorySellCursor({bad})"))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("Invalid inventory slot in ShowInventorySellCursor"),
                "{bad}: {err}"
            );
        }
    }

    /// The two cursor gates come before the argument read (`0x4c90b7`, `0x4c90c9`).
    #[test]
    fn inventory_sell_cursor_under_repair_mode_is_silent() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(crate::script::MerchantState {
            can_repair: true,
            ..Default::default()
        }));
        s.run("ShowRepairCursor()").unwrap();
        s.run("ShowInventorySellCursor(999)").unwrap();
        assert_eq!(s.ui_cursor(), None);
    }

    #[test]
    fn inventory_portrait_setter_only_blanks_the_texture() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"local f = CreateFrame("Frame", "PortHost", UIParent)
               PortTex = f:CreateTexture("PortTex")
               PortTex:SetTexture("Interface\\Icons\\INV_Misc_Bag_08")
               PortText = f:CreateFontString("PortText")"#,
        )
        .unwrap();
        s.run("SetInventoryPortaitTexture(PortTex, 'player', 1)")
            .unwrap();
        assert_eq!(
            s.eval::<Option<String>>("return PortTex:GetTexture()")
                .unwrap(),
            None
        );
        assert_eq!(
            s.eval::<i64>("local n = function(...) return arg.n end return n(SetInventoryPortaitTexture(PortTex, 'player', 1))")
                .unwrap(),
            0
        );

        // The clear precedes the argument checks (`0x4c91ed`).
        s.run(r#"PortTex:SetTexture("Interface\\Icons\\INV_Misc_Bag_08")"#)
            .unwrap();
        let err = s
            .run("SetInventoryPortaitTexture(PortTex, nil, 1)")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: SetInventoryPortaitTexture(texure, unit, slot)"),
            "{err}"
        );
        assert_eq!(
            s.eval::<Option<String>>("return PortTex:GetTexture()")
                .unwrap(),
            None
        );
        let err = s
            .run("SetInventoryPortaitTexture(PortTex, 'player', 30)")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Invalid inventory slot in SetInventoryPortaitTexture"),
            "{err}"
        );

        for (call, message) in [
            (
                "SetInventoryPortaitTexture(1, 'player', 1)",
                "non-table object",
            ),
            (
                "SetInventoryPortaitTexture({}, 'player', 1)",
                "non-framescript object",
            ),
            (
                "SetInventoryPortaitTexture(PortText, 'player', 1)",
                "Wrong object type for member function",
            ),
            (
                "SetInventoryPortaitTexture(PortHost, 'player', 1)",
                "Wrong object type for member function",
            ),
        ] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains(message), "{call}: {err}");
        }
    }

    #[test]
    fn update_inventory_alert_status_fires_the_recompute_event() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"ALERTS = 0
               local f = CreateFrame("Frame")
               f:RegisterEvent("UPDATE_INVENTORY_ALERTS")
               f:SetScript("OnEvent", function() ALERTS = ALERTS + 1 end)"#,
        )
        .unwrap();
        assert_eq!(
            s.eval::<i64>(
                "local n = function(...) return arg.n end return n(UpdateInventoryAlertStatus())"
            )
            .unwrap(),
            0
        );
        s.run("UpdateInventoryAlertStatus()").unwrap();
        s.tick(0.0);
        assert_eq!(s.eval::<i64>("return ALERTS").unwrap(), 2);
    }
}
