//! The world's render flags word `[0xc7b2a4]`, which the console commands `CWorld::Initialize`
//! (`0x66f6c0`) registers toggle. The commands are the app's (`world_console`); the clutter and
//! liquid-drift passes read the flags.

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
