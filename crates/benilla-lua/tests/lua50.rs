//! Where benilla-lua's 5.1 runs as the 1.12.1 client's Lua 5.0 does, in what a script can see:
//! each case names the client's code it follows.

fn run(src: &str) -> String {
    let mut vm = benilla_lua::new_state();
    match vm.exec(src.as_bytes(), b"=t") {
        Ok(v) => v
            .iter()
            .map(|&v| vm.display(v))
            .collect::<Vec<_>>()
            .join(","),
        Err(e) => format!("<raised> {}", vm.display(e)),
    }
}

/// `^` on two numbers calls the global `__pow`, read raw (`0x6f9b5e`), with the operands made
/// numbers; one that is not a function raises (`0x6f9b72`). A constant `^` calls it too.
#[test]
fn pow_calls_the_global_pow() {
    assert_eq!(run("return 2 ^ 10"), "1024");
    assert_eq!(
        run("__pow = function(a, b) return type(a) .. type(b) .. a .. b end return 2 ^ 3, '2' ^ 3"),
        "numbernumber23,numbernumber23"
    );
    assert_eq!(
        run("__pow = nil return 2 ^ 3"),
        "<raised> t:1: `__pow' (`^' operator) is not a function"
    );
    assert_eq!(
        run(
            "setmetatable(_G, {__index = function() return math.pow end}) __pow = nil return 2 ^ 3"
        ),
        "<raised> t:1: `__pow' (`^' operator) is not a function"
    );
    // A table operand goes to its own `__pow` metamethod, never the global.
    assert_eq!(
        run("__pow = nil local t = setmetatable({}, {__pow = function() return 'mm' end}) return t ^ 2"),
        "mm"
    );
}

fn compile_error(src: &str) -> String {
    benilla_lua::check(src.as_bytes(), b"=t")
        .err()
        .map_or("ok".into(), |e| e.to_string())
}

