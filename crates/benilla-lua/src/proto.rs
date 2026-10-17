//! Compiled functions: 5.1.5's register machine, with each test fused with the jump after it
//! and the common operand shapes given ops of their own.

/// Byte strings the compiler, the heap and hot reload share: names, sources, function keys.
pub type Bytes = std::rc::Rc<[u8]>;

use std::cell::Cell;
use std::rc::Rc;

use crate::value::Value;

pub type Reg = u8;
/// A register below [`RK_CONST`], a constant index at or above it.
pub type Rk = u16;
pub const RK_CONST: u16 = 256;

/// 5.1's `MAXSTACK`: a function uses fewer registers than this.
pub const MAXSTACK: usize = 250;
/// 5.1's `LFIELDS_PER_FLUSH`: list items a constructor stores per `SetList`.
pub const FIELDS_PER_FLUSH: u32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Lt,
    Le,
}

/// A comparison and the result that jumps, in one byte, so a compare-and-jump fits in an
/// eight-byte instruction.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CmpJ(u8);

impl CmpJ {
    pub fn new(cmp: Cmp, cond: bool) -> CmpJ {
        CmpJ((cmp as u8) << 1 | cond as u8)
    }

    #[inline(always)]
    pub fn cmp(self) -> Cmp {
        match self.0 >> 1 {
            0 => Cmp::Eq,
            1 => Cmp::Lt,
            _ => Cmp::Le,
        }
    }

    #[inline(always)]
    pub fn cond(self) -> bool {
        self.0 & 1 != 0
    }
}

impl std::fmt::Debug for CmpJ {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}={}", self.cmp(), self.cond())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

#[derive(Clone, Copy, Debug, Hash)]
pub enum Op {
    Move(Reg, Reg),
    LoadK(Reg, u32),
    /// `a..=b` become nil.
    LoadNil(Reg, Reg),
    LoadBool(Reg, bool),
    GetUpval(Reg, u16),
    SetUpval(Reg, u16),
    GetGlobal(Reg, u32),
    SetGlobal(Reg, u32),
    GetTable(Reg, Reg, Rk),
    SetTable(Reg, Rk, Rk),
    /// `R(a) = R(b)[K(c)]` for a string constant key: `GetTable` without the key's type test.
    GetField(Reg, Reg, u16),
    /// `R(a)[K(b)] = RK(c)` for a string constant key.
    SetField(Reg, u16, Rk),
    /// `R(a) = {}` with an array part and a hash part of `2^c` nodes.
    NewTable(Reg, u32, u8),
    /// `R(a+1) = R(b); R(a) = R(b)[RK(c)]`.
    SelfOp(Reg, Reg, Rk),
    /// `SelfOp` for a string constant name: the method call `o:m()`.
    SelfField(Reg, Reg, u16),
    /// `R(a) = R(b) op R(c)`. A constant operand has its own ops (`AddK`, `AddKR`).
    Add(Reg, Reg, Reg),
    Sub(Reg, Reg, Reg),
    Mul(Reg, Reg, Reg),
    Div(Reg, Reg, Reg),
    Pow(Reg, Rk, Rk),
    /// `R(a) = R(b) op K(c)`: the common `i + 1`, without the register-or-constant test.
    AddK(Reg, Reg, u16),
    SubK(Reg, Reg, u16),
    MulK(Reg, Reg, u16),
    DivK(Reg, Reg, u16),
    /// `R(a) = K(b) op R(c)`: the common `2 * i`, operands in source order.
    AddKR(Reg, u16, Reg),
    SubKR(Reg, u16, Reg),
    MulKR(Reg, u16, Reg),
    DivKR(Reg, u16, Reg),
    Unm(Reg, Reg),
    Not(Reg, Reg),
    /// `R(a) = R(b) .. ... .. R(c)`.
    Concat(Reg, Reg, Reg),
    Jmp(i32),
    /// Jump when `cmp(R(b), R(c)) == cond`.
    JmpCmp(CmpJ, Reg, Reg, i32),
    /// `cmp(R(b), K(c))`: the common `n < 2`, `x == nil`. Constants past the first 256 are
    /// loaded into a register and compared with `JmpCmp`.
    JmpCmpK(CmpJ, Reg, u8, i32),
    /// `cmp(K(b), R(c))`: `n > 2`, swapped into `2 < n`.
    JmpCmpKR(CmpJ, u8, Reg, i32),
    /// 5.1's `TEST` and its `JMP`: jump when the truth of `R(a)` is `cond`.
    JmpIf(Reg, bool, i32),
    /// 5.1's `TESTSET` and its `JMP`: when the truth of `R(b)` is `cond`, `R(a) = R(b)` and jump.
    TestSet(Reg, Reg, bool, i32),
    /// `b` is the argument count + 1 (0: up to the top), `c` the result count + 1 (0: all).
    Call(Reg, u8, u8),
    TailCall(Reg, u8),
    /// Return `R(a)..`, `b - 1` values (0: up to the top).
    Return(Reg, u8),
    /// Return `R(a)` alone.
    Return1(Reg),
    /// The client's `FORLOOP` (`0x6f9262`): `R(a)`, the loop's variable, must be a number and
    /// `R(a+1)`, `R(a+2)` are made numbers; `R(a) + R(a+2)`, while within `R(a+1)`, is written
    /// to `R(a)` and the jump taken.
    ForLoop(Reg, i32),
    /// `ForLoop` whose step the compiler knows is a positive constant: the test is `<=`.
    ForLoopUp(Reg, i32),
    /// 5.0's `TFORPREP`: a table generator becomes `next` over it; then the jump.
    TForPrep(Reg, i32),
    /// The client's `TFORLOOP` (`0x6f933b`): `R(a)(R(a+1), R(a+2))`, called from above the `b`
    /// variables, into the variables from `R(a+2)`, the first of them the control; when it is
    /// nil the next instruction, the jump back to the body, is skipped.
    TForLoop(Reg, u8),
    /// 5.1's `SETLIST`: `R(a)[base + i] = R(a + i)` for `i` in `1..=n`, the array part grown
    /// first to hold them all.
    SetList(Reg, u32, u16),
    /// The same, items up to the top (the last item was a call).
    SetListTop(Reg, u32),
    /// Close every open upvalue at or above `R(a)`.
    Close(Reg),
    Closure(Reg, u32),
}

