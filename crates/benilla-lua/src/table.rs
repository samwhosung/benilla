//! A table as the client's Lua 5.0 `ltable.c` lays it out (`0x6fa1a0`..`0x6fad80`): an array part
//! and a chained scatter hash part with Brent's variation, sized, filled and rehashed by 5.0's
//! rules, with 5.0's hashes for strings and numbers. The layout is the iteration order, so `next`
//! walks keys in the order the client's would (keys hashed by address excepted: a table, function
//! or userdata key is hashed by its handle here).

use crate::pools::{Block, Pools};
use crate::value::{TableRef, Value, V};

#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub key: Value,
    pub val: Value,
    /// The next node in this chain, or -1.
    pub next: i32,
    /// The key's object was freed while the entry was empty, which 5.0 leaves as it is, its
    /// pointer dangling. A new object may take the handle, so a write does not find the key here
    /// and takes a node by `newkey`'s rules; `next` still finds it, by identity. A weak table's
    /// clearing instead removes the key ([`Table::remove_node`]).
    pub(crate) dead: bool,
}

const EMPTY: Node = Node {
    key: Value::Nil,
    val: Value::Nil,
    next: -1,
    dead: false,
};

/// What a heap's tables add to its count as they resize: the bytes not yet in it, and the
/// client's block pools ([`crate::pools`]), which a resize takes blocks from and gives them back to
/// as it happens.
#[derive(Debug, Default)]
pub(crate) struct Account {
    pub(crate) growth: std::cell::Cell<isize>,
    pub(crate) pools: std::cell::RefCell<Pools>,
}

impl Account {
    /// A copy of this account, for a forked heap.
    pub(crate) fn fork(&self) -> Account {
        Account {
            growth: std::cell::Cell::new(self.growth.get()),
            pools: std::cell::RefCell::new(self.pools.borrow().clone()),
        }
    }
}

/// A heap's [`Account`]: shared by every table the heap made, so a resize is counted where it
/// happens, by the heap that made the table, and a write pays nothing for it. A table made outside
/// a heap counts nowhere.
#[derive(Clone, Debug, Default)]
pub(crate) struct Growth(pub(crate) Option<std::rc::Rc<Account>>);

impl Growth {
    pub(crate) const NONE: Growth = Growth(None);
}

/// 5.0's `MAXBITS`: `2^MAXBITS` is the largest array index (`arrayindex`'s bound, `0x6fa4d0`) and
/// the largest log node count (`setnodevector`, `0x6fa5e2`).
const MAXBITS: u32 = 24;

/// Aligned to a cache line, which makes it two lines long: the heap reaches a table by index,
/// and a power-of-two size turns that index into a shift instead of a multiply.
#[derive(Clone, Debug)]
#[repr(align(64))]
pub struct Table {
    pub array: Vec<Value>,
    /// The hash part: `2^lsizenode` nodes. A log size of 0 is 5.0's shared `dummynode`, kept
    /// here as one node of the table's own: `newkey` writes a key into it and rehashes at once.
    pub node: Vec<Node>,
    pub lsizenode: u8,
    /// 5.0's `firstfree`: no node above it has a nil key, and itself has one until the table
    /// is full, which `newkey` answers with a rehash.
    firstfree: usize,
    pub meta: Option<TableRef>,
    /// 5.0's `flags`: a set bit caches "this metamethod is absent" (`luaT_gettm`).
    pub flags: u8,
    /// The size `table.setn` remembered, 5.0's `n`-less `getn` (the client's `getsizes` store,
    /// `0x6f4fc0`), negative as given.
    pub setn: Option<i32>,
    /// The weak mode the last collection found: bit 0 keys, bit 1 values.
    pub(crate) weak: u8,
    /// The chunk whose code made this table (`Heap::sources`), for a memory census.
    pub owner: u32,
    /// The key `next` last returned and its position: a traversal's next step starts there
    /// without looking the key up. Cleared by anything that moves a key (a new key, a resize),
    /// so a step after such a change finds the key where 5.0 would.
    hint_key: std::cell::Cell<Value>,
    hint_pos: std::cell::Cell<u32>,
    /// Where its resizes are counted, as C's allocator counts each one.
    pub(crate) growth: Growth,
    /// Where the client's allocator keeps the array part's block: a vector `setarrayvector`
    /// reallocates (`0x6fa584`), so where it lives follows the sizes it has had.
    pub(crate) array_block: Block,
}

