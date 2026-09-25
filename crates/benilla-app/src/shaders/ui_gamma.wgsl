// The UI lane's one gamma decode, after the display-gamma ramp: the sampled value is the byte the
// reference's RAMDAC would read, and `srgb_to_linear` re-encodes it to the exact swapchain byte.
// Alpha is coverage and passes through.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;
// `.x` = the `gamma` CVar; the rest is padding (a uniform is 16-byte aligned).
@group(0) @binding(2) var<uniform> ramp: vec4<f32>;

// sRGB to linear (IEC 61966-2-1), the exact inverse of `ui_quad.wgsl`'s `linear_to_srgb`.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let higher = pow((max(c, vec3<f32>(0.0)) + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    let lower = c / 12.92;
    return select(higher, lower, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_decode(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let ui = textureSample(screen_texture, screen_sampler, in.uv);
    var rgb = ui.rgb;
    // The reference's hardware ramp `pow(i / 255, gamma)` (`0x591680`). The branch keeps gamma 1
    // an exact identity and the floor guards `log2(0)` inside `pow`.
    if (ramp.x != 1.0) {
        rgb = pow(max(rgb, vec3<f32>(1e-6)), vec3<f32>(ramp.x));
    }
    return vec4<f32>(srgb_to_linear(rgb), ui.a);
}
