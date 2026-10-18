//! The vanilla patch chain: a priority-ordered set of MPQ archives, read through `benilla-mpq`. A
//! read resolves a name to the highest-priority archive holding it, so a patch wins; base archives
//! carry no `(listfile)`, so resolution is by name hash.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use benilla_mpq::Archive;

use crate::{Locale, VANILLA_BASE_ORDER};

/// One entry from a chain listing: an internal path and its uncompressed size.
pub struct ChainEntry {
    pub name: String,
    pub size: u64,
}

/// A priority-ordered patch chain of MPQ archives (`Send + Sync`; reads are `&self` and lock-free).
pub struct Chain {
    /// Ascending priority: later archives win.
    archives: Vec<Archive>,
    /// The locale the chain's `Wow.ini` names, read once at open as start-up reads it once.
    locale: Locale,
}

/// `patch-?.MPQ` as the reference's `FindFirstFileW` glob matches it (template `0x82edbc`, wrapper
/// `0x42ad10`): `?` is exactly one character, any case, so `patch-10.MPQ` never mounts.
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

/// The reference's mount order over a `Data` listing, ascending priority (`0x403740`): the ten
/// [`VANILLA_BASE_ORDER`] archives, `patch.MPQ`, every `patch-?.MPQ` by case-folded name (the
/// reference sorts descending with `strnicmp` and walks backwards), then `speech2.MPQ`. Names
/// match case-insensitively and come back as found on disk.
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
    /// Open a `Data` directory's archives in [`mount_order`], or a single `.MPQ` file.
    ///
    /// Deviation: an archive that fails to open is an error, where the reference logs
    /// `"Failed to open archive"` and goes on, because a skipped corrupt archive surfaces only as
    /// missing files far downstream.
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
        let mut chain = Self {
            archives,
            locale: Locale::default(),
        };
        chain.locale = crate::wow_ini::client_locale(&chain);
        Ok(chain)
    }

    /// The client locale, from the chain's `Wow.ini` (`0x402350`): its index picks the column of
    /// every localized DBC string.
    pub fn locale(&self) -> Locale {
        self.locale
    }

    /// The same archives read as another locale, for a test of the column a loader reads.
    #[cfg(test)]
    pub(crate) fn with_locale(mut self, locale: Locale) -> Self {
        self.locale = locale;
        self
    }

    /// The highest-priority archive with an entry for `name`, a delete marker included, as a
    /// tombstone shadows every lower copy: check [`Archive::is_delete_marker`] for a readable file.
    fn resolve(&self, name: &str) -> Option<&Archive> {
        self.archives.iter().rev().find(|a| a.contains(name))
    }

    /// Whether `name` (`/` or `\`, any case) is a readable file, not a delete marker.
    pub fn contains(&self, name: &str) -> bool {
        self.resolve(name)
            .is_some_and(|a| !a.is_delete_marker(name))
    }

    /// The path of the archive `name` resolves to, for debugging and extraction.
    pub fn find_file_archive(&self, name: &str) -> Option<&Path> {
        self.resolve(name).map(|a| a.path())
    }

    /// Read a file by internal path (`/` or `\`) from its winning archive.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let archive = self.archive_for(name)?;
        archive
            .read_file(name)
            .with_context(|| format!("reading {name} from {}", archive.path().display()))
    }

    /// The archive `name` reads from, as a cheap handle: a caller holding the chain behind a lock
    /// looks the file up under it and reads (`Archive::open_file`) after releasing it. No I/O.
    pub fn archive_for(&self, name: &str) -> Result<Archive> {
        let archive = self
            .resolve(name)
            .ok_or_else(|| anyhow!("file not in patch chain: {name}"))?;
        // A tombstone deletes the path from the composite: not found, never a stale lower copy.
        if archive.is_delete_marker(name) {
            bail!(
                "file deleted from patch chain: {name} (tombstoned by {})",
                archive.path().display()
            );
        }
        Ok(archive.clone())
    }

    /// `&mut` alias of [`Chain::read`] for call sites that thread a `&mut Chain`.
    pub fn read_file(&mut self, name: &str) -> Result<Vec<u8>> {
        self.read(name)
    }

    /// The chain's named files with sizes, for development and extraction; a file in no listfile
    /// (most of `texture.MPQ`) is readable by name but not listed. Unions every archive's
    /// `(listfile)`, as each names only its own files; sizes come from the winning archive.
    pub fn list(&self) -> Result<Vec<ChainEntry>> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for archive in &self.archives {
            let Ok(listfile) = archive.read_file("(listfile)") else {
                continue;
            };
            for raw in String::from_utf8_lossy(&listfile).split([';', '\r', '\n']) {
                let name = raw.trim();
                // Dedupe the way MPQ hashing compares names: any case, `/` and `\` alike.
                if name.is_empty() || !seen.insert(name.replace('/', "\\").to_ascii_lowercase()) {
                    continue;
                }
                if let Some(a) = self.resolve(name) {
                    // A tombstoned path is not a file in the composite.
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

    /// `EnumerateArchiveFiles` (`0x401470`): every `(listfile)` line that begins with `prefix`,
    /// without ASCII case and with `\` only as `\` (`SStrCmpI` at `0x4014fe`), as written and
    /// unresolved. The archives are the mount's slots below the base ones, the patches and
    /// `speech2.MPQ` in their opening order (`0x4014a0`-`0x4014bf`, up to `[0x882740]`), then
    /// `interface.MPQ`, base slot 6 (`0x4014c1`-`0x4014e1`); a slot with no archive is passed over.
    ///
    /// Not walked, as benilla mounts neither: the `speech2` child record each patch gets beside a
    /// real `speech2.MPQ` (`0x648e80` at `0x403886`), which the reference's slots also hold and
    /// which names its parent's `speech2\`-prefixed files, and an SZip archive, `0x648fb0`'s
    /// type-1 arm (`0x668b30`; `0x648e38` types any name not ending `.mpq`). A child's type word is
    /// never written (`0x648e85`-`0x648ea0`, `SMemAlloc` without the zero flag), so which arm
    /// reads it is not fixed by the bytes.
    pub fn enumerate_archive_files(&self, prefix: &str) -> Vec<String> {
        let named = |a: &&Archive, name: &str| {
            a.path()
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        };
        let base = |a: &&Archive| VANILLA_BASE_ORDER.iter().any(|b| named(a, b));
        let interface = self.archives.iter().find(|a| named(a, "interface.MPQ"));
        let mut out = Vec::new();
        for archive in self.archives.iter().filter(|a| !base(a)).chain(interface) {
            if let Ok(listfile) = archive.read_file("(listfile)") {
                listfile_lines(&listfile, prefix, &mut out);
            }
        }
        out
    }
}

/// The longest line `0x648fb0` keeps: its buffer takes bytes while below `[ebp-5]` of `[ebp-0x108]`
/// (`0x64902a`-`0x649031`), and drops the rest of a longer one.
const LISTFILE_LINE_MAX: usize = 0x103;

/// `EnumerateArchiveListfile` (`0x648fb0`) over one listfile, with `0x4014f0`'s prefix test: a line
/// ends at a CR or LF (`0x649022`, `0x649026`); an empty one, and one whose text from its last dot
/// is `.md5` without case (`0x865bd8`, `0x64905f`), is passed over. Deviation: a last line with no
/// CR or LF after it is passed too, where `0x648fb0` ends at the buffer's end without a flush
/// (`0x64907f`) and drops it, a bug that loses a listfile's last name when it lacks a newline.
fn listfile_lines(listfile: &[u8], prefix: &str, out: &mut Vec<String>) {
    let mut line: Vec<u8> = Vec::with_capacity(LISTFILE_LINE_MAX);
    for &b in listfile {
        if b != b'\r' && b != b'\n' {
            if line.len() < LISTFILE_LINE_MAX {
                line.push(b);
            }
            continue;
        }
        pass_line(&line, prefix, out);
        line.clear();
    }
    pass_line(&line, prefix, out);
}

/// One finished listfile line, passed unless empty, a `.md5` or outside the prefix.
fn pass_line(line: &[u8], prefix: &str, out: &mut Vec<String>) {
    let md5 = line
        .iter()
        .rposition(|&c| c == b'.')
        .is_some_and(|dot| line[dot..].eq_ignore_ascii_case(b".md5"));
    let under =
        line.len() >= prefix.len() && line[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes());
    if !line.is_empty() && !md5 && under {
        out.push(String::from_utf8_lossy(line).into_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines end at CR or LF; the prefix compares without case and reads `/` as itself; `.md5`
    /// lines and empty lines are passed over, an unterminated last line is kept; a long line is cut.
    #[test]
    fn a_listfile_walk_passes_the_lines_under_the_prefix() {
        let long = format!("Interface\\Icons\\{}", "x".repeat(400));
        let text = format!(
            "Interface\\Icons\\Spell_A.blp\r\ninterface\\ICONS\\b.blp\n\nInterface/Icons/c.blp\n\
             Interface\\Icons\\d.blp.MD5\rTextures\\e.blp\n{long}\nInterface\\Icons\\last.blp"
        );
        let mut out = Vec::new();
        listfile_lines(text.as_bytes(), "Interface\\Icons\\", &mut out);
        assert_eq!(out.len(), 4, "{out:?}");
        assert_eq!(out[0], "Interface\\Icons\\Spell_A.blp");
        assert_eq!(out[1], "interface\\ICONS\\b.blp");
        assert_eq!(out[2].len(), LISTFILE_LINE_MAX);
        assert_eq!(
            out[3], "Interface\\Icons\\last.blp",
            "the unterminated last line"
        );
    }

    /// A minimal V1 archive holding stored files, its tables encrypted as the reader expects: the
    /// MPQ string hash and table cipher, written out here since `benilla-mpq` keeps its own private.
    fn fixture_mpq(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut table = [0u32; 0x500];
        let mut seed: u32 = 0x0010_0001;
        for i in 0..0x100 {
            for j in 0..5 {
                seed = (seed * 125 + 3) % 0x2A_AAAB;
                let hi = (seed & 0xFFFF) << 16;
                seed = (seed * 125 + 3) % 0x2A_AAAB;
                table[i + j * 0x100] = hi | (seed & 0xFFFF);
            }
        }
        let hash = |name: &str, kind: usize| {
            let (mut s1, mut s2): (u32, u32) = (0x7FED_7FED, 0xEEEE_EEEE);
            for b in name.bytes() {
                let ch = b.to_ascii_uppercase() as u32;
                s1 = table[kind + ch as usize] ^ s1.wrapping_add(s2);
                s2 = ch
                    .wrapping_add(s1)
                    .wrapping_add(s2)
                    .wrapping_add(s2 << 5)
                    .wrapping_add(3);
            }
            s1
        };
        let encrypt = |data: &mut [u32], mut key: u32| {
            let mut seed: u32 = 0xEEEE_EEEE;
            for v in data.iter_mut() {
                seed = seed.wrapping_add(table[0x400 + (key & 0xFF) as usize]);
                let plain = *v;
                *v = plain ^ key.wrapping_add(seed);
                key = (!key << 0x15).wrapping_add(0x1111_1111) | (key >> 0x0B);
                seed = plain
                    .wrapping_add(seed)
                    .wrapping_add(seed << 5)
                    .wrapping_add(3);
            }
        };
        const SLOTS: usize = 16;
        let hash_pos = 32u32;
        let block_pos = hash_pos + SLOTS as u32 * 16;
        let mut data_pos = block_pos + files.len() as u32 * 16;
        let mut hashes = vec![0xFFFF_FFFFu32; SLOTS * 4];
        let mut blocks = Vec::new();
        let mut body = Vec::new();
        for (i, (name, bytes)) in files.iter().enumerate() {
            let mut slot = hash(name, 0) as usize & (SLOTS - 1);
            while hashes[slot * 4 + 3] != 0xFFFF_FFFF {
                slot = (slot + 1) & (SLOTS - 1);
            }
            hashes[slot * 4..slot * 4 + 4].copy_from_slice(&[
                hash(name, 0x100),
                hash(name, 0x200),
                0,
                i as u32,
            ]);
            blocks.extend([
                data_pos,
                bytes.len() as u32,
                bytes.len() as u32,
                0x8000_0000,
            ]);
            body.extend_from_slice(bytes);
            data_pos += bytes.len() as u32;
        }
        encrypt(&mut hashes, hash("(hash table)", 0x300));
        encrypt(&mut blocks, hash("(block table)", 0x300));
        let mut out = Vec::new();
        for w in [
            0x1A51_504D,
            32,
            data_pos,
            0,
            hash_pos,
            block_pos,
            SLOTS as u32,
        ] {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out.extend_from_slice(&(files.len() as u32).to_le_bytes());
        // The 16-bit format version and block size shift (bytes 12..16) stay 0.
        for w in hashes.into_iter().chain(blocks) {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out.extend_from_slice(&body);
        out
    }

    /// The walk reads the patches and `speech2.MPQ` in mount order, then `interface.MPQ`, and no
    /// other base archive, each listfile's lines as written.
    #[test]
    fn the_archive_walk_reads_the_patches_then_interface_mpq() {
        let dir = std::env::temp_dir().join(format!(
            "benilla-chain-walk-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for archive in [
            "interface.MPQ",
            "model.MPQ",
            "dbc.MPQ",
            "patch.MPQ",
            "patch-2.MPQ",
            "speech2.MPQ",
        ] {
            let line = format!("X\\{archive}\r\nY\\{archive}\r\n");
            std::fs::write(
                dir.join(archive),
                fixture_mpq(&[("(listfile)", line.as_bytes())]),
            )
            .unwrap();
        }
        let chain = Chain::open(&dir).expect("open the fixture chain");
        let walked = chain.enumerate_archive_files("x\\");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            walked,
            [
                "X\\patch.MPQ",
                "X\\patch-2.MPQ",
                "X\\speech2.MPQ",
                "X\\interface.MPQ"
            ]
        );
    }

    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn patch_glob_matches_exactly_one_character_case_insensitively() {
        assert!(is_patch_glob_match("patch-2.MPQ"));
        assert!(is_patch_glob_match("patch-3.MPQ"));
        assert!(is_patch_glob_match("PATCH-A.mpq"));
        assert!(!is_patch_glob_match("patch-.MPQ"));
        assert!(!is_patch_glob_match("patch-10.MPQ"));
        assert!(!is_patch_glob_match("patch-33.MPQ"));
        assert!(!is_patch_glob_match("patch.MPQ"));
        assert!(!is_patch_glob_match("patch-2.MPQ.bak"));
        assert!(!is_patch_glob_match("mypatch-2.MPQ"));
    }

    #[test]
    fn mount_order_is_the_carved_law() {
        // base.MPQ is telemetry-only in the reference and never mounts.
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
