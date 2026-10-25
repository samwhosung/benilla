//! The movie frame's picture: each playing movie's last shown frame, uploaded into its output
//! bitmap ([`benilla_ui::script::MovieBitmap`]) and drawn as the reference's draw (`0x78d1a0`)
//! places its tiles: across the whole window, centred on its height, `A` times the tiles' extent
//! tall, where `A` is the scaled screen aspect (`0x41ad70`, 1.0 at 4:3), blending, lighting and
//! fog off (`0x78d3f7`-`0x78d414`: render states 14, 15 and 7 set to 0). One texture holds the
//! bitmap the reference splits into two or six for its power-of-two tiles (`0x81d280`), which
//! cover it edge to edge.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_ui::script::{movie_bitmap, MovieBitmap};
use benilla_ui::widget::FrameHandle;

use crate::ui_pass::{UiQuad, UvRect};

/// The pictures the paint draws, by movie frame, and the scaled aspect they draw at.
#[derive(Resource, Default)]
pub(crate) struct MovieScreen {
    images: HashMap<FrameHandle, MovieImage>,
    /// `A = [0x832a4c]`, [`crate::loading_screen::scaled_aspect`].
    scaled_aspect: f32,
}

struct MovieImage {
    serial: u64,
    id: u64,
    handle: Handle<Image>,
    bitmap: MovieBitmap,
}

impl MovieScreen {
    /// Frame `frame`'s picture as a quad in a window `w` by `h` logical pixels, y down, at paint
    /// key `z`: the tiles' extent `y_bottom..y_top` times `A`, in units of the window's height
    /// about its middle, and the whole width (`0x78d46a`-`0x78d4d0`).
    pub(super) fn quad(&self, frame: FrameHandle, w: f32, h: f32, z: u128) -> Option<UiQuad> {
        let image = self.images.get(&frame)?;
        let a = self.scaled_aspect;
        let top = h * (0.5 - a * image.bitmap.y_top);
        let bottom = h * (0.5 - a * image.bitmap.y_bottom);
        Some(UiQuad {
            rect: Rect::new(0.0, top, w, bottom),
            z_key: z,
            texture: Some(image.handle.clone()),
            uv: UvRect::FULL,
            color: [1.0; 4],
            blend: benilla_ui::script::BlendMode::Disable,
            ..default()
        })
    }
}

/// `picture`, `width` by `rows` RGBA8 rows top down, into its bitmap as the tiles show it: the
/// bitmap's first row draws at the bottom and the decoder writes the picture bottom-up from it,
/// so the rows past the picture are the top's, black (`0x78c89b` zeroes the bitmap; blending is
/// off, so they draw opaque).
fn fill_bitmap(bitmap: MovieBitmap, picture: &[u8], rows: u32, out: &mut Vec<u8>) {
    let row_bytes = bitmap.width as usize * 4;
    let pad = bitmap.height.saturating_sub(rows) as usize;
    out.clear();
    out.resize(row_bytes * bitmap.height as usize, 0);
    for px in out[..pad * row_bytes].chunks_exact_mut(4) {
        px[3] = 255;
    }
    let shown = (rows.min(bitmap.height) as usize) * row_bytes;
    out[pad * row_bytes..pad * row_bytes + shown].copy_from_slice(&picture[..shown]);
}

