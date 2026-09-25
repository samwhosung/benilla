//! FrameXML `Backdrop`s at the reference's geometry (`0x77e8d0`, `0x77f0c0`): the `edgeFile`
//! strip split into eight upright pieces, the border rig fitted to its frame's laid-out size, and
//! the tiled background.

use bevy::prelude::*;

use benilla_assets::{sprite_image, sprite_image_tiled, WorldAssets};

/// `UI-Tooltip-Background`'s native size, converting a tile period to a `stretch_value`.
const TOOLTIP_BG_NATIVE: f32 = 64.0;

// ── WoW backdrops ────────────────────────────────────────────────────────────────────────────────

/// One `Backdrop` `edgeFile`'s eight pieces, split out upright and ready to draw.
#[derive(Clone)]
pub(crate) struct BackdropEdges {
    pub(crate) left: Handle<Image>,
    pub(crate) right: Handle<Image>,
    pub(crate) top: Handle<Image>,
    pub(crate) bottom: Handle<Image>,
    pub(crate) tl: Handle<Image>,
    pub(crate) tr: Handle<Image>,
    pub(crate) bl: Handle<Image>,
    pub(crate) br: Handle<Image>,
}

/// Split an `edgeFile` strip into its eight upright `e`×`e` pieces, in the strip's order: left,
/// right, top, bottom, top-left, top-right, bottom-left, bottom-right (`0x77e8d0`, `0x77f0c0`).
///
/// Top and bottom are stored rotated 90° (the reference maps atlas u to screen y and v to screen x
/// reversed), so they are un-rotated here: top reads texel `(2e+y, e−1−x)`, bottom `(3e+y, e−1−x)`.
pub(crate) fn split_backdrop_edges(e: usize, strip: &[u8]) -> [Vec<u8>; 8] {
    let src = |x: usize, y: usize| {
        let i = (y * 8 * e + x) * 4;
        &strip[i..i + 4]
    };
    let mut cells = std::array::from_fn(|_| vec![0u8; e * e * 4]);
    for y in 0..e {
        for x in 0..e {
            let mut put = |cell: usize, px: &[u8]| {
                let i = (y * e + x) * 4;
                cells[cell][i..i + 4].copy_from_slice(px);
            };
            put(0, src(x, y)); // LEFT   ← slice 0, upright
            put(1, src(e + x, y)); // RIGHT  ← slice 1, upright
            put(2, src(2 * e + y, e - 1 - x)); // TOP    ← slice 2, un-rotated
            put(3, src(3 * e + y, e - 1 - x)); // BOTTOM ← slice 3, un-rotated
            put(4, src(4 * e + x, y)); // TOPLEFT
            put(5, src(5 * e + x, y)); // TOPRIGHT
            put(6, src(6 * e + x, y)); // BOTTOMLEFT
            put(7, src(7 * e + x, y)); // BOTTOMRIGHT
        }
    }
    cells
}

/// Load one edge file and split it; the cell size is the strip's height.
pub(super) fn backdrop_edges(
    assets: &mut WorldAssets,
    path: &str,
    images: &mut Assets<Image>,
) -> Option<BackdropEdges> {
    let (w, h, rgba) = assets.decode_rgba(path)?;
    let e = h as usize;
    if e == 0 || w as usize != 8 * e {
        warn!("glue art: {path} is {w}x{h}, not an 8-cell edge strip");
        return None;
    }
    let [left, right, top, bottom, tl, tr, bl, br] = split_backdrop_edges(e, &rgba);
    // The runs tile, so they wrap; the corners clamp, since a repeat-sampled corner bleeds its
    // opposite edge in under linear filtering.
    let mut run = |px: Vec<u8>| images.add(sprite_image_tiled(h, h, px));
    let (left, right, top, bottom) = (run(left), run(right), run(top), run(bottom));
    let mut corner = |px: Vec<u8>| images.add(sprite_image(h, h, px));
    Some(BackdropEdges {
        left,
        right,
        top,
        bottom,
        tl: corner(tl),
        tr: corner(tr),
        bl: corner(bl),
        br: corner(br),
    })
}

