//! The libraries where the 1.12.1 client's own code answers otherwise than 5.1's: each case is
//! the client's answer, read at the address its test names.

/// Run `src` in the script host's state; its results as text, or the error.
fn run(src: &str) -> Result<Vec<String>, String> {
    let mut vm = benilla_lua::new_state();
    match vm.exec(src.as_bytes(), b"=t") {
        Ok(vs) => Ok(vs.into_iter().map(|v| vm.display(v)).collect()),
        Err(e) => Err(vm.display(e)),
    }
}

fn ok(src: &str) -> Vec<String> {
    run(src).unwrap_or_else(|e| panic!("{src}: {e}"))
}

/// How many values `call` answers.
fn count(call: &str) -> Vec<String> {
    ok(&format!(
        "local function n(...) return arg.n end return n({call})"
    ))
}

/// `str_byte` (`0x7fc210`): one position, one byte or none; a third argument is not read.
#[test]
fn string_byte_answers_one_byte() {
    assert_eq!(ok(r#"return string.byte("abc", 1, 3)"#), ["97"]);
    assert_eq!(ok(r#"return string.byte("abc")"#), ["97"]);
    assert_eq!(ok(r#"return string.byte("abc", -1)"#), ["99"]);
    assert_eq!(ok(r#"return string.byte("abc", 2.9)"#), ["98"]);
    // `__ftol`'s low dword: 4294967296 + 2 is position 2.
    assert_eq!(ok(r#"return string.byte("abc", 4294967298)"#), ["98"]);
    for pos in ["0", "4", "-4", "1/0"] {
        assert_eq!(
            count(&format!("string.byte('abc', {pos})")),
            ["0"],
            "position {pos}"
        );
    }
    assert_eq!(count("string.byte('')"), ["0"]);
}

/// `str_gsub` (`0x7fd0e0`) and `add_s` (`0x7fd2a0`): a string or a function replacement, every
/// `%digit` a checked capture, and a function's result added only when it is a string.
#[test]
fn gsub_replaces_as_5_0() {
    assert_eq!(
        ok(
            r#"return string.gsub("a1b2", "%d", function(d) if d == "1" then return "one" end end)"#
        ),
        ["aoneb", "2"]
    );
    for result in ["false", "true", "{}", "nil"] {
        assert_eq!(
            ok(&format!(
                r#"return string.gsub("a1b", "%d", function() return {result} end)"#
            )),
            ["ab", "1"],
            "a {result} result drops the match"
        );
    }
    assert_eq!(
        ok(r#"return string.gsub("a1", "%d", function() return 7 end)"#),
        ["a7", "1"]
    );
    assert_eq!(
        ok(r#"return string.gsub("abc", "(b)", "[%1]")"#),
        ["a[b]c", "1"]
    );
    assert_eq!(
        ok(r#"return string.gsub("abc", "b", "%%%x")"#),
        ["a%xc", "1"]
    );
    assert_eq!(ok(r#"return string.gsub("abc", "b", 5)"#), ["a5c", "1"]);
    for repl in [r#""%0""#, r#""%1""#] {
        assert_eq!(
            run(&format!(r#"return string.gsub("abc", "b", {repl})"#)),
            Err("t:1: invalid capture index".into()),
            "{repl}"
        );
    }
    assert_eq!(
        run(r#"return string.gsub("abc", "(b)", "%2")"#),
        Err("t:1: invalid capture index".into())
    );
    for repl in ["{b = 'x'}", "nil", "true"] {
        assert_eq!(
            run(&format!(r#"return string.gsub("abc", "b", {repl})"#)),
            Err("t:1: bad argument #3 to `gsub' (string or function expected)".into()),
            "{repl}"
        );
    }
    assert_eq!(
        run(r#"return string.gsub("abc", "b")"#),
        Err("t:1: bad argument #3 to `gsub' (string or function expected)".into())
    );
}

/// `str_lower` (`0x7fbdf0`) and `str_upper` (`0x7fc0c0`) case-map UTF-8 code points: ASCII and
/// U+00C0-U+00FE, nothing else; an invalid sequence is dropped with the byte that broke it.
#[test]
fn lower_and_upper_map_utf8_code_points() {
    let q = |call: &str| ok(&format!("return string.format('%q', {call})"));
    assert_eq!(q("string.lower('ÉCOLE Ä')"), ["\"école ä\""]);
    assert_eq!(q("string.upper('école ä')"), ["\"ÉCOLE Ä\""]);
    // U+00D7 and U+00F7 sit inside the ranges and swap; U+00DF and U+00FF sit outside them.
    assert_eq!(q("string.lower('×ß')"), ["\"÷ß\""]);
    assert_eq!(q("string.upper('÷ÿ')"), ["\"×ÿ\""]);
    // Cyrillic is outside both ranges.
    assert_eq!(q("string.lower('ДОМ')"), ["\"ДОМ\""]);
    // A Latin-1 byte leads a two-byte sequence that `a` breaks: both go.
    assert_eq!(q(r#"string.lower("\201a!")"#), ["\"!\""]);
    // A lone continuation byte, and 0xfe, decode to nothing.
    assert_eq!(q(r#"string.upper("a\128b\254c")"#), ["\"ABC\""]);
    // A sequence cut by the end of the string goes.
    assert_eq!(q(r#"string.lower("A\195")"#), ["\"a\""]);
    // An overlong NUL comes back as a NUL byte; a five-byte sequence above 0x3fffff as six bytes.
    assert_eq!(q(r#"string.lower("\192\128A")"#), ["\"\\000a\""]);
    assert_eq!(
        ok(r#"return string.len(string.lower("\249\128\128\128\128"))"#),
        ["6"]
    );
    // A NUL ends the result, where the client never returns.
    assert_eq!(q(r#"string.lower("AB\0CD")"#), ["\"ab\""]);
    assert_eq!(q(r#"string.upper("\195\0x")"#), ["\"\""]);
}

/// `tonumber` with a base (`0x702e30`) parses with the client's 32-bit `strtoul` (`0x40dd0e`), on
/// every platform: an overflow is 0xffffffff, a `-` negates modulo 4294967296.
#[test]
fn tonumber_with_a_base_is_32_bit() {
    assert_eq!(
        ok(r#"return tonumber("fffffffff", 16), tonumber("-1", 16), tonumber("-fffffffff", 16)"#),
        ["4294967295", "4294967295", "1"]
    );
    assert_eq!(
        ok(r#"return tonumber("ffffffff", 16), tonumber("-80000000", 16), tonumber("z", 36)"#),
        ["4294967295", "2147483648", "35"]
    );
    assert_eq!(
        ok(r#"return tonumber(" 0xff ", 16), tonumber("10", 16.9), tonumber("ff\0zz", 16)"#),
        ["255", "16", "255"]
    );
    assert_eq!(
        ok(r#"return tonumber("0x", 16), tonumber("8", 8), tonumber("1 1", 2)"#),
        ["nil", "nil", "nil"]
    );
}

/// Number text is the client's MSVC runtime's (`luaV_tostring` `0x6f7c80`, `sprintf` `0x73fec4`):
/// `1#INF` and its kin, three exponent digits, no sign on a zero, and rounding by digit text.
#[test]
fn number_text_is_msvcs() {
    assert_eq!(
        ok("local z = 0 return tostring(1/z), tostring(-1/z), tostring(-z)"),
        ["1.#INF", "-1.#INF", "0"]
    );
    assert_eq!(
        ok("return 1e20 .. '', 1e-5 .. '', tostring(123456789012345)"),
        ["1e+020", "1e-005", "1.2345678901235e+014"]
    );
    assert_eq!(
        ok(r#"return string.format("%e|%.2f|%5.1f|%g|%.0f", 12345.678, 1/0, -1/0, -1/0, 2.5)"#),
        ["1.234568e+004|1.#J| -1.$|-1.#INF|3"]
    );
}

/// `str_format` (`0x7fd3d0`) scans as Lua 4.0's: a C string, flags without a count, 18 bytes of
/// spec at most, a one-digit `n$` that moves the cursor, and its own error texts.
#[test]
fn format_scans_as_4_0() {
    assert_eq!(ok(r#"return string.format("a\0%d", 1)"#), ["a"]);
    assert_eq!(ok(r#"return string.format("%-----5d|", 3)"#), ["3    |"]);
    assert_eq!(
        ok(r#"return string.format("%------------------d", 3)"#),
        ["3"]
    );
    assert_eq!(
        run(r#"return string.format("%-------------------d", 3)"#),
        Err("t:1: invalid format (width or precision too long)".into())
    );
    assert_eq!(
        ok(r#"return string.len(string.format("%.s", string.rep("x", 100)))"#),
        ["100"]
    );
    assert_eq!(
        ok(r#"return string.format("%.0s|%5.s|", string.rep("x", 100), "ab")"#),
        ["|     |"]
    );
    assert_eq!(
        ok(r#"return string.format("%2$s %s %1$s", "a", "b", "c")"#),
        ["b c a"]
    );
    assert_eq!(
        run(r#"return string.format("%d")"#),
        Err("t:1: bad argument #2 to `format' (number expected, got no value)".into())
    );
    assert_eq!(
        run(r#"return string.format("%y", 1)"#),
        Err("t:1: invalid option in `format'".into())
    );
    assert_eq!(
        run(r#"return string.format("%", 1)"#),
        Err("t:1: invalid option in `format'".into())
    );
}

/// `str_format`'s integer conversions take `__ftol`'s low 32 bits (`0x7fd56a`, `0x7fd593`), and
/// `%q` escapes only `"`, `\`, a newline and a NUL (`luaI_addquoted`, table `0x7fd7b0`).
#[test]
fn format_converts_at_32_bits() {
    assert_eq!(
        ok(r#"return string.format("%x %X %d %u %o", -1, 4294967551, 4294967296, -1, -1)"#),
        ["ffffffff FF 0 4294967295 37777777777"]
    );
    assert_eq!(
        ok(r#"return string.format("%d %d %i", 2147483648, 1/0, -2.9)"#),
        ["-2147483648 0 -2"]
    );
    assert_eq!(ok(r#"return string.format("%c", 4294967361)"#), ["A"]);
    assert_eq!(
        ok(r#"return string.format("%q", "a\"b\\c\nd\re\0f")"#),
        ["\"a\\\"b\\\\c\\\nd\re\\000f\""]
    );
}

/// `$I10_OUTPUT` (`0x4116fd`) rounds an 18th digit of 5 and zeros by its own 96-bit arithmetic:
/// down on the first value, up on the second.
#[test]
fn number_digits_round_as_the_clients_arithmetic() {
    let mut vm = benilla_lua::new_state();
    for (bits, want) in [
        (0x4317_1eee_124d_37a7u64, "1.6269830806113057e+015"),
        (0x4319_1f38_6560_ea63, "1.7678003743115768e+015"),
    ] {
        vm.set_global("V", benilla_lua::Value::Num(f64::from_bits(bits)));
        let got = vm
            .exec(br#"return string.format("%.16e", V)"#, b"=t")
            .unwrap();
        assert_eq!(vm.display(got[0]), want, "{bits:#x}");
    }
}

/// The client's `sprintf` (`0x73fec4`) zero-pads `%s` and `%c` for the `0` flag.
#[test]
fn format_zero_pads_strings_and_chars() {
    assert_eq!(
        ok(r#"return string.format("%05s|%-05s|%05.1s|%03c|%-03c|", "ab", "ab", "ab", 65, 65)"#),
        ["000ab|ab   |0000a|00A|A  |"]
    );
}

/// A NaN an operation makes is the x87's real indefinite, `-1.#IND`, on every host; a NaN that
/// comes in keeps its own sign through arithmetic and the math library.
#[test]
fn a_made_nan_is_the_x87_indefinite() {
    let made = [
        "0/z",
        "(1/z) - (1/z)",
        "(1/z) + (-1/z)",
        "z * (1/z)",
        "math.sqrt(-1)",
        "math.asin(2)",
        "math.acos(2)",
        "math.log(-1)",
        "math.log10(-3)",
        "math.mod(1, 0)",
        "math.sin(1/z)",
        "math.pow(-8, 1/3)",
        "math.acos(0/z)",
        "math.cos(0/z)",
    ];
    for expr in made {
        assert_eq!(
            ok(&format!("local z = 0 return tostring({expr})")),
            ["-1.#IND"],
            "{expr}"
        );
    }
    let mut vm = benilla_lua::new_state();
    vm.set_global(
        "V",
        benilla_lua::Value::Num(f64::from_bits(0x7ff8_0000_0000_0000)),
    );
    let got = vm
        .exec(
            b"return tostring(V), tostring(V + 1), tostring(2 * V), tostring(math.sqrt(V)), \
              tostring(-V), tostring(math.acos(V)), tostring(math.cos(-V)), \
              tostring(math.mod(2, -V)), tostring(math.mod(-V, V)), tostring(math.mod(V, -V))",
            b"=t",
        )
        .unwrap();
    let got: Vec<String> = got.into_iter().map(|v| vm.display(v)).collect();
    // Of two NaNs the client's x87 answers the positive one.
    assert_eq!(
        got,
        [
            "1.#QNAN", "1.#QNAN", "1.#QNAN", "1.#QNAN", "-1.#IND", "1.#QNAN", "-1.#IND", "-1.#IND",
            "1.#QNAN", "1.#QNAN"
        ]
    );
}

/// Where the client's runtime makes a NaN or a zero that C's does not: `exp` of an infinity
/// (`0x7fb11c`), `pow` of `±1^±inf` (`0x744cdc`) or with a NaN operand (`0x73fa0c`), `fmod` by an
/// infinity (`0x743ad6`) and of `-0` (`0x743a38`), and `frexp` of a zero, an infinity or a NaN
/// (`0x7fd8db`).
#[test]
fn the_math_library_answers_the_clients_specials() {
    let mut vm = benilla_lua::new_state();
    vm.set_global(
        "V",
        benilla_lua::Value::Num(f64::from_bits(0x7ff8_0000_0000_0000)),
    );
    let got = vm
        .exec(
            b"local z = 0 local inf, nz = 1/z, -z \
              local fi, ei = math.frexp(inf) local fn, en = math.frexp(V) \
              local f0, e0 = math.frexp(nz) \
              return math.exp(inf), math.exp(-inf), math.pow(1, inf), math.pow(-1, -inf), \
                math.pow(1, V), math.pow(V, 0), math.pow(V, nz), math.mod(5, inf), \
                math.mod(-5, -inf), 1/math.mod(nz, 2), math.mod(nz, inf), fi, ei, fn, en, 1/f0, e0",
            b"=t",
        )
        .unwrap();
    let got: Vec<String> = got.into_iter().map(|v| vm.display(v)).collect();
    assert_eq!(
        got,
        [
            "-1.#IND", "-1.#IND", "-1.#IND", "-1.#IND", "1.#QNAN", "1.#QNAN", "1.#QNAN", "-1.#IND",
            "-1.#IND", "1.#INF", "-1.#IND", "-1.#IND", "-1", "1.#QNAN", "-1", "1.#INF", "0"
        ]
    );
}

/// 5.0's `getfenv` and `setfenv` (`0x702ac0`, `0x702be0`): an environment's raw `__fenv` field
/// (`aux_getfenv`, `0x702ba0`) is what `getfenv` answers and makes `setfenv` refuse, a metafield
/// is not read, and `setfenv` answers nothing.
#[test]
fn fenv_protection_is_the_clients() {
    assert_eq!(
        ok("local function f() return x end
            local env = {x = 1, __fenv = 'locked'}
            setfenv(f, env)
            local ok, e = pcall(setfenv, f, {})
            return getfenv(f), ok, e, f()"),
        [
            "locked",
            "false",
            "`setfenv' cannot change a protected environment",
            "1"
        ]
    );
    assert_eq!(
        ok("local function f() end
            local env = setmetatable({}, {__fenv = 1})
            local function count(...) return arg.n end
            local n = count(setfenv(f, env))
            return getfenv(f) == env, n, pcall(setfenv, f, {})"),
        ["true", "0", "true"]
    );
}

/// `math_random` (`0x7fb310`) over MSVC's `rand` (`0x7400e5`): from `srand(1)` the CRT's own
/// first draws 41, 18467, 6334, 26500, 19169, each `% 32767` times `1/32767` (`0x822d38`), an
/// interval's low end added to the `int` floor. A new state is seeded from the clock (`0x5b800b`).
#[test]
fn math_random_is_msvcs_rand() {
    let draws = |call: &str| {
        ok(&format!(
            "math.randomseed(1) local f = math.{call} return f(), f(), f(), f(), f()"
        ))
    };
    assert_eq!(
        draws("random"),
        [
            "0.0012512588885159",
            "0.56358531449324",
            "0.19330423902097",
            "0.80874050111393",
            "0.58500930814539"
        ]
    );
    assert_eq!(
        ok("math.randomseed(1) local r = math.random \
            return r(100), r(100), r(100), r(-5, 1000000), r(-5, 1000000)"),
        ["1", "57", "20", "808740", "585007"]
    );
    // Every draw is on the 1/32767 grid.
    assert_eq!(
        ok("for i = 1, 1000 do local x = math.random() * 32767 \
              if math.abs(x - math.floor(x + 0.5)) > 1e-6 then return i end end return 0"),
        ["0"]
    );
    // An unseeded state does not draw `srand(1)`'s sequence.
    assert_ne!(
        ok("local r = math.random return r(32767), r(32767), r(32767), r(32767), r(32767)"),
        ["42", "18468", "6335", "26501", "19170"]
    );
}

/// 5.0's `ipairs` (`0x702cd0`), each answer the client's own, run in its opened state: a loop's
/// start answers the global `ipairs`, the table and 0; a second argument, even nil, steps once;
/// a loop keeps the generator it started with, which is the global read at its start.
#[test]
fn ipairs_answers_as_the_clients() {
    assert_eq!(
        ok("local t = {10, 20, 30}
            local f, s, i = ipairs(t)
            local k, v = ipairs(t, 0)
            local k2, v2 = ipairs(t, 2)
            local k4, v4 = ipairs(t, nil)
            return f == ipairs, s == t, i, k, v, k2, v2, ipairs(t, 3) == nil, k4, v4"),
        ["true", "true", "0", "1", "10", "3", "30", "true", "1", "10"]
    );
    // `__ftol` (`0x40a2b0`) keeps the low dword of the truncation: 2^32 + 1 and -2^32 + 1 read
    // `t[1]`.
    assert_eq!(
        ok("local a, b = ipairs({10, 20}, 2^32)
            return a, b, ipairs({10, 20}, -2^32)"),
        ["4294967297", "10", "-4294967295", "10"]
    );
    assert_eq!(
        ok("local t, n, orig = {10, 20, 30}, 0, ipairs
            for i, v in ipairs(t) do n = n + v if i == 1 then ipairs = function() end end end
            ipairs = orig
            local seen = 0
            ipairs = function(t, i) if i == nil then return orig(t) end seen = seen + 1 return orig(t, i) end
            for i, v in orig(t) do end
            ipairs = orig
            return n, seen"),
        ["60", "4"]
    );
}

/// `setglobal` (`0x702770`) sets the global to the top value, so a lone name sets it to itself,
/// and it and `getglobal` (`0x7027b0`) go through the globals' metamethods; each answer the
/// client's own, run in its opened state.
#[test]
fn setglobal_sets_the_top_value_as_the_clients() {
    assert_eq!(
        ok(
            "setglobal('X1') setglobal('X2', 5) setglobal('X3', 5, 6) setglobal('X4', nil)
            X5 = 1 setglobal('X5')
            return X1, X2, X3, X4, X5, getglobal('X2')"
        ),
        ["X1", "5", "6", "nil", "X5", "5"]
    );
    assert_eq!(
        ok(
            "setmetatable(getfenv(0), {__newindex = function(t, k, v) rawset(t, k, 'ni') end,
                                      __index = function(t, k) return 'idx' end})
            setglobal('Z2', 7)
            local z1 = getglobal('nothere')
            setmetatable(getfenv(0), nil)
            return rawget(getfenv(0), 'Z2'), z1"
        ),
        ["ni", "idx"]
    );
}
