//! Every case, run with a collection at every safe point, must answer as it does without one: a
//! value the roots miss is freed and its handle reused, which changes an answer. What each case
//! must answer is benilla-ui's `tests/vm_parity.golden`, recorded on the C Lua build.

/// Serializers both dialects run: `S(...)` writes values, `E(f, ...)` writes a protected call.
const PRELUDE: &str = r#"
local function S1(v)
  local t = type(v)
  if t == "string" then return string.format("%q", v)
  elseif t == "number" or t == "boolean" or t == "nil" then return tostring(v)
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
  return "err " .. tostring(r[2])
end
"#;

const CASES: &[(&str, &str)] = &[
    ("arith", "return S(1+2, 10/4, 2^10, -3, 7-2*3, 3-(-2), 2^-1, -2^2)"),
    ("numfmt", "return S(1e15, 1e100, 0.1, 1/3, 123456789012345, -0.5, 100, 2^53)"),
    ("coerce", r#"return S("10"+5, "3"*"4", 10 .. 20, 1.5 .. "", " 7 " + 0, -"2")"#),
    // A constant on either side of each operator, metamethods in source order.
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
    // ── strings ──
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
    ("gsub", r#"return S(string.gsub("hello world", "o", "0"), gsub("abc", "%w", "%0%0"), gsub("hello", "l+", function(s) return "[" .. s .. "]" end), gsub("abc", "", "-"), gsub("a b c", " ", "", 1))"#),
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
    // ── tables ──
    ("ctor", "local t = {1, 2, 3, x = 4, [10] = 5; 6} return S(t[1], t[2], t[3], t.x, t[10], t[4], getn(t))"),
    ("ctor_semicolon", "local t = {a = 1;; b = 2} return S(t.a, t.b)"),
    ("getn", "local t = {1, 2, nil, 4} local u = {n = 7} return S(getn(t) >= 2, getn(u), table.getn({}), getn({1, 2, 3}))"),
    ("setn", "local t = {} table.setn(t, 5) local u = {n = 1} table.setn(u, 3) return S(getn(t), u.n, t.n)"),
    ("insert_remove", "local t = {} tinsert(t, 'a') tinsert(t, 'c') tinsert(t, 2, 'b') local r = tremove(t, 1) return S(r, t[1], t[2], t[3], getn(t), tremove({}))"),
    ("concat_tbl", "return S(table.concat({1, 2, 3}, ', '), table.concat({'a', 'b'}), table.concat({1, 2, 3}, '-', 2, 3))"),
    ("concat_err", "return E(function() return table.concat({1, {}, 3}) end)"),
    ("sort", "local t = {5, 2, 8, 1, 9, 3} table.sort(t) local u = {'b', 'a', 'c'} sort(u, function(a, b) return a > b end) return table.concat(t, ',') .. '|' .. table.concat(u, ',')"),
    ("sort_many", "local t = {} for i = 1, 100 do t[i] = math.mod(i * 37, 101) end table.sort(t) local ok = true for i = 2, 100 do if t[i-1] > t[i] then ok = false end end return S(ok, t[1], t[100])"),
    ("foreach", "local o = {} foreachi({'a', 'b', 'c'}, function(i, v) table.insert(o, i .. v) end) local r = foreachi({1, 2, 3}, function(i, v) if v == 2 then return 'stop' end end) return table.concat(o, ',') .. S(r)"),
    ("unpack", "return S(unpack({1, 2, 3})) .. '|' .. S(unpack({n = 2, 'a', 'b', 'c'}))"),
    ("next", "local t = {} t.x = 1 local k, v = next(t) return S(k, v, next({}), next(t, 'x'))"),
    ("next_err", "return E(function() return next({}, 'nope') end)"),
    ("ipairs", "local o = {} for i, v in ipairs({'a', 'b', nil, 'd'}) do table.insert(o, i .. v) end return table.concat(o, ',')"),
    ("pairs_array", "local o = {} for k, v in pairs({10, 20, 30}) do table.insert(o, k .. '=' .. v) end return table.concat(o, ',')"),
    ("generic_for_table", "local s = 0 for k, v in {1, 2, 3} do s = s + v end return S(s)"),
    ("nil_key", "return E(function() local t = {} t[nil] = 1 end)"),
    ("rawops", "local t = setmetatable({}, {__index = function() return 'meta' end, __newindex = function() end}) t.x = 1 rawset(t, 'y', 2) return S(t.x, rawget(t, 'x'), t.y, rawget(t, 'y'), rawequal(t, t))"),
    // ── metatables ──
    ("index_chain", "local base = {a = 1} local mid = setmetatable({b = 2}, {__index = base}) local t = setmetatable({}, {__index = mid}) return S(t.a, t.b, t.c)"),
    ("newindex", "local log = {} local t = setmetatable({}, {__newindex = function(t, k, v) rawset(t, k, v * 2) end}) t.x = 5 t.x = 7 return S(t.x)"),
    ("arith_mt", "local mt = {__add = function(a, b) return 'add' end, __unm = function() return 'neg' end, __concat = function(a, b) return 'cat' end} local t = setmetatable({}, mt) return S(t + 1, 1 + t, -t, t .. 'x', 'x' .. t)"),
    ("eq_mt", "local mt = {__eq = function() return true end} local a, b = setmetatable({}, mt), setmetatable({}, mt) local c = setmetatable({}, {__eq = function() return true end}) return S(a == b, a == c, a ~= b)"),
    ("lt_mt", "local mt = {__lt = function(a, b) return a.v < b.v end} local a, b = setmetatable({v = 1}, mt), setmetatable({v = 2}, mt) return S(a < b, b < a, a > b, a <= b)"),
    ("call_mt", "local t = setmetatable({}, {__call = function(self, a, b) return a + b end}) return S(t(2, 3))"),
    ("tostring_mt", "local t = setmetatable({}, {__tostring = function() return 'custom' end}) return tostring(t)"),
    ("protected_mt", "local t = setmetatable({}, {__metatable = 'locked'}) return S(getmetatable(t)) .. E(function() setmetatable(t, {}) end)"),
    ("pow_global", "local old = __pow __pow = function(a, b) return 'p' end local r = 2 ^ 3 __pow = old return S(r, 2 ^ 3)"),
    // ── functions ──
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
    // ── 5.0's loops ──
    ("numfor", "local o = {} for i = 1, 3 do table.insert(o, i) end for i = 3, 1, -1 do table.insert(o, i) end for i = 1, 0 do table.insert(o, 'never') end for i = 1, 2, 0.5 do table.insert(o, i) end return table.concat(o, ',')"),
    ("numfor_closures", "local fs = {} for i = 1, 3 do fs[i] = function() return i end end return S(fs[1](), fs[2](), fs[3]())"),
    ("genfor_closures", "local fs = {} for k, v in ipairs({'a', 'b'}) do fs[k] = function() return v end end return S(fs[1](), fs[2]())"),
    ("body_local_closures", "local fs = {} for i = 1, 3 do local j = i fs[i] = function() return j end end return S(fs[1](), fs[2](), fs[3]())"),
    ("numfor_assign", "local o = {} for i = 1, 5 do table.insert(o, i) i = i + 1 end return table.concat(o, ',')"),
    ("genfor_control", "local o = {} for k, v in pairs({a = 1}) do table.insert(o, k) end return table.concat(o)"),
    ("for_errors", "return E(function() for i = 'x', 2 do end end) .. E(function() for i = 1, 'y' do end end)"),
    ("while_repeat", "local i, o = 0, {} while i < 3 do i = i + 1 table.insert(o, i) end repeat i = i - 1 table.insert(o, i) until i == 0 return table.concat(o, ',')"),
    ("repeat_scope", "local x = 'outer' local n = 0 repeat local x = 'inner' n = n + 1 until x == 'outer' or n > 2 return S(n)"),
    ("break", "local o = {} for i = 1, 10 do if i > 3 then break end table.insert(o, i) end while true do break end return table.concat(o, ',')"),
    ("break_closure", "local fs = {} local i = 0 while true do i = i + 1 local j = i fs[i] = function() return j end if i == 2 then break end end return S(fs[1](), fs[2]())"),
    // ── errors ──
    ("err_index_global", "return E(function() return undefined_thing.x end)"),
    ("err_index_local", "return E(function() local t return t.x end)"),
    ("err_index_field", "return E(function() local t = {} return t.a.b end)"),
    ("err_index_upvalue", "local up return E(function() return up.x end)"),
    ("err_call", "return E(function() undefined_fn() end) .. E(function() local t = {} t.m() end) .. E(function() local t = {} t:m() end)"),
    ("err_arith", "return E(function() local x return x + 1 end) .. E(function() return {} * 2 end)"),
    ("error_fn", "return E(function() error('boom') end) .. E(function() error('noloc', 0) end) .. string.sub(E(function() error({}) end), 1, 11) .. E(function() error(42) end)"),
    ("error_level2", "local function f() error('deep', 2) end return E(function() f() end)"),
    ("assert", "return E(function() assert(false) end) .. E(function() assert(nil, 'msg') end) .. S(assert(1, 2, 3))"),
    ("bad_arg", "return E(function() string.sub() end) .. E(function() strfind(nil, 'x') end) .. E(function() setmetatable(1, {}) end) .. E(function() math.floor('x') end)"),
    ("bad_self", "local t = {f = string.len} return E(function() return t:f() end)"),
    ("xpcall", "local ok, msg = xpcall(function() error('x') end, function(e) return 'handled: ' .. e end) return S(ok, msg) .. '|' .. S(xpcall(function() return 1, 2 end, print))"),
    ("pcall_args", "return S(pcall(function(a, b) return a + b end, 1, 2))"),
    ("nested_pcall", "return E(function() local ok, e = pcall(error, 'inner') error('outer:' .. e, 0) end)"),
    // ── environments ──
    ("fenv", "local env = {x = 'env'} local function f() return x end setfenv(f, env) return S(f(), getfenv(f) == env, getfenv(0) == getfenv(), getfenv(1) == getfenv())"),
    ("fenv_level", "x = 'global' local function f() setfenv(1, {x = 'set'}) return x end return S(f())"),
    ("loadstring", "local f = loadstring('return 1 + 1') local g, e = loadstring('return +') return S(f(), g, e)"),
    ("loadstring_name", "local f = loadstring('error(\"x\")', '=named') return E(f)"),
    ("globals", "return S(type(_G), type(print), type(select), type(coroutine), type(string.gmatch), type(math.fmod), type(gcinfo), type(bit), type(getn), type(strfind), type(sin))"),
    ("type", "return S(type(nil), type(1), type('s'), type({}), type(print), type(type), type(true))"),
    ("tostring", "return S(tostring(nil), tostring(true), tostring(12), tostring('x'), string.sub(tostring({}), 1, 7), string.sub(tostring(print), 1, 10))"),
    // ── bit, math ──
    ("bit", "return S(bit.band(12, 10), bit.bor(12, 10), bit.bxor(12, 10), bit.bnot(0), bit.lshift(1, 4), bit.rshift(256, 4), bit.arshift(-16, 2), bit.lshift(1, 32), bit.mod(7, 3))"),
    ("math", "return S(math.abs(-3), math.max(1, 5, 3), math.min(4, 2), math.sqrt(16), math.floor(2.7), math.pi > 3.14, math.frexp(8), math.ldexp(0.5, 4), math.deg(math.pi))"),
    ("random", "math.randomseed(42) local a = math.random(100) math.randomseed(42) return S(a == math.random(100), math.random() < 1, math.random(5, 5))"),
];

fn ours_with(src: &str, gc_stress: bool) -> String {
    let mut vm = benilla_lua::new_state();
    vm.gc_stress = gc_stress;
    match vm.exec(src.as_bytes(), b"=t") {
        Ok(vals) => vals.first().map_or("<no value>".into(), |&v| vm.display(v)),
        Err(e) => format!("<raised> {}", vm.display(e)),
    }
}

/// Every case again with a collection at every safe point: a value the roots miss is freed and
/// its handle reused, which changes an answer.
#[test]
fn every_case_answers_the_same_under_a_collection_at_every_safe_point() {
    let mut failures = Vec::new();
    for (name, body) in CASES {
        let src = format!("{PRELUDE}\n{body}");
        let (a, b) = (ours_with(&src, false), ours_with(&src, true));
        if a != b {
            failures.push(format!("{name}\n    normal: {a}\n    stress: {b}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