/// Upload each playing movie's new picture, drop the pictures of movies that stopped, and take
/// the frame's scaled aspect; a line per hundred frames and one at each movie's end, the
/// instrument a run's log reads the playback off.
pub(super) fn upload_movie_pictures(
    mut hosted_vm: crate::script_calls::HostedVm,
    window: Query<&Window, With<PrimaryWindow>>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    mut screen: ResMut<MovieScreen>,
    mut images: ResMut<Assets<Image>>,
    mut scratch: Local<(Vec<u8>, Vec<u8>)>,
    mut seen: Local<HashMap<u64, (String, i32, u32)>>,
) {
    let size = window
        .single()
        .map_or(Vec2::ZERO, |w| Vec2::new(w.width(), w.height()));
    screen.scaled_aspect = crate::loading_screen::scaled_aspect(
        size,
        crate::loading_screen::widescreen(cvars.as_deref()),
    );
    hosted_vm.run(|script| {
        let pictures = script.as_deref().map(|s| s.movie_pictures()).unwrap_or_default();
        screen
            .images
            .retain(|frame, image| pictures.iter().any(|p| p.frame == *frame && p.id == image.id));
        seen.retain(|id, (name, shown, count)| {
            if pictures.iter().any(|p| p.id == *id) {
                return true;
            }
            info!("movie: {name} stopped at frame {shown}/{count}");
            false
        });
        let Some(script) = script else {
            return;
        };
        let (rgba, bitmap_bytes) = &mut *scratch;
        for p in pictures {
            let name = p.name.clone().unwrap_or_default();
            let last = seen.insert(p.id, (name.clone(), p.shown, p.frame_count));
            if last.is_none() || last.is_some_and(|(_, s, _)| s / 100 != p.shown / 100) {
                info!(
                    "movie: {name} frame {}/{} at {} ms, {} dropped",
                    p.shown, p.frame_count, p.clock_ms, p.dropped
                );
            }
            let Some(bitmap) = movie_bitmap(p.width) else {
                continue;
            };
            if screen
                .images
                .get(&p.frame)
                .is_some_and(|i| i.id == p.id && i.serial == p.serial)
            {
                continue;
            }
            if !script.write_movie_rgba(p.frame, rgba) {
                continue;
            }
            fill_bitmap(bitmap, rgba, p.height, bitmap_bytes);
            let image = benilla_assets::sprite_image(
                bitmap.width,
                bitmap.height,
                std::mem::take(bitmap_bytes),
            );
            match screen.images.get_mut(&p.frame) {
                Some(slot) => {
                    // The same handle, so the painted quads keep their texture.
                    let _ = images.insert(&slot.handle, image);
                    slot.serial = p.serial;
                    slot.id = p.id;
                    slot.bitmap = bitmap;
                }
                None => {
                    screen.images.insert(
                        p.frame,
                        MovieImage {
                            serial: p.serial,
                            id: p.id,
                            handle: images.add(image),
                            bitmap,
                        },
                    );
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The picture's rows fill the bitmap's bottom, the rows above them opaque black.
    #[test]
    fn the_picture_sits_on_the_bitmap_bottom_under_black_rows() {
        let bitmap = movie_bitmap(800).unwrap();
        let picture: Vec<u8> = (0..800 * 368).flat_map(|i| [1, 2, 3, (i % 251) as u8]).collect();
        let mut out = Vec::new();
        fill_bitmap(bitmap, &picture, 368, &mut out);
        assert_eq!(out.len(), 800 * 384 * 4);
        assert!(out[..800 * 16 * 4]
            .chunks_exact(4)
            .all(|px| px == [0, 0, 0, 255]));
        assert_eq!(&out[800 * 16 * 4..], &picture[..]);
    }

    /// At 4:3 the 1024 bitmap is the window's width and two thirds of its height, centred: rows
    /// 128 to 640 of 768, its 512 rows one window pixel each. At 16:9 it grows with `A`.
    #[test]
    fn the_quad_spans_the_window_width_centred_on_its_height() {
        let s = benilla_ui::script::UiScript::new_glue().unwrap();
        s.run(r#"CreateFrame("MovieFrame", "MovieProbe")"#).unwrap();
        let frame = s.frame_handle("MovieProbe").unwrap();
        let mut screen = MovieScreen {
            images: HashMap::new(),
            scaled_aspect: 1.0,
        };
        screen.images.insert(
            frame,
            MovieImage {
                serial: 1,
                id: 1,
                handle: Handle::default(),
                bitmap: movie_bitmap(1024).unwrap(),
            },
        );
        let q = screen.quad(frame, 1024.0, 768.0, 7).unwrap();
        assert_eq!(q.rect.min.x, 0.0);
        assert_eq!(q.rect.max.x, 1024.0);
        assert!((q.rect.min.y - 128.256).abs() < 1e-3, "{:?}", q.rect);
        assert!((q.rect.max.y - 639.744).abs() < 1e-3, "{:?}", q.rect);
        screen.scaled_aspect = crate::loading_screen::scaled_aspect(Vec2::new(1920.0, 1080.0), true);
        let q = screen.quad(frame, 1920.0, 1080.0, 7).unwrap();
        let tall = q.rect.max.y - q.rect.min.y;
        assert!((tall - 0.666 * 1080.0 * 4.0 / 3.0).abs() < 0.01, "{tall}");
        assert!(((q.rect.min.y + q.rect.max.y) / 2.0 - 540.0).abs() < 1e-3);
    }
}
