//! The `MovieFrame` (`CSimpleMovieFrame`, factory `0x6ef000`; method table `0x87ab4c`) and its
//! player: `StartMovie` (`0x78d770`), `StopMovie` (`0x78d870`) and `EnableSubtitles`
//! (`0x78d940`), the clock its draw callback steps (`0x78d680` → `0x78cf30`), and the three
//! `OnMovie*` scripts it fires (script map `0x78c2e0`, slots `+0x320`/`+0x328`/`+0x330`).
//!
//! `StartMovie` reads `<name>.avi` whole and starts the decoder (`0x78c360`, `0x78c7a0`); either
//! failing answers nil. Then it starts the audio (`0x78ca30`) and loads `<name>.sbt`'s captions
//! (`0x78cb00`), whose results it ignores, sets the playing flag (`+0x314`) and answers 1
//! (`0x78ce40`). The host plays the audio: it takes each new stream's MP3 bytes
//! ([`UiScript::take_movie_audio_starts`]) and reports its play time or its failure back
//! ([`UiScript::set_movie_audio`]) ahead of each tick, and the movie's draw is the host's too
//! ([`UiScript::movie_pictures`]). A movie stops at its last frame, at its audio's end, or by
//! `StopMovie`, each through the one teardown (`0x78ced0`) that fires `OnMovieFinished`.

mod captions;
mod player;

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Table, Value};

use super::binding_abi::{flag_or_default, name_of, optional_string};
use super::event;
use super::object::frame_handle_of;
use super::{Model, UiScript};
use crate::widget::{FrameHandle, FrameKind};

use player::{Audio, Playback, Step};

/// Registry key of the MovieFrame method table (the MAXCSTACK discipline: a Lua-side root).
pub(super) const REG_MOVIEFRAME_METHODS: &str = "__benilla_movieframe_methods";

/// How a movie's file opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovieOpen {
    /// The loose files by the whole name, then the archives: the `.avi`, which `0x78c39f` opens
    /// through `SFile::OpenEx` with flag 1.
    LooseFirst,
    /// The archives alone: the `.sbt`, which `SFile::LoadByName` (`0x6485f0`) opens with flags 0.
    Archives,
}

/// The host's file reader for the movie files: a path's bytes, or `None` when nothing opens it.
pub type MovieReadFn = dyn Fn(&str, MovieOpen) -> Option<Vec<u8>>;

/// [`MovieReadFn`] as the model holds it.
pub type MovieReader = std::rc::Rc<MovieReadFn>;

/// A new movie's audio, for the host to open as a stream and play (`0x78ca30`).
pub struct MovieAudioStart {
    /// The movie's number, which [`UiScript::set_movie_audio`] answers under.
    pub id: u64,
    /// The AVI's audio chunks, concatenated: an MP3 stream for the 1.12.1 movies.
    pub mp3: Vec<u8>,
}

/// What the host reports of a movie's audio stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovieAudio {
    /// The stream would not open or play: the movie runs on the wall clock (`0x78ca91`).
    Failed,
    /// The stream's play time in milliseconds (`FSOUND_Stream_GetTime`).
    At(u32),
    /// The stream played out: its end callback finishes the movie (`0x78c070`).
    Ended,
}

/// A playing movie's last shown picture and its clock, for the host's draw and its instruments.
#[derive(Clone, Debug, PartialEq)]
pub struct MoviePicture {
    pub frame: FrameHandle,
    /// The movie frame's global name.
    pub name: Option<String>,
    /// The movie's number ([`MovieAudioStart::id`]).
    pub id: u64,
    /// The pictures shown so far: a new one means a new picture to upload.
    pub serial: u64,
    pub width: u32,
    pub height: u32,
    /// The frame shown (`+0x3b8`) and the header's frame count (`+0x348`).
    pub shown: i32,
    pub frame_count: u32,
    /// The steps that skipped frames (`+0x3c0`).
    pub dropped: u32,
    /// The movie clock in milliseconds (`+0x3c8`).
    pub clock_ms: u32,
}

