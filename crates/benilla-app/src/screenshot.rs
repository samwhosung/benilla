//! Print screen: the capture, the file writer and the "Screen Captured" status, which must never
//! appear in the shot it announces. The binding runs `TakeScreenshot()` (`WorldFrame.lua:46`),
//! which hides `ScreenshotStatus` and calls `Screenshot()`; the engine later answers
//! `SCREENSHOT_SUCCEEDED` or `SCREENSHOT_FAILED`.
//!
//! Our UI quads are built at the top of `UiInput` and the binding runs at its bottom, so a capture
//! on the press frame would still show a second press's fading status; [`ask_for_captures`]
//! therefore holds each ask exactly one frame.
//!
//! Deviations:
//! - PNG, not TGA, because a 32-bit Targa of a modern window is about 8 MB and few tools open it.
//! - `benilla-config/Screenshots/`, not the install's, because benilla never writes the install.
//!
//! The reference fires `SCREENSHOT_SUCCEEDED` even when the write fails, a bug not copied:
//! [`report_captures`] answers `SCREENSHOT_FAILED` and logs the reason.
//!
//! As in the reference, a raw `Screenshot()` from Lua skips the `Hide()`. The glue screens'
//! capture keys are not built.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::tasks::IoTaskPool;

use benilla_ui::script::UiScript;

use crate::ui_script::{UiFeed, UiInput};

/// The reference's file stem.
const STEM: &str = "WoWScrnShot";

/// The writer thread's answer.
enum Outcome {
    Saved(std::path::PathBuf),
    Failed(String),
}

/// The writer threads' channel and the naming clock.
#[derive(Resource)]
struct ScreenshotState {
    tx: crossbeam_channel::Sender<Outcome>,
    rx: crossbeam_channel::Receiver<Outcome>,
    /// The last name's epoch second and its count; counted here, not probed on disk, because the
    /// worker may not have created the previous file yet.
    last_second: i64,
    within_second: u32,
    /// Captures asked for on the previous frame (module doc).
    pending: u32,
}

impl Default for ScreenshotState {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self {
            tx,
            rx,
            last_second: i64::MIN,
            within_second: 0,
            pending: 0,
        }
    }
}

impl ScreenshotState {
    /// `WoWScrnShot_MMDDYY_HHMMSS.png`, the reference's field order and padding.
    ///
    /// A second capture in the same second gets `_2`, `_3`: the reference overwrites, so a burst
    /// leaves one file, a data loss not copied. Deviation: UTC, not local time, because there is
    /// no timezone source (as for Lua `date()`, [`benilla_ui::civil`]).
    fn next_name(&mut self, now: i64) -> String {
        if now == self.last_second {
            self.within_second += 1;
        } else {
            self.last_second = now;
            self.within_second = 0;
        }
        let c = benilla_ui::civil::from_unix(now);
        let base = format!(
            "{STEM}_{:02}{:02}{:02}_{:02}{:02}{:02}",
            c.month,
            c.day,
            c.year.rem_euclid(100),
            c.hour,
            c.min,
            c.sec
        );
        match self.within_second {
            0 => format!("{base}.png"),
            n => format!("{base}_{}.png", n + 1),
        }
    }
}

/// Captures each `Screenshot()` one frame after the call. Spawn last frame's asks before draining
/// this frame's: that order is the one-frame hold (module doc).
fn ask_for_captures(
    mut commands: Commands,
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<ScreenshotState>,
) {
    for _ in 0..std::mem::take(&mut state.pending) {
        commands
            .spawn(Screenshot::primary_window())
            .observe(write_capture);
    }
    if let Some(mut script) = script {
        state.pending += script.take_screenshot_asks();
    }
}

