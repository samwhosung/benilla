//! What a native function sees: 5.1's `lauxlib` checks and results, over [`Vm`].

use crate::heap::{FuncObj, NativeFn};
use crate::value::{StrRef, TableRef, Value, V};
use crate::vm::{LuaError, Vm};

pub type R<T> = Result<T, LuaError>;

/// 2^63, where a C conversion to a 64-bit integer runs out.
const TWO63: f64 = (1u64 << 63) as f64;

/// 5.1's `lua_number2integer` on x86-64: C's `(ptrdiff_t)` cast, `cvttsd2si`, which truncates
/// toward zero and answers `INT64_MIN` for a NaN or an out-of-range value. The x86-64 Linux C
/// build's answer on every platform (arm64 saturates). The client's MSVC x86-32 conversions are
/// later work.
pub fn c_integer(n: f64) -> i64 {
    if n.is_nan() || !(-TWO63..TWO63).contains(&n) {
        i64::MIN
    } else {
        n as i64
    }
}

/// 5.1's `luaL_checkint`: `(int)` of [`c_integer`], its low 32 bits. The client's `__ftol`
/// (`0x40a2b0`) answers the same: a `fistp` to a qword, rounding set to chop, and the low dword,
/// 0 for what the qword cannot hold.
pub fn c_int(n: f64) -> i32 {
    c_integer(n) as i32
}

/// The longest string the libraries build, `INT32_MAX` bytes: more than the 1.12 client's 32-bit
/// address space could hold. Deviation: the C build grows a string while `realloc` succeeds, and
/// a failure is the catchable `not enough memory`; Rust's allocator aborts the process instead,
/// and Linux grants far more than it can fill, so past this bound the libraries raise C's message.
pub const MAX_STRING: usize = i32::MAX as usize;

/// 5.1's `LUAI_MAXCSTACK`: the most values a native's frame may hold.
pub const MAX_C_STACK: usize = 8000;

impl Vm {
    /// 5.1's memory error: `not enough memory`, with no position (`luaD_throw(L, LUA_ERRMEM)`).
    /// It is thrown as it is: only `luaG_errormsg`, a runtime error, runs an `xpcall` handler.
    #[cold]
    pub fn memory_error(&mut self) -> LuaError {
        self.heap.oom = false;
        LuaError {
            value: Value::Str(self.heap.oom_str),
        }
    }

    /// Room in `buf` for `extra` more bytes of a string being built, within [`MAX_STRING`] and
    /// what the allocator grants; else [`Vm::memory_error`].
    #[inline]
    pub fn reserve(&mut self, buf: &mut Vec<u8>, extra: usize) -> R<()> {
        if extra <= buf.capacity() - buf.len() {
            return Ok(());
        }
        self.grow_str(buf, extra)
    }

    /// [`Vm::memory_error`] once `buf` holds [`MAX_STRING`] bytes, for a string that grows by
    /// short pieces.
    #[inline]
    pub fn check_len(&mut self, buf: &[u8]) -> R<()> {
        if buf.len() >= MAX_STRING {
            return Err(self.memory_error());
        }
        Ok(())
    }

    /// [`Vm::reserve`] past the capacity: doubling, as `Vec` grows, but never past
    /// [`MAX_STRING`].
    #[cold]
    #[inline(never)]
    fn grow_str(&mut self, buf: &mut Vec<u8>, extra: usize) -> R<()> {
        let need = buf.len().saturating_add(extra);
        if need > MAX_STRING {
            return Err(self.memory_error());
        }
        let want = need.max(buf.capacity().saturating_mul(2)).min(MAX_STRING);
        if buf.try_reserve_exact(want - buf.len()).is_err() {
            return Err(self.memory_error());
        }
        Ok(())
    }

    /// 5.1's `lua_checkstack` from a native: room for `n` more values within its frame's
    /// `LUAI_MAXCSTACK`, its arguments counted.
    pub fn check_stack(&self, n: usize) -> bool {
        n <= MAX_C_STACK && self.top - self.native_base() + n <= MAX_C_STACK
    }

    /// `luaL_checkany`.
    pub fn check_any(&mut self, nargs: usize, n: usize) -> R<Value> {
        if n > nargs {
            return Err(self.arg_error(n, "value expected"));
        }
        Ok(self.arg(nargs, n - 1))
    }

    /// `luaL_checktype(L, n, LUA_TTABLE)`.
    pub fn check_table(&mut self, nargs: usize, n: usize) -> R<TableRef> {
        match self.arg(nargs, n - 1).as_table() {
            Some(t) if n <= nargs => Ok(t),
            _ => Err(self.type_arg_error(nargs, n, "table")),
        }
    }

    /// `luaL_checktype(L, n, LUA_TFUNCTION)`.
    pub fn check_function(&mut self, nargs: usize, n: usize) -> R<Value> {
        let v = self.arg(nargs, n - 1);
        match v.as_func() {
            Some(_) if n <= nargs => Ok(v),
            _ => Err(self.type_arg_error(nargs, n, "function")),
        }
    }

    /// `luaL_checknumber`: a number or a numeric string.
    #[inline]
    pub fn check_number(&mut self, nargs: usize, n: usize) -> R<f64> {
        // A number, the common argument, here; a string or an error out of line. An argument
        // past `nargs` reads nil.
        if let Some(x) = self.arg(nargs, n - 1).as_num() {
            return Ok(x);
        }
        self.check_number_slow(nargs, n)
    }

