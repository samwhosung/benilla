//! The order of a scope's macro table: `0x4f0d30` collects the scope's records from the list and
//! sorts them with the CRT `qsort` (`0x73f727`) and the compare `0x4f0e10`, `SStrCmpI` on the names
//! (`0x64a4c0`). The sort is not stable, so records whose names match without case sit in the
//! order that `qsort` leaves them, which this reproduces.

use std::cmp::Ordering;

/// `SStrCmpI` (`0x64a4c0`), which is `_strnicmp` (`0x414310`) with no locale set: the bytes
/// compared unsigned after `A` to `Z` fold to lower case, a string that ends first sorting first.
pub(super) fn name_cmp(a: &str, b: &str) -> Ordering {
    let fold = |c: u8| c.to_ascii_lowercase();
    a.bytes().map(fold).cmp(b.bytes().map(fold))
}

/// The CRT `qsort` at `0x73f727`: a partition sort that hands a slice of 8 or fewer elements to
/// `shortsort` (`0x73f87b`, `73f772 cmp eax,0x8; ja`). The pivot is the middle element moved to the
/// front (`73f7a6`-`73f7b0`); the stack of pending ranges is walked last in, first out, and the
/// larger side is pushed (`73f812`-`73f85f`).
pub(super) fn crt_qsort<T>(items: &mut [T], mut cmp: impl FnMut(&T, &T) -> Ordering) {
    if items.len() < 2 {
        return;
    }
    let mut pending: Vec<(isize, isize)> = Vec::new();
    let (mut lo, mut hi) = (0isize, items.len() as isize - 1);
    loop {
        let size = hi - lo + 1;
        if size <= 8 {
            shortsort(items, lo, hi, &mut cmp);
        } else {
            items.swap((lo + size / 2) as usize, lo as usize);
            let (mut loguy, mut higuy) = (lo, hi + 1);
            loop {
                // `73f7bd`-`73f7d3`: up while at most the pivot.
                loop {
                    loguy += 1;
                    if loguy > hi || cmp(&items[loguy as usize], &items[lo as usize]).is_gt() {
                        break;
                    }
                }
                // `73f7d5`-`73f7e5`: down while at least the pivot.
                loop {
                    higuy -= 1;
                    if higuy <= lo || cmp(&items[higuy as usize], &items[lo as usize]).is_lt() {
                        break;
                    }
                }
                if higuy < loguy {
                    break;
                }
                items.swap(loguy as usize, higuy as usize);
            }
            items.swap(lo as usize, higuy as usize);
            // `73f812 cmp ecx,edx; jl`: the left part is the larger when it is the longer
            // by one or more, and then it waits while the right part is sorted first.
            if higuy - 1 - lo >= hi - loguy {
                if lo + 1 < higuy {
                    pending.push((lo, higuy - 1));
                }
                if loguy < hi {
                    lo = loguy;
                    continue;
                }
            } else {
                if loguy < hi {
                    pending.push((loguy, hi));
                }
                if lo + 1 < higuy {
                    hi = higuy - 1;
                    continue;
                }
            }
        }
        match pending.pop() {
            Some((l, h)) => (lo, hi) = (l, h),
            None => return,
        }
    }
}

/// `shortsort` (`0x73f87b`): each pass moves the largest of `lo..=hi` to `hi`, the later of equal
/// largest winning no place (`73f8a6 jle`: only a strictly greater element replaces the maximum).
fn shortsort<T>(
    items: &mut [T],
    lo: isize,
    mut hi: isize,
    cmp: &mut impl FnMut(&T, &T) -> Ordering,
) {
    while hi > lo {
        let mut max = lo;
        for p in lo + 1..=hi {
            if cmp(&items[p as usize], &items[max as usize]).is_gt() {
                max = p;
            }
        }
        items.swap(max as usize, hi as usize);
        hi -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(names: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        crt_qsort(&mut v, |a, b| name_cmp(a, b));
        v
    }

    /// `_strnicmp` folds to lower case, so the punctuation between `Z` and `a` sorts below letters.
    #[test]
    fn the_compare_folds_ascii_to_lower_and_orders_bytes_unsigned() {
        assert_eq!(name_cmp("abc", "ABC"), Ordering::Equal);
        assert_eq!(name_cmp("a", "ab"), Ordering::Less);
        assert_eq!(
            name_cmp("_x", "ax"),
            Ordering::Less,
            "0x5f sorts below 0x61"
        );
        assert_eq!(name_cmp("Zed", "apple"), Ordering::Greater);
        assert_eq!(name_cmp("é", "z"), Ordering::Greater, "bytes are unsigned");
    }

    /// Three elements are `shortsort`ed: pass one moves the first `b` (the maximum, `B` only ties
    /// it) behind `B`, so the equal pair ends in the reverse of its creation order. A stable sort
    /// would give `a, b, B`.
    #[test]
    fn a_tie_ends_where_the_crt_shortsort_leaves_it() {
        assert_eq!(sorted(&["b", "a", "B"]), ["a", "B", "b"]);
        assert_eq!(sorted(&["a", "b", "B"]), ["a", "B", "b"]);
        assert_eq!(sorted(&["B", "b", "a"]), ["a", "b", "B"]);
    }

    /// Past eight elements the partition runs. These are the outputs of a transcription of
    /// `0x73f727` into Python run over the same lists: ties among equal names (`d`/`D`, `m`/`M`)
    /// fall where the pivot and the partition put them.
    #[test]
    fn a_partitioned_sort_places_equal_names_as_the_crt_does() {
        let names = [
            "m", "d", "Q", "D", "z", "a", "M", "k", "r", "b", "x", "c", "y", "e", "w",
        ];
        assert_eq!(
            sorted(&names),
            ["a", "b", "c", "D", "d", "e", "k", "M", "m", "Q", "r", "w", "x", "y", "z"]
        );
    }

    #[test]
    fn short_and_sorted_inputs_stay_as_they_are() {
        assert!(sorted(&[]).is_empty());
        assert_eq!(sorted(&["one"]), ["one"]);
        let ordered = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"];
        assert_eq!(sorted(&ordered), ordered);
        let reversed: Vec<&str> = ordered.iter().rev().copied().collect();
        assert_eq!(sorted(&reversed), ordered);
    }
}
