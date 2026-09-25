//! The audio-backend seam: kira 0.12 behind a narrow, FMOD-shaped surface.
//!
//! The real client delegates the entire audible mix to FMOD 3.x — spatialization/pan, min/max
//! distance rolloff, doppler, reverb, decode, streaming — and owns only the parameters it feeds
//! (pinned by the import-table byte-fact: no `FSOUND_SetPan`, no
//! `FSOUND_PlaySound`). This module is that delegation seam in benilla: everything above it (the
//! kit player, the schedulers) computes WoW's owned parameter math; everything below is the
//! backend's. Keep this surface shaped like the FMOD import contract — play/stop, per-channel
//! volume/pitch, listener + source positions, streamed music — so the backend stays swappable
//! (bevy_seedling is the revisit candidate, decision 0070 "Why").
//!
//! Coordinate convention: kira's listener is X-right/Y-up (its ears sit at ±X of the orientation
//! quat — kira `track/sub.rs::listener_ear_positions`), which is exactly Bevy camera space, so
//! camera `Transform`s feed straight in with **no remap**. The RE'd `(x,y,z)→(−y,z,x)` transform
//! is the WoW↔FMOD convention pair; ours is Bevy↔kira, already unified by the decision-0002
//! world transform upstream.

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

// Types crossing the seam (consumers hold handles to stop/steer a playing channel; dropping a
// *sound* handle does not stop the sound, and dropping a spatial *track* handle marks its track
// for removal — which lands once the track's sounds finish, see `play_3d`'s
// `persist_until_sounds_finish`, so a fade-then-drop still gets to fade).
pub(crate) use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
pub(crate) use kira::sound::streaming::StreamingSoundHandle;
pub(crate) use kira::track::SpatialTrackHandle;

/// An immediate (zero-duration) parameter change. kira requires a tween on every setter; the
/// WoW-side ramps (volume rates, crossfades) are our own math updated per frame, so the backend
/// must not add smoothing of its own on top. Use this for a change that is genuinely a *step* —
/// a reverb preset switch (instant, `0x45a720`), an initial value. For the
/// **per-frame volume feed** use [`glide`]: a step there is an audible click, not fidelity.
pub(crate) fn snap() -> Tween {
    Tween {
        duration: std::time::Duration::ZERO,
        ..Default::default()
    }
}

/// The per-frame **volume** feed's reconstruction ramp.
///
/// kira applies a track's volume as **one constant gain per internal chunk** — it updates the
/// parameter once per `internal_buffer_size` (128 frames ≈ 2.9 ms) block and multiplies the whole
/// block by it (`track/sub.rs::process`, `backend/renderer.rs`'s `chunks_mut`). Spatial *position*
/// is interpolated across the chunk (`time_in_chunk`); volume is not. So a [`snap`] on the
/// per-frame gain feed is a hard step in the waveform — a click whose loudness scales with the
/// size of the jump.
///
/// At a steady 60 fps the per-frame jumps are small and the clicks stay under the noise floor,
/// which is why this was invisible for so long. The moment frame pacing goes unstable — a
/// background build, an OBS encode, a macOS Space switch (decision 0609's world) — the jumps get
/// big and every live channel steps at once: the reported "crack fest". The frame rate was never
/// supposed to be audible.
///
/// So the feed glides instead of stepping: each frame starts a fresh linear ramp toward the value
/// we just computed. This is **not** backend smoothing layered on WoW's ramps — it is
/// reconstruction of a signal we only sample at frame rate. WoW's owned envelope math upstream is
/// untouched; a ramp shorter than one frame at 60 fps cannot smear a multi-second crossfade, and
/// on a long frame it simply completes early and holds.
pub(crate) fn glide() -> Tween {
    Tween {
        duration: std::time::Duration::from_millis(GLIDE_MS),
        ..Default::default()
    }
}

/// [`glide`]'s ramp, and the de-click fade on a force-stop. Just under one 60 fps frame (16.7 ms):
/// long enough to bridge kira's 2.9 ms gain blocks, short enough that a stop still reads as
/// immediate and a live parameter never audibly lags its source.
const GLIDE_MS: u64 = 15;

/// The de-click fade for a **force-stop** — a `stop()` that cuts a channel which may still be at
/// full amplitude (a tracked loop reaped on despawn, the leave-world blanket stop). Ending a
/// non-zero waveform at an arbitrary sample is a step to zero: the same click [`glide`] fixes on
/// the gain feed. One `GLIDE_MS` ramp removes it without moving the stop in time. The fade
/// survives dropping the handle — `stop_fade_ramps_after_handle_drop` pins exactly that.
pub(crate) fn declick() -> Tween {
    glide()
}

/// A linear fade over `ms` — kira's default easing, matching the client's constant per-tick volume
/// decrement (`0x7a5a50`). Used to fade a stream out before it stops, or a bed out before it swaps
/// (the outgoing side of a music transition / ambience swap).
pub(crate) fn fade(ms: u64) -> Tween {
    Tween {
        duration: std::time::Duration::from_millis(ms),
        ..Default::default()
    }
}

/// Linear amplitude `[0,1]` → the backend's decibel volume. WoW's owned pipeline produces linear
/// amplitudes (the category mix `cat·v·atten`, `0x7a5dc0`); FMOD consumed them as 0..255
/// levels. kira consumes dB, so the seam converts: `20·log10(amp)`, with kira's `SILENCE` floor
/// (−60 dB) for amp ≤ 10⁻³ (which is below one 1/255 FMOD step anyway).
pub(crate) fn amp_to_db(amp: f32) -> Decibels {
    if amp <= 1e-3 {
        Decibels::SILENCE
    } else {
        Decibels(20.0 * amp.log10())
    }
}

