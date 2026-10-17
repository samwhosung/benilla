//! The slice of mlua 0.11's API that benilla uses, over [`benilla_lua`]'s VM for the 1.12 dialect:
//! a crate that answers to the name `mlua`, so code written against mlua runs on benilla's own
//! interpreter unchanged.
//!
//! A [`Lua`] owns one VM. Handles to tables, functions and strings ([`Table`], [`Function`],
//! [`String`]) are counted collector roots, as mlua's registry references are, and hold their
//! state weakly, as mlua 0.11's do: a host closure that captures one makes no cycle, and a handle
//! used after its state is gone panics. A Rust callback runs with the `&mut Vm` the interpreter
//! hands it; while it runs, every access reaches the VM through that reference, so one chain of
//! borrows covers each access, and a handle lets go of its root through the root table alone,
//! never the VM. A callback's error is raised in Lua as a 1.12 C function raises one, through
//! `luaL_error`'s rules ([`raise`]); a callback's panic is raised as an error value `pcall` can
//! catch, and resumes as a panic if it reaches Rust, as mlua's does. A Lua error that reaches
//! Rust is an [`Error::LuaValue`] with that value and its text.

use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::c_void;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::sync::Arc;

use benilla_lua::heap::HostFn;
use benilla_lua::value::V;
use benilla_lua::vm::{LuaError, RootTable, Vm};

mod conv;
mod error;

pub use conv::{FromLua, FromLuaMulti, IntoLua, IntoLuaMulti};
pub use error::{Error, ErrorContext, ExternalError, Result};

/// The interpreter underneath, for code that works below this API.
pub use benilla_lua;

pub type Integer = i64;
pub type Number = f64;

pub mod prelude {
    pub use crate::{
        Error as LuaError, FromLua, FromLuaMulti, Function as LuaFunction, IntoLua, IntoLuaMulti,
        Lua, MultiValue as LuaMultiValue, Result as LuaResult, String as LuaString,
        Table as LuaTable, Value as LuaValue,
    };
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The state
// ─────────────────────────────────────────────────────────────────────────────────────────────

pub(crate) struct Inner {
    /// The VM, owned here (`Box::into_raw`, freed in `drop`) and reached through `cur`. A raw
    /// pointer, not a `Box`: a `Box` asserts unique access, which every reference `cur` makes
    /// would contradict.
    owned: *mut Vm,
    /// The VM every access goes through: the owned one, or the `&mut Vm` a running callback was
    /// handed.
    cur: Cell<*mut Vm>,
    app: RefCell<AppMap>,
    /// Shared by a state and its forks: a host closure the fork copied still holds handles made
    /// on the original, and while a callback of the family runs they resolve through its VM.
    family: u64,
    /// The VM is lent out ([`Lua::with_vm`]): until it is back, this state's API answers only
    /// inside a callback the lent VM runs.
    lent: Cell<bool>,
    /// Callbacks of this state running now, outermost first.
    depth: Cell<u32>,
}

const LENT: &str =
    "the Lua state's VM is lent out (`Lua::with_vm`); reach it through that reference";

/// The host data, by type: callbacks read it on every call.
type AppMap = HashMap<TypeId, Rc<dyn Any>, std::hash::BuildHasherDefault<TypeIdHasher>>;

/// A `TypeId` is already a hash of its type, so the map takes its bits as they are.
#[derive(Default)]
struct TypeIdHasher(u64);

impl std::hash::Hasher for TypeIdHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(b);
        }
    }

    fn write_u64(&mut self, x: u64) {
        self.0 ^= x;
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // SAFETY: `owned` came from `Box::into_raw` in `Lua::over` and is freed only here; no
        // reference to the VM outlives the last `Lua`, which is going.
        let vm = unsafe { Box::from_raw(self.owned) };
        let id = vm.vm_id();
        drop(vm);
        // Its panics' error values went with it. `try_with`: the thread may be ending.
        let _ = PANICS.try_with(|p| p.borrow_mut().retain(|_, e| e.2 != id));
    }
}

thread_local! {
    /// The family and VM of the callback running now, if any.
    static RUNNING: Cell<(u64, *mut Vm)> = const { Cell::new((0, std::ptr::null_mut())) };
    /// Each panic a callback raised as a Lua error, by its error value's tag: the payload until
    /// it resumes in Rust, and its text for `tostring`. An entry a Lua `pcall` swallowed goes
    /// once the collector frees its error value, or with its VM.
    static PANICS: RefCell<BTreeMap<u64, Panic>> = const { RefCell::new(BTreeMap::new()) };
}

/// A panic raised as a Lua error: its payload until it resumes, its text, and the VM and
/// userdata that carry it, by which an entry whose error value is gone is pruned.
type Panic = (
    Option<Box<dyn Any + Send>>,
    std::string::String,
    u64,
    benilla_lua::value::UserRef,
);

static NEXT_FAMILY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The userdata tags of panic values, in a range no host object uses.
const PANIC_TAG: u64 = 0xBE11_0000_0000_0000;
static NEXT_PANIC: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 5.1's `LUAI_MAXCSTACK`: a callback's arguments and results share this many stack slots, as
/// `lua_checkstack` allows a C function.
const MAXCSTACK: usize = 8000;

/// Run `f` as a callback of `lua`'s family on `vm`: handles of the family reach `vm`. The VM in
/// effect before is restored however `f` ends, a panic included.
#[inline]
fn in_callback<T>(lua: &Lua, vm: *mut Vm, f: impl FnOnce() -> T) -> T {
    struct Restore<'a> {
        lua: &'a Lua,
        prev: *mut Vm,
        prev_running: (u64, *mut Vm),
    }
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            RUNNING.set(self.prev_running);
            self.lua.0.cur.set(self.prev);
            self.lua.0.depth.set(self.lua.0.depth.get() - 1);
        }
    }
    lua.0.depth.set(lua.0.depth.get() + 1);
    let _restore = Restore {
        lua,
        prev: lua.0.cur.replace(vm),
        prev_running: RUNNING.replace((lua.0.family, vm)),
    };
    f()
}

/// A callback's panic as a Lua error: a userdata whose `tostring` is the panic's text, as mlua's
/// wrapped panic is. The payload waits in `PANICS` and resumes if the error reaches Rust.
fn raise_panic(vm: &mut Vm, payload: Box<dyn Any + Send>) -> LuaError {
    let text = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<std::string::String>() {
        s.clone()
    } else {
        "<panic>".to_owned()
    };
    let tag = PANIC_TAG | NEXT_PANIC.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let meta = panic_meta(vm);
    let u = vm.new_user(tag, Some(meta));
    let V::User(r) = u.unpack() else {
        unreachable!("a userdata")
    };
    let id = vm.vm_id();
    PANICS.with(|p| {
        let mut p = p.borrow_mut();
        // The entries of this VM whose error value the collector freed go first.
        p.retain(|&t, e| e.2 != id || vm.heap.user_tag(e.3) == Some(t));
        p.insert(tag, (Some(payload), text, id, r));
    });
    vm.throw(u)
}

/// The metatable of panic values, made once per VM and kept in its registry.
fn panic_meta(vm: &mut Vm) -> benilla_lua::value::TableRef {
    let key = vm.str(b"benilla-mlua panic");
    let reg = vm.registry;
    if let Some(t) = vm.heap.table(reg).get(key).as_table() {
        return t;
    }
    fn tostring(vm: &mut Vm, nargs: usize) -> std::result::Result<usize, LuaError> {
        let text = match vm.arg(nargs, 0).unpack() {
            V::User(u) => {
                let tag = vm.heap.user(u).tag;
                PANICS.with(|p| p.borrow().get(&tag).map(|e| e.1.clone()))
            }
            _ => None,
        };
        let s = vm.str(text.unwrap_or_else(|| "<panic>".to_owned()).as_bytes());
        vm.push(s);
        Ok(1)
    }
    let meta = vm.heap.new_table(0, 1);
    let f = vm.new_native("__tostring", tostring);
    let name = vm.str(b"__tostring");
    vm.heap.table_mut(meta).set(name, f).expect("a string key");
    vm.heap
        .table_mut(reg)
        .set(key, benilla_lua::Value::Table(meta))
        .expect("a string key");
    meta
}

/// Panics raised as Lua errors this thread still keeps, for a test that they are let go.
#[doc(hidden)]
pub fn kept_panics() -> usize {
    PANICS.with(|p| p.borrow().len())
}

/// The panic a Lua error value carries, if it is one whose payload has not resumed yet.
fn take_panic(vm: &Vm, e: benilla_lua::Value) -> Option<Box<dyn Any + Send>> {
    let V::User(u) = e.unpack() else {
        return None;
    };
    let tag = vm.heap.user(u).tag;
    if tag & PANIC_TAG != PANIC_TAG {
        return None;
    }
    PANICS.with(|p| p.borrow_mut().get_mut(&tag).and_then(|e| e.0.take()))
}

