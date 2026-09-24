// Celestial discs (sun, white moon, moon02) and their glares, blended in gamma space as the
// reference blends onto gamma bytes. Discs take its horizon clip and fade (`0x6d1960`): clipped
// at height 0 with alpha 0 there, `clamp(2.5 * height, 0, 1)` in the 0.4-unit band above
// (`0x6d1ac5`), the colour's alpha byte beyond. Glares skip the clip (`0x7e57e0`) and add,
// saturating, like the reference's SRC_ALPHA, ONE blend (`0x7e5a16`). Depth is the far pin in
// `sky_vertex.wgsl`.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}

// `CelestialExt::fade`: `.x` the ramp slope in sin-elevation (`2.5 * height` at the reference's
// radius 12 is `30 * y`), `.y` the brightness on RGB only, `.z` 0 for a disc and 1 for a glare,
// `.w` the colour's alpha byte above the band, 0 for moon02 in clear weather (no writer).
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> celestial: vec4<f32>;
// The disc quad's vertical span in sin-elevation, `.x` bottom edge and `.y` top edge, per frame.
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var<uniform> span: vec4<f32>;

// The reference's per-vertex alpha at sin-elevation `y`: the ramp inside the band, the byte above.
fn vertex_alpha(y: f32) -> f32 {
    let ramp = y * celestial.x;
    return select(celestial.w, clamp(ramp, 0.0, 1.0), ramp < 1.0);
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let base = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var a = base.a;
    if celestial.z < 0.5 {
        // Disc: the vertex rule, interpolated from the clipped bottom edge to the top. The
        // reference's GEQUAL 1/255 alpha test needs no discard: zero alpha adds nothing here.
        let dir = normalize(in.world_position.xyz - view.world_position.xyz);
        let y = dir.y;
        if y <= 0.0 {
            a = 0.0;
        } else {
            let y_bot = max(span.x, 0.0); // the horizon cut, or the bottom edge above it
            let a_bot = vertex_alpha(y_bot);
            let a_top = vertex_alpha(span.y);
            let t = clamp((y - y_bot) / max(span.y - y_bot, 1e-6), 0.0, 1.0);
            a *= mix(a_bot, a_top, t);
        }
    }

    // `base.rgb` arrives linear (Bevy decodes the sRGB texture); the target takes gamma values.
    let gamma = linear_to_srgb(base.rgb);
    if celestial.z >= 0.5 {
        // Glare: alpha 0 under `AlphaMode::Add` gives `dst + gamma * a`, `a` the flare envelope.
        return vec4<f32>(gamma * a * celestial.y, 0.0);
    }
    // Disc: premultiplied gamma under `AlphaMode::Premultiplied`.
    return vec4<f32>(gamma * a * celestial.y, a);
}
