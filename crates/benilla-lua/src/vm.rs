//! The virtual machine: one stack, one call-frame list, and the script host's 5.1 semantics for
//! calls, metamethods, coercions and error messages.

use std::rc::Rc;

use crate::heap::{FuncObj, Heap, LuaFunc, NativeFn, UpvalObj, UserObj};
use crate::number::str2d;
use crate::pools::Block;
use crate::proto::{Arith, Cmp, Op, Proto, Reg, Rk, RK_CONST};
use crate::value::{FuncRef, StrRef, TableRef, Value, V};

/// An error in flight: the error object, which `pcall` answers.
#[derive(Clone, Copy, Debug)]
pub struct LuaError {
    pub value: Value,
}

/// 5.1.5's `LUAI_MAXCCALLS`: nested host-level calls (a host call, a metamethod, a library
/// callback); past it by an eighth, an error handler's own calls are refused too.
const MAXCCALLS: u32 = 200;
/// The client's `LUA_MAXCALLS`, 5.0's 4096: `luaD_growCI` (`0x6f61c0`) refuses a call-info
/// array past it (`0x6f61c7`, `0x6f61e4`).
const MAXCALLS: usize = 4096;
/// 5.0's `BASIC_CI_SIZE`: the call-info array's first size (`stack_init`'s 0xc0 bytes,
/// `0x6f6be4`).
const BASIC_CI_SIZE: usize = 8;
/// 5.0's `EXTRA_STACK`: the slots past `stack_last` (`luaD_reallocstack`, `0x6f5e4d`), so a check
/// leaves this many and one more above what it asks for.
const EXTRA_STACK: usize = 5;
/// 5.0's first stack, `BASIC_STACK_SIZE + EXTRA_STACK` slots (`stack_init`'s 0x2d0 bytes,
/// `0x6f6bb8`).
const BASIC_STACK_SIZE: usize = 40 + EXTRA_STACK;
/// 5.0's `LUA_MINSTACK`: the room `luaD_precall` asks for a native (`0x6f6142`).
const LUA_MINSTACK: usize = 20;
/// Rust stack the nested host-level calls under one entry may use. C counts only levels, its
/// frames being small; a Rust level is larger, and much larger unoptimized, so the level count
/// alone could run a 2 MB thread out of stack. Past this the call raises "C stack overflow" as
/// at `LUAI_MAXCCALLS`, with the same eighth of margin for a handler. 200 levels of the deepest
/// library callback use under half of it in an optimized build, so there it never decides.
const MAX_RUST_STACK: usize = 1 << 20;
/// A stack slot in the collector's count: 5.0's 16-byte `TObject` (`stack_init`'s 0x2d0 bytes for
/// 45 slots, `0x6f6bb8`), counted as the stack grows.
const STACK_SLOT_BYTES: usize = 16;
/// A call record in the collector's count: 5.0's 24-byte `CallInfo` (`luaD_reallocCI`,
/// `0x6f5f1a`).
const CI_BYTES: usize = 24;
/// The state's own records in the collector's count, the block count a new state starts from
/// (`0x6f6e0b`): the 0x68-byte `lua_State` (`0x6f6b8d`) and the 0xc0-byte `global_State`
/// (`0x6f6d88`).
const STATE_BYTES: usize = 0x128;
/// 5.1.5's `MAXTAGLOOP`: the steps `luaV_gettable` and `luaV_settable` take along a chain of
/// `__index` or `__newindex` tables before `loop in gettable`.
const MAXTAGLOOP: u32 = 100;

#[derive(Clone, Copy)]
struct Frame {
    func: FuncRef,
    /// The first register; the called function sits just below, where its results go.
    base: u32,
    /// The next instruction, saved as C Lua saves `savedpc`: a pointer, so resuming the frame
    /// reads one value to dispatch. Null for a native.
    ip: *const Op,
    /// Results the caller wants; -1 for all.
    nresults: i32,
    /// The code this frame runs, null for a native. Valid while the frame exists: its closure is
    /// a collector root, and a hot reload retires the code it replaces instead of dropping it
    /// (`Vm::retired`).
    proto: *const Proto,
    /// The closure's upvalue list and environment as the call found them, so entering the
    /// frame reads no closure. The list is valid as `proto` is; the environment is reread
    /// when `Vm::fenv_gen` moves.
    upvals: *const u32,
    env: TableRef,
    /// `Vm::fenv_gen` when `env` was read: a `setfenv` since moves it, and the next global
    /// access rereads the closure's environment.
    env_gen: u32,
    /// The constant table and register count, so resuming the frame reads no prototype.
    consts: *const Value,
    maxstack: u16,
    /// The chunk this frame's code came from (`Heap::sources`).
    source: u32,
    /// Tail calls this frame replaced, which 5.0 counts as stack levels.
    tailcalls: u32,
    /// Returning from this frame ends the `execute` that entered it.
    boundary: bool,
}

// A call writes one cache line of frame.
const _: () = assert!(std::mem::size_of::<Frame>() == 64);

impl Frame {
    /// The index of the next instruction, for error positions.
    fn pc(&self) -> usize {
        if self.proto.is_null() {
            return 0;
        }
        // SAFETY: a live Lua frame's `ip` points into its prototype's code.
        unsafe { self.ip.offset_from((*self.proto).code.as_ptr()) as usize }
    }

    #[inline(always)]
    fn base(&self) -> usize {
        self.base as usize
    }

    /// The stack slot of the called function, where its results go.
    #[inline(always)]
    fn func_idx(&self) -> usize {
        self.base as usize - 1
    }
}

/// Metamethod and library names, interned once.
#[derive(Clone)]
pub struct Names {
    pub index: StrRef,
    pub newindex: StrRef,
    pub call: StrRef,
    pub add: StrRef,
    pub sub: StrRef,
    pub mul: StrRef,
    pub div: StrRef,
    pub pow: StrRef,
    pub unm: StrRef,
    pub concat: StrRef,
    pub eq: StrRef,
    pub lt: StrRef,
    pub le: StrRef,
    pub mode: StrRef,
    pub metatable: StrRef,
    pub tostring: StrRef,
    pub n: StrRef,
    pub next: StrRef,
    pub ipairs: StrRef,
}

#[derive(Clone)]
pub struct Vm {
    pub heap: Heap,
    /// The value stack. Crate-internal: frames address it through raw pointers, so only the VM
    /// changes its length; the host reads and writes slots through `stack` and `stack_mut`.
    pub(crate) stack: Vec<Value>,
    pub(crate) top: usize,
    frames: Vec<Frame>,
    /// Open upvalues, by stack slot, ascending.
    open: Vec<(usize, u32)>,
    /// `concat`'s scratch buffer, kept between calls.
    concat_buf: Vec<u8>,
    /// Byte buffers the string library borrows for a call (`take_bufs`).
    bufs: Vec<Vec<u8>>,
    /// The base library's `next`: a generic `for` over it steps the table in place.
    pub(crate) builtin_next: Value,
    pub globals: TableRef,
    pub registry: TableRef,
    pub names: Names,
    c_depth: u32,
    /// `L->size_ci`: frames fit while `frames.len() + 1 < ci_size`. The array doubles as calls
    /// nest and a doubling past [`MAXCALLS`] raises "stack overflow", so every overflow comes at
    /// 4096 frames; the doubled array leaves the error's handler room to run.
    ci_size: usize,
    /// `ci_size - 1`: a call is made while `frames.len()` is below it, one compare on the call
    /// path.
    ci_limit: usize,
    /// The stack below it may hold the frame of a running function whose code can read a
    /// register it has not written (`Proto::clears`). A Lua frame entered below it starts with
    /// every register past its arguments nil, as `luaD_precall` starts every frame, so such code
    /// reads what C's would; above it no code can tell, and the clearing is skipped. Raised as
    /// such a function is entered, reset at the host's outermost call.
    clear_below: usize,
    /// Error handlers running (`throw`): only they may use the margin past `MAX_RUST_STACK`.
    in_handler: u32,
    /// The Rust stack address where the outermost host-level call entered (`MAX_RUST_STACK`).
    stack_base: usize,
    /// The handler of each active protected call, innermost last; `None` for `pcall`.
    handlers: Vec<Option<Value>>,
    /// Collect at every safe point: a test switch that finds a value the roots miss.
    pub gc_stress: bool,
    /// Where the client's allocator keeps the stack and the call records, which
    /// `luaD_reallocstack` and `luaD_reallocCI` reallocate (`0x6f5e45`, `0x6f5f1a`).
    stack_block: Block,
    ci_block: Block,
    /// Code and upvalue lists a hot reload replaced, kept while a frame may still run them.
    pub(crate) retired: Vec<(Rc<Proto>, crate::heap::Upvals)>,
    /// Bumped by every `setfenv` of a Lua function, so a running frame's cached environment
    /// knows to reread.
    pub(crate) fenv_gen: u32,
    /// Values a host holds outside the VM (a handle to a table or function), each a collector
    /// root while its count is above zero: mlua's registry references.
    roots: Roots,
    /// The instruction-budget hook, and whether one is set (read where loops and calls pass).
    hook: Option<Hook>,
    /// A hook or a profile is on: the flag the dispatch loop tests.
    hooked: bool,
    /// The hook is running: 5.1's `luaD_hook` clears `allowhook` around the hook, so code the
    /// hook runs does not count toward, or run, the hook again.
    in_hook: bool,
    /// The message of an abort unwinding to the host ([`Vm::abort`]).
    abort: Option<Rc<[u8]>>,
    /// Time charged to each chunk while profiling (`set_profiling`).
    pub profile: Option<Box<Profile>>,
    /// The embedding host's handle to its own state, which a host closure reads at each call
    /// instead of capturing it, so a cloned VM's closures reach the clone's host. Weak, so the
    /// host owning this VM makes no cycle.
    pub host: Option<std::rc::Weak<dyn std::any::Any>>,
    /// How the host runs its own code against this VM from inside it (`Vm::as_host`): it makes
    /// the `&mut Vm` it is handed the one its API reaches, as for a callback.
    pub reentry: Option<Reentry>,
    /// `math.random`'s generator.
    pub(crate) rand: crate::lib_math::GlibcRand,
    identity: crate::heap::Identity,
}

/// The host's way in: run the closure with the `&mut Vm` as the VM in effect.
pub type Reentry = Rc<dyn Fn(&mut Vm, &mut dyn FnMut())>;

/// Values a host holds outside the VM, each with a count of holders; mlua's registry
/// references. Counted through `&self`, so a holder can let go while the VM is borrowed.
#[derive(Debug, Default)]
pub struct RootTable {
    /// Each root's value, holder count and serial (`ROOT_SERIAL`).
    slots: std::cell::RefCell<Vec<(Value, u32, u64)>>,
    free: std::cell::RefCell<Vec<u32>>,
}

/// Serials for roots, unique across every VM: a fork's copy of a root keeps its serial, and a
/// root made after the fork, in either, has one the other never had.
static ROOT_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl RootTable {
    fn root(&self, v: Value) -> u32 {
        let serial = ROOT_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut slots = self.slots.borrow_mut();
        match self.free.borrow_mut().pop() {
            Some(i) => {
                slots[i as usize] = (v, 1, serial);
                i
            }
            None => {
                slots.push((v, 1, serial));
                (slots.len() - 1) as u32
            }
        }
    }

    /// Root `i`'s serial; 0 for an id that holds nothing.
    pub fn serial(&self, i: u32) -> u64 {
        self.slots
            .borrow()
            .get(i as usize)
            .filter(|r| r.1 > 0)
            .map_or(0, |r| r.2)
    }

    /// Root `i`'s value if it is the root with `serial`: a handle made in this VM, or in the
    /// VM it was forked from before the fork.
    pub fn get_as(&self, i: u32, serial: u64) -> Option<Value> {
        self.slots
            .borrow()
            .get(i as usize)
            .filter(|r| r.1 > 0 && r.2 == serial)
            .map(|r| r.0)
    }

    /// Another holder of root `i`; an id this table never gave is ignored.
    pub fn retain(&self, i: u32) {
        if let Some(r) = self
            .slots
            .borrow_mut()
            .get_mut(i as usize)
            .filter(|r| r.1 > 0)
        {
            r.1 += 1;
        }
    }

    /// One holder of root `i` lets go; an id that holds nothing is ignored.
    pub fn unroot(&self, i: u32) {
        let mut slots = self.slots.borrow_mut();
        if let Some(r) = slots.get_mut(i as usize).filter(|r| r.1 > 0) {
            r.1 -= 1;
            if r.1 == 0 {
                r.0 = Value::Nil;
                self.free.borrow_mut().push(i);
            }
        }
    }

    /// Root `i`'s value; nil for an id that holds nothing.
    pub fn get(&self, i: u32) -> Value {
        self.slots
            .borrow()
            .get(i as usize)
            .map_or(Value::Nil, |r| r.0)
    }

    fn live(&self) -> Vec<Value> {
        let slots = self.slots.borrow();
        slots.iter().filter(|r| r.1 > 0).map(|r| r.0).collect()
    }
}

/// A VM's root table. A cloned VM gets its own copy, as it gets its own heap.
#[derive(Default)]
struct Roots(Rc<RootTable>);

impl Clone for Roots {
    fn clone(&self) -> Self {
        Roots(Rc::new(RootTable {
            slots: self.0.slots.clone(),
            free: self.0.free.clone(),
        }))
    }
}

/// The host's hook body; an error it answers is raised where the hook ran.
pub type HookFn = Rc<dyn Fn(&mut Vm) -> Result<(), LuaError>>;

/// A host hook run once for every `step` instructions Lua runs, counted as 5.1's
/// `LUA_MASKCOUNT` counts them: each instruction 5.1 dispatches, in every Lua frame (Lua a
/// library function or a metamethod calls included). It does not run at the instruction that
/// completes a step, as 5.1's does, but at the next loop back edge, call or return, once for each
/// step completed since. A non-terminating script passes those, so a budget still stops it, and
/// the dispatch loop pays nothing for the count until a hook is set.
#[derive(Clone)]
pub struct Hook {
    pub step: u64,
    pub f: HookFn,
    counted: u64,
}

/// Wall time charged to each chunk (`Heap::sources`, by id). A switch between chunks (a call, a
/// return) and every loop back-edge closes the running segment and charges it to the chunk that
/// ran it; time in a host function goes to the chunk that called it; time outside Lua to none.
#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub ns: Vec<u64>,
    last: Option<(std::time::Instant, u32)>,
}

impl Profile {
    /// `source` runs from now: the running segment is charged to its chunk. A call or return
    /// within one chunk reads no clock, so the profile costs what crossing chunks costs.
    fn mark(&mut self, source: u32) {
        if self.last.is_some_and(|(_, s)| s == source) {
            return;
        }
        self.charge(std::time::Instant::now(), Some(source));
    }

    fn charge(&mut self, now: std::time::Instant, next: Option<u32>) {
        if let Some((t, s)) = self.last {
            let s = s as usize;
            if self.ns.len() <= s {
                self.ns.resize(s + 1, 0);
            }
            self.ns[s] += now.duration_since(t).as_nanos() as u64;
        }
        self.last = next.map(|n| (now, n));
    }

    /// Close the running segment: execution left Lua.
    fn close(&mut self) {
        self.charge(std::time::Instant::now(), None);
    }
}

/// One stack level, as `lua_getinfo` describes it with `"Snl"`.
#[derive(Clone, Debug, Default)]
pub struct FrameInfo {
    /// The chunk name, for a Lua function.
    pub source: Option<Vec<u8>>,
    /// `"Lua"`, `"main"`, `"C"` or `"tail"`.
    pub what: &'static str,
    pub current_line: Option<u32>,
    pub line_defined: u32,
    /// How the calling instruction named it: `("global", "foo")`.
    pub name: Option<Vec<u8>>,
    pub namewhat: &'static str,
    /// The function running at the level (`lua_getinfo`'s `"f"`); nil for a tail call.
    pub func: Value,
}

