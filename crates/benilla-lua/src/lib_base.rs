//! The base library: 5.1's `lbaselib.c`, whose messages the script host carries, with 1.12's
//! `collectgarbage` and `gcinfo` shapes.

use crate::api::{c_int, R};
use crate::heap::FuncObj;
use crate::value::{Value, V};
use crate::vm::Vm;

/// The base function benilla has under `name`, for a state that registers its own list.
pub(crate) fn native(name: &str) -> Option<crate::heap::NativeFn> {
    Some(match name {
        "error" => error,
        "getmetatable" => getmetatable,
        "setmetatable" => setmetatable,
        "getfenv" => getfenv,
        "setfenv" => setfenv,
        "next" => next,
        "pairs" => pairs,
        "tonumber" => tonumber,
        "tostring" => tostring,
        "type" => type_,
        "assert" => assert,
        "unpack" => unpack,
        "rawequal" => rawequal,
        "rawget" => rawget,
        "rawset" => rawset,
        "pcall" => pcall,
        "xpcall" => xpcall,
        "collectgarbage" => collectgarbage,
        "gcinfo" => gcinfo,
        "loadstring" => loadstring,
        _ => return None,
    })
}

/// 5.1's `pairs`, whose `next` is the one it was opened with (its upvalue), not the global's
/// value now.
pub(crate) fn pairs_51(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let next = vm.upvalue(0);
    vm.push(next);
    vm.push(Value::Table(t));
    vm.push(Value::Nil);
    Ok(3)
}

pub fn open(vm: &mut Vm) {
    vm.register("error", error);
    vm.register("getmetatable", getmetatable);
    vm.register("setmetatable", setmetatable);
    vm.register("getfenv", getfenv);
    vm.register("setfenv", setfenv);
    vm.register("next", next);
    vm.builtin_next = vm.get_global("next");
    let aux = vm.new_native("ipairs_aux", ipairs_aux);
    let ip = vm.new_closure("ipairs", ipairs, vec![aux]);
    vm.set_global("ipairs", ip);
    vm.register("pairs", pairs);
    vm.register("tonumber", tonumber);
    vm.register("tostring", tostring);
    vm.register("type", type_);
    vm.register("assert", assert);
    vm.register("unpack", unpack);
    vm.register("rawequal", rawequal);
    vm.register("rawget", rawget);
    vm.register("rawset", rawset);
    vm.register("pcall", pcall);
    vm.register("xpcall", xpcall);
    vm.register("collectgarbage", collectgarbage);
    vm.register("gcinfo", gcinfo);
    vm.register("loadstring", loadstring);
    let g = Value::Table(vm.globals);
    vm.set_global("_G", g);
}

/// `luaB_error`: a string message gets the position `level` up (default 1).
/// 5.1's `error`: no value raises nil.
fn error(vm: &mut Vm, nargs: usize) -> R<usize> {
    let level = vm.opt_int(nargs, 2, 1)?;
    let msg = vm.arg(nargs, 0);
    if vm.is_string(msg) && level > 0 {
        let mut out = vm.where_(level as usize);
        let s = vm.tostr(msg).unwrap();
        out.extend_from_slice(vm.heap.str(s));
        let v = vm.str(&out);
        return Err(vm.throw(v));
    }
    Err(vm.throw(msg))
}

fn getmetatable(vm: &mut Vm, nargs: usize) -> R<usize> {
    let v = vm.check_any(nargs, 1)?;
    let Some(m) = vm.heap.metatable(v) else {
        return Ok(vm.ret1(Value::Nil));
    };
    let protected = vm.metafield(v, b"__metatable");
    Ok(vm.ret1(if protected.is_nil() {
        Value::Table(m)
    } else {
        protected
    }))
}

fn setmetatable(vm: &mut Vm, nargs: usize) -> R<usize> {
    let mt = vm.arg(nargs, 1);
    let t = vm.check_table(nargs, 1)?;
    let mt = match mt.unpack() {
        // No second argument is neither nil nor a table.
        V::Nil if nargs >= 2 => None,
        V::Table(m) if nargs >= 2 => Some(m),
        _ => return Err(vm.arg_error(2, "nil or table expected")),
    };
    if !vm.metafield(Value::Table(t), b"__metatable").is_nil() {
        return Err(vm.error_at(1, "cannot change a protected metatable"));
    }
    vm.heap.table_mut(t).meta = mt;
    Ok(vm.ret1(Value::Table(t)))
}

