//! The shipped UI's files (`assets/ui`), compiled into the binary so the interface cannot be
//! missing; `load_ui_files` and its `<Include>` provider read them here, never through `std::fs`.
//! A dev build reads the source tree first, so an edit needs no recompile.

/// The shipped UI tree: `benilla.toc` and every file it names.
static UI: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/ui");

/// The text of one shipped UI file, by path relative to `assets/ui`; the basename fallback for a
/// FrameXML reference is the caller's.
pub(super) fn read(req: &str) -> Option<String> {
    if let Some(text) = read_source_tree(req) {
        return Some(text);
    }
    UI.get_file(req)?.contents_utf8().map(str::to_owned)
}

/// `assets/ui/<req>` on disk, or `None` in a player build, whose binary holds no source path.
fn read_source_tree(req: &str) -> Option<String> {
    let dir = crate::run_mode::dev_source_dir()?.join("assets/ui");
    std::fs::read_to_string(dir.join(req)).ok()
}

/// FNV-1a (not cryptographic) over the manifest and each file it names, in load order, as eight
/// hex digits: a stamp that tells two runs whether they loaded the same interface.
pub(crate) fn digest() -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
    };
    eat(super::manifest::MANIFEST.as_bytes());
    if let Some(toc) = read(super::manifest::MANIFEST) {
        eat(toc.as_bytes());
    }
    for name in super::addons::Addon::builtin().toc.files {
        // Only a chain entry's name counts: its bytes are the player's install.
        eat(name.as_bytes());
        if let Some(text) = read(&name) {
            eat(text.as_bytes());
        }
    }
    format!("{:08x}", h as u32)
}

/// Every shipped file's path, relative to `assets/ui`, from the compiled-in set, not the disk.
#[cfg(test)]
pub(super) fn shipped_files() -> impl Iterator<Item = &'static str> {
    UI.files().filter_map(|f| f.path().to_str())
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_manifest_entry_is_compiled_in() {
        for name in crate::ui_script::manifest::shipped_manifest_files() {
            assert!(
                super::read(&name).is_some(),
                "benilla.toc names {name}, which is not in assets/ui"
            );
        }
    }

    /// A dev build prefers disk, so without this the two copies could disagree unnoticed.
    #[test]
    fn the_source_tree_and_the_compiled_in_copy_agree() {
        // A player build has no source tree to disagree with.
        let Some(dir) = crate::run_mode::dev_source_dir().map(|d| d.join("assets/ui")) else {
            return;
        };
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let mut embedded: Vec<String> = super::shipped_files().map(str::to_owned).collect();
        on_disk.sort();
        embedded.sort();
        assert_eq!(
            on_disk, embedded,
            "assets/ui on disk and the compiled-in copy disagree — rebuild, or one of them is stale"
        );
    }
}
