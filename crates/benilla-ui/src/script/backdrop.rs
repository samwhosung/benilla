//! The frame backdrop as the 1.12 client builds it: a `<Backdrop>` element or `SetBackdrop`
//! installs a spec ([`Backdrop`], the 0x68-byte struct at `frame+0x1ac`) and builds up to nine
//! Texture regions on the frame from it (`0x77e8d0`), the background on BACKGROUND and the eight
//! border pieces on BORDER. They are ordinary regions: `GetRegions` returns them, the region draw
//! path paints them, and every Texture verb works on them. The spec drives them: the colour
//! setters tint them and a resize re-runs their texcoords (`0x77f0c0`).

use std::collections::HashMap;

use crate::layout::{self, Anchor, Anchors, Point, Rect};
use crate::order::DrawLayer;
use crate::widget::{FrameHandle, RegionHandle, RegionKind};

use super::texel::TexSet;
use super::{Model, RegionData, TexCoords};

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
    /// The background texture (`+0x24`); none or empty builds no background.
    pub bg_file: Option<String>,
    /// The 8-slice border texture (`+0x30`); none or empty builds no border.
    pub edge_file: Option<String>,
    /// Tile the background, else stretch it (`+0x40`).
    pub(crate) tile: bool,
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

/// One drawable piece, the background or one of the 8 border pieces: its corners, axis-aligned,
/// and per-corner UVs, since the top and bottom edges are rotated 90° (`0x77f0c0`). Both arrays
/// run `[TL, TR, BR, BL]`, y-up.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BackdropPiece {
    /// Corner positions in screen px.
    pub(crate) corners: [[f32; 2]; 4],
    /// Corner UVs in texture space.
    pub(crate) uvs: [[f32; 2]; 4],
    /// The texture needs repeat addressing. The corners stay in `[0,1]` but share the edge texture,
    /// so they carry `true` too and the border batches as one texture (`0x77e8d0`).
    pub tile: bool,
    /// Drawn with `bg_file` and `bg_color`, else with `edge_file` and `border_color`.
    pub(crate) is_bg: bool,
}

const SLICE: f32 = 0.125;

/// One piece's texcoords, `[TL, TR, BR, BL]`.
type Uvs = [[f32; 2]; 4];

/// The eight border pieces in the struct's order, `+0x04` to `+0x20`, which is their creation
/// order and their atlas slice (`0x77e8d0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

const EDGES: [Edge; 8] = [
    Edge::Left,
    Edge::Right,
    Edge::Top,
    Edge::Bottom,
    Edge::TopLeft,
    Edge::TopRight,
    Edge::BottomLeft,
    Edge::BottomRight,
];

/// A piece's anchors on its frame, `(point, relativePoint, x, y)` in the frame's units, and its
/// `(width, height)`, 0 for an axis the anchors span (`0x77e8d0`'s `SetPoint 0x767c70` and
/// layout-vtable `+0x14`/`+0x18` calls).
type PieceLayout = ([Option<(Point, f32, f32)>; 4], (f32, f32));

/// The background's four anchors. Its BOTTOMRIGHT takes the TOP inset for its y, a bug in the
/// reference (`0x77e9ae` loads `+0x50`, top, not `+0x54`), kept: the resolver reads the bottom
/// edge from BOTTOMLEFT and the right edge from TOPRIGHT first (`0x7671a0`, `0x7671f0`), so the
/// bug shows only to `GetPoint`, never on screen.
fn bg_layout(bd: &Backdrop) -> PieceLayout {
    let i = bd.insets;
    (
        [
            Some((Point::TopLeft, i.left, -i.top)),
            Some((Point::TopRight, -i.right, -i.top)),
            Some((Point::BottomLeft, i.left, i.bottom)),
            Some((Point::BottomRight, -i.right, i.top)),
        ],
        (0.0, 0.0),
    )
}

