//! The libraries where the 1.12.1 client's own code answers otherwise than 5.1's: each case is
//! the client's answer, read at the address its test names.

/// Run `src` in the script host's state; its results as text, or the error.
fn run(src: &str) -> Result<Vec<String>, String> {
    let mut vm = benilla_lua::new_script_host_state();
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
    // `__ftol`'s low dword: 2^32 + 2 is position 2.
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
