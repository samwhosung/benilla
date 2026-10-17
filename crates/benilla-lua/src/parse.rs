//! The parser: 5.1.5's `lparser.c` with the 1.12 grammar (`GRAMMAR.md`), generating code as it
//! reads, through [`crate::compile`]. Messages and the token they quote are 5.1's, but for the
//! limits, which are the client's 5.0 ones. Left-associative chains (binary operators,
//! `and`/`or`, suffixes such as `f()()`, `t.a.b`, `o:m()`) are read in loops, so only nesting
//! recurses, and nesting is bounded by the client's level limit.

use std::rc::Rc;

use crate::compile::{BinOpr, BlockCnt, Exp, FuncState, Kind, UnOpr, NO_JUMP, R};
use crate::heap::Heap;
use crate::lex::{Lexer, Tok};
use crate::proto::*;

/// The client's `MAXVARS` (`0x6fd8fc`): active locals per function.
const MAXVARS: usize = 200;
/// The client's `MAXUPVALUES`, 5.0's 32 (`0x6fdfdc`).
const MAXUPVALUES: usize = 32;
/// The client's `LUA_MAXPARAMS`, 5.0's 100 (`0x6fdac7`): a function's parameters, `self` among
/// them and `arg` not.
const MAXPARAMS: usize = 100;
/// The client's `MAXARG_Bx` (`0x6fd7a3`): list items in one table constructor.
const MAXLISTITEMS: u32 = 0x3ffff;
/// The client's `LUA_MAXPARSERLEVEL`, 5.0's 200 (`0x6fcc99`, `0x6fd180`): nested syntax levels,
/// counted from 0 in each chunk (`0x6fc9d0`).
const MAXPARSERLEVEL: u32 = 200;
/// 5.1's `UNARY_PRIORITY`.
const UNARY_PRIORITY: u8 = 8;

pub(crate) struct Parser<'a> {
    pub(crate) lx: Lexer<'a>,
    /// The function being compiled and those enclosing it, innermost last.
    pub(crate) fs: Vec<FuncState>,
    pub(crate) heap: &'a mut Heap,
    /// 5.0's `nestlevel`: the syntax levels open in this chunk.
    nestlevel: u32,
    /// The name of the single target the next expression is assigned to, for the key of a
    /// function or constructor it opens with (`t.x = function`, `local f = function`).
    name_hint: Option<Vec<u8>>,
}

/// `sep` then `name` onto a function's name for its key, cut where a key cuts a name
/// ([`KEY_NAME_MAX`]): a name built along a long chain or deep constructors costs no more.
fn join_name(out: &mut Vec<u8>, sep: &[u8], name: &[u8]) {
    for part in [sep, name] {
        let room = KEY_NAME_MAX.saturating_sub(out.len());
        out.extend_from_slice(&part[..part.len().min(room)]);
    }
}

/// 5.1's `ConsControl`.
struct ConsControl {
    /// The last list item read.
    v: Exp,
    /// The table's register.
    t: Reg,
    nh: u32,
    na: u32,
    tostore: u32,
    /// The name the table is assigned to, which its `name = function` fields' keys extend.
    prefix: Option<Vec<u8>>,
}

/// `luaY_parser`: a chunk compiled into its main function, which is vararg but declares no
/// `arg` (only `parlist` does), so `arg` at chunk level is a global.
pub(crate) fn parse(src: &[u8], chunkname: &[u8], heap: &mut Heap) -> R<Rc<Proto>> {
    let lx = Lexer::new(src, chunkname)?;
    let mut p = Parser {
        lx,
        fs: Vec::new(),
        heap,
        nestlevel: 0,
        name_hint: None,
    };
    p.fs.push(FuncState::new(Rc::from(chunkname), 0, Key::default()));
    p.chunk()?;
    // The client's `luaY_parser` raises its own bare text past the last statement (`0x6fca1a`,
    // `0x871cf4`).
    if p.tok() != Tok::Eof {
        return Err(p.lx.error("<eof> expected"));
    }
    let fs = p.close_func()?;
    let mut root = fs.into_proto(p.heap);
    crate::compile::finish_keys(&mut root);
    root.verify();
    Ok(Rc::new(root))
}

fn block_follow(t: Tok) -> bool {
    matches!(
        t,
        Tok::Else | Tok::Elseif | Tok::End | Tok::Until | Tok::Eof
    )
}

fn getunopr(t: Tok) -> Option<UnOpr> {
    match t {
        Tok::Not => Some(UnOpr::Not),
        Tok::Char(b'-') => Some(UnOpr::Minus),
        _ => None,
    }
}

/// The 1.12 grammar's binary operators: 5.1's without `%`.
fn getbinopr(t: Tok) -> Option<BinOpr> {
    Some(match t {
        Tok::Char(b'+') => BinOpr::Add,
        Tok::Char(b'-') => BinOpr::Sub,
        Tok::Char(b'*') => BinOpr::Mul,
        Tok::Char(b'/') => BinOpr::Div,
        Tok::Char(b'^') => BinOpr::Pow,
        Tok::Concat => BinOpr::Concat,
        Tok::Ne => BinOpr::Ne,
        Tok::Eq => BinOpr::Eq,
        Tok::Char(b'<') => BinOpr::Lt,
        Tok::Le => BinOpr::Le,
        Tok::Char(b'>') => BinOpr::Gt,
        Tok::Ge => BinOpr::Ge,
        Tok::And => BinOpr::And,
        Tok::Or => BinOpr::Or,
        _ => return None,
    })
}

/// 5.1's `priority`: (left, right); `^` and `..` are right associative.
fn priority(op: BinOpr) -> (u8, u8) {
    match op {
        BinOpr::Add | BinOpr::Sub => (6, 6),
        BinOpr::Mul | BinOpr::Div => (7, 7),
        BinOpr::Pow => (10, 9),
        BinOpr::Concat => (5, 4),
        BinOpr::Eq | BinOpr::Ne | BinOpr::Lt | BinOpr::Le | BinOpr::Gt | BinOpr::Ge => (3, 3),
        BinOpr::And => (2, 2),
        BinOpr::Or => (1, 1),
    }
}

