//! The sandbox and the 1.12 stdlib layer on top of stock Lua 5.1.
//!
//! - [`sandbox`]: no filesystem, OS or native reach (`io`, `os`, `package`, `require`, `dofile`,
//!   `loadfile`, `load`, `module`, `newproxy` and `debug` removed) and the reference's
//!   `debugstack`.
//! - [`install`]: the 1.12 bare globals the engine adds (the `debug*` family, the
//!   error-handler pair), `time` and `date`.

use mlua::benilla_lua::vm::{LuaError, Vm};
use mlua::{Lua, Value, Variadic};

/// An mlua error's bare Lua message: what `lua_pcall` leaves for a 1.12 error function (`0x703b40`)
/// and what `loadstring` returns. It drops the `syntax error: ` and `runtime error: ` words mlua's
/// `Display` adds, the `stack traceback:` block mlua's message handler appends to a raise, and the
/// wrappers a Rust callback's failure arrives in.
pub fn lua_message(e: &mlua::Error) -> String {
    match e {
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::RuntimeError(m) | mlua::Error::LuaValue { message: m, .. } => m
            .rfind("\nstack traceback:\n")
            .map_or(m.as_str(), |at| &m[..at])
            .to_string(),
        mlua::Error::CallbackError { cause, .. } | mlua::Error::WithContext { cause, .. } => {
            lua_message(cause)
        }
        other => other.to_string(),
    }
}

/// Sandbox the VM: no filesystem, OS or native reach, and the reference's `debugstack`.
pub(super) fn sandbox(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // The state opens without `io`, `os`, `package`, `require`, `dofile`, `loadfile`, `load`,
    // `module`, `newproxy` or `debug`, and none is set to nil here: that would make the key.

    // debugstack([start [, count1 [, count2]]]): the reference's traceback, frames only, in Lua
    // 5.0's wording. There is no `stack traceback:` header and line 1 is the caller's frame:
    // `AceDB-2.0.lua:742` skips one line to read its caller's addon folder, and `AceLibrary.lua:70`
    // reads line 1 as a frame. `AceLibrary.lua:139` needs 5.0's ``in function `name'`` quoting,
    // not later Luas' `'name'`. `start` defaults to 1, the caller; `count1`/`count2` are 5.0's
    // `LEVELS1`/`LEVELS2`, which `FuBarPlugin-2.0.lua:752` relies on with `debugstack(6, 1, 0)`.
    g.set(
        "debugstack",
        lua.create_function(|lua, args: Variadic<Value>| {
            let mut rest = args.iter().peekable();
            // Each level is taken off the front while `is-number` passes there (`0x703782`),
            // truncated (`0x40a2b0`) and removed (`0x7037a2`), so `"2"` is a level.
            let mut level = |default: i32| {
                let n = rest
                    .peek()
                    .and_then(|v| super::binding_abi::is_number(lua, (*v).clone()));
                match n {
                    Some(n) => {
                        rest.next();
                        super::binding_abi::ftol(n)
                    }
                    None => default,
                }
            };
            let (start, count1, count2) = (level(1), level(12), level(10));
            let mut stack: Vec<Value> = rest.cloned().collect();
            // A first remainder that is no string or number returns the stack's top (`0x70382d`).
            if let Some(first) = stack.first() {
                if !concatenable(first) {
                    return Ok(stack.pop().unwrap_or(Value::Nil));
                }
            }
            // The remainder stays on the stack and each line is concatenated with all of it
            // (`0x7039b0`), so it is joined in front of the frames; an empty stack is `""`
            // (`0x703814`).
            let frames = traceback_frames(lua, start, count1, count2);
            if !frames.is_empty() {
                stack.push(Value::String(lua.create_string(&frames)?));
            }
            concat_stack(lua, stack)
        })?,
    )?;

    Ok(())
}

/// Install the stdlib layer: the error handler, `time`/`date`, the profile verbs, the `debug*`
/// stubs.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    install_error_handler(lua)?;
    install_time(lua)?;
    install_profile(lua)?;
    lua.load(WOW_STDLIB)
        .set_name("=[benilla wow stdlib]")
        .set_mode(mlua::ChunkMode::Text)
        .exec()?;
    Ok(())
}

/// `debugprofilestart`/`debugprofilestop` (`0x702840`/`0x702860`), the two real `debug*` verbs:
/// start latches the processor's time-stamp counter (`rdtsc`, `0x4293d0`), and stop answers the
/// counter's delta over `trunc(1000 / scale) / 1000` (`0x7028a6`-`0x7028c4`), `scale` being the
/// millisecond clock's (`0x8332c8`). That is milliseconds only once the clock has moved onto the
/// counter; on a multi-core processor it never does (`0x42b8be`), the scale stays 1.0, and the
/// answer is a raw cycle count. Deviation: ours answers milliseconds, since a cycle count of unknown
/// rate is no use to an addon timing its code. Both read a live monotonic clock at the call, as the
/// counter is read, not the per-frame `GetTime()` clock, so a span inside one frame measures.
fn install_profile(lua: &Lua) -> mlua::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    // The latch as nanoseconds past this VM's origin; 0 until a start, as `[0xceeb58]` is.
    let origin = std::time::Instant::now();
    let start = std::sync::Arc::new(AtomicU64::new(0));
    let g = lua.globals();
    let latch = start.clone();
    let since_origin = move || u64::try_from(origin.elapsed().as_nanos()).unwrap_or(u64::MAX);
    g.set(
        "debugprofilestart",
        lua.create_function(move |_, ()| {
            latch.store(since_origin(), Ordering::Relaxed);
            Ok(())
        })?,
    )?;
    g.set(
        "debugprofilestop",
        lua.create_function(move |_, ()| {
            let delta = since_origin().saturating_sub(start.load(Ordering::Relaxed));
            Ok(delta as f64 / 1e6)
        })?,
    )?;
    Ok(())
}