/// What a call produced, for the dispatch loop.
enum Called {
    /// A Lua frame was pushed; run it.
    Lua,
    /// A native ran; its results are in place.
    Done,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    pub fn new() -> Vm {
        let mut heap = Heap::default();
        let mut s = |b: &[u8]| heap.intern(b);
        let names = Names {
            index: s(b"__index"),
            newindex: s(b"__newindex"),
            call: s(b"__call"),
            add: s(b"__add"),
            sub: s(b"__sub"),
            mul: s(b"__mul"),
            div: s(b"__div"),
            pow: s(b"__pow"),
            unm: s(b"__unm"),
            concat: s(b"__concat"),
            eq: s(b"__eq"),
            lt: s(b"__lt"),
            le: s(b"__le"),
            mode: s(b"__mode"),
            metatable: s(b"__metatable"),
            tostring: s(b"__tostring"),
            n: s(b"n"),
            next: s(b"next"),
            ipairs: s(b"ipairs"),
        };
        // `f_luaopen`: the globals by `luaH_new(L, 0, 4)` (`0x6f6e62`), the registry by
        // `luaH_new(L, 4, 4)` (`0x6f6e87`).
        let globals = heap.new_table(0, 4);
        let registry = heap.new_table(4, 4);
        let mut vm = Vm {
            heap,
            stack: vec![Value::Nil; BASIC_STACK_SIZE],
            top: 0,
            frames: Vec::new(),
            open: Vec::new(),
            concat_buf: Vec::new(),
            bufs: Vec::new(),
            builtin_next: Value::Nil,
            globals,
            registry,
            names,
            c_depth: 0,
            ci_size: BASIC_CI_SIZE,
            ci_limit: BASIC_CI_SIZE - 1,
            clear_below: 0,
            stack_base: 0,
            in_handler: 0,
            handlers: Vec::new(),
            gc_stress: false,
            stack_block: Block::None,
            ci_block: Block::None,
            retired: Vec::new(),
            fenv_gen: 0,
            roots: Roots::default(),
            hook: None,
            hooked: false,
            in_hook: false,
            abort: None,
            profile: None,
            host: None,
            reentry: None,
            rand: crate::lib_math::GlibcRand::seeded(1),
            identity: Default::default(),
        };
        vm.heap.charge(vm.state_bytes());
        // `stack_init` (`0x6f6bb8`, `0x6f6be4`); the two records are the system's (`mallocstate`
        // and `f_luaopen` allocate them with no state, `0x6f6b8d`, `0x6f6d88`).
        let mut pools = vm.heap.pools();
        let (stack_block, ci_block) = (
            pools.alloc(STACK_SLOT_BYTES * BASIC_STACK_SIZE),
            pools.alloc(CI_BYTES * BASIC_CI_SIZE),
        );
        drop(pools);
        (vm.stack_block, vm.ci_block) = (stack_block, ci_block);
        // `f_luaopen` ends with four times the count (`0x6f6ec8`).
        let count = vm.heap.bytes();
        vm.heap.set_threshold(4 * count);
        vm
    }

    /// 5.0's `lua_setgcthreshold` (`0x6f4400`): the threshold `kb` kilobytes, past `0x3fffff`
    /// (compared unsigned) the most there is, then a collection if the count has reached it.
    pub fn set_gc_threshold(&mut self, kb: u32) {
        let t = if kb > 0x3f_ffff {
            u32::MAX as usize
        } else {
            (kb as usize) << 10
        };
        self.heap.set_threshold(t);
        self.check_gc();
    }

    // ── strings and values ───────────────────────────────────────────────────────────────────

    /// `N` empty byte buffers, reused from earlier calls where there are some.
    pub(crate) fn take_bufs<const N: usize>(&mut self) -> [Vec<u8>; N] {
        std::array::from_fn(|_| self.bufs.pop().unwrap_or_default())
    }

    /// Buffers back to the pool; a very large one is dropped, so one huge string does not stay.
    pub(crate) fn give_bufs<const N: usize>(&mut self, bufs: [Vec<u8>; N]) {
        for mut b in bufs {
            if self.bufs.len() < 16 && b.capacity() <= 64 * 1024 {
                b.clear();
                self.bufs.push(b);
            }
        }
    }

    /// The value stack, up to its allocated length.
    pub fn stack(&self) -> &[Value] {
        &self.stack
    }

    /// The value stack's slots, to write values into (not to resize).
    pub fn stack_mut(&mut self) -> &mut [Value] {
        &mut self.stack
    }

    /// The first free stack slot.
    pub fn top(&self) -> usize {
        self.top
    }

    pub fn str(&mut self, s: &[u8]) -> Value {
        self.heap.str_value(s)
    }

    pub fn bytes(&self, v: Value) -> Option<&[u8]> {
        match v.unpack() {
            V::Str(s) => Some(self.heap.str(s)),
            _ => None,
        }
    }

    /// `luaV_tonumber`: a number, or a string `str2d` reads.
    pub fn tonumber(&self, v: Value) -> Option<f64> {
        match v.unpack() {
            V::Num(n) => Some(n),
            V::Str(s) => str2d(self.heap.str(s)),
            _ => None,
        }
    }

    /// `luaV_tostring`'s conversion: a string, or a number as `%.14g`.
    pub fn tostr(&mut self, v: Value) -> Option<StrRef> {
        match v.unpack() {
            V::Str(s) => Some(s),
            V::Num(n) => Some(crate::number::with_g14(n, |b| self.heap.intern(b))),
            _ => None,
        }
    }

    pub fn get_global(&mut self, name: &str) -> Value {
        let k = self.str(name.as_bytes());
        self.heap.table(self.globals).get(k)
    }

    pub fn set_global(&mut self, name: &str, v: Value) {
        let k = self.str(name.as_bytes());
        let g = self.globals;
        self.heap.table_mut(g).set(k, v).unwrap();
    }

    pub fn new_native(&mut self, name: &'static str, f: NativeFn) -> Value {
        let env = self.globals;
        Value::Func(self.heap.new_func(FuncObj::Native {
            f,
            host: None,
            name,
            upvals: Box::new([]),
            env,
        }))
    }

    /// A native function over a host closure.
    pub fn new_host(&mut self, name: &'static str, f: crate::heap::HostFn) -> Value {
        fn unreachable_native(_: &mut Vm, _: usize) -> Result<usize, LuaError> {
            unreachable!("a host function calls its closure")
        }
        let env = self.globals;
        Value::Func(self.heap.new_func(FuncObj::Native {
            f: unreachable_native,
            host: Some(f),
            name,
            upvals: Box::new([]),
            env,
        }))
    }

    pub fn register(&mut self, name: &'static str, f: NativeFn) {
        let v = self.new_native(name, f);
        self.set_global(name, v);
    }

    pub fn new_user(&mut self, tag: u64, meta: Option<TableRef>) -> Value {
        Value::User(self.heap.new_user(UserObj { meta, tag }))
    }

    // ── loading ──────────────────────────────────────────────────────────────────────────────

    /// Compile a chunk into a function over the globals.
    pub fn load(&mut self, src: &[u8], chunkname: &[u8]) -> Result<Value, crate::SyntaxError> {
        let proto = crate::compile::compile(src, chunkname, &mut self.heap)?;
        self.heap.charge_proto(&proto);
        // A constant the compiler could not make: 5.1's `LUA_ERRMEM`.
        if self.heap.oom {
            self.heap.oom = false;
            return Err(crate::SyntaxError(b"not enough memory".to_vec()));
        }
        let env = self.globals;
        Ok(Value::Func(self.heap.new_func(FuncObj::Lua(LuaFunc::new(
            proto,
            crate::heap::Upvals::new(Vec::new()),
            env,
        )))))
    }

    /// Load and run a chunk protected, answering its results or its error value.
    pub fn exec(&mut self, src: &[u8], chunkname: &[u8]) -> Result<Vec<Value>, Value> {
        let f = match self.load(src, chunkname) {
            Ok(f) => f,
            Err(e) => return Err(self.str(&e.0)),
        };
        self.pcall_value(f, &[], -1)
    }

    /// Run `f`, which may raise (a metamethod, a host call), protected: on an error the stack,
    /// frames and open upvalues above where it started unwind, as a `pcall` unwinds them, and
    /// the error value is the answer.
    pub fn protected<T>(
        &mut self,
        f: impl FnOnce(&mut Vm) -> Result<T, LuaError>,
    ) -> Result<T, Value> {
        let (frames, c_depth, top, handlers) = (
            self.frames.len(),
            self.c_depth,
            self.top,
            self.handlers.len(),
        );
        // A protected call of its own, as mlua's `lua_pcall` with no handler: an error inside
        // runs no enclosing `xpcall`'s handler.
        self.handlers.push(None);
        // A panic that passes leaves the state as at this call (`pcall_at`).
        let r = self.unwind_guard(f);
        self.handlers.truncate(handlers);
        match r {
            Ok(v) => Ok(v),
            Err(e) => {
                self.close_upvals(top);
                self.frames.truncate(frames);
                self.c_depth = c_depth;
                self.top = top;
                self.handlers.truncate(handlers);
                self.restore_stack_limit();
                Err(e.value)
            }
        }
    }

    /// Call `f` protected: its results, or its error value with the state unwound.
    pub fn pcall_value(
        &mut self,
        f: Value,
        args: &[Value],
        nres: i32,
    ) -> Result<Vec<Value>, Value> {
        let func_idx = self.top;
        self.ensure(func_idx + args.len() + 1);
        self.stack[func_idx] = f;
        self.stack[func_idx + 1..func_idx + 1 + args.len()].copy_from_slice(args);
        self.top = func_idx + 1 + args.len();
        match self.pcall_at(func_idx, args.len(), nres, None) {
            Ok(()) => {
                let out = self.stack[func_idx..self.top].to_vec();
                self.top = func_idx;
                Ok(out)
            }
            Err(e) => Err(e.value),
        }
    }

    /// Call `f`, protected and with no arguments, while each global in `globals` holds the value
    /// paired with it, read and written through the globals table's metamethods as
    /// `lua_getglobal` and `lua_setglobal` do; each one's earlier value is put back after the
    /// call, after an error too. The earlier values wait on the stack, where a collection inside
    /// the call keeps them. It is the 1.12 script handler's convention (`this`, `event`,
    /// `arg1..`) in one step. Answers the call's error value, or the first one a read or write
    /// of the globals raised. `f` and the values must be rooted by the caller, as a host's handles
    /// root them; the names wait on the stack with the earlier values.
    pub fn call_with_globals(
        &mut self,
        f: Value,
        globals: &[(StrRef, Value)],
    ) -> Result<(), Value> {
        let base = self.top;
        let n = globals.len();
        let g = Value::Table(self.globals);
        for &(k, _) in globals {
            self.push(Value::Str(k));
        }
        let read = self.protected(|vm| {
            for &(k, _) in globals {
                let old = vm.index(g, Value::Str(k), None)?;
                vm.push(old);
            }
            Ok(())
        });
        if let Err(e) = read {
            self.top = base;
            return Err(e);
        }
        let set = self.protected(|vm| {
            for &(k, v) in globals {
                vm.setindex(g, Value::Str(k), v, None)?;
            }
            Ok(())
        });
        let called = set.and_then(|()| {
            let func_idx = self.top;
            self.push(f);
            match self.pcall_at(func_idx, 0, 0, None) {
                Ok(()) => Ok(()),
                Err(e) => Err(e.value),
            }
        });
        let mut restored = Ok(());
        for (i, &(k, _)) in globals.iter().enumerate() {
            let old = self.stack[base + n + i];
            if self.abort.is_some() {
                // An abort raises again in any Lua `__newindex`: put the value back raw.
                let _ = self.heap.table_mut(self.globals).set(Value::Str(k), old);
                continue;
            }
            let r = self.protected(|vm| vm.setindex(g, Value::Str(k), old, None));
            restored = restored.and(r);
        }
        self.top = base;
        called.and(restored)
    }

    /// A value as a Rust string, for hosts and tests: strings and numbers as `tostring` writes
    /// them, other values by type.
    pub fn display(&mut self, v: Value) -> String {
        match self.tostr(v) {
            Some(s) => String::from_utf8_lossy(self.heap.str(s)).into_owned(),
            None => match v.unpack() {
                V::Nil => "nil".into(),
                V::Bool(b) => b.to_string(),
                _ => v.type_name().into(),
            },
        }
    }

    // ── errors ───────────────────────────────────────────────────────────────────────────────

    /// Raise `v`: the innermost protected call's handler runs here, before the stack unwinds, as
    /// 5.0's `luaD_throw` runs `errfunc` at the raise.
    pub fn throw(&mut self, v: Value) -> LuaError {
        if self.abort.is_some() {
            return self.abort_error();
        }
        let Some(Some(h)) = self.handlers.last().copied() else {
            return LuaError { value: v };
        };
        // 5.1's `luaG_errormsg`: a handler that is not a function is LUA_ERRERR, uncalled.
        if !matches!(h.unpack(), V::Func(_)) {
            return self.errerr();
        }
        // The handler stays in place while it runs, as `errfunc` does: an error inside it runs
        // it again, for that error, until the host-call limit throws LUA_ERRERR past every
        // handler. Its answer, or that error, is what the protected call gets.
        self.in_handler += 1;
        let r = self.call_value(h, &[v], 1);
        self.in_handler -= 1;
        match r {
            Ok(vals) => LuaError {
                value: vals.first().copied().unwrap_or(Value::Nil),
            },
            Err(e) => e,
        }
    }

    /// `luaG_runerror`: the message with the current Lua function's position.
    #[cold]
    #[inline(never)]
    pub fn rt_error(&mut self, msg: &str) -> LuaError {
        self.rt_error_bytes(msg.as_bytes())
    }

    /// [`Vm::rt_error`] of bytes, as the client's C strings are: a name or text that is not
    /// UTF-8 goes through as written.
    #[cold]
    #[inline(never)]
    pub fn rt_error_bytes(&mut self, msg: &[u8]) -> LuaError {
        let mut out = self.where_(0);
        out.extend_from_slice(msg);
        let v = self.str(&out);
        self.throw(v)
    }

    /// `luaL_error`: the message with the position of the Lua function `level` frames up
    /// (`luaL_where(L, 1)` from a native names its caller).
    pub fn error_at(&mut self, level: usize, msg: &str) -> LuaError {
        self.error_at_bytes(level, msg.as_bytes())
    }

    /// [`Vm::error_at`] of bytes, which need not be UTF-8.
    pub fn error_at_bytes(&mut self, level: usize, msg: &[u8]) -> LuaError {
        let mut out = self.where_(level);
        out.extend_from_slice(msg);
        let v = self.str(&out);
        self.throw(v)
    }

    /// `luaL_where`: `chunk:line: ` of the frame `level` up, if it is a Lua function.
    pub fn where_(&self, level: usize) -> Vec<u8> {
        let Some(fi) = self.frame_at_level(level) else {
            return Vec::new();
        };
        let f = &self.frames[fi];
        // SAFETY: a live frame's code is alive (see `Frame::proto`).
        match unsafe { f.proto.as_ref() } {
            Some(p) => {
                let pc = f.pc().saturating_sub(1);
                let line = p.lines.get(pc).copied().unwrap_or(0);
                if line == 0 {
                    return Vec::new();
                }
                let mut out = crate::chunkid_n(&p.source, 60);
                out.extend_from_slice(format!(":{line}: ").as_bytes());
                out
            }
            None => Vec::new(),
        }
    }

    /// The function `level` frames up: `None` past the bottom, `Some(None)` for a level a tail
    /// call replaced.
    pub fn function_at_level(&self, level: usize) -> Option<Option<FuncRef>> {
        let mut lv = level as i64;
        let mut i = self.frames.len();
        while i > 0 {
            i -= 1;
            if lv == 0 {
                return Some(Some(self.frames[i].func));
            }
            lv -= 1;
            if !self.frames[i].proto.is_null() {
                lv -= self.frames[i].tailcalls as i64;
            }
            if lv < 0 {
                return Some(None);
            }
        }
        None
    }

    /// A number naming this VM among every one made, a clone included.
    pub fn vm_id(&self) -> u64 {
        self.identity.0
    }

    /// No Lua or native runs on this VM: no call is in progress.
    pub fn frames_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The lowest Rust stack address a native's own recursion may reach: what the nested calls
    /// under the outermost host-level call may use (`MAX_RUST_STACK`), below where it entered.
    /// The stack grows down on every platform benilla builds for.
    pub(crate) fn rust_stack_limit(&self) -> usize {
        self.stack_base.saturating_sub(MAX_RUST_STACK)
    }

    pub(crate) fn native_base(&self) -> usize {
        self.frames.last().unwrap().base()
    }

    pub(crate) fn running(&self) -> FuncRef {
        self.frames.last().unwrap().func
    }

    /// 5.0's `lua_getstack`: the frame `level` up, tail calls counting as levels.
    fn frame_at_level(&self, level: usize) -> Option<usize> {
        let mut level = level as i64;
        let mut i = self.frames.len();
        while i > 0 {
            i -= 1;
            if level == 0 {
                return Some(i);
            }
            level -= 1;
            if !self.frames[i].proto.is_null() {
                level -= self.frames[i].tailcalls as i64;
            }
            if level < 0 {
                return None;
            }
        }
        None
    }

    /// 5.0's `getobjname` for register `reg` of the current Lua frame: `("local", name)` and so
    /// on, from the locals and a symbolic run of the code up to the current instruction.
    fn objname(&self, fi: usize, reg: usize) -> Option<(&'static str, Vec<u8>)> {
        let f = &self.frames[fi];
        // SAFETY: a live frame's code is alive (see `Frame::proto`).
        let p = unsafe { f.proto.as_ref() }?;
        let pc = f.pc().saturating_sub(1);
        objname_in(p, &self.heap, pc, reg)
    }

    /// `luaG_typeerror`: `attempt to <op> <kind> `<name>' (a <type> value)`.
    #[cold]
    #[inline(never)]
    fn type_error(&mut self, v: Value, reg: Option<usize>, op: &str) -> LuaError {
        let t = v.type_name();
        // A register names a value only in a running Lua frame; the host indexes with none.
        let named = reg.and_then(|r| self.objname(self.frames.len().checked_sub(1)?, r));
        let msg = match named {
            Some((kind, name)) => [
                format!("attempt to {op} {kind} `").as_bytes(),
                &name,
                format!("' (a {t} value)").as_bytes(),
            ]
            .concat(),
            None => format!("attempt to {op} a {t} value").into_bytes(),
        };
        self.rt_error_bytes(&msg)
    }