/// The output bitmap and its tiles' quad as the decoder start lays them out for a width (the
/// size class at `+0x34c`, `0x78c815`-`0x78c833`): the bitmap's size (`0x81d1ac`, `0x81d1b4`), and
/// the extent the tile quads (`0x81d1c0`) cover, in the draw's orthographic units (`0x78d321`:
/// x from 0 to 1 across the window, y from -0.5 to 0.5 up it), y before the scaled screen aspect
/// multiplies it (`0x41ad70`, `0x78d480`). The bitmap's first row draws at the bottom
/// (`0x78d46a`-`0x78d4d0`), so the decoder's bottom-up picture stands upright on it, and the rows
/// around the picture stay black (`0x78c89b`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovieBitmap {
    pub width: u32,
    pub height: u32,
    /// The bitmap row the decoder's output starts at, counted from its first: the frame decode
    /// hands `DivxDecode` the bitmap plus `0x81d400`'s bytes for the size class (`0x78d58e`),
    /// 19200 (6 rows of 800) and 98304 (24 rows of 1024), as the tiles' update rects at
    /// `0x81d340` start at row 6 and 24.
    pub picture_row: u32,
    pub y_bottom: f32,
    pub y_top: f32,
}

/// [`MovieBitmap`] for a movie `width` wide: 800 (six tiles, 0.66 of a unit tall) or 1024 (two
/// tiles, 0.666).
pub fn movie_bitmap(width: u32) -> Option<MovieBitmap> {
    match width {
        800 => Some(MovieBitmap {
            width: 800,
            height: 384,
            picture_row: 6,
            y_bottom: -0.33,
            y_top: 0.33,
        }),
        1024 => Some(MovieBitmap {
            width: 1024,
            height: 512,
            picture_row: 24,
            y_bottom: -0.333,
            y_top: 0.333,
        }),
        _ => None,
    }
}

/// The movie frames' player state: the host's reader and, per frame, the subtitle flag and the
/// playing movie.
#[derive(Clone, Default)]
pub(crate) struct Movies {
    reader: Option<MovieReader>,
    frames: HashMap<FrameHandle, MovieFrameState>,
}

/// Hands out each start's number: process-wide, so a stream the host still holds for a VM that
/// is gone never shares a number with a movie of the VM that replaced it.
static NEXT_MOVIE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Clone, Default)]
struct MovieFrameState {
    /// `+0x31c`, `EnableSubtitles`' flag as `GetScriptFlagArg` gave it; 0 from the constructor
    /// (`0x78c0ee`).
    subtitles: i32,
    playback: Option<Playback>,
    /// The playing movie's audio has been handed to the host.
    audio_asked: bool,
}

/// `SStrPrintf` into the open's `0x104`-byte path buffer (`0x78c389`, `0x78cb21`): at most 259
/// bytes.
fn movie_path(name: &str, ext: &str) -> String {
    let mut path = format!("{name}.{ext}").into_bytes();
    path.truncate(0x103);
    String::from_utf8_lossy(&path).into_owned()
}

impl Model {
    /// `0x78ce40`: start the movie `name` on frame `h`. Whether it started.
    fn start_movie(&mut self, h: FrameHandle, name: &str) -> bool {
        let Some(reader) = self.movies.reader.clone() else {
            return false;
        };
        let Some(file) = reader(&movie_path(name, "avi"), MovieOpen::LooseFirst) else {
            return false;
        };
        let Ok(movie) = benilla_movie::Movie::open(file) else {
            return false;
        };
        // An AVI with no audio chunks opens no stream: `FSOUND_Stream_Open` of an empty buffer.
        let audio = if movie.audio().is_empty() {
            Audio::None
        } else {
            Audio::Pending
        };
        let captions = reader(&movie_path(name, "sbt"), MovieOpen::Archives)
            .map(|f| captions::parse(&f))
            .unwrap_or_default();
        let id = NEXT_MOVIE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let state = self.movies.frames.entry(h).or_default();
        state.playback = Some(Playback::new(id, movie, audio, captions));
        state.audio_asked = false;
        true
    }