/// A border piece's anchors: corners are `edgeSize` squares on their frame corner, edge strips run
/// between them, all flush inside the frame rect.
fn edge_layout(edge: Edge, e: f32) -> PieceLayout {
    use Point::*;
    let (a, b, size) = match edge {
        Edge::Left => ((TopLeft, 0.0, -e), Some((BottomLeft, 0.0, e)), (e, 0.0)),
        Edge::Right => ((TopRight, 0.0, -e), Some((BottomRight, 0.0, e)), (e, 0.0)),
        Edge::Top => ((TopLeft, e, 0.0), Some((TopRight, -e, 0.0)), (0.0, e)),
        Edge::Bottom => ((BottomLeft, e, 0.0), Some((BottomRight, -e, 0.0)), (0.0, e)),
        Edge::TopLeft => ((TopLeft, 0.0, 0.0), None, (e, e)),
        Edge::TopRight => ((TopRight, 0.0, 0.0), None, (e, e)),
        Edge::BottomLeft => ((BottomLeft, 0.0, 0.0), None, (e, e)),
        Edge::BottomRight => ((BottomRight, 0.0, 0.0), None, (e, e)),
    };
    ([Some(a), b, None, None], size)
}

/// A corner's texcoords, set once at creation (`0x77e8d0`): its slice, upright.
fn corner_uvs(edge: Edge) -> Uvs {
    let i = edge as u8 as f32;
    let (u0, u1) = (i * SLICE, (i + 1.0) * SLICE);
    [[u0, 0.0], [u1, 0.0], [u1, 1.0], [u0, 1.0]]
}

/// The texcoords `0x77f0c0` runs for a frame `rect` on screen: the tiled background's, `None`
/// when it stretches (no `SetTexCoord` call), and the four edge strips', which tile `run` periods
/// between the corners. The divisors are the stored, unscaled sizes (`0x77f101 fdiv [esi+0x48]`,
/// `0x77f195`), so a scaled frame repeats its art more or less often than at scale 1. The
/// background divides the frame's own rect (`0x77f18f`, `0x77f1af`), not its inset quad, so the
/// tiles squeeze by the inset width over the frame width. All four arrays run `[TL, TR, BR, BL]`.
fn run_uvs(bd: &Backdrop, rect: Rect) -> (Option<Uvs>, [Uvs; 4]) {
    let (l, r, b, t) = (rect.left, rect.right, rect.bottom, rect.top);
    let bg = (bd.has_bg() && bd.tile).then(|| {
        let period = if bd.tile_size != 0.0 {
            bd.tile_size
        } else {
            bd.edge_size
        };
        let (wt, ht) = ((r - l) / period, (t - b) / period);
        [[0.0, 0.0], [wt, 0.0], [wt, ht], [0.0, ht]]
    });
    // Run lengths clamp at 0 below two edges and never above.
    let w_run = ((r - l) / bd.edge_size - 2.0).max(0.0);
    let h_run = ((t - b) / bd.edge_size - 2.0).max(0.0);
    let s = SLICE;
    let edges = [
        // LEFT, slice 0, upright.
        [[0.0, 0.0], [s, 0.0], [s, h_run], [0.0, h_run]],
        // RIGHT, slice 1, upright.
        [[s, 0.0], [2.0 * s, 0.0], [2.0 * s, h_run], [s, h_run]],
        // TOP, slice 2, rotated: atlas `u` runs down the screen, atlas `v` right to left.
        [
            [2.0 * s, w_run],
            [2.0 * s, 0.0],
            [3.0 * s, 0.0],
            [3.0 * s, w_run],
        ],
        // BOTTOM, slice 3, rotated as TOP.
        [
            [3.0 * s, w_run],
            [3.0 * s, 0.0],
            [4.0 * s, 0.0],
            [4.0 * s, w_run],
        ],
    ];
    (bg, edges)
}

/// Axis-aligned corners `[TL, TR, BR, BL]`, y-up.
#[cfg(test)]
fn aa_corners(x0: f32, x1: f32, y_bottom: f32, y_top: f32) -> [[f32; 2]; 4] {
    [
        [x0, y_top],    // TL
        [x1, y_top],    // TR
        [x1, y_bottom], // BR
        [x0, y_bottom], // BL
    ]
}

