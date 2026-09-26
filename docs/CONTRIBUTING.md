# Contributing

benilla is a faithful 1.12.1 client. It is the foundation people build on, not the place to get
creative: a change is accepted when it makes benilla more like 1.12.1 or fixes a bug, with
evidence from the reference, in one small piece, with the gates green. Everything else is a
fork, and forks are welcome.

## What gets in

- A fix for a bug, with how to see it before and after.
- A step closer to 1.12.1: a missing packet, verb, window, effect or behaviour, done the way
  the real client does it.
- A correction where benilla and the reference disagree, with the reference fact stated.

## What does not

- Features 1.12.1 does not have, and behaviour changed because it seems better. A deviation
  from the reference is the maintainer's call and is recorded where it lives; a pull request is
  not the place to propose one.
- Anything from a WoW install: art, models, sounds, maps, data. The one exception is interface
  code (FrameXML and GlueXML), and only through the migration recipe in `docs/METHOD.md`.
- Big or mixed changes. One change per pull request, small enough to read in one sitting.

## How a change is judged

1. `scripts/gates.sh` is green: fmt, clippy with warnings denied, the workspace tests (once
   with the client data, once without, so a test that reads the install has to declare it), the
   doc-link and render-pass lints, the player build with its own tests, and the engine boot
   checks. A pull request from a fork runs the ones that need neither the install nor a display
   in CI, on Linux; the rest run on the maintainer's machine before it lands.
2. The reference fact is stated: what 1.12.1 does, and where that is known from (the client's
   behaviour you observed, a DBC field, a FrameXML line, a packet capture). The names and shapes
   under `reference/` are the surface benilla tracks.
3. A comment where it matters, one line, saying what the code does and the 1.12 fact behind it.
   No history.
4. The commit message says what changed, for a player or a developer, in one line.

## After you open it

A maintainer reads it, checks it against the reference and runs it. Most pull requests are
finished here rather than sent back: a rebase onto main, a fix, a test or a comment, pushed to
your branch as commits on top of yours, so leave "Allow edits by maintainers" ticked. It lands as
one squash-merged commit with you as its author. When what would land is mostly ours, we land our
own version with you as a co-author and close yours with a note. One that is out of scope is
closed with the reason.

## Setting up

- **The toolchain.** Stable Rust (`rust-toolchain.toml` adds clippy and rustfmt) and a C
  compiler, because the client's Lua is vendored and built from source. On macOS the Xcode
  command line tools, which also supply libclang for the audio bindings; on Linux the ALSA and
  udev development packages and pkg-config. `python3` runs two of the gates.
- **A 1.12.1 install of your own.** `WOW_DATA=<its Data folder>`, or a `WoW` link at the repo
  root, which only a dev build sees: the player build looks for `Data/` or `WoW/Data/` beside
  the binary. benilla reads the install and never writes into it. `WOW_DATA=` (set, empty)
  means "no install", which is how the no-install boot is tested on a machine that has one.
- **Without the install, green is hollow.** About a thousand tests read the install and skip
  when it is absent; six more read a corpus of vanilla addons (`BENILLA_ADDON_CORPUS=<a folder
  of addons>`, or a `wow-addons-vanilla` link at the root), third-party content that is not in
  this repo. `scripts/gates.sh` and `scripts/check.sh` print how many tests skipped and why.
  Where the data is, `BENILLA_REQUIRE_DATA=1` turns a skip into a failure, and the gates set it
  themselves when the install and the corpus both resolve.
- **A server to test against.** Any 1.12.1 server; `WOW_HOST` names it (default
  `localhost:3724`). A scripted run has no default account: `WOW_USER`, `WOW_PASS` and
  `WOW_CHAR` name a test account on your server whose login kicks nobody, all three, either in
  the environment or in a `.probe-identity` file at the repo root (one per line, never
  committed), and `scripts/smoke.sh` (the live login gate) refuses without them. The probes
  drive the body with GM commands, so give that account the top GM level.
- **Running it unattended.** The rules are `docs/METHOD.md`, "The local server"; these are the
  switches.
  - `WOW_UNATTENDED=1` reconnects instead of waiting at a dialog, and exits non-zero on a login
    it cannot pass. `WOW_NOSOUND=1` runs silent. A capture (`WOW_CAPTURE`) is both, and a rig
    (`WOW_RIG`) is unattended. `WOW_ALLOW_ACCOUNT=1` overrides the account guard.
  - On vmangos, `WOW_RIG="tauren druid 60 gear:heal-preraid-bis spec:heal-preraid-bis
    at:ThunderBluff"` finds or creates that character on the account and applies the server's
    premade sets (`WOW_RIG="gear:?"` lists them). `WOW_PROBE_CHAT="<command>"` sends one GM
    command and logs the reply as `net: server says`. An account named `probe` and digits is
    shielded: every world entry sends `.cheat god on`, `.die` clears it for a death test, and
    `WOW_GOD=off` leaves it off.
  - A trace is `WOW_MOVE_TRACE=<file>`, filtered with `WOW_MOVE_TRACE_TAGS` (`"move,snd,in"`) and
    ended by `WOW_PROBE_EXIT_AT=<seconds>`; grep it for the `rly` and `snd` lines.
  - A capture runs through `scripts/visual.sh`, whose recipe is the header of
    `crates/benilla-app/src/capture/mod.rs`. Pixel questions go through `benilla-visual crop`,
    `series` and `hotspot`.
- **The loop.** `cargo play` builds and runs the play profile. `scripts/check.sh` verifies a
  round of work; `scripts/gates.sh` is the full chain, and it opens a window for the engine boot
  checks, so it needs a display. Work on a branch.

## Reporting a bug

Open an issue: what you did, what you saw and what 1.12.1 does instead, with the server and the
platform you ran on. Questions and ideas are welcome there or on the Discord linked from the
README.

Working with an AI agent is expected. The agent reads `AGENTS.md`, and the same rules bind it.
