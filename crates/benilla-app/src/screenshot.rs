//! Print screen: the capture, the file writer and the "Screen Captured" status, which must never
//! appear in the shot it announces. The binding runs `TakeScreenshot()` (`WorldFrame.lua:46`),
//! which hides `ScreenshotStatus` and calls `Screenshot()`; the engine later answers
//! `SCREENSHOT_SUCCEEDED` or `SCREENSHOT_FAILED`. The glue screens call `Screenshot()` directly
//! on `PRINTSCREEN` (`AccountLogin.lua:81`); the same capture answers them with the glue's
//! `GLUE_SCREENSHOT_SUCCEEDED` or `GLUE_SCREENSHOT_FAILED` (`0x46c520`), whichever VM is up
//! ([`UiScript::screenshot_event`]).
//!
//! The binding runs in `UiInput`, where `TakeScreenshot()` hides the status inside its call; the
//! ask drains in `UiDrain`, after the tick and before the paint builds the frame's quads, so the
//! press frame's own image no longer shows the status. [`ask_for_captures`] still holds each ask
//! one frame, which this order no longer needs.
//!
//! Deviation: the file is a PNG where the reference writes a TGA (`0x4426f0`, the name
//! `%s\WoWScrnShot_%s.tga` at `0x834c44`), because a 32-bit Targa of a modern window is about
//! 8 MB and few tools open it. The folder is [`crate::local_state::screenshots_dir`]'s.
//!
//! Deviation: a failed write answers `SCREENSHOT_FAILED` and logs the reason
//! ([`report_captures`]), where the reference fires `SCREENSHOT_SUCCEEDED`, because its capture
//! returns 1 whenever it had a buffer and discards the write's result (`0x442792`, `0x4427a2`);
//! announcing a shot that was never saved is a defect.
//!
//! As in the reference, a raw `Screenshot()` from Lua skips the `Hide()`.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::tasks::IoTaskPool;

use crate::ui_script::UiFeed;

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
    /// `WoWScrnShot_MMDDYY_HHMMSS.png`, the reference's field order and padding, in local time
    /// (`0x429450`, `strftime` of `localtime`).
    ///
    /// A second capture in the same second gets `_2`, `_3`: the reference overwrites, so a burst
    /// leaves one file, a data loss not copied.
    fn next_name(&mut self, now: i64) -> String {
        if now == self.last_second {
            self.within_second += 1;
        } else {
            self.last_second = now;
            self.within_second = 0;
        }
        let (c, _) = benilla_ui::civil::local(now);
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
/// this frame's: that order is the one-frame hold (module doc). While a movie plays the frame pass
/// skips its present block, the capture in it (`0x44267c`), so the asks wait for its end. Not a
/// call's: the verb only arms the request (`0x4893d0`, `0x442a10`), which the frame pass takes.
fn ask_for_captures(
    mut commands: Commands,
    mut hosted_vm: crate::script_calls::HostedVm,
    mut state: ResMut<ScreenshotState>,
) {
    hosted_vm.run(|script| {
        if script
            .as_deref()
            .is_some_and(benilla_ui::script::UiScript::movie_playing)
        {
            return;
        }
        spawn_captures(&mut commands, std::mem::take(&mut state.pending));
        if let Some(script) = script {
            state.pending += script.take_screenshot_asks();
        }
    })
}

/// `count` captures, each a screenshot of the primary window.
fn spawn_captures(commands: &mut Commands, count: u32) {
    for _ in 0..count {
        commands
            .spawn(Screenshot::primary_window())
            .observe(write_capture);
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

/// Answers finished writes with the VM's success or failure event, always a frame after the
/// capture, so the status cannot be in the picture.
fn report_captures(mut hosted_vm: crate::script_calls::HostedVm, state: Res<ScreenshotState>) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        while let Ok(outcome) = state.rx.try_recv() {
            match outcome {
                Outcome::Saved(path) => {
                    // At info: the folder is not where WoW puts it.
                    info!("screenshot: wrote {}", path.display());
                    let event = script.screenshot_event(true);
                    script.fire_event(event, Vec::new());
                }
                Outcome::Failed(why) => {
                    warn!("screenshot: FAILED — {why}");
                    let event = script.screenshot_event(false);
                    script.fire_event(event, Vec::new());
                }
            }
        }
    })
}

pub(crate) struct ScreenshotPlugin;