/// The drawable pieces for a frame's resolved rect (y-up), in paint order: the background (layer
/// `BACKGROUND`), then the 8 border pieces (layer `BORDER`), as the regions `install` builds
/// would resolve and draw, the oracle the region tests check them against. Their geometry is in
/// the frame's own units, so it is multiplied by the frame's effective `scale` (`0x76ac90`), as
/// any region's is.
#[cfg(test)]
pub(crate) fn pieces(frame: Rect, bd: &Backdrop, scale: f32) -> Vec<BackdropPiece> {
    let mut out = Vec::with_capacity(9);
    let e = bd.edge_size * scale;
    let (l, r, b, t) = (frame.left, frame.right, frame.bottom, frame.top);
    let (bg_uvs, edge_uvs) = run_uvs(bd, frame);

    if bd.has_bg() {
        // TOPLEFT, TOPRIGHT and BOTTOMLEFT place it; the BOTTOMRIGHT anchor is never read.
        let i = bd.insets;
        out.push(BackdropPiece {
            corners: aa_corners(
                l + i.left * scale,
                r - i.right * scale,
                b + i.bottom * scale,
                t - i.top * scale,
            ),
            // Untiled, no `SetTexCoord` runs and the background stretches over `[0,1]`.
            uvs: bg_uvs.unwrap_or([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]),
            tile: bd.tile,
            is_bg: true,
        });
    }

    if bd.has_border() {
        for (k, edge) in EDGES.into_iter().enumerate() {
            let corners = match edge {
                Edge::Left => aa_corners(l, l + e, b + e, t - e),
                Edge::Right => aa_corners(r - e, r, b + e, t - e),
                Edge::Top => aa_corners(l + e, r - e, t - e, t),
                Edge::Bottom => aa_corners(l + e, r - e, b, b + e),
                Edge::TopLeft => aa_corners(l, l + e, t - e, t),
                Edge::TopRight => aa_corners(r - e, r, t - e, t),
                Edge::BottomLeft => aa_corners(l, l + e, b, b + e),
                Edge::BottomRight => aa_corners(r - e, r, b, b + e),
            };
            out.push(BackdropPiece {
                corners,
                uvs: edge_uvs.get(k).copied().unwrap_or_else(|| corner_uvs(edge)),
                tile: true,
                is_bg: false,
            });
        }
    }

    out
}

// ── The installed backdrop: the spec and its nine regions ───────────────────────────────────

/// A frame's installed backdrop, the struct's spec and its nine piece slots (`+0x00` the
/// background, `+0x04..+0x20` the edges in `EDGES` order), each `None` where no piece was built.
#[derive(Clone, Debug)]
pub(crate) struct Installed {
    pub(crate) spec: Backdrop,
    pieces: [Option<RegionHandle>; 9],
    /// The frame rect the texcoords last ran for; `ApplyRect 0x76b580` re-runs them only when the
    /// size moves past [`layout::size_changed`].
    ran_for: Option<Rect>,
}

impl Installed {
    fn bg(&self) -> Option<RegionHandle> {
        self.pieces[0]
    }

    fn edges(&self) -> impl Iterator<Item = RegionHandle> + '_ {
        self.pieces[1..].iter().flatten().copied()
    }
}