// Every instruction is eight bytes: the dispatch loads one word and decodes only its own fields.
const _: () = assert!(std::mem::size_of::<Op>() == 8);

/// The longest name a function key keeps of a part: a longer name is cut here, so every key costs
/// one part of bounded size whatever the source spells.
pub const KEY_NAME_MAX: usize = 128;

/// A function's identity within its chunk, for hot reload ([`Proto::key`]): its enclosing
/// function's key and one part of its own, so a key shares every part above its own with its
/// parent and costs one part however deep it is nested. The main function's key has no part.
#[derive(Clone, Default)]
pub struct Key(Option<Rc<KeyPart>>);

struct KeyPart {
    parent: Key,
    /// The name, at most [`KEY_NAME_MAX`] bytes.
    name: Bytes,
    /// Which of the enclosing function's children with this name it is, from 1.
    nth: u32,
    /// How many of the enclosing function's children share the name, known once the chunk is
    /// compiled ([`Key::set_count`]): a numbered key matches across an edit only while it holds.
    count: Cell<u32>,
    /// A hash of the whole chain, so a key in a map is hashed in one step.
    hash: u64,
}

impl Key {
    /// The key of a child of this function named `name`, the `nth` of that name.
    pub fn child(&self, name: &[u8], nth: u32) -> Key {
        let name = &name[..name.len().min(KEY_NAME_MAX)];
        let mut h = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(&self.0.as_ref().map_or(0, |p| p.hash), &mut h);
        std::hash::Hash::hash(name, &mut h);
        std::hash::Hash::hash(&nth, &mut h);
        Key(Some(Rc::new(KeyPart {
            parent: self.clone(),
            name: Rc::from(name),
            nth,
            count: Cell::new(0),
            hash: std::hash::Hasher::finish(&h),
        })))
    }

    /// The main function's key.
    pub fn is_root(&self) -> bool {
        self.0.is_none()
    }

    /// The part's own name, empty for the main function's key.
    pub fn name(&self) -> &[u8] {
        self.0.as_ref().map_or(&[], |p| &p.name)
    }

    pub(crate) fn set_count(&self, n: u32) {
        if let Some(p) = &self.0 {
            p.count.set(n);
        }
    }

