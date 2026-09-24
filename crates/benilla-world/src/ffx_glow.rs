//! The faithful **FFXGlow** post pass — the reference's full-screen glow
//! (`FFXEffects.cpp` / `FFXGlow.bls`), replacing the Bevy-`Bloom` approximation and its two
//! eye-tuned constants.
//!
//! Pipeline (all byte-grounded — the shipped ARB programs):
//! scene → ½ → ¼ downsample (dims floored at 8, `0x6cdb40`) → separable Gauss4
//! (weights ⅛ ⅜ ⅜ ⅛, shipped constants) → `out = screen + w·blur²` in gamma bytes, `w` = the
//! per-zone `LightParams.glow` weight (authored data; the ONLY input, no knobs). The gamma-space
//! byte math and the square-law are in `shaders/ffx_glow.wgsl`.
//!
//! This is the frame's sole glow pass — it won the A/B against Bevy's `Bloom`, and that fallback
//! (plus its debug toggle) is gone. Applied to both the world camera and the portrait-booth bake
//! cameras (`portrait::mod`): the booth rides the same final node so its bake reads at exact world
//! parity, the FFXGlow combine owning the frame's ONE gamma decode.
//!
//! **What a bake does NOT inherit.** The node's two other lanes are keyed on the *viewer's own
//! state*, not on the scene — the drunk/underwater haze and the ghost's FFXDeath combine — and the
//! reference runs its FFX pass inside the WorldFrame's paint, with every UI frame compositing
//! afterwards. [`FfxGlow::state_scale`] is that whole class in one field, `0` on every bake.
//!
//! **Two nodes since decision 2234.** The world view's node runs the three filter passes and,
//! for a view nobody claims, the combine where [`crate::final_pass`] says. The player-UI camera
//! claims the world view ([`FfxBackdrop`]) and draws the combine itself, first in its own main
//! pass, straight into its byte target: the world enters the interface's buffer with no picture
//! in between — no full-window float image written by one camera and read back by the next.

use bevy::core_pipeline::core_2d::graph::{Core2d, Node2d};
use bevy::core_pipeline::core_2d::Transparent2d;
use bevy::core_pipeline::core_3d::graph::{Core3d, Node3d};
use bevy::core_pipeline::FullscreenShader;
use bevy::ecs::query::QueryItem;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_graph::{
    NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
};
use bevy::render::render_phase::{TrackedRenderPass, ViewSortedRenderPhases};
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer_sized};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue};
use bevy::render::sync_world::MainEntity;
use bevy::render::texture::{CachedTexture, TextureCache};
use bevy::render::view::{ExtractedView, ViewDepthTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

use crate::final_pass::FinalPassTarget;
use crate::view::WorldCamera;

/// Marks a camera rendering with the faithful FFXGlow pass (extracted to the render world).
///
/// The pass is TWO things in one: the frame's single gamma→linear decode (mandatory on every view
/// that draws our gamma-byte materials) and the glow add on top. `gain_scale`
/// separates them: it multiplies the zone's [`FfxGlowGain`] for this view, so `0.0` keeps the
/// decode and drops the glow.
#[derive(Component, Clone, Copy, ExtractComponent)]
pub struct FfxGlow {
    /// Per-view multiplier on the zone glow weight. See [`Self::WORLD`] / [`Self::UI_PANE`].
    pub(crate) gain_scale: f32,
    /// **Which FFX pass pair this view runs**, and therefore what drives the combine's `y` (the
    /// FFXDeath gate) and `z` (the haze mix) — see [`FfxState`].
    ///
    /// **The law, and why it is ONE field.** The reference runs its FFX pass inside the
    /// WorldFrame's own paint — a BEGIN/END pair, `0x6cd890`/`0x6cda70`, bracketed at
    /// `0x48350e`/`0x48379d` inside the one paint method `0x483460`. It **cannot reach a bake**,
    /// for reasons that need no frame ordering: the pass's
    /// render targets come from three module globals (`0xce8b5c` full, `0xce8ae8` quarter,
    /// `0xce8b98` backbuffer) and it never observes the ambient binding — while the reference's
    /// own portrait bake `0x524f60` binds no target at all and *copies* the framebuffer corner
    /// out (`0x58acd0`/`0x449bf0`), so the pixels it keeps are pre-pass in every ordering.
    ///
    /// The binary does not separate the two lanes: the haze is `primary.z` of the glow combine
    /// `0x6cb020`, which shares the single active-pass slot `0xce8bb4` with the death pass and
    /// runs through the same bracket into the same three targets. So they are **one class**, and
    /// this is one field. The zone glow is not in it (authored scene data, and a bake wants world
    /// parity for it), which is why [`Self::gain_scale`] stays separate.
    ///
    /// The haze had its own `haze_scale` and the death gate had none, so a released ghost's
    /// portraits baked through the FFXDeath combine and came back steel-blue luma (report B49,
    /// decision 1481). Naming the *class* rather than the member was the fix; naming the **pass
    /// pair** is the same fix one step further, and it is the reference's own
    /// shape — see [`FfxState`].
    pub(crate) state: FfxState,
}

/// **Which of the reference's FFX pass pairs a view runs.** The binary builds *two*, and the
/// active-pass slot `[0xce8bb4]` holds whichever the screen that is painting installed:
///
/// - the **WorldFrame** pair — `0x6cc130` CFFXGlow → `[0xb4b350]`, `0x6cc690` CFFXDeath →
///   `[0xb4b39c]`, built at `0x481c46`; selected by `0x5de9c0` off `PLAYER_FLAGS_GHOST`;
/// - the **glue** pair — `[0xb414c4]` glow / `[0xb41468]` death, built at CGlueMgr init
///   `0x46a723`/`0x46a752`; selected by the select build's tail `0x472fd9 test dh,0x20` off the
///   selected roster record's `CHARSELECT+0xfc & 0x2000`.
///
/// benilla's views coexist where the reference's screens take turns, so what the reference
/// expresses as one global slot written by whoever paints, we express as a property of the view.
/// That is *why* this is an enum and not a scale: a bake's [`Self::None`] cannot inherit a
/// player-state lane by arithmetic accident, which is the invariant decision 1481 legislated after
/// report B49, now structural.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FfxState {
    /// The **WorldFrame** pair — the live player's own state: the ghost death combine
    /// ([`FfxDeathFade`], off `PLAYER_FLAGS_GHOST`) and the drunk/underwater haze
    /// ([`FfxHazeMix`]). The true world view only.
    Player,
    /// The **glue** pair — the death combine iff the SELECTED ROSTER ROW is a ghost
    /// ([`GlueFfx::death`]), and no haze: a glue screen has no drunk player and no camera in a
    /// liquid, and the reference's glue pass pair carries no haze lane to read.
    Glue,
    /// Neither — a bake standing in for a 1.12 UI model widget, which the reference composites
    /// *after* the WorldFrame's own FFX bracket and which therefore sees no pass state at all.
    None,
}

impl FfxGlow {
    /// The reference's own full-screen glow, at the zone's authored weight — the world camera:
    /// zone glow AND the player-state passes (drunk / camera-eye-submerged blur, ghost death
    /// combine). The only view that carries the latter.
    pub const WORLD: Self = Self {
        gain_scale: 1.0,
        state: FfxState::Player,
    };
    /// The **glue screens' own** fullscreen render (login / create / character select) — the
    /// reference's glue pass pair, installed by CGlueMgr and applied around the glue scene's paint
    /// (`0x46fad3 call 0x6cd890` … `0x46fae0 jmp 0x6cda70`). It is not a bake standing in for a UI
    /// widget: it is the screen, so it runs the zone glow AND, for a ghost selection, the death
    /// combine — while the GlueXML frames over it composite afterwards, untinted, exactly as the
    /// reference's character list and buttons do.
    pub const GLUE_SCENE: Self = Self {
        gain_scale: 1.0,
        state: FfxState::Glue,
    };
    /// A portrait/booth bake at world parity for *lighting and glow* — but never
    /// a player-state pass: a bake stands in for a UI model widget, which the reference
    /// composites after the WorldFrame's FFX pass. So a drunk player's unit frame stays sharp
    /// while the world swims, and a **ghost's portrait keeps its living face** while the world
    /// goes steel-blue (report B49).
    pub const BOOTH: Self = Self {
        gain_scale: 1.0,
        state: FfxState::None,
    };
    /// **Decode only, no glow** — for a bake that stands in for a 1.12 *UI model widget*. The
    /// reference applies its FFX pass inside the WorldFrame's own paint (the `0x6cd890`/`0x6cda70`
    /// BEGIN/END bracket at `0x48350e`/`0x48379d`); every UI frame
    /// paints afterwards, at its own strata, so a `<PlayerModel>` pane is composited over an
    /// already-glowed world and never glows itself. (The give-away in-game: the reference's chat
    /// text and buttons don't bloom.)
    pub const UI_PANE: Self = Self {
        gain_scale: 0.0,
        state: FfxState::None,
    };
}

impl Default for FfxGlow {
    fn default() -> Self {
        Self::WORLD
    }
}

