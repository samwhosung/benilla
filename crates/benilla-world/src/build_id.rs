//! **Which build is this?** — the commit the running binary was built from, stamped in at
//! compile time by the `benilla` launcher shim's `build.rs` (which owns the *how*, including why
//! the stamp can't go stale — and why it is stamped into the ~30-line shim rather than this
//! crate: cargo re-dirties whatever package carries the stamp on every commit)
//! and handed to [`crate::run`] as the [`BuildId`] resource.
//!
//! A report from someone else's machine — "the water reads wrong here", "it panicked on login" —
//! is only actionable against known code, and nothing else in the binary identifies it: the crate
//! version is a permanent `0.1.0`, and people build from a clone of the public snapshot repo whose
//! HEAD moves with every sync. So the sha is the version.
//!
//! Two surfaces, deliberately:
//!
//! - **The startup log line** ([`banner`], run from [`crate::preflight`]'s banner family) — this is
//!   the one that matters for someone else's run. They launch from a terminal, so the id is already
//!   in the output they paste; nobody has to know a hotkey to answer "what are you on?".
//! - **The debug panel's footer** (`` ` ``) — the same line where the reader already is when they
//!   are looking at a readout, click-to-copy for the full sha.
//!
//! Mapping a reported sha back: a **public** snapshot sha is tagged `pub/<short-sha>` on the private
//! commit it shipped, so `git show pub/<sha>` (or `git rev-parse pub/<sha>^{commit}`) names the code
//! exactly; a **private** sha is just a commit here.

use bevy::prelude::*;

/// The launcher shim's compile-time git stamp, inserted as a resource by [`crate::run`]. The git
/// fields are empty when the build had no checkout to read (a downloaded source zip, or no `git`
/// on `PATH`), which [`BuildId::summary`] reports as an unknown build.
#[derive(Resource, Clone, Copy)]
pub struct BuildId {
    /// The full 40-char sha of the commit this binary was built from (what the panel copies).
    pub sha: &'static str,
    /// Git's own abbreviation of [`sha`](Self::sha), from the same repo `pubsync` abbreviates in —
    /// so a public build's string is literally the suffix of its `pub/<sha>` tag.
    pub short: &'static str,
    /// The commit date of [`sha`](Self::sha) (`YYYY-MM-DD`) — a property of the sha, so it can
    /// never disagree with it.
    pub date: &'static str,
    /// The cargo profile directory this was built in: `debug`, `release`, or `ship`. Half
    /// of every "it runs badly" report is a debug build.
    pub profile: &'static str,
}

impl BuildId {
    /// The one-line build id: `f5fd009 · 2026-07-30 · release`.
    pub fn summary(&self) -> String {
        if self.short.is_empty() {
            format!("unknown (built without a git checkout) · {}", self.profile)
        } else {
            format!("{} · {} · {}", self.short, self.date, self.profile)
        }
    }
}

/// Log the build id once at startup — the line that answers "what version are you on?" from a
/// pasted terminal log. Not env-gated, for [`crate::preflight`]'s reason: an id nobody knows to
/// switch on is not an id.
pub fn banner(build: Res<BuildId>) {
    info!("benilla build {}", build.summary());
}