    #[cold]
    #[inline(never)]
    fn order_error(&mut self, a: Value, b: Value) -> LuaError {
        let (t1, t2) = (a.type_name(), b.type_name());
        // 5.0 compares the third character of the two type names.
        let msg = if t1.as_bytes()[2] == t2.as_bytes()[2] {
            format!("attempt to compare two {t1} values")
        } else {
            format!("attempt to compare {t1} with {t2}")
        };
        self.rt_error(&msg)
    }

    // ── stack ────────────────────────────────────────────────────────────────────────────────

    #[inline(always)]
    pub fn ensure(&mut self, n: usize) {
        if self.stack.len() < n {
            self.grow(n);
        }
    }

    /// A need past the stack: grown by 5.0's rule ([`Vm::growstack`]) for what the need asks
    /// above `top`, until it fits. Deviation: these are benilla's own needs, past what the client's
    /// checks ([`Vm::checkstack`]) reserve: a frame's registers above `c_maxstack`
    /// (`Proto::maxstack`), and the host's and natives' pushes, which ask for no margin of
    /// `EXTRA_STACK` and one more. Where they pass what the client reserved, the stack grows
    /// where the client's would not.
    #[cold]
    #[inline(never)]
    fn grow(&mut self, n: usize) {
        while self.stack.len() < n {
            self.growstack(n.saturating_sub(self.top));
        }
    }

    /// 5.0's `luaD_checkstack`: room for `n` slots above `top`, which leaves `EXTRA_STACK` and
    /// one more past them (`stack_last - top <= n`, `0x6f60c1`, `0x6f6142`).
    #[inline(always)]
    fn checkstack(&mut self, top: usize, n: usize) {
        if top + n + EXTRA_STACK + 1 >= self.stack.len() {
            self.growstack(n);
        }
    }

    /// 5.0's `luaD_growstack` (`0x6f5f60`): twice the size for a need it holds, else the size and
    /// the need and `EXTRA_STACK`; `luaD_reallocstack` reallocates it (`0x6f5e45`).
    #[cold]
    #[inline(never)]
    fn growstack(&mut self, n: usize) {
        let size = self.stack.len();
        let new = if n <= size {
            2 * size
        } else {
            size + n + EXTRA_STACK
        };
        self.realloc_stack(new);
    }

    fn realloc_stack(&mut self, new: usize) {
        let old = self.stack.len();
        self.heap
            .recharge(STACK_SLOT_BYTES * old, STACK_SLOT_BYTES * new);
        let b = self
            .heap
            .pools()
            .realloc(self.stack_block, STACK_SLOT_BYTES * new);
        self.stack_block = b;
        self.stack.resize(new, Value::Nil);
        self.stack.shrink_to_fit();
    }

    #[inline]
    pub fn push(&mut self, v: Value) {
        self.ensure(self.top + 1);
        self.stack[self.top] = v;
        self.top += 1;
    }

    // ── upvalues ─────────────────────────────────────────────────────────────────────────────

    /// `luaF_findupval`: the open upvalue for stack slot `idx`, made if absent.
    #[inline(never)]
    fn find_upval(&mut self, idx: usize) -> u32 {
        match self.open.binary_search_by_key(&idx, |&(i, _)| i) {
            Ok(p) => self.open[p].1,
            Err(p) => {
                let u = self.heap.new_upval(UpvalObj::Open(idx as u32));
                self.open.insert(p, (idx, u));
                u
            }
        }
    }

    /// `luaF_close`: every open upvalue at or above `level` takes its value.
    #[inline(never)]
    fn close_upvals(&mut self, level: usize) {
        while let Some(&(idx, u)) = self.open.last() {
            if idx < level {
                break;
            }
            *self.heap.upval_mut(u) = UpvalObj::Closed(self.stack[idx]);
            self.open.pop();
        }
    }

    #[inline]
    fn upval_set(&mut self, u: u32, v: Value) {
        match self.heap.upval(u) {
            UpvalObj::Open(i) => self.stack[i as usize] = v,
            UpvalObj::Closed(_) => *self.heap.upval_mut(u) = UpvalObj::Closed(v),
        }
    }

    // ── metamethods ──────────────────────────────────────────────────────────────────────────

    /// `luaT_gettmbyobj`: only tables and userdata carry metatables in 1.12.
    fn tm(&self, v: Value, name: StrRef) -> Value {
        match self.heap.metatable(v) {
            Some(m) => self.heap.table(m).get(Value::Str(name)),
            None => Value::Nil,
        }
    }

    /// `callTMres` (`0x6f7dd0`): `f(a, b)`'s first result, called on the stack's top and read
    /// from where it lands, with nothing allocated.
    fn call_tm_res(&mut self, f: Value, a: Value, b: Value) -> Result<Value, LuaError> {
        let func_idx = self.top;
        self.ensure(func_idx + 4);
        self.stack[func_idx] = f;
        self.stack[func_idx + 1] = a;
        self.stack[func_idx + 2] = b;
        self.top = func_idx + 3;
        self.call_at(func_idx, 2, 1)?;
        let r = self.stack[func_idx];
        self.top = func_idx;
        Ok(r)
    }

    /// `call_binTM`: the first operand's metamethod, else the second's, called whatever it is
    /// (a table with `__call` runs; a number raises "attempt to call").
    #[cold]
    #[inline(never)]
    fn bin_tm(&mut self, a: Value, b: Value, name: StrRef) -> Result<Option<Value>, LuaError> {
        let mut tm = self.tm(a, name);
        if tm.is_nil() {
            tm = self.tm(b, name);
        }
        if tm.is_nil() {
            return Ok(None);
        }
        Ok(Some(self.call_tm_res(tm, a, b)?))
    }

    /// `luaV_gettable`'s common miss, without calls: `t[key]` was nil and `t` has a metatable;
    /// follow `__index` while it is a table. `None` when a function, a non-table or a long
    /// chain needs the full path.
    #[inline(never)]
    fn index_meta_fast(&self, mut tr: TableRef, key: &Value) -> Option<Value> {
        for _ in 0..4 {
            let Some(m) = self.heap.table(tr).meta else {
                return Some(Value::Nil);
            };
            match (self.heap.table(m).get_ref(&Value::Str(self.names.index))).unpack() {
                V::Nil => return Some(Value::Nil),
                V::Table(next) => {
                    let v = self.heap.table(next).get_ref(key);
                    if !v.is_nil() {
                        return Some(v);
                    }
                    tr = next;
                }
                _ => return None,
            }
        }
        None
    }

    /// `luaV_gettable`. `reg` names the indexed value for an error.
    #[cold]
    #[inline(never)]
    pub fn index(&mut self, t: Value, key: Value, reg: Option<usize>) -> Result<Value, LuaError> {
        let mut t = t;
        let mut reg = reg;
        for _ in 0..MAXTAGLOOP {
            let tm = match t.unpack() {
                V::Table(tr) => {
                    let tab = self.heap.table(tr);
                    let v = tab.get(key);
                    if !v.is_nil() {
                        return Ok(v);
                    }
                    let Some(m) = tab.meta else {
                        return Ok(Value::Nil);
                    };
                    let tm = self.heap.table(m).get(Value::Str(self.names.index));
                    if tm.is_nil() {
                        return Ok(Value::Nil);
                    }
                    tm
                }
                _ => {
                    let tm = self.tm(t, self.names.index);
                    if tm.is_nil() {
                        return Err(self.type_error(t, reg, "index"));
                    }
                    tm
                }
            };
            if let V::Func(_) = tm.unpack() {
                return self.call_tm_res(tm, t, key);
            }
            t = tm;
            reg = None;
        }
        Err(self.rt_error("loop in gettable"))
    }

    /// `luaV_settable`: 5.0 makes the key in a table before it consults `__newindex`.
    #[cold]
    #[inline(never)]
    pub fn setindex(
        &mut self,
        t: Value,
        key: Value,
        val: Value,
        reg: Option<usize>,
    ) -> Result<(), LuaError> {
        let mut t = t;
        let mut reg = reg;
        for _ in 0..MAXTAGLOOP {
            let tm = match t.unpack() {
                V::Table(tr) => {
                    let slot = match self.heap.table_mut(tr).slot_for(key) {
                        Ok(s) => s,
                        Err(e) => return Err(self.rt_error(e.message())),
                    };
                    let tab = self.heap.table(tr);
                    let tm = if tab.slot(slot).is_nil() {
                        match tab.meta {
                            Some(m) => self.heap.table(m).get(Value::Str(self.names.newindex)),
                            None => Value::Nil,
                        }
                    } else {
                        Value::Nil
                    };
                    if tm.is_nil() {
                        *self.heap.table_mut(tr).slot_mut(slot) = val;
                        return Ok(());
                    }
                    tm
                }
                _ => {
                    let tm = self.tm(t, self.names.newindex);
                    if tm.is_nil() {
                        return Err(self.type_error(t, reg, "index"));
                    }
                    tm
                }
            };
            if let V::Func(_) = tm.unpack() {
                self.call_value(tm, &[t, key, val], 0)?;
                return Ok(());
            }
            t = tm;
            reg = None;
        }
        Err(self.rt_error("loop in settable"))
    }

    /// The client's `Arith` (`0x6f9a80`). Numbers (and numeric strings) compute, `^` through the
    /// global `__pow`; anything else tries the operands' metamethods, `__pow` for `^`.
    #[cold]
    #[inline(never)]
    fn arith(
        &mut self,
        op: Arith,
        a: Value,
        b: Value,
        regs: (Option<usize>, Option<usize>),
    ) -> Result<Value, LuaError> {
        if let (Some(x), Some(y)) = (self.tonumber(a), self.tonumber(b)) {
            return Ok(Value::Num(match op {
                Arith::Add => crate::number::x87_op(x + y, x, y),
                Arith::Sub => crate::number::x87_op(x - y, x, y),
                Arith::Mul => crate::number::x87_op(x * y, x, y),
                Arith::Div => crate::number::x87_op(x / y, x, y),
                Arith::Pow => return self.pow_global(x, y),
            }));
        }
        let name = match op {
            Arith::Add => self.names.add,
            Arith::Sub => self.names.sub,
            Arith::Mul => self.names.mul,
            Arith::Div => self.names.div,
            Arith::Pow => self.names.pow,
        };
        match self.bin_tm(a, b, name)? {
            Some(v) => Ok(v),
            None => {
                // `luaG_aritherror`: blame the first operand unless it converts.
                let (bad, reg) = if self.tonumber(a).is_none() {
                    (a, regs.0)
                } else {
                    (b, regs.1)
                };
                Err(self.type_error(bad, reg, "perform arithmetic on"))
            }
        }
    }

    /// The client's `OP_FORLOOP` checks, in its order, of the loop at `stack[ra..ra + 3]`: the
    /// counter must be a number, not coerced (`0x6f9262`, `` `for' initial value must be a
    /// number ``), and the limit and step are made numbers where they stand (`0x6f928d`,
    /// `0x6f92b3`) or the loop raises.
    #[cold]
    #[inline(never)]
    fn for_controls(&mut self, ra: usize) -> Result<(f64, f64, f64), LuaError> {
        let Some(idx) = self.stack[ra].as_num() else {
            return Err(self.rt_error("`for' initial value must be a number"));
        };
        let mut controls = [0.0; 2];
        for (i, what) in ["limit", "step"].into_iter().enumerate() {
            let Some(n) = self.tonumber(self.stack[ra + 1 + i]) else {
                return Err(self.rt_error(&format!("`for' {what} must be a number")));
            };
            self.stack[ra + 1 + i] = Value::Num(n);
            controls[i] = n;
        }
        Ok((idx, controls[0], controls[1]))
    }

    /// `x ^ y` on numbers, as the client's `Arith` computes it: the global `__pow`, read raw from
    /// the globals (`0x6f9b5e`), must be a function (`0x6f9b72`) and is called with the two
    /// numbers (`0x6f9b8c`); its first result is the answer.
    fn pow_global(&mut self, x: f64, y: f64) -> Result<Value, LuaError> {
        let f = self
            .heap
            .table(self.globals)
            .get_str(Value::Str(self.names.pow));
        if !matches!(f.unpack(), V::Func(_)) {
            return Err(self.rt_error("`__pow' (`^' operator) is not a function"));
        }
        self.call_tm_res(f, Value::Num(x), Value::Num(y))
    }

    /// `luaV_equalObj`: tables and userdata consult a shared `__eq`.
    #[cold]
    #[inline(never)]
    pub fn equals(&mut self, a: Value, b: Value) -> Result<bool, LuaError> {
        match (a.unpack(), b.unpack()) {
            (V::Table(x), V::Table(y)) if x != y => {}
            (V::User(x), V::User(y)) if x != y => {}
            _ => return Ok(a.raw_eq(b)),
        }
        let tm = self.comp_tm(a, b, self.names.eq);
        match tm {
            Some(f) => Ok(self.call_tm_res(f, a, b)?.truthy()),
            None => Ok(false),
        }
    }

    /// `get_compTM`: both operands' metatables must give the same metamethod.
    fn comp_tm(&self, a: Value, b: Value, name: StrRef) -> Option<Value> {
        let m1 = self.heap.metatable(a)?;
        let tm1 = self.heap.table(m1).get(Value::Str(name));
        if tm1.is_nil() {
            return None;
        }
        let m2 = self.heap.metatable(b)?;
        if m1 == m2 {
            return Some(tm1);
        }
        let tm2 = self.heap.table(m2).get(Value::Str(name));
        if tm1.raw_eq(tm2) {
            Some(tm1)
        } else {
            None
        }
    }

    /// `call_orderTM`: `Some(result)`, or `None` with no shared metamethod.
    fn order_tm(&mut self, a: Value, b: Value, name: StrRef) -> Result<Option<bool>, LuaError> {
        let tm1 = self.tm(a, name);
        if tm1.is_nil() {
            return Ok(None);
        }
        let tm2 = self.tm(b, name);
        if !tm1.raw_eq(tm2) {
            return Ok(None);
        }
        Ok(Some(self.call_tm_res(tm1, a, b)?.truthy()))
    }

    /// `l_strcmp` under the "C" locale: bytewise, embedded zeros included.
    fn str_lt(&self, a: StrRef, b: StrRef) -> std::cmp::Ordering {
        self.heap.str(a).cmp(self.heap.str(b))
    }

    #[cold]
    #[inline(never)]
    pub fn less_than(&mut self, a: Value, b: Value) -> Result<bool, LuaError> {
        match (a.unpack(), b.unpack()) {
            (V::Num(x), V::Num(y)) => Ok(x < y),
            (V::Str(x), V::Str(y)) => Ok(self.str_lt(x, y).is_lt()),
            _ if a.type_name() == b.type_name() => match self.order_tm(a, b, self.names.lt)? {
                Some(r) => Ok(r),
                None => Err(self.order_error(a, b)),
            },
            _ => Err(self.order_error(a, b)),
        }
    }

    #[cold]
    #[inline(never)]
    fn less_equal(&mut self, a: Value, b: Value) -> Result<bool, LuaError> {
        match (a.unpack(), b.unpack()) {
            (V::Num(x), V::Num(y)) => Ok(x <= y),
            (V::Str(x), V::Str(y)) => Ok(self.str_lt(x, y).is_le()),
            _ if a.type_name() == b.type_name() => {
                if let Some(r) = self.order_tm(a, b, self.names.le)? {
                    return Ok(r);
                }
                // 5.0: `a <= b` as `not (b < a)`.
                match self.order_tm(b, a, self.names.lt)? {
                    Some(r) => Ok(!r),
                    None => Err(self.order_error(a, b)),
                }
            }
            _ => Err(self.order_error(a, b)),
        }
    }

