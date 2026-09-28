//! The build stamp: the `build.rs` body the `benilla` and `benilla-worldview` launcher shims
//! share. It stays a build-dependency of the shims, never of `benilla-app`: cargo rebuilds the
//! package that owns a `rerun-if-changed` path whenever that path's mtime moves, and these paths
//! move on every commit, rebase and checkout.

use std::path::Path;
use std::process::Command;

/// Stamp the commit this binary was built from into the calling package as six `rustc-env`
/// vars, which its `main.rs` reads back with `env!` into a `BuildId`, beside its own
/// `CARGO_PKG_VERSION`:
///
/// - `BENILLA_GIT_SHA`: the full sha.
/// - `BENILLA_GIT_SHORT`: git's own abbreviation of it.
/// - `BENILLA_GIT_DATE`: the commit date (`%cs`, `YYYY-MM-DD`), not the build date.
/// - `BENILLA_GIT_DESCRIBE`: `git describe --tags --long` against the nearest release tag
///   (`v0.2.0-12-gb17be27`), which says how far past its release the commit is.
/// - `BENILLA_PROFILE`: the profile directory's name, which tells `ship` from `release` where
///   cargo's `PROFILE` does not.
/// - `BENILLA_PROJECT_DIR`: the folder a dev build keeps its `WoW` link, `benilla-config/` and
///   `.probe-identity` in, the calling package's workspace root ([`workspace_root`]); empty unless
///   the package's own `dev` feature is on, so a player binary carries no source path.
///
/// The git vars are empty when git cannot answer (no `.git`, no `git` on `PATH`, no tag in
/// reach), which the runtime reports as an unknown commit or the bare version. The rerun
/// triggers are `HEAD`, the ref it names, `packed-refs` and the tags, resolved with
/// `git rev-parse --git-path` so a linked worktree resolves too; only paths that exist are
/// emitted, since cargo reruns a build script whose watched path is missing on every build.
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
        watch(git(&["rev-parse", "--git-path", &git_ref]));
    }
    // A fresh clone's refs live only in `packed-refs`; a fetched or new tag lands in either, and
    // moves the describe without moving `HEAD`. Cargo scans a watched directory whole.
    watch(git(&["rev-parse", "--git-path", "packed-refs"]));
    watch(git(&["rev-parse", "--git-path", "refs/tags"]));
    // An edit to the rule restamps: cargo reruns a build script when `build.rs` or one of its
    // build-dependencies changes.
    println!("cargo::rerun-if-changed=build.rs");

    for (var, args) in [
        ("BENILLA_GIT_SHA", &["rev-parse", "HEAD"][..]),
        ("BENILLA_GIT_SHORT", &["rev-parse", "--short", "HEAD"]),
        ("BENILLA_GIT_DATE", &["log", "-1", "--format=%cs"]),
        (
            "BENILLA_GIT_DESCRIBE",
            &["describe", "--tags", "--long", "--match", "v[0-9]*"],
        ),
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

    let project = if std::env::var_os("CARGO_FEATURE_DEV").is_some() {
        workspace_root(Path::new(&dir)).display().to_string()
    } else {
        String::new()
    };
    println!("cargo::rustc-env=BENILLA_PROJECT_DIR={project}");
}

/// The workspace root of the package at `manifest_dir`: the nearest folder up from it holding
/// `Cargo.lock`, which cargo writes at the root before any build script runs, else the package's
/// own folder. For a launcher inside benilla's checkout that is the checkout.
fn workspace_root(manifest_dir: &Path) -> &Path {
    manifest_dir
        .ancestors()
        .find(|d| d.join("Cargo.lock").is_file())
        .unwrap_or(manifest_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workspace_root_is_the_nearest_folder_holding_the_lockfile() {
        let tmp = std::env::temp_dir().join(format!("benilla-stamp-{}", std::process::id()));
        let member = tmp.join("ws/crates/launcher");
        std::fs::create_dir_all(&member).unwrap();
        // A lockfile further up belongs to some other tree and must not win over the nearer one.
        std::fs::write(tmp.join("Cargo.lock"), "").unwrap();
        std::fs::write(tmp.join("ws/Cargo.lock"), "").unwrap();
        assert_eq!(workspace_root(&member), tmp.join("ws"));
        // A package that is its own workspace keeps its lockfile beside its manifest.
        std::fs::write(member.join("Cargo.lock"), "").unwrap();
        assert_eq!(workspace_root(&member), member);
        std::fs::remove_dir_all(&tmp).ok();

        let bare = std::env::temp_dir().join(format!("benilla-stamp-bare-{}", std::process::id()));
        std::fs::create_dir_all(&bare).unwrap();
        let root = workspace_root(&bare);
        assert!(
            root == bare || root.join("Cargo.lock").is_file(),
            "no lockfile anywhere up means the package's own folder: {}",
            root.display()
        );
        std::fs::remove_dir_all(&bare).ok();
    }

    /// benilla's own launchers stamp the checkout, the folder a dev build always used.
    #[test]
    fn a_launcher_in_this_checkout_stamps_the_checkout() {
        let shim = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("benilla");
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap();
        assert_eq!(workspace_root(&shim), checkout);
    }
}
