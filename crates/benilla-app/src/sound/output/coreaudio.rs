//! The CoreAudio half of [`super`] — the thinnest correct layer between the mixer and the
//! device.
//!
//! What lives here and nowhere else: finding the default output device and reading what it
//! runs at; opening a HAL output unit pinned to that device with our stream format and our
//! per-client IO buffer; the four property listeners the backend reacts to; the realtime
//! scheduling of the render thread (time-constraint policy + the device's audio workgroup,
//! Apple's documented pair for an audio worker thread); and the host clock the meters read.
//!
//! Everything is plain C API through the objc2 CoreAudio bindings and `coreaudio-rs`'s
//! `AudioUnit` — the two crates cpal itself stood on, now used directly, because the cpal layer
//! between them and us was where three of the defects 1857 lists lived (a no-op error callback
//! on the default device, a `DefaultOutput` unit pinned to a fixed device, and the IO-cycle
//! timestamp thrown away before it reached anyone).

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use coreaudio::audio_unit::audio_format::LinearPcmFlags;
use coreaudio::audio_unit::render_callback::{self, data};
use coreaudio::audio_unit::{AudioUnit, Element, IOType, SampleFormat, Scope, StreamFormat};
use objc2_core_audio::{
    kAudioDeviceProcessorOverload, kAudioDevicePropertyBufferFrameSize,
    kAudioDevicePropertyBufferFrameSizeRange, kAudioDevicePropertyDeviceIsAlive,
    kAudioDevicePropertyIOThreadOSWorkgroup, kAudioDevicePropertyLatency,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertySafetyOffset,
    kAudioDevicePropertyScopeOutput, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectPropertySelector, AudioObjectRemovePropertyListener,
    AudioObjectSetPropertyData,
};
use objc2_core_audio_types::AudioValueRange;

/// The channel count we render. kira mixes stereo; the HAL output unit maps our two channels
/// onto whatever the device has.
pub(super) const CHANNELS: u32 = 2;

/// `kAudioOutputUnitProperty_CurrentDevice` — pins a HAL output unit to one device.
const OUTPUT_UNIT_CURRENT_DEVICE: u32 = 2000;
/// `kAudioUnitProperty_MaximumFramesPerSlice` — the most frames one render call may ask for.
/// An output unit defaults to 1156; a device buffer larger than that must raise it or the
/// unit refuses the slice (`kAudioUnitErr_TooManyFramesToProcess`).
const UNIT_MAXIMUM_FRAMES_PER_SLICE: u32 = 14;

// ---------------------------------------------------------------------------------------------
// Host clock

// The two mach clock calls, declared here: libc deprecates its copies in favour of a crate we
// have no other use for, and two signatures are not worth a dependency.
#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
}

/// Nanoseconds on the host clock (`mach_absolute_time`, the clock CoreAudio stamps its IO
/// cycles with). Wall-monotonic, cheap (a commpage read), safe on the audio thread.
pub(super) fn now_ns() -> u64 {
    // SAFETY: no arguments, no side effects.
    host_ticks_to_ns(unsafe { mach_absolute_time() })
}

/// Convert a host-clock tick count (an `AudioTimeStamp::mHostTime`) to nanoseconds.
pub(super) fn host_ticks_to_ns(ticks: u64) -> u64 {
    let (numer, denom) = timebase();
    // u128 so a 5-day uptime × numer can't overflow.
    (u128::from(ticks) * u128::from(numer) / u128::from(denom)) as u64
}

fn ns_to_host_ticks(ns: u64) -> u64 {
    let (numer, denom) = timebase();
    (u128::from(ns) * u128::from(denom) / u128::from(numer)) as u64
}

