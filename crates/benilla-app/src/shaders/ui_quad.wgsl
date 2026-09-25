// The player-UI quad material. The reference draws the UI fixed-function into an 8-bit
// backbuffer, so the fragment tints and premultiplies on gamma bytes and outputs raw gamma for the
// `(One, OneMinusSrcAlpha)` blend; `ui_gamma.wgsl` decodes once. One pipeline, a flag per mode:
//   BLEND (EGxBlend 2, `SrcAlpha/OneMinusSrcAlpha`): out = (rgb·a, a) ⇒ dst·(1−a) + rgb·a
//   ADD   (EGxBlend 3, `SrcAlpha/One`):              out = (rgb·a, 0) ⇒ dst      + rgb·a
// `linear_to_srgb` restores the authored byte of every sampled texel except a SKIP_DECODE upload
// (`gamma_texel`), which already holds it.
// Deviation: bilinear filtering runs on the decoded texel, not on the gamma byte the reference
// filters, because the booth bake stores linear bytes.

#import bevy_sprite::mesh2d_functions as mesh_functions

// The mesh vertex at bevy's fixed Mesh2d locations (`Mesh2dPipeline::specialize`: POSITION 0, UV_0
// 2, COLOR 4); `VERTEX_UVS`/`VERTEX_COLORS` are defined only for attributes the mesh carries.
struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
#ifdef VERTEX_UVS
    @location(2) uv: vec2<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(4) color: vec4<f32>,
#endif
}

struct VertexOutput {
    // In the fragment, the physical pixel coordinate the screen mask uses.
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
#ifdef VERTEX_COLORS
    @location(1) color: vec4<f32>,
#endif
    // The run's colour from the per-instance `MeshTag`, a byte per channel as the reference's
    // `CImVector`. Stored complemented, so no `MeshTag` (0) is opaque white.
    @location(2) @interpolate(flat) tint: vec4<f32>,
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let world_position = mesh_functions::mesh2d_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    out.position = mesh_functions::mesh2d_position_world_to_clip(world_position);
#ifdef VERTEX_UVS
    out.uv = vertex.uv;
#else
    out.uv = vec2<f32>(0.0);
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif
    // `unpack4x8unorm` reads byte 0 into `.x`: r, g, b, a from the low byte up.
    out.tint = unpack4x8unorm(~mesh_functions::get_tag(vertex.instance_index));
    return out;
}

@group(2) @binding(0) var<uniform> additive: u32;
@group(2) @binding(1) var quad_texture: texture_2d<f32>;
@group(2) @binding(2) var quad_sampler: sampler;
// Mask to the inscribed circle: the unit portrait's round stencil, which the reference stamps into
// its 64² bake.
@group(2) @binding(3) var<uniform> circular: u32;
// The screen-anchored alpha mask (the minimap's `MinimapMask.blp`): `mask_rect` is its span in
// physical px (min.xy, max.xy; z <= x disables); outside it is dropped. Sampled at level 0 because
// the sample sits in non-uniform control flow.
@group(2) @binding(4) var<uniform> mask_rect: vec4<f32>;
@group(2) @binding(5) var mask_texture: texture_2d<f32>;
@group(2) @binding(6) var mask_sampler: sampler;
// `Texture:SetDesaturated(1)` binds `Shaders\Pixel\Desaturate.bls` (texture object `+0x128`):
//     MUL result.color.w   , fragment.color.primary, texel   ; a = vertexColour.a x texel.a
//     DP3 result.color.xyz , texel, c[0]                     ; rgb = dot(texel.rgb, LUMA)
// It replaces the MODULATE, so the vertex colour's RGB has no effect, only its alpha. The dot runs
// on the gamma byte.
@group(2) @binding(7) var<uniform> desaturate: u32;
// Set only for a booth bake (portrait, paper doll, dressing room), already premultiplied because
// its additive particles add light with no coverage; every other texture is straight alpha.
@group(2) @binding(8) var<uniform> premultiplied: u32;
// Alpha-test reference, <= 0 disables; only the WMO-interior minimap tiles, which the reference
// draws under EGxBlend 1: no blend, `glAlphaFunc(GL_GEQUAL, 224/255)` (`0x85ad20[1]`) on
// `texel.a × frameAlpha`. The screen mask stays out: the reference cuts it at the offscreen blit.
@group(2) @binding(9) var<uniform> alpha_ref: f32;
// Uploaded undecoded (`BlpVariant::MapTile`, the minimap tiles): the texel is already the gamma
// byte. The alpha-test arm ignores this and decodes explicitly.
@group(2) @binding(10) var<uniform> gamma_texel: u32;
// The UV window this quad may sample, `(u_min, v_min, u_max, v_max)`, inset half a texel so a
// magnified atlas cell never filters in its neighbour; `min > max` leaves an axis unclamped.
@group(2) @binding(11) var<uniform> uv_clamp: vec4<f32>;

