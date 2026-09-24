//! The WMO **interior minimap** tile grid: how one WMO group's world footprint subdivides into the
//! `<wmo>_<group>_<X>_<Y>.blp` minimap tiles the client streams inside a building (decision 0203's
//! interior arc). Pure geometry — no Bevy — so the app renderer and the against-real-data test share
//! one source of truth.
//!
//! Mechanism (the reference's tile producer `0x6a5270`, in the CMapObj/WMO TU; the grid
//! **corrected + verified against Ironforge's authored tiles this session**, see
//! `ironforge_group_grid_matches_trs`): the bake is a fixed **0.5 yd/texel**, each tile a
//! power-of-two texel square clamped to `[32, 256]` px, sized to cover the group's footprint extent,
//! and the tiles tile across that extent.
//!
//! Both roundings go **up**: the binary's round-to-int (`FUN_0073fdf5`) loads a control word with
//! the RC field = round-toward-+∞, i.e. `ceil` — so the pixel edge is `2^ceil(log2(extent·2))` and
//! the count is `ceil(extent / 128)` (a full 256-px tile = 128 yd). `floor` would under-count
//! (group 1 → 0 columns, group 66 → 1×1); `ceil` matches the authored data.

/// The minimap tile bake resolution: world yards per texel (RE constant `0xca7ebc`).
pub const YD_PER_TEXEL: f32 = 0.5;

/// Smallest / largest authored tile edge in texels (RE clamp; a full tile = 256 px = 128 yd).
const MIN_TILE_PX: u32 = 32;
const MAX_TILE_PX: u32 = 256;

/// World yards a full (256-px) tile spans — the fixed divisor of the `ceil(extent/128)` tile count.
const TILE_SPAN_YD: f32 = MAX_TILE_PX as f32 * YD_PER_TEXEL;

/// For one axis of a WMO group whose footprint `extent` (yards) is known: `(tile_count, tile_world)`
/// — how many minimap tiles span that axis, and the world size (yards) each tile covers (its stride).
/// The tile world size is the next-pow2 texel square (clamped) that covers `extent` at `0.5 yd/texel`;
/// the count is `ceil(extent / 128)`, both per the binary's round-toward-+∞.
///
/// Verified against Ironforge: group 66 → 2×2, 44 → 1×2, 89 → 2×1, small groups → 1×1
/// (`benilla-assets` test `ironforge_group_grid_matches_trs`).
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
        // Extents (yd) read off the real Ironforge groups; expected counts read off the trs tiles.
        // grp 1  (20.6): 1 tile, sized 64 px = 32 yd.
        assert_eq!(group_axis_grid(20.6), (1, 32.0));
        // grp 44 X (51.3): 1 tile of 128 px = 64 yd; Y (201.0): 2 tiles of 256 px = 128 yd.
        assert_eq!(group_axis_grid(51.3), (1, 64.0));
        assert_eq!(group_axis_grid(201.0), (2, 128.0));
        // grp 66 (185.6): 2 tiles of 128 yd.
        assert_eq!(group_axis_grid(185.6), (2, 128.0));
        // grp 89 X (128.4): just over one tile → 2; Y (107.7): 1.
        assert_eq!(group_axis_grid(128.4).0, 2);
        assert_eq!(group_axis_grid(107.7).0, 1);
    }
}
