//! 4:2:0 Y'CbCr to RGBA8 by ITU-R BT.601, studio range (Y' 16-235, Cb and Cr 16-240), each chroma
//! sample covering its 2×2 luma samples.

use crate::decoder::Picture;

/// 16.16 fixed-point factors, from BT.601's `Kr = 0.299` and `Kb = 0.114`.
const KR: f64 = 0.299;
const KB: f64 = 0.114;
const KG: f64 = 1.0 - KR - KB;
const ONE: f64 = 65536.0;
const Y: i32 = (255.0 / 219.0 * ONE + 0.5) as i32;
const CR_R: i32 = (255.0 / 224.0 * 2.0 * (1.0 - KR) * ONE + 0.5) as i32;
const CB_G: i32 = (255.0 / 224.0 * 2.0 * (1.0 - KB) * KB / KG * ONE + 0.5) as i32;
const CR_G: i32 = (255.0 / 224.0 * 2.0 * (1.0 - KR) * KR / KG * ONE + 0.5) as i32;
const CB_B: i32 = (255.0 / 224.0 * 2.0 * (1.0 - KB) * ONE + 0.5) as i32;

#[inline]
fn px(y: i32, r: i32, g: i32, b: i32) -> [u8; 4] {
    let l = (y - 16) * Y + (1 << 15);
    [
        ((l + r) >> 16).clamp(0, 255) as u8,
        ((l + g) >> 16).clamp(0, 255) as u8,
        ((l + b) >> 16).clamp(0, 255) as u8,
        255,
    ]
}

/// Convert `pic` into `out`, `width * height * 4` bytes, rows top down.
pub(crate) fn to_rgba(pic: &Picture, out: &mut [u8]) {
    let (w, h) = (pic.y.width, pic.y.height);
    for (row, line) in out.chunks_exact_mut(w * 4).take(h).enumerate() {
        let ys = &pic.y.data[row * pic.y.stride..][..w];
        let cbs = &pic.cb.data[row / 2 * pic.cb.stride..];
        let crs = &pic.cr.data[row / 2 * pic.cr.stride..];
        for (i, (pair, yy)) in line.chunks_mut(8).zip(ys.chunks(2)).enumerate() {
            let cb = i32::from(cbs[i]) - 128;
            let cr = i32::from(crs[i]) - 128;
            let (r, g, b) = (CR_R * cr, -CB_G * cb - CR_G * cr, CB_B * cb);
            for (o, &y) in pair.as_chunks_mut::<4>().0.iter_mut().zip(yy) {
                o.copy_from_slice(&px(i32::from(y), r, g, b));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_range_maps_to_full_range_and_primaries_land_on_their_corners() {
        let near = |a: [u8; 4], b: [u8; 3]| a[..3].iter().zip(b).all(|(&x, y)| x.abs_diff(y) <= 1);
        assert_eq!(px(16, 0, 0, 0), [0, 0, 0, 255]);
        assert_eq!(px(235, 0, 0, 0), [255, 255, 255, 255]);
        // BT.601 red, green and blue in studio-range Y'CbCr.
        for ((y, cb, cr), rgb) in [
            ((81, 90, 240), [255, 0, 0]),
            ((145, 54, 34), [0, 255, 0]),
            ((41, 240, 110), [0, 0, 255]),
        ] {
            let (cb, cr) = (cb - 128, cr - 128);
            let got = px(y, CR_R * cr, -CB_G * cb - CR_G * cr, CB_B * cb);
            assert!(near(got, rgb), "{got:?} for {rgb:?}");
        }
    }
}
