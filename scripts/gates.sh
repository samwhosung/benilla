#!/usr/bin/env bash
# The land gate chain: every gate in order, stopping at the first failure.
#
# fmt, clippy, test, test-no-install, doc-links, pass-span-lint, player-build, player-tests,
# enforcer, enforcer-no-install. A failure prints the gate's log tail and exits nonzero; a green
# run stamps the tree, which then re-greens without running. GATES_FORCE=1 runs the chain anyway,
# WOW_DATA picks the install (set and empty: none) and TMPDIR holds the scratch files; the script
# sets BENILLA_REQUIRE_DATA, BENILLA_SKIP_LOG and WOW_WORLDVIEW_CHECK for its gates.
# `.github/workflows/ci.yml` runs the gates that need neither the install nor a display on a fork's
# pull request: a gate added here goes there too, if it can run on a bare Linux runner.
#
#   scripts/gates.sh
set -uo pipefail

# Gate the checkout $PWD is in, not this file's: a caller in one worktree may run another's copy
# by absolute path. Outside a checkout of this repo, gate this file's own checkout.
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
[ -n "$root" ] && [ -f "$root/scripts/gates.sh" ] || root="$here"
cd "$root" || exit 1
echo "gating: $root"

# ── Green-stamp memoization ──────────────────────────────────────────────────────────────────────
# A green run stamps target/.gates-green with the working tree's hash (untracked files included,
# through a throwaway index that leaves the real one alone), rustc's version, this script's hash,
# the install path and whether the addon corpus link is there: the data-gated tests and the
# enforcer run only where the data resolves, and the link is gitignored, so the hash misses it.
# The install is compared last: resolving it is a `cargo run`.
stamp="target/.gates-green"
corpus=""
[ -d "$root/wow-addons-vanilla" ] && corpus="+corpus"
tree_key() {
    local tmpidx t
    tmpidx="$(mktemp "${TMPDIR:-/tmp}/benilla-gates-idx.XXXXXX")" && rm -f "$tmpidx" || return 1
    t="$( (export GIT_INDEX_FILE="$tmpidx"
           git read-tree HEAD && git add -A . && git write-tree) 2>/dev/null )"
    rm -f "$tmpidx"
    [ -n "$t" ] || return 1
    printf '%s|%s|%s' "$t" "$(rustc -V 2>/dev/null)" \
        "$(shasum "$root/scripts/gates.sh" 2>/dev/null | cut -d' ' -f1)"
}
resolve_wow() { cargo run -q -p benilla-formats --example where 2>/dev/null || true; }
# A delta only under docs/ or in top-level *.md files keeps the stamped verdict, since no gate
# reads those paths; a gate that starts to read one must narrow this pattern.
docs_only_delta() { # $1 = stamped tree, $2 = current tree
    git cat-file -e "$1^{tree}" 2>/dev/null && git cat-file -e "$2^{tree}" 2>/dev/null || return 1
    [ -z "$(git diff-tree -r --name-only "$1" "$2" 2>/dev/null | grep -vE '^(docs/|[^/]*\.md$)' || true)" ]
}
key_start="$(tree_key || true)"
if [ "${GATES_FORCE:-}" != "1" ] && [ -n "$key_start" ] && [ -f "$stamp" ]; then
    old="$(cat "$stamp" 2>/dev/null)"
    old_key="${old%|*}" # tree|rustc|script — the part that must match (bar the tree, see below)
    memo=""
    if [ "$old_key" = "$key_start" ]; then
        memo="this exact tree already passed"
    elif [ "${old_key#*|}" = "${key_start#*|}" ] && docs_only_delta "${old_key%%|*}" "${key_start%%|*}"; then
        memo="this tree differs from one that passed only under docs/ or in top-level *.md, which no gate reads"
    fi
    if [ -n "$memo" ] && [ "${old##*|}" = "$(resolve_wow)$corpus" ]; then
        echo "ALL GATES GREEN (memoized — $memo; GATES_FORCE=1 re-runs)"
        [ -d target ] && printf '%s\t%s\t%s\t%s\t%s\n' "$(date -u +%FT%TZ)" \
            "$(git rev-parse --short HEAD 2>/dev/null || echo '-')" chain 0 memo >>target/.gates-timing 2>/dev/null
        echo "  (a clean run is the fourth gate: scripts/smoke.sh — live logout/re-login round trip)"
        exit 0
    fi
fi

log="$(mktemp "${TMPDIR:-/tmp}/benilla-gates.XXXXXX")"
skips="$(mktemp "${TMPDIR:-/tmp}/benilla-gates-skips.XXXXXX")"
trap 'rm -f "$log" "$skips"' EXIT

# libtest hides a passing test's output, so `install.rs`'s `skipped` also appends each data-gated
# skip to `$BENILLA_SKIP_LOG`; this prints them, grouped, after a test rung and empties the file.
report_skips() { # $1 = rung name
    [ -s "$skips" ] || return 0
    echo "  $1: $(wc -l <"$skips" | tr -d ' ') data-gated tests SKIPPED on this machine —"
    sort "$skips" | uniq -c | sort -rn | sed 's/^ *\([0-9]*\) \(.*\)/    \1 × \2/'
    echo "    (they run where the data is: docs/CONTRIBUTING.md, \"Setting up\")"
    : >"$skips"
}

