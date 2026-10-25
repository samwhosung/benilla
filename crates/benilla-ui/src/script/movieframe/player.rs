//! A playing movie: the decoder, the clock that picks its frame (`0x78cf30`) and its captions.

use benilla_movie::Movie;

use super::captions::Caption;

/// The movie's audio stream as the host reports it: the reference opens it inside `StartMovie`
/// (`0x78ca30`), the host one frame later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Audio {
    /// No stream: the file has no audio, or it would not open or play, so the movie runs on the
    /// wall clock (`0x78ca7f`, the handle `+0x3b0` left 0).
    None,
    /// Asked of the host, not yet answered: a stream that has played nothing, so the clock waits
    /// (`0x78cf5d`).
    Pending,
    /// Playing, at this play time (`FSOUND_Stream_GetTime`, `0x7fdaa2`).
    At(u32),
}

/// What one clock step did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Nothing new to show: the audio has not started, or the frame is the one shown.
    Hold,
    /// The movie is over: its last frame passed, its audio ended or a shown frame failed.
    Finish,
    /// A new frame was decoded and shown.
    Shown,
}

/// The playing state `StartMovie` builds and `StopMovie` tears down, the fields of the frame from
/// `+0x318` on that the clock reads.
#[derive(Clone)]
pub(crate) struct Playback {
    /// The start's number, by which the host keys the audio stream it holds for it.
    pub(crate) id: u64,
    pub(crate) movie: Movie,
    pub(crate) audio: Audio,
    /// `+0x318`: set by the clock at the last frame, by the audio's end callback (`0x78c070`) and
    /// by a shown frame's failure (`0x78d0c0`); the next step stops the movie.
    pub(crate) finished: bool,
    /// `+0x3b4`: the wall time the clock counts from, 0 until the first step anchors it.
    base: u32,
    /// `+0x3c8`: the movie clock in milliseconds.
    pub(crate) clock: u32,
    /// `+0x3b8`: the frame shown, -1 before the first.
    pub(crate) shown: i32,
    /// `+0x3bc`: the frame the last step aimed at.
    pub(crate) target: i32,
    /// `+0x3c4`: the frames the audio clock has pulled the wall clock by.
    pub(crate) offset: i32,
    /// `+0x3c0`: the steps that skipped frames.
    pub(crate) dropped: u32,
    pub(crate) captions: Vec<Caption>,
    /// `+0x3dc`: the caption shown, `None` for -1.
    pub(crate) caption: Option<usize>,
    /// Frames shown, so the host uploads each picture once.
    pub(crate) serial: u64,
}

/// The x87's `fmul dword [0x801360]`: 0.001 as a single.
const MS_TO_S: f32 = 0.001;

impl Playback {
    pub(crate) fn new(id: u64, movie: Movie, audio: Audio, captions: Vec<Caption>) -> Self {
        // The decoder start's resets (`0x78c95f`-`0x78c975`) and the audio start's (`0x78ca83`,
        // `0x78ca89`); the loader leaves no caption shown (`0x78cd86`).
        Self {
            id,
            movie,
            audio,
            finished: false,
            base: 0,
            clock: 0,
            shown: -1,
            target: 0,
            offset: 0,
            dropped: 0,
            captions,
            caption: None,
            serial: 0,
        }
    }

    /// The frame at `ms` on a clock: `trunc(ms · rate · 0.001 + 0.5)` at the x87's 53 bits, the
    /// clock loaded as an unsigned 64-bit integer (`0x78cf9c`-`0x78cfb1`).
    fn frame_at(&self, ms: u32) -> i32 {
        let rate = f64::from(self.movie.rate());
        (f64::from(ms) * rate * f64::from(MS_TO_S) + 0.5) as i64 as i32
    }

