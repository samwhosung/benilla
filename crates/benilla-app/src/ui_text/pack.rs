//! The glyph cache's texture: one shelf-packed sheet, and the sub-rect uploads that carry each new
//! cell to the GPU. The sheet is append-only and a UV normalizes against its fixed side, because
//! UVs are copied where this module cannot reach, into the nameplate meshes' `ATTRIBUTE_UV_0`
//! ([`crate::nameplates`]) that persist on the GPU.
//!
//! Cells go up as sub-rect `RenderQueue::write_texture` writes, so the texture's identity never
//! changes. `Assets::get_mut` must never touch the sheet: in Bevy 0.18.1 a modified `Image` is
//! re-prepared into a new, blank `wgpu::Texture` while [`crate::ui_pass`]'s cached bind groups keep
//! sampling the old one.
//!
//! Deviation: one sheet, where the reference keeps up to 8 pages per font (`[CGxFont+0x18c]`),
//! because a nameplate bakes its glyphs into one mesh with one material, which a page would split.
//! When it fills, [`Sheet::reset`] drops every cell at once, the only event that moves a UV.

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::math::Rect;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// The sheet's side in physical texels: 2048² RGBA8 is 16 MB of VRAM, allocated on the first glyph
/// and never reallocated. `WOW_GLYPH_CACHE=1` reports how full it runs.
pub(super) const SHEET_SIZE: u32 = 2048;

/// Texels between cells, holding each cell's transparent [`frame`]; one is enough, as a quad's edge
/// pixel samples under half a texel outside its cell at any magnification.
const PAD: u32 = 1;

/// A cell's texel payload, in the two shapes the rasterizer produces.
pub(super) enum Cell<'a> {
    /// One byte per texel: coverage, expanded to white × alpha (the vertex color supplies the ink).
    Coverage(&'a [u8]),
    /// Four bytes per texel, straight alpha: an outlined cell, taken verbatim.
    Rgba(&'a [u8]),
}

/// One cell's texels, waiting for the render world to write them into the sheet.
pub(super) struct CellUpload {
    pub(super) image: AssetId<Image>,
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) rgba: Vec<u8>,
}

/// The glyph sheet: one texture, a shelf cursor, and the pending uploads.
pub(super) struct Sheet {
    /// Reserved up front: the engine allocating cells sits behind the lock the script VM reaches
    /// into mid-tick, and can never hold `Assets<Image>`.
    handle: Handle<Image>,
    /// Whether the `Image` asset exists yet. Announced once, on the first cell.
    created: bool,
    announce: bool,
    /// Left edge of the next cell on the current shelf.
    cursor_x: u32,
    /// Top edge of the current shelf.
    cursor_y: u32,
    /// Tallest cell placed on the current shelf.
    row_h: u32,
    uploads: Vec<CellUpload>,
}

/// Wrap a cell's texels in the transparent frame it owns: one texel each side carrying the cell's
/// edge colour at alpha 0, clipped at the sheet's edge, where `ClampToEdge` repeats the cell's own.
/// A magnified quad samples up to half a texel past its cell and [`Sheet::reset`] repacks without
/// erasing, so an unowned gap would draw old ink as a faint box around the letter. The edge colour,
/// not black, keeps straight-alpha filtering from darkening the glyph's edge. Neighbours share one
/// gap texel; the later frame wins, both at alpha 0.
fn frame(x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) -> (u32, u32, u32, u32, Vec<u8>) {
    let (fx, fy) = (x.saturating_sub(1), y.saturating_sub(1));
    let (fw, fh) = (
        (x + w + 1).min(SHEET_SIZE) - fx,
        (y + h + 1).min(SHEET_SIZE) - fy,
    );
    let mut out = vec![0u8; (fw * fh * 4) as usize];
    for row in 0..fh {
        let ty = fy + row;
        let sy = ty.clamp(y, y + h - 1) - y;
        for col in 0..fw {
            let tx = fx + col;
            let sx = tx.clamp(x, x + w - 1) - x;
            let si = ((sy * w + sx) * 4) as usize;
            let di = ((row * fw + col) * 4) as usize;
            out[di..di + 3].copy_from_slice(&rgba[si..si + 3]);
            let inside = tx >= x && tx < x + w && ty >= y && ty < y + h;
            out[di + 3] = if inside { rgba[si + 3] } else { 0 };
        }
    }
    (fx, fy, fw, fh, out)
}

