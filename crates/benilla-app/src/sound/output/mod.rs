//! The output stream: benilla's own kira backend, the same code on every platform. It owns
//! everything between kira's render and the speaker: the device, the IO callback, the render
//! thread and the meters. The one platform seam is `device` (`coreaudio.rs` on macOS, `cpal.rs`
//! elsewhere), which finds the default output, opens a stream and hands the callback one buffer
//! per cycle.
//!
//! ## Mix ahead, copy on the deadline
//!
//! The IO callback runs on the host's realtime thread with a hard per-cycle budget, and anything
//! that parks it (a page-in, a stall) skips a cycle, which is a crackle. So it only copies: a
//! render thread runs kira's [`Renderer`] ahead into a lock-free ring,
//! [`OutputSettings::mix_ahead_ms`] deep, the reference's `SoundBufferSize`. That thread is
//! scheduled as the platform documents for an audio worker (`device::set_realtime`: a
//! time-constraint policy in the device's IO workgroup on macOS, `SCHED_FIFO` on unix, MMCSS on
//! Windows); any of those can be refused, and the ring's depth turns a refusal into a
//! degradation rather than a crackle.
//!
//! ## The meters, reported by [`OutputBackend::service`]
//!
//! - lead: at callback entry, the time the buffer is due at the DAC minus now; negative is late.
//! - io: the callback's wall time; anything past microseconds is the thread parked inside it.
//! - gap: spacing of successive cycle timestamps; a doubled gap is a skipped cycle.
//! - underruns: cycles the ring could not fill, the one audible failure left.
//! - render: the render thread's wall time per chunk.
//! - overloads: `kAudioDeviceProcessorOverload`, macOS only; zero elsewhere.
//!
//! Default-output changes, device loss and rate changes raise a notice, and `service` rebuilds
//! the stream on the new default. The callback and the render loop are allocation-free, checked
//! in debug builds by `assert_no_alloc`.

// The device layer, one file per platform behind one name.
#[cfg(target_os = "macos")]
#[path = "coreaudio.rs"]
mod device;
#[cfg(not(target_os = "macos"))]
#[path = "cpal.rs"]
mod device;

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use bevy::log::{debug, warn};
use kira::backend::{Backend, Renderer};
use rtrb::{Consumer, Producer, RingBuffer};

use device::{Cycle, Device, Joined, Listeners, Notices, Stream, Workgroup};

/// Frames per render pass: 5.3 ms at 48 kHz, two of kira's 128-frame parameter blocks.
const RENDER_CHUNK_FRAMES: usize = 256;

/// How long an unopenable device waits before the next attempt.
const REOPEN_EVERY: Duration = Duration::from_secs(1);

/// How long the main thread waits for the IO closure to hand the ring consumer back after a
/// stream drops; the stop is synchronous, so this is a defence.
const HANDBACK_WAIT: Duration = Duration::from_millis(250);

/// The device IO buffer we ask for; it sets how often the host wakes us, not any compute. 512 is
/// FMOD's and Godot's block on macOS, where a realtime workgroup thread at larger buffers can be
/// parked on an efficiency core between wakes.
pub(super) const DEVICE_BUFFER_FRAMES: u32 = 512;

/// How far ahead of the device the mix runs. The reference registers `SoundBufferSize`, FMOD 3's
/// mix-ahead, at `"50"` or `"100"` by host (`0x457520`, strings at `0x835e10`/`0x835e0c`); this is
/// the larger on every host, since the depth exists to hide a stall a whole IO cycle long.
pub(super) const MIX_AHEAD_MS: u32 = 100;

/// Whether an output stream is open, set only by the backend around the stream's life. The stall
/// watchdog stands down while it is set: `/usr/bin/sample` suspends the IO thread too, and every
/// cycle it holds is a crackle.
static DEVICE_OPEN: AtomicBool = AtomicBool::new(false);

/// True while an output stream is open on a device.
// The one reader, the stall watchdog (`perf::stall`), is macOS-only.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn device_open() -> bool {
    DEVICE_OPEN.load(Ordering::Acquire)
}

