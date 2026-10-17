//! benilla-lua: an interpreter for the Lua benilla's script host runs, Lua 5.1.5 patched to the
//! 1.12 grammar (`GRAMMAR.md`). The state is plain Rust data, so it can be
//! cloned, inspected and restored.

pub mod api;
mod cfmt;
mod compile;
pub mod heap;
mod lex;
mod lib_base;
mod lib_math;
mod lib_string;
mod lib_table;
mod number;
mod parse;
mod pools;
pub mod proto;
pub mod reload;
pub mod table;
pub mod value;
pub mod vm;

pub use compile::compile;
pub use lex::SyntaxError;
pub use lib_base::tostring_value;
pub use lib_math::{bit_library, math_pow};
pub use number::fmt_g14;
pub use value::Value;
pub use vm::{LuaError, Vm};

/// `%.<precision><conv>` of a `double`, `conv` one of `e E f g G`, as the client's MSVC runtime
/// writes it (`_cfltcvt` `0x40b7fe` under `sprintf` and `_snprintf`), for host code that formats a
/// number as one of the client's C functions does.
pub fn crt_float(v: f64, conv: u8, precision: Option<usize>) -> String {
    cfmt::fmt_float(
        v,
        conv,
        &cfmt::Spec {
            precision,
            ..Default::default()
        },
    )
}

/// A VM with 1.12's Lua libraries open: base, `string`, `table`, `math` and `bit`, then the
/// bare compatibility globals the client defines over them.
pub fn new_state() -> Vm {
    let mut vm = Vm::new();
    // `InitLua` sets 256 KB right after `lua_open` (`0x7039f2`), before any library.
    vm.set_gc_threshold(0x100);
    lib_base::open(&mut vm);
    lib_string::open(&mut vm);
    lib_table::open(&mut vm);
    lib_math::open(&mut vm);
    vm.exec(COMPAT, b"=[benilla-lua compat]")
        .expect("the compat chunk runs");
    vm
}