/// A callback's error raised in Lua as the client's C functions raise theirs. A message goes
/// through `luaL_error` (`0x6f4940`), which prefixes `luaL_where(1)` (`0x6f48e0`), the calling
/// Lua function's `chunk:line: `; a bad argument is worded as `luaL_argerror` (`0x6f4810`) and
/// `luaL_typerror` (`0x6f48a0`) word it, named as the caller called the function; a Lua error
/// that came back through the callback, or an [`Error::bare`] one, is raised as it is, as
/// `lua_error` (`0x6f4440`) raises a value.
fn raise(lua: &Lua, nargs: usize, e: Error) -> LuaError {
    let vm = lua.vm();
    match e {
        Error::LuaValue {
            value: Some(v),
            message,
        } => {
            // Another family's value, or one gone with its thread, raises its text.
            let x = match v.get(lua) {
                Some(v) => lua.unwrap(&v),
                None => vm.str(message.as_bytes()),
            };
            vm.throw(x)
        }
        Error::LuaValue {
            message,
            value: None,
        } => {
            let msg = vm.str(message.as_bytes());
            vm.throw(msg)
        }
        Error::SyntaxError { raw, .. } => {
            let msg = vm.str(&raw);
            vm.throw(msg)
        }
        // 5.0's memory error carries no position.
        Error::MemoryError(_) => vm.memory_error(),
        Error::CallbackError { cause, .. } | Error::WithContext { cause, .. } => {
            raise(lua, nargs, (*cause).clone())
        }
        Error::BadArgument { pos, cause, .. } => match &*cause {
            // `luaL_checktype` and the `luaL_check*` family: `T expected, got U`, `no value` for
            // an argument past the last.
            Error::FromLuaConversionError { to, message, .. }
                if message.as_deref() != Some("out of range") =>
            {
                match lua_type(to) {
                    Some(t) => vm.type_arg_error(nargs, pos, t),
                    None => vm.arg_error(pos, &cause.lua_message()),
                }
            }
            other => vm.arg_error(pos, &other.lua_message()),
        },
        other => vm.error_at(1, &other.lua_message()),
    }
}

/// The Lua type a conversion's Rust target reads, as `luaT_typenames` (`0x811cd0`) spells it.
fn lua_type(rust: &str) -> Option<&'static str> {
    Some(match rust {
        "table" | "Vec" | "HashMap" | "BTreeMap" => "table",
        "function" => "function",
        "string" | "String" => "string",
        "lightuserdata" => "userdata",
        "i8" | "u8" | "i16" | "u16" | "i32" | "u32" | "i64" | "u64" | "i128" | "u128" | "isize"
        | "usize" | "f32" | "f64" => "number",
        _ => return None,
    })
}

thread_local! {
    /// Each Lua error value an [`Error::LuaValue`] carries, by its tag: the value, rooted, and
    /// the family of the state that made it. An entry goes when the last copy of its error does.
    static RAISED: RefCell<HashMap<u64, (u64, Value)>> = RefCell::new(HashMap::new());
}

static NEXT_RAISED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The Lua value an [`Error::LuaValue`] carries, held out of the error on its state's thread so
/// the error stays `Send` and `Sync` and prints without its state: the error holds only this tag.
#[derive(Clone)]
pub struct ErrorValue(Arc<RaisedTag>);

struct RaisedTag(u64);

impl Drop for RaisedTag {
    fn drop(&mut self) {
        // `try_with`: the thread may be ending. The value drops outside the borrow.
        let gone = RAISED
            .try_with(|r| r.borrow_mut().remove(&self.0))
            .ok()
            .flatten();
        drop(gone);
    }
}

impl std::fmt::Debug for ErrorValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ErrorValue({})", self.0 .0)
    }
}

impl ErrorValue {
    fn new(family: u64, v: Value) -> ErrorValue {
        let tag = NEXT_RAISED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        RAISED.with(|r| r.borrow_mut().insert(tag, (family, v)));
        ErrorValue(Arc::new(RaisedTag(tag)))
    }

    /// The value, for a state of the family that made it, on the thread that made it.
    pub fn get(&self, lua: &Lua) -> Option<Value> {
        RAISED.with(|r| {
            r.borrow()
                .get(&self.0 .0)
                .filter(|(family, _)| *family == lua.0.family)
                .map(|(_, v)| v.clone())
        })
    }
}

impl Error {
    /// The error as Lua sees it, as bytes: a string or number value it carries as written, a
    /// syntax error as the compiler wrote it, else [`Error`]'s message text.
    pub fn lua_bytes(&self, lua: &Lua) -> Vec<u8> {
        match self {
            Error::SyntaxError { raw, .. } => raw.clone(),
            Error::CallbackError { cause, .. } | Error::WithContext { cause, .. } => {
                cause.lua_bytes(lua)
            }
            _ => match self.lua_value(lua) {
                Some(Value::String(s)) => s.as_bytes().to_vec(),
                _ => self.lua_message().into_bytes(),
            },
        }
    }

    /// The Lua value this error carries, when it came out of Lua: [`ErrorValue::get`].
    pub fn lua_value(&self, lua: &Lua) -> Option<Value> {
        match self {
            Error::LuaValue { value: Some(v), .. } => v.get(lua),
            Error::CallbackError { cause, .. } | Error::WithContext { cause, .. } => {
                cause.lua_value(lua)
            }
            _ => None,
        }
    }
}

const DESTROYED: &str = "Lua instance is destroyed";
const FOREIGN: &str = "Lua instance passed Value created from a different main Lua state";

/// A Lua state: a cheap handle, cloned freely, as mlua's is.
#[derive(Clone)]
pub struct Lua(pub(crate) Rc<Inner>);

impl Default for Lua {
    fn default() -> Self {
        Lua::new()
    }
}

