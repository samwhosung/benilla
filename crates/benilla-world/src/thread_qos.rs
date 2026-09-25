//! macOS thread QoS: keep frame-critical threads on the P-cores under system load. Apple Silicon
//! schedules by QoS class, and the main thread is user-interactive, but every worker Bevy spawns
//! starts at default, the class of `rustc`, so a background build starves the frame's workers.
//! `pthread_set_qos_class_self_np` is not in the `libc` crate, so the extern lives here; everything
//! is a no-op off macOS.

use bevy::prelude::*;
use bevy::render::{Render, RenderApp, RenderSystems};

/// The QoS classes used here, as Darwin's `qos_class_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum QosClass {
    /// Work on the critical path of the current frame (compute pool, render thread).
    UserInteractive = 0x21,
    /// Work the frame is waiting on soon but not this frame (asset IO, async compute, net IO).
    UserInitiated = 0x19,
    /// Darwin's default class, for a thread that blocks for seconds: Metal compiles at the calling
    /// thread's QoS, and Apple prescribes this class for pipeline prewarming (WWDC25, "Explore
    /// Metal 4 games").
    Default = 0x15,
}

/// Set by `pipe_warm` while the covered pipeline-warm burst blocks the render thread in Metal
/// compilation; [`promote_render_thread`] then drops that thread to `Default` from inside, since
/// only a thread can set its own QoS.
pub static COMPILE_BURST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Promote the calling thread to `class`. Safe to call repeatedly; logs once on failure.
pub fn promote_current_thread(class: QosClass) {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        let rc = unsafe { pthread_set_qos_class_self_np(class as u32, 0) };
        if rc != 0 {
            warn_once!("thread QoS promotion to {class:?} failed (rc={rc})");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = class;
}

/// Promotes the pipelined-rendering thread, which Bevy spawns with no hook. The exclusive system
/// runs on whichever thread drives the render schedule, so it re-checks each frame behind a
/// thread-local latch; it sits in `ExtractCommands`, where its barrier has nothing to drain.
pub struct ThreadQosPlugin;

impl Plugin for ThreadQosPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app.add_systems(
            Render,
            promote_render_thread.in_set(RenderSystems::ExtractCommands),
        );
    }
}

/// The calling thread's QoS class, for the tests.
#[cfg(all(test, target_os = "macos"))]
fn current_thread_qos() -> Option<u32> {
    unsafe extern "C" {
        fn pthread_self() -> *mut core::ffi::c_void;
        fn pthread_get_qos_class_np(
            thread: *mut core::ffi::c_void,
            qos_class: *mut u32,
            relative_priority: *mut i32,
        ) -> i32;
    }
    let mut qos: u32 = 0;
    let rc = unsafe { pthread_get_qos_class_np(pthread_self(), &mut qos, core::ptr::null_mut()) };
    (rc == 0).then_some(qos)
}

fn promote_render_thread(_world: &mut World) {
    // Frame-critical, or default while it is the shader compiler's caller under a cover.
    let want = if COMPILE_BURST.load(std::sync::atomic::Ordering::Relaxed) {
        QosClass::Default
    } else {
        QosClass::UserInteractive
    };
    std::thread_local! {
        static APPLIED: std::cell::Cell<Option<QosClass>> = const { std::cell::Cell::new(None) };
    }
    APPLIED.with(|p| {
        if p.get() != Some(want) {
            promote_current_thread(want);
            debug!(
                "thread QoS: render-schedule thread {:?} → {want:?}",
                std::thread::current().name().unwrap_or("<unnamed>")
            );
            p.set(Some(want));
        }
    });
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn promotion_applies_to_spawned_thread() {
        for class in [QosClass::UserInteractive, QosClass::UserInitiated] {
            let observed = std::thread::spawn(move || {
                let before = current_thread_qos();
                promote_current_thread(class);
                (before, current_thread_qos())
            })
            .join()
            .unwrap();
            // A bare std thread spawns at default (0x15).
            assert_eq!(observed.0, Some(0x15), "spawned thread not default-QoS");
            assert_eq!(observed.1, Some(class as u32), "promotion did not apply");
        }
    }

    /// kira's decode threads, spawned from the promoted main thread, land at default too, which is
    /// why `sound::mixer::PromotingSource` promotes from inside.
    #[test]
    fn qos_is_not_inherited_by_spawned_threads() {
        let (parent, child) = std::thread::spawn(|| {
            promote_current_thread(QosClass::UserInteractive);
            let child = std::thread::spawn(current_thread_qos).join().unwrap();
            (current_thread_qos(), child)
        })
        .join()
        .unwrap();
        assert_eq!(parent, Some(QosClass::UserInteractive as u32));
        assert_eq!(child, Some(0x15), "QoS inherited — PromotingSource is moot");
    }
}
