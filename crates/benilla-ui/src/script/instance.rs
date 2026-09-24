//! The instance-lockout **Era API surface** — three globals, no snapshot struct.
//!
//! `IsInInstance`, `CanShowResetInstances` and `ResetInstances` are all engine bindings in 1.12
//! (`reference/1.12-globals.tsv` lists the three as `function`/`engine`), and between them they
//! are the whole Lua side of the lockout family: everything else it does arrives as a
//! `CHAT_MSG_SYSTEM` line the client composes itself, with no event and no getter.
//!
//! Both readers answer off state the app pushes, because both are `Map.dbc` questions and the DBC
//! is the app's to read — the same split [`super::party::SavedInstanceInfo`] already makes.
//!
//! **`IsInInstance` returns a pair**: `1`/`nil` for "is this an instance at all", and the type as a
//! string. The four strings are the reference's own table at `0x83de58`, indexed by
//! `Map.dbc`'s `InstanceType` behind a `< 4` guard: `none` (0), `party` (1), `raid` (2),
//! `pvp` (3) — `IsInInstance 0x48a750`.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// `Map.dbc`'s `InstanceType` → the word `IsInInstance()` returns for it — the reference's
/// `0x83de58` string table, with its own out-of-range fallback (`cmp esi,4; jae` → `"none"`).
pub fn instance_type_name(instance_type: u32) -> &'static str {
    match instance_type {
        1 => "party",
        2 => "raid",
        3 => "pvp",
        // 0 and anything the table cannot index.
        _ => "none",
    }
}

impl super::UiScript {
    /// Push the current map's `Map.dbc` `InstanceType` — `IsInInstance()`'s whole input.
    /// `None` = the map id has no DBC row, which the reference reaches by null-checking the record
    /// pointer; it then returns without pushing anything at all (a real 1.12 stack bug). We answer
    /// `nil, "none"` there, which is what every caller of it actually means to read.
    pub fn set_instance_type(&mut self, instance_type: Option<u32>) {
        let mut model = self.model_mut();
        if model.instance_type != instance_type {
            model.instance_type = instance_type;
        }
    }

    /// Push `CanShowResetInstances()`'s answer — the app owns every term of it (the ownership
    /// latch, the last dungeon, its age, and the map we are standing on).
    pub fn set_can_reset_instances(&mut self, can: bool) {
        let mut model = self.model_mut();
        if model.can_reset_instances != can {
            model.can_reset_instances = can;
        }
    }

    /// Drain the `ResetInstances()` calls queued since the last drain — each one is a
    /// `CMSG_RESET_INSTANCES`. A count, like [`super::UiScript::take_binder_confirms`]: the intent
    /// has no payload of its own.
    pub fn take_reset_instance_asks(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().reset_instance_asks)
    }
}

/// Register the three lockout globals ([`super::binder`]'s style).
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // IsInInstance() → inInstance, instanceType. The first return is the NUMBER 1 rather than a
    // boolean because the reference pushes 1.0 (`push 0x3ff00000; push 0` = the double 1.0), and
    // 1.12 UI code compares it numerically.
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

    // CanShowResetInstances() — UnitPopup's gate on the SELF menu's "Reset all instances" row.
    // 1.0/nil for the same reason as above.
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

    // ResetInstances() — the CONFIRM_RESET_INSTANCES dialog's Yes.
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
