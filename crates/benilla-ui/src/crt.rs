//! The C runtime's sort and its case-folding compare, as the reference client links them: `qsort`
//! (`0x73f727`) and `_strnicmp` (`0x414310`), which Storm's `SStrCmpI` (`0x64a4c0`) calls. A list
//! the client sorts with `qsort` leaves its ties, and the rows its comparator cannot order, where
//! this partition leaves them, so a list sorted here keeps the reference's order to the last row.

/// The CRT `qsort` at `0x73f727`, `cmp` answering as a C comparator does: below 0, 0 or above 0. A
/// range of 8 or fewer goes to `shortsort` (`0x73f772 cmp eax,0x8; ja`); a longer one partitions
/// about its middle element swapped to the front (`0x73f7a6`-`0x73f7b0`), pushes its larger side
/// on a stack walked last in, first out, and goes on with the smaller (`0x73f812`-`0x73f871`).
/// It never asks the comparator for a total order, so one that ties a row with every other sorts
/// as the reference's does.
pub fn qsort<T>(items: &mut [T], mut cmp: impl FnMut(&T, &T) -> i32) {
    if items.len() < 2 {
        return;
    }
    let mut pending: Vec<(usize, usize)> = Vec::new();
    let (mut lo, mut hi) = (0usize, items.len() - 1);
    loop {
        let size = hi - lo + 1;
        if size <= 8 {
            shortsort(items, lo, hi, &mut cmp);
        } else {
            items.swap(lo + size / 2, lo);
            // `higuy` starts one past `hi` (`0x73f7bb add edi,esi`).
            let (mut loguy, mut higuy) = (lo, hi + 1);
            loop {
                // `0x73f7bd`-`0x73f7d3`: up while at most the pivot.
                loop {
                    loguy += 1;
                    if loguy > hi || cmp(&items[loguy], &items[lo]) > 0 {
                        break;
                    }
                }
                // `0x73f7d5`-`0x73f7e5`: down while at least the pivot.
                loop {
                    higuy -= 1;
                    if higuy <= lo || cmp(&items[higuy], &items[lo]) < 0 {
                        break;
                    }
                }
                if higuy < loguy {
                    break;
                }
                items.swap(loguy, higuy);
            }
            items.swap(lo, higuy);
            // `0x73f812 cmp ecx,edx; jl`, signed: `higuy - 1 - lo >= hi - loguy`.
            if higuy as isize - 1 - lo as isize >= hi as isize - loguy as isize {
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

/// `shortsort` (`0x73f87b`): each pass swaps the largest of `lo..=hi` to `hi`, an equal one later
/// in the range taking no place from the first (`0x73f8a6 jle`).
fn shortsort<T>(items: &mut [T], lo: usize, mut hi: usize, cmp: &mut impl FnMut(&T, &T) -> i32) {
    while hi > lo {
        let mut max = lo;
        for p in lo + 1..=hi {
            if cmp(&items[p], &items[max]) > 0 {
                max = p;
            }
        }
        items.swap(max, hi);
        hi -= 1;
    }
}

/// `SStrCmpI(a, b, 0x7fffffff)` (`0x64a4c0`): `_strnicmp` (`0x414310`) in its C-locale path, `A` to
/// `Z` folded to lower case and the first pair that differs deciding -1 or 1 by its unsigned
/// bytes; a NUL ends either string, and a string that ends first sorts first.
pub fn str_cmp_i(a: &str, b: &str) -> i32 {
    let fold = |s: &str| {
        s.bytes()
            .take_while(|&c| c != 0)
            .map(|c| c.to_ascii_lowercase())
            .collect::<Vec<u8>>()
    };
    match fold(a).cmp(&fold(b)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(names: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        qsort(&mut v, |a, b| str_cmp_i(a, b));
        v
    }

    #[test]
    fn the_compare_folds_ascii_and_orders_bytes_unsigned() {
        assert_eq!(str_cmp_i("abc", "ABC"), 0);
        assert_eq!(str_cmp_i("a", "ab"), -1);
        assert_eq!(str_cmp_i("_x", "ax"), -1, "0x5f sorts below 0x61");
        assert_eq!(str_cmp_i("Zed", "apple"), 1);
        assert_eq!(str_cmp_i("é", "z"), 1, "bytes are unsigned");
        assert_eq!(str_cmp_i("a\0z", "a"), 0, "a NUL ends the string");
    }

    /// `shortsort` moves the first-found maximum to the end, so `b` passes behind `B`.
    #[test]
    fn a_tie_ends_where_the_crt_shortsort_leaves_it() {
        assert_eq!(sorted(&["b", "a", "B"]), ["a", "B", "b"]);
        assert_eq!(sorted(&["a", "b", "B"]), ["a", "B", "b"]);
        assert_eq!(sorted(&["B", "b", "a"]), ["a", "b", "B"]);
    }

    /// Past eight elements the partition runs; equal names fall where the pivot and the
    /// partition put them, as a transcription of `0x73f727` run over the same list leaves them.
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

    /// A comparator with no total order, one element tying every other, sorts without a panic.
    #[test]
    fn a_comparator_that_ties_a_row_with_all_others_sorts_without_a_total_order() {
        let mut v: Vec<Option<u32>> = vec![Some(5), None, Some(1), Some(9), None, Some(3)]
            .into_iter()
            .cycle()
            .take(30)
            .collect();
        qsort(&mut v, |a, b| match (a, b) {
            (Some(x), Some(y)) => (*x as i32) - (*y as i32),
            _ => 0,
        });
        assert_eq!(v.len(), 30);
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
