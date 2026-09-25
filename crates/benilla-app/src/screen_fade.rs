//! The engine's screen fade to black and back, the reference's general facility:
//! - `0x4c0d10(ecx = completion fn, edx = its arg, [ebp+8] = seconds)` fades out, and the
//!   completion runs when the screen reaches full black, not after a delay;
//! - `0x4c1280(ecx, edx, seconds)` fades in, a no-op unless a fade is up.
//!
//! Completion-driven matters under a hitch: a cinematic shot boundary does a blocking terrain
//! load, and a delay-driven cut would show. The callback stays with the caller: this owns the
//! ramp and exposes [`ScreenFade::is_black`] as a level, and the caller runs its pending step
//! while black. The one caller is `crate::cinematic`.

use bevy::prelude::*;

/// The fade duration at every cinematic boundary: `[0x804550] = 0.25`, in `.rdata`.
pub(crate) const CINEMATIC: f32 = 0.25;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Phase {
    /// Alpha 0, the world visible.
    #[default]
    Clear,
    /// Ramping 0 → 1.
    Out,
    /// Held at 1 until [`ScreenFade::fade_in`]: the caller's step runs here, maybe for many frames.
    Black,
    /// Ramping 1 → 0.
    In,
}

/// The screen fade's state.
#[derive(Resource, Default)]
pub(crate) struct ScreenFade {
    alpha: f32,
    phase: Phase,
    /// Seconds for a full linear 0 to 1 ramp.
    seconds: f32,
}

impl ScreenFade {
    /// Starts going to black; re-arming mid-fade keeps the alpha reached, so the world never
    /// flashes.
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

    /// Comes back from black; a no-op unless a fade is up (`0x4c1280`), since a shot arm's tail
    /// fades in unconditionally.
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

    /// Fully black and holding: the caller's boundary step runs now.
    pub(crate) fn is_black(&self) -> bool {
        self.phase == Phase::Black
    }

    /// No fade in progress and the world fully visible.
    #[cfg(test)]
    fn is_clear(&self) -> bool {
        self.phase == Phase::Clear
    }

    /// Drops the fade with no ramp, for a teardown that must not leave the screen black.
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
            // `PostUpdate`: after this frame's boundary steps, so the cover matches their state.
            .add_systems(PostUpdate, drive_cover);
    }
}

/// One full-screen black node, spawned once and parked hidden.
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
        // Above the loading cover's z-index, 1000: `0x48edd0` dismisses the loading screen at
        // full black. Below the glue screens' 1100, so leaving the world is never trapped.
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
    // One line per edge, for a timeline (`scripts/cine.sh`).
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
        ticks(&mut fade, 5, CINEMATIC / 10.0);
        assert!(!fade.is_black(), "half a ramp is not black");
        ticks(&mut fade, 5, CINEMATIC / 10.0);
        assert!(fade.is_black(), "a full ramp reaches black");
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
        // The `0x4c1280` guard.
        let mut fade = ScreenFade::default();
        fade.fade_in(CINEMATIC);
        assert!(fade.is_clear());
        assert_eq!(fade.alpha, 0.0);
        ticks(&mut fade, 10, CINEMATIC / 10.0);
        assert_eq!(fade.alpha, 0.0, "no black ever appeared");
    }

    #[test]
    fn re_arming_a_fade_out_does_not_restart_from_clear() {
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
