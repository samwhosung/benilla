//! The script host against the C Lua 5.1 build it replaced: every case runs through `UiScript`,
//! and its answer must equal the one that build gave, recorded in `vm_parity.golden` per platform
//! where the C library decides it (`PLATFORM_CASES`), with mlua's `runtime error: ` and
//! `stack traceback:` wrapper taken out. A new case's answer comes from the reference, written in
//! by hand, and so does the answer of a case where benilla-lua follows 1.12.1's Lua 5.0 against
//! 5.1: a comment at the case names the client's address. One such rule spans many cases: a
//! function the calling instruction does not name takes the name of a global holding it
//! (`travglobals`, `0x6fbe11`), so `bad argument #1 to `strfind'' where 5.1 printed `?`.
//!
//! The cases that list every global answer with each binding the script host has, not with the
//! VM, so they live in `vm_parity_surface.golden`, which `BENILLA_BLESS=1` rewrites when a
//! binding changes; the diff is the review.

use benilla_ui::script::UiScript;

/// Serializers both VMs run: `S(...)` writes values, `E(f, ...)` writes a protected call. A number
/// prints exactly, so a one-ulp difference shows.
const PRELUDE: &str = r#"
local function S1(v)
  local t = type(v)
  if t == "string" then return string.format("%q", v)
  elseif t == "number" then
    -- `tostring`'s `%.14g` hides a last-place difference: a value it does not carry exactly
    -- prints all 17 digits.
    local s = tostring(v)
    if v == v and tonumber(s) ~= v then s = string.format("%.17g", v) end
    return s
  elseif t == "boolean" or t == "nil" then return tostring(v)
  else return t end
end
local function S(...)
  local o = {}
  for i = 1, arg.n do o[i] = S1(arg[i]) end
  return table.concat(o, ",", 1, arg.n)
end
local function pack(...) return arg end
local function E(f, ...)
  local r = pack(pcall(f, unpack(arg)))
  if r[1] then
    local o = {}
    for i = 2, r.n do o[i - 1] = S1(r[i]) end
    return "ok " .. table.concat(o, ",", 1, r.n - 1)
  end
  local t = type(r[2])
  -- A table or function error value prints its type: its address differs every run.
  return "err " .. ((t == "table" or t == "function") and t or tostring(r[2]))
end
"#;

