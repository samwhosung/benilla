use std::cell::RefCell;
use std::rc::Rc;

use crate::script::{MovieAudio, MovieOpen, QuadContent, UiScript};

/// Every path the reader was asked for, and how it opened.
type Asked = Rc<RefCell<Vec<(String, MovieOpen)>>>;

/// A 1024-wide movie of `frames` empty chunks at 25 frames a second, `sounds` its audio chunks.
fn avi(frames: u32, sounds: &[&[u8]]) -> Vec<u8> {
    let empty: Vec<&[u8]> = vec![b""; frames as usize];
    benilla_movie::testing::synthetic_avi(1024, 464, (1, 25), frames, &empty, sounds)
}

/// A glue VM whose reader answers `files` by exact path and logs every ask, with `M` a movie
/// frame whose three scripts append to the Lua list `log`.
fn movie_vm(files: Vec<(&'static str, Vec<u8>)>) -> (UiScript, Asked) {
    let mut s = UiScript::new_glue().unwrap();
    let asked = Rc::new(RefCell::new(Vec::new()));
    let seen = asked.clone();
    s.set_movie_reader(Box::new(move |path, open| {
        seen.borrow_mut().push((path.to_string(), open));
        files
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| bytes.clone())
    }));
    s.run(
        r#"log = {}
        M = CreateFrame("MovieFrame", "MovieProbe")
        M:SetScript("OnMovieFinished", function() table.insert(log, "finished") end)
        M:SetScript("OnMovieShowSubtitle", function() table.insert(log, "show:" .. arg1) end)
        M:SetScript("OnMovieHideSubtitle", function() table.insert(log, "hide") end)"#,
    )
    .unwrap();
    s.set_now(8.0);
    (s, asked)
}

fn log(s: &UiScript) -> String {
    s.eval::<String>("return table.concat(log, ' ')").unwrap()
}

fn shown(s: &UiScript) -> Option<i32> {
    s.movie_pictures().first().map(|p| p.shown)
}

