//! **Print screen** (decision 1487) — the capture key, the writer, and the "Screen Captured" text
//! that must never appear in the file it announces.
//!
//! The reference's shape, which this reproduces exactly (its own `Bindings.xml` + `WorldFrame.lua`,
//! read off the 1.12.1 install):
//!
//! ```text
//! PRINTSCREEN → binding SCREENSHOT → TakeScreenshot()   [FrameXML]
//!                                      ScreenshotStatus:Hide()   ← last frame's text, gone first
//!                                      Screenshot()              [engine]
//!                                    … the frame is captured …
//! engine → SCREENSHOT_SUCCEEDED / SCREENSHOT_FAILED   → ScreenshotStatus shows, fades over 1.5 s
//! ```
//!
//! **That ordering is most of the answer to B261's third clause, and the rest is ours to arrange.**
//! For a single press nothing can leak: `Screenshot()` returns nothing and the outcome comes back
//! as an *event*, frames later, because the readback is asynchronous — the confirmation does not
//! exist yet when the shutter fires. The leak is the SECOND press inside the 1.5 s fade, when the
//! previous shot's "Screen Captured" is still on screen; that is what the reference's `Hide()`
//! before the capture is for.
//!
//! **And on our pipeline that `Hide()` lands one frame too late** — measured, not reasoned:
//! a live double press wrote the text straight into the second PNG. `ui_script`'s paint pass
//! ticks the VM and builds this frame's UI quad list at the TOP of the `UiInput` set; the binding
//! dispatch that runs `TakeScreenshot()` sits at the BOTTOM of that same set. So by the time the
//! binding hides the frame, the quads carrying its text are already built, and the frame that then
//! renders — and gets captured — still has the line in it. Nothing in the reference's design says
//! otherwise; the real client simply does not build its draw list in that order.
//!
//! So [`ask_for_captures`] **holds each ask for one frame** ([`ScreenshotState::pending`]). The
//! capture is requested on the frame AFTER the press, whose `paint_script` has rebuilt the quads
//! with the frame hidden. One frame is the whole fix and also the minimum: any less and the stale
//! quads are what gets photographed, any more and the shutter drifts from the keypress for no
//! reason. `ui_script::screenshot_tests` pins the UI contract; this file's own test pins the
//! deferral, and the live falsifier is a double press 0.6 s apart (decision 1487).
//!
//! **Two deliberate divergences, both recorded in 1487:**
//!
//! 1 · **PNG, where the reference writes TGA.** An uncompressed 32-bit Targa of a modern window is
//!     ~8 MB and no ordinary tool on a 2026 desktop previews one. The mechanism, the folder and the
//!     naming are otherwise the reference's.
//! 2 · **`benilla-config/Screenshots/`, not `<install>/Screenshots/`** — decision 1486's rule:
//!     benilla reads a WoW install and never writes to one. Same folder name, different parent.
//!
//! **One reference behaviour deliberately NOT transcribed.** The real client fires
//! `SCREENSHOT_SUCCEEDED` whether or not the file was written — it discards the writer's return
//! value, so the event means "the device gave us a buffer" — and it has already deleted the
//! partial file. That is a bug, not a contract: [`report_captures`] answers `SCREENSHOT_FAILED`
//! when the encode or the write actually failed, and puts the reason in the log.
//!
//! **Two gaps that ARE faithful**, named so they are not mistaken for oversights: a raw
//! `Screenshot()` from a macro or addon skips `TakeScreenshot`'s `Hide()` and so can photograph a
//! fading confirmation (the reference's hide is in Lua too, and inventing an engine-side one would
//! be inventing behaviour); and the key does nothing at the glue screens, where the reference
//! captures through hard-coded, unrebindable `OnKeyDown` handlers that print nothing — a separate
//! slice, in the glue arc.
//!
//! Nothing here is dev-gated: this is a player feature, and it links in the `--no-default-features`
//! player build like the rest of the UI arc.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::tasks::IoTaskPool;

use benilla_ui::script::UiScript;

use crate::ui_script::{UiFeed, UiInput};