/// The value `seterrorhandler` holds, `None` while the slot is -1 (no handler).
fn error_handler(lua: &Lua) -> Option<Value> {
    lua.named_registry_value::<Value>(super::REG_ERRORHANDLER)
        .ok()
        .filter(|v| !v.is_nil())
}

/// The error function every script runner passes `lua_pcall` (`0x703b40`, made at `0x703a5c` and
/// held at `[0x8722c8]`), so it runs at the raise, inside the failing script: `this`, `event` and
/// `arg1`..`argN` are still its own and its frames are on the stack. With the slot at -1
/// (`0x703b45`) it answers the message. Otherwise it pushes the held value under the message
/// (`lua_insert(-2)`) and calls it, `lua_call(1, 1)` (`0x703b6d`), answering its result: what a
/// status reads (`0x704b7f`) and what a failed `GetText` answers (`0x703cb2`). The call is
/// unprotected: a number raises `attempt to call a number value`, and any error there runs this
/// again for that error (`luaG_errormsg`, `0x6fc780`) until `luaD_call` throws status 5 at depth
/// 225 (`0x6f65cf`), `error in error handling`. The host's log reads the message from
/// [`with_capture`], since the call may answer anything.
fn error_function_native(vm: &mut Vm, nargs: usize) -> Result<usize, LuaError> {
    let msg = vm.arg(nargs, 0);
    let key = vm.str(super::REG_ERRORHANDLER.as_bytes());
    let held = vm.heap.table(vm.registry).get(key);
    if held.is_nil() {
        vm.push(msg);
        return Ok(1);
    }
    let text = vm.display(msg);
    let first = CAPTURES.with(|c| match c.borrow_mut().last_mut() {
        Some(top) if top.original.is_none() => {
            top.original = Some(text);
            true
        }
        _ => false,
    });
    match vm.call_value(held, &[msg], 1) {
        Ok(out) => {
            vm.push(
                out.first()
                    .copied()
                    .unwrap_or(mlua::benilla_lua::Value::Nil),
            );
            Ok(1)
        }
        Err(e) => {
            if first {
                let failed = vm.display(e.value);
                CAPTURES.with(|c| {
                    if let Some(top) = c.borrow_mut().last_mut() {
                        top.failed = Some(failed);
                    }
                });
            }
            Err(e)
        }
    }
}

/// What the error function saw for one runner's call: the message it was called with first and
/// what the handler raised on it. A runner's call nests inside a handler's only after it, so the
/// innermost runner's is the top.
#[derive(Default)]
struct Capture {
    original: Option<String>,
    failed: Option<String>,
}

thread_local! {
    static CAPTURES: std::cell::RefCell<Vec<Capture>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Run one runner's call with its own [`Capture`]. An error the held handler heard comes back as
/// the message it heard, with the handler's own failure after it, for the host's log.
pub(crate) fn with_capture<T>(call: impl FnOnce() -> mlua::Result<T>) -> mlua::Result<T> {
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            CAPTURES.with(|c| c.borrow_mut().pop());
        }
    }
    CAPTURES.with(|c| c.borrow_mut().push(Capture::default()));
    let pop = Pop;
    let r = call();
    let seen = CAPTURES.with(|c| c.borrow_mut().last_mut().map(std::mem::take));
    drop(pop);
    r.map_err(|e| match seen {
        Some(Capture {
            original: Some(original),
            failed,
        }) => mlua::Error::RuntimeError(match failed {
            Some(f) => format!("{original} — error handler failed: {f}"),
            None => original,
        }),
        _ => e,
    })
}

/// The error function the runners call scripts under, `[0x8722c8]`; `None` in a state that never
/// installed it.
pub(crate) fn error_function(lua: &Lua) -> Option<mlua::Function> {
    lua.named_registry_value::<Option<mlua::Function>>(REG_ERRORFUNCTION)
        .ok()
        .flatten()
}

/// The registry slot of [`error_function`].
const REG_ERRORFUNCTION: &str = "__benilla_errorfunction";

/// Run a script function as the runners do: under the error function (`0x704e4f`-`0x704e79`).
pub(crate) fn run_under_error_function(lua: &Lua, f: &mlua::Function) -> mlua::Result<()> {
    with_capture(|| match error_function(lua) {
        Some(h) => f.call_under::<()>(&h, ()),
        None => f.call::<()>(()),
    })
}

