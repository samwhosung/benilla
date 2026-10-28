//! The group loot roll bindings `GroupLootFrame` calls: the getters read the engine's list of
//! rolls through the lent host at the call ([`LootRollQuery`]), as the reference's find the roll
//! by id (`0x61b2e0`) and look its item up as they run (`0x4c30d2`); the VM's own copy
//! ([`super::UiScript::set_loot_rolls`]) answers only with no host to ask. The votes `RollOnLoot`
//! makes, 0 Pass, 1 Need, 2 Greed (`LootFrame.xml:374`/`:398`/`:425`), apply inside their calls.
//! `START_LOOT_ROLL` carries `(rollID, rollTime)` and `CANCEL_LOOT_ROLL` the `rollID`
//! (`UIParent.lua:513-515`, `LootFrame.lua:279-285`); a vote's own cancel, and the
//! bind-on-pickup confirm, fire inside the verb. The `rollID` is client-internal, as
//! `CMSG_LOOT_ROLL` addresses `(lootedTarget, itemSlot)`, so the app allocates its own.

use mlua::{Lua, Value};

use super::binding_abi::{self, flag};
use super::live::{ask, LiveQuery};
use super::{Model, ScriptValue};
use crate::script::binding_abi::number_arg;

/// `rollType` 0, the one vote the bind-on-pickup gate never intercepts.
const PASS: i32 = 0;

/// The quality `GetLootRollItemInfo` answers on an item-template miss, from the reference's miss
/// tail `nil, nil, 1.0, 1.0, nil` (`0x4c31a3`): 1, not `GetLootSlotInfo`'s -1. Stock
/// `GroupLootFrame_OnShow` indexes `ITEM_QUALITY_COLORS` with it unguarded
/// (`LootFrame.lua:275-276`).
const CACHE_MISS_QUALITY: i64 = 1;

/// One listed group loot roll as its getters read it.
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
    /// Milliseconds left: the expiry the start packet set, its arrival plus the countdown
    /// (`0x61b404`), less the clock at the call (`0x4c331c`), as a wrapping 32-bit difference.
    pub time_left_ms: u32,
    /// The item id, the key of the hover's item lookup.
    pub item_id: u32,
    /// The link `GetLootRollItemLink` answers for the icon's ctrl/shift clicks
    /// (`LootFrame.xml:353-361`); `None` until the item template lands, since it embeds the name.
    pub link: Option<String>,
    /// `SMSG_LOOT_START_ROLL`'s random suffix, 0 for none: the hover's only enchant source, as
    /// `SetLootRollItem` (`0x5364a0`) copies it to the tooltip's `+0x424` with no item object.
    pub random_property_id: u32,
    /// The node's closed byte `+0x3a`, which our vote sets (`0x61bf32`): the roll stays listed,
    /// and its getters answer, until the server's result frees it, but a vote on it does nothing
    /// (`0x61be3f`).
    pub closed: bool,
}

/// The question: roll `roll_id` as its getters read it at the call, `None` for no such roll.
pub struct LootRollQuery {
    pub roll_id: u32,
}

impl LiveQuery for LootRollQuery {
    type Answer = Option<LootRollEntry>;
}

/// Roll `roll_id` now: the host's answer, else the VM's copy.
fn entry(lua: &Lua, roll_id: u32) -> Option<LootRollEntry> {
    ask(lua, &LootRollQuery { roll_id }).unwrap_or_else(|| {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        find(&model, roll_id).cloned()
    })
}

/// The VM's own copy of the listed rolls, oldest first, which the getters read only with no host.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LootRollsState {
    pub rolls: Vec<LootRollEntry>,
}

impl super::UiScript {
    /// Set the VM's own copy of the listed rolls, which the getters read only with no host lent.
    pub fn set_loot_rolls(&mut self, state: LootRollsState) {
        self.model_mut().loot_rolls = state;
    }

    /// The `(roll_id, roll_type)` votes a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_loot_roll_votes(&mut self) -> Vec<(u32, u8)> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::LootRoll(LootRollCall::Vote { roll_id, roll_type }) => {
                Some((*roll_id, *roll_type))
            }
            _ => None,
        })
    }
}

/// The group loot roll's verbs the host applies inside their calls
/// ([`super::ScriptCall::LootRoll`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LootRollCall {
    /// The vote `0x61bdf0` past its bind-on-pickup gate: `CMSG_LOOT_ROLL` with the roll's
    /// `(lootedTarget, itemSlot)` and the type's low byte (`0x61bf10`). The app maps the
    /// client-internal `rollID` back to the wire's pair.
    Vote { roll_id: u32, roll_type: u8 },
    /// The vote's tail after its `CANCEL_LOOT_ROLL`: the closed byte (`0x61bf32`), the open
    /// count lowered (`0x61bf40`) and the oldest queued roll promoted (`0x61bf46` → `0x61b760`).
    Close { roll_id: u32 },
}