/// `CSimpleFrame::SetBackdrop 0x76a5d0`: release the frame's nine pieces and drop its spec
/// (`0x77f4d0`), then, for a spec, build the pieces (`0x77e8d0`) and run their texcoords against
/// the frame's rect if it has one, as Lua `SetBackdrop` does at its end (`0x7776e0`); otherwise
/// the next resolve runs them. Each piece is a fresh Texture linked at the tail of the region
/// list, so a second backdrop's pieces come after any region made in between.
pub(crate) fn install(model: &mut Model, fh: FrameHandle, spec: Option<Backdrop>) {
    // Each freed piece names its node, and each built one below, so the next resolve takes the
    // swap through the cached graph.
    let old: Vec<RegionHandle> = model
        .backdrops
        .remove(&fh)
        .iter()
        .flat_map(|o| o.pieces)
        .flatten()
        .collect();
    super::region::free_regions(model, &old);
    let Some(spec) = spec.filter(|_| model.arena.frame(fh).is_some()) else {
        return;
    };
    let owner = model.frame_id(fh);
    // `SetTexture 0x770200`: a path that does not load leaves the piece without art.
    let loads = |model: &Model, path: &Option<String>| -> Option<String> {
        let path = path.as_deref().filter(|p| !p.is_empty())?;
        let ok = model.texture_probe.as_ref().is_none_or(|probe| probe(path));
        ok.then(|| path.to_string())
    };
    let bg_art = loads(model, &spec.bg_file);
    let edge_art = loads(model, &spec.edge_file);
    let mut pieces = [None; 9];
    let build = |model: &mut Model,
                 layer: DrawLayer,
                 (anchors, size): PieceLayout,
                 texture: Option<String>,
                 wrap: bool,
                 tex_coords: Option<TexCoords>|
     -> Option<RegionHandle> {
        // `0x76fc40(frame, layer, 1)`: a Texture on the frame, linked at the tail (`0x76a750`).
        let rh = model
            .arena
            .create_region(fh, RegionKind::Texture, layer, 0)?;
        let (anchors, size) = piece_geometry(owner, (anchors, size));
        model.region_data.insert(
            rh,
            RegionData {
                anchors,
                size,
                tex_coords,
                ..RegionData::default()
            },
        );
        if let Some(path) = texture {
            model.set_texel(
                rh,
                if wrap {
                    TexSet::WrapFile(path)
                } else {
                    TexSet::File(path)
                },
            );
        }
        Some(rh)
    };
    if spec.has_bg() {
        // The background loads with the tile flag as its wrap flag (`0x77e9f1`).
        pieces[0] = build(
            model,
            DrawLayer::Background,
            bg_layout(&spec),
            bg_art,
            spec.tile,
            None,
        );
    }
    if spec.has_border() {
        for (k, edge) in EDGES.into_iter().enumerate() {
            let corner = matches!(
                edge,
                Edge::TopLeft | Edge::TopRight | Edge::BottomLeft | Edge::BottomRight
            );
            pieces[k + 1] = build(
                model,
                DrawLayer::Border,
                edge_layout(edge, spec.edge_size),
                edge_art.clone(),
                // A side loads with the wrap flag on, a corner with it off (`0x77ea9e`..`0x77ec9c`,
                // `0x77ed94`..`0x77f09e`): the sides' strips run their texcoords past [0, 1].
                !corner,
                corner.then(|| TexCoords::Corners(corner_uvs(edge))),
            );
        }
    }
    // Each piece is invalidated after its points are set (`0x77e9ca`..`0x77ec77`, `0x77ed05`..).
    for rh in pieces.iter().flatten() {
        model.invalidate_region(*rh, true);
    }
    model.backdrops.insert(
        fh,
        Installed {
            spec,
            pieces,
            ran_for: None,
        },
    );
    if let Some(rect) = model.resolved.get(&fh).copied() {
        run_texcoords(model, fh, rect);
    }
}

/// A piece's anchors on frame `owner` and its explicit size, `None` where the anchors span both
/// axes.
fn piece_geometry(owner: u32, (anchors, size): PieceLayout) -> (Anchors, Option<(f32, f32)>) {
    let anchors = anchors
        .into_iter()
        .flatten()
        .map(|(p, x, y)| Anchor::new(p, owner, p, x, y))
        .collect();
    (anchors, (size != (0.0, 0.0)).then_some(size))
}

/// Re-size an installed backdrop in place, its nine regions kept: the edge size, the tile period
/// and the insets, each piece re-anchored as `0x77e8d0` would build it, then the texcoords re-run.
/// No 1.12 call does this; the engine's chat bubbles take it on a window resize, where the
/// reference's sizes are window-relative and never need it (`0x4b0940`).
pub(crate) fn set_metrics(
    model: &mut Model,
    fh: FrameHandle,
    edge_size: f32,
    tile_size: f32,
    insets: Insets,
) {
    let Some(inst) = model.backdrops.get_mut(&fh) else {
        return;
    };
    if inst.spec.edge_size == edge_size
        && inst.spec.tile_size == tile_size
        && inst.spec.insets == insets
    {
        return;
    }
    inst.spec.edge_size = edge_size;
    inst.spec.tile_size = tile_size;
    inst.spec.insets = insets;
    inst.ran_for = None;
    let (spec, pieces) = (inst.spec.clone(), inst.pieces);
    let owner = model.frame_id(fh);
    let layouts = std::iter::once(bg_layout(&spec)).chain(EDGES.map(|e| edge_layout(e, edge_size)));
    for (slot, layout) in pieces.into_iter().zip(layouts) {
        let Some(d) = slot.and_then(|rh| model.region_data.get_mut(&rh)) else {
            continue;
        };
        (d.anchors, d.size) = piece_geometry(owner, layout);
    }
    // Set as `0x77e8d0` sets them, so invalidated as there.
    for rh in pieces.iter().flatten() {
        model.invalidate_region(*rh, true);
    }
    if let Some(rect) = model.resolved.get(&fh).copied() {
        run_texcoords(model, fh, rect);
    }
}

