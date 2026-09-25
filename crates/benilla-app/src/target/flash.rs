//! The melee combat flash: the target's selection ring and overhead name pulse red to orange while
//! the local player auto-attacks it.
//!
//! The reference recomputes the flag, `[unit+0xc58]` bit `0x10`, every frame in CGUnit's OnUpdate
//! (`0x607f60` set, `0x607fe2` clear), and no packet touches it: set iff the unit is the current
//! target, the local player is auto-attacking (`[player+0xc48]`, here the server-echoed
//! [`Engaged`]) and `CanAttack 0x606980` passes. The pulse is one global phase that advances only
//! while a unit qualifies. Only the ring and the overhead name read it, through
//! `GetSelectionCircleColor 0x605960` (colour global `0xc4d8c8`); the V-key nameplate never does.

use bevy::prelude::*;

use crate::creature_anim::Engaged;
use crate::net::{ObjectStore, Reputations, SelfPlayer};

use super::relations::can_attack;
use super::{Factions, Selection};

/// This frame's flash verdict and the global pulse clock, read by the ring and the nameplate.
#[derive(Resource)]
pub(crate) struct CombatFlash {
    /// The unit that pulses this frame: the current target, or none.
    pub(crate) unit: Option<Entity>,
    pub(crate) color: Color,
    /// `[0xc4daa0]`: the last half-cycle flip, in ms.
    last_reset_ms: u32,
    /// `[0xc4daa4]`: the wave direction.
    rising: bool,
}

impl Default for CombatFlash {
    fn default() -> Self {
        Self {
            unit: None,
            // `0x5fa3f0`'s default, `0xFFFF0000`. Authored bytes go raw into the gamma
            // framebuffer, hence `linear_rgb`.
            color: Color::linear_rgb(1.0, 0.0, 0.0),
            last_reset_ms: 0,
            rising: false,
        }
    }
}

/// One sample of the `0x607f67` triangle on the G byte alone, advancing the clock: red
/// `0xFFFF0000` (G 0) to orange `0xFFFF8000` (G 128) over 500 ms, `G = trunc(128·frac)`. A flip
/// resets the cell to `now`, not to the half-period, so sparse frames drift as in the reference.
fn wave_g(now: u32, last_reset: &mut u32, rising: &mut bool) -> u8 {
    if now.wrapping_sub(*last_reset) >= 500 {
        *rising = !*rising;
        *last_reset = now;
    }
    let t = (500 - now.wrapping_sub(*last_reset)) as f32 / 500.0;
    let frac = if *rising { 1.0 - t } else { t };
    (128.0 * frac) as u8 // truncation toward zero, the client's __ftol
}

/// The per-frame OnUpdate gate, over the one unit that can qualify, the current target. Runs
/// before the ring update and the nameplate drive, which read this frame's verdict.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(super) fn drive_flash(
    mut flash: ResMut<CombatFlash>,
    time: Res<Time>,
    selection: Res<Selection>,
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    units: Query<Option<&ObjectStore>, Without<SelfPlayer>>,
) {
    let was = flash.unit;
    flash.unit = (|| {
        // The current target (`[0xb4e2d8]`).
        let target = selection.target?;
        // Auto-attacking: `[player+0xc48]` is nonzero.
        if engaged.is_empty() {
            return None;
        }
        // `CanAttack 0x606980`: a neutral target flashes too.
        let store = units.get(target).ok()?;
        can_attack(
            store,
            factions.as_deref(),
            &reputations,
            self_store.single().ok(),
        )
        .then_some(target)
    })();
    if flash.unit.is_some() {
        // Only a qualifying frame advances the global wave.
        let now = time.elapsed().as_millis() as u32;
        let CombatFlash {
            last_reset_ms,
            rising,
            ..
        } = &mut *flash;
        let g = wave_g(now, last_reset_ms, rising);
        // Raw into the gamma lane: G 128 is the authored orange `0xFF8000`.
        flash.color = Color::linear_rgb(1.0, g as f32 / 255.0, 0.0);
    }
    // Logged on the arm and disarm edge only.
    if was != flash.unit {
        debug!("combat flash: {was:?} → {:?}", flash.unit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 128 down to 0 over 500 ms and back, continuous at the flip, truncated rather than rounded.
    #[test]
    fn triangle_wave_matches_the_byte_recurrence() {
        let (mut reset, mut rising) = (0u32, false);
        assert_eq!(wave_g(0, &mut reset, &mut rising), 128, "falling start");
        assert_eq!(wave_g(250, &mut reset, &mut rising), 64, "midpoint");
        assert_eq!(wave_g(499, &mut reset, &mut rising), 0, "trunc(128·1/500)");
        // Elapsed ≥ 500 resets the cell and reverses.
        assert_eq!(wave_g(500, &mut reset, &mut rising), 0, "joint");
        assert!(rising && reset == 500);
        assert_eq!(wave_g(750, &mut reset, &mut rising), 64, "rising midpoint");
        assert_eq!(
            wave_g(1000, &mut reset, &mut rising),
            128,
            "peak at the next flip"
        );
        assert!(!rising, "direction reversed again");
    }
}
