//! The heap: every collectable in a typed arena, addressed by index. Cloning a `Heap` is a
//! snapshot of every object; there is no pointer anywhere to fix up.

use std::rc::Rc;

use crate::pools::{Block, Pools, Tally};
use crate::proto::Proto;
use crate::table::{Account, Growth, Table};
use crate::value::{FuncRef, StrRef, TableRef, UserRef, Value, STR_HASH_MASK, STR_ID_BITS, V};

/// A native function. Its arguments are the `nargs` values below the top of the stack; it pushes
/// its results and answers how many.
pub type NativeFn = fn(&mut crate::vm::Vm, usize) -> Result<usize, crate::vm::LuaError>;

/// A native function the host built at run time, with state of its own: the same calling
/// convention as [`NativeFn`].
pub type HostFn = Rc<dyn Fn(&mut crate::vm::Vm, usize) -> Result<usize, crate::vm::LuaError>>;

/// The longest string kept inside its object; a longer one is boxed. Most UI strings fit, and
/// an inline one costs no allocation and no second pointer to compare.
const INLINE: usize = 22;

#[derive(Clone, Debug)]
enum StrData {
    Inline(u8, [u8; INLINE]),
    Boxed(Box<[u8]>),
}

#[derive(Clone, Debug)]
pub struct StrObj {
    data: StrData,
    pub hash: u32,
}

impl StrObj {
    /// A string object holding `s`, or `None` when its bytes cannot be allocated.
    fn new(s: &[u8], hash: u32) -> Option<StrObj> {
        let data = if s.len() <= INLINE {
            let mut buf = [0; INLINE];
            buf[..s.len()].copy_from_slice(s);
            StrData::Inline(s.len() as u8, buf)
        } else {
            let mut b = Vec::new();
            b.try_reserve_exact(s.len()).ok()?;
            b.extend_from_slice(s);
            StrData::Boxed(b.into_boxed_slice())
        };
        Some(StrObj { data, hash })
    }

    #[inline]
    pub fn bytes(&self) -> &[u8] {
        match &self.data {
            StrData::Inline(n, buf) => &buf[..*n as usize],
            StrData::Boxed(b) => b,
        }
    }
}

/// An empty intern slot. No entry equals it: string ids stop below `(1 << STR_ID_BITS) - 1`.
const STRT_EMPTY: u32 = u32::MAX;
const STR_ID_MASK: u32 = (1 << STR_ID_BITS) - 1;

/// An intern slot, four bytes so a large table stays in cache: string `id` in the low
/// `STR_ID_BITS`, eight bits of its hash above, so a probe passes most other strings without
/// touching them.
#[inline]
fn strt_entry(hash: u32, id: u32) -> u32 {
    id | strt_tag(hash)
}

#[inline]
fn strt_tag(hash: u32) -> u32 {
    (hash & 0xff) << STR_ID_BITS
}

/// Where a probe for `hash` starts in a table of `mask + 1` slots: the hash mixed, so 5.0's
/// weak low bits do not cluster a linear probe.
#[inline]
fn strt_home(hash: u32, mask: usize) -> usize {
    (hash.wrapping_mul(0x9E37_79B9) as usize >> 7) & mask
}

/// What a call needs of a Lua function's prototype, copied into the closure so a call
/// reads it without following the prototype pointer. Made only from the prototype it sits
/// beside (`LuaFunc`), so its pointers are always that prototype's.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Shape {
    /// The chunk the function was compiled from (`Heap::sources`).
    pub(crate) source: u32,
    pub(crate) nparams: u8,
    /// A call enters through `Vm::precall`: the function takes `...` or clears its registers.
    pub(crate) precall: bool,
    pub(crate) maxstack: u16,
    /// [`Proto::c_maxstack`], the room the client's `luaD_precall` asks of the stack.
    pub(crate) c_maxstack: u8,
    /// The first instruction, so a call starts its frame without reaching the prototype.
    /// Valid as long as the prototype, which the closure holds.
    pub(crate) code: *const crate::proto::Op,
    /// The constant table, for the same reason.
    pub(crate) consts: *const Value,
}

impl Shape {
    pub(crate) fn of(p: &Proto) -> Shape {
        Shape {
            source: p.source_id,
            nparams: p.nparams,
            precall: p.vararg || p.clears,
            maxstack: p.maxstack,
            c_maxstack: p.c_maxstack,
            code: p.code.as_ptr(),
            consts: p.consts.as_ptr(),
        }
    }
}

/// A Lua closure: its code, its upvalues and its environment. The interpreter runs its code
/// through raw pointers (`Shape`, and a running frame's copies), so its parts are fixed by the
/// VM: the host reads them, and can copy a whole closure, but cannot build one or change one.
///
/// What the host can do:
///
/// ```
/// use benilla_lua::heap::FuncObj;
/// let mut vm = benilla_lua::new_state();
/// vm.exec(b"local n = 1 function F() n = n + 1 return n end", b"=t").unwrap();
/// let f = vm.get_global("F").as_func().unwrap();
/// let FuncObj::Lua(l) = vm.heap.func(f).clone() else { unreachable!() };
/// assert_eq!(&*l.proto().upval_names[0], b"n");
/// assert_eq!(vm.read_upvalue(l.upvals()[0]), benilla_lua::Value::Num(1.0));
/// vm.write_upvalue(l.upvals()[0], benilla_lua::Value::Num(41.0));
/// let env = vm.heap.new_table(0, 0);
/// assert!(vm.set_function_env(f, vm.globals));
/// let copy = vm.heap.new_func(FuncObj::Lua(l));
/// assert_eq!(vm.call_value(benilla_lua::Value::Func(copy), &[], 1).unwrap()[0],
///            benilla_lua::Value::Num(42.0));
/// # let _ = env;
/// ```
///
/// What it cannot, as each would free or misdirect code a frame is running:
///
/// ```compile_fail
/// let mut vm = benilla_lua::new_state();
/// let f = vm.get_global("print").as_func().unwrap();
/// let g = vm.heap.func(f).clone();
/// *vm.heap.func_mut(f) = g;
/// ```
///
/// ```compile_fail
/// use benilla_lua::heap::{FuncObj, LuaFunc};
/// let mut vm = benilla_lua::new_state();
/// vm.exec(b"function F() end", b"=t").unwrap();
/// let f = vm.get_global("F").as_func().unwrap();
/// let FuncObj::Lua(l) = vm.heap.func(f).clone() else { unreachable!() };
/// let forged = LuaFunc { proto: l.proto().clone(), upvals: Box::new([7]), ..l };
/// ```
///
/// ```compile_fail
/// let mut vm = benilla_lua::new_state();
/// vm.stack.truncate(0);
/// ```
///
/// ```compile_fail
/// let mut vm = benilla_lua::new_state();
/// *vm.heap.upval_mut(0) = benilla_lua::heap::UpvalObj::Open(u32::MAX);
/// ```
#[derive(Clone)]
pub struct LuaFunc {
    pub(crate) proto: Rc<Proto>,
    /// One upvalue per entry of `proto.upvals`.
    pub(crate) upvals: Upvals,
    pub(crate) env: TableRef,
    /// The prototype's call shape; replaced with it on a hot reload.
    pub(crate) shape: Shape,
}

impl LuaFunc {
    pub(crate) fn new(proto: Rc<Proto>, upvals: Upvals, env: TableRef) -> LuaFunc {
        debug_assert_eq!(upvals.len(), proto.upvals.len());
        let shape = Shape::of(&proto);
        LuaFunc {
            proto,
            upvals,
            env,
            shape,
        }
    }

    /// The code.
    pub fn proto(&self) -> &Rc<Proto> {
        &self.proto
    }

    /// The upvalues, as handles for `Vm::read_upvalue` and `Vm::write_upvalue`, in `proto().upvals`
    /// order.
    pub fn upvals(&self) -> &[u32] {
        &self.upvals
    }

    pub fn env(&self) -> TableRef {
        self.env
    }
}

