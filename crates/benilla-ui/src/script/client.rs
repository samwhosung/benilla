//! The client-identity and script verbs: build, locale, realm, bind location and framerate, with
//! `RunScript` and `FrameXML_Debug`. The realm, bind location and framerate read slots the app
//! pushes.

use mlua::{Lua, MultiValue, Value};

use super::{Model, UiScript};

/// The 1.12.1 client's build identity, as the strings in its binary spell it.
pub(super) const VERSION: &str = "1.12.1";
pub(super) const BUILD: &str = "5875";
pub(super) const BUILD_DATE: &str = "Sep 19 2006";

/// The release client's debug globals that do nothing (`0x83dec0`-`0x83ded0`, `0x83df80`-`0x83dfa0`).
const DEBUG_TOGGLES: [&str; 8] = [
    "ToggleTris",
    "TogglePortals",
    "ToggleCollision",
    "ToggleCollisionDisplay",
    "TogglePlayerBounds",
    "TogglePerformanceValues",
    "ResetPerformanceValues",
    "TogglePerformanceDisplay",
];

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `GetLocale()` (glue `0x46ce40`, in-game `0x48d8b0`) returns one string, the name at the locale
    // index ([`locale_name`]). `IsMacClient()` (`0x48c980`) returns one nil, the PC build's answer, which
    // takes FrameXML's PC key-label arm.
    g.set(
        "GetLocale",
        lua.create_function(|lua, ()| lua.create_string(locale_name(lua)))?,
    )?;
    g.set("IsMacClient", lua.create_function(|_, ()| Ok(Value::Nil))?)?;
    // The PC build's other two platform answers: `IsWindowsClient()` (`0x48c960`) pushes the
    // number 1 and `IsLinuxClient()` (`0x48c990`) one nil. Arguments are ignored.
    g.set(
        "IsWindowsClient",
        lua.create_function(|_, _: MultiValue| Ok(1.0))?,
    )?;
    g.set(
        "IsLinuxClient",
        lua.create_function(|_, _: MultiValue| Ok(Value::Nil))?,
    )?;

    // `GetDebugStats()` (`0x488af0`, `xor eax,eax; ret`): zero values, which `StatsFrame.lua:20`
    // hands straight to `SetText`.
    g.set(
        "GetDebugStats",
        lua.create_function(|_, _: MultiValue| Ok(MultiValue::new()))?,
    )?;

    // The eight debug toggles, each returning zero values and reading no argument:
    // `ToggleTris`, `TogglePortals`, `ToggleCollision`, `ToggleCollisionDisplay` and
    // `TogglePlayerBounds` are `xor eax,eax; ret` (`0x489330`-`0x489360`, `0x489440`);
    // `TogglePerformanceValues` and `ResetPerformanceValues` call a bare `ret` (`0x489420`,
    // `0x489430`); `TogglePerformanceDisplay` (`0x489400`) flips `[0x885644]`, which nothing reads.
    for name in DEBUG_TOGGLES {
        g.set(
            name,
            lua.create_function(|_, _: MultiValue| Ok(MultiValue::new()))?,
        )?;
    }

    // `OpeningCinematic()` (`0x48c8c0`): sends an empty `CMSG_OPENING_CINEMATIC` (`0xf9`,
    // `0x48c8c9` into `0x5ab630`) and returns nothing; the server may answer with
    // `SMSG_TRIGGER_CINEMATIC`. Arguments are ignored.
    g.set(
        "OpeningCinematic",
        lua.create_function(|lua, _: MultiValue| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .opening_cinematic_asks += 1;
            Ok(())
        })?,
    )?;

    // `SetLayoutMode([mode])` (`0x488540`): a number or numeric string (`0x6f34d0`) truncates
    // (`0x40a2b0`, [`super::binding_abi::ftol`]) into the UI root's `+0xcf8`, anything else
    // stores 1 (`0x488573`); never raises, returns zero values. The press handler reads it
    // (`0x766301`).
    g.set(
        "SetLayoutMode",
        lua.create_function(|lua, v: Value| {
            let mode = lua.coerce_number(v)?.map_or(1, super::binding_abi::ftol);
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .layout_mode = mode;
            Ok(())
        })?,
    )?;

    // `SetConsoleKey(key)` (`0x488640`): a one-character or named key becomes the console's
    // toggle key (`0x63cb00`), a non-string the default `0x30e`. Its state has no reader here, so
    // it takes any argument, never raises, stores nothing and returns zero values: benilla has no
    // console screen.
    g.set(
        "SetConsoleKey",
        lua.create_function(|_, _: MultiValue| Ok(MultiValue::new()))?,
    )?;

    // `SetEuropeanNumbers([flag])` (`0x48dec0`): `GetBoolOrDefault(1, true)` ([`bool_or_default`],
    // `0x6f1c10`) into `[0xcf4d24]` (`0x771c40`), which makes `SetText` swap the dots and commas of
    // its numbers (`0x771d9f`; [`crate::strings::swap_number_separators`]). Returns zero values.
    g.set(
        "SetEuropeanNumbers",
        lua.create_function(|lua, args: MultiValue| {
            let on = super::binding_abi::bool_or_default(args.front(), true);
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .european_numbers = on;
            Ok(())
        })?,
    )?;

    // `FrameXML_Debug([v])` (`0x488440`): get-or-set of the XML loader's trace flag `[0xceea30]`,
    // which gates `crate::loader::LoadReport::traces`. The set arm runs only when `lua_isnumber`
    // (`0x6f34d0`, tested at `0x48845d`) accepts the argument, a number or a numeric string, and
    // stores it truncated toward zero (`0x40a2b0`), so `FrameXML_Debug(0)` clears it; any other
    // argument only reads. It returns the flag after the call.
    g.set(
        "FrameXML_Debug",
        lua.create_function(|lua, v: Value| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            if let Some(n) = lua.coerce_number(v)? {
                model.framexml_debug.set(super::binding_abi::ftol(n));
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
            // The chunk and every call it makes run under `[0x83de24]` (`0x48b9bb`), the macro
            // context the protected-action gate refuses ([`super::taint`]).
            let ran = lua
                .load(&*raw)
                .set_name(name)
                .set_mode(mlua::ChunkMode::Text)
                .into_function()
                .and_then(|chunk| {
                    super::taint::tag(lua, &chunk, &super::taint::Context::Macro);
                    chunk.call::<()>(())
                });
            if let Err(e) = ran {
                let bare = super::stdlib::lua_message(&e);
                lua.app_data_mut::<crate::script::Model>()
                    .expect("model")
                    .record_script_error(bare.clone(), bare);
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

    /// Drain the `OpeningCinematic()` calls, each an empty `CMSG_OPENING_CINEMATIC`.
    pub fn take_opening_cinematic_asks(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().opening_cinematic_asks)
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

impl UiScript {
    /// `SetEuropeanNumbers`'s switch `[0xcf4d24]`, a client global no UI rebuild resets.
    pub fn european_numbers(&self) -> bool {
        self.model_ref().european_numbers
    }

    /// Seat the switch a dying VM held in the VM that replaces it.
    pub fn set_european_numbers(&mut self, on: bool) {
        self.model_mut().european_numbers = on;
    }

    /// The client's locale index, `[0xc0e080]`.
    pub fn locale(&self) -> usize {
        self.model_ref().locale
    }

    /// Seat the install's locale index, which start-up reads from `Wow.ini` once (`0x402562`); an
    /// index past the table's eight names reads as 0.
    pub fn set_locale(&mut self, index: usize) {
        self.model_mut().locale = if index < crate::toc::LOCALES.len() {
            index
        } else {
            0
        };
    }

    /// `Wow.ini`'s `Region` as seated, 0 until the host seats the install's.
    pub fn client_region(&self) -> i32 {
        self.model_ref().client_region
    }

    /// Seat `Wow.ini`'s `Region`, which start-up hands the name check once (`0x402b7f`); a signed
    /// word there, and only 1 to 5 picks a charset pair of its own.
    pub fn set_client_region(&mut self, region: i32) {
        self.model_mut().client_region = region;
    }
}

/// The locale name table `0x8558a4` at the VM's locale index, `GetLocale`'s answer in either state.
pub(super) fn locale_name(lua: &Lua) -> &'static str {
    let index = lua.app_data_ref::<Model>().map_or(0, |m| m.locale);
    crate::toc::LOCALES[index]
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

    /// `GetLocale` answers the table `0x8558a4` at the seated index in either state (in-game
    /// `0x48d8b0`, glue `0x46ce40`); an index past the eight names reads as 0.
    #[test]
    fn get_locale_answers_the_seated_locale_in_game_and_in_glue() {
        for (kind, mut s) in [
            ("in-game", UiScript::new().unwrap()),
            ("glue", UiScript::new_glue().unwrap()),
        ] {
            let locale = |s: &UiScript| s.eval::<String>("return GetLocale()").unwrap();
            assert_eq!(locale(&s), "enUS", "{kind}: unseated");
            s.set_locale(3);
            assert_eq!(locale(&s), "deDE", "{kind}");
            s.set_locale(6);
            assert_eq!(locale(&s), "esES", "{kind}");
            s.set_locale(8);
            assert_eq!((s.locale(), locale(&s).as_str()), (0, "enUS"), "{kind}");
        }
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

    /// `IsWindowsClient` (`0x48c960`) pushes 1 and `IsLinuxClient` (`0x48c990`) nil, one value
    /// each; `GetDebugStats` (`0x488af0`) pushes none.
    #[test]
    fn the_platform_queries_and_debug_stats_answer_the_pc_build() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("IsWindowsClient()").unwrap(), 1);
        assert_eq!(s.eval::<f64>("return IsWindowsClient(0)").unwrap(), 1.0);
        assert_eq!(s.arity("IsLinuxClient()").unwrap(), 1);
        assert!(s.eval::<bool>("return IsLinuxClient() == nil").unwrap());
        assert_eq!(s.arity("GetDebugStats()").unwrap(), 0);
    }

    /// Each `OpeningCinematic()` queues one send and returns nothing.
    #[test]
    fn opening_cinematic_queues_one_send_per_call() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("OpeningCinematic()").unwrap(), 0);
        s.run("OpeningCinematic(1)").unwrap();
        assert_eq!(s.take_opening_cinematic_asks(), 2);
        assert_eq!(
            s.take_opening_cinematic_asks(),
            0,
            "the drain empties the queue"
        );
    }

    /// The two stateless setters and `SetLayoutMode` never raise, whatever they are handed, and
    /// return zero values.
    #[test]
    fn the_setters_take_any_argument() {
        let s = UiScript::new().unwrap();
        for name in ["SetEuropeanNumbers", "SetLayoutMode", "SetConsoleKey"] {
            for arg in ["", "nil", "1", "\"x\"", "{}", "\"ESCAPE\""] {
                assert_eq!(
                    s.arity(&format!("{name}({arg})")).unwrap(),
                    0,
                    "{name}({arg})"
                );
            }
        }
    }

    /// The eight debug globals exist and do nothing: an addon or a `/script` calling one with any
    /// arguments gets zero values and no error, where an undefined global raises.
    #[test]
    fn the_eight_debug_toggles_exist_and_return_nothing() {
        let s = UiScript::new().unwrap();
        for name in super::DEBUG_TOGGLES {
            assert_eq!(
                s.eval::<String>(&format!("return type({name})")).unwrap(),
                "function"
            );
            for args in ["", "1", "nil, {}"] {
                assert_eq!(
                    s.arity(&format!("{name}({args})")).unwrap(),
                    0,
                    "{name}({args})"
                );
            }
        }
        s.run("TogglePerformanceDisplay() TogglePerformanceDisplay() ToggleTris()")
            .expect("a call from a script raises nothing");
    }

    /// `SetEuropeanNumbers` stores its flag (`GetBoolOrDefault(1, true)`: absent is on, nil and 0
    /// are off) and, while it is set, a FontString's and a Button's `SetText` swap the dots and
    /// commas of their numbers; with it off the text is stored as given. An addon sets and
    /// reads it back through `GetText`.
    #[test]
    fn set_european_numbers_makes_set_text_swap_number_separators() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            local f = CreateFrame("Frame", "Owner")
            fs = f:CreateFontString("FS", "ARTWORK")
            btn = CreateFrame("Button", "Btn")
            function Put(text)
                fs:SetText(text)
                btn:SetText(text)
                return fs:GetText(), btn:GetText()
            end
        "#,
        )
        .unwrap();
        let put =
            |text: &str| -> (String, String) { s.eval(&format!("return Put({text:?})")).unwrap() };
        let same = |text: &str| (text.to_string(), text.to_string());
        assert_eq!(put("1,234.56"), same("1,234.56"), "off at boot");

        s.run("SetEuropeanNumbers()").unwrap();
        assert_eq!(put("1,234.56"), ("1.234,56".into(), "1.234,56".into()));
        assert_eq!(put("Costs 12.50 gold"), same("Costs 12,50 gold"));
        s.run("SetEuropeanNumbers(false)").unwrap();
        assert_eq!(put("1,234.56"), same("1,234.56"), "false is off");

        for (arg, on) in [
            ("1", true),
            ("0", false),
            ("nil", false),
            ("true", true),
            ("\"off\"", false),
            ("\"yes\"", true),
            ("{}", true),
            ("", true),
        ] {
            s.run(&format!("SetEuropeanNumbers({arg})")).unwrap();
            let want = if on { "3,5" } else { "3.5" };
            assert_eq!(put("3.5"), same(want), "SetEuropeanNumbers({arg})");
        }

        // A number argument is turned into its string first (`lua_tostring`), then swapped.
        s.run("SetEuropeanNumbers(1) fs:SetText(2.5)").unwrap();
        assert_eq!(s.eval::<String>("return fs:GetText()").unwrap(), "2,5");
    }
}