/// The one open audio device + its listener. `Mixer` methods are the only place kira's manager is
/// touched; everything above computes parameters. There is deliberately **no master filter**:
/// the real client applies no DSP beyond FMOD's reverb (53 FSOUND imports, zero `FSOUND_FX_*` in
/// the import table); underwater is the reverb preset + the ambience swap, both
/// upstream of this seam.
pub(crate) struct Mixer {
    manager: AudioManager<OutputBackend>,
    listener: ListenerHandle,
    /// Rolling mix-health counters — the crackle instrument ([`Mixer::poll_health`]).
    health: MixHealth,
    /// The output meters accumulated since the last [`Mixer::take_output_window`].
    window: Window,
    /// The zone-reverb send track (wet-only Freeverb). Every 3D world track routes into it at
    /// build; this handle's volume is the zone wet level (SILENCE = reverb off).
    reverb_send: SendTrackHandle,
    /// The Freeverb parameters on the send — retuned per zone preset.
    reverb: ReverbHandle,
    /// The main track's level story — what the mix asked for, and what the limiter allowed.
    /// Written on the audio thread, drained by [`Mixer::take_level`].
    level: Arc<MixLevel>,
    /// The `SoundOutputLimiter` CVar cell the limiter reads each block.
    limiter_on: Arc<AtomicBool>,
    /// The master gain — first in the main chain, **upstream** of the limiter ([`main_track`]).
    master: VolumeControlHandle,
    /// The output gate — **last** in the main chain, downstream of every tap ([`main_track`]).
    /// Open (unity) or shut (silence); never a level. [`Mixer::set_output_gate`] is its only
    /// writer.
    output: VolumeControlHandle,
    /// The pre-limiter tap's frame clock while a probing run records; `None` otherwise.
    audio_pos: Option<Arc<AtomicU64>>,
    /// The device's negotiated sample rate — what turns a count of over-scale samples into a
    /// duration in the health report. `None` on a device we could not probe.
    sample_rate: Option<u32>,
}