    /// One step of the clock (`0x78cf30`), at the wall time `now` in milliseconds (`0x42c010`).
    ///
    /// The first step anchors the clock: to the audio's play time, waiting while that is 0, or
    /// with no audio to now. Each later one reads the wall clock since the anchor, and with audio
    /// moves the offset one frame toward the audio's frame when the two differ by more than two
    /// frames, or by two while the frame would not move. The target is the clock's frame plus the
    /// offset, never behind the frame shown; at the frame count, or with the finished flag set,
    /// the movie finishes. A target past the next frame decodes the frames between without
    /// showing them, then shows the target.
    pub(crate) fn step(&mut self, now: u32) -> Step {
        let audio = match self.audio {
            Audio::None => None,
            Audio::Pending => Some(0),
            Audio::At(t) => Some(t),
        };
        if self.base == 0 {
            if let Some(t) = audio {
                self.clock = t;
                if t == 0 {
                    return Step::Hold;
                }
            }
            self.base = now.wrapping_sub(self.clock);
        } else {
            self.clock = now.wrapping_sub(self.base);
        }
        let wall = self.frame_at(self.clock);
        if let Some(t) = audio {
            let d = self
                .frame_at(t)
                .wrapping_sub(self.offset)
                .wrapping_sub(wall);
            let gap = d.wrapping_abs();
            if gap > 2 || (gap == 2 && self.offset.wrapping_add(wall) == self.shown) {
                self.offset += if d > 0 { 1 } else { -1 };
            }
        }
        let target = self.offset.wrapping_add(wall).max(self.shown);
        self.target = target;
        if target >= self.movie.frame_count() as i32 {
            self.finished = true;
        }
        if self.finished {
            return Step::Finish;
        }
        if target == self.shown {
            return Step::Hold;
        }
        if target != self.shown + 1 {
            self.dropped += 1;
        }
        // `0x78d090`: each frame up to the one before the target, decoded and not shown.
        while self.shown < target - 1 {
            let _ = self.movie.decode_next();
            self.shown += 1;
        }
        // A frame past the file's index fails as a decoder error does.
        if matches!(self.movie.decode_next(), Ok(true)) {
            self.serial += 1;
        } else {
            self.finished = true;
        }
        self.shown = target;
        Step::Shown
    }

    /// The caption step after a shown frame (`0x78d0ca`-`0x78d10d`): the caption up whose end the
    /// clock has reached comes down. Whether it did.
    pub(crate) fn caption_ended(&mut self) -> bool {
        match self.caption {
            Some(i) if self.captions.get(i).is_none_or(|c| c.end <= self.clock) => {
                self.caption = None;
                true
            }
            _ => false,
        }
    }

