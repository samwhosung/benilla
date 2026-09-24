#!/usr/bin/env python3
"""Regenerate `reference/1.12-globals.tsv`: the 1.12.1 client's global namespace, by origin.

    scripts/gen-reference-globals.py --capture FILE --framexml DIR [--data DIR] [--drop NAME]...
        [--out FILE]

Each name in a capture of the running reference client's in-world `_G` gets one origin:

  lua       Lua 5.0's own runtime, which mlua provides.
  engine    the C client provides it; benilla implements these in Rust.
  framexml  the shipped UI defines it (its Lua, its XML, or a `$parent`-composed child of one);
            it runs off the player's own patch chain, never hardcoded in Rust.

Attribution is by definition site in the complete 1.12 shipped-UI corpus:

  - a name assigned or `function`-declared in shipped Lua, including Lua inside a FrameXML
    document, is FrameXML's; indented assignments count, because FrameXML leaks globals out of
    function bodies (`button = getglobal(...)`);
  - a `name="..."` on any shipped XML element is FrameXML's, `virtual="true"` included: a virtual
    `<Font>` is a real font object, and 1.12 registers named virtual frames too;
  - `<a shipped name><a $parent suffix>` is a composed child (`ContainerFrame1Item16IconTexture`);
  - LUA_5_0 wins over all of these: FrameXML's `string = getglobal(...)` would otherwise claim
    the stdlib table.

The capture (`--capture`, with `--drop` naming the capturing addon's own globals) and the
extracted FrameXML (`--framexml`) are the maintainer's, not in this repo; the MPQs are the
install's (`--data`, else `$WOW_DATA`, else `WoW/Data`). FrameXML alone is not the shipped UI:
the twelve `Blizzard_*` addons live in the MPQs, and their `.lua` is reached through
`<Script file=>` in their XML, not their `.toc`. A complete corpus is 233 files; the count is
printed.
"""
import argparse
import os
import re
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# The twelve addons shipped inside the MPQs; an install's folder for each holds only a `.pub`.
BLIZZARD_ADDONS = [
    "Blizzard_AuctionUI", "Blizzard_BattlefieldMinimap", "Blizzard_BindingUI",
    "Blizzard_CombatText", "Blizzard_CraftUI", "Blizzard_GMSurveyUI",
    "Blizzard_InspectUI", "Blizzard_MacroUI", "Blizzard_RaidUI",
    "Blizzard_TalentUI", "Blizzard_TradeSkillUI", "Blizzard_TrainerUI",
]

# Lua 5.0's globals as the 1.12 client exposes them: it removes `_G`, `print`, `require`,
# `dofile`, `loadfile` and the `io`/`os`/`debug`/`coroutine` tables.
LUA_5_0 = {
    "assert", "collectgarbage", "error", "gcinfo", "getfenv", "getmetatable", "ipairs",
    "loadstring", "math", "next", "pairs", "pcall", "rawequal", "rawget", "rawset",
    "setfenv", "setmetatable", "string", "table", "tonumber", "tostring", "type",
    "unpack", "xpcall",
}

LUA_ASSIGN = re.compile(r"^[ \t]*([A-Za-z_][A-Za-z0-9_]*)\s*=[^=]", re.M)
LUA_FUNC = re.compile(r"^[ \t]*function\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(", re.M)
LUA_MULTI = re.compile(
    r"^[ \t]*([A-Za-z_][A-Za-z0-9_]*(?:\s*,\s*[A-Za-z_][A-Za-z0-9_]*)+)\s*=[^=]", re.M
)
XML_ELEM = re.compile(r"<(\w+)([^>]*?)/?>", re.S)
XML_NAME = re.compile(r'\bname\s*=\s*"([^"]*)"')
PARENT_SUFFIX = re.compile(r'\bname\s*=\s*"\$parent([A-Za-z0-9_]*)"')
# Lua inside a FrameXML document: an inline `<Script>` body (Fonts.xml's CHAT_FONT_HEIGHTS) and
# every `<Scripts>` handler element (`<OnLoad>`, `<PreClick>`, ...), matched by shape so no handler
# name is missed. `<Script file=>` is skipped: its file is already in the corpus.
XML_SCRIPT = re.compile(
    r"<((?:On|Pre|Post)\w+|Script)\b(?![^>]*\bfile\s*=)[^>]*>(.*?)</\1>", re.S
)
XML_REF = re.compile(rb'<(?:Script|Include)\s+file\s*=\s*"([^"]+)"', re.I)


