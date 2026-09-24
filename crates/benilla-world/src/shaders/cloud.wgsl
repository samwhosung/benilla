// The cloud dome. Its colour is CPU-side as in the reference: the kernel's `0x6cfb00` port builds
// the image the reference binds as its texture (`0x58ac70`). This stage applies the dome's rim
// fade (`0x6d0530`) and blends the raw gamma texels premultiplied. Depth is the far pin in
// `sky_vertex.wgsl`.

#import bevy_pbr::forward_io::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var cloud_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var cloud_samp: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(cloud_tex, cloud_samp, in.uv);
    var a = texel.a;
#ifdef VERTEX_COLORS
    a *= in.color.a; // the dome's rim fade (ring alphas)
#endif
    // Premultiplied gamma blend; the RGB is already the reference's byte math.
    return vec4<f32>(texel.rgb * a, a);
}