/// 5.1's `getfunc`: argument 1 as a function, or the function at that stack level, the level
/// optional (1) for `getfenv` and required for `setfenv`.
fn getfunc(vm: &mut Vm, nargs: usize, opt: bool) -> R<Value> {
    let a = vm.arg(nargs, 0);
    if let V::Func(_) = a.unpack() {
        return Ok(a);
    }
    let level = if opt {
        vm.opt_int(nargs, 1, 1)?
    } else {
        vm.check_int(nargs, 1)?
    };
    if level < 0 {
        return Err(vm.arg_error(1, "level must be non-negative"));
    }
    match vm.function_at_level(level as usize) {
        Some(Some(f)) => Ok(Value::Func(f)),
        Some(None) => Err(vm.error_at(
            1,
            &format!("no function environment for tail call at level {level}"),
        )),
        None => Err(vm.arg_error(1, "invalid level")),
    }
}

/// 5.1's `luaB_getfenv`: a Lua function's environment; a native answers the thread's globals.
fn getfenv(vm: &mut Vm, nargs: usize) -> R<usize> {
    let f = getfunc(vm, nargs, true)?;
    let env = match f.unpack() {
        V::Func(f) => match vm.heap.func(f) {
            FuncObj::Lua(l) => l.env(),
            FuncObj::Native { .. } => vm.globals,
        },
        _ => vm.globals,
    };
    Ok(vm.ret1(Value::Table(env)))
}

/// 5.1's `luaB_setfenv`: a numeric 0 sets the thread's globals and answers nothing; else the
/// function's environment is set and the function answered; a native refuses.
fn setfenv(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 2)?;
    let f = getfunc(vm, nargs, false)?;
    let first = vm.arg(nargs, 0);
    if vm.tonumber(first) == Some(0.0) {
        vm.globals = t;
        return Ok(0);
    }
    match f.unpack() {
        V::Func(fr) => {
            if !vm.set_function_env(fr, t) {
                return Err(vm.error_at(1, "`setfenv' cannot change environment of given object"));
            }
        }
        _ => unreachable!("getfunc answers a function"),
    }
    Ok(vm.ret1(f))
}

/// `luaB_next`: `lua_next` replaces the key argument with the next key and pushes its value,
/// so the results sit where the key was, as a generic `for`'s registers then show.
fn next(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let k = vm.arg(nargs, 1);
    let step = vm.heap.table(t).next(k);
    let base = vm.native_base();
    match step {
        Ok(Some((k, v))) => {
            vm.top = base + 1;
            vm.push(k);
            vm.push(v);
            Ok(2)
        }
        Ok(None) => {
            vm.top = base + 1;
            Ok(vm.ret1(Value::Nil))
        }
        // `luaH_next`'s `luaG_runerror` (`0x6fa467`, text `0x871aac`).
        Err(crate::table::InvalidKey) => Err(vm.rt_error("invalid key for `next'")),
    }
}

/// 5.0's `pairs`: the global `next`, read raw at the call.
fn pairs(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let next = vm.heap.table(vm.globals).get(Value::Str(vm.names.next));
    vm.push(next);
    vm.push(Value::Table(t));
    vm.push(Value::Nil);
    Ok(3)
}

/// 5.1's `ipairs`: its iterator, the table and 0.
pub(crate) fn ipairs(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let aux = vm.upvalue(0);
    vm.push(aux);
    vm.push(Value::Table(t));
    vm.push(Value::Num(0.0));
    Ok(3)
}

/// 5.1's `ipairsaux`: the next index and its value, or nothing at the first nil.
pub(crate) fn ipairs_aux(vm: &mut Vm, nargs: usize) -> R<usize> {
    let i = vm.check_int(nargs, 2)?;
    let t = vm.check_table(nargs, 1)?;
    let i = i.wrapping_add(1);
    let v = vm.heap.table(t).get_int(i);
    if v.is_nil() {
        return Ok(0);
    }
    vm.push(Value::Num(i as f64));
    vm.push(v);
    Ok(2)
}

fn tonumber(vm: &mut Vm, nargs: usize) -> R<usize> {
    let base = vm.opt_int(nargs, 2, 10)?;
    if base == 10 {
        let v = vm.check_any(nargs, 1)?;
        if let Some(n) = vm.tonumber(v) {
            return Ok(vm.ret1(Value::Num(n)));
        }
    } else {
        let s = vm.check_str(nargs, 1)?;
        if !(2..=36).contains(&base) {
            return Err(vm.arg_error(2, "base out of range"));
        }
        if let Some(n) = strtoul(vm.heap.str(s), base as u32) {
            return Ok(vm.ret1(Value::Num(n as f64)));
        }
    }
    Ok(vm.ret1(Value::Nil))
}

/// The client's `tonumber` with a base (`0x702e9b`-`0x702f11`): its 32-bit `strtoul` must read a
/// digit, then only spaces may follow, up to the C string's end; the value is pushed unsigned.
fn strtoul(s: &[u8], base: u32) -> Option<u32> {
    let s = &s[..s.iter().position(|&b| b == 0).unwrap_or(s.len())];
    let (n, end) = crate::number::strtoul(s, base);
    (end > 0 && s[end..].iter().all(|&b| matches!(b, 0x20 | 0x09..=0x0D))).then_some(n)
}

