//! The three coefficient scans of 14496-2 ("Inverse scan"): the scan index `n` maps to the
//! coefficient at `[v * 8 + u]`. Intra blocks with `ac_pred_flag` take the alternate-horizontal
//! scan when their DC is predicted from the block above, the alternate-vertical one when from the
//! left.

pub(crate) const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

pub(crate) const ALT_VERTICAL: [u8; 64] = [
    0, 8, 16, 24, 1, 9, 2, 10, 17, 25, 32, 40, 48, 56, 57, 49, 41, 33, 26, 18, 3, 11, 4, 12, 19,
    27, 34, 42, 50, 58, 35, 43, 51, 59, 20, 28, 5, 13, 6, 14, 21, 29, 36, 44, 52, 60, 37, 45, 53,
    61, 22, 30, 7, 15, 23, 31, 38, 46, 54, 62, 39, 47, 55, 63,
];

pub(crate) const ALT_HORIZONTAL: [u8; 64] = [
    0, 1, 2, 3, 8, 9, 16, 17, 10, 11, 4, 5, 6, 7, 15, 14, 13, 12, 19, 18, 24, 25, 32, 33, 26, 27,
    20, 21, 22, 23, 28, 29, 30, 31, 34, 35, 40, 41, 48, 49, 42, 43, 36, 37, 38, 39, 44, 45, 46, 47,
    50, 51, 56, 57, 58, 59, 52, 53, 54, 55, 60, 61, 62, 63,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The standard prints each scan as the 8×8 grid of scan indices; a scan is the
    /// inverse of its grid.
    fn from_grid(grid: [[u8; 8]; 8]) -> [u8; 64] {
        let mut scan = [255u8; 64];
        for (v, row) in grid.iter().enumerate() {
            for (u, &n) in row.iter().enumerate() {
                assert_eq!(scan[n as usize], 255, "index {n} twice");
                scan[n as usize] = (v * 8 + u) as u8;
            }
        }
        scan
    }

    #[test]
    fn scans_invert_the_standards_grids() {
        let alt_h = [
            [0, 1, 2, 3, 10, 11, 12, 13],
            [4, 5, 8, 9, 17, 16, 15, 14],
            [6, 7, 19, 18, 26, 27, 28, 29],
            [20, 21, 24, 25, 30, 31, 32, 33],
            [22, 23, 34, 35, 42, 43, 44, 45],
            [36, 37, 40, 41, 46, 47, 48, 49],
            [38, 39, 50, 51, 56, 57, 58, 59],
            [52, 53, 54, 55, 60, 61, 62, 63],
        ];
        let alt_v = [
            [0, 4, 6, 20, 22, 36, 38, 52],
            [1, 5, 7, 21, 23, 37, 39, 53],
            [2, 8, 19, 24, 34, 40, 50, 54],
            [3, 9, 18, 25, 35, 41, 51, 55],
            [10, 17, 26, 30, 42, 46, 56, 60],
            [11, 16, 27, 31, 43, 47, 57, 61],
            [12, 15, 28, 32, 44, 48, 58, 62],
            [13, 14, 29, 33, 45, 49, 59, 63],
        ];
        let zigzag = [
            [0, 1, 5, 6, 14, 15, 27, 28],
            [2, 4, 7, 13, 16, 26, 29, 42],
            [3, 8, 12, 17, 25, 30, 41, 43],
            [9, 11, 18, 24, 31, 40, 44, 53],
            [10, 19, 23, 32, 39, 45, 52, 54],
            [20, 22, 33, 38, 46, 51, 55, 60],
            [21, 34, 37, 47, 50, 56, 59, 61],
            [35, 36, 48, 49, 57, 58, 62, 63],
        ];
        assert_eq!(from_grid(alt_h), ALT_HORIZONTAL);
        assert_eq!(from_grid(alt_v), ALT_VERTICAL);
        assert_eq!(from_grid(zigzag), ZIGZAG);
    }
}
