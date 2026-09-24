//! The particle lane's debug-instrument system plumbing, bundled into one parameter.
//!
//! `simulate_particles` sits exactly at Bevy's 16-system-parameter ceiling, and the lane carries
//! two optional instruments — [`super::depthdump`] (`$WOW_PARTICLE_DEPTHDUMP`, the depth numbers
//! a pool brings to the compare) and [`super::emitdump`] (`$WOW_EMIT_DUMP`, what the emission
//! front end decided) — each of which would otherwise spend a parameter or two of its own. That
//! ceiling should never be the reason an instrument doesn't get built, so the
//! pair shares one: both stay inert without their env, and the sim reads them through here.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

#[derive(SystemParam)]
pub(super) struct Dumps<'w, 's> {
    /// `$WOW_PARTICLE_DEPTHDUMP`'s frame counter.
    depth_frames: Local<'s, u32>,
    /// `$WOW_EMIT_DUMP`'s label lookup and period clock.
    pub(super) emit: super::emitdump::EmitDump<'w, 's>,
}

impl Dumps<'_, '_> {
    /// `$WOW_PARTICLE_DEPTHDUMP`: this frame's dump index, if it is a dump frame at all.
    pub(super) fn depth_frame(&mut self, now: f32) -> Option<u32> {
        super::depthdump::frame(now, &mut self.depth_frames)
    }
}
