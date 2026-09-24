//! The auto-follow **Era API surface** — two globals, no state.
//!
//! [`super::duel`]'s shape exactly: everything the UI *reads* about a follow arrives as an event
//! argument (`AUTOFOLLOW_BEGIN`'s name), so there is no snapshot to push — only the outbound half.
//! Each call queues a [`FollowRequest`] the app drains ([`super::UiScript::take_follow_requests`])
//! and turns into its own follow start, keeping the engine free of ECS reach.
//!
//! The two are the reference's own follow bindings, and between them they cover every shipped call
//! site in the 1.12 FrameXML:
//!
//! | binding | called by | subject |
//! |---|---|---|
//! | `FollowUnit(unit)` | `SlashCmdList["FOLLOW"]` with no argument | a unit token |
//! | `FollowByName(name, exactMatch)` | `SlashCmdList["FOLLOW"]` with one; `UnitPopup_OnClick`'s FOLLOW row | a name |
//!
//! `FollowByName` is `0x489ec0`, one more caller of the shared by-name resolver `0x493aa0` —
//! typemask `0x10` (players only) and filter mode 2 (`CanAssist` + alive), with **the second Lua
//! argument steering exact-only matching** (decision 0886's table). That argument is what makes the
//! popup row different from the slash command: `UnitPopup.lua` passes `FollowByName(name, 1)`, so
//! clicking *Follow* on a menu that already names the unit exactly must not prefix-match its way
//! onto somebody else standing nearby, while `/follow rag` still may.
//!
//! benilla parses slash lines in Rust rather than in Lua, so the chat half of that
//! table enters the app's follow funnel directly; these globals are the *other* half — the shipped
//! UI's, and any addon's.

use mlua::Lua;

use super::Model;

/// Outbound follow intents queued by the Era API calls, drained by the app
/// ([`super::UiScript::take_follow_requests`]). Plain data — [`super::duel::DuelRequest`]'s twin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FollowRequest {
    /// `FollowUnit(unit)` — follow whoever a unit token points at. The app resolves the token;
    /// `"target"` is the bare `/follow`, and takes the selection **whatever it is** (the reference
    /// applies no typemask on this path).
    ByUnit(String),
    /// `FollowByName(name, exactMatch)` — follow a player found by name. `exact` is the second Lua
    /// argument: set, it skips the resolver's longest-common-prefix tier and admits only a
    /// whole-string (case-insensitive) hit.
    ByName { name: String, exact: bool },
}

impl super::UiScript {
    /// Drain the follow intents queued since the last call.
    pub fn take_follow_requests(&mut self) -> Vec<FollowRequest> {
        std::mem::take(&mut self.model_mut().follow_requests)
    }
}

/// Register the follow globals (the same style/place [`super::duel`] registers its own).
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // FollowUnit(unit) — `SlashCmdList["FOLLOW"]`'s no-argument arm, `FollowUnit("target")`.
    g.set(
        "FollowUnit",
        lua.create_function(|lua, unit: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.follow_requests.push(FollowRequest::ByUnit(unit));
            Ok(())
        })?,
    )?;

    // FollowByName(name, exactMatch) — the unit popup's Follow row passes `1`; the slash command
    // passes nothing, which is the prefix-matching case (`nil` is false here, as in the ref, where
    // the argument reaches `0x493aa0` as a plain zero).
    g.set(
        "FollowByName",
        lua.create_function(|lua, (name, exact): (String, Option<mlua::Value>)| {
            // Absent is the slash command's case; the popup row passes `1`. A numeric zero counts
            // as false as well — no shipped caller passes one, and treating it that way is the
            // reading both plausible arg-2 fetches on the C side (`lua_toboolean` vs a number
            // compare) agree on for every value that can actually arrive here.
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