impl<'a> Parser<'a> {
    // ── tokens ───────────────────────────────────────────────────────────────────────────────

    fn tok(&self) -> Tok {
        self.lx.cur.tok
    }

    fn next(&mut self) -> R<()> {
        self.lx.advance()
    }

    fn testnext(&mut self, t: Tok) -> R<bool> {
        if self.tok() == t {
            self.next()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn error_expected(&self, t: Tok) -> crate::SyntaxError {
        self.lx.error(&format!(
            "`{}' expected",
            String::from_utf8_lossy(&t.text())
        ))
    }

    fn check(&mut self, t: Tok) -> R<()> {
        if self.tok() != t {
            return Err(self.error_expected(t));
        }
        Ok(())
    }

    fn checknext(&mut self, t: Tok) -> R<()> {
        self.check(t)?;
        self.next()
    }

    fn check_match(&mut self, what: Tok, who: Tok, line: u32) -> R<()> {
        if !self.testnext(what)? {
            if line == self.lx.line {
                return Err(self.error_expected(what));
            }
            return Err(self.lx.error(&format!(
                "`{}' expected (to close `{}' at line {})",
                String::from_utf8_lossy(&what.text()),
                String::from_utf8_lossy(&who.text()),
                line
            )));
        }
        Ok(())
    }

    /// `str_checkname`: the name just read, as the source spells it.
    fn str_checkname(&mut self) -> R<&'a [u8]> {
        self.check(Tok::Name)?;
        let name = self.lx.name();
        self.next()?;
        Ok(name)
    }

    /// The client's `luaX_checklimit` (`0x6ff440`) past its limit: `too many %s (limit=%d)`
    /// (`0x872164`), a syntax error near the current token (`0x6ff467`).
    fn errorlimit(&self, limit: usize, what: &str) -> crate::SyntaxError {
        self.lx.error(&format!("too many {what} (limit={limit})"))
    }

    /// 5.0's `enterlevel` (`0x6fcc90`, `0x6fd170`): a syntax error near the current token past
    /// [`MAXPARSERLEVEL`].
    fn enterlevel(&mut self) -> R<()> {
        self.nestlevel += 1;
        if self.nestlevel > MAXPARSERLEVEL {
            return Err(self.lx.error("too many syntax levels"));
        }
        Ok(())
    }

    fn leavelevel(&mut self) {
        self.nestlevel -= 1;
    }

    // ── scopes ───────────────────────────────────────────────────────────────────────────────

    /// `new_localvar`: the `n`th name of a declaration, not yet in scope.
    fn new_localvar(&mut self, name: &[u8], n: usize) -> R<()> {
        let nactvar = self.fsr().nactvar as usize;
        if nactvar + n + 1 > MAXVARS {
            return Err(self.errorlimit(MAXVARS, "local variables"));
        }
        let idx = self.registerlocalvar(name)?;
        let fs = self.fs();
        fs.actvar.resize(nactvar + n + 1, 0);
        fs.actvar[nactvar + n] = idx;
        Ok(())
    }

    /// `adjustlocalvars`: the next `nvars` declared locals come into scope.
    fn adjustlocalvars(&mut self, nvars: usize) {
        let fs = self.fs();
        let pc = fs.pc() as u32;
        for _ in 0..nvars {
            let reg = fs.nactvar;
            let v = &mut fs.locvars[fs.actvar[reg as usize] as usize];
            v.reg = reg;
            v.startpc = pc;
            fs.nactvar += 1;
        }
    }

    /// `removevars`.
    fn removevars(&mut self, tolevel: u8) {
        let fs = self.fs();
        let pc = fs.pc() as u32;
        while fs.nactvar > tolevel {
            fs.nactvar -= 1;
            fs.locvars[fs.actvar[fs.nactvar as usize] as usize].endpc = pc;
        }
        fs.actvar.truncate(fs.nactvar as usize);
    }

    fn enterblock(&mut self, isbreakable: bool) {
        // `freereg == nactvar` does not hold here in a generic for given more values than it has
        // locals (`for k in f, s, c, x do`): `forlist` adjusts them without freeing the rest.
        let fs = self.fs();
        let nactvar = fs.nactvar;
        fs.bl.push(BlockCnt {
            breaklist: NO_JUMP,
            nactvar,
            upval: false,
            isbreakable,
        });
    }

    fn leaveblock(&mut self) -> R<()> {
        let bl = self.fs().bl.pop().unwrap();
        self.removevars(bl.nactvar);
        if bl.upval {
            self.code(Op::Close(bl.nactvar), 1)?;
        }
        let fs = self.fs();
        fs.freereg = fs.nactvar;
        self.patchtohere(bl.breaklist)
    }

    /// `searchvar` in function `level`: the active local of that name, innermost first.
    fn searchvar(&self, level: usize, name: &[u8]) -> Option<u8> {
        let fs = &self.fs[level];
        (0..fs.nactvar)
            .rev()
            .find(|&i| *fs.locvars[fs.actvar[i as usize] as usize].name == *name)
    }

    /// `markupval`: the block holding local `level` closes its upvalues on exit.
    fn markupval(&mut self, level: usize, local: u8) {
        if let Some(bl) = self.fs[level]
            .bl
            .iter_mut()
            .rev()
            .find(|bl| bl.nactvar <= local)
        {
            bl.upval = true;
        }
    }

    /// `singlevaraux`: a local, an upvalue or (`None`) a global, at function `level`.
    fn singlevaraux(&mut self, level: usize, name: &[u8], base: bool) -> R<Option<Kind>> {
        if let Some(v) = self.searchvar(level, name) {
            if !base {
                self.markupval(level, v);
            }
            return Ok(Some(Kind::Local(v)));
        }
        if level == 0 {
            return Ok(None);
        }
        let Some(k) = self.singlevaraux(level - 1, name, false)? else {
            return Ok(None);
        };
        // `indexupvalue`: an existing upvalue for the same variable is reused.
        let key = match k {
            Kind::Local(r) => (true, r as u16),
            Kind::Upval(u) => (false, u),
            _ => unreachable!(),
        };
        let fs = &self.fs[level];
        if let Some(i) = fs.upvals.iter().position(|&u| u == key) {
            return Ok(Some(Kind::Upval(i as u16)));
        }
        if fs.upvals.len() + 1 > MAXUPVALUES {
            return Err(self.errorlimit(MAXUPVALUES, "upvalues"));
        }
        let decl = match k {
            Kind::Local(r) => self.fs[level - 1].local_decl(r),
            _ => self.fs[level - 1].upval_decls[key.1 as usize].clone(),
        };
        let fs = &mut self.fs[level];
        fs.upvals.push(key);
        fs.upval_names.push(Rc::from(name));
        fs.upval_decls.push(decl);
        Ok(Some(Kind::Upval((fs.upvals.len() - 1) as u16)))
    }

    /// `singlevar`: the name just read. Answers its kind and the name.
    fn singlevar(&mut self) -> R<(Exp, &'a [u8])> {
        let name = self.str_checkname()?;
        let top = self.fs.len() - 1;
        let k = match self.singlevaraux(top, name, true)? {
            Some(k) => k,
            None => Kind::Global(self.string_k(name)?),
        };
        Ok((Exp::new(k), name))
    }

    /// `adjust_assign`: `nvars` values from `nexps` expressions, the last one `e`.
    fn adjust_assign(&mut self, nvars: usize, nexps: usize, e: &mut Exp) -> R<()> {
        let mut extra = nvars as i32 - nexps as i32;
        if e.has_multret() {
            extra += 1;
            if extra < 0 {
                extra = 0;
            }
            self.setreturns(e, extra);
            if extra > 1 {
                self.reserveregs(extra as u32 - 1)?;
            }
        } else {
            if e.k != Kind::Void {
                self.exp2nextreg(e)?;
            }
            if extra > 0 {
                let reg = self.fsr().freereg;
                self.reserveregs(extra as u32)?;
                self.nil(reg, extra as u32)?;
            }
        }
        Ok(())
    }

    // ── functions ────────────────────────────────────────────────────────────────────────────

    /// `open_func`, for a function whose name (for its key) is `name`.
    fn open_func(&mut self, line: u32, name: Option<Vec<u8>>) {
        let source = self.fsr().source.clone();
        let key = self.fs().child_key(name);
        self.fs.push(FuncState::new(source, line, key));
    }

    /// `close_func`: the final return, the locals' ends.
    fn close_func(&mut self) -> R<FuncState> {
        self.removevars(0);
        self.ret(0, 0)?;
        Ok(self.fs.pop().unwrap())
    }

    /// `body`: `(` parlist `)` chunk `end`, as a closure.
    fn body(&mut self, needself: bool, line: u32, name: Option<Vec<u8>>) -> R<Exp> {
        self.open_func(line, name);
        self.checknext(Tok::Char(b'('))?;
        if needself {
            self.new_localvar(b"self", 0)?;
            self.adjustlocalvars(1);
        }
        self.parlist()?;
        self.checknext(Tok::Char(b')'))?;
        self.chunk()?;
        self.check_match(Tok::End, Tok::Function, line)?;
        self.end_body()
    }

    /// The finished function as a closure, in a frame of its own: the function state and the
    /// prototype are large, and this keeps them off the recursion's stack.
    #[inline(never)]
    fn end_body(&mut self) -> R<Exp> {
        let fs = self.close_func()?;
        let p = fs.into_proto(self.heap);
        self.pushclosure(p)
    }

    /// `parlist`, then the client's `code_params` (`0x6fdab0`): the count is checked once
    /// `self` and the named parameters are in scope (`0x6fdacb`), and only then is `...`'s local
    /// `arg` made (`0x6fdaeb`).
    fn parlist(&mut self) -> R<()> {
        let mut nparams = 0;
        if self.tok() != Tok::Char(b')') {
            loop {
                match self.tok() {
                    Tok::Name => {
                        let n = self.str_checkname()?;
                        self.new_localvar(n, nparams)?;
                        nparams += 1;
                    }
                    Tok::Dots => {
                        self.next()?;
                        self.fs().vararg = true;
                    }
                    _ => return Err(self.lx.error("<name> or `...' expected")),
                }
                if self.fsr().vararg || !self.testnext(Tok::Char(b','))? {
                    break;
                }
            }
        }
        self.adjustlocalvars(nparams);
        if self.fsr().nactvar as usize > MAXPARAMS {
            return Err(self.errorlimit(MAXPARAMS, "parameters"));
        }
        self.fs().nparams = self.fsr().nactvar;
        if self.fsr().vararg {
            self.new_localvar(b"arg", 0)?;
            self.adjustlocalvars(1);
        }
        let n = self.fsr().nactvar as u32;
        self.reserveregs(n)
    }

    // ── statements ───────────────────────────────────────────────────────────────────────────

    /// `chunk`: statements until a block follows; `return` and `break` end it.
    fn chunk(&mut self) -> R<()> {
        self.enterlevel()?;
        let mut islast = false;
        while !islast && !block_follow(self.tok()) {
            islast = self.statement()?;
            self.testnext(Tok::Char(b';'))?;
            let fs = self.fs();
            debug_assert!(fs.maxstack >= fs.freereg && fs.freereg >= fs.nactvar);
            fs.freereg = fs.nactvar;
        }
        self.leavelevel();
        Ok(())
    }

    fn block(&mut self) -> R<()> {
        self.enterblock(false);
        self.chunk()?;
        self.leaveblock()
    }

    fn statement(&mut self) -> R<bool> {
        let line = self.lx.line;
        match self.tok() {
            Tok::If => self.ifstat(line)?,
            Tok::While => self.whilestat(line)?,
            Tok::Do => {
                self.next()?;
                self.block()?;
                self.check_match(Tok::End, Tok::Do, line)?;
            }
            Tok::For => self.forstat(line)?,
            Tok::Repeat => self.repeatstat(line)?,
            Tok::Function => self.funcstat(line)?,
            Tok::Local => {
                self.next()?;
                if self.testnext(Tok::Function)? {
                    self.localfunc()?;
                } else {
                    self.localstat()?;
                }
            }
            Tok::Return => {
                self.retstat()?;
                return Ok(true);
            }
            Tok::Break => {
                self.next()?;
                self.breakstat()?;
                return Ok(true);
            }
            _ => self.exprstat()?,
        }
        Ok(false)
    }

    /// `cond`: the jumps taken when the condition is false.
    fn cond(&mut self) -> R<i32> {
        let v = self.expr()?;
        self.cond_of(v)
    }

    /// [`Parser::cond`] of an expression already read.
    fn cond_of(&mut self, mut v: Exp) -> R<i32> {
        if v.k == Kind::Nil {
            v.k = Kind::False;
        }
        self.goiftrue(&mut v)?;
        Ok(v.f)
    }

    fn breakstat(&mut self) -> R<()> {
        let fs = self.fsr();
        let mut upval = false;
        let mut found = None;
        for (i, bl) in fs.bl.iter().enumerate().rev() {
            if bl.isbreakable {
                found = Some(i);
                break;
            }
            upval |= bl.upval;
        }
        let Some(i) = found else {
            return Err(self.lx.error("no loop to break"));
        };
        if upval {
            let n = self.fsr().bl[i].nactvar;
            self.code(Op::Close(n), 1)?;
        }
        let j = self.jump()?;
        let mut l = self.fsr().bl[i].breaklist;
        self.concat(&mut l, j)?;
        self.fs().bl[i].breaklist = l;
        Ok(())
    }

    /// 5.1's layout, the body jumping back to the condition. The client's `whilestat`
    /// (`0x6fcfd0`) copies the condition's code into a buffer of 100 instructions, to emit after
    /// the body, and a longer condition is `` `while' condition too complex `` (`0x6fd03b`,
    /// `0x871d8c`) near the token after it. The length is 5.0's
    /// ([`crate::compile::FuncState::code_size50`]), with its own test at the end: a constant
    /// condition is `true` there (`0x6fd00a`), which `luaK_goiffalse` (`0x6fd022`) jumps back on,
    /// and a false or nil one jumps nowhere, where 5.1's `cond` jumps out on false or nil. Two
    /// gaps of an instruction remain: a constant at index 256 to 261, an operand in 5.0
    /// (`index + 250 <= 511`, `0x701028`) and loaded in 5.1; and a table constructor, which 5.0
    /// flushes every 32 items (`0x6fd6d0`) and 5.1 every 50.
    fn whilestat(&mut self, line: u32) -> R<()> {
        self.next()?;
        let whileinit = self.getlabel();
        let start = self.fsr().code_size50();
        let v = self.expr()?;
        // 5.0's last jump less the one counted for 5.1's: each tests anything else alike.
        let last = match v.k {
            _ if v.is_rt_numeral() => 0,
            // 5.0 tests the operand of its `OP_NOT` (`0x7012be`, `0x7012d9`), as 5.1's is counted.
            Kind::True | Kind::False if v.rt => 0,
            Kind::K(_) | Kind::KNum(_) | Kind::True => 1,
            Kind::Nil | Kind::False => -1,
            _ => 0,
        };
        let condexit = self.cond_of(v)?;
        if self.fsr().code_size50() - start + last > 100 {
            return Err(self.lx.error("`while' condition too complex"));
        }
        self.enterblock(true);
        self.checknext(Tok::Do)?;
        self.block()?;
        let j = self.jump()?;
        self.patchlist(j, whileinit)?;
        self.check_match(Tok::End, Tok::While, line)?;
        self.leaveblock()?;
        self.patchtohere(condexit)
    }

    fn repeatstat(&mut self, line: u32) -> R<()> {
        let repeat_init = self.getlabel();
        self.enterblock(true);
        self.enterblock(false);
        self.next()?;
        self.chunk()?;
        self.check_match(Tok::Until, Tok::Repeat, line)?;
        let condexit = self.cond()?;
        if !self.fsr().bl.last().unwrap().upval {
            self.leaveblock()?;
            self.patchlist(condexit, repeat_init)?;
        } else {
            self.breakstat()?;
            self.patchtohere(condexit)?;
            self.leaveblock()?;
            let j = self.jump()?;
            self.patchlist(j, repeat_init)?;
        }
        self.leaveblock()
    }

    /// `exp1`: an expression into the next register; answers what it was before.
    fn exp1(&mut self) -> R<Kind> {
        let mut e = self.expr()?;
        let k = e.k;
        self.exp2nextreg(&mut e)?;
        Ok(k)
    }

    /// The client's `forbody` (`0x6fe480`), after the jump `prep` that enters the loop at its
    /// test: the loop's locals come into scope (`0x6fe493`), the body runs in a block `break`
    /// leaves (`0x6fe4ab`), and the test ends the loop. Each loop variable is one local for the
    /// whole loop, so every closure the body makes shares it and sees its value at the exit.
    /// `nvars` counts the loop's locals: a numeric loop's three, a generic loop's two hidden ones
    /// and its names. A numeric loop whose step is a positive constant counts up with
    /// `ForLoopUp`.
    fn forbody(
        &mut self,
        base: Reg,
        line: u32,
        nvars: usize,
        num: Option<bool>,
        prep: usize,
    ) -> R<()> {
        self.adjustlocalvars(nvars);
        self.checknext(Tok::Do)?;
        self.enterblock(true);
        self.block()?;
        self.patchtohere(prep as i32)?;
        let endfor = match num {
            Some(true) => self.code(Op::ForLoopUp(base, NO_JUMP), 1)?,
            Some(false) => self.code(Op::ForLoop(base, NO_JUMP), 1)?,
            // `C` is the names less one (`0x6fe4ea`); here the count of names.
            None => self.code(Op::TForLoop(base, (nvars - 2) as u8), 1)?,
        };
        self.fixline(line);
        let back = match num {
            Some(_) => endfor as i32,
            None => self.jump()?,
        };
        self.patchlist(back, prep + 1)?;
        self.leaveblock()
    }

    /// The client's `fornum` (`0x6fe380`): the loop's own name is its counter, beside
    /// `(for limit)` and `(for step)` (`0x6fe390`..`0x6fe3ac`); the step is taken off the start
    /// by a plain subtraction (`0x6fe42c`), and a jump enters the loop at its test.
    fn fornum(&mut self, varname: &[u8], line: u32) -> R<()> {
        let base = self.fsr().freereg;
        self.new_localvar(varname, 0)?;
        self.new_localvar(b"(for limit)", 1)?;
        self.new_localvar(b"(for step)", 2)?;
        self.checknext(Tok::Char(b'='))?;
        self.exp1()?;
        self.checknext(Tok::Char(b','))?;
        self.exp1()?;
        let up = if self.testnext(Tok::Char(b','))? {
            matches!(self.exp1()?, Kind::KNum(n) if n > 0.0)
        } else {
            let r = self.fsr().freereg;
            let k = self.number_k(1.0)?;
            self.code(Op::LoadK(r, k), 1)?;
            self.reserveregs(1)?;
            true
        };
        self.code(Op::Sub(base, base, base + 2), 1)?;
        let prep = self.jump()? as usize;
        self.forbody(base, line, 3, Some(up), prep)
    }

    /// The client's `forlist` (`0x6fe540`): two hidden locals, the generator and the state
    /// (`0x6fe554`..`0x6fe56c`), then the loop's names from the third (`0x6fe574`), the first
    /// of them the control the generator is passed. The values are adjusted to all of them
    /// (`0x6fe5db`), with room for the call above them (`0x6fe5e7`).
    fn forlist(&mut self, indexname: &[u8]) -> R<()> {
        let base = self.fsr().freereg;
        self.new_localvar(b"(for generator)", 0)?;
        self.new_localvar(b"(for state)", 1)?;
        self.new_localvar(indexname, 2)?;
        let mut nvars = 3;
        while self.testnext(Tok::Char(b','))? {
            let n = self.str_checkname()?;
            self.new_localvar(n, nvars)?;
            nvars += 1;
        }
        self.checknext(Tok::In)?;
        let line = self.lx.line;
        let (nexps, mut e) = self.explist1()?;
        // Values past the locals keep their registers: the body can read one unwritten.
        if nexps > nvars {
            self.fs().clears = true;
        }
        self.adjust_assign(nvars, nexps, &mut e)?;
        self.checkstack(3)?;
        // `OP_TFORLOOP` calls from above the variables and takes back a result per name there
        // (`0x6f9344`, `0x6f940f`): with more than three names they pass the three registers
        // checked here, into the slack the client's stack keeps (`EXTRA_STACK`).
        let cb = base as u16 + nvars as u16;
        let fs = self.fs();
        fs.tfor_top = fs.tfor_top.max(cb + (nvars as u16 - 2).max(3));
        // The client's `TFORPREP` (`0x6fe5fc`), which turns a table into `next` over it and
        // jumps to the `TFORLOOP`.
        let prep = self.code(Op::TForPrep(base, NO_JUMP), 1)?;
        self.forbody(base, line, nvars, None, prep)
    }

    /// The client's `forstat` (`0x6fe300`): the loop's locals live in a block of their own,
    /// outside the one `break` leaves, so their upvalues close after a `break` too.
    fn forstat(&mut self, line: u32) -> R<()> {
        self.enterblock(false);
        self.next()?;
        let varname = self.str_checkname()?;
        match self.tok() {
            Tok::Char(b'=') => self.fornum(varname, line)?,
            Tok::Char(b',') | Tok::In => self.forlist(varname)?,
            _ => return Err(self.lx.error("`=' or `in' expected")),
        }
        self.check_match(Tok::End, Tok::For, line)?;
        self.leaveblock()
    }

    fn test_then_block(&mut self) -> R<i32> {
        self.next()?;
        let condexit = self.cond()?;
        self.checknext(Tok::Then)?;
        self.block()?;
        Ok(condexit)
    }

    fn ifstat(&mut self, line: u32) -> R<()> {
        let mut escapelist = NO_JUMP;
        let mut flist = self.test_then_block()?;
        while self.tok() == Tok::Elseif {
            let j = self.jump()?;
            self.concat(&mut escapelist, j)?;
            self.patchtohere(flist)?;
            flist = self.test_then_block()?;
        }
        if self.tok() == Tok::Else {
            let j = self.jump()?;
            self.concat(&mut escapelist, j)?;
            self.patchtohere(flist)?;
            self.next()?;
            self.block()?;
        } else {
            self.concat(&mut escapelist, flist)?;
        }
        self.patchtohere(escapelist)?;
        self.check_match(Tok::End, Tok::If, line)
    }

    fn localfunc(&mut self) -> R<()> {
        let name = self.str_checkname()?;
        self.new_localvar(name, 0)?;
        let v = Exp::new(Kind::Local(self.fsr().freereg));
        self.reserveregs(1)?;
        self.adjustlocalvars(1);
        let mut key = b"local ".to_vec();
        key.extend_from_slice(name);
        let line = self.lx.line;
        let mut b = self.body(false, line, Some(key))?;
        self.storevar(&v, &mut b)?;
        // Debug information sees the variable only from here.
        let fs = self.fs();
        let pc = fs.pc() as u32;
        let i = fs.actvar[fs.nactvar as usize - 1];
        fs.locvars[i as usize].startpc = pc;
        Ok(())
    }

    fn localstat(&mut self) -> R<()> {
        let mut nvars = 0;
        loop {
            let n = self.str_checkname()?;
            self.new_localvar(n, nvars)?;
            nvars += 1;
            if !self.testnext(Tok::Char(b','))? {
                break;
            }
        }
        let (nexps, mut e) = if self.testnext(Tok::Char(b'='))? {
            if nvars == 1 {
                let fs = self.fsr();
                let name = &fs.locvars[fs.actvar[fs.nactvar as usize] as usize].name;
                let mut hint = b"local ".to_vec();
                join_name(&mut hint, b"", name);
                self.name_hint = Some(hint);
            }
            self.explist1()?
        } else {
            (0, Exp::new(Kind::Void))
        };
        self.adjust_assign(nvars, nexps, &mut e)?;
        self.adjustlocalvars(nvars);
        Ok(())
    }

    /// `funcname`: `NAME {'.' NAME} [':' NAME]`. Answers the target, whether the body takes
    /// `self`, and the function's name for its key, as written: `a.b.c`, `Frame:OnEvent`.
    fn funcname(&mut self) -> R<(Exp, bool, Vec<u8>)> {
        let (mut v, root) = self.singlevar()?;
        let mut name = Vec::new();
        join_name(&mut name, b"", root);
        let mut needself = false;
        loop {
            let sep = match self.tok() {
                Tok::Char(b'.') => b'.',
                Tok::Char(b':') if !needself => {
                    needself = true;
                    b':'
                }
                _ => break,
            };
            let field = self.field(&mut v)?;
            join_name(&mut name, &[sep], field);
            if needself {
                break;
            }
        }
        Ok((v, needself, name))
    }

    fn funcstat(&mut self, line: u32) -> R<()> {
        self.next()?;
        let (v, needself, name) = self.funcname()?;
        let mut b = self.body(needself, line, Some(name))?;
        self.storevar(&v, &mut b)?;
        self.fixline(line);
        Ok(())
    }

    /// `check_conflict`: a local assigned here that an earlier indexed target reads as its table
    /// or key is copied first, and that target reads the copy.
    fn check_conflict(&mut self, lhs: &mut [Exp], v: Reg) -> R<()> {
        let extra = self.fsr().freereg;
        let mut conflict = false;
        for e in lhs.iter_mut() {
            if let Kind::Indexed(t, key) = &mut e.k {
                if *t == v {
                    conflict = true;
                    *t = extra;
                }
                if *key == v as Rk {
                    conflict = true;
                    *key = extra as Rk;
                }
            }
        }
        if conflict {
            self.code(Op::Move(extra, v), 1)?;
            self.reserveregs(1)?;
        }
        Ok(())
    }

    /// `assignment`, its recursion over the targets as a loop: the stores run from the last
    /// target back to the first.
    fn assignment(&mut self, first: Exp, name: Option<Vec<u8>>) -> R<()> {
        let mut lhs = vec![first];
        loop {
            let v = lhs.last().unwrap();
            if !matches!(
                v.k,
                Kind::Local(_) | Kind::Upval(_) | Kind::Global(_) | Kind::Indexed(..)
            ) {
                return Err(self.lx.error("syntax error"));
            }
            if !self.testnext(Tok::Char(b','))? {
                break;
            }
            let nv = self.primaryexp()?;
            if let Kind::Local(r) = nv.k {
                self.check_conflict(&mut lhs, r)?;
            }
            lhs.push(nv);
        }
        self.checknext(Tok::Char(b'='))?;
        let nvars = lhs.len();
        if nvars == 1 {
            self.name_hint = name;
        }
        let (nexps, mut e) = self.explist1()?;
        let mut rest = nvars;
        if nexps != nvars {
            self.adjust_assign(nvars, nexps, &mut e)?;
            if nexps > nvars {
                self.fs().freereg -= (nexps - nvars) as u8;
            }
        } else {
            self.setoneret(&mut e);
            let last = lhs[nvars - 1];
            self.storevar(&last, &mut e)?;
            rest -= 1;
        }
        for v in lhs[..rest].iter().rev() {
            let mut e = Exp::new(Kind::NonReloc(self.fsr().freereg - 1));
            self.storevar(v, &mut e)?;
        }
        Ok(())
    }

    fn exprstat(&mut self) -> R<()> {
        let (v, name) = self.primaryexp_named(true)?;
        if let Kind::Call(pc) = v.k {
            // A call statement keeps no results.
            if let Op::Call(_, _, c) = &mut self.fs().code[pc] {
                *c = 1;
            }
            Ok(())
        } else {
            self.assignment(v, name)
        }
    }

    fn retstat(&mut self) -> R<()> {
        self.next()?;
        let (first, nret);
        if block_follow(self.tok()) || self.tok() == Tok::Char(b';') {
            first = 0;
            nret = 0;
        } else {
            let (n, mut e) = self.explist1()?;
            if e.has_multret() {
                self.setmultret(&e);
                if let (Kind::Call(pc), 1) = (e.k, n) {
                    // A tail call. 5.1 asserts its function sits at `nactvar`, which its
                    // release build does not check and which does not hold in the body of a
                    // generic `for` given more values than locals (see `enterblock`).
                    let fs = self.fs();
                    let Op::Call(a, b, _) = fs.code[pc] else {
                        unreachable!()
                    };
                    fs.code[pc] = Op::TailCall(a, b);
                }
                first = self.fsr().nactvar;
                nret = -1;
            } else if n == 1 {
                first = self.exp2anyreg(&mut e)?;
                nret = 1;
            } else {
                // 5.1 asserts the values went to `nactvar` on, unchecked in its release build
                // and untrue in a generic `for` given more values than locals: the return then
                // reads the registers from `nactvar`, as the C build's does.
                self.exp2nextreg(&mut e)?;
                first = self.fsr().nactvar;
                nret = n as i32;
            }
        }
        self.ret(first, nret)
    }

    // ── expressions ──────────────────────────────────────────────────────────────────────────

    /// `field`: `.name` or `:name` on a value made a register. Answers the name.
    fn field(&mut self, v: &mut Exp) -> R<&'a [u8]> {
        self.exp2anyreg(v)?;
        self.next()?;
        let name = self.str_checkname()?;
        let mut key = Exp::new(Kind::K(self.string_k(name)?));
        self.indexed(v, &mut key)?;
        Ok(name)
    }

