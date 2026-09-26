//! The audio-backend seam: kira 0.12 behind a narrow, FMOD-shaped surface.
//!
//! The reference hands the whole mix (pan, rolloff, doppler, reverb, decode, streaming) to FMOD
//! 3.x and owns only the parameters it feeds; this seam keeps FMOD's import shape so the backend
//! stays swappable. kira's listener is X-right/Y-up, Bevy camera space, so camera transforms feed
//! in with no remap.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use bevy::prelude::*;
use kira::effect::reverb::{ReverbBuilder, ReverbHandle};
use kira::effect::volume_control::{VolumeControlBuilder, VolumeControlHandle};
use kira::listener::ListenerHandle;
use kira::sound::streaming::StreamingSoundData;
use kira::sound::FromFileError;
use kira::track::{SendTrackBuilder, SendTrackHandle, SpatialTrackBuilder};
use kira::{AudioManager, AudioManagerSettings, Decibels, Mix, Tween};

use super::limiter;
use super::meter::{self, LevelReading, MixLevel};
use super::output::{self, Event, OutputBackend, OutputSettings, Window};

/// The output backend's per-window meters, re-exported for the report in `sound::poll_mix_health`.
pub(super) type OutputWindow = Window;

pub(crate) use benilla_formats::SoundProvider;

// Handles crossing the seam. Dropping a sound handle does not stop the sound; dropping a spatial
// track handle removes the track once its sounds finish, so a fade-then-drop still fades.
pub(crate) use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
pub(crate) use kira::sound::streaming::StreamingSoundHandle;
pub(crate) use kira::track::SpatialTrackHandle;

/// A zero-duration tween, for a change that is a step (a reverb preset switch is instant,
/// `0x45a720`); the per-frame volume feed uses [`glide`], since a step there clicks.
pub(crate) fn snap() -> Tween {
    Tween {
        duration: std::time::Duration::ZERO,
        ..Default::default()
    }
}

/// The per-frame volume feed's ramp: kira holds a track's volume constant per 128-frame chunk,
/// so a stepped feed clicks once frame pacing goes unstable.
pub(crate) fn glide() -> Tween {
    Tween {
        duration: std::time::Duration::from_millis(GLIDE_MS),
        ..Default::default()
    }
}

/// [`glide`]'s ramp, just under one 60 fps frame.
const GLIDE_MS: u64 = 15;

/// The de-click fade for a force-stop; it survives dropping the handle.
pub(crate) fn declick() -> Tween {
    glide()
}

/// A linear fade over `ms`, like the reference's constant per-tick volume decrement (`0x7a5a50`).
pub(crate) fn fade(ms: u64) -> Tween {
    Tween {
        duration: std::time::Duration::from_millis(ms),
        ..Default::default()
    }
}

/// Linear amplitude (the reference's category mix, `0x7a5dc0`) to dB; amp ≤ 10⁻³, under one
/// 1/255 FMOD step, is kira's silence floor.
pub(crate) fn amp_to_db(amp: f32) -> Decibels {
    if amp <= 1e-3 {
        Decibels::SILENCE
    } else {
        Decibels(20.0 * amp.log10())
    }
}

/// The open audio device and its listener. No master filter: the reference applies no DSP beyond
/// FMOD's reverb (no `FSOUND_FX_*` import).
pub(crate) struct Mixer {
    manager: AudioManager<OutputBackend>,
    listener: ListenerHandle,
    /// Rolling mix-health counters ([`Mixer::poll_health`]).
    health: MixHealth,
    /// The output meters accumulated since the last [`Mixer::take_output_window`].
    window: Window,
    /// The wet-only zone-reverb send every 3D track routes into; its volume is the zone wet level.
    reverb_send: SendTrackHandle,
    /// The Freeverb parameters on the send, retuned per zone preset.
    reverb: ReverbHandle,
    /// What the mix asked for and what the limiter allowed, written on the audio thread.
    level: Arc<MixLevel>,
    /// The `SoundOutputLimiter` CVar cell the limiter reads each block.
    limiter_on: Arc<AtomicBool>,
    /// The master gain, first in the main chain, upstream of the limiter.
    master: VolumeControlHandle,
    /// The output gate, last in the main chain: unity or silence, never a level.
    output: VolumeControlHandle,
    /// The pre-limiter tap's frame clock while a probing run records.
    audio_pos: Option<Arc<AtomicU64>>,
    /// The device's negotiated sample rate, the health report's time axis.
    sample_rate: Option<u32>,
}

