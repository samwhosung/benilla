#!/usr/bin/env python3
"""Fail on a doc link whose target is declared nowhere in the workspace.

It reads rustdoc's unresolved-link warnings and fails on a `crate::`, `super::`, `self::` or
`Self::` path whose leaf name is declared nowhere: the item is gone. Every other warning is a
convention here, not rot (a private module named through its parent, another crate's module
named with `crate::`, a bare name for an unimported bevy type).

    scripts/doc-links.py            # the gate: exit 1 on any dead-target link
    scripts/doc-links.py --report   # + the benign counts

The leaf test is a loose declaration grep: a name declared anywhere counts, since what it catches
is a deleted item, not a misspelled path.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def run_rustdoc() -> str:
    out = subprocess.run(
        ["cargo", "doc", "--workspace", "--no-deps", "--document-private-items"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    return out.stderr + out.stdout


def declared_names() -> set[str]:
    """Every name the workspace declares, in one scan: a grep per link is too slow for a gate."""
    names: set[str] = set()
    # `fn` gets its own pattern: in one alternation, `const fn foo` captures "fn" and never
    # indexes `foo`.
    pats = [
        re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)"),
        re.compile(r"\b(?:struct|enum|trait|type|const|static|union|mod)\s+([A-Za-z_][A-Za-z0-9_]*)"),
        re.compile(r"\bmacro_rules!\s+([A-Za-z_][A-Za-z0-9_]*)"),
    ]
    for rs in (ROOT / "crates").rglob("*.rs"):
        try:
            text = rs.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        for pat in pats:
            names.update(pat.findall(text))
    names.discard("fn")
    return names


def main() -> int:
    if any(a in ("-h", "--help") for a in sys.argv[1:]):
        print(__doc__.strip())
        return 0
    report = "--report" in sys.argv
    text = run_rustdoc()
    found = re.findall(r"warning: unresolved link to `([^`]+)`\n\s*--> ([^\s:]+):(\d+)", text)

    names = declared_names()
    dead, benign = [], 0
    for target, path, line in found:
        # Only a `crate::`/`super::`/`self::`/`Self::` path asserts a local item; a bare name may
        # be a bevy type not imported for linking (`SystemParam`), which is not rot.
        if not re.match(r"^(crate|super|self|Self)::", target):
            benign += 1
            continue
        leaf = target.split("::")[-1].split("(")[0].strip()
        if not leaf or not re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", leaf):
            benign += 1
            continue
        if leaf in names:
            benign += 1
        else:
            dead.append((target, path, line))

    if report:
        print(f"doc-links: {len(found)} unresolved-link warnings")
        print(f"doc-links: {benign} name something that still exists (see this file's header)")

    if not dead:
        print(f"doc-links ok: no doc link points at a deleted item ({len(found)} benign)")
        return 0

    print(f"doc-links FAILED: {len(dead)} doc link(s) point at something that no longer exists:\n")
    for target, path, line in dead:
        print(f"  {path}:{line}\n      [`{target}`]")
    print(
        "\nEach names an item declared nowhere in the workspace. Repoint it at whatever replaced\n"
        "the thing, or drop the brackets — a dangling link promises navigation that is not there."
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