/// The default device's rate, read without opening it, for what is sized before the backend
/// exists (the tap's WAV header, the limiter's delay).
pub(super) fn probe_sample_rate() -> Option<u32> {
    device::default_output().ok().map(|d| d.sample_rate)
}

/// The dials the mixer opens the device with.
#[derive(Clone, Copy, Debug)]
pub(super) struct OutputSettings {
    /// The device IO buffer in frames, clamped to the device's range.
    pub device_buffer_frames: u32,
    /// The mix-ahead in ms, the reference's `SoundBufferSize`: the stall the output absorbs.
    pub mix_ahead_ms: u32,
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            device_buffer_frames: DEVICE_BUFFER_FRAMES,
            mix_ahead_ms: MIX_AHEAD_MS,
        }
    }
}

/// One `service` window's readings, in the units the report prints.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Window {
    /// IOProc calls in the window.
    pub cycles: u64,
    /// Smallest lead at IOProc entry (ms; negative = late). `None` when no cycle stamped one.
    pub lead_min_ms: Option<f64>,
    /// Longest IOProc wall time (ms).
    pub io_wall_max_ms: f64,
    /// Longest spacing between cycle output times (ms). Nominal is one device buffer.
    pub gap_max_ms: f64,
    /// Cycles the ring could not fill, and the audible silence that cost (ms).
    pub underruns: u64,
    pub underrun_ms: f64,
    /// The ring's low-water mark at IOProc entry (ms of audio banked).
    pub ring_min_ms: Option<f64>,
    /// Longest render pass for one chunk (ms), and how many chunks ran.
    pub render_wall_max_ms: f64,
    pub render_chunks: u64,
    /// HAL overloads in the window, and how long ago the last one fired (ms).
    pub overloads: u64,
    pub last_overload_ago_ms: Option<f64>,
    /// Nominal figures for the report: one device buffer (ms) and one render chunk (ms).
    pub cycle_ms: f64,
    pub chunk_ms: f64,
}

impl Window {
    /// Fold a later window into this one: sums add, extrema keep the extreme, nominals follow.
    pub(super) fn merge(&mut self, later: Window) {
        self.cycles += later.cycles;
        self.lead_min_ms = match (self.lead_min_ms, later.lead_min_ms) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.io_wall_max_ms = self.io_wall_max_ms.max(later.io_wall_max_ms);
        self.gap_max_ms = self.gap_max_ms.max(later.gap_max_ms);
        self.underruns += later.underruns;
        self.underrun_ms += later.underrun_ms;
        self.ring_min_ms = match (self.ring_min_ms, later.ring_min_ms) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.render_wall_max_ms = self.render_wall_max_ms.max(later.render_wall_max_ms);
        self.render_chunks += later.render_chunks;
        self.overloads += later.overloads;
        if later.last_overload_ago_ms.is_some() {
            self.last_overload_ago_ms = later.last_overload_ago_ms;
        }
        self.cycle_ms = later.cycle_ms;
        self.chunk_ms = later.chunk_ms;
    }
}

/// Something `service` did or saw that the mixer should log.
#[derive(Debug)]
pub(super) enum Event {
    Opened {
        device: String,
        sample_rate: u32,
        buffer_frames: u32,
        mix_ahead_ms: u32,
        realtime_latency_frames: u32,
    },
    Lost(&'static str),
    OpenFailed(String),
    /// The stream is gone for good: the ring's consumer never came back, a bug, not a device.
    Dead(String),
}

/// Meters written on the IO thread and the render thread, drained by `service`. Atomics only.
#[derive(Default)]
struct Meters {
    cycles: AtomicU64,
    lead_min_ns: AtomicI64,
    io_wall_max_ns: AtomicU64,
    gap_max_ns: AtomicU64,
    underruns: AtomicU64,
    underrun_frames: AtomicU64,
    ring_min_frames: AtomicUsize,
    render_wall_max_ns: AtomicU64,
    render_chunks: AtomicU64,
}

impl Meters {
    fn reset_extrema(&self) {
        self.lead_min_ns.store(i64::MAX, Ordering::Relaxed);
        self.ring_min_frames.store(usize::MAX, Ordering::Relaxed);
    }
}

/// What the main thread tells the render thread. The render thread only ever `try_lock`s the
/// mutex, and only `service` writes it, so the audio path never blocks on it.
#[derive(Default)]
struct Control {
    stop: AtomicBool,
    /// A sample rate the renderer must switch to (0 = none pending).
    pending_rate: AtomicU32,
    /// A workgroup to (re)join after a device change.
    regroup: AtomicBool,
    new_group: Mutex<Option<Workgroup>>,
    /// The render thread, for the IOProc's wake-up.
    render_thread: OnceLock<std::thread::Thread>,
}

#[derive(Default)]
struct Shared {
    meters: Meters,
    control: Control,
    notices: Arc<Notices>,
}

/// Hands `T` back through a one-slot ring when dropped: how the ring consumer in the IO closure
/// returns to the main thread when a stream is torn down.
struct Returning<T> {
    inner: Option<T>,
    back: Producer<T>,
}

impl<T> Returning<T> {
    fn new(value: T) -> (Self, Consumer<T>) {
        let (back, receiver) = RingBuffer::new(1);
        (
            Self {
                inner: Some(value),
                back,
            },
            receiver,
        )
    }