    /// `yindex`: `[` expr `]`.
    fn yindex(&mut self) -> R<Exp> {
        self.next()?;
        let mut v = self.expr()?;
        self.exp2val(&mut v)?;
        self.checknext(Tok::Char(b']'))?;
        Ok(v)
    }

    fn recfield(&mut self, cc: &mut ConsControl) -> R<()> {
        let reg = self.fsr().freereg;
        let mut hint = None;
        let mut key = if self.tok() == Tok::Name {
            // The 1.12 grammar's placement: only a `name = v` field counts toward the hash
            // size hint, as 5.0's `recfield` counted it.
            cc.nh += 1;
            let name = self.str_checkname()?;
            if let Some(p) = &cc.prefix {
                let mut n = p.clone();
                join_name(&mut n, b".", name);
                hint = Some(n);
            }
            Exp::new(Kind::K(self.string_k(name)?))
        } else {
            self.yindex()?
        };
        self.checknext(Tok::Char(b'='))?;
        let rkkey = self.exp2rk(&mut key)?;
        self.name_hint = hint;
        let mut val = self.expr()?;
        let rv = self.exp2rk(&mut val)?;
        self.code_settable(cc.t, rkkey, rv)?;
        self.fs().freereg = reg;
        Ok(())
    }

    fn closelistfield(&mut self, cc: &mut ConsControl) -> R<()> {
        if cc.v.k == Kind::Void {
            return Ok(());
        }
        self.exp2nextreg(&mut cc.v)?;
        cc.v.k = Kind::Void;
        if cc.tostore == FIELDS_PER_FLUSH {
            self.setlist(cc.t, cc.na, cc.tostore as i32)?;
            cc.tostore = 0;
        }
        Ok(())
    }

