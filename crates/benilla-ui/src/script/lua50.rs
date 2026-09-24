//! **The Lua 5.0 dialect** — the layer that makes an mlua 5.1 VM answer the questions a 1.12 addon
//! asks about its interpreter (decision 1194).
//!
//! The 1.12.1 client embeds **Lua 5.0**; we embed mlua's `lua51`. Every prior decision in this
//! crate treated that as a detail — 0068 called the target "stock Lua 5.1", and the WoW stdlib
//! layer beside this one is written as aliases *onto* 5.1. It is not a detail. It is the single
//! largest measured blocker in the whole addon arc:
//!
//! ```text
//! what stopped them (addons whose FIRST load error was each):
//!    61  error: 'X' is obsolete          <- table.setn, raised by Lua 5.1
//! ```
//!
//! Sixty-one of 218 real vanilla addons, plus ~23 more that merely depend on one of them, stopped
//! on their first chunk. The chain is worth spelling out because it is not obvious and because
//! **we caused the last link ourselves**: `AceLibrary.lua` opens with
//!
//! ```lua
//! local version = GetBuildInfo()
//! if string.find(version, "^2%.") then
//!     table_setn = function() end   -- TBC: Lua 5.1, setn is gone
//! else
//!     table_setn = table.setn       -- 1.12: Lua 5.0, setn is real
//! end
//! ```
//!
//! Ace asks the client which client it is and picks the right dialect. We started answering
//! `"1.12.1"` truthfully (1192 phase 5, `GetBuildInfo`), Ace correctly took the 5.0 branch, and our
//! 5.1 VM raised. The addon was right, the API was right, and the interpreter underneath was
//! lying. That is what this module fixes.
//!
//! ## What it does, in three parts
//!
//! 1. **`table.setn`/`table.getn` get their 5.0 meaning back.** 5.0 remembers a table's size —
//!    in a numeric `t.n` field if one already exists, otherwise in a private weak-keyed table
//!    where nothing walking the table can see it. 5.1 removed the concept and made `setn` raise.
//!    The exact mechanism matters more than it looks: see [`LUA_5_0_TABLE_SIZE`] for the draft
//!    that got it wrong and the 39 addons that noticed.
//! 2. **The 5.1-only members are removed.** `string.gmatch`/`match`/`reverse`, `table.maxn`,
//!    `math.fmod`/`modf`/`huge`/`cosh`/`sinh`/`tanh`, and the whole `coroutine` table — none of
//!    which 1.12 has. This is 1189's *"a superset is not free"* one level below `_G`: an addon
//!    that writes `string.gmatch or string.gfind` (Ace does, on the very next line) picks the
//!    branch we leave standing, so leaving both standing chooses the wrong one for it.
//! 3. **The 5.0 compat globals the reference actually has get added** — `sort`, `foreach`,
//!    `foreachi`. `reference/1.12-globals.tsv` lists all three; we had the other seventeen of that
//!    family and not these.
//!
//! ## What this module does NOT have to do: the vararg `arg` table
//!
//! Lua 5.0 gives every vararg function an implicit `arg` table with an `n` field, and 1.12's
//! FrameXML uses it directly — `QuestTimerFrame_Update(...)` walks `for i=1, arg.n`, and so do
//! `FCFDropDown_LoadChannels`, `TradeSkill_OnEvent` and the GM-ticket and CVar families. Three of
//! our own transcriptions carry a note saying they rewrote it "the 5.1-native way" with
//! `select("#", ...)`, which reads as though the dialect demanded it.
//!
//! **It does not.** mlua's vendored 5.1 is built with `LUA_COMPAT_VARARG`, so `arg` and `arg.n`
//! are already there — measured, not assumed: `local function f(...) return arg.n end; f('a','b')`
//! answers 2 in this VM. That matters for 1751: a stock file using 5.0 varargs needs nothing from
//! this module, and the rewrite was a precaution against a problem we did not have.
//!
//! **And the other half of that, which costs an hour to rediscover: `...` is a DECLARATION token
//! here and nothing else.** This VM parses `function(x, ...)`, and then rejects every *use* of
//! `...` as an expression — `return ...`, `f(x, ...)`, and `local a = ...` at chunk level — with
//! `unexpected symbol near '...'`. `arg` and `unpack(arg)` are the only forwarding forms
//! ([`vararg_is_a_declaration_only`] pins all six). That is 5.0's own shape and exactly right for
//! the content we run, but it also binds anything WE write in Lua: an engine-side helper cannot
//! forward an unknown argument list without a table, so a verb whose arity matters (`GetPoint()`
//! vs `GetPoint(nil)`, `SetAllPoints()` vs `SetAllPoints(nil)`) has to stay on the Rust side.
//! Decision 2310 is where that bit — a Lua dispatcher for the Region method map would have been
//! ~4x cheaper than the Rust one and cannot be written in this dialect.
//!
//! ## The one known divergence, stated rather than hidden
//!
//! `table.insert`/`table.remove` stay on 5.1's `#t` border rather than consulting `getn`. **This
//! is byte-confirmed as a real divergence, not a suspicion**: 1.12's whole table library is
//! `n`-based — `luaL_getn 0x6f5050` is called by `insert`/`remove`/`concat`/`sort`/`foreachi` *and*
//! base `unpack`, and `insert`/`remove` **update** the stored size (`luaL_setn 0x6f4ea0`). So
//! `setn(t, 0)` on a non-empty table makes the next `insert` land at index 1 there and at
//! `#t + 1` here.
//!
//! It stays, and the trade was measured before it was made. Over the 218-addon corpus there are
//! 1,720 `setn` call sites; **1,471 pass 0** — always immediately after a
//! `for k in pairs(t) do t[k] = nil end` wipe, where `#t` is already 0 and the two agree — and 290
//! more pass `table.getn(source)` straight after copying that source, where they agree too. Fewer
//! than 20 sites could observe the difference, and none of them is reached at load. Replacing
//! `table.insert` with a Lua-level reimplementation to serve those would put an interpreted
//! function on the hottest path in every addon in existence. Revisit it if a *runtime* failure is
//! ever traced here — not before.
//!
//! ## Ground truth (decision 1196 — verified, no longer derived)
//!
//! This module first shipped with its member lists taken from Lua 5.0's published library
//! registrations and cross-checked against two artifacts we hold. Every list here is now read
//! straight off the array in the image:
//!
//! - **Lua 5.0**, on five independent discriminators — not just the `$Lua: Lua 5.0 …` blob at
//!   `0x811b30` but `LUA_REGISTRYINDEX = -10000` (5.1 uses −10002), a `luaT_eventname[]` pool with
//!   no `__mod`/`__len` (so **no `%` and no `#` operator**), and live `luaL_getn`/`luaL_setn`.
//! - **Five libraries are opened** by `InitLua 0x7039e0` and no more: base (a 36-entry array at
//!   `0x811e28` looped straight into `_G`), `string` (12, array `0x822d88`), `table` (8,
//!   `0x822d40`), `math` (24, `0x822c60`), and **`bit`** (8, `0x822c18`). `luaL_openlib` has four
//!   call sites image-wide, so `os`/`io`/`debug`/`coroutine` are absent by construction.
//! - **The compat globals are a Lua chunk, not C macros** — a 1310-byte `compat.lua` compiled into
//!   `.data` at `0x8722e8`, run last at init. That is why the set is a specific subset (`getn`
//!   aliased, `setn` deliberately not) rather than "whatever `LUA_COMPAT_*` gives you", and it is
//!   also where the **degree-based** bare `sin`/`cos`/`tan` come from — a convention this crate's
//!   stdlib layer had already reached independently.

use mlua::{Lua, Table, Value};

/// Members that exist in Lua 5.1 and **not** in Lua 5.0, by library.
///
/// Removed rather than left standing, per 1189: a feature-detecting addon branches on presence,
/// and the branch it should take is the one 1.12 gives it.
const REMOVED: &[(&str, &[&str])] = &[
    // 5.1 added the new pattern-matching spellings; 5.0 has only `gfind` and no `reverse`.
    ("string", &["gmatch", "match", "reverse"]),
    // 5.1 added `maxn`; 5.0's table library is concat/foreach/foreachi/getn/setn/sort/insert/remove.
    ("table", &["maxn"]),
    // 5.1 renamed `mod`→`fmod` (we keep `mod`, which 1.12 has as a bare global too) and added
    // `modf`, `huge`, and the hyperbolics.
    ("math", &["fmod", "modf", "huge", "cosh", "sinh", "tanh"]),
];

