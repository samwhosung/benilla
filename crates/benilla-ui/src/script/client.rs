//! The client-identity and script verbs: build, locale, realm, bind location and framerate, with
//! `RunScript` and `FrameXML_Debug`. The realm, bind location and framerate read slots the app
//! pushes.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// The 1.12.1 client's build identity, as the strings in its binary spell it.
const VERSION: &str = "1.12.1";
const BUILD: &str = "5875";
const BUILD_DATE: &str = "Sep 19 2006";

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `GetLocale()` (glue `0x46ce40`, in-game `0x48d8b0`) returns one string, `enUS`, as benilla
    // is enUS-only. `IsMacClient()` (`0x48c980`) returns one nil, the PC build's answer, which
    // takes FrameXML's PC key-label arm.
    g.set(
        "GetLocale",
        lua.create_function(|lua, ()| lua.create_string("enUS"))?,
    )?;
    g.set("IsMacClient", lua.create_function(|_, ()| Ok(Value::Nil))?)?;

    // `FrameXML_Debug([v])` (`0x488440`): get-or-set of the XML loader's trace flag `[0xceea30]`,
    // which gates `crate::loader::LoadReport::traces`. A Lua-truthy argument sets it, truncated
    // toward zero (`0x40a2b0`), so `FrameXML_Debug(0)` clears it; nil or false only reads. It
    // returns the flag after the call.
    g.set(
        "FrameXML_Debug",
        lua.create_function(|lua, v: Value| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let truthy = !matches!(v, Value::Nil | Value::Boolean(false));
            if truthy {
                // A value that will not coerce is 0, as `lua_tonumber` (`0x6f3620`) answers.
                let n = lua.coerce_number(v)?.unwrap_or(0.0);
                model.framexml_debug.set(n.trunc() as i32);
            }
            Ok(model.framexml_debug.get())
        })?,
    )?;

    // Version, build and date, and no fourth value (the TOC number is a later expansion's): the
    // in-game `GetBuildInfo` (`0x4884a0`) pushes three strings, the glue one (`0x46cd70`) five.
    g.set(
        "GetBuildInfo",
        lua.create_function(|lua, ()| {
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(VERSION)?),
                Value::String(lua.create_string(BUILD)?),
                Value::String(lua.create_string(BUILD_DATE)?),
            ]))
        })?,
    )?;

    // `RunScript(text)` (`0x48b980`) runs a chunk in the shared global state with full API
    // access, no `setfenv`. Its contract:
    // 1. The chunk name is the source itself: the one `lua_tostring` result is passed as both
    //    code and name to `FrameScript_Execute` (`0x704cd0`), so an error renders
    //    `[string "…"]`, as `loadstring`'s default does.
    // 2. An argument that is neither a string nor a number, or is empty, is a silent no-op, never
    //    a raise (`0x48b98f`, `0x48b99f`, `0x48b9a1`); `lua_isstring` (`0x6f3510`) passes a number.
    // 3. An error never reaches the caller: `0x704ae0` runs the chunk under `lua_pcall` with the
    //    registered error handler (`0x704b68`), and a compile failure goes to the same handler
    //    (`0x704b42`).
    // 4. It returns zero values on every path.
    g.set(
        "RunScript",
        lua.create_function(|lua, text: Value| {
            // `lua_isstring` and `lua_tostring` in one: strings and numbers coerce, anything else
            // is the silent return (2).
            let Some(s) = lua.coerce_string(text)? else {
                return Ok(());
            };
            let raw = s.as_bytes();
            if raw.is_empty() {
                return Ok(());
            }
            // The chunk name is the source bytes as given (1).
            let name = String::from_utf8_lossy(&raw).into_owned();
            if let Err(e) = lua
                .load(&*raw)
                .set_name(name)
                .set_mode(mlua::ChunkMode::Text)
                .exec()
            {
                lua.app_data_mut::<crate::script::Model>()
                    .expect("model")
                    .record_script_error(super::stdlib::lua_message(&e));
            }
            Ok(())
        })?,
    )?;

    // `GetRealmName()`: the realm as the realm list spells it; `""` until the app pushes one, the
    // glue screen's answer, never nil, as addons index saved variables with it at file scope.
    g.set(
        "GetRealmName",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            lua.create_string(&model.realm_name)
        })?,
    )?;

    // `RequestTimePlayed()` queues a `CMSG_PLAYED_TIME` and returns nothing; the answer is the
    // `TIME_PLAYED_MSG(total, level)` the app fires when `SMSG_PLAYED_TIME` lands.
    g.set(
        "RequestTimePlayed",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .played_time_asks += 1;
            Ok(())
        })?,
    )?;

    // `GetBindLocation()`: the hearthstone's bind location name, which the instance-boot dialog
    // formats into its text (`StaticPopup.lua:1742`). `""` until `SMSG_BINDPOINTUPDATE` lands, as
    // the reference's empty buffer, never nil, since addons concatenate it.
    g.set(
        "GetBindLocation",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            lua.create_string(&model.bind_location)
        })?,
    )?;

    // `GetFramerate()`: frames per second, a smoothed value the app pushes each tick; 0, a number,
    // before the first push.
    g.set(
        "GetFramerate",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.framerate)
        })?,
    )?;

    Ok(())
}

