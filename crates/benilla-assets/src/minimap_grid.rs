//! The WMO interior minimap tile grid: how one group's footprint splits into the
//! `<wmo>_<group>_<X>_<Y>.blp` tiles the client streams inside a building (tile producer
//! `0x6a5270`). Each tile is a power-of-two texel square clamped to 32..256 px at 0.5 yd a texel,
//! and both roundings go up: the round-to-int (`0x73fdf5`) runs round-toward-+∞, so the edge
//! is `2^ceil(log2(extent·2))` px and the count `ceil(extent / 128)`.

/// World yards per minimap texel (`0xca7ebc`).
pub const YD_PER_TEXEL: f32 = 0.5;

/// The tile edge clamp in texels; a full 256 px tile spans 128 yd.
const MIN_TILE_PX: u32 = 32;
const MAX_TILE_PX: u32 = 256;

/// World yards a full tile spans, the divisor of the tile count.
const TILE_SPAN_YD: f32 = MAX_TILE_PX as f32 * YD_PER_TEXEL;

/// One axis of a group's footprint `extent` (yd) as `(tile_count, tile_world)`: the tiles across
/// it and the yards each one covers.
pub fn group_axis_grid(extent: f32) -> (u32, f32) {
    let texels = (extent / YD_PER_TEXEL).max(1.0);
    let px = next_pow2(texels.ceil() as u32).clamp(MIN_TILE_PX, MAX_TILE_PX);
    let tile_world = px as f32 * YD_PER_TEXEL;
    let count = (extent / TILE_SPAN_YD).ceil().max(1.0) as u32;
    (count, tile_world)
}

/// Smallest power of two `>= n` (with `next_pow2(0) == 1`).
fn next_pow2(n: u32) -> u32 {
    if n <= 1 {
        1
    } else {
        1u32 << (32 - (n - 1).leading_zeros())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_pow2_rounds_up() {
        assert_eq!(next_pow2(1), 1);
        assert_eq!(next_pow2(41), 64);
        assert_eq!(next_pow2(64), 64);
        assert_eq!(next_pow2(65), 128);
        assert_eq!(next_pow2(372), 512);
    }

    #[test]
    fn grid_counts_and_sizes_match_ironforge_footprints() {
        // Extents (yd) of Ironforge groups 1, 44 (X, Y), 66 and 89 (X, Y); counts from the trs.
        assert_eq!(group_axis_grid(20.6), (1, 32.0));
        assert_eq!(group_axis_grid(51.3), (1, 64.0));
        assert_eq!(group_axis_grid(201.0), (2, 128.0));
        assert_eq!(group_axis_grid(185.6), (2, 128.0));
        assert_eq!(group_axis_grid(128.4).0, 2);
        assert_eq!(group_axis_grid(107.7).0, 1);
    }
}
