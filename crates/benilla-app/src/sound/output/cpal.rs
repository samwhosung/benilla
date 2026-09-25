//! The non-macOS half of [`super`] — the same surface as `coreaudio.rs`, over cpal
//! (Linux and Windows had been silent since the 09-02 sync, bug B356).
//!
//! [`super`] owns everything that decides *how* the mix reaches the speaker: the mix-ahead
//! ring, the render thread, the meters, the rebuild-on-device-change loop. That is platform
//! code nowhere. What is platform code is only the last hop — find the default output device,
//! open a stream on it, and hand our IO callback one buffer per cycle — and off macOS that hop
//! is ALSA or PipeWire on Linux, WASAPI on Windows, and something else again on the BSDs. cpal
//! is the layer that already speaks all of them, so this file is cpal wearing the shape
//! `coreaudio.rs` wears: [`Device`], [`Stream`], [`Cycle`], [`Notices`], [`Listeners`],
//! [`set_realtime`], [`now_ns`].
//!
//! ## The three places cpal does not hand us what CoreAudio does
//!
//! - **Cycle timestamps.** CoreAudio stamps every IO cycle with the host time its first frame
//!   reaches the DAC; the *lead* meter is that minus now. cpal gives the same quantity split in
//!   two (`OutputStreamTimestamp`'s `callback` and `playback`), so the lead is their difference
//!   and [`Cycle::output_time_ns`] is `now + (playback − callback)` — the same number the meter
//!   is about, reconstructed on our own clock.
//! - **Device notices.** CoreAudio raises a property listener when the default output changes,
//!   when a device dies, when its rate changes. cpal has no notification API at all, so
//!   [`Listeners`] is a half-second poll of the default device's identity and rate (the same
//!   mechanism kira's own cpal backend uses off macOS) plus cpal's stream error callback, which
//!   *is* delivered on these hosts — unlike on macOS, where cpal 0.17 wires a no-op for the
//!   default device. `kAudioDeviceProcessorOverload` has no analogue: the overload meter reads
//!   zero here, and the audible failure is counted where it always was, as a ring underrun.
//! - **Realtime scheduling.** No workgroups; [`Workgroup`] is inert. [`set_realtime`] is
//!   `SCHED_FIFO` on unix and MMCSS "Pro Audio" on Windows — see its own docs for what each
//!   costs when the OS refuses.
//!
//! Everything else — including the fact that an unsupported target simply reports no device and
//! the client runs silent — falls out of cpal, whose Null host is what it compiles to there.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};

/// The channel count we render. kira mixes stereo; [`spread`] puts it over whatever the device
/// actually takes.
pub(super) const CHANNELS: u32 = 2;

/// How often [`Listeners`] re-reads the default output device. Half a second is kira's own
/// cadence for this poll off macOS, and it is the granularity at which "I plugged headphones
/// in" should become audible.
const WATCH_EVERY: Duration = Duration::from_millis(500);

/// The most frames one [`Cycle`] carries. A host that hands us a bigger buffer than this gets
/// it served in several cycles rather than an allocation on the audio thread ([`Stream::open`]
/// sizes the scratch once, at open). Generous on purpose: 8192 frames is 170 ms at 48 kHz, well
/// past any buffer a host has been seen to ask for, and 64 KB of scratch is nothing.
const MAX_CYCLE_FRAMES: usize = 8192;

/// Nanoseconds on a monotonic clock. Not the device's clock — cpal's `StreamInstant` is
/// host-defined and not comparable across hosts, so the meters ride [`Instant`] and the one
/// device-timeline quantity we need (the lead) is reconstructed from a *difference* of stream
/// instants, which is well-defined everywhere.
pub(super) fn now_ns() -> u64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

// ---------------------------------------------------------------------------------------------
// Device