impl Sheet {
    /// Reserve the handle; no texels or `Image` asset exist until a cell lands.
    pub(super) fn new(images: &Assets<Image>) -> Self {
        Self {
            handle: images.reserve_handle(),
            created: false,
            announce: false,
            cursor_x: 0,
            cursor_y: 0,
            row_h: 0,
            uploads: Vec::new(),
        }
    }

    /// The texture every glyph quad samples.
    pub(super) fn handle(&self) -> Handle<Image> {
        self.handle.clone()
    }

    /// Place one cell and queue its texels with their [`frame`]; the UV is the inner rect. `None`
    /// means the sheet is full or the cell can never fit: the caller asks for a reset, and the
    /// glyph is missing for at most one frame.
    pub(super) fn alloc(&mut self, w: u32, h: u32, cell: &Cell<'_>) -> Option<Rect> {
        if w == 0 || h == 0 || w > SHEET_SIZE || h > SHEET_SIZE {
            return None; // a zero-ink glyph (a space), or a cell bigger than the whole sheet
        }
        // Walked on a copy of the cursor and committed only on success, so a cell too tall for the
        // rest of the sheet leaves the open shelf to smaller cells.
        let (mut x, mut y, mut row_h) = (self.cursor_x, self.cursor_y, self.row_h);
        if x + w > SHEET_SIZE {
            x = 0; // next shelf
            y += row_h + PAD;
            row_h = 0;
        }
        if y + h > SHEET_SIZE {
            return None;
        }
        self.cursor_x = x + w + PAD;
        self.cursor_y = y;
        self.row_h = row_h.max(h);

        let rgba = match cell {
            Cell::Coverage(cov) => {
                let mut out = vec![255u8; (w * h * 4) as usize];
                for (i, &a) in cov.iter().enumerate().take((w * h) as usize) {
                    out[i * 4 + 3] = a;
                }
                out
            }
            Cell::Rgba(rgba) => rgba[..(w * h * 4) as usize].to_vec(),
        };
        if !self.created {
            self.created = true;
            self.announce = true;
        }
        let (fx, fy, fw, fh, framed) = frame(x, y, w, h, &rgba);
        self.uploads.push(CellUpload {
            image: self.handle.id(),
            x: fx,
            y: fy,
            w: fw,
            h: fh,
            rgba: framed,
        });
        // Normalized against the fixed side, so a cached UV stays valid for the texture's life.
        let s = SHEET_SIZE as f32;
        Some(Rect::new(
            x as f32 / s,
            y as f32 / s,
            (x + w) as f32 / s,
            (y + h) as f32 / s,
        ))
    }

    /// Take whether the `Image` asset still has to be created, and the cells waiting to be written.
    pub(super) fn take_pending(&mut self) -> (bool, Vec<CellUpload>) {
        (
            std::mem::take(&mut self.announce),
            std::mem::take(&mut self.uploads),
        )
    }

    /// Free every shelf. The texture, its handle and its texels survive, so a quad emitted before
    /// the reset draws until it is re-emitted; the caller bumps the engine's generation. The next
    /// packing lands on the old ink, which is why each cell writes its own [`frame`].
    pub(super) fn reset(&mut self) {
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.row_h = 0;
    }

    /// `(texels committed, texels available)`, committed counting whole shelves with their padding
    /// and slack.
    pub(super) fn occupancy(&self) -> (u64, u64) {
        let used = u64::from(self.cursor_y) * u64::from(SHEET_SIZE)
            + u64::from(self.cursor_x) * u64::from(self.row_h);
        (used, u64::from(SHEET_SIZE) * u64::from(SHEET_SIZE))
    }
}

