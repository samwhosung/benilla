//! **The view** — who is looking, with what optics, and how far the detailed world is drawn.
//!
//! Three things, one owner. [`WorldCamera`] marks *the* camera the scene is rendered through;
//! [`CAM_FOVY`] is its fixed optic; [`ViewDistance`] is the faithful `nearclip`/`farclip` **pair**,
//! which is the shape the reference keeps them in (2163).
//!
//! The marker and the optics were in `player::camera` until decision 1160's stage zero, and that
//! was the single largest edge across the engine/game line: **26 engine files** — terrain
//! streaming, the portal PVS, sun follow, picking, every effect sim — reached into the *player
//! controller* to ask which camera to read. None of them care about a player; they care about the
//! viewer, which a world editor and a serverless viewer have without one. `farclip` itself was
//! promoted out of the debug panel earlier, for the same reason at a smaller scale: it was a
//! `ModelDebug` field doing double duty, so it read as a debug knob rather than as config, and
//! subsystems each kept their own idea of it.
//!
//! Read by: the hard far-clip **wall** (terrain/model/liquid/WDL/particle shaders, pushed as
//! `fog_params.w` by `lighting::apply_wow_lighting`), the per-object **cull**
//! (`model_render::apply_model_visibility`), the particle **draw-set gate**
//! (`particles::sim::simulate_particles`) — both through [`within_farclip`] — and the terrain
//! **residency window** (`terrain_stream::window`), which derives its reach from `farclip` the way
//! the reference does. The player's lever is the Terrain Distance row of the
//! options window (the `farclip` CVar); `$WOW_FARCLIP` is the headless one.

use bevy::prelude::*;

/// **The viewer's own body**, as the world needs it — 1160's wire (a), the half that is not about
/// where to stream.
///
/// Three engine lanes read the game's avatar for the same three facts and behind the *same*
/// predicate (`active && !detached`): the WMO interior probe wants the eye's world point, the
/// water foam wants a wading body, the precipitation slab wants the commanded planar speed its
/// tilt keys on. None of them wants a `Player` — they want a body that may or may not be there.
///
/// `None` on [`Self::at`] means exactly what each of those sites used to spell out by hand: no
/// live avatar, or an eye that has been detached from it. A program with no avatar at all leaves
/// this defaulted and every lane takes its no-body branch, which is what the world viewer needs.
#[derive(Resource, Clone, Copy)]
pub struct Viewer {
    /// The avatar's position in **Bevy** space, when one is live and the eye is on it.
    pub at: Option<Vec3>,
    /// Its last-streamed CMovement flags (`MOVEMENTFLAGS`, cached at `unit+0x9e8`). `0` with no
    /// body. Read through [`Self::translating`] / [`Self::turning`] rather than masked at each
    /// site — the bit values are the wire's, and one restatement of them is one too many already.
    pub move_flags: u32,
    /// The **commanded** planar speed in yd/s (`[[player+0x118]+0x84]`): exactly zero with no
    /// direction key held, live rather than a decayed measurement.
    pub planar_speed: f32,
    /// Its collision cylinder height in yards — the foam ring's radius input.
    pub height: f32,
    /// The **first-person feather**: how opaque the viewer's own body is as the camera zooms into
    /// it. `1.0` normally, ramping to `0.0` at full zoom-in. A property of the eye's relationship
    /// to the body, which is why it rides here and not on the body.
    pub self_fade: f32,
    /// **Drunkenness**, `0.0..=1.0` — `PLAYER_BYTES_3` byte 1 clamped at 100. The full-screen haze
    /// is a property of the eye, not of any body in the scene.
    pub drunk: f32,
    /// Is the viewer a **ghost**? While the flag is up the active `LightParams` slot is 4 — the
    /// death profile, applied instantly (`0x6d4620` sets slot 4, `0x6d2260`
    /// re-derives it every frame).
    pub ghost: bool,
    /// Is a loading cover over the world right now, so the viewer has not actually *seen* anything
    /// yet? The appear ramp arms on this falling edge — the faithful trigger is "the player can
    /// see the entity", and the residency proxy goes true well before the cover drops now that
    /// the clear waits for the whole scene.
    pub world_covered: bool,
}

impl Default for Viewer {
    /// No body, and **nothing covering the world** — the defaults a program with no game boots
    /// with. `self_fade` is `1.0`: absent an eye-to-body relationship, nothing is feathered.
    fn default() -> Self {
        Self {
            at: None,
            move_flags: 0,
            planar_speed: 0.0,
            height: 0.0,
            self_fade: 1.0,
            drunk: 0.0,
            ghost: false,
            world_covered: false,
        }
    }
}

impl Viewer {
    /// Is the body **translating**? The four direction bits (`& 0xf`) — the same test the
    /// reference's water-ripple driver runs (`0x5fa760`).
    pub(crate) fn translating(&self) -> bool {
        self.move_flags & 0xf != 0
    }

    /// Is it **turning in place**? The two keyboard turn bits (`& 0x30`). Strafe slides without
    /// turning and is covered by [`Self::translating`]; a mouse-look body-step sets no flag at all.
    pub(crate) fn turning(&self) -> bool {
        self.move_flags & 0x30 != 0
    }
}