    /// `0x78ced0`'s teardown on frame `h`: nothing unless a movie plays; else the decoder, the
    /// audio and the captions go, and the playing and finished flags clear. Whether a caption
    /// was up, which the caption free hides (`0x78cda0`), or `None` when nothing played.
    fn take_movie(&mut self, h: FrameHandle) -> Option<bool> {
        let state = self.movies.frames.get_mut(&h)?;
        let playback = state.playback.take()?;
        state.audio_asked = false;
        Some(playback.caption.is_some())
    }

    /// Frame `h` draws a movie picture: a movie plays there and has shown one.
    pub(crate) fn movie_drawn(&self, h: FrameHandle) -> bool {
        self.movies
            .frames
            .get(&h)
            .and_then(|s| s.playback.as_ref())
            .is_some_and(|p| p.shown >= 0)
    }
}

/// Raises unless `this` is a live MovieFrame: the method table is a plain Lua value a caller can
/// lift onto another frame (`0x847e98`, "Wrong object type for member function").
fn movie_frame_of(lua: &Lua, this: &Table) -> mlua::Result<FrameHandle> {
    let h = frame_handle_of(lua, this)?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    match model.arena.frame(h) {
        Some(f) if f.kind == FrameKind::MovieFrame => Ok(h),
        _ => Err(mlua::Error::runtime(
            "Wrong object type for member function",
        )),
    }
}

/// Fire `script` on frame `h` with `args`; a handler error goes to [`Model::errors`].
fn fire(lua: &Lua, h: FrameHandle, script: &str, args: Vec<Value>) {
    let id = lua
        .app_data_mut::<Model>()
        .expect("model app_data")
        .frame_id(h);
    if let Err(e) = event::fire_widget_handler(lua, id, script, args) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_lua_error(&e);
    }
}

/// `0x78ced0` on frame `h`: the teardown, the caption's `OnMovieHideSubtitle` if one was up, and
/// `OnMovieFinished` (`0x78cf1e`). Nothing when no movie plays (`0x78cedb`).
fn stop_movie(lua: &Lua, h: FrameHandle) {
    let caption_up = lua
        .app_data_mut::<Model>()
        .expect("model app_data")
        .take_movie(h);
    let Some(caption_up) = caption_up else {
        return;
    };
    if caption_up {
        fire(lua, h, "OnMovieHideSubtitle", Vec::new());
    }
    fire(lua, h, "OnMovieFinished", Vec::new());
}

