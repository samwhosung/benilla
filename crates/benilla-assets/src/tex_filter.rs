//! The frame's **texture filter policy** — one process-global pair, exactly as the reference holds
//! it, and the divergence that pair was hiding.
//!
//! ## What the reference actually does
//!
//! The real 1.12.1 client does not let a texture choose how it is filtered. Every
//! `TextureCreate` (`0x449d90`) runs the caller's request through **`0x449ae0`** with a force flag
//! that is a literal `1` at every observed call site, and `0x449ae0` **overwrites** it from two
//! globals: filter bits 0-2 from `[0x835250]`, aniso bits 9-13 from
//! `(mode == 5 ? [0x835254] : 1)`. Their static `.data` values are **`[0x835250] = 3`** and
//! **`[0x835254] = 1`**, and a full image-wide caller census of their only two writers
//! (`0x449ac0` / `0x449ad0`) finds exactly three call sites, all inside the CVar callbacks below.
//! WMO, M2/character and terrain textures all share the one policy; there is no per-draw filter
//! override anywhere. (`0x449ae0`/`0x59f170`/`0x58a980`/`0x6c4c20` + the registration block
//! `0x6885b0`-`0x688840`.)
//!
//! Mode 3 — the static `.data` value — is `GL_LINEAR_MIPMAP_NEAREST` / `GL_LINEAR`: bilinear with
//! nearest-mip select, anisotropy off. Two CVars move it, and only upward:
//!
//! - **`trilinear`** (reg `0x688608`, registered default `"0"`) — nonzero sets mode 4,
//!   `GL_LINEAR_MIPMAP_LINEAR`.
//! - **`anisotropic`** (reg `0x6887d0`, registered default **`"1"`**) — a value >1 sets mode 5
//!   (trilinear *plus* real aniso N, clamped to the device cap) and **outranks** `trilinear`.
//!
//! ## …and why the registrar's string is not what the client renders at
//!
//! **`hwDetect` overwrites it before the first frame.** `hwDetect` registers `"1"`, and
//! `DetectHardware 0x641260` → `0x639a60` then `CVar::Set`s **sixteen** video CVars from the
//! matched `VideoHardware.dbc` row, self-clears to `"0"`, and persists — so the registrar's string
//! is only ever what an install renders at if the hardware table has nothing to say.
//!
//! It always has something to say. Every GPU this client runs on now is unlisted in a 2004 table,
//! so the row comes from the **fallback** scan (`0x641610`, from record index 1, vendor `0xFFFF`
//! against a D3D9 capability tier from `CGxDevice::DeviceAdapterInfer 0x58bc90`), whose reachable
//! set is exactly three rows: **168** (tier 0), **169** (tier 1), **170** (tier 2). Tier 2 needs
//! HW T&L, >2 simultaneous textures and PS ≥ 1.1 — which any D3D9 part saturates.
//!
//! **`trilinear` is 1 on rows 169 and 170, at both CPU tiers, and carries no CPU bias term.** So
//! the mode a real machine gets from a virgin install is **4**, not 3, and that is what benilla
//! registers (decision 1645, superseding 1642's `"0"`). Measured, not only derived: the reference
//! client's own `WoW/Logs/gx.log` on the director's machine reads `VID: 106b` (unlisted) →
//! `DeviceAdapterInfer … DID: 2` → `DetectHardware(): videoID: 170`.
//!
//! **`anisotropic` is NOT one of the sixteen** (verified by scanning `[0x639a60, 0x639b80)`:
//! sixteen record-pointer reads, `0xc7f2e4` absent). It keeps its registered `"1"` on every path,
//! so anisotropy really is off out of the box — 1642's load-bearing half stands.
//!
//! ## One correction to how wide the policy reaches
//!
//! 1642 said there is no per-texture filter override anywhere in the reference. That is **too
//! broad**. `applyGlobal` is a per-*call* argument to `0x449ae0`, and while terrain (`0x6c4c77`),
//! WMO (the same wrapper `0x6c4c20`) and M2 (`0x71da72`) all pass `1` — which is what this module
//! needs and is confirmed — six subsystems pass `0` and keep their own mode: weather sprites
//! (hard-coded mode 4), the minimap, glue/loading, Lua texture widgets and the sky. None of them
//! is a lane this module builds a sampler for, so the code is right; the claim was not.
//!
//! ## The divergence this module closes
//!
//! benilla hardcoded `mipmap_filter: Linear` + `anisotropy_clamp: 8` at every sampler it built —
//! mode 5 at aniso 8, on every terrain layer array, model albedo, WMO albedo and static-GX array
//! in the game, with no CVar and no way to turn it off. That is the *maximum* of what the
//! reference will do on request, shipped as the thing it does before anybody asks.
//!
//! It is [`decisions/1624`]'s mistake one sampler over, and it was made the same way. `farclip`
//! shipped at 777 because that was "what most players' `Config.wtf` had"; the filtering shipped at
//! trilinear+aniso because an apitrace of the repo's reference install showed trilinear+aniso16 —
//! and that install's `Config.wtf` carries `SET anisotropic "16"`. Both read a *set* value as a
//! *registered* one. Verifying that a mechanism exists is not verifying that it is the default.
//!
//! The cost is not incidental. Mode 3 takes one bilinear tap per sample. Mode 5 at aniso 8 takes up
//! to **sixteen** — eight along the anisotropy axis, doubled by the trilinear mip blend — and the
//! worst case for the anisotropy axis is a tiling surface at a grazing angle, which is what ground
//! is from a third-person camera. On the GPU-bound Steam Deck frame behind 1624 (B329) the texture
//! path is where the frame already lives.
//!
//! ## Why a process global here too
//!
//! Not imitation: the same constraint the reference had. A sampler is baked into the [`Image`]
//! asset at load time, and the lanes that build one are an async `AssetLoader` with no world
//! access ([`crate::blp`]) and ordinary systems ([`crate::world_assets`], `static_gx`) — neither
//! can read the other's resource. This is exactly the shape [`crate::gpu_blp`]'s `bc_supported`
//! already has, for exactly the same reason.
//!
//! **Latched, like `gxMultisample`** (decision 1629): read once at boot and never again, because a
//! sampler already baked into an uploaded texture cannot be changed without rebuilding it — which
//! is also why the reference's own UI says "enabled upon restart". The CVar holds the pending
//! value; the textures keep what they were born with.

