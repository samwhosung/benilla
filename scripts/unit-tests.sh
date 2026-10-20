#!/usr/bin/env bash
# The test gates' runner: the tests of every target the cargo package and target flags it is given
# select, all run, a failure not stopping the rest. Under cargo-nextest where it is installed, which
# runs every test binary's tests in one pool (the workspace takes 59 s to `cargo test`'s 103 s on a
# warm tree, M4 Max); else `cargo test`. nextest runs no doctests: `scripts/doc-tests.sh` does.
#
#   scripts/unit-tests.sh --workspace
#   scripts/unit-tests.sh -p benilla-app --no-default-features --lib
set -euo pipefail
if cargo nextest --version >/dev/null 2>&1; then
    exec cargo nextest run --no-fail-fast --no-tests=pass "$@"
fi
exec cargo test --no-fail-fast "$@"
