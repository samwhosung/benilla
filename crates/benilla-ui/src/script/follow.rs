//! `FollowUnit` and `FollowByName`; each call queues a [`FollowRequest`] the app drains.
//! `FollowUnit("target")` is a bare `/follow` and the `FOLLOWTARGET` binding. `FollowByName`
//! (`0x489ec0`) calls the by-name resolver `0x493aa0` for players (`0x10`) who `CanAssist` and are
//! alive, with its second argument as the exact-only flag: `UnitPopup.lua:623` passes 1, so the
//! menu never prefix-matches someone else, while `/follow rag` may. benilla parses `/follow` in
//! Rust, so these globals serve the stock UI and addons.

use mlua::Lua;

use super::Model;

/// A follow intent queued by the globals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FollowRequest {
    /// `FollowUnit(unit)`: whoever the token names, of any type; the reference applies no
    /// typemask here.
    ByUnit(String),
    /// `FollowByName(name, exactMatch)`: with `exact`, the resolver skips its longest-prefix tier
    /// and takes only a whole-name, case-insensitive match.
    ByName { name: String, exact: bool },
}

impl super::UiScript {
    /// Drain the follow intents queued since the last call.
    pub fn take_follow_requests(&mut self) -> Vec<FollowRequest> {
        std::mem::take(&mut self.model_mut().follow_requests)
    }
}

/// Register the follow globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "FollowUnit",
        lua.create_function(|lua, unit: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.follow_requests.push(FollowRequest::ByUnit(unit));
            Ok(())
        })?,
    )?;

    g.set(
        "FollowByName",
        lua.create_function(|lua, (name, exact): (String, Option<mlua::Value>)| {
            // The reference reads this with `GetBoolOrDefault` (`0x6f1c10`), default false; here
            // nil, false and 0 are false and any other value is true.
            let exact = match exact {
                None | Some(mlua::Value::Nil) | Some(mlua::Value::Boolean(false)) => false,
                Some(mlua::Value::Integer(n)) => n != 0,
                Some(mlua::Value::Number(n)) => n != 0.0,
                Some(_) => true,
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model
                .follow_requests
                .push(FollowRequest::ByName { name, exact });
            Ok(())
        })?,
    )?;

    Ok(())
}
