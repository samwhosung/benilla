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

/// The colour getters' scale, the `f32` at `0x81c6b4` (`0x3b80806e`, one ulp below 1/255): each
/// channel reads back as its byte times this, formed in `f64` (`fild`, `fmul dword`, `fstp
/// qword`), so 255 reads 0.9999978030100465.
pub const CHANNEL_SCALE: f32 = f32::from_bits(0x3b80_806e);

/// One byte as the colour getters read it ([`CHANNEL_SCALE`]).
pub fn channel(byte: u8) -> f64 {
    f64::from(byte) * f64::from(CHANNEL_SCALE)
}

/// The alpha-only setters' byte (`Font:SetAlpha` `0x79f449`, `Texture:SetAlpha` `0x79b638`,
/// `FontString:SetAlpha` `0x79cc28`): `__ftol(a * 255)` and its low byte, with no clamp and no
/// rounding, so 2 is 254, -0.1 is 231 and NaN is 0.
pub fn wrap_alpha_byte(a: f64) -> u8 {
    crate::script::binding_abi::ftol(a * 255.0) as u8
}

/// A colour channel as every colour setter clamps it (`0x79f4e9`-`0x79f511`): below 0 is 0, 1 and
/// above or NaN is 1 (`fcom` falls through an unordered compare).
fn clamp_channel(x: f64) -> f64 {
    if x < 0.0 {
        0.0
    } else if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// A clamped channel's byte, rounded half up: `__ftol(x * 255.0 + 0.5)` (`0x79f5e8`-`0x79f5f5`).
fn round_byte(x: f64) -> u8 {
    (x * 255.0 + 0.5) as u8
}

/// The reference's packed colour, `NTempest::CImVector`, one `0xAARRGGBB` dword: every widget
/// colour, region vertex colour, gradient stop, font text and shadow colour, backdrop colour and
/// XML `<Color>` is one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgba8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba8 {
    /// `0xffffffff`, what an unset colour reads (`0x77f8c0`, `0x783a8e`).
    pub const WHITE: Self = Self::rgba(255, 255, 255, 255);
    /// `0xff000000`, a FontString's own default shadow (`0x770da1`).
    pub const BLACK: Self = Self::rgba(0, 0, 0, 255);
    /// `0x00000000`, the default shadow of every font `CSimpleFont`'s constructor builds
    /// (`0x783a72`): a `CreateFont` or `<Font>` font and a widget's own.
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Every colour setter's pack (`SetTextColor` `0x79f4d0`, `SetVertexColor` `0x79abd0`, the
    /// gradient, FontString, Button, StatusBar and backdrop setters, and the XML `<Color>` reader
    /// `0x6f23d0`): each channel clamped ([`clamp_channel`]), red, green and blue stored as `f32`s
    /// (`fstp dword`, `0x79f50c`) while the alpha stays the live double (`0x79f5e8`), then each
    /// rounded half up to its byte.
    pub fn pack(r: f64, g: f64, b: f64, a: f64) -> Self {
        let byte = |x: f64| round_byte(f64::from(clamp_channel(x) as f32));
        Self::rgba(byte(r), byte(g), byte(b), round_byte(clamp_channel(a)))
    }

    /// [`Self::pack`] from the `f32`s an internal writer holds.
    pub fn from_f32(c: [f32; 4]) -> Self {
        Self::pack(
            f64::from(c[0]),
            f64::from(c[1]),
            f64::from(c[2]),
            f64::from(c[3]),
        )
    }

    /// The four values a colour getter pushes, red first, each [`channel`].
    pub fn channels(self) -> (f64, f64, f64, f64) {
        (
            channel(self.r),
            channel(self.g),
            channel(self.b),
            channel(self.a),
        )
    }

    /// Each byte over 255 as a double, the arguments that re-enter a setter as this colour: the
    /// setter's pack gives the same bytes back.
    pub fn fractions(self) -> (f64, f64, f64, f64) {
        let f = |b: u8| f64::from(b) / 255.0;
        (f(self.r), f(self.g), f(self.b), f(self.a))
    }

    /// The colour as the renderer takes it, each byte a fraction of 255.
    pub fn to_f32(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a].map(|b| f32::from(b) / 255.0)
    }

    /// This colour with alpha byte `a`.
    pub fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }
}

/// What a colour handed to a setter becomes: the draw's colour and the getter's answer, for a
/// caller that compares against the numbers it passed.
pub mod quant {
    use super::*;

    /// The colour the draw sees of `c` set through a setter.
    pub fn drawn<T: Into<f64> + Copy>(c: [T; 4]) -> [f32; 4] {
        let [r, g, b, a] = c.map(Into::into);
        Rgba8::pack(r, g, b, a).to_f32()
    }

    /// The value a colour getter reads back of channel `x` set through a setter.
    pub fn read(x: f64) -> f64 {
        channel(Rgba8::pack(x, 0.0, 0.0, 1.0).r)
    }

    /// [`read`] as the `f32` a test's tuple holds.
    pub fn read32(x: f64) -> f32 {
        read(x) as f32
    }
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

    #[test]
    fn the_pack_clamps_rounds_half_up_and_reads_back_through_the_float_scale() {
        assert_eq!(
            Rgba8::pack(0.5, 2.0, -1.0, f64::NAN),
            Rgba8::rgba(128, 255, 0, 255)
        );
        assert_eq!(channel(255), 0.9999978030100465);
        assert_eq!(channel(128), 0.5019596815109253);
        assert_eq!(wrap_alpha_byte(2.0), 254);
        assert_eq!(wrap_alpha_byte(-0.1), 231);
        assert_eq!(wrap_alpha_byte(f64::NAN), 0);
        assert_eq!(wrap_alpha_byte(0.5), 127);
    }
}
