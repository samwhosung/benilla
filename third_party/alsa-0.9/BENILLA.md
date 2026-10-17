# A lock-only stand-in for `alsa` 0.9.1

Written by benilla, MIT OR Apache-2.0; it contains no upstream code. Wired in through
`[patch.crates-io]` in the workspace root.

Why: `alsa-sys` declares `links = "alsa"`, and a lockfile may hold one version of it. `cpal` 0.17.3,
our audio device layer off macOS, takes `alsa` 0.11 and so `alsa-sys` 0.4. `bevy_internal` lists
`bevy_audio` as a weak optional dependency (`bevy_audio?/android_shared_stdcxx`), which puts
`bevy_audio`, its `cpal` 0.15.3 and so `alsa` 0.9.1 and `alsa-sys` 0.3 into the lock although the
workspace never enables bevy's audio (`Cargo.toml`, `bevy`). The two `links` collide and the
resolver refuses. This package satisfies `cpal` 0.15.3's `alsa = "0.9"` with no dependency on
`alsa-sys`.

It is never compiled: `cargo tree --target all` holds no `cpal` 0.15. If bevy's audio were enabled,
the build would stop at the `compile_error!` in `src/lib.rs`, which says why.

Drop it when the lock holds no `bevy_audio` (a bevy that no longer lists it) or when bevy's
`cpal` takes `alsa` 0.11.