impl Mixer {
    /// Open the default audio device; without one the caller runs silent.
    pub(crate) fn new(probe_dir: Option<&Path>) -> Result<Self> {
        // The tap's WAV header and the limiter's delay line are sized at build time.
        let sample_rate = output::probe_sample_rate();
        let backend_settings = OutputSettings {
            mix_ahead_ms: mix_ahead_ms(),
            device_buffer_frames: io_buffer_frames(),
        };
        let level = Arc::new(MixLevel::default());
        let limiter_on = Arc::new(AtomicBool::new(true));
        let MainChain {
            builder: main_track_builder,
            master,
            output,
            audio_pos,
        } = main_track(&level, &limiter_on, sample_rate, probe_dir);
        let settings = AudioManagerSettings::<OutputBackend> {
            backend_settings,
            main_track_builder,
            capacities: kira::Capacities {
                sub_track_capacity: SPATIAL_VOICE_CAPACITY,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut manager = AudioManager::<OutputBackend>::new(settings)
            .map_err(|e| anyhow::anyhow!("audio device init: {e:#}"))?;
        let sample_rate = Some(manager.backend_mut().sample_rate());
        // Wet-only, silent until a zone preset raises it.
        let mut send_builder = SendTrackBuilder::new().volume(Decibels::SILENCE);
        let reverb = send_builder.add_effect(
            ReverbBuilder::new()
                .mix(Mix::WET)
                .feedback(0.5)
                .damping(0.5),
        );
        let reverb_send = manager
            .add_send_track(send_builder)
            .map_err(|e| anyhow::anyhow!("reverb send alloc: {e}"))?;
        let listener = manager
            .add_listener(
                mint::Vector3 {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                mint::Quaternion {
                    v: mint::Vector3 {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    },
                    s: 1.0,
                },
            )
            .map_err(|e| anyhow::anyhow!("listener alloc: {e}"))?;
        Ok(Self {
            manager,
            listener,
            health: MixHealth::default(),
            window: Window::default(),
            reverb_send,
            reverb,
            level,
            limiter_on,
            master,
            output,
            sample_rate,
            audio_pos,
        })
    }

    /// The probing run's shared time axis, the pre-limiter tap's frame clock.
    pub(super) fn audio_pos(&self) -> Option<Arc<AtomicU64>> {
        self.audio_pos.clone()
    }

    /// Apply a zone reverb preset: the reference turns the `SoundProviderPreferences` row into EAX
    /// listener properties (`0x45a790`, `0x7a5fa0`) and switches with no ramp (`0x45a720`);
    /// `None` is its silenced GENERIC default (`0x45a830`). The EAX-to-Freeverb projection is
    /// this backend's own lossy mapping:
    /// - `feedback = 10^(−0.108 / DecayTime)`, Freeverb's RT60 at its ~36 ms mean comb delay,
    ///   clamped to 0.98;
    /// - `damping` is `1 − DecayHFRatio/2` at weight 0.7 plus `−RoomHF/10000` at weight 0.3;
    /// - wet level is `(Room + Reverb)` mB in dB, capped at +6.
    pub(crate) fn set_reverb(&mut self, preset: Option<&SoundProvider>) {
        let Some(p) = preset else {
            self.reverb_send.set_volume(Decibels::SILENCE, snap());
            return;
        };
        let (feedback, damping, wet) = freeverb_projection(p);
        self.reverb.set_feedback(feedback, snap());
        self.reverb.set_damping(damping, snap());
        self.reverb_send.set_volume(wet, snap());
    }

    /// Per-frame listener pose from the world camera, in Bevy space.
    pub(crate) fn set_listener(&mut self, pos: Vec3, rot: Quat) {
        self.listener.set_position(
            mint::Vector3 {
                x: pos.x,
                y: pos.y,
                z: pos.z,
            },
            snap(),
        );
        self.listener.set_orientation(
            mint::Quaternion {
                v: mint::Vector3 {
                    x: rot.x,
                    y: rot.y,
                    z: rot.z,
                },
                s: rot.w,
            },
            snap(),
        );
    }

    /// Master volume as linear amplitude: the first effect in the main chain.
    pub(crate) fn set_master(&mut self, amp: f32) {
        self.master.set_volume(amp_to_db(amp), glide());
    }

    /// Open or shut the output gate, the reference's `FSOUND_SetMute(FSOUND_ALL, …)` on window
    /// activation; its mute is instant, the glide only de-clicks.
    pub(super) fn set_output_gate(&mut self, open: bool) {
        self.output.set_volume(
            if open {
                Decibels::IDENTITY
            } else {
                Decibels::SILENCE
            },
            glide(),
        );
    }

    /// Arm or bypass the output limiter (`SoundOutputLimiter`); its delay line runs either way,
    /// so the switch fades and never steps the output.
    pub(super) fn set_limiter(&mut self, on: bool) {
        self.limiter_on.store(on, Ordering::Relaxed);
    }

    /// Read and reset the mix's level for this report window.
    pub(super) fn take_level(&mut self) -> LevelReading {
        self.level.take()
    }

    /// The device's negotiated sample rate, the report's time axis.
    pub(super) fn sample_rate(&self) -> Option<u32> {
        self.sample_rate
    }

    /// Play a decoded SFX on the main track, the 2D/UI path.
    pub(crate) fn play_2d(&mut self, data: StaticSoundData) -> Result<StaticSoundHandle> {
        self.manager
            .play(data)
            .map_err(|e| anyhow::anyhow!("play 2d: {e}"))
    }

    /// Play a decoded sound on a fresh spatial track; the backend only pans, gain over distance
    /// is the kit pump's. `reverb_send` is whether `SoundEntries.EAXDef` resolves a slot
    /// (`0x458f1c` via `0x45cdc0`): `SoundSamplePreferences.dbc` has only ids 1 and 2, so
    /// `EAXDef 0` stays dry (`0x7a5bf0`). A slot sends at unity; its per-channel EAX properties
    /// are not applied.
    pub(crate) fn play_3d(
        &mut self,
        data: StaticSoundData,
        pos: Vec3,
        reverb_send: bool,
    ) -> Result<(SpatialTrackHandle, StaticSoundHandle)> {
        // One sound per track: kira's default capacity allocates for 128 on every play.
        let mut builder = SpatialTrackBuilder::new()
            .attenuation_function(None)
            .sound_capacity(1);
        if reverb_send {
            builder = builder.with_send(&self.reverb_send, Decibels(0.0));
        }
        let mut track = match self.manager.add_spatial_sub_track(
            self.listener.id(),
            mint::Vector3 {
                x: pos.x,
                y: pos.y,
                z: pos.z,
            },
            builder
                // Else a fade-stop-then-drop is cut mid-ramp; every dropped channel is stopped.
                .persist_until_sounds_finish(true),
        ) {
            Ok(t) => t,
            Err(e) => {
                // Counted: a sound dropped at the voice ceiling leaves no other trace.
                self.health.voices_refused += 1;
                return Err(anyhow::anyhow!(
                    "spatial track alloc ({SPATIAL_VOICE_CAPACITY}-voice ceiling): {e}"
                ));
            }
        };
        let handle = track
            .play(data)
            .map_err(|e| anyhow::anyhow!("play 3d: {e}"))?;
        Ok((track, handle))
    }

    /// Decode-stream a long sound (music, ambience MP3s) on the main track.
    pub(crate) fn play_stream(
        &mut self,
        data: StreamingSoundData<FromFileError>,
    ) -> Result<StreamingSoundHandle<FromFileError>> {
        self.manager
            .play(data)
            .map_err(|e| anyhow::anyhow!("play stream: {e}"))
    }
}

/// The ceiling on simultaneous 3D voices, well past a pack pull: which sounds play is the
/// reference's per-bus caps (`0x87ce60`), not an arena size.
const SPATIAL_VOICE_CAPACITY: usize = 512;

/// The main track's chain: master, meter (what the game asked for), limiter, mix tap (what is
/// heard), output gate. The gate is last so every tap still reads the mix while the window is in
/// the background, as an unattended run's always is.
fn main_track(
    level: &Arc<MixLevel>,
    limiter_on: &Arc<AtomicBool>,
    sample_rate: Option<u32>,
    probe_dir: Option<&Path>,
) -> MainChain {
    let mut main = kira::track::MainTrackBuilder::new();
    // An effect, not the track volume, which kira applies after the effects and so behind the
    // limiter.
    let master = main.add_effect(VolumeControlBuilder::new(Decibels::IDENTITY));
    meter::install(&mut main, level);
    // A probing run brackets the limiter with two taps.
    let probe = probe_dir.zip(sample_rate);
    let audio_pos = probe
        .and_then(|(dir, rate)| super::mix_tap::install_at(&mut main, &dir.join("pre.wav"), rate));
    limiter::install(&mut main, level, limiter_on);
    if let Some((dir, rate)) = probe {
        super::mix_tap::install_at(&mut main, &dir.join("post.wav"), rate);
    }
    let mut main = super::mix_tap::install(main, sample_rate);
    let output = main.add_effect(VolumeControlBuilder::new(Decibels::IDENTITY));
    MainChain {
        builder: main,
        master,
        output,
        audio_pos,
    }
}

/// What [`main_track`] hands back: the built chain and the handles into it.
struct MainChain {
    builder: kira::track::MainTrackBuilder,
    /// The master gain; the main track's own volume, behind the limiter, stays at unity.
    master: VolumeControlHandle,
    output: VolumeControlHandle,
    audio_pos: Option<Arc<AtomicU64>>,
}

/// `SoundBufferSize`, the mix-ahead in ms, read once from the stored config at sound init, as
/// the reference reads it.
fn mix_ahead_ms() -> u32 {
    crate::cvars::boot_cvar("SoundBufferSize")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|ms| ms.is_finite())
        .map_or(output::MIX_AHEAD_MS, |ms| {
            ms.round().clamp(10.0, 1000.0) as u32
        })
}

/// `$WOW_IO_BUFFER`: the device IO buffer in frames.
fn io_buffer_frames() -> u32 {
    std::env::var("WOW_IO_BUFFER")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(output::DEVICE_BUFFER_FRAMES)
}

/// The mix-health counters: a crackle in numbers.
#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct MixHealth {
    /// The last render pass's wall time over its chunk's duration; the thread mixes ahead, so
    /// `>= 1.0` is not yet audible.
    pub(crate) load: f32,
    /// Worst load seen since the last [`Mixer::take_health_peak`].
    pub(crate) peak_load: f32,
    /// Device cycles the mix-ahead ring could not fill since launch, each one output as silence.
    pub(crate) overruns: u64,
    /// Stream-side failures since launch (a lost device, a refused open).
    pub(crate) stream_errors: u64,
    /// 3D plays refused at [`SPATIAL_VOICE_CAPACITY`] since launch.
    pub(crate) voices_refused: u64,
}

impl Mixer {
    /// Service the output backend and fold its window into the health counters, every frame.
    pub(crate) fn poll_health(&mut self) -> MixHealth {
        let backend = self.manager.backend_mut();
        let (window, events) = backend.service();
        self.sample_rate = Some(backend.sample_rate());
        self.health.stream_errors = backend.stream_errors();
        for event in events {
            match event {
                Event::Opened {
                    device,
                    sample_rate,
                    buffer_frames,
                    mix_ahead_ms,
                    realtime_latency_frames,
                } => info!(
                    "audio: {device} — {sample_rate} Hz, IO buffer {buffer_frames} frames \
                     (~{:.1} ms), mix-ahead {mix_ahead_ms} ms, device latency ~{:.1} ms",
                    f64::from(buffer_frames) / f64::from(sample_rate) * 1000.0,
                    f64::from(realtime_latency_frames) / f64::from(sample_rate) * 1000.0,
                ),
                Event::Lost(why) => warn!("audio: output stream dropped — {why}; reopening"),
                Event::OpenFailed(what) => warn!("audio: output device refused — {what}; retrying"),
                Event::Dead(what) => error!("audio: output is gone for good — {what}"),
            }
        }
        if window.render_chunks > 0 && window.chunk_ms > 0.0 {
            self.health.load = (window.render_wall_max_ms / window.chunk_ms) as f32;
            self.health.peak_load = self.health.peak_load.max(self.health.load);
        }
        self.health.overruns += window.underruns;
        self.window.merge(window);
        self.health
    }