/// Call the Lua global `name` as the engine calls `GetText` (`0x703c43`-`0x703cad`) and
/// `GetPluralIndex` (`0x703d14`-`0x703d47`): looked up at the call, `lua_pcall` under the runners'
/// error function, one result. `Err` is a failed call's value, what the error function answered
/// (`0x703b40`); a global that is no function fails as `lua_pcall` does, at the call. The call
/// has a [`Capture`] of its own, so what the error function hears is never taken for the error of
/// the script it runs inside.
pub(crate) fn call_global_under_error_function(
    lua: &Lua,
    name: &str,
    args: impl mlua::IntoLuaMulti,
) -> Result<Value, Value> {
    let handler = error_function(lua);
    let raised = |e: mlua::Error| match e {
        mlua::Error::LuaValue { value: Some(v), .. } => v.get(lua).unwrap_or(Value::Nil),
        e => lua
            .create_string(lua_message(&e))
            .map_or(Value::Nil, Value::String),
    };
    let call = || match lua.globals().get::<Value>(name).unwrap_or(Value::Nil) {
        Value::Function(f) => match &handler {
            Some(h) => f.call_under::<Value>(h, args),
            None => f.call::<Value>(args),
        }
        .map_err(raised),
        other => {
            let message = format!("attempt to call a {} value", other.type_name());
            Err(match &handler {
                Some(h) => h.call::<Value>(message).unwrap_or_else(raised),
                None => lua
                    .create_string(&message)
                    .map_or(Value::Nil, Value::String),
            })
        }
    };
    // `Ok` either way, so the call's capture is dropped unread: the engine logs nothing of it.
    with_capture(|| Ok(call())).unwrap_or_else(|e| Err(raised(e)))
}

/// `FrameScriptExecuteBuffer` (`0x704ae0`): compile `src` as the chunk `name` and run it under the
/// error function (`lua_pcall(0, 0, -2)`, `0x704b68`). A chunk that fails to compile goes to the
/// error function directly, `lua_pcall(1, 0, 0)` (`0x704b42`): no `this` set, nothing on the
/// stack, and what that call raises is the host log's alone. `prepare` sees the compiled function
/// before it runs. Answers the compile or run error for the host's log.
pub(crate) fn execute_buffer(
    lua: &Lua,
    src: &[u8],
    name: impl AsRef<[u8]>,
    prepare: impl FnOnce(&mlua::Function),
) -> mlua::Result<()> {
    execute_buffer_reported(lua, src, name, prepare).map_err(|(e, _)| e)
}

/// [`execute_buffer`], its error paired with the text `0x704ae0` reports into a load status as
/// `%s` at severity 2: the compiler's message (`0x704b31`), or what the error function left on
/// the stack after the failed run (`0x704b8d`), read with `lua_tostring` (`0x6f3690`).
pub(crate) fn execute_buffer_reported(
    lua: &Lua,
    src: &[u8],
    name: impl AsRef<[u8]>,
    prepare: impl FnOnce(&mlua::Function),
) -> Result<(), (mlua::Error, Vec<u8>)> {
    let compiled = lua
        .load(src)
        .set_name(name)
        .set_mode(mlua::ChunkMode::Text)
        .into_function();
    match compiled {
        Ok(f) => {
            prepare(&f);
            let mut top = None;
            let ran = with_capture(|| {
                let r = match error_function(lua) {
                    Some(h) => f.call_under::<()>(&h, ()),
                    None => f.call::<()>(()),
                };
                if let Err(e) = &r {
                    top = Some(stack_text(lua, e));
                }
                r
            });
            ran.map_err(|e| (e, top.unwrap_or_default()))
        }
        Err(e) => {
            let message = e.lua_bytes(lua);
            call_error_function(lua, &message);
            Err((e, message))
        }
    }
}

/// `lua_tostring` of the value a failed protected call leaves: a string's bytes, a number as
/// text, and `(null)` for any other, which answers NULL to the `%s` (`0x82e280`).
fn stack_text(lua: &Lua, e: &mlua::Error) -> Vec<u8> {
    match e.lua_value(lua) {
        Some(v @ (mlua::Value::String(_) | mlua::Value::Integer(_) | mlua::Value::Number(_))) => {
            match lua.coerce_string(v) {
                Ok(Some(s)) => s.as_bytes().to_vec(),
                _ => b"(null)".to_vec(),
            }
        }
        Some(_) => b"(null)".to_vec(),
        None => e.lua_bytes(lua),
    }
}

/// The error function called directly on `message`, as [`execute_buffer`] calls it for a compile
/// failure (`0x704b42`), outside any failing call; a handler that fails there is logged with the
/// message it was given ([`with_capture`]).
pub(crate) fn call_error_function(lua: &Lua, message: &[u8]) {
    let Some(h) = error_function(lua) else {
        return;
    };
    // A capture of its own, so an enclosing runner's is not written.
    let called = with_capture(|| lua.create_string(message).and_then(|m| h.call::<()>(m)));
    if let Err(he) = called {
        lua.app_data_mut::<super::Model>()
            .expect("model app_data")
            .record_lua_error(&he);
    }
}

/// `seterrorhandler` / `geterrorhandler()`, engine globals in 1.12 (`0x702900`, `0x702950`) over
/// one registry reference, `[0x8722cc]`. It starts at -1 (`0x7039e0`), so `geterrorhandler()`
/// answers nil until something is set; in the running client FrameXML sets `_ERRORMESSAGE` first
/// (`BasicControls.xml:16`), so every addon finds that. `seterrorhandler` checks that argument 1 is
/// a function (`0x702908`), then drops the old reference and takes a new one on the stack's top
/// (`0x702930`): the last argument, whatever it is, and a nil there is -1 (`0x6f534a`), no handler.
fn install_error_handler(lua: &Lua) -> mlua::Result<()> {
    lua.set_named_registry_value(
        REG_ERRORFUNCTION,
        lua.create_native("?", error_function_native)?,
    )?;
    let g = lua.globals();
    g.set(
        "seterrorhandler",
        lua.create_function(|lua, args: Variadic<Value>| match args.first() {
            Some(Value::Function(_)) => {
                let top = args.last().cloned().unwrap_or(Value::Nil);
                lua.set_named_registry_value(super::REG_ERRORHANDLER, top)
            }
            _ => Err(mlua::Error::RuntimeError(
                "Usage: seterrorhandler(errfunc)".into(),
            )),
        })?,
    )?;
    g.set(
        "geterrorhandler",
        lua.create_function(|lua, ()| Ok(error_handler(lua).unwrap_or(Value::Nil)))?,
    )?;
    Ok(())
}