def captured_globals(fixture):
    """`["globals"]["keys"]` out of the capture: the in-world `_G`, as (name, lua type)."""
    recs, cur_t, inside = [], None, False
    with open(fixture, encoding="utf-8", errors="replace") as f:
        for line in f:
            if not inside:
                inside = line.strip() == '["globals"] = {'
                continue
            if '["count"]' in line:
                break
            m = re.match(r'\s*\["t"\] = "([^"]*)",', line)
            if m:
                cur_t = m.group(1)
                continue
            m = re.match(r'\s*\["name"\] = "(.*)",\s*$', line)
            if m:
                recs.append((m.group(1), cur_t))
                cur_t = None
    return recs


def join_ref(base, ref):
    """Resolve a reference against the including file's directory, as `loader::join_ref` does."""
    ref = ref.replace("\\", "/").strip()
    parts = ref[1:].split("/") if ref.startswith("/") else (base.split("/") if base else []) + ref.split("/")
    out = []
    for p in parts:
        if p in ("", "."):
            continue
        if p == ".." and out and out[-1] != "..":
            out.pop()
        else:
            out.append(p)
    return "/".join(out)


def build_corpus(data, framexml):
    """Every file of the 1.12 shipped UI, as {corpus-relative path: text}."""
    archives = [os.path.join(data, f"{a}.MPQ") for a in ("patch-2", "patch", "interface", "base")]
    archives = [a for a in archives if os.path.exists(a)]
    mpqcat = os.path.join(REPO, "target", "debug", "examples", "mpqcat")
    if not os.path.exists(mpqcat):
        subprocess.run(
            ["cargo", "build", "-q", "-p", "benilla-mpq", "--example", "mpqcat"],
            cwd=REPO, check=True,
        )

    def mpq_read(archive_path):
        for arc in archives:
            r = subprocess.run([mpqcat, arc, archive_path], capture_output=True)
            if r.returncode == 0:
                return r.stdout
        return None

    files = {}
    fx = framexml
    for fn in sorted(os.listdir(fx)):
        p = os.path.join(fx, fn)
        if os.path.isfile(p):
            files[f"FrameXML/{fn}"] = open(p, "rb").read()
    print(f"  FrameXML: {len(files)} files")

    for addon in BLIZZARD_ADDONS:
        toc_rel = f"{addon}/{addon}.toc"
        toc = mpq_read("Interface\\AddOns\\" + toc_rel.replace("/", "\\"))
        if toc is None:
            print(f"  !! {addon}: no .toc in any archive", file=sys.stderr)
            continue
        files[toc_rel] = toc
        queue, seen = [], set()
        for line in toc.decode("utf-8", "replace").splitlines():
            line = line.strip()
            if line and not line.startswith("#"):
                queue.append(join_ref(addon, line))
        while queue:
            rel = queue.pop(0)
            if rel in seen:
                continue
            seen.add(rel)
            data = mpq_read("Interface\\AddOns\\" + rel.replace("/", "\\"))
            if data is None:
                print(f"  !! {addon}: missing {rel}", file=sys.stderr)
                continue
            files[rel] = data
            # The addons' `.lua` is reached only from their XML, never from the `.toc`.
            if rel.lower().endswith(".xml"):
                base = rel.rsplit("/", 1)[0] if "/" in rel else ""
                for m in XML_REF.finditer(data):
                    queue.append(join_ref(base, m.group(1).decode("utf-8", "replace")))
        print(f"  {addon}: {1 + len(seen)} files")
    return {k: v.decode("utf-8", "replace") for k, v in files.items()}


def lua_defs(text, into):
    into.update(LUA_ASSIGN.findall(text))
    into.update(LUA_FUNC.findall(text))
    for grp in LUA_MULTI.findall(text):
        into.update(n.strip() for n in grp.split(","))


def shipped_names(corpus):
    """Every global the shipped UI defines, plus the `$parent` suffixes children compose with."""
    names, suffixes = set(), set()
    for rel, text in corpus.items():
        if rel.lower().endswith(".lua"):
            lua_defs(text, names)
        elif rel.lower().endswith(".xml"):
            for _tag, body in XML_SCRIPT.findall(text):
                lua_defs(body, names)
            suffixes.update(s for s in PARENT_SUFFIX.findall(text) if s)
            for tag, attrs in XML_ELEM.findall(text):
                # `<Binding name="ACTIONBUTTON1">` is a key-binding command, not a global.
                if tag.lower() == "binding":
                    continue
                m = XML_NAME.search(attrs)
                if m and "$parent" not in m.group(1):
                    names.add(m.group(1))
    return names, suffixes