/// Lua 5.0's remembered table size, and the whole `n`-based table library that rides it
/// (decision 2102) — [`table_size`]'s own header is the mechanism, the byte census behind it, and
/// the runtime failure that retired this module's "revisit if ever traced" note.
mod table_size;

/// Install the dialect. Runs **before** the WoW stdlib layer, so its aliases bind the 5.0-shaped
/// functions rather than the 5.1 ones they replace.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── 1 · the `n`-based table library gets its 5.0 meaning back ─────────────────────────────
    table_size::install(lua)?;

    // ── 2 · the 5.1-only members go ───────────────────────────────────────────────────────────
    for (lib, names) in REMOVED {
        let Ok(t) = g.get::<Table>(*lib) else {
            continue;
        };
        for name in *names {
            t.set(*name, Value::Nil)?;
        }
    }
    // `coroutine` is not in the 1.12 client's `_G` at all — not a member gap, a whole library the
    // client does not open. It was on `reference_surface`'s beyond-1.12 exception list as
    // "inherited rather than chosen"; it is now neither.
    g.set("coroutine", Value::Nil)?;

    // `select` is 5.1's base library, not 5.0's, and the 1.12 client's `_G` has no row for it.
    // It sat on `reference_surface`'s beyond-1.12 exception list under a reason that expired twice
    // over: first "our transcribed FrameXML uses it in 16 files" — 1751's migration retired every
    // one of those files, measured at zero by 2142 — then "it is mlua's, not ours to remove
    // without the 5.1 varargs it comes with". 2101 deleted those varargs from the grammar, which
    // is what turns this from a trade into a deletion: with `...`-as-a-value gone, `select`'s one
    // idiomatic shape cannot be written at all, and what is left — `select(k, f())` — is a 5.1
    // spelling of the multiple assignment 5.0 writes with commas.
    //
    // **Nothing of ours called it in production**: 177 sites in 55 files were our own TESTS asking
    // a binding's arity, which is a host-side question and is now asked host-side
    // ([`super::UiScript::arity`]). The handful that need the count *inside* Lua use 5.0's own
    // answer, `arg.n` — which this VM hands every vararg function since 2101 deleted the arm that
    // cleared `VARARG_NEEDSARG`.
    g.set("select", Value::Nil)?;

    // **The string type has NO metatable in 1.12**, so `("x"):upper()` — and `s:sub(1, 2)` for a
    // string `s` — raises `attempt to index a string value` there and quietly worked here. 5.1's
    // `luaopen_string` ends in a `createmetatable` 5.0's does not have, and mlua runs 5.1.
    //
    // Read out of the client, and the claim is stronger than "nobody installs one": the type
    // *cannot* carry one, on the **reader** side, so the writer side never has to be argued.
    // `luaT_gettmbyobj 0x6f7bd0` is the one function the VM asks for any value's metamethod (7
    // rel32 callers, covering TM_INDEX/TM_NEWINDEX/TM_CALL/order/arith). It loads `o->tt` at
    // `0x6f7be2` and enumerates two tags — `sub ecx,5; je` (LUA_TTABLE), `sub ecx,2; je`
    // (LUA_TUSERDATA) — then falls through at `0x6f7bee` to `mov eax,0x811bc0`, `&luaO_nilobject`
    // in **`.rdata`**: sixteen zero bytes with no writers by construction. There is nothing to
    // read, so a `__index` for a string cannot exist however it were installed. `G+0x80` is
    // `tmname[15]`, the metamethod *name* strings, not a per-type metatable array; a ModRM/SIB
    // census of every ×4-indexed operand across the text section finds no such read into writable
    // `.data` at all.
    //
    // The writer agrees from the other end. `lua_setmetatable 0x6f4020` takes the same two arms
    // (`0x6f409e`, `0x6f40a3`) into one `mov [edx+8], esi` — 5.0 puts `metatable` at offset 8 of
    // both `Table` and `Udata` — and every other tag falls to `0x6f40a8`'s `xor eax,eax`: return
    // 0, write nothing. `lua_getmetatable 0x6f3cf0` carries the identical `{5, 7}` switch. And the
    // layout settles it a third way: `luaS_newlstr 0x6f9d00` puts a string's **hash** at `+8`
    // (`0x6f9dbb`), the very offset the other two use for the metatable pointer.
    //
    // `luaopen_string 0x7fd810` is six instructions, `[0x7fd810, 0x7fd827)` — `push 0;
    // push 0x822d88; mov edx,0x871938; call luaL_openlib; mov eax,1; ret` — with no
    // `createmetatable` step, and `luaL_openlib` does not reach `0x6f4020` either.
    //
    // So this is not a policy choice about a superset — it is the type system. Note the reference
    // also *raises* rather than no-ops if an addon tries it itself: base `setmetatable 0x702a40`'s
    // first act is `luaL_checktype(L, 1, LUA_TTABLE)`, which mlua's 5.1 matches.
    //
    // Same rule as `coroutine` above, one layer below `_G`: an addon writing `s:gsub(...)` or
    // testing `getmetatable("")` is asking which interpreter it is on, and the branch it should
    // get is 1.12's.
    lua.set_type_metatable::<mlua::String>(None);

    // ── 3 · the 5.0 compat globals the reference has and we lacked ────────────────────────────
    // `sort`/`foreach`/`foreachi` are in `reference/1.12-globals.tsv`; `setn` deliberately is not,
    // so no bare `setn` is installed even though `table.setn` now works. The client's compat set
    // is a specific subset, not "all of them".
    let table: Table = g.get("table")?;
    for name in ["sort", "foreach", "foreachi"] {
        if let Ok(f) = table.get::<Value>(name) {
            if !matches!(f, Value::Nil) {
                g.set(name, f)?;
            }
        }
    }
    // `getn` is re-bound here rather than left to the stdlib layer's `table.getn or …` fallback,
    // so the bare global and the member are the same function and cannot drift apart.
    g.set("getn", table.get::<Value>("getn")?)?;

    // ── 4 · what the binary said and nobody had asked (decision 1196) ─────────────────────────
    // `print` and `_VERSION` are **not in 1.12's `_G`** — the captured table says so, and the base
    // library's 36-entry array at `0x811e28` confirms why: neither is in it (`_VERSION`'s literal
    // is not even in the image). Both were on `reference_surface`'s exception list as "inherited
    // rather than chosen"; nothing of ours uses either.
    g.set("print", Value::Nil)?;
    g.set("_VERSION", Value::Nil)?;

    // `__pow` is a **global** in 1.12 (`function engine` in the captured table) — Lua 5.0
    // implements `^` by calling it, and 5.1 made the operator native, so the name simply
    // disappeared. Present here because an addon can see it, not because anything calls it.
    g.set(
        "__pow",
        lua.create_function(|_, (a, b): (f64, f64)| Ok(a.powf(b)))?,
    )?;

    install_bit(lua)?;
    install_gc(lua)?;
    install_assert(lua)?;

    Ok(())
}

