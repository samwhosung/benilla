//! **Lua 5.0's remembered table size, and the whole library that rides it** (decision 2102).
//!
//! 5.0 does **not** simply write `t.n`. `luaL_setn` (`0x6f4ea0`) writes `t.n` *only if a
//! non-negative numeric `t.n` already exists*; otherwise it stores the size in a weak-keyed table
//! kept in the registry (`getsizes 0x6f4fc0`, registry slot 2), so the size is invisible to
//! anything walking the table. `luaL_getn` (`0x6f5050`) reads `t.n`, then `sizes[t]`, then counts
//! `rawgeti` from 1 until the first nil.
//!
//! **And the size is not private to `getn`/`setn` — the whole table library rides it.** A
//! whole-image call-site census lists every one:
//! `luaL_getn` is called by `table.getn`, `table.insert`, `table.remove`, `table.concat`,
//! `table.sort`, `table.foreachi` and **base `unpack`**; `luaL_setn` by `table.setn`,
//! `table.insert` and `table.remove`. So `insert`/`remove` **update** the remembered size and the
//! six readers read it — this library is `n`-based end to end, and a `#`-based (5.1) one is a
//! different observable.
//!
//! **This module shipped for months with only `getn`/`setn` converted, and the divergence was
//! recorded as knowingly-accepted "revisit if a runtime failure is ever traced here". One was.**
//! Mik's Scrolling Battle Text 4.43 displays nothing at all in combat, because every combat line
//! it parses runs through
//!
//! ```lua
//! MikCEH.EraseTable(unorderedCaptureData)          -- wipe, then table.setn(t, 0)
//! … table.insert(unorderedCaptureData, capture) …  -- 5.1: lands at #t+1, size NOT updated
//! if (table.getn(tempCapturedData) ~= 0) then      -- reads the stale 0 → "no match"
//! ```
//!
//! The captures were *there* (`t[1] = "Kobold Vermin"`, `t[2] = "12"`) and `getn` answered 0, so
//! every parser returned nil and the addon drew nothing while scoring a clean load. That is the
//! exact shape the note warned about, and it is why the readers are converted here too rather than
//! left half-way: a library where the mutators maintain one size and the readers consult another
//! is a worse place to stand than a uniformly wrong one.
//!
//! **The first draft of the `getn`/`setn` half wrote `t.n` unconditionally, and the corpus caught
//! it inside an hour.** `AceOO-2.0`'s `_Embed` walks its mixin's exports with
//! `next(state.export, field)` and errors on any field the target already has; a spurious `n` made
//! that `Method conflict in attempt to mixin. Field "n"` — **39 addons**, a *bigger* wall than the
//! `setn` gap it was meant to fix. The invisibility of the side store is load-bearing, and its
//! test is below.

use mlua::{Function, Lua, MultiValue, Table, Value};

/// Where the weak-keyed side store lives. 5.0 keeps it in registry slot 2; mlua's registry is
/// its own, so it gets a name instead — the property that matters is that it is reachable from
/// nothing an addon can see.
const SIZES: &str = "benilla.lua50.sizes";

/// `checkint 0x6f4f80` as `luaL_getn`/`luaL_setn` use it: a stored size counts only when it is
/// a number **and** non-negative — a negative or absent one selects the next leg. (The
/// reference coerces a numeric *string* here too, via `lua_tonumber`; we do not, and no corpus
/// caller writes a string `n`.)
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

/// The bound on how many elements one `insert`/`remove`/`unpack` may shift or push.
///
/// **Not the reference's** — 5.0 has no such limit, and this is the third knowing divergence in
/// this module. It exists because a remembered size is an arbitrary number an addon wrote: after
/// `table.setn(t, 2000000000)`, `table.remove(t, 1)` shifts two billion slots, and this runs in
/// Rust, where the VM's instruction budget cannot interrupt it — a hung client rather than a slow
/// one. A UI table never comes near a million entries; a runaway always exceeds it.
const MAX_SHIFT: i64 = 1_000_000;

/// `luaL_getn 0x6f5050` — `t.n`, then `sizes[t]`, then a linear count.
///
/// The count is `rawgeti` from 1 to the first nil, not the `#` border: 1.12's Lua has no length
/// operator at all (decision 2101) and the two disagree on a table with holes, where `#` is
/// explicitly undefined. It costs O(n) — exactly as it does in the reference, and only until
/// the first `insert`/`setn` remembers a size for that table.
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

