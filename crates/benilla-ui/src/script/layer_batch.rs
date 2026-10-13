//! A layer batch's End (`0x7731a0`): the texture quads of one batch
//! ([`crate::order::quad_batch`]), in the order it filled, sorted by texture handle with the CRT
//! `qsort` (`0x73f727`, width `0x1c`, comparator `0x7731c0`: the records' first dwords
//! subtracted). The handle is the first dword (`0x772fd0` → `0x770700`): the device texture
//! `[[region+0xcc]+0x140]` of the region's texture object. Regions showing one texture share it:
//! a file through the texture cache by name (`0x449d90`), a solid fill through the cache by its
//! packed colour (`0x44aa20`). The sort is not stable, so a batch of one texture comes out in the
//! CRT's own order, two quads reversed.
//!
//! Deviation: the handle's value is the address of a `CGxTex` (`SMemAlloc(0x54)`, `0x594a96`),
//! which benilla cannot reproduce. It ranks solid fills and generated texels first, whose device
//! texture the client makes when they are set (`0x44a940`, `0x449c52`), then files, whose it makes
//! when their asynchronous read completes (`0x44a500` → `0x44a560`, at `0x44a71c`); within each, a
//! texture ranks by its first quad in the batch. Only the order of two different textures rests
//! on that.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use super::{ExtractedQuad, QuadContent};

/// What a batch quad's texture is, for its rank: the reference's two caches and their outside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Texel {
    /// A solid fill (`SetTexture(r, g, b, a)`, `<Color>`), keyed by its packed colour.
    Fill(u32),
    /// A file, keyed by its path ([`ExtractedQuad`]'s), matched as the cache matches it.
    File,
    /// A texture made for this region alone: a portrait, the colour picker's two, a tabard cell.
    Own,
}

impl Texel {
    /// The texture of a region quad's content and its raw fill, `None` when the region has none
    /// (`0x7706e0` appends no quad for an empty `+0xcc`).
    pub(super) fn of(content: &QuadContent, fill: Option<[f32; 4]>) -> Option<Texel> {
        match content {
            QuadContent::Texture {
                path,
                circular,
                portrait_unit,
                ..
            } => {
                let generated = portrait_unit.is_some()
                    || *circular
                    || path
                        .as_deref()
                        .is_some_and(|p| super::emblem_mask_path(p).is_some());
                if generated {
                    Some(Texel::Own)
                } else if path.is_some() {
                    Some(Texel::File)
                } else {
                    fill.map(|f| Texel::Fill(pack(f)))
                }
            }
            QuadContent::ColorWheel | QuadContent::ColorValue { .. } => Some(Texel::Own),
            _ => None,
        }
    }
}

/// A colour as the solid cache keys it, a byte a channel, `×255 + 0.5` truncated after a clamp.
fn pack(c: [f32; 4]) -> u32 {
    c.iter().enumerate().fold(0, |acc, (i, v)| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let byte = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        acc | byte << (8 * i)
    })
}

/// One texture quad of the extract: its index in the list, its batch and its texture.
pub(super) struct BatchQuad {
    pub(super) index: usize,
    pub(super) batch: (u64, u64),
    pub(super) texel: Texel,
}

/// Sort every batch of `quads` (in list order, so each batch's run is contiguous) as `0x7731a0`
/// does: each batch's quads trade places and keys, so the list stays ascending.
pub(super) fn sort_batches(out: &mut [ExtractedQuad], quads: &[BatchQuad]) {
    for run in quads.chunk_by(|a, b| a.batch == b.batch) {
        if run.len() >= 2 {
            sort_batch(out, run);
        }
    }
}

fn sort_batch(out: &mut [ExtractedQuad], run: &[BatchQuad]) {
    // The rank: solid and generated first, then files; within each, the first quad's place.
    let n = run.len();
    let keys: Vec<u64> = {
        let mut files: HashMap<CachedName<'_>, usize> = HashMap::new();
        let mut fills: HashMap<u32, usize> = HashMap::new();
        run.iter()
            .enumerate()
            .map(|(k, q)| {
                let (class, first) = match (q.texel, &out[q.index].content) {
                    (Texel::Fill(c), _) => (0, *fills.entry(c).or_insert(k)),
                    (Texel::File, QuadContent::Texture { path: Some(p), .. }) => {
                        (1, *files.entry(CachedName(p)).or_insert(k))
                    }
                    (Texel::File, _) => (1, k),
                    (Texel::Own, _) => (0, k),
                };
                (class << 32) | first as u64
            })
            .collect()
    };
    let perm = crt_qsort(&keys);
    if perm.iter().enumerate().all(|(k, &p)| k == p) {
        return;
    }
    let zs: Vec<u128> = run.iter().map(|q| out[q.index].z).collect();
    // Apply `new[k] = old[perm[k]]` cycle by cycle, then hand the keys back in order.
    let mut placed = vec![false; n];
    for start in 0..n {
        let mut k = start;
        while !placed[k] {
            placed[k] = true;
            let next = perm[k];
            if next == start {
                break;
            }
            out.swap(run[k].index, run[next].index);
            k = next;
        }
    }
    for (q, z) in run.iter().zip(zs) {
        out[q.index].z = z;
    }
}

