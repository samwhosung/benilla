//! A table as 5.1.5's `ltable.c` lays it out: an array part and a chained scatter hash part with
//! Brent's variation, sized and rehashed by 5.1's rules, as benilla's script host has always run
//! them, with the client's 5.0 hashes for strings and numbers. The layout is the iteration order,
//! so `next` walks keys in the order the layout gives (keys hashed by address excepted: a table,
//! function or userdata key is hashed by its handle here).

use crate::value::{TableRef, Value, V};

#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub key: Value,
    pub val: Value,
    /// The next node in this chain, or -1.
    pub next: i32,
    /// 5.1.5's `LUA_TDEADKEY`: a collection found the entry empty and its key collectable
    /// (`removeentry`). A write does not find the key here again, as `luaH_get` does not, so
    /// setting it takes a node by `newkey`'s rules; `next` still finds it, by identity.
    pub(crate) dead: bool,
}

const EMPTY: Node = Node {
    key: Value::Nil,
    val: Value::Nil,
    next: -1,
    dead: false,
};

/// A heap's bytes of table growth not yet in its count: shared by every table the heap made, so a
/// resize is counted where it happens, by the heap that made the table, and a write pays nothing
/// for it. A table made outside a heap counts nowhere.
#[derive(Clone, Debug, Default)]
pub(crate) struct Growth(pub(crate) Option<std::rc::Rc<std::cell::Cell<isize>>>);

impl Growth {
    pub(crate) const NONE: Growth = Growth(None);

    fn add(&self, n: isize) {
        if let Some(c) = &self.0 {
            c.set(c.get() + n);
        }
    }
}

/// 5.1's `MAXBITS`: `2^MAXBITS` (`MAXASIZE`) is the largest array index kept in the array part.
const MAXBITS: u32 = 26;

/// Aligned to a cache line, which makes it two lines long: the heap reaches a table by index,
/// and a power-of-two size turns that index into a shift instead of a multiply.
#[derive(Clone, Debug)]
#[repr(align(64))]
pub struct Table {
    pub array: Vec<Value>,
    /// The hash part: `2^lsizenode` nodes. A size of zero is 5.0's shared `dummynode`, kept
    /// here as one node of the table's own.
    pub node: Vec<Node>,
    pub lsizenode: u8,
    /// 5.1's `lastfree`: a free node is searched for below it, lazily.
    lastfree: usize,
    /// The hash part is 5.1's shared `dummynode`: its one node takes no key.
    dummy: bool,
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

/// 5.1's `ceillog2`: the ceiling of log2, 0 for one.
fn ceillog2(x: u32) -> u32 {
    if x <= 1 {
        0
    } else {
        32 - (x - 1).leading_zeros()
    }
}

/// 5.1's `countint` test: a key that may live in the array part, or `None`.
fn arrayindex(key: Value) -> Option<usize> {
    if let V::Num(n) = key.unpack() {
        if let Some(k) = num_to_int(n) {
            if k >= 1 && (k as u32) <= (1 << MAXBITS) {
                return Some(k as usize);
            }
        }
    }
    None
}

/// `next` was given a key the table does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidKey;

/// The error a write raises, which the VM renders with a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    Nil,
    NaN,
}

impl Table {
    /// `luaH_new`: `narray` array slots and room for `nhash` keys, rounded up to a power of two
    /// (none at 0).
    pub fn new(narray: usize, nhash: usize) -> Table {
        let mut t = Table {
            array: Vec::new(),
            node: Vec::new(),
            lsizenode: 0,
            lastfree: 0,
            dummy: true,
            meta: None,
            flags: 0,
            setn: None,
            weak: 0,
            owner: 0,
            hint_key: std::cell::Cell::new(Value::Nil),
            hint_pos: std::cell::Cell::new(0),
            growth: Growth::NONE,
        };
        t.array.resize(narray, Value::Nil);
        t.set_node_vector(nhash);
        t
    }

    /// What a heap slot with no table holds: no node at all, so it costs no allocation; a
    /// stale handle reading it panics on the missing node instead of finding a key.
    pub(crate) fn dead() -> Table {
        Table {
            array: Vec::new(),
            node: Vec::new(),
            lsizenode: 0,
            lastfree: 0,
            dummy: true,
            meta: None,
            flags: 0,
            setn: None,
            weak: 0,
            owner: 0,
            hint_key: std::cell::Cell::new(Value::Nil),
            hint_pos: std::cell::Cell::new(0),
            growth: Growth::NONE,
        }
    }

    /// 5.1's `setnodevector`: room for `size` keys, or the dummy node for none.
    fn set_node_vector(&mut self, size: usize) {
        self.node.clear();
        if size == 0 {
            self.node.push(EMPTY);
            self.lsizenode = 0;
            self.dummy = true;
            self.lastfree = 0;
            return;
        }
        let lsize = ceillog2(size as u32);
        let size = 1usize << lsize;
        self.node.resize(size, EMPTY);
        self.lsizenode = lsize as u8;
        self.dummy = false;
        self.lastfree = size;
    }

