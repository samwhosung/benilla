# benilla

A from-scratch World of Warcraft 1.12.1 client in Rust and Bevy. The reference client is the
spec: benilla is a faithful, modern implementation of it, not a place to get creative.

Read, in this order:

1. `docs/METHOD.md` — the rules and how we work. Binding for every session.
2. `docs/MAP.md` — what is built right now, generated from the code. Orient here before
   touching anything.
3. `docs/CONTRIBUTING.md` — what gets in and how a change is judged.

What the real client does comes from [benilla-re](https://github.com/samwhosung/benilla-re), the
reverse engineering of 1.12.1, before you build: clone it beside this repo and grep it, or look an
address up at `https://samwhosung.github.io/benilla-re/?0x5e6853`. `docs/CONTRIBUTING.md`, "The
reference", says how to read it and what to do when it has a gap.

The rest of the root is code and data: `crates/` the workspace, `reference/` the 1.12 name
catalogues the tests check against, `scripts/` the gates and instruments, `third_party/`
vendored code.

@docs/METHOD.md