/// The result of a key lookup: where its slot is, if it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Array(usize),
    Node(usize),
}

/// `lua_number2int`: a value that is exactly an `i32` converts; anything else fails the
/// `(lua_Number)k == n` test that follows every use.
pub fn num_to_int(n: f64) -> Option<i32> {
    let k = n as i32;
    if k as f64 == n {
        Some(k)
    } else {
        None
    }
}

/// `luaO_log2` (`0x6f5830`): the floor of log2, -1 for 0.
fn log2(x: u32) -> i32 {
    if x == 0 {
        -1
    } else {
        31 - x.leading_zeros() as i32
    }
}

/// The log node count 5.0 gives a hash part for `n` keys, `luaO_log2(n) + 1`: the constructor's
/// `OP_NEWTABLE` hint (`0x6fd563`, `0x6fd568`) and `rehash`'s size (`0x6fa9dd`, `0x6fa9e5`). It is
/// one power of two above the count, so `2^k` keys sit in `2^(k+1)` nodes, and none in the dummy.
pub fn lsize_for(n: usize) -> u8 {
    (log2(n.min(u32::MAX as usize) as u32) + 1) as u8
}

/// 5.0's `arrayindex` (`0x6fa4a0`): a key that may live in the array part, 1 to 2^24, or `None`.
fn arrayindex(key: Value) -> Option<usize> {
    if let V::Num(n) = key.unpack() {
        if let Some(k) = num_to_int(n) {
            if k >= 1 && ((k - 1) >> MAXBITS) == 0 {
                return Some(k as usize);
            }
        }
    }
    None
}

/// 5.0's `computesizes` (`0x6fab00`): the array part is the `2^n` of the last slice at which
/// the integer keys up to it reach half of it, `a >= 2^(n-1)` (`0x6fab45`), 0 for none (`0x6fab69`);
/// the rest go to the hash part. `nums[i]` counts the keys in `(2^(i-1), 2^i]`, `nums[0]` key 1;
/// `narray` counts every integer key, `ntotal` every live entry.
fn computesizes(nums: &[usize], ntotal: usize, narray: usize) -> (usize, usize) {
    let mut a = nums[0];
    let mut na = a;
    let mut n: Option<usize> = (na != 0).then_some(0);
    let mut i = 1;
    while a < narray && i < nums.len() && narray >= 1 << (i - 1) {
        if nums[i] > 0 {
            a += nums[i];
            if a >= 1 << (i - 1) {
                n = Some(i);
                na = a;
            }
        }
        i += 1;
    }
    (n.map_or(0, |n| 1 << n), ntotal - na)
}

/// `next` was given a key the table does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidKey;

/// The error a write raises, which the VM renders with a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    Nil,
    NaN,
    /// `table overflow` (`0x871af8`): a hash part past `2^MAXBITS` nodes (`setnodevector`,
    /// `0x6fa5e2`).
    Overflow,
}

impl KeyError {
    /// The message the client raises for it.
    pub fn message(self) -> &'static str {
        match self {
            KeyError::Nil => "table index is nil",
            KeyError::NaN => "table index is NaN",
            KeyError::Overflow => "table overflow",
        }
    }
}

/// `setnodevector`'s bound (`0x6fa5e2`): a log node count above `MAXBITS` raises `table
/// overflow`.
pub fn check_lsize(lsize: u8) -> Result<u8, KeyError> {
    if u32::from(lsize) > MAXBITS {
        Err(KeyError::Overflow)
    } else {
        Ok(lsize)
    }
}

