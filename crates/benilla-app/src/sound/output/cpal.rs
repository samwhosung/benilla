//! The non-macOS half of [`super`]: the same surface as `coreaudio.rs` ([`Device`], [`Stream`],
//! [`Cycle`], [`Notices`], [`Listeners`], [`set_realtime`], [`now_ns`]) over cpal, which speaks
//! ALSA, PipeWire, WASAPI and the rest. Where cpal differs from CoreAudio:
//!
//! - Cycle timestamps: cpal gives `callback` and `playback` instants, so
//!   [`Cycle::output_time_ns`] is `now + (playback - callback)` on our own clock.
//! - Device notices: cpal has no notification API, so [`Listeners`] polls the default device's
//!   identity and rate, backed by cpal's stream error callback. Nothing is an analogue of
//!   `kAudioDeviceProcessorOverload`, so the overload meter reads zero.
//! - Realtime: no workgroups; [`set_realtime`] is `SCHED_FIFO` on unix and MMCSS on Windows.
//!
//! An unsupported target compiles to cpal's Null host, reports no device and runs silent.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};

/// kira mixes stereo; [`spread`] puts it over whatever the device takes.
pub(super) const CHANNELS: u32 = 2;

/// How often [`Listeners`] re-reads the default output device, kira's own cadence off macOS.
const WATCH_EVERY: Duration = Duration::from_millis(500);

/// The most frames one [`Cycle`] carries (170 ms at 48 kHz); a bigger host buffer is served in
/// several cycles rather than allocating on the audio thread.
const MAX_CYCLE_FRAMES: usize = 8192;

/// Nanoseconds on a monotonic clock. cpal's `StreamInstant` is host-defined, so the meters ride
/// [`Instant`] and the lead comes from a difference of stream instants.
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
    /// The default config's rate, which we render at: nothing below us resamples.
    pub sample_rate: u32,
    /// The buffer sizes the host says it accepts, frames.
    pub buffer_range: (u32, u32),
    /// cpal exposes no device latency or safety offset; the report prints 0.
    pub latency_frames: u32,
    pub safety_frames: u32,
    /// The cpal handle and the config we open with.
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    /// The host's stable id, when it has one: what [`Listeners`] compares, since a display name
    /// is neither unique nor fixed.
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

/// The host's default output device; with none (headless, no sound server, an unsupported
/// target) the caller runs silent.
pub(super) fn default_output() -> Result<Device> {
    // Prime the clock's epoch on the main thread, not first in the audio callback.
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
    // The default config is the one the host guarantees it can open; cpal prefers f32 stereo.
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
        // The host takes no size from us; `Stream::open` asks for its default instead.
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

/// What the IO callback sees each cycle: the interleaved stereo buffer to fill and the time its
/// first frame reaches the DAC.
pub(super) struct Cycle<'a> {
    pub buffer: &'a mut [f32],
    pub frames: usize,
    /// When the first frame is due, ns on the [`now_ns`] clock; zero with no usable timestamp.
    pub output_time_ns: u64,
}

/// A running output stream. Dropping it stops the stream and drops the callback; cpal joins its
/// audio thread first.
pub(super) struct Stream {
    _stream: cpal::Stream,
    observed_frames: Arc<AtomicU32>,
}

impl Stream {
    /// The cycle size the last callback carried, frames, seeded with the request: cpal cannot
    /// say what the host granted, and WASAPI's shared mode varies it per wake.
    pub(super) fn buffer_frames(&self) -> u32 {
        self.observed_frames.load(Ordering::Relaxed)
    }

    /// Open `device` at its default config with a buffer of `buffer_frames` (clamped to what the
    /// host accepts) and start it. `on_cycle` runs on cpal's audio thread and must never block,
    /// allocate or log. `notices` gets cpal's stream error callback, the promptest sign of a lost
    /// device; the [`Listeners`] poll is the backstop.
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
        // Only `DeviceNotAvailable` and `StreamInvalidated` are fatal, as kira draws it; an ALSA
        // xrun (`BufferUnderrun`) or `BackendSpecific` must not tear the stream down.
        let reported = std::sync::atomic::AtomicBool::new(false);
        let on_error = move |e: cpal::StreamError| {
            match e {
                cpal::StreamError::DeviceNotAvailable | cpal::StreamError::StreamInvalidated => {
                    errors.device_died.store(true, Ordering::Release);
                }
                // Logged once per stream: this is the audio thread, and a starved device is
                // already counted as an underrun.
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
        // Every output format cpal can hand a typed callback for.
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

/// Everything the audio callback owns, built once for whichever sample format the device wants.
struct Callback<F> {
    on_cycle: F,
    channels: usize,
    sample_rate: u32,
    observed: Arc<AtomicU32>,
    /// Preallocated interleaved stereo, the buffer [`Cycle`] hands out.
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
    /// One cpal callback: render the stereo mix into the scratch and spread it over the
    /// device's channels. Realtime thread: no allocation, no lock, no log.
    fn run<T>(&mut self, data: &mut [T], info: &cpal::OutputCallbackInfo)
    where
        T: SizedSample + FromSample<f32>,
    {
        let frames = data.len() / self.channels;
        if frames == 0 {
            return;
        }
        self.observed.store(frames as u32, Ordering::Relaxed);
        // `None` only if a host reports playback before callback: no timestamp beats a wrong one.
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

/// Write an interleaved stereo block over `channels` device channels: mono averages the pair,
/// stereo copies, and wider puts L and R first and silences the rest, no upmix, since the
/// reference is a stereo client.
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

/// The flags the device watch raises and the main thread polls, shaped as `coreaudio.rs`'s;
/// [`Notices::overloads`] stays zero here.
#[derive(Default)]
pub(super) struct Notices {
    pub default_changed: AtomicBool,
    pub device_died: AtomicBool,
    pub rate_changed: AtomicBool,
    pub overloads: AtomicU64,
    pub last_overload_ns: AtomicU64,
}

/// The device watch: a thread that re-reads the host's default output every [`WATCH_EVERY`] and
/// raises a notice when it changes. One-shot: the backend answers a notice by rebuilding the
/// stream, which arms a fresh watch.
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
                    // By the host's id where there is one, else by name; an unreadable device
                    // is a transient (`None`), not a change.
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
            // The mix still plays; it just will not follow a device change until the next open.
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

/// No audio workgroups off macOS; [`set_realtime`] is the whole story.
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

/// Give the calling thread the strongest standing the OS grants an audio worker; it may be
/// refused, and the caller falls back.
///
/// - unix: `SCHED_FIFO` priority 10, below rtkit's client ceiling (20) so the sound server's
///   threads stay above us; `EPERM` without `RLIMIT_RTPRIO` headroom.
/// - Windows: MMCSS task "Pro Audio" at `AVRT_PRIORITY_CRITICAL`.
///
/// `period_ns` is only for macOS's time-constraint policy.
pub(super) fn set_realtime(_period_ns: u64) -> Result<Realtime> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // SAFETY: a POSIX call on the calling thread with a zeroed `sched_param`, zeroed since
        // it carries reserved fields on some targets.
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

/// MMCSS, declared here rather than through a binding crate.
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

    /// The calling thread's membership in an MMCSS task, reverted on drop on the same thread.
    pub(super) struct Task(*mut core::ffi::c_void);

    // SAFETY: only the creating thread touches the handle; the impl gives `Realtime` the same
    // auto-traits on every target.
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
                // The task membership alone is most of the benefit; keep it.
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