    /// 5.1's `getfreepos`: the highest never-used node below `lastfree`.
    fn getfreepos(&mut self) -> Option<usize> {
        while self.lastfree > 0 {
            self.lastfree -= 1;
            if self.node[self.lastfree].key.is_nil() {
                return Some(self.lastfree);
            }
        }
        None
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

    pub fn set_int(&mut self, k: i32, val: Value) {
        let s = match self.find_int(k) {
            Some(s) => s,
            None => self.newkey(Value::Num(k as f64)),
        };
        *self.slot_mut(s) = val;
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
            _ => Ok(self.newkey(key)),
        }
    }

    /// 5.1's `newkey`: Brent's variation; with no free node, a rehash counting the new key, then
    /// the key set into the grown table.
    fn newkey(&mut self, key: Value) -> Slot {
        // A new key may move another one (the colliding-node move).
        self.hint_key.set(Value::Nil);
        let mut mp = self.mainposition(key);
        if !self.node[mp].val.is_nil() || self.dummy {
            let Some(n) = self.getfreepos() else {
                self.rehash(key);
                return match self.find(key) {
                    Some(s) => s,
                    None => self.newkey(key),
                };
            };
            let othern0 = self.mainposition(self.node[mp].key);
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
        Slot::Node(mp)
    }

    /// 5.1's `rehash`: count the integer keys of both parts and the new key `ek`, size the array
    /// part by `computesizes`, and give the hash part room for the rest.
    fn rehash(&mut self, ek: Value) {
        let mut nums = [0i32; MAXBITS as usize + 1];
        // `numusearray`: slice `(2^(lg-1), 2^lg]` counted into `nums[lg]`.
        let mut nasize = 0i32;
        let mut i = 1usize;
        let mut lg = 0usize;
        let mut ttlg = 1usize;
        while lg <= MAXBITS as usize {
            let mut lim = ttlg;
            if lim > self.array.len() {
                lim = self.array.len();
                if i > lim {
                    break;
                }
            }
            let mut lc = 0;
            while i <= lim {
                if !self.array[i - 1].is_nil() {
                    lc += 1;
                }
                i += 1;
            }
            nums[lg] += lc;
            nasize += lc;
            lg += 1;
            ttlg *= 2;
        }
        let mut totaluse = nasize;
        // `numusehash`, last node first.
        let countint = |k: Value, nums: &mut [i32]| -> i32 {
            match arrayindex(k) {
                Some(k) => {
                    nums[ceillog2(k as u32) as usize] += 1;
                    1
                }
                None => 0,
            }
        };
        for n in self.node.iter().rev() {
            if !n.val.is_nil() {
                nasize += countint(n.key, &mut nums);
                totaluse += 1;
            }
        }
        nasize += countint(ek, &mut nums);
        totaluse += 1;
        // `computesizes`: the largest `n` with more than half of `1..n` in use.
        let (mut a, mut na, mut n) = (0i32, 0i32, 0i32);
        let mut i = 0usize;
        let mut twotoi = 1i32;
        while twotoi / 2 < nasize {
            if nums[i] > 0 {
                a += nums[i];
                if a > twotoi / 2 {
                    n = twotoi;
                    na = a;
                }
            }
            if a == nasize {
                break;
            }
            i += 1;
            twotoi *= 2;
        }
        self.resize(n as usize, (totaluse - na) as usize);
    }

    /// `OP_SETLIST`'s growth: an array part short of `last` slots is resized to `last` by 5.1's
    /// `luaH_resizearray`, the hash part kept at its size and rebuilt.
    #[inline]
    pub fn grow_array(&mut self, last: usize) {
        if last > self.array.len() {
            self.resize_array(last);
        }
    }

    #[cold]
    #[inline(never)]
    fn resize_array(&mut self, nasize: usize) {
        let nsize = if self.dummy { 0 } else { self.sizenode() };
        self.resize(nasize, nsize);
    }

    /// 5.1's `resize`.
    fn resize(&mut self, nasize: usize, nhsize: usize) {
        let before = self.bytes() as isize;
        self.resize_parts(nasize, nhsize);
        // 5.1's `luaM_reallocvector` charges `totalbytes` for both parts.
        self.growth.add(self.bytes() as isize - before);
    }

    fn resize_parts(&mut self, nasize: usize, nhsize: usize) {
        self.hint_key.set(Value::Nil);
        let oldasize = self.array.len();
        let nold = std::mem::take(&mut self.node);
        if nasize > oldasize {
            self.array.resize(nasize, Value::Nil);
        }
        self.set_node_vector(nhsize);
        if nasize < oldasize {
            let vanishing: Vec<Value> = self.array.drain(nasize..).collect();
            for (j, v) in vanishing.into_iter().enumerate() {
                if !v.is_nil() {
                    self.set_int((nasize + j + 1) as i32, v);
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
        if self.dummy {
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

    /// A byte estimate for the collector's accounting.
    pub fn bytes(&self) -> usize {
        64 + self.array.len() * 16 + self.node.len() * 40
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
        let mut t = Table::new(0, 4);
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
