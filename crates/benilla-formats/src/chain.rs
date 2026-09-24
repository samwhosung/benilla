//! The vanilla patch chain — a priority-ordered set of MPQ archives, read through `benilla-mpq`.
//!
//! Replaces `wow-mpq`'s `PatchChain` *and* the old `ChainReader`. Those were two types
//! because `wow-mpq`'s `Archive::open` re-parsed the hash/block (and the useless `(attributes)`) tables
//! on every open, so `ChainReader` bolted a `Mutex<HashMap<…, Archive>>` handle-cache on top to avoid
//! re-paying that per read. `benilla_mpq::Archive` now caches its parsed tables in an `Arc` and reads
//! `&self` (a fresh OS handle per read, no seek-state sharing), so the cache is gone and one `Chain`
//! serves both the `&self` concurrent Bevy `AssetReader` path and the `&mut` streaming-loader path.
//!
//! Later archives override earlier ones for files sharing an internal path (so a patch archive
//! wins); a read resolves a name to the highest-priority archive that holds it. Base content
//! archives carry no `(listfile)`, so resolution is by name **hash**, which works without one.
//! Which archives mount, and in what order, is [`mount_order`]'s law.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use benilla_mpq::Archive;

use crate::VANILLA_BASE_ORDER;

/// One entry from a chain listing: an internal path and its uncompressed size.
pub struct ChainEntry {
    pub name: String,
    pub size: u64,
}

/// A priority-ordered patch chain of MPQ archives (`Send + Sync`; reads are `&self` and lock-free).
pub struct Chain {
    /// Ascending priority — later archives win. `resolve` scans back-to-front.
    archives: Vec<Archive>,
}

/// `patch-?.MPQ` with the reference's FindFirstFileW semantics: `?` matches **exactly one**
/// character, case-insensitively — `patch-3.MPQ` mounts, `patch-10.MPQ` does not (the glob
/// template `0x82edbc` and its wrapper `0x42ad10`).
fn is_patch_glob_match(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some(mid) = lower
        .strip_prefix("patch-")
        .and_then(|rest| rest.strip_suffix(".mpq"))
    else {
        return false;
    };
    mid.chars().count() == 1
}

/// The vanilla mount law over a `Data` directory listing, **ascending priority** (
/// the mounter `0x403740`): the ten
/// [`VANILLA_BASE_ORDER`] archives at their fixed priorities, then `patch.MPQ`, then every
/// `patch-?.MPQ` sorted ascending by case-folded name — the binary sorts its glob matches
/// *descending* (`strnicmp`) and walks the array backwards, so the order is deterministic, never
/// filesystem enumeration; `patch-3` overrides `patch-2` — then `speech2.MPQ` above every patch.
/// Names are matched case-insensitively (the reference runs on a case-insensitive filesystem) and
/// returned as found on disk; absent archives are simply not in the result.
fn mount_order(dir_names: &[String]) -> Vec<String> {
    let find = |want: &str| {
        dir_names
            .iter()
            .find(|n| n.eq_ignore_ascii_case(want))
            .cloned()
    };
    let mut order: Vec<String> = VANILLA_BASE_ORDER.iter().filter_map(|b| find(b)).collect();
    order.extend(find("patch.MPQ"));
    let mut patches: Vec<String> = dir_names
        .iter()
        .filter(|n| is_patch_glob_match(n))
        .cloned()
        .collect();
    patches.sort_by_key(|n| n.to_ascii_lowercase());
    order.extend(patches);
    order.extend(find("speech2.MPQ"));
    order
}

impl Chain {
    /// Open a chain from a vanilla `Data` directory (every archive [`mount_order`] finds in it,
    /// lowest priority first) or a single `.MPQ` file (just that archive).
    ///
    /// An archive that exists but fails to open is a hard error, deliberately: the reference logs
    /// `"Failed to open archive"` and continues, but a silent skip turns a corrupt `dbc.MPQ` — or a
    /// modder's malformed `patch-3.MPQ` — into cryptic missing-file failures far downstream. Same
    /// composite on a healthy install; a clear error instead of a quirk on a broken one (1300).
    pub fn open(path: &Path) -> Result<Self> {
        let mut archives = Vec::new();
        if path.is_dir() {
            let mut names: Vec<String> = std::fs::read_dir(path)
                .with_context(|| format!("listing {}", path.display()))?
                .filter_map(|entry| {
                    let entry = entry.ok()?;
                    // `path().is_file()` follows symlinks (`read_dir`'s file_type doesn't).
                    entry.path().is_file().then(|| entry.file_name())
                })
                .filter_map(|name| name.into_string().ok())
                .collect();
            // read_dir order is arbitrary; sort so case-variant ties resolve deterministically.
            names.sort();
            for name in mount_order(&names) {
                let mpq = path.join(&name);
                archives.push(
                    Archive::open(&mpq).with_context(|| format!("opening {}", mpq.display()))?,
                );
            }
            if archives.is_empty() {
                bail!("no known vanilla MPQs found in {}", path.display());
            }
        } else {
            archives.push(
                Archive::open(path).with_context(|| format!("opening MPQ {}", path.display()))?,
            );
        }
        Ok(Self { archives })
    }

    /// The highest-priority archive with an *entry* for `name` (readable file **or** delete-marker),
    /// if any. Stops at the winning archive — including a tombstone, which correctly shadows any
    /// lower-priority copy. Callers that want "readable" must check
    /// [`Archive::is_delete_marker`].
    fn resolve(&self, name: &str) -> Option<&Archive> {
        self.archives.iter().rev().find(|a| a.contains(name))
    }

