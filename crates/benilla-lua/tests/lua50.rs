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