/// `luaL_setn 0x6f4ea0` — into `t.n` when the table already carries one, else out of sight.
fn set_n(sizes: &Table, t: &Table, n: i64) -> mlua::Result<()> {
    if as_size(&t.raw_get::<Value>("n")?).is_some() {
        t.raw_set("n", n)
    } else {
        sizes.raw_set(t.clone(), n)
    }
}

/// Install the `n`-based table library over mlua's 5.1 one.
///
/// `concat` and `sort` **wrap** the C implementations rather than re-writing them: the only
/// thing 5.0 changes about either is which number bounds the walk, and the C bodies carry the
/// error messages (`invalid value (at index %d) in table for 'concat'`, `invalid order
/// function for sorting`) and the sort algorithm that a Rust re-write would have to reproduce —
/// including *not* handing a Lua comparator to Rust's `sort_by`, which panics on an
/// inconsistent one.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    let store = lua.create_table()?;
    let meta = lua.create_table()?;
    // Weak KEYS: a table nobody else holds must still be collectable, and 5.0's own `getsizes`
    // builds exactly this metatable.
    meta.set("__mode", "k")?;
    store.set_metatable(Some(meta))?;
    // Kept in the registry so it survives for the VM's life and nothing an addon can reach
    // names it — and captured by every closure below, so the hottest function in the whole UI
    // (`table.insert`) costs two raw gets rather than a registry walk per call.
    lua.set_named_registry_value(SIZES, store.clone())?;
    let sizes = store;

    let t: Table = g.get("table")?;

    let getn_sizes = sizes.clone();
    t.set(
        "getn",
        lua.create_function(move |_, t: Table| get_n(&getn_sizes, &t))?,
    )?;
    // `table.setn 0x7fb670` — 0 results.
    let setn_sizes = sizes.clone();
    t.set(
        "setn",
        lua.create_function(move |_, (t, n): (Table, i64)| set_n(&setn_sizes, &t, n))?,
    )?;

    // `table.insert 0x7fb6a0` — `n = getn(t) + 1`; with a position argument the position may
    // GROW n past it; `setn` first, then shift up from n down to pos, then store.
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

    // `table.remove 0x7fb750` — an empty table (remembered size <= 0) returns NO value and
    // touches nothing; otherwise `setn(n-1)`, take `t[pos]`, close the gap, nil the tail.
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

    // `table.foreachi 0x7fb4e0` — 1..getn, stop and hand back the first non-nil result.
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

    // `table.concat 0x7fb860` — `j` defaults to getn, everything else is 5.1's own body.
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

    // `table.sort 0x7fb900` — sorts `1..getn`, where 5.1's sorts `1..#t`. The two agree unless
    // a remembered size is SHORTER than the array border, which is the only case that needs
    // anything: lift the tail out, let the C sort run on exactly the elements 5.0 would have
    // handed it, put the tail back. (A remembered size LONGER than the border would have 5.0
    // comparing against nils and raising; we sort the border instead — a knowing divergence
    // from an error path, not from a mechanism.)
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

    // Base `unpack 0x703080` — `1..getn(t)`, by `rawgeti`. The reference takes only the table
    // and ignores anything after it, so no 1.12 caller can pass a range meaningfully; we keep
    // 5.1's optional `i`/`j` because two of this crate's OWN stdlib helpers pass an explicit
    // range to carry embedded nils (`tostringall`, the positional-`format` shim), and nothing
    // an addon can write distinguishes the two.
    g.set(
        "unpack",
        lua.create_function(move |_, (t, i, j): (Table, Option<i64>, Option<i64>)| {
            let i = i.unwrap_or(1);
            let j = match j {
                Some(j) => j,
                None => get_n(&sizes, &t)?,
            };
            // The reference's `luaL_checkstack(n, "table too big to unpack")`, at the same
            // bound the shifts take.
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

    /// **The failure that retired the "revisit if ever traced" note**, in the shape Mik's
    /// Scrolling Battle Text 4.43 writes it (`MikCombatEventHelper.lua`'s `EraseTable` →
    /// `GetUnorderedCaptureDataTable` → `GetCapturedData`).
    ///
    /// The captures are in the table and `getn` answered 0, so every combat line the addon parsed
    /// came back "no match" and it drew nothing at all — through a clean load, a clean session and
    /// a `drew=nothing(0)` harness row that read like an addon with nothing to draw.
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

    /// `setn`/`getn` are 5.0's real pair, and the side store stays invisible.
    ///
    /// Both halves have been got wrong once each: a no-op `setn` fails the round-trip, and an
    /// unconditional `rawset(t, "n", …)` passes it and breaks the 39 addons whose `AceOO-2.0`
    /// mixin walk errors on a stray `n` (this module's header).
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
        // A table that already carries a numeric `n` keeps using it — 5.0's own first branch, and
        // why `arg.n` behaves the way 5.0-era code expects.
        assert_eq!(
            s.eval::<i64>("local t = {1,2,3, n=3} table.setn(t, 7) return t.n")
                .unwrap(),
            7
        );
        // A NEGATIVE `n` is `checkint < 0`, which selects the side store rather than the field —
        // `0x6f4ee4`'s `jl`.
        assert_eq!(
            s.eval::<i64>("local t = {1,2, n=-1} table.setn(t, 5) return table.getn(t) + t.n")
                .unwrap(),
            4
        );
    }

    /// The count of last resort is `rawgeti` to the first nil, not the `#` border — the two
    /// disagree on a holed table, and `#` is not even in this VM's grammar (2101).
    ///
    /// This is the shape every `local t = { f() }` takes when `f` returns a nil in the middle, and
    /// it is why `table.getn` is not a way to count returns on 1.12.
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

    /// `insert` with a position, and `remove` — the two that write the size back.
    #[test]
    fn insert_and_remove_maintain_the_size_the_way_the_reference_does() {
        let s = UiScript::new().unwrap();
        // A position past the remembered size GROWS it (`if (pos > n) n = pos`).
        assert_eq!(
            s.eval::<String>(
                "local t = {} table.insert(t, 3, 'c') \
                 return table.getn(t) .. ':' .. tostring(t[3])"
            )
            .unwrap(),
            "3:c"
        );
        // Insert-at-position shifts up.
        assert_eq!(
            s.eval::<String>(
                "local t = {'a','c'} table.insert(t, 2, 'b') \
                 return table.concat(t, '') .. ':' .. table.getn(t)"
            )
            .unwrap(),
            "abc:3"
        );
        // Remove returns the element, closes the gap, and shrinks the size.
        assert_eq!(
            s.eval::<String>(
                "local t = {'a','b','c'} local got = table.remove(t, 1) \
                 return got .. table.concat(t, '') .. ':' .. table.getn(t)"
            )
            .unwrap(),
            "abc:2"
        );
        // An empty table answers NO value at all and touches nothing (`if (n <= 0) return 0`).
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

    /// The four readers take the remembered size, not the border: `concat`, `sort`, `foreachi`,
    /// and base `unpack` (the call-site census in this module's header).
    #[test]
    fn concat_sort_foreachi_and_unpack_all_read_the_remembered_size() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<String>("local t = {'a','b','c'} table.setn(t, 2) return table.concat(t, '')")
                .unwrap(),
            "ab"
        );
        // sort touches only 1..getn — the tail past it keeps both its values and its order.
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
        // The one superset this module keeps: an EXPLICIT range, which two of our own stdlib
        // helpers pass to carry embedded nils. `tostringall`/the positional-`format` shim break
        // without it, and no 1.12 caller can observe it (5.0's `unpack` ignores extra arguments).
        assert_eq!(
            s.eval::<String>(
                "local t = {} t[1] = 'a' t[3] = 'c' \
                 local x, y, z = unpack(t, 1, 3) \
                 return tostring(x) .. tostring(y) .. tostring(z)"
            )
            .unwrap(),
            "anilc"
        );
        // ...and with nothing remembered they answer exactly what they always did.
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

    /// The bare `getn` global and `table.getn` are one function, and `tinsert`/`tremove` — bound
    /// by the stdlib layer, which runs after this one — are the new bodies rather than 5.1's.
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