def composed(name, roots, suffixes, depth=0):
    """Is `name` `<a shipped name><a $parent suffix>`, recursively? A numeric tail on the parent
    side is allowed: `ContainerFrame1` and `Item16` come from Lua's `name..i`."""
    if depth > 8:
        return False
    for i in range(1, len(name)):
        head, tail = name[:i], name[i:]
        if tail not in suffixes:
            continue
        if head in roots or head.rstrip("0123456789") in roots:
            return True
        if composed(head, roots, suffixes, depth + 1):
            return True
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--capture", help="a capture of the running client's in-world _G")
    ap.add_argument("--framexml", help="the shipped FrameXML, extracted to a folder")
    ap.add_argument("--data", help="the install's Data folder, which holds the MPQs",
                    default=os.environ.get("WOW_DATA") or os.path.join(REPO, "WoW", "Data"))
    # The capturing addon's own globals (its holder frame, SavedVariables and slash token) are the
    # instrument, not the client. `__framescript_meta` stays in: it is the client's own shared
    # frame metatable.
    ap.add_argument("--drop", action="append", default=[], help="a global of the capturing addon")
    ap.add_argument("--out", default=os.path.join(REPO, "reference", "1.12-globals.tsv"))
    args = ap.parse_args()
    if not args.capture or not args.framexml:
        sys.exit(
            "this table is derived from a runtime capture of the reference client's _G and the "
            "extracted FrameXML, which are not in this repo: pass --capture and --framexml. The "
            "committed reference/1.12-globals.tsv is the surface benilla tracks."
        )
    if not os.path.exists(args.capture):
        sys.exit(f"no capture at {args.capture}")

    print("building the 1.12 shipped-UI corpus...")
    corpus = build_corpus(args.data, args.framexml)
    print(f"  TOTAL: {len(corpus)} files" + ("" if len(corpus) == 233 else "  ** expected 233 **"))

    recs = [(n, t) for n, t in captured_globals(args.capture) if n not in set(args.drop)]
    defined, suffixes = shipped_names(corpus)
    print(f"captured globals: {len(recs)}   shipped definitions: {len(defined)}   "
          f"$parent suffixes: {len(suffixes)}")

    rows, tally = [], {}
    captured = set()
    for name, t in recs:
        captured.add(name)
        if name in LUA_5_0:
            origin = "lua"
        elif name in defined or composed(name, defined, suffixes):
            origin = "framexml"
        else:
            origin = "engine"
        rows.append((name, t, origin))
        tally[(origin, t)] = tally.get((origin, t), 0) + 1

    # ── The LoadOnDemand names ─────────────────────────────────────────────────────────────────
    # The twelve `Blizzard_*` addons are LoadOnDemand, so a live capture holds their globals only
    # if their windows were opened first. Every name the shipped UI defines that the capture lacks
    # joins with type `lod`: the table answers "is this a 1.12 name?", and the halves stay apart.
    lod = 0
    for name in sorted(defined - captured):
        rows.append((name, "lod", "framexml"))
        tally[("framexml", "lod")] = tally.get(("framexml", "lod"), 0) + 1
        lod += 1
    print(f"shipped-UI definitions the capture did not contain: {lod} (LoadOnDemand + unrealized)")

    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out, "w") as f:
        f.write("# The 1.12.1 client's global namespace. Generated by scripts/gen-reference-globals.py.\n")
        f.write("# Source: a capture of the running client's in-world _G (the maintainer's),\n")
        f.write("# attributed against the complete 1.12 shipped-UI corpus.\n")
        f.write("# Also every name that corpus defines but the capture lacks, as type `lod`: the\n")
        f.write("# twelve Blizzard_* addons are LoadOnDemand, so a live dump misses them unless\n")
        f.write("# their windows were opened.\n")
        f.write("# name\ttype\torigin(lua|engine|framexml)\n")
        for row in sorted(rows):
            f.write("\t".join(row) + "\n")

    print(f"\nwrote {args.out}  ({len(rows)} names)")
    for k in sorted(tally):
        print(f"  {k[0]:9s} {k[1]:9s} {tally[k]}")


main()