impl Lua {
    /// A state with Lua 5.1's standard libraries open, as mlua's `Lua::new()` opens them.
    pub fn new() -> Lua {
        Lua::over(
            Box::new(benilla_lua::new_script_host_state()),
            NEXT_FAMILY.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// A state over `vm`, which learns its host here.
    fn over(vm: Box<Vm>, family: u64) -> Lua {
        let ptr: *mut Vm = Box::into_raw(vm);
        let lua = Lua(Rc::new(Inner {
            owned: ptr,
            cur: Cell::new(ptr),
            app: RefCell::new(AppMap::default()),
            family,
            lent: Cell::new(false),
            depth: Cell::new(0),
        }));
        let host: std::rc::Weak<dyn Any> = Rc::downgrade(&lua.0) as std::rc::Weak<dyn Any>;
        // Host code the VM runs from inside (a freed closure's destructors) runs as a callback
        // does, on the `&mut Vm` the VM hands down.
        let reentry: benilla_lua::vm::Reentry = Rc::new(|vm: &mut Vm, f: &mut dyn FnMut()| {
            let inner = vm.host.as_ref().and_then(std::rc::Weak::upgrade);
            match inner.and_then(|i| i.downcast::<Inner>().ok()) {
                Some(inner) => in_callback(&Lua(inner), vm, f),
                None => f(),
            }
        });
        let vm = lua.vm();
        vm.host = Some(host);
        vm.reentry = Some(reentry);
        lua
    }

    /// The state a host closure runs in: the one the VM it was handed belongs to, so a forked
    /// VM's closures reach the fork.
    fn of_vm(vm: &Vm) -> Lua {
        let any = vm
            .host
            .as_ref()
            .and_then(std::rc::Weak::upgrade)
            .expect("a VM called back after its Lua state was dropped");
        Lua(any.downcast::<Inner>().expect("a benilla-mlua VM"))
    }

    /// An independent copy of this state: the whole VM, every value and closure, with no host
    /// data; the caller copies the host data it needs (`set_app_data`). A snapshot to restore
    /// from, or a template to start many runs from.
    ///
    /// # Panics
    ///
    /// Inside a callback of this state: the copy would hold the running calls' frames, which
    /// nothing would ever return from.
    pub fn fork(&self) -> Lua {
        assert!(
            self.vm().frames_empty(),
            "a state is forked from outside its callbacks"
        );
        let vm: Vm = self.vm().clone();
        Lua::over(Box::new(vm), self.0.family)
    }

    /// The VM in effect. Every use is short: a reference from here is not held across anything
    /// that can run Lua or reach the VM another way, except a call on the reference itself (the
    /// VM hands its own `&mut` down to a callback, which is where `RUNNING` points), so no two
    /// live references alias. A handle's root count is kept in its `RootTable`, never through
    /// here, so a handle dropped while the VM is borrowed (a host closure the collector frees)
    /// makes no second reference.
    #[allow(clippy::mut_from_ref)]
    #[inline]
    pub(crate) fn vm(&self) -> &mut Vm {
        let (family, running) = RUNNING.get();
        let vm = if family == self.0.family {
            running
        } else {
            assert!(!self.0.lent.get() || self.0.depth.get() > 0, "{LENT}");
            self.0.cur.get()
        };
        // SAFETY: `vm` is the owned VM or a callback's live `&mut Vm`; the facade never keeps
        // the reference past the operation it serves.
        unsafe { &mut *vm }
    }

    /// The interpreter itself, lent to `f`, for code that works below this API: the heap, the
    /// profile, hot reload. While `f` runs, any other use of this state's API panics instead of
    /// reaching the VM a second way; Lua that `f` runs through the reference reaches its
    /// callbacks as usual, and they use the API as any callback does.
    ///
    /// Panics while a callback of this state runs, whose VM the interpreter is using, another
    /// state's callback under it included: the API stays open to a running callback, so a lend
    /// then would leave two ways in.
    pub fn with_vm<R>(&self, f: impl FnOnce(&mut Vm) -> R) -> R {
        assert!(
            RUNNING.get().0 != self.0.family && self.0.depth.get() == 0,
            "`Lua::with_vm` while a callback of the same state runs"
        );
        let vm = self.vm();
        assert!(!self.0.lent.replace(true), "{LENT}");
        struct Return<'a>(&'a Cell<bool>);
        impl Drop for Return<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _back = Return(&self.0.lent);
        f(vm)
    }

    // ── values ───────────────────────────────────────────────────────────────────────────────

    /// A rooted handle to `v` in the VM in effect, owned by that VM's state.
    fn make_ref(&self, v: benilla_lua::Value) -> RefH {
        let vm = self.vm();
        let id = vm.root(v);
        let roots = vm.root_table();
        RefH {
            serial: roots.serial(id),
            roots,
            owner: vm.host.clone().expect("a benilla-mlua VM"),
            family: self.0.family,
            id,
        }
    }

    /// The value a handle names, for this state. A handle names a value in the state that made
    /// it; while a callback of its family runs, in the VM that callback runs on, which is how a
    /// fork's copy of a host closure reaches the fork's copy of what it captured. Anything else
    /// is another state's value and panics, as mlua's does.
    fn unref(&self, r: &RefH) -> benilla_lua::Value {
        let own = std::ptr::eq(
            r.owner.as_ptr() as *const u8,
            Rc::as_ptr(&self.0) as *const u8,
        );
        let in_family_callback = r.family == self.0.family && RUNNING.get().0 == self.0.family;
        assert!(own || in_family_callback, "{FOREIGN}");
        r.get()
    }

    /// A VM value as an API value: collectables become rooted handles, and an integral number
    /// in `i64`'s range becomes `Integer`, as mlua reads a 5.1 number (`num_traits::cast`, then
    /// the bits compared: `-0` and `2^63` stay `Number`).
    pub(crate) fn wrap(&self, v: benilla_lua::Value) -> Value {
        match v.unpack() {
            V::Nil => Value::Nil,
            V::Bool(b) => Value::Boolean(b),
            V::Num(n) => {
                const TWO63: f64 = 9_223_372_036_854_775_808.0;
                let i = n as i64;
                if (-TWO63..TWO63).contains(&n) && (i as f64).to_bits() == n.to_bits() {
                    Value::Integer(i)
                } else {
                    Value::Number(n)
                }
            }
            V::Str(_) => Value::String(String(self.make_ref(v))),
            V::Table(_) => Value::Table(Table(self.make_ref(v))),
            V::Func(_) => Value::Function(Function(self.make_ref(v))),
            V::User(u) => Value::LightUserData(LightUserData(u.0 as usize as *mut c_void)),
            V::Light(p) => Value::LightUserData(LightUserData(p as *mut c_void)),
        }
    }

    /// An API value as a VM value.
    pub(crate) fn unwrap(&self, v: &Value) -> benilla_lua::Value {
        match v {
            Value::Nil => benilla_lua::Value::Nil,
            Value::Boolean(b) => benilla_lua::Value::Bool(*b),
            Value::Integer(i) => benilla_lua::Value::Num(*i as f64),
            Value::Number(n) => benilla_lua::Value::Num(*n),
            Value::String(s) => self.unref(&s.0),
            Value::Table(t) => self.unref(&t.0),
            Value::Function(f) => self.unref(&f.0),
            Value::LightUserData(l) => benilla_lua::Value::Light(l.0 as usize),
            Value::Thread(_) | Value::Error(_) => benilla_lua::Value::Nil,
        }
    }

    pub fn globals(&self) -> Table {
        let g = self.vm().globals;
        Table(self.make_ref(benilla_lua::Value::Table(g)))
    }

    pub fn create_table(&self) -> Result<Table> {
        let t = self.vm().heap.new_table(0, 0);
        Ok(Table(self.make_ref(benilla_lua::Value::Table(t))))
    }

    pub fn create_table_with_capacity(&self, narr: usize, nrec: usize) -> Result<Table> {
        // `lua_createtable(L, narr, nrec)`.
        let t = self.vm().heap.new_table(narr, nrec);
        Ok(Table(self.make_ref(benilla_lua::Value::Table(t))))
    }

    /// A table with room for the iterator's lower bound of keys, set raw in order, as mlua's.
    pub fn create_table_from<K: IntoLua, V2: IntoLua>(
        &self,
        iter: impl IntoIterator<Item = (K, V2)>,
    ) -> Result<Table> {
        let iter = iter.into_iter();
        let t = self.create_table_with_capacity(0, iter.size_hint().0)?;
        for (k, v) in iter {
            t.raw_set(k, v)?;
        }
        Ok(t)
    }

    /// A table with an array part of the iterator's lower bound, `1..` set raw, as mlua's.
    pub fn create_sequence_from<T: IntoLua>(
        &self,
        iter: impl IntoIterator<Item = T>,
    ) -> Result<Table> {
        let iter = iter.into_iter();
        let t = self.create_table_with_capacity(iter.size_hint().0, 0)?;
        for (i, v) in iter.enumerate() {
            t.raw_set(i + 1, v)?;
        }
        Ok(t)
    }

    /// `s` interned, with no handle: [`IntoLua::into_raw`] of a string.
    pub(crate) fn raw_string(&self, s: &[u8]) -> Result<benilla_lua::Value> {
        let v = self.vm().str(s);
        if self.vm().heap.take_oom() {
            return Err(Error::MemoryError("not enough memory".into()));
        }
        Ok(v)
    }

    /// `x` as a VM value, without a handle when its type allows (`IntoLua::into_raw`), else
    /// through one, which the second field holds for as long as the caller keeps it.
    pub(crate) fn raw_value<T: IntoLua>(
        &self,
        x: T,
    ) -> Result<(benilla_lua::Value, Option<Value>)> {
        match x.into_raw(self) {
            Ok(r) => Ok((r?, None)),
            Err(x) => {
                let v = x.into_lua(self)?;
                Ok((self.unwrap(&v), Some(v)))
            }
        }
    }

    /// A key and a value as VM values, converted in that order as `raw_value` converts them. A
    /// key read without a handle is a collector root while the value's conversion, which may
    /// run Lua, is made through `into_lua`.
    #[allow(clippy::type_complexity)]
    pub(crate) fn raw_pair<K: IntoLua, V2: IntoLua>(
        &self,
        key: K,
        value: V2,
    ) -> Result<((benilla_lua::Value, benilla_lua::Value), [Option<Value>; 2])> {
        let (kx, k) = self.raw_value(key)?;
        let (vx, v) = match value.into_raw(self) {
            Ok(r) => (r?, None),
            Err(value) => {
                let hold = k.is_none().then(|| self.vm().root(kx));
                let v = value.into_lua(self);
                if let Some(h) = hold {
                    self.vm().unroot(h);
                }
                let v = v?;
                (self.unwrap(&v), Some(v))
            }
        };
        Ok(((kx, vx), [k, v]))
    }

    pub fn create_string(&self, s: impl AsRef<[u8]>) -> Result<String> {
        let v = self.vm().str(s.as_ref());
        if self.vm().heap.take_oom() {
            return Err(Error::MemoryError("not enough memory".into()));
        }
        Ok(String(self.make_ref(v)))
    }

    /// A Lua function over a Rust closure.
    pub fn create_function<F, A, R>(&self, func: F) -> Result<Function>
    where
        F: Fn(&Lua, A) -> Result<R> + 'static,
        A: FromLuaMulti,
        R: IntoLuaMulti,
    {
        let host: HostFn = Rc::new(move |vm: &mut Vm, nargs: usize| {
            let lua = Lua::of_vm(vm);
            in_callback(&lua, vm, || {
                // A panic becomes a Lua error here, as mlua's `callback_error` makes one, so it
                // never unwinds through the interpreter.
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // The arguments stay on the VM's stack, rooted, for the whole call; a copy
                    // is read, since converting one reaches the VM. Most calls take a few.
                    let mut few = [benilla_lua::Value::Nil; 8];
                    let many;
                    let args: &[benilla_lua::Value] = if nargs <= few.len() {
                        few[..nargs].copy_from_slice(lua.vm().args(nargs));
                        &few[..nargs]
                    } else {
                        many = lua.vm().args(nargs).to_vec();
                        &many
                    };
                    let a = A::from_raw_args(args, &lua)?;
                    // Pushed inside the guard: another state's value panics here.
                    let n = func(&lua, a)?.push_results(&lua)?;
                    // mlua's `check_stack` for the results, against 5.1's `LUAI_MAXCSTACK`.
                    if nargs + n >= MAXCSTACK {
                        return Err(Error::StackError);
                    }
                    Ok(n)
                }));
                let out = match res {
                    Ok(Ok(n)) => Ok(n),
                    Ok(Err(e)) => Err(raise(&lua, nargs, e)),
                    Err(payload) => Err(raise_panic(lua.vm(), payload)),
                };
                out
            })
        });
        let f = self.vm().new_host("?", host);
        Ok(Function(self.make_ref(f)))
    }

    /// A Lua function over one of the interpreter's own natives, such as
    /// [`benilla_lua::bit_library`]'s: it runs on the VM directly, as a C function of the
    /// client's library runs, with the interpreter's argument checks and messages.
    pub fn create_native(
        &self,
        name: &'static str,
        f: benilla_lua::heap::NativeFn,
    ) -> Result<Function> {
        let v = self.vm().new_native(name, f);
        Ok(Function(self.make_ref(v)))
    }

    /// mlua's `create_function_mut`: the closure behind a `RefCell`.
    pub fn create_function_mut<F, A, R>(&self, func: F) -> Result<Function>
    where
        F: FnMut(&Lua, A) -> Result<R> + 'static,
        A: FromLuaMulti,
        R: IntoLuaMulti,
    {
        let func = RefCell::new(func);
        self.create_function(move |lua, a| {
            let mut f = func
                .try_borrow_mut()
                .map_err(|_| Error::runtime("mutable callback called recursively"))?;
            f(lua, a)
        })
    }

    /// Load a chunk. Unnamed, it is named after the Rust call site, as mlua names it.
    #[track_caller]
    pub fn load(&self, chunk: impl AsChunk) -> Chunk<'_> {
        let at = std::panic::Location::caller();
        Chunk {
            lua: self,
            source: chunk.source(),
            name: chunk
                .name()
                .unwrap_or_else(|| format!("@{}:{}", at.file(), at.line()))
                .into_bytes(),
            env: None,
        }
    }

    // ── host data ────────────────────────────────────────────────────────────────────────────

    pub fn set_app_data<T: 'static>(&self, data: T) -> Option<T> {
        let old = self
            .0
            .app
            .borrow_mut()
            .insert(TypeId::of::<T>(), Rc::new(RefCell::new(data)));
        old.and_then(|o| {
            Rc::try_unwrap(o.downcast::<RefCell<T>>().ok()?)
                .ok()
                .map(RefCell::into_inner)
        })
    }

    fn app_cell<T: 'static>(&self) -> Option<Rc<RefCell<T>>> {
        let app = self.0.app.borrow();
        let any = app.get(&TypeId::of::<T>())?.clone();
        any.downcast::<RefCell<T>>().ok()
    }

    /// A shared borrow of the host's `T`; panics while it is mutably borrowed, as mlua does.
    pub fn app_data_ref<T: 'static>(&self) -> Option<AppDataRef<'_, T>> {
        let cell = self.app_cell::<T>()?;
        // SAFETY: the cell lives in `app` until replaced or removed, and the guard borrows `self`.
        let r: Ref<'_, T> = unsafe { (*Rc::as_ptr(&cell)).borrow() };
        Some(AppDataRef { r, _cell: cell })
    }

    /// A mutable borrow of the host's `T`; panics while it is borrowed, as mlua does.
    pub fn app_data_mut<T: 'static>(&self) -> Option<AppDataRefMut<'_, T>> {
        let cell = self.app_cell::<T>()?;
        // SAFETY: as `app_data_ref`.
        let r: RefMut<'_, T> = unsafe { (*Rc::as_ptr(&cell)).borrow_mut() };
        Some(AppDataRefMut { r, _cell: cell })
    }

    pub fn remove_app_data<T: 'static>(&self) -> Option<T> {
        let old = self.0.app.borrow_mut().remove(&TypeId::of::<T>())?;
        Rc::try_unwrap(old.downcast::<RefCell<T>>().ok()?)
            .ok()
            .map(RefCell::into_inner)
    }

    // ── the registry ─────────────────────────────────────────────────────────────────────────

    fn registry_key(&self, name: &str) -> benilla_lua::Value {
        self.vm().str(name.as_bytes())
    }

    pub fn named_registry_value<T: FromLua>(&self, name: &str) -> Result<T> {
        let k = self.registry_key(name);
        let vm = self.vm();
        let v = vm.heap.table(vm.registry).get(k);
        T::from_lua(self.wrap(v), self)
    }

    pub fn set_named_registry_value(&self, name: &str, value: impl IntoLua) -> Result<()> {
        let v = value.into_lua(self)?;
        let k = self.registry_key(name);
        let x = self.unwrap(&v);
        let vm = self.vm();
        let reg = vm.registry;
        vm.heap
            .table_mut(reg)
            .set(k, x)
            .map_err(|_| Error::runtime("bad registry key"))
    }

    pub fn unset_named_registry_value(&self, name: &str) -> Result<()> {
        self.set_named_registry_value(name, Value::Nil)
    }

    // ── coercion ─────────────────────────────────────────────────────────────────────────────

    /// `lua_tonumber`: a number, or a string that reads as one.
    pub fn coerce_number(&self, v: Value) -> Result<Option<Number>> {
        Ok(match &v {
            Value::Integer(i) => Some(*i as f64),
            Value::Number(n) => Some(*n),
            Value::String(s) => self.vm().tonumber(s.0.get()),
            _ => None,
        })
    }

    /// mlua's `coerce_integer` on 5.1: an `Integer` as it is, else `lua_tointegerx`'s compat
    /// rule, the number truncated if it is within `EPSILON` of that.
    pub fn coerce_integer(&self, v: Value) -> Result<Option<Integer>> {
        if let Value::Integer(i) = v {
            return Ok(Some(i));
        }
        Ok(self
            .coerce_number(v)?
            .and_then(|n| ((n - (n as i64) as f64).abs() < f64::EPSILON).then_some(n as i64)))
    }

    /// `lua_tostring`: a string, or a number written as one.
    pub fn coerce_string(&self, v: Value) -> Result<Option<String>> {
        Ok(match v {
            Value::String(s) => Some(s),
            Value::Integer(_) | Value::Number(_) => {
                let x = self.unwrap(&v);
                let s = self.vm().tostr(x).expect("a number has a string form");
                Some(String(self.make_ref(benilla_lua::Value::Str(s))))
            }
            _ => None,
        })
    }

    pub fn pack_multi(&self, args: impl IntoLuaMulti) -> Result<MultiValue> {
        args.into_lua_multi(self)
    }

    pub fn unpack<T: FromLua>(&self, value: Value) -> Result<T> {
        T::from_lua(value, self)
    }

    pub fn pack(&self, t: impl IntoLua) -> Result<Value> {
        t.into_lua(self)
    }

    // ── the collector ────────────────────────────────────────────────────────────────────────

    pub fn used_memory(&self) -> usize {
        self.vm().heap.bytes()
    }

    pub fn gc_collect(&self) -> Result<()> {
        self.vm().full_gc();
        Ok(())
    }

    /// 1.12 has no type metatables, and this VM none to set: a string never has one.
    pub fn set_type_metatable<T>(&self, _metatable: Option<Table>) {}

    // ── debugging ────────────────────────────────────────────────────────────────────────────

    /// The stack level `level` up, for a traceback: `f` reads it.
    pub fn inspect_stack<R>(&self, level: usize, f: impl FnOnce(&Debug) -> R) -> Option<R> {
        let info = self.vm().frame_info(level)?;
        Some(f(&Debug { info, lua: self }))
    }

    /// Run `callback` once for every `triggers.every_nth_instruction` instructions, counted as
    /// 5.1 counts them, at the next loop back edge, call or return rather than the exact
    /// instruction (`benilla_lua::vm::Hook`). An error it answers is no Lua error, as mlua's is:
    /// it aborts the run to the host, past every protected call (`Vm::abort`).
    pub fn set_hook<F>(&self, triggers: HookTriggers, callback: F) -> Result<()>
    where
        F: Fn(&Lua, &Debug) -> Result<VmState> + 'static,
    {
        let step = triggers.every_nth_instruction.unwrap_or(1_000_000) as u64;
        self.vm().set_hook(
            step,
            Rc::new(move |vm: &mut Vm| {
                let lua = Lua::of_vm(vm);
                in_callback(&lua, vm, || {
                    let info = lua.vm().frame_info(0).unwrap_or_default();
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        callback(&lua, &Debug { info, lua: &lua })
                    }));
                    let out = match r {
                        Ok(Ok(_)) => Ok(()),
                        Ok(Err(e)) => Err(lua.vm().abort(e.lua_message().as_bytes())),
                        Err(payload) => Err(raise_panic(lua.vm(), payload)),
                    };
                    out
                })
            }),
        );
        Ok(())
    }

    pub fn remove_hook(&self) {
        self.vm().remove_hook();
    }

    /// Call `f` protected, from the host: its results, or its error.
    pub(crate) fn call_raw(&self, f: benilla_lua::Value, args: MultiValue) -> Result<MultiValue> {
        let argv: Vec<benilla_lua::Value> = args.iter().map(|v| self.unwrap(v)).collect();
        // The arguments' handles stay alive until the values are on the VM's stack.
        let r = self.vm().pcall_value(f, &argv, -1);
        drop(args);
        match r {
            Ok(vals) => Ok(vals.into_iter().map(|v| self.wrap(v)).collect()),
            Err(e) => Err(self.error_from(e)),
        }
    }

    /// Call `func`, protected and with no arguments, while each named global holds the value
    /// paired with it, each earlier value put back after: the 1.12 script handler's convention
    /// (`Vm::call_with_globals`) in one VM call, without a handle per global read or written.
    pub fn call_with_globals(&self, func: &Function, globals: &[(&str, Value)]) -> Result<()> {
        let f = self.unref(&func.0);
        let sets: Vec<(benilla_lua::value::StrRef, benilla_lua::Value)> = globals
            .iter()
            .map(|(name, v)| (self.vm().heap.intern(name.as_bytes()), self.unwrap(v)))
            .collect();
        self.vm()
            .call_with_globals(f, &sets)
            .map_err(|e| self.error_from(e))
    }

    /// A Lua error value as an `Error`: its text, as mlua reports a runtime error. A callback's
    /// panic resumes here, as mlua's `pop_error` resumes it.
    pub(crate) fn error_from(&self, e: benilla_lua::Value) -> Error {
        if let Some(payload) = take_panic(self.vm(), e) {
            std::panic::resume_unwind(payload);
        }
        Error::LuaValue {
            message: self.vm().display(e),
            value: Some(ErrorValue::new(self.0.family, self.wrap(e))),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Handles
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A counted collector root: mlua's registry reference. It holds its state weakly, as mlua
/// 0.11's `WeakLua`, and counts itself in the root table it was made in, which outlives the VM.
pub(crate) struct RefH {
    roots: Rc<RootTable>,
    /// The state whose VM made the root.
    owner: std::rc::Weak<dyn Any>,
    family: u64,
    id: u32,
    /// The root's serial, which a fork's copy keeps (`RootTable::get_as`).
    serial: u64,
}

impl RefH {
    /// The VM the handle reads: a running callback's of its family, else its own state's.
    #[inline]
    fn vm(&self) -> *mut Vm {
        let (family, running) = RUNNING.get();
        if family == self.family {
            return running;
        }
        let owner = self.owner.upgrade().expect(DESTROYED);
        let inner = owner.downcast_ref::<Inner>().expect("a benilla-mlua state");
        assert!(!inner.lent.get() || inner.depth.get() > 0, "{LENT}");
        // Another holder keeps the state alive past this temporary one: the upgrade succeeded.
        inner.cur.get()
    }

    #[inline]
    pub(crate) fn get(&self) -> benilla_lua::Value {
        // A fork's VM has the roots made before the fork, by serial; a root made after it, in
        // either state, names nothing in the other, which is another state's value.
        // SAFETY: `vm` is live (see `Lua::vm`), and the read ends here.
        unsafe { (*self.vm()).rooted_as(self.id, self.serial) }.expect(FOREIGN)
    }

    /// The state the handle acts in: as `vm`.
    pub(crate) fn lua(&self) -> Lua {
        let (family, running) = RUNNING.get();
        if family == self.family {
            // SAFETY: a running callback's VM, read for its host link alone.
            return Lua::of_vm(unsafe { &*running });
        }
        let owner = self.owner.upgrade().expect(DESTROYED);
        Lua(owner.downcast::<Inner>().expect("a benilla-mlua state"))
    }
}

impl Clone for RefH {
    fn clone(&self) -> Self {
        self.roots.retain(self.id);
        RefH {
            roots: self.roots.clone(),
            owner: self.owner.clone(),
            family: self.family,
            id: self.id,
            serial: self.serial,
        }
    }
}

impl Drop for RefH {
    fn drop(&mut self) {
        self.roots.unroot(self.id);
    }
}

impl PartialEq for RefH {
    fn eq(&self, other: &Self) -> bool {
        self.get().raw_eq(other.get())
    }
}

impl std::fmt::Debug for RefH {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.get())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table(pub(crate) RefH);

#[derive(Clone, Debug, PartialEq)]
pub struct Function(pub(crate) RefH);

#[derive(Clone, Debug)]
pub struct String(pub(crate) RefH);

/// A coroutine; 1.12 has none, so no value ever is one.
#[derive(Clone, Debug, PartialEq)]
pub struct Thread(());

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LightUserData(pub *mut c_void);

#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    Nil,
    Boolean(bool),
    LightUserData(LightUserData),
    Integer(Integer),
    Number(Number),
    String(String),
    Table(Table),
    Function(Function),
    Thread(Thread),
    Error(Box<Error>),
}

pub use Value::Nil;

impl Value {
    pub const NULL: Value = Value::LightUserData(LightUserData(std::ptr::null_mut()));

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Boolean(_) => "boolean",
            Value::LightUserData(_) => "lightuserdata",
            Value::Integer(_) => "integer",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Table(_) => "table",
            Value::Function(_) => "function",
            Value::Thread(_) => "thread",
            Value::Error(_) => "error",
        }
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    pub fn is_boolean(&self) -> bool {
        matches!(self, Value::Boolean(_))
    }

    pub fn is_number(&self) -> bool {
        matches!(self, Value::Number(_) | Value::Integer(_))
    }

    pub fn is_string(&self) -> bool {
        matches!(self, Value::String(_))
    }

    pub fn is_table(&self) -> bool {
        matches!(self, Value::Table(_))
    }

    pub fn is_function(&self) -> bool {
        matches!(self, Value::Function(_))
    }

    pub fn as_boolean(&self) -> Option<bool> {
        match self {
            Value::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<Integer> {
        match self {
            Value::Integer(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<Number> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Integer(i) => Some(*i as f64),
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_integer()
    }

    pub fn as_str(&self) -> Option<BorrowedStr<'_>> {
        match self {
            Value::String(s) => s.to_str().ok(),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&String> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&Table> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }

    pub fn as_function(&self) -> Option<&Function> {
        match self {
            Value::Function(f) => Some(f),
            _ => None,
        }
    }

    pub fn as_light_userdata(&self) -> Option<LightUserData> {
        match self {
            Value::LightUserData(l) => Some(*l),
            _ => None,
        }
    }

    /// A pointer naming the object, for identity.
    pub fn to_pointer(&self) -> *const c_void {
        match self {
            Value::String(s) => s.to_pointer(),
            Value::Table(t) => t.to_pointer(),
            Value::Function(f) => f.to_pointer(),
            Value::LightUserData(l) => l.0,
            _ => std::ptr::null(),
        }
    }

    /// `tostring`, without metamethods.
    pub fn to_string(&self) -> Result<std::string::String> {
        Ok(match self {
            Value::Nil => "nil".into(),
            Value::Boolean(b) => b.to_string(),
            Value::Integer(i) => i.to_string(),
            Value::Number(n) => benilla_lua::fmt_g14(*n),
            Value::String(s) => s.to_string_lossy(),
            other => format!("{}: {:p}", other.type_name(), other.to_pointer()),
        })
    }

    pub fn equals(&self, other: &Value) -> Result<bool> {
        Ok(self == other)
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Boolean(a), Value::Boolean(b)) => a == b,
            (Value::LightUserData(a), Value::LightUserData(b)) => a == b,
            (Value::Integer(a), Value::Integer(b)) => a == b,
            (Value::Integer(a), Value::Number(b)) | (Value::Number(b), Value::Integer(a)) => {
                *a as f64 == *b
            }
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::String(a), Value::String(b)) => a.0 == b.0,
            (Value::Table(a), Value::Table(b)) => a == b,
            (Value::Function(a), Value::Function(b)) => a == b,
            _ => false,
        }
    }
}

fn pointer_of(r: &RefH) -> *const c_void {
    let v = r.get();
    // A synthetic address per object: its type and handle.
    let (kind, id): (usize, usize) = match v.unpack() {
        V::Table(t) => (1, t.0 as usize),
        V::Func(f) => (2, f.0 as usize),
        V::Str(s) => (3, s.id as usize),
        _ => (0, 0),
    };
    ((kind << 40) | (id << 4) | 8) as *const c_void
}

// ── tables ───────────────────────────────────────────────────────────────────────────────────

impl Table {
    fn lua(&self) -> Lua {
        self.0.lua()
    }

    fn tref(&self) -> benilla_lua::value::TableRef {
        self.0.get().as_table().expect("a table handle")
    }

    /// `t[key] = value`, with metamethods.
    pub fn set(&self, key: impl IntoLua, value: impl IntoLua) -> Result<()> {
        let lua = &self.lua();
        let ((kx, vx), _held) = lua.raw_pair(key, value)?;
        let t = self.0.get();
        lua.vm()
            .protected(|vm| vm.setindex(t, kx, vx, None))
            .map_err(|e| lua.error_from(e))
    }

    /// `t[key]`, with metamethods.
    pub fn get<V2: FromLua>(&self, key: impl IntoLua) -> Result<V2> {
        let lua = &self.lua();
        let (kx, _held) = lua.raw_value(key)?;
        let t = self.0.get();
        // The key goes to an `__index` function as an argument, which roots it.
        let v = lua
            .vm()
            .protected(|vm| vm.index(t, kx, None))
            .map_err(|e| lua.error_from(e))?;
        V2::from_raw(v, lua)
    }

    pub fn contains_key(&self, key: impl IntoLua) -> Result<bool> {
        Ok(!self.get::<Value>(key)?.is_nil())
    }

    pub fn raw_set(&self, key: impl IntoLua, value: impl IntoLua) -> Result<()> {
        let lua = &self.lua();
        let ((kx, vx), _held) = lua.raw_pair(key, value)?;
        let t = self.tref();
        lua.vm().heap.table_mut(t).set(kx, vx).map_err(|e| match e {
            benilla_lua::table::KeyError::Nil => Error::runtime("table index is nil"),
            benilla_lua::table::KeyError::NaN => Error::runtime("table index is NaN"),
        })
    }

    pub fn raw_get<V2: FromLua>(&self, key: impl IntoLua) -> Result<V2> {
        let lua = &self.lua();
        let (kx, _held) = lua.raw_value(key)?;
        let v = lua.vm().heap.table(self.tref()).get(kx);
        V2::from_raw(v, lua)
    }

    /// mlua's `raw_remove`: an integer key in `1..=#t` shifts the rest down, as `table.remove`
    /// does; any other key is set to nil.
    pub fn raw_remove(&self, key: impl IntoLua) -> Result<()> {
        let lua = &self.lua();
        match key.into_lua(lua)? {
            Value::Integer(idx) => {
                let size = self.raw_len() as Integer;
                if idx < 1 || idx > size {
                    return Err(Error::runtime("index out of bounds"));
                }
                for i in idx..size {
                    let v: Value = self.raw_get(i + 1)?;
                    self.raw_set(i, v)?;
                }
                self.raw_set(size, Value::Nil)
            }
            key => self.raw_set(key, Value::Nil),
        }
    }

    /// `lua_objlen`: 5.1's `luaH_getn`.
    pub fn raw_len(&self) -> usize {
        self.lua().vm().heap.table(self.tref()).length()
    }

    /// mlua's `len` on 5.1: `#t`, or with a metatable its compat `luaL_len`, which calls a
    /// `__len` and takes its answer as `lua_tointegerx` does.
    pub fn len(&self) -> Result<Integer> {
        let lua = &self.lua();
        let vm = lua.vm();
        let Some(meta) = vm.heap.table(self.tref()).meta else {
            return Ok(self.raw_len() as Integer);
        };
        let key = vm.str(b"__len");
        let len = vm.heap.table(meta).get(key);
        if len.is_nil() {
            return Ok(self.raw_len() as Integer);
        }
        let t = self.0.get();
        let r = lua
            .vm()
            .protected(|vm| vm.call_value(len, &[t], 1))
            .map_err(|e| lua.error_from(e))?;
        let n = lua.wrap(r[0]);
        lua.coerce_integer(n)?
            .ok_or_else(|| Error::runtime("object length is not an integer"))
    }

    pub fn is_empty(&self) -> bool {
        let lua = &self.lua();
        matches!(
            lua.vm()
                .heap
                .table(self.tref())
                .next(benilla_lua::Value::Nil),
            Ok(None)
        )
    }

    /// mlua's `push`: with a metatable, at `len() + 1` through `__newindex`; else raw.
    pub fn push(&self, value: impl IntoLua) -> Result<()> {
        if !self.has_metatable() {
            return self.raw_push(value);
        }
        let n = self.len()?;
        self.set(n + 1, value)
    }

    pub fn raw_push(&self, value: impl IntoLua) -> Result<()> {
        let n = self.raw_len();
        self.raw_set(n + 1, value)
    }

    /// mlua's `pop`: with a metatable, `t[len()]` through `__index`, then set to nil through
    /// `__newindex`, `0` included; else raw.
    pub fn pop<V2: FromLua>(&self) -> Result<V2> {
        if !self.has_metatable() {
            return self.raw_pop();
        }
        let n = self.len()?;
        let v: Value = self.get(n)?;
        self.set(n, Value::Nil)?;
        V2::from_lua(v, &self.lua())
    }

    /// mlua's `raw_pop`: `t[#t]`, then `t[#t] = nil` raw, which on an empty table sets key 0.
    pub fn raw_pop<V2: FromLua>(&self) -> Result<V2> {
        let n = self.raw_len();
        let v: Value = self.raw_get(n)?;
        self.raw_set(n, Value::Nil)?;
        V2::from_lua(v, &self.lua())
    }

    /// mlua's `raw_insert`: `idx` in `1..=#t + 1`, the value converted first, the rest shifted
    /// up raw.
    pub fn raw_insert(&self, idx: Integer, value: impl IntoLua) -> Result<()> {
        let size = self.raw_len() as Integer;
        if idx < 1 || idx > size + 1 {
            return Err(Error::runtime("index out of bounds"));
        }
        let value = value.into_lua(&self.lua())?;
        for i in (idx..=size).rev() {
            let v: Value = self.raw_get(i)?;
            self.raw_set(i + 1, v)?;
        }
        self.raw_set(idx, value)
    }

    pub fn clear(&self) -> Result<()> {
        let keys: Vec<Value> = self
            .pairs::<Value, Value>()
            .map(|r| r.map(|(k, _)| k))
            .collect::<Result<_>>()?;
        for k in keys {
            self.raw_set(k, Value::Nil)?;
        }
        Ok(())
    }

    pub fn metatable(&self) -> Option<Table> {
        let lua = &self.lua();
        let m = lua.vm().heap.table(self.tref()).meta?;
        Some(Table(lua.make_ref(benilla_lua::Value::Table(m))))
    }

    pub fn get_metatable(&self) -> Option<Table> {
        self.metatable()
    }

    pub fn set_metatable(&self, metatable: Option<Table>) -> Result<()> {
        let lua = &self.lua();
        let m = metatable.and_then(|m| lua.unref(&m.0).as_table());
        let t = self.tref();
        lua.vm().heap.table_mut(t).meta = m;
        Ok(())
    }

    pub fn has_metatable(&self) -> bool {
        self.metatable().is_some()
    }

    pub fn equals(&self, other: &Table) -> Result<bool> {
        let lua = &self.lua();
        let (a, b) = (self.0.get(), lua.unref(&other.0));
        lua.vm()
            .protected(|vm| vm.equals(a, b))
            .map_err(|e| lua.error_from(e))
    }

    pub fn to_pointer(&self) -> *const c_void {
        pointer_of(&self.0)
    }

    /// The pairs `next` walks, each converted.
    pub fn pairs<K: FromLua, V2: FromLua>(&self) -> TablePairs<K, V2> {
        TablePairs {
            table: self.clone(),
            key: Some(Value::Nil),
            _p: std::marker::PhantomData,
        }
    }

    pub fn for_each<K: FromLua, V2: FromLua>(
        &self,
        mut f: impl FnMut(K, V2) -> Result<()>,
    ) -> Result<()> {
        for kv in self.pairs::<K, V2>() {
            let (k, v) = kv?;
            f(k, v)?;
        }
        Ok(())
    }

    /// `t[1]`, `t[2]` … to the first nil.
    pub fn sequence_values<V2: FromLua>(&self) -> TableSequence<V2> {
        TableSequence {
            table: self.clone(),
            index: 1,
            _p: std::marker::PhantomData,
        }
    }

    pub fn for_each_value<V2: FromLua>(&self, mut f: impl FnMut(V2) -> Result<()>) -> Result<()> {
        for v in self.sequence_values::<V2>() {
            f(v?)?;
        }
        Ok(())
    }
}

pub struct TablePairs<K, V2> {
    table: Table,
    key: Option<Value>,
    _p: std::marker::PhantomData<(K, V2)>,
}

impl<K: FromLua, V2: FromLua> Iterator for TablePairs<K, V2> {
    type Item = Result<(K, V2)>;

    fn next(&mut self) -> Option<Self::Item> {
        let key = self.key.take()?;
        let lua = self.table.lua();
        let kx = lua.unwrap(&key);
        let next = lua.vm().heap.table(self.table.tref()).next(kx);
        match next {
            Ok(Some((k, v))) => {
                let (k, v) = (lua.wrap(k), lua.wrap(v));
                self.key = Some(k.clone());
                Some(K::from_lua(k, &lua).and_then(|k| Ok((k, V2::from_lua(v, &lua)?))))
            }
            Ok(None) => None,
            Err(_) => Some(Err(Error::runtime("invalid key for `next'"))),
        }
    }
}

pub struct TableSequence<V2> {
    table: Table,
    index: usize,
    _p: std::marker::PhantomData<V2>,
}

impl<V2: FromLua> Iterator for TableSequence<V2> {
    type Item = Result<V2>;

    fn next(&mut self) -> Option<Self::Item> {
        let v: Value = match self.table.raw_get(self.index) {
            Ok(v) => v,
            Err(e) => return Some(Err(e)),
        };
        if v.is_nil() {
            return None;
        }
        self.index += 1;
        Some(V2::from_lua(v, &self.table.lua()))
    }
}

/// mlua's `ObjectLike`, which a table answers.
pub trait ObjectLike {
    fn get<V2: FromLua>(&self, key: impl IntoLua) -> Result<V2>;
    fn set(&self, key: impl IntoLua, value: impl IntoLua) -> Result<()>;
    fn call<R: FromLuaMulti>(&self, args: impl IntoLuaMulti) -> Result<R>;
    fn call_method<R: FromLuaMulti>(&self, name: &str, args: impl IntoLuaMulti) -> Result<R>;
    fn call_function<R: FromLuaMulti>(&self, name: &str, args: impl IntoLuaMulti) -> Result<R>;
    fn get_path<V2: FromLua>(&self, path: &str) -> Result<V2>;
    fn to_string(&self) -> Result<std::string::String>;
    fn to_value(&self) -> Value;
}

impl ObjectLike for Table {
    fn get<V2: FromLua>(&self, key: impl IntoLua) -> Result<V2> {
        Table::get(self, key)
    }

    fn set(&self, key: impl IntoLua, value: impl IntoLua) -> Result<()> {
        Table::set(self, key, value)
    }

    fn call<R: FromLuaMulti>(&self, args: impl IntoLuaMulti) -> Result<R> {
        let lua = &self.lua();
        let mut a = args.into_lua_multi(lua)?;
        a.push_front(Value::Table(self.clone()));
        let r = lua.call_raw(self.0.get(), a)?;
        R::from_lua_multi(r, lua)
    }

    fn call_method<R: FromLuaMulti>(&self, name: &str, args: impl IntoLuaMulti) -> Result<R> {
        let f: Function = Table::get(self, name)?;
        let lua = &self.lua();
        let mut a = args.into_lua_multi(lua)?;
        a.push_front(Value::Table(self.clone()));
        f.call(a)
    }

    fn call_function<R: FromLuaMulti>(&self, name: &str, args: impl IntoLuaMulti) -> Result<R> {
        let f: Function = Table::get(self, name)?;
        f.call(args)
    }

    fn get_path<V2: FromLua>(&self, path: &str) -> Result<V2> {
        let mut cur = Value::Table(self.clone());
        for part in path.split('.') {
            cur = match cur {
                Value::Table(t) => Table::get(&t, part)?,
                _ => Value::Nil,
            };
        }
        V2::from_lua(cur, &self.lua())
    }

    fn to_string(&self) -> Result<std::string::String> {
        let lua = &self.lua();
        let t = self.0.get();
        let s = lua
            .vm()
            .protected(|vm| benilla_lua::tostring_value(vm, t))
            .map_err(|e| lua.error_from(e))?;
        Ok(lua.vm().display(s))
    }

    fn to_value(&self) -> Value {
        Value::Table(self.clone())
    }
}

// ── functions ────────────────────────────────────────────────────────────────────────────────

impl Function {
    pub fn call<R: FromLuaMulti>(&self, args: impl IntoLuaMulti) -> Result<R> {
        let lua = &self.0.lua();
        let a = args.into_lua_multi(lua)?;
        let r = lua.call_raw(self.0.get(), a)?;
        R::from_lua_multi(r, lua)
    }

    pub fn to_pointer(&self) -> *const c_void {
        pointer_of(&self.0)
    }

    /// The function's environment, for a Lua function.
    pub fn environment(&self) -> Option<Table> {
        let lua = &self.0.lua();
        let f = self.0.get().as_func()?;
        let env = match lua.vm().heap.func(f) {
            benilla_lua::heap::FuncObj::Lua(l) => l.env(),
            benilla_lua::heap::FuncObj::Native { env, .. } => *env,
        };
        Some(Table(lua.make_ref(benilla_lua::Value::Table(env))))
    }

    /// Where the function was defined, as `lua_getinfo(L, "S")` says.
    pub fn info(&self) -> FunctionInfo {
        let lua = &self.0.lua();
        let mut out = FunctionInfo {
            what: "C",
            source: Some("=[C]".into()),
            short_src: Some("[C]".into()),
            ..Default::default()
        };
        let Some(f) = self.0.get().as_func() else {
            return out;
        };
        if let benilla_lua::heap::FuncObj::Lua(l) = lua.vm().heap.func(f) {
            let proto = l.proto();
            let short = benilla_lua::chunkid_n(&proto.source, 60);
            out.what = if proto.line_defined == 0 {
                "main"
            } else {
                "Lua"
            };
            out.source = Some(std::string::String::from_utf8_lossy(&proto.source).into_owned());
            out.short_src = Some(std::string::String::from_utf8_lossy(&short).into_owned());
            out.line_defined = Some(proto.line_defined as usize);
        }
        out
    }

    pub fn set_environment(&self, env: Table) -> Result<bool> {
        let lua = &self.0.lua();
        let Some(f) = self.0.get().as_func() else {
            return Ok(false);
        };
        let Some(t) = lua.unref(&env.0).as_table() else {
            return Ok(false);
        };
        // `setfenv`: a running frame rereads its cached environment.
        Ok(lua.vm().set_function_env(f, t))
    }
}

/// mlua's `FunctionInfo`.
#[derive(Clone, Debug, Default)]
pub struct FunctionInfo {
    pub name: Option<std::string::String>,
    pub name_what: Option<&'static str>,
    pub what: &'static str,
    pub source: Option<std::string::String>,
    pub short_src: Option<std::string::String>,
    pub line_defined: Option<usize>,
    pub last_line_defined: Option<usize>,
}

// ── strings ──────────────────────────────────────────────────────────────────────────────────

impl String {
    /// `f` over the string's bytes, read in place: `f` must not reach the VM.
    fn with_bytes<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        // SAFETY: as `RefH::get`; `f` holds nothing that reaches the VM.
        let vm = unsafe { &*self.0.vm() };
        let s = self.0.get().as_str().expect("a string handle");
        f(vm.heap.str(s))
    }

    /// The bytes, copied: a short string's bytes live inside the heap's arena, which moves as
    /// it grows, so a borrow of them could not outlive the next allocation.
    fn copy(&self) -> Bytes {
        self.with_bytes(Bytes::of)
    }

    pub fn to_str(&self) -> Result<BorrowedStr<'_>> {
        let b = self.copy();
        match std::str::from_utf8(b.as_slice()) {
            Ok(_) => Ok(BorrowedStr {
                b,
                _p: std::marker::PhantomData,
            }),
            Err(e) => Err(Error::FromLuaConversionError {
                from: "string",
                to: "&str".to_string(),
                message: Some(e.to_string()),
            }),
        }
    }

    pub fn to_string_lossy(&self) -> std::string::String {
        self.with_bytes(|b| std::string::String::from_utf8_lossy(b).into_owned())
    }

    pub fn as_bytes(&self) -> BorrowedBytes<'_> {
        BorrowedBytes {
            b: self.copy(),
            _p: std::marker::PhantomData,
        }
    }

    pub fn to_pointer(&self) -> *const c_void {
        pointer_of(&self.0)
    }

    pub fn display(&self) -> impl std::fmt::Display + '_ {
        self.to_string_lossy()
    }
}

