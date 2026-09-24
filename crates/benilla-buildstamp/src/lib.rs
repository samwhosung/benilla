//! The build stamp: the `build.rs` body the `benilla` and `benilla-worldview` launcher shims
//! share. It stays a build-dependency of the shims, never of `benilla-app`: cargo rebuilds the
//! package that owns a `rerun-if-changed` path whenever that path's mtime moves, and these paths
//! move on every commit, rebase and checkout.

use std::path::Path;
use std::process::Command;

/// Stamp the commit this binary was built from into the calling package as four `rustc-env`
/// vars, which its `main.rs` reads back with `env!` into a `BuildId`:
///
/// - `BENILLA_GIT_SHA`: the full sha.
/// - `BENILLA_GIT_SHORT`: git's own abbreviation of it.
/// - `BENILLA_GIT_DATE`: the commit date (`%cs`, `YYYY-MM-DD`), not the build date.
/// - `BENILLA_PROFILE`: the profile directory's name, which tells `ship` from `release` where
///   cargo's `PROFILE` does not.
///
/// The git vars are empty when git cannot answer (no `.git`, no `git` on `PATH`), which the
/// runtime reports as an unknown build. The rerun triggers are `HEAD` and the ref it names,
/// resolved with `git rev-parse --git-path` so a linked worktree resolves too; only paths that
/// exist are emitted, since cargo reruns a build script whose watched path is missing on every
/// build.
pub fn emit() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (!s.is_empty()).then_some(s)
    };

    // Rerun triggers first, so they are emitted even if a later call fails.
    let watch = |path: Option<String>| {
        if let Some(p) = path.filter(|p| Path::new(p).exists()) {
            println!("cargo::rerun-if-changed={p}");
        }
    };
    watch(git(&["rev-parse", "--git-path", "HEAD"]));
    if let Some(git_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
        // The loose ref, plus `packed-refs`: a fresh clone's branch has no loose file.
        watch(git(&["rev-parse", "--git-path", &git_ref]));
        watch(git(&["rev-parse", "--git-path", "packed-refs"]));
    }
    // An edit to the rule restamps: cargo reruns a build script when `build.rs` or one of its
    // build-dependencies changes.
    println!("cargo::rerun-if-changed=build.rs");

    for (var, args) in [
        ("BENILLA_GIT_SHA", &["rev-parse", "HEAD"][..]),
        ("BENILLA_GIT_SHORT", &["rev-parse", "--short", "HEAD"]),
        ("BENILLA_GIT_DATE", &["log", "-1", "--format=%cs"]),
    ] {
        println!(
            "cargo::rustc-env={var}={}",
            git(args).unwrap_or_default() // empty: an unknown build at runtime
        );
    }

    // `OUT_DIR` is `<target>/[<triple>/]<profile-dir>/build/<pkg>-<hash>/out`; the fallback is
    // cargo's coarse `PROFILE`.
    let out_dir = std::env::var("OUT_DIR").unwrap_or_default();
    let parts: Vec<_> = Path::new(&out_dir).components().collect();
    let profile = parts
        .iter()
        .position(|c| c.as_os_str() == "build")
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| parts[i].as_os_str().to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| std::env::var("PROFILE").unwrap_or_default());
    println!("cargo::rustc-env=BENILLA_PROFILE={profile}");
}
