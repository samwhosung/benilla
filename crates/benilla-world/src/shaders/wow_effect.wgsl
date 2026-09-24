// The effect-lane pass (particles, ribbons, decals, water foam, precipitation): texture bytes,
// never decoded on sample, times the authored track colour in gamma space, as the reference
// multiplies them. The output stays gamma; the FFXGlow combine decodes the frame once.
//
// Blend variants, one shader def per EffectBlend:
// - BLEND_ADD:      (rgb·α, 0) under (One, 1−srcα), the reference's `src·α` byte add.
// - BLEND_ALPHA:    straight (rgb, α) under standard alpha blending.
// - BLEND_OPAQUE:   (rgb, 1) with blending off.
// - BLEND_ALPHAKEY: BLEND_OPAQUE behind the alpha test (EGxRs id 0x08), a discard below 224/255.
// - BLEND_MULTIPLY: (rgb·α, α) under (Dst, 1−srcα) = `dst·lerp(1, rgb, α)`, the blob shadow's
//   modulate-with-fade and ModelBlend::Mod at α = 1.
// - BLEND_MOD2X:    (rgb, 1) under (Dst, Src) = `2·src·dst`, rain's state; reads no alpha.

#import bevy_render::view::View

// Prefix of `lighting::global_light`'s buffer; keep in sync with wow_model.wgsl's copy.
struct WowLight {
    light_ambient: vec4<f32>,
    light_diffuse: vec4<f32>,
    light_sun: vec4<f32>,
    light_spec: vec4<f32>,
    fog_color: vec4<f32>,  // rgb row-7 fog (gamma); w = enable (>0.5)
    fog_params: vec4<f32>, // x=start y=end z=linear-lighting A/B flag w=farclip wall
    sh_c10_r: vec4<f32>,
    sh_c10_g: vec4<f32>,
    sh_c10_b: vec4<f32>,
    sh_c13_r: vec4<f32>,
    sh_c13_g: vec4<f32>,
    sh_c13_b: vec4<f32>,
    sh_c16: vec4<f32>,
    _water: array<vec4<f32>, 4>,
    grade: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var effect_texture: texture_2d<f32>;
@group(1) @binding(1) var effect_sampler: sampler;
@group(1) @binding(2) var<storage, read> wow_light: WowLight;
// Per-draw params. `fog`: x the fog colour policy, `0x70baf0`'s per-blend table (0 off for file
// flag 0x8, 1 scene, 2 black for Add, 3 white for Mod, 4 grey for Mod2x); y rain's forced fog, zw
// its start and end. `clip`: the target rect in pixels (min.xy, max.xy), the whole target when
// z <= x; the reference clips a UI model to its widget's viewport, and model panes share an atlas.
struct EffectParams {
    fog: vec4<f32>,
    clip: vec4<f32>,
};
@group(1) @binding(3) var<uniform> wow_params: EffectParams;

// The rain pass's forced fog colour: 0x80808080 → grey (render-state 0x0d).
const RAIN_FOG_GREY: vec3<f32> = vec3<f32>(0.50196078, 0.50196078, 0.50196078);

struct Vertex {
    // Camera-relative for f32 precision (absolute verts through `clip_from_world` shear thin
    // geometry far from the origin), except decals under DECAL_WORLD_CLIP, which arrive absolute.
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,    // raw authored gamma RGBA; α is the blend weight
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // Planar eye-Z for fog and the farclip wall, positive in front of the camera.
    @location(2) view_z: f32,
};

@vertex
fn vertex(v: Vertex) -> VertexOutput {
    var out: VertexOutput;
#ifdef DECAL_WORLD_CLIP
    // Decals (raster_bias ≠ 0) take absolute verts through the world meshes' `clip_from_world`:
    // their depth must tie the ground within the raster bias, and the cam-relative route rounds
    // differently by more than that at world-scale coordinates.
    out.clip_position = view.clip_from_world * vec4<f32>(v.position, 1.0);
    // Eye-Z via the full affine transform; its rounding is yard-scale and harmless.
    out.view_z = -(view.view_from_world * vec4<f32>(v.position, 1.0)).z;
#else
    // Cam-relative verts: view_from_world is [R | −R·cam] and the rebase already subtracted cam,
    // so only the rotation applies.
    let view_pos = mat3x3<f32>(
        view.view_from_world[0].xyz,
        view.view_from_world[1].xyz,
        view.view_from_world[2].xyz,
    ) * v.position;
    out.clip_position = view.clip_from_view * vec4<f32>(view_pos, 1.0);
    out.view_z = -view_pos.z;
#endif
    out.uv = v.uv;
    out.color = v.color;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // The target-rect clip; `@builtin(position)` is the framebuffer pixel, a UI tile's atlas texel.
    if (wow_params.clip.z > wow_params.clip.x) {
        let p = in.clip_position.xy;
        if (p.x < wow_params.clip.x || p.y < wow_params.clip.y
            || p.x > wow_params.clip.z || p.y > wow_params.clip.w) {
            discard;
        }
    }
    // `$WOW_PARTICLE_FLAT` debug instrument: solid magenta, no inputs.
#ifdef WOW_PARTICLE_FLAT
    return vec4<f32>(1.0, 0.0, 1.0, 1.0);
#else
    // The hard farclip wall (the live `farclip`, 177 to 777 yd): the reference's far plane clips
    // effects and ours is farther, so every world shader discards. The fog ends by the farclip, so
    // this cuts only fog.
    if (wow_light.fog_params.w > 0.0 && in.view_z > wow_light.fog_params.w) {
        discard;
    }
    // Texture bytes × authored vertex colour in gamma space; `view.mip_bias` is the render-scale
    // LOD compensation, 0.0 at native and above.
    let c = textureSampleBias(effect_texture, effect_sampler, in.uv, view.mip_bias) * in.color;
#ifdef BLEND_ALPHAKEY
    // The fixed-function alpha test (EGxRs id 0x08, `glAlphaFunc(GL_GEQUAL, ref/255)`): `0x70c256`
    // sets ref = round(instanceAlpha × 224), 224 at full alpha, and `c.a` is the reference's
    // fragment alpha (texture α × track α, GL_MODULATE). GEQUAL passes at the ref.
    if (c.a < 224.0 / 255.0) {
        discard;
    }
#endif
    var rgb = c.rgb;
#ifdef EFFECT_LIT
    // Scene lighting (EGxRs id 0x0e, `0x70d8b0`) when the emitter clears the unlit flag 0x1 and
    // its blend is not Mod or Mod2x (`0x70bb00`): the fixed-function matte with N = world up for
    // the whole draw, as `0x7b3fd0` takes the normal from view row 2 and `0x71bce0` moves the
    // light into that frame, so N·L holds as the camera orbits. Bevy +Y is WoW +Z.
    let L = -normalize(wow_light.light_sun.xyz);
    let N = vec3<f32>(0.0, 1.0, 0.0);
    let lit = clamp(
        wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb * max(dot(N, L), 0.0),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    rgb = rgb * lit;
#endif
    // Scene fog: the world's linear fog (same start and end, planar eye-Z) in gamma space before
    // the blend; only its colour follows the per-blend policy in `wow_params.fog.x`.
    if (wow_light.fog_color.w > 0.5 && wow_params.fog.x > 0.5) {
        let denom = max(wow_light.fog_params.y - wow_light.fog_params.x, 0.001);
        let factor = clamp((wow_light.fog_params.y - in.view_z) / denom, 0.0, 1.0);
        var fog_rgb = wow_light.fog_color.xyz;
        if (wow_params.fog.x > 1.5 && wow_params.fog.x < 2.5) { fog_rgb = vec3<f32>(0.0); }
        else if (wow_params.fog.x > 2.5 && wow_params.fog.x < 3.5) { fog_rgb = vec3<f32>(1.0); }
        else if (wow_params.fog.x > 3.5) { fog_rgb = RAIN_FOG_GREY; }
        rgb = mix(fog_rgb, rgb, factor);
    }
    // Rain's forced fog: grey over the draw's own start/end, whatever the scene fog; grey is
    // neutral under Mod2x, so this is the streaks' distance fade.
    if (wow_params.fog.y > 0.5) {
        let denom = max(wow_params.fog.w - wow_params.fog.z, 0.001);
        let factor = clamp((wow_params.fog.w - in.view_z) / denom, 0.0, 1.0);
        rgb = mix(RAIN_FOG_GREY, rgb, factor);
    }
#ifdef BLEND_ADD
    // Premultiplied in gamma, so stacked quads sum like the reference's bytes.
    return vec4<f32>(rgb * c.a, 0.0);
#else
#ifdef BLEND_OPAQUE
    return vec4<f32>(rgb, 1.0);
#else
#ifdef BLEND_ALPHAKEY
    // Blending is off (EGxBlend 1 → `glDisable(GL_BLEND)`); the surviving fragments are solid.
    return vec4<f32>(rgb, 1.0);
#else
#ifdef BLEND_MULTIPLY
    return vec4<f32>(rgb * c.a, c.a);
#else
#ifdef BLEND_MOD2X
    return vec4<f32>(rgb, 1.0);
#else
    return vec4<f32>(rgb, c.a);
#endif
#endif
#endif
#endif
#endif
#endif
}
