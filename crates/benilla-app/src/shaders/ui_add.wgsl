// `alphaMode="ADD"` for the glue highlight overlays: samples the sub-rect `rect` (uv min/max) and
// returns gamma bytes, so the `SrcAlpha, One` blend is the reference's byte add `dst + texel * a`
// (EGxBlend 3, `glBlendFunc(GL_SRC_ALPHA, GL_ONE)`).
#import bevy_ui::ui_vertex_output::UiVertexOutput

@group(1) @binding(0) var texture: texture_2d<f32>;
@group(1) @binding(1) var texture_sampler: sampler;
@group(1) @binding(2) var<uniform> rect: vec4<f32>;

// Linear to sRGB (IEC 61966-2-1), the exact inverse of the sampler's sRGB decode.
fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let higher = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    let lower = c * 12.92;
    return select(higher, lower, c <= vec3<f32>(0.0031308));
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let uv = mix(rect.xy, rect.zw, in.uv);
    let texel = textureSample(texture, texture_sampler, uv);
    return vec4<f32>(linear_to_srgb(texel.rgb), texel.a);
}
