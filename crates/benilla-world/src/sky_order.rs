//! The celestial draw-order ladder: the reference's fixed sky pass order as `Transparent3d` sort
//! biases.
//!
//! The 1.12 client draws `sky → opaque world → weather → glare`. Its sky pass (`CSky::Render`,
//! `0x6d4940`) paints stars, sun disc, white moon, moon02, the additive gradient strip and the
//! cloud dome, in that order, depth-write off in the slice `[0.975, 0.98]`; the glare quads take
//! `[0.995, 1.0]` and draw last. Our sky is camera-anchored and Bevy sorts transparents back to
//! front by `view-z + depth_bias`, so its rungs sit far below any world view-z. Rain has no bias:
//! its view-z lands it after the sky and before the glare, the reference's weather slot; its order
//! against world transparents is untraced.
//!
//! The sign: `Transparent3d` sorts ascending on view-space z (negative in front of the eye) plus
//! the bias, so a positive bias draws later, on top. On a `StandardMaterial` the same field is the
//! rasterizer's depth-bias constant, so a rung on a depth-tested draw stays as small as it can.
//!
//! The depth law: our sky draws after the world, so every sky vertex pins clip z to 0.0, reverse-Z
//! infinitely far, in the shared vertex stage ([`SKY_VERTEX_SHADER`]): a sky fragment survives only
//! where the depth buffer holds the clear value, as the reference's world paints over its sky, and
//! no shell radius decides occlusion. Pinned at the vertex so early-Z rejects covered fragments,
//! with [`sky_pipeline_state`] zeroing the raster bias that would move it.

use bevy::render::render_resource::RenderPipelineDescriptor;

/// The vertex stage every sky material draws through, the far-depth pin (the depth law); each
/// returns it from `vertex_shader()`, and the test below holds them to it.
pub(crate) const SKY_VERTEX_SHADER: &str = "embedded://benilla_world/shaders/sky_vertex.wgsl";

/// Every sky material's `specialize` (the depth law): zero the rasterizer depth-bias constant, so
/// the rung in `StandardMaterial::depth_bias` stays a sort key and the pinned depth is untouched.
pub(crate) fn sky_pipeline_state(descriptor: &mut RenderPipelineDescriptor) {
    if let Some(ds) = descriptor.depth_stencil.as_mut() {
        ds.bias.constant = 0;
    }
}

/// Stars, the sky pass's first draw (`0x6d4a3f`): the ladder's lowest rung.
pub(crate) const STARS_BIAS: f32 = -1.0e6;
/// The WMO skybox ([`crate::skybox`]), which replaces this ladder while it draws: an ordinary
/// blended M2, so its camera-anchored batches need a rung under every world transparent (the
/// deepest, [`FAR_SIDE_BIAS`] at the far plane, ≈ −4.3e4), and no deeper, so its batch-order step
/// (`model_render::SKYBOX_ORDER_EPS`) stays above the f32 ulp here.
pub(crate) const WMO_SKYBOX_BIAS: f32 = -6.0e4;
/// The sun disc, second (`0x7e5b90` via `0x6d4a47`).
pub(crate) const SUN_DISC_BIAS: f32 = -8.2e5;
/// The white moon, third: where the discs cross, it paints over the sun.
pub(crate) const WHITE_MOON_BIAS: f32 = -8.1e5;
/// moon02, fourth: invisible in clear weather, ordered for when the weather seed surfaces it.
pub(crate) const MOON02_BIAS: f32 = -8.0e5;
/// The cloud dome, the sky pass's last draw (`0x6d4a71`): clouds blend over a setting sun.
pub(crate) const CLOUDS_BIAS: f32 = -6.0e5;
/// Far-side model transparents, the water-plane interleave's early half: the reference splits M2
/// transparents per model (`0x707680`; per emitter for particles, `0x7084a0`) and `0x483460` draws
/// the eye's far side of the water plane before the water pass, the near side after, flipping on
/// submersion (`0x4836d6`). Sort-only: the mesh lane's far twin zeroes its raster constant.
pub(crate) const FAR_SIDE_BIAS: f32 = -4.0e4;
/// The water surface: the reference draws ocean, river, WMO liquid, then foam in a fixed slot
/// between the two transparent halves (`0x6701d0 → 0x6816d0`), never view-z sorted against model
/// transparents. It rides a `StandardMaterial`, so it is also a depth-test bias (~0.24% pull).
pub(crate) const WATER_BIAS: f32 = -2.0e4;
/// The water pass's own foam (`CWater0Ripple`'s wade wake), drawn inside the water group after
/// every liquid queue (`0x6816d0`): over every water surface, under the near-side transparents. A
/// rung, not a tie-break over [`WATER_BIAS`], since a chunk's key carries its own view-z.
pub(crate) const FOAM_BIAS: f32 = -1.0e4;
/// The sun and moon glare quads, the frame's last render (`0x483740`): over the clouds and the
/// rain, under the nameplates, occluded only by their pinned far depth.
pub(crate) const GLARE_BIAS: f32 = 2.0e4;

