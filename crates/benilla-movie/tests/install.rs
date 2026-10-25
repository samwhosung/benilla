//! The install's four movies decode end to end, and an instrument that compares one with ffmpeg.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use benilla_movie::Movie;

/// `Interface/Cinematics/<name>` under `data`, matched without regard to case, as the install's
/// folders are named differently from one copy to another.
fn cinematic(data: &Path, name: &str) -> Option<PathBuf> {
    let mut dir = data.to_path_buf();
    for part in ["Interface", "Cinematics", name] {
        let entry = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        dir = entry.path();
    }
    Some(dir)
}

fn open(name: &str) -> Option<Movie> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let path = cinematic(&data, name).unwrap_or_else(|| panic!("{name} is not in the install"));
    let file = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Some(Movie::open(file).unwrap_or_else(|e| panic!("{name}: {e}")))
}

/// Every frame decodes without an error, and the counts, the size, the rate and the audio are the
/// file's.
fn decodes_whole(name: &str, width: u32, height: u32, frames: usize, rate: f32) {
    let Some(mut movie) = open(name) else {
        return;
    };
    assert_eq!((movie.width(), movie.height()), (width, height), "{name}");
    assert_eq!(movie.frame_count() as usize, frames, "{name}");
    assert_eq!(movie.frames_in_index(), frames, "{name}");
    assert!(
        (movie.rate() - rate).abs() < 1e-3,
        "{name}: {} fps",
        movie.rate()
    );
    // The audio is MP3: it opens on a frame sync.
    let audio = movie.audio();
    assert!(
        audio.len() > 100_000 && audio[0] == 0xff && audio[1] & 0xe0 == 0xe0,
        "{name}"
    );
    let mut decoded = 0;
    let mut widest = 0;
    while movie
        .decode_next()
        .unwrap_or_else(|e| panic!("{name}: {e}"))
    {
        decoded += 1;
        // Every 64th picture's luma range: the movie shows pictures, not flat fields.
        if decoded % 64 == 0 {
            let yuv = movie.yuv();
            let (lo, hi) = yuv.y[..yuv.y_stride * yuv.height]
                .iter()
                .fold((255, 0), |(lo, hi), &v| (v.min(lo), v.max(hi)));
            widest = widest.max(hi - lo);
        }
    }
    assert_eq!(decoded, frames, "{name}");
    assert!(
        widest > 128,
        "{name}: no picture spans more than {widest} luma steps"
    );
}

#[test]
fn intro_1024_decodes_every_frame() {
    decodes_whole("WOW_Intro_1024.avi", 1024, 464, 4083, 1e6 / 41599.0);
}

#[test]
fn intro_800_decodes_every_frame() {
    decodes_whole("WOW_Intro_800.avi", 800, 368, 4083, 1e6 / 41599.0);
}

#[test]
fn logo_1024_decodes_every_frame() {
    decodes_whole("Logo_1024.avi", 1024, 464, 648, 1e6 / 41764.0);
}

#[test]
fn logo_800_decodes_every_frame() {
    decodes_whole("Logo_800.avi", 800, 368, 648, 1e6 / 41764.0);
}

/// The instrument: decode `$BENILLA_MOVIE` (default `WOW_Intro_1024.avi`) with ffmpeg, piped as
/// raw yuv420p, and with this decoder; print each plane's mean and worst PSNR over the frames,
/// and this decoder's speed. Run it under `--profile play` for a release-speed number:
/// `cargo test --profile play -p benilla-movie --test install -- --ignored --nocapture`.
#[test]
#[ignore = "an instrument: needs ffmpeg on PATH and the install"]
fn psnr_against_ffmpeg() {
    let name = std::env::var("BENILLA_MOVIE").unwrap_or_else(|_| "WOW_Intro_1024.avi".into());
    let Some(mut movie) = open(&name) else {
        return;
    };
    let data = benilla_formats::wow_data().expect("install");
    let path = cinematic(&data, &name).expect("movie");
    let mut ffmpeg = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&path)
        .args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
        ])
        .args(["-pix_fmt", "yuv420p", "-"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("ffmpeg on PATH");
    let mut pipe = ffmpeg.stdout.take().expect("ffmpeg stdout");
    let (w, h) = (movie.width() as usize, movie.height() as usize);
    let (cw, ch) = (w / 2, h / 2);
    let mut theirs = vec![0u8; w * h + 2 * cw * ch];
    let psnr = |mse: f64| {
        if mse == 0.0 {
            99.0
        } else {
            10.0 * (255.0f64 * 255.0 / mse).log10()
        }
    };
    let mse = |ours: &[u8], stride: usize, theirs: &[u8], pw: usize, ph: usize| {
        let mut sum = 0u64;
        for y in 0..ph {
            for x in 0..pw {
                let d = i64::from(ours[y * stride + x]) - i64::from(theirs[y * pw + x]);
                sum += (d * d) as u64;
            }
        }
        sum as f64 / (pw * ph) as f64
    };
    let mut stats = [(0.0f64, f64::MAX, 0usize); 3];
    let (mut frames, mut exact) = (0usize, 0usize);
    let mut spent = Duration::ZERO;
    loop {
        let t = Instant::now();
        let more = movie.decode_next().expect("our decode");
        spent += t.elapsed();
        if !more {
            break;
        }
        pipe.read_exact(&mut theirs).expect("ffmpeg frame");
        let yuv = movie.yuv();
        let planes = [
            (yuv.y, yuv.y_stride, &theirs[..w * h], w, h),
            (
                yuv.cb,
                yuv.c_stride,
                &theirs[w * h..w * h + cw * ch],
                cw,
                ch,
            ),
            (yuv.cr, yuv.c_stride, &theirs[w * h + cw * ch..], cw, ch),
        ];
        let mut all_exact = true;
        for (s, (ours, stride, th, pw, ph)) in stats.iter_mut().zip(planes) {
            let e = mse(ours, stride, th, pw, ph);
            all_exact &= e == 0.0;
            let p = psnr(e);
            s.0 += p;
            if p < s.1 {
                s.1 = p;
                s.2 = frames;
            }
        }
        exact += usize::from(all_exact);
        frames += 1;
    }
    assert_eq!(
        pipe.read(&mut theirs).expect("ffmpeg end"),
        0,
        "ffmpeg has more frames"
    );
    assert!(ffmpeg.wait().expect("ffmpeg").success());
    println!("{name}: {frames} frames, {exact} identical to ffmpeg's");
    for (plane, (sum, worst, at)) in ["Y", "Cb", "Cr"].iter().zip(stats) {
        println!(
            "  {plane}: mean PSNR {:.2} dB, worst {worst:.2} dB at frame {at}",
            sum / frames as f64
        );
    }
    let secs = spent.as_secs_f64();
    println!(
        "  decode: {secs:.2} s, {:.0} frames/s ({:.1}x realtime at {} fps)",
        frames as f64 / secs,
        frames as f64 / secs / f64::from(movie.rate()),
        movie.rate()
    );
    let mut rgba = Vec::new();
    let t = Instant::now();
    for _ in 0..100 {
        movie.write_rgba(&mut rgba);
    }
    println!("  rgba: {:.2} ms a frame", t.elapsed().as_secs_f64() * 10.0);
}