    fn get_mut(&mut self) -> &mut T {
        self.inner.as_mut().expect("value present until drop")
    }
}

impl<T> Drop for Returning<T> {
    fn drop(&mut self) {
        if let Some(value) = self.inner.take() {
            let _ = self.back.push(value);
        }
    }
}

enum Stage {
    /// `setup` ran; `start` has not.
    Set(Device),
    Running {
        stream: Stream,
        _listeners: Listeners,
        handback: Consumer<Consumer<f32>>,
    },
    /// No stream; the ring consumer is ours; retrying.
    Idle {
        consumer: Consumer<f32>,
        since: Instant,
    },
    Dead,
}

/// benilla's kira backend: the render thread, the ring, the device stream and its meters.
pub(super) struct OutputBackend {
    settings: OutputSettings,
    shared: Arc<Shared>,
    stage: Stage,
    sample_rate: u32,
    buffer_frames: u32,
    render: Option<std::thread::JoinHandle<()>>,
    /// Events raised before the first `service` (the initial open), drained by it.
    pending: Vec<Event>,
    overloads_seen: u64,
    /// Errors the report counts: stream lost / open failed, since launch.
    stream_errors: u64,
}

impl Backend for OutputBackend {
    type Settings = OutputSettings;
    type Error = anyhow::Error;

    fn setup(settings: Self::Settings, _internal_buffer_size: usize) -> Result<(Self, u32)> {
        let device = device::default_output()?;
        let shared = Arc::new(Shared::default());
        shared.meters.reset_extrema();
        let sample_rate = device.sample_rate;
        Ok((
            Self {
                settings,
                shared,
                stage: Stage::Set(device),
                sample_rate,
                buffer_frames: settings.device_buffer_frames,
                render: None,
                pending: Vec::new(),
                overloads_seen: 0,
                stream_errors: 0,
            },
            sample_rate,
        ))
    }

    fn start(&mut self, renderer: Renderer) -> Result<()> {
        let Stage::Set(device) = std::mem::replace(&mut self.stage, Stage::Dead) else {
            anyhow::bail!("output backend started twice");
        };
        let buffer_frames = self
            .settings
            .device_buffer_frames
            .clamp(device.buffer_range.0, device.buffer_range.1);
        // Guaranteed slack is the ring minus one device buffer (the IOProc drains a buffer's
        // worth per wake before the render thread tops up), so the ring is sized for both.
        let ahead_frames = ms_to_frames(self.settings.mix_ahead_ms, self.sample_rate);
        let ring_frames = ahead_frames + buffer_frames as usize;
        let (producer, consumer) = RingBuffer::<f32>::new(ring_frames * 2);

        let group = Workgroup::of_device(&device);
        let shared = Arc::clone(&self.shared);
        let period_ns = frames_to_ns(RENDER_CHUNK_FRAMES, self.sample_rate);
        let handle = std::thread::Builder::new()
            .name("audio-render".into())
            .spawn(move || render_loop(renderer, producer, shared, group, period_ns))
            .context("spawning the render thread")?;
        let _ = self
            .shared
            .control
            .render_thread
            .set(handle.thread().clone());
        self.render = Some(handle);

        self.stage = Stage::Idle {
            consumer,
            since: Instant::now() - REOPEN_EVERY,
        };
        if let Some(event) = self.open(device) {
            self.pending.push(event);
        }
        Ok(())
    }
}

impl OutputBackend {
    /// The rate the renderer runs at (follows the device across rebuilds).
    pub(super) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Stream-side failures since launch (a lost device, a refused open).
    pub(super) fn stream_errors(&self) -> u64 {
        self.stream_errors
    }