/// A texture path as the texture cache keys it: a trailing four-byte `.xxx` extension dropped
/// (`0x449590`), then matched ignoring ASCII case (`0x449eb0` → `0x64a4c0`, `_strnicmp`).
struct CachedName<'a>(&'a str);

impl CachedName<'_> {
    fn folded(&self) -> impl Iterator<Item = u8> + '_ {
        let b = self.0.as_bytes();
        let stem = match b.len().checked_sub(4) {
            Some(dot) if b[dot] == b'.' => &b[..dot],
            _ => b,
        };
        stem.iter().map(u8::to_ascii_uppercase)
    }
}

impl PartialEq for CachedName<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.folded().eq(other.folded())
    }
}

impl Eq for CachedName<'_> {}

impl Hash for CachedName<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for b in self.folded() {
            state.write_u8(b);
        }
    }
}

/// The permutation the CRT `qsort` (`0x73f727`) leaves `keys` in, comparing as `0x7731c0` does:
/// `perm[k]` is the original index now at `k`. A run of at most 8 takes the selection sort
/// (`0x73f87b`), which moves the first-found largest to the end; a longer one partitions about
/// its middle element swapped to the front (`0x73f7a6`), keeping the shorter side on a stack.
pub(super) fn crt_qsort(keys: &[u64]) -> Vec<usize> {
    let mut v: Vec<usize> = (0..keys.len()).collect();
    if v.len() < 2 {
        return v;
    }
    let cmp = |v: &[usize], a: usize, b: usize| keys[v[a]].cmp(&keys[v[b]]);
    use std::cmp::Ordering::{Greater, Less};
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let (mut lo, mut hi) = (0usize, v.len() - 1);
    loop {
        let size = hi - lo + 1;
        if size <= 8 {
            // `shortsort`: the largest, first found on a tie, swapped to the end, shrinking.
            while hi > lo {
                let mut max = lo;
                for p in lo + 1..=hi {
                    if cmp(&v, p, max) == Greater {
                        max = p;
                    }
                }
                v.swap(max, hi);
                hi -= 1;
            }
        } else {
            let mid = lo + size / 2;
            v.swap(mid, lo);
            let (mut loguy, mut higuy) = (lo, hi + 1);
            loop {
                loguy += 1;
                while loguy <= hi && cmp(&v, loguy, lo) != Greater {
                    loguy += 1;
                }
                higuy -= 1;
                while higuy > lo && cmp(&v, higuy, lo) != Less {
                    higuy -= 1;
                }
                if higuy < loguy {
                    break;
                }
                v.swap(loguy, higuy);
            }
            v.swap(lo, higuy);
            // `higuy - 1 - lo >= hi - loguy`, signed (`0x73f812`): recurse into the smaller side.
            let left = higuy as isize - 1 - lo as isize;
            let right = hi as isize - loguy as isize;
            if left >= right {
                if lo + 1 < higuy {
                    stack.push((lo, higuy - 1));
                }
                if loguy < hi {
                    lo = loguy;
                    continue;
                }
            } else {
                if loguy < hi {
                    stack.push((loguy, hi));
                }
                if lo + 1 < higuy {
                    hi = higuy - 1;
                    continue;
                }
            }
        }
        match stack.pop() {
            Some((l, h)) => (lo, hi) = (l, h),
            None => break,
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The selection sort keeps the first-found largest and swaps it to the end, so ties come out
    /// rotated, not kept: two reverse, three go B C A.
    #[test]
    fn short_runs_scramble_ties_as_the_crt_does() {
        assert_eq!(crt_qsort(&[7, 7]), vec![1, 0]);
        assert_eq!(crt_qsort(&[7, 7, 7]), vec![1, 2, 0]);
        assert_eq!(crt_qsort(&[1, 0]), vec![1, 0]);
        assert_eq!(crt_qsort(&[0, 1]), vec![0, 1]);
        // A distinct key splits the tie: [5a, 1, 5b] → max 5a to the end, then 1 and 5b.
        assert_eq!(crt_qsort(&[5, 1, 5]), vec![1, 2, 0]);
    }

    /// Past 8 the run partitions: the middle element goes to the front and is the pivot, and with
    /// every key equal both scans run off their ends, so it only swaps back to the middle.
    #[test]
    fn a_long_tie_moves_only_its_middle_to_the_front() {
        assert_eq!(crt_qsort(&[3; 9]), vec![4, 1, 2, 3, 0, 5, 6, 7, 8]);
    }

    #[test]
    fn the_permutation_sorts_its_keys() {
        let keys: Vec<u64> = (0..40u64).map(|i| (i * 7919) % 13).collect();
        let perm = crt_qsort(&keys);
        let mut seen = perm.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..40).collect::<Vec<_>>());
        assert!(perm.windows(2).all(|w| keys[w[0]] <= keys[w[1]]));
    }

    #[test]
    fn a_path_matches_across_case_and_its_extension() {
        let a = CachedName("Interface\\Glues\\Credits\\Border1");
        let b = CachedName("interface\\glues\\credits\\BORDER1.blp");
        assert!(a == b);
        let hash = |n: &CachedName| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            n.hash(&mut h);
            h.finish()
        };
        assert_eq!(hash(&a), hash(&b));
        assert!(CachedName("Interface\\A") != CachedName("Interface\\B"));
        assert!(
            CachedName("Interface/A") != CachedName("Interface\\A"),
            "the compare folds no separator"
        );
    }
}
