//! The battleground list and queue verbs of `BattlefieldFrame.lua` and `Minimap.xml`. They read
//! the reference's instance list (`0xb6e860`, filled by `0x4aa6c0`), its three queue slots
//! (`0xb6e9d0`, written by `0x4aa850`) and the selected instance id (`[0xb6eba0]`); the app pushes
//! the list and the slots, clock values in ms, and the VM owns the selection.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, flag, number_arg};
use super::Model;

/// One queue slot: the reference's `0x20`-byte slot (`0xb6e9d0`) plus its Map.dbc name.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldQueueSlot {
    /// `+0x00`, the Map.dbc row id; 0 is a cleared slot, its name looked up like any other.
    pub map_id: u32,
    /// The localized map name, or `None` when the id has no row (a real `nil`).
    pub map_name: Option<String>,
    /// `+0x04`: `0` none, `1` queued, `2` confirm, `3` active; anything else answers `"error"`.
    pub status: u32,
    /// `+0x10`.
    pub instance_id: u32,
    /// `+0x08`/`+0x0c`: the bracket-adjusted level pair.
    pub min_level: u32,
    pub max_level: u32,
    /// `GetBattlefieldPortExpiration`: `deadline − now` in ms, 0 when unset or past.
    pub port_expiration_ms: u32,
    /// `GetBattlefieldEstimatedWaitTime`: the stored value, raw (no clock).
    pub estimated_wait_ms: u32,
    /// `GetBattlefieldTimeWaited`: `now − stamp` in ms, 0 when unset.
    pub time_waited_ms: u32,
}

/// The Map.dbc half of `GetBattlefieldInfo` (`0x4ab0b0`), resolved by the app for the listed map.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BattlefieldMapInfo {
    pub name: String,
    /// The description for the player's faction. Deviation: `None` answers `nil` where the
    /// reference's `-1` faction leg pushes nothing (Lua reads below the tuple), because no
    /// playable race reaches that leg.
    pub description: Option<String>,
    /// The row's raw `MinLevel`/`MaxLevel` (values 3 and 4), not the bracket-adjusted pair.
    pub min_level: u32,
    pub max_level: u32,
    /// Values 5–7: `[row+0x40]` signed, `[row+0x44]` and `[row+0x48]` f32.
    pub field_16: i32,
    pub field_17: f32,
    pub field_18: f32,
}

/// The instance list and the scalars beside it, as the app pushes them.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BattlefieldListView {
    /// The instance ids in wire order (`[0xb6e868]`).
    pub instances: Vec<u32>,
    /// The bracket-adjusted level pair the list handler derived (`[0xb6eba8]`/`[0xb6ebac]`).
    pub bracket_min: u32,
    pub bracket_max: u32,
    /// The listed map's row; `None` when the id has no row, which answers no values.
    pub info: Option<BattlefieldMapInfo>,
    /// The map row's group-queue flag, `[row+0xa0]`.
    pub group_queue: bool,
}

/// The reference's 1-based gate (`dec; cmp; jae`), unsigned: 0 and negatives fail as too large.
fn slot_index(index: i32, n: usize) -> Option<usize> {
    usize::try_from(index)
        .ok()?
        .checked_sub(1)
        .filter(|&i| i < n)
}

/// The local-player gate of `GetBattlefieldInfo` and `GetBattlefieldInstanceInfo`, read at call
/// time (`0x468550`, `0x468460` with `ecx = 0x10`): the answer `UnitExists("player")` gives.
fn player_exists(model: &Model) -> bool {
    model.unit("player").is_some_and(|u| u.exists)
}

/// The reference's four-entry status jump table with its `"error"` default (`0x4ab604`).
fn status_text(status: u32) -> &'static str {
    match status {
        0 => "none",
        1 => "queued",
        2 => "confirm",
        3 => "active",
        _ => "error",
    }
}

impl super::UiScript {
    /// Push the instance list. A new list leaves the selection (`[0xb6eba0]`) alone; it only
    /// decides whether `GetSelectedBattlefield` still finds it (`0x4ab360`).
    pub fn set_battlefield_list(&mut self, list: BattlefieldListView) {
        self.model_mut().battlefield_list = list;
    }

    /// The world-enter reset (`0x4a9db0`) clears the selection with the list.
    pub fn reset_battlefield_selection(&mut self) {
        self.model_mut().battlefield_selected = 0;
    }

