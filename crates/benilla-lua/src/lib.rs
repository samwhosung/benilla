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
pub use lib_base::{object_text, tostring_value};
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

/// A VM opened as the client's `FrameScript` opens its state (`0x7039e0`), every global in the
/// client's order, so the globals and the four library tables lay out as the client's: the frame
/// metatable `__framescript_meta` (empty here; the host fills it), the 36 functions of the base
/// array (`0x811e28`), the `string`, `table`, `math` and `bit` libraries (`0x703af5`..`0x703b16`),
/// then the compat chunk (`0x703b26`). A base function only the host can give
/// (`seterrorhandler`, `date`, `debugstack` and the like) is a placeholder that raises, which the
/// host replaces: a new value under a key keeps the layout. There is no `_G`: an addon reaches the
/// globals by `getfenv(0)`.
pub fn new_state() -> Vm {
    let mut vm = Vm::new();
    // `InitLua` sets 256 KB right after `lua_open` (`0x7039f2`), before any library.
    vm.set_gc_threshold(0x100);
    let meta = vm.heap.new_table(0, 0);
    vm.set_global("__framescript_meta", Value::Table(meta));
    lib_base::open(&mut vm, not_in_the_host);
    lib_string::open(&mut vm);
    lib_table::open(&mut vm);
    lib_math::open(&mut vm);
    vm.exec(COMPAT, b"=[benilla-lua compat]")
        .expect("the compat chunk runs");
    vm
}

/// A base function the host gives; until it does, a call raises.
fn not_in_the_host(vm: &mut Vm, _nargs: usize) -> api::R<usize> {
    Err(vm.rt_error("not available in the script host"))
}

/// The bare globals the client's start-up chunk publishes (its `.rdata` at `0x8722e8`): aliases
/// of the table, math and string libraries, and the trigonometry in degrees, one global at a time
/// in the chunk's order, which the globals' layout follows. Written here, not copied; Lua closures
/// as in the client, so a bad argument names the inner call.
const COMPAT: &[u8] = br#"
local t, m, s = table, math, string
foreach = t.foreach
foreachi = t.foreachi
getn = t.getn
tinsert = t.insert
tremove = t.remove
sort = t.sort
abs = m.abs
function acos(x) return m.deg(m.acos(x)) end
function asin(x) return m.deg(m.asin(x)) end
function atan(x) return m.deg(m.atan(x)) end
function atan2(y, x) return m.deg(m.atan2(y, x)) end
ceil = m.ceil
function cos(x) return m.cos(m.rad(x)) end
deg = m.deg
exp = m.exp
floor = m.floor
frexp = m.frexp
ldexp = m.ldexp
log = m.log
log10 = m.log10
max = m.max
min = m.min
mod = m.mod
PI = m.pi
rad = m.rad
random = m.random
randomseed = m.randomseed
function sin(x) return m.sin(m.rad(x)) end
sqrt = m.sqrt
function tan(x) return m.tan(m.rad(x)) end
strbyte = s.byte
strchar = s.char
strfind = s.find
format = s.format
gsub = s.gsub
strlen = s.len
strlower = s.lower
strrep = s.rep
strsub = s.sub
strupper = s.upper
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
        // `[` then `=` is the token `[` (`0x6ff793`).
        assert_eq!(err("x = [=x"), "t:1: unexpected symbol near `['");
        // The numeral stops at the `x`, which then opens a statement.
        assert_eq!(err("x = 3x"), "t:1: `=' expected near `<eof>'");
        assert_eq!(err("x = 3e"), "t:1: malformed number near `3e'");
    }

    /// The client's long brackets, line ends and numerals: `[[` nests and has no `=` levels,
    /// `\n` alone ends a line, and `0x10` is 0 then the name `x10`.
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
        assert_eq!(one(b"return [[=[a]=]]"), b"=[a]=");
        // No levels (`0x6ff793`, `read_long_string` `0x700010`): `[=[` is `[` then `=`, and
        // `--[==[` a line comment, so the line after it is code (`0x6ff6c9`).
        assert_eq!(err("x = [=[a]=]"), "t:1: unexpected symbol near `['");
        assert_eq!(err("x = [==[a]==]"), "t:1: unexpected symbol near `['");
        assert_eq!(
            err("--[==[\nx = 1\n]==]"),
            "t:3: unexpected symbol near `]'"
        );
        assert!(check(b"--[==[ x = 1 ]==]", b"=t").is_ok());
        assert!(check(b"--[[ a [[ b ]] c ]] x = 1", b"=t").is_ok());
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
