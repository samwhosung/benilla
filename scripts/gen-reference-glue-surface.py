#!/usr/bin/env python3
"""Regenerate `reference/1.12-glue-surface.tsv`: every Lua function the glue screens' VM has in `_G`.

    scripts/gen-reference-glue-surface.py [--shapes FILE] [--globals FILE] [--out FILE]

The inputs are vendored under `reference/`, so this runs from any clone and reproduces the
committed table byte for byte.

The glue builder (`0x46a7b0`) starts from the bare base set `0x7039e0`: a rebuild resets the Lua
state (`0x703b80` -> `0x7039e0`, at `0x46a87b`), and the first build (`0x46a505`, passing 0) skips
that reset (`je` at `0x46a879`) and takes the state the startup init left (`0x402aee`). It then
calls the glue registrar `0x46abb0`, which installs seven registrar tables and nothing else:

  0x51bb20  table 0x8503f4    6  shared: in-game, `0x51bb60` calls it before its own table
  0x706350  table 0x872e74    4  shared: in-game at `0x490251`; then the 17 widget method tables
  0x46ddf0  table 0x8373b8   60  glue core
  0x4736c0  table 0x838578   11  character select
  0x471a20  table 0x8380f8   24  character create
  0x46f480  table 0x837ea0   10  realm list
  0x4587f0  table 0x835a50    4  shared: in-game at `0x4902a6`

Each registrar is a `0x704120` loop over its table with the bound read from its own `cmp`, the
counts above. `0x7039e0`, FrameScript's Lua init, is the base set both states share: the 36-entry
base array `0x811e28` (`1.12-shapes.tsv`'s `baselib` table), the `string`, `table`, `math` and
`bit` libraries (fields, not globals, so not rows here), `math`'s global `__pow` ([`POW`]), and
the compat chunk `0x8722e8` run at `0x703b26`. The chunk's 39 function assignments are
[`COMPAT`]: 32 bind a library function under a bare name (`tinsert = table.insert`), each row
taking that binding's `fn` and table, and 7 wrap one in a degree-converting Lua closure (`sin`,
`acos`, ...), `fn` `lua`. Its one other assignment, `PI = math.pi`, is a number. Every engine
function `1.12-globals.tsv` has that no `global` or `baselib` row of the shapes table names must be
one of these, so a chunk the list missed fails the generator.

Columns:
  scope        base (0x7039e0) | shared (a registrar the in-game state calls too) | glue.
  registrar    the function that installs the row.
  fn, table_va the binding and its table; a compat alias carries the library binding it names,
               or fn `lua` and table `-` for a closure; `__pow` has no table.
  arity        the shapes table's return arity, `?` for a closure.
  world_fn     a glue row's in-game namesake from any other `global` table, else `-`. The in-game
               state never sees a glue row, but the name is shared: a glue verb benilla builds is a
               different binding from the in-game one of the same name.
  note         `shape-differs` where both shapes are exact and the argument count or the return
               arity differs between the glue binding and its in-game namesake.
"""
import argparse
import os
import sys

