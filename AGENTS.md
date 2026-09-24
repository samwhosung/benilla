# benilla

A from-scratch World of Warcraft 1.12.1 client in Rust and Bevy. The reference client is the
spec: benilla is a faithful, modern implementation of it, not a place to get creative.

Read, in this order:

1. `docs/METHOD.md` — the rules and how we work. Binding for every session.
2. `docs/MAP.md` — what is built right now, generated from the code. Orient here before
   touching anything.
3. `docs/CONTRIBUTING.md` — what gets in and how a change is judged.

The rest of the root is code and data: `crates/` the workspace, `reference/` the 1.12 name
catalogues the tests check against, `scripts/` the gates and instruments, `third_party/`
vendored code.

@docs/METHOD.md
