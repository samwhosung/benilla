#!/usr/bin/env python3
"""Regenerate `reference/1.12-events.tsv`: each 1.12 FrameScript event and the arguments it carries.

    scripts/gen-reference-events.py [--catalog FILE] [--firesites FILE] [--out FILE]

The inputs are vendored under `reference/`, so this runs from any clone and reproduces the
committed table byte for byte:

  - `1.12-event-catalog.tsv`   eventId -> name, off the name-pointer array at `.data 0xbe1198`
  - `1.12-event-firesites.tsv` every `call` and tail-`jmp` to `FrameScript_SignalEvent`
                               (`0x703e50`, no varargs) and `SignalEvent2` (`0x703f50`,
                               printf-style), with the format string each site pushes

A literal fire site is not every producer; the families are:

  signal              a `0x703e50` site: `__fastcall(ecx = id)`, a plain `ret`, zero Lua values.
  signal2             a `0x703f50` site: the pushed format string is the argument shape.
  unit-field-bridge   the generic UpdateField -> event bridge (`0x51bbb0` registers one watch per
                      named unit-window field; `0x51bd50` -> `0x515e50` fans out over the unit
                      tokens and calls `0x703f50(id, "%s", token)`). Every named event id below
                      `0xb6` is produced there and has no fire-site row.
  token-fanout        the same `0x515e50` fan-out reached with a literal id in `edx`: also `%s`,
                      also absent from the fire sites.

Confidence, and what a consumer may gate on:

  exact      every contributing producer's shape is known. Gate on this.
  advisory   a contributing site's format declares more varargs than its caller pushes
             (`0x496230` TRADE_REPLACE_ENCHANT, `0x5e4527`/`0x5e7960` UPDATE_TICKET), which hands
             Lua undefined values; do not copy it.
  none       no producer family reaches the name here, which does not mean it has none. Never
             gate on it.

Glue-space sites are dropped by address: `0x703d90` loads a separate name table for the glue
screen, so the same id names a different event there. Their names are blank in the input, and
dropping by address keeps a name filled in later from leaking through.
"""
import argparse
import os
import sys

# The `Source\Glue\` fire-site ranges (`1.12-event-firesites.tsv`'s header): glue-space ids.
GLUE_SITES = [(0x46AA34, 0x46AA34), (0x46BC9D, 0x46C52E), (0x46E73D, 0x46E73D), (0x47461E, 0x47461E)]
# The three sites whose format string declares more varargs than the caller pushes.
DECLARES_MORE_THAN_IT_PUSHES = {0x496230: "TRADE_REPLACE_ENCHANT", 0x5E4527: "UPDATE_TICKET", 0x5E7960: "UPDATE_TICKET"}
# `0x515e50` callers with a literal id: the token fan-out reached outside the generic bridge.
TOKEN_FANOUT_IDS = [0x10, 0x16, 0x1C, 0x1D, 0x1E, 0x29, 0xB7, 0xB8, 0xB9, 0xBA, 0xBB, 0x159, 0x197, 0x20A]
# `0x51bbb0`'s walk bound (`cmp esi,0xb6`): every named unit-window field below it is watched.
UNIT_WINDOW_FIELDS = 0xB6
# The zero-argument format, spelled so the column is never empty.
NONE = "()"


REFERENCE = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "reference"))