impl Mixer {
    /// Open the default audio device. Fails cleanly when there is none (headless/CI) — the caller
    /// runs silent with `None` (mirrors the client's `-nosound` gate).
    pub(crate) fn new(probe_dir: Option<&Path>) -> Result<Self> {
        // The device's rate is needed before the manager exists: the mix tap's WAV header and
        // the limiter's delay line are sized at build time, and a kira main track is
        // build-time-only.
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
        // The zone-reverb send: wet-only (the dry path stays on the source tracks), silent until
        // a zone preset raises it. Effects are build-time-only; parameters retune at runtime.
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

    /// The probing run's shared time axis — the pre-limiter tap's frame clock
    /// ([`super::probe`]). `None` when not probing.
    pub(super) fn audio_pos(&self) -> Option<Arc<AtomicU64>> {
        self.audio_pos.clone()
    }

    /// Apply a zone reverb preset — the seam mirror of `FSOUND_Reverb_SetProperties` (the real
    /// client marshals the `SoundProviderPreferences` row into EAX listener properties and hands
    /// them to FMOD; `0x45a790`/`0x7a5fa0`). `None` = no preset → wet to silence (the
    /// client's silenced-GENERIC default, `0x45a830`). The switch is **instant** — the
    /// client applies the new properties with no ramp (`0x45a720`).
    ///
    /// The EAX→Freeverb projection is this backend's own lossy mapping:
    /// - `feedback = 10^(−0.108 / DecayTime)`, from Freeverb's RT60 relation at its ~36 ms mean
    ///   comb delay (`t60 ≈ 3·d̄ / −log10(f)`), clamped to 0.98 so a 20 s hangar can't run away;
    /// - `damping` blends "highs die faster than lows" (`1 − DecayHFRatio/2`, weight 0.7) with
    ///   the wet HF level cut (`−RoomHF/10000`, weight 0.3);
    /// - wet level = `(Room + Reverb)` mB → dB, capped at +6 (Underwater's +700 mB sum);
    ///   ≤ −60 dB (e.g. PRESET_OFF's Room −10000) lands on kira's SILENCE floor.
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

    /// Per-frame listener pose from the world camera (Bevy space, no remap — module docs).
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

    /// Master volume as linear amplitude (the whole mix — every track routes to main). Fed every
    /// frame, so it [`glide`]s: a step clicks the entire mix at once, and this is the knob a
    /// slider drag and the mute toggle both move.
    ///
    /// Drives the **first effect** in the main chain, not the main track's volume — see
    /// [`main_track`] for why the difference is audible. Same `Parameter`, same interpolation;
    /// only the position in the chain changes.
    pub(crate) fn set_master(&mut self, amp: f32) {
        self.master.set_volume(amp_to_db(amp), glide());
    }

    /// Open or shut the **output gate** — the focus mute, the seam mirror of the
    /// reference's `FSOUND_SetMute(FSOUND_ALL, …)` on its window-activation event.
    ///
    /// A *gate*, not a level: unity or silence, nothing between. That is what makes it safe at
    /// the end of the chain — the limiter upstream sees the same signal either way, so shutting
    /// it can never make the limiter duck differently, which is the trap that keeps the master
    /// gain at the *front* ([`main_track`]).
    ///
    /// Shut is a [`glide`] rather than a step for the usual reason: cutting a full mix in one
    /// sample is a click. 16 ms is inaudible as a fade and the reference's own mute is instant,
    /// so nothing about the ramp is a fidelity claim.
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

    /// Arm or bypass the output limiter — the `SoundOutputLimiter` CVar. The
    /// limiter's delay line runs either way, so this is a fade, never a step in the output.
    pub(super) fn set_limiter(&mut self, on: bool) {
        self.limiter_on.store(on, Ordering::Relaxed);
    }

    /// Read and reset the mix's level for this report window.
    pub(super) fn take_level(&mut self) -> LevelReading {
        self.level.take()
    }

    /// The device's negotiated sample rate, when we could probe it — the report's time axis.
    pub(super) fn sample_rate(&self) -> Option<u32> {
        self.sample_rate
    }

    /// Play a decoded (short SFX) sound on the main track — the 2D/UI path.
    pub(crate) fn play_2d(&mut self, data: StaticSoundData) -> Result<StaticSoundHandle> {
        self.manager
            .play(data)
            .map_err(|e| anyhow::anyhow!("play 2d: {e}"))
    }

    /// Play a decoded sound on a fresh spatial track at a world position. Backend attenuation is
    /// **disabled** — gain over distance is WoW's own math (the kit player's pump
    /// computes `rolloff · near_field` and drives the channel volume). The backend contributes
    /// pan only. The returned track handle must live as long as the sound (drop unloads it).
    ///
    /// `reverb_send` is the kit's `SoundEntries.EAXDef` reduced to "does this sound take the wet
    /// send at all". 3D-open is **necessary but not sufficient** in the reference:
    /// `0x458f1c` hands `EAXDef` to the slot lookup `0x45cdc0`, and because
    /// `SoundSamplePreferences.dbc` holds only ids 1 and 2 — **there is no id 0** — an `EAXDef 0`
    /// kit resolves a NULL slot and `FSOUND_Reverb_SetChannelProperties` (`0x7a5bf0`) skips before
    /// it ever tests the 3D flag. Those channels are dry no matter what the zone preset says.
    /// benilla models neither row's per-channel EAX properties, so the projection is the binary's
    /// own branch and nothing more: populated slot ⇒ send at unity, NULL slot ⇒ no send.
    pub(crate) fn play_3d(
        &mut self,
        data: StaticSoundData,
        pos: Vec3,
        reverb_send: bool,
    ) -> Result<(SpatialTrackHandle, StaticSoundHandle)> {
        // The zone wet level lives on the send's own volume, so per-zone reverb stays one knob
        // and never a per-track update; the per-KIT half is this route existing or not.
        // One track, one sound, always — this is a fresh track per play. kira's default
        // `sound_capacity` is 128, and it is not free: the builder allocates two rtrb rings and an
        // arena *sized for 128 sounds* on every play, on the game thread, to hold exactly one
        // (`backend/resources.rs::ResourceStorage::new`). Naming the real number turns roughly ten
        // kilobytes of per-sound churn into a few dozen bytes.
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
                // Outlive the handle by exactly as long as the sound needs.
                // kira defaults this to `false` (`track/sub/spatial_builder.rs`), which makes
                // `should_be_removed()` fire the instant the handle drops — so a channel that
                // is stopped-with-a-fade and then dropped in the same breath (every reaper
                // here: cutoff cull, despawn, leave-world) loses its track mid-ramp and gets
                // cut anyway. `declick()` would have been a no-op on all 3D audio. With this
                // set, the drop only *marks* the track and kira keeps it until its sounds
                // finish. Cannot leak: every path that drops a channel stops its sound first,
                // and a stopped sound finishes.
                .persist_until_sounds_finish(true),
        ) {
            Ok(t) => t,
            Err(e) => {
                // The voice ceiling, counted rather than only logged at the call site: a burst
                // that runs the arena dry drops sounds, and a dropped sound leaves no other trace.
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

    /// Decode-stream a long sound (music/ambience MP3s) on the main track. The data wraps the
    /// *compressed* bytes (a few MB off the MPQ chain) — kira decodes incrementally on its own
    /// thread; nothing is pre-decoded to PCM.
    pub(crate) fn play_stream(
        &mut self,
        data: StreamingSoundData<FromFileError>,
    ) -> Result<StreamingSoundHandle<FromFileError>> {
        self.manager
            .play(data)
            .map_err(|e| anyhow::anyhow!("play stream: {e}"))
    }
}

/// How many **3D voices** can be alive at once — the spatial sub-track arena.
///
/// Every positional one-shot gets its own spatial sub-track ([`Mixer::play_3d`]), so this is the
/// hard ceiling on simultaneous 3D sound, and past it `add_spatial_sub_track` returns
/// `ResourceLimitReached` and the sound is simply not heard.
///
/// kira's default is **128** (`manager/settings.rs`), which is a number we never chose and which
/// a real fight reaches: `sound::combat` fires up to four kits per swing per attacker, and a kit
/// like `HolyProtection` runs 3.4 s, so a pack pull holds many dozens of voices at a time. A
/// ceiling that drops sounds is a *policy*, and the policy about which sounds may play belongs to
/// the game — the reference's own per-bus caps (`0x87ce60`) — not to a backend arena size. So the
/// arena is sized generously and explicitly, and [`MixHealth::voices_refused`] counts any refusal
/// so that hitting even this is visible rather than silent.
///
/// The cost is preallocation, not per-frame work: an idle slot is a few hundred bytes and no DSP.
const SPATIAL_VOICE_CAPACITY: usize = 512;

/// Build the **main track's effect chain**, in signal order:
///
/// 1. **meter** — measures the summed mix as the game asked for it, over-scale and all. Upstream
///    of the limiter on purpose: the number worth reporting is what the mix *wanted*, because
///    that is the one that says whether the game is asking for something impossible.
/// 2. **limiter** — the brickwall that makes it fit, so kira's own hard clamp never fires.
/// 3. **mix tap** — records what is actually heard (`$WOW_MIX_TAP`; a no-op
///    builder unless the env var names a capture path).
/// 4. **output gate** — the focus mute, and the *only* stage downstream of every
///    tap. It is last on purpose: a mute that sits above the taps falsifies every recording made
///    while the window is in the background, which is precisely when an unattended instrumented
///    run records. Here it silences the speakers and nothing else — `pre.wav`, `post.wav`, the
///    `$WOW_MIX_TAP` capture and the health meter all keep reading the mix the game produced.
///
/// A free function so the offline harness (`overlapping_kits_*`, below) can build the **same**
/// chain over kira's mock backend — a headless proof of the audible claim, not a mirror of it.
fn main_track(
    level: &Arc<MixLevel>,
    limiter_on: &Arc<AtomicBool>,
    sample_rate: Option<u32>,
    probe_dir: Option<&Path>,
) -> MainChain {
    let mut main = kira::track::MainTrackBuilder::new();
    // MASTER FIRST — and it is an *effect*, not the main track's own volume, for one reason:
    // kira applies a track's volume **after** its effects (`track/main.rs`: the effect loop, then
    // `*frame *= volume`). Left on the track, the master would sit downstream of the limiter, and
    // the limiter would duck peaks the master was about to remove anyway. Turning the volume down
    // would not stop the pumping — it would move the whole mix down *including* the pumping,
    // which is the one thing a volume knob must never do. Here the limiter sees the signal that
    // is actually going to the device, so it engages exactly when the output would have clipped.
    let master = main.add_effect(VolumeControlBuilder::new(Decibels::IDENTITY));
    meter::install(&mut main, level);
    // A probing run brackets the limiter with two taps. One tap can only ever
    // say what was heard; two say whether the limiter is the thing that changed it — which is
    // precisely the question a "your fix changed nothing" report asks, and one the post-limiter
    // tap alone provably cannot answer.
    let probe = probe_dir.zip(sample_rate);
    let audio_pos = probe
        .and_then(|(dir, rate)| super::mix_tap::install_at(&mut main, &dir.join("pre.wav"), rate));
    limiter::install(&mut main, level, limiter_on);
    if let Some((dir, rate)) = probe {
        super::mix_tap::install_at(&mut main, &dir.join("post.wav"), rate);
    }
    let mut main = super::mix_tap::install(main, sample_rate);
    // Last, after every tap (see the chain doc above). Opens at unity: a run whose focus never
    // changes — and every test in this file — is bit-identical to one built without it.
    let output = main.add_effect(VolumeControlBuilder::new(Decibels::IDENTITY));
    MainChain {
        builder: main,
        master,
        output,
        audio_pos,
    }
}

/// What [`main_track`] hands back: the built chain, plus the two handles a caller needs to reach
/// into it afterwards.
struct MainChain {
    builder: kira::track::MainTrackBuilder,
    /// The master gain, first in the chain (see [`main_track`]). The main track's *own* volume is
    /// left at unity forever — writing to it would reintroduce the post-limiter stage this
    /// exists to avoid.
    master: VolumeControlHandle,
    /// The output gate, last in the chain — the focus mute's handle (see [`Mixer::set_output_gate`]).
    output: VolumeControlHandle,
    /// The pre-limiter tap's frame clock, when a probing run armed one.
    audio_pos: Option<Arc<AtomicU64>>,
}

/// `SoundBufferSize`: the reference's latched mix-ahead dial, read once here
/// the way the reference reads it once at sound-system init — before the `App` exists, so
/// straight from the stored config (`boot_cvar`), the registered default when unset. Clamped
/// to what a ring can sensibly hold; a value outside it is a typo, not a request.
fn mix_ahead_ms() -> u32 {
    crate::cvars::boot_cvar("SoundBufferSize")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|ms| ms.is_finite())
        .map_or(output::MIX_AHEAD_MS, |ms| {
            ms.round().clamp(10.0, 1000.0) as u32
        })
}

/// `$WOW_IO_BUFFER` — the device IO buffer in frames, the A/B arm 1857 leaves open: whether a
/// busier IO thread (512, the shipping-engine norm) or a longer cycle budget (1115's 2048)
/// rides a Space switch better is a question this machine's next crackle answers, and both
/// arms have to be one env var apart to answer it. Unset is the default in `sound::output`.
fn io_buffer_frames() -> u32 {
    std::env::var("WOW_IO_BUFFER")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(output::DEVICE_BUFFER_FRAMES)
}

/// The mix-health counters — what a crackle actually *is*, in numbers (the
/// meters behind them moved into the owned backend in 1857).
///
/// Before this, a crackle was invisible to us: the only report was the director's ear, and we
/// could not tell a missed callback deadline from a stepped parameter from a starved decoder.
#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct MixHealth {
    /// The most recent render pass's load: wall time / the chunk's own duration. The render
    /// thread mixes ahead of the device, so `>= 1.0` here is not yet audible — it is the thread
    /// losing ground; the audible failure is [`Self::overruns`].
    pub(crate) load: f32,
    /// Worst load seen since the last [`Mixer::take_health_peak`].
    pub(crate) peak_load: f32,
    /// Device cycles the mix-ahead ring could not fill, since launch — each one went out as
    /// silence. This is the underrun, the one failure here that is audible by definition.
    pub(crate) overruns: u64,
    /// Stream-side failures since launch (a lost device, a refused open).
    pub(crate) stream_errors: u64,
    /// 3D plays refused because the spatial-voice arena was full ([`SPATIAL_VOICE_CAPACITY`]),
    /// since launch. Every one of these is a sound the player should have heard and did not.
    pub(crate) voices_refused: u64,
}

impl Mixer {
    /// Service the output backend — device notices, stream rebuilds, the meters — and fold the
    /// window into the health counters. Cheap and non-blocking; call every frame.
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

