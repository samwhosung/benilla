//! The V-plate frame border, resampled sharp.
//!
//! Deviation: the reference magnifies the 128 × 32 `Interface\Tooltips\Nameplate-Border` art with
//! `GL_LINEAR` (`0x770200`, filter `[0x878cf0]` = 1), smearing its 1 px bevel once the plate is
//! larger than native; we resample the same pixels to the plate's physical size with sharp
//! bilinear and draw it 1:1, because the chosen look is the same art with a crisp bevel.
//! [`super::drive_vplates`] regenerates it when the physical size changes.

/// Resample `src` (an `sw × sh` sRGB RGBA8 image) to `dw × dh` with sharp bilinear, premultiplied
/// so alpha edges do not fringe dark. Magnifying squeezes each texel boundary to about one output
/// pixel; minifying is plain bilinear.
pub(crate) fn resample_sharp(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let fetch = |x: i32, y: i32| -> [f32; 4] {
        let x = x.clamp(0, sw as i32 - 1) as u32;
        let y = y.clamp(0, sh as i32 - 1) as u32;
        let i = ((y * sw + x) * 4) as usize;
        let a = src[i + 3] as f32 / 255.0;
        [
            src[i] as f32 / 255.0 * a,
            src[i + 1] as f32 / 255.0 * a,
            src[i + 2] as f32 / 255.0 * a,
            a,
        ]
    };
    let sharpen_x = (dw as f32 / sw as f32).max(1.0);
    let sharpen_y = (dh as f32 / sh as f32).max(1.0);
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for dy in 0..dh {
        for dx in 0..dw {
            // Source coordinate at this output texel's centre.
            let su = (dx as f32 + 0.5) * sw as f32 / dw as f32 - 0.5;
            let sv = (dy as f32 + 0.5) * sh as f32 / dh as f32 - 0.5;
            let (x0, y0) = (su.floor(), sv.floor());
            // Steepen the ramp so a texel boundary spans about one output pixel.
            let ru = (((su - x0) - 0.5) * sharpen_x + 0.5).clamp(0.0, 1.0);
            let rv = (((sv - y0) - 0.5) * sharpen_y + 0.5).clamp(0.0, 1.0);
            let (x0, y0) = (x0 as i32, y0 as i32);
            let c00 = fetch(x0, y0);
            let c10 = fetch(x0 + 1, y0);
            let c01 = fetch(x0, y0 + 1);
            let c11 = fetch(x0 + 1, y0 + 1);
            let mut c = [0.0f32; 4];
            for i in 0..4 {
                let top = c00[i] * (1.0 - ru) + c10[i] * ru;
                let bot = c01[i] * (1.0 - ru) + c11[i] * ru;
                c[i] = top * (1.0 - rv) + bot * rv;
            }
            let a = c[3];
            let (r, g, b) = if a > 0.0 {
                (c[0] / a, c[1] / a, c[2] / a) // un-premultiply
            } else {
                (0.0, 0.0, 0.0)
            };
            let i = ((dy * dw + dx) * 4) as usize;
            out[i] = (r * 255.0).round() as u8;
            out[i + 1] = (g * 255.0).round() as u8;
            out[i + 2] = (b * 255.0).round() as u8;
            out[i + 3] = (a * 255.0).round() as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 source with one opaque red texel, upscaled 4×, keeps a solid red core.
    #[test]
    fn resample_sharp_keeps_the_texel_crisp() {
        let mut src = vec![0u8; 2 * 2 * 4];
        src[0] = 255; // r
        src[3] = 255; // a
        let (dw, dh) = (8, 8);
        let out = resample_sharp(&src, 2, 2, dw, dh);
        assert_eq!(out.len(), (dw * dh * 4) as usize);
        let px = |x: u32, y: u32| {
            let i = ((y * dw + x) * 4) as usize;
            [out[i], out[i + 1], out[i + 2], out[i + 3]]
        };
        assert_eq!(px(0, 0), [255, 0, 0, 255], "crisp opaque red core");
        assert_eq!(px(7, 7)[3], 0, "transparent quadrant stays clear");
    }

    /// Opaque pixels, so premultiplication round-trips exactly.
    #[test]
    fn resample_sharp_identity_at_same_size() {
        let mut src = vec![0u8; 3 * 3 * 4];
        for p in 0..9 {
            src[p * 4] = (p * 20) as u8;
            src[p * 4 + 1] = (p * 10) as u8;
            src[p * 4 + 2] = (p * 5) as u8;
            src[p * 4 + 3] = 255;
        }
        let out = resample_sharp(&src, 3, 3, 3, 3);
        assert_eq!(out, src);
    }
}
