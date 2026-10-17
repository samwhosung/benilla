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