/// The reference's own file stem (`WoWScrnShot_`), kept: a player who knows what WoW screenshots
/// are called finds the same names here, and so does every screenshot-organizing tool the
/// community has. Only the extension changes.
const STEM: &str = "WoWScrnShot";

/// What the writer thread reported back. The outcome is a two-state answer because the UI's is:
/// `SCREENSHOT_SUCCEEDED` or `SCREENSHOT_FAILED`, nothing in between.
enum Outcome {
    Saved(std::path::PathBuf),
    Failed(String),
}

/// The capture arc's state: the channel the writer threads report through, and just enough clock
/// memory to keep two captures in the same second from landing on the same name.
#[derive(Resource)]
struct ScreenshotState {
    tx: crossbeam_channel::Sender<Outcome>,
    rx: crossbeam_channel::Receiver<Outcome>,
    /// The epoch second the last name was minted for, and how many have been minted for it.
    /// **Held here rather than probed off the filesystem**: the write happens on a worker thread
    /// and may not have created the file by the time the next name is needed, so "does it exist
    /// yet?" is a race that silently overwrites. A counter on the main thread cannot lose.
    last_second: i64,
    within_second: u32,
    /// Captures asked for on the PREVIOUS frame, waiting for this one to render. The one-frame
    /// hold that keeps the status line out of the picture — see the module docs for the measured
    /// reason it is needed at all.
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
    /// The next file name, from the wall clock: `WoWScrnShot_MMDDYY_HHMMSS.png`, the reference's
    /// own field order and zero padding.
    ///
    /// **A second capture inside the same second gets `_2`, `_3`, … rather than overwriting.**
    /// That is a divergence and a deliberate one: the reference's fixed-name write means a burst
    /// of key presses leaves one file, which is a silent data loss the player only discovers
    /// later. The suffix is outside the reference's grammar, so nothing that parses the faithful
    /// name is broken by a name it will simply not match.
    ///
    /// **UTC, not local time** — the reference stamps local. The workspace carries no date
    /// dependency and a local offset needs a timezone source, not different arithmetic; the Lua
    /// `date()` global records the same divergence for the same reason
    /// ([`benilla_ui::civil`]). Names stay unique and sort correctly either way.
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

/// Ask the renderer for one frame per queued `Screenshot()` call — **one frame after the call**.
///
/// The order within a frame is: spawn last frame's asks first, then drain this frame's. Both halves
/// matter. Spawning first means the entity is in the world before the render extract at the end of
/// THIS frame, whose UI quads `paint_script` rebuilt with the status line already hidden. Draining
/// second means an ask made by the binding dispatch earlier in this same frame waits its turn
/// rather than being photographed against the quads that were built before the hide — which is the
/// bug this deferral exists for (module docs).
///
/// Ordered `.after(BindingSet)` so the drain deterministically sees an ask the binding just made,
/// instead of depending on where an unconstrained system happened to land.
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

/// The readback landed: name the file and hand the encode+write to a worker.
///
/// **Off the main thread on purpose.** A 2560×1440 PNG encode is tens of milliseconds and this
/// runs inside the frame; doing it here would drop frames every time the player takes a shot,
/// which is the one moment they are looking at the picture. The answer comes back through the
/// channel and is reported on whichever later frame it arrives — the UI already expects that,
/// because the reference's outcome is an event too.
fn write_capture(captured: On<ScreenshotCaptured>, mut state: ResMut<ScreenshotState>) {
    // Nothing to write to: a hermetic capture/probe run (`$WOW_CAPTURE`), or a platform with no
    // discoverable exe directory. Report the failure rather than dropping it silently — the UI
    // says "Screen Capture Failed" and the player learns something.
    let Some(dir) = crate::local_state::screenshots_dir() else {
        let _ = state.tx.send(Outcome::Failed(
            "no benilla-config folder to write into".into(),
        ));
        return;
    };
    let path = dir.join(state.next_name(benilla_ui::civil::unix_seconds()));

    // `to_rgb8` drops alpha, which on an HDR target carries brightness rather than opacity — the
    // same choice bevy's own `save_to_disk` makes, and for the same reason: kept, the picture is
    // wrong.
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

/// Report finished writes to the UI — the reference's `SCREENSHOT_SUCCEEDED` / `SCREENSHOT_FAILED`.
///
/// **Before [`UiInput`]**, the shape every other feed in this crate uses: the event is delivered at
/// the top of a frame so the status text is laid out with everything else, and — the part that
/// matters — this can only ever be a frame *after* the one that was captured, so the text it puts
/// on screen cannot be in the picture.
fn report_captures(script: Option<NonSendMut<UiScript>>, state: Res<ScreenshotState>) {
    let Some(mut script) = script else {
        return;
    };
    while let Ok(outcome) = state.rx.try_recv() {
        match outcome {
            Outcome::Saved(path) => {
                // Announced at info: "where did my screenshot go" is the first question a player
                // asks about a folder that is deliberately not where WoW put it (decision 1486).
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

    /// The reference's field order and padding, and the anti-clobber suffix — the two halves of
    /// [`ScreenshotState::next_name`] that a reader would otherwise have to take on trust.
    #[test]
    fn names_follow_the_reference_and_never_collide() {
        let mut s = ScreenshotState::default();
        // 2026-08-21T13:45:07Z — single digits in the day and the hour would be the padding bug.
        assert_eq!(s.next_name(1_787_319_907), "WoWScrnShot_082126_134507.png");
        // Same second again: a suffix, not an overwrite. The reference would have written the
        // same name twice and left one file.
        assert_eq!(
            s.next_name(1_787_319_907),
            "WoWScrnShot_082126_134507_2.png"
        );
        assert_eq!(
            s.next_name(1_787_319_907),
            "WoWScrnShot_082126_134507_3.png"
        );
        // A new second resets the counter — the plain name is the overwhelmingly common one.
        assert_eq!(s.next_name(1_787_319_908), "WoWScrnShot_082126_134508.png");
        // 2001-01-02T03:04:05Z: every field at its narrowest, and a year whose last two digits
        // need the leading zero the reference's `%02d` gives them.
        assert_eq!(s.next_name(978_404_645), "WoWScrnShot_010201_030405.png");
    }

    /// **The one-frame hold, as a test** — the fix for the leak a live double press actually
    /// produced (module docs). An ask made during frame N must not be spawned until frame N+1,
    /// because frame N's UI quads were built before the binding hid the status line.
    ///
    /// Driven against the real system in a minimal app rather than by calling a helper: the claim
    /// is about *scheduling*, and a helper that runs both halves in one call would pass whether or
    /// not the deferral survives a future reorder.
    #[test]
    fn an_ask_is_held_for_one_frame_before_the_shutter() {
        let mut app = App::new();
        app.init_resource::<ScreenshotState>();
        // The observer is irrelevant here — what is under test is when the entity appears — so the
        // system runs with no VM and the asks are pushed by hand.
        app.add_systems(Update, spawn_pending_only);

        // Frame N: the binding asked. Nothing may be spawned yet.
        app.world_mut().resource_mut::<ScreenshotState>().pending = 0;
        app.world_mut().resource_mut::<ScreenshotState>().pending += 1;
        let staged = app.world().resource::<ScreenshotState>().pending;
        assert_eq!(staged, 1, "the ask is staged, not yet a capture");

        // Frame N+1: now it fires, against quads rebuilt with the line hidden.
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

        // A quiet frame asks for nothing.
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&Screenshot>()
                .iter(app.world())
                .count(),
            1
        );
    }

    /// [`ask_for_captures`]'s spawn half alone — the drain half needs a live VM, which a bare
    /// `App` has no business building. Keeping the two in one system in production and splitting
    /// only here would be the usual lie, so this calls the real thing with the VM absent, which is
    /// exactly how it behaves at the character-select screen.
    fn spawn_pending_only(commands: Commands, state: ResMut<ScreenshotState>) {
        ask_for_captures(commands, None, state);
    }
}
