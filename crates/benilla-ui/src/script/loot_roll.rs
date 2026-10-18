//! The group loot roll bindings `GroupLootFrame` calls: the app pushes the open rolls and drains
//! the votes `RollOnLoot` queues, 0 Pass, 1 Need, 2 Greed (`LootFrame.xml:374`/`:398`/`:425`).
//! `START_LOOT_ROLL` carries `(rollID, rollTime)` and `CANCEL_LOOT_ROLL` the `rollID`
//! (`UIParent.lua:513-515`, `LootFrame.lua:279-285`); a vote's own cancel, and the
//! bind-on-pickup confirm, fire inside the verb. The `rollID` is client-internal, as
//! `CMSG_LOOT_ROLL` addresses `(lootedTarget, itemSlot)`, so the app allocates its own.

use mlua::{Lua, Value};

use super::binding_abi::{self, flag};
use super::{Model, ScriptValue};
use crate::script::binding_abi::number_arg;

/// `rollType` 0, the one vote the bind-on-pickup gate never intercepts.
const PASS: i32 = 0;

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
    /// Item quality 0..6; `None` while the template is in flight, answered as `CACHE_MISS_QUALITY`.
    pub quality: Option<u32>,
    /// Bind on pickup, which swaps in the gold backdrop; `false` while the template is in flight.
    pub bind_on_pickup: bool,
    /// Milliseconds left, from `SMSG_LOOT_START_ROLL`'s countdown; saturates at 0.
    pub time_left_ms: u32,
    /// The item id, the key of the hover's item lookup.
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
}

/// `0x61bdf0`, the vote both bindings share: an unknown or closed roll does nothing
/// (`0x61be3f`). The type is not range-tested: any nonzero one takes the bind-on-pickup gate, and
/// the wire carries its low byte (`0x418070`); the server ignores one of 3 or more
/// (`GroupHandler.cpp:367`). Need or Greed on a bind-on-pickup item, unconfirmed, signals
/// `CONFIRM_LOOT_ROLL` with `(rollID, rollType)` and sends nothing (`0x61be8b`). Otherwise the
/// vote is queued for `CMSG_LOOT_ROLL` and `CANCEL_LOOT_ROLL` fires (`0x61bf2a`) before the roll
/// closes (`0x61bf32`), so its handler still reads the roll; the app's later close of the same
/// roll then signals nothing, as the closed byte `+0x3a` guards it.
fn vote(lua: &Lua, roll_id: u32, roll_type: i32, confirmed: bool) {
    let closes = super::tick::verb(lua, |model| {
        let Some(entry) = find(model, roll_id) else {
            return false;
        };
        let id = ScriptValue::Int(i64::from(roll_id));
        if entry.bind_on_pickup && roll_type != PASS && !confirmed {
            model.raise(
                "CONFIRM_LOOT_ROLL",
                vec![id, ScriptValue::Int(i64::from(roll_type))],
            );
            return false;
        }
        model.loot_roll_votes.push((roll_id, roll_type as u8));
        model.raise("CANCEL_LOOT_ROLL", vec![id]);
        true
    });
    if closes {
        super::tick::verb(lua, |model| {
            model.loot_rolls.rolls.retain(|r| r.roll_id != roll_id);
        });
    }
}

