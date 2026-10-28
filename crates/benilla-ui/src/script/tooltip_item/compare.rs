//! The two compare bindings, `SetMerchantCompareItem` and `SetAuctionCompareItem`: the offered
//! item beside the worn one it would replace, through the builder with the header latch armed.

use mlua::{Lua, Table, Value};

use super::build;
use super::view_of;
use crate::script::object::frame_handle_of;
use crate::script::{ItemTemplateView, Model};

/// The candidate slots to pass over, from `offset`: `is-number`, `_ftol` and `dec` (`0x53620e`-
/// `0x536228`, `0x535f73`-`0x535f8d`), absent 1. The countdown stops at 0 (`0x536271`), so a
/// negative one never matches: `None`.
fn offset_left(lua: &Lua, offset: Option<&Value>) -> Option<i32> {
    let left = offset
        .and_then(|v| crate::script::binding_abi::is_number(lua, v.clone()))
        .map_or(0, |n| crate::script::binding_abi::ftol(n).wrapping_sub(1));
    (left >= 0).then_some(left)
}

/// The body both compare bindings run (`0x536080`, `0x535d70`): walk `0x809200[InventoryType]`'s
/// slots, skipping empty ones uncounted and ones whose worn item's class differs (`0x5d9f30`),
/// until `left` matches have passed; then run `SetInventoryItem` there with the header latch armed,
/// as `0x536080` calls `0x52b650` with p4 = 0, p5 = 1. The number 1, or nil on a miss, which leaves
/// the tooltip untouched.
fn compare_against_worn(
    lua: &Lua,
    this: &Table,
    h: crate::widget::FrameHandle,
    offered: &ItemTemplateView,
    mut left: f64,
) -> mlua::Result<Value> {
    let found = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let mut hit = None;
        for &slot in equip_slots_for(offered.inventory_type) {
            let Some(worn) = crate::script::char_stats::own_inv_view(lua, &model, slot as usize)
                .filter(|s| s.item_id != 0)
            else {
                continue; // an empty slot is not counted
            };
            let same_class = model
                .item_templates
                .get(&worn.item_id)
                .is_some_and(|w| w.class == offered.class);
            if !same_class {
                continue;
            }
            if left <= 0.0 {
                hit = Some(slot as usize);
                break;
            }
            left -= 1.0;
        }
        hit
    };
    let Some(slot) = found else {
        return Ok(Value::Nil);
    };
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if let Ok(t) = crate::script::tooltip::tip_mut(&mut model, h) {
            t.equipped_header_armed = true;
        }
    }
    this.get::<mlua::Function>("SetInventoryItem")?
        .call::<mlua::MultiValue>((this.clone(), "player", slot))?;
    Ok(Value::Integer(1))
}

/// A compare's offered template; a miss asks with the tooltip as the answer's cookie (`0x5361ee`),
/// so the answer rebuilds whatever item the tooltip keeps then.
fn offered_view(
    lua: &Lua,
    h: crate::widget::FrameHandle,
    item_id: u32,
) -> Option<ItemTemplateView> {
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .item_cache_misses
        .clear();
    let v = view_of(lua, item_id);
    build::keep_misses(
        &mut lua.app_data_mut::<Model>().expect("model app_data"),
        h,
        false,
    );
    build::ask_tooltip_records(lua);
    v
}

/// An InventoryType's compare candidate slots, as `GetInventorySlotInfo` ids: the reference's
/// mask table `0x809200[InventoryType]`.
fn equip_slots_for(inventory_type: u32) -> &'static [u32] {
    match inventory_type {
        1 => &[1],             // head
        2 => &[2],             // neck
        3 => &[3],             // shoulder
        4 => &[4],             // shirt
        5 | 20 => &[5],        // chest / robe
        6 => &[6],             // waist
        7 => &[7],             // legs
        8 => &[8],             // feet
        9 => &[9],             // wrist
        10 => &[10],           // hands
        11 => &[11, 12],       // finger
        12 => &[13, 14],       // trinket
        16 => &[15],           // back
        19 => &[19],           // tabard
        13 => &[16, 17],       // one-hand
        21 => &[16],           // main hand
        14 | 22 | 23 => &[17], // shield / off-hand weapon / held
        // Two-hand: main hand only, though a two-hander displaces both: `0x809200[17]` is
        // `0x8000`, bit 15.
        17 => &[16],
        15 | 25 | 26 | 28 => &[18], // bow / thrown / wand-gun / relic
        _ => &[],
    }
}