# (registrar, table_va, rows, scope), in the order `0x46abb0` calls them.
REGISTRARS = [
    (0x51BB20, 0x8503F4, 6, "shared"),
    (0x706350, 0x872E74, 4, "shared"),
    (0x46DDF0, 0x8373B8, 60, "glue"),
    (0x4736C0, 0x838578, 11, "glue"),
    (0x471A20, 0x8380F8, 24, "glue"),
    (0x46F480, 0x837EA0, 10, "glue"),
    (0x4587F0, 0x835A50, 4, "shared"),
]
BASE_INIT = 0x7039E0
BASE_TABLE = 0x811E28
BASE_ROWS = 36
# The libraries `0x7039e0` opens, by the name the compat chunk reaches them under.
LIBRARIES = {"table": 0x822D40, "math": 0x822C60, "string": 0x822D88, "bit": 0x822C18}
# The compat chunk's function assignments, in its order: (global, "library.field"), or None for a
# Lua closure that converts degrees around the `math` function of the same name.
COMPAT = [
    ("foreach", "table.foreach"), ("foreachi", "table.foreachi"), ("getn", "table.getn"),
    ("tinsert", "table.insert"), ("tremove", "table.remove"), ("sort", "table.sort"),
    ("abs", "math.abs"), ("acos", None), ("asin", None), ("atan", None), ("atan2", None),
    ("ceil", "math.ceil"), ("cos", None), ("deg", "math.deg"), ("exp", "math.exp"),
    ("floor", "math.floor"), ("frexp", "math.frexp"), ("ldexp", "math.ldexp"), ("log", "math.log"),
    ("log10", "math.log10"), ("max", "math.max"), ("min", "math.min"), ("mod", "math.mod"),
    ("rad", "math.rad"), ("random", "math.random"), ("randomseed", "math.randomseed"),
    ("sin", None), ("sqrt", "math.sqrt"), ("tan", None),
    ("strbyte", "string.byte"), ("strchar", "string.char"), ("strfind", "string.find"),
    ("format", "string.format"), ("gsub", "string.gsub"), ("strlen", "string.len"),
    ("strlower", "string.lower"), ("strrep", "string.rep"), ("strsub", "string.sub"),
    ("strupper", "string.upper"),
]
# `luaopen_math` 0x7fb470 also sets the global `__pow` to 0x7fb050 (`0x7fb4af`-`0x7fb4d2`), which
# Lua 5.0 calls for `^`.
POW = ("__pow", 0x7FB050)

REFERENCE = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "reference"))


