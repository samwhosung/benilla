//! The engine's screen fade — the whole frame to black, and back.
//!
//! This is the reference's own facility, not a cinematic's private effect. Two entry points, both
//! taking a duration in seconds:
//!
//! - **`0x4c0d10(ecx = completion fn, edx = its arg, [ebp+8] = seconds)` — fade OUT to black.** It
//!   latches the completion pair and arms the fade; **the completion runs when the screen reaches
//!   full black**, not after a delay on a parallel clock.
//! - **`0x4c1280(ecx, edx, seconds)` — fade IN from black.** A no-op unless a fade is currently up.
//!
//! That distinction is the whole reason this module exists rather than a `Timer` at each call site.
//! A cinematic boundary is not "wait 0.25 s, then advance" — it is "go black, and *when you are
//! black*, advance". The two coincide only while the frame rate holds; under a hitch (and the shot
//! boundary is exactly where the reference does a blocking terrain load) a delay-driven sequence
//! shows the cut, and a completion-driven one cannot.
//!
//! **The completion callback does not cross into this module.** The reference stores a function
//! pointer; an ECS system cannot, and faking it with a boxed closure in a `Resource` would buy
//! nothing. Instead this owns only the ramp — alpha and phase — and exposes [`ScreenFade::is_black`]
//! as a *level*. The caller keeps its own "which step is pending" state and runs it while black.
//! Same sequencing, with the step left where it can see the data it needs.
//!
//! The one caller today is `crate::cinematic`. It is written as the general facility because that
//! is what it is in the reference (`0x4c0d10` has callers well outside the cinematic band), not in
//! anticipation of a second one here.

use bevy::prelude::*;

/// The reference's fade duration at every cinematic boundary: `[0x804550] = 0.25`, in `.rdata` —
/// genuinely read-only, so this is a constant in the image and not some global's default.
pub(crate) const CINEMATIC: f32 = 0.25;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Phase {
    /// Nothing up: alpha 0, the world visible.
    #[default]
    Clear,
    /// Ramping 0 → 1.
    Out,
    /// Held at 1. **The fade stays here until someone calls [`ScreenFade::fade_in`]** — it does not
    /// bounce on its own, because the caller's step runs in this phase and may take many frames.
    Black,
    /// Ramping 1 → 0.
    In,
}

/// The screen fade's state. Drive it with [`fade_out`](ScreenFade::fade_out) /
/// [`fade_in`](ScreenFade::fade_in); read it with [`is_black`](ScreenFade::is_black).
#[derive(Resource, Default)]
pub(crate) struct ScreenFade {
    alpha: f32,
    phase: Phase,
    /// Seconds for a full 0→1 traverse; the ramp is linear in time over this.
    seconds: f32,
}

impl ScreenFade {
    /// Start going to black. Re-arming while already fading out keeps the alpha reached so far, so
    /// a second request cannot restart the ramp from clear and show a flash of world.
    pub(crate) fn fade_out(&mut self, seconds: f32) {
        if self.phase == Phase::Black {
            return;
        }
        self.seconds = seconds.max(0.0);
        self.phase = Phase::Out;
        if self.seconds == 0.0 {
            self.alpha = 1.0;
            self.phase = Phase::Black;
        }
    }

    /// Come back from black. **A no-op unless a fade is currently up** — the reference's own
    /// guard at `0x4c1280`, which matters because the tail of a shot arm fades in unconditionally
    /// and must not paint a black ramp over a screen that was never darkened.
    pub(crate) fn fade_in(&mut self, seconds: f32) {
        if self.phase == Phase::Clear {
            return;
        }
        self.seconds = seconds.max(0.0);
        self.phase = Phase::In;
        if self.seconds == 0.0 {
            self.alpha = 0.0;
            self.phase = Phase::Clear;
        }
    }

    /// The screen is fully black **and holding**: the caller's boundary step runs now.
    pub(crate) fn is_black(&self) -> bool {
        self.phase == Phase::Black
    }

    /// Nothing up — no fade in progress and the world fully visible.
    ///
    /// Only the tests ask this today. It is the exact complement of the state `fade_in` refuses to
    /// act on, so it is the honest way to assert a ramp came all the way home; it is not carried
    /// as speculative API for a caller that does not exist.
    #[cfg(test)]
    fn is_clear(&self) -> bool {
        self.phase == Phase::Clear
    }

    /// Drop the fade with no ramp. For a teardown that must not leave the screen black behind it
    /// (leaving the world mid-cinematic, a failed start), where a graceful fade-in has no frames
    /// left to run in.
    pub(crate) fn clear(&mut self) {
        self.alpha = 0.0;
        self.phase = Phase::Clear;
    }

    fn tick(&mut self, dt: f32) {
        let step = if self.seconds > 0.0 {
            dt / self.seconds
        } else {
            1.0
        };
        match self.phase {
            Phase::Out => {
                self.alpha = (self.alpha + step).min(1.0);
                if self.alpha >= 1.0 {
                    self.phase = Phase::Black;
                }
            }
            Phase::In => {
                self.alpha = (self.alpha - step).max(0.0);
                if self.alpha <= 0.0 {
                    self.phase = Phase::Clear;
                }
            }
            Phase::Clear | Phase::Black => {}
        }
    }
}

#[derive(Component)]
struct FadeCover;

pub(crate) struct ScreenFadePlugin;

