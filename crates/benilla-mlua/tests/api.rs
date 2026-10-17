//! The API answers as mlua 0.11's does over the C Lua build, for the calls benilla makes.

use benilla_mlua::{Function, Lua};

/// `Debug::function` is the function running at the level: `lua_getinfo(L, "f", ar)`.
#[test]
fn a_stack_level_names_its_function() {
    let lua = Lua::new();
    let seen = lua
        .create_function(|lua, ()| {
            let mine = lua.inspect_stack(0, |d| d.function()).unwrap();
            let caller = lua.inspect_stack(1, |d| d.function()).unwrap();
            Ok((mine, caller))
        })
        .unwrap();
    lua.globals().set("seen", seen.clone()).unwrap();
    let (mine, caller, f): (Function, Function, Function) = lua
        .load("function F() local a, b = seen() return a, b, F end return F()")
        .eval()
        .unwrap();
    assert_eq!(mine, seen);
    assert_eq!(caller, f);
}

/// An index the host makes with no Lua running raises a plain type error, as `lua_gettable`
/// does with no function to name a variable in.
#[test]
fn a_host_index_through_a_bad_index_metamethod_raises() {
    let lua = Lua::new();
    lua.load("setmetatable(getfenv(0), {__index = true})")
        .exec()
        .unwrap();
    let err = lua.globals().get::<benilla_mlua::Value>("x").unwrap_err();
    assert!(
        err.to_string().contains("attempt to index a boolean value"),
        "{err}"
    );
}

/// A number reads as `Integer` when `num_traits::cast` to `i64` round-trips its bits, as mlua
/// reads a 5.1 number: `2^63` is past `i64` and `-0` is not `0`.
#[test]
fn numbers_read_as_mlua_reads_them() {
    use benilla_mlua::Value;
    let lua = Lua::new();
    let (a, b, c, d): (Value, Value, Value, Value) = lua
        .load("return 9223372036854775808, -9223372036854775808, -(0), 9007199254740992")
        .eval()
        .unwrap();
    assert!(
        matches!(a, Value::Number(n) if n == 9_223_372_036_854_775_808.0),
        "{a:?}"
    );
    assert!(matches!(b, Value::Integer(i64::MIN)), "{b:?}");
    assert!(
        matches!(c, Value::Number(n) if n == 0.0 && n.is_sign_negative()),
        "{c:?}"
    );
    assert!(matches!(d, Value::Integer(9_007_199_254_740_992)), "{d:?}");

    let int = |v: Value| lua.coerce_integer(v).unwrap();
    assert_eq!(int(Value::Integer(5)), Some(5));
    assert_eq!(int(Value::Number(1e-17)), Some(0), "within EPSILON of 0");
    assert_eq!(int(Value::Number(0.5)), None);
    assert_eq!(int(Value::Number(-3.0)), Some(-3));
    assert_eq!(
        int(Value::String(lua.create_string(" 7 ").unwrap())),
        Some(7)
    );
    assert_eq!(int(Value::Boolean(true)), None);
}

/// `len`, `push` and `pop` go through `__len`, `__index` and `__newindex` when the table has a
/// metatable, as mlua's do over `luaL_len`.
#[test]
fn length_push_and_pop_use_the_metatable() {
    use benilla_mlua::Table;
    let lua = Lua::new();
    let t: Table = lua
        .load(
            "return setmetatable({1, 2, 3}, {__len = function() return 10 end, \
             __newindex = function(t, k, v) Log = k rawset(t, k, v) end})",
        )
        .eval()
        .unwrap();
    assert_eq!(t.len().unwrap(), 10);
    assert_eq!(t.raw_len(), 3);
    t.push("x").unwrap();
    assert_eq!(lua.globals().get::<i64>("Log").unwrap(), 11);
    let bad: Table = lua
        .load("return setmetatable({}, {__len = function() return 2.5 end})")
        .eval()
        .unwrap();
    assert_eq!(
        bad.len().unwrap_err().to_string(),
        "runtime error: object length is not an integer"
    );
    let empty: Table = lua
        .load("return setmetatable({}, {__index = function(t, k) return 'idx' .. k end})")
        .eval()
        .unwrap();
    assert_eq!(empty.pop::<String>().unwrap(), "idx0");
    let plain = lua.create_table().unwrap();
    assert_eq!(plain.pop::<Option<i64>>().unwrap(), None);
}

/// `raw_insert` and `raw_remove` take an index in the sequence and shift the rest, as mlua's.
#[test]
fn raw_insert_and_remove_shift_within_bounds() {
    let lua = Lua::new();
    let t = lua.create_sequence_from([1, 2, 3]).unwrap();
    assert!(t.raw_insert(0, 9).is_err());
    assert!(t.raw_insert(5, 9).is_err());
    t.raw_insert(1, 0).unwrap();
    t.raw_insert(5, 4).unwrap();
    assert!(t.raw_remove(6).is_err());
    t.raw_remove(2).unwrap();
    let got: Vec<i64> = t.sequence_values().map(Result::unwrap).collect();
    assert_eq!(got, [0, 2, 3, 4]);
    t.raw_set("k", 1).unwrap();
    t.raw_remove("k").unwrap();
    assert_eq!(t.raw_get::<Option<i64>>("k").unwrap(), None);
}

/// A table made from an iterator starts empty and grows by rehash, as the client's C code builds
/// one: five values make an array of eight, and five integer pairs move into it too, the sizes
/// the client gives `t = {} t[1] = 1 ... t[5] = 5`.
#[test]
fn a_table_from_an_iterator_grows_from_empty() {
    let lua = Lua::new();
    let seq = lua.create_sequence_from([1, 2, 3, 4, 5]).unwrap();
    let map = lua
        .create_table_from([(1, "a"), (2, "b"), (3, "c"), (4, "d"), (5, "e")])
        .unwrap();
    lua.globals().set("Seq", seq).unwrap();
    lua.globals().set("Map", map).unwrap();
    lua.with_vm(|vm| {
        let seq = vm.get_global("Seq").as_table().unwrap();
        let map = vm.get_global("Map").as_table().unwrap();
        assert_eq!(vm.heap.table(seq).array.len(), 8);
        assert_eq!(
            (vm.heap.table(map).array.len(), vm.heap.table(map).lsizenode),
            (8, 0)
        );
    });
}

/// Every chunk compiles as source, whatever its mode (`luaD_protectedparser` zeroes the binary
/// flag, `0x6f6a15`): a binary chunk meets the lexer's control-byte error (`0x6ffabb`).
#[test]
fn a_binary_chunk_is_lexed_as_source() {
    let lua = Lua::new();
    for mode in [None, Some(benilla_mlua::ChunkMode::Text)] {
        let mut chunk = lua.load(&b"\x1bLuaP\x01\x04"[..]).set_name("=bin");
        if let Some(m) = mode {
            chunk = chunk.set_mode(m);
        }
        let Err(benilla_mlua::Error::SyntaxError { message, .. }) = chunk.into_function() else {
            panic!("a binary chunk compiled");
        };
        assert_eq!(message, "bin:1: invalid control char near `char(27)'");
    }
}
