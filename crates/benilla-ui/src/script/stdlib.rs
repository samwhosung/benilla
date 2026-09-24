//! The sandbox and the 1.12 stdlib layer on top of stock Lua 5.1.
//!
//! - [`sandbox`]: no filesystem, OS or native reach (`io`, `os`, `package`, `require`, `dofile`,
//!   `loadfile`, `load`, `module`, `newproxy` and `debug` removed), the reference's `debugstack`,
//!   and a text-only `loadstring`.
//! - [`install`]: the 1.12 bare globals (string, table and math aliases, the `debug*` family, the
//!   error-handler pair, the clock and zone getters), `time` and `date`, and a `string.format`
//!   that accepts positional `%N$` specs as the 1.12 client's does, rewriting them into a
//!   sequential format. Mixing positional and sequential specs raises, as in the reference,
//!   whose single argument cursor a positional spec desyncs.

use mlua::{Lua, Value, Variadic};

/// An mlua error's bare Lua message, as `loadstring` returns it: without the `syntax error: ` or
/// `runtime error: ` word mlua's `Display` adds and the reference never does.
pub(super) fn lua_message(e: &mlua::Error) -> String {
    match e {
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::RuntimeError(m) => m.clone(),
        other => other.to_string(),
    }
}

/// Sandbox the VM: no filesystem, OS or native reach, the reference's `debugstack`, and text-only
/// chunk loading.
pub(super) fn sandbox(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // Set to nil explicitly, whatever mlua's safe stdlib already leaves out.
    for name in [
        "io", "os", "package", "require", "dofile", "loadfile", "load", "module", "newproxy",
        "debug",
    ] {
        g.set(name, Value::Nil)?;
    }

    // debugstack([start [, count1 [, count2]]]): the reference's traceback, frames only, in Lua
    // 5.0's wording. There is no `stack traceback:` header and line 1 is the caller's frame:
    // `AceDB-2.0.lua:742` skips one line to read its caller's addon folder, and `AceLibrary.lua:70`
    // reads line 1 as a frame. `AceLibrary.lua:139` needs 5.0's ``in function `name'`` quoting,
    // not later Luas' `'name'`. `start` defaults to 1, the caller; `count1`/`count2` are 5.0's
    // `LEVELS1`/`LEVELS2`, which `FuBarPlugin-2.0.lua:752` relies on with `debugstack(6, 1, 0)`.
    g.set(
        "debugstack",
        lua.create_function(|lua, args: Variadic<Value>| {
            let num = |v: Option<&Value>, default: usize| match v {
                Some(Value::Integer(i)) => usize::try_from(*i).unwrap_or(0),
                Some(Value::Number(n)) if *n >= 0.0 => *n as usize,
                _ => default,
            };
            let start = num(args.first(), 1);
            let count1 = num(args.get(1), 12);
            let count2 = num(args.get(2), 10);
            Ok(traceback_frames(lua, start, count1, count2))
        })?,
    )?;

    // Text-only `loadstring`: a binary chunk is rejected. The BOM strip and `#`-line skip apply,
    // as in the reference they live inside `luaL_loadbuffer`/`luaX_setinput`.
    let loadstring = lua.create_function(
        |lua, (src, chunkname): (mlua::String, Option<mlua::String>)| {
            let raw = src.as_bytes();
            let bytes = crate::source::chunk(&raw).to_vec();
            // The chunk name is the caller's string verbatim, defaulting to the source itself
            // (`luaB_loadstring` `0x703280`, `0x70329a`), which `luaO_chunkid` `0x6f5c40` renders
            // as `[string "…"]`. It comes from the raw bytes: `luaL_loadbuffer` `0x6f5690` strips
            // a BOM only after `0x703296` took the name.
            let name = match &chunkname {
                Some(n) => String::from_utf8_lossy(&n.as_bytes()).into_owned(),
                None => String::from_utf8_lossy(&raw).into_owned(),
            };
            let chunk = lua
                .load(bytes)
                .set_name(name)
                .set_mode(mlua::ChunkMode::Text);
            match chunk.into_function() {
                Ok(f) => Ok((Value::Function(f), Value::Nil)),
                // The failure leg returns nil and Lua's own message, unprefixed (`0x7032c6`,
                // `0x7032d2`).
                Err(e) => Ok((
                    Value::Nil,
                    Value::String(lua.create_string(lua_message(&e))?),
                )),
            }
        },
    )?;
    g.set("loadstring", loadstring)?;

    Ok(())
}