/// View distance in yards. `farclip` = WoW's `farclip` CVar — the ONE view distance: the far plane of
/// the detailed world (geometry beyond it is clipped per-pixel, the wall, and the WDL horizon fills
/// in beyond) **and** the reach of terrain residency (`terrain_stream::window`).
/// Default **350** — the reference client's own registered default (superseding
/// 0954's divergence to the clamp's max 777). 777 shipped as "the `Config.wtf` most players ran",
/// but it is the *maximum*, and it is what every player gets before they touch anything: at 777 the
/// residency window is 24 chunks per axis against 350's 11 ([`terrain_stream::window::inner_radius`]),
/// ~4.8x the area streamed, drawn and held resident. The slider still reaches 777.
///
/// **`nearclip` is the second half of the same resource, because the reference stamps them
/// together** (2163). Its per-frame camera outer `0x511bc0` reads both CVar records and writes the
/// camera in four instructions, unconditionally, before the `[cam+0x48]` branch that picks the
/// cinematic leg:
///
/// ```text
/// 511bcf  mov  eax,[0xbe1078]        ; the `nearclip` record handle (cached by 0x50b728's Lookup)
/// 511bd4  fld  dword [eax+0x24]      ; the record's float
/// 511bdc  fstp dword [esi+0x38]      ; cam near
/// 511bdf  mov  ecx,[0xbe0cc0]        ; the `farclip` record handle
/// 511be5  fld  dword [ecx+0x24]
/// 511be8  fstp dword [esi+0x3c]      ; cam far
/// ```
///
/// So `nearclip` is a **live** setting there, not dead plumbing — see [`NEARCLIP_RANGE`] for the
/// half-truth this field replaced.
#[derive(Resource, Clone, Copy)]
pub struct ViewDistance {
    pub farclip: f32,
    /// The camera **near-plane** distance in yards — WoW's `nearclip` CVar, re-stamped onto the
    /// projection every frame by [`stamp_near_clip`] exactly as `0x511bc0` re-stamps `[cam+0x38]`.
    pub nearclip: f32,
}

/// The settable range of [`ViewDistance::farclip`] — the vanilla `farclip` CVar clamp `[177, 777]`
/// (validate callback `0x688d40`), shared by the CVar
/// apply, the options row and the `$WOW_FARCLIP` env knob so none can drift. It used to run to 1200
/// as an A/B lever against the pre-wall "draw everything in the tile window" look; that look is
/// gone and the window now follows this number, so the headroom went with it (1513).
pub const FARCLIP_RANGE: std::ops::RangeInclusive<f32> = 177.0..=777.0;

/// The settable range of [`ViewDistance::nearclip`] — the vanilla `nearclip` CVar clamp, read off
/// its own change callback `0x688d90` the way [`FARCLIP_RANGE`] is read off `0x688d40`: the two
/// bounds are the f32s at `[0x8029d0]` = `0.01` and `[0x808300]` = `0.33`, and a value outside them
/// is **refused** (the callback returns 0 after echoing `0x869abc` `"NearClip must be in range 0.01
/// - 0.33"`), not clamped. Our apply clamps instead, which is this table's standing posture for
/// every range (the consumer clamps at its own edge — `script::cvars`' module doc).
///
/// **Positive control for that decode:** the same shape at `0x688d40` yields `[0x81021c]` = `177`
/// and `[0x80fed8]` = `777`, which is [`FARCLIP_RANGE`] as it has stood since 1624.
///
/// **This const is the correction of a verified-but-partial finding** (2163). [`NEARCLIP_DEFAULT`]
/// used to be a hardcoded `1.0 / 9.0` named `CAM_NEAR`, documented as "the reference's own 1/9,
/// hardcoded in its camera ctor (`0x50a6c0`: `+0x38 = 0x3de38e39`) — the `nearclip` console cvar
/// stores to a global with zero readers, dead plumbing". Every clause of that is true and the
/// conclusion is wrong: the *derived global* `[0xc7b480]` is indeed dead, but the camera never
/// reads that global — it reads the **record**, every frame, at `0x511bd4`. So the ctor's 1/9 is
/// the value the camera holds for exactly as long as it takes the first frame's outer to
/// overwrite it, and is never
/// rendered with. `worldview`'s own near plane had already drifted to `0.1` under a doc claiming it
/// was "kept in step by hand" with the 1/9 — the drift was the tell.
pub const NEARCLIP_RANGE: std::ops::RangeInclusive<f32> = 0.01..=0.33;

/// The world camera's multisampling level — WoW's **`gxMultisample`** CVar.
///
/// Held on the reference's own scale: a **sample count**, clamped to [`MSAA_RANGE`] (`[1, 16]`,
/// the reference's own clamp in the CVar callback at `0x63b250`), where **1 means no
/// multisampling at all** — not "one sample of MSAA". Both of the reference's device backends read
/// it that way: D3D9 at `0x599899` leaves `pp.MultiSampleType` at `D3DMULTISAMPLE_NONE` for any
/// value `<= 1`, and the GL path at `0x59de32` never writes the `WGL_SAMPLE_BUFFERS_ARB` /
/// `WGL_SAMPLES_ARB` pair at all — the attribute list simply terminates where they would begin.
///
/// **Default 1 — off — and that is the reference's own default, not a perf choice dressed up as
/// fidelity.** The reference does not register a literal here: `CVar::Register` at `0x63a950` is
/// handed a string `snprintf("%d")`'d at runtime from field 21 of the `VideoHardware.dbc` row that
/// `DetectHardware` (`0x641260`) matched the GPU to. Across the shipped 193-row table that field
/// only ever holds 1 (144 rows) or 2 (49 rows) — nothing higher exists anywhere in it — and all
/// three rows reachable by the fallback match hold **1**. Every id in the table is 2004-era, so no
/// modern GPU matches a specific row and the fallback is what answers: the string registered on
/// any machine this client actually runs on today is `"1"`.
///
/// **Latched, like the reference's own flag byte** (`CVar::Register` flags `3` = registered |
/// latched; the callback echoes `"set pending gxRestart"`). The value here is the **pending** one:
/// changing it persists and `GetCVar` reports it, but the device — here, the camera's `Msaa`
/// component — keeps what it was born with until the next launch. That is not a limitation we are
/// working around; it is the reference's behaviour, and it happens to be forced on us anyway,
/// since swapping MSAA live leaves our post passes MSAA-mismatched and freezes the view.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub struct MsaaSetting {
    /// The requested sample count, `[1, 16]`. 1 = no multisampling.
    pub samples: u32,
}