/// Register the two compare bindings into the GameTooltip kind method table.
pub(super) fn install_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // GameTooltip:SetMerchantCompareItem(index [, offset]) (`0x536080`, method-table entry 43):
    // the shopping tooltip beside a vendor row. `offset` counts candidate slots, 1-based: set in
    // `0x809200[InventoryType]`, occupied, and holding an item of the offered class (never
    // subclass), so a worn shield makes offset 2 nil for a one-hand weapon and a worn wand compares
    // against a bow. One value on every non-raising path: the number 1 (`lua_pushnumber`) or nil.
    // A bad `this` or a non-number index raises; a missing `offset` is 1, one of 0 or less is nil.
    // An uncached template answers nil and fires the query, as in the reference.
    m.set(
        "SetMerchantCompareItem",
        lua.create_function(
            |lua, (this, index, offset): (Table, Value, Option<Value>)| {
                let h = frame_handle_of(lua, &this)?;
                // The one argument whose absence raises, with the reference's usage text.
                // `lua_isnumber` (`0x6f34d0`): a numeric string as `luaO_str2d` reads it.
                let Some(idx) = crate::script::binding_abi::is_number(lua, index) else {
                    return Err(mlua::Error::runtime(
                        "Usage: SetMerchantCompareItem(\"slot\" [, offset])",
                    ));
                };
                // Re-based in f64, then `_ftol` (`0x536166`, `0x53616c`) and a signed test: 0.5 is
                // row 1, and so is NaN.
                let Ok(row) = usize::try_from(crate::script::binding_abi::ftol(idx - 1.0)) else {
                    return Ok(Value::Nil);
                };
                let Some(left) = offset_left(lua, offset.as_ref()) else {
                    return Ok(Value::Nil);
                };

                let Some(item_id) = ({
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    crate::script::merchant::row(lua, &model, row).map(|it| it.item_id)
                }) else {
                    return Ok(Value::Nil);
                };
                let Some(offered) = offered_view(lua, h, item_id) else {
                    return Ok(Value::Nil);
                };

                compare_against_worn(lua, &this, h, &offered, f64::from(left))
            },
        )?,
    )?;

    // GameTooltip:SetAuctionCompareItem("type", index [, offset]) (`0x535d70`): the vendor
    // compare's body (`0x53603e` and `0x5362d4` both pass p4 = 0, p5 = 1) over an auction list.
    // `type` is `lua_isstring`-gated (`0x535e28`); a non-string type or non-number index raises
    // the reference's usage, and a bad list, row or uncached template is nil. The number 1 on
    // success, which `Blizzard_AuctionUI.lua:1081` gates each shopping tooltip on.
    m.set(
        "SetAuctionCompareItem",
        lua.create_function(
            |lua, (this, kind, index, offset): (Table, Value, Value, Option<Value>)| {
                let h = frame_handle_of(lua, &this)?;
                // `lua_isstring` then `lua_tostring` (`0x535e28`, `0x535e50`).
                let Some(kind) = crate::script::binding_abi::optional_string(lua, &kind) else {
                    return Err(mlua::Error::runtime(
                        "Usage: SetAuctionCompareItem(\"type\", index [, offset])",
                    ));
                };
                // `lua_isnumber` (`0x6f34d0`): a numeric string as `luaO_str2d` reads it.
                let Some(idx) = crate::script::binding_abi::is_number(lua, index) else {
                    return Err(mlua::Error::runtime(
                        "Usage: SetAuctionCompareItem(\"type\", index [, offset])",
                    ));
                };
                // `_ftol` then `dec` (`0x535e64`, `0x535e79`), compared unsigned with the list's
                // count: 0.5 is row 0, past every list.
                let row = (crate::script::binding_abi::ftol(idx) as u32).wrapping_sub(1) as usize;
                let Some(left) = offset_left(lua, offset.as_ref()) else {
                    return Ok(Value::Nil);
                };
                let Some(item_id) = ({
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    let list = crate::script::auction::list_index_of(&kind);
                    model
                        .auction
                        .as_ref()
                        .zip(list)
                        .and_then(|(a, l)| a.lists[l].rows.get(row))
                        .map(|r| r.item_id)
                }) else {
                    return Ok(Value::Nil);
                };
                let Some(offered) = offered_view(lua, h, item_id) else {
                    return Ok(Value::Nil);
                };
                compare_against_worn(lua, &this, h, &offered, f64::from(left))
            },
        )?,
    )?;

    Ok(())
}