/// The draw callback's step on frame `h` (`0x78d680`): the clock, then after a shown frame the
/// caption that ended comes down and the one the clock reached goes up (`0x78d0ca`-`0x78d191`).
fn step_movie(lua: &Lua, h: FrameHandle) {
    let (step, id, hide) = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let now = model.now_ms() as u32;
        let Some(p) = model
            .movies
            .frames
            .get_mut(&h)
            .and_then(|s| s.playback.as_mut())
        else {
            return;
        };
        let step = p.step(now);
        (step, p.id, step == Step::Shown && p.caption_ended())
    };
    match step {
        Step::Hold => {}
        Step::Finish => stop_movie(lua, h),
        Step::Shown => {
            if hide {
                fire(lua, h, "OnMovieHideSubtitle", Vec::new());
            }
            let show = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let Some(state) = model.movies.frames.get_mut(&h) else {
                    return;
                };
                let subtitles = state.subtitles != 0;
                match state.playback.as_mut() {
                    // A movie a handler started in this one's place has its own clock.
                    Some(p) if p.id == id => p.caption_to_show(subtitles),
                    _ => None,
                }
            };
            if let Some(text) = show {
                match lua.create_string(&text) {
                    Ok(s) => fire(lua, h, "OnMovieShowSubtitle", vec![Value::String(s)]),
                    Err(e) => lua
                        .app_data_mut::<Model>()
                        .expect("model app_data")
                        .record_lua_error(&e),
                }
            }
        }
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // `lua_isstring` on the path (`0x6f3510`: a string or a number), else
    // `Usage: %s:StartMovie("filename")` (`0x87ab8c`); one value back, 1 for a movie that
    // started (`0x78d84d`) and nil for one whose file or decoder failed (`0x78d861`).
    m.set(
        "StartMovie",
        lua.create_function(|lua, (this, path): (Table, Value)| {
            let h = movie_frame_of(lua, &this)?;
            let Some(name) = optional_string(lua, &path) else {
                return Err(mlua::Error::runtime(format!(
                    "Usage: {}:StartMovie(\"filename\")",
                    name_of(lua, &this)
                )));
            };
            let started = lua
                .app_data_mut::<Model>()
                .expect("model app_data")
                .start_movie(h, &name);
            Ok(if started {
                Value::Number(1.0)
            } else {
                Value::Nil
            })
        })?,
    )?;
    // No arguments, no returns (`0x78d8b8`).
    m.set(
        "StopMovie",
        lua.create_function(|lua, this: Table| {
            let h = movie_frame_of(lua, &this)?;
            stop_movie(lua, h);
            Ok(())
        })?,
    )?;
    // `GetScriptFlagArg` with the default 1 (`0x78d9c5`-`0x78d9ce`), so an absent flag is on and
    // an explicit nil off, stored as given at `+0x31c` (`0x78d9d3`); no returns.
    m.set(
        "EnableSubtitles",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            let h = movie_frame_of(lua, &this)?;
            let flag = flag_or_default(args.front(), 1);
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .movies
                .frames
                .entry(h)
                .or_default()
                .subtitles = flag;
            Ok(())
        })?,
    )?;

    lua.set_named_registry_value(REG_MOVIEFRAME_METHODS, m)?;
    Ok(())
}

impl UiScript {
    /// Install the host's reader for the movie files.
    pub fn set_movie_reader(&mut self, reader: Box<MovieReadFn>) {
        self.model_mut().movies.reader = Some(reader.into());
    }

    /// A movie plays on some frame: the word `0x885604` the movie start sets (`0x78ce8b`) and the
    /// teardown clears (`0x78cf06`), under which the frame pass presents nothing and a screenshot
    /// asked for waits (`0x442681`).
    pub fn movie_playing(&self) -> bool {
        self.model_ref()
            .movies
            .frames
            .values()
            .any(|s| s.playback.is_some())
    }

    /// The audio of each movie started since the last call, for the host to open and play.
    pub fn take_movie_audio_starts(&mut self) -> Vec<MovieAudioStart> {
        let mut model = self.model_mut();
        let mut out = Vec::new();
        for state in model.movies.frames.values_mut() {
            let Some(p) = state.playback.as_ref() else {
                continue;
            };
            if p.audio == Audio::Pending && !state.audio_asked {
                state.audio_asked = true;
                out.push(MovieAudioStart {
                    id: p.id,
                    mp3: p.movie.audio().to_vec(),
                });
            }
        }
        out.sort_by_key(|s| s.id);
        out
    }

