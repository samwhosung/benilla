//! The collector: garbage goes, what is reachable stays intact, weak tables drop their dead.

use benilla_lua::Value;

fn run(vm: &mut benilla_lua::Vm, src: &str) -> String {
    match vm.exec(src.as_bytes(), b"=t") {
        Ok(v) => v.first().map_or(String::new(), |&v| vm.display(v)),
        Err(e) => format!("<raised> {}", vm.display(e)),
    }
}

#[test]
fn garbage_is_swept_and_reachable_values_survive() {
    let mut vm = benilla_lua::new_state();
    run(
        &mut vm,
        "keep = {} for i = 1, 20000 do local t = {i, tostring(i)} if math.mod(i, 100) == 0 then table.insert(keep, t) end end",
    );
    vm.full_gc();
    let tables = vm.heap.counts()[1];
    assert!(
        tables < 1000,
        "20000 garbage tables should be gone, {tables} live"
    );
    assert_eq!(
        run(&mut vm, "local s = 0 for _, t in ipairs(keep) do s = s + t[1] assert(t[2] == tostring(t[1])) end return getn(keep) .. ':' .. s"),
        "200:2010000"
    );
}

#[test]
fn closures_keep_their_upvalues_through_collections() {
    let mut vm = benilla_lua::new_state();
    vm.gc_stress = true;
    let r = run(
        &mut vm,
        "local fs = {} for i = 1, 50 do local s = string.rep('x', i) fs[i] = function() return s end end \
         local junk = {} for i = 1, 500 do junk[i] = {tostring(i)} end junk = nil \
         local n = 0 for i = 1, 50 do n = n + string.len(fs[i]()) end return n",
    );
    assert_eq!(r, "1275");
}

#[test]
fn weak_tables_drop_dead_entries() {
    let mut vm = benilla_lua::new_state();
    run(
        &mut vm,
        "wk = setmetatable({}, {__mode = 'k'}) wv = setmetatable({}, {__mode = 'v'}) \
         held = {} for i = 1, 10 do local k = {} wk[k] = i wv[i] = {} if i <= 3 then held[i] = k end end \
         wv[11] = 'strings stay'",
    );
    vm.full_gc();
    assert_eq!(
        run(&mut vm, "local a, b = 0, 0 for k in pairs(wk) do a = a + 1 end for k in pairs(wv) do b = b + 1 end return a .. ',' .. b .. ',' .. tostring(wv[11])"),
        "3,1,strings stay"
    );
}

#[test]
fn a_freed_string_slot_is_safe_to_reuse_as_a_key() {
    let mut vm = benilla_lua::new_state();
    // Keys die, their nodes stay with nil values; new strings take the slots.
    run(
        &mut vm,
        "t = {} for i = 1, 300 do t['key' .. i] = i end for i = 1, 300 do t['key' .. i] = nil end",
    );
    vm.full_gc();
    assert_eq!(
        run(&mut vm, "for i = 1, 300 do t['new' .. i] = i end local n = 0 for k, v in pairs(t) do n = n + v end return n .. ',' .. t.new150"),
        "45150,150"
    );
    let _ = Value::Nil;
}

/// Forks share their prototypes: a collection in one must not stop the other marking the
/// constants of the code they share.
#[test]
fn forks_sharing_code_each_keep_its_constants() {
    let mut a = benilla_lua::new_state();
    run(
        &mut a,
        "function f() return 'a constant string, longer than inline' .. 1 end",
    );
    let mut b = a.clone();
    for _ in 0..4 {
        a.full_gc();
        b.full_gc();
        // Churn, so a freed constant's slot is reused.
        run(&mut a, "for i = 1, 2000 do local s = 'x' .. i end");
        run(&mut b, "for i = 1, 2000 do local s = 'y' .. i end");
    }
    let want = "a constant string, longer than inline1";
    assert_eq!(run(&mut a, "return f()"), want);
    assert_eq!(run(&mut b, "return f()"), want);
}

/// The built-in `next` stays a root with no global naming it: a generic `for` steps a table in
/// place when its iterator is that function, so a new function taking its freed id would be
/// stepped as `next` instead of called.
#[test]
fn the_builtin_next_outlives_its_globals() {
    for mut vm in [
        benilla_lua::new_state(),
        benilla_lua::new_script_host_state(),
    ] {
        run(&mut vm, "next = nil pairs = nil ipairs = nil foreach = nil");
        vm.full_gc();
        assert_eq!(
            run(
                &mut vm,
                "local stubs = {} for i = 1, 2000 do stubs[i] = function(t, k) return nil end end \
                 local n = 0 for i = 1, 2000 do for k in stubs[i], {1, 2, 3} do n = n + 1 end end \
                 return n",
            ),
            "0"
        );
    }
}

/// A vararg call's `arg` table is an allocation like any other: a loop of nothing but vararg
/// calls collects as it goes (5.1.5's `adjust_varargs` checks the collector).
#[test]
fn vararg_calls_collect_as_they_go() {
    let mut vm = benilla_lua::new_state();
    run(
        &mut vm,
        "local function f(...) return arg.n end for i = 1, 200000 do f(i, i) end",
    );
    let tables = vm.heap.counts()[1];
    assert!(
        tables < 50000,
        "200000 `arg` tables should not wait for a collection, {tables} live"
    );
}

/// A table's growth counts toward the heap, as 5.1's `luaM_reallocvector` charges it: an array
/// filled to a hundred thousand slots shows in `gcinfo` before any collection.
#[test]
fn table_growth_counts_toward_the_heap() {
    let mut vm = benilla_lua::new_state();
    let kb = run(
        &mut vm,
        "T = {} for i = 1, 100000 do T[i] = i end return gcinfo()",
    );
    let kb: f64 = kb.parse().unwrap();
    assert!(
        kb > 1000.0,
        "a 100000-slot array is over a megabyte, gcinfo says {kb} KB"
    );
}