/// The floor of any world draw's view-z, the ~3 km far plane negated: two rungs order their draws
/// unconditionally only when their gap exceeds it.
pub(crate) const WORLD_VIEW_Z_FLOOR: f32 = -3.0e3;

/// The world-side rungs other lanes apply to their own draws, kept here as one ladder the
/// compile-time asserts below can see whole.
pub struct Rung;

impl Rung {
    /// The ground-fx spell decals' sort rung, after the water and every unbiased world transparent;
    /// the reference draws these quads as ordinary M2 batches with no frame slot of their own.
    pub const GROUND_FX: f32 = 8192.0;
    /// The rasterizer depth-bias constant every ground decal shares (ring, blob shadow, ground-fx,
    /// footprints, reticle), winning the `GreaterEqual` tie against the ground over the bake
    /// residual: ~1 to 3 world-coordinate ulps between CPU-baked verts and GPU-transformed ones.
    ///
    /// Deviation: one constant, not the reference's per-lane magnitudes (ring and shadow at one
    /// site, `0x6d7480`; footprints 10x, `0x69a54a`; foam 20x), because those suit its 24-bit
    /// fixed-point forward-Z buffer; in our reversed-Z float buffer the bias is ULP-relative, and
    /// one bake path sets one residual.
    pub const DECAL_RASTER: i32 = 32768;
    /// The pre-water decal band, rung 1: the selection ring (and, when built, the corpse and
    /// click-to-move markers). The reference emits it in phase 1's M2 node drain
    /// (`0x6812c5 call 0x683dd0`; the node's `+0x38` tick, `0x48160c` → `0x608e00`) before the
    /// node's shadow gate (`0x683ec3`), so the modulate shadow darkens the additive ring.
    pub const RING: f32 = -5.4e4;
    /// The pre-water band, rung 2: the unit blob shadow, emitted in the same node drain right
    /// after the ring (`0x6d78f0` → `0x6d7920`), before the footprints, the M2 opaque pass
    /// (`0x4836a6`) and the water (phase 3, `0x6816d0`). Sort-only, its raster half being
    /// [`DECAL_RASTER`](Rung::DECAL_RASTER). The band's window is `(−5.7e4, −4.3e4)`, between the
    /// WMO-skybox band and the deepest world transparent.
    pub const SHADOW_SORT: f32 = -5.2e4;
    /// The pre-water band, rung 3: footprints, drawn at `0x483654` (`0x670240` → `0x69a3e0`) after
    /// the shadows and before the M2 opaque pass, so a print paints over a shadow.
    pub const FOOTPRINT: f32 = -5.0e4;
    /// The pre-water band, rung 4: the ground-target reticle's solid-receiver pass (`0x4836c5`,
    /// flags `0x200122`), the last decal before the water. Its liquid pass (`0x483727`, flags
    /// `0xf0000`) is not built: liquid is not a `GroundDecalSurface`, and it would need a rung
    /// above [`WATER_BIAS`].
    pub const RETICLE: f32 = -4.8e4;
    /// The wade-foam decal's rasterizer constant. The reference's foam draw (`0x68fd0f`,
    /// `0x68fae0`) takes `D3DRS_DEPTHBIAS` = `−f32(footstepBias(0.125) × [0x810390])`, −2048 ULPs
    /// of its 24-bit depth; its OpenGL arm's `−4.0` slope never runs, as `gxApi` defaults to
    /// `"direct3d"` (`0x63a81d`). Reversed-Z flips the sign.
    ///
    /// Deviation: only a few ULPs, a guard against drivers rounding coplanar arithmetic
    /// differently, because the reference's `1.29847e−3 · d²` yd pull paints the splash onto dry
    /// bank (0.3 to 1.0 yd at an 11.9-yd camera, measured).
    pub const FOAM_RASTER: i32 = 8;
    /// Zero, as in the reference, which never writes `D3DRS_SLOPESCALEDEPTHBIAS`. The foam patch is
    /// the liquid mesh's own triangles, so `GreaterEqual` wins the tie unaided, and a slope pull
    /// (growing as z²) would spread the wake over the wet cells a shoreline lays on sand.
    pub const FOAM_RASTER_SLOPE: f32 = 0.0;
    /// The underwater drift cloud, at the reference's slot `0x483731`: after the water and both M2
    /// transparent passes, just before the glare (`0x483740`). One batch, no per-mote sort: the
    /// reference submits one indexed triangle list (`0x68f3c9`) with depth-write off.
    pub const DRIFT_CLOUD: f32 = 1.4e4;
    /// World text, above the glare so a flare never washes a nameplate (the reference draws it late
    /// in the frame); also the raster bias of a depth-tested layer, so no bigger than needed.
    pub const NAMEPLATE: f32 = 4.0e4;
}