/// The settable range of [`MsaaSetting::samples`] — the reference's own `atoi`-then-clamp `[1, 16]`
/// at `0x63b250`. Shared by the CVar apply and the `$WOW_MSAA` env knob so the two cannot drift.
pub const MSAA_RANGE: std::ops::RangeInclusive<u32> = 1..=16;

impl Default for MsaaSetting {
    /// `$WOW_MSAA` overrides the default, **session-only** — the A/B lever, the same posture as
    /// `$WOW_FARCLIP` (a value pinned into `config.toml` would make a measurement sticky). The
    /// spellings it has always accepted are kept: `off`/`0`/`1` all mean none.
    fn default() -> Self {
        let samples = match std::env::var("WOW_MSAA").ok().as_deref() {
            Some("off") => Some(1),
            Some(v) => v.parse::<u32>().ok(),
            None => None,
        }
        .map_or(1, |v| v.clamp(*MSAA_RANGE.start(), *MSAA_RANGE.end()));
        Self { samples }
    }
}

impl MsaaSetting {
    /// The sample count as a Bevy [`Msaa`](bevy::render::view::Msaa) level.
    ///
    /// wgpu can only express 1/2/4/8, while the reference's range runs to 16, so a request lands on
    /// the **largest expressible level at or below it**. That direction is the reference's own: its
    /// device-init retry loop (`0x63b380`) steps the sample count *down* — `-= 2`, floored at 1 —
    /// and only once it bottoms out does it start giving up depth and colour bits. Nothing anywhere
    /// steps it up, so rounding down can never hand a player more than they asked for.
    pub fn level(self) -> bevy::render::view::Msaa {
        use bevy::render::view::Msaa;
        match self.samples {
            0..=1 => Msaa::Off,
            2..=3 => Msaa::Sample2,
            4..=7 => Msaa::Sample4,
            _ => Msaa::Sample8,
        }
    }
}

/// The sample counts this run's GPU actually accepts — the guard between a player's
/// `gxMultisample` and a wgpu validation error.
///
/// **This closes a hazard [`MsaaSetting`] itself opened.** Decision 1629 made the sample count a
/// real setting a player can type, and nothing between the CVar and the GPU asked whether the GPU
/// could do it. Bevy does not ask either — `bevy_render`'s `prepare_view_targets` writes
/// `sample_count: msaa.samples()` into the descriptor unchecked (`view/mod.rs:1142`). wgpu *does*:
/// `wgpu-core` tests the count against the format's `sample_count_supported` and fails
/// `CreateTextureError::InvalidSampleCount` (`device/resource.rs:1316`), then tests it again on
/// every pipeline rendering into that texture (`:3882`, `:3961`, `:4013`). A validation error on a
/// texture rebuilt every frame is not a graceful degrade; it is the client dying on a value the
/// player was invited to type. `gxMultisample 8` on a device that stops at 4x is the live case.
///
/// **Stepping down is the reference's own answer, not a defensive invention.** Its device-init
/// retry loop at `0x63b380` walks the sample count *down* (`-= 2`, floored at 1) until the device
/// accepts a mode, and only once it bottoms out does it begin surrendering depth and colour bits.
/// A request the hardware cannot meet becoming the nearest one it can is therefore the faithful
/// behaviour; refusing to boot is what would be unfaithful.
///
/// The three formats are every one we multisample into: the world camera's off-screen colour target
/// (`Rgba16Float` — `benilla_app::world_backdrop`), `Core3d`'s depth (`CORE_3D_DEPTH_FORMAT` =
/// `Depth32Float`), and the swapchain format the world viewer and the fallback camera render
/// straight into. A count is granted only if **all three** take it: they are separate capability
/// bits in wgpu, and a device may well offer 8x colour alongside 4x depth.
fn supported_sample_counts(adapter: &bevy::render::renderer::RenderAdapter) -> Vec<u32> {
    use bevy::image::BevyDefault as _;
    use bevy::render::render_resource::TextureFormat;

    let formats = [
        TextureFormat::Rgba16Float,
        TextureFormat::Depth32Float,
        TextureFormat::bevy_default(),
    ];
    MSAA_RANGE
        .clone()
        .filter(|&n| {
            formats.iter().all(|f| {
                adapter
                    .get_texture_format_features(*f)
                    .flags
                    .sample_count_supported(n)
            })
        })
        .collect()
}

/// The largest count in `supported` at or below `requested`, floored at 1 — no multisampling, which
/// no device can refuse. The step-*down* direction is the reference's; see
/// [`supported_sample_counts`].
pub fn clamp_to_supported(requested: u32, supported: &[u32]) -> u32 {
    supported
        .iter()
        .copied()
        .filter(|&n| n <= requested)
        .max()
        .unwrap_or(1)
}