    #[inline(never)]
    fn check_number_slow(&mut self, nargs: usize, n: usize) -> R<f64> {
        let v = self.arg(nargs, n - 1);
        match self.tonumber(v) {
            Some(x) if n <= nargs => Ok(x),
            _ => Err(self.type_arg_error(nargs, n, "number")),
        }
    }

    /// `luaL_optnumber`: absent or nil is the default.
    pub fn opt_number(&mut self, nargs: usize, n: usize, def: f64) -> R<f64> {
        if self.arg(nargs, n - 1).is_nil() {
            Ok(def)
        } else {
            self.check_number(nargs, n)
        }
    }

    pub fn check_int(&mut self, nargs: usize, n: usize) -> R<i32> {
        Ok(c_int(self.check_number(nargs, n)?))
    }

    pub fn opt_int(&mut self, nargs: usize, n: usize, def: i32) -> R<i32> {
        if self.arg(nargs, n - 1).is_nil() {
            Ok(def)
        } else {
            self.check_int(nargs, n)
        }
    }

    /// `luaL_checkinteger`: a `ptrdiff_t`.
    pub fn check_integer(&mut self, nargs: usize, n: usize) -> R<i64> {
        Ok(c_integer(self.check_number(nargs, n)?))
    }

    /// `luaL_optinteger`.
    pub fn opt_integer(&mut self, nargs: usize, n: usize, def: i64) -> R<i64> {
        if self.arg(nargs, n - 1).is_nil() {
            Ok(def)
        } else {
            self.check_integer(nargs, n)
        }
    }

    /// `luaL_checklstring`: a string, or a number converted in its stack slot as
    /// `lua_tolstring` converts it.
    #[inline]
    pub fn check_str(&mut self, nargs: usize, n: usize) -> R<StrRef> {
        // A string, the common argument, here: converting it in place would store it unchanged.
        // A number or an error out of line. An argument past `nargs` reads nil.
        if let Some(s) = self.arg(nargs, n - 1).as_str() {
            return Ok(s);
        }
        self.check_str_slow(nargs, n)
    }

    #[inline(never)]
    fn check_str_slow(&mut self, nargs: usize, n: usize) -> R<StrRef> {
        let v = self.arg(nargs, n - 1);
        if n <= nargs {
            if let Some(s) = self.tostr(v) {
                self.set_arg(n - 1, Value::Str(s));
                return Ok(s);
            }
        }
        Err(self.type_arg_error(nargs, n, "string"))
    }

    /// `luaL_optlstring`.
    pub fn opt_str(&mut self, nargs: usize, n: usize, def: &[u8]) -> R<StrRef> {
        if self.arg(nargs, n - 1).is_nil() {
            Ok(self.heap.intern(def))
        } else {
            self.check_str(nargs, n)
        }
    }

    /// Replace argument `i` (0-based) of the running native.
    pub fn set_arg(&mut self, i: usize, v: Value) {
        let b = self.native_base();
        self.stack[b + i] = v;
    }

    /// Push one result and answer the count, for `return Ok(vm.ret1(v))`.
    #[inline]
    pub fn ret1(&mut self, v: Value) -> usize {
        self.push(v);
        1
    }

    pub fn ret_str(&mut self, s: &[u8]) -> usize {
        let v = self.str(s);
        self.ret1(v)
    }

    /// A native closure with upvalues, which [`Vm::upvalue`] reads.
    pub fn new_closure(&mut self, name: &'static str, f: NativeFn, upvals: Vec<Value>) -> Value {
        let env = self.globals;
        Value::Func(self.heap.new_func(FuncObj::Native {
            f,
            host: None,
            name,
            upvals: upvals.into_boxed_slice(),
            env,
        }))
    }

    /// The running native closure's upvalue `i`.
    pub fn upvalue(&self, i: usize) -> Value {
        match self.heap.func(self.running()) {
            FuncObj::Native { upvals, .. } => upvals[i],
            _ => Value::Nil,
        }
    }

    pub fn set_upvalue(&mut self, i: usize, v: Value) {
        let f = self.running();
        if let FuncObj::Native { upvals, .. } = self.heap.func_mut(f) {
            upvals[i] = v;
        }
    }

    /// `luaL_getmetafield`: a field of `v`'s metatable, raw.
    pub fn metafield(&mut self, v: Value, name: &[u8]) -> Value {
        match self.heap.metatable(v) {
            Some(m) => {
                let k = self.str(name);
                self.heap.table(m).get(k)
            }
            None => Value::Nil,
        }
    }

    /// `lua_isstring`: a string or a number.
    pub fn is_string(&self, v: Value) -> bool {
        matches!(v.unpack(), V::Str(_) | V::Num(_))
    }
}

/// Room for `extra` more bytes in a string being built, within `MAX_STRING` and what the
/// allocator grants, growing as `Vec` grows: whether it was granted.
#[cold]
#[inline(never)]
pub(crate) fn grow_fallibly(buf: &mut Vec<u8>, extra: usize) -> bool {
    buf.len().saturating_add(extra) <= crate::api::MAX_STRING && buf.try_reserve(extra).is_ok()
}
