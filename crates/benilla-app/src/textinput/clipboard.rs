//! The host OS pasteboard seam — the one place benilla talks to the platform clipboard.
//!
//! The UI engine is engine-free and can't reach the OS clipboard, so the three clipboard chords
//! ([`keymap::Chord`]) resolve host-side: copy/cut write out what `editbox_copy`/`editbox_cut`
//! hand back, paste reads the pasteboard and feeds `UiScript::paste`.
//!
//! **Why this is a held resource with a per-platform backend, and not `arboard::Clipboard::new()`
//! per call.** Both halves of copy/paste were broken on Linux, for two independent reasons:
//!
//! 1. **On X11 the handle *is* the clipboard.** An X11 selection has no backing store: the owning
//!    client serves the bytes on request, so the clipboard lives exactly as long as the connection
//!    that owns it. arboard keeps one process-global connection plus a server thread and tears the
//!    whole thing down in `Drop` as soon as the last handle goes (its `MIN_OWNERS == 3` check —
//!    a lone handle *is* the last one), first offering the data to a clipboard manager. So a fresh
//!    handle per copy relinquishes the selection the instant the copy returns: with no clipboard
//!    manager running — a bare WM, the common gaming setup — text copied out of benilla vanishes
//!    immediately. One handle, held for the process lifetime, is the fix. macOS and Windows never
//!    showed this because `NSPasteboard` and the Win32 clipboard both take ownership of the *data*,
//!    not of a live connection, which is exactly why the per-call pattern read as fine.
//! 2. **Our window is Wayland; arboard here is X11-only.** benilla builds with bevy's `wayland`
//!    feature on, so on a Wayland session winit hands us a native Wayland window with no X11
//!    surface at all — while `arboard`, without its **non-default** `wayland-data-control` feature,
//!    only ever speaks X11. Reading then finds no X11 CLIPBOARD owner and fails with
//!    `ContentNotAvailable`: precisely the reported "can't paste into the client on Linux".
//!    Turning arboard's Wayland feature on is *not* the fix — it speaks `wlr-data-control`, a
//!    privileged clipboard-manager protocol GNOME/Mutter deliberately does not implement, and
//!    wl-clipboard-rs' own README points windowed apps at `smithay-clipboard` instead. So Wayland
//!    goes through `smithay-clipboard`, which drives the **standard** `wl_data_device` from our own
//!    winit `wl_display` and seat — the protocol every compositor supports.
//!
//! Windows needs none of this: `clipboard-win` resolves with no feature gates and the Win32
//! clipboard is store-backed, so the chord table ([`keymap`]) was already the whole story
//! there.

use std::ffi::c_void;

use bevy::prelude::*;
use bevy::window::RawHandleWrapper;

/// One platform pasteboard, behind just the two operations the chord table needs. Boxed as a trait
/// object so the entire Wayland half — crate, types, and `unsafe` — stays inside one `cfg` block
/// instead of sprouting a `cfg` on every enum variant and match arm.
trait Pasteboard {
    /// The pasteboard's text. `Ok(None)` is the ordinary "nothing text-shaped on it" case (an empty
    /// clipboard is not an error); `Err` is a genuine failure worth a `warn!`.
    fn read_text(&mut self) -> Result<Option<String>, String>;

    /// Put `text` on the pasteboard.
    fn write_text(&mut self, text: &str) -> Result<(), String>;

    /// What to call this backend in a log line.
    fn name(&self) -> &'static str;
}