impl Table {
    /// `luaH_new` (`0x6fa4f0`): `narray` array slots and `2^lnhash` nodes, the dummy node for 0.
    /// [`lsize_for`] turns a key count into the log 5.0 would give it.
    pub fn new(narray: usize, lnhash: u8) -> Table {
        let mut t = Table {
            array: Vec::new(),
            node: Vec::new(),
            lsizenode: 0,
            firstfree: 0,
            meta: None,
            flags: 0,
            setn: None,
            weak: 0,
            owner: 0,
            hint_key: std::cell::Cell::new(Value::Nil),
            hint_pos: std::cell::Cell::new(0),
            growth: Growth::NONE,
            array_block: Block::None,
        };
        t.array.resize(narray, Value::Nil);
        t.set_node_vector(lnhash);
        t
    }

    /// What a heap slot with no table holds: no node at all, so it costs no allocation; a
    /// stale handle reading it panics on the missing node instead of finding a key.
    pub(crate) fn dead() -> Table {
        Table {
            array: Vec::new(),
            node: Vec::new(),
            lsizenode: 0,
            firstfree: 0,
            meta: None,
            flags: 0,
            setn: None,
            weak: 0,
            owner: 0,
            hint_key: std::cell::Cell::new(Value::Nil),
            hint_pos: std::cell::Cell::new(0),
            growth: Growth::NONE,
            array_block: Block::None,
        }
    }

    /// 5.0's `setnodevector` (`0x6fa5d0`): `2^lsize` nil nodes, the dummy node for 0, and
    /// `firstfree` at the last one (`0x6fa67c`).
    fn set_node_vector(&mut self, lsize: u8) {
        debug_assert!(check_lsize(lsize).is_ok(), "a log node count past MAXBITS");
        let size = 1usize << lsize;
        self.node.clear();
        self.node.resize(size, EMPTY);
        self.lsizenode = lsize;
        self.firstfree = size - 1;
    }

    /// 5.0's `removekey` (`0x6f79d0`), a weak table's clearing: the value cleared and a
    /// collectable key retagged `LUA_TNONE`, so no lookup finds it, `next` included
    /// (``invalid key for `next'``, `0x6fa467`). The node stays taken until a rehash.
    pub(crate) fn remove_node(&mut self, i: usize) {
        let n = &mut self.node[i];
        n.val = Value::Nil;
        if matches!(
            n.key.unpack(),
            V::Str(_) | V::Table(_) | V::Func(_) | V::User(_)
        ) {
            n.key = Value::NONE_KEY;
            n.dead = false;
        }
        self.hint_key.set(Value::Nil);
    }

    /// The hash part is the dummy node: 5.0 shares one, which never keeps a key.
    fn is_dummy(&self) -> bool {
        self.lsizenode == 0
    }

    fn sizenode(&self) -> usize {
        1 << self.lsizenode
    }

    /// `hashmod`: modulo an odd number, for numbers and pointers.
    fn hashmod(&self, h: u32) -> usize {
        (h % (((self.sizenode() - 1) as u32) | 1)) as usize
    }

    /// `luaH_mainposition`.
    fn mainposition(&self, key: Value) -> usize {
        let mask = self.sizenode() - 1;
        match key.unpack() {
            V::Num(n) => {
                // The client's `hashnum` (`0x6fa260`), 5.0's: the key plus 1.0 (`0x6fa267`),
                // which also folds -0 into 0, then its two 32-bit words summed (`0x6fa27e`).
                let bits = (n + 1.0).to_bits();
                let h = (bits as u32).wrapping_add((bits >> 32) as u32);
                self.hashmod(h)
            }
            V::Str(s) => s.hash as usize & mask,
            V::Bool(b) => b as usize & mask,
            V::Table(r) => self.hashmod(r.0),
            V::Func(r) => self.hashmod(r.0),
            V::User(r) => self.hashmod(r.0),
            V::Light(p) => self.hashmod(p as u32),
            V::Nil => 0,
        }
    }