    fn parts(&self) -> impl Iterator<Item = &KeyPart> {
        std::iter::successors(self.0.as_deref(), |p| p.parent.0.as_deref())
    }

    /// Whether each part counts as many siblings of its name as `other`'s does.
    pub fn same_counts(&self, other: &Key) -> bool {
        let (mut a, mut b) = (self.parts(), other.parts());
        loop {
            match (a.next(), b.next()) {
                (None, None) => return true,
                (Some(x), Some(y)) if x.count.get() == y.count.get() => {}
                _ => return false,
            }
        }
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Key) -> bool {
        let (mut a, mut b) = (self.parts(), other.parts());
        loop {
            match (a.next(), b.next()) {
                (None, None) => return true,
                (Some(x), Some(y)) => {
                    if std::ptr::eq(x, y) {
                        return true;
                    }
                    if x.hash != y.hash || x.nth != y.nth || x.name != y.name {
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }
}

impl Eq for Key {}

impl std::hash::Hash for Key {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.0.as_ref().map_or(0, |p| p.hash).hash(h);
    }
}

/// The parts from the outermost, joined by `/`, a repeated name numbered: `Init/anon#2`.
impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts: Vec<&KeyPart> = self.parts().collect();
        parts.reverse();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                f.write_str("/")?;
            }
            f.write_str(&String::from_utf8_lossy(&p.name))?;
            if p.nth > 1 {
                write!(f, "#{}", p.nth)?;
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.to_string())
    }
}

/// The variable an upvalue captures, for hot reload: the function that declares it, its name,
/// and which declaration of that name in the function it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Decl {
    pub func: Key,
    pub name: Bytes,
    pub nth: u32,
}

/// A local's name and the registers' lifetime, for error messages.
#[derive(Clone, Debug)]
pub struct LocVar {
    pub name: Bytes,
    pub reg: Reg,
    pub startpc: u32,
    pub endpc: u32,
}

#[derive(Clone, Debug)]
pub struct Proto {
    pub code: Vec<Op>,
    pub lines: Vec<u32>,
    pub consts: Vec<Value>,
    pub protos: Vec<std::rc::Rc<Proto>>,
    /// Each upvalue: `(true, register)` in the enclosing frame, or `(false, index)` of its
    /// upvalues.
    pub upvals: Vec<(bool, u16)>,
    pub upval_names: Vec<Bytes>,
    /// The declaration each upvalue captures, for hot reload.
    pub upval_decls: Vec<Decl>,
    /// Parameters including `self`.
    pub nparams: u8,
    /// `...`: the extras become the `arg` table in register `nparams`.
    pub vararg: bool,
    /// The function has a generic `for` over more than three values, whose body can read the
    /// registers above its variables before anything writes them: what the calls under it left.
    /// While it runs, the VM clears each frame entered in its range as `luaD_precall` clears
    /// every frame (`Vm`'s `clear_below`). Elsewhere the code writes each register before it
    /// reads it, so the VM skips the clearing.
    pub clears: bool,
    pub maxstack: u16,
    /// The client's `maxstacksize`, the registers its `luaK_checkstack` (`0x700a50`) reserves for
    /// this function: the frame a call clears ([`crate::vm::Vm`]'s `clear_below`). `maxstack` is
    /// more by the slot the generic `for` steps `next` into, the scratch registers benilla's
    /// operands need, and the results a generic `for` with more than three names takes back
    /// above its variables, which the client's stack holds in its slack.
    pub c_maxstack: u8,
    pub source: Bytes,
    pub line_defined: u32,
    pub locvars: Vec<LocVar>,
    /// The strings 5.0's prototype holds besides its constants, interned so the collector keeps
    /// and counts them as `traverseproto` does: the chunk name, each local's name and each
    /// upvalue's.
    pub names: Box<[Value]>,
    /// The client's 5.0 code size for this function ([`crate::compile::FuncState::code_size50`]),
    /// which sizes its code and its line info alike (`0x6fcb2b`, `0x6fcb59`).
    pub code50: usize,
    /// The client's 5.0 constant count ([`crate::compile::FuncState::consts50`]).
    pub consts50: usize,
    /// The instructions 5.1 has no word for, ascending: the jump a skipping `LOADBOOL` makes, a
    /// `LOADNIL` at the head that `luaD_precall`'s clearing stands for, a constant loaded into a
    /// scratch register. The count hook does not count them, as 5.1 never dispatches them.
    pub uncounted: Box<[u32]>,
    /// The function's identity within its chunk, stable across edits that leave its name and
    /// nesting alone. A function is named as written (`Frame:OnEvent`, `Frame.Init`,
    /// `local helper`), or after the single target it is assigned to (`H.OnShow = function`,
    /// `local f = function`, a constructor's `name = function`), or `anon`; a name used twice
    /// is numbered (`Init/anon#2`). Hot reload matches on it.
    pub key: Key,
    /// Every key of the chunk this function was compiled in.
    pub chunk_keys: std::rc::Rc<[Key]>,
    /// `source`'s id in the heap's chunk list, for profiles and memory censuses.
    pub source_id: u32,
    /// The collection that last marked this prototype's constants (`Heap::collect`).
    pub(crate) gc_mark: std::cell::Cell<u32>,
}