/// The mach timebase, read once. The ratio is a hardware constant; re-fetching it per
/// callback (as cpal did) is gratuitous work on the realtime path.
fn timebase() -> (u32, u32) {
    static TIMEBASE: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: plain out-pointer call; a failure leaves zeros, which we guard.
        let rc = unsafe { mach_timebase_info(&mut info) };
        if rc != 0 || info.denom == 0 {
            (1, 1)
        } else {
            (info.numer, info.denom)
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Device

/// One output device as CoreAudio reports it at open time.
#[derive(Clone, Debug)]
pub(super) struct Device {
    pub id: AudioObjectID,
    pub name: String,
    /// The device's nominal rate — what we render at (the unit does no rate conversion).
    pub sample_rate: u32,
    /// The device's accepted IO buffer range, frames.
    pub buffer_range: (u32, u32),
    /// The device's own output latency, frames (`kAudioDevicePropertyLatency`), for the report.
    pub latency_frames: u32,
    /// The safety offset, frames — the HAL's margin before the DMA that an IOProc must clear.
    pub safety_frames: u32,
}

fn addr(selector: AudioObjectPropertySelector, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Read one fixed-size property. `T` must be the exact C type the selector carries.
fn get<T: Copy>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: u32,
) -> Result<T> {
    let address = addr(selector, scope);
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: `size` bounds the write into `value`; the selector's type is `T` by contract of
    // each call site below.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(value.as_mut_ptr().cast::<c_void>()),
        )
    };
    if status != 0 {
        bail!(
            "property {} read failed (OSStatus {status})",
            fourcc(selector)
        );
    }
    // SAFETY: a zero status means the HAL filled `size` bytes of `T`.
    Ok(unsafe { value.assume_init() })
}

fn set<T: Copy>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: u32,
    value: &T,
) -> Result<()> {
    let address = addr(selector, scope);
    // SAFETY: `value` outlives the call and its size is passed.
    let status = unsafe {
        AudioObjectSetPropertyData(
            object,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            std::mem::size_of::<T>() as u32,
            NonNull::from(value).cast::<c_void>(),
        )
    };
    if status != 0 {
        bail!(
            "property {} write failed (OSStatus {status})",
            fourcc(selector)
        );
    }
    Ok(())
}

fn fourcc(selector: u32) -> String {
    selector
        .to_be_bytes()
        .iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
        .collect()
}

/// The system's current default output device, fully described. `Err` when there is none
/// (headless CI, everything unplugged) — the caller runs silent.
pub(super) fn default_output() -> Result<Device> {
    let id: AudioObjectID = get(
        kAudioObjectSystemObject as AudioObjectID,
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal,
    )
    .context("no default output device")?;
    if id == 0 {
        bail!("no default output device");
    }
    describe(id)
}

fn describe(id: AudioObjectID) -> Result<Device> {
    let name = coreaudio::audio_unit::macos_helpers::get_device_name(id)
        .unwrap_or_else(|_| format!("device {id}"));
    let rate: f64 = get(
        id,
        kAudioDevicePropertyNominalSampleRate,
        kAudioObjectPropertyScopeGlobal,
    )
    .context("device sample rate")?;
    if !(8000.0..=384_000.0).contains(&rate) {
        bail!("device {name} reports an absurd sample rate {rate}");
    }
    let range: AudioValueRange = get(
        id,
        kAudioDevicePropertyBufferFrameSizeRange,
        kAudioObjectPropertyScopeGlobal,
    )
    .context("device buffer range")?;
    let latency_frames = get::<u32>(
        id,
        kAudioDevicePropertyLatency,
        kAudioDevicePropertyScopeOutput,
    )
    .unwrap_or(0);
    let safety_frames = get::<u32>(
        id,
        kAudioDevicePropertySafetyOffset,
        kAudioDevicePropertyScopeOutput,
    )
    .unwrap_or(0);
    Ok(Device {
        id,
        name,
        sample_rate: rate.round() as u32,
        buffer_range: (
            range.mMinimum.max(1.0) as u32,
            range.mMaximum.max(1.0) as u32,
        ),
        latency_frames,
        safety_frames,
    })
}

// ---------------------------------------------------------------------------------------------
// Stream

/// What the IOProc sees each cycle: the interleaved stereo buffer to fill and the host time
/// at which its first frame reaches the DAC (the HAL's own deadline for this cycle).
pub(super) struct Cycle<'a> {
    pub buffer: &'a mut [f32],
    pub frames: usize,
    /// `AudioTimeStamp::mHostTime`, in nanoseconds on the [`now_ns`] clock. Zero if the HAL
    /// did not stamp a host time (never seen on a real device; defended anyway).
    pub output_time_ns: u64,
}