impl PartialEq for String {
    fn eq(&self, other: &Self) -> bool {
        let theirs = other.copy();
        self.with_bytes(|b| b == theirs.as_slice())
    }
}

impl<T: AsRef<[u8]> + ?Sized> PartialEq<T> for String {
    fn eq(&self, other: &T) -> bool {
        self.with_bytes(|b| b == other.as_ref())
    }
}

/// A copy of a string's bytes: in place up to the length the heap keeps inline, so reading a
/// short string allocates nothing; boxed beyond.
enum Bytes {
    Inline(u8, [u8; Bytes::INLINE]),
    Boxed(Box<[u8]>),
}

impl Bytes {
    const INLINE: usize = 22;

    fn of(b: &[u8]) -> Bytes {
        if b.len() <= Bytes::INLINE {
            let mut buf = [0; Bytes::INLINE];
            buf[..b.len()].copy_from_slice(b);
            Bytes::Inline(b.len() as u8, buf)
        } else {
            Bytes::Boxed(b.into())
        }
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            Bytes::Inline(n, buf) => &buf[..*n as usize],
            Bytes::Boxed(b) => b,
        }
    }
}

/// A string's text, read from its handle. Not `Clone`, as mlua's is not, so `to_owned()`
/// reaches `str`'s and answers a `String`. It owns a copy (see `String::copy`).
pub struct BorrowedStr<'a> {
    /// Valid UTF-8, checked when made.
    b: Bytes,
    _p: std::marker::PhantomData<&'a String>,
}

