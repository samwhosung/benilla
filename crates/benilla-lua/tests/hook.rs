//! The instruction-budget hook: it reaches every loop a script can write, and code it runs does
//! not run it again.

use std::cell::Cell;
use std::rc::Rc;

use benilla_lua::Vm;

fn run(vm: &mut Vm, src: &str) -> String {
    match vm.exec(src.as_bytes(), b"=t") {
        Ok(v) => v.first().map_or(String::new(), |&v| vm.display(v)),
        Err(e) => format!("<raised> {}", vm.display(e)),
    }
}

/// A hook that raises once it has run `limit` times.
fn budget(vm: &mut Vm, limit: u32) -> Rc<Cell<u32>> {
    let fired = Rc::new(Cell::new(0));
    let seen = fired.clone();
    vm.set_hook(
        1000,
        Rc::new(move |vm: &mut Vm| {
            seen.set(seen.get() + 1);
            if seen.get() >= limit {
                let v = vm.str(b"budget");
                return Err(vm.throw(v));
            }
            Ok(())
        }),
    );
    fired
}

/// On a 2 MB thread, as the brief's crash tests run.
fn small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn the_budget_stops_a_tail_recursive_loop() {
    small_stack(|| {
        let mut vm = benilla_lua::new_state();
        budget(&mut vm, 10);
        assert_eq!(
            run(
                &mut vm,
                "n = 0 local function f() n = n + 1 return f() end return f()"
            ),
            "<raised> budget"
        );
        // Ten runs of a 1000-instruction step, five instructions a call.
        let n = run(&mut vm, "return n").parse::<u32>().unwrap();
        assert!((1900..=2100).contains(&n), "{n} tail calls ran");
    });
}

#[test]
fn code_the_hook_runs_does_not_run_the_hook() {
    small_stack(|| {
        let mut vm = benilla_lua::new_state();
        run(
            &mut vm,
            "function Busy() local s = 0 for i = 1, 100000 do s = s + i end return s end",
        );
        let fired = Rc::new(Cell::new(0u32));
        let seen = fired.clone();
        vm.set_hook(
            100,
            Rc::new(move |vm: &mut Vm| {
                seen.set(seen.get() + 1);
                let busy = vm.get_global("Busy");
                vm.call_value(busy, &[], 1)?;
                Ok(())
            }),
        );
        assert_eq!(
            run(
                &mut vm,
                "local s = 0 for i = 1, 10000 do s = s + i end return s"
            ),
            "50005000"
        );
        // About one run per 100 instructions of the outer loop, none from `Busy`'s.
        let n = fired.get();
        assert!((10..=1000).contains(&n), "the hook ran {n} times");
    });
}

/// The instructions `src` runs, counted by a hook at every one.
fn instructions(src: &str) -> u64 {
    let mut vm = benilla_lua::new_state();
    let n = Rc::new(Cell::new(0u64));
    let seen = n.clone();
    vm.set_hook(
        1,
        Rc::new(move |_: &mut Vm| {
            seen.set(seen.get() + 1);
            Ok(())
        }),
    );
    let r = run(&mut vm, src);
    assert!(!r.starts_with("<raised>"), "{r}");
    n.get()
}

/// Each instruction a function runs counts, as 5.1's `LUA_MASKCOUNT` counts it: a call costs its
/// body, and Lua a library function calls (a `gsub` replacement, a `sort` comparator, `foreach`,
/// a metamethod, a generic `for`'s iterator) counts as any other.
#[test]
fn the_hook_counts_every_instruction_lua_runs() {
    // `F` runs two more instructions a call than `f`, its two moves.
    let fns = "local function f(x) return x end \
               local function F(x) local a = x local b = a return b end ";
    let it = "local function it(s, c) local x = c + 1 if x <= 100 then return x end end \
              local function IT(s, c) local y = c local z = y local x = z + 1 if x <= 100 then return x end end ";
    for (what, light, heavy, calls) in [
        (
            "a call",
            "for i = 1, 100 do f(i) end",
            "for i = 1, 100 do F(i) end",
            100,
        ),
        (
            "gsub",
            "string.gsub(string.rep('a', 100), 'a', f)",
            "string.gsub(string.rep('a', 100), 'a', F)",
            100,
        ),
        (
            "sort",
            "table.sort({3, 1, 2}, function(a, b) return f(a) < b end)",
            "table.sort({3, 1, 2}, function(a, b) return F(a) < b end)",
            0,
        ),
        (
            "foreach",
            "table.foreach({1, 2, 3, 4}, function(k, v) f(v) end)",
            "table.foreach({1, 2, 3, 4}, function(k, v) F(v) end)",
            4,
        ),
        (
            "__index",
            "local t = setmetatable({}, {__index = f}) for i = 1, 50 do local _ = t.x end",
            "local t = setmetatable({}, {__index = F}) for i = 1, 50 do local _ = t.x end",
            50,
        ),
        (
            "iterator",
            "for i in it, nil, 0 do end",
            "for i in IT, nil, 0 do end",
            101,
        ),
    ] {
        let (a, b) = (
            instructions(&format!("{fns}{it}{light}")),
            instructions(&format!("{fns}{it}{heavy}")),
        );
        assert!(b > a, "{what}: {a} instructions, then {b}");
        // `sort` calls its comparator as often as its algorithm needs: some number of times.
        if what == "sort" {
            assert_eq!((b - a) % 2, 0, "{what}: {a} instructions, then {b}");
        } else {
            assert_eq!(b - a, 2 * calls, "{what}: {a} instructions, then {b}");
        }
    }
}

