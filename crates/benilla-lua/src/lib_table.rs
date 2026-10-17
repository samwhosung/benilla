//! The table library: 1.12.1's, Lua 5.0's `ltablib.c` as the client registers it (`0x822d40`),
//! in the script host's state and in [`open`]'s alike. It is `n`-based end to end: `getn`,
//! `insert`, `remove`, `concat`, `sort` and `foreachi` read the size through `luaL_getn` and
//! `setn`, `insert` and `remove` write it through `luaL_setn` ([`getn`], [`setn`]). A callback
//! (`foreachi`, `foreach`, `sort`'s comparator) is called as `lua_call` calls it: unprotected, one
//! host level deep. `sort` is `ltablib.c`'s quicksort step for step, so an inconsistent
//! comparator does what it does there.
use crate::api::R;
use crate::lib_base::{getn, setn};
use crate::value::{TableRef, Value, V};
use crate::vm::Vm;

pub fn open(vm: &mut Vm) {
    // `luaL_openlib` makes a library's table empty, by `lua_newtable` (`0x6f4dff`).
    let t = vm.heap.new_table(0, 0);
    let fns: [(&'static str, crate::heap::NativeFn); 8] = [
        ("concat", concat),
        ("foreach", foreach),
        ("foreachi", foreachi),
        ("getn", getn_),
        ("setn", setn_),
        ("sort", sort),
        ("insert", insert),
        ("remove", remove),
    ];
    for (name, f) in fns {
        let v = vm.new_native(name, f);
        let k = vm.str(name.as_bytes());
        vm.heap.table_mut(t).set(k, v).unwrap();
    }
    vm.set_global("table", Value::Table(t));
}

/// The most elements one `insert` or `remove` may shift. Deviation: the client's loops have no
/// bound (`0x7fb728`, `0x7fb7e0`), but a remembered size is whatever an addon wrote, and the
/// instruction-budget hook never sees a native's loop, so one over two billion slots would hang
/// the client.
const MAX_SHIFT: i64 = 1_000_000;

/// `luaB_getn` (`0x7fb630`).
fn getn_(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let n = getn(vm, t);
    Ok(vm.ret1(Value::Num(n as f64)))
}

/// `luaB_setn` (`0x7fb670`).
fn setn_(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let n = vm.check_int(nargs, 2)?;
    setn(vm, t, n);
    Ok(0)
}

fn rawgeti(vm: &Vm, t: TableRef, i: i64) -> Value {
    vm.heap.table(t).get_int(i as i32)
}

fn rawseti(vm: &mut Vm, t: TableRef, i: i64, v: Value) -> R<()> {
    let tab = vm.heap.table_mut(t);
    // The array part inline, as `get_int` reads it; `k` below 1 wraps past its length.
    let k = i as i32;
    match tab.array.get_mut((k as usize).wrapping_sub(1)) {
        Some(slot) => *slot = v,
        None => {
            if let Err(e) = tab.set_int(k, v) {
                return Err(vm.rt_error(e.message()));
            }
        }
    }
    Ok(())
}

/// `luaB_foreachi` (`0x7fb4e0`): `f(i, t[i])` for `i` in `1..getn`, the first non-nil answer.
fn foreachi(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let n = getn(vm, t);
    let f = vm.check_function(nargs, 2)?;
    for i in 1..=n {
        let v = rawgeti(vm, t, i as i64);
        let r = vm.call_value(f, &[Value::Num(i as f64), v], 1)?;
        if !r[0].is_nil() {
            return Ok(vm.ret1(r[0]));
        }
    }
    Ok(0)
}

/// `luaB_foreach` (`0x7fb590`): `f(k, v)` over `next`, the first non-nil answer.
fn foreach(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let f = vm.check_function(nargs, 2)?;
    let mut k = Value::Nil;
    loop {
        let next = match vm.heap.table(t).next(k) {
            Ok(n) => n,
            // `luaH_next`'s `luaG_runerror` (`0x6fa467`, text `0x871aac`).
            Err(crate::table::InvalidKey) => return Err(vm.rt_error("invalid key for `next'")),
        };
        let Some((nk, v)) = next else { return Ok(0) };
        // The key stays on the stack across the call, as 5.0's loop keeps it.
        vm.push(nk);
        let r = vm.call_value(f, &[nk, v], 1);
        vm.top -= 1;
        let r = r?;
        if !r[0].is_nil() {
            return Ok(vm.ret1(r[0]));
        }
        k = nk;
    }
}

/// `luaB_tinsert` (`0x7fb6b0`): two arguments append; any other count is the position form
/// (`cmp ebx,2` at `0x7fb6e0`), which grows the size to the position.
fn insert(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let mut n = getn(vm, t) as i64 + 1;
    let (pos, v) = if nargs == 2 {
        (n, vm.arg(nargs, 1))
    } else {
        let pos = vm.check_int(nargs, 2)? as i64;
        if pos > n {
            n = pos;
        }
        (pos, vm.arg(nargs, 2))
    };
    if n.saturating_sub(pos) > MAX_SHIFT {
        return Err(vm.error_at(1, "table too big to insert into"));
    }
    setn(vm, t, n as i32);
    n -= 1;
    while n >= pos {
        let x = rawgeti(vm, t, n);
        rawseti(vm, t, n + 1, x)?;
        n -= 1;
    }
    rawseti(vm, t, pos, v)?;
    Ok(0)
}

/// `luaB_tremove` (`0x7fb770`): its position read before the empty test, so a bad one raises
/// on an empty table too.
fn remove(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let n = getn(vm, t) as i64;
    let mut pos = vm.opt_int(nargs, 2, n as i32)? as i64;
    if n <= 0 {
        return Ok(0);
    }
    if n.saturating_sub(pos) > MAX_SHIFT {
        return Err(vm.error_at(1, "table too big to remove from"));
    }
    setn(vm, t, (n - 1) as i32);
    let out = rawgeti(vm, t, pos);
    while pos < n {
        let x = rawgeti(vm, t, pos + 1);
        rawseti(vm, t, pos, x)?;
        pos += 1;
    }
    rawseti(vm, t, n, Value::Nil)?;
    Ok(vm.ret1(out))
}

/// `str_concat` (`0x7fb830`): items `i` (1) to `j` joined by the separator's bytes, each a string
/// or a number. Its arguments are read 2, 3, 4, then the table (`0x7fb84c`..`0x7fb88e`); a `j`
/// of 0, its default, is `getn` (`0x7fb893`).
fn concat(vm: &mut Vm, nargs: usize) -> R<usize> {
    let sep = vm.opt_str(nargs, 2, b"")?;
    let first = vm.opt_int(nargs, 3, 1)?;
    let mut last = vm.opt_int(nargs, 4, 0)?;
    let t = vm.check_table(nargs, 1)?;
    if last == 0 {
        last = getn(vm, t) as i32;
    }
    let sep = vm.heap.str(sep).to_vec();
    let mut out = Vec::new();
    for i in i64::from(first)..=i64::from(last) {
        let v = rawgeti(vm, t, i);
        // `luaL_argcheck(L, lua_isstring(L, -1), 1, ...)`, text `0x8821f0`.
        let Some(s) = matches!(v.unpack(), V::Str(_) | V::Num(_))
            .then(|| vm.tostr(v))
            .flatten()
        else {
            return Err(vm.arg_error(1, "table contains non-strings"));
        };
        vm.reserve(&mut out, vm.heap.str(s).len())?;
        out.extend_from_slice(vm.heap.str(s));
        if i != i64::from(last) {
            vm.reserve(&mut out, sep.len())?;
            out.extend_from_slice(&sep);
        }
    }
    Ok(vm.ret_str(&out))
}

/// `sort_comp` (`0x7fbc50`): the comparator through `lua_call`, or `lua_lessthan`.
#[inline]
fn comp(vm: &mut Vm, f: Value, a: Value, b: Value) -> R<bool> {
    if f.is_nil() {
        // Two numbers, the common sort, compared here; anything else as `<` compares it.
        if let (Some(x), Some(y)) = (a.as_num(), b.as_num()) {
            return Ok(x < y);
        }
        vm.less_than(a, b)
    } else {
        Ok(vm.call_value(f, &[a, b], 1)?[0].truthy())
    }
}

/// `auxsort` (`0x7fb990`), step for step, so a comparator's call order is the client's.
fn auxsort(vm: &mut Vm, t: TableRef, f: Value, mut l: i64, mut u: i64) -> R<()> {
    while l < u {
        let al = rawgeti(vm, t, l);
        let au = rawgeti(vm, t, u);
        if comp(vm, f, au, al)? {
            rawseti(vm, t, l, au)?;
            rawseti(vm, t, u, al)?;
        }
        if u - l == 1 {
            break;
        }
        let mut i = (l + u) / 2;
        let ai = rawgeti(vm, t, i);
        let al = rawgeti(vm, t, l);
        if comp(vm, f, ai, al)? {
            rawseti(vm, t, i, al)?;
            rawseti(vm, t, l, ai)?;
        } else {
            let au = rawgeti(vm, t, u);
            if comp(vm, f, au, ai)? {
                rawseti(vm, t, i, au)?;
                rawseti(vm, t, u, ai)?;
            }
        }
        if u - l == 2 {
            break;
        }
        let p = rawgeti(vm, t, i);
        let au1 = rawgeti(vm, t, u - 1);
        rawseti(vm, t, i, au1)?;
        rawseti(vm, t, u - 1, p)?;
        i = l;
        let mut j = u - 1;
        loop {
            loop {
                i += 1;
                let ai = rawgeti(vm, t, i);
                if !comp(vm, f, ai, p)? {
                    break;
                }
                if i > u {
                    return Err(vm.error_at(1, "invalid order function for sorting"));
                }
            }
            loop {
                j -= 1;
                let aj = rawgeti(vm, t, j);
                if !comp(vm, f, p, aj)? {
                    break;
                }
                if j < l {
                    return Err(vm.error_at(1, "invalid order function for sorting"));
                }
            }
            if j < i {
                break;
            }
            let ai = rawgeti(vm, t, i);
            let aj = rawgeti(vm, t, j);
            rawseti(vm, t, i, aj)?;
            rawseti(vm, t, j, ai)?;
        }
        let au1 = rawgeti(vm, t, u - 1);
        let ai = rawgeti(vm, t, i);
        rawseti(vm, t, u - 1, ai)?;
        rawseti(vm, t, i, au1)?;
        let (lo, hi);
        if i - l < u - i {
            lo = l;
            hi = i - 1;
            l = i + 1;
        } else {
            lo = i + 1;
            hi = u;
            u = i - 1;
        }
        auxsort(vm, t, f, lo, hi)?;
    }
    Ok(())
}

/// `luaB_sort` (`0x7fb920`): `1..getn`, so a remembered size past the elements compares their
/// nils; a comparator is checked only when one is given (`0x7fb95b`).
fn sort(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let n = getn(vm, t) as i64;
    let f = vm.arg(nargs, 1);
    if !f.is_nil() {
        vm.check_function(nargs, 2)?;
    }
    auxsort(vm, t, f, 1, n)?;
    Ok(0)
}
