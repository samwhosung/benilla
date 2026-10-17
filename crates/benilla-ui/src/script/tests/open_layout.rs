//! The script host's state once open lays out its globals and libraries as the client's does
//! after `0x7039e0`: the host and the dialect replace values under the opening's keys and add
//! none, so `pairs` walks them in the client's order. Each order is the client's, run in its own
//! code (benilla-lua's `tests/table_layout.rs`).

use super::super::{open_state, VmKind};
use mlua::benilla_lua::value::{TableRef, V};
use mlua::benilla_lua::{Value, Vm};

fn keys_of(vm: &Vm, t: TableRef) -> String {
    let mut keys = Vec::new();
    let mut k = Value::Nil;
    while let Some((nk, _)) = vm.heap.table(t).next(k).unwrap() {
        keys.push(match nk.unpack() {
            V::Str(s) => String::from_utf8_lossy(vm.heap.str(s)).into_owned(),
            _ => "?".into(),
        });
        k = nk;
    }
    keys.join(" ")
}

#[test]
fn the_open_state_lays_out_as_the_clients() {
    for kind in [VmKind::InGame, VmKind::Glue] {
        let lua = open_state(kind).unwrap();
        lua.with_vm(|vm| {
            let g = vm.globals;
            assert_eq!(
                keys_of(vm, g),
                "log tostring getn format debuginfo frexp pairs max assert tonumber debugbreak \
                 seterrorhandler atan foreach tan cos log10 strupper asin loadstring tinsert \
                 string xpcall date bit strlen strrep strlower deg table __framescript_meta \
                 strsub random gsub sort strchar debugprofilestart next strbyte sqrt sin ipairs \
                 randomseed mod rawequal rad getglobal PI getmetatable min debugload ceil \
                 tremove acos gcinfo rawset ldexp foreachi unpack debugprint time exp math floor \
                 pcall getfenv setmetatable type debugprofilestop debugtimestamp abs rawget \
                 strfind debugdump setglobal debugstack __pow setfenv atan2 geterrorhandler \
                 error collectgarbage"
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
                    "log atan ldexp deg tan cos pi random randomseed frexp ceil floor rad max \
                     sqrt pow asin min mod exp log10 atan2 acos sin abs",
                ),
                ("bit", "band rshift bor bnot arshift mod lshift bxor"),
            ] {
                let V::Table(t) = vm.get_global(lib).unpack() else {
                    panic!("{lib}")
                };
                assert_eq!(keys_of(vm, t), want, "{lib}");
            }
        });
    }
}
