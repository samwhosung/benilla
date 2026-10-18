//! The reference's byte colours and alphas: how a Lua number becomes the byte the client stores
//! and how the byte reads back, as the setters and getters compute them on the x87 at 53-bit
//! precision (so `f64` arithmetic matches).

/// `Frame:SetAlpha`'s byte (`0x774e90`): below 0 is 0 (`0x774f32`), 1 and above or NaN is 1
/// (`0x774f49`), then `__ftol(a * 255.0)` truncates (`0x774f5e`, `0x774f66`).
pub fn frame_alpha_byte(a: f64) -> u8 {
    let a = if a < 0.0 {
        0.0
    } else if a < 1.0 {
        a
    } else {
        1.0
    };
    (a * 255.0) as u8
}

/// An alpha byte as `Frame:GetAlpha` (`0x774e75`) and the region `GetAlpha`s read it: times the
/// `f64` 1/255 at `0x804578`, unrounded, so 255 reads 1.0 exactly and 127 0.4980392156862745.
pub fn alpha_f64(byte: u8) -> f64 {
    f64::from(byte) * (1.0 / 255.0)
}

/// An alpha byte as the renderer takes it, a fraction of 255.
pub fn alpha_f32(byte: u8) -> f32 {
    f32::from(byte) / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_alpha_byte_clamps_then_truncates() {
        assert_eq!(frame_alpha_byte(0.5), 127);
        assert_eq!(frame_alpha_byte(1.0), 255);
        assert_eq!(frame_alpha_byte(2.0), 255);
        assert_eq!(frame_alpha_byte(f64::NAN), 255);
        assert_eq!(frame_alpha_byte(-1.0), 0);
        assert_eq!(frame_alpha_byte(0.999), 254);
        assert_eq!(alpha_f64(127), 0.4980392156862745);
        assert_eq!(alpha_f64(255), 1.0);
    }
}
