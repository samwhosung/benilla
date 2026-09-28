//! Which build is running: the release and the commit the binary was built from, stamped at
//! compile time by the launcher shims' build script (`benilla-buildstamp`) and passed to `run` as
//! the [`BuildId`] resource, and whether a crate on top of benilla extended it. It shows in the
//! startup log line ([`banner`]), in crash reports and in the debug panel's footer.

use bevy::prelude::*;

/// The launcher shim's compile-time stamp. The git fields are empty when the build had no
/// checkout or no `git` on `PATH`, which [`BuildId::summary`] reports as an unknown commit.
#[derive(Resource, Clone, Copy, Default)]
pub struct BuildId {
    /// The workspace version from `Cargo.toml`, which every release bumps.
    pub version: &'static str,
    /// `git describe --tags --long` against the nearest `v*` tag (`v0.2.0-12-gb17be27`); empty
    /// when no release tag is in reach, as in a source archive or a shallow clone.
    pub describe: &'static str,
    /// The full sha, which the debug panel copies.
    pub sha: &'static str,
    /// Git's own abbreviation of [`sha`](Self::sha).
    pub short: &'static str,
    /// The commit date of [`sha`](Self::sha) (`YYYY-MM-DD`), not the build date.
    pub date: &'static str,
    /// The cargo profile directory: `debug`, `release` or `ship`.
    pub profile: &'static str,
    /// Whether a crate on top of benilla added plugins through `benilla_app::run_with`. The entry
    /// point sets it, so a launcher leaves it to `..Default::default()`.
    pub extended: bool,
}

impl BuildId {
    /// The release this build is: the version, and `+N` when the commit is N past that version's
    /// tag. With no tag for this version in reach (none fetched, or the release commit before its
    /// tag exists), the version alone.
    pub fn release(&self) -> String {
        match commits_past_tag(self.describe, self.version) {
            Some(n) if n > 0 => format!("{}+{n}", self.version),
            _ => self.version.to_owned(),
        }
    }

    /// The one-line build id: release, short sha, commit date and profile, then `extended` when a
    /// crate on top added plugins, so a report from that client reads apart from stock.
    pub fn summary(&self) -> String {
        let stamp = if self.short.is_empty() {
            format!(
                "{} · unknown commit (built without a git checkout) · {}",
                self.release(),
                self.profile
            )
        } else {
            format!(
                "{} · {} · {} · {}",
                self.release(),
                self.short,
                self.date,
                self.profile
            )
        };
        if self.extended {
            stamp + " · extended"
        } else {
            stamp
        }
    }
}

/// How many commits past `v{version}` a `--long` describe (`<tag>-<n>-g<hash>`) is, or `None`
/// when the describe is empty, malformed or names another version's tag.
fn commits_past_tag(describe: &str, version: &str) -> Option<u32> {
    let mut parts = describe.rsplitn(3, '-');
    parts.next().filter(|hash| hash.starts_with('g'))?;
    let n: u32 = parts.next()?.parse().ok()?;
    let tag = parts.next()?;
    (tag.strip_prefix('v')? == version).then_some(n)
}

/// Log the build id once at startup, not env-gated, so every pasted log carries it.
pub fn banner(build: Res<BuildId>) {
    info!("benilla build {}", build.summary());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(version: &'static str, describe: &'static str, short: &'static str) -> BuildId {
        BuildId {
            version,
            describe,
            sha: "",
            short,
            date: "2026-09-27",
            profile: "release",
            extended: false,
        }
    }

    #[test]
    fn a_tagged_release_reads_as_its_version() {
        let b = build("0.2.0", "v0.2.0-0-gb17be27", "b17be27");
        assert_eq!(b.summary(), "0.2.0 · b17be27 · 2026-09-27 · release");
    }

    #[test]
    fn a_commit_past_the_release_counts_its_distance() {
        assert_eq!(
            build("0.2.0", "v0.2.0-12-gb17be27", "b17be27").release(),
            "0.2.0+12"
        );
        // A pre-release tag's own hyphens stay in the tag.
        assert_eq!(
            build("1.0.0-rc1", "v1.0.0-rc1-3-gabc1234", "abc1234").release(),
            "1.0.0-rc1+3"
        );
    }

    #[test]
    fn another_versions_tag_or_none_reads_as_the_version_alone() {
        // The release commit itself, before its tag exists: the nearest tag is the previous one.
        assert_eq!(
            build("0.2.0", "v0.1.0-450-gb17be27", "b17be27").release(),
            "0.2.0"
        );
        // No release tag in reach: a source archive or a shallow clone.
        assert_eq!(build("0.2.0", "", "b17be27").release(), "0.2.0");
        assert_eq!(
            build("0.2.0", "not-a-describe", "b17be27").release(),
            "0.2.0"
        );
    }

    #[test]
    fn a_build_without_git_still_names_its_release() {
        let b = build("0.2.0", "", "");
        assert_eq!(
            b.summary(),
            "0.2.0 · unknown commit (built without a git checkout) · release"
        );
    }

    #[test]
    fn an_extended_client_says_so_and_stock_reads_as_before() {
        let stock = build("0.2.0", "v0.2.0-12-gb17be27", "b17be27");
        assert_eq!(stock.summary(), "0.2.0+12 · b17be27 · 2026-09-27 · release");
        assert_eq!(
            BuildId {
                extended: true,
                ..stock
            }
            .summary(),
            "0.2.0+12 · b17be27 · 2026-09-27 · release · extended"
        );
        assert_eq!(
            BuildId {
                extended: true,
                ..build("0.2.0", "", "")
            }
            .summary(),
            "0.2.0 · unknown commit (built without a git checkout) · release · extended"
        );
    }
}