/// Names the file and hands the encode and write to a worker, off the frame.
fn write_capture(captured: On<ScreenshotCaptured>, mut state: ResMut<ScreenshotState>) {
    // No folder under a `$WOW_CAPTURE` run or without an exe directory: report it as a failure.
    let Some(dir) = crate::local_state::screenshots_dir() else {
        let _ = state.tx.send(Outcome::Failed(
            "no benilla-config folder to write into".into(),
        ));
        return;
    };
    let path = dir.join(state.next_name(benilla_ui::civil::unix_seconds()));

    // Drop alpha: on an HDR target it carries brightness, not opacity.
    let dynamic = match captured.image.clone().try_into_dynamic() {
        Ok(img) => img.to_rgb8(),
        Err(e) => {
            let _ = state
                .tx
                .send(Outcome::Failed(format!("unreadable frame: {e}")));
            return;
        }
    };

    let tx = state.tx.clone();
    IoTaskPool::get()
        .spawn(async move {
            let outcome = std::fs::create_dir_all(&dir)
                .and_then(|()| {
                    dynamic
                        .save_with_format(&path, image::ImageFormat::Png)
                        .map_err(std::io::Error::other)
                })
                .map(|()| Outcome::Saved(path.clone()))
                .unwrap_or_else(|e| Outcome::Failed(format!("{}: {e}", path.display())));
            let _ = tx.send(outcome);
        })
        .detach();
}

/// Fires `SCREENSHOT_SUCCEEDED` or `SCREENSHOT_FAILED` for finished writes, always a frame after
/// the capture, so the status cannot be in the picture.
fn report_captures(script: Option<NonSendMut<UiScript>>, state: Res<ScreenshotState>) {
    let Some(mut script) = script else {
        return;
    };
    while let Ok(outcome) = state.rx.try_recv() {
        match outcome {
            Outcome::Saved(path) => {
                // At info: the folder is not where WoW puts it.
                info!("screenshot: wrote {}", path.display());
                script.fire_event("SCREENSHOT_SUCCEEDED", Vec::new());
            }
            Outcome::Failed(why) => {
                warn!("screenshot: FAILED — {why}");
                script.fire_event("SCREENSHOT_FAILED", Vec::new());
            }
        }
    }
}

pub(crate) struct ScreenshotPlugin;

impl Plugin for ScreenshotPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScreenshotState>().add_systems(
            Update,
            (
                report_captures.in_set(UiFeed),
                ask_for_captures
                    .after(UiInput)
                    .after(crate::bindings::BindingSet),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_reference_and_never_collide() {
        let mut s = ScreenshotState::default();
        // 2026-08-21T13:45:07Z.
        assert_eq!(s.next_name(1_787_319_907), "WoWScrnShot_082126_134507.png");
        assert_eq!(
            s.next_name(1_787_319_907),
            "WoWScrnShot_082126_134507_2.png"
        );
        assert_eq!(
            s.next_name(1_787_319_907),
            "WoWScrnShot_082126_134507_3.png"
        );
        assert_eq!(s.next_name(1_787_319_908), "WoWScrnShot_082126_134508.png");
        // 2001-01-02T03:04:05Z: every field needs its `%02d` zero.
        assert_eq!(s.next_name(978_404_645), "WoWScrnShot_010201_030405.png");
    }

    /// An ask made during frame N is spawned on frame N+1.
    #[test]
    fn an_ask_is_held_for_one_frame_before_the_shutter() {
        let mut app = App::new();
        app.init_resource::<ScreenshotState>();
        // No VM: the asks are pushed by hand.
        app.add_systems(Update, spawn_pending_only);

        // Frame N: the binding asked.
        app.world_mut().resource_mut::<ScreenshotState>().pending = 0;
        app.world_mut().resource_mut::<ScreenshotState>().pending += 1;
        let staged = app.world().resource::<ScreenshotState>().pending;
        assert_eq!(staged, 1, "the ask is staged, not yet a capture");

        // Frame N+1.
        app.update();
        assert_eq!(
            app.world().resource::<ScreenshotState>().pending,
            0,
            "the held ask is consumed on the next frame"
        );
        assert_eq!(
            app.world_mut()
                .query::<&Screenshot>()
                .iter(app.world())
                .count(),
            1,
            "and exactly one capture is requested"
        );

        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&Screenshot>()
                .iter(app.world())
                .count(),
            1
        );
    }

    /// The real [`ask_for_captures`] with no VM, as at character select.
    fn spawn_pending_only(commands: Commands, state: ResMut<ScreenshotState>) {
        ask_for_captures(commands, None, state);
    }
}