impl super::UiScript {
    /// Drain the `RequestTimePlayed()` asks, each a `CMSG_PLAYED_TIME`: a count, as the request
    /// body is empty.
    pub fn take_played_time_asks(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().played_time_asks)
    }

    /// Push the bind location's name, resolved from `SMSG_BINDPOINTUPDATE`'s AreaTable id through
    /// the catalog the hearthstone's `$z` uses, so the two agree; empty until the packet lands.
    pub fn set_bind_location(&mut self, area_name: &str) {
        let mut model = self.model_mut();
        if model.bind_location != area_name {
            model.bind_location = area_name.to_string();
        }
    }

    /// Seed the local player record (the reference's `0xc27d80`, [`super::PlayerRecord`]) from the
    /// char-enum row at Enter World, before any addon file runs, as `UnitName("player")` at file
    /// scope is common. A record with no name is refused: nothing empties the reference's record
    /// once it holds a character, so the last one stands.
    pub fn set_player_record(&mut self, record: super::PlayerRecord) {
        if record.name.is_empty() {
            return;
        }
        let mut model = self.model_mut();
        if model.player_record != record {
            model.player_record = record;
        }
    }

    /// Push the realm name, set at world entry from the realm the session connected to; the empty
    /// string means no realm yet, not a clear.
    pub fn set_realm_name(&mut self, realm: &str) {
        {
            let mut model = self.model_mut();
            if model.realm_name != realm {
                model.realm_name = realm.to_string();
            }
            // The persisted `realmName` CVar (`0x83f2d0`) is the same fact, set here so it cannot
            // drift from `GetRealmName()`; the engine write queues the change, so it is saved.
            super::cvars::set_from_engine(&mut model, "realmName", realm.to_string());
        }
    }

    /// Push the current framerate, from the app's frame clock each tick; this crate has no clock.
    pub fn set_framerate(&mut self, fps: f64) {
        self.model_mut().framerate = fps;
    }
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// Three strings and no fourth, as the in-game registrar pushes (`0x4884a0`); this VM is the
    /// in-game one, not the glue (`0x46cd70`, five).
    #[test]
    fn get_build_info_answers_as_the_1_12_1_client() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<Vec<String>>("return { GetBuildInfo() }").unwrap(),
            vec!["1.12.1", "5875", "Sep 19 2006"]
        );
        assert_eq!(
            s.arity("GetBuildInfo()").unwrap(),
            3,
            "three values, never a fourth"
        );
    }

    /// Never nil: addons index with it at file scope, and a nil index errors far from the cause.
    #[test]
    fn get_realm_name_is_empty_before_it_is_known_and_the_host_pushes_it() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<String>("return GetRealmName()").unwrap(), "");
        assert!(
            s.eval::<bool>("local t = {} t[GetRealmName()] = 1 return t[''] == 1")
                .unwrap(),
            "the empty string is a usable table key; nil is not, and that is the whole point"
        );
        s.set_realm_name("Whitemane");
        assert_eq!(
            s.eval::<String>("return GetRealmName()").unwrap(),
            "Whitemane"
        );
    }

    #[test]
    fn get_framerate_is_a_number_before_the_first_push() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<f64>("return GetFramerate()").unwrap(), 0.0);
        assert_eq!(
            s.eval::<String>("return format(\"%.1f\", GetFramerate())")
                .unwrap(),
            "0.0"
        );
        s.set_framerate(59.94);
        assert_eq!(
            s.eval::<String>("return format(\"%.1f\", GetFramerate())")
                .unwrap(),
            "59.9"
        );
    }

    /// A failure is recorded, not raised: `0x704ae0` runs the chunk under `lua_pcall(L, 0, 0, -2)`
    /// with the registry's error handler.
    #[test]
    fn run_script_evaluates_in_the_shared_state_and_reports_without_raising() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"RunScript("RunScriptProbe = 41 + 1")"#).unwrap();
        assert_eq!(s.eval::<i64>("return RunScriptProbe").unwrap(), 42);
        s.run(r#"RunScript("this is not lua")"#)
            .expect("a malformed script is caught inside RunScript, not raised at its caller");
        let errs = s.take_errors();
        assert!(
            errs.iter()
                .any(|e| e.starts_with("[string \"this is not lua\"]:1:")),
            "it must not vanish either — the handler channel gets it: {errs:?}"
        );
    }

    /// The reference's arities: `GetLocale` (`0x48d8b0`) one string, `IsMacClient` (`0x48c980`)
    /// one nil.
    #[test]
    fn the_client_identity_pair_answers_its_reference_arity() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("GetLocale()").unwrap(),
            1,
            "GetLocale pushes exactly one value"
        );
        assert_eq!(s.eval::<String>("return GetLocale()").unwrap(), "enUS");
        assert_eq!(
            s.arity("IsMacClient()").unwrap(),
            1,
            "IsMacClient pushes one value, and it is nil"
        );
        assert!(
            s.eval::<bool>("return IsMacClient() == nil").unwrap(),
            "nil is the PC arm, which is the arm this client wants"
        );
    }
}