/// The stdlib layer in Lua, run once at construction.
const WOW_STDLIB: &str = r#"
-- ── the bare aliases are not here ──────────────────────────────────────────────────────────────
-- The bare table, math and string names (`strlen`, `tinsert`, `sin`, `PI`, …) are the client's own
-- start-up chunk, `compat.lua`, which `benilla_lua::new_state` runs; this file adds nothing to them.
--
-- `strmatch`, `strrev`, `gmatch`, `strlenutf8` and `strcmputf8i` come from Blizzard's *Era*
-- enumeration, which a Classic Era addon may reach for. **None of them exist in
-- 1.12** — `string.match`/`gmatch` are Lua 5.1 additions the 5.0 client never had — and Era is
-- not our target. They stay out; adding a global the client does not have is how an addon that
-- feature-detects gets sent down a path we cannot honour.

-- ── the debug* family: six verified STUBS and two real ones ────────────────────────────────────
--
-- All eight, read at the bytes (`0x7027e0`..`0x702860`):
-- `debuginfo`, `debugload`, `debugprint`, `debugdump`, `debugbreak` and `debugtimestamp` are
-- **byte-identical `xor eax,eax; ret` stubs** — three bytes, no `call`, no memory write. They
-- cannot print, log, write or set a global, and they return ZERO Lua values. So these are not
-- no-ops we invented under a real name (an "absent capability"); they are the
-- reference's own no-ops, transcribed.
--
-- That is what lets `BasicControls.xml` stop guarding its `debuginfo()` call and transcribe
-- `_ERRORMESSAGE` verbatim, which it now does.
--
-- `debugprofilestart`/`debugprofilestop` are the two that are REAL; they are Rust
-- (`install_profile`).
do
    local function noop() end
    debuginfo = noop
    debugload = noop
    debugprint = noop
    debugdump = noop
    debugbreak = noop
    debugtimestamp = noop
end


-- ── NOT here: wipe / tostringall / strsplit / strjoin / strconcat / strtrim ────────────────────
-- Six 2.0+ names benilla used to define. 1.12 has none of them (absent from
-- `reference/lua_globals.tsv`, from the stock chain, and from every registrar table), and the
-- reason they were kept — that the vanilla addon ecosystem assumes them — did not survive being
-- checked: the corpus callers are multi-client addons whose vanilla paths raise on a real 1.12
-- client too, or that define the name themselves under their own namespace.
-- An addon reaching for one of these is reaching for a later client's API, and it should find
-- what it would find there: nothing.
"#;

/// `time([t])` and `date([format [, when]])`: 1.12 engine globals (`0x7035a0`, `0x7033a0`), Lua
/// 5.0's `os.time`/`os.date` hoisted, over the client's statically linked MSVC runtime. `time` is
/// wall-clock epoch seconds, as addons persist it across sessions; a table and `date` read the
/// system's zone through the platform's runtime ([`crate::civil::local`]).
fn install_time(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    g.set(
        "time",
        lua.create_function(|lua, t: Option<mlua::Table>| {
            // Any argument but nil takes the table path (`0x7035b5`), `luaL_checktype` first.
            let Some(t) = t else {
                return Ok(mlua::Value::Number(crate::civil::unix_seconds() as f64));
            };
            let (fields, isdst) = date_fields(lua, &t)?;
            let secs = mktime_utc(&fields).and_then(|utc| crate::civil::local_seconds(utc, isdst));
            Ok(match secs {
                Some(secs) => mlua::Value::Number(f64::from(secs)),
                None => mlua::Value::Nil,
            })
        })?,
    )?;
    g.set(
        "date",
        lua.create_function(|lua, (fmt, when): (Option<mlua::String>, Option<f64>)| {
            // `luaL_optlstring(1, "%c")` read as a C string, `luaL_optnumber(2, -1)` through
            // `__ftol` to the 32-bit `time_t`, and -1 is now (`0x7033db`).
            let fmt = fmt.map_or_else(|| b"%c".to_vec(), |s| s.as_bytes().to_vec());
            let fmt = fmt.split(|&b| b == 0).next().unwrap_or_default();
            let secs = match when.map_or(-1, super::binding_abi::ftol) {
                -1 => crate::civil::unix_seconds(),
                s => i64::from(s),
            };
            // A leading `!` is `gmtime` (`0x7033f0`), else `localtime` (`0x73f125`).
            let (body, utc) = match fmt.strip_prefix(b"!") {
                Some(body) => (body, true),
                None => (fmt, false),
            };
            // Either refuses a negative time (`0x7421ee`), and `date` answers nil (`0x70341d`).
            if secs < 0 {
                return Ok(mlua::Value::Nil);
            }
            let (c, isdst) = if utc {
                (crate::civil::from_unix(secs), false)
            } else {
                crate::civil::local(secs)
            };
            // `*t` exactly, its NUL compared too (`0x70343c`), answers a table.
            if body == b"*t" {
                let t = lua.create_table()?;
                // The client's `date` (`0x7033a0`) sets them in this order (`0x703452`..`0x7034e2`).
                t.set("sec", c.sec)?;
                t.set("min", c.min)?;
                t.set("hour", c.hour)?;
                t.set("day", c.day)?;
                t.set("month", c.month)?;
                t.set("year", c.year)?;
                // Lua counts weekdays 1..7 from Sunday; ours is the 0-based index into `DAYS`.
                t.set("wday", c.weekday + 1)?;
                // Already 1-based, as in Lua.
                t.set("yday", c.yearday)?;
                t.set("isdst", isdst)?;
                return Ok(mlua::Value::Table(t));
            }
            // An empty or 256-byte answer is `strftime`'s 0 (`0x70350d`).
            match strftime(&c, isdst, body) {
                Some(text) => Ok(mlua::Value::String(lua.create_string(text)?)),
                None => Err(mlua::Error::RuntimeError("`date' format too long".into())),
            }
        })?,
    )?;
    Ok(())
}