    fn lastlistfield(&mut self, cc: &mut ConsControl) -> R<()> {
        if cc.tostore == 0 {
            return Ok(());
        }
        if cc.v.has_multret() {
            self.setmultret(&cc.v);
            self.setlist(cc.t, cc.na, -1)?;
            // The call's values are not counted.
            cc.na -= 1;
        } else {
            if cc.v.k != Kind::Void {
                self.exp2nextreg(&mut cc.v)?;
            }
            self.setlist(cc.t, cc.na, cc.tostore as i32)?;
        }
        Ok(())
    }

    fn listfield(&mut self, cc: &mut ConsControl) -> R<()> {
        cc.v = self.expr()?;
        // The client's `listfield` (`0x6fd7aa`).
        if cc.na > MAXLISTITEMS {
            return Err(self.errorlimit(MAXLISTITEMS as usize, "items in a constructor"));
        }
        cc.na += 1;
        cc.tostore += 1;
        Ok(())
    }

    /// `constructor`, with 5.0's compat-semicolon at the head of each item, as the 1.12 grammar
    /// has it.
    fn constructor(&mut self, prefix: Option<Vec<u8>>) -> R<Exp> {
        let line = self.lx.line;
        let pc = self.code(Op::NewTable(0, 0, 0), 1)?;
        let mut t = Exp::new(Kind::Reloc(pc));
        self.exp2nextreg(&mut t)?;
        let Kind::NonReloc(treg) = t.k else {
            unreachable!()
        };
        let mut cc = ConsControl {
            v: Exp::new(Kind::Void),
            t: treg,
            nh: 0,
            na: 0,
            tostore: 0,
            prefix,
        };
        self.checknext(Tok::Char(b'{'))?;
        loop {
            self.testnext(Tok::Char(b';'))?;
            if self.tok() == Tok::Char(b'}') {
                break;
            }
            self.closelistfield(&mut cc)?;
            match self.tok() {
                Tok::Name => {
                    if self.lx.lookahead()? != Tok::Char(b'=') {
                        self.listfield(&mut cc)?;
                    } else {
                        self.recfield(&mut cc)?;
                    }
                }
                Tok::Char(b'[') => self.recfield(&mut cc)?,
                _ => self.listfield(&mut cc)?,
            }
            if !(self.testnext(Tok::Char(b','))? || self.testnext(Tok::Char(b';'))?) {
                break;
            }
        }
        self.check_match(Tok::Char(b'}'), Tok::Char(b'{'), line)?;
        self.lastlistfield(&mut cc)?;
        // `OP_NEWTABLE`'s hints as floating point bytes; the array one is kept decoded.
        self.fs().code[pc] = Op::NewTable(
            treg,
            crate::compile::fb2int(crate::compile::int2fb(cc.na)),
            crate::compile::int2fb(cc.nh) as u8,
        );
        Ok(t)
    }

