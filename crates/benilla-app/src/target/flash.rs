//! The melee combat flash — the pulsing **red ↔ orange** tint on the current target's selection
//! ring and overhead name while the local player is auto-attacking it.
//!
//! The flag law: `[unit+0xc58]` bit 0x10 is **recomputed every frame** in CGUnit's OnUpdate
//! (`0x607f60` set / `0x607fe2` clear; whole-binary census — no packet touches it). Set iff the
//! unit is the local player's current **TARGET**, the local player is **actively auto-attacking**
//! (`[player+0xc48]` — the swing-target GUID the `Attack()` handler stores; benilla's
//! server-echoed [`Engaged`] bracket is the same predicate), and the unit is **legally
//! attackable** (`0x606980`: UNIT_FIELD_FLAGS disqualifier bits clear + reaction ≤ neutral —
//! the recorded "hostile ≤ 1" gloss was director-falsified, see `scan::can_attack` / 0170;
//! the duel/PVP leg is deferred with duels). So the flash means *"I am in melee with my target"*
//! — not "this unit attacks me" (the hypothesis the bytes refined).
//!
//! The pulse (`0x607f67`–`0x607fd0`): a continuous linear **triangle** wave on the **G byte
//! only** — half-period 500 ms (1 Hz full period), `G = trunc(128·frac)`, red `0xFFFF0000`
//! (G=0) ↔ orange `0xFFFF8000` (G=128); A/R/B fixed. The clock cells (`[0xc4daa0]/[0xc4daa4]`)
//! are **global** — one shared phase — and advance only while a unit qualifies. Consumers: the
//! **selection ring** and the **overhead name**, through the SAME `GetSelectionCircleColor
//! 0x605960` first-priority branch (colour global `0xc4d8c8`) — and nothing else (the V-key
//! nameplate never flashes; verified exhaustive).

use bevy::prelude::*;

use crate::creature_anim::Engaged;
use crate::net::{ObjectStore, Reputations, SelfPlayer};

use super::relations::can_attack;
use super::{Factions, Selection};

/// This frame's flash verdict + the global pulse clock. Recomputed every frame by
/// [`drive_flash`]; read by the ring's material pick and the nameplate colour gate.
#[derive(Resource)]
pub(crate) struct CombatFlash {
    /// The unit whose ring + overhead name pulse this frame — the current target, or nobody.
    pub(crate) unit: Option<Entity>,
    /// This frame's wave colour (the G-byte triangle over red↔orange).
    pub(crate) color: Color,
    /// `[0xc4daa0]` — the last half-cycle reset time (ms).
    last_reset_ms: u32,
    /// `[0xc4daa4]` — the wave direction bit.
    rising: bool,
}

impl Default for CombatFlash {
    fn default() -> Self {
        Self {
            unit: None,
            // The default writer `0x5fa3f0`: 0xFFFF0000. GAMMA LANE: authored bytes go
            // raw into the gamma framebuffer — `linear_rgb`, like the ring/name palettes.
            color: Color::linear_rgb(1.0, 0.0, 0.0),
            last_reset_ms: 0,
            rising: false,
        }
    }
}

/// One G-byte triangle sample at `now`, advancing the global clock cells — the exact `0x607f67`
/// recurrence: `t = (500 − (now − lastReset))/500 ∈ (0,1]`, `frac = rising ? 1−t : t`,
/// `G = trunc(128·frac)`; a half-cycle elapse flips the direction and resets the cell to `now`
/// (the client's own sparse-frame drift). The join is continuous — G hits 0/128 exactly at each
/// flip.
fn wave_g(now: u32, last_reset: &mut u32, rising: &mut bool) -> u8 {
    if now.wrapping_sub(*last_reset) >= 500 {
        *rising = !*rising;
        *last_reset = now;
    }
    let t = (500 - now.wrapping_sub(*last_reset)) as f32 / 500.0;
    let frac = if *rising { 1.0 - t } else { t };
    (128.0 * frac) as u8 // truncation toward zero — the client's __ftol
}

/// Recompute the flash verdict — the client's per-frame OnUpdate gate, evaluated over the one
/// unit that can qualify (your current target). Runs before the ring update and the nameplate
/// drive so both consume this frame's verdict.
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
    // The per-frame gate — `None` means no flash this frame.
    flash.unit = (|| {
        // Gate 1 — the unit is the current target (`[0xb4e2d8]`).
        let target = selection.target?;
        // Gate 3 — the local player is actively auto-attacking (`[player+0xc48]` ≠ 0).
        if engaged.is_empty() {
            return None;
        }
        // Gate 4 — legally attackable: the shared `CanAttack 0x606980` predicate (flag
        // disqualifiers + reaction ≤ neutral — see `scan::can_attack`; the director's reference
        // A/B pinned that neutral targets flash too).
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
        // Qualified — advance the global wave and stamp this frame's colour.
        let now = time.elapsed().as_millis() as u32;
        let CombatFlash {
            last_reset_ms,
            rising,
            ..
        } = &mut *flash;
        let g = wave_g(now, last_reset_ms, rising);
        // The wave byte, raw into the gamma lane: G=128 lands exactly on the authored
        // orange 0xFF8000 = linear_rgb(1.0, 0.502, 0.0).
        flash.color = Color::linear_rgb(1.0, g as f32 / 255.0, 0.0);
    }
    // The arm/disarm EDGE (never per-frame) — so "it never flashes" in the field is diagnosable
    // from the log.
    if was != flash.unit {
        debug!("combat flash: {was:?} → {:?}", flash.unit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wave's pinned points: falling from 128 hits 0 at the half-period, the flip is
    /// continuous (0 at the joint), the rising half climbs back to 128 — 1 Hz round trip, and
    /// truncation (not rounding) on the G byte.
    #[test]
    fn triangle_wave_matches_the_byte_recurrence() {
        let (mut reset, mut rising) = (0u32, false);
        assert_eq!(wave_g(0, &mut reset, &mut rising), 128, "falling start");
        assert_eq!(wave_g(250, &mut reset, &mut rising), 64, "midpoint");
        assert_eq!(wave_g(499, &mut reset, &mut rising), 0, "trunc(128·1/500)");
        // The flip: elapsed ≥ 500 resets the cell and reverses — continuous at 0.
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
