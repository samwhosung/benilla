# benilla's `lua-src` fork: what differs from upstream, and how to check

Upstream: [`mlua-rs/lua-src-rs`](https://github.com/mlua-rs/lua-src-rs), MIT, version `550.0.0`,
the version mlua 0.11 resolves to. Wired in through `[patch.crates-io]` in the workspace root.

## Why a fork exists at all

benilla's Lua has to accept exactly the grammar the 1.12.1 client's Lua accepts, no more and no
less. Every hunk below is that one rule: three restore 5.0 constructs 5.1 deleted, three delete 5.1
constructs 5.0 never had. A grammar difference in either direction is invisible to every instrument
until an addon trips over it, and the addon corpus asks about the difference directly.

The 1.12 addon corpus is Lua 5.0 code. It uses the iterator-less generic-for:

```lua
for k, v in someTable do ... end        -- no pairs(), no iterator function
```

183 of 218 corpus addons are reached by it (118 carry it, 65 inherit it through a declared
dependency), across 1,163 sites. On stock Lua 5.1 it raises `attempt to call a table value`, the
first session-start error for 60 of the 218.

Lua 5.1 removed it at the opcode level, which is why no layer above the VM can reach it: a `__call`
metamethod would need a per-type default metatable for tables, which 5.1 does not have; rewriting
the chunk source means parsing Lua with a regex; and `lua-src` ships 5.1 through 5.5, every one of
which removed it.

## What is not being done

benilla is not adopting Lua 5.0. It stays on 5.1.5 and restores the behaviour of a single opcode
that Lua itself shipped and labelled `/* for compatibility only */`. Every other 5.1 fix, and the
whole of mlua's surface, stay exactly as upstream.

## The delta, and the command that proves it

Ten hunks in five files. `src/lib.rs` additionally differs by having Lua 5.2/5.3/5.4/5.5 stripped
from the `Version` enum (with their source trees deleted): benilla builds 5.1 and only 5.1, and a
fork that still offered the other four would answer a request for one with a missing directory at
build time instead of a compile error.

Five hunks restore what 5.1 changed or removed:

| file | hunk | why |
|---|---|---|
| `lvm.c` | 5.0's `OP_TFORPREP` table-to-`next` substitution, folded into the top of `OP_TFORLOOP` | `for k, v in someTable do` raised "attempt to call a table value" |
| `luaconf.h` | `LUA_COMPAT_LSTR` `1` to `2` | 5.1 kept 5.0's `[[...]]` nesting machinery and put an advisory error in front of it; two corpus addons died on "nesting of `[[...]]` is deprecated" |
| `luaconf.h` | `LUA_QL(x)` back to 5.0's `` `x' `` from 5.1's `'x'` | every Lua error message quotes a program element, and `WoW.exe`'s own `.rdata` carries all five formats in the 5.0 spelling |
| `lparser.c` | 5.0's compat-semicolon skip restored at the top of `constructor()`'s field loop, 5.0's own line verbatim | one extra `;` after a field separator inside a table constructor (`Back_Title = AL["Factions"];;`, which AtlasLoot writes) is accepted by the client and was a parse error here |
| `lparser.c` | `recfield`'s `cc->nh++` moved back inside its `TK_NAME` arm, 5.0's own placement | a `[expr] = value` constructor field credits neither `OP_NEWTABLE` size hint in 5.0, so the table is born on the dummy node |

Three delete what 5.1 added, all in `lparser.c`, all byte-read out of the client's own parser
(`simpleexp 0x6fd240`, `getunopr 0x6fe0a0`, `getbinopr 0x6fe0c0`), all landing on 5.0's own
`unexpected symbol` at `prefixexp 0x6fde40`:

| hunk | why |
|---|---|
| `simpleexp`'s `case TK_DOTS` deleted: `...` is not an expression | the Ace2 corpus asks which interpreter it is on by compiling `return function(...) return ... end`; 170 `loadstring` sites in the corpus are that question, and 92 library files in 24 folders branch on the answer |
| `getunopr`'s `case '#'` deleted: no length operator | the client's `getunopr` tests exactly two tokens (`-`, `not`) and its `OPR_NOUNOPR` is 2, a three-member enum; 5.0 asks a table with `table.getn` and a string with `string.len` |
| `getbinopr`'s `case '%'` deleted: no modulo operator | the client's `getbinopr` switch is based at `'*'` (0x2A), so `%` (0x25) is below its range and reaches `OPR_NOBINOPR` = 14, a fifteen-member `BinOpr`; 5.0 spells it `math.mod` |

Two narrow what 5.1 reads as a number, in `lobject.c` and `llex.c`:

| hunk | why |
|---|---|
| `luaO_str2d` reads only a decimal numeral: 5.1's hexadecimal branch deleted, and a new `decimal_end` stops the platform's C99 `strtod` short of hexadecimal, `inf` and `nan`, and spells a `d`/`D` exponent `e` for it | the client's `luaO_str2d` (`0x6f5900`) is `strtod` (`0x741e39`, decimal only, its exponent marker `e`, `E`, `d` or `D` at `0x4112d1`) and a trailing-space skip, so `tonumber("1d2")` is 100 there, so `tonumber("0x10")` is nil there and `"0x2" + 1` raises; every coercion goes through it (arithmetic, `tonumber`, a binding's numeric argument). The lexer converts numerals through it too; with the `read_numeral` hunk below, a source `0x10` is the number 0 then the name `x10`, as on the client: a syntax error, except where a statement can end after the 0 (`a = 0x10()`) |
| `read_numeral` is 5.0's, the client's numeral reader (`0x6ffd20`): digits, one `.` (a second right after it is `ambiguous syntax (decimal point x string concatenation)`, `0x8721dc`), digits, then `e`/`E`, an optional sign and digits | 5.1's took every further `.`, letter and `_` into the numeral, so `a=1b=2` was a malformed number where the client compiles `a=1 b=2`, and `1.2.3` was one where the client reads `1.2` then `.3` |

The deletions are safe because nothing the reference runs uses those constructs: a comment- and
string-stripping scan of the 1.12 FrameXML (177 files), GlueXML, Blizzard's own addons and the
corpus finds zero sites of all three. No chunk of benilla's own Lua may use a construct the client's
parser rejects.

To verify the Lua sources against upstream at any time:

```sh
# 550.0.0 is the pinned version; adjust the path if cargo's registry hash differs.
diff -r third_party/lua-src/lua-5.1.5 \
  ~/.cargo/registry/src/*/lua-src-550.0.0/lua-5.1.5
```

That must print exactly the ten hunks in the tables above and nothing else.

## The generic-for, in detail

The `lvm.c` comment carries the three details that are the client's own, each byte-read there: the
substitution test is a bare type-tag equality that never consults a metatable (a table carrying
`__call` still gets `next`); the callee is the global `next`, read raw and fetched fresh at every loop
entry (an addon assigning `next = myfn` changes every later generic-for in the session); and userdata
is not substituted. The substitution happens in `OP_TFORPREP`, an opcode 5.1 deleted, so the top of
`OP_TFORLOOP` is where 5.1 can host it.