/// The multisample formats this run's device accepts, as the Video options dropdown offers them —
/// `(color_bits, depth_bits, samples)`, ascending.
///
/// The reference's distilled triple list (`[0xb4b444]`, count `[0xb4b440]`, built by `0x48c3e0` out
/// of the D3D `CheckDeviceMultiSampleType` sweep or the GL `wglGetPixelFormatAttribivARB` sweep) —
/// here, whatever wgpu says it will take. Filled by [`MsaaSupportPlugin`] from the same enumeration
/// that does the clamping, so the list a player picks from and the ceiling a typed value is clamped
/// to cannot disagree.
///
/// Empty until the adapter exists (and forever, headless): an empty dropdown is the honest answer
/// for a run with no device, and the Lua walks it zero times.
#[derive(Resource, Default, Clone, Debug)]
pub struct MsaaFormats {
    pub formats: Vec<(u32, u32, u32)>,
}

impl MsaaFormats {
    /// `requested`, stepped DOWN to the nearest count this device actually offers.
    ///
    /// **The single expression of "the device has the final say"**. It used to
    /// exist only inside [`MsaaSupportPlugin::finish`], which runs once, before the first update —
    /// i.e. against whatever [`MsaaSetting::default`] seeded, and therefore *before*
    /// `config.toml` had been folded in. The doc two paragraphs up already claimed that "the list
    /// a player picks from and the ceiling a typed value is clamped to cannot disagree"; a typed
    /// value was in fact never clamped to it at all, and a saved `gxMultisample = 8` opened on a
    /// GPU that stops at 4 handed the camera an 8 — the 2026-08-26 render-thread kill, one launch
    /// later and on a machine the setting was never chosen on.
    ///
    /// **An empty list is no opinion, not a refusal.** Headless, or before the adapter exists,
    /// there is no device to be wrong about and `requested` passes through — the same early-out
    /// `finish` takes when there is no `RenderAdapter`. Clamping to 1 there would turn every
    /// device-less test and every worldview boot into a silent "multisampling off".
    pub fn clamp(&self, requested: u32) -> u32 {
        if self.formats.is_empty() {
            return requested;
        }
        let counts: Vec<u32> = self.formats.iter().map(|&(_, _, n)| n).collect();
        clamp_to_supported(requested, &counts)
    }
}

/// Colour and depth bit counts for the dropdown's label, derived from the formats we actually
/// render into rather than typed: `MULTISAMPLING_FORMAT_STRING` is
/// `"%d-bit color %d-bit depth %dx multisample"`, and a number in it should be true.
///
/// The colour figure is the **swapchain**'s, not the world camera's off-screen `Rgba16Float`
/// target. What the dropdown is describing is the mode the window is presented in — the thing the
/// reference's enumerators were asking the device about — and quoting 64 there would be describing
/// an internal buffer no player chose.
fn dropdown_bit_depths() -> (u32, u32) {
    use bevy::image::BevyDefault as _;
    use bevy::render::render_resource::TextureFormat;

    let bits = |f: TextureFormat| f.block_copy_size(None).unwrap_or(4) * 8;
    (
        bits(TextureFormat::bevy_default()),
        bits(TextureFormat::Depth32Float),
    )
}

/// Clamps [`MsaaSetting`] to what this run's GPU accepts, once, before anything spawns a camera.
///
/// **All the work is in `finish`, deliberately** — the same reason as
/// `benilla_assets::gpu_blp::BlpGpuSupportPlugin`: `RenderAdapter` is inserted into the main world
/// by `RenderPlugin::finish` (`bevy_render/src/lib.rs:419`), so it does not exist at `build` time
/// and never exists at all in a headless app. Every plugin's `finish` completes before the runner's
/// first update, and the cameras read `MsaaSetting` in `Startup` (1629's latch), so this is both
/// the earliest honest moment and comfortably early enough.
///
/// Headless — no adapter, or no setting — changes nothing: such a run builds no swapchain to be
/// wrong about.
pub struct MsaaSupportPlugin;

impl Plugin for MsaaSupportPlugin {
    fn build(&self, _app: &mut App) {}

    fn finish(&self, app: &mut App) {
        // Scoped so the adapter borrow ends before the resource is written.
        let supported = match app
            .world()
            .get_resource::<bevy::render::renderer::RenderAdapter>()
        {
            Some(adapter) => supported_sample_counts(adapter),
            None => return,
        };
        // Published whether or not anything needs clamping: this is the dropdown's whole menu.
        let (color_bits, depth_bits) = dropdown_bit_depths();
        app.insert_resource(MsaaFormats {
            formats: supported
                .iter()
                .map(|&s| (color_bits, depth_bits, s))
                .collect(),
        });
        let Some(requested) = app.world().get_resource::<MsaaSetting>().map(|m| m.samples) else {
            return;
        };
        // Through `MsaaFormats::clamp`, not `clamp_to_supported` directly: this seed clamp and the
        // per-write clamp in `cvars::apply_to_knobs` must be the same rule, or the pair drifts
        // exactly the way the missing half already did (1643).
        let granted = app.world().resource::<MsaaFormats>().clamp(requested);
        if granted == requested {
            debug!("msaa: {requested}x accepted (this GPU offers {supported:?})");
            return;
        }
        // At `warn`, not `debug`: the player asked for something and did not get it, and this is
        // the only place that fact exists. A line that survives only in a debug build cannot answer
        // "why won't my setting stick?" from a player's log.
        warn!(
            "msaa: this GPU does not offer {requested}x — using {granted}x (it offers {supported:?})"
        );
        app.world_mut().resource_mut::<MsaaSetting>().samples = granted;
    }
}

/// The world camera's projection far plane in yards — the **horizon** plane, far beyond `farclip`
/// on purpose so the coarse WDL ring draws behind the wall (the reference's own
/// `horizonfarclip` is a second plane floored at `farclip + 528`, default 2112). One number, not a
/// function of anything: the detailed world ends at `farclip` by the wall, never by this plane.
pub const CAM_FAR: f32 = 3000.0;