/// `0x77f0c0` for frame `fh` at `rect`, from `install`.
fn run_texcoords(model: &mut Model, fh: FrameHandle, rect: Rect) {
    let Model {
        backdrops,
        region_data,
        ..
    } = model;
    if let Some(inst) = backdrops.get_mut(&fh) {
        rerun(inst, region_data, rect);
    }
}

/// `0x77f0c0`: the tiled background's and the edge strips' texcoords for a frame at `rect`, the
/// corners' left as created, then both colours re-applied (`0x77f410`, `0x77f440`).
fn rerun(inst: &mut Installed, region_data: &mut HashMap<RegionHandle, RegionData>, rect: Rect) {
    inst.ran_for = Some(rect);
    let (bg, edges) = run_uvs(&inst.spec, rect);
    if let (Some(rh), Some(uvs)) = (inst.bg(), bg) {
        if let Some(d) = region_data.get_mut(&rh) {
            d.tex_coords = Some(TexCoords::Corners(uvs));
        }
    }
    for (slot, uvs) in inst.pieces[1..5].iter().zip(edges) {
        if let Some(d) = slot.and_then(|rh| region_data.get_mut(&rh)) {
            d.tex_coords = Some(TexCoords::Corners(uvs));
        }
    }
    paint(inst, region_data);
}

/// Each piece's single vertex colour (`0x77f750`): the backdrop colour on the background only
/// (`0x77f410`), the border colour on the eight edges only (`0x77f440`).
fn paint(inst: &Installed, region_data: &mut HashMap<RegionHandle, RegionData>) {
    if let Some(d) = inst.bg().and_then(|rh| region_data.get_mut(&rh)) {
        d.set_vertex_color(Some(inst.spec.bg_color));
    }
    for rh in inst.edges() {
        if let Some(d) = region_data.get_mut(&rh) {
            d.set_vertex_color(Some(inst.spec.border_color));
        }
    }
}

/// `SetBackdropColor` (`0x77f410`) or, with `border`, `SetBackdropBorderColor` (`0x77f440`): store
/// the colour in the spec and tint that colour's pieces.
pub(crate) fn set_color(model: &mut Model, fh: FrameHandle, border: bool, color: [f32; 4]) {
    let Model {
        backdrops,
        region_data,
        ..
    } = model;
    let Some(inst) = backdrops.get_mut(&fh) else {
        return;
    };
    if border {
        inst.spec.border_color = color;
        for rh in inst.edges() {
            if let Some(d) = region_data.get_mut(&rh) {
                d.set_vertex_color(Some(color));
            }
        }
    } else {
        inst.spec.bg_color = color;
        if let Some(d) = inst.bg().and_then(|rh| region_data.get_mut(&rh)) {
            d.set_vertex_color(Some(color));
        }
    }
}

