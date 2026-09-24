//! Lua 5.0's remembered table size, and the table library that reads it.
//!
//! `luaL_setn` (`0x6f4ea0`) writes `t.n` only when a non-negative numeric `t.n` exists, else it
//! stores the size in a weak-keyed registry table (`getsizes` `0x6f4fc0`, slot 2) no walk can see.
//! `luaL_getn` (`0x6f5050`) reads `t.n`, then that table, then counts `rawgeti` from 1 to the
//! first nil. `luaL_getn` serves `table.getn`, `insert`, `remove`, `concat`, `sort`, `foreachi`
//! and base `unpack`; `luaL_setn` serves `table.setn`, `insert` and `remove`: the library is
//! `n`-based end to end, where 5.1's is `#`-based.
//!
//! The side store must stay invisible: a stray `n` key breaks mixin walks such as `AceOO-2.0`'s,
//! which error on any field the target already has.

use mlua::{Function, Lua, MultiValue, Table, Value};

/// The side store's registry name; 5.0 uses slot 2, and either way no addon can reach it.
const SIZES: &str = "benilla.lua50.sizes";

/// `checkint` (`0x6f4f80`): a stored size counts only if numeric and non-negative. The reference
/// also takes a numeric string through `lua_tonumber`; this does not.
fn as_size(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => (*i >= 0).then_some(*i),
        #[allow(clippy::cast_possible_truncation)]
        Value::Number(n) => {
            let i = *n as i64;
            (i >= 0).then_some(i)
        }
        _ => None,
    }
}

/// The most elements one `insert`/`remove`/`unpack` may shift or push. Deviation: 5.0 has no
/// bound, but a remembered size is whatever an addon wrote, and a Rust loop over two billion slots
/// is beyond the VM's instruction budget: a hung client.
const MAX_SHIFT: i64 = 1_000_000;

/// `luaL_getn` (`0x6f5050`); its count to the first nil is not the `#` border on a holed table.
fn get_n(sizes: &Table, t: &Table) -> mlua::Result<i64> {
    if let Some(n) = as_size(&t.raw_get::<Value>("n")?) {
        return Ok(n);
    }
    if let Some(n) = as_size(&sizes.raw_get::<Value>(t.clone())?) {
        return Ok(n);
    }
    let mut i = 1i64;
    while !matches!(t.raw_get::<Value>(i)?, Value::Nil) {
        i += 1;
    }
    Ok(i - 1)
}

/// `luaL_setn` (`0x6f4ea0`): into `t.n` when the table has one, else the side store.
fn set_n(sizes: &Table, t: &Table, n: i64) -> mlua::Result<()> {
    if as_size(&t.raw_get::<Value>("n")?).is_some() {
        t.raw_set("n", n)
    } else {
        sizes.raw_set(t.clone(), n)
    }
}

