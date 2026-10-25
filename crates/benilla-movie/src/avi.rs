//! The AVI reader, as the reference's `0x78c360` reads a movie: it requires `RIFF`/`AVI `, takes
//! the rate (`dwRate / dwScale`) and the frame count (`dwLength`) from the first `vids` stream
//! header and the size from the stream format after it, then walks `idx1` in 16-byte entries: an
//! entry whose chunk id has `d` as its third letter (`00dc`) is a video frame, one with `w`
//! (`01wb`) is appended to the audio.

use std::ops::Range;

use benilla_bytes::ByteExt;

use crate::Error;

/// What the movie file holds.
pub(crate) struct Avi {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rate: f32,
    pub(crate) frame_count: u32,
    /// Each video frame's bytes, in index order, as ranges of the file.
    pub(crate) video: Vec<Range<usize>>,
    /// The audio chunks, concatenated.
    pub(crate) audio: Vec<u8>,
}

/// A RIFF chunk: its id, and its payload as a range of the file. A `LIST` payload starts with
/// the list's type.
struct Chunk {
    id: [u8; 4],
    body: Range<usize>,
}

/// The chunks laid end to end in `range`, each padded to an even size. A truncated header ends
/// the walk; a payload past the end is an error.
fn chunks(file: &[u8], range: Range<usize>) -> Result<Vec<Chunk>, Error> {
    let mut out = Vec::new();
    let mut pos = range.start;
    while pos + 8 <= range.end {
        let id = file
            .bytes_at(pos, 4)
            .ok_or(Error::Truncated("chunk header"))?;
        let size = file
            .u32_at(pos + 4)
            .ok_or(Error::Truncated("chunk header"))? as usize;
        let start = pos + 8;
        let end = start
            .checked_add(size)
            .filter(|&e| e <= range.end)
            .ok_or(Error::Truncated("chunk past its parent"))?;
        out.push(Chunk {
            id: [id[0], id[1], id[2], id[3]],
            body: start..end,
        });
        pos = end + (size & 1);
    }
    Ok(out)
}

/// The list of type `kind` among `chunks`, its body after the type.
fn find_list(file: &[u8], chunks: &[Chunk], kind: &[u8; 4]) -> Option<Range<usize>> {
    chunks
        .iter()
        .find(|c| &c.id == b"LIST" && file.get(c.body.start..c.body.start + 4) == Some(kind))
        .map(|c| c.body.start + 4..c.body.end)
}

pub(crate) fn read(file: &[u8]) -> Result<Avi, Error> {
    if file.get(..4) != Some(b"RIFF") || file.get(8..12) != Some(b"AVI ") {
        return Err(Error::NotAvi);
    }
    let riff_end = file.u32_at(4).map_or(file.len(), |s| {
        (s as usize).saturating_add(8).min(file.len())
    });
    let top = chunks(file, 12..riff_end)?;
    let hdrl = find_list(file, &top, b"hdrl").ok_or(Error::Missing("hdrl"))?;

    let mut stream = None;
    for strl in chunks(file, hdrl)?.iter().filter(|c| &c.id == b"LIST") {
        if file.get(strl.body.start..strl.body.start + 4) != Some(b"strl") {
            continue;
        }
        let inner = chunks(file, strl.body.start + 4..strl.body.end)?;
        let Some(strh) = inner.iter().find(|c| &c.id == b"strh") else {
            continue;
        };
        let h = &file[strh.body.clone()];
        if h.get(..4) != Some(b"vids") {
            continue;
        }
        let scale = h.u32_at(20).ok_or(Error::Truncated("strh"))?;
        let rate = h.u32_at(24).ok_or(Error::Truncated("strh"))?;
        let length = h.u32_at(32).ok_or(Error::Truncated("strh"))?;
        if scale == 0 {
            return Err(Error::Truncated("strh with a zero dwScale"));
        }
        let strf = inner
            .iter()
            .find(|c| &c.id == b"strf")
            .ok_or(Error::Missing("strf"))?;
        let f = &file[strf.body.clone()];
        let width = f.i32_at(4).ok_or(Error::Truncated("strf"))?;
        let height = f.i32_at(8).ok_or(Error::Truncated("strf"))?;
        stream = Some((
            width.unsigned_abs(),
            height.unsigned_abs(),
            rate as f32 / scale as f32,
            length,
        ));
        break;
    }
    let (width, height, rate, frame_count) = stream.ok_or(Error::Missing("vids stream"))?;

    // idx1 offsets count from the `movi` list's type.
    let movi = find_list(file, &top, b"movi").ok_or(Error::Missing("movi"))?;
    let base = movi.start - 4;
    let idx1 = top
        .iter()
        .find(|c| &c.id == b"idx1")
        .ok_or(Error::Missing("idx1"))?;
    let index = &file[idx1.body.clone()];
    let mut video = Vec::new();
    let mut audio = Vec::new();
    for (n, e) in index.as_chunks::<16>().0.iter().enumerate() {
        let kind = e[2];
        if kind != b'd' && kind != b'w' {
            continue;
        }
        let word = |o: usize| u32::from_le_bytes([e[o], e[o + 1], e[o + 2], e[o + 3]]) as usize;
        let (offset, size) = (word(8), word(12));
        // The entry must name the chunk it points at, and the chunk must lie inside `movi`.
        let data = base
            .checked_add(offset)
            .filter(|&at| file.get(at..at + 4) == Some(&e[..4]))
            .and_then(|at| Some(at + 8..(at + 8).checked_add(size)?))
            .filter(|d| d.end <= movi.end)
            .ok_or(Error::BadIndex { entry: n })?;
        if kind == b'd' {
            video.push(data);
        } else {
            audio.extend_from_slice(&file[data]);
        }
    }
    Ok(Avi {
        width,
        height,
        rate,
        frame_count,
        video,
        audio,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A RIFF AVI with an audio stream listed before the video one, `frames` as the video chunks
    /// and `sounds` as the audio ones, interleaved, an odd-sized chunk among them.
    pub(crate) fn synthetic(
        width: i32,
        height: i32,
        frames: &[&[u8]],
        sounds: &[&[u8]],
    ) -> Vec<u8> {
        crate::testing::synthetic_avi(width, height, (41764, 1_000_000), 648, frames, sounds)
    }

    #[test]
    fn reads_the_first_video_stream_and_splits_frames_from_audio() {
        let file = synthetic(800, -368, &[b"abc", b"", b"defg"], &[b"mp3", b"!!"]);
        let avi = read(&file).expect("synthetic AVI");
        assert_eq!((avi.width, avi.height, avi.frame_count), (800, 368, 648));
        assert!((avi.rate - 1_000_000.0 / 41764.0).abs() < 1e-4);
        let frames: Vec<&[u8]> = avi.video.iter().map(|r| &file[r.clone()]).collect();
        assert_eq!(frames, [&b"abc"[..], b"", b"defg"]);
        assert_eq!(avi.audio, b"mp3!!");
    }

    #[test]
    fn refuses_what_is_not_an_avi_and_an_index_that_points_astray() {
        assert!(matches!(read(b"RIFF\0\0\0\0WAVE"), Err(Error::NotAvi)));
        let mut file = synthetic(1024, 464, &[b"frame"], &[]);
        // Move the one index entry's offset off its chunk.
        let n = file.len();
        file[n - 8] ^= 1;
        assert!(matches!(read(&file), Err(Error::BadIndex { entry: 0 })));
        // Cut the file inside the movi list.
        let file = synthetic(1024, 464, &[b"frame"], &[]);
        assert!(read(&file[..file.len() - 40]).is_err());
    }
}
