//! The battleground **position** family (decision 1980; fed by the wire handler `0x4aad40` for
//! opcode `0x2E9`): the six verbs the stock
//! `WorldMapFrame.lua` and `Blizzard_BattlefieldMinimap.lua` poll to place teammates and the
//! flag carrier on the map.
//!
//! The wire carries raw world floats for the teammates outside the requester's group; the
//! reference filters out itself, its four party slots and its raid roster, prefers the live
//! object's position over the packet's, and normalizes through the world-map projection under the
//! active queue slot's map — all app-side facts here, so the app pushes the finished list every
//! frame ([`BattlefieldPositionView`], already filtered and projected) and the VM owns the getters'
//! shapes: three values on every non-raising leg, `(0, 0, nil)` off the list.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;
use super::Model;

/// One teammate as the app resolved it: the map-normalized pair (`(0, 0)` = off the displayed
/// map) and the name, `None` while the name cache has not answered.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BattlefieldPositionView {
    pub uv: (f32, f32),
    pub name: Option<String>,
}

/// The flag carrier as the app resolved it: the pair, and the token the local player's faction
/// selects (`"HordeFlag"` for an Alliance viewer, `"AllianceFlag"` for a Horde one, `None` for
/// neither).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BattlefieldFlagView {
    pub uv: (f32, f32),
    pub token: Option<String>,
}

impl super::UiScript {
    /// Push the resolved positions, the carrier (or none) and the active map's icon scale.
    pub fn set_battlefield_positions(
        &mut self,
        players: Vec<BattlefieldPositionView>,
        flag: Option<BattlefieldFlagView>,
        icon_scale: f32,
    ) {
        let mut model = self.model_mut();
        model.battlefield_positions = players;
        model.battlefield_flag = flag;
        model.battlefield_icon_scale = icon_scale;
    }

    /// `RequestBattlefieldPositions()` calls since the last drain — the app throttles to 5000 ms
    /// and sends only with an active slot.
    pub fn take_battlefield_position_requests(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().battlefield_position_requests)
    }
}

fn three(lua: &Lua, uv: (f32, f32), third: Option<&str>) -> mlua::Result<MultiValue> {
    Ok(MultiValue::from_vec(vec![
        Value::Number(f64::from(uv.0)),
        Value::Number(f64::from(uv.1)),
        match third {
            Some(s) => Value::String(lua.create_string(s)?),
            None => Value::Nil,
        },
    ]))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `GetNumBattlefieldPositions()` — the survivors of the filter, one number.
    g.set(
        "GetNumBattlefieldPositions",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.battlefield_positions.len() as i64)
        })?,
    )?;

    // `GetBattlefieldPosition(index)` — the usage raise; 1-based over the survivors; three values
    // on every leg, `(0, 0, nil)` off the list and for 0 or a negative.
    g.set(
        "GetBattlefieldPosition",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlefieldPosition(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let row = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.battlefield_positions.get(i));
            match row {
                Some(r) => three(lua, r.uv, r.name.as_deref()),
                None => three(lua, (0.0, 0.0), None),
            }
        })?,
    )?;

    // `GetNumBattlefieldFlagPositions()` — one carrier slot: 1 or 0.
    g.set(
        "GetNumBattlefieldFlagPositions",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.battlefield_flag.is_some()))
        })?,
    )?;

    // `GetBattlefieldFlagPosition(index)` — the usage raise; selected only for index 1 with a
    // carrier; three values always.
    g.set(
        "GetBattlefieldFlagPosition",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlefieldFlagPosition(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            match model.battlefield_flag.as_ref().filter(|_| index == 1) {
                Some(f) => three(lua, f.uv, f.token.as_deref()),
                None => three(lua, (0.0, 0.0), None),
            }
        })?,
    )?;

    // `GetBattlefieldMapIconScale()` — the active map's `MinimapIconScale` (map 0's with no slot,
    // 1.0 on a missing row), pushed by the app.
    g.set(
        "GetBattlefieldMapIconScale",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(f64::from(model.battlefield_icon_scale))
        })?,
    )?;

    // `RequestBattlefieldPositions()` — counted; the app's 5000 ms throttle and slot gate decide.
    g.set(
        "RequestBattlefieldPositions",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.battlefield_position_requests += 1;
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    #[test]
    fn the_getters_answer_three_values_and_zero_off_the_list() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("return GetNumBattlefieldPositions()")
                .unwrap(),
            0
        );
        assert_eq!(s.arity("GetBattlefieldPosition(1)").unwrap(), 3);
        s.set_battlefield_positions(
            vec![
                BattlefieldPositionView {
                    uv: (0.25, 0.5),
                    name: Some("Probe-Realm".into()),
                },
                BattlefieldPositionView {
                    uv: (0.75, 0.125),
                    name: None,
                },
            ],
            Some(BattlefieldFlagView {
                uv: (0.375, 0.625),
                token: Some("HordeFlag".into()),
            }),
            1.25,
        );
        assert_eq!(
            s.eval::<i64>("return GetNumBattlefieldPositions()")
                .unwrap(),
            2
        );
        let got = s
            .eval::<String>(
                "local x, y, n = GetBattlefieldPosition(1) return x .. '|' .. y .. '|' .. n",
            )
            .unwrap();
        assert_eq!(got, "0.25|0.5|Probe-Realm");
        assert!(
            s.eval::<bool>(
                "local x, y, n = GetBattlefieldPosition(2) return x == 0.75 and n == nil"
            )
            .unwrap(),
            "a name the cache has not answered is nil"
        );
        for i in ["0", "3", "-1", "'2.9'"] {
            let got = s
                .eval::<String>(&format!(
                    "local x, y, n = GetBattlefieldPosition({i}) return x .. '|' .. y .. '|' .. tostring(n)"
                ))
                .unwrap();
            let want = if i == "'2.9'" {
                "0.75|0.125|nil"
            } else {
                "0|0|nil"
            };
            assert_eq!(got, want, "index {i}");
        }
        let err = s
            .run("GetBattlefieldPosition(nil)")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: GetBattlefieldPosition(index)"),
            "{err}"
        );

        assert_eq!(
            s.eval::<i64>("return GetNumBattlefieldFlagPositions()")
                .unwrap(),
            1
        );
        let got = s
            .eval::<String>(
                "local x, y, t = GetBattlefieldFlagPosition(1) return x .. '|' .. y .. '|' .. t",
            )
            .unwrap();
        assert_eq!(got, "0.375|0.625|HordeFlag");
        assert!(s
            .eval::<bool>(
                "local x, y, t = GetBattlefieldFlagPosition(2) return x == 0 and t == nil"
            )
            .unwrap());
        let err = s
            .run("GetBattlefieldFlagPosition('q')")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: GetBattlefieldFlagPosition(index)"),
            "{err}"
        );
        assert!(
            (s.eval::<f64>("return GetBattlefieldMapIconScale()")
                .unwrap()
                - 1.25)
                .abs()
                < 1e-6
        );
        s.run("RequestBattlefieldPositions() RequestBattlefieldPositions()")
            .unwrap();
        assert_eq!(s.take_battlefield_position_requests(), 2);
        assert_eq!(s.take_battlefield_position_requests(), 0);
    }
}