    /// Read and reset the peak load, so a report covers only its own window.
    pub(crate) fn take_health_peak(&mut self) -> f32 {
        std::mem::take(&mut self.health.peak_load)
    }

    /// The output meters accumulated since the last take.
    pub(crate) fn take_output_window(&mut self) -> Window {
        std::mem::take(&mut self.window)
    }
}

/// Move a live spatial track's emitter each frame so a tracked loop rides its unit (the
/// reference's tracked play, `0x61fec0`).
pub(crate) fn set_track_position(track: &mut SpatialTrackHandle, pos: Vec3) {
    track.set_position(
        mint::Vector3 {
            x: pos.x,
            y: pos.y,
            z: pos.z,
        },
        snap(),
    );
}

/// EAX listener properties to Freeverb `(feedback, damping, wet level)`, as
/// [`Mixer::set_reverb`] documents.
fn freeverb_projection(p: &SoundProvider) -> (f64, f64, Decibels) {
    let feedback = if p.decay_time > 0.0 {
        10f64.powf(-0.108 / f64::from(p.decay_time)).min(0.98)
    } else {
        0.0
    };
    let damping = ((1.0 - f64::from(p.decay_hf_ratio) / 2.0).max(0.0) * 0.7
        + f64::from(-p.room_hf) / 10_000.0 * 0.3)
        .clamp(0.0, 1.0);
    let wet_db = ((p.room + p.reverb) as f32 / 100.0).min(6.0);
    let wet = if wet_db <= Decibels::SILENCE.0 {
        Decibels::SILENCE
    } else {
        Decibels(wet_db)
    };
    (feedback, damping, wet)
}

/// Decode audio-file bytes (WAV incl. IMA-ADPCM, MP3) into a ready-to-play SFX sound.
pub(crate) fn sfx_from_bytes(bytes: Vec<u8>) -> Result<StaticSoundData> {
    StaticSoundData::from_cursor(std::io::Cursor::new(bytes)).context("decoding sfx")
}

/// Decode a looping bed whole: kira's streaming decoder reads these 22050 Hz PCM WAVs at twice
/// their length, so a streamed loop falls silent past EOF.
pub(crate) fn loop_from_bytes(bytes: Vec<u8>) -> Result<StaticSoundData> {
    Ok(sfx_from_bytes(bytes)?.loop_region(..))
}

/// Wrap compressed audio bytes for decode-streaming, behind [`PromotingSource`].
pub(crate) fn stream_from_bytes(bytes: Vec<u8>) -> Result<StreamingSoundData<FromFileError>> {
    StreamingSoundData::from_media_source(PromotingSource(std::io::Cursor::new(bytes)))
        .context("opening stream")
}

/// Compressed-audio source whose first read on a thread promotes it to user-interactive QoS:
/// kira's decode thread starts at default QoS, below the world-entry burst, and a starved decoder
/// crackles without tripping any [`MixHealth`] meter.
struct PromotingSource(std::io::Cursor<Vec<u8>>);

/// Once-per-thread promotion latch for [`PromotingSource`].
fn promote_decode_thread() {
    std::thread_local! {
        static PROMOTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    PROMOTED.with(|p| {
        if !p.get() {
            benilla_world::thread_qos::promote_current_thread(
                benilla_world::thread_qos::QosClass::UserInteractive,
            );
            debug!(
                "audio: stream decode thread {:?} promoted",
                std::thread::current().id()
            );
            p.set(true);
        }
    });
}

impl std::io::Read for PromotingSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        promote_decode_thread();
        self.0.read(buf)
    }
}

