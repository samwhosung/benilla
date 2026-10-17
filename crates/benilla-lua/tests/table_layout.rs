//! `pairs` order, which a table's layout decides, against the client's: each expected order and
//! size is what the client's own `ltable.c` (`0x6fa1a0`..`0x6fad80`) gives for the same chunk,
//! run in its Lua core (`lua_open` `0x6f6d20`, `luaL_loadbuffer` `0x6f5690`, `lua_next`
//! `0x6f4450`) under benilla-re's Unicorn harness.
//!
//! The rules the orders follow: a hash part of `n` keys has `2^(log2(n)+1)` nodes, the
//! constructor's `luaO_log2(nh) + 1` (`0x6fd563`) and `rehash`'s (`0x6fa9e5`); `newkey`
//! (`0x6fa8a0`) writes the key, takes free nodes from the top down, and rehashes once the last one
//! is taken (`0x6fa98c`), the new key moved with the rest; the array part is the largest `2^n`
//! whose integer keys reach half of it (`computesizes`, `0x6fab45`).

use benilla_lua::value::V;
use benilla_lua::Value;

/// The keys of the global `t` after `src` runs, in `next` order, with its array size and log
/// node count.
fn layout(src: &str) -> (String, usize, u8) {
    let mut vm = benilla_lua::new_state();
    vm.exec(src.as_bytes(), b"=layout").expect("the chunk runs");
    let V::Table(t) = vm.get_global("t").unpack() else {
        panic!("t is a table")
    };
    let mut keys = Vec::new();
    let mut k = Value::Nil;
    while let Some((nk, _)) = vm.heap.table(t).next(k).unwrap() {
        keys.push(match nk.unpack() {
            V::Str(s) => String::from_utf8_lossy(vm.heap.str(s)).into_owned(),
            V::Num(n) => format!("{n}"),
            V::Bool(b) => b.to_string(),
            _ => "?".into(),
        });
        k = nk;
    }
    let tab = vm.heap.table(t);
    (keys.join(" "), tab.array.len(), tab.lsizenode)
}

/// `t.s1 = 1` to `t.sN = N`, one by one.
fn one_by_one(n: usize) -> String {
    let mut s = String::from("t = {}");
    for i in 1..=n {
        s += &format!(" t.s{i} = {i}");
    }
    s
}

/// Two keyed fields get four nodes, where 5.1's two would put `c` first.
#[test]
fn a_two_key_constructor_walks_as_the_clients() {
    assert_eq!(layout("t = {c=1,a=2}"), ("a c".into(), 0, 2));
}

/// Keys added one at a time, either side of 8, 16 and 32: the eighth key fills eight nodes and
/// rehashes into sixteen, with itself among the moved.
#[test]
fn keys_added_one_by_one_walk_as_the_clients() {
    let want: [(usize, &str, u8); 9] = [
        (7, "s4 s6 s5 s1 s7 s3 s2", 3),
        (8, "s6 s7 s4 s5 s8 s1 s3 s2", 4),
        (9, "s6 s7 s4 s5 s9 s8 s1 s3 s2", 4),
        (15, "s13 s15 s11 s6 s7 s4 s5 s12 s14 s9 s8 s1 s10 s3 s2", 4),
        (
            16,
            "s13 s6 s7 s4 s5 s12 s9 s8 s11 s16 s10 s15 s1 s14 s3 s2",
            5,
        ),
        (
            17,
            "s13 s6 s7 s4 s5 s12 s9 s8 s11 s17 s16 s10 s15 s1 s14 s3 s2",
            5,
        ),
        (
            31,
            "s27 s13 s23 s6 s7 s4 s5 s12 s26 s9 s8 s31 s30 s25 s17 s21 s29 s18 s11 s24 s22 s16 \
             s15 s28 s20 s10 s19 s1 s14 s3 s2",
            5,
        ),
        (
            32,
            "s17 s7 s5 s16 s9 s31 s18 s15 s30 s14 s19 s1 s3 s27 s23 s6 s4 s26 s22 s21 s29 s25 \
             s13 s32 s28 s20 s12 s24 s8 s11 s10 s2",
            6,
        ),
        (
            33,
            "s17 s33 s7 s5 s16 s9 s31 s18 s15 s30 s14 s19 s1 s3 s27 s23 s6 s4 s26 s22 s21 s29 \
             s25 s13 s32 s28 s20 s12 s24 s8 s11 s10 s2",
            6,
        ),
    ];
    for (n, keys, lsize) in want {
        assert_eq!(layout(&one_by_one(n)), (keys.into(), 0, lsize), "{n} keys");
    }
}