impl BorrowedStr<'_> {
    fn s(&self) -> &str {
        // SAFETY: `to_str` checked the bytes are UTF-8, and they never change.
        unsafe { std::str::from_utf8_unchecked(self.b.as_slice()) }
    }
}

impl Deref for BorrowedStr<'_> {
    type Target = str;
    fn deref(&self) -> &str {
        self.s()
    }
}

impl AsRef<str> for BorrowedStr<'_> {
    fn as_ref(&self) -> &str {
        self.s()
    }
}

impl std::borrow::Borrow<str> for BorrowedStr<'_> {
    fn borrow(&self) -> &str {
        self.s()
    }
}

impl std::fmt::Display for BorrowedStr<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.s().fmt(f)
    }
}

impl std::fmt::Debug for BorrowedStr<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.s().fmt(f)
    }
}

impl<T: AsRef<str> + ?Sized> PartialEq<T> for BorrowedStr<'_> {
    fn eq(&self, other: &T) -> bool {
        self.s() == other.as_ref()
    }
}

impl From<BorrowedStr<'_>> for std::string::String {
    fn from(b: BorrowedStr<'_>) -> Self {
        b.s().to_owned()
    }
}

impl<'a> From<BorrowedStr<'a>> for Cow<'a, str> {
    fn from(b: BorrowedStr<'a>) -> Self {
        Cow::Owned(b.s().to_owned())
    }
}

