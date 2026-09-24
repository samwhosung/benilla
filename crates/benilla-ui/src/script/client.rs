//! **Client-identity and script-evaluation verbs** — the small engine globals an addon asks for
//! before it does anything else (1188 phase 5, prioritised by the phase 6 corpus).
//!
//! These are not a guess at what addons want. They are the top of the measured demand list: over a
//! 218-addon vanilla corpus, `GetBuildInfo` is called by 69 addons and `RunScript` by 52, and both
//! are called *at file scope* — so a missing one does not degrade an addon, it stops the addon's
//! very first chunk with `attempt to call global … (a nil value)`. `GetBuildInfo` alone was the
//! first error for the whole Ace/Atlas/AtlasLoot family, which is a large slice of the ecosystem.
//!
//! ## The host-fed pair
//!
//! `GetRealmName` and `GetFramerate` are the next two down that list — 24 addons are stopped dead
//! by `GetRealmName` at file scope, and it is the **top runtime wall in the corpus** after the
//! dialect gap. Both read a slot the app pushes, the same shape as the zone-text family: the
//! engine owns the verb, the host owns the fact.
//!
//! `GetRealmName` returning `""` before a realm is known is deliberate and is what the reference
//! does at the glue screen. An addon keys its saved variables on it (`db[GetRealmName()]`), so
//! answering `nil` would make that a `table index is nil` error one call deeper — the failure mode
//! we have been paying for all arc.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// The 1.12.1 client's own build identity, read out of `WoW.exe`'s string table rather than
/// remembered: `5875` and `1.12.1` sit adjacent in the binary, and `Sep 19 2006` is its only
/// build-date string.
///
/// **Hardcoding is the faithful answer here, not a shortcut.** benilla targets exactly 1.12.1,
/// so these are constants of the target, not of our build — an addon asking
/// `GetBuildInfo()` is asking "which client am I on", and the honest answer is the one we
/// implement the API of. Our *own* build stamp is a different question with a different verb
/// ([`crate::script`] has no binding for it, and the reference has none either).
const VERSION: &str = "1.12.1";
const BUILD: &str = "5875";
const BUILD_DATE: &str = "Sep 19 2006";
// There was a `TOC_VERSION = 11200` here, and `GetBuildInfo` pushed it as a fourth value. The
// in-game registrar pushes three; the interface number is a later expansion's
// return, and `benilla.toc`'s own `## Interface` line is where that number belongs.

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `GetLocale()` -> one string, and `IsMacClient()` -> nil, both ARITY-EXACT: `GetLocale
    // 0x46ce40`/`0x48d8b0` is `0 -> 1 (string?)`, `IsMacClient 0x48c980` is `0 -> 1 (nil)`, both
    // `exact`/`agree`.
    //
    // **`IsMacClient` answering nil is the whole verb, not a placeholder.** The table records the
    // return KIND as `(nil)` — this binding pushes nil on the PC build the reference was carved
    // from, and there is nothing else for it to say. FrameXML branches on it for Mac-only key
    // labels; nil takes the PC arm, which is the arm this client wants.
    //
    // `GetLocale` is the two-registration case the shape gate warns about (1843): a glue copy and
    // an in-game copy, same shape. Ours answers the one locale this client ships against — the
    // enUS `GlobalStrings.lua` the manifest loads and 1804's reference defaults are read from.
    // Decision 1880.
    g.set(
        "GetLocale",
        lua.create_function(|lua, ()| lua.create_string("enUS"))?,
    )?;
    g.set("IsMacClient", lua.create_function(|_, ()| Ok(Value::Nil))?)?;

    // **`FrameXML_Debug([v])` — the XML loader's own trace switch, get-or-set**.
    // `0x488440` reads the global `[0xceea30]`
    // through `0x6edb40`, and:
    //
    // - a **Lua-truthy** argument takes the SET arm (`0x48845d je` after `lua_toboolean 0x6f34d0`)
    //   — so `FrameXML_Debug(0)` genuinely disables it rather than being a masked no-op, because
    //   the NUMBER zero is truthy in Lua; only `nil`/`false` are not;
    // - the stored value is `lua_tonumber` truncated **toward zero** (`0x40a2b0`), so `1.9` is 1
    //   and a non-numeric string is 0, which is 5.0's `tonumber` coercion;
    // - an absent, nil or false argument is a pure GET and leaves the flag alone;
    // - it always returns ONE number — the flag's value *after* the call.
    //
    // The reference ships a call to it commented out in its own `BasicControls.xml:20`; the
    // corpus's consumer is `ImprovedErrorFrame`, which drives it off a saved `XMLDebug` CVar at
    // its OnLoad and died on the missing global. What it gates is
    // [`crate::loader::LoadReport::traces`].
    g.set(
        "FrameXML_Debug",
        lua.create_function(|lua, v: Value| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let truthy = !matches!(v, Value::Nil | Value::Boolean(false));
            if truthy {
                // `lua_tonumber`'s coercion, then truncate toward zero. Anything that will not
                // coerce is 0 — the same answer `0x6f3620` gives for a non-numeric argument.
                let n = lua.coerce_number(v)?.unwrap_or(0.0);
                model.framexml_debug.set(n.trunc() as i32);
            }
            Ok(model.framexml_debug.get())
        })?,
    )?;

    // version, build, date — three, and no fourth
    g.set(
        "GetBuildInfo",
        lua.create_function(|lua, ()| {
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(VERSION)?),
                Value::String(lua.create_string(BUILD)?),
                Value::String(lua.create_string(BUILD_DATE)?),
                // **THREE, not four.** The reference's in-game `GetBuildInfo 0x4884a0` pushes
                // `(string, string, string)`; the interface/TOC number is a later-expansion return
                // and 1.12 has no fourth value here. (Its GLUE twin `0x46cd70` pushes five — the
                // same name with a different shape per registrar table, which is why 1842's gate
                // keys on the table and not the name.).
            ]))
        })?,
    )?;

    // `RunScript(text)` — compile and run a chunk in the shared global state.
    //
    // `RunScript 0x48b980` (pair `0x83e288`, name `0x83ea60`) is how a macro body, a `/script`
    // slash command and every addon's "evaluate this snippet" helper reach Lua. It inherits the
    // same sandbox as our chunk loader: there is no `setfenv` here and none in the reference
    // either — a script runs with full API access.
    //
    // Read end to end, its contract is four facts, and three of them were wrong here (2136's
    // "left open", closed at the bytes):
    //
    // **1 · The chunk name is the SOURCE ITSELF, verbatim.** `0x48b9c7 mov edx,eax` and
    // `0x48b9c9 mov ecx,eax` hand `lua_tostring 0x6f3690`'s one return to
    // `0x704cd0 FrameScript_Execute(code, chunkname)` as BOTH arguments. `0x704cd0` strlens the
    // code (`0x704cd7`) and passes the name straight through `FS_DoBuffer 0x704ae0`
    // (`0x704b06 push ecx`, the fourth argument of `luaL_loadbuffer 0x6f5690` at `0x704b0c`)
    // untouched. No `=`, no `@` — so `luaO_chunkid 0x6f5c40` renders it by its third rule,
    // `[string "…"]`, exactly as `loadstring`'s default source-name does (2136 §4). `=[RunScript]`
    // was a placeholder that rendered plausibly; `=` is chunkid's *print-verbatim* marker and
    // `[RunScript]` is not a literal the image contains.
    //
    // **2 · A bad argument is a SILENT NO-OP, not a raise.** `0x48b988 call 0x6f3510`
    // (`lua_isstring` — tag-based, so a *number* passes) failing takes `0x48b98f je` straight to
    // `0x48b9f3 xor eax,eax; ret`. So does a NULL from `lua_tostring` (`0x48b99f`) and an EMPTY
    // string (`0x48b9a1 cmp byte ptr [eax],0`). There is no `luaL_error 0x6f4940` anywhere in the
    // function — `RunScript(nil)` does nothing at all.
    //
    // **3 · An error does NOT reach the caller.** `0x704ae0` pushes the registered error handler
    // from the registry first (`0x704afe`, `[0x8722c8]` at `LUA_REGISTRYINDEX 0xffffd8f0`) and
    // runs the chunk as `lua_pcall(L, 0, 0, -2)` (`0x704b68`); a *compile* failure takes the other
    // leg and pcalls that same handler with the message (`0x704b42`). Either way the raise is
    // consumed, the red line is the handler's doing, and the caller's next statement runs. Raising
    // here made one bad `RunScript` abort whatever ran it — a macro could take FrameXML down with
    // it.
    //
    // **4 · It answers zero values on every path** (`xor eax,eax` at both exits).
    g.set(
        "RunScript",
        lua.create_function(|lua, text: Value| {
            // `lua_isstring` + `lua_tostring`, in one: strings and numbers coerce, everything
            // else is `None` — and `None` is the silent return, per fact 2.
            let Some(s) = lua.coerce_string(text)? else {
                return Ok(());
            };
            let raw = s.as_bytes();
            if raw.is_empty() {
                return Ok(());
            }
            // The name is the source bytes as given. Same rule as `loadstring`'s default
            // (`stdlib::loadstring`), and for the same reason: the image passes one pointer twice.
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

    // `GetRealmName()` — the realm this session is on, as the realm list spells it.
    //
    // The corpus's most-wanted engine verb after the dialect fix: 24 addons stop on it, and they
    // stop *at file scope*, because the idiom is `MyAddonDB[GetRealmName()] = …` in a chunk's
    // opening lines. `""` until the app pushes one (the glue screen's own answer), never `nil`.
    g.set(
        "GetRealmName",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            lua.create_string(&model.realm_name)
        })?,
    )?;

    // `RequestTimePlayed()` — ask the server for /played, answered by `TIME_PLAYED_MSG`.
    //
    // **The request and the answer are two halves and only both are worth having.** The binding
    // queues a `CMSG_PLAYED_TIME` the app drains; the app fires `TIME_PLAYED_MSG(total, level)`
    // when `SMSG_PLAYED_TIME` lands. Shipping only this half would be 1203 exactly: `QuestHistory`
    // calls it and then waits on an event that never comes, which is quieter and worse than the
    // `attempt to call global` it gets today.
    //
    // It returns nothing — the answer arrives as an event, never as a return value.
    g.set(
        "RequestTimePlayed",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .played_time_asks += 1;
            Ok(())
        })?,
    )?;

    // `GetBindLocation()` — the hearthstone's bind location NAME, as the reference's own hearth
    // confirmation prints it (`StaticPopup.lua:1742` formats it straight into the dialog text).
    //
    // Three corpus addons read it, in three separate files: FuBar_TransporterFu
    // (`TransporterFu.lua:456`), Necrosis (`Necrosis.lua:1089`) and _LazyPig (`LazyPig.lua:623`).
    //
    // **`""` until the app pushes one, never nil — the same choice `GetRealmName` above makes, for
    // the same reason.** Necrosis CONCATENATES the result (`..GetBindLocation()`), so a nil is a
    // raise rather than a blank; the reference holds a string buffer that is empty before
    // `SMSG_BINDPOINTUPDATE` lands, and no logged-in character is ever without a bind point, so the
    // empty window is ours alone. Conflating "not yet known" with "bound nowhere" is the price, and
    // it is the price the neighbouring verb already pays deliberately.
    g.set(
        "GetBindLocation",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            lua.create_string(&model.bind_location)
        })?,
    )?;

    // `GetFramerate()` — frames per second, as a number.
    //
    // 71 addons in the corpus call it; every performance readout on the ecosystem's FuBar/Titan
    // panels is this one verb. The app pushes a smoothed value each tick
    // ([`super::UiScript::set_framerate`]); 0 before the first push, which is what an addon's
    // `format("%.1f", GetFramerate())` needs to not error.
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
    /// Drain the `RequestTimePlayed()` asks queued since the last call — each one is a
    /// `CMSG_PLAYED_TIME`. [`super::UiScript::take_pvp_toggles`]'s shape, and a count for the same
    /// reason: the request packet is empty, so nothing distinguishes two asks but their number.
    pub fn take_played_time_asks(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().played_time_asks)
    }

    /// Push the hearthstone bind location's name — the host's half of `GetBindLocation`.
    ///
    /// The app resolves `SMSG_BINDPOINTUPDATE`'s AreaTable id through the same catalog the
    /// hearthstone's `$z` token already goes through, so the two can never disagree about where the
    /// player is bound. Idempotent; the empty string means the packet has not landed yet.
    pub fn set_bind_location(&mut self, area_name: &str) {
        let mut model = self.model_mut();
        if model.bind_location != area_name {
            model.bind_location = area_name.to_string();
        }
    }

    /// Push the realm name — the host's half of `GetRealmName`.
    ///
    /// Set at world entry from the realm the session actually connected to. Idempotent, and the
    /// empty string is a legitimate value (no realm yet), not a "clear".
    /// Seed the **local player record** — the reference's `0xc27d80`, copied from the char-enum
    /// row at the character-select Enter World commit (and see
    /// [`super::PlayerRecord`] for the bytes and the four verbs that read it).
    ///
    /// Called from the world-entry UI load beside [`Self::set_realm_name`], and for the same
    /// reason 1195 put the realm there: the values have to be in the VM before a single addon file
    /// runs, because `local currentPlayer = UnitName("player")` at file scope is the corpus idiom.
    /// The reference has them a whole login earlier still.
    ///
    /// **A record with no name is refused, not stored.** The only unset state is "no Enter World
    /// has been committed in this process"; once the record holds a character, nothing in the
    /// reference's image ever empties it again, so a caller with nothing to say must leave the
    /// last answer standing rather than blank it. The write is whole-record because the
    /// reference's is one `rep movsd`: these four fields describe one character and can never
    /// legitimately be seeded from two.
    pub fn set_player_record(&mut self, record: super::PlayerRecord) {
        if record.name.is_empty() {
            return;
        }
        let mut model = self.model_mut();
        if model.player_record != record {
            model.player_record = record;
        }
    }

    pub fn set_realm_name(&mut self, realm: &str) {
        {
            let mut model = self.model_mut();
            if model.realm_name != realm {
                model.realm_name = realm.to_string();
            }
            // The `realmName` CVar is the SAME fact, and it is set here so it cannot drift from
            // `GetRealmName()`. It is a real 1.12 CVar (`0x83f2d0`, persisted — the client builds
            // its SavedVariables path from it), and it had no
            // value at all here: `Ace/AceState.lua:27` is
            // `ace.trim(GetCVar("realmName"))` inside `SetGameState`, which EVERY Ace addon runs at
            // PLAYER_ENTERING_WORLD, so the nil became `gsub(nil, ...)` and took the family down.
            //
            // Through the **engine-write** path, not a bare slot poke. `realmName` is a persisted
            // CVar — its whole documented job is to be the last realm connected to — and a slot
            // written in place never reaches `cvar_changes`, so the host never hears it and never
            // dirties the config file. The value was therefore correct for exactly as long as the
            // process lived and absent from `config.toml` forever, which is the one thing a
            // persisted CVar must not be. `set_from_engine` is the same silent-on-unknown-name
            // write (a bare test VM registers nothing) and additionally queues the change.
            super::cvars::set_from_engine(&mut model, "realmName", realm.to_string());
        }
    }

    /// Push the current framerate — the host's half of `GetFramerate`.
    ///
    /// Pushed per tick from the app's own frame clock rather than computed here: this crate has no
    /// clock, and a number an addon polls every `OnUpdate` should not be re-derived per call.
    pub fn set_framerate(&mut self, fps: f64) {
        self.model_mut().framerate = fps;
    }
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// `GetBuildInfo` answers as the 1.12.1 client: **three strings, and no fourth value.**
    ///
    /// This used to assert four and explain the fourth as the `tocversion` an addon compares
    /// against its own `## Interface` — which is a TBC-and-later idiom. The in-game registrar
    /// `0x4884a0` pushes `(string, string, string)` and nothing else; the
    /// interface number arrives in a later expansion.
    ///
    /// The corpus agrees, and it is worth the count: **458 sites** write
    /// `local version = GetBuildInfo()` and take one value. **Four** destructure a fourth — and on
    /// the real 1.12 client those four read `nil`, which is what their authors shipped against.
    /// Handing them `11200` was benilla being more generous than the client, which is the superset
    /// 1189 exists to stop.
    ///
    /// **The same name has a different shape in the glue registrar** (`0x46cd70`, five strings).
    /// This VM is the in-game one. That pair is why 1842's gate keys on the table, not the name.
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

    /// `GetRealmName` answers `""` before the host pushes one — **never `nil`**, because the
    /// corpus idiom is `MyAddonDB[GetRealmName()] = …` at file scope and a nil index errors one
    /// call deeper, far from the cause.
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

    /// `GetFramerate` is a number from the first call, so `format("%.1f", GetFramerate())` in an
    /// addon's `OnUpdate` cannot error before the app has pushed a frame.
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

    /// `RunScript` runs its text in the shared global state, and a failure is **recorded, not
    /// raised** — `0x704ae0` runs the chunk under `lua_pcall(L, 0, 0, -2)` with the registry's
    /// error handler, so the raise never reaches the caller. (The full contract, and the three
    /// silent legs, are in `script::tests::stdlib`.)
    ///
    /// This test used to assert the opposite, on the reasoning that "a silent nil is a broken
    /// macro that looks fine". The reasoning was sound and the premise was not: the reference is
    /// not silent, it hands the message to the error handler and keeps going. Failing loudly *at
    /// the caller* is the part it does not do, and doing it meant one bad macro could abort the
    /// FrameXML function that ran it.
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

    /// `GetLocale` answers one string and `IsMacClient` answers nil — the reference's own shapes
    /// (`GetLocale 0x46ce40`/`0x48d8b0`, `IsMacClient 0x48c980`, both `exact`/`agree`), and the
    /// arity is the half a caller branches on.
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