/// A Lua closure's upvalue list: a boxed slice held by raw pointer. A running frame reads it
/// through a raw pointer of its own, and a hot reload moves the list into `Vm::retired` while
/// the frame still runs; a `Box`, moved, would assert unique access and invalidate the frame's
/// pointer, so the list is a `Box` only while it is made and when it is freed.
pub(crate) struct Upvals(std::ptr::NonNull<[u32]>);

impl Upvals {
    pub(crate) fn new(list: Vec<u32>) -> Upvals {
        Upvals(std::ptr::NonNull::from(Box::leak(list.into_boxed_slice())))
    }

    pub(crate) fn as_ptr(&self) -> *const u32 {
        self.0.as_ptr() as *const u32
    }
}

impl std::ops::Deref for Upvals {
    type Target = [u32];
    fn deref(&self) -> &[u32] {
        // SAFETY: the slice is live from `new` to `drop` and never written.
        unsafe { self.0.as_ref() }
    }
}

impl Clone for Upvals {
    fn clone(&self) -> Upvals {
        Upvals::new(self.to_vec())
    }
}

impl Drop for Upvals {
    fn drop(&mut self) {
        // SAFETY: made by `Box::leak` in `new`, freed once, here.
        drop(unsafe { Box::from_raw(self.0.as_ptr()) });
    }
}