use std::sync::OnceLock;

use bevy::image::ImageFilterMode;
use bevy::prelude::*;
use bevy::render::render_resource::FilterMode;

/// The reference's `anisotropic` clamp — its callback parses 1-16 before clamping to the device
/// cap (`0x689110`).
pub const ANISO_RANGE: std::ops::RangeInclusive<u32> = 1..=16;

/// The two globals, as one value: `trilinear` and `anisotropic` resolved into the policy every
/// sampler in the process takes.
///
/// The precedence is the reference's own, not a choice: `anisotropic >= 2` writes mode 5 at
/// `0x68919f` and `trilinear` writes mode 4 at `0x688d0f` *only* when the aniso option bit is
/// clear (`0x688d0d js` skips it), so aniso outranks trilinear whichever order the callbacks run.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub struct TexFilterSetting {
    /// `trilinear` — mode 4 when set and aniso is off.
    pub trilinear: bool,
    /// `anisotropic` — 1 is off; >=2 is mode 5 at that level.
    pub aniso: u32,
}

impl Default for TexFilterSetting {
    /// What a virgin install actually renders at — `trilinear` **on**, `anisotropic` off — with
    /// `$WOW_TRILINEAR` / `$WOW_ANISO` overriding **session-only**, the same law `$WOW_MSAA` and
    /// `$WOW_FARCLIP` run under: this is the A/B lever for pricing the filter policy on one machine
    /// in one session, and a value pinned into `config.toml` would make a measurement sticky.
    ///
    /// `trilinear` is `true` here and not the registrar's `"0"` because `hwDetect` sets it on both
    /// fallback rows a real GPU can reach — see the module doc, and decision 1645.
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
    /// The reference's filter mode 0-5 (`[0x835250]`) this policy resolves to. Only 3, 4 and 5 are
    /// reachable from the two CVars; the lower three are set by nothing in the shipped image.
    pub fn mode(self) -> u8 {
        if self.aniso >= 2 {
            5
        } else if self.trilinear {
            4
        } else {
            3
        }
    }