/// An open, running output stream on one device. Dropping it stops the unit and frees the
/// callback synchronously — nothing runs on the IO thread after the drop returns.
pub(super) struct Stream {
    unit: AudioUnit,
    /// The IO buffer the HAL actually granted, frames.
    buffer_frames: u32,
}

impl Stream {
    /// The cycle size the device is actually running, frames — read back after the open,
    /// because the HAL may quietly hand back a different size than asked (Chromium documents
    /// the silent clamp).
    pub(super) fn buffer_frames(&self) -> u32 {
        self.buffer_frames
    }

    /// Open `device` at its nominal rate with a per-client IO buffer of `buffer_frames` (clamped
    /// to the device's range) and start it. `on_cycle` runs on the HAL's realtime IO thread —
    /// it must never block, allocate, or log.
    ///
    /// `_notices` is what a stream raises on its own where the platform has no listener API
    /// (`cpal.rs` wires it to cpal's error callback); here every notice comes from
    /// [`Listeners`], so the stream needs none of it.
    pub(super) fn open<F>(
        device: &Device,
        buffer_frames: u32,
        _notices: &Arc<Notices>,
        mut on_cycle: F,
    ) -> Result<Self>
    where
        F: FnMut(Cycle<'_>) + Send + 'static,
    {
        let buffer_frames = buffer_frames.clamp(device.buffer_range.0, device.buffer_range.1);
        // The IO buffer is a per-client property on the modern HAL — our request sizes OUR IO
        // cycle and nobody else's (verified 2026-09-02: a second process read 512 while this
        // client ran at 2048). Set on the device object before the unit starts.
        set(
            device.id,
            kAudioDevicePropertyBufferFrameSize,
            kAudioObjectPropertyScopeGlobal,
            &buffer_frames,
        )
        .context("device buffer size")?;

        let mut unit = AudioUnit::new(IOType::HalOutput).context("HAL output unit")?;
        unit.set_property(
            OUTPUT_UNIT_CURRENT_DEVICE,
            Scope::Global,
            Element::Output,
            Some(&device.id),
        )
        .context("pinning the unit to the device")?;
        // Our format on the unit's input scope of the output element: what we hand it.
        let format = StreamFormat {
            sample_rate: f64::from(device.sample_rate),
            sample_format: SampleFormat::F32,
            flags: LinearPcmFlags::IS_FLOAT | LinearPcmFlags::IS_PACKED,
            channels: CHANNELS,
        };
        unit.set_stream_format(format, Scope::Input, Element::Output)
            .context("stream format")?;
        // Raise the slice ceiling to the buffer we asked for, or the unit refuses big cycles.
        unit.set_property(
            UNIT_MAXIMUM_FRAMES_PER_SLICE,
            Scope::Global,
            Element::Output,
            Some(&buffer_frames.max(1156)),
        )
        .context("maximum frames per slice")?;

        type Args = render_callback::Args<data::Interleaved<f32>>;
        unit.set_render_callback(move |args: Args| {
            let Args {
                data,
                time_stamp,
                num_frames,
                ..
            } = args;
            let output_time_ns = if time_stamp.mHostTime == 0 {
                0
            } else {
                host_ticks_to_ns(time_stamp.mHostTime)
            };
            on_cycle(Cycle {
                buffer: data.buffer,
                frames: num_frames,
                output_time_ns,
            });
            Ok(())
        })
        .context("render callback")?;
        unit.start().context("starting the unit")?;
        let buffer_frames = get::<u32>(
            device.id,
            kAudioDevicePropertyBufferFrameSize,
            kAudioObjectPropertyScopeGlobal,
        )
        .unwrap_or(buffer_frames);
        Ok(Self {
            unit,
            buffer_frames,
        })
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // Explicit, so the order is ours: stop the IO thread first, then the unit's own Drop
        // uninitialises and frees the callback (and with it whatever the closure owned).
        let _ = self.unit.stop();
    }
}

// ---------------------------------------------------------------------------------------------
// Listeners

/// The flags CoreAudio's notification thread raises; the backend polls them on the main
/// thread. Atomics only — a listener runs on CoreAudio's own thread and may not touch us.
#[derive(Default)]
pub(super) struct Notices {
    /// The system default output device changed (headphones plugged, AirPods connected…).
    pub default_changed: AtomicBool,
    /// The device we are on says it is no longer alive (unplugged).
    pub device_died: AtomicBool,
    /// The device's nominal sample rate changed under us (Audio MIDI Setup).
    pub rate_changed: AtomicBool,
    /// `kAudioDeviceProcessorOverload` count — the HAL saying our IO cycle ran past its
    /// deadline. The one crackle signature that is definitionally audible.
    pub overloads: AtomicU64,
    /// Host time (ns) of the most recent overload, so the report can place it.
    pub last_overload_ns: AtomicU64,
}

unsafe extern "C-unwind" fn on_notice(
    _object: AudioObjectID,
    count: u32,
    addresses: NonNull<AudioObjectPropertyAddress>,
    client: *mut c_void,
) -> i32 {
    // SAFETY: `client` is the `Arc<Notices>` pointer registered with the listener; the backend
    // removes every listener before it drops that Arc.
    let notices = unsafe { &*(client as *const Notices) };
    for i in 0..count as usize {
        // SAFETY: CoreAudio hands `count` valid addresses.
        let address = unsafe { *addresses.as_ptr().add(i) };
        match address.mSelector {
            s if s == kAudioHardwarePropertyDefaultOutputDevice => {
                notices.default_changed.store(true, Ordering::Release);
            }
            s if s == kAudioDevicePropertyDeviceIsAlive => {
                notices.device_died.store(true, Ordering::Release);
            }
            s if s == kAudioDevicePropertyNominalSampleRate => {
                notices.rate_changed.store(true, Ordering::Release);
            }
            s if s == kAudioDeviceProcessorOverload => {
                notices.last_overload_ns.store(now_ns(), Ordering::Relaxed);
                notices.overloads.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
    0
}

/// The listeners armed on one device (plus the one system-wide default-device listener),
/// removed on drop. Rebuilding the stream on a new device drops and re-arms.
pub(super) struct Listeners {
    notices: Arc<Notices>,
    armed: Vec<(AudioObjectID, AudioObjectPropertyAddress)>,
}

impl Listeners {
    pub(super) fn arm(device: &Device, notices: Arc<Notices>) -> Self {
        let device = device.id;
        let client = Arc::as_ptr(&notices) as *mut c_void;
        let wanted = [
            (
                kAudioObjectSystemObject as AudioObjectID,
                addr(
                    kAudioHardwarePropertyDefaultOutputDevice,
                    kAudioObjectPropertyScopeGlobal,
                ),
            ),
            (
                device,
                addr(
                    kAudioDevicePropertyDeviceIsAlive,
                    kAudioObjectPropertyScopeGlobal,
                ),
            ),
            (
                device,
                addr(
                    kAudioDevicePropertyNominalSampleRate,
                    kAudioObjectPropertyScopeGlobal,
                ),
            ),
            (
                device,
                addr(
                    kAudioDeviceProcessorOverload,
                    kAudioObjectPropertyScopeGlobal,
                ),
            ),
        ];
        let mut armed = Vec::with_capacity(wanted.len());
        for (object, address) in wanted {
            // SAFETY: `client` stays valid while `self` holds the Arc; removed in Drop.
            let status = unsafe {
                AudioObjectAddPropertyListener(
                    object,
                    NonNull::from(&address),
                    Some(on_notice),
                    client,
                )
            };
            if status == 0 {
                armed.push((object, address));
            } else {
                bevy::log::warn!(
                    "audio: listener {} on object {object} refused (OSStatus {status})",
                    fourcc(address.mSelector)
                );
            }
        }
        Self { notices, armed }
    }
}

impl Drop for Listeners {
    fn drop(&mut self) {
        let client = Arc::as_ptr(&self.notices) as *mut c_void;
        for (object, address) in self.armed.drain(..) {
            // SAFETY: mirrors the registration above. A dead device may refuse; that is fine —
            // its listeners die with it.
            let _ = unsafe {
                AudioObjectRemovePropertyListener(
                    object,
                    NonNull::from(&address),
                    Some(on_notice),
                    client,
                )
            };
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Realtime scheduling for the render thread

/// `os_workgroup_t` is an ObjC object pointer; we never look inside it.
type OsWorkgroup = *mut c_void;

/// `os_workgroup_join_token_s`: a signature plus 36 opaque bytes on 64-bit. Sized generously —
/// the OS writes into it, we only carry it back to `leave`.
#[repr(C)]
struct JoinToken {
    sig: u32,
    opaque: [u8; 60],
}

extern "C" {
    fn os_workgroup_join(wg: OsWorkgroup, token_out: *mut JoinToken) -> i32;
    fn os_workgroup_leave(wg: OsWorkgroup, token: *mut JoinToken);
    fn os_release(object: *mut c_void);
}

/// The device's IO-thread audio workgroup — the scheduler's notion of "the threads working
/// toward this device's deadline" (WWDC20 *Meet Audio Workgroups*). Retained; released on drop.
pub(super) struct Workgroup(OsWorkgroup);

// SAFETY: an os_workgroup is a thread-safe OS object; we only pass its pointer to `join` /
// `leave` on the joining thread and `os_release` once.
unsafe impl Send for Workgroup {}

impl Workgroup {
    pub(super) fn of_device(device: &Device) -> Option<Self> {
        let wg: OsWorkgroup = get(
            device.id,
            kAudioDevicePropertyIOThreadOSWorkgroup,
            kAudioObjectPropertyScopeGlobal,
        )
        .ok()?;
        (!wg.is_null()).then_some(Self(wg))
    }
}

impl Drop for Workgroup {
    fn drop(&mut self) {
        // SAFETY: the property hands us a retained object we own.
        unsafe { os_release(self.0) };
    }
}

/// The render thread's membership in a workgroup — leaves on drop, on the same thread.
pub(super) struct Joined {
    group: Workgroup,
    token: Box<JoinToken>,
}

impl Joined {
    /// Join the calling thread to `group`. Must be called from the thread that will render,
    /// after [`set_realtime`] (a thread joins as what it is).
    pub(super) fn join(group: Workgroup) -> Result<Self> {
        let mut token = Box::new(JoinToken {
            sig: 0,
            opaque: [0; 60],
        });
        // SAFETY: the group pointer is a live retained object; the token is ours to hold.
        let rc = unsafe { os_workgroup_join(group.0, &mut *token) };
        if rc != 0 {
            bail!("os_workgroup_join failed ({rc})");
        }
        Ok(Self { group, token })
    }
}

impl Drop for Joined {
    fn drop(&mut self) {
        // SAFETY: `leave` with the token `join` filled, from the same thread (the render thread
        // drops its own membership before it exits or re-joins).
        unsafe { os_workgroup_leave(self.group.0, &mut *self.token) };
    }
}

/// The render thread's realtime standing. A time-constraint policy lasts as long as the thread
/// does and needs no unwinding, so this carries nothing; it exists because the Windows half of
/// the same surface (`cpal.rs`) has an MMCSS registration to hand back.
pub(super) struct Realtime;

/// Give the calling thread a time-constraint (realtime) policy: it produces `period_ns` worth
/// of audio per wake and must be done well inside that. The scheduler then treats it like the
/// HAL's own IO thread — above every QoS band, on a performance core.
pub(super) fn set_realtime(period_ns: u64) -> Result<Realtime> {
    let period = ns_to_host_ticks(period_ns) as u32;
    // A render chunk costs a fraction of a millisecond; the constraint is the deadline from
    // period start by which that computation must be done. Half the period leaves the other
    // half to the IO thread's own copy and to any neighbour in the workgroup.
    let policy = libc::thread_time_constraint_policy {
        period,
        computation: (period / 10).max(1),
        constraint: (period / 2).max(2),
        preemptible: 1,
    };
    // SAFETY: the policy struct is the documented flavor's layout, with its documented count.
    let rc = unsafe {
        libc::thread_policy_set(
            libc::pthread_mach_thread_np(libc::pthread_self()),
            libc::THREAD_TIME_CONSTRAINT_POLICY as libc::thread_policy_flavor_t,
            std::ptr::from_ref(&policy) as libc::thread_policy_t,
            libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    };
    if rc != 0 {
        return Err(anyhow!("thread_policy_set(TIME_CONSTRAINT) failed ({rc})"));
    }
    Ok(Realtime)
}