impl Plugin for ScreenshotPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScreenshotState>().add_systems(
            Update,
            (
                report_captures.in_set(UiFeed),
                ask_for_captures
                    .in_set(crate::ui_script::UiDrain)
                    .after(crate::bindings::BindingSet),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::civil::ZoneGuard;
    use benilla_ui::script::UiScript;

    #[test]
    fn names_follow_the_reference_and_never_collide() {
        let _zone = ZoneGuard::set("UTC0");
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

    /// The stamp is the local time (`0x429450`, `strftime` of `localtime`): 5:45 east of UTC,
    /// 13:45:07Z is 19:30:07, and 20:00Z is past local midnight.
    #[test]
    fn names_stamp_the_local_time() {
        let _zone = ZoneGuard::set("NPT-05:45");
        let mut s = ScreenshotState::default();
        assert_eq!(s.next_name(1_787_319_907), "WoWScrnShot_082126_193007.png");
        assert_eq!(s.next_name(1_787_342_400), "WoWScrnShot_082226_014500.png");
    }

    /// The zone is the process's, read once (`0x414568`): a test holds its own through the guard,
    /// so the stamp follows each zone set in turn, whatever ran before.
    #[test]
    fn the_stamp_follows_the_zone_a_test_holds() {
        for (tz, name) in [
            ("NPT-05:45", "WoWScrnShot_082126_193007.png"),
            ("UTC0", "WoWScrnShot_082126_134507.png"),
            ("NPT-05:45", "WoWScrnShot_082126_193007.png"),
        ] {
            let _zone = ZoneGuard::set(tz);
            let mut s = ScreenshotState::default();
            assert_eq!(s.next_name(1_787_319_907), name, "TZ {tz}");
        }
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

    /// The glue's `Screenshot()` goes through the same ask and the same writer answer: a capture
    /// is spawned a frame after the call, and a finished write fires the glue events (`0x46c520`).
    #[test]
    fn a_glue_screen_screenshot_is_one_capture_answered_with_the_glue_events() {
        let mut app = App::new();
        app.init_resource::<ScreenshotState>();
        let glue = UiScript::new_glue().unwrap();
        glue.run(
            r#"
            heard = {}
            f = CreateFrame("Frame")
            f:SetScript("OnEvent", function() table.insert(heard, event) end)
            f:RegisterEvent("GLUE_SCREENSHOT_SUCCEEDED")
            f:RegisterEvent("GLUE_SCREENSHOT_FAILED")
            Screenshot()
            "#,
        )
        .unwrap();
        app.insert_non_send_resource(glue);
        app.add_systems(Update, (ask_for_captures, report_captures).chain());

        app.update();
        assert_eq!(
            app.world().resource::<ScreenshotState>().pending,
            1,
            "the glue call is staged as an ask"
        );
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&Screenshot>()
                .iter(app.world())
                .count(),
            1,
            "and one capture follows a frame later"
        );

        let tx = app.world().resource::<ScreenshotState>().tx.clone();
        tx.send(Outcome::Saved("x.png".into())).unwrap();
        tx.send(Outcome::Failed("disk".into())).unwrap();
        app.update();
        let heard = app
            .world()
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("return heard")
            .unwrap();
        assert_eq!(
            heard,
            ["GLUE_SCREENSHOT_SUCCEEDED", "GLUE_SCREENSHOT_FAILED"]
        );
    }

    /// The real [`ask_for_captures`] with no VM, as at character select.
    /// While a movie plays the frame pass presents nothing, so an ask waits (`0x44267c`): no
    /// capture until the movie stops, then the held one.
    #[test]
    fn an_ask_waits_while_a_movie_plays() {
        let mut s = benilla_ui::script::UiScript::new_glue().unwrap();
        s.set_movie_reader(Box::new(|path, _| {
            path.ends_with(".avi").then(|| {
                benilla_movie::testing::synthetic_avi(1024, 464, (1, 25), 9, &[&[][..]; 9], &[])
            })
        }));
        s.run(r#"M = CreateFrame("MovieFrame", "MovieProbe") M:StartMovie("A")"#)
            .unwrap();
        let mut app = App::new();
        app.init_resource::<ScreenshotState>();
        app.insert_non_send_resource(s);
        app.add_systems(Update, ask_for_captures);
        app.world_mut().resource_mut::<ScreenshotState>().pending = 1;
        let shots = |app: &mut App| {
            app.world_mut()
                .query::<&Screenshot>()
                .iter(app.world())
                .count()
        };
        app.update();
        app.update();
        assert_eq!(shots(&mut app), 0, "held while the movie plays");
        app.world()
            .non_send_resource::<benilla_ui::script::UiScript>()
            .run("M:StopMovie()")
            .unwrap();
        app.update();
        assert_eq!(shots(&mut app), 1, "taken once it stops");
    }

    fn spawn_pending_only(mut commands: Commands, mut state: ResMut<ScreenshotState>) {
        spawn_captures(&mut commands, std::mem::take(&mut state.pending));
    }
}