    /// Push the queue slots and the instance expiration (`[0xb6ebb8]`) every frame, since the
    /// clock getters move.
    pub fn set_battlefield_queue(
        &mut self,
        slots: Vec<BattlefieldQueueSlot>,
        instance_expiration_ms: u32,
    ) {
        let mut model = self.model_mut();
        model.battlefield_slots = slots;
        model.battlefield_instance_expiration_ms = instance_expiration_ms;
    }

    /// `JoinBattlefield` calls since the last drain, `(instance, as group)`; instance 0 is first
    /// available. The app adds the map, the opcode and the group-size refusal (`0x4a9f60`).
    pub fn take_battlefield_join_requests(&mut self) -> Vec<(u32, bool)> {
        std::mem::take(&mut self.model_mut().battlefield_join_requests)
    }

    /// `ShowBattlefieldList` calls since the last drain: the map ids for `CMSG_BATTLEFIELD_LIST`.
    pub fn take_battlefield_list_requests(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().battlefield_list_requests)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumBattlefields",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.battlefield_list.instances.len() as i64)
        })?,
    )?;

    // No values without a player or a map row, else nine (`0x4ab0b0`).
    g.set(
        "GetBattlefieldInfo",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let list = &model.battlefield_list;
            let (Some(info), true) = (list.info.as_ref(), player_exists(&model)) else {
                return Ok(MultiValue::new());
            };
            let description = match &info.description {
                Some(d) => Value::String(lua.create_string(d)?),
                None => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&info.name)?),
                description,
                Value::Number(f64::from(info.min_level)),
                Value::Number(f64::from(info.max_level)),
                Value::Number(f64::from(info.field_16)),
                Value::Number(f64::from(info.field_17)),
                Value::Number(f64::from(info.field_18)),
                Value::Number(f64::from(list.bracket_min)),
                Value::Number(f64::from(list.bracket_max)),
            ]))
        })?,
    )?;

    // Raises with `GetBattlefieldInfo`'s usage string, the reference's own mislabel (`0x845c48`).
    // No values without a player or out of range, else the instance id pushed signed (`fild`).
    g.set(
        "GetBattlefieldInstanceInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlefieldInfo(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let list = &model.battlefield_list;
            if !player_exists(&model) {
                return Ok(MultiValue::new());
            }
            Ok(match slot_index(index, list.instances.len()) {
                Some(i) => {
                    MultiValue::from_vec(vec![Value::Number(f64::from(list.instances[i] as i32))])
                }
                None => MultiValue::new(),
            })
        })?,
    )?;

    // An out-of-range index, 0 included, joins instance 0, the first available (`0x4a9f60`).
    g.set(
        "JoinBattlefield",
        lua.create_function(|lua, (index, as_group): (Value, Value)| {
            let index = number_arg(lua, index, "Usage: JoinBattlefield(index)")?;
            let as_group = bool_or_default(Some(&as_group), false);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let instance = slot_index(index, model.battlefield_list.instances.len())
                .map_or(0, |i| model.battlefield_list.instances[i]);
            model.battlefield_join_requests.push((instance, as_group));
            Ok(())
        })?,
    )?;

    // A no-op in the reference (`xor eax,eax; ret`); the stock window calls it on hide.
    g.set("CloseBattlefield", lua.create_function(|_, ()| Ok(()))?)?;

    // Stores the instance id at that position, 0 when out of range (`0x4ab300`).
    g.set(
        "SetSelectedBattlefield",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: SetSelectedBattlefield(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.battlefield_selected = slot_index(index, model.battlefield_list.instances.len())
                .map_or(0, |i| model.battlefield_list.instances[i]);
            Ok(())
        })?,
    )?;

    // The stored id's 1-based position in the current list; 0 on a miss, as for a stored 0.
    g.set(
        "GetSelectedBattlefield",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let selected = model.battlefield_selected;
            Ok(model
                .battlefield_list
                .instances
                .iter()
                .position(|&id| id == selected)
                .map_or(0, |i| i as i64 + 1))
        })?,
    )?;

    // Five values on every leg (`0x4ab5c7`): `(nil, nil, 0, 0, 0)` off slots 1..3.
    g.set(
        "GetBattlefieldStatus",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlefieldStatus(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(slot) = slot_index(index, 3).and_then(|i| model.battlefield_slots.get(i))
            else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Number(0.0),
                    Value::Number(0.0),
                    Value::Number(0.0),
                ]));
            };
            let name = match &slot.map_name {
                Some(n) => Value::String(lua.create_string(n)?),
                None => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(status_text(slot.status))?),
                name,
                Value::Number(f64::from(slot.instance_id)),
                Value::Number(f64::from(slot.min_level)),
                Value::Number(f64::from(slot.max_level)),
            ]))
        })?,
    )?;

    // The per-slot clock getters (`0x4ab620`, `0x4ab790`, `0x4ab820`), in ms; 0 off slots 1..3.
    for (name, usage, read) in [
        (
            "GetBattlefieldPortExpiration",
            "Usage: GetBattlefieldPortExpiration(index)",
            (|s: &BattlefieldQueueSlot| s.port_expiration_ms) as fn(&BattlefieldQueueSlot) -> u32,
        ),
        (
            "GetBattlefieldEstimatedWaitTime",
            "Usage: GetBattlefieldEstimatedWaitTime(index)",
            |s: &BattlefieldQueueSlot| s.estimated_wait_ms,
        ),
        (
            "GetBattlefieldTimeWaited",
            "Usage: GetBattlefieldTimeWaited(index)",
            |s: &BattlefieldQueueSlot| s.time_waited_ms,
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, index: Value| {
                let index = number_arg(lua, index, usage)?;
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                Ok(i64::from(
                    slot_index(index, 3)
                        .and_then(|i| model.battlefield_slots.get(i))
                        .map_or(0, read),
                ))
            })?,
        )?;
    }

    // `[0xb6ebb8] − now`, 0 when unset or past.
    g.set(
        "GetBattlefieldInstanceExpiration",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.battlefield_instance_expiration_ms))
        })?,
    )?;

    // Silent unless the slot is queued; fires no event.
    g.set(
        "ShowBattlefieldList",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: ShowBattlefieldList(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let map_id = slot_index(index, 3)
                .and_then(|i| model.battlefield_slots.get(i))
                .filter(|s| s.map_id != 0 && s.status == 1)
                .map(|s| s.map_id);
            if let Some(map_id) = map_id {
                model.battlefield_list_requests.push(map_id);
            }
            Ok(())
        })?,
    )?;

    // `1` or nil (`0x4ac380`); the join-time group-size check reads another column, in the app.
    g.set(
        "CanJoinBattlefieldAsGroup",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.battlefield_list.group_queue))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    /// A VM with a local player, as every in-world call has.
    fn vm() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit(
            "player",
            Some(crate::script::UnitState {
                exists: true,
                name: Some("Probe".into()),
                ..Default::default()
            }),
        );
        s
    }

    fn list(ids: &[u32]) -> BattlefieldListView {
        BattlefieldListView {
            instances: ids.to_vec(),
            bracket_min: 20,
            bracket_max: 29,
            info: Some(BattlefieldMapInfo {
                name: "Arathi Basin".into(),
                description: Some("The Arathi Basin is …".into()),
                min_level: 20,
                max_level: 60,
                field_16: -1,
                field_17: 0.0,
                field_18: 0.0,
            }),
            group_queue: true,
        }
    }

    fn slot(map_id: u32, status: u32, instance: u32) -> BattlefieldQueueSlot {
        BattlefieldQueueSlot {
            map_id,
            map_name: Some(format!("Map {map_id}")),
            status,
            instance_id: instance,
            min_level: 20,
            max_level: 29,
            port_expiration_ms: 60_000,
            estimated_wait_ms: 30_000,
            time_waited_ms: 5_000,
        }
    }

    #[test]
    fn the_index_gate_is_unsigned_and_one_based() {
        assert_eq!(slot_index(1, 3), Some(0));
        assert_eq!(slot_index(3, 3), Some(2));
        assert_eq!(slot_index(4, 3), None);
        assert_eq!(slot_index(0, 3), None);
        assert_eq!(slot_index(-1, 3), None);
        assert_eq!(slot_index(1, 0), None);
    }

    #[test]
    fn get_battlefield_info_answers_nine_or_none() {
        let mut s = vm();
        assert_eq!(
            s.arity("GetBattlefieldInfo()").unwrap(),
            0,
            "no list at all: the map gate"
        );
        s.set_battlefield_list(list(&[7, 3]));
        s.set_unit("player", None);
        assert_eq!(
            s.arity("GetBattlefieldInfo()").unwrap(),
            0,
            "no player: the other gate, read at call time"
        );
        assert_eq!(
            s.arity("GetBattlefieldInstanceInfo(1)").unwrap(),
            0,
            "…and the instance verb's"
        );
        s = vm();
        s.set_battlefield_list(list(&[7, 3]));
        assert_eq!(s.arity("GetBattlefieldInfo()").unwrap(), 9);
        let got = s
            .eval::<String>(
                "local n, d, a, b, c, x, y, lo, hi = GetBattlefieldInfo() \
                 return n .. '|' .. a .. '|' .. b .. '|' .. c .. '|' .. lo .. '|' .. hi",
            )
            .unwrap();
        assert_eq!(got, "Arathi Basin|20|60|-1|20|29");
    }

    #[test]
    fn get_battlefield_instance_info_raises_with_the_mislabel_and_bails_silently() {
        let mut s = vm();
        s.set_battlefield_list(list(&[7, 0xFFFF_FFFF]));
        assert_eq!(
            s.eval::<i64>("return GetBattlefieldInstanceInfo(1)")
                .unwrap(),
            7
        );
        assert_eq!(
            s.eval::<i64>("return GetBattlefieldInstanceInfo('2')")
                .unwrap(),
            -1,
            "a numeric string passes; the id is pushed signed"
        );
        assert_eq!(s.arity("GetBattlefieldInstanceInfo(3)").unwrap(), 0);
        assert_eq!(s.arity("GetBattlefieldInstanceInfo(0)").unwrap(), 0);
        let err = s
            .run("GetBattlefieldInstanceInfo(nil)")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: GetBattlefieldInfo(index)"),
            "the shipped client's own mislabel: {err}"
        );
        assert_eq!(s.eval::<i64>("return GetNumBattlefields()").unwrap(), 2);
    }

    #[test]
    fn the_selection_is_an_instance_id_not_a_position() {
        let mut s = vm();
        s.set_battlefield_list(list(&[7, 3, 11]));
        assert_eq!(s.eval::<i64>("return GetSelectedBattlefield()").unwrap(), 0);
        s.run("SetSelectedBattlefield(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedBattlefield()").unwrap(), 2);
        s.set_battlefield_list(list(&[3, 7, 11]));
        assert_eq!(
            s.eval::<i64>("return GetSelectedBattlefield()").unwrap(),
            1,
            "instance 3 moved to the front"
        );
        s.set_battlefield_list(list(&[7, 11]));
        assert_eq!(
            s.eval::<i64>("return GetSelectedBattlefield()").unwrap(),
            0,
            "instance 3 left the list"
        );
        s.run("SetSelectedBattlefield(0)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedBattlefield()").unwrap(), 0);
        s.run("SetSelectedBattlefield(9)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetSelectedBattlefield()").unwrap(),
            0,
            "out of range stores 0"
        );
        let err = s.run("SetSelectedBattlefield({})").unwrap_err().to_string();
        assert!(
            err.contains("Usage: SetSelectedBattlefield(index)"),
            "{err}"
        );
    }

    #[test]
    fn join_battlefield_resolves_the_instance_and_the_group_flag() {
        let mut s = vm();
        s.set_battlefield_list(list(&[7, 3]));
        s.run("JoinBattlefield(0) JoinBattlefield(2, 1) JoinBattlefield(5, true) JoinBattlefield(1, nil)")
            .unwrap();
        assert_eq!(
            s.take_battlefield_join_requests(),
            vec![(0, false), (3, true), (0, true), (7, false)]
        );
        assert!(s.take_battlefield_join_requests().is_empty(), "drained");
        let err = s.run("JoinBattlefield()").unwrap_err().to_string();
        assert!(err.contains("Usage: JoinBattlefield(index)"), "{err}");
        s.run("CloseBattlefield()").unwrap();
    }

    #[test]
    fn get_battlefield_status_answers_five_values_on_every_leg() {
        let mut s = UiScript::new().unwrap();
        s.set_battlefield_queue(vec![slot(489, 1, 5), slot(0, 0, 0), slot(529, 9, 2)], 0);
        let n = |s: &mut UiScript, i: &str| s.arity(&format!("GetBattlefieldStatus({i})")).unwrap();
        assert_eq!(n(&mut s, "1"), 5);
        assert_eq!(n(&mut s, "0"), 5);
        assert_eq!(n(&mut s, "4"), 5);
        let got = s
            .eval::<String>(
                "local st, name, id, lo, hi = GetBattlefieldStatus(1) \
                 return st .. '|' .. name .. '|' .. id .. '|' .. lo .. '|' .. hi",
            )
            .unwrap();
        assert_eq!(got, "queued|Map 489|5|20|29");
        assert_eq!(
            s.eval::<String>("return (GetBattlefieldStatus(2))")
                .unwrap(),
            "none"
        );
        assert_eq!(
            s.eval::<String>("return (GetBattlefieldStatus(3))")
                .unwrap(),
            "error",
            "a status past 3 takes the jump table's default"
        );
        let off = s
            .eval::<String>(
                "local st, name, id, lo, hi = GetBattlefieldStatus(4) \
                 return tostring(st) .. '|' .. tostring(name) .. '|' .. id .. lo .. hi",
            )
            .unwrap();
        assert_eq!(off, "nil|nil|000");
        let err = s.run("GetBattlefieldStatus('x')").unwrap_err().to_string();
        assert!(err.contains("Usage: GetBattlefieldStatus(index)"), "{err}");
        let mut empty = slot(0, 0, 0);
        empty.map_name = None;
        s.set_battlefield_queue(vec![empty], 0);
        assert!(
            s.eval::<bool>("local _, name = GetBattlefieldStatus(1) return name == nil")
                .unwrap(),
            "no row: a real nil"
        );
    }

    #[test]
    fn the_time_getters_read_the_pushed_milliseconds() {
        let mut s = UiScript::new().unwrap();
        s.set_battlefield_queue(vec![slot(489, 1, 5)], 120_000);
        assert_eq!(
            s.eval::<i64>("return GetBattlefieldPortExpiration(1)")
                .unwrap(),
            60_000
        );
        assert_eq!(
            s.eval::<i64>("return GetBattlefieldEstimatedWaitTime(1)")
                .unwrap(),
            30_000
        );
        assert_eq!(
            s.eval::<i64>("return GetBattlefieldTimeWaited(1)").unwrap(),
            5_000
        );
        assert_eq!(
            s.eval::<i64>("return GetBattlefieldTimeWaited(2)").unwrap(),
            0
        );
        assert_eq!(
            s.eval::<i64>("return GetBattlefieldPortExpiration(0)")
                .unwrap(),
            0
        );
        assert_eq!(
            s.eval::<i64>("return GetBattlefieldInstanceExpiration()")
                .unwrap(),
            120_000
        );
        for (call, usage) in [
            (
                "GetBattlefieldPortExpiration()",
                "Usage: GetBattlefieldPortExpiration(index)",
            ),
            (
                "GetBattlefieldEstimatedWaitTime(nil)",
                "Usage: GetBattlefieldEstimatedWaitTime(index)",
            ),
            (
                "GetBattlefieldTimeWaited({})",
                "Usage: GetBattlefieldTimeWaited(index)",
            ),
        ] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains(usage), "{call}: {err}");
        }
    }

    #[test]
    fn show_battlefield_list_gates_on_a_queued_slot() {
        let mut s = UiScript::new().unwrap();
        s.set_battlefield_queue(vec![slot(489, 1, 5), slot(529, 2, 1), slot(0, 1, 0)], 0);
        s.run("ShowBattlefieldList(1) ShowBattlefieldList(2) ShowBattlefieldList(3) ShowBattlefieldList(4)")
            .unwrap();
        assert_eq!(s.take_battlefield_list_requests(), vec![489]);
        let err = s.run("ShowBattlefieldList('q')").unwrap_err().to_string();
        assert!(err.contains("Usage: ShowBattlefieldList(index)"), "{err}");

        assert!(s
            .eval::<bool>("return CanJoinBattlefieldAsGroup() == nil")
            .unwrap());
        s.set_battlefield_list(list(&[]));
        assert_eq!(
            s.eval::<i64>("return CanJoinBattlefieldAsGroup()").unwrap(),
            1
        );
    }
}
