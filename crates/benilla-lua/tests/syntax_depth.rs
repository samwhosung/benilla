//! Long and deep source on a 2 MB thread: a left-nested chain of any length compiles and runs,
//! as 5.1's parser reads it in a loop, and deep nesting stops at the client's level limit with its
//! message (or, for loops, at its limit on locals). Neither may overflow the Rust stack.

fn on_small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

/// The chunk's first result, or its error.
fn run(src: String) -> String {
    let mut vm = benilla_lua::new_state();
    vm.exec(
        b"function f() return f end m = { m = function(self) return self end }",
        b"=setup",
    )
    .unwrap();
    match vm.exec(src.as_bytes(), b"=t") {
        Ok(v) => v.first().map_or("none".into(), |&v| vm.display(v)),
        Err(e) => vm.display(e),
    }
}

#[test]
fn left_nested_chains_compile_at_any_length() {
    on_small_stack(|| {
        let cases = [
            (
                format!("local x = f{} return type(x)", "()".repeat(30000)),
                "function",
            ),
            (
                format!("local x = 1{} return x", " and 1".repeat(40000)),
                "1",
            ),
            (format!("return m{}", ":m()".repeat(20000)), "table"),
            (format!("return 1{}", "+1".repeat(200000)), "200001"),
            (
                format!("return {}false", "false or ".repeat(49999)),
                "false",
            ),
            (
                format!("local t = {{}} t.a = t return t{} == t", ".a".repeat(50000)),
                "true",
            ),
            (
                format!("local x = 2 return x{}", "*x".repeat(30000)),
                "1.#INF",
            ),
            (format!("return 1 == 1{}", " == true".repeat(30000)), "true"),
        ];
        for (src, want) in cases {
            assert_eq!(run(src.clone()), want, "{}", &src[..40]);
        }
    });
}

#[test]
fn deep_nesting_stops_at_the_level_limit() {
    on_small_stack(|| {
        let n = 100000;
        let cases = [
            format!("return {}1{}", "(".repeat(n), ")".repeat(n)),
            format!("return {}1", "not ".repeat(n)),
            format!("return {}1", "- ".repeat(n)),
            format!("return 1{}", " .. 1".repeat(n)),
            format!("return 2{}", " ^ 2".repeat(n)),
            format!("return {}1{}", "{".repeat(n), "}".repeat(n)),
            format!("return {}1{}", "f(".repeat(n), ")".repeat(n)),
            format!("return {}1{}", "m[".repeat(n), "]".repeat(n)),
            format!("{}{}", "do ".repeat(n), "end ".repeat(n)),
            format!("{}{}", "if x then ".repeat(n), "end ".repeat(n)),
            format!("{}{}", "while x do ".repeat(n), "end ".repeat(n)),
            format!("{}{}", "repeat ".repeat(n), "until x ".repeat(n)),
            format!("{}{}", "for i = 1, 2 do ".repeat(n), "end ".repeat(n)),
            format!("{}{}", "for k in x do ".repeat(n), "end ".repeat(n)),
            format!(
                "return {}1{}",
                "function() return ".repeat(n),
                " end".repeat(n)
            ),
            format!("{}{}", "local function g() ".repeat(n), "end ".repeat(n)),
        ];
        for src in cases {
            let got = run(src.clone());
            // Four locals a loop reach the 200 before the levels do.
            let locals =
                src.starts_with("for") && got.contains("too many local variables (limit=200)");
            assert!(
                got.contains("too many syntax levels near") || locals,
                "{}: {got}",
                &src[..40]
            );
        }
    });
}
