//! The launcher `examples/extended_launcher.rs` documents for a crate on top of benilla stays in
//! step with the workspace: cargo reads `[patch]` and `[profile]` from the root manifest only, so
//! the header repeats the root's, a missing patch builds a client that is silently not benilla's,
//! and a bevy other than the workspace's is a second bevy. The rest mirrors the `benilla` launcher.

use std::path::Path;

/// A manifest by its path from this crate's root.
fn manifest(rel: &str) -> toml::Table {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .parse()
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The example header's ```toml block, parsed.
fn documented() -> toml::Table {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/extended_launcher.rs");
    let text = std::fs::read_to_string(&path).unwrap();
    let block: Vec<&str> = text
        .lines()
        .map(|l| {
            l.strip_prefix("//!")
                .map(|l| l.strip_prefix(' ').unwrap_or(l))
        })
        .skip_while(|l| *l != Some("```toml"))
        .skip(1)
        .take_while(|l| *l != Some("```"))
        .map(|l| l.expect("the ```toml block ends inside the header"))
        .collect();
    assert!(!block.is_empty(), "no ```toml block in {}", path.display());
    block
        .join("\n")
        .parse()
        .expect("the documented manifest parses")
}

/// `t[k1][k2]…`, or a panic naming the path.
fn at<'a>(t: &'a toml::Table, keys: &[&str]) -> &'a toml::Value {
    let (first, rest) = keys.split_first().unwrap();
    let mut v = t.get(*first).unwrap_or_else(|| panic!("no `{first}`"));
    for k in rest {
        v = v
            .get(*k)
            .unwrap_or_else(|| panic!("no `{}`", keys.join(".")));
    }
    v
}

#[test]
fn the_documented_launcher_repeats_the_root_patches_profile_and_bevy() {
    let doc = documented();
    let root = manifest("../../Cargo.toml");
    let patched = |t: &toml::Table| -> Vec<String> {
        let table = at(t, &["patch", "crates-io"]).as_table().unwrap();
        table.keys().cloned().collect()
    };
    assert_eq!(patched(&doc), patched(&root), "[patch.crates-io]");
    for keys in [
        &["profile", "dev", "opt-level"][..],
        &["profile", "dev", "package", "*", "opt-level"],
    ] {
        assert_eq!(at(&doc, keys), at(&root, keys), "{}", keys.join("."));
    }
    assert_eq!(
        at(&doc, &["dependencies", "bevy", "version"]),
        at(&root, &["workspace", "dependencies", "bevy", "version"]),
        "bevy's version"
    );
    assert_eq!(
        at(&doc, &["dependencies", "bevy", "default-features"]).as_bool(),
        Some(false),
        "bevy adds no features"
    );
}

#[test]
fn the_documented_launcher_takes_benilla_app_as_the_benilla_launcher_does() {
    let doc = documented();
    let shim = manifest("../benilla/Cargo.toml");
    for keys in [
        &["features", "dev"][..],
        &["dependencies", "benilla-app", "default-features"],
    ] {
        assert_eq!(at(&doc, keys), at(&shim, keys), "{}", keys.join("."));
    }
    assert!(
        at(&doc, &["build-dependencies"])
            .get("benilla-buildstamp")
            .is_some(),
        "the build script's stamp"
    );
}
