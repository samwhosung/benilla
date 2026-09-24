//! The process texture-filter policy: `trilinear` and `anisotropic` resolved into one filter mode,
//! as the reference holds it in two globals.
//!
//! `TextureCreate` (`0x449d90`) hands `0x449ae0` an `applyGlobal` flag; when it is 1, the filter
//! bits 0-2 are overwritten from `[0x835250]` (static 3) and the aniso bits 9-13 from
//! `(mode == 5 ? [0x835254] : 1)`, globals set only from the CVar callbacks (`0x449ac0`,
//! `0x449ad0`). Terrain (`0x6c4c77`), WMO (the same wrapper `0x6c4c20`), M2 (`0x71da72`) and the
//! liquid sheets (`0x68abab`) pass 1. Six subsystems pass 0 and keep their own mode: weather
//! sprites (a hard-coded mode 4), the minimap, glue and loading screens, Lua texture widgets, the
//! sky and footprint decals (`0x699bd0`).
//!
//! Mode 3 is `GL_LINEAR_MIPMAP_NEAREST`, 4 `GL_LINEAR_MIPMAP_LINEAR`, 5 trilinear with anisotropy
//! N (applied at `0x59f170`). `trilinear` (`0x688608`, registered `"0"`) sets 4; `anisotropic`
//! (`0x6887d0`, registered `"1"`) above 1 sets 5 and outranks it.
//!
//! A fresh install renders at mode 4: `hwDetect` (`0x641260`, `0x639a60`) sets sixteen video CVars
//! from `VideoHardware.dbc`, and any current GPU takes the fallback scan (`0x641610`, its tier from
//! `0x58bc90`), whose rows 169 and 170 set `trilinear` 1 at both CPU tiers. `anisotropic` is not
//! one of the sixteen (`0xc7f2e4` is absent from `[0x639a60, 0x639b80)`), so anisotropy stays off.
//!
//! A process global because a sampler is baked into the [`Image`] at load, by an async loader
//! ([`crate::blp`]) and by systems ([`crate::world_assets`]) that share no resource. Latched at
//! boot like `gxMultisample`: a baked sampler cannot change without a rebuild, which is why the
//! reference's UI says "enabled upon restart".

use std::sync::OnceLock;

use bevy::image::ImageFilterMode;
use bevy::prelude::*;
use bevy::render::render_resource::FilterMode;

/// The reference's `anisotropic` range: its callback parses 1-16, then clamps to the device cap
/// (`0x689110`).
pub const ANISO_RANGE: std::ops::RangeInclusive<u32> = 1..=16;

/// `trilinear` and `anisotropic` resolved into the policy every sampler takes. Aniso outranks
/// trilinear in either callback order: `anisotropic >= 2` writes mode 5 (`0x68919f`), and
/// `trilinear` writes mode 4 (`0x688d0f`) only while the aniso bit is clear (`0x688d0d`).
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub struct TexFilterSetting {
    /// `trilinear`: mode 4 when set and aniso is off.
    pub trilinear: bool,
    /// `anisotropic`: 1 is off; 2 and up is mode 5 at that level.
    pub aniso: u32,
}

impl Default for TexFilterSetting {
    /// A fresh install: `trilinear` on, as `hwDetect` sets it, and `anisotropic` off.
    /// `$WOW_TRILINEAR` and `$WOW_ANISO` override for the session only, like `$WOW_MSAA`, so an A/B
    /// never sticks in `config.toml`.
    fn default() -> Self {
        let env_flag = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|v| !matches!(v.as_str(), "" | "0" | "off"))
        };
        Self {
            trilinear: env_flag("WOW_TRILINEAR").unwrap_or(true),
            aniso: std::env::var("WOW_ANISO")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .map_or(1, |v| v.clamp(*ANISO_RANGE.start(), *ANISO_RANGE.end())),
        }
    }
}

impl TexFilterSetting {
    /// The reference's filter mode (`[0x835250]`); the two CVars reach only 3, 4 and 5.
    pub fn mode(self) -> u8 {
        if self.aniso >= 2 {
            5
        } else if self.trilinear {
            4
        } else {
            3
        }
    }

    /// Mode 3 selects the nearer mip; 4 and 5 blend the two.
    pub fn mipmap_filter(self) -> ImageFilterMode {
        match self.mode() {
            3 => ImageFilterMode::Nearest,
            _ => ImageFilterMode::Linear,
        }
    }