impl Proto {
    /// This function's prototype in the collector's count, as 5.0's `close_func` sizes its
    /// vectors (`0x6fcb48`..`0x6fcc26`) and `luaF_freeproto` frees them (`0x6fa069`..`0x6fa102`):
    /// the 0x48-byte `Proto` (`0x6f9fea`), 4 bytes an instruction and 4 its line, 16 a constant,
    /// 4 a nested prototype, 12 a local and 4 an upvalue name, at the client's own code size and
    /// constant count.
    pub fn bytes(&self) -> usize {
        0x48 + 8 * self.code50
            + 16 * self.consts50
            + 4 * self.protos.len()
            + 12 * self.locvars.len()
            + 4 * self.upval_names.len()
    }

    /// [`Proto::bytes`] of this function and every one nested in it.
    pub fn tree_bytes(&self) -> usize {
        self.bytes() + self.protos.iter().map(|q| q.tree_bytes()).sum::<usize>()
    }

    /// The local in register `reg` at `pc`, if one is active there.
    pub fn local_name(&self, reg: Reg, pc: usize) -> Option<&Bytes> {
        self.locvars
            .iter()
            .rev()
            .find(|l| l.reg == reg && (l.startpc as usize) <= pc && pc < l.endpc as usize)
            .map(|l| &l.name)
    }
}

impl Proto {
    /// A hash of what this function runs, its code and constants, without its lines or the
    /// functions nested in it: two compiles of a chunk whose top level is the same agree on the
    /// main function's, however its functions' bodies changed, when one heap holds both (a string
    /// constant is its id there).
    pub(crate) fn code_digest(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.code.hash(&mut h);
        self.consts.hash(&mut h);
        h.finish()
    }

    /// Check, once per compiled function, every fact the interpreter's unchecked reads take on
    /// trust: each register an instruction touches is below `maxstack`, each constant, upvalue
    /// and child index exists, each jump lands inside the code, and the code ends in a return,
    /// so execution never runs off it. A failure is a code generator bug, and it stops here
    /// instead of becoming a read outside the frame.
    pub(crate) fn verify(&self) {
        self.verify_in(None);
    }