/// A hook set by host code while Lua runs counts from the next loop back edge or call, as the
/// C build's counts from the next instruction: past the few instructions in between, the same.
#[test]
fn a_hook_set_while_lua_runs_counts_from_there() {
    // The second runs in the VM's slow loop throughout: its generic `for` reads registers it
    // has not written.
    for src in [
        "local s = 0 arm() for i = 1, 200 do if i > 100 then s = s + i else s = s - 1 end end \
         local function f(x) return x end arm() for i = 1, 100 do f(i) end",
        "for k in next, {1, 2}, nil, 1 do end local function g() local a = 1 end g() \
         local s = 0 arm() for i = 1, 200 do if i > 100 then s = s + i else s = s - 1 end end \
         local function f(x) return x end arm() for i = 1, 100 do f(i) end",
    ] {
        // The C build's count for each, recorded while it was in the tree, and one more for each
        // numeric `for` entered: the client's starts with a subtraction and a jump (`0x6fe42c`)
        // where 5.1 has one `FORPREP`.
        counts_from_the_arming(src, 1214);
    }
}

/// A loop whose back edge moves it to the VM's slow loop, a hook having been set, runs each
/// iteration once with its own index, whatever its step: the index is written before the jump.
#[test]
fn a_loop_moved_by_a_hook_keeps_its_index() {
    for (src, sum) in [
        (
            "arm() local s = 0 for i = 10, 1, -1 do s = s + i end return s",
            "55",
        ),
        (
            "arm() local s = 0 for i = 1, 20, 2 do s = s + i end return s",
            "100",
        ),
        (
            "arm() local s = 0 for i = 1, 10 do s = s + i end return s",
            "55",
        ),
    ] {
        let mut vm = benilla_lua::new_state();
        let arm = vm.new_host(
            "arm",
            Rc::new(|vm: &mut Vm, _| {
                vm.set_hook(1 << 40, Rc::new(|_: &mut Vm| Ok(())));
                Ok(0)
            }),
        );
        vm.set_global("arm", arm);
        assert_eq!(run(&mut vm, src), sum, "{src}");
    }
}

fn counts_from_the_arming(src: &str, c: u64) {
    let ours = {
        let mut vm = benilla_lua::new_state();
        let n = Rc::new(Cell::new(0u64));
        let seen = n.clone();
        let arm = vm.new_host(
            "arm",
            Rc::new(move |vm: &mut Vm, _| {
                let seen = seen.clone();
                vm.set_hook(
                    1,
                    Rc::new(move |_: &mut Vm| {
                        seen.set(seen.get() + 1);
                        Ok(())
                    }),
                );
                Ok(0)
            }),
        );
        vm.set_global("arm", arm);
        let r = run(&mut vm, src);
        assert!(!r.starts_with("<raised>"), "{r}");
        n.get()
    };
    assert!(
        ours <= c && c - ours <= 10,
        "{src}: ours {ours}, the C build's {c}"
    );
}

