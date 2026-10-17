//! The code generator: 5.1.5's `lcode.c`, run by the parser as it reads, so registers, constants,
//! jumps, line numbers and compile errors come out where the script host's C Lua puts them. The
//! instructions are benilla's: each test is fused with the `JMP` after it, and the common operand
//! shapes (a constant key, a constant operand, a positive constant step) have ops of their own,
//! chosen as an instruction is emitted. Jump ranges are checked in 5.1's instruction count.

use std::collections::HashMap;
use std::rc::Rc;

use crate::heap::Heap;
use crate::lex::SyntaxError;
use crate::parse::Parser;
use crate::proto::Bytes;
use crate::proto::*;
use crate::value::Value;

pub(crate) type R<T> = Result<T, SyntaxError>;

/// The end of a jump list, and a jump not yet pointed anywhere.
pub(crate) const NO_JUMP: i32 = -1;
/// 5.1's `NO_REG` (`MAXARG_A`): no register, beyond any a function has.
pub(crate) const NO_REG: Reg = 255;
/// 5.1's `MAXINDEXRK`: constants past it are loaded into a register to be an operand.
const MAXINDEXRK: u32 = 255;
/// 5.1's `MAXARG_Bx`: constants and nested functions per function.
const MAXARG_BX: usize = (1 << 18) - 1;
/// 5.1's `MAXARG_sBx`: the longest jump, in 5.1's instructions.
const MAXARG_SBX: i64 = MAXARG_BX as i64 >> 1;
/// `SHRT_MAX`, `registerlocalvar`'s limit on local declarations per function.
const MAX_LOCVARS: usize = 32767;

/// Compile a chunk into its main function. Its limits are its own: the host calls it runs
/// under count toward none (the client's per-chunk `nestlevel`, `0x6fc9d0`).
pub fn compile(src: &[u8], chunkname: &[u8], heap: &mut Heap) -> R<Rc<Proto>> {
    crate::parse::parse(src, chunkname, heap)
}

/// 5.0's `luaO_int2fb` (`0x6f5810`): a size as a "floating point byte" `mmmmmxxx`, `xxx * 2^m`,
/// halved and rounded up while it is 8 or more.
pub fn int2fb(mut x: u32) -> u32 {
    let mut m = 0;
    while x >= 8 {
        x = (x + 1) >> 1;
        m += 1;
    }
    (m << 3) | x
}

/// 5.0's `fb2int`, as `OP_NEWTABLE` decodes its `B` (`0x6f8b31`..`0x6f8b41`): `(x & 7) << (x >> 3)`.
pub fn fb2int(x: u32) -> u32 {
    (x & 7) << (x >> 3)
}

/// 5.1's `expkind`, with each kind's `info` in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    Void,
    Nil,
    True,
    False,
    /// A constant, by index.
    K(u32),
    KNum(f64),
    /// A local, by register.
    Local(Reg),
    Upval(u16),
    /// A global, by its name's constant.
    Global(u32),
    /// A table in a register and a key in a register or constant.
    Indexed(Reg, Rk),
    /// A comparison: the pc of its fused jump.
    Jmp(usize),
    /// The pc of an instruction whose destination is still open.
    Reloc(usize),
    NonReloc(Reg),
    /// The pc of an open call.
    Call(usize),
}

/// 5.1's `expdesc`: a kind and the lists of jumps taken when it is true and false.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Exp {
    pub k: Kind,
    pub t: i32,
    pub f: i32,
    /// A `KNum` that 5.1 folded and the client's 5.0 computes when it runs: its `luaK_posfix`
    /// folds no arithmetic (`0x701610`), so the value is an instruction's result there. A `not`
    /// keeps it: 5.0's is an `OP_NOT` of that result, where 5.1's is a constant.
    pub rt: bool,
}

impl Exp {
    pub fn new(k: Kind) -> Exp {
        Exp {
            k,
            t: NO_JUMP,
            f: NO_JUMP,
            rt: false,
        }
    }

    /// A numeral 5.1 folded ([`Exp::rt`]).
    pub fn is_rt_numeral(&self) -> bool {
        self.rt && matches!(self.k, Kind::KNum(_))
    }

    /// A constant 5.1 made of what 5.0 computes ([`Exp::rt`]): a folded numeral or its `not`.
    fn is_rt(&self) -> bool {
        self.rt && matches!(self.k, Kind::KNum(_) | Kind::True | Kind::False)
    }

    fn has_jumps(&self) -> bool {
        self.t != self.f
    }

    fn is_numeral(&self) -> Option<f64> {
        match self.k {
            Kind::KNum(n) if self.t == NO_JUMP && self.f == NO_JUMP => Some(n),
            _ => None,
        }
    }