    /// Main-thread service: rebuild the stream on a device notice, retry an unopened device, and
    /// drain the window's meters.
    pub(super) fn service(&mut self) -> (Window, Vec<Event>) {
        let mut events = std::mem::take(&mut self.pending);
        let notices = &self.shared.notices;
        let lost = if notices.device_died.swap(false, Ordering::AcqRel) {
            Some("the output device went away")
        } else if notices.default_changed.swap(false, Ordering::AcqRel) {
            Some("the system default output changed")
        } else if notices.rate_changed.swap(false, Ordering::AcqRel) {
            Some("the device's sample rate changed")
        } else {
            None
        };
        if let Some(why) = lost {
            if matches!(self.stage, Stage::Running { .. }) {
                self.stream_errors += 1;
                events.push(Event::Lost(why));
                if let Err(e) = self.close() {
                    self.stage = Stage::Dead;
                    events.push(Event::Dead(format!("{e:#}")));
                }
            }
        }
        if let Stage::Idle { since, .. } = &self.stage {
            if since.elapsed() >= REOPEN_EVERY {
                match device::default_output() {
                    Ok(device) => {
                        if let Some(event) = self.open(device) {
                            events.push(event);
                        }
                    }
                    Err(e) => {
                        if let Stage::Idle { since, .. } = &mut self.stage {
                            *since = Instant::now();
                        }
                        self.stream_errors += 1;
                        events.push(Event::OpenFailed(format!("{e:#}")));
                    }
                }
            }
        }
        (self.take_window(), events)
    }

    /// Open a stream on `device` with the ring consumer held in `Stage::Idle`; on failure the
    /// stage stays `Idle` with the retry clock reset.
    fn open(&mut self, device: Device) -> Option<Event> {
        let Stage::Idle { consumer, .. } = std::mem::replace(&mut self.stage, Stage::Dead) else {
            self.stage = Stage::Dead;
            return Some(Event::Dead("open called without the ring consumer".into()));
        };
        if device.sample_rate != self.sample_rate {
            self.sample_rate = device.sample_rate;
            self.shared
                .control
                .pending_rate
                .store(device.sample_rate, Ordering::Release);
            if let Some(thread) = self.shared.control.render_thread.get() {
                thread.unpark();
            }
        }
        let (mut returning, handback) = Returning::new(consumer);
        let shared = Arc::clone(&self.shared);
        let mut last_output_ns = 0u64;
        // `returning` hands the ring consumer back when the closure drops with the stream.
        let on_cycle = move |cycle: Cycle<'_>| {
            io_cycle(cycle, returning.get_mut(), &shared, &mut last_output_ns);
        };
        match Stream::open(
            &device,
            self.settings.device_buffer_frames,
            &self.shared.notices,
            on_cycle,
        ) {
            Ok(stream) => {
                self.buffer_frames = stream.buffer_frames();
                // The render thread joins the new device's workgroup on its next pass.
                if let Some(group) = Workgroup::of_device(&device) {
                    if let Ok(mut slot) = self.shared.control.new_group.lock() {
                        *slot = Some(group);
                        self.shared.control.regroup.store(true, Ordering::Release);
                    }
                }
                let listeners = Listeners::arm(&device, Arc::clone(&self.shared.notices));
                self.shared.meters.reset_extrema();
                DEVICE_OPEN.store(true, Ordering::Release);
                let event = Event::Opened {
                    device: device.name.clone(),
                    sample_rate: device.sample_rate,
                    buffer_frames: stream.buffer_frames(),
                    mix_ahead_ms: self.settings.mix_ahead_ms,
                    realtime_latency_frames: device.latency_frames + device.safety_frames,
                };
                self.stage = Stage::Running {
                    stream,
                    _listeners: listeners,
                    handback,
                };
                Some(event)
            }
            Err(e) => {
                self.stream_errors += 1;
                match take_back(handback) {
                    Some(consumer) => {
                        self.stage = Stage::Idle {
                            consumer,
                            since: Instant::now(),
                        };
                        Some(Event::OpenFailed(format!("{} — {e:#}", device.name)))
                    }
                    None => {
                        self.stage = Stage::Dead;
                        Some(Event::Dead(format!(
                            "ring consumer lost while opening {} — {e:#}",
                            device.name
                        )))
                    }
                }
            }
        }
    }