/// The count at every instruction is the C build's: an instruction 5.1 never dispatches is not
/// counted (the jump a skipping `LOADBOOL` makes, the `LOADNIL` its frame clearing stands for, a
/// constant loaded into a scratch register), whichever way the code branches.
#[test]
fn the_count_is_the_c_builds() {
    // The C build's count for each, recorded while it was in the tree.
    for (src, c) in [
        ("local x, y = 1, 2 local b = x < y", 5),
        ("local x, y = 2, 1 local b = x < y", 5),
        ("local x, y = 1, 2 for i = 1, 10 do local b = x < y local c = y < x end", 59),
        ("table.sort({5, 3, 1, 4, 2, 9, 7}, function(a, b) return a < b end)", 62),
        ("local function f() local a, b return a end for i = 1, 10 do f() end", 48),
        ("local b = 'a' < 'b'", 3),
        ("local x = 'a' .. 'b'", 4),
        ("local x = 1 local y = x == 1", 4),
        ("local t = {1, 2, 3} for k, v in pairs(t) do end", 14),
        ("local t = {1, 2, 3} for k in next, t do end", 14),
        ("local function it(s, c) if c < 5 then return c + 1 end end for i in it, nil, 0 do end", 29),
        ("local t = {} for i = 1, 5 do t[i] = i end local s = 0 for i = 5, 1, -1 do s = s + t[i] end", 40),
        ("local a = nil local b = a and 1 or 2 local c = a or 3 local d = not a", 6),
        ("local function f(...) return arg.n end f(1, 2, 3)", 9),
        ("local t = {1, 2, 3, x = 1, y = 2}", 8),
        ("local x = 0 while x < 5 do x = x + 1 end repeat x = x - 1 until x == 0", 28),
        ("local n = 0 for i = 1, 20 do if math.mod(i, 3) == 0 then n = n + 1 elseif i > 10 then n = n - 1 else n = n * 2 end end", 195),
        ("string.gsub('abcabc', 'b', function(c) return c == 'b' and 'x' or c end)", 15),
        ("local t = setmetatable({}, {__index = function(t, k) return k > 2 end}) for i = 1, 5 do local _ = t[i] end", 38),
    ] {
        assert_eq!(instructions(src), c, "{src}");
    }
}

/// A hook that aborts once it has run `limit` times ([`Vm::abort`]).
fn abort_budget(vm: &mut Vm, limit: u32) {
    let seen = Rc::new(Cell::new(0));
    vm.set_hook(
        1000,
        Rc::new(move |vm: &mut Vm| {
            seen.set(seen.get() + 1);
            if seen.get() >= limit {
                return Err(vm.abort(b"budget"));
            }
            Ok(())
        }),
    );
}

/// An abort unwinds past every `pcall` and `xpcall` to the host, runs no handler, and ends
/// there: the next run is ordinary.
#[test]
fn an_abort_is_caught_by_no_protected_call() {
    small_stack(|| {
        let mut vm = benilla_lua::new_state();
        let cases = [
            "while true do pcall(function() while true do end end) end",
            "while true do local ok, e = pcall(function() while true do end end) error(e) end",
            "handled = nil xpcall(function() while true do end end, function() handled = true \
             while true do end end)",
            "xpcall(function() error('x') end, function() while true do end end)",
            "local function f() return pcall(f) end while true do f() end",
        ];
        for src in cases {
            abort_budget(&mut vm, 10);
            assert_eq!(run(&mut vm, src), "<raised> budget", "{src}");
        }
        vm.remove_hook();
        assert_eq!(run(&mut vm, "return handled"), "nil");
        assert_eq!(
            run(&mut vm, "local ok, e = pcall(error, 'e') return e"),
            "e",
            "a later error is caught as ever"
        );
    });
}

/// A handler fired from inside Lua while an abort unwinds still gets its globals put back, raw
/// past a Lua `__newindex` that would raise the abort again.
#[test]
fn an_abort_leaves_the_handler_globals_restored() {
    fn fire(vm: &mut Vm, _nargs: usize) -> Result<usize, benilla_lua::LuaError> {
        let f = vm.get_global("Handler");
        let this = vm.str(b"this");
        let v = vm.str(b"inner");
        let r = vm.call_with_globals(f, &[(this.as_str().unwrap(), v)]);
        Ok(vm.ret1(benilla_lua::Value::Bool(r.is_ok())))
    }
    small_stack(|| {
        let mut vm = benilla_lua::new_state();
        vm.register("Fire", fire);
        run(
            &mut vm,
            "local store = {this = 'outer'} Handler = function() while true do end end \
             setmetatable(getfenv(0), {__index = store, \
               __newindex = function(t, k, v) k = tostring(k) store[k] = v end})",
        );
        abort_budget(&mut vm, 10);
        assert_eq!(run(&mut vm, "Fire() return 1"), "<raised> budget");
        vm.remove_hook();
        assert_eq!(run(&mut vm, "return this"), "outer");
    });
}