fn find(model: &Model, roll_id: u32) -> Option<&LootRollEntry> {
    model.loot_rolls.rolls.iter().find(|r| r.roll_id == roll_id)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x4c3050`, read at `LootFrame.lua:261`. `count` is the literal 1.0 (`0x4c3160`) and
    // `bindOnPickup` 1.0 when `[template+0x194] == 1` else nil (`0x4c317e`). An unknown id takes
    // the reference's miss tail `nil, nil, 1.0, 1.0, nil` (`0x4c31a3`), as a roll with no item
    // template does there.
    g.set(
        "GetLootRollItemInfo",
        lua.create_function(|lua, roll_id: Value| {
            // `is-number` else `Usage: GetLootRollItemInfo(id)` (`0x8480e0`, `0x4c307e`).
            let roll_id = binding_abi::index_arg32(lua, roll_id, "Usage: GetLootRollItemInfo(id)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(r) = find(&model, roll_id) else {
                return lua.pack_multi((
                    mlua::Value::Nil,
                    mlua::Value::Nil,
                    1,
                    CACHE_MISS_QUALITY,
                    mlua::Value::Nil,
                ));
            };
            let vals = (
                r.texture.clone(),
                r.name.clone(),
                1,
                r.quality.map_or(CACHE_MISS_QUALITY, i64::from),
                flag(r.bind_on_pickup),
            );
            lua.pack_multi(vals)
        })?,
    )?;

    // `nil` for an unknown id or while the item template is in flight.
    g.set(
        "GetLootRollItemLink",
        lua.create_function(|lua, roll_id: Value| {
            // `is-number` else `Usage: GetLootRollItemLink(id)` (`0x848100`, `0x4c321e`).
            let roll_id = binding_abi::index_arg32(lua, roll_id, "Usage: GetLootRollItemLink(id)")?;
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
        lua.create_function(|lua, roll_id: Value| {
            // `is-number` else `Usage: GetLootRollTimeLeft(id)` (`0x848120`, `0x4c32e8`).
            let roll_id = binding_abi::index_arg32(lua, roll_id, "Usage: GetLootRollTimeLeft(id)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(find(&model, roll_id).map_or(0, |r| r.time_left_ms))
        })?,
    )?;

    // Both arguments are numbers (`0x4c3370`, `0x6f34d0`), else the usage line; an unknown id is
    // dropped. The bind-on-pickup gate is in the C function ([`vote`]), so it holds for an
    // addon's call too.
    g.set(
        "RollOnLoot",
        lua.create_function(|lua, (roll_id, roll_type): (Value, Value)| {
            let usage = "Usage: RollOnLoot(id, rollType)";
            let id = number_arg(lua, roll_id, usage)? as u32;
            let ty = number_arg(lua, roll_type, usage)?;
            vote(lua, id, ty, false);
            Ok(())
        })?,
    )?;

    // The gate's bypass: `0x4c33e0` re-enters `0x61bdf0` with a third argument of 1.
    g.set(
        "ConfirmLootRoll",
        lua.create_function(|lua, (roll_id, roll_type): (Value, Value)| {
            let usage = "Usage: ConfirmLootRoll(id, rollType)";
            let id = number_arg(lua, roll_id, usage)? as u32;
            let ty = number_arg(lua, roll_type, usage)?;
            vote(lua, id, ty, true);
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
            .eval::<(String, String, i64, i64, i64)>("return GetLootRollItemInfo(7)")
            .unwrap();
        assert_eq!(texture, "Interface\\Icons\\INV_Staff_12");
        assert_eq!(name, "Staff of Jordan");
        assert_eq!((count, quality, bop), (1, 4, 1));
        assert_eq!(
            s.eval::<i64>("return GetLootRollTimeLeft(7)").unwrap(),
            42_000
        );
        // Five values, a hit and the miss tail alike (`0x4c31a3`).
        for id in [7, 99] {
            let vals = s
                .eval::<mlua::MultiValue>(&format!("return GetLootRollItemInfo({id})"))
                .unwrap();
            assert_eq!(vals.len(), 5, "roll {id}");
        }

        // In flight: the quality is the miss tail's 1, never nil; the count is the literal 1
        // whatever the stack, and a roll that does not bind answers nil, not false.
        assert!(s
            .eval::<bool>(
                "local t, n, c, q, b = GetLootRollItemInfo(8)\n\
                 return t == nil and n == nil and q == 1 and c == 1 and b == nil",
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local _, _, _, _, b7 = GetLootRollItemInfo(7)\n\
                 return type(b7) == 'number' and b7 == 1",
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
            5,
            "the reference's five returns (`0x4c31a3`)"
        );
        assert!(s
            .eval::<bool>("return GetLootRollItemLink(99) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetLootRollTimeLeft(99)").unwrap(), 0);
    }

    /// A vote signals `CANCEL_LOOT_ROLL` inside the verb (`0x61bf2a`), its handler still reading
    /// the roll, which closes after it (`0x61bf32`): a second vote on it does nothing (`0x61be3f`).
    #[test]
    fn a_vote_signals_its_cancel_in_place_and_closes_the_roll() {
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        s.set_loot_rolls(rolls());
        heard::listen(
            &s,
            &["CANCEL_LOOT_ROLL", "CONFIRM_LOOT_ROLL"],
            Some("arg1 .. ' ' .. GetLootRollTimeLeft(arg1)"),
        );
        // Roll 8 is not bind-on-pickup, so the Need goes straight out.
        s.run("RollOnLoot(8, 1) RollOnLoot(8, 2)").unwrap();
        assert_eq!(heard::take(&s), ["CANCEL_LOOT_ROLL 8 60000"]);
        assert_eq!(s.take_loot_roll_votes(), vec![(8, 1)]);
        assert_eq!(s.eval::<i64>("return GetLootRollTimeLeft(8)").unwrap(), 0);
    }

    /// Need or Greed on a bind-on-pickup roll signals `CONFIRM_LOOT_ROLL` with
    /// `(rollID, rollType)` and sends nothing (`0x61be8b`); Pass and the confirm vote.
    #[test]
    fn need_or_greed_on_a_bop_roll_confirms_instead_of_voting() {
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        s.set_loot_rolls(rolls()); // roll 7 is bind_on_pickup
        heard::listen(
            &s,
            &["CANCEL_LOOT_ROLL", "CONFIRM_LOOT_ROLL"],
            Some("arg1 .. ',' .. tostring(arg2)"),
        );
        s.run("RollOnLoot(7, 1) RollOnLoot(7, 2)").unwrap();
        assert!(
            s.take_loot_roll_votes().is_empty(),
            "a BoP need/greed must not reach the wire before the popup is accepted"
        );
        assert_eq!(
            heard::take(&s),
            ["CONFIRM_LOOT_ROLL 7,1", "CONFIRM_LOOT_ROLL 7,2"]
        );

        // `ConfirmLootRoll`, the popup's accept, lands the real vote and closes the roll.
        s.run("ConfirmLootRoll(7, 1) RollOnLoot(7, 0)").unwrap();
        assert_eq!(s.take_loot_roll_votes(), vec![(7, 1)]);
        assert_eq!(heard::take(&s), ["CANCEL_LOOT_ROLL 7,nil"]);
    }

    /// Pass is never gated.
    #[test]
    fn a_pass_on_a_bop_roll_votes() {
        let mut s = UiScript::new().unwrap();
        s.set_loot_rolls(rolls());
        s.run("RollOnLoot(7, 0)").unwrap();
        assert_eq!(s.take_loot_roll_votes(), vec![(7, 0)]);
    }

    #[test]
    fn confirm_still_validates() {
        let mut s = UiScript::new().unwrap();
        s.set_loot_rolls(rolls());
        s.run("ConfirmLootRoll(99, 1)").unwrap(); // no such roll
        assert!(s.take_loot_roll_votes().is_empty());
    }

    /// `0x61bdf0` has no range test on the type: a type the server ignores (`GroupHandler.cpp:367`)
    /// still goes out and closes the roll with its `CANCEL_LOOT_ROLL` (`0x61bf2a`), a bind-on-pickup
    /// one takes the confirm branch (`0x61be8b`) with the type as given, and the wire carries the
    /// low byte (`0x418070`). Only an unknown id is dropped.
    #[test]
    fn any_roll_type_is_sent_and_closes_the_roll() {
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        s.set_loot_rolls(rolls());
        heard::listen(
            &s,
            &["CANCEL_LOOT_ROLL", "CONFIRM_LOOT_ROLL"],
            Some("arg1 .. ',' .. tostring(arg2)"),
        );
        s.run("RollOnLoot(99, 1) RollOnLoot(99, 3)").unwrap(); // no such roll
        assert!(s.take_loot_roll_votes().is_empty());
        assert!(heard::take(&s).is_empty());
        s.run("RollOnLoot(8, 3)").unwrap();
        assert_eq!(s.take_loot_roll_votes(), vec![(8, 3)]);
        assert_eq!(heard::take(&s), ["CANCEL_LOOT_ROLL 8,nil"]);
        assert_eq!(s.eval::<i64>("return GetLootRollTimeLeft(8)").unwrap(), 0);
        s.run("RollOnLoot(7, 200)").unwrap();
        assert!(s.take_loot_roll_votes().is_empty());
        assert_eq!(heard::take(&s), ["CONFIRM_LOOT_ROLL 7,200"]);
        s.run("ConfirmLootRoll(7, 300)").unwrap();
        assert_eq!(s.take_loot_roll_votes(), vec![(7, 44)]);
        assert_eq!(heard::take(&s), ["CANCEL_LOOT_ROLL 7,nil"]);
    }

    /// A non-number argument raises the binding's usage line (`0x848140`, `0x848160`).
    #[test]
    fn roll_bindings_raise_their_usage_on_a_non_number() {
        let s = UiScript::new().unwrap();
        for (call, usage) in [
            ("RollOnLoot(1)", "Usage: RollOnLoot(id, rollType)"),
            ("RollOnLoot('x', 1)", "Usage: RollOnLoot(id, rollType)"),
            (
                "ConfirmLootRoll(1, nil)",
                "Usage: ConfirmLootRoll(id, rollType)",
            ),
        ] {
            let e = s
                .eval::<String>(&format!(
                    "local ok, e = pcall(function() {call} end) return tostring(e)"
                ))
                .unwrap();
            assert!(e.contains(usage), "{call}: {e}");
        }
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
