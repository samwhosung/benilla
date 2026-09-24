//! `IsInInstance`, `CanShowResetInstances` and `ResetInstances`, the whole Lua side of instance
//! lockouts; the rest arrives as `CHAT_MSG_SYSTEM` lines. The readers answer off state the app
//! pushes from `Map.dbc`.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// `IsInInstance()`'s type word for a `Map.dbc` `InstanceType`: the reference's table at
/// `0x83de58`, and `"none"` from 4 up.
pub fn instance_type_name(instance_type: u32) -> &'static str {
    match instance_type {
        1 => "party",
        2 => "raid",
        3 => "pvp",
        _ => "none",
    }
}

impl super::UiScript {
    /// Push the current map's `Map.dbc` `InstanceType`, `None` for a map with no row. Deviation:
    /// `IsInInstance()` then answers `nil, "none"`, because the reference returns two results
    /// without pushing either (`0x48a7a9`), a stack bug.
    pub fn set_instance_type(&mut self, instance_type: Option<u32>) {
        let mut model = self.model_mut();
        if model.instance_type != instance_type {
            model.instance_type = instance_type;
        }
    }

    /// Push `CanShowResetInstances()`'s answer; the app owns its terms (the ownership latch, the
    /// last dungeon, its age and the current map).
    pub fn set_can_reset_instances(&mut self, can: bool) {
        let mut model = self.model_mut();
        if model.can_reset_instances != can {
            model.can_reset_instances = can;
        }
    }

    /// `ResetInstances()` calls since the last drain, each a `CMSG_RESET_INSTANCES`.
    pub fn take_reset_instance_asks(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().reset_instance_asks)
    }
}

/// Register the three lockout globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `IsInInstance()` (`0x48a750`): 1 if `InstanceType` is nonzero, else nil; then the type word.
    g.set(
        "IsInInstance",
        lua.create_function(|lua, ()| {
            let ty = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.instance_type
            };
            let name = instance_type_name(ty.unwrap_or(0));
            let inside = match ty {
                Some(t) if t != 0 => Value::Number(1.0),
                _ => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                inside,
                Value::String(lua.create_string(name)?),
            ]))
        })?,
    )?;

    // `CanShowResetInstances()`: 1 or nil, the gate on the SELF menu's reset row.
    g.set(
        "CanShowResetInstances",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if model.can_reset_instances {
                Value::Number(1.0)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // `ResetInstances()`: the `CONFIRM_RESET_INSTANCES` dialog's Yes.
    g.set(
        "ResetInstances",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.reset_instance_asks += 1;
            Ok(())
        })?,
    )?;

    Ok(())
}