/// The state benilla's script host starts from: Lua 5.1's standard libraries as the C build's
/// mlua `Lua::new()` opened them (`luaopen_base`, then table, io, os, string, math and package),
/// each table sized and filled in its C registration order. The script host's sandbox then deletes
/// what 1.12 lacks, and the deleted keys stay in the tables as 5.1 leaves them, so every table
/// lays out, and iterates, as it did. A function 5.1 has and benilla does not is a placeholder
/// that raises: the sandbox deletes every one.
pub fn new_script_host_state() -> Vm {
    let mut vm = Vm::new();
    let g = Value::Table(vm.globals);
    // `base_open`.
    vm.set_global("_G", g);
    for name in [
        "assert",
        "collectgarbage",
        "dofile",
        "error",
        "gcinfo",
        "getfenv",
        "getmetatable",
        "loadfile",
        "load",
        "loadstring",
        "next",
        "pcall",
        "print",
        "rawequal",
        "rawget",
        "rawset",
        "select",
        "setfenv",
        "setmetatable",
        "tonumber",
        "tostring",
        "type",
        "unpack",
        "xpcall",
    ] {
        let f = vm.new_native(name, lib_base::native(name).unwrap_or(not_in_the_host));
        vm.set_global(name, f);
    }
    let version = vm.str(b"Lua 5.1");
    vm.set_global("_VERSION", version);
    let aux = vm.new_native("ipairs_aux", lib_base::ipairs_aux);
    let ipairs = vm.new_closure("ipairs", lib_base::ipairs, vec![aux]);
    vm.set_global("ipairs", ipairs);
    let next = vm.get_global("next");
    vm.builtin_next = next;
    let pairs = vm.new_closure("pairs", lib_base::pairs_51, vec![next]);
    vm.set_global("pairs", pairs);
    let newproxy = vm.new_native("newproxy", not_in_the_host);
    vm.set_global("newproxy", newproxy);
    register_51(
        &mut vm,
        "coroutine",
        &["create", "resume", "running", "status", "wrap", "yield"],
        |_| None,
    );
    register_51(
        &mut vm,
        "table",
        &[
            "concat", "foreach", "foreachi", "getn", "maxn", "insert", "remove", "setn", "sort",
        ],
        lib_table::native,
    );
    register_51(
        &mut vm,
        "io",
        &[
            "close", "flush", "input", "lines", "open", "output", "popen", "read", "tmpfile",
            "type", "write",
        ],
        |_| None,
    );
    register_51(
        &mut vm,
        "os",
        &[
            "clock",
            "date",
            "difftime",
            "execute",
            "exit",
            "getenv",
            "remove",
            "rename",
            "setlocale",
            "time",
            "tmpname",
        ],
        |_| None,
    );
    // `luaopen_string`, with `LUA_COMPAT_GFIND`'s `gfind` the same function as `gmatch`.
    let string = register_51(
        &mut vm,
        "string",
        &[
            "byte", "char", "dump", "find", "format", "gfind", "gmatch", "gsub", "len", "lower",
            "match", "rep", "reverse", "sub", "upper",
        ],
        |name| lib_string::native(if name == "gmatch" { "gfind" } else { name }),
    );
    set_field(&mut vm, string, b"gfind", |vm, t| {
        get_field(vm, t, b"gmatch")
    });
    // `luaopen_math`, with `pi`, `huge` and `LUA_COMPAT_MOD`'s `mod` the same function as `fmod`.
    let math = register_51(
        &mut vm,
        "math",
        &[
            "abs",
            "acos",
            "asin",
            "atan2",
            "atan",
            "ceil",
            "cosh",
            "cos",
            "deg",
            "exp",
            "floor",
            "fmod",
            "frexp",
            "ldexp",
            "log10",
            "log",
            "max",
            "min",
            "modf",
            "pow",
            "rad",
            "random",
            "randomseed",
            "sinh",
            "sin",
            "sqrt",
            "tanh",
            "tan",
        ],
        lib_math::native,
    );
    set_field(&mut vm, math, b"pi", |_, _| {
        Value::Num(std::f64::consts::PI)
    });
    set_field(&mut vm, math, b"huge", |_, _| Value::Num(f64::INFINITY));
    set_field(&mut vm, math, b"mod", |vm, t| get_field(vm, t, b"fmod"));
    // `luaopen_package`: its table, then `module` and `require` into the globals.
    register_51(&mut vm, "package", &["loadlib", "seeall"], |_| None);
    for name in ["module", "require"] {
        let f = vm.new_native(name, not_in_the_host);
        vm.set_global(name, f);
    }
    vm
}

/// A 5.1 function benilla does not implement; the script host deletes each one.
fn not_in_the_host(vm: &mut Vm, _nargs: usize) -> api::R<usize> {
    Err(vm.rt_error("not available in the script host"))
}