impl ViewDistance {
    /// Set the near plane from a `nearclip` write, under the reference's own bounds.
    ///
    /// **A method and not a public [`NEARCLIP_RANGE`] for the CVar host to clamp against**, which
    /// is how `farclip` does it — because `farclip`'s range has a second consumer (the Terrain
    /// Distance slider is built with those bounds) and this one does not, and `benilla-app`
    /// naming one more engine item is what `tests/world_api_wall.rs` exists to make expensive.
    /// The knob owning its own clamp is also the shape `ClutterConfig::set_frill_density` settled
    /// on for the same question (2151).
    pub fn set_nearclip(&mut self, v: f32) {
        self.nearclip = v.clamp(*NEARCLIP_RANGE.start(), *NEARCLIP_RANGE.end());
    }
}

impl Default for ViewDistance {
    /// `$WOW_FARCLIP` (yd, clamped to [`FARCLIP_RANGE`]) overrides the 350 default. The options row is
    /// the live lever, but a headless capture has no hands — and a horizon or fog report almost always
    /// arrives with the director's slider somewhere other than the default (the 0684 gap was invisible
    /// at 777 and glaring at 320), so reproducing one must not need a human. Read once at startup, like
    /// the other capture-side knobs.
    fn default() -> Self {
        let farclip = std::env::var("WOW_FARCLIP")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .map_or(350.0, |v| {
                v.clamp(*FARCLIP_RANGE.start(), *FARCLIP_RANGE.end())
            });
        Self {
            farclip,
            nearclip: NEARCLIP_DEFAULT,
        }
    }
}

/// Is a world bounding sphere inside the far-clip wall — i.e. does the detailed world still draw it?
///
/// **The one spelling of "is it nearer than `farclip`", shared by every CPU-side consumer.** The test is
/// planar depth along the camera-forward axis (`(center − eye)·fwd`) of the sphere's NEAREST point, which
/// is deliberately the *same coordinate* the per-pixel wall uses in the shaders (`terrain.wgsl` /
/// `wow_model.wgsl` / `wow_effect.wgsl` all discard on eye-Z past `fog_params.w`). Agreeing on the
/// coordinate is what makes an object straddling the boundary **dissolve** through it instead of popping
/// when its origin crosses.
///
/// Radial distance would be the obvious alternative and it is wrong: it disagrees with the wall off-axis,
/// so a wide object at the edge of the frame pops while its pixels were still being drawn.
///
/// ## Why this is not the camera's far plane
/// The world camera's projection far is ~3000 yd — far *beyond* `farclip` on purpose, so the coarse WDL
/// horizon can draw behind the wall. So the frustum's own far plane is **not** the reference's far plane,
/// and a `Frustum::intersects_sphere(.., intersect_far = true)` is not a substitute for this test. In the
/// reference there is one projection far plane at `farclip` and it bounds the detailed world; here that
/// bound is this function plus the shaders' per-pixel discard, and nothing else.
pub fn within_farclip(
    farclip: f32,
    cam_pos: Vec3,
    cam_fwd: Vec3,
    center: Vec3,
    radius: f32,
) -> bool {
    (center - cam_pos).dot(cam_fwd) - radius <= farclip
}

/// Marks **the world camera** — the one flying the scene. Every "where is the viewer" consumer
/// (terrain streaming, PVS, sun follow, sound listener, picking, the capture pin, …) filters on this,
/// NOT on `Camera3d`: since the portrait booths (the portrait booths) there are multiple `Camera3d`s,
/// and a bare `With<WorldCamera>` query silently reads (or writes!) an off-screen booth camera — exactly
/// how the capture pin once yanked the booths to the scenario eye and blanked every portrait.
#[derive(Component)]
pub struct WorldCamera;

/// The `$WOW_MSAA` startup knob → a sample count, shared by both world-camera spawners
/// (`benilla_world::worldview` and the app's `player::setup`) so the two can never disagree.
///
/// **8x is refused rather than passed through.** wgpu validates the sample count against the HDR
/// target's format, and the WebGPU spec guarantees only `[1, 4]` for `Rgba16Float` (this Mac's
/// adapter reports `[1, 2, 4]`). `WOW_MSAA=8` therefore panicked the render thread inside
/// `prepare_view_targets` on frame one — an A/B leg that asked for it died with a wgpu validation
/// error instead of returning an answer, which is how it was found. There is no adapter handle at
/// camera-spawn time to query, so the honest move is to fall back to 4x and say so loudly: a
/// usable knob and a legible failure beat a hard crash.
///
/// A STARTUP knob, not a live toggle: switching MSAA at runtime leaves the post-process passes
/// (glow/egui) MSAA-mismatched and freezes the view, so A/B by restarting.
pub fn msaa_from_env() -> bevy::render::view::Msaa {
    use bevy::render::view::Msaa;
    match std::env::var("WOW_MSAA").ok().as_deref() {
        Some("off") | Some("0") | Some("1") => Msaa::Off,
        Some("2") => Msaa::Sample2,
        Some("8") => {
            warn!(
                "WOW_MSAA=8: Rgba16Float is only guaranteed [1, 4] samples and this adapter \
                 reports [1, 2, 4] — falling back to 4x rather than panicking the render thread"
            );
            Msaa::Sample4
        }
        _ => Msaa::Sample4,
    }
}