/// A string's bytes, read from its handle; it owns a copy (see `String::copy`).
pub struct BorrowedBytes<'a> {
    b: Bytes,
    _p: std::marker::PhantomData<&'a String>,
}

impl Deref for BorrowedBytes<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.b.as_slice()
    }
}

impl AsRef<[u8]> for BorrowedBytes<'_> {
    fn as_ref(&self) -> &[u8] {
        self.b.as_slice()
    }
}

impl std::borrow::Borrow<[u8]> for BorrowedBytes<'_> {
    fn borrow(&self) -> &[u8] {
        self.b.as_slice()
    }
}

impl std::fmt::Debug for BorrowedBytes<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.b.as_slice().fmt(f)
    }
}

impl<T: AsRef<[u8]> + ?Sized> PartialEq<T> for BorrowedBytes<'_> {
    fn eq(&self, other: &T) -> bool {
        self.b.as_slice() == other.as_ref()
    }
}

impl<'a, 'b> IntoIterator for &'b BorrowedBytes<'a> {
    type Item = &'b u8;
    type IntoIter = std::slice::Iter<'b, u8>;
    fn into_iter(self) -> Self::IntoIter {
        self.b.as_slice().iter()
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Multiple values
// ─────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct MultiValue(VecDeque<Value>);

impl Deref for MultiValue {
    type Target = VecDeque<Value>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for MultiValue {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl MultiValue {
    pub const fn new() -> MultiValue {
        MultiValue(VecDeque::new())
    }

    pub fn with_capacity(capacity: usize) -> MultiValue {
        MultiValue(VecDeque::with_capacity(capacity))
    }

    pub fn from_vec(vec: Vec<Value>) -> MultiValue {
        MultiValue(vec.into())
    }

    pub fn into_vec(self) -> Vec<Value> {
        self.0.into()
    }
}

impl From<Vec<Value>> for MultiValue {
    fn from(v: Vec<Value>) -> Self {
        MultiValue::from_vec(v)
    }
}

impl From<MultiValue> for Vec<Value> {
    fn from(m: MultiValue) -> Self {
        m.into_vec()
    }
}

impl FromIterator<Value> for MultiValue {
    fn from_iter<I: IntoIterator<Item = Value>>(iter: I) -> Self {
        MultiValue(iter.into_iter().collect())
    }
}

impl IntoIterator for MultiValue {
    type Item = Value;
    type IntoIter = std::collections::vec_deque::IntoIter<Value>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a MultiValue {
    type Item = &'a Value;
    type IntoIter = std::collections::vec_deque::Iter<'a, Value>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Any number of trailing values of one type.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Variadic<T>(Vec<T>);

impl<T> Variadic<T> {
    pub const fn new() -> Variadic<T> {
        Variadic(Vec::new())
    }

    pub fn with_capacity(capacity: usize) -> Variadic<T> {
        Variadic(Vec::with_capacity(capacity))
    }
}

impl<T> Deref for Variadic<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<T> DerefMut for Variadic<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.0
    }
}

impl<T> From<Vec<T>> for Variadic<T> {
    fn from(v: Vec<T>) -> Self {
        Variadic(v)
    }
}

impl<T> From<Variadic<T>> for Vec<T> {
    fn from(v: Variadic<T>) -> Self {
        v.0
    }
}

impl<T> FromIterator<T> for Variadic<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Variadic(iter.into_iter().collect())
    }
}

impl<T> IntoIterator for Variadic<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Chunks
// ─────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkMode {
    Text,
    Binary,
}

/// What `Lua::load` takes.
pub trait AsChunk {
    fn source(&self) -> Vec<u8>;
    fn name(&self) -> Option<std::string::String> {
        None
    }
}

impl AsChunk for &str {
    fn source(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

impl AsChunk for std::string::String {
    fn source(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

impl AsChunk for &std::string::String {
    fn source(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

impl AsChunk for &[u8] {
    fn source(&self) -> Vec<u8> {
        self.to_vec()
    }
}

impl<const N: usize> AsChunk for &[u8; N] {
    fn source(&self) -> Vec<u8> {
        self.to_vec()
    }
}

impl AsChunk for Vec<u8> {
    fn source(&self) -> Vec<u8> {
        self.clone()
    }
}

impl AsChunk for &Vec<u8> {
    fn source(&self) -> Vec<u8> {
        (*self).clone()
    }
}

impl AsChunk for Cow<'_, str> {
    fn source(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

impl AsChunk for Cow<'_, [u8]> {
    fn source(&self) -> Vec<u8> {
        self.to_vec()
    }
}

pub struct Chunk<'a> {
    lua: &'a Lua,
    source: Vec<u8>,
    /// The chunk name as `lua_load` takes it: bytes, which need not be UTF-8.
    name: Vec<u8>,
    env: Option<Table>,
}

impl<'a> Chunk<'a> {
    pub fn set_name(mut self, name: impl AsRef<[u8]>) -> Self {
        self.name = name.as_ref().to_vec();
        self
    }

    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// mlua's mode, which changes nothing here: the client compiles every chunk as source
    /// (`luaD_protectedparser` zeroes the binary flag, `0x6f6a15`), so a binary one meets the
    /// lexer like text.
    pub fn set_mode(self, _mode: ChunkMode) -> Self {
        self
    }

    pub fn set_environment(mut self, env: Table) -> Self {
        self.env = Some(env);
        self
    }

    fn compile(&self, src: &[u8]) -> Result<Function> {
        // `lua_load` takes the name as a C string: it ends at its first NUL.
        let name = self.name.split(|&b| b == 0).next().unwrap_or_default();
        let lua = self.lua;
        let f = lua.vm().load(src, name).map_err(|e| Error::SyntaxError {
            message: e.to_string(),
            incomplete_input: e.0.ends_with(b"near `<eof>'"),
            raw: e.0,
        })?;
        let f = Function(lua.make_ref(f));
        if let Some(env) = &self.env {
            f.set_environment(env.clone())?;
        }
        Ok(f)
    }

    pub fn into_function(self) -> Result<Function> {
        self.compile(&self.source)
    }

    pub fn exec(self) -> Result<()> {
        self.call::<()>(())
    }

    pub fn call<R: FromLuaMulti>(self, args: impl IntoLuaMulti) -> Result<R> {
        self.into_function()?.call(args)
    }

    /// As mlua: first as an expression (`return <chunk>`), then as statements.
    pub fn eval<R: FromLuaMulti>(self) -> Result<R> {
        let mut expr = b"return ".to_vec();
        expr.extend_from_slice(&self.source);
        match self.compile(&expr) {
            Ok(f) => f.call(()),
            Err(_) => self.call(()),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Host data
// ─────────────────────────────────────────────────────────────────────────────────────────────

pub struct AppDataRef<'a, T: 'static> {
    r: Ref<'a, T>,
    _cell: Rc<RefCell<T>>,
}

impl<T> Deref for AppDataRef<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.r
    }
}

pub struct AppDataRefMut<'a, T: 'static> {
    r: RefMut<'a, T>,
    _cell: Rc<RefCell<T>>,
}

impl<T> Deref for AppDataRefMut<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.r
    }
}

impl<T> DerefMut for AppDataRefMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.r
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Debugging
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// One stack level.
pub struct Debug<'a> {
    info: benilla_lua::vm::FrameInfo,
    lua: &'a Lua,
}

pub struct DebugSource<'a> {
    pub source: Option<Cow<'a, str>>,
    pub short_src: Option<Cow<'a, str>>,
    pub line_defined: Option<usize>,
    pub last_line_defined: Option<usize>,
    pub what: &'static str,
}

pub struct DebugNames<'a> {
    pub name: Option<Cow<'a, str>>,
    pub name_what: Option<&'static str>,
}

impl Debug<'_> {
    /// The function running at this level; a tail call's level names none, as 5.1's does.
    pub fn function(&self) -> Function {
        Function(self.lua.make_ref(self.info.func))
    }