#[derive(Clone)]
pub enum FuncObj {
    Lua(LuaFunc),
    Native {
        f: NativeFn,
        /// A host closure, called in `f`'s place when present.
        host: Option<HostFn>,
        name: &'static str,
        upvals: Box<[Value]>,
        env: TableRef,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum UpvalObj {
    /// Still a register: the absolute stack slot.
    Open(u32),
    Closed(Value),
}

#[derive(Clone, Debug)]
pub struct UserObj {
    pub meta: Option<TableRef>,
    /// The host's own handle for what this userdata stands for.
    pub tag: u64,
}

/// Objects of one type, addressed by handle. A handle is a plain index anyone can build, so
/// every slot holds a valid object: a freed slot, and the padding that keeps `items` a power of
/// two long, hold an empty one. A handle is masked into bounds, so a stale or foreign handle
/// reads some object of the right type, never freed or outside memory, and an access costs one
/// `and` and no branch.
#[derive(Clone)]
struct Arena<T> {
    items: Vec<T>,
    /// Each slot's `FREE`, `LIVE` or `MARKED`.
    state: Vec<u8>,
    free: Vec<u32>,
    /// Slots ever handed out: the rest of `items` is padding.
    used: u32,
    /// `items.len() - 1`.
    mask: usize,
}

/// An arena's first size, and the least a sweep shrinks it to.
const ARENA_FIRST: usize = 64;

/// A slot with no live object.
const FREE: u8 = 0;
/// A live object a collection has not reached yet.
const LIVE: u8 = 1;
/// A live object this collection reached.
const MARKED: u8 = 2;

/// The empty object a slot with no live one holds.
trait Dead {
    fn dead() -> Self;
}

impl Dead for StrObj {
    fn dead() -> Self {
        StrObj {
            data: StrData::Inline(0, [0; INLINE]),
            hash: 0,
        }
    }
}

impl Dead for Table {
    fn dead() -> Self {
        Table::dead()
    }
}

impl Dead for FuncObj {
    fn dead() -> Self {
        FuncObj::Native {
            f: dead_function,
            host: None,
            name: "?",
            upvals: Box::new([]),
            env: TableRef(0),
        }
    }
}

impl Dead for UpvalObj {
    fn dead() -> Self {
        UpvalObj::Closed(Value::Nil)
    }
}

impl Dead for UserObj {
    fn dead() -> Self {
        UserObj { meta: None, tag: 0 }
    }
}

impl<T: Dead> Arena<T> {
    fn new() -> Self {
        Arena {
            items: (0..ARENA_FIRST).map(|_| T::dead()).collect(),
            state: vec![FREE; ARENA_FIRST],
            free: Vec::new(),
            used: 0,
            mask: ARENA_FIRST - 1,
        }
    }

    fn alloc(&mut self, v: T) -> u32 {
        let i = match self.free.pop() {
            Some(i) => i,
            None => {
                if self.used as usize == self.items.len() {
                    let n = self.items.len() * 2;
                    self.items.resize_with(n, T::dead);
                    self.state.resize(n, FREE);
                    self.mask = n - 1;
                }
                self.used += 1;
                self.used - 1
            }
        };
        // The empty object owns nothing (anything a stale handle wrote into it is leaked, not
        // dropped): no drop to run on the hot path of every allocation.
        std::mem::forget(std::mem::replace(&mut self.items[i as usize], v));
        self.state[i as usize] = LIVE;
        i
    }

    /// Whether handle `i` names a live object.
    fn is_live(&self, i: u32) -> bool {
        self.state.get(i as usize).is_some_and(|&s| s != FREE)
    }

    /// The object handle `i` names: masked into bounds (see the type). A debug build panics on
    /// a handle to no live object; the VM never makes one.
    #[inline(always)]
    fn get(&self, i: u32) -> &T {
        debug_assert!(self.is_live(i), "a live handle");
        let i = i as usize & self.mask;
        // SAFETY: `items.len()` is a power of two and `mask` one less, so `i` is below it.
        unsafe { self.items.get_unchecked(i) }
    }

    #[inline(always)]
    fn get_mut(&mut self, i: u32) -> &mut T {
        debug_assert!(self.is_live(i), "a live handle");
        let i = i as usize & self.mask;
        // SAFETY: as `get`.
        unsafe { self.items.get_unchecked_mut(i) }
    }

    /// Mark live object `i`: whether it was live and not yet marked.
    fn mark(&mut self, i: u32) -> bool {
        match self.state.get_mut(i as usize) {
            Some(s) if *s == LIVE => {
                *s = MARKED;
                true
            }
            _ => false,
        }
    }

    fn iter_live(&self) -> impl Iterator<Item = (usize, &T)> {
        self.items
            .iter()
            .zip(&self.state)
            .enumerate()
            .filter(|(_, (_, &s))| s != FREE)
            .map(|(i, (o, _))| (i, o))
    }

    fn iter_live_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.items
            .iter_mut()
            .zip(&self.state)
            .filter(|(_, &s)| s != FREE)
            .map(|(o, _)| o)
    }

    fn live(&self) -> usize {
        self.used as usize - self.free.len()
    }
}

/// 5.0's string hash (`luaS_newlstr`): seeded with the length, sampling at most ~32 bytes.
pub fn str_hash(s: &[u8]) -> u32 {
    let l = s.len();
    let mut h = l as u32;
    let step = (l >> 5) + 1;
    let mut l1 = l;
    while l1 >= step {
        h ^= (h << 5).wrapping_add(h >> 2).wrapping_add(s[l1 - 1] as u32);
        l1 -= step;
    }
    h
}

/// One owner's share of the heap.
#[derive(Clone, Copy, Debug, Default)]
pub struct Census {
    pub tables: usize,
    pub functions: usize,
    pub bytes: usize,
}

#[derive(Clone)]
pub struct Heap {
    pub(crate) identity: Identity,
    strs: Arena<StrObj>,
    /// The intern table: open addressing, linear probing, at most half full.
    strt: Vec<u32>,
    /// The buckets 5.0's string table would have for the same strings, which the count charges:
    /// 32 at open (`0x6f6e96`), doubled when the strings outnumber them (`0x6f9e0f`), halved by a
    /// collection that leaves them under a quarter full above 64 (`0x6f738c`).
    strt_buckets: usize,
    /// The size of 5.0's concatenation buffer (`[G+0x1c]`, `[G+0x20]`), which the count charges:
    /// grown to each concatenation's length, 32 at least (`luaZ_openspace`, `0x6fb5d1`), and
    /// halved by a collection while over 64 (`checkSizes`, `0x6f73a1`..`0x6f73cf`).
    concat50: usize,
    /// Where the client's allocator keeps the concatenation buffer, which `luaZ_resizebuffer`
    /// reallocates (`0x6fb5d1`, `0x6f73c1`).
    concat_block: Block,
    tables: Tables,
    funcs: Arena<FuncObj>,
    upvals: Arena<UpvalObj>,
    users: Arena<UserObj>,
    /// 5.0's block count (`nblocks`, `[G+0x28]`): every object at the size the 32-bit client
    /// allocates it, recounted whole at each collection and added to as objects are made; table
    /// resizes are added as [`Heap::bytes`] reads it.
    bytes: usize,
    /// 5.0's `GCthreshold` (`[G+0x24]`), in 32 bits: what `gcinfo` answers, and where the next
    /// collection's headroom is reckoned from.
    threshold: usize,
    /// A collection runs when `bytes` reaches it: the threshold, or 0 while the heap asks for one
    /// before it ([`Heap::ask_collection`]).
    pub(crate) trigger: usize,
    /// A collection the heap asked for has not run yet: a new threshold leaves the trigger at 0.
    asked: bool,
    /// Collections run since the state opened: an instrument for the collector's pacing.
    pub collections: u64,
    /// The chunk names prototypes were compiled under, by source id: the owners a profile and
    /// a memory census charge to. [`Heap::free_unreferenced_sources`] frees the slot of a chunk
    /// no function or table refers to any more (a `loadstring` chunk is named by its own code),
    /// and a new chunk takes a free slot: `None` is a free one. Id 0 is the host's and never
    /// freed. A profile's time for an id carries on into the chunk that takes its slot:
    /// [`Heap::source_seq`] tells the two apart.
    pub sources: Vec<Option<crate::proto::Bytes>>,
    /// The rest of the sources' bookkeeping, boxed: the heap sits inside the VM, whose layout
    /// the interpreter loop's code is sensitive to.
    src: Box<SourceBook>,
    source_ids: std::collections::HashMap<crate::proto::Bytes, u32>,
    /// The chunk whose code runs now; a new table is charged to it.
    pub cur_source: u32,
    /// `intern_with`'s buffer.
    scratch: Vec<u8>,
    /// A string could not be made: no id was left, or no memory. [`Heap::intern`] answered
    /// [`Heap::oom_str`] in its place, and the VM raises `not enough memory` where it next checks.
    pub(crate) oom: bool,
    /// `not enough memory`, made first and never collected: what a failed intern answers, and the
    /// error value, which needs no new string.
    pub(crate) oom_str: StrRef,
    /// Ids a string may take: [`STR_ID_MASK`], lower in a test.
    str_id_limit: u32,
    /// Past this many ids, with none free, a collection is asked for before ids run out.
    str_gc_at: u32,
    /// Host closures the last collection freed, not yet dropped (`Vm::full_gc`).
    pub(crate) freed_hosts: Vec<FuncObj>,
}

/// The tables and the growth their resizes made that `Heap::bytes` does not hold yet, which
/// every table the heap made adds to. A clone's tables add to the clone's own.
struct Tables {
    arena: Arena<Table>,
    acct: Rc<Account>,
}

impl Clone for Tables {
    fn clone(&self) -> Tables {
        let acct = Rc::new(self.acct.fork());
        let mut arena = self.arena.clone();
        for t in &mut arena.items {
            if t.growth.0.is_some() {
                t.growth = Growth(Some(acct.clone()));
            }
        }
        Tables { arena, acct }
    }
}

impl std::ops::Deref for Tables {
    type Target = Arena<Table>;
    #[inline(always)]
    fn deref(&self) -> &Arena<Table> {
        &self.arena
    }
}

impl std::ops::DerefMut for Tables {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Arena<Table> {
        &mut self.arena
    }
}

/// [`Heap::sources`]' bookkeeping.
#[derive(Clone, Default)]
struct SourceBook {
    /// The order each slot's chunk was first compiled in, which slot order no longer is once a
    /// slot is reused.
    seq: Vec<u64>,
    /// The chunk that was running when each slot's chunk was compiled, and that chunk's `seq`
    /// then, so a reused slot is not taken for it: a `loadstring` chunk's parent is its caller's.
    parent: Vec<(u32, u64)>,
    next_seq: u64,
    /// Moves whenever a chunk takes or leaves a slot.
    epoch: u64,
    free: Vec<u32>,
    /// New slots left before a new chunk first frees the slots nothing refers to: as many as
    /// lived after the last such pass, 64 at least, so the passes cost the heap's size once per
    /// that many new chunks.
    scan_in: usize,
}

/// Which VM or heap this is: a fresh number for each one made and each clone. Natives hold
/// `&mut Vm`, so one could put another VM or heap in place of the running one
/// (`*vm = new_state()`), freeing the code and stack the running frames point into; the
/// interpreter compares identities after every native and stops instead ([`Vm::identity`]).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Identity(pub(crate) u64);

impl Default for Identity {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Identity(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

impl Clone for Identity {
    fn clone(&self) -> Self {
        Identity::default()
    }
}

impl Default for Heap {
    fn default() -> Self {
        let mut h = Heap {
            identity: Identity::default(),
            strs: Arena::new(),
            strt: vec![STRT_EMPTY; 128],
            strt_buckets: STRT_FIRST,
            concat50: 0,
            concat_block: Block::None,
            tables: Tables {
                arena: Arena::new(),
                acct: Default::default(),
            },
            funcs: Arena::new(),
            upvals: Arena::new(),
            users: Arena::new(),
            bytes: strt_bytes(STRT_FIRST),
            threshold: 64 * 1024,
            trigger: 64 * 1024,
            asked: false,
            collections: 0,
            // Id 0 is the host: whatever no Lua chunk created.
            sources: vec![Some(Rc::from(&b"=[host]"[..]))],
            source_ids: Default::default(),
            src: Box::new(SourceBook {
                seq: vec![0],
                parent: vec![(0, 0)],
                scan_in: 64,
                ..Default::default()
            }),
            cur_source: 0,
            scratch: Vec::new(),
            oom: false,
            oom_str: StrRef { id: 0, hash: 0 },
            str_id_limit: STR_ID_MASK,
            str_gc_at: STR_ID_MASK / 2,
            freed_hosts: Vec::new(),
        };
        // `f_luaopen`'s string table (`luaS_resize`, `0x6f6e96`).
        h.pools().alloc(strt_bytes(STRT_FIRST));
        h.oom_str = h.intern(b"not enough memory");
        h
    }
}

impl Heap {
    // ── strings ──────────────────────────────────────────────────────────────────────────────

    /// The interned string for `s`.
    pub fn intern(&mut self, s: &[u8]) -> StrRef {
        let hash = str_hash(s);
        let mask = self.strt.len() - 1;
        let mut b = strt_home(hash, mask);
        loop {
            let e = self.strt[b];
            if e == STRT_EMPTY {
                break;
            }
            if e & !STR_ID_MASK == strt_tag(hash) {
                let id = e & STR_ID_MASK;
                let o = self.strs.get(id);
                if o.hash == hash && o.bytes() == s {
                    return StrRef {
                        id,
                        hash: hash & STR_HASH_MASK,
                    };
                }
            }
            b = (b + 1) & mask;
        }
        // A string value holds its id in `STR_ID_BITS`: sixteen million live strings, the last
        // id left free so no intern slot reads as empty. Deviation: C Lua has no such bound;
        // past it the string is `not enough memory`, as C's would be when memory runs out.
        let fresh = self.strs.free.is_empty();
        if fresh && self.strs.used >= self.str_id_limit {
            return self.out_of_memory();
        }
        if fresh && self.strs.used >= self.str_gc_at {
            // Ids run short: a collection, at the next check, frees the dead ones first. It
            // moves the mark to halfway between what lives and the bound; until it runs, the
            // mark moves halfway on, so the request is not made for every new string.
            self.ask_collection();
            self.str_gc_at = self.strs.used + (self.str_id_limit - self.strs.used) / 2;
        }
        // A string past `MAX_STRING`, or one whose bytes the allocator refuses, is C's
        // `not enough memory` too, where Rust's allocator would abort.
        if s.len() > crate::api::MAX_STRING {
            return self.out_of_memory();
        }
        let Some(obj) = StrObj::new(s, hash) else {
            return self.out_of_memory();
        };
        self.bytes += str_bytes(s);
        self.pools().alloc(str_bytes(s));
        let id = self.strs.alloc(obj);
        self.strt[b] = strt_entry(hash, id);
        if self.strs.live() * 2 > self.strt.len() {
            self.resize_strt(self.strt.len() * 2);
        }
        if self.strs.live() > self.strt_buckets {
            self.resize_strt50(self.strt_buckets * 2);
        }
        StrRef {
            id,
            hash: hash & STR_HASH_MASK,
        }
    }

    /// A string that cannot be made: the pending error set, a collection asked for, and
    /// [`Heap::oom_str`] in its place. Deviation: the client's `luaM_realloc` (`0x6fc980`) has no
    /// failure path to ask from; here a refused string asks for the collection that may free room.
    #[cold]
    #[inline(never)]
    fn out_of_memory(&mut self) -> StrRef {
        self.oom = true;
        self.ask_collection();
        self.oom_str
    }

    /// A collection at the next check, whatever the threshold: benilla's own, which the client
    /// never asks for, so the threshold stays the client's.
    fn ask_collection(&mut self) {
        self.asked = true;
        self.trigger = 0;
    }

    /// Whether a string could not be made since the last check, clearing it: for a host that
    /// made a string outside a call, where no native return checks for it.
    pub fn take_oom(&mut self) -> bool {
        std::mem::take(&mut self.oom)
    }

    /// Lower the number of string ids, so a test reaches the bound without gigabytes of strings.
    #[doc(hidden)]
    pub fn limit_string_ids(&mut self, n: u32) {
        self.str_id_limit = n.min(STR_ID_MASK);
        self.str_gc_at = self.str_id_limit / 2;
    }

    /// Rebuild the intern table at `size` slots from the live strings.
    fn resize_strt(&mut self, size: usize) {
        let mut strt = vec![STRT_EMPTY; size];
        let mask = size - 1;
        for (i, o) in self.strs.iter_live() {
            let mut b = strt_home(o.hash, mask);
            while strt[b] != STRT_EMPTY {
                b = (b + 1) & mask;
            }
            strt[b] = strt_entry(o.hash, i as u32);
        }
        self.strt = strt;
    }

    /// The string `fill` writes, built in a buffer the heap keeps: a string made from heap
    /// strings without a fresh allocation.
    pub fn intern_with(&mut self, fill: impl FnOnce(&Heap, &mut Vec<u8>)) -> StrRef {
        let mut b = std::mem::take(&mut self.scratch);
        b.clear();
        fill(self, &mut b);
        let r = self.intern(&b);
        self.scratch = b;
        r
    }

    pub fn str_value(&mut self, s: &[u8]) -> Value {
        Value::Str(self.intern(s))
    }

    #[inline]
    pub fn str(&self, s: StrRef) -> &[u8] {
        self.strs.get(s.id).bytes()
    }

    // ── tables ───────────────────────────────────────────────────────────────────────────────

    /// The id of chunk name `name`, made on first sight, in a freed slot if there is one.
    pub fn source_id(&mut self, name: &crate::proto::Bytes) -> u32 {
        if let Some(&id) = self.source_ids.get(name) {
            return id;
        }
        if self.src.free.is_empty() {
            if self.src.scan_in == 0 {
                self.free_unreferenced_sources();
            } else {
                self.src.scan_in -= 1;
            }
        }
        let book = &mut *self.src;
        book.next_seq += 1;
        let seq = book.next_seq;
        let cur = self.cur_source;
        let parent = (cur, book.seq.get(cur as usize).copied().unwrap_or(0));
        let id = match book.free.pop() {
            Some(id) => {
                self.sources[id as usize] = Some(name.clone());
                book.seq[id as usize] = seq;
                book.parent[id as usize] = parent;
                id
            }
            None => {
                self.sources.push(Some(name.clone()));
                book.seq.push(seq);
                book.parent.push(parent);
                self.sources.len() as u32 - 1
            }
        };
        self.source_ids.insert(name.clone(), id);
        book.epoch += 1;
        id
    }

    /// Free the slot of every source nothing refers to: no table's owner, no Lua function's
    /// prototype (a nested prototype shares its parent's chunk), not the one being charged now.
    /// Objects a collection has not swept yet still count, so a slot leaves only once a
    /// collection has freed what referred to it. A new chunk runs this once it has made as many
    /// new slots as the last pass kept (64 at least); a host that wants the slots back sooner
    /// runs it after a collection.
    pub fn free_unreferenced_sources(&mut self) {
        let mut live = vec![false; self.sources.len()];
        live[0] = true;
        live[self.cur_source as usize] = true;
        for (_, t) in self.tables.iter_live() {
            live[t.owner as usize] = true;
        }
        for (_, f) in self.funcs.iter_live() {
            if let FuncObj::Lua(LuaFunc { proto, .. }) = f {
                live[proto.source_id as usize] = true;
            }
        }
        let mut kept = 0;
        for (id, live) in live.iter().enumerate() {
            if *live {
                kept += 1;
            } else if let Some(name) = self.sources[id].take() {
                self.source_ids.remove(&name);
                self.src.free.push(id as u32);
                self.src.epoch += 1;
            }
        }
        self.src.scan_in = kept.max(64);
    }

    /// The chunk name of source `id`, if the slot holds one.
    pub fn source_name(&self, id: u32) -> Option<&crate::proto::Bytes> {
        self.sources.get(id as usize)?.as_ref()
    }

    /// The live sources in the order they were first compiled: `(id, name)`.
    pub fn sources_in_order(&self) -> Vec<(u32, &crate::proto::Bytes)> {
        let mut v: Vec<(u32, &crate::proto::Bytes)> = self
            .sources
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((i as u32, s.as_ref()?)))
            .collect();
        v.sort_by_key(|&(i, _)| self.src.seq[i as usize]);
        v
    }

    /// The order source `id`'s chunk was first compiled in among all chunks: slot order is not
    /// compile order once a slot is reused.
    pub fn source_seq(&self, id: u32) -> u64 {
        self.src.seq.get(id as usize).copied().unwrap_or(0)
    }

    /// The chunk that was running when source `id`'s chunk was compiled, while that chunk still
    /// holds its slot; `None` for one the host compiled.
    pub fn source_parent(&self, id: u32) -> Option<u32> {
        let &(p, seq) = self.src.parent.get(id as usize)?;
        (p != 0 && p != id && self.source_seq(p) == seq).then_some(p)
    }

    /// A number that moves whenever a chunk takes or leaves a source slot: a view built from
    /// [`Heap::sources`] is stale once it differs.
    pub fn sources_epoch(&self) -> u64 {
        self.src.epoch
    }

    /// Bytes and objects per owner: each table to the chunk that made it, each closure to its
    /// prototype's chunk. Indexed by source id.
    pub fn census(&self) -> Vec<Census> {
        let mut out = vec![Census::default(); self.sources.len()];
        for (_, t) in self.tables.iter_live() {
            let c = &mut out[t.owner as usize];
            c.tables += 1;
            c.bytes += t.bytes();
        }
        for (_, f) in self.funcs.iter_live() {
            if let FuncObj::Lua(LuaFunc { proto, .. }) = f {
                let c = &mut out[proto.source_id as usize];
                c.functions += 1;
                c.bytes += func_bytes(f);
            }
        }
        out
    }

    /// `luaH_new`: `narray` array slots and room for `nhash` keys.
    pub fn new_table(&mut self, narray: usize, nhash: usize) -> TableRef {
        let mut t = Table::new(narray, nhash);
        t.owner = self.cur_source;
        t.growth = Growth(Some(self.tables.acct.clone()));
        self.bytes += t.bytes();
        // `luaH_new` (`0x6fa4f0`): the table, then its array part, then its node vector.
        let mut p = self.tables.acct.pools.borrow_mut();
        p.alloc(0x20);
        t.array_block = p.alloc(16 * t.array.len());
        p.alloc(40 * t.node_count());
        drop(p);
        TableRef(self.tables.alloc(t))
    }

    #[inline]
    pub fn table(&self, t: TableRef) -> &Table {
        self.tables.get(t.0)
    }

    #[inline]
    pub fn table_mut(&mut self, t: TableRef) -> &mut Table {
        self.tables.get_mut(t.0)
    }

    // ── functions and upvalues ───────────────────────────────────────────────────────────────

    pub fn new_func(&mut self, f: FuncObj) -> FuncRef {
        self.bytes += func_bytes(&f);
        self.pools().alloc(func_bytes(&f));
        FuncRef(self.funcs.alloc(f))
    }

    #[inline]
    pub fn func(&self, f: FuncRef) -> &FuncObj {
        self.funcs.get(f.0)
    }

    #[inline]
    /// Crate-internal: replacing a running closure's code would free what its frame executes.
    pub(crate) fn func_mut(&mut self, f: FuncRef) -> &mut FuncObj {
        self.funcs.get_mut(f.0)
    }

    /// Every live function, for a hot reload's walk.
    pub(crate) fn funcs_mut(&mut self) -> impl Iterator<Item = &mut FuncObj> {
        self.funcs.iter_live_mut()
    }

    pub fn new_upval(&mut self, u: UpvalObj) -> u32 {
        self.bytes += UPVAL_BYTES;
        self.pools().alloc(UPVAL_BYTES);
        self.upvals.alloc(u)
    }

    #[inline]
    pub fn upval(&self, i: u32) -> UpvalObj {
        *self.upvals.get(i)
    }

    #[inline]
    /// Crate-internal: an open upvalue names a stack slot the interpreter reads unchecked, so
    /// the host writes an upvalue's value through `Vm::write_upvalue`.
    pub(crate) fn upval_mut(&mut self, i: u32) -> &mut UpvalObj {
        self.upvals.get_mut(i)
    }

    pub fn new_user(&mut self, u: UserObj) -> UserRef {
        self.bytes += USER_BYTES;
        self.pools().alloc(USER_BYTES);
        UserRef(self.users.alloc(u))
    }

    pub fn user(&self, u: UserRef) -> &UserObj {
        self.users.get(u.0)
    }

    /// The tag of userdata `u`, if the handle names a live one.
    pub fn user_tag(&self, u: UserRef) -> Option<u64> {
        self.users.is_live(u.0).then(|| self.users.get(u.0).tag)
    }

    pub fn user_mut(&mut self, u: UserRef) -> &mut UserObj {
        self.users.get_mut(u.0)
    }

    /// The metatable of a value: tables and userdata carry one; 1.12 has none for any other type.
    pub fn metatable(&self, v: Value) -> Option<TableRef> {
        match v.unpack() {
            V::Table(t) => self.table(t).meta,
            V::User(u) => self.user(u).meta,
            _ => None,
        }
    }

    // ── collection ───────────────────────────────────────────────────────────────────────────

    /// Live objects per arena: strings, tables, functions, upvalues, userdata.
    pub fn counts(&self) -> [usize; 5] {
        [
            self.strs.live(),
            self.tables.live(),
            self.funcs.live(),
            self.upvals.live(),
            self.users.live(),
        ]
    }

    /// Mark from `roots` (and `extra_upvals`, the open upvalues, and `code`, prototypes and
    /// upvalue lists no closure holds but a running frame does), clear weak entries, sweep.
    /// Answers the count of what lives, from which the VM sets the next threshold.
    /// Crate-internal: roots the host chose could free a closure a frame is running.
    pub(crate) fn collect(
        &mut self,
        roots: &[Value],
        extra_upvals: &[u32],
        code: &[(Rc<Proto>, Upvals)],
        mode_key: StrRef,
        fixed: &[Block],
    ) -> usize {
        self.asked = false;
        self.trigger = self.threshold;
        let mut gray: Vec<Value> = roots.to_vec();
        gray.push(Value::Str(self.oom_str));
        let mut gray_upvals: Vec<u32> = extra_upvals.to_vec();
        let mut weak_tables: Vec<u32> = Vec::new();
        // Constants of every reached prototype, once each.
        // A number no other collection, in any VM, has used: prototypes are shared by forks.
        let epoch = GC_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let mut proto_bytes = 0;
        // The client's blocks that stay, each pool: the pools' book is recounted from them.
        let mut tally = Tally::default();
        for (p, ups) in code {
            mark_proto(p, &mut gray, epoch, &mut proto_bytes, &mut tally);
            gray_upvals.extend_from_slice(ups);
        }
        loop {
            if let Some(u) = gray_upvals.pop() {
                if self.upvals.mark(u) {
                    if let UpvalObj::Closed(v) = self.upval(u) {
                        gray.push(v);
                    }
                }
                continue;
            }
            let Some(v) = gray.pop() else { break };
            // A register the VM never cleared may hold a handle whose object died; such a
            // handle is skipped (`Vm::precall` leaves registers above the parameters as they
            // were, and the compiler writes every register before it reads it).
            if !self.is_live(v) {
                continue;
            }
            match v.unpack() {
                V::Str(s) => {
                    self.strs.mark(s.id);
                }
                V::Table(t) => {
                    if !self.tables.mark(t.0) {
                        continue;
                    }
                    let meta = self.tables.get(t.0).meta;
                    let weak = match meta {
                        Some(m) => self.weak_mode(m, mode_key),
                        None => 0,
                    };
                    if let Some(m) = meta {
                        gray.push(Value::Table(m));
                    }
                    let (wk, wv) = (weak & 1 != 0, weak & 2 != 0);
                    if weak != 0 {
                        weak_tables.push(t.0);
                    }
                    // Strings are values, never cleared from a weak table: always marked, here,
                    // since they hold nothing. A table's contents are live objects. Any other
                    // collectable goes gray unless the table holds it weakly.
                    let strs = &mut self.strs;
                    let tab = self.tables.get_mut(t.0);
                    let mut reach = |v: Value, weak: bool| match v.unpack() {
                        V::Str(s) => {
                            strs.mark(s.id);
                        }
                        V::Table(_) | V::Func(_) | V::User(_) if !weak => gray.push(v),
                        _ => {}
                    };
                    for &v in &tab.array {
                        reach(v, wv);
                    }
                    for n in tab.node.iter_mut() {
                        if n.val.is_nil() {
                            // 5.1.5's `removeentry`: an empty entry's collectable key is dead.
                            n.dead |= is_collectable(n.key);
                            continue;
                        }
                        reach(n.key, wk);
                        reach(n.val, wv);
                    }
                    tab.weak = weak;
                }
                V::Func(f) => {
                    if !self.funcs.mark(f.0) {
                        continue;
                    }
                    match self.funcs.get(f.0) {
                        FuncObj::Lua(LuaFunc {
                            proto, upvals, env, ..
                        }) => {
                            gray.push(Value::Table(*env));
                            gray_upvals.extend_from_slice(upvals);
                            mark_proto(proto, &mut gray, epoch, &mut proto_bytes, &mut tally);
                        }
                        FuncObj::Native { upvals, env, .. } => {
                            gray.push(Value::Table(*env));
                            gray.extend_from_slice(upvals);
                        }
                    }
                }
                V::User(u) => {
                    if !self.users.mark(u.0) {
                        continue;
                    }
                    if let Some(m) = self.users.get(u.0).meta {
                        gray.push(Value::Table(m));
                    }
                }
                _ => {}
            }
        }
        // Weak tables drop entries whose collectable key or value died.
        for t in weak_tables {
            let (tables, funcs, users) = (
                &self.tables.arena.state,
                &self.funcs.state,
                &self.users.state,
            );
            let marked = |st: &Vec<u8>, i: u32| st.get(i as usize) == Some(&MARKED);
            let alive = |v: Value| match v.unpack() {
                V::Table(r) => marked(tables, r.0),
                V::Func(r) => marked(funcs, r.0),
                V::User(r) => marked(users, r.0),
                _ => true,
            };
            let tab = &mut self.tables.arena.items[t as usize];
            for v in tab.array.iter_mut() {
                if !alive(*v) {
                    *v = Value::Nil;
                }
            }
            for n in tab.node.iter_mut() {
                if !n.val.is_nil() && (!alive(n.key) || !alive(n.val)) {
                    n.val = Value::Nil;
                    n.dead = is_collectable(n.key);
                }
            }
        }
        // Sweep. Dead strings leave the intern chains.
        let mut live = 0usize;
        live += sweep(
            &mut self.tables,
            |t| {
                tally.count_size(0x20);
                tally.count(t.array_block);
                tally.count_size(40 * t.node_count());
                t.bytes()
            },
            |_| false,
            drop,
        );
        // A host closure's captures run Rust destructors, which may reach the VM: they wait in
        // `freed_hosts` until the collection ends (`Vm::full_gc`).
        let freed = &mut self.freed_hosts;
        live += sweep(
            &mut self.funcs,
            |f| {
                tally.count_size(func_bytes(f));
                func_bytes(f)
            },
            |f| matches!(f, FuncObj::Native { host: Some(_), .. }),
            |f| freed.push(f),
        );
        let mut fixed_size = |n: usize| {
            tally.count_size(n);
            n
        };
        live += sweep(
            &mut self.upvals,
            |_| fixed_size(UPVAL_BYTES),
            |_| false,
            drop,
        );
        live += sweep(&mut self.users, |_| fixed_size(USER_BYTES), |_| false, drop);
        let strs_before = self.strs.live();
        live += sweep(
            &mut self.strs,
            |s| fixed_size(str_bytes(s.bytes())),
            |_| false,
            drop,
        );
        let dead_str = self.strs.live() != strs_before;
        let live_strs = self.strs.live() as u32;
        self.str_gc_at = live_strs + (self.str_id_limit.saturating_sub(live_strs)) / 2;
        if dead_str {
            let size = self.strt.len();
            self.resize_strt(size);
        }
        // A dead key may name a freed object, whose handle a new one may take; as 5.1's dead key,
        // it is found only by `next`, by identity (`Table::slot_for`).
        // Every live table's size is counted whole above, its uncounted growth with it.
        self.tables.acct.growth.set(0);
        self.bytes = live + proto_bytes + strt_bytes(self.strt_buckets) + self.concat50;
        tally.count_size(strt_bytes(self.strt_buckets));
        tally.count(self.concat_block);
        for &b in fixed {
            tally.count(b);
        }
        self.pools().recounted(tally);
        // `checkSizes` (`0x6f7370`): a string table under a quarter full above 64 buckets halves
        // (`0x6f738c`), and a concatenation buffer over 64 bytes (`0x6f73a7`), once each.
        if self.strs.live() < self.strt_buckets / 4 && self.strt_buckets > 2 * STRT_FIRST {
            self.resize_strt50(self.strt_buckets / 2);
        }
        if self.concat50 > 0x40 {
            self.resize_concat50(self.concat50 / 2);
        }
        self.bytes
    }

    /// `n` more bytes the collector counts, as C's allocator would: compiled code, the stack.
    pub(crate) fn charge(&mut self, n: usize) {
        self.bytes += n;
    }

    /// A concatenation `n` bytes long: 5.0's buffer grown to hold it (`luaZ_openspace`,
    /// `0x6fb5c0`).
    pub(crate) fn concat_space(&mut self, n: usize) {
        if n > self.concat50 {
            self.resize_concat50(n.max(0x20));
        }
    }

    /// `luaZ_resizebuffer` on 5.0's concatenation buffer: a reallocation.
    fn resize_concat50(&mut self, size: usize) {
        self.recharge(self.concat50, size);
        let b = self.pools().realloc(self.concat_block, size);
        self.concat_block = b;
        self.concat50 = size;
    }

    /// `luaS_resize` (`0x6f9c50`) on 5.0's string table: a new bucket array, then the old freed.
    fn resize_strt50(&mut self, buckets: usize) {
        self.recharge(strt_bytes(self.strt_buckets), strt_bytes(buckets));
        let mut p = self.pools();
        p.alloc(strt_bytes(buckets));
        p.free(Pools::place(strt_bytes(self.strt_buckets)));
        drop(p);
        self.strt_buckets = buckets;
    }

    /// The client's block pools, as the objects counted so far have used them.
    pub(crate) fn pools(&self) -> std::cell::RefMut<'_, Pools> {
        self.tables.acct.pools.borrow_mut()
    }

