//! The movie frame's picture: each playing movie's last shown frame, uploaded into its output
//! bitmap ([`benilla_ui::script::MovieBitmap`]) and drawn as the reference's draw (`0x78d1a0`)
//! places its tiles: across the whole window, centred on its height, `A` times the tiles' extent
//! tall, where `A` is the scaled screen aspect (`0x41ad70`, 1.0 at 4:3), blending, lighting and
//! fog off (`0x78d3f7`-`0x78d414`: render states 14, 15 and 7 set to 0). One texture holds the
//! bitmap the reference splits into two or six for its power-of-two tiles (`0x81d280`), which
//! cover it edge to edge. And its audio's feed: the streams the movies ask for go to the sound
//! side ([`crate::sound::MovieAudioRequest`]), and the play times it reports come back.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_ui::script::{movie_bitmap, MovieBitmap};
use benilla_ui::widget::FrameHandle;

use crate::sound::{MovieAudioReports, MovieAudioRequest};
use crate::ui_pass::{UiQuad, UvRect};

/// The audio feed, ahead of the tick whose clock reads it: last frame's stream reports into the
/// movies, then each new movie's stream asked for and each stopped movie's stream let go.
pub(super) fn feed_movie_audio(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut reports: ResMut<MovieAudioReports>,
    mut requests: MessageWriter<MovieAudioRequest>,
    mut held: Local<Vec<u64>>,
) {
    hosted_vm.run(|script| {
        let reports = std::mem::take(&mut reports.0);
        let live = match script {
            Some(script) => {
                for (id, report) in reports {
                    script.set_movie_audio(id, report);
                }
                for start in script.take_movie_audio_starts() {
                    held.push(start.id);
                    requests.write(MovieAudioRequest::Start {
                        id: start.id,
                        mp3: start.mp3,
                    });
                }
                script.movie_audio_ids()
            }
            None => Vec::new(),
        };
        held.retain(|id| {
            let keep = live.contains(id);
            if !keep {
                requests.write(MovieAudioRequest::Stop { id: *id });
            }
            keep
        });
    });
}

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

/// `picture`, `width` by `rows` RGBA8 rows top down, into its bitmap as the tiles show it, top
/// down: the bitmap's first row draws at the bottom, and the decoder writes the picture bottom-up
/// from [`MovieBitmap::picture_row`]; every other row stays black (`0x78c89b` zeroes the bitmap;
/// blending is off, so they draw opaque).
fn fill_bitmap(bitmap: MovieBitmap, picture: &[u8], rows: u32, out: &mut Vec<u8>) {
    let row_bytes = bitmap.width as usize * 4;
    let rows = rows.min(bitmap.height.saturating_sub(bitmap.picture_row));
    let top = (bitmap.height - bitmap.picture_row - rows) as usize * row_bytes;
    let shown = rows as usize * row_bytes;
    out.clear();
    out.resize(row_bytes * bitmap.height as usize, 0);
    for px in out.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    out[top..top + shown].copy_from_slice(&picture[..shown]);
}