# ── Timing ───────────────────────────────────────────────────────────────────────────────────────
# target/.gates-timing gets one tab-separated line per gate: UTC time, HEAD's short sha, gate,
# seconds, verdict. A memo hit and a skipped gate are logged at 0 s, the whole chain as `chain`.
timing="target/.gates-timing"
chain_t0=$SECONDS
tree_sha="$(git rev-parse --short HEAD 2>/dev/null || echo '-')"
note_timing() { # $1 = gate, $2 = seconds, $3 = verdict
    [ -d target ] || return 0
    printf '%s\t%s\t%s\t%s\t%s\n' "$(date -u +%FT%TZ)" "$tree_sha" "$1" "$2" "$3" >>"$timing" 2>/dev/null || true
}

run() {
    local name="$1"
    shift
    local t0=$SECONDS
    if ! "$@" >"$log" 2>&1; then
        echo "GATE FAILED: $name ($((SECONDS - t0)) s)"
        note_timing "$name" "$((SECONDS - t0))" "FAILED"
        tail -30 "$log"
        exit 1
    fi
    echo "gate ok: $name ($((SECONDS - t0)) s)"
    note_timing "$name" "$((SECONDS - t0))" "ok"
}

run fmt cargo fmt --all -- --check
run clippy cargo clippy --workspace --all-targets -- -D warnings

# test: where the install resolves and the `wow-addons-vanilla` corpus link exists,
# BENILLA_REQUIRE_DATA=1 refuses a data-gated test that skips; with the install alone it would
# fail every corpus test. `$wow_data` is resolved once, for player-tests and the stamp too.
wow_data="$(resolve_wow)"
require_data=""
if [ -n "$wow_data" ] && [ -n "$corpus" ]; then
    require_data=1
else
    echo "gates: NOTE — install or addon corpus not found here (install='${wow_data:-none}'," \
         "corpus=$([ -d "$root/wow-addons-vanilla" ] && echo yes || echo no)):" \
         "data-gated tests skip silently on this machine"
fi
run test env ${require_data:+BENILLA_REQUIRE_DATA=1} BENILLA_SKIP_LOG="$skips" cargo test --workspace
report_skips test

# test-no-install: refuses a test that reads the install without `wow_data_or_skip!`. `WOW_DATA=`
# (set, empty) is the resolver's "no install"; no build reads it, so the test binaries are reused.
run test-no-install env -u BENILLA_REQUIRE_DATA WOW_DATA= cargo test --workspace

# doc-links: refuses a doc link to a `crate::`, `super::`, `self::` or `Self::` path whose leaf is
# declared nowhere in the workspace. Narrow on purpose: every other unresolved link is a convention.
run doc-links scripts/doc-links.py

# pass-span-lint: refuses a second `pass_span` on a render pass whose span is still open, a fatal
# wgpu validation error on Vulkan and a silent no-op on Metal and DX12, which no macOS run sees.
run pass-span-lint scripts/pass-span-lint.py

# player-build: `benilla` without `dev`, which compiles out the debug panel, perf HUD, inspector,
# capture harness and probes, so it fails when any other code names one of them.
run player-build cargo build -p benilla --no-default-features

# player-tests: the unit tests of the `cfg(not(feature = "dev"))` code (the resolver skips the
# source tree; the state folder sits beside the binary), which no other gate runs. WOW_DATA, which
# a player build reads too, goes in for the shipped-UI tests that read FrameXML off the install; no
# BENILLA_REQUIRE_DATA, since without `dev` only $BENILLA_ADDON_CORPUS finds the addon corpus.
run player-tests env ${wow_data:+WOW_DATA="$wow_data"} BENILLA_SKIP_LOG="$skips" \
    cargo test -p benilla-formats -p benilla-app --no-default-features --lib
report_skips player-tests

# enforcer: `benilla-worldview` boots the engine plugins with no server, login, UI or player, in a
# small window, and refuses any system whose parameters need a gameplay resource, a coupling no
# compiler sees. The client's own resolver decides whether there is an install, so it skips
# exactly where the client could not run.
if cargo run -q -p benilla-formats --example where >/dev/null 2>&1; then
    run enforcer env WOW_WORLDVIEW_CHECK=10 cargo run -q -p benilla-worldview
else
    echo "gate SKIPPED: enforcer (no WoW install found — the engine boot check needs one;"
    echo "             \`cargo run -p benilla-formats --example where\` says where it looked)"
    note_timing enforcer 0 SKIPPED
fi

# enforcer-no-install: the engine boot with no client data, which a dev build reaches only under
# `WOW_DATA=`; refuses a system that needs a resource inserted only when there is client data.
# Five seconds suffice: with nothing to stream, every system validates in the first frames.
run enforcer-no-install env WOW_DATA= WOW_WORLDVIEW_CHECK=5 cargo run -q -p benilla-worldview

echo "ALL GATES GREEN ($((SECONDS - chain_t0)) s; per-gate seconds in $timing)"
note_timing chain "$((SECONDS - chain_t0))" ok

# Stamp only if the tree still hashes as it did at the start: an edit during the run would
# otherwise memoize a green for a tree the chain never saw.
key_end="$(tree_key || true)"
if [ -n "$key_end" ] && [ "$key_end" = "$key_start" ] && [ -d target ]; then
    printf '%s|%s%s\n' "$key_end" "$wow_data" "$corpus" > "$stamp" 2>/dev/null || true
fi

# The clean-run gate, scripts/smoke.sh, needs a server and a window: named here, not run.
echo "  (a clean run is the fourth gate: scripts/smoke.sh — live logout/re-login round trip)"