    /// Whether the chain holds `name` as a **readable** file (accepts `/` or `\`; case-insensitive).
    /// A path whose winning entry is a delete-marker is *not* present — the client deleted it.
    pub fn contains(&self, name: &str) -> bool {
        self.resolve(name)
            .is_some_and(|a| !a.is_delete_marker(name))
    }

    /// The path of the archive `name` resolves to (the winning override) — for debugging / extract.
    pub fn find_file_archive(&self, name: &str) -> Option<&Path> {
        self.resolve(name).map(|a| a.path())
    }

    /// Read a file by internal path (accepts `/` or `\`), from its winning archive. `&self`: safe to
    /// call concurrently (the Bevy `AssetReader` does).
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let archive = self
            .resolve(name)
            .ok_or_else(|| anyhow!("file not in patch chain: {name}"))?;
        // A tombstone shadows every lower copy: the path is deleted from the composite, so this is a
        // clean "not found", not a fall-through to a stale base version.
        if archive.is_delete_marker(name) {
            bail!(
                "file deleted from patch chain: {name} (tombstoned by {})",
                archive.path().display()
            );
        }
        archive
            .read_file(name)
            .with_context(|| format!("reading {name} from {}", archive.path().display()))
    }

    /// `&mut` alias of [`Chain::read`] — kept so the streaming-loader call sites that thread a
    /// `&mut Chain` read exactly as they did against `wow-mpq`'s `PatchChain`.
    pub fn read_file(&mut self, name: &str) -> Result<Vec<u8>> {
        self.read(name)
    }

    /// List the chain's named files with sizes. Dev/extract use only — files absent from every
    /// listfile (most of `texture.MPQ`) are reachable by name but not enumerated.
    ///
    /// Unions the `(listfile)` of **every** archive that carries one: each archive's listfile names
    /// only the files *it* holds, so resolving `(listfile)` like an ordinary overridden file (as this
    /// used to) returns just the top patch archive's sliver — 92 names from `patch-2.MPQ` instead of
    /// the chain's ~86k. Sizes still resolve per-name to the winning archive.
    pub fn list(&self) -> Result<Vec<ChainEntry>> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for archive in &self.archives {
            let Ok(listfile) = archive.read_file("(listfile)") else {
                continue;
            };
            for raw in String::from_utf8_lossy(&listfile).split([';', '\r', '\n']) {
                let name = raw.trim();
                // Dedupe across archives the way MPQ hashing compares names: case-insensitive, `/`≡`\`.
                if name.is_empty() || !seen.insert(name.replace('/', "\\").to_ascii_lowercase()) {
                    continue;
                }
                if let Some(a) = self.resolve(name) {
                    // A tombstoned path isn't a file in the composite — don't list it.
                    if a.is_delete_marker(name) {
                        continue;
                    }
                    out.push(ChainEntry {
                        name: name.to_string(),
                        size: a.file_size(name).unwrap_or(0) as u64,
                    });
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn patch_glob_matches_exactly_one_character_case_insensitively() {
        assert!(is_patch_glob_match("patch-2.MPQ"));
        assert!(is_patch_glob_match("patch-3.MPQ"));
        assert!(is_patch_glob_match("PATCH-A.mpq"));
        // Zero or two-plus characters: FindFirstFileW's `?` is exactly one.
        assert!(!is_patch_glob_match("patch-.MPQ"));
        assert!(!is_patch_glob_match("patch-10.MPQ"));
        assert!(!is_patch_glob_match("patch-33.MPQ"));
        // Not the glob's shape at all.
        assert!(!is_patch_glob_match("patch.MPQ"));
        assert!(!is_patch_glob_match("patch-2.MPQ.bak"));
        assert!(!is_patch_glob_match("mypatch-2.MPQ"));
    }

    #[test]
    fn mount_order_is_the_carved_law() {
        // A shuffled install with a custom patch, plus files the mounter must ignore:
        // base.MPQ (telemetry-only in the reference), backup.MPQ, loose non-archives.
        let dir = owned(&[
            "patch-2.MPQ",
            "backup.MPQ",
            "model.MPQ",
            "base.MPQ",
            "dbc.MPQ",
            "patch.MPQ",
            "eula.html",
            "patch-3.MPQ",
            "speech2.MPQ",
            "texture.MPQ",
        ]);
        assert_eq!(
            mount_order(&dir),
            owned(&[
                "dbc.MPQ",
                "texture.MPQ",
                "model.MPQ",
                "patch.MPQ",
                "patch-2.MPQ",
                "patch-3.MPQ",
                "speech2.MPQ",
            ])
        );
    }

    #[test]
    fn patch_sort_is_ascending_and_case_folded() {
        // Later wins in `Chain`, so ascending case-folded order makes patch-3 override patch-2
        // and `patch-B` override `patch-a` ('a' < 'b' after the strnicmp-style fold).
        let dir = owned(&["patch-B.MPQ", "patch-3.MPQ", "patch-a.MPQ", "patch-2.MPQ"]);
        assert_eq!(
            mount_order(&dir),
            owned(&["patch-2.MPQ", "patch-3.MPQ", "patch-a.MPQ", "patch-B.MPQ"])
        );
    }

    #[test]
    fn base_archives_are_found_case_insensitively() {
        let dir = owned(&["DBC.mpq", "Model.MPQ", "PATCH.mpq"]);
        assert_eq!(
            mount_order(&dir),
            owned(&["DBC.mpq", "Model.MPQ", "PATCH.mpq"])
        );
    }
}