/// One output device as cpal reports it at open time.
#[derive(Clone)]
pub(super) struct Device {
    pub name: String,
    /// The rate the device's default config runs at — what we render at (nothing below us
    /// resamples).
    pub sample_rate: u32,
    /// The buffer sizes the host says it accepts, frames.
    pub buffer_range: (u32, u32),
    /// cpal exposes no device latency or safety offset; the report prints 0 rather than a guess.
    pub latency_frames: u32,
    pub safety_frames: u32,
    /// The cpal handle and the config we will open with. Private: [`super`] only ever reads the
    /// descriptive fields above.
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    /// The host's stable id for this device, when it has one. This — not [`Self::name`] — is
    /// what [`Listeners`] compares: a display name is neither unique nor fixed (two identical
    /// headsets, a renamed endpoint), and cpal 0.17 deprecates `name()` for exactly that.
    id: Option<cpal::DeviceId>,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("name", &self.name)
            .field("sample_rate", &self.sample_rate)
            .field("buffer_range", &self.buffer_range)
            .field("channels", &self.config.channels())
            .field("sample_format", &self.config.sample_format())
            .finish()
    }
}

/// The host's current default output device, fully described. `Err` when there is none (a
/// headless machine, no sound server running, an unsupported target) — the caller runs silent.
pub(super) fn default_output() -> Result<Device> {
    // Prime the monotonic epoch here, on the main thread: the first reader is otherwise the
    // audio callback, and a clock that starts on the realtime path is one more thing to reason
    // about for no gain.
    now_ns();
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no default output device")?;
    describe(device)
}

fn describe(device: cpal::Device) -> Result<Device> {
    let name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "unnamed output device".to_string());
    // The default config is the one config the host guarantees it can open, and cpal's own
    // heuristic already prefers f32 and stereo within it. Negotiating our own would only add a
    // way to be refused.
    let config = device
        .default_output_config()
        .with_context(|| format!("default output config for {name}"))?;
    let sample_rate = config.sample_rate();
    if !(8000..=384_000).contains(&sample_rate) {
        bail!("device {name} reports an absurd sample rate {sample_rate}");
    }
    if config.channels() == 0 {
        bail!("device {name} reports zero output channels");
    }
    let buffer_range = match *config.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => (min.max(1), max.max(1)),
        // "Unknown" means the host will not take a size from us at all; `Stream::open` reads
        // that back off the config and asks for the host's default instead.
        cpal::SupportedBufferSize::Unknown => (1, u32::MAX),
    };
    Ok(Device {
        name,
        sample_rate,
        buffer_range,
        latency_frames: 0,
        safety_frames: 0,
        id: device.id().ok(),
        device,
        config,
    })
}

// ---------------------------------------------------------------------------------------------
// Stream

/// What the IO callback sees each cycle: the interleaved **stereo** buffer to fill and the time
/// at which its first frame reaches the DAC.
pub(super) struct Cycle<'a> {
    pub buffer: &'a mut [f32],
    pub frames: usize,
    /// When this cycle's first frame is due, in nanoseconds on the [`now_ns`] clock —
    /// reconstructed as `now + (playback − callback)` from cpal's pair of stream instants.
    /// Zero if the host gave no usable timestamp.
    pub output_time_ns: u64,
}

/// An open, running output stream on one device. Dropping it stops the stream and drops the
/// callback (and with it whatever the closure owned) — cpal joins its own audio thread first.
pub(super) struct Stream {
    _stream: cpal::Stream,
    observed_frames: Arc<AtomicU32>,
}

impl Stream {
    /// The cycle size the device is actually running, frames.
    ///
    /// cpal cannot tell us what the host granted, and on WASAPI's shared mode it varies from
    /// wake to wake, so the honest number is the one the last callback carried rather than the
    /// one we asked for. Seeded with the request so the first report is never zero.
    pub(super) fn buffer_frames(&self) -> u32 {
        self.observed_frames.load(Ordering::Relaxed)
    }