/// `luaL_register`: a global table with room for every entry, the entries set in order.
fn register_51(
    vm: &mut Vm,
    lib: &str,
    names: &[&'static str],
    native: fn(&str) -> Option<heap::NativeFn>,
) -> value::TableRef {
    let t = vm.heap.new_table(0, names.len());
    vm.set_global(lib, Value::Table(t));
    for &name in names {
        let f = vm.new_native(name, native(name).unwrap_or(not_in_the_host));
        let k = vm.str(name.as_bytes());
        vm.heap.table_mut(t).set(k, f).unwrap();
    }
    t
}

fn get_field(vm: &mut Vm, t: value::TableRef, k: &[u8]) -> Value {
    let k = vm.str(k);
    vm.heap.table(t).get(k)
}

fn set_field(
    vm: &mut Vm,
    t: value::TableRef,
    k: &[u8],
    v: impl FnOnce(&mut Vm, value::TableRef) -> Value,
) {
    let v = v(vm, t);
    let k = vm.str(k);
    vm.heap.table_mut(t).set(k, v).unwrap();
}

/// The bare globals the client's start-up chunk publishes (its `.rdata` at `0x8722e8`): aliases
/// of the table, math and string libraries, and the trigonometry in degrees. Written here, not
/// copied; Lua closures as in the client, so a bad argument names the inner call.
const COMPAT: &[u8] = br#"
local t, m, s = table, math, string
foreach, foreachi, getn = t.foreach, t.foreachi, t.getn
tinsert, tremove, sort = t.insert, t.remove, t.sort
abs, ceil, floor, deg, rad = m.abs, m.ceil, m.floor, m.deg, m.rad
exp, log, log10, frexp, ldexp = m.exp, m.log, m.log10, m.frexp, m.ldexp
max, min, mod, sqrt, PI = m.max, m.min, m.mod, m.sqrt, m.pi
random, randomseed = m.random, m.randomseed
function sin(x) return m.sin(m.rad(x)) end
function cos(x) return m.cos(m.rad(x)) end
function tan(x) return m.tan(m.rad(x)) end
function asin(x) return m.deg(m.asin(x)) end
function acos(x) return m.deg(m.acos(x)) end
function atan(x) return m.deg(m.atan(x)) end
function atan2(y, x) return m.deg(m.atan2(y, x)) end
strbyte, strchar, strfind, format, gsub = s.byte, s.char, s.find, s.format, s.gsub
strlen, strlower, strrep, strsub, strupper = s.len, s.lower, s.rep, s.sub, s.upper
"#;

/// The client's `luaO_chunkid` (`0x6f5c40`): a chunk name as a message shows it, in a buffer of
/// `bufflen` bytes, 80 (`MAXSRC`) for compile errors and 60 (`LUA_IDSIZE`) for runtime ones.
/// `=name` is verbatim, `@file` keeps the file's tail, and anything else is
/// `[string "first line..."]`.
pub fn chunkid_n(source: &[u8], bufflen: usize) -> Vec<u8> {
    // A C string: the name ends at its first NUL.
    let source = &source[..source.iter().position(|&b| b == 0).unwrap_or(source.len())];
    match source.first() {
        Some(b'=') => {
            let s = &source[1..];
            s[..s.len().min(bufflen - 1)].to_vec()
        }
        Some(b'@') => {
            let s = &source[1..];
            let room = bufflen - b" `...' ".len() - 1;
            if s.len() > room {
                let mut out = b"...".to_vec();
                out.extend_from_slice(&s[s.len() - room..]);
                out
            } else {
                s.to_vec()
            }
        }
        _ => {
            let room = bufflen - b" [string \"...\"] ".len() - 1;
            // The client's `strcspn(source, "\n")` (`0x6f5cd1`, the set at `0x835144`): the
            // first line, ended by a `\n` alone.
            let line = source
                .iter()
                .position(|&b| b == b'\n')
                .unwrap_or(source.len());
            let len = line.min(room);
            let mut out = b"[string \"".to_vec();
            if len < source.len() {
                out.extend_from_slice(&source[..len]);
                out.extend_from_slice(b"...");
            } else {
                out.extend_from_slice(source);
            }
            out.extend_from_slice(b"\"]");
            out
        }
    }
}

/// [`chunkid_n`] at the compiler's 80 bytes.
pub fn chunkid(source: &[u8]) -> Vec<u8> {
    chunkid_n(source, 80)
}

/// Compile a chunk and drop it, or answer the client's own syntax error.
pub fn check(src: &[u8], chunkname: &[u8]) -> Result<(), SyntaxError> {
    compile::compile(src, chunkname, &mut heap::Heap::default()).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(src: &str) -> String {
        check(src.as_bytes(), b"=t").unwrap_err().to_string()
    }

    /// The lexer's and parser's messages, as `tests/vm_parity.golden` in benilla-ui records them.
    #[test]
    fn syntax_errors_are_the_script_hosts() {
        assert_eq!(err("x = 1 +"), "t:1: unexpected symbol near `<eof>'");
        assert_eq!(err("local function"), "t:1: `<name>' expected near `<eof>'");
        assert_eq!(err("x = #t"), "t:1: unexpected symbol near `#'");
        // `x = a` ends; `%` then opens a statement.
        assert_eq!(err("x = a % b"), "t:1: unexpected symbol near `%'");
        assert_eq!(
            err("return function(...) return ... end"),
            "t:1: unexpected symbol near `...'"
        );
        assert_eq!(err("for x do end"), "t:1: `=' or `in' expected near `do'");
        assert_eq!(
            err("x = 1..2"),
            "t:1: ambiguous syntax (decimal point x string concatenation) near `1..'"
        );
        assert_eq!(err("x = \"a\nb\""), "t:1: unfinished string near `\"a'");
        assert_eq!(err("x = [[a"), "t:1: unfinished long string near `<eof>'");
        assert_eq!(err("x = 1\x01"), "t:1: invalid control char near `char(1)'");
        assert_eq!(
            err("f()\n(g)()"),
            "t:2: ambiguous syntax (function call x new statement) near `('"
        );
        assert_eq!(err("break"), "t:1: no loop to break near `<eof>'");
        assert_eq!(err("(a) = 1"), "t:1: syntax error near `='");
        assert_eq!(err("if x then\nend end"), "t:2: <eof> expected near `end'");
        assert_eq!(
            err("do\nx = 1"),
            "t:2: `end' expected (to close `do' at line 1) near `<eof>'"
        );
        assert_eq!(
            err("x = [=x"),
            "t:1: invalid long string delimiter near `[='"
        );
        // The numeral stops at the `x`, which then opens a statement.
        assert_eq!(err("x = 3x"), "t:1: `=' expected near `<eof>'");
        assert_eq!(err("x = 3e"), "t:1: malformed number near `3e'");
    }

    /// 5.1's lexer with level-0 nesting and the client's line ends and numerals: `\n` alone ends a
    /// line, `[=[` has levels, and `0x10` is 0 then the name `x10`.
    #[test]
    fn the_lexer_is_the_script_hosts() {
        // The string constant a chunk returns.
        let one = |src: &[u8]| -> Vec<u8> {
            let mut heap = heap::Heap::default();
            let p = compile::compile(src, b"=t", &mut heap).unwrap();
            let value::V::Str(s) = p.consts[0].unpack() else {
                panic!("{:?}", p.consts)
            };
            heap.str(s).to_vec()
        };
        assert_eq!(one(b"return [[a\r\nb]]"), b"a\r\nb", "a \\r is a byte");
        assert_eq!(one(b"return [[a [[b]] c]]"), b"a [[b]] c");
        assert_eq!(one(b"return [=[a]]b]=]"), b"a]]b");
        assert!(check(b"x = 'a\rb'", b"=t").is_ok());
        assert_eq!(err("x = 0x10"), "t:1: `=' expected near `<eof>'");
        assert!(check(b"a=1b=2", b"=t").is_ok());
        assert_eq!(err("x = 1\ry = +"), "t:1: unexpected symbol near `+'");
        assert_eq!(err("x = 1\r\ny = +"), "t:2: unexpected symbol near `+'");
    }

    /// C reads a chunk name as a C string (lua5.1 prints the same with 5.1's quotes).
    #[test]
    fn a_chunk_name_stops_at_a_nul() {
        let e = |src: &[u8], name: &[u8]| check(src, name).unwrap_err().to_string();
        assert_eq!(e(b"x = +", b"=a\0b"), "a:1: unexpected symbol near `+'");
        assert_eq!(
            e(b"x = \0", b"x = \0"),
            "[string \"x = \"]:1: invalid control char near `char(0)'"
        );
        assert_eq!(
            e(b"x = +\0", b"x = +\0"),
            "[string \"x = +\"]:1: unexpected symbol near `+'"
        );
    }

    #[test]
    fn chunk_names_are_5_0_s() {
        assert_eq!(chunkid(b"=name"), b"name");
        assert_eq!(chunkid(b"@a/b.lua"), b"a/b.lua");
        assert_eq!(chunkid(b"x = 1\ny = 2"), b"[string \"x = 1...\"]");
        assert_eq!(chunkid(b"x = 1"), b"[string \"x = 1\"]");
        // A `\r` does not end the line (`0x6f5cd1`).
        assert_eq!(chunkid(b"x = 1\ry = 2"), b"[string \"x = 1\ry = 2\"]");
        assert_eq!(chunkid(b"\rx\ny"), b"[string \"\rx...\"]");
        let long = [b'a'; 100];
        assert_eq!(chunkid_n(&long, 60).len(), 9 + 43 + 3 + 2);
    }
}
