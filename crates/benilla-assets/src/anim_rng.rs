//! The client's one `rand()` stream, the MSVC CRT LCG at `0x7400e5`, in this leaf crate because
//! the engine's placed doodads and the game's creature, GameObject and portrait-booth arms all
//! draw from it. The reference keeps one cell per thread (`[ptd+0x14]`) and runs every arm on the
//! main thread (the frame loop `0x420c00`); that interleaving de-syncs a stand of identical props,
//! so it must stay one stream.
//!
//! The reference seeds it with `srand(GetTickCount())` in CRT static init (`0x5d1c70`, the call
//! at `0x5d1c8b`), so each run rolls differently. Its exact sequence is out of reach: every other
//! draw moves the stream, FrameXML's `random()` and each particle emitter among them.

use bevy::prelude::Resource;

/// The shared `rand()` state, seeded once per session by [`Self::seed_for_session`].
#[derive(Resource)]
pub struct AnimRng(u32);

impl Default for AnimRng {
    fn default() -> Self {
        Self(1) // `_initptd 0x40aca8`: the CRT's per-thread default, before `srand` runs
    }
}

impl AnimRng {
    /// One `rand()` draw, in `[0, 32767]` (`0x7400ed`-`0x740101`).
    pub fn draw(&mut self) -> u16 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.0 >> 16) & 0x7fff) as u16
    }

    /// The play window's replay count `R`, as in the reference's `windowHi = now + span·R`
    /// (`0x712692`-`0x7126cd`). It draws even when the range cannot change `R`, as the reference
    /// does; skipping the draw de-phases every later roll.
    pub fn replay_count(&mut self, replay: (u32, u32)) -> u32 {
        let (lo, hi) = replay;
        let r = lo + ((u64::from(self.draw()) * u64::from(hi.saturating_sub(lo))) >> 15) as u32;
        r.max(1)
    }

    /// The reference's `srand(GetTickCount())`: a wall-clock seed, once per process. Deviation: a
    /// capture run (`deterministic`) keeps the default seed, so its frames reproduce.
    pub fn seed_for_session(&mut self, deterministic: bool) {
        if deterministic {
            return;
        }
        // `GetTickCount` counts milliseconds since boot; wall-clock milliseconds stand in for it.
        self.0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_millis() as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seed 1 gives the MSVC CRT's documented sequence.
    #[test]
    fn the_draw_is_the_crt_lcg() {
        let mut rng = AnimRng::default();
        let first: Vec<u16> = (0..6).map(|_| rng.draw()).collect();
        assert_eq!(first, vec![41, 18_467, 6_334, 26_500, 19_169, 15_724]);
        assert!(first.iter().all(|&r| r <= 0x7fff));
    }

    #[test]
    fn a_zero_replay_pair_is_one_pass_and_still_advances_the_stream() {
        let mut rng = AnimRng::default();
        assert_eq!(rng.replay_count((0, 0)), 1);
        let mut bare = AnimRng::default();
        bare.draw();
        assert_eq!(
            rng.draw(),
            bare.draw(),
            "the (0,0) window drew exactly once"
        );
    }

    #[test]
    fn a_replay_range_scales_the_window() {
        let mut rng = AnimRng::default();
        // roll 41 over a 0..8 range: 41·8 >> 15 = 0, so the floor still applies.
        assert_eq!(rng.replay_count((0, 8)), 1);
        // A `min` above zero is the floor, whatever the roll.
        let mut rng = AnimRng::default();
        assert!(rng.replay_count((3, 3)) == 3);
        // An inverted pair cannot underflow the subtraction.
        let mut rng = AnimRng::default();
        assert_eq!(rng.replay_count((2, 1)), 2);
    }

    #[test]
    fn a_capture_keeps_the_fixed_seed_and_a_session_does_not() {
        let mut capture = AnimRng::default();
        capture.seed_for_session(true);
        let mut fixed = AnimRng::default();
        assert_eq!(capture.draw(), fixed.draw());

        let mut live = AnimRng::default();
        live.seed_for_session(false);
        assert_ne!(
            live.0,
            AnimRng::default().0,
            "a live session seeds off the wall clock"
        );
    }
}