    /// 5.0's `luaV_concat` over `stack[first..=last]`, collapsing from the right; the result is
    /// left in `stack[first]`.
    #[cold]
    #[inline(never)]
    fn concat(&mut self, first: usize, last: usize) -> Result<(), LuaError> {
        let mut total = last - first + 1;
        let mut last = last;
        while total > 1 {
            let top = last + 1;
            let mut n = 2;
            let a = self.stack[top - 2];
            let b = self.stack[top - 1];
            let strnum = |v: Value| matches!(v.unpack(), V::Str(_) | V::Num(_));
            match (strnum(a), strnum(b)) {
                (true, true) => {
                    if matches!(b.unpack(), V::Str(s) if self.heap.str(s).is_empty()) {
                        // `a .. ""` is `a` as a string.
                        let sa = self.tostr(a).expect("a string or number");
                        self.stack[top - 2] = Value::Str(sa);
                    } else {
                        while n < total && strnum(self.stack[top - n - 1]) {
                            n += 1;
                        }
                        // A number is formatted straight into the result: 5.0 interns its text
                        // first, a string nothing else ever sees.
                        let mut buf = std::mem::take(&mut self.concat_buf);
                        buf.clear();
                        // Each piece grows the buffer fallibly, a number's text at most 24 bytes:
                        // past `MAX_STRING`, or room the allocator refuses, is `not enough
                        // memory` (C's `luaV_concat` allocates the sum and fails there).
                        let mut refused = false;
                        for i in (1..=n).rev() {
                            match self.stack[top - i].unpack() {
                                V::Str(s) => {
                                    let b = self.heap.str(s);
                                    if b.len() > buf.capacity() - buf.len()
                                        && !crate::api::grow_fallibly(&mut buf, b.len())
                                    {
                                        refused = true;
                                        break;
                                    }
                                    buf.extend_from_slice(b);
                                }
                                V::Num(x) => {
                                    if 24 > buf.capacity() - buf.len()
                                        && !crate::api::grow_fallibly(&mut buf, 24)
                                    {
                                        refused = true;
                                        break;
                                    }
                                    crate::number::push_g14(x, &mut buf);
                                }
                                _ => unreachable!("checked above"),
                            }
                        }
                        if refused {
                            return Err(self.memory_error());
                        }
                        self.heap.concat_space(buf.len());
                        self.stack[top - n] = self.str(&buf);
                        // A large buffer is not kept: one long result would hold its memory.
                        if buf.capacity() <= 64 * 1024 {
                            self.concat_buf = buf;
                        }
                    }
                }
                _ => match self.bin_tm(a, b, self.names.concat)? {
                    Some(v) => self.stack[top - 2] = v,
                    None => {
                        // `luaG_concaterror`: blame the first operand unless it is a string.
                        let (bad, reg) = if matches!(a.unpack(), V::Str(_) | V::Num(_)) {
                            (b, top - 1)
                        } else {
                            (a, top - 2)
                        };
                        let base = self.frames.last().unwrap().base();
                        return Err(self.type_error(bad, Some(reg - base), "concatenate"));
                    }
                },
            }
            total -= n - 1;
            last -= n - 1;
        }
        if self.heap.oom {
            return Err(self.memory_error());
        }
        Ok(())
    }

    // ── calls ────────────────────────────────────────────────────────────────────────────────

    /// Call `f` with `args` from the host or a native, answering `nres` results (-1: all).
    pub fn call_value(
        &mut self,
        f: Value,
        args: &[Value],
        nres: i32,
    ) -> Result<Vec<Value>, LuaError> {
        let func_idx = self.top;
        self.ensure(func_idx + 1 + args.len() + 1);
        self.stack[func_idx] = f;
        self.stack[func_idx + 1..func_idx + 1 + args.len()].copy_from_slice(args);
        self.top = func_idx + 1 + args.len();
        self.call_at(func_idx, args.len(), nres)?;
        let out = self.stack[func_idx..self.top].to_vec();
        self.top = func_idx;
        Ok(out)
    }

    /// Call the function at `func_idx` with the `nargs` values above it; its results replace
    /// them, `top` just past the last. Runs a Lua function to completion.
    pub fn call_at(&mut self, func_idx: usize, nargs: usize, nres: i32) -> Result<(), LuaError> {
        // From the host into Lua: a profile charges what runs until it is back out.
        if !self.frames.is_empty() {
            return self.call_at_inner(func_idx, nargs, nres);
        }
        self.clear_below = 0;
        if self.abort.take().is_some() {
            // One a panic carried past the host's last call.
            self.hooked = self.hook.is_some() || self.profile.is_some();
        }
        let mut r = self.unwind_guard(|vm| vm.call_at_inner(func_idx, nargs, nres));
        if let Some(p) = self.profile.as_mut() {
            p.close();
        }
        // An abort ends here, at the host, even where a native caught it.
        if let Some(msg) = self.abort.take() {
            self.hooked = self.hook.is_some() || self.profile.is_some();
            let v = self.str(&msg);
            r = Err(LuaError { value: v });
        }
        r
    }

