//! The 1.12.1 intro movies (`Interface/Cinematics/*.avi`): the AVI reader and a decoder for their
//! video, MPEG-4 Part 2 Simple profile (DivX 5, FourCC `DXGM`), written from ISO/IEC 14496-2.
//!
//! [`Movie::open`] is the reference's file reader and decoder start (`0x78c360`, `0x78c7a0`): it
//! reads the AVI whole, keeps the video frames and the concatenated audio (an MP3 stream), and
//! refuses a width other than 1024 or 800. [`Movie::decode_next`] decodes one frame, as the
//! reference's `0x78d560` does whether or not the frame is shown; [`Movie::write_rgba`] converts
//! the last one decoded, the reference's 4-byte output format.
//!
//! The decoder takes what a Simple-profile, rectangular, progressive stream uses: I- and P-VOPs,
//! the VOL headers repeated through the stream, not-coded VOPs and macroblocks, intra DC and AC
//! prediction, the H.263 quantiser, one or four vectors per macroblock with half-sample motion
//! compensation, and stuffing. Any other tool is refused with [`Unsupported`].

mod avi;
mod bits;
mod color;
mod decoder;
mod headers;
mod idct;
mod scan;
mod vlc;

use std::ops::Range;

use bits::BitReader;
use decoder::Decoder;
use headers::{HeaderError, Vol};

/// An MPEG-4 Part 2 tool the decoder does not implement, none of which the 1.12.1 movies use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
    ChromaFormat,
    Shape,
    Interlaced,
    Obmc,
    Sprite,
    Not8Bit,
    Mpeg2Quant,
    QuarterPel,
    ComplexityEstimation,
    ResyncMarkers,
    DataPartitioning,
    NewPred,
    ReducedResolution,
    Scalability,
    BVop,
    /// More than one VOP in one AVI chunk, DivX's packed bitstream.
    PackedVops,
}

/// Why a movie failed to open or a frame to decode.
#[derive(Debug)]
pub enum Error {
    /// No `RIFF` / `AVI ` header.
    NotAvi,
    Truncated(&'static str),
    /// A required chunk or stream is absent.
    Missing(&'static str),
    /// An `idx1` entry that points at no chunk of its id inside `movi`.
    BadIndex {
        entry: usize,
    },
    /// The reference's decoder start takes a width of 1024 or 800 alone (`0x78c7a0`).
    Width(u32),
    Unsupported(Unsupported),
    /// The video stream is broken in frame `frame`.
    Stream {
        frame: usize,
        what: &'static str,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotAvi => write!(f, "not an AVI file"),
            Error::Truncated(what) => write!(f, "truncated AVI: {what}"),
            Error::Missing(what) => write!(f, "AVI without {what}"),
            Error::BadIndex { entry } => write!(f, "AVI index entry {entry} points at no chunk"),
            Error::Width(w) => write!(f, "movie width {w} is neither 1024 nor 800"),
            Error::Unsupported(u) => write!(f, "MPEG-4 tool not supported: {u:?}"),
            Error::Stream { frame, what } => write!(f, "movie frame {frame}: {what}"),
        }
    }
}

impl std::error::Error for Error {}

/// The last decoded picture's planes, 4:2:0: `cb` and `cr` are half the size each way.
pub struct Yuv<'a> {
    pub width: usize,
    pub height: usize,
    pub y: &'a [u8],
    pub y_stride: usize,
    pub cb: &'a [u8],
    pub cr: &'a [u8],
    pub c_stride: usize,
}

/// A movie open for decoding, its frames pulled one at a time.
pub struct Movie {
    file: Vec<u8>,
    width: u32,
    height: u32,
    rate: f32,
    frame_count: u32,
    video: Vec<Range<usize>>,
    audio: Vec<u8>,
    vol: Option<Vol>,
    decoder: Decoder,
    next: usize,
}

