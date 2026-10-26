//! `IsInInstance`, `CanShowResetInstances` and `ResetInstances`, the whole Lua side of instance
//! lockouts; the rest arrives as `CHAT_MSG_SYSTEM` lines. The readers ask the host at the call
//! ([`super::InstanceTypeQuery`], [`super::CanResetInstancesQuery`]), the VM's copy answering
//! with none.

use mlua::{Lua, MultiValue, Value};

use super::live::ask;
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
    /// Set the current map's `Map.dbc` `InstanceType` read with no host lent, `None` for a map with
    /// no row. Deviation: `IsInInstance()` then answers `nil, "none"`, because the reference
    /// returns two results without pushing either (`0x48a7a9`), a stack bug.
    pub fn set_instance_type(&mut self, instance_type: Option<u32>) {
        let mut model = self.model_mut();
        if model.instance_type != instance_type {
            model.instance_type = instance_type;
        }
    }

    /// Set `CanShowResetInstances()`'s answer with no host lent; the app owns its terms (the
    /// ownership latch, the last dungeon, its age and the current map).
    pub fn set_can_reset_instances(&mut self, can: bool) {
        let mut model = self.model_mut();
        if model.can_reset_instances != can {
            model.can_reset_instances = can;
        }
    }

    /// The `ResetInstances()` calls a harness's VM kept ([`Self::keep_calls`]).
    pub fn take_reset_instance_asks(&mut self) -> u32 {
        self.take_calls_like(&super::ScriptCall::Instance(InstanceCall::Reset)) as u32
    }
}

/// The lockouts' verb the host applies inside its call ([`super::ScriptCall::Instance`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceCall {
    /// `ResetInstances()` (`0x48a6b0`): `CMSG_RESET_INSTANCES` (`0x48a6e6`).
    Reset,
}

/// Register the three lockout globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `IsInInstance()` (`0x48a750`): 1 if `InstanceType` is nonzero, else nil; then the type word.
    g.set(
        "IsInInstance",
        lua.create_function(|lua, ()| {
            let ty = ask(lua, &super::InstanceTypeQuery).unwrap_or_else(|| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.instance_type
            });
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
            let can = ask(lua, &super::CanResetInstancesQuery).unwrap_or_else(|| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.can_reset_instances
            });
            Ok(if can { Value::Number(1.0) } else { Value::Nil })
        })?,
    )?;

    // `ResetInstances()`: the `CONFIRM_RESET_INSTANCES` dialog's Yes.
    g.set(
        "ResetInstances",
        lua.create_function(|lua, ()| {
            super::calls::submit(lua, super::ScriptCall::Instance(InstanceCall::Reset))
        })?,
    )?;

    Ok(())
}
