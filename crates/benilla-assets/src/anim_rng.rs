//! The client's **one** `rand()` stream — the MSVC-CRT LCG at `0x7400e5`.
//!
//! **It lives down here, below both sides of decision 1160's line, because both sides draw from
//! it** — `trace`'s placement argument, verbatim, for the same reason. The engine's placed-doodad
//! host and the game's creature, GameObject and portrait-booth arms all roll a variation off this
//! sequence, and in the reference they roll it off the *same cell*: `rand` reads and rewrites
//! `[ptd+0x14]`, one per thread, and every animation arm in the client runs on the primary thread
//! (`_initterm` is pre-`WinMain`; the frame loop `0x420c00` is inline, not spawned). A leaf with no
//! dependencies can be reached by everything; an owner at the top of the stack cannot be reached by
//! the engine at all.
//!
//! **Why one stream and not one per consumer** (decision 0768, and 2301 which found the drift):
//! a shared sequence is what de-syncs a stand of identical props. Not a per-placement seed — just
//! consecutive draws. benilla had four independent streams for a while, three of them seeded 0
//! against the fourth's 1, while all four docstrings claimed to be this one; interleaving is the
//! whole mechanism, so splitting it is not a harmless refactor.
//!
//! **The seed is the reference's, and it is not 1.** `1` is real — the CRT's per-thread default at
//! `_initptd 0x40aca8`, which is what [`AnimRng::default`] returns — but the client overwrites it
//! during CRT static initialization, before `WinMain`: `_initterm` slot **3691** (`.data
//! 0x82a9ac` → the `_dynamic_initializer_for_*` trampoline `0x5b7ff0` → `0x5b8000`) calls
//! `0x5d1c70`, which calls `srand(GetTickCount())` at `0x5d1c8b`. So the reference's variation
//! sequence differs on every run, and [`AnimRng::seed_for_session`] is where benilla says the same
//! thing.
//!
//! **Sequence parity with the reference is not achievable and is not the goal.** Where the stream
//! stands at any given arm depends on every intervening draw — the noise table `Model2::Initialize`
//! builds, the sky LUT, every `CParticleEmitter2` constructor, and FrameXML's own `random()` (bound
//! as a bare global by the embedded `compat.lua`, and called in ordinary play). What benilla
//! reproduces is the *structure*: one stream, wall-clock seeded, shared, non-repeating across runs.

use bevy::prelude::Resource;

/// The shared `rand()` state — see the module note. Seeded by [`Self::seed_for_session`] at
/// startup; [`Default`] is the CRT's own pre-`srand` value so a test or a headless build that
/// never seeds still walks a defined sequence.
#[derive(Resource)]
pub struct AnimRng(u32);

impl Default for AnimRng {
    fn default() -> Self {
        Self(1) // `_initptd 0x40aca8` — the CRT's per-thread default, before `srand` runs
    }
}

impl AnimRng {
    /// One `rand()` draw: `seed = seed·214013 + 2531011`, result `(seed >> 16) & 0x7fff` — the
    /// closed range `[0, 32767]`, 32768 equiprobable outcomes, taken with no scale, mask, modulo
    /// or divide (`0x7400ed`-`0x740101`).
    pub fn draw(&mut self) -> u16 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.0 >> 16) & 0x7fff) as u16
    }

    /// The play-window's **replay count** `R = max(1, min + ((rand()·(max−min)) >> 15))` — the
    /// reference's `windowHi = now + span·R` (`0x712692`-`0x7126cd`). `replay = (0, 0)`, the
    /// overwhelming majority, always yields `R = 1`: one loop per window, so the variation
    /// re-rolls every single pass.
    ///
    /// The draw is taken **unconditionally**, even when it cannot change the answer: it is one
    /// sub-expression of the reference's formula, so the shared stream has to advance the same way
    /// whatever the span. A caller that skips it de-phases every later draw in the process.
    pub fn replay_count(&mut self, replay: (u32, u32)) -> u32 {
        let (lo, hi) = replay;
        let r = lo + ((u64::from(self.draw()) * u64::from(hi.saturating_sub(lo))) >> 15) as u32;
        r.max(1)
    }

    /// The reference's `srand(GetTickCount())` from CRT static init (module note) — a **wall-clock
    /// seed, once per process**, so a fresh session does not replay the last one's variations.
    ///
    /// `deterministic` is the capture carve-out and the only place benilla differs: a golden frame
    /// has to be reproducible, so a capture run keeps the CRT's own pre-`srand` value. That is the
    /// same trade every other sim in the harness makes, at the one site that knows a capture is
    /// running rather than smeared through the lanes that draw.
    pub fn seed_for_session(&mut self, deterministic: bool) {
        if deterministic {
            return;
        }
        // `GetTickCount` is milliseconds since boot; the wall clock's sub-second span is the same
        // kind of number and is what a non-Windows host can offer.
        self.0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_millis() as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The LCG itself, against the bytes at `0x7400ed`-`0x740101` — and against the CRT's own
    /// documented sequence from seed 1, which is the one value a reader can check anywhere.
    #[test]
    fn the_draw_is_the_crt_lcg() {
        let mut rng = AnimRng::default();
        let first: Vec<u16> = (0..6).map(|_| rng.draw()).collect();
        assert_eq!(first, vec![41, 18_467, 6_334, 26_500, 19_169, 15_724]);
        // Every draw is inside the closed range the `and eax,0x7fff` produces.
        assert!(first.iter().all(|&r| r <= 0x7fff));
    }

    /// `(0, 0)` — the overwhelming majority of shipped sequences — is one pass, and it still costs
    /// a draw. The second half is the part a "tidy" rewrite breaks: skipping the draw when it
    /// cannot change the answer de-phases every later roll in the process.
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

    /// The window scales with the range, and never below one pass.
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

    /// A seeded session does not replay the default sequence — the whole point of
    /// `srand(GetTickCount())` — and a capture run does.
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
