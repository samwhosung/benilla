#!/usr/bin/env bash
# The in-loop verify: fmt everywhere, clippy and test on the changed crates and their dependents.
#
# The change set is the fork point against main plus staged, unstaged and untracked files. A path
# under crates/<dir>/ scopes that package, docs/ and *.md scope nothing, and any other path runs
# scripts/gates.sh, the workspace-wide chain run at land.
# CHECK_FULL=1 goes straight to gates.sh.
#
#   scripts/check.sh
set -uo pipefail

# Check the checkout you stand in when it is one of this repo, else this file's (as gates.sh does).
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
[ -n "$root" ] && [ -f "$root/scripts/check.sh" ] || root="$here"
cd "$root" || exit 1

if [ "${CHECK_FULL:-}" = "1" ]; then
    exec "$root/scripts/gates.sh"
fi

# ── The round's change set ───────────────────────────────────────────────────────────────────────
base="$(git merge-base HEAD main 2>/dev/null || git rev-parse HEAD)"
changed="$( { git diff --name-only "$base" HEAD 2>/dev/null
              git diff --name-only HEAD 2>/dev/null
              git ls-files --others --exclude-standard 2>/dev/null; } | sort -u )"

if [ -z "$changed" ]; then
    echo "check: no changes against main and a clean tree — nothing to verify"
    exit 0
fi

# ── Map files → crate dirs; anything unmappable escalates to the full chain ──────────────────────
dirs=""
full_reason=""
while IFS= read -r f; do
    case "$f" in
    docs/* | *.md) ;;                                 # provably gate-inert (gates.sh's docs-only rule)
    crates/*/*) d="${f#crates/}"; dirs="$dirs DIR:crates/${d%%/*}" ;;
    *) full_reason="$f" ;;
    esac
done <<EOF
$changed
EOF

if [ -n "$full_reason" ]; then
    echo "check: '$full_reason' is outside the crate map — the whole workspace could be affected."
    echo "check: escalating to the full chain: scripts/gates.sh"
    exec "$root/scripts/gates.sh"
fi

dirlist="$(printf '%s\n' $dirs | sed 's/.*DIR://' | sort -u)"
if [ -z "$dirlist" ]; then
    echo "check: docs-only round ($(printf '%s\n' "$changed" | wc -l | tr -d ' ') file(s)) — no gate can change; fmt only"
    cargo fmt --all -- --check || exit 1
    echo "check: green (docs only)"
    exit 0
fi

# ── Changed crates → reverse-dependency closure, off cargo metadata ──────────────────────────────
pkgs="$(python3 - "$root" $dirlist <<'PY'
import json, subprocess, sys, os
root, dirs = sys.argv[1], set(sys.argv[2:])
meta = json.loads(subprocess.run(
    ["cargo", "metadata", "--format-version", "1", "--no-deps"],
    capture_output=True, text=True, cwd=root, check=True).stdout)
by_dir, deps = {}, {}
names = {p["name"] for p in meta["packages"]}
for p in meta["packages"]:
    by_dir[os.path.relpath(os.path.dirname(p["manifest_path"]), root)] = p["name"]
    deps[p["name"]] = {d["name"] for d in p["dependencies"] if d["name"] in names}
changed = {by_dir[d] for d in dirs if d in by_dir}
missing = [d for d in dirs if d not in by_dir]
if missing:  # a changed dir cargo doesn't know: not our map's to scope
    print("FULL " + " ".join(missing)); sys.exit(0)
scope = set(changed)
grew = True
while grew:
    grew = False
    for name, ds in deps.items():
        if name not in scope and ds & scope:
            scope.add(name); grew = True
print(" ".join(sorted(scope)))
PY
)" || { echo "check: cargo metadata failed — falling back to the full chain"; exec "$root/scripts/gates.sh"; }

case "$pkgs" in FULL*)
    echo "check: changed path(s) not in the workspace map (${pkgs#FULL }) — full chain"
    exec "$root/scripts/gates.sh" ;;
esac

pflags=""
for p in $pkgs; do pflags="$pflags -p $p"; done
echo "check: scope = $pkgs"
echo "check:   (changed crates + everything that depends on them; the full chain runs once, at land)"

log="$(mktemp "${TMPDIR:-/tmp}/benilla-check.XXXXXX")"
skips="$(mktemp "${TMPDIR:-/tmp}/benilla-check-skips.XXXXXX")"
trap 'rm -f "$log" "$skips"' EXIT
# Each gate logs to a file tailed afterwards: piping through `tail` would mask its exit code.
run() {
    local name="$1"; shift
    if ! "$@" >"$log" 2>&1; then
        echo "CHECK FAILED: $name"
        tail -30 "$log"
        exit 1
    fi
    echo "check ok: $name"
}

run fmt cargo fmt --all -- --check
run clippy cargo clippy $pflags --all-targets -- -D warnings
run test env BENILLA_SKIP_LOG="$skips" cargo test $pflags
# Data-gated tests skip without the install or the addon corpus and libtest swallows their line,
# so each skip is also logged to $BENILLA_SKIP_LOG and counted here.
if [ -s "$skips" ]; then
    echo "  test: $(wc -l <"$skips" | tr -d ' ') data-gated tests SKIPPED on this machine —"
    sort "$skips" | uniq -c | sort -rn | sed 's/^ *\([0-9]*\) \(.*\)/    \1 × \2/'
    echo "    (they run where the data is: docs/CONTRIBUTING.md, \"Setting up\")"
fi

echo "CHECK GREEN (scoped: $pkgs)"
echo "  (the full chain, scripts/gates.sh, runs once on the tree that lands; a run before it repeats it)"