    /// The mip filter this policy asks for: mode 3 selects the nearer mip, 4 and 5 blend the two.
    pub fn mipmap_filter(self) -> ImageFilterMode {
        match self.mode() {
            3 => ImageFilterMode::Nearest,
            _ => ImageFilterMode::Linear,
        }
    }

    /// The same, for the lanes that build a raw wgpu [`SamplerDescriptor`](
    /// bevy::render::render_resource::SamplerDescriptor) instead of an [`Image`]'s.
    pub fn gpu_mipmap_filter(self) -> FilterMode {
        match self.mode() {
            3 => FilterMode::Nearest,
            _ => FilterMode::Linear,
        }
    }

    /// The max anisotropy — `(mode == 5 ? [0x835254] : 1)`, `0x449afc`.
    ///
    /// wgpu additionally requires all three filters to be `Linear` before it will accept a clamp
    /// above 1, which mode 5 satisfies by construction and modes 3/4 never reach.
    pub fn anisotropy_clamp(self) -> u16 {
        if self.mode() == 5 {
            self.aniso.clamp(*ANISO_RANGE.start(), *ANISO_RANGE.end()) as u16
        } else {
            1
        }
    }
}

static POLICY: OnceLock<TexFilterSetting> = OnceLock::new();

/// Publish the resolved policy for the process. Called once, from the CVar load
/// (`benilla_app::cvars::load_config`, the `CvarLoad` set) — the earliest moment `config.toml` has
/// been folded in, and before any system that can request a texture has run.
pub fn publish_tex_filter(setting: TexFilterSetting) {
    let _ = POLICY.set(setting);
}

/// The policy every sampler in the process takes.
///
/// Unpublished — the world viewer, a headless test, or a texture somehow requested ahead of
/// `CvarLoad` — falls back to [`TexFilterSetting::default`], which is what a virgin install
/// renders at. A race can therefore only ever produce the faithful value, never a wrong one.
pub fn tex_filter() -> TexFilterSetting {
    *POLICY.get_or_init(TexFilterSetting::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **What benilla actually ships** — `hwDetect`'s row-170 outcome, not the registrar's string:
    /// trilinear on, anisotropy off, mode 4 (decision 1645). If this ever reads mode 5, we are
    /// shipping the divergence 1642 removed; if it reads mode 3, we are back to reading the
    /// registrar's string as if `hwDetect` did not exist.
    ///
    /// `Default` is the assertion's subject deliberately: it is the value every unpublished lane
    /// falls back to, so this pins the fallback and the shipped default in one place. It reads the
    /// env, hence the guard — the levers are session-only and a set one must not fail the suite.
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

    /// The registrar's own string, which is a real thing that is simply not what a machine gets:
    /// `[0x835250] = 3` with `trilinear "0"` is the state a client would render at if the hardware
    /// table said nothing. Kept as a test because the mode-3 arm is still reachable — any run whose
    /// `config.toml` or `$WOW_TRILINEAR` says 0 takes it.
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

    /// `trilinear 1` alone is mode 4: the mip blend arrives, the aniso does not.
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

    /// `anisotropic >= 2` is mode 5 and **outranks** `trilinear` in both directions — the mip
    /// blend comes with it whether or not `trilinear` was ever set.
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

    /// `anisotropic 1` is off, not "aniso at one sample" — the reference's own reading of the
    /// registered string, and the reason the default is not mode 5.
    #[test]
    fn aniso_one_is_off() {
        let s = TexFilterSetting {
            trilinear: false,
            aniso: 1,
        };
        assert_eq!(s.mode(), 3);
    }

    /// wgpu rejects a clamp above 1 unless every filter is `Linear`; modes 3 and 4 must therefore
    /// never report one, whatever the CVar holds.
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