/// The `struct tm` fields `time` reads, `year` from 1900 and `mon` from 0.
struct TmFields {
    sec: i32,
    min: i32,
    hour: i32,
    mday: i32,
    mon: i32,
    year: i32,
}

/// The `struct tm` fields `time` reads from its table (`0x7035f3`..`0x703666`), in its order:
/// `sec`, `min`, `hour`, `day`, `month` less 1 and `year` less 1900, as `int`s, by 5.0's
/// `getfield` (`0x7036f0`) for each field: `lua_gettable`, so `__index` answers; a number
/// or numeric string through `__ftol`, else the default, and `day`, `month` and `year` have none.
/// `isdst` (`getboolfield`, `0x7036b0`) is read last, as `lua_toboolean`: nil is false, never the
/// C library's "unknown".
fn date_fields(lua: &Lua, t: &mlua::Table) -> mlua::Result<(TmFields, bool)> {
    let field = |key: &str, default: Option<i32>| -> mlua::Result<i32> {
        let v: mlua::Value = t.get(key)?;
        match (lua.coerce_number(v)?, default) {
            (Some(n), _) => Ok(super::binding_abi::ftol(n)),
            (None, Some(d)) => Ok(d),
            (None, None) => Err(mlua::Error::RuntimeError(format!(
                "field `{key}' missing in date table"
            ))),
        }
    };
    let f = TmFields {
        sec: field("sec", Some(0))?,
        min: field("min", Some(0))?,
        hour: field("hour", Some(12))?,
        mday: field("day", None)?,
        mon: field("month", None)?.wrapping_sub(1),
        year: field("year", None)?.wrapping_sub(1900),
    };
    let isdst: mlua::Value = t.get("isdst")?;
    Ok((
        f,
        !matches!(isdst, mlua::Value::Nil | mlua::Value::Boolean(false)),
    ))
}

/// The runtime's `_mktime` (`0x74066b`, through `0x740687`) up to its zone: `tm_year` in
/// 69..=139, the month folded into the year, then days, hours, minutes and seconds, each step
/// refused on a 32-bit overflow (`0x740864`). `None` is its -1; the zone is
/// [`crate::civil::local_seconds`].
fn mktime_utc(f: &TmFields) -> Option<i32> {
    // `_days` (`0x832834`): the days before each month, less one.
    const DAYS: [i32; 13] = [-1, 30, 58, 89, 119, 150, 180, 211, 242, 272, 303, 333, 364];
    let years = 69..=139;
    let (mut year, mut mon) = (f.year, f.mon);
    if !years.contains(&year) {
        return None;
    }
    if !(0..=11).contains(&mon) {
        year += mon / 12;
        mon %= 12;
        if mon < 0 {
            mon += 12;
            year -= 1;
        }
        if !years.contains(&year) {
            return None;
        }
    }
    let mut before = DAYS[mon as usize];
    if year & 3 == 0 && mon > 1 {
        before += 1;
    }
    // Days since 1970 less one: `(y - 70) * 365 + ((y - 1) >> 2) - 17` (`0x740701`-`0x74070c`).
    let days = year * 365 + ((year - 1) >> 2) + before - 25_567;
    days.checked_add(f.mday)?
        .checked_mul(24)?
        .checked_add(f.hour)?
        .checked_mul(60)?
        .checked_add(f.min)?
        .checked_mul(60)?
        .checked_add(f.sec)
}

const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The C locale's date and time patterns in the runtime's time record (`0x8323a0`): `%x`
/// (`0x800aa0`), `%#x` (`0x800a8c`) and `%X` (`0x800a84`). `_setlocale` has no caller.
const SHORT_DATE: &[u8] = b"M/d/yy";
const LONG_DATE: &[u8] = b"dddd, MMMM dd, yyyy";
const TIME_OF_DAY: &[u8] = b"H:mm:ss";

