//! A layer batch's End (`0x7731a0`): the texture quads of one batch
//! ([`crate::order::quad_batch`]), in the order it filled, sorted by texture handle with the CRT
//! `qsort` (`0x73f727`, width `0x1c`, comparator `0x7731c0`: the records' first dwords
//! subtracted). The handle is the first dword (`0x772fd0` → `0x770700`): the device texture
//! `[[region+0xcc]+0x140]` of the region's texture object, which regions showing one texture
//! share ([`super::texel`]). The sort is not stable, so a batch of one texture comes out in the
//! CRT's own order, two quads reversed.
//!
//! Deviation: the handle's value is the address of a `CGxTex` (`SMemAlloc(0x54)`, `0x594a96`),
//! which benilla cannot reproduce; each texture ranks instead by a creation stamp, one global
//! sequence number taken when its cache key is first created and taken anew once its last region
//! has let go of it ([`super::texel::Texels`]). On a heap that grows as it allocates, a later
//! device texture sits at a higher address, so the stamp orders textures as their addresses do,
//! as far as anything deterministic can: it parts from them where the heap reuses a freed block
//! or the device recycles a pooled texture of the same size (`0x448531`).

/// One texture quad of the extract: its index in the list, its batch and its texture's rank.
pub(super) struct BatchQuad {
    pub(super) index: usize,
    pub(super) batch: (u64, u64),
    /// The creation stamp ([`super::texel::Texels::stamp`]).
    pub(super) rank: u64,
}

/// Sort every batch of `quads` (in list order, so each batch's run is contiguous) as `0x7731a0`
/// does: each batch's quads trade places and keys, so the list stays ascending.
pub(super) fn sort_batches(out: &mut [ExtractedQuad], quads: &[BatchQuad]) {
    let mut scratch = Scratch::default();
    for run in quads.chunk_by(|a, b| a.batch == b.batch) {
        // Distinct keys in order are where the CRT leaves them; ties it reorders even sorted.
        if run.windows(2).all(|w| w[0].rank < w[1].rank) {
            continue;
        }
        sort_batch(out, run, &mut scratch);
    }
}

/// The buffers a frame's batch sorts share, allocated once and only if a batch needs sorting.
#[derive(Default)]
struct Scratch {
    keys: Vec<u64>,
    perm: Vec<usize>,
    stack: Vec<(usize, usize)>,
    zs: Vec<u128>,
    placed: Vec<bool>,
}

fn sort_batch(out: &mut [ExtractedQuad], run: &[BatchQuad], sc: &mut Scratch) {
    sc.keys.clear();
    sc.keys.extend(run.iter().map(|q| q.rank));
    crt_qsort_into(&sc.keys, &mut sc.perm, &mut sc.stack);
    if sc.perm.iter().enumerate().all(|(k, &p)| k == p) {
        return;
    }
    sc.zs.clear();
    sc.zs.extend(run.iter().map(|q| out[q.index].z));
    // Apply `new[k] = old[perm[k]]` cycle by cycle, then hand the keys back in order.
    sc.placed.clear();
    sc.placed.resize(run.len(), false);
    for start in 0..run.len() {
        let mut k = start;
        while !sc.placed[k] {
            sc.placed[k] = true;
            let next = sc.perm[k];
            if next == start {
                break;
            }
            out.swap(run[k].index, run[next].index);
            k = next;
        }
    }
    for (q, &z) in run.iter().zip(&sc.zs) {
        out[q.index].z = z;
    }
}

use super::ExtractedQuad;

/// The permutation the CRT `qsort` (`0x73f727`) leaves `keys` in, comparing as `0x7731c0` does:
/// `perm[k]` is the original index now at `k`. A run of at most 8 takes the selection sort
/// (`0x73f87b`), which moves the first-found largest to the end; a longer one partitions about
/// its middle element swapped to the front (`0x73f7a6`), keeping the shorter side on a stack.
pub(super) fn crt_qsort_into(keys: &[u64], v: &mut Vec<usize>, stack: &mut Vec<(usize, usize)>) {
    v.clear();
    v.extend(0..keys.len());
    if v.len() < 2 {
        return;
    }
    let cmp = |v: &[usize], a: usize, b: usize| keys[v[a]].cmp(&keys[v[b]]);
    use std::cmp::Ordering::{Greater, Less};
    stack.clear();
    let (mut lo, mut hi) = (0usize, v.len() - 1);
    loop {
        let size = hi - lo + 1;
        if size <= 8 {
            // `shortsort`: the largest, first found on a tie, swapped to the end, shrinking.
            while hi > lo {
                let mut max = lo;
                for p in lo + 1..=hi {
                    if cmp(v, p, max) == Greater {
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
                while loguy <= hi && cmp(v, loguy, lo) != Greater {
                    loguy += 1;
                }
                higuy -= 1;
                while higuy > lo && cmp(v, higuy, lo) != Less {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crt_qsort(keys: &[u64]) -> Vec<usize> {
        let (mut v, mut stack) = (Vec::new(), Vec::new());
        crt_qsort_into(keys, &mut v, &mut stack);
        v
    }

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
}