/// **A 2D camera whose ground is the world's FFX combine** — the player-UI
/// camera.
///
/// The world reaches the interface's byte buffer as the FIRST DRAW of this camera's main pass:
/// the combine of the view `source` names, rendered straight into this view's main texture in
/// the UI lane's terms — the gamma byte, no decode ([`FfxCombineKey::gamma_out`]) — before
/// anything the camera draws itself, inside the same pass ([`FfxTransparent2dNode`]). What that replaces is a picture: the combine used to write a full-window float
/// image that the UI pass then drew as its first quad, one write and one read of every pixel at
/// eight bytes each, for a value the quad re-encoded straight back to the byte the combine had
/// computed.
///
/// `source` is the world camera drawing this frame — its main-world entity — or `None` when
/// none is (the glue screens, the loading screen, a gated camera): the pass then does nothing
/// and the camera's own clear is the ground, exactly as the quad's absence was. The render world
/// resolves it to the view that actually rendered this frame ([`prepare_backdrops`]): a camera
/// that is active but was not extracted — its target's image not prepared yet, a spawn frame —
/// is not a source, so the pass never samples a main texture older than the frame. The component
/// itself stays on the camera so its pair of combine pipelines is specialised on the camera's
/// first frame, before any world exists to hitch on their compile.
///
/// The world view it names keeps its three filter passes — the ¼-res blur this combine samples
/// — and runs no combine of its own: nothing writes its camera's target, which is a size-carrier
/// only (`benilla_app::world_backdrop`). Its main texture is read unflipped, so a claimed camera
/// runs `CameraOutputMode::Skip`.
#[derive(Component, Clone, Copy, Default, ExtractComponent)]
pub struct FfxBackdrop {
    /// The world camera whose combine this camera runs — a main-world entity — or `None` while
    /// no world is drawing.
    pub source: Option<Entity>,
}

/// The per-zone glow weight `w` (`LightParams.glow` — authored data, synced from
/// [`crate::lighting::WowLighting`] every frame).
#[derive(Resource, Clone, ExtractResource)]
pub struct FfxGlowGain(pub f32);

/// The FFXDeath gate: `1.0` while the
/// player is a released ghost (`0x5de9c0`) — the combine swaps to the FFXDeath program whole —
/// else `0.0`.
/// INSTANT on both edges (the client has no time ramp; the ghost tint is a shader constant).
/// Driven by `benilla-app`'s death arc off `PLAYER_FLAGS_GHOST`; uploaded as the combine
/// uniform's `y`, scaled per view by [`FfxGlow::state_scale`] — it is a **player-state** pass and
/// reaches the world view only.
#[derive(Resource, Clone, Default, ExtractResource)]
pub struct FfxDeathFade(pub f32);

/// **The glue screens' FFX state** — the two things the select build's tail writes, and nothing
/// else (`0x472fba`–`0x473007`; the fork is
/// `472fd9 test dh,0x20` on the selected record's `CHARSELECT+0xfc`):
///
/// ```text
/// 472fde  mov ecx,ds:0xb41468 ; call 0x6cde60   ; ghost  → install the glue DEATH pass
/// 472fe9  mov edx,ds:0x838570 ; mov [esi+0x110],edx      ; …and pin LightParams.glow
/// 472ff7  mov ecx,ds:0xb414c4 ; call 0x6cde60   ; living → install the glue GLOW pass
/// 473002  mov eax,ds:0x838574 ; mov [esi+0x110],eax      ; …and pin LightParams.glow
/// ```
///
/// `[esi+0x110]` is the DN/lighting singleton's `LightParams.glow` (`esi` = `0x6d48b0()` =
/// `&0xce9b60`) — the same scalar `0x6cb930` reads for the death combine's **alpha byte**
/// (`glow × 255`) and the glow combine reads for its blur² weight. So on a glue
/// screen the glow weight is not a zone value at all: it is one of two constants, chosen by the
/// same bit that chooses the pass.
///
/// Written by the game's char-select feed; read by [`sync_gain`]'s off-world arm and by
/// [`FfxState::Glue`]'s combine.
#[derive(Resource, Clone, Copy, Default, PartialEq, Eq, Debug, ExtractResource)]
pub enum GlueFfx {
    /// **A glue ModelFFX widget is up and nothing has re-pinned it** — the login screen, the create
    /// screen, and a character-select screen with an empty account (where no select build runs).
    /// `CSimpleModelFFX`'s own **OnShow** override `0x46fa60` pins `[+0x110] = *0x8380b4 = 0.30f`
    /// and installs the glue GLOW pass (`0x46fa74`). OnShow, not per-frame: it tail-jumps to
    /// `0x76b260`, which runs the widget's `[frame+0x130]` handler (`0x76a0d0` maps `"OnShow"` to
    /// that slot), which is why the select build's own pin below is not overwritten every frame.
    #[default]
    Shown,
    /// **The character-select screen, LIVING selection.** The select build's tail installs the glue
    /// GLOW pass and pins `*0x838574 = 0.40f` (`0x472ff7`/`0x473002`). Note what this is *not*: a
    /// living selection is not "no post-process" — it is the glow pass at a pinned weight.
    SelectLiving,
    /// **The character-select screen, GHOST selection.** The glue DEATH pass, and `*0x838570 = 0.15f`
    /// (`0x472fde`/`0x472fe9`).
    SelectGhost,
}

impl GlueFfx {
    /// The glue pair's death gate — `1.0` only for [`Self::SelectGhost`]. Instant on both edges,
    /// like the world's: the reference's swap is a slot write with no time anchor in it, so
    /// clicking from a ghost row to a living one un-washes the scene on the same frame. And it runs
    /// on **every** selection, not only the first: `0x472a6d jne 0x472fba` sends an already-built
    /// record straight into the swap block.
    fn death(self) -> f32 {
        match self {
            Self::SelectGhost => 1.0,
            Self::Shown | Self::SelectLiving => 0.0,
        }
    }

    /// `LightParams.glow` while a glue screen is up — the DN/lighting singleton's `0xce9c70`
    /// (`0x6d48b0` is `mov eax,0xce9b60; ret`, so `[eax+0x110]` is that field).
    ///
    /// **All three values are byte-VERIFIED**, and an accessor-call-site census over all 41
    /// `call 0x6d48b0` sites found exactly these three writers and three readers (the ffx packs
    /// `0x6cb0e2`/`0x6cb557`/`0x6cb9e1`). Each constant has exactly one reference image-wide — the
    /// read itself — so they are literals, not `.data` defaults that something else moves.
    ///
    /// This is the scalar the death combine turns into its **primary alpha byte** (`0x6cb9ec`:
    /// `×255.0`, `+512.0`, `fstp`, `shr eax,0xe`) — 0.15 → 38, 0.40 → 102 — and which both combines
    /// use as the blur² weight. So a ghost's select screen glows at **less than half** a living
    /// one's, which is the opposite of what "the ghost look is the loud one" would suggest.
    ///
    /// (benilla keeps the float rather than the quantized byte: 38/255 = 0.14902 against 0.15 is
    /// under a thousandth, and the same scalar feeds the in-world glow lane where the reference
    /// carries a float too.)
    fn glow(self) -> f32 {
        match self {
            // `*0x8380b4`, the widget's OnShow pin.
            Self::Shown => 0.30,
            // `*0x838574`, the select build's living arm.
            Self::SelectLiving => 0.40,
            // `*0x838570`, the select build's ghost arm.
            Self::SelectGhost => 0.15,
        }
    }
}

/// The haze mix `z` — the combine's screen-toward-blur cross-fade
/// (`out = lerp(screen, blur, z) + w·blur²`, the shipped FFXGlow.bls). The reference's glow
/// render packs it per frame from the **active player's** state (`0x6cb134`/`0x6cb599`,
/// decision 1009 §A):
/// `z = max(min(drunkByte,100)/100, submerged ? 84/255 : 0)` — fully blurred at 100 inebriation,
/// and a fixed ≈0.329 floor whenever the **camera eye** is in any liquid (the vanilla underwater
/// blur; `0x672470`'s eye-liquid probe, `0xf` = dry). Synced by [`sync_haze`]; uploaded as the
/// combine uniform's `z`, scaled per view by [`FfxGlow::state_scale`].
#[derive(Resource, Clone, Default, ExtractResource)]
pub struct FfxHazeMix(pub f32);

/// The underwater haze floor: 84/255 (the reference's byte 84 in the COLOR z lane whenever the
/// eye-liquid probe reads non-dry — applied regardless of sobriety).
const HAZE_SUBMERGED_FLOOR: f32 = 84.0 / 255.0;

/// Sync [`FfxHazeMix`] from the two player-state inputs the reference's glow render reads each
/// frame: our own drunk byte (`PLAYER_BYTES_3` byte 1 → `min(b,100)/100`) and the camera-eye
/// submersion claim. Off-world both read empty → 0 (the glue screens never haze).
fn sync_haze(
    viewer: Res<crate::view::Viewer>,
    underwater: Option<Res<crate::liquid::Underwater>>,
    mut haze: ResMut<FfxHazeMix>,
) {
    let drunk = viewer.drunk;
    let submerged = if underwater.is_some_and(|u| u.0 != benilla_formats::Submersion::Dry) {
        HAZE_SUBMERGED_FLOOR
    } else {
        0.0
    };
    let target = drunk.max(submerged);
    if haze.0 != target {
        haze.0 = target;
    }
}

/// **The GlowWave lane** — the underwater screen warp's two inputs.
///
/// Underwater the reference swaps its whole post-process pass list: `CFFXGlow::Render 0x6cc630`
/// walks a second list whose third pass is **FFXGlowWave** (`0x6cb1f0`, render `0x6cb310`) rather
/// than FFXGlow, and that pass displaces the combine's two samples through a sine bump map. The two
/// arms are mutually exclusive and the plain-Glow arm is unreachable on both
/// backends, so a submerged frame is ALWAYS the wave.
///
/// **No liquid-type discrimination.** `[0xc7f288] ∈ {0xf dry, 0 water, 1 ocean, 2 magma, 3 slime}`
/// and `0x6cc644` compares against `0xf` alone — the warp runs in lava and slime exactly as in
/// water. (That is not the drift cloud's rule, which *does* fork per kind and gives slime no motes
/// at all; two neighbouring underwater systems reading the same byte with different questions.)
#[derive(Resource, Clone, Copy, Default, ExtractResource)]
pub struct FfxWave {
    /// `(t mod 3174)/3174` — the u-axis phase, 3.174 s.
    phase1: f32,
    /// `(t mod 2805)/2805` — the v-axis phase, 2.805 s. Independent of [`Self::phase1`]: the two
    /// rejoin only every ~49 minutes, which is what keeps the warp from reading as a loop.
    phase2: f32,
    /// Whether the pass-list swap itself is tripped: in-world (`0x467d00`) **and** the camera-eye
    /// liquid probe non-dry (`0x672470 != 0xf`). Not a strength — the swap is a hard fork, and the
    /// warp has no ramp in or out.
    active: bool,
}