/// `0x61bdf0`, the vote both bindings share: an unknown or closed roll does nothing
/// (`0x61be3f`). The type is not range-tested: any nonzero one takes the bind-on-pickup gate, and
/// the wire carries its low byte (`0x418070`); the server ignores one of 3 or more
/// (`GroupHandler.cpp:367`). Need or Greed on a bind-on-pickup item, unconfirmed, signals
/// `CONFIRM_LOOT_ROLL` with `(rollID, rollType)` and sends nothing (`0x61be8b`). Otherwise the
/// host sends `CMSG_LOOT_ROLL` (`0x61bf10`), then `CANCEL_LOOT_ROLL` fires (`0x61bf2a`) before
/// the roll closes (`0x61bf32`), so its handler still reads the roll open; the closed roll stays
/// listed until the server's result frees it, which then signals nothing for it.
fn vote(lua: &Lua, roll_id: u32, roll_type: i32, confirmed: bool) -> mlua::Result<()> {
    let Some(entry) = entry(lua, roll_id).filter(|e| !e.closed) else {
        return Ok(());
    };
    if entry.bind_on_pickup && roll_type != PASS && !confirmed {
        super::tick::verb(lua, |model| {
            model.raise(
                "CONFIRM_LOOT_ROLL",
                vec![
                    ScriptValue::Int(i64::from(roll_id)),
                    ScriptValue::Int(i64::from(roll_type)),
                ],
            );
        });
        return Ok(());
    }
    super::calls::submit(
        lua,
        super::ScriptCall::LootRoll(LootRollCall::Vote {
            roll_id,
            roll_type: roll_type as u8,
        }),
    )?;
    super::tick::verb(lua, |model| {
        model.raise(
            "CANCEL_LOOT_ROLL",
            vec![ScriptValue::Int(i64::from(roll_id))],
        );
    });
    super::calls::submit(
        lua,
        super::ScriptCall::LootRoll(LootRollCall::Close { roll_id }),
    )?;
    super::tick::verb(lua, |model| {
        if let Some(r) = model
            .loot_rolls
            .rolls
            .iter_mut()
            .find(|r| r.roll_id == roll_id)
        {
            r.closed = true;
        }
    });
    Ok(())
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
            let Some(r) = entry(lua, roll_id) else {
                return lua.pack_multi((
                    mlua::Value::Nil,
                    mlua::Value::Nil,
                    1,
                    CACHE_MISS_QUALITY,
                    mlua::Value::Nil,
                ));
            };
            let vals = (
                r.texture,
                r.name,
                1,
                r.quality.map_or(CACHE_MISS_QUALITY, i64::from),
                flag(r.bind_on_pickup),
            );
            lua.pack_multi(vals)
        })?,
    )?;

    // No values at all for an unknown id or while the item template is in flight: both exits are
    // `xor eax,eax` (`0x4c3255`, `0x4c3277` → `0x4c322c`).
    g.set(
        "GetLootRollItemLink",
        lua.create_function(|lua, roll_id: Value| {
            // `is-number` else `Usage: GetLootRollItemLink(id)` (`0x848100`, `0x4c321e`).
            let roll_id = binding_abi::index_arg32(lua, roll_id, "Usage: GetLootRollItemLink(id)")?;
            let link = entry(lua, roll_id).and_then(|r| r.link);
            match link {
                Some(link) => Ok(mlua::MultiValue::from_vec(vec![mlua::Value::String(
                    lua.create_string(&link)?,
                )])),
                None => Ok(mlua::MultiValue::new()),
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
            Ok(entry(lua, roll_id).map_or(0, |r| r.time_left_ms))
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
            vote(lua, id, ty, false)
        })?,
    )?;

    // The gate's bypass: `0x4c33e0` re-enters `0x61bdf0` with a third argument of 1.
    g.set(
        "ConfirmLootRoll",
        lua.create_function(|lua, (roll_id, roll_type): (Value, Value)| {
            let usage = "Usage: ConfirmLootRoll(id, rollType)";
            let id = number_arg(lua, roll_id, usage)? as u32;
            let ty = number_arg(lua, roll_type, usage)?;
            vote(lua, id, ty, true)
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
                    closed: false,
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
                    closed: false,
                },
            ],
        }
    }

    #[test]
    fn roll_snapshot_reads() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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
        assert_eq!(
            s.arity("GetLootRollItemLink(8)").unwrap(),
            0,
            "an uncached item returns no values (`0x4c3277`)"
        );

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
        assert_eq!(
            s.arity("GetLootRollItemLink(99)").unwrap(),
            0,
            "an unknown roll returns no values (`0x4c3255`)"
        );
        assert_eq!(s.eval::<i64>("return GetLootRollTimeLeft(99)").unwrap(), 0);
    }

    /// A vote signals `CANCEL_LOOT_ROLL` inside the verb (`0x61bf2a`), its handler still reading
    /// the roll, which closes after it (`0x61bf32`): a second vote on it does nothing (`0x61be3f`),
    /// and the closed roll stays listed, its getters answering, until the server's result.
    #[test]
    fn a_vote_signals_its_cancel_in_place_and_closes_the_roll() {
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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
        assert_eq!(
            s.eval::<i64>("return GetLootRollTimeLeft(8)").unwrap(),
            60_000
        );
    }

    /// Need or Greed on a bind-on-pickup roll signals `CONFIRM_LOOT_ROLL` with
    /// `(rollID, rollType)` and sends nothing (`0x61be8b`); Pass and the confirm vote.
    #[test]
    fn need_or_greed_on_a_bop_roll_confirms_instead_of_voting() {
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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
        s.keep_calls();
        s.set_loot_rolls(rolls());
        s.run("RollOnLoot(7, 0)").unwrap();
        assert_eq!(s.take_loot_roll_votes(), vec![(7, 0)]);
    }

    #[test]
    fn confirm_still_validates() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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
        s.keep_calls();
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
        s.run("RollOnLoot(8, 1)").unwrap();
        assert!(s.take_loot_roll_votes().is_empty(), "the roll is closed");
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
        s.keep_calls();
        s.set_loot_rolls(rolls());
        s.set_loot_rolls(LootRollsState::default());
        assert!(s
            .eval::<bool>("return GetLootRollItemInfo(7) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetLootRollTimeLeft(7)").unwrap(), 0);
    }
}