/// `StartMovie` answers 1 for a movie whose file and decoder start, and nil when the `.avi` is
/// missing, is no AVI, or is a width the decoder refuses (`0x78ce52`, `0x78ce64`); a missing
/// `.sbt` is ignored (`0x78ce77`). The `.avi` opens loose first and the `.sbt` from the archives,
/// each the name with its extension.
#[test]
fn start_movie_answers_1_for_a_movie_that_starts_and_nil_otherwise() {
    let narrow = benilla_movie::testing::synthetic_avi(640, 480, (1, 25), 1, &[b""], &[]);
    let (s, asked) = movie_vm(vec![
        ("Interface\\Cinematics\\Good.avi", avi(4, &[])),
        ("Bad.avi", b"RIFF\0\0\0\0WAVE".to_vec()),
        ("Narrow.avi", narrow),
    ]);
    assert!(s
        .eval::<bool>(r#"return M:StartMovie("Missing") == nil"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"return M:StartMovie("Bad") == nil"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"return M:StartMovie("Narrow") == nil"#)
        .unwrap());
    assert!(!s.movie_playing(), "no failed start plays");
    assert!(s
        .eval::<bool>(r#"return M:StartMovie("Interface\\Cinematics\\Good") == 1"#)
        .unwrap());
    assert!(s.movie_playing());
    assert_eq!(
        asked.borrow().clone(),
        [
            ("Missing.avi".to_string(), MovieOpen::LooseFirst),
            ("Bad.avi".to_string(), MovieOpen::LooseFirst),
            ("Narrow.avi".to_string(), MovieOpen::LooseFirst),
            (
                "Interface\\Cinematics\\Good.avi".to_string(),
                MovieOpen::LooseFirst
            ),
            (
                "Interface\\Cinematics\\Good.sbt".to_string(),
                MovieOpen::Archives
            ),
        ]
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The usage line for anything but a string or a number, the methods on no other frame type, and
/// the three scripts kept by `SetScript`.
#[test]
fn the_movie_methods_check_their_arguments_and_their_frame() {
    let (s, _) = movie_vm(Vec::new());
    assert!(s.eval::<bool>("return M:StartMovie(7) == nil").unwrap());
    assert!(s
        .eval::<bool>("return table.getn({M:StopMovie()}) == 0")
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return table.getn({M:EnableSubtitles(1)}) + table.getn({M:EnableSubtitles()}) == 0"
        )
        .unwrap());
    for bad in ["M:StartMovie()", "M:StartMovie({})", "M:StartMovie(true)"] {
        let err = s.run(bad).unwrap_err().to_string();
        assert!(
            err.contains(r#"Usage: MovieProbe:StartMovie("filename")"#),
            "{bad}: {err}"
        );
    }
    for name in [
        "OnMovieFinished",
        "OnMovieShowSubtitle",
        "OnMovieHideSubtitle",
    ] {
        assert!(
            s.eval::<bool>(&format!("return M:GetScript('{name}') ~= nil"))
                .unwrap(),
            "{name}"
        );
    }
    s.run(r#"P = CreateFrame("Frame", "NotAMovie")"#).unwrap();
    assert!(s.eval::<bool>("return P.StartMovie == nil").unwrap());
    let err = s.run("M.StopMovie(P)").unwrap_err().to_string();
    assert!(
        err.contains("Wrong object type for member function"),
        "{err}"
    );
    assert_eq!(
        log(&s),
        "",
        "a StopMovie with nothing playing fires nothing"
    );
}

/// `StopMovie` tears a playing movie down and fires `OnMovieFinished` once (`0x78cf1e`); with
/// nothing playing it fires nothing (`0x78cedb`).
#[test]
fn stop_movie_fires_on_movie_finished_once() {
    let (mut s, _) = movie_vm(vec![("A.avi", avi(10, &[]))]);
    assert!(s.eval::<bool>(r#"return M:StartMovie("A") == 1"#).unwrap());
    s.tick(0.0);
    assert_eq!(shown(&s), Some(0));
    s.run("M:StopMovie() M:StopMovie()").unwrap();
    assert_eq!(log(&s), "finished");
    assert!(!s.movie_playing());
    assert!(s.movie_pictures().is_empty());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The clock runs on the tick's clock: the first step shows frame 0, each later step the frame
/// its time rounds to, and the step whose frame reaches the count ends the movie, firing
/// `OnMovieFinished`. A hidden frame steps nothing, as its draw callback is not called.
#[test]
fn a_movie_plays_to_its_frame_count_and_finishes() {
    let (mut s, _) = movie_vm(vec![("A.avi", avi(10, &[]))]);
    assert!(s.eval::<bool>(r#"return M:StartMovie("A") == 1"#).unwrap());
    assert_eq!(shown(&s), None, "nothing shown before the first step");
    s.tick(0.0);
    assert_eq!(shown(&s), Some(0));
    s.run("M:Hide()").unwrap();
    s.tick(0.125);
    assert_eq!(shown(&s), Some(0), "hidden, the clock does not step");
    s.run("M:Show()").unwrap();
    s.tick(0.0);
    assert_eq!(shown(&s), Some(3), "125 ms is frame 3");
    s.tick(0.25);
    assert_eq!(shown(&s), Some(9), "375 ms rounds to frame 9");
    assert_eq!(log(&s), "");
    s.tick(0.0078125);
    assert_eq!(log(&s), "finished", "382 ms is frame 10, the count");
    assert!(!s.movie_playing());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A caption goes up on the first shown frame whose clock its range holds, with subtitles enabled
/// or no audio, and comes down on the first shown frame past its end (`0x78d0ed`); a step that
/// shows no new frame reads no caption. `StopMovie` with a caption up hides it before the finish
/// (`0x78cda0`).
#[test]
fn captions_go_up_and_come_down_on_shown_frames() {
    let sbt = b"00:00:00:02 - 00:00:00:05\r\nHi there\r\n".to_vec();
    let (mut s, _) = movie_vm(vec![("A.avi", avi(100, &[])), ("A.sbt", sbt)]);
    assert!(s.eval::<bool>(r#"return M:StartMovie("A") == 1"#).unwrap());
    s.tick(0.0);
    assert_eq!(log(&s), "", "0 ms is before 82 ms");
    s.tick(0.125);
    assert_eq!(log(&s), "show:Hi there");
    s.tick(0.0625);
    assert_eq!(log(&s), "show:Hi there", "187 ms is before 205 ms");
    s.tick(0.015625);
    assert_eq!(
        (shown(&s), log(&s).as_str()),
        (Some(5), "show:Hi there"),
        "203 ms shows no new frame"
    );
    s.tick(0.03125);
    assert_eq!(
        log(&s),
        "show:Hi there hide",
        "234 ms, frame 6, is past the end"
    );

    let sbt = b"00:00:00:00 - 00:00:10:00\nLong\n".to_vec();
    let (mut s, _) = movie_vm(vec![("A.avi", avi(100, &[])), ("A.sbt", sbt)]);
    s.run(r#"M:StartMovie("A")"#).unwrap();
    s.tick(0.0);
    s.tick(0.125);
    assert_eq!(log(&s), "show:Long");
    s.run("M:StopMovie()").unwrap();
    assert_eq!(log(&s), "show:Long hide finished");
}

/// With audio the captions wait for `EnableSubtitles` (`0x78d121`): its flag defaults on when
/// absent and is off for an explicit nil, as `GetScriptFlagArg` reads it; the constructor's is
/// off (`0x78c0ee`).
#[test]
fn enable_subtitles_gates_the_captions_of_a_movie_with_audio() {
    let sbt = b"00:00:00:00 - 00:00:10:00\nWords\n".to_vec();
    let files = vec![("A.avi", avi(100, &[b"mp3"])), ("A.sbt", sbt)];
    for (call, expect) in [
        ("", ""),
        ("M:EnableSubtitles()", "show:Words"),
        ("M:EnableSubtitles(nil)", ""),
        ("M:EnableSubtitles(1)", "show:Words"),
        ("M:EnableSubtitles(0)", ""),
    ] {
        let (mut s, _) = movie_vm(files.clone());
        s.run(call).unwrap();
        s.run(r#"M:StartMovie("A")"#).unwrap();
        let starts = s.take_movie_audio_starts();
        assert_eq!(starts.len(), 1);
        s.set_movie_audio(starts[0].id, MovieAudio::At(100));
        s.tick(0.0);
        s.set_movie_audio(starts[0].id, MovieAudio::At(225));
        s.tick(0.125);
        assert_eq!(shown(&s), Some(6), "{call}: anchored at 100 ms of audio");
        assert_eq!(log(&s), expect, "{call}");
    }
}

/// The audio: its MP3 bytes go to the host once; the clock waits while the stream has played
/// nothing, runs on the wall clock once the stream failed, and the stream's end finishes the
/// movie at the next step. The host holds a stream only for a movie still playing.
#[test]
fn the_host_plays_the_audio_and_its_end_finishes_the_movie() {
    let (mut s, _) = movie_vm(vec![("A.avi", avi(100, &[b"ab", b"c"]))]);
    s.run(r#"M:StartMovie("A")"#).unwrap();
    let starts = s.take_movie_audio_starts();
    assert_eq!(
        starts.iter().map(|a| a.mp3.clone()).collect::<Vec<_>>(),
        [b"abc".to_vec()]
    );
    assert!(s.take_movie_audio_starts().is_empty(), "handed over once");
    let id = starts[0].id;
    assert_eq!(s.movie_audio_ids(), [id]);
    s.tick(0.0);
    s.set_movie_audio(id, MovieAudio::At(0));
    s.tick(0.125);
    assert_eq!(shown(&s), None, "a stream at 0 ms holds the clock");
    s.set_movie_audio(id, MovieAudio::Failed);
    assert!(s.movie_audio_ids().is_empty());
    s.tick(0.0);
    assert_eq!(shown(&s), Some(0), "on the wall clock from here");
    s.set_movie_audio(id, MovieAudio::Ended);
    assert_eq!(log(&s), "");
    s.tick(0.0);
    assert_eq!(log(&s), "finished");
    assert!(s.movie_audio_ids().is_empty());
}

/// The picture draws at the frame's ARTWORK callback slot once a frame is shown, and no longer
/// once the movie stops.
#[test]
fn the_picture_is_extracted_while_the_movie_shows_one() {
    let (mut s, _) = movie_vm(vec![("A.avi", avi(10, &[]))]);
    let movie_quads = |s: &mut UiScript| {
        s.resolve();
        s.extract()
            .into_iter()
            .filter(|q| matches!(q.content, QuadContent::Movie { .. }))
            .count()
    };
    s.run(r#"M:StartMovie("A")"#).unwrap();
    assert_eq!(movie_quads(&mut s), 0);
    s.tick(0.0);
    assert_eq!(movie_quads(&mut s), 1);
    let p = &s.movie_pictures()[0];
    assert_eq!((p.width, p.height, p.serial), (1024, 464, 1));
    let mut rgba = Vec::new();
    assert!(s.write_movie_rgba(p.frame, &mut rgba));
    assert_eq!(rgba.len(), 1024 * 464 * 4);
    s.run("M:StopMovie()").unwrap();
    assert_eq!(movie_quads(&mut s), 0);
}

/// A movie's number is the process's, not the VM's: a rebuilt glue VM's first movie never takes
/// the number of a stream the host still holds for the old VM's.
#[test]
fn movie_numbers_are_never_reused_across_vms() {
    let start = || {
        let (mut s, _) = movie_vm(vec![("A.avi", avi(10, &[b"mp3"]))]);
        s.run(r#"M:StartMovie("A")"#).unwrap();
        let ids: Vec<u64> = s.take_movie_audio_starts().iter().map(|a| a.id).collect();
        (s, ids)
    };
    let (_old, first) = start();
    let (_new, second) = start();
    assert_eq!((first.len(), second.len()), (1, 1));
    assert_ne!(first, second);
}

/// A stream that ends before it played a millisecond ends the movie: with no play time to anchor
/// to, the clock runs on the wall clock, and the finished flag the end set stops it.
#[test]
fn a_stream_that_ends_unplayed_still_ends_the_movie() {
    let (mut s, _) = movie_vm(vec![("A.avi", avi(100, &[b"mp3"]))]);
    s.run(r#"M:StartMovie("A")"#).unwrap();
    let id = s.take_movie_audio_starts()[0].id;
    s.tick(0.0);
    s.set_movie_audio(id, MovieAudio::Ended);
    s.tick(0.0);
    assert_eq!(log(&s), "finished");
    assert!(!s.movie_playing());
}

/// The frame's destructor runs the teardown (`0x78c280` calls `0x78ced0`), so a VM dropped with a
/// movie playing fires its `OnMovieFinished`.
#[test]
fn a_vm_dropped_mid_movie_finishes_its_movie() {
    let (mut s, _) = movie_vm(vec![("A.avi", avi(100, &[]))]);
    let fired = Rc::new(RefCell::new(0));
    let count = fired.clone();
    let f = s
        .lua()
        .create_function(move |_, ()| {
            *count.borrow_mut() += 1;
            Ok(())
        })
        .unwrap();
    s.lua().globals().set("note_finished", f).unwrap();
    s.run(r#"M:SetScript("OnMovieFinished", function() note_finished() end) M:StartMovie("A")"#)
        .unwrap();
    s.tick(0.0);
    drop(s);
    assert_eq!(*fired.borrow(), 1);
}