    /// `luaH_get`: the slot holding `key`, which may hold `nil`.
    pub fn find(&self, key: Value) -> Option<Slot> {
        match key.unpack() {
            V::Nil => None,
            V::Num(n) => match num_to_int(n) {
                Some(k) => self.find_int(k),
                None => self.find_chain(key),
            },
            V::Str(s) => {
                let mut i = s.hash as usize & (self.sizenode() - 1);
                loop {
                    let n = &self.node[i];
                    // One string, one bit pattern: id and hash bits alike.
                    if n.key == key {
                        return Some(Slot::Node(i));
                    }
                    if n.next < 0 {
                        return None;
                    }
                    i = n.next as usize;
                }
            }
            _ => self.find_chain(key),
        }
    }

    /// `luaH_getnum`.
    pub fn find_int(&self, k: i32) -> Option<Slot> {
        if k >= 1 && (k as usize) <= self.array.len() {
            return Some(Slot::Array(k as usize - 1));
        }
        let nk = k as f64;
        let mut i = self.mainposition(Value::Num(nk));
        loop {
            let n = &self.node[i];
            if n.key.as_num() == Some(nk) {
                return Some(Slot::Node(i));
            }
            if n.next < 0 {
                return None;
            }
            i = n.next as usize;
        }
    }

    fn find_chain(&self, key: Value) -> Option<Slot> {
        let mut i = self.mainposition(key);
        loop {
            let n = &self.node[i];
            if n.key.raw_eq(key) {
                return Some(Slot::Node(i));
            }
            if n.next < 0 {
                return None;
            }
            i = n.next as usize;
        }
    }

    pub fn slot(&self, s: Slot) -> Value {
        match s {
            Slot::Array(i) => self.array[i],
            Slot::Node(i) => self.node[i].val,
        }
    }

    pub fn slot_mut(&mut self, s: Slot) -> &mut Value {
        match s {
            Slot::Array(i) => &mut self.array[i],
            Slot::Node(i) => &mut self.node[i].val,
        }
    }

    /// A raw read.
    #[inline]
    pub fn get(&self, key: Value) -> Value {
        if let Some(i) = self.array_index(key) {
            return self.array[i];
        }
        self.find(key).map_or(Value::Nil, |s| self.slot(s))
    }

    /// `get` for a key known to be a string: the hash chain alone.
    #[inline(always)]
    pub fn get_str(&self, key: Value) -> Value {
        let mut i = key.str_hash_bits() as usize & (self.node.len() - 1);
        loop {
            let n = &self.node[i];
            if n.key == key {
                return n.val;
            }
            if n.next < 0 {
                return Value::Nil;
            }
            i = n.next as usize;
        }
    }

    /// [`Table::get`] with the key read in place.
    #[inline(always)]
    pub fn get_ref(&self, key: &Value) -> Value {
        match (*key).unpack() {
            V::Num(n) => {
                let k = n as i32;
                let i = (k as usize).wrapping_sub(1);
                if k as f64 == n && i < self.array.len() {
                    return self.array[i];
                }
            }
            V::Str(s) => {
                let mut i = s.hash as usize & (self.node.len() - 1);
                loop {
                    let n = &self.node[i];
                    if n.key == *key {
                        return n.val;
                    }
                    if n.next < 0 {
                        return Value::Nil;
                    }
                    i = n.next as usize;
                }
            }
            _ => {}
        }
        self.get(*key)
    }

    /// The array slot an integer key lands in, if the array part holds it.
    #[inline]
    fn array_index(&self, key: Value) -> Option<usize> {
        if let V::Num(n) = key.unpack() {
            let k = n as i32;
            let i = (k as usize).wrapping_sub(1);
            if k as f64 == n && i < self.array.len() {
                return Some(i);
            }
        }
        None
    }