impl std::io::Seek for PromotingSource {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        promote_decode_thread();
        self.0.seek(pos)
    }
}

impl symphonia::core::io::MediaSource for PromotingSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.0.get_ref().len() as u64)
    }
}

/// Audible time that may go missing per window: over two ~43 ms callback blocks.
const STREAM_STARVED_MIN_SECS: f64 = 0.1;
const STREAM_WATCH_WINDOW_SECS: f64 = 1.0;
/// How long a stream may sit audible and unmoved before it is named as never started.
const STREAM_START_MAX_SECS: f64 = 3.0;

/// Starvation meter for a live stream: a starved stream's position freezes while it stays
/// audible, so each window compares wall time against position advanced.
///
/// Every stream opens frozen until the decoder has two frames ready; that spin-up is not a
/// dropout, so counting starts once the position moves. A stream that starves and stops inside
/// one main-thread stall is never counted; the mix tap covers that.
pub(crate) struct StreamWatch {
    label: &'static str,
    phase: Phase,
    /// Wall time counted into the open window, summed from the fed frame deltas.
    expected: f64,
    advanced: f64,
}

#[derive(Clone, Copy)]
enum Phase {
    /// No stream on the slot, or one that is not audible.
    Idle,
    /// Audible, the position not yet moved off `pos`: the spin-up, not counted.
    Starting {
        since: std::time::Instant,
        pos: f64,
        warned: bool,
    },
    /// Advancing. `window_start` is stamped with the position baseline, so the counted time and
    /// the reported span are one interval.
    Running {
        last_pos: f64,
        window_start: std::time::Instant,
    },
}

/// What [`StreamWatch::observe`] found this frame.
enum Verdict {
    /// A window closed having lost more than [`STREAM_STARVED_MIN_SECS`] of audible time.
    Starved {
        lost: f64,
        counted: f64,
        advanced: f64,
        span: f64,
    },
    /// The stream went audible and its position never moved.
    NeverStarted { waited: f64 },
    /// The stream began advancing this long after it went audible: its spin-up.
    Began { after: f64 },
}

impl StreamWatch {
    pub(crate) fn new(label: &'static str) -> Self {
        Self {
            label,
            phase: Phase::Idle,
            expected: 0.0,
            advanced: 0.0,
        }
    }

    /// Per-frame feed from the held handle, every frame it exists.
    pub(crate) fn feed(&mut self, handle: &StreamingSoundHandle<FromFileError>, dt: f64) {
        use kira::sound::PlaybackState as S;
        let audible = matches!(handle.state(), S::Playing | S::Stopping);
        match self.observe(audible, handle.position(), dt, std::time::Instant::now()) {
            Some(Verdict::Starved {
                lost,
                counted,
                advanced,
                span,
            }) => {
                // No cause is named: a starved decoder, a render thread that did not run and a
                // rebuilding device freeze the position identically.
                warn!(
                    "audio: {} stream starved — ~{:.0} ms of injected silence over a {span:.2} s \
                     window (counted {counted:.2} s, position advanced {advanced:.2} s) — this is \
                     what a crackle sounds like",
                    self.label,
                    lost * 1000.0,
                );
            }
            Some(Verdict::NeverStarted { waited }) => warn!(
                "audio: {} has been audible {waited:.1} s with its position still at the start — \
                 not one sample of it has been heard",
                self.label,
            ),
            // A frame-quantized bound, from when the watch first saw the handle.
            Some(Verdict::Began { after }) => debug!(
                "audio: {} took {:.0} ms to start advancing after the watch first saw it — \
                 spin-up, not counted",
                self.label,
                after * 1000.0,
            ),
            None => {}
        }
    }