    /// Run `f`, the host's entry into the VM: a Rust panic that unwinds out of it (a host
    /// function's, or the VM's own) first puts the VM back as it was at entry, so the state
    /// stays usable after the host catches the panic.
    fn unwind_guard<T>(&mut self, f: impl FnOnce(&mut Vm) -> T) -> T {
        let at = (
            self.frames.len(),
            self.c_depth,
            self.top,
            self.handlers.len(),
            self.in_handler,
            self.in_hook,
        );
        let id = self.identity();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))) {
            Ok(v) => v,
            // Another VM in this one's place has its own state to keep.
            Err(payload) if self.identity() != id => std::panic::resume_unwind(payload),
            Err(payload) => {
                let (frames, c_depth, top, handlers, in_handler, in_hook) = at;
                self.close_upvals(top);
                self.frames.truncate(frames);
                self.c_depth = c_depth;
                self.top = top;
                self.handlers.truncate(handlers);
                self.in_handler = in_handler;
                self.in_hook = in_hook;
                self.restore_stack_limit();
                std::panic::resume_unwind(payload)
            }
        }
    }

    /// A call from the host or a native into Lua. The running chunk is the caller's again when
    /// it returns, so the profile and the table census charge what the caller does next to it.
    fn call_at_inner(&mut self, func_idx: usize, nargs: usize, nres: i32) -> Result<(), LuaError> {
        let src = self.heap.cur_source;
        let r = self.call_at_checked(func_idx, nargs, nres);
        if self.heap.cur_source != src {
            self.heap.cur_source = src;
            if self.hooked {
                self.prof_switch();
            }
        }
        r
    }

    fn call_at_checked(
        &mut self,
        func_idx: usize,
        nargs: usize,
        nres: i32,
    ) -> Result<(), LuaError> {
        self.c_depth += 1;
        let marker = 0u8;
        let here = std::ptr::addr_of!(marker) as usize;
        if self.c_depth == 1 {
            self.stack_base = here;
        }
        // Miri gives each local its own allocation, at no stack address: no measure there.
        let used = if cfg!(miri) {
            0
        } else {
            self.stack_base.abs_diff(here)
        };
        if self.c_depth >= MAXCCALLS || used > MAX_RUST_STACK {
            // 5.1.5's `luaD_call`: the error at the limit, raised at the deeper level so its
            // handler has room; a handler's calls past the margin raise without a handler.
            let e = if self.c_depth >= MAXCCALLS + (MAXCCALLS >> 3)
                || used > MAX_RUST_STACK + (MAX_RUST_STACK >> 3)
            {
                self.errerr()
            } else if self.c_depth == MAXCCALLS || (used > MAX_RUST_STACK && self.in_handler == 0) {
                self.rt_error("C stack overflow")
            } else {
                return self.call_at_body(func_idx, nargs, nres);
            };
            self.c_depth -= 1;
            return Err(e);
        }
        self.call_at_body(func_idx, nargs, nres)
    }

    #[inline(always)]
    fn call_at_body(&mut self, func_idx: usize, nargs: usize, nres: i32) -> Result<(), LuaError> {
        let r = match self.precall(func_idx, nargs, nres) {
            Ok(Called::Lua) => {
                self.frames.last_mut().unwrap().boundary = true;
                self.execute()
            }
            Ok(Called::Done) => Ok(()),
            Err(e) => Err(e),
        };
        self.c_depth -= 1;
        r
    }

    /// A protected call: on error the stack and frames unwind to here, open upvalues above
    /// close, and the error is the result.
    pub fn pcall_at(
        &mut self,
        func_idx: usize,
        nargs: usize,
        nres: i32,
        handler: Option<Value>,
    ) -> Result<(), LuaError> {
        let frames = self.frames.len();
        let c_depth = self.c_depth;
        let handlers = self.handlers.len();
        self.handlers.push(handler);
        // A panic that passes leaves this call's handler, frames and depth as they were, so a
        // native that catches it finds the state as at this call.
        let r = self.unwind_guard(|vm| vm.call_at(func_idx, nargs, nres));
        self.handlers.truncate(handlers);
        if let Err(e) = r {
            self.close_upvals(func_idx);
            self.frames.truncate(frames);
            self.c_depth = c_depth;
            self.top = func_idx;
            self.restore_stack_limit();
            return Err(e);
        }
        Ok(())
    }

    /// 5.0's `luaD_precall`.
    #[cold]
    #[inline(never)]
    fn precall(&mut self, func_idx: usize, nargs: usize, nres: i32) -> Result<Called, LuaError> {
        self.precall_clearing(func_idx, nargs, nres, false)
    }

    /// [`Vm::precall`]; `clear` clears a Lua frame's registers wherever it starts.
    #[cold]
    #[inline(never)]
    fn precall_clearing(
        &mut self,
        func_idx: usize,
        nargs: usize,
        nres: i32,
        clear: bool,
    ) -> Result<Called, LuaError> {
        let mut nargs = nargs;
        let fref = match self.stack[func_idx].unpack() {
            V::Func(f) => f,
            _ => {
                nargs = self.tryfunc(func_idx, nargs)?;
                self.stack[func_idx]
                    .as_func()
                    .expect("`tryfunc` placed a function")
            }
        };
        if self.frames.len() >= self.ci_limit {
            self.grow_ci()?;
        }
        match self.heap.func(fref) {
            FuncObj::Lua(LuaFunc {
                proto, upvals, env, ..
            }) => {
                let pp = Rc::as_ptr(proto);
                let (fup, fenv): (*const [u32], TableRef) = (&**upvals, *env);
                // SAFETY: the closure being called holds this code, and the closure sits in the
                // stack slot `func_idx` for the whole call.
                let proto = unsafe { &*pp };
                let base = func_idx + 1;
                let np = proto.nparams as usize;
                let frame_top = base + proto.maxstack as usize;
                self.checkstack(precall_top50(base, nargs, proto), proto.c_maxstack as usize);
                self.ensure(frame_top + 1);
                let mut fill_from = base + nargs;
                if proto.vararg {
                    // 5.0's `adjust_varargs`: the extras into `arg`, made by
                    // `luaH_new(L, actual, 1)` (`0x6f6266`), with `n`.
                    let extra = nargs.saturating_sub(np);
                    let t = self.heap.new_table(extra, 1);
                    for i in 0..extra {
                        let v = self.stack[base + np + i];
                        self.heap
                            .table_mut(t)
                            .set_int(i as i32 + 1, v)
                            .expect("the presized array part holds every extra");
                    }
                    let n = Value::Str(self.names.n);
                    self.heap
                        .table_mut(t)
                        .set(n, Value::Num(extra as f64))
                        .unwrap();
                    for i in nargs..np {
                        self.stack[base + i] = Value::Nil;
                    }
                    self.stack[base + np] = Value::Table(t);
                    fill_from = base + np + 1;
                } else if nargs > np {
                    fill_from = base + np;
                }
                // Missing parameters are nil; below `clear_below`, every register past the
                // parameters too, extra arguments included, as `luaD_precall` clears them.
                if proto.clears {
                    self.clear_below = self.clear_below.max(frame_top);
                }
                let end = if clear || base < self.clear_below {
                    base + proto.c_maxstack as usize
                } else {
                    base + np
                };
                if fill_from < end {
                    for v in &mut self.stack[fill_from..end] {
                        *v = Value::Nil;
                    }
                }
                self.top = frame_top;
                self.frames.push(Frame {
                    func: fref,
                    base: base as u32,
                    ip: proto.code.as_ptr(),
                    nresults: nres,
                    consts: proto.consts.as_ptr(),
                    maxstack: proto.maxstack,
                    source: proto.source_id,
                    proto: pp,
                    upvals: fup.cast::<u32>(),
                    env: fenv,
                    env_gen: self.fenv_gen,
                    tailcalls: 0,
                    boundary: false,
                });
                // The client's `adjust_varargs` makes `arg` without checking the collector
                // (`0x6f6200`..`0x6f6366`), where 5.1.5's checks it.
                Ok(Called::Lua)
            }
            FuncObj::Native { .. } => {
                self.call_native(fref, func_idx, nargs, nres)?;
                Ok(Called::Done)
            }
        }
    }

    /// 5.0's `luaD_precall` for a native, the call depth already checked, then
    /// `luaD_poscall`: the native `fref` at `func_idx` runs on the `nargs` values above it, and
    /// its results replace them.
    #[inline(never)]
    fn call_native(
        &mut self,
        fref: FuncRef,
        func_idx: usize,
        nargs: usize,
        nres: i32,
    ) -> Result<(), LuaError> {
        let FuncObj::Native { f, host, .. } = self.heap.func(fref) else {
            unreachable!("a native")
        };
        let f = *f;
        let host = host.clone();
        let base = func_idx + 1;
        self.top = base + nargs;
        self.checkstack(self.top, LUA_MINSTACK);
        self.frames.push(Frame {
            func: fref,
            base: base as u32,
            ip: std::ptr::null(),
            nresults: nres,
            consts: std::ptr::null(),
            maxstack: 0,
            source: self.heap.cur_source,
            proto: std::ptr::null(),
            upvals: std::ptr::null(),
            env: self.globals,
            env_gen: self.fenv_gen,
            tailcalls: 0,
            boundary: false,
        });
        let id = self.identity();
        let r = match host {
            Some(h) => h(self, nargs),
            None => f(self, nargs),
        };
        self.check_identity(id);
        let n = r?;
        // A string the native made could not be: it raises here, in its caller.
        if self.heap.oom {
            return Err(self.memory_error());
        }
        self.frames.pop();
        let first = self.top - n;
        // One result wanted, at least one given: the common call in an expression.
        if nres == 1 && n >= 1 {
            self.stack[func_idx] = self.stack[first];
            self.top = func_idx + 1;
        } else if nres == 0 {
            // None, a call statement.
            self.top = func_idx;
        } else {
            self.poscall(func_idx, first, n, nres);
        }
        Ok(())
    }

    /// 5.1's `tryfuncTM`: the value at `func_idx` is not a function, so its `__call` goes below
    /// the arguments, the value first among them; answers the new argument count.
    #[cold]
    #[inline(never)]
    fn tryfunc(&mut self, func_idx: usize, nargs: usize) -> Result<usize, LuaError> {
        let other = self.stack[func_idx];
        let tm = self.tm(other, self.names.call);
        if !matches!(tm.unpack(), V::Func(_)) {
            // A name only for a register of the running Lua function, as 5.1's `isinstack`: a
            // metamethod is called from above them.
            let reg = self
                .frames
                .last()
                .filter(|f| !f.proto.is_null())
                .map(|f| func_idx.wrapping_sub(f.base()))
                .filter(|&r| r < self.frames.last().map_or(0, |f| f.maxstack as usize));
            return Err(self.type_error(other, reg, "call"));
        }
        self.ensure(func_idx + nargs + 2);
        self.stack
            .copy_within(func_idx..func_idx + nargs + 1, func_idx + 1);
        self.stack[func_idx] = tm;
        self.top = func_idx + nargs + 2;
        Ok(nargs + 1)
    }

    /// `luaD_growCI` (`0x6f61c0`): the call-info array doubles; past [`MAXCALLS`] the call is
    /// refused with "stack overflow" (`0x6f61f2`), and a call while it is past the limit (the
    /// overflow's handler recursing) with no handler at all (`0x6f61d3`).
    #[cold]
    #[inline(never)]
    fn grow_ci(&mut self) -> Result<(), LuaError> {
        if self.ci_size > MAXCALLS {
            return Err(self.errerr());
        }
        self.set_ci_size(self.ci_size * 2);
        if self.ci_size > MAXCALLS {
            return Err(self.rt_error("stack overflow"));
        }
        Ok(())
    }

    fn set_ci_size(&mut self, n: usize) {
        self.heap.recharge(CI_BYTES * self.ci_size, CI_BYTES * n);
        let b = self.heap.pools().realloc(self.ci_block, CI_BYTES * n);
        self.ci_block = b;
        self.ci_size = n;
        self.ci_limit = n - 1;
    }

    /// 5.1.5's `restore_stack_limit`, after a protected call unwinds: an array an overflow grew
    /// past the limit goes back to it, if the frames still in use leave room.
    fn restore_stack_limit(&mut self) {
        if self.ci_size > MAXCALLS && self.frames.len() + 1 < MAXCALLS {
            self.set_ci_size(MAXCALLS);
        }
    }

    /// 5.1.5's `LUA_ERRERR`: thrown as is, no handler run, its message the one
    /// `luaD_seterrorobj` writes.
    fn errerr(&mut self) -> LuaError {
        LuaError {
            value: self.str(b"error in error handling"),
        }
    }

    /// 5.0's `luaD_poscall`: `n` results from `first` move to `dest`, adjusted to `wanted`.
    #[cold]
    #[inline(never)]
    fn poscall(&mut self, dest: usize, first: usize, n: usize, wanted: i32) {
        // One result wanted, at least one given: the common call in an expression.
        if wanted == 1 && n >= 1 {
            self.stack[dest] = self.stack[first];
            self.top = dest + 1;
            return;
        }
        if wanted < 0 {
            self.stack.copy_within(first..first + n, dest);
            self.top = dest + n;
        } else {
            let wanted = wanted as usize;
            self.ensure(dest + wanted + 1);
            let k = n.min(wanted);
            self.stack.copy_within(first..first + k, dest);
            for v in &mut self.stack[dest + k..dest + wanted] {
                *v = Value::Nil;
            }
            self.top = dest + wanted;
        }
    }

    // ── natives' view ────────────────────────────────────────────────────────────────────────

    /// The `i`th argument (0-based) of the running native, or nil.
    #[inline]
    pub fn arg(&self, nargs: usize, i: usize) -> Value {
        if i < nargs {
            self.stack[self.frames.last().unwrap().base() + i]
        } else {
            Value::Nil
        }
    }

    /// The running native's arguments.
    pub fn args(&self, nargs: usize) -> &[Value] {
        let b = self.frames.last().unwrap().base();
        &self.stack[b..b + nargs]
    }

    /// The running function's environment table (`getfenv(0)`'s answer for a native).
    pub fn current_env(&self) -> TableRef {
        let f = self.frames.last().unwrap().func;
        match self.heap.func(f) {
            FuncObj::Lua(LuaFunc { env, .. }) | FuncObj::Native { env, .. } => *env,
        }
    }

    /// `setfenv` of a Lua function: its environment becomes `env`, and every running frame
    /// rereads its cached one. False for a native, whose environment does not change.
    pub fn set_function_env(&mut self, f: FuncRef, env: TableRef) -> bool {
        match self.heap.func_mut(f) {
            FuncObj::Lua(l) => {
                l.env = env;
                self.fenv_gen = self.fenv_gen.wrapping_add(1);
                true
            }
            FuncObj::Native { .. } => false,
        }
    }

    /// The value of upvalue `u` (a handle from `LuaFunc::upvals`): its stack slot's while open.
    pub fn read_upvalue(&self, u: u32) -> Value {
        match self.heap.upval(u) {
            UpvalObj::Open(i) => self.stack.get(i as usize).copied().unwrap_or(Value::Nil),
            UpvalObj::Closed(v) => v,
        }
    }

    /// Set upvalue `u`, as `SETUPVAL` does: its stack slot while open, else the upvalue.
    pub fn write_upvalue(&mut self, u: u32, v: Value) {
        match self.heap.upval(u) {
            UpvalObj::Open(i) => {
                if let Some(slot) = self.stack.get_mut(i as usize) {
                    *slot = v;
                }
            }
            UpvalObj::Closed(_) => *self.heap.upval_mut(u) = UpvalObj::Closed(v),
        }
    }

    // ── host roots ───────────────────────────────────────────────────────────────────────────

    /// Hold `v` for the host: a collector root until [`Vm::unroot`] brings its count to zero.
    pub fn root(&mut self, v: Value) -> u32 {
        self.roots.0.root(v)
    }

    /// Another holder of root `i`.
    pub fn retain(&mut self, i: u32) {
        self.roots.0.retain(i);
    }

    /// One holder of root `i` lets go.
    pub fn unroot(&mut self, i: u32) {
        self.roots.0.unroot(i);
    }

    pub fn rooted(&self, i: u32) -> Value {
        self.roots.0.get(i)
    }

    /// Root `i` if it is the one with `serial` ([`RootTable::get_as`]).
    pub fn rooted_as(&self, i: u32, serial: u64) -> Option<Value> {
        self.roots.0.get_as(i, serial)
    }

    /// The root table itself, which a host handle keeps to let go of its root without reaching
    /// the VM: a handle dropped while the VM runs (a host closure the collector frees) or after
    /// it is gone touches only the table.
    pub fn root_table(&self) -> Rc<RootTable> {
        self.roots.0.clone()
    }

    // ── the hook ─────────────────────────────────────────────────────────────────────────────

    /// Run `f` once for every `step` instructions, at the next back edge, call or return
    /// ([`Hook`]).
    pub fn set_hook(&mut self, step: u64, f: HookFn) {
        self.hook = Some(Hook {
            step: step.max(1),
            f,
            counted: 0,
        });
        self.hooked = true;
    }

    pub fn remove_hook(&mut self) {
        self.hook = None;
        self.hooked = self.profile.is_some() || self.abort.is_some();
    }

    /// Raise `msg` past every protected call, to the host's call into the VM: no error handler
    /// runs, and until that call returns every raise is this one and every loop back edge, call
    /// and return raises it again. A `pcall` that catches it lets its caller run only to the next
    /// of those, so the unwinding ends. A host's guard against a script that never ends.
    pub fn abort(&mut self, msg: &[u8]) -> LuaError {
        self.abort = Some(msg.into());
        self.hooked = true;
        self.abort_error()
    }

    /// The abort in flight, raised again.
    #[cold]
    #[inline(never)]
    fn abort_error(&mut self) -> LuaError {
        let msg = self.abort.clone().expect("an abort");
        let v = self.str(&msg);
        LuaError { value: v }
    }

    /// Start or stop charging time to chunks; starting keeps what was charged so far.
    pub fn set_profiling(&mut self, on: bool) {
        if on {
            self.profile.get_or_insert_with(Default::default);
        } else {
            self.profile = None;
        }
        self.hooked = self.hook.is_some() || self.profile.is_some() || self.abort.is_some();
    }

    /// The running chunk changed: close the segment and open one for it.
    #[cold]
    #[inline(never)]
    fn prof_switch(&mut self) {
        let src = self.heap.cur_source;
        if let Some(p) = self.profile.as_mut() {
            p.mark(src);
        }
    }

    /// `n` more instructions ran, counted toward the hook's next run without running it.
    #[cold]
    #[inline(never)]
    fn hook_add(&mut self, n: u64) {
        if let Some(h) = self.hook.as_mut().filter(|_| !self.in_hook) {
            h.counted += n;
        }
    }

    /// `n` more instructions ran, as 5.1's `LUA_MASKCOUNT` counts them: the hook runs once for
    /// each `step` they complete.
    #[cold]
    #[inline(never)]
    fn hook_count(&mut self, n: u64) -> Result<(), LuaError> {
        if self.abort.is_some() {
            return Err(self.abort_error());
        }
        if self.in_hook {
            return Ok(());
        }
        let Some(h) = self.hook.as_mut() else {
            return Ok(());
        };
        h.counted += n;
        while let Some(h) = self.hook.as_mut().filter(|h| h.counted >= h.step) {
            h.counted -= h.step;
            let f = h.f.clone();
            self.in_hook = true;
            let id = self.identity();
            let r = f(self);
            self.check_identity(id);
            self.in_hook = false;
            r?;
        }
        Ok(())
    }

    /// This VM's and its heap's identity ([`crate::heap::Identity`]).
    #[inline(always)]
    fn identity(&self) -> (u64, u64) {
        (self.identity.0, self.heap.identity.0)
    }

    /// Stop if a native replaced the VM or its heap: the running frames' code and stack are
    /// gone, and a Lua error would resume them.
    #[inline(always)]
    fn check_identity(&self, id: (u64, u64)) {
        #[cold]
        #[inline(never)]
        fn replaced() -> ! {
            panic!("a native replaced the running Lua VM or its heap");
        }
        if self.identity() != id {
            replaced();
        }
    }

    // ── stack inspection ─────────────────────────────────────────────────────────────────────

    /// The stack level `level` up, as `lua_getstack` and `lua_getinfo(L, "Snl")` see it; a
    /// level a tail call replaced is a `"tail"` frame.
    pub fn frame_info(&self, level: usize) -> Option<FrameInfo> {
        let mut lv = level as i64;
        let mut i = self.frames.len();
        while i > 0 {
            i -= 1;
            if lv == 0 {
                return Some(self.describe(i));
            }
            lv -= 1;
            if !self.frames[i].proto.is_null() {
                lv -= self.frames[i].tailcalls as i64;
            }
            if lv < 0 {
                return Some(FrameInfo {
                    what: "tail",
                    ..Default::default()
                });
            }
        }
        None
    }

    fn describe(&self, i: usize) -> FrameInfo {
        let f = &self.frames[i];
        let (name, namewhat) = match self.name_of(i) {
            Some((kind, n)) => (Some(n), kind),
            None => (None, ""),
        };
        // SAFETY: a live frame's code is alive (see `Frame::proto`).
        match unsafe { f.proto.as_ref() } {
            Some(p) => {
                let pc = f.pc().saturating_sub(1);
                FrameInfo {
                    source: Some(p.source.to_vec()),
                    what: if p.line_defined == 0 { "main" } else { "Lua" },
                    current_line: p.lines.get(pc).copied(),
                    line_defined: p.line_defined,
                    name,
                    namewhat,
                    func: Value::Func(f.func),
                }
            }
            None => FrameInfo {
                source: None,
                what: "C",
                current_line: None,
                line_defined: 0,
                name,
                namewhat,
                func: Value::Func(f.func),
            },
        }
    }

    /// 5.0's `getfuncname` for frame `i`: what the caller's calling instruction named it.
    fn name_of(&self, i: usize) -> Option<(&'static str, Vec<u8>)> {
        if i == 0 || self.frames[i].tailcalls > 0 {
            return None;
        }
        let caller = &self.frames[i - 1];
        // SAFETY: a live frame's code is alive (see `Frame::proto`).
        let p = unsafe { caller.proto.as_ref() }?;
        let pc = caller.pc().checked_sub(1)?;
        match p.code.get(pc)? {
            // The client's `getfuncname` names a function only an `OP_CALL` or `OP_TAILCALL`
            // called (`0x6fc499`..`0x6fc4a1`): a generic `for`'s generator is named nothing.
            Op::Call(a, ..) | Op::TailCall(a, ..) => objname_in(p, &self.heap, pc, *a as usize),
            _ => None,
        }
    }

    /// The name a native was called by, as 5.0's `getfuncname` finds it from the calling
    /// instruction: `("global", "strfind")`, `("method", "SetText")`.
    pub fn called_name(&self) -> Option<(&'static str, Vec<u8>)> {
        let n = self.frames.len();
        if n < 2 {
            return None;
        }
        let me = &self.frames[n - 1];
        let caller = &self.frames[n - 2];
        if me.tailcalls > 0 {
            return None;
        }
        // SAFETY: a live frame's code is alive (see `Frame::proto`).
        let p = unsafe { caller.proto.as_ref() }?;
        let pc = caller.pc().checked_sub(1)?;
        match p.code.get(pc)? {
            // The client's `getfuncname` names a function only an `OP_CALL` or `OP_TAILCALL`
            // called (`0x6fc499`..`0x6fc4a1`): a generic `for`'s generator is named nothing.
            Op::Call(a, ..) | Op::TailCall(a, ..) => objname_in(p, &self.heap, pc, *a as usize),
            _ => None,
        }
    }

    /// `luaL_argerror` (`0x6f4810`): `bad argument #n to `name' (msg)`, with a method's `self` not
    /// counted, the name's bytes as the caller wrote them.
    pub fn arg_error(&mut self, narg: usize, msg: &str) -> LuaError {
        let msg = msg.as_bytes();
        let named = self.called_name();
        let mut narg = narg;
        let name: Vec<u8> = match named {
            Some((kind, n)) => {
                if kind == "method" {
                    narg -= 1;
                    if narg == 0 {
                        let m = [b"calling `", &n[..], b"' on bad self (", msg, b")"].concat();
                        return self.error_at_bytes(1, &m);
                    }
                }
                n
            }
            None => b"?".to_vec(),
        };
        let head = format!("bad argument #{narg} to `");
        let m = [head.as_bytes(), &name, b"' (", msg, b")"].concat();
        self.error_at_bytes(1, &m)
    }

    /// `luaL_typerror`: `<expected> expected, got <type>`; an absent argument is "no value".
    pub fn type_arg_error(&mut self, nargs: usize, narg: usize, expected: &str) -> LuaError {
        let got = if narg > nargs {
            "no value"
        } else {
            self.arg(nargs, narg - 1).type_name()
        };
        self.arg_error(narg, &format!("{expected} expected, got {got}"))
    }

    // ── collection ───────────────────────────────────────────────────────────────────────────

    /// What the state holds outside its objects, in the collector's count: its records, its
    /// stack and its call records.
    /// 5.0's `checkstacksizes` (`0x6f7900`), which `traversestack` runs once a collection as it
    /// marks the thread (`0x6f78ed`): call records under a quarter in use halve while more than
    /// 16 (`0x6f7925`..`0x6f7936`), then a stack under a quarter in use up to the highest Lua
    /// frame's `ci->top` halves while more than 90 slots (`0x6f7949`..`0x6f795e`).
    fn check_stack_sizes(&mut self) {
        if 4 * self.frames.len() < self.ci_size && self.ci_size > 2 * BASIC_CI_SIZE {
            self.set_ci_size(self.ci_size / 2);
        }
        // `lim` (`0x6f7880`..`0x6f78a4`): the top, or the highest `ci->top` of a Lua frame, the
        // registers the client's code reserves (`Proto::c_maxstack`). The innermost Lua frame's
        // top is its `ci->top` in the client, where benilla's may stand above it.
        let mut lim = match self.frames.last() {
            Some(f) if !f.proto.is_null() => 0,
            _ => self.top,
        };
        // What benilla's own frames use, which a halving must keep.
        let mut used = self.top;
        for f in &self.frames {
            if !f.proto.is_null() {
                // SAFETY: a frame's code lives as long as the frame (`Frame::proto`).
                let cmax = unsafe { &*f.proto }.c_maxstack as usize;
                lim = lim.max(f.base as usize + cmax);
                used = used.max(f.base as usize + f.maxstack as usize + 1);
            }
        }
        let size = self.stack.len();
        if 4 * lim < size && size > 2 * BASIC_STACK_SIZE {
            // Deviation: benilla's scratch registers above a frame's `c_maxstack`
            // (`Proto::maxstack`) stay on the stack; the client has none, and halves.
            if used <= size / 2 {
                self.realloc_stack(size / 2);
            }
        }
    }

    fn state_bytes(&self) -> usize {
        STATE_BYTES + STACK_SLOT_BYTES * self.stack.len() + CI_BYTES * self.ci_size
    }

    /// 5.0's `luaC_checkGC`, at a safe point: every live value is on the stack or reachable.
    #[inline]
    pub fn check_gc(&mut self) {
        if self.heap.bytes() >= self.heap.trigger || self.gc_stress {
            self.full_gc();
        }
    }

    /// 5.0's `luaC_collectgarbage` (`0x6f7340`): mark, which checks the stack's sizes as it
    /// traverses it, sweep, `checkSizes`, and the client's threshold (`0x6fae00`).
    #[cold]
    #[inline(never)]
    pub fn full_gc(&mut self) {
        self.heap.collections += 1;
        self.check_stack_sizes();
        let mut roots: Vec<Value> = self.stack[..self.top].to_vec();
        roots.push(Value::Table(self.globals));
        roots.push(Value::Table(self.registry));
        for f in &self.frames {
            roots.push(Value::Func(f.func));
        }
        roots.extend(self.handlers.iter().flatten().copied());
        roots.extend(self.roots.0.live());
        // The generic `for` fast path compares against it by identity: a freed `next` whose id
        // a new function took would step a table without calling that function.
        roots.push(self.builtin_next);
        let open: Vec<u32> = self.open.iter().map(|&(_, u)| u).collect();
        // Names live forever.
        let n = &self.names;
        for s in [
            n.index,
            n.newindex,
            n.call,
            n.add,
            n.sub,
            n.mul,
            n.div,
            n.pow,
            n.unm,
            n.concat,
            n.eq,
            n.lt,
            n.le,
            n.mode,
            n.metatable,
            n.tostring,
            n.n,
            n.next,
        ] {
            roots.push(Value::Str(s));
        }
        // A frame running code a hot reload replaced reads that code's constants and upvalues;
        // with no frame left, nothing runs it.
        if self.frames.is_empty() {
            self.retired.clear();
        }
        let fixed = [self.stack_block, self.ci_block];
        self.heap
            .collect(&roots, &open, &self.retired, self.names.mode, &fixed);
        self.heap.charge(self.state_bytes());
        self.heap.pace();
        // The host closures it freed drop last, with no arena borrowed, as host code: their
        // captures' destructors may use the state.
        if !self.heap.freed_hosts.is_empty() {
            let freed = std::mem::take(&mut self.heap.freed_hosts);
            // The host's `reentry` hands them this `&mut Vm`, while Lua may be running on it.
            let id = self.identity();
            self.as_host(move || drop(freed));
            self.check_identity(id);
        }
    }

    /// Run `f`, host code that may reach this VM through its host (a destructor that calls the
    /// host's API), through `reentry` when the host set one, as a callback runs.
    pub fn as_host(&mut self, f: impl FnOnce()) {
        let mut f = Some(f);
        let mut once = || {
            if let Some(f) = f.take() {
                f()
            }
        };
        match self.reentry.clone() {
            Some(r) => r(self, &mut once),
            None => once(),
        }
    }

    // ── the interpreter ──────────────────────────────────────────────────────────────────────

    /// Run the top Lua frame until the frame that entered with `boundary` returns. The dispatch
    /// loop has two instantiations: the fast one while no hook or profile is on and no running
    /// function can read a register it has not written (`clear_below`), the slow one, which counts
    /// for the hook and clears frames as `luaD_precall`, otherwise. A hook set while the fast
    /// one runs moves it to the slow one at the next call or loop back edge, a function that
    /// needs its frames cleared as it is entered.
    fn execute(&mut self) -> Result<(), LuaError> {
        loop {
            let done = if self.hooked || self.clear_below != 0 {
                self.run::<true>()?
            } else {
                self.run::<false>()?
            };
            if done {
                return Ok(());
            }
        }
    }

    /// [`Vm::execute`]'s dispatch loop: `true` when the frame that entered with `boundary` has
    /// returned, `false` to go on in the other instantiation from the top frame's saved place.
    ///
    /// The register file is addressed through `regs`, a raw pointer to `stack[base]`, as C Lua
    /// keeps `base` in a register. Anything that can grow the stack (a call, a metamethod, an
    /// upvalue that touches the stack) is followed by `refresh!`, which recomputes it.
    // `fr!` carries its own `unsafe`, nested in others at some uses.
    #[allow(unused_unsafe)]
    #[inline(never)]
    fn run<const SLOW: bool>(&mut self) -> Result<bool, LuaError> {
        // The running frame's state, kept in locals as C Lua's `luaV_execute` keeps it. A Lua
        // call sets it straight from the callee; a return reloads it from the caller's frame.
        // Only the three values every instruction touches live in locals; the rest of the
        // running frame is read from it (`fr!`), so they keep their registers.
        let mut ip: *const Op = std::ptr::null();
        let mut k: *const Value;
        let mut regs: *mut Value;
        // While a hook is set, where the current straight run of instructions began, in
        // `seg_code`'s code: the run is charged at a back edge, a forward jump, a call and a
        // return (here, as the caller's frame is entered again), so each charge is the
        // instructions executed.
        let mut seg: *const Op = std::ptr::null();
        let mut seg_code: *const Proto = std::ptr::null();
        'frame: loop {
            {
                if SLOW && self.hooked && !seg.is_null() && seg != ip {
                    // The last run of the frame returned from; a call's run is charged already.
                    // SAFETY: the returned function's code is alive: its closure is unreachable
                    // at most since the return, and nothing has collected since.
                    let n = ran_since(seg, ip, unsafe { &*seg_code });
                    if n > 0 {
                        self.hook_count(n)?;
                    }
                }
                let frame = self.frames.last().expect("a Lua frame");
                let base = frame.base();
                ip = frame.ip;
                k = frame.consts;
                self.heap.cur_source = frame.source;
                if SLOW {
                    if self.hooked {
                        seg = ip;
                        seg_code = frame.proto;
                        self.prof_switch();
                    } else {
                        // No run to keep: a hook set later starts from its first charge.
                        seg = std::ptr::null();
                    }
                }
                // SAFETY: `precall` grew the stack to cover `base + maxstack`, and only a
                // collection shrinks it, never below a frame's registers, which `gc!` and `slow!`
                // refresh after.
                regs = unsafe { self.stack.as_mut_ptr().add(base) };
            }
            // The running frame, the last one. SAFETY: `execute` runs while its frame is on the
            // list; the reference is used within one statement, before any push moves the list.
            macro_rules! fr {
                () => {
                    unsafe { self.frames.last().unwrap_unchecked() }
                };
            }
            macro_rules! fr_mut {
                () => {
                    unsafe { self.frames.last_mut().unwrap_unchecked() }
                };
            }
            macro_rules! base {
                () => {
                    (fr!().base as usize)
                };
            }
            macro_rules! maxstack {
                () => {
                    (fr!().maxstack as usize)
                };
            }
            // SAFETY: the upvalue list lives as the code does (`Frame::upvals`), and
            // `Proto::verify` checked every index against the function's upvalues.
            macro_rules! upval {
                ($i:expr) => {
                    unsafe { *fr!().upvals.add($i as usize) }
                };
            }
            // SAFETY: `Proto::verify` checked every constant index against the table.
            macro_rules! kv {
                ($i:expr) => {{
                    let i = $i as usize;
                    debug_assert!(i < unsafe { (*fr!().proto).consts.len() });
                    unsafe { *k.add(i) }
                }};
            }
            macro_rules! refresh {
                () => {
                    // SAFETY: as above.
                    regs = unsafe { self.stack.as_mut_ptr().add(base!()) }
                };
            }
            // SAFETY for `r!`: `regs` is current (see above), and `Proto::verify` checked that no
            // instruction addresses a register at or past `maxstack`.
            macro_rules! r {
                ($x:expr) => {
                    *{
                        let i = $x as usize;
                        debug_assert!(i < maxstack!() && base!() + i < self.stack.len());
                        unsafe { &mut *regs.add(i) }
                    }
                };
            }
            macro_rules! rk {
                ($x:expr) => {{
                    let x: Rk = $x;
                    if x >= RK_CONST {
                        kv!(x - RK_CONST)
                    } else {
                        r!(x)
                    }
                }};
            }
            // `rk!` by reference: a fast path reads the tag and payload in place instead of
            // copying the value out.
            macro_rules! rkref {
                ($x:expr) => {{
                    let x: Rk = $x;
                    if x >= RK_CONST {
                        let i = (x - RK_CONST) as usize;
                        debug_assert!(i < unsafe { (*fr!().proto).consts.len() });
                        // SAFETY: as `kv!`.
                        unsafe { &*k.add(i) }
                    } else {
                        let i = x as usize;
                        debug_assert!(i < maxstack!() && base!() + i < self.stack.len());
                        // SAFETY: as `r!`.
                        unsafe { &*regs.add(i) }
                    }
                }};
            }
            macro_rules! rkreg {
                ($x:expr) => {{
                    let x: Rk = $x;
                    if x >= RK_CONST {
                        None
                    } else {
                        Some(x as usize)
                    }
                }};
            }
            macro_rules! env {
                () => {{
                    let fenv_gen = self.fenv_gen;
                    let f = fr_mut!();
                    if f.env_gen != fenv_gen {
                        f.env_gen = self.fenv_gen;
                        f.env = match self.heap.func(f.func) {
                            FuncObj::Lua(LuaFunc { env, .. }) => *env,
                            _ => unreachable!(),
                        };
                    }
                    f.env
                }};
            }
            macro_rules! save {
                () => {
                    fr_mut!().ip = ip
                };
            }
            /// `check_gc` at a safe point: a collection drops the host closures it freed, whose
            /// destructors may run Lua and move the stack, so the registers are refreshed after.
            macro_rules! gc {
                () => {
                    if self.heap.bytes() >= self.heap.trigger || self.gc_stress {
                        self.full_gc();
                        refresh!();
                    }
                };
            }
            /// A call that may run Lua: the pc saved first, the registers refreshed after.
            macro_rules! slow {
                ($e:expr) => {{
                    save!();
                    let v = $e;
                    refresh!();
                    v
                }};
            }
            macro_rules! arith {
                ($a:expr, $b:expr, $c:expr, $op:tt, $kind:expr) => {{
                    let (x, y) = (r!($b), r!($c));
                    if let (Some(xn), Some(yn)) = (x.as_num(), y.as_num()) {
                        // Canonical numbers in, a canonical number out.
                        r!($a) = Value::num_raw(crate::number::x87_op(xn $op yn, xn, yn));
                    } else {
                        let v = slow!(self.arith($kind, x, y, (Some($b as usize), Some($c as usize))))?;
                        r!($a) = v;
                    }
                }};
            }
            /// `K(b) op R(c)`, in source order for a metamethod and an error.
            macro_rules! arithkr {
                ($a:expr, $kx:expr, $c:expr, $op:tt, $kind:expr) => {{
                    let (x, y) = (kv!($kx), r!($c));
                    if let (Some(xn), Some(yn)) = (x.as_num(), y.as_num()) {
                        r!($a) = Value::num_raw(crate::number::x87_op(xn $op yn, xn, yn));
                    } else {
                        let v = slow!(self.arith($kind, x, y, (None, Some($c as usize))))?;
                        r!($a) = v;
                    }
                }};
            }
            /// The instructions run since `seg`, the one just fetched included, for the hook; one
            /// when the hook was set during this frame's run.
            macro_rules! ran {
                () => {
                    if seg.is_null() {
                        1
                    } else {
                        // SAFETY: `seg_code` is the code of the frame that set `seg`: this one,
                        // or one that returned since with nothing collected in between; a frame
                        // entered with no hook set clears `seg`.
                        ran_since(seg, ip, unsafe { &*seg_code })
                    }
                };
            }
            /// A run starts here, in the running frame's code.
            macro_rules! mark {
                () => {{
                    seg = ip;
                    seg_code = fr!().proto;
                }};
            }
            /// A jump forward: the run so far is counted when a hook is set, so a run holds only
            /// what ran ([`Proto::uncounted`] is found in it), and the hook waits for the next
            /// back edge, call or return.
            macro_rules! skip {
                ($off:expr) => {{
                    let off = $off as isize;
                    if SLOW && self.hooked {
                        let n = ran!();
                        // SAFETY: the compiler's jumps land inside the function.
                        ip = unsafe { ip.offset(off) };
                        mark!();
                        self.hook_add(n);
                    } else {
                        // SAFETY: as above.
                        ip = unsafe { ip.offset(off) };
                    }
                }};
            }
            macro_rules! jump {
                ($off:expr) => {{
                    let off = $off as isize;
                    if off < 0 && self.hooked {
                        if !SLOW {
                            // A hook was set: the slow loop goes on from the loop's head.
                            // SAFETY: as `skip!`.
                            ip = unsafe { ip.offset(off) };
                            save!();
                            return Ok(false);
                        }
                        // A loop's back edge: the place a loop is charged.
                        let n = ran!();
                        // SAFETY: `Proto::verify` checked that every jump lands inside the code.
                        ip = unsafe { ip.offset(off) };
                        mark!();
                        slow!(self.hook_count(n))?;
                    } else {
                        skip!(off);
                    }
                }};
            }
            /// 5.0's `OP_RETURN`: `n` values from `R(a)` to the caller's frame.
            macro_rules! ret {
                ($a:expr, $n:expr) => {{
                    let a = $a;
                    let n: usize = $n;
                    let fi = self.frames.len() - 1;
                    let base = base!();
                    let first = base + a as usize;
                    if !self.open.is_empty() {
                        self.close_upvals(base);
                    }
                    // The returning frame's two fields that matter, read in place; its slot sits
                    // just below `regs`.
                    let (nresults, boundary) = {
                        let me = self.frames.last().unwrap();
                        (me.nresults, me.boundary)
                    };
                    let func_idx = base - 1;
                    self.frames.truncate(fi);
                    if nresults == 1 && n >= 1 {
                        // One result, the common call in an expression, into the callee's slot
                        // just below this frame's registers.
                        // SAFETY: `base` is at least 1 for a Lua frame; `regs` is still the
                        // returning frame's, and `a` one of its registers (`r!` would check it
                        // against the caller's frame, now on top).
                        unsafe { *regs.sub(1) = *regs.add(a as usize) };
                    } else if nresults == 0 {
                        // None, a call statement.
                        self.top = func_idx;
                    } else {
                        self.poscall(func_idx, first, n, nresults);
                    }
                    if boundary {
                        if nresults == 1 && n >= 1 {
                            self.top = func_idx + 1;
                        }
                        // The frame's last run; a return into Lua charges it as it re-enters.
                        if SLOW && self.hooked {
                            let n = ran!();
                            self.hook_count(n)?;
                        }
                        return Ok(true);
                    }
                    // Back in the calling Lua frame: a fixed result count restores its top.
                    let caller = self.frames.last().unwrap();
                    if nresults >= 0 {
                        self.top = caller.base() + caller.maxstack as usize;
                    }
                    continue 'frame;
                }};
            }
            macro_rules! arithk {
                ($a:expr, $b:expr, $kx:expr, $op:tt, $kind:expr) => {{
                    let y = kv!($kx);
                    if let (Some(x), Some(yn)) = (r!($b).as_num(), y.as_num()) {
                        r!($a) = Value::num_raw(crate::number::x87_op(x $op yn, x, yn));
                    } else {
                        let x = r!($b);
                        let v = slow!(self.arith($kind, x, y, (Some($b as usize), None)))?;
                        r!($a) = v;
                    }
                }};
            }
            /// A string key missing from table `tr`, through its metatable: the common one level
            /// (`__index` a table holding the key) inline, a longer chain by `index_meta_fast`.
            /// `None` when the full `index` must run.
            macro_rules! meta_field {
                ($tr:expr, $key:expr) => {{
                    let (tr, key): (TableRef, Value) = ($tr, $key);
                    match self.heap.table(tr).meta {
                        None => Some(Value::Nil),
                        Some(m) => {
                            let idx = self.heap.table(m).get_str(Value::Str(self.names.index));
                            match idx.as_table() {
                                Some(t2) => {
                                    let v = self.heap.table(t2).get_str(key);
                                    if !v.is_nil() {
                                        Some(v)
                                    } else {
                                        self.index_meta_fast(tr, &key)
                                    }
                                }
                                None if idx.is_nil() => Some(Value::Nil),
                                None => None,
                            }
                        }
                    }
                }};
            }
            /// A comparison with one constant side.
            macro_rules! cmpk {
                ($cmp:expr, $cond:expr, $x:expr, $y:expr, $off:expr) => {{
                    let (x, y): (Value, Value) = ($x, $y);
                    let res = match ($cmp, x.as_num(), y.as_num()) {
                        (Cmp::Lt, Some(p), Some(q)) => p < q,
                        (Cmp::Le, Some(p), Some(q)) => p <= q,
                        // A constant is never a table or userdata, so `__eq` never applies.
                        (Cmp::Eq, _, _) => x.raw_eq(y),
                        (Cmp::Lt, _, _) => slow!(self.less_than(x, y))?,
                        (Cmp::Le, _, _) => slow!(self.less_equal(x, y))?,
                    };
                    if res == $cond {
                        jump!($off);
                    }
                }};
            }
            loop {
                debug_assert!(unsafe { (*fr!().proto).code.as_ptr_range().contains(&ip) });
                // SAFETY: `Proto::verify` checked that every function ends in a return and
                // every jump lands inside it.
                let op = unsafe { *ip };
                ip = unsafe { ip.add(1) };
                match op {
                    Op::Move(a, b) => r!(a) = r!(b),
                    Op::LoadK(a, kx) => r!(a) = kv!(kx),
                    Op::LoadNil(a, b) => {
                        for i in a..=b {
                            r!(i) = Value::Nil;
                        }
                    }
                    Op::LoadBool(a, b) => r!(a) = Value::Bool(b),
                    Op::GetUpval(a, u) => {
                        let v = match self.heap.upval(upval!(u)) {
                            // An open upvalue is a stack slot, below this frame's registers or
                            // among them: addressed from `regs`, which is current.
                            UpvalObj::Open(i) => {
                                debug_assert!((i as usize) < self.stack.len());
                                // SAFETY: an open upvalue's slot is inside the stack.
                                unsafe { *regs.sub(base!()).add(i as usize) }
                            }
                            UpvalObj::Closed(v) => v,
                        };
                        r!(a) = v;
                    }
                    Op::SetUpval(a, u) => {
                        let v = r!(a);
                        let uv = upval!(u);
                        self.upval_set(uv, v);
                        refresh!();
                    }
                    Op::GetGlobal(a, kx) => {
                        let env = env!();
                        let key = kv!(kx);
                        let tab = self.heap.table(env);
                        let v = tab.get(key);
                        if !v.is_nil() || tab.meta.is_none() {
                            r!(a) = v;
                        } else {
                            let v = slow!(self.index(Value::Table(env), key, None))?;
                            r!(a) = v;
                        }
                    }
                    Op::SetGlobal(a, kx) => {
                        let env = env!();
                        let v = r!(a);
                        let key = kv!(kx);
                        let tab = self.heap.table_mut(env);
                        if tab.meta.is_none() && tab.set(key, v).is_ok() {
                            continue;
                        }
                        slow!(self.setindex(Value::Table(env), key, v, None))?;
                    }
                    Op::GetTable(a, b, c) => {
                        if let Some(tr) = r!(b).as_table() {
                            let tab = self.heap.table(tr);
                            let v = tab.get_ref(rkref!(c));
                            if !v.is_nil() || tab.meta.is_none() {
                                r!(a) = v;
                                continue;
                            }
                            if let Some(v) = self.index_meta_fast(tr, rkref!(c)) {
                                r!(a) = v;
                                continue;
                            }
                        }
                        let (t, key) = (r!(b), rk!(c));
                        let v = slow!(self.index(t, key, Some(b as usize)))?;
                        r!(a) = v;
                    }
                    Op::GetField(a, b, kx) => {
                        let key = kv!(kx);
                        if let Some(tr) = r!(b).as_table() {
                            let tab = self.heap.table(tr);
                            let v = tab.get_str(key);
                            if !v.is_nil() || tab.meta.is_none() {
                                r!(a) = v;
                                continue;
                            }
                            if let Some(v) = meta_field!(tr, key) {
                                r!(a) = v;
                                continue;
                            }
                        }
                        let t = r!(b);
                        let v = slow!(self.index(t, key, Some(b as usize)))?;
                        r!(a) = v;
                    }
                    Op::SelfField(a, b, kx) => {
                        let key = kv!(kx);
                        let obj = r!(b);
                        r!(a + 1) = obj;
                        if let Some(tr) = obj.as_table() {
                            let tab = self.heap.table(tr);
                            let v = tab.get_str(key);
                            if !v.is_nil() || tab.meta.is_none() {
                                r!(a) = v;
                                continue;
                            }
                            if let Some(v) = meta_field!(tr, key) {
                                r!(a) = v;
                                continue;
                            }
                        }
                        let v = slow!(self.index(obj, key, Some(b as usize)))?;
                        r!(a) = v;
                    }
                    Op::SetField(a, kx, c) => {
                        let key = kv!(kx);
                        let v = rk!(c);
                        if let Some(tr) = r!(a).as_table() {
                            let tab = self.heap.table_mut(tr);
                            if tab.meta.is_none() {
                                if tab.set(key, v).is_ok() {
                                    continue;
                                }
                            } else if let Some(s) = tab.find(key) {
                                // As `SetTable`: an existing value is overwritten raw.
                                if !tab.slot(s).is_nil() {
                                    *tab.slot_mut(s) = v;
                                    continue;
                                }
                            }
                        }
                        let t = r!(a);
                        slow!(self.setindex(t, key, v, Some(a as usize)))?;
                    }
                    Op::SetTable(a, b, c) => {
                        let key = rk!(b);
                        let v = rk!(c);
                        if let Some(tr) = r!(a).as_table() {
                            let tab = self.heap.table_mut(tr);
                            if tab.meta.is_none() {
                                if tab.set(key, v).is_ok() {
                                    continue;
                                }
                            } else if let Some(s) = tab.find(key) {
                                // An existing value is overwritten raw: `__newindex` is only
                                // consulted for a nil one.
                                if !tab.slot(s).is_nil() {
                                    *tab.slot_mut(s) = v;
                                    continue;
                                }
                            }
                        }
                        let t = r!(a);
                        slow!(self.setindex(t, key, v, Some(a as usize)))?;
                    }
                    Op::NewTable(a, b, c) => {
                        // `luaH_new`'s `setnodevector` (`0x6fa549`) raises past `MAXBITS`.
                        if let Err(e) = crate::table::check_lsize(c) {
                            save!();
                            return Err(self.rt_error(e.message()));
                        }
                        let t = self.heap.new_table(b as usize, c);
                        r!(a) = Value::Table(t);
                        save!();
                        self.top = base!() + maxstack!();
                        gc!();
                    }
                    Op::SelfOp(a, b, c) => {
                        let obj = r!(b);
                        r!(a + 1) = obj;
                        if let Some(tr) = r!(b).as_table() {
                            let tab = self.heap.table(tr);
                            let v = tab.get_ref(rkref!(c));
                            if !v.is_nil() || tab.meta.is_none() {
                                r!(a) = v;
                                continue;
                            }
                            if let Some(v) = self.index_meta_fast(tr, rkref!(c)) {
                                r!(a) = v;
                                continue;
                            }
                        }
                        let key = rk!(c);
                        let v = slow!(self.index(obj, key, Some(b as usize)))?;
                        r!(a) = v;
                    }
                    Op::Add(a, b, c) => arith!(a, b, c, +, Arith::Add),
                    Op::Sub(a, b, c) => arith!(a, b, c, -, Arith::Sub),
                    Op::Mul(a, b, c) => arith!(a, b, c, *, Arith::Mul),
                    Op::Div(a, b, c) => arith!(a, b, c, /, Arith::Div),
                    Op::AddK(a, b, kx) => arithk!(a, b, kx, +, Arith::Add),
                    Op::SubK(a, b, kx) => arithk!(a, b, kx, -, Arith::Sub),
                    Op::MulK(a, b, kx) => arithk!(a, b, kx, *, Arith::Mul),
                    Op::DivK(a, b, kx) => arithk!(a, b, kx, /, Arith::Div),
                    Op::AddKR(a, kx, c) => arithkr!(a, kx, c, +, Arith::Add),
                    Op::SubKR(a, kx, c) => arithkr!(a, kx, c, -, Arith::Sub),
                    Op::MulKR(a, kx, c) => arithkr!(a, kx, c, *, Arith::Mul),
                    Op::DivKR(a, kx, c) => arithkr!(a, kx, c, /, Arith::Div),
                    Op::JmpCmpK(cj, b, kx, off) => cmpk!(cj.cmp(), cj.cond(), r!(b), kv!(kx), off),
                    Op::JmpCmpKR(cj, kx, c, off) => cmpk!(cj.cmp(), cj.cond(), kv!(kx), r!(c), off),
                    Op::Pow(a, b, c) => {
                        // The client's `OP_POW` (`0x6f8e47`): always `Arith`, which calls
                        // the global `__pow` for numbers.
                        let (x, y) = (rk!(b), rk!(c));
                        let v = slow!(self.arith(Arith::Pow, x, y, (rkreg!(b), rkreg!(c))))?;
                        r!(a) = v;
                    }
                    Op::Unm(a, b) => {
                        let x = r!(b);
                        match self.tonumber(x) {
                            Some(n) => r!(a) = Value::Num(-n),
                            None => {
                                // 5.1.5's `OP_UNM`: `Arith(L, ra, rb, rb, TM_UNM)`, the operand
                                // passed twice.
                                let unm = self.names.unm;
                                match slow!(self.bin_tm(x, x, unm))? {
                                    Some(v) => r!(a) = v,
                                    None => {
                                        return Err(self.type_error(
                                            x,
                                            Some(b as usize),
                                            "perform arithmetic on",
                                        ))
                                    }
                                }
                            }
                        }
                    }
                    Op::Not(a, b) => r!(a) = Value::Bool(!r!(b).truthy()),
                    Op::Concat(a, b, c) => {
                        self.top = base!() + maxstack!();
                        let b0 = base!();
                        slow!(self.concat(b0 + b as usize, b0 + c as usize))?;
                        r!(a) = r!(b);
                        gc!();
                    }
                    Op::Jmp(off) => jump!(off),
                    Op::JmpCmp(cj, b, c, off) => {
                        let (cmp, cond) = (cj.cmp(), cj.cond());
                        let res = match (cmp, r!(b).as_num(), r!(c).as_num()) {
                            (Cmp::Lt, Some(p), Some(q)) => p < q,
                            (Cmp::Le, Some(p), Some(q)) => p <= q,
                            (Cmp::Eq, Some(p), Some(q)) => p == q,
                            _ => {
                                let (x, y) = (r!(b), r!(c));
                                match cmp {
                                    Cmp::Eq => x.raw_eq(y) || slow!(self.equals(x, y))?,
                                    Cmp::Lt => slow!(self.less_than(x, y))?,
                                    Cmp::Le => slow!(self.less_equal(x, y))?,
                                }
                            }
                        };
                        if res == cond {
                            jump!(off);
                        }
                    }
                    Op::JmpIf(a, cond, off) => {
                        if r!(a).truthy() == cond {
                            jump!(off);
                        }
                    }
                    Op::TestSet(a, b, cond, off) => {
                        let v = r!(b);
                        if v.truthy() == cond {
                            r!(a) = v;
                            // A value's test jumps forward, never a back edge.
                            skip!(off);
                        }
                    }
                    Op::Call(a, b, c) => {
                        let func_idx = base!() + a as usize;
                        let nargs = if b == 0 {
                            self.top - func_idx - 1
                        } else {
                            b as usize - 1
                        };
                        save!();
                        if self.hooked {
                            if !SLOW {
                                // A hook was set: the slow loop makes this call.
                                fr_mut!().ip = unsafe { ip.sub(1) };
                                return Ok(false);
                            }
                            let n = ran!();
                            mark!();
                            slow!(self.hook_count(n))?;
                        }
                        // A Lua function without `...`: the frame is pushed here, as C Lua's
                        // `luaD_precall` does for it, without the general path.
                        if let Some(f) = r!(a).as_func() {
                            if let FuncObj::Lua(LuaFunc {
                                proto: p,
                                upvals: fup,
                                env: fenv,
                                shape,
                            }) = self.heap.func(f)
                            {
                                // `fi + 1` frames are live: the depth without reading the
                                // frame list.
                                if !shape.precall && self.frames.len() < self.ci_limit {
                                    let np = shape.nparams as usize;
                                    let consts = shape.consts;
                                    let nbase = func_idx + 1;
                                    let ftop = nbase + shape.maxstack as usize;
                                    // `luaD_precall`'s check (`0x6f60c1`) and the frame's own
                                    // registers, in one compare.
                                    let need50 =
                                        nbase + nargs + shape.c_maxstack as usize + EXTRA_STACK + 2;
                                    let frame = Frame {
                                        func: f,
                                        base: nbase as u32,
                                        ip: shape.code,
                                        nresults: c as i32 - 1,
                                        consts: shape.consts,
                                        maxstack: shape.maxstack,
                                        source: shape.source,
                                        proto: Rc::as_ptr(p),
                                        upvals: fup.as_ptr(),
                                        env: *fenv,
                                        env_gen: self.fenv_gen,
                                        tailcalls: 0,
                                        boundary: false,
                                    };
                                    if self.stack.len() < need50.max(ftop + 1) {
                                        self.checkstack(nbase + nargs, shape.c_maxstack as usize);
                                        self.ensure(ftop + 1);
                                    }
                                    if SLOW && nbase < self.clear_below {
                                        // As `precall`: every register past the arguments.
                                        // SAFETY: the closure being called holds its code.
                                        let cmax = unsafe { &*frame.proto }.c_maxstack as usize;
                                        for v in
                                            &mut self.stack[nbase + nargs.min(np)..nbase + cmax]
                                        {
                                            *v = Value::Nil;
                                        }
                                    } else if nargs < np {
                                        for v in &mut self.stack[nbase + nargs..nbase + np] {
                                            *v = Value::Nil;
                                        }
                                    }
                                    self.top = ftop;
                                    self.frames.push(frame);
                                    // Enter the callee from what is in hand, not from the
                                    // frame just written.
                                    ip = frame.ip;
                                    k = consts;
                                    self.heap.cur_source = frame.source;
                                    if SLOW {
                                        if self.hooked {
                                            seg = ip;
                                            seg_code = frame.proto;
                                            self.prof_switch();
                                        } else {
                                            seg = std::ptr::null();
                                        }
                                    }
                                    refresh!();
                                    continue;
                                }
                            } else if self.frames.len() < self.ci_limit {
                                // A native: `precall`'s native half, without its dispatch.
                                self.call_native(f, func_idx, nargs, c as i32 - 1)?;
                                refresh!();
                                if c != 0 {
                                    self.top = base!() + maxstack!();
                                }
                                gc!();
                                continue;
                            }
                        }
                        match self.precall(func_idx, nargs, c as i32 - 1)? {
                            // A function that needs its frames cleared runs in the slow loop.
                            Called::Lua if !SLOW && self.clear_below != 0 => return Ok(false),
                            Called::Lua => continue 'frame,
                            Called::Done => {
                                refresh!();
                                if c != 0 {
                                    self.top = base!() + maxstack!();
                                }
                                gc!();
                            }
                        }
                    }
                    Op::TailCall(a, b) => {
                        let func_idx = base!() + a as usize;
                        let mut nargs = if b == 0 {
                            self.top - func_idx - 1
                        } else {
                            b as usize - 1
                        };
                        save!();
                        // A call like any other for the budget: a tail-recursive loop passes
                        // no back edge and no `Call`.
                        if self.hooked {
                            if !SLOW {
                                // A hook was set: the slow loop makes this call.
                                fr_mut!().ip = unsafe { ip.sub(1) };
                                return Ok(false);
                            }
                            let n = ran!();
                            mark!();
                            slow!(self.hook_count(n))?;
                        }
                        // 5.1's `OP_TAILCALL` resolves `__call` first, so a callable table's
                        // Lua `__call` replaces this frame too.
                        if r!(a).as_func().is_none() {
                            nargs = slow!(self.tryfunc(func_idx, nargs))?;
                        }
                        // A Lua callee replaces this frame; a native runs and its results
                        // return through the `Return` that follows.
                        let is_lua = matches!((r!(a)).unpack(),
                            V::Func(f) if matches!(self.heap.func(f), FuncObj::Lua(_))
                        );
                        if !is_lua {
                            match self.precall(func_idx, nargs, -1)? {
                                Called::Lua if !SLOW && self.clear_below != 0 => return Ok(false),
                                Called::Lua => continue 'frame,
                                Called::Done => {
                                    refresh!();
                                    continue;
                                }
                            }
                        }
                        // `luaD_precall`'s `inc_ci` counts the callee while this frame is still
                        // there, as C replaces it only after.
                        if self.frames.len() >= self.ci_limit {
                            // Saved above; on success the stack is as it was.
                            self.grow_ci()?;
                        }
                        let b0 = base!();
                        self.close_upvals(b0);
                        let dest = fr!().func_idx();
                        if SLOW && dest < self.clear_below {
                            // `OP_TAILCALL`'s order, where code may read what it leaves: the
                            // callee's frame is made where its function sits, registers cleared,
                            // then moved down whole over this one, and the place it was made
                            // keeps its copy.
                            let nres = fr!().nresults;
                            self.precall_clearing(func_idx, nargs, nres, true)?;
                            let mut new = self.frames.pop().unwrap();
                            let me = self.frames.pop().unwrap();
                            // C's frame, to `ci->top`; benilla's spare slots above stay as they
                            // were.
                            // SAFETY: the closure just called holds its code (`Frame::proto`).
                            let cmax = unsafe { &*new.proto }.c_maxstack as usize;
                            self.stack.copy_within(func_idx..func_idx + 1 + cmax, dest);
                            self.top -= func_idx - dest;
                            new.base = dest as u32 + 1;
                            // 5.1's `ci->tailcalls++` is a C int; saturated here so 2^32 tail
                            // calls cannot overflow, a count no traceback reaches.
                            new.tailcalls = me.tailcalls.saturating_add(1);
                            new.boundary = me.boundary;
                            self.frames.push(new);
                            continue 'frame;
                        }
                        // The client's `OP_TAILCALL` makes the call where the function sits
                        // (`0x6f922c`) and moves the frame down after (`0x6f9761`..`0x6f97ef`), so
                        // its stack check is made there.
                        if let V::Func(f) = r!(a).unpack() {
                            if let FuncObj::Lua(LuaFunc { proto, .. }) = self.heap.func(f) {
                                let (top, n) = (
                                    precall_top50(func_idx + 1, nargs, proto),
                                    proto.c_maxstack as usize,
                                );
                                self.checkstack(top, n);
                            }
                        }
                        let me = self.frames.pop().unwrap();
                        self.stack.copy_within(func_idx..func_idx + nargs + 1, dest);
                        self.top = dest + nargs + 1;
                        match self.precall(dest, nargs, me.nresults) {
                            Ok(Called::Lua) => {
                                let f = self.frames.last_mut().unwrap();
                                // 5.1's `ci->tailcalls++` is a C int; saturated here so 2^32
                                // tail calls cannot overflow, a count no traceback reaches.
                                f.tailcalls = me.tailcalls.saturating_add(1);
                                f.boundary = me.boundary;
                                if !SLOW && self.clear_below != 0 {
                                    return Ok(false);
                                }
                                continue 'frame;
                            }
                            Ok(Called::Done) => unreachable!("a Lua callee"),
                            Err(e) => {
                                self.frames.push(me);
                                return Err(e);
                            }
                        }
                    }
                    Op::Return(a, b) => {
                        let n = if b == 0 {
                            self.top - (base!() + a as usize)
                        } else {
                            b as usize - 1
                        };
                        ret!(a, n);
                    }
                    Op::Return1(a) => ret!(a, 1),
                    // The client's `OP_FORLOOP` (`0x6f9262`): the loop's own variable is the
                    // counter, so the body may move it; each pass checks it is a number, makes
                    // the limit and step numbers in place, steps it and, while within the
                    // limit, writes it back and jumps. `ForLoopUp` knows the step is a positive
                    // constant.
                    Op::ForLoopUp(a, off) => {
                        let (idx, limit, _) = match (r!(a).as_num(), r!(a + 1).as_num()) {
                            (Some(i), Some(l)) => (i, l, 0.0),
                            _ => slow!(self.for_controls(base!() + a as usize))?,
                        };
                        let idx = idx + r!(a + 2).as_num().unwrap_or(f64::NAN);
                        if idx <= limit {
                            // Canonical numbers in, a canonical number out.
                            r!(a) = Value::num_raw(idx);
                            jump!(off);
                        }
                    }
                    Op::ForLoop(a, off) => {
                        let (idx, limit, step) =
                            match (r!(a).as_num(), r!(a + 1).as_num(), r!(a + 2).as_num()) {
                                (Some(i), Some(l), Some(s)) => (i, l, s),
                                _ => slow!(self.for_controls(base!() + a as usize))?,
                            };
                        let idx = idx + step;
                        if if step > 0.0 {
                            idx <= limit
                        } else {
                            limit <= idx
                        } {
                            // Canonical numbers in, a canonical number out.
                            r!(a) = Value::num_raw(idx);
                            jump!(off);
                        }
                    }
                    // The client's `TFORPREP` (`0x6f94a2`): a table generator becomes the raw
                    // global `next` over it once, at loop entry. The C build's patch swaps at
                    // every `TFORLOOP` instead, so where the global `next` is a callable table
                    // it raises `invalid key to 'next'` after one pass; the single swap here
                    // moves benilla-lua toward the reference.
                    Op::TForPrep(a, off) => {
                        if let V::Table(_) = (r!(a)).unpack() {
                            r!(a + 1) = r!(a);
                            let g = self.globals;
                            r!(a) = self.heap.table(g).get(Value::Str(self.names.next));
                        }
                        jump!(off);
                    }
                    // The client's `OP_TFORLOOP` (`0x6f933b`): the generator, the state and
                    // the first variable, the control, copied above the variables
                    // (`0x6f9344`..`0x6f93e0`), the call there for `nv` results (`0x6f940f`),
                    // the results moved down into the variables (`0x6f9445`..`0x6f9464`), and
                    // the loop ends when the first is nil (`0x6f9490`).
                    Op::TForLoop(a, nvar) => {
                        let nv = nvar as usize;
                        let ctl = a as usize + 2;
                        let cb = ctl + nv;
                        // The built-in `next` over a table, stepped without a call: what the
                        // call would do, minus the frame. An invalid key takes the call, which
                        // raises from inside `next` as before.
                        if r!(a) == self.builtin_next && !self.builtin_next.is_nil() {
                            let state = r!(a + 1);
                            if let V::Table(t) = state.unpack() {
                                if let Ok(step) = self.heap.table(t).next(r!(ctl)) {
                                    // The registers from `cb` up as the call leaves them: the
                                    // state and control copied to `cb + 1` and `cb + 2`, then
                                    // `lua_next` writes the key over the control and the value
                                    // above it (or `next` one nil there), and `nv` results move
                                    // down to `cb`, nil past those given; those then copy into
                                    // the variables. A body can read past its variables
                                    // (`Proto::clears`).
                                    let (k, v, n) = match step {
                                        Some((k, v)) => (k, v, 2),
                                        None => (Value::Nil, Value::Nil, 1),
                                    };
                                    let left = [k, state, k, v];
                                    let res = [k, v];
                                    for i in 0..nv.max(if n == 2 { 4 } else { 3 }) {
                                        let x = if i < nv {
                                            if i < n {
                                                res[i]
                                            } else {
                                                Value::Nil
                                            }
                                        } else {
                                            left[i]
                                        };
                                        if cb + i < maxstack!() {
                                            r!(cb + i) = x;
                                        }
                                        if i < nv {
                                            r!(ctl + i) = x;
                                        }
                                    }
                                    if k.is_nil() {
                                        skip!(1);
                                    } else {
                                        // SAFETY: a `TForLoop` is always followed by its `Jmp`.
                                        let Op::Jmp(back) = (unsafe { *ip }) else {
                                            unreachable!()
                                        };
                                        jump!(1 + back);
                                    }
                                    continue;
                                }
                            }
                        }
                        r!(cb + 2) = r!(ctl);
                        r!(cb + 1) = r!(a + 1);
                        r!(cb) = r!(a);
                        self.top = base!() + cb + 3;
                        let call_at = base!() + cb;
                        slow!(self.call_at(call_at, 2, nv as i32))?;
                        self.top = base!() + maxstack!();
                        for i in (0..nv).rev() {
                            r!(ctl + i) = r!(cb + i);
                        }
                        if r!(ctl).is_nil() {
                            skip!(1);
                        } else {
                            // SAFETY: a `TForLoop` is always followed by its `Jmp`.
                            let Op::Jmp(back) = (unsafe { *ip }) else {
                                unreachable!()
                            };
                            jump!(1 + back);
                        }
                    }
                    // 5.0's `OP_SETLIST` (`0x6f955a`): the items stored from the last down by
                    // `luaH_setnum`, into the array part as `OP_NEWTABLE` sized it or else as
                    // new keys, which may rehash.
                    Op::SetList(a, first, n) => {
                        let V::Table(t) = (r!(a)).unpack() else {
                            unreachable!()
                        };
                        for i in (1..=n as usize).rev() {
                            let v = r!(a as usize + i);
                            if let Err(e) = self
                                .heap
                                .table_mut(t)
                                .set_int((first as usize + i) as i32, v)
                            {
                                save!();
                                return Err(self.rt_error(e.message()));
                            }
                        }
                    }
                    Op::SetListTop(a, first) => {
                        let V::Table(t) = (r!(a)).unpack() else {
                            unreachable!()
                        };
                        let n = self.top - (base!() + a as usize) - 1;
                        for i in (1..=n).rev() {
                            let v = self.stack[base!() + a as usize + i];
                            if let Err(e) = self
                                .heap
                                .table_mut(t)
                                .set_int((first as usize + i) as i32, v)
                            {
                                save!();
                                return Err(self.rt_error(e.message()));
                            }
                        }
                        self.top = base!() + maxstack!();
                    }
                    Op::Close(a) => {
                        let b0 = base!();
                        self.close_upvals(b0 + a as usize);
                        refresh!();
                    }
                    Op::Closure(a, px) => {
                        // SAFETY: as `Frame::proto`.
                        let p = unsafe { &*fr!().proto }.protos[px as usize].clone();
                        let env = env!();
                        let b0 = base!();
                        let mut ups = Vec::with_capacity(p.upvals.len());
                        for &(in_stack, idx) in &p.upvals {
                            ups.push(if in_stack {
                                self.find_upval(b0 + idx as usize)
                            } else {
                                upval!(idx)
                            });
                        }
                        let f = self.heap.new_func(FuncObj::Lua(LuaFunc::new(
                            p,
                            crate::heap::Upvals::new(ups),
                            env,
                        )));
                        r!(a) = Value::Func(f);
                        save!();
                        self.top = base!() + maxstack!();
                        gc!();
                    }
                }
            }
        }
    }
}