    /// `luaH_getnum` then the read: the array part inline, the hash part out of line.
    #[inline]
    pub fn get_int(&self, k: i32) -> Value {
        // `k` below 1 wraps past any array length.
        let i = (k as usize).wrapping_sub(1);
        if i < self.array.len() {
            return self.array[i];
        }
        self.get_int_hash(k)
    }

    #[inline(never)]
    fn get_int_hash(&self, k: i32) -> Value {
        self.find_int(k).map_or(Value::Nil, |s| self.slot(s))
    }

    /// `luaH_set` then the store: a raw write.
    #[inline]
    pub fn set(&mut self, key: Value, val: Value) -> Result<(), KeyError> {
        if let Some(i) = self.array_index(key) {
            self.flags = 0;
            self.array[i] = val;
            return Ok(());
        }
        let s = self.slot_for(key)?;
        *self.slot_mut(s) = val;
        Ok(())
    }

    pub fn set_int(&mut self, k: i32, val: Value) -> Result<(), KeyError> {
        let s = match self.find_int(k) {
            Some(s) => s,
            None => self.newkey(Value::Num(k as f64))?,
        };
        *self.slot_mut(s) = val;
        Ok(())
    }

    /// `luaH_set`: the slot for `key`, made if absent. 5.0 makes a key even to store `nil`. A
    /// key found keeps its bits (`t[0]` finds a `-0` key and leaves it `-0`); a dead one is not
    /// found.
    pub fn slot_for(&mut self, key: Value) -> Result<Slot, KeyError> {
        self.flags = 0;
        match self.find(key) {
            Some(Slot::Node(i)) if self.node[i].dead => {}
            Some(s) => return Ok(s),
            None => {}
        }
        match key.unpack() {
            V::Nil => Err(KeyError::Nil),
            V::Num(n) if n.is_nan() => Err(KeyError::NaN),
            _ => self.newkey(key),
        }
    }

    /// 5.0's `newkey` (`0x6fa8a0`): Brent's variation, the free node `firstfree`. The key is
    /// written first; then `firstfree` walks down to a node with a nil key (`0x6fa95e`), and
    /// when there is none the table is full: the new key holds `false` while `rehash` counts and
    /// moves it with the rest (`0x6fa974`, `0x6fa98c`), and is found again in the new layout
    /// (`0x6fa995`).
    fn newkey(&mut self, key: Value) -> Result<Slot, KeyError> {
        // A new key may move another one (the colliding-node move), and a rehash moves all.
        self.hint_key.set(Value::Nil);
        let mut mp = self.mainposition(key);
        if !self.node[mp].val.is_nil() {
            let othern0 = self.mainposition(self.node[mp].key);
            let n = self.firstfree;
            if othern0 != mp {
                // The colliding node is out of its main position: move it to the free node.
                let mut othern = othern0;
                while self.node[othern].next != mp as i32 {
                    othern = self.node[othern].next as usize;
                }
                self.node[othern].next = n as i32;
                self.node[n] = self.node[mp];
                self.node[mp].next = -1;
                self.node[mp].val = Value::Nil;
            } else {
                // The colliding node is in its own main position: the new key takes the free node.
                self.node[n].next = self.node[mp].next;
                self.node[mp].next = n as i32;
                mp = n;
            }
        }
        self.node[mp].key = key;
        self.node[mp].dead = false;
        loop {
            if self.node[self.firstfree].key.is_nil() {
                return Ok(Slot::Node(mp));
            }
            if self.firstfree == 0 {
                break;
            }
            self.firstfree -= 1;
        }
        self.node[mp].val = Value::Bool(false);
        // A `table overflow` leaves the key in the table holding `false`, as the client's.
        self.rehash()?;
        let s = self.find(key).expect("a rehash keeps the new key");
        *self.slot_mut(s) = Value::Nil;
        Ok(s)
    }

    /// 5.0's `rehash` (`0x6fa9c0`): the sizes `numuse` counts, the hash part `luaO_log2(n) + 1`
    /// for `n` keys (`0x6fa9e5`).
    fn rehash(&mut self) -> Result<(), KeyError> {
        let (nasize, nhsize) = self.numuse();
        self.resize_to(nasize, nhsize)
    }