    /// `explist1`: the count and the last expression, the others in consecutive registers.
    fn explist1(&mut self) -> R<(usize, Exp)> {
        let mut n = 1;
        let mut v = self.expr()?;
        while self.testnext(Tok::Char(b','))? {
            self.exp2nextreg(&mut v)?;
            v = self.expr()?;
            n += 1;
        }
        Ok((n, v))
    }

    /// `funcargs`; `callee` is the name the call was made through (`SetScript` in
    /// `f:SetScript(...)`), which names a function literal among the arguments.
    fn funcargs(&mut self, f: &mut Exp, callee: Option<&[u8]>) -> R<()> {
        let line = self.lx.line;
        let mut args = match self.tok() {
            Tok::Char(b'(') => {
                if line != self.lx.lastline {
                    return Err(self
                        .lx
                        .error("ambiguous syntax (function call x new statement)"));
                }
                self.next()?;
                let args = if self.tok() == Tok::Char(b')') {
                    Exp::new(Kind::Void)
                } else {
                    let args = self.call_args(callee)?;
                    self.setmultret(&args);
                    args
                };
                self.check_match(Tok::Char(b')'), Tok::Char(b'('), line)?;
                args
            }
            Tok::Char(b'{') => self.constructor(None)?,
            Tok::String => {
                let bytes = std::mem::take(&mut self.lx.cur.bytes);
                let k = self.string_k(&bytes)?;
                self.next()?;
                Exp::new(Kind::K(k))
            }
            _ => return Err(self.lx.error("function arguments expected")),
        };
        let Kind::NonReloc(base) = f.k else {
            unreachable!("the function is in a register")
        };
        let nparams = if args.has_multret() {
            -1
        } else {
            if args.k != Kind::Void {
                self.exp2nextreg(&mut args)?;
            }
            self.fsr().freereg as i32 - (base as i32 + 1)
        };
        f.k = Kind::Call(self.code(Op::Call(base, (nparams + 1) as u8, 2), 1)?);
        self.fixline(line);
        self.fs().freereg = base + 1;
        Ok(())
    }