/// The two GlowWave phase periods in milliseconds — `[0xce89c4]` and `[0xce89c8]`, divided into an
/// integer millisecond clock exactly as the reference's `fild`/`fidiv` pair does.
const WAVE_PERIOD_MS: [u64; 2] = [3174, 2805];

/// The wave LUT's edge — 128×128 texels (`[0xce89a0]`'s 7-field descriptor: 128/128/128/128 and
/// the two 1/128 reciprocals).
const WAVE_LUT_EDGE: u32 = 128;

/// The reference's glow-wave LUT (`ffx_glow_wave_lut` `0x6cbea0`, a diffed PRIMITIVE), as the
/// texels our combine samples.
///
/// Two channels, one sine each and each depending on ONE axis — `du = sin(2πx/128)` across,
/// `dv = sin(2πy/128)` down — which is what makes the sampled value a *displacement* rather than an
/// intensity: the shipped format is the signed two-channel bump format on both backends
/// (`D3DFMT_V8U8` / `GL_DSDT8_NV`), and the pass is a dependent texture read.
///
/// We store the reference's **unsigned** pack (`(s·0.5 + 0.5)·255`, its path B) and bias it back in
/// the shader, because that is the permutation the live client actually runs: `gxApi` defaults to
/// direct3d, the shipped `WTF/` overrides nothing, and the caps tier that selects picks the biased
/// `ps_2_0` blob against an unsigned texture. `as u8` truncates toward zero, which is `__ftol`.
fn wave_lut_texels() -> Vec<u8> {
    let pack = |s: f32| ((s * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8;
    let sine = |i: u32| (std::f32::consts::TAU * i as f32 / WAVE_LUT_EDGE as f32).sin();
    let mut texels = Vec::with_capacity((WAVE_LUT_EDGE * WAVE_LUT_EDGE * 2) as usize);
    for y in 0..WAVE_LUT_EDGE {
        let dv = pack(sine(y));
        for x in 0..WAVE_LUT_EDGE {
            texels.push(pack(sine(x)));
            texels.push(dv);
        }
    }
    texels
}

/// Sync [`FfxWave`] from the same two inputs the reference's swap guard reads, plus the clock its
/// render method phases on (`OsGetAsyncTimeMs`, `0x6cb43a`).
///
/// The phases advance whether or not the wave is armed — they are a free-running wall clock in the
/// reference too, not a timer the swap starts — so surfacing and diving again does not restart the
/// pattern, and nothing has to be reset on the transition.
fn sync_wave(
    time: Res<Time>,
    live: Res<crate::schedule::WorldLive>,
    underwater: Option<Res<crate::liquid::Underwater>>,
    mut wave: ResMut<FfxWave>,
    mut last_dump: Local<Option<u32>>,
) {
    let ms = (time.elapsed_secs_f64() * 1000.0) as u64;
    wave.phase1 = (ms % WAVE_PERIOD_MS[0]) as f32 / WAVE_PERIOD_MS[0] as f32;
    wave.phase2 = (ms % WAVE_PERIOD_MS[1]) as f32 / WAVE_PERIOD_MS[1] as f32;
    let verdict = underwater.map(|u| u.0);
    wave.active = live.0 && verdict.is_some_and(|v| v.any());

    // `WOW_WAVE_DUMP` — 1 Hz, and it reports on EVERY frame including the ones that do not warp,
    // naming why. An instrument that only speaks while the effect is running cannot tell "not
    // armed" from "armed and broken", which is exactly the hour the drift cloud's first probe cost:
    // the effect was off, the screen was silent, and the silence was
    // indistinguishable from a compile failure.
    if std::env::var_os("WOW_WAVE_DUMP").is_some() {
        let sec = time.elapsed_secs() as u32;
        if last_dump.replace(sec) != Some(sec) {
            let why = match (live.0, verdict) {
                (false, _) => "off — not in world (the reference's 0x467d00 half of the guard)",
                (_, None) => "off — no submersion verdict resolved yet",
                (_, Some(benilla_formats::Submersion::Dry)) => "off — the eye is dry",
                (_, Some(_)) => "WARPING",
            };
            info!(
                "glow-wave: {why} — verdict {verdict:?} phase {:.3}/{:.3} (periods {} / {} ms)",
                wave.phase1, wave.phase2, WAVE_PERIOD_MS[0], WAVE_PERIOD_MS[1]
            );
        }
    }
}

/// Whether a view draws the warped combine — the pass-list swap, as a pure function.
///
/// Three conditions, each a fact about the reference rather than a taste call:
/// - the view runs the **WorldFrame** pass pair. The swap lives in `CFFXGlow::Render`, which the
///   glue screens' own pair also reaches — but its guard's first half is `0x467d00`, the in-world
///   objmgr gate, so a glue screen can never trip it. A bake runs no pair at all.
/// - the camera eye is in liquid and we are in-world ([`FfxWave::active`]).
/// - the player is **not a ghost**. `CFFXDeath` REPLACES `CFFXGlow` outright in the single active
///   pass slot `[0xce8bb4]`, and `CFFXDeath::Render 0x6cdf20` owns one list and no guard — so a
///   ghost underwater gets no warp, the same way it already gets no haze.
fn wave_armed(state: FfxState, wave: FfxWave, death: f32) -> bool {
    matches!(state, FfxState::Player) && wave.active && death == 0.0
}

/// Sync the gain from the live zone lighting (the same source `sync_bloom` used). Off-world there
/// is no `Light.dbc` zone — the reference runs its `LightParams` **default 0.5** (the
/// zone/time-of-day ambient glow scalar's own default): the glue screens'
/// soft glow. (Before this, the glue rendered with the derive-default 0.0 — no glow at all.)
fn sync_gain(
    lighting: Option<Res<crate::lighting::WowLighting>>,
    live: Res<crate::schedule::WorldLive>,
    glue: Res<GlueFfx>,
    mut gain: ResMut<FfxGlowGain>,
) {
    let target = if live.0 {
        lighting.map_or(0.5, |l| l.glow)
    } else {
        // Off-world the glue screen pins it outright — the widget's OnShow, or the select build's
        // own fork ([`GlueFfx::glow`], all three byte-verified). It is never `LightParams`' default
        // here: this arm used to read a flat 0.5 on the reasoning that an off-world client falls
        // back to the table default, and the bytes say the glue screens pin it instead.
        glue.glow()
    };
    if gain.0 != target {
        gain.0 = target;
    }
}

/// The FFXGlow pass is MANDATORY on the world camera in the gamma lane: its
/// combine owns the frame's single gamma→linear decode — without it the whole frame presents
/// over-bright. Insert on any world camera that lacks it (idempotent; spawn sites also add it).
fn ensure_ffx_glow(
    mut commands: Commands,
    cam: Query<Entity, (With<WorldCamera>, Without<FfxGlow>)>,
    mut with: Query<&mut FfxGlow>,
) {
    // Perf-bisect kill-switch: $WOW_NO_FFX strips the GLOW from every camera — the three filter
    // passes and the blur term, [`FfxGlow::UI_PANE`]'s shape — and keeps the combine, because in
    // the gamma lane the combine is not an effect: it is the frame's one decode on a `Write`
    // view and the UI camera's ground pass on a claimed one, and a frame
    // without it has no world in it. The frame shows the world un-glowed at its right
    // brightness; what the lever prices is the blur chain and the glow add.
    static NO_FFX: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *NO_FFX.get_or_init(|| std::env::var_os("WOW_NO_FFX").is_some()) {
        for mut glow in &mut with {
            if glow.gain_scale != 0.0 || glow.state != FfxState::None {
                *glow = FfxGlow::UI_PANE;
            }
        }
        for e in &cam {
            commands.entity(e).insert(FfxGlow::UI_PANE);
        }
        return;
    }
    for e in &cam {
        commands.entity(e).insert(FfxGlow::WORLD);
    }
}

// ---------------------------------------------------------------- render world

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct FfxGlowLabel;

/// Layouts, sampler, and the four cached pipelines (downsample ×2 share one).
#[derive(Resource)]
struct FfxGlowPipelines {
    layout_filter: BindGroupLayoutDescriptor,
    layout_combine: BindGroupLayoutDescriptor,
    sampler: Sampler,
    /// The 128×128 sine bump map and its own REPEAT sampler — built once here, not per view: the
    /// LUT is 32 KB of resolution-independent constant, and the reference generates it once too
    /// (the registration callback `0x6cbcf0`).
    ///
    /// REPEAT is load-bearing, not a default. The wave texcoord's scale runs to `W/128` — ten full
    /// cycles across a 1280-wide screen — so under CLAMP every cycle but the first would pin to the
    /// edge texel and the warp would vanish over ~90% of the frame. It is byte-closed to
    /// `D3DTADDRESS_WRAP` (`0x5a2646`/`0x5a266a` through the table `0x80a254 = {CLAMP, WRAP}`).
    wave_view: TextureView,
    wave_sampler: Sampler,
    downsample: CachedRenderPipelineId,
    gauss_h: CachedRenderPipelineId,
    gauss_v: CachedRenderPipelineId,
    combine: FfxCombinePipeline,
}

/// The combine — specialised on the **format it renders in** ([`crate::final_pass`]):
/// a view whose camera runs `CameraOutputMode::Skip` gets it rendered straight into the output
/// texture (a bake's image), a `Write` view into the HDR main texture for bevy's blit to copy
/// out, as before — and the world view's combine is the UI camera's own pass, keyed on the UI
/// target ([`FfxBackdrop`]). The three filter passes never leave the ¼-res chain
/// and stay unspecialised.
struct FfxCombinePipeline {
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
    fullscreen: FullscreenShader,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FfxCombineKey {
    format: TextureFormat,
    /// The combine as a **gamma-lane** pass: it stores the gamma byte it computed
    /// and leaves the frame's one decode to the lane that owns it — the UI camera's, at its end,
    /// for a view this combine grounds ([`FfxBackdrop`]). `false` is the world lane's own exit,
    /// the decode here.
    gamma_out: bool,
    /// The combine drawn INSIDE a 2D main pass — the first draw of a backdrop camera's
    /// transparent pass — which carries the `Core2d` depth attachment and the view's sample
    /// count, and a pipeline in that pass must say so (wgpu validates the pair). `None` is the
    /// combine's own pass: colour only, one sample.
    inside_2d: Option<u32>,
    /// The underwater combine (`fs_combine_wave`). A SEPARATE pipeline rather than a branch inside
    /// `fs_combine`: a dry frame then binds byte-for-byte the pipeline it bound before the warp
    /// existed and pays nothing at all for it — no extra pass, no extra target, not even a uniform
    /// branch.
    wave: bool,
}

impl SpecializedRenderPipeline for FfxCombinePipeline {
    type Key = FfxCombineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        let (label, entry) = if key.wave {
            ("ffx_glow_combine_wave", "fs_combine_wave")
        } else {
            ("ffx_glow_combine", "fs_combine")
        };
        RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: if key.gamma_out {
                    vec!["GAMMA_OUT".into()]
                } else {
                    vec![]
                },
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState {
                    format: key.format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            // Inside a 2D main pass the depth attachment is bound but never consulted: the
            // ground writes every pixel under everything, and the phase's own items decide
            // their order among themselves as they always did.
            depth_stencil: key.inside_2d.map(|_| DepthStencilState {
                format: bevy::core_pipeline::core_2d::CORE_2D_DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: CompareFunction::Always,
                stencil: StencilState::default(),
                bias: DepthBiasState::default(),
            }),
            multisample: MultisampleState {
                count: key.inside_2d.unwrap_or(1),
                ..default()
            },
            ..default()
        }
    }
}

fn init_pipelines(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    fullscreen_shader: Res<FullscreenShader>,
    asset_server: Res<AssetServer>,
    pipeline_cache: Res<PipelineCache>,
) {
    let shader: Handle<Shader> =
        asset_server.load("embedded://benilla_world/shaders/ffx_glow.wgsl");
    // Filter passes bind (tex, sampler); the combine additionally binds (blur tex, gain uniform).
    let layout_filter = BindGroupLayoutDescriptor::new(
        "ffx_glow_filter_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let layout_combine = BindGroupLayoutDescriptor::new(
        "ffx_glow_combine_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_2d(TextureSampleType::Float { filterable: true }),
                uniform_buffer_sized(false, Some(std::num::NonZero::new(32).unwrap())),
                // The wave LUT + its REPEAT sampler ride EVERY combine's layout, dry included, so
                // the two entry points stay interchangeable behind one bind group. Binding a
                // texture the dry shader never samples costs the frame nothing.
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let sampler = render_device.create_sampler(&SamplerDescriptor {
        min_filter: FilterMode::Linear,
        mag_filter: FilterMode::Linear,
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        ..Default::default()
    });
    // LINEAR + REPEAT, both byte-closed: the reference's 128-texel sine is *sampled*, so linear
    // filtering is what makes it a smooth wave rather than 128 steps, and the wrap is what lets the
    // texcoord run past 1.0 into its tenth cycle.
    let wave_sampler = render_device.create_sampler(&SamplerDescriptor {
        min_filter: FilterMode::Linear,
        mag_filter: FilterMode::Linear,
        address_mode_u: AddressMode::Repeat,
        address_mode_v: AddressMode::Repeat,
        ..Default::default()
    });
    let wave_view = render_device
        .create_texture_with_data(
            &render_queue,
            &TextureDescriptor {
                label: Some("ffx_glow_wave_lut"),
                size: Extent3d {
                    width: WAVE_LUT_EDGE,
                    height: WAVE_LUT_EDGE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                // Two unsigned channels, biased back to signed in the shader — the shipped
                // permutation's own encoding (see [`wave_lut_texels`]).
                format: TextureFormat::Rg8Unorm,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            },
            TextureDataOrder::LayerMajor,
            &wave_lut_texels(),
        )
        .create_view(&TextureViewDescriptor::default());
    let pipeline = |label: &'static str,
                    layout: &BindGroupLayoutDescriptor,
                    entry: &'static str|
     -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout.clone()],
            vertex: fullscreen_shader.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: shader.clone(),
                shader_defs: vec![],
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState {
                    format: ViewTarget::TEXTURE_FORMAT_HDR,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            ..default()
        }
    };
    let downsample = pipeline_cache.queue_render_pipeline(pipeline(
        "ffx_glow_downsample",
        &layout_filter,
        "fs_downsample",
    ));
    let gauss_h =
        pipeline_cache.queue_render_pipeline(pipeline("ffx_glow_h", &layout_filter, "fs_gauss_h"));
    let gauss_v =
        pipeline_cache.queue_render_pipeline(pipeline("ffx_glow_v", &layout_filter, "fs_gauss_v"));
    let combine = FfxCombinePipeline {
        layout: layout_combine.clone(),
        shader,
        fullscreen: fullscreen_shader.clone(),
    };
    commands.insert_resource(FfxGlowPipelines {
        layout_filter,
        layout_combine,
        sampler,
        wave_view,
        wave_sampler,
        downsample,
        gauss_h,
        gauss_v,
        combine,
    });
}

/// The two ¼-res ping-pong targets (the reference downsamples full→¼ in ONE Box4 pass,
/// `0x6ca9d0`; a ½ intermediate would be one downsample too many), plus the
/// GPU objects derived from them, so the node does not recreate them per frame.
#[derive(Component)]
struct FfxGlowTextures {
    quarter_a: CachedTexture,
    quarter_b: CachedTexture,
    /// The combine's 32-byte uniform — `(gain, death, haze, dither)` then the GlowWave phases.
    /// Persistent, rewritten each frame with a queue write (queue writes land before the graph's
    /// submit executes).
    gain_buf: Buffer,
    /// The two Gauss bind groups bind only the ¼-res ping-pong views + sampler — all stable
    /// while the textures hold. The downsample and combine bind groups bind the view's finished
    /// main texture, which `ViewTarget::post_process_write` flips per call on a `Write` view, so
    /// those two are not cached (they would sample last frame's texture) and stay per-frame in
    /// `run`.
    gauss_h_bind: BindGroup,
    gauss_v_bind: BindGroup,
    /// The combine pipelines for this view, specialised on the format the combine renders in
    /// for THIS camera's output mode (`FinalPassTarget::format`) — or `None` for a view some
    /// [`FfxBackdrop`] claims, whose combine is that camera's own pass and not this node's.
    combine: Option<FfxCombinePair>,
}

/// One view's combine pipelines — dry and underwater (`fs_combine_wave`) — and the format they
/// were keyed on: the freshness gate beside the two textures'.
#[derive(Clone, Copy)]
struct FfxCombinePair {
    dry: CachedRenderPipelineId,
    wave: CachedRenderPipelineId,
    format: TextureFormat,
    /// The sample count the pair was keyed on — 1 for a combine's own pass, the view's for a
    /// backdrop draw inside a 2D main pass.
    samples: u32,
}

impl FfxCombinePair {
    /// The pipeline the pass-list swap selects: the underwater entry while the wave is armed.
    fn armed(&self, wave: bool) -> CachedRenderPipelineId {
        if wave {
            self.wave
        } else {
            self.dry
        }
    }
}

/// What specialising a combine pair takes — the one hand the two prepare systems reach for it
/// with, so neither carries the three resources separately.
#[derive(bevy::ecs::system::SystemParam)]
struct CombineSpecializer<'w> {
    pipeline_cache: Res<'w, PipelineCache>,
    pipelines: Res<'w, FfxGlowPipelines>,
    cache: ResMut<'w, SpecializedRenderPipelines<FfxCombinePipeline>>,
}

impl CombineSpecializer<'_> {
    /// The combine's own pass: colour only, the decode at its exit.
    fn pair(&mut self, format: TextureFormat) -> FfxCombinePair {
        self.pair_for(format, false, None)
    }

    /// The backdrop draw inside a 2D main pass of `samples` samples: the gamma-lane exit, the
    /// pass's depth attachment declared.
    fn backdrop_pair(&mut self, format: TextureFormat, samples: u32) -> FfxCombinePair {
        self.pair_for(format, true, Some(samples))
    }

    fn pair_for(
        &mut self,
        format: TextureFormat,
        gamma_out: bool,
        inside_2d: Option<u32>,
    ) -> FfxCombinePair {
        let mut key = |wave| {
            self.cache.specialize(
                &self.pipeline_cache,
                &self.pipelines.combine,
                FfxCombineKey {
                    format,
                    wave,
                    gamma_out,
                    inside_2d,
                },
            )
        };
        FfxCombinePair {
            dry: key(false),
            wave: key(true),
            format,
            samples: inside_2d.unwrap_or(1),
        }
    }
}