    /// Open `device` at its default config with a buffer of `buffer_frames` (clamped to what the
    /// host says it accepts) and start it. `on_cycle` runs on cpal's audio thread — it must
    /// never block, allocate, or log.
    ///
    /// `notices` is wired to cpal's stream error callback: on these hosts it is delivered (the
    /// no-op 1857 found was macOS-and-default-device only), and it is the promptest signal that
    /// the device is gone. The poll in [`Listeners`] is the backstop.
    pub(super) fn open<F>(
        device: &Device,
        buffer_frames: u32,
        notices: &Arc<Notices>,
        on_cycle: F,
    ) -> Result<Self>
    where
        F: FnMut(Cycle<'_>) + Send + 'static,
    {
        let buffer_frames = buffer_frames.clamp(device.buffer_range.0, device.buffer_range.1);
        let mut config = device.config.config();
        config.buffer_size = match device.config.buffer_size() {
            cpal::SupportedBufferSize::Range { .. } => cpal::BufferSize::Fixed(buffer_frames),
            cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
        };
        let observed_frames = Arc::new(AtomicU32::new(buffer_frames));

        let errors = Arc::clone(notices);
        // Only two of cpal's four `StreamError`s are fatal to a stream, and getting that wrong is
        // expensive in exactly one direction. `BufferUnderrun` is a routine ALSA xrun — the
        // device starved below us — and `BackendSpecific` is a grab-bag; treating either as a
        // dead device means a full teardown and rebuild of the stream on every glitch, which is
        // the loudest possible response to the quietest problem. **Measured, not
        // reasoned:** with both mapped to `device_died`, one second of the Linux live test
        // carried a `Lost` and a re-`Opened`; with this split, three runs carry neither. kira
        // draws the same line (`stream_manager.rs`: `DeviceNotAvailable | StreamInvalidated`
        // restart, the other two are dropped).
        let reported = std::sync::atomic::AtomicBool::new(false);
        let on_error = move |e: cpal::StreamError| {
            match e {
                cpal::StreamError::DeviceNotAvailable | cpal::StreamError::StreamInvalidated => {
                    errors.device_died.store(true, Ordering::Release);
                }
                // Not fatal, so the stream stays. Said once per stream and no more: this runs on
                // the audio thread, and an xrun storm must not become a logging storm. The
                // audible cost of a starved device is already counted upstairs as an underrun.
                _ => {
                    if !reported.swap(true, Ordering::Relaxed) {
                        bevy::log::warn!("audio: output stream reported {e} (stream kept)");
                    }
                    return;
                }
            }
            bevy::log::warn!("audio: output stream lost ({e})");
        };

        let ctx = Callback {
            on_cycle,
            channels: usize::from(device.config.channels()),
            sample_rate: device.sample_rate,
            observed: Arc::clone(&observed_frames),
            scratch: vec![0.0; MAX_CYCLE_FRAMES * CHANNELS as usize],
        };
        // The format list cpal's own `beep` example carries — every output format it can hand a
        // typed callback for. The default config names one of them, so in practice this picks
        // f32 on WASAPI and whatever ALSA's heuristic settled on.
        let stream = match device.config.sample_format() {
            cpal::SampleFormat::F32 => build::<f32, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::F64 => build::<f64, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::I8 => build::<i8, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::I16 => build::<i16, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::I24 => build::<cpal::I24, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::I32 => build::<i32, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::I64 => build::<i64, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::U8 => build::<u8, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::U16 => build::<u16, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::U32 => build::<u32, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::U64 => build::<u64, _>(device, &config, ctx, on_error),
            other => bail!("device {} wants sample format {other}", device.name),
        }?;
        stream.play().context("starting the output stream")?;
        Ok(Self {
            _stream: stream,
            observed_frames,
        })
    }
}

/// Everything the audio callback owns, so [`Stream::open`] can build it once and hand it to
/// whichever sample-format instantiation the device turns out to want.
struct Callback<F> {
    on_cycle: F,
    channels: usize,
    sample_rate: u32,
    observed: Arc<AtomicU32>,
    /// Preallocated interleaved stereo — the buffer [`Cycle`] hands out. Sized at open; a host
    /// buffer longer than this is served in several cycles rather than by allocating here.
    scratch: Vec<f32>,
}

fn build<T, F>(
    device: &Device,
    config: &cpal::StreamConfig,
    mut ctx: Callback<F>,
    on_error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
    F: FnMut(Cycle<'_>) + Send + 'static,
{
    device
        .device
        .build_output_stream::<T, _, _>(
            config,
            move |data, info| ctx.run(data, info),
            on_error,
            None,
        )
        .with_context(|| format!("opening an output stream on {}", device.name))
}

impl<F: FnMut(Cycle<'_>) + Send + 'static> Callback<F> {
    /// One cpal callback: render our stereo mix into the scratch and spread it over the
    /// device's channels. Realtime thread — no allocation, no lock, no log.
    fn run<T>(&mut self, data: &mut [T], info: &cpal::OutputCallbackInfo)
    where
        T: SizedSample + FromSample<f32>,
    {
        let frames = data.len() / self.channels;
        if frames == 0 {
            return;
        }
        self.observed.store(frames as u32, Ordering::Relaxed);
        // The device timeline, on our clock: how far ahead of this callback the audio it fills
        // is due. `duration_since` is `None` only if a host reports playback before callback,
        // which would make the lead meaningless — report no timestamp rather than a wrong one.
        let ts = info.timestamp();
        let due = match ts.playback.duration_since(&ts.callback) {
            Some(ahead) => now_ns() + ahead.as_nanos() as u64,
            None => 0,
        };
        let per_cycle = self.scratch.len() / CHANNELS as usize;
        let mut done = 0;
        while done < frames {
            let take = (frames - done).min(per_cycle);
            let stereo = &mut self.scratch[..take * CHANNELS as usize];
            (self.on_cycle)(Cycle {
                buffer: stereo,
                frames: take,
                // A split buffer's later parts are due later; a zero stamp stays zero.
                output_time_ns: if due == 0 {
                    0
                } else {
                    due + (done as u64 * 1_000_000_000) / u64::from(self.sample_rate.max(1))
                },
            });
            spread(stereo, &mut data[done * self.channels..], self.channels);
            done += take;
        }
    }
}

/// Write an interleaved stereo block over a device buffer of `channels` channels.
///
/// Stereo is the mix's own shape, and every host here takes the device's channel count
/// literally — there is no HAL output unit above us doing the map, the way `coreaudio.rs` has.
/// So: mono averages the pair, stereo copies, and anything wider puts L and R on the first two
/// channels and silences the rest. That last case is deliberately *not* an upmix — the
/// reference is a stereo client, and inventing a centre or a surround feed would be our
/// invention, not its.
fn spread<T: SizedSample + FromSample<f32>>(stereo: &[f32], out: &mut [T], channels: usize) {
    match channels {
        1 => {
            for (frame, slot) in stereo.as_chunks::<2>().0.iter().zip(out.iter_mut()) {
                *slot = T::from_sample((frame[0] + frame[1]) * 0.5);
            }
        }
        2 => {
            for (sample, slot) in stereo.iter().zip(out.iter_mut()) {
                *slot = T::from_sample(*sample);
            }
        }
        n => {
            for (frame, slot) in stereo
                .as_chunks::<2>()
                .0
                .iter()
                .zip(out.chunks_exact_mut(n))
            {
                slot[0] = T::from_sample(frame[0]);
                slot[1] = T::from_sample(frame[1]);
                for quiet in &mut slot[2..] {
                    *quiet = T::from_sample(0.0f32);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Listeners

/// The flags the device watch raises; the backend polls them on the main thread. Same shape as
/// `coreaudio.rs`'s, so [`super`] reacts identically — with one field that stays zero here
/// ([`Notices::overloads`]: no host below has an analogue of `kAudioDeviceProcessorOverload`,
/// so the audible failure is counted as a ring underrun and nowhere else).
#[derive(Default)]
pub(super) struct Notices {
    pub default_changed: AtomicBool,
    pub device_died: AtomicBool,
    pub rate_changed: AtomicBool,
    pub overloads: AtomicU64,
    pub last_overload_ns: AtomicU64,
}

/// The device watch: a thread that re-reads the host's default output every [`WATCH_EVERY`] and
/// raises a notice when it is no longer the device we are on. cpal has no notification API —
/// this poll is the mechanism kira's own cpal backend uses off macOS, and it is why it was
/// compiled out *on* macOS (kira #38: enumerating devices mid-stream crackles there). One-shot:
/// it stops at the first notice, because the backend's answer to one is to rebuild the stream,
/// which arms a fresh watch on the device it landed on.
pub(super) struct Listeners {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Listeners {
    pub(super) fn arm(device: &Device, notices: Arc<Notices>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let watching = Arc::clone(&stop);
        let (was_id, was_name, was_rate) =
            (device.id.clone(), device.name.clone(), device.sample_rate);
        let thread = std::thread::Builder::new()
            .name("audio-devwatch".into())
            .spawn(move || {
                while !watching.load(Ordering::Acquire) {
                    std::thread::park_timeout(WATCH_EVERY);
                    if watching.load(Ordering::Acquire) {
                        return;
                    }
                    let Some(now) = cpal::default_host().default_output_device() else {
                        notices.device_died.store(true, Ordering::Release);
                        return;
                    };
                    // Identity by the host's own id where there is one, by display name where
                    // there is not. A device we could not read at all is a transient, not a
                    // change — `None` here says nothing and looks again next tick, rather than
                    // forcing a rebuild onto the device we are already on.
                    let changed = match (&was_id, now.id()) {
                        (Some(was), Ok(is_now)) => Some(*was != is_now),
                        _ => now.description().ok().map(|d| d.name() != was_name),
                    };
                    if changed == Some(true) {
                        notices.default_changed.store(true, Ordering::Release);
                        return;
                    }
                    if now
                        .default_output_config()
                        .is_ok_and(|c| c.sample_rate() != was_rate)
                    {
                        notices.rate_changed.store(true, Ordering::Release);
                        return;
                    }
                }
            })
            .ok();
        if thread.is_none() {
            // A watch we could not spawn is a degraded instrument, not a dead stream: the mix
            // plays, it just will not follow a device change until the next open.
            bevy::log::warn!("audio: no device watch — device changes will not be followed");
        }
        Self { stop, thread }
    }
}

impl Drop for Listeners {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Realtime scheduling for the render thread

/// No audio workgroups off macOS — no scheduler here has a notion of "the threads working
/// toward this device's deadline". [`set_realtime`] is the whole story.
pub(super) struct Workgroup;

impl Workgroup {
    pub(super) fn of_device(_device: &Device) -> Option<Self> {
        None
    }
}

pub(super) struct Joined;

impl Joined {
    pub(super) fn join(_group: Workgroup) -> Result<Self> {
        bail!("no audio workgroups on this platform")
    }
}

/// The render thread's realtime standing, released on drop.
pub(super) struct Realtime {
    #[cfg(windows)]
    _task: mmcss::Task,
}

/// Give the calling thread the strongest scheduling standing the OS will grant an audio worker.
/// It may be refused, and [`super`]'s caller then falls back — the mix-ahead ring is what makes
/// that a degradation rather than a crackle.
///
/// - **unix** (Linux, the BSDs): `SCHED_FIFO`. Refused with `EPERM` unless the process has
///   `RLIMIT_RTPRIO` headroom — which a desktop session gets from its audio group or from
///   PipeWire's own limits file, and a bare login shell does not. Priority 10 is deliberately
///   modest: it is inside the ceiling rtkit hands its own clients (20), so we sit below the
///   sound server's threads rather than above them. There is no busy-wait to starve a core
///   with — the loop parks between chunks.
/// - **Windows**: MMCSS, task "Pro Audio" at `AVRT_PRIORITY_CRITICAL` — the documented way for
///   a thread that is not the audio engine's own to be scheduled like one, and what every
///   Windows audio stack (WASAPI's own samples, cpal, `audio_thread_priority`) asks for.
///
/// `period_ns` is the render chunk's duration; only a time-constraint scheduler (macOS) takes
/// one, so it is unused here.
pub(super) fn set_realtime(_period_ns: u64) -> Result<Realtime> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // SAFETY: a POSIX call against the calling thread with a zeroed, correctly typed
        // parameter block — `sched_param` carries reserved fields on some targets, so it is
        // zeroed rather than listed.
        let rc = unsafe {
            let mut param: libc::sched_param = std::mem::zeroed();
            param.sched_priority = 10;
            libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param)
        };
        if rc != 0 {
            let why = std::io::Error::from_raw_os_error(rc);
            bail!("pthread_setschedparam(SCHED_FIFO, 10) failed: {why}");
        }
        Ok(Realtime {})
    }
    #[cfg(windows)]
    {
        Ok(Realtime {
            _task: mmcss::Task::join("Pro Audio")?,
        })
    }
    #[cfg(not(any(all(unix, not(target_os = "macos")), windows)))]
    {
        bail!("no realtime thread policy on this platform")
    }
}

/// MMCSS, declared here rather than pulled through a binding crate — three calls and a constant
/// are not worth a dependency, and it is the posture `coreaudio.rs` already takes with mach and
/// `os_workgroup`.
#[cfg(windows)]
mod mmcss {
    use anyhow::{bail, Result};

    /// `AVRT_PRIORITY_CRITICAL`.
    const PRIORITY_CRITICAL: i32 = 2;

    #[link(name = "avrt")]
    extern "system" {
        fn AvSetMmThreadCharacteristicsW(
            task_name: *const u16,
            task_index: *mut u32,
        ) -> *mut core::ffi::c_void;
        fn AvSetMmThreadPriority(handle: *mut core::ffi::c_void, priority: i32) -> i32;
        fn AvRevertMmThreadCharacteristics(handle: *mut core::ffi::c_void) -> i32;
    }

    /// The calling thread's membership in an MMCSS task, reverted on drop — on the same thread,
    /// which is where the render loop drops it.
    pub(super) struct Task(*mut core::ffi::c_void);

    // SAFETY: the handle is only ever touched by the thread that created it. The impl exists so
    // the enclosing `Realtime` has the same auto-traits on every target, not to move it.
    unsafe impl Send for Task {}

    impl Task {
        pub(super) fn join(name: &str) -> Result<Self> {
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let mut index = 0u32;
            // SAFETY: a NUL-terminated wide string and an out-parameter, per the API contract.
            let handle = unsafe { AvSetMmThreadCharacteristicsW(wide.as_ptr(), &mut index) };
            if handle.is_null() {
                bail!("AvSetMmThreadCharacteristicsW(\"{name}\") was refused");
            }
            // SAFETY: `handle` is the live registration just returned.
            if unsafe { AvSetMmThreadPriority(handle, PRIORITY_CRITICAL) } == 0 {
                // The task membership alone is most of the benefit; keep it, and say so.
                bevy::log::warn!("audio: MMCSS took the task but refused AVRT_PRIORITY_CRITICAL");
            }
            Ok(Self(handle))
        }
    }

    impl Drop for Task {
        fn drop(&mut self) {
            // SAFETY: mirrors the registration above, exactly once.
            unsafe { AvRevertMmThreadCharacteristics(self.0) };
        }
    }
}