/// Install the stdlib layer: the bare globals, `time`/`date` and the positional `format`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // The default `geterrorhandler()` reports into the host's script-error channel; the
    // `__benilla_` prefix marks host plumbing, not a 1.12 global.
    lua.globals().set(
        "__benilla_script_error",
        lua.create_function(|lua, msg: mlua::Value| {
            let text = match &msg {
                mlua::Value::String(s) => s.to_string_lossy(),
                other => format!("{other:?}"),
            };
            lua.app_data_mut::<super::Model>()
                .expect("model app_data")
                .errors
                .push(text);
            Ok(())
        })?,
    )?;

    install_time(lua)?;
    lua.load(WOW_STDLIB)
        .set_name("=[benilla wow stdlib]")
        .set_mode(mlua::ChunkMode::Text)
        .exec()?;
    // Kept by identity: dispatch skips the default handler, whose channel already has the
    // message, and fires only one that FrameXML or an addon set.
    let default: mlua::Function = lua.load("return geterrorhandler()").eval()?;
    lua.set_named_registry_value(super::REG_DEFAULT_ERRORHANDLER, default)?;
    Ok(())
}

/// The stdlib layer in Lua, run once at construction.
const WOW_STDLIB: &str = r#"
-- ── the bare string family ─────────────────────────────────────────────────────────────────────
-- Every name here is present in the REAL 1.12 client's global table — the in-world `_G` captured
-- from the running reference client (19,572 entries), which is the
-- authority this file now answers to (decision 1189).
--
-- 1187 also installed `strmatch`, `strrev`, `gmatch`, `strlenutf8` and `strcmputf8i` from
-- Blizzard's *Era* enumeration, to get a Classic Era addon further. **None of them exist in
-- 1.12** — `string.match`/`gmatch` are Lua 5.1 additions the 5.0 client never had — and Era is
-- not our target. They are gone; adding a global the client does not have is how an addon that
-- feature-detects gets sent down a path we cannot honour.
strlen  = string.len
strsub  = string.sub
strupper = string.upper
strlower = string.lower
strrep  = string.rep
strfind = string.find
strbyte = string.byte
strchar = string.char
gsub    = string.gsub
tinsert = table.insert
tremove = table.remove
-- `getn`, `sort`, `foreach` and `foreachi` are bound by `lua50` (decision 1194), which owns the
-- 5.0 shapes of `table.getn`/`setn` and must not be shadowed by a second implementation here.