/// Integer keys join the array part when a rehash finds them dense enough, at least half of a
/// power of two: a lone `[2]` makes an array of 2, `[3]` and `[4]` one of 4, where 5.1 wants
/// more than half.
#[test]
fn mixed_array_and_hash_walk_as_the_clients() {
    assert_eq!(
        layout(
            "t = {1, 2, 3, x = 1, y = 2} t[5] = 5 t[4] = 4 t.z = 3 t[10] = 10 t[1.5] = 1 t[0] = 0"
        ),
        ("1 2 3 4 5 10 x 1.5 z 0 y".into(), 8, 3)
    );
    assert_eq!(
        layout("t = {} for i = 1, 5 do t[i] = i end t.a = 1 t[7] = 7 t[6] = 6 t.b = 2 t[8] = 8"),
        ("1 2 3 4 5 6 7 8 a b".into(), 8, 2)
    );
    assert_eq!(layout("t = {} t[2] = 1"), ("2".into(), 2, 0));
    assert_eq!(layout("t = {} t[3] = 1 t[4] = 1"), ("3 4".into(), 4, 0));
}

/// A deleted key keeps its node until a rehash drops it; set again before that, it takes the same
/// node back.
#[test]
fn deletes_then_inserts_walk_as_the_clients() {
    let mut src = one_by_one(12);
    src += " t.s2 = nil t.s5 = nil t.s9 = nil";
    for i in 13..=20 {
        src += &format!(" t.s{i} = {i}");
    }
    assert_eq!(
        layout(&src),
        (
            "s17 s6 s7 s4 s16 s8 s18 s11 s13 s15 s20 s14 s19 s1 s12 s3 s10".into(),
            0,
            5
        )
    );
    assert_eq!(
        layout(&(one_by_one(7) + " t.s3 = nil t.s3 = 3 t.s8 = 8")),
        ("s6 s7 s4 s5 s8 s1 s3 s2".into(), 0, 4)
    );
}

/// A collection between the delete and the insert changes nothing while the key's string lives:
/// the client's collector passes an empty entry by (`traversetable` `0x6f76be`).
#[test]
fn a_collection_keeps_a_deleted_live_key_in_its_node() {
    let mut vm = benilla_lua::new_state();
    let src = "keep = {'s1', 's2', 's3', 's4', 's5'} t = {} \
               t.s1 = 1 t.s2 = 2 t.s3 = 3 t.s4 = 4 t.s5 = 5 t.s4 = nil";
    vm.exec(src.as_bytes(), b"=layout").unwrap();
    vm.full_gc();
    let r = vm
        .exec(
            b"t.s4 = 0 local o = '' for k in pairs(t) do o = o .. k .. ' ' end return o",
            b"=layout",
        )
        .unwrap();
    let V::Str(s) = r[0].unpack() else { panic!() };
    // The client's order for the same steps, with `lua_setgcthreshold(L, 0)` (`0x6f4400`)
    // collecting between them, and with no collection at all.
    assert_eq!(vm.heap.str(s), b"s5 s1 s4 s3 s2 ");
}

/// The keys of `t`'s table, in `next` order.
fn keys_of(vm: &benilla_lua::Vm, t: benilla_lua::value::TableRef) -> String {
    let mut keys = Vec::new();
    let mut k = Value::Nil;
    while let Some((nk, _)) = vm.heap.table(t).next(k).unwrap() {
        let V::Str(s) = nk.unpack() else {
            panic!("a string key")
        };
        keys.push(String::from_utf8_lossy(vm.heap.str(s)).into_owned());
        k = nk;
    }
    keys.join(" ")
}

/// The client's globals once `0x7039e0` has opened its state, run in its code.
pub const OPEN_GLOBALS: &str = "log tostring getn format debuginfo frexp pairs max assert \
    tonumber debugbreak seterrorhandler atan foreach tan cos log10 strupper asin loadstring \
    tinsert string xpcall date bit strlen strrep strlower deg table __framescript_meta strsub \
    random gsub sort strchar debugprofilestart next strbyte sqrt sin ipairs randomseed mod \
    rawequal rad getglobal PI getmetatable min debugload ceil tremove acos gcinfo rawset ldexp \
    foreachi unpack debugprint time exp math floor pcall getfenv setmetatable type \
    debugprofilestop debugtimestamp abs rawget strfind debugdump setglobal debugstack __pow \
    setfenv atan2 geterrorhandler error collectgarbage";

/// A new state's globals and its four library tables walk as the client's do after
/// `0x7039e0`: the base array, `luaL_openlib`'s tables filled from empty in their arrays' order,
/// and the compat chunk's globals one at a time.
#[test]
fn a_new_state_lays_out_its_globals_and_libraries_as_the_clients() {
    let mut vm = benilla_lua::new_state();
    let g = vm.globals;
    assert_eq!(vm.heap.table(g).lsizenode, 7);
    assert_eq!(
        keys_of(&vm, g),
        OPEN_GLOBALS
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    );
    for (lib, want) in [
        (
            "string",
            "sub gfind rep gsub char dump find upper len format byte lower",
        ),
        (
            "table",
            "setn insert getn foreachi foreach sort remove concat",
        ),
        (
            "math",
            "log atan ldexp deg tan cos pi random randomseed frexp ceil floor rad max sqrt pow \
             asin min mod exp log10 atan2 acos sin abs",
        ),
        ("bit", "band rshift bor bnot arshift mod lshift bxor"),
    ] {
        let V::Table(t) = vm.get_global(lib).unpack() else {
            panic!("{lib}")
        };
        assert_eq!(keys_of(&vm, t), want, "{lib}");
    }
}