/// The **`bit` library** — 1.12 opens one, and it is not stock Lua (decision 1196).
///
/// `InitLua 0x7039e0` opens exactly five libraries: base, `string`, `table`, `math`, and **`bit`**
/// (`0x7fadc0`, array `0x822c18`). The captured `_G` agrees — `bit` is there, attributed `engine`.
/// We had assumed the opposite: `attempt to index global 'bit'` showed up in the corpus survey and
/// was written off as an addon reaching for something 1.12 lacks. It was reaching for something
/// **we** lacked.
///
/// Eight functions, the array's own order and its own names. Semantics are 32-bit two's
/// complement, which is what every caller assumes (`band(flags, 0xFF)`); `arshift` is the
/// sign-propagating shift `rshift` is not, which is the only pair anyone gets wrong.
fn install_bit(lua: &Lua) -> mlua::Result<()> {
    let bit = lua.create_table()?;
    // Lua numbers are doubles; the client's bit ops truncate to a 32-bit int and return a signed
    // one. `as i64 as u32` is the C cast chain (`(unsigned)(int)x`) rather than a saturating one,
    // so `bnot(0)` answers -1 exactly as it does there.
    fn u32_of(v: f64) -> u32 {
        v as i64 as u32
    }
    fn out(v: u32) -> i64 {
        v as i32 as i64
    }
    bit.set(
        "bnot",
        lua.create_function(|_, a: f64| Ok(out(!u32_of(a))))?,
    )?;
    bit.set(
        "band",
        lua.create_function(|_, (a, b): (f64, f64)| Ok(out(u32_of(a) & u32_of(b))))?,
    )?;
    bit.set(
        "bor",
        lua.create_function(|_, (a, b): (f64, f64)| Ok(out(u32_of(a) | u32_of(b))))?,
    )?;
    bit.set(
        "bxor",
        lua.create_function(|_, (a, b): (f64, f64)| Ok(out(u32_of(a) ^ u32_of(b))))?,
    )?;
    // The shifts mask their count to 5 bits, like the x86 shift instructions the client compiles
    // to — `lshift(1, 32)` is `1`, not `0`, and an addon that shifts by a computed width relies on
    // it not trapping.
    bit.set(
        "lshift",
        lua.create_function(|_, (a, n): (f64, f64)| Ok(out(u32_of(a) << (u32_of(n) & 31))))?,
    )?;
    bit.set(
        "rshift",
        lua.create_function(|_, (a, n): (f64, f64)| Ok(out(u32_of(a) >> (u32_of(n) & 31))))?,
    )?;
    bit.set(
        "arshift",
        lua.create_function(|_, (a, n): (f64, f64)| {
            Ok(out(((u32_of(a) as i32) >> (u32_of(n) & 31)) as u32))
        })?,
    )?;
    // `bit.mod` — integer remainder, not `math.mod`'s float one. The library's own eighth entry.
    bit.set(
        "mod",
        lua.create_function(|_, (a, b): (f64, f64)| {
            let b = u32_of(b) as i32;
            if b == 0 {
                return Err(mlua::Error::runtime("bit.mod: division by zero"));
            }
            Ok(((u32_of(a) as i32) % b) as i64)
        })?,
    )?;
    lua.globals().set("bit", bit)
}