impl Pasteboard for arboard::Clipboard {
    fn read_text(&mut self) -> Result<Option<String>, String> {
        // `ContentNotAvailable` is arboard's "empty, or holds no format we asked for" — the normal
        // nothing-to-paste case, not a failure to report.
        match self.get_text() {
            Ok(text) if text.is_empty() => Ok(None),
            Ok(text) => Ok(Some(text)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn write_text(&mut self, text: &str) -> Result<(), String> {
        self.set_text(text.to_string()).map_err(|e| e.to_string())
    }

    fn name(&self) -> &'static str {
        // The backend arboard itself picked, which is the part worth naming in a bug report.
        if cfg!(target_os = "macos") {
            "arboard/NSPasteboard"
        } else if cfg!(windows) {
            "arboard/win32"
        } else {
            "arboard/x11"
        }
    }
}

/// The Wayland half: the standard `wl_data_device` through our own `wl_display`. Gated to the
/// platforms `smithay-clipboard` is a dependency on (see `Cargo.toml`) — Linux and the BSDs.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
mod wl {
    use super::Pasteboard;

    impl Pasteboard for smithay_clipboard::Clipboard {
        fn read_text(&mut self) -> Result<Option<String>, String> {
            match self.load() {
                Ok(text) if text.is_empty() => Ok(None),
                Ok(text) => Ok(Some(text)),
                // Nothing offered for this seat — the Wayland spelling of an empty clipboard.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.to_string()),
            }
        }

        fn write_text(&mut self, text: &str) -> Result<(), String> {
            // `store` is infallible: it hands the text to the clipboard thread, which then serves
            // it to whoever asks for as long as we hold the seat.
            self.store(text.to_string());
            Ok(())
        }

        fn name(&self) -> &'static str {
            "smithay/wl_data_device"
        }
    }
}

/// The `wl_display` behind the primary window, or `None` when this isn't a Wayland session (X11,
/// macOS, Windows) — which is exactly the signal to fall back to [`arboard`].
///
/// Taken from bevy's own [`RawHandleWrapper`] rather than `WAYLAND_DISPLAY`: `smithay-clipboard`
/// needs *our* connection (it works off the seat holding our keyboard focus), and the env var only
/// says a Wayland session exists, not that winit chose it.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
pub(crate) fn wayland_display(handle: Option<&RawHandleWrapper>) -> Option<*mut c_void> {
    match handle?.get_display_handle() {
        raw_window_handle::RawDisplayHandle::Wayland(wl) => Some(wl.display.as_ptr()),
        _ => None,
    }
}

/// Off the Wayland-capable platforms there is no `wl_display` to find, so the backend is always
/// [`arboard`].
#[cfg(not(all(unix, not(any(target_os = "macos", target_os = "android")))))]
pub(crate) fn wayland_display(_handle: Option<&RawHandleWrapper>) -> Option<*mut c_void> {
    None
}

/// Build the Wayland backend for `display`.
///
/// # Safety
/// `display` must be a live `wl_display` that outlives the returned backend. It comes from bevy's
/// [`RawHandleWrapper`], which holds an `Arc` on the window keeping the connection alive, and the
/// backend lives in a resource dropped during app teardown.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
fn open_wayland(display: *mut c_void) -> Box<dyn Pasteboard> {
    // SAFETY: see the doc comment — `display` is winit's own live `wl_display`.
    Box::new(unsafe { smithay_clipboard::Clipboard::new(display) })
}

#[cfg(not(all(unix, not(any(target_os = "macos", target_os = "android")))))]
fn open_wayland(_display: *mut c_void) -> Box<dyn Pasteboard> {
    unreachable!("wayland_display() only yields Some on Wayland-capable platforms")
}

/// The session's display-server environment, appended to the clipboard log lines. A clipboard bug
/// report is nearly useless without it — "paste does nothing on Linux" has at least three different
/// causes depending on these three variables, and asking the reporter costs a round trip.
fn session_note() -> String {
    if !cfg!(unix) || cfg!(target_os = "macos") {
        return String::new();
    }
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| "unset".to_string());
    format!(
        " [XDG_SESSION_TYPE={}, WAYLAND_DISPLAY={}, DISPLAY={}]",
        var("XDG_SESSION_TYPE"),
        var("WAYLAND_DISPLAY"),
        var("DISPLAY"),
    )
}

/// The process-wide handle on the OS pasteboard.
///
/// A `NonSend` resource, for three reasons that all point the same way: neither backend is `Sync`,
/// macOS's `NSPasteboard` must be touched from the main thread, and — the load-bearing one — the
/// handle has to be *held* for the whole run or X11 copy silently does nothing (module docs).
#[derive(Default)]
pub(crate) struct HostClipboard {
    backend: Option<Box<dyn Pasteboard>>,
    /// Opening is attempted exactly once. A failure is sticky: a box with no usable clipboard
    /// shouldn't re-attempt (and re-warn) on every keystroke.
    opened: bool,
}

impl HostClipboard {
    /// Resolve the backend on first use. Deferred rather than done at startup because the Wayland
    /// pointer comes from the window, and lazily because a run that never copies anything should
    /// never open a pasteboard connection at all.
    fn ensure(&mut self, wl_display: Option<*mut c_void>) {
        if self.opened {
            return;
        }
        self.opened = true;
        self.backend = match wl_display {
            Some(display) => Some(open_wayland(display)),
            None => match arboard::Clipboard::new() {
                Ok(clipboard) => Some(Box::new(clipboard)),
                Err(e) => {
                    warn!("clipboard: unavailable — {e}{}", session_note());
                    None
                }
            },
        };
        if let Some(backend) = &self.backend {
            info!("clipboard: {}{}", backend.name(), session_note());
        }
    }

    /// The pasteboard's text, or `None` if it's empty, non-text, or unavailable.
    pub(crate) fn read(&mut self, wl_display: Option<*mut c_void>) -> Option<String> {
        self.ensure(wl_display);
        let backend = self.backend.as_mut()?;
        match backend.read_text() {
            Ok(text) => text,
            Err(e) => {
                warn!(
                    "clipboard: read failed via {} — {e}{}",
                    backend.name(),
                    session_note()
                );
                None
            }
        }
    }

    /// Put `text` on the pasteboard (the copy/cut half).
    pub(crate) fn write(&mut self, wl_display: Option<*mut c_void>, text: &str) {
        self.ensure(wl_display);
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        if let Err(e) = backend.write_text(text) {
            warn!(
                "clipboard: write failed via {} — {e}{}",
                backend.name(),
                session_note()
            );
        }
    }
}