/// After a resolve: re-run the texcoords of every backdrop whose frame rect changed size, as the
/// rect application `0x76b580` calls `0x77f0c0` (at `0x76b61e`); a move alone re-runs nothing.
pub(crate) fn after_resolve(model: &mut Model) {
    let eps = model.size_eps();
    let Model {
        backdrops,
        resolved,
        region_data,
        ..
    } = model;
    for (fh, inst) in backdrops.iter_mut() {
        let Some(&rect) = resolved.get(fh) else {
            continue;
        };
        if inst
            .ran_for
            .is_some_and(|prev| !layout::size_changed(prev, rect, eps))
        {
            continue;
        }
        rerun(inst, region_data, rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let ps = pieces(frame, &bd, 1.0);
        assert_eq!(ps.len(), 9);

        // Symmetric insets hide the bottom-right quirk.
        let bg = ps[0];
        assert!(bg.is_bg);
        assert_eq!(bg.corners[0], [5.0, 95.0]); // TL
        assert_eq!(bg.corners[1], [195.0, 95.0]); // TR
        assert_eq!(bg.corners[2], [195.0, 5.0]); // BR
        assert_eq!(bg.corners[3], [5.0, 5.0]); // BL

        // The span divides the frame's 200×100, not the 190×90 quad (`0x77f18f`): 12.5 tiles
        // across the inset quad, each squeezed to 190/200 of its period.
        assert_eq!(bg.uvs[0], [0.0, 0.0]);
        assert!((bg.uvs[1][0] - 200.0 / 16.0).abs() < 1e-4);
        assert!((bg.uvs[2][1] - 100.0 / 16.0).abs() < 1e-4);

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

    /// The background's BOTTOMRIGHT anchor takes the top inset (`0x77e9ae`), but the resolver
    /// reads the bottom edge from BOTTOMLEFT and the right from TOPRIGHT, so the rect is the
    /// insets' own, top 7 and bottom 9.
    #[test]
    fn the_bg_bottomright_quirk_never_reaches_the_rect() {
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
        let bg = pieces(frame, &bd, 1.0)[0];
        assert_eq!(
            bg.corners,
            [[3.0, 93.0], [196.0, 93.0], [196.0, 9.0], [3.0, 9.0]]
        );
        let (anchors, _) = bg_layout(&bd);
        assert_eq!(
            anchors[3],
            Some((Point::BottomRight, -4.0, 7.0)),
            "the quirk is kept"
        );
    }

    #[test]
    fn run_clamps_to_zero_below_two_edges() {
        // 20/16 - 2 = -0.75.
        let frame = Rect::new(0.0, 0.0, 20.0, 20.0);
        let bd = tooltip_backdrop(16.0, 5.0);
        let ps = pieces(frame, &bd, 1.0);
        let left = ps[1]; // LEFT edge
        assert_eq!(left.uvs[3][1], 0.0, "h_run clamped to 0"); // BL v
        let top = ps[3]; // TOP edge
        assert_eq!(top.uvs[0][1], 0.0, "w_run clamped to 0"); // TL v
    }

    #[test]
    fn presence_gates_pieces() {
        let frame = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert!(pieces(frame, &Backdrop::default(), 1.0).is_empty());
        let bg_only = Backdrop {
            bg_file: Some("bg".into()),
            ..Default::default()
        };
        assert_eq!(pieces(frame, &bg_only, 1.0).len(), 1);
        let border_only = Backdrop {
            edge_file: Some("edge".into()),
            ..Default::default()
        };
        assert_eq!(pieces(frame, &border_only, 1.0).len(), 8);
    }

    /// A half-scale frame: the geometry halves, but `0x77f0c0` divides the scaled rect by the
    /// unscaled sizes, so a 100-wide (on screen) edge repeats `100/16 - 2` times and the
    /// background, over the same 100×50 frame rect, `100/16` by `50/16` times.
    #[test]
    fn a_scaled_frames_texcoords_repeat_by_the_unscaled_sizes() {
        let frame = Rect::new(0.0, 0.0, 50.0, 100.0);
        let bd = tooltip_backdrop(16.0, 4.0);
        let ps = pieces(frame, &bd, 0.5);
        let bg = ps[0];
        assert_eq!(bg.corners[0], [2.0, 48.0], "inset 4 units, 2 on screen");
        assert_eq!(bg.uvs[2], [100.0 / 16.0, 50.0 / 16.0]);
        // TOP, slice 2, rotated: `u` holds the run.
        let top = ps[3];
        assert_eq!(top.corners[0][1], 50.0);
        assert_eq!(top.corners[2][1], 42.0, "8 thick on screen");
        assert_eq!(top.uvs[0][1], 100.0 / 16.0 - 2.0);
    }
}
