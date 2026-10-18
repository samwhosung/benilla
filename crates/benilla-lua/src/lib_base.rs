//! The base library: 5.1's `lbaselib.c`, whose messages the script host carries, with 1.12's
//! `collectgarbage` and `gcinfo` shapes.

use crate::api::{c_int, R};
use crate::heap::FuncObj;
use crate::value::{Value, V};
use crate::vm::Vm;

/// The client's base array (`0x811e28`), its 36 functions set as globals in the array's order
/// (`0x703ab0`). `host` stands in for the ones only the host gives, which it replaces.
pub fn open(vm: &mut Vm, host: crate::heap::NativeFn) {
    // `debugload` to `debugtimestamp` do nothing: `xor eax,eax; ret` (`0x7027e0`..`0x702830`).
    fn nothing(_: &mut Vm, _: usize) -> R<usize> {
        Ok(0)
    }
    vm.register("setglobal", setglobal);
    vm.register("getglobal", getglobal);
    for name in [
        "debugload",
        "debuginfo",
        "debugprint",
        "debugdump",
        "debugbreak",
        "debugtimestamp",
    ] {
        vm.register(name, nothing);
    }
    for name in [
        "debugprofilestart",
        "debugprofilestop",
        "seterrorhandler",
        "geterrorhandler",
    ] {
        vm.register(name, host);
    }
    vm.register("error", error);
    vm.register("getmetatable", getmetatable);
    vm.register("setmetatable", setmetatable);
    vm.register("getfenv", getfenv);
    vm.register("setfenv", setfenv);
    vm.register("next", next);
    vm.builtin_next = vm.get_global("next");
    vm.register("ipairs", ipairs);
    // `0x702da0`: the global `next`, read raw, as the generator.
    vm.register("pairs", pairs);
    vm.register("type", type_);
    vm.register("tonumber", tonumber);
    vm.register("tostring", tostring);
    vm.register("unpack", unpack);
    vm.register("rawequal", rawequal);
    vm.register("rawget", rawget);
    vm.register("rawset", rawset);
    vm.register("assert", assert);
    vm.register("gcinfo", gcinfo);
    vm.register("collectgarbage", collectgarbage);
    vm.register("loadstring", loadstring);
    vm.register("pcall", pcall);
    vm.register("xpcall", xpcall);
    for name in ["date", "time", "debugstack"] {
        vm.register(name, host);
    }
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

/// 5.0's `getfunc` (`0x702af0`): argument 1 as a function, or the function at that stack level,
/// the level optional (1) for both `getfenv` and `setfenv` (`luaL_optnumber`, `0x702b22`).
fn getfunc(vm: &mut Vm, nargs: usize) -> R<Value> {
    let a = vm.arg(nargs, 0);
    if let V::Func(_) = a.unpack() {
        return Ok(a);
    }
    let level = vm.opt_int(nargs, 1, 1)?;
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

/// `lua_getfenv` (`0x6f3d50`): a Lua function's environment; a native answers the thread's
/// globals.
fn env_of(vm: &Vm, f: Value) -> crate::value::TableRef {
    match f.unpack() {
        V::Func(f) => match vm.heap.func(f) {
            FuncObj::Lua(l) => l.env(),
            FuncObj::Native { .. } => vm.globals,
        },
        _ => vm.globals,
    }
}

/// 5.0's `aux_getfenv` (`0x702ba0`): the environment's raw `__fenv` field, nil when it has none.
fn protected_env(vm: &mut Vm, env: crate::value::TableRef) -> Value {
    let k = vm.str(b"__fenv");
    vm.heap.table(env).get(k)
}

/// 5.0's `luaB_getfenv` (`0x702ac0`): the function's environment, or its `__fenv` when it has one.
fn getfenv(vm: &mut Vm, nargs: usize) -> R<usize> {
    let f = getfunc(vm, nargs)?;
    let env = env_of(vm, f);
    let fenv = protected_env(vm, env);
    let v = if fenv.is_nil() {
        Value::Table(env)
    } else {
        fenv
    };
    Ok(vm.ret1(v))
}

/// 5.0's `luaB_setfenv` (`0x702be0`): an environment with a `__fenv` is refused (`0x702bff`); a
/// numeric 0 sets the thread's globals (`0x702c59`), else the function's environment is set, and
/// a native refuses (`0x702c70`). It answers nothing (`0x702c80`).
fn setfenv(vm: &mut Vm, nargs: usize) -> R<usize> {
    let t = vm.check_table(nargs, 2)?;
    let f = getfunc(vm, nargs)?;
    let env = env_of(vm, f);
    if !protected_env(vm, env).is_nil() {
        return Err(vm.error_at(1, "`setfenv' cannot change a protected environment"));
    }
    let first = vm.arg(nargs, 0);
    if vm.tonumber(first) == Some(0.0) {
        vm.globals = t;
        return Ok(0);
    }
    match f.unpack() {
        V::Func(fr) => {
            if !vm.set_function_env(fr, t) {
                return Err(vm.error_at(1, "`setfenv' cannot change environment of given function"));
            }
        }
        _ => unreachable!("getfunc answers a function"),
    }
    Ok(0)
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

/// `setglobal` (`0x702770`): argument 1 made a string in place (`lua_tostring`), a copy of it
/// pushed and moved under the top value (`lua_insert(-2)`), then `lua_settable` on the globals,
/// metamethods and all. So the global gets the top value: the last argument, or the name itself
/// when it is the only one, so `setglobal("x")` sets `x` to `"x"`. Nothing is answered.
fn setglobal(vm: &mut Vm, nargs: usize) -> R<usize> {
    let key = vm.tostr(vm.arg(nargs, 0)).map_or(Value::Nil, Value::Str);
    let val = if nargs >= 2 {
        vm.arg(nargs, nargs - 1)
    } else {
        key
    };
    let g = Value::Table(vm.globals);
    vm.setindex(g, key, val, None)?;
    Ok(0)
}

/// `getglobal` (`0x7027b0`): argument 1 made a string, then `lua_gettable` on the globals,
/// metamethods and all; one value.
fn getglobal(vm: &mut Vm, nargs: usize) -> R<usize> {
    let key = vm.tostr(vm.arg(nargs, 0)).map_or(Value::Nil, Value::Str);
    let g = Value::Table(vm.globals);
    let v = vm.index(g, key, None)?;
    Ok(vm.ret1(v))
}

/// 5.0's `ipairs` (`0x702cd0`). With no second argument (none, not nil) and so a count of 0, a
/// loop's start: the global `ipairs` read raw, the table and 0 (`0x702d15`..`0x702d41`); the
/// loop then calls what it was given, whatever the global holds later. Otherwise one step: the
/// count plus 1 and the table's raw value there, or nothing at a nil (`0x702d50`..`0x702d8e`).
pub(crate) fn ipairs(vm: &mut Vm, nargs: usize) -> R<usize> {
    let a2 = vm.arg(nargs, 1);
    let n = vm.tonumber(a2).unwrap_or(0.0);
    let t = vm.check_table(nargs, 1)?;
    if n == 0.0 && nargs < 2 {
        let g = vm.heap.table(vm.globals).get(Value::Str(vm.names.ipairs));
        vm.push(g);
        vm.push(Value::Table(t));
        vm.push(Value::Num(0.0));
        return Ok(3);
    }
    let n = n + 1.0;
    let v = vm.heap.table(t).get_int(crate::api::c_int(n));
    if v.is_nil() {
        return Ok(0);
    }
    vm.push(Value::Num(n));
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
        _ => vm.str(object_text(v).as_bytes()),
    })
}

/// `tostring` of a table, function or userdata: its type and MSVC's `%p`, eight uppercase hex
/// digits with no prefix (`0x74301d`, precision 8, the upper-case digits).
pub fn object_text(v: Value) -> String {
    format!("{}: {:08X}", v.type_name(), address(v))
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
        return Err(vm.rt_error(e.message()));
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