/// A `Backdrop`'s border (`0x77e8d0`): inside the frame rect, `e`×`e` corners, and edges between
/// them that tile at period `e` and never stretch. Eight pieces, as the reference's eight
/// `CSimpleTexture`s, on a full-bleed rig that [`fit_backdrop_borders`] fits after layout.
///
/// Not Bevy's `TextureSlicer`: it shrinks the corners on a frame shorter than 3·`e`, pulling the
/// border off the authored `BackgroundInsets`.
pub(crate) fn backdrop_border(
    b: &mut ChildSpawnerCommands,
    edges: &BackdropEdges,
    e: f32,
    color: Color,
) {
    let mut rig = b.spawn((
        BackdropRig {
            e,
            fitted: (Vec2::ZERO, 0.0),
        },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            top: Val::Px(0.0),
            bottom: Val::Px(0.0),
            ..default()
        },
    ));
    rig.with_children(|rig| {
        for (img, kind) in [
            (&edges.tl, BackdropPiece::Tl),
            (&edges.tr, BackdropPiece::Tr),
            (&edges.bl, BackdropPiece::Bl),
            (&edges.br, BackdropPiece::Br),
            (&edges.left, BackdropPiece::Left),
            (&edges.right, BackdropPiece::Right),
            (&edges.top, BackdropPiece::Top),
            (&edges.bottom, BackdropPiece::Bottom),
        ] {
            rig.spawn((
                kind,
                ImageNode {
                    image: img.clone(),
                    color,
                    ..default()
                },
                // Zero-size until the first fit: nothing shows on the pre-layout frame.
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(0.0),
                    height: Val::Px(0.0),
                    ..default()
                },
            ));
        }
    });
}

/// The full-bleed child a [`backdrop_border`] hangs its pieces on; its `ComputedNode` is the
/// frame's laid-out size.
#[derive(Component)]
pub(crate) struct BackdropRig {
    /// The authored `edgeSize`.
    e: f32,
    /// The (physical size, screen scale) last fitted, so a settled frame is skipped.
    fitted: (Vec2, f32),
}

#[derive(Component, Clone, Copy)]
pub(crate) enum BackdropPiece {
    Tl,
    Tr,
    Bl,
    Br,
    Left,
    Right,
    Top,
    Bottom,
}

/// Fit every [`backdrop_border`]'s pieces to its frame's laid-out size, again whenever the frame
/// or the glue scale changes (the dialogs resize to their text).
///
/// Every shared edge is one integer of physical pixels and every piece is placed by
/// left/top/width/height, never a `right`/`bottom` anchor: Bevy rounds position and size
/// separately, so pieces meeting at a fractional coordinate leave a 1 px slit or a darker overlap.
pub(crate) fn fit_backdrop_borders(
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut rigs: Query<(&mut BackdropRig, &ComputedNode, &Children)>,
    mut pieces: Query<(&BackdropPiece, &mut Node, &mut ImageNode)>,
) {
    let s = super::screen_scale(window.single().ok());
    for (mut rig, computed, children) in &mut rigs {
        let size = computed.size();
        if computed.is_empty() || rig.fitted == (size, s) {
            continue;
        }
        rig.fitted = (size, s);
        let inv = computed.inverse_scale_factor;
        let edge = (rig.e * s / inv).round().max(1.0);
        let (bw, bh) = (size.x.round(), size.y.round());
        // The reference floors each run at zero (`side/e − 2`), so on a small frame the corners
        // overlap.
        let run_w = (bw - 2.0 * edge).max(0.0);
        let run_h = (bh - 2.0 * edge).max(0.0);
        // A run tiles along its own axis and maps 1:1 across its `e`-thick side; the period
        // follows the snapped edge so it matches the drawn corner exactly.
        let run = |along_x: bool| NodeImageMode::Tiled {
            tile_x: along_x,
            tile_y: !along_x,
            stretch_value: edge * inv / rig.e,
        };
        for &child in children {
            let Ok((kind, mut node, mut img)) = pieces.get_mut(child) else {
                continue;
            };
            let (x, y, w, h, mode) = match kind {
                BackdropPiece::Tl => (0.0, 0.0, edge, edge, NodeImageMode::Auto),
                BackdropPiece::Tr => (bw - edge, 0.0, edge, edge, NodeImageMode::Auto),
                BackdropPiece::Bl => (0.0, bh - edge, edge, edge, NodeImageMode::Auto),
                BackdropPiece::Br => (bw - edge, bh - edge, edge, edge, NodeImageMode::Auto),
                BackdropPiece::Left => (0.0, edge, edge, run_h, run(false)),
                BackdropPiece::Right => (bw - edge, edge, edge, run_h, run(false)),
                BackdropPiece::Top => (edge, 0.0, run_w, edge, run(true)),
                BackdropPiece::Bottom => (edge, bh - edge, run_w, edge, run(true)),
            };
            // Physical to logical for `Val::Px`; the scale factor at layout restores the integers.
            node.left = Val::Px(x * inv);
            node.top = Val::Px(y * inv);
            node.width = Val::Px(w * inv);
            node.height = Val::Px(h * inv);
            img.image_mode = mode;
        }
    }
}

