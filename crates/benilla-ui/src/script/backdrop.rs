//! The frame backdrop as the 1.12 client builds it: the tiled background and 8-piece border that a
//! `<Backdrop>` element or `SetBackdrop` installs. This module owns the data ([`Backdrop`]) and the
//! geometry ([`pieces`], screen-space quads with per-corner UVs); the app renders them.

use crate::layout::Rect;

/// The default `edgeSize`, logical 32: the constructor writes device 0.025 (`DAT_0081c9b0`).
pub const DEFAULT_EDGE_SIZE: f32 = 32.0;

/// `BackgroundInsets`: the background's inset from the frame rect; the border is never inset.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

/// A frame's installed backdrop (the 0x68-byte struct at `frame+0x1ac`): exactly the keys the
/// reader accepts (`0x7776e0`) plus the two colours. The colours default to opaque white; the XML
/// parser defaults a present but partial `<Color>` or `<BorderColor>` to black.
#[derive(Clone, Debug, PartialEq)]
pub struct Backdrop {
    /// The background texture (`+0x24`); none or empty draws no background.
    pub bg_file: Option<String>,
    /// The 8-slice border texture (`+0x30`); none or empty draws no border.
    pub edge_file: Option<String>,
    /// Tile the background, else stretch it (`+0x40`).
    pub tile: bool,
    /// `tileSize` in logical px (`+0x4c`); 0 tiles at `edge_size` (`0x77f0c0`).
    pub tile_size: f32,
    /// `edgeSize` in logical px (`+0x48`): the border piece size and the edge tiling period.
    pub edge_size: f32,
    /// `BackgroundInsets` (`+0x50..0x5c`).
    pub insets: Insets,
    /// The background's vertex colour (`+0x60`, `SetBackdropColor`).
    pub bg_color: [f32; 4],
    /// All 8 border pieces' vertex colour (`+0x64`, `SetBackdropBorderColor`).
    pub border_color: [f32; 4],
}

impl Default for Backdrop {
    fn default() -> Self {
        // The constructor (`0x77e5f0`).
        Self {
            bg_file: None,
            edge_file: None,
            tile: false,
            tile_size: 0.0,
            edge_size: DEFAULT_EDGE_SIZE,
            insets: Insets::default(),
            bg_color: [1.0, 1.0, 1.0, 1.0],
            border_color: [1.0, 1.0, 1.0, 1.0],
        }
    }
}

impl Backdrop {
    fn has_bg(&self) -> bool {
        self.bg_file.as_deref().is_some_and(|s| !s.is_empty())
    }
    fn has_border(&self) -> bool {
        self.edge_file.as_deref().is_some_and(|s| !s.is_empty())
    }
}

/// One drawable piece, the background or one of the 8 border pieces: explicit corners, not a rect,
/// so the bottom-right inset quirk can slant it, and per-corner UVs, since the top and bottom edges
/// are rotated 90° (`0x77f0c0`). Both arrays run `[TL, TR, BR, BL]`, y-up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackdropPiece {
    /// Corner positions in screen px.
    pub corners: [[f32; 2]; 4],
    /// Corner UVs in texture space.
    pub uvs: [[f32; 2]; 4],
    /// The texture needs repeat addressing. The corners stay in `[0,1]` but share the edge texture,
    /// so they carry `true` too and the border batches as one texture (`0x77e8d0`).
    pub tile: bool,
    /// Drawn with `bg_file` and `bg_color`, else with `edge_file` and `border_color`.
    pub is_bg: bool,
}

const SLICE: f32 = 0.125;

/// Pull a piece's UVs half a texel inward on every axis that does not tile, so bilinear
/// magnification cannot sample the neighbouring slice of the shared border atlas (in
/// `ChatBubble-Backdrop`, a white alpha-0 column: a pale line through the speech bubble). A tiling
/// axis, a strip's `v` past 1, is left exact, or each period would land short and walk the art.
#[must_use]
pub fn inset_atlas_bleed(uvs: [[f32; 2]; 4], tex_w: f32, tex_h: f32) -> [[f32; 2]; 4] {
    let mut out = uvs;
    for (axis, tex) in [(0usize, tex_w), (1usize, tex_h)] {
        if tex < 2.0 {
            continue; // a 1-texel axis has no interior
        }
        let (lo, hi) = uvs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), c| {
            (lo.min(c[axis]), hi.max(c[axis]))
        });
        // A range of exactly 1, the corners' `v`, is bounded and gets the inset.
        if hi - lo > 1.0 + f32::EPSILON {
            continue;
        }
        let half = 0.5 / tex;
        if hi - lo <= 2.0 * half {
            continue; // degenerate: the inset would invert the range
        }
        for c in &mut out {
            c[axis] += if c[axis] <= lo { half } else { -half };
        }
    }
    out
}