/// A synthetic address for `tostring`: stable per object, in MSVC's `%p` shape.
fn address(v: Value) -> u32 {
    let (kind, id) = match v.unpack() {
        V::Table(r) => (1u32, r.0),
        V::Func(r) => (2, r.0),
        V::User(r) => (3, r.0),
        V::Light(p) => return p as u32,
        _ => (0, 0),
    };
    0x1000_0000u32
        .wrapping_add(kind << 24)
        .wrapping_add(id.wrapping_mul(0x20))
}

pub fn tostring_value(vm: &mut Vm, v: Value) -> R<Value> {
    let tm = vm.metafield(v, b"__tostring");
    if !tm.is_nil() {
        let r = vm.call_value(tm, &[v], 1)?;
        return Ok(r[0]);
    }
    Ok(match v.unpack() {
        V::Num(n) => crate::number::with_g14(n, |b| vm.str(b)),
        V::Str(_) => v,
        V::Bool(b) => vm.str(if b { b"true" } else { b"false" }),
        V::Nil => vm.str(b"nil"),
        _ => {
            let s = format!("{}: {:08X}", v.type_name(), address(v));
            vm.str(s.as_bytes())
        }
    })
}

fn tostring(vm: &mut Vm, nargs: usize) -> R<usize> {
    let v = vm.check_any(nargs, 1)?;
    let s = tostring_value(vm, v)?;
    Ok(vm.ret1(s))
}

fn type_(vm: &mut Vm, nargs: usize) -> R<usize> {
    let v = vm.check_any(nargs, 1)?;
    Ok(vm.ret_str(v.type_name().as_bytes()))
}

/// 1.12's `assert` (`0x7031a0`): one result, the first argument.
fn assert(vm: &mut Vm, nargs: usize) -> R<usize> {
    let v = vm.check_any(nargs, 1)?;
    if !v.truthy() {
        let msg = vm.opt_str(nargs, 2, b"assertion failed!")?;
        // `luaL_error(L, "%s", msg)` (`0x7031d0`): the bytes, up to a NUL.
        let m = vm.heap.str(msg);
        let m = m[..m.iter().position(|&b| b == 0).unwrap_or(m.len())].to_vec();
        return Err(vm.error_at_bytes(1, &m));
    }
    Ok(vm.ret1(v))
}

/// `checkint` (`0x6f4f80`): `lua_tonumber` (`0x6f4f8a`) and `__ftol`'s low word, so a numeric
/// string counts; -1 for a value that is no number (`lua_isnumber`, `0x6f4f9f`).
fn checkint(vm: &Vm, v: Value) -> i32 {
    vm.tonumber(v).map_or(-1, c_int)
}

/// `luaL_getn` (`0x6f5050`): `t.n` if `checkint` takes it, else the size `setn` remembered, else
/// a count to the first nil.
pub fn getn(vm: &mut Vm, t: crate::value::TableRef) -> usize {
    let n = checkint(vm, vm.heap.table(t).get(Value::Str(vm.names.n)));
    if n >= 0 {
        return n as usize;
    }
    if let Some(n) = vm.heap.table(t).setn.filter(|&n| n >= 0) {
        return n as usize;
    }
    vm.heap.table(t).border()
}

/// `luaL_setn` (`0x6f4ea0`): into `t.n`, as a number, when `checkint` takes the one there, else the
/// remembered size; either keeps a negative size as given.
pub fn setn(vm: &mut Vm, t: crate::value::TableRef, n: i32) {
    let key = Value::Str(vm.names.n);
    if checkint(vm, vm.heap.table(t).get(key)) >= 0 {
        vm.heap
            .table_mut(t)
            .set(key, Value::Num(f64::from(n)))
            .unwrap();
    } else {
        vm.heap.table_mut(t).setn = Some(n);
    }
}

/// 5.0's `LUA_MAXCSTACK`: the client's `lua_checkstack` (`0x6f2f30`) refuses once the slots in
/// use plus those asked for pass it (`cmp ecx,0x800` at `0x6f2f43`).
const MAXCSTACK_50: usize = 0x800;

/// `luaB_unpack` (`0x7030a0`): items 1 to `getn(t)`, every argument after the table unread,
/// after `luaL_checkstack(n, "table too big to unpack")` (`0x7030c8`, text `0x872bc4`), whose
/// slots in use are the arguments.
fn unpack(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let n = getn(vm, t);
    if vm.top - vm.native_base() + n > MAXCSTACK_50 {
        // `luaL_checkstack` (`0x6f4b20`): `luaL_error` with `stack overflow (%s)` (`0x8716b0`).
        return Err(vm.error_at(1, "stack overflow (table too big to unpack)"));
    }
    for k in 1..=n {
        let v = vm.heap.table(t).get_int(k as i32);
        vm.push(v);
    }
    Ok(n)
}