    /// `explist1` for a call's arguments. A function literal among them is named, for its
    /// hot-reload key, after the callee and the first string argument before it:
    /// `f:SetScript("OnShow", function ... end)` is `SetScript:OnShow`, so deleting one handler
    /// and adding another cannot hand the survivor's closure the newcomer's code, as positional
    /// `anon`, `anon#2` keys would.
    fn call_args(&mut self, callee: Option<&[u8]>) -> R<Exp> {
        let mut tag: Option<Vec<u8>> = None;
        loop {
            if let (Some(c), Tok::Function) = (callee, self.tok()) {
                let mut h = Vec::new();
                join_name(&mut h, b"", c);
                if let Some(t) = &tag {
                    // `/` and `#` structure keys.
                    let t: Vec<u8> = t
                        .iter()
                        .map(|&b| if b == b'/' || b == b'#' { b'_' } else { b })
                        .collect();
                    join_name(&mut h, b":", &t);
                }
                self.name_hint = Some(h);
            }
            if tag.is_none() && self.tok() == Tok::String {
                tag = Some(self.lx.cur.bytes.clone());
            }
            let mut v = self.expr()?;
            if !self.testnext(Tok::Char(b','))? {
                return Ok(v);
            }
            self.exp2nextreg(&mut v)?;
        }
    }