-- ── the bare math aliases (the same 1.12 global family; the reference FrameXML uses them
-- unqualified — WorldMapFrame's overlay pool calls ceil/mod, CooldownFrame floor, …). `mod` is
-- fmod under a 5.0-era name (stock 5.1 dropped math.mod).
ceil  = math.ceil
floor = math.floor
abs   = math.abs
max   = math.max
min   = math.min
sqrt  = math.sqrt
mod   = math.mod        -- 5.0's name for fmod; `math.fmod` is removed by `lua50` (decision 1194)
random = math.random
-- `randomseed` is an ENGINE global in 1.12 exactly like `random` beside it (the captured `_G` types
-- both `function engine`), and it is the half that was missing: `IgniteStatus` calls it at file
-- scope in its OnLoad and dies on `attempt to call global 'randomseed'`. Seeding a PRNG is the one
-- thing an addon does that has no FrameXML counterpart to copy, so the bare name is all it has.
randomseed = math.randomseed
-- `PI` is an ENGINE global in 1.12, not a FrameXML one: the reference reads it bare (UIParent.lua's
-- `elapsedTime * 2 * PI * ROTATIONS_PER_SECOND`, TabardFrame.lua l.61-68) and no shipped FrameXML
-- file ever assigns it (grepped over the whole 1.12 extraction) — so the client supplies it, here,
-- beside the rest of the bare math family.
PI = math.pi
-- The 1.12 bare trig globals are DEGREE-based (the reference CombatText's fountain scroll calls
-- cos/sin with degree arguments); rad/deg ride along as the same family.
function sin(d) return math.sin(math.rad(d)) end
function cos(d) return math.cos(math.rad(d)) end
rad = math.rad
deg = math.deg
-- The rest of the bare math family, each verified present in the real 1.12 client's global
-- table (decision 1189's captured `_G`; 1187 had reached for Blizzard's Era enumeration).
-- `tan` and the inverses follow sin/cos into DEGREES — same family, same convention; that is
-- consistent-with the verified sin/cos finding rather than separately byte-verified, and it is
-- the convention every addon rotation helper assumes (`atan2` returning degrees is why
-- `atan2(dy, dx)` feeds a texture rotation directly).
function tan(d) return math.tan(math.rad(d)) end
function asin(x) return math.deg(math.asin(x)) end
function acos(x) return math.deg(math.acos(x)) end
function atan(x) return math.deg(math.atan(x)) end
function atan2(y, x) return math.deg(math.atan2(y, x)) end
exp = math.exp
log = math.log
log10 = math.log10
frexp = math.frexp
ldexp = math.ldexp

-- ── the debug* family: six verified STUBS and two real ones ────────────────────────────────────
--
-- All eight, read at the bytes (`0x7027e0`..`0x702860`):
-- `debuginfo`, `debugload`, `debugprint`, `debugdump`, `debugbreak` and `debugtimestamp` are
-- **byte-identical `xor eax,eax; ret` stubs** — three bytes, no `call`, no memory write. They
-- cannot print, log, write or set a global, and they return ZERO Lua values. So these are not
-- no-ops we invented under a real name (the "absent capability" class 1203 named); they are the
-- reference's own no-ops, transcribed.
--
-- That is what lets `BasicControls.xml` stop guarding its `debuginfo()` call and transcribe
-- `_ERRORMESSAGE` verbatim, which it now does.
--
-- `debugprofilestart`/`debugprofilestop` are the two that are REAL (RDTSC via `0x4293d0`): start
-- latches, stop answers the elapsed milliseconds since it. Modelled on `GetTime`'s own clock —
-- the same monotonic session seconds the tick advances — because benilla has no cycle counter and
-- an addon uses these to time its own work, which milliseconds answer honestly.
do
    local function noop() end
    debuginfo = noop
    debugload = noop
    debugprint = noop
    debugdump = noop
    debugbreak = noop
    debugtimestamp = noop

    local profileStart = 0
    function debugprofilestart() profileStart = GetTime() end
    function debugprofilestop() return (GetTime() - profileStart) * 1000 end
end

-- ── the error handler (decision 1195) ──────────────────────────────────────────────────────────
-- `seterrorhandler(f)` / `geterrorhandler()` are ENGINE globals in 1.12 (the captured `_G` says
-- so), and `_ERRORMESSAGE` — the default handler they start out holding — is FrameXML's. That
-- split is why the pair lives here and the default is a plain function rather than a Rust binding:
-- our own transcribed UI can replace it exactly as the reference's `UIErrorsFrame` does.
--
-- The idiom this exists for is `geterrorhandler()(msg)` — an addon's pcall wrapper reporting a
-- caught error the way the client would. Without the pair that line is `attempt to call a nil
-- value` INSIDE an error path, which turns a recoverable addon fault into a dead addon.
do
    local handler = function(msg) __benilla_script_error(msg) end
    function seterrorhandler(f) handler = f end
    function geterrorhandler() return handler end
end

-- ── _G accessors (getglobal/setglobal: _G[name] get/set) ───────────────────────────────
function getglobal(name) return _G[name] end
function setglobal(name, value) _G[name] = value end

-- ── GetLocale: benilla ships/reads enUS data only (the 5875 MPQs + Spell.dbc enUS column) ──────
function GetLocale() return "enUS" end

-- ── GetTime: the FrameXML session clock (seconds, monotonic, arbitrary epoch — like the real
-- client's uptime-based GetTime). `__benilla_now` is advanced by `UiScript::tick` in the same
-- call that fires OnUpdate, so `GetTime()` deltas and accumulated `elapsed` agree. Reference
-- FrameXML (CastingBarFrame & co.) anchors cast windows on it.
__benilla_now = 0.0
function GetTime() return __benilla_now end

-- ── The zone-text family (decisions 0203 phase 1 + 0287).
-- The app pushes the host globals on an area change (the same shape as
-- GetTime) and fires MINIMAP_ZONE_CHANGED / the ZONE_CHANGED family; these getters just read the
-- cached slots, like the real bindings (0x48a0a0/c0/e0/100 read BSS caches). GetZoneText = the
-- zone name, replaced by the WMO interior's name indoors; GetRealZoneText = the WMO-immune zone
-- name; GetSubZoneText = the leaf subzone, "" when the leaf IS the zone (and indoors);
-- GetMinimapZoneText = subzone-else-zone. GetZonePVPInfo returns (pvpType or nil, factionName or
-- nil, isArena) — pvpType is friendly/hostile/contested, never "arena".
__benilla_zone_text = ""
function GetMinimapZoneText() return __benilla_zone_text end
__benilla_zone_name = ""
__benilla_real_zone_name = ""
__benilla_subzone_name = ""
__benilla_pvp_type = ""
__benilla_pvp_faction = ""
__benilla_pvp_arena = false
function GetZoneText() return __benilla_zone_name end
function GetRealZoneText() return __benilla_real_zone_name end
function GetSubZoneText() return __benilla_subzone_name end
function GetZonePVPInfo()
    local t, f = __benilla_pvp_type, __benilla_pvp_faction
    if t == "" then t = nil end
    if f == "" then f = nil end
    -- isArena is 1/nil, never a Lua boolean: the reference's third slot is `(nil) | (number)`
    -- like every other 1.12 predicate (decision 2118).
    return t, f, __benilla_pvp_arena and 1 or nil
end

-- ── GetGameTime: the server's in-game clock (hour, minute) — the reference reads the
-- SMSG_LOGIN_SETTIMESPEED-seeded clock the client advances by its timescale. The app pushes the
-- host globals when the game minute ticks (`crate::minimap::feed_game_time` — same shape as the
-- zone-text family); minute resolution is the API's own (the binding returns no seconds).
-- 0:00 until the first time packet lands.
__benilla_game_hour = 0
__benilla_game_minute = 0
function GetGameTime() return __benilla_game_hour, __benilla_game_minute end

-- ── NOT here: wipe / tostringall / strsplit / strjoin / strconcat / strtrim ────────────────────
-- Six 2.0+ names benilla used to define. 1.12 has none of them (absent from
-- `reference/1.12-globals.tsv`, from the stock chain, and from every registrar table), and the
-- reason they were kept — that the vanilla addon ecosystem assumes them — did not survive being
-- checked: the corpus callers are multi-client addons whose vanilla paths raise on a real 1.12
-- client too, or that define the name themselves under their own namespace (decision 2146).
-- An addon reaching for one of these is reaching for a later client's API, and it should find
-- what it would find there: nothing.

-- ── Blizzard's positional string.format (%N$) ──────────────────────────────────────────────────
do
    -- `_find`/`_sub` are captured for the same reason `_format` is: this wrapper runs on every
    -- `format` call in the UI and must not follow an addon's later replacement of `string.*`.
    -- They are CALLS and not method syntax because 1.12 installs no string metatable, so
    -- `fmt:find(...)` — which this used to be written as — raises there (decision 2101's
    -- left-open item, closed in `lua50::install`). No chunk of ours may use what the reference's
    -- VM cannot resolve; the parser enforces that for the grammar, and this is its runtime twin.
    local _format, _find, _sub, _len = string.format, string.find, string.sub, string.len
    local CONV = "diouxXeEfgGqscp"  -- Lua 5.1 conversion letters

    local function reformat(fmt, ...)
        if type(fmt) ~= "string" then
            return _format(fmt, unpack(arg, 1, arg.n))
        end
        -- fast path: no "%<digit>" at all ⇒ definitely no positional spec.
        if not _find(fmt, "%%%d") then
            return _format(fmt, unpack(arg, 1, arg.n))
        end

        local args = arg
        local pieces = {}      -- rebuilt (sequential) format fragments
        local order = {}       -- for each real conversion: the source arg index, or false=sequential
        local seen_pos, seen_seq = false, false
        local npieces, norder = 0, 0
        local i, len = 1, _len(fmt)

        while i <= len do
            local c = _sub(fmt, i, i)
            if c ~= "%" then
                npieces = npieces + 1; pieces[npieces] = c
                i = i + 1
            elseif _sub(fmt, i + 1, i + 1) == "%" then
                npieces = npieces + 1; pieces[npieces] = "%%"
                i = i + 2
            else
                local j = i + 1
                local ds, de = _find(fmt, "^%d+%$", j)  -- optional N$
                if ds then
                    seen_pos = true
                    norder = norder + 1; order[norder] = tonumber(_sub(fmt, ds, de - 1))
                    j = de + 1
                else
                    seen_seq = true
                    norder = norder + 1; order[norder] = false
                end
                -- copy flags/width/precision + the conversion letter
                local ce = _find(fmt, "[" .. CONV .. "]", j)
                if not ce then
                    error("invalid conversion in format string", 2)
                end
                npieces = npieces + 1; pieces[npieces] = "%" .. _sub(fmt, j, ce)
                i = ce + 1
            end
        end

        if seen_pos and seen_seq then
            error("cannot mix positional and sequential arguments in format string", 2)
        end

        local out, seq = {}, 0
        for k = 1, norder do
            local idx = order[k]
            if idx == false then
                seq = seq + 1
                out[k] = args[seq]
            else
                out[k] = args[idx]
            end
        end
        return _format(table.concat(pieces, "", 1, npieces), unpack(out, 1, norder))
    end

    string.format = reformat
    format = reformat
end
"#;

/// `time()` and `date([format [, when]])`: 1.12 engine globals (`0x7035a0`, `0x7033a0`), Lua 5.0's
/// `os.time`/`os.date` hoisted. `time` is wall-clock epoch seconds, as addons persist it across
/// sessions.
///
/// Deviation: `date` formats UTC where the reference uses local time, because the tree has no
/// timezone database.
fn install_time(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    g.set(
        "time",
        lua.create_function(|_, ()| Ok(crate::civil::unix_seconds()))?,
    )?;
    g.set(
        "date",
        lua.create_function(|lua, (fmt, when): (Option<String>, Option<i64>)| {
            // Bare `date()` is `%c`, as in Lua.
            let fmt = fmt.unwrap_or_else(|| "%c".to_string());
            let secs = when.unwrap_or_else(crate::civil::unix_seconds);
            // A leading `!` selects UTC, which every time here already is, so it is only consumed.
            let body = fmt.strip_prefix('!').unwrap_or(&fmt);
            // `*t` answers a table, as in Lua: an addon that loops until its `wday` matches
            // (`Accountant_WeekStart`) spins forever on anything else.
            if body == "*t" {
                let c = crate::civil::from_unix(secs);
                let t = lua.create_table()?;
                t.set("year", c.year)?;
                t.set("month", c.month)?;
                t.set("day", c.day)?;
                t.set("hour", c.hour)?;
                t.set("min", c.min)?;
                t.set("sec", c.sec)?;
                // Lua counts weekdays 1..7 from Sunday; ours is the 0-based index into `DAYS`.
                t.set("wday", c.weekday + 1)?;
                // Already 1-based, as in Lua.
                t.set("yday", c.yearday)?;
                // UTC has no daylight saving.
                t.set("isdst", false)?;
                return Ok(mlua::Value::Table(t));
            }
            Ok(mlua::Value::String(
                lua.create_string(format_epoch(secs, body))?,
            ))
        })?,
    )?;
    Ok(())
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

/// The `strftime` subset addons use; an unknown specifier is emitted verbatim.
fn format_epoch(secs: i64, fmt: &str) -> String {
    let crate::civil::Civil {
        year,
        month,
        day,
        hour,
        min,
        sec,
        weekday: wday,
        yearday: yday,
    } = crate::civil::from_unix(secs);
    let mut out = String::with_capacity(fmt.len() + 16);
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(spec) = chars.next() else {
            out.push('%');
            break;
        };
        let hour12 = match hour % 12 {
            0 => 12,
            h => h,
        };
        match spec {
            'a' => out.push_str(&DAYS[wday as usize][..3]),
            'A' => out.push_str(DAYS[wday as usize]),
            'b' | 'h' => out.push_str(&MONTHS[(month - 1) as usize][..3]),
            'B' => out.push_str(MONTHS[(month - 1) as usize]),
            'c' => out.push_str(&format!(
                "{} {} {:2} {:02}:{:02}:{:02} {}",
                &DAYS[wday as usize][..3],
                &MONTHS[(month - 1) as usize][..3],
                day,
                hour,
                min,
                sec,
                year
            )),
            'd' => out.push_str(&format!("{day:02}")),
            'H' => out.push_str(&format!("{hour:02}")),
            'I' => out.push_str(&format!("{hour12:02}")),
            'j' => out.push_str(&format!("{yday:03}")),
            'm' => out.push_str(&format!("{month:02}")),
            'M' => out.push_str(&format!("{min:02}")),
            'p' => out.push_str(if hour < 12 { "AM" } else { "PM" }),
            'S' => out.push_str(&format!("{sec:02}")),
            'w' => out.push_str(&format!("{wday}")),
            'x' => out.push_str(&format!("{month:02}/{day:02}/{:02}", year.rem_euclid(100))),
            'X' => out.push_str(&format!("{hour:02}:{min:02}:{sec:02}")),
            'y' => out.push_str(&format!("{:02}", year.rem_euclid(100))),
            'Y' => out.push_str(&format!("{year}")),
            '%' => out.push('%'),
            other => {
                out.push('%');
                out.push(other);
            }
        }
    }
    out
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
    // A tail call has nothing to name it from and takes the `" ?"` arm, known by its `what` or as
    // the `(tail call)` pseudo-frame.
    let tail_call = src.what == "t" || short == "(tail call)";
    match names.name.as_deref() {
        Some(name) if !name.is_empty() && !tail_call => {
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

/// `debugstack`'s walk (`0x703760`): Lua 5.0's `db_errorfb` without its header, each frame
/// followed by `"\n"` (`0x703971`) rather than preceded by `"\n\t"`, so line 1 is a frame.
/// Frames print while the level is `<= start + count1` (`0x703857`, `jbe`); then a failed
/// `getstack(level + count2)` probe prints that level as an ordinary frame, else `"...\n"` and
/// the last `count2` follow. So `count1 = 1` gives two frames on a two-deep stack, not one.
fn traceback_frames(lua: &Lua, start: usize, count1: usize, count2: usize) -> String {
    let mut out = String::new();
    let mut level = start;
    let mut first_part = true;
    while lua.inspect_stack(level, |_| ()).is_some() {
        level += 1;
        if level > start + count1 && first_part {
            first_part = false;
            if lua.inspect_stack(level + count2, |_| ()).is_none() {
                level -= 1; // nothing to elide: print this level after all
            } else {
                out.push_str("...\n");
                while lua.inspect_stack(level + count2, |_| ()).is_some() {
                    level += 1;
                }
            }
            continue;
        }
        if let Some(f) = lua.inspect_stack(level - 1, traceback_frame) {
            out.push_str(&f);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod date_table_tests {
    use crate::script::UiScript;

    /// `Accountant.lua:364-375` steps back a day until `date("*t", ct).wday` matches, which a
    /// string return never does.
    #[test]
    fn date_star_t_answers_a_table_that_walks_with_its_argument() {
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