/// Axis-aligned corners `[TL, TR, BR, BL]`, y-up.
fn aa_corners(x0: f32, x1: f32, y_bottom: f32, y_top: f32) -> [[f32; 2]; 4] {
    [
        [x0, y_top],    // TL
        [x1, y_top],    // TR
        [x1, y_bottom], // BR
        [x0, y_bottom], // BL
    ]
}

/// The drawable pieces for a frame's resolved rect (y-up), in paint order: the background (layer
/// `BACKGROUND`), then the 8 border pieces (layer `BORDER`). Geometry `0x77e8d0`, UVs `0x77f0c0`.
pub fn pieces(frame: Rect, bd: &Backdrop) -> Vec<BackdropPiece> {
    let mut out = Vec::with_capacity(9);
    let e = bd.edge_size;
    let (l, r, b, t) = (frame.left, frame.right, frame.bottom, frame.top);

    // ── The background piece ──
    if bd.has_bg() {
        let (il, ir, it, ib) = (
            bd.insets.left,
            bd.insets.right,
            bd.insets.top,
            bd.insets.bottom,
        );
        // The four anchors, y-up. Bottom-right takes the top inset, as the reference does
        // (`0x77e9ae` loads `+0x50`, top, not `+0x54`); invisible when the two are equal.
        let corners = [
            [l + il, t - it],
            [r - ir, t - it],
            [r - ir, b + it], // BR: the top inset
            [l + il, b + ib],
        ];
        // Untiled, no `SetTexCoord` runs and the background stretches over `[0,1]` (`0x77f0c0`).
        let uvs = if bd.tile {
            let period = if bd.tile_size != 0.0 { bd.tile_size } else { e };
            let bg_w = (r - ir) - (l + il);
            let bg_h = (t - it) - (b + ib);
            let (wt, ht) = (bg_w / period, bg_h / period);
            [[0.0, 0.0], [wt, 0.0], [wt, ht], [0.0, ht]]
        } else {
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
        };
        out.push(BackdropPiece {
            corners,
            uvs,
            tile: bd.tile,
            is_bg: true,
        });
    }

    // ── The 8 border pieces ──
    if bd.has_border() {
        // Edge strips tile `run` periods between the corners, 0 below two edges (`0x77f0c0`).
        let w_run = ((r - l) / e - 2.0).max(0.0);
        let h_run = ((t - b) / e - 2.0).max(0.0);

        let mut edge = |x0: f32, x1: f32, y0: f32, y1: f32, uvs: [[f32; 2]; 4]| {
            out.push(BackdropPiece {
                corners: aa_corners(x0, x1, y0, y1),
                uvs,
                tile: true,
                is_bg: false,
            });
        };

        // LEFT, slice 0, upright.
        edge(
            l,
            l + e,
            b + e,
            t - e,
            [[0.0, 0.0], [SLICE, 0.0], [SLICE, h_run], [0.0, h_run]],
        );
        // RIGHT, slice 1, upright.
        edge(
            r - e,
            r,
            b + e,
            t - e,
            [
                [SLICE, 0.0],
                [2.0 * SLICE, 0.0],
                [2.0 * SLICE, h_run],
                [SLICE, h_run],
            ],
        );
        // TOP, slice 2, rotated: atlas `u` runs down the screen, atlas `v` right to left.
        edge(
            l + e,
            r - e,
            t - e,
            t,
            [
                [2.0 * SLICE, w_run],
                [2.0 * SLICE, 0.0],
                [3.0 * SLICE, 0.0],
                [3.0 * SLICE, w_run],
            ],
        );
        // BOTTOM, slice 3, rotated as TOP.
        edge(
            l + e,
            r - e,
            b,
            b + e,
            [
                [3.0 * SLICE, w_run],
                [3.0 * SLICE, 0.0],
                [4.0 * SLICE, 0.0],
                [4.0 * SLICE, w_run],
            ],
        );
        // The corners, slices 4 to 7, upright.
        let corner_uv = |i: f32| {
            let u0 = i * SLICE;
            let u1 = (i + 1.0) * SLICE;
            [[u0, 0.0], [u1, 0.0], [u1, 1.0], [u0, 1.0]]
        };
        edge(l, l + e, t - e, t, corner_uv(4.0)); // TOPLEFT
        edge(r - e, r, t - e, t, corner_uv(5.0)); // TOPRIGHT
        edge(l, l + e, b, b + e, corner_uv(6.0)); // BOTTOMLEFT
        edge(r - e, r, b, b + e, corner_uv(7.0)); // BOTTOMRIGHT
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On the `ChatBubble-Backdrop` geometry: 256×32, eight 32-texel slices.
    #[test]
    fn the_bleed_inset_lands_on_texel_centres_and_spares_tiling() {
        const W: f32 = 256.0;
        const H: f32 = 32.0;
        let half_u = 0.5 / W;

        let corner = [[0.5, 0.0], [0.625, 0.0], [0.625, 1.0], [0.5, 1.0]];
        let got = inset_atlas_bleed(corner, W, H);
        // 128.5 and 159.5 are the centres of the slice's first and last texels.
        let us: Vec<f32> = got.iter().map(|c| c[0] * W).collect();
        assert!((us[0] - 128.5).abs() < 1e-3, "left u at {} texels", us[0]);
        assert!((us[1] - 159.5).abs() < 1e-3, "right u at {} texels", us[1]);
        let vs: Vec<f32> = got.iter().map(|c| c[1] * H).collect();
        assert!((vs[0] - 0.5).abs() < 1e-3, "top v at {} texels", vs[0]);
        assert!((vs[2] - 31.5).abs() < 1e-3, "bottom v at {} texels", vs[2]);

        let strip = [[0.25, 8.0], [0.25, 0.0], [0.375, 0.0], [0.375, 8.0]];
        let got = inset_atlas_bleed(strip, W, H);
        for (g, s) in got.iter().zip(strip.iter()) {
            assert_eq!(g[1], s[1], "the tiling axis must be bit-exact");
        }
        assert!((got[0][0] - (0.25 + half_u)).abs() < 1e-6);
        assert!((got[2][0] - (0.375 - half_u)).abs() < 1e-6);

        for i in 0u8..8 {
            let (u0, u1) = (f32::from(i) * SLICE, f32::from(i + 1) * SLICE);
            let piece = [[u0, 0.0], [u1, 0.0], [u1, 1.0], [u0, 1.0]];
            for c in inset_atlas_bleed(piece, W, H) {
                assert!(
                    c[0] > u0 && c[0] < u1,
                    "slice {i}: u {} escaped ({u0},{u1})",
                    c[0]
                );
            }
        }

        assert_eq!(inset_atlas_bleed(corner, 0.0, 0.0), corner);
        let sliver = [[0.0, 0.0], [0.001, 0.0], [0.001, 1.0], [0.0, 1.0]];
        assert_eq!(
            inset_atlas_bleed(sliver, W, H)[0][0],
            0.0,
            "too thin to inset"
        );
    }

    fn tooltip_backdrop(edge_size: f32, inset: f32) -> Backdrop {
        Backdrop {
            bg_file: Some("Interface\\Tooltips\\UI-Tooltip-Background".into()),
            edge_file: Some("Interface\\Tooltips\\UI-Tooltip-Border".into()),
            tile: true,
            tile_size: edge_size,
            edge_size,
            insets: Insets {
                left: inset,
                right: inset,
                top: inset,
                bottom: inset,
            },
            ..Default::default()
        }
    }

    // The tooltip's shape on a 200×100 frame (`Rect::new` takes bottom, left, top, right).
    #[test]
    fn piece_geometry_200x100_edge16_inset5() {
        let frame = Rect::new(0.0, 0.0, 100.0, 200.0);
        let bd = tooltip_backdrop(16.0, 5.0);
        let ps = pieces(frame, &bd);
        assert_eq!(ps.len(), 9);

        // Symmetric insets hide the bottom-right quirk.
        let bg = ps[0];
        assert!(bg.is_bg);
        assert_eq!(bg.corners[0], [5.0, 95.0]); // TL
        assert_eq!(bg.corners[1], [195.0, 95.0]); // TR
        assert_eq!(bg.corners[2], [195.0, 5.0]); // BR
        assert_eq!(bg.corners[3], [5.0, 5.0]); // BL
        assert_eq!(bg.uvs[0], [0.0, 0.0]);
        assert!((bg.uvs[1][0] - 190.0 / 16.0).abs() < 1e-4);
        assert!((bg.uvs[2][1] - 90.0 / 16.0).abs() < 1e-4);

        // Border pieces 1 to 8: LEFT, RIGHT, TOP, BOTTOM, TL, TR, BL, BR, flush inside the frame.
        let left = ps[1];
        assert!(!left.is_bg && left.tile);
        assert_eq!(
            left.corners,
            [[0.0, 84.0], [16.0, 84.0], [16.0, 16.0], [0.0, 16.0]]
        );
        // h_run = 100/16 - 2 = 4.25.
        assert_eq!(left.uvs[0], [0.0, 0.0]);
        assert_eq!(left.uvs[1], [0.125, 0.0]);
        assert!((left.uvs[3][1] - 4.25).abs() < 1e-4);

        // TOP, rotated; w_run = 200/16 - 2 = 10.5.
        let top = ps[3];
        assert_eq!(top.corners[0], [16.0, 100.0]); // TL
        assert_eq!(top.corners[2], [184.0, 84.0]); // BR
        assert_eq!(top.uvs[0][0], 0.25); // TL u
        assert_eq!(top.uvs[3][0], 0.375); // BL u
        assert!((top.uvs[0][1] - 10.5).abs() < 1e-4); // TL v = w_run
        assert_eq!(top.uvs[1][1], 0.0); // TR v = 0

        // BOTTOMRIGHT, slice 7, upright.
        let br = ps[8];
        assert_eq!(br.corners[0], [184.0, 16.0]); // TL
        assert_eq!(br.uvs[0], [0.875, 0.0]);
        assert_eq!(br.uvs[2], [1.0, 1.0]);
    }

    #[test]
    fn bg_bottomright_rides_top_inset_bug() {
        let frame = Rect::new(0.0, 0.0, 100.0, 200.0);
        let bd = Backdrop {
            insets: Insets {
                left: 3.0,
                right: 4.0,
                top: 7.0,
                bottom: 9.0,
            },
            ..tooltip_backdrop(16.0, 0.0)
        };
        let bg = pieces(frame, &bd)[0];
        assert_eq!(bg.corners[3], [3.0, 9.0]);
        // BR takes the top inset, 7, not the bottom's 9.
        assert_eq!(bg.corners[2], [196.0, 7.0]);
        assert_eq!(bg.corners[0], [3.0, 93.0]); // TL
        assert_eq!(bg.corners[1], [196.0, 93.0]); // TR
    }

    #[test]
    fn run_clamps_to_zero_below_two_edges() {
        // 20/16 - 2 = -0.75.
        let frame = Rect::new(0.0, 0.0, 20.0, 20.0);
        let bd = tooltip_backdrop(16.0, 5.0);
        let ps = pieces(frame, &bd);
        let left = ps[1]; // LEFT edge
        assert_eq!(left.uvs[3][1], 0.0, "h_run clamped to 0"); // BL v
        let top = ps[3]; // TOP edge
        assert_eq!(top.uvs[0][1], 0.0, "w_run clamped to 0"); // TL v
    }

    #[test]
    fn presence_gates_pieces() {
        let frame = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert!(pieces(frame, &Backdrop::default()).is_empty());
        let bg_only = Backdrop {
            bg_file: Some("bg".into()),
            ..Default::default()
        };
        assert_eq!(pieces(frame, &bg_only).len(), 1);
        let border_only = Backdrop {
            edge_file: Some("edge".into()),
            ..Default::default()
        };
        assert_eq!(pieces(frame, &border_only).len(), 8);
    }
}