    fn prefixexp(&mut self) -> R<Exp> {
        match self.tok() {
            Tok::Char(b'(') => {
                let line = self.lx.line;
                self.next()?;
                let mut v = self.expr()?;
                self.check_match(Tok::Char(b')'), Tok::Char(b'('), line)?;
                self.dischargevars(&mut v)?;
                Ok(v)
            }
            Tok::Name => Ok(self.singlevar()?.0),
            _ => Err(self.lx.error("unexpected symbol")),
        }
    }

    /// `primaryexp`: a prefix and its suffixes, read in a loop.
    fn primaryexp(&mut self) -> R<Exp> {
        Ok(self.primaryexp_named(false)?.0)
    }

    /// [`Self::primaryexp`], and with `want` its name when it is a name and fields: `t.a.b`.
    fn primaryexp_named(&mut self, want: bool) -> R<(Exp, Option<Vec<u8>>)> {
        // `last` is the name the value was last reached by, which names a call's function
        // literals.
        let (mut v, mut name, mut last) = if self.tok() == Tok::Name {
            let (v, n) = self.singlevar()?;
            (
                v,
                want.then(|| {
                    let mut name = Vec::new();
                    join_name(&mut name, b"", n);
                    name
                }),
                Some(n),
            )
        } else {
            (self.prefixexp()?, None, None)
        };
        loop {
            match self.tok() {
                Tok::Char(b'.') => {
                    let field = self.field(&mut v)?;
                    if let Some(n) = &mut name {
                        join_name(n, b".", field);
                    }
                    last = Some(field);
                    continue;
                }
                Tok::Char(b'[') => {
                    self.exp2anyreg(&mut v)?;
                    let mut key = self.yindex()?;
                    self.indexed(&mut v, &mut key)?;
                    last = None;
                }
                Tok::Char(b':') => {
                    self.next()?;
                    let method = self.str_checkname()?;
                    let mut key = Exp::new(Kind::K(self.string_k(method)?));
                    self.self_(&mut v, &mut key)?;
                    self.funcargs(&mut v, Some(method))?;
                    last = None;
                }
                Tok::Char(b'(') | Tok::String | Tok::Char(b'{') => {
                    self.exp2nextreg(&mut v)?;
                    self.funcargs(&mut v, last)?;
                    last = None;
                }
                _ => return Ok((v, name)),
            }
            name = None;
        }
    }