    /// The movies whose audio stream the host should hold: every other stream it holds belongs
    /// to a movie that stopped, and stops with it (`0x78caa0`).
    pub fn movie_audio_ids(&self) -> Vec<u64> {
        let model = self.model_ref();
        let mut ids: Vec<u64> = model
            .movies
            .frames
            .values()
            .filter_map(|s| s.playback.as_ref())
            .filter(|p| p.audio != Audio::None)
            .map(|p| p.id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// The host's report on movie `id`'s audio stream.
    pub fn set_movie_audio(&mut self, id: u64, report: MovieAudio) {
        let mut model = self.model_mut();
        let Some(p) = model
            .movies
            .frames
            .values_mut()
            .filter_map(|s| s.playback.as_mut())
            .find(|p| p.id == id)
        else {
            return;
        };
        match report {
            MovieAudio::Failed => p.audio = Audio::None,
            MovieAudio::At(ms) => p.audio = Audio::At(ms),
            MovieAudio::Ended => {
                p.finished = true;
                // A stream that ended having played nothing gives the clock no time to anchor
                // to, where `0x78cf5d` waits on it: it counts as no stream, so the clock runs
                // and the flag stops the movie.
                if !matches!(p.audio, Audio::At(t) if t > 0) {
                    p.audio = Audio::None;
                }
            }
        }
    }

    /// Each playing movie that has shown a picture, by start.
    pub fn movie_pictures(&self) -> Vec<MoviePicture> {
        let model = self.model_ref();
        let mut out: Vec<MoviePicture> = model
            .movies
            .frames
            .iter()
            .filter_map(|(&h, s)| {
                let p = s.playback.as_ref().filter(|p| p.shown >= 0)?;
                Some(MoviePicture {
                    frame: h,
                    name: model.arena.frame(h).and_then(|f| f.name.clone()),
                    id: p.id,
                    serial: p.serial,
                    width: p.movie.width(),
                    height: p.movie.height(),
                    shown: p.shown,
                    frame_count: p.movie.frame_count(),
                    dropped: p.dropped,
                    clock_ms: p.clock,
                })
            })
            .collect();
        out.sort_by_key(|p| p.id);
        out
    }

    /// The last picture frame `h`'s movie decoded, as RGBA8 rows top down, into `out`; false when
    /// no movie plays there.
    pub fn write_movie_rgba(&self, h: FrameHandle, out: &mut Vec<u8>) -> bool {
        let model = self.model_ref();
        match model
            .movies
            .frames
            .get(&h)
            .and_then(|s| s.playback.as_ref())
        {
            Some(p) => {
                p.movie.write_rgba(out);
                true
            }
            None => false,
        }
    }

    /// The frames' destructor's teardown (`0x78c280` calls `0x78ced0`), as the VM goes: each
    /// playing movie stops and fires its `OnMovieFinished`.
    pub(super) fn stop_movies(&mut self) {
        let mut frames: Vec<FrameHandle> = {
            let model = self.model_ref();
            model
                .movies
                .frames
                .iter()
                .filter(|(_, s)| s.playback.is_some())
                .map(|(&h, _)| h)
                .collect()
        };
        frames.sort_by_key(|h| self.model_ref().arena.frame(*h).map(|f| f.insertion_seq));
        for h in frames {
            stop_movie(&self.lua, h);
        }
    }

    /// The draw pass's movie callbacks (`0x78d650` registers `0x78d680` on layer 2): each visible
    /// movie frame that plays steps its clock, in the strata, level and creation order the frames
    /// draw in.
    pub(super) fn paint_movies(&mut self) {
        let frames: Vec<FrameHandle> = {
            let model = self.model_ref();
            let mut frames: Vec<(u8, u16, u32, FrameHandle)> = model
                .movies
                .frames
                .iter()
                .filter(|(_, s)| s.playback.is_some())
                .filter_map(|(&h, _)| {
                    let f = model.arena.frame(h)?;
                    f.effective_visible
                        .then(|| (f.strata.index(), f.level, f.insertion_seq, h))
                })
                .collect();
            frames.sort_unstable_by_key(|&(strata, level, seq, _)| (strata, level, seq));
            frames.into_iter().map(|(_, _, _, h)| h).collect()
        };
        for h in frames {
            step_movie(&self.lua, h);
        }
    }
}

#[cfg(test)]
mod tests;