/// The instructions 5.1 runs from `seg` to `ip` in `p`'s code, for the budget hook
/// ([`Proto::uncounted`] left out); 1 for a run that started before the hook was set, whose
/// start the VM did not keep.
#[cold]
fn ran_since(seg: *const Op, ip: *const Op, p: &Proto) -> u64 {
    let code = p.code.as_ptr_range();
    let (s, i) = (seg as usize, ip as usize);
    if s < code.start as usize || s > i || i > code.end as usize {
        return 1;
    }
    let size = std::mem::size_of::<Op>();
    let (s, i) = (
        (s - code.start as usize) / size,
        (i - code.start as usize) / size,
    );
    let free = if p.uncounted.is_empty() {
        0
    } else {
        let at = |pc: usize| p.uncounted.partition_point(|&u| (u as usize) < pc);
        at(i) - at(s)
    };
    (i - s - free) as u64
}

/// `getobjname` (`0x6fc4c0`) over a prototype: a local's name, else what a symbolic run from the
/// start says last wrote `reg` before `pc`. Names are C strings there, so a constant's stops at a NUL.
fn objname_in(p: &Proto, heap: &Heap, pc: usize, reg: usize) -> Option<(&'static str, Vec<u8>)> {
    if let Some(n) = p.local_name(reg as Reg, pc) {
        return Some(("local", n.to_vec()));
    }
    let last = symbexec(p, pc, reg)?;
    let cstr = |s: &[u8]| crate::lex::cstr(s).to_vec();
    let kname = |c: Rk| -> Vec<u8> {
        if c >= RK_CONST {
            if let V::Str(s) = p.consts[(c - RK_CONST) as usize].unpack() {
                return cstr(heap.str(s));
            }
        }
        b"?".to_vec()
    };
    match p.code[last] {
        Op::GetGlobal(_, kx) => match p.consts[kx as usize].unpack() {
            V::Str(s) => Some(("global", cstr(heap.str(s)))),
            _ => None,
        },
        Op::Move(a, b) if (b as usize) < (a as usize) => objname_in(p, heap, pc, b as usize),
        // The client's `getobjname` has no `OP_GETUPVAL` arm (`0x6fc5e0`): an upvalue is unnamed.
        Op::GetTable(_, _, c) => Some(("field", kname(c))),
        Op::GetField(_, _, kx) => Some(("field", kname(kx + RK_CONST))),
        Op::SelfOp(_, _, c) => Some(("method", kname(c))),
        Op::SelfField(_, _, kx) => Some(("method", kname(kx + RK_CONST))),
        _ => None,
    }
}