    /// `simpleexp`: the 1.12 grammar has no `...` expression.
    fn simpleexp(&mut self) -> R<Exp> {
        // Only the expression's first operand is named after the target.
        let hint = self.name_hint.take();
        let v = match self.tok() {
            Tok::Number => Exp::new(Kind::KNum(self.lx.cur.num)),
            Tok::String => {
                let bytes = std::mem::take(&mut self.lx.cur.bytes);
                let k = self.string_k(&bytes)?;
                Exp::new(Kind::K(k))
            }
            Tok::Nil => Exp::new(Kind::Nil),
            Tok::True => Exp::new(Kind::True),
            Tok::False => Exp::new(Kind::False),
            Tok::Char(b'{') => return self.constructor(hint),
            Tok::Function => {
                self.next()?;
                let line = self.lx.line;
                return self.body(false, line, hint);
            }
            _ => return self.primaryexp(),
        };
        self.next()?;
        Ok(v)
    }

    /// `subexpr`: operators binding tighter than `limit`, left-associative ones in the loop;
    /// answers the first untreated operator.
    fn subexpr(&mut self, limit: u8) -> R<(Exp, Option<BinOpr>)> {
        self.enterlevel()?;
        let mut v = match getunopr(self.tok()) {
            Some(uop) => {
                self.next()?;
                let (mut v, _) = self.subexpr(UNARY_PRIORITY)?;
                self.prefix(uop, &mut v)?;
                v
            }
            None => self.simpleexp()?,
        };
        let mut op = getbinopr(self.tok());
        while let Some(o) = op {
            let (left, right) = priority(o);
            if left <= limit {
                break;
            }
            self.next()?;
            self.infix(o, &mut v)?;
            let (mut v2, nextop) = self.subexpr(right)?;
            self.posfix(o, &mut v, &mut v2)?;
            op = nextop;
        }
        self.leavelevel();
        Ok((v, op))
    }

    fn expr(&mut self) -> R<Exp> {
        Ok(self.subexpr(0)?.0)
    }
}
