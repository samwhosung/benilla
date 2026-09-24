#!/usr/bin/env python3
"""Regenerate `reference/1.12-verb-events.tsv`: the events 1.12 Lua verbs fire, keyed by verb.

    scripts/gen-reference-verb-events.py --names FILE --extents FILE... --disasm FILE [--out FILE]

A row says the reference fires `event` on the call path of the registered Lua binding `verb`:
from the verb's body (`shape=body`), or from a helper that only registered verbs call
(`shape=helper`, `via` = the helper's address; `0x4d8c90` is the trainer's). A helper with any
non-verb caller (a packet handler, the cursor, the tutorial signal) fires a state event and is
left out, and so is a hub (`ClearCursor` -> CURSOR_UPDATE, 49 callers).

A site's function is the greatest start at or below it, over the union of: every function the
analysis sizes, every named function start, every `call` target in the client's disassembly,
every registered verb, and every address after a padding run (an `int3` run anywhere, or a `nop`
run after a `ret`; a `nop` run after a `jmp` is a loop head, not a boundary). A site at or past
`start + size` of a sized function is left unattributed rather than handed to it.

Two families are dropped: glue-space verbs and sites (the GlueXML registrar, 0x46a000..0x476000)
and the unit-field bridge and token fan-out ids below 0xb6, which are state events.

A floor, not a census: a verb with no row may still fire an event, because the fire sites are
only the two signal helpers' literal call sites and `shape=helper` stops one call deep.

The fire sites and the binding shapes are the vendored `reference/` tables; the function names
(`--names`), extents (`--extents`, the `size=` on each `fn` row) and disassembly (`--disasm`) are
the maintainer's analysis, not in this repo, so this is a manual regeneration.
"""
import argparse
import bisect
import collections
import os
import re
import sys

# The GlueXML registrar's address range: an id fired there names a glue-screen event.
GLUE = (0x46A000, 0x476000)
# `0x51bbb0`'s walk bound: every named unit-window field below it fires through the generic bridge.
UNIT_WINDOW_FIELDS = 0xB6
# A helper "only verbs call" is trusted up to this many callers; past it, it is a hub.
MAX_HELPER_CALLERS = 8

LINE = re.compile(r"^\s*([0-9a-f]+):\t(\S+)(?:\s+(.*))?$")
BRANCH = re.compile(r"^(call|jmp)\s+0x([0-9a-f]+)\s*$")
SIZE = re.compile(r"\bsize=(\d+)")


REFERENCE = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "reference"))


