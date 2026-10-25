# benilla's `wgpu-hal` fork — what differs from upstream, and how to check

Upstream: [`gfx-rs/wgpu`](https://github.com/gfx-rs/wgpu) (`wgpu-hal`), MIT OR Apache-2.0, version
`27.0.4`, the version the workspace lock resolves to. Wired in through `[patch.crates-io]` in the
workspace root. The crate is copied from the cargo registry as published (`src/`, `build.rs`,
`Cargo.toml`, `README.md`, the two licence files), except that the three `[[example]]` targets
are dropped from `Cargo.toml` and `examples/` is not copied. Every hunk is marked `BENILLA:`.

## Why a fork exists at all

On Vulkan, the default on Windows and Linux, `Surface::acquire_texture` waits up to wgpu-core's 1 s
frame timeout, and a presentation engine that stalls longer (a machine loaded by other work)
used to end the process:

- The post-acquire `vkWaitForFences` on the acquired image's fence returned `VK_TIMEOUT`, which
  went through `map_host_device_oom_and_lost_err` and became `DeviceError::Unexpected`. wgpu-core
  treats that as fatal and loses the device, and Bevy's `prepare_windows` panics with "Couldn't
  get swap chain texture … generic error".
- A `vkAcquireNextImageKHR` `VK_TIMEOUT` returned `Ok(None)`, which Bevy also panics on off Linux.

Upstream tracks it as [gfx-rs/wgpu#9029](https://github.com/gfx-rs/wgpu/issues/9029), still open
in wgpu 30. 1.12.1 has no such timeout: its frame pass presents when the swap chain is ready.
Found in `pkuzic/benilla-everwood_graphics@6fe464d896`.

## The patch — `src/vulkan/instance.rs`, `Surface::acquire_texture`

A `VK_TIMEOUT` is retried while the call has stalled for less than `STALL_BUDGET` (30 s, counted
from the start of `acquire_texture`) and the caller's timeout is not zero (a zero timeout is a
poll, which answers at once by design). Past the budget the result is what upstream returns.

- **`vkAcquireNextImageKHR` times out:** the acquire is issued again with the same semaphore and
  fence. The spec: when a call "does not successfully acquire an image, `semaphore` and `fence`
  are unaffected", so no signal operation is pending and the "unsignaled, no operations
  pending" valid usage still holds. `VK_TIMEOUT` itself is returned only for a timeout above zero
  and below `UINT64_MAX`.
- **`vkWaitForFences` on the acquired image times out:** the wait is issued again. The image is
  already acquired and its fence has a signal operation submitted, so waiting is the only
  correct move; the spec lets a wait be repeated and returns `VK_TIMEOUT` only when the fence is
  not yet signalled.
- **The wait on the previous submission, before the acquire, times out:** upstream drops
  `wait_for_fence`'s `Ok(false)` and acquires with a semaphore the GPU may still hold, which the
  spec forbids ("unsignaled, with no signal or wait operations pending"). The wait is issued
  again under the same budget; past it the acquire goes on as upstream's does.
- **Logging:** each retry logs at `error!` with the time stalled, so a stall shows in the log. A
  result that is not retried is logged raw (`{:?}` of the `vk::Result`) before it is mapped to
  `DeviceError::Unexpected` or `Ok(None)`; the ordinary resize results (`NOT_READY`,
  `ERROR_OUT_OF_DATE_KHR`, `ERROR_SURFACE_LOST_KHR`) are not logged.

What changes in play: a stall under 30 s only delays the frame. A caller that passes a timeout
now waits up to the budget instead of one timeout, so `Ok(None)` ("not ready, skip the frame")
comes only after 30 s.

## How to check

`diff -u <registry>/wgpu-hal-27.0.4/src/vulkan/instance.rs third_party/wgpu-hal/src/vulkan/instance.rs`
shows the patch and nothing else; `diff -r` of the two trees shows only that file, the dropped
`[[example]]` blocks in `Cargo.toml`, and the files not copied (`examples/`, `Cargo.lock`,
`Cargo.toml.orig`, `.cargo_vcs_info.json`, `.cargo-ok`).
`video::tests::wgpu_hal_resolves_to_the_vendored_stall_patch` checks the lock resolves wgpu-hal to
this path, at the lock's version, with the hunks present.

## When to drop it

When wgpu-hal handles the timeout itself ([gfx-rs/wgpu#9029](https://github.com/gfx-rs/wgpu/issues/9029)),
or when wgpu leaves 27: delete this directory and the `wgpu-hal` line under `[patch.crates-io]`.