/// The **garbage-collector pair** — `gcinfo` answers TWO numbers and `collectgarbage` answers
/// NONE and takes a *number* (decision 2136).
///
/// Both are 5.0-shaped, and 5.1 changed both in ways that are observable from Lua — already
/// carried in `reference/1.12-shapes.tsv` as `gcinfo … 2 exact (number,number) agree` and
/// `collectgarbage … 0 exact () agree` — rows the return-shape gate never checked, because it
/// filters to `table_kind = global` and these two are `baselib`.
///
/// **`gcinfo 0x703200`** — extent `[0x703200, 0x703243)`, reads no argument and cannot raise:
/// `lua_getgccount 0x6f43f0` then `lua_getgcthreshold 0x6f43e0`, each shifted `>> 10`, then
/// `mov eax, 2`. So it is `(count_KB, threshold_KB)` — 5.1 kept the name and dropped the second
/// value, which is what we answered.
///
/// **`collectgarbage 0x703250`** — extent `[0x703250, 0x703273)`, `luaL_optnumber(L, 1, 0.0)` →
/// `_ftol` → `lua_setgcthreshold 0x6f4400` → `xor eax,eax`. Zero values, and a **number** argument:
/// 5.1's string options (`"count"`, `"collect"`, `"step"`, …) do not exist in the 35 bytes. The
/// two dialects are therefore *inverted* on argument type — 5.1 rejects the number the reference
/// requires, and accepts the strings the reference refuses:
///
/// | call | the reference | stock 5.1 (what we had) |
/// |---|---|---|
/// | `collectgarbage()` | threshold 0 → full collect, 0 values | 1 value |
/// | `collectgarbage(0)` | the same, explicitly | **raises** ``invalid option `0'`` |
/// | `collectgarbage("count")` | **raises** `number expected, got string` | returns a number |
///
/// **The threshold is modelled, and this is the one place a value is ours rather than the
/// image's.** 5.1 replaced 5.0's stop-the-world collector with an incremental one and deleted
/// `GCthreshold` outright, so there is no 5.1 counter to read. What a script *can* observe is the
/// rule that moves it, and that rule is 5.0's own: `luaC_collectgarbage` ends
/// `G->GCthreshold = 2*G->nblocks`, and `lua_setgcthreshold` writes `n << 10` then immediately
/// collects if the live count has already reached it. Both are reproduced below against mlua's
/// real heap, so `gcinfo()`'s second value tracks the first exactly as 5.0's does. The saturation
/// is the image's: `0x6f4400` compares **unsigned** against `0x3fffff`, so a negative `n` — or one
/// past 4194303 — saturates the threshold to `0xffffffff` bytes.
///
/// **The reach is measured, not assumed.** `local mem, threshold = gcinfo()` is
/// `AceAddon-2.0.lua`'s memory report (36 copies in the 219-addon corpus), and the next line is
/// `string.format("… %.3f MiB …", threshold / 1024)` — arithmetic on a nil, i.e. a raise inside
/// the report. pfUI's `modules/panel.lua:170` reads the same slot as `gckb` and guards it, so it
/// silently prints `UNAVAILABLE` where the reference prints the number. `collectgarbage` has 14
/// corpus call sites and every one of them is a bare `collectgarbage()` whose result is
/// discarded, so its arity has no reach today — the argument contract is what this fixes.
fn install_gc(lua: &Lua) -> mlua::Result<()> {
    /// 5.0's `GCthreshold`, in **bytes** — the registry cell standing in for `[G+0x24]`.
    const REG_GC_THRESHOLD: &str = "__benilla_gc_threshold";
    /// `lua_setgcthreshold 0x6f4400`'s unsigned bound, in kilobytes.
    const MAX_THRESHOLD_KB: i64 = 0x3f_ffff;

    fn live_bytes(lua: &Lua) -> usize {
        lua.used_memory()
    }
    /// 5.0's post-collection rule, `luaC_collectgarbage`: `GCthreshold = 2 * nblocks`.
    fn settle(lua: &Lua) -> mlua::Result<()> {
        let t = u64::try_from(live_bytes(lua))
            .unwrap_or(u64::MAX)
            .saturating_mul(2);
        lua.set_named_registry_value(REG_GC_THRESHOLD, t as f64)
    }

    settle(lua)?;

    // `gcinfo()` — no argument, two numbers, cannot raise.
    lua.globals().set(
        "gcinfo",
        lua.create_function(|lua, ()| {
            let live = live_bytes(lua);
            let mut threshold: f64 = lua.named_registry_value(REG_GC_THRESHOLD).unwrap_or(0.0);
            // **5.0's invariant, not just its arithmetic.** `luaC_checkGC` runs on allocation, so
            // the instant `nblocks` reaches `GCthreshold` a collection fires and re-settles the
            // threshold to `2 * nblocks`. A script therefore never observes a threshold at or
            // below the live count. We have no allocation hook, so the same rule is applied on
            // read: without it the cell set at construction goes stale as the heap grows and
            // `gcinfo()` reports a threshold *under* its own count, a state 5.0 cannot hold.
            if (live as f64) >= threshold {
                settle(lua)?;
                threshold = lua.named_registry_value(REG_GC_THRESHOLD).unwrap_or(0.0);
            }
            // Both shifts are the image's own `shr eax,0xa`.
            Ok(((live >> 10) as f64, (threshold as u64 >> 10) as f64))
        })?,
    )?;

    // `collectgarbage([n])` — a number, no return values, and a raise on anything that is not one.
    lua.globals().set(
        "collectgarbage",
        lua.create_function(|lua, arg: Value| {
            // `luaL_optnumber(L, 1, 0.0)`: absent or nil takes the default; a numeric STRING is
            // coerced (the image's `lua_tonumber` runs `strtod`); anything else is a type error.
            let n = match &arg {
                Value::Nil => 0.0,
                Value::Integer(i) => *i as f64,
                Value::Number(n) => *n,
                Value::String(s) => {
                    match s.to_str().ok().and_then(|t| t.trim().parse::<f64>().ok()) {
                        Some(v) => v,
                        None => {
                            return Err(mlua::Error::RuntimeError(format!(
                                "bad argument #1 to `collectgarbage' (number expected, got {})",
                                type_name(&arg)
                            )))
                        }
                    }
                }
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "bad argument #1 to `collectgarbage' (number expected, got {})",
                        type_name(other)
                    )))
                }
            };
            // `_ftol` truncates toward zero; `0x6f4400`'s unsigned compare saturates a negative
            // or oversized count to `0xffffffff` bytes.
            let kb = n.trunc();
            let threshold_bytes: f64 = if !(0.0..=MAX_THRESHOLD_KB as f64).contains(&kb) {
                u32::MAX as f64
            } else {
                kb * 1024.0
            };
            lua.set_named_registry_value(REG_GC_THRESHOLD, threshold_bytes)?;
            // `luaC_checkGC`: collect when the live count has reached the threshold, and the
            // collection then re-settles it to `2 * nblocks`.
            if (live_bytes(lua) as f64) >= threshold_bytes {
                lua.gc_collect()?;
                settle(lua)?;
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

/// The type name `luaL_typerror` would print — `luaT_typenames 0x811cd0`'s spelling.
/// **`assert` answers ONE value in 5.0, and all of its arguments in 5.1.**
///
/// `luaB_assert 0x7031a0` is five calls: `luaL_checkany 0x6f4bb0` on argument 1,
/// `lua_toboolean 0x6f3660`, and on the truthy leg `0x7031e5 call 0x6f3080` — `lua_settop(L, 1)`
/// — then `0x7031ea mov eax,1`. The false leg is
/// `luaL_optlstring(L, 2, "assertion failed!" /*0x872bdc*/)` into `luaL_error 0x6f4940`, which
/// longjmps. 5.1 replaced the settop/`mov eax,1` pair with `return lua_gettop(L)`, so
/// `assert(a, b)` answers two values there and one here.
///
/// Found by the shape gate's base-library arm once it grew an argument ladder — the row
/// (`arity 1 exact`) has been in `reference/1.12-shapes.tsv` since it was vendored, and a
/// no-argument probe could never reach it because `assert()` raises.
///
/// **Reach, measured rather than assumed:** 5 647 `assert(` sites across the 219-addon corpus and
/// 73 in the shipped FrameXML, and **zero** of them bind more than one name from the result. So
/// this buys the contract, not a fixed addon — the same footing as `collectgarbage`'s argument
/// (2136 §3). The next caller to write `local a, b = assert(f())` is the one that finds out.
fn install_assert(lua: &Lua) -> mlua::Result<()> {
    let f = lua.create_function(|lua, args: mlua::MultiValue| {
        let mut it = args.into_iter();
        // `luaL_checkany 0x6f4bb0` — an ABSENT argument 1 raises; an explicit `nil` does not, it
        // is the falsy leg below.
        let Some(v) = it.next() else {
            return Err(mlua::Error::RuntimeError(
                "bad argument #1 to `assert' (value expected)".into(),
            ));
        };
        if matches!(v, Value::Nil | Value::Boolean(false)) {
            // `luaL_optlstring` — a string or a number; anything else is its own bad-argument
            // raise, in 5.0's own quoting (2122).
            let msg = match it.next() {
                None | Some(Value::Nil) => "assertion failed!".to_string(),
                Some(other) => match lua.coerce_string(other.clone())? {
                    Some(s) => s.to_string_lossy(),
                    None => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "bad argument #2 to `assert' (string expected, got {})",
                            type_name(&other)
                        )))
                    }
                },
            };
            return Err(mlua::Error::RuntimeError(msg));
        }
        // `lua_settop(L, 1)`: the first argument, alone, whatever else was passed.
        Ok(v)
    })?;
    lua.globals().set("assert", f)
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Nil => "no value",
        Value::Boolean(_) => "boolean",
        Value::Integer(_) | Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Table(_) => "table",
        Value::Function(_) => "function",
        Value::Thread(_) => "thread",
        _ => "userdata",
    }
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// **`gcinfo()` answers TWO numbers, and the second one is what the corpus reads**
    /// (decision 2136).
    ///
    /// `0x703200` is `lua_getgccount` + `lua_getgcthreshold`, each `>> 10`, then `mov eax, 2`.
    /// 5.1 kept the name and dropped the second value, so we answered one — and the reach is not
    /// hypothetical: `AceAddon-2.0.lua`'s memory report is
    /// `local mem, threshold = gcinfo()` followed by `string.format("… %.3f MiB …",
    /// threshold / 1024)`, which is arithmetic on a nil, i.e. a raise *inside* the report, in a
    /// library 36 corpus addons embed. pfUI's `modules/panel.lua:170` reads the same slot as
    /// `gckb`, guards it, and so prints `UNAVAILABLE` where the reference prints a number.
    #[test]
    fn gcinfo_answers_two_numbers_as_1_12_does() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("gcinfo()").unwrap(),
            2,
            "`mov eax, 2` at 0x703239 — one value is 5.1's shape, not 1.12's"
        );
        assert_eq!(
            s.eval::<Vec<String>>("local a, b = gcinfo() return { type(a), type(b) }")
                .unwrap(),
            vec!["number".to_string(), "number".to_string()],
            "(number,number) — `1.12-shapes.tsv` types the row `agree`"
        );
        // 5.0's collector keeps `GCthreshold` above `nblocks` (`luaC_checkGC` collects the moment
        // they meet, and the collection re-settles the threshold to `2 * nblocks`), so a script
        // cannot observe the pair inverted.
        assert!(
            s.eval::<bool>("local c, t = gcinfo() return t > c")
                .unwrap(),
            "the threshold must sit above the live count, as 5.0's collector guarantees"
        );
        // AceAddon-2.0's own line, verbatim in shape — this is the raise the fix removes.
        assert!(
            s.eval::<String>(
                "local mem, threshold = gcinfo() \
                 return string.format('%.3f', threshold / 1024)"
            )
            .is_ok(),
            "AceAddon-2.0's memory report divides the second slot by 1024"
        );
    }

    /// **`collectgarbage` answers NOTHING and takes a NUMBER** (decision 2136) — the two dialects
    /// are inverted on the argument, so this is not a superset either way.
    ///
    /// `0x703250` is 35 bytes: `luaL_optnumber(L, 1, 0.0)` → `_ftol` → `lua_setgcthreshold` →
    /// `xor eax,eax`. There is no string-option dispatch anywhere in it — that is 5.1's
    /// `collectgarbage`, and on this client `gcinfo()` is the only way to read the counters.
    #[test]
    fn collectgarbage_answers_nothing_and_takes_a_number() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("collectgarbage()").unwrap(),
            0,
            "`xor eax,eax` at 0x70326f — 5.1 returns one value here"
        );
        // The reference's ONLY argument form, which stock 5.1 rejects as ``invalid option `0'``.
        assert_eq!(s.arity("collectgarbage(0)").unwrap(), 0);
        assert!(s.eval::<()>("collectgarbage(2048)").is_ok());
        // `lua_tonumber` coerces a numeric string through `strtod`, so this one is accepted.
        assert!(s.eval::<()>(r#"collectgarbage("100")"#).is_ok());
        // …and a non-numeric string is a type error, in 5.0's own words.
        let e = s
            .eval::<()>(r#"collectgarbage("count")"#)
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("bad argument #1 to `collectgarbage' (number expected, got string)"),
            "5.1's string options must not exist here: {e}"
        );
        // An explicit threshold above the live count is what `gcinfo` then reports back.
        assert!(s
            .eval::<bool>(
                "local c = gcinfo() collectgarbage(c * 4) local _, t = gcinfo() return t >= c"
            )
            .unwrap());
    }

    /// **A `loadstring` chunk is named by its own SOURCE, and an explicit name is used verbatim**
    /// (decision 2136).
    ///
    /// `luaB_loadstring 0x703280` pushes the string `luaL_checklstring` just returned as
    /// `luaL_optlstring`'s `def` (`0x70329a`), so the default chunk name is the source text and
    /// `luaO_chunkid 0x6f5c40` renders it `[string "…"]`. We prepended `=` — chunkid's
    /// "print verbatim, undecorated" marker — which stripped the wrapper off every explicit name,
    /// left a literal `@` on a path-shaped one, and named a nameless chunk `(loadstring)`, a
    /// literal the image does not contain. All four are the prefix of a player-visible error.
    #[test]
    fn a_loadstring_chunk_is_named_by_its_own_source() {
        let s = UiScript::new().unwrap();
        let raised = |src: &str| -> String {
            s.eval::<String>(&format!(
                "local f = loadstring({src}) local ok, e = pcall(f) return tostring(e)"
            ))
            .unwrap()
        };
        assert!(
            raised(r#""error('boom')""#).starts_with(r#"[string "error('boom')"]:1: boom"#),
            "the default name is the source: {}",
            raised(r#""error('boom')""#)
        );
        // `'='` — printed verbatim with the marker removed, exactly once.
        assert!(raised(r#""error('boom')", "=myname""#).starts_with("myname:1: boom"));
        // `'@'` — the file branch; the path prints plainly, without the marker.
        assert!(raised(r#""error('boom')", "@a/b.lua""#).starts_with("a/b.lua:1: boom"));
        // No marker — the third branch wraps it, which is what an unprefixed name gets. Four
        // corpus sites pass one (`loadstring(code, "safecall Dispatcher["..n.."]")`).
        assert!(raised(r#""error('boom')", "plain""#).starts_with(r#"[string "plain"]:1: boom"#));

        // The failure leg returns Lua's message unchanged — `load_aux` pushes nil and the string
        // and stops. mlua's `Display` prefixes a category word that the image never adds.
        let e: String = s
            .eval(r#"local f, e = loadstring("return 1+") return tostring(e)"#)
            .unwrap();
        assert_eq!(
            e, "[string \"return 1+\"]:1: unexpected symbol near `<eof>'",
            "the second return is the message verbatim, with no mlua decoration"
        );
    }

    /// **`assert` answers ONE value — 5.0's `lua_settop(L, 1); return 1`, not 5.1's
    /// `return lua_gettop(L)`** (`luaB_assert 0x7031a0`, settop at `0x7031e5`, `mov eax,1` at
    /// `0x7031ea`).
    ///
    /// Found by the shape gate's base-library arm once it grew an argument ladder; the row has
    /// said `arity 1 exact` since the table was vendored, and only a call with MORE than one
    /// argument can tell the two dialects apart.
    #[test]
    fn assert_answers_one_value_and_keeps_5_0_s_messages() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("assert(1, 2, 3)").unwrap(),
            1,
            "5.0 truncates to the first argument; 5.1 returns them all"
        );
        assert_eq!(s.eval::<i64>("return (assert(7, 'x'))").unwrap(), 7);
        // A truthy `false`-adjacent value is still truthy: only nil and false take the raise.
        assert_eq!(s.arity("assert(0)").unwrap(), 1);

        let raised = |call: &str| -> String {
            s.eval::<String>(&format!(
                "local ok, e = pcall(function() {call} end) return tostring(e)"
            ))
            .unwrap()
        };
        // `luaL_optlstring(L, 2, "assertion failed!" /*0x872bdc*/)`, then `luaL_error`.
        assert!(raised("assert(false)").contains("assertion failed!"));
        assert!(raised("assert(nil, 'my message')").contains("my message"));
        // A number is a string to `luaL_optlstring`; anything else is its own bad-argument raise,
        // in 5.0's backtick quoting (2122).
        assert!(raised("assert(false, 42)").contains("42"));
        assert!(
            raised("assert(false, {})").contains("bad argument #2 to `assert' (string expected"),
            "{}",
            raised("assert(false, {})")
        );
        // `luaL_checkany 0x6f4bb0`: an ABSENT argument 1 raises, an explicit nil does not.
        assert!(
            raised("assert()").contains("bad argument #1 to `assert' (value expected)"),
            "{}",
            raised("assert()")
        );
    }

    /// **The 61-addon fix, as the addon actually writes it.**
    ///
    /// This is `AceLibrary.lua`'s own opening, verbatim in shape: ask the client which client it
    /// is, take the 5.0 branch, use `table.setn`. Under mlua's stock 5.1 the last line raises
    /// `'setn' is obsolete` and the addon's first chunk dies — taking ~84 of 218 corpus addons
    /// with it, because the Ace/FuBar half of the ecosystem embeds this file.
    #[test]
    fn acelibrarys_own_dialect_probe_takes_the_5_0_branch_and_works() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            local table_setn
            local version = GetBuildInfo()
            if string.find(version, "^2%.") then
                table_setn = function() end
            else
                table_setn = table.setn
            end
            local t = { "a", "b", "c" }
            for k in pairs(t) do t[k] = nil end
            table_setn(t, 0)
            AceProbe = table.getn(t)
            "#,
        )
        .expect("Ace's dialect probe must not raise");
        assert_eq!(s.eval::<i64>("return AceProbe").unwrap(), 0);
    }

    /// `setn`/`getn` are 5.0's real pair — a remembered size that is **not** a no-op and **not** a
    /// field the table's owner can see.
    ///
    /// Both halves have already been got wrong once each. A no-op passes the Ace probe above and
    /// fails the round-trip here; an unconditional `rawset(t, "n", …)` passes both and breaks 39
    /// addons that walk the table (see [`LUA_5_0_TABLE_SIZE`]).
    #[test]
    fn getn_and_setn_remember_a_size_without_polluting_the_table() {
        let s = UiScript::new().unwrap();
        // No remembered size: the border, exactly as 5.1 answers.
        assert_eq!(s.eval::<i64>("return table.getn({1,2,3})").unwrap(), 3);
        // The size round-trips...
        assert_eq!(
            s.eval::<i64>("local t = {1,2,3} table.setn(t, 7) return table.getn(t)")
                .unwrap(),
            7
        );
        // ...and is INVISIBLE to anything walking the table. This is the assertion that would have
        // caught the first draft: AceOO-2.0's `_Embed` walks a mixin's fields with `next` and
        // errors on any the target already has.
        assert_eq!(
            s.eval::<i64>(
                "local t = {1,2,3} table.setn(t, 0) \
                 local c = 0 for k in pairs(t) do c = c + 1 end return c"
            )
            .unwrap(),
            3,
            "a remembered size must not appear as a key — 39 corpus addons died on a stray 'n'"
        );
        // A table that ALREADY carries a numeric `n` keeps using it, which is 5.0's own branch and
        // why `arg.n` behaves the way 5.0-era code expects.
        assert_eq!(
            s.eval::<i64>("local t = {1,2,3, n=3} table.setn(t, 7) return t.n")
                .unwrap(),
            7
        );
        // ...and the bare global is the same function, not a second implementation.
        assert!(
            s.eval::<bool>("return getn == table.getn").unwrap(),
            "a divergent `getn` global is how the two answers start disagreeing"
        );
    }

    /// The 5.1-only members are gone, so an addon's feature detection picks 1.12's branch.
    ///
    /// `string.gmatch or string.gfind` is the line **directly below** Ace's `setn` probe: with
    /// both present it takes `gmatch`, a function 1.12 does not have. That is 1189's superset
    /// argument one level below `_G`, and this is the test that keeps it true.
    #[test]
    fn the_5_1_only_members_are_not_offered() {
        let s = UiScript::new().unwrap();
        for expr in [
            "string.gmatch",
            "string.match",
            "string.reverse",
            "table.maxn",
            "math.fmod",
            "math.modf",
            "math.huge",
            "math.cosh",
            "coroutine",
        ] {
            assert!(
                s.eval::<bool>(&format!("return {expr} == nil")).unwrap(),
                "{expr} is a Lua 5.1 addition — 1.12 does not have it"
            );
        }
        // ...and the 5.0 spellings that replace them are present.
        for expr in ["string.gfind", "math.mod", "table.setn", "table.foreach"] {
            assert!(
                s.eval::<bool>(&format!("return {expr} ~= nil")).unwrap(),
                "{expr} is Lua 5.0's own spelling and 1.12 has it"
            );
        }
    }

    /// `mod` still works after `math.fmod` is removed — the stdlib layer aliased the 5.1 name.
    #[test]
    fn the_bare_math_family_survives_the_removals() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<f64>("return mod(7, 3)").unwrap(), 1.0);
        assert_eq!(s.eval::<f64>("return floor(2.7)").unwrap(), 2.0);
    }

    /// **`...` declares a vararg function here and can never be read.** The six forms, together,
    /// because the split between them is the whole surprise: the declaration parses, every use
    /// does not, and `arg` carries what `...` cannot (the module header's second vararg note).
    #[test]
    fn vararg_is_a_declaration_only() {
        let s = UiScript::new().unwrap();
        let parses = |src: &str| s.run(src).is_ok();
        // Declaration, and the 5.0 way to read what it captured.
        assert!(parses("return function(x, ...) return x end"));
        assert!(parses("return function(x, ...) return arg.n end"));
        assert!(parses(
            "local f = tostring return function(x, ...) return f(x, unpack(arg)) end"
        ));
        // Every use of `...` as an expression — including at chunk level, where 5.1 proper allows
        // it and this build does not.
        assert!(!parses("return function(x, ...) return ... end"));
        assert!(!parses(
            "local f = tostring return function(x, ...) return f(x, ...) end"
        ));
        assert!(!parses("local a = ... return a"));
    }

    /// The three compat globals the reference has and we did not.
    #[test]
    fn sort_foreach_and_foreachi_are_bare_globals_like_the_reference() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<String>("local t = {'c','a','b'} sort(t) return table.concat(t)")
                .unwrap(),
            "abc"
        );
        assert_eq!(
            s.eval::<i64>("local n = 0 foreach({1,2,3}, function() n = n + 1 end) return n")
                .unwrap(),
            3
        );
        assert_eq!(
            s.eval::<i64>("local n = 0 foreachi({1,2,3}, function() n = n + 1 end) return n")
                .unwrap(),
            3
        );
    }

    /// The `bit` library exists in 1.12 and we did not have it (decision 1196).
    ///
    /// `InitLua 0x7039e0` opens exactly five libraries and `bit` is the fifth; the captured `_G`
    /// lists it as `engine`. The corpus's `attempt to index global 'bit'` had been written off as
    /// an addon reaching past 1.12 — it was reaching past *us*.
    #[test]
    fn the_bit_library_is_present_with_the_references_eight_functions() {
        let s = UiScript::new().unwrap();
        for name in [
            "bnot", "band", "bor", "bxor", "lshift", "rshift", "arshift", "mod",
        ] {
            assert!(
                s.eval::<bool>(&format!("return type(bit.{name}) == 'function'"))
                    .unwrap(),
                "bit.{name} is one of the array's eight entries"
            );
        }
        // Two's complement, signed out — `bnot(0)` is -1, not 4294967295.
        assert_eq!(s.eval::<i64>("return bit.bnot(0)").unwrap(), -1);
        assert_eq!(
            s.eval::<i64>("return bit.band(0x1234, 0xFF)").unwrap(),
            0x34
        );
        assert_eq!(s.eval::<i64>("return bit.bor(0xF0, 0x0F)").unwrap(), 0xFF);
        assert_eq!(s.eval::<i64>("return bit.bxor(0xFF, 0x0F)").unwrap(), 0xF0);
        assert_eq!(s.eval::<i64>("return bit.lshift(1, 4)").unwrap(), 16);
        // The pair everyone gets wrong: rshift is logical, arshift propagates the sign.
        assert_eq!(s.eval::<i64>("return bit.rshift(-1, 28)").unwrap(), 15);
        assert_eq!(s.eval::<i64>("return bit.arshift(-1, 28)").unwrap(), -1);
        // The shift count masks to 5 bits, like the x86 instruction it compiles to.
        assert_eq!(s.eval::<i64>("return bit.lshift(1, 32)").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return bit.mod(17, 5)").unwrap(), 2);
    }

    /// `print` and `_VERSION` are **not** in 1.12's `_G`, and `__pow` is.
    ///
    /// All three are things a `_G`-shaped instrument could always have caught and nobody had
    /// looked: the first two sat on the beyond-1.12 exception list as "inherited rather than
    /// chosen", and the third was simply missing.
    #[test]
    fn the_base_library_matches_the_captured_globals() {
        let s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return print == nil").unwrap());
        assert!(s.eval::<bool>("return _VERSION == nil").unwrap());
        // 5.0 implements `^` by calling `__pow`; 5.1 made it native and the global vanished.
        assert_eq!(s.eval::<f64>("return __pow(2, 10)").unwrap(), 1024.0);
    }

    /// **The corpus's other dialect probe, answered the way 1.12's parser answers it** (2101,
    /// closing 1208).
    ///
    /// Ace2 asks "am I on Lua 5.1?" by compiling a 5.1-only construct — the vararg *expression*,
    /// which 5.0 has no grammar for (5.0 collects varargs into `arg`; the shipped 1.12 FrameXML
    /// reads `arg`/`arg.n` 64 times across 177 files and never once writes `...` as a value):
    ///
    /// ```lua
    /// local lua51 = loadstring("return function(...) return ... end") and true or false
    /// ```
    ///
    /// 170 `loadstring` sites across the 219-addon corpus are that one question. Answering it
    /// `true` puts every Ace-embedding addon on its client-2.0 branch — which is how Cartographer
    /// came to hook `CloseSpecialWindows`, a name 1.12's `_G` does not have.
    ///
    /// The fix is the parser, not a library: `simpleexp` (client `0x6fd240`) has no `TK_DOTS`
    /// arm, so `...` as a value falls to `prefixexp` and raises. The exact probe line is asserted
    /// here in the shape the addons write it.
    #[test]
    fn the_vararg_expression_is_a_syntax_error_as_it_is_on_1_12() {
        let s = UiScript::new().unwrap();
        assert!(
            s.eval::<bool>(
                r#"return (loadstring("return function(...) return ... end") and true or false) == false"#
            )
            .unwrap(),
            "the Ace2 lua51 probe must answer false, as it does on the 1.12 client"
        );
        // ...and it fails the way 5.0 fails it: `simpleexp`'s default arm -> `prefixexp`, whose
        // head accepts only `(` or a NAME. Same reject site, same words, as a stray `;`.
        let msg: String = s
            .eval(r#"local f, e = loadstring("return ...") return tostring(e)"#)
            .unwrap();
        assert!(
            msg.contains("unexpected symbol"),
            "5.0's own message for this, not a bespoke one: {msg}"
        );
        // The 5.0 half of the same question, which we already answered correctly (1194): `arg`.
        assert_eq!(
            s.eval::<i64>("local f = function(...) return arg.n end return f(1, 2, 3)")
                .unwrap(),
            3,
            "`arg` is 5.1's compat-vararg table, which is the form all of 1.12 FrameXML uses"
        );
        // And now EVERY vararg function gets one. 5.1 cleared VARARG_NEEDSARG for any function
        // that mentioned `...`; with the arm deleted nothing clears it, which is 5.0's rule.
        assert_eq!(
            s.eval::<i64>("local f = function(a, ...) return arg.n end return f(1, 2, 3)")
                .unwrap(),
            2
        );
        // A `...` in a PARAMETER LIST is 5.0's own grammar and must still parse — the deletion is
        // `simpleexp`'s arm, not `parlist`'s.
        assert!(s
            .eval::<bool>(r#"return loadstring("return function(...) return arg.n end") ~= nil"#)
            .unwrap());
    }

    /// **`#` and `%` are not in 1.12's grammar either** (2101) — the same class, the same
    /// byte evidence, gated in the same hunk-set.
    ///
    /// `getunopr` (`0x6fe0a0`) is a leaf testing exactly two tokens, `-` and `not`, so its
    /// `OPR_NOUNOPR` is 2 — a three-member enum, 5.0's, not 5.1's four-member one with
    /// `OPR_LEN`. `getbinopr` (`0x6fe0c0`) bases its switch at `'*'` (0x2A); `%` is 0x25, below
    /// the range, and reaches `OPR_NOBINOPR` = 14 — a fifteen-member `BinOpr`, again 5.0's. The
    /// metamethod-name pool at `0x871896` has neither `__len` nor `__mod`.
    #[test]
    fn the_length_and_modulo_operators_are_not_in_the_grammar() {
        let s = UiScript::new().unwrap();
        for probe in ["return #t", "return 7 % 3", "local n = #({1,2}) return n"] {
            assert!(
                s.eval::<bool>(&format!("return loadstring({:?}) == nil", probe))
                    .unwrap(),
                "{probe} is 5.1-only syntax; 1.12's parser rejects it"
            );
        }
        // The 5.0 spellings of both, which the reference has and every 1.12-era addon uses.
        assert_eq!(s.eval::<i64>("return table.getn({1,2,3})").unwrap(), 3);
        assert_eq!(s.eval::<i64>("return getn({1,2,3})").unwrap(), 3);
        assert_eq!(s.eval::<f64>("return math.mod(7, 3)").unwrap(), 1.0);
        assert_eq!(s.eval::<i64>("return string.len('abcd')").unwrap(), 4);
    }

    /// **The shapes the reference's own files are written in still load** — the other half of the
    /// gate, and the one that would make it a regression if it ever stopped being true.
    ///
    /// Measured, not assumed: the three constructs above appear **zero** times in the extracted
    /// 1.12 FrameXML (177 files), GlueXML, Blizzard's own addons, and the director's installed
    /// AddOns folder. This chunk is the 5.0 grammar those files actually use, including the three
    /// 5.0-isms the fork restores (1215's iterator-less generic-for, `LUA_COMPAT_LSTR`'s nesting
    /// long strings, 1315's constructor semicolon).
    #[test]
    fn the_5_0_grammar_the_reference_writes_still_parses() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            -- 5.0 varargs: the implicit `arg` table, the form all of 1.12 FrameXML uses.
            local function count(...) local n = 0 for i = 1, arg.n do n = n + arg[i] end return n end
            -- the iterator-less generic-for (1215)
            local sum = 0
            for k, v in { 3, 4 } do sum = sum + v end
            -- a table constructor with 5.0's compat semicolon (1315)
            local t = { a = 1; b = 2; }
            -- nesting long strings (LUA_COMPAT_LSTR = 2)
            local s2 = [[outer [[inner]] outer]]
            -- 5.0's own spellings for what `#`/`%` would say
            BENILLA_GRAMMAR_OK = count(1, 2, 3) + sum + t.a + t.b
                + table.getn({ 1, 2 }) + math.mod(7, 3) + string.len(s2)
            "#,
        )
        .expect("the reference's own grammar must still load");
        assert_eq!(
            s.eval::<i64>("return BENILLA_GRAMMAR_OK").unwrap(),
            6 + 7 + 3 + 2 + 1 + 21
        );
    }

    /// **The six `debug*` stubs are the REFERENCE's no-ops, not ours** — and that distinction is
    /// the whole reason they are allowed to exist under real 1.12 names.
    ///
    /// Six of the eight, `[0x7027e0, 0x702840)` — `debuginfo`, `debugload`, `debugprint`,
    /// `debugdump`, `debugbreak` and `debugtimestamp` — are byte-identical `xor eax,eax; ret` —
    /// three bytes, no call, no memory write, **zero Lua return values**. Only
    /// `debugprofilestart`/`stop` are real. A no-op we invented would be the "capability absent
    /// without a failure" class 1203 named;
    /// a no-op the client itself ships is a transcription.
    #[test]
    fn the_debug_family_is_six_stubs_and_two_real_ones() {
        let s = UiScript::new().unwrap();
        // Zero values, not nil — a count is the only check that can tell them apart, and
        // `arity` is where it is taken now that `select` is gone (2171).
        for name in [
            "debuginfo",
            "debugload",
            "debugprint",
            "debugdump",
            "debugbreak",
            "debugtimestamp",
        ] {
            assert_eq!(
                s.arity(&format!("{name}()")).unwrap(),
                0,
                "{name} returns nothing at all"
            );
        }
        // ...and the two that are real answer a number of milliseconds.
        let ms: f64 = s
            .eval("debugprofilestart() return debugprofilestop()")
            .unwrap();
        assert!(ms >= 0.0, "elapsed milliseconds, not nil: {ms}");
    }

    /// The divergence that USED to be pinned here is gone (decision 2102): `table.insert` now
    /// consults the remembered size, as `0x7fb6d8` does. The behaviour it asserted — an append
    /// landing at the border after `setn(t, 0)` — was traced to a real addon failure, so the
    /// mechanism moved to [`super::table_size`] and its tests moved with it. This is the one line
    /// that used to say otherwise, kept as the shape a reader will look for.
    #[test]
    fn table_insert_consults_the_remembered_size() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("local t = {1,2,3} table.setn(t, 0) table.insert(t, 9) return t[1]")
                .unwrap(),
            9,
            "5.0's insert writes index getn+1, which after setn(t, 0) is index 1"
        );
    }

    /// **`{ [1] = a, [2] = b, … }` is a LIST, and `pairs` walks it 1, 2, 3** — decision 2111, the
    /// fourth "restore what 5.1 changed" hunk in `third_party/lua-src`.
    ///
    /// 1.12's `recfield` keeps `cc->nh++` **inside** its `TK_NAME` arm (`0x6fd5a4`'s
    /// `cmp [edi+0x10],0x116` / `0x6fd5ad jne 0x6fd5d4` — 5.0's own placement), so a
    /// `[expr] = value` field credits **neither** `OP_NEWTABLE` size hint. The table is born with
    /// `B = C = 0`, i.e. on the shared dummy node, so the first store finds no free node, takes
    /// `luaH_newkey`'s rehash tail, and `rehash` sizes an **array part** for the dense integer
    /// keys; `luaH_next` walks the array part first, ascending by index, and the node part after.
    /// Stock 5.1 moved that `cc->nh++` out of the arm, pre-sized a node vector big enough that no
    /// rehash ever fires, and left all n keys in the hash — where `next` is slot order.
    ///
    /// This was measured by executing `WoW.exe`'s own bytes (`lua_open` → `luaL_loadbuffer` →
    /// `lua_pcall` → `lua_next`), not merely derived from the disassembly — including the exact
    /// `Bagnon_Core.lua` below; ascending holds for n = 1..24 and regardless of the order the
    /// fields are written in.
    ///
    /// It matters because every saved-variables file we write is that constructor (decision 1128's
    /// grammar, and the reference's own writer's — it never emits a bare positional entry), so
    /// before this hunk every list an addon saved came back in hash order. Bagnon 6.10.22 walks
    /// its saved bag order with `pairs` and drew the keyring first, ahead of the backpack.
    #[test]
    fn a_bracketed_key_constructor_is_an_array_and_walks_in_index_order() {
        let s = UiScript::new().unwrap();
        // Bagnon's own table, exactly as its saved-variables file spells it.
        assert_eq!(
            s.eval::<String>(
                "local t = { [1] = 0, [2] = 1, [3] = 2, [4] = 3, [5] = 4, [6] = -2 }
                 local out = '' for _, v in pairs(t) do out = out .. v .. ',' end return out"
            )
            .unwrap(),
            "0,1,2,3,4,-2,",
            "the keyring (-2) is LAST — this is the director's Bagnon grid"
        );
        // …and the order the fields are written in does not change it.
        assert_eq!(
            s.eval::<String>(
                "local t = { [3] = 'c', [1] = 'a', [2] = 'b' }
                 local out = '' for k in pairs(t) do out = out .. k .. ',' end return out"
            )
            .unwrap(),
            "1,2,3,"
        );
        // A key the array part cannot hold comes after the run, in hash order.
        assert_eq!(
            s.eval::<String>(
                "local t = { ['a'] = 1, [1] = 10, [2] = 20, [3] = 30 }
                 local out = '' for k in pairs(t) do out = out .. tostring(k) .. ',' end return out"
            )
            .unwrap(),
            "1,2,3,a,"
        );
        // The positional spelling was always an array and still is — the control that keeps the
        // first assertion from reading as vacuous.
        assert_eq!(
            s.eval::<String>(
                "local t = { 0, 1, 2, 3, 4, -2 }
                 local out = '' for _, v in pairs(t) do out = out .. v .. ',' end return out"
            )
            .unwrap(),
            "0,1,2,3,4,-2,"
        );
    }
}

