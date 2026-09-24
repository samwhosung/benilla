//! The **tuned Bevy boot** — the `DefaultPlugins` set every benilla binary stands on.
//!
//! Extracted from `benilla_app::run` the moment there was a second binary to boot ([`crate::worldview`],
//! decision 1160): the window differs per binary, but the *engine* tuning — the asset root, the log
//! filter, the task-pool sizing and QoS, and the disabled audio plugin — must not. Each of those
//! four is load-bearing and each was learned the hard way; a second boot that quietly re-derived
//! them would be a second place for them to rot.
//!
//! The `Window` stays the caller's, because that is genuinely per-binary: the client's is shaped by
//! the capture harness and the background-run rules, the world viewer's
//! is a plain window.

use bevy::app::{PluginGroupBuilder, TaskPoolOptions, TaskPoolPlugin};
use bevy::prelude::*;

use crate::thread_qos;

/// `DefaultPlugins` with benilla's engine tuning applied, around the caller's primary window.
pub fn tuned_default_plugins(primary_window: Window) -> PluginGroupBuilder {
    DefaultPlugins
        .set(WindowPlugin {
            primary_window: Some(primary_window),
            ..default()
        })
        // NOTE: there is deliberately no `AssetPlugin::file_path` here. This used
        // to bake `concat!(env!("CARGO_MANIFEST_DIR"), "/assets")` — the *build* machine's source
        // tree — because a shim package builds the binary and Bevy's runtime `CARGO_MANIFEST_DIR`
        // fallback would otherwise resolve `assets/` in the shim's dir. It worked on the
        // machine that compiled it and nowhere else: on a player's machine every shader resolved
        // to nothing and the world drew bare, the "silently-no-shaders trap" `capture/mod.rs`'s
        // header names. Every WGSL file in the tree is now compiled into the binary
        // (`crate::shaders`, `benilla_app::shaders`, `benilla_assets::materials`) and addressed
        // `embedded://<crate>/shaders/…`, so nothing reaches for a file root at all and 1171's
        // engine/game line survives as the crate each shader is embedded from.
        // Quiet wgpu/naga; our own crates stay at info. The ring keeps the last lines of what
        // stderr shows for the crash report (`log_ring`).
        .set(bevy::log::LogPlugin {
            filter: "wgpu=error,naga=warn".into(),
            custom_layer: |_| Some(Box::new(crate::log_ring::LogRing)),
            ..default()
        })
        // Asset streaming is this client's load bottleneck: every M2/WMO/BLP read decompresses from
        // MPQ and parses **synchronously** on Bevy's IO task pool, and the AssetServer runs *all*
        // loads there. Bevy's default caps that pool at 4 threads, so a teleport into a dense area —
        // terrain + WMOs + their doodad props, all bursting at once — saturates it and the
        // net-driven NPC/GameObject models queue behind the flood. Give IO more of the box (it sits
        // idle when not streaming); the world-render path is GPU/IO-bound, not compute-bound, so
        // trading some compute threads for streaming throughput is the right call.
        // Thread QoS: Bevy's workers spawn at default QoS — the same Darwin
        // scheduling class as rustc or an OBS encoder — so under a background build the frame's own
        // threads queue behind the compiler for P-core time. Promote them at spawn: compute runs
        // this frame's systems (user-interactive); IO/async-compute feed upcoming frames
        // (user-initiated — above default, below the frame itself). The render thread has no spawn
        // hook; `ThreadQosPlugin` promotes it from inside.
        .set(TaskPoolPlugin {
            task_pool_options: TaskPoolOptions {
                io: bevy::app::TaskPoolThreadAssignmentPolicy {
                    min_threads: 2,
                    max_threads: 8,
                    percent: 0.5,
                    on_thread_spawn: Some(std::sync::Arc::new(|| {
                        thread_qos::promote_current_thread(thread_qos::QosClass::UserInitiated)
                    })),
                    on_thread_destroy: None,
                },
                async_compute: bevy::app::TaskPoolThreadAssignmentPolicy {
                    on_thread_spawn: Some(std::sync::Arc::new(|| {
                        thread_qos::promote_current_thread(thread_qos::QosClass::UserInitiated)
                    })),
                    ..TaskPoolOptions::default().async_compute
                },
                compute: bevy::app::TaskPoolThreadAssignmentPolicy {
                    on_thread_spawn: Some(std::sync::Arc::new(|| {
                        thread_qos::promote_current_thread(thread_qos::QosClass::UserInteractive)
                    })),
                    // `WOW_THREADS=1` serialises the systems that run this frame. Not a performance
                    // dial — a **diagnostic**: a defect that alternates frame to frame with no
                    // camera, geometry or draw-order change behind it is what an unordered write
                    // between two systems looks like, and that is separable from every other cause
                    // only by taking the concurrency away. Anything that survives `WOW_THREADS=1`
                    // is not a race.
                    max_threads: match std::env::var("WOW_THREADS").ok().as_deref() {
                        Some("1") => 1,
                        _ => TaskPoolOptions::default().compute.max_threads,
                    },
                    ..TaskPoolOptions::default().compute
                },
                ..default()
            },
        })
        // Sound is kira behind our own mixer seam. Bevy's `AudioPlugin` used to be
        // disabled here so it would not open a second, never-used OS output stream — but
        // the crate behind it was still compiled and linked. Since 1932 `bevy_audio` is off at the
        // feature level, so there is no plugin to disable and no rodio/cpal/vorbis stack in the
        // binary; the feature list that keeps it out is in the workspace `Cargo.toml`.
        // The dead registrations: DefaultPlugins members whose only runtime
        // trace here was per-frame machinery for types nothing instantiates — every registered
        // asset/material type costs an `Assets<T>` event system in PostUpdate plus
        // extract/prepare/sweep families in the render app, priced by the 1437 census against
        // the 1435 band map. Each cut was usage-grepped NEGATIVE and then proven by a clean
        // boot, and the boot vetoed two of the five candidates the greps had passed (its
        // missing-resource panic names the dependent): gizmos are the bowstring/fishing-line
        // renderer (warmed through a bare-`Gizmos` param no `Gizmos<` pattern sees), and
        // the sprite pair carries our OWN FrameXML quad pass — `UiQuadMaterial` is a
        // `Material2d` riding `Mesh2dPipeline`. What else stays, stays for a reason:
        // bevy_picking drives the world-interact lane, TextPlugin draws the glue-screen text,
        // PostProcessPlugin is the EffectGlow bloom, and two members are pinned by upstream
        // exactly like 1437's Render-MT — ScenePlugin (avian's collider backend reads
        // `SceneSpawner` even in a meshless world; our own harness tests document it) and
        // PbrPlugin's ForwardDecal family (registered inside PbrPlugin::build).
        //
        // We load M2/WMO/ADT through our own mpq:// loaders — no glTF anywhere.
        .disable::<bevy::gltf::GltfPlugin>()
        // No bevy AA: no Fxaa/TAA/SMAA/CAS component anywhere (MSAA is core render, unaffected).
        .disable::<bevy::anti_alias::AntiAliasPlugin>()
        // No gamepad input; 1.12's bindings are keyboard/mouse.
        .disable::<bevy::gilrs::GilrsPlugin>()
}
