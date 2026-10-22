//! The world's console state: the render flags word `[0xc7b2a4]` and the levels the console
//! commands `CWorld::Initialize` (`0x66f6c0`) registers write. The commands themselves are the
//! app's (`world_console`); the passes read these.

use bevy::prelude::*;

/// The world flags `[0xc7b2a4]`, which gate the world's passes. Start-up ORs [`Self::START`] in
/// (`0x66f6cb`); the console toggles flip single bits.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldFlags(pub u32);

impl WorldFlags {
    /// `or eax,0x7100b73` at `0x66f6cb`.
    pub const START: u32 = 0x0710_0b73;
    /// Terrain culling, `showCull` (`0x673250`).
    pub const CULL: u32 = 0x20;
    /// Terrain shadow, `showShadow` (`0x673a30`) and the `mapShadows` setting.
    pub const SHADOW: u32 = 0x40;
    /// Detail doodads (ground clutter), `showDetailDoodads` (`0x673050`), tested by the chunk pass
    /// (`0x6847c9`).
    pub const DETAIL_DOODADS: u32 = 0x0010_0000;
    /// Liquid particulates, `waterParticulates` (`0x673770`): the drift cloud's advect (`0x66fee0`),
    /// draw (`0x6701e0`) and re-scatter on entering a liquid (`0x680afa`).
    pub const PARTICULATES: u32 = 0x0200_0000;
    /// The far low-detail band, `showLowDetail` (`0x673a70`), tested at `0x6844bc`.
    pub const LOW_DETAIL: u32 = 0x0400_0000;

    pub fn has(self, bit: u32) -> bool {
        self.0 & bit != 0
    }
}

impl Default for WorldFlags {
    fn default() -> Self {
        Self(Self::START)
    }
}

/// The levels the world's console commands store besides the flags, at their start-up values.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldLevels {
    /// `maxLOD` (`0x673200`): the near chunk level `[0x867964]`, 2 or 3, 3 at start-up
    /// (`0x66f8ae`).
    pub max_lod: u32,
    /// `waterMaxLOD` (`0x6736c0`): `[0x86a030]`, 0 to 4, initialised 4.
    pub water_max_lod: i32,
    /// `waterWaves` (`0x673710`): `[0x86a034]`, initialised 2.
    pub water_waves: i32,
    /// `waterSpecular` (`0x673730`): `[0x86a038]`, initialised 1.
    pub water_specular: i32,
    /// `waterRipples` (`0x673750`): `[0x86a03c]`, initialised 1, the ripple emitter's gate
    /// (`0x68bda3`).
    pub water_ripples: i32,
    /// `showSimpleDoodads` (`0x673ac0`): `[0xc7b2c4]`, cleared at start-up (`0x66f714`).
    pub simple_doodads: bool,
    /// `setShadow` (`0x673290`): the shadow colour `0x672590` stores, packed first float in the
    /// top byte, which the world frame writes back from the light state each frame (`0x66fff3`).
    pub shadow_color: Option<u32>,
}

impl Default for WorldLevels {
    fn default() -> Self {
        Self {
            max_lod: 3,
            water_max_lod: 4,
            water_waves: 2,
            water_specular: 1,
            water_ripples: 1,
            simple_doodads: false,
            shadow_color: None,
        }
    }
}