#[cfg(test)]
mod error_quoting_tests {
    use crate::script::UiScript;

    /// **Every error message quotes a program element the way 5.0 does — `` `x' ``, not `'x'``
    /// (decision 2122).**
    ///
    /// 5.1 introduced `LUA_QL` and made it two apostrophes; the fork's `luaconf.h` puts 5.0's
    /// backquote back. The five formats it feeds are all readable in `WoW.exe`'s own `.rdata`, and
    /// the assertions below are those strings:
    ///
    /// ```text
    /// bad argument #%d to `%s' (%s)          lauxlib.c luaL_argerror
    /// calling `%s' on bad self (%s)          lauxlib.c luaL_argerror's method arm
    /// attempt to %s %s `%s' (a %s value)     ldebug.c  luaG_typeerror
    /// %s:%d: %s near `%s'                    llex.c    luaX_lexerror (the client's own 0x87217c)
    ///  in function `%s'                      ldblib.c  the line debugstack renders
    /// ```
    ///
    /// **The traceback line is the one this does NOT reach**, and it is the one with teeth.
    /// `debugstack` delegates to mlua's own traceback, which is built in mlua's C shim rather than
    /// in this fork, so `LUA_QL` does not reach it: it still renders `in function 'X'`. That
    /// matters because AceLibrary — shipped inside ~80 corpus addons — reads its own caller back
    /// out of `debugstack()` with `string.find(debugstack(), "`argCheck'.-([`<].-['>])")`, whose
    /// two patterns both require the backquote; the nil it gets is then handed to a `%s`, so an
    /// argument-check *diagnostic* raises inside the error path. Closing that means rendering the
    /// traceback ourselves in the reference's own shape (`debugstack 0x703760`) rather than
    /// copied from stock 5.0's `ldblib.c`.
    #[test]
    fn errors_quote_program_elements_the_way_lua_5_0_does() {
        let s = UiScript::new().unwrap();
        let err = |lua: &str| {
            s.eval::<String>(&format!(
                "local ok, e = pcall(function() {lua} end) return tostring(e)"
            ))
            .unwrap()
        };

        // luaG_typeerror.
        let e = err("local t = nil return t.x");
        assert!(
            e.contains("attempt to index local `t' (a nil value)"),
            "typeerror keeps 5.1's quoting: {e}"
        );
        // luaL_argerror, through a library function that raises one.
        let e = err(r#"return string.rep(nil, 2)"#);
        assert!(
            e.contains("bad argument #1 to `rep'"),
            "argerror keeps 5.1's quoting: {e}"
        );
        // luaX_lexerror — the client's own `0x87217c` format.
        let e = s
            .eval::<String>(r#"local f, e = loadstring("return 1 +") return tostring(e)"#)
            .unwrap();
        assert!(
            e.contains("near `<eof>'"),
            "the lexer keeps 5.1's quoting: {e}"
        );
        // And no message anywhere OPENS a quote with an apostrophe — 5.1's spelling puts one
        // where 5.0 puts the backquote, so the tell is a `'` right after a space or a paren.
        // (The closing quote is an apostrophe in both dialects, which is why this looks for the
        // opening one and not for the character.)
        for e in [
            err("local t = nil return t.x"),
            err("return nosuchfn()"),
            err("return string.rep(nil, 2)"),
        ] {
            assert!(
                !e.contains(" '") && !e.contains("('"),
                "a 5.1-quoted element survives in: {e}"
            );
        }
    }

    /// **`select` is not a 1.12 global** (decision 2171) — and 5.0's own answer to the same
    /// question still is.
    ///
    /// The pair matters more than the removal. `select('#', …)` was how 177 of our own tests asked
    /// a binding's arity, and the property those gates rest on is that **zero returns and one
    /// `nil` are different answers** (`binding_abi` §2) — a distinction no `Option<T>` return type
    /// can hold. 5.0 answers it with the implicit vararg table's `n`, which this VM hands every
    /// vararg function since 2101 deleted the parser arm that cleared `VARARG_NEEDSARG`; the host
    /// answers it with [`UiScript::arity`]. If either ever stops telling those apart, the arity
    /// gates start silently passing a binding that returns nothing.
    ///
    /// The removal's own reach is in the corpus rather than here: `pfUI/libs/libpredict.lua:141`
    /// gates its **TBC** HealComm parser on `select and UnitCastingInfo`, and pfUI supplies the
    /// second name itself (`libs/libcast.lua:86`, loaded 3rd against libpredict's 10th), so
    /// publishing `select` was the whole reason that branch ran. `BuffCheck2.lua:1157` is
    /// `select = select or function(idx, ...)` — a 1.12 addon's own polyfill that ours suppressed.
    #[test]
    fn select_is_not_a_1_12_global_and_arg_n_answers_instead() {
        let s = UiScript::new().unwrap();
        assert!(
            s.eval::<bool>("return select == nil").unwrap(),
            "`select` is 5.1's base library; `reference/1.12-globals.tsv` has no row for it"
        );

        // 5.0's spelling, in the shape an addon writes it.
        assert_eq!(
            s.eval::<Vec<i64>>(
                "local function n(...) return arg.n end \
                 local function two_with_a_nil() return 1, nil end \
                 local function nothing() end \
                 return { n(), n(nil), n(two_with_a_nil()), n(nothing()), n(1, 2, 3) }",
            )
            .unwrap(),
            vec![0, 1, 2, 0, 3],
            "arg.n must count trailing nils AND tell zero returns from one nil"
        );

        // And the host's, which is what our own tests ask now.
        assert_eq!(s.arity("nil").unwrap(), 1, "one nil is one value");
        assert_eq!(s.arity("ShowNameplates()").unwrap(), 0, "and zero is zero");
    }

    /// **1.12 installs no metatable on the string type**, so method-call syntax on a string raises
    /// there and quietly worked here (decision 2171).
    ///
    /// The claim is stronger than "nobody installs one": the reference's `lua_setmetatable
    /// 0x6f4020` accepts exactly two type tags — `LUA_TTABLE` and `LUA_TUSERDATA`, sharing one
    /// `mov [edx+8], esi` because 5.0 puts `metatable` at offset 8 of both structs — and returns 0
    /// without writing for every other tag. 5.0 has no `G(L)->mt[]` array for a string to have a
    /// slot in, so the type *cannot* carry one; `luaopen_string 0x7fd810`'s four instructions
    /// are the same fact from the other end.
    ///
    /// The **wording** is asserted, not just the raise: it is what a player sees in a script
    /// error, and 5.0's quoting convention is already load-bearing one file over — `sandbox`'s
    /// `debugstack` emits 5.0 frames because `AceLibrary.lua:139`'s `argCheck` matches
    /// ``"([`<].-['>])"`` and cannot see 5.4's `'name'` at all. Both strings are byte-verified
    /// and executed on the reference: `.data 0x871c10` ``attempt to %s a %s value`` for the
    /// anonymous form, `.data 0x871c2c` ``attempt to %s %s `%s' (a %s value)`` for the named one —
    /// **backtick open, apostrophe close**.
    #[test]
    fn the_string_type_has_no_metatable() {
        let s = UiScript::new().unwrap();
        assert!(
            s.eval::<bool>(r#"return getmetatable("") == nil"#).unwrap(),
            "5.1's luaopen_string ends in a createmetatable 5.0's does not have"
        );

        let literal: String = s
            .eval(
                r#"local ok, err = pcall(function() return ("abc"):upper() end) return tostring(err)"#,
            )
            .unwrap();
        assert!(
            literal.contains("attempt to index a string value"),
            "5.0's own luaG_typeerror wording: {literal:?}"
        );
        let named: String = s
            .eval(
                "local ok, err = pcall(function() local v = 'abc' return v:sub(1, 2) end) \
                 return tostring(err)",
            )
            .unwrap();
        assert!(
            named.contains("attempt to index local `v' (a string value)"),
            "the named form, with 5.0's backtick-apostrophe quoting: {named:?}"
        );

        // The other direction, and the one that makes this a regression if it ever fails:
        // removing the metatable removes a *dispatch*, never a function. The 12-member string
        // library and the bare aliases the reference publishes over it are untouched.
        s.run(
            r#"assert(string.upper("ab") == "AB")
               assert(string.sub("hello", 2, 3) == "el")
               assert(string.find("hello", "ll") == 3)
               assert(strupper("ab") == "AB" and strsub("hello", 2, 3) == "el")
               assert(format("%2$s %1$s", "a", "b") == "b a")"#,
        )
        .unwrap();
    }
}
