#!/usr/bin/env python3
"""Vendor benilla-re's tables into `reference/` at the commit `reference/PIN` names.

    scripts/sync-reference.py <benilla-re checkout>            copy the tables at the PIN
    scripts/sync-reference.py <benilla-re checkout> --check    fail when a copy differs

Each file is `git show <PIN>:tables/<name>` byte for byte. To update, put a new 40-hex commit of
benilla-re in `reference/PIN` and run the script without `--check`.
"""
import os
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REFERENCE = os.path.join(REPO, "reference")

# The benilla-re tables benilla reads.
TABLES = [
    "bindings.tsv",
    "chat_groups.tsv",
    "chat_types.tsv",
    "cvars.tsv",
    "event_args.tsv",
    "events.tsv",
    "glue_events.tsv",
    "glue_surface.tsv",
    "lua_globals.tsv",
    "messages.tsv",
    "verb_events.tsv",
]


def git_show(checkout, pin, name):
    r = subprocess.run(
        ["git", "-C", checkout, "show", f"{pin}:tables/{name}"], capture_output=True
    )
    if r.returncode != 0:
        sys.exit(f"sync-reference: {pin[:12]}:tables/{name} not in {checkout}: {r.stderr.decode().strip()}")
    return r.stdout


def main():
    args = [a for a in sys.argv[1:] if a != "--check"]
    check = "--check" in sys.argv[1:]
    if len(args) != 1:
        sys.exit("usage: scripts/sync-reference.py <benilla-re checkout> [--check]")
    checkout = args[0]
    with open(os.path.join(REFERENCE, "PIN")) as f:
        pin = f.read().strip()
    if len(pin) != 40 or any(c not in "0123456789abcdef" for c in pin):
        sys.exit("sync-reference: reference/PIN must hold a full 40-hex commit")
    for ref in ("origin/main", "main"):
        r = subprocess.run(["git", "-C", checkout, "merge-base", "--is-ancestor", pin, ref],
                           capture_output=True)
        if r.returncode in (0, 1):
            if r.returncode == 1:
                sys.exit(f"sync-reference: the PIN {pin[:12]} is not on benilla-re's {ref}")
            break
    stray = sorted(n for n in os.listdir(REFERENCE) if n.endswith(".tsv") and n not in TABLES)
    if stray:
        sys.exit("sync-reference: reference/ holds tables it does not vendor: " + ", ".join(stray))
    bad = []
    for name in TABLES:
        want = git_show(checkout, pin, name)
        path = os.path.join(REFERENCE, name)
        have = open(path, "rb").read() if os.path.exists(path) else None
        if have == want:
            continue
        if check:
            bad.append(name)
        else:
            with open(path, "wb") as f:
                f.write(want)
            print(f"sync-reference: wrote reference/{name}")
    if bad:
        sys.exit(
            "sync-reference: differs from benilla-re at the PIN: "
            + ", ".join(bad)
            + " (run scripts/sync-reference.py <benilla-re checkout>)"
        )
    if check:
        print(f"sync-reference: {len(TABLES)} tables match benilla-re at {pin[:12]}")


if __name__ == "__main__":
    main()