impl Movie {
    /// Read a movie from its file's bytes.
    pub fn open(file: Vec<u8>) -> Result<Movie, Error> {
        let avi = avi::read(&file)?;
        if avi.width != 1024 && avi.width != 800 {
            return Err(Error::Width(avi.width));
        }
        if avi.height == 0 || avi.height > 4096 {
            return Err(Error::Truncated("strf with an implausible height"));
        }
        Ok(Movie {
            decoder: Decoder::new(avi.width as usize, avi.height as usize),
            file,
            width: avi.width,
            height: avi.height,
            rate: avi.rate,
            frame_count: avi.frame_count,
            video: avi.video,
            audio: avi.audio,
            vol: None,
            next: 0,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Frames a second: the video stream header's `dwRate / dwScale`.
    pub fn rate(&self) -> f32 {
        self.rate
    }

    /// The video stream header's `dwLength`, which the reference's clock ends the movie at.
    pub fn frame_count(&self) -> u32 {
        self.frame_count
    }

    /// The video frames the index lists.
    pub fn frames_in_index(&self) -> usize {
        self.video.len()
    }

    /// The audio chunks, concatenated: an MP3 stream for the 1.12.1 movies.
    pub fn audio(&self) -> &[u8] {
        &self.audio
    }

    /// The index of the frame [`Movie::decode_next`] decodes next.
    pub fn next_frame(&self) -> usize {
        self.next
    }

    /// Decode the next frame: `Ok(false)` once every indexed frame is decoded. A frame whose VOP
    /// is not coded, or a chunk with no data, repeats the picture before it.
    pub fn decode_next(&mut self) -> Result<bool, Error> {
        let Some(range) = self.video.get(self.next).cloned() else {
            return Ok(false);
        };
        let frame = self.next;
        self.next += 1;
        let stream = |what| Error::Stream { frame, what };
        let header = |e| match e {
            HeaderError::Unsupported(u) => Error::Unsupported(u),
            HeaderError::Corrupt(what) => stream(what),
        };
        let data = &self.file[range];
        let mut vops = 0;
        for (code, body) in start_codes(data) {
            match code {
                0x20..=0x2f => {
                    let vol = headers::parse_vol(&mut BitReader::new(body)).map_err(header)?;
                    if (vol.width, vol.height) != (self.width, self.height) {
                        return Err(stream("VOL size differs from the AVI's"));
                    }
                    self.vol = Some(vol);
                }
                0xb6 => {
                    vops += 1;
                    if vops > 1 {
                        return Err(Error::Unsupported(Unsupported::PackedVops));
                    }
                    let vol = self.vol.as_ref().ok_or(stream("VOP before any VOL"))?;
                    let mut r = BitReader::new(body);
                    if let Some(vop) = headers::parse_vop(&mut r, vol).map_err(header)? {
                        self.decoder.decode_vop(&mut r, &vop).map_err(stream)?;
                    }
                }
                // Sequence, object, user data and group headers carry nothing decoding reads.
                _ => {}
            }
        }
        if vops == 0 && !data.is_empty() {
            return Err(stream("frame chunk without a VOP"));
        }
        Ok(true)
    }

    /// The last decoded picture: black before the first frame.
    pub fn yuv(&self) -> Yuv<'_> {
        let p = self.decoder.picture();
        Yuv {
            width: p.y.width,
            height: p.y.height,
            y: &p.y.data,
            y_stride: p.y.stride,
            cb: &p.cb.data,
            cr: &p.cr.data,
            c_stride: p.cb.stride,
        }
    }

    /// The last decoded picture as RGBA8, rows top down, into `out`, resized to fit.
    pub fn write_rgba(&self, out: &mut Vec<u8>) {
        let p = self.decoder.picture();
        out.resize(p.y.width * p.y.height * 4, 0);
        color::to_rgba(p, out);
    }
}

/// The start codes in `data` (`00 00 01 xx`, byte aligned), each with the bytes up to the next.
fn start_codes(data: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    let find = |from: usize| -> Option<usize> {
        let mut i = from;
        while i + 3 < data.len() {
            if data[i + 2] > 1 {
                i += 3;
            } else if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
                return Some(i);
            } else {
                i += 1;
            }
        }
        None
    };
    let mut at = find(0);
    std::iter::from_fn(move || {
        let s = at?;
        let next = find(s + 4);
        at = next;
        Some((data[s + 3], &data[s + 4..next.unwrap_or(data.len())]))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_codes_split_a_chunk() {
        let data = [9, 0, 0, 1, 0x20, 7, 7, 0, 0, 1, 0xb6, 1, 0, 0, 0, 1, 0xb2];
        let got: Vec<(u8, &[u8])> = start_codes(&data).collect();
        assert_eq!(
            got,
            [(0x20, &[7, 7][..]), (0xb6, &[1, 0][..]), (0xb2, &[][..])]
        );
    }

    #[test]
    fn open_refuses_a_width_the_reference_refuses() {
        let file = avi::tests::synthetic(640, 480, &[b""], &[]);
        assert!(matches!(Movie::open(file), Err(Error::Width(640))));
        let file = avi::tests::synthetic(800, 368, &[b""], &[b"abc"]);
        let mut movie = Movie::open(file).expect("800 wide opens");
        assert_eq!(movie.audio(), b"abc");
        // An empty chunk repeats the picture before it, black at the start.
        assert!(movie.decode_next().expect("empty frame"));
        assert!(!movie.decode_next().expect("end"));
        let mut rgba = Vec::new();
        movie.write_rgba(&mut rgba);
        assert_eq!(rgba.len(), 800 * 368 * 4);
        assert_eq!(&rgba[..4], &[0, 0, 0, 255]);
    }

    /// MSB-first bits from `(value, width)` fields, zero-padded to a byte.
    fn pack(fields: &[(u32, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut bit = 0;
        for &(v, n) in fields {
            for i in (0..n).rev() {
                if bit % 8 == 0 {
                    out.push(0);
                }
                if v >> i & 1 != 0 {
                    *out.last_mut().unwrap() |= 0x80 >> (bit % 8);
                }
                bit += 1;
            }
        }
        out
    }

    /// A version-1 VOL for an 800×368, 25-tick stream, then a VOP header of `kind` (0 I, 1 P).
    fn vol_and_vop(kind: u32) -> Vec<u8> {
        let mut out = vec![0, 0, 1, 0x20];
        out.extend(pack(&[
            (0, 1),
            (1, 8),
            (0, 1),
            (1, 4),
            (0, 1),
            (0, 2),
            (1, 1),
            (25, 16),
            (1, 1),
            (0, 1),
            (1, 1),
            (800, 13),
            (1, 1),
            (368, 13),
            (1, 1),
            (0, 1),
            (1, 1),
            (0, 1),
            (0, 1),
            (0, 1),
            (1, 1),
            (1, 1),
            (0, 1),
            (0, 1),
        ]));
        out.extend([0, 0, 1, 0xb6]);
        let mut vop = vec![(kind, 2), (0, 1), (1, 1), (3, 5), (1, 1), (1, 1)];
        if kind == 1 {
            vop.push((0, 1));
        }
        vop.extend([(0, 3), (4, 5)]);
        if kind == 1 {
            vop.push((1, 3));
        }
        // The macroblocks follow the header unaligned: fill its last byte with ones, which in a
        // P-VOP are not-coded macroblocks.
        let bits: u32 = vop.iter().map(|f| f.1).sum();
        let fill = (8 - bits % 8) % 8;
        vop.push(((1 << fill) - 1, fill));
        out.extend(pack(&vop));
        out
    }

    #[test]
    fn a_broken_stream_fails_its_frame_and_never_panics() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let frames: Vec<Vec<u8>> = (0..300)
            .map(|i| {
                let mut f = vol_and_vop((i % 3 != 0) as u32);
                let len = (next() % 3000) as usize;
                // Mostly ones and runs of zeros, the shapes that stress code lookups and escapes.
                f.extend((0..len).map(|_| match next() % 4 {
                    0 => 0,
                    1 => 0xff,
                    _ => next() as u8,
                }));
                f
            })
            .collect();
        // A P-VOP whose 50 × 23 macroblocks are all not coded decodes: its header's fill holds the
        // first, 144 bytes of ones the rest.
        let mut skipped = vol_and_vop(1);
        skipped.extend([0xff; 144]);
        let mut frames = frames;
        frames.push(skipped);
        let refs: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
        let mut movie = Movie::open(avi::tests::synthetic(800, 368, &refs, &[])).expect("opens");
        let mut failed = 0;
        for _ in 0..300 {
            match movie.decode_next() {
                Ok(_) => {}
                Err(Error::Stream { .. }) => failed += 1,
                Err(e) => panic!("{e}"),
            }
        }
        assert!(movie.decode_next().expect("the not-coded P-VOP"));
        assert!(!movie.decode_next().expect("the end"));
        assert!(failed > 250, "only {failed} of 300 garbage frames failed");
        let mut rgba = Vec::new();
        movie.write_rgba(&mut rgba);
    }
}
