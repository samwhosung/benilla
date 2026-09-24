//! GPU materials for the celestial layer — the alpha-blended **disc** material (with the horizon clip +
//! per-fragment alpha ramp) and the gamma-correct **star** material. Both are [`ExtendedMaterial`]s over
//! `StandardMaterial`; the fragment shaders live in `src/shaders/{celestial,star}.wgsl`,
//! compiled in and served as `embedded://benilla_world/shaders/…` (decision 1175).

use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

use crate::sky_order::{sky_pipeline_state, SKY_VERTEX_SHADER};

/// Material for every celestial body sprite — the **discs** (sun, white moon, moon02) *and* their
/// additive lens-flare **glares** — an [`ExtendedMaterial`] over `StandardMaterial` whose fragment
/// shader (`celestial.wgsl`) replicates the unlit look **in gamma space** (the 0161 composite lane) in
/// one of two modes ([`CelestialExt::fade`]`.z`): discs get the **world-space horizon clip +
/// per-fragment alpha ramp** (the real `0x6d1960` clip+fade every disc routes through; decision 0485)
/// under a premultiplied gamma blend; glares skip the clip and **ADD gamma bytes** under
/// `AlphaMode::Add` — the reference's SRC_ALPHA, ONE lens-flare blend (`0x7e5a16`), which is what lets
/// the flare saturate the sun's core to white (decision 0502; plain linear `Add` under-weighted every
/// mid-tone — the "too yellow" sun).
pub type CelestialMaterial = ExtendedMaterial<StandardMaterial, CelestialExt>;

/// Per-material control. `fade.x` = the horizon alpha-ramp scale `k` (in sin-elevation terms;
/// [`DISC_HORIZON_FADE`] for all three discs — the binary's 0.4-unit band at its radius-12 disc,
/// clip+fade `0x6d1960`, decision 0485). `fade.w` = the disc COLOUR's own alpha byte: `1.0` for the
/// sun/white moon (their diffuse broadcast writes 0xFF every frame), `0.0` for moon02 (its colour
/// dword has no writer in the binary — the draw runs and paints nothing: the reference's invisible
/// second moon; `0xce98a4`). The fade is the reference's PER-VERTEX conditional store
/// (band vertices take the ramp instead of the colour alpha), which the fragment shader
/// reconstructs by interpolating over [`CelestialExt::span`] — a setting body melts as a
/// whole-disc gradient (decision 0529; supersedes the 0485 per-fragment band and 0524's
/// workaround). Under active weather the follow systems overwrite `fade.w` with the celestial
/// alpha seed `floor(255·(1−bcc))/255` on all three discs (`bcc` = weather density × 4, clamped —
/// `0x6d2c74`; `follow::celestial_alpha_seed`), so storms dim the bodies and surface moon02's
/// faint dark disc. `fade.y` = a brightness multiplier on the emitted colour — `1.0` everywhere (the old
/// moon `1.6` HDR boost died with decision 0163: the reference is LDR bytes and its moon glow is
/// FFXGlow's blur² on the disc's own bytes). `fade.z` = mode: 0 = disc, 1 = additive glare (no
/// clip, gamma add — the glares never route `0x6d1960`; their envelope gates them; `fade.w`/`span`
/// unused there).
#[derive(Asset, AsBindGroup, Clone, TypePath, Default)]
pub struct CelestialExt {
    #[uniform(100)]
    pub(super) fade: Vec4,
    /// The disc quad's vertical span in sin-elevation — `.x` = bottom edge, `.y` = top edge —
    /// written per frame by the follow systems ([`super::follow::disc_span`]). The shader
    /// interpolates the per-vertex fade rule across it (decision 0529). `.zw` unused.
    #[uniform(101)]
    pub(super) span: Vec4,
}

impl MaterialExtension for CelestialExt {
    /// The shared sky vertex stage — the far-depth pin ([`crate::sky_order`], "The depth law").
    fn vertex_shader() -> ShaderRef {
        SKY_VERTEX_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://benilla_world/shaders/celestial.wgsl".into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        sky_pipeline_state(descriptor);
        Ok(())
    }
}

/// Disc horizon alpha-ramp scale, shared by every celestial disc: `alpha *= clamp(30·dir.y, 0, 1)` —
/// the faithful `0x6d1960` fade (`alpha = clamp(2.5·height, 0, 1)` with the disc at radius 12 ⇒
/// `2.5·12·dir.y`), a soft edge over the bottom ~1.9° of elevation; ≤ 0 clips (the reference skips
/// the whole body below the horizon).
pub(super) const DISC_HORIZON_FADE: f32 = 30.0;

/// Material for the **star** patches — an [`ExtendedMaterial`] over `StandardMaterial` whose fragment
/// (`star.wgsl`) does a **gamma-correct premultiplied** blend so the soft white dots blend into the sky
/// like the reference, instead of over-brightening (our linear-space alpha blend would; see the shader
/// header). Used with `AlphaMode::Premultiplied`. `StarExt` carries no uniforms.
pub type StarMaterial = ExtendedMaterial<StandardMaterial, StarExt>;

/// Empty material extension — the star fragment needs no per-material uniforms (it reads the base-colour
/// alpha = dot alpha × the star-curve global alpha).
#[derive(Asset, AsBindGroup, Clone, TypePath, Default)]
pub struct StarExt {}

impl MaterialExtension for StarExt {
    /// The shared sky vertex stage — the far-depth pin ([`crate::sky_order`], "The depth law").
    fn vertex_shader() -> ShaderRef {
        SKY_VERTEX_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://benilla_world/shaders/star.wgsl".into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        sky_pipeline_state(descriptor);
        Ok(())
    }
}