def rows(path):
    if not os.path.exists(path):
        sys.exit(f"no table at {path}")
    for line in open(path, encoding="utf-8"):
        if line.startswith("#"):
            continue
        f = line.rstrip("\n").split("\t")
        if f and f[0] in ("eventId", "fire_site_va"):
            continue
        yield f


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--catalog", default=os.path.join(REFERENCE, "1.12-event-catalog.tsv"))
    ap.add_argument("--firesites", default=os.path.join(REFERENCE, "1.12-event-firesites.tsv"))
    ap.add_argument("--out", default=os.path.join(REFERENCE, "1.12-events.tsv"))
    a = ap.parse_args()

    catalog = {}
    for f in rows(a.catalog):
        if len(f) >= 2 and f[1]:
            catalog.setdefault(int(f[0]), f[1])
    if len(catalog) < 300:
        sys.exit(f"{a.catalog} gave only {len(catalog)} names — wrong path?")

    # name -> {ids, producers, formats, notes}
    ev = {}

    def add(name, eid, producer, fmt, note=None):
        e = ev.setdefault(name, {"ids": set(), "producers": set(), "formats": set(), "notes": []})
        e["ids"].add(eid)
        e["producers"].add(producer)
        e["formats"].add(fmt)
        if note and note not in e["notes"]:
            e["notes"].append(note)

    dropped_glue = 0
    for f in rows(a.firesites):
        if len(f) < 4:
            continue
        va = int(f[0], 16)
        dispatcher, eid, name = f[1], f[2], f[3]
        if any(lo <= va <= hi for lo, hi in GLUE_SITES):
            dropped_glue += 1
            continue
        if not name or eid == "dyn":
            continue
        fmt = NONE if dispatcher == "SignalEvent" else (f[4] if len(f) > 4 and f[4] else NONE)
        note = None
        if va in DECLARES_MORE_THAN_IT_PUSHES:
            note = f"0x{va:x} declares more varargs than it pushes — do not copy"
        add(name, int(eid), "signal" if dispatcher == "SignalEvent" else "signal2", fmt, note)

    for eid, name in catalog.items():
        if eid < UNIT_WINDOW_FIELDS:
            add(name, eid, "unit-field-bridge", "%s")
    for eid in TOKEN_FANOUT_IDS:
        if eid in catalog:
            add(catalog[eid], eid, "token-fanout", "%s")

    # Every remaining catalog name too, so "no producer found" differs from "not an event".
    for eid, name in catalog.items():
        if name not in ev:
            ev.setdefault(name, {"ids": set(), "producers": set(), "formats": set(), "notes": []})["ids"].add(eid)

    out = os.path.normpath(a.out)
    with open(out, "w", encoding="utf-8") as fh:
        fh.write(
            "# The 1.12.1 client's FrameScript events and the arguments their producers push.\n"
            "# Generated by scripts/gen-reference-events.py from reference/1.12-event-catalog.tsv\n"
            "# and reference/1.12-event-firesites.tsv.\n"
            "#\n"
            "# arg_formats  `|`-separated, one per distinct producer shape. `()` = zero Lua values.\n"
            "#              Directives: %s string, %d/%u number, %f number. An event with two\n"
            "#              producers of different shapes carries both; which one fires depends\n"
            "#              on the transition, not the name.\n"
            "# conf         exact = gate on it. advisory = a contributing site declares more\n"
            "#              varargs than it pushes. none = no producer family reaches it here,\n"
            "#              which does not mean it has none; never gate on it.\n"
            "# producers    signal (0x703e50, no varargs) | signal2 (0x703f50, format-driven) |\n"
            "#              unit-field-bridge (0x51bbb0/0x51bd50 -> 0x515e50, always %s) |\n"
            "#              token-fanout (0x515e50 with a literal id, also %s).\n"
            f"# {len(ev)} events, {sum(1 for e in ev.values() if e['producers'])} with a known producer;"
            f" {dropped_glue} glue-space fire sites dropped.\n"
            "name\tids\tproducers\targ_formats\tconf\tnote\n"
        )
        for name in sorted(ev):
            e = ev[name]
            conf = "none" if not e["producers"] else ("advisory" if e["notes"] else "exact")
            fh.write(
                "\t".join(
                    (
                        name,
                        "|".join(str(i) for i in sorted(e["ids"])),
                        "|".join(sorted(e["producers"])),
                        "|".join(sorted(e["formats"])) or "?",
                        conf,
                        "; ".join(e["notes"]),
                    )
                )
                + "\n"
            )
    print(f"wrote {out}: {len(ev)} events, {sum(1 for e in ev.values() if e['producers'])} produced")


if __name__ == "__main__":
    main()