impl Plugin for ScreenFadePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScreenFade>()
            .add_systems(Startup, spawn_cover)
            // `PostUpdate`: every boundary step this frame — the cinematic's arm, advance and end —
            // has already run, so the cover drawn is the one that matches the state the frame will
            // actually present. Ticking before them would advance the ramp on last frame's answer.
            .add_systems(PostUpdate, drive_cover);
    }
}

/// One full-screen black node, spawned once and parked hidden — the loading cover's own shape.
fn spawn_cover(mut commands: Commands) {
    commands.spawn((
        FadeCover,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::NONE),
        // **Above the loading cover (1000).** The reference hands the world-enter loading screen
        // over to the cinematic *under* the black: `0x48edd0`'s first instruction dismisses the
        // loading screen, and it runs at full black. A fade below the cover could not
        // do that — the cover would be the thing on screen, and its dismissal would be a visible
        // cut. Below the glue screens (1100) so a state change out of the world can never be
        // trapped behind a fade that has no one left to clear it.
        GlobalZIndex(1050),
        Visibility::Hidden,
    ));
}

fn drive_cover(
    time: Res<Time>,
    mut fade: ResMut<ScreenFade>,
    mut cover: Query<(&mut BackgroundColor, &mut Visibility), With<FadeCover>>,
    mut was: Local<Option<Phase>>,
) {
    fade.tick(time.delta_secs());
    // One line per edge, so a timeline (`scripts/cine.sh`) can see the boundaries at all. Four
    // transitions per fade and two boundaries per single-shot cinematic is eight lines a run;
    // without them the fade is invisible to every instrument we have.
    if *was != Some(fade.phase) {
        match fade.phase {
            Phase::Out => info!("screen fade: out"),
            Phase::Black => info!("screen fade: black"),
            Phase::In => info!("screen fade: in"),
            Phase::Clear => info!("screen fade: clear"),
        }
        *was = Some(fade.phase);
    }
    for (mut color, mut vis) in &mut cover {
        let want = if fade.alpha > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
        let next = Color::srgba(0.0, 0.0, 0.0, fade.alpha);
        if color.0 != next {
            color.0 = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticks(fade: &mut ScreenFade, n: usize, dt: f32) {
        for _ in 0..n {
            fade.tick(dt);
        }
    }

    #[test]
    fn a_fade_out_reaches_black_and_holds_there() {
        let mut fade = ScreenFade::default();
        fade.fade_out(CINEMATIC);
        // Half the duration is half way, and emphatically NOT black yet.
        ticks(&mut fade, 5, CINEMATIC / 10.0);
        assert!(!fade.is_black(), "half a ramp is not black");
        ticks(&mut fade, 5, CINEMATIC / 10.0);
        assert!(fade.is_black(), "a full ramp reaches black");
        // …and stays. The caller's boundary step may take many frames; the fade must not bounce
        // back on its own while it runs.
        ticks(&mut fade, 100, CINEMATIC);
        assert!(fade.is_black(), "black holds until fade_in");
    }

    #[test]
    fn fade_in_from_black_returns_to_clear() {
        let mut fade = ScreenFade::default();
        fade.fade_out(CINEMATIC);
        ticks(&mut fade, 10, CINEMATIC / 10.0);
        assert!(fade.is_black());
        fade.fade_in(CINEMATIC);
        ticks(&mut fade, 10, CINEMATIC / 10.0);
        assert!(fade.is_clear(), "a full ramp back reaches clear");
        assert_eq!(fade.alpha, 0.0);
    }

    #[test]
    fn fade_in_on_a_clear_screen_does_nothing() {
        // The reference's `0x4c1280` guard. The tail of the shot arm fades in unconditionally;
        // on a screen that was never darkened that must not start a ramp *from* black.
        let mut fade = ScreenFade::default();
        fade.fade_in(CINEMATIC);
        assert!(fade.is_clear());
        assert_eq!(fade.alpha, 0.0);
        ticks(&mut fade, 10, CINEMATIC / 10.0);
        assert_eq!(fade.alpha, 0.0, "no black ever appeared");
    }

    #[test]
    fn re_arming_a_fade_out_does_not_restart_from_clear() {
        // Two boundaries landing close together must not flash the world between them.
        let mut fade = ScreenFade::default();
        fade.fade_out(CINEMATIC);
        ticks(&mut fade, 5, CINEMATIC / 10.0);
        let half = fade.alpha;
        assert!(half > 0.0 && half < 1.0);
        fade.fade_out(CINEMATIC);
        assert_eq!(fade.alpha, half, "the alpha reached so far is kept");
    }

    #[test]
    fn a_zero_length_fade_is_instant_in_both_directions() {
        let mut fade = ScreenFade::default();
        fade.fade_out(0.0);
        assert!(fade.is_black(), "no frames to ramp over: black now");
        fade.fade_in(0.0);
        assert!(fade.is_clear());
    }

    #[test]
    fn clear_drops_a_fade_with_no_ramp() {
        let mut fade = ScreenFade::default();
        fade.fade_out(CINEMATIC);
        ticks(&mut fade, 10, CINEMATIC / 10.0);
        assert!(fade.is_black());
        fade.clear();
        assert!(fade.is_clear());
        assert_eq!(fade.alpha, 0.0, "a teardown leaves no black behind it");
    }
}
