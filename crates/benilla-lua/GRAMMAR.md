# The grammar benilla-lua parses

benilla's script host runs Lua 5.1.5 with ten changes, so that it accepts exactly the grammar
the 1.12.1 client's Lua accepts, no more and no less. Until benilla-lua replaced it, that language
was a patched copy of the C Lua (`third_party/lua-src`, in the git history); benilla-lua implements
the same language directly. A grammar difference in either direction is invisible to every
instrument until an addon trips over it, and the addon corpus asks about the difference directly.

The 1.12 addon corpus is Lua 5.0 code. It uses the iterator-less generic-for:

```lua
for k, v in someTable do ... end        -- no pairs(), no iterator function
```

183 of 218 corpus addons are reached by it (118 carry it, 65 inherit it through a declared
dependency), across 1,163 sites. On stock Lua 5.1 it raises `attempt to call a table value`, the
first session-start error for 60 of the 218.

## The ten changes

Five restore what 5.1 changed or removed:

| where in 5.1 | change | why |
|---|---|---|
| `lvm.c` | 5.0's `OP_TFORPREP` table-to-`next` substitution, at the top of `OP_TFORLOOP` | `for k, v in someTable do` raised "attempt to call a table value" |
| `llex.c` | 5.0's long brackets: `[[` nests `[[`, and there are no `=` levels | two corpus addons nest `[[...]]`, which stock 5.1 refuses as "nesting of `[[...]]` is deprecated"; the client's lexer tests the byte after `[` once (`0x6ff793`) and its `read_long_string` (`0x700010`) reads `=` as text, so `[=[` is `[` then `=`, and `--[==[` a line comment (`0x6ff6c9`) |
| `luaconf.h` | `LUA_QL(x)` back to 5.0's `` `x' `` from 5.1's `'x'` | every Lua error message quotes a program element, and `WoW.exe`'s own `.rdata` carries all five formats in the 5.0 spelling |
| `lparser.c` | 5.0's compat-semicolon skip at the top of `constructor()`'s field loop | one extra `;` after a field separator inside a table constructor (`Back_Title = AL["Factions"];;`, which AtlasLoot writes) is accepted by the client |
| `lparser.c` | `recfield`'s `cc->nh++` inside its `TK_NAME` arm, 5.0's placement | a `[expr] = value` constructor field credits neither `OP_NEWTABLE` size hint in 5.0, so the table is born on the dummy node |

Three delete what 5.1 added, all in `lparser.c`, all byte-read out of the client's own parser
(`simpleexp 0x6fd240`, `getunopr 0x6fe0a0`, `getbinopr 0x6fe0c0`). `...` and `#` land on 5.0's
own `unexpected symbol` at `prefixexp 0x6fde40`. `%` ends the expression before it, so the error is
whatever comes next expects: `` unexpected symbol near `%' `` after `x = 5`, and
`` <eof> expected near `%' `` (the client's bare text, `0x871cf4`) after `return 5`.

| change | why |
|---|---|
| `...` is not an expression (`simpleexp`'s `case TK_DOTS`) | the Ace2 corpus asks which interpreter it is on by compiling `return function(...) return ... end`; 170 `loadstring` sites in the corpus are that question, and 92 library files in 24 folders branch on the answer |
| no length operator (`getunopr`'s `case '#'`) | the client's `getunopr` tests exactly two tokens (`-`, `not`) and its `OPR_NOUNOPR` is 2, a three-member enum; 5.0 asks a table with `table.getn` and a string with `string.len` |
| no modulo operator (`getbinopr`'s `case '%'`) | the client's `getbinopr` switch is based at `'*'` (0x2A), so `%` (0x25) is below its range and reaches `OPR_NOBINOPR` = 14, a fifteen-member `BinOpr`; 5.0 spells it `math.mod` |

The deletions change nothing for the stock interface: a comment- and string-stripping scan of the
1.12 FrameXML (177 files), GlueXML and Blizzard's own addons finds zero sites of all three. In the
addon corpus they reject 18 chunks, all from code written for a later client: 17 in
AckisRecipeList and its Ace3 libraries, and one in a later Tablet-2.0 bundled with FuBar_AtlasFu.
The C build rejected the same chunks, and `tests/front_end.rs` names each with its message. No chunk of
benilla's own Lua may use a construct the client's parser rejects.

Two narrow what 5.1 reads as a number, in `lobject.c` and `llex.c` (`number.rs` and `lex.rs`
here):

| change | why |
|---|---|
| `luaO_str2d` reads a decimal numeral only: 5.1's hexadecimal branch deleted, `inf` and `nan` refused, and `d`/`D` taken as the exponent marker | the client's `luaO_str2d` (`0x6f5900`) is its `strtod` (`0x741e39`, decimal only, its exponent marker `e`, `E`, `d` or `D` at `0x4112d1`) and a trailing-space skip, so `tonumber("1d2")` is 100 there, `tonumber("0x10")` is nil and `"0x2" + 1` raises; every coercion goes through it (arithmetic, `tonumber`, a binding's numeric argument) |
| `read_numeral` is 5.0's, the client's numeral reader (`0x6ffd20`): digits, one `.` (a second right after it is `ambiguous syntax (decimal point x string concatenation)`, `0x8721dc`), digits, then `e`/`E`, an optional sign and digits | 5.1's took every further `.`, letter and `_` into the numeral, so `a=1b=2` was a malformed number where the client compiles `a=1 b=2`, and `1.2.3` was one where the client reads `1.2` then `.3`; a source `0x10` is the number 0 then the name `x10` |

Neither changes what compiles in the stock interface or the addon corpus: `tests/front_end.rs`
rejects the same chunks with and without them.

## The generic-for, in detail

Three details are the client's own, each byte-read there: the substitution test is a bare type-tag
equality that never consults a metatable (a table carrying `__call` still gets `next`); the callee
is the global `next`, read raw and fetched fresh at every loop entry (an addon assigning
`next = myfn` changes every later generic-for in the session); and userdata is not substituted.