/// The runtime's `strftime` (`0x4134e3`) into the 256-byte buffer `date` gives it (`0x7034fd`):
/// `None` when it answers 0, its output empty or filling the buffer. `isdst` is the time's
/// `tm_isdst`, false for `gmtime`'s.
fn strftime(c: &crate::civil::Civil, isdst: bool, fmt: &[u8]) -> Option<Vec<u8>> {
    let mut w = TimeWriter {
        c,
        isdst,
        out: Vec::with_capacity(64),
        left: 256,
        alternate: false,
    };
    let mut i = 0;
    // Deviation: a format that ends in `%` reads past its terminator in the client (`0x4135c3`),
    // into whatever memory follows the string; here the output ends there.
    while w.left > 0 && i < fmt.len() {
        if fmt[i] != b'%' {
            w.byte(fmt[i]);
            i += 1;
            continue;
        }
        // Each `%` clears `#` (`0x413592`), which marks the conversion after it.
        w.alternate = false;
        i += 1;
        if fmt.get(i) == Some(&b'#') {
            w.alternate = true;
            i += 1;
        }
        w.expand(fmt.get(i).copied().unwrap_or(0));
        i += 1;
    }
    (w.left > 0 && !w.out.is_empty()).then_some(w.out)
}

/// `strftime`'s state: the output, the room left in the buffer, and `__alternate_form`
/// (`0xcf7284`), the `#` mark.
struct TimeWriter<'a> {
    c: &'a crate::civil::Civil,
    isdst: bool,
    out: Vec<u8>,
    left: usize,
    alternate: bool,
}

impl TimeWriter<'_> {
    fn byte(&mut self, b: u8) {
        self.out.push(b);
        self.left -= 1;
    }

    /// `_store_str` (`0x4139e7`): while there is room.
    fn text(&mut self, s: &[u8]) {
        for &b in s {
            if self.left == 0 {
                break;
            }
            self.byte(b);
        }
    }

    /// `_store_num` (`0x413a0e`) after its case copied `#` into `_no_lead_zeros` (`0xcf7288`):
    /// the number's last `digits` digits, zero-padded, or with `#` its digits alone
    /// (`0x413a7d`). Too little room ends the output.
    ///
    /// Deviation: with `#` and one byte of room, `_store_number` writes no digit and its reversal
    /// swaps before it tests (`0x413ab3`), trading the last byte written for the stale byte after
    /// it in the caller's buffer, which then ends the client's answer; here the output is kept.
    fn num(&mut self, mut n: i32, digits: usize) {
        if self.alternate {
            let start = self.out.len();
            while self.left > 1 {
                self.byte(b'0' + (n % 10) as u8);
                n /= 10;
                if n <= 0 {
                    break;
                }
            }
            self.out[start..].reverse();
        } else if digits < self.left {
            let mut d = vec![0u8; digits];
            for slot in d.iter_mut().rev() {
                *slot = b'0' + (n % 10) as u8;
                n /= 10;
            }
            for b in d {
                self.byte(b);
            }
        } else {
            self.left = 0;
        }
    }

    /// `_expandtime` (`0x4135f8`): one conversion; an unknown one writes nothing.
    fn expand(&mut self, spec: u8) {
        let c = self.c;
        let (wday, yday) = (c.weekday as usize, c.yearday as i32 - 1);
        let tm_year = (c.year - 1900) as i32;
        match spec {
            b'a' => self.text(&DAYS[wday].as_bytes()[..3]),
            b'A' => self.text(DAYS[wday].as_bytes()),
            b'b' => self.text(&MONTHS[c.month as usize - 1].as_bytes()[..3]),
            b'B' => self.text(MONTHS[c.month as usize - 1].as_bytes()),
            // The short date, a space and the time; with `#` the long date (`0x413844`).
            b'c' => {
                let date = if std::mem::take(&mut self.alternate) {
                    LONG_DATE
                } else {
                    SHORT_DATE
                };
                self.pattern(date);
                if self.left == 0 {
                    return;
                }
                self.byte(b' ');
                self.pattern(TIME_OF_DAY);
            }
            b'd' => self.num(c.day as i32, 2),
            b'H' => self.num(c.hour as i32, 2),
            b'I' => self.num(
                match c.hour % 12 {
                    0 => 12,
                    h => h as i32,
                },
                2,
            ),
            b'j' => self.num(yday + 1, 3),
            b'm' => self.num(c.month as i32, 2),
            b'M' => self.num(c.min as i32, 2),
            b'p' => self.text(if c.hour > 11 { b"PM" } else { b"AM" }),
            b'S' => self.num(c.sec as i32, 2),
            // Weeks begun by a Sunday (`0x41377e`) or a Monday (`0x413760`), week 0 before it.
            b'U' | b'W' => {
                let first = match (spec, wday) {
                    (b'U', d) => d as i32,
                    (_, 0) => 6,
                    (_, d) => d as i32 - 1,
                };
                let week = if yday < first {
                    0
                } else {
                    yday / 7 + i32::from(yday % 7 >= first)
                };
                self.num(week, 2);
            }
            b'w' => self.num(wday as i32, 1),
            b'x' => {
                let date = if std::mem::take(&mut self.alternate) {
                    LONG_DATE
                } else {
                    SHORT_DATE
                };
                self.pattern(date);
            }
            b'X' => {
                self.alternate = false;
                self.pattern(TIME_OF_DAY);
            }
            b'y' => self.num(tm_year % 100, 2),
            b'Y' => self.num((tm_year / 100 + 19) * 100 + tm_year % 100, 4),
            // `_tzname[isdst]` (`0x413916`), the zone's standard name after `!`.
            b'z' | b'Z' => self.text(&crate::civil::zone_name(self.isdst)),
            b'%' => self.byte(b'%'),
            _ => {}
        }
    }

    /// `_store_winword` (`0x413ac6`) over a C-locale pattern: each run of one letter is a
    /// conversion by its length (`M` `m`, `MMMM` `B`, `dddd` `A`, `yy` `y`, ...), anything else a
    /// literal. The single letters' no-leading-zero mark (`0x413b9c`) is overwritten by the
    /// conversion's own read of `#` (`0x4138e2`), so `M/d/yy` pads as `MM/dd/yy` does.
    fn pattern(&mut self, p: &[u8]) {
        let mut i = 0;
        while i < p.len() && self.left > 0 {
            let ch = p[i];
            let run = p[i..].iter().take_while(|&&b| b == ch).count();
            let spec = match (ch, run) {
                (b'M', 1 | 2) => Some(b'm'),
                (b'M', 3) => Some(b'b'),
                (b'M', 4) => Some(b'B'),
                (b'd', 1 | 2) => Some(b'd'),
                (b'd', 3) => Some(b'a'),
                (b'd', 4) => Some(b'A'),
                (b'y', 2) => Some(b'y'),
                (b'y', 4) => Some(b'Y'),
                (b'h', 1 | 2) => Some(b'I'),
                (b'H', 1 | 2) => Some(b'H'),
                (b'm', 1 | 2) => Some(b'M'),
                (b's', 1 | 2) => Some(b'S'),
                _ => None,
            };
            match spec {
                Some(s) => {
                    self.expand(s);
                    i += run;
                }
                None => {
                    self.byte(ch);
                    i += 1;
                }
            }
        }
    }
}