/// The sheet's `Image`: a GPU texture with no CPU data, zero-initialized by wgpu and written only
/// by sub-rect `write_texture`, straight alpha in sRGB. `MAIN_WORLD | RENDER_WORLD`, because a
/// `RENDER_WORLD`-only image cannot be re-extracted, which a device-lost recreate needs.
pub(super) fn sheet_image() -> Image {
    Image::new_uninit(
        Extent3d {
            width: SHEET_SIZE,
            height: SHEET_SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet() -> Sheet {
        // Only the handle comes from `Assets`, and a bare `Assets` needs no app.
        Sheet::new(&Assets::<Image>::default())
    }

    fn cov(w: u32, h: u32) -> Vec<u8> {
        vec![255u8; (w * h) as usize]
    }

    /// One texel of an upload in sheet coordinates; an upload is the cell plus its frame.
    fn texel(u: &CellUpload, x: u32, y: u32) -> [u8; 4] {
        assert!(
            x >= u.x && x < u.x + u.w && y >= u.y && y < u.y + u.h,
            "({x},{y}) is outside the upload [{}..{}, {}..{}]",
            u.x,
            u.x + u.w,
            u.y,
            u.y + u.h
        );
        let i = (((y - u.y) * u.w + (x - u.x)) * 4) as usize;
        [u.rgba[i], u.rgba[i + 1], u.rgba[i + 2], u.rgba[i + 3]]
    }

    /// A UV normalizes against the fixed side and never moves: the property nameplate meshes bake.
    #[test]
    fn a_uv_is_fixed_against_the_sheet_and_never_moves() {
        let mut s = sheet();
        let c = cov(8, 11);
        let first = s.alloc(8, 11, &Cell::Coverage(&c)).expect("room");
        let side = SHEET_SIZE as f32;
        assert_eq!(first, Rect::new(0.0, 0.0, 8.0 / side, 11.0 / side));
        for _ in 0..2000 {
            s.alloc(8, 11, &Cell::Coverage(&c)).expect("room");
        }
        // The first upload starts at the sheet's corner, its frame clipped there.
        let (_, uploads) = s.take_pending();
        assert_eq!((uploads[0].x, uploads[0].y), (0, 0));
        assert_eq!((uploads[0].w, uploads[0].h), (9, 12));
    }

    #[test]
    fn coverage_uploads_as_white_times_alpha() {
        let mut s = sheet();
        s.alloc(2, 1, &Cell::Coverage(&[0, 128])).expect("room");
        let (_, uploads) = s.take_pending();
        assert_eq!(texel(&uploads[0], 0, 0), [255, 255, 255, 0]);
        assert_eq!(texel(&uploads[0], 1, 0), [255, 255, 255, 128]);

        let mut s = sheet();
        let ring = vec![0u8, 0, 0, 153, 200, 200, 200, 255];
        s.alloc(2, 1, &Cell::Rgba(&ring)).expect("room");
        let (_, uploads) = s.take_pending();
        assert_eq!(texel(&uploads[0], 0, 0), [0, 0, 0, 153]);
        assert_eq!(
            texel(&uploads[0], 1, 0),
            [200, 200, 200, 255],
            "a composited cell is not re-tinted"
        );
    }

    /// A cell uploads its own frame, its edge colour at alpha 0 on every side, since a reset leaves
    /// old ink in the gaps a magnified quad samples.
    #[test]
    fn a_cell_owns_a_transparent_frame_on_every_side() {
        let mut s = sheet();
        // Keep the subject off both sheet edges, so all four sides of its frame are written.
        let wide = SHEET_SIZE - 4;
        s.alloc(wide, 3, &Cell::Coverage(&cov(wide, 3)))
            .expect("shelf 0");
        s.alloc(4, 3, &Cell::Coverage(&cov(4, 3))).expect("shelf 1");
        // Opaque edge texels, so a transparent frame is a real claim.
        let cell = [9u8, 8, 7, 255].repeat(6); // 3x2, uniform
        let uv = s.alloc(3, 2, &Cell::Rgba(&cell)).expect("room");
        let (_, uploads) = s.take_pending();
        let u = uploads.last().expect("the cell went up");

        // The UV names the inner rect; the upload covers it plus one texel each side.
        let side = SHEET_SIZE as f32;
        let (x0, y0) = ((uv.min.x * side) as u32, (uv.min.y * side) as u32);
        assert_eq!(
            (u.x, u.y),
            (x0 - 1, y0 - 1),
            "the frame is uploaded with the cell"
        );
        assert_eq!((u.w, u.h), (3 + 2, 2 + 2));

        for ty in u.y..u.y + u.h {
            for tx in u.x..u.x + u.w {
                let inside = tx >= x0 && tx < x0 + 3 && ty >= y0 && ty < y0 + 2;
                let t = texel(u, tx, ty);
                assert_eq!(
                    &t[..3],
                    &[9, 8, 7],
                    "the frame extrudes the cell's own colour"
                );
                assert_eq!(
                    t[3],
                    if inside { 255 } else { 0 },
                    "({tx},{ty}) alpha: the cell is itself, the frame is transparent"
                );
            }
        }
    }

    #[test]
    fn a_frame_never_lands_on_a_neighbours_ink() {
        let mut s = sheet();
        let c = cov(6, 5);
        let a = s.alloc(6, 5, &Cell::Coverage(&c)).expect("room");
        let b = s.alloc(6, 5, &Cell::Coverage(&c)).expect("room");
        let (_, uploads) = s.take_pending();
        let side = SHEET_SIZE as f32;
        let a_x1 = (a.max.x * side) as u32; // one past A's last ink column
        let b_x0 = (b.min.x * side) as u32;
        assert_eq!(b_x0, a_x1 + PAD, "one padding column between the two cells");
        assert_eq!(
            uploads[1].x, a_x1,
            "B's frame starts in the padding, not on A's last ink column"
        );
    }

    #[test]
    fn shelves_wrap_downward_and_the_sheet_eventually_refuses() {
        let mut s = sheet();
        // Just under half the sheet wide, so exactly two fit on a shelf and the third must wrap.
        let (w, h) = (SHEET_SIZE / 2 - PAD, 64);
        let c = cov(w, h);
        let first = s.alloc(w, h, &Cell::Coverage(&c)).expect("first");
        let second = s.alloc(w, h, &Cell::Coverage(&c)).expect("second");
        let third = s.alloc(w, h, &Cell::Coverage(&c)).expect("third");
        assert_eq!(first.min.y, second.min.y, "the first two share a shelf");
        assert!(second.min.x > first.min.x, "…side by side");
        assert!(
            third.min.y > first.min.y,
            "the third wrapped to the next shelf"
        );

        // It fills rather than grows; the bound only proves the loop ends.
        let mut placed = 3usize;
        while s.alloc(w, h, &Cell::Coverage(&c)).is_some() {
            placed += 1;
            assert!(placed < 10_000, "the sheet must fill, not grow");
        }
        let (used, total) = s.occupancy();
        assert!(
            used * 10 > total * 9,
            "the shelves should be ~full at refusal, not fragmented away: {used}/{total}"
        );
    }

    #[test]
    fn an_oversized_or_empty_cell_is_refused_for_free() {
        let mut s = sheet();
        let c = cov(4, 4);
        assert!(s.alloc(SHEET_SIZE + 1, 4, &Cell::Coverage(&c)).is_none());
        assert!(s.alloc(4, SHEET_SIZE + 1, &Cell::Coverage(&c)).is_none());
        assert!(s.alloc(0, 0, &Cell::Coverage(&[])).is_none());
        assert_eq!(s.occupancy().0, 0);
        assert!(s.take_pending().1.is_empty());
    }

    /// Announcing the `Image` twice would recreate the texture and erase it.
    #[test]
    fn the_texture_is_announced_for_creation_exactly_once() {
        let mut s = sheet();
        let c = cov(8, 11);
        s.alloc(8, 11, &Cell::Coverage(&c)).expect("room");
        let (announce, uploads) = s.take_pending();
        assert!(announce, "created on the first cell");
        assert_eq!(uploads.len(), 1);
        s.alloc(8, 11, &Cell::Coverage(&c)).expect("room");
        let (announce, uploads) = s.take_pending();
        assert!(!announce, "…and never again");
        assert_eq!(uploads.len(), 1, "but the cell still goes up");
    }

    #[test]
    fn a_reset_frees_the_shelves_and_keeps_the_texture() {
        let mut s = sheet();
        let c = cov(8, 11);
        s.alloc(8, 11, &Cell::Coverage(&c)).expect("room");
        let handle = s.handle();
        assert!(s.take_pending().0);
        assert!(s.occupancy().0 > 0);

        s.reset();
        assert_eq!(s.occupancy().0, 0, "the shelves are free");
        assert_eq!(s.handle(), handle, "the texture survives");
        // No re-announcement: re-creating the image would blank the sheet.
        s.alloc(8, 11, &Cell::Coverage(&c)).expect("room");
        let (announce, uploads) = s.take_pending();
        assert!(!announce, "a reset must not recreate the texture");
        assert_eq!((uploads[0].x, uploads[0].y), (0, 0), "the shelf is reused");
    }
}