    /// A compiled prototype and those nested in it, counted as the client's parser makes them:
    /// the `Proto` as `open_func` begins (`0x6f9fea`), its vectors grown as it compiles and fitted
    /// as `close_func` ends (`0x6fcb48`..`0x6fcc26`).
    pub(crate) fn charge_proto(&mut self, p: &Proto) {
        self.bytes += p.bytes();
        self.pools().alloc(crate::proto::PROTO_BYTES);
        for q in &p.protos {
            self.charge_proto(q);
        }
        let mut pools = self.pools();
        for (elem, count) in p.vectors() {
            pools.grow_vector(elem, count);
        }
    }

    /// 5.0's `GCthreshold` (`lua_getgcthreshold`, `0x6f43e0`, in bytes).
    pub fn threshold(&self) -> usize {
        self.threshold
    }

    /// Set the threshold, and the next collection's trigger with it unless one is asked for.
    pub fn set_threshold(&mut self, t: usize) {
        self.threshold = t;
        self.trigger = if self.asked { 0 } else { t };
    }

    /// The threshold a collection leaves (`0x6fae00`, [`crate::pools::next_threshold`]), from
    /// the count it left and the old threshold.
    pub(crate) fn pace(&mut self) {
        let count = u32::try_from(self.bytes()).unwrap_or(u32::MAX);
        let old = u32::try_from(self.threshold).unwrap_or(u32::MAX);
        let least = self.pools().least_free();
        self.set_threshold(crate::pools::next_threshold(old, count, least) as usize);
    }

