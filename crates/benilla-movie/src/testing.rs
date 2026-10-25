//! A synthetic AVI for the tests of this crate and of the crates that play its movies
//! (`test-support`).

fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = id.to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
    out
}

fn list(kind: &[u8; 4], parts: &[Vec<u8>]) -> Vec<u8> {
    let mut body = kind.to_vec();
    for p in parts {
        body.extend_from_slice(p);
    }
    chunk(b"LIST", &body)
}

fn strh(kind: &[u8; 4], scale: u32, rate: u32, length: u32) -> Vec<u8> {
    let mut h = vec![0u8; 56];
    h[..4].copy_from_slice(kind);
    h[20..24].copy_from_slice(&scale.to_le_bytes());
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[32..36].copy_from_slice(&length.to_le_bytes());
    chunk(b"strh", &h)
}

/// A RIFF AVI with an audio stream listed before the video one, its video header's
/// `(dwScale, dwRate)` and `dwLength` as given, `frames` as the video chunks and `sounds` as the
/// audio ones, interleaved. An empty frame chunk decodes as a repeat of the picture before it.
pub fn synthetic_avi(
    width: i32,
    height: i32,
    (scale, rate): (u32, u32),
    length: u32,
    frames: &[&[u8]],
    sounds: &[&[u8]],
) -> Vec<u8> {
    let mut bih = vec![0u8; 40];
    bih[..4].copy_from_slice(&40u32.to_le_bytes());
    bih[4..8].copy_from_slice(&width.to_le_bytes());
    bih[8..12].copy_from_slice(&height.to_le_bytes());
    let hdrl = list(
        b"hdrl",
        &[
            chunk(b"avih", &[0u8; 56]),
            list(
                b"strl",
                &[strh(b"auds", 1, 44100, 0), chunk(b"strf", &[0u8; 30])],
            ),
            list(
                b"strl",
                &[strh(b"vids", scale, rate, length), chunk(b"strf", &bih)],
            ),
        ],
    );
    let mut movi_body = b"movi".to_vec();
    let mut index = Vec::new();
    let mut entry = |id: &[u8; 4], data: &[u8], movi_body: &mut Vec<u8>| {
        index.extend_from_slice(id);
        index.extend_from_slice(&0x10u32.to_le_bytes());
        index.extend_from_slice(&(movi_body.len() as u32).to_le_bytes());
        index.extend_from_slice(&(data.len() as u32).to_le_bytes());
        movi_body.extend_from_slice(&chunk(id, data));
    };
    for i in 0..frames.len().max(sounds.len()) {
        if let Some(s) = sounds.get(i) {
            entry(b"01wb", s, &mut movi_body);
        }
        if let Some(f) = frames.get(i) {
            entry(b"00dc", f, &mut movi_body);
        }
    }
    let mut body = b"AVI ".to_vec();
    body.extend_from_slice(&hdrl);
    body.extend_from_slice(&chunk(b"JUNK", &[0; 3]));
    body.extend_from_slice(&chunk(b"LIST", &movi_body));
    body.extend_from_slice(&chunk(b"idx1", &index));
    chunk(b"RIFF", &body)
}