    /// `rehash`'s tail: the parts resized to `nasize` slots and room for `nhsize` keys.
    fn resize_to(&mut self, nasize: usize, nhsize: usize) -> Result<(), KeyError> {
        let lsize = lsize_for(nhsize);
        if check_lsize(lsize).is_err() {
            // `resize` grows the array part (`0x6fac1f`) before `setnodevector` raises.
            if nasize > self.array.len() {
                let before = self.bytes() as isize;
                self.array.resize(nasize, Value::Nil);
                if let Some(acct) = &self.growth.0 {
                    acct.growth
                        .set(acct.growth.get() + self.bytes() as isize - before);
                    let mut p = acct.pools.borrow_mut();
                    self.array_block = p.realloc(self.array_block, 16 * nasize);
                }
            }
            return Err(KeyError::Overflow);
        }
        self.resize(nasize, lsize);
        Ok(())
    }

    /// 5.0's `numuse` (`0x6faa00`): the live entries, those that are integer keys counted per
    /// slice `(2^(lg-1), 2^lg]`, the array part by slices then the nodes last first, and the
    /// array and hash sizes `computesizes` makes of them.
    fn numuse(&self) -> (usize, usize) {
        let mut nums = [0usize; MAXBITS as usize + 1];
        let mut totaluse = 0;
        let sizearray = self.array.len();
        let mut i = 0;
        for (lg, num) in nums.iter_mut().enumerate() {
            let mut ttlg = 1usize << lg;
            if ttlg > sizearray {
                ttlg = sizearray;
                if i >= ttlg {
                    break;
                }
            }
            while i < ttlg {
                if !self.array[i].is_nil() {
                    *num += 1;
                    totaluse += 1;
                }
                i += 1;
            }
        }
        let mut narray = totaluse;
        for n in self.node.iter().rev() {
            if !n.val.is_nil() {
                if let Some(k) = arrayindex(n.key) {
                    nums[(log2(k as u32 - 1) + 1) as usize] += 1;
                    narray += 1;
                }
                totaluse += 1;
            }
        }
        computesizes(&nums, totaluse, narray)
    }

    /// 5.0's `resize` (`0x6fab90`).
    fn resize(&mut self, nasize: usize, lnhsize: u8) {
        let before = self.bytes() as isize;
        let (oldasize, oldnodes) = (self.array.len(), self.node_count());
        self.resize_parts(nasize, lnhsize);
        let Some(acct) = &self.growth.0 else { return };
        // `luaM_irealloc` (`0x6fc980`) counts the change in both parts.
        acct.growth
            .set(acct.growth.get() + self.bytes() as isize - before);
        // A growing array part reallocated first (`0x6fac1f`), the new node vector made
        // (`0x6fac2f`), a shrinking array part reallocated after it, and the old node vector
        // freed last (`0x6fad5e`).
        let mut p = acct.pools.borrow_mut();
        if nasize > oldasize {
            self.array_block = p.realloc(self.array_block, 16 * nasize);
        }
        p.alloc(40 * self.node_count());
        if nasize < oldasize {
            self.array_block = p.realloc(self.array_block, 16 * nasize);
        }
        p.free(Pools::place(40 * oldnodes));
    }

    /// The nodes the client's allocator holds for the hash part: none for the shared dummy node.
    pub(crate) fn node_count(&self) -> usize {
        if self.is_dummy() {
            0
        } else {
            self.node.len()
        }
    }