fn rawequal(vm: &mut Vm, nargs: usize) -> R<usize> {
    let a = vm.check_any(nargs, 1)?;
    let b = vm.check_any(nargs, 2)?;
    Ok(vm.ret1(Value::Bool(a.raw_eq(b))))
}

fn rawget(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let k = vm.check_any(nargs, 2)?;
    let v = vm.heap.table(t).get(k);
    Ok(vm.ret1(v))
}

fn rawset(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 1)?;
    let k = vm.check_any(nargs, 2)?;
    let v = vm.check_any(nargs, 3)?;
    if let Err(e) = vm.heap.table_mut(t).set(k, v) {
        let msg = match e {
            crate::table::KeyError::Nil => "table index is nil",
            crate::table::KeyError::NaN => "table index is NaN",
        };
        return Err(vm.rt_error(msg));
    }
    Ok(vm.ret1(Value::Table(t)))
}

fn pcall(vm: &mut Vm, nargs: usize) -> R<usize> {
    vm.check_any(nargs, 1)?;
    let func_idx = vm.top - nargs;
    match vm.pcall_at(func_idx, nargs - 1, -1, None) {
        Ok(()) => {
            let n = vm.top - func_idx;
            vm.ensure(vm.top + 1);
            vm.stack.copy_within(func_idx..vm.top, func_idx + 1);
            vm.stack[func_idx] = Value::Bool(true);
            vm.top += 1;
            Ok(n + 1)
        }
        Err(e) => {
            vm.push(Value::Bool(false));
            vm.push(e.value);
            Ok(2)
        }
    }
}

fn xpcall(vm: &mut Vm, nargs: usize) -> R<usize> {
    vm.check_any(nargs, 2)?;
    let f = vm.arg(nargs, 0);
    let h = vm.arg(nargs, 1);
    let func_idx = vm.top - nargs;
    vm.top = func_idx;
    vm.push(f);
    match vm.pcall_at(func_idx, 0, -1, Some(h)) {
        Ok(()) => {
            let n = vm.top - func_idx;
            vm.ensure(vm.top + 1);
            vm.stack.copy_within(func_idx..vm.top, func_idx + 1);
            vm.stack[func_idx] = Value::Bool(true);
            vm.top += 1;
            Ok(n + 1)
        }
        Err(e) => {
            vm.push(Value::Bool(false));
            vm.push(e.value);
            Ok(2)
        }
    }
}

/// 1.12's `collectgarbage([kb])` (`0x703250`): sets 5.0's threshold, answers nothing. The count
/// is `__ftol`'s low word (`0x703261`), which `lua_setgcthreshold` (`0x6f4400`) compares unsigned:
/// past `0x3fffff` KB, a negative count among them, the threshold is `0xffffffff` bytes; a count
/// the quadword cannot hold, a NaN or an infinity, is 0. A count at the threshold collects.
fn collectgarbage(vm: &mut Vm, nargs: usize) -> R<usize> {
    let kb = c_int(vm.opt_number(nargs, 1, 0.0)?) as u32;
    vm.set_gc_threshold(kb);
    Ok(0)
}

/// 1.12's `gcinfo()` (`0x703200`): live and threshold kilobytes.
fn gcinfo(vm: &mut Vm, _nargs: usize) -> R<usize> {
    let kb = vm.heap.bytes() >> 10;
    vm.push(Value::Num(kb as f64));
    vm.push(Value::Num((vm.heap.threshold() >> 10) as f64));
    Ok(2)
}

/// `luaB_loadstring` (`0x703280`): the chunk named by its own source unless named (`0x70329a`),
/// one value on success and nil and the message on failure (`0x7032d8`). The client's
/// `luaL_loadbuffer` strips a UTF-8 BOM (`0x6f5699`), after the name was taken, and 5.0's lexer
/// skips a leading `#` line.
fn loadstring(vm: &mut Vm, nargs: usize) -> R<usize> {
    let s = vm.check_str(nargs, 1)?;
    let src = vm.heap.str(s).to_vec();
    let name = match vm.arg(nargs, 1) {
        Value::Nil => src.clone(),
        _ => {
            let n = vm.check_str(nargs, 2)?;
            vm.heap.str(n).to_vec()
        }
    };
    let body = src.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&src);
    let body: &[u8] = match body.first() {
        Some(b'#') => match body.iter().position(|&b| b == b'\n') {
            Some(nl) => &body[nl..],
            None => &[],
        },
        _ => body,
    };
    match vm.load(body, &name) {
        Ok(f) => Ok(vm.ret1(f)),
        Err(e) => {
            vm.push(Value::Nil);
            let m = vm.str(&e.0);
            vm.push(m);
            Ok(2)
        }
    }
}