    /// The next caption step (`0x78d117`-`0x78d191`): with none up, and subtitles enabled or no
    /// audio stream, the first caption whose range holds the clock goes up. Its text.
    pub(crate) fn caption_to_show(&mut self, subtitles: bool) -> Option<Vec<u8>> {
        if self.caption.is_some() || !(subtitles || self.audio == Audio::None) {
            return None;
        }
        let clock = self.clock;
        let i = self
            .captions
            .iter()
            .position(|c| c.start <= clock && clock < c.end)?;
        self.caption = Some(i);
        Some(self.captions[i].text.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1024-wide movie of `frames` empty chunks (each a repeat of the picture before it) at
    /// 25 frames a second, its header's frame count `count`.
    fn movie(frames: usize, count: u32) -> Movie {
        let empty: Vec<&[u8]> = vec![b""; frames];
        let file = benilla_movie::testing::synthetic_avi(1024, 464, (1, 25), count, &empty, &[]);
        Movie::open(file).expect("synthetic movie")
    }

    fn playback(audio: Audio) -> Playback {
        Playback::new(1, movie(100, 100), audio, Vec::new())
    }

    /// The wall clock alone: the first step anchors at now and shows frame 0; a step on the same
    /// frame holds; a late step decodes the frames it passed without counting them shown, and
    /// counts one drop; the frame is `trunc(ms · 25 · 0.001 + 0.5)`, so 20 ms is frame 1.
    #[test]
    fn the_wall_clock_picks_the_rounded_frame_and_skips_what_it_passed() {
        let mut p = playback(Audio::None);
        assert_eq!(p.step(5000), Step::Shown);
        assert_eq!((p.shown, p.serial, p.movie.next_frame()), (0, 1, 1));
        assert_eq!(p.step(5019), Step::Hold, "19 ms rounds to frame 0");
        assert_eq!(p.step(5020), Step::Shown, "20 ms is half a frame: frame 1");
        assert_eq!((p.shown, p.dropped), (1, 0));
        // 300 ms is frame 8 (7.5 rounded up): frames 2..=7 decoded unshown, 8 shown.
        assert_eq!(p.step(5300), Step::Shown);
        assert_eq!(
            (p.shown, p.serial, p.movie.next_frame(), p.dropped),
            (8, 3, 9, 1)
        );
        assert_eq!(p.clock, 300);
    }

    /// The finish: the step whose frame reaches the header's frame count finishes without
    /// decoding; so does a finished flag the audio set, whatever the clock says.
    #[test]
    fn the_frame_count_and_the_audio_end_finish_the_movie() {
        let mut p = playback(Audio::None);
        assert_eq!(p.step(1000), Step::Shown);
        // 99 frames on is 3960 ms; frame 100 is 3980 ms.
        assert_eq!(p.step(1000 + 3960), Step::Shown);
        assert_eq!(p.shown, 99);
        assert_eq!(p.step(1000 + 3979), Step::Hold);
        assert_eq!(p.step(1000 + 3980), Step::Finish);
        assert_eq!((p.target, p.shown), (100, 99));

        let mut p = playback(Audio::At(40));
        assert_eq!(p.step(1000), Step::Shown);
        p.finished = true;
        assert_eq!(p.step(1000), Step::Finish);
    }

    /// The header's count rules, not the index: a count past the indexed frames fails the first
    /// frame past them, which finishes the movie at the next step.
    #[test]
    fn a_frame_past_the_index_fails_and_finishes() {
        let mut p = Playback::new(1, movie(2, 10), Audio::None, Vec::new());
        assert_eq!(p.step(1000), Step::Shown);
        assert_eq!(p.step(1040), Step::Shown);
        assert_eq!((p.shown, p.serial, p.finished), (1, 2, false));
        assert_eq!(p.step(1080), Step::Shown);
        assert_eq!((p.shown, p.serial, p.finished), (2, 2, true));
        assert_eq!(p.step(1080), Step::Finish);
    }

    /// With audio the clock waits for the stream's first played millisecond and anchors to it;
    /// then the audio clock pulls the wall clock one frame a step when the two part by more than
    /// two frames, or by exactly two while the frame stands still.
    #[test]
    fn the_audio_anchors_the_clock_and_nudges_it_a_frame_at_a_time() {
        let mut p = playback(Audio::Pending);
        assert_eq!(p.step(5000), Step::Hold, "a stream that has played nothing");
        p.audio = Audio::At(0);
        assert_eq!(p.step(5010), Step::Hold);
        p.audio = Audio::At(400);
        // Anchored at 400 ms of audio: frame 10.
        assert_eq!(p.step(6000), Step::Shown);
        assert_eq!((p.clock, p.shown, p.offset), (400, 10, 0));
        // The wall clock 40 ms on (frame 11), the audio three frames ahead of it (frame 14):
        // the offset moves one frame.
        p.audio = Audio::At(560);
        assert_eq!(p.step(6040), Step::Shown);
        assert_eq!((p.clock, p.offset, p.shown), (440, 1, 12));
        // Two frames apart while the target moves: no nudge.
        p.audio = Audio::At(560);
        assert_eq!(p.step(6080), Step::Shown);
        assert_eq!((p.offset, p.shown), (1, 13));
        // Two apart and the target the frame shown: the nudge, back toward the audio.
        p.audio = Audio::At(440);
        assert_eq!(p.step(6080), Step::Hold);
        assert_eq!(
            p.offset, 0,
            "the audio frame 11 is two behind the target 13"
        );
        // The target never falls behind the frame shown.
        assert_eq!((p.target, p.shown), (13, 13));
    }

    /// A caption comes down at its end, and the one that holds the clock goes up only with
    /// subtitles enabled or no audio.
    #[test]
    fn captions_follow_the_clock_and_the_subtitle_switch() {
        let captions = vec![
            Caption {
                start: 100,
                end: 200,
                text: b"one".to_vec(),
            },
            Caption {
                start: 200,
                end: 300,
                text: b"two".to_vec(),
            },
        ];
        let mut p = Playback::new(1, movie(100, 100), Audio::None, captions.clone());
        p.step(1000);
        assert_eq!(p.caption_to_show(false), None, "the clock is at 0");
        p.step(1100);
        assert!(!p.caption_ended());
        assert_eq!(p.caption_to_show(false).as_deref(), Some(&b"one"[..]));
        assert_eq!(p.caption_to_show(false), None, "one is up");
        p.step(1199);
        assert!(!p.caption_ended(), "199 ms is inside the first");
        p.step(1200);
        assert!(p.caption_ended());
        assert_eq!(p.caption_to_show(false).as_deref(), Some(&b"two"[..]));

        let mut p = Playback::new(1, movie(100, 100), Audio::At(100), captions);
        p.step(1000);
        assert_eq!(p.caption_to_show(false), None, "audio and subtitles off");
        assert_eq!(p.caption_to_show(true).as_deref(), Some(&b"one"[..]));
    }
}
