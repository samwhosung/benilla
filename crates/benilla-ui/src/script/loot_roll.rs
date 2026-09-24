//! The group loot roll bindings `GroupLootFrame` calls: the app pushes the open rolls and drains
//! the votes `RollOnLoot` queues, 0 Pass, 1 Need, 2 Greed (`LootFrame.xml:374`/`:398`/`:425`).
//! `START_LOOT_ROLL` carries `(rollID, rollTime)` and `CANCEL_LOOT_ROLL` the `rollID`
//! (`UIParent.lua:513-515`, `LootFrame.lua:279-285`). The `rollID` is client-internal, as
//! `CMSG_LOOT_ROLL` addresses `(lootedTarget, itemSlot)`, so the app allocates its own.

use mlua::Lua;

use super::Model;

/// `rollType` 0, the one vote the bind-on-pickup gate never intercepts.
const PASS: u8 = 0;

/// The quality `GetLootRollItemInfo` answers on an item-template miss, from the reference's miss
/// tail `nil, nil, 1.0, 1.0, nil` (`0x4c31a3`): 1, not `GetLootSlotInfo`'s -1. Stock
/// `GroupLootFrame_OnShow` indexes `ITEM_QUALITY_COLORS` with it unguarded
/// (`LootFrame.lua:275-276`).
const CACHE_MISS_QUALITY: i64 = 1;

/// One open group loot roll, resolved by the app and pushed whole each frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LootRollEntry {
    /// The client-internal `rollID`, allocated by the app and stable for the roll's life.
    pub roll_id: u32,
    /// `None`, answered as `nil`, while the item-template query is in flight.
    pub name: Option<String>,
    /// Icon texture path (`Interface\Icons\…`). `None` only if the display catalog had no icon.
    pub texture: Option<String>,
    /// The stack size, answered as `GetLootRollItemInfo`'s `count`; the reference answers a literal
    /// `1.0` there, a group roll being one stack (`0x4c3160`).
    pub quantity: u32,
    /// Item quality 0..6; `None` while the template is in flight, answered as `CACHE_MISS_QUALITY`.
    pub quality: Option<u32>,
    /// Bind on pickup, which swaps in the gold backdrop; `false` while the template is in flight.
    pub bind_on_pickup: bool,
    /// Milliseconds left, from `SMSG_LOOT_START_ROLL`'s countdown; saturates at 0.
    pub time_left_ms: u32,
    /// The item id, the key of the hover's item lookup; `GetLootRollItemInfo` also returns it as a
    /// sixth value, which 1.12 does not.
    pub item_id: u32,
    /// The link `GetLootRollItemLink` answers for the icon's ctrl/shift clicks
    /// (`LootFrame.xml:353-361`); `None` until the item template lands, since it embeds the name.
    pub link: Option<String>,
    /// `SMSG_LOOT_START_ROLL`'s random suffix, 0 for none: the hover's only enchant source, as
    /// `SetLootRollItem` (`0x5364a0`) copies it to the tooltip's `+0x424` with no item object.
    pub random_property_id: u32,
}

/// Every open group loot roll, in the order the app opened them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LootRollsState {
    pub rolls: Vec<LootRollEntry>,
}

impl super::UiScript {
    /// Push the open-rolls snapshot (an empty list means no roll is open).
    pub fn set_loot_rolls(&mut self, state: LootRollsState) {
        self.model_mut().loot_rolls = state;
    }

    /// Drain the `(roll_id, roll_type)` votes queued by `RollOnLoot` since the last call. The app
    /// maps each roll id back to its `(lootedTarget, itemSlot)` for `CMSG_LOOT_ROLL`.
    pub fn take_loot_roll_votes(&mut self) -> Vec<(u32, u8)> {
        std::mem::take(&mut self.model_mut().loot_roll_votes)
    }

    /// Drain the `(roll_id, roll_type)` Need or Greed calls on a bind-on-pickup roll: the app fires
    /// `CONFIRM_LOOT_ROLL`, and the popup's `ConfirmLootRoll` queues the real vote.
    pub fn take_loot_roll_confirms(&mut self) -> Vec<(u32, u8)> {
        std::mem::take(&mut self.model_mut().loot_roll_confirms)
    }
}