    /// Drop the baseline when the watched handle is dropped or replaced.
    pub(crate) fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.expected = 0.0;
        self.advanced = 0.0;
    }

    /// Open a counting window: position baseline and wall origin together.
    fn begin(&mut self, pos: f64, now: std::time::Instant) {
        self.phase = Phase::Running {
            last_pos: pos,
            window_start: now,
        };
        self.expected = 0.0;
        self.advanced = 0.0;
    }

    fn observe(
        &mut self,
        audible: bool,
        pos: f64,
        dt: f64,
        now: std::time::Instant,
    ) -> Option<Verdict> {
        // Stopped or paused: the position legitimately holds.
        if !audible {
            self.reset();
            return None;
        }
        match self.phase {
            Phase::Idle => {
                self.phase = Phase::Starting {
                    since: now,
                    pos,
                    warned: false,
                };
                None
            }
            Phase::Starting {
                since,
                pos: start,
                warned,
            } => {
                let waited = now.duration_since(since).as_secs_f64();
                if pos > start {
                    self.begin(pos, now);
                    return Some(Verdict::Began { after: waited });
                }
                if !warned && waited > STREAM_START_MAX_SECS {
                    self.phase = Phase::Starting {
                        since,
                        pos: start,
                        warned: true,
                    };
                    return Some(Verdict::NeverStarted { waited });
                }
                None
            }
            Phase::Running {
                last_pos,
                window_start,
            } => {
                // A backwards jump is a new stream swapped onto the slot (no watched stream
                // loops), so it re-enters its own spin-up.
                if pos < last_pos {
                    self.phase = Phase::Starting {
                        since: now,
                        pos,
                        warned: false,
                    };
                    self.expected = 0.0;
                    self.advanced = 0.0;
                    return None;
                }
                self.expected += dt;
                self.advanced += pos - last_pos;
                self.phase = Phase::Running {
                    last_pos: pos,
                    window_start,
                };
                if self.expected < STREAM_WATCH_WINDOW_SECS {
                    return None;
                }
                let (counted, advanced) = (self.expected, self.advanced);
                let span = now.duration_since(window_start).as_secs_f64();
                self.begin(pos, now);
                let lost = counted - advanced;
                (lost > STREAM_STARVED_MIN_SECS).then_some(Verdict::Starved {
                    lost,
                    counted,
                    advanced,
                    span,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(decay: f32, hf_ratio: f32, room: i32, room_hf: i32, reverb: i32) -> SoundProvider {
        SoundProvider {
            id: 0,
            name: String::new(),
            flags: 0,
            decay_time: decay,
            room,
            room_hf,
            decay_hf_ratio: hf_ratio,
            reflections: 0,
            reverb,
            env_diffusion: 1.0,
            env_size: 1.0,
        }
    }

    #[test]
    fn freeverb_projection_orders_the_presets() {
        // PRESET_GENERIC / PRESET_CAVE / PRESET_HANGAR: decay 1.49 / 3.0 / 10.05 s.
        let generic = preset(1.49, 0.86, -1000, -100, 200);
        let cave = preset(3.0, 1.3, -1000, -500, -402);
        let hangar = preset(10.05, 0.26, -1000, -1000, 198);
        let (fg, dg, wg) = freeverb_projection(&generic);
        let (fc, _, _) = freeverb_projection(&cave);
        let (fh, _, _) = freeverb_projection(&hangar);
        assert!(fg < fc && fc < fh, "feedback monotone in decay time");
        assert!(fh <= 0.98, "runaway clamp");
        assert!(
            (wg.0 - -8.0).abs() < 1e-3,
            "GENERIC wet = (−1000+200) mB → −8 dB"
        );
        assert!((0.0..=1.0).contains(&dg));

        // Underwater (11): DecayHFRatio 0.1 + RoomHF −10000 → near-full damping; wet capped +6.
        let underwater = preset(1.0, 0.1, -1000, -10000, 1700);
        let (_, du, wu) = freeverb_projection(&underwater);
        assert!(du > 0.9, "underwater kills the highs (got {du})");
        assert!((wu.0 - 6.0).abs() < 1e-3, "wet cap at +6 dB");

        // PRESET_OFF: Room −10000 → below the −60 dB floor → SILENCE.
        let off = preset(1.0, 1.0, -10000, -10000, 200);
        let (_, _, w_off) = freeverb_projection(&off);
        assert_eq!(w_off, Decibels::SILENCE);
    }

    #[test]
    fn real_wav_and_mp3_decode() {
        let data = benilla_formats::wow_data_or_skip!();
        let Ok(chain) = benilla_formats::open_chain(&data) else {
            eprintln!("skipping: no client data at {}", data.display());
            return;
        };

        let wav = chain
            .read("Sound\\interface\\LevelUp.wav")
            .expect("LevelUp.wav in the chain");
        let sfx = sfx_from_bytes(wav).expect("WAV decodes");
        assert!(
            sfx.duration().as_millis() > 500,
            "LevelUp.wav should be a real, non-trivial clip"
        );

        let mp3 = chain
            .read("Sound\\Music\\CityMusic\\Darnassus\\Darnassus Walking 1.mp3")
            .expect("Darnassus mp3 in the chain");
        let stream = stream_from_bytes(mp3).expect("MP3 opens for streaming");
        assert!(
            stream.duration().as_secs() > 30,
            "zone music should be minutes long"
        );
    }

    /// Render N simultaneous copies of one kit through the main-track chain on kira's mock
    /// backend: `(asked, heard peak, samples over)`.
    fn render_overlapping(bytes: &[u8], copies: usize, limiter: bool) -> (f32, f32, u64) {
        use kira::backend::mock::{MockBackend, MockBackendSettings};

        const RATE: u32 = 44_100;
        let asked = Arc::new(MixLevel::default());
        let heard = Arc::new(MixLevel::default());
        let limiter_on = Arc::new(AtomicBool::new(limiter));
        // The client's own chain, plus one probe meter appended to read the limiter's output.
        let mut main = main_track(&asked, &limiter_on, Some(RATE), None).builder;
        meter::install(&mut main, &heard);
        let mut manager = AudioManager::<MockBackend>::new(AudioManagerSettings {
            backend_settings: MockBackendSettings { sample_rate: RATE },
            main_track_builder: main,
            ..Default::default()
        })
        .expect("mock backend");

        let data = sfx_from_bytes(bytes.to_vec()).expect("kit decodes");
        for _ in 0..copies {
            manager.play(data.clone()).expect("play");
        }
        // The whole kit plus a beat: a buff sound swells, so its peak comes late.
        let blocks = (data.duration().as_secs_f64() + 0.25) * f64::from(RATE) / 128.0;
        let backend = manager.backend_mut();
        for _ in 0..(blocks.ceil() as usize) {
            backend.on_start_processing();
            backend.process();
        }
        let heard = heard.take();
        let asked = asked.take().peak;
        eprintln!(
            "mix harness: {copies} copies, limiter {} — asked {asked:.2}x, heard {:.2}x, \
             {} sample(s) past full scale",
            if limiter { "on " } else { "off" },
            heard.peak,
            heard.over,
        );
        (asked, heard.peak, heard.over)
    }

    /// A quarter-second 0 dBFS stereo float sine, full scale like every WoW SFX, built in memory.
    fn full_scale_tone(rate: u32) -> Vec<u8> {
        let frames = rate as usize / 4;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + frames as u32 * 8).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&3u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 8).to_le_bytes());
        wav.extend_from_slice(&8u16.to_le_bytes());
        wav.extend_from_slice(&32u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(frames as u32 * 8).to_le_bytes());
        for i in 0..frames {
            let v = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin();
            wav.extend_from_slice(&v.to_le_bytes());
            wav.extend_from_slice(&v.to_le_bytes());
        }
        wav
    }

    /// An unattended run's window is unfocused, so a gate above the meters would make every
    /// capture record silence.
    #[test]
    fn the_output_gate_silences_the_output_and_nothing_upstream() {
        use kira::backend::mock::{MockBackend, MockBackendSettings};

        const RATE: u32 = 44_100;

        let asked = Arc::new(MixLevel::default());
        let heard = Arc::new(MixLevel::default());
        let limiter_on = Arc::new(AtomicBool::new(true));
        let chain = main_track(&asked, &limiter_on, Some(RATE), None);
        let (mut main, mut output) = (chain.builder, chain.output);
        // Appended after the gate, so it reads what reaches the device.
        meter::install(&mut main, &heard);
        let mut manager = AudioManager::<MockBackend>::new(AudioManagerSettings {
            backend_settings: MockBackendSettings { sample_rate: RATE },
            main_track_builder: main,
            ..Default::default()
        })
        .expect("mock backend");

        let data = sfx_from_bytes(full_scale_tone(RATE)).expect("tone decodes");
        let render = |manager: &mut AudioManager<MockBackend>| {
            manager.play(data.clone()).expect("play");
            let blocks = (data.duration().as_secs_f64() + 0.05) * f64::from(RATE) / 128.0;
            let backend = manager.backend_mut();
            for _ in 0..(blocks.ceil() as usize) {
                backend.on_start_processing();
                backend.process();
            }
            (asked.take().peak, heard.take().peak)
        };

        let (open_asked, open_heard) = render(&mut manager);
        assert!(
            open_asked > 0.9 && open_heard > 0.9,
            "gate open: asked {open_asked:.3}, heard {open_heard:.3} — both should be full scale"
        );

        output.set_volume(Decibels::SILENCE, glide());
        {
            let backend = manager.backend_mut();
            for _ in 0..((f64::from(RATE) * 0.05 / 128.0).ceil() as usize) {
                backend.on_start_processing();
                backend.process();
            }
        }
        let _ = (asked.take(), heard.take());

        let (shut_asked, shut_heard) = render(&mut manager);
        assert!(
            shut_heard < 1e-3,
            "gate shut: the output must be silent, heard {shut_heard:.6}"
        );
        assert!(
            shut_asked > 0.9,
            "gate shut: the METER plane must still read the mix the game produced — asked \
             {shut_asked:.3}. A tap that goes silent with the speakers is the false negative \
             this ordering exists to prevent"
        );
    }

    /// Two 0 dBFS copies at master 0.25 sum to 0.5 after the master, so a limiter behind it
    /// never engages; one ahead of it would see 2.0 and duck.
    #[test]
    fn the_master_volume_sits_upstream_of_the_limiter() {
        use kira::backend::mock::{MockBackend, MockBackendSettings};

        const RATE: u32 = 44_100;
        const MASTER: f32 = 0.25;
        const COPIES: usize = 2;

        let asked = Arc::new(MixLevel::default());
        let heard = Arc::new(MixLevel::default());
        let limiter_on = Arc::new(AtomicBool::new(true));
        let chain = main_track(&asked, &limiter_on, Some(RATE), None);
        let (mut main, mut master) = (chain.builder, chain.master);
        // Appended last, so it reads the chain's actual output.
        meter::install(&mut main, &heard);
        let mut manager = AudioManager::<MockBackend>::new(AudioManagerSettings {
            backend_settings: MockBackendSettings { sample_rate: RATE },
            main_track_builder: main,
            ..Default::default()
        })
        .expect("mock backend");
        master.set_volume(amp_to_db(MASTER), snap());
        // Even a zero-duration tween interpolates across one chunk; settle it over silence.
        {
            let backend = manager.backend_mut();
            for _ in 0..4 {
                backend.on_start_processing();
                backend.process();
            }
        }
        asked.take();
        heard.take();

        let data = sfx_from_bytes(full_scale_tone(RATE)).expect("tone decodes");
        for _ in 0..COPIES {
            manager.play(data.clone()).expect("play");
        }
        let blocks = (data.duration().as_secs_f64() + 0.25) * f64::from(RATE) / 128.0;
        let backend = manager.backend_mut();
        for _ in 0..(blocks.ceil() as usize) {
            backend.on_start_processing();
            backend.process();
        }
        let heard = heard.take();
        // The limiter reports its gain into the level cell it was installed with.
        let inner = asked.take();

        eprintln!(
            "master ordering: {COPIES} copies at master {MASTER} — after master {:.3}x, \
             heard {:.3}x, limiter deepest gain {:.3}",
            inner.peak, heard.peak, inner.reduction,
        );
        assert!(
            inner.reduction > 0.99,
            "the limiter engaged (deepest gain {:.3}) on a mix that never exceeded full scale — \
             the master is downstream of it again",
            inner.reduction,
        );
        assert!(
            (heard.peak - MASTER * COPIES as f32).abs() < 0.05,
            "expected the plain scaled sum ~{:.2}, heard {:.3}",
            MASTER * COPIES as f32,
            heard.peak,
        );
        assert_eq!(heard.over, 0, "nothing should have passed full scale");
    }

    /// An over-scale mix reads far past full scale in `pre.wav` and under it in `post.wav`. The
    /// capture stays in the temp dir as a known answer for `scripts/soundprobe.py`.
    #[test]
    fn the_probe_taps_bracket_the_limiter() {
        use kira::backend::mock::{MockBackend, MockBackendSettings};

        const RATE: u32 = 44_100;
        const COPIES: usize = 5;

        let dir = std::env::temp_dir().join("benilla-probe-selftest");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp capture dir");

        let wav = full_scale_tone(RATE);

        {
            let level = Arc::new(MixLevel::default());
            let limiter_on = Arc::new(AtomicBool::new(true));
            let chain = main_track(&level, &limiter_on, Some(RATE), Some(&dir));
            let (main, audio_pos) = (chain.builder, chain.audio_pos);
            let audio_pos = audio_pos.expect("the pre-tap publishes a frame clock");
            let mut manager = AudioManager::<MockBackend>::new(AudioManagerSettings {
                backend_settings: MockBackendSettings { sample_rate: RATE },
                main_track_builder: main,
                ..Default::default()
            })
            .expect("mock backend");

            let data = sfx_from_bytes(wav).expect("tone decodes");
            for _ in 0..COPIES {
                manager.play(data.clone()).expect("play");
            }
            let blocks = (data.duration().as_secs_f64() + 0.1) * f64::from(RATE) / 128.0;
            let backend = manager.backend_mut();
            for _ in 0..(blocks.ceil() as usize) {
                backend.on_start_processing();
                backend.process();
            }
            assert!(
                audio_pos.load(Ordering::Relaxed) > 0,
                "the shared clock must advance — a capture with a dead clock cannot place a mark"
            );
        } // manager dropped: the tap producers abandon, the writers do their final flush.

        // The writers wake on a 250 ms cadence; give them room to drain and exit.
        std::thread::sleep(std::time::Duration::from_millis(900));

        let peak = |name: &str| -> (f32, u64) {
            let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(bytes.len() > 44, "{name} has no audio");
            let (mut peak, mut over) = (0.0f32, 0u64);
            for c in bytes[44..].as_chunks::<4>().0 {
                let v = f32::from_le_bytes(*c).abs();
                peak = peak.max(v);
                over += u64::from(v > 1.0);
            }
            (peak, over)
        };
        let (pre_peak, pre_over) = peak("pre.wav");
        let (post_peak, post_over) = peak("post.wav");
        eprintln!(
            "probe self-test: pre {pre_peak:.2}x ({pre_over} over) -> post {post_peak:.2}x \
             ({post_over} over); capture left at {}",
            dir.display()
        );

        assert!(
            pre_peak > 3.0,
            "pre.wav must record what the game ASKED for — {COPIES} full-scale copies, got \
             {pre_peak:.2}x. A pre tap that already shows a limited signal is measuring the \
             wrong side of the chain."
        );
        assert!(pre_over > 0, "and it must show the over-scale samples");
        assert!(
            post_peak <= 1.0 && post_over == 0,
            "post.wav must record what was HEARD — held under full scale, got {post_peak:.2}x \
             with {post_over} over"
        );
    }

    /// The arena refuses past its capacity; pins kira's default of 128 and ours, so an upgrade
    /// cannot move the voice ceiling silently.
    #[test]
    fn the_spatial_voice_arena_refuses_past_its_capacity() {
        use kira::backend::mock::{MockBackend, MockBackendSettings};

        fn fill(capacity: usize) -> usize {
            let mut manager = AudioManager::<MockBackend>::new(AudioManagerSettings {
                backend_settings: MockBackendSettings {
                    sample_rate: 44_100,
                },
                capacities: kira::Capacities {
                    sub_track_capacity: capacity,
                    ..Default::default()
                },
                ..Default::default()
            })
            .expect("mock backend");
            let listener = manager
                .add_listener(
                    mint::Vector3 {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    },
                    mint::Quaternion {
                        v: mint::Vector3 {
                            x: 0.0,
                            y: 0.0,
                            z: 0.0,
                        },
                        s: 1.0,
                    },
                )
                .expect("listener");
            let mut held = Vec::new();
            for n in 0.. {
                let track = manager.add_spatial_sub_track(
                    listener.id(),
                    mint::Vector3 {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    },
                    SpatialTrackBuilder::new().sound_capacity(1),
                );
                match track {
                    Ok(t) => held.push(t),
                    Err(_) => return n,
                }
            }
            unreachable!()
        }

        assert_eq!(
            fill(kira::Capacities::default().sub_track_capacity),
            128,
            "kira's default voice ceiling moved — re-read the SPATIAL_VOICE_CAPACITY reasoning"
        );
        assert_eq!(fill(SPATIAL_VOICE_CAPACITY), SPATIAL_VOICE_CAPACITY);
    }

    /// `HolyProtection.wav`, the Fortitude buff, peaks at 1.000 and its kit (`SoundEntries` 3116)
    /// has `Flags 0x0000`, so a five-target cast starts five sample-aligned copies. Bypassed, the
    /// ~5x sum reaches the renderer's clamp; armed, nothing passes the ceiling.
    #[test]
    fn overlapping_kits_clip_without_the_limiter_and_do_not_with_it() {
        let data = benilla_formats::wow_data_or_skip!();
        let Ok(chain) = benilla_formats::open_chain(&data) else {
            eprintln!("skipping: no client data at {}", data.display());
            return;
        };
        let bytes = chain
            .read("Sound\\Spells\\HolyProtection.wav")
            .expect("HolyProtection.wav in the chain");

        // One copy already sits on full scale.
        let (asked, heard, over) = render_overlapping(&bytes, 1, false);
        assert!(
            asked > 0.99,
            "a single kit should already reach full scale, got {asked}"
        );
        assert!(over <= 2, "one copy should not meaningfully clip ({over})");
        assert!(heard <= 1.001);

        let (asked, heard, over) = render_overlapping(&bytes, 5, false);
        assert!(asked > 4.0, "five copies should sum to ~5x, got {asked}");
        assert!(
            heard > 4.0 && over > 1_000,
            "bypassed, the over-scale mix must reach the renderer's clamp \
             (peak {heard}, {over} samples over)"
        );

        let (asked, heard, over) = render_overlapping(&bytes, 5, true);
        assert!(asked > 4.0, "the game still asks for ~5x, got {asked}");
        assert_eq!(over, 0, "the limiter let {over} samples past full scale");
        assert!(
            heard <= limiter::ceiling() + 1e-5,
            "output peaked at {heard}"
        );
    }

    fn wall(t0: std::time::Instant, secs: f64) -> std::time::Instant {
        t0 + std::time::Duration::from_secs_f64(secs)
    }

    #[test]
    fn stream_watch_accounts_freezes_not_swaps() {
        let dt = 1.0 / 60.0;
        let t0 = std::time::Instant::now();

        let mut w = StreamWatch::new("test");
        let mut pos = 0.0;
        for i in 0..90 {
            pos += dt;
            assert!(
                !matches!(
                    w.observe(true, pos, dt, wall(t0, f64::from(i) * dt)),
                    Some(Verdict::Starved { .. })
                ),
                "healthy stream reported"
            );
        }

        // Starved: 18 frames (~0.3 s) frozen inside the window.
        let mut w = StreamWatch::new("test");
        let mut pos = 0.0;
        let mut reports = Vec::new();
        for i in 0..90 {
            if !(30..48).contains(&i) {
                pos += dt;
            }
            if let Some(Verdict::Starved {
                lost,
                counted,
                span,
                ..
            }) = w.observe(true, pos, dt, wall(t0, f64::from(i) * dt))
            {
                reports.push((lost, counted, span));
            }
        }
        assert_eq!(reports.len(), 1, "one starved window: {reports:?}");
        assert!(
            (reports[0].0 - 0.3).abs() < 0.05,
            "lost ≈ 0.3 s, got {reports:?}"
        );

        // Slot swap: the position jumps back to 0 once.
        let mut w = StreamWatch::new("test");
        let mut pos = 40.0;
        for i in 0..120 {
            if i == 30 {
                pos = 0.0;
            }
            assert!(
                !matches!(
                    w.observe(true, pos, dt, wall(t0, f64::from(i) * dt)),
                    Some(Verdict::Starved { .. })
                ),
                "slot swap reported as freeze"
            );
            pos += dt;
        }

        // Stopped, then restarted far behind the old position.
        let mut w = StreamWatch::new("test");
        let mut pos = 70.0;
        for i in 0..30 {
            assert!(w
                .observe(true, pos, dt, wall(t0, f64::from(i) * dt))
                .is_none_or(|v| matches!(v, Verdict::Began { .. })));
            pos += dt;
        }
        assert!(w.observe(false, pos, dt, wall(t0, 0.5)).is_none());
        let mut pos = 0.0;
        for i in 30..120 {
            assert!(
                !matches!(
                    w.observe(true, pos, dt, wall(t0, f64::from(i) * dt)),
                    Some(Verdict::Starved { .. })
                ),
                "restart read as freeze"
            );
            pos += dt;
        }
    }

    /// A 298 ms hitch at the head of a window, the stream frozen through it, is both counted and
    /// spanned.
    #[test]
    fn stream_watch_spans_the_window_it_counted() {
        let dt = 1.0 / 60.0;
        let t0 = std::time::Instant::now();
        let hitch = 0.298;
        let mut w = StreamWatch::new("test");

        assert!(w.observe(true, 0.0, dt, wall(t0, 0.0)).is_none());
        // The first sample lands and the window opens.
        assert!(matches!(
            w.observe(true, 0.001, dt, wall(t0, 0.010)),
            Some(Verdict::Began { .. })
        ));
        // The hitch: its delta reaches back to the instant the window opened.
        assert!(w
            .observe(true, 0.001, hitch, wall(t0, 0.010 + hitch))
            .is_none());

        let mut pos = 0.001;
        let mut report = None;
        for i in 0..90 {
            pos += dt;
            let t = 0.010 + hitch + f64::from(i + 1) * dt;
            if let Some(v) = w.observe(true, pos, dt, wall(t0, t)) {
                report = Some(v);
                break;
            }
        }
        let Some(Verdict::Starved {
            lost,
            counted,
            span,
            ..
        }) = report
        else {
            panic!("the frozen hitch went unreported");
        };
        assert!(
            (counted - span).abs() < 1e-6,
            "the window must span the interval it counted: counted {counted}, span {span}"
        );
        assert!(
            (lost - hitch).abs() < 0.01,
            "the hitch is the whole loss: {lost}"
        );
    }

    /// A login's glue theme: played in a frame that then blocks 298 ms, first sample 233 ms in
    /// (mix-ahead, IO buffer, decode). The spin-up is reported once and never charged.
    #[test]
    fn stream_watch_does_not_charge_a_stream_for_starting() {
        let dt = 1.0 / 60.0;
        let t0 = std::time::Instant::now();
        let hitch = 0.298;
        let first_sample = 0.233;
        // Position as the handle reports it: 0 until the first sample, then wall time.
        let pos_at = |t: f64| (t - first_sample).max(0.0);

        let mut w = StreamWatch::new("test");
        let mut began = Vec::new();
        let mut starved = 0;
        // The frame the theme started on, the hitch, then three seconds of ordinary frames.
        let mut ticks = vec![0.0, hitch];
        for i in 1..=180 {
            ticks.push(hitch + f64::from(i) * dt);
        }
        for (i, t) in ticks.iter().enumerate() {
            let delta = if i == 0 { dt } else { t - ticks[i - 1] };
            match w.observe(true, pos_at(*t), delta, wall(t0, *t)) {
                Some(Verdict::Began { after }) => began.push(after),
                Some(Verdict::Starved {
                    lost,
                    counted,
                    span,
                    ..
                }) => {
                    starved += 1;
                    eprintln!("starved: lost {lost} counted {counted} span {span}");
                }
                Some(Verdict::NeverStarted { .. }) => {
                    panic!("a playing stream read as never started")
                }
                None => {}
            }
        }
        assert_eq!(
            starved, 0,
            "the login's own spin-up was charged as injected silence again"
        );
        assert_eq!(began.len(), 1, "one spin-up line per stream: {began:?}");
        // Frame-quantized: the first advance shows on the frame after the hitch.
        assert!(
            (began[0] - hitch).abs() < 1e-6,
            "the skipped spin-up, stated as the bound it is: {began:?}"
        );
    }

    #[test]
    fn stream_watch_names_a_stream_that_never_starts() {
        let dt = 1.0 / 60.0;
        let t0 = std::time::Instant::now();
        let mut w = StreamWatch::new("test");
        let mut waited = Vec::new();
        for i in 0..600 {
            if let Some(Verdict::NeverStarted { waited: s }) =
                w.observe(true, 0.0, dt, wall(t0, f64::from(i) * dt))
            {
                waited.push(s);
            }
        }
        assert_eq!(waited.len(), 1, "one line per stream, not one per frame");
        assert!(
            waited[0] > STREAM_START_MAX_SECS && waited[0] < STREAM_START_MAX_SECS + 0.1,
            "named as soon as the wait is past the bound: {waited:?}"
        );
    }

    /// `stop(tween)` then dropping the handle still fades: a full-amplitude loop ramps through
    /// the middle, neither cut to 0 nor left at full.
    #[test]
    fn stop_fade_ramps_after_handle_drop() {
        use kira::backend::{Backend, Renderer};
        use kira::sound::static_sound::StaticSoundData;
        use kira::{AudioManager, AudioManagerSettings, Frame, Tween};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        // A backend that just holds the renderer and lets the test pump audio out of it.
        struct Capture(Arc<Mutex<Option<Renderer>>>);
        impl Backend for Capture {
            type Settings = ();
            type Error = ();
            fn setup(_: (), _buf: usize) -> Result<(Self, u32), ()> {
                Ok((Capture(Arc::new(Mutex::new(None))), 100)) // 100 Hz → one frame = 10 ms
            }
            fn start(&mut self, renderer: Renderer) -> Result<(), ()> {
                *self.0.lock().unwrap() = Some(renderer);
                Ok(())
            }
        }

        let mut manager =
            AudioManager::<Capture>::new(AudioManagerSettings::default()).expect("manager");
        let slot = manager.backend_mut().0.clone();
        // Render `n` frames; return the mean absolute left-channel amplitude of the block.
        let render = |n: usize| -> f32 {
            let mut guard = slot.lock().unwrap();
            let r = guard.as_mut().expect("renderer started");
            r.on_start_processing();
            let mut out = vec![0.0f32; n * 2];
            r.process(&mut out, 2);
            out.iter().step_by(2).map(|s| s.abs()).sum::<f32>() / n as f32
        };

        // Looping, so only the fade can silence it.
        let frames: Arc<[Frame]> = (0..100).map(|_| Frame::from_mono(1.0)).collect();
        let bed = StaticSoundData {
            sample_rate: 100,
            frames,
            settings: Default::default(),
            slice: None,
        }
        .loop_region(..);

        let mut h = manager.play(bed).expect("play");
        assert!(render(5) > 0.9, "bed plays at full before the fade");

        // Fade over 1 s (100 frames) and drop the handle at once.
        h.stop(Tween {
            duration: Duration::from_secs(1),
            ..Default::default()
        });
        drop(h);

        // 50 ms blocks, 1.3 s over the 1 s fade.
        let series: Vec<f32> = (0..26).map(|_| render(5)).collect();
        let midband = series.iter().filter(|&&a| (0.05..0.9).contains(&a)).count();
        assert!(
            *series.last().unwrap() < 0.05,
            "silent once the fade completes — the stop survived the handle drop ({series:?})"
        );
        assert!(
            midband >= 3,
            "the fade is gradual, not an instant cut — needs blocks mid-ramp ({series:?})"
        );
    }
}