/// The ladder is the reference order, checked at compile time. Gaps exceed 1e4, more than any
/// view-z spread, except in the narrow windows (the decal band, the foam, the drift cloud), which
/// are checked against their neighbours.
const _: () = {
    assert!(SUN_DISC_BIAS - STARS_BIAS > 1.0e4);
    assert!(WHITE_MOON_BIAS > SUN_DISC_BIAS && MOON02_BIAS > WHITE_MOON_BIAS);
    assert!(CLOUDS_BIAS - MOON02_BIAS > 1.0e4);
    // sky < far-side transparents < water < near-side transparents; a far-side draw's own offsets
    // (zfill −8, owner rung, batch eps) stay far under 1e4.
    assert!(FAR_SIDE_BIAS - CLOUDS_BIAS > 1.0e4);
    assert!(WATER_BIAS - FAR_SIDE_BIAS > 1.0e4);
    // −3e3 is the world view-z floor, −far.
    assert!(-3.0e3 - WATER_BIAS > 1.0e4);
    // Foam: over every liquid surface and under the near-side default, at any distance.
    assert!(FOAM_BIAS + WORLD_VIEW_Z_FLOOR - WATER_BIAS > 1.0e3);
    assert!(WORLD_VIEW_Z_FLOOR - FOAM_BIAS > 1.0e3);
    assert!(Rung::GROUND_FX - CLOUDS_BIAS > 1.0e4);
    assert!(GLARE_BIAS - Rung::GROUND_FX > 1.0e4);
    assert!(Rung::NAMEPLATE - GLARE_BIAS > 1.0e4);
    // The drift cloud's anchor is the eye, so its margin need only beat one far plane.
    assert!(Rung::DRIFT_CLOUD - Rung::GROUND_FX > -WORLD_VIEW_Z_FLOOR);
    assert!(GLARE_BIAS - Rung::DRIFT_CLOUD > -WORLD_VIEW_Z_FLOOR);
    // The raster constants are their own axis, comparable only to zero and to each other.
    assert!(Rung::DECAL_RASTER > 0 && Rung::FOAM_RASTER < Rung::DECAL_RASTER);
    // Positive pulls toward the eye under reversed-Z; negative would push foam into its receiver.
    assert!(Rung::FOAM_RASTER > 0);
    assert!(Rung::FOAM_RASTER_SLOPE == 0.0);

    // ─── The pre-water decal band ─────────────────────────────────────────
    // Ring, shadow, footprints, reticle, each step wider than two stacked decals' view-z spread.
    assert!(Rung::SHADOW_SORT - Rung::RING > 1.0e3);
    assert!(Rung::FOOTPRINT - Rung::SHADOW_SORT > 1.0e3);
    assert!(Rung::RETICLE - Rung::FOOTPRINT > 1.0e3);
    // Below every world transparent at any distance: the deepest is a far-side zfill twin (−8).
    assert!(FAR_SIDE_BIAS - 8.0 + WORLD_VIEW_Z_FLOOR - Rung::RETICLE > 1.0e3);
    // Above the WMO-skybox band at any distance, measured from the band's floor, the ring.
    assert!(Rung::RING + WORLD_VIEW_Z_FLOOR - WMO_SKYBOX_BIAS > 1.0e3);
};

/// The depth law, in the shaders: the shared vertex stage pins the far depth (without it stars
/// show through distant hills), every sky material draws through it, and no fragment rewrites it.
#[test]
fn the_sky_depth_is_pinned_at_the_vertex_and_nowhere_else() {
    use bevy::pbr::Material;
    use bevy::shader::ShaderRef;

    let vertex = include_str!("shaders/sky_vertex.wgsl");
    assert!(
        vertex.contains("const SKY_FAR_CLIP_Z: f32 = 0.0;")
            && vertex.contains("out.position.z = SKY_FAR_CLIP_Z;"),
        "sky_vertex.wgsl no longer pins the far depth — every shell radius is deciding occlusion \
         again (sky_order.rs, \"The depth law\")"
    );
    for (name, src) in [
        ("sky.wgsl", include_str!("shaders/sky.wgsl")),
        ("star.wgsl", include_str!("shaders/star.wgsl")),
        ("cloud.wgsl", include_str!("shaders/cloud.wgsl")),
        ("celestial.wgsl", include_str!("shaders/celestial.wgsl")),
        // The WMO skybox draws on the shared model lane, whose `WOW_SKY_DEPTH` branch is held to
        // the same law in `benilla_assets::materials`.
    ] {
        assert!(
            !src.contains("@builtin(frag_depth)"),
            "{name}: a sky fragment writes its depth again — the pin is the vertex stage's, and a \
             fragment write costs the pipeline its early-Z (sky_order.rs, \"The depth law\")"
        );
    }
    fn shared(name: &str, shader: ShaderRef) {
        match shader {
            ShaderRef::Path(p) => assert_eq!(
                p.to_string(),
                SKY_VERTEX_SHADER,
                "{name}: not drawing through the shared sky vertex stage"
            ),
            _ => panic!("{name}: vertex shader is not a path — not the shared sky vertex stage"),
        }
    }
    shared(
        "SkyMaterial",
        <crate::sky::SkyMaterial as Material>::vertex_shader(),
    );
    shared(
        "CloudMaterial",
        <crate::clouds::CloudMaterial as Material>::vertex_shader(),
    );
    shared(
        "StarMaterial",
        <crate::sun::StarMaterial as Material>::vertex_shader(),
    );
    shared(
        "CelestialMaterial",
        <crate::sun::CelestialMaterial as Material>::vertex_shader(),
    );
}