    /// Read and reset the peak — so a report covers the window since the last one, not all time.
    pub(crate) fn take_health_peak(&mut self) -> f32 {
        std::mem::take(&mut self.health.peak_load)
    }

    /// The output meters accumulated since the last take — the report's raw material.
    pub(crate) fn take_output_window(&mut self) -> Window {
        std::mem::take(&mut self.window)
    }
}

/// Move a live spatial track's emitter — the tracked-channel follow (the client's `0x61fec0`
/// tracked play): the kit pump drives this each frame for a source-tagged loop so the sound
/// rides its unit. Free function (not a `Mixer` method) because the pump holds only the track
/// handle, not the mixer.
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

/// EAX listener properties → Freeverb `(feedback, damping, wet level)` — the lossy projection
/// [`Mixer::set_reverb`] documents. Pure so the mapping is pinnable by tests without a device.
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

/// Decode a looping **bed** (zone ambience, weather, underwater) fully and mark it whole-file
/// looping. Loop beds must NOT stream: kira's streaming decoder misreports these 22050 Hz PCM
/// WAVs at 2× their real duration (probe-verified 2026-07-02: ForestNormalDay.wav is 60 s,
/// streaming said 120 s), so `loop_region(..)` lands past EOF and the stream dies mid-file
/// instead of wrapping — the "ambience goes silent after a minute" bug. Statically decoded, the
/// frame count is exact and the loop wraps (probe-verified). Cost: the decoded PCM (~10 MB for a
/// 60 s stereo bed), paid only while the bed plays; music keeps streaming (it never loops).
pub(crate) fn loop_from_bytes(bytes: Vec<u8>) -> Result<StaticSoundData> {
    Ok(sfx_from_bytes(bytes)?.loop_region(..))
}

/// Wrap compressed audio bytes for decode-streaming (music/ambience). The bytes go in behind
/// [`PromotingSource`] — the decode-thread QoS fix — never a bare cursor.
pub(crate) fn stream_from_bytes(bytes: Vec<u8>) -> Result<StreamingSoundData<FromFileError>> {
    StreamingSoundData::from_media_source(PromotingSource(std::io::Cursor::new(bytes)))
        .context("opening stream")
}

/// Compressed-audio source whose reads promote the calling thread — the stream-decode QoS fix.
///
/// kira decodes each streaming sound on a thread of its own (`decode_scheduler.rs`, a bare
/// `std::thread::spawn`) feeding a 16 384-frame ring buffer (~0.37 s at 44.1 kHz); when that
/// buffer runs dry mid-play the sound zero-fills whole callback blocks (`streaming/sound.rs`,
/// the `slots() < 2` branch) — hard amplitude steps, i.e. a crackle, and one that registers on
/// **neither** [`MixHealth`] meter: the mix met its deadline and no stream error fired. A bare
/// spawn lands at *default* QoS — macOS does not inherit the spawner's class
/// (`thread_qos::tests` pins this) — which is below everything the world-entry burst runs: the
/// compute pool at user-interactive, eight IO workers at user-initiated. First login is the
/// guaranteed collision: the glue theme is mid-fade (a live stream) exactly while every core
/// saturates, so its decoder starves and the loading screen crackles.
///
/// The only place our code runs on that thread is the decoder's reads from the byte source, so
/// the source itself promotes: first touch per thread raises it to user-interactive. The class
/// is justified — the mix's input has the hardest deadline in the app, and the thread sleeps
/// ~99% of its life (MP3 decodes at ~100× real time), so the promotion costs nothing. The latch
/// is thread-local; the open/probe calls kira makes on the *main* thread re-assert that
/// thread's existing class, harmlessly.
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

/// How much audible time may go missing per watch window before [`StreamWatch`] reports —
/// over two zero-filled ~43 ms callback blocks: past the ±1-block quantization of the shared
/// position and the sample-clock/wall-clock drift, and far under any real starvation burst
/// (hundreds of ms).
const STREAM_STARVED_MIN_SECS: f64 = 0.1;
/// The accounting window [`StreamWatch`] compares over.
const STREAM_WATCH_WINDOW_SECS: f64 = 1.0;
/// How long a stream may sit audible with its position still pinned where it was constructed
/// before the watch says it never played at all.
///
/// Well past any spin-up this machine produces — the login theme's is the worst of them, because
/// the output device opens in the same frame — and short enough that a cue which never arrives is
/// named while it should still be playing.
const STREAM_START_MAX_SECS: f64 = 3.0;

/// Position-freeze watch over a live stream — the starvation meter [`MixHealth`] lacks (decision
/// 1109; `poll_mix_health`'s docs name this exact blind spot).
///
/// A starved stream is *audible* — kira zero-fills the callback block — but the mix made its
/// deadline, so nothing else measures it. What does move is the stream's shared position: it
/// freezes while the state stays audible. Fed once per frame from the held handle; over each
/// ~[`STREAM_WATCH_WINDOW_SECS`] window it compares wall time elapsed against position advanced
/// and WARNs when more than [`STREAM_STARVED_MIN_SECS`] went missing. Playback rate is always
/// 1.0 here (WoW pitches no music), so the two clocks agree to drift well under the threshold.
///
/// **Every stream opens with a frozen position, and that is not a dropout**.
/// kira's `Shared::new` publishes `state = Playing` and `position = 0.0` when `play()` returns, so
/// the handle reads audible before the renderer has ever seen the sound; and
/// `StreamingSound::process` opens with `if self.frame_consumer.slots() < 2 { out.fill(ZERO);
/// return; }` — it zero-fills *without advancing the position* until the decode thread has two
/// frames ready. The interval between `play()` and the first sample heard is therefore a frozen
/// position by construction, and it is spin-up latency (the cue begins a fraction of a second
/// late), never silence punched into audio that was already playing. So the watch does not start
/// counting until the position has actually moved ([`Phase::Starting`]), and says at `debug!` what
/// it skipped. Before that it charged the spin-up as loss: the 2026-09-15 login reported the glue
/// theme's own first ~235 ms as "injected silence", which is what a crackle does *not* sound like.
///
/// **Known blind spot (1112, deliberate):** the watch samples on the main thread, so a stream
/// that starves *during a main-thread stall* and reaches `Stopped` before the next frame is
/// never accounted — the window is discarded by the reset. Reconstructing it from the stop
/// deadline would trade false positives for coverage; the mix tap (`$WOW_MIX_TAP`) owns that
/// class instead, because the tap records on the audio thread and doesn't care what the main
/// thread was doing.
pub(crate) struct StreamWatch {
    label: &'static str,
    phase: Phase,
    /// Wall time counted into the open window, summed from the fed frame deltas.
    expected: f64,
    /// Stream position gained over those same frames.
    advanced: f64,
}

/// Where a watched stream is in its life: nothing to watch, spinning up, or playing.
#[derive(Clone, Copy)]
enum Phase {
    /// No stream on the slot, or one that is not audible. Position legitimately holds.
    Idle,
    /// Audible, and the position has not moved off `pos` yet — the spin-up (see [`StreamWatch`]).
    /// Nothing is counted here; `warned` holds [`STREAM_START_MAX_SECS`] to one line per stream.
    Starting {
        since: std::time::Instant,
        pos: f64,
        warned: bool,
    },
    /// Advancing. `window_start` is the WALL instant this window's counting began — stamped at
    /// that same instant and only there, so the counted time and the reported span are two
    /// measurements of one interval and *cannot* disagree.
    ///
    /// **They did disagree, across three runs, and that is why this is a field rather than an
    /// `Instant::now()` taken in the report.** It used to be stamped lazily, on the first frame
    /// that found `expected == 0.0` — which is the frame *after* the one the first delta reaches
    /// back to, so the span dropped exactly one frame of the interval it claimed to cover. Two
    /// 2026-09-06 logins called the result arithmetically impossible (a 1.0 s window closing
    /// 0.81 s and 0.756 s of wall after the stream started) and suspected the fed clock; the
    /// 2026-09-15 login that first carried the span printed `counted 1.00 s` over a `0.70 s`
    /// window, and the missing 0.30 s is the 298 ms frame hitch stamped in that same instant.
    /// The clock was right all along; the span was one frame short of what was counted.
    Running {
        last_pos: f64,
        window_start: std::time::Instant,
    },
}

/// What [`StreamWatch::observe`] found this frame. Returned rather than logged so the accounting
/// stays pure and the tests can drive it without a device.
enum Verdict {
    /// A window closed having lost more than [`STREAM_STARVED_MIN_SECS`] of audible time.
    Starved {
        lost: f64,
        counted: f64,
        advanced: f64,
        span: f64,
    },
    /// The stream went audible and its position never moved — nothing from it was ever heard.
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