    pub fn has_multret(&self) -> bool {
        matches!(self.k, Kind::Call(_))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum KKey {
    Nil,
    Bool(bool),
    Num(u64),
    Str(u32),
}

/// The constant map's hash: a multiply-rotate over the key's words, as 5.1's constant table
/// (a Lua table) hashes a number by its bits and a string by its own hash. The map is only
/// looked up, never iterated, so the hash decides no order.
#[derive(Default)]
struct KHasher(u64);

impl std::hash::Hasher for KHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u64(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn write_u32(&mut self, x: u32) {
        self.write_u64(u64::from(x));
    }

    fn write_u8(&mut self, x: u8) {
        self.write_u64(u64::from(x));
    }

    fn write_isize(&mut self, x: isize) {
        self.write_u64(x as u64);
    }

    fn write_usize(&mut self, x: usize) {
        self.write_u64(x as u64);
    }

    fn finish(&self) -> u64 {
        // A product's low bits depend on the key's low bits alone, and a number's are often
        // zero: the high bits are turned down to where the map picks its bucket.
        self.0.rotate_left(26)
    }
}

type KMap = HashMap<KKey, u32, std::hash::BuildHasherDefault<KHasher>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinOpr {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Ne,
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnOpr {
    Minus,
    Not,
}

/// 5.1's `BlockCnt`.
pub(crate) struct BlockCnt {
    pub breaklist: i32,
    pub nactvar: u8,
    pub upval: bool,
    pub isbreakable: bool,
}

/// 5.1's `FuncState`, with the `Proto` it fills.
pub(crate) struct FuncState {
    pub code: Vec<Op>,
    pub lines: Vec<u32>,
    /// Each instruction's position in 5.1's own code, which a fused test takes two words of: the
    /// jump-range limit is 5.1's.
    cpc: Vec<u32>,
    cpc_next: u32,
    /// The client's 5.0 code size less 5.1's, summed where their code generators emit
    /// differently ([`FuncState::code_size50`]).
    c50_delta: i64,
    pub consts: Vec<Value>,
    kmap: KMap,
    /// The constants the client's 5.0 makes for the same code ([`FuncState::consts50`]).
    k50: std::collections::HashSet<KKey, std::hash::BuildHasherDefault<KHasher>>,
    /// The registers of the `OP_LOADNIL` 5.0 emits at the head of the function, where 5.1 emits
    /// none ([`Parser::nil`]).
    head_nil50: Option<(Reg, Reg)>,
    pub protos: Vec<Rc<Proto>>,
    /// Each upvalue as `(true, register)` in the enclosing function or `(false, upvalue)`.
    pub upvals: Vec<(bool, u16)>,
    pub upval_names: Vec<Bytes>,
    /// The declaration each upvalue captures ([`FuncState::local_decl`]).
    pub upval_decls: Vec<Decl>,
    pub locvars: Vec<LocVar>,
    /// `locvars` index of each active local, then of those declared but not yet active.
    pub actvar: Vec<u16>,
    pub nactvar: u8,
    pub freereg: u8,
    pub maxstack: u8,
    /// [`Proto::c_maxstack`]: `maxstack` as `luaK_checkstack` alone raises it.
    c_maxstack: u8,
    /// The registers a generic `for`'s call and results reach above its variables, which
    /// `luaK_checkstack` does not count ([`Proto::maxstack`]).
    pub tfor_top: u16,
    /// The last pc marked as a jump target (`luaK_getlabel`), in 5.1's count, or -1.
    lasttarget: i64,
    /// `LoadNil`s at the head of the function that 5.1 leaves out (see [`Parser::nil`]).
    silent_nils: usize,
    /// Jumps pending to the next instruction.
    jpc: i32,
    pub bl: Vec<BlockCnt>,
    pub linedefined: u32,
    pub nparams: u8,
    pub vararg: bool,
    /// [`Proto::clears`].
    pub clears: bool,
    pub source: Bytes,
    pub key: Key,
    /// How many children so far have each name, cut as a key cuts it.
    child_names: HashMap<Bytes, u32>,
    /// Which declaration of its name each of `locvars` is, from 1, and the count by name.
    locvar_nth: Vec<u32>,
    locvar_names: HashMap<Bytes, u32>,
}

impl FuncState {
    /// The code so far as the client's 5.0 would count it: 5.1's words, and per difference of
    /// the code generators, each at its site: 5.0 folds no arithmetic (`luaK_posfix`,
    /// `0x701610`) and only a constant's minus (`luaK_prefix`, `0x701445`); its `luaK_exp2RK`
    /// makes no constant of `true` or `false` (`0x701023`); and its `luaK_goiftrue` jumps on
    /// `false` (`0x7011ff`) and `luaK_goiffalse` on `true` (`0x7013b4`), where 5.1 tests them.
    pub(crate) fn code_size50(&self) -> i64 {
        self.cpc_next as i64 + self.c50_delta
    }

    /// The constants the client's 5.0 makes for the same code: every string, `nil` where an
    /// operand takes it, and every numeral as its token is read (`simpleexp`'s `luaK_numberK`)
    /// and again negated (`luaK_prefix`, `0x701445`); no boolean (`luaK_exp2RK`, `0x701023`) and
    /// nothing 5.1 folds (`luaK_posfix`, `0x701610`).
    pub(crate) fn consts50(&self) -> usize {
        self.k50.len()
    }

    pub fn new(source: Bytes, linedefined: u32, key: Key) -> FuncState {
        FuncState {
            code: Vec::new(),
            lines: Vec::new(),
            cpc: Vec::new(),
            cpc_next: 0,
            c50_delta: 0,
            consts: Vec::new(),
            kmap: KMap::default(),
            k50: Default::default(),
            head_nil50: None,
            protos: Vec::new(),
            upvals: Vec::new(),
            upval_names: Vec::new(),
            upval_decls: Vec::new(),
            locvars: Vec::new(),
            actvar: Vec::new(),
            nactvar: 0,
            freereg: 0,
            maxstack: 2,
            c_maxstack: 2,
            tfor_top: 0,
            lasttarget: -1,
            silent_nils: 0,
            jpc: NO_JUMP,
            bl: Vec::new(),
            linedefined,
            nparams: 0,
            vararg: false,
            clears: false,
            source,
            key,
            child_names: HashMap::new(),
            locvar_nth: Vec::new(),
            locvar_names: HashMap::new(),
        }
    }

    pub fn pc(&self) -> usize {
        self.code.len()
    }

    /// The declaration of the active local in register `r`, as hot reload tells captured
    /// variables apart: this function's key, the name, and which declaration of that name it is.
    /// It shares the key and the name, so a capture costs no copy of either.
    pub fn local_decl(&self, r: Reg) -> Decl {
        let idx = self.actvar[r as usize] as usize;
        Decl {
            func: self.key.clone(),
            name: self.locvars[idx].name.clone(),
            nth: self.locvar_nth[idx],
        }
    }

    /// A local declared: which declaration of its name it is.
    pub(crate) fn count_locvar(&mut self, name: &Bytes) {
        let n = self.locvar_names.entry(name.clone()).or_insert(0);
        *n += 1;
        self.locvar_nth.push(*n);
    }

    /// A child's key: this function's, then the child's name, numbered when it repeats. Hot
    /// reload matches functions across an edit by it.
    pub fn child_key(&mut self, name: Option<Vec<u8>>) -> Key {
        let name = name.as_deref().unwrap_or(b"anon");
        let name: Bytes = Rc::from(&name[..name.len().min(KEY_NAME_MAX)]);
        let n = self.child_names.entry(name.clone()).or_insert(0);
        *n += 1;
        self.key.child(&name, *n)
    }

    /// The finished prototype (`close_func`, after its final return).
    pub fn into_proto(mut self, heap: &mut Heap) -> Proto {
        // Instructions 5.1 does not have take the line of the first one it does, so a line hook
        // sees 5.1's lines.
        let n = self.silent_nils;
        let line = self.lines[n];
        self.lines[..n].fill(line);
        debug_assert!(!self
            .code
            .iter()
            .any(|op| matches!(op, Op::TestSet(NO_REG, ..))));
        let uncounted = (0..self.code.len())
            .filter(|&i| self.cpc.get(i + 1).copied().unwrap_or(self.cpc_next) == self.cpc[i])
            .map(|i| i as u32)
            .collect();
        let code50 = self.code_size50().max(0) as usize;
        let consts50 = self.consts50();
        // `luaY_parser` takes the chunk name by `strlen` (`0x6fc9c2`..`0x6fc9df`).
        let names = std::iter::once(crate::lex::cstr(&self.source))
            .chain(self.locvars.iter().map(|l| &*l.name))
            .chain(self.upval_names.iter().map(|n| &**n))
            .map(|n| heap.str_value(n))
            .collect();
        Proto {
            code: self.code,
            uncounted,
            lines: self.lines,
            consts: self.consts,
            protos: self.protos,
            upvals: self.upvals,
            upval_names: self.upval_names,
            upval_decls: self.upval_decls,
            nparams: self.nparams,
            vararg: self.vararg,
            clears: self.clears,
            maxstack: (self.maxstack as u16 + 1).max(self.tfor_top),
            c_maxstack: self.c_maxstack,
            source_id: heap.source_id(&self.source),
            source: self.source,
            line_defined: self.linedefined,
            locvars: self.locvars,
            names,
            code50,
            consts50,
            key: self.key,
            chunk_keys: Rc::from(Vec::new()),
            gc_mark: Default::default(),
        }
    }
}

/// Fill in the whole chunk's key facts once it is compiled: how many siblings share each key's
/// name ([`Key::same_counts`]) and the chunk's [`Proto::chunk_keys`].
pub(crate) fn finish_keys(root: &mut Proto) {
    fn counts(p: &Proto, keys: &mut Vec<Key>) {
        keys.push(p.key.clone());
        let mut same: HashMap<&[u8], u32> = HashMap::new();
        for q in &p.protos {
            *same.entry(q.key.name()).or_insert(0) += 1;
        }
        for q in &p.protos {
            q.key.set_count(same[q.key.name()]);
            counts(q, keys);
        }
    }
    fn share(p: &mut Proto, keys: &Rc<[Key]>) {
        p.chunk_keys = keys.clone();
        for q in &mut p.protos {
            share(Rc::get_mut(q).expect("a fresh prototype"), keys);
        }
    }
    let mut keys = Vec::new();
    counts(root, &mut keys);
    share(root, &Rc::from(keys));
}

/// The jump offset an instruction carries, if it is a jump.
fn jump_of(op: &Op) -> Option<i32> {
    match *op {
        Op::Jmp(o)
        | Op::JmpCmp(_, _, _, o)
        | Op::JmpCmpK(_, _, _, o)
        | Op::JmpCmpKR(_, _, _, o)
        | Op::JmpIf(_, _, o)
        | Op::TestSet(_, _, _, o)
        | Op::ForLoop(_, o)
        | Op::ForLoopUp(_, o)
        | Op::TForPrep(_, o) => Some(o),
        _ => None,
    }
}

fn set_jump(op: &mut Op, off: i32) {
    match op {
        Op::Jmp(o)
        | Op::JmpCmp(_, _, _, o)
        | Op::JmpCmpK(_, _, _, o)
        | Op::JmpCmpKR(_, _, _, o)
        | Op::JmpIf(_, _, o)
        | Op::TestSet(_, _, _, o)
        | Op::ForLoop(_, o)
        | Op::ForLoopUp(_, o)
        | Op::TForPrep(_, o) => *o = off,
        op => unreachable!("not a jump: {op:?}"),
    }
}

/// A test fused with its `JMP`: 5.1's jump word is the second of its two.
fn is_fused_test(op: &Op) -> bool {
    matches!(
        op,
        Op::JmpCmp(..) | Op::JmpCmpK(..) | Op::JmpCmpKR(..) | Op::JmpIf(..) | Op::TestSet(..)
    )
}

/// `SETARG_A` on an instruction whose destination was left open.
fn set_a(op: &mut Op, r: Reg) {
    match op {
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
        | Op::Closure(a, _) => *a = r,
        op => unreachable!("not relocatable: {op:?}"),
    }
}

/// A number's constant key: `-0` and `0` are one constant, the first seen.
fn num_key(n: f64) -> KKey {
    KKey::Num(if n == 0.0 { 0 } else { n.to_bits() })
}

fn is_k(x: Rk) -> bool {
    x >= RK_CONST
}

/// 5.1's `constfolding`: two numerals fold unless the result would be a NaN or a division by
/// zero is asked for. `^` never folds: the client's `^` calls the global `__pow` (`0x6f9b52`),
/// which a script may replace, so `2 ^ 3` in source calls it too.
fn fold(op: BinOpr, a: f64, b: f64) -> Option<f64> {
    let r = match op {
        BinOpr::Add => a + b,
        BinOpr::Sub => a - b,
        BinOpr::Mul => a * b,
        BinOpr::Div => {
            if b == 0.0 {
                return None;
            }
            a / b
        }
        _ => return None,
    };
    (!r.is_nan()).then_some(r)
}

impl<'a> Parser<'a> {
    pub(crate) fn fs(&mut self) -> &mut FuncState {
        self.fs.last_mut().unwrap()
    }

    pub(crate) fn fsr(&self) -> &FuncState {
        self.fs.last().unwrap()
    }

    // ── emission ─────────────────────────────────────────────────────────────────────────────

    /// `luaK_code`: pending jumps land here, then the instruction, at the last token's line.
    /// `words` is the instruction's size in 5.1's code.
    pub(crate) fn code(&mut self, op: Op, words: u32) -> R<usize> {
        self.dischargejpc()?;
        Ok(self.code_raw(op, words))
    }

    fn code_raw(&mut self, op: Op, words: u32) -> usize {
        let line = self.lx.lastline;
        let fs = self.fs();
        fs.code.push(op);
        fs.lines.push(line);
        fs.cpc.push(fs.cpc_next);
        fs.cpc_next += words;
        fs.code.len() - 1
    }

    /// `luaK_fixline`: the last instruction's line. After a closure with upvalues 5.1's last
    /// instruction is the closure's last pseudo-instruction, which benilla has not: the
    /// closure keeps its own line.
    pub(crate) fn fixline(&mut self, line: u32) {
        let fs = self.fs();
        if let Some(&Op::Closure(_, px)) = fs.code.last() {
            if !fs.protos[px as usize].upvals.is_empty() {
                return;
            }
        }
        *fs.lines.last_mut().unwrap() = line;
    }

    /// `luaK_getlabel`: the current pc, marked as a jump target.
    pub(crate) fn getlabel(&mut self) -> usize {
        let fs = self.fs();
        fs.lasttarget = fs.cpc_next as i64;
        fs.pc()
    }

    fn getjump(&self, pc: usize) -> i32 {
        let off = jump_of(&self.fsr().code[pc]).unwrap();
        if off == NO_JUMP {
            NO_JUMP
        } else {
            pc as i32 + 1 + off
        }
    }

    /// `fixjump`, with 5.1's range check made on 5.1's own instruction positions.
    fn fixjump(&mut self, pc: usize, dest: usize) -> R<()> {
        let fs = self.fsr();
        let cjump = fs.cpc[pc] + is_fused_test(&fs.code[pc]) as u32;
        let cdest = fs.cpc.get(dest).copied().unwrap_or(fs.cpc_next);
        if (cdest as i64 - (cjump as i64 + 1)).abs() > MAXARG_SBX {
            return Err(self.lx.error("control structure too long"));
        }
        let off = dest as i32 - (pc as i32 + 1);
        set_jump(&mut self.fs().code[pc], off);
        Ok(())
    }

    /// `luaK_concat`: list `l2` appended to list `l1`.
    pub(crate) fn concat(&mut self, l1: &mut i32, l2: i32) -> R<()> {
        if l2 == NO_JUMP {
            return Ok(());
        }
        if *l1 == NO_JUMP {
            *l1 = l2;
            return Ok(());
        }
        let mut list = *l1 as usize;
        loop {
            let next = self.getjump(list);
            if next == NO_JUMP {
                break;
            }
            list = next as usize;
        }
        self.fixjump(list, l2 as usize)
    }

    /// `luaK_jump`: an unconditional jump, which takes over the jumps pending to here.
    pub(crate) fn jump(&mut self) -> R<i32> {
        let jpc = std::mem::replace(&mut self.fs().jpc, NO_JUMP);
        let mut j = self.code(Op::Jmp(NO_JUMP), 1)? as i32;
        self.concat(&mut j, jpc)?;
        Ok(j)
    }

    /// `luaK_ret`.
    pub(crate) fn ret(&mut self, first: Reg, nret: i32) -> R<()> {
        let op = if nret == 1 {
            Op::Return1(first)
        } else {
            Op::Return(first, (nret + 1) as u8)
        };
        self.code(op, 1)?;
        Ok(())
    }

    /// `patchtestreg`: a `TESTSET` gets its destination, or becomes a `TEST` when there is none
    /// or the value is already there. Answers whether `node` is a test that produces a value.
    fn patchtestreg(&mut self, node: usize, reg: Reg) -> bool {
        let fs = self.fs();
        let Op::TestSet(_, b, c, off) = fs.code[node] else {
            return false;
        };
        fs.code[node] = if reg != NO_REG && reg != b {
            Op::TestSet(reg, b, c, off)
        } else {
            Op::JmpIf(b, c, off)
        };
        true
    }

    /// `need_value`: a jump in the list that is not a value-producing test.
    fn need_value(&self, mut list: i32) -> bool {
        while list != NO_JUMP {
            if !matches!(self.fsr().code[list as usize], Op::TestSet(..)) {
                return true;
            }
            list = self.getjump(list as usize);
        }
        false
    }

    fn removevalues(&mut self, mut list: i32) {
        while list != NO_JUMP {
            self.patchtestreg(list as usize, NO_REG);
            list = self.getjump(list as usize);
        }
    }

    fn patchlistaux(&mut self, mut list: i32, vtarget: usize, reg: Reg, dtarget: usize) -> R<()> {
        while list != NO_JUMP {
            let next = self.getjump(list as usize);
            if self.patchtestreg(list as usize, reg) {
                self.fixjump(list as usize, vtarget)?;
            } else {
                self.fixjump(list as usize, dtarget)?;
            }
            list = next;
        }
        Ok(())
    }

    fn dischargejpc(&mut self) -> R<()> {
        let fs = self.fs();
        let jpc = std::mem::replace(&mut fs.jpc, NO_JUMP);
        let pc = fs.pc();
        self.patchlistaux(jpc, pc, NO_REG, pc)
    }

    /// `luaK_patchlist`.
    pub(crate) fn patchlist(&mut self, list: i32, target: usize) -> R<()> {
        if target == self.fsr().pc() {
            self.patchtohere(list)
        } else {
            self.patchlistaux(list, target, NO_REG, target)
        }
    }

    /// `luaK_patchtohere`.
    pub(crate) fn patchtohere(&mut self, list: i32) -> R<()> {
        self.getlabel();
        let mut jpc = self.fsr().jpc;
        self.concat(&mut jpc, list)?;
        self.fs().jpc = jpc;
        Ok(())
    }

    // ── registers ────────────────────────────────────────────────────────────────────────────

    /// `luaK_checkstack`: a function keeps below `MAXSTACK` registers.
    pub(crate) fn checkstack(&mut self, n: u32) -> R<()> {
        let fs = self.fsr();
        let newstack = fs.freereg as u32 + n;
        if newstack > fs.maxstack as u32 {
            if newstack >= MAXSTACK as u32 {
                return Err(self.lx.error("function or expression too complex"));
            }
            self.fs().maxstack = newstack as u8;
        }
        let fs = self.fs();
        fs.c_maxstack = fs.c_maxstack.max(newstack as u8);
        Ok(())
    }

    /// `luaK_reserveregs`.
    pub(crate) fn reserveregs(&mut self, n: u32) -> R<()> {
        self.checkstack(n)?;
        self.fs().freereg += n as u8;
        Ok(())
    }

    /// A register past the live ones for a constant 5.1 reads straight from its operand field,
    /// which benilla's op for that shape cannot: written and read by the next instruction alone.
    fn scratch(&mut self) -> Reg {
        let fs = self.fs();
        let r = fs.freereg;
        fs.maxstack = fs.maxstack.max(r + 1);
        r
    }

    /// `freereg`: a temporary on top of the stack is released.
    fn freereg(&mut self, reg: Rk) {
        let fs = self.fs();
        if !is_k(reg) && reg >= fs.nactvar as Rk {
            fs.freereg -= 1;
            debug_assert_eq!(reg, fs.freereg as Rk);
        }
    }

    fn freeexp(&mut self, e: &Exp) {
        if let Kind::NonReloc(r) = e.k {
            self.freereg(r as Rk);
        }
    }

    // ── constants ────────────────────────────────────────────────────────────────────────────

    /// `addk`: a value's index in the constant table, added when new. Its keys are values, so
    /// `-0` and `0` are one constant, the first seen.
    fn addk(&mut self, key: KKey, v: Value) -> R<u32> {
        if let Some(&i) = self.fsr().kmap.get(&key) {
            return Ok(i);
        }
        if self.fsr().consts.len() >= MAXARG_BX {
            // `luaM_growvector`'s `luaG_runerror`, which names no position from the compiler.
            return Err(SyntaxError(b"constant table overflow".to_vec()));
        }
        let fs = self.fs();
        fs.consts.push(v);
        let i = (fs.consts.len() - 1) as u32;
        fs.kmap.insert(key, i);
        Ok(i)
    }

    pub(crate) fn string_k(&mut self, s: &[u8]) -> R<u32> {
        let r = self.heap.intern(s);
        self.fs().k50.insert(KKey::Str(r.id));
        self.addk(KKey::Str(r.id), Value::Str(r))
    }

    /// A number constant. The client's 5.0 counts it where its own code makes it
    /// ([`Parser::number_k50`]), not here: what 5.1 makes of a numeral 5.0 already has, and what
    /// it makes of a fold 5.0 has not.
    pub(crate) fn number_k(&mut self, n: f64) -> R<u32> {
        self.addk(num_key(n), Value::Num(n))
    }

    /// A number the client's 5.0 makes a constant of ([`FuncState::consts50`]).
    pub(crate) fn number_k50(&mut self, n: f64) {
        self.fs().k50.insert(num_key(n));
    }

    fn bool_k(&mut self, b: bool) -> R<u32> {
        self.addk(KKey::Bool(b), Value::Bool(b))
    }

    fn nil_k(&mut self) -> R<u32> {
        self.fs().k50.insert(KKey::Nil);
        self.addk(KKey::Nil, Value::Nil)
    }

    /// The constant `x` names, when it is a string: a field name, for the string-key ops.
    fn str_k(&self, x: Rk) -> Option<u16> {
        let fs = self.fsr();
        (is_k(x) && fs.consts[(x - RK_CONST) as usize].as_str().is_some()).then(|| x - RK_CONST)
    }

    // ── expressions ──────────────────────────────────────────────────────────────────────────

    /// `luaK_nil`: registers `from..from + n` become nil, merged into a `LOADNIL` just before
    /// when no jump lands between them.
    ///
    /// At the start of a function 5.1 emits nothing for registers no local holds yet, which
    /// `luaD_precall` cleared. benilla's frames are not cleared, so the instruction is emitted
    /// all the same, sized as no 5.1 instruction and given the next one's line: positions,
    /// jumps and line hooks stay 5.1's.
    pub(crate) fn nil(&mut self, from: Reg, n: u32) -> R<()> {
        let to = (from as u32 + n - 1) as u8;
        let fs = self.fsr();
        if fs.cpc_next as i64 > fs.lasttarget {
            if fs.cpc_next == 0 {
                // The client's 5.0 emits these (its `lasttarget` starts at 0, `0x6fcaba`, so its
                // `luaK_nil` at pc 0 has no previous to widen, `0x700798`), one `OP_LOADNIL`
                // widened while each follows on.
                let fs = self.fs();
                match fs.head_nil50 {
                    Some((pfrom, pto)) if pfrom <= from && from as u32 <= pto as u32 + 1 => {
                        fs.head_nil50 = Some((pfrom, to.max(pto)));
                    }
                    _ => {
                        fs.c50_delta += 1;
                        fs.head_nil50 = Some((from, to));
                    }
                }
                if from >= fs.nactvar {
                    self.code(Op::LoadNil(from, to), 0)?;
                    self.fs().silent_nils += 1;
                    return Ok(());
                }
                // A local's register: 5.1 emits it, which 5.0 widened its own into or emitted.
                self.fs().c50_delta -= 1;
            } else if let Some(&Op::LoadNil(pfrom, pto)) = fs.code.last() {
                if pfrom <= from && from as u32 <= pto as u32 + 1 {
                    if to > pto {
                        *self.fs().code.last_mut().unwrap() = Op::LoadNil(pfrom, to);
                    }
                    return Ok(());
                }
            }
        }
        self.code(Op::LoadNil(from, to), 1)?;
        Ok(())
    }

    /// `luaK_setreturns`: an open call keeps `nresults` values (-1: all).
    pub(crate) fn setreturns(&mut self, e: &Exp, nresults: i32) {
        if let Kind::Call(pc) = e.k {
            if let Op::Call(_, _, c) = &mut self.fs().code[pc] {
                *c = (nresults + 1) as u8;
            }
        }
    }

    pub(crate) fn setmultret(&mut self, e: &Exp) {
        self.setreturns(e, -1);
    }

    /// `luaK_setoneret`: an open call's single value, in its base register.
    pub(crate) fn setoneret(&mut self, e: &mut Exp) {
        if let Kind::Call(pc) = e.k {
            let Op::Call(a, ..) = self.fsr().code[pc] else {
                unreachable!()
            };
            e.k = Kind::NonReloc(a);
        }
    }

    /// `luaK_dischargevars`: a variable's value read into an instruction.
    pub(crate) fn dischargevars(&mut self, e: &mut Exp) -> R<()> {
        match e.k {
            Kind::Local(r) => e.k = Kind::NonReloc(r),
            Kind::Upval(u) => e.k = Kind::Reloc(self.code(Op::GetUpval(0, u), 1)?),
            Kind::Global(k) => e.k = Kind::Reloc(self.code(Op::GetGlobal(0, k), 1)?),
            Kind::Indexed(t, key) => {
                self.freereg(key);
                self.freereg(t as Rk);
                let op = match self.str_k(key) {
                    Some(k) => Op::GetField(0, t, k),
                    None => Op::GetTable(0, t, key),
                };
                e.k = Kind::Reloc(self.code(op, 1)?);
            }
            Kind::Call(_) => self.setoneret(e),
            _ => {}
        }
        Ok(())
    }

    fn discharge2reg(&mut self, e: &mut Exp, reg: Reg) -> R<()> {
        self.dischargevars(e)?;
        match e.k {
            Kind::Nil => self.nil(reg, 1)?,
            Kind::False | Kind::True => {
                if e.rt {
                    // 5.0 names the register its `OP_NOT` writes.
                    self.fs().c50_delta -= 1;
                }
                self.code(Op::LoadBool(reg, e.k == Kind::True), 1)?;
            }
            Kind::K(k) => {
                self.code(Op::LoadK(reg, k), 1)?;
            }
            Kind::KNum(n) => {
                if e.rt {
                    // 5.0 names the register its instruction writes.
                    self.fs().c50_delta -= 1;
                }
                let k = self.number_k(n)?;
                self.code(Op::LoadK(reg, k), 1)?;
            }
            Kind::Reloc(pc) => set_a(&mut self.fs().code[pc], reg),
            Kind::NonReloc(r) => {
                if r != reg {
                    self.code(Op::Move(reg, r), 1)?;
                }
            }
            _ => {
                debug_assert!(matches!(e.k, Kind::Void | Kind::Jmp(_)));
                return Ok(());
            }
        }
        e.k = Kind::NonReloc(reg);
        Ok(())
    }

    fn discharge2anyreg(&mut self, e: &mut Exp) -> R<()> {
        if !matches!(e.k, Kind::NonReloc(_)) {
            self.reserveregs(1)?;
            let r = self.fsr().freereg - 1;
            self.discharge2reg(e, r)?;
        }
        Ok(())
    }

    /// `code_label`: a `LOADBOOL`, which may be a jump target. 5.1's skip of the next
    /// instruction is a jump of its own here, counted in 5.1's single word.
    fn code_label(&mut self, a: Reg, b: bool, skip: bool) -> R<usize> {
        self.getlabel();
        let pc = self.code(Op::LoadBool(a, b), 1)?;
        if skip {
            self.code_raw(Op::Jmp(1), 0);
        }
        Ok(pc)
    }

    /// `exp2reg`: the value in `reg`, its true and false jumps resolved into it.
    fn exp2reg(&mut self, e: &mut Exp, reg: Reg) -> R<()> {
        self.discharge2reg(e, reg)?;
        if let Kind::Jmp(pc) = e.k {
            let mut t = e.t;
            self.concat(&mut t, pc as i32)?;
            e.t = t;
        }
        if e.has_jumps() {
            let mut p_f = NO_JUMP as usize;
            let mut p_t = NO_JUMP as usize;
            if self.need_value(e.t) || self.need_value(e.f) {
                let fj = if matches!(e.k, Kind::Jmp(_)) {
                    NO_JUMP
                } else {
                    self.jump()?
                };
                p_f = self.code_label(reg, false, true)?;
                p_t = self.code_label(reg, true, false)?;
                self.patchtohere(fj)?;
            }
            let fin = self.getlabel();
            self.patchlistaux(e.f, fin, reg, p_f)?;
            self.patchlistaux(e.t, fin, reg, p_t)?;
        }
        e.f = NO_JUMP;
        e.t = NO_JUMP;
        e.k = Kind::NonReloc(reg);
        Ok(())
    }

    /// `luaK_exp2nextreg`.
    pub(crate) fn exp2nextreg(&mut self, e: &mut Exp) -> R<()> {
        self.dischargevars(e)?;
        self.freeexp(e);
        self.reserveregs(1)?;
        let r = self.fsr().freereg - 1;
        self.exp2reg(e, r)
    }

    /// `luaK_exp2anyreg`.
    pub(crate) fn exp2anyreg(&mut self, e: &mut Exp) -> R<Reg> {
        self.dischargevars(e)?;
        if let Kind::NonReloc(r) = e.k {
            if !e.has_jumps() {
                return Ok(r);
            }
            if r >= self.fsr().nactvar {
                self.exp2reg(e, r)?;
                return Ok(r);
            }
        }
        self.exp2nextreg(e)?;
        let Kind::NonReloc(r) = e.k else {
            unreachable!()
        };
        Ok(r)
    }

    /// `luaK_exp2val`.
    pub(crate) fn exp2val(&mut self, e: &mut Exp) -> R<()> {
        if e.has_jumps() {
            self.exp2anyreg(e)?;
            Ok(())
        } else {
            self.dischargevars(e)
        }
    }

    /// `luaK_exp2RK`: a constant among the first 256 as itself, anything else in a register.
    pub(crate) fn exp2rk(&mut self, e: &mut Exp) -> R<Rk> {
        self.exp2val(e)?;
        match e.k {
            Kind::KNum(_) | Kind::True | Kind::False | Kind::Nil => {
                if self.fsr().consts.len() as u32 <= MAXINDEXRK {
                    let k = match e.k {
                        Kind::Nil => self.nil_k()?,
                        Kind::KNum(n) => self.number_k(n)?,
                        k => {
                            // 5.0 loads a boolean into a register, or has its `OP_NOT` there.
                            if !e.rt {
                                self.fs().c50_delta += 1;
                            }
                            self.bool_k(k == Kind::True)?
                        }
                    };
                    e.k = Kind::K(k);
                    return Ok(k as Rk + RK_CONST);
                }
            }
            Kind::K(k) if k <= MAXINDEXRK => return Ok(k as Rk + RK_CONST),
            _ => {}
        }
        Ok(self.exp2anyreg(e)? as Rk)
    }

    /// `luaK_storevar`: `ex` stored into the variable `var`.
    pub(crate) fn storevar(&mut self, var: &Exp, ex: &mut Exp) -> R<()> {
        match var.k {
            Kind::Local(r) => {
                self.freeexp(ex);
                return self.exp2reg(ex, r);
            }
            Kind::Upval(u) => {
                let e = self.exp2anyreg(ex)?;
                self.code(Op::SetUpval(e, u), 1)?;
            }
            Kind::Global(k) => {
                let e = self.exp2anyreg(ex)?;
                self.code(Op::SetGlobal(e, k), 1)?;
            }
            Kind::Indexed(t, key) => {
                let e = self.exp2rk(ex)?;
                self.code_settable(t, key, e)?;
            }
            _ => unreachable!("not a variable"),
        }
        self.freeexp(ex);
        Ok(())
    }

    /// `OP_SETTABLE`, as `SetField` for a string constant key.
    pub(crate) fn code_settable(&mut self, t: Reg, key: Rk, v: Rk) -> R<usize> {
        let op = match self.str_k(key) {
            Some(k) => Op::SetField(t, k, v),
            None => Op::SetTable(t, key, v),
        };
        self.code(op, 1)
    }

    /// `luaK_self`: the method and its object in two fresh registers.
    pub(crate) fn self_(&mut self, e: &mut Exp, key: &mut Exp) -> R<()> {
        self.exp2anyreg(e)?;
        self.freeexp(e);
        let func = self.fsr().freereg;
        self.reserveregs(2)?;
        let Kind::NonReloc(obj) = e.k else {
            unreachable!()
        };
        let rk = self.exp2rk(key)?;
        let op = match self.str_k(rk) {
            Some(k) => Op::SelfField(func, obj, k),
            None => Op::SelfOp(func, obj, rk),
        };
        self.code(op, 1)?;
        self.freeexp(key);
        e.k = Kind::NonReloc(func);
        Ok(())
    }

    /// `invertjump`: a comparison's sense flipped.
    fn invertjump(&mut self, pc: usize) {
        let op = &mut self.fs().code[pc];
        *op = match *op {
            Op::JmpCmp(cj, b, c, o) => Op::JmpCmp(CmpJ::new(cj.cmp(), !cj.cond()), b, c, o),
            Op::JmpCmpK(cj, b, c, o) => Op::JmpCmpK(CmpJ::new(cj.cmp(), !cj.cond()), b, c, o),
            Op::JmpCmpKR(cj, b, c, o) => Op::JmpCmpKR(CmpJ::new(cj.cmp(), !cj.cond()), b, c, o),
            op => unreachable!("not a comparison: {op:?}"),
        };
    }

    /// `jumponcond`: a jump taken when `e`'s truth is `cond`. A `not` just emitted is dropped
    /// and its operand tested the other way.
    fn jumponcond(&mut self, e: &mut Exp, cond: bool) -> R<i32> {
        if let Kind::Reloc(pc) = e.k {
            if let Op::Not(_, b) = self.fsr().code[pc] {
                let fs = self.fs();
                fs.code.pop();
                fs.lines.pop();
                fs.cpc_next = fs.cpc.pop().unwrap();
                return Ok(self.code(Op::JmpIf(b, !cond, NO_JUMP), 2)? as i32);
            }
        }
        self.discharge2anyreg(e)?;
        self.freeexp(e);
        let Kind::NonReloc(r) = e.k else {
            unreachable!()
        };
        Ok(self.code(Op::TestSet(NO_REG, r, cond, NO_JUMP), 2)? as i32)
    }

    /// `luaK_goiftrue`: falls through when `e` is true, its false jumps kept.
    pub(crate) fn goiftrue(&mut self, e: &mut Exp) -> R<()> {
        self.dischargevars(e)?;
        if e.is_rt_numeral() {
            // 5.0 tests the register: `OP_TEST` and its jump.
            self.fs().c50_delta += 2;
        } else if e.k == Kind::False {
            // 5.0 always jumps (`0x7011ff`): one `OP_JMP` for 5.1's load, test and jump. For an
            // `OP_NOT` it tests the operand instead, as many, the load counted at the discharge.
            self.fs().c50_delta -= if e.rt { 1 } else { 2 };
        } else if e.k == Kind::True && e.rt {
            // 5.0 drops its `OP_NOT` for a test and its jump (`0x7012be`, `0x7012d9`).
            self.fs().c50_delta += 1;
        }
        let pc = match e.k {
            Kind::K(_) | Kind::KNum(_) | Kind::True => NO_JUMP,
            Kind::Jmp(pc) => {
                self.invertjump(pc);
                pc as i32
            }
            _ => self.jumponcond(e, false)?,
        };
        let mut f = e.f;
        self.concat(&mut f, pc)?;
        e.f = f;
        self.patchtohere(e.t)?;
        e.t = NO_JUMP;
        Ok(())
    }

    /// `luaK_goiffalse`.
    fn goiffalse(&mut self, e: &mut Exp) -> R<()> {
        self.dischargevars(e)?;
        if e.k == Kind::True {
            // 5.0 always jumps (`0x7013b4`): one `OP_JMP` for 5.1's load, test and jump. For an
            // `OP_NOT` it tests the operand instead, as many, the load counted at the discharge.
            self.fs().c50_delta -= if e.rt { 1 } else { 2 };
        } else if e.k == Kind::False && e.rt {
            // 5.0 drops its `OP_NOT` for a test and its jump (`0x7012be`, `0x7012d9`).
            self.fs().c50_delta += 1;
        }
        let pc = match e.k {
            Kind::Nil | Kind::False => NO_JUMP,
            Kind::Jmp(pc) => pc as i32,
            _ => self.jumponcond(e, true)?,
        };
        let mut t = e.t;
        self.concat(&mut t, pc)?;
        e.t = t;
        self.patchtohere(e.f)?;
        e.f = NO_JUMP;
        Ok(())
    }

    fn codenot(&mut self, e: &mut Exp) -> R<()> {
        self.dischargevars(e)?;
        if e.is_rt() {
            // 5.0's `OP_NOT` of the register; the result stays one ([`Exp::rt`]).
            self.fs().c50_delta += 1;
        }
        match e.k {
            Kind::Nil | Kind::False => e.k = Kind::True,
            Kind::K(_) | Kind::KNum(_) | Kind::True => e.k = Kind::False,
            Kind::Jmp(pc) => self.invertjump(pc),
            Kind::Reloc(_) | Kind::NonReloc(_) => {
                self.discharge2anyreg(e)?;
                self.freeexp(e);
                let Kind::NonReloc(r) = e.k else {
                    unreachable!()
                };
                e.k = Kind::Reloc(self.code(Op::Not(0, r), 1)?);
            }
            _ => unreachable!("cannot happen"),
        }
        std::mem::swap(&mut e.f, &mut e.t);
        self.removevalues(e.f);
        self.removevalues(e.t);
        Ok(())
    }

    /// `luaK_indexed`: `t[k]`, `t` already in a register.
    pub(crate) fn indexed(&mut self, t: &mut Exp, k: &mut Exp) -> R<()> {
        let Kind::NonReloc(r) = t.k else {
            unreachable!("the table is in a register")
        };
        let key = self.exp2rk(k)?;
        t.k = Kind::Indexed(r, key);
        Ok(())
    }

    /// `codearith`: folded when both are numerals, else the op for the operands' shapes.
    fn codearith(&mut self, op: BinOpr, e1: &mut Exp, e2: &mut Exp) -> R<()> {
        if let (Some(a), Some(b)) = (e1.is_numeral(), e2.is_numeral()) {
            if let Some(r) = fold(op, a, b) {
                // 5.0's arithmetic instruction.
                self.fs().c50_delta += 1;
                e1.k = Kind::KNum(r);
                e1.rt = true;
                return Ok(());
            }
        }
        let o2 = self.exp2rk(e2)?;
        let o1 = self.exp2rk(e1)?;
        if o1 > o2 {
            self.freeexp(e1);
            self.freeexp(e2);
        } else {
            self.freeexp(e2);
            self.freeexp(e1);
        }
        if op == BinOpr::Pow {
            e1.k = Kind::Reloc(self.code(Op::Pow(0, o1, o2), 1)?);
            return Ok(());
        }
        if op == BinOpr::Concat {
            e1.k = Kind::Reloc(self.code(Op::Concat(0, o1 as Reg, o2 as Reg), 1)?);
            return Ok(());
        }
        type Ops = (
            fn(Reg, Reg, Reg) -> Op,
            fn(Reg, Reg, u16) -> Op,
            fn(Reg, u16, Reg) -> Op,
        );
        let (rr, rk, kr): Ops = match op {
            BinOpr::Add => (Op::Add, Op::AddK, Op::AddKR),
            BinOpr::Sub => (Op::Sub, Op::SubK, Op::SubKR),
            BinOpr::Mul => (Op::Mul, Op::MulK, Op::MulKR),
            _ => (Op::Div, Op::DivK, Op::DivKR),
        };
        let k = |x: Rk| x - RK_CONST;
        let ins = match (is_k(o1), is_k(o2)) {
            (false, false) => rr(0, o1 as Reg, o2 as Reg),
            (false, true) => rk(0, o1 as Reg, k(o2)),
            (true, false) => kr(0, k(o1), o2 as Reg),
            (true, true) => {
                let s = self.scratch();
                self.code(Op::LoadK(s, k(o1) as u32), 0)?;
                rk(0, s, k(o2))
            }
        };
        e1.k = Kind::Reloc(self.code(ins, 1)?);
        Ok(())
    }

    /// `codecomp`: a fused comparison and jump, `>` and `>=` swapped into `<` and `<=`.
    fn codecomp(&mut self, cmp: Cmp, cond: bool, e1: &mut Exp, e2: &mut Exp) -> R<()> {
        let mut o1 = self.exp2rk(e1)?;
        let mut o2 = self.exp2rk(e2)?;
        self.freeexp(e2);
        self.freeexp(e1);
        let mut cond = cond;
        if !cond && cmp != Cmp::Eq {
            std::mem::swap(&mut o1, &mut o2);
            cond = true;
        }
        let cj = CmpJ::new(cmp, cond);
        let k = |x: Rk| (x - RK_CONST) as u8;
        let ins = match (is_k(o1), is_k(o2)) {
            (false, false) => Op::JmpCmp(cj, o1 as Reg, o2 as Reg, NO_JUMP),
            (false, true) => Op::JmpCmpK(cj, o1 as Reg, k(o2), NO_JUMP),
            (true, false) => Op::JmpCmpKR(cj, k(o1), o2 as Reg, NO_JUMP),
            (true, true) => {
                let s = self.scratch();
                self.code(Op::LoadK(s, k(o1) as u32), 0)?;
                Op::JmpCmpK(cj, s, k(o2), NO_JUMP)
            }
        };
        e1.k = Kind::Jmp(self.code(ins, 2)?);
        Ok(())
    }

    /// `luaK_prefix`.
    pub(crate) fn prefix(&mut self, op: UnOpr, e: &mut Exp) -> R<()> {
        match op {
            UnOpr::Minus => {
                if let Some(n) = e.is_numeral() {
                    if e.rt {
                        // 5.0's `OP_UNM` of an instruction's result.
                        self.fs().c50_delta += 1;
                    } else {
                        // 5.0's constant of the negated numeral.
                        self.number_k50(-n);
                    }
                    e.k = Kind::KNum(-n);
                    return Ok(());
                }
                let r = self.exp2anyreg(e)?;
                self.freeexp(e);
                e.k = Kind::Reloc(self.code(Op::Unm(0, r), 1)?);
                Ok(())
            }
            UnOpr::Not => self.codenot(e),
        }
    }

    /// `luaK_infix`: the left operand made ready before the right one is read.
    pub(crate) fn infix(&mut self, op: BinOpr, v: &mut Exp) -> R<()> {
        match op {
            BinOpr::And => self.goiftrue(v),
            BinOpr::Or => self.goiffalse(v),
            BinOpr::Concat => self.exp2nextreg(v),
            BinOpr::Add | BinOpr::Sub | BinOpr::Mul | BinOpr::Div | BinOpr::Pow => {
                if v.is_numeral().is_none() {
                    self.exp2rk(v)?;
                }
                Ok(())
            }
            _ => {
                self.exp2rk(v)?;
                Ok(())
            }
        }
    }

    /// `luaK_posfix`.
    pub(crate) fn posfix(&mut self, op: BinOpr, e1: &mut Exp, e2: &mut Exp) -> R<()> {
        match op {
            BinOpr::And => {
                debug_assert_eq!(e1.t, NO_JUMP);
                self.dischargevars(e2)?;
                let mut f = e2.f;
                self.concat(&mut f, e1.f)?;
                e2.f = f;
                *e1 = *e2;
            }
            BinOpr::Or => {
                debug_assert_eq!(e1.f, NO_JUMP);
                self.dischargevars(e2)?;
                let mut t = e2.t;
                self.concat(&mut t, e1.t)?;
                e2.t = t;
                *e1 = *e2;
            }
            BinOpr::Concat => {
                self.exp2val(e2)?;
                if let Kind::Reloc(pc) = e2.k {
                    if let Op::Concat(_, b, _) = &mut self.fs().code[pc] {
                        let Kind::NonReloc(r) = e1.k else {
                            unreachable!()
                        };
                        debug_assert_eq!(r + 1, *b);
                        *b = r;
                        self.freeexp(e1);
                        e1.k = Kind::Reloc(pc);
                        return Ok(());
                    }
                }
                self.exp2nextreg(e2)?;
                self.codearith(op, e1, e2)?;
            }
            BinOpr::Add | BinOpr::Sub | BinOpr::Mul | BinOpr::Div | BinOpr::Pow => {
                self.codearith(op, e1, e2)?
            }
            BinOpr::Eq => self.codecomp(Cmp::Eq, true, e1, e2)?,
            BinOpr::Ne => self.codecomp(Cmp::Eq, false, e1, e2)?,
            BinOpr::Lt => self.codecomp(Cmp::Lt, true, e1, e2)?,
            BinOpr::Le => self.codecomp(Cmp::Le, true, e1, e2)?,
            BinOpr::Gt => self.codecomp(Cmp::Lt, false, e1, e2)?,
            BinOpr::Ge => self.codecomp(Cmp::Le, false, e1, e2)?,
        }
        Ok(())
    }

    /// `luaK_setlist`: list items `base + 1..` stored into the table at `base`, `nelems` of them
    /// so far, the last `tostore` pending (-1: up to the top).
    pub(crate) fn setlist(&mut self, base: Reg, nelems: u32, tostore: i32) -> R<()> {
        // The batch's base, as `OP_SETLIST` clears the low bits of `Bx = nelems - 1` (`0x6f9593`):
        // a word every 32 items, as the client's 5.0 emits them.
        let first = (nelems - 1) / FIELDS_PER_FLUSH * FIELDS_PER_FLUSH;
        let op = if tostore < 0 {
            Op::SetListTop(base, first)
        } else {
            Op::SetList(base, first, tostore as u16)
        };
        self.code(op, 1)?;
        self.fs().freereg = base + 1;
        Ok(())
    }

    // ── functions ────────────────────────────────────────────────────────────────────────────

    /// `registerlocalvar`: a declaration, not yet in scope.
    pub(crate) fn registerlocalvar(&mut self, name: &[u8]) -> R<u16> {
        if self.fsr().locvars.len() >= MAX_LOCVARS {
            // `luaM_growvector`'s `luaG_runerror`.
            return Err(SyntaxError(b"too many local variables".to_vec()));
        }
        let fs = self.fs();
        let name: Bytes = Rc::from(name);
        fs.count_locvar(&name);
        fs.locvars.push(LocVar {
            name,
            reg: 0,
            startpc: 0,
            endpc: 0,
        });
        Ok((fs.locvars.len() - 1) as u16)
    }

    /// `pushclosure`: the finished function as a closure of this one.
    pub(crate) fn pushclosure(&mut self, p: Proto) -> R<Exp> {
        if self.fsr().protos.len() >= MAXARG_BX {
            return Err(SyntaxError(b"constant table overflow".to_vec()));
        }
        // 5.1 follows `CLOSURE` with a pseudo-instruction per upvalue.
        let words = 1 + p.upvals.len() as u32;
        let fs = self.fs();
        fs.protos.push(Rc::new(p));
        let idx = (fs.protos.len() - 1) as u32;
        Ok(Exp::new(Kind::Reloc(
            self.code(Op::Closure(0, idx), words)?,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// 5.0's floating point bytes (`0x6f5810`): exact below 8, then rounded up to 3 significant
    /// bits, where 5.1's keep 4.
    fn constructor_hints_are_5_0_s() {
        assert_eq!(fb2int(int2fb(0)), 0);
        assert_eq!(fb2int(int2fb(7)), 7);
        assert_eq!(fb2int(int2fb(8)), 8);
        assert_eq!(fb2int(int2fb(9)), 10);
        assert_eq!(fb2int(int2fb(15)), 16);
        assert_eq!(fb2int(int2fb(17)), 20);
        assert_eq!(fb2int(int2fb(33)), 40);
        assert_eq!(int2fb(6), 6);
    }
}