/// The **registered default** of the `nearclip` CVar — `"0.1"`, the default string at `0x84fb48`
/// passed by `CVar::Register 0x63db90` at `0x68867a` (name `0x84ffb0` `"nearclip"`, help
/// `"Near clip plane distance"`, callback `0x688d90`, record `[0xc7f348]`). 1804's law, so this is
/// a `Same` row, not a choice.
///
/// The live value is [`ViewDistance::nearclip`]; this is only where it starts and what the
/// off-world spawners ([`crate::worldview`], the depth probe) use when there is no CVar table.
///
/// **Why the number matters even though it is a tenth of a yard.** It was `1.0` from 0062 to 0905
/// "for depth precision" — a rationale that predates knowing the pipeline: the projection is
/// `perspective_infinite_reverse_rh` on a float depth buffer (the app's `capture::depth_probe`
/// tests draw with the real one — it lives a crate up, so this is deliberately not a doc link),
/// where `depth = near/z` makes relative precision — and our
/// ULP-relative bias ladder ([`crate::sky_order`]) — independent of the near value. The small near
/// is what keeps the whole waterline-crossing band (the corner-min submersion probe,
/// `liquid::detect_submersion`) inches tall instead of a yard. It is also the distance the
/// self-avatar fade completes over ([`crate::model_fade::self_model_fade_alpha`] takes it as
/// `nearclip`), so the model finishes fading exactly as the near plane would begin to slice it —
/// the coupling the reference has for free, both being `[cam+0x38]`.
pub const NEARCLIP_DEFAULT: f32 = 0.1;

/// **The reference's `0x511bc0`** — re-stamp the world camera's near plane from the live `nearclip`
/// every frame, so a `SetCVar`/`/console nearclip` write reaches this frame's picture the way it
/// does there (2163). The far plane is deliberately NOT stamped here: ours is [`CAM_FAR`], the
/// horizon plane, and `farclip` is the wall — 0684's split, which the reference does not have.
///
/// Cheap enough to run unconditionally (one resource read, one component write on one entity), and
/// unconditional is also what the reference does — it re-stamps on every frame, changed or not.
pub fn stamp_near_clip(
    view: Res<ViewDistance>,
    mut cam: Query<&mut Projection, With<WorldCamera>>,
) {
    for mut proj in &mut cam {
        // Read through `as_ref` first: taking `&mut` on a Bevy component marks it changed whether
        // or not the write differs, and a projection that reports "changed" every frame is a lie
        // every downstream `Changed<Projection>` has to pay for.
        let Projection::Perspective(current) = proj.as_ref() else {
            continue;
        };
        if current.near == view.nearclip {
            continue;
        }
        if let Projection::Perspective(p) = &mut *proj {
            p.near = view.nearclip;
        }
    }
}
/// The camera's vertical field of view (radians) — one constant shared by the projection
/// (the camera spawn) and every consumer that needs the near rectangle's true shape. 45°, the value the
/// projection has always used (Bevy's `PerspectiveProjection` default, ≈ the reference's 44.1° —
/// [`crate::sun`]'s projection note); naming it here just stops the consumers drifting apart.
pub const CAM_FOVY: f32 = std::f32::consts::FRAC_PI_4;

/// **The world camera's world pose, made current before anything in `Update` reads it** — the
/// set every `Update`-stage viewer authority must order after.
///
/// Bevy propagates `GlobalTransform` in `PostUpdate`. For the whole of `Update`, therefore, a
/// camera's `GlobalTransform` is the pose it had **last** frame, while its `Transform` — written
/// by the controller in [`crate::schedule::WorldStage::Input`] — is this frame's. Walking, the two
/// differ by centimetres and nothing shows. On the frame a teleport snaps they differ by the whole
/// jump, and an authority that decides *what may draw* off the stale one gates a frame drawn from
/// the new pose with a verdict about the old place.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CameraPoseSet;

/// Copy the world camera's `Transform` into its `GlobalTransform` (see [`CameraPoseSet`]).
///
/// **Root cameras only.** The world camera is spawned unparented (both the fallback and the real
/// one), and for a root the propagation Bevy will run in `PostUpdate` is exactly this copy — so
/// this makes the same value available earlier and `PostUpdate` recomputes it identically. A
/// camera that ever acquires a parent keeps Bevy's propagated value and its old one-frame lag,
/// which is no worse than before and never silently wrong: the pose it reports is a real pose the
/// camera had, just not this frame's.
///
/// Written through `set_if_neq` so a still camera does not flag `Changed<GlobalTransform>` every
/// frame for the consumers that key on it.
/// The root world cameras and their two transforms — see [`publish_camera_pose`].
type RootCameraPose<'w, 's> =
    Query<'w, 's, (&'static Transform, &'static mut GlobalTransform), RootCamera>;
/// A world camera Bevy will propagate as a root (no parent to inherit from).
type RootCamera = (With<WorldCamera>, Without<ChildOf>);

pub(crate) fn publish_camera_pose(mut cam: RootCameraPose) {
    for (t, mut g) in &mut cam {
        g.set_if_neq(GlobalTransform::from(*t));
    }
}

/// The view lane's plugin — today, [`publish_camera_pose`] and its ordering contract.
pub(crate) struct ViewPlugin;

impl Plugin for ViewPlugin {
    fn build(&self, app: &mut App) {
        plugin(app);
    }
}