/// Install the `n`-based table library over mlua's 5.1 one. `concat` and `sort` wrap the C bodies
/// for their error messages and algorithm (Rust's `sort_by` panics on an inconsistent comparator).
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    let store = lua.create_table()?;
    let meta = lua.create_table()?;
    // Weak keys, as 5.0's `getsizes` builds it: a table nobody else holds stays collectable.
    meta.set("__mode", "k")?;
    store.set_metatable(Some(meta))?;
    // Held by the registry for the VM's life; each closure below captures it.
    lua.set_named_registry_value(SIZES, store.clone())?;
    let sizes = store;

    let t: Table = g.get("table")?;

    let getn_sizes = sizes.clone();
    t.set(
        "getn",
        lua.create_function(move |_, t: Table| get_n(&getn_sizes, &t))?,
    )?;
    // `table.setn` (`0x7fb670`): no results.
    let setn_sizes = sizes.clone();
    t.set(
        "setn",
        lua.create_function(move |_, (t, n): (Table, i64)| set_n(&setn_sizes, &t, n))?,
    )?;

    // `table.insert` (`0x7fb6a0`): `n = getn(t) + 1`, raised to a later position; `setn` first,
    // then shift up from `n` down to `pos`, then store.
    let insert_sizes = sizes.clone();
    t.set(
        "insert",
        lua.create_function(move |_, args: MultiValue| {
            let argc = args.len();
            let mut it = args.into_iter();
            let Some(Value::Table(t)) = it.next() else {
                return Err(mlua::Error::runtime(
                    "bad argument #1 to 'insert' (table expected)",
                ));
            };
            let mut n = get_n(&insert_sizes, &t)? + 1;
            // The 2-vs-3 argument split is `lua_gettop`, so an explicit trailing nil counts:
            // `insert(t, 4, nil)` writes nil at 4, it does not append the 4.
            let (pos, v) = if argc <= 2 {
                (n, it.next().unwrap_or(Value::Nil))
            } else {
                let pos = match it.next() {
                    Some(Value::Integer(i)) => i,
                    #[allow(clippy::cast_possible_truncation)]
                    Some(Value::Number(x)) => x as i64,
                    _ => {
                        return Err(mlua::Error::runtime(
                            "bad argument #2 to 'insert' (number expected)",
                        ))
                    }
                };
                if pos > n {
                    n = pos;
                }
                (pos, it.next().unwrap_or(Value::Nil))
            };
            if n.saturating_sub(pos) > MAX_SHIFT {
                return Err(mlua::Error::runtime("table too big to insert into"));
            }
            set_n(&insert_sizes, &t, n)?;
            while n > pos {
                let above: Value = t.raw_get(n - 1)?;
                t.raw_set(n, above)?;
                n -= 1;
            }
            t.raw_set(pos, v)
        })?,
    )?;

    // `table.remove` (`0x7fb750`): at a size of 0 or less, no value and no change; else
    // `setn(n - 1)`, take `t[pos]`, close the gap, nil the tail.
    let remove_sizes = sizes.clone();
    t.set(
        "remove",
        lua.create_function(move |_, (t, pos): (Table, Option<i64>)| {
            let n = get_n(&remove_sizes, &t)?;
            let pos = pos.unwrap_or(n);
            if n <= 0 {
                return Ok(MultiValue::new());
            }
            if n.saturating_sub(pos) > MAX_SHIFT {
                return Err(mlua::Error::runtime("table too big to remove from"));
            }
            set_n(&remove_sizes, &t, n - 1)?;
            let taken: Value = t.raw_get(pos)?;
            for i in pos..n {
                let next: Value = t.raw_get(i + 1)?;
                t.raw_set(i, next)?;
            }
            t.raw_set(n, Value::Nil)?;
            Ok(MultiValue::from_vec(vec![taken]))
        })?,
    )?;

    // `table.foreachi` (`0x7fb4e0`): `1..getn`, returning the first non-nil result.
    let foreachi_sizes = sizes.clone();
    t.set(
        "foreachi",
        lua.create_function(move |_, (t, f): (Table, Function)| {
            let n = get_n(&foreachi_sizes, &t)?;
            for i in 1..=n {
                let v: Value = t.raw_get(i)?;
                let r: Value = f.call((i, v))?;
                if !matches!(r, Value::Nil) {
                    return Ok(MultiValue::from_vec(vec![r]));
                }
            }
            Ok(MultiValue::new())
        })?,
    )?;

    // `table.concat` (`0x7fb860`): `j` defaults to getn; the rest is 5.1's body.
    let concat_sizes = sizes.clone();
    let concat: Function = t.get("concat")?;
    t.set(
        "concat",
        lua.create_function(
            move |_, (t, sep, i, j): (Table, Option<String>, Option<i64>, Option<i64>)| {
                let j = match j {
                    Some(j) => j,
                    None => get_n(&concat_sizes, &t)?,
                };
                concat.call::<Value>((t, sep.unwrap_or_default(), i.unwrap_or(1), j))
            },
        )?,
    )?;

    // `table.sort` (`0x7fb900`) sorts `1..getn`, 5.1's `1..#t`: a shorter remembered size lifts
    // the tail out around the C sort. Deviation: a longer one sorts only the border, where 5.0
    // compares nils and raises: it departs from an error path, not from a mechanism.
    let sort_sizes = sizes.clone();
    let sort: Function = t.get("sort")?;
    t.set(
        "sort",
        lua.create_function(move |_, (t, cmp): (Table, Option<Function>)| {
            let n = get_n(&sort_sizes, &t)?;
            let border = i64::try_from(t.raw_len()).unwrap_or(i64::MAX);
            if n >= border {
                return sort.call::<()>((t, cmp));
            }
            let tail: Vec<Value> = ((n + 1)..=border)
                .map(|i| t.raw_get::<Value>(i))
                .collect::<mlua::Result<_>>()?;
            for i in (n + 1)..=border {
                t.raw_set(i, Value::Nil)?;
            }
            let sorted = sort.call::<()>((t.clone(), cmp));
            for (k, v) in tail.into_iter().enumerate() {
                t.raw_set(n + 1 + i64::try_from(k).unwrap_or(0), v)?;
            }
            sorted
        })?,
    )?;

    // Base `unpack` (`0x703080`): `1..getn(t)` by `rawgeti`. Deviation: the reference ignores
    // every argument after the table; `i`/`j` stay because the positional-`format` shim passes
    // a range to carry embedded nils.
    g.set(
        "unpack",
        lua.create_function(move |_, (t, i, j): (Table, Option<i64>, Option<i64>)| {
            let i = i.unwrap_or(1);
            let j = match j {
                Some(j) => j,
                None => get_n(&sizes, &t)?,
            };
            // The reference's `luaL_checkstack(n, "table too big to unpack")`, at `MAX_SHIFT`.
            if j.saturating_sub(i) >= MAX_SHIFT {
                return Err(mlua::Error::runtime("too many results to unpack"));
            }
            let mut out = Vec::new();
            for k in i..=j {
                out.push(t.raw_get::<Value>(k)?);
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// Mik's Scrolling Battle Text 4.43's capture tables: wipe, `setn(t, 0)`, then insert.
    #[test]
    fn a_wipe_then_setn_zero_then_insert_is_countable_again() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>(
                r#"
                local t = { "stale", "stale" }
                for k in pairs(t) do t[k] = nil end
                table.setn(t, 0)
                table.insert(t, "Kobold Vermin")
                table.insert(t, "12")
                return table.getn(t)
                "#
            )
            .unwrap(),
            2,
            "insert must update the remembered size, or getn answers the wiped 0 forever"
        );
    }

    #[test]
    fn the_remembered_size_round_trips_and_never_appears_as_a_key() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return table.getn({1,2,3})").unwrap(), 3);
        assert_eq!(
            s.eval::<i64>("local t = {1,2,3} table.setn(t, 7) return table.getn(t)")
                .unwrap(),
            7
        );
        assert_eq!(
            s.eval::<i64>(
                "local t = {1,2,3} table.setn(t, 0) \
                 local c = 0 for k in pairs(t) do c = c + 1 end return c"
            )
            .unwrap(),
            3,
            "a remembered size must not appear as a key"
        );
        // A table with a numeric `n` keeps using it: 5.0's first branch, which `arg.n` relies on.
        assert_eq!(
            s.eval::<i64>("local t = {1,2,3, n=3} table.setn(t, 7) return t.n")
                .unwrap(),
            7
        );
        // A negative `n` fails `checkint` and selects the side store (`0x6f4ee4`'s `jl`).
        assert_eq!(
            s.eval::<i64>("local t = {1,2, n=-1} table.setn(t, 5) return table.getn(t) + t.n")
                .unwrap(),
            4
        );
    }

    #[test]
    fn the_fallback_count_stops_at_the_first_hole() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("local t = {} t[1] = 'a' t[3] = 'c' return table.getn(t)")
                .unwrap(),
            1
        );
        assert_eq!(s.eval::<i64>("return table.getn({})").unwrap(), 0);
    }

    #[test]
    fn insert_and_remove_maintain_the_size_the_way_the_reference_does() {
        let s = UiScript::new().unwrap();
        // A position past the remembered size grows it (`if (pos > n) n = pos`).
        assert_eq!(
            s.eval::<String>(
                "local t = {} table.insert(t, 3, 'c') \
                 return table.getn(t) .. ':' .. tostring(t[3])"
            )
            .unwrap(),
            "3:c"
        );
        assert_eq!(
            s.eval::<String>(
                "local t = {'a','c'} table.insert(t, 2, 'b') \
                 return table.concat(t, '') .. ':' .. table.getn(t)"
            )
            .unwrap(),
            "abc:3"
        );
        assert_eq!(
            s.eval::<String>(
                "local t = {'a','b','c'} local got = table.remove(t, 1) \
                 return got .. table.concat(t, '') .. ':' .. table.getn(t)"
            )
            .unwrap(),
            "abc:2"
        );
        // At size 0, no value and no change (`if (n <= 0) return 0`).
        assert_eq!(
            s.eval::<i64>(
                "local t = {1,2,3} table.setn(t, 0) \
                 local got = table.remove(t) \
                 if got ~= nil then return -1 end return table.getn(t) + t[1]"
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn concat_sort_foreachi_and_unpack_all_read_the_remembered_size() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<String>("local t = {'a','b','c'} table.setn(t, 2) return table.concat(t, '')")
                .unwrap(),
            "ab"
        );
        assert_eq!(
            s.eval::<String>(
                "local t = {'c','a','z','y'} table.setn(t, 2) table.sort(t) \
                 return table.concat(t, '', 1, 4)"
            )
            .unwrap(),
            "aczy"
        );
        assert_eq!(
            s.eval::<String>(
                "local t = {'a','b','c'} table.setn(t, 2) local out = '' \
                 table.foreachi(t, function(i, v) out = out .. v end) return out"
            )
            .unwrap(),
            "ab"
        );
        assert_eq!(
            s.eval::<String>(
                "local t = {'a','b','c'} table.setn(t, 2) \
                 local x, y, z = unpack(t) return tostring(x) .. tostring(y) .. tostring(z)"
            )
            .unwrap(),
            "abnil"
        );
        assert_eq!(
            s.eval::<String>(
                "local t = {} t[1] = 'a' t[3] = 'c' \
                 local x, y, z = unpack(t, 1, 3) \
                 return tostring(x) .. tostring(y) .. tostring(z)"
            )
            .unwrap(),
            "anilc"
        );
        assert_eq!(
            s.eval::<String>("return table.concat({'a','b','c'}, '')")
                .unwrap(),
            "abc"
        );
        assert_eq!(
            s.eval::<String>("local t = {'c','b','a'} table.sort(t) return table.concat(t, '')")
                .unwrap(),
            "abc"
        );
    }

    /// `getn`, `tinsert` and `tremove`, bound by the stdlib layer after this one, are these bodies.
    #[test]
    fn the_bare_aliases_bind_the_5_0_bodies() {
        let s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return getn == table.getn").unwrap());
        assert!(s.eval::<bool>("return tinsert == table.insert").unwrap());
        assert!(s.eval::<bool>("return tremove == table.remove").unwrap());
        assert_eq!(
            s.eval::<i64>("local t = {1,2,3} table.setn(t, 0) tinsert(t, 9) return t[1]")
                .unwrap(),
            9
        );
    }
}