/// The tiled `UI-Tooltip-Background` behind a bordered box, at the authored `tileSize` period.
pub(crate) fn tiled_bg_node(sheet: Handle<Image>, period: f32, s: f32, color: Color) -> ImageNode {
    ImageNode {
        image: sheet,
        color,
        image_mode: NodeImageMode::Tiled {
            tile_x: true,
            tile_y: true,
            stretch_value: period * s / TOOLTIP_BG_NATIVE,
        },
        ..default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // An e=2 strip holding each pixel's (x, y) in r/g, so every copied texel names its source.
    #[test]
    fn backdrop_split_follows_the_edgefile_law() {
        let e = 2usize;
        let (w, h) = (8 * e, e);
        let mut strip = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                strip[i] = x as u8;
                strip[i + 1] = y as u8;
                strip[i + 2] = 0xAB;
                strip[i + 3] = 0xFF;
            }
        }
        let cells = split_backdrop_edges(e, &strip);
        let at = |cell: usize, x: usize, y: usize| {
            let i = (y * e + x) * 4;
            (cells[cell][i], cells[cell][i + 1])
        };
        // The four upright pieces are direct copies of their slice.
        assert_eq!(at(0, 0, 0), (0, 0)); // LEFT   ← slice 0 (x 0..2)
        assert_eq!(at(0, 1, 1), (1, 1));
        assert_eq!(at(1, 0, 0), (2, 0)); // RIGHT  ← slice 1 (x 2..4)
        assert_eq!(at(4, 0, 0), (8, 0)); // TOPLEFT     ← slice 4 (x 8..10)
        assert_eq!(at(4, 1, 1), (9, 1));
        assert_eq!(at(5, 0, 0), (10, 0)); // TOPRIGHT    ← slice 5
        assert_eq!(at(6, 0, 0), (12, 0)); // BOTTOMLEFT  ← slice 6
        assert_eq!(at(7, 0, 0), (14, 0)); // BOTTOMRIGHT ← slice 7
                                          // TOP ← slice 2, un-rotated: dst(x,y) = src(2e+y, e-1-x).
        assert_eq!(at(2, 0, 0), (4, 1)); // dst(0,0) ← src(4, 1)
        assert_eq!(at(2, 1, 0), (4, 0)); // dst(1,0) ← src(4, 0)
        assert_eq!(at(2, 0, 1), (5, 1)); // dst(0,1) ← src(5, 1)
                                         // BOTTOM ← slice 3 (x 6..8), same un-rotation.
        assert_eq!(at(3, 0, 0), (6, 1));
        assert_eq!(at(3, 1, 1), (7, 0));
        assert!(cells.iter().all(|c| c.len() == e * e * 4));
    }
}
