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
