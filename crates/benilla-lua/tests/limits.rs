//! Inputs that once crashed the process from Lua source: each must raise a catchable Lua error
//! instead, in a debug build, on a 2 MB thread.

/// Run `src` in the script host's state on a 2 MB thread; its results as text, or the error.
fn run(src: &'static str) -> Result<Vec<String>, String> {
    run_with(src, |_| {})
}

/// [`run`] in a state `setup` prepared first.
fn run_with(
    src: &'static str,
    setup: impl FnOnce(&mut benilla_lua::Vm) + Send + 'static,
) -> Result<Vec<String>, String> {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let mut vm = benilla_lua::new_state();
            setup(&mut vm);
            match vm.exec(src.as_bytes(), b"=t") {
                Ok(vs) => Ok(vs.into_iter().map(|v| vm.display(v)).collect()),
                Err(e) => Err(vm.display(e)),
            }
        })
        .unwrap()
        .join()
        .unwrap()
}

/// A result past `MAX_STRING` is 5.1's `not enough memory`, before anything is allocated.
#[test]
fn a_huge_string_rep_is_not_enough_memory() {
    assert_eq!(
        run(r#"return pcall(string.rep, string.rep("x", 1024), 2147483647)"#),
        Ok(vec!["false".into(), "not enough memory".into()])
    );
    assert_eq!(
        run(r#"return pcall(string.rep, "xy", 2147483647)"#),
        Ok(vec!["false".into(), "not enough memory".into()])
    );
    assert_eq!(
        run(r#"return string.rep("ab", 3), string.rep("", 2147483647), string.rep("x", -1)"#),
        Ok(vec!["ababab".into(), "".into(), "".into()])
    );
}

/// A match nested past the Rust stack the VM allows is 5.2's `pattern too complex`, where 5.1
/// overflows the stack: through `find`, `gsub` and `gfind`, and under the deepest native nesting
/// the VM allows. It holds at opt-level 0 too (`CARGO_PROFILE_DEV_OPT_LEVEL=0`).
#[test]
fn a_deep_match_is_pattern_too_complex() {
    let deep = r#"local n = 20000
        local s, p = string.rep("a", n), string.rep("a?", n)
        local ok1, e1 = pcall(string.find, s, p)
        local ok2, e2 = pcall(string.gsub, s, p, "")
        local ok3, e3 = pcall(function() for w in string.gfind(s, p) do end end)
        return ok1, e1, ok2, e2, ok3, e3"#;
    let msg = "pattern too complex";
    let got = run(deep).unwrap();
    assert_eq!(got.len(), 6, "{got:?}");
    for (i, r) in got.chunks(2).enumerate() {
        assert_eq!(r[0], "false", "call {i}: {r:?}");
        assert!(r[1].ends_with(msg), "call {i}: {r:?}");
    }
    let nested = r#"local s, p = string.rep("a", 20000), string.rep("a?", 20000)
        local d = 0
        local function f()
          d = d + 1
          local ok, e = pcall(string.find, s, p)
          if not ok and not string.find(e, "too complex", 1, true) then return e end
          local r = string.gsub("x", "x", f)
          return r
        end
        local ok, e = pcall(f)
        return d, e"#;
    let got = run(nested).unwrap();
    // Unoptimised frames are larger, so fewer levels fit; any build nests some.
    assert!(got[0].parse::<u32>().unwrap() > 10, "{got:?}");
    assert!(got[1].ends_with("C stack overflow"), "{got:?}");
    // Far below the bound, a match nested deeper than any real pattern still runs.
    assert_eq!(
        run(r#"return string.find(string.rep("a", 300), string.rep("a?", 300))"#),
        Ok(vec!["1".into(), "300".into()])
    );
}

/// `unpack` checks against the client's own bound, 5.0's 2048 (`0x6f2f43`), its one argument
/// counted, and raises its text (`0x872bc4`).
#[test]
fn results_beyond_the_c_stack_raise() {
    let src = r#"local t = {} for i = 1, 9000 do t[i] = i end
        local ok1, e1 = pcall(unpack, t)
        local u = {} for i = 1, 2048 do u[i] = i end
        local ok2, e2 = pcall(function() return unpack(u) end)
        u[2048] = nil
        local n3 = table.getn({unpack(u)})
        return ok1, e1, ok2, e2, n3"#;
    let got = run(src).unwrap();
    assert_eq!(
        &got[..2],
        ["false", "stack overflow (table too big to unpack)"],
        "{got:?}"
    );
    // One argument in use: 2048 more pass 2048.
    assert_eq!(got[2], "false", "{got:?}");
    assert_eq!(
        got[3], "t:4: stack overflow (table too big to unpack)",
        "{got:?}"
    );
    assert_eq!(got[4], "2047");
}

/// Past the ids a string value can name, a new string is `not enough memory`, raised where it
/// was made (a concatenation, a native, the compiler) and caught by the `pcall` around it. Dead
/// strings give their ids back first: a collection is asked for before the ids run out.
#[test]
fn running_out_of_string_ids_is_not_enough_memory() {
    fn limit(vm: &mut benilla_lua::Vm) {
        vm.heap.limit_string_ids(50_000);
    }
    let src = r#"local t = {}
        local ok1, e1 = pcall(function() for i = 1, 100000 do t[i] = "s" .. i end end)
        local n1 = table.getn(t)
        local ok2, e2 = pcall(function() for i = 1, 100000 do t[i] = tostring(-i) end end)
        local ok3, e3 = pcall(loadstring, "return 'a new constant'")
        t = nil
        collectgarbage()
        local s = 0
        for i = 1, 300000 do s = s + string.len("g" .. i) end
        return ok1, e1, n1 > 40000 and n1 < 50000, ok2, e2, ok3, e3, s"#;
    let got = run_with(src, limit).unwrap();
    assert_eq!(
        got,
        [
            "false",
            "not enough memory",
            "true",
            "false",
            "not enough memory",
            "true",
            "nil",
            "1988895"
        ],
        "{got:?}"
    );
    // Live strings near the bound and garbage made on top: the collection asked for frees it.
    let churn = r#"local keep = {} for i = 1, 40000 do keep[i] = "k" .. i end
        local n = 0 for i = 1, 300000 do if ("g" .. i) ~= "" then n = n + 1 end end
        return n, table.getn(keep)"#;
    assert_eq!(
        run_with(churn, limit),
        Ok(vec!["300000".into(), "40000".into()])
    );
}

/// A memory error is thrown as it is: `xpcall`'s handler runs only for a runtime error
/// (`luaG_errormsg`), so the protected call answers `not enough memory` itself.
#[test]
fn a_memory_error_runs_no_error_handler() {
    let src = r#"local t = {}
        local ok, e = xpcall(function() for i = 1, 100000 do t[i] = "s" .. i end end,
          function(m) Handled = true return "H:" .. m end)
        return ok, e, Handled == nil"#;
    let got = run_with(src, |vm| vm.heap.limit_string_ids(50_000));
    assert_eq!(
        got,
        Ok(vec![
            "false".into(),
            "not enough memory".into(),
            "true".into()
        ])
    );
}

/// A hot reload whose constants cannot be made changes nothing and says so, as `loadstring`
/// answers `not enough memory`.
#[test]
fn a_hot_reload_out_of_string_ids_is_not_enough_memory() {
    let got = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            let mut vm = benilla_lua::new_state();
            vm.heap.limit_string_ids(50_000);
            vm.exec(b"function F() return 'old' end", b"@f.lua")
                .unwrap();
            vm.exec(
                b"Keep = {} for i = 1, 100000 do Keep[i] = 'k' .. i end",
                b"=t",
            )
            .unwrap_err();
            let r = vm.hot_reload(
                b"@f.lua",
                b"function F() return 'a constant no one made' end",
            );
            let f = vm.get_global("F");
            let old = vm.call_value(f, &[], 1).map(|v| vm.display(v[0]));
            (r.map(|_| ()).map_err(|e| e.to_string()), old.ok())
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(
        got,
        (
            Err("not enough memory".to_string()),
            Some("old".to_string())
        )
    );
}

/// `pcall` and `xpcall` return every result of the call, however many fill its frame, as
/// `luaB_pcall` does: the C build's API check aborted a debug build at 22.
#[test]
fn a_protected_call_returns_every_result() {
    let src = r#"local function seq(k) local t = {} for i = 1, k do t[i] = i end return t end
        local forty = seq(40)
        local function many() return unpack(forty) end
        local function n(...) return arg.n, arg[1], arg[arg.n] end
        local o = {}
        for _, k in {21, 22, 23, 60, 300} do
          local a, b, c = n(pcall(unpack, seq(k)))
          table.insert(o, a .. ":" .. tostring(b) .. ":" .. c)
        end
        local a, b, c = n(pcall(many))
        table.insert(o, a .. ":" .. tostring(b) .. ":" .. c)
        a, b, c = n(xpcall(many, function(e) return e end))
        table.insert(o, a .. ":" .. tostring(b) .. ":" .. c)
        a, b, c = n(pcall(string.find, string.rep("a", 30), string.rep("(a)", 30)))
        table.insert(o, a .. ":" .. tostring(b) .. ":" .. c)
        return table.concat(o, " ")"#;
    assert_eq!(
        run(src).unwrap(),
        ["22:true:21 23:true:22 24:true:23 61:true:60 301:true:300 41:true:40 41:true:40 33:true:a"]
    );
}