def rows(path, header):
    if not os.path.exists(path):
        sys.exit(f"no table at {path}")
    for line in open(path, encoding="utf-8"):
        if line.startswith("#"):
            continue
        f = line.rstrip("\n").split("\t")
        if f and f[0] == header:
            continue
        yield f


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--shapes", default=os.path.join(REFERENCE, "1.12-shapes.tsv"))
    ap.add_argument("--globals", default=os.path.join(REFERENCE, "1.12-globals.tsv"))
    ap.add_argument("--out", default=os.path.join(REFERENCE, "1.12-glue-surface.tsv"))
    a = ap.parse_args()

    # name, fn, pair_va, table_va, table_kind, argc, argc_conf, arity, arity_conf, ...
    shapes = [f for f in rows(a.shapes, "name") if len(f) >= 9]
    by_table = {}
    for f in shapes:
        by_table.setdefault(int(f[3], 16), []).append(f)

    out = []

    def take(table_va, n, registrar, scope):
        got = by_table.get(table_va, [])
        if len(got) != n:
            sys.exit(f"table 0x{table_va:x}: the registrar loops {n} rows, the shapes table has {len(got)}")
        for f in got:
            out.append({"f": f, "scope": scope, "registrar": registrar})

    take(BASE_TABLE, BASE_ROWS, BASE_INIT, "base")
    for registrar, table_va, n, scope in REGISTRARS:
        take(table_va, n, registrar, scope)

    # The base set's Lua side: the compat chunk's aliases and `__pow`, each a binding of a library
    # the shapes table carries, or a Lua closure.
    fields = {}
    for lib, table_va in LIBRARIES.items():
        for f in by_table.get(table_va, []):
            fields[f"{lib}.{f[0]}"] = f
    aliases = []
    for name, target in COMPAT:
        if target is None:
            aliases.append((name, "lua", "-", "?"))
            continue
        f = fields.get(target)
        if f is None:
            sys.exit(f"compat alias {name} = {target}: no such library binding in {a.shapes}")
        aliases.append((name, f[1], f[3], f[7]))
    aliases.append((POW[0], f"0x{POW[1]:x}", "-", "1"))

    # A 1.12 engine function no registrar row names is one of the aliases, or the list is short.
    registered = {f[0] for f in shapes if f[4] in ("global", "baselib")}
    alias_names = {x[0] for x in aliases}
    for f in rows(a.globals, "name"):
        if len(f) >= 3 and f[1] == "function" and f[2] in ("engine", "lua"):
            if f[0] not in registered and f[0] not in alias_names:
                sys.exit(f"{f[0]} is a 1.12 engine function no registrar and no compat alias names")

    # In-game namesakes: rows of every other `global` table, which only the in-game state installs.
    ours = {t for _, t, _, _ in REGISTRARS}
    world = {}
    for f in shapes:
        if f[4] == "global" and int(f[3], 16) not in ours:
            world.setdefault(f[0], []).append(f)

    lines = []
    differs = 0
    for r in out:
        f = r["f"]
        name, fn, table_va, argc, argc_conf, arity, arity_conf = f[0], f[1], f[3], f[5], f[6], f[7], f[8]
        world_fn, note = "-", ""
        if r["scope"] == "glue" and name in world:
            w = world[name]
            world_fn = "|".join(x[1] for x in w)
            notes = []
            for x in w:
                if argc_conf == "exact" and x[6] == "exact" and argc != x[5]:
                    notes.append(f"argc {argc} here, {x[5]} in-game ({x[1]})")
                if arity_conf == "exact" and x[8] == "exact" and arity != x[7]:
                    notes.append(f"arity {arity} here, {x[7]} in-game ({x[1]})")
            if notes:
                note = "shape-differs: " + "; ".join(notes)
                differs += 1
        lines.append((r["scope"], f"0x{r['registrar']:x}", name, fn, table_va, arity, world_fn, note))
    for name, fn, table_va, arity in aliases:
        lines.append(("base", f"0x{BASE_INIT:x}", name, fn, table_va, arity, "-", ""))

    names = [l[2] for l in lines]
    if len(names) != len(set(names)):
        sys.exit("a name appears twice in the glue state; the table is keyed by name")

    count = {s: sum(1 for l in lines if l[0] == s) for s in ("base", "shared", "glue")}
    path = os.path.normpath(a.out)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(
            "# The glue screens' Lua surface in the 1.12.1 client (build 5875): every function in `_G`\n"
            "# once the glue builder 0x46a7b0 has installed its registrars, before GlueXML loads.\n"
            "# Generated by scripts/gen-reference-glue-surface.py from reference/1.12-shapes.tsv and\n"
            "# reference/1.12-globals.tsv; the generator's header carries the registrars and the method.\n"
            "#\n"
            "# scope      base = FrameScript's Lua init 0x7039e0, which both states run; shared = a\n"
            "#            registrar the in-game state calls too (0x51bb20, 0x706350, 0x4587f0); glue =\n"
            "#            a glue-only registrar table (0x46ddf0, 0x4736c0, 0x471a20, 0x46f480).\n"
            "# fn, table_va, arity  as 1.12-shapes.tsv has them. A base alias the compat chunk\n"
            "#            0x8722e8 defines carries the library binding it names (`tinsert` is table\n"
            "#            0x822d40's `insert`), or fn `lua` for a degree-converting closure; math's\n"
            "#            global `__pow` (0x7fb050) has no table.\n"
            "# world_fn   a glue row's in-game namesake, a different binding; `-` if none.\n"
            "# note       shape-differs = both shapes exact, and the glue binding takes or returns a\n"
            "#            different count than its in-game namesake.\n"
            f"# {len(lines)} functions: {count['base']} base, {count['shared']} shared, {count['glue']} glue;"
            f" {differs} glue rows differ in shape from their in-game namesake.\n"
            "scope\tregistrar\tname\tfn\ttable_va\tarity\tworld_fn\tnote\n"
        )
        for l in lines:
            fh.write("\t".join(l) + "\n")
    print(f"wrote {path}: {len(lines)} functions, {differs} shape differences")


if __name__ == "__main__":
    main()
