#!/usr/bin/env bash
# The doctests `scripts/unit-tests.sh` leaves to `cargo test` under cargo-nextest, which runs none:
# those of the library packages among the ones named, or of every workspace library with
# `--workspace`. Without nextest the unit-test run has run them already, so this does nothing.
#
#   scripts/doc-tests.sh --workspace
#   scripts/doc-tests.sh benilla-ui benilla-app
set -euo pipefail
cargo nextest --version >/dev/null 2>&1 || exit 0
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
flags="$(python3 - "$root" "$@" <<'PY'
import json, subprocess, sys
root, want = sys.argv[1], sys.argv[2:]
meta = json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps"],
                                 capture_output=True, text=True, cwd=root, check=True).stdout)
libs = [p["name"] for p in meta["packages"]
        if any("lib" in t["kind"] for t in p["targets"])
        and (want == ["--workspace"] or p["name"] in want)]
print(" ".join("-p " + n for n in sorted(libs)))
PY
)"
[ -n "$flags" ] || exit 0
# shellcheck disable=SC2086 # one word per flag
exec cargo test --no-fail-fast --doc $flags