/// The handler convention: the globals hold the given values for the call alone, and their
/// earlier values come back after it, after an error too, even when the call collected.
#[test]
fn a_call_with_globals_puts_them_back() {
    let mut vm = benilla_lua::new_state();
    vm.gc_stress = true;
    run(&mut vm, "this = {name = 'outer'} event = nil");
    let f = vm
        .load(
            b"Seen = this.name .. ':' .. event .. ':' .. arg1 collectgarbage() error('boom')",
            b"=handler",
        )
        .unwrap();
    let hf = vm.root(f);
    let t = vm.exec(b"return {name = 'frame'}", b"=t").unwrap()[0];
    let held = [hf, vm.root(t)];
    let name = |vm: &mut benilla_lua::Vm, s: &[u8]| {
        let v = vm.str(s);
        vm.root(v);
        v
    };
    let (this, event, arg1, ev) = (
        name(&mut vm, b"this"),
        name(&mut vm, b"event"),
        name(&mut vm, b"arg1"),
        name(&mut vm, b"PLAYER_LOGIN"),
    );
    let sets = [
        (this.as_str().unwrap(), t),
        (event.as_str().unwrap(), ev),
        (arg1.as_str().unwrap(), Value::Num(7.0)),
    ];
    let r = vm.call_with_globals(f, &sets);
    assert!(r.is_err(), "the handler's error is the answer");
    assert_eq!(run(&mut vm, "return Seen"), "frame:PLAYER_LOGIN:7");
    assert_eq!(
        run(
            &mut vm,
            "return this.name .. tostring(event) .. tostring(arg1)"
        ),
        "outernilnil"
    );
    for h in held {
        vm.unroot(h);
    }
}

fn kb(vm: &mut benilla_lua::Vm) -> f64 {
    run(vm, "return gcinfo()").parse().unwrap()
}

/// The collector counts memory as C's allocator does, as it grows: a table's resizes, a
/// compiled chunk's code, the value stack. Garbage that grows after it is made still brings the
/// collection that frees it.
#[test]
fn the_count_follows_every_growth_and_paces_the_collector() {
    let mut vm = benilla_lua::new_state();
    vm.full_gc();
    let tables = vm.heap.counts()[1];
    run(
        &mut vm,
        "for i = 1, 300 do local t = {} for j = 1, 4000 do t[j] = j end end",
    );
    let left = vm.heap.counts()[1] - tables;
    assert!(left < 50, "{left} garbage tables of 64 KB still live");

    // One table grown with no other written after it: its growth is counted where the
    // collector checks, so the next table made collects it.
    vm.full_gc();
    let tables = vm.heap.counts()[1];
    run(
        &mut vm,
        "local g = {} for j = 1, 200000 do g[j] = j end g = nil local x = {}",
    );
    assert_eq!(
        vm.heap.counts()[1] - tables,
        1,
        "the grown table still live"
    );

    let before = kb(&mut vm);
    run(&mut vm, "Big = {} for j = 1, 100000 do Big[j] = j end");
    let grown = kb(&mut vm) - before;
    assert!(grown >= 1500.0, "a 100000-slot table counted as {grown} KB");

    run(
        &mut vm,
        "Src = 'local x = 0 ' .. string.rep('x = x + 1 ', 20000)",
    );
    let before = kb(&mut vm);
    run(&mut vm, "Chunk = loadstring(Src)");
    let compiled = kb(&mut vm) - before;
    assert!(
        compiled >= 100.0,
        "40000 instructions counted as {compiled} KB"
    );

    let before = kb(&mut vm);
    run(
        &mut vm,
        "local function r(n) if n > 0 then return 1 + r(n - 1) end return 0 end r(4000)",
    );
    let stack = kb(&mut vm) - before;
    assert!(
        stack >= 100.0,
        "a stack 4000 calls deep counted as {stack} KB"
    );
}

/// A snapshot counts its own tables' growth, and the state it was taken from does not.
#[test]
fn a_snapshot_counts_its_own_growth() {
    let mut vm = benilla_lua::new_state();
    run(&mut vm, "collectgarbage(-1) Big = {}");
    let mut snap = vm.clone();
    let (before, snap_before) = (kb(&mut vm), kb(&mut snap));
    run(&mut snap, "for j = 1, 100000 do Big[j] = j end");
    assert!(kb(&mut snap) - snap_before >= 1500.0);
    assert_eq!(kb(&mut vm), before, "the snapshot's growth counted here");
}

/// 5.1.5's dead keys: a collection marks a deleted string key dead, so setting it again takes a
/// node by `newkey`'s rules, and `pairs` walks in the C build's order; a number key is never
/// dead, and one equal to it keeps its bits.
#[test]
fn a_collected_deleted_key_is_set_again_as_a_new_one() {
    let mut vm = benilla_lua::new_state();
    let order = "local function order(t) local o = {} for k in pairs(t) do table.insert(o, k) end \
                 return table.concat(o, ' ') end ";
    let set = "local t = {} for i = 1, 5 do t['k' .. i] = i end t.k4 = nil ";
    assert_eq!(
        run(
            &mut vm,
            &format!("{order}{set} collectgarbage() t.k4 = 0 return order(t)")
        ),
        "k4 k5 k1 k3 k2"
    );
    assert_eq!(
        run(&mut vm, &format!("{order}{set} t.k4 = 0 return order(t)")),
        "k5 k1 k4 k3 k2"
    );
    assert_eq!(
        run(
            &mut vm,
            "local z = 0 local t = {} t[-z] = 1 t[0] = 2 local k = next(t) return 1 / k"
        ),
        "-inf"
    );
}