    /// `parent`: the enclosing function's register count and upvalue count, which this
    /// function's upvalue descriptions index.
    fn verify_in(&self, parent: Option<(u16, usize)>) {
        let fail = |pc: usize, what: &str| -> ! {
            panic!(
                "benilla-lua compiled bad code: {what} at instruction {pc} of `{}` in {}",
                self.key,
                String::from_utf8_lossy(&self.source),
            )
        };
        let n = self.code.len();
        let (nk, nup, nproto) = (self.consts.len(), self.upval_names.len(), self.protos.len());
        if !matches!(self.code.last(), Some(Op::Return(..) | Op::Return1(_))) {
            fail(n, "no final return");
        }
        if self.lines.len() != n {
            fail(n, "a line per instruction");
        }
        if (self.nparams as usize + self.vararg as usize) > self.maxstack as usize
            // The registers, then a generic `for`'s results above at most 198 variables.
            || self.maxstack as usize > 2 * MAXSTACK
        {
            fail(0, "the register count");
        }
        if self.upvals.len() != nup {
            fail(0, "the upvalue count");
        }
        if let Some((pmax, pups)) = parent {
            for &(in_stack, i) in &self.upvals {
                if (in_stack && i >= pmax) || (!in_stack && i as usize >= pups) {
                    fail(0, "an upvalue the enclosing function lacks");
                }
            }
        }
        for (pc, &op) in self.code.iter().enumerate() {
            let reg = |r: usize| {
                if r >= self.maxstack as usize {
                    fail(pc, "a register past maxstack");
                }
            };
            let k = |i: usize| {
                if i >= nk {
                    fail(pc, "a constant past the table");
                }
            };
            let rk = |x: Rk| {
                if x >= RK_CONST {
                    k((x - RK_CONST) as usize)
                } else {
                    reg(x as usize)
                }
            };
            let to = |skip: usize, off: i32| {
                let t = pc as i64 + 1 + skip as i64 + off as i64;
                if t < 0 || t >= n as i64 {
                    fail(pc, "a jump outside the code");
                }
            };
            let r = |x: Reg| reg(x as usize);
            let ku = |i: u16| k(i as usize);
            match op {
                Op::Move(x, y) | Op::Unm(x, y) | Op::Not(x, y) => {
                    r(x);
                    r(y);
                }
                Op::LoadK(x, i) | Op::GetGlobal(x, i) | Op::SetGlobal(x, i) => {
                    r(x);
                    k(i as usize);
                }
                Op::LoadNil(x, y) => {
                    if y < x {
                        fail(pc, "an empty nil range");
                    }
                    r(y);
                }
                Op::LoadBool(x, _) | Op::Close(x) | Op::NewTable(x, ..) | Op::Return1(x) => r(x),
                Op::SetListTop(x, _) => r(x),
                Op::GetUpval(x, u) | Op::SetUpval(x, u) => {
                    if u as usize >= nup {
                        fail(pc, "an upvalue past the list");
                    }
                    r(x);
                }
                Op::GetTable(x, y, z) => {
                    r(x);
                    r(y);
                    rk(z);
                }
                Op::SetTable(x, y, z) | Op::Pow(x, y, z) => {
                    r(x);
                    rk(y);
                    rk(z);
                }
                Op::GetField(x, y, i)
                | Op::AddK(x, y, i)
                | Op::SubK(x, y, i)
                | Op::MulK(x, y, i)
                | Op::DivK(x, y, i) => {
                    r(x);
                    r(y);
                    ku(i);
                }
                Op::SetField(x, i, z) => {
                    r(x);
                    ku(i);
                    rk(z);
                }
                Op::SelfOp(x, y, z) => {
                    reg(x as usize + 1);
                    r(y);
                    rk(z);
                }
                Op::SelfField(x, y, i) => {
                    reg(x as usize + 1);
                    r(y);
                    ku(i);
                }
                Op::Add(x, y, z) | Op::Sub(x, y, z) | Op::Mul(x, y, z) | Op::Div(x, y, z) => {
                    r(x);
                    r(y);
                    r(z);
                }
                Op::AddKR(x, i, z)
                | Op::SubKR(x, i, z)
                | Op::MulKR(x, i, z)
                | Op::DivKR(x, i, z) => {
                    r(x);
                    ku(i);
                    r(z);
                }
                Op::Concat(x, y, z) => {
                    if z <= y {
                        fail(pc, "a concat of fewer than two values");
                    }
                    r(x);
                    r(z);
                }
                Op::Jmp(off) => to(0, off),
                Op::JmpCmp(_, y, z, off) => {
                    r(y);
                    r(z);
                    to(0, off);
                }
                Op::JmpCmpK(_, y, i, off) => {
                    r(y);
                    k(i as usize);
                    to(0, off);
                }
                Op::JmpCmpKR(_, i, z, off) => {
                    k(i as usize);
                    r(z);
                    to(0, off);
                }
                Op::JmpIf(x, _, off) => {
                    r(x);
                    to(0, off);
                }
                Op::TestSet(x, y, _, off) => {
                    r(x);
                    r(y);
                    to(0, off);
                }
                // Arguments `a+1..a+b-1`, results `a..a+c-2`; a count of zero is up to the top,
                // which the stack covers at run time.
                Op::Call(x, nb, nc) => {
                    let x = x as usize;
                    reg(x + (nb as usize).saturating_sub(1));
                    reg(x + (nc as usize).saturating_sub(2));
                }
                Op::TailCall(x, nb) => reg(x as usize + (nb as usize).saturating_sub(1)),
                // `Return(a, 1)` returns nothing, and its `a` may be the first free register.
                Op::Return(x, nb) => {
                    if nb > 1 {
                        reg(x as usize + nb as usize - 2);
                    } else if x as usize > self.maxstack as usize {
                        fail(pc, "a return past maxstack");
                    }
                }
                Op::ForLoop(x, off) | Op::ForLoopUp(x, off) => {
                    reg(x as usize + 2);
                    to(0, off);
                }
                Op::TForPrep(x, off) => {
                    reg(x as usize + 2);
                    to(0, off);
                }
                // The loop variables `a+2..a+1+b`, and above them the call's three registers and
                // its `b` results; past a nil, the jump after it is skipped.
                Op::TForLoop(x, nb) => {
                    let nb = nb as usize;
                    reg(x as usize + 1 + nb + nb.max(3));
                    to(1, 0);
                }
                Op::SetList(x, _, cnt) => reg(x as usize + cnt as usize),
                Op::Closure(x, i) => {
                    if i as usize >= nproto {
                        fail(pc, "a child function past the list");
                    }
                    r(x);
                }
            }
        }
        for p in &self.protos {
            p.verify_in(Some((self.maxstack, nup)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiled(src: &str) -> Proto {
        let mut heap = crate::heap::Heap::default();
        let p = crate::compile::compile(src.as_bytes(), b"=t", &mut heap).unwrap();
        (*p).clone()
    }

    /// A prototype is counted at the client's 5.0 code size and constant count, which its own
    /// code generator gives for the same source.
    #[test]
    fn a_prototype_is_counted_at_the_clients_code_and_constants() {
        // (source, 5.0's instructions, 5.0's constants)
        for (src, code, k) in [
            // `LOADNIL 0 1`, which 5.1 leaves out, and `RETURN`.
            ("local a, b", 2, 0),
            // One `OP_LOADNIL`, widened.
            ("local a local b", 2, 0),
            // `MUL 0 K K`, no fold: the two numerals are constants, the product is not.
            ("local x = 2 * 3", 2, 2),
            // `5` as its token is read, and `-5` (`luaK_prefix`).
            ("local x = -5", 2, 2),
            // `GETGLOBAL`, `LOADBOOL`, `SETTABLE`, `RETURN`: no boolean constant.
            ("t.a = true", 4, 2),
            // `NEWTABLE`, 33 `LOADK`s, a `SETLIST` at 32 items and one for the last, `RETURN`.
            (
                "local t = {1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,\
                 26,27,28,29,30,31,32,33}",
                37,
                33,
            ),
        ] {
            let p = compiled(src);
            assert_eq!((p.code50, p.consts50), (code, k), "{src}");
            assert_eq!(
                p.bytes(),
                0x48 + 8 * code + 16 * k + 12 * p.locvars.len(),
                "{src}"
            );
        }
    }

    fn rejects(p: Proto) -> String {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.verify()));
        let e = r.expect_err("the verifier refuses it");
        e.downcast_ref::<String>().cloned().unwrap_or_default()
    }

    #[test]
    fn the_verifier_refuses_code_the_interpreter_would_read_past() {
        let mut p = compiled("local a = 1 return a");
        p.code[0] = Op::Move(p.maxstack as Reg, 0);
        assert!(rejects(p).contains("a register past maxstack"));

        let mut p = compiled("local a = 1 return a");
        p.code[0] = Op::LoadK(0, p.consts.len() as u32);
        assert!(rejects(p).contains("a constant past the table"));

        let mut p = compiled("local a = 1 return a");
        p.code[0] = Op::Jmp(p.code.len() as i32);
        assert!(rejects(p).contains("a jump outside the code"));

        let mut p = compiled("local a = 1 return a");
        *p.code.last_mut().unwrap() = Op::Move(0, 0);
        assert!(rejects(p).contains("no final return"));

        let mut p = compiled("local u = 1 F = function() return u end");
        let mut child = (*p.protos[0]).clone();
        child.code[0] = Op::GetUpval(0, 5);
        p.protos[0] = std::rc::Rc::new(child);
        assert!(rejects(p).contains("an upvalue past the list"));
    }
}