    /// A block of `old` bytes the collector counts reallocated to `new`, as `luaM_irealloc`
    /// counts it (`0x6fc996`..`0x6fc99e`).
    pub(crate) fn recharge(&mut self, old: usize, new: usize) {
        self.bytes = (self.bytes + new).saturating_sub(old);
    }

    /// The count, 5.0's `nblocks`: what the last collection found, what was made since, and what
    /// tables grew by since.
    pub fn bytes(&mut self) -> usize {
        let g = self.tables.acct.growth.replace(0);
        if g != 0 {
            self.bytes = self.bytes.saturating_add_signed(g);
        }
        self.bytes
    }

    /// Whether a value's object still exists; anything not collectable does.
    fn is_live(&self, v: Value) -> bool {
        match v.unpack() {
            V::Str(s) => self.strs.is_live(s.id),
            V::Table(t) => self.tables.is_live(t.0),
            V::Func(f) => self.funcs.is_live(f.0),
            V::User(u) => self.users.is_live(u.0),
            _ => true,
        }
    }

    /// `__mode` of a metatable: bit 0 weak keys, bit 1 weak values.
    fn weak_mode(&self, meta: TableRef, mode_key: StrRef) -> u8 {
        match (self.table(meta).get(Value::Str(mode_key))).unpack() {
            V::Str(s) => {
                let m = self.str(s);
                (m.contains(&b'k') as u8) | ((m.contains(&b'v') as u8) << 1)
            }
            _ => 0,
        }
    }
}

/// 5.0's string table at open, `MINSTRTABSIZE` buckets (`luaS_resize` at `0x6f6e96`).
const STRT_FIRST: usize = 32;

/// An upvalue in the collector's count: `luaF_findupval`'s 0x20 bytes (`0x6f9f38`).
const UPVAL_BYTES: usize = 0x20;

/// A userdata in the collector's count: `luaS_newudata`'s header, `size + 0x10` (`0x6f9e45`),
/// with no block of its own here.
const USER_BYTES: usize = 0x10;

/// 5.0's string table in the collector's count: a pointer a bucket (`luaS_resize`, `0x6f9c62`).
fn strt_bytes(buckets: usize) -> usize {
    4 * buckets
}

/// A string in the collector's count: 5.0's `TString`, its text and a NUL, `len + 0x11`
/// (`newlstr`, `0x6f9da1`).
fn str_bytes(s: &[u8]) -> usize {
    0x11 + s.len()
}

/// A closure in the collector's count: a Lua closure is `0x24` and 4 an upvalue
/// (`luaF_newLclosure`, `0x6f9ebf`), a C closure `(n + 1) * 16` (`luaF_newCclosure`, `0x6f9e7c`).
fn func_bytes(f: &FuncObj) -> usize {
    match f {
        FuncObj::Lua(l) => 0x24 + 4 * l.upvals.len(),
        FuncObj::Native { upvals, .. } => 16 * (upvals.len() + 1),
    }
}

/// The function a handle to no live function calls.
fn dead_function(vm: &mut crate::vm::Vm, _: usize) -> Result<usize, crate::vm::LuaError> {
    Err(vm.rt_error("attempt to call a dead function"))
}

fn is_collectable(v: Value) -> bool {
    matches!(
        v.unpack(),
        V::Str(_) | V::Table(_) | V::Func(_) | V::User(_)
    )
}

/// Hands out collection numbers for `Proto::gc_mark`.
static GC_EPOCH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Constants of `p` and its nested prototypes, once per collection.
fn mark_proto(
    p: &Rc<Proto>,
    gray: &mut Vec<Value>,
    epoch: u32,
    bytes: &mut usize,
    tally: &mut Tally,
) {
    if p.gc_mark.get() == epoch {
        return;
    }
    p.gc_mark.set(epoch);
    *bytes += p.bytes();
    tally.count_size(crate::proto::PROTO_BYTES);
    for (elem, count) in p.vectors() {
        tally.count(crate::pools::vector_block(elem, count));
    }
    gray.extend(p.consts.iter().copied().filter(|v| is_collectable(*v)));
    gray.extend_from_slice(&p.names);
    for q in &p.protos {
        mark_proto(q, gray, epoch, bytes, tally);
    }
}

/// Free every live object the mark left unmarked, and unmark the rest; answers the bytes that
/// stay.
///
/// The slots past the last live one are given back, and the free list is rebuilt so `alloc`
/// takes the lowest free slot first: new objects fill the bottom, and the next sweep's range
/// follows the live set instead of the most the arena ever held.
fn sweep<T: Dead>(
    a: &mut Arena<T>,
    mut size: impl FnMut(&T) -> usize,
    keep: impl Fn(&T) -> bool,
    mut kept: impl FnMut(T),
) -> usize {
    let mut live = 0;
    let n = a.used as usize;
    let (items, state) = (&mut a.items[..n], &mut a.state[..n]);
    let mut top = 0;
    for i in 0..n {
        match state[i] {
            FREE => {}
            MARKED => {
                state[i] = LIVE;
                live += size(&items[i]);
                top = i + 1;
            }
            _ => {
                if keep(&items[i]) {
                    kept(std::mem::replace(&mut items[i], T::dead()));
                } else {
                    items[i] = T::dead();
                }
                state[i] = FREE;
            }
        }
    }
    a.used = top as u32;
    a.free.clear();
    a.free.extend(
        (0..top as u32)
            .rev()
            .filter(|&i| a.state[i as usize] == FREE),
    );
    // Halve while a quarter or less is in use, so a spike's arena does not stay that size.
    let mut len = a.items.len();
    while len > ARENA_FIRST && top <= len / 4 {
        len /= 2;
    }
    if len < a.items.len() {
        a.items.truncate(len);
        a.items.shrink_to_fit();
        a.state.truncate(len);
        a.state.shrink_to_fit();
        a.mask = len - 1;
    }
    live
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What making `f`'s object adds to the count.
    fn charged<T>(h: &mut Heap, f: impl FnOnce(&mut Heap) -> T) -> usize {
        let before = h.bytes();
        let _ = f(h);
        h.bytes() - before
    }

    fn native(_: &mut crate::vm::Vm, _: usize) -> Result<usize, crate::vm::LuaError> {
        Ok(0)
    }

    /// Every object is counted at the size the 32-bit client's Lua 5.0 allocates it: a string 17
    /// and its length (`0x6f9da1`), a table 32 (`0x6fa500`) and 16 an array slot (`0x6fa579`) and
    /// 40 a node (`0x6fa613`), a Lua closure 36 and 4 an upvalue (`0x6f9ebf`), a C closure 16 and
    /// 16 an upvalue (`0x6f9e7c`), an upvalue 32 (`0x6f9f38`).
    #[test]
    fn objects_are_counted_at_the_clients_5_0_sizes() {
        let mut h = Heap::default();
        assert_eq!(charged(&mut h, |h| h.intern(b"hello")), 0x11 + 5);
        assert_eq!(charged(&mut h, |h| h.intern(b"hello")), 0, "interned once");
        assert_eq!(charged(&mut h, |h| h.intern(&[b'x'; 100])), 0x11 + 100);

        assert_eq!(charged(&mut h, |h| h.new_table(0, 0)), 0x20);
        assert_eq!(charged(&mut h, |h| h.new_table(4, 0)), 0x20 + 4 * 16);
        let mut t = TableRef(0);
        let n = charged(&mut h, |h| t = h.new_table(0, 4));
        let nodes = h.table(t).node.len();
        assert!(nodes >= 4);
        assert_eq!(n, 0x20 + 40 * nodes);

        let p = crate::compile::compile(
            b"local a, b, c function F() return a, b, c end",
            b"=t",
            &mut h,
        )
        .unwrap();
        let inner = p.protos[0].clone();
        assert_eq!(inner.upvals.len(), 3);
        let lua = FuncObj::Lua(LuaFunc::new(inner, Upvals::new(vec![0; 3]), TableRef(0)));
        assert_eq!(charged(&mut h, |h| h.new_func(lua)), 0x24 + 3 * 4);
        let c = |n: usize| FuncObj::Native {
            f: native,
            host: None,
            name: "c",
            upvals: vec![Value::Nil; n].into_boxed_slice(),
            env: TableRef(0),
        };
        assert_eq!(charged(&mut h, |h| h.new_func(c(0))), 16);
        assert_eq!(charged(&mut h, |h| h.new_func(c(2))), 48);
        assert_eq!(charged(&mut h, |h| h.new_upval(UpvalObj::Open(0))), 0x20);
    }

    /// A prototype's chunk name is the string `luaY_parser` takes by `strlen` (`0x6fc9c2`): up to
    /// its first NUL.
    #[test]
    fn a_chunk_name_is_counted_to_its_first_nul() {
        let mut h = Heap::default();
        let _p = crate::compile::compile(b"local x", b"=name\0tail", &mut h).unwrap();
        assert_eq!(
            charged(&mut h, |h| h.intern(b"=name")),
            0,
            "made by the compile"
        );
        assert_eq!(
            charged(&mut h, |h| h.intern(b"=name\0tail")),
            0x11 + 10,
            "never made"
        );
    }

    /// 5.0's concatenation buffer is counted (`[G+0x20]`): grown to the longest concatenation,
    /// 32 bytes at least (`luaZ_openspace`, `0x6fb5d1`), halved by each collection while over 64
    /// (`checkSizes`, `0x6f73a1`..`0x6f73cf`).
    #[test]
    fn the_concatenation_buffer_is_counted_as_5_0_sizes_it() {
        let mut vm = crate::new_state();
        vm.full_gc();
        let before = vm.heap.concat50;
        assert!(before < 1001);
        let n = {
            let b = vm.heap.bytes();
            vm.exec(b"S = string.rep('x', 1000) .. 'y'", b"=t").unwrap();
            vm.heap.bytes() - b
        };
        assert_eq!(vm.heap.concat50, 1001);
        assert!(n >= 1001 - before, "the buffer's growth is counted: {n}");
        for want in [500, 250, 125, 62, 62] {
            vm.full_gc();
            assert_eq!(vm.heap.concat50, want);
        }
        let mut h = Heap::default();
        assert_eq!(charged(&mut h, |h| h.concat_space(5)), 0x20);
        assert_eq!(charged(&mut h, |h| h.concat_space(20)), 0);
        assert_eq!(charged(&mut h, |h| h.concat_space(100)), 100 - 0x20);
    }

    /// A collection leaves the threshold `0x6fae00` reckons from the count, the old threshold
    /// and the pools' free space, on numbers worked by hand.
    #[test]
    fn a_collection_paces_by_the_old_headroom_the_pools_and_a_quarter() {
        let mut h = Heap::default();
        let mode = h.intern(b"__mode");
        // One block in each pool: a 16-byte userdata, strings of 18, 77 and 167 bytes, the
        // 34-byte `not enough memory`, and the 128-byte string table of 32 buckets.
        let keep = [
            Value::User(h.new_user(UserObj { meta: None, tag: 0 })),
            Value::Str(h.intern(b"x")),
            Value::Str(h.intern(&[b'y'; 60])),
            Value::Str(h.intern(&[b'z'; 150])),
        ];
        let count = 16 + 18 + 77 + 167 + 34 + 128;
        // Free after the collection: 8191 blocks of 16 bytes, 4095 of 32, 2047 of 64, 1364 of 96,
        // 1023 of 128 and 511 of 256, the least 130 816 bytes.
        let least = 511 * 256;
        for (old, next) in [
            // The headroom capped by the pools.
            (1 << 20, count + least),
            // The headroom, 160, over a quarter, 110.
            (600, 600),
            // A quarter over the headroom, 60.
            (500, count + count / 4),
            // `collectgarbage(0)`: a headroom wrapped past 4 GB, capped by the pools.
            (0, count + least),
        ] {
            h.set_threshold(old);
            assert_eq!(h.collect(&keep, &[], &[], mode, &[]), count);
            h.pace();
            assert_eq!(h.threshold(), next, "from {old}");
        }
        // With the userdata gone its pool keeps its chunk, all of it free now: 131 072 bytes,
        // still not the least.
        h.set_threshold(1 << 20);
        assert_eq!(h.collect(&keep[1..], &[], &[], mode, &[]), count - 16);
        h.pace();
        assert_eq!(h.threshold(), count - 16 + least);
    }

    /// A table's resizes take their new blocks and give the old back as they happen, so the
    /// pools' book between collections is what a collection's recount finds.
    #[test]
    fn a_tables_resizes_keep_the_pools_book_exact() {
        let mut h = Heap::default();
        let mode = h.intern(b"__mode");
        let keys: Vec<Value> = (0..40)
            .map(|i| h.str_value(format!("k{i}").as_bytes()))
            .collect();
        let before = h.pools().used();
        let t = h.new_table(0, 0);
        for (i, &k) in keys.iter().enumerate() {
            h.table_mut(t).set(k, Value::Num(i as f64)).unwrap();
        }
        for i in 1..=40 {
            h.table_mut(t)
                .set(Value::Num(i as f64), Value::Bool(true))
                .unwrap();
        }
        let _ = h.bytes();
        let grown = h.pools().used();
        let diff: Vec<i64> = (0..6).map(|i| grown[i] as i64 - before[i] as i64).collect();
        // The table's 32-byte record; its node vector and its array part, both grown through the
        // pools past 256 bytes, are on the system heap, and nothing they left stays in a pool.
        assert_eq!(diff, [0, 1, 0, 0, 0, 0]);
        let mut roots = keys.clone();
        roots.push(Value::Table(t));
        roots.push(Value::Str(mode));
        h.collect(&roots, &[], &[], mode, &[]);
        assert_eq!(h.pools().used(), grown, "the recount agrees");
    }

    /// A pool that has taken no chunk has no free space, so only a quarter is left above the
    /// count.
    #[test]
    fn a_collection_with_a_pool_never_used_leaves_a_quarter() {
        let mut h = Heap::default();
        let mode = h.intern(b"__mode");
        h.set_threshold(1 << 20);
        let count = h.collect(&[], &[], &[], mode, &[]);
        assert_eq!(count, 34 + 128);
        h.pace();
        assert_eq!(h.threshold(), count + count / 4);
    }

    /// The string table is counted as 5.0 sizes it, 4 bytes a bucket: 32 at open (`0x6f6e96`),
    /// doubled when the strings outnumber the buckets (`0x6f9e0f`), and halved by a collection
    /// that leaves more than 64 buckets under a quarter full (`0x6f738c`).
    #[test]
    fn the_string_table_is_counted_as_5_0_sizes_it() {
        let mut h = Heap::default();
        assert_eq!(h.strt_buckets, 32);
        // `not enough memory` is the first string; 31 more fill the 32 buckets.
        let mut keep = Vec::new();
        for i in 0..31 {
            let s = format!("s{i:02}");
            let n = charged(&mut h, |h| keep.push(Value::Str(h.intern(s.as_bytes()))));
            assert_eq!(n, 0x11 + 3);
        }
        let n = charged(&mut h, |h| keep.push(Value::Str(h.intern(b"s31"))));
        assert_eq!(n, 0x11 + 3 + 4 * 32, "the 33rd string doubles the buckets");
        assert_eq!(h.strt_buckets, 64);
        for i in 32..65 {
            h.intern(format!("s{i:02}").as_bytes());
        }
        assert_eq!(h.strt_buckets, 128);

        // A collection that keeps 33 strings leaves 128 buckets a quarter full; one that keeps
        // 11 halves them, once.
        let mode = h.intern(b"__mode");
        let live = h.collect(&keep, &[], &[], mode, &[]);
        assert_eq!(h.strt_buckets, 128);
        let kept: Vec<Value> = keep[..10].to_vec();
        let live_after = h.collect(&kept, &[], &[], mode, &[]);
        assert_eq!(h.strt_buckets, 64);
        // What lives: the ten, `not enough memory`, and the buckets.
        assert_eq!(live_after, 10 * (0x11 + 3) + (0x11 + 17) + 4 * 64);
        assert_eq!(h.bytes(), live_after);
        assert!(live > live_after);
    }

    /// A spike of garbage leaves no high-water mark: the arena shrinks back, and what is made
    /// after fills the bottom, so a sweep's range follows the live set.
    #[test]
    fn a_sweep_gives_back_a_spikes_slots() {
        let mut h = Heap::default();
        let keep = h.new_table(0, 0);
        for _ in 0..100_000 {
            h.new_table(0, 0);
        }
        let mode = h.intern(b"__mode");
        h.collect(&[Value::Table(keep)], &[], &[], mode, &[]);
        assert!(
            h.tables.arena.items.len() <= 4 * ARENA_FIRST,
            "the arena shrank"
        );
        let t = h.new_table(0, 0);
        assert!(
            (t.0 as usize) < ARENA_FIRST,
            "a new table takes a low slot, got {}",
            t.0
        );
        h.collect(&[Value::Table(keep), Value::Table(t)], &[], &[], mode, &[]);
        assert_eq!(h.table(keep).array.len(), 0, "the kept table is intact");
        assert!(h.tables.arena.used as usize <= ARENA_FIRST);
    }
}
