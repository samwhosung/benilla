//! `FollowUnit` and `FollowByName`; each hands its [`FollowRequest`] to the host, which runs the
//! start gate and arms the follow inside the call ([`super::ScriptCall::Follow`]).
//! `FollowUnit("target")` is a bare `/follow` and the `FOLLOWTARGET` binding. `FollowByName`
//! (`0x489ec0`) calls the by-name resolver `0x493aa0` for players (`0x10`) who `CanAssist` and are
//! alive, with its second argument as the exact-only flag: `UnitPopup.lua:623` passes 1, so the
//! menu never prefix-matches someone else, while `/follow rag` may. benilla parses `/follow` in
//! Rust, so these globals serve the stock UI and addons.

use mlua::Lua;

/// A follow verb, which the host applies inside its call.
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
    /// The follow verbs a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_follow_requests(&mut self) -> Vec<FollowRequest> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Follow(request) => Some(request.clone()),
            _ => None,
        })
    }
}

/// Register the follow globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "FollowUnit",
        lua.create_function(|lua, unit: String| {
            super::calls::submit(lua, super::ScriptCall::Follow(FollowRequest::ByUnit(unit)))
        })?,
    )?;

    g.set(
        "FollowByName",
        lua.create_function(|lua, (name, exact): (String, Option<mlua::Value>)| {
            // `GetBoolOrDefault` (`0x6f1c10`, `0x489f14`) with default false.
            let exact = super::binding_abi::bool_or_default(exact.as_ref(), false);
            super::calls::submit(
                lua,
                super::ScriptCall::Follow(FollowRequest::ByName { name, exact }),
            )
        })?,
    )?;

    Ok(())
}