// BT.601 luma, `Desaturate.bls`'s `c[0]` as f32 words `0x3E991687`, `0x3F1645A2`, `0x3DE978D5`.
// Do not normalise: they sum just above 1.0 and white relies on the output clamp.
const LUMA: vec3<f32> = vec3<f32>(0.299, 0.587, 0.114);

// Linear to sRGB (IEC 61966-2-1), the exact inverse of the sampler's decode in f32.
fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let higher = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    let lower = c * 12.92;
    return select(higher, lower, c <= vec3<f32>(0.0031308));
}

// sRGB to linear for an undecoded texture: the reference filters the bytes, then converts.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let higher = pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    let lower = c / 12.92;
    return select(higher, lower, c <= vec3<f32>(0.04045));
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // Branchless, for implicit derivatives. Sorted because `select` evaluates both arms and a
    // disabled axis must still hand `clamp` an ordered range.
    let lo = min(uv_clamp.xy, uv_clamp.zw);
    let hi = max(uv_clamp.xy, uv_clamp.zw);
    let uv = select(in.uv, clamp(in.uv, lo, hi), uv_clamp.xy <= uv_clamp.zw);
    let t = textureSample(quad_texture, quad_sampler, uv);
#ifdef VERTEX_COLORS
    let c = in.color * in.tint;
#else
    let c = in.tint;
#endif
    // The tint is a client-space byte value (`<Color>`, `|cff…`), so `tint × texel` runs on bytes.
    let texel = select(linear_to_srgb(t.rgb), t.rgb, gamma_texel != 0u);
    var rgb = texel * c.rgb;
    if desaturate != 0u {
        rgb = vec3<f32>(dot(texel, LUMA));
    }
    // `k`, the UI's coverage (frame alpha and masks), stays apart from `t.a`: it scales a
    // premultiplied source's colour and alpha alike, while `t.a` weights only a straight source.
    var k = c.a;
    if circular != 0u {
        // A soft edge 2% of the width, like the reference's stencil at portrait size.
        k *= 1.0 - smoothstep(0.48, 0.5, distance(in.uv, vec2<f32>(0.5)));
    }
    if mask_rect.z > mask_rect.x {
        let muv = (in.position.xy - mask_rect.xy) / (mask_rect.zw - mask_rect.xy);
        let inside = f32(all(muv >= vec2<f32>(0.0)) && all(muv <= vec2<f32>(1.0)));
        let m = textureSampleLevel(mask_texture, mask_sampler, clamp(muv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).a;
        k *= m * inside;
    }
    // Alpha test: returns the linear texel, not `rgb`, because it draws only into the minimap's
    // linear 256² composite, whose blit encodes.
    if alpha_ref > 0.0 {
        if t.a * c.a < alpha_ref {
            discard;
        }
        return vec4<f32>(srgb_to_linear(t.rgb) * c.rgb, 1.0);
    }
    let a = t.a * k;
    // Premultiply in gamma, not by the hardware `SrcAlpha`. A booth bake takes `k` alone: `t.a`
    // again would zero its additive light over empty space.
    let weight = select(a, k, premultiplied != 0u);
    if additive != 0u {
        return vec4<f32>(rgb * weight, 0.0);
    }
    return vec4<f32>(rgb * weight, a);
}