/// `luaO_chunkid` (`0x6f5c40`), for `debugstack`'s `short_src`. An `@` name longer than
/// [`CHUNKID_KEEP`] keeps only its tail behind `"..."`, so a long addon path loses the `\AddOns\`
/// that corpus libraries search for.
fn chunk_id(source: &str) -> String {
    if let Some(rest) = source.strip_prefix('=') {
        // A `=name` chunk is taken verbatim, clipped to the buffer from the front.
        return rest.chars().take(LUA_IDSIZE - 1).collect();
    }
    if let Some(rest) = source.strip_prefix('@') {
        let n = rest.chars().count();
        if n <= CHUNKID_KEEP {
            return rest.to_string();
        }
        let tail: String = rest.chars().skip(n - CHUNKID_KEEP).collect();
        return format!("...{tail}");
    }
    // A string chunk: `[string "<first line, clipped>"]`.
    let first = source.split('\n').next().unwrap_or_default();
    let budget = LUA_IDSIZE - STRING_CHUNK_OVERHEAD;
    if first.chars().count() > budget || first.len() < source.len() {
        let clipped: String = first.chars().take(budget).collect();
        format!("[string \"{clipped}...\"]")
    } else {
        format!("[string \"{first}\"]")
    }
}

/// `LUA_IDSIZE`: the `short_src` buffer, 60 bytes with the terminator.
const LUA_IDSIZE: usize = 60;

/// Characters an `@` name keeps, tail-first, behind `"..."` (`0x6f5c40`).
const CHUNKID_KEEP: usize = 52;

/// What `[string "…"]` costs the budget in the third `luaO_chunkid` arm.
const STRING_CHUNK_OVERHEAD: usize = 17;

/// One `debugstack` frame, `short_src ":" [currentline ":"] DESC`, the caller pushing its `\n`.
/// DESC (`0x7038fa`) is `` in function `%s' `` for a named frame (`0x872c98`), else by `what`:
/// `" in main chunk"` (`0x872c88`), `" ?"` for C or a tail call (`0x872c6c`), or
/// `" in function <%s:%d>"` (`0x872c70`).
fn traceback_frame(d: &mlua::Debug) -> String {
    let src = d.source();
    let names = d.names();
    let short = src
        .source
        .as_deref()
        .map(chunk_id)
        .or_else(|| src.short_src.as_deref().map(str::to_owned))
        .unwrap_or_else(|| "?".to_string());
    let mut line = String::with_capacity(short.len() + 32);
    line.push_str(&short);
    line.push(':');
    if let Some(n) = d.current_line() {
        line.push_str(&n.to_string());
        line.push(':');
    }
    // A name wins over `what` (the switch on `namewhat`, `0x7038fa`); a tail call has none
    // (`0x6fbd20`) and takes the `" ?"` arm, known by its `what` or as the `(tail call)`
    // pseudo-frame.
    let tail_call = src.what == "t" || short == "(tail call)";
    match names.name.as_deref() {
        Some(name) if !name.is_empty() => {
            line.push_str(" in function `");
            line.push_str(name);
            line.push('\'');
        }
        _ if src.what == "main" => line.push_str(" in main chunk"),
        _ if src.what == "C" || tail_call => line.push_str(" ?"),
        _ => {
            line.push_str(" in function <");
            line.push_str(&short);
            line.push(':');
            line.push_str(&src.line_defined.unwrap_or(0).to_string());
            line.push('>');
        }
    }
    line
}

/// A value `lua_concat` joins without a metamethod: a string or a number.
fn concatenable(v: &Value) -> bool {
    matches!(v, Value::String(_) | Value::Integer(_) | Value::Number(_))
}