    /// Per-frame feed from the held handle. Call every frame the handle exists.
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
                // **No cause is named.** This used to say "(decode thread outrun)", which is one
                // of three mechanisms that freeze a stream's position identically — a starved
                // decoder, a render thread that did not run, or a closed/rebuilding device — and
                // the meter cannot tell them apart. Asserting one of the three in the line is how
                // a log hands a reader a conclusion the instrument never reached.
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
            // A bound, not a stopwatch, and it says which bound: the clock starts when the WATCH
            // first saw the handle (within a frame of `play()`, but that frame can be a long one
            // during a login burst), and it can only stop on a frame, so a spin-up that ends
            // inside a long frame is reported as that whole frame.
            Some(Verdict::Began { after }) => debug!(
                "audio: {} took {:.0} ms to start advancing after the watch first saw it — \
                 spin-up, not counted",
                self.label,
                after * 1000.0,
            ),
            None => {}
        }
    }

    /// Drop the baseline — call when the watched handle is dropped/replaced, so the next
    /// stream's position (starting at 0, i.e. *behind* the old one's) can't read as a freeze.
    pub(crate) fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.expected = 0.0;
        self.advanced = 0.0;
    }

    /// Open a counting window at `now` — which is both the position baseline and the wall origin.
    /// The pair is set together on purpose; see [`Phase::Running`] for what happened when it
    /// wasn't.
    fn begin(&mut self, pos: f64, now: std::time::Instant) {
        self.phase = Phase::Running {
            last_pos: pos,
            window_start: now,
        };
        self.expected = 0.0;
        self.advanced = 0.0;
    }

    /// The accounting core, pure so the tests below can drive it without a device.
    fn observe(
        &mut self,
        audible: bool,
        pos: f64,
        dt: f64,
        now: std::time::Instant,
    ) -> Option<Verdict> {
        // Non-audible states (stopped, paused) drop the baseline: position legitimately holds.
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
                // A track swapped onto the slot mid-window starts at 0 — *behind* the one it
                // replaced. That is a new stream, not a freeze, so it goes back to `Starting` and
                // gets its own spin-up excluded exactly like a fresh one. No watched stream ever
                // loops in place: `loop_region` belongs to the static path, never a streamed one
                // (see `stream_from_bytes`), so a backwards jump has no other reading.
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
                // The next window's origin is THIS instant — the one its first delta reaches back
                // to.
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

    /// The EAX→Freeverb projection over the byte-verified 5875 rows: longer decay → higher
    /// feedback (monotone), the underwater HF kill lands near-full damping, and PRESET_OFF's
    /// Room −10000 collapses the wet level to the silence floor.
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

    /// Real 1.12 assets decode through the seam — a short interface WAV as SFX and a zone-music
    /// MP3 as a stream (both paths verified against the 5875 listfile). Needs no audio device
    /// (decode only); skips silently when the gitignored client install isn't present, so CI
    /// without `WoW/Data` stays green.
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

    /// The **offline mix harness**: render N simultaneous copies of one real kit
    /// through the *same* main-track chain the client builds, over kira's mock backend, and
    /// report `(what the mix asked for, what came out)`.
    ///
    /// This is the director's report reduced to arithmetic. No device, no game, no ear: kira's
    /// mock backend runs the real renderer on this thread, so the numbers are the numbers the
    /// audio callback would have produced.
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
        // The whole kit, plus a beat: a buff sound swells, so its peak is nowhere near its first
        // millisecond, and a window that stops early measures silence and calls it headroom.
        let blocks = (data.duration().as_secs_f64() + 0.25) * f64::from(RATE) / 128.0;
        let backend = manager.backend_mut();
        for _ in 0..(blocks.ceil() as usize) {
            backend.on_start_processing();
            backend.process();
        }
        let heard = heard.take();
        let asked = asked.take().peak;
        // `--nocapture` turns the harness into a readout: the arithmetic of the reported defect.
        eprintln!(
            "mix harness: {copies} copies, limiter {} — asked {asked:.2}x, heard {:.2}x, \
             {} sample(s) past full scale",
            if limiter { "on " } else { "off" },
            heard.peak,
            heard.over,
        );
        (asked, heard.peak, heard.over)
    }

    /// A quarter-second 0 dBFS sine as a stereo float WAV — the shape every WoW SFX ships in
    /// (mastered to full scale), built in memory so a test about the *chain* never needs the
    /// gitignored install.
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

    /// The **focus gate silences the output and nothing upstream**.
    ///
    /// This is the claim the whole design rests on, and the one that is expensive to be wrong
    /// about: an instrumented run's window is unfocused by construction, so a mute placed above
    /// the taps would make every unattended audio capture record silence — a false negative that
    /// reads exactly like "the sound never fired". Here the gate is the last effect, so the
    /// meter/tap plane keeps reading the mix the game produced.
    ///
    /// **What falsifies it:** move `output`'s `add_effect` above `meter::install` (or onto the
    /// main track's own volume, which kira applies after every effect) and `asked` collapses to
    /// silence with `heard` — this test fails on the first assert.
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

        // Gate open (its birth state): what the mix asked for is what comes out.
        let (open_asked, open_heard) = render(&mut manager);
        assert!(
            open_asked > 0.9 && open_heard > 0.9,
            "gate open: asked {open_asked:.3}, heard {open_heard:.3} — both should be full scale"
        );

        // Shut it, then settle the 16 ms ramp over silence so what follows measures the steady
        // state and not the fade.
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
            "gate shut: the METER plane must still read the mix the game produced — asked              {shut_asked:.3}. A tap that goes silent with the speakers is the false negative              this ordering exists to prevent"
        );
    }

    /// The master volume must sit **upstream** of the limiter — the ordering fix in
    /// [`main_track`], and the one place a unit test can prove it.
    ///
    /// kira applies a track's own volume *after* its effects, so a master left on the main track
    /// lands downstream of the limiter. The audible symptom is specific and nasty: turn the
    /// slider down and the limiter keeps ducking peaks that the slider was already going to
    /// remove, so the *pumping scales with the mix instead of going away*. A quiet mix would
    /// breathe exactly as hard as a loud one.
    ///
    /// Two 0 dBFS copies at master 0.25 sum to 2.0 before the master and **0.5 after** it —
    /// comfortably inside full scale, so a correctly-placed limiter never engages at all.
    /// Wrongly placed it sees 2.0, clamps to the ceiling, and the reading shows deep gain
    /// reduction on a mix that was never going to clip.
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
        // Snap, not glide: a 10 ms ramp would let the tone's own attack through at near unity and
        // the peak would be the ramp, not the steady state under test.
        master.set_volume(amp_to_db(MASTER), snap());
        // Even a zero-duration tween takes one block to land: kira interpolates a parameter from
        // its previous value across the chunk in which the command is read. Settle over silence
        // and reset the meters, so what follows measures the steady state and not that ramp.
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
        // The limiter reports its gain into the *shared* level cell it was installed with, not
        // into the meter appended after it — read it from the one that can actually see it.
        let inner = asked.take();

        eprintln!(
            "master ordering: {COPIES} copies at master {MASTER} — after master {:.3}x, \
             heard {:.3}x, limiter deepest gain {:.3}",
            inner.peak, heard.peak, inner.reduction,
        );
        // The whole point: the limiter never had anything to do.
        assert!(
            inner.reduction > 0.99,
            "the limiter engaged (deepest gain {:.3}) on a mix that never exceeded full scale — \
             the master is downstream of it again",
            inner.reduction,
        );
        // And the output is the scaled sum, not the limited-then-scaled one (~0.25).
        assert!(
            (heard.peak - MASTER * COPIES as f32).abs() < 0.05,
            "expected the plain scaled sum ~{:.2}, heard {:.3}",
            MASTER * COPIES as f32,
            heard.peak,
        );
        assert_eq!(heard.over, 0, "nothing should have passed full scale");
    }

    /// The probe's two taps must **bracket** the limiter — and this is the test
    /// that the capture handed to the director can actually tell the mechanisms apart.
    ///
    /// The whole reason for a second tap is that `post.wav` alone cannot distinguish "the mix
    /// never clipped" from "the limiter failed to hold it". Both produce a clean post file for
    /// entirely different reasons, and only the pre file separates them. So: render an
    /// over-scale mix through the real chain with the probe armed, and assert the two files
    /// disagree in exactly the way the diagnosis depends on — `pre.wav` far past full scale,
    /// `post.wav` held under it. If these two ever agree, the instrument has gone blind and every
    /// verdict it prints is worthless.
    ///
    /// Writes into the OS temp dir (never the install, never `benilla-config`) and leaves the
    /// capture behind on purpose: `scripts/soundprobe.py $TMPDIR/benilla-probe-selftest` is then
    /// a live check of the analyser against a capture with a known answer.
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

    /// The **voice ceiling is real, and it is ours**: a spatial-track arena
    /// refuses past its capacity, and kira's unnamed default would have refused at 128 — a number
    /// a pack pull reaches, and one nobody here chose. Pins both halves so a kira upgrade that
    /// moves either cannot move benilla's voice ceiling silently.
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

    /// **The reported defect, reproduced and fixed, in numbers**.
    ///
    /// `HolyProtection.wav` is the Fortitude buff — the director's own example ("a priest buffing
    /// a group with mass fort"). It is mastered to 1.000 peak and its kit (`SoundEntries` 3116)
    /// carries `Flags 0x0000`, so the no-duplicate bit does not gate it: a five-target cast
    /// starts five sample-aligned copies in one frame.
    ///
    /// Bypassed, the mix asks for ~5x full scale and delivers it — which the renderer's
    /// `clamp(-1.0, 1.0)` turns into a squared-off waveform. Armed, the mix still *asks* for 5x
    /// (the game's own gain math is untouched) and nothing past the ceiling comes out.
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

        // One copy is already sitting on full scale — that is the headroom problem, before any
        // overlap at all.
        let (asked, heard, over) = render_overlapping(&bytes, 1, false);
        assert!(
            asked > 0.99,
            "a single kit should already reach full scale, got {asked}"
        );
        assert!(over <= 2, "one copy should not meaningfully clip ({over})");
        assert!(heard <= 1.001);

        // Five, bypassed: the reported case. The sum is ~5x and it comes out at ~5x.
        let (asked, heard, over) = render_overlapping(&bytes, 5, false);
        assert!(asked > 4.0, "five copies should sum to ~5x, got {asked}");
        assert!(
            heard > 4.0 && over > 1_000,
            "bypassed, the over-scale mix must reach the renderer's clamp \
             (peak {heard}, {over} samples over)"
        );

        // Five, armed: the same request, held under the ceiling. Nothing clips.
        let (asked, heard, over) = render_overlapping(&bytes, 5, true);
        assert!(asked > 4.0, "the game still asks for ~5x, got {asked}");
        assert_eq!(over, 0, "the limiter let {over} samples past full scale");
        assert!(
            heard <= limiter::ceiling() + 1e-5,
            "output peaked at {heard}"
        );
    }

    /// Frame `i`'s wall instant, so a test reads as a timeline rather than as bookkeeping.
    fn wall(t0: std::time::Instant, secs: f64) -> std::time::Instant {
        t0 + std::time::Duration::from_secs_f64(secs)
    }

    /// The starvation accounting: a stream that advances in lockstep with
    /// wall time stays quiet; one whose position freezes mid-window (kira's zero-fill) reports the
    /// missing time; a slot swap's backward jump is a new stream, not a freeze; and going
    /// non-audible drops the baseline so a later stream starts clean.
    #[test]
    fn stream_watch_accounts_freezes_not_swaps() {
        let dt = 1.0 / 60.0;
        let t0 = std::time::Instant::now();

        // Healthy: position tracks wall time exactly — every window closes clean.
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

        // Starved: 18 frames (~0.3 s) frozen inside the window → that window reports ~0.3 s.
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

        // Slot swap: the position jumps backwards once (the new track starts at 0). That is a new
        // stream, not a freeze — it re-enters the spin-up, and nobody is charged for the jump.
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

        // Non-audible resets the baseline: a stopped-then-restarted stream (position far behind
        // the old one's) opens a fresh window instead of inheriting a phantom freeze.
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

    /// **The span is the interval that was counted** — the pin on decision 2253's off-by-one.
    ///
    /// The window used to be wall-stamped lazily, on the first frame that found nothing counted
    /// yet — one frame *after* the frame its first delta reaches back to. So a hitch at the head
    /// of a window was counted but not spanned, and the line contradicted itself: the 2026-09-15
    /// login printed `counted 1.00 s` over a `0.70 s` window with a 298 ms hitch in the gap, and
    /// two earlier logins had called the same arithmetic impossible and suspected the clock.
    /// Here the head of the window *is* that hitch, and the stream froze through it.
    #[test]
    fn stream_watch_spans_the_window_it_counted() {
        let dt = 1.0 / 60.0;
        let t0 = std::time::Instant::now();
        let hitch = 0.298;
        let mut w = StreamWatch::new("test");

        // `play()` has returned; nothing has been heard yet, so nothing is counted yet.
        assert!(w.observe(true, 0.0, dt, wall(t0, 0.0)).is_none());
        // The first sample lands: the window opens here, at this instant.
        assert!(matches!(
            w.observe(true, 0.001, dt, wall(t0, 0.010)),
            Some(Verdict::Began { .. })
        ));
        // The hitch — 298 ms of wall in one frame, the stream frozen through it. Its delta
        // reaches back to the instant the window opened, so the span must contain it.
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

    /// **A stream's spin-up is not a dropout**, driven here on the timeline of
    /// the 2026-09-15 login that reported the glue theme's own first 235 ms as injected silence.
    ///
    /// kira publishes `Playing` with `position = 0.0` the moment `play()` returns, and
    /// `StreamingSound::process` zero-fills *without advancing the position* until the decode
    /// thread has two frames ready. So every stream opens frozen, and that interval is latency
    /// before the first sample — never silence cut into audio that was already playing.
    ///
    /// That login's shape, relative to the frame the theme started on: the output device opened
    /// and the stream was played inside a frame that then blocked for 298 ms, and the first
    /// sample landed 233 ms in (~100 ms of mix-ahead, ~11 ms of IO buffer, the rest decode).
    /// The old accounting charged all of it, closed its first window at 1.00 s counted against
    /// 0.77 s advanced, and called the difference a crackle.
    #[test]
    fn stream_watch_does_not_charge_a_stream_for_starting() {
        let dt = 1.0 / 60.0;
        let t0 = std::time::Instant::now();
        let hitch = 0.298;
        let first_sample = 0.233;
        // Position as the handle reports it: pinned where it was constructed until the renderer
        // has audio to consume, then advancing with wall time.
        let pos_at = |t: f64| (t - first_sample).max(0.0);

        let mut w = StreamWatch::new("test");
        let mut began = Vec::new();
        let mut starved = 0;
        // The frame the theme started on — audible, position still 0 — and then the hitch.
        let mut ticks = vec![0.0, hitch];
        // Then three seconds of ordinary frames.
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
        // Frame-quantized: the first advance is only visible on the frame after the hitch.
        assert!(
            (began[0] - hitch).abs() < 1e-6,
            "the skipped spin-up, stated as the bound it is: {began:?}"
        );
    }

    /// Excluding the spin-up must not mean excluding a spin-up that never ends: a stream that
    /// goes audible and never plays a sample is still named, once.
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

    /// The fade contract every transition rests on: `stop(tween)` fades the channel to silence
    /// over the tween's duration on the audio thread, and **dropping the handle immediately after
    /// does not cut it short** — the command is already published to the renderer's triple-buffer
    /// (decision 0100's fire-and-forget fade-stop; the director doubted it fires at all). We drive
    /// kira's renderer by hand through a capturing backend and watch a full-amplitude loop after
    /// its handle is dropped: it must ramp *through the middle* — never jump full→0 (an instant
    /// cut) and never stay at full (a command lost with the handle). Device-free (mock renderer),
    /// so it runs in CI.
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

        // A full-amplitude looping bed, so it can only fall silent by our fade — never by EOF.
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

        // benilla's exact pattern: fade to silence over 1 s (100 frames), drop the handle now.
        h.stop(Tween {
            duration: Duration::from_secs(1),
            ..Default::default()
        });
        drop(h);

        // Watch the whole fade in 50 ms blocks (1.3 s of coverage over the 1 s fade).
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
