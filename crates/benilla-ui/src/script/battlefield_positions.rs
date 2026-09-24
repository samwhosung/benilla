//! The battleground position verbs `WorldMapFrame.lua` and `Blizzard_BattlefieldMinimap.lua` poll
//! to place teammates and the flag carrier (opcode `0x2E9`, handler `0x4aad40`). The reference
//! drops itself, its party and its raid, prefers a live object's position and projects onto the
//! active slot's map; the app does all that and pushes the finished list here.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;
use super::Model;

/// One teammate: map-normalized position, `(0, 0)` off the displayed map, and name if known.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BattlefieldPositionView {
    pub uv: (f32, f32),
    pub name: Option<String>,
}

/// The flag carrier: the position and the token the viewer's faction picks, `"HordeFlag"` for an
/// Alliance viewer and `"AllianceFlag"` for a Horde one.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BattlefieldFlagView {
    pub uv: (f32, f32),
    pub token: Option<String>,
}

impl super::UiScript {
    /// Push the positions, the carrier and the active map's icon scale.
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

    /// `RequestBattlefieldPositions()` calls since the last drain; the app sends at most every
    /// 5000 ms, and only with an active slot.
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

    g.set(
        "GetNumBattlefieldPositions",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.battlefield_positions.len() as i64)
        })?,
    )?;

    // Three values on every leg that does not raise: `(0, 0, nil)` off the list, 0 and below too.
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

    g.set(
        "GetNumBattlefieldFlagPositions",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.battlefield_flag.is_some()))
        })?,
    )?;

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

    // The active map's `MinimapIconScale`: map 0's with no slot, 1.0 for a missing row.
    g.set(
        "GetBattlefieldMapIconScale",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(f64::from(model.battlefield_icon_scale))
        })?,
    )?;

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