    /// Tear the running stream down and take the ring consumer back.
    fn close(&mut self) -> Result<()> {
        let Stage::Running {
            stream,
            _listeners,
            handback,
        } = std::mem::replace(&mut self.stage, Stage::Dead)
        else {
            anyhow::bail!("close without a running stream");
        };
        DEVICE_OPEN.store(false, Ordering::Release);
        drop(_listeners);
        drop(stream);
        let consumer = take_back(handback).context("the IO closure never returned the ring")?;
        self.stage = Stage::Idle {
            consumer,
            since: Instant::now() - REOPEN_EVERY,
        };
        Ok(())
    }

    fn take_window(&mut self) -> Window {
        // The running cycle size, not the one asked for: a host may clamp it, and WASAPI's
        // shared mode varies it per wake.
        if let Stage::Running { stream, .. } = &self.stage {
            self.buffer_frames = stream.buffer_frames();
        }
        let m = &self.shared.meters;
        let rate = f64::from(self.sample_rate.max(1));
        let frames_ms = |frames: f64| frames / rate * 1000.0;
        let ns_ms = |ns: u64| ns as f64 / 1e6;
        let lead_min = m.lead_min_ns.swap(i64::MAX, Ordering::Relaxed);
        let ring_min = m.ring_min_frames.swap(usize::MAX, Ordering::Relaxed);
        let overloads_total = self.shared.notices.overloads.load(Ordering::Relaxed);
        let overloads = overloads_total - self.overloads_seen;
        self.overloads_seen = overloads_total;
        let last_overload_ago_ms = (overloads > 0).then(|| {
            let at = self.shared.notices.last_overload_ns.load(Ordering::Relaxed);
            ns_ms(device::now_ns().saturating_sub(at))
        });
        Window {
            cycles: m.cycles.swap(0, Ordering::Relaxed),
            lead_min_ms: (lead_min != i64::MAX).then(|| lead_min as f64 / 1e6),
            io_wall_max_ms: ns_ms(m.io_wall_max_ns.swap(0, Ordering::Relaxed)),
            gap_max_ms: ns_ms(m.gap_max_ns.swap(0, Ordering::Relaxed)),
            underruns: m.underruns.swap(0, Ordering::Relaxed),
            underrun_ms: frames_ms(m.underrun_frames.swap(0, Ordering::Relaxed) as f64),
            ring_min_ms: (ring_min != usize::MAX).then(|| frames_ms(ring_min as f64)),
            render_wall_max_ms: ns_ms(m.render_wall_max_ns.swap(0, Ordering::Relaxed)),
            render_chunks: m.render_chunks.swap(0, Ordering::Relaxed),
            overloads,
            last_overload_ago_ms,
            cycle_ms: frames_ms(f64::from(self.buffer_frames)),
            chunk_ms: frames_ms(RENDER_CHUNK_FRAMES as f64),
        }
    }
}

impl Drop for OutputBackend {
    fn drop(&mut self) {
        // Stream first (no IOProc after this), then the render thread.
        DEVICE_OPEN.store(false, Ordering::Release);
        self.stage = Stage::Dead;
        self.shared.control.stop.store(true, Ordering::Release);
        if let Some(thread) = self.shared.control.render_thread.get() {
            thread.unpark();
        }
        if let Some(handle) = self.render.take() {
            let _ = handle.join();
        }
    }
}