/// Upload each playing movie's new picture, drop the pictures of movies that stopped, and take
/// the frame's scaled aspect; a line per hundred frames and one at each movie's end with the glue
/// screen it left up, the instrument a run's log reads the playback off.
pub(super) fn upload_movie_pictures(
    // With the registry, whose callbacks run inside the calls that write it.
    mut vm: crate::script_calls::VmWith<Option<Res<'static, crate::cvars::Cvars>>>,
    window: Query<&Window, With<PrimaryWindow>>,
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
        crate::loading_screen::widescreen(vm.own().as_deref()),
    );
    vm.run(|script| {
        let pictures = script
            .as_deref()
            .map(|s| s.movie_pictures())
            .unwrap_or_default();
        screen.images.retain(|frame, image| {
            pictures
                .iter()
                .any(|p| p.frame == *frame && p.id == image.id)
        });
        let screen_now = script
            .as_deref()
            .map(|s| s.glue_screen())
            .unwrap_or_default();
        seen.retain(|id, (name, shown, count)| {
            if pictures.iter().any(|p| p.id == *id) {
                return true;
            }
            info!("movie: {name} stopped at frame {shown}/{count}, glue screen \"{screen_now}\"");
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

    /// A glue VM whose `MovieProbe` frame plays a 1024-wide movie with audio.
    fn playing_vm() -> benilla_ui::script::UiScript {
        let mut s = benilla_ui::script::UiScript::new_glue().unwrap();
        s.set_movie_reader(Box::new(|path, _| {
            path.ends_with(".avi").then(|| {
                benilla_movie::testing::synthetic_avi(1024, 464, (1, 25), 2, &[b"", b""], &[b"mp3"])
            })
        }));
        s.run(r#"M = CreateFrame("MovieFrame", "MovieProbe") M:StartMovie("A")"#)
            .unwrap();
        s
    }

    /// The feed asks for each new movie's stream once, and lets it go when its movie is gone,
    /// the VM that played it replaced by one that never did.
    #[test]
    fn the_feed_starts_a_movies_stream_and_stops_it_with_its_vm() {
        let mut world = World::new();
        // One schedule, so the feed keeps its own record of the streams it asked for.
        let mut schedule = Schedule::default();
        schedule.add_systems(feed_movie_audio);
        world.init_resource::<MovieAudioReports>();
        world.init_resource::<Messages<MovieAudioRequest>>();
        world.insert_non_send_resource(playing_vm());
        let mut asked = |world: &mut World| -> Vec<String> {
            schedule.run(world);
            world
                .resource_mut::<Messages<MovieAudioRequest>>()
                .drain()
                .map(|r| match r {
                    MovieAudioRequest::Start { mp3, .. } => format!("start {mp3:?}"),
                    MovieAudioRequest::Stop { .. } => "stop".to_string(),
                })
                .collect()
        };
        assert_eq!(asked(&mut world), [format!("start {:?}", b"mp3".to_vec())]);
        assert!(asked(&mut world).is_empty(), "asked once");
        world.insert_non_send_resource(benilla_ui::script::UiScript::new_glue().unwrap());
        assert_eq!(asked(&mut world), ["stop"]);
        assert!(asked(&mut world).is_empty());
    }

    /// The picture starts `0x81d400`'s rows above the bitmap's bottom (19200 bytes, 6 rows of
    /// 800; 98304, 24 rows of 1024) and the rest stays opaque black: 800×368 is texture rows
    /// 10..378 of 384, 1024×464 rows 24..488 of 512.
    #[test]
    fn the_picture_sits_where_the_decoder_writes_it_between_black_rows() {
        for (width, rows, top, bottom) in [(800u32, 368u32, 10usize, 6usize), (1024, 464, 24, 24)] {
            let bitmap = movie_bitmap(width).unwrap();
            let picture: Vec<u8> = (0..width * rows)
                .flat_map(|i| [1, 2, 3, (i % 251) as u8])
                .collect();
            let mut out = Vec::new();
            fill_bitmap(bitmap, &picture, rows, &mut out);
            let row = width as usize * 4;
            assert_eq!(out.len(), row * bitmap.height as usize, "{width}");
            let black = |bytes: &[u8]| {
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|px| *px == [0, 0, 0, 255])
            };
            assert!(black(&out[..top * row]), "{width}: the rows above");
            assert!(
                out[top * row..top * row + picture.len()] == picture[..],
                "{width}: the picture's rows"
            );
            assert!(
                black(&out[out.len() - bottom * row..]),
                "{width}: the rows below"
            );
            assert_eq!(top + rows as usize + bottom, bitmap.height as usize);
        }
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
        screen.scaled_aspect =
            crate::loading_screen::scaled_aspect(Vec2::new(1920.0, 1080.0), true);
        let q = screen.quad(frame, 1920.0, 1080.0, 7).unwrap();
        let tall = q.rect.max.y - q.rect.min.y;
        assert!((tall - 0.666 * 1080.0 * 4.0 / 3.0).abs() < 0.01, "{tall}");
        assert!(((q.rect.min.y + q.rect.max.y) / 2.0 - 540.0).abs() < 1e-3);
    }
}
