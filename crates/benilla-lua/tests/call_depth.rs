//! Call depth: the client's limits raise catchable errors, and no script reaches the end of the Rust
//! stack. Each case runs in a debug build on a 2 MB thread, recursing through Lua frames,
//! metamethods, protected calls and library callbacks, mixed.

fn run(vm: &mut benilla_lua::Vm, src: &str) -> String {
    match vm.exec(src.as_bytes(), b"=t") {
        Ok(v) => v.first().map_or(String::new(), |&v| vm.display(v)),
        Err(e) => format!("<raised> {}", vm.display(e)),
    }
}

/// `src` on a 2 MB thread, in a fresh state; its answer.
fn small_stack(src: &'static str) -> String {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let mut vm = benilla_lua::new_state();
            run(&mut vm, src)
        })
        .unwrap()
        .join()
        .unwrap()
}

/// The client's `luaD_growCI` (`0x6f61c0`) doubles the call-info array from 8 and raises once it
/// passes 4096 (`0x6f61c7`, `0x6f61e4`): the main chunk and 4094 nested calls fit, one more is
/// refused, and every later overflow comes at the same depth.
#[test]
fn lua_recursion_goes_4096_deep_then_raises() {
    let depth = |n: u32| {
        let src = format!(
            "local function f(n) if n == 0 then return 0 end return 1 + f(n - 1) end \
             local r = f({n}) return r"
        );
        small_stack(Box::leak(src.into_boxed_str()))
    };
    assert_eq!(depth(4093), "4093");
    assert_eq!(depth(4094), "<raised> t:1: stack overflow");
    assert_eq!(
        small_stack(
            "local n = 0 local function f() n = n + 1 f() end \
             local o = {} for i = 1, 3 do n = 0 local ok, e = pcall(f) o[i] = n .. ' ' .. e end \
             return table.concat(o, ', ')"
        ),
        "4093 t:1: stack overflow, 4093 t:1: stack overflow, 4093 t:1: stack overflow"
    );
}

#[test]
fn an_overflows_handler_runs_and_a_runaway_handler_is_stopped() {
    assert_eq!(
        small_stack(
            "local function f() f() end \
             local ok, e = xpcall(f, function(e) return 'H:' .. e end) return e"
        ),
        "H:t:1: stack overflow"
    );
    assert_eq!(
        small_stack(
            "local function f() f() end local function h(e) return h(e) .. '' end \
             local ok, e = xpcall(f, h) return e"
        ),
        "error in error handling"
    );
}

#[test]
fn host_level_recursion_raises_c_stack_overflow() {
    for (src, want) in [
        (
            "local t = setmetatable({}, {__index = function(t, k) return t[k] end}) return t.x",
            "<raised> t:1: C stack overflow",
        ),
        (
            "local t = setmetatable({}, {}) getmetatable(t).__newindex = function(t, k, v) t[k] = v end t.x = 1",
            "<raised> t:1: C stack overflow",
        ),
        (
            // `__call` makes a Lua call, which 5.1 counts as one: the Lua limit.
            "local t = setmetatable({}, {}) getmetatable(t).__call = function(self) return self() + 1 end return t()",
            "<raised> t:1: stack overflow",
        ),
        (
            "local mt = {} mt.__concat = function(a, b) return a .. b end \
             return setmetatable({}, mt) .. 'x'",
            "<raised> t:1: C stack overflow",
        ),
        (
            "local mt = {} mt.__lt = function(a, b) return a < b end \
             local a, b = setmetatable({}, mt), setmetatable({}, mt) return a < b",
            "<raised> t:1: C stack overflow",
        ),
        (
            "local mt = {} mt.__eq = function(a, b) return a == b end \
             local a, b = setmetatable({}, mt), setmetatable({}, mt) return a == b",
            "<raised> t:1: C stack overflow",
        ),
        (
            "local t = setmetatable({}, {}) getmetatable(t).__tostring = function(x) return tostring(x) end \
             return tostring(t)",
            "<raised> C stack overflow",
        ),
        (
            "local function h() return (string.gsub('a', 'a', h)) end return h()",
            "<raised> C stack overflow",
        ),
        (
            "local function s() table.sort({3, 1, 2}, function(a, b) s() return a < b end) end s()",
            "<raised> C stack overflow",
        ),
        (
            "local function g() local ok, e = pcall(g) if not ok then error(e, 0) end end g()",
            "<raised> C stack overflow",
        ),
    ] {
        assert_eq!(small_stack(src), want, "{src}");
    }
}

#[test]
fn mixed_native_and_lua_recursion_raises() {
    // Each host level runs a deep stretch of Lua frames, a metamethod, a protected call and a
    // library callback.
    let got = small_stack(
        "local t local function deep(n) if n > 0 then return deep(n - 1) + 0 end return t.x end \
         t = setmetatable({}, {__index = function(t, k) \
           local ok, e = pcall(function() return string.gsub('a', 'a', function() return deep(50) end) end) \
           if not ok then error(e, 0) end return e end}) \
         return deep(10)",
    );
    assert!(got.ends_with("C stack overflow"), "{got}");
}

#[test]
fn a_handler_at_the_host_limit_is_bounded() {
    // The C build fails an API assertion here (`lua_pushboolean`); this VM answers.
    let got = small_stack(
        "local m, last = 0 \
         local function g() m = m + 1 return xpcall(g, function(e) last = 'H' .. m .. ':' .. tostring(e) return last end) end \
         g() return last",
    );
    assert!(got.contains("C stack overflow"), "{got}");
}