/// 5.1's `symbexec`: the instruction that last wrote `reg` before `lastpc`, running the code
/// from the start and taking each forward jump that lands at or before `lastpc`. As in 5.1, a
/// test counts as writing the register it tests, and a fused test takes its jump.
fn symbexec(p: &Proto, lastpc: usize, reg: usize) -> Option<usize> {
    let mut last = None;
    let mut pc = 0;
    while pc < lastpc {
        let op = p.code[pc];
        let writes = |a: Reg| a as usize == reg;
        let hit = match op {
            Op::Move(a, _)
            | Op::LoadK(a, _)
            | Op::LoadBool(a, _)
            | Op::GetUpval(a, _)
            | Op::GetGlobal(a, _)
            | Op::GetTable(a, ..)
            | Op::GetField(a, ..)
            | Op::NewTable(a, ..)
            | Op::Add(a, ..)
            | Op::Sub(a, ..)
            | Op::Mul(a, ..)
            | Op::Div(a, ..)
            | Op::Pow(a, ..)
            | Op::AddK(a, ..)
            | Op::SubK(a, ..)
            | Op::MulK(a, ..)
            | Op::DivK(a, ..)
            | Op::AddKR(a, ..)
            | Op::SubKR(a, ..)
            | Op::MulKR(a, ..)
            | Op::DivKR(a, ..)
            | Op::Unm(a, _)
            | Op::Not(a, _)
            | Op::Concat(a, ..)
            | Op::JmpIf(a, ..)
            | Op::TestSet(a, ..)
            | Op::ForLoop(a, _)
            | Op::ForLoopUp(a, _)
            | Op::Closure(a, _) => writes(a),
            Op::LoadNil(a, b) => (a as usize..=b as usize).contains(&reg),
            Op::SelfOp(a, ..) | Op::SelfField(a, ..) => writes(a) || reg == a as usize + 1,
            Op::Call(a, ..) | Op::TailCall(a, ..) => reg >= a as usize,
            // The client's rule writes every register from `a` up (`0x6fc22c`).
            Op::TForLoop(a, _) => reg >= a as usize,
            _ => false,
        };
        if hit {
            last = Some(pc);
        }
        let jump = match op {
            Op::Jmp(off)
            | Op::TForPrep(_, off)
            | Op::ForLoop(_, off)
            | Op::ForLoopUp(_, off)
            | Op::JmpIf(_, _, off)
            | Op::TestSet(_, _, _, off)
            | Op::JmpCmp(_, _, _, off)
            | Op::JmpCmpK(_, _, _, off)
            | Op::JmpCmpKR(_, _, _, off) => Some(off),
            _ => None,
        };
        if let Some(off) = jump {
            let dest = (pc as i32 + 1 + off) as usize;
            if pc < dest && dest <= lastpc {
                pc = dest;
                continue;
            }
        }
        pc += 1;
    }
    last
}