pub(crate) fn plugin(app: &mut App) {
    // The reference's per-frame clip stamp (2163). In `Update` and not `PostUpdate` so a
    // `SetCVar("nearclip", …)` drained this frame reaches this frame's projection, which is the
    // ordering `0x511bc0` has for free by running inside the world-frame driver.
    //
    // The resource is init'd HERE and not only in `world_plugins`: a system's own plugin owes it
    // its parameters, and a harness that adds this plugin for the pose set (`view::tests`) got a
    // "Resource does not exist" panic instead. `init_resource` is idempotent, so the app's own
    // registration still wins wherever it already ran.
    app.init_resource::<ViewDistance>()
        .add_systems(Update, stamp_near_clip);
    app.add_systems(
        Update,
        publish_camera_pose
            .in_set(CameraPoseSet)
            // After the controller that writes the camera's `Transform`
            // ([`crate::schedule::WorldStage::Input`]) — the point of the whole system is to
            // publish the pose the controller just chose, not the one before it.
            .after(crate::schedule::WorldStage::Input),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// **`0x511bc0`'s near stamp** — the live `nearclip` reaches the projection, and a projection
    /// already holding it is not marked changed for the privilege (2163).
    #[test]
    fn the_near_plane_follows_the_cvar_and_settles() {
        let mut app = App::new();
        app.add_systems(Update, stamp_near_clip);
        let cam = app
            .world_mut()
            .spawn((
                WorldCamera,
                Projection::from(PerspectiveProjection {
                    near: NEARCLIP_DEFAULT,
                    ..default()
                }),
            ))
            .id();
        // A player (or an addon's `ConsoleExec("nearClip 0.3")`) moves the knob.
        app.insert_resource(ViewDistance {
            farclip: 350.0,
            nearclip: 0.3,
        });
        app.update();
        let near = |app: &App| match app.world().get::<Projection>(cam).unwrap() {
            Projection::Perspective(p) => p.near,
            _ => unreachable!("spawned perspective"),
        };
        assert_eq!(near(&app), 0.3, "the stamp is the reference's, every frame");

        // ...and the SECOND frame, with nothing moved, must not report the projection changed:
        // `&mut` on a Bevy component marks it whether or not the write differs. (Verified by
        // mutation: dropping the `as_ref` guard in `stamp_near_clip` fails exactly this line.)
        let tick = app.world().read_change_tick();
        app.update();
        assert_eq!(near(&app), 0.3);
        let now = app.world().read_change_tick();
        assert!(
            !app.world()
                .entity(cam)
                .get_ref::<Projection>()
                .unwrap()
                .last_changed()
                .is_newer_than(tick, now),
            "an unchanged near plane must not dirty the projection"
        );
    }

    /// The contract of [`CameraPoseSet`]: a system ordered after it, in the same `Update`, reads
    /// the pose the controller wrote **this** frame — not the one Bevy will propagate at the end
    /// of it. Written as the frame it fixes: move the camera the way a teleport snap does, and
    /// assert the downstream authority sees the destination.
    #[test]
    fn a_reader_after_the_pose_set_sees_this_frames_camera() {
        #[derive(Resource, Default)]
        struct Seen(Vec3);

        fn snap_the_camera(mut cam: Query<&mut Transform, With<WorldCamera>>) {
            for mut t in &mut cam {
                t.translation = Vec3::new(500.0, 0.0, 0.0);
            }
        }
        fn read_the_pose(cam: Query<&GlobalTransform, With<WorldCamera>>, mut seen: ResMut<Seen>) {
            for g in &cam {
                seen.0 = g.translation();
            }
        }

        let mut app = App::new();
        app.init_resource::<Seen>();
        app.configure_sets(Update, crate::schedule::WorldStage::Input);
        app.add_systems(
            Update,
            snap_the_camera.in_set(crate::schedule::WorldStage::Input),
        );
        plugin(&mut app);
        app.add_systems(Update, read_the_pose.after(CameraPoseSet));
        app.world_mut().spawn((
            WorldCamera,
            Transform::default(),
            GlobalTransform::default(),
        ));

        app.update();
        assert_eq!(
            app.world().resource::<Seen>().0,
            Vec3::new(500.0, 0.0, 0.0),
            "an Update-stage viewer authority must see the snapped pose, not the frame-old one"
        );
    }

    /// A **parented** camera is left to Bevy's propagation: a plain copy of a child's local
    /// `Transform` would be a wrong pose, which is worse than a frame-old right one.
    #[test]
    fn a_parented_camera_is_left_alone() {
        let mut app = App::new();
        plugin(&mut app);
        let parent = app.world_mut().spawn(Transform::default()).id();
        let child = app
            .world_mut()
            .spawn((
                WorldCamera,
                Transform::from_xyz(7.0, 0.0, 0.0),
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut().entity_mut(parent).add_child(child);

        app.world_mut()
            .run_system_once(publish_camera_pose)
            .unwrap();
        assert_eq!(
            app.world()
                .entity(child)
                .get::<GlobalTransform>()
                .unwrap()
                .translation(),
            Vec3::ZERO,
            "the child's local transform is not a world pose"
        );
    }
    /// The wall is planar depth along camera-forward, measured to the sphere's nearest point.
    #[test]
    fn planar_depth_of_the_nearest_point() {
        let eye = Vec3::ZERO;
        let fwd = Vec3::NEG_Z; // Bevy's camera looks down −Z
        let at = |d: f32, r: f32| within_farclip(777.0, eye, fwd, Vec3::new(0.0, 0.0, -d), r);
        assert!(at(700.0, 0.0));
        assert!(at(777.0, 0.0)); // exactly at the wall still draws (the shader discards past it)
        assert!(!at(778.0, 0.0));
        // A big object straddling the wall stays in: its near side is still inside, and the
        // per-pixel wall dissolves the far half. This is the no-pop property.
        assert!(at(800.0, 30.0));
        assert!(!at(900.0, 30.0));
    }

    /// Off-axis is where radial distance and the shader's eye-Z part company — the wall is a PLANE,
    /// so a point 700 yd forward and 700 yd sideways (radially ~990) is still inside it.
    #[test]
    fn the_wall_is_a_plane_not_a_sphere() {
        let eye = Vec3::ZERO;
        let fwd = Vec3::NEG_Z;
        let off = Vec3::new(700.0, 0.0, -700.0);
        assert!(off.length() > 777.0, "radially outside");
        assert!(
            within_farclip(777.0, eye, fwd, off, 0.0),
            "but inside the planar wall — must match the shader, which discards on eye-Z"
        );
    }

    /// Behind the camera is trivially inside the wall (negative depth); the lateral frustum planes,
    /// not this test, are what reject it. Pinned so nobody "fixes" this into an abs().
    #[test]
    fn behind_the_camera_is_not_this_tests_job() {
        let eye = Vec3::ZERO;
        assert!(within_farclip(
            777.0,
            eye,
            Vec3::NEG_Z,
            Vec3::new(0.0, 0.0, 5000.0),
            0.0
        ));
    }
}

#[cfg(test)]
mod msaa_tests {
    use super::*;
    use bevy::render::view::Msaa;

    /// The env-less default is **off**, and off is spelled 1 (the reference's scale), not 0.
    /// Welded to the `gxMultisample` registration in `benilla-app`'s `cvars.rs` by its own test.
    #[test]
    fn the_default_is_the_references_one_sample_which_means_none() {
        // Not `MsaaSetting::default()` — a test run under `$WOW_MSAA` would read the env and this
        // is a claim about the literal.
        let literal = MsaaSetting { samples: 1 };
        assert_eq!(literal.level(), Msaa::Off);
        assert_eq!(*MSAA_RANGE.start(), 1);
        assert_eq!(*MSAA_RANGE.end(), 16);
    }

    /// A request lands on the largest level wgpu can express at or below it — the direction the
    /// reference's own device-init retry loop steps (`0x63b380`, `-= 2` floored at 1). Never up:
    /// asking for 3 must not silently buy 4 samples' worth of bandwidth.
    #[test]
    fn an_unsupported_count_steps_down_to_the_nearest_the_device_offers() {
        // The live hazard: a player types 8 on a device that stops at 4x. Stepping down is the
        // reference's own retry direction (`0x63b380`), so this is faithful, not just safe.
        let upto4 = [1, 2, 4];
        assert_eq!(clamp_to_supported(8, &upto4), 4);
        assert_eq!(clamp_to_supported(16, &upto4), 4);
        assert_eq!(clamp_to_supported(4, &upto4), 4);
        assert_eq!(clamp_to_supported(3, &upto4), 2);
        assert_eq!(clamp_to_supported(1, &upto4), 1);
    }

    #[test]
    fn a_supported_count_is_never_stepped_up_and_never_touched() {
        let all = [1, 2, 4, 8, 16];
        for n in all {
            assert_eq!(clamp_to_supported(n, &all), n, "{n}x should pass through");
        }
        // Between two offered levels it still rounds DOWN — the direction is the whole point.
        assert_eq!(clamp_to_supported(6, &all), 4);
        assert_eq!(clamp_to_supported(15, &all), 8);
    }

    #[test]
    fn a_device_that_offers_nothing_still_boots_at_one_sample() {
        // `supported_sample_counts` returning empty means some format refused even 1x, which should
        // be impossible — but the floor is 1 (no multisampling) rather than 0 or a panic, because a
        // client that will not start is strictly worse than one that starts unaliased.
        assert_eq!(clamp_to_supported(8, &[]), 1);
        assert_eq!(clamp_to_supported(1, &[]), 1);
        assert_eq!(MsaaSetting { samples: 1 }.level(), Msaa::Off);
    }

    /// [`MsaaFormats::clamp`] — the one expression of "the device has the final say" (1643),
    /// and both of its halves matter.
    ///
    /// A device list steps a request DOWN to something the GPU will actually take, which is what
    /// stops a saved `gxMultisample` from reaching the camera as a wgpu validation error. An
    /// EMPTY list is no opinion rather than a refusal: headless, and before the adapter exists,
    /// there is no device to be wrong about — clamping to 1 there would turn every device-less
    /// run into a silent "multisampling off", including the one this crate's own tests run in.
    #[test]
    fn the_device_list_steps_down_and_an_empty_one_has_no_opinion() {
        let apple = MsaaFormats {
            formats: vec![(32, 32, 1), (32, 32, 2), (32, 32, 4)],
        };
        assert_eq!(apple.clamp(4), 4, "a count it offers is untouched");
        assert_eq!(apple.clamp(3), 2, "down to the nearest offered, never up");
        assert_eq!(apple.clamp(8), 4, "the 2026-08-26 render-thread kill");
        assert_eq!(apple.clamp(16), 4);
        assert_eq!(apple.clamp(1), 1);

        let headless = MsaaFormats::default();
        assert!(headless.formats.is_empty());
        for n in [1, 2, 4, 8, 16] {
            assert_eq!(headless.clamp(n), n, "no adapter, no opinion");
        }
    }

    #[test]
    fn a_sample_count_rounds_down_never_up() {
        let level = |n| MsaaSetting { samples: n }.level();
        assert_eq!(level(1), Msaa::Off);
        assert_eq!(level(2), Msaa::Sample2);
        assert_eq!(level(3), Msaa::Sample2);
        assert_eq!(level(4), Msaa::Sample4);
        assert_eq!(level(7), Msaa::Sample4);
        assert_eq!(level(8), Msaa::Sample8);
        // The reference clamps at 16; anything that reaches here is already in range, and the top
        // of that range is still only eight samples of real hardware.
        assert_eq!(level(16), Msaa::Sample8);
        // 0 cannot arrive through the clamp, but the mapping must not panic if it ever does.
        assert_eq!(level(0), Msaa::Off);
    }
}