    /// The same, for a lane that builds a raw wgpu `SamplerDescriptor`.
    pub fn gpu_mipmap_filter(self) -> FilterMode {
        match self.mode() {
            3 => FilterMode::Nearest,
            _ => FilterMode::Linear,
        }
    }

    /// The max anisotropy, `(mode == 5 ? [0x835254] : 1)` (`0x449afc`). wgpu takes a clamp above 1
    /// only with every filter `Linear`, which mode 5 always has.
    pub fn anisotropy_clamp(self) -> u16 {
        if self.mode() == 5 {
            self.aniso.clamp(*ANISO_RANGE.start(), *ANISO_RANGE.end()) as u16
        } else {
            1
        }
    }
}

static POLICY: OnceLock<TexFilterSetting> = OnceLock::new();

/// Publish the resolved policy once, from the CVar load (`benilla_app::cvars::load_config`, the
/// `CvarLoad` set): after `config.toml` is folded in, before any system can request a texture.
pub fn publish_tex_filter(setting: TexFilterSetting) {
    let _ = POLICY.set(setting);
}

/// The policy every sampler takes; unpublished (the world viewer, a headless test), it is
/// [`TexFilterSetting::default`], a fresh install's value.
pub fn tex_filter() -> TexFilterSetting {
    *POLICY.get_or_init(TexFilterSetting::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `hwDetect`'s row-170 outcome, mode 4. `Default` is also the unpublished fallback; it reads
    /// the env, so a set lever skips the test.
    #[test]
    fn what_ships_is_trilinear_with_anisotropy_off() {
        if std::env::var_os("WOW_TRILINEAR").is_some() || std::env::var_os("WOW_ANISO").is_some() {
            return; // an A/B session owns the value; the levers are tested below on literals
        }
        let shipped = TexFilterSetting::default();
        assert!(
            shipped.trilinear,
            "hwDetect sets trilinear on rows 169 and 170"
        );
        assert_eq!(
            shipped.aniso, 1,
            "anisotropic is not one of hwDetect's sixteen"
        );
        assert_eq!(shipped.mode(), 4);
        assert_eq!(shipped.mipmap_filter(), ImageFilterMode::Linear);
        assert_eq!(shipped.anisotropy_clamp(), 1);
    }

    /// The registered strings alone, mode 3, reached when `config.toml` or `$WOW_TRILINEAR` says 0.
    #[test]
    fn the_registrars_own_string_is_still_mode_three() {
        let registrar = TexFilterSetting {
            trilinear: false,
            aniso: 1,
        };
        assert_eq!(registrar.mode(), 3);
        assert_eq!(registrar.mipmap_filter(), ImageFilterMode::Nearest);
        assert_eq!(registrar.gpu_mipmap_filter(), FilterMode::Nearest);
        assert_eq!(registrar.anisotropy_clamp(), 1);
    }

    #[test]
    fn trilinear_alone_is_mode_four() {
        let s = TexFilterSetting {
            trilinear: true,
            aniso: 1,
        };
        assert_eq!(s.mode(), 4);
        assert_eq!(s.mipmap_filter(), ImageFilterMode::Linear);
        assert_eq!(s.anisotropy_clamp(), 1);
    }

    #[test]
    fn aniso_outranks_trilinear_in_both_directions() {
        for trilinear in [false, true] {
            let s = TexFilterSetting {
                trilinear,
                aniso: 8,
            };
            assert_eq!(s.mode(), 5, "aniso 8 is mode 5 with trilinear={trilinear}");
            assert_eq!(s.mipmap_filter(), ImageFilterMode::Linear);
            assert_eq!(s.anisotropy_clamp(), 8);
        }
    }

    #[test]
    fn aniso_one_is_off() {
        let s = TexFilterSetting {
            trilinear: false,
            aniso: 1,
        };
        assert_eq!(s.mode(), 3);
    }

    #[test]
    fn a_clamp_above_one_only_ever_rides_all_linear_filters() {
        for s in [
            TexFilterSetting {
                trilinear: false,
                aniso: 1,
            },
            TexFilterSetting {
                trilinear: true,
                aniso: 1,
            },
            TexFilterSetting {
                trilinear: true,
                aniso: 16,
            },
        ] {
            if s.anisotropy_clamp() > 1 {
                assert_eq!(s.mipmap_filter(), ImageFilterMode::Linear);
                assert_eq!(s.gpu_mipmap_filter(), FilterMode::Linear);
            }
        }
    }
}