/// The top `luaD_precall` checks the stack from for a Lua function whose first register is
/// `base` (`0x6f60ab`..`0x6f60c7`): above its arguments, or above `arg`, which `adjust_varargs`
/// leaves past the parameters.
fn precall_top50(base: usize, nargs: usize, p: &Proto) -> usize {
    if p.vararg {
        base + p.nparams as usize + 1
    } else {
        base + nargs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deep recursion grows the stack and the call records by 5.0's rules; each collection
    /// after it halves both once (`0x6f7936`, `0x6f795e`), and the count drops by what they shed.
    #[test]
    fn the_stack_and_the_call_records_halve_once_a_collection() {
        let mut vm = crate::new_state();
        vm.full_gc();
        assert_eq!(vm.stack.len(), BASIC_STACK_SIZE);
        assert_eq!(vm.ci_size, BASIC_CI_SIZE);
        vm.exec(
            b"local function f(n) if n > 0 then return 1 + f(n - 1) end return 0 end f(300)",
            b"=t",
        )
        .unwrap();
        // 45 doubled: the stack grows as `luaD_growstack` doubles it (`0x6f5f67`).
        let (s0, k0) = (vm.stack.len(), vm.ci_size);
        assert_eq!(s0 % BASIC_STACK_SIZE, 0);
        assert!(
            (s0 / BASIC_STACK_SIZE).is_power_of_two() && s0 >= 4 * 300 / 2,
            "{s0}"
        );
        assert_eq!(k0, 512, "301 records: 8 doubled six times");
        vm.full_gc();
        assert_eq!(
            (vm.stack.len(), vm.ci_size),
            (s0 / 2, k0 / 2),
            "halved once"
        );
        let mut prev = (vm.stack.len(), vm.ci_size, vm.heap.bytes());
        loop {
            vm.full_gc();
            let now = (vm.stack.len(), vm.ci_size, vm.heap.bytes());
            assert_eq!(
                prev.2 - now.2,
                STACK_SLOT_BYTES * (prev.0 - now.0) + CI_BYTES * (prev.1 - now.1),
                "the count sheds what the stack and the records do"
            );
            if now == prev {
                break;
            }
            assert!(now.0 == prev.0 / 2 || now.0 == prev.0);
            assert!(now.1 == prev.1 / 2 || now.1 == prev.1);
            prev = now;
        }
        // Never back to the first sizes: a stack halves only above 90 slots, the records above 16.
        assert_eq!((vm.stack.len(), vm.ci_size), (2 * BASIC_STACK_SIZE, 16));
    }

    /// A tail call checks the stack where the called function sits, before its frame moves down
    /// (`0x6f922c`, then `0x6f9761`..`0x6f97ef`). `f`'s frame starts at slot 4, under the main
    /// chunk's three registers; with `n` locals it calls `g` from slot `4 + n`, its argument
    /// above, and `g` reserves 11 registers: the check from `6 + n` asks `6 + n + 11 + 6 < 45`,
    /// which 21 locals meet and 22 do not, where from `f`'s own slot it would ask only `5 + 17`.
    #[test]
    fn a_tail_call_checks_the_stack_where_the_call_is_made() {
        for (n, size) in [(21, 45), (22, 90)] {
            let mut vm = crate::new_state();
            let locals: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
            let src = format!(
                "local function g(x) local p, q, r, s, t, u, v, w, y, z = 1 return x end \
                 local function f() local {} = 1 return g(1) end f()",
                locals.join(", ")
            );
            vm.exec(src.as_bytes(), b"=t").unwrap();
            assert_eq!(vm.stack.len(), size, "{n} locals");
        }
    }

    /// The client's `adjust_varargs` makes the `arg` table without checking the collector
    /// (`0x6f6200`..`0x6f6366`): a call to a vararg function whose body allocates nothing never
    /// collects, with the count at the threshold.
    #[test]
    fn a_vararg_call_does_not_collect() {
        let mut vm = crate::new_state();
        vm.exec(b"function V(...) return 1 end", b"=t").unwrap();
        let v = vm.get_global("V");
        let count = vm.heap.bytes();
        vm.heap.set_threshold(count);
        let n = vm.heap.collections;
        vm.call_value(v, &[Value::Num(1.0), Value::Num(2.0)], 0)
            .unwrap();
        assert_eq!(vm.heap.collections, n);
        assert!(vm.heap.bytes() > count, "`arg` was made");
    }

    /// A collection the heap asks for (string ids running short) stays asked for through a new
    /// threshold: `collectgarbage` with the most there is still collects at once.
    #[test]
    fn a_new_threshold_keeps_a_collection_the_heap_asked_for() {
        let mut vm = crate::new_state();
        vm.full_gc();
        let used = vm.heap.counts()[0] as u32;
        vm.heap.limit_string_ids(2 * used + 8);
        let n = vm.heap.collections;
        let mut i = 0;
        while vm.heap.trigger != 0 {
            vm.heap.intern(format!("s{i}").as_bytes());
            i += 1;
        }
        assert_eq!(vm.heap.collections, n, "asked for, not run");
        vm.set_gc_threshold(0x3f_ffff);
        assert_eq!(vm.heap.collections, n + 1);
        assert_eq!(vm.heap.threshold(), vm.heap.trigger, "the request is met");
    }

    /// A bare state has nothing of 16 bytes or less, so its smallest pool has taken no chunk and
    /// has no free space: a collection leaves the count and a quarter of it (`0x6fae6f`).
    #[test]
    fn a_collection_with_a_pool_unused_leaves_a_quarter_more() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.heap.threshold(),
            4 * vm.heap.bytes(),
            "`f_luaopen` (`0x6f6ec8`)"
        );
        vm.full_gc();
        let count = vm.heap.bytes();
        assert_eq!(vm.heap.threshold(), count + count / 4);
    }
}
