//! The **build-id stamp emitter** — the `build.rs` body both launcher shims share.
//!
//! Two binaries carry the stamp now (`benilla` and `benilla-worldview`), and a
//! copy-pasted build script is exactly the thing that rots: one shim gets a fix, the other keeps
//! reporting a sha by a rule that changed. So the script is a function and each shim's `build.rs`
//! is one line that calls it.
//!
//! It stays a **build-dependency of the shims**, never of `benilla-app`: cargo dirties the compile
//! units of the package that owns a `rerun-if-changed` path on the file's mtime alone, and those
//! paths move on every commit, rebase and checkout. In a ~30-line shim that costs the shim's
//! recompile plus one relink; in the app crate it cost ~2 minutes of pure stamp tax per commit
//! across the gates, which is what decision 0993 measured and moved out.

use std::path::Path;
use std::process::Command;

/// Stamp the **commit this binary was built from** into the calling package: its `main.rs`
/// reads the four vars back with `env!` and hands them to `benilla_app`'s entry point as the
/// `BuildId` resource (`benilla-app/src/build_id.rs` — the runtime side and its two surfaces).
///
/// Why it exists: people run benilla from a clone of the public snapshot repo, and a report ("this
/// looks wrong", "it crashed here") is only actionable if we know *which* code they ran. A git sha
/// is the only id that can't drift — the crate version is a permanent `0.1.0` and there are no
/// releases to number. The stamp is the commit the binary was built from, on any checkout.
///
/// It emits four `rustc-env` vars. The three git ones come back **empty** when git can't answer (a
/// source zip with no `.git`, or no `git` on PATH), which the runtime side reports as an unknown
/// build rather than failing to compile:
///
/// - `BENILLA_GIT_SHA` — the full 40-char sha (what the panel copies to the clipboard).
/// - `BENILLA_GIT_SHORT` — git's own abbreviation, so the string matches the `pub/<sha>` tag names
///   (`pubsync` abbreviates the same way, in the same repo).
/// - `BENILLA_GIT_DATE` — the commit date (`%cs`, `YYYY-MM-DD`). Committer date, not build date:
///   it is a property of the sha, so it can never disagree with it.
///
/// …plus `BENILLA_PROFILE`, the profile directory name — `debug` / `release` / `ship` (0736's
/// shipping profile). Cargo's own `PROFILE` var collapses every release-like profile to `release`,
/// so `ship` and `release` would be indistinguishable; the profile *directory* inside `OUT_DIR`
/// carries the real name, which is what a "why is this slow" report needs to say.
///
/// ## Staleness — the only thing that could make this lie
///
/// A stamp is worthless if it can report a commit the binary isn't. The rerun triggers below are
/// therefore exactly the files that change when HEAD moves: `HEAD` itself (checkout, rebase,
/// detach) and the ref it names (a commit rewrites `refs/heads/<branch>`, not `HEAD`). Both are
/// resolved with `git rev-parse --git-path`, which applies the worktree rules — in a linked
/// worktree `.git` is a *file*, `HEAD` lives in `<main>/.git/worktrees/<name>/`, and `refs/heads/*`
/// stay in the common dir. Only paths that exist are emitted: cargo reruns a build script whose
/// watched path is *missing* on every build, and every one of those reruns relinks the app crate.
///
/// What the stamp deliberately does **not** claim is that the working tree was clean: it names the
/// commit the build came *from*. A dirty flag can only be as fresh as the last build-script run, so
/// in the normal dev loop (edit, run, don't stage) it would read "clean" while the binary carried
/// uncommitted work — a flag that is wrong exactly when it matters. "Which snapshot" is the
/// question a remote report needs answered, and a bare sha answers it without lying.
pub fn emit() {
    // Build scripts run with the package root as cwd, but be explicit: `-C` makes the git calls
    // independent of that guarantee.
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
        // The loose ref file, plus `packed-refs` for the freshly-cloned/gc'd repo where the branch
        // has no loose file at all (a public clone is exactly that).
        watch(git(&["rev-parse", "--git-path", &git_ref]));
        watch(git(&["rev-parse", "--git-path", "packed-refs"]));
    }
    // A stamp that never changes when the rule is edited would be its own kind of stale. The
    // shim's `build.rs` is watched by cargo automatically; this file is watched because cargo
    // rebuilds a build script when its build-dependency changes.
    println!("cargo::rerun-if-changed=build.rs");

    for (var, args) in [
        ("BENILLA_GIT_SHA", &["rev-parse", "HEAD"][..]),
        ("BENILLA_GIT_SHORT", &["rev-parse", "--short", "HEAD"]),
        ("BENILLA_GIT_DATE", &["log", "-1", "--format=%cs"]),
    ] {
        println!(
            "cargo::rustc-env={var}={}",
            git(args).unwrap_or_default() // empty ⇒ "unknown build" at runtime
        );
    }

    // `OUT_DIR` is `<target>/[<triple>/]<profile-dir>/build/<pkg>-<hash>/out` — the component
    // before `build` is the profile directory. Falls back to cargo's coarse `PROFILE`.
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
