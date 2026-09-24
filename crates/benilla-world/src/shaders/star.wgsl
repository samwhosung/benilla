// Night-sky stars (`Stars.m2`): white `Stars.blp` dots blended as premultiplied white in gamma
// space, as the reference blends them. Depth is the far pin in `sky_vertex.wgsl`.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::VertexOutput,
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let pbr_input = pbr_input_from_standard_material(in, is_front);
    let a = pbr_input.material.base_color.a; // texel alpha × the per-frame `base_color` alpha
    return vec4<f32>(vec3<f32>(a), a); // premultiplied white, raw gamma
}