const CASES: &[(&str, &str)] = &[
    ("arith", "return S(1+2, 10/4, 2^10, -3, 7-2*3, 3-(-2), 2^-1, -2^2)"),
    ("numfmt", "return S(1e15, 1e100, 0.1, 1/3, 123456789012345, -0.5, 100, 2^53)"),
    ("coerce", r#"return S("10"+5, "3"*"4", 10 .. 20, 1.5 .. "", " 7 " + 0, -"2")"#),
    ("arith_sides", r#"local mt = {}
        for _, e in {"__add", "__sub", "__mul", "__div"} do
          -- A fresh local per pass: 5.0 shares the loop variable itself.
          local name = e
          mt[e] = function(a, b) return name .. ":" .. type(a) .. "," .. type(b) end
        end
        local t = setmetatable({}, mt)
        return S(1 + t, t + 1, 2 - t, t - 2, 3 * t, t * 3, 4 / t, t / 4, t + t)"#),
    ("arith_const_errors", r#"return S(E(function() local s = "x" return 2 * s end),
                 E(function() local x return 1 + x end),
                 E(function() local s = "x" return s - 1 end), E(function() return 1 / {} end),
                 E(function() local t = {} return 2 - t.y end))"#),
    ("arith_regs", r#"local a, b = 7, "3" return S(a + b, a - b, a * b, a / b, b - a)"#),
    ("numeric_for", r#"local o = {}
        for i = 1, 3 do table.insert(o, i) end
        for i = 3, 1, -1 do table.insert(o, i) end
        for i = 1, 2, 0.5 do table.insert(o, i) end
        for i = 1, "2" do table.insert(o, i) end
        local st = 2 for i = 1, 5, st do table.insert(o, i) end
        for i = 5, 1 do table.insert(o, "never") end
        return table.concat(o, " ")"#),
    ("numeric_for_limit_error", r#"return E(function() for i = 1, "x" do end end)"#),
    // A numeric `for`'s variable is its counter, one for the loop: the client's `fornum`
    // (`0x6fe380`) subtracts the step from the start (`0x6fe42c`) and `OP_FORLOOP` (`0x6f9262`)
    // steps the variable.
    ("numeric_for_start_error", r#"return S(E(function() for i = "a", 2 do end end), E(function() for i = 1, 2, {} do end end))"#),
    ("coerce_hex", r#"return E(function() return "0x10" + 0 end)"#),
    ("tonumber", r#"return S(tonumber("12"), tonumber("1e2"), tonumber("x"), tonumber("ff", 16), tonumber("777", 8), tonumber("  5  "), tonumber(nil))"#),
    ("tonumber_hex", r#"return S(tonumber("0x1F"))"#),
    ("div0", "return S(1/0 > 1e308, -1/0 < 0, 0/0 ~= 0/0)"),
    ("mod", "return S(math.mod(7, 3), math.mod(-7, 3), mod(7.5, 2), math.floor(-3.5), math.ceil(-3.5))"),
    ("cmp", r#"return S(1 < 2, "a" < "b", "abc" < "abd", "Z" < "a", "" < "a", 2 <= 2, 3 >= 4, "10" < "9")"#),
    ("cmp_err", r#"return E(function() return 1 < "2" end)"#),
    ("cmp_err2", r#"return E(function() return {} < {} end)"#),
    ("eq", r#"return S(1 == 1.0, "1" == 1, nil == false, {} == {}, "a" == "a")"#),
    ("concat", r#"return S("a" .. "b" .. 1 .. 2.5, "x" .. "")"#),
    ("concat_err", r#"return E(function() local t = {} return "a" .. t end)"#),
    ("sub", r#"local s = "hello world" return S(string.sub(s, 1, 5), strsub(s, -5), strsub(s, 3), strsub(s, 0), strsub(s, 5, 2), strsub(s, -100, 2), strsub(s, 2, 100))"#),
    ("byte_char", r#"return S(string.byte("A"), strbyte("abc", 2), strbyte("abc", -1), strbyte("abc", 10), string.char(72, 105))"#),
    ("case", r#"return S(string.upper("abcXYZ!"), strlower("HeLLo"), string.len("\0ab"), string.rep("ab", 3), string.rep("x", 0))"#),
    ("find", r#"return S(string.find("hello world", "wor"), strfind("hello", "l"), strfind("hello", "xyz"), strfind("a.b", ".", 1, true), strfind("hello", "l", 4), strfind("hello", "", 10))"#),
    ("find_pat", r#"return S(strfind("key = value", "(%w+)%s*=%s*(%w+)"), strfind("  x", "^%s*"), strfind("abc", "b()"), strfind("[test]", "%[(.-)%]"))"#),
    ("find_anchor", r#"return S(strfind("hello", "^h"), strfind("hello", "^e"), strfind("hello", "o$"), strfind("a$b", "$b"))"#),
    ("classes", r#"return S(strfind("abc123", "%d+"), strfind("a b", "%s"), strfind("x_y", "%p"), strfind("AbC", "%u%l%u"), strfind("\t", "%c"), strfind("ff", "%x+"), strfind("a1", "%W"))"#),
    ("sets", r#"return S(strfind("hello", "[aeiou]"), strfind("hello", "[^hel]"), strfind("a-b", "[%-]"), strfind("x]", "[]]"), strfind("5", "[0-9]"))"#),
    ("quantifiers", r#"return S(strfind("aaab", "a-b"), strfind("aaab", "a*"), strfind("b", "a?b"), strfind("<a><b>", "<(.-)>"), strfind("<a><b>", "<(.*)>"))"#),
    ("balance", r#"return S(strfind("f(a(b)c)d", "%b()"), strfind("THE (quick) fox", "%f[%a]%a+", 5))"#),
    ("backref", r#"return S(strfind('say "hi" now', "([\"'])(.-)%1"))"#),
    ("gsub", r#"return S(string.gsub("hello world", "o", "0"), E(gsub, "abc", "%w", "%0%0"), gsub("hello", "l+", function(s) return "[" .. s .. "]" end), gsub("abc", "", "-"), gsub("a b c", " ", "", 1))"#),
    ("gsub_basic", r#"return S(string.gsub("hello world", "o", "0"), gsub("hello", "l+", function(s) return "[" .. s .. "]" end), gsub("abc", "", "-"), gsub("a b c", " ", "", 1), gsub("$1.50", "%$", "USD"))"#),
    ("gsub_caps", r#"return S(gsub("John Smith", "(%w+) (%w+)", "%2, %1"), gsub("x=1, y=2", "(%w+)=(%w+)", "%2=%1"), gsub("abc", "(b)", "%%"))"#),
    ("gsub_nil_fn", r#"return S(gsub("a1b2", "%d", function(d) if d == "1" then return "one" end end))"#),
    ("gsub_anchor", r#"return S(gsub("aaa", "^a", "b"), gsub("hello", "^", ">"))"#),
    ("gfind", r#"local o = {} for w in string.gfind("one two  three", "%a+") do table.insert(o, w) end for k, v in string.gfind("a=1,b=2", "(%w+)=(%w+)") do table.insert(o, k .. v) end return table.concat(o, "|")"#),
    ("gfind_empty", r#"local o = {} for w in string.gfind("abc", "x*") do table.insert(o, "[" .. w .. "]") end return table.concat(o)"#),
    ("pat_errors", r#"return E(function() return strfind("a", "[a") end) .. E(function() return strfind("a", "%") end) .. E(function() return strfind("a", "(a") end) .. E(function() return gsub("a", "a", "%2") end)"#),
    ("format", r#"return S(string.format("%d %5d %-5d| %05d %+d", 42, 42, 42, 42, 42), format("%x %X %o %#x", 255, 255, 8, 255), format("%s %10s %-4s| %.2s", "hi", "right", "l", "abc"))"#),
    ("format_float", r#"return S(format("%f %.2f %10.3f %e %g %g %g", 3.14159, 2.5, 1/3, 12345.678, 0.0001, 1e20, 123456789))"#),
    ("format_q", r#"return format("%q", "a\"b\\c\nd\0e")"#),
    ("format_c", r#"return S(format("%c%c%c", 76, 117, 97), format("%5.1f%%", 99.5))"#),
    ("format_err", r#"return E(function() return format("%d", "x") end) .. E(function() return format("%y", 1) end)"#),
    ("ctor", "local t = {1, 2, 3, x = 4, [10] = 5; 6} return S(t[1], t[2], t[3], t.x, t[10], t[4], getn(t))"),
    ("ctor_semicolon", "local t = {a = 1;; b = 2} return S(t.a, t.b)"),
    ("getn", "local t = {1, 2, nil, 4} local u = {n = 7} return S(getn(t) >= 2, getn(u), table.getn({}), getn({1, 2, 3}))"),
    ("setn", "local t = {} table.setn(t, 5) local u = {n = 1} table.setn(u, 3) return S(getn(t), u.n, t.n)"),
    ("insert_remove", "local t = {} tinsert(t, 'a') tinsert(t, 'c') tinsert(t, 2, 'b') local r = tremove(t, 1) return S(r, t[1], t[2], t[3], getn(t), tremove({}))"),
    ("concat_tbl", "return S(table.concat({1, 2, 3}, ', '), table.concat({'a', 'b'}), table.concat({1, 2, 3}, '-', 2, 3))"),
    // The client's `str_concat` fails argument 1's check (`0x7fb8d1`, text `0x8821f0`) through
    // `luaL_error`, at the caller's line.
    ("concat_err_2", "return E(function() return table.concat({1, {}, 3}) end)"),
    ("sort", "local t = {5, 2, 8, 1, 9, 3} table.sort(t) local u = {'b', 'a', 'c'} sort(u, function(a, b) return a > b end) return table.concat(t, ',') .. '|' .. table.concat(u, ',')"),
    ("sort_many", "local t = {} for i = 1, 100 do t[i] = math.mod(i * 37, 101) end table.sort(t) local ok = true for i = 2, 100 do if t[i-1] > t[i] then ok = false end end return S(ok, t[1], t[100])"),
    ("foreach", "local o = {} foreachi({'a', 'b', 'c'}, function(i, v) table.insert(o, i .. v) end) local r = foreachi({1, 2, 3}, function(i, v) if v == 2 then return 'stop' end end) return table.concat(o, ',') .. S(r)"),
    ("unpack", "return S(unpack({1, 2, 3})) .. '|' .. S(unpack({n = 2, 'a', 'b', 'c'}))"),
    ("next", "local t = {} t.x = 1 local k, v = next(t) return S(k, v, next({}), next(t, 'x'))"),
    // `luaH_next` raises the client's text (`0x6fa467`, `0x871aac`): "for", not 5.1's "to".
    ("next_err", "return E(function() return next({}, 'nope') end)"),
    ("ipairs", "local o = {} for i, v in ipairs({'a', 'b', nil, 'd'}) do table.insert(o, i .. v) end return table.concat(o, ',')"),
    ("pairs_array", "local o = {} for k, v in pairs({10, 20, 30}) do table.insert(o, k .. '=' .. v) end return table.concat(o, ',')"),
    ("generic_for_table", "local s = 0 for k, v in {1, 2, 3} do s = s + v end return S(s)"),
    ("nil_key", "return E(function() local t = {} t[nil] = 1 end)"),
    ("rawops", "local t = setmetatable({}, {__index = function() return 'meta' end, __newindex = function() end}) t.x = 1 rawset(t, 'y', 2) return S(t.x, rawget(t, 'x'), t.y, rawget(t, 'y'), rawequal(t, t))"),
    ("index_chain", "local base = {a = 1} local mid = setmetatable({b = 2}, {__index = base}) local t = setmetatable({}, {__index = mid}) return S(t.a, t.b, t.c)"),
    ("newindex", "local log = {} local t = setmetatable({}, {__newindex = function(t, k, v) rawset(t, k, v * 2) end}) t.x = 5 t.x = 7 return S(t.x)"),
    ("arith_mt", "local mt = {__add = function(a, b) return 'add' end, __unm = function() return 'neg' end, __concat = function(a, b) return 'cat' end} local t = setmetatable({}, mt) return S(t + 1, 1 + t, -t, t .. 'x', 'x' .. t)"),
    ("eq_mt", "local mt = {__eq = function() return true end} local a, b = setmetatable({}, mt), setmetatable({}, mt) local c = setmetatable({}, {__eq = function() return true end}) return S(a == b, a == c, a ~= b)"),
    ("lt_mt", "local mt = {__lt = function(a, b) return a.v < b.v end} local a, b = setmetatable({v = 1}, mt), setmetatable({v = 2}, mt) return S(a < b, b < a, a > b, a <= b)"),
    ("call_mt", "local t = setmetatable({}, {__call = function(self, a, b) return a + b end}) return S(t(2, 3))"),
    ("tostring_mt", "local t = setmetatable({}, {__tostring = function() return 'custom' end}) return tostring(t)"),
    ("protected_mt", "local t = setmetatable({}, {__metatable = 'locked'}) return S(getmetatable(t)) .. E(function() setmetatable(t, {}) end)"),
    // `^` on numbers calls the global `__pow`, the client's `Arith` (`0x6f9b52`).
    ("pow_global", "local old = __pow __pow = function(a, b) return 'p' end local r = 2 ^ 3 __pow = old return S(r, 2 ^ 3)"),
    ("varargs", "local function f(...) return arg.n, arg[1], arg[2] end return S(f()) .. '|' .. S(f(1, nil, 3)) .. '|' .. S(f('a'))"),
    ("varargs_named", "local function f(a, ...) return a, arg.n end return S(f(1, 2, 3))"),
    ("multret", "local function f() return 1, 2, 3 end local t = {f(), f()} return S(getn(t), (f()), select and 1 or 0)"),
    ("multret_adjust", "local function f() return 1, 2 end local a, b, c = f() local d, e = (f()) return S(a, b, c, d, e)"),
    ("closures", "local function counter() local n = 0 return function() n = n + 1 return n end end local c1, c2 = counter(), counter() c1() c1() return S(c1(), c2())"),
    ("shared_upvalue", "local a, b do local x = 0 a = function() x = x + 1 return x end b = function() return x end end a() a() return S(b())"),
    ("recursion", "local function fib(n) if n < 2 then return n end return fib(n-1) + fib(n-2) end return S(fib(20))"),
    ("tailcall", "local function loop(n) if n == 0 then return 'done' end return loop(n - 1) end return S(loop(10000))"),
    ("method", "local obj = {v = 5} function obj:get(x) return self.v + x end return S(obj:get(1), obj.get(obj, 2))"),
    ("nested_fn_names", "local t = {a = {}} function t.a.f() return 1 end function t.a:g() return self == t.a end return S(t.a.f(), t.a:g())"),
    ("numfor", "local o = {} for i = 1, 3 do table.insert(o, i) end for i = 3, 1, -1 do table.insert(o, i) end for i = 1, 0 do table.insert(o, 'never') end for i = 1, 2, 0.5 do table.insert(o, i) end return table.concat(o, ',')"),
    // A numeric `for`'s variable is its counter, one for the loop: the client's `fornum`
    // (`0x6fe380`) subtracts the step from the start (`0x6fe42c`) and `OP_FORLOOP` (`0x6f9262`)
    // steps the variable.
    ("numfor_closures", "local fs = {} for i = 1, 3 do fs[i] = function() return i end end return S(fs[1](), fs[2](), fs[3]())"),
    // A generic `for`'s variables are one for the loop (`forbody`, `0x6fe480`): its closures
    // see them as the loop leaves them, nil.
    ("genfor_closures", "local fs = {} for k, v in ipairs({'a', 'b'}) do fs[k] = function() return v end end return S(fs[1](), fs[2]())"),
    ("body_local_closures", "local fs = {} for i = 1, 3 do local j = i fs[i] = function() return j end end return S(fs[1](), fs[2](), fs[3]())"),
    // A numeric `for`'s variable is its counter, one for the loop: the client's `fornum`
    // (`0x6fe380`) subtracts the step from the start (`0x6fe42c`) and `OP_FORLOOP` (`0x6f9262`)
    // steps the variable.
    ("numfor_assign", "local o = {} for i = 1, 5 do table.insert(o, i) i = i + 1 end return table.concat(o, ',')"),
    ("genfor_control", "local o = {} for k, v in pairs({a = 1}) do table.insert(o, k) end return table.concat(o)"),
    // A numeric `for`'s variable is its counter, one for the loop: the client's `fornum`
    // (`0x6fe380`) subtracts the step from the start (`0x6fe42c`) and `OP_FORLOOP` (`0x6f9262`)
    // steps the variable.
    ("for_errors", "return E(function() for i = 'x', 2 do end end) .. E(function() for i = 1, 'y' do end end)"),
    ("while_repeat", "local i, o = 0, {} while i < 3 do i = i + 1 table.insert(o, i) end repeat i = i - 1 table.insert(o, i) until i == 0 return table.concat(o, ',')"),
    ("repeat_scope", "local x = 'outer' local n = 0 repeat local x = 'inner' n = n + 1 until x == 'outer' or n > 2 return S(n)"),
    ("break", "local o = {} for i = 1, 10 do if i > 3 then break end table.insert(o, i) end while true do break end return table.concat(o, ',')"),
    ("break_closure", "local fs = {} local i = 0 while true do i = i + 1 local j = i fs[i] = function() return j end if i == 2 then break end end return S(fs[1](), fs[2]())"),
    ("err_index_global", "return E(function() return undefined_thing.x end)"),
    ("err_index_local", "return E(function() local t return t.x end)"),
    ("err_index_field", "return E(function() local t = {} return t.a.b end)"),
    // `getobjname` names no upvalue (`0x6fc5e0`).
    ("err_index_upvalue", "local up return E(function() return up.x end)"),
    ("err_call", "return E(function() undefined_fn() end) .. E(function() local t = {} t.m() end) .. E(function() local t = {} t:m() end)"),
    ("err_arith", "return E(function() local x return x + 1 end) .. E(function() return {} * 2 end)"),
    ("error_fn", "return E(function() error('boom') end) .. E(function() error('noloc', 0) end) .. string.sub(E(function() error({}) end), 1, 11) .. E(function() error(42) end)"),
    ("error_level2", "local function f() error('deep', 2) end return E(function() f() end)"),
    // `luaB_assert` raises through `luaL_error` (`0x7031d6`), at the caller's line.
    ("assert", "return E(function() assert(false) end) .. E(function() assert(nil, 'msg') end) .. S(assert(1, 2, 3))"),
    ("bad_arg", "return E(function() string.sub() end) .. E(function() strfind(nil, 'x') end) .. E(function() setmetatable(1, {}) end) .. E(function() math.floor('x') end)"),
    ("bad_self", "local t = {f = string.len} return E(function() return t:f() end)"),
    ("xpcall", "local ok, msg = xpcall(function() error('x') end, function(e) return 'handled: ' .. e end) return S(ok, msg) .. '|' .. S(xpcall(function() return 1, 2 end, print))"),
    ("pcall_args", "return S(pcall(function(a, b) return a + b end, 1, 2))"),
    ("nested_pcall", "return E(function() local ok, e = pcall(error, 'inner') error('outer:' .. e, 0) end)"),
    ("fenv", "local env = {x = 'env'} local function f() return x end setfenv(f, env) return S(f(), getfenv(f) == env, getfenv(0) == getfenv(), getfenv(1) == getfenv())"),
    ("fenv_level", "x = 'global' local function f() setfenv(1, {x = 'set'}) return x end return S(f())"),
    ("loadstring", "local f = loadstring('return 1 + 1') local g, e = loadstring('return +') return S(f(), g, e)"),
    ("loadstring_name", "local f = loadstring('error(\"x\")', '=named') return E(f)"),
    // 1.12's base array (`0x811e28`) has no `_G`: an addon reaches the globals by `getfenv(0)`.
    ("globals", "return S(type(_G), type(print), type(select), type(coroutine), type(string.gmatch), type(math.fmod), type(gcinfo), type(bit), type(getn), type(strfind), type(sin))"),
    ("type", "return S(type(nil), type(1), type('s'), type({}), type(print), type(type), type(true))"),
    ("tostring", "return S(tostring(nil), tostring(true), tostring(12), tostring('x'), string.sub(tostring({}), 1, 7), string.sub(tostring(print), 1, 10))"),
    // The client's `bit` answers unsigned but for `arshift` and `mod` (`0x7faa6d`, `0x7fad06`).
    ("bit", "return S(bit.band(12, 10), bit.bor(12, 10), bit.bxor(12, 10), bit.bnot(0), bit.lshift(1, 4), bit.rshift(256, 4), bit.arshift(-16, 2), bit.lshift(1, 32), bit.mod(7, 3))"),
    ("math", "return S(math.abs(-3), math.max(1, 5, 3), math.min(4, 2), math.sqrt(16), math.floor(2.7), math.pi > 3.14, math.frexp(8), math.ldexp(0.5, 4), math.deg(math.pi))"),
    ("random", "math.randomseed(42) local a = math.random(100) math.randomseed(42) return S(a == math.random(100), math.random() < 1, math.random(5, 5))"),
    // ── What the current VM's 5.1 accepts, which 5.0 does not ──
    ("hex_literals", "return S(0x10, 0xff + 1, 0XA)"),
    // The client's lexer has no long-bracket levels: `[=` is the token `[` (`0x6ff793`).
    ("long_brackets", "return S([=[a]]b]=], [==[x]=]y]==], [[plain]])"),
    ("string_dump", "return S(type(string.dump), E(function() return type(string.dump(function() end)) end))"),
    // A numeric `for`'s variable is its counter, one for the loop: the client's `fornum`
    // (`0x6fe380`) subtracts the step from the start (`0x6fe42c`) and `OP_FORLOOP` (`0x6f9262`)
    // steps the variable.
    ("numeric_for_fresh_var", "local fs = {} for i = 1, 3 do fs[i] = function() return i end end return S(fs[1](), fs[2](), fs[3]())"),
    // A generic `for`'s variables are one for the loop (`forbody`, `0x6fe480`): its closures
    // see them as the loop leaves them, nil.
    ("generic_for_fresh_var", "local fs = {} for i, v in ipairs({10, 20}) do fs[i] = function() return v end end return S(fs[1](), fs[2]())"),
    // A numeric `for`'s variable is its counter, one for the loop: the client's `fornum`
    // (`0x6fe380`) subtracts the step from the start (`0x6fe42c`) and `OP_FORLOOP` (`0x6f9262`)
    // steps the variable.
    ("for_var_assigned", "local s = 0 for i = 1, 5 do s = s + i i = i + 10 end return S(s)"),
    ("until_sees_body_locals", "local n = 0 repeat local stop = n >= 2 n = n + 1 until stop or n > 5 return S(n)"),
    ("pow_metamethod", "local t = setmetatable({}, {__pow = function(a, b) return 'mm' end}) return S(E(function() return t ^ 2 end), E(function() return 2 ^ t end))"),
    // `getobjname` names no upvalue (`0x6fc5e0`).
    ("upvalue_error_name", "local u return S(E(function() return u + 1 end), E(function() return u.x end), E(function() u() end))"),
    ("vararg_expr", "return S(E(loadstring, 'return function(...) return ... end'))"),
    // ── Numbers as the C library reads and writes them ──
    ("nonfinite_print", "return S(1/0, -1/0, string.format('%f %g %e %5.1f', 1/0, -1/0, 1/0, -1/0), tostring(1/0) .. '', 1e308 * 10)"),
    ("nan_print", "local n = 0/0 return S(tostring(n) == tostring(n), string.format('%d', 3.7), string.format('%x', 255), string.format('%5.2s|', 'abc'))"),
    ("tonumber_edges", r#"return S(tonumber("inf"), tonumber("-inf"), tonumber("nan") ~= nil, tonumber("1e"), tonumber("0x"), tonumber(" 0x1A "), tonumber("0x1p4"), tonumber("1e+"), tonumber(".5"), tonumber("5."), tonumber("- 1"), tonumber("1 2"), tonumber(""), tonumber("  "), tonumber("0x10", 10), tonumber("10", 36), tonumber("z", 36))"#),
    ("tonumber_base_edges", r#"return S(tonumber("-1", 16), tonumber("0x10", 16), tonumber(" ff ", 16), tonumber("ffffffffffffffffffff", 16), tonumber("1z", 36), tonumber("8", 8), tonumber("", 16), tonumber("-ff", 16), tonumber("+7", 8))"#),
    ("numeral_lexing", "return S(1e10, .5, 5., 3e-2, 1E2, 2^53 + 1, 1.5e+3, .5E-1, 2., 5.e2)"),
    // Input past the chunk's end is the client's bare `<eof> expected` (`0x6fca1a`). A NUL after
    // a `.` is no `.` (`0x6ff98a`, `0x6ff9b3`).
    ("numeral_reader", r#"local o = {}
        for _, src in ipairs({"return 0x10", "return 0xA0", "a = 0x10()", "a=1b=2 return a, b",
            "return 1..2", "return 1.2.3", "return 3e", "return 3e+", "return .5.3", "return 1d2",
            "return 1e", "return 1.e5", "return 1 .. 2", "return 1._", "return 1x", "return 0X1",
            "return 1e5e5", "return 1.5.", "return ..5", "x = 3.. 'a'", "return 1E-x",
            "return 'a' .\0 'b'", "return 'a' ..\0 'b'"}) do
          local f, e = loadstring(src, "=t")
          if f then
            local ok, a, b = pcall(f)
            table.insert(o, "ok " .. tostring(ok) .. " " .. tostring(a) .. " " .. tostring(b))
          else
            table.insert(o, e)
          end
        end
        return table.concat(o, " | ")"#),
    ("tonumber_d_exponent", r#"return S(tonumber("1d2"), tonumber("-2.5D-1"), tonumber(" 1d+1 "), tonumber("1" .. string.rep("0", 70) .. "d-70"), tonumber("1d"), tonumber("1D+"), tonumber("1f2"), tonumber("+3"), tonumber("-.5"), tonumber("1e400"), tonumber("0X1A"), tonumber("-0x2"), tonumber("infinity"), tonumber(".e1"), tonumber("5.d1"), tonumber("1d2 x"), tonumber("1d2\0x"))"#),
    ("coerce_d_and_hex", r#"return S("1d2" + 0, "2" + 1, E(function() return "0x2" + 1 end), E(function() return "nan" * 1 end), " 5.D1 " * 2, E(string.rep, "x", "0x2"), string.rep("x", "2d0"))"#),
    ("coerce_edges", r#"return S(E(function() return "0x10" * 1 end), " 2 " * " 3 ", E(function() return "1e" + 1 end), E(function() return "inf" + 0 end), 10 .. "", 1e100 .. "", -0.0 .. "", 2^63 .. "")"#),
    // ── Syntax errors and the lexer's edges, through `loadstring` ──
    // Input past the chunk's end is the client's bare `<eof> expected` (`0x6fca1a`). A control
    // byte is the lexer's `invalid control char` (`0x6ffabb`). `\n` alone ends a line
    // (`0x6ffb94`): a `\r` is a blank, and a byte in a string (`0x700326`). `[=` is the token
    // `[` (`0x6ff793`) and `--[==[` a line comment (`0x6ff6c9`).
    ("syntax_errors", r#"local o = {}
        for _, src in ipairs({"x = 1 +", "local function", "x = #t", "x = a % b",
            "return function(...) return ... end", "for x do end", "x = 1..2", "x = \"a\nb\"",
            "x = [[a", "x = 1\1", "f()\n(g)()", "break", "(a) = 1", "if x then\nend end",
            "do\nx = 1", "x = 'a\rb'", "x = 1\ry = +", "x = 1\r\ny = +", "x = 1\n\ry = +",
            "x = [=x", "x = [==[a]=]", "x = 0x", "x = 1e", "x = 3x", "--[==[ c\n]==] y = +",
            "--[[ a [[ b ]] c ]] y = +", "x = [[a\r\nb\rc\n\rd]] y = +"}) do
          local f, e = loadstring(src, "=t")
          table.insert(o, (f and "ok" or e))
        end
        return table.concat(o, " | ")"#),
    // `read_long_string` (`0x700010`) reads `=` as text.
    ("long_strings", r#"return S([[a [[b]] c]], [[a [[ ]] ]=] b]], [[
first newline skipped]], [[=[
also]=]], [[a
b]])"#),
    // ── Table layout, which `pairs` order shows ──
    // The orders of `pairs_constructors`, `pairs_mixed`, `fix-front_setlist_order` and
    // `fix-vm dead_keys` are the client's, run in its own `ltable.c` (`0x6fa1a0`..`0x6fad80`):
    // `n` keys sit in `2^(log2(n)+1)` nodes, the constructor's `luaO_log2(nh) + 1` (`0x6fd563`)
    // and `rehash`'s (`0x6fa9e5`), so an eight-key constructor walks otherwise than 5.1's.
    ("pairs_constructors", r#"local o = {}
        for n = 1, 20 do
          local parts = {}
          for i = 1, n do table.insert(parts, "k" .. i .. "=" .. i) end
          local t = loadstring("return {" .. table.concat(parts, ",") .. "}")()
          local ks = {} for k in pairs(t) do table.insert(ks, k) end
          table.insert(o, table.concat(ks, " "))
        end
        return table.concat(o, " | ")"#),
    // Number keys hash as the client's `hashnum` (`0x6fa260`) does, the key plus 1.0.
    ("pairs_mixed", r#"local function keys(t) local ks = {} for k, v in pairs(t) do table.insert(ks, tostring(k) .. "=" .. tostring(v)) end return table.concat(ks, " ") end
        local a = {1, 2, 3, x = 1, y = 2, [10] = 5, [1.5] = 6, [-1] = 7, z = 3}
        local b = {} for i = 1, 10 do b[i] = i end b[5] = nil b[20] = 20 b.k = 1
        local c = {} for i = 10, 1, -1 do c[i * 3] = i end
        local d = {["a"] = 1, [2] = 2, [true] = 3, b = 4}
        local e = {1, 2, nil, 4, nil, nil, 7}
        return keys(a) .. " | " .. keys(b) .. " | " .. keys(c) .. " | " .. keys(d) .. " | " .. keys(e)"#),
    ("pairs_growth", r#"local o = {}
        local t = {}
        for i = 1, 40 do t["s" .. i] = i if i == 13 or i == 27 then local ks = {} for k in pairs(t) do table.insert(ks, k) end table.insert(o, table.concat(ks, " ")) end end
        for i = 1, 40, 3 do t["s" .. i] = nil end
        for i = 41, 50 do t["s" .. i] = i end
        local ks = {} for k in pairs(t) do table.insert(ks, k) end table.insert(o, table.concat(ks, " "))
        return table.concat(o, " | ")"#),
    ("newindex_layout", r#"local log = {}
        local t = setmetatable({}, {__newindex = function(t, k, v) table.insert(log, k) end})
        for i = 1, 10 do t["k" .. i] = i end
        rawset(t, "z", 1) rawset(t, "y", 2) rawset(t, "x", 3)
        local ks = {} for k in pairs(t) do table.insert(ks, k) end
        local u = setmetatable({}, {__newindex = function(t, k, v) rawset(t, k, v * 10) end})
        u.a = 1 u.b = 2 u.a = 3
        local us = {} for k, v in pairs(u) do table.insert(us, k .. "=" .. v) end
        return table.concat(log, ",") .. " | " .. table.concat(ks, ",") .. " | " .. table.concat(us, ",")"#),
    ("metamethods", r#"local mt = {}
        mt.__concat = function(a, b) return "cat" end
        mt.__eq = function(a, b) return true end
        mt.__lt = function(a, b) return true end
        mt.__le = function(a, b) return false end
        mt.__unm = function(a) return "neg" end
        mt.__call = function(self, x) return "called " .. tostring(x) end
        mt.__index = function(t, k) return "idx " .. k end
        mt.__tostring = function() return "TOSTR" end
        local a, b = setmetatable({}, mt), setmetatable({}, mt)
        local c = setmetatable({}, {__metatable = "locked"})
        return S(a .. "x", "x" .. a, a == b, a < b, a <= b, a > b, -a, a(5), a.foo, tostring(a),
                 getmetatable(c), E(setmetatable, c, {}), rawequal(a, b), E(function() return a < 1 end))"#),
    ("le_fallback", r#"local mt = {__lt = function(a, b) return a.v < b.v end}
        local a, b = setmetatable({v = 1}, mt), setmetatable({v = 2}, mt)
        return S(E(function() return a <= b end), E(function() return b >= a end), a < b)"#),
    ("varargs_arg", r#"local function f(...) return arg.n, arg[1], arg[3] end
        return S(E(f, 1, nil, 3), E(function(a, ...) return a, arg.n end, 9))"#),
    // The client's order, run in its code: `luaL_openlib` (`0x6f4dc0`) fills each table from empty
    // in its array's order.
    ("pairs_libraries", r#"local o = {}
        for _, lib in ipairs({"string", "table", "math", "bit"}) do
          local ks = {} for k in pairs(getfenv(0)[lib]) do table.insert(ks, k) end
          table.insert(o, lib .. ": " .. table.concat(ks, " "))
        end
        return table.concat(o, " | ")"#),
    ("pairs_globals", r#"local ks, n = {}, 0
        for k in pairs(getfenv(0)) do n = n + 1 if n <= 400 then table.insert(ks, tostring(k)) end end
        return n .. ": " .. table.concat(ks, " ")"#),
    ("getn_holes", r#"local function g(t) return table.getn(t) end
        local a = {1, 2, nil, 4}
        local b = {nil, nil, 3}
        local c = {} c[1] = 1 c[2] = 2 c[4] = 4
        local d = {} for i = 1, 20 do d[i] = i end d[7] = nil d[15] = nil
        local e = {} for i = 1, 10 do e[i] = i end for i = 12, 40 do e[i] = i end
        local f = {n = 3}
        local h = {} h[1] = 1 h[3] = 3 h[2] = 2 h[100] = 100
        return S(g(a), g(b), g(c), g(d), g(e), g(f), g(h), g({}), g({nil}), g({1, nil, 3, nil, 5, 6, 7, 8}))"#),
    ("string_metatable", r#"return S(E(function() return ("abc"):upper() end), getmetatable("x") ~= nil, E(function() local s = "x" return s.len end))"#),
    // ── The whole library surface: every global and every library member, with its type ──
    ("global_surface", r#"local o = {} for k, v in pairs(getfenv(0)) do if type(k) == "string" then table.insert(o, k .. ":" .. type(v)) end end table.sort(o) return table.concat(o, " ")"#),
    ("libraries", r#"local o = {}
        for _, lib in ipairs({"string", "table", "math", "bit", "os", "coroutine", "debug", "io"}) do
          local t = getfenv(0)[lib]
          if type(t) == "table" then
            local ks = {} for k, v in pairs(t) do table.insert(ks, k .. ":" .. type(v)) end table.sort(ks)
            table.insert(o, lib .. "={" .. table.concat(ks, " ") .. "}")
          else table.insert(o, lib .. "=" .. type(t)) end
        end
        return table.concat(o, " ")"#),
    // ── fix-front: the single-pass front end, 5.1.5's `lparser.c` and `lcode.c` ──
    // 5.0's parser flushes a constructor's items every 32 (`closelistfield`, `0x6fd6d0`), and
    // `OP_SETLIST` stores each by `luaH_setnum` (`0x6f95b1`) into the array part `OP_NEWTABLE`
    // sized; a trailing call counts as one item toward that size (`0x6fd72b`).
    ("fix-front_setlist_order", r#"local function K(t) local o = {} for k in pairs(t) do table.insert(o, tostring(k)) end return table.concat(o, " ") end
        local function list(n) local o = {} for i = 1, n do o[i] = i end return table.concat(o, ",") end
        local function f() return 1, 2, 3 end
        local big = loadstring("return {" .. list(31) .. ",[1.5]=1,[-1]=2}")()
        local big60 = loadstring("return {x=1," .. list(60) .. ",[1.5]=1,y=2,[-1]=2}")()
        local big120 = loadstring("return {[0.5]=0," .. list(120) .. ",[-2]=1,a=1,b=2}")()
        return S(K({["x"]=4,["y"]=4,["z"]=4,5}), K({0,1,[true]=2,[2]=4,5}), K({0,["x"]=4,["y"]=4,5}),
          K(big), K(big60), K(big120), K({x = 1, f()}), K({f(), x = 1, y = 2, z = 3}), K({[1]=1, [2]=2, 3, f()}))"#),
    // The parser's limits are the client's (`0x6ff440`): 200 locals, 100 parameters, 32
    // upvalues, 200 syntax levels counted in each chunk (`0x6fc9d0`).
    ("fix-front_register_reuse", r#"local function L(src) local f, e = loadstring(src) if not f then return e end local ok, r = pcall(f) return tostring(r) end
        local p = {} for i = 1, 250 do p[i] = "x" end
        local q = {} for i = 1, 400 do q[i] = "a*0.25" end
        local locs = {} for i = 1, 190 do locs[i] = "l" .. i end
        return S(L("local x = 1 return " .. table.concat(p, "+")),
          L("local t = {} t.a = t return t" .. string.rep(".a", 300) .. " == t"),
          L("local a = 4 local x = " .. table.concat(q, " + ") .. " return x"),
          L("local " .. table.concat(locs, ",") .. " local t = {} t.x = t return t" .. string.rep(".x", 59) .. " == t"),
          L("local x = 1 return " .. string.rep("(x+", 150) .. "x" .. string.rep(")", 150)),
          L("local x = 1 return " .. string.rep("(x+", 245) .. "x" .. string.rep(")", 245)),
          L("return " .. string.rep("1 .. ", 190) .. "1"))"#),
    // A generic `for` has two hidden locals and its names (`forlist`, `0x6fe540`); the
    // parameters are counted before `...` makes `arg` (`code_params`, `0x6fdab0`).
    ("fix-front_local_limits", r#"local function L(src) local f, e = loadstring(src) if f then return "ok" end return e end
        local function names(p, n) local o = {} for i = 1, n do o[i] = p .. i end return table.concat(o, ",") end
        return S(L("local " .. names("a", 200)), L("local " .. names("a", 201)),
          L("\nfunction f(" .. names("p", 200) .. ") end"), L("function f(" .. names("p", 201) .. ") end"),
          L("function f(" .. names("p", 199) .. ", ...) end"), L("function f(" .. names("p", 200) .. ", ...) end"),
          L("function f(a) local " .. names("b", 199) .. " end"), L("\n\nlocal x = function(a) local " .. names("b", 200) .. " end"),
          L("for " .. names("k", 197) .. " in x do end"), L("for " .. names("k", 198) .. " in x do end"),
          L("local function g() " .. string.rep("do local a end ", 10) .. "end"))"#),
    ("fix-front_upvalue_limits", r#"local function L(src) local f, e = loadstring(src) if f then return "ok" end return e end
        local function names(n) local o = {} for i = 1, n do o[i] = "u" .. i end return table.concat(o, ",") end
        return S(L("local " .. names(32) .. "\nreturn function() return " .. names(32) .. " end"),
          L("local " .. names(33) .. "\nreturn function() return " .. names(33) .. " end"),
          L("local " .. names(33) .. " return function() return function() return " .. names(33) .. " end end"))"#),
    // Syntax levels are counted per chunk (`0x6fc9d0`); an assignment has no limit of its own
    // but the registers' (`luaK_checkstack`, `0x700a63`).
    ("fix-front_syntax_levels", r#"local function first_failing(make)
          for n = 1, 400 do
            local f, e = loadstring(make(n))
            if not f then return n .. " " .. e end
          end
          return "none"
        end
        local parens = function(n) return "return " .. string.rep("(", n) .. "1" .. string.rep(")", n) end
        local nots = function(n) return "return " .. string.rep("not ", n) .. "1" end
        local concat = function(n) return "return " .. string.rep("1 .. ", n) .. "1" end
        local blocks = function(n) return string.rep("do ", n) .. string.rep("end ", n) end
        local funcs = function(n) return string.rep("function f() ", n) .. string.rep("end ", n) end
        local tables = function(n) return "return " .. string.rep("{", n) .. string.rep("}", n) end
        local assign = function(n) local o = {} for i = 1, n do o[i] = "a" .. i end return table.concat(o, ",") .. " = 1" end
        local nested = {}
        pcall(function() nested[1] = first_failing(parens) end)
        return S(first_failing(parens), first_failing(nots), first_failing(concat), first_failing(blocks),
          first_failing(funcs), first_failing(tables), first_failing(assign), nested[1])"#),
    ("fix-front_register_limits", r#"local function first_failing(make)
          for n = 200, 260 do
            local f, e = loadstring(make(n))
            if not f then return n .. " " .. e end
          end
          return "none"
        end
        local function args(n) local o = {} for i = 1, n do o[i] = "1" end return table.concat(o, ",") end
        return S(first_failing(function(n) return "f(" .. args(n) .. ")" end),
          first_failing(function(n) return "local a, b, c = 1\nreturn f(" .. args(n) .. ")" end),
          first_failing(function(n) return "return {" .. args(n) .. "}" end),
          first_failing(function(n) return "local t = {" .. args(n) .. "}" end),
          first_failing(function(n) return "x = f(" .. args(n) .. ") $" end))"#),
    ("fix-front_code_limits", r#"local function L(src) local f, e = loadstring(src) if f then return "ok" end return e end
        local function list(n) local o = {} for i = 1, n do o[i] = i end return table.concat(o, ",") end
        local function stats(n) return "local a if a then\n" .. string.rep("a=1 ", n) .. "\nend" end
        return S(L("local t = {" .. list(262143) .. "}"), L("local t = {" .. list(262144) .. "}"),
          L(stats(131070)), L(stats(131071)), L(stats(131072)),
          L("local a while a do\n" .. string.rep("a=1 ", 131072) .. "\nend"))"#),
    ("fix-front_names_in_errors", r#"local function R(src) return E(loadstring(src, "=t")) end
        return S(R('local n; local t = {}; return (t.y and n) .. "x"'), R('local n; local t = {}; return (n and t.x)()'),
          R('local t = {} return t.a.b'), R('local t = {x = 1} return (t.y or t.z) + 1'),
          R('local t = {} local x = t.y return x.z'), R('local a, t = nil, {} return (a or t.q)()'),
          R('local t = {} return (not t.y) .. t.z'), R('local t = {} if t.a then end return t.b()'),
          R('local t = {} return t.a < 1'), R('local x x = x or nil return x.y'))"#),
    ("fix-front_negative_zero", r#"local function R(src) return S(loadstring(src)()) end
        return table.concat({R("return -0, 1/0"), R("local x = 1/0 return -0, x, 1/0"), R("local a = 0 local b = -0 return 1/b"),
          R("return -0, 5, 1/0, 0"), R("return 1/0, -0"),
          R("return 0 * -1, 1/(0 * -1), -0 == 0"), R("local x = -0 return x, 1/x"),
          R("return 2^0.5, 2 - 3, 1/3, 0/0 ~= 0/0, 1e308 * 10")}, " | ")"#),
    ("fix-front_negative_zero_main", "return S(-0, 1/0)"),
    ("fix-front_negative_zero_local", "local x = 1/0 return S(-0, x, 1/0)"),
    // A generic `for`'s generator is named `?`: the client's `getfuncname` names only what an
    // `OP_CALL` or `OP_TAILCALL` called (`0x6fc499`..`0x6fc4a1`).
    ("fix-front_for_names", r#"return S(E(function() for k, v in pairs({1}), 5 do end end), E(function() for i in string.rep do end end),
        E(function() for i in next, 5 do end end), E(function() local t = {} for k in t.f do end end))"#),
    // A NUL byte outside a string is a control byte, the lexer's `invalid control char`
    // (`0x6ffabb`) near `char(0)`.
    ("fix-front_nul_in_messages", r#"local function L(src, name) local f, e = loadstring(src, name or "=t") return e end
        return S(L('x = "a\0b\n'), L("x = \0"), L("\0"), L("x = 1 \0 y"), L('x = "a\0b" +'),
          E(function() local t = {} t["a\0b"]() end), E(function() local t = {} t:m(1) end), E(function() return getfenv(0)["g\0h"].x end))"#),
    ("fix-front_constants_and_folding", r#"local function R(src) return S(loadstring(src)()) end
        return table.concat({R("return 1 + 2 * 3 - 4 / 2, 2^10, -(-3), - - 2, 10 / 0 > 1, 3 .. 4"),
          R("local a = 5 return a + 1 * 2, 2 * a, a / 2, 1 - a, a ^ 2, -a"),
          R("local a = {} return (1 < 2), (2 <= 1), (1 == 1), not nil, not 1, nil and 1, false or nil, 1 and 2 or 3"),
          R("local t = {} t[1], t[2] = 1, 2 local a, b = t[1], t[2] a, b = b, a return a, b"),
          R("local a, b, c = (function() return 1, 2, 3 end)() return a, b, c")}, " | ")"#),
    // ── fix-libs: the library fixes, each blessed on the C build ──
    // `bit.lshift` answers unsigned (`0x7fac40`); `bit.mod(-2^31, -1)` and `bit.mod(2^31, -1)`,
    // whose dividend's low word is `INT32_MIN` too, fault in the client's `idiv` (`0x7fad9b`), and
    // their 0 here is benilla's own.
    ("fix-libs bit_mod_overflow", "return S(E(bit.mod, -2^31, -1), E(bit.mod, 2^31, -1), E(bit.mod, 7, -1), E(bit.mod, -7, 2), E(bit.mod, 0/0, -1), E(bit.mod, 1/0, -1), E(bit.lshift, 1, 31), E(bit.arshift, -2^31, 31))"),
    ("fix-libs rep_format_sizes", r#"return S(string.rep("ab", 3), string.rep("", 1000000), string.rep("x", -1), string.rep("x", 0/0), E(string.format, "%123d", 1), E(string.format, "%1.123f", 1), string.format("%99d", 1) == string.rep(" ", 98) .. "1", string.len(string.format("%.99f", 1)), string.format("%-5c|%5s|%.1q", 65, "ab", "x"), table.concat({"a", 1, "b"}, string.rep("-", 3)), E(gsub, "abc", "%w", "%0%0%0"))"#),
    ("fix-libs deep_match", r#"local n = 3000 local s, p = string.rep("a", n), string.rep("a?", n) local t = {} for w in string.gfind(s, string.rep("(a)?", 30)) do table.insert(t, w) end return S(E(string.find, s, p), E(string.gsub, s, p, "x"), table.getn(t), E(string.find, s, string.rep("a-", 500) .. "$"))"#),
    ("fix-libs deg_rad", r#"local bad = 0 for i = -720, 720 do if math.deg(math.rad(i)) ~= i then bad = bad + 1 end end
        local o = {} for _, x in {0.1, 1, 3, 0.7, 1e-300, 1e300, -2.5, 1/3} do table.insert(o, string.format("%.17g %.17g", math.deg(x), math.rad(x))) end
        return S(bad, math.deg(math.rad(3)) == 3, string.format("%.17g %.17g %.17g %.17g", asin(0.5), acos(0.3), atan(1), atan2(1, 2))) .. "|" .. table.concat(o, "|")"#),
    // MSVC's `rand` (`0x7400e5`) on every platform, scaled as `math_random` (`0x7fb310`) does:
    // the full range's width wraps to 0, so it answers its low end.
    ("fix-libs random_ranges", r#"math.randomseed(7) local o = {}
        for _ = 1, 4 do table.insert(o, math.random(-2^31, 2^31 - 1)) table.insert(o, math.random(2^31 - 2, 2^31 - 1)) table.insert(o, math.random(2^31 - 1)) table.insert(o, math.random(-5, 5)) end
        return table.concat(o, " ")"#),
    ("fix-libs ldexp", r#"local z = 0 local inf = 1 / z local f = function(x) return string.format("%.17g", x) end
        return S(f(math.ldexp(0.5, 1024)), f(math.ldexp(1, -1074)), f(math.ldexp(2^-1074, 1074)), f(math.ldexp(1, -1075)), f(math.ldexp(3, -1075)), f(math.ldexp(1, 1024)), f(math.ldexp(1, -2000)), f(math.ldexp(2^-1074, 2098)), f(math.ldexp(1.5, -1074)), f(math.ldexp(inf, -5000)), f(math.ldexp(5, 2^40)), f(ldexp(0.75, -1073)), f(math.ldexp(1, 2^31)))"#),
    ("fix-libs hex_float", r#"local f = function(x) return string.format("%.17g", x or -1) end
        return S(f(tonumber("0x1p-1074")), f(tonumber("0x1000000000000p-1070")), f(tonumber("0x1p1023")), f(tonumber("0x1p1024")), f(tonumber("0x.1p-1070")), f(tonumber("0x1p-1075")), f(tonumber("0x3p-1075")), f(tonumber("0x1.8p-1074")), f(tonumber("0x1fffffffffffffp-1126")), f(tonumber("0x123456789abcdef0123p-1100")), f(tonumber("0x1p-1080")), E(function() return f("0x1p-1074" + 0) end))"#),
    ("fix-libs str_nul", r#"return S(tonumber("1\0"), tonumber("12\0 "), tonumber("1\0x"), E(function() return "1\0" + 1 end), E(function() return "0x10\0" + 0 end), tonumber("\0"), tonumber(" 5\0abc"), tonumber("ff\0", 16), tonumber("0x1\0"), tonumber("\0 1"), tonumber("1e\0"), tonumber("1 \0 x"))"#),
    // The client's `sort` (`0x7fb920`) sorts `1..getn`: `f`'s `arg.n` of 3 takes in its nil,
    // which `lua_lessthan` cannot compare.
    ("fix-libs sort_objlen", r#"local function f(...) table.sort(arg) return arg[1], arg[2], arg[3] end
        local t = {3, 1, 2, n = 5} local ok = pcall(table.sort, t)
        local a = {3, 1, nil, 2} local oka = pcall(table.sort, a)
        local b = {5, 4, 3, 2, 1, n = 2} table.sort(b)
        local c = {} c[1] = 3 c[2] = 1 c[4] = 0 local okc = pcall(sort, c)
        local d = {} table.setn(d, 2) d[1] = 9 d[2] = 8 d[3] = 7 sort(d)
        local e = {4, 3, 2, 1} e[3] = nil local oke = pcall(table.sort, e)
        return S(E(f, 3, 1, nil), ok, t[1], t[2], t[3], oka, a[1], a[2], a[3], a[4]) .. "|" .. S(b[1], b[2], b[3], b[4], b[5], okc, c[1], c[2], c[4], d[1], d[2], d[3], oke, e[1], e[2], e[4])"#),
    // 5.0's `setfenv` (`0x702be0`) answers nothing, takes its level as optional as `getfenv` does
    // (`0x702b22`), and refuses a native as "given function" (`0x702c72`); `getfenv` answers an
    // environment's raw `__fenv` (`0x702ba0`).
    ("fix-libs fenv", r#"local function f() end
        local t = {}
        local o = {S(setfenv(f, t) == f, getfenv(f) == t)}
        table.insert(o, E(setfenv, nil, t))
        local e = {__fenv = 1}
        table.insert(o, E(function() setfenv(f, e) return getfenv(f) == e end))
        table.insert(o, E(setfenv, tostring, t))
        table.insert(o, E(setfenv, 1, t))
        table.insert(o, E(function() return getfenv(0) == getfenv(1), getfenv(tostring) == getfenv(0), getfenv() == getfenv(1) end))
        table.insert(o, E(setfenv, 100, t))
        table.insert(o, E(getfenv, 100))
        table.insert(o, E(getfenv, -1))
        table.insert(o, E(setfenv, f))
        table.insert(o, E(setfenv, f, 1))
        table.insert(o, E(getfenv, "x"))
        table.insert(o, E(getfenv, {}))
        local function g() local env = {S = S} setfenv(1, env) x = 5 return env.x end
        table.insert(o, E(g))
        local function h() return getfenv(2) == getfenv(0) end
        table.insert(o, E(h))
        table.insert(o, E(function() local G = getfenv(0) setfenv(0, G) return getfenv(0) == G end))
        return table.concat(o, "|")"#),
    ("fix-libs format_no_value", r#"return S(E(string.format, "%d"), E(string.format, "%s"), E(string.format, "%d %d", 1), E(string.format, "%q"), E(string.format, "%c"), E(string.format, "%f"), E(string.format, "%y"), E(string.format, "%"), E(string.format, "%------d"), E(string.format, "%123d"), E(format, "%d"))"#),
    ("fix-libs format_alt_g", r#"local z = 0 local m = -z
        return S(string.format("%#g", 123456), string.format("%#.0g", 1), string.format("%#G", 1e20), string.format("%#g", 0), string.format("%#.3g", 1), string.format("%#g", 1e-10), string.format("%#.0e", 1), string.format("%#.0f", 1), string.format("%#.0G", 1e20), string.format("%#10.1g|%-#8g|", 5, 2.5), string.format("%g %f %e %.3g", m, m, m, m))"#),
    ("fix-libs format_ulong", r#"local z = 0 local o = {} for _, v in {2^64, 1e300, 1 / z, -1 / z, 0 / z, 2^63, -2^63, -1, 2^65, 2^63 + 2^11, -2^64, -1e300, 2^64 - 2^11} do table.insert(o, E(string.format, "%x %u %o %X", v, v, v, v)) end return table.concat(o, "|")"#),
    // `luaH_next` raises the client's text (`0x6fa467`, `0x871aac`): "for", not 5.1's "to".
    ("fix-libs foreach_invalid_key", r#"local t = {a = 1} return S(E(foreach, t, function(k) t[k] = nil for i = 1, 20 do t["x" .. i] = i end end), E(next, {}, "nope"))"#),
    // A chunk name's first line ends at `\n` alone (`strcspn` set `0x835144`), and a `\r` ends
    // no line of the source (`0x6ffb94`).
    ("fix-libs chunkid_cr", r#"local o = {}
        for _, s in {"error('e')\r--x", "error('e')\n--x", "error('e')\r\n--x", "error('e') --" .. string.rep("y", 100), "\rerror('e')", "error('e')", "error('e')\0--x"} do
          local f, e = loadstring(s) if f then table.insert(o, E(f)) else table.insert(o, e) end
        end
        table.insert(o, tostring(({loadstring("x = = 1\r--y")})[2]))
        return table.concat(o, "|")"#),
    ("fix-libs byte_c_stack", r#"local s = string.rep("a", 9000)
        return S(E(function() return table.getn({string.byte(s, 1, -1)}) end), E(function() return table.getn({string.byte(s, 1, 7997)}) end), E(function() return table.getn({string.byte(s, 1, 7998)}) end), E(function() local n = table.getn({string.byte(s, 1, 7990)}) return n end))"#),
    // ── fix-vm: call depth, 5.1.5's `LUAI_MAXCALLS` and `LUAI_MAXCCALLS` ──
    // The depth cases overflow at 4096 call records, the client's `luaD_growCI` (`0x6f61c0`).
    ("fix-vm depth_lua", r#"local n = 0
        local function f() n = n + 1 f() end
        local o = {}
        for i = 1, 3 do n = 0 local ok, e = pcall(f) table.insert(o, tostring(ok) .. ":" .. n .. ":" .. e) end
        return table.concat(o, " ")"#),
    ("fix-vm depth_deep_ok", r#"local function f(n) if n == 0 then return 0 end return 1 + f(n - 1) end
        return S(f(4000), f(4093))"#),
    ("fix-vm depth_pcall", r#"local m = 0
        local function g() m = m + 1 local ok, e = pcall(g) if not ok then error(e, 0) end end
        local ok, e = pcall(g) return S(ok, m, e)"#),
    ("fix-vm depth_index", r#"local t = setmetatable({}, {__index = function(t, k) return t[k] end})
        return E(function() return t.x end)"#),
    ("fix-vm depth_gsub", r#"local d = 0
        local function h(s) d = d + 1 return (string.gsub("a", "a", h)) end
        local ok, e = pcall(h) return S(ok, d, e)"#),
    ("fix-vm depth_xpcall", r#"local n = 0
        local function f() n = n + 1 f() end
        local ok, e = xpcall(f, function(e) return "H:" .. e end)
        return S(ok, n, e)"#),
    ("fix-vm depth_xpcall_handler_recurses", r#"local n, hn = 0, 0
        local function f() n = n + 1 f() end
        local function h(e) hn = hn + 1 return h(e) .. "" end
        local ok, e = xpcall(f, h)
        return S(ok, n, e)"#),
    ("fix-vm depth_concat_tm", r#"local mt = {}
        mt.__concat = function(a, b) return a .. b end
        local t = setmetatable({}, mt)
        return E(function() return t .. "x" end)"#),
    ("fix-vm depth_tostring_tm", r#"local t = setmetatable({}, {})
        getmetatable(t).__tostring = function(x) return tostring(x) end
        return E(function() return tostring(t) end)"#),
    ("fix-vm depth_sort", r#"local function s() table.sort({3, 1, 2}, function(a, b) s() return a < b end) end
        return E(s)"#),
    ("fix-vm depth_mixed", r#"local t = setmetatable({}, {__index = function(t, k) local ok, e = pcall(function() return string.gsub("a", "a", function() return t[k] end) end) if not ok then error(e, 0) end return e end})
        return E(function() return t.x end)"#),
    // ── fix-vm: metamethods are called whatever they are, as 5.1's `callTMres` ──
    ("fix-vm tm_callable", r#"local h = setmetatable({}, {__call = function(self, a, b) return 42 end})
        local t = setmetatable({}, {__add = h, __concat = h, __unm = h, __sub = h, __div = h, __mul = h, __pow = h})
        return S(t + 1, 1 - t, t * 2, t / 2, t ^ 2, t .. "x", "x" .. t, -t, E(function() return t .. {} end))"#),
    ("fix-vm tm_compare_callable", r#"local h = setmetatable({}, {__call = function(self, a, b) return 1 end})
        local mt = {__eq = h, __lt = h, __le = h}
        local q, r = setmetatable({}, mt), setmetatable({}, mt)
        return S(q == r, q < r, q <= r, q > r, q >= r)"#),
    ("fix-vm tm_not_callable", r#"return S(E(function() local q = setmetatable({}, {__add = 5}) return q + 1 end),
        E(function() local q = setmetatable({}, {__concat = 5}) return q .. "x" end),
        E(function() local q = setmetatable({}, {__unm = true}) return -q end),
        E(function() local q = setmetatable({}, {__lt = 5}) return q < q end),
        E(function() local q = setmetatable({}, {__eq = 5}) return q == setmetatable({}, getmetatable(q)) end),
        E(function() local q = setmetatable({}, {__call = {}}) return q() end),
        E(function() local q = setmetatable({}, {__index = 5}) return q.x end))"#),
    ("fix-vm tm_unm_args", r#"local u = setmetatable({}, {__unm = function(a, b) return S(a == b, rawequal(a, b)) end})
        return -u"#),
    // ── perf-fix: gsub skips to a literal first byte; every shape answers as before ──
    ("perf-fix gsub_literal_first", r##"local s = "a-b--c-d.e%f(g)h"
        return S(string.gsub(s, "%-", "+"), string.gsub(s, "-", "+"), string.gsub(s, "%-+", "+"), string.gsub(s, "%-*", "+"),
          string.gsub(s, "%-?", "+"), string.gsub(s, "b%-", "B"), string.gsub(s, "%-", "+", 2), string.gsub(s, "%-", "+", 0),
          string.gsub(s, "%.", "!"), string.gsub(s, "%%", "#"), string.gsub(s, "%(", "["), string.gsub(s, "d(.)", "<%1>"),
          string.gsub(s, "x", "y"), string.gsub("", "x", "y"), string.gsub(s, "^a", "A"), string.gsub(s, "^b", "B"),
          string.gsub(s, "h$", "H"), string.gsub(s, "-c", function(m) return "[" .. m .. "]" end),
          E(string.gsub, s, "c", {c = "C"}), E(string.gsub, "aaa", "a", "%0%0"))"##),
    ("perf-fix gsub_literal_errors", r#"return S(E(function() return string.gsub("abc", "b%", "x") end), E(function() return string.gsub("xyz", "b%", "x") end),
          E(function() return string.gsub("abc", "b(", "x") end), E(function() return string.gsub("xyz", "b(", "x") end),
          E(function() return string.gsub("abc", "b%1", "x") end), E(function() return string.gsub("xyz", "b[", "x") end),
          E(function() return string.gsub("abc", "b[", "x") end), E(function() return string.gsub("abc", "b", "%2") end),
          E(function() return string.gsub("xyz", "b", "%2") end))"#),
    // A generic `for` given more values than locals: the body's first local reads the register
    // the client's `OP_TFORLOOP` called the generator at (`0x6f9344`), its first result.
    ("r2-front_forlist_extra_values", r#"local function R(src) return E(loadstring(src, "=t")) end
        return S(R("for k in function() end, nil, nil, 42 do end return 7"),
          R("local n = 0 for k in function(s, c) if c ~= 3 then return (c or 0) + 1 end end, nil, nil, 9, 8 do do local y = k n = n + y end end return n"),
          R("local n = 0 for k, v in function(s, c) if c ~= 2 then return (c or 0) + 1, 10 end end, 1, nil, 4 do n = n + k + v end return n"))"#),
    // A `return` in a generic `for` given more values than locals: the client's `retstat`
    // (`0x6feae0`) returns from the first free local, a register below where the values went,
    // which holds the generator's first result (`OP_TFORLOOP` called it there, `0x6f9344`).
    ("fix-front_forlist_return", r#"local function R(src) return E(loadstring(src, "=t")) end
        local f = "local function f(s, c) if not c then return 1 end end "
        return S(R(f .. "for k in f, nil, nil, 42 do return k, 5 end"),
          R(f .. "for k in f, nil, nil, 42 do return k, 5, 6 end"),
          R(f .. "for k in f, nil, nil, 42 do return tostring(k) end"),
          R(f .. "local function g(x) return 'g' .. tostring(x) end for k in f, nil, nil, 42 do return g(k) end"),
          R(f .. "for k in f, nil, nil, 42, 43 do return k, 5 end"))"#),
    // A generic `for` over `next` given more values than it has locals: the body's first locals
    // read the registers above the loop's variables, where the client's `OP_TFORLOOP` called
    // the generator (`0x6f9344`), as the call and `lua_next` left them.
    ("fix-vm tforloop_next_registers", r#"local function R(src) return E(loadstring(src, "=t")) end
        local t = "local t, o = {10, 20}, {} "
        return S(R(t .. "for k in next, t, nil, 42 do local y = k table.insert(o, type(y)) end return table.concat(o, ' ')"),
          R(t .. "for k in next, t, nil, 42, 43 do local y, z = k table.insert(o, type(z)) end return table.concat(o, ' ')"),
          R(t .. "for k, v in next, t, nil, 42 do local y = k table.insert(o, tostring(y)) end return table.concat(o, ' ')"),
          R(t .. "for k, v, w in next, t, nil, 42 do local y = k table.insert(o, tostring(y) .. tostring(w)) end return table.concat(o, ' ')"),
          R(t .. "for k, v, w, x in next, t, nil, 42 do local y = k table.insert(o, tostring(y) .. tostring(x)) end return table.concat(o, ' ')"),
          R(t .. "for k in pairs(t) do end for k in next, t, nil, 42 do local y = k table.insert(o, type(y)) end return table.concat(o, ' ')"),
          R(t .. "local function f() local a, b, c, d, e, g, h, i, j, l = 'x', 'x', 'x', 'x', 'x', 'x', 'x', 'x', 'x', 'x' end "
            .. "local function h() for k, v, w, x in next, t, nil, 42 do local y = k table.insert(o, tostring(y)) end end f() h() return table.concat(o, ' ')"))"#),
    // `luaD_precall` starts every Lua frame with its registers nil, and `OP_TAILCALL` makes the
    // callee's frame where its function sits before moving it down: a generic `for` given more
    // values than it has locals lets its body read what the calls under it left, from the
    // register the client's `OP_TFORLOOP` calls at (`0x6f9344`) up.
    ("fix-vm frame_clears", r#"local function R(src) return E(loadstring(src, "=t")) end
        local P = 'local o, n = {}, 0 local function T(v) return type(v) == "function" and "f" or tostring(v) end '
        local B = ' for k in it, "s", "c", 1, 2, 3, 4, 5, 6 do local x1, x2, x3, x4, x5, x6, x7, x8, x9 '
          .. 'table.insert(o, T(x1)..T(x2)..T(x3)..T(x4)..T(x5)..T(x6)..T(x7)..T(x8)..T(x9)) end return table.concat(o, " ")'
        local I = 'local function it(s, c) n = n + 1 if n > 3 then return nil end if n == 1 then '
        -- First, as no other runs before it: the chunk calling the leaky function is in the VM's
        -- fast loop until then, and `g`'s frame, entered from the leaky function, clears `w`'s.
        return S(R(P .. 'local function it(s, c) n = n + 1 if n > 2 then return nil end return n end '
            .. 'local function w() local a, b, c, d, e, f, g, h, i, j, k, l = "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l" end '
            .. 'local function g(x) if x then local a, b, c, d, e, f, h, i, j, k, l = 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11 end end '
            .. 'local function leaky() w() g(false)' .. B .. ' end local r = leaky() return r'),
          R(P .. I .. 'local a, b, c2, d = "A", "B", "C", "D" end return n end' .. B),
          R(P .. 'local function deep(m) if m > 0 then return deep(m - 1) end local a, b, c, d, e = "P", "Q", "R", "S", "T" end '
            .. I .. 'local function g() local a, b, c2, d, e, f = "A", "B", "C", "D", "E", "F" end g() deep(2) end return n end' .. B),
          R(P .. 'local function two(a, b) local x, y = "X", "Y" end ' .. I .. 'two(1, 2, 3, 4, 5, 6, 7) end return n end' .. B),
          R(P .. 'local function w() local a, b, c, d, e, f, g, h = "a", "b", "c", "d", "e", "f", "g", "h" end '
            .. 'local function t2(a) end local function t1() return t2(1) end ' .. I .. 'w() t1() end return n end' .. B))"#),
    // `__index` and `__newindex` chains: 5.1.5's `luaV_gettable` and `luaV_settable` take 100 steps.
    ("fix-vm tag_loop", r#"local function chain(n, field)
          local last = {x = "found"}
          for i = 1, n do last = setmetatable({}, {[field] = last}) end
          return last
        end
        local o = {}
        for _, n in {98, 99, 100, 101} do
          table.insert(o, E(function() return chain(n, "__index").x end))
          table.insert(o, E(function() local t = chain(n, "__newindex") t.y = 1 return "set" end))
        end
        return table.concat(o, "|")"#),
    // A collection leaves a deleted key whose string lives in its node (5.0's `traversetable`
    // passes an empty entry by, `0x6f76be`), so a key set again finds its old node, as it does
    // with no collection: each pair of orders is one. A key equal to an existing one keeps that
    // key, `-0` or `0`.
    ("fix-vm dead_keys", r#"local function order(t) local o = {} for k in pairs(t) do table.insert(o, k) end return table.concat(o, " ") end
        local o = {}
        for _, c in {{5, 4}, {6, 5}, {7, 6}, {8, 4}, {13, 10}, {14, 14}} do
          local n, d = c[1], c[2]
          local keys = {} for i = 1, n do keys[i] = "k" .. i end
          local a = {} for i = 1, n do a[keys[i]] = i end
          local b = {} for i = 1, n do b[keys[i]] = i end
          a[keys[d]] = nil collectgarbage() a[keys[d]] = 0
          b[keys[d]] = nil b[keys[d]] = 0
          table.insert(o, order(a) .. " / " .. order(b))
        end
        local z = 0 local nz = -z
        local function keys(t) local r = {} for k, v in pairs(t) do table.insert(r, tostring(k) .. "=" .. v .. ":" .. tostring(1 / k)) end return table.concat(r, " ") end
        local t = {} t[nz] = 1 t[0] = 2 table.insert(o, keys(t))
        local u = {} u[0] = 1 u[nz] = 2 table.insert(o, keys(u))
        local w = {} w[nz] = 1 w[nz] = nil collectgarbage() w[0] = 2 table.insert(o, keys(w))
        return table.concat(o, " | ")"#),
    // ── fix-libs: library errors the cases above leave out ──
    // The message's first line: the C build's carries a Rust-side traceback after it.
    // `auxsort` raises through `luaL_error` (`0x7fb990`), at the line that called `sort`.
    ("fix-libs sort_invalid_order", r#"local function M(f, ...) local ok, e = pcall(f, unpack(arg)) if ok then return "ok" end
          local _, _, m = string.find(tostring(e), "^([^\n]*)") return m end
        local t = {} for i = 1, 50 do t[i] = math.mod(i * 7, 13) end
        return S(M(table.sort, t, function(a, b) return true end), M(function() table.sort({3, 2, 1, 5, 4, 6}, function(a, b) return a <= b end) end),
          M(function() local u = {5, 3, 1, 4, 2} table.sort(u, function(a, b) return true end) return table.concat(u, " ") end))"#),
    ("fix-libs pattern_errors", r#"local caps = string.rep("(a)", 33)
        -- 32 captures are 34 results: counted in a Lua function, which has room for them, where
        -- `pcall` pushing its status past them trips the C build's API check.
        return S(E(string.find, string.rep("a", 40), caps), E(function() return table.getn({string.find(string.rep("a", 40), string.rep("(a)", 32))}) end),
          E(string.find, "abc", "%b"), E(string.find, "abc", "%ba"), E(string.gsub, "abc", "%b(", "x"),
          E(string.find, "abc", "%fa"), E(string.find, "abc", "%f"), E(string.gfind("abc", "%f%a")),
          E(string.find, "abc", "a)"), E(string.find, "abc", "(a%1)"), E(string.find, "abc", "%0"), E(string.gsub, "abc", "(a)", "%9"))"#),
    ("fix-libs format_repeated_flags", r#"return S(E(string.format, "%-----5d", 1), E(string.format, "%------5d", 1), E(string.format, "%-+ #05d", 1), E(string.format, "%-+ #0-5d", 1),
          E(string.format, "%1234d", 1), E(string.format, "%.1234f", 1), E(string.format, "%12.123f", 1))"#),
    ("fix-libs random_args", r#"return S(E(math.random, 2, 1), E(math.random, 0), E(math.random, -1), E(math.random, 1, 2, 3), E(math.random, 1, 0),
          E(math.random, "x"), E(math.random, 1, "y"), E(random, 0), E(math.random, 3, 3))"#),
    ("fix-libs gsub_replacement_value", r#"return S(E(function() return string.gsub("abc", "%w", {a = {}}) end),
          E(function() return string.gsub("abc", "%w", function(c) return true end) end),
          E(function() return gsub("abc", "%w", function(c) return {} end) end), E(string.gsub, "abc", "%w", {b = false}))"#),
    ("fix-libs debugstack_tail", r#"local function inner() return debugstack() end
        local function viatail() return inner() end
        local function caller() local s = viatail() return s end
        local function deep(n) if n == 0 then return debugstack(1, 2, 1) end return deep(n - 1) end
        local function deeper() local s = deep(3) return s end
        return S(caller(), deeper(), debugstack(1, 0, 0))"#),
    // ── r2-vm: error handlers as 5.1's `luaG_errormsg` runs them ──
    ("r2-vm handler_reenters", r#"local n = 0
        local ok, e = xpcall(function() error("a", 0) end, function(m) n = n + 1 error("b", 0) end)
        return S(ok, e, n)"#),
    ("r2-vm handler_raises_once", r#"local n = 0
        local ok, e = xpcall(function() error("a", 0) end, function(m) n = n + 1 if n == 1 then error("again", 0) end return "H:" .. m end)
        return S(ok, e, n)"#),
    ("r2-vm handler_not_function", r#"local h = setmetatable({}, {__call = function(self, m) return "T:" .. m end})
        local r1 = {xpcall(function() error("a", 0) end, h)}
        local r2 = {xpcall(function() error("a", 0) end, 42)}
        local r3 = {xpcall(function() return 1 end, 42)}
        return S(r1[1], r1[2], r2[1], r2[2], r3[1], r3[2])"#),
    ("r2-vm handler_runtime_error", r#"local n = 0
        local ok, e = xpcall(function() local x = nil; return x.y end, function(m) n = n + 1 return "H:" .. m end)
        return S(ok, e, n)"#),
    // ── r2-vm: tail calls through `__call`, and the depth they are counted at ──
    ("r2-vm tailcall_call_tm", r#"local t = setmetatable({}, {__call = function(self, n) if n == 0 then return "done" end return self(n - 1) end})
        return S(E(function() return t(30000) end), E(function() local u = setmetatable({}, {}) return u(1) end))"#),
    // Overflows at 4096 call records, the client's `luaD_growCI` (`0x6f61c0`).
    ("r2-vm tailcall_depth", r#"local n = 0
        local f
        local function h() return f() end
        f = function() n = n + 1 local r = h() return r end
        local o = {}
        for i = 1, 3 do n = 0 local ok, e = pcall(f) table.insert(o, n .. ":" .. tostring(e)) end
        return table.concat(o, " ")"#),
    // ── r2-vm: a binary chunk is lexed as source (`luaD_protectedparser` zeroes the binary
    // flag, `0x6f6a15`), so its `\27` is an `invalid control char` (`0x6ffabb`) ──
    ("r2-vm loadstring_binary", r#"return S(loadstring("\27Lua\81\0", "=bin"), loadstring("\27Lua"))"#),
];

/// Every library function the sweep calls, each with every argument set in `SWEEP_ARGS`.
const SWEEP_FNS: &[&str] = &[
    "string.byte",
    "string.char",
    "string.find",
    "string.format",
    "string.gfind",
    "string.gsub",
    "string.len",
    "string.lower",
    "string.rep",
    "string.sub",
    "string.upper",
    "string.match",
    "string.gmatch",
    "string.reverse",
    // The table library and `unpack` are the client's: `luaL_checktype`, `luaL_checknumber` and
    // `luaL_argerror`'s messages (`0x6f4b50`, `0x6f4c80`, `0x6f4810`), in the order each reads
    // its arguments (`str_concat` 2, 3, 4, then 1), and `tinsert`'s position form for any count
    // but 2 (`0x7fb6e0`).
    "table.concat",
    "table.getn",
    "table.setn",
    "table.insert",
    "table.remove",
    "table.sort",
    "table.foreach",
    "table.foreachi",
    "table.maxn",
    "math.abs",
    "math.ceil",
    "math.floor",
    "math.max",
    "math.min",
    // `math.mod`, `math.pow` and `math.atan2` check argument 1, then 2, as the client's do
    // (`0x7fafd0`, `0x7fb050`, `0x7faf30`), on every platform.
    "math.mod",
    "math.fmod",
    "math.sqrt",
    "math.exp",
    "math.log",
    "math.log10",
    "math.pow",
    "math.frexp",
    "math.ldexp",
    "math.deg",
    "math.rad",
    "math.sin",
    "math.cos",
    "math.tan",
    "math.asin",
    "math.acos",
    "math.atan",
    "math.atan2",
    "math.modf",
    "math.cosh",
    "math.sinh",
    "math.tanh",
    "tonumber",
    "type",
    "rawequal",
    "rawget",
    "rawset",
    "select",
    // The client's `unpack` reads its table alone (`0x7030a0`): items 1 to `getn`.
    "unpack",
    // `luaH_next`'s text is the client's (`0x871aac`).
    "next",
    // 5.0's `ipairs` (`0x702cd0`): any second argument steps once, as the client answers.
    "ipairs",
    "pairs",
    "getmetatable",
    "setmetatable",
    // `assert` and `loadstring` check their arguments as `luaL_argerror` words it (`0x6f4810`),
    // named `?` when a C function, `pcall`, called them.
    "assert",
    "error",
    "pcall",
    "xpcall",
    "getfenv",
    "setfenv",
    "loadstring",
    "strfind",
    "strsub",
    "strlen",
    "strlower",
    "strupper",
    "strrep",
    "format",
    "gsub",
    // The table library's own functions, so its messages, as above.
    "tinsert",
    "tremove",
    "getn",
    "foreach",
    "foreachi",
    "sort",
    "abs",
    "ceil",
    "floor",
    "max",
    "min",
    // `math.mod`'s order, as above.
    "mod",
    "sqrt",
    "frexp",
    "ldexp",
    "deg",
    "rad",
    "sin",
    "cos",
    "tan",
];

/// The argument sets, built fresh for each call so a mutation stays in it; and the function
/// that runs one name over all of them.
const SWEEP: &str = r#"
local function ARGS() return {
  {}, {nil}, {"abc"}, {"abc", 2}, {"abc", -2}, {"abc", 2, 3}, {"abc", 0, -1}, {"abc", 5, 9},
  {"a.c", "%."}, {"hello", "l+"}, {"hello", "(l)(l)", 1}, {"hello", "l", "L"},
  {"hello", "(h)(e)", "%2%1"}, {"x%sy", "%%"}, {"abc", "[a"}, {"abc", "(", 1}, {1.5}, {-3},
  {0}, {"10"}, {{1, 2, 3}}, {{1, 2, 3}, 2}, {{1, 2, 3}, 2, 9}, {{"a", "b"}, "-", 1, 2},
  {{}, 1}, {2, 3}, {"%5.2f", 3.14159}, {"%d", "x"}, {"%q", "a\"b\n\r\0"}, {true},
  {"abc", "b", 1, true}, {1e300, 1e300}, {-1, 0.5}, {65, 66, 300}, {{3, 1, 2}, "x"},
}
end
local function name(f)
  local t, k = string.find(f, ".", 1, true)
  if t then return getfenv(0)[string.sub(f, 1, t - 1)][string.sub(f, t + 1)] end
  return getfenv(0)[f]
end
function SWEEP(fname)
  local f = name(fname)
  if type(f) ~= "function" then return "absent" end
  local o = {}
  for i, a in ipairs(ARGS()) do
    o[i] = E(f, unpack(a))
  end
  return table.concat(o, " | ")
end
"#;

fn run(src: &str) -> String {
    let s = UiScript::new().expect("a script host");
    match s.lua().load(src).set_name("=t").eval::<String>() {
        Ok(v) => v,
        Err(e) => format!("<raised> {e}"),
    }
}

const GOLDEN: &str = "tests/vm_parity.golden";

/// The cases that list every global: their answers move with each binding the script host
/// gains, not with the VM, so they live in a file of their own that a binding change re-blesses.
const SURFACE_GOLDEN: &str = "tests/vm_parity_surface.golden";
const SURFACE_CASES: &[&str] = &["pairs_globals", "global_surface"];

/// `escape_default` undone: the golden file's one-line answers back to text.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let hex: String = it.by_ref().skip(1).take_while(|&c| c != '}').collect();
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
            }
            Some(c) => out.push(c),
            None => {}
        }
    }
    out
}

/// Cases whose answer on the C build depends on the platform's C library or compiler, so the
/// golden file holds one answer per platform, `name@<os>-<arch>`, each blessed there:
/// - the transcendental functions are the host libm's, rounded differently by glibc and Apple's;
/// - a NaN prints its sign under glibc (`-nan`), never under Apple's libc, and which NaN a
///   function answers is the host libm's;
/// - `luaL_checknumber` on two arguments in one C call (`fmod`, `pow`, `ldexp`, `atan2`) checked
///   them in the compiler's order: gcc on x86-64 the second first, clang on arm64 the first.
///   benilla-lua checks them in the client's order on every platform (`pow`, `atan2` and `mod`
///   the first, `ldexp` the second), so the error items of the first three agree everywhere;
/// - a `double` past the integer range cast to `ptrdiff_t` or `unsigned long` is undefined: x86-64
///   answers `INT64_MIN`, arm64 saturates.
///
/// benilla-lua pins the reference platform's answers for the compiler cases, and runs
/// the host libm, so it answers a whole case as the C build only on the reference platform.
/// Elsewhere, and on a platform no answer was blessed on, a case's answer is split into items at
/// its separator, and each item every recorded platform answers alike is checked: only the items
/// the platforms differ on go unchecked.
const PLATFORM_CASES: &[&str] = &[
    "fix-libs deg_rad",
    "sweep string.char",
    "sweep string.sub",
    "sweep strsub",
    "sweep getfenv",
    "sweep math.mod",
    "sweep mod",
    "sweep math.sqrt",
    "sweep sqrt",
    "sweep math.log",
    "sweep math.pow",
    "sweep math.ldexp",
    "sweep ldexp",
    "sweep math.sin",
    "sweep math.tan",
    "sweep tan",
    "sweep math.asin",
    "sweep math.atan",
    "sweep math.atan2",
];

/// Where a platform case's answer splits into items: a sweep's calls, the random numbers, else
/// the answer's `|`-separated parts.
fn item_separator(name: &str) -> &'static str {
    if name.starts_with("sweep ") {
        " | "
    } else if name == "fix-libs random_ranges" {
        " "
    } else {
        "|"
    }
}

/// A platform case checked item by item against `recorded`, each blessed platform's answer: the
/// failures, how many items went unchecked because the platforms answer them differently, and
/// how many there are. `None` when nothing was recorded, or the platforms' answers do not split
/// alike.
fn check_items(name: &str, got: &str, recorded: &[String]) -> Option<(Vec<String>, usize, usize)> {
    let sep = item_separator(name);
    let sets: Vec<Vec<&str>> = recorded.iter().map(|a| a.split(sep).collect()).collect();
    let n = sets.first()?.len();
    if sets.iter().any(|s| s.len() != n) {
        return None;
    }
    let got: Vec<&str> = got.split(sep).collect();
    if got.len() != n {
        return Some((
            vec![format!("{name}: {} items, the C build {n}", got.len())],
            0,
            n,
        ));
    }
    let mut failures = Vec::new();
    let mut unchecked = 0;
    // A NaN prints the same `nan` under Apple's libc whatever its sign, so a NaN item agreeing
    // says nothing of the sign another platform's libm gives it.
    let nan = |a: &str| {
        a.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|t| t == "nan")
    };
    for (i, g) in got.iter().enumerate() {
        let w = sets[0][i];
        if sets.iter().any(|s| s[i] != w || nan(s[i])) {
            unchecked += 1;
        } else if *g != w {
            failures.push(format!(
                "{name} item {i}\n    got:  {}\n    want: {}",
                g.escape_default(),
                w.escape_default()
            ));
        }
    }
    Some((failures, unchecked, n))
}

/// The platform benilla-lua's pinned answers are the C build's on: Linux, x86-64, glibc.
const REFERENCE_PLATFORM: &str = "linux-x86_64";

fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The golden key a case's answer is under on `platform`: its name, or for a platform case its
/// name and the platform.
fn golden_key(name: &str, platform: &str) -> String {
    if PLATFORM_CASES.contains(&name) {
        format!("{name}@{platform}")
    } else {
        name.to_string()
    }
}

/// Each answer against its golden line; a difference goes into `failures`.
fn compare(golden: &str, answers: &[(&str, String)], failures: &mut Vec<String>) {
    let want: std::collections::HashMap<&str, &str> =
        golden.lines().filter_map(|l| l.split_once('\t')).collect();
    for (name, got) in answers {
        let got = got.escape_default().to_string();
        match want.get(name) {
            Some(w) if *w == got => {}
            Some(w) => failures.push(format!("{name}\n    got:  {got}\n    want: {w}")),
            None => failures.push(format!("{name}: no golden answer")),
        }
    }
}

#[test]
fn every_case_answers_as_the_c_lua_build_recorded() {
    let sweep: Vec<(String, String)> = SWEEP_FNS
        .iter()
        .map(|f| (format!("sweep {f}"), format!("return SWEEP({f:?})")))
        .collect();
    let cases: Vec<(&str, String)> = CASES
        .iter()
        .map(|(n, b)| (*n, b.to_string()))
        .chain(
            sweep
                .iter()
                .map(|(n, b)| (n.as_str(), format!("{SWEEP}\n{b}"))),
        )
        .collect();
    let answers: Vec<(&str, String)> = cases
        .iter()
        .map(|(name, body)| (*name, run(&format!("{PRELUDE}\n{body}"))))
        .collect();
    let mut names: Vec<&str> = cases.iter().map(|c| c.0).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), cases.len(), "case names are unique");
    for p in PLATFORM_CASES {
        assert!(names.contains(p), "platform case {p:?} is a case");
    }
    // The cases that list every global have a file of their own, one answer each.
    let (surface, answers): (Vec<_>, Vec<_>) = answers
        .into_iter()
        .partition(|(name, _)| SURFACE_CASES.contains(name));
    let platform = platform();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let golden = std::fs::read_to_string(dir.join(GOLDEN)).expect("the golden file");
    let want: std::collections::BTreeMap<String, String> = golden
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(k, a)| (k.to_string(), a.to_string()))
        .collect();
    for k in want.keys() {
        let (name, on) = k
            .split_once('@')
            .map_or((k.as_str(), false), |(n, _)| (n, true));
        assert_eq!(
            on,
            PLATFORM_CASES.contains(&name),
            "{k}: a platform case's answers are per platform, any other case's one for all"
        );
    }
    // The VM's answers are the C build's record, and no C build is left to bless them.
    if std::env::var_os("BENILLA_BLESS").is_some() {
        let surface_lines: String = surface
            .iter()
            .map(|(n, a)| format!("{n}\t{}\n", a.escape_default()))
            .collect();
        std::fs::write(dir.join(SURFACE_GOLDEN), surface_lines).expect("write the golden file");
    }
    let mut failures = Vec::new();
    let mut skipped = Vec::new();
    let (mut unchecked_items, mut items) = (0, 0);
    for (name, got) in &answers {
        let platform_case = PLATFORM_CASES.contains(name);
        // benilla-lua answers a whole platform case as the C build only where its pins were
        // taken; elsewhere, and where no answer was blessed, the items every platform agrees on.
        let own = want.get(&golden_key(name, &platform));
        if platform_case && (own.is_none() || platform != REFERENCE_PLATFORM) {
            let prefix = format!("{name}@");
            let recorded: Vec<String> = want
                .iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(_, a)| unescape(a))
                .collect();
            match check_items(name, got, &recorded) {
                Some((f, unchecked, n)) => {
                    failures.extend(f);
                    unchecked_items += unchecked;
                    items += n;
                }
                None => skipped.push(*name),
            }
            continue;
        }
        let got = got.escape_default().to_string();
        match own {
            Some(w) if *w == got => {}
            Some(w) => failures.push(format!("{name}\n    got:  {got}\n    want: {w}")),
            None => failures.push(format!("{name}: no golden answer")),
        }
    }
    let surface_golden =
        std::fs::read_to_string(dir.join(SURFACE_GOLDEN)).expect("the surface golden file");
    compare(&surface_golden, &surface, &mut failures);
    if items > 0 {
        eprintln!(
            "platform cases on {platform}: {} of {items} items checked, {unchecked_items} the \
             platforms answer differently not",
            items - unchecked_items
        );
    }
    if !skipped.is_empty() {
        eprintln!(
            "{} platform cases not checked on {platform}: {}",
            skipped.len(),
            skipped.join(", ")
        );
    }
    assert!(
        failures.is_empty(),
        "{} answers of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