def rows(path):
    if not os.path.exists(path):
        sys.exit(f"no table at {path}")
    for line in open(path, encoding="utf-8"):
        if line.startswith("#"):
            continue
        f = line.rstrip("\n").split("\t")
        if f and f[0] in ("name", "addr", "eventId", "fire_site_va"):
            continue
        yield f


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--names", help="the function names: `addr<TAB>kind` rows, `func` a start")
    ap.add_argument("--extents", nargs="+", default=[], help="tables whose `fn` rows carry size=")
    ap.add_argument("--disasm", help="the client's disassembly, one objdump line per instruction")
    ap.add_argument("--out", default=os.path.join(REFERENCE, "1.12-verb-events.tsv"))
    a = ap.parse_args()
    if not (a.names and a.extents and a.disasm):
        sys.exit(
            "this table is derived from the maintainer's function names, extents and disassembly, "
            "which are not in this repo: pass --names, --extents and --disasm. The committed "
            "reference/1.12-verb-events.tsv is the surface benilla tracks."
        )

    starts = set()
    for f in rows(a.names):
        if len(f) >= 2 and f[1] == "func":
            starts.add(int(f[0], 16))
    if len(starts) < 1000:
        sys.exit(f"{a.names} gave only {len(starts)} functions — wrong file?")

    # Function extents: `size=` on the `fn` rows of the extent tables.
    extent = {}  # fn start -> size in bytes
    for path in a.extents:
        for line in open(path, encoding="utf-8", errors="replace"):
            if not line.startswith("fn\t"):
                continue
            f = line.rstrip("\n").split("\t")
            m = SIZE.search(line)
            if m and len(f) > 1 and f[1].startswith("0x"):
                fn = int(f[1], 16)
                extent[fn] = max(extent.get(fn, 0), int(m.group(1)))
    if len(extent) < 5000:
        sys.exit(f"the extent tables gave only {len(extent)} sized functions — wrong files?")
    starts |= set(extent)

    verbs = {}  # fn address -> name
    for f in rows(os.path.join(REFERENCE, "1.12-shapes.tsv")):
        if len(f) < 2:
            continue
        fn = int(f[1], 16)
        if GLUE[0] <= fn < GLUE[1] or f[0].startswith("Get"):
            # A getter's fire is a cache-miss re-query announced when the answer lands
            # (`GetInboxText`, `GetAuctionItemInfo`): a state event, fired by benilla's feeds.
            continue
        verbs.setdefault(fn, f[0])
    if len(verbs) < 1000:
        sys.exit(f"1.12-shapes.tsv gave only {len(verbs)} verbs — wrong path?")
    starts |= set(verbs)

    sites = []  # (site va, event name)
    for f in rows(os.path.join(REFERENCE, "1.12-event-firesites.tsv")):
        if len(f) < 4 or not f[0].startswith("0x") or not f[3] or f[2] == "dyn":
            continue
        va, eid = int(f[0], 16), int(f[2])
        if GLUE[0] <= va < GLUE[1] or eid < UNIT_WINDOW_FIELDS:
            continue
        sites.append((va, f[3]))
    if len(sites) < 300:
        sys.exit(f"1.12-event-firesites.tsv gave only {len(sites)} sites — wrong path?")

    # One pass over the disassembly: every call/jmp edge, and every padding boundary (an `int3`
    # run, or a `nop` run right after a `ret`). A `nop` run after a `jmp` is a loop head's
    # alignment inside a function, not a start.
    edges = []
    after_pad = False
    ended = False  # the previous real instruction was a `ret`
    n_lines = 0
    disasm = a.disasm
    if not os.path.exists(disasm):
        sys.exit(f"no disassembly at {disasm}")
    for line in open(disasm, encoding="utf-8", errors="replace"):
        m = LINE.match(line)
        if not m:
            continue
        n_lines += 1
        va, mnem, ops = int(m.group(1), 16), m.group(2), m.group(3) or ""
        if mnem == "int3" or (mnem == "nop" and (ended or after_pad)):
            after_pad = True
            continue
        if after_pad:
            starts.add(va)
            after_pad = False
        ended = mnem == "ret"
        b = BRANCH.match(f"{mnem} {ops}".strip())
        if b:
            edges.append((va, b.group(1), int(b.group(2), 16)))
    if n_lines < 1_000_000:
        sys.exit(f"{disasm} gave only {n_lines} lines — wrong file?")
    starts |= {t for _, k, t in edges if k == "call"}
    ordered = sorted(starts)

    def enclosing(va):
        """The function holding `va`; None when `va` lies past its candidate's extent."""
        fn = ordered[bisect.bisect_right(ordered, va) - 1]
        if fn in extent and va >= fn + extent[fn]:
            return None
        return fn

    callers = collections.defaultdict(set)
    for va, kind, target in edges:
        if kind == "call" or target in starts:  # a jmp to a function start is a tail call
            src = enclosing(va)
            if src is not None and src != target:
                callers[target].add(src)

    # (verb, event) -> {"shape", "via", "sites"}
    table = {}

    def add(verb, event, shape, via, site):
        row = table.setdefault((verb, event), {"shape": shape, "via": via, "sites": set()})
        if shape == "body":  # a body fire outranks a helper fire for the same pair
            row["shape"], row["via"] = "body", "-"
        row["sites"].add(site)

    unattributed = 0
    for va, event in sites:
        fn = enclosing(va)
        if fn is None:
            unattributed += 1
            continue
        if fn in verbs:
            add(verbs[fn], event, "body", "-", va)
            continue
        cs = callers.get(fn, set())
        if cs and len(cs) <= MAX_HELPER_CALLERS and all(c in verbs for c in cs):
            for c in cs:
                add(verbs[c], event, "helper", f"0x{fn:x}", va)

    out = os.path.normpath(a.out)
    with open(out, "w", encoding="utf-8") as fh:
        fh.write(
            "# The 1.12.1 client's FrameScript events fired from inside a Lua verb, keyed by the verb.\n"
            "# Generated by scripts/gen-reference-verb-events.py from reference/1.12-shapes.tsv,\n"
            "# reference/1.12-event-firesites.tsv and the maintainer's analysis.\n"
            "#\n"
            "# shape  body   = a fire site inside the verb's own extent.\n"
            "#        helper = a fire site inside a function that only registered verbs call\n"
            "#                 (at most 8 of them); `via` is that helper. A helper with any\n"
            "#                 non-verb caller produces a state event and is not here.\n"
            "# sites  the fire sites themselves.\n"
            "# A floor, not a census: a verb with no row may still fire an event.\n"
            "# A site past its candidate function's extent is left out rather than mis-filed.\n"
            f"# {len(table)} pairs over {len({v for v, _ in table})} verbs and"
            f" {len({e for _, e in table})} events; {unattributed} site(s) past a verified extent.\n"
            "verb\tevent\tshape\tvia\tsites\n"
        )
        for (verb, event), row in sorted(table.items()):
            fh.write(
                "\t".join(
                    (
                        verb,
                        event,
                        row["shape"],
                        row["via"],
                        "|".join(f"0x{s:x}" for s in sorted(row["sites"])),
                    )
                )
                + "\n"
            )
    print(
        f"wrote {out}: {len(table)} pairs, {len({v for v, _ in table})} verbs,"
        f" {len({e for _, e in table})} events"
    )


if __name__ == "__main__":
    main()