    /// The array grows first (`0x6fac1f`), then the new node vector is made (`0x6fac2f`); a
    /// shrinking array's vanishing slice goes into it by `luaH_setnum`, lowest index first
    /// (`0x6fac63`), then the old nodes by `luaH_set`, last node first (`0x6facfd`). That order
    /// is the new layout.
    fn resize_parts(&mut self, nasize: usize, lnhsize: u8) {
        self.hint_key.set(Value::Nil);
        let oldasize = self.array.len();
        let nold = std::mem::take(&mut self.node);
        if nasize > oldasize {
            self.array.resize(nasize, Value::Nil);
        }
        self.set_node_vector(lnhsize);
        if nasize < oldasize {
            let vanishing: Vec<Value> = self.array.drain(nasize..).collect();
            for (j, v) in vanishing.into_iter().enumerate() {
                if !v.is_nil() {
                    self.set_int((nasize + j + 1) as i32, v)
                        .expect("the new hash part has room for every key");
                }
            }
        }
        for old in nold.iter().rev() {
            if !old.val.is_nil() {
                // A live old key cannot be nil or NaN.
                let s = self.slot_for(old.key).unwrap();
                *self.slot_mut(s) = old.val;
            }
        }
    }

    /// 5.0's `luaH_next`: the pair after `key`, `Ok(None)` at the end, `Err` for a key not in the
    /// table (`invalid key for 'next'`).
    pub fn next(&self, key: Value) -> Result<Option<(Value, Value)>, InvalidKey> {
        // `luaH_index`, or where the last step stopped.
        let mut i: i64 = match key {
            Value::Nil => -1,
            k if k == self.hint_key.get() => self.hint_pos.get() as i64,
            _ => match arrayindex(key) {
                Some(k) if k <= self.array.len() => k as i64 - 1,
                _ => match self.find(key) {
                    Some(Slot::Node(n)) => (n + self.array.len()) as i64,
                    Some(Slot::Array(a)) => a as i64,
                    None => return Err(InvalidKey),
                },
            },
        };
        i += 1;
        while (i as usize) < self.array.len() {
            let v = self.array[i as usize];
            if !v.is_nil() {
                let k = Value::Num((i + 1) as f64);
                self.hint_key.set(k);
                self.hint_pos.set(i as u32);
                return Ok(Some((k, v)));
            }
            i += 1;
        }
        let mut n = i as usize - self.array.len();
        while n < self.node.len() {
            let node = &self.node[n];
            if !node.val.is_nil() {
                self.hint_key.set(node.key);
                self.hint_pos.set((n + self.array.len()) as u32);
                return Ok(Some((node.key, node.val)));
            }
            n += 1;
        }
        Ok(None)
    }

    /// 5.0's `luaH_getn` has no such thing; this is the `getn` fallback scan the base library's
    /// `aux_getn` runs when no `n` is set: the count of the array prefix up to the first nil.
    pub fn border(&self) -> usize {
        let mut n = 0;
        while !self.get_int(n as i32 + 1).is_nil() {
            n += 1;
        }
        n
    }

    /// 5.1's `luaH_getn`, the `lua_objlen` 5.1's table library reads: a border found by binary
    /// search in the array part, or past it by `unbound_search`. On a table with holes it need
    /// not be the first one.
    pub fn length(&self) -> usize {
        let mut j = self.array.len();
        if j > 0 && self.array[j - 1].is_nil() {
            let mut i = 0;
            while j - i > 1 {
                let m = (i + j) / 2;
                if self.array[m - 1].is_nil() {
                    j = m;
                } else {
                    i = m;
                }
            }
            return i;
        }
        if self.is_dummy() {
            return j;
        }
        // `unbound_search`.
        let get = |k: usize| self.get(Value::Num(k as f64));
        let mut i = j;
        let mut j = j + 1;
        while !get(j).is_nil() {
            i = j;
            j *= 2;
            if j > i32::MAX as usize {
                // A pathological table: a linear count.
                let mut i = 1;
                while !get(i).is_nil() {
                    i += 1;
                }
                return i - 1;
            }
        }
        while j - i > 1 {
            let m = (i + j) / 2;
            if get(m).is_nil() {
                j = m;
            } else {
                i = m;
            }
        }
        i
    }