fn prepare_textures(
    mut commands: Commands,
    mut texture_cache: ResMut<TextureCache>,
    render_device: Res<RenderDevice>,
    mut specializer: CombineSpecializer,
    claims: Res<FfxBackdropClaims>,
    views: Query<
        (
            Entity,
            &ExtractedCamera,
            &ViewTarget,
            Option<&FfxGlowTextures>,
        ),
        With<FfxGlow>,
    >,
) {
    for (entity, camera, target, existing) in &views {
        let Some(vp) = camera.physical_viewport_size else {
            continue;
        };
        // The view's OWN combine pair, specialised **whether or not this view is claimed**.
        // A claimed view (some [`FfxBackdrop`] names it) has no combine of its
        // own — the UI camera's ground pass is it, keyed on THAT camera's target — so it carries
        // none rather than a pair keyed on a target nothing writes. But the claim is not a
        // property of the world, it is a property of *this frame*: it drops the moment the UI
        // camera loses its `ViewTarget`, which is what `prepare_view_targets` does as soon as the
        // window's surface goes away. At app exit that is harmless (bevy runs one to three more
        // updates after the last presented frame — see `benilla_app::shutdown`) and it is exactly
        // what the director's 2026-09-15 log caught: two `pipeline compiled LIVE` lines in the
        // same millisecond as "No windows are open, exiting". A minimize to zero size or a surface
        // reconfigure reaches the same branch **while the player is looking at the frame**, and
        // there the pair would be two synchronous Metal compiles on the render thread. Specialising
        // is a cached lookup on a four-field key, so holding the pair warm from the first covered
        // frame costs that lookup and nothing else.
        let own = specializer.pair(FinalPassTarget::format(&camera.output_mode, target));
        let combine = (!claims.0.contains(&entity)).then_some(own);
        // The reference's RT-dim chain: ½ and ¼, floored (clamp ≥8 — `0x6cdb40`).
        let mut tex = |label: &'static str, w: u32, h: u32| {
            texture_cache.get(
                &render_device,
                TextureDescriptor {
                    label: Some(label),
                    size: Extent3d {
                        width: w.max(8),
                        height: h.max(8),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: ViewTarget::TEXTURE_FORMAT_HDR,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let quarter_a = tex("ffx_glow_quarter_a", vp.x / 4, vp.y / 4);
        let quarter_b = tex("ffx_glow_quarter_b", vp.x / 4, vp.y / 4);
        // `TextureCache::get` hands the same textures back while the viewport holds, and the
        // derived objects only depend on them — rebuilding the component anyway would be right
        // back to per-frame bind-group creation.
        if existing.is_some_and(|t| {
            t.quarter_a.texture.id() == quarter_a.texture.id()
                && t.quarter_b.texture.id() == quarter_b.texture.id()
                && t.combine.as_ref().map(|c| c.format) == combine.as_ref().map(|c| c.format)
        }) {
            continue;
        }
        let pipelines = &specializer.pipelines;
        let layout_filter = specializer
            .pipeline_cache
            .get_bind_group_layout(&pipelines.layout_filter);
        let gauss_h_bind = render_device.create_bind_group(
            "ffx_glow_gauss_h",
            &layout_filter,
            &BindGroupEntries::sequential((&quarter_a.default_view, &pipelines.sampler)),
        );
        let gauss_v_bind = render_device.create_bind_group(
            "ffx_glow_gauss_v",
            &layout_filter,
            &BindGroupEntries::sequential((&quarter_b.default_view, &pipelines.sampler)),
        );
        let gain_buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("ffx_glow_gain"),
            size: 32,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        commands.entity(entity).insert(FfxGlowTextures {
            quarter_a,
            quarter_b,
            gain_buf,
            gauss_h_bind,
            gauss_v_bind,
            combine,
        });
    }
}

/// `WOW_DITHER=1` — arm the combine's deband dither (the shader's `glow.w`).
///
/// The frame's ONE quantization to 8 bits is the surface's sRGB present-encode of the combine's
/// gamma-space output, so a smooth surface steps in 1/255. Measured on a lit bare arm the shading
/// gradient is ~1.3 levels/px; a breathing idle drifts the body ~0.11 px/frame; so a pixel needs
/// ~7 FRAMES to cross one step and the shading updates at ~8 Hz under a 60 Hz frame rate. Motion
/// large enough to clear a level every frame hides it entirely — the reported
/// small-moves-tick / big-moves-smooth split.
///
/// **Off by default because it is a divergence.** The reference's framebuffer was 8-bit and
/// undithered; this lane is byte-exact against it and dithering trades that for a smoother
/// gradient. Bevy would normally apply its own in the tonemapping pass, but `Tonemapping::None`
/// makes that node return immediately, so the `DebandDither::Enabled` our camera inherits from
/// `Camera3d` never runs — this is the only place it can live.
fn dither_armed() -> f32 {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    match *ON.get_or_init(|| std::env::var_os("WOW_DITHER").is_some()) {
        true => 1.0,
        false => 0.0,
    }
}

/// The combine's `(x, y, z, w)` uniform for one view — the whole per-view scaling law in one
/// pure function, so it can be tested without a render world.
///
/// - **x** — the zone glow weight, scaled by [`FfxGlow::gain_scale`]: authored *scene* data, so a
///   portrait bake carries it at world parity and only a UI model pane drops it,
///   keeping the combine for its gamma decode alone.
/// - **y/z** — the FFXDeath gate and the haze mix, both selected by [`FfxGlow::state`]: which pass
///   pair this view runs decides what drives them, and a bake runs neither pair (
///   report B49 — now structural rather than arithmetic).
/// - **w** — the deband-dither arm ([`dither_armed`]), 0 or 1.
///
/// The second row is the GlowWave lane: the two phases, written on every frame and read only by
/// the warped entry point. They are unconditional because they are a free-running clock — gating
/// the *write* on the arm would make the pattern restart on every dive.
fn combine_uniform(
    zone_gain: f32,
    world: FfxPassState,
    glue: FfxPassState,
    glow: &FfxGlow,
    wave: FfxWave,
) -> [f32; 8] {
    let feed = match glow.state {
        FfxState::Player => world,
        FfxState::Glue => glue,
        FfxState::None => FfxPassState::INERT,
    };
    [
        zone_gain * glow.gain_scale,
        feed.death,
        feed.haze,
        dither_armed(),
        wave.phase1,
        wave.phase2,
        0.0,
        0.0,
    ]
}

/// One pass pair's live state, as [`combine_uniform`] consumes it: the death gate and the haze mix
/// that pair carries. The glue pair has no haze lane at all, which is a fact about the reference
/// and not a value we happen to leave at zero.
#[derive(Clone, Copy)]
struct FfxPassState {
    death: f32,
    haze: f32,
}

impl FfxPassState {
    /// The **WorldFrame** pair's live state: the ghost gate off `PLAYER_FLAGS_GHOST` and the
    /// drunk/underwater haze, both of the live player.
    fn world(death: f32, haze: f32) -> Self {
        Self { death, haze }
    }

    /// The **glue** pair's live state. Death only, and the constructor is where that is enforced:
    /// the reference's haze is `primary.z` of the *WorldFrame* glow combine `0x6cb020`, packed from
    /// the active player's inebriation and the camera-eye liquid probe — a glue
    /// screen has neither, and CGlueMgr's pair has no lane to read them into. Passing a haze here
    /// should be impossible rather than merely wrong.
    fn glue(death: f32) -> Self {
        Self { death, haze: 0.0 }
    }

    /// What a view running neither pair reads — a bake.
    const INERT: Self = Self {
        death: 0.0,
        haze: 0.0,
    };
}

#[derive(Default)]
struct FfxGlowNode;

impl ViewNode for FfxGlowNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static FfxGlowTextures,
        &'static FfxGlow,
        &'static ExtractedCamera,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        (view_target, textures, glow, camera): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let pipelines = world.resource::<FfxGlowPipelines>();
        let pipeline_cache = world.resource::<PipelineCache>();
        let (uniform, wave) = live_combine(world, glow);
        let (Some(downsample), Some(gauss_h), Some(gauss_v)) = (
            pipeline_cache.get_render_pipeline(pipelines.downsample),
            pipeline_cache.get_render_pipeline(pipelines.gauss_h),
            pipeline_cache.get_render_pipeline(pipelines.gauss_v),
        ) else {
            return Ok(()); // pipelines still compiling — draw the frame un-glowed
        };
        // Where this view's combine lands (2206, `final_pass`): a `Skip` camera's output texture
        // itself, a `Write` camera's ping-pong for bevy's blit to copy out. A CLAIMED view
        // ([`FfxBackdrop`]) has no combine here at all: the UI camera's ground
        // pass is its combine, and samples this view's finished main texture — unflipped, which
        // is why a claimed camera runs `Skip` — once the filter passes below have built the
        // blur it reads beside it.
        let out = match textures.combine.as_ref() {
            None => None,
            Some(pair) => {
                // THE PASS-LIST SWAP (`0x6cc630`). The reference forks the whole list here; we
                // fork one pipeline, which is the same fork — the first two passes are identical
                // in both lists.
                let Some(combine) = pipeline_cache.get_render_pipeline(pair.armed(wave)) else {
                    return Ok(());
                };
                Some((combine, FinalPassTarget::resolve(camera, view_target)))
            }
        };
        let source = out
            .as_ref()
            .map_or(view_target.main_texture_view(), |(_, out)| out.source);
        let render_device = render_context.render_device().clone();
        let diagnostics = render_context.diagnostic_recorder();

        // The combine reads the blur through exactly two terms — `w·blur²` (`lane.x`) and the
        // haze cross-fade (`lane.z`; the ghost combine has no haze and reads the blur through
        // the same `x`). With both exactly zero the three filter passes would compute a texture
        // the combine multiplies by nothing, so they are skipped and the combine samples what
        // the ¼-res target holds — finite bytes from an earlier frame, or wgpu's zero-init — × 0.
        // Every UI-pane view (`gain_scale` 0, no haze lane) skips on every frame; the world view
        // skips in a zone whose `LightParams.glow` is 0 while the eye is dry and sober. The same
        // shader then runs with the same uniform: look-neutral by construction, and the
        // journal's `gpu_glow` column reads 0 for a view that skipped (2008).
        let blur_read = uniform[0] != 0.0 || uniform[2] != 0.0;
        if blur_read {
            let layout_filter = pipeline_cache.get_bind_group_layout(&pipelines.layout_filter);
            // The downsample binds the finished main texture, which a `Write` view's
            // `post_process_write` flips per call — this bind group is not cached (it would
            // sample last frame's texture); the two Gauss ones bind only the stable ¼-res
            // ping-pong and ride prepared on [`FfxGlowTextures`].
            let down_bind = render_device.create_bind_group(
                "ffx_glow_down_quarter",
                &layout_filter,
                &BindGroupEntries::sequential((source, &pipelines.sampler)),
            );

            // The filter passes (byte-pinned chain): source→¼ (one Box4), ¼a→¼b (H), ¼b→¼a (V).
            // Each pass opens its own diagnostic span (`render/ffx_glow_*/elapsed_gpu` on a
            // device that times passes): the journal's `gpu_glow` column is their sum (2008).
            let filter_passes: [(&'static str, &RenderPipeline, &BindGroup, &TextureView); 3] = [
                (
                    "ffx_glow_down_quarter",
                    downsample,
                    &down_bind,
                    &textures.quarter_a.default_view,
                ),
                (
                    "ffx_glow_gauss_h",
                    gauss_h,
                    &textures.gauss_h_bind,
                    &textures.quarter_b.default_view,
                ),
                (
                    "ffx_glow_gauss_v",
                    gauss_v,
                    &textures.gauss_v_bind,
                    &textures.quarter_a.default_view,
                ),
            ];
            for (label, pipeline, bind, dst) in filter_passes {
                let mut pass =
                    render_context
                        .command_encoder()
                        .begin_render_pass(&RenderPassDescriptor {
                            label: Some(label),
                            color_attachments: &[Some(RenderPassColorAttachment {
                                view: dst,
                                depth_slice: None,
                                resolve_target: None,
                                ops: Operations::default(),
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        });
                let span = diagnostics.pass_span(&mut pass, label);
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..3, 0..1);
                span.end(&mut pass);
            }
        }

        let Some((combine, out)) = out else {
            return Ok(()); // claimed: the combine is the UI camera's pass
        };
        // The uniform is the combine's alone (the filter shaders never bind it), written by
        // whichever node runs the combine — this one here, the backdrop node for a claimed view.
        world.resource::<RenderQueue>().write_buffer(
            &textures.gain_buf,
            0,
            bytemuck::cast_slice(&uniform),
        );
        // Combine: screen + w·blur² (gamma-space byte math in the shader) → the view's output.
        // Also binds the flipping `out.source` — per-frame for the downsample's reason.
        let layout_combine = pipeline_cache.get_bind_group_layout(&pipelines.layout_combine);
        let bind = render_device.create_bind_group(
            "ffx_glow_combine",
            &layout_combine,
            &BindGroupEntries::sequential((
                out.source,
                &pipelines.sampler,
                &textures.quarter_a.default_view,
                textures.gain_buf.as_entire_binding(),
                &pipelines.wave_view,
                &pipelines.wave_sampler,
            )),
        );
        let scissor = out.scissor_rect();
        let mut pass = render_context
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("ffx_glow_combine"),
                color_attachments: &[Some(out.destination)],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        if let Some((x, y, w, h)) = scissor {
            pass.set_scissor_rect(x, y, w, h);
        }
        let span = diagnostics.pass_span(&mut pass, "ffx_glow_combine");
        pass.set_pipeline(combine);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
        span.end(&mut pass);
        Ok(())
    }
}

/// The live inputs of one view's combine, read off the render world: its uniform, and whether
/// the underwater entry is armed. One reading for the two nodes that can run a combine.
fn live_combine(world: &World, glow: &FfxGlow) -> ([f32; 8], bool) {
    let death = world.resource::<FfxDeathFade>().0;
    let wave = *world.resource::<FfxWave>();
    let uniform = combine_uniform(
        world.resource::<FfxGlowGain>().0,
        FfxPassState::world(death, world.resource::<FfxHazeMix>().0),
        FfxPassState::glue(world.resource::<GlueFfx>().death()),
        glow,
        wave,
    );
    (uniform, wave_armed(glow.state, wave, death))
}

/// Every view an [`FfxBackdrop`] names this frame: the render-world entities whose combine is
/// somebody else's pass. Rebuilt in prepare, read by [`prepare_textures`] (no combine pair for a
/// claimed view) and by the world node (no combine pass).
#[derive(Resource, Default)]
struct FfxBackdropClaims(Vec<Entity>);

/// A backdrop camera's view, as the render world resolved it this frame: its own combine pair —
/// specialised on ITS main texture's format with the gamma-lane exit
/// ([`FfxCombineKey::gamma_out`]), where a world view's pair is keyed on the world's target and
/// decodes — and the world view it grounds on, a render-world entity whose `ViewTarget` was
/// prepared THIS frame, or `None`.
#[derive(Component)]
struct FfxBackdropView {
    pair: FfxCombinePair,
    source: Option<Entity>,
}

/// The world views bevy prepared a `ViewTarget` for this frame — the ones that rendered — by
/// their main-world entity.
type RenderedWorldViews<'w, 's> =
    Query<'w, 's, (Entity, &'static MainEntity), (With<FfxGlow>, Changed<ViewTarget>)>;

/// Resolve every [`FfxBackdrop`]: the claims, and each claiming view's pair and source.
///
/// The source is looked up among the views whose `ViewTarget` bevy prepared this frame
/// (`Changed<ViewTarget>`: `prepare_view_targets` re-inserts it for every view it extracted),
/// by the main-world entity the camera named. A world camera that is active in the main world
/// but was not rendered this frame — its target's image not yet prepared, the frame it spawned
/// — resolves to nothing, so the backdrop pass never samples a main texture older than the
/// frame; and a world view is claimed from its very first rendered frame, so it never builds a
/// combine pair it would drop a frame later.
///
/// The pair is keyed on the view's main texture, which exists from the view's first frame — so
/// a player-UI camera spawned before the world has its pipelines compiling before there is a
/// world to hitch on them (`pipe_warm`'s pre-world cover), whether or not it has a source yet.
fn prepare_backdrops(
    mut commands: Commands,
    mut claims: ResMut<FfxBackdropClaims>,
    mut specializer: CombineSpecializer,
    views: Query<(
        Entity,
        &FfxBackdrop,
        &ViewTarget,
        &Msaa,
        Option<&FfxBackdropView>,
    )>,
    rendered: RenderedWorldViews,
) {
    claims.0.clear();
    for (entity, backdrop, target, msaa, existing) in &views {
        let source = backdrop.source.and_then(|main| {
            rendered
                .iter()
                .find(|(_, m)| m.id() == main)
                .map(|(view, _)| view)
        });
        claims.0.extend(source);
        let (format, samples) = (target.main_texture_format(), msaa.samples());
        let fresh =
            |view: &FfxBackdropView| view.pair.format == format && view.pair.samples == samples;
        let pair = match existing {
            Some(view) if fresh(view) => view.pair,
            _ => specializer.backdrop_pair(format, samples),
        };
        if existing.is_some_and(|view| view.source == source && fresh(view)) {
            continue;
        }
        commands
            .entity(entity)
            .insert(FfxBackdropView { pair, source });
    }
}

/// **The 2D main pass with the world as its first draw** — bevy's
/// `MainTransparentPass2dNode` with one addition, registered under bevy's own label
/// (`RenderGraph::add_node` is a map insert; the graph's edges, keyed by label, carry over — the
/// same replacement `benilla_app::opaque2d` makes of the opaque node).
///
/// On a view carrying an [`FfxBackdrop`] whose source rendered this frame
/// ([`FfxBackdropView`]), the pass opens on the view's main texture — taking its first-call clear
/// — and the world view's combine is drawn before any item of the transparent phase: the
/// finished world main texture and the blur the world's own node just built, through the
/// gamma-lane exit, into every pixel. Then the interface, over it, exactly as before.
///
/// **Inside the pass, not a pass of its own**, and that is the point on a tile GPU: a render
/// pass boundary on a full-window target is a store of every tile to memory and a load of every
/// tile back, so a separate ground pass before this one cost the Air 0.9 ms a frame it did not
/// cost the RTX (2234's measurement). A draw inside the pass costs the tile nothing but the
/// fragment work. On any other view — no source, a world that has not drawn yet, a camera that
/// is not a backdrop camera at all — this is bevy's node to the letter, and the camera's own
/// clear is the ground, as the backdrop quad's absence was.
#[derive(Default)]
struct FfxTransparent2dNode;

impl ViewNode for FfxTransparent2dNode {
    type ViewQuery = (
        &'static ExtractedCamera,
        &'static ExtractedView,
        &'static ViewTarget,
        &'static ViewDepthTexture,
        Option<&'static FfxBackdropView>,
    );

    fn run<'w>(
        &self,
        graph: &mut RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        (camera, view, target, depth, backdrop): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let Some(transparent_phases) =
            world.get_resource::<ViewSortedRenderPhases<Transparent2d>>()
        else {
            return Ok(());
        };
        let view_entity = graph.view_entity();
        let Some(transparent_phase) = transparent_phases.get(&view.retained_view_entity) else {
            return Ok(());
        };

        // The ground: resolved and bound here, drawn inside the pass below. Everything it needs
        // is read before the pass so the task closure owns only handles.
        let ground = backdrop
            .and_then(|view| view.source.map(|source| (view, source)))
            .and_then(|(view, source)| {
                let (Some(world_target), Some(textures), Some(glow)) = (
                    world.get::<ViewTarget>(source),
                    world.get::<FfxGlowTextures>(source),
                    world.get::<FfxGlow>(source),
                ) else {
                    return None;
                };
                let pipelines = world.resource::<FfxGlowPipelines>();
                let pipeline_cache = world.resource::<PipelineCache>();
                let (uniform, wave) = live_combine(world, glow);
                // Still compiling: the frame has no world in it for a frame.
                let combine = pipeline_cache.get_render_pipeline(view.pair.armed(wave))?;
                world.resource::<RenderQueue>().write_buffer(
                    &textures.gain_buf,
                    0,
                    bytemuck::cast_slice(&uniform),
                );
                let layout = pipeline_cache.get_bind_group_layout(&pipelines.layout_combine);
                // The world view's finished main texture — the very texture its own node would
                // have combined from — and the ¼-res blur its filter passes built this frame.
                // Per-frame, as the world node's own combine bind group is.
                let bind = render_context.render_device().create_bind_group(
                    "ffx_glow_combine",
                    &layout,
                    &BindGroupEntries::sequential((
                        world_target.main_texture_view(),
                        &pipelines.sampler,
                        &textures.quarter_a.default_view,
                        textures.gain_buf.as_entire_binding(),
                        &pipelines.wave_view,
                        &pipelines.wave_sampler,
                    )),
                );
                Some((combine, bind))
            });

        let diagnostics = render_context.diagnostic_recorder();
        let color_attachments = [Some(target.get_color_attachment())];
        let depth_stencil_attachment = Some(depth.get_attachment(StoreOp::Store));

        render_context.add_command_buffer_generation_task(move |render_device| {
            let mut command_encoder =
                render_device.create_command_encoder(&CommandEncoderDescriptor {
                    label: Some("main_transparent_pass_2d_command_encoder"),
                });
            {
                let render_pass = command_encoder.begin_render_pass(&RenderPassDescriptor {
                    label: Some("main_transparent_pass_2d"),
                    color_attachments: &color_attachments,
                    depth_stencil_attachment,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                let mut render_pass = TrackedRenderPass::new(&render_device, render_pass);
                let pass_span = diagnostics.pass_span(&mut render_pass, "main_transparent_pass_2d");
                if let Some(viewport) = camera.viewport.as_ref() {
                    render_pass.set_camera_viewport(viewport);
                }
                if let Some((combine, bind)) = ground.as_ref() {
                    // No span of its own. A `pass_span` is a pipeline-statistics query as well
                    // as a timestamp pair, and wgpu allows ONE such query active at a time: a
                    // second one opened inside the pass's own was the validation error that
                    // aborted every Vulkan build of the 09-15 sync on its first world frame
                    // (2258) — and only Vulkan exposes the feature, so Metal and DX12
                    // never nested anything and no gate saw it. The transparent pass's number
                    // carries the combine; the journal never read a nested span.
                    render_pass.set_render_pipeline(combine);
                    render_pass.set_bind_group(0, bind, &[]);
                    render_pass.draw(0..3, 0..1);
                }
                if !transparent_phase.items.is_empty() {
                    if let Err(err) = transparent_phase.render(&mut render_pass, world, view_entity)
                    {
                        error!(
                            "Error encountered while rendering the transparent 2D phase {err:?}"
                        );
                    }
                }
                pass_span.end(&mut render_pass);
            }
            command_encoder.finish()
        });
        Ok(())
    }
}

pub struct FfxGlowPlugin;

impl Plugin for FfxGlowPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FfxGlowGain(0.647)) // overwritten by sync_gain from zone data
            .init_resource::<FfxDeathFade>()
            .init_resource::<GlueFfx>()
            .init_resource::<FfxHazeMix>()
            .init_resource::<FfxWave>()
            .add_plugins((
                ExtractComponentPlugin::<FfxGlow>::default(),
                ExtractComponentPlugin::<FfxBackdrop>::default(),
                ExtractResourcePlugin::<FfxGlowGain>::default(),
                ExtractResourcePlugin::<FfxDeathFade>::default(),
                ExtractResourcePlugin::<GlueFfx>::default(),
                ExtractResourcePlugin::<FfxHazeMix>::default(),
                ExtractResourcePlugin::<FfxWave>::default(),
            ))
            .add_systems(
                Update,
                (
                    // The gain is the zone's `LightParams.glow`, so the sync is on the resolve's
                    // read side; the haze floor and the wave's arm are the camera-eye submersion
                    // verdict, so they are after the slot that writes it. Unordered, both flipped
                    // a frame late — the underwater blur and warp outlived the surfacing frame
                    // they belong to, exactly like the sky dome's stops.
                    sync_gain.in_set(crate::lighting::LightingConsumeSet),
                    (sync_haze, sync_wave).after(crate::liquid::SubmersionVerdict),
                    ensure_ffx_glow,
                ),
            );
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<SpecializedRenderPipelines<FfxCombinePipeline>>()
            .init_resource::<FfxBackdropClaims>()
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(
                Render,
                // The claims first: a claimed world view builds no combine pair of its own.
                (prepare_backdrops, prepare_textures)
                    .chain()
                    .in_set(RenderSystems::PrepareResources),
            )
            .add_render_graph_node::<ViewNodeRunner<FfxGlowNode>>(Core3d, FfxGlowLabel)
            .add_render_graph_edges(
                Core3d,
                (
                    Node3d::StartMainPassPostProcessing,
                    FfxGlowLabel,
                    Node3d::Bloom,
                ),
            )
            // The 2D main pass with the world as its first draw, replacing bevy's transparent
            // node under bevy's own label (the edges survive; `benilla_app::opaque2d` does the
            // same to the opaque node, which is skipped when empty so this pass takes the
            // target's first-call clear under the ground draw).
            .add_render_graph_node::<ViewNodeRunner<FfxTransparent2dNode>>(
                Core2d,
                Node2d::MainTransparentPass,
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The world pair, live: a released ghost, sober.
    fn ghost_world() -> FfxPassState {
        FfxPassState::world(1.0, 0.0)
    }
    /// The glue pair with a living selection — the common case.
    fn living_glue() -> FfxPassState {
        FfxPassState::glue(0.0)
    }

    /// The zone glow is scene data — a portrait bake wants it at world parity —
    /// while the death and haze lanes belong to a *pass pair*, and a bake runs neither
    /// (now structural: 1731). One preset table, checked as a whole so a new preset
    /// can't quietly join the wrong side.
    #[test]
    fn only_the_two_screen_views_run_a_pass_pair() {
        for (name, view) in [("BOOTH", FfxGlow::BOOTH), ("UI_PANE", FfxGlow::UI_PANE)] {
            assert_eq!(
                view.state,
                FfxState::None,
                "{name} is a bake standing in for a UI widget — it runs no pass pair"
            );
        }
        assert_eq!(FfxGlow::WORLD.state, FfxState::Player);
        assert_eq!(FfxGlow::GLUE_SCENE.state, FfxState::Glue);
        // The glow half is unchanged by that law: a bake still glows like the world.
        assert_eq!(FfxGlow::BOOTH.gain_scale, 1.0);
        assert_eq!(FfxGlow::GLUE_SCENE.gain_scale, 1.0);
        assert_eq!(FfxGlow::UI_PANE.gain_scale, 0.0);
    }

    /// B49: a released ghost's portrait baked through the FFXDeath combine and came back steel-blue
    /// luma. The gate reaches the world view and nothing else — while the zone glow still does.
    #[test]
    fn a_ghosts_bake_is_not_death_combined() {
        let zone = 0.5;
        assert_eq!(
            combine_uniform(
                zone,
                ghost_world(),
                living_glue(),
                &FfxGlow::WORLD,
                FfxWave::default()
            ),
            [0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
        );
        assert_eq!(
            combine_uniform(
                zone,
                ghost_world(),
                living_glue(),
                &FfxGlow::BOOTH,
                FfxWave::default()
            ),
            [0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "the booth keeps the zone glow and drops the death gate"
        );
        assert_eq!(
            combine_uniform(
                zone,
                ghost_world(),
                living_glue(),
                &FfxGlow::UI_PANE,
                FfxWave::default()
            ),
            [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
        );
    }

    /// The control this refactor must not disturb: the drunk/underwater haze was already
    /// world-only, and still is.
    #[test]
    fn a_drunk_players_bake_stays_sharp() {
        let (zone, drunk) = (0.5, FfxPassState::world(0.0, 1.0));
        assert_eq!(
            combine_uniform(
                zone,
                drunk,
                living_glue(),
                &FfxGlow::WORLD,
                FfxWave::default()
            ),
            [0.5, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]
        );
        assert_eq!(
            combine_uniform(
                zone,
                drunk,
                living_glue(),
                &FfxGlow::BOOTH,
                FfxWave::default()
            ),
            [0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
        );
    }

    /// **The two pairs are independent, which is the whole point of naming them**.
    /// The reference has one active-pass slot because its screens take turns; ours coexist, so the
    /// invariant has to be stated: a ghost on the CHARACTER-SELECT list death-combines the glue
    /// scene and nothing else, and a released ghost in the WORLD never reaches the glue view.
    #[test]
    fn each_pass_pair_reaches_only_its_own_view() {
        let zone = 0.5;
        let ghost_glue = FfxPassState::glue(1.0);
        let alive_world = FfxPassState::world(0.0, 0.0);
        assert_eq!(
            combine_uniform(
                zone,
                alive_world,
                ghost_glue,
                &FfxGlow::GLUE_SCENE,
                FfxWave::default()
            ),
            [0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "a ghost roster row death-combines the glue scene"
        );
        assert_eq!(
            combine_uniform(
                zone,
                alive_world,
                ghost_glue,
                &FfxGlow::WORLD,
                FfxWave::default()
            ),
            [0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "…and never the world view, whose own player is alive"
        );
        assert_eq!(
            combine_uniform(
                zone,
                ghost_world(),
                living_glue(),
                &FfxGlow::GLUE_SCENE,
                FfxWave::default()
            ),
            [0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "and a world ghost never reaches the glue view"
        );
    }

    /// The glue pair carries **no haze lane** — a fact about the reference (its pair is built by
    /// CGlueMgr; the haze is `primary.z` of the *WorldFrame* glow combine), enforced by
    /// [`FfxPassState::glue`] having no way to say otherwise. Pinned so the day someone widens that
    /// constructor, a test argues back — and pinned against a fully hazed WORLD, which is the state
    /// that would leak if the two pairs were ever refolded into one.
    #[test]
    fn the_glue_pair_never_hazes() {
        let drunk_world = FfxPassState::world(0.0, 1.0);
        assert_eq!(
            combine_uniform(
                0.5,
                drunk_world,
                FfxPassState::glue(1.0),
                &FfxGlow::GLUE_SCENE,
                FfxWave::default()
            ),
            [0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "the glue view death-combines on its own gate and never hazes"
        );
    }

    /// **The pass-list swap** (`0x6cc630`) — the three conditions, each independently load-bearing.
    ///
    /// The guard is `0x467d00() != 0 && 0x672470() != 0xf`, and the death pass REPLACES the glow
    /// pass outright rather than composing with it, so a ghost has no wave for the same structural
    /// reason it has no haze.
    #[test]
    fn the_warp_arms_only_for_a_living_submerged_world_view() {
        let wet = FfxWave {
            phase1: 0.0,
            phase2: 0.0,
            active: true,
        };
        let dry = FfxWave::default();
        assert!(
            wave_armed(FfxState::Player, wet, 0.0),
            "in-world, submerged, alive — the reference walks its second list"
        );
        assert!(
            !wave_armed(FfxState::Player, dry, 0.0),
            "a dry camera keeps the first list"
        );
        assert!(
            !wave_armed(FfxState::Player, wet, 1.0),
            "a ghost runs CFFXDeath, which owns one list and no guard"
        );
        assert!(
            !wave_armed(FfxState::Glue, wet, 0.0),
            "the glue pair reaches the same Render, but 0x467d00 gates it out of the swap"
        );
        assert!(
            !wave_armed(FfxState::None, wet, 0.0),
            "a bake runs neither pair"
        );
    }

    /// **A dry frame is untouched by this feature** — the perf claim, made structurally rather than
    /// with a frame counter (the machine that would measure it is busy, and a stopwatch could not
    /// prove this anyway).
    ///
    /// Dry, the uniform's first row is bit-identical to what it was before the warp existed and the
    /// phase row is inert; the node then selects `combine` — the same pipeline object, compiled at
    /// the same startup, bound through the same layout. The warp costs a dry frame nothing because
    /// there is nothing of it in a dry frame, not because its cost is small.
    #[test]
    fn a_dry_frame_pays_nothing_for_the_warp() {
        let live = FfxWave {
            phase1: 0.4,
            phase2: 0.7,
            active: false,
        };
        let u = combine_uniform(
            0.5,
            FfxPassState::world(0.0, 0.0),
            living_glue(),
            &FfxGlow::WORLD,
            live,
        );
        assert_eq!(
            u[..4],
            [0.5, 0.0, 0.0, 0.0],
            "the combine lane is exactly what it was before the wave lane existed"
        );
        assert!(
            !wave_armed(FfxState::Player, live, 0.0),
            "and the node binds the unwarped pipeline"
        );
    }

    /// The phases are a FREE-RUNNING clock, not a timer the dive starts: they advance whether or not
    /// the wave is armed, so surfacing and diving again does not restart the pattern. Their periods
    /// are the reference's own two integer millisecond moduli, and being coprime-ish is what stops
    /// the warp reading as a loop — they rejoin only every ~49 minutes.
    #[test]
    fn the_two_phases_run_independently_off_one_clock() {
        let phase = |ms: u64, i: usize| (ms % WAVE_PERIOD_MS[i]) as f32 / WAVE_PERIOD_MS[i] as f32;
        assert_eq!(WAVE_PERIOD_MS, [3174, 2805]);
        // Each wraps on its own period and nowhere else.
        assert!(phase(3173, 0) > 0.999 && phase(3174, 0) == 0.0);
        assert!(phase(2804, 1) > 0.999 && phase(2805, 1) == 0.0);
        // At its own wrap the OTHER phase is mid-stride — the whole point of two moduli.
        assert!(phase(3174, 1) > 0.13 && phase(3174, 1) < 0.14);
        // The joint period: lcm(3174, 2805) ms ≈ 49 min 28 s.
        let gcd = |mut a: u64, mut b: u64| {
            while b != 0 {
                (a, b) = (b, a % b);
            }
            a
        };
        let joint =
            WAVE_PERIOD_MS[0] / gcd(WAVE_PERIOD_MS[0], WAVE_PERIOD_MS[1]) * WAVE_PERIOD_MS[1];
        assert_eq!(joint, 2_967_690);
    }

    /// **The LUT is a displacement map, and each channel bends ONE axis.** That is the fact that
    /// makes the effect a geometric warp rather than a brightness shimmer, and it is visible in the
    /// texels: `du` depends only on x, `dv` only on y (the shipped format is the signed two-channel
    /// bump format on both backends — `D3DFMT_V8U8` / `GL_DSDT8_NV`).
    #[test]
    fn the_wave_lut_is_one_sine_per_axis() {
        let lut = wave_lut_texels();
        let edge = WAVE_LUT_EDGE as usize;
        assert_eq!(lut.len(), edge * edge * 2);
        let at = |x: usize, y: usize| {
            let i = (y * edge + x) * 2;
            (lut[i], lut[i + 1])
        };
        for x in 0..edge {
            for y in [0usize, 37, 91, 127] {
                assert_eq!(at(x, y).0, at(x, 0).0, "du must not vary down the texture");
                assert_eq!(at(x, y).1, at(0, y).1, "dv must not vary across it");
            }
        }
        // Biased back the way the shipped ps_2_0 permutation does, every texel is its axis's sine
        // to within one 8-bit step — the quantization the reference itself ships.
        for i in 0..edge {
            let want = (std::f32::consts::TAU * i as f32 / edge as f32).sin();
            let got = (at(i, i).0 as f32 / 255.0 - 0.5) * 2.0;
            assert!(
                (got - want).abs() <= 2.0 / 255.0,
                "texel {i}: {got} vs sin {want}"
            );
        }
    }

    /// GOLDEN — the reference's own pack, down to the truncation. `ffx_glow_wave_lut`'s path B is
    /// `(s·0.5 + 0.5)·255` → clamp → `__ftol`, and `__ftol` truncates toward zero rather than
    /// rounding: `sin = 0` packs to **127**, not 128, so the map carries a half-step DC bias the
    /// reference carries too. Rounding here would be a "cleaner" number and the wrong one.
    #[test]
    fn the_wave_pack_truncates_like_ftol() {
        let lut = wave_lut_texels();
        // x = 0 and x = 64 are the sine's two zeros; both truncate down.
        assert_eq!(lut[0], 127, "sin(0) = 0 → trunc(127.5) = 127");
        assert_eq!(lut[64 * 2], 127, "sin(π) ≈ 0 → 127");
        // The quarter points saturate the ends of the range.
        assert_eq!(lut[32 * 2], 255, "sin(π/2) = 1 → 255");
        assert_eq!(lut[96 * 2], 0, "sin(3π/2) = −1 → 0");
    }

    /// GOLDEN — the three writers of the glue screens' `LightParams.glow`, and the alpha byte the
    /// death combine quantizes the ghost one into. Each constant has exactly one reference
    /// image-wide.
    ///
    /// The default is a shown-but-unselected glue screen — login, create, and an empty account —
    /// which is the state the login screen renders in and which is NOT either select arm.
    #[test]
    fn the_glue_screens_pin_three_verified_glow_weights() {
        assert_eq!(GlueFfx::default(), GlueFfx::Shown);
        assert_eq!(
            GlueFfx::Shown.glow(),
            0.30,
            "*0x8380b4, the widget's OnShow"
        );
        assert_eq!(GlueFfx::SelectLiving.glow(), 0.40, "*0x838574");
        assert_eq!(GlueFfx::SelectGhost.glow(), 0.15, "*0x838570");
        // Only the ghost arm installs the death pass, and it is the only arm that does.
        assert_eq!(GlueFfx::SelectGhost.death(), 1.0);
        assert_eq!(GlueFfx::Shown.death(), 0.0);
        assert_eq!(GlueFfx::SelectLiving.death(), 0.0);
        // The combine's primary alpha byte (`0x6cb9ec`: ×255, +512, fstp, shr 14) — 38 ghosted,
        // 102 living. A ghost's screen glows at less than half a living one's.
        let alpha_byte = |glow: f32| ((glow * 255.0 + 512.0).to_bits() >> 14) & 0xff;
        assert_eq!(alpha_byte(GlueFfx::SelectGhost.glow()), 38);
        assert_eq!(alpha_byte(GlueFfx::SelectLiving.glow()), 102);
    }
}