/// `lua_concat` over the whole stack (`0x6f44e0`): `luaV_concat` works down from the top, so the
/// raise names the first non-string it meets from there, the lower of a failing pair unless that
/// one is a string or number. One value is left as it is, none is `""`.
fn concat_stack(lua: &Lua, mut stack: Vec<Value>) -> mlua::Result<Value> {
    let Some(top) = stack.last() else {
        return Ok(Value::String(lua.create_string("")?));
    };
    if stack.len() == 1 {
        return Ok(stack.pop().unwrap_or(Value::Nil));
    }
    let below = &stack[stack.len() - 2];
    let bad = if !concatenable(below) {
        Some(below)
    } else if !concatenable(top) {
        Some(top)
    } else {
        stack.iter().rev().find(|v| !concatenable(v))
    };
    if let Some(bad) = bad {
        let kind = match bad {
            Value::LightUserData(_) => "userdata",
            other => other.type_name(),
        };
        return Err(mlua::Error::runtime(format!(
            "attempt to concatenate a {kind} value"
        )));
    }
    let mut out = Vec::new();
    for v in stack {
        if let Some(s) = lua.coerce_string(v)? {
            out.extend_from_slice(&s.as_bytes());
        }
    }
    Ok(Value::String(lua.create_string(&out)?))
}

/// `debugstack`'s walk (`0x703760`): Lua 5.0's `db_errorfb` without its header, each frame
/// followed by `"\n"` (`0x703971`) rather than preceded by `"\n\t"`, so line 1 is a frame.
/// The levels are 32-bit and wrap: a level prints while the next one is at most `start + count1`
/// compared unsigned (`0x70384c`, `0x703857`, `jbe`), so a negative limit prints every level. Past
/// it a failed `getstack(level + 1 + count2)` probe prints that level as an ordinary frame, else
/// `"...\n"` and the last `count2` follow. So `count1 = 1` gives two frames on a two-deep stack.
/// A level below 0 is `lua_getstack`'s tail-call pseudo-frame (`0x6fbad4`-`0x6fbade`), which
/// `0x6fbd20` fills as `(tail call)` with no line, so it prints `(tail call): ?`.
fn traceback_frames(lua: &Lua, start: i32, count1: i32, count2: i32) -> String {
    let exists = |level: i32| level < 0 || lua.inspect_stack(level as usize, |_| ()).is_some();
    let mut out = String::new();
    if !exists(start) {
        return out;
    }
    let limit = count1.wrapping_add(start) as u32;
    // `ebx` is the level after the one in hand (`0x70383c`).
    let (mut level, mut next, mut first_part) = (start, start.wrapping_add(1), true);
    loop {
        if next as u32 <= limit || !first_part {
            if level < 0 {
                out.push_str("(tail call): ?");
            } else if let Some(f) = lua.inspect_stack(level as usize, traceback_frame) {
                out.push_str(&f);
            }
            out.push('\n');
        } else {
            first_part = false;
            if !exists(next.wrapping_add(count2)) {
                next = next.wrapping_sub(1); // nothing to elide: print this level after all
            } else {
                out.push_str("...\n");
                loop {
                    next = next.wrapping_add(1);
                    if !exists(next.wrapping_add(count2)) {
                        break;
                    }
                }
            }
        }
        // `0x70398f`: the next level in hand, or the end.
        level = next;
        next = next.wrapping_add(1);
        if !exists(level) {
            return out;
        }
    }
}

#[cfg(test)]
mod date_table_tests {
    use crate::script::UiScript;

    /// `Accountant.lua:364-375` steps back a day until `date("*t", ct).wday` matches, which a
    /// string return never does.
    #[test]
    fn date_star_t_answers_a_table_that_walks_with_its_argument() {
        let _zone = crate::civil::ZoneGuard::set("UTC0");
        let s = UiScript::new().unwrap();
        // A Wednesday at 00:00 UTC: wday 4, counting from Sunday = 1.
        let t: i64 = 1_786_492_800;
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, bool)>(&format!(
                "local d = date(\"*t\", {t}) return d.year, d.month, d.day, d.wday, d.isdst"
            ))
            .unwrap(),
            (2026, 8, 12, 4, false)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(&format!(
                "local d = date(\"*t\", {t} + 3661) return d.hour, d.min, d.sec, d.yday"
            ))
            .unwrap(),
            (1, 1, 1, 224)
        );

        // Accountant's loop: one step back from Wednesday reaches Tuesday.
        assert_eq!(
            s.eval::<i64>(&format!(
                "local ct = {t} local n = 0 \
                 while date(\"*t\", ct).wday ~= 3 and n < 100 do ct = ct - 86400 n = n + 1 end \
                 return n"
            ))
            .unwrap(),
            1,
            "Wednesday(4) back to Tuesday(3) is exactly one step; a constant wday would spin"
        );

        // `!` is consumed, in the table form and the string form.
        assert_eq!(
            s.eval::<i64>(&format!("return date(\"!*t\", {t}).wday"))
                .unwrap(),
            4
        );
        assert!(
            !s.eval::<String>(&format!("return date(\"!%A\", {t})"))
                .unwrap()
                .contains('!'),
            "the UTC flag is consumed, not printed"
        );
        assert_eq!(
            s.eval::<String>(&format!("return date(\"%A\", {t})"))
                .unwrap(),
            "Wednesday"
        );
    }
}