    /// The table in the collector's count: 5.0's 0x20-byte `Table` (`luaH_new`, `0x6fa500`), 16
    /// bytes an array slot (`setarrayvector`, `0x6fa579`) and 40 a node (`setnodevector`,
    /// `0x6fa613`), none for the shared dummy node (`0x6fa601`).
    pub fn bytes(&self) -> usize {
        0x20 + self.array.len() * 16 + self.node_count() * 40
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::StrRef;

    fn n(x: f64) -> Value {
        Value::Num(x)
    }

    #[test]
    fn integer_keys_move_to_the_array_part_on_rehash() {
        let mut t = Table::new(0, 0);
        for i in 1..=10 {
            t.set(n(i as f64), n(i as f64 * 10.0)).unwrap();
        }
        assert!(t.array.len() >= 8, "array part: {}", t.array.len());
        for i in 1..=10 {
            assert!(t.get(n(i as f64)).raw_eq(n(i as f64 * 10.0)));
        }
    }

    #[test]
    fn next_visits_every_key_once() {
        let mut t = Table::new(0, 0);
        for i in 0..200u32 {
            let k = if i % 3 == 0 {
                Value::Str(StrRef {
                    id: i,
                    hash: i.wrapping_mul(2654435761) & crate::value::STR_HASH_MASK,
                })
            } else {
                n(i as f64 * 0.5)
            };
            t.set(k, n(i as f64)).unwrap();
        }
        let mut seen = 0;
        let mut k = Value::Nil;
        while let Some((nk, _)) = t.next(k).unwrap() {
            seen += 1;
            k = nk;
        }
        assert_eq!(seen, 200);
    }

    /// The client's `hashnum` (`0x6fa260`) hashes the key plus 1.0: in four nodes 10 and 20 both
    /// start at node 0 and 30 at node 1, so the walk is 10, 30, then 20 from the free node.
    #[test]
    fn number_keys_take_the_clients_main_positions() {
        let mut t = Table::new(0, 2);
        assert_eq!(t.mainposition(n(10.0)), 0);
        assert_eq!(t.mainposition(n(20.0)), 0);
        assert_eq!(t.mainposition(n(30.0)), 1);
        assert_eq!(t.mainposition(n(-1.0)), 0);
        assert_eq!(t.mainposition(n(-0.0)), t.mainposition(n(0.0)));
        for k in [10.0, 20.0, 30.0] {
            t.set(n(k), n(k)).unwrap();
        }
        let mut order = Vec::new();
        let mut k = Value::Nil;
        while let Some((nk, _)) = t.next(k).unwrap() {
            order.push(nk.as_num().unwrap());
            k = nk;
        }
        assert_eq!(order, [10.0, 30.0, 20.0]);
    }

    /// `setnodevector` (`0x6fa5e2`) raises past a log of 24, after `resize` grew the array part
    /// (`0x6fac1f`) and before it touched the nodes.
    #[test]
    fn a_hash_part_past_maxbits_is_a_table_overflow() {
        assert_eq!(check_lsize(lsize_for((1 << 24) - 1)), Ok(24));
        assert_eq!(check_lsize(lsize_for(1 << 24)), Err(KeyError::Overflow));
        assert_eq!(KeyError::Overflow.message(), "table overflow");
        let mut t = Table::new(0, 1);
        t.set(n(0.5), n(1.0)).unwrap();
        assert_eq!(t.resize_to(4, 1 << 24), Err(KeyError::Overflow));
        assert_eq!((t.array.len(), t.lsizenode), (4, 1));
        assert!(t.get(n(0.5)).raw_eq(n(1.0)));
    }

    #[test]
    fn deleted_keys_stay_valid_for_next() {
        let mut t = Table::new(0, 0);
        for i in 0..20 {
            t.set(n(i as f64 + 0.5), n(1.0)).unwrap();
        }
        let mut k = Value::Nil;
        let mut count = 0;
        while let Some((nk, _)) = t.next(k).unwrap() {
            // Clearing the current key during a traversal is allowed.
            t.set(nk, Value::Nil).unwrap();
            k = nk;
            count += 1;
        }
        assert_eq!(count, 20);
    }
}