/// Wait (briefly) for the ring consumer to come back out of a dropped IO closure.
fn take_back(mut handback: Consumer<Consumer<f32>>) -> Option<Consumer<f32>> {
    let deadline = Instant::now() + HANDBACK_WAIT;
    loop {
        if let Ok(consumer) = handback.pop() {
            return Some(consumer);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn ms_to_frames(ms: u32, sample_rate: u32) -> usize {
    (u64::from(ms) * u64::from(sample_rate) / 1000) as usize
}

fn frames_to_ns(frames: usize, sample_rate: u32) -> u64 {
    (frames as u64 * 1_000_000_000) / u64::from(sample_rate.max(1))
}

/// Run `f` with allocation forbidden: debug builds abort on a violation, release runs it bare.
#[inline]
fn no_alloc<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(debug_assertions)]
    {
        assert_no_alloc::assert_no_alloc(f)
    }
    #[cfg(not(debug_assertions))]
    {
        f()
    }
}

/// The IO callback: copy one device buffer out of the ring and stamp the meters. Realtime
/// thread: no allocation, no lock, no log.
fn io_cycle(
    cycle: Cycle<'_>,
    consumer: &mut Consumer<f32>,
    shared: &Shared,
    last_output_ns: &mut u64,
) {
    no_alloc(|| {
        let entry = device::now_ns();
        let meters = &shared.meters;
        if cycle.output_time_ns != 0 {
            let lead = cycle.output_time_ns as i64 - entry as i64;
            meters.lead_min_ns.fetch_min(lead, Ordering::Relaxed);
            if *last_output_ns != 0 {
                let gap = cycle.output_time_ns.saturating_sub(*last_output_ns);
                meters.gap_max_ns.fetch_max(gap, Ordering::Relaxed);
            }
            *last_output_ns = cycle.output_time_ns;
        }
        let out = cycle.buffer;
        let need = out.len();
        let banked = consumer.slots() / 2 * 2;
        meters
            .ring_min_frames
            .fetch_min(banked / 2, Ordering::Relaxed);
        let take = banked.min(need);
        if take > 0 {
            if let Ok(chunk) = consumer.read_chunk(take) {
                let (a, b) = chunk.as_slices();
                out[..a.len()].copy_from_slice(a);
                out[a.len()..a.len() + b.len()].copy_from_slice(b);
                chunk.commit_all();
            }
        }
        if take < need {
            out[take..].fill(0.0);
            meters.underruns.fetch_add(1, Ordering::Relaxed);
            meters
                .underrun_frames
                .fetch_add(((need - take) / 2) as u64, Ordering::Relaxed);
        }
        if let Some(thread) = shared.control.render_thread.get() {
            thread.unpark();
        }
        meters.cycles.fetch_add(1, Ordering::Relaxed);
        meters
            .io_wall_max_ns
            .fetch_max(device::now_ns().saturating_sub(entry), Ordering::Relaxed);
        let _ = cycle.frames;
    });
}

/// The render thread: keep the ring full, one chunk at a time, woken by the IO callback after
/// every cycle and by a timeout so a missed wake cannot starve it.
// The explicit `drop`s matter on macOS, where `Joined`'s `Drop` leaves the workgroup: the old
// membership goes before the new one is taken. Off macOS `Joined` has no `Drop`.
#[allow(clippy::drop_non_drop)]
fn render_loop(
    mut renderer: Renderer,
    mut producer: Producer<f32>,
    shared: Arc<Shared>,
    group: Option<Workgroup>,
    period_ns: u64,
) {
    // Held for the thread's life: on Windows its drop hands the MMCSS registration back.
    // `WOW_AUDIO_NO_RT=1` skips realtime for the user-interactive QoS fallback, an A/B lever for
    // the render thread preempting the compute pools.
    let no_rt = std::env::var_os("WOW_AUDIO_NO_RT").is_some();
    let realtime = if no_rt {
        Err(anyhow::anyhow!("WOW_AUDIO_NO_RT=1"))
    } else {
        device::set_realtime(period_ns)
    };
    let _realtime = match realtime {
        Ok(realtime) => {
            debug!(
                "audio: render thread scheduled realtime, period {} µs",
                period_ns / 1000
            );
            Some(realtime)
        }
        Err(e) => {
            warn!("audio: render thread could not go realtime ({e}); using user-interactive QoS");
            benilla_world::thread_qos::promote_current_thread(
                benilla_world::thread_qos::QosClass::UserInteractive,
            );
            None
        }
    };
    let mut joined = group.and_then(|g| match Joined::join(g) {
        Ok(j) => {
            debug!("audio: render thread joined the device's IO workgroup");
            Some(j)
        }
        Err(e) => {
            warn!("audio: render thread could not join the device workgroup ({e})");
            None
        }
    });
    let mut chunk = vec![0f32; RENDER_CHUNK_FRAMES * device::CHANNELS as usize];
    let period = Duration::from_nanos(period_ns);
    let control = &shared.control;
    let meters = &shared.meters;
    loop {
        if control.stop.load(Ordering::Acquire) {
            break;
        }
        let rate = control.pending_rate.swap(0, Ordering::AcqRel);
        if rate != 0 {
            // Off the no-alloc path: effects may resize their state here.
            renderer.on_change_sample_rate(rate);
        }
        if control.regroup.swap(false, Ordering::AcqRel) {
            if let Ok(mut slot) = control.new_group.try_lock() {
                if let Some(group) = slot.take() {
                    drop(joined.take());
                    joined = Joined::join(group).ok();
                }
            } else {
                control.regroup.store(true, Ordering::Release);
            }
        }
        while producer.slots() >= chunk.len() {
            let start = device::now_ns();
            no_alloc(|| {
                renderer.on_start_processing();
                renderer.process(&mut chunk, device::CHANNELS as u16);
                if let Ok(mut slot) = producer.write_chunk(chunk.len()) {
                    let (a, b) = slot.as_mut_slices();
                    a.copy_from_slice(&chunk[..a.len()]);
                    b.copy_from_slice(&chunk[a.len()..a.len() + b.len()]);
                    slot.commit_all();
                }
            });
            meters.render_chunks.fetch_add(1, Ordering::Relaxed);
            meters
                .render_wall_max_ns
                .fetch_max(device::now_ns().saturating_sub(start), Ordering::Relaxed);
        }
        std::thread::park_timeout(period);
    }
    drop(joined);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opt-in (`WOW_AUDIO_LIVE=1`) since it opens the real output device: does the device layer
    /// open a stream, does the callback run, and does the render thread keep ahead? Off macOS it
    /// is the only run of this code.
    ///
    ///     WOW_AUDIO_LIVE=1 cargo test -p benilla-app --lib sound::output:: -- --nocapture
    ///
    /// A device with no clock fails the underrun check by construction: ALSA's `null` PCM
    /// accepts every write at once, so the callback free-runs far above `1 s / cycle_ms` cycles.
    /// A pacing sink (PulseAudio's `module-null-sink`) reads 0 underruns.
    #[test]
    fn live_output_opens_and_runs() {
        if std::env::var("WOW_AUDIO_LIVE").is_err() {
            eprintln!("skipped: set WOW_AUDIO_LIVE=1 to open the real output device");
            return;
        }
        let mut manager = kira::AudioManager::<OutputBackend>::new(kira::AudioManagerSettings::<
            OutputBackend,
        >::default())
        .expect("the output backend opened a device");
        // The stream opens inside `start`, so the first `service` carries the `Opened` event.
        std::thread::sleep(Duration::from_secs(1));
        let (window, events) = manager.backend_mut().service();
        for event in &events {
            eprintln!("event: {event:?}");
        }
        eprintln!("window: {window:?}");
        assert!(
            events.iter().any(|e| matches!(e, Event::Opened { .. })),
            "no device was opened: {events:?}"
        );
        assert!(window.cycles > 0, "the IO callback never ran: {window:?}");
        assert!(
            window.render_chunks > 0,
            "the render thread never produced a chunk: {window:?}"
        );
        assert_eq!(
            window.underruns, 0,
            "the ring ran dry — the render thread is not keeping ahead: {window:?}"
        );
    }
}
