//! Which build is running: the commit the binary was built from, stamped at compile time by the
//! launcher shims' build script (`benilla-buildstamp`) and passed to `run` as the [`BuildId`]
//! resource. It shows in the startup log line ([`banner`]) and in the debug panel's footer.

use bevy::prelude::*;

/// The launcher shim's compile-time git stamp. The git fields are empty when the build had no
/// checkout or no `git` on `PATH`, which [`BuildId::summary`] reports as an unknown build.
#[derive(Resource, Clone, Copy)]
pub struct BuildId {
    /// The full sha, which the debug panel copies.
    pub sha: &'static str,
    /// Git's own abbreviation of [`sha`](Self::sha).
    pub short: &'static str,
    /// The commit date of [`sha`](Self::sha) (`YYYY-MM-DD`), not the build date.
    pub date: &'static str,
    /// The cargo profile directory: `debug`, `release` or `ship`.
    pub profile: &'static str,
}

impl BuildId {
    /// The one-line build id: short sha, commit date and profile.
    pub fn summary(&self) -> String {
        if self.short.is_empty() {
            format!("unknown (built without a git checkout) · {}", self.profile)
        } else {
            format!("{} · {} · {}", self.short, self.date, self.profile)
        }
    }
}

/// Log the build id once at startup, not env-gated, so every pasted log carries it.
pub fn banner(build: Res<BuildId>) {
    info!("benilla build {}", build.summary());
}
