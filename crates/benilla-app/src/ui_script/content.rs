//! **The shipped UI's files, compiled into the binary**.
//!
//! One question, one answer: *give me the text of `ui/<name>.xml`*. [`load_ui_files`] and the
//! `provider` closure it hands the loader (for `<Include>` / `<Script file=>` references) both ask
//! here instead of touching `std::fs`.
//!
//! Until 1175 they read `concat!(env!("CARGO_MANIFEST_DIR"), "/assets/ui")` — the *build*
//! machine's source tree. On a player's machine every one of those files resolves to nothing and
//! the client boots with no interface at all: not a crash, just an empty screen and one log line
//! per manifest entry that nobody sees.
//! `assets/ui` is our own content (MIT/Apache, not Blizzard's — `docs/METHOD.md`'s hard rule is
//! untouched), so it can simply be part of the program.
//!
//! ## Resolution order — the same in every build
//!
//! 1. **`#[cfg(feature = "dev")]`** — the crate's own `assets/ui/` on disk. Editing a FrameXML file
//!    and re-running still costs no recompile, which is the whole reason the source tree is worth
//!    probing at all. This is the only place a source path survives, and 1174's feature is what
//!    keeps it out of a player binary.
//! 2. **The compiled-in copy** — `include_dir!` over the same directory.
//!
//! A dev build therefore prefers disk and silently falls back to the embedded copy, so a file
//! deleted from `assets/ui` does not *appear* to still work: [`tests`] pins that the two agree.
//!
//! ## Why not the asset server
//!
//! [`super::manifest::MANIFEST`] is a **dependency-ordered** manifest loaded synchronously
//! at a known point (a template must exist before the frame that inherits it), and the asset server
//! is async. Serving these through it would mean re-deriving a dependency graph that a hand-ordered
//! list already encodes correctly. This stays a byte lookup.
//!
//! ## The override layer this is not
//!
//! A `content/` folder beside the executable — addon and content-pack territory — is deliberately
//! *not* built here (1175 "Rejected"), but the shape leaves room for it: it would be one more arm
//! at the front of [`read`]. Building it now would be a mod system with no mods.

/// The shipped UI tree, compiled in: `benilla.toc` and every file it names, a few MB — a small
/// single-digit % of the release binary, and what it buys is "the interface cannot be missing".
/// (A hard file count lived here until it had silently drifted by four; the manifest is the
/// authority on how many there are, and `manifest::tests` is what keeps the two agreeing.)
static UI: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/ui");

/// The text of one shipped UI file, by path relative to `assets/ui` — `None` if we do not ship it.
///
/// `req` may arrive in Blizzard's backslash form and may name a directory it does not have; the
/// basename fallback is `super::load_ui_files`'s and lives at the call site, not here, because
/// it is a property of FrameXML references rather than of the content store.
pub(super) fn read(req: &str) -> Option<String> {
    // 1 · the source tree, dev builds only — so editing a FrameXML file needs no recompile.
    if let Some(text) = read_source_tree(req) {
        return Some(text);
    }
    // 2 · the compiled-in copy.
    UI.get_file(req)?.contents_utf8().map(str::to_owned)
}

/// `assets/ui/<req>` on disk, or `None` in a player build.
///
/// The source directory comes from [`crate::run_mode::dev_source_dir`] rather than a `cfg` here:
/// the seam has one door per kind (1179), and a path literal must be *absent* from a player
/// binary, not merely unreached (1175). With no source dir this is a cheap `None` and the caller
/// falls through to the compiled-in copy.
fn read_source_tree(req: &str) -> Option<String> {
    let dir = crate::run_mode::dev_source_dir()?.join("assets/ui");
    std::fs::read_to_string(dir.join(req)).ok()
}

/// A digest of the FrameXML **this process will actually load** — the manifest plus every file it
/// names, hashed in load order, as eight hex digits.
///
/// It exists because a number taken from the corpus harness is meaningless without it. In a dev
/// build [`read`] prefers the SOURCE TREE, so editing an `assets/ui` file changes what a survey
/// loads **with no rebuild** — and a tree is routinely edited by several agents at
/// once. Three separate measurements this arc were taken across a moving tree and one of them
/// landed a wrong attribution in a decision record: 87/218 was credited to a single table when a
/// controlled A/B later put it at 75, with the other twelve belonging to a neighbour's uncommitted
/// files. The stamp does not prevent that; it makes it visible, which is all an instrument can do.
///
/// FNV-1a, no dependency, not cryptographic and not meant to be: the only question it answers is
/// *were these two runs looking at the same interface*.
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
        // A chain entry (1751) contributes its NAME only: its bytes are the player's install,
        // which does not move between two runs on one machine — and this stamp's only question is
        // *were these two runs looking at the same interface*. Migrating a window still changes the
        // digest, because the manifest line changes.
        eat(name.as_bytes());
        if let Some(text) = read(&name) {
            eat(text.as_bytes());
        }
    }
    format!("{:08x}", h as u32)
}

/// Every shipped file's path, relative to `assets/ui`. The compiled-in set is the authority —
/// a dev build must not appear to ship a file that is only on this machine.
#[cfg(test)]
pub(super) fn shipped_files() -> impl Iterator<Item = &'static str> {
    UI.files().filter_map(|f| f.path().to_str())
}

#[cfg(test)]
mod tests {
    /// Every entry of the manifest the client actually loads is a file we actually ship.
    ///
    /// This is the check the old `std::fs` loader could not make: a typo'd manifest entry used to
    /// be one `error!` line in a log at boot, and capture runs skip the loader entirely unless
    /// `WOW_CAPTURE_UI=1`, so nothing failed. Now a name that is not in the binary fails here.
    #[test]
    fn every_manifest_entry_is_compiled_in() {
        for name in crate::ui_script::manifest::shipped_manifest_files() {
            assert!(
                super::read(&name).is_some(),
                "benilla.toc names {name}, which is not in assets/ui"
            );
        }
    }

    /// The dev probe and the compiled-in copy describe the same tree.
    ///
    /// A dev build prefers disk, so without this a file deleted from `assets/ui` would keep
    /// working from the embedded copy (and a file added there would keep working from disk) until
    /// someone built a release. Either way the two would silently disagree about what benilla
    /// ships, which is the failure mode 1175 is about.
    #[test]
    fn the_source_tree_and_the_compiled_in_copy_agree() {
        // A player build has no source dir to disagree with — that IS the property there.
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