fn find(model: &Model, roll_id: u32) -> Option<&LootRollEntry> {
    model.loot_rolls.rolls.iter().find(|r| r.roll_id == roll_id)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x4c3050`, read at `LootFrame.lua:261`. An unknown id takes the reference's miss tail
    // `nil, nil, 1.0, 1.0, nil` (`0x4c31a3`), as a roll with no item template does there.
    g.set(
        "GetLootRollItemInfo",
        lua.create_function(|lua, roll_id: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(r) = find(&model, roll_id) else {
                return lua.pack_multi((
                    mlua::Value::Nil,
                    mlua::Value::Nil,
                    1,
                    CACHE_MISS_QUALITY,
                    mlua::Value::Nil,
                    mlua::Value::Nil,
                ));
            };
            let vals = (
                r.texture.clone(),
                r.name.clone(),
                r.quantity,
                r.quality.map_or(CACHE_MISS_QUALITY, i64::from),
                r.bind_on_pickup,
                r.item_id,
            );
            lua.pack_multi(vals)
        })?,
    )?;

    // `nil` for an unknown id or while the item template is in flight.
    g.set(
        "GetLootRollItemLink",
        lua.create_function(|lua, roll_id: u32| {
            let link = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                find(&model, roll_id).and_then(|r| r.link.clone())
            };
            match link {
                Some(link) => Ok(mlua::Value::String(lua.create_string(&link)?)),
                None => Ok(mlua::Value::Nil),
            }
        })?,
    )?;

    // 0 for an unknown id; the stock `OnUpdate` clamps it to the bar's minimum anyway
    // (`LootFrame.lua:290-293`).
    g.set(
        "GetLootRollTimeLeft",
        lua.create_function(|lua, roll_id: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(find(&model, roll_id).map_or(0, |r| r.time_left_ms))
        })?,
    )?;

    // The server ignores a `rollType` of 3 or more (`GroupHandler.cpp:376`), so those and unknown
    // ids are dropped. Need or Greed on a bind-on-pickup item sends nothing and fires
    // `CONFIRM_LOOT_ROLL` (`0x61bdf0`, `0x61be8b`); the gate is in the C function, so it holds for
    // an addon's call too.
    g.set(
        "RollOnLoot",
        lua.create_function(|lua, (roll_id, roll_type): (u32, u8)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let Some(entry) = find(&model, roll_id) else {
                return Ok(());
            };
            if roll_type > 2 {
                return Ok(());
            }
            if entry.bind_on_pickup && roll_type != PASS {
                model.loot_roll_confirms.push((roll_id, roll_type));
            } else {
                model.loot_roll_votes.push((roll_id, roll_type));
            }
            Ok(())
        })?,
    )?;

    // The gate's bypass: `0x4c33e0` re-enters `0x61bdf0` with a third argument of 1.
    g.set(
        "ConfirmLootRoll",
        lua.create_function(|lua, (roll_id, roll_type): (u32, u8)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if roll_type <= 2 && find(&model, roll_id).is_some() {
                model.loot_roll_votes.push((roll_id, roll_type));
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{LootRollEntry, LootRollsState};
    use crate::script::UiScript;

    fn rolls() -> LootRollsState {
        LootRollsState {
            rolls: vec![
                // A resolved BoP item: name, quality and link land together.
                LootRollEntry {
                    roll_id: 7,
                    name: Some("Staff of Jordan".into()),
                    texture: Some("Interface\\Icons\\INV_Staff_12".into()),
                    quantity: 1,
                    quality: Some(4),
                    bind_on_pickup: true,
                    time_left_ms: 42_000,
                    item_id: 17182,
                    link: Some("|cffa335ee|Hitem:17182:0:0:0|h[Staff of Jordan]|h|r".into()),
                    random_property_id: 0,
                },
                // In flight: the roll opened, the item template has not landed.
                LootRollEntry {
                    roll_id: 8,
                    name: None,
                    texture: None,
                    quantity: 2,
                    quality: None,
                    bind_on_pickup: false,
                    time_left_ms: 60_000,
                    item_id: 4306,
                    link: None,
                    random_property_id: 0,
                },
            ],
        }
    }

    #[test]
    fn roll_snapshot_reads() {
        let mut s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("return GetLootRollItemInfo(7) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetLootRollTimeLeft(7)").unwrap(), 0);

        s.set_loot_rolls(rolls());

        let (texture, name, count, quality, bop) = s
            .eval::<(String, String, i64, i64, bool)>("return GetLootRollItemInfo(7)")
            .unwrap();
        assert_eq!(texture, "Interface\\Icons\\INV_Staff_12");
        assert_eq!(name, "Staff of Jordan");
        assert_eq!((count, quality, bop), (1, 4, true));
        assert_eq!(
            s.eval::<i64>("return GetLootRollTimeLeft(7)").unwrap(),
            42_000
        );
        assert_eq!(
            s.eval::<i64>("local _, _, _, _, _, id = GetLootRollItemInfo(7)\nreturn id")
                .unwrap(),
            17182
        );

        // In flight: the quality is the miss tail's 1, never nil.
        assert!(s
            .eval::<bool>(
                "local t, n, c, q, b = GetLootRollItemInfo(8)\n\
                 return t == nil and n == nil and q == 1 and c == 2 and b == false",
            )
            .unwrap());

        assert_eq!(
            s.eval::<String>("return GetLootRollItemLink(7)").unwrap(),
            "|cffa335ee|Hitem:17182:0:0:0|h[Staff of Jordan]|h|r"
        );
        assert!(s
            .eval::<bool>("return GetLootRollItemLink(8) == nil")
            .unwrap());

        // An unknown id: the reference's miss tail (`0x4c31a3`).
        assert!(s
            .eval::<bool>("return GetLootRollItemInfo(99) == nil")
            .unwrap());
        let (count, quality) = s
            .eval::<(i64, i64)>("local _, _, c, q = GetLootRollItemInfo(99)\nreturn c, q")
            .unwrap();
        assert_eq!((count, quality), (1, 1), "the miss tail is 1, 1");
        assert_eq!(
            s.arity("GetLootRollItemInfo(99)").unwrap(),
            6,
            "five reference returns plus benilla's trailing item id"
        );
        assert!(s
            .eval::<bool>("return GetLootRollItemLink(99) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetLootRollTimeLeft(99)").unwrap(), 0);
    }

    #[test]
    fn roll_on_loot_queues_votes() {
        let mut s = UiScript::new().unwrap();
        s.set_loot_rolls(rolls());
        // Roll 8 is not bind-on-pickup, so every vote goes straight out.
        s.run("RollOnLoot(8, 1)").unwrap(); // Need
        s.run("RollOnLoot(8, 2)").unwrap(); // Greed
        s.run("RollOnLoot(8, 0)").unwrap(); // Pass
        assert_eq!(s.take_loot_roll_votes(), vec![(8, 1), (8, 2), (8, 0)]);
        assert!(s.take_loot_roll_votes().is_empty(), "drained");
        assert!(s.take_loot_roll_confirms().is_empty(), "nothing to confirm");
    }

    #[test]
    fn need_or_greed_on_a_bop_roll_confirms_instead_of_voting() {
        let mut s = UiScript::new().unwrap();
        s.set_loot_rolls(rolls()); // roll 7 is bind_on_pickup
        s.run("RollOnLoot(7, 1)").unwrap(); // Need on BoP
        s.run("RollOnLoot(7, 2)").unwrap(); // Greed on BoP
        assert!(
            s.take_loot_roll_votes().is_empty(),
            "a BoP need/greed must not reach the wire before the popup is accepted"
        );
        assert_eq!(s.take_loot_roll_confirms(), vec![(7, 1), (7, 2)]);

        // Pass is never gated.
        s.run("RollOnLoot(7, 0)").unwrap();
        assert_eq!(s.take_loot_roll_votes(), vec![(7, 0)]);
        assert!(s.take_loot_roll_confirms().is_empty());

        // `ConfirmLootRoll`, the popup's accept, lands the real vote.
        s.run("ConfirmLootRoll(7, 1)").unwrap();
        assert_eq!(s.take_loot_roll_votes(), vec![(7, 1)]);
        assert!(s.take_loot_roll_confirms().is_empty(), "no second prompt");
    }

    #[test]
    fn confirm_still_validates() {
        let mut s = UiScript::new().unwrap();
        s.set_loot_rolls(rolls());
        s.run("ConfirmLootRoll(99, 1)").unwrap(); // no such roll
        s.run("ConfirmLootRoll(7, 3)").unwrap(); // server-only rollType
        assert!(s.take_loot_roll_votes().is_empty());
    }

    #[test]
    fn bad_votes_are_dropped() {
        let mut s = UiScript::new().unwrap();
        s.set_loot_rolls(rolls());
        s.run("RollOnLoot(7, 3)").unwrap(); // ROLL_NOT_EMITED_YET, server-only
        s.run("RollOnLoot(7, 200)").unwrap();
        s.run("RollOnLoot(99, 1)").unwrap(); // no such roll
        assert!(s.take_loot_roll_votes().is_empty());
    }

    #[test]
    fn clearing_the_rolls_empties_them() {
        let mut s = UiScript::new().unwrap();
        s.set_loot_rolls(rolls());
        s.set_loot_rolls(LootRollsState::default());
        assert!(s
            .eval::<bool>("return GetLootRollItemInfo(7) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetLootRollTimeLeft(7)").unwrap(), 0);
    }
}