/// The client's limits (`luaX_checklimit`, `0x6ff440`), each a syntax error near the current
/// token: 32 upvalues (`0x6fdfdc`), 100 parameters with `self` and without `arg` (`0x6fdac7`),
/// 200 locals (`0x6fd8fc`), 262143 list items in a constructor (`0x6fd7aa`).
#[test]
fn parser_limits_are_the_clients() {
    let names = |n: usize| {
        (1..=n)
            .map(|i| format!("v{i}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let ups = |n: usize| format!("local {}\nf = function() return {} end", names(n), names(n));
    assert_eq!(compile_error(&ups(32)), "ok");
    assert_eq!(
        compile_error(&ups(33)),
        "t:2: too many upvalues (limit=32) near `end'"
    );
    let params = |pre: &str, n: usize, post: &str| {
        compile_error(&format!("function {pre}({}{post}) end", names(n)))
    };
    assert_eq!(params("f", 100, ""), "ok");
    assert_eq!(params("f", 100, ", ..."), "ok");
    assert_eq!(
        params("f", 101, ""),
        "t:1: too many parameters (limit=100) near `)'"
    );
    assert_eq!(
        params("t:m", 100, ""),
        "t:1: too many parameters (limit=100) near `)'"
    );
    assert_eq!(
        compile_error(&format!("local {}", names(201))),
        "t:1: too many local variables (limit=200) near `<eof>'"
    );
    let list = |n: usize| format!("return {{{}}}", "0,".repeat(n));
    assert_eq!(compile_error(&list(262144)), "ok");
    assert_eq!(
        compile_error(&list(262145)),
        "t:1: too many items in a constructor (limit=262143) near `,'"
    );
}

/// Syntax levels are counted in each chunk from 0 (`0x6fc9d0`), by `chunk` and `subexpr`
/// (`0x6fcc99`, `0x6fd180`): the chunk and its `return` expression take two, each parenthesis
/// one more, and the host calls the compile runs under take none.
#[test]
fn syntax_levels_are_counted_per_chunk() {
    let parens = |n: usize| format!("return {}1{}", "(".repeat(n), ")".repeat(n));
    assert_eq!(compile_error(&parens(198)), "ok");
    assert_eq!(
        compile_error(&parens(199)),
        "t:1: too many syntax levels near `1'"
    );
    let src = format!(
        "local src = {:?} \
         local function r(n) if n == 0 then return loadstring(src) ~= nil end \
         local ok, v = pcall(r, n - 1) return v end return r(190)",
        parens(198)
    );
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || assert_eq!(run(&src), "true"))
        .unwrap()
        .join()
        .unwrap();
}

/// A numeric `for`'s variable is its counter (`fornum`, `0x6fe380`): `OP_FORLOOP` (`0x6f9262`)
/// steps what the body left in it, refuses a non-number there, not coerced, and makes the limit
/// and step numbers each pass. The start minus the step is a plain subtraction (`0x6fe42c`).
#[test]
fn a_numeric_for_steps_its_own_variable() {
    assert_eq!(
        run(
            "local o = {} for i = 1, 5 do table.insert(o, i) i = i + 1 end \
             return table.concat(o, ' ')"
        ),
        "1 3 5"
    );
    assert_eq!(
        run(
            "local o = {} for i = 10, 1, -1 do table.insert(o, i) if i == 8 then i = 3 end end \
             return table.concat(o, ' ')"
        ),
        "10 9 8 2 1"
    );
    assert_eq!(
        run("for i = 1, 2 do i = nil end"),
        "<raised> t:1: `for' initial value must be a number"
    );
    assert_eq!(
        run("for i = 1, 2 do i = '1' end"),
        "<raised> t:1: `for' initial value must be a number"
    );
    assert_eq!(
        run("for i = 1, 'x' do end"),
        "<raised> t:1: `for' limit must be a number"
    );
    assert_eq!(
        run("local n = 0 for i = '1', '3', '1' do n = n + i end return n"),
        "6"
    );
    assert_eq!(
        run("for i = 'a', 2 do end"),
        "<raised> t:1: attempt to perform arithmetic on a string value"
    );
    assert_eq!(
        run(
            "local t = setmetatable({}, {__sub = function() return 10 end}) \
             local o = {} for i = t, 12 do table.insert(o, i) end return table.concat(o, ' ')"
        ),
        "11 12"
    );
}

/// Each loop variable is one local for the whole loop (`forbody`, `0x6fe480`, declares them at
/// `0x6fe493` before it enters the loop's block at `0x6fe4ab`): closures made in the body share
/// it and see its value when the loop ends, nil for a generic `for` (`OP_TFORLOOP` writes the
/// generator's nil into it, `0x6f9445`) and the last counter for a numeric one, also after a
/// `break`. A body local is fresh each pass.
#[test]
fn closures_in_a_for_share_its_variables() {
    assert_eq!(
        run(
            "local fs = {} for i = 1, 3 do fs[i] = function() return i end end \
             return fs[1](), fs[2](), fs[3]()"
        ),
        "3,3,3"
    );
    assert_eq!(
        run("local fs = {} for k, v in ipairs({'a', 'b'}) do fs[k] = function() return k, v end end \
             return fs[1]()"),
        "nil,nil"
    );
    assert_eq!(
        run("local f for k, v in pairs({x = 1}) do f = function() return k end end return f()"),
        "nil"
    );
    assert_eq!(
        run("local fs = {} for i = 1, 10 do fs[i] = function() return i end if i == 2 then break end end \
             return fs[1](), fs[2]()"),
        "2,2"
    );
    assert_eq!(
        run(
            "local fs = {} for k, v in ipairs({'a', 'b', 'c'}) do fs[k] = function() return v end \
             if k == 2 then break end end return fs[1](), fs[2]()"
        ),
        "b,b"
    );
    // The upvalue is closed when the loop ends: the slot's later use does not reach it.
    assert_eq!(
        run("local f for i = 1, 2 do f = function() return i end end local x = 'later' return f()"),
        "2"
    );
    assert_eq!(
        run("local fs = {} for k, v in ipairs({'a', 'b'}) do local w = v fs[k] = function() return w end end \
             return fs[1](), fs[2]()"),
        "a,b"
    );
}

/// The generator a generic `for` calls is a copy above its variables (`0x6f9345`): calling a
/// nil one names no variable.
#[test]
fn a_nil_generator_is_named_nothing() {
    assert_eq!(
        run("local t = {} for k in t.f do end"),
        "<raised> t:1: attempt to call a nil value"
    );
}

/// A generic `for`'s first variable is its control (`forlist`, `0x6fe540`): the body may move it,
/// and `OP_TFORLOOP` calls the generator from above the variables (`0x6f9344`), so a closure the
/// body made reads them as the body left them while the generator runs. Two hidden locals and 198
/// names make the 200 a function may have.
#[test]
fn a_generic_for_steps_from_its_first_variable() {
    assert_eq!(
        run("local o = {} local t = {'a', 'b', 'c', 'd'} \
             for k, v in next, t do table.insert(o, v) if k == 1 then k = 3 end end \
             return table.concat(o, ' ')"),
        "a d"
    );
    assert_eq!(
        run("local seen = {} local peek \
             local function gen(s, c) if peek then table.insert(seen, tostring(peek())) end \
             if c < 3 then return c + 1, c * 10 end end \
             for i, v in gen, nil, 0 do peek = function() return i .. ':' .. v end end \
             return table.concat(seen, ' ')"),
        "1:0 2:10 3:20"
    );
    let names = |n: usize| {
        (1..=n)
            .map(|i| format!("k{i}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    assert_eq!(
        compile_error(&format!("for {} in x do end", names(198))),
        "ok"
    );
    assert_eq!(
        compile_error(&format!("for {} in x do end", names(199))),
        "t:1: too many local variables (limit=200) near `in'"
    );
}

/// `getfuncname` names a function only an `OP_CALL` or `OP_TAILCALL` called (`0x6fc499`..
/// `0x6fc4a1`): a library function failing as a generic `for`'s generator is `?`.
#[test]
fn a_generator_is_not_named_by_its_loop() {
    assert_eq!(
        run("for k in next, 5 do end"),
        "<raised> t:1: bad argument #1 to `?' (table expected, got number)"
    );
}

/// `code_params` (`0x6fdab0`) checks the parameters before it makes `...`'s `arg`; an assignment
/// has no limit of its own, only the registers' (`luaK_checkstack`, `0x700a63`): 249 targets of
/// one value fit, 250 do not.
#[test]
fn parameters_and_assignments_fail_where_the_client_does() {
    let names = |n: usize| {
        (1..=n)
            .map(|i| format!("p{i}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    assert_eq!(
        compile_error(&format!("function f({}, ...) end", names(200))),
        "t:1: too many parameters (limit=100) near `)'"
    );
    assert_eq!(
        compile_error(&format!("function f({}, ...) end", names(100))),
        "ok"
    );
    assert_eq!(compile_error(&format!("{} = 1", names(249))), "ok");
    assert_eq!(
        compile_error(&format!("{} = 1", names(250))),
        "t:1: function or expression too complex near `<eof>'"
    );
}

/// A generic `for` with more than three names takes its results back from above its variables
/// (`0x6f9445`), past the three registers `forlist` checks (`0x6fe5e7`).
#[test]
fn a_generic_for_with_many_names_runs() {
    assert_eq!(
        run(
            "local t = {} for a, b, c, d, e in string.gfind('abcde', '(%a)(%a)(%a)(%a)(%a)') do \
             t[a] = e end return t.a"
        ),
        "e"
    );
    let names = |n: usize| (1..=n).map(|i| format!("v{i}")).collect::<Vec<_>>();
    for n in [6, 8] {
        let vs = names(n);
        let src = format!(
            "local function gen(s, c) if not c then return {} end end \
             local r for {} in gen do r = {} end return r",
            (1..=n)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            vs.join(", "),
            vs.join(" .. "),
        );
        let want: String = (1..=n).map(|i| i.to_string()).collect();
        assert_eq!(run(&src), want, "{n} names");
    }
}
