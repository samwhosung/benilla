//! The particle lane's two debug instruments, [`super::depthdump`] and [`super::emitdump`], as one
//! system parameter: `simulate_particles` sits at Bevy's 16-parameter ceiling. Both are inert
//! without their env.

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