    pub fn source(&self) -> DebugSource<'_> {
        let i = &self.info;
        let (source, short_src) = match (i.what, &i.source) {
            ("tail", _) => (
                Some(Cow::Borrowed("=(tail call)")),
                Some(Cow::Borrowed("(tail call)")),
            ),
            ("C", _) | (_, None) => (Some(Cow::Borrowed("=[C]")), Some(Cow::Borrowed("[C]"))),
            (_, Some(s)) => {
                let short = benilla_lua::chunkid_n(s, 60);
                (
                    Some(Cow::Owned(
                        std::string::String::from_utf8_lossy(s).into_owned(),
                    )),
                    Some(Cow::Owned(
                        std::string::String::from_utf8_lossy(&short).into_owned(),
                    )),
                )
            }
        };
        DebugSource {
            source,
            short_src,
            line_defined: (i.what != "C" && i.what != "tail").then_some(i.line_defined as usize),
            last_line_defined: None,
            what: i.what,
        }
    }

    pub fn names(&self) -> DebugNames<'_> {
        DebugNames {
            name: self
                .info
                .name
                .as_ref()
                .map(|n| Cow::Owned(std::string::String::from_utf8_lossy(n).into_owned())),
            name_what: (!self.info.namewhat.is_empty()).then_some(self.info.namewhat),
        }
    }

    pub fn current_line(&self) -> Option<usize> {
        self.info.current_line.map(|l| l as usize)
    }

    pub fn curr_line(&self) -> i32 {
        self.info.current_line.map_or(-1, |l| l as i32)
    }

    pub fn is_tail_call(&self) -> bool {
        self.info.what == "tail"
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HookTriggers {
    pub on_calls: bool,
    pub on_returns: bool,
    pub every_line: bool,
    pub every_nth_instruction: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmState {
    Continue,
    Yield,
}
